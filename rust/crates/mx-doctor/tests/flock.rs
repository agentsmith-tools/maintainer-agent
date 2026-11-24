//! The flock check on a real search path.
//!
//! `Ctx::command_v` asks bash (`command -v flock`), so for a file that is odd in some way the
//! answer is bash's own, and the doctor never runs flock. Each test builds one such file in a
//! scratch directory that holds a link to bash and nothing else, so the search path cannot
//! reach the host's flock, runs the Backend section through `RealProc` and reads what it says.
//! The same five cases are pinned for both builds in
//! `tests/characterization/doctor-core.sh` (FU1-4).

mod common;

use common::{Rig, SHARE, has_fail, has_ok, lines, out};
use mx_doctor::ctx::Ctx;
use mx_doctor::install;
use mx_doctor::report::Report;
use mx_ports::RealProc;
use std::ffi::OsString;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

const FLOCK_OK: &str = "flock on PATH";
const FLOCK_MISSING: &str = "flock is required to serialize runs and is NOT on PATH";
const FLOCK_FIX: &str = "install flock for this platform";

/// A scratch directory under the target directory, empty.
fn scratch(tag: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let base = exe.parent().and_then(Path::parent).unwrap().join("mx-doctor-flock-scratch");
    let dir = base.join(format!("{tag}-{}", std::process::id()));
    drop(std::fs::remove_dir_all(&dir));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A search directory that holds a link to bash and nothing else.
fn bin_dir(parent: &Path, name: &str) -> PathBuf {
    let bin = parent.join(name);
    std::fs::create_dir_all(&bin).unwrap();
    let bash =
        ["/usr/bin/bash", "/bin/bash"].iter().find(|p| Path::new(p).exists()).expect("no bash");
    symlink(bash, bin.join("bash")).unwrap();
    bin
}

fn write_mode(path: &Path, text: &str, mode: u32) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, PermissionsExt::from_mode(mode)).unwrap();
}

/// What the Backend section reports for `BACKEND=weird` when bash looks for flock on `dirs`.
fn backend_report(dirs: &[&Path]) -> Report {
    let rig = Rig::new(&[("BACKEND", "weird")]);
    rig.host.file(&format!("{SHARE}/backends/weird.sh"), b"", false, 0);
    let path: OsString = std::env::join_paths(dirs).unwrap();
    let proc = RealProc::new(path, Vec::new());
    let ctx = Ctx {
        proc: &proc,
        clock: &rig.clock,
        host: &rig.host,
        env: &rig.env,
        settings: &rig.settings,
        profile: &rig.profile,
        share: SHARE.to_string(),
        quick: false,
        suite_env: &rig.suite_env,
        python: std::cell::Cell::new(None),
    };
    let mut o = out();
    install::backend(&ctx, &mut o);
    o.report
}

fn assert_on_path(report: &Report) {
    assert!(has_ok(report, FLOCK_OK), "{:?}", lines(report));
    assert_eq!(report.counts(), (2, 0, 0), "{:?}", lines(report));
}

fn assert_missing(report: &Report) {
    assert!(has_fail(report, FLOCK_MISSING, FLOCK_FIX), "{:?}", lines(report));
    assert_eq!(report.counts(), (1, 0, 1), "{:?}", lines(report));
}

#[test]
fn an_executable_flock_is_on_path() {
    let dir = scratch("exec");
    let bin = bin_dir(&dir, "bin");
    write_mode(&bin.join("flock"), "#!/bin/sh\nexit 0\n", 0o755);
    assert_on_path(&backend_report(&[&bin]));
}

#[test]
fn a_flock_file_without_execute_permission_counts_as_on_path_because_bash_names_it() {
    let dir = scratch("noexec");
    let bin = bin_dir(&dir, "bin");
    write_mode(&bin.join("flock"), "", 0o644);
    assert_on_path(&backend_report(&[&bin]));
}

#[test]
fn a_directory_named_flock_counts_as_missing() {
    let dir = scratch("dir");
    let bin = bin_dir(&dir, "bin");
    std::fs::create_dir(bin.join("flock")).unwrap();
    assert_missing(&backend_report(&[&bin]));
}

#[test]
fn a_dangling_symlink_named_flock_counts_as_missing() {
    let dir = scratch("dangling");
    let bin = bin_dir(&dir, "bin");
    symlink(dir.join("no-such-flock"), bin.join("flock")).unwrap();
    assert_missing(&backend_report(&[&bin]));
}

#[test]
fn a_flock_that_exits_non_zero_counts_as_on_path_and_is_never_run() {
    let dir = scratch("exits");
    let bin = bin_dir(&dir, "bin");
    let marker = dir.join("flock-ran");
    let script = format!("#!/bin/sh\necho \"$*\" >> '{}'\nexit 3\n", marker.display());
    write_mode(&bin.join("flock"), &script, 0o755);
    assert_on_path(&backend_report(&[&bin]));
    assert!(!marker.exists(), "the doctor ran flock: {:?}", std::fs::read_to_string(&marker));
}

#[test]
fn a_non_executable_flock_ahead_of_an_executable_one_on_the_path_counts_as_on_path() {
    let dir = scratch("later");
    let first = bin_dir(&dir, "first");
    let second = dir.join("second");
    std::fs::create_dir_all(&second).unwrap();
    write_mode(&first.join("flock"), "", 0o644);
    write_mode(&second.join("flock"), "#!/bin/sh\nexit 0\n", 0o755);
    assert_on_path(&backend_report(&[&first, &second]));
}
