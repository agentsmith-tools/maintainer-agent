//! A fatal signal runs the exit trap, stops the agent's process groups, and ends
//! the process by that signal.
//!
//! The test binary starts itself as a victim (`victim`, which does nothing unless
//! `MX_TRAP_VICTIM` names a directory), because a handler that ends the process
//! cannot be tested in the process that runs the test. The victim starts through
//! `env --default-signal=...`, so the test does not depend on how cargo was
//! started: a background job of a shell leaves SIGINT ignored, and a process that
//! inherits an ignored signal keeps it ignored (as bash does). A runner in
//! Python reads how the victim ended, because a death by signal 15 and an exit
//! with 143 look the same to a shell, and only the first is a clean stop to systemd.

use mx_core::NonEmpty;
use mx_ports::{
    Pipe, PipelineOutcome, PipelineSpec, Proc, ProcError, ProcExit, ProcSpec, RealPipe, RealProc,
    Sink, StageSpec,
};
use mx_run::env::Env;
use mx_run::exec::{Adapters, Exec};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The signals the exit trap handles.
const FATAL: [(&str, i32); 6] =
    [("HUP", 1), ("INT", 2), ("TERM", 15), ("USR1", 10), ("USR2", 12), ("ALRM", 14)];

/// A scratch directory that is removed when it drops, also when an assertion fails.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.0));
    }
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        assert!(Instant::now() < deadline, "{} never appeared", path.display());
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A pipe port that writes a file when it is asked to run anything. The executor must
/// never ask it after a signal: the file is the evidence.
struct Marker(PathBuf);

impl Pipe for Marker {
    fn run_pipeline(&self, _spec: &PipelineSpec) -> Result<PipelineOutcome, ProcError> {
        std::fs::write(&self.0, "1").unwrap();
        Ok(PipelineOutcome { exits: vec![ProcExit::Code(0)] })
    }
}

/// The victim: arms work, says it is ready, then waits for a signal. `MX_TRAP_MODE`:
///
/// - `idle` (the default): loops, calling the checkpoint as the lifecycle does after
///   every external call;
/// - `main`: its own thread runs the exit trap, as `run` does when it returns;
/// - `spawn`: waits for the signal flag and then asks the executor to start a program,
///   which must not start.
#[test]
fn victim() {
    let Ok(dir) = std::env::var("MX_TRAP_VICTIM") else { return };
    let dir = PathBuf::from(dir);
    mx_run::trap::install();
    let work_dir = dir.clone();
    mx_run::trap::arm(move || {
        // Slow on purpose: the main thread must not slip past the checkpoint while
        // the signal thread is still working.
        std::fs::write(work_dir.join("started"), "1").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(work_dir.join("ran"), "1").unwrap();
    });
    // A running pipeline, which the signal thread must stop before it scrubs.
    let script = format!("echo $$ > {}/stage; sleep 60", dir.display());
    let stage = StageSpec::new(NonEmpty::new("sh".to_string(), ["-c".to_string(), script]));
    drop(std::thread::spawn(move || {
        let spec =
            PipelineSpec { stages: NonEmpty::new(stage, []), timeout: Duration::from_mins(2) };
        drop(RealPipe::new("/usr/bin:/bin", None).run_pipeline(&spec));
    }));
    wait_for(&dir.join("stage"));
    std::fs::write(dir.join("pid"), std::process::id().to_string()).unwrap();
    std::fs::write(dir.join("ready"), "1").unwrap();
    match std::env::var("MX_TRAP_MODE").as_deref() {
        Ok("main") => {
            // The pass finishes and its own thread runs the exit trap. A signal that arrives
            // meanwhile must wait for the work, and the process must end by the signal and not
            // by this thread returning.
            wait_for(&dir.join("go"));
            mx_run::trap::run_now();
            mx_run::trap::checkpoint();
            std::fs::write(dir.join("escaped"), "1").unwrap();
        }
        Ok("spawn") => {
            while !mx_run::trap::has_signalled() {
                std::thread::sleep(Duration::from_millis(2));
            }
            // The flag is up. The executor must stop here and never start the program,
            // whatever the signal thread has done so far.
            let exec = Exec {
                adapters: Adapters {
                    proc: Arc::new(RealProc::new("/usr/bin:/bin", Vec::new())),
                    pipe: Arc::new(Marker(dir.join("spawned"))),
                },
                env: Env::default(),
                cwd: None,
                locale: None,
                checkpoint: true,
            };
            let mut late = StageSpec::new(NonEmpty::new(
                "touch".to_string(),
                [dir.join("spawned").display().to_string()],
            ));
            late.stdout = Sink::Null;
            let spec =
                PipelineSpec { stages: NonEmpty::new(late, []), timeout: Duration::from_secs(30) };
            drop(exec.pipeline(&spec));
            std::fs::write(dir.join("escaped"), "1").unwrap();
        }
        _ => {
            for _ in 0..1500 {
                std::thread::sleep(Duration::from_millis(10));
                if mx_run::trap::has_signalled() {
                    mx_run::trap::checkpoint();
                    std::fs::write(dir.join("escaped"), "1").unwrap();
                }
            }
        }
    }
}

/// Runs the victim, sends it `signal`, and returns how it ended as Python's
/// `returncode` reads it: `-n` for a death by signal `n`, `143` for an exit with 143.
fn kill_victim(signal: &str, mode: &str) -> (i32, Scratch) {
    let dir = Scratch(
        std::env::temp_dir().join(format!("mx-run-sig-{signal}-{mode}-{}", std::process::id())),
    );
    drop(std::fs::remove_dir_all(&dir.0));
    std::fs::create_dir_all(&dir.0).unwrap();
    let exe = std::env::current_exe().unwrap();
    let proc = RealProc::new("/usr/bin:/bin", Vec::new());
    let runner = "import subprocess, sys; sys.stdout.write(str(subprocess.run(sys.argv[1:], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode))";
    let spec = ProcSpec::command(
        "python3",
        [
            "-I",
            "-c",
            runner,
            "env",
            "--default-signal=HUP,INT,TERM,USR1,USR2,ALRM",
            exe.to_str().unwrap(),
            "--exact",
            "victim",
            "--nocapture",
            "--test-threads=1",
        ],
        Duration::from_mins(1),
    )
    .env("MX_TRAP_VICTIM", dir.0.to_string_lossy())
    .env("MX_TRAP_MODE", mode);
    let handle = {
        let proc = proc.clone();
        std::thread::spawn(move || proc.run(&spec))
    };
    wait_for(&dir.0.join("ready"));
    if mode == "main" {
        std::fs::write(dir.0.join("go"), "1").unwrap();
        wait_for(&dir.0.join("started"));
    }
    let pid = std::fs::read_to_string(dir.0.join("pid")).unwrap();
    let sent = proc
        .run(&ProcSpec::command("kill", ["-s", signal, pid.trim()], Duration::from_secs(10)))
        .unwrap();
    assert_eq!(sent.exit, ProcExit::Code(0), "kill could not signal the victim");
    let output = handle.join().unwrap().unwrap();
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let code = text.trim().parse::<i32>().unwrap_or_else(|_| {
        panic!(
            "{signal}/{mode}: the runner printed {text:?}; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (code, dir)
}

/// What every ending must show: the process died of the signal (not an exit with
/// 128 + n, which systemd reads as a failed unit), the exit trap ran, the agent's
/// process group is gone, and the pass did not go on.
fn check_victim(signal: &str, number: i32, mode: &str) {
    let (code, dir) = kill_victim(signal, mode);
    assert_eq!(code, -number, "{signal}/{mode}: the victim must die of the signal, not exit");
    assert!(
        dir.0.join("ran").exists(),
        "{signal}/{mode}: the exit trap did not run before the process ended"
    );
    let stage: u32 = std::fs::read_to_string(dir.0.join("stage")).unwrap().trim().parse().unwrap();
    let proc_dir = format!("/proc/{stage}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Path::new(&proc_dir).exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !Path::new(&proc_dir).exists(),
        "{signal}/{mode}: the agent's process group outlived the orchestrator"
    );
    assert!(
        !dir.0.join("escaped").exists(),
        "{signal}/{mode}: the pass went on past the checkpoint"
    );
    assert!(!dir.0.join("spawned").exists(), "{signal}/{mode}: a program started after the signal");
}

#[test]
fn every_fatal_signal_runs_the_exit_trap_and_the_process_dies_of_it() {
    for (name, number) in FATAL {
        check_victim(name, number, "idle");
    }
}

#[test]
fn a_signal_while_the_main_thread_runs_the_exit_trap_waits_for_the_work_and_ends_by_the_signal() {
    check_victim("TERM", 15, "main");
}

#[test]
fn no_program_starts_after_the_signal_flag_is_up() {
    for (name, number) in [("TERM", 15), ("HUP", 1)] {
        check_victim(name, number, "spawn");
    }
}
