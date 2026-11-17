//! The real host adapter, on directories the test makes for itself.

use mx_core::Fact;
use mx_doctor::host::{FakeHost, HostFs, Kind, RealHost, contains_any, glob_names, ls_count};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let exe = std::env::current_exe().unwrap();
        let base = exe.parent().and_then(std::path::Path::parent).unwrap().join("mx-doctor-host");
        let dir = base.join(format!("{tag}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn p(&self, rel: &str) -> String {
        self.0.join(rel).display().to_string()
    }
    fn write(&self, rel: &str, bytes: &[u8]) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.0));
    }
}

#[test]
fn stat_tells_files_directories_links_and_missing_paths_apart_and_follows_links() {
    let d = Dir::new("stat");
    d.write("f", b"x");
    std::fs::create_dir(d.p("sub")).unwrap();
    std::os::unix::fs::symlink(d.p("f"), d.p("link-to-f")).unwrap();
    std::os::unix::fs::symlink(d.p("nowhere"), d.p("dangling")).unwrap();
    let host = RealHost::new();
    assert_eq!(host.stat(&d.p("f")).kind, Kind::File);
    assert_eq!(host.stat(&d.p("sub")).kind, Kind::Dir);
    assert_eq!(host.stat(&d.p("link-to-f")).kind, Kind::File);
    assert_eq!(host.stat(&d.p("dangling")).kind, Kind::Missing);
    assert_eq!(host.stat(&d.p("absent")).kind, Kind::Missing);
}

#[test]
fn the_execute_and_write_bits_of_a_file_decide_executable_and_writable() {
    let d = Dir::new("bits");
    d.write("run", b"#!/bin/sh\n");
    let host = RealHost::new();
    std::fs::set_permissions(d.p("run"), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!host.stat(&d.p("run")).executable);
    std::fs::set_permissions(d.p("run"), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(host.stat(&d.p("run")).executable);
    assert!(host.stat(&d.p("run")).writable);
}

#[test]
fn the_modification_time_comes_back_in_seconds_and_nanoseconds() {
    let d = Dir::new("mtime");
    d.write("f", b"x");
    let file = std::fs::File::options().write(true).open(d.p("f")).unwrap();
    let at = std::time::UNIX_EPOCH + std::time::Duration::new(1_700_000_000, 500);
    file.set_modified(at).unwrap();
    assert_eq!(RealHost::new().stat(&d.p("f")).mtime, (1_700_000_000, 500));
}

#[test]
fn reading_a_missing_file_is_absent_and_a_directory_is_unreadable() {
    let d = Dir::new("read");
    d.write("f", b"abc");
    std::fs::create_dir(d.p("sub")).unwrap();
    let host = RealHost::new();
    assert_eq!(host.read(&d.p("f")), Fact::Known(b"abc".to_vec()));
    assert_eq!(host.read(&d.p("gone")), Fact::Absent);
    assert!(host.read(&d.p("sub")).is_unreadable());
}

#[test]
fn listing_gives_sorted_names_including_dot_files_and_absent_for_a_non_directory() {
    let d = Dir::new("list");
    d.write("b", b"");
    d.write("a", b"");
    d.write(".h", b"");
    let host = RealHost::new();
    assert_eq!(
        host.list(&d.0.display().to_string()),
        Fact::Known(vec![".h".into(), "a".into(), "b".into()])
    );
    assert_eq!(host.list(&d.p("a")), Fact::Absent);
    assert_eq!(host.list(&d.p("none")), Fact::Absent);
}

#[test]
fn ls_count_skips_dot_files_counts_a_plain_file_as_one_and_a_missing_path_as_zero() {
    let d = Dir::new("ls");
    d.write("runs/a", b"");
    d.write("runs/b", b"");
    d.write("runs/.hidden", b"");
    d.write("one", b"");
    let host = RealHost::new();
    assert_eq!(ls_count(&host, &d.p("runs")), 2);
    assert_eq!(ls_count(&host, &d.p("one")), 1);
    assert_eq!(ls_count(&host, &d.p("none")), 0);
}

#[test]
fn glob_names_match_prefix_and_suffix_hide_dot_files_and_sort_by_bytes() {
    let d = Dir::new("glob");
    for name in [
        "failed-b.json",
        "failed-a.json",
        "failed-.json",
        "failed-x.txt",
        "other.json",
        ".failed-h.json",
    ] {
        d.write(name, b"");
    }
    let host = RealHost::new();
    assert_eq!(
        glob_names(&host, &d.0.display().to_string(), "failed-", ".json"),
        ["failed-.json", "failed-a.json", "failed-b.json"]
    );
    assert_eq!(glob_names(&host, &d.0.display().to_string(), "", ".json").len(), 4);
    assert!(glob_names(&host, &d.p("none"), "", ".json").is_empty());
}

#[test]
fn counting_named_directories_includes_the_root_itself_and_does_not_follow_links() {
    let d = Dir::new("find");
    std::fs::create_dir_all(d.p("profiles")).unwrap();
    std::fs::create_dir_all(d.p("profiles/backends/profiles")).unwrap();
    std::fs::create_dir_all(d.p("elsewhere/backends")).unwrap();
    std::os::unix::fs::symlink(d.p("elsewhere"), d.p("profiles/link")).unwrap();
    let host = RealHost::new();
    assert_eq!(host.count_dirs_named(&d.0.display().to_string(), &["profiles", "backends"]), 4);
    // a root that is itself named like one of them counts too
    assert_eq!(host.count_dirs_named(&d.p("profiles"), &["profiles", "backends"]), 3);
    assert_eq!(host.count_dirs_named(&d.p("missing"), &["profiles"]), 0);
}

#[test]
fn the_scan_lists_files_holding_a_pattern_and_never_follows_a_link_met_inside_a_tree() {
    let d = Dir::new("scan");
    d.write("logs/a.log", b"xx SECRET yy");
    d.write("logs/deep/b.log", b"SECRET");
    d.write("logs/clean.log", b"nothing");
    d.write("outside/c.log", b"SECRET");
    std::os::unix::fs::symlink(d.p("outside"), d.p("logs/linked")).unwrap();
    std::os::unix::fs::symlink(d.p("outside/c.log"), d.p("logs/linkfile")).unwrap();
    let host = RealHost::new();
    let mut found = host.scan_for(&[d.p("logs")], &[b"SECRET".to_vec()]);
    found.sort();
    assert_eq!(found, [d.p("logs/a.log"), d.p("logs/deep/b.log")]);
}

#[test]
fn a_root_given_on_the_command_line_is_followed_when_it_is_a_link_or_a_file() {
    let d = Dir::new("scan-root");
    d.write("real/a.log", b"SECRET");
    d.write("file.log", b"SECRET");
    std::os::unix::fs::symlink(d.p("real"), d.p("alias")).unwrap();
    let host = RealHost::new();
    assert_eq!(host.scan_for(&[d.p("alias")], &[b"SECRET".to_vec()]), [d.p("alias/a.log")]);
    assert_eq!(host.scan_for(&[d.p("file.log")], &[b"SECRET".to_vec()]), [d.p("file.log")]);
    assert!(host.scan_for(&[d.p("missing")], &[b"SECRET".to_vec()]).is_empty());
}

#[test]
fn an_empty_pattern_matches_any_non_empty_file_and_no_empty_one() {
    assert!(contains_any(b"x", &[Vec::new()]));
    assert!(!contains_any(b"", &[Vec::new()]));
    assert!(contains_any(b"abc", &[b"zz".to_vec(), b"bc".to_vec()]));
    assert!(!contains_any(b"abc", &[b"zz".to_vec()]));
}

#[test]
fn a_temporary_directory_is_named_tmp_dot_and_ten_letters_or_digits_and_is_private() {
    let d = Dir::new("mktemp");
    let host = RealHost::new();
    let made = host.make_temp_dir(&d.0.display().to_string()).unwrap();
    let name = made.rsplit('/').next().unwrap();
    assert!(name.starts_with("tmp.") && name.len() == 14, "{name}");
    assert!(name[4..].bytes().all(|b| b.is_ascii_alphanumeric()), "{name}");
    let bits = std::fs::metadata(&made).unwrap().permissions().mode() & 0o777;
    assert_eq!(bits, 0o700);
    let second = host.make_temp_dir(&d.0.display().to_string()).unwrap();
    assert_ne!(made, second);
    assert!(host.make_temp_dir(&d.p("missing-base")).is_err());
    host.remove_tree(&made);
    assert!(!std::path::Path::new(&made).exists());
}

#[test]
fn the_fake_host_lists_scans_and_counts_like_the_real_one_for_what_the_tests_use() {
    let host = FakeHost::new();
    host.file("/s/runs/a", b"x", false, 0);
    host.file("/s/runs/.h", b"x", false, 0);
    assert_eq!(ls_count(&host, "/s/runs"), 1);
    assert_eq!(host.scan_for(&["/s".to_string()], &[b"x".to_vec()]).len(), 2);
}
