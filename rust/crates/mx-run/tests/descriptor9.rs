//! Descriptor 9 of the run lock, in processes of their own.
//!
//! Descriptor numbers are process-wide, and a caller may hold descriptor 9 (the
//! machine's `with-hook-lock.sh` does, and so does every program its hook starts).
//! So no test here depends on the descriptors of the process cargo started: each
//! one starts the test binary again as a victim, through a shell that sets up
//! exactly the descriptors the case needs (closes 9, or opens it on a file), and
//! reads what the victim wrote. A victim does nothing unless its variable names a
//! directory.

use mx_ports::{Proc, ProcExit, ProcSpec, RealProc};
use mx_run::lock::{LOCK_FD, LockOpenError, open_run_lock};
use mx_run::reexec;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A scratch directory that is removed when it drops, also when an assertion fails.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("mx-run-fd9-{tag}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.0));
    }
}

/// Starts this test binary as `victim`, through `sh -c SETUP`, which runs the setup
/// (it may close or open descriptors) and then `exec`s the binary.
fn run_victim(
    victim: &str,
    variable: &str,
    dir: &Path,
    setup: &str,
    extra: &[(&str, &str)],
) -> ProcExit {
    let exe = std::env::current_exe().unwrap();
    let script = format!("{setup}; exec \"$0\" \"$@\"");
    let mut spec = ProcSpec::command(
        "sh",
        [
            "-c",
            script.as_str(),
            exe.to_str().unwrap(),
            "--exact",
            victim,
            "--nocapture",
            "--test-threads=1",
        ],
        Duration::from_mins(1),
    )
    .env(variable, dir.to_string_lossy());
    for (name, value) in extra {
        spec = spec.env(*name, *value);
    }
    let out = RealProc::new("/usr/bin:/bin", Vec::new()).run(&spec).unwrap();
    assert_eq!(
        out.exit,
        ProcExit::Code(0),
        "{victim} failed; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out.exit
}

/// Victim: descriptor 9 is closed at start; the lock lands on it.
#[test]
fn victim_lock() {
    let Ok(dir) = std::env::var("MX_FD9_LOCK") else { return };
    let dir = PathBuf::from(dir);
    let lock_file = dir.join("run.lock");
    std::fs::write(&lock_file, "stale bytes").unwrap();
    assert!(std::fs::read_link("/proc/self/fd/9").is_err(), "the setup did not close descriptor 9");
    let lock = open_run_lock(&lock_file).expect("the lock file opens");
    assert_eq!(lock.raw_fd(), LOCK_FD);
    assert_eq!(LOCK_FD, 9);

    // Open on the file, and the file was truncated as `>` truncates.
    assert_eq!(std::fs::read_link("/proc/self/fd/9").unwrap(), lock_file.canonicalize().unwrap());
    assert_eq!(std::fs::metadata(&lock_file).unwrap().len(), 0);

    // Without FD_CLOEXEC, so an exec keeps it: `fdinfo` lists O_CLOEXEC (02000000) in flags.
    let info = std::fs::read_to_string("/proc/self/fdinfo/9").unwrap();
    let flags = info
        .lines()
        .find_map(|line| line.strip_prefix("flags:"))
        .map(|text| u32::from_str_radix(text.trim(), 8).unwrap())
        .expect("fdinfo has a flags line");
    assert_eq!(flags & 0o200_0000, 0, "descriptor 9 is close-on-exec: {info}");

    // A child started through the process port sees the same file on fd 9.
    let proc = RealProc::new("/usr/bin:/bin", Vec::new());
    let spec = ProcSpec::command("sh", ["-c", "readlink /proc/$$/fd/9"], Duration::from_secs(10));
    let out = proc.run(&spec).unwrap();
    assert_eq!(out.exit, ProcExit::Code(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        lock_file.canonicalize().unwrap().to_str().unwrap(),
        "the child did not inherit descriptor 9 on the lock file"
    );

    // Descriptor 9 is taken now, so a second lock is refused and not placed elsewhere.
    let second = open_run_lock(&dir.join("other.lock"));
    assert_eq!(second.unwrap_err(), LockOpenError::Fd9Taken);
    drop(lock);
    std::fs::write(dir.join("ok"), "1").unwrap();
}

#[test]
fn the_lock_file_is_open_on_descriptor_9_and_a_child_inherits_it() {
    let dir = Scratch::new("lock");
    run_victim("victim_lock", "MX_FD9_LOCK", &dir.0, "exec 9>&-", &[]);
    assert!(dir.path("ok").exists(), "the victim did not finish");
}

/// Victim: the caller holds descriptor 9 on a file of its own. `free_descriptor_9`
/// starts the program again with it closed, and the lock lands on 9, as the bash's
/// `exec 9>` replaces whatever descriptor 9 was.
#[test]
fn victim_reexec() {
    let Ok(dir) = std::env::var("MX_FD9_REEXEC") else { return };
    let dir = PathBuf::from(dir);
    let exe = std::env::current_exe().unwrap();
    let args: Vec<OsString> = ["--exact", "victim_reexec", "--nocapture", "--test-threads=1"]
        .iter()
        .map(OsString::from)
        .collect();
    if let Some(error) = reexec::free_descriptor_9(&exe, OsStr::new("run.sh"), &args) {
        panic!("the program could not start again: {error}");
    }
    // Here only the second image arrives: the first one was replaced.
    assert_eq!(
        reexec::take_argv0(),
        Some(OsString::from("run.sh")),
        "this is the first image: descriptor 9 was not open at start, so nothing proves the replacement"
    );
    assert!(!reexec::descriptor_9_is_open(), "descriptor 9 is still open in the second image");
    let lock = open_run_lock(&dir.join("run.lock")).expect("the lock lands on descriptor 9");
    assert_eq!(lock.raw_fd(), 9);
    assert_eq!(
        std::fs::read_link("/proc/self/fd/9").unwrap(),
        dir.join("run.lock").canonicalize().unwrap()
    );
    std::fs::write(dir.join("ok"), "1").unwrap();
}

#[test]
fn a_caller_that_holds_descriptor_9_does_not_stop_the_run_the_bash_replaces_it() {
    let dir = Scratch::new("reexec");
    let setup = format!("exec 9>\"{}\"", dir.path("held").display());
    run_victim("victim_reexec", "MX_FD9_REEXEC", &dir.0, &setup, &[]);
    assert!(dir.path("held").exists());
    assert!(dir.path("ok").exists(), "the victim did not finish");
}

/// Victim: descriptor 9 is open and the program has already been started again, so it
/// does not start again, and the lock refuses.
#[test]
fn victim_still_open() {
    let Ok(dir) = std::env::var("MX_FD9_OPEN") else { return };
    let dir = PathBuf::from(dir);
    let exe = std::env::current_exe().unwrap();
    assert!(reexec::free_descriptor_9(&exe, OsStr::new("run.sh"), &[]).is_none());
    assert!(reexec::descriptor_9_is_open(), "the setup did not open descriptor 9");
    assert_eq!(open_run_lock(&dir.join("run.lock")).unwrap_err(), LockOpenError::Fd9Taken);
    std::fs::write(dir.join("ok"), "1").unwrap();
}

#[test]
fn descriptor_9_that_is_still_open_after_the_second_start_refuses_the_lock() {
    let dir = Scratch::new("open");
    let setup = format!("exec 9>\"{}\"", dir.path("held").display());
    run_victim(
        "victim_still_open",
        "MX_FD9_OPEN",
        &dir.0,
        &setup,
        &[(reexec::ARGV0_VAR, "run.sh")],
    );
    assert!(dir.path("ok").exists(), "the victim did not finish");
}

#[test]
fn a_directory_cannot_be_the_lock_file() {
    // No descriptor is claimed: the open fails first.
    let dir = std::env::temp_dir();
    match open_run_lock(&dir) {
        Err(LockOpenError::Open(reason)) => assert_eq!(reason, "Is a directory"),
        other => panic!("expected an open failure, got {other:?}"),
    }
}
