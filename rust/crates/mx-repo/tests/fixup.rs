//! `fixup`: the command line, the answer of `fixup-check`, the text of the
//! heads-up, the count check and the whole command over a scripted process fake.
//! The git and gh calls are pinned with their exact argument vectors, working
//! directories and environments.

mod common;

use common::{Outcome, Rig, Scratch, args, output, settings_with};
use mx_ports::{Proc, ProcError, ProcOutput, ProcSpec};
use mx_repo::fixup::{
    Check, cmd_fixup, heads_up_confirmed, item_kinds, join_kinds, notice_body, parse_args,
    parse_check, patch_states_count, same_login, split_account,
};
use mx_repo::{Settings, Tree};
use std::cell::RefCell;
use std::path::PathBuf;

// ---- pure pieces --------------------------------------------------------------------------------

#[test]
fn the_command_line_takes_digits_and_the_last_number_wins() {
    assert_eq!(parse_args(&args(&["7"])).pr.as_deref(), Some("7"));
    assert_eq!(parse_args(&args(&["8", "7"])).pr.as_deref(), Some("7"));
    assert_eq!(parse_args(&args(&["007"])).pr.as_deref(), Some("007"));
    assert!(parse_args(&args(&["7", "--dry-run"])).dry_run);
    assert!(parse_args(&args(&["--dry-run", "7"])).dry_run);
}

#[test]
fn any_word_that_is_not_all_digits_cancels_the_number_and_stops_reading() {
    for words in
        [&["7", "abc"][..], &["abc", "7"], &["12x"], &["-7"], &["1.5"], &[""], &["7", ""], &[]]
    {
        assert_eq!(parse_args(&args(words)).pr, None, "{words:?}");
    }
    assert_eq!(parse_args(&args(&["--dry-run"])).pr, None);
}

#[test]
fn an_answer_is_read_field_by_field_and_a_repeated_key_makes_a_multi_line_value() {
    let out = b"state=ready\nhead=abc\nbase=main\nbranch=fix-1\nfork=alice/r\nauthor=bob\nitems=typo:docs.md\n\
                patch=/p\nnotice_marker=<!-- m -->\nnotice_due=2026-11-20\nnotice_days=5\nreview_age=12\nstate=extra\n";
    let check = parse_check(out).unwrap();
    assert_eq!(check.state, "ready\nextra");
    assert_eq!(check.items, "typo:docs.md");
    assert_eq!(check.marker, "<!-- m -->");
    assert!(check.is_complete());
}

#[test]
fn an_answer_missing_any_of_five_fields_is_not_complete() {
    let full = "state=ready\nhead=h\nbranch=b\nfork=f\nitems=i\n";
    assert!(parse_check(full.as_bytes()).unwrap().is_complete());
    for key in ["state", "head", "branch", "fork", "items"] {
        let cut = full.lines().filter(|line| !line.starts_with(key)).fold(
            String::new(),
            |mut acc, line| {
                acc.push_str(line);
                acc.push('\n');
                acc
            },
        );
        assert!(!parse_check(cut.as_bytes()).unwrap().is_complete(), "without {key}");
        let emptied = full.replace(&format!("{key}="), &format!("{key}=\n{key}="));
        assert!(parse_check(emptied.as_bytes()).is_ok());
    }
    assert!(!Check::default().is_complete());
}

#[test]
fn an_answer_with_text_that_is_not_utf8_is_refused_by_name() {
    assert_eq!(parse_check(b"branch=a\xffb\n").unwrap_err(), "branch is not UTF-8 text");
}

#[test]
fn item_kinds_split_on_spaces_only_and_cut_each_item_at_its_first_colon() {
    assert_eq!(item_kinds("typo:docs.md"), ["typo"]);
    assert_eq!(item_kinds("rebase typo:a:b"), ["rebase", "typo"]);
    assert_eq!(item_kinds("a  b"), ["a", "", "b"], "a double space makes an empty kind");
    assert_eq!(item_kinds("a "), ["a", ""], "a trailing space makes one too");
    assert_eq!(item_kinds("a\tb"), ["a\tb"], "tr turns spaces only");
}

#[test]
fn kinds_join_with_comma_space_and_every_comma_is_widened() {
    assert_eq!(join_kinds(&["docs".into(), "typo".into()]), "docs, typo");
    assert_eq!(join_kinds(&["a,b".into(), "c".into()]), "a, b, c");
    assert_eq!(join_kinds(&[]), "");
}

#[test]
fn the_heads_up_text_is_the_bash_s_with_no_trailing_newline() {
    let body =
        notice_body("12", "typo, test-count", "2026-11-20", "<!-- maintainer-fixup-notice -->");
    assert_eq!(
        body,
        "This has had no reply for 12 days since my review, and what is left is mechanical (typo, test-count).\n\n\
         I will push a fixup for it to your branch after 2026-11-20 unless you push first. Your commits stay yours, \
         and I will credit you on anything I add. If you would rather do it yourself, push or reply and I will leave it alone.\n\n\
         <!-- maintainer-fixup-notice -->"
    );
    assert!(
        notice_body("1", "x", "d", "").ends_with("leave it alone."),
        "an empty marker leaves no trailing blank lines"
    );
}

#[test]
fn a_count_is_stated_only_by_an_added_line_that_holds_it_as_a_whole_number() {
    assert!(patch_states_count(b"+there are 3 tests", "3"));
    assert!(patch_states_count(b"+3 tests", "3"));
    assert!(patch_states_count(b"+tests: 3", "3"));
    assert!(patch_states_count(b"+ 3", "3"));
    assert!(!patch_states_count(b"+there are 13 tests", "3"), "a digit before the count");
    assert!(!patch_states_count(b"+there are 30 tests", "3"), "a digit after the count");
    assert!(patch_states_count(b"+3", "3"), "a line of '+3' states 3");
    assert!(!patch_states_count(b" there are 3 tests", "3"), "a context line is not an added line");
    assert!(!patch_states_count(b"-there are 3 tests", "3"), "a removed line is not an added line");
    assert!(!patch_states_count(b"++3 tests", "3"), "a line that starts ++ is a file header");
    assert!(!patch_states_count(b"+", "3"), "a lone plus has no second character");
    assert!(patch_states_count(b"+003 tests", "003"));
    assert!(!patch_states_count(b"+003 tests", "3"));
    assert!(patch_states_count(b"x\n+there are 3 tests\ny", "3"));
    assert!(!patch_states_count(b"", "3"));
}

#[test]
fn an_account_answer_splits_at_the_first_tab_and_with_no_tab_both_halves_are_the_whole() {
    assert_eq!(split_account("42\ttestuser"), ("42", "testuser"));
    assert_eq!(split_account("42\ta\tb"), ("42", "a\tb"));
    assert_eq!(split_account("42"), ("42", "42"));
    assert_eq!(split_account("\tx"), ("", "x"));
}

#[test]
fn logins_compare_without_regard_to_ascii_case() {
    assert!(same_login("TestUser", "testuser"));
    assert!(!same_login("testuser", "testuser2"));
}

#[test]
fn a_heads_up_is_confirmed_by_the_not_ripe_wording_or_a_ready_state_line() {
    assert!(heads_up_confirmed(b"state=ready\nhead=x\n"));
    assert!(heads_up_confirmed(
        b"maintainer: fixup-check #7: the heads-up is not ripe: the author has until x"
    ));
    assert!(!heads_up_confirmed(b"state=notice\n"));
    assert!(!heads_up_confirmed(b"x state=ready\n"));
    assert!(!heads_up_confirmed(b"state=ready \n"));
    assert!(!heads_up_confirmed(b""));
}

// ---- the command --------------------------------------------------------------------------------

const HEAD: &str = "1111111111111111111111111111111111111111";
const NEW: &str = "2222222222222222222222222222222222222222";

fn tree() -> Tree {
    Tree { bin_dir: PathBuf::from("/tree/bin"), lib_dir: PathBuf::from("/tree/lib") }
}

fn check_argv(pr: &str) -> Vec<&str> {
    vec!["python3", "/tree/bin/maintainer", "fixup-check", pr]
}

fn answer(state: &str, items: &str, patch: &str) -> String {
    format!(
        "state={state}\nhead={HEAD}\nbase=main\nbranch=fix-1\nfork=alice/r\nauthor=bob\nitems={items}\npatch={patch}\n\
         notice_marker=<!-- maintainer-fixup-notice -->\nnotice_due=2026-11-20\nnotice_days=5\nreview_age=12\n"
    )
}

const SWITCH: &[&str] = &["gh", "auth", "switch", "--hostname", "github.com", "--user", "testuser"];
const ME: &[&str] = &["gh", "api", "user", "--jq", "[.id,.login]|@tsv"];
const AUTHOR: &[&str] = &["gh", "api", "users/bob", "--jq", "[.id,.login]|@tsv"];

/// The git command the bash's `g` runs inside the scratch clone.
fn g(work: &str, rest: &[&str]) -> Vec<String> {
    let mut argv: Vec<String> = [
        "git",
        "-C",
        &format!("{work}/r"),
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.fsmonitor=false",
    ]
    .map(String::from)
    .to_vec();
    argv.extend(rest.iter().map(|word| (*word).to_string()));
    argv
}

fn script(rig: &Rig, argv: &[String], code: i32, stdout: &str) {
    let words: Vec<&str> = argv.iter().map(String::as_str).collect();
    rig.reply(&words, code, stdout, "");
}

struct World {
    scratch: Scratch,
    work: String,
    patch: PathBuf,
    rig: Rig,
}

/// What differs from a clean `ready` run.
struct Opts {
    /// What `git rev-parse HEAD` prints, call by call.
    head_reads: Vec<String>,
    mktemp_fails: bool,
    clone_fails: bool,
    switch_rc: i32,
    /// The patch path `fixup-check` names, when it is not the real file.
    patch_path: Option<String>,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            head_reads: vec![HEAD.to_string(), NEW.to_string()],
            mktemp_fails: false,
            clone_fails: false,
            switch_rc: 0,
            patch_path: None,
        }
    }
}

/// Everything a clean `ready` fixup needs, scripted up to the clone and the first read of HEAD.
fn ready(items: &str, patch_text: &str) -> World {
    world_with(items, patch_text, &Opts::default())
}

fn world_with(items: &str, patch_text: &str, opts: &Opts) -> World {
    let scratch = Scratch::new("fixup");
    let work = scratch.dir("work");
    let patch = scratch.file("fix.patch", patch_text);
    let work_text = work.to_string_lossy().into_owned();
    let rig = Rig::new();
    let named = opts.patch_path.clone().unwrap_or_else(|| patch.to_string_lossy().into_owned());
    rig.ok(&check_argv("7"), &answer("ready", items, &named));
    rig.reply(SWITCH, opts.switch_rc, "", "no such account\n");
    rig.ok(ME, "42\ttestuser\n");
    rig.ok(AUTHOR, "7\tbob\n");
    let state = scratch.0.to_string_lossy().into_owned();
    let mktemp = ["mktemp", "-d", &format!("{state}/fixup-XXXXXX")];
    if opts.mktemp_fails {
        rig.reply(&mktemp, 1, "", "mktemp: failed to create directory\n");
    } else {
        rig.ok(&mktemp, &format!("{work_text}\n"));
    }
    let url = "https://github.com/alice/r.git";
    let clone = [
        "git",
        "-c",
        "core.hooksPath=/dev/null",
        "clone",
        "-q",
        "--no-tags",
        "--branch",
        "fix-1",
        "--single-branch",
        "--",
        url,
        &format!("{work_text}/r"),
    ];
    if opts.clone_fails {
        rig.reply(&clone, 128, "", "fatal: nope\n");
    } else {
        rig.ok(&clone, "");
    }
    let reads = opts.head_reads.iter().map(|text| output(0, &format!("{text}\n"), "")).collect();
    let head_argv = g(&work_text, &["rev-parse", "HEAD"]);
    rig.sequence(&head_argv.iter().map(String::as_str).collect::<Vec<_>>(), reads);
    World { scratch, work: work_text, patch, rig }
}

fn tunables(world: &World, extra: &[(&str, &str)]) -> Settings {
    let state = world.scratch.0.to_string_lossy().into_owned();
    let mut all = vec![("GH_TRIES", "3"), ("GH_BACKOFF", "0")];
    all.extend_from_slice(extra);
    settings_with(&world.scratch.0, &state, "on", &all)
}

fn run_fixup(world: &World, settings: &Settings, words: &[&str]) -> Outcome {
    world.rig.run(&[("HOME", "/h"), ("GH_TOKEN", "t0k"), ("MAINTAINER_SLUG", "o/r")], |ctx| {
        match cmd_fixup(ctx, settings, &tree(), &args(words)) {
            Ok(code) => code,
            Err(stop) => stop.0,
        }
    })
}

/// The scripted calls of a typo fixup up to the plan line.
fn commit_calls(world: &World, signed: bool) {
    let w = &world.work;
    let patch = world.patch.to_string_lossy().into_owned();
    script(&world.rig, &g(w, &["apply", "--check", &patch]), 0, "");
    script(&world.rig, &g(w, &["apply", "--index", &patch]), 0, "");
    script(
        &world.rig,
        &g(w, &["log", "-n", "50", "--format=%B", HEAD]),
        0,
        if signed { "docs: add one\n\nSigned-off-by: Bob <b@x>\n" } else { "docs: add one\n" },
    );
    let mut commit = vec!["commit", "-q"];
    if signed {
        commit.push("-s");
    }
    commit.extend([
        "-m",
        "fixup: typo from the review",
        "-m",
        "Co-authored-by: bob <7+bob@users.noreply.github.com>",
    ]);
    script(&world.rig, &g(w, &commit), 0, "");
    script(&world.rig, &g(w, &["rev-parse", "--short", HEAD]), 0, "1111111\n");
    script(&world.rig, &g(w, &["rev-parse", "--short", NEW]), 0, "2222222\n");
    world.rig.ok(&["sort", "-u"], "typo\n");
}

const TYPO_PATCH: &str =
    "diff --git a/docs.md b/docs.md\n--- a/docs.md\n+++ b/docs.md\n@@ -1 +1,2 @@\n one\n+two\n";

#[test]
fn no_pull_request_prints_the_usage_line_and_exits_64_before_any_call() {
    let rig = Rig::new();
    let got = rig.run(&[], |ctx| {
        cmd_fixup(
            ctx,
            &common::settings(std::path::Path::new("/r"), "on"),
            &tree(),
            &args(&["abc"]),
        )
        .unwrap_err()
        .0
    });
    assert_eq!(got.code, 64);
    assert_eq!(got.err, "usage: maintainer-repo fixup <pr> [--dry-run]\n");
    assert_eq!(got.out, "");
    assert!(rig.argvs().is_empty());
}

#[test]
fn post_off_says_so_before_it_asks_fixup_check_and_makes_the_run_a_rehearsal() {
    let world = ready("typo:docs.md", TYPO_PATCH);
    commit_calls(&world, false);
    let w = world.work.clone();
    script(&world.rig, &g(&w, &["show", "--stat", "--patch", "--format=%B", "HEAD"]), 0, "");
    let off = settings_with(
        &world.scratch.0,
        &world.scratch.0.to_string_lossy(),
        "off",
        &[("GH_TRIES", "3")],
    );
    let got = run_fixup(&world, &off, &["7"]);
    assert_eq!(got.code, 0, "{}", got.err);
    assert_eq!(got.out_lines()[0], "  POST=off: rehearsing, nothing will be posted or pushed");
    assert!(got.out.contains("  rehearsal: nothing was pushed\n"));
}

#[test]
fn fixup_check_runs_as_python3_beside_the_program_and_its_failure_ends_the_run_with_1_after_its_own_message()
 {
    let rig = Rig::new();
    rig.reply(&check_argv("7"), 3, "state=ready\n", "refused: not ripe\n");
    let settings = settings_with(&PathBuf::from("/r"), "/s", "on", &[]);
    let got = rig.run(&[], |ctx| cmd_fixup(ctx, &settings, &tree(), &args(&["7"])).unwrap_err().0);
    assert_eq!(got.code, 1);
    assert_eq!(got.out, "", "its standard output is discarded with it");
    assert_eq!(got.err, "refused: not ripe\n");
    assert_eq!(rig.argvs().len(), 1, "no gh call");
}

#[test]
fn an_answer_missing_a_branch_is_refused_in_one_line_before_any_gh_call() {
    let rig = Rig::new();
    rig.ok(&check_argv("7"), &format!("state=ready\nhead={HEAD}\nfork=a/b\nitems=typo:x\n"));
    let settings = settings_with(&PathBuf::from("/r"), "/s", "on", &[]);
    let got = rig.run(&[], |ctx| cmd_fixup(ctx, &settings, &tree(), &args(&["7"])).unwrap_err().0);
    assert_eq!(got.code, 1);
    assert_eq!(
        got.err,
        "maintainer-repo: fixup-check answered without a state, head, branch, fork or items; refusing to act on that\n"
    );
    assert_eq!(rig.argvs().len(), 1);
}

#[test]
fn the_account_is_switched_to_first_and_then_proven_by_gh_api_user() {
    let rig = Rig::new();
    rig.ok(&check_argv("7"), &answer("ready", "typo:x", "/p"));
    rig.ok(SWITCH, "");
    rig.reply(ME, 1, "", "gh: boom\n");
    let settings = settings_with(&PathBuf::from("/r"), "/s", "on", &[]);
    let got = rig.run(&[], |ctx| cmd_fixup(ctx, &settings, &tree(), &args(&["7"])).unwrap_err().0);
    assert_eq!(got.err, "maintainer-repo: gh could not confirm an account\n");
    assert_eq!(rig.argvs()[1], SWITCH);
    assert_eq!(rig.argvs()[2], ME);
}

#[test]
fn each_account_answer_that_is_not_the_profile_s_account_is_refused_by_name() {
    for (reply, message) in [
        ("abc\ttestuser", "gh did not return a numeric account id"),
        ("\ttestuser", "gh did not return a numeric account id"),
        ("42\tsomeoneelse", "gh is authenticated as 'someoneelse', not testuser"),
    ] {
        let rig = Rig::new();
        rig.ok(&check_argv("7"), &answer("ready", "typo:x", "/p"));
        rig.ok(SWITCH, "");
        rig.ok(ME, &format!("{reply}\n"));
        let settings = settings_with(&PathBuf::from("/r"), "/s", "on", &[]);
        let got =
            rig.run(&[], |ctx| cmd_fixup(ctx, &settings, &tree(), &args(&["7"])).unwrap_err().0);
        assert_eq!(got.err, format!("maintainer-repo: {message}\n"), "{reply}");
    }
}

#[test]
fn a_failing_auth_switch_is_ignored_when_gh_api_user_answers() {
    let world = world_with("typo:docs.md", TYPO_PATCH, &Opts { switch_rc: 1, ..Opts::default() });
    commit_calls(&world, false);
    let w = world.work.clone();
    script(&world.rig, &g(&w, &["show", "--stat", "--patch", "--format=%B", "HEAD"]), 0, "");
    let got = run_fixup(&world, &tunables(&world, &[]), &["7", "--dry-run"]);
    assert_eq!(got.code, 0, "{}", got.err);
    assert_eq!(got.err, "", "the switch's own message is dropped");
}

#[test]
fn a_dry_run_prints_the_plan_the_patch_indented_and_pushes_nothing() {
    let world = ready("typo:docs.md", TYPO_PATCH);
    commit_calls(&world, false);
    let w = world.work.clone();
    script(
        &world.rig,
        &g(&w, &["show", "--stat", "--patch", "--format=%B", "HEAD"]),
        0,
        "fixup: typo\n\nCo-authored-by: bob\n\n docs.md | 1 +\n",
    );
    let got = run_fixup(&world, &tunables(&world, &[]), &["7", "--dry-run"]);
    assert_eq!(got.code, 0, "{}", got.err);
    assert_eq!(
        got.out,
        "  plan: typo:docs.md on alice/r fix-1, 1111111 -> 2222222, as testuser <42+testuser@users.noreply.github.com>\n\
         \x20   fixup: typo\n    \n    Co-authored-by: bob\n    \n     docs.md | 1 +\n  rehearsal: nothing was pushed\n"
    );
    assert!(
        world
            .rig
            .calls_starting(&["git", "-C"])
            .iter()
            .all(|argv| !argv.contains(&"push".to_string()))
    );
    assert!(
        !std::path::Path::new(&world.work).exists(),
        "the scratch directory is removed on every exit"
    );
}

#[test]
fn the_scratch_clone_is_made_with_hooks_off_and_every_later_child_has_the_git_exports() {
    let world = ready("typo:docs.md", TYPO_PATCH);
    commit_calls(&world, false);
    let w = world.work.clone();
    script(&world.rig, &g(&w, &["show", "--stat", "--patch", "--format=%B", "HEAD"]), 0, "");
    let _ = run_fixup(&world, &tunables(&world, &[]), &["7", "--dry-run"]);
    let specs = world.rig.specs();
    let env_of = |spec: &ProcSpec, name: &str| {
        spec.env.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    };
    let clone = specs.iter().find(|s| s.argv.as_slice().contains(&"clone".to_string())).unwrap();
    assert_eq!(env_of(clone, "GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
    assert_eq!(env_of(clone, "GIT_EDITOR").as_deref(), Some("true"));
    assert_eq!(
        env_of(clone, "GIT_COMMITTER_NAME"),
        None,
        "the identity is exported after the clone"
    );
    let commit = specs.iter().find(|s| s.argv.as_slice().contains(&"commit".to_string())).unwrap();
    assert_eq!(env_of(commit, "GIT_COMMITTER_NAME").as_deref(), Some("testuser"));
    assert_eq!(
        env_of(commit, "GIT_AUTHOR_EMAIL").as_deref(),
        Some("42+testuser@users.noreply.github.com")
    );
    assert_eq!(
        env_of(commit, "GH_TOKEN").as_deref(),
        Some("t0k"),
        "the caller's token reaches gh and git as it does in the bash"
    );
}

#[test]
fn a_real_run_pushes_one_commit_without_a_lease_and_confirms_the_head_from_the_pull_request() {
    let world = ready("typo:docs.md", TYPO_PATCH);
    commit_calls(&world, false);
    let w = world.work.clone();
    script(
        &world.rig,
        &g(&w, &["push", "-q", "https://github.com/alice/r.git", "HEAD:refs/heads/fix-1"]),
        0,
        "",
    );
    world.rig.ok(
        &["gh", "pr", "view", "7", "--repo", "o/r", "--json", "headRefOid", "--jq", ".headRefOid"],
        &format!("{NEW}\n"),
    );
    let got = run_fixup(&world, &tunables(&world, &[]), &["7"]);
    assert_eq!(got.code, 0, "{}", got.err);
    assert_eq!(
        got.out_lines().last().copied(),
        Some(&*format!(
            "  fixup pushed to alice/r fix-1; #7 is now at {NEW} (read back from GitHub)"
        ))
    );
    assert_eq!(got.err, "");
    assert_eq!(world.rig.calls_starting(&["gh", "pr", "view"]).len(), 1);
}

#[test]
fn a_signed_off_branch_makes_the_fixup_commit_signed_off() {
    let world = ready("typo:docs.md", TYPO_PATCH);
    commit_calls(&world, true);
    let w = world.work.clone();
    script(&world.rig, &g(&w, &["show", "--stat", "--patch", "--format=%B", "HEAD"]), 0, "");
    let got = run_fixup(&world, &tunables(&world, &[]), &["7", "--dry-run"]);
    assert_eq!(got.code, 0, "{}", got.err);
}

#[test]
fn a_message_that_only_mentions_signed_off_by_mid_line_does_not_turn_the_sign_off_on() {
    let world = ready("typo:docs.md", TYPO_PATCH);
    let w = world.work.clone();
    let patch = world.patch.to_string_lossy().into_owned();
    script(&world.rig, &g(&w, &["apply", "--check", &patch]), 0, "");
    script(&world.rig, &g(&w, &["apply", "--index", &patch]), 0, "");
    script(
        &world.rig,
        &g(&w, &["log", "-n", "50", "--format=%B", HEAD]),
        0,
        "reminder: Signed-off-by: is still to come\n",
    );
    world.rig.ok(&["sort", "-u"], "typo\n");
    // No `-s` in this commit call: a call with `-s` has no script, and the fake refuses it.
    script(
        &world.rig,
        &g(
            &w,
            &[
                "commit",
                "-q",
                "-m",
                "fixup: typo from the review",
                "-m",
                "Co-authored-by: bob <7+bob@users.noreply.github.com>",
            ],
        ),
        1,
        "",
    );
    let got = run_fixup(&world, &tunables(&world, &[]), &["7"]);
    assert_eq!(got.err, "maintainer-repo: could not commit the patch\n");
}

#[test]
fn the_kinds_in_the_subject_are_what_sort_u_returns_for_the_kinds_the_run_sends_it() {
    let world = ready("typo:docs.md docs:docs.md typo:other rebase", TYPO_PATCH);
    let w = world.work.clone();
    // The rebase item runs first.
    script(&world.rig, &g(&w, &["remote", "add", "base", "https://github.com/o/r.git"]), 0, "");
    script(&world.rig, &g(&w, &["fetch", "-q", "base", "main"]), 0, "");
    script(&world.rig, &g(&w, &["rebase", "-q", "refs/remotes/base/main"]), 0, "");
    world.rig.ok(&["sort", "-u"], "docs\ntypo\n");
    let patch = world.patch.to_string_lossy().into_owned();
    script(&world.rig, &g(&w, &["apply", "--check", &patch]), 0, "");
    script(&world.rig, &g(&w, &["apply", "--index", &patch]), 0, "");
    script(&world.rig, &g(&w, &["log", "-n", "50", "--format=%B", HEAD]), 0, "");
    script(
        &world.rig,
        &g(
            &w,
            &[
                "commit",
                "-q",
                "-m",
                "fixup: docs, typo from the review",
                "-m",
                "Co-authored-by: bob <7+bob@users.noreply.github.com>",
            ],
        ),
        1,
        "",
    );
    let got = run_fixup(&world, &tunables(&world, &[]), &["7"]);
    assert_eq!(got.err, "maintainer-repo: could not commit the patch\n");
    let sort = world.rig.specs().into_iter().find(|s| s.argv.as_slice() == ["sort", "-u"]).unwrap();
    assert_eq!(
        sort.stdin.as_deref(),
        Some(&b"typo\ndocs\ntypo\n"[..]),
        "the rebase kind is filtered, the rest sorted by sort -u"
    );
}

#[test]
fn a_rebase_conflict_aborts_the_rebase_and_pushes_nothing() {
    let world = ready("rebase", "");
    let w = world.work.clone();
    script(&world.rig, &g(&w, &["remote", "add", "base", "https://github.com/o/r.git"]), 0, "");
    script(&world.rig, &g(&w, &["fetch", "-q", "base", "main"]), 0, "");
    script(&world.rig, &g(&w, &["rebase", "-q", "refs/remotes/base/main"]), 1, "");
    script(&world.rig, &g(&w, &["rebase", "--abort"]), 0, "");
    let got = run_fixup(&world, &tunables(&world, &[]), &["7"]);
    assert_eq!(got.code, 1);
    assert_eq!(
        got.err,
        "maintainer-repo: the rebase onto main conflicts. Resolving conflicts is not automated; nothing was pushed\n"
    );
    assert_eq!(
        world.rig.calls_starting(&["git", "-C"]).last().unwrap()[9..],
        ["rebase", "--abort"]
    );
}

#[test]
fn a_rebase_pushes_with_a_lease_against_the_checked_head() {
    let world = ready("rebase", "");
    let w = world.work.clone();
    script(&world.rig, &g(&w, &["remote", "add", "base", "https://github.com/o/r.git"]), 0, "");
    script(&world.rig, &g(&w, &["fetch", "-q", "base", "main"]), 0, "");
    script(&world.rig, &g(&w, &["rebase", "-q", "refs/remotes/base/main"]), 0, "");
    script(&world.rig, &g(&w, &["rev-parse", "--short", HEAD]), 0, "1111111\n");
    script(&world.rig, &g(&w, &["rev-parse", "--short", NEW]), 0, "2222222\n");
    let lease = format!("--force-with-lease=refs/heads/fix-1:{HEAD}");
    script(
        &world.rig,
        &g(&w, &["push", "-q", &lease, "https://github.com/alice/r.git", "HEAD:refs/heads/fix-1"]),
        0,
        "",
    );
    world.rig.ok(
        &["gh", "pr", "view", "7", "--repo", "o/r", "--json", "headRefOid", "--jq", ".headRefOid"],
        &format!("{NEW}\n"),
    );
    let got = run_fixup(&world, &tunables(&world, &[]), &["7"]);
    assert_eq!(got.code, 0, "{}", got.err);
    assert!(got.out.contains("  plan: rebase on alice/r fix-1, 1111111 -> 2222222, as testuser"));
    assert!(
        world.rig.calls_starting(&["gh", "api", "users/bob"]).is_empty(),
        "a bare rebase asks gh for no author"
    );
}

#[test]
fn a_mechanical_item_with_no_patch_is_refused_naming_the_kind() {
    let scratch_missing = "/nonexistent/nowhere.patch".to_string();
    let world = world_with(
        "typo:docs.md",
        TYPO_PATCH,
        &Opts { patch_path: Some(scratch_missing), ..Opts::default() },
    );
    world.rig.ok(&["sort", "-u"], "typo\n");
    let got = run_fixup(&world, &tunables(&world, &[]), &["7"]);
    assert_eq!(got.err, "maintainer-repo: no patch for: typo\n");
    assert_eq!(got.code, 1);
}

#[test]
fn a_patch_that_does_not_apply_is_refused_and_nothing_is_committed() {
    let world = ready("typo:docs.md", TYPO_PATCH);
    let w = world.work.clone();
    let patch = world.patch.to_string_lossy().into_owned();
    script(&world.rig, &g(&w, &["apply", "--check", &patch]), 1, "");
    world.rig.ok(&["sort", "-u"], "typo\n");
    let got = run_fixup(&world, &tunables(&world, &[]), &["7"]);
    assert_eq!(
        got.err,
        "maintainer-repo: the patch does not apply to fix-1 after any rebase; nothing was pushed\n"
    );
}

#[test]
fn a_branch_that_moved_since_the_check_is_refused_with_both_hashes() {
    let world = world_with(
        "typo:docs.md",
        TYPO_PATCH,
        &Opts { head_reads: vec![NEW.to_string()], ..Opts::default() },
    );
    let w = world.work.clone();
    script(&world.rig, &g(&w, &["rev-parse", "--short", "HEAD"]), 0, "2222222\n");
    let got = run_fixup(&world, &tunables(&world, &[]), &["7"]);
    assert_eq!(
        got.err,
        format!(
            "maintainer-repo: fix-1 has moved since the check (now 2222222, checked {HEAD}); nothing was pushed\n"
        )
    );
}

#[test]
fn a_fork_that_cannot_be_cloned_is_named_in_the_refusal() {
    let world =
        world_with("typo:docs.md", TYPO_PATCH, &Opts { clone_fails: true, ..Opts::default() });
    let got = run_fixup(&world, &tunables(&world, &[]), &["7"]);
    assert_eq!(
        got.err, "maintainer-repo: could not clone alice/r fix-1\n",
        "clone's own message goes to /dev/null"
    );
}

#[test]
fn a_state_directory_that_cannot_hold_the_scratch_clone_is_refused_after_mktemp_s_own_message() {
    let world =
        world_with("typo:docs.md", TYPO_PATCH, &Opts { mktemp_fails: true, ..Opts::default() });
    let got = run_fixup(&world, &tunables(&world, &[]), &["7"]);
    assert_eq!(
        got.err,
        "mktemp: failed to create directory\nmaintainer-repo: no scratch directory\n"
    );
}

#[test]
fn an_empty_state_directory_is_the_shell_s_parameter_null_error_and_exits_1() {
    let world = ready("typo:docs.md", TYPO_PATCH);
    let settings = settings_with(&world.scratch.0, "", "on", &[("GH_TRIES", "3")]);
    let got = run_fixup(&world, &settings, &["7"]);
    assert_eq!(got.code, 1);
    assert_eq!(
        got.err,
        "/tree/bin/maintainer-repo: line 270: MAINTAINER_STATE: parameter null or not set\n"
    );
}

#[test]
fn the_count_command_runs_in_the_clone_without_the_gh_tokens_and_the_patch_must_state_its_number() {
    let world = ready("test-count:docs.md", "diff\n+there are 13 tests\n");
    let w = world.work.clone();
    let patch = world.patch.to_string_lossy().into_owned();
    script(&world.rig, &g(&w, &["apply", "--check", &patch]), 0, "");
    script(&world.rig, &g(&w, &["apply", "--index", &patch]), 0, "");
    world.rig.ok(&["sort", "-u"], "test-count\n");
    world.rig.ok(
        &["env", "-u", "GH_TOKEN", "-u", "GITHUB_TOKEN", "timeout", "120", "bash", "-c", "echo 3"],
        " 3 \n",
    );
    script(&world.rig, &g(&w, &["diff", "--cached", "-U0"]), 0, "+there are 13 tests\n");
    let got = run_fixup(&world, &tunables(&world, &[("FIXUP_COUNT_CMD", "echo 3")]), &["7"]);
    assert_eq!(
        got.err,
        "maintainer-repo: the patch does not state 3, the count FIXUP_COUNT_CMD derives; a typed count is refused\n"
    );
    let count = world
        .rig
        .specs()
        .into_iter()
        .find(|s| s.argv.as_slice().first().map(String::as_str) == Some("env"))
        .unwrap();
    assert_eq!(count.cwd, Some(PathBuf::from(format!("{w}/r"))));
}

#[test]
fn with_no_count_command_a_test_count_item_is_refused_and_a_count_that_is_not_a_number_too() {
    for (command, message) in [
        (
            None,
            "this profile declares no FIXUP_COUNT_CMD, so a test count would be typed rather than derived",
        ),
        (
            Some("echo many"),
            "FIXUP_COUNT_CMD did not print a whole number, so no count could be derived",
        ),
    ] {
        let world = ready("test-count:docs.md", "diff\n+there are 3 tests\n");
        let w = world.work.clone();
        let patch = world.patch.to_string_lossy().into_owned();
        script(&world.rig, &g(&w, &["apply", "--check", &patch]), 0, "");
        script(&world.rig, &g(&w, &["apply", "--index", &patch]), 0, "");
        world.rig.ok(&["sort", "-u"], "test-count\n");
        world.rig.ok(
            &[
                "env",
                "-u",
                "GH_TOKEN",
                "-u",
                "GITHUB_TOKEN",
                "timeout",
                "120",
                "bash",
                "-c",
                "echo many",
            ],
            "many\n",
        );
        let extra: Vec<(&str, &str)> =
            command.map(|c| ("FIXUP_COUNT_CMD", c)).into_iter().collect();
        let got = run_fixup(&world, &tunables(&world, &extra), &["7"]);
        assert_eq!(got.err, format!("maintainer-repo: {message}\n"));
    }
}

#[test]
fn the_heads_up_dry_run_prints_the_body_indented_and_posts_nothing() {
    let scratch = Scratch::new("fixup-notice-dry");
    let rig = Rig::new();
    rig.ok(&check_argv("7"), &answer("notice", "typo:docs.md test-count:docs.md", "/p"));
    rig.ok(SWITCH, "");
    rig.ok(ME, "42\ttestuser\n");
    let settings = settings_with(&scratch.0, "/s", "on", &[]);
    let got =
        rig.run(&[], |ctx| cmd_fixup(ctx, &settings, &tree(), &args(&["7", "--dry-run"])).unwrap());
    assert_eq!(got.code, 0);
    let lines = got.out_lines();
    assert_eq!(lines[0], "  would post this heads-up on #7 (ripe after 2026-11-20, 5 days):");
    assert_eq!(
        lines[1],
        "    This has had no reply for 12 days since my review, and what is left is mechanical (typo, test-count)."
    );
    assert_eq!(lines[2], "    ");
    assert_eq!(*lines.last().unwrap(), "    <!-- maintainer-fixup-notice -->");
    assert!(rig.calls_starting(&["gh", "pr"]).is_empty());
}

/// A process port that reads the file named by `--body-file` when `gh pr comment` runs.
struct BodyReader<'a> {
    inner: &'a mx_ports::FakeProc,
    body: RefCell<Option<Vec<u8>>>,
}

impl Proc for BodyReader<'_> {
    fn run(&self, spec: &ProcSpec) -> Result<ProcOutput, ProcError> {
        let argv = spec.argv.as_slice();
        if argv.len() > 3 && argv[..3] == ["gh", "pr", "comment"] {
            let file = argv.last().unwrap();
            *self.body.borrow_mut() = std::fs::read(file).ok();
        }
        self.inner.run(spec)
    }
}

#[test]
fn a_real_heads_up_goes_through_a_temp_file_without_a_trailing_newline_which_is_removed_after() {
    let scratch = Scratch::new("fixup-notice");
    let temp = scratch.path("fixup-notice.AbCdEf");
    std::fs::write(&temp, "").unwrap();
    let state = scratch.0.to_string_lossy().into_owned();
    let rig = Rig::new();
    let first = answer("notice", "typo:docs.md", "/p");
    rig.sequence(&check_argv("7"), vec![output(0, &first, ""), output(0, "state=ready\n", "")]);
    rig.ok(SWITCH, "");
    rig.ok(ME, "42\ttestuser\n");
    rig.ok(&["mktemp", &format!("{state}/fixup-notice.XXXXXX")], &format!("{}\n", temp.display()));
    rig.ok(
        &["gh", "pr", "comment", "7", "--repo", "o/r", "--body-file", &temp.to_string_lossy()],
        "",
    );
    let reader = BodyReader { inner: &rig.proc, body: RefCell::new(None) };
    let settings = settings_with(&scratch.0, &state, "on", &[]);
    let got = Rig::run_over(&reader, &[], |ctx| {
        cmd_fixup(ctx, &settings, &tree(), &args(&["7"])).unwrap()
    });
    assert_eq!(got.code, 0);
    assert_eq!(got.out, "  posted the heads-up on #7; a fixup may follow after 2026-11-20\n");
    let body = String::from_utf8(reader.body.borrow().clone().unwrap()).unwrap();
    assert!(body.starts_with(
        "This has had no reply for 12 days since my review, and what is left is mechanical (typo)."
    ));
    assert!(body.ends_with("<!-- maintainer-fixup-notice -->"), "{body:?}");
    assert!(!temp.exists(), "the temp file is removed");
    assert_eq!(
        rig.calls_starting(&check_argv("7")).len(),
        2,
        "fixup-check is asked again to read it back"
    );
}

#[test]
fn a_heads_up_that_the_read_back_does_not_show_is_not_reported_as_told() {
    let scratch = Scratch::new("fixup-notice-unread");
    let temp = scratch.file("fixup-notice.Zz", "");
    let state = scratch.0.to_string_lossy().into_owned();
    let rig = Rig::new();
    let first = answer("notice", "typo:docs.md", "/p");
    rig.sequence(&check_argv("7"), vec![output(0, &first, ""), output(0, "state=notice\n", "")]);
    rig.ok(SWITCH, "");
    rig.ok(ME, "42\ttestuser\n");
    rig.ok(&["mktemp", &format!("{state}/fixup-notice.XXXXXX")], &format!("{}\n", temp.display()));
    rig.ok(
        &["gh", "pr", "comment", "7", "--repo", "o/r", "--body-file", &temp.to_string_lossy()],
        "",
    );
    let settings = settings_with(&scratch.0, &state, "on", &[]);
    let got = rig.run(&[], |ctx| cmd_fixup(ctx, &settings, &tree(), &args(&["7"])).unwrap_err().0);
    assert_eq!(
        got.err,
        "maintainer-repo: posted the heads-up but could not read it back on #7, so I cannot say the author was told\n"
    );
}

#[test]
fn a_comment_that_gh_refuses_is_could_not_post_and_the_temp_file_is_still_removed() {
    let scratch = Scratch::new("fixup-notice-fail");
    let temp = scratch.file("fixup-notice.Qq", "");
    let state = scratch.0.to_string_lossy().into_owned();
    let rig = Rig::new();
    rig.ok(&check_argv("7"), &answer("notice", "typo:docs.md", "/p"));
    rig.ok(SWITCH, "");
    rig.ok(ME, "42\ttestuser\n");
    rig.ok(&["mktemp", &format!("{state}/fixup-notice.XXXXXX")], &format!("{}\n", temp.display()));
    rig.reply(
        &["gh", "pr", "comment", "7", "--repo", "o/r", "--body-file", &temp.to_string_lossy()],
        1,
        "",
        "boom\n",
    );
    let settings = settings_with(&scratch.0, &state, "on", &[]);
    let got = rig.run(&[], |ctx| cmd_fixup(ctx, &settings, &tree(), &args(&["7"])).unwrap_err().0);
    assert_eq!(got.err, "maintainer-repo: could not post the heads-up on #7\n");
    assert!(!temp.exists());
}

fn readback_world(tries: &str, views: Vec<ProcOutput>) -> (World, Outcome) {
    let world = ready("typo:docs.md", TYPO_PATCH);
    commit_calls(&world, false);
    let w = world.work.clone();
    script(
        &world.rig,
        &g(&w, &["push", "-q", "https://github.com/alice/r.git", "HEAD:refs/heads/fix-1"]),
        0,
        "",
    );
    world.rig.sequence(
        &["gh", "pr", "view", "7", "--repo", "o/r", "--json", "headRefOid", "--jq", ".headRefOid"],
        views,
    );
    world.rig.ok(&["sleep", "0"], "");
    let got = run_fixup(&world, &tunables(&world, &[("GH_TRIES", tries)]), &["7"]);
    (world, got)
}

#[test]
fn a_head_that_github_does_not_report_is_not_confirmed_after_gh_tries_reads_with_a_sleep_between() {
    let (world, got) = readback_world("3", vec![output(0, "old\n", ""); 3]);
    assert_eq!(
        got.err,
        format!(
            "maintainer-repo: pushed {NEW} but #7 reports old as its head: NOT confirmed, look at it before anything else\n"
        )
    );
    assert_eq!(got.code, 1);
    assert_eq!(world.rig.calls_starting(&["gh", "pr", "view"]).len(), 3);
    assert_eq!(world.rig.calls_starting(&["sleep"]).len(), 2, "no sleep after the last read");
}

#[test]
fn an_empty_read_back_says_nothing_and_a_later_good_read_confirms() {
    let (world, got) =
        readback_world("2", vec![output(0, "", ""), output(0, &format!("{NEW}\n"), "")]);
    assert_eq!(got.code, 0, "{}", got.err);
    assert_eq!(world.rig.calls_starting(&["gh", "pr", "view"]).len(), 2);
    let (_, got) = readback_world("1", vec![output(0, "", "")]);
    assert!(got.err.contains("reports nothing as its head: NOT confirmed"), "{}", got.err);
}

#[test]
fn a_tries_setting_that_is_not_a_bash_integer_or_is_not_positive_runs_no_read_at_all() {
    for tries in ["abc", "0", "-1", ""] {
        let (world, got) = readback_world(tries, vec![]);
        assert!(
            got.err.contains("reports nothing as its head: NOT confirmed"),
            "{tries}: {}",
            got.err
        );
        assert!(world.rig.calls_starting(&["gh", "pr", "view"]).is_empty(), "{tries}");
    }
    let (world, _) = readback_world("010", vec![output(0, "x\n", ""); 8]);
    assert_eq!(
        world.rig.calls_starting(&["gh", "pr", "view"]).len(),
        8,
        "a leading zero is octal in bash arithmetic"
    );
    assert_eq!(
        world.rig.calls_starting(&["sleep"]).len(),
        8,
        "the test builtin reads 010 as ten, so the last read sleeps too"
    );
}
