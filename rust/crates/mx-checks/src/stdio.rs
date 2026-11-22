//! What every `main` shares: the real adapters, the two output streams and the exit status.
//!
//! Rust ignores `SIGPIPE`, so a write to a closed pipe is an error where the bash script would
//! have been killed by the signal. Restoring the default action needs `unsafe`, which the
//! workspace forbids, so [`PipeEnd`] ends the program with status 141 (128 + `SIGPIPE`, what a
//! shell reports) the moment a write meets a closed pipe.

use crate::ctx::{Ctx, DEFAULT_PATH};
use crate::host::RealHost;
use mx_ports::RealProc;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{ErrorKind, Write};
use std::path::PathBuf;
use std::process::ExitCode;

/// A stream that ends the program quietly when its reader has gone.
pub struct PipeEnd<W: Write>(pub W);

impl<W: Write> PipeEnd<W> {
    fn check<T>(result: std::io::Result<T>) -> std::io::Result<T> {
        if let Err(error) = &result
            && error.kind() == ErrorKind::BrokenPipe
        {
            crate::cleanup::run_now();
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

/// Builds the real adapters and the context, runs `run` with the command line (without the
/// program name) and turns its answer into the process exit status.
pub fn main_with(run: impl FnOnce(&mut Ctx<'_>, &[OsString]) -> i32) -> ExitCode {
    crate::cleanup::install();
    let args: Vec<OsString> = std::env::args_os().collect();
    let argv0 = args.first().cloned().unwrap_or_default();
    let exe = std::env::current_exe().unwrap_or_default();
    let vars: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    let search_path = vars.get("PATH").cloned().unwrap_or_else(|| DEFAULT_PATH.to_string());
    let proc = RealProc::new(search_path, Vec::new());
    let host = RealHost;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let mut out = PipeEnd(std::io::stdout());
    let mut err = PipeEnd(std::io::stderr());
    let mut ctx = Ctx::new(&proc, &host, argv0, exe, vars, cwd, &mut out, &mut err);
    let code = run(&mut ctx, args.get(1..).unwrap_or(&[]));
    ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1))
}
