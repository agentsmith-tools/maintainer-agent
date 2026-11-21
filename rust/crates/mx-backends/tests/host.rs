//! `RealHost` against a real directory tree.

use mx_backends::{FakeHost, Host, RealHost};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("host-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

fn file(dir: &Path, name: &str, mode: u32) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"#!/bin/sh\n").expect("a file");
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("a mode");
    path
}

fn path_of(dirs: &[&Path]) -> OsString {
    std::env::join_paths(dirs).expect("a search path")
}

#[test]
fn an_executable_regular_file_on_the_path_is_found() {
    let dir = scratch("found");
    file(&dir, "claude", 0o755);
    assert!(RealHost::new(&path_of(&[&dir])).on_path("claude"));
}

#[test]
fn a_program_is_found_in_a_later_entry_when_the_first_does_not_hold_it() {
    let first = scratch("later-first");
    let second = scratch("later-second");
    file(&second, "codex", 0o755);
    assert!(RealHost::new(&path_of(&[&first, &second])).on_path("codex"));
}

#[test]
fn a_program_that_no_entry_holds_is_not_found() {
    let dir = scratch("absent");
    file(&dir, "claude", 0o755);
    assert!(!RealHost::new(&path_of(&[&dir])).on_path("codex"));
}

#[test]
fn a_file_without_an_execute_bit_is_not_a_program() {
    let dir = scratch("noexec");
    file(&dir, "claude", 0o644);
    assert!(!RealHost::new(&path_of(&[&dir])).on_path("claude"));
}

#[test]
fn a_directory_with_the_program_name_is_not_a_program() {
    let dir = scratch("dir");
    fs::create_dir(dir.join("claude")).expect("a directory");
    assert!(!RealHost::new(&path_of(&[&dir])).on_path("claude"));
}

#[test]
fn a_symlink_to_an_executable_file_is_a_program() {
    let dir = scratch("link");
    let real = file(&dir, "real-claude", 0o755);
    symlink(&real, dir.join("claude")).expect("a symlink");
    assert!(RealHost::new(&path_of(&[&dir])).on_path("claude"));
}

#[test]
fn a_dangling_symlink_is_not_a_program() {
    let dir = scratch("dangling");
    symlink(dir.join("nowhere"), dir.join("claude")).expect("a symlink");
    assert!(!RealHost::new(&path_of(&[&dir])).on_path("claude"));
}

#[test]
fn a_program_name_with_a_slash_is_never_searched_for() {
    let dir = scratch("slash");
    file(&dir, "claude", 0o755);
    let host = RealHost::new(&path_of(&[&dir]));
    assert!(!host.on_path("./claude"));
    assert!(!host.on_path(&format!("{}/claude", dir.display())));
    assert!(!host.on_path(""));
}

#[test]
fn the_orchestrator_path_puts_local_bin_and_cargo_bin_of_home_before_the_path() {
    let home = scratch("home");
    let local = home.join(".local/bin");
    let cargo = home.join(".cargo/bin");
    let system = scratch("system");
    fs::create_dir_all(&local).expect("local bin");
    fs::create_dir_all(&cargo).expect("cargo bin");
    file(&local, "opencode", 0o755);
    file(&cargo, "cursor-agent", 0o755);
    file(&system, "claude", 0o755);
    let host = RealHost::for_orchestrator(&home, &path_of(&[&system]));
    assert!(host.on_path("opencode"), "in $HOME/.local/bin");
    assert!(host.on_path("cursor-agent"), "in $HOME/.cargo/bin");
    assert!(host.on_path("claude"), "in the inherited PATH");
    assert!(!host.on_path("codex"));
}

#[test]
fn the_plain_path_does_not_include_the_home_bin_directories() {
    let home = scratch("home-plain");
    fs::create_dir_all(home.join(".local/bin")).expect("local bin");
    file(&home.join(".local/bin"), "opencode", 0o755);
    let system = scratch("system-plain");
    assert!(!RealHost::new(&path_of(&[&system])).on_path("opencode"));
}

#[test]
fn is_file_is_true_for_a_regular_file_and_for_a_link_to_one() {
    let dir = scratch("isfile");
    let real = file(&dir, "a.json", 0o600);
    symlink(&real, dir.join("b.json")).expect("a symlink");
    let host = RealHost::new(&path_of(&[&dir]));
    assert!(host.is_file(&real));
    assert!(host.is_file(&dir.join("b.json")));
}

#[test]
fn is_file_is_false_for_a_directory_a_missing_path_and_a_dangling_link() {
    let dir = scratch("notfile");
    symlink(dir.join("nowhere"), dir.join("dangling")).expect("a symlink");
    let host = RealHost::new(&path_of(&[&dir]));
    assert!(!host.is_file(&dir));
    assert!(!host.is_file(&dir.join("missing")));
    assert!(!host.is_file(&dir.join("dangling")));
}

#[test]
fn file_len_and_read_return_the_bytes_of_a_file() {
    let dir = scratch("read");
    let path = dir.join("prompt");
    fs::write(&path, b"abc\n").expect("a file");
    let host = RealHost::new(&path_of(&[&dir]));
    assert_eq!(host.file_len(&path), Some(4));
    assert_eq!(host.read(&path), Some(b"abc\n".to_vec()));
}

#[test]
fn file_len_and_read_return_nothing_for_a_missing_file() {
    let dir = scratch("read-missing");
    let host = RealHost::new(&path_of(&[&dir]));
    assert_eq!(host.file_len(&dir.join("nope")), None);
    assert_eq!(host.read(&dir.join("nope")), None);
}

#[test]
fn a_file_that_cannot_be_read_reads_as_nothing_and_still_counts_as_a_file() {
    let dir = scratch("unreadable");
    let path = file(&dir, "key", 0o000);
    let host = RealHost::new(&path_of(&[&dir]));
    assert!(host.is_file(&path));
    // Root reads any file, so the read check only means something for a user.
    let root = fs::read(&path).is_ok();
    assert_eq!(host.read(&path).is_some(), root);
}

#[test]
fn the_fake_and_the_real_host_agree_on_a_file_and_its_length() {
    let dir = scratch("agree");
    let path = dir.join("f");
    fs::write(&path, vec![b'x'; 10]).expect("a file");
    let real = RealHost::new(&path_of(&[&dir]));
    let fake = FakeHost::new().with_file(&path, vec![b'x'; 10]);
    assert_eq!(real.is_file(&path), fake.is_file(&path));
    assert_eq!(real.file_len(&path), fake.file_len(&path));
    assert_eq!(real.read(&path), fake.read(&path));
}

#[test]
fn the_fake_refuses_a_path_nobody_declared() {
    let host = FakeHost::new().with_file("/a", "x");
    let result = std::panic::catch_unwind(|| host.is_file(Path::new("/b")));
    assert!(result.is_err());
}

#[test]
fn the_fake_answers_a_declared_absent_path_and_a_declared_unreadable_one() {
    let host = FakeHost::new().without_file("/gone").with_unreadable_file("/locked");
    assert!(!host.is_file(Path::new("/gone")));
    assert_eq!(host.read(Path::new("/gone")), None);
    assert!(host.is_file(Path::new("/locked")));
    assert_eq!(host.file_len(Path::new("/locked")), None);
    assert_eq!(host.read(Path::new("/locked")), None);
}

#[test]
fn the_length_of_a_symlinked_file_is_the_length_of_its_target() {
    // `wc -c < link` follows the link, and so must the size check on a prompt.
    let dir = scratch("len-link");
    let real = dir.join("real");
    fs::write(&real, vec![b'x'; 131_072]).expect("a file");
    symlink(&real, dir.join("link")).expect("a symlink");
    let host = RealHost::new(&path_of(&[&dir]));
    assert_eq!(host.file_len(&dir.join("link")), Some(131_072));
    assert_eq!(host.read(&dir.join("link")).map(|bytes| bytes.len()), Some(131_072));
}

#[test]
fn the_length_of_a_dangling_link_is_nothing() {
    let dir = scratch("len-dangling");
    symlink(dir.join("nowhere"), dir.join("link")).expect("a symlink");
    let host = RealHost::new(&path_of(&[&dir]));
    assert_eq!(host.file_len(&dir.join("link")), None);
}
