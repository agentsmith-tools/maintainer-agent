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
    /// `mapfile -t PROD_GLOBS < <(tr -s '[:space:]' '\n' <<<"${PROD_GLOBS:-}" | sed '/^$/d')`.
    ///
    /// The value splits on every run of whitespace, newlines included (bash line 83, #269).
    /// An older gate used `read -r -a`, which stops at the first newline and dropped the
    /// patterns of a second line without a word; this port never had that bug to preserve
    /// (decision P3A-15, replacing the pin of the first interface).
    pub fn from_profile_value(value: &str) -> Result<Self, NoProdGlobs> {
        let words: Vec<String> = value
            .split([' ', '\t', '\n', '\x0b', '\x0c', '\r'])
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
    /// (ranges, `!` or `^` negation, `[:alpha:]` and the other POSIX classes, `[=a=]`,
    /// `[.a.]`), `\x` the character `x`, and the extended patterns `?(..)`, `*(..)`, `+(..)`,
    /// `@(..)` and `!(..)` that `[[ ]]` reads whatever `shopt extglob` says. A leading `.` is an
    /// ordinary character.
    ///
    /// The answer errs toward "production" and never away from it (P3A-16): a construct this
    /// matcher cannot decide exactly, a matcher that ran past its step or depth bound, a
    /// `?` or bracket against a path that is not ASCII (bash counts characters in a UTF-8
    /// locale, this counts bytes), and a negation that bash may read differently, all answer
    /// yes. A pattern also matches when it does so in lower case or in upper case on both sides, because
    /// a profile can set `nocasematch` and this code cannot see it (P3A-19). A pattern over 512
    /// bytes or nested over 32 deep answers yes unmatched (P3A-20). Another `shopt` that changes
    /// `[[ == ]]` is not supported: the profile file is shell and can change anything.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        self.0.iter().any(|glob| glob_match(glob.as_bytes(), path.as_bytes()))
    }
}

/// bash pattern matching over bytes, as `[[ "$p" == $g ]]` does it with extglob on (the
/// operator turns it on whatever `shopt` says): `*`, `?`, bracket classes with ranges,
/// negation (`!` or `^`), POSIX classes `[:alpha:]`, equivalence classes `[=a=]` and
/// collating symbols `[.a.]`, `\x`, and the extended patterns `?(..)`, `*(..)`, `+(..)`,
/// `@(..)` and `!(..)` with `|` alternatives, nested. The matcher decides on bytes, the way
/// bash does in the C locale.
///
/// Whatever it cannot be sure of answers YES, "this is a production path": calling a
/// production path ordinary is the unsafe error, because the receipt then speaks for less than
/// the profile asked (P3A-16). That covers an unknown `[:name:]`, a negation that bash may read
/// differently ([`unsure`]), a `?` or bracket against text that is not ASCII, and a match that
/// ran past [`STEP_LIMIT`] steps or [`DEPTH_LIMIT`] nested calls.
fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    // A pattern this long or this deeply nested is not a real profile glob. Every step of the
    // matcher rescans the pattern, so the cost grows with the product; over the cap the answer
    // is yes without matching (P3A-20).
    if pattern.len() > PATTERN_LIMIT || nesting(pattern) > NESTING_LIMIT {
        return true;
    }
    // `[[ == ]]` follows `shopt -s nocasematch`, which a sourced profile.env can set, and the
    // matcher cannot see it. The answer is yes when the pattern matches as written, or with
    // both sides in lower case, or both in upper case (P3A-19): a path that differs from a
    // pattern only in case is called production, never ordinary.
    exact(pattern, text)
        || exact(&fold(pattern, false), &text.to_ascii_lowercase())
        || exact(&fold(pattern, true), &text.to_ascii_uppercase())
}

/// The pattern in one case, with the names inside `[:..:]`, `[=..=]` and `[..]` left alone: the
/// class `[:upper:]` is not `[:UPPER:]`.
fn fold(pattern: &[u8], upper: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(pattern.len());
    let mut i = 0;
    while i < pattern.len() {
        if pattern[i] == b'[' && matches!(pattern.get(i + 1), Some(b':' | b'=' | b'.')) {
            let kind = pattern[i + 1];
            let close = (i + 2..pattern.len().saturating_sub(1))
                .find(|&j| pattern[j] == kind && pattern[j + 1] == b']');
            if let Some(j) = close {
                out.extend_from_slice(&pattern[i..j + 2]);
                i = j + 2;
                continue;
            }
        }
        out.push(if upper {
            pattern[i].to_ascii_uppercase()
        } else {
            pattern[i].to_ascii_lowercase()
        });
        i += 1;
    }
    out
}

/// The longest pattern matched at all, in bytes.
const PATTERN_LIMIT: usize = 512;
/// The deepest `(` nesting matched at all.
const NESTING_LIMIT: usize = 32;

/// How deep the parentheses of the pattern nest.
fn nesting(pattern: &[u8]) -> usize {
    let (mut depth, mut deepest, mut i) = (0_usize, 0_usize, 0);
    while i < pattern.len() {
        match pattern[i] {
            b'\\' => i += 1,
            b'(' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    deepest
}

fn exact(pattern: &[u8], text: &[u8]) -> bool {
    if unsure(pattern, text) {
        return true;
    }
    let mut m = Bounds { steps: 0, depth: 0, tripped: false };
    let hit = full(pattern, text, &mut m);
    hit || m.tripped
}

/// Pattern work done so far and how deep it recursed. Past a limit the answer is yes.
struct Bounds {
    steps: u64,
    depth: u32,
    tripped: bool,
}

/// The most steps one match may take before it answers yes.
const STEP_LIMIT: u64 = 2_000_000;
/// The deepest the matcher recurses before it answers yes. A text of 60 000 bytes against
/// `*(a)` recursed once per byte and overflowed the stack.
const DEPTH_LIMIT: u32 = 1_000;

/// Cases the matcher refuses to decide, because bash and this code may disagree.
fn unsure(pattern: &[u8], text: &[u8]) -> bool {
    // bash counts characters under a UTF-8 locale and this matcher counts bytes, so a
    // single-character wildcard or a class against non-ASCII text can differ.
    if !text.is_ascii() && pattern.iter().any(|b| matches!(b, b'?' | b'[')) {
        return true;
    }
    // Negation: `!(..)` is exact only when it is the one wildcard group of the pattern and no
    // star stands outside it. Two of them, one inside another, or one beside a `*` is where
    // bash answers differently from the straightforward reading (found by differential).
    let negations = pattern.windows(2).filter(|w| w == b"!(").count();
    if negations >= 2 {
        return true;
    }
    negations == 1 && outside_group_has_star(pattern)
}

/// Does a `*` stand outside every `(..)` of the pattern?
fn outside_group_has_star(pattern: &[u8]) -> bool {
    let mut depth = 0_usize;
    let mut i = 0;
    while i < pattern.len() {
        match pattern[i] {
            b'\\' => i += 1,
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b'*' if depth == 0 && pattern.get(i + 1) != Some(&b'(') => return true,
            _ => {}
        }
        i += 1;
    }
    false
}

fn full(pattern: &[u8], text: &[u8], m: &mut Bounds) -> bool {
    m.steps += 1;
    if m.steps > STEP_LIMIT || m.depth >= DEPTH_LIMIT {
        m.tripped = true;
        return true;
    }
    m.depth += 1;
    let hit = step(pattern, text, m);
    m.depth -= 1;
    hit
}

fn step(pattern: &[u8], text: &[u8], m: &mut Bounds) -> bool {
    let Some(&first) = pattern.first() else { return text.is_empty() };
    // An extended pattern: one of ?*+@! right before a `(` that closes.
    if matches!(first, b'?' | b'*' | b'+' | b'@' | b'!')
        && pattern.get(1) == Some(&b'(')
        && let Some((alts, after)) = group(pattern, 1)
    {
        return extended(first, &alts, &pattern[after..], text, m);
    }
    match first {
        b'*' => (0..=text.len()).any(|k| full(&pattern[1..], &text[k..], m)),
        b'?' => !text.is_empty() && full(&pattern[1..], &text[1..], m),
        b'[' => match class(pattern, text.first().copied()) {
            Class::Hit(width) => !text.is_empty() && full(&pattern[width..], &text[1..], m),
            Class::Miss => false,
            // bash reads a `[` that never closes as a plain `[`.
            Class::Unclosed => text.first() == Some(&b'[') && full(&pattern[1..], &text[1..], m),
            Class::Unknown => {
                m.tripped = true;
                true
            }
        },
        b'\\' if pattern.len() > 1 => {
            text.first() == Some(&pattern[1]) && full(&pattern[2..], &text[1..], m)
        }
        c => text.first() == Some(&c) && full(&pattern[1..], &text[1..], m),
    }
}

/// The alternatives of the group whose `(` is at `pattern[open]`, and the index after its `)`.
/// `None` when the group never closes (then `@(` is plain characters).
fn group(pattern: &[u8], open: usize) -> Option<(Vec<&[u8]>, usize)> {
    let mut depth = 0_usize;
    let mut alts = Vec::new();
    let mut start = open + 1;
    let mut i = open;
    while i < pattern.len() {
        match pattern[i] {
            b'\\' => i += 1,
            b'[' => {
                // a bracket expression inside the group may hold `)` or `|`
                if let Some(end) = bracket_end(pattern, i) {
                    i = end - 1;
                }
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    alts.push(&pattern[start..i]);
                    return Some((alts, i + 1));
                }
            }
            b'|' if depth == 1 => {
                alts.push(&pattern[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn extended(op: u8, alts: &[&[u8]], rest: &[u8], text: &[u8], m: &mut Bounds) -> bool {
    let one = |span: &[u8], m: &mut Bounds| alts.iter().any(|alt| full(alt, span, m));
    match op {
        b'@' => (0..=text.len()).any(|k| one(&text[..k], m) && full(rest, &text[k..], m)),
        b'?' => {
            full(rest, text, m)
                || (0..=text.len()).any(|k| one(&text[..k], m) && full(rest, &text[k..], m))
        }
        b'*' | b'+' => repeat(op == b'+', alts, rest, text, m),
        _ => (0..=text.len()).any(|k| !one(&text[..k], m) && full(rest, &text[k..], m)),
    }
}

/// `*(..)` (zero or more) and `+(..)` (one or more) followed by `rest`.
fn repeat(at_least_one: bool, alts: &[&[u8]], rest: &[u8], text: &[u8], m: &mut Bounds) -> bool {
    if m.depth >= DEPTH_LIMIT {
        m.tripped = true;
        return true;
    }
    m.depth += 1;
    let hit = repeat_inner(at_least_one, alts, rest, text, m);
    m.depth -= 1;
    hit
}

fn repeat_inner(
    at_least_one: bool,
    alts: &[&[u8]],
    rest: &[u8],
    text: &[u8],
    m: &mut Bounds,
) -> bool {
    if at_least_one {
        // One round, which may be empty when an alternative is, then any number more.
        return (0..=text.len()).any(|k| {
            alts.iter().any(|alt| full(alt, &text[..k], m))
                && repeat(false, alts, rest, &text[k..], m)
        });
    }
    if full(rest, text, m) {
        return true;
    }
    // A further round must take at least one byte, or the recursion never ends.
    (1..=text.len()).any(|k| {
        alts.iter().any(|alt| full(alt, &text[..k], m)) && repeat(false, alts, rest, &text[k..], m)
    })
}

/// What a `[` in a pattern means for one byte of the text.
enum Class {
    /// The byte is in the class, which is this many bytes of the pattern wide.
    Hit(usize),
    /// A well-formed class that does not hold the byte (or there is no byte).
    Miss,
    /// The class never closes, so the `[` is an ordinary character.
    Unclosed,
    /// A `[:name:]` this matcher does not know.
    Unknown,
}

/// The index just past the `]` that closes the bracket expression at `pattern[at]`, if it closes.
fn bracket_end(pattern: &[u8], at: usize) -> Option<usize> {
    let mut i = at + 1;
    if matches!(pattern.get(i), Some(b'!' | b'^')) {
        i += 1;
    }
    let first = i;
    loop {
        let c = *pattern.get(i)?;
        if c == b']' && i > first {
            return Some(i + 1);
        }
        if c == b'[' && matches!(pattern.get(i + 1), Some(b':' | b'=' | b'.')) {
            let kind = pattern[i + 1];
            let close = (i + 2..pattern.len().saturating_sub(1))
                .find(|&j| pattern[j] == kind && pattern[j + 1] == b']');
            if let Some(j) = close {
                i = j + 2;
                continue;
            }
        }
        i += if c == b'\\' { 2 } else { 1 };
    }
}

fn posix_class(name: &[u8], byte: u8) -> Option<bool> {
    Some(match name {
        b"alpha" => byte.is_ascii_alphabetic(),
        b"digit" => byte.is_ascii_digit(),
        b"alnum" => byte.is_ascii_alphanumeric(),
        b"upper" => byte.is_ascii_uppercase(),
        b"lower" => byte.is_ascii_lowercase(),
        b"space" => matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'),
        b"blank" => matches!(byte, b' ' | b'\t'),
        b"punct" => byte.is_ascii_punctuation(),
        b"print" => (0x20..0x7f).contains(&byte),
        b"graph" => (0x21..0x7f).contains(&byte),
        b"cntrl" => byte < 0x20 || byte == 0x7f,
        b"xdigit" => byte.is_ascii_hexdigit(),
        b"ascii" => byte < 0x80,
        b"word" => byte.is_ascii_alphanumeric() || byte == b'_',
        _ => return None,
    })
}

/// Matches the first byte of the text against the class that starts at `pattern[0]`.
fn class(pattern: &[u8], byte: Option<u8>) -> Class {
    let Some(end) = bracket_end(pattern, 0) else { return Class::Unclosed };
    let Some(byte) = byte else { return Class::Miss };
    let mut i = 1;
    let negate = matches!(pattern.get(i), Some(b'!' | b'^'));
    if negate {
        i += 1;
    }
    let mut hit = false;
    // `end - 1` is the `]` that closes the expression; a `]` right after the opening is a member.
    while i < end - 1 {
        let c = pattern[i];
        if c == b'[' && matches!(pattern.get(i + 1), Some(b':' | b'=' | b'.')) {
            let kind = pattern[i + 1];
            let close = (i + 2..end - 1).find(|&j| pattern[j] == kind && pattern[j + 1] == b']');
            if let Some(j) = close {
                let name = &pattern[i + 2..j];
                match kind {
                    b':' => match posix_class(name, byte) {
                        Some(yes) => hit |= yes,
                        None => return Class::Unknown,
                    },
                    // `[=a=]` and `[.a.]` name one character in the C locale; a longer name
                    // is a collating element this matcher does not know.
                    _ if name.len() == 1 => hit |= name[0] == byte,
                    _ => return Class::Unknown,
                }
                i = j + 2;
                continue;
            }
        }
        let (lo, next) =
            if c == b'\\' && i + 1 < end { (pattern[i + 1], i + 2) } else { (c, i + 1) };
        if pattern.get(next) == Some(&b'-') && pattern.get(next + 1).is_some_and(|&e| e != b']') {
            let hi = pattern[next + 1];
            hit |= (lo..=hi).contains(&byte);
            i = next + 2;
        } else {
            hit |= lo == byte;
            i = next;
        }
    }
    if hit == negate { Class::Miss } else { Class::Hit(end) }
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
    fn a_bracket_that_never_closes_is_a_plain_bracket_as_in_bash() {
        // bash: [[ "[" == [ ]] and [[ "[a" == [a ]] are true; the class never closes.
        let globs = |g: &str| ProdGlobs::from_profile_value(g).unwrap();
        assert!(globs("[").matches("["));
        assert!(globs("[a").matches("[a"));
        assert!(globs("x[").matches("x["));
        assert!(globs("[!").matches("[!"));
        assert!(!globs("[").matches("a"));
        assert!(!globs("[a").matches("a"));
        assert!(globs("[a]").matches("a"), "a class that closes is still a class");
    }

    #[test]
    fn a_second_line_is_read_as_the_bash_reads_it() {
        // bash line 83 (#269): `tr -s '[:space:]' '\n'` splits on every whitespace byte, so
        // the second line's patterns count (P3A-15).
        assert_eq!(globs("bin/* lib/*\ndocs/*"), ["bin/*", "lib/*", "docs/*"]);
        assert_eq!(globs("a\n\n b\t\x0bc\r\x0cd"), ["a", "b", "c", "d"]);
    }

    #[test]
    fn a_leading_newline_does_not_hide_the_patterns_after_it() {
        assert_eq!(globs("\nbin/*"), ["bin/*"]);
    }

    #[test]
    fn an_empty_or_blank_value_declares_nothing() {
        for value in ["", " ", "\t", "   \t  ", "\n", " \n\r\x0b\x0c "] {
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
    fn extended_patterns_follow_bash_with_extglob_on() {
        // bash turns extglob on inside [[ ]] whatever `shopt` says.
        assert!(matches("@(a|b)", "a"));
        assert!(!matches("@(a|b)", "c"));
        assert!(matches("src/@(a|b).rs", "src/b.rs"));
        assert!(matches("?(x)y", "y") && matches("?(x)y", "xy") && !matches("?(x)y", "xxy"));
        assert!(matches("*(ab)c", "c") && matches("*(ab)c", "ababc") && !matches("*(ab)c", "abac"));
        assert!(matches("+(ab)c", "abc") && !matches("+(ab)c", "c"));
        assert!(matches("+(|a)b", "b"), "an empty alternative is one empty round");
        assert!(matches("!(a)b", "xb") && !matches("!(a)b", "ab") && matches("!(a)b", "b"));
        assert!(matches("bin/!(*.md)", "bin/run") && !matches("bin/!(*.md)", "bin/a.md"));
        assert!(matches("@(a|@(b|c))d", "cd"), "groups nest");
        assert!(matches("@(a", "@(a"), "a group that never closes is plain characters");
    }

    #[test]
    fn posix_classes_equivalence_classes_and_collating_symbols_follow_bash() {
        assert!(matches("[[:alpha:]]x", "ax") && !matches("[[:alpha:]]x", "1x"));
        assert!(matches("[![:digit:]]", "a") && !matches("[![:digit:]]", "5"));
        assert!(matches("[[:upper:][:digit:]]", "Q") && matches("[[:upper:][:digit:]]", "7"));
        assert!(matches("[[=a=]]", "a") && !matches("[[=a=]]", "b"));
        assert!(matches("[[.a.]]", "a"));
        assert!(matches("[]a]", "]") && matches("[!]a]", "b"));
    }

    #[test]
    fn every_member_of_every_posix_class_is_pinned_at_its_edges() {
        // (class, bytes that are in it, bytes that are not)
        let table: [(&str, &str, &str); 14] = [
            ("alpha", "azAZ", "09_ -"),
            ("digit", "09", "aA_ /"),
            ("alnum", "az09AZ", "_ -/"),
            ("upper", "AZ", "az09_"),
            ("lower", "az", "AZ09_"),
            ("space", " \t\n\x0b\x0c\r", "a0_"),
            ("blank", " \t", "\n\r\x0b\x0ca"),
            ("punct", "!/_-~[]@", "az09 \t"),
            ("print", " a~09", "\x1f\x7f\t"),
            ("graph", "!a~09", " \x1f\x7f"),
            ("cntrl", "\x00\x1f\x7f\t", " a09~"),
            ("xdigit", "09afAF", "gG_ -"),
            ("ascii", "a\x00\x7f", ""),
            ("word", "az09AZ_", "- /."),
        ];
        for (name, inside, outside) in table {
            let class = format!("[[:{name}:]]");
            for c in inside.chars() {
                assert!(exact(class.as_bytes(), c.to_string().as_bytes()), "{class} holds {c:?}");
            }
            for c in outside.chars() {
                assert!(!exact(class.as_bytes(), c.to_string().as_bytes()), "{class} lacks {c:?}");
            }
        }
    }

    #[test]
    fn a_backslash_inside_an_extglob_group_protects_the_next_byte() {
        assert!(matches("@(a\\)b)", "a)b"));
        assert!(!matches("@(a\\)b)", "a"));
        assert!(
            matches("@(a\\|b)", "a|b") && !matches("@(a\\|b)", "a") && !matches("@(a\\|b)", "b")
        );
        assert!(matches("@([)]|x)", ")"), "a bracket expression inside a group may hold a paren");
    }

    #[test]
    fn a_question_mark_is_exactly_one_byte_of_ascii_text() {
        assert!(matches("?", "a") && !matches("?", "") && !matches("?", "ab"));
        assert!(matches("a?c", "abc") && !matches("a?c", "ac") && !matches("a?c", "abbc"));
        assert!(!matches("??", "a") && matches("??", "ab"));
    }

    #[test]
    fn a_wildcard_or_class_against_text_that_is_not_ascii_answers_production() {
        // bash counts characters under a UTF-8 locale and this matcher counts bytes (P3A-16).
        assert!(matches("?", "é") && matches("??", "é") && matches("[!x]", "é"));
        assert!(
            matches("[[:alpha:]]", "é") && matches("a?c", "aéc") && matches("src/?.rs", "src/é.rs")
        );
        // a literal and a star decide exactly on bytes
        assert!(matches("é*", "ébc") && !matches("é*", "ebc") && matches("*é", "aé"));
        assert!(!matches("src/*", "lib/é.rs"));
    }

    #[test]
    fn a_negation_that_bash_may_read_differently_answers_production() {
        // Found by differential: bash says production for these and the straightforward
        // reading says no. Answering yes is the safe direction (P3A-16).
        assert!(matches("!(a)*!(a|!(b))", "x"));
        assert!(matches("*!(@(a|b))*c\\?", "zz"));
        assert!(matches("@(a|b|c)*!(a)*[[=a=]]", "q"));
        // two negations and no star: `!(*)` matches nothing, so the plain reading says no
        assert!(matches("!(*)!(*)", "x"));
        // a lone negation is still exact
        assert!(!matches("bin/!(*.md)", "bin/a.md") && matches("bin/!(*.md)", "bin/run"));
    }

    #[test]
    fn a_match_that_runs_past_the_step_limit_answers_production() {
        // Nothing here can match (no `b` in the text), and the nested repeats take exponential
        // time, so the step limit ends it. The answer is yes, never no (P3A-16, N01).
        let pattern = "*(a|aa)*(a|aa)*(a|aa)*(a|aa)*(a|aa)*(a|aa)b";
        let text = "a".repeat(60);
        assert!(matches(pattern, &text));
        let mut m = Bounds { steps: 0, depth: 0, tripped: false };
        assert!(!full(pattern.as_bytes(), text.as_bytes(), &mut m) || m.tripped);
        assert!(m.tripped, "the limit, not the pattern, ended the match");
    }

    #[test]
    fn a_step_limit_hit_inside_a_negation_still_answers_production() {
        // `!(..)` inverts a tripped inner answer, so the limit must be remembered outside the
        // negation (R3-1). The group cannot match 60 `a`, bash says the negation does, and the
        // exponential inner pattern ends at the step limit, not by deciding.
        let pattern = "!(*(a|aa)*(a|aa)*(a|aa)*(a|aa)*(a|aa)*(a|aa)b)";
        let text = "a".repeat(60);
        assert!(matches(pattern, &text));
        let mut m = Bounds { steps: 0, depth: 0, tripped: false };
        full(pattern.as_bytes(), text.as_bytes(), &mut m);
        assert!(m.tripped, "the limit, not the pattern, ended the match");
        assert!(exact(pattern.as_bytes(), text.as_bytes()), "yes comes from the remembered limit");
    }

    #[test]
    fn a_pattern_nested_thousands_deep_answers_production_at_once() {
        let pattern = format!("{}a{}", "+(".repeat(5000), ")".repeat(5000));
        let started = std::time::Instant::now();
        assert!(matches(&pattern, &"a".repeat(100)));
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
        let long = "a".repeat(600);
        assert!(matches(&long, "zzz"), "a pattern over 512 bytes answers yes unmatched");
        assert!(!matches(&"a".repeat(512), "zzz"), "512 bytes is still matched");
        let deep = format!("{}a{}", "@(".repeat(33), ")".repeat(33));
        assert!(
            matches(&deep, "zzz")
                && !matches(&format!("{}a{}", "@(".repeat(32), ")".repeat(32)), "zzz")
        );
    }

    #[test]
    fn a_path_that_differs_from_a_pattern_only_in_case_answers_production() {
        // A profile can set nocasematch and this code cannot see it (P3A-19).
        assert!(matches("src/*", "SRC/x") && matches("SRC/*", "src/x") && matches("*.rs", "A.RS"));
        assert!(matches("*[a-c][A-Z]", "acAZ"));
        assert!(matches("*(!(a))b", "bB"));
        // each case variant is needed on its own: a class names its case
        assert!(matches("[[:lower:]]", "A"), "only the lower-case variant matches");
        assert!(matches("[[:upper:]]", "a"), "only the upper-case variant matches");
        assert!(!matches("src/*", "lib/x") && !matches("*.rs", "a.md"));
    }

    #[test]
    fn a_path_of_a_million_bytes_against_a_repeat_answers_production_without_overflowing() {
        // 60 000 bytes against `*(a)` overflowed the stack when the matcher recursed per byte.
        let text = "a".repeat(1_000_000);
        assert!(matches("*(a)", &text));
        assert!(matches("*(a)b", &text), "depth bound: yes, not no and not a crash");
        assert!(matches("+(a)", &text));
        let long_path = format!("{}x", "a/".repeat(500_000));
        assert!(matches("*x", &long_path), "a star over a long path is still decided");
    }

    #[test]
    fn a_class_name_this_matcher_does_not_know_answers_production_not_ordinary() {
        // Fail closed (P3A-16): calling a production path ordinary is the unsafe error.
        assert!(matches("[[:nosuch:]]", "anything"));
        assert!(matches("[[.space.]]", "anything"));
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
