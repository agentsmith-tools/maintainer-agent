use crate::real_proc::{
    Deadline, KILL_TOOLS, absolute_entries, exit_code, kill_group, launch, new_command,
    plain_locale, resolve, wait_until,
};
use crate::{Pipe, PipelineOutcome, PipelineSpec, ProcError, ProcExit, Sink, Source, StageSpec};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{PipeReader, PipeWriter};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

/// The leaders of the process groups of the pipelines that run now, and whether
/// the owner has begun to stop them.
///
/// A stage is put on the list under the same lock that spawns it, so a stop
/// either finds the stage or comes first and refuses the spawn. Registering after
/// the spawn left a window in which a signal found no stage to kill and the agent
/// then ran on with the token in its environment (decision `P2A-live`).
#[derive(Debug, Default)]
pub struct LiveSet {
    inner: Mutex<Live>,
}

#[derive(Debug, Default)]
struct Live {
    leaders: Vec<u32>,
    closing: bool,
}

impl LiveSet {
    fn lock(&self) -> MutexGuard<'_, Live> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// An empty set. [`kill_live_pipelines`] uses the one every [`RealPipe`] shares by default.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The leaders registered now.
    #[must_use]
    pub fn leaders(&self) -> Vec<u32> {
        self.lock().leaders.clone()
    }

    /// Refuses every later spawn, then sends `SIGKILL` to every registered group.
    /// Returns how many groups the kill utility reached. The lock is held while
    /// it kills, so a spawn that is under way registers before this looks.
    pub fn kill_all(&self) -> usize {
        let mut live = self.lock();
        live.closing = true;
        let tools: Vec<PathBuf> = KILL_TOOLS.iter().map(PathBuf::from).collect();
        live.leaders.iter().filter(|pid| kill_group(&tools, **pid).is_ok()).count()
    }
}

static GLOBAL: OnceLock<Arc<LiveSet>> = OnceLock::new();

fn global() -> &'static Arc<LiveSet> {
    GLOBAL.get_or_init(LiveSet::new)
}

/// Stops every pipeline a [`RealPipe`] started and has not yet reaped, and
/// refuses every pipeline that would start after this call. Returns how many
/// process groups it signalled.
///
/// A terminal's Ctrl-C reaches a shell's whole pipeline because the shell leaves
/// its children in its own process group. Every stage here leads a group of its
/// own, so the signal handler of a program that runs a pipeline calls this
/// before it leaves, or the agent would outlive its orchestrator.
#[must_use]
pub fn kill_live_pipelines() -> usize {
    global().kill_all()
}

/// A function that runs on the spawning thread between the spawn and the
/// registration, so a test can put a kill exactly there.
#[cfg(test)]
type Hook = std::sync::Arc<dyn Fn() + Send + Sync>;

#[cfg(test)]
static HOOK: Mutex<Option<(std::thread::ThreadId, Hook)>> = Mutex::new(None);

#[cfg(test)]
fn run_hook() {
    let hook = HOOK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .filter(|(thread, _)| *thread == std::thread::current().id())
        .map(|(_, hook)| Arc::clone(hook));
    if let Some(hook) = hook {
        hook();
    }
}

/// Runs a [`PipelineSpec`] for real.
///
/// - Each stage is an argument vector, resolved against the adapter's own
///   search path. No shell reads it.
/// - A stage starts with an empty environment: the variables its spec lists,
///   then `PATH` (the search path) and `LC_ALL`, which a stage cannot override.
///   `LC_ALL` is the locale the adapter was built with when that is a plain
///   name, else `C`.
/// - Every stage leads its own process group, so one deadline kills them all.
/// - A stage's streams go to files and to the next stage's input through
///   operating-system pipes that this process never reads, so a stage that writes
///   more than a pipe buffer cannot deadlock the caller.
/// - [`Sink::AppendFile`] opens with `O_APPEND` and creates a missing file, as
///   the shell's `>>` does. [`Sink::SameAsStdout`] is a duplicate of the standard
///   output descriptor, as `2>&1` is, so both streams interleave in write order.
/// - A stage that dies of signal `n` reports exit code `128 + n`. At the
///   deadline every still-running stage is killed and reports
///   [`ProcExit::TimedOut`].
///
/// A stage that cannot be started stops the pipeline: the stages already running
/// are killed and the call returns the error. A shell would report 127 for that
/// stage and run the others, which then read an empty input.
///
/// The group kill runs the `kill` utility, because the standard library has no
/// safe call for it and this workspace forbids `unsafe`.
#[derive(Debug, Clone)]
pub struct RealPipe {
    search_dirs: Vec<PathBuf>,
    locale: Option<String>,
    kill_tools: Vec<PathBuf>,
    live: Arc<LiveSet>,
}

impl RealPipe {
    /// An adapter that resolves programs against `search_path` and runs every
    /// stage under `locale` (`None` is `C`).
    pub fn new(search_path: impl Into<OsString>, locale: Option<String>) -> Self {
        Self {
            search_dirs: absolute_entries(&search_path.into()),
            locale,
            kill_tools: KILL_TOOLS.iter().map(PathBuf::from).collect(),
            live: Arc::clone(global()),
        }
    }

    /// Registers stages in `live` instead of the set [`kill_live_pipelines`] stops.
    #[must_use]
    pub fn with_live_set(mut self, live: Arc<LiveSet>) -> Self {
        self.live = live;
        self
    }

    /// Uses `tool` instead of the system `kill` for the process-group kill.
    #[must_use]
    pub fn with_group_kill_tool(mut self, tool: impl Into<PathBuf>) -> Self {
        self.kill_tools = vec![tool.into()];
        self
    }
}

/// One stage that has started.
struct Running {
    name: String,
    child: Child,
    status: Option<ExitStatus>,
}

/// Where a stream of a stage ends up.
enum Dest {
    Null,
    File(File),
    Pipe(PipeWriter),
}

impl Dest {
    fn try_clone(&self) -> std::io::Result<Self> {
        Ok(match self {
            Self::Null => Self::Null,
            Self::File(file) => Self::File(file.try_clone()?),
            Self::Pipe(pipe) => Self::Pipe(pipe.try_clone()?),
        })
    }

    fn into_stdio(self) -> Stdio {
        match self {
            Self::Null => Stdio::null(),
            Self::File(file) => Stdio::from(file),
            Self::Pipe(pipe) => Stdio::from(pipe),
        }
    }
}

fn invalid(stage: &StageSpec, reason: &str) -> ProcError {
    ProcError::Spawn {
        program: stage.argv.first().clone(),
        reason: format!("invalid pipeline: {reason}"),
    }
}

/// Refuses a pipeline whose wiring cannot be made.
fn validate(stages: &[StageSpec]) -> Result<(), ProcError> {
    let last = stages.len() - 1;
    for (index, stage) in stages.iter().enumerate() {
        if stage.stdin == Source::PreviousStage && index == 0 {
            return Err(invalid(stage, "the first stage reads the previous stage"));
        }
        if stage.stdout == Sink::NextStage && index == last {
            return Err(invalid(stage, "the last stage writes to the next stage"));
        }
        if stage.stdout == Sink::SameAsStdout {
            return Err(invalid(stage, "standard output cannot be the same as itself"));
        }
        if stage.stderr == Sink::NextStage {
            return Err(invalid(stage, "only standard output can feed the next stage"));
        }
        let fed = index > 0 && stages[index - 1].stdout == Sink::NextStage;
        if fed != (stage.stdin == Source::PreviousStage) {
            return Err(invalid(
                stage,
                "a stage reads the previous stage exactly when that stage writes to it",
            ));
        }
    }
    Ok(())
}

fn append_file(path: &std::path::Path, program: &str) -> Result<File, ProcError> {
    OpenOptions::new().create(true).append(true).open(path).map_err(|error| ProcError::Io {
        program: program.to_string(),
        reason: format!("{}: {error}", path.display()),
    })
}

impl RealPipe {
    /// Starts one stage. `upstream` is the read end of the previous stage's pipe.
    fn start(
        &self,
        stage: &StageSpec,
        upstream: Option<PipeReader>,
    ) -> Result<(Child, Option<PipeReader>), ProcError> {
        let name = stage.argv.first();
        let program = resolve(&self.search_dirs, name)?;
        let io_error = |reason: String| ProcError::Io { program: name.clone(), reason };

        let stdin = match (&stage.stdin, upstream) {
            (Source::Empty, _) => Stdio::null(),
            (Source::File(path), _) => Stdio::from(
                File::open(path)
                    .map_err(|error| io_error(format!("{}: {error}", path.display())))?,
            ),
            (Source::PreviousStage, Some(reader)) => Stdio::from(reader),
            (Source::PreviousStage, None) => {
                return Err(invalid(stage, "no previous stage to read"));
            }
        };

        let mut next_reader = None;
        let stdout = match &stage.stdout {
            Sink::Null | Sink::SameAsStdout => Dest::Null,
            Sink::AppendFile(path) => Dest::File(append_file(path, name)?),
            Sink::NextStage => {
                let (reader, writer) = std::io::pipe()
                    .map_err(|error| io_error(format!("could not make a pipe: {error}")))?;
                next_reader = Some(reader);
                Dest::Pipe(writer)
            }
        };
        let stderr = match &stage.stderr {
            Sink::Null | Sink::NextStage => Dest::Null,
            Sink::AppendFile(path) => Dest::File(append_file(path, name)?),
            Sink::SameAsStdout => stdout
                .try_clone()
                .map_err(|error| io_error(format!("could not duplicate stdout: {error}")))?,
        };

        let launch = launch(&self.search_dirs, &program, name)?;
        let mut command = new_command(&launch.program);
        command
            .arg0(&launch.arg0)
            .args(&launch.leading)
            .args(&stage.argv.as_slice()[1..])
            .env_clear();
        for (key, value) in &stage.env {
            command.env(key, value);
        }
        command
            .env("PATH", std::env::join_paths(&self.search_dirs).unwrap_or_default())
            .env("LC_ALL", plain_locale(self.locale.as_deref()));
        if let Some(dir) = &stage.cwd {
            command.current_dir(dir);
        }
        command
            .process_group(0)
            .stdin(stdin)
            .stdout(stdout.into_stdio())
            .stderr(stderr.into_stdio());
        // The lock covers the spawn and the registration: a stop that runs now
        // waits for the stage to be on the list, and a stop that ran first
        // refuses the spawn.
        let mut live = self.live.lock();
        if live.closing {
            return Err(ProcError::Spawn {
                program: name.clone(),
                reason: "the orchestrator is stopping, so no stage starts".to_string(),
            });
        }
        let child = command.spawn().map_err(|error| ProcError::Spawn {
            program: name.clone(),
            reason: error.to_string(),
        })?;
        // The command still holds this process's copies of the write ends; drop
        // it so the next stage sees end of file when the writer exits.
        drop(command);
        #[cfg(test)]
        run_hook();
        live.leaders.push(child.id());
        drop(live);
        Ok((child, next_reader))
    }

    /// Kills every group, then every program, then reaps them. Returns the first
    /// reason a group kill could not run, and whether any group kill failed
    /// while its stage was still running.
    fn kill_all(&self, running: &mut [Running]) -> Result<(), ProcError> {
        let mut failure = None;
        for stage in running.iter_mut() {
            if stage.status.is_some() {
                continue;
            }
            match kill_group(&self.kill_tools, stage.child.id()) {
                Err(reason) => {
                    failure.get_or_insert(ProcError::KillFailed {
                        program: stage.name.clone(),
                        reason,
                    });
                }
                Ok(false) => {
                    failure.get_or_insert(ProcError::KillFailed {
                        program: stage.name.clone(),
                        reason:
                            "the kill utility reported failure while the program was still running"
                                .to_string(),
                    });
                }
                Ok(true) => {}
            }
            drop(stage.child.kill());
            stage.status = stage.child.wait().ok();
        }
        failure.map_or(Ok(()), Err)
    }
}

impl Pipe for RealPipe {
    fn run_pipeline(&self, spec: &PipelineSpec) -> Result<PipelineOutcome, ProcError> {
        let stages = spec.stages.as_slice();
        validate(stages)?;
        let deadline = Deadline::after(spec.timeout);

        let mut running: Vec<Running> = Vec::with_capacity(stages.len());
        let mut upstream: Option<PipeReader> = None;
        for stage in stages {
            match self.start(stage, upstream.take()) {
                Ok((child, reader)) => {
                    upstream = reader;
                    running.push(Running { name: stage.argv.first().clone(), child, status: None });
                }
                Err(error) => {
                    drop(self.kill_all(&mut running));
                    self.forget(&running);
                    return Err(error);
                }
            }
        }

        let mut wait_failure: Option<(String, std::io::Error)> = None;
        let finished = wait_until(deadline, || {
            for stage in &mut running {
                if stage.status.is_none() {
                    match stage.child.try_wait() {
                        Ok(Some(done)) => stage.status = Some(done),
                        Ok(None) => {}
                        Err(error) => {
                            wait_failure = Some((stage.name.clone(), error));
                            return true;
                        }
                    }
                }
            }
            running.iter().all(|stage| stage.status.is_some())
        });
        if let Some((program, error)) = wait_failure {
            drop(self.kill_all(&mut running));
            self.forget(&running);
            return Err(ProcError::Io { program, reason: format!("waiting failed: {error}") });
        }

        let timed_out: Vec<bool> = running.iter().map(|stage| stage.status.is_none()).collect();
        let kill = if finished { Ok(()) } else { self.kill_all(&mut running) };
        self.forget(&running);
        kill?;
        let exits = running
            .iter()
            .zip(&timed_out)
            .map(|(stage, timed_out)| match (stage.status, timed_out) {
                (_, true) => ProcExit::TimedOut,
                (Some(status), false) => ProcExit::Code(exit_code(status)),
                (None, false) => ProcExit::Code(255),
            })
            .collect();
        Ok(PipelineOutcome { exits })
    }
}

impl RealPipe {
    /// Takes the leaders of `running` off the live list.
    fn forget(&self, running: &[Running]) {
        let ids: Vec<u32> = running.iter().map(|stage| stage.child.id()).collect();
        self.live.lock().leaders.retain(|pid| !ids.contains(pid));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mx_core::NonEmpty;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    const SEARCH_PATH: &str = "/usr/bin:/bin";
    const LONG: Duration = Duration::from_secs(30);
    /// No test may wait longer than this for one pipeline.
    const BOUND: Duration = Duration::from_secs(15);

    /// A directory under the system temp dir that is removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("mx-pipe-{tag}-{}-{unique}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir.canonicalize().unwrap())
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            drop(std::fs::remove_dir_all(&self.0));
        }
    }

    fn pipe() -> RealPipe {
        RealPipe::new(SEARCH_PATH, None)
    }

    /// A stage that runs `sh -c script`, with both streams discarded.
    fn sh(script: &str) -> StageSpec {
        StageSpec::new(NonEmpty::new("sh".to_string(), ["-c".to_string(), script.to_string()]))
    }

    fn program(name: &str, args: &[&str]) -> StageSpec {
        StageSpec::new(NonEmpty::new(name.to_string(), args.iter().map(|arg| (*arg).to_string())))
    }

    fn spec(stages: Vec<StageSpec>, timeout: Duration) -> PipelineSpec {
        let mut stages = stages.into_iter();
        let first = stages.next().expect("a pipeline has a stage");
        PipelineSpec { stages: NonEmpty::new(first, stages), timeout }
    }

    /// Runs the pipeline on its own thread and fails the test when it does not
    /// return within `BOUND`: a deadlock or a kill that does not work would
    /// otherwise hang the suite.
    fn run_bounded(pipe: RealPipe, spec: PipelineSpec) -> Result<PipelineOutcome, ProcError> {
        let (sender, receiver) = std::sync::mpsc::channel();
        drop(std::thread::spawn(move || drop(sender.send(pipe.run_pipeline(&spec)))));
        receiver.recv_timeout(BOUND).unwrap_or_else(|_| {
            panic!("RealPipe did not return within {BOUND:?}: deadlock, or the kill does not work")
        })
    }

    fn read(path: &Path) -> String {
        String::from_utf8(std::fs::read(path).unwrap()).unwrap()
    }

    fn exits(outcome: &PipelineOutcome) -> Vec<ProcExit> {
        outcome.exits.clone()
    }

    #[cfg(target_os = "linux")]
    fn is_alive(pid: u32) -> bool {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Err(_) => false,
            Ok(stat) => stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.trim_start().chars().next())
                .is_some_and(|state| state != 'Z' && state != 'X'),
        }
    }

    // Decision P5B-1, as in `RealProc`: a stage that is a text file naming no
    // interpreter runs as a shell script, as it does when bash starts it.
    #[test]
    fn a_stage_that_is_a_text_file_without_an_interpreter_line_runs_as_a_shell_script() {
        let dir = TempDir::new("stage-noshebang");
        let tool = dir.path("mx-stage");
        std::fs::write(&tool, "printf 'stage %s\\n' \"$1\"\n").unwrap();
        std::fs::set_permissions(&tool, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let log = dir.path("log");
        let mut stage = program(tool.to_str().unwrap(), &["one"]);
        stage.stdout = Sink::AppendFile(log.clone());
        let outcome = run_bounded(pipe(), spec(vec![stage], LONG)).unwrap();
        assert_eq!(exits(&outcome), vec![ProcExit::Code(0)]);
        assert_eq!(read(&log), "stage one\n");
    }

    #[test]
    fn one_stage_appends_both_streams_to_the_file_in_write_order() {
        let dir = TempDir::new("both");
        let log = dir.path("log");
        let mut stage = sh("echo out1; echo err1 >&2; echo out2; echo err2 >&2");
        stage.stdout = Sink::AppendFile(log.clone());
        stage.stderr = Sink::SameAsStdout;
        let outcome = run_bounded(pipe(), spec(vec![stage], LONG)).unwrap();
        assert_eq!(exits(&outcome), vec![ProcExit::Code(0)]);
        assert_eq!(read(&log), "out1\nerr1\nout2\nerr2\n");
    }

    #[test]
    fn appending_keeps_what_the_file_already_holds_and_creates_a_missing_file() {
        let dir = TempDir::new("append");
        let existing = dir.path("existing");
        std::fs::write(&existing, "before\n").unwrap();
        let missing = dir.path("missing");
        let mut stage = sh("echo after");
        stage.stdout = Sink::AppendFile(existing.clone());
        run_bounded(pipe(), spec(vec![stage], LONG)).unwrap();
        let mut stage = sh("echo created");
        stage.stdout = Sink::AppendFile(missing.clone());
        run_bounded(pipe(), spec(vec![stage], LONG)).unwrap();
        assert_eq!(read(&existing), "before\nafter\n");
        assert_eq!(read(&missing), "created\n");
    }

    #[test]
    fn the_two_streams_of_a_stage_can_go_to_two_files() {
        let dir = TempDir::new("split");
        let mut stage = sh("echo to-out; echo to-err >&2");
        stage.stdout = Sink::AppendFile(dir.path("out"));
        stage.stderr = Sink::AppendFile(dir.path("err"));
        run_bounded(pipe(), spec(vec![stage], LONG)).unwrap();
        assert_eq!(read(&dir.path("out")), "to-out\n");
        assert_eq!(read(&dir.path("err")), "to-err\n");
    }

    #[test]
    fn a_file_source_feeds_the_stage_its_bytes() {
        let dir = TempDir::new("source");
        let input = dir.path("prompt");
        std::fs::write(&input, "line one\nline two\n").unwrap();
        let mut stage = program("cat", &[]);
        stage.stdin = Source::File(input);
        stage.stdout = Sink::AppendFile(dir.path("out"));
        run_bounded(pipe(), spec(vec![stage], LONG)).unwrap();
        assert_eq!(read(&dir.path("out")), "line one\nline two\n");
    }

    #[test]
    fn an_empty_source_gives_end_of_file_at_once() {
        let dir = TempDir::new("empty");
        let mut stage = program("cat", &[]);
        stage.stdout = Sink::AppendFile(dir.path("out"));
        let outcome = run_bounded(pipe(), spec(vec![stage], LONG)).unwrap();
        assert_eq!(exits(&outcome), vec![ProcExit::Code(0)]);
        assert_eq!(read(&dir.path("out")), "");
    }

    #[test]
    fn the_next_stage_reads_what_the_previous_stage_wrote() {
        let dir = TempDir::new("chain");
        let mut first = sh("echo hello pipe");
        first.stdout = Sink::NextStage;
        let mut second = program("tr", &["a-z", "A-Z"]);
        second.stdin = Source::PreviousStage;
        second.stdout = Sink::AppendFile(dir.path("out"));
        let outcome = run_bounded(pipe(), spec(vec![first, second], LONG)).unwrap();
        assert_eq!(exits(&outcome), vec![ProcExit::Code(0), ProcExit::Code(0)]);
        assert_eq!(read(&dir.path("out")), "HELLO PIPE\n");
    }

    #[test]
    fn standard_error_can_follow_standard_output_into_the_next_stage() {
        let dir = TempDir::new("errpipe");
        let mut first = sh("echo out; echo err >&2");
        first.stdout = Sink::NextStage;
        first.stderr = Sink::SameAsStdout;
        let mut second = program("cat", &[]);
        second.stdin = Source::PreviousStage;
        second.stdout = Sink::AppendFile(dir.path("out"));
        run_bounded(pipe(), spec(vec![first, second], LONG)).unwrap();
        assert_eq!(read(&dir.path("out")), "out\nerr\n");
    }

    #[test]
    fn a_stage_that_writes_a_megabyte_does_not_deadlock_the_pipeline() {
        let dir = TempDir::new("megabyte");
        let mut first = program("head", &["-c", "1048576", "/dev/zero"]);
        first.stdout = Sink::NextStage;
        let mut second = program("wc", &["-c"]);
        second.stdin = Source::PreviousStage;
        second.stdout = Sink::AppendFile(dir.path("count"));
        let outcome = run_bounded(pipe(), spec(vec![first, second], LONG)).unwrap();
        assert_eq!(exits(&outcome), vec![ProcExit::Code(0), ProcExit::Code(0)]);
        assert_eq!(read(&dir.path("count")).trim(), "1048576");
    }

    #[test]
    fn a_megabyte_on_standard_error_alone_reaches_the_file() {
        let dir = TempDir::new("megaerr");
        let mut stage = sh("head -c 1048576 /dev/zero >&2");
        stage.stderr = Sink::AppendFile(dir.path("err"));
        let outcome = run_bounded(pipe(), spec(vec![stage], LONG)).unwrap();
        assert_eq!(exits(&outcome), vec![ProcExit::Code(0)]);
        assert_eq!(std::fs::metadata(dir.path("err")).unwrap().len(), 1_048_576);
    }

    #[test]
    fn a_megabyte_source_through_cat_reaches_the_file_whole() {
        let dir = TempDir::new("bigsource");
        let input = dir.path("in");
        std::fs::write(&input, vec![b'x'; 1_048_576]).unwrap();
        let mut stage = program("cat", &[]);
        stage.stdin = Source::File(input);
        stage.stdout = Sink::AppendFile(dir.path("out"));
        run_bounded(pipe(), spec(vec![stage], LONG)).unwrap();
        assert_eq!(std::fs::metadata(dir.path("out")).unwrap().len(), 1_048_576);
    }

    #[test]
    fn yes_into_head_ends_and_the_writer_reports_sigpipe_as_141() {
        let dir = TempDir::new("yes");
        let mut first = program("yes", &[]);
        first.stdout = Sink::NextStage;
        let mut second = program("head", &["-n", "3"]);
        second.stdin = Source::PreviousStage;
        second.stdout = Sink::AppendFile(dir.path("out"));
        let outcome = run_bounded(pipe(), spec(vec![first, second], LONG)).unwrap();
        assert_eq!(read(&dir.path("out")), "y\ny\ny\n");
        assert_eq!(exits(&outcome), vec![ProcExit::Code(141), ProcExit::Code(0)]);
        assert_eq!(outcome.pipefail(), ProcExit::Code(141));
    }

    #[test]
    fn a_stage_that_exits_3_before_its_reader_does_keeps_its_own_code() {
        let mut first = sh("echo some; exit 3");
        first.stdout = Sink::NextStage;
        let mut second = sh("sleep 1; cat >/dev/null");
        second.stdin = Source::PreviousStage;
        let outcome = run_bounded(pipe(), spec(vec![first, second], LONG)).unwrap();
        assert_eq!(exits(&outcome), vec![ProcExit::Code(3), ProcExit::Code(0)]);
        assert_eq!(outcome.pipefail(), ProcExit::Code(3));
    }

    #[test]
    fn the_rightmost_failing_stage_decides_pipefail() {
        let mut first = sh("exit 3");
        first.stdout = Sink::NextStage;
        let mut second = sh("cat >/dev/null; exit 4");
        second.stdin = Source::PreviousStage;
        let outcome = run_bounded(pipe(), spec(vec![first, second], LONG)).unwrap();
        assert_eq!(exits(&outcome), vec![ProcExit::Code(3), ProcExit::Code(4)]);
        assert_eq!(outcome.pipefail(), ProcExit::Code(4));
    }

    #[test]
    fn a_stage_killed_by_a_signal_reports_128_plus_the_signal() {
        let outcome = run_bounded(pipe(), spec(vec![sh("kill -9 $$")], LONG)).unwrap();
        assert_eq!(exits(&outcome), vec![ProcExit::Code(137)]);
        let outcome = run_bounded(pipe(), spec(vec![sh("kill -TERM $$")], LONG)).unwrap();
        assert_eq!(exits(&outcome), vec![ProcExit::Code(143)]);
    }

    #[test]
    fn a_stage_that_ignores_sigterm_is_killed_at_the_deadline() {
        let dir = TempDir::new("sigterm");
        let pidfile = dir.path("pid");
        let script =
            format!("trap '' TERM; echo $$ > {}; while :; do sleep 1; done", pidfile.display());
        let started = Instant::now();
        let outcome = run_bounded(pipe(), spec(vec![sh(&script)], Duration::from_secs(1))).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(exits(&outcome), vec![ProcExit::TimedOut]);
        assert_eq!(outcome.pipefail(), ProcExit::TimedOut);
        assert!(elapsed >= Duration::from_secs(1), "returned after {elapsed:?}, too early");
        // The deadline is the stage's lifetime: twice the deadline would be 2 s.
        assert!(elapsed < Duration::from_millis(1900), "returned after {elapsed:?}");
        let pid: u32 = read(&pidfile).trim().parse().expect("the stage wrote its pid");
        let gone = wait_until(Instant::now() + Duration::from_secs(3), || !is_alive(pid));
        assert!(gone, "the stage {pid} survived the deadline");
    }

    #[test]
    fn the_deadline_kills_every_stage_and_the_grandchildren_of_each() {
        let dir = TempDir::new("deadline");
        let mut first = sh(&format!("sleep 60 & echo $! > {}; wait", dir.path("g1").display()));
        first.stdout = Sink::NextStage;
        let mut second = sh(&format!("sleep 60 & echo $! > {}; wait", dir.path("g2").display()));
        second.stdin = Source::PreviousStage;
        let outcome =
            run_bounded(pipe(), spec(vec![first, second], Duration::from_millis(1500))).unwrap();
        assert_eq!(exits(&outcome), vec![ProcExit::TimedOut, ProcExit::TimedOut]);
        for name in ["g1", "g2"] {
            let pid: u32 = read(&dir.path(name)).trim().parse().unwrap();
            let gone = wait_until(Instant::now() + Duration::from_secs(3), || !is_alive(pid));
            assert!(gone, "the grandchild {name} ({pid}) survived the deadline");
        }
    }

    #[test]
    fn a_stage_that_finished_before_the_deadline_keeps_its_code() {
        let mut first = sh("exit 5");
        first.stdout = Sink::NextStage;
        let mut second = sh("cat >/dev/null; sleep 60");
        second.stdin = Source::PreviousStage;
        let outcome =
            run_bounded(pipe(), spec(vec![first, second], Duration::from_millis(1500))).unwrap();
        assert_eq!(exits(&outcome), vec![ProcExit::Code(5), ProcExit::TimedOut]);
    }

    /// Runs `spec` on its own thread and tells an answer from a panic from a hang. A timeout the
    /// platform's time type cannot hold once made the call panic (decision `FU1-1`).
    fn run_watched(spec: PipelineSpec) -> Result<Result<PipelineOutcome, ProcError>, String> {
        let (sender, receiver) = std::sync::mpsc::channel();
        drop(std::thread::spawn(move || drop(sender.send(pipe().run_pipeline(&spec)))));
        receiver.recv_timeout(BOUND).map_err(|error| match error {
            std::sync::mpsc::RecvTimeoutError::Disconnected => "the call panicked".to_string(),
            std::sync::mpsc::RecvTimeoutError::Timeout => {
                format!("the call did not return within {BOUND:?}")
            }
        })
    }

    fn assert_oversized_timeout_waits_for_the_stage(timeout: Duration) {
        let dir = TempDir::new("oversized");
        let log = dir.path("log");
        let mut stage = sh("sleep 0.3; echo finished");
        stage.stdout = Sink::AppendFile(log.clone());
        let answer = run_watched(spec(vec![stage], timeout))
            .unwrap_or_else(|why| panic!("timeout {timeout:?}: {why}"));
        let outcome = answer.unwrap_or_else(|error| panic!("timeout {timeout:?}: {error}"));
        assert_eq!(
            exits(&outcome),
            vec![ProcExit::Code(0)],
            "timeout {timeout:?} expired a stage that was still running"
        );
        assert_eq!(read(&log), "finished\n", "timeout {timeout:?}");
    }

    fn assert_tiny_timeout_kills_a_long_stage(timeout: Duration) {
        let started = Instant::now();
        let answer = run_watched(spec(vec![sh("sleep 30")], timeout))
            .unwrap_or_else(|why| panic!("timeout {timeout:?}: {why}"));
        let outcome = answer.unwrap_or_else(|error| panic!("timeout {timeout:?}: {error}"));
        assert_eq!(exits(&outcome), vec![ProcExit::TimedOut], "timeout {timeout:?}");
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(5), "timeout {timeout:?} returned after {elapsed:?}");
    }

    #[test]
    fn a_pipeline_timeout_of_duration_max_waits_for_the_stage_and_does_not_panic() {
        assert_oversized_timeout_waits_for_the_stage(Duration::MAX);
    }

    #[test]
    fn a_pipeline_timeout_of_u64_max_seconds_waits_for_the_stage_and_does_not_panic() {
        assert_oversized_timeout_waits_for_the_stage(Duration::from_secs(u64::MAX));
    }

    #[test]
    fn a_pipeline_timeout_of_i64_max_seconds_waits_for_the_stage_and_does_not_panic() {
        assert_oversized_timeout_waits_for_the_stage(Duration::from_secs(u64::MAX / 2));
    }

    #[test]
    fn a_pipeline_timeout_the_platform_clock_can_still_hold_waits_for_the_stage() {
        assert_oversized_timeout_waits_for_the_stage(Duration::from_secs(1 << 62));
    }

    #[test]
    fn a_zero_pipeline_timeout_times_out_a_stage_that_is_still_running() {
        assert_tiny_timeout_kills_a_long_stage(Duration::ZERO);
    }

    #[test]
    fn a_pipeline_timeout_of_one_nanosecond_times_out_a_stage_that_is_still_running() {
        assert_tiny_timeout_kills_a_long_stage(Duration::from_nanos(1));
    }

    #[test]
    fn every_stage_leads_its_own_process_group() {
        let dir = TempDir::new("groups");
        let script = |name: &str| {
            format!(
                "read pid rest < /proc/self/stat; echo \"$pid $(cut -d' ' -f5 /proc/self/stat)\" > {}",
                dir.path(name).display()
            )
        };
        let mut first = sh(&script("one"));
        first.stdout = Sink::NextStage;
        let mut second = sh(&format!("cat > /dev/null; {}", script("two")));
        second.stdin = Source::PreviousStage;
        run_bounded(pipe(), spec(vec![first, second], LONG)).unwrap();
        let mut groups = Vec::new();
        for name in ["one", "two"] {
            let text = read(&dir.path(name));
            let (leader, group) = text.trim().split_once(' ').unwrap();
            assert_eq!(leader, group, "stage {name} leads no group of its own");
            groups.push(group.to_string());
        }
        assert_ne!(groups[0], groups[1]);
    }

    #[test]
    fn a_stage_sees_only_its_own_variables_plus_path_and_lc_all() {
        let dir = TempDir::new("env");
        let mut stage = program("printenv", &[]);
        stage.env = vec![("ONLY_THIS".to_string(), "yes".to_string())];
        stage.stdout = Sink::AppendFile(dir.path("env"));
        let ambient = std::env::vars_os().filter(|(name, _)| name != "PATH").count();
        assert!(ambient > 0, "the test process has no variable, so this proves nothing");
        run_bounded(
            RealPipe::new(SEARCH_PATH, Some("en_US.UTF-8".to_string())),
            spec(vec![stage], LONG),
        )
        .unwrap();
        let mut seen: Vec<String> = read(&dir.path("env")).lines().map(str::to_string).collect();
        seen.sort();
        assert_eq!(seen, vec!["LC_ALL=en_US.UTF-8", "ONLY_THIS=yes", "PATH=/usr/bin:/bin"]);
    }

    #[test]
    fn a_stage_cannot_override_path_or_lc_all() {
        let dir = TempDir::new("override");
        let mut stage = program("printenv", &[]);
        stage.env = vec![
            ("PATH".to_string(), "/evil".to_string()),
            ("LC_ALL".to_string(), "evil".to_string()),
        ];
        stage.stdout = Sink::AppendFile(dir.path("env"));
        run_bounded(pipe(), spec(vec![stage], LONG)).unwrap();
        let text = read(&dir.path("env"));
        assert!(text.lines().any(|line| line == "PATH=/usr/bin:/bin"), "{text}");
        assert!(text.lines().any(|line| line == "LC_ALL=C"), "{text}");
        assert!(!text.contains("evil"), "{text}");
    }

    #[test]
    fn a_locale_that_is_not_a_plain_name_falls_back_to_c() {
        let dir = TempDir::new("locale");
        let mut stage = program("printenv", &["LC_ALL"]);
        stage.stdout = Sink::AppendFile(dir.path("out"));
        run_bounded(
            RealPipe::new(SEARCH_PATH, Some("x; rm -rf /".to_string())),
            spec(vec![stage], LONG),
        )
        .unwrap();
        assert_eq!(read(&dir.path("out")), "C\n");
    }

    #[test]
    fn a_stage_runs_in_its_working_directory() {
        let dir = TempDir::new("cwd");
        let mut stage = program("pwd", &[]);
        stage.cwd = Some(dir.path(""));
        stage.stdout = Sink::AppendFile(dir.path("out"));
        run_bounded(pipe(), spec(vec![stage], LONG)).unwrap();
        assert_eq!(
            read(&dir.path("out")).trim(),
            dir.path("").to_str().unwrap().trim_end_matches('/')
        );
    }

    #[test]
    fn a_program_that_is_not_on_the_search_path_is_an_error() {
        let stage = program("no-such-program-xyz", &[]);
        let error = run_bounded(pipe(), spec(vec![stage], LONG)).unwrap_err();
        assert!(matches!(error, ProcError::NotFound { .. }), "got {error:?}");
    }

    #[test]
    fn a_stage_that_cannot_start_stops_the_stages_already_running() {
        let dir = TempDir::new("abort");
        let pidfile = dir.path("pid");
        let mut first = sh(&format!("echo $$ > {}; sleep 60", pidfile.display()));
        first.stdout = Sink::NextStage;
        let mut second = program("no-such-program-xyz", &[]);
        second.stdin = Source::PreviousStage;
        let error = run_bounded(pipe(), spec(vec![first, second], LONG)).unwrap_err();
        assert!(matches!(error, ProcError::NotFound { .. }), "got {error:?}");
        let wrote = wait_until(Instant::now() + Duration::from_secs(2), || pidfile.exists());
        if wrote {
            let pid: u32 = read(&pidfile).trim().parse().unwrap_or(0);
            let gone = wait_until(Instant::now() + Duration::from_secs(3), || !is_alive(pid));
            assert!(gone, "the first stage {pid} survived the failed start");
        }
    }

    #[test]
    fn an_unreadable_source_file_is_an_error_and_starts_nothing() {
        let dir = TempDir::new("nosource");
        let mut stage = program("cat", &[]);
        stage.stdin = Source::File(dir.path("missing"));
        stage.stdout = Sink::AppendFile(dir.path("out"));
        let error = run_bounded(pipe(), spec(vec![stage], LONG)).unwrap_err();
        assert!(matches!(error, ProcError::Io { .. }), "got {error:?}");
        assert!(!dir.path("out").exists() || read(&dir.path("out")).is_empty());
    }

    #[test]
    fn wiring_that_cannot_be_made_is_refused_before_anything_starts() {
        let dir = TempDir::new("wiring");
        let marker = dir.path("ran");
        let touch = |name: &str| sh(&format!("touch {}; {name}", marker.display()));
        let mut first_reads_previous = touch("true");
        first_reads_previous.stdin = Source::PreviousStage;
        let mut last_feeds_next = touch("true");
        last_feeds_next.stdout = Sink::NextStage;
        let mut stdout_same = touch("true");
        stdout_same.stdout = Sink::SameAsStdout;
        let mut stderr_next = touch("true");
        stderr_next.stderr = Sink::NextStage;
        for stage in [first_reads_previous, last_feeds_next, stdout_same, stderr_next] {
            let error = run_bounded(pipe(), spec(vec![stage], LONG)).unwrap_err();
            assert!(matches!(error, ProcError::Spawn { .. }), "got {error:?}");
        }
        let mut writer = touch("true");
        writer.stdout = Sink::NextStage;
        let reader_that_ignores = touch("true");
        let error = run_bounded(pipe(), spec(vec![writer, reader_that_ignores], LONG)).unwrap_err();
        assert!(matches!(error, ProcError::Spawn { .. }), "got {error:?}");
        assert!(!marker.exists(), "a refused pipeline started a stage");
    }

    #[test]
    fn a_missing_kill_utility_is_reported_after_the_stage_is_killed() {
        let broken = pipe().with_group_kill_tool("/nonexistent/kill");
        let error =
            run_bounded(broken, spec(vec![program("sleep", &["30"])], Duration::from_millis(800)))
                .unwrap_err();
        assert!(matches!(error, ProcError::KillFailed { .. }), "got {error:?}");
    }

    #[test]
    fn a_running_stage_is_on_the_live_list_and_a_finished_one_is_not() {
        let dir = TempDir::new("live");
        let set = LiveSet::new();
        let pipe = || pipe().with_live_set(Arc::clone(&set));
        // Two pipelines one after the other: the second must not leave the first's pid
        // behind, and the first's end must not take the second's pid away.
        run_bounded(pipe(), spec(vec![sh("exit 0")], LONG)).unwrap();
        assert!(set.leaders().is_empty(), "a finished stage stays on the live list");
        let pidfile = dir.path("pid");
        let script = format!("echo $$ > {}; sleep 60", pidfile.display());
        let second = pipe();
        let handle = std::thread::spawn(move || {
            run_bounded(second, spec(vec![sh(&script)], Duration::from_millis(2500)))
        });
        assert!(
            wait_until(Instant::now() + Duration::from_secs(5), || pidfile.exists()
                && !read(&pidfile).trim().is_empty()),
            "the stage never started"
        );
        let pid: u32 = read(&pidfile).trim().parse().unwrap();
        // The registration happens under the lock that spawned the stage, so this
        // read waits for it. No timing is involved.
        assert_eq!(set.leaders(), vec![pid], "the running stage is not on the live list");
        let outcome = handle.join().unwrap().unwrap();
        assert_eq!(exits(&outcome), vec![ProcExit::TimedOut]);
        assert!(set.leaders().is_empty(), "a finished stage stays on the live list");
    }

    /// Puts a kill between the spawn and the registration, on the spawning thread,
    /// and waits (bounded) for it to finish. With the registration under the spawn's
    /// lock the kill blocks until the stage is listed and then kills it. With the
    /// registration after an unlocked spawn the kill finds nothing, finishes, and the
    /// stage runs to its deadline.
    #[test]
    fn a_kill_between_the_spawn_and_the_registration_still_reaches_the_stage() {
        for round in 0..10 {
            let dir = TempDir::new("race");
            let set = LiveSet::new();
            let pipe = pipe().with_live_set(Arc::clone(&set));
            let reaper: Arc<Mutex<Option<std::thread::JoinHandle<usize>>>> =
                Arc::new(Mutex::new(None));
            let (hook_set, slot) = (Arc::clone(&set), Arc::clone(&reaper));
            let hook: Hook = Arc::new(move || {
                let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let flag = Arc::clone(&entered);
                let target = Arc::clone(&hook_set);
                let handle = std::thread::spawn(move || {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    target.kill_all()
                });
                while !entered.load(std::sync::atomic::Ordering::SeqCst) {
                    std::thread::yield_now();
                }
                let give_up = Instant::now() + Duration::from_millis(200);
                while !handle.is_finished() && Instant::now() < give_up {
                    std::thread::sleep(Duration::from_millis(1));
                }
                *slot.lock().unwrap() = Some(handle);
            });
            *HOOK.lock().unwrap() = Some((std::thread::current().id(), hook));
            let script = format!("echo $$ > {}; sleep 60", dir.path("pid").display());
            let ended = pipe.run_pipeline(&spec(vec![sh(&script)], Duration::from_secs(4)));
            *HOOK.lock().unwrap() = None;
            let killed = reaper.lock().unwrap().take().unwrap().join().unwrap();
            let outcome = ended.unwrap();
            assert_eq!(
                exits(&outcome),
                vec![ProcExit::Code(137)],
                "round {round}: the stage was not killed (kill reached {killed} groups)"
            );
            assert_eq!(killed, 1, "round {round}");
        }
    }

    #[test]
    fn no_stage_starts_once_the_set_is_closing() {
        let dir = TempDir::new("closing");
        let set = LiveSet::new();
        assert_eq!(set.kill_all(), 0);
        let pipe = pipe().with_live_set(Arc::clone(&set));
        let error = pipe
            .run_pipeline(&spec(vec![sh(&format!("touch {}", dir.path("ran").display()))], LONG))
            .unwrap_err();
        assert!(matches!(error, ProcError::Spawn { .. }), "got {error:?}");
        assert!(!dir.path("ran").exists(), "a stage started after the stop");
        assert!(set.leaders().is_empty());
    }
}
