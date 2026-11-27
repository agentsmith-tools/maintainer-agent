//! `mx-verify-deps` asks the `maintainer` beside its own tree, not the one first on `PATH`.
//!
//! `assemble-tree.sh` runs the program from `.rust-bin/` under a wrapper's path, so `argv[0]` is
//! `TREE/bin/mx-verify-deps`. The lease question belongs to `TREE/bin/maintainer`. Both helpers
//! here answer "held", each with its own name, so the output says which one was asked.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn write_exec(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn held_stub(answer: &str) -> String {
    format!("#!/bin/sh\necho {answer}\nexit 1\n")
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
    let out = Command::new(env!("CARGO_BIN_EXE_mx-verify-deps"))
        .arg0(&wrapper)
        .args(["verify-deps", "7", SHA])
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", on_path.display()))
        .env("HOME", &root)
        .env("MAINTAINER_SLUG", "owner/repo")
        .env("MAINTAINER_STATE", root.join("state"))
        .env("MAINTAINER_CLASSIFY_TIMEOUT", "60")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&root);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("held-by-tree"),
        "expected the helper beside the wrapper to answer, got stderr: {stderr}"
    );
    assert!(!stderr.contains("held-by-path"), "the helper on PATH was asked: {stderr}");
    assert_ne!(out.status.code(), Some(0), "a held lease must refuse the verify");
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
    write_exec(&tree.join(".rust-bin/maintainer"), &held_stub("held-by-exe-dir"));
    write_exec(&on_path.join("maintainer"), &held_stub("held-by-path"));

    // The install's own path as `argv[0]`, in a directory with no helper.
    let argv0: PathBuf = root.join("elsewhere/bin/mx-verify-deps");
    let out = Command::new(&copied)
        .arg0(&argv0)
        .args(["verify-deps", "7", SHA])
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", on_path.display()))
        .env("HOME", &root)
        .env("MAINTAINER_SLUG", "owner/repo")
        .env("MAINTAINER_STATE", root.join("state"))
        .env("MAINTAINER_CLASSIFY_TIMEOUT", "60")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&root);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("held-by-exe-dir"),
        "expected the helper beside the executable to answer, got stderr: {stderr}"
    );
    assert!(!stderr.contains("held-by-path"), "the helper on PATH was asked: {stderr}");
    assert_ne!(out.status.code(), Some(0), "a held lease must refuse the verify");
}
