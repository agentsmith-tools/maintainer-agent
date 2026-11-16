//! The `maintainer-repo` binary: `prune`, `release-check` and `fixup`.
//!
//! This file reads the environment and the command line, builds the real process
//! adapter, and hands everything to [`mx_repo::run`]. It holds no logic.
//!
//! Rust ignores `SIGPIPE`, so a write to a closed pipe is an error where the
//! bash program would have been killed by the signal. Restoring the default
//! action needs `unsafe` or a new dependency, and the workspace allows neither,
//! so [`PipeEnd`] ends the program with status 141 (128 + `SIGPIPE`, what a shell
//! reports) the moment a write meets a closed pipe (decision P1H-3).

use mx_ports::RealProc;
use mx_repo::ctx::Ctx;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{ErrorKind, Write};
use std::process::ExitCode;

/// bash's compiled-in default when `PATH` is unset.
const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// A stream that ends the program quietly when its reader has gone.
struct PipeEnd<W: Write>(W);

impl<W: Write> PipeEnd<W> {
    fn check<T>(result: std::io::Result<T>) -> std::io::Result<T> {
        if let Err(error) = &result
            && error.kind() == ErrorKind::BrokenPipe
        {
            std::process::exit(141);
        }
        result
    }
}

impl<W: Write> Write for PipeEnd<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        Self::check(self.0.write(bytes))
    }

    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        Self::check(self.0.write_all(bytes))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Self::check(self.0.flush())
    }
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    let argv0 = args.first().cloned().unwrap_or_else(|| OsString::from("maintainer-repo"));
    let vars: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    let search_path = vars.get("PATH").cloned().unwrap_or_else(|| DEFAULT_PATH.to_string());
    let proc = RealProc::new(search_path, Vec::new());
    let exe = std::env::current_exe().unwrap_or_default();
    let mut out = PipeEnd(std::io::stdout());
    let mut err = PipeEnd(std::io::stderr());
    let mut ctx = Ctx::new(&proc, argv0, exe, vars, &mut out, &mut err);
    let code = mx_repo::run(&mut ctx, args.get(1..).unwrap_or(&[]));
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
