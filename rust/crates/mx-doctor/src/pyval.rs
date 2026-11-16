//! The parts of Python's `json` and `datetime` the doctor's four `python3 -c`
//! snippets depend on, so the Rust reads a file as Python read it.
//!
//! One gap is documented, not closed (decision P1I-10): Python's `json` accepts
//! the literals `NaN`, `Infinity` and `-Infinity` and lone surrogate escapes,
//! and `serde_json` does not. A file holding one reads as unreadable here, where
//! Python would go on.

use serde_json::Value;
use std::fmt::Write;

/// A Python snippet that would have died with a traceback, or hit the error it
/// catches: the caller decides which of the two it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crash;

/// Parses bytes as `json.load` on a UTF-8 text file would: invalid UTF-8 is an
/// error, as `UnicodeDecodeError` is.
pub fn parse(bytes: &[u8]) -> Result<Value, Crash> {
    let text = std::str::from_utf8(bytes).map_err(|_| Crash)?;
    serde_json::from_str(text).map_err(|_| Crash)
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
/// epoch, for the forms GitHub and `date` produce: `YYYY-MM-DD` and `YYYYMMDD`
/// dates, a `T` or space and `HH:MM[:SS[.f]]` (or `HHMM[SS[.f]]`), then a zone
/// offset. A text with no offset is naive, and subtracting it from an aware
/// time is a `TypeError`, so it is `None`, as is anything else Python rejects.
#[must_use]
pub fn iso_epoch(text: &str) -> Option<f64> {
    let text = text.replace('Z', "+00:00");
    let b = text.as_bytes();
    let digits = |s: &[u8]| -> Option<i64> {
        (!s.is_empty() && s.iter().all(u8::is_ascii_digit))
            .then(|| std::str::from_utf8(s).ok()?.parse().ok())
            .flatten()
    };
    let (year, month, day, mut at) =
        if b.len() >= 10 && b.get(4) == Some(&b'-') && b.get(7) == Some(&b'-') {
            (digits(&b[0..4])?, digits(&b[5..7])?, digits(&b[8..10])?, 10)
        } else if b.len() >= 8 {
            (digits(&b[0..4])?, digits(&b[4..6])?, digits(&b[6..8])?, 8)
        } else {
            return None;
        };
    if b.len() <= at {
        return None;
    }
    at += 1; // any one character separates date and time
    let rest = &text[at..];
    let split = rest.find(['+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(split);
    if zone.is_empty() {
        return None;
    }
    let cb = clock.as_bytes();
    let (hh, mm, ss, frac) = if cb.get(2) == Some(&b':') {
        let hh = digits(cb.get(0..2)?)?;
        let mm = digits(cb.get(3..5)?)?;
        let (ss, frac) = if cb.len() > 5 {
            if cb.get(5) != Some(&b':') {
                return None;
            }
            let sec = digits(cb.get(6..8)?)?;
            let frac = match cb.get(8..) {
                Some([]) | None => 0.0,
                Some([b'.', f @ ..]) if !f.is_empty() && f.iter().all(u8::is_ascii_digit) => {
                    format!("0.{}", std::str::from_utf8(f).ok()?).parse::<f64>().ok()?
                }
                _ => return None,
            };
            (sec, frac)
        } else {
            (0, 0.0)
        };
        (hh, mm, ss, frac)
    } else {
        let hh = digits(cb.get(0..2)?)?;
        let mm = digits(cb.get(2..4)?)?;
        let (ss, frac) = if cb.len() > 4 {
            let sec = digits(cb.get(4..6)?)?;
            let frac = match cb.get(6..) {
                Some([]) | None => 0.0,
                Some([b'.', f @ ..]) if !f.is_empty() && f.iter().all(u8::is_ascii_digit) => {
                    format!("0.{}", std::str::from_utf8(f).ok()?).parse::<f64>().ok()?
                }
                _ => return None,
            };
            (sec, frac)
        } else {
            (0, 0.0)
        };
        (hh, mm, ss, frac)
    };
    let zb = zone.as_bytes();
    let sign = if zb[0] == b'-' { -1 } else { 1 };
    let zdigits: Vec<u8> = zone[1..].bytes().filter(|c| *c != b':').collect();
    let (zh, zm) = match zdigits.len() {
        2 => (digits(&zdigits)?, 0),
        4 => (digits(&zdigits[0..2])?, digits(&zdigits[2..4])?),
        _ => return None,
    };
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hh > 23
        || mm > 59
        || ss > 59
        || zh > 23
        || zm > 59
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss - sign * (zh * 3600 + zm * 60);
    #[allow(clippy::cast_precision_loss)]
    Some(secs as f64 + frac)
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
