//! `maintainer-doctor` and `maintainer-doctor --quick`.
//!
//! The process adapter passes children only the variables named in `ALLOW` and in
//! `MAINTAINER_CHILD_ENV`, a space-separated list of extra names an operator or a test
//! harness chooses. The token the doctor looks for never reaches a child.
//!
//! A write to a closed pipe ends the program with status 141, which is what a
//! shell reports for a process killed by `SIGPIPE`. Restoring the default signal
//! action needs `unsafe`, which this workspace forbids (decision P1I-9).

use mx_doctor::ctx::Stream;
use mx_doctor::host::RealHost;
use mx_doctor::{NotingProc, RunInput, SystemClock, run_live};
use mx_ports::{Proc, RealProc};
use std::io::Write;
use std::process::ExitCode;

/// Variables the tools the doctor starts may see, besides `PATH` and `LC_ALL`.
/// `GIT_DIR`, `GIT_WORK_TREE` and `GIT_INDEX_FILE` are left out on purpose: a git
/// hook exports them, and a child git would write to the hook's repository.
const ALLOW: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "TZ",
    "LANG",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "DBUS_SESSION_BUS_ADDRESS",
    "SSH_AUTH_SOCK",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GH_HOST",
    "GH_CONFIG_DIR",
    "GIT_CONFIG_NOSYSTEM",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_TERMINAL_PROMPT",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_ASKPASS",
    "GIT_EXEC_PATH",
    "CONTAINERS_CONF",
    "CONTAINERS_STORAGE_CONF",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
];

/// What bash searches when `PATH` is unset.
const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

fn main() -> ExitCode {
    let vars: Vec<(String, String)> = std::env::vars().collect();
    let lookup = |name: &str| vars.iter().find(|(key, _)| key == name).map(|(_, v)| v.clone());
    let search =
        lookup("PATH").filter(|path| !path.is_empty()).unwrap_or_else(|| DEFAULT_PATH.to_string());
    let mut allowed: Vec<String> = ALLOW.iter().map(|name| (*name).to_string()).collect();
    if let Some(extra) = lookup("MAINTAINER_CHILD_ENV") {
        allowed.extend(extra.split_whitespace().map(str::to_string));
    }
    let base_env: Vec<(String, String)> = vars
        .iter()
        .filter(|(name, _)| allowed.iter().any(|allow| allow == name))
        .cloned()
        .collect();
    let search_dirs: Vec<std::path::PathBuf> = std::env::split_paths(&search).collect();
    let proc = RealProc::new(search, base_env);
    // The tree is found from argv[0], as the bash tool finds it from $0, so a
    // wrapper that execs this binary under its own name still points at its tree.
    let tree = std::env::args_os()
        .next()
        .map(std::path::PathBuf::from)
        .and_then(|arg0| resolve_arg0(&arg0, &search_dirs))
        .and_then(|exe| {
            exe.parent().and_then(std::path::Path::parent).map(std::path::Path::to_path_buf)
        })
        .unwrap_or_default();
    let missing_log = lookup("MAINTAINER_DOCTOR_MISSING_LOG");
    let input = RunInput { args: std::env::args().skip(1).collect(), vars, tree };
    let noting;
    let proc_ref: &dyn Proc = match missing_log {
        Some(log) if !log.is_empty() => {
            noting = NotingProc::new(&proc, log);
            &noting
        }
        _ => &proc,
    };
    // Every piece of output is written and flushed as it is made, so a capture of
    // both streams interleaves as the bash does and a killed run keeps what it had
    // printed. A closed standard output ends the run with 141.
    let mut emit = |stream: Stream, bytes: &[u8]| match stream {
        Stream::Stdout => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(bytes).is_ok() && stdout.flush().is_ok()
        }
        Stream::Stderr => {
            let mut stderr = std::io::stderr().lock();
            drop(stderr.write_all(bytes));
            true
        }
    };
    let code = run_live(proc_ref, &SystemClock, &RealHost::new(), &input, &mut emit);
    ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1))
}

/// `readlink -f` of the program as it was invoked: a path with a slash is taken
/// as is, a bare name is looked up on the search path.
fn resolve_arg0(
    arg0: &std::path::Path,
    search: &[std::path::PathBuf],
) -> Option<std::path::PathBuf> {
    if arg0.components().count() > 1 {
        return std::fs::canonicalize(arg0).ok();
    }
    search.iter().map(|dir| dir.join(arg0)).find_map(|path| std::fs::canonicalize(path).ok())
}
