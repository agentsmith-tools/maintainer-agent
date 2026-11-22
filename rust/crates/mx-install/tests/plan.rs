//! The decisions of `install`: facts in, a plan or a refusal out, no machine involved.

use mx_install::args::Opts;
use mx_install::plan::{
    Cmd, Decision, InstallFacts, Layout, OnFail, Plan, RollbackFacts, RustProgram, Step, Timer,
    UninstallFacts, decide_install, decide_rollback, decide_uninstall, kept_line, timer_profile,
};
use mx_install::refusal::Refusal;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

fn layout() -> Layout {
    Layout::new(PathBuf::from("/co"), OsStr::new("/h"))
}

fn timer(name: &str, present: bool) -> Timer {
    let name = OsString::from(name);
    Timer {
        path: PathBuf::from("/co/platform/linux").join(&name),
        profile: timer_profile(&name),
        name,
        profile_present: present,
    }
}

fn facts() -> InstallFacts {
    InstallFacts {
        layout: layout(),
        opts: Opts::default(),
        flock_on_path: true,
        busy: Vec::new(),
        uname: "Linux".into(),
        scripts: vec![PathBuf::from("/co/scripts/a.py"), PathBuf::from("/co/scripts/b.py")],
        linux_units: vec![
            PathBuf::from("/co/platform/linux/maintainer@.service"),
            PathBuf::from("/co/platform/linux/maintainer@sk-ci.timer"),
        ],
        timers: vec![timer("maintainer@sk-ci.timer", true)],
        rust: Vec::new(),
        rust_stamp: None,
        rust_leftovers: Vec::new(),
    }
}

fn proceed(decision: Decision) -> Plan {
    match decision {
        Decision::Proceed(plan) => plan,
        Decision::Refuse(refusal) => panic!("refused: {refusal:?}"),
    }
}

fn refusal(decision: Decision) -> Refusal {
    match decision {
        Decision::Refuse(refusal) => refusal,
        Decision::Proceed(plan) => panic!("proceeded: {:?}", plan.steps),
    }
}

fn texts(plan: &Plan) -> Vec<String> {
    plan.steps
        .iter()
        .map(|step| match step {
            Step::Say(text) => format!("say {}", String::from_utf8_lossy(text)),
            Step::Run(cmd, OnFail::Abort) => {
                format!("run {}", String::from_utf8_lossy(&cmd.text()))
            }
            Step::Run(cmd, OnFail::Record) => {
                format!("step {}", String::from_utf8_lossy(&cmd.text()))
            }
            other => format!("{other:?}")
                .split('(')
                .next()
                .unwrap_or("")
                .split(' ')
                .next()
                .unwrap_or("")
                .to_string(),
        })
        .collect()
}

fn position(lines: &[String], needle: &str) -> usize {
    lines
        .iter()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no step with {needle}: {lines:#?}"))
}

#[test]
fn a_missing_flock_refuses_before_any_step_and_exits_1() {
    let mut f = facts();
    f.flock_on_path = false;
    let refusal = refusal(decide_install(&f));
    assert_eq!(refusal.exit_code(), 1);
    assert_eq!(
        String::from_utf8(refusal.stderr()).unwrap(),
        "install.sh: flock is required to install and protect active runs; install flock and ensure it is on PATH.\n"
    );
}

#[test]
fn a_missing_flock_refuses_a_dry_run_and_a_forced_run_too() {
    for (dry, force) in [(true, false), (false, true)] {
        let mut f = facts();
        f.flock_on_path = false;
        f.opts.dry = dry;
        f.opts.force = force;
        assert_eq!(refusal(decide_install(&f)), Refusal::FlockMissing);
    }
}

#[test]
fn a_held_lock_refuses_with_69_and_names_the_profiles_before_any_step() {
    let mut f = facts();
    f.busy = vec!["magent".into(), "sysknife".into()];
    let refusal = refusal(decide_install(&f));
    assert_eq!(refusal.exit_code(), 69);
    assert_eq!(
        String::from_utf8(refusal.stderr()).unwrap(),
        "install.sh: a run is in flight for: magent sysknife\n\
         \x20           This replaces /h/.local/share/maintainer/profiles, which that run reads for\n\
         \x20           the whole of its pass.\n\
         \n\
         \x20           Wait for it (maintainer status), or --force if you are sure.\n"
    );
}

#[test]
fn a_held_lock_does_not_stop_a_dry_run_or_a_forced_run() {
    for (dry, force) in [(true, false), (false, true)] {
        let mut f = facts();
        f.busy = vec!["magent".into()];
        f.opts.dry = dry;
        f.opts.force = force;
        proceed(decide_install(&f));
    }
}

#[test]
fn a_dry_run_and_a_real_run_decide_the_same_steps() {
    // `--dry-run` runs the code of the real run: the sink, not the plan, tells them apart.
    let real = proceed(decide_install(&facts()));
    let mut f = facts();
    f.opts.dry = true;
    let dry = proceed(decide_install(&f));
    assert_eq!(real, dry);
}

#[test]
fn the_deployment_runs_in_the_order_the_bash_pins() {
    let lines = texts(&proceed(decide_install(&facts())));
    assert_eq!(lines[0], "say deploying from /co");
    assert_eq!(
        lines[1],
        "run mkdir -p /h/.local/share/maintainer /h/.local/bin /h/.config/systemd/user"
    );
    let order = [
        "cp /co/lib/run.sh /h/.local/share/maintainer/run.sh",
        "cp /co/lib/prose-style.md ",
        "cp /co/lib/preamble-core.md ",
        "cp /co/lib/profile.sh ",
        "cp /co/lib/failure-shapes.json ",
        "cp /co/lib/defaults.env ",
        "Stamp",
        "MintKey",
        "mkdir -p /h/.local/share/maintainer/scripts",
        "cp /co/scripts/a.py /h/.local/share/maintainer/scripts/a.py",
        "cp /co/scripts/b.py /h/.local/share/maintainer/scripts/b.py",
        "rm -rf /h/.local/share/maintainer/backends /h/.local/share/maintainer/profiles",
        "cp -r /co/lib/backends /h/.local/share/maintainer/backends",
        "cp -r /co/profiles /h/.local/share/maintainer/profiles",
        "rm -rf /h/.local/share/maintainer/profiles/_template",
        "RenderWalls",
        "cp /co/bin/maintainer /h/.local/bin/maintainer",
        "cp /co/bin/maintainer-mcp /h/.local/bin/maintainer-mcp",
        "chmod +x /h/.local/share/maintainer/run.sh /h/.local/bin/maintainer /h/.local/bin/maintainer-merge /h/.local/bin/maintainer-doctor /h/.local/bin/maintainer-repo /h/.local/bin/maintainer-mcp",
        "LandingCheck",
    ];
    let mut at = 0;
    for needle in order {
        let found = position(&lines, needle);
        assert!(found >= at, "{needle} is out of order at {found}, after {at}");
        at = found;
    }
}

#[test]
fn the_profiles_are_removed_before_they_are_copied_and_the_template_after() {
    let lines = texts(&proceed(decide_install(&facts())));
    let rm = position(&lines, "rm -rf /h/.local/share/maintainer/backends");
    let copy = position(&lines, "cp -r /co/profiles");
    let template = position(&lines, "_template");
    assert!(rm < copy && copy < template);
}

#[test]
fn no_script_in_the_checkout_leaves_the_pattern_as_the_file_name() {
    let mut f = facts();
    f.scripts.clear();
    let lines = texts(&proceed(decide_install(&f)));
    assert!(
        lines.contains(
            &"run cp /co/scripts/*.py /h/.local/share/maintainer/scripts/*.py".to_string()
        )
    );
}

#[test]
fn linux_copies_the_units_then_the_two_scripts_and_says_so() {
    let lines = texts(&proceed(decide_install(&facts())));
    let units: Vec<&String> =
        lines.iter().filter(|l| l.contains("/.config/systemd/user/")).collect();
    assert_eq!(units.len(), 2);
    assert!(
        lines.contains(
            &"run cp /co/platform/linux/run-instance.sh /h/.local/share/maintainer/run-instance.sh"
                .to_string()
        )
    );
    assert!(lines.contains(&"run chmod +x /h/.local/share/maintainer/alert.sh".to_string()));
    assert!(lines.contains(&"say platform: Linux (systemd user units)".to_string()));
}

#[test]
fn darwin_windows_and_unknown_systems_deploy_no_unit_and_ignore_timers() {
    for (uname, first) in [
        ("Darwin", "say platform: macOS (launchd)"),
        ("MINGW64_NT-10.0", "say platform: Windows"),
        ("MSYS_NT-10.0", "say platform: Windows"),
        ("CYGWIN_NT-10.0", "say platform: Windows"),
        (
            "FreeBSD",
            "say platform: FreeBSD is unrecognised; files deployed, scheduling is yours to wire",
        ),
    ] {
        let mut f = facts();
        f.uname = uname.into();
        f.opts.timers = true;
        let lines = texts(&proceed(decide_install(&f)));
        assert!(lines.contains(&first.to_string()), "{uname}: {lines:#?}");
        assert!(!lines.iter().any(|l| l.contains("systemctl")), "{uname}");
        assert!(!lines.iter().any(|l| l.contains("run-instance.sh")), "{uname}");
        assert!(
            lines.contains(&"say timers not touched (pass --timers to enable them)".to_string())
        );
    }
}

#[test]
fn the_hint_at_the_end_names_the_scheduler_of_the_platform() {
    let hint = |uname: &str| {
        let mut f = facts();
        f.uname = uname.into();
        texts(&proceed(decide_install(&f))).last().unwrap().clone()
    };
    assert_eq!(hint("Linux"), "say       ./install.sh --timers     # enable the systemd timers");
    assert_eq!(hint("Darwin"), "say       platform/macos/install-launchd.sh <profile>");
    assert_eq!(
        hint("MINGW64"),
        "say       MAINTAINER_PROFILE=<profile> platform/posix/install-cron.sh   # works anywhere with crontab"
    );
}

#[test]
fn timers_reload_first_then_stamp_before_each_enable_and_say_so() {
    let mut f = facts();
    f.opts.timers = true;
    let lines = texts(&proceed(decide_install(&f)));
    let reload = position(&lines, "systemctl --user daemon-reload");
    let mkdir = position(&lines, "mkdir -p /h/.local/share/systemd/timers");
    let touch =
        position(&lines, "touch /h/.local/share/systemd/timers/stamp-maintainer@sk-ci.timer");
    let enable = position(&lines, "systemctl --user enable --now maintainer@sk-ci.timer");
    let said = position(&lines, "say enabled maintainer@sk-ci.timer");
    assert!(reload < mkdir && mkdir < touch && touch < enable && enable < said);
    assert!(!lines.contains(&"say timers not touched (pass --timers to enable them)".to_string()));
    assert!(!lines.iter().any(|l| l.contains("--timers     #")));
}

#[test]
fn a_timer_whose_profile_is_not_in_the_checkout_is_skipped_and_none_enabled_ends_in_a_refusal_step()
{
    let mut f = facts();
    f.opts.timers = true;
    f.timers = vec![timer("maintainer@gone-ci.timer", false)];
    let plan = proceed(decide_install(&f));
    let lines = texts(&plan);
    assert!(lines.contains(
        &"say skipped maintainer@gone-ci.timer (profile gone is not deployed)".to_string()
    ));
    assert!(plan.steps.contains(&Step::NoTimersEnabled));
}

#[test]
fn the_profile_of_a_timer_is_cut_at_its_last_hyphen() {
    assert_eq!(timer_profile(OsStr::new("maintainer@my-repo-review.timer")), "my-repo");
    assert_eq!(timer_profile(OsStr::new("maintainer@solo.timer")), "solo.timer");
    assert_eq!(timer_profile(OsStr::new("other-x.timer")), "other");
}

#[test]
fn a_rust_install_swaps_after_the_platform_files_and_writes_its_stamp_last() {
    let program = |build: &'static str, installed: &str, linux_only: bool| RustProgram {
        build,
        installed: PathBuf::from(installed),
        backup: PathBuf::from(format!("{installed}.sh")),
        source: PathBuf::from("/b").join(build),
        linux_only,
    };
    let mut f = facts();
    f.opts.rust = true;
    f.opts.timers = true;
    f.rust = vec![
        program("maintainer-doctor", "/h/.local/bin/maintainer-doctor", false),
        program("alert", "/h/.local/share/maintainer/alert.sh", true),
    ];
    let plan = proceed(decide_install(&f));
    let lines = texts(&plan);
    let alert_copy = position(&lines, "cp /co/platform/linux/alert.sh");
    let swap = lines.iter().position(|l| l == "Swap").unwrap();
    let stamp = lines.iter().position(|l| l == "WriteRustStamp").unwrap();
    let reload = position(&lines, "daemon-reload");
    assert!(alert_copy < swap && swap < stamp && stamp < reload);
    assert_eq!(lines.iter().filter(|l| *l == "Swap").count(), 2);
}

#[test]
fn a_bash_install_after_a_rust_install_removes_the_stamp_and_the_saved_originals() {
    let mut f = facts();
    f.rust_stamp = Some(PathBuf::from("/h/.local/share/maintainer/RUST_STAMP"));
    f.rust_leftovers = vec![PathBuf::from("/h/.local/bin/maintainer-doctor.sh")];
    let lines = texts(&proceed(decide_install(&f)));
    let linux = position(&lines, "say platform: Linux");
    let copy = position(&lines, "rm -f /h/.local/bin/maintainer-doctor.sh");
    let stamp = position(&lines, "rm -f /h/.local/share/maintainer/RUST_STAMP");
    assert!(linux < copy && copy < stamp);
}

#[test]
fn a_rust_install_keeps_the_originals_it_saves_again_and_removes_the_others() {
    let program = |build: &'static str, installed: &str| RustProgram {
        build,
        installed: PathBuf::from(installed),
        backup: PathBuf::from(format!("{installed}.sh")),
        source: PathBuf::from("/b").join(build),
        linux_only: false,
    };
    let mut f = facts();
    f.opts.rust = true;
    f.rust = vec![program("maintainer-doctor", "/h/.local/bin/maintainer-doctor")];
    f.rust_stamp = Some(PathBuf::from("/h/.local/share/maintainer/RUST_STAMP"));
    f.rust_leftovers = vec![
        PathBuf::from("/h/.local/bin/maintainer-doctor.sh"),
        PathBuf::from("/h/.local/bin/maintainer-repo.sh"),
    ];
    let lines = texts(&proceed(decide_install(&f)));
    assert!(lines.contains(&"run rm -f /h/.local/bin/maintainer-repo.sh".to_string()));
    assert!(!lines.iter().any(|l| l.contains("rm -f /h/.local/bin/maintainer-doctor.sh")));
    assert!(!lines.iter().any(|l| l.contains("rm -f /h/.local/share/maintainer/RUST_STAMP")));
}

#[test]
fn a_rust_build_for_a_linux_only_file_is_skipped_elsewhere_and_said() {
    let mut f = facts();
    f.uname = "Darwin".into();
    f.opts.rust = true;
    f.rust = vec![RustProgram {
        build: "run-instance",
        installed: PathBuf::from("/h/.local/share/maintainer/run-instance.sh"),
        backup: PathBuf::from("/h/.local/share/maintainer/run-instance.sh.sh"),
        source: PathBuf::from("/b/run-instance"),
        linux_only: true,
    }];
    let lines = texts(&proceed(decide_install(&f)));
    assert!(
        lines
            .contains(&"say rust: skipped run-instance (it is deployed on Linux only)".to_string())
    );
    assert!(!lines.iter().any(|l| l == "Swap"));
}

// --- uninstall ---------------------------------------------------------------------

fn ufacts() -> UninstallFacts {
    UninstallFacts {
        layout: layout(),
        opts: Opts { uninstall: true, ..Opts::default() },
        assume_yes: false,
        live_timers: None,
        systemd_user: true,
        timer_units: vec![PathBuf::from("/h/.config/systemd/user/maintainer@sk-ci.timer")],
        unit_files: vec![
            PathBuf::from("/h/.config/systemd/user/maintainer@.service"),
            PathBuf::from("/h/.config/systemd/user/maintainer@sk-ci.timer"),
        ],
        stamps: vec![PathBuf::from("/h/.local/share/systemd/timers/stamp-maintainer@sk-ci.timer")],
        darwin: false,
        plists: Vec::new(),
        cron_removal_possible: true,
        cron_script: PathBuf::from("/co/platform/posix/install-cron.sh"),
        deployed_profiles: vec![PathBuf::from(
            "/h/.local/share/maintainer/profiles/sk/profile.env",
        )],
        installed_commands: vec![PathBuf::from("/h/.local/bin/maintainer")],
        rollback_copies: Vec::new(),
        kept: vec![Ok(b"${MAINTAINER_STATE_DIR:-/h/.local/state/sk-maint}".to_vec())],
    }
}

#[test]
fn enabled_timers_refuse_an_uninstall_with_3_and_the_count_in_the_first_line() {
    let mut f = ufacts();
    f.live_timers = Some(2);
    let refusal = refusal(decide_uninstall(&f));
    assert_eq!(refusal.exit_code(), 3);
    let text = String::from_utf8(refusal.stderr()).unwrap();
    assert!(text.starts_with("install.sh: 2 maintainer timer(s) are enabled on this machine.\n"));
    assert_eq!(text.lines().count(), 6);
}

#[test]
fn one_enabled_timer_is_enough_for_the_plan_to_refuse() {
    let mut f = ufacts();
    f.live_timers = Some(1);
    assert_eq!(refusal(decide_uninstall(&f)), Refusal::LiveTimers { count: 1 });
}

#[test]
fn the_yes_answer_and_a_dry_run_skip_the_question() {
    let mut f = ufacts();
    f.live_timers = Some(2);
    f.assume_yes = true;
    proceed(decide_uninstall(&f));
    let mut f = ufacts();
    f.live_timers = Some(2);
    f.opts.dry = true;
    proceed(decide_uninstall(&f));
}

#[test]
fn no_listed_timer_lets_the_uninstall_run() {
    let mut f = ufacts();
    f.live_timers = Some(0);
    proceed(decide_uninstall(&f));
}

#[test]
fn an_uninstall_stops_the_timers_then_removes_units_stamps_cron_commands_and_the_share() {
    let lines = texts(&proceed(decide_uninstall(&ufacts())));
    let want = [
        "say removing the maintainer agent",
        "step systemctl --user disable --now maintainer@sk-ci.timer",
        "step rm -f /h/.config/systemd/user/maintainer@.service",
        "step rm -f /h/.config/systemd/user/maintainer@sk-ci.timer",
        "step rm -f /h/.local/share/systemd/timers/stamp-maintainer@sk-ci.timer",
        "run systemctl --user daemon-reload",
        "step env MAINTAINER_PROFILE=sk /co/platform/posix/install-cron.sh --remove",
        "run rm -f /h/.local/bin/maintainer",
        "step rm -rf /h/.local/share/maintainer",
        "say ",
        "FailureReport",
        "say removed: /h/.local/share/maintainer, the five commands in /h/.local/bin, and every scheduler entry.",
        "say kept:    the audit trail. Delete it yourself if you want it gone:",
        "KeptLine",
    ];
    assert_eq!(lines, want);
}

#[test]
fn without_a_systemd_user_session_the_units_still_go_and_systemctl_is_not_called() {
    let mut f = ufacts();
    f.systemd_user = false;
    let lines = texts(&proceed(decide_uninstall(&f)));
    assert!(!lines.iter().any(|l| l.contains("systemctl")));
    assert!(lines.iter().any(|l| l.contains("rm -f /h/.config/systemd/user/maintainer@.service")));
}

#[test]
fn which_failures_end_the_run_and_which_are_listed_is_the_bash_rule() {
    // `step` records; a bare `run` ends the run under set -e.
    let plan = proceed(decide_uninstall(&{
        let mut f = ufacts();
        f.darwin = true;
        f.plists = vec![PathBuf::from("/h/Library/LaunchAgents/dev.maintainer.sk.ci.plist")];
        f
    }));
    for step in &plan.steps {
        if let Step::Run(cmd, on_fail) = step {
            let text = String::from_utf8(cmd.text()).unwrap();
            let want = match cmd {
                Cmd::Systemctl(args) if args.iter().any(|a| a == "daemon-reload") => OnFail::Abort,
                Cmd::Launchctl(_) | Cmd::RmF(_)
                    if text.contains("/Library/") || text.contains("/.local/bin/") =>
                {
                    OnFail::Abort
                }
                _ => OnFail::Record,
            };
            assert_eq!(*on_fail, want, "{text}");
        }
    }
}

#[test]
fn on_darwin_each_plist_is_unloaded_then_removed_and_on_linux_none_is_looked_at() {
    let mut f = ufacts();
    f.darwin = true;
    f.plists = vec![PathBuf::from("/h/Library/LaunchAgents/dev.maintainer.sk.ci.plist")];
    let lines = texts(&proceed(decide_uninstall(&f)));
    let unload =
        position(&lines, "launchctl unload /h/Library/LaunchAgents/dev.maintainer.sk.ci.plist");
    assert_eq!(lines[unload + 1], "run rm -f /h/Library/LaunchAgents/dev.maintainer.sk.ci.plist");
    f.darwin = false;
    assert!(!texts(&proceed(decide_uninstall(&f))).iter().any(|l| l.contains("launchctl")));
}

#[test]
fn cron_is_removed_only_when_crontab_and_the_installer_are_there() {
    let mut f = ufacts();
    f.cron_removal_possible = false;
    assert!(!texts(&proceed(decide_uninstall(&f))).iter().any(|l| l.contains("install-cron")));
}

#[test]
fn the_rollback_copies_of_a_rust_install_go_with_the_commands() {
    let mut f = ufacts();
    f.rollback_copies = vec![PathBuf::from("/h/.local/bin/maintainer-doctor.sh")];
    let lines = texts(&proceed(decide_uninstall(&f)));
    let copy = position(&lines, "rm -f /h/.local/bin/maintainer-doctor.sh");
    assert!(
        copy > position(&lines, "rm -f /h/.local/bin/maintainer")
            && copy < position(&lines, "rm -rf /h/")
    );
}

// --- the kept-trail lines ----------------------------------------------------------

#[test]
fn the_kept_line_is_the_state_dir_line_after_the_sed_pipeline() {
    let line = b"STATE_DIR=\"${MAINTAINER_STATE_DIR:-$HOME/.local/state/sk-maint}\"";
    assert_eq!(
        kept_line(Some(line), OsStr::new("/home/u")).unwrap(),
        b"${MAINTAINER_STATE_DIR:-/home/u/.local/state/sk-maint}"
    );
}

#[test]
fn the_kept_line_cuts_at_the_last_quote_equals_and_changes_only_the_first_home() {
    let line = b"X=\"a\" STATE_DIR=\"$HOME/x $HOME/y\"";
    assert_eq!(kept_line(Some(line), OsStr::new("/h")).unwrap(), b"/h/x $HOME/y");
}

#[test]
fn a_profile_with_no_state_dir_line_gives_an_empty_kept_line() {
    assert_eq!(kept_line(None, OsStr::new("/h")).unwrap(), b"");
}

#[test]
fn an_ampersand_in_home_is_the_matched_text_and_a_pipe_breaks_the_sed_script() {
    let line = b"STATE_DIR=\"$HOME/s\"";
    assert_eq!(kept_line(Some(line), OsStr::new("/a&b")).unwrap(), b"/a$HOMEb/s");
    // `|` ends the replacement early in sed, and the next character is read as a flag.
    let failure = kept_line(Some(line), OsStr::new("/a|b")).unwrap_err();
    assert_eq!(failure.message(), "sed: -e expression #1, char 31: unknown option to `s'");
}

// --- rollback -----------------------------------------------------------------------

fn rfacts() -> RollbackFacts {
    RollbackFacts {
        layout: layout(),
        opts: Opts { rollback: true, ..Opts::default() },
        flock_on_path: true,
        busy: Vec::new(),
        stamped: Some(vec![
            (
                PathBuf::from("/h/.local/bin/maintainer-doctor"),
                PathBuf::from("/h/.local/bin/maintainer-doctor.sh"),
                true,
            ),
            (
                PathBuf::from("/h/.local/share/maintainer/run.sh"),
                PathBuf::from("/h/.local/share/maintainer/run.sh.sh"),
                true,
            ),
        ]),
    }
}

#[test]
fn a_rollback_moves_every_saved_original_back_and_removes_the_stamp_last() {
    let lines = texts(&proceed(decide_rollback(&rfacts())));
    assert_eq!(
        lines,
        [
            "say rolling back from /h/.local/share/maintainer",
            "run mv /h/.local/bin/maintainer-doctor.sh /h/.local/bin/maintainer-doctor",
            "run mv /h/.local/share/maintainer/run.sh.sh /h/.local/share/maintainer/run.sh",
            "run rm -f /h/.local/share/maintainer/RUST_STAMP",
            "say rolled back 2 program(s) to the bash originals",
        ]
    );
}

#[test]
fn a_rollback_with_no_stamp_refuses_with_1() {
    let mut f = rfacts();
    f.stamped = None;
    let refusal = refusal(decide_rollback(&f));
    assert_eq!(refusal.exit_code(), 1);
    assert_eq!(
        String::from_utf8(refusal.stderr()).unwrap(),
        "install.sh: nothing to roll back: /h/.local/share/maintainer/RUST_STAMP does not exist\n"
    );
}

#[test]
fn a_rollback_whose_saved_original_is_missing_refuses_before_it_moves_anything() {
    let mut f = rfacts();
    f.stamped.as_mut().unwrap()[1].2 = false;
    let refusal = refusal(decide_rollback(&f));
    assert_eq!(refusal.exit_code(), 1);
    assert!(String::from_utf8(refusal.stderr()).unwrap().contains("run.sh.sh is missing"));
}

#[test]
fn a_rollback_asks_for_flock_and_the_lock_like_an_install() {
    let mut f = rfacts();
    f.flock_on_path = false;
    assert_eq!(refusal(decide_rollback(&f)), Refusal::FlockMissing);
    let mut f = rfacts();
    f.busy = vec!["sk".into()];
    assert_eq!(refusal(decide_rollback(&f)).exit_code(), 69);
    f.opts.force = true;
    proceed(decide_rollback(&f));
}

#[test]
fn every_refusal_keeps_the_status_the_bash_gives() {
    let cases: [(Refusal, i32); 13] = [
        (Refusal::UnknownFlag("x".into()), 64),
        (Refusal::RustBinNeedsValue, 64),
        (Refusal::RustNeedsBin, 64),
        (Refusal::RustBinNeedsRust, 64),
        (Refusal::RollbackAlone, 64),
        (Refusal::RustWithUninstall, 64),
        (Refusal::FlockMissing, 1),
        (Refusal::RunInFlight { share: "s".into(), profiles: vec![] }, 69),
        (Refusal::LiveTimers { count: 1 }, 3),
        (Refusal::HomeUnset { script: "install.sh".into(), line: 13 }, 1),
        (Refusal::RustBinUnusable { detail: "d".into() }, 1),
        (Refusal::NothingToRollBack { stamp: "s".into() }, 1),
        (Refusal::RollbackBlocked { detail: "d".into() }, 1),
    ];
    for (refusal, code) in cases {
        assert_eq!(refusal.exit_code(), code, "{refusal:?}");
        assert!(refusal.stderr().ends_with(b"\n"));
    }
}
