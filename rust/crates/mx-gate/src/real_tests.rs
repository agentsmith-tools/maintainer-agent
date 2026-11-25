//! The real adapters against a scripted `Proc`: the argv of each `gh` and `git` call is the one
//! the bash spells, a failure reaches the caller as a failure, and the credentials go only to the
//! programs that need them.

use crate::real::{FileLocks, RealGateGit, RealGh, RealHelper, RealIdentity, Runner};
use crate::{
    Account, CommitId, GateGit, GhPort, Helper, HelperMissing, HoldersFailure, IdentityPort,
    IssueNumber, LeaseAnswer, LockError, LockPort, MergeState, PrNumber, WhoAmI,
};
use mx_ports::{Effect, EffectLog, FakeProc, ProcEffect, ProcError, Script};
use std::collections::BTreeMap;
use std::path::Path;

fn pr() -> PrNumber {
    PrNumber::parse("7").unwrap()
}

fn ok(stdout: &str) -> Script<Result<mx_ports::ProcOutput, ProcError>> {
    Script::always(Ok(FakeProc::exited(0, stdout)))
}

fn exit(code: i32, stdout: &str) -> Script<Result<mx_ports::ProcOutput, ProcError>> {
    Script::always(Ok(FakeProc::exited(code, stdout)))
}

fn vars() -> BTreeMap<String, String> {
    BTreeMap::new()
}

fn argv_of(log: &EffectLog) -> Vec<Vec<String>> {
    log.entries()
        .into_iter()
        .filter_map(|e| match e {
            Effect::Proc(ProcEffect::Run(spec)) => Some(spec.argv.as_slice().to_vec()),
            _ => None,
        })
        .collect()
}

#[test]
#[allow(clippy::too_many_lines, reason = "one table of the nine reads")]
fn each_gh_read_is_spelled_as_the_bash_spells_it() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    for (args, answer) in [
        (
            &[
                "gh",
                "pr",
                "view",
                "7",
                "--repo",
                "o/r",
                "--json",
                "headRefOid",
                "--jq",
                ".headRefOid",
            ][..],
            "abc",
        ),
        (
            &[
                "gh",
                "pr",
                "view",
                "7",
                "--repo",
                "o/r",
                "--json",
                "mergeStateStatus",
                "--jq",
                ".mergeStateStatus",
            ],
            "CLEAN",
        ),
        (
            &[
                "gh",
                "pr",
                "view",
                "7",
                "--repo",
                "o/r",
                "--json",
                "reviewDecision",
                "--jq",
                ".reviewDecision",
            ],
            "APPROVED",
        ),
        (
            &[
                "gh",
                "pr",
                "view",
                "7",
                "--repo",
                "o/r",
                "--json",
                "author",
                "--jq",
                ".author.login",
            ],
            "alice",
        ),
        (
            &[
                "gh",
                "pr",
                "view",
                "7",
                "--repo",
                "o/r",
                "--json",
                "closingIssuesReferences",
                "--jq",
                ".closingIssuesReferences[].number",
            ],
            "3\n4\n",
        ),
        (
            &["gh", "pr", "view", "7", "--repo", "o/r", "--json", "files", "--jq", ".files[].path"],
            "bin/a\nlib/b\n",
        ),
        (
            &[
                "gh",
                "issue",
                "view",
                "3",
                "--repo",
                "o/r",
                "--json",
                "labels",
                "--jq",
                "[.labels[].name]|index(\"claimed\")",
            ],
            "2",
        ),
        (
            &[
                "gh",
                "issue",
                "view",
                "3",
                "--repo",
                "o/r",
                "--json",
                "assignees",
                "--jq",
                ".assignees[].login",
            ],
            "bob\n",
        ),
        (
            &[
                "gh",
                "issue",
                "view",
                "3",
                "--repo",
                "o/r",
                "--json",
                "comments",
                "--jq",
                "[.comments[].author.login]|unique|.[]",
            ],
            "bob\ncarol\n",
        ),
    ] {
        proc.script(args, ok(answer));
    }
    let runner = Runner::new(&proc, &vars());
    let gh = RealGh::new(&runner, "o/r");
    assert_eq!(gh.head(&pr()).unwrap(), "abc");
    assert_eq!(gh.merge_state(&pr()).unwrap(), "CLEAN");
    assert_eq!(gh.review_decision(&pr()).unwrap(), "APPROVED");
    assert_eq!(gh.author(&pr()).unwrap(), "alice");
    assert_eq!(
        gh.closing_issues(&pr()).unwrap(),
        vec![IssueNumber::new(3).unwrap(), IssueNumber::new(4).unwrap()]
    );
    assert_eq!(gh.changed_files(&pr()).unwrap(), vec!["bin/a", "lib/b"]);
    let three = IssueNumber::new(3).unwrap();
    assert!(gh.issue_has_label(three, "claimed").unwrap());
    assert_eq!(gh.issue_assignees(three).unwrap(), vec!["bob"]);
    assert_eq!(gh.issue_commenters(three).unwrap(), vec!["bob", "carol"]);
    assert_eq!(argv_of(&log).len(), 9);
}

#[test]
fn an_absent_label_reads_as_empty_or_null_and_is_not_a_failure() {
    for printed in ["", "null"] {
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        proc.script(
            &[
                "gh",
                "issue",
                "view",
                "3",
                "--repo",
                "o/r",
                "--json",
                "labels",
                "--jq",
                "[.labels[].name]|index(\"claimed\")",
            ],
            ok(printed),
        );
        let runner = Runner::new(&proc, &vars());
        assert!(
            !RealGh::new(&runner, "o/r")
                .issue_has_label(IssueNumber::new(3).unwrap(), "claimed")
                .unwrap()
        );
    }
}

#[test]
fn a_label_that_would_end_the_jq_string_is_refused_as_unreadable() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    let runner = Runner::new(&proc, &vars());
    let err = RealGh::new(&runner, "o/r")
        .issue_has_label(IssueNumber::new(3).unwrap(), "a\") | $ENV | (\"")
        .unwrap_err();
    assert!(err.output.contains("cannot be quoted"), "{err}");
    assert!(argv_of(&log).is_empty(), "gh was never run");
}

#[test]
fn gh_pr_checks_exit_8_with_a_board_is_a_board_preserved() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    proc.script(
        &["gh", "pr", "checks", "7", "--repo", "o/r", "--json", "name,bucket"],
        exit(8, "[{\"bucket\":\"pending\"}]"),
    );
    let runner = Runner::new(&proc, &vars());
    assert_eq!(RealGh::new(&runner, "o/r").checks(&pr()).unwrap(), "[{\"bucket\":\"pending\"}]");
}

#[test]
fn a_failing_read_is_a_failure_carrying_gh_exit_code() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    proc.script(
        &["gh", "pr", "view", "7", "--repo", "o/r", "--json", "headRefOid", "--jq", ".headRefOid"],
        exit(4, ""),
    );
    let runner = Runner::new(&proc, &vars());
    assert_eq!(RealGh::new(&runner, "o/r").head(&pr()).unwrap_err().exit, 4);
}

#[test]
fn the_update_facts_are_read_from_one_json_call_and_a_null_decision_is_unreadable() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    let args = [
        "gh",
        "pr",
        "view",
        "7",
        "--repo",
        "o/r",
        "--json",
        "headRefOid,mergeStateStatus,reviewDecision,author,isCrossRepository,maintainerCanModify",
    ];
    proc.script(
        &args,
        Script::sequence([
            Ok(FakeProc::exited(0, r#"{"headRefOid":"abc","mergeStateStatus":"BEHIND","reviewDecision":"APPROVED","author":{"login":"alice"},"isCrossRepository":true,"maintainerCanModify":true}"#)),
            Ok(FakeProc::exited(0, r#"{"headRefOid":"abc","mergeStateStatus":"BEHIND","reviewDecision":null,"author":null,"isCrossRepository":false,"maintainerCanModify":false}"#)),
            Ok(FakeProc::exited(0, r#"{"headRefOid":"abc","mergeStateStatus":"BEHIND","reviewDecision":"","author":null,"isCrossRepository":false,"maintainerCanModify":"yes"}"#)),
        ]),
    );
    let runner = Runner::new(&proc, &vars());
    let gh = RealGh::new(&runner, "o/r");
    let f = gh.update_facts(&pr()).unwrap();
    assert_eq!(
        (f.head.as_str(), f.state.clone(), f.decision.as_str(), f.author.as_str()),
        ("abc", MergeState::Behind, "APPROVED", "alice")
    );
    assert!(f.cross_repository && f.maintainer_can_modify);
    assert_eq!(
        gh.update_facts(&pr()).unwrap_err().output,
        "decision",
        "a null reviewDecision exits 1 in the bash helper after head and state"
    );
    let f = gh.update_facts(&pr()).unwrap();
    assert!(!f.maintainer_can_modify, "only the JSON true is permission");
    assert_eq!(f.author, "");
}

#[test]
fn the_reviews_are_paged_and_read_line_by_line_and_a_bad_line_is_unreadable() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    proc.script(
        &["gh", "api", "--paginate", "repos/o/r/pulls/7/reviews", "--jq", ".[]"],
        Script::sequence([
            Ok(FakeProc::exited(0, "{\"user\":{\"login\":\"me\"},\"state\":\"APPROVED\",\"commit_id\":\"abc\"}\n\n{\"user\":null,\"state\":\"COMMENTED\"}\n")),
            Ok(FakeProc::exited(0, "not json\n")),
        ]),
    );
    let runner = Runner::new(&proc, &vars());
    let gh = RealGh::new(&runner, "o/r");
    let reviews = gh.reviews(&pr()).unwrap();
    assert_eq!(reviews.len(), 2);
    assert_eq!((reviews[0].user.as_str(), reviews[0].commit_id.as_deref()), ("me", Some("abc")));
    assert_eq!((reviews[1].user.as_str(), reviews[1].commit_id.clone()), ("", None));
    assert!(gh.reviews(&pr()).is_err());
}

#[test]
fn the_update_and_the_merge_use_the_bash_argv() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    let head = CommitId::parse("0123456789abcdef0123456789abcdef01234567").unwrap();
    proc.script(
        &[
            "gh",
            "api",
            "-X",
            "PUT",
            "repos/o/r/pulls/7/update-branch",
            "-f",
            "expected_head_sha=0123456789abcdef0123456789abcdef01234567",
        ],
        ok("{}"),
    );
    proc.script(
        &[
            "gh",
            "pr",
            "merge",
            "7",
            "--repo",
            "o/r",
            "--squash",
            "--delete-branch",
            "--match-head-commit",
            "0123456789abcdef0123456789abcdef01234567",
        ],
        ok(""),
    );
    let runner = Runner::new(&proc, &vars());
    let gh = RealGh::new(&runner, "o/r");
    gh.update_branch(&pr(), &head).unwrap();
    gh.merge(&pr(), &head, crate::SpendToken::new()).unwrap();
    assert_eq!(argv_of(&log).len(), 2);
}

#[test]
fn the_identity_switches_the_account_then_asks_who_gh_is() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    proc.script(
        &["gh", "auth", "switch", "--hostname", "github.com", "--user", "testuser"],
        exit(1, ""),
    );
    proc.script(&["gh", "api", "user", "--jq", ".login"], ok("testuser\n"));
    let runner = Runner::new(&proc, &vars());
    let account = Account::new("testuser").unwrap();
    assert_eq!(RealIdentity::new(&runner).who_am_i(&account), WhoAmI::Login("testuser".into()));
    assert_eq!(argv_of(&log)[0][1], "auth", "the switch comes first and its failure is ignored");
}

#[test]
fn a_failed_gh_api_user_is_a_failed_identity_with_the_exit_code() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    proc.script(&["gh", "auth", "switch", "--hostname", "github.com", "--user", "u"], ok(""));
    proc.script(
        &["gh", "api", "user", "--jq", ".login"],
        exit(4, "{\"message\":\"Bad credentials\"}"),
    );
    let runner = Runner::new(&proc, &vars());
    assert_eq!(
        RealIdentity::new(&runner).who_am_i(&Account::new("u").unwrap()),
        WhoAmI::Failed { exit: 4 }
    );
}

#[test]
fn git_calls_name_the_repository_and_a_failed_diff_is_an_error_not_an_empty_list() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    proc.script(
        &["git", "-C", "/repo", "diff", "--name-only", "tree", "mm-7", "--", "bin/*", "lib/*"],
        exit(128, ""),
    );
    proc.script(
        &["git", "-C", "/repo", "diff", "--name-only", "abc..mm-7", "--", "bin/*"],
        ok("bin/a\n"),
    );
    let runner = Runner::new(&proc, &vars());
    let git = RealGateGit::new(&runner);
    let specs = vec!["bin/*".to_string(), "lib/*".to_string()];
    let err = git.diff_names(Path::new("/repo"), "tree", "mm-7", &specs).unwrap_err();
    assert!(err.reason.starts_with("git exited 128"), "{err:?}");
    assert_eq!(git.diff_names(Path::new("/repo"), "abc..mm-7", "", &specs[..1]).unwrap(), "bin/a");
}

#[test]
fn the_git_helpers_answer_from_exit_codes_and_first_lines() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    proc.script(&["git", "-C", "/repo", "merge-base", "mm-7", "origin/main"], ok("base\n"));
    proc.script(&["git", "-C", "/repo", "merge-base", "--is-ancestor", "a", "b"], exit(1, ""));
    proc.script(
        &["git", "-C", "/repo", "merge-tree", "--write-tree", "base", "v"],
        exit(1, "conflict"),
    );
    proc.script(
        &["git", "-C", "/repo", "fetch", "-q", "origin", "refs/pull/7/head:mm-7", "--force"],
        exit(128, ""),
    );
    let runner = Runner::new(&proc, &vars());
    let git = RealGateGit::new(&runner);
    let repo = Path::new("/repo");
    assert_eq!(git.merge_base(repo, "mm-7", "origin/main"), Some("base".into()));
    assert!(!git.is_ancestor(repo, "a", "b"));
    assert_eq!(git.merge_tree(repo, "base", "v"), None, "a conflict is no tree");
    assert!(git.fetch_ref(repo, "refs/pull/7/head:mm-7").is_err());
}

#[test]
fn credentials_reach_gh_git_and_the_helper_and_no_other_program() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    proc.script(&["gh", "x"], ok(""));
    proc.script(&["date", "+%z"], ok("+0000\n"));
    let mut v = vars();
    v.insert("GH_TOKEN".into(), "secret-value".into());
    let runner = Runner::new(&proc, &v);
    drop(runner.run("gh", &["x"], std::time::Duration::from_secs(1), &[]));
    drop(runner.run("date", &["+%z"], std::time::Duration::from_secs(1), &[]));
    let envs: Vec<(String, bool)> = log
        .entries()
        .into_iter()
        .filter_map(|e| match e {
            Effect::Proc(ProcEffect::Run(spec)) => {
                Some((spec.argv.first().clone(), spec.env.iter().any(|(n, _)| n == "GH_TOKEN")))
            }
            _ => None,
        })
        .collect();
    assert_eq!(envs, vec![("gh".to_string(), true), ("date".to_string(), false)]);
}

#[test]
fn the_helper_answers_free_held_and_unreadable_from_its_exit_code() {
    for (code, want) in [
        (0, LeaseAnswer::Free),
        (1, LeaseAnswer::Held { output: "held".into() }),
        (2, LeaseAnswer::Unreadable { output: "held".into() }),
    ] {
        let dir = tempdir();
        let helper_path = dir.join("maintainer");
        std::fs::write(&helper_path, "#!/bin/sh\n").unwrap();
        std::os::unix::fs::PermissionsExt::set_mode(
            &mut std::fs::metadata(&helper_path).unwrap().permissions(),
            0o755,
        );
        std::fs::set_permissions(&helper_path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        let program = helper_path.to_str().unwrap();
        proc.script(&[program, "lease", "check", "7"], exit(code, "held\n"));
        let runner = Runner::new(&proc, &vars());
        let helper = RealHelper::new(
            &runner,
            &dir,
            "/nonexistent",
            vec![("MAINTAINER_SLUG".into(), "o/r".into())],
        );
        assert_eq!(helper.lease_check(&pr()).unwrap(), want, "exit {code}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn a_missing_helper_is_missing_for_the_lease_and_for_the_holders() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    let runner = Runner::new(&proc, &vars());
    let helper = RealHelper::new(&runner, Path::new("/nonexistent"), "/nonexistent", Vec::new());
    assert_eq!(helper.lease_check(&pr()), Err(HelperMissing));
    assert_eq!(helper.holders(IssueNumber::new(3).unwrap()), Err(HoldersFailure::HelperMissing));
}

fn tempdir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mxgate-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_second_lock_on_a_file_is_held_until_the_first_guard_drops_and_a_directory_is_unopenable() {
    let dir = tempdir();
    let file = dir.join(".merge-7.lock");
    let first = FileLocks.try_lock(&file).unwrap();
    assert_eq!(FileLocks.try_lock(&file).unwrap_err(), LockError::Held);
    drop(first);
    assert!(FileLocks.try_lock(&file).is_ok());
    assert_eq!(FileLocks.try_lock(&dir).unwrap_err(), LockError::Unopenable);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_failed_merge_base_is_no_base_even_when_it_printed_something() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    proc.script(&["git", "-C", "/repo", "merge-base", "a", "b"], exit(1, "partial\n"));
    let runner = Runner::new(&proc, &vars());
    assert_eq!(RealGateGit::new(&runner).merge_base(Path::new("/repo"), "a", "b"), None);
}

#[test]
fn a_gh_that_cannot_start_is_a_failed_checks_read_not_an_empty_board() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    proc.script(
        &["gh", "pr", "checks", "7", "--repo", "o/r", "--json", "name,bucket"],
        Script::always(Err(ProcError::NotFound { program: "gh".into() })),
    );
    let runner = Runner::new(&proc, &vars());
    assert_eq!(RealGh::new(&runner, "o/r").checks(&pr()).unwrap_err().exit, 127);
}

fn facts_of(json: &str) -> Result<crate::UpdateFacts, crate::GhFailure> {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    proc.script(
        &[
            "gh",
            "pr",
            "view",
            "7",
            "--repo",
            "o/r",
            "--json",
            "headRefOid,mergeStateStatus,reviewDecision,author,isCrossRepository,maintainerCanModify",
        ],
        ok(json),
    );
    let runner = Runner::new(&proc, &vars());
    RealGh::new(&runner, "o/r").update_facts(&pr())
}

const FACTS_OK: &str =
    r#""headRefOid":"abc","mergeStateStatus":"BEHIND","reviewDecision":"APPROVED""#;

#[test]
fn update_facts_without_is_cross_repository_are_unreadable_not_same_repository() {
    let json = format!(r#"{{{FACTS_OK},"author":{{"login":"a"}},"maintainerCanModify":true}}"#);
    assert_eq!(facts_of(&json).unwrap_err().output, "head");
}

#[test]
fn update_facts_whose_is_cross_repository_is_neither_a_bool_nor_a_number_are_unreadable() {
    for bad in [r#""false""#, "null", "[]", "{}"] {
        let json = format!(
            r#"{{{FACTS_OK},"author":{{"login":"a"}},"isCrossRepository":{bad},"maintainerCanModify":true}}"#
        );
        assert_eq!(facts_of(&json).unwrap_err().output, "head", "{bad}");
    }
    let json = format!(
        r#"{{{FACTS_OK},"author":{{"login":"a"}},"isCrossRepository":1,"maintainerCanModify":false}}"#
    );
    assert!(facts_of(&json).unwrap().cross_repository, "a number is truthy as in python");
}

#[test]
fn update_facts_whose_author_is_a_string_or_a_list_are_unreadable_so_dependabot_cannot_hide() {
    for bad in [r#""dependabot""#, "[]", "1", "true"] {
        let json = format!(
            r#"{{{FACTS_OK},"author":{bad},"isCrossRepository":false,"maintainerCanModify":false}}"#
        );
        assert_eq!(facts_of(&json).unwrap_err().output, "head", "{bad}");
    }
}

#[test]
fn update_facts_accept_a_null_author_and_an_author_object_without_a_login() {
    for author in ["null", "{}"] {
        let json = format!(
            r#"{{{FACTS_OK},"author":{author},"isCrossRepository":false,"maintainerCanModify":false}}"#
        );
        assert_eq!(facts_of(&json).unwrap().author, "");
    }
}

#[test]
fn a_gh_that_ran_past_its_timeout_is_exit_124_and_a_failed_read_never_a_success() {
    let log = EffectLog::new();
    let proc = FakeProc::new(&log);
    proc.script(
        &["gh", "pr", "view", "7", "--repo", "o/r", "--json", "headRefOid", "--jq", ".headRefOid"],
        Script::always(Ok(mx_ports::ProcOutput {
            exit: mx_ports::ProcExit::TimedOut,
            stdout: b"abc".to_vec(),
            stderr: Vec::new(),
        })),
    );
    let runner = Runner::new(&proc, &vars());
    let failure = RealGh::new(&runner, "o/r").head(&pr()).unwrap_err();
    assert_eq!(failure.exit, 124);
}

#[test]
fn an_author_login_with_a_newline_makes_the_update_facts_unreadable_by_name() {
    let json = format!(
        r#"{{{FACTS_OK},"author":{{"login":"a\nb"}},"isCrossRepository":false,"maintainerCanModify":false}}"#
    );
    assert_eq!(facts_of(&json).unwrap_err().output, "author");
}

#[test]
fn a_claim_label_with_a_backslash_a_quote_or_a_newline_never_reaches_gh() {
    for label in ["a\\b", "a\"b", "a\nb"] {
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        let runner = Runner::new(&proc, &vars());
        let failure = RealGh::new(&runner, "o/r")
            .issue_has_label(IssueNumber::new(3).unwrap(), label)
            .unwrap_err();
        assert!(failure.output.contains("cannot be quoted"), "{label:?}");
        assert!(argv_of(&log).is_empty());
    }
}

#[test]
fn a_truthy_login_that_is_not_text_is_an_unreadable_author_so_dependabot_cannot_hide_n1() {
    for bad in ["5", "true", "[1]", r#"{"a":1}"#, "1.5"] {
        let json = format!(
            r#"{{{FACTS_OK},"author":{{"login":{bad}}},"isCrossRepository":false,"maintainerCanModify":false}}"#
        );
        assert_eq!(facts_of(&json).unwrap_err().output, "author", "{bad}");
    }
}

#[test]
fn a_falsy_login_reads_as_empty_as_python_or_does_n1() {
    for empty in ["null", "0", "0.0", "false", r#""""#, "{}", "[]"] {
        let json = format!(
            r#"{{{FACTS_OK},"author":{{"login":{empty}}},"isCrossRepository":false,"maintainerCanModify":false}}"#
        );
        assert_eq!(facts_of(&json).unwrap().author, "", "{empty}");
    }
    let json = format!(
        r#"{{{FACTS_OK},"author":{{"login":"alice"}},"isCrossRepository":false,"maintainerCanModify":false}}"#
    );
    assert_eq!(facts_of(&json).unwrap().author, "alice");
}
