//! Where the loader gets its bytes (SPEC §10.2, §10.6).
//!
//! `load` reads the real filesystem. `open` reads it while recording every
//! document, graft, and variable lookup; a plan's candidate is then evaluated
//! against the snapshot's recorded bytes with the edited document replaced in
//! memory, falling back to disk only for a file the snapshot never saw.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::edit::Format;

pub(crate) trait FileSource {
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn is_file(&self, path: &Path) -> bool;
    /// The `*.env` files directly in `dir` (SPEC §4), by name.
    fn env_file_names(&self, dir: &Path) -> io::Result<Vec<String>>;
}

/// The real filesystem.
pub(crate) struct OsSource;

impl FileSource for OsSource {
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn env_file_names(&self, dir: &Path) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let is_env = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".env"));
            if is_env && path.is_file() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        Ok(names)
    }
}

/// Recorded bytes first, disk second; the `*.env` listing is the one the
/// snapshot saw.
pub(crate) struct OverlaySource {
    pub(crate) files: BTreeMap<PathBuf, Vec<u8>>,
    pub(crate) env_names: Vec<String>,
}

impl FileSource for OverlaySource {
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        match self.files.get(path) {
            Some(data) => Ok(data.clone()),
            None => fs::read(path),
        }
    }

    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(path) || path.is_file()
    }

    fn env_file_names(&self, _dir: &Path) -> io::Result<Vec<String>> {
        Ok(self.env_names.clone())
    }
}

/// One graft as the resolver sees it: the referencing chain is by lexical
/// path; `open` turns paths into document keys.
#[derive(Clone)]
pub(crate) struct RawGraft {
    pub(crate) effective: String,
    pub(crate) chain: Vec<(PathBuf, String)>,
}

pub(crate) struct DocRecord {
    pub(crate) path: PathBuf,
    pub(crate) data: Vec<u8>,
    pub(crate) format: Format,
    pub(crate) parsed: Option<Value>,
    pub(crate) grafts: Vec<RawGraft>,
}

/// Captures what one load touched (SPEC §10.2).
#[derive(Default)]
pub(crate) struct Recorder {
    docs: RefCell<BTreeMap<PathBuf, DocRecord>>,
    order: RefCell<Vec<PathBuf>>,
}

impl Recorder {
    pub(crate) fn record(&self, path: &Path, data: &[u8], format: Format, parsed: Option<Value>) {
        let mut docs = self.docs.borrow_mut();
        match docs.get_mut(path) {
            // A shared include is parsed once per reference and recorded once;
            // the entrypoint may have been grafted before it was recorded.
            Some(existing) if !existing.data.is_empty() || existing.parsed.is_some() => {}
            Some(existing) => {
                existing.data = data.to_vec();
                existing.format = format;
                existing.parsed = parsed;
            }
            None => {
                docs.insert(
                    path.to_path_buf(),
                    DocRecord {
                        path: path.to_path_buf(),
                        data: data.to_vec(),
                        format,
                        parsed,
                        grafts: Vec::new(),
                    },
                );
                self.order.borrow_mut().push(path.to_path_buf());
            }
        }
    }

    pub(crate) fn graft(&self, path: &Path, graft: RawGraft) {
        let mut docs = self.docs.borrow_mut();
        if !docs.contains_key(path) {
            docs.insert(
                path.to_path_buf(),
                DocRecord {
                    path: path.to_path_buf(),
                    data: Vec::new(),
                    format: Format::Json,
                    parsed: None,
                    grafts: Vec::new(),
                },
            );
            self.order.borrow_mut().push(path.to_path_buf());
        }
        docs.get_mut(path).expect("inserted above").grafts.push(graft);
    }

    pub(crate) fn into_docs(self) -> (Vec<PathBuf>, BTreeMap<PathBuf, DocRecord>) {
        (self.order.into_inner(), self.docs.into_inner())
    }
}

/// Lexically normalizes a path: `.` dropped, `..` folded onto the preceding
/// normal component (or kept at the front when there is nothing to fold),
/// like Go's `filepath.Clean`. No filesystem access, so symlinks are not
/// followed — that is deliberate: SPEC §10.2 keys documents by the path as
/// referenced.
pub(crate) fn lexical_clean(path: &Path) -> PathBuf {
    let mut out: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => out.push(component),
            },
            other => out.push(other),
        }
    }
    let mut result = PathBuf::new();
    for component in out {
        result.push(component.as_os_str());
    }
    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

/// Resolves `target` against the directory of the referencing file, cleaned
/// (SPEC §5): the same computation the include resolver makes, shared with
/// `inspect`.
pub(crate) fn include_target(target: &str, base_dir: &Path) -> PathBuf {
    let joined = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        base_dir.join(target)
    };
    lexical_clean(&joined)
}

/// SPEC §10.2's document key: the path relative to the config directory,
/// slash-separated, or the absolute path when no relative form exists.
pub(crate) fn document_key(dir: &Path, path: &Path) -> String {
    let rel = relative_to(dir, path).unwrap_or_else(|| path.to_path_buf());
    let mut key = String::new();
    for (i, component) in rel.components().enumerate() {
        if i > 0 {
            key.push('/');
        }
        match component {
            Component::RootDir => {}
            other => key.push_str(&other.as_os_str().to_string_lossy()),
        }
    }
    if rel.is_absolute() && !key.starts_with('/') && cfg!(not(windows)) {
        key.insert(0, '/');
    }
    if key.is_empty() {
        ".".to_string()
    } else {
        key
    }
}

/// A lexical relative path from `base` to `path` (both absolute and cleaned),
/// like Go's `filepath.Rel`; `None` when they share no common root (a
/// different Windows drive, or one absolute and one not).
fn relative_to(base: &Path, path: &Path) -> Option<PathBuf> {
    let base: Vec<Component<'_>> = base.components().collect();
    let target: Vec<Component<'_>> = path.components().collect();
    if base.first().map(component_is_root) != target.first().map(component_is_root) {
        return None;
    }
    if let (Some(Component::Prefix(a)), Some(Component::Prefix(b))) = (base.first(), target.first()) {
        if a != b {
            return None;
        }
    }
    let mut common = 0;
    while common < base.len() && common < target.len() && base[common] == target[common] {
        common += 1;
    }
    let mut rel = PathBuf::new();
    for component in &base[common..] {
        if matches!(component, Component::Normal(_)) {
            rel.push("..");
        }
    }
    for component in &target[common..] {
        rel.push(component.as_os_str());
    }
    Some(rel)
}

fn component_is_root(c: &Component<'_>) -> bool {
    matches!(c, Component::RootDir | Component::Prefix(_))
}
