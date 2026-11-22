//! Which suite covers which paths: the coverage rules of the merge gate, as a port.
//!
//! The real implementation (the suite agent, brief `p3a`) answers from a [`crate::SuiteHost`]
//! and the profile's [`ProdGlobs`]: `suites_cover_production` is the check `merge` runs on a
//! receipt, `pick_suite` is the inference `verify` runs, `coverage_map` says which suite would
//! cover each path. Their bash is `bin/maintainer-merge` lines 764 to 1098 (`coverage_map`,
//! `is_production`, `covers_production`, `covers_when_no_production`,
//! `suites_cover_production`, `pick_suite`). The refusals are typed here with the text of
//! the bash, so p3a and the gate agent code against one set of messages.

use crate::{ProdGlobs, SuiteError, SuiteName};
use mx_core::NonEmpty;
use mx_ports::Script;
use std::sync::{Mutex, PoisonError};
use thiserror::Error;

use crate::text::indent4;

/// Why `suites_cover_production` answered no. The bash prints these as `printf ... >&2`
/// lines, then the caller dies with its own sentence (`ReceiptDoesNotCover`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoverageFailure {
    /// The profile declares no `PROD_GLOBS`.
    #[error(
        "this profile declares no PROD_GLOBS, so nothing here can say which\n  paths a receipt has to cover. Refusing rather than answering."
    )]
    NoProdGlobs,
    /// The receipt names no suite.
    #[error("the receipt names no suite, so it speaks for nothing.")]
    NamesNoSuite,
    /// A name in the receipt is not a suite file on disk (or is not a valid name at all).
    #[error("the receipt names a suite that is not on disk: {name}")]
    NotOnDisk { name: String },
    /// Production paths that no named suite runs.
    #[error(
        "no suite this receipt was earned by runs these production path(s):\n{}\n  Naming more suites does not help unless one of them runs the file.",
        indent4(&paths.join("\n"))
    )]
    Uncovered { paths: Vec<String> },
    /// A suite could not be asked.
    #[error("{0}")]
    Suite(SuiteError),
}

impl CoverageFailure {
    /// The lines the bash writes to standard error, each ending in a newline, before
    /// `receipt_covers` dies. The text is indented two spaces, as the `printf`s write it.
    #[must_use]
    pub fn stderr_lines(&self) -> String {
        format!("  {self}\n")
    }
}

/// Why `pick_suite` found no suite.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PickRefusal {
    /// A named suite does not cover every changed path.
    // bash: die "the '$named' suite does not cover every changed path:
    // $(coverage_map "$dir" "$@")
    //   A receipt from a suite that does not run the changed code proves nothing."
    #[error(
        "the '{named}' suite does not cover every changed path:\n{map}\n  A receipt from a suite that does not run the changed code proves nothing."
    )]
    NamedDoesNotCover { named: SuiteName, map: String },
    /// No single suite covers every changed path.
    // bash: die "no single suite in $dir covers every changed path:
    // $map
    //
    //   Suites that would cover the rest between them: ${suites:-none}.
    //   Paths no suite covers at all: $orphans.
    //
    //   A receipt written by a suite that does not run the changed code proves
    //   nothing, so this refuses rather than picking the closest suite. Add a suite
    //   for the uncovered paths, or split the pull request along suite lines."
    #[error(
        "no single suite in {dir} covers every changed path:\n{map}\n\n  Suites that would cover the rest between them: {suites}.\n  Paths no suite covers at all: {orphans}.\n\n  A receipt written by a suite that does not run the changed code proves\n  nothing, so this refuses rather than picking the closest suite. Add a suite\n  for the uncovered paths, or split the pull request along suite lines."
    )]
    NoSingleSuite { dir: String, map: String, suites: String, orphans: u64 },
    /// A suite or the directory could not be read.
    #[error("{0}")]
    Suite(SuiteError),
}

/// The suite `pick_suite` chose, and what it said on standard error while choosing: the lines
/// `covers_production` prints for the paths the suite does not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    pub suite: SuiteName,
    pub notes: String,
}

/// One row of `coverage_map`: `    <path><TAB><suite or "no suite covers this">`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageRow {
    pub path: String,
    pub suite: Option<SuiteName>,
}

impl CoverageRow {
    /// The line the bash prints.
    #[must_use]
    pub fn line(&self) -> String {
        match &self.suite {
            Some(name) => format!("    {}\t{name}", self.path),
            None => format!("    {}\tno suite covers this", self.path),
        }
    }
}

/// The coverage rules.
pub trait SuiteSelector {
    /// `suites_cover_production`: does the union of the named suites cover every production
    /// path of `changed`? `names` are the receipt's suite names as text, because a name in a
    /// receipt may not be a valid [`SuiteName`], and that is [`CoverageFailure::NotOnDisk`].
    fn suites_cover_production(
        &self,
        globs: &ProdGlobs,
        names: &[String],
        changed: &NonEmpty<String>,
    ) -> Result<(), CoverageFailure>;

    /// `pick_suite`: the named suite if it covers the diff, else the first inferable suite
    /// (in byte order of name) that covers every path or every production path.
    fn pick_suite(
        &self,
        globs: &ProdGlobs,
        named: Option<&SuiteName>,
        changed: &NonEmpty<String>,
    ) -> Result<Picked, PickRefusal>;

    /// `coverage_map`: for each path, the first inferable suite that covers it.
    fn coverage_map(&self, changed: &[String]) -> Result<Vec<CoverageRow>, SuiteError>;
}

/// A scripted [`SuiteSelector`]. A call nobody scripted panics.
#[derive(Debug, Default)]
pub struct FakeSelector {
    covers: Mutex<Script<Result<(), CoverageFailure>>>,
    pick: Mutex<Script<Result<Picked, PickRefusal>>>,
    map: Mutex<Script<Result<Vec<CoverageRow>, SuiteError>>>,
}

impl FakeSelector {
    /// A fake with no scripted answers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Scripts `suites_cover_production`.
    pub fn script_covers(&self, script: Script<Result<(), CoverageFailure>>) {
        *self.covers.lock().unwrap_or_else(PoisonError::into_inner) = script;
    }

    /// Scripts `pick_suite`.
    pub fn script_pick(&self, script: Script<Result<Picked, PickRefusal>>) {
        *self.pick.lock().unwrap_or_else(PoisonError::into_inner) = script;
    }

    /// Scripts `coverage_map`.
    pub fn script_map(&self, script: Script<Result<Vec<CoverageRow>, SuiteError>>) {
        *self.map.lock().unwrap_or_else(PoisonError::into_inner) = script;
    }
}

impl SuiteSelector for FakeSelector {
    fn suites_cover_production(
        &self,
        _globs: &ProdGlobs,
        _names: &[String],
        _changed: &NonEmpty<String>,
    ) -> Result<(), CoverageFailure> {
        self.covers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .next("SuiteSelector::suites_cover_production")
    }

    fn pick_suite(
        &self,
        _globs: &ProdGlobs,
        _named: Option<&SuiteName>,
        _changed: &NonEmpty<String>,
    ) -> Result<Picked, PickRefusal> {
        self.pick.lock().unwrap_or_else(PoisonError::into_inner).next("SuiteSelector::pick_suite")
    }

    fn coverage_map(&self, _changed: &[String]) -> Result<Vec<CoverageRow>, SuiteError> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner).next("SuiteSelector::coverage_map")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> SuiteName {
        SuiteName::parse(text).unwrap()
    }

    fn globs() -> ProdGlobs {
        ProdGlobs::from_profile_value("bin/*").unwrap()
    }

    fn changed() -> NonEmpty<String> {
        NonEmpty::new("bin/x".to_string(), [])
    }

    #[test]
    fn the_coverage_failures_print_the_printf_lines_of_suites_cover_production() {
        // bash: printf '  this profile declares no PROD_GLOBS, so nothing here can say which\n' >&2
        //       printf '  paths a receipt has to cover. Refusing rather than answering.\n' >&2
        assert_eq!(
            CoverageFailure::NoProdGlobs.stderr_lines(),
            "  this profile declares no PROD_GLOBS, so nothing here can say which\n  paths a receipt has to cover. Refusing rather than answering.\n"
        );
        // bash: printf '  the receipt names no suite, so it speaks for nothing.\n' >&2
        assert_eq!(
            CoverageFailure::NamesNoSuite.stderr_lines(),
            "  the receipt names no suite, so it speaks for nothing.\n"
        );
        // bash: printf '  the receipt names a suite that is not on disk: %s\n' "$s" >&2
        assert_eq!(
            CoverageFailure::NotOnDisk { name: "../x".into() }.stderr_lines(),
            "  the receipt names a suite that is not on disk: ../x\n"
        );
        // bash: printf '  no suite this receipt was earned by runs these production path(s):\n'
        //       printf '    %s\n' "${uncovered[@]}"
        //       printf '  Naming more suites does not help unless one of them runs the file.\n'
        assert_eq!(
            CoverageFailure::Uncovered { paths: vec!["bin/a".into(), "bin/b".into()] }
                .stderr_lines(),
            "  no suite this receipt was earned by runs these production path(s):\n    bin/a\n    bin/b\n  Naming more suites does not help unless one of them runs the file.\n"
        );
    }

    #[test]
    fn the_pick_refusals_print_the_text_of_the_bash_dies() {
        assert_eq!(
            PickRefusal::NamedDoesNotCover {
                named: name("rust"),
                map: "    a.md\tno suite covers this".into()
            }
            .to_string(),
            "the 'rust' suite does not cover every changed path:\n    a.md\tno suite covers this\n  A receipt from a suite that does not run the changed code proves nothing."
        );
        assert_eq!(
            PickRefusal::NoSingleSuite {
                dir: "/d".into(),
                map: "    x\tdocs".into(),
                suites: "docs".into(),
                orphans: 0
            }
            .to_string(),
            "no single suite in /d covers every changed path:\n    x\tdocs\n\n  Suites that would cover the rest between them: docs.\n  Paths no suite covers at all: 0.\n\n  A receipt written by a suite that does not run the changed code proves\n  nothing, so this refuses rather than picking the closest suite. Add a suite\n  for the uncovered paths, or split the pull request along suite lines."
        );
    }

    #[test]
    fn a_coverage_row_prints_the_line_of_coverage_map() {
        assert_eq!(
            CoverageRow { path: "a.rs".into(), suite: Some(name("rust")) }.line(),
            "    a.rs\trust"
        );
        assert_eq!(
            CoverageRow { path: "a.py".into(), suite: None }.line(),
            "    a.py\tno suite covers this"
        );
    }

    #[test]
    fn the_fake_plays_its_scripts() {
        let fake = FakeSelector::new();
        fake.script_covers(Script::always(Err(CoverageFailure::NamesNoSuite)));
        fake.script_pick(Script::always(Ok(Picked { suite: name("rust"), notes: String::new() })));
        fake.script_map(Script::always(Ok(vec![])));
        assert_eq!(
            fake.suites_cover_production(&globs(), &[], &changed()),
            Err(CoverageFailure::NamesNoSuite)
        );
        assert_eq!(fake.pick_suite(&globs(), None, &changed()).unwrap().suite, name("rust"));
        assert_eq!(fake.coverage_map(&[]), Ok(vec![]));
    }

    #[test]
    #[should_panic(expected = "unscripted call: no answer left for SuiteSelector::pick_suite")]
    fn an_unscripted_call_panics() {
        drop(FakeSelector::new().pick_suite(&globs(), None, &changed()));
    }
}
