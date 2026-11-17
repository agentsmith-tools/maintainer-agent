//! `release-check`: the CHANGELOG reading and the verdict as pure functions, and
//! the command over a scripted process fake.

mod common;

use common::{Rig, Scratch, settings, settings_with};
use mx_repo::release::{
    ChangelogFacts, Verdict, changelog_facts, closing_lines, cmd_release_check, digit_is_middle,
    unreleased_section, verdict,
};

fn facts(text: &str) -> ChangelogFacts {
    changelog_facts(Some(text.as_bytes()))
}

// ---- the Unreleased section -------------------------------------------------------------------

#[test]
fn the_unreleased_section_runs_from_its_heading_to_the_next_version_heading_and_drops_both() {
    let text = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- x\n\n## [1.0.0] - 2026-01-01\n\n### Security\n\n- old\n";
    assert_eq!(unreleased_section(text.as_bytes()), b"\n### Added\n\n- x");
}

#[test]
fn a_second_unreleased_heading_after_a_version_heading_opens_a_second_range() {
    // `sed -n '/start/,/end/p'` restarts after each range, so both sections count.
    let text =
        "## [Unreleased]\n- a\n## [1.0.0]\n- old\n## [Unreleased]\n- b\n## [0.9.0]\n- older\n";
    assert_eq!(unreleased_section(text.as_bytes()), b"- a\n- b");
}

#[test]
fn a_section_with_no_version_heading_after_it_runs_to_the_end_of_the_file() {
    assert_eq!(unreleased_section(b"## [Unreleased]\n- a\n- b\n"), b"- a\n- b");
}

#[test]
fn every_line_that_starts_with_two_hashes_and_a_bracket_is_dropped_even_inside_the_section() {
    assert_eq!(
        unreleased_section(b"## [Unreleased]\n- a\n## [Note] odd\n- b\n## [1.0.0]\n"),
        b"- a\n- b"
    );
}

#[test]
fn a_changelog_without_an_unreleased_heading_has_an_empty_section() {
    assert_eq!(unreleased_section(b"# Changelog\n## [1.0.0]\n- x\n"), b"");
    assert_eq!(unreleased_section(b""), b"");
}

#[test]
fn a_last_line_with_no_newline_is_still_read() {
    assert_eq!(unreleased_section(b"## [Unreleased]\n### Security"), b"### Security");
}

#[test]
fn the_unreleased_match_is_a_prefix_of_the_line_so_trailing_text_still_opens_it() {
    assert_eq!(unreleased_section(b"## [Unreleased] - soon\n- a\n"), b"- a");
    assert_eq!(unreleased_section(b" ## [Unreleased]\n- a\n"), b"");
}

// ---- the counts --------------------------------------------------------------------------------

#[test]
fn security_and_added_are_counted_as_lines_that_start_with_the_heading() {
    let f = facts(
        "## [Unreleased]\n### Security\n### Security notes\n### Addedum\n  ### Added\n### Added\n## [1.0.0]\n### Security\n",
    );
    assert_eq!(f.security, 2, "### Security and ### Security notes, not the one under 1.0.0");
    assert_eq!(f.added, 2, "### Addedum starts with the heading; the indented one does not");
}

#[test]
fn removed_capability_wording_is_counted_per_line_and_ignores_case() {
    let f = facts(
        "## [Unreleased]\n- Node 18 is no longer supported\n- a call that used to succeed now fails\n- used to work\n\
         - the gate refuses stale input\n- the gate REFUSES odd input\n- no longer and no longer\n- nothing here\n## [1.0.0]\n",
    );
    assert_eq!(f.breaking, 6, "the last line with two phrases counts once");
}

#[test]
fn the_wording_needs_the_whole_phrase() {
    let f = facts(
        "## [Unreleased]\n- used to  work\n- usedto work\n- no  longer\n- refuse\n## [1.0.0]\n",
    );
    assert_eq!(f.breaking, 0);
}

#[test]
fn a_section_of_blank_lines_is_empty_and_one_with_text_is_not() {
    assert!(facts("## [Unreleased]\n   \n\t\n\n## [1.0.0]\n").empty);
    assert!(!facts("## [Unreleased]\n- x\n## [1.0.0]\n").empty);
    assert!(changelog_facts(None).empty, "no CHANGELOG reads as an empty section");
}

#[test]
fn the_newest_version_heading_is_the_first_heading_with_a_digit_in_the_whole_file() {
    let f = facts("# C\n## [Unreleased]\n## [1.2.0] - 2026-02-02\n## [1.1.0]\n");
    assert_eq!(f.newest_heading.as_deref(), Some("## [1.2.0] - 2026-02-02"));
    assert_eq!(facts("## [Unreleased]\n").newest_heading, None);
    assert_eq!(changelog_facts(None).newest_heading, None);
}

// ---- the verdict -------------------------------------------------------------------------------

#[test]
fn the_verdict_reads_the_production_count_first() {
    assert_eq!(verdict(0, 0, 0), Verdict::DocsOnly);
    assert_eq!(
        verdict(0, 3, 3),
        Verdict::DocsOnly,
        "pins current behaviour: no production file, so a release is optional"
    );
    assert_eq!(verdict(2, 0, 0), Verdict::Worthwhile);
    assert_eq!(verdict(2, 1, 0), Verdict::ReleaseDue);
    assert_eq!(verdict(2, 0, 1), Verdict::ReleaseDue);
}

#[test]
fn the_digit_rule_does_not_read_the_production_count() {
    assert!(digit_is_middle(1, 0));
    assert!(digit_is_middle(0, 1));
    assert!(!digit_is_middle(0, 0));
}

#[test]
fn the_closing_lines_for_each_verdict_are_the_bash_s_text() {
    let none = facts("## [Unreleased]\n- x\n");
    assert_eq!(
        closing_lines(1, &none),
        [
            "verdict: release worthwhile; production code changed.",
            "digit:   last. Compatible additions and fixes.",
            "",
            "Cutting one is a human decision: crates.io and npm versions can never be",
            "replaced.",
        ]
    );
    let sec = facts("## [Unreleased]\n### Security\n");
    assert_eq!(
        closing_lines(1, &sec),
        [
            "verdict: RELEASE DUE. A fix that removes a capability, or a security",
            "         fix, is sitting unreleased, so every installed copy is still",
            "         affected.",
            "digit:   middle. Something that used to succeed now refuses, which",
            "         docs/release.md counts as a break in the 0.y series.",
            "",
            "Cutting one is a human decision: crates.io and npm versions can never be",
            "replaced.",
        ]
    );
    assert_eq!(closing_lines(0, &sec)[0], "verdict: docs and tests only. A release is optional.");
    assert_eq!(
        closing_lines(0, &sec)[1],
        "digit:   middle. Something that used to succeed now refuses, which"
    );
}

// ---- the command -------------------------------------------------------------------------------

const FETCH_TAGS: &[&str] = &["git", "fetch", "-q", "--tags", "origin"];
const DESCRIBE: &[&str] = &["git", "describe", "--tags", "--abbrev=0", "origin/main"];

#[test]
fn a_repository_with_no_tag_says_no_tags_after_the_fetch_and_exits_1() {
    let scratch = Scratch::new("rel-notag");
    let rig = Rig::new();
    rig.ok(FETCH_TAGS, "");
    rig.reply(DESCRIBE, 128, "", "fatal: No names found\n");
    let got = rig
        .run(&[], |ctx| cmd_release_check(ctx, &settings(&scratch.0, "on")).expect_err("stops").0);
    assert_eq!(got.code, 1);
    assert_eq!(got.out, "");
    assert_eq!(got.err, "maintainer-repo: no tags\n", "describe's own error goes to /dev/null");
}

#[test]
fn no_commits_since_the_tag_prints_three_lines_and_stops_before_the_log() {
    let scratch = Scratch::new("rel-nothing");
    let rig = Rig::new();
    rig.ok(FETCH_TAGS, "");
    rig.ok(DESCRIBE, "v1.0.0\n");
    rig.ok(&["git", "rev-list", "--count", "v1.0.0..origin/main"], "0\n");
    let got = rig.run(&[], |ctx| cmd_release_check(ctx, &settings(&scratch.0, "on")).unwrap());
    assert_eq!(got.code, 0);
    assert_eq!(
        got.out_lines(),
        ["  last tag: v1.0.0", "  commits since: 0", "  verdict: nothing to release"]
    );
    assert_eq!(rig.argvs().len(), 3);
}

#[test]
fn the_tag_fetch_is_not_checked_and_git_s_error_goes_to_standard_error() {
    let scratch = Scratch::new("rel-fetch");
    let rig = Rig::new();
    rig.reply(FETCH_TAGS, 128, "", "fatal: unreachable\n");
    rig.ok(DESCRIBE, "v1.0.0\n");
    rig.ok(&["git", "rev-list", "--count", "v1.0.0..origin/main"], "0\n");
    let got = rig.run(&[], |ctx| cmd_release_check(ctx, &settings(&scratch.0, "on")).unwrap());
    assert_eq!(got.code, 0);
    assert_eq!(got.err, "fatal: unreachable\n");
}

#[test]
fn a_failed_rev_list_leaves_the_count_empty_and_the_report_goes_on() {
    // Pins current behaviour: `n` is empty, `[ "$n" = 0 ]` is false, and the
    // report continues with `commits since: ` and nothing after it.
    let scratch = Scratch::new("rel-revlist");
    let rig = Rig::new();
    rig.ok(FETCH_TAGS, "");
    rig.ok(DESCRIBE, "v1.0.0\n");
    rig.reply(
        &["git", "rev-list", "--count", "v1.0.0..origin/main"],
        128,
        "",
        "fatal: bad range\n",
    );
    rig.ok(&["git", "log", "--oneline", "v1.0.0..origin/main"], "");
    rig.ok(&["git", "rev-parse", "--abbrev-ref", "HEAD"], "main\n");
    rig.ok(&["git", "status", "--porcelain", "CHANGELOG.md"], "");
    rig.ok(&["git", "merge-base", "--is-ancestor", "HEAD", "origin/main"], "");
    rig.ok(&["git", "rev-list", "--count", "HEAD..origin/main"], "0\n");
    let got = rig.run(&[], |ctx| cmd_release_check(ctx, &settings(&scratch.0, "on")).unwrap());
    assert_eq!(got.err, "fatal: bad range\n");
    assert_eq!(got.out_lines()[..2], ["  last tag: v1.0.0", "  commits since: "]);
}

/// A scripted history: two commits since v1.0.0.
fn history(rig: &Rig, prod: &str, shipped: &str) {
    rig.ok(FETCH_TAGS, "");
    rig.ok(DESCRIBE, "v1.0.0\n");
    rig.ok(&["git", "rev-list", "--count", "v1.0.0..origin/main"], "2\n");
    rig.ok(&["git", "log", "--oneline", "v1.0.0..origin/main"], "bbb2222 second\naaa1111 first\n");
    rig.ok(&["git", "diff", "--name-only", "v1.0.0..origin/main", "--", "crates/*/src/*"], prod);
    rig.ok(&["git", "diff", "--name-only", "v1.0.0..origin/main", "--", "crates/*"], shipped);
}

#[test]
fn the_full_report_prints_the_log_counts_sections_verdict_and_digit() {
    let scratch = Scratch::new("rel-full");
    scratch.file(
        "CHANGELOG.md",
        "# C\n\n## [Unreleased]\n\n### Added\n\n- a thing\n\n## [1.0.0] - 2026-01-01\n",
    );
    let rig = Rig::new();
    history(&rig, "crates/x/src/lib.rs\n", "crates/x/src/lib.rs\ncrates/y/tests/t.rs\n");
    rig.ok(&["git", "rev-parse", "--abbrev-ref", "HEAD"], "main\n");
    rig.ok(&["git", "status", "--porcelain", "CHANGELOG.md"], "");
    rig.ok(&["git", "merge-base", "--is-ancestor", "HEAD", "origin/main"], "");
    rig.ok(&["git", "rev-list", "--count", "HEAD..origin/main"], "0\n");
    let settings = settings_with(
        &scratch.0,
        "/state",
        "on",
        &[("PROD_GLOBS", "crates/*/src/*"), ("SHIPPED_GLOBS", "crates/*")],
    );
    let got = rig.run(&[], |ctx| cmd_release_check(ctx, &settings).unwrap());
    assert_eq!(got.code, 0);
    assert_eq!(
        got.out,
        "  last tag: v1.0.0\n  commits since: 2\n\n    bbb2222 second\n    aaa1111 first\n\n\
         \x20 CHANGELOG read from:        the working tree (main)\n\
         \x20 production files touched:  1\n  shipped surface touched:    2\n\
         \x20 CHANGELOG Security section: no\n  CHANGELOG Added section:    yes\n\
         \x20 removed-capability wording: 0 occurrence(s)\n\n\
         \x20 verdict: release worthwhile; production code changed.\n\
         \x20 digit:   last. Compatible additions and fixes.\n  \n\
         \x20 Cutting one is a human decision: crates.io and npm versions can never be\n  replaced.\n"
    );
}

#[test]
fn globs_reach_git_as_separate_words_after_a_double_dash_and_are_not_expanded() {
    let scratch = Scratch::new("rel-globs");
    scratch.file("CHANGELOG.md", "## [Unreleased]\n- x\n## [1.0.0]\n");
    let rig = Rig::new();
    rig.ok(FETCH_TAGS, "");
    rig.ok(DESCRIBE, "v1.0.0\n");
    rig.ok(&["git", "rev-list", "--count", "v1.0.0..origin/main"], "1\n");
    rig.ok(&["git", "log", "--oneline", "v1.0.0..origin/main"], "a1 x\n");
    rig.ok(&["git", "diff", "--name-only", "v1.0.0..origin/main", "--", "a/*", "b/*.rs"], "a/1\n");
    rig.ok(&["git", "diff", "--name-only", "v1.0.0..origin/main", "--", "c/*"], "");
    rig.ok(&["git", "rev-parse", "--abbrev-ref", "HEAD"], "main\n");
    rig.ok(&["git", "status", "--porcelain", "CHANGELOG.md"], "");
    rig.ok(&["git", "merge-base", "--is-ancestor", "HEAD", "origin/main"], "");
    rig.ok(&["git", "rev-list", "--count", "HEAD..origin/main"], "0\n");
    let settings = settings_with(
        &scratch.0,
        "/state",
        "on",
        &[("PROD_GLOBS", "  a/*\tb/*.rs  "), ("SHIPPED_GLOBS", "c/*")],
    );
    let got = rig.run(&[], |ctx| cmd_release_check(ctx, &settings).unwrap());
    assert!(got.out.contains("  production files touched:  1\n"), "{}", got.out);
    assert!(got.out.contains("  shipped surface touched:    0\n"), "{}", got.out);
    assert!(
        got.out.contains("nothing that installs has changed since v1.0.0, so a\n"),
        "{}",
        got.out
    );
}

#[test]
fn a_profile_with_no_globs_warns_and_counts_zero_without_asking_git() {
    let scratch = Scratch::new("rel-noglobs");
    scratch.file("CHANGELOG.md", "## [Unreleased]\n- x\n## [1.0.0]\n");
    let rig = Rig::new();
    rig.ok(FETCH_TAGS, "");
    rig.ok(DESCRIBE, "v1.0.0\n");
    rig.ok(&["git", "rev-list", "--count", "v1.0.0..origin/main"], "1\n");
    rig.ok(&["git", "log", "--oneline", "v1.0.0..origin/main"], "a1 x\n");
    rig.ok(&["git", "rev-parse", "--abbrev-ref", "HEAD"], "main\n");
    rig.ok(&["git", "status", "--porcelain", "CHANGELOG.md"], "");
    rig.ok(&["git", "merge-base", "--is-ancestor", "HEAD", "origin/main"], "");
    rig.ok(&["git", "rev-list", "--count", "HEAD..origin/main"], "0\n");
    let got = rig.run(&[], |ctx| cmd_release_check(ctx, &settings(&scratch.0, "on")).unwrap());
    assert!(
        got.out
            .contains("  WARNING: this profile declares no PROD_GLOBS, so the production-file\n")
    );
    assert!(
        got.out.contains(
            "  WARNING: this profile declares no SHIPPED_GLOBS, so nothing below can say\n"
        )
    );
    assert!(got.out.contains("  production files touched:  0\n"));
    assert!(!got.out.contains("shipped surface touched"));
    assert!(rig.calls_starting(&["git", "diff"]).is_empty());
}

#[test]
fn globs_come_from_the_caller_s_environment_when_the_profile_does_not_set_them() {
    let scratch = Scratch::new("rel-envglobs");
    scratch.file("CHANGELOG.md", "## [Unreleased]\n- x\n## [1.0.0]\n");
    let rig = Rig::new();
    rig.ok(FETCH_TAGS, "");
    rig.ok(DESCRIBE, "v1.0.0\n");
    rig.ok(&["git", "rev-list", "--count", "v1.0.0..origin/main"], "1\n");
    rig.ok(&["git", "log", "--oneline", "v1.0.0..origin/main"], "a1 x\n");
    rig.ok(&["git", "diff", "--name-only", "v1.0.0..origin/main", "--", "e/*"], "e/1\ne/2\n");
    rig.ok(&["git", "rev-parse", "--abbrev-ref", "HEAD"], "main\n");
    rig.ok(&["git", "status", "--porcelain", "CHANGELOG.md"], "");
    rig.ok(&["git", "merge-base", "--is-ancestor", "HEAD", "origin/main"], "");
    rig.ok(&["git", "rev-list", "--count", "HEAD..origin/main"], "0\n");
    let got = rig.run(&[("PROD_GLOBS", "e/*")], |ctx| {
        cmd_release_check(ctx, &settings(&scratch.0, "on")).unwrap()
    });
    assert!(got.out.contains("  production files touched:  2\n"), "{}", got.out);
}

#[test]
fn an_uncommitted_changelog_hides_the_behind_warning_because_the_notes_are_an_if_elif_chain() {
    let scratch = Scratch::new("rel-dirty");
    scratch.file("CHANGELOG.md", "## [Unreleased]\n- x\n## [1.0.0]\n");
    let rig = Rig::new();
    rig.ok(FETCH_TAGS, "");
    rig.ok(DESCRIBE, "v1.0.0\n");
    rig.ok(&["git", "rev-list", "--count", "v1.0.0..origin/main"], "1\n");
    rig.ok(&["git", "log", "--oneline", "v1.0.0..origin/main"], "a1 x\n");
    rig.ok(&["git", "rev-parse", "--abbrev-ref", "HEAD"], "main\n");
    rig.ok(&["git", "status", "--porcelain", "CHANGELOG.md"], " M CHANGELOG.md\n");
    let got = rig.run(&[], |ctx| cmd_release_check(ctx, &settings(&scratch.0, "on")).unwrap());
    assert!(got.out.contains(
        "                             uncommitted, so origin/main does not have it yet\n"
    ));
    assert!(
        rig.calls_starting(&["git", "merge-base"]).is_empty(),
        "no ancestry test after the uncommitted note"
    );
}

#[test]
fn a_head_that_is_not_an_ancestor_of_origin_main_says_the_counts_are_from_another_tree() {
    let scratch = Scratch::new("rel-ahead");
    scratch.file("CHANGELOG.md", "## [Unreleased]\n- x\n## [1.0.0]\n");
    let rig = Rig::new();
    rig.ok(FETCH_TAGS, "");
    rig.ok(DESCRIBE, "v1.0.0\n");
    rig.ok(&["git", "rev-list", "--count", "v1.0.0..origin/main"], "1\n");
    rig.ok(&["git", "log", "--oneline", "v1.0.0..origin/main"], "a1 x\n");
    rig.ok(&["git", "rev-parse", "--abbrev-ref", "HEAD"], "feat/x\n");
    rig.ok(&["git", "status", "--porcelain", "CHANGELOG.md"], "");
    rig.reply(&["git", "merge-base", "--is-ancestor", "HEAD", "origin/main"], 1, "", "");
    let got = rig.run(&[], |ctx| cmd_release_check(ctx, &settings(&scratch.0, "on")).unwrap());
    assert!(got.out.contains("  CHANGELOG read from:        the working tree (feat/x)\n"));
    assert!(got.out.contains(
        "                             HEAD is not on origin/main; the counts above are\n"
    ));
    assert!(!got.out.contains("BEHIND"));
}

#[test]
fn a_head_behind_origin_main_prints_the_three_line_warning_with_the_count() {
    let scratch = Scratch::new("rel-behind");
    scratch.file("CHANGELOG.md", "## [Unreleased]\n- x\n## [1.0.0]\n");
    let rig = Rig::new();
    rig.ok(FETCH_TAGS, "");
    rig.ok(DESCRIBE, "v1.0.0\n");
    rig.ok(&["git", "rev-list", "--count", "v1.0.0..origin/main"], "2\n");
    rig.ok(&["git", "log", "--oneline", "v1.0.0..origin/main"], "a1 x\n");
    rig.ok(&["git", "rev-parse", "--abbrev-ref", "HEAD"], "main\n");
    rig.ok(&["git", "status", "--porcelain", "CHANGELOG.md"], "");
    rig.ok(&["git", "merge-base", "--is-ancestor", "HEAD", "origin/main"], "");
    rig.ok(&["git", "rev-list", "--count", "HEAD..origin/main"], "2\n");
    let got = rig.run(&[], |ctx| cmd_release_check(ctx, &settings(&scratch.0, "on")).unwrap());
    for line in [
        "                             HEAD is 2 commit(s) BEHIND origin/main, so this\n",
        "                             is NOT the CHANGELOG you are about to tag. Pull, then\n",
        "                             run this again; the digit below may be wrong.\n",
    ] {
        assert!(got.out.contains(line), "{line:?} in {}", got.out);
    }
}

#[test]
fn an_empty_unreleased_section_warns_and_names_the_newest_version_heading() {
    let scratch = Scratch::new("rel-empty");
    scratch.file("CHANGELOG.md", "# C\n\n## [Unreleased]\n\n## [1.0.0] - 2026-01-01\n\n- first\n");
    let rig = Rig::new();
    rig.ok(FETCH_TAGS, "");
    rig.ok(DESCRIBE, "v1.0.0\n");
    rig.ok(&["git", "rev-list", "--count", "v1.0.0..origin/main"], "1\n");
    rig.ok(&["git", "log", "--oneline", "v1.0.0..origin/main"], "a1 x\n");
    rig.ok(&["git", "rev-parse", "--abbrev-ref", "HEAD"], "main\n");
    rig.ok(&["git", "status", "--porcelain", "CHANGELOG.md"], "");
    rig.ok(&["git", "merge-base", "--is-ancestor", "HEAD", "origin/main"], "");
    rig.ok(&["git", "rev-list", "--count", "HEAD..origin/main"], "0\n");
    let got = rig.run(&[], |ctx| cmd_release_check(ctx, &settings(&scratch.0, "on")).unwrap());
    assert!(
        got.out.contains(
            "           The newest version heading is '[1.0.0] - 2026-01-01'. If you have already\n"
        ),
        "{}",
        got.out
    );
    assert!(
        got.out.contains("           promoted Unreleased into it, read that section instead.\n")
    );
}

#[test]
fn a_head_exactly_one_commit_behind_origin_main_is_reported() {
    let scratch = Scratch::new("rel-behind-one");
    scratch.file("CHANGELOG.md", "## [Unreleased]\n- x\n## [1.0.0]\n");
    let rig = Rig::new();
    rig.ok(FETCH_TAGS, "");
    rig.ok(DESCRIBE, "v1.0.0\n");
    rig.ok(&["git", "rev-list", "--count", "v1.0.0..origin/main"], "1\n");
    rig.ok(&["git", "log", "--oneline", "v1.0.0..origin/main"], "a1 x\n");
    rig.ok(&["git", "rev-parse", "--abbrev-ref", "HEAD"], "main\n");
    rig.ok(&["git", "status", "--porcelain", "CHANGELOG.md"], "");
    rig.ok(&["git", "merge-base", "--is-ancestor", "HEAD", "origin/main"], "");
    rig.ok(&["git", "rev-list", "--count", "HEAD..origin/main"], "1\n");
    let got = rig.run(&[], |ctx| cmd_release_check(ctx, &settings(&scratch.0, "on")).unwrap());
    assert!(got.out.contains("HEAD is 1 commit(s) BEHIND origin/main, so this\n"), "{}", got.out);
}

use mx_repo::release::{MAX_LINE, changelog_facts_from};
use std::io::{BufReader, Cursor};

#[test]
fn reading_in_one_byte_chunks_gives_the_same_facts_as_reading_the_whole_text() {
    let text = "# C\n## [Unreleased]\n### Security\n- no longer works\n### Added\n\n## [1.0.0]\n- old\n## [Unreleased]\n- x\n";
    let tiny = changelog_facts_from(BufReader::with_capacity(1, Cursor::new(text.as_bytes())));
    assert_eq!(tiny, facts(text));
    assert_eq!((tiny.security, tiny.added, tiny.breaking, tiny.empty), (1, 1, 1, false));
}

#[test]
fn a_line_longer_than_the_buffer_cap_is_cut_and_its_start_still_counts() {
    // Decision P1H-15: a line is kept to MAX_LINE bytes. The heading at its start
    // still counts, a phrase after the cap does not, and the next line is read.
    let mut text = b"## [Unreleased]\n### Security".to_vec();
    text.extend(std::iter::repeat_n(b'x', MAX_LINE + 10));
    text.extend_from_slice(b" no longer\n### Added\n## [1.0.0]\n");
    let got = changelog_facts_from(BufReader::with_capacity(4096, Cursor::new(text)));
    assert_eq!((got.security, got.added, got.breaking), (1, 1, 0));
}

#[test]
fn a_very_long_changelog_is_scanned_line_by_line() {
    let mut text = String::from("## [Unreleased]\n");
    for n in 0..200_000 {
        text.push_str(if n % 50_000 == 0 { "### Security\n" } else { "- entry\n" });
    }
    text.push_str("## [1.0.0]\n");
    let got = changelog_facts_from(BufReader::with_capacity(8192, Cursor::new(text.into_bytes())));
    assert_eq!(got.security, 4);
    assert!(!got.empty);
}

/// Peak resident memory of this test process, in KiB, from `VmHWM`.
fn peak_rss_kib() -> u64 {
    let status =
        std::fs::read_to_string("/proc/self/status").expect("/proc/self/status is readable");
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("VmHWM is in /proc/self/status")
}

#[test]
fn a_changelog_of_a_quarter_gigabyte_is_read_in_a_bounded_buffer_not_loaded_whole() {
    // Lead 1 of the review of this PR: the bash reads the file with sed and grep line by line,
    // so its memory does not grow with the file. The file is sparse, so it costs no disk.
    use std::io::Write;
    const SIZE: u64 = 256 << 20;
    let scratch = Scratch::new("rel-huge");
    let path = scratch.path("CHANGELOG.md");
    {
        let mut file = std::fs::File::create(&path).expect("the changelog can be made");
        file.write_all(b"## [Unreleased]\n### Security\nit refuses now\n").expect("write");
        let head = file.metadata().expect("metadata").len();
        assert!(head < 100, "the head is {head} bytes");
        file.set_len(SIZE).expect("the file can be extended");
    }
    assert_eq!(std::fs::metadata(&path).expect("stat").len(), SIZE, "the file is 256 MiB");
    let rig = Rig::new();
    history(&rig, "", "");
    rig.ok(&["git", "rev-parse", "--abbrev-ref", "HEAD"], "main\n");
    rig.ok(&["git", "status", "--porcelain", "CHANGELOG.md"], "");
    rig.ok(&["git", "merge-base", "--is-ancestor", "HEAD", "origin/main"], "");
    rig.ok(&["git", "rev-list", "--count", "HEAD..origin/main"], "0\n");
    let settings = settings_with(&scratch.0, "/state", "on", &[("PROD_GLOBS", "crates/*/src/*")]);
    let before = peak_rss_kib();
    let got = rig.run(&[], |ctx| cmd_release_check(ctx, &settings).unwrap());
    let grown = peak_rss_kib().saturating_sub(before);
    assert_eq!(got.code, 0);
    assert!(got.out.contains("CHANGELOG Security section: yes"), "the head was read: {}", got.out);
    assert!(
        got.out.contains("removed-capability wording: 1 occurrence(s)"),
        "the head was read: {}",
        got.out
    );
    assert!(grown < 96 * 1024, "peak memory grew by {grown} KiB reading a 256 MiB file");
}
