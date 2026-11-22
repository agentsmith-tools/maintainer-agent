//! A basename glob a suite mutates.

use thiserror::Error;

/// Why a line is not a mutate glob.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MutateGlobError {
    /// An empty glob would match no file name.
    #[error("a mutate glob cannot be empty")]
    Empty,
}

/// One line of `suite_mutate_glob`: a pattern the mutation step matches against
/// the base name of each file (`find -name`), so `*.rs` reaches every Rust file
/// at any depth and a pattern with a `/` matches nothing.
///
/// `apply_mutation` skips an empty line (`[ -n "$g" ] || continue`). The host
/// does the same when it reads the suite's output, so a value of this type is
/// never empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutateGlob(String);

impl MutateGlob {
    /// Checks one glob.
    pub fn parse(text: &str) -> Result<Self, MutateGlobError> {
        if text.is_empty() {
            return Err(MutateGlobError::Empty);
        }
        Ok(Self(text.to_string()))
    }

    /// The pattern.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_globs_of_the_shipped_suites_are_accepted() {
        for glob in ["*.rs", "*.sh", "*.py", "maintainer*", "sysknife-*", "Makefile", "*.yml"] {
            assert_eq!(MutateGlob::parse(glob).unwrap().as_str(), glob);
        }
    }

    #[test]
    fn an_empty_glob_is_refused() {
        assert_eq!(MutateGlob::parse(""), Err(MutateGlobError::Empty));
    }
}
