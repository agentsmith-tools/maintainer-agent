//! Behaviour of `verify` and `verify-part`: what a clean run, a mutated run and the tree between
//! them have to look like before a receipt is written, and what each refusal says.

use crate::testkit::{HEAD, World, observed, pr};
use crate::{Exit, HeadArg, LeaseAnswer, VerifyRefusal};
use mx_ports::{Fs, GitError, Script};
use mx_receipt::Stored;
use mx_sandbox::{ContainerExit, Escape, ExtractedTree, SandboxCall, SandboxError, SedExpression};
use mx_suite::{FakeSuite, FakeSuiteHost, Picked, SuiteName, TestFilter};
use std::path::PathBuf;

const WORK: &str = "/s/verify-work/w1";
const SCRATCH: &str = "/s/verify-work/s1";

fn suite(name: &str, covers: &[&str]) -> FakeSuite {
    FakeSuite::new(name, "rust:1")
        .covering(covers)
        .mutating(&["*.rs"])
        .ran(3)
        .command_template("cargo test {filter}")
}

fn world() -> World {
    let mut w = World::new();
    w.fs.remove(std::path::Path::new("/s/receipts/7.json")).unwrap();
    w.suites = FakeSuiteHost::new(
        "/verify.d",
        vec![suite("rust", &["bin/a", "lib/b"]), suite("docs", &["docs/x.md"])],
    );
    w.selector.script_pick(Script::always(Ok(Picked {
        suite: SuiteName::parse("rust").unwrap(),
        notes: String::new(),
    })));
    w.gh.script_changed_files(Script::always(Ok(vec!["bin/a".into()])));
    w.git.script_rev_parse(Script::always(Ok(HEAD.into())));
    w.sandbox
        .script_scratch(Script::sequence([Ok(PathBuf::from(WORK)), Ok(PathBuf::from(SCRATCH))]));
    w.sandbox.script_extract(Script::always(Ok(ExtractedTree::counted(
        WORK.into(),
        "v-7-1",
        1,
        1,
    )
    .unwrap())));
    w.sandbox.script_escapes(Script::always(Ok(Vec::new())));
    w.sandbox.script_seed(Script::always(Ok(())));
    w.sandbox.script_run(Script::sequence([Ok(ContainerExit(0)), Ok(ContainerExit(101))]));
    w.sandbox.script_mutation(Script::always(Ok(vec!["bin/a".into()])));
    w.fs.put(format!("{WORK}/clean.log"), b"running\ntest ok\n".to_vec(), 0);
    w.fs.put(
        format!("{WORK}/mutated.log"),
        b"thread panicked at x\nassertion failed: guard\n".to_vec(),
        0,
    );
    w
}

fn go(w: &World, part: bool, named: Option<&str>) -> Result<crate::Printed, Exit> {
    let named = named.map(|n| SuiteName::parse(n).unwrap());
    w.gate().verify(
        part,
        &pr("7"),
        &HeadArg::parse("0123456").unwrap(),
        &TestFilter::parse("guard").unwrap(),
        &SedExpression::parse("s/a/b/").unwrap(),
        named.as_ref(),
    )
}

fn refuse(w: &World, r: &VerifyRefusal) {
    assert_eq!(go(w, false, None).unwrap_err(), Exit::from(r));
}

fn receipt(w: &World) -> Stored {
    Stored::parse(&w.receipt_on_disk(7).expect("a receipt was written")).unwrap()
}

#[test]
fn a_clean_pass_and_a_mutated_failure_record_an_observed_receipt_at_the_real_head() {
    let w = world();
    go(&w, false, None).unwrap();
    let stored = receipt(&w);
    assert_eq!(stored.field_text("kind", ""), "observed");
    assert_eq!(stored.field_text("head", ""), HEAD);
    assert_eq!(stored.field_text("suite", ""), "rust");
    assert_eq!(stored.field_text("units_run", ""), "3");
    assert_eq!(stored.field_text("observed_failure", ""), "thread panicked at x");
    assert!(
        stored
            .field_text("proof", "")
            .contains("`guard` passes clean and fails under `s/a/b/` (rc=101)")
    );
    stored.check(&crate::testkit::key(), "7", true).unwrap();
    assert_eq!(
        w.stdout(),
        "  suite: rust (rust:1 under podman)\n  running 'guard' unmutated\n  rust suite: evidence of 3 executed unit(s), passing unmutated\n  applying the mutation and re-running\n  receipt recorded for #7 at 01234567 (observed: clean pass, mutated fail)\n"
    );
}

#[test]
fn the_two_container_runs_are_cut_off_from_the_network_and_run_the_suite_command() {
    let w = world();
    go(&w, false, None).unwrap();
    let runs: Vec<(Vec<String>, String)> = w
        .sandbox
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            SandboxCall::Run { argv, log, .. } => Some((argv, log.display().to_string())),
            _ => None,
        })
        .collect();
    assert_eq!(runs.len(), 2);
    for (argv, _) in &runs {
        assert!(argv.contains(&"--network=none".to_string()));
        assert_eq!(&argv[argv.len() - 3..], ["sh", "-c", "cargo test guard"]);
    }
    assert!(runs[0].1.ends_with("clean.log") && runs[1].1.ends_with("mutated.log"));
}

#[test]
fn a_lease_held_by_somebody_else_refuses_a_verify_before_the_runtime_is_asked() {
    let w = world();
    w.helper.script_lease(Script::always(Ok(LeaseAnswer::Held { output: "x".into() })));
    let exit = go(&w, false, None).unwrap_err();
    assert!(exit.stderr.contains("will not verify #7: another actor has leased it."), "{exit:?}");
}

#[test]
fn no_container_runtime_means_no_receipt() {
    let mut w = world();
    w.runtime.0 = None;
    refuse(&w, &VerifyRefusal::NoContainerRuntime);
}

#[test]
fn a_second_verify_of_the_same_pull_request_is_refused_while_the_first_runs() {
    let w = world();
    w.locks.hold(std::path::Path::new("/s/receipts/.verify-7.lock"));
    refuse(&w, &VerifyRefusal::VerifyLockHeld { pr: pr("7"), receipts: "/s/receipts".into() });
    let w = world();
    w.locks.make_unopenable(std::path::Path::new("/s/receipts/.verify-7.lock"));
    refuse(&w, &VerifyRefusal::CannotOpenVerifyLock { pr: pr("7") });
}

#[test]
fn an_empty_or_unreadable_changed_file_list_refuses() {
    let w = world();
    w.gh.script_changed_files(Script::always(Ok(Vec::new())));
    refuse(&w, &VerifyRefusal::ChangedFilesUnreadable { pr: pr("7") });
    let w = world();
    w.gh.script_changed_files(Script::always(Err(crate::GhFailure {
        exit: 1,
        output: String::new(),
    })));
    refuse(&w, &VerifyRefusal::ChangedFilesUnreadable { pr: pr("7") });
}

#[test]
fn a_profile_without_prod_globs_cannot_verify_decision_p3b_4() {
    let mut w = world();
    w.cfg.prod_globs = None;
    assert_eq!(go(&w, false, None).unwrap_err(), Exit::from(&crate::GateDecision::NoProdGlobs));
}

#[test]
fn the_notes_the_suite_picker_prints_reach_standard_error_on_success_and_on_refusal() {
    let w = world();
    w.selector.script_pick(Script::always(Ok(Picked {
        suite: SuiteName::parse("rust").unwrap(),
        notes: "  note\n".into(),
    })));
    assert_eq!(go(&w, false, None).unwrap().stderr, "  note\n");
    let w = world();
    w.selector.script_pick(Script::always(Ok(Picked {
        suite: SuiteName::parse("rust").unwrap(),
        notes: "  note\n".into(),
    })));
    w.sandbox.script_run(Script::always(Ok(ContainerExit(1))));
    assert!(go(&w, false, None).unwrap_err().stderr.starts_with("  note\n"));
}

#[test]
fn a_part_must_name_its_suite_and_the_refusal_lists_the_suites_with_a_trailing_blank() {
    let w = world();
    let exit = go(&w, true, None).unwrap_err();
    assert_eq!(
        exit,
        Exit::from(&VerifyRefusal::PartNeedsSuiteNamed {
            pr: pr("7"),
            suites: "docs rust ".into()
        })
    );
}

#[test]
fn a_part_that_runs_none_of_the_production_paths_adds_nothing_and_is_refused() {
    let w = world();
    assert_eq!(
        go(&w, true, Some("docs")).unwrap_err(),
        Exit::from(&VerifyRefusal::PartRunsNoProduction { suite: "docs".into(), pr: pr("7") })
    );
}

#[test]
fn a_part_is_never_picked_by_inference() {
    let w = world();
    go(&w, true, Some("rust")).unwrap();
    assert!(!w.sandbox.calls().is_empty());
    let stored = receipt(&w);
    assert_eq!(stored.field_text("partial", ""), "True");
}

#[test]
fn a_part_folds_into_a_signed_receipt_at_the_same_head_and_replaces_its_own_suite() {
    let mut w = world();
    w.suites = FakeSuiteHost::new(
        "/verify.d",
        vec![suite("rust", &["bin/a"]), suite("shell", &["bin/a"])],
    );
    w.receipt(&observed(7, HEAD, "shell", true));
    w.sandbox
        .script_scratch(Script::sequence([Ok(PathBuf::from(WORK)), Ok(PathBuf::from(SCRATCH))]));
    go(&w, true, Some("rust")).unwrap();
    assert_eq!(receipt(&w).suites(), vec!["rust".to_string(), "shell".to_string()]);
    assert!(!w.stdout().contains("starting a new one"));
}

#[test]
fn a_part_at_another_head_starts_a_new_receipt_and_says_so() {
    let w = world();
    w.receipt(&observed(7, "ffffffffffffffffffffffffffffffffffffffff", "shell", true));
    go(&w, true, Some("rust")).unwrap();
    assert!(w.stdout().contains(
        "  the receipt on disk is for #7 at ffffffff, not this head; starting a new one\n"
    ));
    assert_eq!(receipt(&w).suites(), vec!["rust".to_string()]);
}

#[test]
fn a_part_refuses_to_fold_into_a_receipt_whose_signature_does_not_check_out() {
    let w = world();
    let bytes = String::from_utf8(observed(7, HEAD, "shell", true).to_bytes())
        .unwrap()
        .replace("assertion failed", "edited");
    w.fs.put("/s/receipts/7.json", bytes.into_bytes(), 0);
    assert_eq!(
        go(&w, true, Some("rust")).unwrap_err(),
        Exit::from(&VerifyRefusal::PriorReceiptNotTrusted {
            pr: pr("7"),
            psig: "3".into(),
            receipts: "/s/receipts".into()
        })
    );
}

#[test]
fn a_plain_verify_replaces_a_receipt_on_disk_without_folding() {
    let w = world();
    w.receipt(&observed(7, HEAD, "shell", true));
    go(&w, false, None).unwrap();
    assert_eq!(receipt(&w).suites(), vec!["rust".to_string()]);
    assert_eq!(receipt(&w).field_text("partial", ""), "False");
}

#[test]
fn a_head_that_is_not_the_one_named_refuses_and_removes_the_branch() {
    let w = world();
    w.git.script_rev_parse(Script::always(Ok("ffffffffffffffffffffffffffffffffffffffff".into())));
    refuse(&w, &VerifyRefusal::HeadIsNot { real_head: "f".repeat(40), sha: "0123456".into() });
    assert!(
        w.git
            .calls()
            .contains(&crate::GitCall::DeleteBranch { repo: "/repo".into(), name: "v-7-1".into() })
    );
    assert!(w.sandbox.calls().iter().all(|c| !matches!(c, SandboxCall::Extract { .. })));
}

#[test]
fn a_failed_fetch_of_the_pull_request_ref_refuses() {
    let w = world();
    w.git.script_fetch_ref(Script::always(Err(GitError {
        command: "fetch".into(),
        reason: "x".into(),
    })));
    refuse(&w, &VerifyRefusal::CannotFetch { pr: pr("7") });
}

#[test]
fn a_failed_extraction_prints_the_sandbox_line_then_the_die_and_removes_the_branch() {
    let w = world();
    w.sandbox.script_extract(Script::always(Err(SandboxError::PartialExtract {
        rev: "v-7-1".into(),
        want: 5,
        got: 3,
    })));
    let exit = go(&w, false, None).unwrap_err();
    assert_eq!(
        exit.stderr,
        "extracted 3 of the 5 files in v-7-1\nmaintainer-merge: cannot extract #7; nothing was verified\n"
    );
    assert!(
        w.git
            .calls()
            .contains(&crate::GitCall::DeleteBranch { repo: "/repo".into(), name: "v-7-1".into() })
    );
}

#[test]
fn a_symlink_that_leaves_the_tree_refuses_before_anything_runs_on_the_host() {
    let w = world();
    w.sandbox.script_escapes(Script::always(Ok(vec![Escape {
        link: "h.sh".into(),
        target: "/etc/passwd".into(),
    }])));
    let exit = go(&w, false, None).unwrap_err();
    assert!(exit.stderr.contains("symlink(s) pointing outside itself"), "{exit:?}");
    assert!(exit.stderr.contains("    h.sh -> /etc/passwd"));
    assert!(
        w.sandbox
            .calls()
            .iter()
            .all(|c| !matches!(c, SandboxCall::Mutate { .. } | SandboxCall::Run { .. }))
    );
}

#[test]
fn a_seed_that_fails_prints_a_note_and_carries_on() {
    let w = world();
    w.sandbox.script_seed(Script::always(Err(SandboxError::IndexNotSeeded)));
    go(&w, false, None).unwrap();
    assert!(w.stdout().contains("  note: could not seed a git index in the extracted tree;"));
}

#[test]
fn a_killed_clean_run_is_the_gates_fault_and_never_the_contributors() {
    for code in [137, 143, 124] {
        let w = world();
        w.sandbox.script_run(Script::always(Ok(ContainerExit(code))));
        refuse(&w, &VerifyRefusal::CleanRunKilled { clean_rc: code.to_string(), pr: pr("7") });
    }
}

#[test]
fn a_clean_failure_that_names_a_missing_binary_is_the_suite_environment_not_the_pull_request() {
    let w = world();
    w.sandbox.script_run(Script::always(Ok(ContainerExit(127))));
    w.fs.put(
        format!("{WORK}/clean.log"),
        b"a\nb\nc\nline 1\nsh: git: command not found\nlast\n".to_vec(),
        0,
    );
    refuse(
        &w,
        &VerifyRefusal::FailedOnSuiteEnvironment {
            env_hit: "sh: git: command not found".into(),
            clean_log_tail: "b\nc\nline 1\nsh: git: command not found\nlast".into(),
            pr: pr("7"),
        },
    );
}

#[test]
fn a_clean_failure_with_its_own_assertion_is_the_contributors_and_shows_the_last_five_lines() {
    let w = world();
    w.sandbox.script_run(Script::always(Ok(ContainerExit(101))));
    w.fs.put(format!("{WORK}/clean.log"), b"1\n2\n3\n4\n5\n6\n7\n".to_vec(), 0);
    refuse(
        &w,
        &VerifyRefusal::DoesNotPassUnmutated {
            clean_rc: "101".into(),
            clean_log_tail: "3\n4\n5\n6\n7".into(),
        },
    );
}

#[test]
fn a_filter_that_ran_nothing_is_a_vacuous_pass_and_refuses_with_three_lines() {
    let mut w = world();
    w.suites = FakeSuiteHost::new(
        "/verify.d",
        vec![FakeSuite::new("rust", "rust:1").covering(&["bin/a"]).mutating(&["*.rs"]).ran(0)],
    );
    w.fs.put(format!("{WORK}/clean.log"), b"a\nb\nc\nd\n".to_vec(), 0);
    refuse(
        &w,
        &VerifyRefusal::NoEvidenceItRan {
            filter: "guard".into(),
            clean_log_tail: "b\nc\nd".into(),
        },
    );
}

#[test]
fn a_mutation_that_changed_nothing_says_nothing_about_the_guard() {
    let w = world();
    w.sandbox.script_mutation(Script::always(Ok(Vec::new())));
    let exit = go(&w, false, None).unwrap_err();
    assert!(exit.stderr.starts_with("maintainer-merge: the mutation changed nothing, so this run says nothing about the guard.\n"), "{exit:?}");
    assert!(exit.stderr.contains("    *.rs\n"));
}

#[test]
fn a_sed_mutation_the_sandbox_refused_is_its_own_refusal() {
    let w = world();
    w.sandbox.script_mutation(Script::always(Err(SandboxError::MutationFailed)));
    let exit = go(&w, false, None).unwrap_err();
    assert_eq!(
        exit.stderr,
        "maintainer-merge: mutation failed to apply, or used a sandboxed sed command (e/w/r are refused)\n"
    );
}

#[test]
fn a_mutation_that_landed_only_in_a_test_file_is_refused_when_the_diff_has_production_code() {
    let w = world();
    w.sandbox.script_mutation(Script::always(Ok(vec!["tests/t.rs".into()])));
    refuse(
        &w,
        &VerifyRefusal::LandedOutsideProduction {
            mutated: "tests/t.rs".into(),
            prod_globs: "bin/* lib/*".into(),
            filter: "guard".into(),
        },
    );
}

#[test]
fn a_mutation_of_production_code_the_pull_request_did_not_change_is_refused() {
    let w = world();
    w.sandbox.script_mutation(Script::always(Ok(vec!["lib/b".into()])));
    refuse(
        &w,
        &VerifyRefusal::LandedOnUnchangedProduction {
            mutated: "lib/b".into(),
            changed_prod: "bin/a".into(),
            filter: "guard".into(),
        },
    );
}

#[test]
fn a_mutation_that_hits_one_changed_production_file_among_others_is_accepted() {
    let w = world();
    w.gh.script_changed_files(Script::always(Ok(vec![
        "bin/a".into(),
        "lib/b".into(),
        "docs/x.md".into(),
    ])));
    w.sandbox.script_mutation(Script::always(Ok(vec!["tests/t.rs".into(), "lib/b".into()])));
    go(&w, false, None).unwrap();
}

#[test]
fn a_diff_with_no_production_code_may_mutate_any_file_the_suite_allows() {
    let w = world();
    w.gh.script_changed_files(Script::always(Ok(vec!["docs/x.md".into()])));
    w.sandbox.script_mutation(Script::always(Ok(vec!["tests/t.rs".into()])));
    go(&w, false, None).unwrap();
}

#[test]
fn a_guard_that_still_passes_when_mutated_does_not_bite_and_no_receipt_is_written() {
    let w = world();
    w.sandbox.script_run(Script::always(Ok(ContainerExit(0))));
    refuse(&w, &VerifyRefusal::GuardDoesNotBite { filter: "guard".into() });
    assert_eq!(w.receipt_on_disk(7), None);
}

#[test]
fn a_killed_mutated_run_is_not_a_guard_biting() {
    for code in [137, 143, 124] {
        let w = world();
        w.sandbox.script_run(Script::sequence([Ok(ContainerExit(0)), Ok(ContainerExit(code))]));
        refuse(&w, &VerifyRefusal::MutatedRunKilled { mut_rc: code.to_string() });
        assert_eq!(w.receipt_on_disk(7), None);
    }
}

#[test]
fn every_scratch_directory_is_removed_when_the_verify_ends_whatever_the_outcome() {
    for fail_run in [false, true] {
        let w = world();
        if fail_run {
            w.sandbox.script_run(Script::always(Ok(ContainerExit(0))));
        }
        let outcome = go(&w, false, None);
        assert_eq!(outcome.is_err(), fail_run);
        let removed = w.dirs.removed.lock().unwrap().clone();
        assert_eq!(
            removed,
            vec![PathBuf::from(WORK), PathBuf::from(SCRATCH)],
            "fail_run={fail_run}"
        );
    }
}

#[test]
fn a_scratch_directory_inside_the_extracted_tree_is_refused() {
    let w = world();
    w.sandbox.script_scratch(Script::sequence([
        Ok(PathBuf::from(WORK)),
        Ok(PathBuf::from(format!("{WORK}/inner"))),
    ]));
    refuse(&w, &VerifyRefusal::ScratchInsideTree);
}

#[test]
fn the_receipt_key_is_read_after_the_proof_not_before_preserved() {
    let w = world();
    w.fs.remove(std::path::Path::new("/k/receipt.key")).unwrap();
    let exit = go(&w, false, None).unwrap_err();
    assert!(exit.stderr.contains("no receipt key at /k/receipt.key"));
    assert!(
        w.sandbox.calls().iter().any(|c| matches!(c, SandboxCall::Run { .. })),
        "the proof ran first"
    );
}

#[test]
fn a_suite_that_needs_another_runtime_refuses_before_a_tree_is_extracted() {
    let mut w = world();
    w.suites = FakeSuiteHost::new(
        "/verify.d",
        vec![suite("rust", &["bin/a"]).requiring(mx_suite::Runtime::Docker, "docker")],
    );
    let exit = go(&w, false, None).unwrap_err();
    assert_eq!(
        exit.stderr,
        "maintainer-merge: the 'rust' suite needs docker, and this machine has podman\n"
    );
}
