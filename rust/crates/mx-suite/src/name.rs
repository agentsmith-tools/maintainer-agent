//! The name of a suite: the stem of a file in `verify.d/`.

use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// Why a string is not a suite name.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SuiteNameError {
    /// Nothing was given.
    #[error("a suite name cannot be empty")]
    Empty,
    /// More than [`SuiteName::MAX_LEN`] characters.
    #[error("a suite name has at most {max} characters; this one has {len}")]
    TooLong { len: usize, max: usize },
    /// The first character is not an ASCII letter or digit.
    #[error("a suite name starts with an ASCII letter or digit, not {found:?}")]
    BadStart { found: char },
    /// A later character is not an ASCII letter, digit, `-` or `_`.
    #[error(
        "a suite name has only ASCII letters, digits, '-' and '_'; {found:?} is at position {at}"
    )]
    BadChar { found: char, at: usize },
}

/// The name of a suite, checked once.
///
/// Plan decision F3, and `P30-2` for the grammar. The bash gate builds `"$dir/$named.sh"` from the command
/// line and sources it, so `verify 7 <sha> <filter> <mutation> ../../tmp/x`
/// ran whatever file that path reached. A `SuiteName` has no `/`, no `.`, no
/// space and no control character, so `format!("{name}.sh")` stays one file
/// name inside `verify.d/`, and a receipt's space-separated `suite` field splits
/// back into the names it was joined from. Every function of this crate that
/// reaches a suite file takes a `SuiteName`, so a string from the command line or
/// from a receipt cannot get there without passing [`SuiteName::parse`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SuiteName(String);

impl SuiteName {
    /// The longest name accepted, in characters.
    pub const MAX_LEN: usize = 64;

    /// Checks a suite name: 1 to 64 ASCII letters, digits, `-` and `_`, and the
    /// first one a letter or digit.
    pub fn parse(text: &str) -> Result<Self, SuiteNameError> {
        let len = text.chars().count();
        if len == 0 {
            return Err(SuiteNameError::Empty);
        }
        if len > Self::MAX_LEN {
            return Err(SuiteNameError::TooLong { len, max: Self::MAX_LEN });
        }
        for (at, found) in text.chars().enumerate() {
            let ok_anywhere = found.is_ascii_alphanumeric();
            if at == 0 && !ok_anywhere {
                return Err(SuiteNameError::BadStart { found });
            }
            if !ok_anywhere && found != '-' && found != '_' {
                return Err(SuiteNameError::BadChar { found, at });
            }
        }
        Ok(Self(text.to_string()))
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The file this suite lives in, `<name>.sh`, relative to `verify.d/`.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}.sh", self.0)
    }
}

impl fmt::Display for SuiteName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for SuiteName {
    type Err = SuiteNameError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_of_the_shipped_suites_are_accepted() {
        for name in ["rust", "docs", "shell", "node", "characterization"] {
            let parsed = SuiteName::parse(name).unwrap();
            assert_eq!(parsed.as_str(), name);
            assert_eq!(parsed.file_name(), format!("{name}.sh"));
            assert_eq!(parsed.to_string(), name);
        }
    }

    #[test]
    fn digits_underscore_hyphen_and_capitals_are_accepted() {
        for name in ["rust2", "7z", "my-suite", "my_suite", "Docs", "a", "A-1_b"] {
            assert_eq!(SuiteName::parse(name).unwrap().as_str(), name);
        }
    }

    #[test]
    fn an_empty_name_is_refused() {
        assert_eq!(SuiteName::parse(""), Err(SuiteNameError::Empty));
    }

    #[test]
    fn a_name_of_64_characters_is_accepted_and_65_is_refused() {
        assert!(SuiteName::parse(&"a".repeat(64)).is_ok());
        assert_eq!(
            SuiteName::parse(&"a".repeat(65)),
            Err(SuiteNameError::TooLong { len: 65, max: 64 })
        );
    }

    #[test]
    fn a_path_that_leaves_verify_d_is_refused() {
        // F3. The bash gate sources "$dir/$named.sh" for whatever the command line says.
        for name in ["../../tmp/x", "/etc/passwd", "sub/dir", "..", "a/../b"] {
            assert!(SuiteName::parse(name).is_err(), "{name:?} must be refused");
        }
    }

    #[test]
    fn a_dot_a_space_or_a_control_character_inside_a_name_is_refused() {
        assert_eq!(SuiteName::parse("a.b"), Err(SuiteNameError::BadChar { found: '.', at: 1 }));
        assert_eq!(SuiteName::parse("a b"), Err(SuiteNameError::BadChar { found: ' ', at: 1 }));
        assert_eq!(SuiteName::parse("a\nb"), Err(SuiteNameError::BadChar { found: '\n', at: 1 }));
        assert_eq!(SuiteName::parse("ab\0"), Err(SuiteNameError::BadChar { found: '\0', at: 2 }));
    }

    #[test]
    fn a_name_cannot_start_with_a_hyphen_underscore_or_dot() {
        assert_eq!(SuiteName::parse("-x"), Err(SuiteNameError::BadStart { found: '-' }));
        assert_eq!(SuiteName::parse("_x"), Err(SuiteNameError::BadStart { found: '_' }));
        assert_eq!(SuiteName::parse(".x"), Err(SuiteNameError::BadStart { found: '.' }));
    }

    #[test]
    fn a_non_ascii_letter_is_refused() {
        assert_eq!(SuiteName::parse("é"), Err(SuiteNameError::BadStart { found: 'é' }));
        assert_eq!(SuiteName::parse("aé"), Err(SuiteNameError::BadChar { found: 'é', at: 1 }));
    }

    #[test]
    fn names_order_by_their_bytes() {
        let mut names: Vec<SuiteName> =
            ["rust", "Rust", "docs", "7z"].iter().map(|n| SuiteName::parse(n).unwrap()).collect();
        names.sort();
        let shown: Vec<&str> = names.iter().map(SuiteName::as_str).collect();
        assert_eq!(shown, ["7z", "Rust", "docs", "rust"]);
    }

    #[test]
    fn from_str_is_parse() {
        assert!("rust".parse::<SuiteName>().is_ok());
        assert!("../x".parse::<SuiteName>().is_err());
    }

    #[test]
    fn each_refusal_prints_a_sentence_that_names_the_fault() {
        assert_eq!(SuiteNameError::Empty.to_string(), "a suite name cannot be empty");
        assert_eq!(
            SuiteNameError::TooLong { len: 70, max: 64 }.to_string(),
            "a suite name has at most 64 characters; this one has 70"
        );
        assert_eq!(
            SuiteNameError::BadStart { found: '-' }.to_string(),
            "a suite name starts with an ASCII letter or digit, not '-'"
        );
        assert_eq!(
            SuiteNameError::BadChar { found: '/', at: 3 }.to_string(),
            "a suite name has only ASCII letters, digits, '-' and '_'; '/' is at position 3"
        );
    }
}
