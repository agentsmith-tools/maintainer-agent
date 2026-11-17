//! The few shell behaviours the bash program leans on, as pure functions.
//!
//! Each function says which construct it stands for, so a reader can set it
//! beside `bin/maintainer-repo`.

/// `$(...)`: bash drops NUL bytes and every trailing newline.
#[must_use]
pub fn cmd_subst(bytes: &[u8]) -> Vec<u8> {
    let mut kept: Vec<u8> = bytes.iter().copied().filter(|byte| *byte != 0).collect();
    while kept.last() == Some(&b'\n') {
        kept.pop();
    }
    kept
}

/// The lines of a stream, without their newlines. A last line with no newline
/// is a line. An empty stream has no lines.
#[must_use]
pub fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut parts: Vec<&[u8]> = bytes.split(|byte| *byte == b'\n').collect();
    if bytes.ends_with(b"\n") {
        parts.pop();
    }
    parts
}

/// `while read -r line`: the line with leading and trailing IFS whitespace
/// (space and tab) removed.
#[must_use]
pub fn read_trim(line: &[u8]) -> &[u8] {
    let is_ifs = |byte: &u8| *byte == b' ' || *byte == b'\t';
    let start = line.iter().position(|byte| !is_ifs(byte)).unwrap_or(line.len());
    let end = line.iter().rposition(|byte| !is_ifs(byte)).map_or(start, |at| at + 1);
    &line[start..end]
}

/// `read -r -a words <<< "$text"`: only the first line is read, and it is
/// split on runs of space and tab, with leading and trailing ones dropped.
#[must_use]
pub fn read_words(text: &str) -> Vec<String> {
    let first = text.split('\n').next().unwrap_or("");
    first.split([' ', '\t']).filter(|word| !word.is_empty()).map(str::to_string).collect()
}

/// `$(sed -n 's/^KEY=//p' <<<"$out")`: the text after `KEY=` on every line that
/// starts with it, one value per line, trailing newlines dropped.
#[must_use]
pub fn sed_field(out: &[u8], key: &str) -> Vec<u8> {
    let prefix = [key.as_bytes(), b"="].concat();
    let mut printed = Vec::new();
    for line in lines(out) {
        if let Some(rest) = line.strip_prefix(prefix.as_slice()) {
            printed.extend_from_slice(rest);
            printed.push(b'\n');
        }
    }
    cmd_subst(&printed)
}

/// `sed 's/^/    /'`: four spaces before every line. A last line with no newline
/// gets one.
#[must_use]
pub fn indent4(bytes: &[u8]) -> Vec<u8> {
    let mut indented = Vec::new();
    for line in lines(bytes) {
        indented.extend_from_slice(b"    ");
        indented.extend_from_slice(line);
        indented.push(b'\n');
    }
    indented
}

/// `[ -n "$x" ]` and the digits-only test of `case $x in ''|*[!0-9]*)`.
#[must_use]
pub fn is_whole_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// The value of a bash arithmetic operand that is a plain integer: decimal,
/// octal after a leading `0`, or hexadecimal after `0x`, optionally signed and
/// padded with spaces. Anything else is an arithmetic error in bash, which
/// ends the `for ((...))` loop that read it. `None` stands for that error.
#[must_use]
pub fn bash_integer(text: &str) -> Option<i64> {
    let trimmed = text.trim_matches([' ', '\t']);
    let (negative, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    if digits.is_empty() {
        return None;
    }
    let value = if let Some(hex) = digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).ok()?
    } else if digits.len() > 1 && digits.starts_with('0') {
        i64::from_str_radix(&digits[1..], 8).ok()?
    } else {
        if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        digits.parse::<i64>().ok()?
    };
    Some(if negative { -value } else { value })
}

/// The integer `[ A -lt B ]` reads from `B`: decimal only, so `010` is ten,
/// with optional sign and surrounding blanks. `None` is `[`'s "integer
/// expression expected", which makes the test false.
#[must_use]
pub fn decimal_integer(text: &str) -> Option<i64> {
    let trimmed = text.trim_matches([' ', '\t']);
    let digits = trimmed.strip_prefix(['-', '+']).unwrap_or(trimmed);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    trimmed.parse::<i64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_substitution_drops_nul_and_every_trailing_newline() {
        assert_eq!(cmd_subst(b"a\nb\n\n\n"), b"a\nb");
        assert_eq!(cmd_subst(b"a\0b\n"), b"ab");
        assert_eq!(cmd_subst(b"\n\n"), b"");
        assert_eq!(cmd_subst(b" a \n"), b" a ");
    }

    #[test]
    fn lines_keeps_an_unterminated_last_line_and_empty_middle_lines() {
        assert_eq!(lines(b"a\n\nb"), vec![&b"a"[..], b"", b"b"]);
        assert_eq!(lines(b"a\n"), vec![&b"a"[..]]);
        assert!(lines(b"").is_empty());
        assert_eq!(lines(b"\n"), vec![&b""[..]]);
    }

    #[test]
    fn read_trims_space_and_tab_only() {
        assert_eq!(read_trim(b"  feat/x\t"), b"feat/x");
        assert_eq!(read_trim(b"   "), b"");
        assert_eq!(read_trim(b"\x0bx"), b"\x0bx");
    }

    #[test]
    fn read_words_takes_the_first_line_and_splits_on_runs_of_blanks() {
        assert_eq!(read_words("  a/* \t b/*  "), vec!["a/*", "b/*"]);
        assert_eq!(read_words("a\nb"), vec!["a"]);
        assert!(read_words("").is_empty());
        assert!(read_words("   ").is_empty());
    }

    #[test]
    fn a_field_is_every_matching_line_joined_and_trimmed() {
        let out = b"state=ready\nhead=abc\nhead=def\nbranch=\nstate=x\n";
        assert_eq!(sed_field(out, "state"), b"ready\nx");
        assert_eq!(sed_field(out, "head"), b"abc\ndef");
        assert_eq!(sed_field(out, "branch"), b"");
        assert_eq!(sed_field(out, "fork"), b"");
        assert_eq!(sed_field(b"xstate=1\n state=2\n", "state"), b"");
    }

    #[test]
    fn indent4_prefixes_every_line_including_empty_ones() {
        assert_eq!(indent4(b"a\n\nb"), b"    a\n    \n    b\n");
        assert_eq!(indent4(b""), b"");
    }

    #[test]
    fn whole_numbers_are_ascii_digits_only() {
        assert!(is_whole_number("007"));
        assert!(!is_whole_number(""));
        assert!(!is_whole_number("-7"));
        assert!(!is_whole_number("1.5"));
        assert!(!is_whole_number("\u{0663}"));
    }

    #[test]
    fn test_integers_are_decimal_so_a_leading_zero_does_not_make_octal() {
        assert_eq!(decimal_integer("010"), Some(10));
        assert_eq!(decimal_integer(" 3 "), Some(3));
        assert_eq!(decimal_integer("-1"), Some(-1));
        assert_eq!(decimal_integer("0x10"), None);
        assert_eq!(decimal_integer(""), None);
        assert_eq!(decimal_integer("abc"), None);
    }

    #[test]
    fn bash_integers_read_decimal_octal_and_hex() {
        assert_eq!(bash_integer("3"), Some(3));
        assert_eq!(bash_integer(" 3 "), Some(3));
        assert_eq!(bash_integer("010"), Some(8));
        assert_eq!(bash_integer("0x10"), Some(16));
        assert_eq!(bash_integer("-2"), Some(-2));
        assert_eq!(bash_integer("08"), None);
        assert_eq!(bash_integer(""), None);
        assert_eq!(bash_integer("abc"), None);
        assert_eq!(bash_integer("3x"), None);
    }
}
