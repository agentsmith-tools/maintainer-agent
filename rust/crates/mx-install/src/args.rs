//! The command line of `install`.
//!
//! The bash loops over the words once and ends the program at the first one it does
//! not know (exit 64, before any output), so `--dry-run --bogus` prints nothing and
//! refuses. The three flags added for the cutover (`--rust`, `--rust-bin DIR`,
//! `--rollback`, decision P4A-add-1) follow the same rule and are checked for sense
//! after the whole line is read, still before any output.

use crate::refusal::Refusal;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

/// What the flags ask for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one bool per flag, which is what the command line is"
)]
pub struct Opts {
    pub dry: bool,
    pub timers: bool,
    pub uninstall: bool,
    pub force: bool,
    /// `--rust`: install the Rust builds found in `rust_bin`.
    pub rust: bool,
    pub rust_bin: Option<PathBuf>,
    /// `--rollback`: put the bash originals back.
    pub rollback: bool,
}

/// Reads the words after the program name.
pub fn parse(args: &[OsString]) -> Result<Opts, Refusal> {
    let mut opts = Opts::default();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match word.to_str() {
            Some("--dry-run") => opts.dry = true,
            Some("--timers") => opts.timers = true,
            Some("--uninstall") => opts.uninstall = true,
            Some("--force") => opts.force = true,
            Some("--rust") => opts.rust = true,
            Some("--rollback") => opts.rollback = true,
            Some("--rust-bin") => {
                let dir = words.next().ok_or(Refusal::RustBinNeedsValue)?;
                opts.rust_bin = Some(PathBuf::from(dir));
            }
            _ => return Err(Refusal::UnknownFlag(OsStr::new(word).to_os_string())),
        }
    }
    if opts.rust_bin.is_some() && !opts.rust {
        return Err(Refusal::RustBinNeedsRust);
    }
    if opts.rust && opts.rust_bin.is_none() {
        return Err(Refusal::RustNeedsBin);
    }
    if opts.rust && opts.uninstall {
        return Err(Refusal::RustWithUninstall);
    }
    if opts.rollback && (opts.rust || opts.uninstall || opts.timers) {
        return Err(Refusal::RollbackAlone);
    }
    Ok(opts)
}
