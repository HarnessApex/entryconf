//! SPEC §6 — `$` interpolation over the assembled tree.

use serde_json::{Map, Value};

use crate::envfile::{is_var_name, Vars};
use crate::error::{Error, ErrorCode};

/// Interpolates every string value in the tree. Object keys are never touched.
pub(crate) fn interpolate(value: Value, vars: &Vars<'_>) -> Result<Value, Error> {
    match value {
        Value::String(s) => interpolate_string(&s, vars),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(interpolate(item, vars)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(object) => {
            let mut out = Map::new();
            for (key, item) in object {
                out.insert(key, interpolate(item, vars)?);
            }
            Ok(Value::Object(out))
        }
        scalar => Ok(scalar),
    }
}

fn substitution(detail: impl Into<String>) -> Error {
    Error::new(ErrorCode::Substitution, detail)
}

/// Expands one string. Substituted text is inert: it is copied into the output
/// and never re-scanned for `$` or `@file:`.
fn interpolate_string(s: &str, vars: &Vars<'_>) -> Result<Value, Error> {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    // Whole-value typing applies only when the string is exactly one reference.
    let mut references = 0usize;
    let mut spans_whole_string = false;

    while i < bytes.len() {
        if bytes[i] != b'$' {
            let next = s[i..].find('$').map_or(bytes.len(), |p| i + p);
            out.push_str(&s[i..next]);
            i = next;
            continue;
        }

        let Some(&after) = bytes.get(i + 1) else {
            return Err(substitution(format!("{s:?}: trailing `$`")));
        };

        match after {
            b'$' => {
                out.push('$');
                i += 2;
            }
            b'{' => {
                let Some(close) = s[i + 2..].find('}').map(|p| i + 2 + p) else {
                    return Err(substitution(format!("{s:?}: unterminated `${{`")));
                };
                let inner = &s[i + 2..close];
                let (name, default) = match inner.find(':') {
                    None => (inner, None),
                    Some(at) => {
                        let Some(text) = inner[at + 1..].strip_prefix('-') else {
                            return Err(substitution(format!(
                                "{s:?}: `${{NAME:...}}` forms other than `:-` are reserved"
                            )));
                        };
                        (&inner[..at], Some(text))
                    }
                };
                if !is_var_name(name) {
                    return Err(substitution(format!(
                        "{s:?}: invalid variable name {name:?}"
                    )));
                }
                let expanded = match vars.get(name) {
                    Some(v) => v.to_string(),
                    None => match default {
                        Some(text) => text.to_string(),
                        None => {
                            return Err(Error::new(
                                ErrorCode::MissingVar,
                                format!("variable {name:?} is not set and has no default"),
                            ))
                        }
                    },
                };
                references += 1;
                spans_whole_string = i == 0 && close + 1 == bytes.len();
                out.push_str(&expanded);
                i = close + 1;
            }
            c if c == b'_' || c.is_ascii_alphabetic() => {
                let mut end = i + 1;
                while end < bytes.len() && (bytes[end] == b'_' || bytes[end].is_ascii_alphanumeric())
                {
                    end += 1;
                }
                let name = &s[i + 1..end];
                let Some(expanded) = vars.get(name) else {
                    return Err(Error::new(
                        ErrorCode::MissingVar,
                        format!("variable {name:?} is not set and has no default"),
                    ));
                };
                let expanded = expanded.to_string();
                references += 1;
                spans_whole_string = i == 0 && end == bytes.len();
                out.push_str(&expanded);
                i = end;
            }
            _ => {
                return Err(substitution(format!(
                    "{s:?}: `$` must be followed by `$`, `{{NAME...}}`, or a name starting with a letter or `_`"
                )))
            }
        }
    }

    if references == 1 && spans_whole_string {
        if let Some(typed) = typed_scalar(&out) {
            return Ok(typed);
        }
    }
    Ok(Value::String(out))
}

/// Whole-value typing: `true`, `false`, `null`, or a valid JSON number that
/// parses to a *finite* IEEE-754 double becomes that scalar; anything else
/// stays a string. An overflowing literal such as `1e400` is not a number, so
/// it survives as the text `"1e400"`.
fn typed_scalar(text: &str) -> Option<Value> {
    match text {
        "true" => return Some(Value::Bool(true)),
        "false" => return Some(Value::Bool(false)),
        "null" => return Some(Value::Null),
        _ => {}
    }
    // serde_json tolerates surrounding whitespace around a bare value; JSON
    // number syntax itself does not, so reject it before asking.
    if text.is_empty() || text.trim() != text {
        return None;
    }
    serde_json::from_str::<Value>(text)
        .ok()
        .filter(|v| v.as_f64().is_some_and(f64::is_finite))
}
