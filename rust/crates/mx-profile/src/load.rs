//! Profile resolution: `maintainer_load_profile`.
//!
//! The bash function sources `lib/defaults.env`, then a profile's `profile.env`,
//! then applies `MAINTAINER_<KEY>` from the environment, and leaves the result
//! in the calling shell. Here the files are evaluated by a child bash (see
//! [`crate::shell`]) and the function's own logic runs in Rust:
//!
//! 1. Find `defaults.env`: beside the library, else `~/.local/share/maintainer`.
//!    Source it. If its last command fails the function returns 1, silently.
//! 2. If `MAINTAINER_SLUG`, `MAINTAINER_REPO`, `MAINTAINER_ACCOUNT` and
//!    `MAINTAINER_STATE` are all non-empty, a run exported them. Source
//!    `$PROFILE_DIR/profile.env` when that file exists, apply the overrides and
//!    return. The five names are not exported in this branch.
//! 3. Otherwise pick the profile: `MAINTAINER_PROFILE` when set, else the only
//!    deployed one. Several deployed profiles and none are refusals. A named
//!    profile without a `profile.env` is a refusal.
//! 4. Source the profile, apply the overrides, then give `MAINTAINER_SLUG`,
//!    `MAINTAINER_REPO`, `MAINTAINER_STATE`, `MAINTAINER_ACCOUNT` and
//!    `MAINTAINER_POST` their defaults from `REPO_SLUG`, `REPO_PATH`,
//!    `STATE_DIR`, `GH_ACCOUNT` and `POST`, and export them.
//!
//! Every refusal is a message on standard error and return code 1.
//!
//! Quirks kept, each pinned by a test that names it:
//!
//! - The deployed profiles are the directories `~/.local/share/maintainer/profiles/*/`
//!   that hold a `profile.env`. A glob skips names that start with a dot, and
//!   sorts by the whole path, so `a-b` comes before `a`.
//! - `MAINTAINER_PROFILE` is joined into a path unchecked, so `../x` leaves the
//!   share directory. Decision P1F-8 (the same class as D18).
//! - An override applies only to a key that `defaults.env` declares at the start
//!   of a line, and only when the variable is non-empty.
//! - The caller's `set -e` and `set -u` are not modelled. The profile is sourced
//!   under the child's default options. Decision P1F-9.
//! - The profile sees only the variables the caller's allowlist passes (decision
//!   P1F-7), and the sourcing is bounded by a timeout where the bash is not
//!   (decision P1F-6).
//! - The bash runs `sed` and `printf` after the profile, so a profile that sets
//!   `PATH` to nothing, or defines a function called `printf`, silently stops the
//!   `MAINTAINER_<KEY>` step. Here the overrides run in Rust and nothing a
//!   profile does to its shell can stop them. Decision P1F-12.

use crate::shell::evaluate;
use mx_core::Refusal;
use mx_ports::Proc;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

/// How long a child bash may run. The bash function has no limit.
pub const DEFAULT_EVAL_TIMEOUT: Duration = Duration::from_mins(1);

/// The names `maintainer_load_profile` derives and exports, with the profile
/// key each one defaults from.
pub const EXPORTED_FROM: [(&str, &str); 5] = [
    ("MAINTAINER_SLUG", "REPO_SLUG"),
    ("MAINTAINER_REPO", "REPO_PATH"),
    ("MAINTAINER_STATE", "STATE_DIR"),
    ("MAINTAINER_ACCOUNT", "GH_ACCOUNT"),
    ("MAINTAINER_POST", "POST"),
];

/// The part of the caller's environment the loader uses. A library reads no
/// environment, so the caller says. [`LoadEnv::from_vars`] applies the
/// allowlist `HOME`, `PROFILE_DIR` and `MAINTAINER_*`; `PATH` belongs to the
/// `Proc` adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadEnv {
    /// The directory holding `lib/profile.sh` and `defaults.env`.
    pub lib_dir: PathBuf,
    /// `$HOME`. `None` when unset, which the bash expands to the empty string.
    pub home: Option<String>,
    /// `$PROFILE_DIR`, which a run exports.
    pub profile_dir: Option<String>,
    /// Every `MAINTAINER_*` variable of the caller.
    pub maintainer: Vec<(String, String)>,
    /// How long one child bash may run.
    pub eval_timeout: Duration,
}

impl LoadEnv {
    /// Keeps the allowlisted names out of `vars` and drops the rest.
    pub fn from_vars(
        lib_dir: impl Into<PathBuf>,
        vars: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        let mut env = Self {
            lib_dir: lib_dir.into(),
            home: None,
            profile_dir: None,
            maintainer: Vec::new(),
            eval_timeout: DEFAULT_EVAL_TIMEOUT,
        };
        for (name, value) in vars {
            match name.as_str() {
                "HOME" => env.home = Some(value),
                "PROFILE_DIR" => env.profile_dir = Some(value),
                _ if name.starts_with("MAINTAINER_") => env.maintainer.push((name, value)),
                _ => {}
            }
        }
        env
    }
}

/// A successful load: every exported variable, as the files and the function
/// left them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    vars: BTreeMap<String, Vec<u8>>,
    exported: Vec<String>,
    deployed_name: Option<Vec<u8>>,
}

impl Profile {
    /// The value of a variable, or `None` when it is not set.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.vars.get(name).map(Vec::as_slice)
    }

    /// The value as text, when it is UTF-8.
    #[must_use]
    pub fn get_str(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(|value| std::str::from_utf8(value).ok())
    }

    /// Every variable, sorted by name.
    pub fn vars(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.vars.iter().map(|(name, value)| (name.as_str(), value.as_slice()))
    }

    /// The names the function itself exported: the five of
    /// [`EXPORTED_FROM`] after a full resolution, none in the early-return branch.
    #[must_use]
    pub fn exported(&self) -> &[String] {
        &self.exported
    }

    /// The deployed profile that was resolved, or `None` in the early-return
    /// branch, which takes its profile from `PROFILE_DIR`.
    #[must_use]
    pub fn deployed_name(&self) -> Option<&[u8]> {
        self.deployed_name.as_deref()
    }
}

/// Why the load did not give a profile.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LoadError {
    /// No `defaults.env` beside the library or installed.
    #[error("lib/defaults.env not found, so no tunable has a value")]
    DefaultsMissing,
    /// `defaults.env`'s last command failed, so the bash function returns 1 with no message.
    #[error("sourcing defaults.env ended with status {status}")]
    DefaultsFailed { status: i32 },
    /// No profile is deployed and none was named.
    #[error("no profile is deployed")]
    NoProfileDeployed,
    /// Several profiles are deployed and none was named.
    #[error("{} profiles are deployed", names.len())]
    ManyProfiles { names: Vec<Vec<u8>> },
    /// The named profile has no `profile.env` in the share directory.
    #[error("no deployed profile named {}", String::from_utf8_lossy(name))]
    ProfileMissing { name: Vec<u8>, share: PathBuf },
    /// A sourced file ended the shell with `exit`. The bash tool would have
    /// exited with this status.
    #[error("a sourced file ended the shell with status {code}")]
    ShellExited { code: i32 },
    /// The child bash ran past the timeout and was killed.
    #[error("sourcing the profile files did not finish in time")]
    EvalTimedOut,
    /// The child bash could not be started.
    #[error("could not run bash: {0}")]
    Bash(String),
    /// The child's read-back could not be understood.
    #[error("the shell's read-back is malformed: {0}")]
    Malformed(String),
}

impl LoadError {
    /// What the bash function prints to standard error for this refusal. Empty
    /// for the failures the bash does not word (`DefaultsFailed`, a shell
    /// `exit`, and the three that only exist because bash is a child here).
    #[must_use]
    pub fn message(&self) -> Vec<u8> {
        match self {
            Self::DefaultsMissing => {
                b"maintainer: lib/defaults.env not found, so no tunable has a value. Run ./install.sh.\n"
                    .to_vec()
            }
            Self::NoProfileDeployed => [
                b"maintainer: no profile is deployed. Run ./install.sh, or set\n".as_slice(),
                b"            MAINTAINER_PROFILE, MAINTAINER_SLUG and MAINTAINER_REPO.\n",
            ]
            .concat(),
            Self::ManyProfiles { names } => {
                let joined = names.join(&b' ');
                [
                    format!("maintainer: {} profiles are deployed (", names.len()).as_bytes(),
                    joined.as_slice(),
                    b").\n            Set MAINTAINER_PROFILE to say which one you mean.\n",
                ]
                .concat()
            }
            Self::ProfileMissing { name, share } => [
                b"maintainer: no deployed profile named ".as_slice(),
                name,
                b" (looked in ",
                share.as_os_str().as_bytes(),
                b")\n",
            ]
            .concat(),
            Self::DefaultsFailed { .. }
            | Self::ShellExited { .. }
            | Self::EvalTimedOut
            | Self::Bash(_)
            | Self::Malformed(_) => Vec::new(),
        }
    }

    /// The status a tool built on the bash function ends with: 1 for the
    /// function's own refusals, and the `exit` status of a file that ended the
    /// shell. A timeout is 124, as `timeout` reports it.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::ShellExited { code } => *code,
            Self::EvalTimedOut => 124,
            Self::DefaultsMissing
            | Self::DefaultsFailed { .. }
            | Self::NoProfileDeployed
            | Self::ManyProfiles { .. }
            | Self::ProfileMissing { .. }
            | Self::Bash(_)
            | Self::Malformed(_) => 1,
        }
    }

    /// The refusal a program built on this crate reports. Every bash tool turns
    /// the function's return 1 into exit 64 with `|| exit 64`.
    #[must_use]
    pub fn into_refusal(self) -> Refusal {
        match self {
            Self::ProfileMissing { name, .. } => {
                Refusal::ProfileNotFound { profile: String::from_utf8_lossy(&name).into_owned() }
            }
            other => Refusal::ProfileLoadFailed { reason: other.to_string() },
        }
    }
}

/// One call of the loader: what the sourced files printed, and the result.
#[derive(Debug)]
pub struct Load {
    /// What the files wrote to standard output. The bash tool shows it.
    pub stdout: Vec<u8>,
    /// What the files wrote to standard error, before any refusal text.
    pub stderr: Vec<u8>,
    /// The profile, or why there is none.
    pub result: Result<Profile, LoadError>,
}

/// `maintainer_load_profile`.
#[must_use]
pub fn load_profile(proc: &dyn Proc, env: &LoadEnv) -> Load {
    let mut shown = Streams::default();
    let result = resolve(proc, env, &mut shown);
    Load { stdout: shown.stdout, stderr: shown.stderr, result }
}

/// What the sourced files printed, in the order the bash would have printed it.
#[derive(Default)]
struct Streams {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Streams {
    fn keep(&mut self, evaluation: &crate::shell::Evaluation) {
        self.stdout.extend_from_slice(&evaluation.stdout);
        self.stderr.extend_from_slice(&evaluation.stderr);
    }
}

fn resolve(proc: &dyn Proc, env: &LoadEnv, shown: &mut Streams) -> Result<Profile, LoadError> {
    // maintainer_load_defaults. This first run only decides which branch to take:
    // the branch reads variables the defaults may set, and the profile has to see
    // the defaults, so the files are sourced again together below. What this run
    // printed is shown only when no second run follows.
    let defaults = defaults_path(env).ok_or(LoadError::DefaultsMissing)?;
    let first = evaluate(proc, env, std::slice::from_ref(&defaults));
    let after_defaults = match &first.outcome {
        Ok(dump) if dump.last_status == 0 => dump.vars.clone(),
        // `maintainer_load_defaults` ends with `. "$f"`, so it returns that
        // file's last status, and the load ends there without a word.
        Ok(dump) => {
            shown.keep(&first);
            return Err(LoadError::DefaultsFailed { status: dump.last_status });
        }
        Err(error) => {
            shown.keep(&first);
            return Err(error.clone());
        }
    };

    // The run exported the names, and they win.
    let early = ["MAINTAINER_SLUG", "MAINTAINER_REPO", "MAINTAINER_ACCOUNT", "MAINTAINER_STATE"]
        .iter()
        .all(|name| non_empty(&after_defaults, name).is_some());
    if early {
        let profile_dir = non_empty(&after_defaults, "PROFILE_DIR").unwrap_or(b"/nonexistent");
        let file = bytes_path(&[profile_dir, b"/profile.env"]);
        let mut vars = if is_file(&file) {
            source(proc, env, &[defaults.clone(), file], shown)?
        } else {
            shown.keep(&first);
            after_defaults
        };
        apply_overrides(&mut vars, &defaults);
        return Ok(Profile { vars, exported: Vec::new(), deployed_name: None });
    }

    // Which profile: the one named, else the only one deployed.
    let home = after_defaults.get("HOME").map_or(&[][..], Vec::as_slice);
    let share = bytes_path(&[home, b"/.local/share/maintainer/profiles"]);
    let name: Vec<u8> = if let Some(named) = non_empty(&after_defaults, "MAINTAINER_PROFILE") {
        named.to_vec()
    } else {
        let mut found = deployed_profiles(&share);
        match found.len() {
            0 => {
                shown.keep(&first);
                return Err(LoadError::NoProfileDeployed);
            }
            1 => found.remove(0),
            _ => {
                shown.keep(&first);
                return Err(LoadError::ManyProfiles { names: found });
            }
        }
    };
    let env_file = bytes_path(&[share.as_os_str().as_bytes(), b"/", &name, b"/profile.env"]);
    if !is_file(&env_file) {
        shown.keep(&first);
        return Err(LoadError::ProfileMissing { name, share });
    }
    let mut vars = source(proc, env, &[defaults.clone(), env_file], shown)?;
    apply_overrides(&mut vars, &defaults);
    let mut exported = Vec::new();
    for (name, from) in EXPORTED_FROM {
        let value = non_empty(&vars, name)
            .or_else(|| vars.get(from).map(Vec::as_slice))
            .unwrap_or_default()
            .to_vec();
        vars.insert(name.to_string(), value);
        exported.push(name.to_string());
    }
    Ok(Profile { vars, exported, deployed_name: Some(name) })
}

/// Sources `files` in one child and returns the variables it left. What the
/// files printed goes to `shown`.
fn source(
    proc: &dyn Proc,
    env: &LoadEnv,
    files: &[PathBuf],
    shown: &mut Streams,
) -> Result<BTreeMap<String, Vec<u8>>, LoadError> {
    let evaluation = evaluate(proc, env, files);
    shown.keep(&evaluation);
    evaluation.outcome.map(|dump| dump.vars)
}

/// `maintainer_defaults_path`.
fn defaults_path(env: &LoadEnv) -> Option<PathBuf> {
    let home = env.home.as_deref().unwrap_or("");
    [
        bytes_path(&[env.lib_dir.as_os_str().as_bytes(), b"/defaults.env"]),
        bytes_path(&[home.as_bytes(), b"/.local/share/maintainer/defaults.env"]),
    ]
    .into_iter()
    .find(|candidate| is_file(candidate))
}

/// The names of `"$share"/*/profile.env` that exist, as the glob lists them.
///
/// `*` does not match a name that starts with a dot. The glob sorts the whole
/// path, so the key here is `name/profile.env` byte for byte.
fn deployed_profiles(share: &Path) -> Vec<Vec<u8>> {
    let Ok(listing) = std::fs::read_dir(share) else { return Vec::new() };
    let mut names: Vec<Vec<u8>> = listing
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().as_bytes().to_vec())
        .filter(|name| !name.starts_with(b"."))
        .filter(|name| {
            std::fs::metadata(bytes_path(&[
                share.as_os_str().as_bytes(),
                b"/",
                name,
                b"/profile.env",
            ]))
            .is_ok()
        })
        .collect();
    names.sort_by_key(|name| [name.as_slice(), b"/profile.env"].concat());
    names
}

/// `maintainer_apply_overrides`: for every key `defaults.env` declares at the
/// start of a line, `MAINTAINER_<KEY>` replaces the value when it is non-empty.
fn apply_overrides(vars: &mut BTreeMap<String, Vec<u8>>, defaults: &Path) {
    let Ok(text) = std::fs::read(defaults) else { return };
    for line in text.split(|byte| *byte == b'\n') {
        let Some(key) = declared_key(line) else { continue };
        let override_name = format!("MAINTAINER_{key}");
        if let Some(value) = non_empty(vars, &override_name).map(<[u8]>::to_vec) {
            vars.insert(key.to_string(), value);
        }
    }
}

/// The `KEY` of a line `sed -n 's/^\([A-Z][A-Z0-9_]*\)=.*/\1/p'` prints.
fn declared_key(line: &[u8]) -> Option<&str> {
    let end = line
        .iter()
        .position(|byte| !(byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_'))?;
    if end == 0 || !line[0].is_ascii_uppercase() || line[end] != b'=' {
        return None;
    }
    std::str::from_utf8(&line[..end]).ok()
}

/// `[ -n "${NAME:-}" ]` and the value it guards.
fn non_empty<'a>(vars: &'a BTreeMap<String, Vec<u8>>, name: &str) -> Option<&'a [u8]> {
    vars.get(name).map(Vec::as_slice).filter(|value| !value.is_empty())
}

/// `[ -f path ]`.
fn is_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

/// A path built from parts by plain concatenation, as the bash builds strings.
fn bytes_path(parts: &[&[u8]]) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(&parts.concat()))
}
