//! The two answers to [`Host`]: the real machine and a table.

use crate::Host;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Answers from the file system and a search path.
///
/// `on_path` is `command -v` against the path the orchestrator exports
/// (`$HOME/.local/bin:$HOME/.cargo/bin:$PATH`, see [`RealHost::for_orchestrator`]):
/// the entries are tried in order, an empty entry means the current directory,
/// and the first entry that holds an executable regular file wins.
///
/// "Executable" means a regular file with an execute bit set for anyone. The
/// shell asks the kernel whether the effective user may execute the file, and
/// `std` has no safe way to ask that. The two differ for a file that is
/// executable for some other user and not for this one.
///
/// Bash 5.3 also reports a non-executable regular file, and a FIFO, as found when
/// no executable file of that name exists in the path. Here neither is a program,
/// so `check` refuses a run that the bash would have started and failed with 126
/// (decision P2B-9). That is the safe side of the difference.
#[derive(Debug, Clone)]
pub struct RealHost {
    dirs: Vec<PathBuf>,
}

impl RealHost {
    /// A host that searches `search_path`, entries split the way the shell does.
    #[must_use]
    pub fn new(search_path: &OsStr) -> Self {
        Self { dirs: std::env::split_paths(search_path).collect() }
    }

    /// A host that searches the path `lib/run.sh` exports before it starts a
    /// backend: `$HOME/.local/bin:$HOME/.cargo/bin:$PATH`.
    #[must_use]
    pub fn for_orchestrator(home: &Path, path: &OsStr) -> Self {
        let mut joined = OsString::new();
        joined.push(home);
        joined.push("/.local/bin:");
        joined.push(home);
        joined.push("/.cargo/bin:");
        joined.push(path);
        Self::new(&joined)
    }
}

impl Host for RealHost {
    fn on_path(&self, program: &str) -> bool {
        // A name with a slash is not searched for: `command -v` tests that path
        // itself. The backends only ask for bare program names, and a slash here
        // would let a caller probe an arbitrary path as if it were a tool.
        if program.is_empty() || program.contains('/') {
            return false;
        }
        self.dirs.iter().any(|dir| is_executable_file(&dir.join(program)))
    }

    fn is_file(&self, path: &Path) -> bool {
        std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
    }

    fn file_len(&self, path: &Path) -> Option<u64> {
        std::fs::metadata(path).ok().map(|meta| meta.len())
    }

    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// What a [`FakeHost`] was told about one path.
#[derive(Clone, PartialEq, Eq)]
enum Declared {
    /// `[ -f path ]` fails.
    Absent,
    /// A regular file with these contents.
    File(Vec<u8>),
    /// `[ -f path ]` succeeds and every read fails (a file without read permission).
    Unreadable,
}

/// Answers from tables a test fills in, and panics on a question nobody
/// declared. A fake that answered "no" to everything it was not told about
/// would let a backend ask the wrong question and still pass.
///
/// `Debug` prints the declared names and paths and never a file's contents,
/// because a test may store a key in a file.
#[derive(Clone, Default)]
pub struct FakeHost {
    programs: BTreeMap<String, bool>,
    files: BTreeMap<PathBuf, Declared>,
}

impl FakeHost {
    /// A host that knows nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `program` is on the search path.
    #[must_use]
    pub fn with_program(mut self, program: &str) -> Self {
        self.programs.insert(program.to_string(), true);
        self
    }

    /// `program` is not on the search path.
    #[must_use]
    pub fn without_program(mut self, program: &str) -> Self {
        self.programs.insert(program.to_string(), false);
        self
    }

    /// `path` is a file with these contents.
    #[must_use]
    pub fn with_file(mut self, path: impl Into<PathBuf>, contents: impl Into<Vec<u8>>) -> Self {
        self.files.insert(path.into(), Declared::File(contents.into()));
        self
    }

    /// `path` is not a file.
    #[must_use]
    pub fn without_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.files.insert(path.into(), Declared::Absent);
        self
    }

    /// `path` is a file that cannot be read.
    #[must_use]
    pub fn with_unreadable_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.files.insert(path.into(), Declared::Unreadable);
        self
    }

    fn declared(&self, path: &Path, question: &str) -> &Declared {
        self.files.get(path).unwrap_or_else(|| {
            panic!("FakeHost: undeclared path {} asked by {question}", path.display())
        })
    }
}

impl fmt::Debug for FakeHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let files: Vec<(&Path, &str)> = self
            .files
            .iter()
            .map(|(path, declared)| {
                let kind = match declared {
                    Declared::Absent => "absent",
                    Declared::File(_) => "file",
                    Declared::Unreadable => "unreadable",
                };
                (path.as_path(), kind)
            })
            .collect();
        f.debug_struct("FakeHost").field("programs", &self.programs).field("files", &files).finish()
    }
}

impl Host for FakeHost {
    fn on_path(&self, program: &str) -> bool {
        *self
            .programs
            .get(program)
            .unwrap_or_else(|| panic!("FakeHost: undeclared program {program} asked by on_path"))
    }

    fn is_file(&self, path: &Path) -> bool {
        !matches!(self.declared(path, "is_file"), Declared::Absent)
    }

    fn file_len(&self, path: &Path) -> Option<u64> {
        match self.declared(path, "file_len") {
            Declared::File(bytes) => Some(bytes.len() as u64),
            Declared::Absent | Declared::Unreadable => None,
        }
    }

    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        match self.declared(path, "read") {
            Declared::File(bytes) => Some(bytes.clone()),
            Declared::Absent | Declared::Unreadable => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_refuses_a_program_nobody_declared() {
        let host = FakeHost::new().with_program("claude");
        assert!(host.on_path("claude"));
        let result = std::panic::catch_unwind(|| host.on_path("codex"));
        assert!(result.is_err());
    }

    #[test]
    fn debug_of_a_fake_never_shows_file_contents() {
        let host = FakeHost::new().with_file("/home/u/.groq-staging-key", "gsk_supersecret");
        let shown = format!("{host:?}");
        assert!(shown.contains(".groq-staging-key"));
        assert!(!shown.contains("supersecret"));
    }
}
