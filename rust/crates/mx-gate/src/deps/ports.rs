//! The local helpers `verify-deps` starts: the classifier and the whole-file pip check.
//!
//! `Net::image_digest` and `Net::pypi_release` stand for `scripts/resolve_image_digest.py` and
//! `scripts/verify_pypi_release.py`, which the bash starts as scripts. The adapter of those two
//! methods starts the same scripts with the same arguments and must give the same answers, so
//! the port keeps the bash's behaviour and drops only the shell.

use crate::deps::{DepsRefusal, NetFailure, PlainPath};
use mx_ports::Script;
use mx_suite::Recorder;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// How the classifier ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifierExit {
    /// Exit 0 is a bump and 1 is a refusal; both print JSON.
    Code(i32),
    /// `timeout` exited 124: the classifier ran past its limit (`MAINTAINER_CLASSIFY_TIMEOUT`,
    /// default `CLASSIFY_TIMEOUT` 60 s). Out of time is a refusal, never a pass.
    TimedOut,
}

/// What `printf '%s\n' "$diff" | timeout "$limit" python3 classify_dependency_bump.py` left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifierRun {
    pub exit: ClassifierExit,
    pub stdout: String,
}

/// `scripts/classify_dependency_bump.py`, bounded.
pub trait Classifier {
    /// Runs the classifier on `diff` (the net diff, `gh pr diff` without `--patch`) with a time
    /// limit. It reads text a pull request author wrote, and a pattern that backtracks on it once
    /// took over 30 s on 1.6 KB, so the limit is part of the call.
    fn classify(&self, diff: &str, limit: Duration) -> Result<ClassifierRun, NetFailure>;
}

/// `scripts/verify_pip_files.py <base> <head>`: reads two whole requirements files the way pip does.
pub trait WholeFile {
    /// What it printed on success (JSON of the requirements whose version moved), or its output on failure.
    fn check(&self, path: &PlainPath, base: &[u8], head: &[u8]) -> Result<String, NetFailure>;
}

/// A helper script `verify-deps` needs on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperScript {
    Classifier,
    ImageResolver,
    PypiVerifier,
    WholeFile,
}

impl HelperScript {
    /// The refusal when the script is not at `path` ("no X at <path>; run ./install.sh").
    #[must_use]
    pub fn missing(self, path: &str) -> DepsRefusal {
        let path = path.to_string();
        match self {
            Self::Classifier => DepsRefusal::NoClassifier { classifier: path },
            Self::ImageResolver => DepsRefusal::NoImageResolver { resolver: path },
            Self::PypiVerifier => DepsRefusal::NoPypiVerifier { verifier: path },
            Self::WholeFile => DepsRefusal::NoWholeFileCheck { wf: path },
        }
    }
}

/// One call to a [`FakeClassifier`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifierCall {
    pub diff: String,
    pub limit: Duration,
}

/// A scripted [`Classifier`].
#[derive(Debug, Default)]
pub struct FakeClassifier {
    calls: Recorder<ClassifierCall>,
    answers: Mutex<Script<Result<ClassifierRun, NetFailure>>>,
}

impl FakeClassifier {
    /// A fake with no scripted answers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Scripts the answers.
    pub fn script(&self, script: Script<Result<ClassifierRun, NetFailure>>) {
        *self.answers.lock().unwrap_or_else(PoisonError::into_inner) = script;
    }

    /// Every call so far.
    #[must_use]
    pub fn calls(&self) -> Vec<ClassifierCall> {
        self.calls.entries()
    }
}

impl Classifier for FakeClassifier {
    fn classify(&self, diff: &str, limit: Duration) -> Result<ClassifierRun, NetFailure> {
        self.calls.push(ClassifierCall { diff: diff.into(), limit });
        self.answers.lock().unwrap_or_else(PoisonError::into_inner).next("Classifier::classify")
    }
}

/// A scripted [`WholeFile`].
#[derive(Debug, Default)]
pub struct FakeWholeFile {
    calls: Recorder<(String, Vec<u8>, Vec<u8>)>,
    answers: Mutex<Script<Result<String, NetFailure>>>,
}

impl FakeWholeFile {
    /// A fake with no scripted answers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Scripts the answers.
    pub fn script(&self, script: Script<Result<String, NetFailure>>) {
        *self.answers.lock().unwrap_or_else(PoisonError::into_inner) = script;
    }

    /// Every call: the path, the base bytes and the head bytes.
    #[must_use]
    pub fn calls(&self) -> Vec<(String, Vec<u8>, Vec<u8>)> {
        self.calls.entries()
    }
}

impl WholeFile for FakeWholeFile {
    fn check(&self, path: &PlainPath, base: &[u8], head: &[u8]) -> Result<String, NetFailure> {
        self.calls.push((path.as_str().into(), base.to_vec(), head.to_vec()));
        self.answers.lock().unwrap_or_else(PoisonError::into_inner).next("WholeFile::check")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_script_names_itself_and_the_install_hint() {
        // bash: die "no dependency classifier at $classifier; run ./install.sh" and its three twins.
        assert_eq!(
            HelperScript::Classifier.missing("/s/classify.py").to_string(),
            "no dependency classifier at /s/classify.py; run ./install.sh"
        );
        assert_eq!(
            HelperScript::ImageResolver.missing("/s/r.py").to_string(),
            "no image resolver at /s/r.py; run ./install.sh"
        );
        assert_eq!(
            HelperScript::PypiVerifier.missing("/s/v.py").to_string(),
            "no PyPI verifier at /s/v.py; run ./install.sh"
        );
        assert_eq!(
            HelperScript::WholeFile.missing("/s/w.py").to_string(),
            "no whole-file pip check at /s/w.py; run ./install.sh"
        );
    }

    #[test]
    fn the_classifier_fake_records_the_diff_and_the_limit_and_plays_a_timeout() {
        let classifier = FakeClassifier::new();
        classifier.script(Script::sequence([
            Ok(ClassifierRun { exit: ClassifierExit::TimedOut, stdout: String::new() }),
            Ok(ClassifierRun { exit: ClassifierExit::Code(0), stdout: "{}".into() }),
        ]));
        let limit = Duration::from_mins(1);
        assert_eq!(classifier.classify("diff", limit).unwrap().exit, ClassifierExit::TimedOut);
        assert_eq!(classifier.classify("diff2", limit).unwrap().exit, ClassifierExit::Code(0));
        assert_eq!(
            classifier.calls(),
            vec![
                ClassifierCall { diff: "diff".into(), limit },
                ClassifierCall { diff: "diff2".into(), limit }
            ]
        );
    }

    #[test]
    fn the_whole_file_fake_records_both_files() {
        let whole = FakeWholeFile::new();
        whole.script(Script::always(Ok("{\"changed\": []}".into())));
        let path = PlainPath::parse("requirements.txt").unwrap();
        assert_eq!(whole.check(&path, b"old", b"new").unwrap(), "{\"changed\": []}");
        assert_eq!(
            whole.calls(),
            vec![("requirements.txt".into(), b"old".to_vec(), b"new".to_vec())]
        );
    }

    #[test]
    #[should_panic(expected = "unscripted call: no answer left for Classifier::classify")]
    fn an_unscripted_classifier_call_panics() {
        drop(FakeClassifier::new().classify("d", Duration::from_secs(1)));
    }
}
