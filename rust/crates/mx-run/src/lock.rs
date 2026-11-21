//! The run lock: descriptor 9, open on `STATE_DIR/run.lock`.
//!
//! Every task of a profile shares one working tree, so two runs at once would
//! fight over `HEAD`. The bash does `exec 9>"$STATE_DIR/run.lock"` and then
//! `flock -w "$LOCK_WAIT" 9`. Two properties matter and both are kept:
//!
//! - The descriptor is number 9 and stays inheritable. The agent and whatever it
//!   starts inherit it, so they hold the lock after `run.sh` itself is gone, and
//!   `install.sh` and the helper's `_lock_held` ask the file whether the lock is
//!   taken. A Rust `File` is opened close-on-exec, so [`open_run_lock`] opens
//!   the file, makes a descriptor numbered 9 and duplicates the file onto it,
//!   which leaves the copy without the close-on-exec flag.
//! - The file is created when missing and truncated, as `>` does.
//!
//! No `unsafe` is needed. Descriptor 9 is reached by opening `/dev/null` until
//! the kernel hands out number 9, which the lowest-free-descriptor rule
//! guarantees when 9 is free, and which fails the call when 9 is taken by
//! something this process did not open. The bash replaces whatever sits there;
//! here the run refuses (decision `P2A-fd9`), because closing a descriptor the
//! program does not own needs `unsafe`.

use rustix::io::dup2;
use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::Path;

/// The descriptor the lock lives on.
pub const LOCK_FD: i32 = 9;

/// Why the lock file could not be opened on descriptor 9.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockOpenError {
    /// `open` failed. The text is the system's, such as `Is a directory`.
    Open(String),
    /// Descriptor 9 is already in use by something this process did not open.
    Fd9Taken,
    /// Duplicating the file onto descriptor 9 failed.
    Dup(String),
}

impl std::fmt::Display for LockOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open(reason) | Self::Dup(reason) => f.write_str(reason),
            Self::Fd9Taken => f.write_str("file descriptor 9 is already open in this process"),
        }
    }
}

/// An error as the C library words it: Rust adds ` (os error N)`.
#[must_use]
pub fn os_text(error: &std::io::Error) -> String {
    let text = error.to_string();
    match text.rfind(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

/// The lock file, open on descriptor 9 for the rest of the process.
#[derive(Debug)]
pub struct RunLock {
    fd9: OwnedFd,
}

impl RunLock {
    /// A lock on whatever descriptor the file has. For tests and callers that
    /// cannot claim descriptor 9; the program itself uses [`open_run_lock`].
    #[must_use]
    pub fn from_file(file: File) -> Self {
        Self { fd9: OwnedFd::from(file) }
    }

    /// The descriptor number: [`LOCK_FD`] for a lock made by [`open_run_lock`].
    #[must_use]
    pub fn raw_fd(&self) -> i32 {
        self.fd9.as_raw_fd()
    }
}

/// `exec 9>"$path"`.
pub fn open_run_lock(path: &Path) -> Result<RunLock, LockOpenError> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|error| LockOpenError::Open(os_text(&error)))?;
    let mut target = claim_descriptor_9()?;
    dup2(&file, &mut target).map_err(|error| LockOpenError::Dup(error.to_string()))?;
    Ok(RunLock { fd9: target })
}

/// An owned descriptor numbered 9, or an error when 9 is taken.
fn claim_descriptor_9() -> Result<OwnedFd, LockOpenError> {
    let mut below: Vec<File> = Vec::new();
    loop {
        let probe =
            File::open("/dev/null").map_err(|error| LockOpenError::Open(os_text(&error)))?;
        let number = probe.as_raw_fd();
        if number == LOCK_FD {
            return Ok(OwnedFd::from(probe));
        }
        if number > LOCK_FD {
            return Err(LockOpenError::Fd9Taken);
        }
        below.push(probe);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_os_error_text_loses_rusts_suffix() {
        let error = std::fs::File::open("/nonexistent/x").unwrap_err();
        assert_eq!(os_text(&error), "No such file or directory");
    }

    #[test]
    fn a_directory_cannot_be_the_lock_file() {
        let dir = std::env::temp_dir();
        match open_run_lock(&dir) {
            Err(LockOpenError::Open(reason)) => assert_eq!(reason, "Is a directory"),
            other => panic!("expected an open failure, got {other:?}"),
        }
    }
}
