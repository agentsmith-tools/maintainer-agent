//! Every call `verify-deps` makes to GitHub, a registry or `PyPI`.

use crate::CommitId;
use crate::deps::{ActionRepo, PlainPath};
use mx_ports::Script;
use mx_suite::Recorder;
use std::fmt;
use std::sync::{Mutex, PoisonError};

/// A network call that did not answer. `output` is what the call printed, standard
/// output and standard error together (`2>&1`), because the refusal quotes it
/// indented under a sentence that says what could not be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetFailure {
    pub output: String,
}

impl fmt::Display for NetFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.output)
    }
}

/// What `gh api repos/<repo>/git/ref/tags/<tag>` says about a tag:
/// `[.ref, .object.type, .object.sha] | @tsv`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRef {
    /// The ref GitHub found. Only `refs/tags/<tag>` exactly is the tag the comment
    /// claims: `commits/<tag>` once resolved `pull/1/head`, which anyone can push.
    pub name: String,
    /// `commit` for a lightweight tag, `tag` for an annotated one.
    pub object_type: String,
    pub object_sha: String,
}

/// What `gh api repos/<repo>/git/tags/<sha>` says about a tag object:
/// `[.object.type, .object.sha] | @tsv`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitObject {
    pub object_type: String,
    pub sha: String,
}

/// The network checks of `verify-deps`, one method per call.
///
/// The real adapter starts `gh` and the two registry scripts through
/// [`mx_ports::Proc`] with the argv the bash uses. A test never reaches the
/// network, because a fake answers. Each method says its bash call.
pub trait Net {
    /// `gh api repos/<repo>/git/ref/heads/<branch> --jq .ref`. A pin whose comment
    /// says `(branch)` claims the SHA is on a branch, so first the name must BE a
    /// branch: `compare` resolves any ref the name reaches after GitHub normalises
    /// the path, a pull request's head included.
    fn branch_ref(&self, repo: &ActionRepo, branch: &str) -> Result<String, NetFailure>;

    /// `gh api repos/<repo>/compare/<base>...<head> --jq .status`: `identical` or
    /// `behind` means the branch contains the pin.
    fn compare_status(
        &self,
        repo: &ActionRepo,
        base: &str,
        head: &str,
    ) -> Result<String, NetFailure>;

    /// The tag, asked for as a tag.
    fn tag_ref(&self, repo: &ActionRepo, tag: &str) -> Result<TagRef, NetFailure>;

    /// An annotated tag names a tag object, which names the commit. Real ones are
    /// one level deep; the caller bounds the chain at four.
    fn tag_object(&self, repo: &ActionRepo, sha: &str) -> Result<GitObject, NetFailure>;

    /// `python3 scripts/resolve_image_digest.py <image> <tag>`: the digest the tag
    /// resolves to at the registry right now, as printed.
    fn image_digest(&self, image: &str, tag: &str) -> Result<String, NetFailure>;

    /// `python3 scripts/verify_pypi_release.py <name> <version> <hash,hash,...>`:
    /// what it printed (how many of the release's files the hashes matched).
    fn pypi_release(
        &self,
        name: &str,
        version: &str,
        hashes: &[String],
    ) -> Result<String, NetFailure>;

    /// `gh api repos/<slug>/compare/<base_ref>...<head> --jq .merge_base_commit.sha`,
    /// as printed. The caller checks it with [`CommitId::parse_full`].
    fn merge_base(&self, slug: &str, base_ref: &str, head: &CommitId)
    -> Result<String, NetFailure>;

    /// `gh api -H 'Accept: application/vnd.github.raw' repos/<slug>/contents/<path>?ref=<rev>`:
    /// the file as it was at `rev`.
    fn file_at(&self, slug: &str, path: &PlainPath, rev: &CommitId) -> Result<Vec<u8>, NetFailure>;
}

/// One call to a [`FakeNet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetCall {
    BranchRef { repo: String, branch: String },
    CompareStatus { repo: String, base: String, head: String },
    TagRef { repo: String, tag: String },
    TagObject { repo: String, sha: String },
    ImageDigest { image: String, tag: String },
    PypiRelease { name: String, version: String, hashes: Vec<String> },
    MergeBase { slug: String, base_ref: String, head: CommitId },
    FileAt { slug: String, path: String, rev: CommitId },
}

type Answer<T> = Result<T, NetFailure>;

/// A scripted [`Net`] that records its calls and reaches nobody. A call nobody
/// scripted panics.
#[derive(Debug, Default)]
pub struct FakeNet {
    calls: Recorder<NetCall>,
    branch_ref: Mutex<Script<Answer<String>>>,
    compare_status: Mutex<Script<Answer<String>>>,
    tag_ref: Mutex<Script<Answer<TagRef>>>,
    tag_object: Mutex<Script<Answer<GitObject>>>,
    image_digest: Mutex<Script<Answer<String>>>,
    pypi_release: Mutex<Script<Answer<String>>>,
    merge_base: Mutex<Script<Answer<String>>>,
    file_at: Mutex<Script<Answer<Vec<u8>>>>,
}

fn next<T: Clone>(slot: &Mutex<Script<T>>, call: &str) -> T {
    slot.lock().unwrap_or_else(PoisonError::into_inner).next(call)
}

fn set<T: Clone>(slot: &Mutex<Script<T>>, script: Script<T>) {
    *slot.lock().unwrap_or_else(PoisonError::into_inner) = script;
}

impl FakeNet {
    /// A fake with no scripted answers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every call so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<NetCall> {
        self.calls.entries()
    }

    /// Scripts `branch_ref`.
    pub fn script_branch_ref(&self, s: Script<Answer<String>>) {
        set(&self.branch_ref, s);
    }

    /// Scripts `compare_status`.
    pub fn script_compare_status(&self, s: Script<Answer<String>>) {
        set(&self.compare_status, s);
    }

    /// Scripts `tag_ref`.
    pub fn script_tag_ref(&self, s: Script<Answer<TagRef>>) {
        set(&self.tag_ref, s);
    }

    /// Scripts `tag_object`.
    pub fn script_tag_object(&self, s: Script<Answer<GitObject>>) {
        set(&self.tag_object, s);
    }

    /// Scripts `image_digest`.
    pub fn script_image_digest(&self, s: Script<Answer<String>>) {
        set(&self.image_digest, s);
    }

    /// Scripts `pypi_release`.
    pub fn script_pypi_release(&self, s: Script<Answer<String>>) {
        set(&self.pypi_release, s);
    }

    /// Scripts `merge_base`.
    pub fn script_merge_base(&self, s: Script<Answer<String>>) {
        set(&self.merge_base, s);
    }

    /// Scripts `file_at`.
    pub fn script_file_at(&self, s: Script<Answer<Vec<u8>>>) {
        set(&self.file_at, s);
    }
}

impl Net for FakeNet {
    fn branch_ref(&self, repo: &ActionRepo, branch: &str) -> Answer<String> {
        self.calls.push(NetCall::BranchRef { repo: repo.slug().into(), branch: branch.into() });
        next(&self.branch_ref, "Net::branch_ref")
    }

    fn compare_status(&self, repo: &ActionRepo, base: &str, head: &str) -> Answer<String> {
        self.calls.push(NetCall::CompareStatus {
            repo: repo.slug().into(),
            base: base.into(),
            head: head.into(),
        });
        next(&self.compare_status, "Net::compare_status")
    }

    fn tag_ref(&self, repo: &ActionRepo, tag: &str) -> Answer<TagRef> {
        self.calls.push(NetCall::TagRef { repo: repo.slug().into(), tag: tag.into() });
        next(&self.tag_ref, "Net::tag_ref")
    }

    fn tag_object(&self, repo: &ActionRepo, sha: &str) -> Answer<GitObject> {
        self.calls.push(NetCall::TagObject { repo: repo.slug().into(), sha: sha.into() });
        next(&self.tag_object, "Net::tag_object")
    }

    fn image_digest(&self, image: &str, tag: &str) -> Answer<String> {
        self.calls.push(NetCall::ImageDigest { image: image.into(), tag: tag.into() });
        next(&self.image_digest, "Net::image_digest")
    }

    fn pypi_release(&self, name: &str, version: &str, hashes: &[String]) -> Answer<String> {
        self.calls.push(NetCall::PypiRelease {
            name: name.into(),
            version: version.into(),
            hashes: hashes.to_vec(),
        });
        next(&self.pypi_release, "Net::pypi_release")
    }

    fn merge_base(&self, slug: &str, base_ref: &str, head: &CommitId) -> Answer<String> {
        self.calls.push(NetCall::MergeBase {
            slug: slug.into(),
            base_ref: base_ref.into(),
            head: head.clone(),
        });
        next(&self.merge_base, "Net::merge_base")
    }

    fn file_at(&self, slug: &str, path: &PlainPath, rev: &CommitId) -> Answer<Vec<u8>> {
        self.calls.push(NetCall::FileAt {
            slug: slug.into(),
            path: path.as_str().into(),
            rev: rev.clone(),
        });
        next(&self.file_at, "Net::file_at")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> ActionRepo {
        ActionRepo::of_action("actions/checkout").unwrap()
    }

    fn sha() -> CommitId {
        CommitId::parse_full("0123456789abcdef0123456789abcdef01234567").unwrap()
    }

    fn failure(text: &str) -> NetFailure {
        NetFailure { output: text.into() }
    }

    #[test]
    fn a_scripted_tag_is_returned_and_the_call_is_recorded() {
        let net = FakeNet::new();
        let tag = TagRef {
            name: "refs/tags/v4.2.2".into(),
            object_type: "commit".into(),
            object_sha: "b".repeat(40),
        };
        net.script_tag_ref(Script::always(Ok(tag.clone())));
        assert_eq!(net.tag_ref(&repo(), "v4.2.2"), Ok(tag));
        assert_eq!(
            net.calls(),
            vec![NetCall::TagRef { repo: "actions/checkout".into(), tag: "v4.2.2".into() }]
        );
    }

    #[test]
    fn an_annotated_tag_chain_is_scriptable_level_by_level() {
        let net = FakeNet::new();
        net.script_tag_object(Script::sequence([
            Ok(GitObject { object_type: "tag".into(), sha: "1".repeat(40) }),
            Ok(GitObject { object_type: "commit".into(), sha: "2".repeat(40) }),
        ]));
        assert_eq!(net.tag_object(&repo(), "0").unwrap().object_type, "tag");
        assert_eq!(net.tag_object(&repo(), "1").unwrap().object_type, "commit");
    }

    #[test]
    fn a_failure_keeps_what_the_call_printed() {
        let net = FakeNet::new();
        net.script_image_digest(Script::always(Err(failure(
            "resolve_image_digest: registry unreachable",
        ))));
        let refused = net.image_digest("docker.io/library/rust", "1").unwrap_err();
        assert_eq!(refused.to_string(), "resolve_image_digest: registry unreachable");
    }

    #[test]
    fn branch_and_compare_answers_are_text() {
        let net = FakeNet::new();
        net.script_branch_ref(Script::always(Ok("refs/heads/stable".into())));
        net.script_compare_status(Script::always(Ok("behind".into())));
        assert_eq!(net.branch_ref(&repo(), "stable").unwrap(), "refs/heads/stable");
        assert_eq!(net.compare_status(&repo(), "stable", &"a".repeat(40)).unwrap(), "behind");
    }

    #[test]
    fn pypi_and_the_merge_base_and_a_file_are_scripted_and_recorded() {
        let net = FakeNet::new();
        net.script_pypi_release(Script::always(Ok("1 of 1 files matched".into())));
        net.script_merge_base(Script::always(Ok("c".repeat(40))));
        net.script_file_at(Script::always(Ok(b"PyYAML==6.0.2\n".to_vec())));
        let hashes = vec!["d".repeat(64)];
        assert_eq!(net.pypi_release("PyYAML", "6.0.3", &hashes).unwrap(), "1 of 1 files matched");
        assert_eq!(net.merge_base("o/r", "main", &sha()).unwrap(), "c".repeat(40));
        let path = PlainPath::parse("requirements.txt").unwrap();
        assert_eq!(net.file_at("o/r", &path, &sha()).unwrap(), b"PyYAML==6.0.2\n");
        assert_eq!(
            net.calls(),
            vec![
                NetCall::PypiRelease { name: "PyYAML".into(), version: "6.0.3".into(), hashes },
                NetCall::MergeBase { slug: "o/r".into(), base_ref: "main".into(), head: sha() },
                NetCall::FileAt { slug: "o/r".into(), path: "requirements.txt".into(), rev: sha() },
            ]
        );
    }

    #[test]
    fn the_image_digest_call_records_the_image_and_the_tag() {
        let net = FakeNet::new();
        net.script_image_digest(Script::always(Ok(format!("sha256:{}", "e".repeat(64)))));
        net.image_digest("docker.io/library/rust", "1-bookworm").unwrap();
        assert_eq!(
            net.calls(),
            vec![NetCall::ImageDigest {
                image: "docker.io/library/rust".into(),
                tag: "1-bookworm".into()
            }]
        );
    }

    #[test]
    #[should_panic(expected = "unscripted call: no answer left for Net::tag_ref")]
    fn an_unscripted_call_panics_and_names_the_method() {
        drop(FakeNet::new().tag_ref(&repo(), "v1"));
    }

    #[test]
    fn a_compare_call_records_its_three_arguments() {
        let net = FakeNet::new();
        net.script_compare_status(Script::always(Ok("identical".into())));
        net.compare_status(&repo(), "main", "abc").unwrap();
        assert_eq!(
            net.calls(),
            vec![NetCall::CompareStatus {
                repo: "actions/checkout".into(),
                base: "main".into(),
                head: "abc".into()
            }]
        );
    }
}
