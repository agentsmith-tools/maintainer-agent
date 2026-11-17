//! The parts of Python's `json` and `datetime` the doctor's four `python3 -c`
//! snippets depend on, so the Rust reads a file as Python read it.
//!
//! Decision P1I-5. One gap is documented, not closed: Python's `json` accepts lone
//! surrogate escapes, and `serde_json` does not, so a file holding one reads as
//! unreadable here where Python would go on. The words `NaN`, `Infinity` and
//! `-Infinity` are read as Python reads them.

use serde_json::Value;
use std::fmt::Write;

/// A Python snippet that would have died with a traceback, or hit the error it
/// catches: the caller decides which of the two it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crash;

/// Parses bytes as `json.load` on a UTF-8 text file would: invalid UTF-8 is an
/// error, as `UnicodeDecodeError` is. Python also reads the bare words `NaN`,
/// `Infinity` and `-Infinity`; they are rewritten before parsing, `NaN` to the
/// string [`NAN_MARK`] and the infinities to `1e999` and `-1e999`, which every
/// reader here treats as Python does.
pub fn parse(bytes: &[u8]) -> Result<Value, Crash> {
    let text = std::str::from_utf8(bytes).map_err(|_| Crash)?;
    match serde_json::from_str(text) {
        Ok(value) => Ok(value),
        Err(_) if text.contains("NaN") || text.contains("Infinity") => {
            serde_json::from_str(&rewrite_special_numbers(text)).map_err(|_| Crash)
        }
        Err(_) => Err(Crash),
    }
}

/// What the JSON word `NaN` is read as.
pub const NAN_MARK: &str = "\u{0}NaN";

fn rewrite_special_numbers(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    let mut in_string = false;
    while at < bytes.len() {
        let byte = bytes[at];
        if in_string {
            let ch = text[at..].chars().next().unwrap_or(' ');
            out.push(ch);
            at += ch.len_utf8();
            if ch == '\\' {
                if let Some(next) = text[at..].chars().next() {
                    out.push(next);
                    at += next.len_utf8();
                }
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        let rest = &text[at..];
        if rest.starts_with("NaN") {
            out.push_str("\"\\u0000NaN\"");
            at += 3;
        } else if rest.starts_with("Infinity") {
            out.push_str("1e999");
            at += 8;
        } else {
            if byte == b'"' {
                in_string = true;
            }
            // Multi-byte characters only occur inside strings, handled above, or
            // are errors the parser reports; copy the whole character.
            let ch = rest.chars().next().unwrap_or(' ');
            out.push(ch);
            at += ch.len_utf8();
        }
    }
    out
}

/// `len(x)` of a JSON value: a list, a dict or a string. Anything else has no length.
#[must_use]
pub fn py_len(value: &Value) -> Option<usize> {
    match value {
        Value::Array(items) => Some(items.len()),
        Value::Object(map) => Some(map.len()),
        Value::String(text) => Some(text.chars().count()),
        _ => None,
    }
}

/// `value[k0][k1]...` for dict keys. A list, string or number indexed by a
/// string is a `TypeError`, a missing key a `KeyError`: both are `None`.
#[must_use]
pub fn index<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    let mut at = value;
    for key in keys {
        at = at.as_object()?.get(*key)?;
    }
    Some(at)
}

/// Python truthiness of a JSON value.
#[must_use]
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// What `print(value)` writes for a JSON value, without the newline.
#[must_use]
pub fn py_str(value: &Value) -> String {
    match value {
        Value::String(text) if text == NAN_MARK => "nan".to_string(),
        Value::String(text) => text.clone(),
        other => py_repr(other),
    }
}

/// `repr(value)` for what `json.load` returns.
#[must_use]
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(number) => number_repr(&number.to_string()),
        Value::String(text) => str_repr(text),
        Value::Array(items) => {
            format!("[{}]", items.iter().map(py_repr).collect::<Vec<_>>().join(", "))
        }
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(key, item)| format!("{}: {}", str_repr(key), py_repr(item)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn number_repr(text: &str) -> String {
    if !text.contains(['.', 'e', 'E']) {
        let digits = text.trim_start_matches('-');
        if digits.bytes().all(|b| b == b'0') {
            return "0".to_string();
        }
        return text.to_string();
    }
    let Ok(value) = text.parse::<f64>() else { return text.to_string() };
    float_repr(value)
}

/// Python's `repr(float)`: the shortest digits that round-trip, positional
/// notation for 1e-4 <= |x| < 1e16 and an exponent outside it.
#[must_use]
pub fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    if value == 0.0 {
        return if value.is_sign_negative() { "-0.0" } else { "0.0" }.to_string();
    }
    let sci = format!("{:e}", value.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if value < 0.0 { "-" } else { "" };
    let decpt = exp + 1;
    let body = if (-3..=16).contains(&decpt) {
        if decpt <= 0 {
            format!("0.{}{digits}", "0".repeat(usize::try_from(-decpt).unwrap_or(0)))
        } else {
            let at = usize::try_from(decpt).unwrap_or(0);
            if digits.len() <= at {
                format!("{digits}{}.0", "0".repeat(at - digits.len()))
            } else {
                format!("{}.{}", &digits[..at], &digits[at..])
            }
        }
    } else {
        let rest = &digits[1..];
        let frac = if rest.is_empty() { String::new() } else { format!(".{rest}") };
        format!("{}{frac}e{}{:02}", &digits[..1], if exp < 0 { '-' } else { '+' }, exp.abs())
    };
    format!("{sign}{body}")
}

fn str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `datetime.fromisoformat(text.replace("Z", "+00:00"))` as seconds since the
/// epoch (Python 3.11 and later). A text with no zone offset is naive, and
/// subtracting it from an aware time is a `TypeError`, so it is `None`, as is
/// anything else Python rejects. Accepted forms: dates `YYYY-MM-DD`, `YYYYMMDD`,
/// `YYYY-Www-D`, `YYYYWwwD` and `YYYY-Www`; one separator character; times `HH`,
/// `HH:MM`, `HH:MM:SS`, `HHMM`, `HHMMSS` with a `.` or `,` fraction; `24:00:00`
/// as the start of the next day; offsets `+HH`, `+HHMM`, `+HH:MM`, `+HH:MM:SS`
/// (and without colons) with a fraction on the seconds.
#[must_use]
pub fn iso_epoch(text: &str) -> Option<f64> {
    let text = text.replace('Z', "+00:00");
    if !text.is_ascii() {
        return None;
    }
    let b = text.as_bytes();
    let num = |s: &[u8]| -> Option<i64> {
        (!s.is_empty() && s.iter().all(u8::is_ascii_digit))
            .then(|| std::str::from_utf8(s).ok()?.parse().ok())
            .flatten()
    };
    let (days, date_len) = parse_date(b, &num)?;
    let rest = text.get(date_len + 1..)?;
    if b.len() <= date_len {
        return None;
    }
    let split = rest.find(['+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(split);
    if zone.is_empty() {
        return None;
    }
    let (hh, mm, ss, frac) = parse_clock(clock.as_bytes(), &num)?;
    let (sign, off_secs, off_frac) = parse_zone(zone.as_bytes(), &num)?;
    let (days, hh) = if hh == 24 {
        if mm != 0 || ss != 0 || frac != 0.0 {
            return None;
        }
        (days + 1, 0)
    } else {
        (days, hh)
    };
    if hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss - sign * off_secs;
    #[allow(clippy::cast_precision_loss)]
    Some(secs as f64 + frac - (sign as f64) * off_frac)
}

type Num<'a> = &'a dyn Fn(&[u8]) -> Option<i64>;

/// A date at the start of `b`: days since the epoch and the characters used.
fn parse_date(b: &[u8], num: Num<'_>) -> Option<(i64, usize)> {
    let year = num(b.get(0..4)?)?;
    if year < 1 {
        return None;
    }
    // Week dates.
    let (week_at, extended) = if b.get(4) == Some(&b'-') { (5, true) } else { (4, false) };
    if b.get(week_at) == Some(&b'W') {
        let week = num(b.get(week_at + 1..week_at + 3)?)?;
        let mut len = week_at + 3;
        let mut dow = 1;
        let day_at = if extended { len + 1 } else { len };
        let has_day = if extended {
            b.get(len) == Some(&b'-')
        } else {
            b.len() > len && b[len].is_ascii_digit()
        };
        if has_day {
            dow = num(b.get(day_at..day_at + 1)?)?;
            len = day_at + 1;
        }
        if !(1..=7).contains(&dow) {
            return None;
        }
        let jan4 = days_from_civil(year, 1, 4);
        let monday = jan4 - (jan4 + 3).rem_euclid(7);
        let days = monday + (week - 1) * 7 + (dow - 1);
        // week 53 only exists in a long year
        if !(1..=53).contains(&week) || (week == 53 && civil_year(days + 3 - (dow - 1)) != year) {
            return None;
        }
        return Some((days, len));
    }
    let (month, day, len) = if extended {
        if b.get(7) != Some(&b'-') {
            return None;
        }
        (num(b.get(5..7)?)?, num(b.get(8..10)?)?, 10)
    } else {
        (num(b.get(4..6)?)?, num(b.get(6..8)?)?, 8)
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let last = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 28 + i64::from(leap),
        _ => return None,
    };
    if !(1..=last).contains(&day) {
        return None;
    }
    Some((days_from_civil(year, month, day), len))
}

/// The civil year a day count falls in.
fn civil_year(days: i64) -> i64 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let year = yoe + era * 400;
    if mp >= 10 { year + 1 } else { year }
}

/// `HH`, `HH:MM`, `HH:MM:SS[.f]`, `HHMM`, `HHMMSS[.f]`: hours, minutes, seconds, fraction.
fn parse_clock(c: &[u8], num: Num<'_>) -> Option<(i64, i64, i64, f64)> {
    let hh = num(c.get(0..2)?)?;
    let extended = c.get(2) == Some(&b':');
    let mut at = 2;
    let mut mm = 0;
    let mut ss = 0;
    if extended || c.len() > 2 {
        at = if extended { 3 } else { 2 };
        mm = num(c.get(at..at + 2)?)?;
        at += 2;
        if c.len() > at {
            if extended {
                if c.get(at) != Some(&b':') {
                    return None;
                }
                at += 1;
            }
            ss = num(c.get(at..at + 2)?)?;
            at += 2;
        }
    }
    let frac = match c.get(at..) {
        Some([]) | None => 0.0,
        Some([b'.' | b',', f @ ..])
            if !f.is_empty() && f.iter().all(u8::is_ascii_digit) && ss_present(c, at) =>
        {
            format!("0.{}", std::str::from_utf8(&f[..f.len().min(6)]).ok()?).parse::<f64>().ok()?
        }
        _ => return None,
    };
    Some((hh, mm, ss, frac))
}

/// A fraction is only valid after seconds.
fn ss_present(c: &[u8], at: usize) -> bool {
    at >= 6 || (at == 5 && c.get(2) != Some(&b':'))
}

/// `+HH[[:]MM[[:]SS[.f]]]`: the sign, the offset in whole seconds, and its fraction.
fn parse_zone(z: &[u8], num: Num<'_>) -> Option<(i64, i64, f64)> {
    let sign = if z.first() == Some(&b'-') { -1 } else { 1 };
    let body: Vec<u8> = z[1..].iter().copied().filter(|ch| *ch != b':').collect();
    let (digits, frac) = match body.iter().position(|ch| *ch == b'.' || *ch == b',') {
        Some(p) => (&body[..p], Some(&body[p + 1..])),
        None => (&body[..], None),
    };
    let (zh, zm, zs) = match digits.len() {
        2 => (num(digits)?, 0, 0),
        4 => (num(&digits[0..2])?, num(&digits[2..4])?, 0),
        6 => (num(&digits[0..2])?, num(&digits[2..4])?, num(&digits[4..6])?),
        _ => return None,
    };
    let frac = match frac {
        Some(f) if digits.len() == 6 && !f.is_empty() && f.iter().all(u8::is_ascii_digit) => {
            format!("0.{}", std::str::from_utf8(&f[..f.len().min(6)]).ok()?).parse::<f64>().ok()?
        }
        Some(_) => return None,
        None => 0.0,
    };
    if zh > 23 || zm > 59 || zs > 59 {
        return None;
    }
    Some((sign, zh * 3600 + zm * 60 + zs, frac))
}

/// Days since 1970-01-01 for a civil date (proleptic Gregorian).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
