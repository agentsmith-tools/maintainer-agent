//! The real adapters of the `verify-deps` ports: [`Net`], [`Classifier`] and [`WholeFile`]
//! through [`mx_ports::Proc`], with the argument vector the bash runs, and the three `gh` reads
//! the flow makes ([`ProcGh`]).
//!
//! Every string that goes into an API path is checked here, whatever the caller already checked:
//! the pin tag and the action come out of a diff a pull request author wrote, the tag object and
//! the merge base come out of GitHub's answers, and GitHub normalises a path before it resolves
//! it (`v4.2.2/../pull/1/head` reaches a pull request's head). A string that fails is a call
//! that was not made, and it reads as the network not answering: the pin stays unverified.

use crate::deps::{
    ActionRepo, Classifier, ClassifierExit, ClassifierRun, DepsGh, GitObject, Net, NetFailure,
    PlainPath, TagRef, WholeFile,
};
use crate::{CommitId, GhFailure, PrNumber};
use mx_ports::{Proc, ProcError, ProcExit, ProcOutput, ProcSpec};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::time::Duration;

/// How long one `gh api` call may take. The bash sets no limit; the port has no unbounded call
/// (decision `P3C-5`).
pub const GH_TIMEOUT: Duration = Duration::from_mins(1);
/// How long a registry script may take. The scripts bound their own requests at 30 s each.
pub const SCRIPT_TIMEOUT: Duration = Duration::from_mins(2);

/// `owner/repo`: the shape `MAINTAINER_SLUG` has, and the only one that goes into
/// `repos/<slug>/...`.
#[must_use]
pub fn is_slug(text: &str) -> bool {
    let part = |p: &str| {
        !p.is_empty()
            && p.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    };
    text.split_once('/').is_some_and(|(owner, repo)| part(owner) && part(repo))
        && !text.contains("..")
}

/// A ref name that is safe in an API path: `/`-separated segments, each starting with a letter,
/// digit, `_` or `+` and holding only those, `.` and `-`. This is the classifier's `REF_SEGMENT`
/// (`classify_dependency_bump.py`), so no segment is empty, `.` or `..`.
#[must_use]
pub fn is_ref_name(text: &str) -> bool {
    !text.is_empty()
        && text.split('/').all(|segment| {
            let mut bytes = segment.bytes();
            bytes.next().is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+'))
                && bytes
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
        })
}

fn is_hex(text: &str, lengths: std::ops::RangeInclusive<usize>) -> bool {
    lengths.contains(&text.len()) && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// What the child printed that a person or a comparison will read: text, with every control
/// character but the newline and the tab made visible (`?`), so a registry cannot move the
/// cursor or recolour a refusal. Trailing newlines go, as `$(...)` drops them.
fn printable(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text: String = text
        .chars()
        .map(|c| if c.is_control() && c != '\n' && c != '\t' { '?' } else { c })
        .collect();
    text.trim_end_matches('\n').to_string()
}

/// `2>&1`: what the child printed on standard output, then on standard error.
fn combined(out: &ProcOutput) -> String {
    let mut bytes = out.stdout.clone();
    bytes.extend_from_slice(&out.stderr);
    printable(&bytes)
}

/// A call whose standard error the bash leaves on the terminal (`$(cmd)` and `cmd > file` without
/// `2>&1`): the child's words reach ours, so a person sees what `gh` said before the refusal.
fn pass_stderr(out: &ProcOutput) {
    if !out.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
    }
}

fn refused(reason: impl Into<String>) -> NetFailure {
    NetFailure { output: reason.into() }
}

/// Runs a call and keeps its output only when it exited 0.
fn run_ok(proc: &dyn Proc, spec: &ProcSpec) -> Result<ProcOutput, NetFailure> {
    match proc.run(spec) {
        Ok(out) if out.exit.success() => Ok(out),
        Ok(out) if out.exit == ProcExit::TimedOut => {
            Err(refused(format!("{} timed out\n{}", spec.argv.first(), combined(&out))))
        }
        Ok(out) => Err(refused(combined(&out))),
        Err(error) => Err(refused(error.to_string())),
    }
}

/// [`run_ok`] for a call the bash does not merge with `2>&1`: standard error is passed on and is
/// not part of the failure text.
fn run_quiet(proc: &dyn Proc, spec: &ProcSpec) -> Result<ProcOutput, NetFailure> {
    match proc.run(spec) {
        Ok(out) => {
            pass_stderr(&out);
            match out.exit {
                ProcExit::Code(0) => Ok(out),
                ProcExit::TimedOut => Err(refused(format!("{} timed out", spec.argv.first()))),
                ProcExit::Code(code) => {
                    Err(refused(format!("{} exited {code}", spec.argv.first())))
                }
            }
        }
        Err(error) => Err(refused(error.to_string())),
    }
}

/// [`Net`] through `gh` and the two registry scripts.
pub struct RealNet<'a> {
    proc: &'a dyn Proc,
    image_resolver: PathBuf,
    pypi_verifier: PathBuf,
}

impl<'a> RealNet<'a> {
    /// An adapter that starts `gh` and `python3 <script>` through `proc`.
    #[must_use]
    pub fn new(proc: &'a dyn Proc, image_resolver: PathBuf, pypi_verifier: PathBuf) -> Self {
        Self { proc, image_resolver, pypi_verifier }
    }

    /// `gh api <path> [--jq <expr>]`, standard output and error together.
    fn gh_api(&self, path: &str, jq: &str) -> Result<String, NetFailure> {
        let spec = ProcSpec::command("gh", ["api", path, "--jq", jq], GH_TIMEOUT);
        run_ok(self.proc, &spec).map(|out| combined(&out))
    }
}

/// `read -r a b c <<<"$line"` with `IFS=$'\t'`: tab is whitespace, so runs of tabs are one
/// separator, leading and trailing tabs are dropped, and the last name takes the rest.
fn tab_fields<const N: usize>(line: &str) -> [String; N] {
    let mut fields: [String; N] = std::array::from_fn(|_| String::new());
    let mut rest = line.trim_matches('\t');
    for (at, field) in fields.iter_mut().enumerate() {
        if at + 1 == N {
            *field = rest.to_string();
            break;
        }
        if let Some((head, tail)) = rest.split_once('\t') {
            *field = head.to_string();
            rest = tail.trim_start_matches('\t');
        } else {
            *field = rest.to_string();
            rest = "";
        }
    }
    fields
}

impl Net for RealNet<'_> {
    fn branch_ref(&self, repo: &ActionRepo, branch: &str) -> Result<String, NetFailure> {
        if !is_ref_name(branch) {
            return Err(refused(format!(
                "'{branch}' is not a plain branch name, so it is not put in an API URL"
            )));
        }
        self.gh_api(&format!("repos/{repo}/git/ref/heads/{branch}"), ".ref")
    }

    fn compare_status(
        &self,
        repo: &ActionRepo,
        base: &str,
        head: &str,
    ) -> Result<String, NetFailure> {
        if !is_ref_name(base) || !is_hex(head, 7..=40) {
            return Err(refused(format!(
                "'{base}...{head}' is not a plain comparison, so it is not put in an API URL"
            )));
        }
        self.gh_api(&format!("repos/{repo}/compare/{base}...{head}"), ".status")
    }

    fn tag_ref(&self, repo: &ActionRepo, tag: &str) -> Result<TagRef, NetFailure> {
        if !is_ref_name(tag) {
            return Err(refused(format!(
                "'{tag}' is not a plain tag name, so it is not put in an API URL"
            )));
        }
        let line = self.gh_api(
            &format!("repos/{repo}/git/ref/tags/{tag}"),
            "[.ref, .object.type, .object.sha] | @tsv",
        )?;
        let first = line.lines().next().unwrap_or("");
        let [name, object_type, object_sha] = tab_fields::<3>(first);
        Ok(TagRef { name, object_type, object_sha })
    }

    fn tag_object(&self, repo: &ActionRepo, sha: &str) -> Result<GitObject, NetFailure> {
        if !is_hex(sha, 40..=40) {
            return Err(refused(format!(
                "'{sha}' is not a commit id, so it is not put in an API URL"
            )));
        }
        let line = self.gh_api(
            &format!("repos/{repo}/git/tags/{sha}"),
            "[.object.type, .object.sha] | @tsv",
        )?;
        let first = line.lines().next().unwrap_or("");
        let [object_type, sha] = tab_fields::<2>(first);
        Ok(GitObject { object_type, sha })
    }

    fn image_digest(&self, image: &str, tag: &str) -> Result<String, NetFailure> {
        let script = self.image_resolver.display().to_string();
        let spec = ProcSpec::command("python3", [script.as_str(), image, tag], SCRIPT_TIMEOUT);
        run_ok(self.proc, &spec).map(|out| combined(&out))
    }

    fn pypi_release(
        &self,
        name: &str,
        version: &str,
        hashes: &[String],
    ) -> Result<String, NetFailure> {
        let script = self.pypi_verifier.display().to_string();
        let joined = hashes.join(",");
        let spec = ProcSpec::command(
            "python3",
            [script.as_str(), name, version, joined.as_str()],
            SCRIPT_TIMEOUT,
        );
        run_ok(self.proc, &spec).map(|out| combined(&out))
    }

    fn merge_base(
        &self,
        slug: &str,
        base_ref: &str,
        head: &CommitId,
    ) -> Result<String, NetFailure> {
        if !is_slug(slug) || !is_ref_name(base_ref) {
            return Err(refused(format!(
                "'{slug}' and '{base_ref}' are not a plain repository and branch, so they are not put in an API URL"
            )));
        }
        // No `2>&1`: the bash reads the merge base from standard output alone.
        let spec = ProcSpec::command(
            "gh",
            [
                "api".to_string(),
                format!("repos/{slug}/compare/{base_ref}...{head}"),
                "--jq".into(),
                ".merge_base_commit.sha".into(),
            ],
            GH_TIMEOUT,
        );
        run_quiet(self.proc, &spec).map(|out| printable(&out.stdout))
    }

    fn file_at(&self, slug: &str, path: &PlainPath, rev: &CommitId) -> Result<Vec<u8>, NetFailure> {
        if !is_slug(slug) {
            return Err(refused(format!(
                "'{slug}' is not a plain repository, so it is not put in an API URL"
            )));
        }
        let spec = ProcSpec::command(
            "gh",
            [
                "api".to_string(),
                "-H".into(),
                "Accept: application/vnd.github.raw".into(),
                format!("repos/{slug}/contents/{path}?ref={rev}"),
            ],
            GH_TIMEOUT,
        );
        // The file as GitHub sent it, byte for byte: the whole-file check reads bytes.
        run_quiet(self.proc, &spec).map(|out| out.stdout)
    }
}

/// [`Classifier`] through `timeout`-bounded `python3 classify_dependency_bump.py`, the diff on
/// standard input. The helper's standard error reaches ours, as it reaches the terminal in bash.
pub struct RealClassifier<'a> {
    proc: &'a dyn Proc,
    script: PathBuf,
}

impl<'a> RealClassifier<'a> {
    /// An adapter that runs `python3 <script>`.
    #[must_use]
    pub fn new(proc: &'a dyn Proc, script: PathBuf) -> Self {
        Self { proc, script }
    }
}

impl Classifier for RealClassifier<'_> {
    fn classify(&self, diff: &str, limit: Duration) -> Result<ClassifierRun, NetFailure> {
        let script = self.script.display().to_string();
        let spec = ProcSpec::command("python3", [script.as_str()], limit).stdin(diff.as_bytes());
        let out = self.proc.run(&spec).map_err(|error| refused(error.to_string()))?;
        // Best effort: the helper's own words, where the bash leaves them on the terminal.
        let _ = std::io::stderr().write_all(&out.stderr);
        let exit = match out.exit {
            ProcExit::TimedOut => ClassifierExit::TimedOut,
            ProcExit::Code(code) => ClassifierExit::Code(code),
        };
        Ok(ClassifierRun { exit, stdout: String::from_utf8_lossy(&out.stdout).into_owned() })
    }
}

/// A private scratch directory, removed when it goes out of scope (`mktemp -d` and `rm -rf`).
struct Scratch(PathBuf);

impl Scratch {
    /// Creates `<tmp>/mx-verify-deps-<pid>-<time>-<n>` with mode 0700. `create` fails when the
    /// name exists, so a planted link or directory is never entered.
    fn new() -> std::io::Result<Self> {
        let base = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        for attempt in 0..64u32 {
            let path =
                base.join(format!("mx-verify-deps-{}-{nanos:08x}-{attempt}", std::process::id()));
            match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "no free scratch name"))
    }

    fn put(&self, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        let path = self.0.join(name);
        let mut file =
            std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        file.write_all(bytes)?;
        Ok(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// [`WholeFile`] through `python3 verify_pip_files.py <base> <head>` on two files in a private
/// scratch directory.
pub struct RealWholeFile<'a> {
    proc: &'a dyn Proc,
    script: PathBuf,
}

impl<'a> RealWholeFile<'a> {
    /// An adapter that runs `python3 <script>`.
    #[must_use]
    pub fn new(proc: &'a dyn Proc, script: PathBuf) -> Self {
        Self { proc, script }
    }
}

impl WholeFile for RealWholeFile<'_> {
    fn check(&self, _path: &PlainPath, base: &[u8], head: &[u8]) -> Result<String, NetFailure> {
        let scratch = Scratch::new()
            .map_err(|error| refused(format!("cannot create a scratch directory: {error}")))?;
        let write = |name: &str, bytes: &[u8]| -> Result<PathBuf, NetFailure> {
            scratch
                .put(name, bytes)
                .map_err(|error| refused(format!("cannot write {name}: {error}")))
        };
        let (base, head) = (write("base", base)?, write("head", head)?);
        let script = self.script.display().to_string();
        let spec = ProcSpec::command(
            "python3",
            [script.as_str(), &base.display().to_string(), &head.display().to_string()],
            SCRIPT_TIMEOUT,
        );
        run_ok(self.proc, &spec).map(|out| combined(&out))
    }
}

/// [`DepsGh`] through `gh`, until the gate's own `gh` adapter exists.
pub struct ProcGh<'a> {
    proc: &'a dyn Proc,
    slug: String,
}

impl<'a> ProcGh<'a> {
    /// An adapter for the repository `slug`, which must be `owner/repo`.
    #[must_use]
    pub fn new(proc: &'a dyn Proc, slug: &str) -> Self {
        Self { proc, slug: slug.to_string() }
    }

    /// Runs `gh` and returns what it exited with and printed. With `pass_stderr`, what it said on
    /// standard error also reaches ours, as it reaches the terminal in `$(gh ...) || die`.
    fn run(&self, args: Vec<String>, pass_stderr_on: bool) -> Result<ProcOutput, GhFailure> {
        let spec = ProcSpec::command("gh", args, GH_TIMEOUT);
        let result = self.proc.run(&spec);
        if let (true, Ok(out)) = (pass_stderr_on, &result) {
            pass_stderr(out);
        }
        match result {
            Ok(out) if out.exit.success() => Ok(out),
            Ok(out) => Err(GhFailure {
                exit: match out.exit {
                    ProcExit::Code(code) => code,
                    ProcExit::TimedOut => 124,
                },
                output: combined(&out),
            }),
            Err(error) => Err(GhFailure {
                exit: if matches!(error, ProcError::NotFound { .. }) { 127 } else { 126 },
                output: error.to_string(),
            }),
        }
    }

    fn view(&self, pr: &PrNumber, field: &str, jq: &str) -> Result<String, GhFailure> {
        let args =
            ["pr", "view", pr.as_arg(), "--repo", self.slug.as_str(), "--json", field, "--jq", jq];
        let out = self.run(args.map(String::from).to_vec(), true)?;
        Ok(printable(&out.stdout))
    }
}

impl DepsGh for ProcGh<'_> {
    fn head(&self, pr: &PrNumber) -> Result<String, GhFailure> {
        self.view(pr, "headRefOid", ".headRefOid")
    }

    fn base_ref(&self, pr: &PrNumber) -> Result<String, GhFailure> {
        self.view(pr, "baseRefName", ".baseRefName")
    }

    fn diff(&self, pr: &PrNumber) -> Result<String, GhFailure> {
        // Never `--patch`: an mbox carries each commit message, which its author writes.
        let args = ["pr", "diff", pr.as_arg(), "--repo", self.slug.as_str()];
        let out = self.run(args.map(String::from).to_vec(), false)?;
        // The classifier decodes strictly and refuses a diff that is not UTF-8, and refuses the
        // control characters that make two readers split a line differently. A lossy decode or a
        // cleaned copy would hand it a diff it can accept, so the diff is exactly what `gh`
        // printed, or it is not read (decision `P3C-6`). Standard error is not merged in: the
        // bash merged it, and a warning on it became a line of the diff.
        let diff = String::from_utf8(out.stdout).map_err(|_| GhFailure {
            exit: 0,
            output: "the diff is not UTF-8, so it was not read".to_string(),
        })?;
        Ok(diff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::{Verdict, WholeFile};
    use mx_ports::{Effect, EffectLog, FakeProc, ProcEffect, RealProc, Script};

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn repo() -> ActionRepo {
        ActionRepo::of_action("actions/checkout").unwrap()
    }

    fn commit() -> CommitId {
        CommitId::parse(SHA).unwrap()
    }

    fn out(code: i32, stdout: &str, stderr: &str) -> ProcOutput {
        ProcOutput {
            exit: ProcExit::Code(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn net(proc: &FakeProc) -> RealNet<'_> {
        RealNet::new(proc, "/s/resolve.py".into(), "/s/pypi.py".into())
    }

    fn scripted(argv: &[&str], answer: ProcOutput) -> (FakeProc, EffectLog) {
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        proc.script(argv, Script::always(Ok(answer)));
        (proc, log)
    }

    fn calls(log: &EffectLog) -> usize {
        log.entries().len()
    }

    // ----- argument vectors, one per call of the bash -----

    #[test]
    fn each_network_call_starts_the_program_with_the_argv_the_bash_uses() {
        let (proc, log) = scripted(
            &["gh", "api", "repos/actions/checkout/git/ref/heads/stable", "--jq", ".ref"],
            out(0, "refs/heads/stable\n", ""),
        );
        assert_eq!(net(&proc).branch_ref(&repo(), "stable").unwrap(), "refs/heads/stable");

        let (proc, _) = scripted(
            &[
                "gh",
                "api",
                &format!("repos/actions/checkout/compare/stable...{SHA}"),
                "--jq",
                ".status",
            ],
            out(0, "behind\n", ""),
        );
        assert_eq!(net(&proc).compare_status(&repo(), "stable", SHA).unwrap(), "behind");

        assert_eq!(calls(&log), 1);
    }

    #[test]
    fn the_tag_call_asks_for_the_ref_endpoint_and_a_tab_separated_answer() {
        let (proc, log) = scripted(
            &[
                "gh",
                "api",
                "repos/actions/checkout/git/ref/tags/v4.2.2",
                "--jq",
                "[.ref, .object.type, .object.sha] | @tsv",
            ],
            out(0, &format!("refs/tags/v4.2.2\tcommit\t{SHA}\n"), ""),
        );
        let tag = net(&proc).tag_ref(&repo(), "v4.2.2").unwrap();
        assert_eq!(
            tag,
            TagRef {
                name: "refs/tags/v4.2.2".into(),
                object_type: "commit".into(),
                object_sha: SHA.into()
            }
        );
        assert_eq!(calls(&log), 1);
    }

    #[test]
    fn the_tag_object_call_asks_the_tags_endpoint_with_the_sha() {
        let (proc, _) = scripted(
            &[
                "gh",
                "api",
                &format!("repos/actions/checkout/git/tags/{SHA}"),
                "--jq",
                "[.object.type, .object.sha] | @tsv",
            ],
            out(0, &format!("commit\t{SHA}\n"), ""),
        );
        assert_eq!(
            net(&proc).tag_object(&repo(), SHA).unwrap(),
            GitObject { object_type: "commit".into(), sha: SHA.into() }
        );
    }

    #[test]
    fn the_image_and_pypi_calls_run_python_on_the_scripts_with_the_bash_arguments() {
        let (proc, _) = scripted(
            &["python3", "/s/resolve.py", "docker.io/library/rust", "1-bookworm"],
            out(0, "sha256:abc\n", ""),
        );
        assert_eq!(
            net(&proc).image_digest("docker.io/library/rust", "1-bookworm").unwrap(),
            "sha256:abc"
        );
        let hashes = vec!["a".repeat(64), "b".repeat(64)];
        let joined = hashes.join(",");
        let (proc, _) = scripted(
            &["python3", "/s/pypi.py", "PyYAML", "6.0.3", &joined],
            out(0, "2 of 5 files\n", ""),
        );
        assert_eq!(net(&proc).pypi_release("PyYAML", "6.0.3", &hashes).unwrap(), "2 of 5 files");
    }

    #[test]
    fn the_merge_base_call_reads_standard_output_alone() {
        let (proc, _) = scripted(
            &[
                "gh",
                "api",
                &format!("repos/o/r/compare/main...{SHA}"),
                "--jq",
                ".merge_base_commit.sha",
            ],
            out(0, &format!("{}\n", "c".repeat(40)), "warning: a new release is available"),
        );
        assert_eq!(net(&proc).merge_base("o/r", "main", &commit()).unwrap(), "c".repeat(40));
    }

    #[test]
    fn a_file_is_fetched_raw_and_returned_byte_for_byte() {
        let bytes = b"PyYAML==6.0.2 \\\n  --hash=sha256:aa\n\xff\xfe\n\n".to_vec();
        let (proc, _) = scripted(
            &[
                "gh",
                "api",
                "-H",
                "Accept: application/vnd.github.raw",
                &format!("repos/o/r/contents/docs/requirements.txt?ref={SHA}"),
            ],
            ProcOutput {
                exit: ProcExit::Code(0),
                stdout: bytes.clone(),
                stderr: b"noise".to_vec(),
            },
        );
        let path = PlainPath::parse("docs/requirements.txt").unwrap();
        assert_eq!(net(&proc).file_at("o/r", &path, &commit()).unwrap(), bytes);
    }

    #[test]
    fn standard_error_follows_standard_output_where_the_bash_merges_them() {
        let (proc, _) = scripted(
            &["gh", "api", "repos/actions/checkout/git/ref/heads/stable", "--jq", ".ref"],
            out(0, "refs/heads/stable\n", "a warning\n"),
        );
        assert_eq!(
            net(&proc).branch_ref(&repo(), "stable").unwrap(),
            "refs/heads/stable\na warning"
        );
    }

    // ----- strings that go into a path -----

    #[test]
    fn a_name_that_could_reach_another_ref_is_never_put_in_a_path() {
        let proc = FakeProc::new(&EffectLog::new());
        let net = net(&proc);
        for hostile in [
            "v4.2.2/../pull/1/head",
            "..",
            "../x",
            "v4/",
            "/v4",
            "v4//x",
            "v4?x=1",
            "v4#frag",
            "v4%2e%2e",
            "a b",
            "",
            ".hidden",
            "-x",
            "v4\n",
            "é",
            "v4\\x",
        ] {
            assert!(net.tag_ref(&repo(), hostile).is_err(), "tag {hostile:?}");
            assert!(net.branch_ref(&repo(), hostile).is_err(), "branch {hostile:?}");
            assert!(net.compare_status(&repo(), hostile, SHA).is_err(), "base {hostile:?}");
            assert!(net.merge_base("o/r", hostile, &commit()).is_err(), "base_ref {hostile:?}");
        }
    }

    #[test]
    fn a_comparison_head_that_is_not_a_hex_commit_is_never_put_in_a_path() {
        let proc = FakeProc::new(&EffectLog::new());
        for head in
            ["", "abc", "ABCDEF0123", "../x", "0123456789abcdef0123456789abcdef012345678", "main"]
        {
            assert!(net(&proc).compare_status(&repo(), "stable", head).is_err(), "{head:?}");
        }
    }

    #[test]
    fn a_tag_object_that_is_not_forty_hex_digits_is_never_put_in_a_path() {
        let proc = FakeProc::new(&EffectLog::new());
        for sha in [
            "",
            "abc",
            &"A".repeat(40),
            &"a".repeat(39),
            &"a".repeat(41),
            "../../pull/1/head",
            &"g".repeat(40),
        ] {
            assert!(net(&proc).tag_object(&repo(), sha).is_err(), "{sha:?}");
        }
    }

    #[test]
    fn a_repository_that_is_not_owner_slash_repo_is_never_put_in_a_path() {
        let proc = FakeProc::new(&EffectLog::new());
        let path = PlainPath::parse("requirements.txt").unwrap();
        for slug in
            ["", "o", "o/r/x", "/r", "o/", "o/..", "../r", "o/r?x", "o r/r", "o/r\n", "a..b/c"]
        {
            assert!(net(&proc).merge_base(slug, "main", &commit()).is_err(), "{slug:?}");
            assert!(net(&proc).file_at(slug, &path, &commit()).is_err(), "{slug:?}");
        }
    }

    #[test]
    fn slug_and_ref_rules_accept_what_real_names_look_like() {
        for slug in ["o/r", "agentsmith-tools/maintainer-agent", "a.b/c_d", "1/2"] {
            assert!(is_slug(slug), "{slug}");
        }
        for name in ["main", "v4.2.2", "release/1.x", "feature/a-b_c", "v1+build", "_x", "9"] {
            assert!(is_ref_name(name), "{name}");
        }
    }

    // ----- failures, timeouts and what a person reads -----

    #[test]
    fn a_failed_call_carries_everything_it_printed_and_trailing_newlines_are_dropped() {
        let (proc, _) = scripted(
            &["gh", "api", "repos/actions/checkout/git/ref/heads/stable", "--jq", ".ref"],
            out(1, "partial\n", "gh: Not Found (HTTP 404)\n\n"),
        );
        assert_eq!(
            net(&proc).branch_ref(&repo(), "stable").unwrap_err().output,
            "partial\ngh: Not Found (HTTP 404)"
        );
    }

    #[test]
    fn a_timeout_and_a_missing_program_are_failures_that_say_so() {
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        proc.script(
            &["gh", "api", "repos/actions/checkout/git/ref/heads/a", "--jq", ".ref"],
            Script::always(Ok(ProcOutput {
                exit: ProcExit::TimedOut,
                stdout: vec![],
                stderr: b"slow".to_vec(),
            })),
        );
        proc.script(
            &["gh", "api", "repos/actions/checkout/git/ref/heads/b", "--jq", ".ref"],
            Script::always(Err(ProcError::NotFound { program: "gh".into() })),
        );
        let net = net(&proc);
        assert_eq!(net.branch_ref(&repo(), "a").unwrap_err().output, "gh timed out\nslow");
        assert_eq!(
            net.branch_ref(&repo(), "b").unwrap_err().output,
            "gh: not found on the search path"
        );
    }

    #[test]
    fn what_a_registry_prints_cannot_move_the_cursor_or_recolour_a_refusal() {
        let (proc, _) = scripted(
            &["python3", "/s/resolve.py", "i", "t"],
            out(1, "\u{1b}[31mred\u{1b}[0m\tkept\r\x07\n", ""),
        );
        let failure = net(&proc).image_digest("i", "t").unwrap_err();
        assert_eq!(failure.output, "?[31mred?[0m\tkept??");
    }

    #[test]
    fn a_tab_separated_line_reads_as_the_shell_reads_it_with_a_tab_ifs() {
        assert_eq!(tab_fields::<3>("a\tb\tc"), ["a", "b", "c"].map(String::from));
        assert_eq!(
            tab_fields::<3>("a\t\tc"),
            ["a", "c", ""].map(String::from),
            "runs of tabs are one separator"
        );
        assert_eq!(tab_fields::<3>("\ta\tb\t"), ["a", "b", ""].map(String::from));
        assert_eq!(
            tab_fields::<3>("a\tb\tc\td"),
            ["a", "b", "c\td"].map(String::from),
            "the last name takes the rest"
        );
        assert_eq!(tab_fields::<3>("a"), ["a", "", ""].map(String::from));
        assert_eq!(tab_fields::<3>(""), ["", "", ""].map(String::from));
        assert_eq!(tab_fields::<2>("commit\tdeadbeef"), ["commit", "deadbeef"].map(String::from));
    }

    #[test]
    fn a_tag_answer_with_missing_fields_leaves_them_empty_for_the_flow_to_refuse() {
        let (proc, _) = scripted(
            &[
                "gh",
                "api",
                "repos/actions/checkout/git/ref/tags/v4",
                "--jq",
                "[.ref, .object.type, .object.sha] | @tsv",
            ],
            out(0, "refs/tags/v4\n", ""),
        );
        let tag = net(&proc).tag_ref(&repo(), "v4").unwrap();
        assert_eq!((tag.object_type.as_str(), tag.object_sha.as_str()), ("", ""));
    }

    // ----- the classifier, the whole-file check and the scratch directory -----

    #[test]
    fn the_classifier_gets_the_diff_on_standard_input_and_the_limit_as_its_timeout() {
        let (proc, log) =
            scripted(&["python3", "/s/classify.py"], out(1, "{\"verdict\": \"refused\"}\n", ""));
        let run = RealClassifier::new(&proc, "/s/classify.py".into())
            .classify("diff --git a/x b/x\n", Duration::from_secs(7))
            .unwrap();
        assert_eq!(
            run,
            ClassifierRun {
                exit: ClassifierExit::Code(1),
                stdout: "{\"verdict\": \"refused\"}\n".into()
            }
        );
        let entries = log.entries();
        let [Effect::Proc(ProcEffect::Run(spec))] = entries.as_slice() else { panic!("one call") };
        assert_eq!(spec.stdin.as_deref(), Some(&b"diff --git a/x b/x\n"[..]));
        assert_eq!(spec.timeout, Duration::from_secs(7));
    }

    #[test]
    fn a_classifier_killed_at_its_limit_is_a_timeout_and_one_that_cannot_start_is_a_failure() {
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        proc.script(
            &["python3", "/s/classify.py"],
            Script::sequence([
                Ok(ProcOutput { exit: ProcExit::TimedOut, stdout: vec![], stderr: vec![] }),
                Err(ProcError::NotFound { program: "python3".into() }),
            ]),
        );
        let classifier = RealClassifier::new(&proc, "/s/classify.py".into());
        assert_eq!(
            classifier.classify("d", Duration::from_secs(1)).unwrap().exit,
            ClassifierExit::TimedOut
        );
        assert!(classifier.classify("d", Duration::from_secs(1)).is_err());
    }

    #[test]
    fn a_scratch_directory_is_private_and_removed_when_it_goes() {
        use std::os::unix::fs::PermissionsExt;
        let (first, second) = (Scratch::new().unwrap(), Scratch::new().unwrap());
        assert_ne!(first.0, second.0);
        let file = first.put("base", b"x").unwrap();
        let mode = |path: &PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&first.0), 0o700);
        assert_eq!(mode(&file), 0o600);
        assert!(first.put("base", b"y").is_err(), "an existing name is never reopened");
        let dir = first.0.clone();
        drop(first);
        assert!(!dir.exists());
        assert!(second.0.exists());
    }

    fn repo_script(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../scripts").join(name)
    }

    fn real_proc() -> RealProc {
        RealProc::new("/usr/local/bin:/usr/bin:/bin", Vec::new())
    }

    fn pins(version: &str, hash: &str) -> Vec<u8> {
        format!("PyYAML=={version} \\\n    --hash=sha256:{hash}\n").into_bytes()
    }

    #[test]
    fn the_real_whole_file_helper_reads_two_files_and_lists_the_pin_that_moved() {
        let proc = real_proc();
        let whole = RealWholeFile::new(&proc, repo_script("verify_pip_files.py"));
        let path = PlainPath::parse("requirements.txt").unwrap();
        let answer = whole
            .check(&path, &pins("6.0.2", &"a".repeat(64)), &pins("6.0.3", &"b".repeat(64)))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(value["changed"][0]["name"], "PyYAML");
        assert_eq!(value["changed"][0]["version"], "6.0.3");
    }

    #[test]
    fn the_real_whole_file_helper_refuses_a_file_that_is_not_a_version_bump() {
        let proc = real_proc();
        let whole = RealWholeFile::new(&proc, repo_script("verify_pip_files.py"));
        let path = PlainPath::parse("requirements.txt").unwrap();
        let failure = whole
            .check(&path, &pins("6.0.2", &"a".repeat(64)), b"evil==1 \\\n    --hash=sha256:00\n")
            .unwrap_err();
        assert!(failure.output.starts_with("verify_pip_files:"), "{}", failure.output);
    }

    const CARGO_DIFF: &str = "diff --git a/Cargo.lock b/Cargo.lock\n--- a/Cargo.lock\n+++ b/Cargo.lock\n@@ -1,3 +1,3 @@\n [[package]]\n name = \"serde\"\n-version = \"1.0.1\"\n+version = \"1.0.2\"\n";

    #[test]
    fn the_real_classifier_reads_a_lockfile_diff_as_a_bump_and_a_source_diff_as_a_refusal() {
        let proc = real_proc();
        let classifier = RealClassifier::new(&proc, repo_script("classify_dependency_bump.py"));
        let run = classifier.classify(CARGO_DIFF, Duration::from_secs(30)).unwrap();
        assert_eq!(run.exit, ClassifierExit::Code(0), "{}", run.stdout);
        assert!(matches!(Verdict::parse(&run.stdout).unwrap(), Verdict::Bump(_)));
        let source = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-a\n+b\n";
        let run = classifier.classify(source, Duration::from_secs(30)).unwrap();
        assert_eq!(run.exit, ClassifierExit::Code(1));
        assert!(matches!(Verdict::parse(&run.stdout).unwrap(), Verdict::Refused(_)));
    }

    // ----- the three gh reads -----

    #[test]
    fn the_gh_reads_run_the_bash_commands() {
        let pr = PrNumber::parse("7").unwrap();
        let view = |field: &str, jq: &str| {
            ["gh", "pr", "view", "7", "--repo", "o/r", "--json", field, "--jq", jq]
                .map(String::from)
        };
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        let head = view("headRefOid", ".headRefOid");
        let base = view("baseRefName", ".baseRefName");
        proc.script(
            &head.each_ref().map(String::as_str),
            Script::always(Ok(out(0, &format!("{SHA}\n"), ""))),
        );
        proc.script(&base.each_ref().map(String::as_str), Script::always(Ok(out(0, "main\n", ""))));
        proc.script(
            &["gh", "pr", "diff", "7", "--repo", "o/r"],
            Script::always(Ok(out(0, "diff\n", "stderr noise"))),
        );
        let gh = ProcGh::new(&proc, "o/r");
        assert_eq!(gh.head(&pr).unwrap(), SHA);
        assert_eq!(gh.base_ref(&pr).unwrap(), "main");
        assert_eq!(gh.diff(&pr).unwrap(), "diff\n", "the diff is standard output alone");
    }

    #[test]
    fn a_failed_gh_read_keeps_the_exit_status_and_what_it_printed() {
        let (proc, _) = scripted(
            &["gh", "pr", "diff", "7", "--repo", "o/r"],
            out(4, "", "gh: not logged in\n"),
        );
        let failure = ProcGh::new(&proc, "o/r").diff(&PrNumber::parse("7").unwrap()).unwrap_err();
        assert_eq!((failure.exit, failure.output.as_str()), (4, "gh: not logged in"));
    }

    #[test]
    fn a_diff_that_is_not_utf8_is_not_read_so_the_classifier_never_sees_a_cleaned_copy() {
        let (proc, _) = scripted(
            &["gh", "pr", "diff", "7", "--repo", "o/r"],
            ProcOutput {
                exit: ProcExit::Code(0),
                stdout: b"diff \xff\xfe\n".to_vec(),
                stderr: vec![],
            },
        );
        let failure = ProcGh::new(&proc, "o/r").diff(&PrNumber::parse("7").unwrap()).unwrap_err();
        assert_eq!(failure.output, "the diff is not UTF-8, so it was not read");
    }

    #[test]
    fn a_diff_keeps_the_control_characters_the_classifier_refuses() {
        let text = "diff --git a/x b/x\n+a\rb\x0c\n";
        let (proc, _) = scripted(&["gh", "pr", "diff", "7", "--repo", "o/r"], out(0, text, ""));
        assert_eq!(ProcGh::new(&proc, "o/r").diff(&PrNumber::parse("7").unwrap()).unwrap(), text);
    }
}
