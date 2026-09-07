//! RFC 6901 JSON Pointers over `serde_json::Value` (SPEC §10.3, §10.5).

use serde_json::Value;

use crate::error::{Error, ErrorCode};

fn path_err(detail: String) -> Error {
    Error::new(ErrorCode::Path, detail)
}

/// Splits a pointer into its unescaped tokens; `""` is the root (no tokens).
pub(crate) fn parse(pointer: &str) -> Result<Vec<String>, Error> {
    if pointer.is_empty() {
        return Ok(Vec::new());
    }
    let Some(rest) = pointer.strip_prefix('/') else {
        return Err(path_err(format!(
            "pointer {pointer:?} must be empty or start with \"/\""
        )));
    };
    rest.split('/')
        .map(|raw| {
            let mut out = String::with_capacity(raw.len());
            let mut chars = raw.chars();
            while let Some(c) = chars.next() {
                if c != '~' {
                    out.push(c);
                    continue;
                }
                match chars.next() {
                    Some('0') => out.push('~'),
                    Some('1') => out.push('/'),
                    _ => {
                        return Err(path_err(format!(
                            "pointer {pointer:?}: \"~\" must be followed by 0 or 1"
                        )))
                    }
                }
            }
            Ok(out)
        })
        .collect()
}

/// RFC 6901 escaping of one reference token.
pub(crate) fn escape_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// An array index token: decimal digits with no leading zeros. `None` for
/// anything else, including `-`.
pub(crate) fn array_index(token: &str) -> Option<usize> {
    if token.is_empty() || (token.len() > 1 && token.starts_with('0')) {
        return None;
    }
    if !token.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

/// Applies a `set` edit (SPEC §10.5) in place. Missing parents are created as
/// objects; array steps must exist; the final token may append with the index
/// equal to the length or `-`.
pub(crate) fn set(root: &mut Value, pointer: &str, value: Value) -> Result<(), Error> {
    let tokens = parse(pointer)?;
    let Some((last, parents)) = tokens.split_last() else {
        *root = value;
        return Ok(());
    };
    let mut cur = root;
    for token in parents {
        cur = match cur {
            Value::Object(map) => map
                .entry(token.clone())
                .or_insert_with(|| Value::Object(Default::default())),
            Value::Array(items) => {
                let len = items.len();
                match array_index(token).filter(|&i| i < len) {
                    Some(i) => &mut items[i],
                    None => {
                        return Err(path_err(format!(
                            "pointer {pointer:?}: array index {token:?} does not exist"
                        )))
                    }
                }
            }
            _ => {
                return Err(path_err(format!(
                    "pointer {pointer:?}: cannot descend into a scalar at {token:?}"
                )))
            }
        };
    }
    match cur {
        Value::Object(map) => {
            map.insert(last.clone(), value);
            Ok(())
        }
        Value::Array(items) => {
            if last == "-" {
                items.push(value);
                return Ok(());
            }
            let len = items.len();
            match array_index(last) {
                Some(i) if i < len => {
                    items[i] = value;
                    Ok(())
                }
                Some(i) if i == len => {
                    items.push(value);
                    Ok(())
                }
                _ => Err(path_err(format!(
                    "pointer {pointer:?}: array index {last:?} is out of range (0..{len})"
                ))),
            }
        }
        _ => Err(path_err(format!(
            "pointer {pointer:?}: cannot set a member of a scalar"
        ))),
    }
}

/// Applies a `remove` edit (SPEC §10.5) in place: every step must exist, the
/// root cannot be removed, and array elements shift down.
pub(crate) fn remove(root: &mut Value, pointer: &str) -> Result<(), Error> {
    let tokens = parse(pointer)?;
    let Some((last, parents)) = tokens.split_last() else {
        return Err(path_err("the root value cannot be removed".to_string()));
    };
    let mut cur = root;
    for token in parents {
        cur = match cur {
            Value::Object(map) => match map.get_mut(token) {
                Some(v) => v,
                None => {
                    return Err(path_err(format!(
                        "pointer {pointer:?}: no member {token:?}"
                    )))
                }
            },
            Value::Array(items) => {
                let len = items.len();
                match array_index(token).filter(|&i| i < len) {
                    Some(i) => &mut items[i],
                    None => {
                        return Err(path_err(format!(
                            "pointer {pointer:?}: array index {token:?} does not exist"
                        )))
                    }
                }
            }
            _ => {
                return Err(path_err(format!(
                    "pointer {pointer:?}: cannot descend into a scalar at {token:?}"
                )))
            }
        };
    }
    match cur {
        Value::Object(map) => map.remove(last).map(|_| ()).ok_or_else(|| {
            path_err(format!(
                "pointer {pointer:?}: no member {last:?} to remove"
            ))
        }),
        Value::Array(items) => match array_index(last) {
            Some(i) if i < items.len() => {
                items.remove(i);
                Ok(())
            }
            _ => Err(path_err(format!(
                "pointer {pointer:?}: array index {last:?} does not exist"
            ))),
        },
        _ => Err(path_err(format!(
            "pointer {pointer:?}: cannot remove a member of a scalar"
        ))),
    }
}
