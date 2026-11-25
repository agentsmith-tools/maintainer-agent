//! The sandbox interface of the merge gate.
//!
//! `verify` unpacks a pull request's files, mutates one of them on the host, and
//! runs the suite's tests twice in a container. Each step is a function of
//! `bin/maintainer-merge` that holds an incident in its comments. This crate is
//! the interface and the checks that belong to the values; it starts no process
//! and runs no container. The real adapter (`RealSandbox`) is the work of the
//! suite agent (brief `p3a`), against these signatures, and starts everything
//! through `mx_ports`.
//!
//! - [`Sandbox`]: `extract_tree`, `tree_escapes`, `apply_mutation`, `run_container`,
//!   and [`FakeSandbox`], which records every call.
//! - [`ContainerSpec`]: the exact `podman run` flag set as data, for the verify
//!   run and for the networked prefetch, with the limits' precedence
//!   ([`Limits::resolve`]) and the unquoted splitting of `suite_podman_args`
//!   ([`ExtraArgs::split`]).
//! - [`ExtractedTree`] then [`ContainedTree`]: a tree that arrived whole, then a
//!   tree with no link leaving it. Only the second reaches the mutation, which
//!   runs on the host.
//! - [`SedExpression`] and [`Mutated`]: the expression, and the proof it changed
//!   something.
//!
//! # Decisions
//!
//! `P30-n` are this pull request's, each listed in its body.
//!
//! - P30-3: a limit of zero, or one that is not a plain amount, is refused.
//! - P30-4: a timeout of zero is refused.
//! - P30-5: a work directory that is relative, has a `:`, or is not UTF-8 is
//!   refused, and so is an empty command.
//! - P30-10, P30-11, P30-12: found in the bash and pinned, not fixed.

mod container;
mod error;
mod limits;
mod mutation;
mod real;
mod realpath;
mod sandbox;
mod tree;

pub use container::{ContainerInputs, ContainerSpec, LogMode, Purpose};
pub use error::SandboxError;
pub use limits::{ExtraArgs, Limit, LimitSource, Limits, Timeout};
pub use mutation::{Mutated, NothingChanged, SedExpression};
pub use real::{RUNTIME_ENV_NAMES, RealSandbox, container_runtime, pick_env};
pub use realpath::realpath;
pub use sandbox::{ContainerExit, FakeSandbox, Sandbox, SandboxCall};
pub use tree::{ContainedTree, Escape, EscapingTree, ExtractedTree};
