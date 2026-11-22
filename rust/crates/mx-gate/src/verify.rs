//! The refusals of `verify`, `verify-part` and `show` that no other type carries (bash lines 1221 to 1663), each with its exact text.
//!
//! The verdict logic that raises them (clean run, landing check, mutated run, receipt fold) is the gate
//! agent's (brief p3b).

#![allow(
    clippy::doc_markdown,
    reason = "each variant quotes the bash die line it replaces, with its $variables"
)]

use crate::PrNumber;
use mx_suite::text::indent4;
use thiserror::Error;

/// Every refusal of `verify`, `verify-part` and `show` that `GateDecision`, `SuiteError`, `SandboxError` and
/// `NothingChanged` do not already carry, each with the text of its `die`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VerifyRefusal {
    /// bash line 48: `cannot create $VERIFY_WORK`
    #[error("cannot create {verify_work}")]
    CannotCreateVerifyWork { verify_work: String },
    /// bash line 50: `cannot create a scratch directory in $VERIFY_WORK`
    #[error("cannot create a scratch directory in {verify_work}")]
    CannotCreateScratch { verify_work: String },
    /// bash line 1227: `no container runtime; install podman or docker. No container, no receipt.`
    #[error("no container runtime; install podman or docker. No container, no receipt.")]
    NoContainerRuntime,
    /// bash line 1237: `cannot open the verify lock for #$pr`
    #[error("cannot open the verify lock for #{pr}")]
    CannotOpenVerifyLock { pr: PrNumber },
    /// bash line 1238: `another verify of #$pr is already running in this state directory.`
    #[error(
        "another verify of #{pr} is already running in this state directory.\n  One actor per action, and this one builds the whole workspace twice. Wait for\n  it, or remove {receipts}/.verify-{pr}.lock if you are sure nothing else is\n  running."
    )]
    VerifyLockHeld { pr: PrNumber, receipts: String },
    /// bash line 1244: `could not read the changed files for #$pr`
    #[error("could not read the changed files for #{pr}")]
    ChangedFilesUnreadable { pr: PrNumber },
    /// bash line 1252: `verify-part needs the suite named, because a part cannot be inferred:`
    #[error(
        "verify-part needs the suite named, because a part cannot be inferred:\n    maintainer-merge verify-part {pr} <sha> <test> <mutation> <suite>\n  Suites here: {suites}"
    )]
    PartNeedsSuiteNamed { pr: PrNumber, suites: String },
    /// bash line 1267: `the '$suite' suite runs none of the production paths #$pr changes, so`
    #[error(
        "the '{suite}' suite runs none of the production paths #{pr} changes, so\n  proving it bites here adds nothing a merge re-checks. Name the suite that\n  runs the code you mean to prove."
    )]
    PartRunsNoProduction { suite: String, pr: PrNumber },
    /// bash line 1292: `the suite scratch is inside the extracted tree`
    #[error("the suite scratch is inside the extracted tree")]
    ScratchInsideTree,
    /// bash line 1294: `cannot fetch #$pr`
    #[error("cannot fetch #{pr}")]
    CannotFetch { pr: PrNumber },
    /// bash line 1297: `head is $real_head, not the $sha you named`
    #[error("head is {real_head}, not the {sha} you named")]
    HeadIsNot { real_head: String, sha: String },
    /// bash line 1301: `cannot extract #$pr; nothing was verified`
    #[error("cannot extract #{pr}; nothing was verified")]
    CannotExtract { pr: PrNumber },
    /// bash line 1427: `the dependency fetch failed. That is my container's network or a`
    #[error(
        "the dependency fetch failed. That is my container's network or a\n  lockfile that does not match the manifests, not the pull request's test:\n\n{}",
        indent4(work_tail)
    )]
    PrefetchFailed { work_tail: String },
    /// bash line 1438: `the unmutated run was KILLED (exit $clean_rc), so it never reached a verdict.`
    #[error(
        "the unmutated run was KILLED (exit {clean_rc}), so it never reached a verdict.\n\n  That is my container hitting its memory cap or its timeout, not #{pr}. Raise\n  MAINTAINER_VERIFY_MEMORY or lower MAINTAINER_CARGO_JOBS and run it again.\n  Do NOT report the contributor as failing their own test."
    )]
    CleanRunKilled { clean_rc: String, pr: PrNumber },
    /// bash line 1455: `the unmutated test failed on my suite environment, not the pull request.`
    #[error(
        "the unmutated test failed on my suite environment, not the pull request.\n\n  the line that says so:\n{}\n\n  the last of the run:\n{}\n\n  That is a missing binary or an unexecutable path inside the verify\n  container, so it says nothing about #{pr}. Fix the suite in\n  profiles/*/verify.d/ (suite_image, suite_needs, the mount flags) and run\n  this again. Do NOT report the contributor as failing their own test.",
        indent4(env_hit),
        indent4(clean_log_tail)
    )]
    FailedOnSuiteEnvironment { env_hit: String, clean_log_tail: String, pr: PrNumber },
    /// bash line 1468: `the test does not pass unmutated (rc=$clean_rc); nothing to prove yet`
    #[error(
        "the test does not pass unmutated (rc={clean_rc}); nothing to prove yet\n{}",
        indent4(clean_log_tail)
    )]
    DoesNotPassUnmutated { clean_rc: String, clean_log_tail: String },
    /// bash line 1478: `'$filter' produced no evidence it ran anything; a vacuous pass proves nothing`
    #[error(
        "'{filter}' produced no evidence it ran anything; a vacuous pass proves nothing\n{}",
        indent4(clean_log_tail)
    )]
    NoEvidenceItRan { filter: String, clean_log_tail: String },
    /// bash line 1538: `the mutation landed only outside production code, so this run says`
    #[error(
        "the mutation landed only outside production code, so this run says\n  nothing about the guard.\n\n  changed by the mutation:\n{}\n  none of which matches this profile's PROD_GLOBS:\n    {prod_globs}\n\n  A mutation inside a test proves the test notices its own edit. The receipt is\n  a claim about the production code the test is supposed to hold, so break that\n  instead and watch '{filter}' go red. No receipt written.",
        indent4(mutated)
    )]
    LandedOutsideProduction { mutated: String, prod_globs: String, filter: String },
    /// bash line 1557: `the mutation landed on production code this pull request did not change, so this`
    #[error(
        "the mutation landed on production code this pull request did not change, so this\n  run says nothing about the change.\n\n  changed by the mutation:\n{}\n  production files this pull request changed:\n{}\n\n  Break one of the files the pull request changed and watch '{filter}' go red.\n  No receipt written.",
        indent4(mutated),
        indent4(changed_prod)
    )]
    LandedOnUnchangedProduction { mutated: String, changed_prod: String, filter: String },
    /// bash line 1572: `THE GUARD DOES NOT BITE: '$filter' still passes with the mutation applied.`
    #[error(
        "THE GUARD DOES NOT BITE: '{filter}' still passes with the mutation applied.\n  This is the defect the gate exists for. No receipt written."
    )]
    GuardDoesNotBite { filter: String },
    /// bash line 1579: `the mutated run was KILLED (exit $mut_rc), so nothing was proved.`
    #[error(
        "the mutated run was KILLED (exit {mut_rc}), so nothing was proved.\n\n  A killed container is not a guard biting. Raise MAINTAINER_VERIFY_MEMORY or\n  lower MAINTAINER_CARGO_JOBS and run it again. No receipt written."
    )]
    MutatedRunKilled { mut_rc: String },
    /// bash line 1601: `a receipt for #$pr already exists at this head and its signature does`
    #[error(
        "a receipt for #{pr} already exists at this head and its signature does\n  not check out (code {psig}). Refusing to fold a new proof into it and re-seal\n  it, because the seal is the only reason merge believes what it says. Delete\n  {receipts}/{pr}.json and prove each part again."
    )]
    PriorReceiptNotTrusted { pr: PrNumber, psig: String, receipts: String },
    /// bash line 1658: `no receipt for #$1`
    #[error("no receipt for #{arg1}")]
    NoReceiptToShow { arg1: String },
    /// bash line 1251 (PB-1): the filter is spliced into `sh -c`, so it is an allow-list
    #[error(
        "the test filter may hold only letters, digits and _ . : / - and spaces, and may not start with -: '{filter}'"
    )]
    TestFilterRefused { filter: String },
}

impl VerifyRefusal {
    /// The process exit code.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::CannotCreateVerifyWork { .. }
            | Self::CannotCreateScratch { .. }
            | Self::NoContainerRuntime
            | Self::CannotOpenVerifyLock { .. }
            | Self::VerifyLockHeld { .. }
            | Self::ChangedFilesUnreadable { .. }
            | Self::PartNeedsSuiteNamed { .. }
            | Self::PartRunsNoProduction { .. }
            | Self::ScratchInsideTree
            | Self::CannotFetch { .. }
            | Self::HeadIsNot { .. }
            | Self::CannotExtract { .. }
            | Self::PrefetchFailed { .. }
            | Self::CleanRunKilled { .. }
            | Self::FailedOnSuiteEnvironment { .. }
            | Self::DoesNotPassUnmutated { .. }
            | Self::NoEvidenceItRan { .. }
            | Self::LandedOutsideProduction { .. }
            | Self::LandedOnUnchangedProduction { .. }
            | Self::GuardDoesNotBite { .. }
            | Self::MutatedRunKilled { .. }
            | Self::PriorReceiptNotTrusted { .. }
            | Self::NoReceiptToShow { .. }
            | Self::TestFilterRefused { .. } => 1,
        }
    }
}

#[cfg(test)]
pub(crate) fn samples() -> Vec<(VerifyRefusal, String)> {
    vec![
            (VerifyRefusal::CannotCreateVerifyWork { verify_work: "⟦verify_work⟧".to_string() }, "cannot create ⟦verify_work⟧".to_string()),
            (VerifyRefusal::CannotCreateScratch { verify_work: "⟦verify_work⟧".to_string() }, "cannot create a scratch directory in ⟦verify_work⟧".to_string()),
            (VerifyRefusal::NoContainerRuntime, "no container runtime; install podman or docker. No container, no receipt.".to_string()),
            (VerifyRefusal::CannotOpenVerifyLock { pr: PrNumber::parse("7").unwrap() }, "cannot open the verify lock for #7".to_string()),
            (VerifyRefusal::VerifyLockHeld { pr: PrNumber::parse("7").unwrap(), receipts: "⟦receipts⟧".to_string() }, "another verify of #7 is already running in this state directory.\n  One actor per action, and this one builds the whole workspace twice. Wait for\n  it, or remove ⟦receipts⟧/.verify-7.lock if you are sure nothing else is\n  running.".to_string()),
            (VerifyRefusal::ChangedFilesUnreadable { pr: PrNumber::parse("7").unwrap() }, "could not read the changed files for #7".to_string()),
            (VerifyRefusal::PartNeedsSuiteNamed { pr: PrNumber::parse("7").unwrap(), suites: "⟦suites⟧".to_string() }, "verify-part needs the suite named, because a part cannot be inferred:\n    maintainer-merge verify-part 7 <sha> <test> <mutation> <suite>\n  Suites here: ⟦suites⟧".to_string()),
            (VerifyRefusal::PartRunsNoProduction { suite: "⟦suite⟧".to_string(), pr: PrNumber::parse("7").unwrap() }, "the '⟦suite⟧' suite runs none of the production paths #7 changes, so\n  proving it bites here adds nothing a merge re-checks. Name the suite that\n  runs the code you mean to prove.".to_string()),
            (VerifyRefusal::ScratchInsideTree, "the suite scratch is inside the extracted tree".to_string()),
            (VerifyRefusal::CannotFetch { pr: PrNumber::parse("7").unwrap() }, "cannot fetch #7".to_string()),
            (VerifyRefusal::HeadIsNot { real_head: "⟦real_head⟧".to_string(), sha: "⟦sha⟧".to_string() }, "head is ⟦real_head⟧, not the ⟦sha⟧ you named".to_string()),
            (VerifyRefusal::CannotExtract { pr: PrNumber::parse("7").unwrap() }, "cannot extract #7; nothing was verified".to_string()),
            (VerifyRefusal::PrefetchFailed { work_tail: "⟦work_tail⟧".to_string() }, "the dependency fetch failed. That is my container's network or a\n  lockfile that does not match the manifests, not the pull request's test:\n\n    ⟦work_tail⟧".to_string()),
            (VerifyRefusal::CleanRunKilled { clean_rc: "⟦clean_rc⟧".to_string(), pr: PrNumber::parse("7").unwrap() }, "the unmutated run was KILLED (exit ⟦clean_rc⟧), so it never reached a verdict.\n\n  That is my container hitting its memory cap or its timeout, not #7. Raise\n  MAINTAINER_VERIFY_MEMORY or lower MAINTAINER_CARGO_JOBS and run it again.\n  Do NOT report the contributor as failing their own test.".to_string()),
            (VerifyRefusal::FailedOnSuiteEnvironment { env_hit: "⟦env_hit⟧".to_string(), clean_log_tail: "⟦clean_log_tail⟧".to_string(), pr: PrNumber::parse("7").unwrap() }, "the unmutated test failed on my suite environment, not the pull request.\n\n  the line that says so:\n    ⟦env_hit⟧\n\n  the last of the run:\n    ⟦clean_log_tail⟧\n\n  That is a missing binary or an unexecutable path inside the verify\n  container, so it says nothing about #7. Fix the suite in\n  profiles/*/verify.d/ (suite_image, suite_needs, the mount flags) and run\n  this again. Do NOT report the contributor as failing their own test.".to_string()),
            (VerifyRefusal::DoesNotPassUnmutated { clean_rc: "⟦clean_rc⟧".to_string(), clean_log_tail: "⟦clean_log_tail⟧".to_string() }, "the test does not pass unmutated (rc=⟦clean_rc⟧); nothing to prove yet\n    ⟦clean_log_tail⟧".to_string()),
            (VerifyRefusal::NoEvidenceItRan { filter: "⟦filter⟧".to_string(), clean_log_tail: "⟦clean_log_tail⟧".to_string() }, "'⟦filter⟧' produced no evidence it ran anything; a vacuous pass proves nothing\n    ⟦clean_log_tail⟧".to_string()),
            (VerifyRefusal::LandedOutsideProduction { mutated: "⟦mutated⟧".to_string(), prod_globs: "⟦prod_globs⟧".to_string(), filter: "⟦filter⟧".to_string() }, "the mutation landed only outside production code, so this run says\n  nothing about the guard.\n\n  changed by the mutation:\n    ⟦mutated⟧\n  none of which matches this profile's PROD_GLOBS:\n    ⟦prod_globs⟧\n\n  A mutation inside a test proves the test notices its own edit. The receipt is\n  a claim about the production code the test is supposed to hold, so break that\n  instead and watch '⟦filter⟧' go red. No receipt written.".to_string()),
            (VerifyRefusal::LandedOnUnchangedProduction { mutated: "⟦mutated⟧".to_string(), changed_prod: "⟦changed_prod⟧".to_string(), filter: "⟦filter⟧".to_string() }, "the mutation landed on production code this pull request did not change, so this\n  run says nothing about the change.\n\n  changed by the mutation:\n    ⟦mutated⟧\n  production files this pull request changed:\n    ⟦changed_prod⟧\n\n  Break one of the files the pull request changed and watch '⟦filter⟧' go red.\n  No receipt written.".to_string()),
            (VerifyRefusal::GuardDoesNotBite { filter: "⟦filter⟧".to_string() }, "THE GUARD DOES NOT BITE: '⟦filter⟧' still passes with the mutation applied.\n  This is the defect the gate exists for. No receipt written.".to_string()),
            (VerifyRefusal::MutatedRunKilled { mut_rc: "⟦mut_rc⟧".to_string() }, "the mutated run was KILLED (exit ⟦mut_rc⟧), so nothing was proved.\n\n  A killed container is not a guard biting. Raise MAINTAINER_VERIFY_MEMORY or\n  lower MAINTAINER_CARGO_JOBS and run it again. No receipt written.".to_string()),
            (VerifyRefusal::PriorReceiptNotTrusted { pr: PrNumber::parse("7").unwrap(), psig: "⟦psig⟧".to_string(), receipts: "⟦receipts⟧".to_string() }, "a receipt for #7 already exists at this head and its signature does\n  not check out (code ⟦psig⟧). Refusing to fold a new proof into it and re-seal\n  it, because the seal is the only reason merge believes what it says. Delete\n  ⟦receipts⟧/7.json and prove each part again.".to_string()),
            (VerifyRefusal::NoReceiptToShow { arg1: "⟦arg1⟧".to_string() }, "no receipt for #⟦arg1⟧".to_string()),
            (VerifyRefusal::TestFilterRefused { filter: "⟦filter⟧".to_string() }, "the test filter may hold only letters, digits and _ . : / - and spaces, and may not start with -: '⟦filter⟧'".to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The position of a variant in `samples()`; no `_` arm, so a new variant fails to
    /// compile here until it has an index and a row.
    fn rank(value: &VerifyRefusal) -> usize {
        match value {
            VerifyRefusal::CannotCreateVerifyWork { .. } => 0,
            VerifyRefusal::CannotCreateScratch { .. } => 1,
            VerifyRefusal::NoContainerRuntime => 2,
            VerifyRefusal::CannotOpenVerifyLock { .. } => 3,
            VerifyRefusal::VerifyLockHeld { .. } => 4,
            VerifyRefusal::ChangedFilesUnreadable { .. } => 5,
            VerifyRefusal::PartNeedsSuiteNamed { .. } => 6,
            VerifyRefusal::PartRunsNoProduction { .. } => 7,
            VerifyRefusal::ScratchInsideTree => 8,
            VerifyRefusal::CannotFetch { .. } => 9,
            VerifyRefusal::HeadIsNot { .. } => 10,
            VerifyRefusal::CannotExtract { .. } => 11,
            VerifyRefusal::PrefetchFailed { .. } => 12,
            VerifyRefusal::CleanRunKilled { .. } => 13,
            VerifyRefusal::FailedOnSuiteEnvironment { .. } => 14,
            VerifyRefusal::DoesNotPassUnmutated { .. } => 15,
            VerifyRefusal::NoEvidenceItRan { .. } => 16,
            VerifyRefusal::LandedOutsideProduction { .. } => 17,
            VerifyRefusal::LandedOnUnchangedProduction { .. } => 18,
            VerifyRefusal::GuardDoesNotBite { .. } => 19,
            VerifyRefusal::MutatedRunKilled { .. } => 20,
            VerifyRefusal::PriorReceiptNotTrusted { .. } => 21,
            VerifyRefusal::NoReceiptToShow { .. } => 22,
            VerifyRefusal::TestFilterRefused { .. } => 23,
        }
    }

    #[test]
    fn every_variant_has_exactly_one_sample() {
        let table = samples();
        assert_eq!(table.len(), 24, "one row per variant");
        for (at, (value, _)) in table.iter().enumerate() {
            assert_eq!(rank(value), at, "row {at} is out of order: {value:?}");
        }
    }

    #[test]
    fn every_variant_prints_the_text_of_its_bash_line() {
        for (value, bash) in samples() {
            assert_eq!(value.to_string(), bash, "{value:?}");
        }
    }

    #[test]
    fn every_static_line_is_a_line_of_a_die_or_say_in_the_bash_gate() {
        let texts: Vec<String> = samples().into_iter().map(|(_, text)| text).collect();
        crate::anchor::check_static_lines(&texts);
    }

    #[test]
    fn exit_codes_are_the_ones_of_the_bash() {
        let mut seen = std::collections::BTreeSet::new();
        for (value, _) in samples() {
            seen.insert(value.exit_code());
        }
        assert_eq!(seen, std::collections::BTreeSet::from([1]));
    }
}
