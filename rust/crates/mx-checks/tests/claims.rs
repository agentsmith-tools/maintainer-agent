//! `check-claims` over fakes: what it asks, in which order, and what it prints.

mod common;

use common::{Rig, exited, ok, timed_out};
use mx_checks::claims;

const SCRIPT: &str = "/t/scripts/check_claims.sh";
const SUITE: [&str; 2] = ["bash", "/t/tests/run-tests.sh"];
const FIGS_CHECK: [&str; 7] =
    ["python3", "/t/scripts/render_figures.py", "--root", "/t", "--check", "--tests", "10"];
const FIGS_VALUES: [&str; 7] =
    ["python3", "/t/scripts/render_figures.py", "--root", "/t", "--values", "--tests", "10"];
const WALL: &str =
    "python3 /t/scripts/render-settings.py /tmp/tmp.fake000002/sysknife /home/claimcheck";
const COUNT: &str = "python3 -c \nimport json;print(len(json.load(open('/tmp/tmp.fake000002/sysknife/settings.json'))['permissions']['deny']))";
const WALL_ALL: &str = "python3 /t/scripts/render-settings.py /tmp/tmp.fake000002 /home/claimcheck";
const COUNT_ALL: &str = "python3 -c \nimport json\nprint(len(json.load(open('/tmp/tmp.fake000002/settings.json'))['permissions']['deny']))";

/// A rig with a green suite, one profile, and a figure check that passes.
fn green(summary: &str) -> Rig {
    green_with(
        summary,
        ok("ok: 3 figure markers in 2 files are current\n"),
        ok("tests 10\nevals 9\n"),
    )
}

/// The same, with the answers of the two figure calls chosen.
fn green_with(summary: &str, check: mx_ports::ProcOutput, values: mx_ports::ProcOutput) -> Rig {
    let rig = Rig::new();
    rig.host.put_file("/t/profiles/sysknife/deny.json", "{}");
    rig.always(&SUITE, ok(summary));
    rig.always(
        &[
            "python3",
            "/t/scripts/render-settings.py",
            "/tmp/tmp.fake000002/sysknife",
            "/home/claimcheck",
        ],
        ok(""),
    );
    rig.always(&["python3", "-c", "\nimport json;print(len(json.load(open('/tmp/tmp.fake000002/sysknife/settings.json'))['permissions']['deny']))"], ok("40\n"));
    rig.always(
        &["python3", "/t/scripts/render-settings.py", "/tmp/tmp.fake000002", "/home/claimcheck"],
        ok(""),
    );
    rig.always(&["python3", "-c", "\nimport json\nprint(len(json.load(open('/tmp/tmp.fake000002/settings.json'))['permissions']['deny']))"], ok("41\n"));
    rig.always(&FIGS_CHECK, check);
    rig.always(&FIGS_VALUES, values);
    rig
}

fn run(rig: &Rig) -> common::Outcome {
    rig.run(SCRIPT, &[], claims::run)
}

#[test]
fn a_green_run_prints_the_profile_rule_and_figure_lines_and_exits_zero() {
    let rig = green("  10 passed, 0 failed\n");
    let outcome = run(&rig);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert_eq!(
        outcome.out_lines(),
        [
            "  profile sysknife: 40 rules",
            "  those verbs generate 41 rules on this machine",
            "  ok: 3 figure markers in 2 files are current",
            "  measured: tests 10, evals 9",
        ]
    );
    assert_eq!(outcome.err, "");
}

#[test]
fn the_calls_come_in_the_scripts_order_with_the_scripts_arguments() {
    let rig = green("  10 passed, 0 failed\n");
    run(&rig);
    assert_eq!(
        rig.calls(),
        [
            SUITE.join(" "),
            WALL.to_string(),
            COUNT.to_string(),
            WALL_ALL.to_string(),
            COUNT_ALL.to_string(),
            FIGS_CHECK.join(" "),
            FIGS_VALUES.join(" "),
        ]
    );
}

#[test]
fn the_suite_output_file_stays_and_the_generated_walls_go() {
    let rig = green("  10 passed, 0 failed\n");
    run(&rig);
    // Pins the script's replaced trap: the second `trap ... EXIT` forgets the file the suite wrote.
    assert_eq!(rig.host.peek("/tmp/tmp.fake000001"), Some(b"  10 passed, 0 failed\n".to_vec()));
    assert!(
        rig.host.paths().iter().all(|path| !path.starts_with("/tmp/tmp.fake000002")),
        "{:?}",
        rig.host.paths()
    );
}

#[test]
fn skipped_cases_count_and_the_run_says_so() {
    let rig = Rig::new();
    rig.host.put_file("/t/profiles/sysknife/deny.json", "{}");
    rig.always(&SUITE, ok("  10 passed, 0 failed, 3 skipped\n"));
    for argv in [
        vec![
            "python3",
            "/t/scripts/render-settings.py",
            "/tmp/tmp.fake000002/sysknife",
            "/home/claimcheck",
        ],
        vec!["python3", "/t/scripts/render-settings.py", "/tmp/tmp.fake000002", "/home/claimcheck"],
    ] {
        rig.always(&argv, ok(""));
    }
    rig.always(&["python3", "-c", "\nimport json;print(len(json.load(open('/tmp/tmp.fake000002/sysknife/settings.json'))['permissions']['deny']))"], ok("1\n"));
    rig.always(&["python3", "-c", "\nimport json\nprint(len(json.load(open('/tmp/tmp.fake000002/settings.json'))['permissions']['deny']))"], ok("1\n"));
    let check =
        ["python3", "/t/scripts/render_figures.py", "--root", "/t", "--check", "--tests", "13"];
    let values =
        ["python3", "/t/scripts/render_figures.py", "--root", "/t", "--values", "--tests", "13"];
    rig.always(&check, ok("ok\n"));
    rig.always(&values, ok("tests 13\n"));
    let outcome = run(&rig);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert_eq!(outcome.out_lines()[0], "  counting 10 passed + 3 skipped as the suite size");
    assert!(rig.calls().contains(&check.join(" ")));
}

#[test]
fn a_red_suite_prints_the_fail_line_and_ten_failing_cases_and_removes_its_file() {
    let rig = Rig::new();
    let stderr: String =
        (1..=12).map(|i| format!("  FAIL case {i}\n")).collect::<Vec<_>>().concat();
    rig.always(&SUITE, exited(1, "  PASS a\n", &stderr));
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1);
    assert_eq!(outcome.out, "");
    let lines = outcome.err_lines();
    assert_eq!(lines[0], "FAIL: the test suite is RED; fix that before believing any number here");
    assert_eq!(lines.len(), 11);
    assert_eq!(lines[10], "  FAIL case 10");
    assert_eq!(rig.calls(), [SUITE.join(" ")]);
    assert!(rig.host.paths().iter().all(|path| !path.starts_with("/tmp/tmp.fake000001")));
}

#[test]
fn a_suite_that_runs_past_its_deadline_is_red_with_one_extra_line() {
    let rig = Rig::new();
    rig.always(&SUITE, timed_out());
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        outcome.err_lines()[0],
        "FAIL: the test suite is RED; fix that before believing any number here"
    );
    assert_eq!(
        outcome.err_lines()[1],
        "check_claims: the suite did not finish within 7200 seconds"
    );
}

#[test]
fn a_green_suite_with_no_summary_is_refused_before_any_profile_is_rendered() {
    let rig = Rig::new();
    rig.always(&SUITE, ok("everything is fine, no summary line\n"));
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        outcome.err,
        "FAIL: could not measure the test count; refusing to pass over nothing\n"
    );
    assert_eq!(rig.calls(), [SUITE.join(" ")]);
    assert!(rig.host.paths().iter().all(|path| !path.starts_with("/tmp/tmp.fake000001")));
}

#[test]
fn a_profile_that_does_not_render_is_named_and_fails_the_run_but_the_run_goes_on() {
    let rig = Rig::new();
    rig.host.put_file("/t/profiles/broken/deny.json", "{}");
    rig.host.put_file("/t/profiles/sysknife/deny.json", "{}");
    rig.always(&SUITE, ok("  10 passed\n"));
    rig.always(
        &[
            "python3",
            "/t/scripts/render-settings.py",
            "/tmp/tmp.fake000002/broken",
            "/home/claimcheck",
        ],
        exited(1, "", "no credential path"),
    );
    rig.always(&["python3", "-c", "\nimport json;print(len(json.load(open('/tmp/tmp.fake000002/broken/settings.json'))['permissions']['deny']))"], exited(1, "", "boom"));
    rig.always(
        &[
            "python3",
            "/t/scripts/render-settings.py",
            "/tmp/tmp.fake000002/sysknife",
            "/home/claimcheck",
        ],
        ok(""),
    );
    rig.always(&["python3", "-c", "\nimport json;print(len(json.load(open('/tmp/tmp.fake000002/sysknife/settings.json'))['permissions']['deny']))"], ok("7\n"));
    rig.always(
        &["python3", "/t/scripts/render-settings.py", "/tmp/tmp.fake000002", "/home/claimcheck"],
        ok(""),
    );
    rig.always(&["python3", "-c", "\nimport json\nprint(len(json.load(open('/tmp/tmp.fake000002/settings.json'))['permissions']['deny']))"], ok("8\n"));
    rig.always(&FIGS_CHECK, ok("ok\n"));
    rig.always(&FIGS_VALUES, ok("tests 10\n"));
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1);
    assert_eq!(outcome.err, "FAIL: profile 'broken' does not generate a deny wall\n");
    assert_eq!(outcome.out_lines()[0], "  profile broken:  rules");
    assert_eq!(outcome.out_lines()[1], "  profile sysknife: 7 rules");
}

#[test]
fn with_no_profile_the_literal_glob_is_a_profile_named_star_and_cp_says_it_cannot_stat_it() {
    let rig = Rig::new();
    rig.always(&SUITE, ok("  10 passed\n"));
    rig.always(
        &["python3", "/t/scripts/render-settings.py", "/tmp/tmp.fake000002/*", "/home/claimcheck"],
        exited(1, "", ""),
    );
    rig.always(&["python3", "-c", "\nimport json;print(len(json.load(open('/tmp/tmp.fake000002/*/settings.json'))['permissions']['deny']))"], exited(1, "", ""));
    rig.always(
        &["python3", "/t/scripts/render-settings.py", "/tmp/tmp.fake000002", "/home/claimcheck"],
        exited(1, "", ""),
    );
    rig.always(&["python3", "-c", "\nimport json\nprint(len(json.load(open('/tmp/tmp.fake000002/settings.json'))['permissions']['deny']))"], exited(1, "", ""));
    rig.always(&FIGS_CHECK, ok("ok\n"));
    rig.always(&FIGS_VALUES, ok("tests 10\n"));
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        outcome.err_lines(),
        [
            "cp: cannot stat '/t/profiles/*/deny.json': No such file or directory",
            "FAIL: profile '*' does not generate a deny wall"
        ]
    );
    assert!(
        outcome.out.contains("  those verbs generate  rules on this machine"),
        "{}",
        outcome.out
    );
}

#[test]
fn profiles_are_rendered_in_byte_order_and_a_directory_without_deny_json_is_skipped() {
    let rig = Rig::new();
    for name in ["sysknife", "magent", "_template"] {
        rig.host.put_file(format!("/t/profiles/{name}/deny.json"), "{}");
    }
    rig.host.put_dir("/t/profiles/empty");
    rig.host.put_file("/t/profiles/.hidden/deny.json", "{}");
    rig.always(&SUITE, ok("  10 passed\n"));
    for name in ["_template", "magent", "sysknife"] {
        rig.always(
            &[
                "python3",
                "/t/scripts/render-settings.py",
                &format!("/tmp/tmp.fake000002/{name}"),
                "/home/claimcheck",
            ],
            ok(""),
        );
        rig.always(
            &["python3", "-c", &format!("\nimport json;print(len(json.load(open('/tmp/tmp.fake000002/{name}/settings.json'))['permissions']['deny']))")],
            ok("1\n"),
        );
    }
    rig.always(
        &["python3", "/t/scripts/render-settings.py", "/tmp/tmp.fake000002", "/home/claimcheck"],
        ok(""),
    );
    rig.always(&["python3", "-c", "\nimport json\nprint(len(json.load(open('/tmp/tmp.fake000002/settings.json'))['permissions']['deny']))"], ok("2\n"));
    rig.always(&FIGS_CHECK, ok("ok\n"));
    rig.always(&FIGS_VALUES, ok("tests 10\n"));
    let outcome = run(&rig);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    let profiles: Vec<&str> =
        outcome.out_lines().into_iter().filter(|line| line.starts_with("  profile ")).collect();
    assert_eq!(
        profiles,
        [
            "  profile _template: 1 rules",
            "  profile magent: 1 rules",
            "  profile sysknife: 1 rules"
        ]
    );
}

#[test]
fn a_stale_figure_prints_its_report_and_the_fail_line_and_never_asks_for_values() {
    let rig =
        green_with("  10 passed\n", exited(1, "", "FAIL README.md:639: stale marker\n"), ok(""));
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        outcome.err_lines(),
        [
            "FAIL README.md:639: stale marker",
            "FAIL: a figure in the documents is stale, typed outside a marker, or unreadable; run scripts/render_figures.py --write"
        ]
    );
    assert!(!rig.calls().contains(&FIGS_VALUES.join(" ")));
}

#[test]
fn a_values_run_that_fails_prints_its_output_and_fails_the_run_after_the_ok_line() {
    let rig = green_with(
        "  10 passed\n",
        ok("ok: 3 figure markers in 2 files are current\n"),
        exited(1, "", "cannot read docs/lessons.md\n"),
    );
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        outcome.err_lines(),
        ["cannot read docs/lessons.md", "FAIL: could not print the measured figures"]
    );
    assert!(outcome.out.contains("  ok: 3 figure markers"));
    assert!(!outcome.out.contains("measured"));
}

#[test]
fn a_multi_line_report_is_indented_on_its_first_line_only() {
    let rig = green_with("  10 passed\n", ok("first line\nsecond line\n"), ok("tests 10\n"));
    let outcome = run(&rig);
    assert!(outcome.out.contains("  first line\nsecond line\n"), "{}", outcome.out);
}

#[test]
fn every_child_sees_the_callers_variables_but_not_path_or_lc_all() {
    let rig = green("  10 passed\n");
    rig.run(
        SCRIPT,
        &[("HOME", "/h"), ("CZ_CASE_DIR", "/c"), ("PATH", "/bin"), ("LC_ALL", "C")],
        claims::run,
    );
    assert_eq!(rig.env_names_of("bash /t/tests/run-tests.sh"), ["CZ_CASE_DIR", "HOME"]);
    assert_eq!(rig.env_names_of("python3 /t/scripts/render_figures.py"), ["CZ_CASE_DIR", "HOME"]);
}

#[test]
fn a_temporary_file_that_cannot_be_made_ends_the_run_before_the_suite() {
    let rig = Rig::new();
    rig.host.fail_temp(mx_checks::host::HostError::new(
        mx_checks::host::Failure::Permission,
        "Permission denied",
    ));
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        outcome.err,
        "check_claims: cannot make a temporary file under /tmp: Permission denied\n"
    );
    assert!(rig.calls().is_empty());
}

#[test]
fn the_tree_is_found_from_the_programs_own_path() {
    let rig = Rig::new();
    rig.always(&["bash", "/other/tests/run-tests.sh"], exited(1, "", ""));
    let outcome = rig.run("/other/scripts/check_claims.sh", &[], claims::run);
    assert_eq!(outcome.code, 1);
    assert_eq!(rig.calls(), ["bash /other/tests/run-tests.sh"]);
}

#[test]
fn a_nul_byte_in_a_renderers_output_is_dropped_with_the_shells_warning_once_per_substitution() {
    let rig = green_with("  10 passed\n", ok("ok\0nul\n"), ok("a\0b 1\n"));
    let outcome = run(&rig);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert_eq!(
        outcome.err_lines(),
        [
            "/t/scripts/check_claims.sh: line 65: warning: command substitution: ignored null byte in input",
            "/t/scripts/check_claims.sh: line 71: warning: command substitution: ignored null byte in input"
        ]
    );
    assert!(outcome.out.contains("  oknul\n"), "{}", outcome.out);
    assert!(outcome.out.contains("  measured: ab 1\n"), "{}", outcome.out);
}
