//! The small pieces of shell behaviour the five scripts lean on, as pure functions.
//!
//! Each one is the Rust spelling of one shell construct, and each says which. They work
//! on bytes: a file name or a figure may hold anything, and the scripts never decoded it.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

/// `dirname`: everything before the last component. No slash gives `.`, and a path whose
/// only slash leads gives `/`. Trailing slashes are ignored, as GNU `dirname` ignores them.
#[must_use]
pub fn dirname(path: &[u8]) -> Vec<u8> {
    let mut end = path.len();
    while end > 1 && path[end - 1] == b'/' {
        end -= 1;
    }
    let trimmed = &path[..end];
    match trimmed.iter().rposition(|byte| *byte == b'/') {
        None => b".".to_vec(),
        Some(slash) => {
            let mut keep = slash;
            while keep > 0 && trimmed[keep - 1] == b'/' {
                keep -= 1;
            }
            if keep == 0 { b"/".to_vec() } else { trimmed[..keep].to_vec() }
        }
    }
}

/// `basename PATH`: the last component, trailing slashes dropped. An all-slash path gives `/`.
#[must_use]
pub fn basename(path: &[u8]) -> Vec<u8> {
    let mut end = path.len();
    while end > 1 && path[end - 1] == b'/' {
        end -= 1;
    }
    let trimmed = &path[..end];
    match trimmed.iter().rposition(|byte| *byte == b'/') {
        Some(slash) if trimmed.len() > 1 => trimmed[slash + 1..].to_vec(),
        _ => trimmed.to_vec(),
    }
}

/// `cd DIR/.. && pwd` without `-P`: the directory above `dir`, with `.` and `..` resolved by
/// the text of the path and no link followed. A relative `dir` is taken from `cwd`.
#[must_use]
pub fn logical_parent(dir: &Path, cwd: &Path) -> PathBuf {
    let joined = if dir.is_absolute() { dir.to_path_buf() } else { cwd.join(dir) };
    let mut parts: Vec<&OsStr> = Vec::new();
    for component in joined.components() {
        match component {
            Component::RootDir | Component::Prefix(_) | Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(name) => parts.push(name),
        }
    }
    parts.pop();
    let mut out = PathBuf::from("/");
    for part in parts {
        out.push(part);
    }
    out
}

/// What `$(cmd)` hands back: the bytes without trailing newlines and without NUL bytes
/// (bash drops them and warns).
#[must_use]
pub fn command_substitution(bytes: &[u8]) -> Vec<u8> {
    let mut text: Vec<u8> = bytes.iter().copied().filter(|byte| *byte != 0).collect();
    while text.last() == Some(&b'\n') {
        text.pop();
    }
    text
}

/// The lines of `bytes` as `read -r` or `sed` sees them: split at newlines, and a final
/// piece with no newline after it is a line too when `keep_unterminated` is true. `read`
/// returns failure for that last piece, so a `while read` loop never runs its body for it.
#[must_use]
pub fn lines(bytes: &[u8], keep_unterminated: bool) -> Vec<&[u8]> {
    let mut out: Vec<&[u8]> = bytes.split(|byte| *byte == b'\n').collect();
    let last = out.pop();
    if let Some(tail) = last
        && keep_unterminated
        && !tail.is_empty()
    {
        out.push(tail);
    }
    out
}

/// A byte of the default `IFS`: space, tab or newline.
#[must_use]
pub fn is_ifs(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n')
}

/// `for f in $words`: the text cut at the default `IFS`, with no empty word.
#[must_use]
pub fn split_words(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split(|byte| is_ifs(*byte)).filter(|word| !word.is_empty()).collect()
}

/// A byte `[[:space:]]` matches in the C locale.
#[must_use]
pub fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `sed 's/[[:space:]]*$//'`.
#[must_use]
pub fn trim_trailing_space(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && is_space(line[end - 1]) {
        end -= 1;
    }
    &line[..end]
}

/// Whether `needle` occurs in `hay`. An empty needle occurs everywhere.
#[must_use]
pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|window| window == needle)
}

/// `grep -F -i` for a needle that holds no newline: an ASCII case-insensitive substring search.
#[must_use]
pub fn contains_ignore_case(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|window| window.eq_ignore_ascii_case(needle))
}

/// The number `[ A -ge B ]` reads: optional surrounding white space, an optional sign, digits.
/// `None` when the shell would print "integer expected" (and for a value past
/// 64 bits, where it complains the same way).
#[must_use]
pub fn legal_number(text: &[u8]) -> Option<i64> {
    let mut start = 0;
    let mut end = text.len();
    while start < end && matches!(text[start], b' ' | b'\t' | b'\n') {
        start += 1;
    }
    while end > start && matches!(text[end - 1], b' ' | b'\t' | b'\n') {
        end -= 1;
    }
    let body = &text[start..end];
    let (negative, digits) = match body.first() {
        Some(b'-') => (true, &body[1..]),
        Some(b'+') => (false, &body[1..]),
        _ => (false, body),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut value: i64 = 0;
    for digit in digits {
        value = value.checked_mul(10)?.checked_add(i64::from(digit - b'0'))?;
    }
    if negative { value.checked_neg() } else { Some(value) }
}

/// `cut -d/ -f1,2`: the first two fields, or the whole line when it holds no slash.
#[must_use]
pub fn first_two_fields(line: &[u8]) -> Vec<u8> {
    if !line.contains(&b'/') {
        return line.to_vec();
    }
    let mut fields = line.split(|byte| *byte == b'/');
    let first = fields.next().unwrap_or_default();
    let mut out = first.to_vec();
    if let Some(second) = fields.next() {
        out.push(b'/');
        out.extend_from_slice(second);
    }
    out
}

/// The message the shell prints when it cannot find a program: `SCRIPT: line N: NAME: command not found`.
#[must_use]
pub fn command_not_found(script: &OsStr, line: u32, name: &str) -> Vec<u8> {
    let mut text = script.as_bytes().to_vec();
    text.extend_from_slice(format!(": line {line}: {name}: command not found\n").as_bytes());
    text
}

/// A path as the shell prints it in a message: the bytes, unchanged.
#[must_use]
pub fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}

/// A `printf '%-46s'`-style pad: the text, then spaces up to `width` bytes. Text longer than
/// the width is left whole.
#[must_use]
pub fn pad_right(text: &[u8], width: usize) -> Vec<u8> {
    let mut out = text.to_vec();
    while out.len() < width {
        out.push(b' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirname_of_a_bare_name_is_dot() {
        assert_eq!(dirname(b"run.sh"), b".");
    }

    #[test]
    fn dirname_drops_the_last_component_and_trailing_slashes() {
        assert_eq!(dirname(b"/a/b/c.sh"), b"/a/b");
        assert_eq!(dirname(b"/a/b/"), b"/a");
        assert_eq!(dirname(b"a//b"), b"a");
        assert_eq!(dirname(b"/x"), b"/");
        assert_eq!(dirname(b"//"), b"/");
    }

    #[test]
    fn basename_takes_the_last_component() {
        assert_eq!(basename(b"/a/b/deny.json"), b"deny.json");
        assert_eq!(basename(b"a/b/"), b"b");
        assert_eq!(basename(b"x"), b"x");
        assert_eq!(basename(b"/"), b"/");
    }

    #[test]
    fn logical_parent_resolves_dots_by_text_and_follows_no_link() {
        assert_eq!(logical_parent(Path::new("/t/scripts"), Path::new("/w")), Path::new("/t"));
        assert_eq!(logical_parent(Path::new("scripts"), Path::new("/w/x")), Path::new("/w/x"));
        assert_eq!(
            logical_parent(Path::new("../t/./scripts"), Path::new("/w/x")),
            Path::new("/w/t")
        );
        assert_eq!(logical_parent(Path::new("/"), Path::new("/w")), Path::new("/"));
    }

    #[test]
    fn command_substitution_strips_trailing_newlines_and_nul_bytes() {
        assert_eq!(command_substitution(b"a\n\n"), b"a");
        assert_eq!(command_substitution(b"a\0b\n"), b"ab");
        assert_eq!(command_substitution(b"\n\nx"), b"\n\nx");
    }

    #[test]
    fn lines_drops_an_unterminated_last_line_when_asked() {
        assert_eq!(lines(b"a\nb", false), vec![&b"a"[..]]);
        assert_eq!(lines(b"a\nb", true), vec![&b"a"[..], &b"b"[..]]);
        assert_eq!(lines(b"a\n", true), vec![&b"a"[..]]);
        assert_eq!(lines(b"a\n\nb\n", false), vec![&b"a"[..], &b""[..], &b"b"[..]]);
    }

    #[test]
    fn split_words_cuts_at_space_tab_and_newline_only() {
        assert_eq!(split_words(b"a b\tc\nd  e"), vec![&b"a"[..], b"b", b"c", b"d", b"e"]);
        assert_eq!(split_words(b"a\rb"), vec![&b"a\rb"[..]]);
    }

    #[test]
    fn trim_trailing_space_strips_every_c_space_byte() {
        assert_eq!(trim_trailing_space(b"x \t\r\x0b\x0c"), b"x");
        assert_eq!(trim_trailing_space(b"  "), b"");
        assert_eq!(trim_trailing_space(b"x y"), b"x y");
    }

    #[test]
    fn contains_ignore_case_folds_ascii_only() {
        assert!(contains_ignore_case(b"Data, not Instruction", b"data, NOT instruction"));
        assert!(!contains_ignore_case(b"data. not", b"data, not"));
        assert!(contains_ignore_case(b"x", b""));
    }

    #[test]
    fn legal_number_accepts_what_test_accepts() {
        assert_eq!(legal_number(b"12"), Some(12));
        assert_eq!(legal_number(b" 12 "), Some(12));
        assert_eq!(legal_number(b"-3"), Some(-3));
        assert_eq!(legal_number(b"+3"), Some(3));
        assert_eq!(legal_number(b"008"), Some(8));
        assert_eq!(legal_number(b""), None);
        assert_eq!(legal_number(b"abc"), None);
        assert_eq!(legal_number(b"1 2"), None);
        assert_eq!(legal_number(b"99999999999999999999"), None);
    }

    #[test]
    fn first_two_fields_matches_cut() {
        assert_eq!(first_two_fields(b"actions/checkout"), b"actions/checkout");
        assert_eq!(first_two_fields(b"github/codeql-action/analyze"), b"github/codeql-action");
        assert_eq!(first_two_fields(b"solo"), b"solo");
        assert_eq!(first_two_fields(b"/x/y"), b"/x");
        assert_eq!(first_two_fields(b"a//b"), b"a/");
    }

    #[test]
    fn command_not_found_names_script_line_and_program() {
        let text = command_not_found(OsStr::new("scripts/x.sh"), 12, "git");
        assert_eq!(text, b"scripts/x.sh: line 12: git: command not found\n");
    }

    #[test]
    fn pad_right_pads_to_the_width_and_never_cuts() {
        assert_eq!(pad_right(b"ab", 5), b"ab   ");
        assert_eq!(pad_right(b"abcdef", 3), b"abcdef");
    }
}
