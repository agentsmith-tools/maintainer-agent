//! What every command needs: the caller's environment, the process port, the
//! two output streams, and the way to run a child the way the bash runs one.

use mx_core::Fact;
use mx_ports::{Proc, ProcError, ProcExit, ProcSpec};
use mx_profile::classify::effective_locale;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// bash's compiled-in default when `PATH` is unset.
pub const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// How long one child may run. The bash has no limit; the `Proc` port needs
/// one, so a call that has not finished in an hour is killed (decision P1H-5).
pub const CHILD_TIMEOUT: Duration = Duration::from_hours(1);

/// A command ended the program: its exit code. The message, if any, is already
/// on standard error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stop(pub i32);

/// The result of a step that can end the program.
pub type R<T> = Result<T, Stop>;

/// The environment names a child may see besides the prefixes below (decision
/// P1H-4). The bash passes everything; a child here gets what `gh`, `git`,
/// `python3` and the tools need to behave as they do under the caller.
const CHILD_ENV_NAMES: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "TZ",
    "LANG",
    "LANGUAGE",
    "TERM",
    "SSH_AUTH_SOCK",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "PROFILE_DIR",
];
const CHILD_ENV_PREFIXES: &[&str] =
    &["MAINTAINER_", "GH_", "GITHUB_", "GIT_", "XDG_", "LC_", "PYTHON"];

/// Credentials the caller may hold. Only the programs in [`TOKEN_CHILDREN`] get them
/// (decision P1H-17): `gh` and `git` use them, and `python3` runs `maintainer
/// fixup-check`, which starts `gh`. `mktemp`, `sort`, `sleep`, `env`, `timeout` and
/// `bash -c` never need one, and the bash handed them every variable it had.
const TOKEN_NAMES: &[&str] =
    &["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"];
const TOKEN_CHILDREN: &[&str] = &["gh", "git", "python3"];

/// The variables of `vars` a child receives. `MAINTAINER_CHILD_ENV` may name
/// more, separated by spaces: the black-box harness uses it to pass the names
/// its stubs read. `LC_ALL` is left out because the `Proc` port owns it.
#[must_use]
pub fn child_env(vars: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let extra: Vec<&str> = vars
        .get("MAINTAINER_CHILD_ENV")
        .map(|names| names.split(' ').collect())
        .unwrap_or_default();
    vars.iter()
        .filter(|(name, _)| name.as_str() != "LC_ALL" && name.as_str() != "PATH")
        .filter(|(name, _)| {
            CHILD_ENV_NAMES.contains(&name.as_str())
                || CHILD_ENV_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
                || extra.contains(&name.as_str())
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// `line N: ` when the line is known.
fn line_text(line: Option<u32>) -> String {
    line.map_or_else(String::new, |n| format!("line {n}: "))
}

/// What a child left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    /// The exit code, as the shell would report it: 127 when the program was
    /// not found, 124 when it ran past the timeout.
    pub rc: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// False when the program could not be started at all.
    pub started: bool,
}

impl Done {
    /// `if cmd; then`.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.rc == 0
    }

    /// The stdout as a read that may fail: known when the child ran, even when
    /// it exited non-zero (the bash uses what a failed command printed), and
    /// unreadable when it could not be started.
    pub fn stdout_fact(&self) -> Fact<Vec<u8>> {
        if self.started {
            Fact::Known(self.stdout.clone())
        } else {
            Fact::Unreadable(String::from_utf8_lossy(&self.stderr).trim_end().to_string())
        }
    }
}

/// One child to run.
#[derive(Debug, Clone)]
pub struct Call {
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Duration,
    /// The line of `bin/maintainer-repo` that runs this call, for the
    /// `line N:` the bash puts in its `command not found` message.
    pub line: Option<u32>,
}

impl Call {
    /// A call with the default timeout.
    #[must_use]
    pub fn new<S: Into<String>>(argv: impl IntoIterator<Item = S>) -> Self {
        Self {
            argv: argv.into_iter().map(Into::into).collect(),
            cwd: None,
            stdin: None,
            timeout: CHILD_TIMEOUT,
            line: None,
        }
    }

    /// Names the line of the bash program this call stands for.
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

    /// Feeds `bytes` on standard input.
    #[must_use]
    pub fn stdin(mut self, bytes: Vec<u8>) -> Self {
        self.stdin = Some(bytes);
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
    /// `$0` as the program was started, for the messages bash puts in front of
    /// its own diagnostics.
    pub argv0: OsString,
    /// The running executable, used to find the tree when `argv0` has no slash.
    pub exe: PathBuf,
    /// The caller's whole environment (UTF-8 entries).
    pub vars: BTreeMap<String, String>,
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
    child_env: Vec<(String, String)>,
    locale: Option<String>,
    /// Variables the program exported after it started (`export GIT_EDITOR=true`).
    pub exports: Vec<(String, String)>,
    /// Where children run: the repository, after the bash's `cd`.
    pub cwd: Option<PathBuf>,
}

impl<'a> Ctx<'a> {
    /// A context over `proc`, writing to `out` and `err`.
    pub fn new(
        proc: &'a dyn Proc,
        argv0: OsString,
        exe: PathBuf,
        vars: BTreeMap<String, String>,
        out: &'a mut dyn Write,
        err: &'a mut dyn Write,
    ) -> Self {
        let child_env = child_env(&vars);
        let locale = effective_locale(
            vars.get("LC_ALL").map(String::as_str),
            vars.get("LC_CTYPE").map(String::as_str),
            vars.get("LANG").map(String::as_str),
        );
        Self { proc, argv0, exe, vars, out, err, child_env, locale, exports: Vec::new(), cwd: None }
    }

    /// The process port, for the library calls that take one.
    #[must_use]
    pub fn proc(&self) -> &dyn Proc {
        self.proc
    }

    /// Writes bytes to standard output now, so output and error keep their order.
    pub fn put_out(&mut self, bytes: &[u8]) {
        // A closed pipe ends the real program in `main`'s writer; a failure
        // here has nowhere better to go.
        drop(self.out.write_all(bytes));
        drop(self.out.flush());
    }

    /// Writes bytes to standard error now.
    pub fn put_err(&mut self, bytes: &[u8]) {
        drop(self.err.write_all(bytes));
        drop(self.err.flush());
    }

    /// `say`: two spaces, the text, a newline.
    pub fn say(&mut self, text: &str) {
        self.put_out(format!("  {text}\n").as_bytes());
    }

    /// `die`: `maintainer-repo: MESSAGE` on standard error, and the program ends with 1.
    pub fn die(&mut self, message: &str) -> Stop {
        self.put_err(format!("maintainer-repo: {message}\n").as_bytes());
        Stop(1)
    }

    /// The `$0` text bash prints before its own diagnostics.
    #[must_use]
    pub fn script_name(&self) -> String {
        self.argv0.to_string_lossy().into_owned()
    }

    /// Runs a child. The environment is the allowlist, then the program's own
    /// exports, then the `Proc` port's `PATH` and `LC_ALL`.
    #[must_use]
    pub fn exec(&self, call: &Call) -> Done {
        let Some(program) = call.argv.first() else {
            return Done { rc: 127, stdout: Vec::new(), stderr: Vec::new(), started: false };
        };
        let mut spec = ProcSpec::command(program, call.argv[1..].iter().cloned(), call.timeout);
        let may_hold_tokens = TOKEN_CHILDREN.contains(&program.as_str());
        for (name, value) in self.child_env.iter().chain(&self.exports) {
            if !may_hold_tokens && TOKEN_NAMES.contains(&name.as_str()) {
                continue;
            }
            spec = spec.env(name.as_str(), value.as_str());
        }
        if let Some(locale) = &self.locale {
            spec = spec.locale(locale.as_str());
        }
        // A child runs where the bash's `cd` left the process: the repository.
        if let Some(dir) = call.cwd.as_ref().or(self.cwd.as_ref()) {
            spec = spec.cwd(dir.as_path());
        }
        if let Some(bytes) = &call.stdin {
            spec = spec.stdin(bytes.clone());
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
                let (rc, text) = match &error {
                    ProcError::NotFound { .. } | ProcError::RelativeProgram { .. } => (
                        127,
                        format!("{name}: {}{program}: command not found\n", line_text(call.line)),
                    ),
                    other => (126, format!("{name}: {}{program}: {other}\n", line_text(call.line))),
                };
                Done { rc, stdout: Vec::new(), stderr: text.into_bytes(), started: false }
            }
        }
    }

    /// Is `name` an executable file in a directory of the `PATH` the children
    /// get? The in-process replacements of `tr`, `sed`, `grep`, `awk`, `paste`,
    /// `wc` and `rm` ask this, so a host that lacks the tool behaves as the bash
    /// does there (decision P1H-16).
    #[must_use]
    pub fn has_tool(&self, name: &str) -> bool {
        use std::os::unix::fs::PermissionsExt;
        let path = self.vars.get("PATH").map_or(DEFAULT_PATH, String::as_str);
        path.split(':').filter(|dir| dir.starts_with('/')).any(|dir| {
            std::fs::metadata(Path::new(dir).join(name))
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
    }

    /// True, after printing the bash's `SCRIPT: line N: TOOL: command not found`
    /// on standard error, when the host has no `tool`. The caller then does what
    /// the bash does with a command that failed with status 127 and printed nothing.
    pub fn missing_tool(&mut self, tool: &str, line: u32) -> bool {
        if self.has_tool(tool) {
            return false;
        }
        let name = self.script_name();
        self.put_err(format!("{name}: line {line}: {tool}: command not found\n").as_bytes());
        true
    }

    /// `missing_tool` for a tool that is only asked when `when` holds.
    pub fn missing_tool_if(&mut self, when: bool, tool: &str, line: u32) -> bool {
        when && self.missing_tool(tool, line)
    }

    /// `git ARGS`, in the repository when there is one.
    #[must_use]
    pub fn git(&self, args: &[&str]) -> Done {
        let words = std::iter::once("git").chain(args.iter().copied());
        self.exec(&Call::new(words).cwd(self.cwd.as_deref()))
    }

    /// Lets a child's standard error through, as the bash does when it does
    /// not redirect it.
    pub fn show_err(&mut self, done: &Done) {
        self.put_err(&done.stderr);
    }

    /// Lets a child's standard output through.
    pub fn show_out(&mut self, done: &Done) {
        self.put_out(&done.stdout);
    }

    /// Both streams of a child that the bash does not capture or redirect:
    /// output first, then error (the two arrive separately here, so their
    /// interleaving inside one call is not kept; decision P1H-6).
    pub fn show_both(&mut self, done: &Done) {
        self.show_out(done);
        self.show_err(done);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
    }

    #[test]
    fn a_child_gets_the_allowlist_and_the_named_prefixes_and_nothing_else() {
        let env = vars(&[
            ("HOME", "/h"),
            ("GH_TOKEN", "t"),
            ("GIT_CONFIG_COUNT", "1"),
            ("MAINTAINER_SLUG", "o/r"),
            ("SECRET_SAUCE", "x"),
            ("AWS_SECRET_ACCESS_KEY", "k"),
            ("LC_ALL", "C"),
            ("PATH", "/bin"),
            ("BASH_ENV", "/x"),
        ]);
        let names: Vec<String> = child_env(&env).into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            ["GH_TOKEN", "GIT_CONFIG_COUNT", "HOME", "MAINTAINER_SLUG"],
            "PATH and LC_ALL belong to the Proc port; other names stay out"
        );
    }

    #[test]
    fn maintainer_child_env_adds_the_names_it_lists() {
        let env = vars(&[
            ("MAINTAINER_CHILD_ENV", "CZ_CASE_DIR BASH_ENV"),
            ("CZ_CASE_DIR", "/c"),
            ("BASH_ENV", "/b"),
            ("OTHER", "o"),
        ]);
        let names: Vec<String> = child_env(&env).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["BASH_ENV", "CZ_CASE_DIR", "MAINTAINER_CHILD_ENV"]);
    }
}
