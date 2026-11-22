//! The refusals of `maintainer-merge update` (bash `cmd_update`, lines 2119 to 2228), each with its exact text.
//!
//! `update_refuse3` writes `maintainer-merge: will not update #N: <why>` and exits 3: "this needs the
//! author, not a retry". Every other refusal is a `die` and exits 1. The flow that raises them is the
//! gate agent's (brief p3b).

#![allow(
    clippy::doc_markdown,
    reason = "each variant quotes the bash die line it replaces, with its $variables"
)]

use crate::PrNumber;
use mx_suite::text::indent4;
use thiserror::Error;

/// Every refusal of `maintainer-merge update`: its `die` lines (exit 1) and `update_refuse3` (exit 3,
/// "this needs the author, not a retry").
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UpdateRefusal {
    /// bash line 2132: `could not read '$k' of #$pr from GitHub, so I will not touch its branch`
    #[error("could not read '{k}' of #{pr} from GitHub, so I will not touch its branch")]
    FactUnreadable { k: String, pr: PrNumber },
    /// bash line 2136: `the head of #$pr read as '$head', which is not a 40-hex commit; refusing to name it to GitHub`
    #[error(
        "the head of #{pr} read as '{head}', which is not a 40-hex commit; refusing to name it to GitHub"
    )]
    HeadNotFull { pr: PrNumber, head: String },
    /// bash line 2142: `#$pr has conflicts (mergeStateStatus DIRTY). Conflicts are never resolved by this agent; the author `
    #[error(
        "#{pr} has conflicts (mergeStateStatus DIRTY). Conflicts are never resolved by this agent; the author resolves them."
    )]
    Conflicts { pr: PrNumber },
    /// bash line 2143: `#$pr mergeStateStatus is '$state', not BEHIND, so Update branch is not what is missing. Refusing.`
    #[error(
        "#{pr} mergeStateStatus is '{state}', not BEHIND, so Update branch is not what is missing. Refusing."
    )]
    StateNotBehind { pr: PrNumber, state: String },
    /// bash line 2149: `#$pr is Dependabot's. A commit of ours on its branch stops it rebasing, so use: maintainer deps reba`
    #[error(
        "#{pr} is Dependabot's. A commit of ours on its branch stops it rebasing, so use: maintainer deps rebase {pr}"
    )]
    Dependabots { pr: PrNumber },
    /// bash line 2153: `#$pr reviewDecision is '$decision', not APPROVED. An unapproved pull request is not waiting only for`
    #[error(
        "#{pr} reviewDecision is '{decision}', not APPROVED. An unapproved pull request is not waiting only for Update branch."
    )]
    NotApprovedForUpdate { pr: PrNumber, decision: String },
    /// bash line 2158: `$ACCOUNT has no standing approval of #$pr at ${head:0:8}. An approval of an older head, somebody els`
    #[error(
        "{account} has no standing approval of #{pr} at {head_8}. An approval of an older head, somebody else's approval, or a later change request does not count."
    )]
    NoStandingApproval { account: String, pr: PrNumber, head_8: String },
    /// bash line 2159: `could not read the reviews of #$pr, so I cannot say $ACCOUNT approved ${head:0:8}. Refusing.`
    #[error(
        "could not read the reviews of #{pr}, so I cannot say {account} approved {head_8}. Refusing."
    )]
    ReviewsUnreadable { pr: PrNumber, account: String, head_8: String },
    /// bash line 2189: `the head of #$pr moved after I read it (GitHub: expected_head_sha did not match). Nothing was update`
    #[error(
        "the head of #{pr} moved after I read it (GitHub: expected_head_sha did not match). Nothing was updated. Read it again:\n{}",
        indent4(put)
    )]
    HeadMoved { pr: PrNumber, put: String },
    /// bash line 2195: `GitHub refused the update of #$pr, so nothing was updated:`
    #[error("GitHub refused the update of #{pr}, so nothing was updated:\n{}", indent4(put))]
    GithubRefused { pr: PrNumber, put: String },
    /// bash line 2215: `GitHub accepted the update of #$pr, but $reads reads did not show it done (head ${nhead:0:8}, state `
    #[error(
        "GitHub accepted the update of #{pr}, but {reads} reads did not show it done (head {nhead_8}, state {nstate}): not confirmed.\n  Do not assume it worked. Read the pull request again before acting on it."
    )]
    NotConfirmed { pr: PrNumber, reads: String, nhead_8: String, nstate: String },
    /// bash line 0: `will not update #$pr: the author must press Update branch; they have not allowed maintainer edits`
    #[error(
        "will not update #{pr}: the author must press Update branch; they have not allowed maintainer edits"
    )]
    NeedsAuthorNoEdits { pr: PrNumber },
    /// bash line 0: `will not update #$pr: the author must press Update branch; GitHub did not let this account push to t`
    #[error(
        "will not update #{pr}: the author must press Update branch; GitHub did not let this account push to the head repository:\n{}",
        indent4(put)
    )]
    NeedsAuthorNoPush { pr: PrNumber, put: String },
}

impl UpdateRefusal {
    /// The process exit code.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::FactUnreadable { .. }
            | Self::HeadNotFull { .. }
            | Self::Conflicts { .. }
            | Self::StateNotBehind { .. }
            | Self::Dependabots { .. }
            | Self::NotApprovedForUpdate { .. }
            | Self::NoStandingApproval { .. }
            | Self::ReviewsUnreadable { .. }
            | Self::HeadMoved { .. }
            | Self::GithubRefused { .. }
            | Self::NotConfirmed { .. } => 1,
            Self::NeedsAuthorNoEdits { .. } | Self::NeedsAuthorNoPush { .. } => 3,
        }
    }
}

#[cfg(test)]
pub(crate) fn samples() -> Vec<(UpdateRefusal, String)> {
    vec![
            (UpdateRefusal::FactUnreadable { k: "⟦k⟧".to_string(), pr: PrNumber::parse("7").unwrap() }, "could not read '⟦k⟧' of #7 from GitHub, so I will not touch its branch".to_string()),
            (UpdateRefusal::HeadNotFull { pr: PrNumber::parse("7").unwrap(), head: "⟦head⟧".to_string() }, "the head of #7 read as '⟦head⟧', which is not a 40-hex commit; refusing to name it to GitHub".to_string()),
            (UpdateRefusal::Conflicts { pr: PrNumber::parse("7").unwrap() }, "#7 has conflicts (mergeStateStatus DIRTY). Conflicts are never resolved by this agent; the author resolves them.".to_string()),
            (UpdateRefusal::StateNotBehind { pr: PrNumber::parse("7").unwrap(), state: "⟦state⟧".to_string() }, "#7 mergeStateStatus is '⟦state⟧', not BEHIND, so Update branch is not what is missing. Refusing.".to_string()),
            (UpdateRefusal::Dependabots { pr: PrNumber::parse("7").unwrap() }, "#7 is Dependabot's. A commit of ours on its branch stops it rebasing, so use: maintainer deps rebase 7".to_string()),
            (UpdateRefusal::NotApprovedForUpdate { pr: PrNumber::parse("7").unwrap(), decision: "⟦decision⟧".to_string() }, "#7 reviewDecision is '⟦decision⟧', not APPROVED. An unapproved pull request is not waiting only for Update branch.".to_string()),
            (UpdateRefusal::NoStandingApproval { account: "⟦account⟧".to_string(), pr: PrNumber::parse("7").unwrap(), head_8: "⟦head_8⟧".to_string() }, "⟦account⟧ has no standing approval of #7 at ⟦head_8⟧. An approval of an older head, somebody else's approval, or a later change request does not count.".to_string()),
            (UpdateRefusal::ReviewsUnreadable { pr: PrNumber::parse("7").unwrap(), account: "⟦account⟧".to_string(), head_8: "⟦head_8⟧".to_string() }, "could not read the reviews of #7, so I cannot say ⟦account⟧ approved ⟦head_8⟧. Refusing.".to_string()),
            (UpdateRefusal::HeadMoved { pr: PrNumber::parse("7").unwrap(), put: "⟦put⟧".to_string() }, "the head of #7 moved after I read it (GitHub: expected_head_sha did not match). Nothing was updated. Read it again:\n    ⟦put⟧".to_string()),
            (UpdateRefusal::GithubRefused { pr: PrNumber::parse("7").unwrap(), put: "⟦put⟧".to_string() }, "GitHub refused the update of #7, so nothing was updated:\n    ⟦put⟧".to_string()),
            (UpdateRefusal::NotConfirmed { pr: PrNumber::parse("7").unwrap(), reads: "⟦reads⟧".to_string(), nhead_8: "⟦nhead_8⟧".to_string(), nstate: "⟦nstate⟧".to_string() }, "GitHub accepted the update of #7, but ⟦reads⟧ reads did not show it done (head ⟦nhead_8⟧, state ⟦nstate⟧): not confirmed.\n  Do not assume it worked. Read the pull request again before acting on it.".to_string()),
            (UpdateRefusal::NeedsAuthorNoEdits { pr: PrNumber::parse("7").unwrap() }, "will not update #7: the author must press Update branch; they have not allowed maintainer edits".to_string()),
            (UpdateRefusal::NeedsAuthorNoPush { pr: PrNumber::parse("7").unwrap(), put: "⟦put⟧".to_string() }, "will not update #7: the author must press Update branch; GitHub did not let this account push to the head repository:\n    ⟦put⟧".to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The position of a variant in `samples()`; no `_` arm, so a new variant fails to
    /// compile here until it has an index and a row.
    fn rank(value: &UpdateRefusal) -> usize {
        match value {
            UpdateRefusal::FactUnreadable { .. } => 0,
            UpdateRefusal::HeadNotFull { .. } => 1,
            UpdateRefusal::Conflicts { .. } => 2,
            UpdateRefusal::StateNotBehind { .. } => 3,
            UpdateRefusal::Dependabots { .. } => 4,
            UpdateRefusal::NotApprovedForUpdate { .. } => 5,
            UpdateRefusal::NoStandingApproval { .. } => 6,
            UpdateRefusal::ReviewsUnreadable { .. } => 7,
            UpdateRefusal::HeadMoved { .. } => 8,
            UpdateRefusal::GithubRefused { .. } => 9,
            UpdateRefusal::NotConfirmed { .. } => 10,
            UpdateRefusal::NeedsAuthorNoEdits { .. } => 11,
            UpdateRefusal::NeedsAuthorNoPush { .. } => 12,
        }
    }

    #[test]
    fn every_variant_has_exactly_one_sample() {
        let table = samples();
        assert_eq!(table.len(), 13, "one row per variant");
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
        assert_eq!(seen, std::collections::BTreeSet::from([1, 3]));
    }

    #[test]
    fn only_update_refuse3_exits_3_and_every_other_refusal_exits_1() {
        for (value, _) in samples() {
            let want = matches!(
                value,
                UpdateRefusal::NeedsAuthorNoEdits { .. } | UpdateRefusal::NeedsAuthorNoPush { .. }
            );
            assert_eq!(value.exit_code(), if want { 3 } else { 1 }, "{value:?}");
        }
    }
}
