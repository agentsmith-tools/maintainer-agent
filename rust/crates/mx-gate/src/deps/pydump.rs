//! Python's `json.dumps` for the one shape `verify-deps` compares: a sorted list of
//! `(name, version, sorted hashes)` triples.
//!
//! The bash builds both sides of the whole-file comparison with
//! `json.dumps(sorted(...))` and quotes the first 300 characters of each in its
//! refusal, so the text has to match Python's default output byte for byte:
//! `", "` and `": "` separators and `ensure_ascii` escapes.

use std::fmt::Write as _;

/// One requirement whose version moved: name, version and hashes.
pub(crate) type Triple = (String, String, Vec<String>);

/// A string the way `json.dumps` writes it with `ensure_ascii=True`: every character outside
/// space to tilde is escaped, and a character above U+FFFF becomes a surrogate pair.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
    out
}

/// `json.dumps(sorted((name, version, sorted(hashes)) for ...))`.
pub(crate) fn dumps_sorted(items: &[Triple]) -> String {
    let mut rows: Vec<Triple> = items
        .iter()
        .map(|(name, version, hashes)| {
            let mut hashes = hashes.clone();
            hashes.sort();
            (name.clone(), version.clone(), hashes)
        })
        .collect();
    rows.sort();
    let rows: Vec<String> = rows
        .iter()
        .map(|(name, version, hashes)| {
            let hashes: Vec<String> = hashes.iter().map(|h| quote(h)).collect();
            format!("[{}, {}, [{}]]", quote(name), quote(version), hashes.join(", "))
        })
        .collect();
    format!("[{}]", rows.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, version: &str, hashes: &[&str]) -> Triple {
        (name.into(), version.into(), hashes.iter().map(|h| (*h).to_string()).collect())
    }

    #[test]
    fn an_empty_list_is_two_brackets() {
        assert_eq!(dumps_sorted(&[]), "[]");
    }

    #[test]
    fn rows_and_hashes_are_sorted_and_joined_with_python_separators() {
        let rows = [row("b", "2", &["y", "x"]), row("a", "1", &[])];
        assert_eq!(dumps_sorted(&rows), r#"[["a", "1", []], ["b", "2", ["x", "y"]]]"#);
    }

    #[test]
    fn quotes_backslashes_controls_and_non_ascii_are_escaped_as_ensure_ascii_does() {
        let rows = [row("a\"b\\c\n\u{1}\u{7f}é😀", "1", &[])];
        assert_eq!(
            dumps_sorted(&rows),
            "[[\"a\\\"b\\\\c\\n\\u0001\\u007f\\u00e9\\ud83d\\ude00\", \"1\", []]]"
        );
    }
}
