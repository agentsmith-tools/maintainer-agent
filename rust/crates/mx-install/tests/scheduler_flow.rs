//! `install-cron`, `install-launchd`, `alert` and `run-instance` against real files.

mod common;

use common::{Answer, Sandbox, ScriptedProc, capture, write};
use mx_install::console::Console;
use mx_install::machine::RealMachine;
use mx_install::schedulers::{install_cron, install_launchd};
use mx_install::units::{alert, run_instance};
use std::fs;

const MARKER: &str = "# maintainer-agent (sk)";

fn deployed(tag: &str, env_text: &str) -> Sandbox {
    let sb = Sandbox::new(tag);
    write(&sb.share().join("run.sh"), "#!/bin/sh\n", 0o755);
    write(&sb.share().join("profiles/sk/profile.env"), env_text, 0o644);
    sb
}

fn cron(sb: &Sandbox, proc: &ScriptedProc, args: &[&str], profile: Option<&str>) -> common::Run {
    let extra: Vec<(&str, &str)> = profile.map(|p| ("MAINTAINER_PROFILE", p)).into_iter().collect();
    let env = sb.env_with("platform/posix/install-cron.sh", args, &extra);
    capture(|con| install_cron(&env, &RealMachine, proc, con))
}

fn launchd(sb: &Sandbox, proc: &ScriptedProc, args: &[&str], profile: Option<&str>) -> common::Run {
    let extra: Vec<(&str, &str)> = profile.map(|p| ("MAINTAINER_PROFILE", p)).into_iter().collect();
    let env = sb.env_with("platform/macos/install-launchd.sh", args, &extra);
    capture(|con| install_launchd(&env, &RealMachine, proc, con))
}

const ENV: &str = "TASKS=\"review issues\"\nSTATE_DIR=\"$HOME/.local/state/sk-maint\"\nREPO_PATH=\"$HOME/code/sk\"\nMIN_HOURS_review=6\nMIN_HOURS_issues=36\n";

fn block(sb: &Sandbox) -> String {
    let home = sb.home.display();
    format!(
        "{MARKER}\nPATH={home}/.local/bin:{home}/.cargo/bin:/usr/local/bin:/usr/bin:/bin\n17 9 * * *  {share}/run.sh sk review >>\"{home}/.local/state/sk-maint/logs/cron.out\" 2>&1\n17 10 * * *  {share}/run.sh sk issues >>\"{home}/.local/state/sk-maint/logs/cron.out\" 2>&1\n{MARKER} end\n",
        share = sb.share().display()
    )
}

// --- install-cron -------------------------------------------------------------------

#[test]
fn no_profile_exits_64_and_lists_the_deployed_ones_with_a_trailing_space() {
    let sb = deployed("cronnone", ENV);
    write(&sb.share().join("profiles/magent/profile.env"), ENV, 0o644);
    let proc = ScriptedProc::new();
    let out = cron(&sb, &proc, &[], None);
    assert_eq!(out.code, 64);
    assert_eq!(out.out, "");
    assert_eq!(
        out.err,
        "install-cron: no profile name given. Set MAINTAINER_PROFILE=<name>.\n              deployed here: magent sk \n"
    );
    assert!(proc.of("crontab").is_empty());
}

#[test]
fn nothing_deployed_lists_nothing_and_an_empty_profiles_directory_lists_a_star() {
    let sb = Sandbox::new("cronempty");
    let out = cron(&sb, &ScriptedProc::new(), &[], None);
    assert_eq!(out.err.lines().nth(1), Some("              deployed here: "));
    fs::create_dir_all(sb.share().join("profiles")).unwrap();
    let out = cron(&sb, &ScriptedProc::new(), &[], None);
    assert_eq!(
        out.err.lines().nth(1),
        Some("              deployed here: * "),
        "pins current behaviour: the unmatched pattern is printed"
    );
}

#[test]
fn a_missing_run_sh_a_missing_crontab_and_an_unknown_profile_each_exit_1_in_that_order() {
    let sb = Sandbox::new("cronrefuse");
    let out = cron(&sb, &ScriptedProc::new(), &[], Some("sk"));
    assert_eq!(
        (out.code, out.err.as_str()),
        (
            1,
            format!(
                "install-cron: {}/run.sh not found; run ./install.sh first\n",
                sb.share().display()
            )
            .as_str()
        )
    );
    write(&sb.share().join("run.sh"), "#!/bin/sh\n", 0o755);
    sb.drop_tool("crontab");
    let out = cron(&sb, &ScriptedProc::new(), &[], Some("sk"));
    assert_eq!((out.code, out.err.as_str()), (1, "install-cron: no crontab on this system\n"));
    let sb = deployed("cronunknown", ENV);
    let proc = ScriptedProc::new();
    let out = cron(&sb, &proc, &[], Some("zzz"));
    assert_eq!((out.code, out.err.as_str()), (1, "install-cron: no deployed profile 'zzz'\n"));
    assert!(proc.of("crontab").is_empty(), "no crontab call before the profile is found");
}

#[test]
fn a_profile_file_whose_last_command_fails_reads_as_not_deployed() {
    // Pins current behaviour: `. file || ...` tests the status of the file's last command.
    let sb = deployed("cronfalse", &format!("{ENV}false\n"));
    let out = cron(&sb, &ScriptedProc::new(), &[], Some("sk"));
    assert_eq!((out.code, out.err.as_str()), (1, "install-cron: no deployed profile 'sk'\n"));
}

#[test]
fn a_dry_run_prints_the_block_after_one_heading_and_only_reads_the_crontab() {
    let sb = deployed("crondry", ENV);
    let proc = ScriptedProc::new();
    proc.on("crontab -l", Answer::err(1, "no crontab for u\n"));
    let out = cron(&sb, &proc, &["--dry-run"], Some("sk"));
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!(out.out, format!("--- crontab that would be installed ---\n{}", block(&sb)));
    assert_eq!(proc.of("crontab"), vec!["crontab -l".to_string()]);
    assert!(!sb.home.join(".local/state/sk-maint").exists(), "a dry run makes no directory");
}

#[test]
fn an_install_writes_a_blank_line_then_the_block_makes_the_log_directory_and_says_three_lines() {
    let sb = deployed("cronwrite", ENV);
    let proc = ScriptedProc::new();
    proc.on("crontab -l", Answer::err(1, "no crontab for u\n"));
    let out = cron(&sb, &proc, &[], Some("sk"));
    assert_eq!(out.code, 0, "{}", out.err);
    let written = proc
        .specs
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.argv.as_slice() == ["crontab", "-"])
        .unwrap()
        .stdin
        .clone()
        .unwrap();
    assert_eq!(String::from_utf8(written).unwrap(), format!("\n{}", block(&sb)));
    assert!(sb.home.join(".local/state/sk-maint/logs").is_dir());
    assert_eq!(
        out.out,
        "  installed 2 cron entries for profile 'sk'\n  inspect with: crontab -l\n  note: cron does not catch up a missed run; see the header of this script\n"
    );
}

#[test]
fn the_install_replaces_its_own_block_keeps_the_rest_and_the_other_profiles() {
    let sb = deployed("cronreplace", ENV);
    let proc = ScriptedProc::new();
    let current = format!(
        "0 1 * * * /usr/bin/backup\n{MARKER}\nold\n{MARKER} end\n# maintainer-agent (other)\nx\n# maintainer-agent (other) end\n"
    );
    proc.on("crontab -l", Answer::ok(&current));
    cron(&sb, &proc, &[], Some("sk"));
    let written = proc
        .specs
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.argv.as_slice() == ["crontab", "-"])
        .unwrap()
        .stdin
        .clone()
        .unwrap();
    let written = String::from_utf8(written).unwrap();
    assert_eq!(written.matches(MARKER).count(), 2, "one opening and one closing line: {written}");
    assert!(written.starts_with("0 1 * * * /usr/bin/backup\n# maintainer-agent (other)\nx\n# maintainer-agent (other) end\n# maintainer-agent (sk)\nPATH="), "{written}");
    assert!(!written.contains("old"));
}

#[test]
fn a_failed_crontab_read_stops_the_install_and_leaves_the_crontab_alone() {
    let sb = deployed("cronspool", ENV);
    let proc = ScriptedProc::new();
    proc.on("crontab -l", Answer::err(1, "crontab: temporary failure reading spool\n"));
    let out = cron(&sb, &proc, &[], Some("sk"));
    assert_eq!(out.code, 1);
    assert_eq!(
        out.err,
        "crontab: temporary failure reading spool\ninstall-cron: crontab -l failed; leaving the crontab unchanged\n"
    );
    assert!(proc.of("crontab").iter().all(|c| c == "crontab -l"), "nothing was written");
    assert!(!sb.home.join(".local/state/sk-maint").exists());
}

#[test]
fn remove_writes_the_crontab_without_the_block_and_says_one_line() {
    let sb = deployed("cronremove", ENV);
    let proc = ScriptedProc::new();
    proc.on("crontab -l", Answer::ok(&format!("0 1 * * * /usr/bin/backup\n{}", block(&sb))));
    let out = cron(&sb, &proc, &["--remove"], Some("sk"));
    assert_eq!((out.code, out.out.as_str()), (0, "  maintainer entries removed\n"));
    let written = proc
        .specs
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.argv.as_slice() == ["crontab", "-"])
        .unwrap()
        .stdin
        .clone()
        .unwrap();
    assert_eq!(written, b"0 1 * * * /usr/bin/backup\n");
}

#[test]
fn a_crontab_that_cannot_be_written_ends_with_its_status_and_no_success_line() {
    let sb = deployed("cronwritefail", ENV);
    let proc = ScriptedProc::new();
    proc.on("crontab -l", Answer::err(1, "no crontab for u\n"));
    proc.on("crontab -", Answer::err(3, "stub crontab: cannot write\n"));
    for args in [&[][..], &["--remove"][..]] {
        let out = cron(&sb, &proc, args, Some("sk"));
        assert_eq!(out.code, 3, "{args:?}");
        assert!(!out.out.contains("installed") && !out.out.contains("removed"), "{}", out.out);
        assert_eq!(out.err, "stub crontab: cannot write\n");
    }
}

#[test]
fn an_unknown_mode_exits_64_after_reading_the_crontab() {
    let sb = deployed("cronmode", ENV);
    let proc = ScriptedProc::new();
    proc.on("crontab -l", Answer::err(1, "no crontab for u\n"));
    let out = cron(&sb, &proc, &["--bogus"], Some("sk"));
    assert_eq!((out.code, out.err.as_str()), (64, "usage: install-cron.sh [--dry-run|--remove]\n"));
    assert_eq!(proc.of("crontab"), vec!["crontab -l".to_string()]);
}

#[test]
fn a_state_directory_that_cannot_hold_logs_stops_the_install_before_crontab_is_written() {
    let sb = deployed(
        "cronbad",
        &format!("TASKS=\"review\"\nSTATE_DIR=\"{}/file\"\n", sb_dir_placeholder()),
    );
    let file = sb.dir.join("file");
    fs::write(&file, "").unwrap();
    fs::write(
        sb.share().join("profiles/sk/profile.env"),
        format!("TASKS=\"review\"\nSTATE_DIR=\"{}\"\n", file.display()),
    )
    .unwrap();
    let proc = ScriptedProc::new();
    proc.on("crontab -l", Answer::err(1, "no crontab for u\n"));
    let out = cron(&sb, &proc, &[], Some("sk"));
    assert_ne!(out.code, 0);
    assert!(proc.of("crontab").iter().all(|c| c == "crontab -l"), "{:?}", proc.of("crontab"));
}

fn sb_dir_placeholder() -> &'static str {
    "/nonexistent"
}

#[test]
fn a_profile_with_no_state_dir_or_no_tasks_prints_what_came_first_then_the_shells_message() {
    for (env_text, line, name) in
        [("TASKS=\"a\"\n", 56, "STATE_DIR"), ("STATE_DIR=/s\n", 60, "TASKS")]
    {
        let sb = deployed("cronunbound", env_text);
        let proc = ScriptedProc::new();
        proc.on("crontab -l", Answer::err(1, "no crontab for u\n"));
        let out = cron(&sb, &proc, &["--dry-run"], Some("sk"));
        assert_eq!(out.code, 1);
        assert!(
            out.out
                .starts_with(&format!("--- crontab that would be installed ---\n{MARKER}\nPATH=")),
            "{}",
            out.out
        );
        assert_eq!(
            out.err,
            format!(
                "{}/platform/posix/install-cron.sh: line {line}: {name}: unbound variable\n",
                sb.co.display()
            )
        );
    }
}

// --- install-launchd ------------------------------------------------------------------

#[test]
fn launchd_unloads_then_loads_each_task_in_order_and_prints_the_hours() {
    let sb = deployed("launchd", ENV);
    let proc = ScriptedProc::new();
    let out = launchd(&sb, &proc, &["sk"], None);
    assert_eq!(out.code, 0, "{}", out.err);
    let agents = sb.home.join("Library/LaunchAgents");
    assert_eq!(
        proc.of("launchctl"),
        vec![
            format!("launchctl unload {}/dev.maintainer.sk.review.plist", agents.display()),
            format!("launchctl load {}/dev.maintainer.sk.review.plist", agents.display()),
            format!("launchctl unload {}/dev.maintainer.sk.issues.plist", agents.display()),
            format!("launchctl load {}/dev.maintainer.sk.issues.plist", agents.display()),
        ]
    );
    assert_eq!(
        out.out,
        "  loaded dev.maintainer.sk.review (fires daily at 09:17; runs when 6h have passed)\n  loaded dev.maintainer.sk.issues (fires daily at 10:17; runs when 36h have passed)\n  done. list with: launchctl list | grep dev.maintainer.sk\n"
    );
    let plist = fs::read_to_string(agents.join("dev.maintainer.sk.issues.plist")).unwrap();
    assert!(plist.contains("<key>Hour</key><integer>10</integer>"));
    assert!(plist.contains(&format!("<string>{}/code/sk</string>", sb.home.display())));
    assert!(sb.home.join(".local/state/sk-maint/logs").is_dir());
}

#[test]
fn the_profile_comes_from_the_argument_before_the_environment_and_a_task_without_a_minimum_says_0h()
{
    let sb = deployed("launchdenv", "TASKS=\"nomin\"\nSTATE_DIR=\"$HOME/st\"\nREPO_PATH=/r\n");
    let out = launchd(&sb, &ScriptedProc::new(), &[], Some("sk"));
    assert!(
        out.out.starts_with(
            "  loaded dev.maintainer.sk.nomin (fires daily at 09:17; runs when 0h have passed)\n"
        ),
        "{}",
        out.out
    );
    write(&sb.share().join("profiles/magent/profile.env"), ENV, 0o644);
    let out = launchd(&sb, &ScriptedProc::new(), &["magent"], Some("sk"));
    assert!(
        out.out.contains("dev.maintainer.magent.review") && !out.out.contains("dev.maintainer.sk."),
        "{}",
        out.out
    );
}

#[test]
fn launchd_refusals_exit_64_and_1_and_create_nothing() {
    let sb = deployed("launchdrefuse", ENV);
    let out = launchd(&sb, &ScriptedProc::new(), &[], None);
    assert_eq!(out.code, 64);
    assert_eq!(
        out.err,
        "install-launchd: no profile name given. Pass it as an argument or set MAINTAINER_PROFILE.\n                 deployed here: sk \n"
    );
    let out = launchd(&sb, &ScriptedProc::new(), &["nope"], None);
    assert_eq!(
        (out.code, out.err.as_str()),
        (1, "install-launchd: no deployed profile 'nope'; run ./install.sh first\n")
    );
    assert!(!sb.home.join("Library").exists());
    assert_eq!(launchd(&sb, &ScriptedProc::new(), &["--remove"], None).code, 64);
}

#[test]
fn a_failing_load_stops_at_the_first_plist_and_a_failing_unload_is_ignored() {
    let sb = deployed("launchdfail", ENV);
    let proc = ScriptedProc::new();
    proc.on(
        &format!(
            "launchctl load {}/Library/LaunchAgents/dev.maintainer.sk.review.plist",
            sb.home.display()
        ),
        Answer::err(1, "no\n"),
    );
    let out = launchd(&sb, &proc, &["sk"], None);
    assert_eq!(out.code, 1);
    assert!(!out.out.contains("loaded"));
    assert!(sb.home.join("Library/LaunchAgents/dev.maintainer.sk.review.plist").is_file());
    assert!(!sb.home.join("Library/LaunchAgents/dev.maintainer.sk.issues.plist").exists());
    let sb = deployed("launchdunload", ENV);
    let proc = ScriptedProc::new();
    proc.on_program("launchctl", Answer::code(0));
    proc.on(
        &format!(
            "launchctl unload {}/Library/LaunchAgents/dev.maintainer.sk.review.plist",
            sb.home.display()
        ),
        Answer::code(1),
    );
    assert_eq!(launchd(&sb, &proc, &["sk"], None).code, 0);
}

#[test]
fn remove_unloads_and_deletes_one_profiles_plists_in_name_order() {
    let sb = deployed("launchdremove", ENV);
    let agents = sb.home.join("Library/LaunchAgents");
    for name in [
        "dev.maintainer.sk.review.plist",
        "dev.maintainer.sk.audit.plist",
        "dev.maintainer.magent.review.plist",
        "other.plist",
        "dev.maintainer.sk.plist",
    ] {
        write(&agents.join(name), "", 0o644);
    }
    let proc = ScriptedProc::new();
    let out = launchd(&sb, &proc, &["--remove", "sk"], None);
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!(
        out.out,
        "  removed dev.maintainer.sk.audit.plist\n  removed dev.maintainer.sk.review.plist\n"
    );
    assert!(
        agents.join("dev.maintainer.magent.review.plist").exists()
            && agents.join("other.plist").exists()
            && agents.join("dev.maintainer.sk.plist").exists()
    );
}

// --- alert ----------------------------------------------------------------------------

const MSG: &str = "the run unit failed before it could report; systemd killed or refused it";

fn run_alert(
    sb: &Sandbox,
    proc: &common::ScriptedProc,
    args: &[&str],
    extra: &[(&str, &str)],
) -> common::Run {
    let env = sb.env_with("platform/linux/alert.sh", args, extra);
    capture(|con| alert(&env, &RealMachine, proc, con))
}

fn alerts(sb: &Sandbox, profile: &str) -> String {
    fs::read_to_string(sb.home.join(format!(".local/state/{profile}-maint/logs/alerts.log")))
        .unwrap_or_default()
}

fn state(sb: &Sandbox, profile: &str) {
    fs::create_dir_all(sb.home.join(format!(".local/state/{profile}-maint"))).unwrap();
}

#[test]
fn the_alert_line_goes_to_the_failing_profiles_directory_and_to_no_other() {
    let sb = Sandbox::new("alertown");
    state(&sb, "sk");
    state(&sb, "magent");
    let out = run_alert(&sb, &common::ScriptedProc::new(), &["sk-ci"], &[]);
    assert_eq!(out.code, 0);
    assert_eq!(alerts(&sb, "sk"), format!("2026-01-01T00:00:00+00:00  ALERT sk-ci: {MSG}\n"));
    assert_eq!(alerts(&sb, "magent"), "");
    assert!(!sb.home.join(".local/state/magent-maint/logs").exists());
}

#[test]
fn a_hyphenated_profile_is_everything_before_the_last_hyphen_and_a_second_alert_appends() {
    let sb = Sandbox::new("alerthyphen");
    state(&sb, "my-repo");
    state(&sb, "my");
    let proc = common::ScriptedProc::new();
    run_alert(&sb, &proc, &["my-repo-review"], &[]);
    run_alert(&sb, &proc, &["my-repo-ci"], &[]);
    assert_eq!(alerts(&sb, "my-repo").lines().count(), 2);
    assert_eq!(alerts(&sb, "my"), "");
}

#[test]
fn with_no_directory_for_the_profile_every_maint_directory_gets_the_line_and_with_none_nothing_is_written()
 {
    let sb = Sandbox::new("alertfallback");
    let before = sb.listing(&sb.home);
    assert_eq!(run_alert(&sb, &common::ScriptedProc::new(), &["ghost-review"], &[]).code, 0);
    assert_eq!(sb.listing(&sb.home), before);
    state(&sb, "sk");
    state(&sb, "magent");
    run_alert(&sb, &common::ScriptedProc::new(), &["ghost-review"], &[]);
    assert!(
        alerts(&sb, "sk").contains("ALERT ghost-review")
            && alerts(&sb, "magent").contains("ALERT ghost-review")
    );
}

#[test]
fn plan_decision_f7_the_unit_name_systemd_passes_lands_the_alert_in_every_profile() {
    let sb = Sandbox::new("alertf7");
    state(&sb, "sk");
    state(&sb, "magent");
    let proc = common::ScriptedProc::new();
    run_alert(&sb, &proc, &["maintainer@sk-ci.service"], &[]);
    assert!(
        alerts(&sb, "sk").contains("ALERT maintainer@sk-ci.service")
            && alerts(&sb, "magent").contains("ALERT maintainer@sk-ci.service")
    );
    let notify = proc.of("notify-send");
    assert!(
        notify[0]
            .contains("journalctl --user -u maintainer@maintainer@sk-ci.service.service -n 40"),
        "{notify:?}"
    );
}

#[test]
fn the_notification_is_critical_names_the_unit_and_carries_display_and_the_bus() {
    let sb = Sandbox::new("alertnotify");
    state(&sb, "sk");
    let proc = common::ScriptedProc::new();
    run_alert(&sb, &proc, &["sk-ci"], &[]);
    let call = proc
        .specs
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.argv.first() == "notify-send")
        .unwrap()
        .clone();
    assert_eq!(call.argv.as_slice()[..5], ["notify-send", "-u", "critical", "-a", "maintainer"]);
    assert_eq!(call.argv.as_slice()[5], "maintainer \u{b7} sk-ci \u{b7} FAILED");
    assert_eq!(
        call.argv.as_slice()[6],
        format!(
            "{MSG}\nfix    maintainer-doctor\nthen   journalctl --user -u maintainer@sk-ci.service -n 40"
        )
    );
    assert!(call.env.contains(&("DISPLAY".to_string(), ":1".to_string())));
    assert!(call.env.contains(&(
        "DBUS_SESSION_BUS_ADDRESS".to_string(),
        "unix:path=/run/user/1000/bus".to_string()
    )));
    let proc = common::ScriptedProc::new();
    run_alert(
        &sb,
        &proc,
        &["sk-ci"],
        &[("DISPLAY", ":7"), ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/x/bus")],
    );
    let call = proc
        .specs
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.argv.first() == "notify-send")
        .unwrap()
        .clone();
    assert!(call.env.contains(&("DISPLAY".to_string(), ":7".to_string())));
    assert!(
        call.env
            .contains(&("DBUS_SESSION_BUS_ADDRESS".to_string(), "unix:path=/x/bus".to_string()))
    );
}

#[test]
fn notify_off_still_writes_the_log_and_a_failing_or_missing_notify_send_changes_nothing() {
    let sb = Sandbox::new("alertoff");
    state(&sb, "sk");
    let proc = common::ScriptedProc::new();
    let out = run_alert(&sb, &proc, &["sk-ci"], &[("MAINTAINER_NOTIFY", "off")]);
    assert_eq!(out.code, 0);
    assert!(proc.of("notify-send").is_empty() && alerts(&sb, "sk").contains("ALERT sk-ci"));
    let proc = common::ScriptedProc::new();
    proc.on_program("notify-send", Answer::code(1));
    assert_eq!(run_alert(&sb, &proc, &["sk-ci"], &[]).code, 0);
    let proc = common::ScriptedProc::new();
    proc.on_program("notify-send", Answer::missing());
    let out = run_alert(&sb, &proc, &["sk-ci"], &[]);
    assert_eq!((out.code, out.err.as_str()), (0, ""));
}

#[test]
fn no_argument_reports_the_instance_as_unknown_and_a_name_with_no_hyphen_is_its_own_profile() {
    let sb = Sandbox::new("alertunknown");
    state(&sb, "sk");
    state(&sb, "solo");
    run_alert(&sb, &common::ScriptedProc::new(), &[], &[]);
    assert!(alerts(&sb, "sk").contains("ALERT unknown:"));
    run_alert(&sb, &common::ScriptedProc::new(), &["solo"], &[]);
    assert!(
        alerts(&sb, "solo").contains("ALERT solo:") && !alerts(&sb, "sk").contains("ALERT solo:")
    );
}

// --- run-instance ------------------------------------------------------------------------

fn instance_env(sb: &Sandbox, args: &[&str]) -> mx_install::env::Env {
    sb.env("inst/run-instance.sh", args)
}

#[test]
fn a_run_instance_with_no_argument_exits_1_with_the_usage_text_and_runs_nothing() {
    let sb = Sandbox::new("instnone");
    let env = instance_env(&sb, &[]);
    let out = capture(|con: &mut Console| run_instance(&env, con));
    assert_eq!(out.code, 1);
    assert_eq!(
        out.err,
        format!(
            "{}/inst/run-instance.sh: line 5: 1: usage: run-instance.sh <profile>-<task>\n",
            sb.co.display()
        )
    );
    let env = instance_env(&sb, &[""]);
    assert_eq!(
        capture(|con: &mut Console| run_instance(&env, con)).code,
        1,
        ":? treats an empty argument as unset"
    );
}

#[test]
fn a_missing_or_unexecutable_run_sh_ends_with_127_or_126_in_the_words_of_the_shell() {
    let sb = Sandbox::new("instmissing");
    write(&sb.co.join("inst/run-instance.sh"), "#!/bin/sh\n", 0o755);
    let env = instance_env(&sb, &["sk-ci"]);
    let out = capture(|con: &mut Console| run_instance(&env, con));
    assert_eq!(out.code, 127);
    assert!(
        out.err.ends_with(&format!("{}/inst/run.sh: No such file or directory\n", sb.co.display())),
        "{}",
        out.err
    );
    write(&sb.co.join("inst/run.sh"), "#!/bin/sh\n", 0o644);
    let out = capture(|con: &mut Console| run_instance(&env, con));
    assert_eq!(out.code, 126);
    assert!(out.err.ends_with("run.sh: Permission denied\n"), "{}", out.err);
}

#[test]
fn a_missing_date_is_reported_once_for_each_directory_the_alert_writes_to_and_never_without_one() {
    let sb = Sandbox::new("alertdate");
    let proc = ScriptedProc::new();
    proc.on_program("date", Answer::missing());
    run_alert(&sb, &proc, &["ghost-x"], &[("MAINTAINER_NOTIFY", "off")]);
    assert!(proc.of("date").is_empty(), "no directory, no date call");
    state(&sb, "sk");
    state(&sb, "magent");
    let out = run_alert(&sb, &proc, &["ghost-x"], &[("MAINTAINER_NOTIFY", "off")]);
    assert_eq!(proc.of("date").len(), 2);
    assert_eq!(out.err.lines().count(), 2, "{}", out.err);
    assert!(
        alerts(&sb, "sk").starts_with("  ALERT ghost-x:"),
        "the date is blank: {}",
        alerts(&sb, "sk")
    );
}

#[test]
fn an_empty_instance_an_empty_display_and_an_empty_bus_address_read_as_unset() {
    let sb = Sandbox::new("alertempty");
    state(&sb, "unknown");
    let proc = ScriptedProc::new();
    run_alert(&sb, &proc, &[""], &[("DISPLAY", ""), ("DBUS_SESSION_BUS_ADDRESS", "")]);
    assert!(alerts(&sb, "unknown").contains("ALERT unknown:"));
    let call = proc
        .specs
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.argv.first() == "notify-send")
        .unwrap()
        .clone();
    assert!(call.env.contains(&("DISPLAY".to_string(), ":1".to_string())));
    assert!(call.env.contains(&(
        "DBUS_SESSION_BUS_ADDRESS".to_string(),
        "unix:path=/run/user/1000/bus".to_string()
    )));
}

#[test]
fn a_cron_install_for_a_profile_with_no_tasks_writes_a_block_with_no_closing_line_and_says_zero() {
    // Pins current behaviour: `gen` runs in a subshell of the pipeline, dies at `$TASKS`, and
    // crontab still receives the lines it printed first; the count runs `gen` again.
    let sb = deployed("cronnotasks", "STATE_DIR=\"$HOME/st\"\n");
    let proc = ScriptedProc::new();
    proc.on("crontab -l", Answer::err(1, "no crontab for u\n"));
    let out = cron(&sb, &proc, &[], Some("sk"));
    assert_eq!(out.code, 0);
    let written = proc
        .specs
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.argv.as_slice() == ["crontab", "-"])
        .unwrap()
        .stdin
        .clone()
        .unwrap();
    let written = String::from_utf8(written).unwrap();
    assert!(written.starts_with(&format!("\n{MARKER}\nPATH=")), "{written}");
    assert!(!written.contains("end"), "no closing line: {written}");
    let line = format!(
        "{}/platform/posix/install-cron.sh: line 60: TASKS: unbound variable\n",
        sb.co.display()
    );
    assert_eq!(out.err, format!("{line}{line}"));
    assert!(out.out.starts_with("  installed 0 cron entries for profile 'sk'\n"));
}

#[test]
fn a_home_with_a_space_in_its_name_loses_the_alert_because_the_directory_list_is_split_on_blanks() {
    // Pins current behaviour (found, not fixed): alert.sh passes `$dirs` unquoted to printf, so
    // a path with a blank becomes two words and neither is a directory.
    let sb = Sandbox::new("alertspace");
    let spaced = sb.dir.join("my home");
    fs::create_dir_all(spaced.join(".local/state/sk-maint")).unwrap();
    let proc = ScriptedProc::new();
    let env = mx_install::env::Env::new(
        &sb.co.join("platform/linux/alert.sh").to_string_lossy(),
        &["sk-ci"],
        &spaced.to_string_lossy(),
        &[
            ("HOME", &spaced.to_string_lossy()),
            ("PATH", &sb.tools.to_string_lossy()),
            ("MAINTAINER_NOTIFY", "off"),
        ],
    );
    let out = capture(|con| alert(&env, &RealMachine, &proc, con));
    assert_eq!(out.code, 0);
    assert!(!spaced.join(".local/state/sk-maint/logs").exists(), "the alert was written nowhere");
}

#[test]
fn a_profile_that_stops_on_an_unbound_variable_ends_install_cron_with_1_and_says_nothing() {
    // Pins current behaviour: `set -u` ends the shell inside the sourced file, and the
    // redirection hides the message (the shell script ends with 1, not bash -c's 127).
    let sb = deployed("cronunboundvar", "echo \"$NO_SUCH_VARIABLE\"\nTASKS=a\nSTATE_DIR=/s\n");
    let proc = ScriptedProc::new();
    proc.on("crontab -l", Answer::err(1, "no crontab for u\n"));
    let out = cron(&sb, &proc, &["--dry-run"], Some("sk"));
    assert_eq!((out.code, out.out.as_str(), out.err.as_str()), (1, "", ""));
}
