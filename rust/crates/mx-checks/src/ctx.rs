//! What every check needs: the caller's environment, the process port, the file system, the
//! two output streams, and the way to run a child the way the bash runs one.

use crate::host::Host;
use crate::shell;
use mx_ports::{Proc, ProcError, ProcExit, ProcSpec};
use mx_profile::classify::effective_locale;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// bash's compiled-in default when `PATH` is unset.
pub const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// How long one child may run unless a call says otherwise. The bash has no limit; the `Proc`
/// port needs one, so a call that has not finished in two hours is killed (decision P4B-4).
pub const CHILD_TIMEOUT: Duration = Duration::from_hours(2);

/// A check ended the program: its exit code. The message, if any, is already on standard error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stop(pub i32);

/// The result of a step that can end the program.
pub type R<T> = Result<T, Stop>;

/// What a child left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    /// The exit code as the shell reports it: 127 when the program was not found, 126 when it
    /// could not be started, 124 when it ran past its timeout.
    pub rc: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// False when the program could not be started at all. Then `stderr` holds the shell's
    /// `command not found` line, for a caller whose bash command did not redirect it away.
    pub started: bool,
}

impl Done {
    /// `if cmd; then`.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.rc == 0
    }

    /// What `cmd 2>&1` prints: standard output, then standard error (decision `P4B-5`, the same
    /// reading of a merged capture as `P2A-merge`). `$(...)` then strips it: see [`Ctx::substitution`].
    #[must_use]
    pub fn merged(&self) -> Vec<u8> {
        let mut both = self.stdout.clone();
        both.extend_from_slice(&self.stderr);
        both
    }
}

/// One child to run.
#[derive(Debug, Clone)]
pub struct Call {
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Variables set for this call on top of the caller's environment (`VAR=x cmd`).
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
    /// The line of the bash script that runs this call, for the `line N:` its
    /// `command not found` message carries.
    pub line: Option<u32>,
}

impl Call {
    /// A call with the default timeout.
    #[must_use]
    pub fn new<S: Into<String>>(argv: impl IntoIterator<Item = S>) -> Self {
        Self {
            argv: argv.into_iter().map(Into::into).collect(),
            cwd: None,
            env: Vec::new(),
            timeout: CHILD_TIMEOUT,
            line: None,
        }
    }

    /// Names the line of the bash script that this call stands for.
    #[must_use]
    pub fn at(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }

    /// Runs in `dir`.
    #[must_use]
    pub fn cwd(mut self, dir: Option<&Path>) -> Self {
        self.cwd = dir.map(Path::to_path_buf);
        self
    }

    /// Sets one variable for this call.
    #[must_use]
    pub fn with_env(mut self, name: &str, value: &str) -> Self {
        self.env.push((name.to_string(), value.to_string()));
        self
    }

    /// Bounds the call.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// The program's surroundings.
pub struct Ctx<'a> {
    proc: &'a dyn Proc,
    host: &'a dyn Host,
    /// `$0` as the program was started, for the messages bash puts in front of its own
    /// diagnostics, and the place the tree is found from.
    pub argv0: OsString,
    /// The running executable, used to find the tree when `argv0` has no slash.
    pub exe: PathBuf,
    /// The caller's whole environment (UTF-8 entries).
    pub vars: BTreeMap<String, String>,
    /// The working directory the program was started in.
    pub cwd: PathBuf,
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
    child_env: Vec<(String, String)>,
    locale: Option<String>,
}

impl<'a> Ctx<'a> {
    /// A context over `proc` and `host`, writing to `out` and `err`.
    #[allow(
        clippy::too_many_arguments,
        reason = "each one is a distinct fact about the surroundings"
    )]
    pub fn new(
        proc: &'a dyn Proc,
        host: &'a dyn Host,
        argv0: OsString,
        exe: PathBuf,
        vars: BTreeMap<String, String>,
        cwd: PathBuf,
        out: &'a mut dyn Write,
        err: &'a mut dyn Write,
    ) -> Self {
        // A child gets the caller's whole environment but PATH and LC_ALL, which the `Proc`
        // port owns (decision P4B-3), as the bash children get theirs.
        let child_env = vars
            .iter()
            .filter(|(name, _)| name.as_str() != "PATH" && name.as_str() != "LC_ALL")
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let locale = effective_locale(
            vars.get("LC_ALL").map(String::as_str),
            vars.get("LC_CTYPE").map(String::as_str),
            vars.get("LANG").map(String::as_str),
        );
        Self { proc, host, argv0, exe, vars, cwd, out, err, child_env, locale }
    }

    /// The file system.
    #[must_use]
    pub fn host(&self) -> &dyn Host {
        self.host
    }

    /// Writes bytes to standard output now, so output and error keep their order.
    pub fn put_out(&mut self, bytes: &[u8]) {
        // A closed pipe ends the real program in `main`'s writer; a failure here has
        // nowhere better to go.
        drop(self.out.write_all(bytes));
        drop(self.out.flush());
    }

    /// Writes bytes to standard error now.
    pub fn put_err(&mut self, bytes: &[u8]) {
        drop(self.err.write_all(bytes));
        drop(self.err.flush());
    }

    /// One line on standard output.
    pub fn out_line(&mut self, text: &str) {
        self.put_out(format!("{text}\n").as_bytes());
    }

    /// One line on standard error.
    pub fn err_line(&mut self, text: &str) {
        self.put_err(format!("{text}\n").as_bytes());
    }

    /// The `$0` text bash prints before its own diagnostics.
    #[must_use]
    pub fn script_name(&self) -> OsString {
        self.argv0.clone()
    }

    /// `$(...)`: the bytes without trailing newlines and without NUL bytes. Bash drops each NUL and
    /// says so once per substitution, on standard error, with the line of the script that holds it.
    pub fn substitution(&mut self, bytes: &[u8], line: u32) -> Vec<u8> {
        if bytes.contains(&0) {
            let mut text = self.script_name().as_bytes().to_vec();
            text.extend_from_slice(
                format!(
                    ": line {line}: warning: command substitution: ignored null byte in input\n"
                )
                .as_bytes(),
            );
            self.put_err(&text);
        }
        shell::command_substitution(bytes)
    }

    /// `PATH` as the caller had it, or the shell's default.
    #[must_use]
    pub fn search_path(&self) -> OsString {
        OsString::from(self.vars.get("PATH").map_or(DEFAULT_PATH, String::as_str))
    }

    /// `command -v NAME`.
    #[must_use]
    pub fn on_path(&self, name: &str) -> bool {
        self.host.on_path(name, &self.search_path())
    }

    /// `${TMPDIR:-/tmp}`, where `mktemp` makes its files.
    #[must_use]
    pub fn tmpdir(&self) -> PathBuf {
        match self.vars.get("TMPDIR") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => PathBuf::from("/tmp"),
        }
    }

    /// The directory of the program, as `$(dirname "${BASH_SOURCE[0]}")` gives it: the text of
    /// `argv0` up to its last slash, or `.` when it has none. A program started without a slash
    /// (a name found on `PATH`) is where the running executable is.
    #[must_use]
    pub fn script_dir(&self) -> PathBuf {
        let given = self.argv0.as_bytes();
        let source: Vec<u8> = if given.contains(&b'/') {
            given.to_vec()
        } else {
            self.exe.as_os_str().as_bytes().to_vec()
        };
        PathBuf::from(std::ffi::OsStr::from_bytes(&shell::dirname(&source)))
    }

    /// The caller's working directory as the shell would show it: `PWD` when it names the
    /// current directory, else the real one.
    #[must_use]
    pub fn logical_cwd(&self) -> PathBuf {
        if let Some(pwd) = self.vars.get("PWD") {
            let named = Path::new(pwd);
            if named.is_absolute() && self.host.real_path(named) == self.host.real_path(&self.cwd) {
                return named.to_path_buf();
            }
        }
        self.cwd.clone()
    }

    /// `$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)`: the tree this program lives in, the
    /// parent of its directory, with `..` taken out by the text of the path and no link followed.
    #[must_use]
    pub fn tree_root(&self) -> PathBuf {
        shell::logical_parent(&self.script_dir(), &self.logical_cwd())
    }

    /// Runs a child. The environment is the caller's whole environment, then the call's own
    /// variables, then the `Proc` port's `PATH` and `LC_ALL`.
    #[must_use]
    pub fn exec(&self, call: &Call) -> Done {
        let Some(program) = call.argv.first() else {
            return Done { rc: 127, stdout: Vec::new(), stderr: Vec::new(), started: false };
        };
        let mut spec = ProcSpec::command(program, call.argv[1..].iter().cloned(), call.timeout);
        for (name, value) in self.child_env.iter().chain(&call.env) {
            spec = spec.env(name.as_str(), value.as_str());
        }
        if let Some(locale) = &self.locale {
            spec = spec.locale(locale.as_str());
        }
        if let Some(dir) = &call.cwd {
            spec = spec.cwd(dir.as_path());
        }
        match self.proc.run(&spec) {
            Ok(output) => {
                let rc = match output.exit {
                    ProcExit::Code(code) => code,
                    ProcExit::TimedOut => 124,
                };
                Done { rc, stdout: output.stdout, stderr: output.stderr, started: true }
            }
            Err(error) => {
                let name = self.script_name();
                let line = call.line.unwrap_or(0);
                let (rc, text) = match &error {
                    ProcError::NotFound { .. } | ProcError::RelativeProgram { .. } => {
                        (127, shell::command_not_found(&name, line, program))
                    }
                    other => {
                        let mut text = name.as_bytes().to_vec();
                        text.extend_from_slice(
                            format!(": line {line}: {program}: {other}\n").as_bytes(),
                        );
                        (126, text)
                    }
                };
                Done { rc, stdout: Vec::new(), stderr: text, started: false }
            }
        }
    }

    /// Lets a child's standard output through, as the bash does when it does not capture it.
    pub fn show_out(&mut self, done: &Done) {
        self.put_out(&done.stdout);
    }

    /// Lets a child's standard error through.
    pub fn show_err(&mut self, done: &Done) {
        self.put_err(&done.stderr);
    }

    /// Both streams of a child the bash leaves on the terminal: output first, then error (the
    /// two arrive separately here, so their interleaving inside one call is not kept; decision
    /// P4B-5).
    pub fn show_both(&mut self, done: &Done) {
        self.show_out(done);
        self.show_err(done);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::FakeHost;
    use mx_ports::{EffectLog, FakeProc, Script};

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
    }

    #[test]
    fn a_child_gets_every_variable_but_path_and_lc_all() {
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        proc.script(&["tool"], Script::always(Ok(FakeProc::exited(0, ""))));
        let host = FakeHost::new();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let ctx = Ctx::new(
            &proc,
            &host,
            "x".into(),
            "/x".into(),
            vars(&[
                ("HOME", "/h"),
                ("CZ_CASE_DIR", "/c"),
                ("LC_ALL", "en_US.UTF-8"),
                ("PATH", "/bin"),
            ]),
            "/w".into(),
            &mut out,
            &mut err,
        );
        let done = ctx.exec(&Call::new(["tool"]).with_env("EXTRA", "1"));
        assert!(done.ok());
        let entries = log.entries();
        let mx_ports::Effect::Proc(mx_ports::ProcEffect::Run(spec)) = &entries[0] else {
            panic!("{entries:?}")
        };
        let names: Vec<&str> = spec.env.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["CZ_CASE_DIR", "HOME", "EXTRA"]);
        assert_eq!(spec.locale.as_deref(), Some("en_US.UTF-8"));
    }

    #[test]
    fn a_program_that_is_not_found_reads_as_127_with_the_shells_message() {
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        proc.script(
            &["git"],
            Script::always(Err(ProcError::NotFound { program: "git".to_string() })),
        );
        let host = FakeHost::new();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let ctx = Ctx::new(
            &proc,
            &host,
            "scripts/x.sh".into(),
            "/x".into(),
            vars(&[]),
            "/w".into(),
            &mut out,
            &mut err,
        );
        let done = ctx.exec(&Call::new(["git"]).at(12));
        assert_eq!((done.rc, done.started), (127, false));
        assert_eq!(done.stderr, b"scripts/x.sh: line 12: git: command not found\n");
    }

    #[test]
    fn a_timeout_reads_as_124() {
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        proc.script(
            &["slow"],
            Script::always(Ok(mx_ports::ProcOutput {
                exit: ProcExit::TimedOut,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })),
        );
        let host = FakeHost::new();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let ctx = Ctx::new(
            &proc,
            &host,
            "x".into(),
            "/x".into(),
            vars(&[]),
            "/w".into(),
            &mut out,
            &mut err,
        );
        assert_eq!(ctx.exec(&Call::new(["slow"])).rc, 124);
    }

    #[test]
    fn merged_is_stdout_then_stderr_and_a_substitution_strips_it() {
        let done =
            Done { rc: 0, stdout: b"out\n".to_vec(), stderr: b"err\n\n".to_vec(), started: true };
        assert_eq!(done.merged(), b"out\nerr\n\n");
        assert_eq!(shell::command_substitution(&done.merged()), b"out\nerr");
    }

    #[test]
    fn a_substitution_that_holds_a_nul_byte_warns_once_with_the_scripts_line() {
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        let host = FakeHost::new();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut ctx = Ctx::new(
            &proc,
            &host,
            "scripts/x.sh".into(),
            "/x".into(),
            vars(&[]),
            "/w".into(),
            &mut out,
            &mut err,
        );
        assert_eq!(ctx.substitution(b"a\0b\0c\n", 65), b"abc");
        assert_eq!(ctx.substitution(b"plain\n", 66), b"plain");
        drop(ctx);
        assert_eq!(
            err,
            b"scripts/x.sh: line 65: warning: command substitution: ignored null byte in input\n"
        );
    }

    #[test]
    fn the_tree_is_the_logical_parent_of_the_scripts_directory() {
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        let host = FakeHost::new();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let ctx = Ctx::new(
            &proc,
            &host,
            "/t/scripts/check_claims.sh".into(),
            "/x".into(),
            vars(&[]),
            "/w".into(),
            &mut out,
            &mut err,
        );
        assert_eq!(ctx.tree_root(), Path::new("/t"));
        let rel = Ctx::new(
            &proc,
            &host,
            "scripts/check_claims.sh".into(),
            "/x".into(),
            vars(&[]),
            "/w".into(),
            &mut out,
            &mut err,
        );
        assert_eq!(rel.tree_root(), Path::new("/w"));
    }

    #[test]
    fn tmpdir_is_tmpdir_unless_unset_or_empty() {
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        let host = FakeHost::new();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let set = Ctx::new(
            &proc,
            &host,
            "x".into(),
            "/x".into(),
            vars(&[("TMPDIR", "/scratch")]),
            "/w".into(),
            &mut out,
            &mut err,
        );
        assert_eq!(set.tmpdir(), Path::new("/scratch"));
        let empty = Ctx::new(
            &proc,
            &host,
            "x".into(),
            "/x".into(),
            vars(&[("TMPDIR", "")]),
            "/w".into(),
            &mut out,
            &mut err,
        );
        assert_eq!(empty.tmpdir(), Path::new("/tmp"));
    }
}
