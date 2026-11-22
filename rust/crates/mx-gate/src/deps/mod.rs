//! `maintainer-merge verify-deps`: the types of the dependency-bump receipt.
//!
//! `verify` proves a guard bites by mutating it. A lockfile bump has no guard, so
//! no mutation can be written and no receipt was earnable, and `merge` refused
//! every Dependabot pull request until `verify-deps` (2026-09-23). What it checks
//! instead is mechanical: every changed path is dependency metadata, no changed
//! line adds a source outside crates.io, npm or `PyPI`, and every pin, base-image
//! digest and pip hash is what its registry says it is. The first two checks are
//! `scripts/classify_dependency_bump.py` (Python, stays); the third needs the
//! network and is [`Net`].
//!
//! # Why a module of `mx-gate` and not a crate (decision `P30-6`)
//!
//! A crate `mx-deps` would need `PrNumber`, `CommitId` and the receipt evidence
//! that live here, and `mx-gate` would need `mx-deps` for the evidence a verified
//! dependency bump produces: a cycle. The types they share would move to
//! `mx-core`, a merged crate this pull request leaves alone. A directory module
//! gives the dependency agent (brief `p3c`) its own files and no edit in common
//! with the gate agent but this module's line in `lib.rs`.
//!
//! # What is here
//!
//! - [`Verdict`]: the classifier's JSON, as types. [`Bump`] or [`Refused`].
//! - [`Net`] and [`FakeNet`]: every call the checks make to GitHub, a registry or
//!   `PyPI`, one method each.
//! - [`Checked`]: the proof that the checks inspected every pin, image and pip
//!   package the classifier declared. A verifier that inspected nothing must not
//!   look like one that found nothing to inspect.
//! - [`PlainPath`], [`ActionRepo`]: the two strings that go into a GitHub API
//!   path, checked before they do.
//!
//! The order of the checks, the messages of each refusal, and the real
//! adapter of [`Net`] are the dependency agent's work.

mod checked;
mod net;
mod paths;
mod ports;
mod refusal;
mod verdict;

pub use checked::Checked;
pub use refusal::DepsRefusal;

#[cfg(test)]
pub(crate) fn refusal_texts() -> Vec<String> {
    refusal::samples().into_iter().map(|(_, text)| text).collect()
}
pub use net::{FakeNet, GitObject, Net, NetCall, NetFailure, TagRef};
pub use paths::{ActionRepo, PlainPath};
pub use ports::{
    Classifier, ClassifierCall, ClassifierExit, ClassifierRun, FakeClassifier, FakeWholeFile,
    HelperScript, WholeFile,
};
pub use verdict::{Bump, Image, LineRef, Pin, PipPackage, Refused, Verdict, VerdictError};
