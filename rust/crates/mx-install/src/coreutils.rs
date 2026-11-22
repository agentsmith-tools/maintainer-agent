//! `mkdir -p`, `cp`, `cp -r`, `rm -rf`, `rm -f`, `chmod +x`, `touch` and `mv` as the shell
//! runs them, done in this process.
//!
//! The bash starts these tools; the port does the same work with `std::fs` and reports a
//! failure the way the tool does: one line on standard error and status 1. Quoting follows
//! GNU coreutils in the C locale (a plain apostrophe on each side). What the tools decide
//! is reproduced:
//!
//! - `cp` writes through a symbolic link at the destination, keeps the mode of a file it
//!   replaces, gives a new file the mode of its source under the caller's umask, and puts
//!   the file inside a destination that is a directory.
//! - `cp -r` copies a link as a link, and continues after an error.
//! - `rm -rf` never follows a link, and says nothing about a path that is not there.
//! - `chmod +x` adds the execute bits the umask allows.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What a command did: its status and the lines it printed on standard error.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outcome {
    pub status: i32,
    pub diag: Vec<u8>,
}

impl Outcome {
    #[must_use]
    pub fn ok() -> Self {
        Self::default()
    }

    fn fail(&mut self, tool: &str, what: &str, path: &Path, error: &io::Error) {
        self.status = 1;
        let text = format!("{tool}: {what} '{}': {}\n", path.display(), strerror(error));
        self.diag.extend_from_slice(text.as_bytes());
    }

    fn say(&mut self, text: &str) {
        self.status = 1;
        self.diag.extend_from_slice(text.as_bytes());
        self.diag.push(b'\n');
    }
}

/// The text `strerror` gives, which is what Rust prints before its ` (os error N)`.
#[must_use]
pub fn strerror(error: &io::Error) -> String {
    let text = error.to_string();
    match text.rfind(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

/// The caller's umask.
#[must_use]
pub fn umask() -> u32 {
    let old = rustix::process::umask(rustix::fs::Mode::empty());
    rustix::process::umask(old);
    old.bits() & 0o777
}

/// `mkdir -p`.
#[must_use]
pub fn mkdir_p(paths: &[PathBuf]) -> Outcome {
    let mut out = Outcome::ok();
    for path in paths {
        if let Err(error) = fs::create_dir_all(path) {
            out.fail("mkdir", "cannot create directory", path, &error);
        }
    }
    out
}

fn exists_following(path: &Path) -> bool {
    fs::metadata(path).is_ok()
}

fn is_dir_following(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// `cp SRC DST`.
#[must_use]
pub fn cp(src: &Path, dst: &Path) -> Outcome {
    let mut out = Outcome::ok();
    let meta = match fs::metadata(src) {
        Ok(meta) => meta,
        Err(error) => {
            out.fail("cp", "cannot stat", src, &error);
            return out;
        }
    };
    if meta.is_dir() {
        out.say(&format!("cp: -r not specified; omitting directory '{}'", src.display()));
        return out;
    }
    let target = if is_dir_following(dst) {
        dst.join(src.file_name().unwrap_or_else(|| OsStr::new("")))
    } else {
        dst.to_path_buf()
    };
    copy_file(src, &meta, &target, &mut out);
    out
}

fn copy_file(src: &Path, meta: &fs::Metadata, dst: &Path, out: &mut Outcome) {
    let dangling = fs::symlink_metadata(dst).is_ok_and(|m| m.file_type().is_symlink())
        && !exists_following(dst);
    if dangling {
        out.say(&format!("cp: not writing through dangling symlink '{}'", dst.display()));
        return;
    }
    if let Ok(existing) = fs::metadata(dst)
        && existing.dev() == meta.dev()
        && existing.ino() == meta.ino()
    {
        out.say(&format!("cp: '{}' and '{}' are the same file", src.display(), dst.display()));
        return;
    }
    let mut reader = match File::open(src) {
        Ok(file) => file,
        Err(error) => {
            out.fail("cp", "cannot open", src, &error);
            return;
        }
    };
    let opened = if exists_following(dst) {
        OpenOptions::new().write(true).truncate(true).open(dst)
    } else {
        OpenOptions::new().write(true).create_new(true).mode(meta.mode() & 0o777).open(dst)
    };
    match opened {
        Ok(mut writer) => {
            if let Err(error) = io::copy(&mut reader, &mut writer) {
                out.fail("cp", "error writing", dst, &error);
            }
        }
        Err(error) => out.fail("cp", "cannot create regular file", dst, &error),
    }
}

/// `cp -r SRC DST`.
#[must_use]
pub fn cp_r(src: &Path, dst: &Path) -> Outcome {
    let mut out = Outcome::ok();
    let meta = match fs::symlink_metadata(src) {
        Ok(meta) => meta,
        Err(error) => {
            out.fail("cp", "cannot stat", src, &error);
            return out;
        }
    };
    let target = if is_dir_following(dst) {
        dst.join(src.file_name().unwrap_or_else(|| OsStr::new("")))
    } else {
        dst.to_path_buf()
    };
    copy_entry(src, &meta, &target, &mut out);
    out
}

fn copy_entry(src: &Path, meta: &fs::Metadata, dst: &Path, out: &mut Outcome) {
    let kind = meta.file_type();
    if kind.is_symlink() {
        match fs::read_link(src) {
            Ok(link) => {
                if let Err(error) = symlink(&link, dst) {
                    out.fail("cp", "cannot create symbolic link", dst, &error);
                }
            }
            Err(error) => out.fail("cp", "cannot read symbolic link", src, &error),
        }
    } else if kind.is_dir() {
        copy_dir(src, meta, dst, out);
    } else if kind.is_file() {
        copy_file(src, meta, dst, out);
    } else {
        out.say(&format!(
            "cp: cannot copy special file '{}': Operation not supported",
            src.display()
        ));
    }
}

fn copy_dir(src: &Path, meta: &fs::Metadata, dst: &Path, out: &mut Outcome) {
    let mode = meta.mode() & 0o777;
    // The directory is made writable for its owner while it is filled, as cp does, and
    // takes its own mode (under the umask) at the end.
    let created = fs::DirBuilder::new().mode(mode | 0o700).create(dst);
    if let Err(error) = created {
        out.fail("cp", "cannot create directory", dst, &error);
        return;
    }
    let entries = match fs::read_dir(src) {
        Ok(entries) => entries,
        Err(error) => {
            out.fail("cp", "cannot open directory", src, &error);
            return;
        }
    };
    let mut names: Vec<_> = entries.filter_map(Result::ok).map(|entry| entry.file_name()).collect();
    names.sort();
    for name in names {
        let from = src.join(&name);
        match fs::symlink_metadata(&from) {
            Ok(child) => copy_entry(&from, &child, &dst.join(&name), out),
            Err(error) => out.fail("cp", "cannot stat", &from, &error),
        }
    }
    let final_mode = mode & !umask();
    if let Err(error) = fs::set_permissions(dst, fs::Permissions::from_mode(final_mode)) {
        out.fail("cp", "preserving permissions for", dst, &error);
    }
}

/// `rm -rf PATH...`.
#[must_use]
pub fn rm_rf(paths: &[PathBuf]) -> Outcome {
    let mut out = Outcome::ok();
    for path in paths {
        remove_tree(path, &mut out);
    }
    out
}

/// Removes a tree; returns whether all of it is gone. A directory with an entry that
/// could not be removed is left alone and is not reported a second time, as `rm` does.
fn remove_tree(path: &Path, out: &mut Outcome) -> bool {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return true,
        Err(error) => {
            out.fail("rm", "cannot remove", path, &error);
            return false;
        }
    };
    if !meta.file_type().is_dir() {
        return match fs::remove_file(path) {
            Ok(()) => true,
            Err(error) => {
                out.fail("rm", "cannot remove", path, &error);
                false
            }
        };
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            out.fail("rm", "cannot remove", path, &error);
            return false;
        }
    };
    let mut names: Vec<_> = entries.filter_map(Result::ok).map(|entry| entry.file_name()).collect();
    names.sort();
    let mut all_gone = true;
    for name in names {
        all_gone &= remove_tree(&path.join(name), out);
    }
    if !all_gone {
        return false;
    }
    match fs::remove_dir(path) {
        Ok(()) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => true,
        Err(error) => {
            out.fail("rm", "cannot remove", path, &error);
            false
        }
    }
}

/// `rm -f PATH`.
#[must_use]
pub fn rm_f(path: &Path) -> Outcome {
    let mut out = Outcome::ok();
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => out.fail("rm", "cannot remove", path, &error),
        Ok(meta) if meta.file_type().is_dir() => {
            out.say(&format!("rm: cannot remove '{}': Is a directory", path.display()));
        }
        Ok(_) => {
            if let Err(error) = fs::remove_file(path) {
                out.fail("rm", "cannot remove", path, &error);
            }
        }
    }
    out
}

/// `chmod +x PATH...`: the execute bits the umask leaves, on the file a link points at.
#[must_use]
pub fn chmod_x(paths: &[PathBuf]) -> Outcome {
    let mut out = Outcome::ok();
    let add = 0o111 & !umask();
    for path in paths {
        match fs::metadata(path) {
            Err(error) => out.fail("chmod", "cannot access", path, &error),
            Ok(meta) => {
                let mode = (meta.mode() & 0o7777) | add;
                if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(mode)) {
                    out.fail("chmod", "changing permissions of", path, &error);
                }
            }
        }
    }
    out
}

/// `touch PATH`: create the file, or set its times to now.
#[must_use]
pub fn touch(path: &Path) -> Outcome {
    let mut out = Outcome::ok();
    let opened = if is_dir_following(path) {
        File::open(path)
    } else {
        OpenOptions::new().write(true).create(true).truncate(false).mode(0o666).open(path)
    };
    match opened.and_then(|file| file.set_modified(SystemTime::now())) {
        Ok(()) => {}
        Err(error) => out.fail("touch", "cannot touch", path, &error),
    }
    out
}

/// `mv SRC DST` within one file system.
#[must_use]
pub fn mv(src: &Path, dst: &Path) -> Outcome {
    let mut out = Outcome::ok();
    if let Err(error) = fs::rename(src, dst) {
        let text = format!(
            "mv: cannot move '{}' to '{}': {}",
            src.display(),
            dst.display(),
            strerror(&error)
        );
        out.say(&text);
    }
    out
}

/// Puts the file `src` at `dst` with mode 755, replacing `dst` in one step: the copy is
/// made beside it and renamed over it, so a process that has `dst` open keeps the old file.
#[must_use]
pub fn install_binary(src: &Path, dst: &Path) -> Outcome {
    let mut out = Outcome::ok();
    let mut staged = dst.as_os_str().to_os_string();
    staged.push(".new");
    let staged = PathBuf::from(staged);
    drop(fs::remove_file(&staged));
    let result = (|| -> io::Result<()> {
        let mut reader = File::open(src)?;
        let mut writer =
            OpenOptions::new().write(true).create_new(true).mode(0o755).open(&staged)?;
        io::copy(&mut reader, &mut writer)?;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
        fs::rename(&staged, dst)
    })();
    if let Err(error) = result {
        drop(fs::remove_file(&staged));
        let text = format!(
            "install: cannot install '{}' as '{}': {}",
            src.display(),
            dst.display(),
            strerror(&error)
        );
        out.say(&text);
    }
    out
}
