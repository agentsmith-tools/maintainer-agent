//! A pull request on its way to a merge, as a typestate.
//!
//! ```text
//! Pr<Unverified> --verified(Evidence)--> Pr<Verified> --sealed(&Key)--> Pr<Receipted>
//!       |                                                                    ^
//!       +----------------- replay(Receipt<Signed>, path) --------------------+
//!                                                                            |
//! Merger::merge(Pr<Mergeable>) <------------ cleared(Conditions) -----------+
//! ```
//!
//! What each state proves, and what only the transition into it can build
//! (decision `P30-7`):
//!
//! - [`Unverified`]: a pull request number that passed [`PrNumber::parse`]. Nothing
//!   else is known.
//! - [`Verified`]: the gate itself produced a verdict for this number, in this
//!   process: an unsigned receipt body ([`Evidence`]). `verify` and `verify-deps`
//!   end here and sign.
//! - [`Receipted`]: a SIGNED receipt for this number exists: one this process
//!   sealed ([`Pr::sealed`]), or one a past verification left on disk and
//!   `mx_receipt::Stored::check` has authenticated ([`Pr::replay`]).
//!   `merge` and `update` enter here, because their process verified nothing:
//!   the signature is the memory of the journey.
//! - [`Mergeable`]: every condition of `merge` held at one head ([`Conditions`]).
//!   The head is the fence [`Merger::merge`] pins the merge to.
//!
//! Every field is private and no state has a public constructor, so a value of
//! `Pr<Mergeable>` exists only after `Pr::open` and the transitions above. The
//! examples below do not compile, one per illegal transition; each names the
//! error it must fail with, so an example that breaks for another reason (a typo,
//! a missing import) fails the test instead of passing it.
//!
//! The way in from outside the crate is two calls, and they stop at `Unverified`:
//!
//! ```
//! use mx_gate::{Pr, PrNumber};
//! let pr = Pr::open(PrNumber::parse("7").unwrap());
//! assert_eq!(pr.number().as_arg(), "7");
//! ```
//!
//! A pull request that is unverified cannot be sealed:
//!
//! ```compile_fail,E0599
//! use mx_gate::{Pr, PrNumber};
//! let pr = Pr::open(PrNumber::parse("7").unwrap());
//! let _ = pr.sealed(todo!());
//! ```
//!
//! nor cleared for a merge:
//!
//! ```compile_fail,E0599
//! use mx_gate::{Pr, PrNumber};
//! let pr = Pr::open(PrNumber::parse("7").unwrap());
//! let _ = pr.cleared(todo!());
//! ```
//!
//! A verified one has no receipt yet, so it cannot be cleared either:
//!
//! ```compile_fail,E0599
//! fn skip_the_receipt(pr: mx_gate::Pr<mx_gate::Verified>) {
//!     let _ = pr.cleared(todo!());
//! }
//! ```
//!
//! A receipted one is not mergeable, so the merger refuses it at the type:
//!
//! ```compile_fail,E0308
//! fn merge_without_conditions(merger: &mx_gate::Merger<'_>, pr: mx_gate::Pr<mx_gate::Receipted>) {
//!     let _ = merger.merge(pr);
//! }
//! ```
//!
//! Nobody builds a state by hand, because its fields are private:
//!
//! ```compile_fail,E0451
//! fn forge(number: mx_gate::PrNumber) {
//!     let _ = mx_gate::Pr::<mx_gate::Mergeable> { number, state: todo!() };
//! }
//! ```
//!
//! ```compile_fail,E0451
//! fn forge() {
//!     let _ = mx_gate::Receipted { receipt: todo!() };
//! }
//! ```
//!
//! ```compile_fail,E0423
//! fn forge() {
//!     let _ = mx_gate::Unverified(());
//! }
//! ```
//!
//! And the witnesses a transition asks for cannot be made outside the crate
//! either:
//!
//! ```compile_fail,E0624
//! fn forge(receipt: mx_receipt::Receipt<mx_receipt::Unsigned>) {
//!     let _ = mx_gate::Evidence::new(receipt);
//! }
//! ```
//!
//! ```compile_fail,E0624
//! fn forge() {
//!     let _ = mx_gate::ClaimsClear::new(todo!());
//! }
//! ```
//!
//! The spending call of the GitHub port is sealed the same way: it asks for a [`SpendToken`] only
//! [`Merger::merge`] can make, so code that holds a port and a pull request number still cannot
//! merge without a `Pr<Mergeable>` (review rv271 F3):
//!
//! ```compile_fail,E0061
//! fn shortcut(gh: &dyn mx_gate::GhPort, pr: &mx_gate::PrNumber, head: &mx_gate::CommitId) {
//!     let _ = gh.merge(pr, head);
//! }
//! ```
//!
//! ```compile_fail,E0624
//! fn forge_the_token() {
//!     let _ = mx_gate::SpendToken::new();
//! }
//! ```

use crate::{
    Approved, CommitId, Confirmed, Fresh, GateDecision, GhPort, GreenBoard, LeaseFree, PrNumber,
    StateClear,
};
use mx_receipt::{Key, Receipt, Signed, Unsigned};
use std::path::Path;

/// A pull request in state `S`.
#[derive(Debug)]
pub struct Pr<S> {
    number: PrNumber,
    state: S,
}

/// Nothing is proven.
#[derive(Debug)]
pub struct Unverified(());

/// This process produced a verdict: the unsigned body of a receipt.
#[derive(Debug)]
pub struct Verified {
    receipt: Receipt<Unsigned>,
}

/// A signed receipt for this pull request exists.
#[derive(Debug)]
pub struct Receipted {
    receipt: Receipt<Signed>,
}

/// Every condition of `merge` held at `head`.
#[derive(Debug)]
pub struct Mergeable {
    receipt: Receipt<Signed>,
    head: CommitId,
    conditions: Conditions,
}

/// What `verify`, `verify-part` or `verify-deps` observed, as the unsigned
/// receipt it will write. Only code inside this crate builds one: the verdict
/// logic that decides a mutation bit, or a diff is a pure bump, owns it.
#[derive(Debug)]
pub struct Evidence {
    receipt: Receipt<Unsigned>,
}

impl Evidence {
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "the verdict logic of the gate agent builds it")
    )]
    pub(crate) fn new(receipt: Receipt<Unsigned>) -> Self {
        Self { receipt }
    }
}

/// The witness that the claim checks of `merge` passed: every issue the pull
/// request closes was read (closing issues, author, holders, label, assignees,
/// commenters) and none of them takes somebody's work away. Only the code that
/// asks those questions builds one, so it has no public constructor.
#[derive(Debug)]
pub struct ClaimsClear {
    pr: PrNumber,
}

impl ClaimsClear {
    #[cfg_attr(not(test), allow(dead_code, reason = "the claim checks of the gate agent build it"))]
    pub(crate) fn new(pr: PrNumber) -> Self {
        Self { pr }
    }

    /// The pull request whose claims were checked.
    #[must_use]
    pub fn pr(&self) -> &PrNumber {
        &self.pr
    }
}

/// Every witness `merge` collects, in the order the bash collects them: the
/// lease, the identity, the review decision, the claim checks, the merge state,
/// the checks, the receipt's freshness, and the head they were all read at.
///
/// Each witness names the pull request it was made for, and `Fresh` names the
/// head it was computed against. [`Conditions::new`] refuses witnesses of
/// different pull requests, or a freshness for a head other than the fence, and
/// [`Pr::cleared`] refuses conditions of another pull request or a freshness for
/// another receipt head. The fields are private and the type is not `Clone`, so
/// the conditions of #7 cannot be kept and spent on #8 by a retry loop or a
/// watcher. It is a struct of witnesses and not a function of facts, so the code
/// that gathers them keeps the bash's order of reads and refusals.
#[derive(Debug)]
pub struct Conditions {
    pr: PrNumber,
    lease: LeaseFree,
    identity: Confirmed,
    approved: Approved,
    claims: ClaimsClear,
    state: StateClear,
    board: GreenBoard,
    fresh: Fresh,
    head: CommitId,
}

impl Conditions {
    /// Collects the witnesses. Every one must be for the same pull request as the
    /// lease, and the freshness must have been computed against `head`, the head
    /// `gh pr merge --match-head-commit` is pinned to.
    #[allow(
        clippy::too_many_arguments,
        reason = "one argument per witness of merge, in the bash order"
    )]
    pub fn new(
        lease: LeaseFree,
        identity: Confirmed,
        approved: Approved,
        claims: ClaimsClear,
        state: StateClear,
        board: GreenBoard,
        fresh: Fresh,
        head: CommitId,
    ) -> Result<Self, GateDecision> {
        let pr = lease.pr().clone();
        for other in [identity.pr(), approved.pr(), claims.pr(), state.pr(), board.pr(), fresh.pr()]
        {
            if other != &pr {
                return Err(GateDecision::ConditionsForOtherPr { pr, found: other.to_string() });
            }
        }
        if fresh.acted_head() != &head {
            return Err(GateDecision::ConditionsForOtherHead {
                expected: head.to_string(),
                found: fresh.acted_head().to_string(),
            });
        }
        Ok(Self { pr, lease, identity, approved, claims, state, board, fresh, head })
    }

    /// The pull request every witness is for.
    #[must_use]
    pub fn pr(&self) -> &PrNumber {
        &self.pr
    }

    /// The head the merge is pinned to.
    #[must_use]
    pub fn head(&self) -> &CommitId {
        &self.head
    }

    /// The green board, for the success line.
    #[must_use]
    pub fn board(&self) -> &GreenBoard {
        &self.board
    }

    /// The lease witness.
    #[must_use]
    pub fn lease(&self) -> &LeaseFree {
        &self.lease
    }

    /// The confirmed account.
    #[must_use]
    pub fn identity(&self) -> &Confirmed {
        &self.identity
    }

    /// The approval.
    #[must_use]
    pub fn approved(&self) -> &Approved {
        &self.approved
    }

    /// The claim checks.
    #[must_use]
    pub fn claims(&self) -> &ClaimsClear {
        &self.claims
    }

    /// The merge state.
    #[must_use]
    pub fn state(&self) -> &StateClear {
        &self.state
    }

    /// The freshness of the receipt.
    #[must_use]
    pub fn fresh(&self) -> &Fresh {
        &self.fresh
    }
}

impl<S> Pr<S> {
    /// The pull request.
    #[must_use]
    pub fn number(&self) -> &PrNumber {
        &self.number
    }
}

impl Pr<Unverified> {
    /// Starts with a pull request number and nothing proven.
    #[must_use]
    pub fn open(number: PrNumber) -> Self {
        Self { number, state: Unverified(()) }
    }

    /// The gate's own verdict for this pull request. The evidence must have been
    /// earned for the same number.
    pub fn verified(self, evidence: Evidence) -> Result<Pr<Verified>, GateDecision> {
        let found = evidence.receipt.body().pr;
        if found != self.number.value() {
            return Err(GateDecision::EvidenceForOtherPr { pr: self.number, found });
        }
        Ok(Pr { number: self.number, state: Verified { receipt: evidence.receipt } })
    }

    /// A receipt a past verification wrote and `Stored::check` has
    /// authenticated. `path` is where it was read from, for the refusal; the
    /// receipt must name this pull request, because the filename is not signed.
    pub fn replay(
        self,
        receipt: Receipt<Signed>,
        path: &Path,
    ) -> Result<Pr<Receipted>, GateDecision> {
        let found = receipt.body().pr.to_string();
        if found != self.number.as_arg() {
            return Err(GateDecision::ReceiptForOtherPr {
                path: path.to_path_buf(),
                found,
                pr: self.number,
            });
        }
        Ok(Pr { number: self.number, state: Receipted { receipt } })
    }
}

impl Pr<Verified> {
    /// Signs the verdict. Only a signed receipt can become bytes.
    #[must_use]
    pub fn sealed(self, key: &Key) -> Pr<Receipted> {
        Pr { number: self.number, state: Receipted { receipt: self.state.receipt.sign(key) } }
    }

    /// The unsigned receipt.
    #[must_use]
    pub fn receipt(&self) -> &Receipt<Unsigned> {
        &self.state.receipt
    }
}

impl Pr<Receipted> {
    /// Every condition held, for this pull request and this receipt. The only way to
    /// a `Pr<Mergeable>`.
    pub fn cleared(self, conditions: Conditions) -> Result<Pr<Mergeable>, GateDecision> {
        if conditions.pr != self.number {
            return Err(GateDecision::ConditionsForOtherPr {
                pr: self.number,
                found: conditions.pr.to_string(),
            });
        }
        if conditions.fresh.verified_head() != self.state.receipt.head() {
            return Err(GateDecision::ConditionsForOtherHead {
                expected: self.state.receipt.head().to_string(),
                found: conditions.fresh.verified_head().to_string(),
            });
        }
        Ok(Pr {
            number: self.number,
            state: Mergeable {
                receipt: self.state.receipt,
                head: conditions.head.clone(),
                conditions,
            },
        })
    }

    /// The signed receipt.
    #[must_use]
    pub fn receipt(&self) -> &Receipt<Signed> {
        &self.state.receipt
    }
}

impl Pr<Mergeable> {
    /// The head every condition was read at: the merge's fence.
    #[must_use]
    pub fn head(&self) -> &CommitId {
        &self.state.head
    }

    /// The receipt the merge spends.
    #[must_use]
    pub fn receipt(&self) -> &Receipt<Signed> {
        &self.state.receipt
    }

    /// The conditions that held.
    #[must_use]
    pub fn conditions(&self) -> &Conditions {
        &self.state.conditions
    }
}

/// Whether this run posts. `POST="${MAINTAINER_POST:-on}"`, and ONLY the exact
/// word `off` rehearses.
///
/// `gh pr merge` runs inside the gate, so no Bash deny rule sees it: without this
/// check a `POST=off` rehearsal could merge a pull request while reporting that
/// it reached nobody. Any other value, `OFF`, `Off`, ` off`, `0` or `false`,
/// posts (listed under "Found, not fixed").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posting {
    On,
    Off,
}

impl Posting {
    /// Reads `MAINTAINER_POST`. Unset and empty are `On`.
    #[must_use]
    pub fn from_setting(value: Option<&str>) -> Self {
        if value == Some("off") { Self::Off } else { Self::On }
    }
}

/// The right to call [`GhPort::merge`]. Only [`Merger::merge`] makes one, from a `Pr<Mergeable>`.
#[derive(Debug)]
pub struct SpendToken(());

impl SpendToken {
    #[cfg_attr(not(test), allow(dead_code, reason = "made by Merger::merge only"))]
    pub(crate) fn new() -> Self {
        Self(())
    }
}

/// What a merge did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    number: PrNumber,
    head: CommitId,
}

impl Merged {
    /// The pull request that was merged.
    #[must_use]
    pub fn number(&self) -> &PrNumber {
        &self.number
    }

    /// The head it was pinned to.
    #[must_use]
    pub fn head(&self) -> &CommitId {
        &self.head
    }
}

/// The one thing that merges. It exists only when the run posts, and it takes a
/// `Pr<Mergeable>` and nothing else.
pub struct Merger<'a> {
    gh: &'a dyn GhPort,
}

impl<'a> Merger<'a> {
    /// The merger of a run that posts, or `None` for a `POST=off` rehearsal.
    #[must_use]
    pub fn new(posting: Posting, gh: &'a dyn GhPort) -> Option<Self> {
        match posting {
            Posting::Off => None,
            Posting::On => Some(Self { gh }),
        }
    }

    /// `gh pr merge <n> --squash --delete-branch --match-head-commit <head>`,
    /// pinned to the head the conditions were read at. A refusal by `gh` is
    /// [`GateDecision::MergeFailed`]; the adapter has already shown what `gh` said.
    pub fn merge(&self, pr: Pr<Mergeable>) -> Result<Merged, GateDecision> {
        let Pr { number, state } = pr;
        self.gh
            .merge(&number, &state.head, SpendToken::new())
            .map_err(|failure| GateDecision::MergeFailed { output: failure.output })?;
        Ok(Merged { number, head: state.head })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Account, CheckBoard, FakeGh, GhCall, GhFailure, LeaseAnswer, MergeState, ProductionDiff,
        WhoAmI,
    };
    use mx_ports::Script;
    use mx_receipt::{DependencyFacts, Stamp};
    use std::path::PathBuf;

    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

    fn number(n: &str) -> PrNumber {
        PrNumber::parse(n).unwrap()
    }

    fn unsigned(pr: u64) -> Receipt<Unsigned> {
        Receipt::dependency(
            pr,
            HEAD,
            &DependencyFacts {
                files: "Cargo.lock (cargo-lock)".into(),
                changed_lines: 4,
                pins_verified: 0,
                images_verified: 0,
                pip_verified: 0,
            },
            &Stamp::new("2026-11-22T10:00:00+00:00", "gate"),
        )
    }

    fn key() -> Key {
        Key::from_file_bytes(b"test key\n").unwrap()
    }

    fn commit(text: &str) -> CommitId {
        CommitId::parse(text).unwrap()
    }

    /// Conditions for `pr`, with the receipt's head `verified` and the merge's fence `acted`.
    fn conditions_at(
        pr: &PrNumber,
        verified: &str,
        acted: &str,
    ) -> Result<Conditions, GateDecision> {
        let account = Account::new("vladimirrott").unwrap();
        let diff = ProductionDiff::from_git(pr, verified, Ok(String::new())).unwrap();
        Conditions::new(
            LeaseFree::from_answer(pr, crate::Action::Merge, Ok(LeaseAnswer::Free)).unwrap(),
            Confirmed::check(pr, WhoAmI::Login("vladimirrott".into()), &account).unwrap(),
            Approved::from_decision(pr, "APPROVED").unwrap(),
            ClaimsClear::new(pr.clone()),
            MergeState::Clean.clear(pr).unwrap(),
            CheckBoard { pass: 3, skip: 1, fail: 0, pending: 0, cancel: 0, total: 4 }
                .green(pr)
                .unwrap(),
            Fresh::from_diff(&diff, &commit(acted)).unwrap(),
            commit(acted),
        )
    }

    fn conditions(pr: &PrNumber) -> Conditions {
        conditions_at(pr, HEAD, HEAD).unwrap()
    }

    fn mergeable(n: &str) -> Pr<Mergeable> {
        let pr = number(n);
        let value = pr.value();
        Pr::open(pr.clone())
            .verified(Evidence::new(unsigned(value)))
            .unwrap()
            .sealed(&key())
            .cleared(conditions(&pr))
            .unwrap()
    }

    fn receipted(n: u64) -> Pr<Receipted> {
        Pr::open(number(&n.to_string()))
            .verified(Evidence::new(unsigned(n)))
            .unwrap()
            .sealed(&key())
    }

    #[test]
    fn the_conditions_of_seven_do_not_clear_the_receipt_of_eight() {
        // rv264 B1: the witnesses of #7 cleared #8's receipt.
        let refused = receipted(8).cleared(conditions(&number("7"))).unwrap_err();
        assert_eq!(
            refused,
            GateDecision::ConditionsForOtherPr { pr: number("8"), found: "7".into() }
        );
    }

    #[test]
    fn a_witness_of_another_pull_request_cannot_join_the_conditions() {
        let seven = number("7");
        let eight = number("8");
        let account = Account::new("vladimirrott").unwrap();
        let diff = ProductionDiff::from_git(&seven, HEAD, Ok(String::new())).unwrap();
        let refused = Conditions::new(
            LeaseFree::from_answer(&seven, crate::Action::Merge, Ok(LeaseAnswer::Free)).unwrap(),
            Confirmed::check(&seven, WhoAmI::Login("vladimirrott".into()), &account).unwrap(),
            Approved::from_decision(&eight, "APPROVED").unwrap(),
            ClaimsClear::new(seven.clone()),
            MergeState::Clean.clear(&seven).unwrap(),
            CheckBoard { pass: 1, skip: 0, fail: 0, pending: 0, cancel: 0, total: 1 }
                .green(&seven)
                .unwrap(),
            Fresh::from_diff(&diff, &commit(HEAD)).unwrap(),
            commit(HEAD),
        )
        .unwrap_err();
        assert_eq!(refused, GateDecision::ConditionsForOtherPr { pr: seven, found: "8".into() });
    }

    #[test]
    fn every_witness_is_checked_against_the_lease() {
        // One witness at a time is for #8; each must refuse.
        let seven = number("7");
        let eight = number("8");
        let account = Account::new("vladimirrott").unwrap();
        let ok_diff = ProductionDiff::from_git(&seven, HEAD, Ok(String::new())).unwrap();
        let bad_diff = ProductionDiff::from_git(&eight, HEAD, Ok(String::new())).unwrap();
        let board = |p: &PrNumber| {
            CheckBoard { pass: 1, skip: 0, fail: 0, pending: 0, cancel: 0, total: 1 }
                .green(p)
                .unwrap()
        };
        for which in 0..6 {
            let pick = |i: usize| if i == which { &eight } else { &seven };
            let refused = Conditions::new(
                LeaseFree::from_answer(&seven, crate::Action::Merge, Ok(LeaseAnswer::Free))
                    .unwrap(),
                Confirmed::check(pick(0), WhoAmI::Login("vladimirrott".into()), &account).unwrap(),
                Approved::from_decision(pick(1), "APPROVED").unwrap(),
                ClaimsClear::new(pick(2).clone()),
                MergeState::Clean.clear(pick(3)).unwrap(),
                board(pick(4)),
                Fresh::from_diff(if which == 5 { &bad_diff } else { &ok_diff }, &commit(HEAD))
                    .unwrap(),
                commit(HEAD),
            );
            assert_eq!(
                refused.unwrap_err(),
                GateDecision::ConditionsForOtherPr { pr: seven.clone(), found: "8".into() },
                "witness {which}"
            );
        }
    }

    #[test]
    fn a_freshness_computed_for_another_head_than_the_fence_is_refused() {
        let other = "fedcba9876543210fedcba9876543210fedcba98";
        let diff = ProductionDiff::from_git(&number("7"), HEAD, Ok(String::new())).unwrap();
        let account = Account::new("vladimirrott").unwrap();
        let pr = number("7");
        let refused = Conditions::new(
            LeaseFree::from_answer(&pr, crate::Action::Merge, Ok(LeaseAnswer::Free)).unwrap(),
            Confirmed::check(&pr, WhoAmI::Login("vladimirrott".into()), &account).unwrap(),
            Approved::from_decision(&pr, "APPROVED").unwrap(),
            ClaimsClear::new(pr.clone()),
            MergeState::Clean.clear(&pr).unwrap(),
            CheckBoard { pass: 1, skip: 0, fail: 0, pending: 0, cancel: 0, total: 1 }
                .green(&pr)
                .unwrap(),
            Fresh::from_diff(&diff, &commit(HEAD)).unwrap(),
            commit(other),
        )
        .unwrap_err();
        assert_eq!(
            refused,
            GateDecision::ConditionsForOtherHead { expected: other.into(), found: HEAD.into() }
        );
    }

    #[test]
    fn a_freshness_computed_against_another_receipt_head_does_not_clear() {
        // The receipt says HEAD, the diff was taken from another verified head.
        let other = "fedcba9876543210fedcba9876543210fedcba98";
        let refused =
            receipted(7).cleared(conditions_at(&number("7"), other, HEAD).unwrap()).unwrap_err();
        assert_eq!(
            refused,
            GateDecision::ConditionsForOtherHead { expected: HEAD.into(), found: other.into() }
        );
    }

    #[test]
    fn the_whole_path_ends_in_a_pull_request_that_knows_its_head_and_its_receipt() {
        let pr = mergeable("7");
        assert_eq!(pr.number().as_arg(), "7");
        assert_eq!(pr.head().as_str(), HEAD);
        assert_eq!(pr.receipt().head(), HEAD);
        assert_eq!(pr.conditions().board().pass(), 3);
    }

    #[test]
    fn evidence_for_another_pull_request_is_refused() {
        let refused = Pr::open(number("7")).verified(Evidence::new(unsigned(8))).unwrap_err();
        assert_eq!(refused, GateDecision::EvidenceForOtherPr { pr: number("7"), found: 8 });
        assert_eq!(refused.to_string(), "the verification evidence was earned for #8, not #7");
    }

    #[test]
    fn a_verified_pull_request_keeps_its_unsigned_receipt_until_it_is_sealed() {
        let verified = Pr::open(number("7")).verified(Evidence::new(unsigned(7))).unwrap();
        assert_eq!(verified.receipt().body().pr, 7);
        let sealed = verified.sealed(&key());
        assert_eq!(sealed.receipt().signature().len(), 64);
    }

    #[test]
    fn a_receipt_read_back_from_disk_enters_at_receipted() {
        let signed = unsigned(7).sign(&key());
        let pr =
            Pr::open(number("7")).replay(signed, &PathBuf::from("/s/receipts/7.json")).unwrap();
        assert_eq!(pr.receipt().head(), HEAD);
    }

    #[test]
    fn a_copy_of_another_pull_requests_receipt_is_refused_at_replay() {
        // bash receipt_check: `cp 1.json 2.json` verifies perfectly and proves a
        // guard bites on a different pull request, because the filename is not signed.
        let signed = unsigned(1).sign(&key());
        let refused =
            Pr::open(number("2")).replay(signed, &PathBuf::from("/s/receipts/2.json")).unwrap_err();
        assert_eq!(
            refused,
            GateDecision::ReceiptForOtherPr {
                path: PathBuf::from("/s/receipts/2.json"),
                found: "1".into(),
                pr: number("2")
            }
        );
    }

    #[test]
    fn an_asserted_receipt_replays_because_replay_only_asks_for_a_signed_one() {
        // Found, not fixed (rv264 O4): the refusal of an asserted receipt during an
        // unattended run lives in `mx_receipt::Stored::check`, and `replay` accepts
        // any `Receipt<Signed>`, so a caller that skips `check` can spend one.
        let asserted = Receipt::asserted(
            7,
            HEAD,
            "claim",
            &Stamp::new("2026-11-22T10:00:00+00:00", "human"),
            false,
        )
        .unwrap()
        .sign(&key());
        assert!(
            Pr::open(number("7")).replay(asserted, &PathBuf::from("/s/receipts/7.json")).is_ok()
        );
    }

    #[test]
    fn a_receipt_of_007_is_not_the_receipt_of_7() {
        // The bash compares the receipt's `pr` with the argument as text.
        let signed = unsigned(7).sign(&key());
        assert!(
            Pr::open(number("007")).replay(signed, &PathBuf::from("/s/receipts/007.json")).is_err()
        );
    }

    #[test]
    fn only_the_exact_word_off_rehearses() {
        assert_eq!(Posting::from_setting(Some("off")), Posting::Off);
        for setting in [
            None,
            Some(""),
            Some("on"),
            Some("OFF"),
            Some("Off"),
            Some(" off"),
            Some("off "),
            Some("0"),
            Some("false"),
            Some("no"),
        ] {
            assert_eq!(Posting::from_setting(setting), Posting::On, "{setting:?}");
        }
    }

    #[test]
    fn a_rehearsal_has_no_merger_and_a_run_that_posts_has_one() {
        let gh = FakeGh::new();
        assert!(Merger::new(Posting::Off, &gh).is_none());
        assert!(Merger::new(Posting::On, &gh).is_some());
        assert!(gh.calls().is_empty(), "making a merger reaches nobody");
    }

    #[test]
    fn the_merge_is_pinned_to_the_head_the_conditions_were_read_at() {
        let gh = FakeGh::new();
        gh.script_merge(Script::always(Ok(())));
        let live = Merger::new(Posting::On, &gh).unwrap();
        let done = live.merge(mergeable("7")).unwrap();
        assert_eq!((done.number(), done.head()), (&number("7"), &CommitId::parse(HEAD).unwrap()));
        assert_eq!(
            gh.calls(),
            vec![GhCall::Merge { pr: number("7"), head: CommitId::parse(HEAD).unwrap() }]
        );
    }

    #[test]
    fn a_merge_that_gh_refuses_is_the_bash_refusal() {
        let gh = FakeGh::new();
        gh.script_merge(Script::always(Err(GhFailure {
            exit: 1,
            output: "head branch was modified".into(),
        })));
        let refused = Merger::new(Posting::On, &gh).unwrap().merge(mergeable("7")).unwrap_err();
        assert_eq!(
            refused,
            GateDecision::MergeFailed { output: "head branch was modified".into() }
        );
        assert_eq!(refused.to_string(), "merge failed");
    }
}
