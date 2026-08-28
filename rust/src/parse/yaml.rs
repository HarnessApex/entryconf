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
//! Anchors and aliases are resolved here and produce plain values, under the
//! expansion budget of SPEC §2.

use std::collections::HashMap;

use saphyr_parser::{Event, Parser, ScalarStyle, Tag};
use serde_json::{Map, Number, Value};

/// The tag handle of the YAML core schema (`!!str` and friends).
const CORE: &str = "tag:yaml.org,2002:";

/// SPEC §2: a document whose fully expanded tree would exceed this many nodes
/// is `E_PARSE`.
///
/// The spec's counting rule: a scalar counts one; a sequence or mapping
/// contributes one per element or entry **plus** each element value's own
/// count; mapping keys are not counted. So a sequence of 1,000 scalars counts
/// 2,000 — each element once as a slot and once as the scalar in it — and an
/// empty collection counts zero.
///
/// The budget is charged as the tree is built: each completed value pays one
/// for the element/entry slot that holds it (nothing in mapping-key position,
/// nothing at the document root) plus, for a scalar or an alias expansion, its
/// own count. A collection pays nothing extra of its own, because its children
/// charged themselves as they completed.
///
/// An alias is charged the *whole* count of the anchored subtree it expands to,
/// taken from the anchor table, **before** that subtree is cloned. So a layered
/// alias bomb is rejected after a bounded amount of work (the charge for the
/// first over-budget alias fails, and the clone never happens) rather than being
/// materialized first.
const NODE_BUDGET: usize = 1_000_000;

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

/// Whether a completed value's own node count still has to be charged. A scalar
/// or an alias expansion pays it as it is placed; a collection has already paid
/// it child by child, and pays only for its own slot.
#[derive(Copy, Clone)]
enum Own {
    Charge,
    AlreadyCharged,
}

enum Frame {
    Seq {
        anchor: usize,
        items: Vec<Value>,
        /// This subtree's node count so far under the SPEC §2 rule: one per
        /// element plus that element's own count, counting alias expansions in
        /// full. The collection itself adds nothing, so an empty one is zero.
        nodes: usize,
    },
    Map {
        anchor: usize,
        object: Map<String, Value>,
        key: Option<String>,
        /// This subtree's node count so far (see [`Frame::Seq`]); keys are not
        /// counted, so an entry costs one plus its value's own count.
        nodes: usize,
    },
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    /// Each anchored value plus its SPEC §2 node count, so an alias can be
    /// charged against the budget without being expanded first.
    anchors: HashMap<usize, (Value, usize)>,
    documents: Vec<Value>,
    /// Nodes charged so far, across the whole document (SPEC §2).
    nodes: usize,
}

impl Builder {
    fn handle(&mut self, event: Event<'_>) -> Result<(), String> {
        match event {
            Event::Scalar(value, style, anchor, tag) => {
                let resolved = resolve_scalar(&value, style, tag.as_deref())?;
                self.charge_placement(1, Own::Charge)?;
                self.push(resolved, 1, anchor)
            }
            Event::SequenceStart(anchor, tag) => {
                check_container_tag(tag.as_deref(), "seq")?;
                self.stack.push(Frame::Seq {
                    anchor,
                    items: Vec::new(),
                    nodes: 0,
                });
                Ok(())
            }
            Event::SequenceEnd => match self.stack.pop() {
                Some(Frame::Seq {
                    anchor,
                    items,
                    nodes,
                }) => {
                    self.charge_placement(nodes, Own::AlreadyCharged)?;
                    self.push(Value::Array(items), nodes, anchor)
                }
                _ => Err("unbalanced sequence end".to_string()),
            },
            Event::MappingStart(anchor, tag) => {
                check_container_tag(tag.as_deref(), "map")?;
                self.stack.push(Frame::Map {
                    anchor,
                    object: Map::new(),
                    key: None,
                    nodes: 0,
                });
                Ok(())
            }
            Event::MappingEnd => match self.stack.pop() {
                Some(Frame::Map {
                    anchor,
                    object,
                    key: None,
                    nodes,
                }) => {
                    self.charge_placement(nodes, Own::AlreadyCharged)?;
                    self.push(Value::Object(object), nodes, anchor)
                }
                Some(Frame::Map { .. }) => Err("mapping ended with a key but no value".to_string()),
                _ => Err("unbalanced mapping end".to_string()),
            },
            Event::Alias(id) => {
                // The size lookup comes first: the expansion is charged against
                // the budget *before* the subtree is cloned, so an over-budget
                // alias costs a comparison rather than a copy.
                let size = self
                    .anchors
                    .get(&id)
                    .map(|(_, size)| *size)
                    .ok_or_else(|| format!("alias to unknown or recursive anchor {id}"))?;
                self.charge_placement(size, Own::Charge)?;
                let value = self.anchors[&id].0.clone();
                self.push(value, size, 0)
            }
            // StreamStart / DocumentStart / DocumentEnd / Nothing carry no data.
            _ => Ok(()),
        }
    }

    /// Charges the budget for a completed value of `size` nodes landing in the
    /// position the stack is currently in (SPEC §2).
    ///
    /// The cost is one for the element or entry slot that holds it — the
    /// document root has no slot — plus, when `own` says so, the value's own
    /// node count. A value in mapping-key position costs nothing at all: keys
    /// are not counted.
    fn charge_placement(&mut self, size: usize, own: Own) -> Result<(), String> {
        if matches!(self.stack.last(), Some(Frame::Map { key: None, .. })) {
            return Ok(());
        }
        let slot = usize::from(!self.stack.is_empty());
        let own = match own {
            Own::Charge => size,
            Own::AlreadyCharged => 0,
        };
        self.charge(slot.saturating_add(own))
    }

    /// Charges `n` nodes against the expansion budget (SPEC §2).
    fn charge(&mut self, n: usize) -> Result<(), String> {
        self.nodes = self.nodes.saturating_add(n);
        if self.nodes > NODE_BUDGET {
            return Err(format!(
                "expanded tree exceeds the {NODE_BUDGET}-node limit; \
                 check for an alias that multiplies its anchor"
            ));
        }
        Ok(())
    }

    fn push(&mut self, value: Value, size: usize, anchor: usize) -> Result<(), String> {
        if anchor != 0 {
            self.anchors.insert(anchor, (value.clone(), size));
        }
        match self.stack.last_mut() {
            None => {
                self.documents.push(value);
                Ok(())
            }
            Some(frame) => Self::push_into(frame, value, size),
        }
    }

    /// Adds a completed child of `size` nodes to the frame on top of the stack,
    /// growing that frame's subtree count by the SPEC §2 cost of the child: one
    /// for its slot plus its own count, or nothing at all for a mapping key.
    fn push_into(frame: &mut Frame, value: Value, size: usize) -> Result<(), String> {
        match frame {
            Frame::Seq { items, nodes, .. } => {
                items.push(value);
                *nodes += 1 + size;
                Ok(())
            }
            Frame::Map {
                object,
                key,
                nodes,
                ..
            } => match key.take() {
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
                    *nodes += 1 + size;
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

#[cfg(test)]
mod tests {
    use super::{parse, NODE_BUDGET};
    use serde_json::Value;

    /// Builds a layered alias bomb: `layer0` is a scalar, and each of the next
    /// `layers` lines is a nine-element sequence of aliases to the layer below.
    ///
    /// Under the SPEC §2 rule each layer costs nine slots plus nine copies of
    /// the layer below — `c(0) = 1`, `c(n) = 9 * (1 + c(n-1))` — so the layers
    /// run 1, 18, 171, 1_548, 13_941, 125_478, 1_129_311, …: five layers fit
    /// inside the budget (141_163 nodes all told) and six do not.
    fn layered_bomb(layers: usize) -> String {
        let mut text = String::from("layer0: &layer0 seed\n");
        for n in 1..=layers {
            let refs = (0..9)
                .map(|_| format!("*layer{}", n - 1))
                .collect::<Vec<_>>()
                .join(", ");
            text.push_str(&format!("layer{n}: &layer{n} [{refs}]\n"));
        }
        text
    }

    /// The budget is charged during expansion, so an over-budget document is
    /// rejected without being materialized. Nine layers expand to hundreds of
    /// millions of nodes; if this test ever hangs or exhausts memory instead of
    /// returning, the charge has stopped preceding the clone.
    #[test]
    fn an_alias_bomb_is_rejected_rather_than_expanded() {
        let err = parse(&layered_bomb(9)).expect_err("48M nodes is over budget");
        assert!(
            err.contains(&NODE_BUDGET.to_string()),
            "the message should name the budget: {err}"
        );
    }

    /// The bound must not be a timeout in disguise: rejection happens after
    /// work proportional to the budget, not to the expansion.
    #[test]
    fn an_alias_bomb_is_rejected_quickly() {
        let start = std::time::Instant::now();
        assert!(parse(&layered_bomb(9)).is_err());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "took {:?}; the budget is not being charged before expansion",
            start.elapsed()
        );
    }

    /// Heavy but legal aliasing still loads: five layers expand to 141,163
    /// nodes, inside the budget.
    #[test]
    fn heavy_but_legal_aliasing_still_loads() {
        let tree = parse(&layered_bomb(5)).expect("141,163 nodes is inside the budget");
        let Value::Object(map) = &tree else {
            panic!("expected a mapping, got {tree}");
        };
        assert_eq!(map.len(), 6, "one key per layer, plus layer0");
        // Five indices walk layer5 down through layer1 to the seed scalar.
        assert_eq!(tree["layer5"][0][0][0][0][0], Value::String("seed".into()));
    }

    /// SPEC §2's counting rule, pinned at the budget boundary: a sequence
    /// charges one per element *plus* the element's own count, so the anchored
    /// row below is 2,000 nodes rather than 1,001, and a mapping entry costs one
    /// plus its value.
    ///
    /// `row` is 1,000 element slots + 1,000 scalars = 2,000; the document is
    /// `(1 + 2000) + (1 + copies * (1 + 2000))` = `2002 + 2001 * copies`. So 498
    /// copies is 998,500 nodes and fits; 499 is 1,000,501 and does not. Under a
    /// naive one-charge-per-value rule both would be around 500k and load.
    #[test]
    fn the_counting_rule_charges_a_slot_plus_the_value() {
        let doc = |copies: usize| {
            let row = vec!["x"; 1000].join(", ");
            let refs = vec!["*row"; copies].join(", ");
            format!("row: &row [{row}]\nbomb: [{refs}]\n")
        };

        let tree = parse(&doc(498)).expect("998,500 nodes is inside the budget");
        assert_eq!(tree["bomb"][497][999], Value::String("x".into()));

        let err = parse(&doc(499)).expect_err("1,000,501 nodes is over budget");
        assert!(
            err.contains(&NODE_BUDGET.to_string()),
            "the message should name the budget: {err}"
        );
    }

    /// A collection charges nothing for itself, so nesting empty ones costs only
    /// their slots — and mapping keys are not counted at all.
    #[test]
    fn empty_collections_and_keys_are_free() {
        let tree = parse("a: {}\nb: []\n").expect("valid");
        assert_eq!(tree["a"], Value::Object(Default::default()));
        assert_eq!(tree["b"], Value::Array(Vec::new()));
    }

    /// The budget counts the expansion, not the source: a document that is
    /// small either way is unaffected.
    #[test]
    fn ordinary_documents_are_unaffected() {
        let tree = parse("a: &x [1, 2]\nb: *x\n").expect("tiny document");
        assert_eq!(tree["a"], tree["b"]);
        assert_eq!(tree["b"][1], Value::from(2));
    }

    /// SPEC §2: unquoted `010` is decimal 10 — YAML 1.2 has no leading-zero
    /// octal form; octal is `0o10` and hex `0x10`.
    #[test]
    fn leading_zero_integers_are_decimal() {
        let tree = parse("d: 010\no: 0o10\nh: 0x10\nq: \"010\"\nneg: -010\n").expect("valid");
        assert_eq!(tree["d"], Value::from(10));
        assert_eq!(tree["o"], Value::from(8));
        assert_eq!(tree["h"], Value::from(16));
        assert_eq!(tree["q"], Value::String("010".into()));
        assert_eq!(tree["neg"], Value::from(-10));
    }

    /// A sign is not part of the core schema's hex or octal forms, so a signed
    /// one is an ordinary string rather than a number.
    #[test]
    fn signed_hex_and_octal_are_strings() {
        let tree = parse("h: -0x10\no: +0o10\n").expect("valid");
        assert_eq!(tree["h"], Value::String("-0x10".into()));
        assert_eq!(tree["o"], Value::String("+0o10".into()));
    }
}
