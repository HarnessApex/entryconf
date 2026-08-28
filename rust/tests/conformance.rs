//! The shared conformance suite (SPEC §8).
//!
//! One harness, walking `../testdata/cases/`; every case directory becomes a
//! named trial via `libtest-mimic`, so `cargo test` reports `06-include ... ok`
//! per fixture. Nothing here is hand-written per case — the fixtures are the
//! only source of truth.
//!
//! The process environment is *injected*, not mutated: each case sees exactly
//! the variables its `procenv.json` names and nothing else, which satisfies the
//! "case-named vars otherwise unset" contract and lets trials run in parallel.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use libtest_mimic::{Arguments, Failed, Trial};
use serde_json::Value;

fn cases_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/cases")
}

fn main() -> std::process::ExitCode {
    let args = Arguments::from_args();
    let root = cases_dir();

    let mut dirs: Vec<PathBuf> = fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", root.display()))
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty(), "no cases under {}", root.display());

    let trials: Vec<Trial> = dirs
        .into_iter()
        .map(|dir| {
            let name = dir
                .file_name()
                .expect("case directory has a name")
                .to_string_lossy()
                .into_owned();
            Trial::test(name, move || run_case(&dir))
        })
        .collect();

    libtest_mimic::run(&args, trials).exit_code()
}

fn run_case(dir: &Path) -> Result<(), Failed> {
    let config = dir.join("config");
    if !config.is_dir() {
        return Err(format!("missing config/ in {}", dir.display()).into());
    }

    let procenv_path = dir.join("procenv.json");
    let process_env: BTreeMap<String, String> = if procenv_path.exists() {
        serde_json::from_str(&read(&procenv_path)?)
            .map_err(|e| format!("{}: {e}", procenv_path.display()))?
    } else {
        BTreeMap::new()
    };

    let result = entryconf::load_with_env(&config, &process_env);

    let error_path = dir.join("expected_error.txt");
    let tree_path = dir.join("expected.json");
    match (error_path.exists(), tree_path.exists()) {
        (true, true) => Err(format!(
            "{}: has both expected.json and expected_error.txt",
            dir.display()
        )
        .into()),
        (false, false) => Err(format!(
            "{}: has neither expected.json nor expected_error.txt",
            dir.display()
        )
        .into()),
        (true, false) => {
            let want = read(&error_path)?.trim().to_string();
            match result {
                Ok(tree) => Err(format!("want error {want}, got tree {tree}").into()),
                Err(e) if e.code() == want => Ok(()),
                Err(e) => {
                    Err(format!("want error {want}, got {} ({})", e.code(), e.message()).into())
                }
            }
        }
        (false, true) => {
            let want: Value = serde_json::from_str(&read(&tree_path)?)
                .map_err(|e| format!("{}: {e}", tree_path.display()))?;
            match result {
                Err(e) => {
                    Err(format!("want tree, got error {} ({})", e.code(), e.message()).into())
                }
                Ok(got) if equivalent(&got, &want) => Ok(()),
                Ok(got) => Err(format!(
                    "tree mismatch\n  want: {}\n  got:  {}",
                    serde_json::to_string(&want).expect("serializable"),
                    serde_json::to_string(&got).expect("serializable"),
                )
                .into()),
            }
        }
    }
}

fn read(path: &Path) -> Result<String, Failed> {
    fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()).into())
}

/// Structural equality, with numbers compared numerically (`8080` == `8080.0`).
fn equivalent(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(p), Some(q)) => p == q,
            _ => x == y,
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| equivalent(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| equivalent(v, w)))
        }
        _ => a == b,
    }
}
