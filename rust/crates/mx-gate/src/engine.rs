//! The gate itself: `receipt`, `merge`, `update` and `show`, written against the ports of this
//! crate, in the order of `bin/maintainer-merge` (`cmd_receipt` 395, `cmd_merge` 1747,
//! `cmd_update` 2119, `cmd_show` 1656). No process, no file and no clock is reached except
//! through a port, so every refusal is reachable from a unit test with a fake.
//!
//! A merge is only reachable through the typestate: `Pr<Unverified>` becomes `Pr<Receipted>`
//! through [`Pr::replay`] (a signed receipt for that pull request), becomes `Pr<Mergeable>`
//! through [`Pr::cleared`] (every witness of one pull request and one head), and
//! [`Merger::merge`] takes nothing else.

use crate::{
    Account, Action, Approved, CheckBoard, ClaimsClear, Command, CommitId, Conditions, Confirmed,
    DiffFailure, Exit, Fresh, GateDecision, GateGit, GhFailure, GhPort, HeadArg, Helper,
    HoldersFailure, IdentityPort, IssueNumber, LeaseFree, LockError, LockKind, LockPort,
    MergeState, Merger, Posting, Pr, ProductionDiff, Progress, Receipted, Say, UpdateRefusal,
};
use mx_core::{Fact, NonEmpty};
use mx_ports::{Clock, Fs};
use mx_receipt::{
    Key, Kind, ListEntry, Receipt, ReceiptError, Signed, Stamp, Stored, receipt_path,
};
use mx_suite::{ProdGlobs, SuiteHost, SuiteSelector};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The refusal of `receipt` in an unattended run (bash 404).
pub const UNATTENDED_ASSERT: &str = "an unattended run may not assert a receipt.\n  Only `maintainer-merge verify` earns one, because only it observes the\n  mutation. If this pull request cannot be mutation-proven, say so in the report\n  and leave the merge to a human.";

/// What `verify` and the check of a partial receipt say while this build has no suite host.
pub const NO_SUITE_HOST: &str = "this build of the Rust gate has no suite host or sandbox yet, so it cannot verify a pull request or check which suites a partial receipt covers. Nothing was changed.";

/// Directories the gate makes and removes (`mkdir -p`, the EXIT trap's `rm -rf`).
pub trait Dirs {
    /// `mkdir -p`.
    fn make_dir(&self, path: &Path) -> Result<(), String>;
    /// `rm -rf`, status ignored.
    fn remove_tree(&self, path: &Path);
}

/// What `recorded_at` and `recorded_by` of a new receipt say.
pub trait StampSource {
    /// The moment and the user, as `datetime.now().astimezone().isoformat()` and
    /// `getpass.getuser()` write them.
    fn stamp(&self) -> Stamp;
}

/// `container_runtime`: podman if it is on the search path, docker otherwise.
pub trait RuntimeProbe {
    /// `None` when neither is installed.
    fn runtime(&self) -> Option<mx_suite::Runtime>;
}

/// The profile values and the environment the gate reads, resolved once by the caller.
#[derive(Debug, Clone)]
pub struct GateConfig {
    /// `MAINTAINER_SLUG`.
    pub slug: String,
    /// `MAINTAINER_REPO`.
    pub repo: PathBuf,
    /// `MAINTAINER_STATE`.
    pub state: PathBuf,
    /// `MAINTAINER_ACCOUNT`.
    pub account: Account,
    /// `MAINTAINER_POST`: only the exact word `off` rehearses (decision P30-6).
    pub posting: Posting,
    /// `PROD_GLOBS`, or `None` when the profile declares none.
    pub prod_globs: Option<ProdGlobs>,
    /// `CLAIM_LABEL`, empty when the project uses none.
    pub claim_label: String,
    /// `MAINTAINER_RECEIPT_KEY`.
    pub receipt_key: PathBuf,
    /// `MAINTAINER_IN_RUN` is set and non-empty.
    pub in_run: bool,
    /// Makes the temporary refs of this run its own, so two runs never share one.
    pub run_id: u32,
    /// False while the suite and sandbox adapters of the p3a lane are not built in.
    pub has_suite_host: bool,
    /// `UPDATE_READBACK_READS`.
    pub readback_reads: Option<String>,
    /// `UPDATE_READBACK_WAIT`.
    pub readback_wait: Option<String>,
}

impl GateConfig {
    /// `$STATE/receipts`.
    #[must_use]
    pub fn receipts(&self) -> PathBuf {
        self.state.join("receipts")
    }

    /// `$STATE/verify-work`.
    #[must_use]
    pub fn verify_work(&self) -> PathBuf {
        self.state.join("verify-work")
    }
}

/// Every port the gate reads and writes through.
pub struct Ports<'a> {
    /// The GitHub reads and writes.
    pub gh: &'a dyn GhPort,
    /// The repository checkout.
    pub git: &'a dyn GateGit,
    /// `maintainer lease check` and `maintainer holders`.
    pub helper: &'a dyn Helper,
    /// `gh auth switch` and `gh api user`.
    pub identity: &'a dyn IdentityPort,
    /// The per-pull-request locks.
    pub locks: &'a dyn LockPort,
    /// Receipts and the key.
    pub fs: &'a dyn Fs,
    /// `mkdir -p` and the scratch cleanup.
    pub dirs: &'a dyn Dirs,
    /// The progress lines of standard output.
    pub say: &'a dyn Say,
    /// The sleep between read-backs of `update`.
    pub clock: &'a dyn Clock,
    /// `recorded_at` and `recorded_by`.
    pub stamps: &'a dyn StampSource,
    /// `suites_cover_production` and `pick_suite`.
    pub selector: &'a dyn SuiteSelector,
    /// Loading a suite.
    pub suites: &'a dyn SuiteHost,
    /// Extraction, mutation and the container.
    pub sandbox: &'a dyn mx_sandbox::Sandbox,
    /// The container runtime on this machine.
    pub runtime: &'a dyn RuntimeProbe,
    /// The verify limits and timeouts of the profile (`VERIFY_TIMEOUT` and friends).
    pub verify: &'a crate::verify_run::VerifySettings,
}

/// The gate: a configuration and the ports it uses.
pub struct Gate<'a> {
    pub(crate) cfg: &'a GateConfig,
    pub(crate) p: Ports<'a>,
}

/// What a refusal ends with: a die line and an exit code.
pub(crate) type R<T> = Result<T, Exit>;

pub(crate) fn die(message: &str) -> Exit {
    Exit::die(1, message)
}

fn lists(names: &[String], who: &str) -> bool {
    names.iter().any(|name| name.eq_ignore_ascii_case(who))
}

fn first_8(text: &str) -> String {
    text.chars().take(8).collect()
}

/// What a successful command leaves on standard output besides its progress lines.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Printed {
    /// Raw bytes for standard output (`show`).
    pub stdout: Vec<u8>,
    /// Text for standard error that is not a refusal (the notes of `pick_suite`).
    pub stderr: String,
}

impl<'a> Gate<'a> {
    /// A gate over `cfg` and `ports`.
    #[must_use]
    pub fn new(cfg: &'a GateConfig, ports: Ports<'a>) -> Self {
        Self { cfg, p: ports }
    }

    /// Runs a parsed command. `Ok` is exit status 0.
    pub fn run(&self, command: Command) -> R<Printed> {
        match command {
            Command::Receipt { pr, sha, proof } => {
                self.receipt(&pr, &sha, &proof).map(|()| Printed::default())
            }
            Command::Verify { part, pr, sha, filter, mutation, suite } => {
                self.verify(part, &pr, &sha, &filter, &mutation, suite.as_ref())
            }
            Command::Merge { pr } => self.merge(&pr).map(|()| Printed::default()),
            Command::Update { pr } => self.update(&pr).map(|()| Printed::default()),
            Command::Show { pr } => self.show(pr.as_ref()),
            Command::VerifyDeps { .. } => {
                Err(die("verify-deps runs through crate::deps::run, wired in app::run"))
            }
        }
    }

    // ----- shared checks ---------------------------------------------------------------

    /// `lease_gate`.
    pub(crate) fn lease_gate(&self, pr: &crate::PrNumber, action: Action) -> R<LeaseFree> {
        LeaseFree::from_answer(pr, action, self.p.helper.lease_check(pr))
            .map_err(|d| Exit::from(&d))
    }

    /// `require_prod_globs`.
    pub(crate) fn prod_globs(&self) -> R<&ProdGlobs> {
        self.cfg.prod_globs.as_ref().ok_or_else(|| Exit::from(&GateDecision::NoProdGlobs))
    }

    /// `identity`.
    fn identity(&self, pr: &crate::PrNumber) -> R<Confirmed> {
        let who = self.p.identity.who_am_i(&self.cfg.account);
        Confirmed::check(pr, who, &self.cfg.account).map_err(|d| Exit::from(&d))
    }

    /// The receipt key, or the refusal `receipt_sign` dies with.
    pub(crate) fn key(&self) -> Result<Key, Exit> {
        match self.p.fs.read(&self.cfg.receipt_key) {
            Fact::Known(bytes) => Key::from_file_bytes(&bytes).map_err(|e| die(&e.to_string())),
            Fact::Absent | Fact::Unreadable(_) => {
                Err(Exit::from(&GateDecision::NoReceiptKey { path: self.cfg.receipt_key.clone() }))
            }
        }
    }

    fn mkdir_receipts(&self) {
        // `mkdir -p "$RECEIPTS"`: the bash ignores a failure here, and the open that follows
        // reports it.
        drop(self.p.dirs.make_dir(&self.cfg.receipts()));
    }

    /// `receipt_check`: is there a signed receipt for the pull request, earned for it, of a
    /// kind this caller may spend?
    pub(crate) fn receipt_check(&self, pr: &crate::PrNumber) -> R<Pr<Receipted>> {
        let path = receipt_path(&self.cfg.receipts(), pr.as_arg());
        let Fact::Known(bytes) = self.p.fs.read(&path) else {
            return Err(Exit::from(&GateDecision::NoReceipt { pr: pr.clone() }));
        };
        let stored = Stored::parse(&bytes)
            .map_err(|_| Exit::from(&GateDecision::ReceiptNotJson { pr: pr.clone() }))?;
        if stored.field_text("signature", "").is_empty() {
            return Err(Exit::from(&GateDecision::ReceiptUnsigned { pr: pr.clone() }));
        }
        let key = match self.key() {
            Ok(key) => key,
            Err(mut exit) => {
                // `receipt_sign` dies inside a command substitution, so its line is printed
                // and the caller then reports the receipt as unreadable (decision P3B-1).
                let tail = Exit::from(&GateDecision::ReceiptNotJson { pr: pr.clone() });
                exit.stderr.push_str(&tail.stderr);
                return Err(exit);
            }
        };
        let receipt = stored.check(&key, pr.as_arg(), self.cfg.in_run).map_err(|error| {
            let decision = match error {
                ReceiptError::Unsigned => GateDecision::ReceiptUnsigned { pr: pr.clone() },
                ReceiptError::SignatureMismatch => {
                    GateDecision::ReceiptSignatureMismatch { pr: pr.clone() }
                }
                ReceiptError::WrongPullRequest { found, .. } => {
                    GateDecision::ReceiptForOtherPr { path: path.clone(), found, pr: pr.clone() }
                }
                ReceiptError::KindNotSpendable { kind } => {
                    GateDecision::ReceiptNotObserved { pr: pr.clone(), kind }
                }
                // A signed receipt of a shape the gate cannot read (decision P3B-2): the bash
                // would read each field on its own and carry on.
                _ => GateDecision::ReceiptNotJson { pr: pr.clone() },
            };
            Exit::from(&decision)
        })?;
        match receipt.body().kind {
            Kind::Observed => {}
            Kind::Dependency => self.p.say.say(&Progress::DependencyReceiptNotice),
            Kind::Asserted | Kind::Other(_) => self.p.say.say(&Progress::AssertedReceiptNotice),
        }
        Pr::open(pr.clone()).replay(receipt, &path).map_err(|d| Exit::from(&d))
    }

    /// `receipt_covers`: does what the receipt was earned by reach every production path?
    pub(crate) fn receipt_covers(&self, pr: &crate::PrNumber, receipt: &Receipt<Signed>) -> R<()> {
        // Only a receipt proved in parts is asked (the bash reads `partial`, default false).
        if !receipt.is_partial() {
            return Ok(());
        }
        if !self.cfg.has_suite_host {
            return Err(die(NO_SUITE_HOST));
        }
        let globs = self.prod_globs()?;
        let files =
            self.p.gh.changed_files(pr).map_err(|_| {
                Exit::from(&GateDecision::ChangedFilesUnreadable { pr: pr.clone() })
            })?;
        let changed = NonEmpty::from_vec(files)
            .map_err(|_| Exit::from(&GateDecision::ChangedFilesEmpty { pr: pr.clone() }))?;
        let suites = receipt.suites();
        self.p.selector.suites_cover_production(globs, &suites, &changed).map_err(|failure| {
            let mut exit = Exit::from(&GateDecision::ReceiptDoesNotCover {
                pr: pr.clone(),
                suites,
                verified_head: receipt.head().to_string(),
            });
            exit.stderr.insert_str(0, &failure.stderr_lines());
            exit
        })
    }

    /// `receipt_fresh`: has production code moved between the verified head and `head`?
    pub(crate) fn receipt_fresh(
        &self,
        pr: &crate::PrNumber,
        head: &CommitId,
        verified_head: &str,
    ) -> R<Fresh> {
        let globs = self.prod_globs()?;
        let repo = self.cfg.repo.as_path();
        let branch = format!("mm-{pr}-{}", self.cfg.run_id);
        self.p
            .git
            .fetch_ref(repo, &format!("refs/pull/{pr}/head:{branch}"))
            .map_err(|_| Exit::from(&GateDecision::CannotFetch { pr: pr.clone() }))?;
        // The ref may have moved since `head` was read (a push, or another fetch). The freshness
        // check must be about the commit the merge is pinned to, so the ref is resolved to an
        // object id and everything below names that commit, never the ref (decision P3B-11).
        let fetched = self.p.git.rev_parse(repo, &branch);
        if fetched.as_deref().map(str::trim) != Ok(head.as_str()) {
            self.p.git.delete_branch(repo, &branch);
            let found = fetched.unwrap_or_else(|e| format!("unreadable ({e})"));
            return Err(die(&format!(
                "the head of #{pr} moved while I was reading it: GitHub said {head}, the fetch holds {found}.\n  Nothing was checked against a commit I did not read. Run it again."
            )));
        }
        let (how, diff): (Option<&'static str>, Result<String, DiffFailure>) =
            if head.as_str() == verified_head {
                (None, Ok(String::new()))
            } else {
                self.p.git.fetch_main(repo);
                let base = self.p.git.merge_base(repo, head.as_str(), "origin/main");
                let merged = base
                    .as_deref()
                    .filter(|base| !self.p.git.is_ancestor(repo, verified_head, base))
                    .and_then(|base| self.p.git.merge_tree(repo, base, verified_head));
                match merged {
                    Some(tree) => (
                        Some("the tree is the clean merge of the verified head with main"),
                        self.p.git.diff_names(repo, &tree, head.as_str(), globs.pathspecs()),
                    ),
                    None => (
                        Some("no production diff"),
                        self.p.git.diff_names(
                            repo,
                            &format!("{verified_head}..{head}"),
                            "",
                            globs.pathspecs(),
                        ),
                    ),
                }
            };
        let outcome = ProductionDiff::from_git(pr, verified_head, diff)
            .and_then(|diff| Fresh::from_diff(&diff, head));
        self.p.git.delete_branch(repo, &branch);
        let fresh = outcome.map_err(|d| Exit::from(&d))?;
        if let Some(how) = how {
            self.p.say.say(&Progress::HeadMoved {
                verified_head: verified_head.to_string(),
                head_8: head.short().to_string(),
                how: how.to_string(),
            });
        }
        Ok(fresh)
    }

    // ----- receipt ------------------------------------------------------------------------

    /// `cmd_receipt`: records an asserted receipt.
    pub fn receipt(&self, pr: &crate::PrNumber, sha: &HeadArg, proof: &str) -> R<()> {
        if self.cfg.in_run {
            return Err(die(UNATTENDED_ASSERT));
        }
        self.mkdir_receipts();
        let receipt =
            Receipt::asserted(pr.value(), sha.as_str(), proof, &self.p.stamps.stamp(), false)
                .map_err(|e| die(&e.to_string()))?;
        let key = self.key()?;
        let signed = receipt.sign(&key);
        mx_receipt::write(self.p.fs, &receipt_path(&self.cfg.receipts(), pr.as_arg()), &signed)
            .map_err(|_| {
                die(&format!("could not record the receipt (is '{pr}' a pull request number?)"))
            })?;
        self.p.say.say(&Progress::AssertedReceiptRecorded {
            pr: pr.clone(),
            sha: sha.as_str().to_string(),
        });
        Ok(())
    }

    // ----- show -----------------------------------------------------------------------------

    /// `cmd_show`. The number was validated by [`crate::PrNumber`] (F2).
    pub fn show(&self, pr: Option<&crate::PrNumber>) -> R<Printed> {
        self.mkdir_receipts();
        let dir = self.cfg.receipts();
        if let Some(pr) = pr {
            let bytes = crate::show_receipt(self.p.fs, &dir, pr).map_err(|_| {
                Exit::from(&crate::VerifyRefusal::NoReceiptToShow { arg1: pr.as_arg().to_string() })
            })?;
            return Ok(Printed { stdout: bytes, stderr: String::new() });
        }
        let entries = mx_receipt::show_list(self.p.fs, &dir)
            .map_err(|e| die(&format!("cannot list {}: {e}", dir.display())))?;
        if entries.is_empty() {
            self.p.say.say(&Progress::NoReceiptsRecorded);
            return Ok(Printed::default());
        }
        let mut out = String::new();
        for entry in entries {
            match entry {
                ListEntry::Line(line) => out.push_str(&line),
                // The bash prints `  <pr>: ` and python's traceback goes to stderr (P3B-3).
                ListEntry::Unreadable { file, .. } => {
                    out.push_str("  ");
                    out.push_str(file.strip_suffix(".json").unwrap_or(&file));
                    out.push_str(": ");
                }
            }
            out.push('\n');
        }
        Ok(Printed { stdout: out.into_bytes(), stderr: String::new() })
    }

    // ----- merge ------------------------------------------------------------------------------

    /// The claim checks of `cmd_merge`: a pull request may not close an issue somebody else holds.
    fn claims(&self, pr: &crate::PrNumber) -> R<(ClaimsClear, bool)> {
        let closes = self.p.gh.closing_issues(pr).map_err(|f| {
            Exit::from(&GateDecision::ClosesUnreadable { pr: pr.clone(), output: f.output })
        })?;
        let mut author = String::new();
        if !closes.is_empty() {
            author = match self.p.gh.author(pr) {
                Ok(name) if !name.is_empty() => name,
                Ok(_) => {
                    return Err(Exit::from(&GateDecision::AuthorUnreadable {
                        pr: pr.clone(),
                        output: String::new(),
                    }));
                }
                Err(f) => {
                    return Err(Exit::from(&GateDecision::AuthorUnreadable {
                        pr: pr.clone(),
                        output: f.output,
                    }));
                }
            };
        }
        for issue in &closes {
            let holders = self.p.helper.holders(*issue).map_err(|failure| {
                Exit::from(&match failure {
                    HoldersFailure::HelperMissing => {
                        GateDecision::HoldersHelperMissing { pr: pr.clone(), issue: *issue }
                    }
                    HoldersFailure::Failed { output } => {
                        GateDecision::HoldersUnreadable { pr: pr.clone(), issue: *issue, output }
                    }
                })
            })?;
            if holders.is_empty() || lists(&holders, &author) {
                continue;
            }
            return Err(Exit::from(&GateDecision::IssuePointedAtOthers {
                pr: pr.clone(),
                issue: *issue,
                holders,
                author,
            }));
        }
        if !self.cfg.claim_label.is_empty() {
            for issue in &closes {
                self.label_claim(pr, *issue, &author)?;
            }
        }
        Ok((ClaimsClear::new(pr.clone()), closes.is_empty()))
    }

    fn label_claim(&self, pr: &crate::PrNumber, issue: IssueNumber, author: &str) -> R<()> {
        let label = &self.cfg.claim_label;
        let labelled = self.p.gh.issue_has_label(issue, label).map_err(|f| {
            Exit::from(&GateDecision::LabelsUnreadable {
                pr: pr.clone(),
                issue,
                claim_label: label.clone(),
                output: f.output,
            })
        })?;
        if !labelled {
            return Ok(());
        }
        let assignees = self.p.gh.issue_assignees(issue).map_err(|f| {
            Exit::from(&GateDecision::AssigneesUnreadable {
                pr: pr.clone(),
                issue,
                output: f.output,
            })
        })?;
        if !assignees.is_empty() {
            if lists(&assignees, author) {
                return Ok(());
            }
            return Err(Exit::from(&GateDecision::ClaimedAssignedToOthers {
                pr: pr.clone(),
                issue,
                claim_label: label.clone(),
                assignees,
                author: author.to_string(),
            }));
        }
        let commenters = self.p.gh.issue_commenters(issue).map_err(|f| {
            Exit::from(&GateDecision::CommentersUnreadable {
                pr: pr.clone(),
                issue,
                output: f.output,
            })
        })?;
        let account = self.cfg.account.as_str();
        let others: Vec<String> = commenters
            .into_iter()
            .filter(|name| {
                !name.eq_ignore_ascii_case(account) && !name.eq_ignore_ascii_case(author)
            })
            .collect();
        if others.is_empty() {
            return Ok(());
        }
        Err(Exit::from(&GateDecision::ClaimedWithSecondVoice {
            pr: pr.clone(),
            issue,
            claim_label: label.clone(),
            others,
            author: author.to_string(),
        }))
    }

    /// `cmd_merge`.
    pub fn merge(&self, pr: &crate::PrNumber) -> R<()> {
        let lease = self.lease_gate(pr, Action::Merge)?;
        self.prod_globs()?;
        let identity = self.identity(pr)?;

        self.mkdir_receipts();
        let lock_file = LockKind::Merge.file(&self.cfg.receipts(), pr);
        let _lock = self.p.locks.try_lock(&lock_file).map_err(|error| {
            Exit::from(&match error {
                LockError::Unopenable => GateDecision::MergeLockUnopenable { pr: pr.clone() },
                LockError::Held => {
                    GateDecision::MergeLockHeld { pr: pr.clone(), lock: lock_file.clone() }
                }
            })
        })?;

        let receipted = self.receipt_check(pr)?;
        self.receipt_covers(pr, receipted.receipt())?;
        let verified_head = receipted.receipt().head().to_string();

        let head_text = self
            .p
            .gh
            .head(pr)
            .map_err(|_| Exit::from(&GateDecision::HeadUnreadable { pr: pr.clone() }))?;
        let head = CommitId::parse(&head_text).map_err(|_| {
            Exit::from(&GateDecision::HeadNotACommit { pr: pr.clone(), head: head_text.clone() })
        })?;
        // A failed read of the next two is an empty string in the bash, and each refuses by name.
        let state = self.p.gh.merge_state(pr).unwrap_or_default();
        let review = self.p.gh.review_decision(pr).unwrap_or_default();

        let approved = Approved::from_decision(pr, &review).map_err(|d| Exit::from(&d))?;
        let (claims, closes_nothing) = self.claims(pr)?;
        let state_clear = MergeState::parse(&state).clear(pr).map_err(|d| Exit::from(&d))?;

        let checks = self.p.gh.checks(pr).unwrap_or_default();
        let board = CheckBoard::from_gh_json(pr, &checks)
            .and_then(|board| board.green(pr))
            .map_err(|d| Exit::from(&d))?;

        let fresh = self.receipt_fresh(pr, &head, &verified_head)?;

        self.p.say.say(&Progress::AllConditionsHold {
            pr: pr.clone(),
            pass_n: board.pass().to_string(),
            skip_n: board.skip().to_string(),
        });
        let conditions =
            Conditions::new(lease, identity, approved, claims, state_clear, board, fresh, head)
                .map_err(|d| Exit::from(&d))?;
        let mergeable = receipted.cleared(conditions).map_err(|d| Exit::from(&d))?;

        let Some(merger) = Merger::new(self.cfg.posting, self.p.gh) else {
            self.p.say.say(&Progress::RehearsalMergeNotMade);
            self.p.say.say(&Progress::RehearsalMergeHint);
            return Ok(());
        };
        merger.merge(mergeable).map_err(|d| Exit::from(&d))?;
        self.p.say.say(&Progress::Merged { pr: pr.clone() });
        if closes_nothing {
            self.p.say.say(&Progress::ClosesNothing { pr: pr.clone() });
            self.p.say.say(&Progress::ClosesNothingHint);
        }
        drop(self.p.fs.remove(&receipt_path(&self.cfg.receipts(), pr.as_arg())));
        Ok(())
    }

    // ----- update -------------------------------------------------------------------------------

    /// Did this account's latest standing verdict approve exactly `head`?
    fn approved_by_me(&self, pr: &crate::PrNumber, head: &str) -> Result<bool, GhFailure> {
        let reviews = self.p.gh.reviews(pr)?;
        let me = self.cfg.account.as_str();
        let mut verdict: Option<(&str, Option<&str>)> = None;
        for review in &reviews {
            if !review.user.eq_ignore_ascii_case(me) {
                continue;
            }
            if matches!(review.state.as_str(), "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED") {
                verdict = Some((review.state.as_str(), review.commit_id.as_deref()));
            }
        }
        Ok(verdict == Some(("APPROVED", Some(head))))
    }

    fn readback_number(name: &str, value: Option<&String>) -> R<f64> {
        let unset = || die(&format!("{name}: lib/defaults.env was not loaded"));
        let text = value.filter(|v| !v.is_empty()).ok_or_else(unset)?;
        text.parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .ok_or_else(|| die(&format!("{name} is '{text}', which is not a number")))
    }

    /// `cmd_update`.
    #[allow(clippy::too_many_lines, reason = "one function per bash command, in its order")]
    pub fn update(&self, pr: &crate::PrNumber) -> R<()> {
        self.lease_gate(pr, Action::Update)?;
        self.prod_globs()?;
        self.identity(pr)?;

        let refuse = |r: UpdateRefusal| Exit::from(&r);
        // The bash helper prints `key=value` lines until the first bad value, so the refusal
        // names the first key it could not read; an adapter names it in `output`.
        let facts = self.p.gh.update_facts(pr).map_err(|failure| {
            let k = match failure.output.as_str() {
                key @ ("head" | "state" | "decision" | "author") => key.to_string(),
                _ => "head".to_string(),
            };
            refuse(UpdateRefusal::FactUnreadable { k, pr: pr.clone() })
        })?;
        let head = CommitId::parse_full(&facts.head).map_err(|_| {
            refuse(UpdateRefusal::HeadNotFull { pr: pr.clone(), head: facts.head.clone() })
        })?;

        match &facts.state {
            MergeState::Behind => {}
            MergeState::Clean | MergeState::HasHooks => {
                self.p.say.say(&Progress::NothingToUpdate {
                    pr: pr.clone(),
                    state: facts.state.as_str().to_string(),
                });
                return Ok(());
            }
            MergeState::Dirty => return Err(refuse(UpdateRefusal::Conflicts { pr: pr.clone() })),
            other => {
                return Err(refuse(UpdateRefusal::StateNotBehind {
                    pr: pr.clone(),
                    state: other.as_str().to_string(),
                }));
            }
        }
        if matches!(
            facts.author.to_lowercase().as_str(),
            "app/dependabot" | "dependabot" | "dependabot[bot]" | "app/dependabot[bot]"
        ) {
            return Err(refuse(UpdateRefusal::Dependabots { pr: pr.clone() }));
        }
        if facts.decision != "APPROVED" {
            return Err(refuse(UpdateRefusal::NotApprovedForUpdate {
                pr: pr.clone(),
                decision: facts.decision.clone(),
            }));
        }
        let account = self.cfg.account.as_str().to_string();
        match self.approved_by_me(pr, head.as_str()) {
            Ok(true) => {}
            Ok(false) => {
                return Err(refuse(UpdateRefusal::NoStandingApproval {
                    account,
                    pr: pr.clone(),
                    head_8: head.short().to_string(),
                }));
            }
            Err(_) => {
                return Err(refuse(UpdateRefusal::ReviewsUnreadable {
                    pr: pr.clone(),
                    account,
                    head_8: head.short().to_string(),
                }));
            }
        }

        let receipted = self.receipt_check(pr)?;
        self.receipt_covers(pr, receipted.receipt())?;
        let receipt_head = receipted.receipt().head().to_string();
        self.receipt_fresh(pr, &head, &receipt_head)?;

        if facts.cross_repository && !facts.maintainer_can_modify {
            return Err(refuse(UpdateRefusal::NeedsAuthorNoEdits { pr: pr.clone() }));
        }
        self.p.say.say(&Progress::UpdateConditionsHold {
            pr: pr.clone(),
            account: self.cfg.account.as_str().to_string(),
            head_8: head.short().to_string(),
        });
        if self.cfg.posting == Posting::Off {
            self.p.say.say(&Progress::RehearsalUpdateNotMade);
            self.p.say.say(&Progress::RehearsalUpdateWouldRun {
                repo_slug: self.cfg.slug.clone(),
                pr: pr.clone(),
                head: head.to_string(),
            });
            return Ok(());
        }

        if let Err(failure) = self.p.gh.update_branch(pr, &head) {
            let put = failure.output;
            return Err(refuse(if put.contains("expected head sha") {
                UpdateRefusal::HeadMoved { pr: pr.clone(), put }
            } else if put.contains("(HTTP 403)") || put.contains("permission to update") {
                UpdateRefusal::NeedsAuthorNoPush { pr: pr.clone(), put }
            } else {
                UpdateRefusal::GithubRefused { pr: pr.clone(), put }
            }));
        }

        // The 202 means "accepted", not "moved": read it back.
        let reads_text = self.cfg.readback_reads.clone();
        let reads_f = Self::readback_number("UPDATE_READBACK_READS", reads_text.as_ref())?;
        let wait = Self::readback_number("UPDATE_READBACK_WAIT", self.cfg.readback_wait.as_ref())?;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "validated finite and non-negative"
        )]
        let reads = reads_f.floor() as u64;
        let mut nhead = String::new();
        let mut nstate = String::new();
        let mut ndecision = String::new();
        let mut confirmed = false;
        for i in 1..=reads {
            if i != 1 {
                self.p.clock.sleep(Duration::from_secs_f64(wait));
            }
            let Ok(now) = self.p.gh.update_facts(pr) else {
                nhead.clear();
                continue;
            };
            nhead.clone_from(&now.head);
            nstate = now.state.as_str().to_string();
            ndecision.clone_from(&now.decision);
            if nhead.is_empty() || nhead == head.as_str() {
                continue;
            }
            if matches!(nstate.as_str(), "BEHIND" | "UNKNOWN" | "") {
                continue;
            }
            confirmed = true;
            break;
        }
        if !confirmed {
            return Err(refuse(UpdateRefusal::NotConfirmed {
                pr: pr.clone(),
                reads: reads_text.unwrap_or_default(),
                nhead_8: first_8(&nhead),
                nstate: if nstate.is_empty() { "unread".to_string() } else { nstate },
            }));
        }
        self.p.say.say(&Progress::Updated {
            pr: pr.clone(),
            nhead_8: first_8(&nhead),
            head_8: head.short().to_string(),
            nstate,
        });
        self.p.say.say(&Progress::ChecksMustRerun { pr: pr.clone() });
        if ndecision == "APPROVED" {
            self.p.say.say(&Progress::ApprovalKept);
        } else {
            self.p.say.say(&Progress::ApprovalDismissed {
                ndecision: if ndecision.is_empty() { "empty".to_string() } else { ndecision },
            });
            self.p.say.say(&Progress::RepostReview1 { nhead_8: first_8(&nhead) });
            self.p.say.say(&Progress::RepostReview2 {
                receipt_head_8: first_8(&receipt_head),
                nhead_8: first_8(&nhead),
            });
            self.p.say.say(&Progress::RepostReview3);
        }
        Ok(())
    }
}
