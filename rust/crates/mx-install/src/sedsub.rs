//! The replacement half of a GNU sed `s` command, which `new-profile.sh` and the
//! "kept the audit trail" lines of `install.sh --uninstall` hand user text to.
//!
//! The bash builds a sed script around the text (`s|__SLUG__|$slug|g`), so sed reads the
//! text as a replacement and not as data: `\n` is a newline, `\\` is one backslash, `\U`
//! turns the rest to upper case, `\0` is the matched text and `\1` is an error because
//! the pattern has no group. The scripts forbid `|` and `&`, which sed reads as the
//! delimiter and as the matched text, and nothing else (plan decision: preserved, pinned by
//! tests that name it). This module reproduces what GNU sed 4.9 does with the rest, so the
//! port writes the same bytes and fails on the same inputs.
//!
//! The pattern is always a literal, so there is no group: `\1` to `\9` fail and `\0` and
//! `&` stand for the pattern itself. Case conversion is ASCII only, as in the C locale
//! the suites run in.

/// Why sed refused the script. The caller knows where the expression sits and says how
/// many characters sed had read; see [`SedFailure::message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SedFault {
    /// A newline, or a backslash that swallowed the closing delimiter, ended the text
    /// early. `before` counts the characters of the replacement that came first, and
    /// `None` means the text ran to its end.
    Unterminated { before: Option<usize> },
    /// `\1` to `\9`: the pattern has no such group.
    InvalidReference { digit: u8 },
    /// `\c\` followed by something other than a second backslash.
    RecursiveEscape,
    /// An unescaped `|` closed the replacement early and the text after it is not a flag.
    UnknownOption,
}

/// A fault with the position GNU sed reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SedFailure {
    /// 1-based number of the `-e` expression, which sed counts from its command line.
    pub expression: usize,
    /// Characters of the expression that sed had read when it stopped.
    pub chars: usize,
    pub fault: SedFault,
}

impl SedFailure {
    /// The line sed prints on standard error (without its newline).
    #[must_use]
    pub fn message(&self) -> String {
        let what = match &self.fault {
            SedFault::Unterminated { .. } => "unterminated `s' command".to_string(),
            SedFault::InvalidReference { digit } => {
                format!("invalid reference \\{} on `s' command's RHS", *digit as char)
            }
            SedFault::RecursiveEscape => "recursive escaping after \\c not allowed".to_string(),
            SedFault::UnknownOption => "unknown option to `s'".to_string(),
        };
        format!("sed: -e expression #{}, char {}: {what}", self.expression, self.chars)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Case {
    Keep,
    Upper,
    Lower,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
    Literal(u8),
    /// `&` or `\0`: the text the pattern matched.
    Matched,
    /// `\U`, `\L`, `\E`: the case of everything after it.
    Mode(Case),
    /// `\u`, `\l`: the case of the next character only.
    Once(Case),
}

/// A parsed replacement, ready to apply to any number of matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    items: Vec<Item>,
}

/// Parses the raw text between the second and third delimiter. `prefix` is the number of
/// characters of the expression that come before the replacement (`s|PATTERN|`), `tail`
/// the text after it (`|g`: the delimiter and the flags), and `expression` the number sed
/// gives the expression: they place the error exactly where sed does.
pub fn parse_replacement(
    raw: &[u8],
    expression: usize,
    prefix: usize,
    tail: &[u8],
) -> Result<Replacement, SedFailure> {
    let suffix = tail.len();
    let fail = |chars: usize, fault: SedFault| SedFailure { expression, chars, fault };
    // sed finds the end of the replacement first. A newline that no backslash took ends it
    // (the error names the characters before the newline), and so does running out of text
    // after a backslash took the delimiter.
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'\n' => return Err(fail(prefix + i, SedFault::Unterminated { before: Some(i) })),
            b'\\' => i += 2,
            b'|' => {
                // The delimiter ends the replacement here, and what follows is read as the
                // flags of the command: `g`, `p`, `i`, `I`, `m`, `M`, `e` and digits are
                // flags, anything else is an unknown option.
                let rest = raw[i + 1..].iter().chain(tail.iter());
                for (offset, flag) in rest.enumerate() {
                    if !(b"gpiImMe".contains(flag) || flag.is_ascii_digit()) {
                        return Err(fail(prefix + i + 1 + offset + 1, SedFault::UnknownOption));
                    }
                }
                break;
            }
            _ => i += 1,
        }
    }
    if i > raw.len() {
        return Err(fail(prefix + raw.len() + suffix, SedFault::Unterminated { before: None }));
    }
    let end = prefix + raw.len() + suffix;
    let mut items = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let byte = raw[i];
        if byte == b'&' {
            items.push(Item::Matched);
            i += 1;
            continue;
        }
        if byte != b'\\' {
            items.push(Item::Literal(byte));
            i += 1;
            continue;
        }
        let next = raw[i + 1];
        i += 2;
        match next {
            b'0' => items.push(Item::Matched),
            b'1'..=b'9' => return Err(fail(end, SedFault::InvalidReference { digit: next })),
            b'U' => items.push(Item::Mode(Case::Upper)),
            b'L' => items.push(Item::Mode(Case::Lower)),
            b'E' => items.push(Item::Mode(Case::Keep)),
            b'u' => items.push(Item::Once(Case::Upper)),
            b'l' => items.push(Item::Once(Case::Lower)),
            b'n' | b'\n' => items.push(Item::Literal(b'\n')),
            b'a' => items.push(Item::Literal(0x07)),
            b'f' => items.push(Item::Literal(0x0c)),
            b'r' => items.push(Item::Literal(b'\r')),
            b't' => items.push(Item::Literal(b'\t')),
            b'v' => items.push(Item::Literal(0x0b)),
            b'c' => {
                if i >= raw.len() {
                    // `\c` with nothing after it leaves a backslash behind.
                    items.push(Item::Literal(b'\\'));
                } else {
                    let target = raw[i];
                    i += 1;
                    if target == b'\\' {
                        if raw.get(i) != Some(&b'\\') {
                            return Err(fail(end, SedFault::RecursiveEscape));
                        }
                        i += 1;
                    }
                    items.push(Item::Literal(target.to_ascii_uppercase() ^ 0x40));
                }
            }
            b'd' | b'o' | b'x' => {
                let (base, limit): (u32, usize) = match next {
                    b'd' => (10, 3),
                    b'o' => (8, 3),
                    _ => (16, 2),
                };
                let mut value: u32 = 0;
                let mut used = 0;
                while used < limit {
                    let Some(digit) = raw.get(i).and_then(|b| (*b as char).to_digit(base)) else {
                        break;
                    };
                    value = value * base + digit;
                    used += 1;
                    i += 1;
                }
                if used == 0 {
                    items.push(Item::Literal(next));
                } else {
                    items.push(Item::Literal((value & 0xff) as u8));
                }
            }
            other => items.push(Item::Literal(other)),
        }
    }
    Ok(Replacement { items })
}

impl Replacement {
    /// The text that replaces one match of `matched`.
    #[must_use]
    pub fn expand(&self, matched: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut mode = Case::Keep;
        let mut once = Case::Keep;
        let mut push = |byte: u8, mode: Case, once: &mut Case| {
            let case = if *once == Case::Keep { mode } else { *once };
            *once = Case::Keep;
            out.push(match case {
                Case::Keep => byte,
                Case::Upper => byte.to_ascii_uppercase(),
                Case::Lower => byte.to_ascii_lowercase(),
            });
        };
        for item in &self.items {
            match item {
                Item::Literal(byte) => push(*byte, mode, &mut once),
                Item::Matched => {
                    for byte in matched {
                        push(*byte, mode, &mut once);
                    }
                }
                // `\L`, `\U` and `\E` also drop a pending `\u` or `\l`: `\u\LBCD` is `bcd`.
                Item::Mode(case) => {
                    mode = *case;
                    once = Case::Keep;
                }
                Item::Once(case) => once = *case,
            }
        }
        out
    }
}

/// `s|PATTERN|REPLACEMENT|g` over the whole text: every non-overlapping occurrence of the
/// literal `pattern`, left to right.
#[must_use]
pub fn replace_all(text: &[u8], pattern: &[u8], replacement: &Replacement) -> Vec<u8> {
    replace(text, pattern, replacement, true)
}

/// `s|PATTERN|REPLACEMENT|` over one line: the first occurrence only.
#[must_use]
pub fn replace_first(text: &[u8], pattern: &[u8], replacement: &Replacement) -> Vec<u8> {
    replace(text, pattern, replacement, false)
}

fn replace(text: &[u8], pattern: &[u8], replacement: &Replacement, all: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut at = 0;
    let mut done = false;
    while at < text.len() {
        if !done && !pattern.is_empty() && text[at..].starts_with(pattern) {
            out.extend(replacement.expand(pattern));
            at += pattern.len();
            done = !all;
        } else {
            out.push(text[at]);
            at += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(raw: &str) -> Result<Vec<u8>, SedFailure> {
        // The expression `s|__A__|RAW|g` is what the suites ran against real sed.
        let repl = parse_replacement(raw.as_bytes(), 1, 8, b"|g")?;
        Ok(replace_all(b"X__A__Y", b"__A__", &repl))
    }

    fn text(raw: &str) -> String {
        String::from_utf8(sub(raw).unwrap()).unwrap()
    }

    #[test]
    fn a_backslash_n_is_a_newline_and_a_double_backslash_is_one_backslash() {
        assert_eq!(text(r"a\nb"), "Xa\nbY");
        assert_eq!(text(r"a\\b"), "Xa\\bY");
        assert_eq!(text(r"a\tb"), "Xa\tbY");
    }

    #[test]
    fn a_backslash_zero_is_the_matched_text_and_other_backslashes_are_dropped() {
        assert_eq!(text(r"a\0b"), "Xa__A__bY");
        assert_eq!(text(r"a\/b"), "Xa/bY");
        assert_eq!(text(r"C:\Users\me"), "XC:SERSMEY");
    }

    #[test]
    fn a_numbered_group_is_an_error_at_the_end_of_the_expression() {
        let failure = sub(r"a\1b").unwrap_err();
        assert_eq!(
            failure.message(),
            "sed: -e expression #1, char 14: invalid reference \\1 on `s' command's RHS"
        );
        assert_eq!(sub(r"a\9b").unwrap_err().fault, SedFault::InvalidReference { digit: b'9' });
    }

    #[test]
    fn a_trailing_backslash_swallows_the_delimiter_and_a_newline_ends_the_text() {
        assert_eq!(
            sub("ab\\").unwrap_err().message(),
            "sed: -e expression #1, char 13: unterminated `s' command"
        );
        assert_eq!(
            sub("a\nb").unwrap_err().message(),
            "sed: -e expression #1, char 9: unterminated `s' command"
        );
        assert_eq!(
            sub("ab\nc|").unwrap_err().message(),
            "sed: -e expression #1, char 10: unterminated `s' command"
        );
    }

    #[test]
    fn an_unescaped_pipe_ends_the_replacement_and_the_next_character_is_an_unknown_flag() {
        // The script of the uninstall summary: `s/.*="//; s/"$//; s|\$HOME|<HOME>|`, 27
        // characters before the replacement. These positions are what GNU sed 4.9 prints.
        let kept =
            |home: &str| parse_replacement(home.as_bytes(), 1, 27, b"|").unwrap_err().message();
        assert_eq!(kept("/a|b"), "sed: -e expression #1, char 31: unknown option to `s'");
        assert_eq!(kept("|"), "sed: -e expression #1, char 29: unknown option to `s'");
        assert_eq!(kept("/a|g|b"), "sed: -e expression #1, char 32: unknown option to `s'");
        assert_eq!(kept("/a|gb"), "sed: -e expression #1, char 32: unknown option to `s'");
    }

    #[test]
    fn a_backslash_before_a_newline_keeps_the_newline() {
        assert_eq!(text("a\\\nb"), "Xa\nbY");
    }

    #[test]
    fn case_conversion_follows_gnu_sed() {
        assert_eq!(text(r"a\Ub"), "XaBY");
        assert_eq!(text(r"a\ub\Ec"), "XaBcY");
        assert_eq!(text(r"a\LBc\Ed"), "XabcdY");
        assert_eq!(text(r"a\L\uBCD"), "XaBcdY");
        assert_eq!(text(r"a\U\lBCD"), "XabCDY");
        assert_eq!(text(r"\uabc"), "XAbcY");
        assert_eq!(text(r"a\UbC\lDE"), "XaBCdEY");
        // `\L` after `\u` drops the pending `\u`.
        assert_eq!(text(r"a\u\LBCD"), "XabcdY");
        assert_eq!(text(r"a\U\nb"), "Xa\nBY");
    }

    #[test]
    fn numeric_escapes_read_three_decimal_or_octal_digits_and_two_hex_digits() {
        assert_eq!(text(r"a\x41b"), "XaAbY");
        assert_eq!(text(r"a\x26b"), "Xa&bY");
        assert_eq!(text(r"a\d065b"), "XaAbY");
        assert_eq!(text(r"a\o101b"), "XaAbY");
        assert_eq!(text(r"a\xZb"), "XaxZbY");
        assert_eq!(text(r"a\oAb"), "XaoAbY");
        assert_eq!(text(r"a\d1234"), "Xa{4Y");
        assert_eq!(text(r"a\x4"), "Xa\u{4}Y");
    }

    #[test]
    fn control_escapes_follow_gnu_sed() {
        assert_eq!(text(r"a\cAb"), "Xa\u{1}bY");
        assert_eq!(text(r"ab\c"), "Xab\\Y");
        assert_eq!(text(r"a\c\\b"), "Xa\u{1c}bY");
        assert_eq!(sub(r"a\c\b").unwrap_err().fault, SedFault::RecursiveEscape);
    }

    #[test]
    fn replace_first_changes_one_occurrence_and_replace_all_every_one() {
        let repl = parse_replacement(b"y", 1, 0, b"").unwrap();
        assert_eq!(replace_first(b"xax", b"x", &repl), b"yax");
        assert_eq!(replace_all(b"xax", b"x", &repl), b"yay");
    }

    #[test]
    fn a_later_substitution_sees_the_output_of_an_earlier_one() {
        let first = parse_replacement(b"__B__", 1, 0, b"").unwrap();
        let second = parse_replacement(b"done", 2, 0, b"").unwrap();
        let once = replace_all(b"__A__", b"__A__", &first);
        assert_eq!(replace_all(&once, b"__B__", &second), b"done");
    }
}
