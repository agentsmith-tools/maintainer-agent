//! The ports `receipt_fresh`, the locks and the `say` lines use.

use crate::{DiffFailure, PrNumber, Progress};
use mx_ports::{GitError, Script};
use mx_sandbox::{ContainedTree, SandboxError};
use mx_suite::Recorder;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// The git steps of `receipt_fresh`, `cmd_verify` and `cmd_merge`, run in the maintainer's checkout
/// (`REPO_PATH`). `mx_ports::Git` has `fetch`, `rev_parse`, `diff_names` without a pathspec and
/// `clone`; this port has what the gate calls, with the results the bash reads.
///
/// The real adapter starts `git` through `mx_ports::Proc` with the argv the bash uses (`-C <repo>`),
/// and shares nothing else with `mx_ports::Git`.
pub trait GateGit {
    /// `git -C repo fetch -q origin <refspec> --force`, for `refs/pull/N/head:mm-N` (merge, update)
    /// or `refs/pull/N/head:v-N` (verify). `Err` is the bash's "cannot fetch #N".
    fn fetch_ref(&self, repo: &Path, refspec: &str) -> Result<(), GitError>;

    /// `git -C repo fetch -q origin main 2>/dev/null || true`: failure is ignored by design, so
    /// this returns nothing.
    fn fetch_main(&self, repo: &Path);

    /// `git -C repo rev-parse <rev>`.
    fn rev_parse(&self, repo: &Path, rev: &str) -> Result<String, GitError>;

    /// `git -C repo merge-base <a> <b> 2>/dev/null`: `None` when git printed nothing (no merge
    /// base, or a failure), which sends the staleness check to the strict range.
    fn merge_base(&self, repo: &Path, a: &str, b: &str) -> Option<String>;

    /// `git -C repo merge-base --is-ancestor <a> <b>`: false on any non-zero status.
    fn is_ancestor(&self, repo: &Path, ancestor: &str, of: &str) -> bool;

    /// `git -C repo merge-tree --write-tree <base> <head> 2>/dev/null`: the tree when it succeeded,
    /// `None` on a conflict or a git too old for `--write-tree`.
    fn merge_tree(&self, repo: &Path, base: &str, head: &str) -> Option<String>;

    /// `git -C repo diff --name-only <from> <to> -- <pathspecs>`. The `Result` is the F1 input of
    /// [`crate::ProductionDiff::from_git`]: a failure is `Err` and never an empty string.
    fn diff_names(
        &self,
        repo: &Path,
        from: &str,
        to: &str,
        pathspecs: &[String],
    ) -> Result<String, DiffFailure>;

    /// `git -C repo branch -D <name> >/dev/null 2>&1`: the status is ignored.
    fn delete_branch(&self, repo: &Path, name: &str);
}

/// One call to a [`FakeGateGit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitCall {
    FetchRef { repo: PathBuf, refspec: String },
    FetchMain { repo: PathBuf },
    RevParse { repo: PathBuf, rev: String },
    MergeBase { repo: PathBuf, a: String, b: String },
    IsAncestor { repo: PathBuf, ancestor: String, of: String },
    MergeTree { repo: PathBuf, base: String, head: String },
    DiffNames { repo: PathBuf, from: String, to: String, pathspecs: Vec<String> },
    DeleteBranch { repo: PathBuf, name: String },
}

/// A scripted [`GateGit`]. Calls nobody scripted panic, except the two that return nothing.
#[derive(Debug, Default)]
pub struct FakeGateGit {
    calls: Recorder<GitCall>,
    fetch_ref: Mutex<Script<Result<(), GitError>>>,
    rev_parse: Mutex<Script<Result<String, GitError>>>,
    merge_base: Mutex<Script<Option<String>>>,
    is_ancestor: Mutex<Script<bool>>,
    merge_tree: Mutex<Script<Option<String>>>,
    diff_names: Mutex<Script<Result<String, DiffFailure>>>,
}

fn next<T: Clone>(slot: &Mutex<Script<T>>, call: &str) -> T {
    slot.lock().unwrap_or_else(PoisonError::into_inner).next(call)
}

fn set<T: Clone>(slot: &Mutex<Script<T>>, script: Script<T>) {
    *slot.lock().unwrap_or_else(PoisonError::into_inner) = script;
}

impl FakeGateGit {
    /// A fake with no scripted answers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every call so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<GitCall> {
        self.calls.entries()
    }

    /// Scripts `fetch_ref`.
    pub fn script_fetch_ref(&self, s: Script<Result<(), GitError>>) {
        set(&self.fetch_ref, s);
    }

    /// Scripts `rev_parse`.
    pub fn script_rev_parse(&self, s: Script<Result<String, GitError>>) {
        set(&self.rev_parse, s);
    }

    /// Scripts `merge_base`.
    pub fn script_merge_base(&self, s: Script<Option<String>>) {
        set(&self.merge_base, s);
    }

    /// Scripts `is_ancestor`.
    pub fn script_is_ancestor(&self, s: Script<bool>) {
        set(&self.is_ancestor, s);
    }

    /// Scripts `merge_tree`.
    pub fn script_merge_tree(&self, s: Script<Option<String>>) {
        set(&self.merge_tree, s);
    }

    /// Scripts `diff_names`.
    pub fn script_diff_names(&self, s: Script<Result<String, DiffFailure>>) {
        set(&self.diff_names, s);
    }
}

impl GateGit for FakeGateGit {
    fn fetch_ref(&self, repo: &Path, refspec: &str) -> Result<(), GitError> {
        self.calls.push(GitCall::FetchRef { repo: repo.into(), refspec: refspec.into() });
        next(&self.fetch_ref, "GateGit::fetch_ref")
    }

    fn fetch_main(&self, repo: &Path) {
        self.calls.push(GitCall::FetchMain { repo: repo.into() });
    }

    fn rev_parse(&self, repo: &Path, rev: &str) -> Result<String, GitError> {
        self.calls.push(GitCall::RevParse { repo: repo.into(), rev: rev.into() });
        next(&self.rev_parse, "GateGit::rev_parse")
    }

    fn merge_base(&self, repo: &Path, a: &str, b: &str) -> Option<String> {
        self.calls.push(GitCall::MergeBase { repo: repo.into(), a: a.into(), b: b.into() });
        next(&self.merge_base, "GateGit::merge_base")
    }

    fn is_ancestor(&self, repo: &Path, ancestor: &str, of: &str) -> bool {
        self.calls.push(GitCall::IsAncestor {
            repo: repo.into(),
            ancestor: ancestor.into(),
            of: of.into(),
        });
        next(&self.is_ancestor, "GateGit::is_ancestor")
    }

    fn merge_tree(&self, repo: &Path, base: &str, head: &str) -> Option<String> {
        self.calls.push(GitCall::MergeTree {
            repo: repo.into(),
            base: base.into(),
            head: head.into(),
        });
        next(&self.merge_tree, "GateGit::merge_tree")
    }

    fn diff_names(
        &self,
        repo: &Path,
        from: &str,
        to: &str,
        pathspecs: &[String],
    ) -> Result<String, DiffFailure> {
        self.calls.push(GitCall::DiffNames {
            repo: repo.into(),
            from: from.into(),
            to: to.into(),
            pathspecs: pathspecs.to_vec(),
        });
        next(&self.diff_names, "GateGit::diff_names")
    }

    fn delete_branch(&self, repo: &Path, name: &str) {
        self.calls.push(GitCall::DeleteBranch { repo: repo.into(), name: name.into() });
    }
}

/// Puts the gate's own executable into the tree a container mounts, so a suite that tests the gate
/// can run it from inside the container (the host path of the binary does not exist there).
///
/// The sandbox records a hash of every staged file and refuses to start a container when one
/// changed, and a container of an earlier run has the tree mounted writable. So `verify` calls
/// this IMMEDIATELY before each container run, the prefetch, the unmutated run and the mutated
/// run, and never relies on an earlier call (decision P3W-1).
pub trait GateStage {
    /// Copies the executable to `.container-target/stage/<name>` in `tree`. An `Err` ends the
    /// verify: a container must not start over a file that could not be staged.
    fn stage(&self, tree: &ContainedTree) -> Result<(), SandboxError>;
}

/// A [`GateStage`] that records the root of every tree it was asked to stage into.
#[derive(Debug, Default)]
pub struct FakeGateStage {
    calls: Recorder<PathBuf>,
    fail: Mutex<Option<(usize, SandboxError)>>,
}

impl FakeGateStage {
    /// A stage that succeeds.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes every call answer `error`.
    pub fn fail_with(&self, error: SandboxError) {
        self.fail_from(1, error);
    }

    /// Makes call number `first` (counted from 1) and every later call answer `error`.
    pub fn fail_from(&self, first: usize, error: SandboxError) {
        *self.fail.lock().unwrap_or_else(PoisonError::into_inner) = Some((first, error));
    }

    /// The tree roots staged into so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<PathBuf> {
        self.calls.entries()
    }
}

impl GateStage for FakeGateStage {
    fn stage(&self, tree: &ContainedTree) -> Result<(), SandboxError> {
        self.calls.push(tree.root().to_path_buf());
        let number = self.calls.entries().len();
        match self.fail.lock().unwrap_or_else(PoisonError::into_inner).clone() {
            Some((first, error)) if number >= first => Err(error),
            _ => Ok(()),
        }
    }
}

/// Which lock a command takes. Both are `flock -n` on a file of the receipts directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockKind {
    /// `.merge-<pr>.lock`, held by `merge` (descriptor 200).
    Merge,
    /// `.verify-<pr>.lock`, held by `verify` (descriptor 201).
    Verify,
}

impl LockKind {
    /// The lock file for `pr` in the receipts directory.
    #[must_use]
    pub fn file(self, receipts: &Path, pr: &PrNumber) -> PathBuf {
        let kind = match self {
            Self::Merge => "merge",
            Self::Verify => "verify",
        };
        receipts.join(format!(".{kind}-{pr}.lock"))
    }
}

/// Why a lock was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockError {
    /// The lock file cannot be opened (`exec 200>"$file" || die "cannot open ..."`).
    Unopenable,
    /// Another process holds it (`flock -n` failed).
    Held,
}

/// A lock that is held until the guard drops. The adapter builds it with the release action.
pub struct LockGuard {
    release: Option<Box<dyn FnOnce() + Send>>,
}

impl LockGuard {
    /// A guard that runs `release` when it drops.
    #[must_use]
    pub fn new(release: impl FnOnce() + Send + 'static) -> Self {
        Self { release: Some(Box::new(release)) }
    }
}

impl std::fmt::Debug for LockGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LockGuard")
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

/// One non-blocking lock per action: "one actor per action, enforced and not remembered".
pub trait LockPort {
    /// `flock -n` on `file`. The descriptor stays inheritable by children, as the bash's does.
    fn try_lock(&self, file: &Path) -> Result<LockGuard, LockError>;
}

/// A fake that holds locks in memory.
#[derive(Debug, Default)]
pub struct FakeLockPort {
    held: std::sync::Arc<Mutex<Vec<PathBuf>>>,
    unopenable: Mutex<Vec<PathBuf>>,
    calls: Recorder<PathBuf>,
}

impl FakeLockPort {
    /// A fake with every lock free.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes `file` held by somebody else.
    pub fn hold(&self, file: &Path) {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).push(file.to_path_buf());
    }

    /// Makes `file` impossible to open.
    pub fn make_unopenable(&self, file: &Path) {
        self.unopenable.lock().unwrap_or_else(PoisonError::into_inner).push(file.to_path_buf());
    }

    /// The files whose lock was asked for, in order.
    #[must_use]
    pub fn asked(&self) -> Vec<PathBuf> {
        self.calls.entries()
    }

    /// The files held right now, by anyone.
    #[must_use]
    pub fn held(&self) -> Vec<PathBuf> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl LockPort for FakeLockPort {
    fn try_lock(&self, file: &Path) -> Result<LockGuard, LockError> {
        self.calls.push(file.to_path_buf());
        if self.unopenable.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|f| f == file)
        {
            return Err(LockError::Unopenable);
        }
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if held.iter().any(|f| f == file) {
            return Err(LockError::Held);
        }
        held.push(file.to_path_buf());
        let registry = std::sync::Arc::clone(&self.held);
        let mine = file.to_path_buf();
        Ok(LockGuard::new(move || {
            let mut held = registry.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(at) = held.iter().position(|f| *f == mine) {
                held.remove(at);
            }
        }))
    }
}

/// Where the `say` lines go: standard output, two spaces and the text, a newline.
pub trait Say {
    /// Says one line.
    fn say(&self, line: &Progress);
}

/// A [`Say`] that records what it was told, as the text of each line (two spaces included).
#[derive(Debug, Default)]
pub struct FakeSay {
    lines: Recorder<String>,
}

impl FakeSay {
    /// A fake that has said nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Standard output so far: each line as `say` prints it.
    #[must_use]
    pub fn stdout(&self) -> String {
        self.lines.entries().iter().fold(String::new(), |mut out, l| {
            out.push_str(l);
            out.push('\n');
            out
        })
    }
}

impl Say for FakeSay {
    fn say(&self, line: &Progress) {
        self.lines.push(format!("  {line}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> &'static Path {
        Path::new("/repo")
    }

    #[test]
    fn a_failed_diff_stays_an_error_through_the_port() {
        let git = FakeGateGit::new();
        git.script_diff_names(Script::sequence([
            Err(DiffFailure { reason: "fatal: bad revision".into() }),
            Ok(String::new()),
        ]));
        let pathspecs = vec!["bin/*".to_string()];
        assert_eq!(
            git.diff_names(repo(), "abc..mm-7", "", &pathspecs),
            Err(DiffFailure { reason: "fatal: bad revision".into() })
        );
        assert_eq!(git.diff_names(repo(), "a", "b", &pathspecs), Ok(String::new()));
    }

    #[test]
    fn every_git_call_is_recorded_in_order_with_its_arguments() {
        let git = FakeGateGit::new();
        git.script_fetch_ref(Script::always(Ok(())));
        git.script_merge_base(Script::always(None));
        git.script_is_ancestor(Script::always(true));
        git.script_merge_tree(Script::always(Some("tree".into())));
        git.script_rev_parse(Script::always(Ok("abc".into())));
        git.fetch_ref(repo(), "refs/pull/7/head:mm-7").unwrap();
        git.fetch_main(repo());
        git.rev_parse(repo(), "v-7").unwrap();
        assert_eq!(git.merge_base(repo(), "mm-7", "origin/main"), None);
        assert!(git.is_ancestor(repo(), "v", "base"));
        assert_eq!(git.merge_tree(repo(), "base", "v"), Some("tree".into()));
        git.delete_branch(repo(), "mm-7");
        assert_eq!(
            git.calls(),
            vec![
                GitCall::FetchRef { repo: repo().into(), refspec: "refs/pull/7/head:mm-7".into() },
                GitCall::FetchMain { repo: repo().into() },
                GitCall::RevParse { repo: repo().into(), rev: "v-7".into() },
                GitCall::MergeBase {
                    repo: repo().into(),
                    a: "mm-7".into(),
                    b: "origin/main".into()
                },
                GitCall::IsAncestor {
                    repo: repo().into(),
                    ancestor: "v".into(),
                    of: "base".into()
                },
                GitCall::MergeTree { repo: repo().into(), base: "base".into(), head: "v".into() },
                GitCall::DeleteBranch { repo: repo().into(), name: "mm-7".into() },
            ]
        );
    }

    #[test]
    #[should_panic(expected = "unscripted call: no answer left for GateGit::diff_names")]
    fn an_unscripted_diff_panics() {
        drop(FakeGateGit::new().diff_names(repo(), "a", "b", &[]));
    }

    #[test]
    fn the_lock_files_are_named_as_the_bash_names_them() {
        let pr = PrNumber::parse("7").unwrap();
        assert_eq!(
            LockKind::Merge.file(Path::new("/s/receipts"), &pr),
            Path::new("/s/receipts/.merge-7.lock")
        );
        assert_eq!(
            LockKind::Verify.file(Path::new("/s/receipts"), &pr),
            Path::new("/s/receipts/.verify-7.lock")
        );
    }

    #[test]
    fn a_second_lock_on_the_same_file_is_refused_until_the_first_guard_drops() {
        let locks = FakeLockPort::new();
        let file = Path::new("/s/receipts/.merge-7.lock");
        let guard = locks.try_lock(file).unwrap();
        assert_eq!(locks.try_lock(file).unwrap_err(), LockError::Held);
        drop(guard);
        assert!(locks.try_lock(file).is_ok());
        assert_eq!(locks.asked().len(), 3);
    }

    #[test]
    fn a_lock_held_by_somebody_else_and_an_unopenable_file_are_two_errors() {
        let locks = FakeLockPort::new();
        locks.hold(Path::new("/a"));
        locks.make_unopenable(Path::new("/b"));
        assert_eq!(locks.try_lock(Path::new("/a")).unwrap_err(), LockError::Held);
        assert_eq!(locks.try_lock(Path::new("/b")).unwrap_err(), LockError::Unopenable);
        assert_eq!(locks.held(), vec![PathBuf::from("/a")]);
    }

    #[test]
    fn say_prints_two_spaces_and_the_text_and_a_newline() {
        let say = FakeSay::new();
        say.say(&Progress::NoReceiptsRecorded);
        say.say(&Progress::Merged { pr: PrNumber::parse("7").unwrap() });
        assert_eq!(say.stdout(), "  no receipts recorded\n  merged #7\n");
    }
}
