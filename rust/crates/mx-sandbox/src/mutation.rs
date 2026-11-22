//! The mutation a verify applies to the extracted tree.

use crate::SandboxError;
use mx_suite::MutateGlob;

/// The fourth argument of `verify`: the expression `sed --sandbox -i` applies.
///
/// `--sandbox` makes GNU sed refuse the `e`, `w` and `r` commands, so the
/// expression is a pure text transform and not host code execution. That is the
/// only protection in the bash: an expression that BEGINS with `-` is read by sed
/// as an option (`-n`, `-f<file>`, `--follow-symlinks`), because the bash gate puts
/// it right after `-i` without `-e` or `--`. This type refuses it (decision P30-11,
/// rv264 O8), together with what bash refuses (an empty expression,
/// `${4:?sed expression}`) and what no argument can hold (NUL). The adapter must
/// still pass `-e <expression> --`, as the bash fix will.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SedExpression(String);

impl SedExpression {
    /// Checks the expression.
    pub fn parse(text: &str) -> Result<Self, SandboxError> {
        if text.is_empty() || text.contains('\0') || text.starts_with('-') {
            return Err(SandboxError::BadSedExpression);
        }
        Ok(Self(text.to_string()))
    }

    /// The expression, one argument for `sed`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The mutation landed: at least one file changed.
///
/// `apply_mutation` prints the paths whose hash changed, `./` stripped. A sed
/// that matches no line, or that names a file the suite's mutate glob does not
/// list, leaves the tree as it was, and the mutated run then passes for the
/// gate's own reason. The bash read that as THE GUARD DOES NOT BITE, a verdict
/// about the contributor's test from a mutation that never happened. This type
/// cannot hold an empty list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mutated {
    paths: Vec<String>,
}

/// The mutation changed nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NothingChanged {
    /// The globs the suite mutates, for the message.
    pub globs: Vec<String>,
}

impl NothingChanged {
    /// The refusal the bash `cmd_verify` dies with, for pull request `pr`.
    ///
    /// The globs are listed on one line separated by a space. The bash prints
    /// `$(suite_mutate_glob | tr '\n' ' ')`, which ends in a space when the suite
    /// printed its last glob with a newline and not when it did not; the list here
    /// has none (decision P30-12).
    #[must_use]
    pub fn message(&self, pr: &str) -> String {
        format!(
            "the mutation changed nothing, so this run says nothing about the guard.\n\n  sed matched no line in any file this suite mutates:\n    {}\n  Either the expression matches nothing, or the file it targets is outside that\n  list. Re-run with a mutation that lands inside it. This is not a verdict about\n  #{pr}.",
            self.globs.join(" ")
        )
    }
}

impl Mutated {
    /// The paths the mutation changed, from [`crate::Sandbox::apply_mutation`].
    pub fn new(paths: Vec<String>, globs: &[MutateGlob]) -> Result<Self, NothingChanged> {
        if paths.is_empty() {
            return Err(NothingChanged {
                globs: globs.iter().map(|glob| glob.as_str().to_string()).collect(),
            });
        }
        Ok(Self { paths })
    }

    /// The changed paths, `./` stripped, in the order sed visited them.
    #[must_use]
    pub fn paths(&self) -> &[String] {
        &self.paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_substitution_is_accepted_as_typed() {
        for text in ["s/a/b/", "s/if x/if !x/g", "/guard/d", "s/\\(a\\)/\\1x/;s/b/c/"] {
            assert_eq!(SedExpression::parse(text).unwrap().as_str(), text);
        }
    }

    #[test]
    fn an_empty_expression_is_refused() {
        assert_eq!(SedExpression::parse(""), Err(SandboxError::BadSedExpression));
    }

    #[test]
    fn an_expression_with_a_nul_byte_is_refused() {
        assert_eq!(SedExpression::parse("s/a/b\0/"), Err(SandboxError::BadSedExpression));
    }

    #[test]
    fn an_expression_that_starts_with_a_hyphen_is_refused() {
        // P30-11, rv264 O8: sed reads it as an option (`-f<script>` ran a host script).
        for bad in ["-n", "--follow-symlinks", "-fscript", "-e s/a/b/"] {
            assert_eq!(SedExpression::parse(bad), Err(SandboxError::BadSedExpression), "{bad}");
        }
        assert!(SedExpression::parse("s/-a/-b/").is_ok());
    }

    fn globs(list: &[&str]) -> Vec<MutateGlob> {
        list.iter().map(|g| MutateGlob::parse(g).unwrap()).collect()
    }

    #[test]
    fn a_mutation_that_changed_files_keeps_their_paths_in_order() {
        let mutated =
            Mutated::new(vec!["src/b.rs".into(), "src/a.rs".into()], &globs(&["*.rs"])).unwrap();
        assert_eq!(mutated.paths(), ["src/b.rs", "src/a.rs"]);
    }

    #[test]
    fn a_mutation_that_changed_nothing_is_refused_with_the_globs() {
        let refused = Mutated::new(Vec::new(), &globs(&["*.sh", "*.py"])).unwrap_err();
        assert_eq!(refused, NothingChanged { globs: vec!["*.sh".into(), "*.py".into()] });
    }

    #[test]
    fn the_refusal_reads_as_the_sentence_of_cmd_verify() {
        // bash, cmd_verify:
        //   die "the mutation changed nothing, so this run says nothing about the guard.
        //
        //     sed matched no line in any file this suite mutates:
        //       $(suite_mutate_glob | tr '\n' ' ')
        //     Either the expression matches nothing, or the file it targets is outside that
        //     list. Re-run with a mutation that lands inside it. This is not a verdict about
        //     #$pr."
        let refused = NothingChanged { globs: vec!["*.sh".into(), "*.py".into()] };
        assert_eq!(
            refused.message("7"),
            "the mutation changed nothing, so this run says nothing about the guard.\n\n  sed matched no line in any file this suite mutates:\n    *.sh *.py\n  Either the expression matches nothing, or the file it targets is outside that\n  list. Re-run with a mutation that lands inside it. This is not a verdict about\n  #7."
        );
    }
}
