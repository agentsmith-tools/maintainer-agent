//! Every way `install` declines to go on before it changes anything, with the status
//! and the words the bash gives.
//!
//! [`Refusal::exit_code`] has no catch-all arm: a new refusal does not compile until
//! it has a status, and a test fails when a status changes.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;

/// The reasons a run ends before its first write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A word that is not a flag. Exit 64.
    UnknownFlag(OsString),
    /// `--rust-bin` with nothing after it. Exit 64 (decision P4A-add-1).
    RustBinNeedsValue,
    /// `--rust` without `--rust-bin DIR`. Exit 64.
    RustNeedsBin,
    /// `--rust-bin DIR` without `--rust`. Exit 64.
    RustBinNeedsRust,
    /// `--rollback` next to `--rust`, `--uninstall` or `--timers`. Exit 64.
    RollbackAlone,
    /// `--rust` next to `--uninstall`. Exit 64.
    RustWithUninstall,
    /// No `flock` on the search path. Exit 1.
    FlockMissing,
    /// A run holds the lock of these profiles. Exit 69.
    RunInFlight { share: OsString, profiles: Vec<OsString> },
    /// This many maintainer timers are enabled, and nobody said yes. Exit 3.
    LiveTimers { count: usize },
    /// `HOME` is not set. Exit 1, as bash's `set -u` ends the script.
    HomeUnset { program: &'static str, line: u32 },
    /// The directory given to `--rust-bin` is unusable. Exit 1.
    RustBinUnusable { detail: OsString },
    /// `--rollback` with no stamp to roll back. Exit 1.
    NothingToRollBack { stamp: OsString },
    /// `--rollback` cannot restore what the stamp lists. Exit 1.
    RollbackBlocked { detail: OsString },
}

impl Refusal {
    /// The status the program ends with.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::UnknownFlag(_)
            | Self::RustBinNeedsValue
            | Self::RustNeedsBin
            | Self::RustBinNeedsRust
            | Self::RollbackAlone
            | Self::RustWithUninstall => 64,
            Self::FlockMissing
            | Self::HomeUnset { .. }
            | Self::RustBinUnusable { .. }
            | Self::NothingToRollBack { .. }
            | Self::RollbackBlocked { .. } => 1,
            Self::RunInFlight { .. } => 69,
            Self::LiveTimers { .. } => 3,
        }
    }

    /// What goes to standard error, newline included.
    #[must_use]
    pub fn stderr(&self) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let mut line = |text: &[u8]| {
            out.extend_from_slice(text);
            out.push(b'\n');
        };
        match self {
            Self::UnknownFlag(flag) => line(&cat(&[b"install.sh: unknown flag ", flag.as_bytes()])),
            Self::RustBinNeedsValue => line(b"install.sh: --rust-bin needs a directory"),
            Self::RustNeedsBin => line(b"install.sh: --rust needs --rust-bin DIR"),
            Self::RustBinNeedsRust => line(b"install.sh: --rust-bin needs --rust"),
            Self::RollbackAlone => line(
                b"install.sh: --rollback stands alone; only --dry-run and --force go with it",
            ),
            Self::RustWithUninstall => line(b"install.sh: --rust cannot go with --uninstall"),
            Self::FlockMissing => line(b"install.sh: flock is required to install and protect active runs; install flock and ensure it is on PATH."),
            Self::RunInFlight { share, profiles } => {
                let mut names: Vec<u8> = Vec::new();
                for name in profiles {
                    names.push(b' ');
                    names.extend_from_slice(name.as_bytes());
                }
                line(&cat(&[b"install.sh: a run is in flight for:", &names]));
                line(&cat(&[
                    b"            This replaces ",
                    share.as_bytes(),
                    b"/profiles, which that run reads for",
                ]));
                line(b"            the whole of its pass.");
                line(b"");
                line(b"            Wait for it (maintainer status), or --force if you are sure.");
            }
            Self::LiveTimers { count } => {
                line(format!("install.sh: {count} maintainer timer(s) are enabled on this machine.").as_bytes());
                line(b"            Removing them stops every profile, including any that");
                line(b"            maintains a repository other than this one.");
                line(b"");
                line(b"            Re-run with MAINTAINER_UNINSTALL_YES=1 to go ahead, or");
                line(b"            --dry-run to see what would be removed.");
            }
            Self::HomeUnset { program, line: at } => {
                line(format!("{program}: line {at}: HOME: unbound variable").as_bytes());
            }
            Self::RustBinUnusable { detail } => {
                line(&cat(&[b"install.sh: --rust-bin: ", detail.as_bytes()]));
            }
            Self::NothingToRollBack { stamp } => line(&cat(&[
                b"install.sh: nothing to roll back: ",
                stamp.as_bytes(),
                b" does not exist",
            ])),
            Self::RollbackBlocked { detail } => {
                line(&cat(&[b"install.sh: cannot roll back: ", detail.as_bytes()]));
            }
        }
        out
    }
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// A path or name as the OS string it is.
#[must_use]
pub fn os(bytes: &[u8]) -> OsString {
    OsStr::from_bytes(bytes).to_os_string()
}
