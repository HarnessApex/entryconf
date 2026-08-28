//! YAML 1.2, **core schema** (SPEC §2).
//!
//! Built on `saphyr-parser`'s event stream rather than a document loader so that
//! the three things the spec cares about are under our control:
//!
//!   * core-schema tag resolution — `on`/`off`/`yes`/`no` are plain strings, only
//!     `true|True|TRUE|false|False|FALSE` are booleans;
//!   * custom tags are `E_PARSE`;
//!   * duplicate mapping keys are `E_PARSE`.
//!
//! Anchors and aliases are resolved here and produce plain values.

use std::collections::HashMap;

use saphyr_parser::{Event, Parser, ScalarStyle, Tag};
use serde_json::{Map, Number, Value};

/// The tag handle of the YAML core schema (`!!str` and friends).
const CORE: &str = "tag:yaml.org,2002:";

pub(super) fn parse(text: &str) -> Result<Value, String> {
    let mut parser = Parser::new_from_str(text);
    let mut builder = Builder::default();

    loop {
        match parser.next_event() {
            None => break,
            Some(Err(e)) => return Err(e.to_string()),
            Some(Ok((event, _span))) => {
                if matches!(event, Event::StreamEnd) {
                    break;
                }
                builder.handle(event)?;
            }
        }
    }
    builder.finish()
}

enum Frame {
    Seq {
        anchor: usize,
        items: Vec<Value>,
    },
    Map {
        anchor: usize,
        object: Map<String, Value>,
        key: Option<String>,
    },
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    anchors: HashMap<usize, Value>,
    documents: Vec<Value>,
}

impl Builder {
    fn handle(&mut self, event: Event<'_>) -> Result<(), String> {
        match event {
            Event::Scalar(value, style, anchor, tag) => {
                let resolved = resolve_scalar(&value, style, tag.as_deref())?;
                self.push(resolved, anchor)
            }
            Event::SequenceStart(anchor, tag) => {
                check_container_tag(tag.as_deref(), "seq")?;
                self.stack.push(Frame::Seq {
                    anchor,
                    items: Vec::new(),
                });
                Ok(())
            }
            Event::SequenceEnd => match self.stack.pop() {
                Some(Frame::Seq { anchor, items }) => self.push(Value::Array(items), anchor),
                _ => Err("unbalanced sequence end".to_string()),
            },
            Event::MappingStart(anchor, tag) => {
                check_container_tag(tag.as_deref(), "map")?;
                self.stack.push(Frame::Map {
                    anchor,
                    object: Map::new(),
                    key: None,
                });
                Ok(())
            }
            Event::MappingEnd => match self.stack.pop() {
                Some(Frame::Map {
                    anchor,
                    object,
                    key: None,
                }) => self.push(Value::Object(object), anchor),
                Some(Frame::Map { .. }) => Err("mapping ended with a key but no value".to_string()),
                _ => Err("unbalanced mapping end".to_string()),
            },
            Event::Alias(id) => {
                let value = self
                    .anchors
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| format!("alias to unknown or recursive anchor {id}"))?;
                self.push(value, 0)
            }
            // StreamStart / DocumentStart / DocumentEnd / Nothing carry no data.
            _ => Ok(()),
        }
    }

    fn push(&mut self, value: Value, anchor: usize) -> Result<(), String> {
        if anchor != 0 {
            self.anchors.insert(anchor, value.clone());
        }
        match self.stack.last_mut() {
            None => {
                self.documents.push(value);
                Ok(())
            }
            Some(Frame::Seq { items, .. }) => {
                items.push(value);
                Ok(())
            }
            Some(Frame::Map { object, key, .. }) => match key.take() {
                None => match value {
                    Value::String(name) => {
                        *key = Some(name);
                        Ok(())
                    }
                    other => Err(format!(
                        "mapping key {other} is not a string; the entryconf data model has string keys only"
                    )),
                },
                Some(name) => {
                    if object.insert(name.clone(), value).is_some() {
                        return Err(format!("duplicate key {name:?}"));
                    }
                    Ok(())
                }
            },
        }
    }

    fn finish(mut self) -> Result<Value, String> {
        if !self.stack.is_empty() {
            return Err("unterminated collection".to_string());
        }
        match self.documents.len() {
            0 => Ok(Value::Null),
            1 => Ok(self.documents.pop().expect("length checked")),
            n => Err(format!(
                "{n} documents in one file; entryconf files hold exactly one"
            )),
        }
    }
}

fn check_container_tag(tag: Option<&Tag>, want: &str) -> Result<(), String> {
    match tag {
        None => Ok(()),
        // `!` is the non-specific tag, not a custom one.
        Some(t) if t.handle == "!" && t.suffix.is_empty() => Ok(()),
        Some(t) if t.handle == CORE && t.suffix == want => Ok(()),
        Some(t) => Err(format!("unsupported tag `{}{}`", t.handle, t.suffix)),
    }
}

fn resolve_scalar(raw: &str, style: ScalarStyle, tag: Option<&Tag>) -> Result<Value, String> {
    if let Some(t) = tag {
        if t.handle == "!" && t.suffix.is_empty() {
            return Ok(Value::String(raw.to_string()));
        }
        if t.handle != CORE {
            return Err(format!("unsupported tag `{}{}`", t.handle, t.suffix));
        }
        return match t.suffix.as_str() {
            "str" => Ok(Value::String(raw.to_string())),
            "null" => core_null(raw).ok_or_else(|| format!("{raw:?} is not a valid !!null value")),
            "bool" => core_bool(raw).ok_or_else(|| format!("{raw:?} is not a valid !!bool value")),
            "int" => core_int(raw)?.ok_or_else(|| format!("{raw:?} is not a valid !!int value")),
            "float" => {
                core_float(raw)?.ok_or_else(|| format!("{raw:?} is not a valid !!float value"))
            }
            other => Err(format!("unsupported tag `{CORE}{other}`")),
        };
    }

    // Only plain scalars are resolved; quoted and block scalars are always strings.
    if style != ScalarStyle::Plain {
        return Ok(Value::String(raw.to_string()));
    }
    if let Some(v) = core_null(raw) {
        return Ok(v);
    }
    if let Some(v) = core_bool(raw) {
        return Ok(v);
    }
    if let Some(v) = core_int(raw)? {
        return Ok(v);
    }
    if let Some(v) = core_float(raw)? {
        return Ok(v);
    }
    Ok(Value::String(raw.to_string()))
}

/// `null | Null | NULL | ~` and the empty scalar.
fn core_null(raw: &str) -> Option<Value> {
    matches!(raw, "" | "null" | "Null" | "NULL" | "~").then_some(Value::Null)
}

/// Exactly `true | True | TRUE | false | False | FALSE` — the core schema's
/// boolean regex. YAML 1.1's `on`/`off`/`yes`/`no`/`y`/`n` are strings.
fn core_bool(raw: &str) -> Option<Value> {
    match raw {
        "true" | "True" | "TRUE" => Some(Value::Bool(true)),
        "false" | "False" | "FALSE" => Some(Value::Bool(false)),
        _ => None,
    }
}

/// `[-+]?[0-9]+` | `0o[0-7]+` | `0x[0-9a-fA-F]+`.
fn core_int(raw: &str) -> Result<Option<Value>, String> {
    let (digits, radix) = if let Some(rest) = raw.strip_prefix("0x") {
        if rest.is_empty() || !rest.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Ok(None);
        }
        (rest.to_string(), 16)
    } else if let Some(rest) = raw.strip_prefix("0o") {
        if rest.is_empty() || !rest.bytes().all(|c| (b'0'..=b'7').contains(&c)) {
            return Ok(None);
        }
        (rest.to_string(), 8)
    } else {
        let body = raw.strip_prefix(['-', '+']).unwrap_or(raw);
        if body.is_empty() || !body.bytes().all(|c| c.is_ascii_digit()) {
            return Ok(None);
        }
        (raw.to_string(), 10)
    };

    if let Ok(n) = i64::from_str_radix(&digits, radix) {
        return Ok(Some(Value::Number(n.into())));
    }
    if let Ok(n) = u64::from_str_radix(&digits, radix) {
        return Ok(Some(Value::Number(n.into())));
    }
    Err(format!("integer {raw} is out of range for the data model"))
}

/// The core schema's float regex, plus `.inf` / `.nan`, which have no JSON form.
fn core_float(raw: &str) -> Result<Option<Value>, String> {
    let body = raw.strip_prefix(['-', '+']).unwrap_or(raw);
    if matches!(body, ".inf" | ".Inf" | ".INF") || matches!(raw, ".nan" | ".NaN" | ".NAN") {
        return Err(format!(
            "{raw} has no representation in the entryconf data model"
        ));
    }
    if !is_core_float(raw) {
        return Ok(None);
    }
    let parsed: f64 = raw.parse().map_err(|_| format!("bad float {raw}"))?;
    Number::from_f64(parsed)
        .map(|n| Some(Value::Number(n)))
        .ok_or_else(|| format!("{raw} has no representation in the entryconf data model"))
}

/// `[-+]? ( \. [0-9]+ | [0-9]+ ( \. [0-9]* )? ) ( [eE] [-+]? [0-9]+ )?`
fn is_core_float(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    } else {
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
        if i < b.len() && b[i] == b'.' {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    i == b.len()
}
