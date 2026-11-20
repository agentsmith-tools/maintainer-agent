//! The GitHub identity pin of `lib/run.sh`, as pure decisions and message text.
//!
//! `gh`'s active account has been observed flipping to a different account on
//! its own. Unattended, that posts reviews and comments under the wrong name, so
//! `run.sh` asks GitHub who it is before anything else and refuses the wrong
//! answer. The questions are effects (the lifecycle calls `gh`); what to make of
//! each answer is decided here.
//!
//! # Three answers, three reactions
//!
//! An outage is transient, so it is retried. A wrong account is not, so the first
//! answer refuses it: retrying would wait for a different identity to show up,
//! which is the one thing the gate exists to prevent. A rejected token (HTTP
//! 401) needs a re-login, so it also refuses at once. Only a zero exit makes
//! standard output a login: `gh api user` prints GitHub's JSON error body on a
//! 401 and exits 1, and a check that read that body as an account named
//! `{ "message": "Bad credentials", ... }` in an alert.

use mx_profile::scrub::MIN_SECRET_CHARS;

/// What one `gh api user --jq .login` call told the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// Exit 0 and a login on standard output.
    Login(String),
    /// Exit 0 with nothing printed, or a failure that is not a 401: the run
    /// could not ask.
    NoAnswer,
    /// A failure whose standard error says `HTTP 401`. `body` is what `gh`
    /// printed on standard output, which the log keeps.
    Unauthorized { body: String },
}

/// Reads one answer. `rc` is the exit code of `gh`, `stdout` what it printed with
/// trailing newlines removed (`$(...)`), `stderr` what it printed to standard error.
#[must_use]
pub fn classify_answer(rc: i32, stdout: &str, stderr: &[u8]) -> Answer {
    if rc != 0 {
        if contains(stderr, b"HTTP 401") {
            return Answer::Unauthorized { body: stdout.to_string() };
        }
        return Answer::NoAnswer;
    }
    if stdout.is_empty() { Answer::NoAnswer } else { Answer::Login(stdout.to_string()) }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

/// What to do after an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// A login arrived: compare it with the account.
    Accept(String),
    /// Ask again after the backoff.
    Retry,
    /// Out of tries: the run could not ask, exit 75.
    GiveUp,
    /// The token was rejected: exit 1 at once.
    Refuse401,
}

/// Decides after attempt number `attempt` (counting from 1) of `tries`.
#[must_use]
pub fn decide_attempt(answer: &Answer, attempt: i64, tries: i64) -> Next {
    match answer {
        Answer::Login(login) => Next::Accept(login.clone()),
        Answer::Unauthorized { .. } => Next::Refuse401,
        Answer::NoAnswer if attempt >= tries => Next::GiveUp,
        Answer::NoAnswer => Next::Retry,
    }
}

/// How long to sleep after attempt number `attempt`: `attempt * backoff` seconds,
/// or `None` when the backoff is not positive.
#[must_use]
pub fn backoff_seconds(attempt: i64, backoff: i64) -> Option<i64> {
    (backoff > 0).then(|| attempt.saturating_mul(backoff))
}

/// GitHub logins are case-insensitive, and the profile's account may be spelled
/// in any case.
#[must_use]
pub fn same_login(login: &str, account: &str) -> bool {
    login.eq_ignore_ascii_case(account)
}

/// What to make of the second `gh api user`, which runs with the pinned token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinnedVerdict {
    /// The token belongs to the account.
    Confirmed,
    /// `gh` failed or answered nothing: nobody confirmed the identity. Exit 75.
    Unresolved { rc: i32 },
    /// The token belongs to somebody else. Exit 1.
    WrongAccount { login: String },
}

/// Judges the pinned token. An empty answer with exit 0 is unresolved: the log
/// once recorded a verification that had not happened because `|| true` turned
/// "could not resolve" into "resolved to nothing".
#[must_use]
pub fn judge_pinned(rc: i32, login: &str, account: &str) -> PinnedVerdict {
    if rc != 0 || login.is_empty() {
        PinnedVerdict::Unresolved { rc }
    } else if same_login(login, account) {
        PinnedVerdict::Confirmed
    } else {
        PinnedVerdict::WrongAccount { login: login.to_string() }
    }
}

/// Decision D15: the exit scrub refuses a secret under 16 characters, because a
/// short needle rewrites every file it walks, so a token that short can never be
/// removed from what the run leaves behind. The bash ran on without noticing.
#[must_use]
pub fn token_can_be_scrubbed(token: &str) -> bool {
    token.chars().count() >= MIN_SECRET_CHARS
}

/// The alert for a login that is not the account.
#[must_use]
pub fn wrong_account_message(login: &str, account: &str, log: &str) -> String {
    format!("gh is authenticated as '{login}', not {account}; refusing to run. See {log}")
}

/// The alert for HTTP 401.
#[must_use]
pub fn rejected_token_message(account: &str, log: &str) -> String {
    format!(
        "GitHub rejected the stored token for {account} (HTTP 401), so nothing ran. Log in again with: gh auth login -h github.com (as {account}). See {log}"
    )
}

/// The transient alert for a run that never got an answer.
#[must_use]
pub fn unreachable_message(tries: i64, log: &str) -> String {
    format!(
        "gh could not reach GitHub in {tries} tries, so the identity was never verified and nothing ran. Check the network, not the token. See {log}"
    )
}

/// The alert for an account with no token to pin.
#[must_use]
pub fn no_token_message(account: &str, log: &str) -> String {
    format!(
        "could not pin a token for {account}, so this run's identity is not guaranteed for the whole pass; refusing rather than risk posting as another account. See {log}"
    )
}

/// The alert for a pinned token that the exit scrub could not remove (decision D15).
#[must_use]
pub fn short_token_message(account: &str, log: &str) -> String {
    format!(
        "the token pinned for {account} is shorter than {MIN_SECRET_CHARS} characters, so the exit scrub could not remove it from what this run leaves behind; refusing to run. See {log}"
    )
}

/// The transient alert for a pinned token that `gh` could not resolve.
#[must_use]
pub fn pinned_unresolved_message(rc: i32, log: &str) -> String {
    format!(
        "the pinned token could not be resolved to an account (gh exited {rc}), so this run's identity was never confirmed and nothing ran. See {log}"
    )
}

/// The alert for a pinned token that belongs to another account.
#[must_use]
pub fn pinned_wrong_account_message(login: &str, account: &str, log: &str) -> String {
    format!("the pinned token resolves to '{login}', not {account}; refusing to run. See {log}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_zero_with_a_login_is_a_login() {
        assert_eq!(classify_answer(0, "fxacct", b""), Answer::Login("fxacct".to_string()));
    }

    #[test]
    fn exit_zero_with_nothing_is_no_answer() {
        assert_eq!(classify_answer(0, "", b""), Answer::NoAnswer);
    }

    #[test]
    fn a_failure_without_http_401_is_no_answer_whatever_it_printed() {
        assert_eq!(
            classify_answer(1, "{\"message\": \"Bad credentials\"}", b"error connecting"),
            Answer::NoAnswer
        );
    }

    #[test]
    fn a_failure_with_http_401_is_a_rejection_and_keeps_the_body() {
        let body = "{\"message\": \"Bad credentials\"}";
        assert_eq!(
            classify_answer(1, body, b"gh: Bad credentials (HTTP 401)\n"),
            Answer::Unauthorized { body: body.to_string() }
        );
    }

    #[test]
    fn an_http_401_on_a_zero_exit_is_not_a_rejection() {
        assert_eq!(
            classify_answer(0, "fxacct", b"HTTP 401 in a warning"),
            Answer::Login("fxacct".to_string())
        );
    }

    #[test]
    fn no_answer_retries_until_the_last_try_and_then_gives_up() {
        assert_eq!(decide_attempt(&Answer::NoAnswer, 1, 3), Next::Retry);
        assert_eq!(decide_attempt(&Answer::NoAnswer, 2, 3), Next::Retry);
        assert_eq!(decide_attempt(&Answer::NoAnswer, 3, 3), Next::GiveUp);
        assert_eq!(decide_attempt(&Answer::NoAnswer, 1, 1), Next::GiveUp);
    }

    #[test]
    fn a_rejection_never_retries() {
        let answer = Answer::Unauthorized { body: String::new() };
        assert_eq!(decide_attempt(&answer, 1, 3), Next::Refuse401);
    }

    #[test]
    fn a_login_is_accepted_on_the_first_try_even_with_one_try_left() {
        assert_eq!(
            decide_attempt(&Answer::Login("x".to_string()), 3, 3),
            Next::Accept("x".to_string())
        );
    }

    #[test]
    fn the_backoff_grows_with_the_attempt_and_zero_means_no_sleep() {
        assert_eq!(backoff_seconds(1, 7), Some(7));
        assert_eq!(backoff_seconds(2, 7), Some(14));
        assert_eq!(backoff_seconds(1, 0), None);
        assert_eq!(backoff_seconds(1, -5), None);
    }

    #[test]
    fn logins_compare_without_regard_to_case_on_both_sides() {
        assert!(same_login("FXAcct", "fxacct"));
        assert!(same_login("fxacct", "FxAcct"));
        assert!(!same_login("fxacct2", "fxacct"));
        assert!(!same_login("", "fxacct"));
    }

    #[test]
    fn a_pinned_token_is_confirmed_only_for_the_account() {
        assert_eq!(judge_pinned(0, "fxacct", "fxacct"), PinnedVerdict::Confirmed);
        assert_eq!(judge_pinned(0, "FXACCT", "fxacct"), PinnedVerdict::Confirmed);
        assert_eq!(
            judge_pinned(0, "someone-else", "fxacct"),
            PinnedVerdict::WrongAccount { login: "someone-else".to_string() }
        );
    }

    #[test]
    fn a_pinned_token_that_gh_could_not_resolve_is_unresolved_not_wrong() {
        assert_eq!(judge_pinned(3, "", "fxacct"), PinnedVerdict::Unresolved { rc: 3 });
        assert_eq!(judge_pinned(0, "", "fxacct"), PinnedVerdict::Unresolved { rc: 0 });
        assert_eq!(judge_pinned(1, "fxacct", "fxacct"), PinnedVerdict::Unresolved { rc: 1 });
    }

    // Plan decision D15.
    #[test]
    fn a_token_under_16_characters_cannot_be_scrubbed_plan_decision_d15() {
        assert!(!token_can_be_scrubbed("shorttoken"));
        assert!(!token_can_be_scrubbed("123456789012345"));
        assert!(token_can_be_scrubbed("1234567890123456"));
        assert!(!token_can_be_scrubbed(""));
    }

    #[test]
    fn the_alert_texts_match_the_bash() {
        assert_eq!(
            wrong_account_message("someone-else", "fxacct", "/l"),
            "gh is authenticated as 'someone-else', not fxacct; refusing to run. See /l"
        );
        assert_eq!(
            rejected_token_message("fxacct", "/l"),
            "GitHub rejected the stored token for fxacct (HTTP 401), so nothing ran. Log in again with: gh auth login -h github.com (as fxacct). See /l"
        );
        assert_eq!(
            unreachable_message(3, "/l"),
            "gh could not reach GitHub in 3 tries, so the identity was never verified and nothing ran. Check the network, not the token. See /l"
        );
        assert_eq!(
            no_token_message("fxacct", "/l"),
            "could not pin a token for fxacct, so this run's identity is not guaranteed for the whole pass; refusing rather than risk posting as another account. See /l"
        );
        assert_eq!(
            pinned_unresolved_message(3, "/l"),
            "the pinned token could not be resolved to an account (gh exited 3), so this run's identity was never confirmed and nothing ran. See /l"
        );
        assert_eq!(
            pinned_wrong_account_message("o", "fxacct", "/l"),
            "the pinned token resolves to 'o', not fxacct; refusing to run. See /l"
        );
    }
}
