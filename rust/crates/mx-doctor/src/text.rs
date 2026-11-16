//! Small pieces of shell and Python text semantics that `bin/maintainer-doctor`
//! relies on, each one named for the construct it reproduces.

/// `wc -l`: the number of newline bytes.
#[must_use]
pub fn newlines(bytes: &[u8]) -> usize {
    bytes.split(|byte| *byte == b'\n').count() - 1
}

/// What `$(...)` does to captured output: removes every trailing newline.
#[must_use]
pub fn strip_trailing_newlines(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == b'\n' {
        end -= 1;
    }
    &bytes[..end]
}

/// `$(...)` as text. Bytes that are not UTF-8 become U+FFFD: a path or a
/// message with such bytes is the one place the port differs (decision P1I-9).
#[must_use]
pub fn captured(bytes: &[u8]) -> String {
    String::from_utf8_lossy(strip_trailing_newlines(bytes)).into_owned()
}

/// The words an unquoted `$VAR` becomes: split on spaces, tabs and newlines.
/// Glob characters are not expanded (decision P1I-8).
#[must_use]
pub fn ifs_words(text: &str) -> Vec<&str> {
    text.split([' ', '\t', '\n']).filter(|word| !word.is_empty()).collect()
}

/// The operand of `[ A -gt B ]` as bash reads it: optional blanks, an optional
/// sign, digits, optional blanks. `None` is what bash reports as
/// `integer expected` (bash 5.3; bash before 5.3 says `integer expression expected`).
#[must_use]
pub fn test_int(text: &str) -> Option<i64> {
    let trimmed = text.trim_matches([' ', '\t']);
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    trimmed.parse::<i64>().ok()
}

/// The text bash writes to standard error when `[` gets a non-number.
#[must_use]
pub fn integer_expected(line: u32, operand: &str) -> String {
    format!("maintainer-doctor: line {line}: [: {operand}: integer expected\n")
}

/// A basic regular expression made of literal characters and `.`, searched for
/// anywhere in a line (or at its start). `.` is how a profile name with a dot in
/// it reaches `grep -c "maintainer@$profile-"`: it matches any one byte.
#[must_use]
pub fn line_has(line: &[u8], pattern: &str, anchored: bool) -> bool {
    let pat = pattern.as_bytes();
    if pat.len() > line.len() {
        return false;
    }
    let fits =
        |at: usize| pat.iter().enumerate().all(|(i, want)| *want == b'.' || line[at + i] == *want);
    if anchored { fits(0) } else { (0..=line.len() - pat.len()).any(fits) }
}

/// `grep -c PATTERN` over the lines of `text`.
#[must_use]
pub fn count_lines_with(text: &[u8], pattern: &str, anchored: bool) -> usize {
    lines(text).filter(|line| line_has(line, pattern, anchored)).count()
}

/// The lines of a byte string, as `grep` and `read` see them: split on `\n`,
/// with no empty last line after a final newline.
pub fn lines(text: &[u8]) -> impl Iterator<Item = &[u8]> {
    let body = text.strip_suffix(b"\n").unwrap_or(text);
    let none = text.is_empty();
    body.split(|byte| *byte == b'\n').filter(move |_| !none)
}

/// `grep -qE '^\S+ +[0-9]+s '` on one line: a timer row whose next elapse is
/// a whole number of seconds away.
#[must_use]
pub fn fires_in_seconds(line: &[u8]) -> bool {
    let not_space = |byte: u8| !matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r');
    let token = line.iter().take_while(|byte| not_space(**byte)).count();
    if token == 0 {
        return false;
    }
    let rest = &line[token..];
    let spaces = rest.iter().take_while(|byte| **byte == b' ').count();
    if spaces == 0 {
        return false;
    }
    let rest = &rest[spaces..];
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    digits > 0 && rest.get(digits) == Some(&b's') && rest.get(digits + 1) == Some(&b' ')
}

/// `IFS=$'\t' read -r _ w age url` on one line: tab is a whitespace character of
/// IFS, so runs of tabs are one separator and leading and trailing tabs are
/// dropped. The last variable takes the rest of the line, minus trailing tabs.
#[must_use]
pub fn read_tab_fields(line: &str, vars: usize) -> Vec<String> {
    let mut fields = Vec::with_capacity(vars);
    let mut rest = line.trim_start_matches('\t');
    while fields.len() + 1 < vars && !rest.is_empty() {
        if let Some(at) = rest.find('\t') {
            fields.push(rest[..at].to_string());
            rest = rest[at..].trim_start_matches('\t');
        } else {
            fields.push(rest.to_string());
            rest = "";
        }
    }
    if fields.len() + 1 == vars {
        fields.push(rest.trim_end_matches('\t').to_string());
    }
    while fields.len() < vars {
        fields.push(String::new());
    }
    fields
}

/// `tr '/.' '--'`.
#[must_use]
pub fn dashes_for_slash_and_dot(text: &str) -> String {
    text.chars().map(|ch| if ch == '/' || ch == '.' { '-' } else { ch }).collect()
}

/// The first `n` lines of `text`, newline-terminated as `head -n` prints them,
/// with each newline turned into a space as `tr '\n' ' '` does.
#[must_use]
pub fn head_lines_spaced(text: &str, n: usize) -> String {
    let mut out = String::new();
    let mut seen = 0;
    for ch in text.chars() {
        if ch == '\n' {
            out.push(' ');
            seen += 1;
            if seen == n {
                break;
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// `"%.0f" % x` in Python and `printf '%.0f'`: round half to even.
#[must_use]
pub fn round0(value: f64) -> String {
    format!("{value:.0}")
}

/// Python's `float(text)`: surrounding whitespace, a sign, digits with single
/// underscores between them, a fraction, an exponent, or `inf`, `infinity`, `nan`.
#[must_use]
pub fn py_float(text: &str) -> Option<f64> {
    let t = text.trim_matches(|ch: char| ch.is_ascii_whitespace() || ch == '\u{b}');
    let (sign, body) = match t.as_bytes().first() {
        Some(b'-') => (-1.0, &t[1..]),
        Some(b'+') => (1.0, &t[1..]),
        _ => (1.0, t),
    };
    let lower = body.to_ascii_lowercase();
    match lower.as_str() {
        "inf" | "infinity" => return Some(sign * f64::INFINITY),
        "nan" => return Some(f64::NAN),
        _ => {}
    }
    let (mantissa, exponent) = match lower.find('e') {
        Some(at) => (&lower[..at], Some(&lower[at + 1..])),
        None => (lower.as_str(), None),
    };
    let (whole, fraction) = match mantissa.find('.') {
        Some(at) => (&mantissa[..at], Some(&mantissa[at + 1..])),
        None => (mantissa, None),
    };
    let whole_digits = underscored_digits(whole)?;
    let fraction_digits = match fraction {
        Some(part) => underscored_digits(part)?,
        None => String::new(),
    };
    if whole_digits.is_empty() && fraction_digits.is_empty() {
        return None;
    }
    let mut clean = String::new();
    clean.push_str(if whole_digits.is_empty() { "0" } else { &whole_digits });
    if !fraction_digits.is_empty() {
        clean.push('.');
        clean.push_str(&fraction_digits);
    }
    if let Some(exp) = exponent {
        let (esign, edigits) = match exp.as_bytes().first() {
            Some(b'-') => ("-", &exp[1..]),
            Some(b'+') => ("+", &exp[1..]),
            _ => ("", exp),
        };
        let digits = underscored_digits(edigits)?;
        if digits.is_empty() {
            return None;
        }
        clean.push('e');
        clean.push_str(esign);
        clean.push_str(&digits);
    }
    clean.parse::<f64>().ok().map(|value| sign * value)
}

/// Python's `int(text)` for a string: whitespace, a sign, digits with single
/// underscores between them. The value is returned as `f64` because the doctor
/// only subtracts it from the clock.
#[must_use]
pub fn py_int_text(text: &str) -> Option<f64> {
    let t = text.trim_matches(|ch: char| ch.is_ascii_whitespace() || ch == '\u{b}');
    let (sign, body) = match t.as_bytes().first() {
        Some(b'-') => (-1.0, &t[1..]),
        Some(b'+') => (1.0, &t[1..]),
        _ => (1.0, t),
    };
    let digits = underscored_digits(body)?;
    if digits.is_empty() {
        return None;
    }
    digits.parse::<f64>().ok().map(|value| sign * value)
}

/// Digits with single underscores between digits, underscores removed. `None`
/// when the text has anything else, or a misplaced underscore.
fn underscored_digits(part: &str) -> Option<String> {
    let bytes = part.as_bytes();
    let mut out = String::new();
    for (i, byte) in bytes.iter().enumerate() {
        match byte {
            b'0'..=b'9' => out.push(char::from(*byte)),
            b'_' => {
                let between = i > 0
                    && i + 1 < bytes.len()
                    && bytes[i - 1].is_ascii_digit()
                    && bytes[i + 1].is_ascii_digit();
                if !between {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(out)
}
