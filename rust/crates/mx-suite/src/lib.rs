//! The suite plugin interface of the merge gate.
//!
//! `profiles/<name>/verify.d/*.sh` are bash files, one per suite, each defining
//! functions the gate calls (`suite_covers`, `suite_image`, `suite_command`,
//! `suite_ran`, ...). They stay bash (plan decision D1), so the Rust gate asks
//! them questions by starting `bash` through `mx_ports::Proc`, one question per
//! process, and never sources one in its own address space.
//!
//! This crate is the interface and nothing else: the value types a question
//! and its answer are made of, the [`SuiteHost`] and [`Suite`] ports, and a fake
//! of each ([`FakeSuiteHost`], [`FakeSuite`]). The real host, suite discovery,
//! the coverage rules (`is_production`, `covers_production`,
//! `suites_cover_production`, `pick_suite`, `coverage_map`) and the refusals
//! that report them are the work of the suite agent (brief `p3a`), against
//! these signatures. A change to a signature here is a change to the
//! briefs of p3a, p3b and p3c, so it goes in a pull request all three read.
//!
//! # What each bash function becomes
//!
//! | bash | Rust |
//! |---|---|
//! | `suite_dir` | [`SuiteHost::dir`] |
//! | `"$dir"/*.sh` | [`SuiteHost::names`] |
//! | `. "$dir/$name.sh"` | [`SuiteHost::load`], one [`Suite`] |
//! | `suite_covers` | [`Suite::covers`] |
//! | `suite_image` | [`SuiteSpec::image`] |
//! | `suite_needs` | [`SuiteSpec::needs`] |
//! | `suite_mutate_glob` | [`SuiteSpec::mutate_globs`] |
//! | `suite_named_only` | [`SuiteSpec::named_only`] |
//! | `suite_command <filter>` | [`Suite::command`] |
//! | `suite_ran <log>` | [`Suite::ran`] |
//! | `suite_podman_args`, `suite_extra_args` | [`Suite::extra_args`] |
//! | `suite_requires`, `suite_requires_name` | [`Suite::requires`] |
//! | `suite_memory`, `suite_cpus`, `suite_pids`, `suite_tmpfs` | [`Suite::limit`] |
//! | `suite_prefetch_command`, `suite_prefetch_guard` | [`SuiteSpec::prefetch`], [`Suite::prefetch_guard`] |
//! | `PROD_GLOBS` | [`ProdGlobs`] |
//!
//! # Decisions
//!
//! `P30-n` are this pull request's; each is listed in its body. F3 is the plan's:
//! a suite name from the command line or from a receipt is a [`SuiteName`], and
//! no function that reaches a suite file takes a string.

mod choose;
mod filter;
mod glob;
mod host;
mod image;
mod name;
mod prod;
mod real;
mod record;
mod selector;
mod spec;
pub mod text;

pub use choose::HostSelector;
pub use filter::{TestFilter, TestFilterError};
pub use glob::{MutateGlob, MutateGlobError};
pub use host::{FakeSuite, FakeSuiteHost, Suite, SuiteCall, SuiteError, SuiteHost};
pub use image::{ImageRef, ImageRefError};
pub use name::{SuiteName, SuiteNameError};
pub use prod::{NoProdGlobs, ProdGlobs};
pub use real::{RealSuiteHost, SUITE_ENV_NAMES, SuiteDirs};
pub use record::Recorder;
pub use selector::{
    CoverageFailure, CoverageRow, FakeSelector, PickRefusal, Picked, SuiteSelector,
};
pub use spec::{LimitKey, NoMutateGlob, Prefetch, Runtime, SuiteSpec};
