//! `verify-action-pins` over fakes: which files it reads, what it asks GitHub, what it prints.

mod common;

use common::{Rig, exited, ok};
use mx_checks::ctx::Ctx;
use mx_checks::pins::{self, FakeTagRefs, GhTagRefs};
use std::ffi::OsString;

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SCRIPT: &str = "/t/scripts/verify-action-pins.sh";
const ALL_OK: &str = "  every pin is the tag it claims";
const SOME_BAD: &str = "  a pin does not match its comment";

fn workflow(rig: &Rig, name: &str, lines: &[String]) {
    rig.host.put_file(format!("/t/.github/workflows/{name}"), format!("{}\n", lines.join("\n")));
}

fn run(rig: &Rig, refs: &FakeTagRefs, args: &[&str]) -> common::Outcome {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    rig.run(SCRIPT, &[], |ctx| pins::run(ctx, refs, &args))
}

fn ok_line(action: &str, sha: &str, claim: &str) -> String {
    format!("  ok      {action:<46} {} is {claim}", &sha[..8])
}

#[test]
fn a_pin_on_a_lightweight_tag_passes_after_two_questions() {
    let rig = Rig::new();
    workflow(&rig, "ci.yml", &[format!("      - uses: actions/checkout@{A} # v4")]);
    let refs = FakeTagRefs::new().lightweight("actions/checkout", "v4", A);
    let outcome = run(&rig, &refs, &[]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert_eq!(outcome.out_lines(), [ok_line("actions/checkout", A, "v4"), ALL_OK.to_string()]);
    assert_eq!(outcome.err, "");
    assert_eq!(refs.calls(), ["sha actions/checkout v4", "type actions/checkout v4"]);
}

#[test]
fn a_pin_on_an_annotated_tag_is_followed_to_the_commit() {
    let rig = Rig::new();
    workflow(&rig, "ci.yml", &[format!("      - uses: actions/checkout@{A} # v4")]);
    let refs = FakeTagRefs::new().annotated("actions/checkout", "v4", "OBJ", A);
    let outcome = run(&rig, &refs, &[]);
    assert_eq!(outcome.code, 0);
    assert_eq!(
        refs.calls(),
        ["sha actions/checkout v4", "type actions/checkout v4", "object actions/checkout OBJ"]
    );
}

#[test]
fn a_mismatch_prints_its_line_on_stdout_the_verdict_on_stderr_and_exits_one() {
    let rig = Rig::new();
    workflow(&rig, "ci.yml", &[format!("      - uses: actions/checkout@{A} # v4")]);
    let refs = FakeTagRefs::new().lightweight("actions/checkout", "v4", B);
    let outcome = run(&rig, &refs, &[]);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        outcome.out,
        format!("  FAIL    {:<46} pinned aaaaaaaa but v4 is bbbbbbbb\n", "actions/checkout")
    );
    assert_eq!(outcome.err, format!("{SOME_BAD}\n"));
}

#[test]
fn a_floating_tag_is_named_and_asks_github_nothing() {
    let rig = Rig::new();
    workflow(&rig, "x.yml", &["      - uses: actions/deploy-pages@v5".to_string()]);
    let refs = FakeTagRefs::new();
    let outcome = run(&rig, &refs, &[]);
    assert_eq!(outcome.code, 1);
    assert_eq!(outcome.out, "");
    assert_eq!(
        outcome.err_lines(),
        [
            "  FAIL    not SHA-pinned with a version comment, so nothing below checks them:",
            "            uses: actions/deploy-pages@v5",
            SOME_BAD
        ]
    );
    assert!(refs.calls().is_empty());
}

#[test]
fn an_unparsed_line_fails_the_run_while_the_good_pins_beside_it_are_still_checked() {
    let rig = Rig::new();
    workflow(
        &rig,
        "x.yml",
        &[
            "      - uses: actions/deploy-pages@v5".to_string(),
            format!("      - uses: actions/checkout@{A} # v4"),
        ],
    );
    let refs = FakeTagRefs::new().lightweight("actions/checkout", "v4", A);
    let outcome = run(&rig, &refs, &[]);
    assert_eq!(outcome.code, 1);
    assert_eq!(outcome.out_lines(), [ok_line("actions/checkout", A, "v4")]);
    assert_eq!(outcome.err_lines().last(), Some(&SOME_BAD));
}

#[test]
fn pins_are_checked_in_byte_order_and_the_same_pin_in_two_files_once() {
    let rig = Rig::new();
    workflow(
        &rig,
        "a.yml",
        &[format!("      - uses: zeta/a@{A} # v1"), format!("      - uses: alpha/c@{B} # v1")],
    );
    workflow(
        &rig,
        "b.yml",
        &[format!("  - uses: zeta/a@{A} # v1"), format!("      - uses: Alpha/b@{A} # v1")],
    );
    let refs = FakeTagRefs::new()
        .lightweight("zeta/a", "v1", A)
        .lightweight("alpha/c", "v1", B)
        .lightweight("Alpha/b", "v1", A);
    let outcome = run(&rig, &refs, &[]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert_eq!(
        refs.calls().iter().filter(|call| call.starts_with("sha ")).collect::<Vec<_>>(),
        ["sha Alpha/b v1", "sha alpha/c v1", "sha zeta/a v1"]
    );
}

#[test]
fn a_branch_pin_is_reported_by_design_and_asks_nothing() {
    let rig = Rig::new();
    workflow(&rig, "x.yml", &[format!("      - uses: acme/rolling@{A} # main")]);
    let refs = FakeTagRefs::new();
    let outcome = run(&rig, &refs, &[]);
    assert_eq!(outcome.code, 0);
    assert_eq!(
        outcome.out_lines()[0],
        format!("  branch  {:<46} tracks main by design", "acme/rolling")
    );
    assert!(refs.calls().is_empty());
}

#[test]
fn a_tag_that_is_not_there_prints_the_not_a_tag_line() {
    let rig = Rig::new();
    workflow(&rig, "x.yml", &[format!("      - uses: acme/one@{A} # v9")]);
    let refs = FakeTagRefs::new().missing("acme/one", "v9");
    let outcome = run(&rig, &refs, &[]);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        outcome.out,
        format!("  FAIL    {:<46} claims v9, which is not a tag in acme/one\n", "acme/one")
    );
}

#[test]
fn no_workflows_directory_exits_two_naming_the_root() {
    let rig = Rig::new();
    let outcome = run(&rig, &FakeTagRefs::new(), &[]);
    assert_eq!(outcome.code, 2);
    assert_eq!(outcome.err, "verify-action-pins: no .github/workflows under /t\n");
    assert_eq!(outcome.out, "");
}

#[test]
fn a_workflows_directory_with_no_yaml_exits_two_saying_nothing_was_scanned() {
    let rig = Rig::new();
    rig.host.put_file("/t/.github/workflows/notes.txt", "text");
    let outcome = run(&rig, &FakeTagRefs::new(), &[]);
    assert_eq!(outcome.code, 2);
    assert_eq!(
        outcome.err,
        "verify-action-pins: no workflow files under /t/.github/workflows, so nothing was scanned\n"
    );
}

#[test]
fn an_actions_directory_with_no_action_yml_exits_two() {
    let rig = Rig::new();
    workflow(&rig, "a.yml", &["      - run: echo hi".to_string()]);
    rig.host.put_file("/t/.github/actions/readme.md", "text");
    let outcome = run(&rig, &FakeTagRefs::new(), &[]);
    assert_eq!(outcome.code, 2);
    assert_eq!(
        outcome.err,
        "verify-action-pins: /t/.github/actions exists and holds no action.yml, so the composite scan read nothing\n"
    );
}

#[test]
fn the_messages_repeat_the_root_as_given() {
    let rig = Rig::new();
    rig.host.put_dir("/w/../tree/.github/workflows");
    let outcome = run(&rig, &FakeTagRefs::new(), &["../tree/"]);
    assert_eq!(outcome.code, 2);
    assert_eq!(outcome.err, "verify-action-pins: no .github/workflows under ../tree/\n");
}

#[test]
fn a_root_argument_replaces_the_scripts_own_tree_and_an_empty_one_does_not() {
    let rig = Rig::new();
    rig.host
        .put_file("/other/.github/workflows/a.yml", format!("      - uses: acme/one@{A} # v1\n"));
    workflow(&rig, "a.yml", &["      - uses: acme/own@v1".to_string()]);
    let refs = FakeTagRefs::new().lightweight("acme/one", "v1", A);
    let outcome = run(&rig, &refs, &["/other"]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    let own = run(&Rig::new(), &FakeTagRefs::new(), &[""]);
    assert_eq!(own.err, "verify-action-pins: no .github/workflows under /t\n");
}

#[test]
fn only_yml_and_yaml_files_directly_under_workflows_are_workflows() {
    let rig = Rig::new();
    rig.host.put_file("/t/.github/workflows/a.yaml", format!("      - uses: acme/yaml@{A} # v1\n"));
    rig.host.put_file("/t/.github/workflows/b.yml.bak", "      - uses: acme/bak@v1\n");
    rig.host.put_file("/t/.github/workflows/sub/c.yml", "      - uses: acme/deep@v1\n");
    let refs = FakeTagRefs::new().lightweight("acme/yaml", "v1", A);
    let outcome = run(&rig, &refs, &[]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
}

#[test]
fn a_workflow_reached_through_a_link_is_not_a_file_for_find() {
    let rig = Rig::new();
    rig.host.put_file("/elsewhere/real.yml", "      - uses: acme/linked@v1\n");
    rig.host.put_dir("/t/.github/workflows");
    rig.host.put_link("/t/.github/workflows/link.yml", "/elsewhere/real.yml");
    let outcome = run(&rig, &FakeTagRefs::new(), &[]);
    assert_eq!(outcome.code, 2, "{outcome:?}");
}

#[test]
fn composite_actions_at_any_depth_and_one_level_of_examples_are_scanned() {
    let rig = Rig::new();
    workflow(&rig, "a.yml", &["      - run: echo hi".to_string()]);
    rig.host.put_file(
        "/t/.github/actions/a/b/action.yaml",
        format!("    - uses: acme/inner@{A} # v1\n"),
    );
    rig.host.put_file("/t/.github/actions/a/README.md", "    - uses: acme/ignored@v1\n");
    rig.host.put_file("/t/examples/one.yml", format!("      - uses: acme/example@{B} # v2\n"));
    rig.host.put_file("/t/examples/two.yaml", "      - uses: acme/yaml-example@v1\n");
    rig.host.put_file("/t/examples/sub/three.yml", "      - uses: acme/deep-example@v1\n");
    let refs =
        FakeTagRefs::new().lightweight("acme/inner", "v1", A).lightweight("acme/example", "v2", B);
    let outcome = run(&rig, &refs, &[]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert_eq!(outcome.out_lines().len(), 3);
}

#[test]
fn a_workflow_with_no_uses_passes_and_asks_nothing() {
    let rig = Rig::new();
    workflow(&rig, "x.yml", &["      - run: echo hi".to_string()]);
    let refs = FakeTagRefs::new();
    let outcome = run(&rig, &refs, &[]);
    assert_eq!(
        (outcome.code, outcome.out.as_str(), outcome.err.as_str()),
        (0, "  every pin is the tag it claims\n", "")
    );
    assert!(refs.calls().is_empty());
}

#[test]
fn an_unreadable_file_is_skipped_without_a_word() {
    let rig = Rig::new();
    workflow(&rig, "a.yml", &["      - run: echo hi".to_string()]);
    rig.host.put_file("/t/.github/workflows/b.yml", "      - uses: acme/hidden@v1\n");
    rig.host.make_unreadable("/t/.github/workflows/b.yml", "Permission denied");
    let outcome = run(&rig, &FakeTagRefs::new(), &[]);
    assert_eq!((outcome.code, outcome.err.as_str()), (0, ""));
}

// --- the real adapter, over a fake process --------------------------------------------------------------

fn with_gh<T>(rig: &Rig, body: impl FnOnce(&Ctx<'_>) -> T) -> T {
    let vars = std::collections::BTreeMap::new();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let ctx = Ctx::new(
        &rig.proc,
        &rig.host,
        SCRIPT.into(),
        "/x".into(),
        vars,
        "/w".into(),
        &mut out,
        &mut err,
    );
    body(&ctx)
}

#[test]
fn the_real_adapter_asks_gh_api_with_the_scripts_arguments() {
    use mx_checks::pins::TagRefs;
    let rig = Rig::new();
    rig.always(&["gh", "api", "repos/a/b/git/ref/tags/v1", "--jq", ".object.sha"], ok("OBJ\n"));
    rig.always(&["gh", "api", "repos/a/b/git/ref/tags/v1", "--jq", ".object.type"], ok("tag\n"));
    rig.always(&["gh", "api", "repos/a/b/git/tags/OBJ", "--jq", ".object.sha"], ok("COMMIT\n"));
    with_gh(&rig, |ctx| {
        assert_eq!(GhTagRefs.ref_sha(ctx, b"a/b", b"v1"), Some(b"OBJ".to_vec()));
        assert_eq!(GhTagRefs.ref_type(ctx, b"a/b", b"v1"), b"tag");
        assert_eq!(GhTagRefs.tag_object_sha(ctx, b"a/b", b"OBJ"), b"COMMIT");
    });
    assert_eq!(rig.calls().len(), 3);
}

#[test]
fn the_real_adapter_reads_a_failed_first_call_as_none_and_uses_the_last_calls_output_whatever_its_status()
 {
    use mx_checks::pins::TagRefs;
    let rig = Rig::new();
    rig.always(
        &["gh", "api", "repos/a/b/git/ref/tags/v9", "--jq", ".object.sha"],
        exited(1, "{\"message\":\"Not Found\"}\n", "HTTP 404"),
    );
    rig.always(
        &["gh", "api", "repos/a/b/git/tags/OBJ", "--jq", ".object.sha"],
        exited(1, "partial\n", "boom"),
    );
    with_gh(&rig, |ctx| {
        assert_eq!(GhTagRefs.ref_sha(ctx, b"a/b", b"v9"), None);
        assert_eq!(GhTagRefs.tag_object_sha(ctx, b"a/b", b"OBJ"), b"partial");
    });
}

#[test]
fn the_real_adapter_with_no_gh_answers_nothing() {
    use mx_checks::pins::TagRefs;
    let rig = Rig::new();
    rig.proc.script(
        &["gh", "api", "repos/a/b/git/ref/tags/v1", "--jq", ".object.sha"],
        mx_ports::Script::always(Err(mx_ports::ProcError::NotFound { program: "gh".to_string() })),
    );
    with_gh(&rig, |ctx| assert_eq!(GhTagRefs.ref_sha(ctx, b"a/b", b"v1"), None));
}

#[test]
fn a_workflow_with_a_nul_byte_is_binary_to_grep_and_contributes_no_line() {
    let rig = Rig::new();
    workflow(&rig, "ok.yml", &[format!("      - uses: acme/one@{A} # v1")]);
    rig.host.put_file(
        "/t/.github/workflows/binary.yml",
        format!("\0      - uses: acme/two@{B} # v2\n      - uses: acme/floating@v1\n"),
    );
    let refs = FakeTagRefs::new().lightweight("acme/one", "v1", A);
    let outcome = run(&rig, &refs, &[]);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert_eq!(outcome.out_lines(), [ok_line("acme/one", A, "v1"), ALL_OK.to_string()]);
    assert_eq!(outcome.err, "");
}
