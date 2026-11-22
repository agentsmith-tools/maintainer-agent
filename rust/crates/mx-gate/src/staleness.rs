//! Has production code moved since the verified head? (`receipt_fresh`)

use crate::{GateDecision, PrNumber};

/// `git diff --name-only` failed. `reason` is what git said, for the refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFailure {
    pub reason: String,
}

/// The production paths that differ between the verified head and the tree being
/// merged, as a SUCCESSFUL `git diff --name-only` listed them. Empty is a real
/// answer: nothing in production moved.
///
/// Plan decision F1. The bash writes
///
/// ```text
/// diff="$(git -C "$REPO_PATH" diff --name-only "$merged" "mm-$pr" -- "${PROD_GLOBS[@]}" 2>/dev/null)"
/// ```
///
/// and then asks only `[ -n "$diff" ]`. A `git` that failed (an unknown revision,
/// a corrupt object, no repository) prints nothing, so the failure reads as "no
/// production diff; receipt still applies", and the gate merges against a
/// receipt that describes a tree nobody compared. An asserted receipt with a
/// bogus head reaches this branch on every merge. The only way to a
/// `ProductionDiff` is [`ProductionDiff::from_git`], which takes the Result of the
/// call, so a failure cannot be turned into an empty list on the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionDiff {
    paths: Vec<String>,
}

impl ProductionDiff {
    /// Reads the result of the diff. `Ok` carries git's standard output, one path
    /// per line (the bash `$(...)` has already dropped trailing newlines); `Err`
    /// is F1's refusal.
    pub fn from_git(
        pr: &PrNumber,
        verified_head: &str,
        result: Result<String, DiffFailure>,
    ) -> Result<Self, GateDecision> {
        match result {
            Ok(stdout) => Ok(Self {
                paths: stdout
                    .split('\n')
                    .filter(|path| !path.is_empty())
                    .map(str::to_string)
                    .collect(),
            }),
            Err(DiffFailure { reason }) => Err(GateDecision::StalenessUnreadable {
                pr: pr.clone(),
                verified_head: verified_head.to_string(),
                reason,
            }),
        }
    }

    /// The changed production paths, in git's order.
    #[must_use]
    pub fn paths(&self) -> &[String] {
        &self.paths
    }
}

/// The witness that production code did not move since the verified head: the
/// receipt still describes the tree being merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fresh(());

impl Fresh {
    /// bash `[ -n "$diff" ] && die "production code changed since the verified head ..."`.
    pub fn from_diff(diff: &ProductionDiff, verified_head: &str) -> Result<Self, GateDecision> {
        if diff.paths.is_empty() {
            Ok(Self(()))
        } else {
            Err(GateDecision::ProductionChanged {
                verified_head: verified_head.to_string(),
                paths: diff.paths.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> PrNumber {
        PrNumber::parse("7").unwrap()
    }

    const VERIFIED: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn a_successful_diff_with_no_output_means_nothing_moved() {
        let diff = ProductionDiff::from_git(&pr(), VERIFIED, Ok(String::new())).unwrap();
        assert!(diff.paths().is_empty());
        assert_eq!(Fresh::from_diff(&diff, VERIFIED), Ok(Fresh(())));
    }

    #[test]
    fn a_successful_diff_lists_its_paths_in_order() {
        let diff = ProductionDiff::from_git(
            &pr(),
            VERIFIED,
            Ok("crates/x/src/lib.rs\ncrates/y/src/new.rs".to_string()),
        )
        .unwrap();
        assert_eq!(diff.paths(), ["crates/x/src/lib.rs", "crates/y/src/new.rs"]);
    }

    #[test]
    fn a_failed_diff_is_not_an_empty_diff() {
        // F1: the bash reads this as "no production diff; receipt still applies".
        let refused = ProductionDiff::from_git(
            &pr(),
            VERIFIED,
            Err(DiffFailure { reason: "fatal: bad revision 'abcdefg..mm-7'".into() }),
        )
        .unwrap_err();
        assert_eq!(
            refused,
            GateDecision::StalenessUnreadable {
                pr: pr(),
                verified_head: VERIFIED.into(),
                reason: "fatal: bad revision 'abcdefg..mm-7'".into()
            }
        );
        assert_eq!(
            refused.to_string(),
            format!(
                "could not tell whether production code changed since the verified head {VERIFIED} of #7: git diff failed (fatal: bad revision 'abcdefg..mm-7').\n  An error is not an empty diff, so the receipt is not treated as still applying. Re-verify and record a new one."
            )
        );
    }

    #[test]
    fn a_failed_diff_with_no_message_still_refuses() {
        assert!(
            ProductionDiff::from_git(&pr(), VERIFIED, Err(DiffFailure { reason: String::new() }))
                .is_err()
        );
    }

    #[test]
    fn a_production_path_in_the_diff_makes_the_receipt_stale() {
        let diff = ProductionDiff::from_git(
            &pr(),
            VERIFIED,
            Ok("crates/x/src/lib.rs\ncrates/y/src/new.rs".into()),
        )
        .unwrap();
        let refused = Fresh::from_diff(&diff, VERIFIED).unwrap_err();
        // bash receipt_fresh:
        //   die "production code changed since the verified head $verified_head:
        //   $(printf '%s' "$diff" | sed 's/^/    /')
        //     The receipt no longer describes this tree. Re-verify and record a new one."
        assert_eq!(
            refused.to_string(),
            format!(
                "production code changed since the verified head {VERIFIED}:\n    crates/x/src/lib.rs\n    crates/y/src/new.rs\n  The receipt no longer describes this tree. Re-verify and record a new one."
            )
        );
    }

    #[test]
    fn a_single_changed_path_is_enough() {
        let diff = ProductionDiff::from_git(&pr(), VERIFIED, Ok("bin/x".into())).unwrap();
        assert!(matches!(
            Fresh::from_diff(&diff, VERIFIED),
            Err(GateDecision::ProductionChanged { .. })
        ));
    }
}
