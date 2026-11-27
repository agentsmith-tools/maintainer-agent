//! Running the programs `run.sh` runs, the way it runs them.
//!
//! The bash passes the shell's whole environment to every command and runs each
//! one in the shell's working directory. [`Exec`] holds both (decision
//! `P2A-env`, see [`crate::env`]) and goes through the two process ports:
//! [`Proc`] for a call whose output the run reads, and [`Pipe`] for a call whose
//! output goes to the log, as `>>"$log" 2>&1` does.
//!
//! Every call ends at [`crate::trap::checkpoint`], so a fatal signal that arrived
//! while the child ran stops the pass before the next step.
//!
//! `$(cmd 2>&1)` reads the two streams interleaved. [`Proc`] captures them apart,
//! so [`Done::merged`] reads standard output and then standard error (decision
//! `P2A-merge`). A helper that writes to both while one call runs reads in
//! another order than the bash did.

use crate::env::Env;
use mx_core::NonEmpty;
use mx_ports::{
    Pipe, PipelineOutcome, PipelineSpec, Proc, ProcError, ProcExit, ProcSpec, Sink, StageSpec,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// How long one child may run. The bash has no limit and systemd stops the unit
/// at 3 h (`TimeoutStartSec`), so a call still running after that is killed.
pub const CALL_TIMEOUT: Duration = Duration::from_hours(3);

/// The shared process adapters.
#[derive(Clone)]
pub struct Adapters {
    pub proc: Arc<dyn Proc + Send + Sync>,
    pub pipe: Arc<dyn Pipe + Send + Sync>,
}

impl std::fmt::Debug for Adapters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Adapters")
    }
}

/// What a finished call left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    /// The exit code as a shell reports it: 127 for a program that is not found,
    /// 126 for one that cannot run, 124 for a timeout.
    pub rc: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// False when the program could not be started at all.
    pub started: bool,
}

impl Done {
    /// `[ rc -eq 0 ]`.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.rc == 0
    }

    /// `$(cmd 2>&1)`: standard output then standard error, as text, without
    /// trailing newlines.
    #[must_use]
    pub fn merged(&self) -> String {
        let mut bytes = self.stdout.clone();
        bytes.extend_from_slice(&self.stderr);
        trimmed_text(&bytes)
    }

    /// `$(cmd)`: standard output as text, without trailing newlines.
    #[must_use]
    pub fn stdout_text(&self) -> String {
        trimmed_text(&self.stdout)
    }
}

/// `$(...)`: lossy text without NUL bytes or trailing newlines.
#[must_use]
pub fn trimmed_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).replace('\0', "");
    text.trim_end_matches('\n').to_string()
}

fn failed_to_start(error: &ProcError) -> Done {
    let (rc, text) = match error {
        ProcError::NotFound { program } => (127, format!("{program}: command not found\n")),
        ProcError::NotExecutable { program } => (126, format!("{program}: Permission denied\n")),
        other => (126, format!("{other}\n")),
    };
    Done { rc, stdout: Vec::new(), stderr: text.into_bytes(), started: false }
}

fn exit_code(exit: ProcExit) -> i32 {
    match exit {
        ProcExit::Code(code) => code,
        ProcExit::TimedOut => 124,
    }
}

/// Runs programs in the environment of the run.
#[derive(Clone, Debug)]
pub struct Exec {
    pub adapters: Adapters,
    /// What every child inherits.
    pub env: Env,
    /// Where children run, after the bash's `cd`. `None` is the caller's own.
    pub cwd: Option<PathBuf>,
    /// The caller's `LC_ALL`, which the adapters pass on when it is a plain name.
    pub locale: Option<String>,
    /// Whether each call ends at [`crate::trap::checkpoint`]. The exit trap runs
    /// its own calls with this off: it must finish after a signal, not wait for it.
    pub checkpoint: bool,
}

impl Exec {
    fn after_call(&self) {
        if self.checkpoint {
            crate::trap::checkpoint();
        }
    }

    /// A signal that has arrived stops the pass before the next program starts, not
    /// after it: a program started now would outlive the signal thread's snapshot of
    /// what is running (decision `P2A-live`).
    fn before_call(&self) {
        if self.checkpoint {
            crate::trap::checkpoint();
        }
    }

    /// The variables of one child: the environment, then `extra`, which wins.
    fn child_env(&self, extra: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut vars: BTreeMap<String, String> = self.env.for_child().into_iter().collect();
        for (name, value) in extra {
            vars.insert((*name).to_string(), (*value).to_string());
        }
        vars.into_iter().collect()
    }

    fn argv(words: &[&str]) -> Option<NonEmpty<String>> {
        let (first, rest) = words.split_first()?;
        Some(NonEmpty::new((*first).to_string(), rest.iter().map(|word| (*word).to_string())))
    }

    /// Runs `words`, reads its output, and waits.
    #[must_use]
    pub fn capture(&self, words: &[&str], extra: &[(&str, &str)]) -> Done {
        self.before_call();
        let Some(argv) = Self::argv(words) else {
            return Done { rc: 127, stdout: Vec::new(), stderr: Vec::new(), started: false };
        };
        let mut spec = ProcSpec::new(argv, CALL_TIMEOUT);
        spec.env = self.child_env(extra);
        spec.cwd.clone_from(&self.cwd);
        spec.locale.clone_from(&self.locale);
        let done = match self.adapters.proc.run(&spec) {
            Ok(output) => Done {
                rc: exit_code(output.exit),
                stdout: output.stdout,
                stderr: output.stderr,
                started: true,
            },
            Err(error) => failed_to_start(&error),
        };
        self.after_call();
        done
    }

    /// Runs `words` with both streams appended to `log`, as `>>"$log" 2>&1`.
    /// Returns the exit code. A program that cannot start leaves its message in
    /// the log, where the shell's own message would have gone.
    ///
    /// The exit trap runs its calls with `checkpoint` off, after the signal thread has
    /// stopped every pipeline and closed the live set to new ones (decision `P2A-live`).
    /// A call through the pipe port would be refused then, so the trap's calls go through
    /// the process port, which captures the two streams and appends them (standard output
    /// first, as in `P2A-merge`).
    #[must_use]
    pub fn to_log(&self, words: &[&str], extra: &[(&str, &str)], log: &Path) -> i32 {
        if !self.checkpoint {
            let done = self.capture(words, extra);
            append(log, &done.stdout);
            append(log, &done.stderr);
            return done.rc;
        }
        self.before_call();
        let Some(argv) = Self::argv(words) else {
            return 127;
        };
        let mut stage = StageSpec::new(argv);
        stage.env = self.child_env(extra);
        stage.cwd.clone_from(&self.cwd);
        stage.stdout = Sink::AppendFile(log.to_path_buf());
        stage.stderr = Sink::SameAsStdout;
        let spec = PipelineSpec { stages: NonEmpty::new(stage, []), timeout: CALL_TIMEOUT };
        let rc = match self.adapters.pipe.run_pipeline(&spec) {
            Ok(outcome) => exit_code(outcome.pipefail()),
            Err(error) => {
                let done = failed_to_start(&error);
                append(log, &done.stderr);
                done.rc
            }
        };
        self.after_call();
        rc
    }

    /// Runs a backend's pipeline. Every stage gets the run's environment under
    /// the variables the plan names, and the working directory of the run when
    /// the plan names none.
    pub fn pipeline(&self, spec: &PipelineSpec) -> Result<PipelineOutcome, ProcError> {
        self.before_call();
        let mut spec = spec.clone();
        let mut stages: Vec<StageSpec> = spec.stages.as_slice().to_vec();
        for stage in &mut stages {
            let plan_env: Vec<(&str, &str)> =
                stage.env.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
            stage.env = self.child_env(&plan_env);
            if stage.cwd.is_none() {
                stage.cwd.clone_from(&self.cwd);
            }
        }
        let mut stages = stages.into_iter();
        if let Some(first) = stages.next() {
            spec.stages = NonEmpty::new(first, stages);
        }
        let outcome = self.adapters.pipe.run_pipeline(&spec);
        self.after_call();
        outcome
    }
}

/// `>>"$log"`: appends bytes to a file, creating it. Returns whether every byte was
/// written. A failure is otherwise ignored, as a redirection that fails on a command
/// that has already printed cannot be reported anywhere better.
pub fn append(log: &Path, bytes: &[u8]) -> bool {
    use std::io::Write;
    match std::fs::OpenOptions::new().create(true).append(true).open(log) {
        Ok(mut file) => file.write_all(bytes).is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mx_ports::{EffectLog, FakePipe, FakeProc, Script};

    fn exec_with(log: &EffectLog, env: &[(&str, &str)]) -> (Exec, Arc<FakeProc>, Arc<FakePipe>) {
        let proc = Arc::new(FakeProc::new(log));
        let pipe = Arc::new(FakePipe::new(log));
        let adapters = Adapters { proc: proc.clone(), pipe: pipe.clone() };
        let env = Env::from_pairs(env.iter().map(|(a, b)| ((*a).to_string(), (*b).to_string())));
        (
            Exec {
                adapters,
                env,
                cwd: Some(PathBuf::from("/repo")),
                locale: None,
                checkpoint: true,
            },
            proc,
            pipe,
        )
    }

    #[test]
    fn a_child_gets_the_whole_environment_the_extras_and_the_working_directory() {
        let log = EffectLog::new();
        let (exec, proc, _) = exec_with(&log, &[("A", "1"), ("B", "2"), ("PATH", "/x")]);
        proc.script(&["tool", "arg"], Script::always(Ok(FakeProc::exited(0, "out\n"))));
        let done = exec.capture(&["tool", "arg"], &[("B", "override"), ("C", "3")]);
        assert_eq!(done.rc, 0);
        let entries = log.entries();
        let mx_ports::Effect::Proc(mx_ports::ProcEffect::Run(spec)) = &entries[0] else {
            panic!("no proc call recorded")
        };
        assert_eq!(
            spec.env,
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "override".to_string()),
                ("C".to_string(), "3".to_string()),
            ]
        );
        assert_eq!(spec.cwd, Some(PathBuf::from("/repo")));
    }

    #[test]
    fn a_program_that_cannot_be_found_reads_as_127_with_the_shells_words() {
        let log = EffectLog::new();
        let (exec, proc, _) = exec_with(&log, &[]);
        proc.script(
            &["gh"],
            Script::always(Err(ProcError::NotFound { program: "gh".to_string() })),
        );
        let done = exec.capture(&["gh"], &[]);
        assert_eq!(done.rc, 127);
        assert!(!done.started);
        assert_eq!(done.merged(), "gh: command not found");
    }

    #[test]
    fn a_timeout_reads_as_124() {
        let log = EffectLog::new();
        let (exec, proc, _) = exec_with(&log, &[]);
        let timed_out = mx_ports::ProcOutput {
            exit: ProcExit::TimedOut,
            stdout: b"partial".to_vec(),
            stderr: Vec::new(),
        };
        proc.script(&["slow"], Script::always(Ok(timed_out)));
        let done = exec.capture(&["slow"], &[]);
        assert_eq!(done.rc, 124);
        assert_eq!(done.stdout_text(), "partial");
    }

    #[test]
    fn merged_output_is_stdout_then_stderr_without_trailing_newlines() {
        let done =
            Done { rc: 0, stdout: b"out\n\n".to_vec(), stderr: b"err\n".to_vec(), started: true };
        assert_eq!(done.merged(), "out\n\nerr");
        assert_eq!(done.stdout_text(), "out");
    }

    #[test]
    fn command_substitution_keeps_trailing_blanks_and_drops_only_newlines() {
        assert_eq!(trimmed_text(b"a \t\n\n"), "a \t");
        assert_eq!(trimmed_text(b"  \n"), "  ");
    }

    #[test]
    fn one_call_may_run_for_three_hours_and_no_longer() {
        // systemd stops the unit at 3 h (`TimeoutStartSec`), so no call outlives it.
        assert_eq!(CALL_TIMEOUT, Duration::from_hours(3));
    }

    #[test]
    fn command_substitution_drops_nul_bytes() {
        assert_eq!(trimmed_text(b"a\0b\n"), "ab");
    }

    #[test]
    fn a_log_call_appends_both_streams_to_the_log_and_returns_the_pipeline_status() {
        let log = EffectLog::new();
        let (exec, _, pipe) = exec_with(&log, &[("A", "1")]);
        pipe.script(Script::always(Ok(FakePipe::exited(&[3]))));
        let rc = exec.to_log(&["helper", "run-lease", "--clear"], &[], Path::new("/the/log"));
        assert_eq!(rc, 3);
        let entries = log.entries();
        let mx_ports::Effect::Pipe(mx_ports::PipeEffect::Run(spec)) = &entries[0] else {
            panic!("no pipe call recorded")
        };
        let stage = spec.stages.first();
        assert_eq!(stage.argv.as_slice(), ["helper", "run-lease", "--clear"]);
        assert_eq!(stage.stdout, Sink::AppendFile(PathBuf::from("/the/log")));
        assert_eq!(stage.stderr, Sink::SameAsStdout);
        assert_eq!(stage.env, vec![("A".to_string(), "1".to_string())]);
    }

    #[test]
    fn an_exit_trap_call_goes_through_the_process_port_and_appends_both_streams() {
        let log = EffectLog::new();
        let (mut exec, proc, pipe) = exec_with(&log, &[]);
        exec.checkpoint = false;
        proc.script(
            &["helper", "run-lease", "--clear"],
            Script::always(Ok(mx_ports::ProcOutput {
                exit: ProcExit::Code(2),
                stdout: b"out\n".to_vec(),
                stderr: b"err\n".to_vec(),
            })),
        );
        let file = std::env::temp_dir().join(format!("mx-run-trapcall-{}.log", std::process::id()));
        drop(std::fs::remove_file(&file));
        let rc = exec.to_log(&["helper", "run-lease", "--clear"], &[], &file);
        assert_eq!(rc, 2);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "out\nerr\n");
        drop(std::fs::remove_file(&file));
        // The pipe port was never asked: it refuses everything once the live set is closing.
        assert!(
            log.entries().iter().all(|entry| matches!(entry, mx_ports::Effect::Proc(_))),
            "{:?}",
            log.entries()
        );
        drop(pipe);
    }

    #[test]
    fn a_pipeline_stage_gets_the_run_environment_under_its_own_variables() {
        let log = EffectLog::new();
        let (exec, _, pipe) = exec_with(&log, &[("A", "1"), ("B", "2")]);
        pipe.script(Script::always(Ok(FakePipe::exited(&[0]))));
        let mut stage = StageSpec::new(NonEmpty::new("claude".to_string(), []));
        stage.env = vec![("B".to_string(), "from-plan".to_string())];
        let spec =
            PipelineSpec { stages: NonEmpty::new(stage, []), timeout: Duration::from_secs(9) };
        exec.pipeline(&spec).unwrap();
        let entries = log.entries();
        let mx_ports::Effect::Pipe(mx_ports::PipeEffect::Run(sent)) = &entries[0] else {
            panic!("no pipe call recorded")
        };
        let stage = sent.stages.first();
        assert_eq!(
            stage.env,
            vec![("A".to_string(), "1".to_string()), ("B".to_string(), "from-plan".to_string())]
        );
        assert_eq!(stage.cwd, Some(PathBuf::from("/repo")));
        assert_eq!(sent.timeout, Duration::from_secs(9));
    }

    #[test]
    fn a_stage_with_its_own_working_directory_keeps_it() {
        let log = EffectLog::new();
        let (exec, _, pipe) = exec_with(&log, &[]);
        pipe.script(Script::always(Ok(FakePipe::exited(&[0]))));
        let mut stage = StageSpec::new(NonEmpty::new("claude".to_string(), []));
        stage.cwd = Some(PathBuf::from("/elsewhere"));
        let spec =
            PipelineSpec { stages: NonEmpty::new(stage, []), timeout: Duration::from_secs(1) };
        exec.pipeline(&spec).unwrap();
        let entries = log.entries();
        let mx_ports::Effect::Pipe(mx_ports::PipeEffect::Run(sent)) = &entries[0] else {
            panic!("no pipe call recorded")
        };
        assert_eq!(sent.stages.first().cwd, Some(PathBuf::from("/elsewhere")));
    }
}
