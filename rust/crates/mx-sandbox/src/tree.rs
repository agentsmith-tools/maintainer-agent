//! The tree a verify unpacks, and the proof that it stays inside itself.

use crate::SandboxError;
use mx_suite::text::indent4;
use std::fmt;
use std::path::{Path, PathBuf};

/// A tree that arrived whole: every file the revision tracks is on disk.
///
/// `git archive | tar -x` threw tar's status away, and under the timer unit's
/// seccomp filter not one file arrived while the run read the empty tree as
/// success (the comment above `extract_tree`). The count is the difference
/// between a tree that arrived and one that merely did not raise an error, so
/// the only way to an `ExtractedTree` is [`ExtractedTree::counted`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedTree {
    root: PathBuf,
}

impl ExtractedTree {
    /// Compares what the revision lists (`git ls-tree -r -z`, submodules
    /// left out) with what `find` counted on disk (files and symlinks).
    ///
    /// A revision that lists no file is refused, and so is any count that is not
    /// equal, in either direction.
    pub fn counted(
        root: PathBuf,
        rev: &str,
        listed: u64,
        found: u64,
    ) -> Result<Self, SandboxError> {
        if listed == 0 {
            return Err(SandboxError::NoFiles { rev: rev.to_string() });
        }
        if found != listed {
            return Err(SandboxError::PartialExtract {
                rev: rev.to_string(),
                want: listed,
                got: found,
            });
        }
        Ok(Self { root })
    }

    /// The directory the tree was written into.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// One symlink whose target resolves outside the tree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Escape {
    /// The link, relative to the tree's root.
    pub link: String,
    /// What `readlink` says, as written.
    pub target: String,
}

impl fmt::Display for Escape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} -> {}", self.link, self.target)
    }
}

/// The tree has links that leave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscapingTree {
    pub escapes: Vec<Escape>,
}

impl EscapingTree {
    /// The refusal the bash `cmd_verify` dies with, for pull request `pr`.
    #[must_use]
    pub fn message(&self, pr: &str) -> String {
        let links: Vec<String> = self.escapes.iter().map(ToString::to_string).collect();
        format!(
            "the tree for #{pr} contains symlink(s) pointing outside itself, and the\n  mutation step runs on the host:\n{}\n  Refusing. Review this by reading.",
            indent4(&links.join("\n"))
        )
    }
}

/// A tree with no symlink that resolves outside it. Only this type reaches the
/// mutation step, which runs `sed -i` on the HOST: a tracked link
/// `harness.sh -> ~/.ssh/id_ed25519` would be dereferenced and its target written
/// into the tree the container then reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainedTree {
    root: PathBuf,
}

impl ContainedTree {
    /// Takes the tree and what [`crate::Sandbox::tree_escapes`] found in it.
    /// The tree passes only when the list is empty.
    pub fn check(tree: ExtractedTree, escapes: Vec<Escape>) -> Result<Self, EscapingTree> {
        if escapes.is_empty() {
            Ok(Self { root: tree.root })
        } else {
            Err(EscapingTree { escapes })
        }
    }

    /// The directory of the tree.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counted(listed: u64, found: u64) -> Result<ExtractedTree, SandboxError> {
        ExtractedTree::counted(PathBuf::from("/w"), "v-7", listed, found)
    }

    #[test]
    fn a_tree_whose_count_matches_its_listing_arrived() {
        let tree = counted(120, 120).unwrap();
        assert_eq!(tree.root(), Path::new("/w"));
    }

    #[test]
    fn a_partial_extraction_is_refused_with_both_counts() {
        let refused = counted(120, 80).unwrap_err();
        assert_eq!(refused, SandboxError::PartialExtract { rev: "v-7".into(), want: 120, got: 80 });
        // bash: printf 'extracted %s of the %s files in %s\n' "$((got))" "$want" "$ref"
        assert_eq!(refused.to_string(), "extracted 80 of the 120 files in v-7");
    }

    #[test]
    fn a_tree_with_extra_files_is_refused_too() {
        assert_eq!(
            counted(10, 11),
            Err(SandboxError::PartialExtract { rev: "v-7".into(), want: 10, got: 11 })
        );
    }

    #[test]
    fn a_revision_that_lists_no_file_is_refused_even_when_nothing_arrived() {
        let refused = counted(0, 0).unwrap_err();
        assert_eq!(refused, SandboxError::NoFiles { rev: "v-7".into() });
        // bash: printf '%s lists no files\n' "$ref"
        assert_eq!(refused.to_string(), "v-7 lists no files");
    }

    fn tree() -> ExtractedTree {
        counted(3, 3).unwrap()
    }

    fn escape(link: &str, target: &str) -> Escape {
        Escape { link: link.to_string(), target: target.to_string() }
    }

    #[test]
    fn a_tree_with_no_escaping_link_is_contained() {
        let contained = ContainedTree::check(tree(), Vec::new()).unwrap();
        assert_eq!(contained.root(), Path::new("/w"));
    }

    #[test]
    fn a_tree_with_an_escaping_link_is_refused_and_keeps_the_list() {
        let escapes = vec![escape("harness.sh", "/home/u/.ssh/id_ed25519")];
        let refused = ContainedTree::check(tree(), escapes.clone()).unwrap_err();
        assert_eq!(refused.escapes, escapes);
    }

    #[test]
    fn one_escaping_link_among_many_refuses_the_tree() {
        let refused = ContainedTree::check(tree(), vec![escape("a", "../x")]);
        assert!(refused.is_err());
    }

    #[test]
    fn the_refusal_reads_as_the_sentence_of_cmd_verify() {
        // bash, cmd_verify:
        //   die "the tree for #$pr contains symlink(s) pointing outside itself, and the
        //     mutation step runs on the host:
        //   $(printf '%s\n' "$links" | sed 's/^/    /')
        //     Refusing. Review this by reading."
        let refused = EscapingTree {
            escapes: vec![escape("a.sh", "/etc/passwd"), escape("docs/b", "../../c")],
        };
        assert_eq!(
            refused.message("7"),
            "the tree for #7 contains symlink(s) pointing outside itself, and the\n  mutation step runs on the host:\n    a.sh -> /etc/passwd\n    docs/b -> ../../c\n  Refusing. Review this by reading."
        );
    }

    #[test]
    fn an_escape_prints_as_link_arrow_target() {
        assert_eq!(escape("x", "../y").to_string(), "x -> ../y");
    }
}
