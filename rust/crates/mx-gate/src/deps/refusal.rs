//! The refusals of `maintainer-merge verify-deps` (bash lines 454 to 723), each with its exact text.
//!
//! The order of the checks and the flow that raises them are the dependency agent's (brief p3c).

#![allow(
    clippy::doc_markdown,
    reason = "each variant quotes the bash die line it replaces, with its $variables"
)]

use crate::PrNumber;
use mx_suite::text::indent4;
use thiserror::Error;

/// Every refusal of `verify-deps`, each with the text of its `die`: the generated ones and the six the types of
/// this module raise themselves (`Checked::new`, `PlainPath::parse`, `ActionRepo::of_action`, the merge base).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DepsRefusal {
    /// The loop over the action pins checked fewer or more than the classifier declared (bash line 601).
    #[error(
        "the classifier declared {declared} action pin(s) and {checked}\n  were verified. A receipt is a claim about all of them, so this one is not\n  written. Nothing here says the bump is bad; it says the check did not run."
    )]
    PinsUnchecked { declared: u64, checked: u64 },
    /// The loop over the base images checked fewer or more than declared (bash line 625).
    #[error(
        "the classifier declared {declared} base image(s) and\n  {checked} were verified. No receipt written: the check did not run."
    )]
    ImagesUnchecked { declared: u64, checked: u64 },
    /// The loop over the pip packages checked fewer or more than declared (bash line 643).
    #[error(
        "the classifier declared {declared} pip package(s) and\n  {checked} were verified. No receipt written: the check did not run."
    )]
    PipUnchecked { declared: u64, checked: u64 },
    /// bash line 673: die "the requirements path '$pf' is not a plain path, so it is not put in an API URL"
    #[error("the requirements path '{path}' is not a plain path, so it is not put in an API URL")]
    NotAPlainPath { path: String },
    /// **new.** An action path with no plain `owner/repo` in front. The bash takes `checkout` for the repository `checkout/checkout`.
    #[error(
        "the action '{action}' is not owner/repo[/dir] with plain segments, so it is not put in an API URL"
    )]
    ActionNotOwnerRepo { action: String },
    /// bash line 668: die "GitHub gave no merge base for #$pr: '${merge_base:0:40}'"
    #[error("GitHub gave no merge base for #{pr}: '{got}'")]
    NoMergeBase { pr: PrNumber, got: String },

    /// bash line 460: `no dependency classifier at $classifier; run ./install.sh`
    #[error("no dependency classifier at {classifier}; run ./install.sh")]
    NoClassifier { classifier: String },
    /// bash lines 467, 468: `could not read the head of #$pr`
    #[error("could not read the head of #{pr}")]
    HeadUnreadable { pr: PrNumber },
    /// bash line 471: `#$pr is at ${real_head:0:8}, not ${sha:0:8}. It moved while you were reading it.`
    #[error("#{pr} is at {real_head_8}, not {sha_8}. It moved while you were reading it.")]
    HeadMoved { pr: PrNumber, real_head_8: String, sha_8: String },
    /// bash line 478: `could not read the diff for #$pr:`
    #[error("could not read the diff for #{pr}:\n\n{}", indent4(diff))]
    DiffUnreadable { pr: PrNumber, diff: String },
    /// bash line 481: `the diff for #$pr is empty, so nothing was classified`
    #[error("the diff for #{pr} is empty, so nothing was classified")]
    DiffEmpty { pr: PrNumber },
    /// bash line 488: `the dependency classifier ran past ${limit}s on #$pr, so nothing was classified`
    #[error("the dependency classifier ran past {limit}s on #{pr}, so nothing was classified")]
    ClassifierTimedOut { limit: String, pr: PrNumber },
    /// bash line 490: `#$pr is not a pure dependency bump, so it needs a real receipt:`
    #[error(
        "#{pr} is not a pure dependency bump, so it needs a real receipt:\n\n{}",
        indent4(verdict)
    )]
    NotAPureBump { pr: PrNumber, verdict: String },
    /// bash line 510: `could not read the action pins out of the classifier verdict`
    #[error("could not read the action pins out of the classifier verdict")]
    PinsUnreadable,
    /// bash line 515: `could not read the base images out of the classifier verdict`
    #[error("could not read the base images out of the classifier verdict")]
    ImagesUnreadable,
    /// bash line 517: `could not count the action pins the classifier declared`
    #[error("could not count the action pins the classifier declared")]
    PinsUncounted,
    /// bash line 519: `could not count the base images the classifier declared`
    #[error("could not count the base images the classifier declared")]
    ImagesUncounted,
    /// bash line 525: `could not read the pip packages out of the classifier verdict`
    #[error("could not read the pip packages out of the classifier verdict")]
    PipUnreadable,
    /// bash line 527: `could not count the pip packages the classifier declared`
    #[error("could not count the pip packages the classifier declared")]
    PipUncounted,
    /// bash line 543: `could not ask GitHub whether $repo has a branch named $tag, so the pin is unverified:`
    #[error(
        "could not ask GitHub whether {repo} has a branch named {tag}, so the pin is unverified:\n\n{}",
        indent4(headref)
    )]
    BranchUnaskable { repo: String, tag: String, headref: String },
    /// bash line 546: `the pin comment is not a branch: $action names '$tag', and GitHub`
    #[error(
        "the pin comment is not a branch: {action} names '{tag}', and GitHub\n  answers {headref} for refs/heads/{tag}. A name that resolves to another ref\n  is how a pin to a pull request's head passes as a branch."
    )]
    PinCommentNotABranch { action: String, tag: String, headref: String },
    /// bash line 552: `could not ask GitHub whether $repo $pinsha is on $tag, so the pin is unverified:`
    #[error(
        "could not ask GitHub whether {repo} {pinsha} is on {tag}, so the pin is unverified:\n\n{}",
        indent4(status)
    )]
    CompareUnaskable { repo: String, pinsha: String, tag: String, status: String },
    /// bash line 557: `the pin comment is not true: $action is pinned to ${pinsha:0:12},`
    #[error(
        "the pin comment is not true: {action} is pinned to {pinsha_12},\n  which is not reachable from {tag} in {repo} (compare says '{status}'). A\n  branch pin off its branch is a commit nobody on that branch published."
    )]
    PinNotOnBranch { action: String, pinsha_12: String, tag: String, repo: String, status: String },
    /// bash line 573: `could not ask GitHub for the tag $tag in $repo, so the pin is unverified:`
    #[error(
        "could not ask GitHub for the tag {tag} in {repo}, so the pin is unverified:\n\n{}",
        indent4(refline)
    )]
    TagUnaskable { tag: String, repo: String, refline: String },
    /// bash line 577: `the pin comment is not a tag: $action names '$tag', and GitHub`
    #[error(
        "the pin comment is not a tag: {action} names '{tag}', and GitHub\n  answers {refname} for refs/tags/{tag}. A name that resolves to another\n  ref is how a pin to a pull request's head passes as a release."
    )]
    PinCommentNotATag { action: String, tag: String, refname: String },
    /// bash line 584: `the tag $tag in $repo is nested more than 4 tag objects deep`
    #[error(
        "the tag {tag} in {repo} is nested more than 4 tag objects deep\n  without reaching a commit, so the pin is unverified."
    )]
    TagNestedTooDeep { tag: String, repo: String },
    /// bash line 587: `could not read the tag object ${objsha:0:12} behind $repo $tag, so the pin is unverified:`
    #[error(
        "could not read the tag object {objsha_12} behind {repo} {tag}, so the pin is unverified:\n\n{}",
        indent4(refline)
    )]
    TagObjectUnreadable { objsha_12: String, repo: String, tag: String, refline: String },
    /// bash line 592: `the tag $tag in $repo points at a ${objtype:-nothing}, not a commit,`
    #[error(
        "the tag {tag} in {repo} points at a {objtype}, not a commit,\n  so no pin can be on it."
    )]
    TagNotACommit { tag: String, repo: String, objtype: String },
    /// bash line 595: `the pin comment is not true: $action is pinned to`
    #[error(
        "the pin comment is not true: {action} is pinned to\n  {pinsha_12} and {repo} {tag} is {upstream_12}. A pin whose comment names a\n  version it is not on is the shape a supply-chain swap hides in."
    )]
    PinNotOnTag {
        action: String,
        pinsha_12: String,
        repo: String,
        tag: String,
        upstream_12: String,
    },
    /// bash line 614: `no image resolver at $resolver; run ./install.sh`
    #[error("no image resolver at {resolver}; run ./install.sh")]
    NoImageResolver { resolver: String },
    /// bash line 615: `could not ask the registry what $image:$tag is,`
    #[error(
        "could not ask the registry what {image}:{tag} is,\n  so the pin is unverified:\n\n{}",
        indent4(live)
    )]
    ImageUnaskable { image: String, tag: String, live: String },
    /// bash line 619: `the base image pin is not the tag it claims: $image:$tag`
    #[error(
        "the base image pin is not the tag it claims: {image}:{tag}\n  resolves to {live_19} and the Dockerfile pins {digest_19}. A digest\n  that is not the one its tag names is the shape a base-image swap hides in."
    )]
    ImageDigestMismatch { image: String, tag: String, live_19: String, digest_19: String },
    /// bash line 635: `no PyPI verifier at $verifier; run ./install.sh`
    #[error("no PyPI verifier at {verifier}; run ./install.sh")]
    NoPypiVerifier { verifier: String },
    /// bash line 636: `could not confirm $pkg==$version against PyPI,`
    #[error(
        "could not confirm {pkg}=={version} against PyPI,\n  so the pin is unverified:\n\n{}",
        indent4(answer)
    )]
    PypiUnconfirmed { pkg: String, version: String, answer: String },
    /// bash line 658: `could not read the pip files out of the classifier verdict`
    #[error("could not read the pip files out of the classifier verdict")]
    PipFilesUnreadable,
    /// bash line 662: `no whole-file pip check at $wf; run ./install.sh`
    #[error("no whole-file pip check at {wf}; run ./install.sh")]
    NoWholeFileCheck { wf: String },
    /// bash lines 664, 665: `could not read the base branch of #$pr`
    #[error("could not read the base branch of #{pr}")]
    BaseBranchUnreadable { pr: PrNumber },
    /// bash line 667: `could not ask GitHub for the merge base of #$pr`
    #[error("could not ask GitHub for the merge base of #{pr}")]
    MergeBaseUnaskable { pr: PrNumber },
    /// bash line 675: `could not read $pf at the merge base ${merge_base:0:8}`
    #[error("could not read {pf} at the merge base {merge_base_8}")]
    FileAtMergeBaseUnreadable { pf: String, merge_base_8: String },
    /// bash line 677: `could not read $pf at the head ${real_head:0:8}`
    #[error("could not read {pf} at the head {real_head_8}")]
    FileAtHeadUnreadable { pf: String, real_head_8: String },
    /// bash line 679: `$pf, read whole the way pip reads it, is not a version bump:`
    #[error("{pf}, read whole the way pip reads it, is not a version bump:\n\n{}", indent4(got))]
    NotAVersionBump { pf: String, got: String },
    /// bash line 686: `could not read the pip packages for $pf out of the verdict`
    #[error("could not read the pip packages for {pf} out of the verdict")]
    PipForFileUnreadable { pf: String },
    /// bash line 690: `could not read the whole-file answer for $pf`
    #[error("could not read the whole-file answer for {pf}")]
    WholeFileAnswerUnreadable { pf: String },
    /// bash line 691: `the diff and the whole file disagree about $pf:`
    #[error(
        "the diff and the whole file disagree about {pf}:\n  diff:  {want_300}\n  files: {got_300}\n  A bump the two readings describe differently is not one this gate can vouch for."
    )]
    DiffAndFileDisagree { pf: String, want_300: String, got_300: String },
    /// bash line 705: `could not record the dependency receipt`
    #[error("could not record the dependency receipt")]
    CannotRecord,
}

impl DepsRefusal {
    /// The process exit code.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::NoClassifier { .. }
            | Self::HeadUnreadable { .. }
            | Self::HeadMoved { .. }
            | Self::DiffUnreadable { .. }
            | Self::DiffEmpty { .. }
            | Self::ClassifierTimedOut { .. }
            | Self::NotAPureBump { .. }
            | Self::PinsUnreadable
            | Self::ImagesUnreadable
            | Self::PinsUncounted
            | Self::ImagesUncounted
            | Self::PipUnreadable
            | Self::PipUncounted
            | Self::BranchUnaskable { .. }
            | Self::PinCommentNotABranch { .. }
            | Self::CompareUnaskable { .. }
            | Self::PinNotOnBranch { .. }
            | Self::TagUnaskable { .. }
            | Self::PinCommentNotATag { .. }
            | Self::TagNestedTooDeep { .. }
            | Self::TagObjectUnreadable { .. }
            | Self::TagNotACommit { .. }
            | Self::PinNotOnTag { .. }
            | Self::NoImageResolver { .. }
            | Self::ImageUnaskable { .. }
            | Self::ImageDigestMismatch { .. }
            | Self::NoPypiVerifier { .. }
            | Self::PypiUnconfirmed { .. }
            | Self::PipFilesUnreadable
            | Self::NoWholeFileCheck { .. }
            | Self::BaseBranchUnreadable { .. }
            | Self::MergeBaseUnaskable { .. }
            | Self::FileAtMergeBaseUnreadable { .. }
            | Self::FileAtHeadUnreadable { .. }
            | Self::NotAVersionBump { .. }
            | Self::PipForFileUnreadable { .. }
            | Self::WholeFileAnswerUnreadable { .. }
            | Self::DiffAndFileDisagree { .. }
            | Self::CannotRecord
            | Self::PinsUnchecked { .. }
            | Self::ImagesUnchecked { .. }
            | Self::PipUnchecked { .. }
            | Self::NotAPlainPath { .. }
            | Self::ActionNotOwnerRepo { .. }
            | Self::NoMergeBase { .. } => 1,
        }
    }
}

#[cfg(test)]
pub(crate) fn samples() -> Vec<(DepsRefusal, String)> {
    vec![
            (DepsRefusal::NoClassifier { classifier: "⟦classifier⟧".to_string() }, "no dependency classifier at ⟦classifier⟧; run ./install.sh".to_string()),
            (DepsRefusal::HeadUnreadable { pr: PrNumber::parse("7").unwrap() }, "could not read the head of #7".to_string()),
            (DepsRefusal::HeadMoved { pr: PrNumber::parse("7").unwrap(), real_head_8: "⟦real_head_8⟧".to_string(), sha_8: "⟦sha_8⟧".to_string() }, "#7 is at ⟦real_head_8⟧, not ⟦sha_8⟧. It moved while you were reading it.".to_string()),
            (DepsRefusal::DiffUnreadable { pr: PrNumber::parse("7").unwrap(), diff: "⟦diff⟧".to_string() }, "could not read the diff for #7:\n\n    ⟦diff⟧".to_string()),
            (DepsRefusal::DiffEmpty { pr: PrNumber::parse("7").unwrap() }, "the diff for #7 is empty, so nothing was classified".to_string()),
            (DepsRefusal::ClassifierTimedOut { limit: "⟦limit⟧".to_string(), pr: PrNumber::parse("7").unwrap() }, "the dependency classifier ran past ⟦limit⟧s on #7, so nothing was classified".to_string()),
            (DepsRefusal::NotAPureBump { pr: PrNumber::parse("7").unwrap(), verdict: "⟦verdict⟧".to_string() }, "#7 is not a pure dependency bump, so it needs a real receipt:\n\n    ⟦verdict⟧".to_string()),
            (DepsRefusal::PinsUnreadable, "could not read the action pins out of the classifier verdict".to_string()),
            (DepsRefusal::ImagesUnreadable, "could not read the base images out of the classifier verdict".to_string()),
            (DepsRefusal::PinsUncounted, "could not count the action pins the classifier declared".to_string()),
            (DepsRefusal::ImagesUncounted, "could not count the base images the classifier declared".to_string()),
            (DepsRefusal::PipUnreadable, "could not read the pip packages out of the classifier verdict".to_string()),
            (DepsRefusal::PipUncounted, "could not count the pip packages the classifier declared".to_string()),
            (DepsRefusal::BranchUnaskable { repo: "⟦repo⟧".to_string(), tag: "⟦tag⟧".to_string(), headref: "⟦headref⟧".to_string() }, "could not ask GitHub whether ⟦repo⟧ has a branch named ⟦tag⟧, so the pin is unverified:\n\n    ⟦headref⟧".to_string()),
            (DepsRefusal::PinCommentNotABranch { action: "⟦action⟧".to_string(), tag: "⟦tag⟧".to_string(), headref: "⟦headref⟧".to_string() }, "the pin comment is not a branch: ⟦action⟧ names '⟦tag⟧', and GitHub\n  answers ⟦headref⟧ for refs/heads/⟦tag⟧. A name that resolves to another ref\n  is how a pin to a pull request's head passes as a branch.".to_string()),
            (DepsRefusal::CompareUnaskable { repo: "⟦repo⟧".to_string(), pinsha: "⟦pinsha⟧".to_string(), tag: "⟦tag⟧".to_string(), status: "⟦status⟧".to_string() }, "could not ask GitHub whether ⟦repo⟧ ⟦pinsha⟧ is on ⟦tag⟧, so the pin is unverified:\n\n    ⟦status⟧".to_string()),
            (DepsRefusal::PinNotOnBranch { action: "⟦action⟧".to_string(), pinsha_12: "⟦pinsha_12⟧".to_string(), tag: "⟦tag⟧".to_string(), repo: "⟦repo⟧".to_string(), status: "⟦status⟧".to_string() }, "the pin comment is not true: ⟦action⟧ is pinned to ⟦pinsha_12⟧,\n  which is not reachable from ⟦tag⟧ in ⟦repo⟧ (compare says '⟦status⟧'). A\n  branch pin off its branch is a commit nobody on that branch published.".to_string()),
            (DepsRefusal::TagUnaskable { tag: "⟦tag⟧".to_string(), repo: "⟦repo⟧".to_string(), refline: "⟦refline⟧".to_string() }, "could not ask GitHub for the tag ⟦tag⟧ in ⟦repo⟧, so the pin is unverified:\n\n    ⟦refline⟧".to_string()),
            (DepsRefusal::PinCommentNotATag { action: "⟦action⟧".to_string(), tag: "⟦tag⟧".to_string(), refname: "⟦refname⟧".to_string() }, "the pin comment is not a tag: ⟦action⟧ names '⟦tag⟧', and GitHub\n  answers ⟦refname⟧ for refs/tags/⟦tag⟧. A name that resolves to another\n  ref is how a pin to a pull request's head passes as a release.".to_string()),
            (DepsRefusal::TagNestedTooDeep { tag: "⟦tag⟧".to_string(), repo: "⟦repo⟧".to_string() }, "the tag ⟦tag⟧ in ⟦repo⟧ is nested more than 4 tag objects deep\n  without reaching a commit, so the pin is unverified.".to_string()),
            (DepsRefusal::TagObjectUnreadable { objsha_12: "⟦objsha_12⟧".to_string(), repo: "⟦repo⟧".to_string(), tag: "⟦tag⟧".to_string(), refline: "⟦refline⟧".to_string() }, "could not read the tag object ⟦objsha_12⟧ behind ⟦repo⟧ ⟦tag⟧, so the pin is unverified:\n\n    ⟦refline⟧".to_string()),
            (DepsRefusal::TagNotACommit { tag: "⟦tag⟧".to_string(), repo: "⟦repo⟧".to_string(), objtype: "⟦objtype⟧".to_string() }, "the tag ⟦tag⟧ in ⟦repo⟧ points at a ⟦objtype⟧, not a commit,\n  so no pin can be on it.".to_string()),
            (DepsRefusal::PinNotOnTag { action: "⟦action⟧".to_string(), pinsha_12: "⟦pinsha_12⟧".to_string(), repo: "⟦repo⟧".to_string(), tag: "⟦tag⟧".to_string(), upstream_12: "⟦upstream_12⟧".to_string() }, "the pin comment is not true: ⟦action⟧ is pinned to\n  ⟦pinsha_12⟧ and ⟦repo⟧ ⟦tag⟧ is ⟦upstream_12⟧. A pin whose comment names a\n  version it is not on is the shape a supply-chain swap hides in.".to_string()),
            (DepsRefusal::NoImageResolver { resolver: "⟦resolver⟧".to_string() }, "no image resolver at ⟦resolver⟧; run ./install.sh".to_string()),
            (DepsRefusal::ImageUnaskable { image: "⟦image⟧".to_string(), tag: "⟦tag⟧".to_string(), live: "⟦live⟧".to_string() }, "could not ask the registry what ⟦image⟧:⟦tag⟧ is,\n  so the pin is unverified:\n\n    ⟦live⟧".to_string()),
            (DepsRefusal::ImageDigestMismatch { image: "⟦image⟧".to_string(), tag: "⟦tag⟧".to_string(), live_19: "⟦live_19⟧".to_string(), digest_19: "⟦digest_19⟧".to_string() }, "the base image pin is not the tag it claims: ⟦image⟧:⟦tag⟧\n  resolves to ⟦live_19⟧ and the Dockerfile pins ⟦digest_19⟧. A digest\n  that is not the one its tag names is the shape a base-image swap hides in.".to_string()),
            (DepsRefusal::NoPypiVerifier { verifier: "⟦verifier⟧".to_string() }, "no PyPI verifier at ⟦verifier⟧; run ./install.sh".to_string()),
            (DepsRefusal::PypiUnconfirmed { pkg: "⟦pkg⟧".to_string(), version: "⟦version⟧".to_string(), answer: "⟦answer⟧".to_string() }, "could not confirm ⟦pkg⟧==⟦version⟧ against PyPI,\n  so the pin is unverified:\n\n    ⟦answer⟧".to_string()),
            (DepsRefusal::PipFilesUnreadable, "could not read the pip files out of the classifier verdict".to_string()),
            (DepsRefusal::NoWholeFileCheck { wf: "⟦wf⟧".to_string() }, "no whole-file pip check at ⟦wf⟧; run ./install.sh".to_string()),
            (DepsRefusal::BaseBranchUnreadable { pr: PrNumber::parse("7").unwrap() }, "could not read the base branch of #7".to_string()),
            (DepsRefusal::MergeBaseUnaskable { pr: PrNumber::parse("7").unwrap() }, "could not ask GitHub for the merge base of #7".to_string()),
            (DepsRefusal::FileAtMergeBaseUnreadable { pf: "⟦pf⟧".to_string(), merge_base_8: "⟦merge_base_8⟧".to_string() }, "could not read ⟦pf⟧ at the merge base ⟦merge_base_8⟧".to_string()),
            (DepsRefusal::FileAtHeadUnreadable { pf: "⟦pf⟧".to_string(), real_head_8: "⟦real_head_8⟧".to_string() }, "could not read ⟦pf⟧ at the head ⟦real_head_8⟧".to_string()),
            (DepsRefusal::NotAVersionBump { pf: "⟦pf⟧".to_string(), got: "⟦got⟧".to_string() }, "⟦pf⟧, read whole the way pip reads it, is not a version bump:\n\n    ⟦got⟧".to_string()),
            (DepsRefusal::PipForFileUnreadable { pf: "⟦pf⟧".to_string() }, "could not read the pip packages for ⟦pf⟧ out of the verdict".to_string()),
            (DepsRefusal::WholeFileAnswerUnreadable { pf: "⟦pf⟧".to_string() }, "could not read the whole-file answer for ⟦pf⟧".to_string()),
            (DepsRefusal::DiffAndFileDisagree { pf: "⟦pf⟧".to_string(), want_300: "⟦want_300⟧".to_string(), got_300: "⟦got_300⟧".to_string() }, "the diff and the whole file disagree about ⟦pf⟧:\n  diff:  ⟦want_300⟧\n  files: ⟦got_300⟧\n  A bump the two readings describe differently is not one this gate can vouch for.".to_string()),
            (DepsRefusal::CannotRecord, "could not record the dependency receipt".to_string()),
            (DepsRefusal::PinsUnchecked { declared: 2, checked: 0 }, "the classifier declared 2 action pin(s) and 0\n  were verified. A receipt is a claim about all of them, so this one is not\n  written. Nothing here says the bump is bad; it says the check did not run.".to_string()),
            (DepsRefusal::ImagesUnchecked { declared: 2, checked: 1 }, "the classifier declared 2 base image(s) and\n  1 were verified. No receipt written: the check did not run.".to_string()),
            (DepsRefusal::PipUnchecked { declared: 3, checked: 1 }, "the classifier declared 3 pip package(s) and\n  1 were verified. No receipt written: the check did not run.".to_string()),
            (DepsRefusal::NotAPlainPath { path: "../x".to_string() }, "the requirements path '../x' is not a plain path, so it is not put in an API URL".to_string()),
            (DepsRefusal::ActionNotOwnerRepo { action: "checkout".to_string() }, "the action 'checkout' is not owner/repo[/dir] with plain segments, so it is not put in an API URL".to_string()),
            (DepsRefusal::NoMergeBase { pr: PrNumber::parse("7").unwrap(), got: "x".to_string() }, "GitHub gave no merge base for #7: 'x'".to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The position of a variant in `samples()`; no `_` arm, so a new variant fails to
    /// compile here until it has an index and a row.
    fn rank(value: &DepsRefusal) -> usize {
        match value {
            DepsRefusal::PinsUnchecked { .. } => 39,
            DepsRefusal::ImagesUnchecked { .. } => 40,
            DepsRefusal::PipUnchecked { .. } => 41,
            DepsRefusal::NotAPlainPath { .. } => 42,
            DepsRefusal::ActionNotOwnerRepo { .. } => 43,
            DepsRefusal::NoMergeBase { .. } => 44,
            DepsRefusal::NoClassifier { .. } => 0,
            DepsRefusal::HeadUnreadable { .. } => 1,
            DepsRefusal::HeadMoved { .. } => 2,
            DepsRefusal::DiffUnreadable { .. } => 3,
            DepsRefusal::DiffEmpty { .. } => 4,
            DepsRefusal::ClassifierTimedOut { .. } => 5,
            DepsRefusal::NotAPureBump { .. } => 6,
            DepsRefusal::PinsUnreadable => 7,
            DepsRefusal::ImagesUnreadable => 8,
            DepsRefusal::PinsUncounted => 9,
            DepsRefusal::ImagesUncounted => 10,
            DepsRefusal::PipUnreadable => 11,
            DepsRefusal::PipUncounted => 12,
            DepsRefusal::BranchUnaskable { .. } => 13,
            DepsRefusal::PinCommentNotABranch { .. } => 14,
            DepsRefusal::CompareUnaskable { .. } => 15,
            DepsRefusal::PinNotOnBranch { .. } => 16,
            DepsRefusal::TagUnaskable { .. } => 17,
            DepsRefusal::PinCommentNotATag { .. } => 18,
            DepsRefusal::TagNestedTooDeep { .. } => 19,
            DepsRefusal::TagObjectUnreadable { .. } => 20,
            DepsRefusal::TagNotACommit { .. } => 21,
            DepsRefusal::PinNotOnTag { .. } => 22,
            DepsRefusal::NoImageResolver { .. } => 23,
            DepsRefusal::ImageUnaskable { .. } => 24,
            DepsRefusal::ImageDigestMismatch { .. } => 25,
            DepsRefusal::NoPypiVerifier { .. } => 26,
            DepsRefusal::PypiUnconfirmed { .. } => 27,
            DepsRefusal::PipFilesUnreadable => 28,
            DepsRefusal::NoWholeFileCheck { .. } => 29,
            DepsRefusal::BaseBranchUnreadable { .. } => 30,
            DepsRefusal::MergeBaseUnaskable { .. } => 31,
            DepsRefusal::FileAtMergeBaseUnreadable { .. } => 32,
            DepsRefusal::FileAtHeadUnreadable { .. } => 33,
            DepsRefusal::NotAVersionBump { .. } => 34,
            DepsRefusal::PipForFileUnreadable { .. } => 35,
            DepsRefusal::WholeFileAnswerUnreadable { .. } => 36,
            DepsRefusal::DiffAndFileDisagree { .. } => 37,
            DepsRefusal::CannotRecord => 38,
        }
    }

    #[test]
    fn every_variant_has_exactly_one_sample() {
        let table = samples();
        assert_eq!(table.len(), 45, "one row per variant");
        for (at, (value, _)) in table.iter().enumerate() {
            assert_eq!(rank(value), at, "row {at} is out of order: {value:?}");
        }
    }

    #[test]
    fn every_variant_prints_the_text_of_its_bash_line() {
        for (value, bash) in samples() {
            assert_eq!(value.to_string(), bash, "{value:?}");
        }
    }

    #[test]
    fn every_static_line_is_a_line_of_a_die_or_say_in_the_bash_gate() {
        let texts: Vec<String> = samples().into_iter().map(|(_, text)| text).collect();
        crate::anchor::check_static_lines(&texts);
    }

    #[test]
    fn exit_codes_are_the_ones_of_the_bash() {
        let mut seen = std::collections::BTreeSet::new();
        for (value, _) in samples() {
            seen.insert(value.exit_code());
        }
        assert_eq!(seen, std::collections::BTreeSet::from([1]));
    }
}
