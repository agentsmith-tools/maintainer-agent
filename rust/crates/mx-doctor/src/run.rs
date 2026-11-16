//! The program: choose the profile, load it, run the sections in order, print
//! the summary. No terminal and no environment access: the caller passes the
//! variables and the directory of the tree.

use crate::audit::{audit_trail, deployed_code};
use crate::ctx::{Ctx, Env, Out, Settings, Stop, limit};
use crate::github::{identity, posting, repository};
use crate::host::HostFs;
use crate::hostchecks::{container_verification, sandbox, scheduler};
use crate::install::{backend, commands, deployment, profile_and_containment};
use crate::report::Report;
use mx_ports::{Clock, Proc};
use mx_profile::{LoadEnv, LoadError, load_for_doctor};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// What the caller hands over.
#[derive(Debug, Clone)]
pub struct RunInput {
    /// The arguments after the program name.
    pub args: Vec<String>,
    /// The caller's environment, every variable that is valid UTF-8.
    pub vars: Vec<(String, String)>,
    /// The directory that holds `bin/` and `lib/`: where the bash tool finds
    /// `lib/profile.sh` beside itself.
    pub tree: PathBuf,
}

/// What the program wrote and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub code: i32,
}

/// Variables the child bash that evaluates the profile must not inherit: the
/// search path and locale belong to the process adapter, and the others would
/// run or change a shell the caller did not mean to start.
const NOT_PASSED_TO_PROFILE: [&str; 8] =
    ["PATH", "LC_ALL", "BASH_ENV", "ENV", "SHELLOPTS", "BASHOPTS", "PS4", "PROMPT_COMMAND"];

/// True for a name a shell variable can have.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn refusal(stderr: &str, code: i32) -> Outcome {
    Outcome { stdout: Vec::new(), stderr: stderr.as_bytes().to_vec(), code }
}

/// Runs the doctor.
pub fn run(proc: &dyn Proc, clock: &dyn Clock, host: &dyn HostFs, input: &RunInput) -> Outcome {
    let var = |name: &str| input.vars.iter().find(|(key, _)| key == name).map(|(_, v)| v.as_str());
    let Some(home) = var("HOME") else {
        return refusal("maintainer-doctor: line 12: HOME: unbound variable\n", 127);
    };
    let share = format!("{home}/.local/share/maintainer");
    // One deployed profile needs no naming; several do. Defaulting to a particular
    // repository's name is how a tool ends up reporting on somebody else's project.
    let profile = match choose_profile(host, &share, var("MAINTAINER_PROFILE").unwrap_or("")) {
        Ok(profile) => profile,
        Err(outcome) => return outcome,
    };
    let quick = input.args.first().is_some_and(|arg| arg == "--quick");

    // lib/profile.sh beside the program, else the deployed copy.
    let tree_lib = format!("{}/lib", input.tree.display());
    let lib_dir = if host.stat(&format!("{tree_lib}/profile.sh")).is_file() {
        tree_lib
    } else if host.stat(&format!("{share}/profile.sh")).is_file() {
        share.clone()
    } else {
        return refusal("maintainer-doctor: cannot find profile.sh\n", 64);
    };
    let ambient: Vec<(String, String)> = input
        .vars
        .iter()
        .filter(|(name, _)| {
            name != "HOME" && valid_name(name) && !NOT_PASSED_TO_PROFILE.contains(&name.as_str())
        })
        .cloned()
        .collect();
    let load_env = LoadEnv {
        lib_dir: PathBuf::from(lib_dir),
        home: Some(home.to_string()),
        profile_dir: None,
        maintainer: ambient,
        eval_timeout: limit::SOURCING,
    };
    let loaded = load_for_doctor(
        proc,
        &load_env,
        std::path::Path::new(&format!("{share}/profiles/{profile}/profile.env")),
    );
    let profile_vars = match loaded.result {
        Ok(vars) => vars,
        Err(error) => {
            let mut stderr = loaded.stderr.clone();
            stderr.extend_from_slice(&error.message());
            let code = match &error {
                &LoadError::ShellExited { code } => code,
                LoadError::EvalTimedOut => 124,
                _ => 64,
            };
            return Outcome { stdout: loaded.stdout, stderr, code };
        }
    };
    let mut values = BTreeMap::new();
    let mut suite_env = Vec::new();
    for (name, value) in profile_vars.vars() {
        let text = String::from_utf8_lossy(value).into_owned();
        if !NOT_PASSED_TO_PROFILE.contains(&name) {
            suite_env.push((name.to_string(), text.clone()));
        }
        values.insert(name.to_string(), text);
    }
    let settings = Settings::new(values);
    let env = Env {
        home: home.to_string(),
        user: var("USER").filter(|user| !user.is_empty()).map(str::to_string),
        tmpdir: var("TMPDIR").filter(|dir| !dir.is_empty()).map(str::to_string),
    };
    let ctx = Ctx {
        proc,
        clock,
        host,
        env: &env,
        settings: &settings,
        profile: &profile,
        share,
        quick,
        suite_env: &suite_env,
    };
    let mut out = Out::default();
    let stopped = sections(&ctx, &mut out);
    let mut stdout = loaded.stdout;
    for item in out.report.items() {
        stdout.extend_from_slice(Report::render_item(item).as_bytes());
    }
    let mut stderr = loaded.stderr;
    stderr.extend_from_slice(&out.stderr);
    if let Some(code) = stopped {
        Outcome { stdout, stderr, code }
    } else {
        stdout.extend_from_slice(out.report.render_summary().as_bytes());
        Outcome { stdout, stderr, code: out.report.exit_code() }
    }
}

/// The profile to check: the one named, else the only one deployed.
fn choose_profile(host: &dyn HostFs, share: &str, named: &str) -> Result<String, Outcome> {
    // Validated before it reaches a `python3 -c` string in the bash. An empty value
    // does not match: the unset case falls through to the enumeration below.
    if !named.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) {
        return Err(refusal(
            &format!("maintainer-doctor: '{named}' is not a plain profile name\n"),
            64,
        ));
    }
    if !named.is_empty() {
        return Ok(named.to_string());
    }
    let found = deployed(host, &format!("{share}/profiles"));
    match found.len() {
        1 => Ok(found[0].clone()),
        0 => Ok("(none deployed)".to_string()),
        n => Err(refusal(
            &format!(
                "maintainer-doctor: {n} profiles are deployed ({}).\n                   Set MAINTAINER_PROFILE to say which one to check.\n",
                found.join(" ")
            ),
            64,
        )),
    }
}

/// One check that may end the run or the section.
type Step<'a> = &'a dyn Fn(&Ctx<'_>, &mut Out) -> Result<(), Stop>;

/// Every section in order. `Some(code)` when the shell ended early.
fn sections(ctx: &Ctx<'_>, out: &mut Out) -> Option<i32> {
    deployment(ctx, out);
    let steps: [Step; 11] = [
        &profile_and_containment,
        &|c, o| {
            backend(c, o);
            Ok(())
        },
        &|c, o| {
            commands(c, o);
            Ok(())
        },
        &|c, o| {
            identity(c, o);
            Ok(())
        },
        &|c, o| {
            posting(c, o);
            Ok(())
        },
        &repository,
        &|c, o| {
            scheduler(c, o);
            Ok(())
        },
        &|c, o| {
            container_verification(c, o);
            Ok(())
        },
        &|c, o| {
            sandbox(c, o);
            Ok(())
        },
        &audit_trail,
        &|c, o| {
            deployed_code(c, o);
            Ok(())
        },
    ];
    for step in steps {
        match step(ctx, out) {
            Ok(()) | Err(Stop::AbortSection) => {}
            Err(Stop::Exit(code)) => return Some(code),
        }
    }
    None
}

/// The profiles deployed under `dir`: the directories that hold a `profile.env`,
/// ordered as the shell glob `*/profile.env` orders them (by the whole path).
fn deployed(host: &dyn HostFs, dir: &str) -> Vec<String> {
    let mut names: Vec<String> = match host.list(dir) {
        mx_core::Fact::Known(names) => names,
        _ => Vec::new(),
    }
    .into_iter()
    .filter(|name| !name.starts_with('.'))
    .filter(|name| host.stat(&format!("{dir}/{name}/profile.env")).exists())
    .collect();
    names.sort_by_key(|name| format!("{name}/profile.env"));
    names
}

/// A clock backed by the system time.
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs())
    }

    fn sleep(&self, duration: std::time::Duration) {
        std::thread::sleep(duration);
    }
}
