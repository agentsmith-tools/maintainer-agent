//! The host's file system, as the doctor reads it.
//!
//! `mx_ports::Fs` cannot tell a missing directory from an empty one and has no
//! real adapter yet, so the doctor asks this trait instead. [`RealHost`] is the
//! adapter for the process; [`FakeHost`] is an in-memory tree for tests, so no
//! test reads the machine it runs on. Every method that follows a path follows
//! symbolic links, as `[ -d ]`, `[ -f ]`, `[ -x ]` and `stat` do, unless its
//! documentation says otherwise.

use mx_core::Fact;
use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::os::unix::fs::{MetadataExt, PermissionsExt};

/// An I/O error as the C library words it: "Permission denied", with Rust's
/// " (os error 13)" left off.
fn reason(error: &std::io::Error) -> String {
    let text = error.to_string();
    match text.rfind(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

/// The path a string made by `text::lossless` names.
fn os(path: &str) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(&crate::text::to_bytes(path)))
}
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// What a path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Missing,
    Dir,
    File,
    Other,
}

/// What `[ -e ]`, `[ -d ]`, `[ -f ]`, `[ -x ]`, `[ -w ]` and `stat -c %Y` ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    pub kind: Kind,
    pub executable: bool,
    pub writable: bool,
    /// Modification time: whole seconds and the nanoseconds after them.
    pub mtime: (i64, u32),
}

impl Stat {
    #[must_use]
    pub fn missing() -> Self {
        Self { kind: Kind::Missing, executable: false, writable: false, mtime: (0, 0) }
    }

    /// `[ -e ]`.
    #[must_use]
    pub fn exists(&self) -> bool {
        self.kind != Kind::Missing
    }
    /// `[ -d ]`.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }
    /// `[ -f ]`.
    #[must_use]
    pub fn is_file(&self) -> bool {
        self.kind == Kind::File
    }
}

/// Reads of the host file system, and the few writes the sandbox probe makes.
pub trait HostFs {
    /// `stat` after following links. A path that cannot be examined is `Missing`.
    fn stat(&self, path: &str) -> Stat;

    /// The bytes of a file.
    fn read(&self, path: &str) -> Fact<Vec<u8>>;

    /// The names in a directory, sorted by bytes, dot files included. `Absent`
    /// when the path is not a directory.
    fn list(&self, path: &str) -> Fact<Vec<String>>;

    /// `find ROOT -type d \( -name A -o -name B \) | wc -l`: directories at any
    /// depth whose own name is one of `names`, symbolic links not followed.
    fn count_dirs_named(&self, root: &str, names: &[&str]) -> usize;

    /// `grep -rlaF` over `roots`: the files whose bytes hold any of `patterns`,
    /// in the order grep lists them. A root that is a link is followed, a link
    /// met inside a tree is not, and only regular files are read. An empty
    /// pattern matches every non-empty file, as an empty `-e` matches every line.
    /// The patterns never leave the process.
    fn scan_for(&self, roots: &[String], patterns: &[Vec<u8>]) -> Vec<String>;

    /// `mktemp -d` under `base`: a new directory named `tmp.` and ten letters or digits.
    fn make_temp_dir(&self, base: &str) -> Result<String, String>;

    /// `mkdir -p`.
    fn make_dirs(&self, path: &str) -> Result<(), String>;

    /// Writes a file, creating or replacing it.
    fn write(&self, path: &str, contents: &[u8]) -> Result<(), String>;

    /// `rm -rf`.
    fn remove_tree(&self, path: &str);
}

/// `ls DIR | wc -l`: the entries that do not start with a dot, and 1 for a file
/// (`ls` prints the name it was given). A missing or unreadable path counts 0.
#[must_use]
pub fn ls_count(host: &dyn HostFs, path: &str) -> usize {
    match host.list(path) {
        Fact::Known(names) => names.iter().filter(|name| !name.starts_with('.')).count(),
        Fact::Absent => usize::from(host.stat(path).exists()),
        Fact::Unreadable(_) => 0,
    }
}

/// The names in a directory that start with `prefix` and end with `suffix`, as
/// the glob `DIR/PREFIX*SUFFIX` lists them when its first character is a letter:
/// no dot files, sorted by bytes, and each name must have something between.
/// A name matches when it is at least as long as prefix and suffix together.
#[must_use]
pub fn glob_names(host: &dyn HostFs, dir: &str, prefix: &str, suffix: &str) -> Vec<String> {
    let Fact::Known(names) = host.list(dir) else { return Vec::new() };
    let mut found: Vec<String> = names
        .into_iter()
        .filter(|name| {
            name.len() >= prefix.len() + suffix.len()
                && name.starts_with(prefix)
                && name.ends_with(suffix)
                && (!prefix.is_empty() || !name.starts_with('.'))
        })
        .collect();
    found.sort();
    found
}

// --- the real host --------------------------------------------------------------

/// The identity `[ -x ]` and `[ -w ]` judge access by.
#[derive(Debug, Clone)]
struct Ids {
    uid: u32,
    gids: Vec<u32>,
}

/// The file system of this machine.
#[derive(Debug)]
pub struct RealHost {
    ids: Option<Ids>,
}

impl Default for RealHost {
    fn default() -> Self {
        Self::new()
    }
}

impl RealHost {
    /// Reads the effective user and groups from `/proc/self/status`. Where that
    /// file is absent (macOS) `-x` means "any execute bit" and `-w` means "any
    /// write bit", which is right for the owner of the file.
    #[must_use]
    pub fn new() -> Self {
        Self { ids: read_ids() }
    }

    fn allowed(&self, meta: &std::fs::Metadata, bit: u32) -> bool {
        let mode = meta.permissions().mode();
        let Some(ids) = &self.ids else { return mode & (bit * 0o111) != 0 };
        if ids.uid == 0 {
            // root passes `-w` always and `-x` when any execute bit is set or it is a directory.
            return bit == 2 || meta.is_dir() || mode & 0o111 != 0;
        }
        let class = if meta.uid() == ids.uid {
            (mode >> 6) & 7
        } else if ids.gids.contains(&meta.gid()) {
            (mode >> 3) & 7
        } else {
            mode & 7
        };
        class & bit != 0
    }
}

fn read_ids() -> Option<Ids> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = |name: &str| text.lines().find_map(|line| line.strip_prefix(name));
    let uid = field("Uid:")?.split_whitespace().nth(1)?.parse().ok()?;
    let gid: u32 = field("Gid:")?.split_whitespace().nth(1)?.parse().ok()?;
    let mut gids: Vec<u32> = field("Groups:")
        .map(|groups| groups.split_whitespace().filter_map(|g| g.parse().ok()).collect())
        .unwrap_or_default();
    gids.push(gid);
    Some(Ids { uid, gids })
}

impl HostFs for RealHost {
    fn stat(&self, path: &str) -> Stat {
        let Ok(meta) = std::fs::metadata(os(path)) else { return Stat::missing() };
        let kind = if meta.is_dir() {
            Kind::Dir
        } else if meta.is_file() {
            Kind::File
        } else {
            Kind::Other
        };
        Stat {
            kind,
            executable: self.allowed(&meta, 1),
            writable: self.allowed(&meta, 2),
            mtime: (meta.mtime(), u32::try_from(meta.mtime_nsec()).unwrap_or(0)),
        }
    }

    fn read(&self, path: &str) -> Fact<Vec<u8>> {
        match std::fs::read(os(path)) {
            Ok(bytes) => Fact::Known(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Fact::Absent,
            Err(error) => Fact::Unreadable(error.to_string()),
        }
    }

    fn list(&self, path: &str) -> Fact<Vec<String>> {
        match std::fs::metadata(os(path)) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Fact::Absent,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Fact::Absent,
            Err(error) => return Fact::Unreadable(error.to_string()),
        }
        match std::fs::read_dir(os(path)) {
            Ok(entries) => {
                let mut names: Vec<String> = entries
                    .filter_map(Result::ok)
                    .map(|entry| crate::text::lossless(entry.file_name().as_bytes()))
                    .collect();
                names.sort();
                Fact::Known(names)
            }
            Err(error) => Fact::Unreadable(error.to_string()),
        }
    }

    fn count_dirs_named(&self, root: &str, names: &[&str]) -> usize {
        fn walk(dir: &Path, names: &[&str], count: &mut usize) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for entry in entries.filter_map(Result::ok) {
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_dir() {
                    if names.iter().any(|name| entry.file_name() == *name) {
                        *count += 1;
                    }
                    walk(&entry.path(), names, count);
                }
            }
        }
        let mut count = 0;
        let root_path = os(root);
        let root_path = root_path.as_path();
        // `find` starts at the root itself, which is a directory when it is one
        // (a link given on the command line is not followed by default).
        if std::fs::symlink_metadata(root_path).is_ok_and(|meta| meta.is_dir()) {
            if root_path.file_name().is_some_and(|base| names.iter().any(|name| base == *name)) {
                count += 1;
            }
            walk(root_path, names, &mut count);
        }
        count
    }

    fn scan_for(&self, roots: &[String], patterns: &[Vec<u8>]) -> Vec<String> {
        let mut found = Vec::new();
        for root in roots {
            let Ok(meta) = std::fs::metadata(os(root)) else { continue };
            if meta.is_dir() {
                scan_dir(root, patterns, &mut found);
            } else if meta.is_file() && file_matches(&os(root), patterns) {
                found.push(root.clone());
            }
        }
        found
    }

    fn make_temp_dir(&self, base: &str) -> Result<String, String> {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let mut last = String::new();
        for _ in 0..100 {
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_usize(0);
            let mut seed = hasher.finish();
            let mut suffix = String::new();
            for _ in 0..10 {
                suffix.push(char::from(ALPHABET[usize::try_from(seed % 62).unwrap_or(0)]));
                seed /= 62;
                if seed == 0 {
                    seed = RandomState::new().build_hasher().finish() | 1;
                }
            }
            let path = format!("{}/tmp.{suffix}", base.trim_end_matches('/'));
            let mut builder = std::fs::DirBuilder::new();
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            match builder.create(os(&path)) {
                Ok(()) => return Ok(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    last = reason(&error);
                }
                Err(error) => return Err(reason(&error)),
            }
        }
        Err(last)
    }

    fn make_dirs(&self, path: &str) -> Result<(), String> {
        std::fs::create_dir_all(os(path)).map_err(|error| reason(&error))
    }

    fn write(&self, path: &str, contents: &[u8]) -> Result<(), String> {
        std::fs::write(os(path), contents).map_err(|error| error.to_string())
    }

    fn remove_tree(&self, path: &str) {
        drop(std::fs::remove_dir_all(os(path)));
    }
}

fn scan_dir(dir: &str, patterns: &[Vec<u8>], found: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(os(dir)) else { return };
    for entry in entries.filter_map(Result::ok) {
        let name = crate::text::lossless(entry.file_name().as_bytes());
        let shown =
            if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") };
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            scan_dir(&shown, patterns, found);
        } else if kind.is_file() && file_matches(&entry.path(), patterns) {
            found.push(shown);
        }
    }
}

/// How much of a file the scan holds at once. A state directory can hold a
/// log of many gigabytes, and the Claude transcripts of a repository grow without
/// bound, so a file is read in pieces, as `grep` does.
pub const SCAN_CHUNK: usize = 64 * 1024;

fn file_matches(path: &Path, patterns: &[Vec<u8>]) -> bool {
    let Ok(file) = std::fs::File::open(path) else { return false };
    stream_contains(file, patterns, SCAN_CHUNK).unwrap_or(false)
}

/// True when the bytes of `reader` hold any pattern, reading `chunk` bytes at a
/// time. The last `longest pattern - 1` bytes are kept between reads, so a
/// pattern that straddles two reads is found. An empty pattern matches any
/// non-empty input. A read error ends the search with that error (a match found
/// before it is still a match).
pub fn stream_contains(
    mut reader: impl std::io::Read,
    patterns: &[Vec<u8>],
    chunk: usize,
) -> std::io::Result<bool> {
    let keep = patterns.iter().map(Vec::len).max().unwrap_or(0).saturating_sub(1);
    let mut window: Vec<u8> = Vec::new();
    let mut buffer = vec![0u8; chunk.max(1)];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => return Ok(false),
            Ok(n) => n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        window.extend_from_slice(&buffer[..read]);
        if contains_any(&window, patterns) {
            return Ok(true);
        }
        if window.len() > keep {
            window.drain(..window.len() - keep);
        }
    }
}

/// True when `bytes` holds any pattern. An empty pattern matches any non-empty text.
#[must_use]
pub fn contains_any(bytes: &[u8], patterns: &[Vec<u8>]) -> bool {
    patterns.iter().any(|pattern| {
        if pattern.is_empty() {
            !bytes.is_empty()
        } else {
            bytes.windows(pattern.len()).any(|window| window == pattern.as_slice())
        }
    })
}

// --- an in-memory host for tests -------------------------------------------------

/// One entry of a [`FakeHost`].
#[derive(Debug, Clone)]
enum Node {
    Dir,
    File { data: Vec<u8>, executable: bool, writable: bool, mtime: (i64, u32) },
}

/// An in-memory tree. Paths are absolute strings; parents are created by the
/// builder methods. It never touches the machine it runs on.
#[derive(Debug, Default)]
pub struct FakeHost {
    nodes: Mutex<BTreeMap<String, Node>>,
    unreadable: Mutex<BTreeMap<String, String>>,
    temp_counter: Mutex<u32>,
    removed: Mutex<Vec<String>>,
    read_only: Mutex<Vec<String>>,
    temp_dir_fails: Mutex<bool>,
    make_dirs_fails: Mutex<Option<String>>,
}

impl FakeHost {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Creates a directory and its parents.
    pub fn dir(&self, path: &str) {
        let mut nodes = Self::lock(&self.nodes);
        let mut at = String::new();
        for part in path.split('/').filter(|part| !part.is_empty()) {
            at.push('/');
            at.push_str(part);
            nodes.entry(at.clone()).or_insert(Node::Dir);
        }
    }

    /// Creates a file with a modification time, making its parents.
    pub fn file(&self, path: &str, data: &[u8], executable: bool, mtime: i64) {
        if let Some(at) = path.rfind('/') {
            self.dir(&path[..at]);
        }
        Self::lock(&self.nodes).insert(
            path.to_string(),
            Node::File { data: data.to_vec(), executable, writable: true, mtime: (mtime, 0) },
        );
    }

    /// Makes reads of `path` fail with `reason`.
    pub fn make_unreadable(&self, path: &str, reason: &str) {
        Self::lock(&self.unreadable).insert(path.to_string(), reason.to_string());
    }

    /// Makes `make_temp_dir` fail, as `mktemp -d` does on a full or missing temporary directory.
    pub fn fail_temp_dir(&self) {
        *Self::lock(&self.temp_dir_fails) = true;
    }

    /// Makes `make_dirs` fail with `reason`, as `mkdir -p` does where it may not write.
    pub fn fail_make_dirs(&self, reason: &str) {
        *Self::lock(&self.make_dirs_fails) = Some(reason.to_string());
    }

    /// Takes the write permission away from a path.
    pub fn make_read_only(&self, path: &str) {
        Self::lock(&self.read_only).push(path.trim_end_matches('/').to_string());
    }

    /// The paths `remove_tree` was called with.
    #[must_use]
    pub fn removed(&self) -> Vec<String> {
        Self::lock(&self.removed).clone()
    }

    /// The bytes of a file, without recording anything.
    #[must_use]
    pub fn peek(&self, path: &str) -> Option<Vec<u8>> {
        match Self::lock(&self.nodes).get(path) {
            Some(Node::File { data, .. }) => Some(data.clone()),
            _ => None,
        }
    }

    fn children(&self, dir: &str) -> Vec<(String, Node)> {
        let prefix = format!("{}/", dir.trim_end_matches('/'));
        Self::lock(&self.nodes)
            .iter()
            .filter_map(|(path, node)| {
                let rest = path.strip_prefix(&prefix)?;
                (!rest.contains('/')).then(|| (rest.to_string(), node.clone()))
            })
            .collect()
    }
}

impl HostFs for FakeHost {
    fn stat(&self, path: &str) -> Stat {
        let key = path.trim_end_matches('/');
        let read_only = Self::lock(&self.read_only).iter().any(|p| p == key);
        match Self::lock(&self.nodes).get(key) {
            None => Stat::missing(),
            Some(Node::Dir) => {
                Stat { kind: Kind::Dir, executable: true, writable: !read_only, mtime: (0, 0) }
            }
            Some(Node::File { executable, writable, mtime, .. }) => Stat {
                kind: Kind::File,
                executable: *executable,
                writable: *writable && !read_only,
                mtime: *mtime,
            },
        }
    }

    fn read(&self, path: &str) -> Fact<Vec<u8>> {
        if let Some(reason) = Self::lock(&self.unreadable).get(path) {
            return Fact::Unreadable(reason.clone());
        }
        match Self::lock(&self.nodes).get(path) {
            Some(Node::File { data, .. }) => Fact::Known(data.clone()),
            Some(Node::Dir) => Fact::Unreadable("is a directory".to_string()),
            None => Fact::Absent,
        }
    }

    fn list(&self, path: &str) -> Fact<Vec<String>> {
        if let Some(reason) = Self::lock(&self.unreadable).get(path) {
            return Fact::Unreadable(reason.clone());
        }
        if !matches!(Self::lock(&self.nodes).get(path.trim_end_matches('/')), Some(Node::Dir)) {
            return Fact::Absent;
        }
        let mut names: Vec<String> =
            self.children(path).into_iter().map(|(name, _)| name).collect();
        names.sort();
        Fact::Known(names)
    }

    fn count_dirs_named(&self, root: &str, names: &[&str]) -> usize {
        let prefix = format!("{}/", root.trim_end_matches('/'));
        let nodes = Self::lock(&self.nodes);
        let mut count = 0;
        if matches!(nodes.get(root.trim_end_matches('/')), Some(Node::Dir)) {
            let base = root.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            count += usize::from(names.contains(&base));
        }
        for (path, node) in nodes.iter() {
            if let (Some(rest), Node::Dir) = (path.strip_prefix(&prefix), node) {
                let base = rest.rsplit('/').next().unwrap_or("");
                count += usize::from(names.contains(&base));
            }
        }
        count
    }

    fn scan_for(&self, roots: &[String], patterns: &[Vec<u8>]) -> Vec<String> {
        let nodes = Self::lock(&self.nodes);
        let mut found = Vec::new();
        for root in roots {
            let key = root.trim_end_matches('/');
            match nodes.get(key) {
                Some(Node::File { data, .. }) if contains_any(data, patterns) => {
                    found.push(root.clone());
                }
                Some(Node::Dir) => {
                    let prefix = format!("{key}/");
                    for (path, node) in nodes.iter() {
                        if let (true, Node::File { data, .. }) = (path.starts_with(&prefix), node)
                            && contains_any(data, patterns)
                        {
                            found.push(path.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        found
    }

    fn make_temp_dir(&self, base: &str) -> Result<String, String> {
        if *Self::lock(&self.temp_dir_fails) {
            return Err("cannot create temp dir".to_string());
        }
        let n = {
            let mut counter = Self::lock(&self.temp_counter);
            *counter += 1;
            *counter
        };
        let path = format!("{}/tmp.fake{n:06}", base.trim_end_matches('/'));
        self.dir(&path);
        Ok(path)
    }

    fn make_dirs(&self, path: &str) -> Result<(), String> {
        if let Some(reason) = Self::lock(&self.make_dirs_fails).clone() {
            return Err(reason);
        }
        self.dir(path);
        Ok(())
    }

    fn write(&self, path: &str, contents: &[u8]) -> Result<(), String> {
        self.file(path, contents, false, 0);
        Ok(())
    }

    fn remove_tree(&self, path: &str) {
        let prefix = format!("{}/", path.trim_end_matches('/'));
        Self::lock(&self.nodes).retain(|key, _| key != path && !key.starts_with(&prefix));
        Self::lock(&self.removed).push(path.to_string());
    }
}
