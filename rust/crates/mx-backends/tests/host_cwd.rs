//! An empty entry of the search path means the current directory.
//!
//! This file holds one test because it changes the working directory of the
//! process, and no other test in the same binary may be running.

use mx_backends::{Host, RealHost};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

#[test]
fn an_empty_search_path_entry_finds_a_program_in_the_current_directory() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("host-cwd");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    let tool = dir.join("claude");
    fs::write(&tool, b"#!/bin/sh\n").expect("a file");
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).expect("a mode");
    std::env::set_current_dir(&dir).expect("cd into the scratch directory");

    let with_empty_entry = RealHost::new(&OsString::from(":/nonexistent-dir"));
    let without = RealHost::new(&OsString::from("/nonexistent-dir"));
    assert!(with_empty_entry.on_path("claude"), "the empty entry is the current directory");
    assert!(!without.on_path("claude"), "no empty entry, no current directory");
    assert!(!with_empty_entry.on_path("codex"));
}
