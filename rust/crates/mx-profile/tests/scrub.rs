//! The secret scrubber on real files. Each case builds a tree, scrubs it and
//! reads the files back. The cases mirror `tests/characterization/run-identity.sh`
//! and the scrub section of `tests/run-tests.sh`, which pin the bash.

mod common;

use common::{Scratch, real_proc};
use mx_ports::{Proc, ProcSpec, Secret};
use mx_profile::scrub::{MASK, ScrubEntry, ScrubRefusal, scrub_secret};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// 28 characters, shaped like a token and not one.
const TOKEN: &str = "ghp_FIXTURE0123456789abcdefghij";

fn secret() -> Secret {
    Secret::new(TOKEN)
}

fn leak(scratch: &Scratch, rel: &str) -> PathBuf {
    scratch.write(rel, format!("before {TOKEN} after\n"))
}

fn masked() -> Vec<u8> {
    format!("before {MASK} after\n").into_bytes()
}

fn render(entries: &[ScrubEntry]) -> String {
    let report = mx_profile::scrub::ScrubReport { entries: entries.to_vec() };
    String::from_utf8_lossy(&report.render()).into_owned()
}

fn scrub(targets: &[&Path]) -> Vec<ScrubEntry> {
    let targets: Vec<PathBuf> = targets.iter().map(|path| path.to_path_buf()).collect();
    scrub_secret(&secret(), &targets).expect("the scrub ran").entries
}

// ----- the refusals -----------------------------------------------------------

// Plan decision D15 records that run.sh ignores this refusal; the function
// itself refuses so a short needle rewrites nothing.
#[test]
fn a_secret_under_16_characters_is_refused_and_no_file_is_touched() {
    let scratch = Scratch::new("short");
    let file = scratch.write("f.txt", "has shorttoken15ch in it\n");
    for short in ["shorttoken15chr", "x", ""] {
        let refused = scrub_secret(&Secret::new(short), std::slice::from_ref(&file));
        assert_eq!(refused, Err(ScrubRefusal::SecretTooShort { chars: short.chars().count() }));
    }
    assert_eq!(scratch.read("f.txt"), b"has shorttoken15ch in it\n");
}

#[test]
fn a_secret_of_exactly_16_characters_is_accepted() {
    let scratch = Scratch::new("sixteen");
    let sixteen = "0123456789abcdef";
    assert_eq!(sixteen.chars().count(), 16);
    let file = scratch.write("f.txt", format!("a {sixteen} b\n"));
    let report = scrub_secret(&Secret::new(sixteen), std::slice::from_ref(&file)).unwrap();
    assert_eq!(report.entries, vec![ScrubEntry::Scrubbed(file)]);
    assert_eq!(scratch.read("f.txt"), format!("a {MASK} b\n").into_bytes());
}

// The length counts characters, the way python's len() and bash's ${#x} in a
// UTF-8 locale do: 8 four-byte characters are 32 bytes and still too short.
#[test]
fn the_length_counts_characters_and_not_bytes() {
    let scratch = Scratch::new("chars");
    let eight = "😀".repeat(8);
    let file = scratch.write("f.txt", eight.as_bytes());
    assert!(eight.len() >= 16);
    assert_eq!(
        scrub_secret(&Secret::new(eight), &[file]),
        Err(ScrubRefusal::SecretTooShort { chars: 8 })
    );
}

#[test]
fn an_empty_list_of_paths_is_refused() {
    assert_eq!(scrub_secret(&secret(), &[]), Err(ScrubRefusal::NoPaths));
}

#[test]
fn a_refusal_never_carries_the_secret_in_its_text() {
    let refused = scrub_secret(&Secret::new("tooshort-secret"), &[]).unwrap_err();
    let shown = format!("{refused} {refused:?}");
    assert!(!shown.contains("tooshort-secret"), "{shown}");
    let refused = scrub_secret(&Secret::new("a-short-secret"), &[PathBuf::from("/x")]).unwrap_err();
    let shown = format!("{refused} {refused:?}");
    assert!(!shown.contains("a-short-secret"), "{shown}");
}

// ----- what a scrub does ------------------------------------------------------

#[test]
fn a_text_file_holding_the_secret_gets_the_mask_and_is_reported() {
    let scratch = Scratch::new("text");
    let file = leak(&scratch, "leak.txt");
    let entries = scrub(&[&file]);
    assert_eq!(entries, vec![ScrubEntry::Scrubbed(file.clone())]);
    assert_eq!(std::fs::read(&file).unwrap(), masked());
    assert_eq!(render(&entries), format!("scrubbed the pinned token from {}\n", file.display()));
}

#[test]
fn every_occurrence_in_a_file_is_replaced() {
    let scratch = Scratch::new("many");
    let file = scratch.write("f.txt", format!("{TOKEN}\n{TOKEN}{TOKEN} x {TOKEN}"));
    scrub(&[&file]);
    assert_eq!(
        std::fs::read(&file).unwrap(),
        format!("{MASK}\n{MASK}{MASK} x {MASK}").into_bytes()
    );
}

// Python's bytes.replace takes non-overlapping matches from the left.
#[test]
fn overlapping_occurrences_are_replaced_left_to_right_without_overlap() {
    let scratch = Scratch::new("overlap");
    let needle = "a".repeat(16);
    let file = scratch.write("f.txt", "a".repeat(40));
    scrub_secret(&Secret::new(needle), std::slice::from_ref(&file)).unwrap();
    assert_eq!(
        std::fs::read(&file).unwrap(),
        format!("{MASK}{MASK}{}", "a".repeat(8)).into_bytes()
    );
}

#[test]
fn a_file_without_the_secret_is_neither_rewritten_nor_reported() {
    let scratch = Scratch::new("clean");
    let file = scratch.write("f.txt", "nothing to see\n");
    let old = std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    std::fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
    assert_eq!(scrub(&[&file]), vec![]);
    assert_eq!(
        std::fs::metadata(&file).unwrap().modified().unwrap(),
        old,
        "the file was rewritten"
    );
}

#[test]
fn the_mode_of_a_scrubbed_file_is_kept() {
    let scratch = Scratch::new("mode");
    let file = leak(&scratch, "f.txt");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
    scrub(&[&file]);
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o640);
}

// Plan decision D15 aside, this is the bash's NUL rule: the file is a disk
// image or an object file, and a byte-length change there corrupts it.
#[test]
fn a_file_with_a_nul_byte_is_left_alone_and_reported_as_binary() {
    let scratch = Scratch::new("binary");
    let file = scratch.write("disk.img", [b"bin\0".as_slice(), TOKEN.as_bytes(), b"\0"].concat());
    let before = std::fs::read(&file).unwrap();
    let entries = scrub(&[&file]);
    assert_eq!(std::fs::read(&file).unwrap(), before);
    assert_eq!(entries, vec![ScrubEntry::Binary(file.clone())]);
    // Decision P1F-2: the line still starts with "scrubbed", as in the bash.
    assert_eq!(
        render(&entries),
        format!(
            "scrubbed the pinned token from {}  BINARY, not modified: rotate the secret\n",
            file.display()
        )
    );
}

// A NUL after the needle counts, because the bash tests the whole file.
#[test]
fn a_nul_anywhere_in_the_file_makes_it_binary_even_after_the_secret() {
    let scratch = Scratch::new("binary-late");
    let file = scratch.write("late.bin", format!("{TOKEN} then a nul\0").into_bytes());
    let entries = scrub(&[&file]);
    assert_eq!(entries, vec![ScrubEntry::Binary(file)]);
}

#[test]
fn a_binary_file_without_the_secret_is_not_reported() {
    let scratch = Scratch::new("binary-clean");
    let file = scratch.write("clean.bin", b"abc\0def");
    assert_eq!(scrub(&[&file]), vec![]);
}

// ----- the walk ---------------------------------------------------------------

#[test]
fn a_directory_is_walked_and_the_four_skipped_directory_names_are_not_entered() {
    let scratch = Scratch::new("skips");
    for skipped in [".git", "node_modules", "target", "ctargets"] {
        leak(&scratch, &format!("{skipped}/inside.txt"));
        leak(&scratch, &format!("deep/{skipped}/inside.txt"));
    }
    leak(&scratch, "other/inside.txt");
    leak(&scratch, "top.txt");
    let entries = scrub(&[scratch.path()]);
    let mut got: Vec<String> = entries
        .iter()
        .map(|entry| match entry {
            ScrubEntry::Scrubbed(path) => {
                path.strip_prefix(scratch.path()).unwrap().display().to_string()
            }
            ScrubEntry::Binary(path) => panic!("unexpected binary {}", path.display()),
        })
        .collect();
    got.sort();
    assert_eq!(got, ["other/inside.txt", "top.txt"]);
    for skipped in [".git", "node_modules", "target", "ctargets"] {
        assert_eq!(
            scratch.read(&format!("{skipped}/inside.txt")),
            format!("before {TOKEN} after\n").into_bytes(),
            "{skipped} was entered"
        );
        assert_eq!(
            scratch.read(&format!("deep/{skipped}/inside.txt")),
            format!("before {TOKEN} after\n").into_bytes(),
            "deep/{skipped} was entered"
        );
    }
}

// Each name is checked on its own, so dropping one from the list fails one case.
#[test]
fn each_of_the_four_skipped_names_is_skipped_on_its_own() {
    for skipped in [".git", "node_modules", "target", "ctargets"] {
        let scratch = Scratch::new("skip-one");
        let inside = leak(&scratch, &format!("{skipped}/inside.txt"));
        let outside = leak(&scratch, "plain/inside.txt");
        scrub(&[scratch.path()]);
        assert!(
            std::fs::read(&inside).unwrap().windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()),
            "{skipped}"
        );
        assert_eq!(std::fs::read(&outside).unwrap(), masked());
    }
}

// The skip applies to directories below the root. The root is walked whatever
// it is called, and a FILE named like a skipped directory is scrubbed.
#[test]
fn a_root_named_target_is_walked_and_a_file_named_git_is_scrubbed() {
    let scratch = Scratch::new("names");
    let root = scratch.path().join("target");
    let inner = leak(&scratch, "target/inner.txt");
    let nested = leak(&scratch, "target/target/nested.txt");
    let file_named_git = leak(&scratch, "target/sub/.git");
    scrub(&[&root]);
    assert_eq!(std::fs::read(&inner).unwrap(), masked());
    assert_eq!(std::fs::read(&file_named_git).unwrap(), masked());
    assert!(String::from_utf8(std::fs::read(&nested).unwrap()).unwrap().contains(TOKEN));
}

#[test]
fn the_files_of_a_directory_are_reported_before_the_files_of_its_subdirectories() {
    let scratch = Scratch::new("order");
    leak(&scratch, "a/deep.txt");
    leak(&scratch, "z.txt");
    let entries = scrub(&[scratch.path()]);
    let names: Vec<String> = entries
        .iter()
        .map(|entry| match entry {
            ScrubEntry::Scrubbed(path) => path.file_name().unwrap().to_string_lossy().into_owned(),
            ScrubEntry::Binary(_) => panic!("binary"),
        })
        .collect();
    assert_eq!(names, ["z.txt", "deep.txt"]);
}

#[test]
fn several_paths_are_scrubbed_in_the_order_given() {
    let scratch = Scratch::new("several");
    let first = leak(&scratch, "one/f.txt");
    let second = leak(&scratch, "two/f.txt");
    let entries = scrub(&[&scratch.path().join("two"), &first]);
    assert_eq!(entries, vec![ScrubEntry::Scrubbed(second), ScrubEntry::Scrubbed(first)]);
}

#[test]
fn a_path_that_does_not_exist_is_ignored_without_a_report() {
    let scratch = Scratch::new("missing");
    let file = leak(&scratch, "f.txt");
    let entries = scrub(&[&scratch.path().join("nope"), Path::new(""), &file]);
    assert_eq!(entries, vec![ScrubEntry::Scrubbed(file)]);
}

// os.walk lists a symlink to a directory among the directories and does not
// enter it. A symlink to a file is a file and is followed, so the file it names
// is rewritten even when it lies outside the walked tree.
#[test]
fn a_symlink_to_a_directory_is_not_entered_and_a_symlink_to_a_file_is_followed() {
    let scratch = Scratch::new("links");
    let outside_dir = leak(&scratch, "outside/dir/f.txt");
    let outside_file = leak(&scratch, "outside/file.txt");
    std::fs::create_dir_all(scratch.path().join("tree")).unwrap();
    symlink(scratch.path().join("outside/dir"), scratch.path().join("tree/dirlink")).unwrap();
    symlink(&outside_file, scratch.path().join("tree/filelink")).unwrap();
    symlink(scratch.path().join("nowhere"), scratch.path().join("tree/broken")).unwrap();
    let entries = scrub(&[&scratch.path().join("tree")]);
    assert_eq!(entries, vec![ScrubEntry::Scrubbed(scratch.path().join("tree/filelink"))]);
    assert_eq!(std::fs::read(&outside_file).unwrap(), masked());
    assert!(String::from_utf8(std::fs::read(&outside_dir).unwrap()).unwrap().contains(TOKEN));
}

#[test]
fn a_symlink_given_as_the_path_is_followed_whether_it_names_a_file_or_a_directory() {
    let scratch = Scratch::new("toplinks");
    let real_dir_file = leak(&scratch, "real/f.txt");
    let real_file = leak(&scratch, "file.txt");
    symlink(scratch.path().join("real"), scratch.path().join("dirlink")).unwrap();
    symlink(&real_file, scratch.path().join("filelink")).unwrap();
    scrub(&[&scratch.path().join("dirlink"), &scratch.path().join("filelink")]);
    assert_eq!(std::fs::read(&real_dir_file).unwrap(), masked());
    assert_eq!(std::fs::read(&real_file).unwrap(), masked());
}

// Failing to read or write is silent in the bash: no report, no error.
#[test]
fn a_file_that_cannot_be_read_or_written_is_skipped_without_a_word() {
    let scratch = Scratch::new("perms");
    let unreadable = leak(&scratch, "unreadable.txt");
    let readonly = leak(&scratch, "readonly.txt");
    let fine = leak(&scratch, "fine.txt");
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o444)).unwrap();
    let as_root = std::fs::File::open(&unreadable).is_ok();
    assert!(
        !as_root,
        "this test must run as a user that the mode bits bind; it cannot prove anything as root"
    );
    let entries = scrub(&[scratch.path()]);
    assert_eq!(entries, vec![ScrubEntry::Scrubbed(fine)]);
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(String::from_utf8(std::fs::read(&unreadable).unwrap()).unwrap().contains(TOKEN));
    assert!(String::from_utf8(std::fs::read(&readonly).unwrap()).unwrap().contains(TOKEN));
}

// Decision P1F-3: the bash opens a named pipe and waits for a writer that never
// comes. The scrubber reads regular files only, so it returns.
#[test]
fn a_named_pipe_in_the_tree_is_skipped_and_the_scrub_still_finishes() {
    let scratch = Scratch::new("fifo");
    let fifo = scratch.path().join("pipe");
    let made = real_proc()
        .run(&ProcSpec::command("mkfifo", [fifo.to_str().unwrap()], Duration::from_secs(10)))
        .expect("mkfifo can run");
    assert!(made.exit.success(), "mkfifo failed: {made:?}");
    let file = leak(&scratch, "f.txt");
    let (sender, receiver) = mpsc::channel();
    let root = scratch.path().to_path_buf();
    std::thread::spawn(move || {
        let outcome = scrub_secret(&Secret::new(TOKEN), &[root]);
        drop(sender.send(outcome));
    });
    let report = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("the scrub hung on a named pipe")
        .expect("the scrub ran");
    assert_eq!(report.entries, vec![ScrubEntry::Scrubbed(file)]);
}

// The report prints a path as the bytes the directory holds, as the bash does
// under a C locale.
#[test]
fn a_file_name_that_is_not_utf8_is_reported_with_its_raw_bytes() {
    let scratch = Scratch::new("rawname");
    let name = std::ffi::OsStr::from_bytes(b"leak-\xff\xfe.txt");
    let path = scratch.path().join(name);
    std::fs::write(&path, format!("x {TOKEN} y")).unwrap();
    let report = scrub_secret(&secret(), &[scratch.path().to_path_buf()]).unwrap();
    let mut want = b"scrubbed the pinned token from ".to_vec();
    want.extend_from_slice(path.as_os_str().as_bytes());
    want.push(b'\n');
    assert_eq!(report.render(), want);
}

#[test]
fn a_scrub_that_changed_nothing_renders_nothing() {
    let scratch = Scratch::new("quiet");
    let file = scratch.write("f.txt", "clean\n");
    let report = scrub_secret(&secret(), &[file]).unwrap();
    assert!(report.entries.is_empty());
    assert!(report.render().is_empty());
}

// A trailing slash in the root stays in the printed path, as os.path.join keeps it.
#[test]
fn the_printed_path_is_the_root_as_given_joined_to_the_name() {
    let scratch = Scratch::new("joined");
    leak(&scratch, "f.txt");
    let root = format!("{}/", scratch.path().display());
    let report = scrub_secret(&secret(), &[PathBuf::from(&root)]).unwrap();
    assert_eq!(report.entries, vec![ScrubEntry::Scrubbed(PathBuf::from(format!("{root}f.txt")))]);
}
