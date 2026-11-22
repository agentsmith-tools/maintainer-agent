//! The repository checks, in Rust: the five scripts that the local gate, the pre-commit hook and
//! the merge gate run over the tree itself.
//!
//! | binary               | script                           |
//! |----------------------|----------------------------------|
//! | `check-claims`       | `scripts/check_claims.sh`        |
//! | `shellcheck-sweep`   | `scripts/shellcheck-sweep.sh`    |
//! | `verify-action-pins` | `scripts/verify-action-pins.sh`  |
//! | `run-evals`          | `evals/run-evals.sh`             |
//! | `containment-probe`  | `tests/containment-probe.sh`     |
//!
//! Each program is a library function over facts read through two ports, plus a thin binary:
//! [`ctx::Ctx`] holds the process port (`mx_ports::Proc`, the only way to start anything) and the
//! file system and search path ([`host::Host`]). The binary reads the environment and the command
//! line, builds the real adapters and calls the function. A program prints what the script prints,
//! on the same stream, with the same exit code, and makes the same calls to the same tools in the
//! same order with the same arguments.
//!
//! `scripts/with-hook-lock.sh` and `.githooks/pre-commit` stay bash (decision `P4-D23`): the gate
//! that tests the Rust build must not depend on the Rust build.
//!
//! # Decisions
//!
//! Each program's module lists the `P4B-n` decisions it applies. The ones every program shares:
//!
//! - `P4B-1`: an inline `python3 -c` program of the script runs through `python3` with the script's text, so
//!   the call log is the script's and a malformed input fails the way it fails today.
//! - `P4B-3`: a child gets the caller's whole environment but `PATH` and `LC_ALL` (the process port owns
//!   those), as the script's children do.
//! - `P4B-4`: no call is unbounded. A child that runs past two hours is killed and reads as exit 124.
//! - `P4B-5`: a child's two streams arrive separately. What the script leaves on the terminal is passed on as
//!   standard output and then standard error, and `$(cmd 2>&1)` holds the same two in that order (the reading
//!   `P2A-merge` made).
//! - `P4B-6`: names and lines sort by bytes, the order the scripts get under `LC_ALL=C`. A glob (`profiles/*`,
//!   `evals/scenarios/*.md`) lists in byte order too, where the shell uses the locale's collation.
//! - `P4B-12`: a SIGHUP, SIGINT or SIGTERM removes the temporary files, as the scripts' `EXIT` trap does, and
//!   the program then dies of the signal ([`cleanup`]).
//! - `P4B-13`: a temporary file or directory that cannot be made ends the program with one line and exit 1,
//!   where the script goes on and fails in a cascade of shell messages.
//!
//! The decisions only one program applies (`P4B-2`, `P4B-14`, `P4B-16` in `pins`, for instance) are in that
//! program's module.
//!
//! Not reproduced, and said so where it matters: a count with a leading zero is octal in the shell's
//! arithmetic (here decimal); an argument that is not UTF-8 reaches a child with U+FFFD in its place; a child
//! starts with an empty standard input where the shell's child inherits the caller's; a missing coreutil
//! (`sort`, `grep`, `cp`) is not noticed because those steps run in the process; and a child keeps running
//! when a signal names only this process, because the process port starts each child in a group of its own.
//!
//! # Layout
//!
//! - [`shell`]: the shell constructs the scripts lean on, as pure functions (`dirname`, `read`,
//!   `$(...)`, `sed` and `grep` over bytes).
//! - [`host`]: the file system and `command -v`, with a real adapter and an in-memory fake.
//! - [`ctx`]: the surroundings of one run.
//! - [`claims`], [`sweep`], [`pins`], [`evals`] and [`probe`]: the five programs. Each module's
//!   docs list its decisions (`P4B-n`) and what it found and did not fix.
//! - [`stdio`]: what every `main` shares.
//!
//! Where the script does something dangerous or wrong that the plan does not list, the port keeps
//! it, a case in `tests/characterization/checks-*.sh` pins it, and the module docs say so under
//! "Found, not fixed".

pub mod claims;
pub mod cleanup;
pub mod ctx;
pub mod evals;
pub mod host;
pub mod pins;
pub mod probe;
pub mod shell;
pub mod stdio;
pub mod sweep;
