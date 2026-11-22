//! The sandbox port and its fake.

use crate::{
    ContainedTree, ContainerSpec, Escape, ExtractedTree, LogMode, Mutated, SandboxError,
    SedExpression,
};
use mx_core::NonEmpty;
use mx_ports::Script;
use mx_suite::{MutateGlob, Recorder};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// How a container run ended: the exit status of the runtime's command, which is
/// the container's own. A death by signal reads as `128 + n` (137 and 143 are
/// what `--memory` and `--timeout` look like).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerExit(pub i32);

impl ContainerExit {
    /// The status.
    #[must_use]
    pub fn code(self) -> i32 {
        self.0
    }
}

/// What the gate does to a pull request's files on the host and in a container.
///
/// The real adapter (the suite agent's `RealSandbox`) starts `git`, `find`,
/// `sed`, `sha256sum` and the runtime through [`mx_ports::Proc`] or `Pipe`, and
/// is the only code that does. This crate starts no process. Each method is one
/// bash function of `bin/maintainer-merge`:
///
/// | method | bash |
/// |---|---|
/// | `extract_tree` | `extract_tree` |
/// | `tree_escapes` | `tree_escapes` |
/// | `apply_mutation` | `apply_mutation` |
/// | `run_container` | `run_in_container` and the prefetch `podman run` |
pub trait Sandbox {
    /// Writes the files `rev` tracks into the empty directory `dest`, with
    /// `git read-tree` into a private index and `git checkout-index -a`, under
    /// `core.hooksPath=/dev/null`. The result counts them
    /// ([`ExtractedTree::counted`]).
    fn extract_tree(
        &self,
        repo: &Path,
        rev: &str,
        dest: &Path,
    ) -> Result<ExtractedTree, SandboxError>;

    /// The symlinks of the tree whose target resolves outside it, sorted, as
    /// `link -> target`. `os.walk` does not follow links; `realpath` resolves
    /// the whole chain and normalises `..` without touching the disk, so a
    /// dangling link inside the tree reads as inside.
    fn tree_escapes(&self, tree: &ExtractedTree) -> Result<Vec<Escape>, SandboxError>;

    /// `find . -type f \( -name g1 -o -name g2 \) -not -path './.container-target/*'
    /// -not -path './.git/*'`, then `sed --sandbox -i <expression>` over the
    /// result, and the paths whose sha256 changed, `./` stripped. `-type f`
    /// and never `-L`: `sed -i` on a symlink replaces the link with the content
    /// it points at. Only a [`ContainedTree`] reaches it.
    fn apply_mutation(
        &self,
        tree: &ContainedTree,
        globs: &NonEmpty<MutateGlob>,
        expression: &SedExpression,
    ) -> Result<Mutated, SandboxError>;

    /// A new private directory under `parent` (`scratch_dir`): `mkdir -p`, mode 700, then
    /// `mktemp -d <parent>/XXXXXXXX`. The adapter removes every directory it made when the
    /// gate ends, on every exit and not only on success (the bash does it from one `EXIT` trap).
    fn scratch_dir(&self, parent: &Path) -> Result<PathBuf, SandboxError>;

    /// `git init -q` and `git add -A -f` inside the extracted tree, under
    /// `-c core.hooksPath=/dev/null -c init.templateDir=`, so `git ls-files` answers what the
    /// archive held. `Err` is a note and not a refusal: the gate says so and goes on.
    fn seed_index(&self, tree: &ContainedTree) -> Result<(), SandboxError>;

    /// Runs the container, its output going to `log` as the spec's
    /// [`ContainerSpec::log_mode`] says (`>>` or `>`, with standard error on
    /// standard output). The runtime's own `--timeout` ends a run that never
    /// finishes; a GNU `timeout` around the client would not.
    fn run_container(
        &self,
        spec: &ContainerSpec,
        log: &Path,
    ) -> Result<ContainerExit, SandboxError>;
}

/// One call to a [`FakeSandbox`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxCall {
    Extract { repo: PathBuf, rev: String, dest: PathBuf },
    Escapes { root: PathBuf },
    Scratch { parent: PathBuf },
    SeedIndex { root: PathBuf },
    Mutate { root: PathBuf, globs: Vec<String>, expression: String },
    Run { argv: Vec<String>, log: PathBuf, mode: LogMode },
}

/// A scripted [`Sandbox`]. It starts nothing, reads nothing and records every
/// call. A call nobody scripted panics, as the other fakes do: a stub with no arm
/// for a call the code makes returns nothing, and the guard under test skips in
/// silence.
#[derive(Debug, Default)]
pub struct FakeSandbox {
    calls: Recorder<SandboxCall>,
    extract: Mutex<Script<Result<ExtractedTree, SandboxError>>>,
    escapes: Mutex<Script<Result<Vec<Escape>, SandboxError>>>,
    mutate: Mutex<Script<Result<Vec<String>, SandboxError>>>,
    run: Mutex<Script<Result<ContainerExit, SandboxError>>>,
    scratch: Mutex<Script<Result<PathBuf, SandboxError>>>,
    seed: Mutex<Script<Result<(), SandboxError>>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl FakeSandbox {
    /// A fake with no scripted answers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The answers to `extract_tree`.
    pub fn script_extract(&self, script: Script<Result<ExtractedTree, SandboxError>>) {
        *locked(&self.extract) = script;
    }

    /// The answers to `tree_escapes`.
    pub fn script_escapes(&self, script: Script<Result<Vec<Escape>, SandboxError>>) {
        *locked(&self.escapes) = script;
    }

    /// The answers to `apply_mutation`: the paths that changed.
    pub fn script_mutation(&self, script: Script<Result<Vec<String>, SandboxError>>) {
        *locked(&self.mutate) = script;
    }

    /// The answers to `run_container`.
    pub fn script_run(&self, script: Script<Result<ContainerExit, SandboxError>>) {
        *locked(&self.run) = script;
    }

    /// The answers to `scratch_dir`.
    pub fn script_scratch(&self, script: Script<Result<PathBuf, SandboxError>>) {
        *locked(&self.scratch) = script;
    }

    /// The answers to `seed_index`.
    pub fn script_seed(&self, script: Script<Result<(), SandboxError>>) {
        *locked(&self.seed) = script;
    }

    /// Every call so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<SandboxCall> {
        self.calls.entries()
    }
}

impl Sandbox for FakeSandbox {
    fn extract_tree(
        &self,
        repo: &Path,
        rev: &str,
        dest: &Path,
    ) -> Result<ExtractedTree, SandboxError> {
        self.calls.push(SandboxCall::Extract {
            repo: repo.to_path_buf(),
            rev: rev.to_string(),
            dest: dest.to_path_buf(),
        });
        locked(&self.extract).next("Sandbox::extract_tree")
    }

    fn tree_escapes(&self, tree: &ExtractedTree) -> Result<Vec<Escape>, SandboxError> {
        self.calls.push(SandboxCall::Escapes { root: tree.root().to_path_buf() });
        locked(&self.escapes).next("Sandbox::tree_escapes")
    }

    fn apply_mutation(
        &self,
        tree: &ContainedTree,
        globs: &NonEmpty<MutateGlob>,
        expression: &SedExpression,
    ) -> Result<Mutated, SandboxError> {
        self.calls.push(SandboxCall::Mutate {
            root: tree.root().to_path_buf(),
            globs: globs.iter().map(|g| g.as_str().to_string()).collect(),
            expression: expression.as_str().to_string(),
        });
        let paths = locked(&self.mutate).next("Sandbox::apply_mutation")?;
        let globs: Vec<MutateGlob> = globs.iter().cloned().collect();
        Mutated::new(paths, &globs)
            .map_err(|nothing| SandboxError::NothingChanged { globs: nothing.globs })
    }

    fn scratch_dir(&self, parent: &Path) -> Result<PathBuf, SandboxError> {
        self.calls.push(SandboxCall::Scratch { parent: parent.to_path_buf() });
        locked(&self.scratch).next("Sandbox::scratch_dir")
    }

    fn seed_index(&self, tree: &ContainedTree) -> Result<(), SandboxError> {
        self.calls.push(SandboxCall::SeedIndex { root: tree.root().to_path_buf() });
        locked(&self.seed).next("Sandbox::seed_index")
    }

    fn run_container(
        &self,
        spec: &ContainerSpec,
        log: &Path,
    ) -> Result<ContainerExit, SandboxError> {
        self.calls.push(SandboxCall::Run {
            argv: spec.argv().into_vec(),
            log: log.to_path_buf(),
            mode: spec.log_mode(),
        });
        locked(&self.run).next("Sandbox::run_container")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContainerInputs, ExtraArgs, Limit, Limits, Timeout};
    use mx_suite::{ImageRef, LimitKey, Runtime};

    fn tree() -> ExtractedTree {
        ExtractedTree::counted(PathBuf::from("/w"), "v-7", 2, 2).unwrap()
    }

    fn spec() -> ContainerSpec {
        ContainerSpec::verify(ContainerInputs {
            runtime: Runtime::Podman,
            work: PathBuf::from("/w"),
            image: ImageRef::parse("img:1").unwrap(),
            command: "true".into(),
            extra_args: ExtraArgs::default(),
            timeout: Timeout::from_secs(60).unwrap(),
            limits: Limits {
                tmpfs: Limit::parse(LimitKey::Tmpfs, "64m").unwrap(),
                pids: Limit::parse(LimitKey::Pids, "64").unwrap(),
                memory: Limit::parse(LimitKey::Memory, "1g").unwrap(),
                cpus: Limit::parse(LimitKey::Cpus, "1").unwrap(),
            },
        })
        .unwrap()
    }

    #[test]
    fn a_scripted_extraction_is_returned_and_the_call_is_recorded() {
        let fake = FakeSandbox::new();
        fake.script_extract(Script::always(Ok(tree())));
        let got = fake.extract_tree(Path::new("/repo"), "v-7", Path::new("/w")).unwrap();
        assert_eq!(got.root(), Path::new("/w"));
        assert_eq!(
            fake.calls(),
            vec![SandboxCall::Extract {
                repo: PathBuf::from("/repo"),
                rev: "v-7".into(),
                dest: PathBuf::from("/w")
            }]
        );
    }

    #[test]
    fn a_scripted_failure_is_returned_as_scripted() {
        let fake = FakeSandbox::new();
        fake.script_extract(Script::always(Err(SandboxError::ReadTree { rev: "v-7".into() })));
        assert_eq!(
            fake.extract_tree(Path::new("/r"), "v-7", Path::new("/w")),
            Err(SandboxError::ReadTree { rev: "v-7".into() })
        );
    }

    #[test]
    fn escapes_are_scripted_and_the_tree_root_is_recorded() {
        let fake = FakeSandbox::new();
        let found = vec![Escape { link: "a".into(), target: "/etc/passwd".into() }];
        fake.script_escapes(Script::always(Ok(found.clone())));
        assert_eq!(fake.tree_escapes(&tree()), Ok(found));
        assert_eq!(fake.calls(), vec![SandboxCall::Escapes { root: PathBuf::from("/w") }]);
    }

    #[test]
    fn a_mutation_records_the_globs_and_the_expression_it_was_given() {
        let fake = FakeSandbox::new();
        fake.script_mutation(Script::always(Ok(vec!["src/a.rs".into()])));
        let contained = ContainedTree::check(tree(), Vec::new()).unwrap();
        let globs = NonEmpty::new(
            MutateGlob::parse("*.rs").unwrap(),
            [MutateGlob::parse("Makefile").unwrap()],
        );
        let changed = fake
            .apply_mutation(&contained, &globs, &SedExpression::parse("s/a/b/").unwrap())
            .unwrap();
        assert_eq!(changed.paths(), ["src/a.rs"]);
        assert_eq!(
            fake.calls(),
            vec![SandboxCall::Mutate {
                root: PathBuf::from("/w"),
                globs: vec!["*.rs".into(), "Makefile".into()],
                expression: "s/a/b/".into()
            }]
        );
    }

    #[test]
    fn a_mutation_that_changed_nothing_cannot_be_used_unchecked() {
        let fake = FakeSandbox::new();
        fake.script_mutation(Script::always(Ok(Vec::new())));
        let contained = ContainedTree::check(tree(), Vec::new()).unwrap();
        let globs = NonEmpty::new(MutateGlob::parse("*.rs").unwrap(), []);
        assert_eq!(
            fake.apply_mutation(&contained, &globs, &SedExpression::parse("s/a/b/").unwrap()),
            Err(SandboxError::NothingChanged { globs: vec!["*.rs".into()] })
        );
    }

    #[test]
    fn a_container_run_records_the_whole_command_line_the_log_and_the_mode() {
        let fake = FakeSandbox::new();
        fake.script_run(Script::sequence([Ok(ContainerExit(0)), Ok(ContainerExit(101))]));
        let spec = spec();
        assert_eq!(fake.run_container(&spec, Path::new("/w/clean.log")), Ok(ContainerExit(0)));
        assert_eq!(fake.run_container(&spec, Path::new("/w/mutated.log")), Ok(ContainerExit(101)));
        let calls = fake.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[1],
            SandboxCall::Run {
                argv: spec.argv().into_vec(),
                log: PathBuf::from("/w/mutated.log"),
                mode: LogMode::Append
            }
        );
    }

    #[test]
    fn calls_to_different_methods_are_recorded_in_call_order() {
        let fake = FakeSandbox::new();
        fake.script_extract(Script::always(Ok(tree())));
        fake.script_escapes(Script::always(Ok(Vec::new())));
        fake.script_run(Script::always(Ok(ContainerExit(0))));
        let extracted = fake.extract_tree(Path::new("/r"), "v-7", Path::new("/w")).unwrap();
        fake.tree_escapes(&extracted).unwrap();
        fake.run_container(&spec(), Path::new("/w/clean.log")).unwrap();
        let kinds: Vec<&str> = fake
            .calls()
            .iter()
            .map(|c| match c {
                SandboxCall::Extract { .. } => "extract",
                SandboxCall::Escapes { .. } => "escapes",
                SandboxCall::Scratch { .. } => "scratch",
                SandboxCall::SeedIndex { .. } => "seed",
                SandboxCall::Mutate { .. } => "mutate",
                SandboxCall::Run { .. } => "run",
            })
            .collect();
        assert_eq!(kinds, ["extract", "escapes", "run"]);
    }

    #[test]
    #[should_panic(expected = "unscripted call: no answer left for Sandbox::run_container")]
    fn an_unscripted_run_panics_and_names_the_call() {
        let fake = FakeSandbox::new();
        drop(fake.run_container(&spec(), Path::new("/w/clean.log")));
    }

    #[test]
    #[should_panic(expected = "unscripted call: no answer left for Sandbox::extract_tree")]
    fn an_unscripted_extraction_panics_and_names_the_call() {
        let fake = FakeSandbox::new();
        drop(fake.extract_tree(Path::new("/r"), "v", Path::new("/w")));
    }

    #[test]
    fn a_call_is_recorded_before_the_panic_for_a_missing_script() {
        let fake = FakeSandbox::new();
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drop(fake.tree_escapes(&tree()));
        }));
        assert!(caught.is_err());
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn scratch_and_index_seeding_are_scripted_and_recorded() {
        let fake = FakeSandbox::new();
        fake.script_scratch(Script::always(Ok(PathBuf::from("/s/verify-work/ab12"))));
        fake.script_seed(Script::sequence([Ok(()), Err(SandboxError::IndexNotSeeded)]));
        let contained = ContainedTree::check(tree(), Vec::new()).unwrap();
        assert_eq!(
            fake.scratch_dir(Path::new("/s/verify-work")),
            Ok(PathBuf::from("/s/verify-work/ab12"))
        );
        assert_eq!(fake.seed_index(&contained), Ok(()));
        assert_eq!(fake.seed_index(&contained), Err(SandboxError::IndexNotSeeded));
        assert_eq!(
            fake.calls()[0],
            SandboxCall::Scratch { parent: PathBuf::from("/s/verify-work") }
        );
        assert_eq!(fake.calls().len(), 3);
    }

    #[test]
    fn the_exit_code_is_the_runtimes() {
        assert_eq!(ContainerExit(137).code(), 137);
    }
}
