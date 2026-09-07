//! SPEC §4 — the variable namespace: `*.env` files plus the process environment.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::error::{Error, ErrorCode};
use crate::source::FileSource;

/// The variable namespace. Process environment shadows `*.env` definitions.
/// Every name looked up is remembered, which is what a plan's `variables`
/// (SPEC §10.6) are built from.
pub(crate) struct Vars<'a> {
    process: &'a dyn Fn(&str) -> Option<String>,
    files: BTreeMap<String, String>,
    origins: BTreeMap<String, PathBuf>,
    used: RefCell<BTreeSet<String>>,
}

impl<'a> Vars<'a> {
    pub(crate) fn new(
        process: &'a dyn Fn(&str) -> Option<String>,
        files: EnvFiles,
    ) -> Self {
        Vars {
            process,
            files: files.values,
            origins: files.origins,
            used: RefCell::new(BTreeSet::new()),
        }
    }

    pub(crate) fn get(&self, name: &str) -> Option<String> {
        self.used.borrow_mut().insert(name.to_string());
        (self.process)(name).or_else(|| self.files.get(name).cloned())
    }

    /// Which SPEC §4 layer supplies `name`: the process environment, a `*.env`
    /// file (with its path), or neither.
    pub(crate) fn origin(&self, name: &str) -> VarOrigin<'_> {
        if (self.process)(name).is_some() {
            return VarOrigin::Process;
        }
        match self.origins.get(name) {
            Some(path) => VarOrigin::File(path),
            None => VarOrigin::Unset,
        }
    }

    pub(crate) fn used(&self) -> Vec<String> {
        self.used.borrow().iter().cloned().collect()
    }
}

pub(crate) enum VarOrigin<'a> {
    Process,
    File(&'a Path),
    Unset,
}

/// The parsed `*.env` peers: values plus the file defining each name.
#[derive(Default)]
pub(crate) struct EnvFiles {
    pub(crate) values: BTreeMap<String, String>,
    pub(crate) origins: BTreeMap<String, PathBuf>,
}

/// A name is `[A-Za-z_][A-Za-z0-9_]*` (SPEC §4, §6).
pub(crate) fn is_var_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    match bytes.next() {
        Some(c) if c == b'_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    bytes.all(|c| c == b'_' || c.is_ascii_alphanumeric())
}

/// Loads every `*.env` file directly in `dir` (non-recursive) into one namespace.
///
/// The files are unordered peers: any name defined twice — in one file or across
/// two — is `E_ENV_CONFLICT`. `seen` is told about each file read.
pub(crate) fn load_dir(
    dir: &Path,
    src: &dyn FileSource,
    mut seen: impl FnMut(&Path, &[u8]),
) -> Result<EnvFiles, Error> {
    let mut paths: Vec<PathBuf> = match src.env_file_names(dir) {
        Ok(names) => names.into_iter().map(|n| dir.join(n)).collect(),
        // A missing directory is reported by the entrypoint search (SPEC §3).
        Err(_) => Vec::new(),
    };
    // Deterministic conflict reporting; the files themselves stay unordered peers.
    paths.sort();

    let mut out = EnvFiles::default();
    for path in &paths {
        let bytes = src.read_file(path).map_err(|e| {
            Error::new(
                ErrorCode::Parse,
                format!("cannot read {}: {e}", path.display()),
            )
        })?;
        let text = String::from_utf8(bytes.clone()).map_err(|_| {
            Error::new(
                ErrorCode::Parse,
                format!("{} is not valid UTF-8", path.display()),
            )
        })?;
        let pairs = parse_env_file(&text, path)?;
        seen(path, &bytes);
        for (name, value) in pairs {
            if let Some(previous) = out.origins.get(&name) {
                let detail = if previous == path {
                    format!(
                        "variable {name:?} is defined twice in {}",
                        path.display()
                    )
                } else {
                    format!(
                        "variable {name:?} is defined in both {} and {}",
                        previous.display(),
                        path.display()
                    )
                };
                return Err(Error::new(ErrorCode::EnvConflict, detail));
            }
            out.origins.insert(name.clone(), path.clone());
            out.values.insert(name, value);
        }
    }

    Ok(out)
}

/// Parses one `*.env` file (SPEC §4) into name/value pairs in file order. A
/// name defined twice within the file is `E_ENV_CONFLICT`.
pub(crate) fn parse_env_file(text: &str, path: &Path) -> Result<Vec<(String, String)>, Error> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for (index, raw) in text.lines().enumerate() {
        let lineno = index + 1;
        let line = if index == 0 {
            raw.strip_prefix('\u{feff}').unwrap_or(raw)
        } else {
            raw
        };
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(eq) = trimmed.find('=') else {
            return Err(Error::new(
                ErrorCode::Parse,
                format!(
                    "{}:{lineno}: not a blank line, comment, or NAME=value",
                    path.display()
                ),
            ));
        };
        // SPEC §4: the whole line was trimmed above, so indentation is
        // fine — but the name is taken verbatim up to the first `=`, so
        // `FOO = bar` leaves a trailing space in the name and is E_PARSE.
        let name = &trimmed[..eq];
        if !is_var_name(name) {
            return Err(Error::new(
                ErrorCode::Parse,
                format!(
                    "{}:{lineno}: invalid variable name {name:?} (want [A-Za-z_][A-Za-z0-9_]*)",
                    path.display()
                ),
            ));
        }
        let value = unquote(trimmed[eq + 1..].trim());
        if let Some(previous) = seen.get(name) {
            return Err(Error::new(
                ErrorCode::EnvConflict,
                format!(
                    "variable {name:?} is defined twice in {} (lines {previous} and {lineno})",
                    path.display()
                ),
            ));
        }
        seen.insert(name.to_string(), lineno);
        out.push((name.to_string(), value));
    }
    Ok(out)
}

/// Strips one layer of matching single or double quotes. No escape processing:
/// the format is a strict subset of dotenv.
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}
