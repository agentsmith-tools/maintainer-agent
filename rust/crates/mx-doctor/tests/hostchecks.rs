//! Scheduler, Container verification and Sandbox.

mod common;

use common::*;
use mx_doctor::host::HostFs;
use mx_doctor::hostchecks::{
    self, Probe, RecipeFacts, SuiteFacts, TimerFacts, decide_suite, decide_timers,
    is_hardening_line,
};
use mx_doctor::report::Item;

// --- Scheduler -----------------------------------------------------------------------

fn timer_row(profile: &str) -> String {
    format!(
        "Mon 2026-01-05 10:00:00 UTC 3h left Mon 2026-01-05 04:00:00 UTC 3h ago maintainer@{profile}-review.timer maintainer@{profile}-review.service\n"
    )
}

fn linux_session(rig: &Rig, timers: &str, units: &str, linger: &str) {
    rig.proc.on(&["uname", "-s"], 0, "Linux\n");
    rig.proc.have("systemctl", "/bin/systemctl");
    rig.proc.on(&["systemctl", "--user", "show-environment"], 0, "");
    rig.proc.on(
        &["systemctl", "--user", "list-timers", "maintainer@magent-*", "--all", "--no-pager"],
        0,
        timers,
    );
    rig.proc.on(
        &["systemctl", "--user", "list-unit-files", "maintainer@magent-*.timer", "--no-pager"],
        0,
        units,
    );
    rig.proc.on(&["loginctl", "show-user", "fixture", "-p", "Linger", "--value"], 0, linger);
}

#[test]
fn a_linux_scheduler_asks_systemd_in_the_order_timers_files_linger_timers_again() {
    let rig = Rig::new(&[]);
    linux_session(&rig, &timer_row("magent"), "", "yes\n");
    let mut o = out();
    hostchecks::scheduler(&rig.ctx(), &mut o);
    assert_eq!(
        rig.proc.tool_calls(),
        [
            "uname -s",
            "systemctl --user show-environment",
            "systemctl --user list-timers maintainer@magent-* --all --no-pager",
            "systemctl --user list-unit-files maintainer@magent-*.timer --no-pager",
            "loginctl show-user fixture -p Linger --value",
            "systemctl --user list-timers maintainer@magent-* --all --no-pager",
        ]
    );
    assert!(has_ok(&o.report, "1 systemd timer(s) enabled for magent"));
    assert!(has_ok(&o.report, "lingering on: timers run without a login session"));
}

#[test]
fn timer_files_that_are_not_loaded_read_as_none_enabled() {
    let facts = TimerFacts { listed: 0, files: 2, lingering: true, fires_soon: false };
    assert_eq!(
        decide_timers(&facts, "magent", "me")[0],
        Item::warn(
            "2 timer file(s) for magent, none enabled: nothing runs unattended",
            "./install.sh --timers"
        )
    );
    let none = TimerFacts { listed: 0, files: 0, lingering: true, fires_soon: false };
    assert_eq!(
        decide_timers(&none, "magent", "me")[0],
        Item::warn("no timers for magent", "./install.sh --timers")
    );
}

#[test]
fn listed_timers_win_over_unit_files_and_linger_off_names_the_user() {
    let facts = TimerFacts { listed: 3, files: 9, lingering: false, fires_soon: true };
    assert_eq!(
        decide_timers(&facts, "p", "me"),
        [
            Item::ok("3 systemd timer(s) enabled for p"),
            Item::warn(
                "lingering off: timers stop when you log out",
                "sudo loginctl enable-linger me"
            ),
            Item::warn(
                "a timer fires within seconds; Persistent=true may be catching up",
                "expected after enabling; otherwise check the stamp files"
            ),
        ]
    );
}

#[test]
fn unit_files_are_counted_only_when_the_name_starts_the_line() {
    let rig = Rig::new(&[]);
    linux_session(&rig, "", "x maintainer@magent-review.timer disabled\n", "yes\n");
    let mut o = out();
    hostchecks::scheduler(&rig.ctx(), &mut o);
    assert!(has_warn(&o.report, "no timers for magent", "./install.sh --timers"));
}

#[test]
fn another_profile_s_timers_are_not_counted() {
    let rig = Rig::new(&[]);
    linux_session(
        &rig,
        &timer_row("sysknife"),
        "maintainer@sysknife-review.timer disabled\n",
        "yes\n",
    );
    let mut o = out();
    hostchecks::scheduler(&rig.ctx(), &mut o);
    assert!(has_warn(&o.report, "no timers for magent", "./install.sh --timers"));
}

#[test]
fn without_a_user_variable_the_user_comes_from_id_and_an_empty_linger_answer_means_off() {
    let mut rig = Rig::new(&[]);
    rig.env.user = None;
    rig.proc.on(&["uname", "-s"], 0, "Linux\n");
    rig.proc.have("systemctl", "/bin/systemctl");
    rig.proc.on(&["systemctl", "--user", "show-environment"], 0, "");
    rig.proc.on(
        &["systemctl", "--user", "list-timers", "maintainer@magent-*", "--all", "--no-pager"],
        0,
        "",
    );
    rig.proc.on(
        &["systemctl", "--user", "list-unit-files", "maintainer@magent-*.timer", "--no-pager"],
        0,
        "",
    );
    rig.proc.on(&["id", "-un"], 0, "idfallback\n");
    rig.proc.on(&["loginctl", "show-user", "idfallback", "-p", "Linger", "--value"], 0, "");
    let mut o = out();
    hostchecks::scheduler(&rig.ctx(), &mut o);
    assert!(has_warn(
        &o.report,
        "lingering off: timers stop when you log out",
        "sudo loginctl enable-linger idfallback"
    ));
}

#[test]
fn a_failing_show_environment_is_no_systemd_user_session_and_stops_the_section() {
    let rig = Rig::new(&[]);
    rig.proc.on(&["uname", "-s"], 0, "Linux\n");
    rig.proc.have("systemctl", "/bin/systemctl");
    rig.proc.on(&["systemctl", "--user", "show-environment"], 1, "");
    let mut o = out();
    hostchecks::scheduler(&rig.ctx(), &mut o);
    assert!(has_warn(&o.report, "no systemd user session", "use platform/posix/install-cron.sh"));
    assert_eq!(rig.proc.tool_calls(), ["uname -s", "systemctl --user show-environment"]);
}

#[test]
fn on_darwin_only_labels_naming_dev_maintainer_with_a_literal_dot_count() {
    let rig = Rig::new(&[]);
    rig.proc.on(&["uname", "-s"], 0, "Darwin\n");
    rig.proc.on(
        &["launchctl", "list"],
        0,
        "dev.maintainer.a\ndevXmaintainer.b\nother\ndev.maintainer.c\n",
    );
    let mut o = out();
    hostchecks::scheduler(&rig.ctx(), &mut o);
    assert!(has_ok(&o.report, "2 launchd agent(s) loaded"));
    assert!(has_warn(
        &o.report,
        "launchd has no Persistent=true",
        "a run missed while asleep does not catch up"
    ));
}

#[test]
fn another_system_reads_no_native_scheduler_and_asks_uname_twice() {
    let rig = Rig::new(&[]);
    rig.proc.on(&["uname", "-s"], 0, "FreeBSD\n");
    let mut o = out();
    hostchecks::scheduler(&rig.ctx(), &mut o);
    assert!(has_warn(
        &o.report,
        "FreeBSD: no native scheduler wired",
        "platform/posix/install-cron.sh"
    ));
    assert_eq!(rig.proc.tool_calls(), ["uname -s", "uname -s"]);
}

// --- Verify suites ---------------------------------------------------------------------------

fn suite(name: &str, image: &str, needs: &str) -> SuiteFacts {
    SuiteFacts {
        name: name.into(),
        image: image.into(),
        needs: needs.into(),
        recipe: None,
        probe: None,
    }
}

#[test]
fn a_suite_with_no_needs_fails_and_is_not_probed() {
    let (items, probed) = decide_suite(&suite("s", "docker.io/x:1", ""), "magent");
    assert!(!probed);
    assert_eq!(
        items,
        [Item::fail(
            "verify suite 's' declares no suite_needs, so its image is unchecked",
            "add suite_needs() naming the binaries its tests invoke"
        )]
    );
}

fn recipe(hash: &str, stamp: &str, exists: bool, file: bool) -> RecipeFacts {
    RecipeFacts {
        containerfile: file.then(|| "/p/s.Containerfile".to_string()),
        hash: hash.into(),
        build: format!(
            "podman build --label maintainer.recipe={hash} -t localhost/i:1 -f /p/s.Containerfile /p"
        ),
        image_exists: exists,
        stamp: stamp.into(),
    }
}

#[test]
fn a_local_image_with_no_recipe_or_not_on_the_host_fails_and_skips_the_probe() {
    let mut facts = suite("s", "localhost/i:1", "bash");
    facts.recipe = Some(recipe("abc", "", false, false));
    let (items, probed) = decide_suite(&facts, "magent");
    assert!(!probed);
    assert_eq!(
        items,
        [Item::fail(
            "verify suite 's' runs localhost/i:1 and no s.Containerfile builds it",
            "add the recipe beside profiles/magent/verify.d/s.sh"
        )]
    );
    facts.recipe = Some(recipe("abc", "", false, true));
    let (items, probed) = decide_suite(&facts, "magent");
    assert!(!probed);
    assert_eq!(
        items,
        [Item::fail(
            "verify suite 's': localhost/i:1 is not on this host, so nothing can earn a receipt there",
            "podman build --label maintainer.recipe=abc -t localhost/i:1 -f /p/s.Containerfile /p"
        )]
    );
}

#[test]
fn a_stamp_equal_to_the_recipe_hash_goes_on_to_the_probe_without_a_finding() {
    let mut facts = suite("s", "localhost/i:1", "bash git");
    facts.recipe = Some(recipe("abc", "abc", true, true));
    facts.probe = Some(Probe { code: 0, missing: String::new() });
    let (items, probed) = decide_suite(&facts, "magent");
    assert!(probed);
    assert_eq!(items, [Item::ok("verify suite 's': localhost/i:1 provides bash git")]);
}

#[test]
fn an_unstamped_image_warns_and_a_drifted_one_fails_and_both_still_probe() {
    for stamp in ["", "<no value>"] {
        let mut facts = suite("s", "localhost/i:1", "bash");
        facts.recipe = Some(recipe("abc", stamp, true, true));
        let (items, probed) = decide_suite(&facts, "magent");
        assert!(probed, "{stamp:?}");
        assert!(
            matches!(&items[0], Item::Warn(text, _) if text == "verify suite 's': localhost/i:1 carries no recipe fingerprint, so drift from s.Containerfile is invisible"),
            "{items:?}"
        );
    }
    let mut facts = suite("s", "localhost/i:1", "bash");
    facts.recipe = Some(recipe("abc", "other", true, true));
    let (items, probed) = decide_suite(&facts, "magent");
    assert!(probed);
    assert!(
        matches!(&items[0], Item::Fail(text, _) if text == "verify suite 's': localhost/i:1 was built from a different s.Containerfile")
    );
}

#[test]
fn an_empty_recipe_hash_equals_an_empty_stamp_and_reads_as_current() {
    let mut facts = suite("s", "localhost/i:1", "bash");
    facts.recipe = Some(recipe("", "", true, true));
    let (items, _) = decide_suite(&facts, "magent");
    assert!(items.is_empty(), "{items:?}");
}

#[test]
fn a_failing_probe_names_the_exit_code_and_never_reads_as_nothing_missing() {
    let mut facts = suite("s", "docker.io/x:1", "bash");
    facts.probe = Some(Probe { code: 125, missing: String::new() });
    let (items, _) = decide_suite(&facts, "magent");
    assert_eq!(
        items,
        [Item::warn(
            "verify suite 's': could not probe docker.io/x:1 (podman exited 125)",
            "podman pull docker.io/x:1, then re-run; until then this suite's image is unverified"
        )]
    );
}

#[test]
fn missing_binaries_are_listed_with_the_trailing_space_trimmed() {
    let mut facts = suite("s", "docker.io/x:1", "bash python3");
    facts.probe = Some(Probe { code: 0, missing: "python3 yamllint ".into() });
    let (items, _) = decide_suite(&facts, "magent");
    assert_eq!(
        items,
        [Item::fail(
            "verify suite 's' runs in docker.io/x:1, which has no: python3 yamllint",
            "no receipt is earnable for that suite; change suite_image or suite_needs in profiles/magent/verify.d/s.sh"
        )]
    );
}

// --- Container verification --------------------------------------------------------------------

const PROBE_SCRIPT: &str =
    "for b in \"$@\"; do command -v \"$b\" >/dev/null 2>&1 || printf \"%s \" \"$b\"; done";

fn podman_ready(rig: &Rig) {
    rig.proc.have("podman", "/bin/podman");
    rig.proc.on(&["podman", "system", "migrate"], 0, "");
    rig.proc.on(
        &["podman", "run", "--rm", "--network=none", "docker.io/library/busybox:latest", "true"],
        0,
        "",
    );
    rig.proc.on(&["uname", "-s"], 0, "Linux\n");
    rig.host.file(
        &format!("{HOME}/.config/systemd/user/podman-userns-warmup.service"),
        b"",
        false,
        0,
    );
}

#[test]
fn quick_skips_everything_with_a_warning_that_has_no_fix_line() {
    let mut rig = Rig::new(&[]);
    rig.quick = true;
    let mut o = out();
    hostchecks::container_verification(&rig.ctx(), &mut o);
    assert!(rig.proc.argvs().is_empty());
    assert_eq!(lines(&o.report), ["", "Container verification", "  warn  skipped (--quick)"]);
}

#[test]
fn without_podman_the_section_warns_and_skips_the_warm_up_check() {
    let rig = Rig::new(&[]);
    rig.proc.lack("podman");
    let mut o = out();
    hostchecks::container_verification(&rig.ctx(), &mut o);
    assert!(has_warn(
        &o.report,
        "podman not installed; the merge gate cannot observe a mutation",
        "install podman"
    ));
    assert!(rig.proc.tool_calls().is_empty());
}

#[test]
fn a_deployment_with_no_suite_makes_exactly_two_podman_calls_migrate_then_the_busybox_run() {
    let rig = Rig::new(&[]);
    podman_ready(&rig);
    let mut o = out();
    hostchecks::container_verification(&rig.ctx(), &mut o);
    assert_eq!(
        rig.proc.tool_calls(),
        [
            "podman system migrate",
            "podman run --rm --network=none docker.io/library/busybox:latest true",
            "uname -s",
        ]
    );
    assert!(has_ok(&o.report, "podman runs from cold"));
    assert!(has_ok(
        &o.report,
        "userns warm-up unit installed (hardened units can reuse the namespace)"
    ));
}

#[test]
fn a_cold_start_that_fails_is_a_failure_with_the_newuidmap_fix() {
    let rig = Rig::new(&[]);
    podman_ready(&rig);
    rig.proc.on(
        &["podman", "run", "--rm", "--network=none", "docker.io/library/busybox:latest", "true"],
        125,
        "",
    );
    let mut o = out();
    hostchecks::container_verification(&rig.ctx(), &mut o);
    assert!(has_fail(
        &o.report,
        "podman cannot start a container from cold",
        "check newuidmap and /etc/subuid"
    ));
}

#[test]
fn a_missing_warm_up_unit_fails_on_linux_and_asks_uname_a_second_time() {
    let rig = Rig::new(&[]);
    podman_ready(&rig);
    rig.host.remove_tree(&format!("{HOME}/.config/systemd/user/podman-userns-warmup.service"));
    let mut o = out();
    hostchecks::container_verification(&rig.ctx(), &mut o);
    assert!(has_fail(
        &o.report,
        "no podman-userns-warmup.service",
        "./install.sh; without it a hardened timer cannot start a container"
    ));
    assert_eq!(rig.proc.tool_calls().iter().filter(|call| *call == "uname -s").count(), 2);
}

fn suite_rig(file_body: &str) -> Rig {
    let mut rig = Rig::new(&[]);
    podman_ready(&rig);
    let dir = format!("{}/verify.d", rig.pdir());
    rig.host.file(&format!("{dir}/s.sh"), file_body.as_bytes(), false, 0);
    rig.suite_env = vec![("MAINTAINER_SHELL_IMAGE".into(), "x".into())];
    rig
}

fn strs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

fn bash_ask(script_marker: &str, file: &str) -> Vec<String> {
    let script = if script_marker == "image" {
        "set -uo pipefail; . \"$1\" >/dev/null 2>&1; declare -F suite_image >/dev/null && suite_image"
    } else {
        "set -uo pipefail; . \"$1\" >/dev/null 2>&1; declare -F suite_needs >/dev/null && suite_needs"
    };
    ["bash", "-c", script, "_", file].iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn a_suite_is_sourced_twice_in_a_child_bash_and_its_needs_travel_as_probe_arguments() {
    let rig = suite_rig("x");
    let file = format!("{}/verify.d/s.sh", rig.pdir());
    let image = bash_ask("image", &file);
    let needs = bash_ask("needs", &file);
    rig.proc.on(&strs(&image), 0, "docker.io/library/python:3.12");
    rig.proc.on(&strs(&needs), 0, "bash python3 git");
    rig.proc.on(
        &[
            "podman",
            "run",
            "--rm",
            "--network=none",
            "docker.io/library/python:3.12",
            "sh",
            "-c",
            PROBE_SCRIPT,
            "sh",
            "bash",
            "python3",
            "git",
        ],
        0,
        "",
    );
    let mut o = out();
    hostchecks::container_verification(&rig.ctx(), &mut o);
    assert!(has_ok(
        &o.report,
        "verify suite 's': docker.io/library/python:3.12 provides bash python3 git"
    ));
    let specs = rig.proc.specs();
    let sourcing =
        specs.iter().find(|spec| spec.argv.as_slice()[2].contains("suite_image")).unwrap();
    assert!(sourcing.env.contains(&("MAINTAINER_SHELL_IMAGE".to_string(), "x".to_string())));
}

#[test]
fn only_files_ending_in_dot_sh_are_suites_and_dot_files_are_skipped() {
    let rig = suite_rig("x");
    let dir = format!("{}/verify.d", rig.pdir());
    rig.host.file(&format!("{dir}/notes.txt"), b"x", false, 0);
    rig.host.file(&format!("{dir}/.hidden.sh"), b"x", false, 0);
    let file = format!("{dir}/s.sh");
    let image = bash_ask("image", &file);
    let needs = bash_ask("needs", &file);
    rig.proc.on(&strs(&image), 0, "docker.io/x:1");
    rig.proc.on(&strs(&needs), 0, "");
    let mut o = out();
    hostchecks::container_verification(&rig.ctx(), &mut o);
    let findings: Vec<_> = o
        .report
        .items()
        .iter()
        .filter(|i| matches!(i, Item::Fail(t, _) if t.contains("verify suite")))
        .collect();
    assert_eq!(findings.len(), 1, "{findings:?}");
}

// --- Sandbox -----------------------------------------------------------------------------------------

const UNIT: &str = "[Service]\nType=oneshot\nNoNewPrivileges=yes\nProtectSystem=strict\nRestrictSUIDSGID=yes\nSystemCallFilter=@system-service\n  PrivateDevices=yes\n#ProtectKernelTunables=yes\nNice=5\nProtect=yes\nProtect2=\n";

#[test]
fn a_hardening_line_is_one_of_four_names_or_a_prefix_with_letters_then_an_equals_sign() {
    for good in [
        "NoNewPrivileges=yes",
        "LockPersonality=yes",
        "MemoryDenyWriteExecute=yes",
        "PrivateTmp=yes",
        "ProtectSystem=strict",
        "RestrictSUIDSGID=yes",
        "SystemCallFilter=@x",
        "SystemCallArchitectures=native",
    ] {
        assert!(is_hardening_line(good.as_bytes()), "{good}");
    }
    for bad in [
        "  PrivateDevices=yes",
        "#ProtectKernelTunables=yes",
        "Nice=5",
        "Protect=yes",
        "Protect2=",
        "PrivateTmp",
        "PrivateTmpx=1",
        "ExecStart=/x",
    ] {
        assert!(!is_hardening_line(bad.as_bytes()), "{bad}");
    }
}

fn sandbox_rig() -> Rig {
    let rig = Rig::new(&[]);
    rig.proc.on(&["uname", "-s"], 0, "Linux\n");
    rig.host.file(
        &format!("{HOME}/.config/systemd/user/maintainer@.service"),
        UNIT.as_bytes(),
        false,
        0,
    );
    rig.proc.have("systemd-run", "/bin/systemd-run");
    rig.proc.have("maintainer-merge", "/bin/maintainer-merge");
    rig
}

#[test]
fn no_unit_file_no_systemd_run_or_another_system_means_there_is_nothing_to_probe() {
    let rig = Rig::new(&[]);
    rig.proc.on(&["uname", "-s"], 0, "Linux\n");
    let mut o = out();
    hostchecks::sandbox(&rig.ctx(), &mut o);
    assert!(has_ok(&o.report, "no systemd user unit to probe on this machine"));

    let rig = sandbox_rig();
    rig.proc.lack("systemd-run");
    let mut o = out();
    hostchecks::sandbox(&rig.ctx(), &mut o);
    assert!(has_ok(&o.report, "no systemd user unit to probe on this machine"));

    let rig = sandbox_rig();
    rig.proc.on(&["uname", "-s"], 0, "Darwin\n");
    let mut o = out();
    hostchecks::sandbox(&rig.ctx(), &mut o);
    assert!(has_ok(&o.report, "no systemd user unit to probe on this machine"));
    assert!(!rig.proc.tool_calls().iter().any(|c| c.starts_with("systemd-run")));
}

#[test]
fn a_unit_without_directives_warns_with_grep_s_exit_code_and_runs_nothing() {
    let rig = sandbox_rig();
    rig.host.file(
        &format!("{HOME}/.config/systemd/user/maintainer@.service"),
        b"[Service]\nNice=5\n",
        false,
        0,
    );
    let mut o = out();
    hostchecks::sandbox(&rig.ctx(), &mut o);
    let unit = format!("{HOME}/.config/systemd/user/maintainer@.service");
    assert!(has_warn(
        &o.report,
        &format!("no hardening directive found in {unit} (grep exited 1)"),
        "the sandbox probe has nothing to apply; read the unit by hand"
    ));
    assert!(!rig.proc.tool_calls().iter().any(|c| c.starts_with("systemd-run")));
}

#[test]
fn an_unreadable_unit_reads_as_grep_exit_two() {
    let rig = sandbox_rig();
    rig.host.make_unreadable(&format!("{HOME}/.config/systemd/user/maintainer@.service"), "denied");
    let mut o = out();
    hostchecks::sandbox(&rig.ctx(), &mut o);
    assert!(
        o.report
            .items()
            .iter()
            .any(|i| matches!(i, Item::Warn(t, _) if t.ends_with("(grep exited 2)")))
    );
}

#[test]
fn without_maintainer_merge_the_probe_warns_and_runs_nothing_and_a_bare_unit_failure_stops_it() {
    let rig = sandbox_rig();
    rig.proc.lack("maintainer-merge");
    let mut o = out();
    hostchecks::sandbox(&rig.ctx(), &mut o);
    assert!(has_warn(
        &o.report,
        "maintainer-merge is not on PATH, so the sandbox probe cannot run",
        "./install.sh"
    ));

    let rig = sandbox_rig();
    rig.proc.on(&["systemd-run", "--user", "--wait", "--pipe", "--quiet", "true"], 1, "");
    let mut o = out();
    hostchecks::sandbox(&rig.ctx(), &mut o);
    assert!(has_warn(
        &o.report,
        "systemd-run --user cannot start a transient unit, so the sandbox is unprobed",
        "log in to a user session, then re-run"
    ));
}

fn probe_argv(sb: &str) -> Vec<String> {
    let mut v: Vec<String> = ["systemd-run", "--user", "--wait", "--pipe", "--quiet"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    for line in [
        "NoNewPrivileges=yes",
        "ProtectSystem=strict",
        "RestrictSUIDSGID=yes",
        "SystemCallFilter=@system-service",
    ] {
        v.push("-p".into());
        v.push(line.into());
    }
    v.extend([
        "-E".to_string(),
        format!("MAINTAINER_REPO={sb}/r"),
        "-E".to_string(),
        format!("MAINTAINER_STATE={sb}/s"),
    ]);
    v.extend(
        ["bash", "-c", ". \"$1\" && extract_tree HEAD \"$2\"", "_", "/bin/maintainer-merge"]
            .map(str::to_string),
    );
    v.push(format!("{sb}/x"));
    v
}

fn git_chain(rig: &Rig, sb: &str, commit_code: i32) {
    rig.proc.on(&["git", "init", "-q", "-b", "main", &format!("{sb}/r")], 0, "");
    rig.proc.on(&["git", "-C", &format!("{sb}/r"), "add", "f"], 0, "");
    rig.proc.on(
        &[
            "git",
            "-C",
            &format!("{sb}/r"),
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-qm",
            "probe",
        ],
        commit_code,
        "",
    );
}

#[test]
fn the_hardened_probe_applies_the_unit_s_directives_in_file_order_counts_them_and_cleans_up() {
    let rig = sandbox_rig();
    rig.proc.on(&["systemd-run", "--user", "--wait", "--pipe", "--quiet", "true"], 0, "");
    let sb = "/tmp-case/tmp.fake000001";
    git_chain(&rig, sb, 0);
    let argv = probe_argv(sb);
    rig.proc.on(&argv.iter().map(String::as_str).collect::<Vec<_>>(), 0, "");
    let mut o = out();
    hostchecks::sandbox(&rig.ctx(), &mut o);
    assert!(has_ok(
        &o.report,
        "verify extracts a tree under the unit's hardening (4 directive(s) applied)"
    ));
    assert_eq!(rig.host.removed(), [sb]);
    let calls = rig.proc.tool_calls();
    let kinds: Vec<&str> =
        calls.iter().map(|c| c.split(' ').take(2).last().unwrap_or("")).collect();
    assert_eq!(
        kinds,
        ["-s", "--user", "init", "-C", "-C", "--user"],
        "uname, bare unit test, then init, add, commit, then the hardened probe: {calls:?}"
    );
}

#[test]
fn a_failing_probe_quotes_its_first_two_output_lines_and_still_cleans_up() {
    let rig = sandbox_rig();
    rig.proc.on(&["systemd-run", "--user", "--wait", "--pipe", "--quiet", "true"], 0, "");
    let sb = "/tmp-case/tmp.fake000001";
    git_chain(&rig, sb, 0);
    let argv = probe_argv(sb);
    rig.proc.on(
        &argv.iter().map(String::as_str).collect::<Vec<_>>(),
        1,
        "line one\nline two\nline three\n",
    );
    let mut o = out();
    hostchecks::sandbox(&rig.ctx(), &mut o);
    let unit = format!("{HOME}/.config/systemd/user/maintainer@.service");
    assert!(has_fail(
        &o.report,
        "verify cannot extract a tree under the unit's hardening: line one line two ",
        &format!(
            "a directive in {unit} blocks a call the extraction needs; every unattended verify fails until it is fixed"
        )
    ));
    assert_eq!(rig.host.removed().len(), 1);
}

#[test]
fn a_probe_repository_that_cannot_be_built_warns_names_tmpdir_and_leaves_no_directory() {
    let rig = sandbox_rig();
    rig.proc.on(&["systemd-run", "--user", "--wait", "--pipe", "--quiet", "true"], 0, "");
    let sb = "/tmp-case/tmp.fake000001";
    git_chain(&rig, sb, 1);
    let mut o = out();
    hostchecks::sandbox(&rig.ctx(), &mut o);
    assert!(has_warn(&o.report, "could not build the probe repository", "check git and /tmp-case"));
    assert_eq!(rig.host.removed(), [sb]);
    assert!(!rig.proc.tool_calls().iter().any(|c| c.contains("extract_tree")));
}

#[test]
fn a_temporary_directory_that_cannot_be_made_leaves_the_probe_to_be_built_under_the_root() {
    // Pinned, found and not fixed: `mktemp -d` failing leaves `_sb` empty in the
    // bash, which goes on with /r, /x and /s. The doctor then reports the probe
    // repository as unbuildable when the user may not write there.
    let rig = sandbox_rig();
    rig.host.fail_temp_dir();
    rig.proc.on(&["systemd-run", "--user", "--wait", "--pipe", "--quiet", "true"], 0, "");
    rig.proc.on(&["git", "init", "-q", "-b", "main", "/r"], 128, "");
    let mut o = out();
    hostchecks::sandbox(&rig.ctx(), &mut o);
    assert!(has_warn(&o.report, "could not build the probe repository", "check git and /tmp-case"));
    assert!(rig.host.removed().is_empty(), "there is nothing to remove");
    assert!(rig.proc.tool_calls().contains(&"git init -q -b main /r".to_string()));
}
