//! SPEC §4 — the variable namespace: `*.env` files plus the process environment.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, ErrorCode};

/// The variable namespace. Process environment shadows `*.env` definitions.
pub(crate) struct Vars<'a> {
    process: &'a BTreeMap<String, String>,
    files: BTreeMap<String, String>,
}

impl<'a> Vars<'a> {
    pub(crate) fn new(
        process: &'a BTreeMap<String, String>,
        files: BTreeMap<String, String>,
    ) -> Self {
        Vars { process, files }
    }

    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.process
            .get(name)
            .or_else(|| self.files.get(name))
            .map(String::as_str)
    }
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
/// two — is `E_ENV_CONFLICT`.
pub(crate) fn load_dir(dir: &Path) -> Result<BTreeMap<String, String>, Error> {
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries {
            let entry = entry.map_err(|e| {
                Error::new(
                    ErrorCode::Parse,
                    format!("cannot read directory {}: {e}", dir.display()),
                )
            })?;
            let path = entry.path();
            let is_env = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".env"));
            if is_env && path.is_file() {
                paths.push(path);
            }
        }
    }
    // Deterministic conflict reporting; the files themselves stay unordered peers.
    paths.sort();

    let mut values: BTreeMap<String, String> = BTreeMap::new();
    let mut origins: BTreeMap<String, PathBuf> = BTreeMap::new();

    for path in &paths {
        let text = read_utf8(path)?;
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
            if let Some(previous) = origins.get(name) {
                let detail = if previous == path {
                    format!(
                        "variable {name:?} is defined twice in {} (again at line {lineno})",
                        path.display()
                    )
                } else {
                    format!(
                        "variable {name:?} is defined in both {} and {}:{lineno}",
                        previous.display(),
                        path.display()
                    )
                };
                return Err(Error::new(ErrorCode::EnvConflict, detail));
            }
            origins.insert(name.to_string(), path.clone());
            values.insert(name.to_string(), value);
        }
    }

    Ok(values)
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

fn read_utf8(path: &Path) -> Result<String, Error> {
    let bytes = fs::read(path).map_err(|e| {
        Error::new(
            ErrorCode::Parse,
            format!("cannot read {}: {e}", path.display()),
        )
    })?;
    String::from_utf8(bytes).map_err(|_| {
        Error::new(
            ErrorCode::Parse,
            format!("{} is not valid UTF-8", path.display()),
        )
    })
}
