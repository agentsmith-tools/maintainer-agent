//! The program as a whole: which profile it picks, what it refuses before any
//! check runs, and the order of the sections.

mod common;

use common::{NOW, ScriptProc};
use mx_doctor::host::{FakeHost, RealHost};
use mx_doctor::{RunInput, SystemClock, run};
use mx_ports::{EffectLog, FakeClock, RealProc};
use std::path::PathBuf;

const HOME: &str = "/home/u";

fn input(vars: &[(&str, &str)], args: &[&str]) -> RunInput {
    let mut all = vec![("HOME".to_string(), HOME.to_string())];
    all.extend(vars.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())));
    RunInput {
        args: args.iter().map(|a| (*a).to_string()).collect(),
        vars: all,
        tree: PathBuf::from("/tree"),
    }
}

fn run_fake(host: &FakeHost, vars: &[(&str, &str)]) -> mx_doctor::Outcome {
    let proc = ScriptProc::default();
    let log = EffectLog::new();
    let clock = FakeClock::at(NOW, &log);
    run(&proc, &clock, host, &input(vars, &[]))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_profile_name_with_a_character_outside_the_plain_set_is_refused_with_status_64() {
    for bad in ["a;b", "a b", "a/b", "a$b", "../x", "a\nb"] {
        let outcome = run_fake(&FakeHost::new(), &[("MAINTAINER_PROFILE", bad)]);
        assert_eq!(outcome.code, 64, "{bad:?}");
        assert_eq!(
            text(&outcome.stderr),
            format!("maintainer-doctor: '{bad}' is not a plain profile name\n")
        );
        assert!(outcome.stdout.is_empty());
    }
}

#[test]
fn letters_digits_dot_underscore_and_hyphen_pass_the_name_check() {
    for ok in ["magent", "a.b", "x_y", "x-y", "A1"] {
        let outcome = run_fake(&FakeHost::new(), &[("MAINTAINER_PROFILE", ok)]);
        assert_eq!(text(&outcome.stderr), "maintainer-doctor: cannot find profile.sh\n", "{ok}");
    }
}

#[test]
fn two_deployed_profiles_and_none_named_is_refused_listing_both_in_glob_order() {
    let host = FakeHost::new();
    host.file(&format!("{HOME}/.local/share/maintainer/profiles/b/profile.env"), b"", false, 0);
    host.file(&format!("{HOME}/.local/share/maintainer/profiles/a-b/profile.env"), b"", false, 0);
    host.file(&format!("{HOME}/.local/share/maintainer/profiles/a/profile.env"), b"", false, 0);
    host.file(
        &format!("{HOME}/.local/share/maintainer/profiles/.hidden/profile.env"),
        b"",
        false,
        0,
    );
    host.dir(&format!("{HOME}/.local/share/maintainer/profiles/empty"));
    let outcome = run_fake(&host, &[]);
    assert_eq!(outcome.code, 64);
    assert_eq!(
        text(&outcome.stderr),
        "maintainer-doctor: 3 profiles are deployed (a-b a b).\n                   Set MAINTAINER_PROFILE to say which one to check.\n"
    );
}

#[test]
fn without_profile_sh_beside_the_program_or_deployed_the_run_stops_with_64() {
    let host = FakeHost::new();
    let outcome = run_fake(&host, &[]);
    assert_eq!(outcome.code, 64);
    assert_eq!(text(&outcome.stderr), "maintainer-doctor: cannot find profile.sh\n");
}

#[test]
fn an_unset_home_ends_the_run_with_the_unbound_variable_status() {
    let proc = ScriptProc::default();
    let log = EffectLog::new();
    let clock = FakeClock::at(NOW, &log);
    let bare = RunInput { args: vec![], vars: vec![], tree: PathBuf::from("/tree") };
    let outcome = run(&proc, &clock, &FakeHost::new(), &bare);
    assert_eq!(outcome.code, 127);
    assert_eq!(text(&outcome.stderr), "maintainer-doctor: line 12: HOME: unbound variable\n");
}

#[test]
fn a_missing_defaults_file_is_refused_with_the_library_message_and_status_64() {
    let host = FakeHost::new();
    host.file("/tree/lib/profile.sh", b"", false, 0);
    let outcome = run_fake(&host, &[]);
    assert_eq!(outcome.code, 64);
    assert_eq!(
        text(&outcome.stderr),
        "maintainer: lib/defaults.env not found, so no tunable has a value. Run ./install.sh.\n"
    );
}

// --- a whole run on a real scratch directory with only bash on the search path ---

fn scratch(tag: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let base = exe.parent().and_then(std::path::Path::parent).unwrap().join("mx-doctor-scratch");
    let dir = base.join(format!("{tag}-{}", std::process::id()));
    drop(std::fs::remove_dir_all(&dir));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn bash_only_path(dir: &std::path::Path) -> PathBuf {
    let bin = dir.join("only-bash");
    std::fs::create_dir_all(&bin).unwrap();
    let bash =
        ["/usr/bin/bash", "/bin/bash"].iter().find(|p| std::path::Path::new(p).exists()).unwrap();
    std::os::unix::fs::symlink(bash, bin.join("bash")).unwrap();
    bin
}

#[test]
fn a_whole_run_prints_the_eleven_headings_in_order_then_the_summary_and_exits_one_on_a_failure() {
    let dir = scratch("whole");
    let home = dir.join("home");
    let share = home.join(".local/share/maintainer");
    std::fs::create_dir_all(share.join("profiles/p")).unwrap();
    std::fs::create_dir_all(dir.join("tree/lib")).unwrap();
    std::fs::write(dir.join("tree/lib/profile.sh"), "").unwrap();
    std::fs::write(
        dir.join("tree/lib/defaults.env"),
        "DENY_WALL_MIN_RULES=100\nRED_GRACE_HOURS=6\nDOCTOR_RUN_LIMIT=40\nDOCTOR_LEASE_HOURS=12\n",
    )
    .unwrap();
    std::fs::write(share.join("profiles/p/profile.env"), "POST=off\n").unwrap();
    let path = bash_only_path(&dir);
    let proc = RealProc::new(path.as_os_str(), Vec::new());
    let vars = vec![
        ("HOME".to_string(), home.display().to_string()),
        ("PATH".to_string(), path.display().to_string()),
    ];
    let outcome = run(
        &proc,
        &SystemClock,
        &RealHost::new(),
        &RunInput { args: vec!["--quick".into()], vars, tree: dir.join("tree") },
    );
    let stdout = text(&outcome.stdout);
    let plain = stdout
        .replace("\x1b[1m", "")
        .replace("\x1b[0m", "")
        .replace("\x1b[31m", "")
        .replace("\x1b[32m", "")
        .replace("\x1b[33m", "");
    let headings: Vec<&str> =
        plain.lines().filter(|l| !l.is_empty() && !l.starts_with(' ')).collect();
    assert_eq!(
        headings,
        [
            "Deployment",
            "Profile and containment",
            "Backend",
            "Commands the prompts tell the agent to run",
            "GitHub identity",
            "Posting",
            "Repository",
            "Scheduler",
            "Container verification",
            "Sandbox",
            "Audit trail",
            "Deployed code",
        ],
        "{plain}"
    );
    assert_eq!(outcome.code, 1, "{plain}");
    assert!(plain.contains("ok, ") && plain.trim_end().ends_with("failure(s)"), "{plain}");
    assert_eq!(text(&outcome.stderr), "");
    drop(std::fs::remove_dir_all(&dir));
}

#[test]
fn a_profile_file_that_prints_has_its_output_replayed_before_the_report() {
    let dir = scratch("replay");
    let home = dir.join("home");
    let share = home.join(".local/share/maintainer");
    std::fs::create_dir_all(share.join("profiles/p")).unwrap();
    std::fs::create_dir_all(dir.join("tree/lib")).unwrap();
    std::fs::write(dir.join("tree/lib/profile.sh"), "").unwrap();
    std::fs::write(
        dir.join("tree/lib/defaults.env"),
        "DENY_WALL_MIN_RULES=100\nRED_GRACE_HOURS=6\nDOCTOR_RUN_LIMIT=40\nDOCTOR_LEASE_HOURS=12\n",
    )
    .unwrap();
    std::fs::write(share.join("profiles/p/profile.env"), "echo from-the-profile\necho oops >&2\n")
        .unwrap();
    let path = bash_only_path(&dir);
    let proc = RealProc::new(path.as_os_str(), Vec::new());
    let vars = vec![
        ("HOME".to_string(), home.display().to_string()),
        ("PATH".to_string(), path.display().to_string()),
    ];
    let outcome = run(
        &proc,
        &SystemClock,
        &RealHost::new(),
        &RunInput { args: vec!["--quick".into()], vars, tree: dir.join("tree") },
    );
    assert!(text(&outcome.stdout).starts_with("from-the-profile\n"), "{}", text(&outcome.stdout));
    assert!(text(&outcome.stderr).starts_with("oops\n"));
    drop(std::fs::remove_dir_all(&dir));
}

#[test]
fn an_ambient_variable_the_profile_does_not_set_still_reaches_the_report() {
    let dir = scratch("ambient");
    let home = dir.join("home");
    let share = home.join(".local/share/maintainer");
    std::fs::create_dir_all(share.join("profiles/p")).unwrap();
    std::fs::create_dir_all(dir.join("tree/lib")).unwrap();
    std::fs::write(dir.join("tree/lib/profile.sh"), "").unwrap();
    std::fs::write(
        dir.join("tree/lib/defaults.env"),
        "DENY_WALL_MIN_RULES=100\nRED_GRACE_HOURS=6\nDOCTOR_RUN_LIMIT=40\nDOCTOR_LEASE_HOURS=12\n",
    )
    .unwrap();
    std::fs::write(share.join("profiles/p/profile.env"), "").unwrap();
    let path = bash_only_path(&dir);
    let proc = RealProc::new(path.as_os_str(), Vec::new());
    let vars = vec![
        ("HOME".to_string(), home.display().to_string()),
        ("PATH".to_string(), path.display().to_string()),
        ("POST".to_string(), "off".to_string()),
    ];
    let outcome = run(
        &proc,
        &SystemClock,
        &RealHost::new(),
        &RunInput { args: vec!["--quick".into()], vars, tree: dir.join("tree") },
    );
    assert!(text(&outcome.stdout).contains("POST=off: this profile writes reports and drafts"));
    drop(std::fs::remove_dir_all(&dir));
}

#[test]
fn with_no_profile_deployed_the_report_names_none_deployed_and_goes_on() {
    let dir = scratch("none");
    let home = dir.join("home");
    std::fs::create_dir_all(home.join(".local/share/maintainer/profiles")).unwrap();
    std::fs::create_dir_all(dir.join("tree/lib")).unwrap();
    std::fs::write(dir.join("tree/lib/profile.sh"), "").unwrap();
    std::fs::write(
        dir.join("tree/lib/defaults.env"),
        "DENY_WALL_MIN_RULES=100\nRED_GRACE_HOURS=6\nDOCTOR_RUN_LIMIT=40\nDOCTOR_LEASE_HOURS=12\n",
    )
    .unwrap();
    let path = bash_only_path(&dir);
    let proc = RealProc::new(path.as_os_str(), Vec::new());
    let vars = vec![
        ("HOME".to_string(), home.display().to_string()),
        ("PATH".to_string(), path.display().to_string()),
    ];
    let outcome = run(
        &proc,
        &SystemClock,
        &RealHost::new(),
        &RunInput { args: vec!["--quick".into()], vars, tree: dir.join("tree") },
    );
    assert!(
        text(&outcome.stdout).contains("no profile '(none deployed)'"),
        "{}",
        text(&outcome.stdout)
    );
    drop(std::fs::remove_dir_all(&dir));
}
