//! What every check shares: the ports, the settings the profile resolved, the
//! output sinks, and the few helpers that stand for a shell construct.

use crate::host::HostFs;
use crate::report::{Item, Report};
use crate::text;
use mx_ports::{Clock, Proc, ProcExit, ProcSpec};
use std::collections::BTreeMap;
use std::time::Duration;

/// Time allowed to one external call. The bash has no limits; each limit is
/// generous and a call that reaches it reads as a failure with status 124
/// (decision P1I-7).
pub mod limit {
    use std::time::Duration;
    pub const QUICK: Duration = Duration::from_mins(1);
    pub const GH: Duration = Duration::from_mins(3);
    pub const GIT: Duration = Duration::from_mins(3);
    pub const PODMAN: Duration = Duration::from_mins(30);
    pub const SYSTEMD_RUN: Duration = Duration::from_mins(10);
    pub const SOURCING: Duration = Duration::from_mins(1);
}

/// Why a check ends the run or the section early, as a bash error would.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The shell exits with this status.
    Exit(i32),
    /// An expansion error aborts the whole compound command it sits in. The
    /// audit trail is one `if`, so the rest of that section does not run.
    AbortSection,
}

/// The report and standard error, written as the checks run.
#[derive(Debug, Default)]
pub struct Out {
    pub report: Report,
    pub stderr: Vec<u8>,
}

impl Out {
    pub fn add(&mut self, item: Item) {
        self.report.push(item);
    }
    pub fn warn_text(&mut self, text: &str) {
        self.stderr.extend_from_slice(text.as_bytes());
    }
}

/// The caller's surroundings that checks read.
#[derive(Debug, Clone, Default)]
pub struct Env {
    pub home: String,
    /// `$USER`, when set and non-empty.
    pub user: Option<String>,
    /// `$TMPDIR`, when set and non-empty.
    pub tmpdir: Option<String>,
}

/// The variables the profile resolved, by name. Values are text; a byte that is
/// not UTF-8 becomes U+FFFD (decision P1I-9).
#[derive(Debug, Clone, Default)]
pub struct Settings {
    vars: BTreeMap<String, String>,
}

impl Settings {
    #[must_use]
    pub fn new(vars: BTreeMap<String, String>) -> Self {
        Self { vars }
    }

    /// `${NAME:-}`: the value, or the empty string when unset.
    #[must_use]
    pub fn get(&self, name: &str) -> &str {
        self.vars.get(name).map_or("", String::as_str)
    }

    /// True when the variable is set, even to the empty string.
    #[must_use]
    pub fn is_set(&self, name: &str) -> bool {
        self.vars.contains_key(name)
    }

    /// `${NAME:-fallback}`: the fallback when unset or empty.
    #[must_use]
    pub fn get_or<'a>(&'a self, name: &str, fallback: &'a str) -> &'a str {
        match self.vars.get(name) {
            Some(value) if !value.is_empty() => value,
            _ => fallback,
        }
    }

    /// `$NAME` under `set -u`: an unset variable ends the shell.
    pub fn need(&self, name: &str, line: u32, out: &mut Out) -> Result<&str, Stop> {
        if let Some(value) = self.vars.get(name) {
            Ok(value)
        } else {
            out.warn_text(&format!("maintainer-doctor: line {line}: {name}: unbound variable\n"));
            Err(Stop::Exit(127))
        }
    }
}

/// Everything a check may use.
pub struct Ctx<'a> {
    pub proc: &'a dyn Proc,
    pub clock: &'a dyn Clock,
    pub host: &'a dyn HostFs,
    pub env: &'a Env,
    pub settings: &'a Settings,
    /// The profile being checked.
    pub profile: &'a str,
    /// `$HOME/.local/share/maintainer`.
    pub share: String,
    pub quick: bool,
    /// Variables for the child bash that sources a verify suite: what the
    /// profile resolved, as the subshell of the bash doctor would see.
    pub suite_env: &'a [(String, String)],
}

/// What a finished call left.
#[derive(Debug, Clone)]
pub struct Call {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Call {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.code == 0
    }
    /// `$(...)` of the standard output.
    #[must_use]
    pub fn text(&self) -> String {
        text::captured(&self.stdout)
    }
}

impl Ctx<'_> {
    /// Runs `argv` with no shell. A tool that is not on the search path gives
    /// `None`, which is the shell's "command not found" (status 127, no output).
    #[must_use]
    pub fn run(&self, argv: &[&str], timeout: Duration) -> Option<Call> {
        self.run_with(argv, timeout, &[])
    }

    #[must_use]
    pub fn run_with(&self, argv: &[&str], timeout: Duration, env: &[(&str, &str)]) -> Option<Call> {
        let mut spec = ProcSpec::command(argv[0], argv[1..].iter().copied(), timeout);
        for (name, value) in env {
            spec = spec.env(*name, *value);
        }
        let output = self.proc.run(&spec).ok()?;
        let code = match output.exit {
            ProcExit::Code(code) => code,
            ProcExit::TimedOut => 124,
        };
        Some(Call { code, stdout: output.stdout, stderr: output.stderr })
    }

    /// `command -v NAME`: what bash resolves, which includes builtins.
    /// `Some(path)` when it resolves, `None` otherwise.
    #[must_use]
    pub fn command_v(&self, name: &str) -> Option<String> {
        let call = self.run(&["bash", "-c", "command -v \"$1\"", "_", name], limit::QUICK)?;
        call.ok().then(|| call.text())
    }

    /// `$(uname -s)`. Asked again at each place the bash asks, so the call log
    /// matches. A host with no `uname` answers the empty string.
    #[must_use]
    pub fn uname(&self) -> String {
        self.run(&["uname", "-s"], limit::QUICK).map(|call| call.text()).unwrap_or_default()
    }
}
