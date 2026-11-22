//! `cmd_verify` (bash 1221 to 1654): run the filter in a container against the pull request head,
//! apply the mutation, run it again, and record a receipt only when it passed clean and failed
//! mutated. Every container, extraction and mutation goes through [`mx_sandbox::Sandbox`] and every
//! suite question through [`mx_suite::SuiteHost`], so this file holds the verdict logic and nothing
//! that touches the machine.

use crate::engine::{Dirs, Gate, Printed, R, die};
use crate::{
    Action, CommitId, Exit, HeadArg, LockError, LockKind, PrNumber, Progress, VerifyRefusal,
    first_failure_line, looks_environmental, looks_killed,
};
use mx_core::{Fact, NonEmpty};
use mx_ports::Fs;
use mx_receipt::{FoldBase, Part, Receipt, ReceiptError, Stored, plan_fold, receipt_path};
use mx_sandbox::{
    ContainedTree, ContainerInputs, ContainerSpec, ExtraArgs, LimitSource, Limits, SandboxError,
    SedExpression, Timeout,
};
use mx_suite::{LimitKey, ProdGlobs, SuiteName, TestFilter};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The profile's own figures for a verify run: the two timeouts and the resource limits, with the
/// precedence `MAINTAINER_<KEY>` in the environment, then the suite, then these.
#[derive(Debug, Clone, Default)]
pub struct VerifySettings {
    /// `VERIFY_TIMEOUT`.
    pub verify_timeout: String,
    /// `PREFETCH_TIMEOUT`.
    pub prefetch_timeout: String,
    /// `MAINTAINER_VERIFY_MEMORY` and the other three, by their environment names.
    pub env: BTreeMap<String, String>,
    /// `VERIFY_MEMORY` and the other three, by their `lib/defaults.env` names.
    pub profile: BTreeMap<String, String>,
}

/// Removes every scratch directory when the verify ends, on every path (the bash EXIT trap).
struct Scratch<'a> {
    dirs: &'a dyn Dirs,
    paths: RefCell<Vec<PathBuf>>,
}

impl Drop for Scratch<'_> {
    fn drop(&mut self) {
        for path in self.paths.borrow().iter() {
            self.dirs.remove_tree(path);
        }
    }
}

/// The last `n` lines of `text`, as `tail -n` prints them.
pub(crate) fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn log_text(fs: &dyn Fs, path: &Path) -> String {
    match fs.read(path) {
        Fact::Known(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Fact::Absent | Fact::Unreadable(_) => String::new(),
    }
}

impl Gate<'_> {
    /// `cmd_verify` and `verify-part`.
    pub fn verify(
        &self,
        part: bool,
        pr: &PrNumber,
        sha: &HeadArg,
        filter: &TestFilter,
        mutation: &SedExpression,
        named: Option<&SuiteName>,
    ) -> R<Printed> {
        let mut notes = String::new();
        let scratch = Scratch { dirs: self.p.dirs, paths: RefCell::new(Vec::new()) };
        let outcome =
            self.verify_inner(part, pr, sha, filter, mutation, named, &scratch, &mut notes);
        match outcome {
            Ok(()) => Ok(Printed { stdout: Vec::new(), stderr: notes }),
            Err(mut exit) => {
                exit.stderr.insert_str(0, &notes);
                Err(exit)
            }
        }
    }

    #[allow(
        clippy::too_many_lines,
        clippy::too_many_arguments,
        reason = "one function per bash command, in its order"
    )]
    fn verify_inner(
        &self,
        part: bool,
        pr: &PrNumber,
        sha: &HeadArg,
        filter: &TestFilter,
        mutation: &SedExpression,
        named: Option<&SuiteName>,
        scratch: &Scratch<'_>,
        notes: &mut String,
    ) -> R<()> {
        let refuse = |r: VerifyRefusal| Exit::from(&r);
        self.lease_gate(pr, Action::Verify)?;
        let runtime =
            self.p.runtime.runtime().ok_or_else(|| refuse(VerifyRefusal::NoContainerRuntime))?;

        let receipts = self.cfg.receipts();
        drop(self.p.dirs.make_dir(&receipts));
        let lock_file = LockKind::Verify.file(&receipts, pr);
        let _lock = self.p.locks.try_lock(&lock_file).map_err(|error| {
            refuse(match error {
                LockError::Unopenable => VerifyRefusal::CannotOpenVerifyLock { pr: pr.clone() },
                LockError::Held => VerifyRefusal::VerifyLockHeld {
                    pr: pr.clone(),
                    receipts: receipts.display().to_string(),
                },
            })
        })?;

        let files = self.p.gh.changed_files(pr).unwrap_or_default();
        let changed = NonEmpty::from_vec(files)
            .map_err(|_| refuse(VerifyRefusal::ChangedFilesUnreadable { pr: pr.clone() }))?;
        // A profile without PROD_GLOBS cannot be merged on either, so it cannot be verified
        // either (decision P3B-4); the bash verified and refused later, at `merge`.
        let globs: &ProdGlobs = self.prod_globs()?;

        let suite_name = if part {
            let Some(named) = named else {
                let listed = self.p.suites.names().map_err(|e| die(&e.to_string()))?;
                let suites: String = listed.iter().fold(String::new(), |mut all, n| {
                    all.push_str(n.as_str());
                    all.push(' ');
                    all
                });
                return Err(refuse(VerifyRefusal::PartNeedsSuiteNamed { pr: pr.clone(), suites }));
            };
            let suite = self.p.suites.load(named).map_err(|e| die(&e.to_string()))?;
            let mut covered_any = false;
            for path in changed.iter().filter(|p| globs.matches(p)) {
                if suite.covers(path).map_err(|e| die(&e.to_string()))? {
                    covered_any = true;
                    break;
                }
            }
            if !covered_any {
                return Err(refuse(VerifyRefusal::PartRunsNoProduction {
                    suite: named.to_string(),
                    pr: pr.clone(),
                }));
            }
            named.clone()
        } else {
            let picked = self
                .p
                .selector
                .pick_suite(globs, named, &changed)
                .map_err(|e| die(&e.to_string()))?;
            notes.push_str(&picked.notes);
            picked.suite
        };
        let suite = self.p.suites.load(&suite_name).map_err(|e| die(&e.to_string()))?;
        suite.requires(runtime).map_err(|e| die(&e.to_string()))?;
        let spec = suite.spec();
        self.p.say.say(&Progress::SuiteChosen {
            suite: suite_name.to_string(),
            suite_image: spec.image.as_str().to_string(),
            rt: runtime.as_str().to_string(),
        });

        let verify_work = self.cfg.verify_work();
        let work = self.p.sandbox.scratch_dir(&verify_work).map_err(|_| {
            refuse(VerifyRefusal::CannotCreateScratch {
                verify_work: verify_work.display().to_string(),
            })
        })?;
        scratch.paths.borrow_mut().push(work.clone());
        let scratch_dir = self.p.sandbox.scratch_dir(&verify_work).map_err(|_| {
            refuse(VerifyRefusal::CannotCreateScratch {
                verify_work: verify_work.display().to_string(),
            })
        })?;
        scratch.paths.borrow_mut().push(scratch_dir.clone());
        if scratch_dir.starts_with(&work) {
            return Err(refuse(VerifyRefusal::ScratchInsideTree));
        }

        let repo = self.cfg.repo.as_path();
        let branch = format!("v-{pr}-{}", self.cfg.run_id);
        self.p
            .git
            .fetch_ref(repo, &format!("refs/pull/{pr}/head:{branch}"))
            .map_err(|_| refuse(VerifyRefusal::CannotFetch { pr: pr.clone() }))?;
        let real_head_text = self.p.git.rev_parse(repo, &branch).unwrap_or_default();
        let real_head = match CommitId::parse(&real_head_text) {
            Ok(head) if sha.names(&head) => head,
            _ => {
                self.p.git.delete_branch(repo, &branch);
                return Err(refuse(VerifyRefusal::HeadIsNot {
                    real_head: real_head_text,
                    sha: sha.as_str().to_string(),
                }));
            }
        };
        let extracted = self.p.sandbox.extract_tree(repo, &branch, &work);
        self.p.git.delete_branch(repo, &branch);
        let extracted = match extracted {
            Ok(tree) => tree,
            Err(error) => {
                let mut exit = refuse(VerifyRefusal::CannotExtract { pr: pr.clone() });
                exit = Exit { code: exit.code, stderr: format!("{error}\n{}", exit.stderr) };
                return Err(exit);
            }
        };
        let escapes = self.p.sandbox.tree_escapes(&extracted).map_err(|e| die(&e.to_string()))?;
        let tree: ContainedTree = ContainedTree::check(extracted, escapes)
            .map_err(|escaping| die(&escaping.message(pr.as_arg())))?;
        if self.p.sandbox.seed_index(&tree).is_err() {
            self.p.say.say(&Progress::NoGitIndex);
        }

        // Limits: the environment, then the suite, then the profile.
        let mut from_suite: BTreeMap<&'static str, Option<String>> = BTreeMap::new();
        for key in LimitKey::ALL {
            from_suite.insert(key.env_name(), suite.limit(key).map_err(|e| die(&e.to_string()))?);
        }
        let settings = self.p.verify;
        let limits = Limits::resolve(|key| LimitSource {
            env: settings.env.get(key.env_name()).cloned(),
            suite: from_suite.get(key.env_name()).cloned().flatten(),
            profile: settings.profile.get(key.defaults_key()).cloned(),
        })
        .map_err(|e| die(&e.to_string()))?;
        let extra = suite.extra_args(runtime, &scratch_dir).map_err(|e| die(&e.to_string()))?;
        let extra_args = ExtraArgs::split(&extra);
        let image = spec.image.clone();
        let inputs = |command: String, timeout: Timeout| ContainerInputs {
            runtime,
            work: work.clone(),
            image: image.clone(),
            command,
            extra_args: extra_args.clone(),
            timeout,
            limits: limits.clone(),
        };

        if let Some(prefetch) = &spec.prefetch {
            if prefetch.guarded {
                suite.prefetch_guard(&work).map_err(|e| die(&e.to_string()))?;
            }
            self.p.say.say(&Progress::FetchingLockfile);
            let timeout =
                Timeout::parse(&settings.prefetch_timeout).map_err(|e| die(&e.to_string()))?;
            let run = ContainerSpec::prefetch(inputs(prefetch.command.clone(), timeout))
                .map_err(|e| die(&e.to_string()))?;
            let log = work.join("prefetch.log");
            let code = self
                .p
                .sandbox
                .run_container(&run, &log)
                .map_or(125, mx_sandbox::ContainerExit::code);
            if code != 0 {
                let work_tail = tail(&log_text(self.p.fs, &log), 5);
                return Err(refuse(VerifyRefusal::PrefetchFailed { work_tail }));
            }
        }

        let timeout = Timeout::parse(&settings.verify_timeout).map_err(|e| die(&e.to_string()))?;
        let command = suite.command(filter).map_err(|e| die(&e.to_string()))?;
        let run_spec =
            ContainerSpec::verify(inputs(command, timeout)).map_err(|e| die(&e.to_string()))?;
        let clean_log = work.join("clean.log");
        let mut_log = work.join("mutated.log");
        self.p.say.say(&Progress::RunningUnmutated { filter: filter.as_str().to_string() });
        let clean_rc = self.run_container(&run_spec, &clean_log)?;
        if clean_rc != 0 {
            let text = log_text(self.p.fs, &clean_log);
            if looks_killed(clean_rc) {
                return Err(refuse(VerifyRefusal::CleanRunKilled {
                    clean_rc: clean_rc.to_string(),
                    pr: pr.clone(),
                }));
            }
            if let Some(env_hit) = looks_environmental(&text) {
                return Err(refuse(VerifyRefusal::FailedOnSuiteEnvironment {
                    env_hit,
                    clean_log_tail: tail(&text, 5),
                    pr: pr.clone(),
                }));
            }
            return Err(refuse(VerifyRefusal::DoesNotPassUnmutated {
                clean_rc: clean_rc.to_string(),
                clean_log_tail: tail(&text, 5),
            }));
        }

        let ran = suite.ran(&clean_log).unwrap_or(0);
        if ran < 1 {
            return Err(refuse(VerifyRefusal::NoEvidenceItRan {
                filter: filter.as_str().to_string(),
                clean_log_tail: tail(&log_text(self.p.fs, &clean_log), 3),
            }));
        }
        self.p.say.say(&Progress::EvidenceOfUnits {
            suite: suite_name.to_string(),
            ran: ran.to_string(),
        });

        self.p.say.say(&Progress::ApplyingMutation);
        let globs_of_suite = spec.mutate_globs().map_err(|e| die(&e.to_string()))?;
        let mutated = match self.p.sandbox.apply_mutation(&tree, &globs_of_suite, mutation) {
            Ok(mutated) => mutated,
            Err(SandboxError::NothingChanged { globs }) => {
                return Err(die(&mx_sandbox::NothingChanged { globs }.message(pr.as_arg())));
            }
            Err(error) => return Err(die(&error.to_string())),
        };
        let mutated_paths = mutated.paths();

        let changed_prod: Vec<&String> = changed.iter().filter(|p| globs.matches(p)).collect();
        let diff_has_prod = !changed_prod.is_empty();
        let hit_prod = mutated_paths.iter().any(|p| globs.matches(p));
        let hit_changed =
            mutated_paths.iter().any(|p| globs.matches(p) && changed_prod.contains(&p));
        if diff_has_prod && !hit_prod {
            return Err(refuse(VerifyRefusal::LandedOutsideProduction {
                mutated: mutated_paths.join("\n"),
                prod_globs: globs.as_slice().join(" "),
                filter: filter.as_str().to_string(),
            }));
        }
        if diff_has_prod && !hit_changed {
            return Err(refuse(VerifyRefusal::LandedOnUnchangedProduction {
                mutated: mutated_paths.join("\n"),
                changed_prod: changed_prod
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                filter: filter.as_str().to_string(),
            }));
        }

        let mut_rc = self.run_container(&run_spec, &mut_log)?;
        if mut_rc == 0 {
            return Err(refuse(VerifyRefusal::GuardDoesNotBite {
                filter: filter.as_str().to_string(),
            }));
        }
        if looks_killed(mut_rc) {
            return Err(refuse(VerifyRefusal::MutatedRunKilled { mut_rc: mut_rc.to_string() }));
        }

        drop(self.p.dirs.make_dir(&receipts));
        let key = self.key()?;
        let path = receipt_path(&receipts, pr.as_arg());
        let prior = match self.p.fs.read(&path) {
            Fact::Known(bytes) => Some(bytes),
            Fact::Absent | Fact::Unreadable(_) => None,
        };
        let base =
            plan_fold(prior.as_deref(), pr.as_arg(), real_head.as_str(), part.then_some("1"), &key)
                .map_err(|error| match error {
                    ReceiptError::PriorNotTrusted { fault } => {
                        refuse(VerifyRefusal::PriorReceiptNotTrusted {
                            pr: pr.clone(),
                            psig: fault.code().to_string(),
                            receipts: receipts.display().to_string(),
                        })
                    }
                    other => die(&other.to_string()),
                })?;
        if let (FoldBase::Fresh { note: Some(_) }, Some(bytes)) = (&base, &prior) {
            let (prior_pr, prior_head) = Stored::parse(bytes)
                .map(|s| (s.field_text("pr", ""), s.field_text("head", "")))
                .unwrap_or_default();
            self.p.say.say(&Progress::ReceiptOnDiskIsForOther {
                prior_pr: if prior_pr.is_empty() { "?".to_string() } else { prior_pr },
                prior_head_8: prior_head.chars().take(8).collect(),
            });
        }
        let observed = Part {
            suite: suite_name.to_string(),
            units_run: ran.to_string(),
            filter: filter.as_str().to_string(),
            mutation: mutation.as_str().to_string(),
            mutated_rc: mut_rc,
            observed_failure: first_failure_line(&log_text(self.p.fs, &mut_log)),
        };
        let receipt = Receipt::observed(
            pr.value(),
            real_head.as_str(),
            base,
            &observed,
            part.then_some("1"),
            &self.p.stamps.stamp(),
        );
        mx_receipt::write(self.p.fs, &path, &receipt.sign(&key))
            .map_err(|e| die(&e.to_string()))?;
        self.p.say.say(&Progress::ObservedReceiptRecorded {
            pr: pr.clone(),
            real_head_8: real_head.short().to_string(),
        });
        Ok(())
    }

    /// One container run. A run that never started reads as the runtime's own failure, 125.
    fn run_container(&self, spec: &ContainerSpec, log: &Path) -> R<i32> {
        match self.p.sandbox.run_container(spec, log) {
            Ok(exit) => Ok(exit.code()),
            Err(error) => Err(die(&error.to_string())),
        }
    }
}
