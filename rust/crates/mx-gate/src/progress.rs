//! The `say` lines of `maintainer-merge`, each with its exact text (without the two-space indent `say` adds).

#![allow(
    clippy::doc_markdown,
    reason = "each variant quotes the bash die line it replaces, with its $variables"
)]

use crate::PrNumber;
use thiserror::Error;

/// The lines `maintainer-merge` says on standard output (bash `say`, which prints two spaces and the text).
/// A recorder diffs standard output, so the text of each is part of the contract.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Progress {
    /// bash line 293: `the receipt is a DEPENDENCY receipt: nothing but dependency metadata`
    #[error(
        "the receipt is a DEPENDENCY receipt: nothing but dependency metadata\n     changed, and every action pin was checked against its tag"
    )]
    DependencyReceiptNotice,
    /// bash line 295: `the receipt is ASSERTED, not observed: it records a claim a`
    #[error(
        "the receipt is ASSERTED, not observed: it records a claim a\n     human made, and nothing here checked it"
    )]
    AssertedReceiptNotice,
    /// bash line 381: `head moved $verified_head -> ${head:0:8}, $how; receipt still applies`
    #[error("head moved {verified_head} -> {head_8}, {how}; receipt still applies")]
    HeadMoved { verified_head: String, head_8: String, how: String },
    /// bash line 424: `receipt recorded for #$pr at $sha (asserted, not observed)`
    #[error("receipt recorded for #{pr} at {sha} (asserted, not observed)")]
    AssertedReceiptRecorded { pr: PrNumber, sha: String },
    /// bash line 561: `pin: $action ${pinsha:0:12} is reachable from $tag (GitHub agrees)`
    #[error("pin: {action} {pinsha_12} is reachable from {tag} (GitHub agrees)")]
    PinOnBranch { action: String, pinsha_12: String, tag: String },
    /// bash line 598: `pin: $action $tag = ${pinsha:0:12} (GitHub agrees)`
    #[error("pin: {action} {tag} = {pinsha_12} (GitHub agrees)")]
    PinOnTag { action: String, tag: String, pinsha_12: String },
    /// bash line 622: `image: $image:$tag = ${digest:7:19} (the registry agrees)`
    #[error("image: {image}:{tag} = {digest} (the registry agrees)")]
    ImageAgrees { image: String, tag: String, digest: String },
    /// bash line 640: `pip: $pkg==$version, $answer (PyPI agrees)`
    #[error("pip: {pkg}=={version}, {answer} (PyPI agrees)")]
    PipAgrees { pkg: String, version: String, answer: String },
    /// bash line 695: `pip file: $pf read whole at ${merge_base:0:8} and ${real_head:0:8} (same bump as the diff)`
    #[error(
        "pip file: {pf} read whole at {merge_base_8} and {real_head_8} (same bump as the diff)"
    )]
    PipFileAgrees { pf: String, merge_base_8: String, real_head_8: String },
    /// bash line 722: `dependency receipt recorded for #$pr at ${real_head:0:8} ($lines line(s), $checked pin(s), $images_c`
    #[error(
        "dependency receipt recorded for #{pr} at {real_head_8} ({lines} line(s), {checked} pin(s), {images_checked} image(s), {pip_checked} pip package(s) verified)"
    )]
    DependencyReceiptRecorded {
        pr: PrNumber,
        real_head_8: String,
        lines: String,
        checked: String,
        images_checked: String,
        pip_checked: String,
    },
    /// bash line 1280: `suite: $suite ($(suite_image) under $rt)`
    #[error("suite: {suite} ({suite_image} under {rt})")]
    SuiteChosen { suite: String, suite_image: String, rt: String },
    /// bash line 1327: `note: could not seed a git index in the extracted tree; a test that`
    #[error(
        "note: could not seed a git index in the extracted tree; a test that\n  asks git what is tracked will fail for that reason and not for the code's."
    )]
    NoGitIndex,
    /// bash line 1414: `fetching what the lockfile pins (network, no build scripts, --locked)`
    #[error("fetching what the lockfile pins (network, no build scripts, --locked)")]
    FetchingLockfile,
    /// bash line 1434: `running '$filter' unmutated`
    #[error("running '{filter}' unmutated")]
    RunningUnmutated { filter: String },
    /// bash line 1480: `$suite suite: evidence of $ran executed unit(s), passing unmutated`
    #[error("{suite} suite: evidence of {ran} executed unit(s), passing unmutated")]
    EvidenceOfUnits { suite: String, ran: String },
    /// bash line 1482: `applying the mutation and re-running`
    #[error("applying the mutation and re-running")]
    ApplyingMutation,
    /// bash line 1607: `the receipt on disk is for #${prior_pr:-?} at ${prior_head:0:8}, not this head; starting a new one`
    #[error(
        "the receipt on disk is for #{prior_pr} at {prior_head_8}, not this head; starting a new one"
    )]
    ReceiptOnDiskIsForOther { prior_pr: String, prior_head_8: String },
    /// bash line 1653: `receipt recorded for #$pr at ${real_head:0:8} (observed: clean pass, mutated fail)`
    #[error("receipt recorded for #{pr} at {real_head_8} (observed: clean pass, mutated fail)")]
    ObservedReceiptRecorded { pr: PrNumber, real_head_8: String },
    /// bash line 1661: `no receipts recorded`
    #[error("no receipts recorded")]
    NoReceiptsRecorded,
    /// bash line 2024: `#$pr: approved, $pass_n checks pass, $skip_n skipped, 0 fail, 0 pending, 0 cancelled, receipt valid`
    #[error(
        "#{pr}: approved, {pass_n} checks pass, {skip_n} skipped, 0 fail, 0 pending, 0 cancelled, receipt valid"
    )]
    AllConditionsHold { pr: PrNumber, pass_n: String, skip_n: String },
    /// bash line 2026: `POST=off: every condition holds and the merge is NOT being made`
    #[error("POST=off: every condition holds and the merge is NOT being made")]
    RehearsalMergeNotMade,
    /// bash line 2027: `         re-run with posting on to land it`
    #[error("         re-run with posting on to land it")]
    RehearsalMergeHint,
    /// bash line 2035: `merged #$pr`
    #[error("merged #{pr}")]
    Merged { pr: PrNumber },
    /// bash line 2040: `#$pr closes no issue, so nothing was closed by this merge.`
    #[error("#{pr} closes no issue, so nothing was closed by this merge.")]
    ClosesNothing { pr: PrNumber },
    /// bash line 2041: `  If it resolves one, close it by hand and say which commit did it.`
    #[error("  If it resolves one, close it by hand and say which commit did it.")]
    ClosesNothingHint,
    /// bash line 2141: `#$pr is $state: nothing to update`
    #[error("#{pr} is {state}: nothing to update")]
    NothingToUpdate { pr: PrNumber, state: String },
    /// bash line 2172: `#$pr: BEHIND, approved by $ACCOUNT at ${head:0:8}, receipt valid, no other lease`
    #[error("#{pr}: BEHIND, approved by {account} at {head_8}, receipt valid, no other lease")]
    UpdateConditionsHold { pr: PrNumber, account: String, head_8: String },
    /// bash line 2175: `POST=off: every condition holds and the branch is NOT being updated`
    #[error("POST=off: every condition holds and the branch is NOT being updated")]
    RehearsalUpdateNotMade,
    /// bash line 2176: `         it would run: gh api -X PUT repos/$REPO_SLUG/pulls/$pr/update-branch -f expected_head_sha=$`
    #[error(
        "         it would run: gh api -X PUT repos/{repo_slug}/pulls/{pr}/update-branch -f expected_head_sha={head}"
    )]
    RehearsalUpdateWouldRun { repo_slug: String, pr: PrNumber, head: String },
    /// bash line 2218: `#$pr updated: new head ${nhead:0:8} (was ${head:0:8}), mergeStateStatus $nstate`
    #[error("#{pr} updated: new head {nhead_8} (was {head_8}), mergeStateStatus {nstate}")]
    Updated { pr: PrNumber, nhead_8: String, head_8: String, nstate: String },
    /// bash line 2219: `the required checks must re-run on the new head, and \'maintainer-merge merge $pr\' refuses until th`
    #[error(
        "the required checks must re-run on the new head, and `maintainer-merge merge {pr}` refuses until they are green"
    )]
    ChecksMustRerun { pr: PrNumber },
    /// bash line 2221: `reviewDecision is still APPROVED after the update`
    #[error("reviewDecision is still APPROVED after the update")]
    ApprovalKept,
    /// bash line 2223: `reviewDecision is now '${ndecision:-empty}': the update dismissed the approval.`
    #[error("reviewDecision is now '{ndecision}': the update dismissed the approval.")]
    ApprovalDismissed { ndecision: String },
    /// bash line 2224: `The review must be re-posted at ${nhead:0:8}. That is allowed only if \'git diff\', three-dot / tree`
    #[error(
        "The review must be re-posted at {nhead_8}. That is allowed only if `git diff`, three-dot / tree-merge,"
    )]
    RepostReview1 { nhead_8: String },
    /// bash line 2225: `of the verified head ${RECEIPT_HEAD:0:8} against ${nhead:0:8} shows no production or workflow change`
    #[error(
        "of the verified head {receipt_head_8} against {nhead_8} shows no production or workflow change;"
    )]
    RepostReview2 { receipt_head_8: String, nhead_8: String },
    /// bash line 2226: `this command approves nothing. Read the diff, then decide.`
    #[error("this command approves nothing. Read the diff, then decide.")]
    RepostReview3,
}

#[cfg(test)]
pub(crate) fn samples() -> Vec<(Progress, String)> {
    vec![
            (Progress::DependencyReceiptNotice, "the receipt is a DEPENDENCY receipt: nothing but dependency metadata\n     changed, and every action pin was checked against its tag".to_string()),
            (Progress::AssertedReceiptNotice, "the receipt is ASSERTED, not observed: it records a claim a\n     human made, and nothing here checked it".to_string()),
            (Progress::HeadMoved { verified_head: "⟦verified_head⟧".to_string(), head_8: "⟦head_8⟧".to_string(), how: "⟦how⟧".to_string() }, "head moved ⟦verified_head⟧ -> ⟦head_8⟧, ⟦how⟧; receipt still applies".to_string()),
            (Progress::AssertedReceiptRecorded { pr: PrNumber::parse("7").unwrap(), sha: "⟦sha⟧".to_string() }, "receipt recorded for #7 at ⟦sha⟧ (asserted, not observed)".to_string()),
            (Progress::PinOnBranch { action: "⟦action⟧".to_string(), pinsha_12: "⟦pinsha_12⟧".to_string(), tag: "⟦tag⟧".to_string() }, "pin: ⟦action⟧ ⟦pinsha_12⟧ is reachable from ⟦tag⟧ (GitHub agrees)".to_string()),
            (Progress::PinOnTag { action: "⟦action⟧".to_string(), tag: "⟦tag⟧".to_string(), pinsha_12: "⟦pinsha_12⟧".to_string() }, "pin: ⟦action⟧ ⟦tag⟧ = ⟦pinsha_12⟧ (GitHub agrees)".to_string()),
            (Progress::ImageAgrees { image: "⟦image⟧".to_string(), tag: "⟦tag⟧".to_string(), digest: "⟦digest⟧".to_string() }, "image: ⟦image⟧:⟦tag⟧ = ⟦digest⟧ (the registry agrees)".to_string()),
            (Progress::PipAgrees { pkg: "⟦pkg⟧".to_string(), version: "⟦version⟧".to_string(), answer: "⟦answer⟧".to_string() }, "pip: ⟦pkg⟧==⟦version⟧, ⟦answer⟧ (PyPI agrees)".to_string()),
            (Progress::PipFileAgrees { pf: "⟦pf⟧".to_string(), merge_base_8: "⟦merge_base_8⟧".to_string(), real_head_8: "⟦real_head_8⟧".to_string() }, "pip file: ⟦pf⟧ read whole at ⟦merge_base_8⟧ and ⟦real_head_8⟧ (same bump as the diff)".to_string()),
            (Progress::DependencyReceiptRecorded { pr: PrNumber::parse("7").unwrap(), real_head_8: "⟦real_head_8⟧".to_string(), lines: "⟦lines⟧".to_string(), checked: "⟦checked⟧".to_string(), images_checked: "⟦images_checked⟧".to_string(), pip_checked: "⟦pip_checked⟧".to_string() }, "dependency receipt recorded for #7 at ⟦real_head_8⟧ (⟦lines⟧ line(s), ⟦checked⟧ pin(s), ⟦images_checked⟧ image(s), ⟦pip_checked⟧ pip package(s) verified)".to_string()),
            (Progress::SuiteChosen { suite: "⟦suite⟧".to_string(), suite_image: "⟦suite_image⟧".to_string(), rt: "⟦rt⟧".to_string() }, "suite: ⟦suite⟧ (⟦suite_image⟧ under ⟦rt⟧)".to_string()),
            (Progress::NoGitIndex, "note: could not seed a git index in the extracted tree; a test that\n  asks git what is tracked will fail for that reason and not for the code's.".to_string()),
            (Progress::FetchingLockfile, "fetching what the lockfile pins (network, no build scripts, --locked)".to_string()),
            (Progress::RunningUnmutated { filter: "⟦filter⟧".to_string() }, "running '⟦filter⟧' unmutated".to_string()),
            (Progress::EvidenceOfUnits { suite: "⟦suite⟧".to_string(), ran: "⟦ran⟧".to_string() }, "⟦suite⟧ suite: evidence of ⟦ran⟧ executed unit(s), passing unmutated".to_string()),
            (Progress::ApplyingMutation, "applying the mutation and re-running".to_string()),
            (Progress::ReceiptOnDiskIsForOther { prior_pr: "⟦prior_pr⟧".to_string(), prior_head_8: "⟦prior_head_8⟧".to_string() }, "the receipt on disk is for #⟦prior_pr⟧ at ⟦prior_head_8⟧, not this head; starting a new one".to_string()),
            (Progress::ObservedReceiptRecorded { pr: PrNumber::parse("7").unwrap(), real_head_8: "⟦real_head_8⟧".to_string() }, "receipt recorded for #7 at ⟦real_head_8⟧ (observed: clean pass, mutated fail)".to_string()),
            (Progress::NoReceiptsRecorded, "no receipts recorded".to_string()),
            (Progress::AllConditionsHold { pr: PrNumber::parse("7").unwrap(), pass_n: "⟦pass_n⟧".to_string(), skip_n: "⟦skip_n⟧".to_string() }, "#7: approved, ⟦pass_n⟧ checks pass, ⟦skip_n⟧ skipped, 0 fail, 0 pending, 0 cancelled, receipt valid".to_string()),
            (Progress::RehearsalMergeNotMade, "POST=off: every condition holds and the merge is NOT being made".to_string()),
            (Progress::RehearsalMergeHint, "         re-run with posting on to land it".to_string()),
            (Progress::Merged { pr: PrNumber::parse("7").unwrap() }, "merged #7".to_string()),
            (Progress::ClosesNothing { pr: PrNumber::parse("7").unwrap() }, "#7 closes no issue, so nothing was closed by this merge.".to_string()),
            (Progress::ClosesNothingHint, "  If it resolves one, close it by hand and say which commit did it.".to_string()),
            (Progress::NothingToUpdate { pr: PrNumber::parse("7").unwrap(), state: "⟦state⟧".to_string() }, "#7 is ⟦state⟧: nothing to update".to_string()),
            (Progress::UpdateConditionsHold { pr: PrNumber::parse("7").unwrap(), account: "⟦account⟧".to_string(), head_8: "⟦head_8⟧".to_string() }, "#7: BEHIND, approved by ⟦account⟧ at ⟦head_8⟧, receipt valid, no other lease".to_string()),
            (Progress::RehearsalUpdateNotMade, "POST=off: every condition holds and the branch is NOT being updated".to_string()),
            (Progress::RehearsalUpdateWouldRun { repo_slug: "⟦repo_slug⟧".to_string(), pr: PrNumber::parse("7").unwrap(), head: "⟦head⟧".to_string() }, "         it would run: gh api -X PUT repos/⟦repo_slug⟧/pulls/7/update-branch -f expected_head_sha=⟦head⟧".to_string()),
            (Progress::Updated { pr: PrNumber::parse("7").unwrap(), nhead_8: "⟦nhead_8⟧".to_string(), head_8: "⟦head_8⟧".to_string(), nstate: "⟦nstate⟧".to_string() }, "#7 updated: new head ⟦nhead_8⟧ (was ⟦head_8⟧), mergeStateStatus ⟦nstate⟧".to_string()),
            (Progress::ChecksMustRerun { pr: PrNumber::parse("7").unwrap() }, "the required checks must re-run on the new head, and `maintainer-merge merge 7` refuses until they are green".to_string()),
            (Progress::ApprovalKept, "reviewDecision is still APPROVED after the update".to_string()),
            (Progress::ApprovalDismissed { ndecision: "⟦ndecision⟧".to_string() }, "reviewDecision is now '⟦ndecision⟧': the update dismissed the approval.".to_string()),
            (Progress::RepostReview1 { nhead_8: "⟦nhead_8⟧".to_string() }, "The review must be re-posted at ⟦nhead_8⟧. That is allowed only if `git diff`, three-dot / tree-merge,".to_string()),
            (Progress::RepostReview2 { receipt_head_8: "⟦receipt_head_8⟧".to_string(), nhead_8: "⟦nhead_8⟧".to_string() }, "of the verified head ⟦receipt_head_8⟧ against ⟦nhead_8⟧ shows no production or workflow change;".to_string()),
            (Progress::RepostReview3, "this command approves nothing. Read the diff, then decide.".to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The position of a variant in `samples()`; no `_` arm, so a new variant fails to
    /// compile here until it has an index and a row.
    fn rank(value: &Progress) -> usize {
        match value {
            Progress::DependencyReceiptNotice => 0,
            Progress::AssertedReceiptNotice => 1,
            Progress::HeadMoved { .. } => 2,
            Progress::AssertedReceiptRecorded { .. } => 3,
            Progress::PinOnBranch { .. } => 4,
            Progress::PinOnTag { .. } => 5,
            Progress::ImageAgrees { .. } => 6,
            Progress::PipAgrees { .. } => 7,
            Progress::PipFileAgrees { .. } => 8,
            Progress::DependencyReceiptRecorded { .. } => 9,
            Progress::SuiteChosen { .. } => 10,
            Progress::NoGitIndex => 11,
            Progress::FetchingLockfile => 12,
            Progress::RunningUnmutated { .. } => 13,
            Progress::EvidenceOfUnits { .. } => 14,
            Progress::ApplyingMutation => 15,
            Progress::ReceiptOnDiskIsForOther { .. } => 16,
            Progress::ObservedReceiptRecorded { .. } => 17,
            Progress::NoReceiptsRecorded => 18,
            Progress::AllConditionsHold { .. } => 19,
            Progress::RehearsalMergeNotMade => 20,
            Progress::RehearsalMergeHint => 21,
            Progress::Merged { .. } => 22,
            Progress::ClosesNothing { .. } => 23,
            Progress::ClosesNothingHint => 24,
            Progress::NothingToUpdate { .. } => 25,
            Progress::UpdateConditionsHold { .. } => 26,
            Progress::RehearsalUpdateNotMade => 27,
            Progress::RehearsalUpdateWouldRun { .. } => 28,
            Progress::Updated { .. } => 29,
            Progress::ChecksMustRerun { .. } => 30,
            Progress::ApprovalKept => 31,
            Progress::ApprovalDismissed { .. } => 32,
            Progress::RepostReview1 { .. } => 33,
            Progress::RepostReview2 { .. } => 34,
            Progress::RepostReview3 => 35,
        }
    }

    #[test]
    fn every_variant_has_exactly_one_sample() {
        let table = samples();
        assert_eq!(table.len(), 36, "one row per variant");
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
}
