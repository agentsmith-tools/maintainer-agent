//! The identity pin: `gh api user` must name the profile's account.

use crate::{Account, GateDecision, PrNumber};
use mx_ports::Script;
use mx_suite::Recorder;
use std::sync::{Mutex, PoisonError};

/// What `gh` said when asked who is signed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhoAmI {
    /// `gh api user --jq .login` exited 0 and printed this (possibly empty) text.
    Login(String),
    /// It exited with this status. On a 401 `gh` prints GitHub's JSON error body
    /// on standard output, which is not a login, so a failed call never carries
    /// its output here.
    Failed { exit: i32 },
}

/// Asks GitHub who `gh` is, after making `account` the active account (decision `P30-14`).
///
/// One method, because the bash always switches first and asks second
/// (`identity`): `gh auth switch --hostname github.com --user "$ACCOUNT"` with
/// its status ignored, then `gh api user --jq .login`. A port with the two as
/// separate calls would let a caller ask without switching. The switch changes
/// the machine-wide active account of `gh`, even in a `POST=off` rehearsal
/// (listed under "Found, not fixed").
pub trait IdentityPort {
    /// The two calls, in that order.
    fn who_am_i(&self, account: &Account) -> WhoAmI;
}

/// The witness that `gh` is signed in as the profile's account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmed {
    pr: PrNumber,
    account: Account,
}

impl Confirmed {
    /// The pin of `identity`: the call must have succeeded, and the login must be
    /// the account in any case. An empty login reads as `unknown`.
    pub fn check(pr: &PrNumber, who: WhoAmI, account: &Account) -> Result<Self, GateDecision> {
        match who {
            WhoAmI::Failed { exit } => Err(GateDecision::IdentityUnconfirmed {
                account: account.as_str().to_string(),
                exit,
            }),
            WhoAmI::Login(login) if account.is(&login) => {
                Ok(Self { pr: pr.clone(), account: account.clone() })
            }
            WhoAmI::Login(login) => Err(GateDecision::IdentityMismatch {
                who: if login.is_empty() { "unknown".to_string() } else { login },
                account: account.as_str().to_string(),
            }),
        }
    }

    /// The pull request this confirmation was made for.
    #[must_use]
    pub fn pr(&self) -> &PrNumber {
        &self.pr
    }

    /// The account that was confirmed.
    #[must_use]
    pub fn account(&self) -> &Account {
        &self.account
    }
}

/// One call to a [`FakeIdentity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityCall {
    pub account: String,
}

/// A scripted [`IdentityPort`]. It records the account it was asked to switch to.
#[derive(Debug, Default)]
pub struct FakeIdentity {
    calls: Recorder<IdentityCall>,
    answers: Mutex<Script<WhoAmI>>,
}

impl FakeIdentity {
    /// A fake with no scripted answers: a call panics.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A fake that answers every call with `who`.
    #[must_use]
    pub fn always(who: WhoAmI) -> Self {
        let fake = Self::new();
        fake.script(Script::always(who));
        fake
    }

    /// Scripts the answers.
    pub fn script(&self, script: Script<WhoAmI>) {
        *self.answers.lock().unwrap_or_else(PoisonError::into_inner) = script;
    }

    /// Every call so far.
    #[must_use]
    pub fn calls(&self) -> Vec<IdentityCall> {
        self.calls.entries()
    }
}

impl IdentityPort for FakeIdentity {
    fn who_am_i(&self, account: &Account) -> WhoAmI {
        self.calls.push(IdentityCall { account: account.as_str().to_string() });
        self.answers.lock().unwrap_or_else(PoisonError::into_inner).next("IdentityPort::who_am_i")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> Account {
        Account::new("vladimirrott").unwrap()
    }

    #[test]
    fn the_account_itself_is_confirmed() {
        let confirmed = Confirmed::check(
            &PrNumber::parse("7").unwrap(),
            WhoAmI::Login("vladimirrott".into()),
            &account(),
        )
        .unwrap();
        assert_eq!(confirmed.account().as_str(), "vladimirrott");
    }

    #[test]
    fn the_same_login_in_another_case_is_the_same_account() {
        // bash: [ "${who,,}" = "${ACCOUNT,,}" ]
        assert!(
            Confirmed::check(
                &PrNumber::parse("7").unwrap(),
                WhoAmI::Login("VladimirRott".into()),
                &account()
            )
            .is_ok()
        );
    }

    #[test]
    fn another_account_is_refused_with_both_names() {
        let refused = Confirmed::check(
            &PrNumber::parse("7").unwrap(),
            WhoAmI::Login("someone-else".into()),
            &account(),
        )
        .unwrap_err();
        assert_eq!(
            refused,
            GateDecision::IdentityMismatch {
                who: "someone-else".into(),
                account: "vladimirrott".into()
            }
        );
        // bash: die "gh is authenticated as '${who:-unknown}', not $ACCOUNT"
        assert_eq!(refused.to_string(), "gh is authenticated as 'someone-else', not vladimirrott");
    }

    #[test]
    fn an_empty_login_reads_as_unknown() {
        let refused = Confirmed::check(
            &PrNumber::parse("7").unwrap(),
            WhoAmI::Login(String::new()),
            &account(),
        )
        .unwrap_err();
        assert_eq!(refused.to_string(), "gh is authenticated as 'unknown', not vladimirrott");
    }

    #[test]
    fn a_failed_call_is_not_a_login_and_names_the_exit_status() {
        let refused = Confirmed::check(
            &PrNumber::parse("7").unwrap(),
            WhoAmI::Failed { exit: 1 },
            &account(),
        )
        .unwrap_err();
        assert_eq!(
            refused,
            GateDecision::IdentityUnconfirmed { account: "vladimirrott".into(), exit: 1 }
        );
        // bash: die "gh could not confirm an account (gh exited $rc); if the token was rejected, run gh auth login -h github.com as $ACCOUNT"
        assert_eq!(
            refused.to_string(),
            "gh could not confirm an account (gh exited 1); if the token was rejected, run gh auth login -h github.com as vladimirrott"
        );
    }

    #[test]
    fn a_failed_call_is_refused_even_when_the_account_is_the_one_asked_for() {
        assert!(
            Confirmed::check(
                &PrNumber::parse("7").unwrap(),
                WhoAmI::Failed { exit: 4 },
                &account()
            )
            .is_err()
        );
    }

    #[test]
    fn the_fake_records_the_account_it_was_asked_to_switch_to() {
        let fake = FakeIdentity::always(WhoAmI::Login("vladimirrott".into()));
        assert_eq!(fake.who_am_i(&account()), WhoAmI::Login("vladimirrott".into()));
        assert_eq!(fake.calls(), vec![IdentityCall { account: "vladimirrott".into() }]);
    }

    #[test]
    fn the_fake_plays_a_sequence_in_order() {
        let fake = FakeIdentity::new();
        fake.script(Script::sequence([WhoAmI::Failed { exit: 1 }, WhoAmI::Login("x".into())]));
        assert_eq!(fake.who_am_i(&account()), WhoAmI::Failed { exit: 1 });
        assert_eq!(fake.who_am_i(&account()), WhoAmI::Login("x".into()));
    }

    #[test]
    #[should_panic(expected = "unscripted call: no answer left for IdentityPort::who_am_i")]
    fn an_unscripted_call_panics() {
        drop(FakeIdentity::new().who_am_i(&account()));
    }
}
