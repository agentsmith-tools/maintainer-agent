//! Behaviour of `merge`, `update`, `receipt` and `show`: each test changes one fact of the ready
//! world in `testkit` and names what the gate then does. A test named "preserved" pins a behaviour
//! of the bash gate that the port keeps.

use crate::testkit::{HEAD, NEW_HEAD, World, asserted, dependency, observed, pr};
use crate::{
    Action, Exit, GateDecision, GhCall, GhFailure, GitCall, HelperCall, HelperMissing,
    HoldersFailure, IssueNumber, LeaseAnswer, MergeState, Posting, Review, UpdateFacts,
    UpdateRefusal, WhoAmI,
};
use mx_ports::{Clock, Fs, Script};
use mx_suite::CoverageFailure;

fn is(exit: Result<(), Exit>, decision: &GateDecision) {
    assert_eq!(exit.unwrap_err(), Exit::from(decision));
}

fn fail(output: &str) -> GhFailure {
    GhFailure { exit: 1, output: output.to_string() }
}

fn issue(n: u64) -> IssueNumber {
    IssueNumber::new(n).unwrap()
}

// ----- the ready pull request ------------------------------------------------------------------

#[test]
fn a_ready_pull_request_merges_pinned_to_the_checked_head_and_spends_the_receipt() {
    let w = World::new();
    w.merge().unwrap();
    let merge = w.gh.calls().into_iter().find_map(|c| match c {
        GhCall::Merge { pr, head } => Some((pr.to_string(), head.to_string())),
        _ => None,
    });
    assert_eq!(merge, Some(("7".into(), HEAD.into())));
    assert_eq!(
        w.stdout(),
        "  #7: approved, 1 checks pass, 0 skipped, 0 fail, 0 pending, 0 cancelled, receipt valid\n  merged #7\n  #7 closes no issue, so nothing was closed by this merge.\n    If it resolves one, close it by hand and say which commit did it.\n"
    );
    assert_eq!(w.receipt_on_disk(7), None, "a spent receipt is removed");
}

#[test]
fn a_merge_that_closes_an_issue_does_not_print_the_closes_nothing_hint() {
    let w = World::new();
    w.gh.script_closing_issues(Script::always(Ok(vec![issue(3)])));
    w.gh.script_author(Script::always(Ok("alice".into())));
    w.merge().unwrap();
    assert!(!w.stdout().contains("closes no issue"), "{}", w.stdout());
    assert!(w.stdout().contains("  merged #7\n"));
}

#[test]
fn the_merge_asks_head_state_and_review_in_that_order_and_checks_after_the_claims() {
    let w = World::new();
    w.merge().unwrap();
    let names: Vec<String> =
        w.gh.calls()
            .iter()
            .map(|c| format!("{c:?}").split(['(', ' ']).next().unwrap().to_string())
            .collect();
    assert_eq!(names, ["Head", "MergeState", "ReviewDecision", "ClosingIssues", "Checks", "Merge"]);
}

#[test]
fn a_rehearsal_makes_every_check_and_never_calls_merge() {
    let mut w = World::new();
    w.cfg.posting = Posting::Off;
    w.merge().unwrap();
    assert_eq!(w.calls_of("Merge {"), 0);
    assert!(w.stdout().ends_with(
        "  POST=off: every condition holds and the merge is NOT being made\n           re-run with posting on to land it\n"
    ));
    assert!(w.receipt_on_disk(7).is_some(), "a rehearsal keeps the receipt");
}

#[test]
fn a_merge_that_gh_refuses_ends_with_the_bash_die_and_keeps_the_receipt() {
    let w = World::new();
    w.gh.script_merge(Script::always(Err(fail("gh: conflict"))));
    is(w.merge(), &GateDecision::MergeFailed { output: String::new() });
    assert!(w.receipt_on_disk(7).is_some());
    assert!(!w.stdout().contains("merged #7"));
}

// ----- the order of the first refusals ---------------------------------------------------------

#[test]
fn a_leased_pull_request_refuses_before_anything_else_is_read() {
    let w = World::new();
    w.helper.script_lease(Script::always(Ok(LeaseAnswer::Held { output: "held by x".into() })));
    is(
        w.merge(),
        &GateDecision::LeaseHeld { pr: pr("7"), action: Action::Merge, output: "held by x".into() },
    );
    assert!(w.identity.calls().is_empty() && w.gh.calls().is_empty());
}

#[test]
fn a_missing_helper_and_an_unreadable_lease_each_refuse_by_name() {
    let w = World::new();
    w.helper.script_lease(Script::always(Err(HelperMissing)));
    is(w.merge(), &GateDecision::LeaseHelperMissing { pr: pr("7"), action: Action::Merge });
    let w = World::new();
    w.helper.script_lease(Script::always(Ok(LeaseAnswer::Unreadable { output: "boom".into() })));
    is(
        w.merge(),
        &GateDecision::LeaseUnreadable {
            pr: pr("7"),
            action: Action::Merge,
            output: "boom".into(),
        },
    );
}

#[test]
fn a_profile_without_prod_globs_refuses_before_the_identity_is_asked() {
    let mut w = World::new();
    w.cfg.prod_globs = None;
    is(w.merge(), &GateDecision::NoProdGlobs);
    assert!(w.identity.calls().is_empty());
}

#[test]
fn another_login_and_a_failed_identity_read_refuse_before_the_lock_is_taken() {
    let w = World::new();
    w.identity.script(Script::always(WhoAmI::Login("mallory".into())));
    is(
        w.merge(),
        &GateDecision::IdentityMismatch { who: "mallory".into(), account: "testuser".into() },
    );
    assert!(w.locks.asked().is_empty());
    let w = World::new();
    w.identity.script(Script::always(WhoAmI::Failed { exit: 4 }));
    is(w.merge(), &GateDecision::IdentityUnconfirmed { account: "testuser".into(), exit: 4 });
}

#[test]
fn a_held_merge_lock_refuses_after_the_identity_and_before_any_pull_request_read() {
    let w = World::new();
    w.locks.hold(std::path::Path::new("/s/receipts/.merge-7.lock"));
    is(
        w.merge(),
        &GateDecision::MergeLockHeld { pr: pr("7"), lock: "/s/receipts/.merge-7.lock".into() },
    );
    assert_eq!(w.identity.calls().len(), 1);
    assert!(w.gh.calls().is_empty());
}

#[test]
fn an_unopenable_merge_lock_is_its_own_refusal() {
    let w = World::new();
    w.locks.make_unopenable(std::path::Path::new("/s/receipts/.merge-7.lock"));
    is(w.merge(), &GateDecision::MergeLockUnopenable { pr: pr("7") });
}

#[test]
fn the_merge_lock_is_released_when_the_merge_ends() {
    let w = World::new();
    w.merge().unwrap();
    assert!(w.locks.held().is_empty());
}

// ----- the receipt -----------------------------------------------------------------------------

#[test]
fn no_receipt_for_the_pull_request_refuses_with_the_command_to_earn_one() {
    let w = World::new();
    w.fs.remove(std::path::Path::new("/s/receipts/7.json")).unwrap();
    is(w.merge(), &GateDecision::NoReceipt { pr: pr("7") });
}

#[test]
fn a_receipt_that_is_not_json_is_refused_as_unreadable() {
    let w = World::new();
    w.fs.put("/s/receipts/7.json", b"not json".to_vec(), 0);
    is(w.merge(), &GateDecision::ReceiptNotJson { pr: pr("7") });
}

#[test]
fn a_receipt_without_a_signature_is_refused_before_the_key_is_read() {
    let w = World::new();
    w.fs.put("/s/receipts/7.json", br#"{"pr": 7, "head": "abc", "kind": "observed"}"#.to_vec(), 0);
    w.fs.make_unreadable("/k/receipt.key", "gone");
    is(w.merge(), &GateDecision::ReceiptUnsigned { pr: pr("7") });
}

#[test]
fn an_edited_receipt_fails_its_signature() {
    let w = World::new();
    let bytes = String::from_utf8(w.receipt_on_disk(7).unwrap())
        .unwrap()
        .replace("assertion failed", "edited");
    w.fs.put("/s/receipts/7.json", bytes.into_bytes(), 0);
    is(w.merge(), &GateDecision::ReceiptSignatureMismatch { pr: pr("7") });
}

#[test]
fn a_missing_key_prints_the_key_line_and_then_calls_the_receipt_unreadable_preserved() {
    let w = World::new();
    w.fs.remove(std::path::Path::new("/k/receipt.key")).unwrap();
    let exit = w.merge().unwrap_err();
    let key_line = Exit::from(&GateDecision::NoReceiptKey { path: "/k/receipt.key".into() });
    let tail = Exit::from(&GateDecision::ReceiptNotJson { pr: pr("7") });
    assert_eq!(exit.stderr, format!("{}{}", key_line.stderr, tail.stderr));
}

#[test]
fn a_receipt_copied_from_another_pull_request_is_refused_by_name() {
    let w = World::new();
    w.fs.put("/s/receipts/7.json", observed(8, HEAD, "rust", false).to_bytes(), 0);
    is(
        w.merge(),
        &GateDecision::ReceiptForOtherPr {
            path: "/s/receipts/7.json".into(),
            found: "8".into(),
            pr: pr("7"),
        },
    );
}

#[test]
fn an_unattended_run_may_not_spend_an_asserted_receipt_but_a_human_may() {
    let w = World::new();
    w.receipt(&asserted(7, HEAD));
    w.merge().unwrap();
    assert!(w.stdout().starts_with("  the receipt is ASSERTED, not observed: it records a claim a\n     human made, and nothing here checked it\n"));
    let mut w = World::new();
    w.receipt(&asserted(7, HEAD));
    w.cfg.in_run = true;
    is(w.merge(), &GateDecision::ReceiptNotObserved { pr: pr("7"), kind: "asserted".into() });
    w.cfg.in_run = true;
}

#[test]
fn a_dependency_receipt_is_spendable_in_a_run_and_says_so() {
    let mut w = World::new();
    w.receipt(&dependency(7, HEAD));
    w.cfg.in_run = true;
    w.merge().unwrap();
    assert!(
        w.stdout().starts_with(
            "  the receipt is a DEPENDENCY receipt: nothing but dependency metadata\n"
        )
    );
}

#[test]
fn a_whole_receipt_never_asks_which_files_changed() {
    let w = World::new();
    w.merge().unwrap();
    assert_eq!(w.calls_of("ChangedFiles"), 0);
}

#[test]
fn a_partial_receipt_is_checked_against_the_changed_files_and_the_selector_stderr_comes_first() {
    let w = World::new();
    w.receipt(&observed(7, HEAD, "rust", true));
    w.gh.script_changed_files(Script::always(Ok(vec!["bin/a".into(), "lib/b".into()])));
    w.selector.script_covers(Script::always(Err(CoverageFailure::Uncovered {
        paths: vec!["lib/b".into()],
    })));
    let exit = w.merge().unwrap_err();
    let die = Exit::from(&GateDecision::ReceiptDoesNotCover {
        pr: pr("7"),
        suites: vec!["rust".into()],
        verified_head: HEAD.into(),
    });
    let lines = CoverageFailure::Uncovered { paths: vec!["lib/b".into()] }.stderr_lines();
    assert_eq!(exit.stderr, format!("{lines}{}", die.stderr));
}

#[test]
fn a_partial_receipt_that_the_suites_cover_merges() {
    let w = World::new();
    w.receipt(&observed(7, HEAD, "rust", true));
    w.gh.script_changed_files(Script::always(Ok(vec!["bin/a".into()])));
    w.selector.script_covers(Script::always(Ok(())));
    w.merge().unwrap();
    assert_eq!(w.calls_of("ChangedFiles"), 1);
}

#[test]
fn an_unreadable_and_an_empty_changed_file_list_each_refuse_a_partial_receipt() {
    let w = World::new();
    w.receipt(&observed(7, HEAD, "rust", true));
    w.gh.script_changed_files(Script::always(Err(fail("502"))));
    is(w.merge(), &GateDecision::ChangedFilesUnreadable { pr: pr("7") });
    let w = World::new();
    w.receipt(&observed(7, HEAD, "rust", true));
    w.gh.script_changed_files(Script::always(Ok(Vec::new())));
    is(w.merge(), &GateDecision::ChangedFilesEmpty { pr: pr("7") });
}

// ----- the facts of the pull request -----------------------------------------------------------

#[test]
fn an_unreadable_head_and_a_head_that_is_not_a_commit_id_refuse_before_the_merge() {
    let w = World::new();
    w.gh.script_head(Script::always(Err(fail("x"))));
    is(w.merge(), &GateDecision::HeadUnreadable { pr: pr("7") });
    let w = World::new();
    w.gh.script_head(Script::always(Ok("NOTHEX".into())));
    is(w.merge(), &GateDecision::HeadNotACommit { pr: pr("7"), head: "NOTHEX".into() });
    let w = World::new();
    w.gh.script_head(Script::always(Ok(String::new())));
    is(w.merge(), &GateDecision::HeadNotACommit { pr: pr("7"), head: String::new() });
}

#[test]
fn only_the_exact_word_approved_lets_a_merge_through() {
    for review in ["REVIEW_REQUIRED", "CHANGES_REQUESTED", "approved", ""] {
        let w = World::new();
        w.gh.script_review_decision(Script::always(Ok(review.into())));
        is(w.merge(), &GateDecision::NotApproved { pr: pr("7"), review: review.into() });
    }
}

#[test]
fn a_review_read_that_failed_refuses_as_an_empty_decision_preserved() {
    let w = World::new();
    w.gh.script_review_decision(Script::always(Err(fail("x"))));
    is(w.merge(), &GateDecision::NotApproved { pr: pr("7"), review: String::new() });
}

#[test]
fn each_merge_state_refuses_with_its_own_message() {
    let cases: [(&str, GateDecision); 6] = [
        ("BEHIND", GateDecision::Behind { pr: pr("7") }),
        ("DIRTY", GateDecision::Dirty { pr: pr("7") }),
        ("BLOCKED", GateDecision::Blocked { pr: pr("7") }),
        ("UNSTABLE", GateDecision::Unstable { pr: pr("7") }),
        ("UNKNOWN", GateDecision::Unknown { pr: pr("7") }),
        ("DRAFT", GateDecision::MergeStateNotAllowed { pr: pr("7"), state: "DRAFT".into() }),
    ];
    for (state, want) in cases {
        let w = World::new();
        w.gh.script_merge_state(Script::always(Ok(state.into())));
        is(w.merge(), &want);
    }
    let w = World::new();
    w.gh.script_merge_state(Script::always(Ok("HAS_HOOKS".into())));
    w.merge().unwrap();
}

#[test]
fn an_unreadable_merge_state_refuses_as_an_empty_state_preserved() {
    let w = World::new();
    w.gh.script_merge_state(Script::always(Err(fail("x"))));
    is(w.merge(), &GateDecision::MergeStateNotAllowed { pr: pr("7"), state: String::new() });
}

#[test]
fn the_review_is_judged_before_the_claims_and_the_claims_before_the_state() {
    let w = World::new();
    w.gh.script_review_decision(Script::always(Ok("REVIEW_REQUIRED".into())));
    w.gh.script_merge_state(Script::always(Ok("DIRTY".into())));
    is(w.merge(), &GateDecision::NotApproved { pr: pr("7"), review: "REVIEW_REQUIRED".into() });
    let w = World::new();
    w.gh.script_merge_state(Script::always(Ok("DIRTY".into())));
    w.gh.script_closing_issues(Script::always(Err(fail("502"))));
    is(w.merge(), &GateDecision::ClosesUnreadable { pr: pr("7"), output: "502".into() });
}

// ----- the claim checks ------------------------------------------------------------------------

fn closing_world() -> World {
    let w = World::new();
    w.gh.script_closing_issues(Script::always(Ok(vec![issue(3)])));
    w.gh.script_author(Script::always(Ok("Alice".into())));
    w
}

#[test]
fn a_pull_request_that_closes_nothing_never_reads_its_author() {
    let w = World::new();
    w.merge().unwrap();
    assert_eq!(w.calls_of("Author"), 0);
    assert_eq!(w.helper.calls(), vec![HelperCall::LeaseCheck(pr("7"))], "only the lease was asked");
}

#[test]
fn an_unreadable_or_empty_author_refuses_when_an_issue_is_closed() {
    let w = closing_world();
    w.gh.script_author(Script::always(Err(fail("502"))));
    is(w.merge(), &GateDecision::AuthorUnreadable { pr: pr("7"), output: "502".into() });
    let w = closing_world();
    w.gh.script_author(Script::always(Ok(String::new())));
    is(w.merge(), &GateDecision::AuthorUnreadable { pr: pr("7"), output: String::new() });
}

#[test]
fn a_missing_helper_and_a_failing_helper_refuse_by_name_instead_of_reading_as_nobody() {
    let w = closing_world();
    w.helper.script_holders(Script::always(Err(HoldersFailure::HelperMissing)));
    is(w.merge(), &GateDecision::HoldersHelperMissing { pr: pr("7"), issue: issue(3) });
    let w = closing_world();
    w.helper.script_holders(Script::always(Err(HoldersFailure::Failed {
        output: "no profile".into(),
    })));
    is(
        w.merge(),
        &GateDecision::HoldersUnreadable {
            pr: pr("7"),
            issue: issue(3),
            output: "no profile".into(),
        },
    );
}

#[test]
fn an_issue_pointed_at_somebody_else_blocks_the_merge_and_the_author_among_them_does_not() {
    let w = closing_world();
    w.helper.script_holders(Script::always(Ok(vec!["bob".into(), "carol".into()])));
    is(
        w.merge(),
        &GateDecision::IssuePointedAtOthers {
            pr: pr("7"),
            issue: issue(3),
            holders: vec!["bob".into(), "carol".into()],
            author: "Alice".into(),
        },
    );
    let w = closing_world();
    w.helper.script_holders(Script::always(Ok(vec!["alice".into()])));
    w.merge().unwrap();
}

#[test]
fn a_holder_is_matched_whole_and_without_case_not_as_a_prefix() {
    let w = closing_world();
    w.helper.script_holders(Script::always(Ok(vec!["alice-b".into()])));
    assert!(w.merge().is_err(), "alice-b is not alice");
}

fn labelled_world() -> World {
    let mut w = closing_world();
    w.cfg.claim_label = "claimed".into();
    w.gh.script_issue_has_label(Script::always(Ok(true)));
    w.gh.script_issue_assignees(Script::always(Ok(Vec::new())));
    w.gh.script_issue_commenters(Script::always(Ok(Vec::new())));
    w
}

#[test]
fn a_project_without_a_claim_label_never_reads_labels() {
    let w = closing_world();
    w.merge().unwrap();
    assert_eq!(w.calls_of("IssueHasLabel"), 0);
}

#[test]
fn an_unreadable_label_list_refuses_and_an_absent_label_lets_the_issue_pass() {
    let w = labelled_world();
    w.gh.script_issue_has_label(Script::always(Err(fail("502"))));
    is(
        w.merge(),
        &GateDecision::LabelsUnreadable {
            pr: pr("7"),
            issue: issue(3),
            claim_label: "claimed".into(),
            output: "502".into(),
        },
    );
    let w = labelled_world();
    w.gh.script_issue_has_label(Script::always(Ok(false)));
    w.merge().unwrap();
    assert_eq!(w.calls_of("IssueAssignees"), 0);
}

#[test]
fn a_labelled_issue_assigned_to_somebody_else_blocks_and_one_assigned_to_the_author_passes() {
    let w = labelled_world();
    w.gh.script_issue_assignees(Script::always(Ok(vec!["bob".into()])));
    is(
        w.merge(),
        &GateDecision::ClaimedAssignedToOthers {
            pr: pr("7"),
            issue: issue(3),
            claim_label: "claimed".into(),
            assignees: vec!["bob".into()],
            author: "Alice".into(),
        },
    );
    let w = labelled_world();
    w.gh.script_issue_assignees(Script::always(Ok(vec!["ALICE".into()])));
    w.merge().unwrap();
    assert_eq!(w.calls_of("IssueCommenters"), 0, "an assignment ends the check");
}

#[test]
fn an_unreadable_assignee_list_and_an_unreadable_comment_list_each_refuse() {
    let w = labelled_world();
    w.gh.script_issue_assignees(Script::always(Err(fail("a"))));
    is(
        w.merge(),
        &GateDecision::AssigneesUnreadable { pr: pr("7"), issue: issue(3), output: "a".into() },
    );
    let w = labelled_world();
    w.gh.script_issue_commenters(Script::always(Err(fail("c"))));
    is(
        w.merge(),
        &GateDecision::CommentersUnreadable { pr: pr("7"), issue: issue(3), output: "c".into() },
    );
}

#[test]
fn a_second_voice_on_an_unassigned_labelled_issue_blocks_but_the_maintainer_and_the_author_do_not()
{
    let w = labelled_world();
    w.gh.script_issue_commenters(Script::always(Ok(vec![
        "alice".into(),
        "TESTUSER".into(),
        "bob".into(),
    ])));
    is(
        w.merge(),
        &GateDecision::ClaimedWithSecondVoice {
            pr: pr("7"),
            issue: issue(3),
            claim_label: "claimed".into(),
            others: vec!["bob".into()],
            author: "Alice".into(),
        },
    );
    let w = labelled_world();
    w.gh.script_issue_commenters(Script::always(Ok(vec!["alice".into(), "testuser".into()])));
    w.merge().unwrap();
}

#[test]
fn every_closed_issue_is_checked_not_only_the_first() {
    let w = labelled_world();
    w.gh.script_closing_issues(Script::always(Ok(vec![issue(3), issue(4)])));
    w.helper.script_holders(Script::sequence([Ok(Vec::new()), Ok(vec!["bob".into()])]));
    is(
        w.merge(),
        &GateDecision::IssuePointedAtOthers {
            pr: pr("7"),
            issue: issue(4),
            holders: vec!["bob".into()],
            author: "Alice".into(),
        },
    );
    assert_eq!(
        w.helper.calls(),
        vec![
            HelperCall::LeaseCheck(pr("7")),
            HelperCall::Holders(issue(3)),
            HelperCall::Holders(issue(4))
        ]
    );
}

// ----- the board -------------------------------------------------------------------------------

fn board(json: &str) -> World {
    let w = World::new();
    w.gh.script_checks(Script::always(Ok(json.into())));
    w
}

#[test]
fn an_empty_answer_from_gh_checks_and_a_failed_call_both_read_as_no_checks_preserved() {
    is(board("").merge(), &GateDecision::NoChecks { pr: pr("7") });
    let w = World::new();
    w.gh.script_checks(Script::always(Err(fail("x"))));
    is(w.merge(), &GateDecision::NoChecks { pr: pr("7") });
}

#[test]
fn a_board_that_is_not_json_refuses_with_the_message_written_for_it() {
    let exit = board("this is not json").merge().unwrap_err();
    assert_eq!(
        exit,
        Exit::from(&GateDecision::ChecksUnparsable {
            pr: pr("7"),
            head: "this is not json".into()
        })
    );
}

#[test]
fn the_board_refuses_a_failing_a_pending_a_cancelled_and_an_all_skipped_list() {
    let row = |b: &str| format!(r#"{{"name":"c","bucket":"{b}"}}"#);
    is(
        board(&format!("[{},{}]", row("pass"), row("fail"))).merge(),
        &GateDecision::FailingChecks { pr: pr("7"), count: 1 },
    );
    is(
        board(&format!("[{},{}]", row("pass"), row("pending"))).merge(),
        &GateDecision::PendingChecks { pr: pr("7"), count: 1 },
    );
    is(
        board(&format!("[{},{}]", row("pass"), row("cancel"))).merge(),
        &GateDecision::CancelledChecks { pr: pr("7"), count: 1 },
    );
    is(
        board(&format!("[{}]", row("skipping"))).merge(),
        &GateDecision::AllChecksSkipped { pr: pr("7"), total: 1 },
    );
    is(board("[]").merge(), &GateDecision::EmptyBoard { pr: pr("7") });
    is(
        board(&format!("[{},{}]", row("pass"), row("future"))).merge(),
        &GateDecision::ChecksInUnknownBucket { pr: pr("7"), accepted: 1, total: 2 },
    );
}

#[test]
fn passes_and_skips_are_counted_apart_on_the_ready_line() {
    let w = board(r#"[{"bucket":"pass"},{"bucket":"pass"},{"bucket":"skipping"}]"#);
    w.merge().unwrap();
    assert!(
        w.stdout().contains("#7: approved, 2 checks pass, 1 skipped, 0 fail"),
        "{}",
        w.stdout()
    );
}

// ----- staleness (F1 and the tree-merge comparison) --------------------------------------------

fn moved() -> World {
    let w = World::new();
    w.gh.script_head(Script::always(Ok(NEW_HEAD.into())));
    w.git.script_rev_parse(Script::always(Ok(NEW_HEAD.into())));
    w.git.script_merge_base(Script::always(Some("base".into())));
    w.git.script_is_ancestor(Script::always(false));
    w.git.script_merge_tree(Script::always(Some("mergedtree".into())));
    w.git.script_diff_names(Script::always(Ok(String::new())));
    w
}

fn diffs(w: &World) -> Vec<(String, String, Vec<String>)> {
    w.git
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            GitCall::DiffNames { from, to, pathspecs, .. } => Some((from, to, pathspecs)),
            _ => None,
        })
        .collect()
}

#[test]
fn an_unmoved_head_runs_no_diff_and_removes_its_temporary_branch() {
    let w = World::new();
    w.merge().unwrap();
    assert!(diffs(&w).is_empty());
    assert_eq!(
        w.git.calls().last(),
        Some(&GitCall::DeleteBranch { repo: "/repo".into(), name: "mm-7-1".into() })
    );
    assert!(!w.stdout().contains("head moved"));
}

#[test]
fn a_failed_fetch_of_the_pull_request_refuses_by_name() {
    let w = World::new();
    w.git.script_fetch_ref(Script::always(Err(mx_ports::GitError {
        command: "fetch".into(),
        reason: "x".into(),
    })));
    is(w.merge(), &GateDecision::CannotFetch { pr: pr("7") });
}

#[test]
fn a_moved_head_compares_the_clean_merge_tree_with_the_pull_request_over_the_production_pathspecs()
{
    let w = moved();
    w.merge().unwrap();
    assert_eq!(
        diffs(&w),
        vec![(
            "mergedtree".to_string(),
            NEW_HEAD.to_string(),
            vec!["bin/*".to_string(), "lib/*".to_string()]
        )]
    );
    assert!(w.stdout().contains(&format!("head moved {HEAD} -> fedcba98, the tree is the clean merge of the verified head with main; receipt still applies")));
    assert!(w.git.calls().contains(&GitCall::MergeTree {
        repo: "/repo".into(),
        base: "base".into(),
        head: HEAD.into()
    }));
}

#[test]
fn a_moved_head_falls_back_to_the_strict_range_when_the_merge_does_not_come_out_clean() {
    let w = moved();
    w.git.script_merge_tree(Script::always(None));
    w.merge().unwrap();
    assert_eq!(diffs(&w)[0].0, format!("{HEAD}..{NEW_HEAD}"));
    assert_eq!(diffs(&w)[0].1, "");
    assert!(w.stdout().contains(", no production diff; receipt still applies"));
}

#[test]
fn a_verified_head_that_main_already_contains_is_compared_by_the_strict_range() {
    let w = moved();
    w.git.script_is_ancestor(Script::always(true));
    w.merge().unwrap();
    assert_eq!(diffs(&w)[0].0, format!("{HEAD}..{NEW_HEAD}"));
    assert_eq!(w.git.calls().iter().filter(|c| matches!(c, GitCall::MergeTree { .. })).count(), 0);
}

#[test]
fn a_branch_with_no_merge_base_is_compared_by_the_strict_range() {
    let w = moved();
    w.git.script_merge_base(Script::always(None));
    w.merge().unwrap();
    assert_eq!(diffs(&w)[0].0, format!("{HEAD}..{NEW_HEAD}"));
}

#[test]
fn a_production_change_since_the_verified_head_refuses_and_names_the_paths_and_removes_the_branch()
{
    let w = moved();
    w.git.script_diff_names(Script::always(Ok("bin/x\nlib/y\n".into())));
    is(
        w.merge(),
        &GateDecision::ProductionChanged {
            verified_head: HEAD.into(),
            paths: vec!["bin/x".into(), "lib/y".into()],
        },
    );
    assert_eq!(
        w.git.calls().last(),
        Some(&GitCall::DeleteBranch { repo: "/repo".into(), name: "mm-7-1".into() })
    );
    assert_eq!(w.calls_of("Merge {"), 0);
}

#[test]
fn a_failing_git_diff_refuses_instead_of_reading_as_no_production_change_f1() {
    let w = moved();
    w.git.script_diff_names(Script::always(Err(crate::DiffFailure {
        reason: "fatal: bad revision".into(),
    })));
    is(
        w.merge(),
        &GateDecision::StalenessUnreadable {
            pr: pr("7"),
            verified_head: HEAD.into(),
            reason: "fatal: bad revision".into(),
        },
    );
    assert_eq!(w.calls_of("Merge {"), 0);
    assert_eq!(
        w.git.calls().last(),
        Some(&GitCall::DeleteBranch { repo: "/repo".into(), name: "mm-7-1".into() })
    );
}

#[test]
fn a_failing_strict_range_diff_refuses_too_f1() {
    let w = moved();
    w.git.script_merge_tree(Script::always(None));
    w.git.script_diff_names(Script::always(Err(crate::DiffFailure { reason: "x".into() })));
    is(
        w.merge(),
        &GateDecision::StalenessUnreadable {
            pr: pr("7"),
            verified_head: HEAD.into(),
            reason: "x".into(),
        },
    );
    assert_eq!(w.calls_of("Merge {"), 0);
}

#[test]
fn a_receipt_at_a_short_head_never_equals_the_full_head_so_the_diff_runs_preserved() {
    let w = World::new();
    w.receipt(&observed(7, &HEAD[..7], "rust", false));
    w.git.script_merge_base(Script::always(None));
    w.git.script_diff_names(Script::always(Ok(String::new())));
    w.merge().unwrap();
    assert_eq!(diffs(&w).len(), 1);
}

#[test]
fn the_pull_request_ref_is_fetched_into_mm_n() {
    let w = World::new();
    w.merge().unwrap();
    assert!(w.git.calls().contains(&GitCall::FetchRef {
        repo: "/repo".into(),
        refspec: "refs/pull/7/head:mm-7-1".into()
    }));
}

// ----- update ----------------------------------------------------------------------------------

fn facts(head: &str, state: MergeState, decision: &str) -> UpdateFacts {
    UpdateFacts {
        head: head.into(),
        state,
        decision: decision.into(),
        author: "alice".into(),
        cross_repository: false,
        maintainer_can_modify: false,
    }
}

fn review(user: &str, state: &str, commit: &str) -> Review {
    Review { user: user.into(), state: state.into(), commit_id: Some(commit.into()) }
}

fn behind() -> World {
    let w = World::new();
    w.gh.script_update_facts(Script::sequence([
        Ok(facts(HEAD, MergeState::Behind, "APPROVED")),
        Ok(facts(NEW_HEAD, MergeState::Clean, "APPROVED")),
    ]));
    w.gh.script_reviews(Script::always(Ok(vec![review("testuser", "APPROVED", HEAD)])));
    w.gh.script_update_branch(Script::always(Ok(())));
    w
}

fn refused(w: &World, r: &UpdateRefusal) {
    assert_eq!(w.update().unwrap_err(), Exit::from(r));
}

#[test]
fn a_behind_pull_request_with_a_receipt_and_an_approval_is_updated_at_the_expected_head() {
    let w = behind();
    w.update().unwrap();
    let put = w.gh.calls().into_iter().find_map(|c| match c {
        GhCall::UpdateBranch { pr, expected_head } => {
            Some((pr.to_string(), expected_head.to_string()))
        }
        _ => None,
    });
    assert_eq!(put, Some(("7".into(), HEAD.into())));
    let out = w.stdout();
    assert!(
        out.contains(
            "  #7: BEHIND, approved by testuser at 01234567, receipt valid, no other lease\n"
        ),
        "{out}"
    );
    assert!(
        out.contains("  #7 updated: new head fedcba98 (was 01234567), mergeStateStatus CLEAN\n"),
        "{out}"
    );
    assert!(out.contains("  reviewDecision is still APPROVED after the update\n"), "{out}");
}

#[test]
fn an_update_that_dismissed_the_approval_says_what_to_do_in_four_lines() {
    let w = World::new();
    w.gh.script_update_facts(Script::sequence([
        Ok(facts(HEAD, MergeState::Behind, "APPROVED")),
        Ok(facts(NEW_HEAD, MergeState::Clean, "REVIEW_REQUIRED")),
    ]));
    w.gh.script_reviews(Script::always(Ok(vec![review("testuser", "APPROVED", HEAD)])));
    w.gh.script_update_branch(Script::always(Ok(())));
    w.update().unwrap();
    let out = w.stdout();
    assert!(out.contains(
        "  reviewDecision is now 'REVIEW_REQUIRED': the update dismissed the approval.\n"
    ));
    assert!(out.contains("  The review must be re-posted at fedcba98."));
    assert!(out.contains(
        "  of the verified head 01234567 against fedcba98 shows no production or workflow change;\n"
    ));
    assert!(out.ends_with("  this command approves nothing. Read the diff, then decide.\n"));
}

#[test]
fn a_clean_pull_request_has_nothing_to_update_and_asks_nothing_more() {
    for state in [MergeState::Clean, MergeState::HasHooks] {
        let w = World::new();
        let shown = state.as_str().to_string();
        w.gh.script_update_facts(Script::always(Ok(facts(HEAD, state, "APPROVED"))));
        w.update().unwrap();
        assert_eq!(w.stdout(), format!("  #7 is {shown}: nothing to update\n"));
        assert_eq!(w.calls_of("Reviews"), 0);
    }
}

#[test]
fn conflicts_and_every_other_state_refuse_an_update_by_name() {
    let w = World::new();
    w.gh.script_update_facts(Script::always(Ok(facts(HEAD, MergeState::Dirty, "APPROVED"))));
    refused(&w, &UpdateRefusal::Conflicts { pr: pr("7") });
    for state in [MergeState::Blocked, MergeState::Unstable, MergeState::Unknown] {
        let w = World::new();
        let shown = state.as_str().to_string();
        w.gh.script_update_facts(Script::always(Ok(facts(HEAD, state, "APPROVED"))));
        refused(&w, &UpdateRefusal::StateNotBehind { pr: pr("7"), state: shown });
    }
}

#[test]
fn an_unreadable_fact_set_and_a_short_head_refuse_before_the_branch_is_touched() {
    let w = World::new();
    w.gh.script_update_facts(Script::always(Err(fail("x"))));
    refused(&w, &UpdateRefusal::FactUnreadable { k: "head".into(), pr: pr("7") });
    let w = World::new();
    w.gh.script_update_facts(Script::always(Ok(facts("abc1234", MergeState::Behind, "APPROVED"))));
    refused(&w, &UpdateRefusal::HeadNotFull { pr: pr("7"), head: "abc1234".into() });
    assert_eq!(w.calls_of("UpdateBranch"), 0);
}

#[test]
fn dependabots_pull_requests_are_refused_in_all_four_spellings_and_any_case() {
    for author in [
        "app/dependabot",
        "dependabot",
        "dependabot[bot]",
        "app/dependabot[bot]",
        "Dependabot[BOT]",
    ] {
        let w = World::new();
        let mut f = facts(HEAD, MergeState::Behind, "APPROVED");
        f.author = author.into();
        w.gh.script_update_facts(Script::always(Ok(f)));
        refused(&w, &UpdateRefusal::Dependabots { pr: pr("7") });
    }
}

#[test]
fn an_update_needs_the_overall_decision_and_this_accounts_own_standing_approval_of_this_head() {
    let w = World::new();
    w.gh.script_update_facts(Script::always(Ok(facts(
        HEAD,
        MergeState::Behind,
        "REVIEW_REQUIRED",
    ))));
    refused(
        &w,
        &UpdateRefusal::NotApprovedForUpdate { pr: pr("7"), decision: "REVIEW_REQUIRED".into() },
    );
    let no_standing = |reviews: Vec<Review>| {
        let w = World::new();
        w.gh.script_update_facts(Script::always(Ok(facts(HEAD, MergeState::Behind, "APPROVED"))));
        w.gh.script_reviews(Script::always(Ok(reviews)));
        refused(
            &w,
            &UpdateRefusal::NoStandingApproval {
                account: "testuser".into(),
                pr: pr("7"),
                head_8: "01234567".into(),
            },
        );
    };
    no_standing(vec![review("testuser", "APPROVED", NEW_HEAD)]);
    no_standing(vec![review("somebody", "APPROVED", HEAD)]);
    no_standing(vec![
        review("testuser", "APPROVED", HEAD),
        review("testuser", "CHANGES_REQUESTED", HEAD),
    ]);
    no_standing(vec![review("testuser", "APPROVED", HEAD), review("testuser", "DISMISSED", HEAD)]);
    no_standing(Vec::new());
}

#[test]
fn a_later_approval_after_a_change_request_counts_and_a_comment_does_not_clear_one() {
    let w = behind();
    w.gh.script_reviews(Script::always(Ok(vec![
        review("TestUser", "CHANGES_REQUESTED", HEAD),
        review("testuser", "APPROVED", HEAD),
        review("testuser", "COMMENTED", NEW_HEAD),
    ])));
    w.update().unwrap();
}

#[test]
fn unreadable_reviews_refuse_by_name() {
    let w = World::new();
    w.gh.script_update_facts(Script::always(Ok(facts(HEAD, MergeState::Behind, "APPROVED"))));
    w.gh.script_reviews(Script::always(Err(fail("x"))));
    refused(
        &w,
        &UpdateRefusal::ReviewsUnreadable {
            pr: pr("7"),
            account: "testuser".into(),
            head_8: "01234567".into(),
        },
    );
}

#[test]
fn an_update_applies_the_receipt_checks_of_merge_before_it_touches_the_branch() {
    let w = behind();
    w.fs.remove(std::path::Path::new("/s/receipts/7.json")).unwrap();
    assert_eq!(w.update().unwrap_err(), Exit::from(&GateDecision::NoReceipt { pr: pr("7") }));
    assert_eq!(w.calls_of("UpdateBranch"), 0);
}

#[test]
fn a_fork_without_maintainer_edits_exits_3_and_one_with_edits_is_updated() {
    let w = World::new();
    let mut f = facts(HEAD, MergeState::Behind, "APPROVED");
    f.cross_repository = true;
    w.gh.script_update_facts(Script::always(Ok(f)));
    w.gh.script_reviews(Script::always(Ok(vec![review("testuser", "APPROVED", HEAD)])));
    let exit = w.update().unwrap_err();
    assert_eq!(exit.code, 3);
    assert_eq!(exit, Exit::from(&UpdateRefusal::NeedsAuthorNoEdits { pr: pr("7") }));
    assert_eq!(w.calls_of("UpdateBranch"), 0);
    let w = World::new();
    let mut f = facts(HEAD, MergeState::Behind, "APPROVED");
    f.cross_repository = true;
    f.maintainer_can_modify = true;
    w.gh.script_update_facts(Script::sequence([
        Ok(f),
        Ok(facts(NEW_HEAD, MergeState::Clean, "APPROVED")),
    ]));
    w.gh.script_reviews(Script::always(Ok(vec![review("testuser", "APPROVED", HEAD)])));
    w.gh.script_update_branch(Script::always(Ok(())));
    w.update().unwrap();
}

#[test]
fn a_rehearsal_names_the_put_it_would_run_and_makes_none() {
    let mut w = behind();
    w.cfg.posting = Posting::Off;
    w.update().unwrap();
    assert_eq!(w.calls_of("UpdateBranch"), 0);
    assert!(w.stdout().ends_with(&format!(
        "  POST=off: every condition holds and the branch is NOT being updated\n           it would run: gh api -X PUT repos/o/r/pulls/7/update-branch -f expected_head_sha={HEAD}\n"
    )), "{}", w.stdout());
}

#[test]
fn a_put_that_github_refuses_is_told_apart_as_moved_head_author_job_or_refused() {
    let moved = |msg: &str| {
        let w = behind();
        w.gh.script_update_branch(Script::always(Err(fail(msg))));
        w.update().unwrap_err()
    };
    let put = "gh: expected head sha didn't match".to_string();
    assert_eq!(moved(&put), Exit::from(&UpdateRefusal::HeadMoved { pr: pr("7"), put }));
    let put = "gh: Resource not accessible (HTTP 403)".to_string();
    let exit = moved(&put);
    assert_eq!(exit.code, 3);
    assert_eq!(exit, Exit::from(&UpdateRefusal::NeedsAuthorNoPush { pr: pr("7"), put }));
    let put = "you do not have permission to update this branch".to_string();
    assert_eq!(moved(&put).code, 3);
    let put = "HTTP 500".to_string();
    let exit = moved(&put);
    assert_eq!(exit.code, 1);
    assert_eq!(exit, Exit::from(&UpdateRefusal::GithubRefused { pr: pr("7"), put }));
}

#[test]
fn the_head_that_moved_wins_over_a_403_in_the_same_message() {
    let w = behind();
    w.gh.script_update_branch(Script::always(Err(fail("expected head sha mismatch (HTTP 403)"))));
    assert_eq!(w.update().unwrap_err().code, 1);
}

#[test]
fn the_readback_waits_between_reads_and_stops_at_the_first_confirmation() {
    let w = World::new();
    w.gh.script_update_facts(Script::sequence([
        Ok(facts(HEAD, MergeState::Behind, "APPROVED")),
        Ok(facts(HEAD, MergeState::Behind, "APPROVED")),
        Ok(facts(NEW_HEAD, MergeState::Unknown, "APPROVED")),
        Ok(facts(NEW_HEAD, MergeState::Clean, "APPROVED")),
    ]));
    w.gh.script_reviews(Script::always(Ok(vec![review("testuser", "APPROVED", HEAD)])));
    w.gh.script_update_branch(Script::always(Ok(())));
    let mut cfg = w.cfg.clone();
    cfg.readback_reads = Some("3".into());
    cfg.readback_wait = Some("2".into());
    let gate = crate::Gate::new(&cfg, w.ports());
    gate.update(&pr("7")).unwrap();
    assert_eq!(w.calls_of("UpdateFacts"), 4, "one read before the PUT and three after");
    assert!(w.stdout().contains("mergeStateStatus CLEAN"));
}

#[test]
fn a_readback_that_never_shows_the_move_is_not_confirmed_and_names_what_it_saw() {
    let w = World::new();
    w.gh.script_update_facts(Script::sequence([
        Ok(facts(HEAD, MergeState::Behind, "APPROVED")),
        Ok(facts(HEAD, MergeState::Behind, "APPROVED")),
        Ok(facts(HEAD, MergeState::Behind, "APPROVED")),
        Ok(facts(HEAD, MergeState::Behind, "APPROVED")),
    ]));
    w.gh.script_reviews(Script::always(Ok(vec![review("testuser", "APPROVED", HEAD)])));
    w.gh.script_update_branch(Script::always(Ok(())));
    refused(
        &w,
        &UpdateRefusal::NotConfirmed {
            pr: pr("7"),
            reads: "3".into(),
            nhead_8: "01234567".into(),
            nstate: "BEHIND".into(),
        },
    );
}

#[test]
fn an_unreadable_readback_reports_the_state_as_unread() {
    let w = World::new();
    w.gh.script_update_facts(Script::sequence([
        Ok(facts(HEAD, MergeState::Behind, "APPROVED")),
        Err(fail("x")),
        Err(fail("x")),
        Err(fail("x")),
    ]));
    w.gh.script_reviews(Script::always(Ok(vec![review("testuser", "APPROVED", HEAD)])));
    w.gh.script_update_branch(Script::always(Ok(())));
    refused(
        &w,
        &UpdateRefusal::NotConfirmed {
            pr: pr("7"),
            reads: "3".into(),
            nhead_8: String::new(),
            nstate: "unread".into(),
        },
    );
}

#[test]
fn an_update_with_no_readback_settings_refuses_after_the_put_naming_the_setting() {
    let mut w = behind();
    w.cfg.readback_reads = None;
    let exit = w.update().unwrap_err();
    assert!(
        exit.stderr.contains("UPDATE_READBACK_READS: lib/defaults.env was not loaded"),
        "{exit:?}"
    );
}

#[test]
fn an_update_takes_the_lease_the_account_and_prod_globs_first() {
    let mut w = behind();
    w.helper.script_lease(Script::always(Ok(LeaseAnswer::Held { output: "x".into() })));
    assert_eq!(
        w.update().unwrap_err(),
        Exit::from(&GateDecision::LeaseHeld {
            pr: pr("7"),
            action: Action::Update,
            output: "x".into()
        })
    );
    w.helper.script_lease(Script::always(Ok(LeaseAnswer::Free)));
    w.cfg.prod_globs = None;
    assert_eq!(w.update().unwrap_err(), Exit::from(&GateDecision::NoProdGlobs));
    assert_eq!(w.calls_of("UpdateFacts"), 0);
}

// ----- receipt and show ------------------------------------------------------------------------

#[test]
fn a_recorded_receipt_is_signed_asserted_and_checks_out_for_merge() {
    let w = World::new();
    w.gate()
        .receipt(&pr("9"), &crate::HeadArg::parse(HEAD).unwrap(), "I mutated it by hand")
        .unwrap();
    assert_eq!(
        w.stdout(),
        format!("  receipt recorded for #9 at {HEAD} (asserted, not observed)\n")
    );
    let bytes = w.receipt_on_disk(9).unwrap();
    let stored = mx_receipt::Stored::parse(&bytes).unwrap();
    assert_eq!(stored.field_text("kind", ""), "asserted");
    assert_eq!(stored.field_text("recorded_by", ""), "tester");
    stored.check(&crate::testkit::key(), "9", false).unwrap();
}

#[test]
fn an_unattended_run_may_not_record_a_receipt_and_nothing_is_written() {
    let mut w = World::new();
    w.cfg.in_run = true;
    let exit = w.gate().receipt(&pr("9"), &crate::HeadArg::parse(HEAD).unwrap(), "p").unwrap_err();
    assert!(
        exit.stderr.starts_with("maintainer-merge: an unattended run may not assert a receipt.\n"),
        "{exit:?}"
    );
    assert_eq!(w.receipt_on_disk(9), None);
}

#[test]
fn recording_without_a_key_refuses_and_leaves_nothing_on_disk_decision_p3b_8() {
    let w = World::new();
    w.fs.remove(std::path::Path::new("/k/receipt.key")).unwrap();
    let exit = w.gate().receipt(&pr("9"), &crate::HeadArg::parse(HEAD).unwrap(), "p").unwrap_err();
    assert_eq!(exit, Exit::from(&GateDecision::NoReceiptKey { path: "/k/receipt.key".into() }));
    assert_eq!(w.receipt_on_disk(9), None);
}

#[test]
fn a_second_receipt_for_the_same_pull_request_replaces_the_first_preserved() {
    let w = World::new();
    let head = crate::HeadArg::parse(HEAD).unwrap();
    w.gate().receipt(&pr("9"), &head, "first").unwrap();
    w.gate().receipt(&pr("9"), &head, "second").unwrap();
    assert!(String::from_utf8(w.receipt_on_disk(9).unwrap()).unwrap().contains("second"));
}

#[test]
fn show_of_one_pull_request_prints_the_file_bytes_and_a_missing_one_refuses() {
    let w = World::new();
    let printed = w.gate().show(Some(&pr("7"))).unwrap();
    assert_eq!(printed.stdout, w.receipt_on_disk(7).unwrap());
    let exit = w.gate().show(Some(&pr("99"))).unwrap_err();
    assert_eq!(
        exit.stderr, "maintainer-merge: no receipt for #99\n",
        "show says this, not the merge text"
    );
}

#[test]
fn show_lists_each_receipt_as_pr_head_and_proof_and_says_so_when_there_are_none() {
    let w = World::new();
    let printed = w.gate().show(None).unwrap();
    let text = String::from_utf8(printed.stdout).unwrap();
    assert!(text.starts_with("  7: 01234567 `guard` passes clean"), "{text}");
    let empty = World::new();
    empty.fs.remove(std::path::Path::new("/s/receipts/7.json")).unwrap();
    let printed = empty.gate().show(None).unwrap();
    assert!(printed.stdout.is_empty());
    assert_eq!(empty.stdout(), "  no receipts recorded\n");
}

#[test]
fn a_dir_the_gate_needs_is_made_before_it_is_read() {
    let w = World::new();
    w.gate().show(None).unwrap();
    assert_eq!(w.dirs.made.lock().unwrap().as_slice(), [std::path::PathBuf::from("/s/receipts")]);
}

#[test]
fn an_update_names_the_first_fact_the_helper_could_not_read() {
    for (key, want) in
        [("decision", "decision"), ("state", "state"), ("author", "author"), ("boom", "head")]
    {
        let w = World::new();
        w.gh.script_update_facts(Script::always(Err(fail(key))));
        refused(&w, &UpdateRefusal::FactUnreadable { k: want.into(), pr: pr("7") });
    }
}

#[test]
fn a_moved_head_refreshes_main_before_it_asks_for_the_merge_base() {
    let w = moved();
    w.merge().unwrap();
    let calls = w.git.calls();
    let fetch =
        calls.iter().position(|c| matches!(c, GitCall::FetchMain { .. })).expect("fetch of main");
    let base = calls.iter().position(|c| matches!(c, GitCall::MergeBase { .. })).unwrap();
    assert!(fetch < base);
}

#[test]
fn an_approval_by_the_account_in_another_case_counts_as_this_accounts_own() {
    let w = behind();
    w.gh.script_reviews(Script::always(Ok(vec![review("TESTUSER", "APPROVED", HEAD)])));
    w.update().unwrap();
}

#[test]
fn the_readback_sleeps_the_configured_wait_between_reads_only() {
    let w = World::new();
    w.gh.script_update_facts(Script::sequence([
        Ok(facts(HEAD, MergeState::Behind, "APPROVED")),
        Ok(facts(HEAD, MergeState::Behind, "APPROVED")),
        Ok(facts(HEAD, MergeState::Behind, "APPROVED")),
        Ok(facts(HEAD, MergeState::Behind, "APPROVED")),
    ]));
    w.gh.script_reviews(Script::always(Ok(vec![review("testuser", "APPROVED", HEAD)])));
    w.gh.script_update_branch(Script::always(Ok(())));
    let mut cfg = w.cfg.clone();
    cfg.readback_wait = Some("2".into());
    assert!(crate::Gate::new(&cfg, w.ports()).update(&pr("7")).is_err());
    assert_eq!(w.clock.now_unix(), 4, "two sleeps of 2 s between three reads");
}

#[test]
fn a_readback_that_shows_the_same_head_is_not_confirmed_even_when_the_state_is_clean() {
    let w = World::new();
    w.gh.script_update_facts(Script::sequence([
        Ok(facts(HEAD, MergeState::Behind, "APPROVED")),
        Ok(facts(HEAD, MergeState::Clean, "APPROVED")),
        Ok(facts(HEAD, MergeState::Clean, "APPROVED")),
        Ok(facts(HEAD, MergeState::Clean, "APPROVED")),
    ]));
    w.gh.script_reviews(Script::always(Ok(vec![review("testuser", "APPROVED", HEAD)])));
    w.gh.script_update_branch(Script::always(Ok(())));
    refused(
        &w,
        &UpdateRefusal::NotConfirmed {
            pr: pr("7"),
            reads: "3".into(),
            nhead_8: "01234567".into(),
            nstate: "CLEAN".into(),
        },
    );
}

#[test]
fn a_ref_that_moved_between_the_head_read_and_the_fetch_refuses_and_diffs_nothing_p3b_11() {
    let w = moved();
    w.git.script_rev_parse(Script::always(Ok("a".repeat(40))));
    let exit = w.merge().unwrap_err();
    assert!(exit.stderr.contains("the head of #7 moved while I was reading it"), "{exit:?}");
    assert!(exit.stderr.contains(&"a".repeat(40)));
    assert!(diffs(&w).is_empty(), "no diff of a commit that is not the head");
    assert_eq!(w.calls_of("Merge {"), 0);
    assert_eq!(
        w.git.calls().last(),
        Some(&GitCall::DeleteBranch { repo: "/repo".into(), name: "mm-7-1".into() })
    );
}

#[test]
fn an_unreadable_fetched_ref_refuses_too_and_the_branch_is_still_removed() {
    let w = World::new();
    w.git.script_rev_parse(Script::always(Err(mx_ports::GitError {
        command: "rev-parse".into(),
        reason: "bad".into(),
    })));
    let exit = w.merge().unwrap_err();
    assert!(exit.stderr.contains("unreadable"), "{exit:?}");
    assert_eq!(w.calls_of("Merge {"), 0);
}

#[test]
fn the_staleness_diff_names_commits_and_the_ref_is_private_to_the_run() {
    let w = moved();
    w.merge().unwrap();
    for (_, to, _) in diffs(&w) {
        assert_eq!(to, NEW_HEAD, "a commit id, not a movable name");
    }
    assert!(
        w.git
            .calls()
            .iter()
            .all(|c| !matches!(c, GitCall::MergeBase { a, .. } if a.starts_with("mm-")))
    );
    assert!(w.git.calls().contains(&GitCall::FetchRef {
        repo: "/repo".into(),
        refspec: "refs/pull/7/head:mm-7-1".into()
    }));
}

#[test]
fn the_merge_is_pinned_to_the_head_that_was_checked_not_to_the_receipt_head() {
    let w = moved();
    w.merge().unwrap();
    let pinned = w.gh.calls().into_iter().find_map(|c| match c {
        GhCall::Merge { head, .. } => Some(head.to_string()),
        _ => None,
    });
    assert_eq!(pinned, Some(NEW_HEAD.to_string()));
}

// ----- a build with no suite host yet (the p3a lane) ----------------------------------------------

fn without_a_suite_host() -> World {
    let mut w = World::new();
    w.cfg.has_suite_host = false;
    w
}

#[test]
fn a_build_without_a_suite_host_says_so_and_names_no_command_that_refuses_too_f4() {
    let w = without_a_suite_host();
    w.gh.script_changed_files(Script::always(Ok(vec!["bin/a".into()])));
    let exit = w
        .gate()
        .verify(
            false,
            &pr("7"),
            &crate::HeadArg::parse("0123456").unwrap(),
            &mx_suite::TestFilter::parse("guard").unwrap(),
            &mx_sandbox::SedExpression::parse("s/a/b/").unwrap(),
            None,
        )
        .unwrap_err();
    assert_eq!(exit, Exit::die(1, crate::NO_SUITE_HOST));
    for word in ["bash", "verify-part", "maintainer-merge", "proves less"] {
        assert!(!crate::NO_SUITE_HOST.contains(word), "{word}");
    }
    assert!(w.sandbox.calls().is_empty() && w.git.calls().is_empty(), "nothing ran");
}

#[test]
fn a_partial_receipt_cannot_be_checked_without_a_suite_host_and_the_text_says_that() {
    let w = without_a_suite_host();
    w.receipt(&observed(7, HEAD, "rust", true));
    w.gh.script_changed_files(Script::always(Ok(vec!["bin/a".into()])));
    assert_eq!(w.merge().unwrap_err(), Exit::die(1, crate::NO_SUITE_HOST));
    assert_eq!(w.calls_of("Merge {"), 0);
}

#[test]
fn a_whole_receipt_merges_without_a_suite_host() {
    let w = without_a_suite_host();
    w.merge().unwrap();
}

#[test]
fn a_verify_without_a_suite_host_still_reads_the_changed_files_first_as_the_bash_cases_pin() {
    let w = without_a_suite_host();
    w.gh.script_changed_files(Script::always(Ok(Vec::new())));
    let exit = w
        .gate()
        .verify(
            false,
            &pr("7"),
            &crate::HeadArg::parse("0123456").unwrap(),
            &mx_suite::TestFilter::parse("guard").unwrap(),
            &mx_sandbox::SedExpression::parse("s/a/b/").unwrap(),
            None,
        )
        .unwrap_err();
    assert_eq!(exit, Exit::from(&crate::VerifyRefusal::ChangedFilesUnreadable { pr: pr("7") }));
}
