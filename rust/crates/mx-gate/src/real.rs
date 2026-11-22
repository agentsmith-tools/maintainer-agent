//! The real adapters of the gate's ports: `gh` and `git` through [`mx_ports::Proc`], the
//! `maintainer` helper, file locks, the receipts directory and the clock. This is the only
//! module of the crate that touches the machine, and it starts no process except through `Proc`.
//!
//! One method of [`GhPort`] is one `gh` call, spelled argument for argument as the bash spells it,
//! because the black-box suites record the argv of every stub.

use crate::{
    Account, CommitId, DiffFailure, Dirs, GateGit, GhFailure, GhPort, Helper, HelperMissing,
    HoldersFailure, IdentityPort, IssueNumber, LeaseAnswer, LockError, LockGuard, LockPort,
    MergeState, PrNumber, Progress, Review, RuntimeProbe, Say, StampSource, UpdateFacts, WhoAmI,
};
use mx_core::Fact;
use mx_ports::{Clock, Fs, FsError, GitError, Proc, ProcExit, ProcSpec};
use mx_receipt::{Stamp, iso_with_offset};
use mx_suite::Runtime;
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const GH_TIMEOUT: Duration = Duration::from_mins(2);
const MERGE_TIMEOUT: Duration = Duration::from_mins(5);
const GIT_TIMEOUT: Duration = Duration::from_mins(5);

const TOKEN_NAMES: &[&str] =
    &["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"];

/// How the gate starts the programs it runs.
pub struct Runner<'a> {
    proc: &'a dyn Proc,
    tokens: Vec<(String, String)>,
}

/// What a program printed and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// Exit status; 124 when the timeout fired and 127 when the program was not found.
    pub code: i32,
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
}

impl Ran {
    fn ok(&self) -> bool {
        self.code == 0
    }

    /// What `$(cmd 2>&1)` holds: both streams, trailing newlines removed.
    fn merged(&self) -> String {
        let mut text = self.stdout.clone();
        text.push_str(&self.stderr);
        text.trim_end_matches('\n').to_string()
    }

    /// What `$(cmd)` holds.
    fn out(&self) -> String {
        self.stdout.trim_end_matches('\n').to_string()
    }
}

impl<'a> Runner<'a> {
    /// A runner over `proc`. `tokens` are the credentials of the caller's environment: only `gh`,
    /// `git` and the `maintainer` helper receive them (decision P1H-17).
    #[must_use]
    pub fn new(proc: &'a dyn Proc, vars: &std::collections::BTreeMap<String, String>) -> Self {
        let tokens = TOKEN_NAMES
            .iter()
            .filter_map(|name| vars.get(*name).map(|v| ((*name).to_string(), v.clone())))
            .collect();
        Self { proc, tokens }
    }

    /// Runs `program` with `args`; a program that cannot start is code 127 with the reason as
    /// standard error.
    pub fn run(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
        env: &[(String, String)],
    ) -> Ran {
        let mut spec = ProcSpec::command(program, args.iter().map(ToString::to_string), timeout);
        let base = Path::new(program).file_name().and_then(|n| n.to_str()).unwrap_or(program);
        if matches!(base, "gh" | "git" | "maintainer") {
            for (name, value) in &self.tokens {
                spec = spec.env(name.as_str(), value.as_str());
            }
        }
        for (name, value) in env {
            spec = spec.env(name.as_str(), value.as_str());
        }
        match self.proc.run(&spec) {
            Ok(out) => Ran {
                code: match out.exit {
                    ProcExit::Code(code) => code,
                    ProcExit::TimedOut => 124,
                },
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            },
            Err(error) => Ran { code: 127, stdout: String::new(), stderr: error.to_string() },
        }
    }
}

fn pass_stderr(text: &str) {
    if !text.is_empty() {
        let mut err = std::io::stderr().lock();
        drop(err.write_all(text.as_bytes()));
    }
}

/// `gh`, one method per call.
pub struct RealGh<'a> {
    runner: &'a Runner<'a>,
    slug: String,
}

impl<'a> RealGh<'a> {
    /// A `gh` adapter for the repository `slug`.
    #[must_use]
    pub fn new(runner: &'a Runner<'a>, slug: &str) -> Self {
        Self { runner, slug: slug.to_string() }
    }

    fn gh(&self, args: &[&str], timeout: Duration) -> Ran {
        self.runner.run("gh", args, timeout, &[])
    }

    /// `gh pr view N --repo SLUG --json FIELD --jq JQ`, standard error passed through.
    fn view(&self, pr: &PrNumber, field: &str, jq: &str) -> Result<String, GhFailure> {
        let ran = self.gh(
            &["pr", "view", pr.as_arg(), "--repo", &self.slug, "--json", field, "--jq", jq],
            GH_TIMEOUT,
        );
        pass_stderr(&ran.stderr);
        if ran.ok() { Ok(ran.out()) } else { Err(GhFailure { exit: ran.code, output: ran.out() }) }
    }

    /// The same call with `2>&1`: a failure carries both streams.
    fn view_merged(&self, args: &[&str]) -> Result<String, GhFailure> {
        let ran = self.gh(args, GH_TIMEOUT);
        if ran.ok() {
            Ok(ran.out())
        } else {
            Err(GhFailure { exit: ran.code, output: ran.merged() })
        }
    }
}

fn lines(text: &str) -> Vec<String> {
    text.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
}

fn text_field<'v>(object: &'v Value, name: &str) -> Option<&'v str> {
    object.get(name).and_then(Value::as_str).filter(|s| !s.contains('\n'))
}

impl GhPort for RealGh<'_> {
    fn list_open(&self) -> Result<Vec<PrNumber>, GhFailure> {
        let ran = self.gh(
            &[
                "pr",
                "list",
                "--repo",
                &self.slug,
                "--state",
                "open",
                "--json",
                "number",
                "--jq",
                ".[].number",
            ],
            GH_TIMEOUT,
        );
        if !ran.ok() {
            return Err(GhFailure { exit: ran.code, output: ran.merged() });
        }
        Ok(lines(&ran.stdout).iter().filter_map(|n| PrNumber::parse(n).ok()).collect())
    }

    fn changed_files(&self, pr: &PrNumber) -> Result<Vec<String>, GhFailure> {
        self.view(pr, "files", ".files[].path").map(|text| lines(&text))
    }

    fn head(&self, pr: &PrNumber) -> Result<String, GhFailure> {
        self.view(pr, "headRefOid", ".headRefOid")
    }

    fn base_ref(&self, pr: &PrNumber) -> Result<String, GhFailure> {
        self.view(pr, "baseRefName", ".baseRefName")
    }

    fn merge_state(&self, pr: &PrNumber) -> Result<String, GhFailure> {
        self.view(pr, "mergeStateStatus", ".mergeStateStatus")
    }

    fn review_decision(&self, pr: &PrNumber) -> Result<String, GhFailure> {
        self.view(pr, "reviewDecision", ".reviewDecision")
    }

    fn author(&self, pr: &PrNumber) -> Result<String, GhFailure> {
        self.view_merged(&[
            "pr",
            "view",
            pr.as_arg(),
            "--repo",
            &self.slug,
            "--json",
            "author",
            "--jq",
            ".author.login",
        ])
    }

    fn closing_issues(&self, pr: &PrNumber) -> Result<Vec<IssueNumber>, GhFailure> {
        let text = self.view_merged(&[
            "pr",
            "view",
            pr.as_arg(),
            "--repo",
            &self.slug,
            "--json",
            "closingIssuesReferences",
            "--jq",
            ".closingIssuesReferences[].number",
        ])?;
        // A number that is not a positive integer is not an issue the gate can name; refusing is
        // the safe reading of an answer it cannot use.
        lines(&text)
            .iter()
            .map(|n| {
                n.parse::<u64>().ok().and_then(IssueNumber::new).ok_or_else(|| GhFailure {
                    exit: 0,
                    output: format!(
                        "gh named '{n}' as a closing issue, which is not an issue number"
                    ),
                })
            })
            .collect()
    }

    fn checks(&self, pr: &PrNumber) -> Result<String, GhFailure> {
        // The bash ignores the exit status: 8 means "pending" and the list is still valid.
        let ran = self.gh(
            &["pr", "checks", pr.as_arg(), "--repo", &self.slug, "--json", "name,bucket"],
            GH_TIMEOUT,
        );
        if ran.code == 127 {
            return Err(GhFailure { exit: 127, output: ran.stderr });
        }
        Ok(ran.stdout)
    }

    fn issue_has_label(&self, issue: IssueNumber, label: &str) -> Result<bool, GhFailure> {
        // The label is spelled into the jq program, as the bash does, so a quote or a backslash
        // in it would end the string (decision P3B-6: refused as unreadable).
        if label.contains(['"', '\\', '\n']) {
            return Err(GhFailure {
                exit: 1,
                output: format!("the claim label {label:?} cannot be quoted for gh --jq"),
            });
        }
        let jq = format!("[.labels[].name]|index(\"{label}\")");
        let text = self.view_merged(&[
            "issue",
            "view",
            &issue.to_string(),
            "--repo",
            &self.slug,
            "--json",
            "labels",
            "--jq",
            &jq,
        ])?;
        Ok(!matches!(text.as_str(), "" | "null"))
    }

    fn issue_assignees(&self, issue: IssueNumber) -> Result<Vec<String>, GhFailure> {
        self.view_merged(&[
            "issue",
            "view",
            &issue.to_string(),
            "--repo",
            &self.slug,
            "--json",
            "assignees",
            "--jq",
            ".assignees[].login",
        ])
        .map(|t| lines(&t))
    }

    fn issue_commenters(&self, issue: IssueNumber) -> Result<Vec<String>, GhFailure> {
        self.view_merged(&[
            "issue",
            "view",
            &issue.to_string(),
            "--repo",
            &self.slug,
            "--json",
            "comments",
            "--jq",
            "[.comments[].author.login]|unique|.[]",
        ])
        .map(|t| lines(&t))
    }

    fn update_facts(&self, pr: &PrNumber) -> Result<UpdateFacts, GhFailure> {
        let ran = self.gh(
            &[
                "pr",
                "view",
                pr.as_arg(),
                "--repo",
                &self.slug,
                "--json",
                "headRefOid,mergeStateStatus,reviewDecision,author,isCrossRepository,maintainerCanModify",
            ],
            GH_TIMEOUT,
        );
        let unreadable = |output: &str| GhFailure { exit: 1, output: output.to_string() };
        if !ran.ok() {
            return Err(unreadable(""));
        }
        let value: Value = serde_json::from_str(&ran.stdout).map_err(|_| unreadable("not JSON"))?;
        let author = value.get("author").filter(|a| !a.is_null());
        let author_login = match author {
            None => "",
            Some(object) => object.get("login").and_then(Value::as_str).unwrap_or(""),
        };
        // Every value must be text without a newline, as the python helper insists, so a null
        // reviewDecision is unreadable (decision P30-7, preserved).
        let head = text_field(&value, "headRefOid").ok_or_else(|| unreadable("head"))?;
        let state = text_field(&value, "mergeStateStatus").ok_or_else(|| unreadable("state"))?;
        let decision =
            text_field(&value, "reviewDecision").ok_or_else(|| unreadable("decision"))?;
        if author_login.contains('\n') {
            return Err(unreadable("author"));
        }
        let cross = value
            .get("isCrossRepository")
            .is_some_and(|v| !matches!(v, Value::Null | Value::Bool(false)));
        let modify = value.get("maintainerCanModify") == Some(&Value::Bool(true));
        Ok(UpdateFacts {
            head: head.to_string(),
            state: MergeState::parse(state),
            decision: decision.to_string(),
            author: author_login.to_string(),
            cross_repository: cross,
            maintainer_can_modify: modify,
        })
    }

    fn reviews(&self, pr: &PrNumber) -> Result<Vec<Review>, GhFailure> {
        let ran = self.gh(
            &[
                "api",
                "--paginate",
                &format!("repos/{}/pulls/{pr}/reviews", self.slug),
                "--jq",
                ".[]",
            ],
            GH_TIMEOUT,
        );
        let unreadable = || GhFailure { exit: 2, output: String::new() };
        if !ran.ok() {
            return Err(unreadable());
        }
        let mut reviews = Vec::new();
        for line in ran.stdout.lines().filter(|l| !l.trim().is_empty()) {
            let value: Value = serde_json::from_str(line).map_err(|_| unreadable())?;
            let user = value.get("user").filter(|u| !u.is_null());
            let login = user.and_then(|u| u.get("login")).map(|l| match l {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            });
            reviews.push(Review {
                user: login.unwrap_or_default(),
                state: value.get("state").and_then(Value::as_str).unwrap_or_default().to_string(),
                commit_id: value.get("commit_id").and_then(Value::as_str).map(str::to_string),
            });
        }
        Ok(reviews)
    }

    fn update_branch(&self, pr: &PrNumber, expected_head: &CommitId) -> Result<(), GhFailure> {
        let ran = self.gh(
            &[
                "api",
                "-X",
                "PUT",
                &format!("repos/{}/pulls/{pr}/update-branch", self.slug),
                "-f",
                &format!("expected_head_sha={expected_head}"),
            ],
            GH_TIMEOUT,
        );
        if ran.ok() { Ok(()) } else { Err(GhFailure { exit: ran.code, output: ran.merged() }) }
    }

    fn merge(&self, pr: &PrNumber, head: &CommitId) -> Result<(), GhFailure> {
        let ran = self.gh(
            &[
                "pr",
                "merge",
                pr.as_arg(),
                "--repo",
                &self.slug,
                "--squash",
                "--delete-branch",
                "--match-head-commit",
                head.as_str(),
            ],
            MERGE_TIMEOUT,
        );
        // gh's own words reach the terminal on their own in the bash, between the gate's
        // "approved" line and its "merged" line.
        {
            let mut out = std::io::stdout().lock();
            drop(out.write_all(ran.stdout.as_bytes()));
            drop(out.flush());
        }
        pass_stderr(&ran.stderr);
        if ran.ok() { Ok(()) } else { Err(GhFailure { exit: ran.code, output: String::new() }) }
    }

    fn diff(&self, pr: &PrNumber) -> Result<String, GhFailure> {
        let ran = self.gh(&["pr", "diff", pr.as_arg(), "--repo", &self.slug], GH_TIMEOUT);
        if ran.ok() {
            Ok(ran.stdout)
        } else {
            Err(GhFailure { exit: ran.code, output: ran.merged() })
        }
    }
}

/// `git -C REPO ...`, one method per call of the gate.
pub struct RealGateGit<'a> {
    runner: &'a Runner<'a>,
}

impl<'a> RealGateGit<'a> {
    /// A git adapter over `runner`.
    #[must_use]
    pub fn new(runner: &'a Runner<'a>) -> Self {
        Self { runner }
    }

    fn git(&self, repo: &Path, args: &[&str]) -> Ran {
        let repo = repo.to_string_lossy();
        let mut full = vec!["-C", repo.as_ref()];
        full.extend_from_slice(args);
        self.runner.run("git", &full, GIT_TIMEOUT, &[])
    }
}

impl GateGit for RealGateGit<'_> {
    fn fetch_ref(&self, repo: &Path, refspec: &str) -> Result<(), GitError> {
        let ran = self.git(repo, &["fetch", "-q", "origin", refspec, "--force"]);
        pass_stderr(&ran.stderr);
        if ran.ok() {
            Ok(())
        } else {
            Err(GitError { command: "fetch".into(), reason: ran.stderr })
        }
    }

    fn fetch_main(&self, repo: &Path) {
        drop(self.git(repo, &["fetch", "-q", "origin", "main"]));
    }

    fn rev_parse(&self, repo: &Path, rev: &str) -> Result<String, GitError> {
        let ran = self.git(repo, &["rev-parse", rev]);
        if ran.ok() {
            Ok(ran.out())
        } else {
            Err(GitError { command: "rev-parse".into(), reason: ran.stderr })
        }
    }

    fn merge_base(&self, repo: &Path, a: &str, b: &str) -> Option<String> {
        let ran = self.git(repo, &["merge-base", a, b]);
        Some(ran.out()).filter(|base| ran.ok() && !base.is_empty())
    }

    fn is_ancestor(&self, repo: &Path, ancestor: &str, of: &str) -> bool {
        self.git(repo, &["merge-base", "--is-ancestor", ancestor, of]).ok()
    }

    fn merge_tree(&self, repo: &Path, base: &str, head: &str) -> Option<String> {
        let ran = self.git(repo, &["merge-tree", "--write-tree", base, head]);
        Some(ran.out()).filter(|tree| ran.ok() && !tree.is_empty())
    }

    fn diff_names(
        &self,
        repo: &Path,
        from: &str,
        to: &str,
        pathspecs: &[String],
    ) -> Result<String, DiffFailure> {
        let mut args = vec!["diff", "--name-only", from];
        if !to.is_empty() {
            args.push(to);
        }
        args.push("--");
        args.extend(pathspecs.iter().map(String::as_str));
        let ran = self.git(repo, &args);
        if ran.ok() {
            Ok(ran.out())
        } else {
            Err(DiffFailure {
                reason: format!("git exited {}: {}", ran.code, ran.stderr.trim_end()),
            })
        }
    }

    fn delete_branch(&self, repo: &Path, name: &str) {
        drop(self.git(repo, &["branch", "-D", name]));
    }
}

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The first executable called `name` in `search_path`.
#[must_use]
pub fn find_on_path(search_path: &str, name: &str) -> Option<PathBuf> {
    search_path
        .split(':')
        .filter(|dir| dir.starts_with('/'))
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| executable(candidate))
}

/// `maintainer lease check` and `maintainer holders`.
pub struct RealHelper<'a> {
    runner: &'a Runner<'a>,
    program: Option<PathBuf>,
    env: Vec<(String, String)>,
}

impl<'a> RealHelper<'a> {
    /// The helper: the `maintainer` beside the gate (`beside`), else the one on `search_path`.
    /// `env` is `MAINTAINER_STATE`, `MAINTAINER_SLUG` and `MAINTAINER_PROFILE`.
    #[must_use]
    pub fn new(
        runner: &'a Runner<'a>,
        beside: &Path,
        search_path: &str,
        env: Vec<(String, String)>,
    ) -> Self {
        let beside = beside.join("maintainer");
        let program = if executable(&beside) {
            Some(beside)
        } else {
            find_on_path(search_path, "maintainer")
        };
        Self { runner, program, env }
    }
}

impl Helper for RealHelper<'_> {
    fn lease_check(&self, pr: &PrNumber) -> Result<LeaseAnswer, HelperMissing> {
        let program = self.program.as_ref().ok_or(HelperMissing)?;
        let ran = self.runner.run(
            &program.to_string_lossy(),
            &["lease", "check", pr.as_arg()],
            GH_TIMEOUT,
            &self.env,
        );
        Ok(match ran.code {
            0 => LeaseAnswer::Free,
            1 => LeaseAnswer::Held { output: ran.merged() },
            _ => LeaseAnswer::Unreadable { output: ran.merged() },
        })
    }

    fn holders(&self, issue: IssueNumber) -> Result<Vec<String>, HoldersFailure> {
        let program = self.program.as_ref().ok_or(HoldersFailure::HelperMissing)?;
        let ran = self.runner.run(
            &program.to_string_lossy(),
            &["holders", &issue.to_string()],
            GH_TIMEOUT,
            &self.env,
        );
        if ran.ok() {
            Ok(lines(&ran.merged()))
        } else {
            Err(HoldersFailure::Failed { output: ran.merged() })
        }
    }
}

/// `gh auth switch` and `gh api user`.
pub struct RealIdentity<'a> {
    runner: &'a Runner<'a>,
}

impl<'a> RealIdentity<'a> {
    /// An identity check over `runner`.
    #[must_use]
    pub fn new(runner: &'a Runner<'a>) -> Self {
        Self { runner }
    }
}

impl IdentityPort for RealIdentity<'_> {
    fn who_am_i(&self, account: &Account) -> WhoAmI {
        // The switch comes first and its failure is ignored, as in the bash; it changes gh's
        // global account even in a rehearsal (found-not-fixed 7, preserved).
        drop(self.runner.run(
            "gh",
            &["auth", "switch", "--hostname", "github.com", "--user", account.as_str()],
            GH_TIMEOUT,
            &[],
        ));
        let ran = self.runner.run("gh", &["api", "user", "--jq", ".login"], GH_TIMEOUT, &[]);
        if ran.ok() { WhoAmI::Login(ran.out()) } else { WhoAmI::Failed { exit: ran.code } }
    }
}

/// The standard filesystem behind the `Fs` port.
pub struct StdFs;

impl Fs for StdFs {
    fn read(&self, path: &Path) -> Fact<Vec<u8>> {
        match std::fs::read(path) {
            Ok(bytes) => Fact::Known(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Fact::Absent,
            // `[ -f ]` and `cat` read a directory as "not there"; so does the gate.
            Err(error) if path.is_dir() => Fact::Unreadable(error.to_string()),
            Err(error) => Fact::Unreadable(error.to_string()),
        }
    }

    fn mtime(&self, path: &Path) -> Fact<u64> {
        match std::fs::metadata(path).and_then(|m| m.modified()) {
            Ok(time) => Fact::Known(time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Fact::Absent,
            Err(error) => Fact::Unreadable(error.to_string()),
        }
    }

    fn list(&self, path: &Path) -> Fact<Vec<PathBuf>> {
        let dir = match std::fs::read_dir(path) {
            Ok(dir) => dir,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Fact::Absent,
            Err(error) => return Fact::Unreadable(error.to_string()),
        };
        let mut files: Vec<PathBuf> =
            dir.filter_map(|entry| entry.ok().map(|e| e.path())).collect();
        files.sort();
        if files.is_empty() { Fact::Absent } else { Fact::Known(files) }
    }

    fn write(&self, path: &Path, contents: &[u8]) -> Result<(), FsError> {
        // `mkdir -p "$RECEIPTS"` comes first in the bash, and verify-deps writes through this port.
        if let Some(dir) = path.parent() {
            drop(std::fs::create_dir_all(dir));
        }
        std::fs::write(path, contents)
            .map_err(|error| FsError { path: path.to_path_buf(), reason: error.to_string() })
    }

    fn remove(&self, path: &Path) -> Result<(), FsError> {
        std::fs::remove_file(path)
            .map_err(|error| FsError { path: path.to_path_buf(), reason: error.to_string() })
    }
}

/// `mkdir -p` and `rm -rf` on the standard filesystem.
pub struct StdDirs;

impl Dirs for StdDirs {
    fn make_dir(&self, path: &Path) -> Result<(), String> {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())
    }

    fn remove_tree(&self, path: &Path) {
        drop(std::fs::remove_dir_all(path));
    }
}

/// `exec 200>FILE; flock -n 200`, with the standard library's advisory lock.
///
/// The lock is held until the guard drops, and the kernel drops it when the process ends, as it
/// drops the bash's descriptor 200 (decision P3B-7: no `flock` program is run).
pub struct FileLocks;

impl LockPort for FileLocks {
    fn try_lock(&self, file: &Path) -> Result<LockGuard, LockError> {
        let handle = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(file)
            .map_err(|_| LockError::Unopenable)?;
        match handle.try_lock() {
            Ok(()) => Ok(LockGuard::new(move || drop(handle))),
            Err(_) => Err(LockError::Held),
        }
    }
}

/// `command -v podman || command -v docker` on a given search path.
pub struct PathRuntime {
    /// The directories searched, colon separated.
    pub search_path: String,
}

impl RuntimeProbe for PathRuntime {
    fn runtime(&self) -> Option<Runtime> {
        if find_on_path(&self.search_path, "podman").is_some() {
            Some(Runtime::Podman)
        } else if find_on_path(&self.search_path, "docker").is_some() {
            Some(Runtime::Docker)
        } else {
            None
        }
    }
}

/// The wall clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Standard output, two spaces and a newline per line, as `say` prints it.
pub struct StdoutSay;

impl Say for StdoutSay {
    fn say(&self, line: &Progress) {
        let mut out = std::io::stdout().lock();
        drop(writeln!(out, "  {line}"));
        drop(out.flush());
    }
}

/// `recorded_at` from the system clock and the local offset (`date +%z`), `recorded_by` from the
/// login name the way Python's `getpass.getuser` finds it.
pub struct RealStamps<'a> {
    /// How `date` and `id` are run.
    pub runner: &'a Runner<'a>,
    /// The caller's environment.
    pub vars: &'a std::collections::BTreeMap<String, String>,
}

impl RealStamps<'_> {
    fn offset_secs(&self) -> i32 {
        let ran = self.runner.run("date", &["+%z"], GH_TIMEOUT, &[]);
        let text = ran.out();
        let bytes = text.as_bytes();
        if !ran.ok() || bytes.len() != 5 || !bytes[1..].iter().all(u8::is_ascii_digit) {
            return 0;
        }
        let hours: i32 = text[1..3].parse().unwrap_or(0);
        let minutes: i32 = text[3..5].parse().unwrap_or(0);
        let secs = hours * 3600 + minutes * 60;
        if bytes[0] == b'-' { -secs } else { secs }
    }

    fn user(&self) -> String {
        for name in ["LOGNAME", "USER", "LNAME", "USERNAME"] {
            if let Some(value) = self.vars.get(name).filter(|v| !v.is_empty()) {
                return value.clone();
            }
        }
        let ran = self.runner.run("id", &["-un"], GH_TIMEOUT, &[]);
        if ran.ok() { ran.out() } else { String::new() }
    }
}

impl StampSource for RealStamps<'_> {
    fn stamp(&self) -> Stamp {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let secs = i64::try_from(now.as_secs()).unwrap_or(0);
        Stamp::new(iso_with_offset(secs, now.subsec_micros(), self.offset_secs()), self.user())
    }
}
