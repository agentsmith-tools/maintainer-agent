//! What can go wrong in the sandbox, with the bash message where bash has one.

use std::path::PathBuf;
use thiserror::Error;

/// A sandbox step refused or failed.
///
/// A message that quotes a bash line says which line. The ones that do not were
/// added by this port (decision `P30-n` in the pull request) and say so.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SandboxError {
    /// No value for a verify resource in the environment, the suite or the profile.
    // bash: ${!key:?lib/defaults.env was not loaded}
    #[error("{key}: lib/defaults.env was not loaded")]
    LimitUnset { key: &'static str },
    /// The value is not a positive amount. Added by this port (P30-3): bash passes
    /// `0` to the runtime, which reads it as no limit at all.
    #[error("{name} is '{value}', which is not a positive {what}")]
    InvalidLimit { name: &'static str, value: String, what: &'static str },
    /// The timeout is not a whole number of seconds above 0. Added by this port
    /// (P30-4): podman reads `--timeout 0` as no timeout.
    #[error("the container timeout is '{value}', which is not a whole number of seconds above 0")]
    BadTimeout { value: String },
    /// The tree a container mounts is not an absolute path. Added by this port
    /// (P30-5): `-v name:/repo` names a volume, not a directory.
    #[error("the container's work directory '{path}' is not an absolute path")]
    WorkNotAbsolute { path: String },
    /// The path has a `:`, which would end the host half of `-v host:/repo:z`. P30-5.
    #[error(
        "the container's work directory '{path}' has a ':', which the runtime reads as the end of the host path"
    )]
    WorkHasColon { path: String },
    /// The path is not valid UTF-8, so it cannot be one argument. P30-5.
    #[error("the container's work directory is not valid UTF-8")]
    WorkNotText,
    /// `docker run` has no `--timeout` (docker 29.10.0 lists only `--stop-timeout`),
    /// and the flag set of the bash passes it. Refused at construction (P30-20,
    /// rv264 O3), so the failure names itself and is not read as "the test does not
    /// pass unmutated".
    #[error(
        "docker run has no --timeout, so the verify container cannot be bounded under docker; use podman"
    )]
    RuntimeHasNoRunTimeout,
    /// An unvalidated filter or other text is not allowed in a shell string. The
    /// mutation changed no file (bash: `cmd_verify`, "the mutation changed nothing").
    #[error("the mutation changed nothing, so this run says nothing about the guard")]
    NothingChanged { globs: Vec<String> },
    /// `sh -c ''` runs nothing and exits 0. P30-5.
    #[error("the container would run an empty command")]
    EmptyCommand,
    /// `git read-tree` failed.
    // bash: printf 'read-tree %s failed\n' "$ref"
    #[error("read-tree {rev} failed")]
    ReadTree { rev: String },
    /// `git checkout-index` failed.
    // bash: printf 'checkout-index of %s into %s failed\n' "$ref" "$dir"
    #[error("checkout-index of {rev} into {} failed", dir.display())]
    CheckoutIndex { rev: String, dir: PathBuf },
    /// `git ls-tree` failed.
    // bash: printf 'ls-tree %s failed\n' "$ref"
    #[error("ls-tree {rev} failed")]
    LsTree { rev: String },
    /// The revision tracks no file that is not a submodule.
    // bash: printf '%s lists no files\n' "$ref"
    #[error("{rev} lists no files")]
    NoFiles { rev: String },
    /// Fewer, or more, files arrived than the revision lists.
    // bash: printf 'extracted %s of the %s files in %s\n' "$((got))" "$want" "$ref"
    #[error("extracted {got} of the {want} files in {rev}")]
    PartialExtract { rev: String, want: u64, got: u64 },
    /// The scratch directory could not be made.
    // bash: die "cannot create $VERIFY_WORK" / "cannot create a scratch directory in $VERIFY_WORK"
    #[error("cannot create a scratch directory in {}", dir.display())]
    ScratchDir { dir: PathBuf },
    /// `git init` or `git add -A -f` failed in the extracted tree, so a test that asks git
    /// what is tracked will fail for that reason (the gate prints a note and goes on).
    #[error("could not seed a git index in the extracted tree")]
    IndexNotSeeded,
    /// `find` or `sed --sandbox` failed, or the expression used `e`, `w` or `r`.
    // bash: die "mutation failed to apply, or used a sandboxed sed command (e/w/r are refused)"
    #[error("mutation failed to apply, or used a sandboxed sed command (e/w/r are refused)")]
    MutationFailed,
    /// The sed expression is empty (bash: `${4:?sed expression}`) or has a NUL, which no argument can hold.
    #[error("the sed expression is empty or contains a NUL byte")]
    BadSedExpression,
    /// The runtime or one of its helpers could not be started or did not finish.
    #[error("the container run did not finish: {reason}")]
    Run { reason: String },
}
