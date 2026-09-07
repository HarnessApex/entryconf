//! SPEC §10 — snapshots, provenance, plans, and commits.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::envfile::{self, VarOrigin, Vars};
use crate::error::{Error, ErrorCode};
use crate::include::{is_include, INCLUDE_PREFIX};
use crate::jsonfmt;
use crate::lock;
use crate::pointer;
use crate::source::{
    document_key, include_target, lexical_clean, DocRecord, FileSource, OsSource, OverlaySource,
    Recorder,
};
use crate::{Loader, ENTRYPOINTS};

/// The live process environment, consulted when a plan is committed.
type LiveEnv = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// A source document's format (SPEC §10.2). Only [`Format::Json`] is writable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// A `.json` document.
    Json,
    /// A `.yaml` or `.yml` document.
    Yaml,
    /// A `.toml` document.
    Toml,
    /// A `*.env` variable file.
    Env,
}

impl Format {
    /// The wire form: `"json"`, `"yaml"`, `"toml"`, or `"env"`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::Yaml => "yaml",
            Format::Toml => "toml",
            Format::Env => "env",
        }
    }

    /// Whether documents of this format can be edited (SPEC §10.1).
    #[must_use]
    pub fn writable(self) -> bool {
        self == Format::Json
    }
}

/// One source file of a [`Snapshot`] (SPEC §10.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Document {
    /// The document key: its path relative to the config directory,
    /// `/`-separated. Keys identify documents in every other operation.
    pub key: String,
    /// The absolute path the document was read from.
    pub path: PathBuf,
    /// The document's format.
    pub format: Format,
    /// `sha256:` plus the lowercase hex SHA-256 of the file's bytes.
    pub revision: String,
    /// True iff `format` is JSON.
    pub writable: bool,
    /// Every position of the effective tree the document's root occupies,
    /// sorted by effective pointer. A `*.env` file has none.
    pub grafts: Vec<Graft>,
}

/// One position a document's root value occupies in the effective tree, with
/// the `@file:` references traversed to reach it, outermost first (SPEC §10.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Graft {
    /// The effective pointer of the grafted root.
    pub effective: String,
    /// The references from the entrypoint down to this graft.
    pub chain: Vec<Reference>,
}

/// An `@file:` string: the document holding it and its pointer there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reference {
    /// The key of the document holding the reference.
    pub document: String,
    /// The reference's source pointer within that document.
    pub pointer: String,
}

/// The provenance of one effective value (SPEC §10.3).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Origin {
    /// The effective pointer asked about.
    pub effective: String,
    /// The key of the document whose authored value produces the effective one.
    pub document: String,
    /// The source pointer of the authored value within `document`.
    pub pointer: String,
    /// The authored value, as parsed, before includes and interpolation.
    pub authored: Value,
    /// The references traversed from the entrypoint to `document`.
    pub chain: Vec<Reference>,
    /// The `$` references the authored string makes, in order of appearance.
    pub variables: Vec<Variable>,
    /// Whether `document` is writable.
    pub writable: bool,
}

/// Which SPEC §4 layer supplies a variable's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VariableOrigin {
    /// The process environment.
    Process,
    /// A `*.env` file (named in [`Variable::file`]).
    File,
    /// The variable is unset and the `${NAME:-default}` default applies.
    Default,
}

/// One `$` reference of an authored string and where its value comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Variable {
    /// The variable name.
    pub name: String,
    /// The layer supplying its value.
    pub origin: VariableOrigin,
    /// The `*.env` document key, when `origin` is [`VariableOrigin::File`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// An edit operation (SPEC §10.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    /// Add or replace the value at the pointer.
    Set,
    /// Delete the member or element at the pointer.
    Remove,
}

/// How a `set` value is authored (SPEC §10.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Strings are escaped so they load back as themselves.
    #[default]
    Literal,
    /// A string is written verbatim as an entryconf expression.
    Expression,
}

/// One operation on one source document (SPEC §10.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edit {
    /// The key of the document to edit. Required; there is no default.
    pub document: String,
    /// `set` or `remove`.
    pub op: Op,
    /// An RFC 6901 pointer within `document`; `""` is the root.
    pub pointer: String,
    /// The value to set (`Some(Value::Null)` sets JSON null); ignored for
    /// `remove`.
    #[serde(default, deserialize_with = "present_value")]
    pub value: Option<Value>,
    /// Literal (the default) or expression.
    #[serde(default)]
    pub mode: Mode,
}

/// A `value` member that is present — even as `null` — is `Some`.
fn present_value<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

/// Decodes the `edits` array of a SPEC §11 request (`[{"document", "op",
/// "pointer", "value"?, "mode"?}, …]`) into [`Edit`]s, reporting a malformed
/// entry — a missing or non-string `document`/`op`/`pointer`, an unknown `op`
/// or `mode` — as `E_EDIT` rather than as a decoding fault, so a CLI can give
/// it the library's verdict.
///
/// # Errors
///
/// `E_EDIT` for anything that is not a well-formed edit list.
pub fn parse_edits(edits: &Value) -> Result<Vec<Edit>, Error> {
    let bad = |detail: String| Error::new(ErrorCode::Edit, detail);
    let Value::Array(items) = edits else {
        return Err(bad("\"edits\" must be an array".to_string()));
    };
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let Value::Object(map) = item else {
            return Err(bad(format!("edit {i} is not an object")));
        };
        let field = |name: &str| -> Result<String, Error> {
            match map.get(name) {
                Some(Value::String(s)) => Ok(s.clone()),
                _ => Err(bad(format!("edit {i} lacks a string {name:?}"))),
            }
        };
        let op = match field("op")?.as_str() {
            "set" => Op::Set,
            "remove" => Op::Remove,
            other => return Err(bad(format!("edit {i}: unknown op {other:?} (want \"set\" or \"remove\")"))),
        };
        let mode = match map.get("mode") {
            None => Mode::Literal,
            Some(Value::String(m)) if m == "literal" => Mode::Literal,
            Some(Value::String(m)) if m == "expression" => Mode::Expression,
            Some(other) => return Err(bad(format!("edit {i}: unknown mode {other}"))),
        };
        out.push(Edit {
            document: field("document")?,
            op,
            pointer: field("pointer")?,
            value: map.get("value").cloned(),
            mode,
        });
    }
    Ok(out)
}

impl Edit {
    /// A literal `set`: `value` loads back as itself.
    #[must_use]
    pub fn set(document: &str, pointer: &str, value: Value) -> Edit {
        Edit {
            document: document.to_string(),
            op: Op::Set,
            pointer: pointer.to_string(),
            value: Some(value),
            mode: Mode::Literal,
        }
    }

    /// An expression `set`: `expression` is written verbatim and interpreted
    /// by the load (`$VAR`, `@file:…`).
    #[must_use]
    pub fn set_expression(document: &str, pointer: &str, expression: &str) -> Edit {
        Edit {
            document: document.to_string(),
            op: Op::Set,
            pointer: pointer.to_string(),
            value: Some(Value::String(expression.to_string())),
            mode: Mode::Expression,
        }
    }

    /// A `remove`.
    #[must_use]
    pub fn remove(document: &str, pointer: &str) -> Edit {
        Edit {
            document: document.to_string(),
            op: Op::Remove,
            pointer: pointer.to_string(),
            value: None,
            mode: Mode::Literal,
        }
    }
}

/// What [`open`] returns (SPEC §10.2): the effective tree, every source
/// document the load read, and the process environment as captured at open
/// time. Immutable; reads nothing from disk after `open` returns.
#[derive(Serialize)]
pub struct Snapshot {
    /// The absolute path of the config directory.
    pub dir: PathBuf,
    /// The effective tree — exactly what [`crate::load`] returns.
    pub tree: Value,
    /// Every document the load read, by key.
    pub documents: BTreeMap<String, Document>,
    #[serde(skip)]
    by_path: BTreeMap<PathBuf, DocRecord>,
    #[serde(skip)]
    env_names: Vec<String>,
    #[serde(skip)]
    captured: BTreeMap<String, String>,
    #[serde(skip)]
    live: LiveEnv,
}

/// A prepared, validated change to one document (SPEC §10.6). Nothing has been
/// written; [`Plan::commit`] writes `after` if every dependency is unchanged.
#[derive(Serialize)]
pub struct Plan {
    /// The edited document's key.
    pub document: String,
    /// The document's text as the snapshot holds it.
    pub before: String,
    /// The document's new text (SPEC §10.8).
    pub after: String,
    /// The candidate effective tree: what the directory loads to after commit.
    pub candidate: Value,
    /// The effective pointers where `candidate` differs from the snapshot.
    pub affected: Vec<String>,
    /// The edited document's grafts — every effective position the change lands in.
    pub grafts: Vec<Graft>,
    /// The revision of every document the candidate load read.
    pub revisions: BTreeMap<String, String>,
    /// The names of every variable the candidate load looked up.
    pub variables: Vec<String>,
    /// The fingerprint of those variables' values (SPEC §10.6).
    pub variables_revision: String,
    #[serde(skip)]
    dir: PathBuf,
    #[serde(skip)]
    path: PathBuf,
    #[serde(skip)]
    paths: BTreeMap<String, PathBuf>,
    #[serde(skip)]
    live: LiveEnv,
}

/// What a successful [`Plan::commit`] returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Receipt {
    /// The written document's key.
    pub document: String,
    /// Its new revision.
    pub revision: String,
    /// The plan's revisions, with the written document's updated.
    pub revisions: BTreeMap<String, String>,
}

/// Tunes [`Plan::commit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitOptions {
    /// How long to wait for the directory lock before `E_LOCKED`.
    pub lock_timeout: Duration,
}

impl Default for CommitOptions {
    fn default() -> Self {
        CommitOptions {
            lock_timeout: Duration::from_secs(5),
        }
    }
}

/// Loads `dir` like [`crate::load`] and returns a [`Snapshot`] for inspecting
/// provenance and preparing edits (SPEC §10.2). The process environment is
/// captured now; plans resolve variables against that capture, and a commit
/// fails with `E_STALE_PLAN` if a variable the candidate depends on has since
/// changed.
///
/// # Errors
///
/// Any load failure, with its SPEC §7 code.
pub fn open(dir: &Path) -> Result<Snapshot, Error> {
    let captured = crate::process_environment();
    open_with_env(dir, &captured, |name| std::env::var(name).ok())
}

/// Like [`open`], with the captured and live environments supplied explicitly.
/// The seam the editing harness uses; not part of the stable surface.
#[doc(hidden)]
pub fn open_with_env(
    dir: &Path,
    captured: &BTreeMap<String, String>,
    live: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
) -> Result<Snapshot, Error> {
    let abs = if dir.is_absolute() {
        lexical_clean(dir)
    } else {
        let cwd = std::env::current_dir().map_err(|e| {
            Error::new(
                ErrorCode::NoEntrypoint,
                format!("cannot resolve {}: {e}", dir.display()),
            )
        })?;
        lexical_clean(&cwd.join(dir))
    };
    // A failure to list surfaces from the load itself (SPEC §3).
    let mut env_names = OsSource.env_file_names(&abs).unwrap_or_default();
    env_names.sort();

    let rec = Recorder::default();
    let lookup = |name: &str| captured.get(name).cloned();
    let loader = Loader {
        src: &OsSource,
        process: &lookup,
        rec: Some(&rec),
    };
    let (tree, _) = loader.load_tree(&abs)?;
    let (order, by_path) = rec.into_docs();

    let mut documents = BTreeMap::new();
    for path in order {
        let d = &by_path[&path];
        let key = document_key(&abs, &path);
        documents.insert(
            key.clone(),
            Document {
                key,
                path: path.clone(),
                format: d.format,
                revision: revision_of(&d.data),
                writable: d.format.writable(),
                grafts: keyed_grafts(&abs, d),
            },
        );
    }
    Ok(Snapshot {
        dir: abs,
        tree,
        documents,
        by_path,
        env_names,
        captured: captured.clone(),
        live: Arc::new(live),
    })
}

/// The resolver records references by path; the API speaks in keys.
fn keyed_grafts(dir: &Path, d: &DocRecord) -> Vec<Graft> {
    let mut grafts: Vec<Graft> = d
        .grafts
        .iter()
        .map(|g| Graft {
            effective: g.effective.clone(),
            chain: g
                .chain
                .iter()
                .map(|(path, pointer)| Reference {
                    document: document_key(dir, path),
                    pointer: pointer.clone(),
                })
                .collect(),
        })
        .collect();
    grafts.sort_by(|a, b| a.effective.cmp(&b.effective));
    grafts
}

/// SPEC §10.2's revision: `sha256:` + hex(SHA-256(bytes)).
pub(crate) fn revision_of(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

impl Snapshot {
    /// Reports where the effective value at `pointer` comes from (SPEC §10.3).
    ///
    /// # Errors
    ///
    /// `E_PATH` when the pointer is malformed or does not resolve.
    pub fn inspect(&self, pointer: &str) -> Result<Origin, Error> {
        let tokens = pointer::parse(pointer)?;
        let mut doc = self.entrypoint_record()?;
        let mut node = doc.parsed.as_ref().expect("entrypoint is parsed");
        let mut src = String::new();
        let mut chain: Vec<Reference> = Vec::new();

        for token in &tokens {
            self.follow_includes(&mut doc, &mut node, &mut src, &mut chain)?;
            node = match node {
                Value::Object(map) => map.get(token).ok_or_else(|| {
                    Error::new(
                        ErrorCode::Path,
                        format!("effective pointer {pointer:?}: no member {token:?}"),
                    )
                })?,
                Value::Array(items) => pointer::array_index(token)
                    .and_then(|i| items.get(i))
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::Path,
                            format!(
                                "effective pointer {pointer:?}: array index {token:?} does not exist"
                            ),
                        )
                    })?,
                _ => {
                    return Err(Error::new(
                        ErrorCode::Path,
                        format!(
                            "effective pointer {pointer:?}: cannot descend into a scalar at {token:?}"
                        ),
                    ))
                }
            };
            src.push('/');
            src.push_str(&pointer::escape_token(token));
        }
        self.follow_includes(&mut doc, &mut node, &mut src, &mut chain)?;

        let key = document_key(&self.dir, &doc.path);
        let variables = match node {
            Value::String(s) => self.variables_of(s),
            _ => Vec::new(),
        };
        Ok(Origin {
            effective: pointer.to_string(),
            document: key.clone(),
            pointer: src,
            authored: node.clone(),
            chain,
            variables,
            writable: self.documents.get(&key).is_some_and(|d| d.writable),
        })
    }

    /// Follows an `@file:` reference at the current walk position, repeatedly,
    /// appending each traversed reference to `chain` (SPEC §10.3).
    fn follow_includes<'s>(
        &'s self,
        doc: &mut &'s DocRecord,
        node: &mut &'s Value,
        src: &mut String,
        chain: &mut Vec<Reference>,
    ) -> Result<(), Error> {
        loop {
            let Value::String(s) = *node else { return Ok(()) };
            if !is_include(s) {
                return Ok(());
            }
            let base = doc
                .path
                .parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
            let target = include_target(&s[INCLUDE_PREFIX.len()..], &base);
            let Some(next) = self.by_path.get(&target) else {
                return Err(Error::new(
                    ErrorCode::Path,
                    format!(
                        "include {} was not loaded by this snapshot",
                        target.display()
                    ),
                ));
            };
            chain.push(Reference {
                document: document_key(&self.dir, &doc.path),
                pointer: src.clone(),
            });
            *doc = next;
            *node = next.parsed.as_ref().expect("included documents are parsed");
            src.clear();
        }
    }

    /// The `$` references of an authored string, in order, with the SPEC §4
    /// layer that supplies each.
    fn variables_of(&self, authored: &str) -> Vec<Variable> {
        let src = self.source(BTreeMap::new());
        let Ok(files) = envfile::load_dir(&self.dir, &src, |_, _| {}) else {
            return Vec::new(); // the snapshot loaded, so this cannot happen
        };
        let lookup = |name: &str| self.captured.get(name).cloned();
        let vars = Vars::new(&lookup, files);
        scan_references(authored)
            .into_iter()
            .map(|(name, has_default)| match vars.origin(&name) {
                VarOrigin::Process => Variable {
                    name,
                    origin: VariableOrigin::Process,
                    file: None,
                },
                VarOrigin::File(path) => Variable {
                    name,
                    origin: VariableOrigin::File,
                    file: Some(document_key(&self.dir, path)),
                },
                // A reference with no default and no value would have failed
                // the load, so this is the default case.
                VarOrigin::Unset => {
                    let _ = has_default;
                    Variable {
                        name,
                        origin: VariableOrigin::Default,
                        file: None,
                    }
                }
            })
            .collect()
    }

    fn entrypoint_record(&self) -> Result<&DocRecord, Error> {
        ENTRYPOINTS
            .iter()
            .map(|name| lexical_clean(&self.dir.join(name)))
            .find_map(|path| self.by_path.get(&path).filter(|d| d.parsed.is_some()))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::NoEntrypoint,
                    format!("snapshot of {} has no entrypoint", self.dir.display()),
                )
            })
    }

    /// The overlay a candidate is evaluated against: the snapshot's bytes with
    /// `overrides` replacing documents in memory, and disk for anything else.
    fn source(&self, overrides: BTreeMap<PathBuf, Vec<u8>>) -> OverlaySource {
        let mut files: BTreeMap<PathBuf, Vec<u8>> = self
            .by_path
            .iter()
            .map(|(path, d)| (path.clone(), d.data.clone()))
            .collect();
        files.extend(overrides);
        OverlaySource {
            files,
            env_names: self.env_names.clone(),
        }
    }

    /// Validates `edits` (SPEC §10.4), applies them to a copy of the selected
    /// document (SPEC §10.5), and evaluates the candidate configuration with
    /// the new text substituted in memory (SPEC §10.6). No file is changed.
    ///
    /// # Errors
    ///
    /// `E_EDIT`, `E_UNSUPPORTED_EDIT`, or `E_PATH` for a bad request; a
    /// candidate that fails to load fails the plan with that load's code.
    pub fn plan(&self, edits: &[Edit]) -> Result<Plan, Error> {
        let Some(first) = edits.first() else {
            return Err(Error::new(
                ErrorCode::Edit,
                "a plan needs at least one edit".to_string(),
            ));
        };
        let key = &first.document;
        if let Some(other) = edits.iter().find(|e| e.document != *key) {
            return Err(Error::new(
                ErrorCode::UnsupportedEdit,
                format!(
                    "edits name both {key:?} and {:?}; a plan edits exactly one document",
                    other.document
                ),
            ));
        }
        let Some(doc) = self.documents.get(key) else {
            return Err(Error::new(
                ErrorCode::Edit,
                format!("no document {key:?} in the snapshot of {}", self.dir.display()),
            ));
        };
        if !doc.writable {
            return Err(Error::new(
                ErrorCode::UnsupportedEdit,
                format!(
                    "document {key:?} is {}; only JSON documents are writable",
                    doc.format.as_str()
                ),
            ));
        }
        let rec = &self.by_path[&doc.path];

        let mut value = rec.parsed.clone().expect("a JSON document is parsed");
        for (i, e) in edits.iter().enumerate() {
            match e.op {
                Op::Set => {
                    let Some(v) = e.value.clone() else {
                        return Err(Error::new(
                            ErrorCode::Edit,
                            format!("edit {i}: set needs a value"),
                        ));
                    };
                    let v = match e.mode {
                        Mode::Literal => escape_literal(v),
                        Mode::Expression => {
                            if !v.is_string() {
                                return Err(Error::new(
                                    ErrorCode::Edit,
                                    format!("edit {i}: expression mode requires a string value"),
                                ));
                            }
                            v
                        }
                    };
                    pointer::set(&mut value, &e.pointer, v)?;
                }
                Op::Remove => pointer::remove(&mut value, &e.pointer)?,
            }
        }
        let after = jsonfmt::format(&value);

        // Evaluate the candidate against the snapshot with the new text in place.
        let cand_rec = Recorder::default();
        let overlay = self.source(BTreeMap::from([(doc.path.clone(), after.clone().into_bytes())]));
        let lookup = |name: &str| self.captured.get(name).cloned();
        let loader = Loader {
            src: &overlay,
            process: &lookup,
            rec: Some(&cand_rec),
        };
        let (candidate, used) = loader.load_tree(&self.dir)?;
        let (order, docs) = cand_rec.into_docs();

        let mut revisions = BTreeMap::new();
        let mut paths = BTreeMap::new();
        for path in order {
            let d = &docs[&path];
            let k = document_key(&self.dir, &path);
            revisions.insert(k.clone(), revision_of(&d.data));
            paths.insert(k, path);
        }
        // The edited document's own revision is what is on disk now, not the
        // new text: commit must find the file as the snapshot saw it.
        revisions.insert(key.clone(), doc.revision.clone());

        // Fingerprint the variables the candidate looked up (SPEC §10.6).
        let files = envfile::load_dir(&self.dir, &overlay, |_, _| {})?;
        let vars = Vars::new(&lookup, files);
        let variables: Vec<String> = used.into_iter().collect::<BTreeSet<_>>().into_iter().collect();
        let variables_revision = variables_revision(&variables, &vars);

        let affected = affected_pointers(&self.tree, &candidate);
        Ok(Plan {
            document: key.clone(),
            before: String::from_utf8_lossy(&rec.data).into_owned(),
            after,
            candidate,
            affected,
            grafts: doc.grafts.clone(),
            revisions,
            variables,
            variables_revision,
            dir: self.dir.clone(),
            path: doc.path.clone(),
            paths,
            live: self.live.clone(),
        })
    }
}

/// SPEC §10.6's fingerprint over the named variables.
fn variables_revision(names: &[String], vars: &Vars<'_>) -> String {
    let mut text = String::new();
    for name in names {
        match vars.get(name) {
            Some(value) => text.push_str(&format!("={name}={value}\n")),
            None => text.push_str(&format!("-{name}\n")),
        }
    }
    revision_of(text.as_bytes())
}

impl Plan {
    /// Writes `after` to the plan's document (SPEC §10.7): lock, recheck every
    /// dependency's revision and the variables fingerprint, write atomically,
    /// unlock.
    ///
    /// # Errors
    ///
    /// `E_LOCKED` if the lock is not acquired within `opts.lock_timeout`;
    /// `E_STALE_PLAN` if any dependency changed; `E_WRITE` if the replacement
    /// could not be written (the target is unchanged).
    pub fn commit(&self, opts: &CommitOptions) -> Result<Receipt, Error> {
        let _guard = lock::acquire(&self.dir, opts.lock_timeout)?;

        let mut files = envfile::EnvFiles::default();
        for (key, want) in &self.revisions {
            let path = &self.paths[key];
            let data = fs::read(path).map_err(|e| {
                Error::new(
                    ErrorCode::StalePlan,
                    format!("document {key:?} can no longer be read: {e}"),
                )
            })?;
            let got = revision_of(&data);
            if got != *want {
                return Err(Error::new(
                    ErrorCode::StalePlan,
                    format!(
                        "document {key:?} changed since the plan was made ({got}, plan expected {want})"
                    ),
                ));
            }
            if key.ends_with(".env") {
                let text = String::from_utf8_lossy(&data);
                for (name, value) in envfile::parse_env_file(&text, path)
                    .map_err(|e| Error::new(ErrorCode::StalePlan, e.message().to_string()))?
                {
                    files.origins.insert(name.clone(), path.clone());
                    files.values.insert(name, value);
                }
            }
        }
        let live = self.live.clone();
        let lookup = move |name: &str| live(name);
        let vars = Vars::new(&lookup, files);
        if variables_revision(&self.variables, &vars) != self.variables_revision {
            return Err(Error::new(
                ErrorCode::StalePlan,
                format!(
                    "a variable the candidate depends on changed since the plan was made ({:?})",
                    self.variables
                ),
            ));
        }

        lock::atomic_write(&self.path, self.after.as_bytes())?;
        let mut revisions = self.revisions.clone();
        let revision = revision_of(self.after.as_bytes());
        revisions.insert(self.document.clone(), revision.clone());
        Ok(Receipt {
            document: self.document.clone(),
            revision,
            revisions,
        })
    }
}

/// Literal mode (SPEC §10.5): every string, recursively but never a key, has
/// each `$` doubled and then a leading `@` doubled, so it loads back as itself.
fn escape_literal(value: Value) -> Value {
    match value {
        Value::String(s) => {
            let mut out = s.replace('$', "$$");
            if out.starts_with('@') {
                out.insert(0, '@');
            }
            Value::String(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(escape_literal).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k, escape_literal(v));
            }
            Value::Object(out)
        }
        other => other,
    }
}

/// The `$` references of a well-formed authored string (name, has_default),
/// in order of appearance.
fn scan_references(s: &str) -> Vec<(String, bool)> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' || i + 1 >= bytes.len() {
            i += 1;
            continue;
        }
        match bytes[i + 1] {
            b'$' => i += 2,
            b'{' => {
                let Some(end) = s[i + 2..].find('}').map(|p| i + 2 + p) else {
                    return out;
                };
                let inner = &s[i + 2..end];
                match inner.find(':') {
                    Some(colon) => out.push((inner[..colon].to_string(), true)),
                    None => out.push((inner.to_string(), false)),
                }
                i = end + 1;
            }
            c if c == b'_' || c.is_ascii_alphabetic() => {
                let mut end = i + 1;
                while end < bytes.len() && (bytes[end] == b'_' || bytes[end].is_ascii_alphanumeric())
                {
                    end += 1;
                }
                out.push((s[i + 1..end].to_string(), false));
                i = end;
            }
            _ => i += 1,
        }
    }
    out
}

/// SPEC §10.6's diff: the shallowest effective pointers at which `before` and
/// `after` differ, sorted by code point.
pub(crate) fn affected_pointers(before: &Value, after: &Value) -> Vec<String> {
    let mut out = Vec::new();
    diff_into(before, after, String::new(), &mut out);
    out.sort();
    out
}

fn diff_into(a: &Value, b: &Value, ptr: String, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                let child = format!("{ptr}/{}", pointer::escape_token(k));
                match (x.get(k), y.get(k)) {
                    (Some(av), Some(bv)) => diff_into(av, bv, child, out),
                    _ => out.push(child),
                }
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (av, bv)) in x.iter().zip(y).enumerate() {
                diff_into(av, bv, format!("{ptr}/{i}"), out);
            }
        }
        (Value::Number(x), Value::Number(y)) => {
            if x.as_f64() != y.as_f64() {
                out.push(ptr);
            }
        }
        _ => {
            if a != b {
                out.push(ptr);
            }
        }
    }
}
