//! What GitHub says about a pull request, and the checks that turn a fact into a witness.

use crate::{CommitId, GateDecision, PrNumber};
use mx_core::{Fact, NonEmpty};
use serde_json::Value;

/// `printf '%s' "$text" | head -c N`, then `$(...)`: the first `n` BYTES, with
/// trailing newlines gone. A cut inside a character shows as U+FFFD.
fn first_bytes(text: &str, n: usize) -> String {
    let bytes = text.as_bytes();
    let cut = &bytes[..bytes.len().min(n)];
    String::from_utf8_lossy(cut).trim_end_matches('\n').to_string()
}

/// Whether the pull request is open. `maintainer-merge` never reads it: `gh pr
/// merge` refuses a closed one. It is here because the gate's facts are the
/// facts of the pull request, and a port that serves a closed pull request must
/// be able to say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrState {
    Open,
    Closed,
    Merged,
    /// Any other text.
    Other(String),
}

impl PrState {
    /// Reads `gh`'s `state` field. Total: no text is an error.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "OPEN" => Self::Open,
            "CLOSED" => Self::Closed,
            "MERGED" => Self::Merged,
            other => Self::Other(other.to_string()),
        }
    }
}

/// `mergeStateStatus`, as GitHub spells each value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeState {
    Clean,
    HasHooks,
    Behind,
    Dirty,
    Blocked,
    Unstable,
    Unknown,
    Draft,
    /// An empty read or a value GitHub adds later.
    Other(String),
}

/// The witness that a pull request's `mergeStateStatus` is `CLEAN` or `HAS_HOOKS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateClear(());

impl MergeState {
    /// Reads `mergeStateStatus`. Total, and exact: `clean` is not `CLEAN`.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "CLEAN" => Self::Clean,
            "HAS_HOOKS" => Self::HasHooks,
            "BEHIND" => Self::Behind,
            "DIRTY" => Self::Dirty,
            "BLOCKED" => Self::Blocked,
            "UNSTABLE" => Self::Unstable,
            "UNKNOWN" => Self::Unknown,
            "DRAFT" => Self::Draft,
            other => Self::Other(other.to_string()),
        }
    }

    /// The text GitHub uses.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Clean => "CLEAN",
            Self::HasHooks => "HAS_HOOKS",
            Self::Behind => "BEHIND",
            Self::Dirty => "DIRTY",
            Self::Blocked => "BLOCKED",
            Self::Unstable => "UNSTABLE",
            Self::Unknown => "UNKNOWN",
            Self::Draft => "DRAFT",
            Self::Other(text) => text,
        }
    }

    /// bash `cmd_merge`'s `case "$state" in CLEAN|HAS_HOOKS) ;; BEHIND) ... esac`:
    /// only two states may merge, and every other one is its own refusal.
    ///
    /// BEHIND, DIRTY and BLOCKED have sentences of their own in the bash.
    /// UNSTABLE, UNKNOWN, DRAFT and an unheard-of value share the `*)` arm; this
    /// port gives UNSTABLE and UNKNOWN a variant each, so a caller can tell a state
    /// that heals ("UNKNOWN" while GitHub computes it) from one that needs a fix.
    pub fn clear(&self, pr: &PrNumber) -> Result<StateClear, GateDecision> {
        let pr = pr.clone();
        match self {
            Self::Clean | Self::HasHooks => Ok(StateClear(())),
            Self::Behind => Err(GateDecision::Behind { pr }),
            Self::Dirty => Err(GateDecision::Dirty { pr }),
            Self::Blocked => Err(GateDecision::Blocked { pr }),
            Self::Unstable => Err(GateDecision::Unstable { pr }),
            Self::Unknown => Err(GateDecision::Unknown { pr }),
            Self::Draft | Self::Other(_) => {
                Err(GateDecision::MergeStateNotAllowed { pr, state: self.as_str().to_string() })
            }
        }
    }
}

/// The witness that the head is approved: `reviewDecision` is exactly `APPROVED`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approved(());

impl Approved {
    /// bash `[ "$review" = "APPROVED" ] || die "#$pr reviewDecision is '$review', not APPROVED"`.
    /// An empty read is not approval, and neither is `approved`.
    pub fn from_decision(pr: &PrNumber, review: &str) -> Result<Self, GateDecision> {
        if review == "APPROVED" {
            Ok(Self(()))
        } else {
            Err(GateDecision::NotApproved { pr: pr.clone(), review: review.to_string() })
        }
    }
}

/// One entry of `gh pr checks --json name,bucket`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    /// `pass`, `fail`, `pending`, `skipping` or `cancel`; any other text is a
    /// bucket the gate does not accept.
    pub bucket: String,
}

/// The checks of a pull request, counted as `gh pr checks` buckets them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckBoard {
    pub pass: u64,
    /// The `skipping` bucket.
    pub skip: u64,
    pub fail: u64,
    pub pending: u64,
    pub cancel: u64,
    /// Every entry of the list, whatever its bucket.
    pub total: u64,
}

/// The witness that the board is green: nothing failed, pending or cancelled,
/// every check is pass or skipping, and at least one passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GreenBoard {
    pass: u64,
    skip: u64,
}

impl GreenBoard {
    /// Checks that passed.
    #[must_use]
    pub fn pass(self) -> u64 {
        self.pass
    }

    /// Checks that were skipped.
    #[must_use]
    pub fn skip(self) -> u64 {
        self.skip
    }
}

impl CheckBoard {
    /// Reads what `gh pr checks <pr> --json name,bucket` printed.
    ///
    /// `gh` exits 8 while a check is pending and still prints the list, so the
    /// bash ignores its status and so does this. Text that is empty after the
    /// trailing newlines go (`$(...)`) is [`GateDecision::NoChecks`]. Text that is
    /// not a JSON list of objects is [`GateDecision::ChecksUnparsable`], with the
    /// first 300 bytes. A bucket the gate has never heard of counts toward the
    /// total and toward nothing else, so [`CheckBoard::green`] refuses it.
    pub fn from_gh_json(pr: &PrNumber, stdout: &str) -> Result<Self, GateDecision> {
        let text = stdout.trim_end_matches('\n');
        if text.is_empty() {
            return Err(GateDecision::NoChecks { pr: pr.clone() });
        }
        let unparsable =
            || GateDecision::ChecksUnparsable { pr: pr.clone(), head: first_bytes(text, 300) };
        let Ok(Value::Array(items)) = serde_json::from_str::<Value>(text) else {
            return Err(unparsable());
        };
        let mut board = Self { pass: 0, skip: 0, fail: 0, pending: 0, cancel: 0, total: 0 };
        for item in &items {
            let Value::Object(entry) = item else { return Err(unparsable()) };
            match entry.get("bucket").and_then(Value::as_str) {
                Some("pass") => board.pass += 1,
                Some("skipping") => board.skip += 1,
                Some("fail") => board.fail += 1,
                Some("pending") => board.pending += 1,
                Some("cancel") => board.cancel += 1,
                _ => {}
            }
        }
        board.total = items.len() as u64;
        Ok(board)
    }

    /// The rule of `cmd_merge`, in its order: no failing, no pending, no
    /// cancelled, a board that is not empty, every check pass or skipping, and
    /// one that passed.
    ///
    /// "A skipped check made no claim either way, but a board where all of them
    /// skipped has shown nothing." Counting only fail and pending once let a
    /// cancelled check, a bucket `gh` adds next year, and an empty list all read
    /// as a green board.
    pub fn green(self, pr: &PrNumber) -> Result<GreenBoard, GateDecision> {
        let pr = pr.clone();
        if self.fail != 0 {
            return Err(GateDecision::FailingChecks { pr, count: self.fail });
        }
        if self.pending != 0 {
            return Err(GateDecision::PendingChecks { pr, count: self.pending });
        }
        if self.cancel != 0 {
            return Err(GateDecision::CancelledChecks { pr, count: self.cancel });
        }
        if self.total == 0 {
            return Err(GateDecision::EmptyBoard { pr });
        }
        if self.pass + self.skip != self.total {
            return Err(GateDecision::ChecksInUnknownBucket {
                pr,
                accepted: self.pass + self.skip,
                total: self.total,
            });
        }
        if self.pass == 0 {
            return Err(GateDecision::AllChecksSkipped { pr, total: self.total });
        }
        Ok(GreenBoard { pass: self.pass, skip: self.skip })
    }
}

/// What GitHub says about one pull request, as a plain value.
///
/// Each field is a [`Fact`]: a read that failed is `Unreadable`, a read that found
/// nothing is `Absent`, and the two never turn into each other. `maintainer-merge`
/// reads these with separate `gh pr view --json` calls, in a fixed order, and a
/// refusal stops the later calls, so the port ([`crate::GhPort`]) has one method per
/// call and the real adapter calls them in the bash order. This value is the
/// assembled picture: what a fake serves ([`crate::FakeGh::from_facts`]) and what a
/// test arranges.
///
/// `labels` are the pull request's own labels. `maintainer-merge` reads the
/// CLAIM label of the ISSUES a pull request closes ([`crate::GhPort::issue_has_label`]),
/// and no label of the pull request itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrFacts {
    pub number: PrNumber,
    pub head: Fact<CommitId>,
    pub base: Fact<String>,
    pub state: Fact<PrState>,
    pub merge_state: Fact<MergeState>,
    pub review_decision: Fact<String>,
    pub author: Fact<String>,
    pub checks: Fact<Vec<Check>>,
    pub labels: Fact<Vec<String>>,
    /// The changed paths. An empty list is not an answer, so it cannot be built
    /// (`#N: the changed file list came back empty, which is not an answer`).
    pub changed_files: Fact<NonEmpty<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> PrNumber {
        PrNumber::parse("7").unwrap()
    }

    #[test]
    fn each_merge_state_reads_as_itself_and_prints_as_github_spells_it() {
        for (text, state) in [
            ("CLEAN", MergeState::Clean),
            ("HAS_HOOKS", MergeState::HasHooks),
            ("BEHIND", MergeState::Behind),
            ("DIRTY", MergeState::Dirty),
            ("BLOCKED", MergeState::Blocked),
            ("UNSTABLE", MergeState::Unstable),
            ("UNKNOWN", MergeState::Unknown),
            ("DRAFT", MergeState::Draft),
        ] {
            assert_eq!(MergeState::parse(text), state);
            assert_eq!(state.as_str(), text);
        }
    }

    #[test]
    fn a_state_nobody_has_heard_of_keeps_its_text_and_case_matters() {
        assert_eq!(MergeState::parse(""), MergeState::Other(String::new()));
        assert_eq!(MergeState::parse("clean"), MergeState::Other("clean".into()));
        assert_eq!(MergeState::parse("CLEAN "), MergeState::Other("CLEAN ".into()));
        assert_eq!(MergeState::Other("X".into()).as_str(), "X");
    }

    #[test]
    fn clean_and_has_hooks_may_merge() {
        assert_eq!(MergeState::Clean.clear(&pr()), Ok(StateClear(())));
        assert_eq!(MergeState::HasHooks.clear(&pr()), Ok(StateClear(())));
    }

    #[test]
    fn behind_has_its_own_refusal_with_the_update_hint() {
        let refused = MergeState::Behind.clear(&pr()).unwrap_err();
        assert_eq!(refused, GateDecision::Behind { pr: pr() });
        assert!(refused.to_string().ends_with("maintainer-merge update 7"));
    }

    #[test]
    fn dirty_and_blocked_have_their_own_refusals() {
        assert_eq!(MergeState::Dirty.clear(&pr()), Err(GateDecision::Dirty { pr: pr() }));
        assert_eq!(MergeState::Blocked.clear(&pr()), Err(GateDecision::Blocked { pr: pr() }));
    }

    #[test]
    fn unstable_and_unknown_are_distinct_refusals_with_the_generic_text() {
        assert_eq!(MergeState::Unstable.clear(&pr()), Err(GateDecision::Unstable { pr: pr() }));
        assert_eq!(MergeState::Unknown.clear(&pr()), Err(GateDecision::Unknown { pr: pr() }));
        assert_ne!(
            MergeState::Unstable.clear(&pr()).unwrap_err(),
            MergeState::Unknown.clear(&pr()).unwrap_err()
        );
        assert_eq!(
            MergeState::Unstable.clear(&pr()).unwrap_err().to_string(),
            "#7 mergeStateStatus is 'UNSTABLE'; only CLEAN and HAS_HOOKS may merge"
        );
        assert_eq!(
            MergeState::Unknown.clear(&pr()).unwrap_err().to_string(),
            "#7 mergeStateStatus is 'UNKNOWN'; only CLEAN and HAS_HOOKS may merge"
        );
    }

    #[test]
    fn draft_an_empty_read_and_a_new_state_are_refused_with_their_text() {
        assert_eq!(
            MergeState::Draft.clear(&pr()),
            Err(GateDecision::MergeStateNotAllowed { pr: pr(), state: "DRAFT".into() })
        );
        assert_eq!(
            MergeState::parse("").clear(&pr()).unwrap_err().to_string(),
            "#7 mergeStateStatus is ''; only CLEAN and HAS_HOOKS may merge"
        );
        assert_eq!(
            MergeState::parse("QUEUED").clear(&pr()).unwrap_err().to_string(),
            "#7 mergeStateStatus is 'QUEUED'; only CLEAN and HAS_HOOKS may merge"
        );
    }

    #[test]
    fn the_pull_request_state_reads_as_github_spells_it() {
        assert_eq!(PrState::parse("OPEN"), PrState::Open);
        assert_eq!(PrState::parse("CLOSED"), PrState::Closed);
        assert_eq!(PrState::parse("MERGED"), PrState::Merged);
        assert_eq!(PrState::parse("open"), PrState::Other("open".into()));
    }

    #[test]
    fn only_the_exact_word_approved_is_approval() {
        assert_eq!(Approved::from_decision(&pr(), "APPROVED"), Ok(Approved(())));
        for review in ["", "approved", "APPROVED ", "REVIEW_REQUIRED", "CHANGES_REQUESTED", "null"]
        {
            assert_eq!(
                Approved::from_decision(&pr(), review),
                Err(GateDecision::NotApproved { pr: pr(), review: review.to_string() }),
                "{review:?}"
            );
        }
        assert_eq!(
            Approved::from_decision(&pr(), "").unwrap_err().to_string(),
            "#7 reviewDecision is '', not APPROVED"
        );
    }

    const MIXED: &str = r#"[
      {"name":"a","bucket":"pass"},{"name":"b","bucket":"pass"},{"name":"c","bucket":"skipping"},
      {"name":"d","bucket":"fail"},{"name":"e","bucket":"pending"},{"name":"f","bucket":"cancel"},
      {"name":"g","bucket":"weird"},{"name":"h"}
    ]"#;

    #[test]
    fn the_buckets_of_a_check_list_are_counted_and_the_total_is_the_length() {
        let board = CheckBoard::from_gh_json(&pr(), MIXED).unwrap();
        assert_eq!(
            board,
            CheckBoard { pass: 2, skip: 1, fail: 1, pending: 1, cancel: 1, total: 8 }
        );
    }

    #[test]
    fn a_list_with_no_entry_counts_zero_everywhere() {
        assert_eq!(
            CheckBoard::from_gh_json(&pr(), "[]").unwrap(),
            CheckBoard { pass: 0, skip: 0, fail: 0, pending: 0, cancel: 0, total: 0 }
        );
    }

    #[test]
    fn nothing_printed_is_no_checks_at_all() {
        for stdout in ["", "\n", "\n\n"] {
            assert_eq!(
                CheckBoard::from_gh_json(&pr(), stdout),
                Err(GateDecision::NoChecks { pr: pr() }),
                "{stdout:?}"
            );
        }
        assert_eq!(
            GateDecision::NoChecks { pr: pr() }.to_string(),
            "#7 reported no checks at all; refusing"
        );
    }

    #[test]
    fn text_that_is_not_a_json_list_of_objects_is_unparsable_and_quotes_gh() {
        for stdout in ["not json", "{\"a\":1}", "[1,2]", "[\"x\"]", "null", "\"[]\"", " "] {
            let refused = CheckBoard::from_gh_json(&pr(), stdout).unwrap_err();
            assert!(
                matches!(refused, GateDecision::ChecksUnparsable { .. }),
                "{stdout:?}: {refused:?}"
            );
        }
        assert_eq!(
            CheckBoard::from_gh_json(&pr(), "gh: HTTP 502\n").unwrap_err().to_string(),
            "could not read the check list for #7; gh returned:\ngh: HTTP 502"
        );
    }

    #[test]
    fn the_quote_of_what_gh_returned_stops_at_300_bytes() {
        let long = "x".repeat(500);
        let refused = CheckBoard::from_gh_json(&pr(), &long).unwrap_err();
        assert_eq!(refused, GateDecision::ChecksUnparsable { pr: pr(), head: "x".repeat(300) });
    }

    #[test]
    fn a_cut_in_the_middle_of_a_character_does_not_panic() {
        let long = format!("a{}", "é".repeat(200));
        let refused = CheckBoard::from_gh_json(&pr(), &long).unwrap_err();
        let GateDecision::ChecksUnparsable { head, .. } = refused else { panic!("{refused:?}") };
        assert!(head.starts_with('a'));
        assert!(head.ends_with('\u{fffd}'), "the 300th byte falls inside a character");
    }

    fn board(pass: u64, skip: u64, fail: u64, pending: u64, cancel: u64, total: u64) -> CheckBoard {
        CheckBoard { pass, skip, fail, pending, cancel, total }
    }

    #[test]
    fn a_board_of_passes_and_skips_with_one_pass_is_green() {
        let green = board(3, 1, 0, 0, 0, 4).green(&pr()).unwrap();
        assert_eq!((green.pass(), green.skip()), (3, 1));
        assert!(board(1, 0, 0, 0, 0, 1).green(&pr()).is_ok());
    }

    #[test]
    fn a_failing_check_refuses_first_and_names_the_count() {
        let refused = board(3, 0, 2, 1, 1, 7).green(&pr()).unwrap_err();
        assert_eq!(refused, GateDecision::FailingChecks { pr: pr(), count: 2 });
        assert_eq!(refused.to_string(), "#7 has 2 failing check(s)");
    }

    #[test]
    fn a_pending_check_refuses_before_a_cancelled_one() {
        let refused = board(3, 0, 0, 2, 1, 6).green(&pr()).unwrap_err();
        assert_eq!(refused, GateDecision::PendingChecks { pr: pr(), count: 2 });
        assert_eq!(refused.to_string(), "#7 has 2 pending check(s)");
    }

    #[test]
    fn a_cancelled_check_never_reported_a_verdict() {
        let refused = board(3, 0, 0, 0, 1, 4).green(&pr()).unwrap_err();
        assert_eq!(refused, GateDecision::CancelledChecks { pr: pr(), count: 1 });
    }

    #[test]
    fn an_empty_board_is_a_failure_not_a_pass() {
        assert_eq!(
            board(0, 0, 0, 0, 0, 0).green(&pr()),
            Err(GateDecision::EmptyBoard { pr: pr() })
        );
    }

    #[test]
    fn a_check_in_a_bucket_the_gate_does_not_accept_refuses_with_both_counts() {
        let refused = board(3, 1, 0, 0, 0, 6).green(&pr()).unwrap_err();
        assert_eq!(
            refused,
            GateDecision::ChecksInUnknownBucket { pr: pr(), accepted: 4, total: 6 }
        );
    }

    #[test]
    fn a_board_where_everything_skipped_has_shown_nothing() {
        let refused = board(0, 5, 0, 0, 0, 5).green(&pr()).unwrap_err();
        assert_eq!(refused, GateDecision::AllChecksSkipped { pr: pr(), total: 5 });
    }

    #[test]
    fn parsed_buckets_flow_into_the_rule() {
        let refused = CheckBoard::from_gh_json(&pr(), MIXED).unwrap().green(&pr()).unwrap_err();
        assert_eq!(refused, GateDecision::FailingChecks { pr: pr(), count: 1 });
        let ok = CheckBoard::from_gh_json(&pr(), r#"[{"name":"a","bucket":"pass"}]"#).unwrap();
        assert!(ok.green(&pr()).is_ok());
    }
}
