//! The two strings of `verify-deps` that go into a GitHub API path.

use crate::deps::DepsRefusal;
use std::fmt;

/// A changed requirements path that is safe to put in
/// `repos/<slug>/contents/<path>?ref=<sha>`.
///
/// bash: `[[ "$pf" =~ ^[A-Za-z0-9._/-]+$ ]] && [[ "$pf" != *..* ]]`, or
/// die "the requirements path '$pf' is not a plain path, so it is not put in an API URL".
/// GitHub normalises a path before it resolves it, so `..` or an escape could
/// reach a different file than the one the classifier read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainPath(String);

impl PlainPath {
    /// Checks the path: ASCII letters, digits, `.`, `_`, `/` and `-`, at least one,
    /// and no `..` anywhere in it.
    pub fn parse(path: &str) -> Result<Self, DepsRefusal> {
        let plain = !path.is_empty()
            && path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
            && !path.contains("..");
        if plain {
            Ok(Self(path.to_string()))
        } else {
            Err(DepsRefusal::NotAPlainPath { path: path.to_string() })
        }
    }

    /// The path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PlainPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The repository an action pin lives in, `owner/repo`, from `owner/repo[/dir]`.
///
/// bash: `repo="${action%%/*}/$(cut -d/ -f2 <<<"$action")"`. An action path is
/// `owner/repo` or `owner/repo/sub/dir`, and the tag belongs to the repository.
/// The classifier already refuses a segment that is not plain (`REF_SEGMENT`,
/// `ACTION_SEGMENT`); this type checks the two it uses again, so the gate does not
/// depend on the helper for what goes into an API path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRepo {
    slug: String,
}

fn is_segment(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl ActionRepo {
    /// The repository of an action path. An action with fewer than two segments,
    /// or whose owner or repository segment is not plain, is refused (the bash
    /// turns `checkout` into `checkout/checkout`).
    pub fn of_action(action: &str) -> Result<Self, DepsRefusal> {
        let mut segments = action.split('/');
        match (segments.next(), segments.next()) {
            (Some(owner), Some(repo)) if is_segment(owner) && is_segment(repo) => {
                Ok(Self { slug: format!("{owner}/{repo}") })
            }
            _ => Err(DepsRefusal::ActionNotOwnerRepo { action: action.to_string() }),
        }
    }

    /// `owner/repo`.
    #[must_use]
    pub fn slug(&self) -> &str {
        &self.slug
    }
}

impl fmt::Display for ActionRepo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.slug)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_requirements_paths_are_plain() {
        for path in ["requirements.txt", "docs/requirements-dev.txt", "a/b_c/d.e-f/req.in", "x"] {
            assert_eq!(PlainPath::parse(path).unwrap().as_str(), path);
        }
    }

    #[test]
    fn a_character_outside_the_class_is_refused() {
        for path in ["a b", "a?ref=1", "a#b", "a%2e%2e/b", "a\nb", "é", "a:b", "a\\b", "a+b", ""] {
            assert_eq!(
                PlainPath::parse(path),
                Err(DepsRefusal::NotAPlainPath { path: path.to_string() }),
                "{path:?}"
            );
        }
    }

    #[test]
    fn a_dot_dot_anywhere_is_refused() {
        for path in ["..", "../x", "a/../b", "a/..", "a..b", "...", "a/b..c/d"] {
            assert_eq!(
                PlainPath::parse(path),
                Err(DepsRefusal::NotAPlainPath { path: path.to_string() }),
                "{path:?}"
            );
        }
    }

    #[test]
    fn a_single_dot_and_a_dotted_name_are_plain() {
        assert!(PlainPath::parse("./req.txt").is_ok());
        assert!(PlainPath::parse("a.b.c/req.txt").is_ok());
    }

    #[test]
    fn the_refusal_reads_as_the_bash_die() {
        // bash: die "the requirements path '$pf' is not a plain path, so it is not put in an API URL"
        assert_eq!(
            PlainPath::parse("../x").unwrap_err().to_string(),
            "the requirements path '../x' is not a plain path, so it is not put in an API URL"
        );
    }

    #[test]
    fn an_action_path_names_its_repository() {
        assert_eq!(ActionRepo::of_action("actions/checkout").unwrap().slug(), "actions/checkout");
        assert_eq!(
            ActionRepo::of_action("github/codeql-action/analyze").unwrap().slug(),
            "github/codeql-action"
        );
        assert_eq!(ActionRepo::of_action("a/b/c/d").unwrap().to_string(), "a/b");
    }

    #[test]
    fn an_action_with_fewer_than_two_plain_segments_is_refused() {
        for action in
            ["checkout", "", "/x", "x/", "a//b", "a/.b", ".a/b", "a/-b", "a b/c", "../x", "a/b c"]
        {
            assert_eq!(
                ActionRepo::of_action(action),
                Err(DepsRefusal::ActionNotOwnerRepo { action: action.to_string() }),
                "{action:?}"
            );
        }
    }
}
