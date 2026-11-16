//! `maintainer-doctor` in Rust.
//!
//! The doctor checks that the maintainer can run, by running things. This crate
//! keeps every message, every exit code and the order of every external call of
//! `bin/maintainer-doctor`, and changes one thing on purpose: the pinned-token
//! scan reads the files in this process and never puts the token in a child's
//! argument vector or environment (plan decision F4, recorded as P1I-F4).
//!
//! # Shape
//!
//! Each section reads its facts through a port and then decides in a pure
//! function over those facts: [`install`], [`github`], [`hostchecks`] and
//! [`audit`]. [`run::run`] chains them. External programs go through
//! `mx_ports::Proc`, the file system through [`host::HostFs`], time through
//! `mx_ports::Clock`, and profile loading through `mx_profile`.
//!
//! # Differences from the bash, besides F4
//!
//! Each is a numbered decision in the pull request that added this crate.
//! A profile file is evaluated by a child bash that sees the caller's
//! environment, so ambient `BACKEND` or `POST` still reach it. The tools the
//! doctor starts see an allowlist of variables. Every call has a time limit.
//! Output bytes that are not UTF-8 are shown as U+FFFD.

pub mod audit;
pub mod ctx;
pub mod github;
pub mod host;
pub mod hostchecks;
pub mod install;
pub mod pyval;
pub mod report;
pub mod run;
pub mod text;

pub use run::{Outcome, RunInput, SystemClock, run};
