//! What a suite declares, read once.

use crate::{ImageRef, MutateGlob, SuiteName};
use mx_core::NonEmpty;
use std::fmt;
use thiserror::Error;

/// The container runtime `container_runtime` found: podman if it is there,
/// docker otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Podman,
    Docker,
}

impl Runtime {
    /// The program name, and the argument `suite_requires` and `suite_podman_args`
    /// receive.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Podman => "podman",
            Self::Docker => "docker",
        }
    }
}

impl fmt::Display for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One verify resource. A suite may define a function for each, and
/// `lib/defaults.env` holds the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKey {
    Memory,
    Cpus,
    Pids,
    Tmpfs,
}

impl LimitKey {
    /// Every key, in the order the `podman run` line lists them (`--tmpfs`,
    /// `--pids-limit`, `--memory`, `--cpus`).
    pub const ALL: [LimitKey; 4] = [Self::Tmpfs, Self::Pids, Self::Memory, Self::Cpus];

    /// The key in `lib/defaults.env` and `profile.env`.
    #[must_use]
    pub fn defaults_key(self) -> &'static str {
        match self {
            Self::Memory => "VERIFY_MEMORY",
            Self::Cpus => "VERIFY_CPUS",
            Self::Pids => "VERIFY_PIDS",
            Self::Tmpfs => "VERIFY_TMPFS",
        }
    }

    /// The environment variable that overrides it for one run.
    #[must_use]
    pub fn env_name(self) -> &'static str {
        match self {
            Self::Memory => "MAINTAINER_VERIFY_MEMORY",
            Self::Cpus => "MAINTAINER_VERIFY_CPUS",
            Self::Pids => "MAINTAINER_VERIFY_PIDS",
            Self::Tmpfs => "MAINTAINER_VERIFY_TMPFS",
        }
    }

    /// The function a suite may define to override the default.
    #[must_use]
    pub fn suite_function(self) -> &'static str {
        match self {
            Self::Memory => "suite_memory",
            Self::Cpus => "suite_cpus",
            Self::Pids => "suite_pids",
            Self::Tmpfs => "suite_tmpfs",
        }
    }
}

/// The network fetch a suite asks for before its tests run (`suite_prefetch_command`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefetch {
    /// What the networked container runs, as the text `sh -c` receives.
    pub command: String,
    /// Whether the suite also defines `suite_prefetch_guard`, which reads the
    /// extracted tree on the host before the container gets a network.
    pub guarded: bool,
}

/// What a suite file declares without being asked about a path or a log.
///
/// The functions that depend on an argument (`suite_covers`, `suite_command`,
/// `suite_ran`, `suite_podman_args`, `suite_requires`, `suite_prefetch_guard`,
/// and the four limits) are methods of [`crate::Suite`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuiteSpec {
    /// The file stem.
    pub name: SuiteName,
    /// `suite_image`.
    pub image: ImageRef,
    /// `suite_needs`: the programs the image has to contain, one word each.
    /// `maintainer-doctor` checks them; the gate does not.
    pub needs: Vec<String>,
    /// `suite_mutate_glob`: one pattern per line, empty lines dropped.
    pub mutate_globs: Vec<MutateGlob>,
    /// The suite defines `suite_named_only`: inference never picks it.
    pub named_only: bool,
    /// `suite_prefetch_command`, when the suite defines it.
    pub prefetch: Option<Prefetch>,
}

/// A suite that declares no mutate glob cannot be mutated.
// bash: printf 'this suite declares no mutate glob\n' >&2; return 1
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("this suite declares no mutate glob")]
pub struct NoMutateGlob;

impl SuiteSpec {
    /// The mutate globs, at least one: a suite that declares none cannot be
    /// mutated, and `apply_mutation` refuses it.
    pub fn mutate_globs(&self) -> Result<NonEmpty<MutateGlob>, NoMutateGlob> {
        NonEmpty::from_vec(self.mutate_globs.clone()).map_err(|_| NoMutateGlob)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(globs: &[&str]) -> SuiteSpec {
        SuiteSpec {
            name: SuiteName::parse("rust").unwrap(),
            image: ImageRef::parse("docker.io/library/rust:1-slim").unwrap(),
            needs: vec!["bash".into(), "cargo".into(), "rustc".into()],
            mutate_globs: globs.iter().map(|g| MutateGlob::parse(g).unwrap()).collect(),
            named_only: false,
            prefetch: None,
        }
    }

    #[test]
    fn mutate_globs_come_back_in_declaration_order() {
        let globs = spec(&["*.sh", "*.py", "maintainer*"]).mutate_globs().unwrap();
        let shown: Vec<&str> = globs.iter().map(MutateGlob::as_str).collect();
        assert_eq!(shown, ["*.sh", "*.py", "maintainer*"]);
    }

    #[test]
    fn a_suite_with_no_mutate_glob_is_refused() {
        assert_eq!(spec(&[]).mutate_globs(), Err(NoMutateGlob));
        assert_eq!(NoMutateGlob.to_string(), "this suite declares no mutate glob");
    }

    #[test]
    fn the_runtime_names_are_the_program_names() {
        assert_eq!(Runtime::Podman.as_str(), "podman");
        assert_eq!(Runtime::Docker.to_string(), "docker");
    }

    #[test]
    fn each_limit_key_names_its_defaults_key_its_override_and_its_suite_function() {
        let rows: Vec<(&str, &str, &str)> = LimitKey::ALL
            .iter()
            .map(|k| (k.defaults_key(), k.env_name(), k.suite_function()))
            .collect();
        assert_eq!(
            rows,
            [
                ("VERIFY_TMPFS", "MAINTAINER_VERIFY_TMPFS", "suite_tmpfs"),
                ("VERIFY_PIDS", "MAINTAINER_VERIFY_PIDS", "suite_pids"),
                ("VERIFY_MEMORY", "MAINTAINER_VERIFY_MEMORY", "suite_memory"),
                ("VERIFY_CPUS", "MAINTAINER_VERIFY_CPUS", "suite_cpus"),
            ]
        );
    }
}
