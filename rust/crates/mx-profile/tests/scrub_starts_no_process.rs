//! The scrubber starts no process, so no argument vector or environment of a
//! child can carry the secret.
//!
//! The kernel adds the page-fault counts of every child a process has waited
//! for to the `cminflt` and `cmajflt` fields of `/proc/self/stat`. A control
//! first runs `true` through the real process adapter and demands the counters
//! move, so the census can see a child. Then the scrub runs over a tree of files
//! and the counters must stay where they were.
//!
//! This file holds one test on purpose: another test in the same binary that
//! starts a child would move the counters for its own reasons.

mod common;

use common::{Scratch, real_proc};
use mx_ports::{Proc, ProcSpec, Secret};
use mx_profile::scrub::scrub_secret;
use std::path::PathBuf;
use std::time::Duration;

const TOKEN: &str = "ghp_FIXTURE0123456789abcdefghij";

/// `(cminflt, cmajflt)` of this process: the faults of its waited-for children.
fn child_faults() -> (u64, u64) {
    let stat = std::fs::read_to_string("/proc/self/stat").expect("/proc/self/stat is readable");
    // The command name sits in parentheses and may hold spaces, so count from the last one.
    let after_name = &stat[stat.rfind(')').expect("the stat line closes the command name") + 1..];
    let fields: Vec<&str> = after_name.split_whitespace().collect();
    // Field 3 (state) is fields[0], so field n is fields[n - 3]: cminflt is 11, cmajflt is 13.
    let read = |field: usize| fields[field - 3].parse::<u64>().expect("a numeric stat field");
    (read(11), read(13))
}

#[test]
fn scrubbing_waits_for_no_child_process_while_the_control_run_of_true_does() {
    // The control: a child that is started and waited for moves the counters.
    let before_control = child_faults();
    let control = real_proc()
        .run(&ProcSpec::command("true", Vec::<String>::new(), Duration::from_secs(10)))
        .expect("true can run");
    assert!(control.exit.success());
    let after_control = child_faults();
    assert!(
        after_control.0 > before_control.0,
        "the census cannot see a child (before {before_control:?}, after {after_control:?}), so it proves nothing"
    );

    // The scrub, over a tree with text, binary and clean files.
    let scratch = Scratch::new("census");
    for index in 0..40 {
        scratch.write(&format!("d{}/f{index}.txt", index % 4), format!("a {TOKEN} b {index}\n"));
    }
    scratch.write("bin/disk.img", [b"x\0".as_slice(), TOKEN.as_bytes()].concat());
    scratch.write("clean.txt", "nothing\n");
    let before = child_faults();
    let report = scrub_secret(&Secret::new(TOKEN), &[PathBuf::from(scratch.path())]).unwrap();
    let after = child_faults();
    assert_eq!(report.entries.len(), 41, "the scrub did not reach every file");
    assert_eq!(after, before, "the scrub waited for a child process");
    println!(
        "child fault counters before {before:?} and after {after:?} the scrub of 41 changed files"
    );
}
