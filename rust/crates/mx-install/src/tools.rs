//! Starting the programs the installer needs, through the `Proc` port.
//!
//! Every call has a time limit (decision P4A-limits): the bash has none, a call that
//! reaches its limit counts as a failure with status 124, and the message says so. A
//! program that is not on the search path reads as the shell reads it: status 127 and
//! `<script>: line N: <program>: command not found`.

use crate::console::{Console, Flow};
use crate::env::Env;
use mx_ports::{Proc, ProcError, ProcExit, ProcOutput, ProcSpec};
use std::time::Duration;

/// A question that has a quick answer: `flock`, `uname`, `date`, `git`, `bash`.
pub const QUICK: Duration = Duration::from_mins(3);
/// A command that changes something: `systemctl`, `crontab`, `python3`, a removal script.
pub const COMMAND: Duration = Duration::from_mins(30);

/// How a call ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ran {
    Done(ProcOutput),
    /// Not on the search path (status 127).
    Missing,
    /// On the path and not executable, or refused by the system (status 126).
    Denied(String),
    /// Over its limit.
    TimedOut,
}

/// Runs one program.
pub fn call(proc: &dyn Proc, spec: &ProcSpec) -> Ran {
    match proc.run(spec) {
        Ok(output) if output.exit == ProcExit::TimedOut => Ran::TimedOut,
        Ok(output) => Ran::Done(output),
        Err(ProcError::NotFound { .. }) => Ran::Missing,
        Err(other) => Ran::Denied(other.to_string()),
    }
}

/// A call with the variables a child may see.
#[must_use]
pub fn spec(env: &Env, program: &str, args: &[&str], timeout: Duration) -> ProcSpec {
    let mut spec = ProcSpec::command(program, args.iter().map(|a| (*a).to_string()), timeout);
    for (name, value) in env.child_vars() {
        spec = spec.env(name, value);
    }
    spec
}

impl Ran {
    /// The exit status a shell would see.
    #[must_use]
    pub fn status(&self) -> i32 {
        match self {
            Self::Done(output) => match output.exit {
                ProcExit::Code(code) => code,
                ProcExit::TimedOut => 124,
            },
            Self::Missing => 127,
            Self::Denied(_) => 126,
            Self::TimedOut => 124,
        }
    }

    /// Standard output of a finished call, or nothing.
    #[must_use]
    pub fn stdout(&self) -> &[u8] {
        match self {
            Self::Done(output) => &output.stdout,
            _ => &[],
        }
    }

    /// True when the program ran and exited 0.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.status() == 0 && matches!(self, Self::Done(_))
    }
}

/// Runs a program the way a shell command runs it: what it prints reaches the program's
/// own streams, and a program that is not there says so in the shell's words.
pub fn run_visible(
    proc: &dyn Proc,
    con: &mut Console,
    spec: &ProcSpec,
    script: &str,
    line: u32,
) -> Flow<i32> {
    let program = spec.argv.first().clone();
    match call(proc, spec) {
        Ran::Done(output) => {
            con.out(&output.stdout)?;
            con.err(&output.stderr)?;
            Ok(Ran::Done(output).status())
        }
        Ran::Missing => {
            con.echo_err(
                format!("{script}: line {line}: {program}: command not found").as_bytes(),
            )?;
            Ok(127)
        }
        Ran::Denied(reason) => {
            con.echo_err(format!("{script}: line {line}: {program}: {reason}").as_bytes())?;
            Ok(126)
        }
        Ran::TimedOut => {
            con.echo_err(
                format!("{script}: {program}: gave no answer in time and was stopped").as_bytes(),
            )?;
            Ok(124)
        }
    }
}
