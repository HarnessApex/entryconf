//! SPEC §12.3 — [`watch`]: hold a current snapshot of a config directory and
//! hand each change on disk to a callback that accepts or refuses it.
//!
//! Behind the cargo feature `watch`, on [`notify`] (inotify, FSEvents,
//! kqueue, ReadDirectoryChangesW): a program that only loads or edits takes
//! no dependency for it. Nothing is polled. Like every platform watch it does
//! not see changes made on another machine to a network filesystem (NFS,
//! SMB); on such a directory call [`crate::refresh`] yourself.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::edit::{open, Snapshot};
use crate::error::{Error, ErrorCode};
use crate::refresh::{refresh, DocumentChanges};
use crate::ENTRYPOINTS;

/// How long a watcher waits after the last event of a burst before it
/// refreshes (SPEC §12.3).
pub const DEFAULT_SETTLE: Duration = Duration::from_millis(100);

/// What a callback is handed (SPEC §12.3).
pub struct Change {
    /// The current snapshot.
    pub old: Arc<Snapshot>,
    /// The refreshed snapshot; `None` when `error` is set.
    pub new: Option<Arc<Snapshot>>,
    /// The affected pointers between `old` and `new` (SPEC §10.6).
    pub changed: Vec<String>,
    /// The document keys added, removed and changed.
    pub documents: DocumentChanges,
    /// The open's failure, or `E_WATCH` when the platform refused to watch a
    /// path; `old` stays current whatever the callback returns.
    pub error: Option<Error>,
}

/// Why a callback refused a change: the program's own validation.
pub type Refusal = Box<dyn std::error::Error + Send + Sync>;

/// Tunes [`watch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// How long after the last event of a burst a refresh runs.
    pub settle: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            settle: DEFAULT_SETTLE,
        }
    }
}

/// Watches one config directory. Made by [`watch`]; dropping it closes it.
pub struct Watcher {
    current: Arc<Mutex<Arc<Snapshot>>>,
    closing: Arc<AtomicBool>,
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    /// The snapshot [`watch`] opened, or the last one a callback accepted.
    #[must_use]
    pub fn current(&self) -> Arc<Snapshot> {
        self.current.lock().unwrap().clone()
    }

    /// Stops the watch. A callback running is waited for; none starts after
    /// `close` returns.
    pub fn close(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.closing.store(true, Ordering::SeqCst);
        let _ = self.tx.send(Msg::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop();
    }
}

enum Msg {
    Event(Vec<PathBuf>),
    Failed(String),
    Stop,
}

/// Opens `dir` ([`crate::open`]) and watches it (SPEC §12.3). `callback` is
/// called with each stale refresh until the watcher is closed: `Ok(())`
/// accepts the change (`new` becomes current), `Err` refuses it (`old` stays
/// current, and the next change is refreshed from `old`). Callbacks never
/// overlap; they run on the watcher's own thread.
///
/// # Errors
///
/// A failed open, with its code; `E_WATCH` if the platform refuses a watch.
pub fn watch<F>(dir: &Path, callback: F, options: Options) -> Result<Watcher, Error>
where
    F: FnMut(&Change) -> Result<(), Refusal> + Send + 'static,
{
    let snapshot = Arc::new(open(dir)?);
    let (tx, rx) = mpsc::channel();
    let events = tx.clone();
    let platform = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let msg = match res {
            Ok(event) if matches!(event.kind, EventKind::Access(_)) => return,
            Ok(event) => Msg::Event(event.paths),
            Err(e) => Msg::Failed(e.to_string()),
        };
        let _ = events.send(msg);
    })
    .map_err(|e| watch_error(&snapshot.dir, &e.to_string()))?;
    let current = Arc::new(Mutex::new(snapshot.clone()));
    let closing = Arc::new(AtomicBool::new(false));
    let mut worker = Worker {
        platform,
        config: snapshot.dir.clone(),
        config_real: fs::canonicalize(&snapshot.dir).unwrap_or_else(|_| snapshot.dir.clone()),
        watched: BTreeSet::new(),
        names: BTreeMap::new(),
        current: current.clone(),
        closing: closing.clone(),
        callback: Box::new(callback),
        settle: if options.settle.is_zero() {
            DEFAULT_SETTLE
        } else {
            options.settle
        },
    };
    worker.arm(targets(&worker.config, &snapshot.document_paths()))?;
    let thread = std::thread::Builder::new()
        .name("entryconf-watch".to_owned())
        .spawn(move || worker.run(&rx))
        .map_err(|e| watch_error(&snapshot.dir, &e.to_string()))?;
    Ok(Watcher {
        current,
        closing,
        tx,
        thread: Some(thread),
    })
}

type BoxedCallback = Box<dyn FnMut(&Change) -> Result<(), Refusal> + Send>;

struct Worker {
    platform: RecommendedWatcher,
    config: PathBuf,
    config_real: PathBuf,
    watched: BTreeSet<PathBuf>,
    /// Each watched directory (and its real path) and the entry names in it
    /// that matter; the config directory also takes entrypoints and `*.env`.
    names: BTreeMap<PathBuf, BTreeSet<String>>,
    current: Arc<Mutex<Arc<Snapshot>>>,
    closing: Arc<AtomicBool>,
    callback: BoxedCallback,
    settle: Duration,
}

impl Worker {
    fn run(&mut self, rx: &Receiver<Msg>) {
        // A change made between open and the watches being placed is caught
        // by one refresh at the start; it calls nothing if nothing changed.
        let mut due = Some(Instant::now() + self.settle);
        loop {
            let msg = match due {
                Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match msg {
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(Msg::Event(paths)) => {
                    // An event naming no path may be anything.
                    if paths.is_empty() || paths.iter().any(|p| self.relevant(p)) {
                        due = Some(Instant::now() + self.settle);
                    }
                }
                Ok(Msg::Failed(message)) => {
                    let old = self.current();
                    self.deliver(&Change {
                        old,
                        new: None,
                        changed: Vec::new(),
                        documents: DocumentChanges::default(),
                        error: Some(watch_error(&self.config, &message)),
                    });
                }
                Err(RecvTimeoutError::Timeout) => {
                    due = self.refresh().then(|| Instant::now() + self.settle);
                }
            }
        }
    }

    fn current(&self) -> Arc<Snapshot> {
        self.current.lock().unwrap().clone()
    }

    /// Refreshes and delivers; true when a refresh is due again (a directory
    /// newly watched, where a change may have landed before its watch).
    fn refresh(&mut self) -> bool {
        let old = self.current();
        let r = refresh(&old);
        if !r.stale {
            return false;
        }
        let new = r.snapshot.map(Arc::new);
        let failed = r.error.is_some();
        let accepted = self.deliver(&Change {
            old: old.clone(),
            new: new.clone(),
            changed: r.changed,
            documents: r.documents,
            error: r.error,
        });
        let mut paths = old.document_paths();
        if failed {
            paths.extend(r.tried);
        } else if let Some(new) = new {
            if accepted {
                paths = new.document_paths();
                *self.current.lock().unwrap() = new;
            } else {
                paths.extend(new.document_paths());
            }
        }
        match self.arm(targets(&self.config, &paths)) {
            Ok(added) => added,
            Err(error) => {
                let old = self.current();
                self.deliver(&Change {
                    old,
                    new: None,
                    changed: Vec::new(),
                    documents: DocumentChanges::default(),
                    error: Some(error),
                });
                false
            }
        }
    }

    fn deliver(&mut self, change: &Change) -> bool {
        if self.closing.load(Ordering::SeqCst) {
            return false;
        }
        (self.callback)(change).is_ok() && change.error.is_none()
    }

    fn relevant(&self, path: &Path) -> bool {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str()))
        else {
            return false;
        };
        if self
            .names
            .get(dir)
            .is_some_and(|names| names.contains(name))
        {
            return true;
        }
        (dir == self.config || dir == self.config_real)
            && (ENTRYPOINTS.contains(&name) || name.ends_with(".env"))
    }

    /// Watches exactly the directories in `want`; true if any was added.
    /// `E_WATCH` if the platform refused one (the others stay watched).
    fn arm(&mut self, mut want: BTreeMap<PathBuf, BTreeSet<String>>) -> Result<bool, Error> {
        let mut failed = None;
        let mut added = false;
        let dirs: Vec<PathBuf> = want.keys().cloned().collect();
        for dir in dirs {
            if self.watched.contains(&dir) {
                continue;
            }
            match add_watch(&mut self.platform, &dir) {
                Ok(()) => {
                    self.watched.insert(dir);
                    added = true;
                }
                Err(message) => {
                    failed.get_or_insert_with(|| watch_error(&dir, &message));
                    want.remove(&dir);
                }
            }
        }
        let gone: Vec<PathBuf> = self
            .watched
            .iter()
            .filter(|d| !want.contains_key(*d))
            .cloned()
            .collect();
        for dir in gone {
            let _ = self.platform.unwatch(&dir); // a directory gone is unwatched anyway
            self.watched.remove(&dir);
        }
        // Some platforms (FSEvents) report a directory by its real path.
        let mut names = want.clone();
        for (dir, entries) in want {
            if let Ok(real) = fs::canonicalize(&dir) {
                names.entry(real).or_insert(entries);
            }
        }
        self.names = names;
        match failed {
            Some(error) => Err(error),
            None => Ok(added),
        }
    }
}

#[cfg(test)]
static REFUSED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn add_watch(platform: &mut RecommendedWatcher, dir: &Path) -> Result<(), String> {
    #[cfg(test)]
    if REFUSED.lock().unwrap().iter().any(|d| d == dir) {
        return Err("no space left on device (inotify watches)".to_owned());
    }
    platform
        .watch(dir, RecursiveMode::NonRecursive)
        .map_err(|e| e.to_string())
}

fn watch_error(path: &Path, message: &str) -> Error {
    Error::new(
        ErrorCode::Watch,
        format!("cannot watch {}: {message}", path.display()),
    )
}

/// Directories to watch and the entry names in each that matter. A path's own
/// directory is watched (an editor's rename replaces the file); a symbolic
/// link is also watched where it points; a path that does not exist (what a
/// failed open could not read) is watched at its nearest existing ancestor,
/// by the name of the next step down, so its creation is seen.
fn targets(config: &Path, paths: &[PathBuf]) -> BTreeMap<PathBuf, BTreeSet<String>> {
    let mut out: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    out.insert(config.to_path_buf(), BTreeSet::new());
    let mut add = |dir: &Path, name: &std::ffi::OsStr| {
        out.entry(dir.to_path_buf())
            .or_default()
            .insert(name.to_string_lossy().into_owned());
    };
    for p in paths {
        if p == config {
            continue;
        }
        let mut q = p.as_path();
        while let (Some(dir), Some(name)) = (q.parent(), q.file_name()) {
            if dir.is_dir() {
                add(dir, name);
                break;
            }
            q = dir;
        }
        if p.is_symlink() {
            if let Ok(real) = fs::canonicalize(p) {
                if let (Some(dir), Some(name)) = (real.parent(), real.file_name()) {
                    add(dir, name);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
