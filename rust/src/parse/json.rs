//! JSON with duplicate-key detection.
//!
//! `serde_json::Value`'s own `Deserialize` silently last-wins on duplicate keys,
//! which SPEC §2 makes `E_PARSE`. So we drive serde_json's (strict, well-tested)
//! parser with our own visitor and check every insert.

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

pub(super) fn parse(text: &str) -> Result<Value, String> {
    serde_json::from_str::<Node>(text)
        .map(|node| node.0)
        .map_err(|e| e.to_string())
}

/// A `serde_json::Value` that rejects duplicate object keys.
struct Node(Value);

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(NodeVisitor)
    }
}

struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
    type Value = Node;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Node, E> {
        Ok(Node(Value::Null))
    }

    fn visit_none<E: de::Error>(self) -> Result<Node, E> {
        Ok(Node(Value::Null))
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Node, E> {
        Ok(Node(Value::Bool(v)))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Node, E> {
        Ok(Node(Value::Number(v.into())))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Node, E> {
        Ok(Node(Value::Number(v.into())))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Node, E> {
        Number::from_f64(v)
            .map(|n| Node(Value::Number(n)))
            .ok_or_else(|| de::Error::custom("number is not representable in JSON"))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Node, E> {
        Ok(Node(Value::String(v.to_string())))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Node, E> {
        Ok(Node(Value::String(v)))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let mut items = Vec::new();
        while let Some(Node(item)) = seq.next_element::<Node>()? {
            items.push(item);
        }
        Ok(Node(Value::Array(items)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let Node(value) = map.next_value::<Node>()?;
            if object.insert(key.clone(), value).is_some() {
                return Err(de::Error::custom(format!("duplicate key {key:?}")));
            }
        }
        Ok(Node(Value::Object(object)))
    }
}
