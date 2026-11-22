//! `lib/run.sh` in Rust: one unattended maintenance pass that leaves an auditable
//! trail.
//!
//! The binary `run` has the command line of the bash, `run [--show-prompt]
//! <profile> <task>`, the same exit codes (0, 1, 64, 75, 78), the same lines on
//! standard output and standard error, the same log, the same effects on the state
//! directory and the same calls to `gh`, `git`, `flock`, `notify-send` and the
//! Python helper `maintainer`, in the same order with the same arguments. The
//! helper stays Python: this crate calls it through the `Proc` port exactly as the
//! bash does.
//!
//! # Layout
//!
//! - [`cadence`], [`identity`], [`humanise`] and [`prompt`] are pure: the cadence
//!   gate, what to make of each answer of `gh`, the toast text and the prompt
//!   bytes. Each is tested without a process.
//! - [`env`], [`exec`], [`host`], [`lock`], [`trap`] and [`tree`] are the places
//!   the program meets the machine: the exported environment, the process ports,
//!   `command -v`, descriptor 9, the exit trap and the tree it was installed into.
//! - [`pass`] is the lifecycle, step for step, with the line numbers of `run.sh`
//!   in its comments.
//!
//! # Decisions
//!
//! `D10` to `D19` are the plan's. `P2A-n` are this crate's: each is named in the
//! module that applies it, and the pull request lists them with "Found, not fixed".
//!
//! - D10, D11, D12, D13, D15 and D18 are fixed, because each fails open today.
//! - D14, D16, D17 and D19 are preserved, with a test whose name says so.
//! - `P2A-env`: a child gets the exported environment name by name ([`env`]).
//! - `P2A-merge`: `$(cmd 2>&1)` reads standard output and then standard error
//!   ([`exec`]).
//! - `P2A-fd9`: descriptor 9 already in use is a refusal ([`lock`]).
//! - `P2A-x1`: a `GH_TRIES` or `GH_BACKOFF` that is not a whole number refuses.
//! - `P5B-1`: a helper, tool or agent that is a text file with no `#!` line runs as a
//!   shell script, as bash starts it. The fix is in the process adapters of `mx-ports`,
//!   so every program that starts one gets it; `helper-without-interpreter-line` in
//!   `tests/differential.py` pins it for this crate.

pub mod cadence;
pub mod env;
pub mod exec;
pub mod host;
pub mod humanise;
pub mod identity;
pub mod lock;
pub mod pass;
pub mod prompt;
pub mod reexec;
pub mod trap;
pub mod tree;

pub use pass::{Invocation, Pass, Services, Stop};

/// Runs one pass and returns the exit code. The exit trap has run when this
/// returns, and it also runs if a step panics.
pub fn run(invocation: &Invocation, services: &mut Services<'_>) -> i32 {
    match Pass::begin(invocation, services) {
        Err(Stop(code)) => code,
        Ok(mut pass) => {
            // Declared after `pass`, so it drops first: when a step panics the unwinding runs
            // the exit trap and only then closes descriptor 9. The run lock stays held until the
            // scrub has finished, as on a normal return.
            let _guard = trap::ExitGuard;
            let flow = pass.go();
            // The trap runs before descriptor 9 closes, as the bash's does.
            trap::run_now();
            // A signal that arrived meanwhile ends the process by the signal, not by this code.
            trap::checkpoint();
            drop(pass);
            match flow {
                Ok(()) => 0,
                Err(Stop(code)) => code,
            }
        }
    }
}
