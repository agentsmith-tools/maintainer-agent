//! The suite host: how the gate asks a suite file a question.
//!
//! A suite is a bash file that defines functions (`suite_covers`, `suite_image`,
//! `suite_command`, ...). The bash gate sources it into the gate's own shell, so
//! a suite runs with the gate's privileges and a name that reaches the wrong file
//! sources arbitrary code (decision F3). The Rust gate never sources in process:
//! the real host (`RealSuiteHost`, the work of the suite agent) starts `bash`
//! through [`mx_ports::Proc`] for every question, one question per process, as a
//! subshell does in the bash (`( . "$file"; suite_covers "$path" )`). Three
//! rules bind that adapter, and the tests of a real host prove them:
//!
//! 1. The suite file and every argument reach bash as positional parameters. They
//!    are never spliced into the script text.
//! 2. A suite name is a [`SuiteName`], so the file is `<dir>/<name>.sh` and stays
//!    in `verify.d/`.
//! 3. A file that is missing is [`SuiteError::NotOnDisk`]. Sourcing a missing
//!    file leaves the previous suite's functions defined in a long-lived shell, so
//!    one typo would inherit another suite's coverage and read as an answer.

use crate::text::indent4;
use crate::{
    ImageRef, LimitKey, MutateGlob, Prefetch, Recorder, Runtime, SuiteName, SuiteSpec, TestFilter,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// A question to a suite or to the directory of suites failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SuiteError {
    /// No `verify.d` directory is reachable.
    // bash: die "this profile declares no verify.d/ suites; see docs/verifying.md"
    #[error("this profile declares no verify.d/ suites; see docs/verifying.md")]
    NoSuiteDir,
    /// There is no file `<dir>/<name>.sh`.
    // bash: die "no suite '$named' in $dir"
    #[error("no suite '{name}' in {}", dir.display())]
    NotOnDisk { name: SuiteName, dir: PathBuf },
    /// The suite needs a runtime the machine does not have.
    // bash: die "the '$suite' suite needs $(suite_requires_name), and this machine has $rt"
    #[error("the '{suite}' suite needs {needs}, and this machine has {runtime}")]
    NeedsOtherRuntime { suite: SuiteName, needs: String, runtime: Runtime },
    /// `suite_prefetch_guard` refused the extracted tree; `output` is what it printed.
    // bash: die "the dependency fetch was refused before it got a network:\n\n$(printf '%s' "$gmsg" | sed 's/^/    /')"
    #[error("the dependency fetch was refused before it got a network:\n\n{}", indent4(output))]
    PrefetchRefused { output: String },
    /// `bash` could not be started or did not finish, so the question was not answered.
    #[error("bash could not answer for the suite: {reason}")]
    Bash { reason: String },
}

/// One suite, loaded. Each method is one question to the suite's bash functions.
///
/// A method that returns `Err` means the question was not answered. The bash
/// gate reads a non-zero status of `suite_covers` as "no" ([`Suite::covers`]
/// keeps that), and every other `Err` stops the caller: a guard that could not
/// ask has not said yes.
pub trait Suite {
    /// What the file declares without an argument.
    fn spec(&self) -> &SuiteSpec;

    /// `suite_covers <path>`: does this suite run the code at `path`? Status 0 is
    /// `Ok(true)`. Any other status, including a function that does not exist or
    /// a syntax error, is `Ok(false)`, as in the bash (`( . "$s"; suite_covers
    /// "$p" ) && ...`): the refusal that follows is the safe direction.
    fn covers(&self, path: &str) -> Result<bool, SuiteError>;

    /// `suite_command <filter>`: the text the container runs as `sh -c`, with every trailing
    /// newline removed as `$(...)` removes it (the shell suite prints `bash %s\n`), so the
    /// container is never handed a command that ends in a newline. The filter is a
    /// [`TestFilter`], which cannot hold shell syntax (decision P30-9).
    fn command(&self, filter: &TestFilter) -> Result<String, SuiteError>;

    /// `suite_ran <log>`: how many test units the log shows. Output that is not
    /// a whole number counts as 0, so the caller refuses it as "no evidence it
    /// ran anything" (`[ "${ran:-0}" -ge 1 ]`).
    fn ran(&self, log: &Path) -> Result<u64, SuiteError>;

    /// `suite_extra_args <runtime> <scratch>`: the words `suite_podman_args`
    /// prints, or none when the suite does not define it. The bash splits them
    /// unquoted, on blanks and newlines (`mx_sandbox::ExtraArgs::split` pins
    /// that). The function may create directories under `scratch` (the rust
    /// suite does), so asking is not free of effects.
    fn extra_args(&self, runtime: Runtime, scratch: &Path) -> Result<String, SuiteError>;

    /// `suite_requires <runtime>`: `Ok` when the suite runs on this runtime or
    /// does not say; `NeedsOtherRuntime` with `suite_requires_name` otherwise.
    fn requires(&self, runtime: Runtime) -> Result<(), SuiteError>;

    /// One of `suite_memory`, `suite_cpus`, `suite_pids`, `suite_tmpfs`: what the
    /// function prints, or `None` when the suite does not define it.
    fn limit(&self, key: LimitKey) -> Result<Option<String>, SuiteError>;

    /// `suite_prefetch_guard <tree>`: runs on the host before the fetch
    /// container gets a network. `Ok` when the guard passes or the suite has
    /// none; `PrefetchRefused` with what it printed otherwise.
    fn prefetch_guard(&self, tree: &Path) -> Result<(), SuiteError>;
}

/// The directory of suites of a profile.
pub trait SuiteHost {
    /// `suite_dir`: the first of `$PROFILE_DIR/verify.d`,
    /// `~/.local/share/maintainer/profiles/<profile>/verify.d` and
    /// `<tree>/../profiles/<profile>/verify.d` that is a directory.
    fn dir(&self) -> Result<PathBuf, SuiteError>;

    /// The suites in `dir()`, one per `*.sh` file, in the order a glob lists
    /// them. The real host lists in the byte order of file names (`LC_ALL=C`),
    /// because inference takes the first suite that covers every path and
    /// bash's own glob order depends on the locale.
    fn names(&self) -> Result<Vec<SuiteName>, SuiteError>;

    /// Reads one suite's declarations. A file that is not there is `NotOnDisk`.
    fn load(&self, name: &SuiteName) -> Result<Box<dyn Suite>, SuiteError>;

    /// `( . "$dir/$name.sh"; suite_covers "$path" )`, without reading the other declarations.
    /// A suite whose declarations cannot be read (no `suite_image`) still answers this in
    /// the bash, because coverage never asks for them; the real host answers it the same
    /// way. The default asks the loaded suite, which suits a host that loads from memory.
    /// Added by the suite agent (P3A-2); additive.
    fn covers(&self, name: &SuiteName, path: &str) -> Result<bool, SuiteError> {
        self.load(name)?.covers(path)
    }

    /// `suite_inferable`: the file does not define `suite_named_only`. Same reasoning as
    /// [`SuiteHost::covers`] for the default. Added by the suite agent (P3A-2); additive.
    fn inferable(&self, name: &SuiteName) -> Result<bool, SuiteError> {
        Ok(!self.load(name)?.spec().named_only)
    }

    /// Tells the host where the verify run extracted the pull request. The bash gate keeps the
    /// directory in `work`, a local of `cmd_verify` that every suite function can read, and the
    /// characterization suite reads it to find its case files. A host that has no such variable
    /// ignores the call. Added by the wiring lane (P3W-2); additive.
    fn set_work(&self, _work: &Path) {}
}

/// One call to a fake host or to a suite it loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuiteCall {
    Dir,
    Names,
    Load(SuiteName),
    Covers { suite: SuiteName, path: String },
    Command { suite: SuiteName, filter: String },
    Ran { suite: SuiteName, log: PathBuf },
    ExtraArgs { suite: SuiteName, runtime: Runtime, scratch: PathBuf },
    Requires { suite: SuiteName, runtime: Runtime },
    Limit { suite: SuiteName, key: LimitKey },
    PrefetchGuard { suite: SuiteName, tree: PathBuf },
    SetWork(PathBuf),
}

/// A suite with scripted answers.
#[derive(Debug, Clone)]
pub struct FakeSuite {
    spec: SuiteSpec,
    covered: BTreeSet<String>,
    command_template: String,
    ran: u64,
    extra_args: String,
    requires: Option<(Runtime, String)>,
    limits: Vec<(LimitKey, String)>,
    guard_refusal: Option<String>,
    calls: Recorder<SuiteCall>,
}

impl FakeSuite {
    /// A suite called `name` in `image`. It covers nothing, mutates nothing and
    /// runs `true`.
    ///
    /// # Panics
    ///
    /// Panics when `name` or `image` is not valid: a fixture with a bad name is a
    /// bug in the test.
    #[must_use]
    pub fn new(name: &str, image: &str) -> Self {
        Self {
            spec: SuiteSpec {
                name: SuiteName::parse(name).expect("a fixture suite name"),
                image: ImageRef::parse(image).expect("a fixture image"),
                needs: Vec::new(),
                mutate_globs: Vec::new(),
                named_only: false,
                prefetch: None,
            },
            covered: BTreeSet::new(),
            command_template: "true".to_string(),
            ran: 0,
            extra_args: String::new(),
            requires: None,
            limits: Vec::new(),
            guard_refusal: None,
            calls: Recorder::new(),
        }
    }

    /// The paths `suite_covers` answers yes to. Every other path is a no.
    #[must_use]
    pub fn covering(mut self, paths: &[&str]) -> Self {
        self.covered = paths.iter().map(|p| (*p).to_string()).collect();
        self
    }

    /// `suite_mutate_glob`.
    ///
    /// # Panics
    ///
    /// Panics on an empty glob.
    #[must_use]
    pub fn mutating(mut self, globs: &[&str]) -> Self {
        self.spec.mutate_globs =
            globs.iter().map(|g| MutateGlob::parse(g).expect("a fixture glob")).collect();
        self
    }

    /// `suite_needs`, as words.
    #[must_use]
    pub fn needing(mut self, words: &[&str]) -> Self {
        self.spec.needs = words.iter().map(|w| (*w).to_string()).collect();
        self
    }

    /// Defines `suite_named_only`.
    #[must_use]
    pub fn named_only(mut self) -> Self {
        self.spec.named_only = true;
        self
    }

    /// `suite_command`: the text with `{filter}` replaced by the filter.
    #[must_use]
    pub fn command_template(mut self, template: &str) -> Self {
        self.command_template = template.to_string();
        self
    }

    /// What `suite_ran` answers.
    #[must_use]
    pub fn ran(mut self, units: u64) -> Self {
        self.ran = units;
        self
    }

    /// What `suite_podman_args` prints.
    #[must_use]
    pub fn extra_args(mut self, printed: &str) -> Self {
        self.extra_args = printed.to_string();
        self
    }

    /// The suite runs only on `runtime`, and `needs_name` is its
    /// `suite_requires_name`.
    #[must_use]
    pub fn requiring(mut self, runtime: Runtime, needs_name: &str) -> Self {
        self.requires = Some((runtime, needs_name.to_string()));
        self
    }

    /// Defines one limit function.
    #[must_use]
    pub fn limit(mut self, key: LimitKey, printed: &str) -> Self {
        self.limits.retain(|(k, _)| *k != key);
        self.limits.push((key, printed.to_string()));
        self
    }

    /// Defines `suite_prefetch_command`, with a guard that passes.
    #[must_use]
    pub fn prefetching(mut self, command: &str) -> Self {
        self.spec.prefetch = Some(Prefetch { command: command.to_string(), guarded: true });
        self
    }

    /// The guard refuses, printing `output`.
    #[must_use]
    pub fn guard_refuses(mut self, output: &str) -> Self {
        self.guard_refusal = Some(output.to_string());
        self
    }

    fn name(&self) -> SuiteName {
        self.spec.name.clone()
    }
}

impl Suite for FakeSuite {
    fn spec(&self) -> &SuiteSpec {
        &self.spec
    }

    fn covers(&self, path: &str) -> Result<bool, SuiteError> {
        self.calls.push(SuiteCall::Covers { suite: self.name(), path: path.to_string() });
        Ok(self.covered.contains(path))
    }

    fn command(&self, filter: &TestFilter) -> Result<String, SuiteError> {
        self.calls.push(SuiteCall::Command { suite: self.name(), filter: filter.as_str().into() });
        Ok(self.command_template.replace("{filter}", filter.as_str()))
    }

    fn ran(&self, log: &Path) -> Result<u64, SuiteError> {
        self.calls.push(SuiteCall::Ran { suite: self.name(), log: log.to_path_buf() });
        Ok(self.ran)
    }

    fn extra_args(&self, runtime: Runtime, scratch: &Path) -> Result<String, SuiteError> {
        self.calls.push(SuiteCall::ExtraArgs {
            suite: self.name(),
            runtime,
            scratch: scratch.to_path_buf(),
        });
        Ok(self.extra_args.clone())
    }

    fn requires(&self, runtime: Runtime) -> Result<(), SuiteError> {
        self.calls.push(SuiteCall::Requires { suite: self.name(), runtime });
        match &self.requires {
            Some((needed, name)) if *needed != runtime => Err(SuiteError::NeedsOtherRuntime {
                suite: self.name(),
                needs: name.clone(),
                runtime,
            }),
            _ => Ok(()),
        }
    }

    fn limit(&self, key: LimitKey) -> Result<Option<String>, SuiteError> {
        self.calls.push(SuiteCall::Limit { suite: self.name(), key });
        Ok(self.limits.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()))
    }

    fn prefetch_guard(&self, tree: &Path) -> Result<(), SuiteError> {
        self.calls.push(SuiteCall::PrefetchGuard { suite: self.name(), tree: tree.to_path_buf() });
        match &self.guard_refusal {
            Some(output) => Err(SuiteError::PrefetchRefused { output: output.clone() }),
            None => Ok(()),
        }
    }
}

/// A directory of [`FakeSuite`]s. Every call, on the host or on a suite it
/// loaded, lands in one record.
#[derive(Debug)]
pub struct FakeSuiteHost {
    dir: Option<PathBuf>,
    suites: Vec<FakeSuite>,
    calls: Recorder<SuiteCall>,
}

impl FakeSuiteHost {
    /// A host whose `verify.d` is `dir` and which holds `suites`.
    #[must_use]
    pub fn new(dir: &str, suites: Vec<FakeSuite>) -> Self {
        let calls = Recorder::new();
        let suites = suites
            .into_iter()
            .map(|mut s| {
                s.calls = calls.clone();
                s
            })
            .collect();
        Self { dir: Some(PathBuf::from(dir)), suites, calls }
    }

    /// A host with no `verify.d` at all: `dir` and `names` answer `NoSuiteDir`.
    #[must_use]
    pub fn without_dir() -> Self {
        Self { dir: None, suites: Vec::new(), calls: Recorder::new() }
    }

    /// Every call so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<SuiteCall> {
        self.calls.entries()
    }
}

impl SuiteHost for FakeSuiteHost {
    fn dir(&self) -> Result<PathBuf, SuiteError> {
        self.calls.push(SuiteCall::Dir);
        self.dir.clone().ok_or(SuiteError::NoSuiteDir)
    }

    fn names(&self) -> Result<Vec<SuiteName>, SuiteError> {
        self.calls.push(SuiteCall::Names);
        if self.dir.is_none() {
            return Err(SuiteError::NoSuiteDir);
        }
        let mut names: Vec<SuiteName> = self.suites.iter().map(FakeSuite::name).collect();
        names.sort();
        Ok(names)
    }

    fn load(&self, name: &SuiteName) -> Result<Box<dyn Suite>, SuiteError> {
        self.calls.push(SuiteCall::Load(name.clone()));
        let Some(dir) = &self.dir else { return Err(SuiteError::NoSuiteDir) };
        match self.suites.iter().find(|s| &s.spec.name == name) {
            Some(suite) => Ok(Box::new(suite.clone())),
            None => Err(SuiteError::NotOnDisk { name: name.clone(), dir: dir.clone() }),
        }
    }

    fn set_work(&self, work: &Path) {
        self.calls.push(SuiteCall::SetWork(work.to_path_buf()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = "/profiles/sysknife/verify.d";

    fn rust() -> FakeSuite {
        FakeSuite::new("rust", "docker.io/library/rust:1-slim")
            .covering(&["src/lib.rs", "Cargo.toml"])
            .mutating(&["*.rs"])
            .needing(&["bash", "cargo", "rustc"])
            .command_template("cargo test --offline --workspace {filter}")
            .ran(12)
            .extra_args("-v\n/home/u/.cargo:/cargo:O")
            .requiring(Runtime::Podman, "podman (for the :O overlay mount on ~/.cargo)")
            .limit(LimitKey::Memory, "6g")
            .prefetching("CARGO_NET_OFFLINE=false cargo fetch --locked")
    }

    fn docs() -> FakeSuite {
        FakeSuite::new("docs", "docker.io/library/python:3.12-slim")
            .covering(&["README.md"])
            .mutating(&["*.md"])
            .named_only()
    }

    fn host() -> FakeSuiteHost {
        FakeSuiteHost::new(DIR, vec![rust(), docs()])
    }

    fn name(text: &str) -> SuiteName {
        SuiteName::parse(text).unwrap()
    }

    #[test]
    fn a_loaded_suite_answers_covers_by_its_table_and_nothing_else() {
        let host = host();
        let suite = host.load(&name("rust")).unwrap();
        assert_eq!(suite.covers("src/lib.rs"), Ok(true));
        assert_eq!(suite.covers("README.md"), Ok(false));
        assert_eq!(suite.covers("src/lib.rs "), Ok(false), "a path matches exactly");
    }

    #[test]
    fn a_loaded_suite_hands_back_its_declarations() {
        let host = host();
        let spec = host.load(&name("rust")).unwrap().spec().clone();
        assert_eq!(spec.name.as_str(), "rust");
        assert_eq!(spec.image.as_str(), "docker.io/library/rust:1-slim");
        assert_eq!(spec.needs, ["bash", "cargo", "rustc"]);
        assert!(!spec.named_only);
        assert_eq!(spec.mutate_globs().unwrap().first().as_str(), "*.rs");
        assert_eq!(
            spec.prefetch,
            Some(Prefetch {
                command: "CARGO_NET_OFFLINE=false cargo fetch --locked".into(),
                guarded: true
            })
        );
        assert!(host.load(&name("docs")).unwrap().spec().named_only);
    }

    #[test]
    fn command_puts_the_filter_where_the_suite_puts_it() {
        let host = host();
        let suite = host.load(&name("rust")).unwrap();
        let filter = TestFilter::parse("guard_bites").unwrap();
        assert_eq!(suite.command(&filter).unwrap(), "cargo test --offline --workspace guard_bites");
    }

    #[test]
    fn ran_extra_args_and_limits_are_scripted() {
        let host = host();
        let suite = host.load(&name("rust")).unwrap();
        assert_eq!(suite.ran(Path::new("/w/clean.log")), Ok(12));
        assert_eq!(
            suite.extra_args(Runtime::Podman, Path::new("/s")),
            Ok("-v\n/home/u/.cargo:/cargo:O".to_string())
        );
        assert_eq!(suite.limit(LimitKey::Memory), Ok(Some("6g".to_string())));
        assert_eq!(suite.limit(LimitKey::Cpus), Ok(None), "an undefined limit is None");
    }

    #[test]
    fn a_suite_that_needs_podman_refuses_docker_with_the_bash_sentence() {
        let host = host();
        let suite = host.load(&name("rust")).unwrap();
        assert_eq!(suite.requires(Runtime::Podman), Ok(()));
        let refusal = suite.requires(Runtime::Docker).unwrap_err();
        assert_eq!(
            refusal.to_string(),
            "the 'rust' suite needs podman (for the :O overlay mount on ~/.cargo), and this machine has docker"
        );
        assert_eq!(host.load(&name("docs")).unwrap().requires(Runtime::Docker), Ok(()));
    }

    #[test]
    fn the_prefetch_guard_passes_until_a_test_makes_it_refuse() {
        let host = host();
        assert_eq!(host.load(&name("rust")).unwrap().prefetch_guard(Path::new("/w")), Ok(()));
        let strict =
            FakeSuiteHost::new(DIR, vec![rust().guard_refuses("no Cargo.lock\nin the tree")]);
        let refusal =
            strict.load(&name("rust")).unwrap().prefetch_guard(Path::new("/w")).unwrap_err();
        assert_eq!(
            refusal.to_string(),
            "the dependency fetch was refused before it got a network:\n\n    no Cargo.lock\n    in the tree"
        );
    }

    #[test]
    fn names_come_back_in_byte_order_whatever_order_the_suites_went_in() {
        let host = FakeSuiteHost::new(DIR, vec![rust(), FakeSuite::new("Zed", "x"), docs()]);
        let names: Vec<String> = host.names().unwrap().iter().map(ToString::to_string).collect();
        assert_eq!(names, ["Zed", "docs", "rust"]);
    }

    #[test]
    fn a_suite_that_is_not_on_disk_is_refused_with_the_directory() {
        let host = host();
        let refusal = host.load(&name("python")).err().unwrap();
        assert_eq!(
            refusal,
            SuiteError::NotOnDisk { name: name("python"), dir: PathBuf::from(DIR) }
        );
        assert_eq!(refusal.to_string(), "no suite 'python' in /profiles/sysknife/verify.d");
    }

    #[test]
    fn a_host_without_verify_d_answers_no_suite_dir_everywhere() {
        let host = FakeSuiteHost::without_dir();
        assert_eq!(host.dir(), Err(SuiteError::NoSuiteDir));
        assert_eq!(host.names(), Err(SuiteError::NoSuiteDir));
        assert_eq!(host.load(&name("rust")).err(), Some(SuiteError::NoSuiteDir));
        assert_eq!(
            SuiteError::NoSuiteDir.to_string(),
            "this profile declares no verify.d/ suites; see docs/verifying.md"
        );
    }

    #[test]
    fn every_call_to_the_host_and_to_a_loaded_suite_is_recorded_in_order() {
        let host = host();
        assert_eq!(host.dir().unwrap(), PathBuf::from(DIR));
        let suite = host.load(&name("rust")).unwrap();
        suite.covers("a.rs").unwrap();
        suite.command(&TestFilter::parse("f").unwrap()).unwrap();
        suite.ran(Path::new("/log")).unwrap();
        suite.extra_args(Runtime::Docker, Path::new("/scratch")).unwrap();
        suite.requires(Runtime::Podman).unwrap();
        suite.limit(LimitKey::Pids).unwrap();
        suite.prefetch_guard(Path::new("/tree")).unwrap();
        host.names().unwrap();
        let rust = name("rust");
        assert_eq!(
            host.calls(),
            vec![
                SuiteCall::Dir,
                SuiteCall::Load(rust.clone()),
                SuiteCall::Covers { suite: rust.clone(), path: "a.rs".into() },
                SuiteCall::Command { suite: rust.clone(), filter: "f".into() },
                SuiteCall::Ran { suite: rust.clone(), log: PathBuf::from("/log") },
                SuiteCall::ExtraArgs {
                    suite: rust.clone(),
                    runtime: Runtime::Docker,
                    scratch: PathBuf::from("/scratch")
                },
                SuiteCall::Requires { suite: rust.clone(), runtime: Runtime::Podman },
                SuiteCall::Limit { suite: rust.clone(), key: LimitKey::Pids },
                SuiteCall::PrefetchGuard { suite: rust, tree: PathBuf::from("/tree") },
                SuiteCall::Names,
            ]
        );
    }

    #[test]
    fn a_refused_load_is_recorded_too() {
        let host = host();
        drop(host.load(&name("nope")));
        assert_eq!(host.calls(), vec![SuiteCall::Load(name("nope"))]);
    }
}
