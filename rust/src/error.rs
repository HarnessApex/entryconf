//! The normative error codes of SPEC §7.

use std::fmt;

/// The normative entryconf error codes: the eight load codes of SPEC §7 and,
/// since 0.3.0, the six editing codes of SPEC §10.9.
///
/// Codes are part of the public contract; messages are not. New codes may be
/// added in later versions, so a `match` on this enum should keep a wildcard
/// arm unless it is prepared to be updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// `E_NO_ENTRYPOINT` — no entrypoint file in the directory.
    NoEntrypoint,
    /// `E_MULTIPLE_ENTRYPOINTS` — two or more entrypoint files.
    MultipleEntrypoints,
    /// `E_PARSE` — malformed config, env, or included file.
    Parse,
    /// `E_ENV_CONFLICT` — a variable defined more than once across/within `*.env` files.
    EnvConflict,
    /// `E_INCLUDE` — `@file:` target missing, unreadable, or unsupported extension.
    Include,
    /// `E_INCLUDE_CYCLE` — a file transitively includes itself.
    IncludeCycle,
    /// `E_MISSING_VAR` — reference to an unset variable with no default.
    MissingVar,
    /// `E_SUBSTITUTION` — malformed `$` or `@` form.
    Substitution,
    /// `E_UNSUPPORTED_EDIT` — the document's format is not writable (YAML,
    /// TOML, `*.env`), or one request names more than one document.
    UnsupportedEdit,
    /// `E_EDIT` — an invalid edit request (unknown document, unknown op,
    /// expression mode with a non-string value, empty request).
    Edit,
    /// `E_PATH` — a malformed JSON Pointer, or one that cannot be resolved or
    /// applied.
    Path,
    /// `E_STALE_PLAN` — at commit, a dependency's revision or the variables
    /// fingerprint differs from the plan's.
    StalePlan,
    /// `E_LOCKED` — the directory lock could not be acquired in time.
    Locked,
    /// `E_WRITE` — the replacement file could not be written or renamed; the
    /// target is unchanged.
    Write,
}

impl ErrorCode {
    /// The wire form of the code, e.g. `"E_PARSE"`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::NoEntrypoint => "E_NO_ENTRYPOINT",
            ErrorCode::MultipleEntrypoints => "E_MULTIPLE_ENTRYPOINTS",
            ErrorCode::Parse => "E_PARSE",
            ErrorCode::EnvConflict => "E_ENV_CONFLICT",
            ErrorCode::Include => "E_INCLUDE",
            ErrorCode::IncludeCycle => "E_INCLUDE_CYCLE",
            ErrorCode::MissingVar => "E_MISSING_VAR",
            ErrorCode::Substitution => "E_SUBSTITUTION",
            ErrorCode::UnsupportedEdit => "E_UNSUPPORTED_EDIT",
            ErrorCode::Edit => "E_EDIT",
            ErrorCode::Path => "E_PATH",
            ErrorCode::StalePlan => "E_STALE_PLAN",
            ErrorCode::Locked => "E_LOCKED",
            ErrorCode::Write => "E_WRITE",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A load or editing failure: a normative [`ErrorCode`] plus a non-normative
/// message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    code: ErrorCode,
    message: String,
}

impl Error {
    pub(crate) fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Error {
            code,
            message: message.into(),
        }
    }

    /// The normative error code as a string, e.g. `"E_INCLUDE_CYCLE"`.
    #[must_use]
    pub fn code(&self) -> &str {
        self.code.as_str()
    }

    /// The normative error code as an enum.
    #[must_use]
    pub fn kind(&self) -> ErrorCode {
        self.code
    }

    /// The human-readable detail. Not normative; do not match on it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}
