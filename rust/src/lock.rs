//! SPEC §10.7 — the directory lock and the atomic replacement write.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::error::{Error, ErrorCode};

pub(crate) const LOCK_FILE_NAME: &str = ".entryconf.lock";

/// How old a lock file must be before a waiter may presume its writer crashed.
pub(crate) const LOCK_STALE_AFTER: Duration = Duration::from_secs(30);

/// A held lock; dropping it deletes the lock file.
pub(crate) struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Exclusive-creates `<dir>/.entryconf.lock`, retrying with backoff until
/// `timeout` (`E_LOCKED`), breaking a lock older than [`LOCK_STALE_AFTER`] by
/// renaming it to a unique name first so at most one waiter breaks it.
pub(crate) fn acquire(dir: &Path, timeout: Duration) -> Result<LockGuard, Error> {
    let path = dir.join(LOCK_FILE_NAME);
    let deadline = Instant::now() + timeout;
    let mut wait = Duration::from_millis(5);
    loop {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let _ = writeln!(
                    file,
                    "{{\"pid\": {}, \"created\": \"{}\"}}",
                    std::process::id(),
                    unix_time_rfc3339()
                );
                return Ok(LockGuard { path });
            }
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(Error::new(
                    ErrorCode::Locked,
                    format!("cannot create lock file {}: {e}", path.display()),
                ));
            }
            Err(_) => {}
        }
        let is_stale = fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age > LOCK_STALE_AFTER);
        if is_stale {
            let stale = path.with_extension(format!("lock.{}", random_suffix()));
            if fs::rename(&path, &stale).is_ok() {
                let _ = fs::remove_file(&stale);
            }
            continue;
        }
        if Instant::now() >= deadline {
            return Err(Error::new(
                ErrorCode::Locked,
                format!(
                    "lock file {} is held by another writer (waited {timeout:?})",
                    path.display()
                ),
            ));
        }
        std::thread::sleep(wait);
        if wait < Duration::from_millis(100) {
            wait *= 2;
        }
    }
}

fn unix_time_rfc3339() -> String {
    // Informational only; a civil-time rendering without a chrono dependency.
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = secs / 86_400;
    let (h, m, s) = ((secs % 86_400) / 3600, (secs % 3600) / 60, secs % 60);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

pub(crate) fn random_suffix() -> String {
    // Enough entropy to make sibling temp names distinct; no crate needed.
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let addr = &nanos as *const u32 as usize;
    format!("{:x}{:x}{:x}", std::process::id(), nanos, addr & 0xffff)
}

/// Replaces `target`'s content with `data` (SPEC §10.7 step 3): a temporary
/// file beside the resolved target, flushed, given the target's permission
/// bits, and renamed over it. A symlinked target is resolved first so the link
/// survives. On any failure the temporary file is removed and the target is
/// untouched.
pub(crate) fn atomic_write(target: &Path, data: &[u8]) -> Result<(), Error> {
    let write_err = |step: &str, e: std::io::Error, path: &Path| {
        Error::new(ErrorCode::Write, format!("{step} {}: {e}", path.display()))
    };
    let resolved = fs::canonicalize(target).map_err(|e| write_err("cannot resolve", e, target))?;
    let meta = fs::metadata(&resolved).map_err(|e| write_err("cannot stat", e, &resolved))?;
    let dir = resolved
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let base = resolved
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp_path = dir.join(format!(".{base}.entryconf-tmp-{}", random_suffix()));

    let mut tmp = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .map_err(|e| write_err("cannot create a temporary file beside", e, &resolved))?;

    let result = (|| -> std::io::Result<()> {
        tmp.write_all(data)?;
        tmp.sync_all()?;
        #[cfg(unix)]
        tmp.set_permissions(meta.permissions())?;
        #[cfg(not(unix))]
        let _ = &meta;
        Ok(())
    })();
    drop(tmp);
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp_path);
        return Err(write_err("cannot write replacement for", e, &resolved));
    }
    if let Err(e) = fs::rename(&tmp_path, &resolved) {
        let _ = fs::remove_file(&tmp_path);
        return Err(write_err("cannot replace", e, &resolved));
    }
    // Best effort: make the rename itself durable where the platform allows.
    #[cfg(unix)]
    if let Ok(d) = File::open(&dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = File::open(&dir);
    Ok(())
}
