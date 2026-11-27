//! `mx-verify-deps` asks the `maintainer` beside its own tree, not the one first on `PATH`.
//!
//! `assemble-tree.sh` runs the program from `.rust-bin/` under a wrapper's path, so `argv[0]` is
//! `TREE/bin/mx-verify-deps`. The lease question belongs to `TREE/bin/maintainer`. Both helpers
//! here answer "held", each with its own name, so the output says which one was asked.

#![cfg(unix)]

use mx_ports::{Proc, ProcExit, ProcSpec, RealProc};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn write_exec(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn held_stub(answer: &str) -> String {
    format!("#!/bin/sh\necho {answer}\nexit 1\n")
}

fn run_binary(program: &Path, on_path: &Path, root: &Path) -> mx_ports::ProcOutput {
    let search_path = std::env::join_paths([
        on_path.as_os_str(),
        std::ffi::OsStr::new("/usr/bin"),
        std::ffi::OsStr::new("/bin"),
    ])
    .unwrap();
    let proc = RealProc::new(search_path, Vec::new());
    proc.run(
        &ProcSpec::command(
            &program.display().to_string(),
            ["verify-deps", "7", SHA],
            Duration::from_mins(1),
        )
        .env("HOME", root.display().to_string())
        .env("MAINTAINER_SLUG", "owner/repo")
        .env("MAINTAINER_STATE", root.join("state").display().to_string())
        .env("MAINTAINER_CLASSIFY_TIMEOUT", "60"),
    )
    .unwrap()
}

#[test]
fn the_lease_is_asked_of_the_helper_beside_the_wrapper_and_not_the_one_on_path() {
    let root = std::env::temp_dir().join(format!("lease-beside-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let tree = root.join("tree");
    let on_path = root.join("path-bin");
    write_exec(&tree.join("bin/maintainer"), &held_stub("held-by-tree"));
    write_exec(&on_path.join("maintainer"), &held_stub("held-by-path"));

    // The wrapper's own path as `argv[0]`, as `exec -a "$0"` leaves it.
    let wrapper: PathBuf = tree.join("bin/mx-verify-deps");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_mx-verify-deps"), &wrapper).unwrap();
    let out = run_binary(&wrapper, &on_path, &root);
    let _ = std::fs::remove_dir_all(&root);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("held-by-tree"),
        "expected the helper beside the wrapper to answer, got stderr: {stderr}"
    );
    assert!(!stderr.contains("held-by-path"), "the helper on PATH was asked: {stderr}");
    assert_ne!(out.exit, ProcExit::Code(0), "a held lease must refuse the verify");
}

/// A copied binary whose `argv[0]` directory has no helper still finds the one beside its own
/// executable, as a symlinked or copied install does.
#[test]
fn a_copied_binary_with_no_helper_beside_argv0_finds_the_one_beside_itself() {
    let root = std::env::temp_dir().join(format!("lease-copied-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let tree = root.join("tree");
    let on_path = root.join("path-bin");
    let copied = tree.join(".rust-bin/mx-verify-deps");
    std::fs::create_dir_all(copied.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_mx-verify-deps"), &copied).unwrap();
    std::fs::set_permissions(&copied, std::fs::Permissions::from_mode(0o755)).unwrap();
    write_exec(&tree.join(".rust-bin/maintainer"), &held_stub("held-by-exe-dir"));
    write_exec(&on_path.join("maintainer"), &held_stub("held-by-path"));

    // The install's own path as `argv[0]`, in a directory with no helper.
    let argv0: PathBuf = root.join("elsewhere/bin/mx-verify-deps");
    std::fs::create_dir_all(argv0.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&copied, &argv0).unwrap();
    let out = run_binary(&argv0, &on_path, &root);
    let _ = std::fs::remove_dir_all(&root);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("held-by-exe-dir"),
        "expected the helper beside the executable to answer, got stderr: {stderr}"
    );
    assert!(!stderr.contains("held-by-path"), "the helper on PATH was asked: {stderr}");
    assert_ne!(out.exit, ProcExit::Code(0), "a held lease must refuse the verify");
}
