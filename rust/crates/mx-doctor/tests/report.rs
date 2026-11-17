//! What the report prints, byte for byte.

use mx_doctor::report::{Item, Report};

#[test]
fn each_kind_of_line_prints_its_colour_its_label_and_its_four_space_gap() {
    assert_eq!(Report::render_item(&Item::head("Backend")), "\n\x1b[1mBackend\x1b[0m\n");
    assert_eq!(Report::render_item(&Item::ok("fine")), "  \x1b[32mok\x1b[0m    fine\n");
    assert_eq!(Report::render_item(&Item::warn("hmm", "")), "  \x1b[33mwarn\x1b[0m  hmm\n");
    assert_eq!(Report::render_item(&Item::fail("bad", "")), "  \x1b[31mFAIL\x1b[0m  bad\n");
}

#[test]
fn a_fix_line_follows_a_finding_only_when_the_fix_is_not_empty() {
    assert_eq!(
        Report::render_item(&Item::warn("hmm", "do this")),
        "  \x1b[33mwarn\x1b[0m  hmm\n        fix: do this\n"
    );
    assert_eq!(
        Report::render_item(&Item::fail("bad", "fix it")),
        "  \x1b[31mFAIL\x1b[0m  bad\n        fix: fix it\n"
    );
}

#[test]
fn the_summary_counts_findings_but_not_headings_and_exit_code_follows_failures_only() {
    let mut report = Report::new();
    report.push(Item::head("A"));
    report.extend([
        Item::ok("1"),
        Item::ok("2"),
        Item::warn("w", ""),
        Item::fail("f", ""),
        Item::fail("g", "x"),
    ]);
    assert_eq!(report.counts(), (2, 1, 2));
    assert_eq!(report.render_summary(), "\n  \x1b[1m2 ok, 1 warning(s), 2 failure(s)\x1b[0m\n");
    assert_eq!(report.exit_code(), 1);
    let mut calm = Report::new();
    calm.push(Item::warn("w", ""));
    assert_eq!(calm.exit_code(), 0);
}
