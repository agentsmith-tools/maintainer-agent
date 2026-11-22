//! The merge gate as types: what `bin/maintainer-merge` knows, proves and refuses,
//! without a process, a file or a network.
//!
//! This crate is the interface of the merge-gate port. It holds no behaviour that
//! the bash does not already have in one line: the values a decision is made of,
//! the ports a decision reads through, the refusals a decision ends in, and a
//! typestate that makes `merge` impossible without a receipt, a lease and an
//! approval. The decisions themselves (`receipt_check`, `receipt_covers`,
//! `receipt_fresh`, the claim checks, `update`, the real adapters of the ports) are the work of
//! the gate agent (brief `p3b`), and `verify-deps` is the dependency agent's
//! (`p3c`), against these signatures. A change to a signature here is a change to the
//! briefs of p3a, p3b and p3c, so it goes in a pull request all three read.
//!
//! # Layout
//!
//! - [`PrNumber`], [`IssueNumber`], [`CommitId`], [`HeadArg`], [`Account`]: the
//!   identifiers, each checked once. A pull request number that is not a
//!   [`PrNumber`] cannot reach `show` (plan decision F2).
//! - [`GateDecision`]: every refusal `merge` prints, with its exact text.
//! - [`PrFacts`], [`MergeState`], [`CheckBoard`], [`Approved`], [`StateClear`],
//!   [`GreenBoard`]: what GitHub says, and the witnesses that its answers are good enough.
//! - [`GhPort`] and [`FakeGh`]; [`IdentityPort`] and [`FakeIdentity`]; [`Helper`]
//!   and [`FakeHelper`]: the three ports `merge` reads through.
//! - [`ProductionDiff`] and [`Fresh`]: the staleness check, with F1 in its type.
//! - [`Pr`], [`Conditions`], [`Merger`], [`Posting`]: the typestate.
//! - [`deps`]: the types of `verify-deps`.
//!
//! # What each bash function becomes
//!
//! | bash | Rust |
//! |---|---|
//! | `pr_or_die` | [`PrNumber::parse`] |
//! | `[ ${#sha} -ge 7 ]` | [`HeadArg::parse`] |
//! | `lease_gate` | [`Helper::lease_check`], then [`LeaseFree::from_answer`] |
//! | `require_prod_globs` | [`mx_suite::ProdGlobs`] and [`GateDecision::NoProdGlobs`] |
//! | `identity` | [`IdentityPort::who_am_i`], then [`Confirmed::check`] |
//! | `receipt_check` | `mx_receipt::Stored::check`, then [`Pr::replay`] |
//! | `receipt_covers` | [`GhPort::changed_files`] and the suite agent's `suites_cover_production` |
//! | `receipt_fresh` | [`ProductionDiff::from_git`], then [`Fresh::from_diff`] |
//! | the claim checks of `cmd_merge` | [`GhPort::closing_issues`], [`GhPort::author`], [`Helper::holders`], [`GhPort::issue_has_label`], [`GhPort::issue_assignees`], [`GhPort::issue_commenters`] |
//! | `[ "$review" = APPROVED ]` | [`Approved::from_decision`] |
//! | `case "$state" in ...` | [`MergeState::clear`] |
//! | the `gh pr checks` counts | [`CheckBoard::from_gh_json`], then [`CheckBoard::green`] |
//! | `gh pr merge ... --match-head-commit` | [`Merger::merge`] |
//! | `POST=off` | [`Posting`] and [`Merger::new`] |
//! | `update_facts`, `update_approved_by_me` | [`GhPort::update_facts`], [`GhPort::reviews`] |
//! | `gh api -X PUT .../update-branch` | [`GhPort::update_branch`] |
//!
//! # Decisions
//!
//! F1 (the staleness `git diff` fails closed), F2 (`show` validates the number)
//! and F3 (a suite name is validated, in `mx-suite`) are the plan's. `P30-n` are
//! this pull request's, each listed in its body.

#[cfg(test)]
mod anchor;
pub mod app;
mod cli;
mod decision;
pub mod deps;
mod engine;
#[cfg(test)]
mod engine_tests;
mod facts;
mod gh;
mod helper;
mod identity;
mod ids;
mod logs;
mod pending;
mod ports;
mod pr;
mod progress;
pub mod real;
#[cfg(test)]
mod real_tests;
mod staleness;
#[cfg(test)]
mod testkit;
mod update;
mod verify;
mod verify_run;
#[cfg(test)]
mod verify_tests;

pub use cli::{Command, CommandError, Exit, USAGE, parse_command};
pub use decision::{Action, GateDecision, PROGRAM};
pub use engine::{
    Dirs, Gate, GateConfig, Ports, Printed, RuntimeProbe, StampSource, UNATTENDED_ASSERT,
};
pub use facts::{
    Approved, Check, CheckBoard, GreenBoard, MergeState, PrFacts, PrState, StateClear,
};
pub use gh::{FakeGh, GhCall, GhFailure, GhPort, Review, UpdateFacts};
pub use helper::{
    FakeHelper, Helper, HelperCall, HelperMissing, HoldersFailure, LeaseAnswer, LeaseFree,
};
pub use identity::{Confirmed, FakeIdentity, IdentityCall, IdentityPort, WhoAmI};
pub use ids::{
    Account, CommitId, HeadArg, IssueNumber, NoAccount, NotACommitId, PrNumber, show_receipt,
};
pub use logs::{first_failure_line, looks_environmental, looks_killed};
pub use ports::{
    FakeGateGit, FakeLockPort, FakeSay, GateGit, GitCall, LockError, LockGuard, LockKind, LockPort,
    Say,
};
pub use pr::{
    ClaimsClear, Conditions, Evidence, Mergeable, Merged, Merger, Posting, Pr, Receipted,
    Unverified, Verified,
};
pub use progress::Progress;
pub use staleness::{DiffFailure, Fresh, ProductionDiff};
pub use update::UpdateRefusal;
pub use verify::VerifyRefusal;
pub use verify_run::VerifySettings;
