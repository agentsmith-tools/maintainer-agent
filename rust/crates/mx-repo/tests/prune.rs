//! `prune`: the decisions as pure functions, and the whole command over a
//! scripted process fake. Each test says the behaviour it pins; a quirk the
//! bash has and the port keeps names the plan decision (F5) or the pull
//! request's (P1H-n).

mod common;

use common::{Rig, Scratch, args, output, settings};
use mx_core::Fact;
use mx_repo::prune::{
    Verdict, decide_local, decide_remote, is_dry_run, is_protected, join_open_heads,
    open_pr_guard_fires,
};
use mx_repo::{USAGE, prune::cmd_prune};

const FETCH: &[&str] = &["git", "fetch", "-q", "--prune", "origin"];
const LIST: &[&str] = &[
    "gh",
    "pr",
    "list",
    "--repo",
    "o/r",
    "--state",
    "open",
    "--json",
    "headRefName",
    "--jq",
    ".[].headRefName",
];
const REMOTE_REFS: &[&str] =
    &["git", "for-each-ref", "--format=%(refname:strip=3)", "refs/remotes/origin"];
const LOCAL_REFS: &[&str] = &["git", "for-each-ref", "--format=%(refname:strip=2)", "refs/heads"];

fn never() -> Fact<bool> {
    panic!("the merge test must not run for this branch");
}

// ---- the decisions ----------------------------------------------------------------------

#[test]
fn f5_the_open_pr_guard_never_fires_for_a_branch_that_has_an_open_pull_request() {
    // Plan decision F5: the bash tests `grep -qx` against the heads joined with
    // `|`, so a head `feat` is the line `feat|` and no branch is named that.
    // The port keeps the dead guard; the output change is proposed in the pull request.
    let heads = Fact::Known(join_open_heads(b"feat\n"));
    assert_eq!(heads, Fact::Known("feat|".to_string()));
    assert_eq!(decide_remote("feat", &heads, || Fact::Known(true)), Verdict::Delete);
}

#[test]
fn f5_two_open_heads_keep_neither_branch() {
    let heads = Fact::Known(join_open_heads(b"a\nb\n"));
    assert_eq!(decide_remote("a", &heads, || Fact::Known(true)), Verdict::Delete);
    assert_eq!(decide_remote("b", &heads, || Fact::Known(true)), Verdict::Delete);
}

#[test]
fn f5_only_a_branch_named_exactly_like_the_joined_heads_is_kept() {
    assert!(open_pr_guard_fires("a|b|", "a|b|"));
    assert!(!open_pr_guard_fires("a", "a|b|"));
    assert!(!open_pr_guard_fires("a|b", "a|b|"));
    let heads = Fact::Known("a|b|".to_string());
    assert_eq!(decide_remote("a|b|", &heads, never), Verdict::KeepOpenPr);
}

#[test]
fn an_empty_list_of_open_heads_never_keeps_a_branch() {
    assert!(!open_pr_guard_fires("", ""));
    assert!(!open_pr_guard_fires("main", ""));
}

#[test]
fn open_heads_are_the_listing_with_every_newline_turned_into_a_pipe() {
    assert_eq!(join_open_heads(b"a\nb\n"), "a|b|");
    assert_eq!(join_open_heads(b"a"), "a");
    assert_eq!(join_open_heads(b""), "");
    assert_eq!(join_open_heads(b"\n\n"), "||");
    assert_eq!(join_open_heads(b"a\0b\n"), "ab|");
}

#[test]
fn a_gh_that_could_not_be_started_reads_as_no_open_pull_requests_and_the_branch_is_still_deleted() {
    // Fail open, pinned and listed under "Found, not fixed": the bash sends gh's
    // errors to /dev/null and goes on with an empty list.
    let heads: Fact<String> = Fact::Unreadable("gh: command not found".into());
    assert_eq!(decide_remote("feat", &heads, || Fact::Known(true)), Verdict::Delete);
}

#[test]
fn main_master_and_head_are_never_candidates_and_the_merge_test_is_not_asked() {
    let heads = Fact::Known(String::new());
    for name in ["main", "master", "HEAD"] {
        assert_eq!(decide_remote(name, &heads, never), Verdict::Skip, "{name}");
    }
    for name in ["main", "master"] {
        assert_eq!(decide_local(name, never), Verdict::Skip, "{name}");
    }
    assert!(is_protected("HEAD", true));
    assert!(!is_protected("HEAD", false), "a local branch may be called HEAD, as in the bash");
}

#[test]
fn only_a_known_merged_branch_is_deleted() {
    let heads = Fact::Known(String::new());
    assert_eq!(decide_remote("f", &heads, || Fact::Known(true)), Verdict::Delete);
    assert_eq!(decide_remote("f", &heads, || Fact::Known(false)), Verdict::Skip);
    assert_eq!(decide_remote("f", &heads, || Fact::Absent), Verdict::Skip);
    assert_eq!(decide_remote("f", &heads, || Fact::Unreadable("git died".into())), Verdict::Skip);
    assert_eq!(decide_local("f", || Fact::Unreadable("git died".into())), Verdict::Skip);
    assert_eq!(decide_local("f", || Fact::Known(true)), Verdict::Delete);
}

#[test]
fn only_a_first_argument_equal_to_dry_run_is_a_dry_run() {
    assert!(is_dry_run(&args(&["--dry-run"])));
    assert!(is_dry_run(&args(&["--dry-run", "x"])));
    assert!(!is_dry_run(&args(&["--dryrun"])));
    assert!(!is_dry_run(&args(&["--verbose", "--dry-run"])));
    assert!(!is_dry_run(&args(&[])));
}

// ---- the command ---------------------------------------------------------------------------

/// A repository with two merged remote branches, one unmerged, and two local ones.
fn scripted() -> Rig {
    let rig = Rig::new();
    rig.ok(FETCH, "");
    rig.ok(LIST, "");
    rig.ok(REMOTE_REFS, "main\nHEAD\nmerged-a\nopen-b\nmerged-c\n");
    rig.ok(&["git", "merge-base", "--is-ancestor", "origin/merged-a", "origin/main"], "");
    rig.reply(&["git", "merge-base", "--is-ancestor", "origin/open-b", "origin/main"], 1, "", "");
    rig.ok(&["git", "merge-base", "--is-ancestor", "origin/merged-c", "origin/main"], "");
    rig.ok(LOCAL_REFS, "main\nmerged-a\nwip\n");
    rig.ok(&["git", "merge-base", "--is-ancestor", "merged-a", "origin/main"], "");
    rig.reply(&["git", "merge-base", "--is-ancestor", "wip", "origin/main"], 1, "", "");
    rig
}

#[test]
fn a_dry_run_lists_what_it_would_delete_deletes_nothing_and_exits_1() {
    let scratch = Scratch::new("prune-dry");
    let rig = scripted();
    let got = rig.run(&[], |ctx| {
        cmd_prune(ctx, &settings(&scratch.0, "on"), &args(&["--dry-run"])).unwrap()
    });
    assert_eq!(
        got.code, 1,
        "the last command of the bash function is a false `[ \"$dry\" = 0 ] && ...`"
    );
    assert_eq!(
        got.out_lines(),
        [
            "  remote branches merged into main:",
            "    would delete merged-a",
            "    would delete merged-c",
            "  local branches fully merged into main:",
            "    would delete merged-a",
        ]
    );
    assert_eq!(got.err, "");
    let argvs = rig.argvs();
    assert!(!argvs.iter().any(|argv| argv.get(1).map(String::as_str) == Some("push")), "{argvs:?}");
    assert!(
        !argvs.iter().any(|argv| argv.get(1).map(String::as_str) == Some("branch")),
        "{argvs:?}"
    );
    assert!(
        !argvs.iter().any(|argv| argv.get(2).map(String::as_str) == Some("prune")),
        "{argvs:?}"
    );
    assert_eq!(
        argvs.len(),
        9,
        "fetch, gh, remote refs, 3 merge tests, local refs, 2 merge tests: {argvs:?}"
    );
}

#[test]
fn a_real_run_deletes_remote_then_local_branches_then_prunes_and_exits_0() {
    let scratch = Scratch::new("prune-real");
    let rig = scripted();
    rig.ok(&["git", "push", "-q", "origin", "--delete", "merged-a"], "");
    rig.reply(&["git", "push", "-q", "origin", "--delete", "merged-c"], 1, "", "remote: denied\n");
    rig.ok(&["git", "branch", "-D", "merged-a"], "");
    rig.ok(&["git", "worktree", "prune"], "");
    rig.ok(&["git", "remote", "prune", "origin"], "");
    let got = rig.run(&[], |ctx| cmd_prune(ctx, &settings(&scratch.0, "on"), &args(&[])).unwrap());
    assert_eq!(got.code, 0);
    assert_eq!(
        got.out_lines(),
        [
            "  remote branches merged into main:",
            "    deleted merged-a",
            "    could not delete merged-c (protected or already gone)",
            "  local branches fully merged into main:",
            "    deleted merged-a",
            "  worktrees and remote refs pruned",
        ]
    );
    assert_eq!(got.err, "", "the failed push's own message goes to /dev/null");
    let tail: Vec<Vec<String>> = rig.argvs().into_iter().rev().take(2).collect();
    assert_eq!(tail[0], ["git", "remote", "prune", "origin"]);
    assert_eq!(tail[1], ["git", "worktree", "prune"]);
}

#[test]
fn every_git_call_runs_in_the_repository_and_carries_the_caller_s_environment() {
    let scratch = Scratch::new("prune-env");
    let rig = scripted();
    let _ = rig.run(
        &[("HOME", "/h"), ("GH_TOKEN", "t0k"), ("UNRELATED", "x"), ("MAINTAINER_SLUG", "o/r")],
        |ctx| cmd_prune(ctx, &settings(&scratch.0, "on"), &args(&["--dry-run"])).unwrap(),
    );
    for spec in rig.specs() {
        assert_eq!(spec.cwd.as_deref(), Some(scratch.0.as_path()), "{spec:?}");
        let names: Vec<&str> = spec.env.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["GH_TOKEN", "HOME", "MAINTAINER_SLUG"], "{spec:?}");
    }
}

#[test]
fn post_off_says_so_first_and_makes_even_a_plain_run_a_listing_that_exits_1() {
    let scratch = Scratch::new("prune-off");
    let rig = scripted();
    let got = rig.run(&[], |ctx| cmd_prune(ctx, &settings(&scratch.0, "off"), &args(&[])).unwrap());
    assert_eq!(got.code, 1);
    assert_eq!(got.out_lines()[0], "  POST=off: listing only, nothing will be deleted");
    assert!(got.out.contains("    would delete merged-a"));
    assert!(rig.calls_starting(&["git", "push"]).is_empty());
}

#[test]
fn only_the_exact_word_off_is_a_rehearsal_so_post_uppercase_off_deletes() {
    let scratch = Scratch::new("prune-upper");
    let rig = scripted();
    rig.ok(&["git", "push", "-q", "origin", "--delete", "merged-a"], "");
    rig.ok(&["git", "push", "-q", "origin", "--delete", "merged-c"], "");
    rig.ok(&["git", "branch", "-D", "merged-a"], "");
    rig.ok(&["git", "worktree", "prune"], "");
    rig.ok(&["git", "remote", "prune", "origin"], "");
    let got = rig.run(&[], |ctx| cmd_prune(ctx, &settings(&scratch.0, "OFF"), &args(&[])).unwrap());
    assert_eq!(got.code, 0);
    assert!(got.out.contains("    deleted merged-a"));
}

#[test]
fn a_mistyped_flag_deletes_for_real() {
    let scratch = Scratch::new("prune-typo");
    let rig = scripted();
    rig.ok(&["git", "push", "-q", "origin", "--delete", "merged-a"], "");
    rig.ok(&["git", "push", "-q", "origin", "--delete", "merged-c"], "");
    rig.ok(&["git", "branch", "-D", "merged-a"], "");
    rig.ok(&["git", "worktree", "prune"], "");
    rig.ok(&["git", "remote", "prune", "origin"], "");
    let got = rig
        .run(&[], |ctx| cmd_prune(ctx, &settings(&scratch.0, "on"), &args(&["--dryrun"])).unwrap());
    assert_eq!(got.code, 0);
    assert_eq!(rig.calls_starting(&["git", "push"]).len(), 2);
}

#[test]
fn a_failed_fetch_stops_with_fetch_failed_after_git_s_own_message_and_before_any_gh_call() {
    let scratch = Scratch::new("prune-fetch");
    let rig = Rig::new();
    rig.reply(FETCH, 128, "", "fatal: 'x' does not appear to be a git repository\n");
    let got = rig.run(&[], |ctx| {
        cmd_prune(ctx, &settings(&scratch.0, "on"), &args(&["--dry-run"])).expect_err("must stop").0
    });
    assert_eq!(got.code, 1);
    assert_eq!(got.out, "");
    assert_eq!(
        got.err,
        "fatal: 'x' does not appear to be a git repository\nmaintainer-repo: fetch failed\n"
    );
    assert_eq!(rig.argvs().len(), 1, "no gh call, no listing");
}

#[test]
fn a_repository_path_that_is_not_a_directory_says_cd_failed_then_no_repo_at() {
    let rig = Rig::new();
    let missing = "/nonexistent/for/this/test";
    let got = rig.run(&[], |ctx| {
        cmd_prune(ctx, &settings(std::path::Path::new(missing), "on"), &args(&[]))
            .expect_err("must stop")
            .0
    });
    assert_eq!(got.code, 1);
    assert_eq!(
        got.err,
        "/tree/bin/maintainer-repo: line 32: cd: /nonexistent/for/this/test: No such file or directory\nmaintainer-repo: no repo at /nonexistent/for/this/test\n"
    );
    assert!(rig.argvs().is_empty());
}

#[test]
fn the_post_off_line_is_printed_before_the_repository_is_looked_for() {
    let rig = Rig::new();
    let got = rig.run(&[], |ctx| {
        cmd_prune(ctx, &settings(std::path::Path::new("/nonexistent/x"), "off"), &args(&[]))
            .expect_err("must stop")
            .0
    });
    assert_eq!(got.out, "  POST=off: listing only, nothing will be deleted\n");
    assert_eq!(got.code, 1);
}

#[test]
fn a_gh_that_fails_is_ignored_and_prints_nothing_and_the_listing_goes_on() {
    let scratch = Scratch::new("prune-ghfail");
    let rig = scripted();
    rig.proc.script(LIST, mx_ports::Script::always(Ok(output(1, "", "gh: boom\n"))));
    let got = rig.run(&[], |ctx| {
        cmd_prune(ctx, &settings(&scratch.0, "on"), &args(&["--dry-run"])).unwrap()
    });
    assert_eq!(got.err, "");
    assert!(got.out.contains("    would delete merged-a"));
}

#[test]
fn a_merged_local_branch_that_cannot_be_deleted_prints_nothing_but_is_counted() {
    // The bash counts the branch before it tries `git branch -D`, so the section
    // is empty rather than `none` when the delete fails (a checked-out branch).
    let scratch = Scratch::new("prune-checked-out");
    let rig = Rig::new();
    rig.ok(FETCH, "");
    rig.ok(LIST, "");
    rig.ok(REMOTE_REFS, "main\n");
    rig.ok(LOCAL_REFS, "feat\n");
    rig.ok(&["git", "merge-base", "--is-ancestor", "feat", "origin/main"], "");
    rig.reply(&["git", "branch", "-D", "feat"], 1, "", "error: Cannot delete branch\n");
    rig.ok(&["git", "worktree", "prune"], "");
    rig.ok(&["git", "remote", "prune", "origin"], "");
    let got = rig.run(&[], |ctx| cmd_prune(ctx, &settings(&scratch.0, "on"), &args(&[])).unwrap());
    assert_eq!(
        got.out_lines(),
        [
            "  remote branches merged into main:",
            "    none",
            "  local branches fully merged into main:",
            "  worktrees and remote refs pruned",
        ]
    );
}

#[test]
fn a_branch_name_that_is_not_utf8_ends_the_run_before_anything_is_deleted() {
    // Decision P1H-8: a process argument here is text, so a name that is not
    // text is refused up front. The bash would pass the bytes through.
    let scratch = Scratch::new("prune-utf8");
    let rig = Rig::new();
    rig.ok(FETCH, "");
    rig.ok(LIST, "");
    rig.proc.script(
        REMOTE_REFS,
        mx_ports::Script::always(Ok(mx_ports::ProcOutput {
            exit: mx_ports::ProcExit::Code(0),
            stdout: b"good\nbad\xffname\n".to_vec(),
            stderr: Vec::new(),
        })),
    );
    let got = rig.run(&[], |ctx| {
        cmd_prune(ctx, &settings(&scratch.0, "on"), &args(&[])).expect_err("must stop").0
    });
    assert_eq!(got.code, 1);
    assert!(
        got.err.contains("a branch name is not UTF-8 text, so nothing was deleted"),
        "{}",
        got.err
    );
    assert!(rig.calls_starting(&["git", "push"]).is_empty());
    assert!(rig.calls_starting(&["git", "merge-base"]).is_empty());
}

#[test]
fn the_usage_line_is_the_bash_s_word_for_word() {
    assert_eq!(
        USAGE,
        "usage: maintainer-repo {prune [--dry-run]|release-check|fixup <pr> [--dry-run]}"
    );
}

#[test]
fn branches_are_read_the_way_while_read_reads_them() {
    // `read -r` trims space and tab, skips empty lines, and a remote name with a
    // slash is whole.
    let scratch = Scratch::new("prune-read");
    let rig = Rig::new();
    rig.ok(FETCH, "");
    rig.ok(LIST, "");
    rig.ok(REMOTE_REFS, "\n  feature/login \t\n\nmain\n");
    rig.ok(&["git", "merge-base", "--is-ancestor", "origin/feature/login", "origin/main"], "");
    rig.ok(LOCAL_REFS, "");
    let got = rig.run(&[], |ctx| {
        cmd_prune(ctx, &settings(&scratch.0, "on"), &args(&["--dry-run"])).unwrap()
    });
    assert_eq!(
        got.out_lines(),
        [
            "  remote branches merged into main:",
            "    would delete feature/login",
            "  local branches fully merged into main:",
            "    none",
        ]
    );
}

#[test]
fn a_child_that_runs_past_its_timeout_counts_as_failed_not_as_success() {
    // The bash has no timeout; here a killed child reads as status 124, as
    // `timeout` reports it (decision P1H-5), so the fetch check refuses.
    let scratch = Scratch::new("prune-timeout");
    let rig = Rig::new();
    rig.proc.script(
        FETCH,
        mx_ports::Script::always(Ok(mx_ports::ProcOutput {
            exit: mx_ports::ProcExit::TimedOut,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })),
    );
    let got = rig.run(&[], |ctx| {
        cmd_prune(ctx, &settings(&scratch.0, "on"), &args(&[])).expect_err("must stop").0
    });
    assert_eq!(got.code, 1);
    assert_eq!(got.err, "maintainer-repo: fetch failed\n");
}
