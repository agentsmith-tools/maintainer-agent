//! The identifiers the gate takes from the command line and from GitHub, checked once.

use crate::GateDecision;
use mx_ports::Fs;
use std::fmt;
use std::num::NonZeroU64;
use std::path::Path;
use thiserror::Error;

/// A pull request number, as the command line typed it.
///
/// Plan decision F2, and the whole class: `show 7`, `show ../x` and every other
/// subcommand build `"$RECEIPTS/$pr.json"` from this argument, and the bash
/// validated it in `merge`, `verify` and `receipt` (`pr_or_die`) and not in
/// `show`, so `show ../../etc/passwd` read outside the receipt directory. In
/// the Rust gate no function that reaches a receipt path, a `gh` argument or a
/// `git` ref takes a string: it takes a `PrNumber`, and the only way to one is
/// [`PrNumber::parse`].
///
/// The type keeps the text the user typed ([`PrNumber::as_arg`]) next to its value
/// ([`PrNumber::value`]), because the receipt's file name is the text: `007`
/// names `007.json`, and `receipt_check` compares the receipt's `pr` with that
/// text. Two numbers are equal when their texts are.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrNumber {
    text: String,
    value: NonZeroU64,
}

impl PrNumber {
    /// Checks a pull request argument: ASCII digits only, at least one, a value
    /// above zero that fits in 64 bits.
    ///
    /// The bash (`case "$1" in ''|*[!0-9]*)`) refuses an empty argument and any
    /// character but a digit, and says "positive" while accepting `0`. This
    /// refuses `0` too, and a number of 20 digits or more that overflows
    /// (decision P30-1): `gh` never hands out either.
    pub fn parse(arg: &str) -> Result<Self, GateDecision> {
        let refuse = || GateDecision::InvalidPrNumber { given: arg.to_string() };
        if arg.is_empty() || !arg.bytes().all(|b| b.is_ascii_digit()) {
            return Err(refuse());
        }
        let value = arg.parse::<u64>().ok().and_then(NonZeroU64::new).ok_or_else(refuse)?;
        Ok(Self { text: arg.to_string(), value })
    }

    /// The argument as typed, for file names and messages.
    #[must_use]
    pub fn as_arg(&self) -> &str {
        &self.text
    }

    /// The number, for `gh` arguments and the receipt's `pr` field.
    #[must_use]
    pub fn value(&self) -> u64 {
        self.value.get()
    }
}

/// `show <pr>`: the bytes of one receipt. The number is a [`PrNumber`], so the text `show` builds a
/// path from has passed [`PrNumber::parse`] before `mx_receipt::show_one` checks it again (decision
/// P30-23: `show_one` takes text and has its own digits-only check, which accepts `0` and any
/// length, so F2 is a runtime check there and the type here).
pub fn show_receipt(fs: &dyn Fs, dir: &Path, pr: &PrNumber) -> Result<Vec<u8>, GateDecision> {
    mx_receipt::show_one(fs, dir, pr.as_arg())
        .map_err(|_| GateDecision::NoReceipt { pr: pr.clone() })
}

impl fmt::Display for PrNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// An issue a pull request closes, as `closingIssuesReferences` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IssueNumber(NonZeroU64);

impl IssueNumber {
    /// An issue number, or `None` for zero.
    #[must_use]
    pub fn new(number: u64) -> Option<Self> {
        NonZeroU64::new(number).map(Self)
    }

    /// The number.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for IssueNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Text that is not a commit id.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("'{given}' is not a commit id")]
pub struct NotACommitId {
    pub given: String,
}

/// A commit id as GitHub reports it: lower-case hexadecimal.
///
/// `gh pr merge --match-head-commit` is the fence of the whole gate: the merge
/// lands only if the pull request's head is still the one that was checked. A
/// read that failed left the bash gate with an empty pin, and the merge went
/// ahead with no fence at all (the comment in `cmd_merge`). A `CommitId` is never
/// empty, and [`crate::GhPort::merge`] takes one and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitId(String);

fn is_lower_hex(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl CommitId {
    /// The rule of `merge`: one or more lower-case hexadecimal digits
    /// (`case "$head" in ''|*[!0-9a-f]*)`). Upper case is refused, as in the bash.
    pub fn parse(text: &str) -> Result<Self, NotACommitId> {
        if is_lower_hex(text) {
            Ok(Self(text.to_string()))
        } else {
            Err(NotACommitId { given: text.to_string() })
        }
    }

    /// The rule of `update`: exactly 40 lower-case hexadecimal digits
    /// (`[[ "$head" =~ ^[0-9a-f]{40}$ ]]`), the form GitHub's API accepts as
    /// `expected_head_sha`.
    pub fn parse_full(text: &str) -> Result<Self, NotACommitId> {
        if text.len() == 40 && is_lower_hex(text) {
            Ok(Self(text.to_string()))
        } else {
            Err(NotACommitId { given: text.to_string() })
        }
    }

    /// The whole id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first eight characters, `${head:0:8}`, as messages show a head.
    #[must_use]
    pub fn short(&self) -> &str {
        &self.0[..self.0.len().min(8)]
    }
}

impl fmt::Display for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The head argument of `verify`, `verify-part`, `verify-deps` and `receipt`:
/// at least seven characters. It is a prefix of the real head, or for `receipt`
/// any text of that length, which the bash records as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadArg(String);

impl HeadArg {
    /// bash `[ ${#sha} -ge 7 ] || die "head sha looks truncated: $sha"`. The
    /// length is in characters, as `${#sha}` counts them in a UTF-8 locale.
    pub fn parse(text: &str) -> Result<Self, GateDecision> {
        if text.chars().count() < 7 {
            return Err(GateDecision::HeadTruncated { sha: text.to_string() });
        }
        Ok(Self(text.to_string()))
    }

    /// The argument as typed.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `head` starts with this argument: `case "$real_head" in "$sha"*)`.
    #[must_use]
    pub fn names(&self, head: &CommitId) -> bool {
        head.as_str().starts_with(&self.0)
    }
}

/// `MAINTAINER_ACCOUNT` was empty. The bash stops at start up
/// (`: "${MAINTAINER_ACCOUNT:?}"`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("MAINTAINER_ACCOUNT is empty")]
pub struct NoAccount;

/// The GitHub account the profile names. Logins are case-insensitive, and no two
/// accounts differ only in case, so two spellings of one login are one account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account(String);

impl Account {
    /// The account, as the profile spells it.
    pub fn new(login: &str) -> Result<Self, NoAccount> {
        if login.is_empty() { Err(NoAccount) } else { Ok(Self(login.to_string())) }
    }

    /// The login as spelled.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// bash `[ "${who,,}" = "${ACCOUNT,,}" ]`: the same login in any case.
    #[must_use]
    pub fn is(&self, login: &str) -> bool {
        self.0.eq_ignore_ascii_case(login)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_request_number_keeps_its_text_and_its_value() {
        let pr = PrNumber::parse("7").unwrap();
        assert_eq!((pr.as_arg(), pr.value(), pr.to_string().as_str()), ("7", 7, "7"));
        assert_eq!(PrNumber::parse("40213").unwrap().value(), 40213);
    }

    #[test]
    fn leading_zeros_keep_their_text_because_the_receipt_file_is_named_by_it() {
        let pr = PrNumber::parse("007").unwrap();
        assert_eq!(pr.as_arg(), "007");
        assert_eq!(pr.value(), 7);
        assert_ne!(pr, PrNumber::parse("7").unwrap());
    }

    #[test]
    fn a_path_a_sign_a_space_or_a_word_is_not_a_pull_request_number() {
        // F2: `show ../x` read outside the receipt directory.
        for given in [
            "../x",
            "../../etc/passwd",
            "7/../8",
            "-1",
            "+1",
            " 7",
            "7 ",
            "7\n",
            "1e3",
            "x",
            "٣",
            "0x10",
            "",
        ] {
            assert_eq!(
                PrNumber::parse(given),
                Err(GateDecision::InvalidPrNumber { given: given.to_string() }),
                "{given:?}"
            );
        }
    }

    #[test]
    fn zero_is_not_a_pull_request_number() {
        // P30-1: the bash says "positive" and accepts 0.
        for given in ["0", "00", "000"] {
            assert_eq!(
                PrNumber::parse(given),
                Err(GateDecision::InvalidPrNumber { given: given.to_string() })
            );
        }
    }

    #[test]
    fn a_number_that_does_not_fit_in_64_bits_is_refused() {
        assert!(PrNumber::parse("18446744073709551615").is_ok());
        assert_eq!(
            PrNumber::parse("18446744073709551616"),
            Err(GateDecision::InvalidPrNumber { given: "18446744073709551616".into() })
        );
    }

    #[test]
    fn the_refusal_reads_as_pr_or_die() {
        // bash: die "pull request must be a positive integer, not '$1'"
        assert_eq!(
            PrNumber::parse("../x").unwrap_err().to_string(),
            "pull request must be a positive integer, not '../x'"
        );
    }

    #[test]
    fn show_reads_one_receipt_by_a_checked_number() {
        use mx_ports::{EffectLog, FakeFs};
        let log = EffectLog::new();
        let fs = FakeFs::new(&log);
        fs.put("/s/receipts/7.json", b"{}".to_vec(), 0);
        let seven = PrNumber::parse("7").unwrap();
        assert_eq!(show_receipt(&fs, Path::new("/s/receipts"), &seven).unwrap(), b"{}");
        let eight = PrNumber::parse("8").unwrap();
        assert_eq!(
            show_receipt(&fs, Path::new("/s/receipts"), &eight),
            Err(GateDecision::NoReceipt { pr: eight })
        );
    }

    #[test]
    fn an_issue_number_is_above_zero() {
        assert_eq!(IssueNumber::new(355).unwrap().get(), 355);
        assert_eq!(IssueNumber::new(355).unwrap().to_string(), "355");
        assert_eq!(IssueNumber::new(0), None);
    }

    #[test]
    fn a_commit_id_is_lower_case_hex_of_any_length_as_merge_checks_it() {
        for text in ["a", "abc123", "0123456789abcdef0123456789abcdef01234567"] {
            assert_eq!(CommitId::parse(text).unwrap().as_str(), text);
        }
    }

    #[test]
    fn text_that_is_not_lower_case_hex_is_not_a_commit_id() {
        // bash: case "$head" in ''|*[!0-9a-f]*) die "... is not a commit id ..."
        for text in ["", "NOT-A-COMMIT", "ABCDEF", "abc ", "abcg", "abc\n", "0x1f", "-1"] {
            assert_eq!(
                CommitId::parse(text),
                Err(NotACommitId { given: text.to_string() }),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_full_commit_id_is_exactly_40_lower_case_hex_digits() {
        let forty = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(CommitId::parse_full(forty).unwrap().as_str(), forty);
        for text in [
            &forty[..39],
            &format!("{forty}0"),
            "",
            "0123456789ABCDEF0123456789abcdef01234567",
            "g123456789abcdef0123456789abcdef01234567",
        ] {
            assert!(CommitId::parse_full(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn a_head_is_shown_as_its_first_eight_characters() {
        let long = CommitId::parse("0123456789abcdef0123456789abcdef01234567").unwrap();
        assert_eq!(long.short(), "01234567");
        assert_eq!(CommitId::parse("abc").unwrap().short(), "abc");
        assert_eq!(long.to_string(), long.as_str());
    }

    #[test]
    fn a_head_argument_needs_seven_characters() {
        assert_eq!(HeadArg::parse("abcdef1").unwrap().as_str(), "abcdef1");
        assert!(HeadArg::parse("abcdef12345").is_ok());
        for text in ["", "a", "abcdef"] {
            assert_eq!(
                HeadArg::parse(text),
                Err(GateDecision::HeadTruncated { sha: text.to_string() }),
                "{text:?}"
            );
        }
    }

    #[test]
    fn seven_multibyte_characters_count_as_seven() {
        // `${#sha}` counts characters in a UTF-8 locale.
        assert!(HeadArg::parse("ééééééé").is_ok());
        assert!(HeadArg::parse("éééééé").is_err());
    }

    #[test]
    fn a_head_argument_names_the_heads_it_is_a_prefix_of() {
        let head = CommitId::parse("0123456789abcdef0123456789abcdef01234567").unwrap();
        assert!(HeadArg::parse("0123456").unwrap().names(&head));
        assert!(HeadArg::parse(head.as_str()).unwrap().names(&head));
        assert!(!HeadArg::parse("1234567").unwrap().names(&head));
    }

    #[test]
    fn the_truncated_head_refusal_reads_as_the_bash_die() {
        // bash: die "head sha looks truncated: $sha"
        assert_eq!(HeadArg::parse("abc").unwrap_err().to_string(), "head sha looks truncated: abc");
    }

    #[test]
    fn an_account_is_the_profiles_login_in_any_case() {
        let account = Account::new("VladimirRott").unwrap();
        assert_eq!(account.as_str(), "VladimirRott");
        assert!(account.is("vladimirrott"));
        assert!(account.is("VLADIMIRROTT"));
        assert!(!account.is("vladimirrot"));
        assert!(!account.is(""));
    }

    #[test]
    fn an_empty_account_is_refused() {
        assert_eq!(Account::new(""), Err(NoAccount));
    }
}
