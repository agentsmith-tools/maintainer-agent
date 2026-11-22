//! The file commands of the shell, done in this process: what they do and what they say.

use mx_install::coreutils::{chmod_x, cp, cp_r, mkdir_p, mv, rm_f, rm_rf, strerror, touch};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mx-install-cu-{tag}-{}", std::process::id()));
    drop(fs::remove_dir_all(&dir));
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn text(diag: &[u8]) -> String {
    String::from_utf8_lossy(diag).into_owned()
}

#[test]
fn rm_rf_removes_a_link_and_not_what_it_points_at_and_says_nothing_about_a_missing_path() {
    let dir = scratch("rmlink");
    fs::create_dir_all(dir.join("target/inner")).unwrap();
    fs::write(dir.join("target/inner/file"), "keep").unwrap();
    symlink(dir.join("target"), dir.join("link")).unwrap();
    let out = rm_rf(&[dir.join("link"), dir.join("never-there")]);
    assert_eq!((out.status, out.diag.as_slice()), (0, &b""[..]));
    assert!(fs::symlink_metadata(dir.join("link")).is_err());
    assert_eq!(fs::read(dir.join("target/inner/file")).unwrap(), b"keep");
    fs::create_dir_all(dir.join("tree/a/b")).unwrap();
    fs::write(dir.join("tree/a/b/f"), "").unwrap();
    symlink(dir.join("target"), dir.join("tree/a/inner-link")).unwrap();
    assert_eq!(rm_rf(&[dir.join("tree")]).status, 0);
    assert!(!dir.join("tree").exists());
    assert!(
        dir.join("target/inner/file").exists(),
        "a link inside the tree is removed, not followed"
    );
    drop(fs::remove_dir_all(&dir));
}

#[test]
fn rm_rf_names_the_path_it_could_not_remove_and_goes_on_with_the_next() {
    let dir = scratch("rmfail");
    fs::create_dir_all(dir.join("locked/sub")).unwrap();
    fs::write(dir.join("locked/sub/f"), "").unwrap();
    fs::set_permissions(dir.join("locked"), fs::Permissions::from_mode(0o555)).unwrap();
    fs::write(dir.join("other"), "").unwrap();
    let out = rm_rf(&[dir.join("locked"), dir.join("other")]);
    assert_eq!(out.status, 1);
    assert!(text(&out.diag).starts_with("rm: cannot remove '"), "{}", text(&out.diag));
    assert!(text(&out.diag).ends_with("Permission denied\n"));
    assert!(!dir.join("other").exists(), "the next path is still removed");
    fs::set_permissions(dir.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    drop(fs::remove_dir_all(&dir));
}

#[test]
fn rm_f_says_nothing_about_a_missing_path_and_refuses_a_directory_with_status_1() {
    let dir = scratch("rmf");
    assert_eq!(rm_f(&dir.join("absent")).status, 0);
    fs::create_dir_all(dir.join("d")).unwrap();
    let out = rm_f(&dir.join("d"));
    assert_eq!(out.status, 1);
    assert_eq!(
        text(&out.diag),
        format!("rm: cannot remove '{}': Is a directory\n", dir.join("d").display())
    );
    assert!(dir.join("d").is_dir());
    drop(fs::remove_dir_all(&dir));
}

#[test]
fn touch_creates_a_file_and_moves_the_time_of_one_that_is_there() {
    let dir = scratch("touch");
    assert_eq!(touch(&dir.join("new")).status, 0);
    assert_eq!(fs::metadata(dir.join("new")).unwrap().len(), 0);
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let file = fs::File::options().write(true).open(dir.join("new")).unwrap();
    file.set_modified(old).unwrap();
    drop(file);
    assert_eq!(touch(&dir.join("new")).status, 0);
    assert!(
        fs::metadata(dir.join("new")).unwrap().modified().unwrap()
            > old + Duration::from_secs(100_000)
    );
    let out = touch(&dir.join("no/such/dir/file"));
    assert_eq!(out.status, 1);
    assert!(text(&out.diag).starts_with("touch: cannot touch '"), "{}", text(&out.diag));
    drop(fs::remove_dir_all(&dir));
}

#[test]
fn cp_writes_through_a_link_keeps_the_mode_of_a_file_it_replaces_and_fills_a_directory_destination()
{
    let dir = scratch("cp");
    fs::write(dir.join("src"), "new text").unwrap();
    fs::set_permissions(dir.join("src"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(dir.join("real"), "old").unwrap();
    fs::set_permissions(dir.join("real"), fs::Permissions::from_mode(0o640)).unwrap();
    symlink(dir.join("real"), dir.join("link")).unwrap();
    assert_eq!(cp(&dir.join("src"), &dir.join("link")).status, 0);
    assert_eq!(fs::read(dir.join("real")).unwrap(), b"new text");
    assert_eq!(mode(&dir.join("real")), 0o640, "an existing file keeps its mode");
    assert!(fs::symlink_metadata(dir.join("link")).unwrap().file_type().is_symlink());
    fs::create_dir(dir.join("into")).unwrap();
    assert_eq!(cp(&dir.join("src"), &dir.join("into")).status, 0);
    assert_eq!(fs::read(dir.join("into/src")).unwrap(), b"new text");
    drop(fs::remove_dir_all(&dir));
}

#[test]
fn cp_refuses_what_the_tool_refuses_in_its_words() {
    let dir = scratch("cpfail");
    fs::write(dir.join("a"), "x").unwrap();
    let out = cp(&dir.join("absent"), &dir.join("b"));
    assert_eq!(
        text(&out.diag),
        format!("cp: cannot stat '{}': No such file or directory\n", dir.join("absent").display())
    );
    fs::create_dir(dir.join("d")).unwrap();
    let out = cp(&dir.join("d"), &dir.join("e"));
    assert_eq!(
        (out.status, text(&out.diag)),
        (1, format!("cp: -r not specified; omitting directory '{}'\n", dir.join("d").display()))
    );
    let out = cp(&dir.join("a"), &dir.join("a"));
    assert_eq!(
        text(&out.diag),
        format!("cp: '{0}' and '{0}' are the same file\n", dir.join("a").display())
    );
    symlink(dir.join("nowhere"), dir.join("dangling")).unwrap();
    let out = cp(&dir.join("a"), &dir.join("dangling"));
    assert_eq!(
        text(&out.diag),
        format!("cp: not writing through dangling symlink '{}'\n", dir.join("dangling").display())
    );
    drop(fs::remove_dir_all(&dir));
}

#[test]
fn cp_r_copies_a_tree_with_its_modes_and_links_as_links() {
    let dir = scratch("cpr");
    fs::create_dir_all(dir.join("src/sub")).unwrap();
    fs::write(dir.join("src/sub/run.sh"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(dir.join("src/sub/run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(dir.join("src/plain"), "p").unwrap();
    fs::set_permissions(dir.join("src/plain"), fs::Permissions::from_mode(0o644)).unwrap();
    symlink("plain", dir.join("src/link")).unwrap();
    assert_eq!(cp_r(&dir.join("src"), &dir.join("dst")).status, 0);
    assert_eq!(mode(&dir.join("dst/sub/run.sh")), 0o755);
    assert_eq!(mode(&dir.join("dst/plain")), 0o644);
    assert_eq!(fs::read_link(dir.join("dst/link")).unwrap(), Path::new("plain"));
    assert!(dir.join("dst/sub").is_dir());
    let out = cp_r(&dir.join("absent"), &dir.join("x"));
    assert_eq!(out.status, 1);
    drop(fs::remove_dir_all(&dir));
}

#[test]
fn mkdir_p_makes_parents_accepts_a_directory_and_refuses_a_file() {
    let dir = scratch("mkdir");
    assert_eq!(mkdir_p(&[dir.join("a/b/c"), dir.join("a/b/c")]).status, 0);
    fs::write(dir.join("file"), "").unwrap();
    let out = mkdir_p(&[dir.join("file"), dir.join("ok")]);
    assert_eq!(out.status, 1);
    assert!(text(&out.diag).starts_with("mkdir: cannot create directory '"), "{}", text(&out.diag));
    assert!(text(&out.diag).ends_with("File exists\n"));
    assert!(dir.join("ok").is_dir(), "the next path is still made");
    drop(fs::remove_dir_all(&dir));
}

#[test]
fn chmod_x_names_a_path_it_cannot_reach_and_mv_a_move_it_cannot_make() {
    let dir = scratch("chmod");
    let out = chmod_x(&[dir.join("absent")]);
    assert_eq!(
        text(&out.diag),
        format!(
            "chmod: cannot access '{}': No such file or directory\n",
            dir.join("absent").display()
        )
    );
    let out = mv(&dir.join("absent"), &dir.join("b"));
    assert_eq!(out.status, 1);
    assert!(text(&out.diag).starts_with("mv: cannot move '"), "{}", text(&out.diag));
    fs::write(dir.join("a"), "").unwrap();
    assert_eq!(mv(&dir.join("a"), &dir.join("b")).status, 0);
    assert!(dir.join("b").exists() && !dir.join("a").exists());
    drop(fs::remove_dir_all(&dir));
}

#[test]
fn strerror_drops_the_os_error_suffix() {
    assert_eq!(strerror(&std::io::Error::from_raw_os_error(13)), "Permission denied");
    assert_eq!(strerror(&std::io::Error::from_raw_os_error(2)), "No such file or directory");
}
