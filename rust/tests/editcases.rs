//! The shared editing suite (SPEC §11).
//!
//! One harness walking `../testdata/editcases/`; every case becomes a named
//! trial. Each case runs against a fresh copy of its `config/`; afterwards
//! every file the case did not expect to be written must be byte-identical to
//! the copy, and no file may have been added (no lock, no temporary file).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use libtest_mimic::{Arguments, Failed, Trial};
use serde_json::{json, Value};

use entryconf::{CommitOptions, Edit};

fn cases_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/editcases")
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

fn read(path: &Path) -> Result<String, Failed> {
    fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()).into())
}

fn read_json(path: &Path) -> Result<Option<Value>, Failed> {
    if !path.exists() {
        return Ok(None);
    }
    serde_json::from_str(&read(path)?)
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

fn file_bytes(root: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) -> std::io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            file_bytes(&path, base, out)?;
        } else {
            let rel = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            out.insert(rel, fs::read(&path)?);
        }
    }
    Ok(())
}

fn snapshot_files(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, Failed> {
    let mut out = BTreeMap::new();
    file_bytes(root, root, &mut out).map_err(|e| format!("walking {}: {e}", root.display()))?;
    Ok(out)
}

fn run_case(dir: &Path) -> Result<(), Failed> {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let work = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "target/editcases/{}-{}-{n}",
        std::process::id(),
        dir.file_name().unwrap().to_string_lossy()
    ));
    let _ = fs::remove_dir_all(&work);
    copy_tree(&dir.join("config"), &work).map_err(|e| format!("copying fixture: {e}"))?;
    let result = run_in(dir, &work);
    let _ = fs::remove_dir_all(&work);
    result
}

fn run_in(dir: &Path, work: &Path) -> Result<(), Failed> {
    let mut original = snapshot_files(work)?;

    let env: Arc<Mutex<BTreeMap<String, String>>> = Arc::new(Mutex::new(
        match read_json(&dir.join("procenv.json"))? {
            Some(v) => serde_json::from_value(v).map_err(|e| format!("procenv.json: {e}"))?,
            None => BTreeMap::new(),
        },
    ));
    let captured = env.lock().unwrap().clone();
    let live_env = env.clone();
    let live = move |name: &str| live_env.lock().unwrap().get(name).cloned();

    let request = read_json(&dir.join("request.json"))?.ok_or("missing request.json")?;
    let want_err = dir
        .join("expected_error.txt")
        .exists()
        .then(|| read(&dir.join("expected_error.txt")))
        .transpose()?
        .map(|s| s.trim().to_string());
    let expected = read_json(&dir.join("expected.json"))?;

    let mut expect_written: Vec<String> = Vec::new();
    let outcome: Result<Value, entryconf::Error> = (|| {
        let snap = entryconf::open_with_env(work, &captured, live.clone())?;
        if let Some(Value::String(ptr)) = request.get("inspect") {
            let origin = snap.inspect(ptr)?;
            return Ok(json!({ "origin": serde_json::to_value(&origin).unwrap() }));
        }
        if request.get("documents").is_some() {
            let docs: BTreeMap<&String, Value> = snap
                .documents
                .iter()
                .map(|(k, d)| {
                    (
                        k,
                        json!({"format": d.format, "writable": d.writable, "grafts": d.grafts}),
                    )
                })
                .collect();
            return Ok(json!({ "documents": docs }));
        }
        let edits: Vec<Edit> = entryconf::parse_edits(&request["edits"])?;
        let plan = snap.plan(&edits)?;
        if let Some(bc) = request.get("before_commit") {
            if let Some(Value::Object(files)) = bc.get("files") {
                for (key, text) in files {
                    let text = text.as_str().unwrap();
                    fs::write(work.join(key), text).expect("before_commit write");
                    original.insert(key.clone(), text.as_bytes().to_vec());
                }
            }
            if let Some(Value::Object(pe)) = bc.get("procenv") {
                let mut e = env.lock().unwrap();
                for (k, v) in pe {
                    e.insert(k.clone(), v.as_str().unwrap().to_string());
                }
            }
        }
        let receipt = plan.commit(&CommitOptions::default())?;
        expect_written.push(plan.document.clone());
        assert_eq!(receipt.document, plan.document);
        let reloaded = entryconf::load_with_env(work, &env.lock().unwrap())?;
        assert!(
            equivalent(&reloaded, &plan.candidate),
            "committed tree differs from the plan's candidate"
        );
        let on_disk = fs::read(work.join(&plan.document)).expect("read written document");
        assert_eq!(on_disk, plan.after.as_bytes(), "file on disk is not plan.after");
        let written: Value = serde_json::from_str(&plan.after).expect("plan.after is JSON");
        Ok(json!({
            "tree": reloaded,
            "affected": plan.affected,
            "documents": { plan.document.clone(): written },
        }))
    })();

    match (want_err, outcome) {
        (Some(code), Ok(got)) => return Err(format!("want error {code}, got {got}").into()),
        (Some(code), Err(e)) if e.code() == code => {}
        (Some(code), Err(e)) => {
            return Err(format!("want error {code}, got {} ({})", e.code(), e.message()).into())
        }
        (None, Err(e)) => {
            return Err(format!("unexpected error {} ({})", e.code(), e.message()).into())
        }
        (None, Ok(got)) => {
            let want = expected.ok_or("missing expected.json")?;
            if !equivalent(&got, &want) {
                return Err(format!(
                    "result mismatch\n  want: {}\n  got:  {}",
                    serde_json::to_string(&want).unwrap(),
                    serde_json::to_string(&got).unwrap()
                )
                .into());
            }
        }
    }

    // Filesystem discipline (SPEC §11).
    let after = snapshot_files(work)?;
    for (rel, data) in &after {
        match original.get(rel) {
            None => return Err(format!("file {rel} was created (lock or temp file left behind?)").into()),
            Some(orig) if orig != data && !expect_written.contains(rel) => {
                return Err(format!("file {rel} changed but was not the edited document").into())
            }
            _ => {}
        }
    }
    for rel in original.keys() {
        if !after.contains_key(rel) {
            return Err(format!("file {rel} disappeared").into());
        }
    }
    Ok(())
}

/// Structural equality, numbers compared numerically.
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
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| equivalent(v, w)))
        }
        _ => a == b,
    }
}
