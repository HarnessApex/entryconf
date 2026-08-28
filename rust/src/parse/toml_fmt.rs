//! TOML. Duplicate keys are already a hard error in the TOML grammar, so the
//! stock parser covers SPEC §2's duplicate rule.
//!
//! Datetimes become RFC 3339-style strings under SPEC §2's exact rendering
//! rules. The `toml` crate's own `Display` is close but diverges twice: it
//! writes a zero fraction as `.0` rather than dropping it, and it renders an
//! authored `+00:00` offset numerically rather than as `Z`. So the rendering
//! here works from the parsed fields instead of `Datetime::to_string`.

use serde_json::{Map, Number, Value};
use toml::value::{Date, Datetime, Offset, Time};

pub(super) fn parse(text: &str) -> Result<Value, String> {
    let root: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    convert(root)
}

fn convert(value: toml::Value) -> Result<Value, String> {
    Ok(match value {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::Number(i.into()),
        toml::Value::Float(f) => Number::from_f64(f)
            .map(Value::Number)
            .ok_or_else(|| format!("float {f} is not representable in JSON"))?,
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(d) => Value::String(render_datetime(&d)),
        toml::Value::Array(items) => {
            Value::Array(items.into_iter().map(convert).collect::<Result<_, _>>()?)
        }
        toml::Value::Table(table) => {
            let mut object = Map::new();
            for (key, item) in table {
                object.insert(key, convert(item)?);
            }
            Value::Object(object)
        }
    })
}

/// SPEC §2: uppercase `T` separator; a UTC offset (`Z`, `z`, or `+00:00` in the
/// source) renders as `Z`; any other offset keeps its authored numeric form;
/// fractional seconds drop trailing zeros, and the `.` goes with them when the
/// fraction reaches zero; local date-times, dates, and times keep their
/// offset-less fragment.
fn render_datetime(d: &Datetime) -> String {
    let mut out = String::with_capacity(32);
    if let Some(date) = d.date {
        render_date(&mut out, date);
    }
    if let Some(time) = d.time {
        if d.date.is_some() {
            out.push('T');
        }
        render_time(&mut out, time);
    }
    if let Some(offset) = d.offset {
        render_offset(&mut out, offset);
    }
    out
}

fn render_date(out: &mut String, date: Date) {
    out.push_str(&format!(
        "{:04}-{:02}-{:02}",
        date.year, date.month, date.day
    ));
}

fn render_time(out: &mut String, time: Time) {
    out.push_str(&format!("{:02}:{:02}", time.hour, time.minute));
    // A nanosecond field implies a seconds field, even where the grammar let
    // the seconds be elided.
    if let Some(second) = time.second.or(time.nanosecond.map(|_| 0)) {
        out.push_str(&format!(":{second:02}"));
    }
    // Trailing zeros go, and the `.` goes with them at zero — so `.500` is
    // `.5` and `.000` renders as nothing at all.
    if let Some(nanosecond) = time.nanosecond {
        let digits = format!("{nanosecond:09}");
        let trimmed = digits.trim_end_matches('0');
        if !trimmed.is_empty() {
            out.push('.');
            out.push_str(trimmed);
        }
    }
}

fn render_offset(out: &mut String, offset: Offset) {
    match offset {
        // `Z`, `z`, and `+00:00` are the same offset and render alike.
        Offset::Z => out.push('Z'),
        Offset::Custom { minutes: 0 } => out.push('Z'),
        Offset::Custom { minutes } => {
            let sign = if minutes < 0 { '-' } else { '+' };
            let total = minutes.abs();
            out.push_str(&format!("{sign}{:02}:{:02}", total / 60, total % 60));
        }
    }
}
