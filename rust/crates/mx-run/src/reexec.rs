//! Taking descriptor 9 when the caller already holds it.
//!
//! The bash does `exec 9>"$STATE_DIR/run.lock"`, which replaces whatever descriptor
//! 9 was, so a run started by something that holds descriptor 9 (the machine's own
//! `with-hook-lock.sh` holds its lock there, and every program its hook starts
//! inherits it) works. The port must work the same way (decision `P2A-fd9`).
//!
//! The standard library has no safe call that closes a descriptor by number, so
//! [`free_descriptor_9`] starts the program again through a shell that closes it
//! (`mx_ports::exec_with_descriptor_closed`). The new image knows it was started
//! that way by [`ARGV0_VAR`], which carries the name the program was called by, and
//! does not start again. If descriptor 9 is still open then, the run lock refuses.

use std::ffi::{OsStr, OsString};
use std::path::Path;

/// Carries the original `argv[0]` into the second image. [`take_argv0`] reads it
/// and the caller removes it from the environment it passes on.
pub const ARGV0_VAR: &str = "MX_RUN_ARGV0";

/// Whether descriptor 9 is open in this process.
#[must_use]
pub fn descriptor_9_is_open() -> bool {
    std::fs::read_link("/proc/self/fd/9").is_ok()
}

/// The `argv[0]` a previous image passed on, if this process is the second image.
#[must_use]
pub fn take_argv0() -> Option<OsString> {
    std::env::var_os(ARGV0_VAR)
}

/// Starts this program again with descriptor 9 closed when it is open and this is
/// the first image. Returns only when nothing was done (`None`) or the new start
/// failed (`Some(error)`, after which the run lock refuses, as it would without this).
#[must_use]
pub fn free_descriptor_9(exe: &Path, argv0: &OsStr, args: &[OsString]) -> Option<std::io::Error> {
    if !descriptor_9_is_open() || take_argv0().is_some() {
        return None;
    }
    Some(mx_ports::exec_with_descriptor_closed(9, exe, args, &[(ARGV0_VAR, argv0)]))
}
