//! Where the loader gets its bytes (SPEC §10.2, §10.6).
//!
//! `load` reads the real filesystem. `open` reads it while recording every
//! document, graft, and variable lookup; a plan's candidate is then evaluated
//! against the snapshot's recorded bytes with the edited document replaced in
//! memory, falling back to disk only for a file the snapshot never saw.

use std::borrow::Cow;
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

/// A classified path component for joining SPEC §10.2 document keys,
/// independent of std::path::Component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KeyComponent<'a> {
    Prefix(Cow<'a, str>),
    RootDir,
    CurDir,
    Normal(Cow<'a, str>),
}

impl<'a> From<Component<'a>> for KeyComponent<'a> {
    fn from(c: Component<'a>) -> Self {
        match c {
            Component::Prefix(p) => KeyComponent::Prefix(p.as_os_str().to_string_lossy()),
            Component::RootDir => KeyComponent::RootDir,
            Component::CurDir => KeyComponent::CurDir,
            Component::ParentDir => KeyComponent::Normal(Cow::Borrowed("..")),
            Component::Normal(s) => KeyComponent::Normal(s.to_string_lossy()),
        }
    }
}

pub(crate) fn join_key_components<'a, I>(components: I, is_absolute: bool) -> String
where
    I: IntoIterator<Item = KeyComponent<'a>>,
{
    let mut key = String::new();
    let mut has_prefix = false;
    for component in components {
        match component {
            KeyComponent::Prefix(p) => {
                has_prefix = true;
                key.push_str(&p);
            }
            KeyComponent::RootDir => {
                if !key.ends_with('/') {
                    key.push('/');
                }
            }
            KeyComponent::CurDir => {}
            KeyComponent::Normal(s) => {
                if !key.is_empty() && !key.ends_with('/') {
                    key.push('/');
                }
                key.push_str(&s);
            }
        }
    }
    if is_absolute && !has_prefix && !key.starts_with('/') && cfg!(not(windows)) {
        key.insert(0, '/');
    }
    if key.is_empty() {
        ".".to_string()
    } else {
        key
    }
}

/// SPEC §10.2's document key: the path relative to the config directory,
/// slash-separated, or the absolute path when no relative form exists.
pub(crate) fn document_key(dir: &Path, path: &Path) -> String {
    let rel = relative_to(dir, path).unwrap_or_else(|| path.to_path_buf());
    join_key_components(rel.components().map(KeyComponent::from), rel.is_absolute())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_absolute_path_components_do_not_produce_double_slash() {
        let components = vec![
            KeyComponent::Prefix(Cow::Borrowed("C:")),
            KeyComponent::RootDir,
            KeyComponent::Normal(Cow::Borrowed("root")),
            KeyComponent::Normal(Cow::Borrowed("cfg.json")),
        ];
        let key = join_key_components(components, true);
        assert_eq!(key, "C:/root/cfg.json");
    }

    #[test]
    fn windows_drive_root_components() {
        let components = vec![
            KeyComponent::Prefix(Cow::Borrowed("C:")),
            KeyComponent::RootDir,
        ];
        let key = join_key_components(components, true);
        assert_eq!(key, "C:/");
    }

    #[test]
    fn embedded_cur_dir_components_are_dropped() {
        // SPEC §10.2 keys are lexically normalized: a CurDir segment must
        // neither emit a '.' nor disturb the separator state.
        let components = vec![
            KeyComponent::RootDir,
            KeyComponent::Normal(Cow::Borrowed("sub")),
            KeyComponent::CurDir,
            KeyComponent::Normal(Cow::Borrowed("file.json")),
        ];
        let key = join_key_components(components, true);
        assert_eq!(key, "/sub/file.json");
    }

    #[test]
    fn document_key_relative_and_absolute_unix() {
        let dir = Path::new("/app/config");
        assert_eq!(
            document_key(dir, Path::new("/app/config/entrypoint.json")),
            "entrypoint.json"
        );
        assert_eq!(
            document_key(dir, Path::new("/app/config/sub/inc.json")),
            "sub/inc.json"
        );
        assert_eq!(document_key(dir, Path::new("/app/config")), ".");
        assert_eq!(
            document_key(dir, Path::new("/app/other.json")),
            "../other.json"
        );
        assert_eq!(
            document_key(dir, Path::new("/etc/passwd")),
            "../../etc/passwd"
        );
        assert_eq!(
            document_key(Path::new("app/config"), Path::new("/etc/passwd")),
            "/etc/passwd"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_document_key_absolute_different_drive() {
        // Pins the two Windows std::path facts the platform-independent
        // join_key_components tests above can only assume: a drive-letter
        // absolute path parses as Prefix, RootDir, then Normal components,
        // and relative_to finds no shared root across drives — so the key is
        // produced by the absolute-path fallback rather than a `..` chain.
        let path = Path::new(r"C:\root\cfg.json");
        assert_eq!(
            path.components()
                .map(KeyComponent::from)
                .collect::<Vec<_>>(),
            vec![
                KeyComponent::Prefix(Cow::Borrowed("C:")),
                KeyComponent::RootDir,
                KeyComponent::Normal(Cow::Borrowed("root")),
                KeyComponent::Normal(Cow::Borrowed("cfg.json")),
            ]
        );
        let dir = Path::new(r"D:\other");
        assert_eq!(relative_to(dir, path), None);
        assert_eq!(document_key(dir, path), "C:/root/cfg.json");
    }
}
