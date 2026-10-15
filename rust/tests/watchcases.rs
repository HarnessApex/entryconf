//! The shared watching suite (SPEC §13).
//!
//! One harness walking `../testdata/watchcases/`; every case becomes a named
//! trial: open a copy of `config/` (with `outside/` beside it), take its watch
//! set, apply `change.json`, refresh, and compare with `expected.json`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use libtest_mimic::{Arguments, Failed, Trial};
use serde_json::{json, Value};

fn cases_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/watchcases")
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

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
            let name = dir.file_name().unwrap().to_string_lossy().into_owned();
            Trial::test(name, move || run_case(&dir))
        })
        .collect();
    libtest_mimic::run(&args, trials).exit_code()
}

fn read_json(path: &Path) -> Result<Option<Value>, Failed> {
    if !path.exists() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()).into())
}

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn run_case(dir: &Path) -> Result<(), Failed> {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let work = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "target/watchcases/{}-{}-{n}",
        std::process::id(),
        dir.file_name().unwrap().to_string_lossy()
    ));
    let _ = fs::remove_dir_all(&work);
    let result = run_in(dir, &work);
    let _ = fs::remove_dir_all(&work);
    result
}

fn run_in(dir: &Path, work: &Path) -> Result<(), Failed> {
    let config = work.join("config");
    copy_tree(&dir.join("config"), &config).map_err(|e| format!("copying fixture: {e}"))?;
    if dir.join("outside").is_dir() {
        copy_tree(&dir.join("outside"), work).map_err(|e| format!("copying outside/: {e}"))?;
    }
    let env: Arc<Mutex<BTreeMap<String, String>>> =
        Arc::new(Mutex::new(match read_json(&dir.join("procenv.json"))? {
            Some(v) => serde_json::from_value(v).map_err(|e| format!("procenv.json: {e}"))?,
            None => BTreeMap::new(),
        }));
    let live = |env: &Arc<Mutex<BTreeMap<String, String>>>| {
        let env = env.clone();
        move |name: &str| env.lock().unwrap().get(name).cloned()
    };
    let captured = env.lock().unwrap().clone();
    let snap = entryconf::open_with_env(&config, &captured, live(&env))
        .map_err(|e| format!("open: {e}"))?;
    let watch = serde_json::to_value(snap.watch_set()).unwrap();

    let change = read_json(&dir.join("change.json"))?.ok_or("missing change.json")?;
    if let Some(files) = change.get("files").and_then(Value::as_object) {
        for (key, text) in files {
            let path = config.join(key);
            match text {
                Value::Null => fs::remove_file(&path).map_err(|e| format!("change: {e}"))?,
                Value::String(text) => {
                    fs::create_dir_all(path.parent().unwrap())
                        .map_err(|e| format!("change: {e}"))?;
                    fs::write(&path, text).map_err(|e| format!("change: {e}"))?;
                }
                other => return Err(format!("change.json: {key} is {other}").into()),
            }
        }
    }
    if let Some(procenv) = change.get("procenv").and_then(Value::as_object) {
        let mut env = env.lock().unwrap();
        for (name, value) in procenv {
            match value {
                Value::Null => {
                    env.remove(name);
                }
                Value::String(v) => {
                    env.insert(name.clone(), v.clone());
                }
                other => return Err(format!("change.json: {name} is {other}").into()),
            }
        }
    }

    let captured = env.lock().unwrap().clone();
    let r = entryconf::refresh_with_env(&snap, &captured, live(&env));
    let mut got = json!({
        "watch": watch,
        "stale": r.stale,
        "changed": r.changed,
        "documents": serde_json::to_value(&r.documents).unwrap(),
    });
    match (&r.error, &r.snapshot) {
        (Some(e), None) => got["error"] = json!(e.code()),
        (None, Some(next)) => got["tree"] = next.tree.clone(),
        (None, None) if !r.stale => got["tree"] = snap.tree.clone(),
        _ => {
            return Err("a refresh carries a snapshot and an error, or neither while stale".into())
        }
    }
    let expected = read_json(&dir.join("expected.json"))?.ok_or("missing expected.json")?;
    if !equivalent(&got, &expected) {
        return Err(format!("result mismatch:\n  got      {got}\n  expected {expected}").into());
    }
    Ok(())
}

/// Structural equality; numbers compare numerically.
fn equivalent(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
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
