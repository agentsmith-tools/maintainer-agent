//! The Audit trail and Deployed code sections.

mod common;

use common::*;
use mx_core::Fact;
use mx_doctor::audit::{
    self, Crash, DeployFacts, LeaseFile, decide_deployed, decide_leases, failure_reason,
    forgotten_leases,
};
use mx_doctor::ctx::Stop;
use mx_doctor::report::Item;

const NOWI: i64 = 1_767_614_400;
const ST: &str = "/state";

fn audit_rig(pairs: &[(&str, &str)]) -> Rig {
    let mut all = vec![
        ("STATE_DIR", ST),
        ("GH_ACCOUNT", "testuser"),
        ("REPO_SLUG", "o/r"),
        ("REPO_PATH", "/r"),
    ];
    all.extend_from_slice(pairs);
    let rig = Rig::new(&all);
    rig.host.dir(ST);
    rig.proc.on(
        &["gh", "auth", "token", "--user", "testuser"],
        0,
        "ghp_FAKEFAKEFAKEFAKEFAKE0000\n",
    );
    rig
}

fn run_audit(rig: &Rig) -> (mx_doctor::ctx::Out<'static>, Result<(), Stop>) {
    let mut o = out();
    let r = audit::audit_trail(&rig.ctx(), &mut o);
    (o, r)
}

fn lease(hours_ago: i64, extra_minutes: i64) -> String {
    format!("{{\"renewed\": {}}}", NOWI - hours_ago * 3600 - extra_minutes * 60)
}

fn lease_file(name: &str, body: &str) -> LeaseFile {
    LeaseFile { name: name.into(), contents: Fact::Known(body.as_bytes().to_vec()) }
}

// --- state directory ---------------------------------------------------------------

#[test]
fn a_profile_with_no_state_dir_fails_and_skips_the_rest_of_the_audit_trail() {
    let rig = Rig::new(&[("STATE_DIR", "")]);
    let (o, r) = run_audit(&rig);
    assert!(r.is_ok());
    assert!(has_fail(
        &o.report,
        "the profile names no STATE_DIR",
        "set it in profiles/magent/profile.env"
    ));
    assert_eq!(o.report.counts(), (0, 0, 1));
}

#[test]
fn a_state_directory_that_is_not_a_directory_is_no_state_yet_with_no_fix_line() {
    let rig = Rig::new(&[("STATE_DIR", "/nope")]);
    let (o, _) = run_audit(&rig);
    assert_eq!(
        lines(&o.report),
        ["", "Audit trail", "  warn  no state yet; the first run creates it"]
    );
}

#[test]
fn run_reports_and_receipts_are_counted_without_dot_files() {
    let rig = audit_rig(&[]);
    for name in ["r1.md", "r2.md", ".hidden"] {
        rig.host.file(&format!("{ST}/runs/{name}"), b"x", false, 0);
    }
    rig.host.file(&format!("{ST}/receipts/notes.txt"), b"x", false, 0);
    let (o, _) = run_audit(&rig);
    assert!(has_ok(&o.report, "2 run report(s), 1 open receipt(s)"));
}

#[test]
fn a_state_directory_without_the_write_bit_fails_with_the_chown_fix() {
    let rig = audit_rig(&[]);
    rig.host.make_read_only(ST);
    let (o, _) = run_audit(&rig);
    assert!(has_fail(&o.report, "state directory not writable", "chown it"));
    assert!(!has_ok(&o.report, "state directory writable"));
}

#[test]
fn the_audit_trail_prints_counts_receipts_leases_writability_token_and_tasks_in_that_order() {
    let rig = audit_rig(&[]);
    rig.host.file(&format!("{ST}/receipts/12.json"), b"{}", false, 0);
    rig.proc.on(
        &["gh", "pr", "view", "12", "--repo", "o/r", "--json", "state", "--jq", ".state"],
        0,
        "MERGED\n",
    );
    rig.host.file(&format!("{ST}/leases/pr-2.json"), lease(13, 20).as_bytes(), false, 0);
    age(&rig, "failed-review.json", 5, "{}");
    let (o, _) = run_audit(&rig);
    let text = dump(&o.report);
    let at =
        |needle: &str| text.find(needle).unwrap_or_else(|| panic!("{needle} missing in {text}"));
    let order = [
        at("run report(s)"),
        at("receipt(s) for an already merged"),
        at("lease(s) nobody has renewed"),
        at("state directory writable"),
        at("no pinned token left"),
        at("review has never completed"),
    ];
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?} in {text}");
}

// --- receipts -------------------------------------------------------------------------

#[test]
fn merged_and_closed_pull_requests_are_listed_in_file_name_order_and_open_ones_are_not() {
    let rig = audit_rig(&[]);
    for pr in ["9", "12", "100", "14"] {
        rig.host.file(&format!("{ST}/receipts/{pr}.json"), b"{}", false, 0);
    }
    for (pr, state) in [("9", "CLOSED"), ("12", "MERGED"), ("100", "MERGED"), ("14", "OPEN")] {
        rig.proc.on(
            &["gh", "pr", "view", pr, "--repo", "o/r", "--json", "state", "--jq", ".state"],
            0,
            &format!("{state}\n"),
        );
    }
    let (o, _) = run_audit(&rig);
    assert!(has_warn(
        &o.report,
        "receipt(s) for an already merged pull request: 100 (MERGED) 12 (MERGED) 9 (CLOSED)",
        &format!("rm {ST}/receipts/<pr>.json once you have checked the work really landed")
    ));
}

#[test]
fn a_pull_request_gh_cannot_look_up_is_skipped_and_a_non_numeric_name_is_never_asked() {
    let rig = audit_rig(&[]);
    rig.host.file(&format!("{ST}/receipts/15.json"), b"{}", false, 0);
    rig.host.file(&format!("{ST}/receipts/abc.json"), b"{}", false, 0);
    rig.proc.on(
        &["gh", "pr", "view", "15", "--repo", "o/r", "--json", "state", "--jq", ".state"],
        1,
        "",
    );
    let (o, _) = run_audit(&rig);
    assert!(has_ok(&o.report, "no receipt outlives its pull request"));
    assert!(!rig.proc.tool_calls().iter().any(|c| c.contains("abc")));
}

#[test]
fn a_receipt_check_with_no_repo_slug_asks_gh_about_an_empty_repo() {
    let rig = audit_rig(&[("REPO_SLUG", "")]);
    rig.host.file(&format!("{ST}/receipts/15.json"), b"{}", false, 0);
    rig.proc.on(
        &["gh", "pr", "view", "15", "--repo", "", "--json", "state", "--jq", ".state"],
        1,
        "",
    );
    let _ = run_audit(&rig);
}

// --- leases -----------------------------------------------------------------------------

#[test]
fn a_lease_exactly_at_the_limit_is_not_forgotten_and_one_minute_past_it_is() {
    let at = [lease_file("pr-1.json", &lease(12, 0))];
    assert_eq!(forgotten_leases("12", &at, NOWI), Ok(vec![]));
    let past = [lease_file("pr-1.json", &lease(12, 1))];
    assert_eq!(forgotten_leases("12", &past, NOWI), Ok(vec!["#1 (renewed 12h ago)".to_string()]));
}

#[test]
fn leases_print_in_file_name_order_with_the_age_rounded_to_whole_hours() {
    let files = [
        lease_file("pr-11.json", &lease(40, 20)),
        lease_file("pr-2.json", &lease(13, 20)),
        lease_file("pr-3.json", &lease(1, 0)),
    ];
    assert_eq!(
        forgotten_leases("12", &files, NOWI),
        Ok(vec!["#11 (renewed 40h ago)".to_string(), "#2 (renewed 13h ago)".to_string()])
    );
}

#[test]
fn a_lease_that_is_not_json_lacks_renewed_or_holds_no_number_is_reported_unreadable() {
    let files = [
        lease_file("pr-4.json", "not json"),
        lease_file("pr-5.json", "{\"x\": 1}"),
        lease_file("pr-6.json", "{\"renewed\": \"abc\"}"),
        lease_file("pr-7.json", "{\"renewed\": null}"),
        lease_file("pr-8.json", "[1]"),
        LeaseFile { name: "pr-9.json".into(), contents: Fact::Unreadable("denied".into()) },
    ];
    let want: Vec<String> = (4..=9).map(|n| format!("#{n} (unreadable)")).collect();
    assert_eq!(forgotten_leases("12", &files, NOWI), Ok(want));
}

#[test]
fn a_renewed_time_may_be_text_a_float_or_a_boolean_as_python_reads_it() {
    let old = NOWI - 13 * 3600 - 1200;
    let files = [
        lease_file("pr-1.json", &format!("{{\"renewed\": \"{old}\"}}")),
        lease_file("pr-2.json", &format!("{{\"renewed\": {old}.9}}")),
        lease_file("pr-3.json", "{\"renewed\": true}"),
    ];
    let got = forgotten_leases("12", &files, NOWI).unwrap();
    assert_eq!(got.len(), 3);
    assert_eq!(got[0], "#1 (renewed 13h ago)");
    assert_eq!(got[1], "#2 (renewed 13h ago)");
    assert!(got[2].starts_with("#3 (renewed "), "{got:?}");
}

#[test]
fn a_boolean_true_reads_as_the_integer_one_and_false_as_zero() {
    // At time 7200 a lease renewed at 1 is 7199 s old and one renewed at 0 is 7200 s old.
    let limit_hours = "1.9998611111111111";
    let files = [lease_file("pr-1.json", "{\"renewed\": true}")];
    assert_eq!(forgotten_leases(limit_hours, &files, 7200), Ok(vec![]));
    let files = [lease_file("pr-2.json", "{\"renewed\": false}")];
    assert_eq!(
        forgotten_leases(limit_hours, &files, 7200),
        Ok(vec!["#2 (renewed 2h ago)".to_string()])
    );
}

#[test]
fn an_hours_limit_python_cannot_read_and_an_infinite_renewed_time_both_crash_the_reader() {
    assert_eq!(forgotten_leases("abc", &[], NOWI), Err(Crash));
    assert_eq!(forgotten_leases("", &[], NOWI), Err(Crash));
    let files = [lease_file("pr-1.json", "{\"renewed\": 1e999}")];
    assert_eq!(forgotten_leases("12", &files, NOWI), Err(Crash));
}

#[test]
fn the_lease_finding_lists_the_pull_requests_and_the_release_command_with_its_trailing_space() {
    let found = vec![
        "#2 (renewed 13h ago)".to_string(),
        "#11 (unreadable)".to_string(),
        "#x (unreadable)".to_string(),
    ];
    assert_eq!(
        decide_leases(&found, "12"),
        Item::warn(
            "lease(s) nobody has renewed for over 12h: #2 (renewed 13h ago), #11 (unreadable), #x (unreadable)",
            "maintainer lease release 2 11 "
        )
    );
    assert_eq!(decide_leases(&[], "12"), Item::ok("no lease has gone 12h without a renewal"));
}

#[test]
fn only_pr_dot_json_files_in_leases_are_read_and_a_crash_warns_and_still_prints_the_ok_line() {
    let rig = audit_rig(&[("DOCTOR_LEASE_HOURS", "abc")]);
    rig.host.file(&format!("{ST}/leases/pr-2.json"), lease(13, 20).as_bytes(), false, 0);
    let (o, _) = run_audit(&rig);
    assert!(has_warn(
        &o.report,
        &format!("could not read the leases under {ST}/leases, so forgotten ones are unknown"),
        "python3 and a readable state directory"
    ));
    assert!(has_ok(&o.report, "no lease has gone abch without a renewal"));
}

#[test]
fn two_forgotten_leases_are_listed_in_file_name_order_which_puts_pr_11_before_pr_2() {
    let rig = audit_rig(&[]);
    rig.host.file(&format!("{ST}/leases/pr-2.json"), lease(13, 20).as_bytes(), false, 0);
    rig.host.file(&format!("{ST}/leases/pr-11.json"), lease(40, 20).as_bytes(), false, 0);
    let (o, _) = run_audit(&rig);
    assert!(has_warn(
        &o.report,
        "lease(s) nobody has renewed for over 12h: #11 (renewed 40h ago), #2 (renewed 13h ago)",
        "maintainer lease release 11 2 "
    ));
}

#[test]
fn leases_other_than_pr_files_are_ignored() {
    let rig = audit_rig(&[]);
    rig.host.file(&format!("{ST}/leases/pr-2.json"), lease(13, 20).as_bytes(), false, 0);
    rig.host.file(&format!("{ST}/leases/notes.json"), b"{}", false, 0);
    rig.host.file(&format!("{ST}/leases/pr-x.txt"), b"{}", false, 0);
    rig.host.file(&format!("{ST}/leases/.pr-h.json"), b"{}", false, 0);
    let (o, _) = run_audit(&rig);
    assert!(has_warn(
        &o.report,
        "lease(s) nobody has renewed for over 12h: #2 (renewed 13h ago)",
        "maintainer lease release 2 "
    ));
}

// --- the pinned token (decision F4) ---------------------------------------------------------

const TOKEN: &str = "ghp_FAKEFAKEFAKEFAKEFAKE0000";

#[test]
fn a_token_in_a_scanned_directory_is_a_failure_naming_the_file_and_the_token_is_not_printed() {
    let rig = audit_rig(&[]);
    rig.host.file(&format!("{ST}/logs/a.log"), format!("GH_TOKEN={TOKEN}\n").as_bytes(), false, 0);
    let (o, _) = run_audit(&rig);
    assert!(has_fail(
        &o.report,
        &format!("the pinned token is on disk in 1 file(s), starting {ST}/logs/a.log"),
        &format!("rotate the token, then: {SHARE}/run.sh magent <task> rescrubs on exit")
    ));
    assert!(!dump(&o.report).contains(TOKEN));
}

#[test]
fn the_token_never_appears_in_the_arguments_or_the_environment_of_any_child() {
    let rig = audit_rig(&[]);
    rig.proc.allow_unscripted();
    rig.host.file(&format!("{ST}/logs/a.log"), TOKEN.as_bytes(), false, 0);
    let _ = run_audit(&rig);
    for spec in rig.proc.specs() {
        let shown = format!("{:?} {:?}", spec.argv.as_slice(), spec.env);
        assert!(!shown.contains(TOKEN), "{shown}");
    }
    assert!(!rig.proc.tool_calls().iter().any(|call| call.starts_with("grep")));
}

#[test]
fn the_scan_covers_each_of_the_five_subdirectories_and_the_claude_session_directory() {
    for sub in ["logs", "runs", "drafts", "state", "receipts"] {
        let rig = audit_rig(&[]);
        // A second scanned directory keeps the whole-state-directory fallback out of play.
        rig.host.dir(&format!("{ST}/logs"));
        rig.host.file(&format!("{ST}/{sub}/a"), TOKEN.as_bytes(), false, 0);
        let (o, _) = run_audit(&rig);
        assert!(
            o.report
                .items()
                .iter()
                .any(|i| matches!(i, Item::Fail(t, _) if t.contains(&format!("{ST}/{sub}/a")))),
            "{sub}"
        );
    }
    let rig = audit_rig(&[]);
    rig.host.file(&format!("{HOME}/.claude/projects/-r/t.jsonl"), TOKEN.as_bytes(), false, 0);
    let (o, _) = run_audit(&rig);
    assert!(
        o.report.items().iter().any(|i| matches!(i, Item::Fail(t, _) if t.contains("-r/t.jsonl")))
    );
}

#[test]
fn a_token_in_the_state_directory_top_level_is_missed_once_a_subdirectory_exists() {
    let rig = audit_rig(&[]);
    rig.host.dir(&format!("{ST}/logs"));
    rig.host.file(&format!("{ST}/stray.txt"), TOKEN.as_bytes(), false, 0);
    let (o, _) = run_audit(&rig);
    assert!(has_ok(&o.report, "no pinned token left on disk by an earlier run"));
}

#[test]
fn with_none_of_the_subdirectories_the_whole_state_directory_is_searched() {
    let rig = audit_rig(&[]);
    rig.host.file(&format!("{ST}/stray.txt"), TOKEN.as_bytes(), false, 0);
    let (o, _) = run_audit(&rig);
    assert!(
        o.report
            .items()
            .iter()
            .any(|i| matches!(i, Item::Fail(t, _) if t.ends_with("starting /state/stray.txt")))
    );
}

#[test]
fn seven_files_holding_the_token_are_reported_as_five() {
    let rig = audit_rig(&[]);
    for i in 1..=7 {
        rig.host.file(&format!("{ST}/logs/f{i}.log"), TOKEN.as_bytes(), false, 0);
    }
    let (o, _) = run_audit(&rig);
    assert!(o.report.items().iter().any(|i| matches!(i, Item::Fail(t, _) if t.starts_with("the pinned token is on disk in 5 file(s)"))));
}

#[test]
fn an_answer_of_fifteen_characters_is_too_short_and_sixteen_is_scanned() {
    let rig = audit_rig(&[]);
    rig.proc.on(&["gh", "auth", "token", "--user", "testuser"], 0, "shorttoken12345\n");
    let (o, _) = run_audit(&rig);
    assert!(has_warn(
        &o.report,
        "cannot read the pinned token, so nothing checked whether one was left on disk",
        "gh auth switch --user testuser"
    ));
    let rig = audit_rig(&[]);
    rig.proc.on(&["gh", "auth", "token", "--user", "testuser"], 0, "abcdefghijklmnop\n");
    rig.host.file(&format!("{ST}/logs/a.log"), b"abcdefghijklmnop", false, 0);
    let (o, _) = run_audit(&rig);
    assert!(
        o.report
            .items()
            .iter()
            .any(|i| matches!(i, Item::Fail(t, _) if t.contains("pinned token is on disk")))
    );
}

#[test]
fn the_output_of_a_failing_token_command_is_scanned_as_if_it_were_the_token() {
    let rig = audit_rig(&[]);
    let text = "no oauth token found for account testuser";
    rig.proc.on(&["gh", "auth", "token", "--user", "testuser"], 1, &format!("{text}\n"));
    rig.host.file(&format!("{ST}/logs/a.log"), text.as_bytes(), false, 0);
    let (o, _) = run_audit(&rig);
    assert!(
        o.report
            .items()
            .iter()
            .any(|i| matches!(i, Item::Fail(t, _) if t.contains("pinned token is on disk")))
    );
}

#[test]
fn a_missing_gh_and_a_missing_account_name_read_as_a_token_that_cannot_be_read() {
    let rig = Rig::new(&[("STATE_DIR", ST), ("GH_ACCOUNT", "")]);
    rig.host.dir(ST);
    rig.proc.missing(&["gh", "auth", "token", "--user", ""]);
    let (o, _) = run_audit(&rig);
    assert!(has_warn(
        &o.report,
        "cannot read the pinned token, so nothing checked whether one was left on disk",
        "gh auth switch --user <account>"
    ));
}

#[test]
fn a_token_of_several_lines_matches_a_file_holding_any_one_line() {
    let rig = audit_rig(&[]);
    rig.proc.on(
        &["gh", "auth", "token", "--user", "testuser"],
        0,
        "first-line-of-the-token-0000\nsecond-line-aaaaaaaa\n",
    );
    rig.host.file(&format!("{ST}/logs/a.log"), b"xx second-line-aaaaaaaa yy", false, 0);
    let (o, _) = run_audit(&rig);
    assert!(
        o.report
            .items()
            .iter()
            .any(|i| matches!(i, Item::Fail(t, _) if t.contains("pinned token is on disk")))
    );
}

#[test]
fn a_binary_file_holding_the_token_is_found() {
    let rig = audit_rig(&[]);
    let mut bytes = vec![0u8, 1];
    bytes.extend_from_slice(TOKEN.as_bytes());
    bytes.push(0);
    rig.host.file(&format!("{ST}/logs/b.bin"), &bytes, false, 0);
    let (o, _) = run_audit(&rig);
    assert!(o.report.items().iter().any(|i| matches!(i, Item::Fail(t, _) if t.contains("b.bin"))));
}

// --- tasks ---------------------------------------------------------------------------------------

fn task_rig(pairs: &[(&str, &str)]) -> Rig {
    audit_rig(pairs)
}

fn age(rig: &Rig, name: &str, hours: i64, body: &str) {
    rig.host.file(
        &format!("{ST}/state/{name}"),
        body.as_bytes(),
        false,
        NOWI - hours * 3600 - 1200,
    );
}

#[test]
fn a_failure_with_no_success_on_record_has_never_completed_and_names_age_and_reason() {
    let rig = task_rig(&[]);
    age(&rig, "failed-review.json", 5, "{\"reason\": \"network down\"}");
    let (o, _) = run_audit(&rig);
    assert!(has_fail(
        &o.report,
        "review has never completed; its only record is a failure 5h ago: network down",
        &format!("{SHARE}/run.sh magent review")
    ));
    assert!(!has_ok(&o.report, "every task has completed since its last failure"));
}

#[test]
fn a_missing_garbage_or_null_reason_reads_as_no_reason_recorded() {
    for body in ["{}", "garbage", "{\"reason\": null}", "{\"reason\": \"\"}", "[1]"] {
        let rig = task_rig(&[]);
        age(&rig, "failed-review.json", 5, body);
        let (o, _) = run_audit(&rig);
        assert!(
            o.report.items().iter().any(|i| matches!(i, Item::Fail(t, _) if t.ends_with("failure 5h ago: no reason recorded in failed-review.json"))),
            "{body}"
        );
    }
}

#[test]
fn a_success_newer_than_the_failure_and_a_tie_both_count_as_recovered() {
    let rig = task_rig(&[]);
    age(&rig, "failed-review.json", 30, "{}");
    age(&rig, "last-review.json", 5, "{}");
    let (o, _) = run_audit(&rig);
    assert!(has_ok(&o.report, "every task has completed since its last failure"));

    let rig = task_rig(&[]);
    age(&rig, "failed-review.json", 5, "{}");
    age(&rig, "last-review.json", 5, "{}");
    let (o, _) = run_audit(&rig);
    assert!(has_ok(&o.report, "every task has completed since its last failure"));
}

#[test]
fn a_failure_after_a_success_younger_than_the_task_interval_is_a_warning_and_a_success_exactly_as_old_is_not_excused()
 {
    let rig = task_rig(&[("MIN_HOURS_review", "6")]);
    age(&rig, "last-review.json", 3, "{}");
    age(&rig, "failed-review.json", 1, "{\"reason\": \"x\"}");
    let (o, _) = run_audit(&rig);
    assert!(has_warn(
        &o.report,
        "review failed 1h ago, but it completed 3h ago and its interval is 6h: x",
        "nothing is overdue; read the log if you want to know why that attempt died"
    ));

    let rig = task_rig(&[("MIN_HOURS_review", "3")]);
    age(&rig, "last-review.json", 3, "{}");
    age(&rig, "failed-review.json", 1, "{\"reason\": \"x\"}");
    let (o, _) = run_audit(&rig);
    assert!(has_fail(
        &o.report,
        "review failed 1h ago and has not completed since: x",
        &format!("{SHARE}/run.sh magent review")
    ));
}

#[test]
fn an_interval_of_zero_or_text_never_excuses_a_failure_and_text_prints_the_test_error() {
    let rig = task_rig(&[("MIN_HOURS_review", "abc")]);
    age(&rig, "last-review.json", 40, "{}");
    age(&rig, "failed-review.json", 1, "{}");
    let (o, _) = run_audit(&rig);
    assert!(
        o.report
            .items()
            .iter()
            .any(|i| matches!(i, Item::Fail(t, _) if t.starts_with("review failed")))
    );
    assert_eq!(
        String::from_utf8_lossy(&o.stderr),
        "maintainer-doctor: line 531: [: abc: integer expected\n"
    );
}

#[test]
fn two_stopped_tasks_print_two_failures_in_file_name_order_and_no_every_task_line() {
    let rig = task_rig(&[]);
    age(&rig, "failed-issues.json", 7, "{}");
    age(&rig, "failed-alpha.json", 9, "{}");
    let (o, _) = run_audit(&rig);
    let fails: Vec<&Item> =
        o.report.items().iter().filter(|i| matches!(i, Item::Fail(..))).collect();
    assert_eq!(fails.len(), 2);
    assert!(matches!(fails[0], Item::Fail(t, _) if t.starts_with("alpha")));
}

#[test]
fn a_task_name_with_a_hyphen_and_a_success_on_record_aborts_the_rest_of_the_section() {
    let rig = task_rig(&[]);
    age(&rig, "failed-read-only.json", 1, "{}");
    age(&rig, "last-read-only.json", 40, "{}");
    age(&rig, "failed-zeta.json", 3, "{}");
    let (o, r) = run_audit(&rig);
    assert_eq!(r, Err(Stop::AbortSection));
    assert_eq!(
        String::from_utf8_lossy(&o.stderr),
        "maintainer-doctor: line 530: MIN_HOURS_read-only: invalid variable name\n"
    );
    assert!(!lines(&o.report).iter().any(|l| l.contains("zeta") || l.contains("every task")));
}

#[test]
fn the_failure_reason_follows_python_print_for_strings_numbers_and_containers() {
    let known = |text: &str| failure_reason(&Fact::Known(text.as_bytes().to_vec()));
    assert_eq!(known("{\"reason\": \"it broke\\n\\n\"}"), "it broke");
    assert_eq!(known("{\"reason\": 42}"), "42");
    assert_eq!(known("{\"reason\": [\"a\", {\"b\": true}]}"), "['a', {'b': True}]");
    assert_eq!(known("{\"reason\": 0}"), "");
    assert_eq!(failure_reason(&Fact::Unreadable("x".into())), "");
}

// --- deployed code ---------------------------------------------------------------------------------

fn facts(commit: &str, head: &str, dirty: usize) -> DeployFacts {
    DeployFacts {
        commit: commit.into(),
        from: "/src".into(),
        from_is_repo: true,
        head: head.into(),
        behind: "3".into(),
        dirty,
    }
}

#[test]
fn deployed_code_that_is_a_clean_head_is_ok_and_a_dirty_one_warns_that_neither_version_runs() {
    assert_eq!(
        decide_deployed(&facts("abc", "abc", 0)),
        Item::ok("deployed code is that tree's HEAD (abc)")
    );
    assert_eq!(
        decide_deployed(&facts("abc", "abc", 2)),
        Item::warn(
            "deployed matches HEAD, but /src has 2 uncommitted change(s)",
            "commit them, then ./install.sh, or the next run uses neither version"
        )
    );
}

#[test]
fn deployed_code_behind_its_source_fails_with_the_count_and_the_reinstall_command() {
    assert_eq!(
        decide_deployed(&facts("abc", "def", 0)),
        Item::fail(
            "deployed abc is 3 commit(s) behind /src (def); every timer is running old code",
            "cd /src && ./install.sh"
        )
    );
}

#[test]
fn a_stamp_with_a_missing_half_and_a_vanished_source_tree_each_warn() {
    let mut f = facts("", "", 0);
    assert!(
        matches!(decide_deployed(&f), Item::Warn(t, _) if t == "VERSION does not record what was installed or where from")
    );
    f = facts("abc", "abc", 0);
    f.from = String::new();
    assert!(matches!(decide_deployed(&f), Item::Warn(..)));
    f = facts("abc", "abc", 0);
    f.from_is_repo = false;
    assert_eq!(
        decide_deployed(&f),
        Item::warn(
            "the tree it was installed from is gone: /src",
            "reinstall from a checkout so drift can be measured"
        )
    );
}

#[test]
fn deployed_code_reads_version_then_asks_git_three_questions_about_the_source_tree() {
    let rig = Rig::new(&[]);
    rig.host.file(&format!("{SHARE}/VERSION"), b"commit=abc\ninstalled_from=/src\n", false, 0);
    rig.host.dir("/src/.git");
    rig.proc.on(&["git", "-C", "/src", "rev-parse", "--short", "HEAD"], 0, "abc\n");
    rig.proc.on(&["git", "-C", "/src", "rev-list", "--count", "abc..HEAD"], 0, "0\n");
    rig.proc.on(&["git", "-C", "/src", "status", "--porcelain"], 0, "");
    let mut o = out();
    audit::deployed_code(&rig.ctx(), &mut o);
    assert!(has_ok(&o.report, "deployed code is that tree's HEAD (abc)"));
    assert_eq!(rig.proc.tool_calls().len(), 3);
}

#[test]
fn a_commit_the_source_tree_has_never_seen_reads_as_a_question_mark_behind() {
    let rig = Rig::new(&[]);
    rig.host.file(&format!("{SHARE}/VERSION"), b"commit=0123abc\ninstalled_from=/src\n", false, 0);
    rig.host.dir("/src/.git");
    rig.proc.on(&["git", "-C", "/src", "rev-parse", "--short", "HEAD"], 0, "abc\n");
    rig.proc.on(&["git", "-C", "/src", "rev-list", "--count", "0123abc..HEAD"], 128, "");
    rig.proc.on(&["git", "-C", "/src", "status", "--porcelain"], 0, "");
    let mut o = out();
    audit::deployed_code(&rig.ctx(), &mut o);
    assert!(has_fail(
        &o.report,
        "deployed 0123abc is ? commit(s) behind /src (abc); every timer is running old code",
        "cd /src && ./install.sh"
    ));
}
