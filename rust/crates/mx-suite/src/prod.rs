//! The profile's list of production paths.

use mx_core::NonEmpty;
use thiserror::Error;

/// A profile that declares no `PROD_GLOBS` gets no merge and no update.
///
/// The text is the `die` message of `require_prod_globs` in `bin/maintainer-merge`,
/// without the `maintainer-merge: ` prefix `die` adds.
// bash: die "this profile declares no PROD_GLOBS, so nothing can say
//   whether production code moved since the receipt. Set it in profile.env, for example:
//     PROD_GLOBS=\"bin/* lib/* scripts/* .github/*\""
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "this profile declares no PROD_GLOBS, so nothing can say\n  whether production code moved since the receipt. Set it in profile.env, for example:\n    PROD_GLOBS=\"bin/* lib/* scripts/* .github/*\""
)]
pub struct NoProdGlobs;

/// `PROD_GLOBS`: the paths whose change invalidates a receipt. At least one.
///
/// There is no default, because guessing which paths are production is how the
/// original bug worked (the comment above `PROD_GLOBS` in the bash gate). The
/// matching of a path against these patterns (`is_production`) is the work of
/// the suite agent; this type holds the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProdGlobs(NonEmpty<String>);

impl ProdGlobs {
    /// Reads the profile's value as the bash gate does:
    /// `read -r -a PROD_GLOBS <<< "${PROD_GLOBS:-}"`.
    ///
    /// `read` takes the FIRST LINE only and splits it on spaces and tabs, so a
    /// value that spans two lines silently loses everything after the first
    /// newline. That narrows what counts as production without a word, so the
    /// pull request lists it under "Found, not fixed"; this function pins it,
    /// and the test `a_second_line_is_ignored_as_the_bash_ignores_it` carries
    /// the name of that decision.
    pub fn from_profile_value(value: &str) -> Result<Self, NoProdGlobs> {
        let first_line = value.split('\n').next().unwrap_or("");
        let words: Vec<String> = first_line
            .split([' ', '\t'])
            .filter(|word| !word.is_empty())
            .map(str::to_string)
            .collect();
        NonEmpty::from_vec(words).map(Self).map_err(|_| NoProdGlobs)
    }

    /// The patterns, in the order the profile wrote them. Each is one word.
    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        self.0.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn globs(value: &str) -> Vec<String> {
        ProdGlobs::from_profile_value(value).unwrap().as_slice().to_vec()
    }

    #[test]
    fn the_example_of_the_refusal_splits_into_four_patterns() {
        assert_eq!(
            globs("bin/* lib/* scripts/* .github/*"),
            ["bin/*", "lib/*", "scripts/*", ".github/*"]
        );
    }

    #[test]
    fn spaces_and_tabs_separate_words_and_blanks_at_the_ends_are_ignored() {
        assert_eq!(globs("  a\tb   c \t"), ["a", "b", "c"]);
    }

    #[test]
    fn a_glob_is_one_word_and_is_never_expanded() {
        assert_eq!(globs("crates/*/src/*"), ["crates/*/src/*"]);
    }

    #[test]
    fn a_second_line_is_ignored_as_the_bash_ignores_it() {
        // Found, not fixed (P30-8): `read -r -a` stops at the first newline, so
        // PROD_GLOBS="bin/*<newline>docs/*" declares one pattern.
        assert_eq!(globs("bin/* lib/*\ndocs/*"), ["bin/*", "lib/*"]);
    }

    #[test]
    fn an_empty_or_blank_value_declares_nothing() {
        for value in ["", " ", "\t", "   \t  ", "\n", "\nbin/*"] {
            assert_eq!(ProdGlobs::from_profile_value(value), Err(NoProdGlobs), "{value:?}");
        }
    }

    #[test]
    fn the_refusal_prints_the_text_of_require_prod_globs() {
        assert_eq!(
            NoProdGlobs.to_string(),
            "this profile declares no PROD_GLOBS, so nothing can say\n  whether production code moved since the receipt. Set it in profile.env, for example:\n    PROD_GLOBS=\"bin/* lib/* scripts/* .github/*\""
        );
    }
}
