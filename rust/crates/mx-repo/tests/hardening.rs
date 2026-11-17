//! What a host without `tr`, `sed`, `grep`, `awk`, `paste`, `wc` or `rm` does to the program,
//! and which children get a credential. The bash runs those tools as processes, so a host
//! that lacks one prints `SCRIPT: line N: TOOL: command not found` and carries on with an
//! empty answer; the port replaced the tools with code of its own and asks whether the tool
//! is there (decision P1H-16). The expected text of every test below is what the differential
//! (`tests/differential.sh`, the `notool` scenarios) measured on the bash program.

mod common;

use common::{Rig, Scratch, args, path_without, settings, settings_with};
use mx_repo::ctx::Call;
use mx_repo::prune::cmd_prune;
use mx_repo::release::cmd_release_check;

const SCRIPT: &str = "/tree/bin/maintainer-repo";

// ---- credentials ------------------------------------------------------------------------------

#[test]
fn a_token_reaches_gh_git_and_python3_and_no_other_child() {
    let vars = [
        ("GH_TOKEN", "t1"),
        ("GITHUB_TOKEN", "t2"),
        ("GH_ENTERPRISE_TOKEN", "t3"),
        ("GITHUB_ENTERPRISE_TOKEN", "t4"),
        ("GH_HOST", "github.com"),
    ];
    let rig = Rig::new();
    let programs = ["gh", "git", "python3", "mktemp", "sort", "sleep", "env", "timeout", "bash"];
    for program in programs {
        rig.ok(&[program], "");
    }
    let _ = rig.run(&vars, |ctx| {
        for program in programs {
            drop(ctx.exec(&Call::new([program])));
        }
        0
    });
    let specs = rig.specs();
    assert_eq!(specs.len(), programs.len(), "every call reached the process port");
    for spec in &specs {
        let program = spec.argv.as_slice()[0].as_str();
        let names: Vec<&str> = spec.env.iter().map(|(name, _)| name.as_str()).collect();
        let holds_token = names.iter().any(|name| name.ends_with("_TOKEN"));
        let may = ["gh", "git", "python3"].contains(&program);
        assert_eq!(holds_token, may, "{program} got {names:?}");
        if may {
            assert_eq!(
                names.iter().filter(|name| name.ends_with("_TOKEN")).count(),
                4,
                "{program}"
            );
        }
        assert!(names.contains(&"GH_HOST"), "{program}: a variable that is not a credential stays");
    }
}

// ---- the tool probe ---------------------------------------------------------------------------

#[test]
fn a_tool_is_present_when_an_executable_file_of_its_name_is_in_an_absolute_path_entry() {
    let scratch = Scratch::new("hard-probe");
    let path = path_without(&scratch, &["sed"]);
    let rig = Rig::new();
    let got = rig.run(&[("PATH", &format!("relative:{path}"))], |ctx| {
        i32::from(ctx.has_tool("tr")) * 100
            + i32::from(ctx.has_tool("sed")) * 10
            + i32::from(ctx.has_tool("nosuchtool"))
    });
    assert_eq!(got.code, 100, "tr is there, sed and nosuchtool are not");
    let not_executable = scratch.file("plain-dir-file", "");
    let rig = Rig::new();
    let got = rig.run(&[("PATH", &scratch.0.to_string_lossy())], |ctx| {
        i32::from(not_executable.exists() && ctx.has_tool("plain-dir-file"))
    });
    assert_eq!(got.code, 0, "a file without the execute bit is not a tool");
}

// ---- prune ------------------------------------------------------------------------------------

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

fn prune_rig(open: &str, refs: &str) -> Rig {
    let rig = Rig::new();
    rig.ok(FETCH, "");
    rig.ok(LIST, open);
    rig.ok(REMOTE_REFS, refs);
    rig.ok(LOCAL_REFS, "");
    for branch in ["feat", "other"] {
        rig.ok(
            &["git", "merge-base", "--is-ancestor", &format!("origin/{branch}"), "origin/main"],
            "",
        );
        rig.ok(&["git", "push", "-q", "origin", "--delete", branch], "");
    }
    rig.ok(&["git", "worktree", "prune"], "");
    rig.ok(&["git", "remote", "prune", "origin"], "");
    rig
}

#[test]
fn prune_without_tr_says_so_once_reads_no_open_heads_and_still_deletes() {
    let scratch = Scratch::new("hard-prune-tr");
    let path = path_without(&scratch, &["tr"]);
    let rig = prune_rig("feat\n", "main\nfeat\n");
    let got = rig.run(&[("PATH", &path)], |ctx| {
        cmd_prune(ctx, &settings(&scratch.0, "on"), &args(&[])).unwrap_or_else(|stop| stop.0)
    });
    assert_eq!(got.err, format!("{SCRIPT}: line 38: tr: command not found\n"));
    assert!(got.out.contains("  deleted feat\n"), "{}", got.out);
}

#[test]
fn prune_without_grep_says_so_for_each_unprotected_remote_branch_when_a_pull_request_is_open() {
    let scratch = Scratch::new("hard-prune-grep");
    let path = path_without(&scratch, &["grep"]);
    let rig = prune_rig("feat\n", "main\nfeat\nmaster\nHEAD\nother\n");
    let got = rig.run(&[("PATH", &path)], |ctx| {
        cmd_prune(ctx, &settings(&scratch.0, "on"), &args(&[])).unwrap_or_else(|stop| stop.0)
    });
    assert_eq!(
        got.err,
        format!("{SCRIPT}: line 45: grep: command not found\n").repeat(2),
        "feat and other, not main, master or HEAD"
    );
}

#[test]
fn prune_without_grep_says_nothing_when_no_pull_request_is_open() {
    let scratch = Scratch::new("hard-prune-grep-none");
    let path = path_without(&scratch, &["grep"]);
    let rig = prune_rig("", "main\nfeat\n");
    let got = rig.run(&[("PATH", &path)], |ctx| {
        cmd_prune(ctx, &settings(&scratch.0, "on"), &args(&[])).unwrap_or_else(|stop| stop.0)
    });
    assert_eq!(got.err, "", "the bash tests -n first, so grep never runs");
}

// ---- release-check ----------------------------------------------------------------------------

const FETCH_TAGS: &[&str] = &["git", "fetch", "-q", "--tags", "origin"];

fn release_rig(changelog: &str) -> (Rig, Scratch) {
    let scratch = Scratch::new("hard-release");
    scratch.file("CHANGELOG.md", changelog);
    let rig = Rig::new();
    rig.ok(FETCH_TAGS, "");
    rig.ok(&["git", "describe", "--tags", "--abbrev=0", "origin/main"], "v1.0.0\n");
    rig.ok(&["git", "rev-list", "--count", "v1.0.0..origin/main"], "2\n");
    rig.ok(&["git", "log", "--oneline", "v1.0.0..origin/main"], "bbb2222 second\naaa1111 first\n");
    rig.ok(
        &["git", "diff", "--name-only", "v1.0.0..origin/main", "--", "crates/*"],
        "crates/a\ncrates/b\n",
    );
    rig.ok(&["git", "rev-parse", "--abbrev-ref", "HEAD"], "main\n");
    rig.ok(&["git", "status", "--porcelain", "CHANGELOG.md"], "");
    rig.ok(&["git", "merge-base", "--is-ancestor", "HEAD", "origin/main"], "");
    rig.ok(&["git", "rev-list", "--count", "HEAD..origin/main"], "0\n");
    (rig, scratch)
}

fn release_out(missing: &[&str], changelog: &str) -> common::Outcome {
    let (rig, scratch) = release_rig(changelog);
    let path = path_without(&scratch, missing);
    let settings = settings_with(&scratch.0, "/state", "on", &[("PROD_GLOBS", "crates/*")]);
    rig.run(&[("PATH", &path)], |ctx| cmd_release_check(ctx, &settings).unwrap_or_else(|s| s.0))
}

const NOTES: &str = "## [Unreleased]\n### Security\n- it refuses now\n## [1.0.0]\n";

#[test]
fn release_check_without_wc_leaves_the_production_count_empty_and_that_is_not_zero() {
    let got = release_out(&["wc"], NOTES);
    assert_eq!(got.err, format!("{SCRIPT}: line 101: wc: command not found\n"));
    assert!(got.out.contains("  production files touched:  \n"), "{}", got.out);
    assert!(got.out.contains("verdict: RELEASE DUE."), "{}", got.out);
    assert!(got.out.contains("  WARNING: this profile declares no SHIPPED_GLOBS"), "{}", got.out);
}

#[test]
fn release_check_without_wc_drops_the_shipped_surface_lines_and_says_so_for_each_count() {
    let (rig, scratch) = release_rig(NOTES);
    let path = path_without(&scratch, &["wc"]);
    let settings = settings_with(
        &scratch.0,
        "/state",
        "on",
        &[("PROD_GLOBS", "crates/*"), ("SHIPPED_GLOBS", "crates/*")],
    );
    let got = rig
        .run(&[("PATH", &path)], |ctx| cmd_release_check(ctx, &settings).unwrap_or_else(|s| s.0));
    assert_eq!(
        got.err,
        format!(
            "{SCRIPT}: line 101: wc: command not found\n{SCRIPT}: line 119: wc: command not found\n"
        )
    );
    assert!(!got.out.contains("shipped surface touched"), "{}", got.out);
    assert!(!got.out.contains("WARNING: this profile declares no SHIPPED_GLOBS"), "{}", got.out);
}

#[test]
fn release_check_without_sed_prints_no_log_and_reads_an_empty_unreleased_section() {
    let got = release_out(&["sed"], NOTES);
    assert_eq!(got.err, format!("{SCRIPT}: line 83: sed: command not found\n"));
    assert!(!got.out.contains("bbb2222"), "{}", got.out);
    assert!(got.out.contains("  CHANGELOG Security section: no\n"), "{}", got.out);
    assert!(got.out.contains("  removed-capability wording: 0 occurrence(s)\n"), "{}", got.out);
    assert!(got.out.contains("  WARNING: the Unreleased section is empty"), "{}", got.out);
    assert!(
        got.out.contains("The newest version heading is '[1.0.0]'. If you have already"),
        "grep -m1 still reads the heading: {}",
        got.out
    );
}

#[test]
fn release_check_without_grep_leaves_every_count_empty_and_each_comparison_errors() {
    let got = release_out(&["grep"], NOTES);
    let want = [153, 154, 155, 158]
        .map(|line| format!("{SCRIPT}: line {line}: grep: command not found\n"))
        .concat()
        + &format!("{SCRIPT}: line 168: [: : integer expected\n")
        + &format!("{SCRIPT}: line 169: [: : integer expected\n")
        + &format!("{SCRIPT}: line 182: [: : integer expected\n").repeat(2)
        + &format!("{SCRIPT}: line 189: [: : integer expected\n").repeat(2);
    assert_eq!(got.err, want);
    assert!(got.out.contains("  removed-capability wording:  occurrence(s)\n"), "{}", got.out);
    assert!(
        got.out.contains("verdict: release worthwhile; production code changed."),
        "{}",
        got.out
    );
    assert!(got.out.contains("digit:   last."), "{}", got.out);
}

#[test]
fn release_check_without_tr_always_warns_that_the_unreleased_section_is_empty() {
    let got = release_out(&["tr"], NOTES);
    assert_eq!(got.err, format!("{SCRIPT}: line 171: tr: command not found\n"));
    assert!(got.out.contains("  CHANGELOG Security section: yes\n"), "{}", got.out);
    assert!(got.out.contains("  WARNING: the Unreleased section is empty"), "{}", got.out);
    assert!(got.out.contains("verdict: RELEASE DUE."), "{}", got.out);
}

#[test]
fn release_check_with_every_tool_prints_no_diagnostic() {
    let got = release_out(&[], NOTES);
    assert_eq!(got.err, "");
    assert!(got.out.contains("  CHANGELOG Security section: yes\n"), "{}", got.out);
    assert!(!got.out.contains("WARNING: the Unreleased section is empty"), "{}", got.out);
}
