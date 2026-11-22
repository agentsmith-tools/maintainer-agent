//! `install` against real files in a scratch directory, with every tool scripted.

mod common;

use common::{Answer, Sandbox, ScriptedProc, capture};
use mx_install::install;
use mx_install::machine::RealMachine;
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn run(sb: &Sandbox, proc: &ScriptedProc, args: &[&str]) -> common::Run {
    run_env(sb, proc, args, &[])
}

fn run_env(
    sb: &Sandbox,
    proc: &ScriptedProc,
    args: &[&str],
    extra: &[(&str, &str)],
) -> common::Run {
    let env = sb.env_with("install.sh", args, extra);
    capture(|con| install::run(&env, &RealMachine, proc, con))
}

fn mode(path: &std::path::Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn lock(sb: &Sandbox) -> String {
    // The state directory the scripted `bash` answers for profile sk.
    let dir = sb.home.join(".local/state/sk-maint");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("run.lock"), "").unwrap();
    dir.join("run.lock").to_string_lossy().into_owned()
}

fn answer_state_dir(sb: &Sandbox, proc: &ScriptedProc) {
    let env_file = sb.co.join("profiles/sk/profile.env");
    let words = format!(
        "bash -c set -u; set -o pipefail; . \"$1\" >/dev/null 2>&1; printf '%s' \"${{STATE_DIR:-}}\" bash {}",
        env_file.display()
    );
    proc.on(&words, Answer::ok(&sb.home.join(".local/state/sk-maint").to_string_lossy()));
}

#[test]
fn a_plain_install_copies_every_file_with_its_mode_and_makes_the_commands_executable() {
    let sb = Sandbox::new("copies");
    let proc = ScriptedProc::new();
    let out = run(&sb, &proc, &[]);
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!(out.err, "");
    for name in [
        "run.sh",
        "prose-style.md",
        "preamble-core.md",
        "profile.sh",
        "failure-shapes.json",
        "defaults.env",
    ] {
        assert_eq!(
            fs::read(sb.share().join(name)).unwrap(),
            fs::read(sb.co.join("lib").join(name)).unwrap(),
            "{name}"
        );
    }
    assert_eq!(mode(&sb.share().join("run.sh")), 0o755);
    assert_eq!(mode(&sb.share().join("prose-style.md")), 0o644);
    for name in
        ["maintainer", "maintainer-merge", "maintainer-doctor", "maintainer-repo", "maintainer-mcp"]
    {
        assert_eq!(mode(&sb.bin().join(name)), 0o755, "{name}");
    }
    for name in ["run-instance.sh", "alert.sh"] {
        assert_eq!(mode(&sb.share().join(name)), 0o755, "{name}");
    }
    assert_eq!(fs::read(sb.share().join("scripts/a.py")).unwrap(), b"print('a.py')\n");
    assert!(sb.share().join("backends/claude.sh").is_file());
    assert!(sb.share().join("profiles/sk/profile.env").is_file());
    assert!(
        !sb.share().join("profiles/_template").exists(),
        "the template is scaffolding and is not deployed"
    );
    assert!(sb.units().join("maintainer@.service").is_file());
    assert!(sb.units().join("maintainer@sk-review.timer").is_file());
    assert_eq!(mode(&sb.units().join("maintainer@.service")), 0o644);
}

#[test]
fn a_plain_install_prints_the_banner_the_wall_lines_and_the_hint_and_leaves_the_timers_alone() {
    let sb = Sandbox::new("banner");
    let proc = ScriptedProc::new();
    let out = run(&sb, &proc, &[]);
    let want = format!(
        "  deploying from {}\n  minted a receipt-signing key\n  rendered\n  platform: Linux (systemd user units)\n  timers not touched (pass --timers to enable them)\n  done\n  \n  next: maintainer-doctor        # checks the install by running it, not by listing files\n        ./install.sh --timers     # enable the systemd timers\n",
        sb.co.display()
    );
    assert_eq!(out.out, want);
    assert!(proc.of("systemctl").is_empty());
}

#[test]
fn a_second_install_replaces_profiles_and_backends_and_keeps_the_key() {
    let sb = Sandbox::new("second");
    let proc = ScriptedProc::new();
    run(&sb, &proc, &[]);
    let key = fs::read(sb.share().join("receipt.key")).unwrap();
    fs::write(sb.share().join("profiles/stale.txt"), "").unwrap();
    fs::write(sb.share().join("backends/stale.sh"), "").unwrap();
    let out = run(&sb, &proc, &[]);
    assert_eq!(out.code, 0);
    assert!(!out.out.contains("minted"));
    assert_eq!(fs::read(sb.share().join("receipt.key")).unwrap(), key);
    assert!(!sb.share().join("profiles/stale.txt").exists());
    assert!(!sb.share().join("backends/stale.sh").exists());
    assert!(
        !sb.share().join("profiles/profiles").exists()
            && !sb.share().join("backends/backends").exists()
    );
}

#[test]
fn the_key_is_64_lower_case_hex_characters_with_no_newline_and_mode_600_and_is_never_printed() {
    let sb = Sandbox::new("key");
    let proc = ScriptedProc::new();
    let out = run(&sb, &proc, &[]);
    let key = fs::read_to_string(sb.share().join("receipt.key")).unwrap();
    assert_eq!(key.len(), 64);
    assert!(key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{key}");
    assert_eq!(mode(&sb.share().join("receipt.key")), 0o600);
    assert!(!out.out.contains(&key) && !out.err.contains(&key));
    let other = Sandbox::new("key2");
    let again = run(&other, &ScriptedProc::new(), &[]);
    assert_eq!(again.code, 0);
    assert_ne!(
        fs::read_to_string(other.share().join("receipt.key")).unwrap(),
        key,
        "two homes must mint two keys"
    );
}

#[test]
fn an_existing_key_keeps_its_content_and_its_mode() {
    let sb = Sandbox::new("keykept");
    fs::create_dir_all(sb.share()).unwrap();
    fs::write(sb.share().join("receipt.key"), "existing-key-material").unwrap();
    fs::set_permissions(sb.share().join("receipt.key"), fs::Permissions::from_mode(0o640)).unwrap();
    let out = run(&sb, &ScriptedProc::new(), &[]);
    assert_eq!(out.code, 0);
    assert!(!out.out.contains("minted"));
    assert_eq!(fs::read(sb.share().join("receipt.key")).unwrap(), b"existing-key-material");
    assert_eq!(mode(&sb.share().join("receipt.key")), 0o640);
}

#[test]
fn the_version_stamp_has_four_lines_naming_the_checkout() {
    let sb = Sandbox::new("version");
    let proc = ScriptedProc::new();
    proc.on(&format!("git -C {} rev-parse --short HEAD", sb.co.display()), Answer::ok("abc1234\n"));
    run(&sb, &proc, &[]);
    assert_eq!(
        fs::read_to_string(sb.share().join("VERSION")).unwrap(),
        format!(
            "version=v9.9.9\ncommit=abc1234\ninstalled_at=2026-01-01T00:00:00+00:00\ninstalled_from={}\n",
            sb.co.display()
        )
    );
    assert_eq!(
        proc.of("git"),
        vec![
            format!("git -C {} describe --tags --always --dirty", sb.co.display()),
            format!("git -C {} rev-parse --short HEAD", sb.co.display()),
        ]
    );
    assert_eq!(proc.of("date"), vec!["date -Is".to_string()]);
}

#[test]
fn a_tree_git_cannot_describe_is_stamped_unknown_and_a_missing_date_leaves_a_blank() {
    let sb = Sandbox::new("unknown");
    let proc = ScriptedProc::new();
    proc.on_program("git", Answer::err(128, "fatal: not a git repository\n"));
    proc.on_program("date", Answer::missing());
    let out = run(&sb, &proc, &[]);
    assert_eq!(out.code, 0);
    assert_eq!(
        fs::read_to_string(sb.share().join("VERSION")).unwrap(),
        format!(
            "version=unknown\ncommit=unknown\ninstalled_at=\ninstalled_from={}\n",
            sb.co.display()
        )
    );
    assert!(out.err.contains("install.sh: line 143: date: command not found"), "{}", out.err);
    assert!(!out.err.contains("fatal"), "git's own message is discarded");
}

#[test]
fn a_missing_flock_refuses_before_anything_is_written_even_for_a_dry_run() {
    for flags in [&[][..], &["--dry-run"][..]] {
        let sb = Sandbox::new("noflock");
        sb.drop_tool("flock");
        let before = sb.listing(&sb.home);
        let proc = ScriptedProc::new();
        let out = run(&sb, &proc, flags);
        assert_eq!(out.code, 1);
        assert_eq!(out.out, "");
        assert_eq!(
            out.err,
            "install.sh: flock is required to install and protect active runs; install flock and ensure it is on PATH.\n"
        );
        assert_eq!(sb.listing(&sb.home), before);
        assert_eq!(
            proc.log(),
            vec!["uname -s".to_string()],
            "only the platform is asked before the flock check"
        );
    }
}

#[test]
fn a_held_lock_refuses_with_69_and_writes_nothing_not_even_a_directory() {
    let sb = Sandbox::new("busy");
    let proc = ScriptedProc::new();
    let lock_path = lock(&sb);
    answer_state_dir(&sb, &proc);
    proc.on(&format!("flock -n {lock_path} true"), Answer::code(1));
    let before = sb.listing(&sb.home);
    let out = run(&sb, &proc, &["--timers"]);
    assert_eq!(out.code, 69);
    assert_eq!(out.out, "");
    assert!(out.err.starts_with("install.sh: a run is in flight for: sk\n"), "{}", out.err);
    assert_eq!(out.err.lines().count(), 5);
    assert_eq!(sb.listing(&sb.home), before);
    assert!(!sb.share().exists() && !sb.bin().exists() && !sb.units().exists());
    assert!(proc.of("systemctl").is_empty());
    assert_eq!(proc.of("flock"), vec![format!("flock -n {lock_path} true")]);
}

#[test]
fn the_lock_is_asked_only_for_a_profile_whose_state_has_a_lock_file_and_never_with_force_or_dry_run()
 {
    let sb = Sandbox::new("askonly");
    let proc = ScriptedProc::new();
    answer_state_dir(&sb, &proc);
    let out = run(&sb, &proc, &[]);
    assert_eq!(out.code, 0);
    assert!(proc.of("flock").is_empty(), "no lock file, no question");
    let lock_path = lock(&sb);
    proc.on(&format!("flock -n {lock_path} true"), Answer::code(1));
    for flag in ["--force", "--dry-run"] {
        let before = proc.of("flock").len();
        let out = run(&sb, &proc, &[flag]);
        assert_eq!(out.code, 0, "{flag}: {}", out.err);
        assert_eq!(proc.of("flock").len(), before, "{flag} must not ask the lock");
        assert_eq!(
            out.out.contains("would"),
            flag == "--dry-run",
            "{flag}: only a dry run says would: {}",
            out.out
        );
    }
    let free = Sandbox::new("free");
    let proc2 = ScriptedProc::new();
    answer_state_dir(&free, &proc2);
    let lock_path = lock(&free);
    let out = run(&free, &proc2, &[]);
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!(proc2.of("flock"), vec![format!("flock -n {lock_path} true")]);
}

#[test]
fn a_profile_that_cannot_be_read_is_skipped_by_the_busy_check_and_the_install_goes_ahead() {
    // Pins current behaviour: the check fails open for a profile it cannot read.
    let sb = Sandbox::new("unreadable");
    let proc = ScriptedProc::new();
    lock(&sb);
    let env_file = sb.co.join("profiles/sk/profile.env");
    proc.on(
        &format!("bash -c set -u; set -o pipefail; . \"$1\" >/dev/null 2>&1; printf '%s' \"${{STATE_DIR:-}}\" bash {}", env_file.display()),
        Answer::ok(""),
    );
    proc.on_program("flock", Answer::code(1));
    let out = run(&sb, &proc, &[]);
    assert_eq!(out.code, 0);
    assert!(proc.of("flock").is_empty());
}

#[test]
fn a_dry_run_changes_nothing_and_starts_only_the_probes() {
    let sb = Sandbox::new("dry");
    let proc = ScriptedProc::new();
    let before = (sb.listing(&sb.home), sb.listing(&sb.co));
    let out = run(&sb, &proc, &["--dry-run", "--timers"]);
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!((sb.listing(&sb.home), sb.listing(&sb.co)), before);
    assert_eq!(
        proc.log(),
        vec!["uname -s".to_string()],
        "a dry run asks the platform and nothing else"
    );
    assert!(out.out.contains(&format!(
        "  would: mkdir -p {} {} {}\n",
        sb.share().display(),
        sb.bin().display(),
        sb.units().display()
    )));
    assert!(out.out.contains(&format!("  would stamp {}/VERSION\n", sb.share().display())));
    assert!(
        out.out.contains(&format!("  would mint {}/receipt.key if absent\n", sb.share().display()))
    );
    assert!(out.out.contains("  would: systemctl --user enable --now maintainer@sk-review.timer\n  enabled maintainer@sk-review.timer\n"));
    assert!(!out.out.contains("rendered"), "no wall is generated in a dry run");
}

#[test]
fn a_dry_run_lists_the_walls_of_an_existing_deployment() {
    let sb = Sandbox::new("drywalls");
    run(&sb, &ScriptedProc::new(), &[]);
    let out = run(&sb, &ScriptedProc::new(), &["--dry-run"]);
    assert!(
        out.out.contains("  would generate sk/settings.json and settings-rehearsal.json\n"),
        "{}",
        out.out
    );
}

#[test]
fn the_walls_are_rendered_once_per_deployed_profile_with_the_profile_directory_and_home() {
    let sb = Sandbox::new("walls");
    let proc = ScriptedProc::new();
    run(&sb, &proc, &[]);
    assert_eq!(
        proc.of("python3"),
        vec![format!(
            "python3 {}/scripts/render-settings.py {}/profiles/sk {}",
            sb.co.display(),
            sb.share().display(),
            sb.home.display()
        )]
    );
}

#[test]
fn a_failing_wall_ends_with_1_before_the_commands_are_copied() {
    let sb = Sandbox::new("wallfail");
    let proc = ScriptedProc::new();
    proc.on_program("python3", Answer::err(3, "boom\n"));
    let out = run(&sb, &proc, &[]);
    assert_eq!(out.code, 1);
    assert_eq!(out.err, "boom\ninstall.sh: generating the deny wall failed\n");
    assert!(!sb.bin().join("maintainer").exists());
}

#[test]
fn a_missing_python3_is_the_same_failure_in_the_words_of_the_shell() {
    let sb = Sandbox::new("nopython");
    let proc = ScriptedProc::new();
    proc.on_program("python3", Answer::missing());
    let out = run(&sb, &proc, &[]);
    assert_eq!(out.code, 1);
    assert_eq!(
        out.err,
        format!(
            "{}/install.sh: line 233: python3: command not found\ninstall.sh: generating the deny wall failed\n",
            sb.co.display()
        )
    );
}

#[test]
fn a_checkout_with_no_profile_exits_1_after_copying_and_says_where_run_sh_looks() {
    let sb = Sandbox::new("noprofile");
    fs::remove_dir_all(sb.co.join("profiles/sk")).unwrap();
    let out = run(&sb, &ScriptedProc::new(), &[]);
    assert_eq!(out.code, 1);
    assert_eq!(out.err, "install.sh: profiles did not land where run.sh expects\n");
    assert!(!out.out.contains("  done"));
    assert!(sb.share().join("run.sh").is_file(), "the copy already happened, as in the bash");
}

#[test]
fn a_profile_directory_with_no_profile_env_does_not_count() {
    let sb = Sandbox::new("noenv");
    fs::remove_file(sb.co.join("profiles/sk/profile.env")).unwrap();
    let out = run(&sb, &ScriptedProc::new(), &[]);
    assert_eq!(out.code, 1);
    assert!(sb.share().join("profiles/sk/prompts/review.md").is_file());
}

#[test]
fn timers_are_stamped_before_they_are_enabled_and_each_enable_is_said() {
    let sb = Sandbox::new("timers");
    let proc = ScriptedProc::new();
    let out = run(&sb, &proc, &["--timers"]);
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!(
        proc.of("systemctl"),
        vec![
            "systemctl --user daemon-reload".to_string(),
            "systemctl --user enable --now maintainer@sk-issues.timer".to_string(),
            "systemctl --user enable --now maintainer@sk-review.timer".to_string(),
        ]
    );
    for name in ["maintainer@sk-issues.timer", "maintainer@sk-review.timer"] {
        let stamp = sb.home.join(".local/share/systemd/timers").join(format!("stamp-{name}"));
        assert_eq!(fs::metadata(&stamp).unwrap().len(), 0);
        assert!(out.out.contains(&format!("  enabled {name}\n")));
    }
    assert!(!out.out.contains("timers not touched"));
    assert!(!out.out.contains("--timers     #"));
}

#[test]
fn a_failing_enable_ends_the_install_with_that_status_at_the_first_timer() {
    let sb = Sandbox::new("enablefail");
    let proc = ScriptedProc::new();
    proc.on("systemctl --user enable --now maintainer@sk-issues.timer", Answer::err(5, "no\n"));
    let out = run(&sb, &proc, &["--timers"]);
    assert_eq!(out.code, 5);
    assert_eq!(out.err, "no\n");
    assert!(!out.out.contains("enabled") && !out.out.contains("  done"));
    assert_eq!(proc.of("systemctl").len(), 2, "daemon-reload and the one enable");
}

#[test]
fn a_missing_systemctl_ends_the_install_with_127_after_the_files_are_deployed() {
    let sb = Sandbox::new("nosystemctl");
    let proc = ScriptedProc::new();
    proc.on_program("systemctl", Answer::missing());
    let out = run(&sb, &proc, &["--timers"]);
    assert_eq!(out.code, 127);
    assert_eq!(
        out.err,
        format!("{}/install.sh: line 28: systemctl: command not found\n", sb.co.display())
    );
    assert!(sb.bin().join("maintainer").is_file());
}

#[test]
fn no_timer_unit_in_the_checkout_exits_1_after_the_reload() {
    let sb = Sandbox::new("notimers");
    for entry in fs::read_dir(sb.co.join("platform/linux")).unwrap().flatten() {
        if entry.file_name().to_string_lossy().ends_with(".timer") {
            fs::remove_file(entry.path()).unwrap();
        }
    }
    let proc = ScriptedProc::new();
    let out = run(&sb, &proc, &["--timers"]);
    assert_eq!(out.code, 1);
    assert_eq!(
        out.err,
        format!(
            "install.sh: --timers matched no unit files under {}/platform/linux\n",
            sb.co.display()
        )
    );
    assert_eq!(proc.of("systemctl"), vec!["systemctl --user daemon-reload".to_string()]);
}

#[test]
fn a_timer_whose_profile_is_not_in_the_checkout_is_skipped_by_name() {
    let sb = Sandbox::new("skipped");
    fs::write(sb.co.join("platform/linux/maintainer@gone-ci.timer"), "[Timer]\n").unwrap();
    let out = run(&sb, &ScriptedProc::new(), &["--timers"]);
    assert_eq!(out.code, 0);
    assert!(
        out.out.contains("  skipped maintainer@gone-ci.timer (profile gone is not deployed)\n")
    );
    assert!(!sb.home.join(".local/share/systemd/timers/stamp-maintainer@gone-ci.timer").exists());
}

#[test]
fn other_platforms_deploy_the_files_ignore_timers_and_name_their_scheduler() {
    for (uname, line, hint) in [
        (
            "Darwin",
            "  platform: macOS (launchd)\n",
            "        platform/macos/install-launchd.sh <profile>\n",
        ),
        (
            "MINGW64_NT-10.0",
            "  platform: Windows\n",
            "        MAINTAINER_PROFILE=<profile> platform/posix/install-cron.sh   # works anywhere with crontab\n",
        ),
        (
            "FreeBSD",
            "  platform: FreeBSD is unrecognised; files deployed, scheduling is yours to wire\n",
            "        MAINTAINER_PROFILE=<profile> platform/posix/install-cron.sh   # works anywhere with crontab\n",
        ),
    ] {
        let sb = Sandbox::new("platform");
        let proc = ScriptedProc::new();
        proc.on_program("uname", Answer::ok(&format!("{uname}\n")));
        let out = run(&sb, &proc, &["--timers"]);
        assert_eq!(out.code, 0, "{uname}: {}", out.err);
        assert!(out.out.contains(line), "{uname}: {}", out.out);
        assert!(out.out.ends_with(hint), "{uname}: {}", out.out);
        assert!(proc.of("systemctl").is_empty(), "{uname}");
        assert!(
            !sb.share().join("run-instance.sh").exists()
                && !sb.units().join("maintainer@.service").exists(),
            "{uname}"
        );
        assert!(sb.bin().join("maintainer").is_file());
    }
}

#[test]
fn an_unknown_flag_exits_64_with_one_line_and_touches_nothing() {
    let sb = Sandbox::new("flag");
    let proc = ScriptedProc::new();
    let before = sb.listing(&sb.home);
    for args in [&["--bogus"][..], &["--dry-run", "--bogus"][..], &["install"][..], &["--help"][..]]
    {
        let out = run(&sb, &proc, args);
        assert_eq!(out.code, 64, "{args:?}");
        assert_eq!(out.out, "");
        assert_eq!(out.err, format!("install.sh: unknown flag {}\n", args.last().unwrap()));
    }
    assert_eq!(sb.listing(&sb.home), before);
    assert!(proc.log().is_empty());
}

#[test]
fn an_unset_home_ends_the_run_before_the_flags_are_read() {
    let sb = Sandbox::new("nohome");
    let env = mx_install::env::Env::new(
        &sb.co.join("install.sh").to_string_lossy(),
        &["--bogus"],
        "/",
        &[("PATH", "/bin")],
    );
    let out = capture(|con| install::run(&env, &RealMachine, &ScriptedProc::new(), con));
    assert_eq!(out.code, 1);
    assert_eq!(
        out.err,
        format!("{}/install.sh: line 13: HOME: unbound variable\n", sb.co.display())
    );
}

#[test]
fn a_scripts_directory_with_no_python_file_copies_the_pattern_and_fails() {
    let sb = Sandbox::new("nopy");
    for entry in fs::read_dir(sb.co.join("scripts")).unwrap().flatten() {
        fs::remove_file(entry.path()).unwrap();
    }
    let out = run(&sb, &ScriptedProc::new(), &[]);
    assert_eq!(out.code, 1);
    assert!(out.err.contains("cp: cannot stat '"), "{}", out.err);
    assert!(out.err.contains("*.py"), "{}", out.err);
}

#[test]
fn a_destination_that_is_a_link_is_written_through_and_a_destination_that_is_a_directory_receives_the_file()
 {
    let sb = Sandbox::new("links");
    fs::create_dir_all(sb.bin()).unwrap();
    fs::write(sb.dir.join("elsewhere"), "old").unwrap();
    std::os::unix::fs::symlink(sb.dir.join("elsewhere"), sb.bin().join("maintainer")).unwrap();
    fs::create_dir_all(sb.bin().join("maintainer-merge")).unwrap();
    let out = run(&sb, &ScriptedProc::new(), &[]);
    assert_eq!(out.code, 0, "{}", out.err);
    assert!(fs::symlink_metadata(sb.bin().join("maintainer")).unwrap().file_type().is_symlink());
    assert_eq!(
        fs::read(sb.dir.join("elsewhere")).unwrap(),
        fs::read(sb.co.join("bin/maintainer")).unwrap()
    );
    assert!(sb.bin().join("maintainer-merge/maintainer-merge").is_file());
}

#[test]
fn a_read_only_bin_directory_stops_the_install_with_cp_status_and_its_message() {
    let sb = Sandbox::new("readonly");
    fs::create_dir_all(sb.bin()).unwrap();
    fs::set_permissions(sb.bin(), fs::Permissions::from_mode(0o555)).unwrap();
    let out = run(&sb, &ScriptedProc::new(), &[]);
    assert_eq!(out.code, 1);
    assert!(out.err.contains("cp: cannot create regular file '"), "{}", out.err);
    assert!(out.err.contains("Permission denied"));
    assert!(!out.out.contains("  done"));
}

#[test]
fn a_flock_file_without_the_execute_bit_is_not_a_command_on_the_path() {
    let sb = Sandbox::new("flockx");
    fs::set_permissions(sb.tools.join("flock"), fs::Permissions::from_mode(0o644)).unwrap();
    let out = run(&sb, &ScriptedProc::new(), &[]);
    assert_eq!(out.code, 1);
    assert!(out.err.starts_with("install.sh: flock is required"), "{}", out.err);
    assert!(!sb.share().exists());
}
