//! The test filter a verify runs.

use thiserror::Error;

/// Why a string is not a test filter.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TestFilterError {
    /// bash: `${3:?test filter}`.
    #[error("the test filter is empty")]
    Empty,
    /// A character outside the allow-list, or a leading `-`. The text is the one
    /// the bash fix proposed in rv264 B3 will print.
    #[error(
        "the test filter may hold only letters, digits and _ . : / - and spaces, and may not start with -: '{filter}'"
    )]
    Rejected { filter: String },
}

/// The third argument of `verify`: the text a suite puts on its test command.
///
/// Decision `P30-9`, from rv264 B3. The bash splices the filter into the string
/// the container runs as `sh -c` (`suite_command "$filter"`), so a filter such as
/// `x; grep -q OLD f && echo "test result: ok. 3 passed; 0 failed"` made a clean run
/// pass with `suite_ran` 3 and a mutated run fail, a forged proof for any guard,
/// chosen by the agent that picks the filter and the mutation. Here a filter that
/// is not made of ASCII letters, digits and `_ . : / -` and spaces, or that starts
/// with `-`, is unrepresentable, so no shell metacharacter, quote, `$`, backtick,
/// newline or option reaches a suite. `mx_sandbox::ContainerSpec` takes the
/// finished command string of the suite and never a filter, so a filter is never
/// put into a shell string by this port. The bash fix is a separate change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFilter(String);

impl TestFilter {
    /// Checks the filter.
    pub fn parse(text: &str) -> Result<Self, TestFilterError> {
        if text.is_empty() {
            return Err(TestFilterError::Empty);
        }
        let allowed =
            |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | ' ' | '-');
        if text.trim_start_matches(' ').starts_with('-') || !text.chars().all(allowed) {
            return Err(TestFilterError::Rejected { filter: text.to_string() });
        }
        Ok(Self(text.to_string()))
    }

    /// The text, to be passed to `suite_command` as one argument.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filter_is_kept_as_typed() {
        for text in ["x", "a::b", "guard_bites", "tests/a.rs", "a b-c", "x-y"] {
            assert_eq!(TestFilter::parse(text).unwrap().as_str(), text);
        }
    }

    #[test]
    fn a_filter_whose_first_word_is_an_option_is_refused_even_after_blanks_p3b_14() {
        // The bash allow-list accepts ' -x' (its leading-dash test looks at the first character
        // only); the runner would read it as an option (review rv271 F6).
        for text in [" -x", "  --help", "\t-x", " -"] {
            assert!(
                matches!(TestFilter::parse(text), Err(TestFilterError::Rejected { .. })),
                "{text:?}"
            );
        }
        assert_eq!(TestFilter::parse("a -b").unwrap().as_str(), "a -b", "only the first word");
    }

    #[test]
    fn an_empty_filter_is_refused() {
        assert_eq!(TestFilter::parse(""), Err(TestFilterError::Empty));
    }

    #[test]
    fn the_filter_that_forges_evidence_is_unrepresentable() {
        // rv264 B3, repro1b.
        let forged = "x; grep -q OLD f && echo \"test result: ok. 3 passed; 0 failed\" || { echo \"test result: FAILED. 0 passed; 1 failed\"; exit 101; }";
        assert_eq!(
            TestFilter::parse(forged),
            Err(TestFilterError::Rejected { filter: forged.to_string() })
        );
    }

    #[test]
    fn every_shell_metacharacter_is_refused() {
        for bad in [
            ";", "a;b", "a&&b", "a||b", "a|b", "a&b", "$x", "$(x)", "`x`", "a\nb", "a\tb", "a'b",
            "a\"b", "a>b", "a<b", "a(b", "a)b", "a{b", "a}b", "a*b", "a?b", "a[b", "a\\b", "a#b",
            "a!b", "a~b", "a=b", "a,b", "a%b", "a@b", "a\0b", "é",
        ] {
            assert!(
                matches!(TestFilter::parse(bad), Err(TestFilterError::Rejected { .. })),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn a_leading_dash_is_refused_so_a_filter_cannot_be_an_option() {
        for bad in ["-x", "--exact", "- x"] {
            assert!(
                matches!(TestFilter::parse(bad), Err(TestFilterError::Rejected { .. })),
                "{bad:?}"
            );
        }
        assert!(TestFilter::parse("a -- --exact").is_ok());
    }

    #[test]
    fn the_refusal_prints_the_sentence_of_the_bash_fix() {
        assert_eq!(
            TestFilter::parse("a;b").unwrap_err().to_string(),
            "the test filter may hold only letters, digits and _ . : / - and spaces, and may not start with -: 'a;b'"
        );
    }
}
