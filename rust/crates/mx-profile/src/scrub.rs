//! The secret scrubber: `maintainer_scrub_secret`.
//!
//! `run.sh` pins one GitHub token for the whole run on purpose, so the token is
//! reachable by the agent. What must not happen is the token outliving the run
//! in a transcript or a state file. The scrubber runs on the way out and
//! removes the literal secret from every text file under the paths it is given.
//!
//! The bash function hands the secret to a `python3` child through the
//! environment. This version reads and rewrites the files in-process, so no
//! process is started, no argument vector or environment of a child can carry
//! the secret, and `python3` is not needed. Decision P1F-1.
//!
//! What the bash does, kept here:
//!
//! - A secret under 16 characters, or an empty list of paths, is refused with
//!   return code 1 and no output, because a short needle rewrites every file it
//!   walks. The refusal is a typed error here. Plan decision D15 notes that
//!   `run.sh` ignores it, so a pinned token under 16 characters is never
//!   scrubbed and the run does not notice; the orchestrator, not this crate, is
//!   where D15 gets fixed.
//! - A path that is a file is scrubbed directly. A path that is a directory is
//!   walked top-down and symbolic links to directories are not entered. Inside
//!   the walk, a *directory* named `.git`, `node_modules`, `target` or `ctargets`
//!   is skipped, but a *file* with one of those names is scrubbed, and the walk
//!   root itself is walked whatever its name.
//! - A file that cannot be read or written is skipped without a word.
//! - A file with a NUL byte anywhere is left alone and reported with
//!   `  BINARY, not modified: rotate the secret` after its path. The report line
//!   still begins `scrubbed the pinned token from`, so a binary file reads as
//!   scrubbed. The bash does exactly that. Decision P1F-2.
//! - Files of a directory come before the files of its subdirectories, in the
//!   order the directory lists them.
//! - Exit status 0 whenever the scrub ran, even when it changed nothing.
//!
//! What differs, on purpose: only regular files are read. The bash opens a
//! named pipe and waits for a writer, and opens a device file and reads it to
//! the end. Decision P1F-3.

use mx_ports::Secret;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// What every occurrence of the secret becomes.
pub const MASK: &str = "REDACTED_BY_MAINTAINER_ROTATE_THIS_SECRET";
/// A secret shorter than this many characters is refused.
pub const MIN_SECRET_CHARS: usize = 16;
/// Directories inside a walk that are never entered.
pub const SKIPPED_DIRS: [&str; 4] = [".git", "node_modules", "target", "ctargets"];

/// Why the scrubber did nothing. The bash returns 1 and prints nothing.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScrubRefusal {
    /// The secret is shorter than [`MIN_SECRET_CHARS`] characters. The error
    /// never holds the secret, only its length.
    #[error("the secret is {chars} characters long; the scrubber refuses anything under 16")]
    SecretTooShort { chars: usize },
    /// There is no path to scrub.
    #[error("no path was given to scrub")]
    NoPaths,
}

impl ScrubRefusal {
    /// The bash return code.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        1
    }
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScrubEntry {
    /// The file held the secret and now holds the mask.
    Scrubbed(PathBuf),
    /// The file holds the secret and a NUL byte, and was not touched.
    Binary(PathBuf),
}

/// What a scrub did, in the order it did it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScrubReport {
    pub entries: Vec<ScrubEntry>,
}

impl ScrubReport {
    /// The bytes the bash function prints: one line per entry.
    #[must_use]
    pub fn render(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for entry in &self.entries {
            out.extend_from_slice(b"scrubbed the pinned token from ");
            match entry {
                ScrubEntry::Scrubbed(path) => out.extend_from_slice(path.as_os_str().as_bytes()),
                ScrubEntry::Binary(path) => {
                    out.extend_from_slice(path.as_os_str().as_bytes());
                    out.extend_from_slice(b"  BINARY, not modified: rotate the secret");
                }
            }
            out.push(b'\n');
        }
        out
    }
}

/// `maintainer_scrub_secret`.
///
/// The secret arrives as a [`Secret`], whose `Debug` is redacted, and is never
/// put in an argument vector or an environment: this function starts no process.
pub fn scrub_secret(secret: &Secret, targets: &[PathBuf]) -> Result<ScrubReport, ScrubRefusal> {
    let needle = secret.expose();
    let chars = needle.chars().count();
    if chars < MIN_SECRET_CHARS {
        return Err(ScrubRefusal::SecretTooShort { chars });
    }
    if targets.is_empty() {
        return Err(ScrubRefusal::NoPaths);
    }
    let mut report = ScrubReport::default();
    for target in targets {
        // `os.path.isfile` follows symlinks and is true for regular files only.
        if is_regular_file(target) {
            scrub_file(target, needle.as_bytes(), &mut report);
        } else {
            walk(target, needle.as_bytes(), &mut report);
        }
    }
    Ok(report)
}

/// `os.path.isfile`: the path, followed through symlinks, is a regular file.
fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

/// `os.path.islink`.
fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// `os.walk(top)` with the scrubber's pruning, top-down. Files of a directory
/// are handled before its subdirectories, and the subdirectories in listing
/// order. A directory that cannot be listed, or whose listing fails halfway, is
/// skipped whole, and so is its subtree.
fn walk(top: &Path, needle: &[u8], report: &mut ScrubReport) {
    let mut pending = vec![top.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(listing) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = Vec::new();
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut complete = true;
        for entry in listing {
            let Ok(entry) = entry else {
                complete = false;
                break;
            };
            // `DirEntry.is_dir()` follows symlinks and reads an error as "no".
            let is_dir = match entry.file_type() {
                Ok(kind) if kind.is_dir() => true,
                Ok(kind) if kind.is_symlink() => {
                    std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir())
                }
                _ => false,
            };
            let name = entry.file_name();
            if is_dir {
                if !is_skipped(&name) {
                    dirs.push(dir.join(&name));
                }
            } else {
                files.push(dir.join(&name));
            }
        }
        if !complete {
            continue;
        }
        for file in &files {
            scrub_file(file, needle, report);
        }
        // A symlink to a directory is listed among the directories and not entered.
        for subdir in dirs.into_iter().rev() {
            if !is_symlink(&subdir) {
                pending.push(subdir);
            }
        }
    }
}

fn is_skipped(name: &OsStr) -> bool {
    SKIPPED_DIRS.iter().any(|skipped| OsStr::new(skipped) == name)
}

/// The `scrub(path)` function of the python child.
fn scrub_file(path: &Path, needle: &[u8], report: &mut ScrubReport) {
    // Decision P1F-3: regular files only, so a named pipe cannot hang the exit
    // trap and a device cannot be read without end.
    if !is_regular_file(path) {
        return;
    }
    let Ok(data) = std::fs::read(path) else { return };
    if find(&data, needle, 0).is_none() {
        return;
    }
    if data.contains(&0) {
        report.entries.push(ScrubEntry::Binary(path.to_path_buf()));
        return;
    }
    let replaced = replace_all(&data, needle, MASK.as_bytes());
    if std::fs::write(path, replaced).is_err() {
        return;
    }
    report.entries.push(ScrubEntry::Scrubbed(path.to_path_buf()));
}

/// The first position at or after `from` where `needle` occurs.
fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    // An empty needle would match everywhere. The 16-character rule keeps it out,
    // and this keeps `windows(0)` from panicking if that rule ever moves.
    if needle.is_empty() {
        return None;
    }
    let tail = haystack.get(from..)?;
    tail.windows(needle.len()).position(|window| window == needle).map(|at| from + at)
}

/// `bytes.replace(needle, mask)`: non-overlapping, left to right.
fn replace_all(data: &[u8], needle: &[u8], mask: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut at = 0;
    while let Some(found) = find(data, needle, at) {
        out.extend_from_slice(&data[at..found]);
        out.extend_from_slice(mask);
        at = found + needle.len();
    }
    out.extend_from_slice(&data[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_needle_matches_nowhere_and_replaces_nothing() {
        assert_eq!(find(b"abc", b"", 0), None);
        assert_eq!(replace_all(b"abc", b"", b"X"), b"abc");
    }

    #[test]
    fn find_starts_at_the_given_offset_and_ends_at_the_data() {
        assert_eq!(find(b"abcabc", b"bc", 0), Some(1));
        assert_eq!(find(b"abcabc", b"bc", 2), Some(4));
        assert_eq!(find(b"abcabc", b"bc", 5), None);
        assert_eq!(find(b"abcabc", b"bc", 99), None);
    }
}
