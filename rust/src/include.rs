//! SPEC §5 — `@file:` include grafting and `@@` escaping.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::{Error, ErrorCode};
use crate::parse;

const INCLUDE_PREFIX: &str = "@file:";

/// Resolves every `@file:` reference in `value`.
///
/// `base_dir` is the directory of the file `value` came from — include paths are
/// relative to it, never to the entrypoint or the working directory. `stack` holds
/// the canonical paths of the files currently being resolved, innermost last; a
/// repeat entry is a cycle.
pub(crate) fn resolve(
    value: Value,
    base_dir: &Path,
    stack: &mut Vec<PathBuf>,
) -> Result<Value, Error> {
    match value {
        Value::String(s) => resolve_string(s, base_dir, stack),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(resolve(item, base_dir, stack)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(object) => {
            let mut out = Map::new();
            for (key, item) in object {
                out.insert(key, resolve(item, base_dir, stack)?);
            }
            Ok(Value::Object(out))
        }
        scalar => Ok(scalar),
    }
}

fn resolve_string(s: String, base_dir: &Path, stack: &mut Vec<PathBuf>) -> Result<Value, Error> {
    // `@@` is the escape and must be tested before the include prefix.
    if let Some(rest) = s.strip_prefix("@@") {
        return Ok(Value::String(format!("@{rest}")));
    }
    if let Some(target) = s.strip_prefix(INCLUDE_PREFIX) {
        return graft(target, base_dir, stack);
    }
    if s.starts_with('@') {
        return Err(Error::new(
            ErrorCode::Substitution,
            format!("{s:?} starts with `@` but is not an include; the `@` namespace is reserved (write `@@` for a literal `@`)"),
        ));
    }
    Ok(Value::String(s))
}

fn graft(target: &str, base_dir: &Path, stack: &mut Vec<PathBuf>) -> Result<Value, Error> {
    if target.is_empty() {
        return Err(Error::new(
            ErrorCode::Include,
            "`@file:` with an empty path".to_string(),
        ));
    }
    let joined = base_dir.join(target);

    let Some(format) = parse::format_for_path(&joined) else {
        return Err(Error::new(
            ErrorCode::Include,
            format!(
                "include {target:?}: unsupported extension (want .json, .yaml, .yml, or .toml)"
            ),
        ));
    };

    let canonical = fs::canonicalize(&joined).map_err(|e| {
        Error::new(
            ErrorCode::Include,
            format!("include {target:?} ({}): {e}", joined.display()),
        )
    })?;
    if !canonical.is_file() {
        return Err(Error::new(
            ErrorCode::Include,
            format!(
                "include {target:?} ({}) is not a regular file",
                joined.display()
            ),
        ));
    }

    if let Some(at) = stack.iter().position(|p| *p == canonical) {
        let mut chain: Vec<String> = stack[at..]
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        chain.push(canonical.display().to_string());
        return Err(Error::new(
            ErrorCode::IncludeCycle,
            format!("include cycle: {}", chain.join(" -> ")),
        ));
    }

    let bytes = fs::read(&canonical).map_err(|e| {
        Error::new(
            ErrorCode::Include,
            format!("include {target:?} ({}): {e}", canonical.display()),
        )
    })?;
    let text = String::from_utf8(bytes).map_err(|_| {
        Error::new(
            ErrorCode::Parse,
            format!("{} is not valid UTF-8", canonical.display()),
        )
    })?;
    let tree = parse::parse(&text, format, &canonical)?;

    let parent = canonical
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    stack.push(canonical);
    let resolved = resolve(tree, &parent, stack);
    stack.pop();
    resolved
}
