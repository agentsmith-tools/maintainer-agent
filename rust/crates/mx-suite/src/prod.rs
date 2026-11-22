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

    /// The patterns as `git diff -- <pathspec>...` arguments, one per pattern, never expanded.
    /// A git pathspec and a bash pattern read `*` alike (it crosses `/` in both), but
    /// git also reads `:(magic)` prefixes and the bash does not, so [`ProdGlobs::matches`]
    /// and git can disagree on a pattern that starts with `:`.
    #[must_use]
    pub fn pathspecs(&self) -> &[String] {
        self.0.as_slice()
    }

    /// bash `is_production`: does `path` match any pattern, as `[[ "$p" == $g ]]` matches it?
    /// `*` matches any run of characters including `/`, `?` one character, `[...]` a class
    /// (with `!` or `^` to negate and `a-z` ranges), and `\x` the character `x`. There is no
    /// extended globbing, and a leading `.` is an ordinary character.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        self.0.iter().any(|glob| glob_match(glob.as_bytes(), path.as_bytes()))
    }
}

/// bash pattern matching over bytes, without extglob.
fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        let step = match pattern.get(p) {
            Some(b'*') => {
                star = Some((p, t));
                p += 1;
                continue;
            }
            Some(b'?') => Some(1),
            Some(b'[') => class_match(pattern, p, text[t]),
            Some(b'\\') if p + 1 < pattern.len() => (pattern[p + 1] == text[t]).then_some(2),
            Some(&c) => (c == text[t]).then_some(1),
            None => None,
        };
        match step {
            Some(width) => {
                p += width;
                t += 1;
            }
            None => match star {
                Some((sp, st)) => {
                    p = sp + 1;
                    t = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

/// Matches one byte against the class that starts at `pattern[at]`. Returns the width of the
/// class in the pattern when the byte is in it, and `None` when it is not or when the class
/// never closes (an unclosed `[` is a plain `[`, which then has to equal the byte).
fn class_match(pattern: &[u8], at: usize, byte: u8) -> Option<usize> {
    let mut i = at + 1;
    let negate = matches!(pattern.get(i), Some(b'!' | b'^'));
    if negate {
        i += 1;
    }
    let first = i;
    let mut hit = false;
    loop {
        let c = *pattern.get(i)?;
        if c == b']' && i > first {
            break;
        }
        let (lo, next) =
            if c == b'\\' && i + 1 < pattern.len() { (pattern[i + 1], i + 2) } else { (c, i + 1) };
        if pattern.get(next) == Some(&b'-') && pattern.get(next + 1).is_some_and(|&e| e != b']') {
            let hi = pattern[next + 1];
            hit |= (lo..=hi).contains(&byte);
            i = next + 2;
        } else {
            hit |= lo == byte;
            i = next;
        }
    }
    (hit != negate).then_some(i + 1 - at)
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

    fn matches(globs: &str, path: &str) -> bool {
        ProdGlobs::from_profile_value(globs).unwrap().matches(path)
    }

    #[test]
    fn a_star_crosses_slashes_as_it_does_in_a_bash_double_bracket() {
        assert!(matches("bin/*", "bin/x"));
        assert!(matches("bin/*", "bin/a/b/c"));
        assert!(matches("crates/*/src/*", "crates/x/src/deep/lib.rs"));
        assert!(matches(".github/*", ".github/workflows/ci.yml"));
        assert!(!matches("bin/*", "lib/x"));
        assert!(!matches("bin/*", "bin"));
        assert!(matches("*", ""));
    }

    #[test]
    fn a_pattern_must_match_the_whole_path() {
        assert!(matches("*.rs", "src/lib.rs"));
        assert!(!matches("*.rs", "src/lib.rs.bak"));
        assert!(!matches("lib", "lib/x"));
        assert!(matches("lib", "lib"));
        assert!(matches("a?b/*", "a b/c"));
    }

    #[test]
    fn question_mark_class_and_backslash_follow_bash() {
        assert!(matches("a?c", "abc"));
        assert!(!matches("a?c", "ac"));
        assert!(matches("[abc]x", "bx"));
        assert!(!matches("[abc]x", "dx"));
        assert!(matches("[a-c]x", "bx"));
        assert!(matches("[!a-c]x", "dx"));
        assert!(matches("[^a-c]x", "dx"));
        assert!(!matches("[!a-c]x", "bx"));
        assert!(matches("a\\*b", "a*b"));
        assert!(!matches("a\\*b", "aXb"));
        assert!(matches("[]]x", "]x"));
    }

    #[test]
    fn any_one_pattern_is_enough_and_a_leading_dot_is_ordinary() {
        assert!(matches("bin/* lib/*", "lib/y"));
        assert!(matches("*", ".hidden"));
        assert!(!matches("bin/* lib/*", "docs/y"));
    }

    #[test]
    fn it_agrees_with_bash_on_the_recorded_cases() {
        // Each row was run through `[[ "$p" == $g ]]` in bash 5.3 on 2026-11-22.
        let rows: &[(&str, &str, bool)] = &[
            (r"bin/*", r"bin/x", true),
            (r"bin/*", r"bin/a/b", true),
            (r"bin/*", r"lib/x", false),
            (r"*.rs", r"src/lib.rs", true),
            (r"*.rs", r"src/lib.rs.bak", false),
            (r"a?c", r"abc", true),
            (r"a?c", r"ac", false),
            (r"[abc]x", r"bx", true),
            (r"[a-c]x", r"bx", true),
            (r"[!a-c]x", r"dx", true),
            (r"[^a-c]x", r"dx", true),
            (r"[!a-c]x", r"bx", false),
            (r"a\*b", r"a*b", true),
            (r"a\*b", r"aXb", false),
            (r"[]]x", r"]x", true),
            (r"*", r".hidden", true),
            (r"crates/*/src/*", r"crates/x/src/deep/lib.rs", true),
            (r".github/*", r".github/ci.yml", true),
            (r"a*b*c", r"aXbYc", true),
            (r"a*b*c", r"aXbY", false),
            (r"*a", r"banana", true),
            (r"*a*", r"xyz", false),
            (r"[a-c-e]x", r"dx", false),
            (r"[a\]]x", r"]x", true),
        ];
        for (glob, path, want) in rows {
            let globs = ProdGlobs(NonEmpty::new((*glob).to_string(), []));
            assert_eq!(globs.matches(path), *want, "{glob} against {path}");
        }
        assert_eq!(rows.len(), 24);
    }

    #[test]
    fn extglob_is_not_special() {
        assert!(!matches("@(a|b)", "a"));
        assert!(matches("@(a|b)", "@(a|b)"));
    }

    #[test]
    fn pathspecs_are_the_patterns_as_written() {
        let globs = ProdGlobs::from_profile_value("bin/* .github/*").unwrap();
        assert_eq!(globs.pathspecs(), ["bin/*", ".github/*"]);
    }

    #[test]
    fn the_refusal_prints_the_text_of_require_prod_globs() {
        assert_eq!(
            NoProdGlobs.to_string(),
            "this profile declares no PROD_GLOBS, so nothing can say\n  whether production code moved since the receipt. Set it in profile.env, for example:\n    PROD_GLOBS=\"bin/* lib/* scripts/* .github/*\""
        );
    }
}
