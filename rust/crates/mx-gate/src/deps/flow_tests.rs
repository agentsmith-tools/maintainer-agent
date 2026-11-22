//! Behaviour of the `verify-deps` flow, against the fakes of every port.
//!
//! A port nobody scripted panics when called, so a test that expects a refusal before some
//! call proves the call was not made by not scripting it.

use super::*;
use crate::deps::{GitObject, NetFailure, TagRef};
use crate::{
    CommitId, FakeGh, FakeHelper, FakeSay, GhFailure, HeadArg, HelperMissing, LeaseAnswer, PrNumber,
};
use mx_ports::{EffectLog, FakeFs, Script};
use mx_receipt::{DependencyFacts, Stamp};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const MERGE_BASE: &str = "cccccccccccccccccccccccccccccccccccccccc";
const PIN_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DIGEST: &str = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn pr() -> PrNumber {
    PrNumber::parse("7").unwrap()
}

fn fail(text: &str) -> NetFailure {
    NetFailure { output: text.to_string() }
}

fn pin(action: &str, tag: &str, branch: bool) -> Value {
    json!({"action": action, "sha": PIN_SHA, "tag": tag, "branch": branch})
}

fn image(name: &str, tag: &str) -> Value {
    json!({"image": name, "tag": tag, "digest": DIGEST, "same_tag": true})
}

fn package(path: &str, name: &str, version: &str, hashes: &[&str]) -> Value {
    json!({"path": path, "name": name, "version": version, "hashes": hashes})
}

fn bump(files: &Value, pins: &[Value], images: &[Value], pip: &[Value]) -> String {
    json!({
        "verdict": "dependency-bump", "files": files, "changed_lines": 6,
        "pins": pins, "images": images, "pip": pip,
    })
    .to_string()
}

fn lockfile() -> String {
    bump(&json!({"Cargo.lock": "cargo-lock"}), &[], &[], &[])
}

fn scripts() -> ScriptPaths {
    ScriptPaths {
        classifier: "/s/classify.py".into(),
        image_resolver: "/s/resolve.py".into(),
        pypi_verifier: "/s/pypi.py".into(),
        pip_files: "/s/pipfiles.py".into(),
    }
}

/// Every port of the flow, with the answers of a pull request whose bump is `verdict`.
struct Rig {
    gh: FakeGh,
    net: FakeNet,
    classifier: FakeClassifier,
    whole: FakeWholeFile,
    fs: FakeFs,
    say: FakeSay,
    log: EffectLog,
    config: DepsConfig,
}

impl Rig {
    fn new(verdict: &str) -> Self {
        let log = EffectLog::new();
        let fs = FakeFs::new(&log);
        for path in ["/s/classify.py", "/s/resolve.py", "/s/pypi.py", "/s/pipfiles.py"] {
            fs.put(path, "# script\n", 1);
        }
        let gh = FakeGh::new();
        gh.script_head(Script::always(Ok(HEAD.to_string())));
        gh.script_diff(Script::always(Ok("diff --git a/Cargo.lock b/Cargo.lock\n".to_string())));
        let classifier = FakeClassifier::new();
        classifier.script(Script::always(Ok(ClassifierRun {
            exit: ClassifierExit::Code(0),
            stdout: format!("{verdict}\n"),
        })));
        Self {
            gh,
            net: FakeNet::new(),
            classifier,
            whole: FakeWholeFile::new(),
            fs,
            say: FakeSay::new(),
            log,
            config: DepsConfig {
                repo_slug: "o/r".into(),
                scripts: scripts(),
                limit: ClassifyLimit::parse("60"),
            },
        }
    }

    fn run_as(&self, sha: &str) -> Result<DepsVerified, DepsRefusal> {
        let ports = DepsPorts {
            gh: &self.gh,
            net: &self.net,
            classifier: &self.classifier,
            whole_file: &self.whole,
            fs: &self.fs,
            say: &self.say,
        };
        let helper = FakeHelper::new();
        helper.script_lease(Script::always(Ok(LeaseAnswer::Free)));
        let lease = lease_gate(&helper, &pr()).unwrap();
        verify_deps(&self.config, &ports, &HeadArg::parse(sha).unwrap(), &lease)
    }

    fn run(&self) -> Result<DepsVerified, DepsRefusal> {
        self.run_as("0123456")
    }

    fn refusal(&self) -> DepsRefusal {
        self.run().expect_err("the checks should have refused")
    }

    fn said(&self) -> String {
        self.say.stdout()
    }
}

fn tag_ref(tag: &str, object_type: &str, sha: &str) -> TagRef {
    TagRef {
        name: format!("refs/tags/{tag}"),
        object_type: object_type.to_string(),
        object_sha: sha.to_string(),
    }
}

fn workflow_rig(pins: &[Value]) -> Rig {
    Rig::new(&bump(&json!({".github/workflows/ci.yml": "workflow"}), pins, &[], &[]))
}

// ----- the head, the diff and the classifier -----

#[test]
fn a_lockfile_bump_earns_facts_at_the_head_github_reports() {
    let rig = Rig::new(&lockfile());
    let verified = rig.run().unwrap();
    assert_eq!(verified.head().as_str(), HEAD);
    assert_eq!(verified.pr(), &pr());
    assert_eq!(
        verified.facts(),
        &DependencyFacts {
            files: "Cargo.lock (cargo-lock)".into(),
            changed_lines: 6,
            pins_verified: 0,
            images_verified: 0,
            pip_verified: 0
        }
    );
    assert_eq!(rig.said(), "");
}

#[test]
fn the_receipt_records_the_full_head_and_not_the_seven_characters_typed() {
    let rig = Rig::new(&lockfile());
    let receipt = rig.run_as("0123456").unwrap().receipt(&Stamp::new("t", "u"));
    assert_eq!(receipt.body().head, HEAD);
}

#[test]
fn the_classifier_runs_on_the_diff_with_exactly_one_trailing_newline_and_the_limit() {
    let rig = Rig::new(&lockfile());
    rig.gh.script_diff(Script::always(Ok("diff --git a/x b/x\n+y\n\n\n".to_string())));
    rig.run().unwrap();
    assert_eq!(
        rig.classifier.calls(),
        vec![ClassifierCall {
            diff: "diff --git a/x b/x\n+y\n".into(),
            limit: Duration::from_mins(1)
        }]
    );
}

#[test]
fn a_missing_classifier_refuses_before_gh_is_asked_anything() {
    let mut rig = Rig::new(&lockfile());
    rig.config.scripts.classifier = "/s/gone.py".into();
    assert_eq!(rig.refusal(), DepsRefusal::NoClassifier { classifier: "/s/gone.py".into() });
    assert!(rig.gh.calls().is_empty());
}

#[test]
fn a_directory_where_the_classifier_should_be_is_a_missing_classifier() {
    let rig = Rig::new(&lockfile());
    rig.fs.make_unreadable("/s/classify.py", "Is a directory");
    assert!(matches!(rig.refusal(), DepsRefusal::NoClassifier { .. }));
}

#[test]
fn a_head_that_could_not_be_read_refuses() {
    let rig = Rig::new(&lockfile());
    rig.gh.script_head(Script::always(Err(GhFailure { exit: 1, output: "boom".into() })));
    assert_eq!(rig.refusal(), DepsRefusal::HeadUnreadable { pr: pr() });
}

#[test]
fn an_empty_head_refuses_as_unreadable() {
    let rig = Rig::new(&lockfile());
    rig.gh.script_head(Script::always(Ok("\n".into())));
    assert_eq!(rig.refusal(), DepsRefusal::HeadUnreadable { pr: pr() });
}

#[test]
fn a_head_that_is_not_lower_case_hex_is_not_used_in_an_api_path() {
    let rig = Rig::new(&lockfile());
    rig.gh.script_head(Script::always(Ok("0123456/../pull/1/head".into())));
    assert_eq!(rig.refusal(), DepsRefusal::HeadUnreadable { pr: pr() });
}

#[test]
fn a_head_that_moved_names_both_heads_to_eight_characters() {
    let rig = Rig::new(&lockfile());
    let refusal = rig.run_as("fedcba98765").unwrap_err();
    assert_eq!(
        refusal,
        DepsRefusal::HeadMoved {
            pr: pr(),
            real_head_8: "01234567".into(),
            sha_8: "fedcba98".into()
        }
    );
    assert_eq!(
        refusal.to_string(),
        "#7 is at 01234567, not fedcba98. It moved while you were reading it."
    );
}

#[test]
fn a_diff_that_could_not_be_read_quotes_what_gh_printed_indented() {
    let rig = Rig::new(&lockfile());
    rig.gh.script_diff(Script::always(Err(GhFailure {
        exit: 1,
        output: "HTTP 502\nbad gateway".into(),
    })));
    assert_eq!(
        rig.refusal().to_string(),
        "could not read the diff for #7:\n\n    HTTP 502\n    bad gateway"
    );
    assert!(rig.classifier.calls().is_empty());
}

#[test]
fn a_diff_of_only_newlines_is_empty_and_is_not_classified() {
    let rig = Rig::new(&lockfile());
    rig.gh.script_diff(Script::always(Ok("\n\n".into())));
    assert_eq!(rig.refusal(), DepsRefusal::DiffEmpty { pr: pr() });
    assert!(rig.classifier.calls().is_empty());
}

#[test]
fn a_classifier_that_ran_past_its_limit_refuses_with_the_limit_text() {
    for exit in [ClassifierExit::TimedOut, ClassifierExit::Code(124)] {
        let mut rig = Rig::new(&lockfile());
        rig.config.limit = ClassifyLimit::parse("1.5m");
        rig.classifier.script(Script::always(Ok(ClassifierRun { exit, stdout: String::new() })));
        assert_eq!(
            rig.refusal().to_string(),
            "the dependency classifier ran past 1.5ms on #7, so nothing was classified",
            "{exit:?}"
        );
    }
}

#[test]
fn a_classifier_refusal_is_quoted_whole_and_indented() {
    let rig = Rig::new(&lockfile());
    rig.classifier.script(Script::always(Ok(ClassifierRun {
        exit: ClassifierExit::Code(1),
        stdout: "{\n  \"verdict\": \"refused\"\n}\n".into(),
    })));
    assert_eq!(
        rig.refusal().to_string(),
        "#7 is not a pure dependency bump, so it needs a real receipt:\n\n    {\n      \"verdict\": \"refused\"\n    }"
    );
}

#[test]
fn any_nonzero_classifier_exit_is_a_refusal() {
    for code in [1, 2, 125, 127, 137] {
        let rig = Rig::new(&lockfile());
        rig.classifier.script(Script::always(Ok(ClassifierRun {
            exit: ClassifierExit::Code(code),
            stdout: String::new(),
        })));
        assert!(matches!(rig.refusal(), DepsRefusal::NotAPureBump { .. }), "{code}");
    }
}

#[test]
fn a_classifier_that_could_not_start_is_a_refusal_with_an_empty_verdict() {
    let rig = Rig::new(&lockfile());
    rig.classifier.script(Script::always(Err(fail("python3: not found on the search path"))));
    assert_eq!(
        rig.refusal().to_string(),
        "#7 is not a pure dependency bump, so it needs a real receipt:\n\n"
    );
}

#[test]
fn a_limit_timeout_cannot_bound_never_starts_the_classifier() {
    for text in ["0", "abc", "", "-1", "inf", "1.2.3", "5x"] {
        let mut rig = Rig::new(&lockfile());
        rig.config.limit = ClassifyLimit::parse(text);
        assert!(matches!(rig.refusal(), DepsRefusal::NotAPureBump { .. }), "{text:?}");
        assert!(rig.classifier.calls().is_empty(), "{text:?}");
    }
}

#[test]
fn classifier_output_that_is_not_a_verdict_refuses_as_unreadable_pins() {
    let rig = Rig::new("this is not json");
    assert_eq!(rig.refusal(), DepsRefusal::PinsUnreadable);
}

#[test]
fn a_verdict_the_bash_could_not_read_is_refused_naming_the_first_section_that_fails() {
    let verdict = |pins: &str, images: &str, pip: &str, files: &str| {
        format!(
            r#"{{"verdict": "dependency-bump", "files": {files}, "changed_lines": 1, "pins": {pins}, "images": {images}, "pip": {pip}}}"#
        )
    };
    let good_pin = r#"[{"action": "a/b", "sha": "x", "tag": "t", "branch": false}]"#;
    let cases = [
        (verdict(r#"[{"action": "a/b"}]"#, "[]", "[]", "{}"), DepsRefusal::PinsUnreadable),
        (verdict("{}", "[]", "[]", "{}"), DepsRefusal::PinsUnreadable),
        (verdict("null", "[]", "[]", "{}"), DepsRefusal::PinsUnreadable),
        (verdict(good_pin, r#"[{"image": "i"}]"#, "[]", "{}"), DepsRefusal::ImagesUnreadable),
        (verdict(good_pin, "[]", r#"[{"name": "n"}]"#, "{}"), DepsRefusal::PipUnreadable),
        (verdict(good_pin, "[]", "[]", "[]"), DepsRefusal::PipFilesUnreadable),
        (
            r#"{"verdict": "dependency-bump", "files": {}, "changed_lines": 1, "pins": [], "images": [], "pip": [], "extra": 1}"#.to_string(),
            DepsRefusal::PinsUnreadable,
        ),
        ("[1, 2]".to_string(), DepsRefusal::PinsUnreadable),
        (String::new(), DepsRefusal::PinsUnreadable),
    ];
    for (text, want) in cases {
        let rig = Rig::new(&text);
        assert_eq!(rig.refusal(), want, "{text}");
        assert!(rig.net.calls().is_empty(), "no check runs on a verdict that was not read: {text}");
    }
}

#[test]
fn a_refused_verdict_with_exit_zero_is_still_a_refusal() {
    let rig = Rig::new(r#"{"verdict": "refused", "reason": "x"}"#);
    assert!(matches!(rig.refusal(), DepsRefusal::NotAPureBump { .. }));
}

// ----- action pins -----

#[test]
fn a_tag_pin_agrees_when_github_says_the_tag_is_that_commit() {
    let rig = workflow_rig(&[pin("actions/checkout", "v4.2.2", false)]);
    rig.net.script_tag_ref(Script::always(Ok(tag_ref("v4.2.2", "commit", PIN_SHA))));
    let verified = rig.run().unwrap();
    assert_eq!(verified.facts().pins_verified, 1);
    assert_eq!(rig.said(), "  pin: actions/checkout v4.2.2 = bbbbbbbbbbbb (GitHub agrees)\n");
    assert_eq!(
        rig.net.calls(),
        vec![NetCall::TagRef { repo: "actions/checkout".into(), tag: "v4.2.2".into() }]
    );
}

#[test]
fn an_action_inside_a_repository_asks_about_the_repository() {
    let rig = workflow_rig(&[pin("github/codeql-action/analyze", "v3", false)]);
    rig.net.script_tag_ref(Script::always(Ok(tag_ref("v3", "commit", PIN_SHA))));
    rig.run().unwrap();
    assert_eq!(
        rig.net.calls(),
        vec![NetCall::TagRef { repo: "github/codeql-action".into(), tag: "v3".into() }]
    );
}

#[test]
fn a_pin_to_a_different_commit_than_its_tag_refuses_with_twelve_characters_each() {
    let rig = workflow_rig(&[pin("actions/checkout", "v4.2.2", false)]);
    rig.net.script_tag_ref(Script::always(Ok(tag_ref("v4.2.2", "commit", &"e".repeat(40)))));
    assert_eq!(
        rig.refusal().to_string(),
        "the pin comment is not true: actions/checkout is pinned to\n  bbbbbbbbbbbb and actions/checkout v4.2.2 is eeeeeeeeeeee. A pin whose comment names a\n  version it is not on is the shape a supply-chain swap hides in."
    );
    assert_eq!(rig.said(), "");
}

#[test]
fn a_tag_github_could_not_be_asked_about_refuses_with_its_answer_indented() {
    let rig = workflow_rig(&[pin("actions/checkout", "v4.2.2", false)]);
    rig.net.script_tag_ref(Script::always(Err(fail("gh: Not Found (HTTP 404)\n"))));
    assert_eq!(
        rig.refusal().to_string(),
        "could not ask GitHub for the tag v4.2.2 in actions/checkout, so the pin is unverified:\n\n    gh: Not Found (HTTP 404)"
    );
}

#[test]
fn a_ref_that_is_not_the_tag_asked_for_refuses_and_names_what_github_answered() {
    let rig = workflow_rig(&[pin("actions/checkout", "v4.2.2", false)]);
    rig.net.script_tag_ref(Script::always(Ok(TagRef {
        name: "refs/pull/1/head".into(),
        object_type: "commit".into(),
        object_sha: PIN_SHA.into(),
    })));
    assert_eq!(
        rig.refusal().to_string(),
        "the pin comment is not a tag: actions/checkout names 'v4.2.2', and GitHub\n  answers refs/pull/1/head for refs/tags/v4.2.2. A name that resolves to another\n  ref is how a pin to a pull request's head passes as a release."
    );
}

#[test]
fn an_empty_ref_name_reads_as_nothing_in_the_refusal() {
    let rig = workflow_rig(&[pin("actions/checkout", "v4.2.2", false)]);
    rig.net.script_tag_ref(Script::always(Ok(TagRef {
        name: String::new(),
        object_type: String::new(),
        object_sha: String::new(),
    })));
    assert_eq!(
        rig.refusal(),
        DepsRefusal::PinCommentNotATag {
            action: "actions/checkout".into(),
            tag: "v4.2.2".into(),
            refname: "nothing".into()
        }
    );
}

#[test]
fn an_annotated_tag_is_followed_to_the_commit_it_names() {
    let rig = workflow_rig(&[pin("actions/checkout", "v4", false)]);
    rig.net.script_tag_ref(Script::always(Ok(tag_ref("v4", "tag", &"1".repeat(40)))));
    rig.net.script_tag_object(Script::sequence([
        Ok(GitObject { object_type: "tag".into(), sha: "2".repeat(40) }),
        Ok(GitObject { object_type: "commit".into(), sha: PIN_SHA.into() }),
    ]));
    rig.run().unwrap();
    assert_eq!(
        rig.net.calls(),
        vec![
            NetCall::TagRef { repo: "actions/checkout".into(), tag: "v4".into() },
            NetCall::TagObject { repo: "actions/checkout".into(), sha: "1".repeat(40) },
            NetCall::TagObject { repo: "actions/checkout".into(), sha: "2".repeat(40) },
        ]
    );
}

#[test]
fn a_chain_of_four_tag_objects_is_followed_and_a_fifth_refuses() {
    let chain = |levels: usize| {
        let rig = workflow_rig(&[pin("actions/checkout", "v4", false)]);
        rig.net.script_tag_ref(Script::always(Ok(tag_ref("v4", "tag", &"1".repeat(40)))));
        let mut answers: Vec<_> = (0..levels - 1)
            .map(|_| Ok(GitObject { object_type: "tag".into(), sha: "2".repeat(40) }))
            .collect();
        answers.push(Ok(GitObject { object_type: "commit".into(), sha: PIN_SHA.into() }));
        rig.net.script_tag_object(Script::sequence(answers));
        rig
    };
    assert!(chain(4).run().is_ok());
    let rig = workflow_rig(&[pin("actions/checkout", "v4", false)]);
    rig.net.script_tag_ref(Script::always(Ok(tag_ref("v4", "tag", &"1".repeat(40)))));
    rig.net.script_tag_object(Script::always(Ok(GitObject {
        object_type: "tag".into(),
        sha: "2".repeat(40),
    })));
    assert_eq!(
        rig.refusal().to_string(),
        "the tag v4 in actions/checkout is nested more than 4 tag objects deep\n  without reaching a commit, so the pin is unverified."
    );
    assert_eq!(rig.net.calls().len(), 1 + 4, "the fifth tag object is never asked for");
}

#[test]
fn a_tag_object_that_could_not_be_read_names_twelve_characters_of_its_sha() {
    let rig = workflow_rig(&[pin("actions/checkout", "v4", false)]);
    rig.net.script_tag_ref(Script::always(Ok(tag_ref(
        "v4",
        "tag",
        "123456789abc0000000000000000000000000000",
    ))));
    rig.net.script_tag_object(Script::always(Err(fail("gh: HTTP 500"))));
    assert_eq!(
        rig.refusal().to_string(),
        "could not read the tag object 123456789abc behind actions/checkout v4, so the pin is unverified:\n\n    gh: HTTP 500"
    );
}

#[test]
fn a_tag_that_ends_at_something_other_than_a_commit_refuses() {
    for (object_type, shown) in [("blob", "blob"), ("", "nothing")] {
        let rig = workflow_rig(&[pin("actions/checkout", "v4", false)]);
        rig.net.script_tag_ref(Script::always(Ok(tag_ref("v4", object_type, PIN_SHA))));
        assert_eq!(
            rig.refusal().to_string(),
            format!(
                "the tag v4 in actions/checkout points at a {shown}, not a commit,\n  so no pin can be on it."
            )
        );
    }
}

#[test]
fn a_branch_pin_agrees_when_the_commit_is_identical_or_behind() {
    for status in ["identical", "behind"] {
        let rig = workflow_rig(&[pin("actions/checkout", "stable", true)]);
        rig.net.script_branch_ref(Script::always(Ok("refs/heads/stable\n".into())));
        rig.net.script_compare_status(Script::always(Ok(format!("{status}\n"))));
        rig.run().unwrap();
        assert_eq!(
            rig.said(),
            "  pin: actions/checkout bbbbbbbbbbbb is reachable from stable (GitHub agrees)\n"
        );
        assert_eq!(
            rig.net.calls(),
            vec![
                NetCall::BranchRef { repo: "actions/checkout".into(), branch: "stable".into() },
                NetCall::CompareStatus {
                    repo: "actions/checkout".into(),
                    base: "stable".into(),
                    head: PIN_SHA.into()
                },
            ]
        );
    }
}

#[test]
fn a_branch_pin_off_its_branch_refuses_for_ahead_diverged_and_anything_else() {
    for status in ["ahead", "diverged", "", "identical-ish"] {
        let rig = workflow_rig(&[pin("actions/checkout", "stable", true)]);
        rig.net.script_branch_ref(Script::always(Ok("refs/heads/stable".into())));
        rig.net.script_compare_status(Script::always(Ok(status.into())));
        assert_eq!(
            rig.refusal().to_string(),
            format!(
                "the pin comment is not true: actions/checkout is pinned to bbbbbbbbbbbb,\n  which is not reachable from stable in actions/checkout (compare says '{status}'). A\n  branch pin off its branch is a commit nobody on that branch published."
            ),
            "{status:?}"
        );
    }
}

#[test]
fn a_branch_name_that_resolves_to_another_ref_is_refused_before_compare_is_asked() {
    let rig = workflow_rig(&[pin("actions/checkout", "stable", true)]);
    rig.net.script_branch_ref(Script::always(Ok("refs/pull/1/head".into())));
    assert_eq!(
        rig.refusal().to_string(),
        "the pin comment is not a branch: actions/checkout names 'stable', and GitHub\n  answers refs/pull/1/head for refs/heads/stable. A name that resolves to another ref\n  is how a pin to a pull request's head passes as a branch."
    );
    assert_eq!(rig.net.calls().len(), 1);
}

#[test]
fn a_branch_github_could_not_be_asked_about_refuses() {
    let rig = workflow_rig(&[pin("actions/checkout", "stable", true)]);
    rig.net.script_branch_ref(Script::always(Err(fail("gh: Not Found"))));
    assert_eq!(
        rig.refusal().to_string(),
        "could not ask GitHub whether actions/checkout has a branch named stable, so the pin is unverified:\n\n    gh: Not Found"
    );
}

#[test]
fn a_comparison_github_could_not_make_refuses_and_names_the_whole_sha() {
    let rig = workflow_rig(&[pin("actions/checkout", "stable", true)]);
    rig.net.script_branch_ref(Script::always(Ok("refs/heads/stable".into())));
    rig.net.script_compare_status(Script::always(Err(fail("gh: HTTP 404"))));
    assert_eq!(
        rig.refusal().to_string(),
        format!(
            "could not ask GitHub whether actions/checkout {PIN_SHA} is on stable, so the pin is unverified:\n\n    gh: HTTP 404"
        )
    );
}

#[test]
fn an_action_that_is_not_owner_and_repo_refuses_before_any_network_call() {
    let rig = workflow_rig(&[pin("checkout", "v4", false)]);
    assert_eq!(rig.refusal(), DepsRefusal::ActionNotOwnerRepo { action: "checkout".into() });
    assert!(rig.net.calls().is_empty());
}

#[test]
fn pins_are_verified_in_the_order_the_classifier_listed_them_and_all_of_them_count() {
    let rig = workflow_rig(&[
        pin("actions/checkout", "v4", false),
        pin("actions/cache", "v3", false),
        pin("actions/upload-artifact", "stable", true),
    ]);
    rig.net.script_tag_ref(Script::sequence([
        Ok(tag_ref("v4", "commit", PIN_SHA)),
        Ok(tag_ref("v3", "commit", PIN_SHA)),
    ]));
    rig.net.script_branch_ref(Script::always(Ok("refs/heads/stable".into())));
    rig.net.script_compare_status(Script::always(Ok("identical".into())));
    let verified = rig.run().unwrap();
    assert_eq!(verified.facts().pins_verified, 3);
    let repos: Vec<_> = rig
        .net
        .calls()
        .into_iter()
        .map(|call| match call {
            NetCall::TagRef { repo, .. } | NetCall::BranchRef { repo, .. } => repo,
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(repos[..3], ["actions/checkout", "actions/cache", "actions/upload-artifact"]);
}

// ----- base images and pip packages -----

fn dockerfile_rig() -> Rig {
    Rig::new(&bump(
        &json!({"Dockerfile": "dockerfile"}),
        &[],
        &[image("docker.io/library/rust", "1-bookworm")],
        &[],
    ))
}

#[test]
fn an_image_agrees_when_the_registry_names_the_pinned_digest() {
    let rig = dockerfile_rig();
    rig.net.script_image_digest(Script::always(Ok(format!("{DIGEST}\n"))));
    let verified = rig.run().unwrap();
    assert_eq!(verified.facts().images_verified, 1);
    assert_eq!(
        rig.said(),
        "  image: docker.io/library/rust:1-bookworm = ddddddddddddddddddd (the registry agrees)\n"
    );
}

#[test]
fn an_image_digest_the_registry_does_not_name_refuses_with_nineteen_characters_each() {
    let rig = dockerfile_rig();
    rig.net.script_image_digest(Script::always(Ok(format!("sha256:{}", "e".repeat(64)))));
    assert_eq!(
        rig.refusal().to_string(),
        "the base image pin is not the tag it claims: docker.io/library/rust:1-bookworm\n  resolves to sha256:eeeeeeeeeeee and the Dockerfile pins sha256:dddddddddddd. A digest\n  that is not the one its tag names is the shape a base-image swap hides in."
    );
}

#[test]
fn a_registry_that_could_not_be_asked_refuses_with_its_answer() {
    let rig = dockerfile_rig();
    rig.net.script_image_digest(Script::always(Err(fail("resolve_image_digest: could not reach"))));
    assert_eq!(
        rig.refusal().to_string(),
        "could not ask the registry what docker.io/library/rust:1-bookworm is,\n  so the pin is unverified:\n\n    resolve_image_digest: could not reach"
    );
}

#[test]
fn a_missing_image_resolver_refuses_only_when_there_is_an_image_to_resolve() {
    let mut rig = dockerfile_rig();
    rig.config.scripts.image_resolver = "/s/no-resolver.py".into();
    assert_eq!(
        rig.refusal().to_string(),
        "no image resolver at /s/no-resolver.py; run ./install.sh"
    );
    let mut quiet = Rig::new(&lockfile());
    quiet.config.scripts.image_resolver = "/s/no-resolver.py".into();
    quiet.config.scripts.pypi_verifier = "/s/no-verifier.py".into();
    quiet.config.scripts.pip_files = "/s/no-wholefile.py".into();
    assert!(quiet.run().is_ok());
}

fn pip_rig(hashes: &[&str]) -> Rig {
    Rig::new(&bump(
        &json!({"requirements.txt": "pip-requirements"}),
        &[],
        &[],
        &[package("requirements.txt", "PyYAML", "6.0.3", hashes)],
    ))
}

fn script_whole_file_path(rig: &Rig, changed: &Value) {
    rig.gh.script_base_ref(Script::always(Ok("main\n".into())));
    rig.net.script_merge_base(Script::always(Ok(format!("{MERGE_BASE}\n"))));
    rig.net.script_file_at(Script::sequence([Ok(b"base".to_vec()), Ok(b"head".to_vec())]));
    rig.whole.script(Script::always(Ok(json!({ "changed": changed }).to_string())));
}

fn changed(hashes: &[&str]) -> Value {
    json!([{"name": "PyYAML", "version": "6.0.3", "hashes": hashes}])
}

#[test]
fn a_pip_package_agrees_when_pypi_publishes_every_hash_and_says_how_many_files() {
    let rig = pip_rig(&[HASH_A, HASH_B]);
    rig.net.script_pypi_release(Script::always(Ok("2 of 5 files\n".into())));
    script_whole_file_path(&rig, &changed(&[HASH_B, HASH_A]));
    let verified = rig.run().unwrap();
    assert_eq!(verified.facts().pip_verified, 1);
    assert_eq!(
        rig.net.calls()[0],
        NetCall::PypiRelease {
            name: "PyYAML".into(),
            version: "6.0.3".into(),
            hashes: vec![HASH_A.into(), HASH_B.into()]
        }
    );
    assert!(rig.said().starts_with("  pip: PyYAML==6.0.3, 2 of 5 files (PyPI agrees)\n"));
}

#[test]
fn a_hash_pypi_does_not_publish_refuses_with_its_answer() {
    let rig = pip_rig(&[HASH_A]);
    rig.net.script_pypi_release(Script::always(Err(fail(
        "sha256:aaaaaaaaaaaa is not a file PyPI publishes",
    ))));
    assert_eq!(
        rig.refusal().to_string(),
        "could not confirm PyYAML==6.0.3 against PyPI,\n  so the pin is unverified:\n\n    sha256:aaaaaaaaaaaa is not a file PyPI publishes"
    );
}

#[test]
fn a_missing_pypi_verifier_refuses_with_the_install_hint() {
    let mut rig = pip_rig(&[HASH_A]);
    rig.config.scripts.pypi_verifier = "/s/no-verifier.py".into();
    assert_eq!(
        rig.refusal().to_string(),
        "no PyPI verifier at /s/no-verifier.py; run ./install.sh"
    );
}

// ----- requirements files read whole -----

fn whole_rig() -> Rig {
    let rig = pip_rig(&[HASH_A, HASH_B]);
    rig.net.script_pypi_release(Script::always(Ok("2 of 2 files".into())));
    script_whole_file_path(&rig, &changed(&[HASH_B, HASH_A]));
    rig
}

#[test]
fn the_whole_file_at_the_merge_base_and_the_head_must_tell_the_same_story_as_the_diff() {
    let rig = whole_rig();
    rig.run().unwrap();
    assert_eq!(
        rig.net.calls()[1..],
        [
            NetCall::MergeBase {
                slug: "o/r".into(),
                base_ref: "main".into(),
                head: CommitId::parse(HEAD).unwrap()
            },
            NetCall::FileAt {
                slug: "o/r".into(),
                path: "requirements.txt".into(),
                rev: CommitId::parse(MERGE_BASE).unwrap()
            },
            NetCall::FileAt {
                slug: "o/r".into(),
                path: "requirements.txt".into(),
                rev: CommitId::parse(HEAD).unwrap()
            },
        ]
    );
    assert_eq!(
        rig.whole.calls(),
        vec![("requirements.txt".into(), b"base".to_vec(), b"head".to_vec())]
    );
    assert!(rig.said().ends_with(
        "  pip file: requirements.txt read whole at cccccccc and 01234567 (same bump as the diff)\n"
    ));
}

#[test]
fn a_missing_whole_file_check_refuses_before_the_base_branch_is_read() {
    let mut rig = pip_rig(&[HASH_A]);
    rig.net.script_pypi_release(Script::always(Ok("1 of 1".into())));
    rig.config.scripts.pip_files = "/s/no-wholefile.py".into();
    assert_eq!(
        rig.refusal().to_string(),
        "no whole-file pip check at /s/no-wholefile.py; run ./install.sh"
    );
}

#[test]
fn a_base_branch_that_could_not_be_read_refuses_whether_gh_failed_or_said_nothing() {
    for answer in [Err(GhFailure { exit: 1, output: "x".into() }), Ok("\n".to_string())] {
        let rig = whole_rig();
        rig.gh.script_base_ref(Script::always(answer));
        assert_eq!(rig.refusal(), DepsRefusal::BaseBranchUnreadable { pr: pr() });
    }
}

#[test]
fn a_merge_base_github_could_not_give_refuses() {
    let rig = whole_rig();
    rig.net.script_merge_base(Script::always(Err(fail("HTTP 404"))));
    assert_eq!(rig.refusal(), DepsRefusal::MergeBaseUnaskable { pr: pr() });
}

#[test]
fn a_merge_base_that_is_not_forty_hex_digits_refuses_and_shows_forty_characters() {
    let rig = whole_rig();
    rig.net.script_merge_base(Script::always(Ok("n".repeat(60))));
    assert_eq!(
        rig.refusal().to_string(),
        format!("GitHub gave no merge base for #7: '{}'", "n".repeat(40))
    );
}

#[test]
fn a_requirements_path_that_is_not_plain_never_reaches_an_api_url() {
    let rig = Rig::new(&bump(&json!({"../etc/passwd": "pip-requirements"}), &[], &[], &[]));
    rig.gh.script_base_ref(Script::always(Ok("main".into())));
    rig.net.script_merge_base(Script::always(Ok(MERGE_BASE.into())));
    assert_eq!(
        rig.refusal().to_string(),
        "the requirements path '../etc/passwd' is not a plain path, so it is not put in an API URL"
    );
    assert!(rig.net.calls().iter().all(|call| !matches!(call, NetCall::FileAt { .. })));
}

#[test]
fn a_file_that_could_not_be_read_at_the_merge_base_or_the_head_refuses_naming_which() {
    let rig = whole_rig();
    rig.net.script_file_at(Script::sequence([Err(fail("404"))]));
    assert_eq!(
        rig.refusal().to_string(),
        "could not read requirements.txt at the merge base cccccccc"
    );
    let rig = whole_rig();
    rig.net.script_file_at(Script::sequence([Ok(b"base".to_vec()), Err(fail("404"))]));
    assert_eq!(rig.refusal().to_string(), "could not read requirements.txt at the head 01234567");
}

#[test]
fn a_whole_file_that_is_not_a_version_bump_quotes_the_checkers_words() {
    let rig = whole_rig();
    rig.whole.script(Script::always(Err(fail("verify_pip_files: a line was added\nand more"))));
    assert_eq!(
        rig.refusal().to_string(),
        "requirements.txt, read whole the way pip reads it, is not a version bump:\n\n    verify_pip_files: a line was added\n    and more"
    );
}

#[test]
fn a_whole_file_answer_that_is_not_the_expected_json_refuses() {
    for answer in [
        "not json",
        "{}",
        r#"{"changed": [{"name": 1}]}"#,
        r#"{"changed": [{"name":"a","version":"1"}]}"#,
    ] {
        let rig = whole_rig();
        rig.whole.script(Script::always(Ok(answer.into())));
        assert_eq!(
            rig.refusal(),
            DepsRefusal::WholeFileAnswerUnreadable { pf: "requirements.txt".into() },
            "{answer}"
        );
    }
}

#[test]
fn a_diff_and_a_whole_file_that_disagree_refuse_and_show_both_readings() {
    let rig = whole_rig();
    rig.whole.script(Script::always(Ok(json!({"changed": []}).to_string())));
    let refusal = rig.refusal();
    assert_eq!(
        refusal.to_string(),
        format!(
            "the diff and the whole file disagree about requirements.txt:\n  diff:  [[\"PyYAML\", \"6.0.3\", [\"{HASH_A}\", \"{HASH_B}\"]]]\n  files: []\n  A bump the two readings describe differently is not one this gate can vouch for."
        )
    );
}

#[test]
fn each_reading_is_cut_at_three_hundred_characters() {
    let many: Vec<String> = (0..10).map(|i| format!("{i:0>64}")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let rig = pip_rig(&refs);
    rig.net.script_pypi_release(Script::always(Ok("ok".into())));
    script_whole_file_path(&rig, &json!([]));
    let DepsRefusal::DiffAndFileDisagree { want_300, got_300, .. } = rig.refusal() else {
        panic!("expected a disagreement");
    };
    assert_eq!(want_300.chars().count(), 300);
    assert_eq!(got_300, "[]");
}

#[test]
fn the_whole_file_only_has_to_match_the_packages_the_diff_found_in_that_file() {
    let rig = Rig::new(&bump(
        &json!({"a/requirements.txt": "pip-requirements", "b/requirements.txt": "pip-requirements"}),
        &[],
        &[],
        &[
            package("a/requirements.txt", "A", "2", &[HASH_A]),
            package("b/requirements.txt", "B", "3", &[HASH_B]),
        ],
    ));
    rig.net.script_pypi_release(Script::always(Ok("1 of 1".into())));
    rig.gh.script_base_ref(Script::always(Ok("main".into())));
    rig.net.script_merge_base(Script::always(Ok(MERGE_BASE.into())));
    rig.net.script_file_at(Script::always(Ok(b"x".to_vec())));
    rig.whole.script(Script::sequence([
        Ok(json!({"changed": [{"name": "A", "version": "2", "hashes": [HASH_A]}]}).to_string()),
        Ok(json!({"changed": [{"name": "B", "version": "3", "hashes": [HASH_B]}]}).to_string()),
    ]));
    let verified = rig.run().unwrap();
    assert_eq!(verified.facts().pip_verified, 2);
    assert_eq!(rig.whole.calls().len(), 2);
}

// ----- what a bump with nothing to ask earns, the receipt facts, the lease -----

#[test]
fn a_mixed_bump_checks_pins_then_images_then_pip_then_files_and_counts_each() {
    let rig = Rig::new(&bump(
        &json!({"requirements.txt": "pip-requirements", "Dockerfile": "dockerfile", ".github/workflows/ci.yml": "workflow"}),
        &[pin("actions/checkout", "v4", false)],
        &[image("docker.io/library/rust", "1")],
        &[package("requirements.txt", "PyYAML", "6.0.3", &[HASH_A, HASH_B])],
    ));
    rig.net.script_tag_ref(Script::always(Ok(tag_ref("v4", "commit", PIN_SHA))));
    rig.net.script_image_digest(Script::always(Ok(DIGEST.into())));
    rig.net.script_pypi_release(Script::always(Ok("2 of 2 files".into())));
    script_whole_file_path(&rig, &changed(&[HASH_A, HASH_B]));
    let verified = rig.run().unwrap();
    assert_eq!(
        verified.facts(),
        &DependencyFacts {
            files: "requirements.txt (pip-requirements), Dockerfile (dockerfile), .github/workflows/ci.yml (workflow)".into(),
            changed_lines: 6,
            pins_verified: 1,
            images_verified: 1,
            pip_verified: 1
        }
    );
    let said = rig.said();
    let at =
        |needle: &str| said.find(needle).unwrap_or_else(|| panic!("{needle} not said in {said}"));
    assert!(
        at("pin: ") < at("image: ")
            && at("image: ") < at("pip: ")
            && at("pip: ") < at("pip file: ")
    );
    assert_eq!(
        verified.recorded_line().to_string(),
        "dependency receipt recorded for #7 at 01234567 (6 line(s), 1 pin(s), 1 image(s), 1 pip package(s) verified)"
    );
}

#[test]
fn the_receipt_is_a_dependency_receipt_carrying_the_counts() {
    let rig = Rig::new(&lockfile());
    let verified = rig.run().unwrap();
    let receipt = verified.receipt(&Stamp::new("2026-11-22T10:00:00+00:00", "gate"));
    let body = receipt.body();
    assert_eq!((body.pr, body.head.as_str()), (7, HEAD));
    assert_eq!(body.pins_verified, Some(0));
    assert_eq!(body.files.as_deref(), Some("Cargo.lock (cargo-lock)"));
    assert_eq!(body.changed_lines, Some(6));
}

#[test]
fn a_lease_held_by_someone_else_refuses_before_the_flow_starts() {
    let helper = FakeHelper::new();
    helper.script_lease(Script::always(Ok(LeaseAnswer::Held { output: "held by amy".into() })));
    let failure = lease_gate(&helper, &pr()).unwrap_err();
    assert_eq!(
        failure.to_string(),
        "will not verify #7: another actor has leased it.\n  held by amy\n  Ask them, wait for it to expire, or have them `maintainer lease release 7`."
    );
    assert_eq!(failure.exit_code(), 1);
}

#[test]
fn a_lease_that_could_not_be_read_or_asked_refuses() {
    let helper = FakeHelper::new();
    helper.script_lease(Script::sequence([
        Ok(LeaseAnswer::Unreadable { output: "state locked".into() }),
        Err(HelperMissing),
    ]));
    assert!(
        lease_gate(&helper, &pr())
            .unwrap_err()
            .to_string()
            .starts_with("could not read the lease on #7, so I will not verify it:")
    );
    assert!(
        lease_gate(&helper, &pr())
            .unwrap_err()
            .to_string()
            .starts_with("cannot tell whether #7 is leased")
    );
}

#[test]
fn a_failure_prints_the_program_name_and_the_message_on_one_line_and_exits_one() {
    let failure = DepsFailure::Refused(Box::new(DepsRefusal::DiffEmpty { pr: pr() }));
    assert_eq!(
        failure.stderr_line(),
        "maintainer-merge: the diff for #7 is empty, so nothing was classified\n"
    );
    assert_eq!(failure.exit_code(), 1);
}

#[test]
fn any_gh_port_answers_the_three_reads_of_the_flow() {
    let gh = FakeGh::new();
    gh.script_head(Script::always(Ok("abc".into())));
    gh.script_base_ref(Script::always(Ok("main".into())));
    gh.script_diff(Script::always(Ok("d".into())));
    let narrow: &dyn DepsGh = &gh;
    assert_eq!(narrow.head(&pr()).unwrap(), "abc");
    assert_eq!(narrow.base_ref(&pr()).unwrap(), "main");
    assert_eq!(narrow.diff(&pr()).unwrap(), "d");
}

// ----- the limit and the script paths -----

#[test]
fn the_classify_limit_reads_what_timeout_reads() {
    let secs = |text: &str| ClassifyLimit::parse(text).duration();
    assert_eq!(secs("60"), Some(Duration::from_mins(1)));
    assert_eq!(secs("60s"), Some(Duration::from_mins(1)));
    assert_eq!(secs("2m"), Some(Duration::from_mins(2)));
    assert_eq!(secs("1h"), Some(Duration::from_hours(1)));
    assert_eq!(secs("1d"), Some(Duration::from_hours(24)));
    assert_eq!(secs("0.5"), Some(Duration::from_millis(500)));
    assert_eq!(secs("1.5m"), Some(Duration::from_secs(90)));
    assert_eq!(secs(".5"), Some(Duration::from_millis(500)));
    assert_eq!(secs("5."), Some(Duration::from_secs(5)));
    assert_eq!(secs("0"), None, "timeout reads 0 as no limit, which is no bound here");
    assert_eq!(secs("0s"), None);
    assert_eq!(secs("1e2"), Some(Duration::from_secs(100)));
    assert_eq!(secs("+5"), Some(Duration::from_secs(5)));
    for text in ["", "s", ".", "-5", "1.2.3", "1x", "inf", "nan", "infinitys", "1e999", "0e0", "-0"]
    {
        assert_eq!(secs(text), None, "{text:?}");
    }
}

#[test]
fn script_paths_follow_the_bash_defaults_and_overrides() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| pairs.iter().find(|(key, _)| *key == name).map(|(_, v)| (*v).to_string())
    };
    let defaults = ScriptPaths::from_env(env(&[("HOME", "/home/u")]));
    assert_eq!(
        defaults.classifier,
        PathBuf::from("/home/u/.local/share/maintainer/scripts/classify_dependency_bump.py")
    );
    assert_eq!(
        defaults.pip_files,
        PathBuf::from("/home/u/.local/share/maintainer/scripts/verify_pip_files.py")
    );
    let moved = ScriptPaths::from_env(env(&[
        ("MAINTAINER_SCRIPTS", "/opt/s"),
        ("MAINTAINER_IMAGE_RESOLVER", "/x/resolve.py"),
        ("MAINTAINER_PYPI_VERIFIER", ""),
    ]));
    assert_eq!(moved.classifier, PathBuf::from("/opt/s/classify_dependency_bump.py"));
    assert_eq!(moved.image_resolver, PathBuf::from("/x/resolve.py"));
    assert_eq!(
        moved.pypi_verifier,
        PathBuf::from("/opt/s/verify_pypi_release.py"),
        "an empty override is unset"
    );
    assert_eq!(moved.pip_files, PathBuf::from("/opt/s/verify_pip_files.py"));
}

#[test]
fn nothing_was_written_by_any_refusal() {
    let rig = Rig::new(&lockfile());
    rig.gh.script_diff(Script::always(Ok(String::new())));
    drop(rig.refusal());
    assert!(
        rig.log
            .entries()
            .iter()
            .all(|effect| matches!(effect, mx_ports::Effect::Fs(mx_ports::FsEffect::Read(_))))
    );
}
