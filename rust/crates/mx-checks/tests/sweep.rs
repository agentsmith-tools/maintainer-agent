//! `shellcheck-sweep` over fakes: the file selection, the memory gate, the chain of programs each
//! linter run goes through, and the exit codes.

mod common;

use common::{Rig, exited, ok};
use mx_checks::sweep;
use mx_ports::{ProcError, Script};

const SCRIPT: &str = "/t/scripts/shellcheck-sweep.sh";
const SC: &str = "shellcheck --severity=warning";
const SCOPE: &str = "systemd-run --user --scope --quiet -p MemoryMax=18G -p MemorySwapMax=0";
const ROOMY: &str = "MemTotal: 1 kB\nMemAvailable: 20000000 kB\n";

/// A checkout of one script (the sweep itself), a roomy meminfo and the tools on `PATH`.
fn rig(tools: &[&str]) -> Rig {
    let rig = Rig::new();
    rig.host.put_file("/t/scripts/shellcheck-sweep.sh", "#!/usr/bin/env bash\n");
    rig.host.put_file("/meminfo", ROOMY);
    for tool in tools {
        rig.host.put_program(format!("/bin/{tool}"));
    }
    rig
}

fn listing(rig: &Rig, text: &str) {
    rig.always(&["git", "ls-files"], ok(text));
}

fn env<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut vars = vec![("PATH", "/bin"), ("MAGENT_MEMINFO", "/meminfo")];
    vars.extend_from_slice(extra);
    vars
}

fn run(rig: &Rig, extra: &[(&str, &str)]) -> common::Outcome {
    rig.run(SCRIPT, &env(extra), sweep::run)
}

fn script_all(rig: &Rig, files: &[&str], rc: i32) {
    for file in files {
        rig.always(&["shellcheck", "--severity=warning", file], exited(rc, "", ""));
    }
}

#[test]
fn it_lints_each_selected_script_once_in_byte_order_and_says_how_many() {
    let rig = rig(&["shellcheck"]);
    rig.host.put_file("/t/a.sh", "#!/bin/bash\n");
    rig.host.put_file("/t/b", "#!/bin/sh\n");
    rig.host.put_file("/t/d.py", "#!/usr/bin/env python3\n");
    rig.host.put_file("/t/profiles/p/verify.d/x.sh", "echo a fragment\n");
    listing(&rig, "scripts/shellcheck-sweep.sh\nprofiles/p/verify.d/x.sh\nd.py\nb\na.sh\n");
    script_all(&rig, &["a.sh", "b", "profiles/p/verify.d/x.sh", "scripts/shellcheck-sweep.sh"], 0);
    let outcome = run(&rig, &[]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert_eq!(outcome.out, "checking 4 scripts\n");
    assert_eq!(
        outcome.err,
        "sweep: no memory cap (no systemd user scope on this host); relying on the free-memory check.\n"
    );
    assert_eq!(
        rig.calls(),
        [
            "git ls-files".to_string(),
            format!("{SC} a.sh"),
            format!("{SC} b"),
            format!("{SC} profiles/p/verify.d/x.sh"),
            format!("{SC} scripts/shellcheck-sweep.sh"),
        ]
    );
}

#[test]
fn a_file_that_matches_both_rules_is_linted_once() {
    let rig = rig(&["shellcheck"]);
    rig.host.put_file("/t/profiles/p/verify.d/x.sh", "#!/bin/bash\necho both rules\n");
    listing(&rig, "profiles/p/verify.d/x.sh\nscripts/shellcheck-sweep.sh\n");
    script_all(&rig, &["profiles/p/verify.d/x.sh", "scripts/shellcheck-sweep.sh"], 0);
    let outcome = run(&rig, &[]);
    assert_eq!(outcome.out, "checking 2 scripts\n");
    assert_eq!(rig.calls().len(), 3);
}

#[test]
fn every_tool_the_script_starts_after_its_cd_runs_in_the_checkout() {
    let rig = rig(&["shellcheck", "nice", "systemd-run"]);
    rig.host.put_file("/meminfo", "MemAvailable: 1 kB\n");
    listing(&rig, "scripts/shellcheck-sweep.sh\n");
    rig.proc.script(&["sleep", "1"], Script::sequence([Ok(ok(""))]));
    let outcome = run(&rig, &[("MAGENT_SWEEP_WAIT", "1"), ("MAGENT_SWEEP_POLL", "1")]);
    assert_eq!(outcome.code, 75, "{outcome:?}");
    let probe = self::rig(&["shellcheck", "systemd-run"]);
    listing(&probe, "scripts/shellcheck-sweep.sh\n");
    probe.always(
        &[
            "systemd-run",
            "--user",
            "--scope",
            "--quiet",
            "-p",
            "MemoryMax=18G",
            "-p",
            "MemorySwapMax=0",
            "true",
        ],
        ok(""),
    );
    probe.always(
        &[
            "systemd-run",
            "--user",
            "--scope",
            "--quiet",
            "-p",
            "MemoryMax=18G",
            "-p",
            "MemorySwapMax=0",
            "shellcheck",
            "--severity=warning",
            "scripts/shellcheck-sweep.sh",
        ],
        ok(""),
    );
    run(&probe, &[]);
    for rig in [&rig, &probe] {
        let calls: Vec<_> = rig
            .log
            .entries()
            .into_iter()
            .filter_map(|effect| match effect {
                mx_ports::Effect::Proc(mx_ports::ProcEffect::Run(spec)) => Some(spec),
                _ => None,
            })
            .collect();
        assert!(
            calls
                .iter()
                .any(|spec| spec.argv.first() == "sleep" || spec.argv.first() == "systemd-run"),
            "{calls:?}"
        );
        for spec in calls {
            assert_eq!(spec.cwd.as_deref(), Some(std::path::Path::new("/t")), "{:?}", spec.argv);
        }
    }
}

#[test]
fn a_name_with_white_space_a_quoted_name_and_a_name_with_a_dash_are_left_out_silently() {
    let rig = rig(&["shellcheck"]);
    rig.host.put_file("/t/ok.sh", "#!/bin/bash\n");
    rig.host.put_file("/t/my script.sh", "#!/bin/bash\n");
    rig.host.put_file("/t/-x.sh", "#!/bin/bash\n");
    listing(&rig, "my script.sh\n\"caf\\303\\251.sh\"\n-x.sh\nok.sh\n");
    script_all(&rig, &["ok.sh"], 0);
    let outcome = run(&rig, &[]);
    assert_eq!(outcome.out, "checking 1 scripts\n");
    assert_eq!(rig.calls(), ["git ls-files".to_string(), format!("{SC} ok.sh")]);
}

#[test]
fn an_untracked_name_the_listing_does_not_hold_and_a_name_that_is_gone_are_not_swept() {
    let rig = rig(&["shellcheck"]);
    rig.host.put_file("/t/tracked.sh", "#!/bin/bash\n");
    rig.host.put_file("/t/untracked.sh", "#!/bin/bash\n");
    listing(&rig, "tracked.sh\ngone.sh\n");
    script_all(&rig, &["tracked.sh"], 0);
    let outcome = run(&rig, &[]);
    assert_eq!(outcome.out, "checking 1 scripts\n");
}

#[test]
fn a_checkout_with_no_script_says_it_inspected_nothing_and_exits_one() {
    let rig = rig(&["shellcheck"]);
    rig.host.put_file("/t/notes.txt", "text\n");
    listing(&rig, "notes.txt\n");
    let outcome = run(&rig, &[]);
    assert_eq!(outcome.code, 1);
    assert_eq!(outcome.out, "checking 0 scripts\n");
    assert_eq!(outcome.err, "no scripts found; this gate inspected nothing\n");
    assert_eq!(rig.calls(), ["git ls-files"]);
}

#[test]
fn gits_own_error_reaches_stderr_before_the_sweeps_lines() {
    let rig = rig(&["shellcheck"]);
    rig.always(&["git", "ls-files"], exited(128, "", "fatal: not a git repository\n"));
    let outcome = run(&rig, &[]);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        outcome.err,
        "fatal: not a git repository\nno scripts found; this gate inspected nothing\n"
    );
}

#[test]
fn a_host_with_no_git_prints_the_shells_message_with_the_scripts_line() {
    let rig = rig(&["shellcheck"]);
    rig.proc.script(
        &["git", "ls-files"],
        Script::always(Err(ProcError::NotFound { program: "git".to_string() })),
    );
    let outcome = run(&rig, &[]);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        outcome.err_lines()[0],
        "/t/scripts/shellcheck-sweep.sh: line 12: git: command not found"
    );
}

#[test]
fn with_no_linter_it_prints_the_install_hint_and_exits_127_before_looking_at_memory() {
    let rig = rig(&[]);
    rig.host.put_file("/t/a.sh", "#!/bin/bash\n");
    listing(&rig, "a.sh\n");
    let outcome = run(&rig, &[("MAGENT_MEMINFO", "/absent")]);
    assert_eq!(outcome.code, 127);
    assert_eq!(outcome.out, "checking 1 scripts\n");
    assert_eq!(outcome.err_lines().len(), 7);
    assert_eq!(
        outcome.err_lines()[0],
        "shellcheck is not installed, so this gate inspected nothing."
    );
    assert_eq!(
        outcome.err_lines()[6],
        "CI runs it either way, so a push will find what this did not."
    );
}

#[test]
fn memory_equal_to_the_need_starts_and_one_kilobyte_short_gives_up_at_once_with_75() {
    let rig = rig(&["shellcheck"]);
    rig.host.put_file("/meminfo", "MemAvailable: 10485760 kB\n");
    listing(&rig, "scripts/shellcheck-sweep.sh\n");
    script_all(&rig, &["scripts/shellcheck-sweep.sh"], 0);
    assert_eq!(run(&rig, &[("MAGENT_SWEEP_WAIT", "0")]).code, 0);

    let short = self::rig(&["shellcheck"]);
    short.host.put_file("/meminfo", "MemAvailable: 10485759 kB\n");
    listing(&short, "scripts/shellcheck-sweep.sh\n");
    let outcome = run(&short, &[("MAGENT_SWEEP_WAIT", "0")]);
    assert_eq!(outcome.code, 75);
    assert_eq!(
        outcome.err_lines(),
        [
            "sweep: MemAvailable is 10485759 KB and the linter needs 10485760 KB; gave up after 0s.",
            "sweep: close something heavy and retry, or set MAGENT_SWEEP_MIN_AVAIL_KB."
        ]
    );
    assert_eq!(short.calls(), ["git ls-files"]);
}

#[test]
fn it_sleeps_one_poll_at_a_time_and_counts_whole_polls_past_the_limit() {
    let rig = rig(&["shellcheck"]);
    rig.host.put_file("/meminfo", "MemAvailable: 1000 kB\n");
    listing(&rig, "scripts/shellcheck-sweep.sh\n");
    rig.always(&["sleep", "10"], ok(""));
    let outcome = run(&rig, &[("MAGENT_SWEEP_WAIT", "25"), ("MAGENT_SWEEP_POLL", "10")]);
    assert_eq!(outcome.code, 75);
    assert!(outcome.err.contains("gave up after 30s."), "{}", outcome.err);
    assert_eq!(rig.calls(), ["git ls-files", "sleep 10", "sleep 10", "sleep 10"]);
}

#[test]
fn a_meminfo_with_no_usable_line_refuses_naming_the_file_and_the_need() {
    for body in
        ["MemTotal: 1 kB\n", "MemAvailable: lots kB\n", "MemAvailable:\n", "MemAvailable: 5 kB"]
    {
        let rig = rig(&["shellcheck"]);
        rig.host.put_file("/meminfo", body);
        listing(&rig, "scripts/shellcheck-sweep.sh\n");
        let outcome = run(&rig, &[]);
        assert_eq!(outcome.code, 75, "{body:?}");
        assert_eq!(
            outcome.err,
            "sweep: no usable MemAvailable in /meminfo; not starting a job that needs 10485760 KB.\n",
            "{body:?}"
        );
    }
}

#[test]
fn an_unreadable_meminfo_is_a_warning_and_the_sweep_goes_on() {
    let rig = rig(&["shellcheck"]);
    listing(&rig, "scripts/shellcheck-sweep.sh\n");
    script_all(&rig, &["scripts/shellcheck-sweep.sh"], 0);
    let outcome = run(&rig, &[("MAGENT_MEMINFO", "/absent-meminfo")]);
    assert_eq!(outcome.code, 0);
    assert!(
        outcome.err.starts_with(
            "sweep: /absent-meminfo is not readable here; not checking free memory.\n"
        ),
        "{}",
        outcome.err
    );
}

#[test]
fn under_ci_neither_the_gate_nor_the_cap_runs() {
    let rig = rig(&["shellcheck", "nice", "systemd-run"]);
    rig.host.put_file("/meminfo", "MemAvailable: 1 kB\n");
    listing(&rig, "scripts/shellcheck-sweep.sh\n");
    rig.always(
        &["nice", "-n", "15", "shellcheck", "--severity=warning", "scripts/shellcheck-sweep.sh"],
        ok(""),
    );
    let outcome = run(&rig, &[("CI", "1")]);
    assert_eq!((outcome.code, outcome.err.as_str()), (0, ""));
    assert_eq!(
        rig.calls(),
        ["git ls-files", "nice -n 15 shellcheck --severity=warning scripts/shellcheck-sweep.sh"]
    );
    let empty = self::rig(&["shellcheck"]);
    empty.host.put_file("/meminfo", "MemAvailable: 1 kB\n");
    listing(&empty, "scripts/shellcheck-sweep.sh\n");
    assert_eq!(run(&empty, &[("CI", ""), ("MAGENT_SWEEP_WAIT", "0")]).code, 75);
}

#[test]
fn with_a_user_scope_the_host_is_probed_once_and_every_run_goes_scope_nice_linter() {
    let rig = rig(&["shellcheck", "nice", "systemd-run"]);
    listing(&rig, "scripts/shellcheck-sweep.sh\n");
    rig.always(
        &[
            "systemd-run",
            "--user",
            "--scope",
            "--quiet",
            "-p",
            "MemoryMax=18G",
            "-p",
            "MemorySwapMax=0",
            "true",
        ],
        ok(""),
    );
    rig.always(
        &[
            "systemd-run",
            "--user",
            "--scope",
            "--quiet",
            "-p",
            "MemoryMax=18G",
            "-p",
            "MemorySwapMax=0",
            "nice",
            "-n",
            "15",
            "shellcheck",
            "--severity=warning",
            "scripts/shellcheck-sweep.sh",
        ],
        ok(""),
    );
    let outcome = run(&rig, &[]);
    assert_eq!((outcome.code, outcome.err.as_str()), (0, ""));
    assert_eq!(
        rig.calls(),
        [
            "git ls-files".to_string(),
            format!("{SCOPE} true"),
            format!("{SCOPE} nice -n 15 {SC} scripts/shellcheck-sweep.sh"),
        ]
    );
}

#[test]
fn the_cap_variable_reaches_the_probe_and_the_run_and_a_failing_probe_runs_uncapped() {
    let rig = rig(&["shellcheck", "systemd-run"]);
    listing(&rig, "scripts/shellcheck-sweep.sh\n");
    rig.proc.script(
        &[
            "systemd-run",
            "--user",
            "--scope",
            "--quiet",
            "-p",
            "MemoryMax=3G",
            "-p",
            "MemorySwapMax=0",
            "true",
        ],
        Script::always(Ok(exited(1, "", ""))),
    );
    rig.always(&["shellcheck", "--severity=warning", "scripts/shellcheck-sweep.sh"], ok(""));
    let outcome = run(&rig, &[("MAGENT_SWEEP_MEM_MAX", "3G")]);
    assert_eq!(
        outcome.err,
        "sweep: no memory cap (no systemd user scope on this host); relying on the free-memory check.\n"
    );
    assert_eq!(
        rig.calls()[1],
        "systemd-run --user --scope --quiet -p MemoryMax=3G -p MemorySwapMax=0 true"
    );
}

#[test]
fn the_no_cap_variable_skips_the_probe_and_says_nothing_but_the_empty_string_does_not() {
    let rig = rig(&["shellcheck", "systemd-run"]);
    listing(&rig, "scripts/shellcheck-sweep.sh\n");
    rig.always(&["shellcheck", "--severity=warning", "scripts/shellcheck-sweep.sh"], ok(""));
    let outcome = run(&rig, &[("MAGENT_SWEEP_NO_CAP", "1")]);
    assert_eq!((outcome.code, outcome.err.as_str()), (0, ""));
    assert_eq!(
        rig.calls(),
        ["git ls-files".to_string(), format!("{SC} scripts/shellcheck-sweep.sh")]
    );
}

#[test]
fn the_exit_code_is_the_largest_status_and_every_script_is_still_linted() {
    let rig = rig(&["shellcheck"]);
    for name in ["a.sh", "b.sh", "c.sh"] {
        rig.host.put_file(format!("/t/{name}"), "#!/bin/bash\n");
    }
    listing(&rig, "a.sh\nb.sh\nc.sh\n");
    rig.always(
        &["shellcheck", "--severity=warning", "a.sh"],
        exited(2, "In a.sh line 2: warning\n", "diagnostic\n"),
    );
    rig.always(&["shellcheck", "--severity=warning", "b.sh"], exited(1, "", ""));
    rig.always(&["shellcheck", "--severity=warning", "c.sh"], exited(0, "", ""));
    let outcome = run(&rig, &[]);
    assert_eq!(outcome.code, 2);
    assert_eq!(outcome.out, "checking 3 scripts\nIn a.sh line 2: warning\n");
    assert!(outcome.err.ends_with("diagnostic\n"), "{}", outcome.err);
    assert_eq!(rig.calls().len(), 4);
}

#[test]
fn a_status_of_137_stops_the_sweep_names_the_script_and_the_cap() {
    let rig = rig(&["shellcheck"]);
    rig.host.put_file("/t/a.sh", "#!/bin/bash\n");
    rig.host.put_file("/t/b.sh", "#!/bin/bash\n");
    listing(&rig, "a.sh\nb.sh\n");
    rig.always(&["shellcheck", "--severity=warning", "a.sh"], exited(137, "", ""));
    rig.always(&["shellcheck", "--severity=warning", "b.sh"], exited(1, "", ""));
    let outcome = run(&rig, &[("MAGENT_SWEEP_MEM_MAX", "2G")]);
    assert_eq!(outcome.code, 137);
    assert!(
        outcome.err.ends_with("sweep: shellcheck was killed (rc=137) on a.sh, most likely by the MemoryMax=2G cap. Raise MAGENT_SWEEP_MEM_MAX.\n"),
        "{}",
        outcome.err
    );
    assert_eq!(
        rig.calls(),
        ["git ls-files".to_string(), format!("{SC} a.sh")],
        "the script after the kill is never linted"
    );
}

#[test]
fn a_status_above_137_is_passed_on_and_is_not_a_kill() {
    let rig = rig(&["shellcheck"]);
    listing(&rig, "scripts/shellcheck-sweep.sh\n");
    rig.always(
        &["shellcheck", "--severity=warning", "scripts/shellcheck-sweep.sh"],
        exited(255, "", ""),
    );
    let outcome = run(&rig, &[]);
    assert_eq!(outcome.code, 255);
    assert!(!outcome.err.contains("killed"));
}

#[test]
fn the_linter_runs_in_the_checkout_whatever_the_callers_directory() {
    let rig = rig(&["shellcheck"]);
    listing(&rig, "scripts/shellcheck-sweep.sh\n");
    rig.always(&["shellcheck", "--severity=warning", "scripts/shellcheck-sweep.sh"], ok(""));
    run(&rig, &[]);
    for effect in rig.log.entries() {
        if let mx_ports::Effect::Proc(mx_ports::ProcEffect::Run(spec)) = effect {
            assert_eq!(spec.cwd.as_deref(), Some(std::path::Path::new("/t")), "{:?}", spec.argv);
        }
    }
}

#[test]
fn a_wait_or_poll_that_is_not_whole_seconds_refuses_instead_of_looping() {
    for (wait, poll) in [("abc", "1"), ("5", "abc"), ("5", "08"), ("1.5", "1")] {
        let rig = rig(&["shellcheck"]);
        rig.host.put_file("/meminfo", "MemAvailable: 1 kB\n");
        listing(&rig, "scripts/shellcheck-sweep.sh\n");
        let outcome = run(&rig, &[("MAGENT_SWEEP_WAIT", wait), ("MAGENT_SWEEP_POLL", poll)]);
        assert_eq!(outcome.code, 75, "{wait}/{poll}");
        assert!(outcome.err.contains("must be whole numbers of seconds"), "{}", outcome.err);
        assert_eq!(rig.calls(), ["git ls-files"]);
    }
}

#[test]
fn a_need_that_is_not_a_number_prints_the_shells_test_error_and_waits() {
    let rig = rig(&["shellcheck"]);
    listing(&rig, "scripts/shellcheck-sweep.sh\n");
    let outcome = run(&rig, &[("MAGENT_SWEEP_MIN_AVAIL_KB", "abc"), ("MAGENT_SWEEP_WAIT", "0")]);
    assert_eq!(outcome.code, 75);
    assert_eq!(
        outcome.err_lines()[0],
        "/t/scripts/shellcheck-sweep.sh: line 81: [: abc: integer expected"
    );
    assert!(outcome.err.contains("the linter needs abc KB; gave up after 0s."));
}

#[test]
fn the_program_finds_its_checkout_through_a_link_to_the_real_script() {
    let rig = rig(&["shellcheck"]);
    rig.host.put_link("/elsewhere/sweep.sh", "/t/scripts/shellcheck-sweep.sh");
    listing(&rig, "scripts/shellcheck-sweep.sh\n");
    rig.always(&["shellcheck", "--severity=warning", "scripts/shellcheck-sweep.sh"], ok(""));
    let outcome = rig.run("/elsewhere/sweep.sh", &env(&[]), sweep::run);
    assert_eq!(outcome.code, 0, "{outcome:?}");
}
