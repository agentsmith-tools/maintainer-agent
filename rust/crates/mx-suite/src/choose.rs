//! The coverage rules, answered from a [`SuiteHost`]: the port of `coverage_map`,
//! `covers_production`, `covers_when_no_production`, `suites_cover_production` and
//! `pick_suite` of `bin/maintainer-merge` (lines 773 to 1098).
//!
//! Every question about one path is one [`SuiteHost::covers`], which the real host answers with
//! one `bash` process, as the bash gate answers it with one subshell. A question that could
//! not be asked (bash would not start, a suite ran past its bound) is an `Err` here, where the
//! bash reads any failure as "no": both refuse, and the `Err` says why. The order of every
//! loop and of every message is the bash's, so the refusal a person reads is the same one.

use crate::{
    CoverageFailure, CoverageRow, PickRefusal, Picked, ProdGlobs, SuiteError, SuiteHost, SuiteName,
    SuiteSelector,
};
use mx_core::NonEmpty;
use std::fmt::Write as _;

/// The real [`SuiteSelector`]: the coverage rules over one host.
pub struct HostSelector<'a> {
    host: &'a dyn SuiteHost,
}

impl<'a> HostSelector<'a> {
    /// Rules that ask `host`.
    #[must_use]
    pub fn new(host: &'a dyn SuiteHost) -> Self {
        Self { host }
    }

    /// `pick_suite` for a profile that declares no `PROD_GLOBS`. `verify` runs without them
    /// (only `merge` and `update` refuse a profile that declares none), and with an empty list
    /// both narrowings return 1 at once, so a suite is picked only when it covers every changed
    /// path. [`SuiteSelector::pick_suite`] takes a [`ProdGlobs`], which cannot be empty, so the
    /// gate calls this one when the profile has none. Added by the suite agent (P3A-5).
    pub fn pick_suite_without_prod_globs(
        &self,
        named: Option<&SuiteName>,
        changed: &NonEmpty<String>,
    ) -> Result<Picked, PickRefusal> {
        self.pick(None, named, changed)
    }

    /// Does `suite` cover every one of `paths`? Stops at the first that it does not.
    fn covers_all(&self, suite: &SuiteName, paths: &[&String]) -> Result<bool, SuiteError> {
        for path in paths {
            if !self.host.covers(suite, path)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Is `path` covered by any suite on disk, inferable or not?
    fn covered_by_any(&self, all: &[SuiteName], path: &str) -> Result<bool, SuiteError> {
        for suite in all {
            if self.host.covers(suite, path)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The paths of `rest` that `suite` does not cover.
    fn unproven<'p>(
        &self,
        suite: &SuiteName,
        rest: &[&'p String],
    ) -> Result<Vec<&'p String>, SuiteError> {
        let mut out = Vec::new();
        for path in rest {
            if !self.host.covers(suite, path)? {
                out.push(*path);
            }
        }
        Ok(out)
    }

    /// `covers_production`: `Some(notes)` when `suite` may write a receipt for this diff.
    fn covers_production(
        &self,
        globs: Option<&ProdGlobs>,
        suite: &SuiteName,
        all: &[SuiteName],
        changed: &NonEmpty<String>,
    ) -> Result<Option<String>, SuiteError> {
        // `[ "${#PROD_GLOBS[@]}" -gt 0 ] || return 1`
        let Some(globs) = globs else { return Ok(None) };
        let (prod, rest): (Vec<&String>, Vec<&String>) =
            changed.iter().partition(|p| globs.matches(p));
        if prod.is_empty() || rest.is_empty() {
            return Ok(None);
        }
        // An incomplete PROD_GLOBS must not become a way to smuggle an unknown file past the
        // gate, so a path no suite covers at all still refuses.
        for q in &rest {
            if !self.covered_by_any(all, q)? {
                return Ok(None);
            }
        }
        if !self.covers_all(suite, &prod)? {
            return Ok(None);
        }
        let unproven = self.unproven(suite, &rest)?;
        let mut notes = String::new();
        if !unproven.is_empty() {
            let _ = write!(
                notes,
                "  the {suite} suite covers every PRODUCTION path; {} other path(s) are\n  not covered by the receipt and it proves nothing about them:\n",
                unproven.len()
            );
            for path in &unproven {
                let _ = writeln!(notes, "    {path}");
            }
            notes.push_str(
                "  PROD_GLOBS is what cmd_merge re-checks at merge, so those paths\n  cannot invalidate this proof. CI still gates them.\n",
            );
        }
        Ok(Some(notes))
    }

    /// `covers_when_no_production`: the same narrowing for a diff with no production path.
    fn covers_when_no_production(
        &self,
        globs: Option<&ProdGlobs>,
        suite: &SuiteName,
        all: &[SuiteName],
        changed: &NonEmpty<String>,
    ) -> Result<Option<String>, SuiteError> {
        let Some(globs) = globs else { return Ok(None) };
        if changed.iter().any(|p| globs.matches(p)) {
            return Ok(None);
        }
        for q in changed {
            if !self.covered_by_any(all, q)? {
                return Ok(None);
            }
        }
        let paths: Vec<&String> = changed.iter().collect();
        let unproven = self.unproven(suite, &paths)?;
        let runs = paths.len() - unproven.len();
        // The suite has to run something the pull request actually changed.
        if runs < 1 {
            return Ok(None);
        }
        let mut notes = String::new();
        if !unproven.is_empty() {
            let _ = write!(
                notes,
                "  nothing in this diff is in PROD_GLOBS, so no path here can invalidate\n  the receipt at merge. The {suite} suite runs {runs} of {} changed path(s); the\n  rest are not covered by the receipt and it proves nothing about them:\n",
                paths.len()
            );
            for path in &unproven {
                let _ = writeln!(notes, "    {path}");
            }
            notes.push_str("  CI still gates them.\n");
        }
        Ok(Some(notes))
    }

    fn pick(
        &self,
        globs: Option<&ProdGlobs>,
        named: Option<&SuiteName>,
        changed: &NonEmpty<String>,
    ) -> Result<Picked, PickRefusal> {
        let suite_err = PickRefusal::Suite;
        let dir = self.host.dir().map_err(suite_err)?;
        let all = self.host.names().map_err(PickRefusal::Suite)?;
        let paths: Vec<&String> = changed.iter().collect();
        if let Some(named) = named {
            if !all.contains(named) {
                return Err(PickRefusal::Suite(SuiteError::NotOnDisk { name: named.clone(), dir }));
            }
            // Naming a suite does not skip the check: a receipt from a suite that does not
            // run the changed code proves nothing.
            if self.covers_all(named, &paths).map_err(PickRefusal::Suite)? {
                return Ok(Picked { suite: named.clone(), notes: String::new() });
            }
            let narrowed = match self
                .covers_production(globs, named, &all, changed)
                .map_err(PickRefusal::Suite)?
            {
                Some(notes) => Some(notes),
                None => self
                    .covers_when_no_production(globs, named, &all, changed)
                    .map_err(PickRefusal::Suite)?,
            };
            if let Some(notes) = narrowed {
                return Ok(Picked { suite: named.clone(), notes });
            }
            let rows = self.map(changed.as_slice()).map_err(PickRefusal::Suite)?;
            return Err(PickRefusal::NamedDoesNotCover {
                named: named.clone(),
                map: map_text(&rows),
            });
        }
        let mut inferable = Vec::new();
        for name in &all {
            if self.host.inferable(name).map_err(PickRefusal::Suite)? {
                inferable.push(name);
            }
        }
        for name in &inferable {
            if self.covers_all(name, &paths).map_err(PickRefusal::Suite)? {
                return Ok(Picked { suite: (*name).clone(), notes: String::new() });
            }
        }
        // Nothing covered every path. Try again over the production paths alone, because
        // those are the ones a receipt binds.
        for name in &inferable {
            if let Some(notes) =
                self.covers_production(globs, name, &all, changed).map_err(PickRefusal::Suite)?
            {
                return Ok(Picked { suite: (*name).clone(), notes });
            }
        }
        let rows = self.map(changed.as_slice()).map_err(PickRefusal::Suite)?;
        let orphans = rows.iter().filter(|r| r.suite.is_none()).count() as u64;
        let mut between: Vec<String> =
            rows.iter().filter_map(|r| r.suite.as_ref().map(ToString::to_string)).collect();
        between.sort();
        between.dedup();
        let suites = if between.is_empty() { "none".to_string() } else { between.join(" ") };
        Err(PickRefusal::NoSingleSuite {
            dir: dir.display().to_string(),
            map: map_text(&rows),
            suites,
            orphans,
        })
    }

    fn map(&self, paths: &[String]) -> Result<Vec<CoverageRow>, SuiteError> {
        let all = self.host.names()?;
        let mut inferable = Vec::new();
        for name in &all {
            if self.host.inferable(name)? {
                inferable.push(name);
            }
        }
        let mut rows = Vec::with_capacity(paths.len());
        for path in paths {
            let mut hit = None;
            for name in &inferable {
                if self.host.covers(name, path)? {
                    hit = Some((*name).clone());
                    break;
                }
            }
            rows.push(CoverageRow { path: path.clone(), suite: hit });
        }
        Ok(rows)
    }
}

fn map_text(rows: &[CoverageRow]) -> String {
    rows.iter().map(CoverageRow::line).collect::<Vec<_>>().join("\n")
}

impl SuiteSelector for HostSelector<'_> {
    fn suites_cover_production(
        &self,
        globs: &ProdGlobs,
        names: &[String],
        changed: &NonEmpty<String>,
    ) -> Result<(), CoverageFailure> {
        if names.is_empty() {
            return Err(CoverageFailure::NamesNoSuite);
        }
        self.host.dir().map_err(CoverageFailure::Suite)?;
        let on_disk = self.host.names().map_err(CoverageFailure::Suite)?;
        // A name with no file fails closed. A name that is not a valid suite name has no file
        // either, and is never turned into a path (F3).
        let mut chosen = Vec::with_capacity(names.len());
        for text in names {
            match SuiteName::parse(text) {
                Ok(name) if on_disk.contains(&name) => chosen.push(name),
                _ => return Err(CoverageFailure::NotOnDisk { name: text.clone() }),
            }
        }
        let mut uncovered = Vec::new();
        for path in changed.iter().filter(|p| globs.matches(p)) {
            let mut any = false;
            for suite in &chosen {
                if self.host.covers(suite, path).map_err(CoverageFailure::Suite)? {
                    any = true;
                    break;
                }
            }
            if !any {
                uncovered.push(path.clone());
            }
        }
        if uncovered.is_empty() {
            Ok(())
        } else {
            Err(CoverageFailure::Uncovered { paths: uncovered })
        }
    }

    fn pick_suite(
        &self,
        globs: &ProdGlobs,
        named: Option<&SuiteName>,
        changed: &NonEmpty<String>,
    ) -> Result<Picked, PickRefusal> {
        self.pick(Some(globs), named, changed)
    }

    fn coverage_map(&self, changed: &[String]) -> Result<Vec<CoverageRow>, SuiteError> {
        self.map(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FakeSuite, FakeSuiteHost, SuiteCall};

    const DIR: &str = "/profiles/p/verify.d";

    fn name(text: &str) -> SuiteName {
        SuiteName::parse(text).unwrap()
    }

    fn globs() -> ProdGlobs {
        ProdGlobs::from_profile_value("bin/* lib/* src/*").unwrap()
    }

    fn changed(paths: &[&str]) -> NonEmpty<String> {
        NonEmpty::from_vec(paths.iter().map(|p| (*p).to_string()).collect()).unwrap()
    }

    fn rust() -> FakeSuite {
        FakeSuite::new("rust", "img").covering(&["src/a.rs", "src/b.rs", "tests/t.rs"])
    }

    fn docs() -> FakeSuite {
        FakeSuite::new("docs", "img").covering(&["README.md", "docs/x.md"])
    }

    fn shell() -> FakeSuite {
        FakeSuite::new("shell", "img").covering(&["bin/run", "lib/x.sh", "tests/t.sh"])
    }

    fn host(suites: Vec<FakeSuite>) -> FakeSuiteHost {
        FakeSuiteHost::new(DIR, suites)
    }

    #[test]
    fn a_named_suite_that_covers_every_path_is_picked_without_a_note() {
        let host = host(vec![rust(), docs()]);
        let picked = HostSelector::new(&host)
            .pick_suite(&globs(), Some(&name("rust")), &changed(&["src/a.rs", "tests/t.rs"]))
            .unwrap();
        assert_eq!(picked, Picked { suite: name("rust"), notes: String::new() });
    }

    #[test]
    fn a_named_suite_that_covers_the_production_paths_only_says_what_it_leaves_unproven() {
        // bash covers_production: the printf lines on standard error, then `return 0`.
        let host = host(vec![rust(), docs()]);
        let picked = HostSelector::new(&host)
            .pick_suite(
                &globs(),
                Some(&name("rust")),
                &changed(&["src/a.rs", "tests/t.rs", "README.md"]),
            )
            .unwrap();
        assert_eq!(picked.suite, name("rust"));
        // tests/t.rs is not production but rust runs it, so only README.md is unproven.
        assert_eq!(
            picked.notes,
            "  the rust suite covers every PRODUCTION path; 1 other path(s) are\n  not covered by the receipt and it proves nothing about them:\n    README.md\n  PROD_GLOBS is what cmd_merge re-checks at merge, so those paths\n  cannot invalidate this proof. CI still gates them.\n"
        );
    }

    #[test]
    fn a_named_suite_that_misses_a_production_path_is_refused_with_the_coverage_map() {
        let host = host(vec![rust(), docs(), shell()]);
        let refused = HostSelector::new(&host)
            .pick_suite(&globs(), Some(&name("rust")), &changed(&["src/a.rs", "bin/run"]))
            .unwrap_err();
        assert_eq!(
            refused.to_string(),
            "the 'rust' suite does not cover every changed path:\n    src/a.rs\trust\n    bin/run\tshell\n  A receipt from a suite that does not run the changed code proves nothing."
        );
    }

    #[test]
    fn a_named_suite_that_is_not_on_disk_is_refused_with_the_directory() {
        let host = host(vec![rust()]);
        let refused = HostSelector::new(&host)
            .pick_suite(&globs(), Some(&name("python")), &changed(&["src/a.rs"]))
            .unwrap_err();
        assert_eq!(refused.to_string(), "no suite 'python' in /profiles/p/verify.d");
    }

    #[test]
    fn no_verify_d_is_the_bash_sentence() {
        let host = FakeSuiteHost::without_dir();
        let refused = HostSelector::new(&host)
            .pick_suite(&globs(), None, &changed(&["src/a.rs"]))
            .unwrap_err();
        assert_eq!(
            refused.to_string(),
            "this profile declares no verify.d/ suites; see docs/verifying.md"
        );
    }

    #[test]
    fn inference_takes_the_first_inferable_suite_in_name_order_that_covers_every_path() {
        let both = FakeSuite::new("alpha", "img").covering(&["src/a.rs"]);
        let also = FakeSuite::new("beta", "img").covering(&["src/a.rs"]);
        let host = host(vec![also, both]);
        let picked =
            HostSelector::new(&host).pick_suite(&globs(), None, &changed(&["src/a.rs"])).unwrap();
        assert_eq!(picked.suite, name("alpha"));
    }

    #[test]
    fn inference_never_picks_a_named_only_suite() {
        let only = FakeSuite::new("aaa", "img").covering(&["src/a.rs"]).named_only();
        let host = host(vec![only, rust()]);
        let picked =
            HostSelector::new(&host).pick_suite(&globs(), None, &changed(&["src/a.rs"])).unwrap();
        assert_eq!(picked.suite, name("rust"), "aaa sorts first and covers the path");
    }

    #[test]
    fn inference_falls_back_to_the_production_paths_alone() {
        let host = host(vec![rust(), docs()]);
        let picked = HostSelector::new(&host)
            .pick_suite(&globs(), None, &changed(&["src/a.rs", "README.md"]))
            .unwrap();
        assert_eq!(picked.suite, name("rust"));
        assert!(picked.notes.starts_with("  the rust suite covers every PRODUCTION path; 1 other"));
    }

    #[test]
    fn no_suite_for_the_diff_refuses_with_the_map_the_suites_between_them_and_the_orphans() {
        let host = host(vec![rust(), docs()]);
        let refused = HostSelector::new(&host)
            .pick_suite(
                &globs(),
                None,
                &changed(&["src/a.rs", "bin/run", "README.md", "x.py", "src/b.rs"]),
            )
            .unwrap_err();
        assert_eq!(
            refused.to_string(),
            "no single suite in /profiles/p/verify.d covers every changed path:\n    src/a.rs\trust\n    bin/run\tno suite covers this\n    README.md\tdocs\n    x.py\tno suite covers this\n    src/b.rs\trust\n\n  Suites that would cover the rest between them: docs rust.\n  Paths no suite covers at all: 2.\n\n  A receipt written by a suite that does not run the changed code proves\n  nothing, so this refuses rather than picking the closest suite. Add a suite\n  for the uncovered paths, or split the pull request along suite lines."
        );
    }

    #[test]
    fn a_production_path_no_suite_runs_cannot_be_narrowed_away() {
        // An incomplete PROD_GLOBS must not smuggle an unknown file past the gate: the
        // non-production path x.py is covered by no suite, so covers_production refuses.
        let host = host(vec![rust()]);
        let refused = HostSelector::new(&host)
            .pick_suite(&globs(), None, &changed(&["src/a.rs", "x.py"]))
            .unwrap_err();
        assert!(matches!(refused, PickRefusal::NoSingleSuite { orphans: 1, .. }), "{refused:?}");
    }

    #[test]
    fn a_named_suite_may_cover_a_diff_with_no_production_path_and_says_what_it_leaves() {
        // bash covers_when_no_production, sysknife#422: test code, an artifact and prose.
        let host = host(vec![rust(), docs()]);
        let picked = HostSelector::new(&host)
            .pick_suite(&globs(), Some(&name("rust")), &changed(&["tests/t.rs", "README.md"]))
            .unwrap();
        assert_eq!(picked.suite, name("rust"));
        assert_eq!(
            picked.notes,
            "  nothing in this diff is in PROD_GLOBS, so no path here can invalidate\n  the receipt at merge. The rust suite runs 1 of 2 changed path(s); the\n  rest are not covered by the receipt and it proves nothing about them:\n    README.md\n  CI still gates them.\n"
        );
    }

    #[test]
    fn inference_does_not_guess_for_a_diff_with_no_production_path() {
        // With no anchor, both rust and docs would qualify; pick_suite refuses to choose.
        let host = host(vec![rust(), docs()]);
        let refused = HostSelector::new(&host)
            .pick_suite(&globs(), None, &changed(&["tests/t.rs", "README.md"]))
            .unwrap_err();
        assert!(matches!(refused, PickRefusal::NoSingleSuite { .. }), "{refused:?}");
    }

    #[test]
    fn a_suite_that_runs_none_of_a_no_production_diff_is_refused() {
        // docs runs neither path of the diff, so a receipt from it would be about files the
        // pull request never touched.
        let host = host(vec![rust(), docs(), shell()]);
        let refused = HostSelector::new(&host)
            .pick_suite(&globs(), Some(&name("docs")), &changed(&["tests/t.rs"]))
            .unwrap_err();
        assert!(matches!(refused, PickRefusal::NamedDoesNotCover { .. }), "{refused:?}");
    }

    #[test]
    fn the_coverage_map_names_the_first_inferable_suite_of_each_path() {
        let only = FakeSuite::new("aaa", "img").covering(&["README.md"]).named_only();
        let host = host(vec![only, docs(), rust()]);
        let rows = HostSelector::new(&host)
            .coverage_map(&["README.md".to_string(), "src/a.rs".into(), "z".into()])
            .unwrap();
        let lines: Vec<String> = rows.iter().map(CoverageRow::line).collect();
        assert_eq!(
            lines,
            ["    README.md\tdocs", "    src/a.rs\trust", "    z\tno suite covers this"]
        );
    }

    #[test]
    fn the_union_of_the_named_suites_covers_the_production_paths() {
        let host = host(vec![rust(), shell(), docs()]);
        let rules = HostSelector::new(&host);
        let diff = changed(&["src/a.rs", "bin/run", "README.md"]);
        assert_eq!(
            rules.suites_cover_production(&globs(), &["rust".into(), "shell".into()], &diff),
            Ok(())
        );
        assert_eq!(
            rules.suites_cover_production(&globs(), &["rust".into()], &diff),
            Err(CoverageFailure::Uncovered { paths: vec!["bin/run".into()] })
        );
    }

    #[test]
    fn a_receipt_that_names_no_suite_or_one_that_is_not_on_disk_is_refused() {
        let host = host(vec![rust()]);
        let rules = HostSelector::new(&host);
        let diff = changed(&["src/a.rs"]);
        assert_eq!(
            rules.suites_cover_production(&globs(), &[], &diff),
            Err(CoverageFailure::NamesNoSuite)
        );
        assert_eq!(
            rules.suites_cover_production(&globs(), &["python".into()], &diff),
            Err(CoverageFailure::NotOnDisk { name: "python".into() })
        );
    }

    #[test]
    fn a_name_that_is_not_a_suite_name_is_never_turned_into_a_path() {
        let host = host(vec![rust()]);
        let rules = HostSelector::new(&host);
        for bad in ["../rust", "a/b", "ru st", "rust.sh", ""] {
            assert_eq!(
                rules.suites_cover_production(
                    &globs(),
                    &[bad.to_string()],
                    &changed(&["src/a.rs"])
                ),
                Err(CoverageFailure::NotOnDisk { name: bad.to_string() }),
                "{bad:?}"
            );
        }
        assert!(
            !host
                .calls()
                .iter()
                .any(|c| matches!(c, SuiteCall::Load(_) | SuiteCall::Covers { .. })),
            "no suite was loaded or asked"
        );
    }

    #[test]
    fn a_diff_with_no_production_path_has_nothing_for_the_union_to_cover() {
        let host = host(vec![rust()]);
        let rules = HostSelector::new(&host);
        assert_eq!(
            rules.suites_cover_production(&globs(), &["rust".into()], &changed(&["README.md"])),
            Ok(())
        );
        assert!(
            !host.calls().iter().any(|c| matches!(c, SuiteCall::Covers { .. })),
            "no path was asked about"
        );
    }

    #[test]
    fn a_host_without_verify_d_refuses_the_union_check_silently() {
        let host = FakeSuiteHost::without_dir();
        let refused = HostSelector::new(&host)
            .suites_cover_production(&globs(), &["rust".into()], &changed(&["src/a.rs"]))
            .unwrap_err();
        assert_eq!(refused, CoverageFailure::Suite(SuiteError::NoSuiteDir));
        assert_eq!(refused.stderr_lines(), "");
    }
}
