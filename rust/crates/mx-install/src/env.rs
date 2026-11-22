//! What a program reads from its surroundings, collected once at the start.
//!
//! The library reads no environment of its own: `main` builds an [`Env`] and everything
//! below asks it. Children get an allowlist of variables and no others (decision P4A-env):
//! `GIT_DIR` and its kin are left out because a git hook exports them and a child `git`
//! would then work on the hook's repository.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

/// Variables a tool the installer starts may see, besides `PATH` and `LC_ALL`.
pub const CHILD_ALLOW: &[&str] = &[
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
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "GIT_CONFIG_NOSYSTEM",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_TERMINAL_PROMPT",
    "GIT_EXEC_PATH",
    // A program that starts another Rust program passes the operator's list on.
    "MAINTAINER_CHILD_ENV",
];

/// What bash searches when `PATH` is unset.
pub const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// The command line, the working directory and the variables of one run.
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// The name the program was started by: the path of the script it stands for.
    pub argv0: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub uid: u32,
    vars: BTreeMap<OsString, OsString>,
}

impl Env {
    /// An environment from pairs, for a test.
    #[must_use]
    pub fn new(argv0: &str, args: &[&str], cwd: &str, vars: &[(&str, &str)]) -> Self {
        Self {
            argv0: argv0.into(),
            args: args.iter().map(OsString::from).collect(),
            cwd: cwd.into(),
            uid: 1000,
            vars: vars.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect(),
        }
    }

    /// The real one.
    #[must_use]
    pub fn from_process() -> Self {
        let mut args = std::env::args_os();
        let argv0 = args.next().unwrap_or_default();
        Self {
            argv0,
            args: args.collect(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            uid: rustix::process::getuid().as_raw(),
            vars: std::env::vars_os().collect(),
        }
    }

    /// `$NAME`, when set (even to the empty string).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&OsStr> {
        self.vars.get(OsStr::new(name)).map(OsString::as_os_str)
    }

    /// `${NAME:-}` read as "is it this text".
    #[must_use]
    pub fn is(&self, name: &str, value: &str) -> bool {
        self.get(name) == Some(OsStr::new(value))
    }

    /// The search path: `PATH`, or bash's default when it is unset.
    #[must_use]
    pub fn path(&self) -> OsString {
        self.get("PATH").map_or_else(|| OsString::from(DEFAULT_PATH), OsStr::to_os_string)
    }

    /// The directories of the search path that a program can be found in: relative and
    /// empty entries are skipped, as the process adapter skips them.
    #[must_use]
    pub fn path_dirs(&self) -> Vec<PathBuf> {
        std::env::split_paths(&self.path()).filter(|dir| dir.is_absolute()).collect()
    }

    /// The variables a child sees: the allowlist, and the names `MAINTAINER_CHILD_ENV`
    /// adds (an operator or a test harness chooses them). A value that is not UTF-8 is
    /// left out.
    #[must_use]
    pub fn child_vars(&self) -> Vec<(String, String)> {
        let mut allowed: Vec<String> = CHILD_ALLOW.iter().map(|n| (*n).to_string()).collect();
        if let Some(extra) = self.get("MAINTAINER_CHILD_ENV").and_then(OsStr::to_str) {
            allowed.extend(extra.split_whitespace().map(str::to_string));
        }
        let mut out = Vec::new();
        for name in allowed {
            if name == "PATH" || name == "LC_ALL" {
                continue;
            }
            if let Some(value) = self.get(&name).and_then(OsStr::to_str) {
                out.push((name, value.to_string()));
            }
        }
        out
    }

    /// The `MAINTAINER_*` variables: a profile file reads them (`${MAINTAINER_STATE_DIR:-...}`).
    #[must_use]
    pub fn maintainer_vars(&self) -> Vec<(String, String)> {
        self.vars
            .iter()
            .filter(|(name, _)| name.as_bytes().starts_with(b"MAINTAINER_"))
            .filter_map(|(name, value)| {
                Some((name.to_str()?.to_string(), value.to_str()?.to_string()))
            })
            .collect()
    }

    /// `$(cd "$(dirname "$0")" && pwd)`: the directory of the script this program stands
    /// for, as a logical path (links are kept, `.` and `..` are resolved by name).
    #[must_use]
    pub fn script_dir(&self) -> PathBuf {
        let parent = Path::new(&self.argv0).parent().unwrap_or_else(|| Path::new("."));
        let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
        logical(parent, &self.cwd)
    }
}

/// `cd DIR && pwd`: an absolute path, `.` and `..` resolved by name.
#[must_use]
pub fn logical(dir: &Path, cwd: &Path) -> PathBuf {
    let joined = if dir.is_absolute() { dir.to_path_buf() } else { cwd.join(dir) };
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() { PathBuf::from("/") } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_directory_is_logical_and_relative_names_join_the_working_directory() {
        let env = Env::new("./sub/../install.sh", &[], "/w/x", &[]);
        assert_eq!(env.script_dir(), PathBuf::from("/w/x"));
        let env = Env::new("install.sh", &[], "/w/x", &[]);
        assert_eq!(env.script_dir(), PathBuf::from("/w/x"));
        let env = Env::new("/t/link/install.sh", &[], "/w", &[]);
        assert_eq!(env.script_dir(), PathBuf::from("/t/link"));
    }

    #[test]
    fn a_child_gets_the_allowlist_and_the_names_the_operator_adds_and_nothing_else() {
        let env = Env::new(
            "x",
            &[],
            "/",
            &[
                ("HOME", "/h"),
                ("GH_TOKEN", "secret"),
                ("GIT_DIR", "/hook/.git"),
                ("CZ_CASE_DIR", "/case"),
                ("MAINTAINER_CHILD_ENV", "CZ_CASE_DIR"),
                ("PATH", "/bin"),
            ],
        );
        let names: Vec<String> = env.child_vars().into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            vec!["HOME".to_string(), "MAINTAINER_CHILD_ENV".to_string(), "CZ_CASE_DIR".to_string()]
        );
    }

    #[test]
    fn an_unset_path_is_the_shells_default_and_relative_entries_are_not_searched() {
        assert_eq!(Env::new("x", &[], "/", &[]).path(), OsString::from(DEFAULT_PATH));
        let env = Env::new("x", &[], "/", &[("PATH", ".:rel:/usr/bin::/bin")]);
        assert_eq!(env.path_dirs(), vec![PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
    }
}
