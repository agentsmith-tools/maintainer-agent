//! The four jobs of `lib/profile.sh`, for the Rust orchestrator and gate to call.
//!
//! - [`load_profile`] is `maintainer_load_profile`: defaults, then a profile,
//!   then `MAINTAINER_<KEY>` overrides, then the five exported names.
//! - [`scrub::scrub_secret`] is `maintainer_scrub_secret`: remove a literal
//!   secret from every text file under some paths.
//! - [`classify::classify_failure`] is `maintainer_classify_failure`, with the two
//!   predicates `backend_transient_reason` and `backend_needs_human_reason` on
//!   [`classify::Classification`], and [`classify::failure_shapes_path`] for
//!   `maintainer_failure_shapes_path`.
//! - [`table`] reads `lib/failure-shapes.json` the way the bash reads it, quirks
//!   included.
//!
//! The behaviour is the bash's, byte for byte, and each quirk that is kept has a
//! test whose name says it and whose comment names the plan decision. Where the
//! port differs on purpose, the module docs say so and number the decision
//! (`P1F-n`, listed in the pull request that added this crate).
//!
//! # No re-implementation of shell or regex
//!
//! `defaults.env` and `profile.env` are shell code, so a child `bash` evaluates
//! them ([`load`], [`shell`]). The failure shapes are POSIX extended regular
//! expressions with leftmost-longest matching, which the `regex` crate does not
//! do, so `grep` runs them through the `Proc` port ([`classify`]). Both run with
//! an empty environment plus an allowlist, a timeout and a process group.
//!
//! # What the library does not do
//!
//! It reads no environment and writes no terminal. Callers pass what the bash
//! function read from its surroundings ([`LoadEnv`], [`classify::ClassifyEnv`])
//! and print what the results carry. Only `mx-ports` starts processes, and the
//! scrubber starts none. Files are read and written with `std::fs`, because the
//! `Fs` port has no real adapter yet.
//!
//! # The oracle
//!
//! `mx-profile-oracle` (`src/bin`) prints what each bash function prints and
//! exits with the same code. `tests/differential.sh` runs the bash function and
//! the oracle on the same inputs and compares stdout, stderr, exit code and
//! the resulting files or variables.

pub mod classify;
pub mod load;
pub mod scrub;
mod shell;
pub mod table;

pub use load::{Load, LoadEnv, LoadError, Profile, load_profile};

#[cfg(test)]
mod testutil;
