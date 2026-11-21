//! A fatal signal runs the exit trap, then ends the process by that signal.
//!
//! The test binary starts itself as a victim (`victim`, which does nothing unless
//! `MX_TRAP_VICTIM` names a directory), because a handler that ends the process
//! cannot be tested in the process that runs the test.

use mx_core::NonEmpty;
use mx_ports::{Pipe, PipelineSpec, Proc, ProcExit, ProcSpec, RealPipe, RealProc, StageSpec};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The victim: arms work, says it is ready, then waits for a signal in a loop that
/// calls the checkpoint, as the lifecycle does after every external call.
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
    if std::env::var("MX_TRAP_MODE").as_deref() == Ok("main") {
        // The pass finishes and its own thread runs the exit trap, as `run` does when it
        // returns. A signal that arrives meanwhile must wait for the work, and the process
        // must end by the signal and not by this thread returning.
        wait_for(&dir.join("go"));
        mx_run::trap::run_now();
        mx_run::trap::checkpoint();
        std::fs::write(dir.join("escaped"), "1").unwrap();
        return;
    }
    for _ in 0..1500 {
        std::thread::sleep(Duration::from_millis(10));
        if mx_run::trap::has_signalled() {
            mx_run::trap::checkpoint();
            std::fs::write(dir.join("escaped"), "1").unwrap();
        }
    }
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        assert!(Instant::now() < deadline, "{} never appeared", path.display());
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn kill_victim(signal: &str, expected_exit: i32) {
    kill_victim_in(signal, expected_exit, "idle");
}

fn kill_victim_in(signal: &str, expected_exit: i32, mode: &str) {
    let dir =
        std::env::temp_dir().join(format!("mx-run-sig-{signal}-{mode}-{}", std::process::id()));
    drop(std::fs::remove_dir_all(&dir));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = std::env::current_exe().unwrap();
    let proc = RealProc::new("/usr/bin:/bin", Vec::new());
    let spec = ProcSpec::command(
        exe.to_str().unwrap(),
        ["--exact", "victim", "--nocapture", "--test-threads=1"],
        Duration::from_mins(1),
    )
    .env("MX_TRAP_VICTIM", dir.to_string_lossy())
    .env("MX_TRAP_MODE", mode);
    let runner = {
        let proc = proc.clone();
        std::thread::spawn(move || proc.run(&spec))
    };
    wait_for(&dir.join("ready"));
    if mode == "main" {
        std::fs::write(dir.join("go"), "1").unwrap();
        wait_for(&dir.join("started"));
    }
    let pid = std::fs::read_to_string(dir.join("pid")).unwrap();
    let sent = proc
        .run(&ProcSpec::command("kill", ["-s", signal, pid.trim()], Duration::from_secs(10)))
        .unwrap();
    assert_eq!(sent.exit, ProcExit::Code(0));
    let output = runner.join().unwrap().unwrap();
    assert_eq!(
        output.exit,
        ProcExit::Code(expected_exit),
        "{signal}: the victim must end by the signal; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        dir.join("ran").exists(),
        "{signal}: the exit trap did not run before the process ended"
    );
    let stage: u32 = std::fs::read_to_string(dir.join("stage")).unwrap().trim().parse().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::path::Path::new(&format!("/proc/{stage}")).exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !std::path::Path::new(&format!("/proc/{stage}")).exists(),
        "{signal}: the agent's process group outlived the orchestrator"
    );
    assert!(!dir.join("escaped").exists(), "{signal}: the pass went on past the checkpoint");
    drop(std::fs::remove_dir_all(&dir));
}

#[test]
fn sigterm_runs_the_exit_trap_and_the_process_ends_with_143() {
    kill_victim("TERM", 143);
}

#[test]
fn sighup_runs_the_exit_trap_and_the_process_ends_with_129() {
    kill_victim("HUP", 129);
}

#[test]
fn sigint_runs_the_exit_trap_and_the_process_ends_with_130() {
    kill_victim("INT", 130);
}

#[test]
fn a_signal_while_the_main_thread_runs_the_exit_trap_waits_for_the_work_and_ends_by_the_signal() {
    kill_victim_in("TERM", 143, "main");
}
