//! SPEC §12.1–12.2 — a snapshot's watch set, and refreshing it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::edit::{affected_pointers, open_tracked, LiveEnv, Snapshot};
use crate::error::Error;
use crate::source::{OsSource, TrackingSource};

/// What can change the result of opening a snapshot's directory again (SPEC
/// §12.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WatchSet {
    /// Always `"."`: the config directory, for entrypoints and `*.env` files
    /// appearing, going or renamed.
    pub directory: String,
    /// The snapshot's document keys, sorted by code point.
    pub documents: Vec<String>,
}

impl Snapshot {
    /// The snapshot's watch set (SPEC §12.1). Reads nothing from disk.
    #[must_use]
    pub fn watch_set(&self) -> WatchSet {
        WatchSet {
            directory: ".".to_owned(),
            // A BTreeMap's keys are in byte order, which for UTF-8 is code
            // point order.
            documents: self.documents.keys().cloned().collect(),
        }
    }

    /// The watch set's documents as the paths the load resolved them to.
    #[cfg(feature = "watch")]
    pub(crate) fn document_paths(&self) -> Vec<PathBuf> {
        self.documents.values().map(|d| d.path.clone()).collect()
    }
}

/// Document keys between two snapshots, each sorted by code point (SPEC §12.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DocumentChanges {
    /// Keys the new snapshot has and the old one does not.
    pub added: Vec<String>,
    /// Keys the old snapshot has and the new one does not.
    pub removed: Vec<String>,
    /// Keys in both whose revisions differ.
    pub changed: Vec<String>,
}

/// What [`refresh`] reports (SPEC §12.2).
pub struct Refreshed {
    /// True iff the open failed, read other documents, read a document whose
    /// revision differs, or looked up a variable whose value differs.
    pub stale: bool,
    /// The new snapshot, when stale and the open succeeded. `None` when not
    /// stale (the old snapshot is still current: keep it) or when the open
    /// failed.
    pub snapshot: Option<Snapshot>,
    /// The open's failure; the old snapshot stays valid.
    pub error: Option<Error>,
    /// The affected pointers between the old and the new tree.
    pub changed: Vec<String>,
    /// The document keys added, removed and changed.
    pub documents: DocumentChanges,
    /// After a failed open, every path it asked about, including those it
    /// could not read: a watcher watches them so creating a missing include
    /// is seen.
    pub tried: Vec<PathBuf>,
}

/// Opens the snapshot's directory again, against the process environment as
/// it is now, and reports whether and what changed (SPEC §12.2). Never writes
/// a file.
#[must_use]
pub fn refresh(snapshot: &Snapshot) -> Refreshed {
    let captured = crate::process_environment();
    refresh_with_env(snapshot, &captured, |name| std::env::var(name).ok())
}

/// Like [`refresh`], with the captured and live environments supplied
/// explicitly. The seam the watching harness uses; not part of the stable
/// surface.
#[doc(hidden)]
pub fn refresh_with_env(
    snapshot: &Snapshot,
    captured: &BTreeMap<String, String>,
    live: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
) -> Refreshed {
    refresh_live(snapshot, captured, Arc::new(live))
}

fn refresh_live(
    snapshot: &Snapshot,
    captured: &BTreeMap<String, String>,
    live: LiveEnv,
) -> Refreshed {
    let src = TrackingSource::new(&OsSource);
    let next = match open_tracked(Path::new(&snapshot.dir), captured, live, &src) {
        Ok(next) => next,
        Err(error) => {
            return Refreshed {
                stale: true,
                snapshot: None,
                error: Some(error),
                changed: Vec::new(),
                documents: DocumentChanges::default(),
                tried: src.paths(),
            }
        }
    };
    let mut documents = DocumentChanges::default();
    for (key, d) in &next.documents {
        match snapshot.documents.get(key) {
            None => documents.added.push(key.clone()),
            Some(old) if old.revision != d.revision => documents.changed.push(key.clone()),
            Some(_) => {}
        }
    }
    for key in snapshot.documents.keys() {
        if !next.documents.contains_key(key) {
            documents.removed.push(key.clone());
        }
    }
    let stale = !(documents.added.is_empty()
        && documents.removed.is_empty()
        && documents.changed.is_empty())
        || next.variables_revision != snapshot.variables_revision;
    if !stale {
        return Refreshed {
            stale,
            snapshot: None,
            error: None,
            changed: Vec::new(),
            documents,
            tried: Vec::new(),
        };
    }
    Refreshed {
        stale,
        changed: affected_pointers(&snapshot.tree, &next.tree),
        snapshot: Some(next),
        error: None,
        documents,
        tried: Vec::new(),
    }
}
