//! The machine the installer works on: questions it asks and changes it makes.
//!
//! [`Machine`] is the file system as `install` and `new-profile` use it. [`RealMachine`]
//! answers from `std::fs` and does the work of `cp`, `rm`, `chmod` and the rest in this
//! process ([`crate::coreutils`]). [`FakeMachine`] answers from tables and records every
//! change, so a test can show that a dry run changed nothing and where a failure lands.
//!
//! The `mx_ports::Fs` trait has too few verbs for this and no way to tell a missing
//! directory from an empty one, so (as `mx-doctor` does) the installer has its own.

use crate::coreutils::{self, Outcome};
use crate::plan::Cmd;
use crate::util::sort_c;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The file system, as the installer asks and changes it.
pub trait Machine {
    /// `[ -e path ]`: follows links.
    fn exists(&self, path: &Path) -> bool;
    /// Whether the path is there at all, a dangling link included.
    fn lexists(&self, path: &Path) -> bool;
    /// `[ -f path ]`.
    fn is_file(&self, path: &Path) -> bool;
    /// `[ -d path ]`.
    fn is_dir(&self, path: &Path) -> bool;
    /// `[ -x path ]`.
    fn is_executable(&self, path: &Path) -> bool;
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    /// The names in a directory, in no order.
    fn dir_names(&self, dir: &Path) -> io::Result<Vec<OsString>>;
    /// `find dir -type f`: every regular file below, links not followed.
    fn files_below(&self, dir: &Path) -> io::Result<Vec<PathBuf>>;
    /// One of the file commands of the bash.
    fn exec(&self, cmd: &Cmd) -> Outcome;
    /// `> path`: create the file, or empty the one there and keep its mode.
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// `>> path`.
    fn append(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// `sed -i`: the file's new text, in a new file that takes the old one's mode.
    fn rewrite(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// `chmod MODE path`.
    fn chmod(&self, path: &Path, mode: u32) -> io::Result<()>;
    /// 32 bytes the system chooses, as 64 lower-case hex characters.
    fn random_hex(&self) -> io::Result<String>;
    /// The sha256 of a file, as 64 lower-case hex characters.
    fn sha256(&self, path: &Path) -> io::Result<String>;
    /// A file that can be created exclusively, for a key: mode 600.
    fn write_private(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
}

/// The paths in `dir` whose names `accept` takes, as a shell glob lists them: names that
/// start with a dot are skipped, and the order is by the bytes of the whole path.
pub fn glob(machine: &dyn Machine, dir: &Path, accept: &dyn Fn(&[u8]) -> bool) -> Vec<PathBuf> {
    let Ok(names) = machine.dir_names(dir) else { return Vec::new() };
    let mut found: Vec<PathBuf> = names
        .into_iter()
        .filter(|name| !name.as_bytes().starts_with(b".") && accept(name.as_bytes()))
        .map(|name| dir.join(name))
        .collect();
    sort_c(&mut found);
    found
}

/// `dir/PREFIX*SUFFIX`.
pub fn glob_affixes(machine: &dyn Machine, dir: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    glob(machine, dir, &|name| {
        name.len() >= prefix.len() + suffix.len()
            && name.starts_with(prefix.as_bytes())
            && name.ends_with(suffix.as_bytes())
    })
}

/// `base/*/leaf`, as the shell expands it: the directories of `base` that hold `leaf`,
/// ordered by the bytes of the whole path (so `a-b` comes before `a`).
pub fn glob_children(machine: &dyn Machine, base: &Path, leaf: &str) -> Vec<PathBuf> {
    let Ok(names) = machine.dir_names(base) else { return Vec::new() };
    let mut found: Vec<PathBuf> = names
        .into_iter()
        .filter(|name| !name.as_bytes().starts_with(b"."))
        .map(|name| base.join(name).join(leaf))
        .filter(|path| machine.lexists(path))
        .collect();
    sort_c(&mut found);
    found
}

/// The real machine.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealMachine;

impl Machine for RealMachine {
    fn exists(&self, path: &Path) -> bool {
        fs::metadata(path).is_ok()
    }

    fn lexists(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok()
    }

    fn is_file(&self, path: &Path) -> bool {
        fs::metadata(path).is_ok_and(|meta| meta.is_file())
    }

    fn is_dir(&self, path: &Path) -> bool {
        fs::metadata(path).is_ok_and(|meta| meta.is_dir())
    }

    fn is_executable(&self, path: &Path) -> bool {
        // The kernel decides for the effective user; the mode bits are what std shows.
        fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn dir_names(&self, dir: &Path) -> io::Result<Vec<OsString>> {
        Ok(fs::read_dir(dir)?.filter_map(Result::ok).map(|entry| entry.file_name()).collect())
    }

    fn files_below(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
            let mut names: Vec<OsString> =
                fs::read_dir(dir)?.filter_map(Result::ok).map(|e| e.file_name()).collect();
            names.sort();
            for name in names {
                let path = dir.join(name);
                let kind = fs::symlink_metadata(&path)?.file_type();
                if kind.is_dir() {
                    walk(&path, out)?;
                } else if kind.is_file() {
                    out.push(path);
                }
            }
            Ok(())
        }
        let mut out = Vec::new();
        walk(dir, &mut out)?;
        Ok(out)
    }

    fn exec(&self, cmd: &Cmd) -> Outcome {
        match cmd {
            Cmd::MkdirP(paths) => coreutils::mkdir_p(paths),
            Cmd::Cp { src, dst } => coreutils::cp(src, dst),
            Cmd::CpR { src, dst } => coreutils::cp_r(src, dst),
            Cmd::RmRf(paths) => coreutils::rm_rf(paths),
            Cmd::RmF(path) => coreutils::rm_f(path),
            Cmd::ChmodX(paths) => coreutils::chmod_x(paths),
            Cmd::Touch(path) => coreutils::touch(path),
            Cmd::Mv { src, dst } => coreutils::mv(src, dst),
            Cmd::InstallBinary { src, dst } => coreutils::install_binary(src, dst),
            Cmd::Systemctl(_) | Cmd::Launchctl(_) | Cmd::CronRemove { .. } => {
                unreachable!("a scheduler command goes through Proc, never through the machine")
            }
        }
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut file =
            OpenOptions::new().write(true).create(true).truncate(true).mode(0o666).open(path)?;
        file.write_all(bytes)
    }

    fn append(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut file = OpenOptions::new().append(true).create(true).mode(0o666).open(path)?;
        file.write_all(bytes)
    }

    fn rewrite(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mode = fs::metadata(path)?.permissions().mode() & 0o7777;
        let mut staged = path.as_os_str().to_os_string();
        staged.push(".mx-new");
        let staged = PathBuf::from(staged);
        drop(fs::remove_file(&staged));
        let result = (|| {
            let mut file =
                OpenOptions::new().write(true).create_new(true).mode(0o600).open(&staged)?;
            file.write_all(bytes)?;
            fs::set_permissions(&staged, fs::Permissions::from_mode(mode))?;
            fs::rename(&staged, path)
        })();
        if result.is_err() {
            drop(fs::remove_file(&staged));
        }
        result
    }

    fn chmod(&self, path: &Path, mode: u32) -> io::Result<()> {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }

    fn random_hex(&self) -> io::Result<String> {
        let mut bytes = [0_u8; 32];
        File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(crate::util::hex(&bytes))
    }

    fn sha256(&self, path: &Path) -> io::Result<String> {
        let mut file = File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 65536];
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        Ok(crate::util::hex(&hasher.finalize()))
    }

    fn write_private(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        // The file exists, empty, before its text does, as the bash's redirection makes it.
        let mut file =
            OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        file.write_all(bytes)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    }
}

/// One change a test can read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Exec(String),
    Write(PathBuf, Vec<u8>),
    Append(PathBuf, Vec<u8>),
    Rewrite(PathBuf, Vec<u8>),
    Chmod(PathBuf, u32),
    WritePrivate(PathBuf, Vec<u8>),
}

#[derive(Default)]
struct Tables {
    files: BTreeMap<PathBuf, Vec<u8>>,
    dirs: BTreeSet<PathBuf>,
    executable: BTreeSet<PathBuf>,
    statuses: BTreeMap<String, Outcome>,
    write_failures: BTreeSet<PathBuf>,
    changes: Vec<Change>,
    hex: String,
}

/// A machine that answers from tables and records every change.
///
/// With [`FakeMachine::forbid_changes`] any change panics, which is how a test shows that
/// a dry run touched nothing.
#[derive(Default)]
pub struct FakeMachine {
    tables: Mutex<Tables>,
    forbid: bool,
}

impl FakeMachine {
    #[must_use]
    pub fn new() -> Self {
        let machine = Self::default();
        machine.lock().hex = "ab".repeat(32);
        machine
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Tables> {
        self.tables.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Every change from now on panics.
    #[must_use]
    pub fn forbid_changes(mut self) -> Self {
        self.forbid = true;
        self
    }

    pub fn put(&self, path: impl Into<PathBuf>, bytes: impl Into<Vec<u8>>) {
        let path = path.into();
        let mut tables = self.lock();
        let mut parent = path.parent();
        while let Some(dir) = parent {
            tables.dirs.insert(dir.to_path_buf());
            parent = dir.parent();
        }
        tables.files.insert(path, bytes.into());
    }

    pub fn put_dir(&self, path: impl Into<PathBuf>) {
        let path = path.into();
        let mut tables = self.lock();
        let mut at = Some(path.as_path());
        while let Some(dir) = at {
            tables.dirs.insert(dir.to_path_buf());
            at = dir.parent();
        }
    }

    pub fn put_executable(&self, path: impl Into<PathBuf>, bytes: impl Into<Vec<u8>>) {
        let path = path.into();
        self.put(path.clone(), bytes);
        self.lock().executable.insert(path);
    }

    /// The command whose text is `text` ends with this outcome.
    pub fn script(&self, text: &str, status: i32, diag: &str) {
        self.lock()
            .statuses
            .insert(text.to_string(), Outcome { status, diag: diag.as_bytes().to_vec() });
    }

    /// Writes to `path` fail (the file is created empty first, as a redirection does).
    pub fn fail_writes_to(&self, path: impl Into<PathBuf>) {
        self.lock().write_failures.insert(path.into());
    }

    /// The text `random_hex` returns.
    pub fn set_hex(&self, hex: &str) {
        self.lock().hex = hex.to_string();
    }

    #[must_use]
    pub fn changes(&self) -> Vec<Change> {
        self.lock().changes.clone()
    }

    #[must_use]
    pub fn peek(&self, path: &Path) -> Option<Vec<u8>> {
        self.lock().files.get(path).cloned()
    }

    fn record(&self, change: Change) {
        assert!(!self.forbid, "a change on a machine that forbids them: {change:?}");
        self.lock().changes.push(change);
    }
}

impl Machine for FakeMachine {
    fn exists(&self, path: &Path) -> bool {
        let tables = self.lock();
        tables.files.contains_key(path) || tables.dirs.contains(path)
    }

    fn lexists(&self, path: &Path) -> bool {
        self.exists(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.lock().files.contains_key(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.lock().dirs.contains(path)
    }

    fn is_executable(&self, path: &Path) -> bool {
        self.lock().executable.contains(path)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.lock().files.get(path).cloned().ok_or_else(|| io::Error::from_raw_os_error(2))
    }

    fn dir_names(&self, dir: &Path) -> io::Result<Vec<OsString>> {
        let tables = self.lock();
        if !tables.dirs.contains(dir) {
            return Err(io::Error::from_raw_os_error(2));
        }
        let mut names: BTreeSet<OsString> = BTreeSet::new();
        for path in tables.files.keys().chain(tables.dirs.iter()) {
            if path.parent() == Some(dir) {
                names.insert(path.file_name().unwrap_or_else(|| OsStr::new("")).to_os_string());
            }
        }
        Ok(names.into_iter().collect())
    }

    fn files_below(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
        let tables = self.lock();
        Ok(tables.files.keys().filter(|path| path.starts_with(dir)).cloned().collect())
    }

    fn exec(&self, cmd: &Cmd) -> Outcome {
        let text = String::from_utf8_lossy(&cmd.text()).into_owned();
        self.record(Change::Exec(text.clone()));
        self.lock().statuses.get(&text).cloned().unwrap_or_default()
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.record(Change::Write(path.to_path_buf(), bytes.to_vec()));
        let failing = self.lock().write_failures.contains(path);
        self.put(path, if failing { Vec::new() } else { bytes.to_vec() });
        if failing { Err(io::Error::from_raw_os_error(13)) } else { Ok(()) }
    }

    fn append(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.record(Change::Append(path.to_path_buf(), bytes.to_vec()));
        let mut text = self.peek(path).unwrap_or_default();
        text.extend_from_slice(bytes);
        self.put(path, text);
        Ok(())
    }

    fn rewrite(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.record(Change::Rewrite(path.to_path_buf(), bytes.to_vec()));
        self.put(path, bytes);
        Ok(())
    }

    fn chmod(&self, path: &Path, mode: u32) -> io::Result<()> {
        self.record(Change::Chmod(path.to_path_buf(), mode));
        Ok(())
    }

    fn random_hex(&self) -> io::Result<String> {
        Ok(self.lock().hex.clone())
    }

    fn sha256(&self, path: &Path) -> io::Result<String> {
        let bytes = self.read(path)?;
        Ok(crate::util::hex(&Sha256::digest(&bytes)))
    }

    fn write_private(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.record(Change::WritePrivate(path.to_path_buf(), bytes.to_vec()));
        let failing = self.lock().write_failures.contains(path);
        self.put(path, if failing { Vec::new() } else { bytes.to_vec() });
        if failing { Err(io::Error::from_raw_os_error(13)) } else { Ok(()) }
    }
}
