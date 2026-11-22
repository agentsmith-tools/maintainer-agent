//! The command line of `maintainer-merge`: the usage line, the arguments of each subcommand and the
//! exit codes (bash lines 2230 to 2243 and the `${N:?name}` reads at the top of each `cmd_*`).

use crate::{GateDecision, HeadArg, PrNumber, UpdateRefusal, VerifyRefusal};
use mx_sandbox::SandboxError;
use mx_suite::{SuiteName, SuiteNameError, TestFilter, TestFilterError};
use thiserror::Error;

/// The line the bash prints on standard error for no subcommand or an unknown one, exit 64.
pub const USAGE: &str = "usage: maintainer-merge {verify <pr> <sha> <test-filter> <sed-mutation> [suite]|verify-deps <pr> <sha>|verify-part <pr> <sha> <test-filter> <sed-mutation> <suite>|receipt <pr> <sha> <proof>|merge <pr>|update <pr>|show [pr]}";

/// One parsed subcommand. Every number, head, filter, expression and suite name is checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `receipt <pr> <sha> <proof>`.
    Receipt { pr: PrNumber, sha: HeadArg, proof: String },
    /// `verify <pr> <sha> <filter> <mutation> [suite]`, and `verify-part` with `part` true
    /// (`VERIFY_PART=1`). `verify-part` without a suite parses: the bash refuses it later, with
    /// [`VerifyRefusal::PartNeedsSuiteNamed`], after the lease and the runtime are checked.
    Verify {
        part: bool,
        pr: PrNumber,
        sha: HeadArg,
        filter: TestFilter,
        mutation: mx_sandbox::SedExpression,
        suite: Option<SuiteName>,
    },
    /// `verify-deps <pr> <sha>`.
    VerifyDeps { pr: PrNumber, sha: HeadArg },
    /// `merge <pr>`.
    Merge { pr: PrNumber },
    /// `update <pr>`.
    Update { pr: PrNumber },
    /// `show [pr]`.
    Show { pr: Option<PrNumber> },
}

/// Why the arguments are not a command.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CommandError {
    /// No subcommand, or one the gate does not know, or one with the wrong case. Exit 64.
    #[error("{USAGE}")]
    Usage,
    /// An argument the bash reads with `${N:?name}` is missing or empty. Bash itself prints
    /// `maintainer-merge: line <n>: <position>: <name>` and exits 1; the line number is the bash
    /// source's and has no meaning here, so the binary prints [`CommandError::Missing`]'s text
    /// after `maintainer-merge: `.
    #[error("{position}: {name}")]
    Missing { position: u8, name: &'static str },
    /// The pull request number or the head argument is refused.
    #[error("{0}")]
    Refused(GateDecision),
    /// The test filter is refused (P30-9).
    #[error("{0}")]
    Filter(TestFilterError),
    /// The sed expression is refused.
    #[error("{0}")]
    Mutation(SandboxError),
    /// The suite name is refused (F3).
    #[error("{0}")]
    Suite(SuiteNameError),
}

impl CommandError {
    /// 64 for the usage line, else 1.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage => 64,
            Self::Missing { .. }
            | Self::Refused(_)
            | Self::Filter(_)
            | Self::Mutation(_)
            | Self::Suite(_) => 1,
        }
    }
}

/// How a run ends: the exit code and what goes to standard error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    pub code: i32,
    pub stderr: String,
}

impl Exit {
    /// What `die` does: `maintainer-merge: <message>` and a newline, exit `code`.
    #[must_use]
    pub fn die(code: i32, message: &str) -> Self {
        Self { code, stderr: format!("maintainer-merge: {message}\n") }
    }

    /// The usage line, exit 64. It has no `maintainer-merge:` prefix.
    #[must_use]
    pub fn usage() -> Self {
        Self { code: 64, stderr: format!("{USAGE}\n") }
    }
}

impl From<&GateDecision> for Exit {
    fn from(decision: &GateDecision) -> Self {
        Self::die(decision.exit_code(), &decision.to_string())
    }
}

impl From<&UpdateRefusal> for Exit {
    fn from(refusal: &UpdateRefusal) -> Self {
        Self::die(refusal.exit_code(), &refusal.to_string())
    }
}

impl From<&VerifyRefusal> for Exit {
    fn from(refusal: &VerifyRefusal) -> Self {
        Self::die(refusal.exit_code(), &refusal.to_string())
    }
}

impl From<&CommandError> for Exit {
    fn from(error: &CommandError) -> Self {
        match error {
            CommandError::Usage => Self::usage(),
            other => Self::die(other.exit_code(), &other.to_string()),
        }
    }
}

/// The argument at `position` (1 for the first after the subcommand), or the `${N:?name}` refusal.
fn need<'a>(args: &'a [String], position: u8, name: &'static str) -> Result<&'a str, CommandError> {
    match args.get(usize::from(position) - 1) {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(CommandError::Missing { position, name }),
    }
}

fn pr_of(text: &str) -> Result<PrNumber, CommandError> {
    PrNumber::parse(text).map_err(CommandError::Refused)
}

fn sha_of(text: &str) -> Result<HeadArg, CommandError> {
    HeadArg::parse(text).map_err(CommandError::Refused)
}

/// Reads `args`, the arguments after the program name.
///
/// The bash reads every `${N:?name}` of a subcommand first, in order, so the first missing one is
/// reported; then it validates the number. The head, filter, expression and suite are checked
/// later in the bash (after the lease and the container runtime), and here they are checked in one
/// pass after the number, so a command with two faults reports the number first and then the head,
/// the filter, the expression and the suite, where the bash may report the lease first.
pub fn parse_command(args: &[String]) -> Result<Command, CommandError> {
    let subcommand = args.first().map(String::as_str).ok_or(CommandError::Usage)?;
    // arguments after the subcommand are positions 1.. of `rest`
    let rest = &args[1..];
    match subcommand {
        "receipt" => {
            let (pr, sha, proof) = (
                need(rest, 1, "pr")?,
                need(rest, 2, "head sha")?,
                need(rest, 3, "what was proved")?,
            );
            Ok(Command::Receipt { pr: pr_of(pr)?, sha: sha_of(sha)?, proof: proof.to_string() })
        }
        "verify" | "verify-part" => {
            let pr = need(rest, 1, "pr")?;
            let sha = need(rest, 2, "head sha")?;
            let filter = need(rest, 3, "test filter")?;
            let mutation = need(rest, 4, "sed expression")?;
            let suite = rest.get(4).filter(|s| !s.is_empty());
            Ok(Command::Verify {
                part: subcommand == "verify-part",
                pr: pr_of(pr)?,
                sha: sha_of(sha)?,
                filter: TestFilter::parse(filter).map_err(CommandError::Filter)?,
                mutation: mx_sandbox::SedExpression::parse(mutation)
                    .map_err(CommandError::Mutation)?,
                suite: suite
                    .map(|name| SuiteName::parse(name).map_err(CommandError::Suite))
                    .transpose()?,
            })
        }
        "verify-deps" => {
            let pr = need(rest, 1, "pr")?;
            let sha = need(rest, 2, "head sha")?;
            Ok(Command::VerifyDeps { pr: pr_of(pr)?, sha: sha_of(sha)? })
        }
        "merge" => Ok(Command::Merge { pr: pr_of(need(rest, 1, "pr")?)? }),
        "update" => Ok(Command::Update { pr: pr_of(need(rest, 1, "pr")?)? }),
        "show" => Ok(Command::Show { pr: rest.first().map(|text| pr_of(text)).transpose()? }),
        _ => Err(CommandError::Usage),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| (*a).to_string()).collect()
    }

    #[test]
    fn no_subcommand_an_unknown_one_and_a_wrong_case_print_the_usage_and_exit_64() {
        // gate-usage.sh: none, frobnicate, "", --help and Merge are all the usage line.
        for list in [&[][..], &["frobnicate"], &[""], &["--help"], &["Merge", "7"]] {
            let error = parse_command(&args(list)).unwrap_err();
            assert_eq!(error, CommandError::Usage, "{list:?}");
            assert_eq!(error.exit_code(), 64, "{list:?}");
            assert_eq!(Exit::from(&error), Exit { code: 64, stderr: format!("{USAGE}\n") });
        }
    }

    #[test]
    fn the_usage_line_names_every_subcommand() {
        for word in [
            "verify <pr>",
            "verify-deps <pr>",
            "verify-part <pr>",
            "receipt <pr>",
            "merge <pr>",
            "update <pr>",
            "show [pr]",
        ] {
            assert!(USAGE.contains(word), "{word}");
        }
    }

    #[test]
    fn each_missing_argument_names_its_position_and_exits_1() {
        // The table of gate-usage.sh.
        let cases: &[(&[&str], &str)] = &[
            (&["verify"], "1: pr"),
            (&["verify", "7"], "2: head sha"),
            (&["verify", "7", "1234567"], "3: test filter"),
            (&["verify", "7", "1234567", "t"], "4: sed expression"),
            (&["verify-part"], "1: pr"),
            (&["verify-part", "7", "1234567", "t"], "4: sed expression"),
            (&["verify-deps"], "1: pr"),
            (&["verify-deps", "7"], "2: head sha"),
            (&["receipt"], "1: pr"),
            (&["receipt", "7"], "2: head sha"),
            (&["receipt", "7", "1234567"], "3: what was proved"),
            (&["merge"], "1: pr"),
            (&["update"], "1: pr"),
        ];
        for (list, want) in cases {
            let error = parse_command(&args(list)).unwrap_err();
            assert_eq!(error.to_string(), *want, "{list:?}");
            assert_eq!(error.exit_code(), 1, "{list:?}");
        }
    }

    #[test]
    fn an_empty_argument_counts_as_missing_as_in_the_bash_colon_question_form() {
        assert_eq!(parse_command(&args(&["merge", ""])).unwrap_err().to_string(), "1: pr");
    }

    #[test]
    fn a_pull_request_number_that_is_not_one_is_refused_before_anything_else() {
        for list in [
            &["merge", "abc"][..],
            &["update", "abc"],
            &["receipt", "abc", "1234567", "proof"],
            &["verify-deps", "abc", "1234567"],
            &["verify", "abc", "1234567", "t", "s/a/b/"],
            &["verify-part", "abc", "1234567", "t", "s/a/b/", "suite"],
            &["show", "../x"],
        ] {
            let error = parse_command(&args(list)).unwrap_err();
            assert!(
                matches!(error, CommandError::Refused(GateDecision::InvalidPrNumber { .. })),
                "{list:?}: {error:?}"
            );
            assert_eq!(error.exit_code(), 1);
        }
    }

    #[test]
    fn a_verify_parses_its_five_arguments_into_typed_values() {
        let command =
            parse_command(&args(&["verify", "7", "0123456", "guard_bites", "s/a/b/", "rust"]))
                .unwrap();
        let Command::Verify { part, pr, sha, filter, mutation, suite } = command else { panic!() };
        assert!(!part);
        assert_eq!(
            (pr.as_arg(), sha.as_str(), filter.as_str(), mutation.as_str()),
            ("7", "0123456", "guard_bites", "s/a/b/")
        );
        assert_eq!(suite.unwrap().as_str(), "rust");
    }

    #[test]
    fn verify_part_sets_the_part_flag_and_may_lack_its_suite_until_the_bash_refuses_it_later() {
        let Command::Verify { part, suite, .. } =
            parse_command(&args(&["verify-part", "7", "0123456", "t", "s/a/b/"])).unwrap()
        else {
            panic!()
        };
        assert!(part);
        assert_eq!(suite, None);
    }

    #[test]
    fn a_suite_name_from_the_command_line_is_validated() {
        let error = parse_command(&args(&["verify", "7", "0123456", "t", "s/a/b/", "../../x"]))
            .unwrap_err();
        assert!(matches!(error, CommandError::Suite(_)), "{error:?}");
    }

    #[test]
    fn a_filter_with_a_metacharacter_and_a_dash_expression_are_refused() {
        assert!(matches!(
            parse_command(&args(&["verify", "7", "0123456", "x; echo", "s/a/b/"])).unwrap_err(),
            CommandError::Filter(_)
        ));
        assert!(matches!(
            parse_command(&args(&["verify", "7", "0123456", "x", "-n"])).unwrap_err(),
            CommandError::Mutation(_)
        ));
    }

    #[test]
    fn a_truncated_head_is_refused() {
        let error = parse_command(&args(&["receipt", "7", "abc", "proof"])).unwrap_err();
        assert_eq!(error.to_string(), "head sha looks truncated: abc");
    }

    #[test]
    fn merge_update_show_and_verify_deps_parse() {
        assert_eq!(
            parse_command(&args(&["merge", "7"])).unwrap(),
            Command::Merge { pr: PrNumber::parse("7").unwrap() }
        );
        assert_eq!(
            parse_command(&args(&["update", "7"])).unwrap(),
            Command::Update { pr: PrNumber::parse("7").unwrap() }
        );
        assert_eq!(parse_command(&args(&["show"])).unwrap(), Command::Show { pr: None });
        assert_eq!(
            parse_command(&args(&["show", "7"])).unwrap(),
            Command::Show { pr: Some(PrNumber::parse("7").unwrap()) }
        );
        assert!(matches!(
            parse_command(&args(&["verify-deps", "7", "0123456"])).unwrap(),
            Command::VerifyDeps { .. }
        ));
    }

    #[test]
    fn a_refusal_becomes_the_die_line_and_its_exit_code() {
        let pr = PrNumber::parse("7").unwrap();
        let exit = Exit::from(&GateDecision::Dirty { pr: pr.clone() });
        assert_eq!(
            exit,
            Exit {
                code: 1,
                stderr: "maintainer-merge: #7 has conflicts (mergeStateStatus DIRTY)\n".into()
            }
        );
        let needs_author = UpdateRefusal::NeedsAuthorNoEdits { pr };
        let exit = Exit::from(&needs_author);
        assert_eq!(exit.code, 3);
        assert_eq!(
            exit.stderr,
            "maintainer-merge: will not update #7: the author must press Update branch; they have not allowed maintainer edits\n"
        );
    }
}
