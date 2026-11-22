//! `maintainer-merge verify-deps <pr> <sha>`: the order of the checks (bash `cmd_verify_deps`,
//! lines 454 to 723) and the refusal each one ends in.
//!
//! The flow reads through ports only: `gh` for the head, the diff and the base branch
//! ([`DepsGh`]), the classifier ([`Classifier`]), the network ([`Net`]), the whole-file pip
//! check ([`WholeFile`]), a file system for the "is the helper script there" questions, and
//! [`Say`] for the progress lines. It returns what it proved ([`DepsVerified`]) and leaves the
//! receipt to [`crate::deps::record`], so a test of a refusal writes nothing.

use crate::deps::pydump::{Triple, dumps_sorted};
use crate::deps::{
    ActionRepo, Bump, Checked, Classifier, ClassifierExit, DepsRefusal, HelperScript, Net,
    PlainPath, Verdict, WholeFile,
};
use crate::{
    Action, CommitId, GateDecision, GhFailure, GhPort, HeadArg, Helper, LeaseFree, PrNumber,
    Progress, Say,
};
use mx_core::Fact;
use mx_ports::Fs;
use mx_receipt::{DependencyFacts, Receipt, Stamp, Unsigned};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The three questions `verify-deps` puts to `gh` itself. Any [`GhPort`] answers them; the
/// narrow trait lets this module run, and be tested, before the gate's real `gh` adapter
/// exists (decision `P3C-1`).
pub trait DepsGh {
    /// `gh pr view N --repo R --json headRefOid --jq .headRefOid`, as printed.
    fn head(&self, pr: &PrNumber) -> Result<String, GhFailure>;
    /// `gh pr view N --repo R --json baseRefName --jq .baseRefName`, as printed.
    fn base_ref(&self, pr: &PrNumber) -> Result<String, GhFailure>;
    /// `gh pr diff N --repo R` with `2>&1`: the net diff, never `--patch`.
    fn diff(&self, pr: &PrNumber) -> Result<String, GhFailure>;
}

impl<T: GhPort + ?Sized> DepsGh for T {
    fn head(&self, pr: &PrNumber) -> Result<String, GhFailure> {
        GhPort::head(self, pr)
    }

    fn base_ref(&self, pr: &PrNumber) -> Result<String, GhFailure> {
        GhPort::base_ref(self, pr)
    }

    fn diff(&self, pr: &PrNumber) -> Result<String, GhFailure> {
        GhPort::diff(self, pr)
    }
}

/// Where the four helper scripts are. Each is `python3 <path>` for the adapters and a
/// `[ -f ]` question for the flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPaths {
    pub classifier: PathBuf,
    pub image_resolver: PathBuf,
    pub pypi_verifier: PathBuf,
    pub pip_files: PathBuf,
}

impl ScriptPaths {
    /// The bash defaults: `${MAINTAINER_SCRIPTS:-$HOME/.local/share/maintainer/scripts}` and the
    /// three `MAINTAINER_*` overrides. An empty variable counts as unset, as `${v:-x}` does.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Self {
        let set = |name: &str| var(name).filter(|value| !value.is_empty());
        let dir = set("MAINTAINER_SCRIPTS").map_or_else(
            || {
                PathBuf::from(set("HOME").unwrap_or_default())
                    .join(".local/share/maintainer/scripts")
            },
            PathBuf::from,
        );
        let pick = |name: &str, file: &str| set(name).map_or_else(|| dir.join(file), PathBuf::from);
        Self {
            classifier: dir.join("classify_dependency_bump.py"),
            image_resolver: pick("MAINTAINER_IMAGE_RESOLVER", "resolve_image_digest.py"),
            pypi_verifier: pick("MAINTAINER_PYPI_VERIFIER", "verify_pypi_release.py"),
            pip_files: pick("MAINTAINER_PIP_FILES_VERIFIER", "verify_pip_files.py"),
        }
    }
}

/// The classifier's time limit as the bash holds it: the text of `timeout`'s argument, which the
/// refusal quotes, and the time it means.
///
/// `timeout` takes a number with an optional `s`, `m`, `h` or `d`. A text it cannot read, a limit
/// that is not positive and finite, and `0` (which `timeout` reads as "no limit"), and one past a day, are no limit
/// at all here: the classifier is not
/// started and the pull request is refused, because an unbounded read of text its author
/// wrote is the stall the limit exists to prevent (decision `P3C-4`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifyLimit {
    text: String,
    duration: Option<Duration>,
}

impl ClassifyLimit {
    /// Reads the text of `MAINTAINER_CLASSIFY_TIMEOUT` or `CLASSIFY_TIMEOUT`.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        Self { text: text.to_string(), duration: duration_of(text) }
    }

    /// The text, as `die` prints it before its `s`.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The limit as a time, or `None` for a text `timeout` would not bound by.
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }
}

fn duration_of(text: &str) -> Option<Duration> {
    let (number, scale) = match text.char_indices().last() {
        Some((at, 's')) => (&text[..at], 1.0),
        Some((at, 'm')) => (&text[..at], 60.0),
        Some((at, 'h')) => (&text[..at], 3600.0),
        Some((at, 'd')) => (&text[..at], 86_400.0),
        _ => (text, 1.0),
    };
    // `timeout` reads the number with strtod: a sign, digits, a point, an exponent. Rust's parser
    // takes the same text, and also `inf` and `nan`, which are no limit and are refused here.
    let seconds = number.parse::<f64>().ok().filter(|n| n.is_finite())? * scale;
    if seconds <= 0.0 {
        return None;
    }
    // A limit past a day is longer than any run waits, and a time that large cannot be added to
    // a clock (the process port would panic on it): it is refused like an unreadable one.
    Duration::try_from_secs_f64(seconds)
        .ok()
        .filter(|limit| !limit.is_zero() && *limit <= MAX_LIMIT)
}

/// The longest classifier limit the port accepts.
const MAX_LIMIT: Duration = Duration::from_hours(24);

/// What the checks are set up with.
#[derive(Debug, Clone)]
pub struct DepsConfig {
    /// `MAINTAINER_SLUG`: `owner/repo`.
    pub repo_slug: String,
    pub scripts: ScriptPaths,
    pub limit: ClassifyLimit,
}

/// Every port the checks read through.
#[derive(Clone, Copy)]
pub struct DepsPorts<'a> {
    pub gh: &'a dyn DepsGh,
    pub net: &'a dyn Net,
    pub classifier: &'a dyn Classifier,
    pub whole_file: &'a dyn WholeFile,
    pub fs: &'a dyn Fs,
    pub say: &'a dyn Say,
}

/// A failed `verify-deps`: a refusal of the checks, or the lease refusal that comes first.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DepsFailure {
    /// A refusal the gate shares with its other subcommands: the lease, the receipt key.
    #[error("{0}")]
    Gate(Box<GateDecision>),
    #[error("{0}")]
    Refused(Box<DepsRefusal>),
}

impl DepsFailure {
    /// Every `die` of the bash exits 1.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Gate(decision) => decision.exit_code(),
            Self::Refused(refusal) => refusal.exit_code(),
        }
    }

    /// What `die` writes to standard error.
    #[must_use]
    pub fn stderr_line(&self) -> String {
        format!("{}: {self}\n", crate::PROGRAM)
    }
}

impl From<DepsRefusal> for DepsFailure {
    fn from(refusal: DepsRefusal) -> Self {
        Self::Refused(Box::new(refusal))
    }
}

/// `maintainer lease check`, asked as the bash does before it reads anything else (`lease_gate`).
pub fn lease_gate(helper: &dyn Helper, pr: &PrNumber) -> Result<LeaseFree, DepsFailure> {
    LeaseFree::from_answer(pr, Action::Verify, helper.lease_check(pr))
        .map_err(|decision| DepsFailure::Gate(Box::new(decision)))
}

/// What the checks proved: every changed path is dependency metadata and every pin, image and
/// hash is what its registry says. Only [`verify_deps`] builds one.
#[derive(Debug)]
pub struct DepsVerified {
    pr: PrNumber,
    head: CommitId,
    facts: DependencyFacts,
}

impl DepsVerified {
    /// The pull request the checks ran for.
    #[must_use]
    pub fn pr(&self) -> &PrNumber {
        &self.pr
    }

    /// The head the checks ran at, as GitHub reported it (the receipt records this, not the argument).
    #[must_use]
    pub fn head(&self) -> &CommitId {
        &self.head
    }

    /// The counts and files the receipt records.
    #[must_use]
    pub fn facts(&self) -> &DependencyFacts {
        &self.facts
    }

    /// The unsigned receipt of kind `dependency`.
    #[must_use]
    pub fn receipt(&self, stamp: &Stamp) -> Receipt<Unsigned> {
        Receipt::dependency(self.pr.value(), self.head.as_str(), &self.facts, stamp)
    }

    /// The last line `verify-deps` says, after the receipt is written.
    #[must_use]
    pub fn recorded_line(&self) -> Progress {
        Progress::DependencyReceiptRecorded {
            pr: self.pr.clone(),
            real_head_8: self.head.short().to_string(),
            lines: self.facts.changed_lines.to_string(),
            checked: self.facts.pins_verified.to_string(),
            images_checked: self.facts.images_verified.to_string(),
            pip_checked: self.facts.pip_verified.to_string(),
        }
    }
}

/// `${text:from:len}`: characters, not bytes.
fn slice(text: &str, from: usize, len: usize) -> String {
    text.chars().skip(from).take(len).collect()
}

/// What `$(...)` leaves of a command's output: no trailing newlines.
fn captured(text: &str) -> &str {
    text.trim_end_matches('\n')
}

fn exists(fs: &dyn Fs, path: &Path) -> bool {
    matches!(fs.read(path), Fact::Known(_))
}

fn missing(fs: &dyn Fs, script: HelperScript, path: &Path) -> Result<(), DepsRefusal> {
    if exists(fs, path) { Ok(()) } else { Err(script.missing(&path.display().to_string())) }
}

/// Runs the checks of `verify-deps` for the pull request the lease was taken on, in the bash order.
///
/// The head is re-read here and the receipt records that head, never the argument: a push
/// between the caller reading the head and this running would otherwise be classified under
/// the old SHA and receipted under the new one.
pub fn verify_deps(
    config: &DepsConfig,
    ports: &DepsPorts<'_>,
    sha: &HeadArg,
    lease: &LeaseFree,
) -> Result<DepsVerified, DepsRefusal> {
    let pr = lease.pr();
    missing(ports.fs, HelperScript::Classifier, &config.scripts.classifier)?;
    let head = real_head(ports, pr, sha)?;
    let bump = classify(config, ports, pr, &head)?;
    let pins = check_pins(ports, pr, &head, &bump)?;
    let images = check_images(config, ports, pr, &head, &bump)?;
    let pip = check_pip(config, ports, pr, &head, &bump)?;
    check_pip_files(config, ports, pr, &head, &bump)?;
    let checked = Checked::new(&bump, pins, images, pip)?;
    still_at(ports, pr, &head)?;
    Ok(DepsVerified { pr: pr.clone(), head, facts: checked.facts(&bump) })
}

fn real_head(ports: &DepsPorts<'_>, pr: &PrNumber, sha: &HeadArg) -> Result<CommitId, DepsRefusal> {
    let unreadable = || DepsRefusal::HeadUnreadable { pr: pr.clone() };
    let raw = ports.gh.head(pr).map_err(|_| unreadable())?;
    let real = captured(&raw);
    if real.is_empty() {
        return Err(unreadable());
    }
    if !real.starts_with(sha.as_str()) {
        return Err(DepsRefusal::HeadMoved {
            pr: pr.clone(),
            real_head_8: slice(real, 0, 8),
            sha_8: slice(sha.as_str(), 0, 8),
        });
    }
    // The head goes into API paths below. GitHub's answer is trusted for what it names, not for
    // its spelling: anything but lower-case hexadecimal is a head that was not read (`P3C-2`).
    CommitId::parse(real).map_err(|_| unreadable())
}

/// Reads the head again and refuses when it is not `head`. The bash reads the head once and
/// classifies and checks whatever the pull request holds afterwards, so a push in between got
/// a receipt for one commit about the content of another. Every stage ends here (decision
/// `P3C-12`); a head that cannot be read is a head that was not confirmed.
fn still_at(ports: &DepsPorts<'_>, pr: &PrNumber, head: &CommitId) -> Result<(), DepsRefusal> {
    let unreadable = || DepsRefusal::HeadUnreadable { pr: pr.clone() };
    let raw = ports.gh.head(pr).map_err(|_| unreadable())?;
    let now = captured(&raw);
    if now.is_empty() {
        return Err(unreadable());
    }
    if now != head.as_str() {
        return Err(DepsRefusal::HeadMoved {
            pr: pr.clone(),
            real_head_8: slice(now, 0, 8),
            sha_8: head.short().to_string(),
        });
    }
    Ok(())
}

fn classify(
    config: &DepsConfig,
    ports: &DepsPorts<'_>,
    pr: &PrNumber,
    head: &CommitId,
) -> Result<Bump, DepsRefusal> {
    let raw = ports
        .gh
        .diff(pr)
        .map_err(|failure| DepsRefusal::DiffUnreadable { pr: pr.clone(), diff: failure.output })?;
    still_at(ports, pr, head)?;
    let diff = captured(&raw);
    if diff.is_empty() {
        return Err(DepsRefusal::DiffEmpty { pr: pr.clone() });
    }
    let not_a_bump = |verdict: &str| DepsRefusal::NotAPureBump {
        pr: pr.clone(),
        verdict: captured(verdict).to_string(),
    };
    let Some(limit) = config.limit.duration() else {
        return Err(not_a_bump(""));
    };
    // `printf '%s\n' "$diff"`: the diff, one newline after it.
    let Ok(run) = ports.classifier.classify(&format!("{diff}\n"), limit) else {
        return Err(not_a_bump(""));
    };
    match run.exit {
        ClassifierExit::TimedOut | ClassifierExit::Code(124) => {
            Err(DepsRefusal::ClassifierTimedOut {
                limit: config.limit.text().to_string(),
                pr: pr.clone(),
            })
        }
        ClassifierExit::Code(0) => match Verdict::parse(&run.stdout) {
            Ok(Verdict::Bump(bump)) => Ok(bump),
            // Exit 0 with a refusal in the JSON cannot happen in the helper; if it did, a refusal
            // is what it says (`P3C-3`).
            Ok(Verdict::Refused(_)) => Err(not_a_bump(&run.stdout)),
            Err(_) => Err(unreadable_verdict(&run.stdout)),
        },
        ClassifierExit::Code(_) => Err(not_a_bump(&run.stdout)),
    }
}

/// Which section of a verdict the bash could not read, in the order it reads them: the pins, the
/// base images, the pip packages, then the files. A verdict that is not JSON, or not an object,
/// fails the first read. A verdict whose sections all read but that the typed parse still refuses
/// (an unknown field, a changed-line count that is not a number) is reported as unreadable pins:
/// the whole verdict is validated before the first check runs (decision `P3C-11`).
fn unreadable_verdict(stdout: &str) -> DepsRefusal {
    use serde_json::Value;
    let Ok(Value::Object(top)) = serde_json::from_str::<Value>(stdout) else {
        return DepsRefusal::PinsUnreadable;
    };
    // A section is a list of objects with these text fields; an absent section is an empty one.
    let readable = |section: &str, fields: &[&str]| match top.get(section) {
        None => true,
        Some(Value::Array(items)) => items
            .iter()
            .all(|item| fields.iter().all(|field| item.get(*field).is_some_and(Value::is_string))),
        Some(_) => false,
    };
    if !readable("pins", &["action", "sha", "tag"]) {
        DepsRefusal::PinsUnreadable
    } else if !readable("images", &["image", "tag", "digest"]) {
        DepsRefusal::ImagesUnreadable
    } else if !readable("pip", &["name", "version"]) {
        DepsRefusal::PipUnreadable
    } else if !top.get("files").is_some_and(Value::is_object) {
        DepsRefusal::PipFilesUnreadable
    } else {
        DepsRefusal::PinsUnreadable
    }
}

fn check_pins(
    ports: &DepsPorts<'_>,
    pr: &PrNumber,
    head: &CommitId,
    bump: &Bump,
) -> Result<u64, DepsRefusal> {
    let mut checked = 0;
    for pin in &bump.pins {
        let repo = ActionRepo::of_action(&pin.action)?;
        let pinsha_12 = slice(&pin.sha, 0, 12);
        if pin.branch {
            check_branch_pin(ports, &repo, &pin.action, &pin.tag, &pin.sha, &pinsha_12)?;
        } else {
            check_tag_pin(ports, &repo, &pin.action, &pin.tag, &pin.sha, &pinsha_12)?;
        }
        still_at(ports, pr, head)?;
        checked += 1;
    }
    Ok(checked)
}

/// `# stable (branch)`: the name must BE a branch, asked of the ref endpoint and read back
/// exactly, and then the SHA must be reachable from it.
fn check_branch_pin(
    ports: &DepsPorts<'_>,
    repo: &ActionRepo,
    action: &str,
    tag: &str,
    pinsha: &str,
    pinsha_12: &str,
) -> Result<(), DepsRefusal> {
    let headref =
        ports.net.branch_ref(repo, tag).map_err(|failure| DepsRefusal::BranchUnaskable {
            repo: repo.to_string(),
            tag: tag.to_string(),
            headref: captured(&failure.output).to_string(),
        })?;
    let headref = captured(&headref);
    if headref != format!("refs/heads/{tag}") {
        return Err(DepsRefusal::PinCommentNotABranch {
            action: action.to_string(),
            tag: tag.to_string(),
            headref: headref.to_string(),
        });
    }
    let status = ports.net.compare_status(repo, tag, pinsha).map_err(|failure| {
        DepsRefusal::CompareUnaskable {
            repo: repo.to_string(),
            pinsha: pinsha.to_string(),
            tag: tag.to_string(),
            status: captured(&failure.output).to_string(),
        }
    })?;
    let status = captured(&status);
    if !matches!(status, "identical" | "behind") {
        return Err(DepsRefusal::PinNotOnBranch {
            action: action.to_string(),
            pinsha_12: pinsha_12.to_string(),
            tag: tag.to_string(),
            repo: repo.to_string(),
            status: status.to_string(),
        });
    }
    ports.say.say(&Progress::PinOnBranch {
        action: action.to_string(),
        pinsha_12: pinsha_12.to_string(),
        tag: tag.to_string(),
    });
    Ok(())
}

/// The depth bound on an annotated tag's chain of tag objects.
const TAG_DEPTH: u32 = 4;

fn check_tag_pin(
    ports: &DepsPorts<'_>,
    repo: &ActionRepo,
    action: &str,
    tag: &str,
    pinsha: &str,
    pinsha_12: &str,
) -> Result<(), DepsRefusal> {
    let found = ports.net.tag_ref(repo, tag).map_err(|failure| DepsRefusal::TagUnaskable {
        tag: tag.to_string(),
        repo: repo.to_string(),
        refline: captured(&failure.output).to_string(),
    })?;
    if found.name != format!("refs/tags/{tag}") {
        return Err(DepsRefusal::PinCommentNotATag {
            action: action.to_string(),
            tag: tag.to_string(),
            refname: if found.name.is_empty() { "nothing".to_string() } else { found.name },
        });
    }
    let (mut object_type, mut object_sha) = (found.object_type, found.object_sha);
    let mut depth = 0;
    while object_type == "tag" {
        depth += 1;
        if depth > TAG_DEPTH {
            return Err(DepsRefusal::TagNestedTooDeep {
                tag: tag.to_string(),
                repo: repo.to_string(),
            });
        }
        let next = ports.net.tag_object(repo, &object_sha).map_err(|failure| {
            DepsRefusal::TagObjectUnreadable {
                objsha_12: slice(&object_sha, 0, 12),
                repo: repo.to_string(),
                tag: tag.to_string(),
                refline: captured(&failure.output).to_string(),
            }
        })?;
        (object_type, object_sha) = (next.object_type, next.sha);
    }
    if object_type != "commit" {
        return Err(DepsRefusal::TagNotACommit {
            tag: tag.to_string(),
            repo: repo.to_string(),
            objtype: if object_type.is_empty() { "nothing".to_string() } else { object_type },
        });
    }
    if object_sha != pinsha {
        return Err(DepsRefusal::PinNotOnTag {
            action: action.to_string(),
            pinsha_12: pinsha_12.to_string(),
            repo: repo.to_string(),
            tag: tag.to_string(),
            upstream_12: slice(&object_sha, 0, 12),
        });
    }
    ports.say.say(&Progress::PinOnTag {
        action: action.to_string(),
        tag: tag.to_string(),
        pinsha_12: pinsha_12.to_string(),
    });
    Ok(())
}

/// Every base image against the registry it comes from.
fn check_images(
    config: &DepsConfig,
    ports: &DepsPorts<'_>,
    pr: &PrNumber,
    head: &CommitId,
    bump: &Bump,
) -> Result<u64, DepsRefusal> {
    let mut checked = 0;
    for image in &bump.images {
        missing(ports.fs, HelperScript::ImageResolver, &config.scripts.image_resolver)?;
        let live = ports.net.image_digest(&image.image, &image.tag).map_err(|failure| {
            DepsRefusal::ImageUnaskable {
                image: image.image.clone(),
                tag: image.tag.clone(),
                live: captured(&failure.output).to_string(),
            }
        })?;
        let live = captured(&live);
        if live != image.digest {
            return Err(DepsRefusal::ImageDigestMismatch {
                image: image.image.clone(),
                tag: image.tag.clone(),
                live_19: slice(live, 0, 19),
                digest_19: slice(&image.digest, 0, 19),
            });
        }
        ports.say.say(&Progress::ImageAgrees {
            image: image.image.clone(),
            tag: image.tag.clone(),
            digest: slice(&image.digest, 7, 19),
        });
        still_at(ports, pr, head)?;
        checked += 1;
    }
    Ok(checked)
}

/// Every pip requirement against the release on `PyPI`.
fn check_pip(
    config: &DepsConfig,
    ports: &DepsPorts<'_>,
    pr: &PrNumber,
    head: &CommitId,
    bump: &Bump,
) -> Result<u64, DepsRefusal> {
    let mut checked = 0;
    for package in &bump.pip {
        missing(ports.fs, HelperScript::PypiVerifier, &config.scripts.pypi_verifier)?;
        let answer = ports
            .net
            .pypi_release(&package.name, &package.version, &package.hashes)
            .map_err(|failure| DepsRefusal::PypiUnconfirmed {
                pkg: package.name.clone(),
                version: package.version.clone(),
                answer: captured(&failure.output).to_string(),
            })?;
        ports.say.say(&Progress::PipAgrees {
            pkg: package.name.clone(),
            version: package.version.clone(),
            answer: captured(&answer).to_string(),
        });
        still_at(ports, pr, head)?;
        checked += 1;
    }
    Ok(checked)
}

/// What the whole-file helper's JSON says moved, as the triples the diff's reading is compared with.
fn whole_file_triples(answer: &str) -> Option<Vec<Triple>> {
    let value: serde_json::Value = serde_json::from_str(answer).ok()?;
    let changed = value.get("changed")?.as_array()?;
    changed
        .iter()
        .map(|entry| {
            let text = |key: &str| entry.get(key)?.as_str().map(str::to_string);
            let hashes = entry
                .get("hashes")?
                .as_array()?
                .iter()
                .map(|hash| hash.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()?;
            Some((text("name")?, text("version")?, hashes))
        })
        .collect()
}

/// Every requirements file read whole, the way pip reads it, and compared with the diff's reading.
fn check_pip_files(
    config: &DepsConfig,
    ports: &DepsPorts<'_>,
    pr: &PrNumber,
    head: &CommitId,
    bump: &Bump,
) -> Result<(), DepsRefusal> {
    let files = bump.pip_files();
    if files.is_empty() {
        return Ok(());
    }
    missing(ports.fs, HelperScript::WholeFile, &config.scripts.pip_files)?;
    let base_ref = match ports.gh.base_ref(pr) {
        Ok(raw) if !captured(&raw).is_empty() => captured(&raw).to_string(),
        _ => return Err(DepsRefusal::BaseBranchUnreadable { pr: pr.clone() }),
    };
    let merge_base = ports
        .net
        .merge_base(&config.repo_slug, &base_ref, head)
        .map_err(|_| DepsRefusal::MergeBaseUnaskable { pr: pr.clone() })?;
    let merge_base = CommitId::parse_full(captured(&merge_base)).map_err(|_| {
        DepsRefusal::NoMergeBase { pr: pr.clone(), got: slice(captured(&merge_base), 0, 40) }
    })?;
    for file in files {
        let path = PlainPath::parse(file)?;
        let at_base = ports.net.file_at(&config.repo_slug, &path, &merge_base).map_err(|_| {
            DepsRefusal::FileAtMergeBaseUnreadable {
                pf: file.to_string(),
                merge_base_8: merge_base.short().to_string(),
            }
        })?;
        let at_head = ports.net.file_at(&config.repo_slug, &path, head).map_err(|_| {
            DepsRefusal::FileAtHeadUnreadable {
                pf: file.to_string(),
                real_head_8: head.short().to_string(),
            }
        })?;
        let answer = ports.whole_file.check(&path, &at_base, &at_head).map_err(|failure| {
            DepsRefusal::NotAVersionBump {
                pf: file.to_string(),
                got: captured(&failure.output).to_string(),
            }
        })?;
        let want: Vec<Triple> = bump
            .pip
            .iter()
            .filter(|package| package.path == file)
            .map(|package| (package.name.clone(), package.version.clone(), package.hashes.clone()))
            .collect();
        let got = whole_file_triples(captured(&answer))
            .ok_or_else(|| DepsRefusal::WholeFileAnswerUnreadable { pf: file.to_string() })?;
        let (want, got) = (dumps_sorted(&want), dumps_sorted(&got));
        if want != got {
            return Err(DepsRefusal::DiffAndFileDisagree {
                pf: file.to_string(),
                want_300: slice(&want, 0, 300),
                got_300: slice(&got, 0, 300),
            });
        }
        ports.say.say(&Progress::PipFileAgrees {
            pf: file.to_string(),
            merge_base_8: merge_base.short().to_string(),
            real_head_8: head.short().to_string(),
        });
        still_at(ports, pr, head)?;
    }
    Ok(())
}
