//! `install --uninstall` and the `--rust` / `--rollback` pair, against real files.

mod common;

use common::{Answer, Sandbox, ScriptedProc, capture, write};
use mx_install::install;
use mx_install::machine::RealMachine;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn run(sb: &Sandbox, proc: &ScriptedProc, args: &[&str], extra: &[(&str, &str)]) -> common::Run {
    let env = sb.env_with("install.sh", args, extra);
    capture(|con| install::run(&env, &RealMachine, proc, con))
}

fn deploy(sb: &Sandbox) {
    let out = run(sb, &ScriptedProc::new(), &["--timers"], &[]);
    assert_eq!(out.code, 0, "{}", out.err);
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

const LIST: &str = "systemctl --user list-timers maintainer@* --no-pager";

#[test]
fn enabled_timers_refuse_the_uninstall_with_3_and_the_count_and_remove_nothing() {
    let sb = Sandbox::new("guard");
    deploy(&sb);
    let before = (sb.listing(&sb.home), sb.listing(&sb.co));
    let proc = ScriptedProc::new();
    proc.on(LIST, Answer::ok("maintainer@sk-review.timer n/a n/a\nsomething else\nmaintainer@a.timer maintainer@b.timer\n"));
    let out = run(&sb, &proc, &["--uninstall"], &[]);
    assert_eq!(out.code, 3);
    assert_eq!(out.out, "");
    assert_eq!(
        out.err,
        "install.sh: 2 maintainer timer(s) are enabled on this machine.\n            Removing them stops every profile, including any that\n            maintains a repository other than this one.\n\n            Re-run with MAINTAINER_UNINSTALL_YES=1 to go ahead, or\n            --dry-run to see what would be removed.\n"
    );
    assert_eq!((sb.listing(&sb.home), sb.listing(&sb.co)), before);
    assert_eq!(
        proc.of("systemctl"),
        vec!["systemctl --user show-environment".to_string(), LIST.to_string()]
    );
}

#[test]
fn a_single_enabled_timer_is_enough_to_refuse() {
    let sb = Sandbox::new("guardone");
    deploy(&sb);
    let proc = ScriptedProc::new();
    proc.on(LIST, Answer::ok("maintainer@sk-ci.timer n/a n/a\n"));
    let out = run(&sb, &proc, &["--uninstall"], &[]);
    assert_eq!(out.code, 3);
    assert!(
        out.err.starts_with("install.sh: 1 maintainer timer(s) are enabled on this machine.\n"),
        "{}",
        out.err
    );
    assert!(sb.share().join("VERSION").is_file());
    assert_eq!(
        proc.of("systemctl"),
        vec!["systemctl --user show-environment".to_string(), LIST.to_string()],
        "the refusal comes before the second question about the session"
    );
}

#[test]
fn only_the_value_1_answers_the_question() {
    for value in ["yes", "true", "0", ""] {
        let sb = Sandbox::new("only1");
        deploy(&sb);
        let proc = ScriptedProc::new();
        proc.on(LIST, Answer::ok("maintainer@sk-review.timer\n"));
        let out = run(&sb, &proc, &["--uninstall"], &[("MAINTAINER_UNINSTALL_YES", value)]);
        assert_eq!(out.code, 3, "MAINTAINER_UNINSTALL_YES={value:?}");
        assert!(sb.share().join("VERSION").is_file());
    }
}

#[test]
fn a_listing_with_no_maintainer_line_and_a_machine_with_no_session_do_not_stop_it() {
    let sb = Sandbox::new("nolive");
    deploy(&sb);
    let proc = ScriptedProc::new();
    proc.on(LIST, Answer::ok("NEXT LEFT\n0 timers listed.\n"));
    assert_eq!(run(&sb, &proc, &["--uninstall"], &[]).code, 0);
    let sb = Sandbox::new("nosession");
    deploy(&sb);
    let proc = ScriptedProc::new();
    proc.on("systemctl --user show-environment", Answer::code(1));
    proc.on(LIST, Answer::ok("maintainer@sk-review.timer\n"));
    let out = run(&sb, &proc, &["--uninstall"], &[]);
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!(
        proc.of("systemctl"),
        vec![
            "systemctl --user show-environment".to_string(),
            "systemctl --user show-environment".to_string()
        ]
    );
    assert!(!sb.units().join("maintainer@.service").exists(), "units go without a session");
}

#[test]
fn an_uninstall_calls_systemctl_in_the_order_the_bash_pins_and_removes_what_the_install_made() {
    let sb = Sandbox::new("full");
    deploy(&sb);
    let audit = sb.home.join(".local/state/sk-maint");
    fs::create_dir_all(audit.join("logs")).unwrap();
    fs::write(audit.join("audit.log"), "published\n").unwrap();
    fs::write(sb.home.join(".local/share/systemd/timers/stamp-other.timer"), "").unwrap();
    fs::write(sb.units().join("other.service"), "").unwrap();
    let proc = ScriptedProc::new();
    let out = run(&sb, &proc, &["--uninstall"], &[]);
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!(out.err, "");
    assert_eq!(
        proc.of("systemctl"),
        vec![
            "systemctl --user show-environment".to_string(),
            LIST.to_string(),
            "systemctl --user show-environment".to_string(),
            "systemctl --user disable --now maintainer@sk-issues.timer".to_string(),
            "systemctl --user disable --now maintainer@sk-review.timer".to_string(),
            "systemctl --user daemon-reload".to_string(),
        ]
    );
    assert!(!sb.share().exists());
    for name in
        ["maintainer", "maintainer-merge", "maintainer-doctor", "maintainer-repo", "maintainer-mcp"]
    {
        assert!(!sb.bin().join(name).exists(), "{name}");
    }
    assert!(
        !sb.units().join("maintainer@.service").exists()
            && !sb.units().join("maintainer@sk-review.timer").exists()
    );
    assert!(sb.units().join("other.service").exists(), "a unit that is not ours stays");
    assert!(
        sb.home.join(".local/share/systemd/timers/stamp-other.timer").exists(),
        "a foreign stamp stays"
    );
    assert!(!sb.home.join(".local/share/systemd/timers/stamp-maintainer@sk-review.timer").exists());
    assert_eq!(
        fs::read(audit.join("audit.log")).unwrap(),
        b"published\n",
        "the audit trail is never removed"
    );
    assert!(out.out.contains(&format!(
        "  removed: {}, the five commands in {}, and every scheduler entry.\n",
        sb.share().display(),
        sb.bin().display()
    )));
    assert!(
        out.out.contains("  kept:    the audit trail. Delete it yourself if you want it gone:\n")
    );
    assert!(out.out.contains(&format!(
        "           ${{MAINTAINER_STATE_DIR:-{}/.local/state/sk-maint}}\n",
        sb.home.display()
    )));
    assert!(out.out.contains(&format!(
        "           ${{MAINTAINER_STATE_DIR:-{}/.local/state/__PROFILE__-maint}}\n",
        sb.home.display()
    )));
}

#[test]
fn a_dry_run_uninstall_changes_nothing_and_asks_only_whether_a_session_exists() {
    let sb = Sandbox::new("drygone");
    deploy(&sb);
    let before = (sb.listing(&sb.home), sb.listing(&sb.co));
    let proc = ScriptedProc::new();
    proc.on(LIST, Answer::ok("maintainer@sk-review.timer\n"));
    let out = run(&sb, &proc, &["--uninstall", "--dry-run"], &[]);
    assert_eq!(out.code, 0);
    assert_eq!((sb.listing(&sb.home), sb.listing(&sb.co)), before);
    assert_eq!(proc.of("systemctl"), vec!["systemctl --user show-environment".to_string()]);
    assert!(out.out.contains(&format!("  would: rm -rf {}\n", sb.share().display())));
    assert!(
        out.out.contains("  would: systemctl --user disable --now maintainer@sk-review.timer\n")
    );
}

#[test]
fn failed_steps_are_listed_on_stderr_and_everything_else_is_still_removed() {
    let sb = Sandbox::new("failed");
    deploy(&sb);
    let proc = ScriptedProc::new();
    proc.on_program("crontab", Answer::code(0));
    proc.on(
        "systemctl --user disable --now maintainer@sk-review.timer",
        Answer::err(1, "stub: disable failed\n"),
    );
    let out = run(&sb, &proc, &["--uninstall"], &[]);
    assert_eq!(out.code, 0);
    assert_eq!(
        out.err,
        "stub: disable failed\n\n  run systemctl --user disable --now maintainer@sk-review.timer\n"
    );
    assert!(out.out.contains("  SOME STEPS FAILED. This deployment is part-removed, and that is the\n  state to fix rather than to leave:\n"));
    assert!(!sb.share().exists());
}

#[test]
fn a_failing_daemon_reload_ends_the_uninstall_with_its_status_before_any_command_is_removed() {
    // `run` without `step`: set -e ends the script. Pins current behaviour.
    let sb = Sandbox::new("reload");
    deploy(&sb);
    let proc = ScriptedProc::new();
    proc.on("systemctl --user daemon-reload", Answer::err(4, "no bus\n"));
    let out = run(&sb, &proc, &["--uninstall"], &[]);
    assert_eq!(out.code, 4);
    assert!(sb.bin().join("maintainer").exists() && sb.share().exists());
    assert!(!out.out.contains("removed:"));
}

#[test]
fn cron_is_removed_once_per_deployed_profile_through_the_installer_of_the_checkout() {
    let sb = Sandbox::new("cron");
    deploy(&sb);
    let proc = ScriptedProc::new();
    let out = run(&sb, &proc, &["--uninstall"], &[]);
    assert_eq!(out.code, 0);
    let spec: Vec<_> = proc
        .specs
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.argv.first().ends_with("install-cron.sh"))
        .cloned()
        .collect();
    assert_eq!(spec.len(), 1);
    assert_eq!(
        spec[0].argv.as_slice(),
        [format!("{}/platform/posix/install-cron.sh", sb.co.display()), "--remove".to_string()]
    );
    assert!(spec[0].env.contains(&("MAINTAINER_PROFILE".to_string(), "sk".to_string())));
}

#[test]
fn without_crontab_the_cron_step_is_skipped_silently() {
    let sb = Sandbox::new("nocron");
    deploy(&sb);
    sb.drop_tool("crontab");
    let proc = ScriptedProc::new();
    let out = run(&sb, &proc, &["--uninstall"], &[]);
    assert_eq!(out.code, 0);
    assert_eq!(out.err, "");
    assert!(proc.of("crontab").is_empty());
}

#[test]
fn on_darwin_each_plist_is_unloaded_and_removed_and_a_failed_unload_ends_the_run() {
    let sb = Sandbox::new("darwin");
    deploy(&sb);
    let agents = sb.home.join("Library/LaunchAgents");
    fs::create_dir_all(&agents).unwrap();
    for name in ["dev.maintainer.sk.review.plist", "com.other.plist"] {
        fs::write(agents.join(name), "").unwrap();
    }
    let proc = ScriptedProc::new();
    proc.on_program("uname", Answer::ok("Darwin\n"));
    let out = run(&sb, &proc, &["--uninstall"], &[]);
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!(
        proc.of("launchctl"),
        vec![format!("launchctl unload {}/dev.maintainer.sk.review.plist", agents.display())]
    );
    assert!(
        !agents.join("dev.maintainer.sk.review.plist").exists()
            && agents.join("com.other.plist").exists()
    );
}

#[test]
fn a_profile_with_no_state_dir_line_prints_an_empty_kept_line() {
    let sb = Sandbox::new("nokept");
    deploy(&sb);
    fs::write(sb.co.join("profiles/sk/profile.env"), "TASKS=\"review\"\n").unwrap();
    let out = run(&sb, &ScriptedProc::new(), &["--uninstall"], &[]);
    assert!(out.out.contains("\n           \n"), "eleven spaces: {:?}", out.out);
}

// --- --rust and --rollback ----------------------------------------------------------

fn rust_dir(sb: &Sandbox, names: &[&str]) -> std::path::PathBuf {
    let dir = sb.dir.join("rustbin");
    for name in names {
        write(&dir.join(name), &format!("#!/bin/sh\necho rust {name}\n"), 0o755);
    }
    dir
}

#[test]
fn a_rust_install_puts_the_build_at_the_installed_path_and_keeps_the_bash_original_beside_it() {
    let sb = Sandbox::new("rust");
    let dir = rust_dir(&sb, &["maintainer-doctor", "run", "alert"]);
    let out =
        run(&sb, &ScriptedProc::new(), &["--rust", "--rust-bin", &dir.to_string_lossy()], &[]);
    assert_eq!(out.code, 0, "{}", out.err);
    let doctor = sb.bin().join("maintainer-doctor");
    assert_eq!(fs::read(&doctor).unwrap(), fs::read(dir.join("maintainer-doctor")).unwrap());
    assert_eq!(
        fs::read(sb.bin().join("maintainer-doctor.sh")).unwrap(),
        fs::read(sb.co.join("bin/maintainer-doctor")).unwrap()
    );
    assert_eq!(fs::read(sb.share().join("run.sh")).unwrap(), fs::read(dir.join("run")).unwrap());
    assert_eq!(
        fs::read(sb.share().join("run.sh.sh")).unwrap(),
        fs::read(sb.co.join("lib/run.sh")).unwrap()
    );
    assert_eq!(
        fs::read(sb.share().join("alert.sh")).unwrap(),
        fs::read(dir.join("alert")).unwrap()
    );
    assert_eq!(
        fs::read(sb.share().join("alert.sh.sh")).unwrap(),
        fs::read(sb.co.join("platform/linux/alert.sh")).unwrap()
    );
    for path in [
        doctor,
        sb.bin().join("maintainer-doctor.sh"),
        sb.share().join("run.sh"),
        sb.share().join("run.sh.sh"),
    ] {
        assert_eq!(mode(&path), 0o755, "{}", path.display());
    }
    assert!(
        !sb.bin().join("maintainer-repo.sh").exists(),
        "a program with no build in the directory is untouched"
    );
    assert_eq!(
        fs::read(sb.bin().join("maintainer-repo")).unwrap(),
        fs::read(sb.co.join("bin/maintainer-repo")).unwrap()
    );
}

#[test]
fn the_rust_stamp_lists_the_sha256_of_each_installed_binary_in_the_format_sha256sum_reads() {
    use sha2::{Digest, Sha256};
    let sb = Sandbox::new("stamp");
    let dir = rust_dir(&sb, &["maintainer-doctor", "run"]);
    run(&sb, &ScriptedProc::new(), &["--rust", "--rust-bin", &dir.to_string_lossy()], &[]);
    let hex = |path: &Path| mx_install::util::hex(&Sha256::digest(fs::read(path).unwrap()));
    let want = format!(
        "{}  {}\n{}  {}\n",
        hex(&sb.bin().join("maintainer-doctor")),
        sb.bin().join("maintainer-doctor").display(),
        hex(&sb.share().join("run.sh")),
        sb.share().join("run.sh").display()
    );
    assert_eq!(fs::read_to_string(sb.share().join("RUST_STAMP")).unwrap(), want);
    assert_eq!(
        mode(&sb.share().join("RUST_STAMP")),
        0o666 & !common::umask(),
        "a redirection makes the file with the caller's umask"
    );
}

#[test]
fn a_rust_install_twice_keeps_the_bash_original_and_does_not_save_the_rust_build_as_the_backup() {
    let sb = Sandbox::new("twice");
    let dir = rust_dir(&sb, &["maintainer-doctor"]);
    let args = ["--rust", "--rust-bin", &dir.to_string_lossy()];
    run(&sb, &ScriptedProc::new(), &args, &[]);
    let out = run(&sb, &ScriptedProc::new(), &args, &[]);
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!(
        fs::read(sb.bin().join("maintainer-doctor.sh")).unwrap(),
        fs::read(sb.co.join("bin/maintainer-doctor")).unwrap()
    );
}

#[test]
fn a_rust_install_dry_run_writes_nothing_and_says_what_it_would_do() {
    let sb = Sandbox::new("rustdry");
    let dir = rust_dir(&sb, &["maintainer-doctor"]);
    let before = sb.listing(&sb.home);
    let out = run(
        &sb,
        &ScriptedProc::new(),
        &["--dry-run", "--rust", "--rust-bin", &dir.to_string_lossy()],
        &[],
    );
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!(sb.listing(&sb.home), before);
    assert!(out.out.contains(&format!(
        "  would: cp {0}/maintainer-doctor {0}/maintainer-doctor.sh\n",
        sb.bin().display()
    )));
    assert!(out.out.contains(&format!(
        "  would: install -m 755 {} {}/maintainer-doctor\n",
        dir.join("maintainer-doctor").display(),
        sb.bin().display()
    )));
    assert!(out.out.contains(&format!("  would write {}/RUST_STAMP\n", sb.share().display())));
}

#[test]
fn a_rust_directory_that_cannot_be_used_refuses_before_anything_is_written() {
    let sb = Sandbox::new("rustbad");
    let before = sb.listing(&sb.home);
    let dir = rust_dir(&sb, &["maintainer-doctor"]);
    write(&dir.join("run"), "", 0o755);
    let empty = sb.dir.join("empty");
    fs::create_dir_all(&empty).unwrap();
    write(&sb.dir.join("plain/maintainer-repo"), "x", 0o644);
    for (path, needle) in [
        (sb.dir.join("missing"), "is not a directory"),
        (
            empty,
            "none of maintainer-merge, maintainer-doctor, maintainer-repo, run, run-instance, alert is in",
        ),
        (dir, "run is not an executable file with content"),
        (sb.dir.join("plain"), "maintainer-repo is not an executable file with content"),
    ] {
        let out =
            run(&sb, &ScriptedProc::new(), &["--rust", "--rust-bin", &path.to_string_lossy()], &[]);
        assert_eq!(out.code, 1, "{needle}");
        assert!(
            out.err.starts_with("install.sh: --rust-bin: ") && out.err.contains(needle),
            "{}",
            out.err
        );
        assert_eq!(sb.listing(&sb.home), before, "{needle}");
    }
}

#[test]
fn the_new_flags_are_checked_for_sense_before_any_output() {
    let sb = Sandbox::new("rustflags");
    for (args, want) in [
        (vec!["--rust"], "install.sh: --rust needs --rust-bin DIR\n"),
        (vec!["--rust-bin", "/x"], "install.sh: --rust-bin needs --rust\n"),
        (vec!["--rust-bin"], "install.sh: --rust-bin needs a directory\n"),
        (
            vec!["--rollback", "--timers"],
            "install.sh: --rollback stands alone; only --dry-run and --force go with it\n",
        ),
        (
            vec!["--rollback", "--uninstall"],
            "install.sh: --rollback stands alone; only --dry-run and --force go with it\n",
        ),
        (
            vec!["--rust", "--rust-bin", "/x", "--uninstall"],
            "install.sh: --rust cannot go with --uninstall\n",
        ),
    ] {
        let out = run(&sb, &ScriptedProc::new(), &args, &[]);
        assert_eq!(out.code, 64, "{args:?}");
        assert_eq!(out.err, want, "{args:?}");
        assert_eq!(out.out, "");
    }
}

#[test]
fn a_rollback_restores_every_saved_original_and_removes_the_stamp() {
    let sb = Sandbox::new("rollback");
    let dir = rust_dir(&sb, &["maintainer-doctor", "run"]);
    run(&sb, &ScriptedProc::new(), &["--rust", "--rust-bin", &dir.to_string_lossy()], &[]);
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!(
        fs::read(sb.bin().join("maintainer-doctor")).unwrap(),
        fs::read(sb.co.join("bin/maintainer-doctor")).unwrap()
    );
    assert_eq!(
        fs::read(sb.share().join("run.sh")).unwrap(),
        fs::read(sb.co.join("lib/run.sh")).unwrap()
    );
    assert!(
        !sb.bin().join("maintainer-doctor.sh").exists() && !sb.share().join("run.sh.sh").exists()
    );
    assert!(!sb.share().join("RUST_STAMP").exists());
    assert!(out.out.contains("  rolled back 2 program(s) to the bash originals\n"));
    assert_eq!(mode(&sb.bin().join("maintainer-doctor")), 0o755);
}

#[test]
fn a_rollback_with_no_stamp_refuses_and_a_missing_backup_refuses_before_moving_anything() {
    let sb = Sandbox::new("rollbackrefuse");
    deploy(&sb);
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 1);
    assert_eq!(
        out.err,
        format!(
            "install.sh: nothing to roll back: {}/RUST_STAMP does not exist\n",
            sb.share().display()
        )
    );
    let dir = rust_dir(&sb, &["maintainer-doctor", "run"]);
    run(&sb, &ScriptedProc::new(), &["--rust", "--rust-bin", &dir.to_string_lossy()], &[]);
    fs::remove_file(sb.share().join("run.sh.sh")).unwrap();
    let before = sb.listing(&sb.home);
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 1);
    assert!(out.err.contains("run.sh.sh is missing"), "{}", out.err);
    assert_eq!(sb.listing(&sb.home), before, "nothing was moved");
}

#[test]
fn a_stamp_that_names_a_file_the_installer_does_not_swap_is_refused() {
    let sb = Sandbox::new("tamper");
    let dir = rust_dir(&sb, &["maintainer-doctor"]);
    run(&sb, &ScriptedProc::new(), &["--rust", "--rust-bin", &dir.to_string_lossy()], &[]);
    let line = format!("{}  /etc/passwd\n", "0".repeat(64));
    fs::write(sb.share().join("RUST_STAMP"), line).unwrap();
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 1);
    assert!(
        out.err.contains("/etc/passwd, which is not a program this installer swaps"),
        "{}",
        out.err
    );
    fs::write(sb.share().join("RUST_STAMP"), "not a stamp\n").unwrap();
    assert_eq!(run(&sb, &ScriptedProc::new(), &["--rollback"], &[]).code, 1);
}

#[test]
fn a_rollback_asks_for_flock_and_the_lock_like_an_install_and_a_dry_run_moves_nothing() {
    let sb = Sandbox::new("rollbacklock");
    let dir = rust_dir(&sb, &["maintainer-doctor"]);
    run(&sb, &ScriptedProc::new(), &["--rust", "--rust-bin", &dir.to_string_lossy()], &[]);
    let before = sb.listing(&sb.home);
    let out = run(&sb, &ScriptedProc::new(), &["--rollback", "--dry-run"], &[]);
    assert_eq!(out.code, 0);
    assert_eq!(sb.listing(&sb.home), before);
    assert!(out.out.contains(&format!(
        "  would: mv {0}/maintainer-doctor.sh {0}/maintainer-doctor\n",
        sb.bin().display()
    )));
    sb.drop_tool("flock");
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 1);
    assert!(out.err.starts_with("install.sh: flock is required"));
}

#[test]
fn an_uninstall_after_a_rust_install_also_removes_the_rollback_copies_of_the_commands() {
    let sb = Sandbox::new("rustgone");
    let dir = rust_dir(&sb, &["maintainer-doctor"]);
    run(&sb, &ScriptedProc::new(), &["--rust", "--rust-bin", &dir.to_string_lossy()], &[]);
    let out = run(&sb, &ScriptedProc::new(), &["--uninstall"], &[]);
    assert_eq!(out.code, 0, "{}", out.err);
    assert!(
        !sb.bin().join("maintainer-doctor.sh").exists()
            && !sb.bin().join("maintainer-doctor").exists()
    );
}

#[test]
fn a_plain_install_after_a_rust_install_puts_the_bash_programs_back() {
    let sb = Sandbox::new("backtobash");
    let dir = rust_dir(&sb, &["maintainer-doctor"]);
    run(&sb, &ScriptedProc::new(), &["--rust", "--rust-bin", &dir.to_string_lossy()], &[]);
    let out = run(&sb, &ScriptedProc::new(), &[], &[]);
    assert_eq!(out.code, 0);
    assert_eq!(
        fs::read(sb.bin().join("maintainer-doctor")).unwrap(),
        fs::read(sb.co.join("bin/maintainer-doctor")).unwrap()
    );
    assert!(!sb.share().join("RUST_STAMP").exists(), "the stamp no longer tells the truth");
    assert!(
        !sb.bin().join("maintainer-doctor.sh").exists(),
        "a saved original is not kept beside a bash program"
    );
}

fn rust_run(sb: &Sandbox, dir: &Path, extra: &[&str]) -> common::Run {
    let mut args = vec!["--rust", "--rust-bin"];
    let d = dir.to_string_lossy().into_owned();
    args.push(&d);
    args.extend_from_slice(extra);
    run(sb, &ScriptedProc::new(), &args, &[])
}

#[test]
fn a_rust_install_that_fails_part_way_can_be_rolled_back_to_the_bash_install() {
    let sb = Sandbox::new("midfail");
    let dir = rust_dir(&sb, &["maintainer-doctor", "run", "alert"]);
    // A directory where the staging file of the last program goes makes its install fail.
    run(&sb, &ScriptedProc::new(), &[], &[]);
    fs::create_dir_all(sb.share().join("alert.sh.new/x")).unwrap();
    let out = rust_run(&sb, &dir, &[]);
    assert_eq!(out.code, 1, "{}", out.out);
    assert!(sb.share().join("RUST_STAMP").exists(), "the stamp exists before the first swap");
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!(
        fs::read(sb.share().join("run.sh")).unwrap(),
        fs::read(sb.co.join("lib/run.sh")).unwrap()
    );
    assert_eq!(
        fs::read(sb.bin().join("maintainer-doctor")).unwrap(),
        fs::read(sb.co.join("bin/maintainer-doctor")).unwrap()
    );
    assert!(!sb.share().join("RUST_STAMP").exists());
}

#[test]
fn a_rollback_that_failed_part_way_runs_again_and_finishes() {
    let sb = Sandbox::new("rbretry");
    let dir = rust_dir(&sb, &["maintainer-doctor", "alert"]);
    rust_run(&sb, &dir, &[]);
    let alert = sb.share().join("alert.sh");
    let rust_alert = fs::read(&alert).unwrap();
    fs::remove_file(&alert).unwrap();
    fs::create_dir_all(alert.join("d")).unwrap();
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 1);
    fs::remove_dir_all(&alert).unwrap();
    fs::write(&alert, &rust_alert).unwrap();
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 0, "{}", out.err);
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 1, "a finished rollback has no stamp: {}", out.err);
}

#[test]
fn a_rust_install_refuses_a_backup_name_that_is_a_link_or_a_file_of_the_user() {
    let sb = Sandbox::new("backupname");
    let dir = rust_dir(&sb, &["maintainer-doctor", "maintainer-merge"]);
    fs::create_dir_all(sb.bin()).unwrap();
    fs::write(sb.dir.join("precious"), "keep").unwrap();
    std::os::unix::fs::symlink(sb.dir.join("precious"), sb.bin().join("maintainer-merge.sh"))
        .unwrap();
    fs::write(sb.bin().join("maintainer-doctor.sh"), "mine").unwrap();
    let before = sb.listing(&sb.home);
    let out = rust_run(&sb, &dir, &[]);
    assert_eq!(out.code, 1);
    assert!(out.err.contains("is not a saved original of this installer"), "{}", out.err);
    assert_eq!(fs::read(sb.dir.join("precious")).unwrap(), b"keep");
    assert_eq!(sb.listing(&sb.home), before, "nothing was written");
}

#[test]
fn a_symlinked_backup_name_left_by_a_stamped_install_is_still_refused_and_not_followed() {
    let sb = Sandbox::new("backuplink");
    let dir = rust_dir(&sb, &["maintainer-doctor"]);
    rust_run(&sb, &dir, &[]);
    let backup = sb.bin().join("maintainer-doctor.sh");
    fs::remove_file(&backup).unwrap();
    fs::write(sb.dir.join("precious"), "keep").unwrap();
    std::os::unix::fs::symlink(sb.dir.join("precious"), &backup).unwrap();
    assert_eq!(rust_run(&sb, &dir, &[]).code, 1);
    assert_eq!(fs::read(sb.dir.join("precious")).unwrap(), b"keep");
}

#[test]
fn a_rollback_refuses_when_a_newer_bash_install_replaced_the_stamped_build() {
    let sb = Sandbox::new("stale");
    let dir = rust_dir(&sb, &["run"]);
    rust_run(&sb, &dir, &[]);
    fs::write(sb.share().join("run.sh"), "# newer bash\n").unwrap();
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 1);
    assert!(out.err.contains("no longer matches the stamp"), "{}", out.err);
    assert_eq!(fs::read(sb.share().join("run.sh")).unwrap(), b"# newer bash\n");
}

// --- a rollback never follows or replaces a link (decision P6A-1) ----------------------------------

fn link_refusal(path: &Path) -> String {
    format!(
        "install.sh: cannot roll back: {} is a symbolic link; rollback never follows or replaces one\n",
        path.display()
    )
}

#[test]
fn a_rollback_refuses_a_link_where_an_installed_program_sits_even_when_it_points_at_the_stamped_build()
 {
    let sb = Sandbox::new("rblinkinstalled");
    let dir = rust_dir(&sb, &["maintainer-doctor", "run"]);
    rust_run(&sb, &dir, &[]);
    let doctor = sb.bin().join("maintainer-doctor");
    let copy = sb.dir.join("copy-of-the-build");
    fs::copy(&doctor, &copy).unwrap();
    fs::remove_file(&doctor).unwrap();
    std::os::unix::fs::symlink(&copy, &doctor).unwrap();
    let before = sb.listing(&sb.home);
    let copy_before = fs::read(&copy).unwrap();
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 1);
    assert_eq!(out.out, "");
    assert_eq!(out.err, link_refusal(&doctor));
    assert_eq!(sb.listing(&sb.home), before, "nothing was moved");
    assert_eq!(fs::read(&copy).unwrap(), copy_before, "the link target was left alone");
}

#[test]
fn a_rollback_refuses_a_link_where_a_saved_original_sits_and_does_not_move_it_into_place() {
    let sb = Sandbox::new("rblinkbackup");
    let dir = rust_dir(&sb, &["maintainer-doctor", "run"]);
    rust_run(&sb, &dir, &[]);
    let backup = sb.share().join("run.sh.sh");
    let real = sb.dir.join("real-original");
    fs::rename(&backup, &real).unwrap();
    std::os::unix::fs::symlink(&real, &backup).unwrap();
    let before = sb.listing(&sb.home);
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 1);
    assert_eq!(out.err, link_refusal(&backup));
    assert_eq!(sb.listing(&sb.home), before, "nothing was moved");
}

#[test]
fn a_rollback_refuses_a_dangling_link_at_a_saved_original_as_a_link_and_not_as_missing() {
    let sb = Sandbox::new("rblinkdangling");
    let dir = rust_dir(&sb, &["maintainer-doctor"]);
    rust_run(&sb, &dir, &[]);
    let backup = sb.bin().join("maintainer-doctor.sh");
    fs::remove_file(&backup).unwrap();
    std::os::unix::fs::symlink("/nonexistent/original", &backup).unwrap();
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 1);
    assert_eq!(out.err, link_refusal(&backup));
}

#[test]
fn a_rollback_refuses_a_stamp_that_is_a_link_and_keeps_what_it_points_at() {
    let sb = Sandbox::new("rblinkstamp");
    let dir = rust_dir(&sb, &["maintainer-doctor"]);
    rust_run(&sb, &dir, &[]);
    let stamp = sb.share().join("RUST_STAMP");
    let real = sb.dir.join("stamp-copy");
    fs::rename(&stamp, &real).unwrap();
    std::os::unix::fs::symlink(&real, &stamp).unwrap();
    let before = sb.listing(&sb.home);
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 1);
    assert_eq!(out.err, link_refusal(&stamp));
    assert_eq!(sb.listing(&sb.home), before);
    assert!(real.exists());
}

#[test]
fn a_rollback_refuses_a_stamp_that_lists_a_program_twice_before_it_moves_anything() {
    let sb = Sandbox::new("rbduplicate");
    let dir = rust_dir(&sb, &["maintainer-doctor", "run"]);
    rust_run(&sb, &dir, &[]);
    let stamp = sb.share().join("RUST_STAMP");
    let text = fs::read_to_string(&stamp).unwrap();
    let first = text.lines().next().unwrap().to_string();
    fs::write(&stamp, format!("{text}{first}\n")).unwrap();
    let before = sb.listing(&sb.home);
    let out = run(&sb, &ScriptedProc::new(), &["--rollback"], &[]);
    assert_eq!(out.code, 1);
    assert_eq!(
        out.err,
        format!(
            "install.sh: cannot roll back: RUST_STAMP lists {} twice\n",
            sb.bin().join("maintainer-doctor").display()
        )
    );
    assert_eq!(sb.listing(&sb.home), before, "nothing was moved");
}
