//! The `run` binary: `lib/run.sh` in Rust.
//!
//! This file reads the environment and the command line, builds the real process
//! adapters, and hands everything to [`mx_run::run`]. It holds no logic.
//!
//! Rust ignores `SIGPIPE`, so a write to a closed pipe is an error where the bash
//! program would have been killed by the signal. Restoring the default action
//! needs `unsafe` or a new dependency, and the workspace allows neither, so
//! [`PipeEnd`] runs the exit trap and ends the program with status 141 (128 +
//! `SIGPIPE`, what a shell reports) the moment a write meets a closed pipe.

use mx_ports::{RealPipe, RealProc};
use mx_run::env::Env;
use mx_run::exec::Adapters;
use mx_run::{Invocation, Services};
use std::ffi::OsString;
use std::io::{ErrorKind, Write};
use std::process::ExitCode;
use std::sync::Arc;

/// A stream that ends the program quietly when its reader has gone.
struct PipeEnd<W: Write>(W);

impl<W: Write> PipeEnd<W> {
    fn check<T>(result: std::io::Result<T>) -> std::io::Result<T> {
        if let Err(error) = &result
            && error.kind() == ErrorKind::BrokenPipe
        {
            mx_run::trap::run_now();
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
    let exe = std::env::current_exe().unwrap_or_default();
    // A caller that holds descriptor 9 (the machine's hook lock does) is the bash's case
    // of `exec 9>`, which replaces it: start again with it closed. The second image takes
    // its `argv[0]` from the variable the first one set.
    let called_as = args.first().cloned().unwrap_or_else(|| OsString::from("run.sh"));
    if let Some(error) =
        mx_run::reexec::free_descriptor_9(&exe, &called_as, args.get(1..).unwrap_or(&[]))
    {
        eprintln!("run.sh: could not start again with descriptor 9 closed: {error}");
    }
    mx_run::trap::install();
    let argv0 = mx_run::reexec::take_argv0().unwrap_or(called_as);
    let invocation = Invocation {
        argv0,
        exe,
        args: args.iter().skip(1).map(|arg| arg.to_string_lossy().into_owned()).collect(),
        cwd: std::env::current_dir().unwrap_or_else(|_| "/".into()),
        env: {
            let mut env = Env::from_process();
            env.unset(mx_run::reexec::ARGV0_VAR);
            env
        },
    };
    let mut out = PipeEnd(std::io::stdout());
    let mut err = PipeEnd(std::io::stderr());
    let adapters = |path: &std::ffi::OsStr, locale: Option<String>| Adapters {
        proc: Arc::new(RealProc::new(path, Vec::new())),
        pipe: Arc::new(RealPipe::new(path, locale)),
    };
    let mut services = Services {
        out: &mut out,
        err: &mut err,
        adapters: &adapters,
        backend: &mx_backends::backend_by_name,
        open_lock: &mx_run::lock::open_run_lock,
    };
    let code = mx_run::run(&invocation, &mut services);
    ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1))
}
