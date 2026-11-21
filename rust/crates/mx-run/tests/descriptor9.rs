//! Descriptor 9 of the run lock, in a process of its own.
//!
//! Descriptor numbers are process-wide, so this file holds ONE test: cargo runs
//! each integration test file as its own process, and a second test in the file
//! would race the first for descriptor 9.

use mx_ports::{Proc, ProcExit, ProcSpec, RealProc};
use mx_run::lock::{LOCK_FD, LockOpenError, open_run_lock};
use std::time::Duration;

#[test]
fn the_lock_file_is_open_on_descriptor_9_and_a_child_inherits_it() {
    let dir = std::env::temp_dir().join(format!("mx-run-fd9-{}", std::process::id()));
    drop(std::fs::remove_dir_all(&dir));
    std::fs::create_dir_all(&dir).unwrap();
    let lock_file = dir.join("run.lock");
    std::fs::write(&lock_file, "stale bytes").unwrap();

    assert!(
        std::fs::read_link("/proc/self/fd/9").is_err(),
        "descriptor 9 is already open in the test process, so this test proves nothing"
    );
    let lock = open_run_lock(&lock_file).expect("the lock file opens");
    assert_eq!(lock.raw_fd(), LOCK_FD);
    assert_eq!(LOCK_FD, 9);

    // Open on the file, and the file was truncated as `>` truncates.
    let target = std::fs::read_link("/proc/self/fd/9").unwrap();
    assert_eq!(target, lock_file.canonicalize().unwrap());
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
    drop(std::fs::remove_dir_all(&dir));
}
