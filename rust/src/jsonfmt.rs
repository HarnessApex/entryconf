//! The normalized JSON form of SPEC §10.8.

use serde_json::Value;

/// Serializes `value`: two-space indentation, one member or element per line,
/// keys by code point (a `serde_json::Map` without `preserve_order` already
/// iterates in byte order, which for UTF-8 is code-point order), minimal
/// string escaping, integers as integers, one trailing newline.
pub(crate) fn format(value: &Value) -> String {
    let mut out = String::new();
    write(&mut out, value, 0);
    out.push('\n');
    out
}

fn write(out: &mut String, value: &Value, depth: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::String(s) => write_string(out, s),
        Value::Number(n) => write_number(out, n),
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            let last = map.len() - 1;
            for (i, (key, item)) in map.iter().enumerate() {
                indent(out, depth + 1);
                write_string(out, key);
                out.push_str(": ");
                write(out, item, depth + 1);
                if i < last {
                    out.push(',');
                }
                out.push('\n');
            }
            indent(out, depth);
            out.push('}');
        }
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            let last = items.len() - 1;
            for (i, item) in items.iter().enumerate() {
                indent(out, depth + 1);
                write(out, item, depth + 1);
                if i < last {
                    out.push(',');
                }
                out.push('\n');
            }
            indent(out, depth);
            out.push(']');
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

const TWO_POW_53: f64 = 9_007_199_254_740_992.0;

/// Integral values of magnitude below 2^53 as plain integers, other finite
/// numbers in the shortest round-trip form.
fn write_number(out: &mut String, n: &serde_json::Number) {
    if let Some(i) = n.as_i64() {
        out.push_str(&i.to_string());
        return;
    }
    if let Some(u) = n.as_u64() {
        out.push_str(&u.to_string());
        return;
    }
    let f = n.as_f64().unwrap_or(0.0);
    if f == f.trunc() && f.abs() < TWO_POW_53 {
        out.push_str(&(f as i64).to_string());
        return;
    }
    // ryu's shortest round-trip form, exponent spelling included; SPEC §10.8
    // allows that to differ between implementations.
    out.push_str(&f.to_string());
}

fn write_string(out: &mut String, s: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let b = c as u32 as usize;
                out.push_str("\\u00");
                out.push(HEX[b >> 4] as char);
                out.push(HEX[b & 0xf] as char);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::format;
    use serde_json::json;

    /// SPEC §10.8's byte-level rules, pinned.
    #[test]
    fn normalized_form() {
        let v = json!({
            "b": [1, 2.0, 1.5, "x"],
            "a": {},
            "c": [],
            "s": "q\"\\\n\t\u{1}é",
            "big": 9007199254740991.0_f64,
        });
        let want = "{\n  \"a\": {},\n  \"b\": [\n    1,\n    2,\n    1.5,\n    \"x\"\n  ],\n  \"big\": 9007199254740991,\n  \"c\": [],\n  \"s\": \"q\\\"\\\\\\n\\t\\u0001é\"\n}\n";
        assert_eq!(format(&v), want);
    }

    #[test]
    fn keys_sort_by_code_point() {
        let v = json!({"é": 1, "z": 2, "Z": 3, "a": 4});
        assert_eq!(format(&v), "{\n  \"Z\": 3,\n  \"a\": 4,\n  \"z\": 2,\n  \"é\": 1\n}\n");
    }
}
