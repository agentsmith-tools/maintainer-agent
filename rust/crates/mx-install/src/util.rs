//! Small text and path helpers that stand for shell constructs.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// The bytes of the parts, one after the other.
#[must_use]
pub fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// The bytes of an OS string.
#[must_use]
pub fn raw(text: &OsStr) -> &[u8] {
    text.as_bytes()
}

/// An OS string from bytes.
#[must_use]
pub fn os(bytes: &[u8]) -> OsString {
    OsString::from_vec(bytes.to_vec())
}

/// `"$a$b"`: two pieces of a path joined as text, with no separator added and none
/// removed, as the shell joins `"$HOME/.local"`.
#[must_use]
pub fn concat(base: &OsStr, rest: &str) -> PathBuf {
    let mut text = base.as_bytes().to_vec();
    text.extend_from_slice(rest.as_bytes());
    PathBuf::from(OsString::from_vec(text))
}

/// `basename PATH` for a path with a file name: its last component, as bytes.
#[must_use]
pub fn basename(path: &Path) -> OsString {
    path.file_name().map_or_else(|| path.as_os_str().to_os_string(), OsStr::to_os_string)
}

/// `$(basename "$(dirname "$p")")`: the name of the directory a file sits in.
#[must_use]
pub fn parent_name(path: &Path) -> OsString {
    path.parent().map_or_else(OsString::new, basename)
}

/// Paths in the order bash expands a glob under `LC_ALL=C`: by the bytes of the whole path.
pub fn sort_c(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| a.as_os_str().as_bytes().cmp(b.as_os_str().as_bytes()));
}

/// `${name%-*}`: the text up to the last hyphen, or all of it when there is none.
#[must_use]
pub fn before_last_hyphen(name: &[u8]) -> &[u8] {
    name.iter().rposition(|byte| *byte == b'-').map_or(name, |at| &name[..at])
}

/// The words `for x in $text` visits: runs of space, tab and newline separate them.
#[must_use]
pub fn words(text: &[u8]) -> Vec<&[u8]> {
    text.split(|byte| matches!(byte, b' ' | b'\t' | b'\n'))
        .filter(|word| !word.is_empty())
        .collect()
}

/// Lower-case hex of some bytes.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
        // Writing to a String cannot fail.
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// `$(...)` drops the newlines at the end of what a command printed.
#[must_use]
pub fn strip_trailing_newlines(mut bytes: Vec<u8>) -> Vec<u8> {
    while bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_before_the_last_hyphen_is_the_profile_of_a_hyphenated_repository() {
        assert_eq!(before_last_hyphen(b"my-repo-review"), b"my-repo");
        assert_eq!(before_last_hyphen(b"solo"), b"solo");
        assert_eq!(before_last_hyphen(b"-x"), b"");
    }

    #[test]
    fn words_split_on_blanks_and_drop_empty_ones() {
        assert_eq!(words(b" a  b\tc\nd "), vec![&b"a"[..], b"b", b"c", b"d"]);
        assert!(words(b"").is_empty());
    }

    #[test]
    fn a_glob_sorts_by_the_bytes_of_the_whole_path() {
        let mut paths: Vec<PathBuf> =
            ["/p/a/profile.env", "/p/a-b/profile.env"].iter().map(PathBuf::from).collect();
        sort_c(&mut paths);
        assert_eq!(paths[0], PathBuf::from("/p/a-b/profile.env"));
    }

    #[test]
    fn command_substitution_strips_every_trailing_newline_and_nothing_else() {
        assert_eq!(strip_trailing_newlines(b"a\n\n b\n\n".to_vec()), b"a\n\n b");
    }
}
