//! SPEC §5 — `@file:` include grafting and `@@` escaping.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::{Error, ErrorCode};
use crate::parse;
use crate::pointer::escape_token;
use crate::source::{include_target, RawGraft};
use crate::Loader;

pub(crate) const INCLUDE_PREFIX: &str = "@file:";

/// Whether an authored string is an `@file:` reference (not an `@@` escape, a
/// literal, or a reserved directive).
pub(crate) fn is_include(s: &str) -> bool {
    s.starts_with(INCLUDE_PREFIX)
}

/// The resolver's position: the file whose value is being walked (lexical
/// path) and its directory, the pointer within that file and within the
/// effective tree, the canonical paths of the files being resolved (for cycle
/// detection, innermost last), and the `@file:` references traversed so far
/// (SPEC §10.3's chain, outermost first).
pub(crate) struct Walk {
    pub(crate) doc: PathBuf,
    pub(crate) dir: PathBuf,
    pub(crate) src: String,
    pub(crate) eff: String,
    pub(crate) stack: Vec<PathBuf>,
    pub(crate) chain: Vec<(PathBuf, String)>,
}

impl Walk {
    fn step(&self, token: &str) -> Walk {
        let suffix = format!("/{}", escape_token(token));
        Walk {
            doc: self.doc.clone(),
            dir: self.dir.clone(),
            src: format!("{}{suffix}", self.src),
            eff: format!("{}{suffix}", self.eff),
            stack: self.stack.clone(),
            chain: self.chain.clone(),
        }
    }
}

/// Resolves every `@file:` reference in `value` (SPEC §5).
pub(crate) fn resolve(l: &Loader<'_>, value: Value, w: &Walk) -> Result<Value, Error> {
    match value {
        Value::String(s) => resolve_string(l, s, w),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.into_iter().enumerate() {
                out.push(resolve(l, item, &w.step(&i.to_string()))?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(object) => {
            let mut out = Map::new();
            for (key, item) in object {
                let resolved = resolve(l, item, &w.step(&key))?;
                out.insert(key, resolved);
            }
            Ok(Value::Object(out))
        }
        scalar => Ok(scalar),
    }
}

fn resolve_string(l: &Loader<'_>, s: String, w: &Walk) -> Result<Value, Error> {
    // `@@` is the escape and must be tested before the include prefix.
    if let Some(rest) = s.strip_prefix("@@") {
        return Ok(Value::String(format!("@{rest}")));
    }
    if let Some(target) = s.strip_prefix(INCLUDE_PREFIX) {
        return graft(l, target, w);
    }
    if s.starts_with('@') {
        return Err(Error::new(
            ErrorCode::Substitution,
            format!("{s:?} starts with `@` but is not an include; the `@` namespace is reserved (write `@@` for a literal `@`)"),
        ));
    }
    Ok(Value::String(s))
}

fn graft(l: &Loader<'_>, target: &str, w: &Walk) -> Result<Value, Error> {
    if target.is_empty() {
        return Err(Error::new(
            ErrorCode::Include,
            "`@file:` with an empty path".to_string(),
        ));
    }
    let joined = include_target(target, &w.dir);

    let Some(format) = parse::format_for_path(&joined) else {
        return Err(Error::new(
            ErrorCode::Include,
            format!(
                "include {target:?}: unsupported extension (want .json, .yaml, .yml, or .toml)"
            ),
        ));
    };

    // Cycle detection compares canonical paths, so a file reached under two
    // names (a symlink, a `..` detour) is still one file.
    let canonical = fs::canonicalize(&joined).map_err(|e| {
        Error::new(
            ErrorCode::Include,
            format!("include {target:?} ({}): {e}", joined.display()),
        )
    })?;
    if !l.src.is_file(&joined) {
        return Err(Error::new(
            ErrorCode::Include,
            format!(
                "include {target:?} ({}) is not a regular file",
                joined.display()
            ),
        ));
    }

    if let Some(at) = w.stack.iter().position(|p| *p == canonical) {
        let mut chain: Vec<String> = w.stack[at..]
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        chain.push(canonical.display().to_string());
        return Err(Error::new(
            ErrorCode::IncludeCycle,
            format!("include cycle: {}", chain.join(" -> ")),
        ));
    }

    let bytes = l.src.read_file(&joined).map_err(|e| {
        Error::new(
            ErrorCode::Include,
            format!("include {target:?} ({}): {e}", joined.display()),
        )
    })?;
    let text = String::from_utf8(bytes.clone()).map_err(|_| {
        Error::new(
            ErrorCode::Parse,
            format!("{} is not valid UTF-8", joined.display()),
        )
    })?;
    let tree = parse::parse(&text, format, &joined)?;
    let mut refs = w.chain.clone();
    refs.push((w.doc.clone(), w.src.clone()));
    if let Some(rec) = l.rec {
        rec.record(&joined, &bytes, format.into(), Some(tree.clone()));
        rec.graft(
            &joined,
            RawGraft {
                effective: w.eff.clone(),
                chain: refs.clone(),
            },
        );
    }

    let parent = joined
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let mut stack = w.stack.clone();
    stack.push(canonical);
    let next = Walk {
        doc: joined,
        dir: parent,
        src: String::new(),
        eff: w.eff.clone(),
        stack,
        chain: refs,
    };
    resolve(l, tree, &next)
}
