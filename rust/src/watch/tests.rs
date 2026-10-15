//! SPEC §12.3's rules, against real directories.

use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::Receiver;

use serde_json::json;

use super::*;

const SETTLE: Duration = Duration::from_millis(50);
const QUIET: Duration = Duration::from_millis(400);

static SERIAL: AtomicUsize = AtomicUsize::new(0);

/// A fresh directory under target/, removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(files: &[(&str, &str)]) -> Dir {
        let n = SERIAL.fetch_add(1, Ordering::SeqCst);
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("target/watch-tests/{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        let dir = Dir(fs::canonicalize(&path).unwrap());
        for (name, text) in files {
            dir.put(name, text);
        }
        dir
    }

    fn put(&self, name: &str, text: &str) {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// What a test sees of a change.
struct Seen {
    old: Arc<Snapshot>,
    new: Option<Arc<Snapshot>>,
    changed: Vec<String>,
    added: Vec<String>,
    code: Option<String>,
}

fn start(
    dir: &Dir,
    mut answer: impl FnMut(&Change) -> Result<(), Refusal> + Send + 'static,
) -> (Watcher, Receiver<Seen>) {
    let (tx, rx) = mpsc::channel();
    let w = watch(
        &dir.0,
        move |c| {
            let _ = tx.send(Seen {
                old: c.old.clone(),
                new: c.new.clone(),
                changed: c.changed.clone(),
                added: c.documents.added.clone(),
                code: c.error.as_ref().map(|e| e.code().to_owned()),
            });
            answer(c)
        },
        Options { settle: SETTLE },
    )
    .expect("watch");
    (w, rx)
}

fn accept(_: &Change) -> Result<(), Refusal> {
    Ok(())
}

fn next(rx: &Receiver<Seen>) -> Seen {
    rx.recv_timeout(Duration::from_secs(5))
        .expect("no change delivered")
}

fn none(rx: &Receiver<Seen>) {
    if let Ok(seen) = rx.recv_timeout(QUIET) {
        panic!("unexpected change: {:?} {:?}", seen.changed, seen.code);
    }
}

fn tree(s: &Seen) -> serde_json::Value {
    s.new.as_ref().expect("a new snapshot").tree.clone()
}

#[test]
fn burst_is_one_change() {
    let dir = Dir::new(&[("entrypoint.json", r#"{"n": 0}"#)]);
    let (w, rx) = start(&dir, accept);
    for i in 1..=5 {
        dir.put("entrypoint.json", &format!(r#"{{"n": {i}}}"#));
        std::thread::sleep(SETTLE / 5);
    }
    let c = next(&rx);
    assert_eq!(tree(&c)["n"], json!(5));
    assert_eq!(c.changed, vec!["/n"]);
    none(&rx);
    assert!(Arc::ptr_eq(&w.current(), c.new.as_ref().unwrap()));
}

#[test]
fn rename_over_is_one_change() {
    let dir = Dir::new(&[
        ("entrypoint.json", r#"{"db": "@file:db.json"}"#),
        ("db.json", r#"{"port": 1}"#),
    ]);
    let (_w, rx) = start(&dir, accept);
    dir.put(".db.json.swp", r#"{"port": 2}"#);
    fs::rename(dir.0.join(".db.json.swp"), dir.0.join("db.json")).unwrap();
    assert_eq!(next(&rx).changed, vec!["/db/port"]);
    none(&rx);
    dir.put("db.json", r#"{"port": 3}"#);
    assert_eq!(tree(&next(&rx))["db"]["port"], json!(3));
}

#[test]
fn no_empty_calls() {
    let dir = Dir::new(&[("entrypoint.json", r#"{"n": 1}"#)]);
    let (_w, rx) = start(&dir, accept);
    dir.put("entrypoint.json", r#"{"n": 1}"#);
    dir.put("notes.txt", "not config");
    none(&rx);
}

#[test]
fn refused_keeps_old() {
    let dir = Dir::new(&[("entrypoint.json", r#"{"port": 80}"#)]);
    let (w, rx) = start(&dir, |c| match &c.new {
        Some(new) if new.tree["port"] == json!(0) => Err("port out of range".into()),
        _ => Ok(()),
    });
    let first = w.current();
    dir.put("entrypoint.json", r#"{"port": 0}"#);
    assert!(Arc::ptr_eq(&next(&rx).old, &first));
    assert!(Arc::ptr_eq(&w.current(), &first));
    dir.put("entrypoint.json", r#"{"port": 81}"#);
    let c = next(&rx);
    assert!(Arc::ptr_eq(&c.old, &first));
    assert_eq!(c.changed, vec!["/port"]);
    std::thread::sleep(Duration::from_millis(10));
    assert!(Arc::ptr_eq(&w.current(), c.new.as_ref().unwrap()));
}

#[test]
fn refused_new_include_is_watched() {
    let dir = Dir::new(&[("entrypoint.json", r#"{"port": 80}"#)]);
    let (_w, rx) = start(&dir, |c| match &c.new {
        Some(new) if new.tree["db"]["port"] == json!(0) => Err("refused".into()),
        _ => Ok(()),
    });
    dir.put("sub/db.json", r#"{"port": 0}"#);
    dir.put(
        "entrypoint.json",
        r#"{"port": 80, "db": "@file:sub/db.json"}"#,
    );
    next(&rx);
    dir.put("sub/db.json", r#"{"port": 5432}"#);
    let c = next(&rx);
    assert_eq!(c.code, None);
    assert_eq!(tree(&c)["db"]["port"], json!(5432));
}

#[test]
fn one_at_a_time() {
    let running = Arc::new(AtomicBool::new(false));
    let overlapped = Arc::new(AtomicBool::new(false));
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let dir = Dir::new(&[("entrypoint.json", r#"{"n": 0}"#)]);
    let (r, o) = (running.clone(), overlapped.clone());
    let (_w, rx) = start(&dir, move |c| {
        if r.swap(true, Ordering::SeqCst) {
            o.store(true, Ordering::SeqCst);
        }
        if c.new.as_ref().is_some_and(|n| n.tree["n"] == json!(1)) {
            let _ = release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10));
        }
        r.store(false, Ordering::SeqCst);
        Ok(())
    });
    dir.put("entrypoint.json", r#"{"n": 1}"#);
    let first = next(&rx);
    dir.put("entrypoint.json", r#"{"n": 2}"#);
    std::thread::sleep(QUIET);
    dir.put("entrypoint.json", r#"{"n": 3}"#);
    none(&rx);
    release_tx.send(()).unwrap();
    let second = next(&rx);
    assert!(Arc::ptr_eq(&second.old, first.new.as_ref().unwrap()));
    assert_eq!(tree(&second)["n"], json!(3));
    none(&rx);
    assert!(!overlapped.load(Ordering::SeqCst));
}

#[test]
fn failed_open_then_created_include() {
    let dir = Dir::new(&[("entrypoint.json", r#"{"n": 1}"#)]);
    let (w, rx) = start(&dir, accept);
    let first = w.current();
    dir.put("entrypoint.json", r#"{"n": 1, "db": "@file:conf/db.json"}"#);
    let c = next(&rx);
    assert_eq!(c.code.as_deref(), Some("E_INCLUDE"));
    assert!(c.new.is_none());
    assert!(Arc::ptr_eq(&w.current(), &first));
    dir.put("conf/db.json", r#"{"port": 5432}"#);
    let c = next(&rx);
    assert_eq!(c.code, None);
    assert!(Arc::ptr_eq(&c.old, &first));
    assert_eq!(tree(&c)["db"]["port"], json!(5432));
}

#[test]
fn second_entrypoint_and_env_file() {
    let dir = Dir::new(&[("entrypoint.json", r#"{"host": "${EC_WT_HOST:-a}"}"#)]);
    let (_w, rx) = start(&dir, accept);
    dir.put("host.env", "EC_WT_HOST=b\n");
    let c = next(&rx);
    assert_eq!(tree(&c)["host"], json!("b"));
    assert_eq!(c.added, vec!["host.env"]);
    dir.put("entrypoint.yaml", "host: c\n");
    assert_eq!(next(&rx).code.as_deref(), Some("E_MULTIPLE_ENTRYPOINTS"));
}

#[test]
fn close_waits_for_callback() {
    let finished = Arc::new(AtomicBool::new(false));
    let closed = Arc::new(AtomicBool::new(false));
    let late = Arc::new(AtomicBool::new(false));
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let dir = Dir::new(&[("entrypoint.json", r#"{"n": 0}"#)]);
    let (f, c2, l) = (finished.clone(), closed.clone(), late.clone());
    let entered_tx = Mutex::new(entered_tx);
    let (w, _rx) = start(&dir, move |c| {
        if c2.load(Ordering::SeqCst) {
            l.store(true, Ordering::SeqCst);
        }
        if c.new.as_ref().is_some_and(|n| n.tree["n"] == json!(1)) {
            let _ = entered_tx.lock().unwrap().send(());
            std::thread::sleep(QUIET);
            f.store(true, Ordering::SeqCst);
        }
        Ok(())
    });
    dir.put("entrypoint.json", r#"{"n": 1}"#);
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("callback entered");
    dir.put("entrypoint.json", r#"{"n": 2}"#);
    w.close();
    closed.store(true, Ordering::SeqCst);
    assert!(
        finished.load(Ordering::SeqCst),
        "close returned while a callback ran"
    );
    dir.put("entrypoint.json", r#"{"n": 3}"#);
    std::thread::sleep(QUIET);
    assert!(
        !late.load(Ordering::SeqCst),
        "a callback started after close"
    );
}

#[test]
fn watch_refused() {
    let first = Dir::new(&[("entrypoint.json", r#"{"n": 1}"#)]);
    REFUSED.lock().unwrap().push(first.0.clone());
    let err = watch(&first.0, accept, Options::default())
        .err()
        .expect("E_WATCH");
    assert_eq!(err.code(), "E_WATCH");

    let dir = Dir::new(&[("entrypoint.json", r#"{"n": 1}"#), ("sub/x.json", "{}")]);
    let (w, rx) = start(&dir, accept);
    REFUSED.lock().unwrap().push(dir.0.join("sub"));
    dir.put("entrypoint.json", r#"{"n": 1, "x": "@file:sub/x.json"}"#);
    let accepted = next(&rx);
    assert_eq!(accepted.code, None);
    let c = next(&rx);
    assert_eq!(c.code.as_deref(), Some("E_WATCH"));
    assert!(Arc::ptr_eq(&c.old, accepted.new.as_ref().unwrap()));
    assert!(Arc::ptr_eq(&w.current(), accepted.new.as_ref().unwrap()));
}
