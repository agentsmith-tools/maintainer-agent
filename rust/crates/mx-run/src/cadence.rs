//! The cadence gate of `lib/run.sh` (`min_gate`), as a pure decision.
//!
//! The scheduler is not the only thing that decides how often a task runs, so
//! `run.sh` reads the mtime of `state/last-<task>.json` twice: once at the start
//! and once after it holds the lock, because a run that queued behind another
//! run decides on a stamp the run ahead of it is about to replace. Both reads use
//! [`decide`].
//!
//! # What the bash does, kept
//!
//! - `MAINTAINER_FORCE=1` (exactly `1`) turns the gate off.
//! - A minimum that is unset, empty or the string `0` turns it off.
//! - A task with no stamp has never run, so it runs.
//! - The age is `(now - mtime) / 3600` in whole hours, truncated toward zero.
//! - The task is skipped when `age < minimum`, and the skip line says so.
//! - A stamp whose mtime cannot be read counts as mtime 0, which is a very old
//!   stamp.
//!
//! # What differs, on purpose
//!
//! - D11: a minimum that is not a whole number refuses. The bash `[` fails on
//!   it, prints a complaint and carries on, so the gate disappears.
//! - D12: a stamp from the future does not skip the run. In the bash its age is
//!   negative, which is below any positive minimum, so one stamp written by a
//!   clock that ran ahead blocked a task until the clock caught up.

/// Seconds in the hour that ages are floored to.
pub const SECONDS_PER_HOUR: i64 = 3600;

/// What is known about `state/last-<task>.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stamp {
    /// There is no such file: the task has never finished.
    Missing,
    /// The file exists and was last written at this epoch second. A file whose
    /// mtime could not be read is `ModifiedAt(0)`, as `stat ... || echo 0` makes it.
    ModifiedAt(i64),
}

/// Everything the gate reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CadenceFacts {
    /// `MAINTAINER_FORCE` was exactly `1` when the run started.
    pub forced: bool,
    /// `${MIN_HOURS_<task>:-0}`: the text of the minimum, `0` when unset or empty.
    pub min_hours: String,
    /// The stamp file.
    pub stamp: Stamp,
}

/// What the gate decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Go ahead.
    Run,
    /// Too soon: print the skip line and exit 0.
    Skip { age_hours: i64, min_hours: i64 },
    /// The minimum is not a number (decision D11): refuse and exit 64.
    Refuse { min_hours: String },
}

/// `[ "$min" -lt ... ]`: an integer with optional blanks and sign, as `[` reads
/// one. `None` for anything else, including a number that overflows 64 bits.
#[must_use]
pub fn parse_hours(text: &str) -> Option<i64> {
    let trimmed = text.trim_matches([' ', '\t']);
    let (negative, digits) = match trimmed.as_bytes().first() {
        Some(b'-') => (true, &trimmed[1..]),
        Some(b'+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let magnitude: i64 = digits.parse().ok()?;
    Some(if negative { -magnitude } else { magnitude })
}

/// Decides whether the task may run now. `now` is the epoch second.
#[must_use]
pub fn decide(facts: &CadenceFacts, now: i64) -> Decision {
    if facts.forced {
        return Decision::Run;
    }
    // A string comparison in the bash (`[ "$min" = 0 ]`), kept: `00` is a number.
    if facts.min_hours == "0" {
        return Decision::Run;
    }
    let Some(min_hours) = parse_hours(&facts.min_hours) else {
        return Decision::Refuse { min_hours: facts.min_hours.clone() };
    };
    let Stamp::ModifiedAt(last) = facts.stamp else {
        return Decision::Run;
    };
    if last > now {
        return Decision::Run;
    }
    let age_hours = (now - last) / SECONDS_PER_HOUR;
    if age_hours < min_hours { Decision::Skip { age_hours, min_hours } } else { Decision::Run }
}

/// The line `min_gate` prints when it skips, with its newline. `suffix` says
/// where the call happened (`after waiting for the lock`), or is `None`.
#[must_use]
pub fn skip_line(task: &str, age_hours: i64, min_hours: i64, suffix: Option<&str>) -> String {
    let place = suffix.map_or_else(String::new, |text| format!(", {text}"));
    format!(
        "skipped: {task} last ran {age_hours}h ago, minimum is {min_hours}h{place} (MAINTAINER_FORCE=1 overrides)\n"
    )
}

/// The refusal line for a minimum that is not a number (decision D11).
#[must_use]
pub fn refusal_line(task: &str, min_hours: &str) -> String {
    format!(
        "run.sh: MIN_HOURS_{task}='{min_hours}' is not a whole number of hours, so the cadence gate cannot decide; refusing to run\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_767_225_600;

    fn facts(min: &str, stamp: Stamp) -> CadenceFacts {
        CadenceFacts { forced: false, min_hours: min.to_string(), stamp }
    }

    fn aged(seconds: i64) -> Stamp {
        Stamp::ModifiedAt(NOW - seconds)
    }

    #[test]
    fn a_stamp_younger_than_the_minimum_skips_and_names_the_floored_age() {
        assert_eq!(
            decide(&facts("6", aged(3600)), NOW),
            Decision::Skip { age_hours: 1, min_hours: 6 }
        );
        assert_eq!(
            decide(&facts("6", aged(21_599)), NOW),
            Decision::Skip { age_hours: 5, min_hours: 6 }
        );
    }

    #[test]
    fn a_stamp_exactly_as_old_as_the_minimum_runs() {
        assert_eq!(decide(&facts("6", aged(21_600)), NOW), Decision::Run);
        assert_eq!(decide(&facts("6", aged(21_600 + 3600)), NOW), Decision::Run);
    }

    #[test]
    fn the_boundary_is_one_hour_below_the_minimum() {
        assert_eq!(
            decide(&facts("6", aged(5 * 3600)), NOW),
            Decision::Skip { age_hours: 5, min_hours: 6 }
        );
        assert_eq!(decide(&facts("6", aged(6 * 3600)), NOW), Decision::Run);
    }

    #[test]
    fn force_turns_the_gate_off_even_for_a_fresh_stamp() {
        let mut forced = facts("6", aged(60));
        forced.forced = true;
        assert_eq!(decide(&forced, NOW), Decision::Run);
    }

    #[test]
    fn force_also_passes_a_minimum_that_is_not_a_number() {
        let mut forced = facts("soon", aged(60));
        forced.forced = true;
        assert_eq!(decide(&forced, NOW), Decision::Run);
    }

    #[test]
    fn a_minimum_of_zero_turns_the_gate_off() {
        assert_eq!(decide(&facts("0", aged(60)), NOW), Decision::Run);
    }

    #[test]
    fn a_task_that_never_ran_runs_whatever_the_minimum() {
        assert_eq!(decide(&facts("999999999", Stamp::Missing), NOW), Decision::Run);
    }

    #[test]
    fn a_stamp_whose_mtime_is_unreadable_counts_as_mtime_zero() {
        let decision = decide(&facts("6", Stamp::ModifiedAt(0)), NOW);
        assert_eq!(decision, Decision::Run);
        let huge = decide(&facts("999999999", Stamp::ModifiedAt(0)), NOW);
        assert!(matches!(huge, Decision::Skip { .. }), "{huge:?}");
    }

    // Plan decision D11: the bash falls open here, the port refuses.
    #[test]
    fn a_minimum_that_is_not_a_number_refuses_plan_decision_d11() {
        for text in ["soon", "6h", "1.5", "0x10", "--6", "+", "-", "99999999999999999999"] {
            assert_eq!(
                decide(&facts(text, aged(3600)), NOW),
                Decision::Refuse { min_hours: text.to_string() },
                "{text}"
            );
        }
    }

    #[test]
    fn a_minimum_that_is_not_a_number_refuses_even_with_no_stamp() {
        assert_eq!(
            decide(&facts("soon", Stamp::Missing), NOW),
            Decision::Refuse { min_hours: "soon".to_string() }
        );
    }

    // Plan decision D12: the bash skips a stamp from the future, the port runs.
    #[test]
    fn a_stamp_from_the_future_does_not_skip_the_run_plan_decision_d12() {
        assert_eq!(decide(&facts("6", aged(-36_000)), NOW), Decision::Run);
        assert_eq!(decide(&facts("6", aged(-1)), NOW), Decision::Run);
    }

    #[test]
    fn a_stamp_exactly_now_has_age_zero_and_skips() {
        assert_eq!(
            decide(&facts("6", aged(0)), NOW),
            Decision::Skip { age_hours: 0, min_hours: 6 }
        );
    }

    #[test]
    fn whitespace_and_a_plus_sign_are_read_as_bash_reads_them() {
        assert_eq!(parse_hours(" 6 "), Some(6));
        assert_eq!(parse_hours("+6"), Some(6));
        assert_eq!(parse_hours("-6"), Some(-6));
        assert_eq!(parse_hours("00"), Some(0));
        assert_eq!(parse_hours(""), None);
    }

    #[test]
    fn the_string_00_is_a_number_and_the_gate_stays_on() {
        // `[ "$min" = 0 ]` compares text, so "00" does not switch the gate off.
        assert_eq!(decide(&facts("00", aged(60)), NOW), Decision::Run);
        assert_eq!(
            decide(&facts("+6", aged(60)), NOW),
            Decision::Skip { age_hours: 0, min_hours: 6 }
        );
    }

    #[test]
    fn the_skip_line_matches_the_bash_text() {
        assert_eq!(
            skip_line("review", 1, 6, None),
            "skipped: review last ran 1h ago, minimum is 6h (MAINTAINER_FORCE=1 overrides)\n"
        );
        assert_eq!(
            skip_line("review", 0, 6, Some("after waiting for the lock")),
            "skipped: review last ran 0h ago, minimum is 6h, after waiting for the lock (MAINTAINER_FORCE=1 overrides)\n"
        );
    }

    #[test]
    fn a_negative_minimum_never_skips() {
        assert_eq!(decide(&facts("-3", aged(60)), NOW), Decision::Run);
    }
}
