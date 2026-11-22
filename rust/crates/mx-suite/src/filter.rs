//! The test filter a verify runs.

use thiserror::Error;

/// Why a string is not a test filter.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TestFilterError {
    /// bash: `${3:?test filter}`.
    #[error("the test filter is empty")]
    Empty,
}

/// The third argument of `verify`: the text a suite puts on its test command.
///
/// **This text reaches a shell.** `suite_command "$filter"` interpolates it into
/// the string the container runs as `sh -c`, so a filter that contains `;`, `&&`
/// or a newline runs more than a filter. The container is contained (no network,
/// no capabilities, a read-only image), but the filter chooses what the clean and
/// the mutated run print, and `suite_ran` counts what they print. The pull
/// request lists this under "Found, not fixed" with a proposal; the type pins
/// today's behaviour and refuses only the empty filter, as bash does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFilter(String);

impl TestFilter {
    /// Checks the filter.
    pub fn parse(text: &str) -> Result<Self, TestFilterError> {
        if text.is_empty() {
            return Err(TestFilterError::Empty);
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
        for text in ["x", "a::b", "-- --exact x", "a; b"] {
            assert_eq!(TestFilter::parse(text).unwrap().as_str(), text);
        }
    }

    #[test]
    fn an_empty_filter_is_refused() {
        assert_eq!(TestFilter::parse(""), Err(TestFilterError::Empty));
    }

    #[test]
    fn shell_syntax_in_a_filter_is_accepted_today() {
        // Found, not fixed (P30-9): the filter is interpolated into `sh -c`.
        assert!(TestFilter::parse("x; echo 'test result: ok. 3 passed;'").is_ok());
    }
}
