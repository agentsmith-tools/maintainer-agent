//! Python's `os.path.realpath`, as the bash gate's `tree_escapes` uses it.
//!
//! `tree_escapes` is a `python3` heredoc: it takes `os.path.realpath` of the tree and of every
//! symlink in it and compares them. The port does the same comparison in Rust, so the
//! resolution has to be the same one, step for step, or a link the bash calls inside would be
//! called outside here (or the reverse). This is the algorithm of `CPython` 3.14's `posixpath`
//! (`strict=False`), which the differential test runs against the machine's own `python3`:
//!
//! - the path is split on `/` into parts, resolved from the left;
//! - `.` and empty parts are skipped, `..` removes the last part of what is resolved so far
//!   (never going above `/`), with no look at the disk;
//! - a part that is a symlink is replaced by its target's parts, and an absolute target
//!   restarts from `/`; a link met again while it is still being resolved is a loop and stays
//!   as the link's own path;
//! - a part that does not exist (or cannot be read) is kept as written.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

enum Item {
    Part(Vec<u8>),
    /// The link at this path has been resolved to what `path` holds now.
    Resolved(Vec<u8>),
}

/// The canonical path of `input`, which must be absolute.
///
/// `input` is always absolute in this crate, so the working directory is never consulted.
#[must_use]
pub fn realpath(input: &Path) -> PathBuf {
    let bytes = input.as_os_str().as_bytes();
    let mut rest: Vec<Item> =
        bytes.split(|b| *b == b'/').rev().map(|p| Item::Part(p.to_vec())).collect();
    let mut path: Vec<u8> = b"/".to_vec();
    // The cache of resolved links; a `None` value is a link still being resolved.
    let mut seen: HashMap<Vec<u8>, Option<Vec<u8>>> = HashMap::new();
    while let Some(item) = rest.pop() {
        let name = match item {
            Item::Resolved(link) => {
                seen.insert(link, Some(path.clone()));
                continue;
            }
            Item::Part(name) => name,
        };
        if name.is_empty() || name == b"." {
            continue;
        }
        if name == b".." {
            let cut = path.iter().rposition(|b| *b == b'/').unwrap_or(0);
            path.truncate(cut);
            if path.is_empty() {
                path.push(b'/');
            }
            continue;
        }
        let mut new_path = path.clone();
        if new_path != b"/" {
            new_path.push(b'/');
        }
        new_path.extend_from_slice(&name);
        let as_path = PathBuf::from(OsStr::from_bytes(&new_path));
        let Ok(meta) = std::fs::symlink_metadata(&as_path) else {
            path = new_path;
            continue;
        };
        if !meta.file_type().is_symlink() {
            path = new_path;
            continue;
        }
        if let Some(cached) = seen.get(&new_path) {
            match cached {
                Some(done) => path.clone_from(done),
                // Not resolved yet, so this is a loop: the link stays as it is.
                None => path = new_path,
            }
            continue;
        }
        let Ok(target) = std::fs::read_link(&as_path) else {
            path = new_path;
            continue;
        };
        let target = target.into_os_string().into_vec();
        if target.first() == Some(&b'/') {
            path = b"/".to_vec();
        }
        seen.insert(new_path.clone(), None);
        rest.push(Item::Resolved(new_path));
        rest.extend(target.split(|b| *b == b'/').rev().map(|p| Item::Part(p.to_vec())));
    }
    PathBuf::from(OsStr::from_bytes(&path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mx-realpath-{name}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn a_plain_path_is_returned_with_dots_and_double_slashes_removed() {
        assert_eq!(realpath(Path::new("/a//b/./c/../d")), PathBuf::from("/a/b/d"));
        assert_eq!(realpath(Path::new("/../..")), PathBuf::from("/"));
    }

    #[test]
    fn a_link_to_a_sibling_resolves_through_the_link() {
        let root = scratch("sibling");
        std::fs::create_dir(root.join("d")).unwrap();
        symlink("d", root.join("l")).unwrap();
        assert_eq!(realpath(&root.join("l/x")), root.join("d/x"));
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn an_absolute_target_restarts_from_the_root() {
        let root = scratch("absolute");
        symlink("/etc", root.join("l")).unwrap();
        assert_eq!(realpath(&root.join("l/passwd")), PathBuf::from("/etc/passwd"));
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn a_dangling_link_with_dot_dot_is_resolved_without_the_disk() {
        let root = scratch("dangling");
        symlink("../../outside", root.join("l")).unwrap();
        let parent = root.parent().unwrap().parent().unwrap().join("outside");
        assert_eq!(realpath(&root.join("l")), parent);
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn a_link_loop_stays_the_path_of_the_link() {
        let root = scratch("loop");
        symlink("b", root.join("a")).unwrap();
        symlink("a", root.join("b")).unwrap();
        // Python 3.14 answers the link that closes the loop, as written.
        let got = realpath(&root.join("a"));
        assert!(got == root.join("a") || got == root.join("b"), "{got:?}");
        drop(std::fs::remove_dir_all(&root));
    }
}
