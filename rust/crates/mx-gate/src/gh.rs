//! The `gh` port of the merge gate, and its fake.

use crate::{CommitId, IssueNumber, MergeState, PrFacts, PrNumber, SpendToken};
use mx_core::Fact;
use mx_ports::Script;
use mx_suite::Recorder;
use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// A `gh` call that did not succeed. `output` is what the call printed, standard
/// output and standard error together where the bash reads them together
/// (`2>&1`), because the refusal quotes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhFailure {
    /// The exit status of `gh`, or 127 when it could not be started.
    pub exit: i32,
    pub output: String,
}

impl fmt::Display for GhFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "gh exited {}: {}", self.exit, self.output)
    }
}

/// The fields `update` reads in one `gh pr view` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFacts {
    /// `headRefOid`, as `gh` printed it. The caller checks it with
    /// [`CommitId::parse_full`].
    pub head: String,
    pub state: MergeState,
    pub decision: String,
    pub author: String,
    /// `isCrossRepository`: the head lives in a fork.
    pub cross_repository: bool,
    /// `maintainerCanModify`, true only when GitHub says `true`.
    pub maintainer_can_modify: bool,
}

/// One review of a pull request: `gh api --paginate repos/<slug>/pulls/<n>/reviews --jq '.[]'`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub user: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `DISMISSED`, `COMMENTED`, ...
    pub state: String,
    pub commit_id: Option<String>,
}

/// Everything `maintainer-merge` asks GitHub, one method per `gh` call (decision `P30-13`).
///
/// The real adapter (the gate agent's `RealGh`) starts `gh` through
/// [`mx_ports::Proc`], with the argv the bash uses, in the order the bash makes
/// the calls: the stubs of `tests/` match on `"$*"`, and a refusal in the middle
/// must stop the later calls, so one `PrFacts`-shaped read up front would change
/// both. Each method says its bash call.
///
/// Every method returns `Result`, and an `Err` is "the question was not
/// answered": the bash dies on it, or, where it did not, the decision code says
/// so. Raw text stays raw (`head`, `merge_state`, `review_decision`, `author`)
/// so the caller's refusal can quote what came back.
pub trait GhPort {
    /// `gh pr list --repo R --state open --json number --jq '.[].number'`.
    /// No bash call of `maintainer-merge` lists pull requests (the Python helper
    /// does); the method exists for the port's own checks.
    fn list_open(&self) -> Result<Vec<PrNumber>, GhFailure>;

    /// `gh pr view N --repo R --json files --jq '.files[].path'`: one path per
    /// line. An empty list is returned as it is; the caller refuses it.
    fn changed_files(&self, pr: &PrNumber) -> Result<Vec<String>, GhFailure>;

    /// `gh pr view N --repo R --json headRefOid --jq .headRefOid`, as printed.
    fn head(&self, pr: &PrNumber) -> Result<String, GhFailure>;

    /// `gh pr view N --repo R --json baseRefName --jq .baseRefName`, as printed.
    fn base_ref(&self, pr: &PrNumber) -> Result<String, GhFailure>;

    /// `gh pr view N --repo R --json mergeStateStatus --jq .mergeStateStatus`.
    /// The bash does not check this read's status (a failed read is an empty
    /// state, and an empty state refuses); `Err` here means the same.
    fn merge_state(&self, pr: &PrNumber) -> Result<String, GhFailure>;

    /// `gh pr view N --repo R --json reviewDecision --jq .reviewDecision`.
    fn review_decision(&self, pr: &PrNumber) -> Result<String, GhFailure>;

    /// `gh pr view N --repo R --json author --jq .author.login`, status and text
    /// read together (`2>&1`). An empty login is the caller's refusal.
    fn author(&self, pr: &PrNumber) -> Result<String, GhFailure>;

    /// `gh pr view N --repo R --json closingIssuesReferences --jq
    /// '.closingIssuesReferences[].number'` with `2>&1`.
    fn closing_issues(&self, pr: &PrNumber) -> Result<Vec<IssueNumber>, GhFailure>;

    /// `gh pr checks N --repo R --json name,bucket 2>/dev/null`: what it printed,
    /// WHATEVER its exit status. `gh` exits 8 while a check is pending and still
    /// prints the list, so the status is not an answer. Parse it with
    /// [`crate::CheckBoard::from_gh_json`].
    fn checks(&self, pr: &PrNumber) -> Result<String, GhFailure>;

    /// `gh issue view I --repo R --json labels --jq '[.labels[].name]|index("<label>")'`
    /// with `2>&1`: `Ok(true)` when the answer is a number, `Ok(false)` for the empty
    /// answer or `null`, `Err` when `gh` failed. The label is profile text and
    /// goes into a `jq` program: the adapter must put it in as a string argument
    /// and never splice it into the program text.
    fn issue_has_label(&self, issue: IssueNumber, label: &str) -> Result<bool, GhFailure>;

    /// `gh issue view I --repo R --json assignees --jq '.assignees[].login'` with `2>&1`.
    fn issue_assignees(&self, issue: IssueNumber) -> Result<Vec<String>, GhFailure>;

    /// `gh issue view I --repo R --json comments --jq '[.comments[].author.login]|unique|.[]'` with `2>&1`.
    fn issue_commenters(&self, issue: IssueNumber) -> Result<Vec<String>, GhFailure>;

    /// `gh pr view N --repo R --json headRefOid,mergeStateStatus,reviewDecision,author,isCrossRepository,maintainerCanModify`
    /// (`update_facts`). A field that is not a one-line string is `Err`, as in the bash.
    fn update_facts(&self, pr: &PrNumber) -> Result<UpdateFacts, GhFailure>;

    /// `gh api --paginate repos/R/pulls/N/reviews --jq '.[]'` (`update_approved_by_me`).
    fn reviews(&self, pr: &PrNumber) -> Result<Vec<Review>, GhFailure>;

    /// `gh api -X PUT repos/R/pulls/N/update-branch -f expected_head_sha=<head>`
    /// with `2>&1`. GitHub answers 422 when the head moved since it was read.
    fn update_branch(&self, pr: &PrNumber, expected_head: &CommitId) -> Result<(), GhFailure>;

    /// `gh pr merge N --repo R --squash --delete-branch --match-head-commit <head>`.
    /// Pinned to the head that was checked: without it a push landing between the
    /// production diff and this line is merged unchecked. The head is a
    /// [`CommitId`], so an unreadable head cannot reach here as an empty pin.
    /// `gh pr merge`. Spending: it takes a [`SpendToken`], which only `Merger::merge` can make, so
    /// no code holding a port can merge without a `Pr<Mergeable>` (review rv271 F3).
    fn merge(&self, pr: &PrNumber, head: &CommitId, spend: SpendToken) -> Result<(), GhFailure>;

    /// `gh pr diff N --repo R` with `2>&1`: the net diff, never `--patch`. An mbox
    /// carries each commit message, which the author writes, and a message can
    /// hold a forged `diff --git` and hunk.
    fn diff(&self, pr: &PrNumber) -> Result<String, GhFailure>;
}

/// One call to a [`FakeGh`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhCall {
    ListOpen,
    ChangedFiles(PrNumber),
    Head(PrNumber),
    BaseRef(PrNumber),
    MergeState(PrNumber),
    ReviewDecision(PrNumber),
    Author(PrNumber),
    ClosingIssues(PrNumber),
    Checks(PrNumber),
    IssueHasLabel { issue: IssueNumber, label: String },
    IssueAssignees(IssueNumber),
    IssueCommenters(IssueNumber),
    UpdateFacts(PrNumber),
    Reviews(PrNumber),
    UpdateBranch { pr: PrNumber, expected_head: CommitId },
    Merge { pr: PrNumber, head: CommitId },
    Diff(PrNumber),
}

/// The scripted answers to one method.
#[derive(Debug)]
struct Slot<T: Clone>(Mutex<Script<T>>);

impl<T: Clone> Default for Slot<T> {
    fn default() -> Self {
        Self(Mutex::new(Script::none()))
    }
}

impl<T: Clone> Slot<T> {
    fn guard(&self) -> MutexGuard<'_, Script<T>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set(&self, script: Script<T>) {
        *self.guard() = script;
    }

    fn next(&self, call: &str) -> T {
        self.guard().next(call)
    }
}

type Answer<T> = Result<T, GhFailure>;

/// A scripted [`GhPort`] that records its calls. A call nobody scripted panics.
#[derive(Debug, Default)]
pub struct FakeGh {
    calls: Recorder<GhCall>,
    list_open: Slot<Answer<Vec<PrNumber>>>,
    changed_files: Slot<Answer<Vec<String>>>,
    head: Slot<Answer<String>>,
    base_ref: Slot<Answer<String>>,
    merge_state: Slot<Answer<String>>,
    review_decision: Slot<Answer<String>>,
    author: Slot<Answer<String>>,
    closing_issues: Slot<Answer<Vec<IssueNumber>>>,
    checks: Slot<Answer<String>>,
    issue_has_label: Slot<Answer<bool>>,
    issue_assignees: Slot<Answer<Vec<String>>>,
    issue_commenters: Slot<Answer<Vec<String>>>,
    update_facts: Slot<Answer<UpdateFacts>>,
    reviews: Slot<Answer<Vec<Review>>>,
    update_branch: Slot<Answer<()>>,
    merge: Slot<Answer<()>>,
    diff: Slot<Answer<String>>,
}

/// How a [`Fact`] reads through a port that answers with text: known is the
/// text, absent is the empty answer, and unreadable is a failed call.
fn text_of(fact: &Fact<String>) -> Answer<String> {
    match fact {
        Fact::Known(text) => Ok(text.clone()),
        Fact::Absent => Ok(String::new()),
        Fact::Unreadable(reason) => Err(GhFailure { exit: 1, output: reason.clone() }),
    }
}

impl FakeGh {
    /// A fake with no scripted answers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A fake that serves one pull request's facts for the read methods:
    /// `head`, `base_ref`, `merge_state`, `review_decision`, `author`,
    /// `changed_files` and `checks`. A known fact is its text, an absent one is
    /// the empty answer, and an unreadable one is a failed call. Every answer
    /// repeats. The write methods and the issue reads stay unscripted.
    #[must_use]
    pub fn from_facts(facts: &PrFacts) -> Self {
        let fake = Self::new();
        fake.script_head(Script::always(text_of(&facts.head.clone().map(|h| h.to_string()))));
        fake.script_base_ref(Script::always(text_of(&facts.base)));
        fake.script_merge_state(Script::always(text_of(
            &facts.merge_state.clone().map(|s| s.as_str().to_string()),
        )));
        fake.script_review_decision(Script::always(text_of(&facts.review_decision)));
        fake.script_author(Script::always(text_of(&facts.author)));
        fake.script_changed_files(Script::always(match &facts.changed_files {
            Fact::Known(paths) => Ok(paths.as_slice().to_vec()),
            Fact::Absent => Ok(Vec::new()),
            Fact::Unreadable(reason) => Err(GhFailure { exit: 1, output: reason.clone() }),
        }));
        fake.script_checks(Script::always(match &facts.checks {
            Fact::Known(checks) => {
                let list: Vec<serde_json::Value> = checks
                    .iter()
                    .map(|c| serde_json::json!({"name": c.name, "bucket": c.bucket}))
                    .collect();
                Ok(serde_json::Value::Array(list).to_string())
            }
            Fact::Absent => Ok(String::new()),
            Fact::Unreadable(reason) => Err(GhFailure { exit: 8, output: reason.clone() }),
        }));
        fake
    }

    /// Every call so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<GhCall> {
        self.calls.entries()
    }

    /// Scripts `list_open`.
    pub fn script_list_open(&self, s: Script<Answer<Vec<PrNumber>>>) {
        self.list_open.set(s);
    }

    /// Scripts `changed_files`.
    pub fn script_changed_files(&self, s: Script<Answer<Vec<String>>>) {
        self.changed_files.set(s);
    }

    /// Scripts `head`.
    pub fn script_head(&self, s: Script<Answer<String>>) {
        self.head.set(s);
    }

    /// Scripts `base_ref`.
    pub fn script_base_ref(&self, s: Script<Answer<String>>) {
        self.base_ref.set(s);
    }

    /// Scripts `merge_state`.
    pub fn script_merge_state(&self, s: Script<Answer<String>>) {
        self.merge_state.set(s);
    }

    /// Scripts `review_decision`.
    pub fn script_review_decision(&self, s: Script<Answer<String>>) {
        self.review_decision.set(s);
    }

    /// Scripts `author`.
    pub fn script_author(&self, s: Script<Answer<String>>) {
        self.author.set(s);
    }

    /// Scripts `closing_issues`.
    pub fn script_closing_issues(&self, s: Script<Answer<Vec<IssueNumber>>>) {
        self.closing_issues.set(s);
    }

    /// Scripts `checks`.
    pub fn script_checks(&self, s: Script<Answer<String>>) {
        self.checks.set(s);
    }

    /// Scripts `issue_has_label`.
    pub fn script_issue_has_label(&self, s: Script<Answer<bool>>) {
        self.issue_has_label.set(s);
    }

    /// Scripts `issue_assignees`.
    pub fn script_issue_assignees(&self, s: Script<Answer<Vec<String>>>) {
        self.issue_assignees.set(s);
    }

    /// Scripts `issue_commenters`.
    pub fn script_issue_commenters(&self, s: Script<Answer<Vec<String>>>) {
        self.issue_commenters.set(s);
    }

    /// Scripts `update_facts`.
    pub fn script_update_facts(&self, s: Script<Answer<UpdateFacts>>) {
        self.update_facts.set(s);
    }

    /// Scripts `reviews`.
    pub fn script_reviews(&self, s: Script<Answer<Vec<Review>>>) {
        self.reviews.set(s);
    }

    /// Scripts `update_branch`.
    pub fn script_update_branch(&self, s: Script<Answer<()>>) {
        self.update_branch.set(s);
    }

    /// Scripts `merge`.
    pub fn script_merge(&self, s: Script<Answer<()>>) {
        self.merge.set(s);
    }

    /// Scripts `diff`.
    pub fn script_diff(&self, s: Script<Answer<String>>) {
        self.diff.set(s);
    }
}

impl GhPort for FakeGh {
    fn list_open(&self) -> Answer<Vec<PrNumber>> {
        self.calls.push(GhCall::ListOpen);
        self.list_open.next("GhPort::list_open")
    }

    fn changed_files(&self, pr: &PrNumber) -> Answer<Vec<String>> {
        self.calls.push(GhCall::ChangedFiles(pr.clone()));
        self.changed_files.next("GhPort::changed_files")
    }

    fn head(&self, pr: &PrNumber) -> Answer<String> {
        self.calls.push(GhCall::Head(pr.clone()));
        self.head.next("GhPort::head")
    }

    fn base_ref(&self, pr: &PrNumber) -> Answer<String> {
        self.calls.push(GhCall::BaseRef(pr.clone()));
        self.base_ref.next("GhPort::base_ref")
    }

    fn merge_state(&self, pr: &PrNumber) -> Answer<String> {
        self.calls.push(GhCall::MergeState(pr.clone()));
        self.merge_state.next("GhPort::merge_state")
    }

    fn review_decision(&self, pr: &PrNumber) -> Answer<String> {
        self.calls.push(GhCall::ReviewDecision(pr.clone()));
        self.review_decision.next("GhPort::review_decision")
    }

    fn author(&self, pr: &PrNumber) -> Answer<String> {
        self.calls.push(GhCall::Author(pr.clone()));
        self.author.next("GhPort::author")
    }

    fn closing_issues(&self, pr: &PrNumber) -> Answer<Vec<IssueNumber>> {
        self.calls.push(GhCall::ClosingIssues(pr.clone()));
        self.closing_issues.next("GhPort::closing_issues")
    }

    fn checks(&self, pr: &PrNumber) -> Answer<String> {
        self.calls.push(GhCall::Checks(pr.clone()));
        self.checks.next("GhPort::checks")
    }

    fn issue_has_label(&self, issue: IssueNumber, label: &str) -> Answer<bool> {
        self.calls.push(GhCall::IssueHasLabel { issue, label: label.to_string() });
        self.issue_has_label.next("GhPort::issue_has_label")
    }

    fn issue_assignees(&self, issue: IssueNumber) -> Answer<Vec<String>> {
        self.calls.push(GhCall::IssueAssignees(issue));
        self.issue_assignees.next("GhPort::issue_assignees")
    }

    fn issue_commenters(&self, issue: IssueNumber) -> Answer<Vec<String>> {
        self.calls.push(GhCall::IssueCommenters(issue));
        self.issue_commenters.next("GhPort::issue_commenters")
    }

    fn update_facts(&self, pr: &PrNumber) -> Answer<UpdateFacts> {
        self.calls.push(GhCall::UpdateFacts(pr.clone()));
        self.update_facts.next("GhPort::update_facts")
    }

    fn reviews(&self, pr: &PrNumber) -> Answer<Vec<Review>> {
        self.calls.push(GhCall::Reviews(pr.clone()));
        self.reviews.next("GhPort::reviews")
    }

    fn update_branch(&self, pr: &PrNumber, expected_head: &CommitId) -> Answer<()> {
        self.calls
            .push(GhCall::UpdateBranch { pr: pr.clone(), expected_head: expected_head.clone() });
        self.update_branch.next("GhPort::update_branch")
    }

    fn merge(&self, pr: &PrNumber, head: &CommitId, _spend: SpendToken) -> Answer<()> {
        self.calls.push(GhCall::Merge { pr: pr.clone(), head: head.clone() });
        self.merge.next("GhPort::merge")
    }

    fn diff(&self, pr: &PrNumber) -> Answer<String> {
        self.calls.push(GhCall::Diff(pr.clone()));
        self.diff.next("GhPort::diff")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Check, CheckBoard, PrState};
    use mx_core::NonEmpty;

    fn pr(n: &str) -> PrNumber {
        PrNumber::parse(n).unwrap()
    }

    fn sha() -> CommitId {
        CommitId::parse("0123456789abcdef0123456789abcdef01234567").unwrap()
    }

    fn facts() -> PrFacts {
        PrFacts {
            number: pr("7"),
            head: Fact::Known(sha()),
            base: Fact::Known("main".into()),
            state: Fact::Known(PrState::Open),
            merge_state: Fact::Known(MergeState::Clean),
            review_decision: Fact::Known("APPROVED".into()),
            author: Fact::Known("alice".into()),
            checks: Fact::Known(vec![
                Check { name: "rust".into(), bucket: "pass".into() },
                Check { name: "docs".into(), bucket: "skipping".into() },
            ]),
            labels: Fact::Known(vec!["claimed".into()]),
            changed_files: Fact::Known(NonEmpty::new("src/lib.rs".to_string(), [])),
        }
    }

    #[test]
    fn a_scripted_answer_is_returned_and_the_call_is_recorded() {
        let gh = FakeGh::new();
        gh.script_head(Script::always(Ok(sha().to_string())));
        assert_eq!(gh.head(&pr("7")).unwrap(), sha().as_str());
        assert_eq!(gh.calls(), vec![GhCall::Head(pr("7"))]);
    }

    #[test]
    fn a_failure_carries_the_exit_status_and_what_gh_printed() {
        let gh = FakeGh::new();
        gh.script_closing_issues(Script::always(Err(GhFailure {
            exit: 1,
            output: "GraphQL: Resource not accessible".into(),
        })));
        let failure = gh.closing_issues(&pr("7")).unwrap_err();
        assert_eq!(failure.exit, 1);
        assert_eq!(failure.to_string(), "gh exited 1: GraphQL: Resource not accessible");
    }

    #[test]
    fn a_retry_sequence_plays_out_in_order() {
        let gh = FakeGh::new();
        gh.script_merge_state(Script::sequence([
            Ok("UNKNOWN".to_string()),
            Ok("CLEAN".to_string()),
        ]));
        assert_eq!(gh.merge_state(&pr("7")).unwrap(), "UNKNOWN");
        assert_eq!(gh.merge_state(&pr("7")).unwrap(), "CLEAN");
        assert_eq!(gh.calls().len(), 2);
    }

    #[test]
    #[should_panic(expected = "unscripted call: no answer left for GhPort::head")]
    fn an_unscripted_call_panics_and_names_the_method() {
        let gh = FakeGh::new();
        drop(gh.head(&pr("7")));
    }

    #[test]
    fn the_merge_call_records_the_head_it_is_pinned_to() {
        let gh = FakeGh::new();
        gh.script_merge(Script::always(Ok(())));
        gh.merge(&pr("7"), &sha(), SpendToken::new()).unwrap();
        assert_eq!(gh.calls(), vec![GhCall::Merge { pr: pr("7"), head: sha() }]);
    }

    #[test]
    fn every_method_records_its_arguments() {
        let gh = FakeGh::new();
        let issue = IssueNumber::new(355).unwrap();
        gh.script_list_open(Script::always(Ok(vec![pr("1")])));
        gh.script_changed_files(Script::always(Ok(vec!["a".into()])));
        gh.script_base_ref(Script::always(Ok("main".into())));
        gh.script_review_decision(Script::always(Ok("APPROVED".into())));
        gh.script_author(Script::always(Ok("alice".into())));
        gh.script_checks(Script::always(Ok("[]".into())));
        gh.script_issue_has_label(Script::always(Ok(true)));
        gh.script_issue_assignees(Script::always(Ok(vec![])));
        gh.script_issue_commenters(Script::always(Ok(vec![])));
        gh.script_update_branch(Script::always(Ok(())));
        gh.script_diff(Script::always(Ok("diff".into())));
        gh.list_open().unwrap();
        gh.changed_files(&pr("2")).unwrap();
        gh.base_ref(&pr("3")).unwrap();
        gh.review_decision(&pr("4")).unwrap();
        gh.author(&pr("5")).unwrap();
        gh.checks(&pr("6")).unwrap();
        gh.issue_has_label(issue, "claimed").unwrap();
        gh.issue_assignees(issue).unwrap();
        gh.issue_commenters(issue).unwrap();
        gh.update_branch(&pr("8"), &sha()).unwrap();
        gh.diff(&pr("9")).unwrap();
        assert_eq!(
            gh.calls(),
            vec![
                GhCall::ListOpen,
                GhCall::ChangedFiles(pr("2")),
                GhCall::BaseRef(pr("3")),
                GhCall::ReviewDecision(pr("4")),
                GhCall::Author(pr("5")),
                GhCall::Checks(pr("6")),
                GhCall::IssueHasLabel { issue, label: "claimed".into() },
                GhCall::IssueAssignees(issue),
                GhCall::IssueCommenters(issue),
                GhCall::UpdateBranch { pr: pr("8"), expected_head: sha() },
                GhCall::Diff(pr("9")),
            ]
        );
    }

    #[test]
    fn the_update_reads_are_scriptable() {
        let gh = FakeGh::new();
        gh.script_update_facts(Script::always(Ok(UpdateFacts {
            head: sha().to_string(),
            state: MergeState::Behind,
            decision: "APPROVED".into(),
            author: "alice".into(),
            cross_repository: true,
            maintainer_can_modify: false,
        })));
        gh.script_reviews(Script::always(Ok(vec![Review {
            user: "vladimirrott".into(),
            state: "APPROVED".into(),
            commit_id: Some(sha().to_string()),
        }])));
        assert_eq!(gh.update_facts(&pr("7")).unwrap().state, MergeState::Behind);
        assert_eq!(gh.reviews(&pr("7")).unwrap()[0].state, "APPROVED");
    }

    #[test]
    fn from_facts_serves_each_read_from_the_value() {
        let gh = FakeGh::from_facts(&facts());
        let n = pr("7");
        assert_eq!(gh.head(&n).unwrap(), sha().as_str());
        assert_eq!(gh.base_ref(&n).unwrap(), "main");
        assert_eq!(gh.merge_state(&n).unwrap(), "CLEAN");
        assert_eq!(gh.review_decision(&n).unwrap(), "APPROVED");
        assert_eq!(gh.author(&n).unwrap(), "alice");
        assert_eq!(gh.changed_files(&n).unwrap(), ["src/lib.rs"]);
        let board = CheckBoard::from_gh_json(&n, &gh.checks(&n).unwrap()).unwrap();
        assert_eq!(
            board,
            CheckBoard { pass: 1, skip: 1, fail: 0, pending: 0, cancel: 0, total: 2 }
        );
        assert!(board.green(&n).is_ok());
    }

    #[test]
    fn from_facts_keeps_an_unreadable_fact_apart_from_an_absent_one() {
        let mut f = facts();
        f.author = Fact::Unreadable("HTTP 502".into());
        f.review_decision = Fact::Absent;
        let gh = FakeGh::from_facts(&f);
        assert_eq!(gh.author(&pr("7")), Err(GhFailure { exit: 1, output: "HTTP 502".into() }));
        assert_eq!(gh.review_decision(&pr("7")), Ok(String::new()));
    }

    #[test]
    fn from_facts_serves_an_unreadable_check_list_as_the_exit_status_eight() {
        let mut f = facts();
        f.checks = Fact::Unreadable("timeout".into());
        let gh = FakeGh::from_facts(&f);
        assert_eq!(gh.checks(&pr("7")).unwrap_err().exit, 8);
    }

    #[test]
    fn from_facts_leaves_the_write_calls_unscripted() {
        let gh = FakeGh::from_facts(&facts());
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drop(gh.merge(&pr("7"), &sha(), SpendToken::new()));
        }));
        assert!(caught.is_err(), "a merge nobody scripted must not succeed in silence");
    }
}
