//! What the editing fixtures cannot express (SPEC §10.9): concurrency, lock
//! behavior, filesystem failure, permissions, symlinks, and the snapshot's
//! environment contract.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use serde_json::json;

use entryconf::{CommitOptions, Edit, ErrorCode};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn scratch(entrypoint: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("target/edit-unit/{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("entrypoint.json"), entrypoint).unwrap();
    dir
}

fn opts(ms: u64) -> CommitOptions {
    CommitOptions {
        lock_timeout: Duration::from_millis(ms),
    }
}

#[test]
fn two_cooperating_writers_the_second_is_stale() {
    let dir = scratch(r#"{"a": 1, "b": 2}"#);
    let snap = entryconf::open(&dir).unwrap();
    let p1 = snap.plan(&[Edit::set("entrypoint.json", "/a", json!(10))]).unwrap();
    let p2 = snap.plan(&[Edit::set("entrypoint.json", "/b", json!(20))]).unwrap();
    p1.commit(&CommitOptions::default()).unwrap();
    assert_eq!(p2.commit(&CommitOptions::default()).unwrap_err().kind(), ErrorCode::StalePlan);
    // The same plan twice is stale too: its own write moved the revision.
    assert_eq!(p1.commit(&CommitOptions::default()).unwrap_err().kind(), ErrorCode::StalePlan);
    let tree = entryconf::load(&dir).unwrap();
    assert_eq!(tree, json!({"a": 10, "b": 2}));
}

#[test]
fn a_held_lock_is_locked_and_a_stale_lock_is_broken() {
    let dir = scratch(r#"{"a": 1}"#);
    let snap = entryconf::open(&dir).unwrap();
    let plan = snap.plan(&[Edit::set("entrypoint.json", "/a", json!(2))]).unwrap();
    let lock = dir.join(".entryconf.lock");
    fs::write(&lock, "{}").unwrap();

    let start = Instant::now();
    assert_eq!(plan.commit(&opts(150)).unwrap_err().kind(), ErrorCode::Locked);
    assert!(start.elapsed() < Duration::from_secs(2), "timeout not respected");
    assert_eq!(fs::read_to_string(dir.join("entrypoint.json")).unwrap(), r#"{"a": 1}"#);
    assert!(lock.exists(), "another writer's lock was removed");

    // Older than the stale threshold: broken, and the commit proceeds.
    let old = SystemTime::now() - Duration::from_secs(120);
    File::options().write(true).open(&lock).unwrap().set_modified(old).unwrap();
    plan.commit(&opts(1000)).expect("stale lock is broken");
    assert!(!lock.exists(), "lock file left behind after commit");
}

#[cfg(unix)]
#[test]
fn commit_preserves_permission_bits() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch(r#"{"a": 1}"#);
    let target = dir.join("entrypoint.json");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let snap = entryconf::open(&dir).unwrap();
    let plan = snap.plan(&[Edit::set("entrypoint.json", "/a", json!(2))]).unwrap();
    plan.commit(&CommitOptions::default()).unwrap();
    assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);
}

#[cfg(unix)]
#[test]
fn a_failed_write_leaves_the_source_and_no_temp_file() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch(r#"{"x": "@file:sub/x.json"}"#);
    let sub = dir.join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("x.json"), r#"{"v": 1}"#).unwrap();
    let snap = entryconf::open(&dir).unwrap();
    let plan = snap.plan(&[Edit::set("sub/x.json", "/v", json!(2))]).unwrap();

    fs::set_permissions(&sub, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::write(sub.join("probe"), "x").is_ok() {
        // Root (or an ACL) ignores the mode; the failure cannot be provoked.
        let _ = fs::remove_file(sub.join("probe"));
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let err = plan.commit(&CommitOptions::default()).unwrap_err();
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(err.kind(), ErrorCode::Write);
    let entries: Vec<_> = fs::read_dir(&sub).unwrap().collect();
    assert_eq!(entries.len(), 1, "temporary file left behind");
    assert_eq!(fs::read_to_string(sub.join("x.json")).unwrap(), r#"{"v": 1}"#);
    assert!(!dir.join(".entryconf.lock").exists(), "lock left after a failed commit");
}

#[cfg(unix)]
#[test]
fn commit_through_a_symlink_replaces_the_target_and_keeps_the_link() {
    let dir = scratch(r#"{"x": "@file:link.json"}"#);
    let real = dir.join("real-target.json");
    fs::write(&real, r#"{"v": 1}"#).unwrap();
    std::os::unix::fs::symlink(&real, dir.join("link.json")).unwrap();
    let snap = entryconf::open(&dir).unwrap();
    let plan = snap.plan(&[Edit::set("link.json", "/v", json!(2))]).unwrap();
    plan.commit(&CommitOptions::default()).unwrap();
    assert!(fs::symlink_metadata(dir.join("link.json")).unwrap().file_type().is_symlink());
    assert!(fs::read_to_string(&real).unwrap().contains("\"v\": 2"));
}

#[test]
fn plan_does_not_touch_disk() {
    let dir = scratch(r#"{"a": 1}"#);
    let snap = entryconf::open(&dir).unwrap();
    snap.plan(&[Edit::set("entrypoint.json", "/a", json!(2))]).unwrap();
    assert_eq!(fs::read_to_string(dir.join("entrypoint.json")).unwrap(), r#"{"a": 1}"#);
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn a_live_environment_change_makes_a_plan_stale() {
    // `open` captures the real environment; the seam lets this test change
    // the live view without racing other tests on the process environment.
    let dir = scratch(r#"{"host": "${EC_EDIT_HOST}"}"#);
    let captured = [("EC_EDIT_HOST".to_string(), "prod".to_string())].into();
    let live_value = std::sync::Arc::new(std::sync::Mutex::new("prod".to_string()));
    let seen = live_value.clone();
    let snap = entryconf::open_with_env(&dir, &captured, move |name| {
        (name == "EC_EDIT_HOST").then(|| seen.lock().unwrap().clone())
    })
    .unwrap();
    assert_eq!(snap.tree["host"], "prod");
    let origin = snap.inspect("/host").unwrap();
    assert_eq!(origin.variables.len(), 1);
    assert_eq!(origin.variables[0].origin, entryconf::VariableOrigin::Process);

    let plan = snap.plan(&[Edit::set("entrypoint.json", "/x", json!(1))]).unwrap();
    *live_value.lock().unwrap() = "other".to_string();
    assert_eq!(plan.commit(&CommitOptions::default()).unwrap_err().kind(), ErrorCode::StalePlan);
}

#[test]
fn edits_deserialize_from_the_request_shape() {
    let edits: Vec<Edit> = serde_json::from_value(json!([
        {"document": "a.json", "op": "set", "pointer": "/k", "value": "$X"},
        {"document": "a.json", "op": "set", "pointer": "/e", "value": "$X", "mode": "expression"},
        {"document": "a.json", "op": "remove", "pointer": "/r"}
    ]))
    .unwrap();
    assert_eq!(edits[0], Edit::set("a.json", "/k", json!("$X")));
    assert_eq!(edits[1], Edit::set_expression("a.json", "/e", "$X"));
    assert_eq!(edits[2], Edit::remove("a.json", "/r"));
}
