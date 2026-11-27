//! `mx-verify-deps verify-deps <pr> <sha>`: the dependency-bump receipt, as a program.
//!
//! This is `maintainer-merge verify-deps` and nothing else: the gate binary calls the same
//! [`mx_gate::deps::run`] for its `verify-deps` arm. A separate program exists so the module
//! can be run against the bash gate, case by case, before the rest of the gate is ported.
//! It reads the environment and the command line, builds the real adapters, and holds no logic.

use mx_core::Fact;
use mx_gate::deps::{
    DepsPorts, ProcGh, RealClassifier, RealNet, RealWholeFile, ScriptPaths, Wiring, run,
};
use mx_gate::{
    Command, Exit, Helper, HelperMissing, HoldersFailure, IssueNumber, LeaseAnswer, PrNumber,
    Progress, Say, parse_command,
};
use mx_ports::{Fs, FsError, Proc, ProcError, ProcExit, ProcSpec, RealProc};
use mx_receipt::{Stamp, iso_with_offset};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// The variables a child may see: where `gh` finds its login, the proxy and certificate
/// settings the registry scripts use, and the override that points `PyPI` checks at a test server.
const ALLOWED: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "TZ",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_HOST",
    "GH_CONFIG_DIR",
    "GH_ENTERPRISE_TOKEN",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "MAINTAINER_PYPI_URL",
    "MAINTAINER_STATE",
    "MAINTAINER_SLUG",
    "MAINTAINER_PROFILE",
    "MAINTAINER_ACCOUNT",
    "MAINTAINER_REPO",
];

struct StdFs;

impl Fs for StdFs {
    fn read(&self, path: &Path) -> Fact<Vec<u8>> {
        match std::fs::read(path) {
            Ok(bytes) => Fact::Known(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Fact::Absent,
            Err(error) => Fact::Unreadable(error.to_string()),
        }
    }

    fn mtime(&self, path: &Path) -> Fact<u64> {
        Fact::Unreadable(format!("{}: mtime is not used here", path.display()))
    }

    fn list(&self, path: &Path) -> Fact<Vec<PathBuf>> {
        Fact::Unreadable(format!("{}: list is not used here", path.display()))
    }

    /// `mkdir -p` the directory, then the file: the bash makes `$RECEIPTS` before it writes.
    fn write(&self, path: &Path, contents: &[u8]) -> Result<(), FsError> {
        let fail =
            |error: std::io::Error| FsError { path: path.to_path_buf(), reason: error.to_string() };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        std::fs::write(path, contents).map_err(fail)
    }

    fn remove(&self, path: &Path) -> Result<(), FsError> {
        std::fs::remove_file(path)
            .map_err(|error| FsError { path: path.to_path_buf(), reason: error.to_string() })
    }
}

struct Stdout;

impl Say for Stdout {
    fn say(&self, line: &Progress) {
        let mut out = std::io::stdout().lock();
        // A closed pipe ends the program the way SIGPIPE ends the bash.
        if writeln!(out, "  {line}").and_then(|()| out.flush()).is_err() {
            std::process::exit(141);
        }
    }
}

/// The `maintainer` beside this program, as `mx-checks` and `mx-doctor` find their tree: the
/// directory of `argv[0]` when it has a slash (a wrapper in `bin/` runs this binary under its own
/// path, so `current_exe()` names the `.rust-bin/` copy, which has no helper), then the directory
/// of the running executable, which covers a symlinked or copied install whose `argv[0]` directory
/// has no helper.
fn helper_beside(argv0: &Path) -> Option<PathBuf> {
    // A relative `argv[0]` (`./mx-verify-deps`, `bin/mx-verify-deps`) names a place under the working
    // directory, and `RealProc` runs a program with a slash only by its absolute path.
    let argv_dir = if argv0.components().count() > 1 {
        std::path::absolute(argv0).ok().and_then(|path| path.parent().map(Path::to_path_buf))
    } else {
        None
    };
    let exe_dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
    argv_dir
        .into_iter()
        .chain(exe_dir)
        .map(|dir| dir.join("maintainer"))
        .find(|path| path.is_file())
}

/// `maintainer lease check <pr>`, with the helper found beside this program or on the search path.
struct LeaseHelper<'a> {
    proc: &'a dyn Proc,
    env: &'a BTreeMap<String, String>,
    beside: Option<PathBuf>,
}

impl Helper for LeaseHelper<'_> {
    fn lease_check(&self, pr: &PrNumber) -> Result<LeaseAnswer, HelperMissing> {
        let program = self
            .beside
            .as_ref()
            .map_or_else(|| "maintainer".to_string(), |path| path.display().to_string());
        let var = |name: &str| self.env.get(name).cloned().unwrap_or_default();
        let spec =
            ProcSpec::command(&program, ["lease", "check", pr.as_arg()], Duration::from_mins(1))
                .env("MAINTAINER_STATE", var("MAINTAINER_STATE"))
                .env("MAINTAINER_SLUG", var("MAINTAINER_SLUG"))
                .env("MAINTAINER_PROFILE", var("MAINTAINER_PROFILE"));
        let output = |out: &mx_ports::ProcOutput| {
            let mut bytes = out.stdout.clone();
            bytes.extend_from_slice(&out.stderr);
            String::from_utf8_lossy(&bytes).trim_end_matches('\n').to_string()
        };
        match self.proc.run(&spec) {
            Ok(out) => Ok(match out.exit {
                ProcExit::Code(0) => LeaseAnswer::Free,
                ProcExit::Code(1) => LeaseAnswer::Held { output: output(&out) },
                _ => LeaseAnswer::Unreadable { output: output(&out) },
            }),
            Err(ProcError::NotFound { .. } | ProcError::NotExecutable { .. }) => Err(HelperMissing),
            Err(error) => Ok(LeaseAnswer::Unreadable { output: error.to_string() }),
        }
    }

    fn holders(&self, _issue: IssueNumber) -> Result<Vec<String>, HoldersFailure> {
        Err(HoldersFailure::HelperMissing)
    }
}

/// `recorded_at` as `datetime.now().astimezone().isoformat()` and `recorded_by` as
/// `getpass.getuser()` give them.
fn stamp_now(proc: &dyn Proc, env: &BTreeMap<String, String>) -> Stamp {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let text = |args: &[&str]| {
        let spec = ProcSpec::command(args[0], args[1..].iter().copied(), Duration::from_secs(10));
        proc.run(&spec)
            .ok()
            .filter(|out| out.exit.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let offset = text(&["date", "+%z"])
        .and_then(|zone| {
            let (sign, digits) = zone.split_at_checked(1)?;
            let hours: i32 = digits.get(..2)?.parse().ok()?;
            let minutes: i32 = digits.get(2..4)?.parse().ok()?;
            let seconds = hours * 3600 + minutes * 60;
            Some(if sign == "-" { -seconds } else { seconds })
        })
        .unwrap_or(0);
    let at =
        iso_with_offset(i64::try_from(since.as_secs()).unwrap_or(0), since.subsec_micros(), offset);
    let user = ["LOGNAME", "USER", "LNAME", "USERNAME"]
        .iter()
        .find_map(|name| env.get(*name).filter(|value| !value.is_empty()).cloned())
        .or_else(|| text(&["id", "-un"]))
        .unwrap_or_default();
    Stamp::new(at, user)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    let exit = match parse_command(&args) {
        Ok(Command::VerifyDeps { pr, sha }) => {
            let search = env.get("PATH").cloned().unwrap_or_else(|| DEFAULT_PATH.to_string());
            let proc = RealProc::from_process_env(search, ALLOWED);
            let scripts = ScriptPaths::from_env(|name| env.get(name).cloned());
            let slug = env.get("MAINTAINER_SLUG").cloned().unwrap_or_default();
            let gh = ProcGh::new(&proc, &slug);
            let net =
                RealNet::new(&proc, scripts.image_resolver.clone(), scripts.pypi_verifier.clone());
            let classifier = RealClassifier::new(&proc, scripts.classifier.clone());
            let whole = RealWholeFile::new(&proc, scripts.pip_files.clone());
            let argv0 = PathBuf::from(std::env::args_os().next().unwrap_or_default());
            let helper = LeaseHelper { proc: &proc, env: &env, beside: helper_beside(&argv0) };
            let now = || stamp_now(&proc, &env);
            let wiring = Wiring {
                ports: DepsPorts {
                    gh: &gh,
                    net: &net,
                    classifier: &classifier,
                    whole_file: &whole,
                    fs: &StdFs,
                    say: &Stdout,
                },
                helper: &helper,
                stamp: &now,
            };
            run(&env, &pr, &sha, &wiring)
        }
        Ok(_) => Exit::usage(),
        Err(error) => Exit::from(&error),
    };
    if !exit.stderr.is_empty() {
        let _ = std::io::stderr().write_all(exit.stderr.as_bytes());
    }
    ExitCode::from(u8::try_from(exit.code).unwrap_or(1))
}
