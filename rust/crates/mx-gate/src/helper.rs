//! The Python helper `maintainer`: the lease and the issue holders.

use crate::{Action, GateDecision, IssueNumber, PrNumber};
use mx_ports::Script;
use mx_suite::Recorder;
use std::sync::{Mutex, PoisonError};

/// There is no `maintainer` helper beside the gate or on `PATH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelperMissing;

/// What `maintainer lease check <pr>` answered, with its output and standard error together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseAnswer {
    /// Status 0: nobody holds it, or the holder is this actor.
    Free,
    /// Status 1: another actor holds it. `output` says who.
    Held { output: String },
    /// Any other status, including a helper that could not start: the question
    /// was not answered.
    Unreadable { output: String },
}

/// Why `maintainer holders <issue>` gave no list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoldersFailure {
    /// No helper to ask.
    HelperMissing,
    /// The helper exited non-zero. `output` is what it printed. Exit 0 with no
    /// output means nobody holds the issue; any non-zero status means the
    /// question was never answered.
    Failed { output: String },
}

/// The two questions the gate puts to the helper.
///
/// The gate asks the helper because the helper is "the one place that knows who
/// holds what", and deriving it a second time in the gate is how two answers
/// drift apart. The real adapter starts it through [`mx_ports::Proc`] with
/// `MAINTAINER_STATE`, `MAINTAINER_SLUG` and `MAINTAINER_PROFILE` in the
/// environment, as the bash does, and finds it beside the gate or on `PATH`.
pub trait Helper {
    /// `maintainer lease check <pr> 2>&1`.
    fn lease_check(&self, pr: &PrNumber) -> Result<LeaseAnswer, HelperMissing>;

    /// `maintainer holders <issue> 2>&1`: the logins pointed at the issue by the
    /// maintainer and not released, one per line. Empty lines are dropped.
    fn holders(&self, issue: IssueNumber) -> Result<Vec<String>, HoldersFailure>;
}

/// The witness that nobody else holds a live lease on the pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseFree {
    pr: PrNumber,
}

impl LeaseFree {
    /// bash `lease_gate`: status 0 is free, 1 is held, anything else could not be
    /// read, and a missing helper could not be asked. Each refuses, because a gate
    /// that could not ask has not said "free".
    pub fn from_answer(
        pr: &PrNumber,
        action: Action,
        answer: Result<LeaseAnswer, HelperMissing>,
    ) -> Result<Self, GateDecision> {
        let pr = pr.clone();
        match answer {
            Ok(LeaseAnswer::Free) => Ok(Self { pr }),
            Ok(LeaseAnswer::Held { output }) => Err(GateDecision::LeaseHeld { pr, action, output }),
            Ok(LeaseAnswer::Unreadable { output }) => {
                Err(GateDecision::LeaseUnreadable { pr, action, output })
            }
            Err(HelperMissing) => Err(GateDecision::LeaseHelperMissing { pr, action }),
        }
    }

    /// The pull request this witness is about.
    #[must_use]
    pub fn pr(&self) -> &PrNumber {
        &self.pr
    }
}

/// One call to a [`FakeHelper`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperCall {
    LeaseCheck(PrNumber),
    Holders(IssueNumber),
}

/// A scripted [`Helper`].
#[derive(Debug, Default)]
pub struct FakeHelper {
    calls: Recorder<HelperCall>,
    lease: Mutex<Script<Result<LeaseAnswer, HelperMissing>>>,
    holders: Mutex<Script<Result<Vec<String>, HoldersFailure>>>,
}

impl FakeHelper {
    /// A fake with no scripted answers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Scripts `lease_check`.
    pub fn script_lease(&self, script: Script<Result<LeaseAnswer, HelperMissing>>) {
        *self.lease.lock().unwrap_or_else(PoisonError::into_inner) = script;
    }

    /// Scripts `holders`.
    pub fn script_holders(&self, script: Script<Result<Vec<String>, HoldersFailure>>) {
        *self.holders.lock().unwrap_or_else(PoisonError::into_inner) = script;
    }

    /// Every call so far.
    #[must_use]
    pub fn calls(&self) -> Vec<HelperCall> {
        self.calls.entries()
    }
}

impl Helper for FakeHelper {
    fn lease_check(&self, pr: &PrNumber) -> Result<LeaseAnswer, HelperMissing> {
        self.calls.push(HelperCall::LeaseCheck(pr.clone()));
        self.lease.lock().unwrap_or_else(PoisonError::into_inner).next("Helper::lease_check")
    }

    fn holders(&self, issue: IssueNumber) -> Result<Vec<String>, HoldersFailure> {
        self.calls.push(HelperCall::Holders(issue));
        self.holders.lock().unwrap_or_else(PoisonError::into_inner).next("Helper::holders")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> PrNumber {
        PrNumber::parse("7").unwrap()
    }

    #[test]
    fn a_free_lease_passes_and_remembers_the_pull_request() {
        let free = LeaseFree::from_answer(&pr(), Action::Merge, Ok(LeaseAnswer::Free)).unwrap();
        assert_eq!(free.pr(), &pr());
    }

    #[test]
    fn a_lease_held_by_another_actor_refuses_and_quotes_the_helper() {
        let refused = LeaseFree::from_answer(
            &pr(),
            Action::Merge,
            Ok(LeaseAnswer::Held { output: "#7 held by run:other (run)".into() }),
        )
        .unwrap_err();
        // bash lease_gate, status 1:
        //   die "will not $2 #$1: another actor has leased it.
        //     $out
        //     Ask them, wait for it to expire, or have them \`maintainer lease release $1\`."
        assert_eq!(
            refused.to_string(),
            "will not merge #7: another actor has leased it.\n  #7 held by run:other (run)\n  Ask them, wait for it to expire, or have them `maintainer lease release 7`."
        );
    }

    #[test]
    fn an_unreadable_lease_refuses_because_the_question_was_not_answered() {
        let refused = LeaseFree::from_answer(
            &pr(),
            Action::Verify,
            Ok(LeaseAnswer::Unreadable { output: "not a lease".into() }),
        )
        .unwrap_err();
        // bash: die "could not read the lease on #$1, so I will not $2 it:\n  $out"
        assert_eq!(
            refused.to_string(),
            "could not read the lease on #7, so I will not verify it:\n  not a lease"
        );
    }

    #[test]
    fn a_missing_helper_refuses_and_names_the_gate() {
        let refused =
            LeaseFree::from_answer(&pr(), Action::Update, Err(HelperMissing)).unwrap_err();
        assert_eq!(refused, GateDecision::LeaseHelperMissing { pr: pr(), action: Action::Update });
        // bash: die "cannot tell whether #$1 is leased, so I will not $2 it:
        //   no \`maintainer\` helper beside $(basename ...) or on PATH.
        //   Install the agent (./install.sh) or put \`maintainer\` on PATH, then retry."
        assert_eq!(
            refused.to_string(),
            "cannot tell whether #7 is leased, so I will not update it:\n  no `maintainer` helper beside maintainer-merge or on PATH.\n  Install the agent (./install.sh) or put `maintainer` on PATH, then retry."
        );
    }

    #[test]
    fn the_action_words_are_the_ones_the_bash_passes() {
        assert_eq!(Action::Verify.to_string(), "verify");
        assert_eq!(Action::Merge.to_string(), "merge");
        assert_eq!(Action::Update.to_string(), "update");
    }

    #[test]
    fn the_fake_records_each_call_and_plays_its_scripts() {
        let helper = FakeHelper::new();
        let issue = IssueNumber::new(355).unwrap();
        helper.script_lease(Script::always(Ok(LeaseAnswer::Free)));
        helper.script_holders(Script::sequence([
            Ok(vec!["TayfurYldz".to_string()]),
            Err(HoldersFailure::Failed { output: "no profile".into() }),
        ]));
        assert_eq!(helper.lease_check(&pr()), Ok(LeaseAnswer::Free));
        assert_eq!(helper.holders(issue), Ok(vec!["TayfurYldz".to_string()]));
        assert_eq!(
            helper.holders(issue),
            Err(HoldersFailure::Failed { output: "no profile".into() })
        );
        assert_eq!(
            helper.calls(),
            vec![
                HelperCall::LeaseCheck(pr()),
                HelperCall::Holders(issue),
                HelperCall::Holders(issue)
            ]
        );
    }

    #[test]
    #[should_panic(expected = "unscripted call: no answer left for Helper::lease_check")]
    fn an_unscripted_lease_check_panics() {
        drop(FakeHelper::new().lease_check(&pr()));
    }
}
