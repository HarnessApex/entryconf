//! entryconf — load a config directory into a single tree.
//!
//! Implements [entryconf spec 0.2.0](../../SPEC.md): one
//! `entrypoint.{json,yaml,yml,toml}` per directory, `*.env` variable files as
//! unordered peers with the process environment on top, `@file:` include
//! grafting, and `$NAME` / `${NAME}` / `${NAME:-default}` interpolation.
//!
//! ```no_run
//! let tree = entryconf::load(std::path::Path::new("envs/deploy"))?;
//! println!("{}", tree["database"]["host"]);
//! # Ok::<(), entryconf::Error>(())
//! ```
//!
//! Every failure is a hard error carrying a normative code:
//!
//! ```no_run
//! # use std::path::Path;
//! match entryconf::load(Path::new("envs/deploy")) {
//!     Ok(tree) => println!("{tree}"),
//!     Err(e) => eprintln!("{}: {}", e.code(), e.message()),
//! }
//! ```

#![deny(missing_docs)]

mod envfile;
mod error;
mod include;
mod interp;
mod parse;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub use crate::error::{Error, ErrorCode};

/// The entrypoint file names, in the order SPEC §3 lists them.
const ENTRYPOINTS: [&str; 4] = [
    "entrypoint.json",
    "entrypoint.yaml",
    "entrypoint.yml",
    "entrypoint.toml",
];

/// Loads the config directory `dir` into a single tree.
///
/// Variables come from the directory's `*.env` files plus this process's
/// environment, which overrides them.
///
/// # Errors
///
/// Returns an [`Error`] whose [`Error::code`] is one of the eight normative
/// `E_*` codes (SPEC §7). No partial tree is ever returned.
pub fn load(dir: &Path) -> Result<Value, Error> {
    let process_env: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect();
    load_with_env(dir, &process_env)
}

/// Like [`load`], but with the process environment supplied explicitly.
///
/// This is the seam the conformance harness uses so that fixtures see exactly
/// the variables their `procenv.json` names and nothing else — no mutation of
/// the real environment, so cases can run in parallel. Not part of the stable
/// surface; use [`load`].
#[doc(hidden)]
pub fn load_with_env(dir: &Path, process_env: &BTreeMap<String, String>) -> Result<Value, Error> {
    // 1. Locate the entrypoint.
    let entrypoint = find_entrypoint(dir)?;

    // 2. Build the variable namespace.
    let vars = envfile::Vars::new(process_env, envfile::load_dir(dir)?);

    // 3. Parse the entrypoint and graft every include.
    let format =
        parse::format_for_path(&entrypoint).expect("entrypoint names carry a supported extension");
    let bytes = fs::read(&entrypoint).map_err(|e| {
        Error::new(
            ErrorCode::Parse,
            format!("cannot read {}: {e}", entrypoint.display()),
        )
    })?;
    let text = String::from_utf8(bytes).map_err(|_| {
        Error::new(
            ErrorCode::Parse,
            format!("{} is not valid UTF-8", entrypoint.display()),
        )
    })?;
    let tree = parse::parse(&text, format, &entrypoint)?;
    // SPEC §3: the entrypoint's top-level value MUST be an object — an empty
    // document (which parses as `null`) included. Included files (§5) are
    // unconstrained; only the entrypoint carries this rule.
    if !tree.is_object() {
        return Err(Error::new(
            ErrorCode::Parse,
            format!(
                "{}: top-level value is {}, not an object",
                entrypoint.display(),
                type_name(&tree)
            ),
        ));
    }

    let canonical = fs::canonicalize(&entrypoint).unwrap_or_else(|_| entrypoint.clone());
    let base_dir = canonical
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let mut stack = vec![canonical];
    let tree = include::resolve(tree, &base_dir, &mut stack)?;

    // 4. Interpolate.
    interp::interpolate(tree, &vars)
}

/// The data-model kind of a value, for the entrypoint-root diagnostic. An empty
/// document reaches this as `null`, which is what the message should say.
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null (an empty document)",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn find_entrypoint(dir: &Path) -> Result<PathBuf, Error> {
    let mut found: Vec<PathBuf> = ENTRYPOINTS
        .iter()
        .map(|name| dir.join(name))
        .filter(|path| path.is_file())
        .collect();

    match found.len() {
        // SPEC §3 folds "directory does not exist or cannot be read" into this
        // same code: with nothing readable there is no entrypoint, and
        // `is_file()` on a path under a missing directory is simply false.
        0 => Err(Error::new(
            ErrorCode::NoEntrypoint,
            format!("no entrypoint.{{json,yaml,yml,toml}} in {}", dir.display()),
        )),
        1 => Ok(found.remove(0)),
        _ => Err(Error::new(
            ErrorCode::MultipleEntrypoints,
            format!(
                "{} holds {} entrypoints: {}",
                dir.display(),
                found.len(),
                found
                    .iter()
                    .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use crate::{load_with_env, ErrorCode};

    /// SPEC §3: "A config directory that does not exist or cannot be read is
    /// `E_NO_ENTRYPOINT`."
    ///
    /// This lives here rather than in `testdata/cases/` because git cannot
    /// track a directory that does not exist — the fixture suite has no way to
    /// express "no `config/` at all", so the rule needs a unit test.
    #[test]
    fn missing_config_directory_is_no_entrypoint() {
        let env = BTreeMap::new();
        let absent = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/no-such-config-dir");
        assert!(!absent.exists(), "the test path must not exist");

        let err = load_with_env(&absent, &env).expect_err("a missing directory cannot load");
        assert_eq!(err.kind(), ErrorCode::NoEntrypoint);
        assert_eq!(err.code(), "E_NO_ENTRYPOINT");
    }

    /// The same code covers a path that exists but is not a directory: there is
    /// no entrypoint inside a regular file either.
    #[test]
    fn a_file_used_as_a_config_directory_is_no_entrypoint() {
        let env = BTreeMap::new();
        let not_a_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(not_a_dir.is_file(), "the test path must be a regular file");

        let err = load_with_env(&not_a_dir, &env).expect_err("a file cannot load");
        assert_eq!(err.kind(), ErrorCode::NoEntrypoint);
    }
}
