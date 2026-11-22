//! The file system and the search path, as the checks ask about them.
//!
//! `mx_ports::Fs` has no real adapter, cannot tell a missing directory from an empty one, and
//! has no walk, so the checks ask this trait instead. [`RealHost`] answers from the machine;
//! [`FakeHost`] is an in-memory tree for the tests, so no test reads the machine it runs on.
//! Every question that follows a path follows symbolic links, as `[ -d ]`, `[ -f ]` and
//! `[ -x ]` do, unless its documentation says otherwise.

use mx_core::Fact;
use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::ffi::{OsStr, OsString};
use std::hash::{BuildHasher, Hasher};
use std::io::Read;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// What a path is, after following links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Missing,
    Dir,
    File,
    Other,
}

/// What a directory entry is, links not followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// One entry of [`Host::walk`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: EntryKind,
}

/// How a file operation failed, in the words of the C library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    NotFound,
    Permission,
    IsDirectory,
    Other,
}

/// A file operation that failed. `reason` is `strerror` text: `No such file or directory`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostError {
    pub failure: Failure,
    pub reason: String,
}

impl HostError {
    fn of(error: &std::io::Error) -> Self {
        let text = error.to_string();
        let reason = match text.rfind(" (os error ") {
            Some(at) => text[..at].to_string(),
            None => text,
        };
        let failure = match error.kind() {
            std::io::ErrorKind::NotFound => Failure::NotFound,
            std::io::ErrorKind::PermissionDenied => Failure::Permission,
            std::io::ErrorKind::IsADirectory => Failure::IsDirectory,
            _ => Failure::Other,
        };
        Self { failure, reason }
    }

    /// A failure with a chosen class and wording, for the fake.
    #[must_use]
    pub fn new(failure: Failure, reason: &str) -> Self {
        Self { failure, reason: reason.to_string() }
    }
}

/// The host's file system and search path.
pub trait Host {
    /// What the path is, after following links. A path that cannot be examined is `Missing`.
    fn kind(&self, path: &Path) -> Kind;

    /// Whether the path itself is a symbolic link (`[ -L ]`).
    fn is_symlink(&self, path: &Path) -> bool;

    /// Whether anything stands at the path, a dangling link included (what a glob lists).
    fn lexists(&self, path: &Path) -> bool;

    /// `[ -r ]`: whether the contents can be read.
    fn readable(&self, path: &Path) -> bool;

    /// The bytes of a file.
    fn read(&self, path: &Path) -> Fact<Vec<u8>>;

    /// At most `limit` bytes from the start of a file (`head -c`).
    fn read_head(&self, path: &Path, limit: usize) -> Fact<Vec<u8>>;

    /// The names in a directory, sorted by bytes, dot files included. `Absent` when the
    /// path is not a directory.
    fn list(&self, dir: &Path) -> Fact<Vec<OsString>>;

    /// `find START ...` without `-L`: every entry below `start` down to `max_depth` levels
    /// (`None` is no limit), the start itself left out, symbolic links reported and not
    /// entered. A start that is a link, or is missing, has no entries.
    fn walk(&self, start: &Path, max_depth: Option<usize>) -> Vec<Entry>;

    /// `readlink -f`: the path with every link resolved, when it exists.
    fn real_path(&self, path: &Path) -> Option<PathBuf>;

    /// `command -v NAME` against `search`: whether a regular file with an execute bit
    /// stands in one of its directories. An empty entry means the current directory.
    fn on_path(&self, program: &str, search: &OsStr) -> bool;

    /// `mktemp -d` under `base`: a new directory named `tmp.` and ten letters or digits.
    fn make_temp_dir(&self, base: &Path) -> Result<PathBuf, HostError>;

    /// `mktemp` under `base`: a new empty file named like [`Host::make_temp_dir`]'s directory.
    fn make_temp_file(&self, base: &Path) -> Result<PathBuf, HostError>;

    /// `mkdir -p`.
    fn make_dirs(&self, path: &Path) -> Result<(), HostError>;

    /// Writes a file, creating or replacing it.
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), HostError>;

    /// `cp FILE DIR/`: copies a regular file into an existing directory under its own name.
    fn copy_into_dir(&self, from: &Path, dir: &Path) -> Result<(), CopyError>;

    /// `rm -f`.
    fn remove_file(&self, path: &Path);

    /// `rm -rf`.
    fn remove_tree(&self, path: &Path);
}

/// Why a `cp` failed, split the way its message is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyError {
    /// `cp: cannot stat 'FROM': ...`
    Stat(HostError),
    /// `cp: cannot open 'FROM' for reading: ...`
    Open(HostError),
    /// `cp: cannot create regular file 'TO': ...`
    Create(HostError),
}

impl CopyError {
    /// The line `cp` prints for this failure, without its newline.
    #[must_use]
    pub fn message(&self, from: &Path, dir: &Path) -> Vec<u8> {
        let name = from.file_name().unwrap_or_default();
        let target = dir.join(name);
        let mut text = b"cp: ".to_vec();
        match self {
            Self::Stat(error) => {
                text.extend_from_slice(b"cannot stat '");
                text.extend_from_slice(from.as_os_str().as_bytes());
                text.extend_from_slice(format!("': {}", error.reason).as_bytes());
            }
            Self::Open(error) => {
                text.extend_from_slice(b"cannot open '");
                text.extend_from_slice(from.as_os_str().as_bytes());
                text.extend_from_slice(format!("' for reading: {}", error.reason).as_bytes());
            }
            Self::Create(error) => {
                text.extend_from_slice(b"cannot create regular file '");
                text.extend_from_slice(target.as_os_str().as_bytes());
                text.extend_from_slice(format!("': {}", error.reason).as_bytes());
            }
        }
        text
    }
}

fn temp_name() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut seed = RandomState::new().build_hasher().finish();
    let mut suffix = String::new();
    for _ in 0..10 {
        suffix.push(char::from(ALPHABET[usize::try_from(seed % 62).unwrap_or(0)]));
        seed /= 62;
        if seed == 0 {
            seed = RandomState::new().build_hasher().finish() | 1;
        }
    }
    format!("tmp.{suffix}")
}

/// Answers from the machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealHost;

impl RealHost {
    fn walk_into(dir: &Path, depth: usize, max_depth: Option<usize>, out: &mut Vec<Entry>) {
        let Ok(reader) = std::fs::read_dir(dir) else { return };
        for entry in reader.flatten() {
            let path = entry.path();
            let kind = match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_symlink() => EntryKind::Symlink,
                Ok(meta) if meta.is_dir() => EntryKind::Dir,
                Ok(meta) if meta.is_file() => EntryKind::File,
                _ => EntryKind::Other,
            };
            out.push(Entry { path: path.clone(), kind });
            if kind == EntryKind::Dir && max_depth.is_none_or(|limit| depth < limit) {
                Self::walk_into(&path, depth + 1, max_depth, out);
            }
        }
    }
}

impl Host for RealHost {
    fn kind(&self, path: &Path) -> Kind {
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => Kind::Dir,
            Ok(meta) if meta.is_file() => Kind::File,
            Ok(_) => Kind::Other,
            Err(_) => Kind::Missing,
        }
    }

    fn is_symlink(&self, path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
    }

    fn lexists(&self, path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok()
    }

    fn readable(&self, path: &Path) -> bool {
        match self.kind(path) {
            Kind::Dir => std::fs::read_dir(path).is_ok(),
            Kind::Missing => false,
            Kind::File | Kind::Other => std::fs::File::open(path).is_ok(),
        }
    }

    fn read(&self, path: &Path) -> Fact<Vec<u8>> {
        match std::fs::read(path) {
            Ok(bytes) => Fact::Known(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Fact::Absent,
            Err(error) => Fact::Unreadable(HostError::of(&error).reason),
        }
    }

    fn read_head(&self, path: &Path, limit: usize) -> Fact<Vec<u8>> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Fact::Absent,
            Err(error) => return Fact::Unreadable(HostError::of(&error).reason),
        };
        let mut bytes = Vec::new();
        match file.take(limit as u64).read_to_end(&mut bytes) {
            Ok(_) => Fact::Known(bytes),
            Err(error) => Fact::Unreadable(HostError::of(&error).reason),
        }
    }

    fn list(&self, dir: &Path) -> Fact<Vec<OsString>> {
        match std::fs::read_dir(dir) {
            Ok(reader) => {
                let mut names: Vec<OsString> =
                    reader.flatten().map(|entry| entry.file_name()).collect();
                names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
                Fact::Known(names)
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || error.kind() == std::io::ErrorKind::NotADirectory =>
            {
                Fact::Absent
            }
            Err(error) => Fact::Unreadable(HostError::of(&error).reason),
        }
    }

    fn walk(&self, start: &Path, max_depth: Option<usize>) -> Vec<Entry> {
        let mut out = Vec::new();
        match std::fs::symlink_metadata(start) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
                Self::walk_into(start, 1, max_depth, &mut out);
            }
            _ => {}
        }
        out
    }

    fn real_path(&self, path: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(path).ok()
    }

    fn on_path(&self, program: &str, search: &OsStr) -> bool {
        if program.is_empty() || program.contains('/') {
            return false;
        }
        search.as_bytes().split(|byte| *byte == b':').any(|entry| {
            let dir = if entry.is_empty() {
                PathBuf::from(".")
            } else {
                PathBuf::from(OsStr::from_bytes(entry))
            };
            std::fs::metadata(dir.join(program))
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
    }

    fn make_temp_dir(&self, base: &Path) -> Result<PathBuf, HostError> {
        let mut last = HostError::new(Failure::Other, "File exists");
        for _ in 0..100 {
            let path = base.join(temp_name());
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&path) {
                Ok(()) => return Ok(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    last = HostError::of(&error);
                }
                Err(error) => return Err(HostError::of(&error)),
            }
        }
        Err(last)
    }

    fn make_temp_file(&self, base: &Path) -> Result<PathBuf, HostError> {
        let mut last = HostError::new(Failure::Other, "File exists");
        for _ in 0..100 {
            let path = base.join(temp_name());
            match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path) {
                Ok(_) => return Ok(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    last = HostError::of(&error);
                }
                Err(error) => return Err(HostError::of(&error)),
            }
        }
        Err(last)
    }

    fn make_dirs(&self, path: &Path) -> Result<(), HostError> {
        std::fs::create_dir_all(path).map_err(|error| HostError::of(&error))
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), HostError> {
        std::fs::write(path, bytes).map_err(|error| HostError::of(&error))
    }

    fn copy_into_dir(&self, from: &Path, dir: &Path) -> Result<(), CopyError> {
        let name = from.file_name().unwrap_or_default();
        let source =
            std::fs::metadata(from).map_err(|error| CopyError::Stat(HostError::of(&error)))?;
        if source.is_dir() {
            return Err(CopyError::Open(HostError::new(Failure::IsDirectory, "Is a directory")));
        }
        let bytes = std::fs::read(from).map_err(|error| CopyError::Open(HostError::of(&error)))?;
        let target = dir.join(name);
        std::fs::write(&target, bytes).map_err(|error| CopyError::Create(HostError::of(&error)))?;
        std::fs::set_permissions(&target, source.permissions()).ok();
        Ok(())
    }

    fn remove_file(&self, path: &Path) {
        drop(std::fs::remove_file(path));
    }

    fn remove_tree(&self, path: &Path) {
        drop(std::fs::remove_dir_all(path));
    }
}

/// One node of the fake tree.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    File { bytes: Vec<u8>, executable: bool },
    Dir,
    Link(PathBuf),
}

#[derive(Debug, Default)]
struct State {
    nodes: BTreeMap<PathBuf, Node>,
    unreadable: BTreeMap<PathBuf, String>,
    temp_counter: u32,
    temp_fails: Option<HostError>,
    write_fails: BTreeMap<PathBuf, HostError>,
    log: Vec<String>,
    history: Vec<(PathBuf, Vec<u8>)>,
}

/// An in-memory tree. Paths are absolute. A file put in makes its parent directories.
#[derive(Debug, Default)]
pub struct FakeHost {
    state: Mutex<State>,
}

impl FakeHost {
    /// An empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn make_parents(state: &mut State, path: &Path) {
        let mut parent = path.parent();
        while let Some(dir) = parent {
            if dir.as_os_str().is_empty() {
                break;
            }
            state.nodes.entry(dir.to_path_buf()).or_insert(Node::Dir);
            parent = dir.parent();
        }
    }

    /// Puts a regular file with no execute bit.
    pub fn put_file(&self, path: impl Into<PathBuf>, bytes: impl Into<Vec<u8>>) {
        let path = path.into();
        let mut state = self.lock();
        Self::make_parents(&mut state, &path);
        state.nodes.insert(path, Node::File { bytes: bytes.into(), executable: false });
    }

    /// Puts a regular file with an execute bit.
    pub fn put_program(&self, path: impl Into<PathBuf>) {
        let path = path.into();
        let mut state = self.lock();
        Self::make_parents(&mut state, &path);
        state.nodes.insert(path, Node::File { bytes: Vec::new(), executable: true });
    }

    /// Puts a directory.
    pub fn put_dir(&self, path: impl Into<PathBuf>) {
        let path = path.into();
        let mut state = self.lock();
        Self::make_parents(&mut state, &path);
        state.nodes.insert(path, Node::Dir);
    }

    /// Puts a symbolic link to `target`.
    pub fn put_link(&self, path: impl Into<PathBuf>, target: impl Into<PathBuf>) {
        let path = path.into();
        let mut state = self.lock();
        Self::make_parents(&mut state, &path);
        state.nodes.insert(path, Node::Link(target.into()));
    }

    /// Makes reads of a file fail with `reason`.
    pub fn make_unreadable(&self, path: impl Into<PathBuf>, reason: &str) {
        self.lock().unreadable.insert(path.into(), reason.to_string());
    }

    /// Makes the next temporary file or directory fail.
    pub fn fail_temp(&self, error: HostError) {
        self.lock().temp_fails = Some(error);
    }

    /// Makes writes to a path fail.
    pub fn fail_writes_to(&self, path: impl Into<PathBuf>, error: HostError) {
        self.lock().write_fails.insert(path.into(), error);
    }

    /// The contents of a file, without recording anything.
    #[must_use]
    pub fn peek(&self, path: impl AsRef<Path>) -> Option<Vec<u8>> {
        match self.lock().nodes.get(path.as_ref()) {
            Some(Node::File { bytes, .. }) => Some(bytes.clone()),
            _ => None,
        }
    }

    /// Every write made through the port, in order, with its bytes: a file that was written and then removed
    /// is still here.
    #[must_use]
    pub fn writes(&self) -> Vec<(PathBuf, Vec<u8>)> {
        self.lock().history.clone()
    }

    /// Every path in the tree, sorted.
    #[must_use]
    pub fn paths(&self) -> Vec<PathBuf> {
        self.lock().nodes.keys().cloned().collect()
    }

    /// The write operations done so far, one line each, in order.
    #[must_use]
    pub fn log(&self) -> Vec<String> {
        self.lock().log.clone()
    }

    fn normalise(path: &Path) -> PathBuf {
        let mut out = PathBuf::from("/");
        for component in path.components() {
            match component {
                std::path::Component::ParentDir => {
                    out.pop();
                }
                std::path::Component::Normal(name) => out.push(name),
                _ => {}
            }
        }
        out
    }

    /// The node at `path`, following links in every component, and the path it was found at.
    fn resolve(state: &State, path: &Path, follow_last: bool) -> Option<(PathBuf, Node)> {
        let mut pending: Vec<std::ffi::OsString> = Self::normalise(path)
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(name) => Some(name.to_os_string()),
                _ => None,
            })
            .collect();
        pending.reverse();
        let mut current = PathBuf::from("/");
        let mut hops = 0;
        while let Some(name) = pending.pop() {
            let next = current.join(&name);
            match state.nodes.get(&next) {
                None => return None,
                Some(Node::Link(target)) if !pending.is_empty() || follow_last => {
                    hops += 1;
                    if hops > 40 {
                        return None;
                    }
                    let base =
                        if target.is_absolute() { PathBuf::from("/") } else { current.clone() };
                    let mut rest: Vec<std::ffi::OsString> = Self::normalise(&base.join(target))
                        .components()
                        .filter_map(|component| match component {
                            std::path::Component::Normal(part) => Some(part.to_os_string()),
                            _ => None,
                        })
                        .collect();
                    rest.reverse();
                    pending.extend(rest);
                    current = PathBuf::from("/");
                }
                Some(_) => current = next,
            }
        }
        if current == Path::new("/") {
            return Some((current, Node::Dir));
        }
        state.nodes.get(&current).map(|node| (current.clone(), node.clone()))
    }

    fn children(state: &State, dir: &Path) -> Vec<PathBuf> {
        state.nodes.keys().filter(|path| path.parent() == Some(dir)).cloned().collect()
    }
}

impl Host for FakeHost {
    fn kind(&self, path: &Path) -> Kind {
        match Self::resolve(&self.lock(), path, true) {
            Some((_, Node::Dir)) => Kind::Dir,
            Some((_, Node::File { .. })) => Kind::File,
            Some((_, Node::Link(_))) | None => Kind::Missing,
        }
    }

    fn is_symlink(&self, path: &Path) -> bool {
        matches!(Self::resolve(&self.lock(), path, false), Some((_, Node::Link(_))))
    }

    fn lexists(&self, path: &Path) -> bool {
        Self::resolve(&self.lock(), path, false).is_some()
    }

    fn readable(&self, path: &Path) -> bool {
        let state = self.lock();
        match Self::resolve(&state, path, true) {
            Some((found, _)) => !state.unreadable.contains_key(&found),
            None => false,
        }
    }

    fn read(&self, path: &Path) -> Fact<Vec<u8>> {
        let state = self.lock();
        match Self::resolve(&state, path, true) {
            None => Fact::Absent,
            Some((found, node)) => {
                if let Some(reason) = state.unreadable.get(&found) {
                    return Fact::Unreadable(reason.clone());
                }
                match node {
                    Node::File { bytes, .. } => Fact::Known(bytes),
                    _ => Fact::Unreadable("Is a directory".to_string()),
                }
            }
        }
    }

    fn read_head(&self, path: &Path, limit: usize) -> Fact<Vec<u8>> {
        self.read(path).map(|mut bytes| {
            bytes.truncate(limit);
            bytes
        })
    }

    fn list(&self, dir: &Path) -> Fact<Vec<OsString>> {
        let state = self.lock();
        match Self::resolve(&state, dir, true) {
            Some((found, Node::Dir)) => {
                if let Some(reason) = state.unreadable.get(&found) {
                    return Fact::Unreadable(reason.clone());
                }
                let mut names: Vec<OsString> = Self::children(&state, &found)
                    .iter()
                    .filter_map(|path| path.file_name().map(OsStr::to_os_string))
                    .collect();
                names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
                Fact::Known(names)
            }
            _ => Fact::Absent,
        }
    }

    fn walk(&self, start: &Path, max_depth: Option<usize>) -> Vec<Entry> {
        let state = self.lock();
        let Some((found, Node::Dir)) = Self::resolve(&state, start, false) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut stack = vec![(found, 1usize)];
        while let Some((dir, depth)) = stack.pop() {
            for child in Self::children(&state, &dir) {
                let kind = match state.nodes.get(&child) {
                    Some(Node::Dir) => EntryKind::Dir,
                    Some(Node::File { .. }) => EntryKind::File,
                    Some(Node::Link(_)) => EntryKind::Symlink,
                    None => EntryKind::Other,
                };
                out.push(Entry { path: child.clone(), kind });
                if kind == EntryKind::Dir && max_depth.is_none_or(|limit| depth < limit) {
                    stack.push((child, depth + 1));
                }
            }
        }
        out
    }

    fn real_path(&self, path: &Path) -> Option<PathBuf> {
        Self::resolve(&self.lock(), path, true).map(|(found, _)| found)
    }

    fn on_path(&self, program: &str, search: &OsStr) -> bool {
        if program.is_empty() || program.contains('/') {
            return false;
        }
        let state = self.lock();
        search.as_bytes().split(|byte| *byte == b':').any(|entry| {
            if entry.is_empty() {
                return false;
            }
            let candidate = PathBuf::from(OsStr::from_bytes(entry)).join(program);
            matches!(
                Self::resolve(&state, &candidate, true),
                Some((_, Node::File { executable: true, .. }))
            )
        })
    }

    fn make_temp_dir(&self, base: &Path) -> Result<PathBuf, HostError> {
        let mut state = self.lock();
        if let Some(error) = state.temp_fails.take() {
            return Err(error);
        }
        state.temp_counter += 1;
        let path = base.join(format!("tmp.fake{:06}", state.temp_counter));
        Self::make_parents(&mut state, &path);
        state.nodes.insert(path.clone(), Node::Dir);
        state.log.push(format!("mktemp -d {}", path.display()));
        Ok(path)
    }

    fn make_temp_file(&self, base: &Path) -> Result<PathBuf, HostError> {
        let mut state = self.lock();
        if let Some(error) = state.temp_fails.take() {
            return Err(error);
        }
        state.temp_counter += 1;
        let path = base.join(format!("tmp.fake{:06}", state.temp_counter));
        Self::make_parents(&mut state, &path);
        state.nodes.insert(path.clone(), Node::File { bytes: Vec::new(), executable: false });
        state.log.push(format!("mktemp {}", path.display()));
        Ok(path)
    }

    fn make_dirs(&self, path: &Path) -> Result<(), HostError> {
        let mut state = self.lock();
        if let Some(error) = state.write_fails.get(path) {
            return Err(error.clone());
        }
        Self::make_parents(&mut state, path);
        state.nodes.entry(path.to_path_buf()).or_insert(Node::Dir);
        state.log.push(format!("mkdir -p {}", path.display()));
        Ok(())
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), HostError> {
        let mut state = self.lock();
        if let Some(error) = state.write_fails.get(path) {
            return Err(error.clone());
        }
        Self::make_parents(&mut state, path);
        state
            .nodes
            .insert(path.to_path_buf(), Node::File { bytes: bytes.to_vec(), executable: false });
        state.log.push(format!("write {}", path.display()));
        state.history.push((path.to_path_buf(), bytes.to_vec()));
        Ok(())
    }

    fn copy_into_dir(&self, from: &Path, dir: &Path) -> Result<(), CopyError> {
        let mut state = self.lock();
        let Some((found, node)) = Self::resolve(&state, from, true) else {
            return Err(CopyError::Stat(HostError::new(
                Failure::NotFound,
                "No such file or directory",
            )));
        };
        if let Some(reason) = state.unreadable.get(&found) {
            return Err(CopyError::Open(HostError::new(Failure::Permission, reason)));
        }
        let Node::File { bytes, executable } = node else {
            return Err(CopyError::Open(HostError::new(Failure::IsDirectory, "Is a directory")));
        };
        if !matches!(Self::resolve(&state, dir, true), Some((_, Node::Dir))) {
            return Err(CopyError::Create(HostError::new(
                Failure::NotFound,
                "No such file or directory",
            )));
        }
        let name = from.file_name().unwrap_or_default();
        let target = dir.join(name);
        state.nodes.insert(target.clone(), Node::File { bytes, executable });
        state.log.push(format!("cp {} {}", from.display(), target.display()));
        Ok(())
    }

    fn remove_file(&self, path: &Path) {
        let mut state = self.lock();
        if matches!(state.nodes.get(path), Some(Node::File { .. } | Node::Link(_))) {
            state.nodes.remove(path);
            state.log.push(format!("rm -f {}", path.display()));
        }
    }

    fn remove_tree(&self, path: &Path) {
        let mut state = self.lock();
        let doomed: Vec<PathBuf> =
            state.nodes.keys().filter(|candidate| candidate.starts_with(path)).cloned().collect();
        for candidate in doomed {
            state.nodes.remove(&candidate);
        }
        state.log.push(format!("rm -rf {}", path.display()));
    }
}

/// Reads a path's bytes for a message: `OsString` to bytes.
#[must_use]
pub fn os_bytes(name: &OsString) -> Vec<u8> {
    name.clone().into_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_put_file_makes_its_parent_directories() {
        let host = FakeHost::new();
        host.put_file("/t/.github/workflows/ci.yml", "x");
        assert_eq!(host.kind(Path::new("/t/.github")), Kind::Dir);
        assert_eq!(host.kind(Path::new("/t/.github/workflows/ci.yml")), Kind::File);
        assert_eq!(host.kind(Path::new("/t/nope")), Kind::Missing);
    }

    #[test]
    fn a_link_to_a_directory_is_a_directory_and_a_link() {
        let host = FakeHost::new();
        host.put_dir("/real");
        host.put_file("/real/a", "1");
        host.put_link("/alias", "/real");
        assert_eq!(host.kind(Path::new("/alias")), Kind::Dir);
        assert!(host.is_symlink(Path::new("/alias")));
        assert_eq!(host.read(Path::new("/alias/a")), Fact::Known(b"1".to_vec()));
    }

    #[test]
    fn a_dangling_link_exists_for_a_glob_and_is_missing_for_a_test() {
        let host = FakeHost::new();
        host.put_link("/dangling", "/nowhere");
        assert!(host.lexists(Path::new("/dangling")));
        assert_eq!(host.kind(Path::new("/dangling")), Kind::Missing);
    }

    #[test]
    fn walk_does_not_enter_a_symlink_and_reports_it_as_one() {
        let host = FakeHost::new();
        host.put_file("/w/a.yml", "");
        host.put_dir("/elsewhere");
        host.put_file("/elsewhere/b.yml", "");
        host.put_link("/w/link", "/elsewhere");
        let entries = host.walk(Path::new("/w"), None);
        let kinds: Vec<_> = entries.iter().map(|entry| (entry.path.clone(), entry.kind)).collect();
        assert!(kinds.contains(&(PathBuf::from("/w/link"), EntryKind::Symlink)), "{kinds:?}");
        assert!(!kinds.iter().any(|(path, _)| path.ends_with("b.yml")), "{kinds:?}");
    }

    #[test]
    fn walk_from_a_symlink_has_no_entries_as_find_without_dash_l() {
        let host = FakeHost::new();
        host.put_file("/real/a.yml", "");
        host.put_link("/alias", "/real");
        assert!(host.walk(Path::new("/alias"), None).is_empty());
    }

    #[test]
    fn walk_stops_at_max_depth() {
        let host = FakeHost::new();
        host.put_file("/w/a.yml", "");
        host.put_file("/w/sub/b.yml", "");
        let shallow = host.walk(Path::new("/w"), Some(1));
        assert!(shallow.iter().any(|entry| entry.path.ends_with("a.yml")));
        assert!(!shallow.iter().any(|entry| entry.path.ends_with("b.yml")));
        let deep = host.walk(Path::new("/w"), None);
        assert!(deep.iter().any(|entry| entry.path.ends_with("b.yml")));
    }

    #[test]
    fn on_path_needs_an_executable_regular_file() {
        let host = FakeHost::new();
        host.put_program("/bin/shellcheck");
        host.put_file("/bin/nice", "");
        assert!(host.on_path("shellcheck", OsStr::new("/usr/bin:/bin")));
        assert!(!host.on_path("nice", OsStr::new("/bin")));
        assert!(!host.on_path("shellcheck", OsStr::new("/usr/bin")));
        assert!(!host.on_path("a/b", OsStr::new("/bin")));
    }

    #[test]
    fn list_is_sorted_by_bytes_and_absent_for_a_file() {
        let host = FakeHost::new();
        host.put_file("/d/b", "");
        host.put_file("/d/B", "");
        host.put_file("/d/a", "");
        assert_eq!(
            host.list(Path::new("/d")),
            Fact::Known(vec![OsString::from("B"), OsString::from("a"), OsString::from("b")])
        );
        assert_eq!(host.list(Path::new("/d/a")), Fact::Absent);
    }

    #[test]
    fn temp_names_are_deterministic_in_the_fake_and_logged() {
        let host = FakeHost::new();
        let dir = host.make_temp_dir(Path::new("/tmp")).unwrap();
        let file = host.make_temp_file(Path::new("/tmp")).unwrap();
        assert_eq!(dir, Path::new("/tmp/tmp.fake000001"));
        assert_eq!(file, Path::new("/tmp/tmp.fake000002"));
        assert_eq!(host.log(), vec!["mktemp -d /tmp/tmp.fake000001", "mktemp /tmp/tmp.fake000002"]);
    }

    #[test]
    fn copy_into_dir_names_the_failing_step_the_way_cp_does() {
        let host = FakeHost::new();
        host.put_dir("/dst");
        let error = host.copy_into_dir(Path::new("/src/x"), Path::new("/dst")).unwrap_err();
        assert_eq!(
            error.message(Path::new("/src/x"), Path::new("/dst")),
            b"cp: cannot stat '/src/x': No such file or directory"
        );
        host.put_file("/src/x", "1");
        host.copy_into_dir(Path::new("/src/x"), Path::new("/dst")).unwrap();
        assert_eq!(host.peek("/dst/x"), Some(b"1".to_vec()));
    }

    #[test]
    fn remove_tree_removes_everything_below() {
        let host = FakeHost::new();
        host.put_file("/t/a/b", "");
        host.put_file("/t2/c", "");
        host.remove_tree(Path::new("/t"));
        assert_eq!(host.kind(Path::new("/t/a/b")), Kind::Missing);
        assert_eq!(host.kind(Path::new("/t2/c")), Kind::File);
    }
}
