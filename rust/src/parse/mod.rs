//! SPEC §2 — parsing documents into the JSON-equivalent data model.

mod json;
mod yaml;

use std::path::Path;

use serde_json::Value;

use crate::error::{Error, ErrorCode};

mod toml_fmt;

/// The document formats entryconf understands. The file extension selects one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Json,
    Yaml,
    Toml,
}

/// Maps a path's extension to a parser. `None` means "unsupported extension".
///
/// Extensions are matched exactly as the spec spells them (lowercase).
pub(crate) fn format_for_path(path: &Path) -> Option<Format> {
    match path.extension().and_then(|e| e.to_str())? {
        "json" => Some(Format::Json),
        "yaml" | "yml" => Some(Format::Yaml),
        "toml" => Some(Format::Toml),
        _ => None,
    }
}

/// Parses one document. Every failure is `E_PARSE`.
pub(crate) fn parse(text: &str, format: Format, origin: &Path) -> Result<Value, Error> {
    let result = match format {
        Format::Json => json::parse(text),
        Format::Yaml => yaml::parse(text),
        Format::Toml => toml_fmt::parse(text),
    };
    result.map_err(|detail| Error::new(ErrorCode::Parse, format!("{}: {detail}", origin.display())))
}
