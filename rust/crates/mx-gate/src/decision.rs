//! Every refusal `maintainer-merge merge` prints, each with its exact text.
//!
//! The text of a variant is the argument the bash passes to `die`, byte for byte
//! (embedded newlines and the two-space indentation included), without the
//! `maintainer-merge: ` prefix `die` adds and without the newline it ends with.
//! [`GateDecision::stderr_line`] adds both. Each variant's comment quotes its
//! bash line. Where the same sentence is built by a helper both `merge` and
//! `update` call (`lease_gate`, `require_prod_globs`, `identity`, `receipt_check`,
//! `receipt_covers`, `receipt_fresh`), the variant is here too.
//!
//! What this enum does NOT hold: the refusals of `verify`, `verify-part`,
//! `verify-deps`, `receipt`, `show` and the `update` branch of
//! `maintainer-merge update`. Those belong to the code that ports them (`update`
//! to the gate agent, `verify-deps` to [`crate::deps`]).
//!
//! Every variant but the two marked `new` exists in the bash. The new ones are
//! decisions of this port: [`GateDecision::StalenessUnreadable`] is F1, and
//! [`GateDecision::EvidenceForOtherPr`] has no bash line because the bash has no
//! typed seam there.

#![allow(
    clippy::doc_markdown,
    reason = "each variant quotes the bash die line it replaces, with its $variables"
)]

use crate::{IssueNumber, PrNumber};
use mx_suite::text::indent4;
use std::path::PathBuf;
use thiserror::Error;

/// The program name that `die` prefixes and that two messages mention.
pub const PROGRAM: &str = "maintainer-merge";

/// One name per line in, `a, b` out: bash `join_names`.
fn join_names(names: &[String]) -> String {
    names.join(", ")
}

/// A refusal of `maintainer-merge merge`, or of a step it shares with `update`.
///
/// Every variant exits 1 ([`GateDecision::exit_code`]). A refusal is a value:
/// the code that decides returns it, and only the binary's `main` prints it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GateDecision {
    // ----- the command line -----
    /// bash `pr_or_die`: die "pull request must be a positive integer, not '$1'"
    ///
    /// Zero and a number above `u64::MAX` are refused too (P30-1): the bash
    /// message says positive, and its check is digits only.
    #[error("pull request must be a positive integer, not '{given}'")]
    InvalidPrNumber { given: String },

    /// bash `cmd_verify`, `cmd_verify_deps` and `cmd_receipt`:
    /// die "head sha looks truncated: $sha"
    #[error("head sha looks truncated: {sha}")]
    HeadTruncated { sha: String },

    // ----- the lease: `maintainer lease check` -----
    /// bash `lease_gate`, no helper:
    /// die "cannot tell whether #$1 is leased, so I will not $2 it:
    ///   no \`maintainer\` helper beside $(basename "${BASH_SOURCE[0]}") or on PATH.
    ///   Install the agent (./install.sh) or put \`maintainer\` on PATH, then retry."
    #[error(
        "cannot tell whether #{pr} is leased, so I will not {action} it:\n  no `maintainer` helper beside {PROGRAM} or on PATH.\n  Install the agent (./install.sh) or put `maintainer` on PATH, then retry."
    )]
    LeaseHelperMissing { pr: PrNumber, action: Action },

    /// bash `lease_gate`, status 1:
    /// die "will not $2 #$1: another actor has leased it.
    ///   $out
    ///   Ask them, wait for it to expire, or have them \`maintainer lease release $1\`."
    #[error(
        "will not {action} #{pr}: another actor has leased it.\n  {output}\n  Ask them, wait for it to expire, or have them `maintainer lease release {pr}`."
    )]
    LeaseHeld { pr: PrNumber, action: Action, output: String },

    /// bash `lease_gate`, any other status:
    /// die "could not read the lease on #$1, so I will not $2 it:
    ///   $out"
    #[error("could not read the lease on #{pr}, so I will not {action} it:\n  {output}")]
    LeaseUnreadable { pr: PrNumber, action: Action, output: String },

    // ----- the profile and the account -----
    /// bash `require_prod_globs`; the text is [`mx_suite::NoProdGlobs`]'s.
    #[error(
        "this profile declares no PROD_GLOBS, so nothing can say\n  whether production code moved since the receipt. Set it in profile.env, for example:\n    PROD_GLOBS=\"bin/* lib/* scripts/* .github/*\""
    )]
    NoProdGlobs,

    /// bash `identity`:
    /// die "gh could not confirm an account (gh exited $rc); if the token was rejected, run gh auth login -h github.com as $ACCOUNT"
    #[error(
        "gh could not confirm an account (gh exited {exit}); if the token was rejected, run gh auth login -h github.com as {account}"
    )]
    IdentityUnconfirmed { account: String, exit: i32 },

    /// bash `identity`: die "gh is authenticated as '${who:-unknown}', not $ACCOUNT"
    #[error("gh is authenticated as '{who}', not {account}")]
    IdentityMismatch { who: String, account: String },

    // ----- the merge lock -----
    /// bash `cmd_merge`: die "cannot open the merge lock for #$pr"
    #[error("cannot open the merge lock for #{pr}")]
    MergeLockUnopenable { pr: PrNumber },

    /// bash `cmd_merge`:
    /// die "another merge of #$pr is already running in this state directory.
    ///   One watcher per action. If you are sure nothing else is merging, remove
    ///   $RECEIPTS/.merge-$pr.lock and try again."
    #[error(
        "another merge of #{pr} is already running in this state directory.\n  One watcher per action. If you are sure nothing else is merging, remove\n  {} and try again.",
        lock.display()
    )]
    MergeLockHeld { pr: PrNumber, lock: PathBuf },

    // ----- the receipt: `receipt_check` -----
    /// bash `receipt_check`:
    /// die "no verification receipt for #$pr. Prove the guard bites, then: maintainer-merge receipt $pr <sha> '<proof>'"
    #[error(
        "no verification receipt for #{pr}. Prove the guard bites, then: maintainer-merge receipt {pr} <sha> '<proof>'"
    )]
    NoReceipt { pr: PrNumber },

    /// bash `receipt_check`, signature code 1: die "receipt for #$pr is not readable JSON"
    #[error("receipt for #{pr} is not readable JSON")]
    ReceiptNotJson { pr: PrNumber },

    /// bash `receipt_check`, signature code 2:
    /// die "the receipt for #$pr carries no signature, so it was not written by
    ///   \`maintainer-merge verify\`. A receipt that anything can write proves nothing.
    ///   Run: maintainer-merge verify $pr <head> <test> <mutation>"
    #[error(
        "the receipt for #{pr} carries no signature, so it was not written by\n  `maintainer-merge verify`. A receipt that anything can write proves nothing.\n  Run: maintainer-merge verify {pr} <head> <test> <mutation>"
    )]
    ReceiptUnsigned { pr: PrNumber },

    /// bash `receipt_check`, signature code 3:
    /// die "the receipt for #$pr does not match its signature: it was edited
    ///   after it was written, or written by something other than the gate."
    #[error(
        "the receipt for #{pr} does not match its signature: it was edited\n  after it was written, or written by something other than the gate."
    )]
    ReceiptSignatureMismatch { pr: PrNumber },

    /// bash `receipt_sign`: die "no receipt key at $RECEIPT_KEY; run ./install.sh"
    #[error("no receipt key at {}; run ./install.sh", path.display())]
    NoReceiptKey { path: PathBuf },

    /// bash `receipt_check`, the `pr` read back:
    /// die "the receipt at $receipt was earned for #${receipt_pr:-<none>}, not #$pr.
    ///   It carries a valid signature because the signature covers the receipt's
    ///   bytes and those bytes are unchanged; the filename is not signed. Run
    ///   \`maintainer-merge verify $pr <head> <test> <mutation>\` and earn one for
    ///   this pull request."
    ///
    /// `found` is the receipt's own `pr`, or empty when it has none.
    #[error(
        "the receipt at {} was earned for #{}, not #{pr}.\n  It carries a valid signature because the signature covers the receipt's\n  bytes and those bytes are unchanged; the filename is not signed. Run\n  `maintainer-merge verify {pr} <head> <test> <mutation>` and earn one for\n  this pull request.",
        path.display(),
        if found.is_empty() { "<none>" } else { found.as_str() }
    )]
    ReceiptForOtherPr { path: PathBuf, found: String, pr: PrNumber },

    /// bash `receipt_check`, an unattended run and a receipt that is not observed or dependency:
    /// die "the receipt for #$pr is '$kind', not 'observed'. An unattended run merges
    ///   only on a receipt \`maintainer-merge verify\` produced by watching the
    ///   mutation fail, or \`verify-deps\` for a pure dependency bump."
    #[error(
        "the receipt for #{pr} is '{kind}', not 'observed'. An unattended run merges\n  only on a receipt `maintainer-merge verify` produced by watching the\n  mutation fail, or `verify-deps` for a pure dependency bump."
    )]
    ReceiptNotObserved { pr: PrNumber, kind: String },

    // ----- the receipt: `receipt_covers` -----
    /// bash `receipt_covers`:
    /// die "#$pr: I could not read the changed files, so I cannot tell whether the
    ///   receipt covers them. Refusing rather than merging on a question I did not ask."
    #[error(
        "#{pr}: I could not read the changed files, so I cannot tell whether the\n  receipt covers them. Refusing rather than merging on a question I did not ask."
    )]
    ChangedFilesUnreadable { pr: PrNumber },

    /// bash `receipt_covers`:
    /// die "#$pr: the changed file list came back empty, which is not an answer."
    #[error("#{pr}: the changed file list came back empty, which is not an answer.")]
    ChangedFilesEmpty { pr: PrNumber },

    /// bash `receipt_covers`:
    /// die "the receipt for #$pr was earned by: ${rec_suites:-<none>}
    ///   Those suites do not run all of the production code this merge would land, so
    ///   the receipt proves less than the merge claims. Prove the rest:
    ///     maintainer-merge verify-part $pr $verified_head <test> <mutation> <suite>"
    #[error(
        "the receipt for #{pr} was earned by: {}\n  Those suites do not run all of the production code this merge would land, so\n  the receipt proves less than the merge claims. Prove the rest:\n    maintainer-merge verify-part {pr} {verified_head} <test> <mutation> <suite>",
        if suites.is_empty() { "<none>".to_string() } else { suites.join(" ") }
    )]
    ReceiptDoesNotCover { pr: PrNumber, suites: Vec<String>, verified_head: String },

    // ----- the pull request's own facts -----
    /// bash `cmd_merge`:
    /// die "could not read the head commit of #$pr; refusing to merge without a head to pin it to"
    #[error(
        "could not read the head commit of #{pr}; refusing to merge without a head to pin it to"
    )]
    HeadUnreadable { pr: PrNumber },

    /// bash `cmd_merge`:
    /// die "the head commit of #$pr came back as '$head', which is not a commit id; refusing to merge without a head to pin it to"
    #[error(
        "the head commit of #{pr} came back as '{head}', which is not a commit id; refusing to merge without a head to pin it to"
    )]
    HeadNotACommit { pr: PrNumber, head: String },

    /// bash `cmd_merge`: die "#$pr reviewDecision is '$review', not APPROVED"
    #[error("#{pr} reviewDecision is '{review}', not APPROVED")]
    NotApproved { pr: PrNumber, review: String },

    // ----- the claim checks -----
    /// bash `cmd_merge`, `closingIssuesReferences` could not be read:
    /// die "#$pr: I could not read which issues this closes:
    ///
    ///   $closes
    ///
    ///   A pull request that closes nothing and a question that could not be asked
    ///   produce the same empty answer here, and one of them means every claim check
    ///   below inspects nothing. Refusing instead."
    #[error(
        "#{pr}: I could not read which issues this closes:\n\n  {output}\n\n  A pull request that closes nothing and a question that could not be asked\n  produce the same empty answer here, and one of them means every claim check\n  below inspects nothing. Refusing instead."
    )]
    ClosesUnreadable { pr: PrNumber, output: String },

    /// bash `cmd_merge`, the author could not be read:
    /// die "#$pr: I could not read who opened this pull request:
    ///
    ///   $author
    ///
    ///   Every claim check below compares against that name, and comparing against an
    ///   empty one matches nobody and blocks nothing. Refusing instead."
    #[error(
        "#{pr}: I could not read who opened this pull request:\n\n  {output}\n\n  Every claim check below compares against that name, and comparing against an\n  empty one matches nobody and blocks nothing. Refusing instead."
    )]
    AuthorUnreadable { pr: PrNumber, output: String },

    /// bash `cmd_merge`, no `maintainer` helper for the holders question:
    /// die "#$pr closes #$iss and I could not read who was pointed at it:
    ///
    ///   no \`maintainer\` helper beside $(basename "${BASH_SOURCE[0]}") or on PATH
    ///
    ///   That question decides whether merging this takes somebody's work away.
    ///   Install the agent (./install.sh) or put \`maintainer\` on PATH, then retry."
    #[error(
        "#{pr} closes #{issue} and I could not read who was pointed at it:\n\n  no `maintainer` helper beside {PROGRAM} or on PATH\n\n  That question decides whether merging this takes somebody's work away.\n  Install the agent (./install.sh) or put `maintainer` on PATH, then retry."
    )]
    HoldersHelperMissing { pr: PrNumber, issue: IssueNumber },

    /// bash `cmd_merge`, `maintainer holders` exited non-zero:
    /// die "#$pr closes #$iss and I could not read who was pointed at it:
    ///
    ///   $holders
    ///
    ///   That question decides whether merging this takes somebody's work away, so
    ///   the gate refuses rather than assuming the answer is nobody."
    #[error(
        "#{pr} closes #{issue} and I could not read who was pointed at it:\n\n  {output}\n\n  That question decides whether merging this takes somebody's work away, so\n  the gate refuses rather than assuming the answer is nobody."
    )]
    HoldersUnreadable { pr: PrNumber, issue: IssueNumber, output: String },

    /// bash `cmd_merge`, somebody else was pointed at the issue and not released:
    /// die "#$pr closes #$iss, and $(join_names "$holders") was pointed at #$iss
    ///   by the maintainer and has not been released from it. $author was not.
    ///
    ///   Somebody was told that issue was theirs, in public, where other contributors
    ///   read it and stayed off it. Merging this spends their evening. Talk to both
    ///   before it lands, and if the offer is genuinely stale release it on the thread
    ///   first: a release comment has to @mention them and carry the release marker,
    ///   or nothing but a human can see it."
    #[error(
        "#{pr} closes #{issue}, and {} was pointed at #{issue}\n  by the maintainer and has not been released from it. {author} was not.\n\n  Somebody was told that issue was theirs, in public, where other contributors\n  read it and stayed off it. Merging this spends their evening. Talk to both\n  before it lands, and if the offer is genuinely stale release it on the thread\n  first: a release comment has to @mention them and carry the release marker,\n  or nothing but a human can see it.",
        join_names(holders)
    )]
    IssuePointedAtOthers { pr: PrNumber, issue: IssueNumber, holders: Vec<String>, author: String },

    /// bash `cmd_merge`, the labels could not be read:
    /// die "#$pr closes #$iss and I could not read its labels:
    ///
    ///   $lbl
    ///
    ///   The '$claim_label' label is how this project records that somebody was
    ///   promised an issue, so a merge decided without reading it is decided blind."
    #[error(
        "#{pr} closes #{issue} and I could not read its labels:\n\n  {output}\n\n  The '{claim_label}' label is how this project records that somebody was\n  promised an issue, so a merge decided without reading it is decided blind."
    )]
    LabelsUnreadable { pr: PrNumber, issue: IssueNumber, claim_label: String, output: String },

    /// bash `cmd_merge`, the assignees could not be read:
    /// die "#$pr closes #$iss and I could not read who it is assigned to:
    ///
    ///   $holders_a
    ///
    ///   An unread assignee list and an empty one are the same string here, and one of
    ///   them means somebody's name is on this issue and I cannot see it."
    #[error(
        "#{pr} closes #{issue} and I could not read who it is assigned to:\n\n  {output}\n\n  An unread assignee list and an empty one are the same string here, and one of\n  them means somebody's name is on this issue and I cannot see it."
    )]
    AssigneesUnreadable { pr: PrNumber, issue: IssueNumber, output: String },

    /// bash `cmd_merge`, a claimed issue assigned to somebody else:
    /// die "#$pr closes #$iss, which carries the '$claim_label' label and is
    ///   assigned to $(join_names "$holders_a"), not to $author.
    ///
    ///   Merging this takes their work away. Talk to both before it lands, and if the
    ///   assignment is stale, unassign it on the thread first so the record matches."
    #[error(
        "#{pr} closes #{issue}, which carries the '{claim_label}' label and is\n  assigned to {}, not to {author}.\n\n  Merging this takes their work away. Talk to both before it lands, and if the\n  assignment is stale, unassign it on the thread first so the record matches.",
        join_names(assignees)
    )]
    ClaimedAssignedToOthers {
        pr: PrNumber,
        issue: IssueNumber,
        claim_label: String,
        assignees: Vec<String>,
        author: String,
    },

    /// bash `cmd_merge`, the commenters could not be read:
    /// die "#$pr closes #$iss and I could not read who has posted there:
    ///
    ///   $others_raw
    ///
    ///   A reserved issue with a second voice on it is the case a human has to look
    ///   at, and I cannot tell whether this is one."
    #[error(
        "#{pr} closes #{issue} and I could not read who has posted there:\n\n  {output}\n\n  A reserved issue with a second voice on it is the case a human has to look\n  at, and I cannot tell whether this is one."
    )]
    CommentersUnreadable { pr: PrNumber, issue: IssueNumber, output: String },

    /// bash `cmd_merge`, a claimed, unassigned issue with another voice on it:
    /// die "#$pr closes #$iss, which carries the '$claim_label' label, has no
    ///   assignee, and $(join_names "$others") also posted there besides $author.
    ///
    ///   Merging this takes their work away, and the label is public: other
    ///   contributors read it and stayed off the issue. Talk to both of them first.
    ///   If the claim is genuinely stale, release it on the thread before merging."
    #[error(
        "#{pr} closes #{issue}, which carries the '{claim_label}' label, has no\n  assignee, and {} also posted there besides {author}.\n\n  Merging this takes their work away, and the label is public: other\n  contributors read it and stayed off the issue. Talk to both of them first.\n  If the claim is genuinely stale, release it on the thread before merging.",
        join_names(others)
    )]
    ClaimedWithSecondVoice {
        pr: PrNumber,
        issue: IssueNumber,
        claim_label: String,
        others: Vec<String>,
        author: String,
    },

    // ----- mergeStateStatus -----
    /// bash `cmd_merge`, BEHIND:
    /// die "#$pr is BEHIND its base; its checks describe a different tree. Update the branch and let it re-run. When it is approved and only that is missing: maintainer-merge update $pr"
    #[error(
        "#{pr} is BEHIND its base; its checks describe a different tree. Update the branch and let it re-run. When it is approved and only that is missing: maintainer-merge update {pr}"
    )]
    Behind { pr: PrNumber },

    /// bash `cmd_merge`, DIRTY: die "#$pr has conflicts (mergeStateStatus DIRTY)"
    #[error("#{pr} has conflicts (mergeStateStatus DIRTY)")]
    Dirty { pr: PrNumber },

    /// bash `cmd_merge`, BLOCKED: die "#$pr is BLOCKED: a required review or check is missing"
    #[error("#{pr} is BLOCKED: a required review or check is missing")]
    Blocked { pr: PrNumber },

    /// bash `cmd_merge`, UNSTABLE, through the `*)` arm:
    /// die "#$pr mergeStateStatus is '$state'; only CLEAN and HAS_HOOKS may merge"
    ///
    /// UNSTABLE has no arm of its own in the bash; it is its own variant here so
    /// that a test, and a caller, can tell it from UNKNOWN and from a state this
    /// enum has not heard of. The text is the bash's.
    #[error("#{pr} mergeStateStatus is 'UNSTABLE'; only CLEAN and HAS_HOOKS may merge")]
    Unstable { pr: PrNumber },

    /// bash `cmd_merge`, UNKNOWN, through the `*)` arm. GitHub answers UNKNOWN while it computes the state; a later read may differ.
    #[error("#{pr} mergeStateStatus is 'UNKNOWN'; only CLEAN and HAS_HOOKS may merge")]
    Unknown { pr: PrNumber },

    /// bash `cmd_merge`, every other state (`DRAFT`, an empty read, a state GitHub adds next year), through the `*)` arm:
    /// die "#$pr mergeStateStatus is '$state'; only CLEAN and HAS_HOOKS may merge"
    #[error("#{pr} mergeStateStatus is '{state}'; only CLEAN and HAS_HOOKS may merge")]
    MergeStateNotAllowed { pr: PrNumber, state: String },

    // ----- the checks -----
    /// bash `cmd_merge`: die "#$pr reported no checks at all; refusing"
    #[error("#{pr} reported no checks at all; refusing")]
    NoChecks { pr: PrNumber },

    /// bash `cmd_merge`:
    /// die "could not read the check list for #$pr; gh returned:
    /// $(printf '%s' "$checks" | head -c 300)"
    #[error("could not read the check list for #{pr}; gh returned:\n{head}")]
    ChecksUnparsable { pr: PrNumber, head: String },

    /// bash `cmd_merge`: die "#$pr has $fails failing check(s)"
    #[error("#{pr} has {count} failing check(s)")]
    FailingChecks { pr: PrNumber, count: u64 },

    /// bash `cmd_merge`: die "#$pr has $pend pending check(s)"
    #[error("#{pr} has {count} pending check(s)")]
    PendingChecks { pr: PrNumber, count: u64 },

    /// bash `cmd_merge`:
    /// die "#$pr has $cancel_n cancelled check(s); a cancelled check never reported a verdict. Re-run it."
    #[error(
        "#{pr} has {count} cancelled check(s); a cancelled check never reported a verdict. Re-run it."
    )]
    CancelledChecks { pr: PrNumber, count: u64 },

    /// bash `cmd_merge`:
    /// die "#$pr has an empty check list; an empty board is a failure, not a pass"
    #[error("#{pr} has an empty check list; an empty board is a failure, not a pass")]
    EmptyBoard { pr: PrNumber },

    /// bash `cmd_merge`:
    /// die "#$pr: $((pass_n + skip_n)) of $total checks are pass or skipping; the rest are in a bucket this gate does not accept"
    #[error(
        "#{pr}: {accepted} of {total} checks are pass or skipping; the rest are in a bucket this gate does not accept"
    )]
    ChecksInUnknownBucket { pr: PrNumber, accepted: u64, total: u64 },

    /// bash `cmd_merge`: die "#$pr: all $total checks were skipped, so nothing ran"
    #[error("#{pr}: all {total} checks were skipped, so nothing ran")]
    AllChecksSkipped { pr: PrNumber, total: u64 },

    // ----- `receipt_fresh` -----
    /// bash `receipt_fresh`: die "cannot fetch #$pr"
    #[error("cannot fetch #{pr}")]
    CannotFetch { pr: PrNumber },

    /// bash `receipt_fresh`:
    /// die "production code changed since the verified head $verified_head:
    /// $(printf '%s' "$diff" | sed 's/^/    /')
    ///   The receipt no longer describes this tree. Re-verify and record a new one."
    #[error(
        "production code changed since the verified head {verified_head}:\n{}\n  The receipt no longer describes this tree. Re-verify and record a new one.",
        indent4(&paths.join("\n"))
    )]
    ProductionChanged { verified_head: String, paths: Vec<String> },

    /// **new, plan decision F1.** The bash reads `git diff --name-only` with
    /// `2>/dev/null` and never looks at its status, so a git error is an empty
    /// diff and an empty diff is "no production diff; receipt still applies"
    /// (`bin/maintainer-merge` lines 1926 and 1934 of the bash the plan read). A failed diff
    /// refuses here.
    #[error(
        "could not tell whether production code changed since the verified head {verified_head} of #{pr}: git diff failed ({reason}).\n  An error is not an empty diff, so the receipt is not treated as still applying. Re-verify and record a new one."
    )]
    StalenessUnreadable { pr: PrNumber, verified_head: String, reason: String },

    // ----- the merge -----
    /// bash `cmd_merge`: `gh pr merge ... || die "merge failed"`
    #[error("merge failed")]
    MergeFailed,

    // ----- the typestate -----
    /// **new.** Evidence built for one pull request was offered for another.
    /// Nothing in the bash can say this, because its receipt is a file that names
    /// the pull request and `receipt_check` reads it back; the typestate carries
    /// the number and checks it at the transition.
    #[error("the verification evidence was earned for #{found}, not #{pr}")]
    EvidenceForOtherPr { pr: PrNumber, found: u64 },
}

/// What a step was about to do when it asked whether the pull request was leased.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Verify,
    Merge,
    Update,
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Verify => "verify",
            Self::Merge => "merge",
            Self::Update => "update",
        })
    }
}

impl GateDecision {
    /// The process exit code. Every refusal of `merge` is `die`, which is 1.
    ///
    /// The `match` has no `_` arm, so a new variant fails to compile until it
    /// says what its code is.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::InvalidPrNumber { .. }
            | Self::HeadTruncated { .. }
            | Self::LeaseHelperMissing { .. }
            | Self::LeaseHeld { .. }
            | Self::LeaseUnreadable { .. }
            | Self::NoProdGlobs
            | Self::IdentityUnconfirmed { .. }
            | Self::IdentityMismatch { .. }
            | Self::MergeLockUnopenable { .. }
            | Self::MergeLockHeld { .. }
            | Self::NoReceipt { .. }
            | Self::ReceiptNotJson { .. }
            | Self::ReceiptUnsigned { .. }
            | Self::ReceiptSignatureMismatch { .. }
            | Self::NoReceiptKey { .. }
            | Self::ReceiptForOtherPr { .. }
            | Self::ReceiptNotObserved { .. }
            | Self::ChangedFilesUnreadable { .. }
            | Self::ChangedFilesEmpty { .. }
            | Self::ReceiptDoesNotCover { .. }
            | Self::HeadUnreadable { .. }
            | Self::HeadNotACommit { .. }
            | Self::NotApproved { .. }
            | Self::ClosesUnreadable { .. }
            | Self::AuthorUnreadable { .. }
            | Self::HoldersHelperMissing { .. }
            | Self::HoldersUnreadable { .. }
            | Self::IssuePointedAtOthers { .. }
            | Self::LabelsUnreadable { .. }
            | Self::AssigneesUnreadable { .. }
            | Self::ClaimedAssignedToOthers { .. }
            | Self::CommentersUnreadable { .. }
            | Self::ClaimedWithSecondVoice { .. }
            | Self::Behind { .. }
            | Self::Dirty { .. }
            | Self::Blocked { .. }
            | Self::Unstable { .. }
            | Self::Unknown { .. }
            | Self::MergeStateNotAllowed { .. }
            | Self::NoChecks { .. }
            | Self::ChecksUnparsable { .. }
            | Self::FailingChecks { .. }
            | Self::PendingChecks { .. }
            | Self::CancelledChecks { .. }
            | Self::EmptyBoard { .. }
            | Self::ChecksInUnknownBucket { .. }
            | Self::AllChecksSkipped { .. }
            | Self::CannotFetch { .. }
            | Self::ProductionChanged { .. }
            | Self::StalenessUnreadable { .. }
            | Self::MergeFailed
            | Self::EvidenceForOtherPr { .. } => 1,
        }
    }

    /// What `die` writes to standard error: `maintainer-merge: <message>` and a newline.
    #[must_use]
    pub fn stderr_line(&self) -> String {
        format!("{PROGRAM}: {self}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> PrNumber {
        PrNumber::parse("7").unwrap()
    }

    fn issue() -> IssueNumber {
        IssueNumber::new(355).unwrap()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| (*n).to_string()).collect()
    }

    /// The position of a variant in `samples()`. The `match` has no `_` arm, so a
    /// new variant fails to compile here until it has an index, and the test
    /// `every_variant_has_exactly_one_sample` fails until `samples()` has a row for it.
    fn rank(decision: &GateDecision) -> usize {
        match decision {
            GateDecision::InvalidPrNumber { .. } => 0,
            GateDecision::HeadTruncated { .. } => 1,
            GateDecision::LeaseHelperMissing { .. } => 2,
            GateDecision::LeaseHeld { .. } => 3,
            GateDecision::LeaseUnreadable { .. } => 4,
            GateDecision::NoProdGlobs => 5,
            GateDecision::IdentityUnconfirmed { .. } => 6,
            GateDecision::IdentityMismatch { .. } => 7,
            GateDecision::MergeLockUnopenable { .. } => 8,
            GateDecision::MergeLockHeld { .. } => 9,
            GateDecision::NoReceipt { .. } => 10,
            GateDecision::ReceiptNotJson { .. } => 11,
            GateDecision::ReceiptUnsigned { .. } => 12,
            GateDecision::ReceiptSignatureMismatch { .. } => 13,
            GateDecision::NoReceiptKey { .. } => 14,
            GateDecision::ReceiptForOtherPr { .. } => 15,
            GateDecision::ReceiptNotObserved { .. } => 16,
            GateDecision::ChangedFilesUnreadable { .. } => 17,
            GateDecision::ChangedFilesEmpty { .. } => 18,
            GateDecision::ReceiptDoesNotCover { .. } => 19,
            GateDecision::HeadUnreadable { .. } => 20,
            GateDecision::HeadNotACommit { .. } => 21,
            GateDecision::NotApproved { .. } => 22,
            GateDecision::ClosesUnreadable { .. } => 23,
            GateDecision::AuthorUnreadable { .. } => 24,
            GateDecision::HoldersHelperMissing { .. } => 25,
            GateDecision::HoldersUnreadable { .. } => 26,
            GateDecision::IssuePointedAtOthers { .. } => 27,
            GateDecision::LabelsUnreadable { .. } => 28,
            GateDecision::AssigneesUnreadable { .. } => 29,
            GateDecision::ClaimedAssignedToOthers { .. } => 30,
            GateDecision::CommentersUnreadable { .. } => 31,
            GateDecision::ClaimedWithSecondVoice { .. } => 32,
            GateDecision::Behind { .. } => 33,
            GateDecision::Dirty { .. } => 34,
            GateDecision::Blocked { .. } => 35,
            GateDecision::Unstable { .. } => 36,
            GateDecision::Unknown { .. } => 37,
            GateDecision::MergeStateNotAllowed { .. } => 38,
            GateDecision::NoChecks { .. } => 39,
            GateDecision::ChecksUnparsable { .. } => 40,
            GateDecision::FailingChecks { .. } => 41,
            GateDecision::PendingChecks { .. } => 42,
            GateDecision::CancelledChecks { .. } => 43,
            GateDecision::EmptyBoard { .. } => 44,
            GateDecision::ChecksInUnknownBucket { .. } => 45,
            GateDecision::AllChecksSkipped { .. } => 46,
            GateDecision::CannotFetch { .. } => 47,
            GateDecision::ProductionChanged { .. } => 48,
            GateDecision::StalenessUnreadable { .. } => 49,
            GateDecision::MergeFailed => 50,
            GateDecision::EvidenceForOtherPr { .. } => 51,
        }
    }

    /// One row per variant, in `rank` order: a sample and the text bash prints for
    /// it. Each text is copied from the `die` line quoted in the variant's own
    /// comment, with `$var` replaced by the sample's value.
    #[allow(clippy::too_many_lines, reason = "the table is data: one row per refusal")]
    fn samples() -> Vec<(GateDecision, String)> {
        let verified = "0123456789abcdef0123456789abcdef01234567";
        vec![
            // die "pull request must be a positive integer, not '$1'"
            (
                GateDecision::InvalidPrNumber { given: "../x".into() },
                "pull request must be a positive integer, not '../x'".into(),
            ),
            // die "head sha looks truncated: $sha"
            (
                GateDecision::HeadTruncated { sha: "abc".into() },
                "head sha looks truncated: abc".into(),
            ),
            // lease_gate: no helper
            (
                GateDecision::LeaseHelperMissing { pr: pr(), action: Action::Merge },
                "cannot tell whether #7 is leased, so I will not merge it:\n  no `maintainer` helper beside maintainer-merge or on PATH.\n  Install the agent (./install.sh) or put `maintainer` on PATH, then retry.".into(),
            ),
            // lease_gate: status 1
            (
                GateDecision::LeaseHeld { pr: pr(), action: Action::Merge, output: "#7 held by run:other (run)".into() },
                "will not merge #7: another actor has leased it.\n  #7 held by run:other (run)\n  Ask them, wait for it to expire, or have them `maintainer lease release 7`.".into(),
            ),
            // lease_gate: any other status
            (
                GateDecision::LeaseUnreadable { pr: pr(), action: Action::Merge, output: "not a lease".into() },
                "could not read the lease on #7, so I will not merge it:\n  not a lease".into(),
            ),
            // require_prod_globs
            (
                GateDecision::NoProdGlobs,
                "this profile declares no PROD_GLOBS, so nothing can say\n  whether production code moved since the receipt. Set it in profile.env, for example:\n    PROD_GLOBS=\"bin/* lib/* scripts/* .github/*\"".into(),
            ),
            // identity: die "gh could not confirm an account (gh exited $rc); if the token was rejected, run gh auth login -h github.com as $ACCOUNT"
            (
                GateDecision::IdentityUnconfirmed { account: "vladimirrott".into(), exit: 4 },
                "gh could not confirm an account (gh exited 4); if the token was rejected, run gh auth login -h github.com as vladimirrott".into(),
            ),
            // identity: die "gh is authenticated as '${who:-unknown}', not $ACCOUNT"
            (
                GateDecision::IdentityMismatch { who: "someone-else".into(), account: "vladimirrott".into() },
                "gh is authenticated as 'someone-else', not vladimirrott".into(),
            ),
            // die "cannot open the merge lock for #$pr"
            (GateDecision::MergeLockUnopenable { pr: pr() }, "cannot open the merge lock for #7".into()),
            // die "another merge of #$pr is already running in this state directory.\n  One watcher ... remove\n  $RECEIPTS/.merge-$pr.lock and try again."
            (
                GateDecision::MergeLockHeld { pr: pr(), lock: PathBuf::from("/s/receipts/.merge-7.lock") },
                "another merge of #7 is already running in this state directory.\n  One watcher per action. If you are sure nothing else is merging, remove\n  /s/receipts/.merge-7.lock and try again.".into(),
            ),
            // die "no verification receipt for #$pr. Prove the guard bites, then: maintainer-merge receipt $pr <sha> '<proof>'"
            (
                GateDecision::NoReceipt { pr: pr() },
                "no verification receipt for #7. Prove the guard bites, then: maintainer-merge receipt 7 <sha> '<proof>'".into(),
            ),
            // die "receipt for #$pr is not readable JSON"
            (GateDecision::ReceiptNotJson { pr: pr() }, "receipt for #7 is not readable JSON".into()),
            // receipt_check, code 2
            (
                GateDecision::ReceiptUnsigned { pr: pr() },
                "the receipt for #7 carries no signature, so it was not written by\n  `maintainer-merge verify`. A receipt that anything can write proves nothing.\n  Run: maintainer-merge verify 7 <head> <test> <mutation>".into(),
            ),
            // receipt_check, code 3
            (
                GateDecision::ReceiptSignatureMismatch { pr: pr() },
                "the receipt for #7 does not match its signature: it was edited\n  after it was written, or written by something other than the gate.".into(),
            ),
            // receipt_sign: die "no receipt key at $RECEIPT_KEY; run ./install.sh"
            (
                GateDecision::NoReceiptKey { path: PathBuf::from("/h/receipt.key") },
                "no receipt key at /h/receipt.key; run ./install.sh".into(),
            ),
            // receipt_check, pr read back
            (
                GateDecision::ReceiptForOtherPr { path: PathBuf::from("/s/receipts/7.json"), found: "1".into(), pr: pr() },
                "the receipt at /s/receipts/7.json was earned for #1, not #7.\n  It carries a valid signature because the signature covers the receipt's\n  bytes and those bytes are unchanged; the filename is not signed. Run\n  `maintainer-merge verify 7 <head> <test> <mutation>` and earn one for\n  this pull request.".into(),
            ),
            // receipt_check, unattended and asserted
            (
                GateDecision::ReceiptNotObserved { pr: pr(), kind: "asserted".into() },
                "the receipt for #7 is 'asserted', not 'observed'. An unattended run merges\n  only on a receipt `maintainer-merge verify` produced by watching the\n  mutation fail, or `verify-deps` for a pure dependency bump.".into(),
            ),
            // receipt_covers: the files could not be read
            (
                GateDecision::ChangedFilesUnreadable { pr: pr() },
                "#7: I could not read the changed files, so I cannot tell whether the\n  receipt covers them. Refusing rather than merging on a question I did not ask.".into(),
            ),
            // receipt_covers: the list is empty
            (
                GateDecision::ChangedFilesEmpty { pr: pr() },
                "#7: the changed file list came back empty, which is not an answer.".into(),
            ),
            // receipt_covers: the suites do not cover
            (
                GateDecision::ReceiptDoesNotCover { pr: pr(), suites: names(&["rust", "docs"]), verified_head: verified.into() },
                format!("the receipt for #7 was earned by: rust docs\n  Those suites do not run all of the production code this merge would land, so\n  the receipt proves less than the merge claims. Prove the rest:\n    maintainer-merge verify-part 7 {verified} <test> <mutation> <suite>"),
            ),
            // cmd_merge: the head could not be read
            (
                GateDecision::HeadUnreadable { pr: pr() },
                "could not read the head commit of #7; refusing to merge without a head to pin it to".into(),
            ),
            // cmd_merge: the head is not a commit id
            (
                GateDecision::HeadNotACommit { pr: pr(), head: "NOT-A-COMMIT".into() },
                "the head commit of #7 came back as 'NOT-A-COMMIT', which is not a commit id; refusing to merge without a head to pin it to".into(),
            ),
            // die "#$pr reviewDecision is '$review', not APPROVED"
            (
                GateDecision::NotApproved { pr: pr(), review: "REVIEW_REQUIRED".into() },
                "#7 reviewDecision is 'REVIEW_REQUIRED', not APPROVED".into(),
            ),
            // cmd_merge: closingIssuesReferences unreadable
            (
                GateDecision::ClosesUnreadable { pr: pr(), output: "HTTP 502".into() },
                "#7: I could not read which issues this closes:\n\n  HTTP 502\n\n  A pull request that closes nothing and a question that could not be asked\n  produce the same empty answer here, and one of them means every claim check\n  below inspects nothing. Refusing instead.".into(),
            ),
            // cmd_merge: the author unreadable
            (
                GateDecision::AuthorUnreadable { pr: pr(), output: "HTTP 502".into() },
                "#7: I could not read who opened this pull request:\n\n  HTTP 502\n\n  Every claim check below compares against that name, and comparing against an\n  empty one matches nobody and blocks nothing. Refusing instead.".into(),
            ),
            // cmd_merge: no helper for holders
            (
                GateDecision::HoldersHelperMissing { pr: pr(), issue: issue() },
                "#7 closes #355 and I could not read who was pointed at it:\n\n  no `maintainer` helper beside maintainer-merge or on PATH\n\n  That question decides whether merging this takes somebody's work away.\n  Install the agent (./install.sh) or put `maintainer` on PATH, then retry.".into(),
            ),
            // cmd_merge: holders exited non-zero
            (
                GateDecision::HoldersUnreadable { pr: pr(), issue: issue(), output: "no profile".into() },
                "#7 closes #355 and I could not read who was pointed at it:\n\n  no profile\n\n  That question decides whether merging this takes somebody's work away, so\n  the gate refuses rather than assuming the answer is nobody.".into(),
            ),
            // cmd_merge: pointed at somebody else
            (
                GateDecision::IssuePointedAtOthers { pr: pr(), issue: issue(), holders: names(&["TayfurYldz", "bob"]), author: "alice".into() },
                "#7 closes #355, and TayfurYldz, bob was pointed at #355\n  by the maintainer and has not been released from it. alice was not.\n\n  Somebody was told that issue was theirs, in public, where other contributors\n  read it and stayed off it. Merging this spends their evening. Talk to both\n  before it lands, and if the offer is genuinely stale release it on the thread\n  first: a release comment has to @mention them and carry the release marker,\n  or nothing but a human can see it.".into(),
            ),
            // cmd_merge: labels unreadable
            (
                GateDecision::LabelsUnreadable { pr: pr(), issue: issue(), claim_label: "claimed".into(), output: "HTTP 502".into() },
                "#7 closes #355 and I could not read its labels:\n\n  HTTP 502\n\n  The 'claimed' label is how this project records that somebody was\n  promised an issue, so a merge decided without reading it is decided blind.".into(),
            ),
            // cmd_merge: assignees unreadable
            (
                GateDecision::AssigneesUnreadable { pr: pr(), issue: issue(), output: "HTTP 502".into() },
                "#7 closes #355 and I could not read who it is assigned to:\n\n  HTTP 502\n\n  An unread assignee list and an empty one are the same string here, and one of\n  them means somebody's name is on this issue and I cannot see it.".into(),
            ),
            // cmd_merge: claimed and assigned to others
            (
                GateDecision::ClaimedAssignedToOthers { pr: pr(), issue: issue(), claim_label: "claimed".into(), assignees: names(&["bob"]), author: "alice".into() },
                "#7 closes #355, which carries the 'claimed' label and is\n  assigned to bob, not to alice.\n\n  Merging this takes their work away. Talk to both before it lands, and if the\n  assignment is stale, unassign it on the thread first so the record matches.".into(),
            ),
            // cmd_merge: commenters unreadable
            (
                GateDecision::CommentersUnreadable { pr: pr(), issue: issue(), output: "HTTP 502".into() },
                "#7 closes #355 and I could not read who has posted there:\n\n  HTTP 502\n\n  A reserved issue with a second voice on it is the case a human has to look\n  at, and I cannot tell whether this is one.".into(),
            ),
            // cmd_merge: claimed, unassigned, a second voice
            (
                GateDecision::ClaimedWithSecondVoice { pr: pr(), issue: issue(), claim_label: "claimed".into(), others: names(&["bob", "carol"]), author: "alice".into() },
                "#7 closes #355, which carries the 'claimed' label, has no\n  assignee, and bob, carol also posted there besides alice.\n\n  Merging this takes their work away, and the label is public: other\n  contributors read it and stayed off the issue. Talk to both of them first.\n  If the claim is genuinely stale, release it on the thread before merging.".into(),
            ),
            // BEHIND
            (
                GateDecision::Behind { pr: pr() },
                "#7 is BEHIND its base; its checks describe a different tree. Update the branch and let it re-run. When it is approved and only that is missing: maintainer-merge update 7".into(),
            ),
            // DIRTY
            (GateDecision::Dirty { pr: pr() }, "#7 has conflicts (mergeStateStatus DIRTY)".into()),
            // BLOCKED
            (
                GateDecision::Blocked { pr: pr() },
                "#7 is BLOCKED: a required review or check is missing".into(),
            ),
            // *) with $state = UNSTABLE
            (
                GateDecision::Unstable { pr: pr() },
                "#7 mergeStateStatus is 'UNSTABLE'; only CLEAN and HAS_HOOKS may merge".into(),
            ),
            // *) with $state = UNKNOWN
            (
                GateDecision::Unknown { pr: pr() },
                "#7 mergeStateStatus is 'UNKNOWN'; only CLEAN and HAS_HOOKS may merge".into(),
            ),
            // *) with any other $state
            (
                GateDecision::MergeStateNotAllowed { pr: pr(), state: "DRAFT".into() },
                "#7 mergeStateStatus is 'DRAFT'; only CLEAN and HAS_HOOKS may merge".into(),
            ),
            // die "#$pr reported no checks at all; refusing"
            (GateDecision::NoChecks { pr: pr() }, "#7 reported no checks at all; refusing".into()),
            // die "could not read the check list for #$pr; gh returned:\n$(printf '%s' "$checks" | head -c 300)"
            (
                GateDecision::ChecksUnparsable { pr: pr(), head: "gh: HTTP 502".into() },
                "could not read the check list for #7; gh returned:\ngh: HTTP 502".into(),
            ),
            // die "#$pr has $fails failing check(s)"
            (GateDecision::FailingChecks { pr: pr(), count: 2 }, "#7 has 2 failing check(s)".into()),
            // die "#$pr has $pend pending check(s)"
            (GateDecision::PendingChecks { pr: pr(), count: 3 }, "#7 has 3 pending check(s)".into()),
            // die "#$pr has $cancel_n cancelled check(s); a cancelled check never reported a verdict. Re-run it."
            (
                GateDecision::CancelledChecks { pr: pr(), count: 1 },
                "#7 has 1 cancelled check(s); a cancelled check never reported a verdict. Re-run it.".into(),
            ),
            // die "#$pr has an empty check list; an empty board is a failure, not a pass"
            (
                GateDecision::EmptyBoard { pr: pr() },
                "#7 has an empty check list; an empty board is a failure, not a pass".into(),
            ),
            // die "#$pr: $((pass_n + skip_n)) of $total checks are pass or skipping; the rest are in a bucket this gate does not accept"
            (
                GateDecision::ChecksInUnknownBucket { pr: pr(), accepted: 4, total: 6 },
                "#7: 4 of 6 checks are pass or skipping; the rest are in a bucket this gate does not accept".into(),
            ),
            // die "#$pr: all $total checks were skipped, so nothing ran"
            (
                GateDecision::AllChecksSkipped { pr: pr(), total: 5 },
                "#7: all 5 checks were skipped, so nothing ran".into(),
            ),
            // receipt_fresh: die "cannot fetch #$pr"
            (GateDecision::CannotFetch { pr: pr() }, "cannot fetch #7".into()),
            // receipt_fresh: production code changed
            (
                GateDecision::ProductionChanged { verified_head: verified.into(), paths: names(&["crates/x/src/lib.rs", "bin/y"]) },
                format!("production code changed since the verified head {verified}:\n    crates/x/src/lib.rs\n    bin/y\n  The receipt no longer describes this tree. Re-verify and record a new one."),
            ),
            // F1 (new): a failed git diff
            (
                GateDecision::StalenessUnreadable { pr: pr(), verified_head: verified.into(), reason: "fatal: bad revision".into() },
                format!("could not tell whether production code changed since the verified head {verified} of #7: git diff failed (fatal: bad revision).\n  An error is not an empty diff, so the receipt is not treated as still applying. Re-verify and record a new one."),
            ),
            // gh pr merge ... || die "merge failed"
            (GateDecision::MergeFailed, "merge failed".into()),
            // new: evidence for another pull request
            (
                GateDecision::EvidenceForOtherPr { pr: pr(), found: 8 },
                "the verification evidence was earned for #8, not #7".into(),
            ),
        ]
    }

    #[test]
    fn every_variant_has_exactly_one_sample() {
        let table = samples();
        assert_eq!(table.len(), 52, "one row per variant");
        for (at, (decision, _)) in table.iter().enumerate() {
            assert_eq!(rank(decision), at, "row {at} is out of order: {decision:?}");
        }
    }

    #[test]
    fn every_variant_prints_the_text_of_its_bash_die() {
        let mut checked = 0;
        for (decision, bash) in samples() {
            assert_eq!(decision.to_string(), bash, "{decision:?}");
            checked += 1;
        }
        assert_eq!(checked, 52);
    }

    #[test]
    fn every_refusal_of_merge_exits_1() {
        for (decision, _) in samples() {
            assert_eq!(decision.exit_code(), 1, "{decision:?}");
        }
    }

    #[test]
    fn the_stderr_line_is_the_program_name_a_colon_the_message_and_a_newline() {
        let line = GateDecision::MergeFailed.stderr_line();
        assert_eq!(line, "maintainer-merge: merge failed\n");
        let line = GateDecision::Dirty { pr: pr() }.stderr_line();
        assert_eq!(line, "maintainer-merge: #7 has conflicts (mergeStateStatus DIRTY)\n");
    }

    #[test]
    fn behind_blocked_unstable_and_unknown_are_four_different_refusals() {
        let four = [
            GateDecision::Behind { pr: pr() },
            GateDecision::Blocked { pr: pr() },
            GateDecision::Unstable { pr: pr() },
            GateDecision::Unknown { pr: pr() },
        ];
        for (i, a) in four.iter().enumerate() {
            for (j, b) in four.iter().enumerate() {
                assert_eq!(i == j, a == b, "{a:?} vs {b:?}");
                assert_eq!(i == j, a.to_string() == b.to_string(), "{a} vs {b}");
            }
        }
    }

    #[test]
    fn a_receipt_with_no_pr_of_its_own_prints_none() {
        let refused = GateDecision::ReceiptForOtherPr {
            path: PathBuf::from("/s/receipts/7.json"),
            found: String::new(),
            pr: pr(),
        };
        assert!(
            refused
                .to_string()
                .starts_with("the receipt at /s/receipts/7.json was earned for #<none>, not #7.\n"),
            "{refused}"
        );
    }

    #[test]
    fn a_receipt_that_names_no_suite_prints_none() {
        let refused = GateDecision::ReceiptDoesNotCover {
            pr: pr(),
            suites: Vec::new(),
            verified_head: "abc".into(),
        };
        assert!(
            refused.to_string().starts_with("the receipt for #7 was earned by: <none>\n"),
            "{refused}"
        );
    }

    #[test]
    fn names_are_joined_by_a_comma_and_a_space() {
        assert_eq!(join_names(&names(&["a"])), "a");
        assert_eq!(join_names(&names(&["a", "b", "c"])), "a, b, c");
    }

    #[test]
    fn a_leading_zero_pull_request_keeps_its_text_in_the_message() {
        let refused = GateDecision::NoReceipt { pr: PrNumber::parse("007").unwrap() };
        assert!(refused.to_string().contains("#007"));
    }

    /// Every `die "..."` literal of `bin/maintainer-merge`, with each expansion
    /// (`$x`, `${x}`, `$(...)`) turned into a NUL, split into lines.
    fn bash_die_lines(source: &str) -> Vec<String> {
        let bytes: Vec<char> = source.chars().collect();
        let mut lines = Vec::new();
        let mut from = 0;
        while let Some(at) = source[from..].find("die \"") {
            let start_byte = from + at;
            let prev = source[..start_byte].chars().next_back();
            from = start_byte + 5;
            if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let start = source[..from].chars().count();
            let mut text = String::new();
            let mut depth = 0_usize;
            let mut i = start;
            while i < bytes.len() {
                let c = bytes[i];
                if c == '\\' && depth == 0 {
                    // \` \" \$ and \\ stand for the character after the backslash.
                    if let Some(next) = bytes.get(i + 1) {
                        text.push(*next);
                    }
                    i += 2;
                    continue;
                }
                if depth == 0 && c == '"' {
                    break;
                }
                if c == '$' && bytes.get(i + 1) == Some(&'(') {
                    if depth == 0 {
                        text.push('\0');
                    }
                    depth += 1;
                    i += 2;
                    continue;
                }
                if depth > 0 {
                    match c {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                    continue;
                }
                if c == '$' && bytes.get(i + 1) == Some(&'{') {
                    while i < bytes.len() && bytes[i] != '}' {
                        i += 1;
                    }
                    text.push('\0');
                    i += 1;
                    continue;
                }
                if c == '$'
                    && bytes
                        .get(i + 1)
                        .is_some_and(|n| n.is_ascii_alphanumeric() || *n == '_' || *n == '#')
                {
                    i += 1;
                    while bytes.get(i).is_some_and(|n| n.is_ascii_alphanumeric() || *n == '_') {
                        i += 1;
                    }
                    text.push('\0');
                    continue;
                }
                text.push(c);
                i += 1;
            }
            lines.extend(text.split('\n').map(str::to_string));
        }
        lines
    }

    /// Does `text` match a bash line whose expansions are wildcards?
    fn matches_bash_line(bash: &str, text: &str) -> bool {
        let pieces: Vec<&str> = bash.split('\0').collect();
        let (first, rest) = pieces.split_first().expect("split yields one piece");
        let Some(mut remaining) = text.strip_prefix(first) else { return false };
        if rest.is_empty() {
            return remaining.is_empty();
        }
        let (last, middle) = rest.split_last().expect("rest is not empty");
        for piece in middle {
            let Some(at) = remaining.find(piece) else { return false };
            remaining = &remaining[at + piece.len()..];
        }
        remaining.ends_with(last)
    }

    /// The lines of a sample that are only an interpolated value, which no `die`
    /// literal can quote.
    fn interpolated_lines(rank: usize) -> &'static [&'static str] {
        match rank {
            3 => &["  #7 held by run:other (run)"],
            4 => &["  not a lease"],
            23 | 24 | 28 | 29 | 31 => &["  HTTP 502"],
            26 => &["  no profile"],
            40 => &["gh: HTTP 502"],
            48 => &["    crates/x/src/lib.rs", "    bin/y"],
            _ => &[],
        }
    }

    #[test]
    fn every_static_line_of_every_message_is_a_line_of_a_die_in_the_bash_gate() {
        // The table above and the enum were both written from one reading of the
        // bash, so a slip in that reading would pass both. This test reads the
        // bash file itself. Lines whose bash literal is only an interpolation
        // (fewer than eight characters of text) are left out, because they match
        // anything. The new refusals (F1 and the typestate's) have no bash line.
        const NEW: [usize; 2] = [49, 51];
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../bin/maintainer-merge"
        ))
        .expect("bin/maintainer-merge is two directories above rust/crates/mx-gate");
        let strong: Vec<String> = bash_die_lines(&source)
            .into_iter()
            .filter(|line| line.replace('\0', "").trim().len() >= 8)
            .collect();
        assert!(strong.len() > 100, "the scanner found {} lines; it read nothing", strong.len());
        let mut checked_lines = 0;
        for (at, (decision, text)) in samples().iter().enumerate() {
            if NEW.contains(&at) {
                continue;
            }
            for line in text.split('\n') {
                if line.is_empty() || interpolated_lines(at).contains(&line) {
                    continue;
                }
                assert!(
                    strong.iter().any(|bash| matches_bash_line(bash, line)),
                    "{decision:?}: no die line of bin/maintainer-merge matches {line:?}"
                );
                checked_lines += 1;
            }
        }
        assert!(checked_lines >= 90, "only {checked_lines} lines were compared");
    }

    #[test]
    fn the_bash_anchor_refuses_a_line_that_is_in_no_die() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../bin/maintainer-merge"
        ))
        .unwrap();
        let strong: Vec<String> = bash_die_lines(&source)
            .into_iter()
            .filter(|line| line.replace('\0', "").trim().len() >= 8)
            .collect();
        for invented in [
            "this sentence is in no bash die",
            "#7 is BLOCKD: a required review or check is missing",
            "",
        ] {
            assert!(!strong.iter().any(|bash| matches_bash_line(bash, invented)), "{invented:?}");
        }
        assert!(strong.iter().any(|bash| matches_bash_line(
            bash,
            "#7 is BLOCKED: a required review or check is missing"
        )));
    }
}
