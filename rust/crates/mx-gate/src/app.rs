//! The `maintainer-merge` program: locate the tree, load the profile through `bash` as the bash
//! program does, build the real adapters and run the gate. `main.rs` only reads the process
//! environment and exits with the status this returns.

use crate::real::{
    FileLocks, PathRuntime, RealGateGit, RealGh, RealHelper, RealIdentity, RealStage, RealStamps,
    Runner, StdDirs, StdFs, StdoutSay, SystemClock,
};
use crate::{
    Account, Command, Exit, Gate, GateConfig, Ports, Posting, StampSource, USAGE, VerifySettings,
    parse_command,
};
use mx_ports::{Proc, RealPipe, RealProc};
use mx_profile::{LoadEnv, load_profile};
use mx_sandbox::{RUNTIME_ENV_NAMES, RealSandbox, pick_env};
use mx_suite::{
    HostSelector, LimitKey, ProdGlobs, RealSuiteHost, Runtime, SUITE_ENV_NAMES, SuiteDirs,
};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The search path when the caller has none.
pub const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

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
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
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
/// Credentials: [`Runner`] hands them to `gh`, `git` and the helper only.
const TOKENS: &[&str] =
    &["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"];
/// Variables that point git at another repository than the one `-C` names (MISTAKES 11).
const GIT_SCRUB: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_COMMON_DIR",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_PREFIX",
    "GIT_NAMESPACE",
];

/// The variables of `vars` a child receives; `MAINTAINER_CHILD_ENV` may name more.
#[must_use]
pub fn child_env(vars: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let extra: Vec<&str> =
        vars.get("MAINTAINER_CHILD_ENV").map(|n| n.split(' ').collect()).unwrap_or_default();
    vars.iter()
        .filter(|(name, _)| !matches!(name.as_str(), "LC_ALL" | "PATH"))
        .filter(|(name, _)| !TOKENS.contains(&name.as_str()) && !GIT_SCRUB.contains(&name.as_str()))
        .filter(|(name, _)| {
            CHILD_ENV_NAMES.contains(&name.as_str())
                || CHILD_ENV_PREFIXES.iter().any(|p| name.starts_with(p))
                || extra.contains(&name.as_str())
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// The variables a suite function can read: what a child receives, plus the ones the shipped
/// suites name ([`SUITE_ENV_NAMES`]), from the profile or the environment as the bash shell
/// that sources a suite sees them. No credential.
fn suite_env(
    vars: &BTreeMap<String, String>,
    pick: &dyn Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let mut env = child_env(vars);
    for name in SUITE_ENV_NAMES {
        if env.iter().all(|(have, _)| have != name)
            && let Some(value) = pick(name)
        {
            env.push((name.to_string(), value));
        }
    }
    env
}

/// Where the program's own files are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    /// The folder of the program, where `maintainer` lives.
    pub bin_dir: PathBuf,
    /// The folder with `profile.sh` and `defaults.env`.
    pub lib_dir: PathBuf,
}

fn locate(argv0: &str, exe: &Path, vars: &BTreeMap<String, String>) -> Result<Tree, Exit> {
    let given = PathBuf::from(argv0);
    let source = if argv0.contains('/') { given } else { exe.to_path_buf() };
    let program = std::fs::canonicalize(&source).unwrap_or(source);
    let bin_dir = program.parent().map(Path::to_path_buf).unwrap_or_default();
    let beside = bin_dir.join("../lib");
    if std::fs::metadata(beside.join("profile.sh")).is_ok_and(|m| m.is_file()) {
        return Ok(Tree { bin_dir, lib_dir: beside });
    }
    let home = vars.get("HOME").cloned().ok_or_else(|| Exit {
        code: 1,
        stderr: format!("{argv0}: line 18: HOME: unbound variable\n"),
    })?;
    let share = PathBuf::from(format!("{home}/.local/share/maintainer"));
    if std::fs::metadata(share.join("profile.sh")).is_ok_and(|m| m.is_file()) {
        Ok(Tree { bin_dir, lib_dir: share })
    } else {
        Err(Exit {
            code: 64,
            stderr: format!(
                "{argv0}: line 18: {}: No such file or directory\n{argv0}: line 26: maintainer_load_profile: command not found\n",
                share.join("profile.sh").display()
            ),
        })
    }
}

/// Every whitespace character as a blank, so a value of several lines reads as one list.
fn one_line(value: &str) -> String {
    value.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).collect()
}

/// `printf %q`: the word as bash quotes it for reuse as input.
fn shell_quote(word: &str) -> String {
    if word.chars().any(char::is_control) {
        let mut out = String::from("$'");
        for c in word.chars() {
            match c {
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                '\'' => out.push_str("\\'"),
                '\\' => out.push_str("\\\\"),
                c if c.is_control() => {
                    use std::fmt::Write as _;
                    write!(out, "\\{:03o}", c as u32).ok();
                }
                c => out.push(c),
            }
        }
        out.push('\'');
        return out;
    }
    let mut out = String::new();
    for c in word.chars() {
        if !(c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c) || !c.is_ascii()) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `${N:?name}`: bash names its own file and the line of the read (bash 406, 465, 1242, 1783 and
/// 2155), so the Rust program does the same with the line of each.
fn missing_argument(argv0: &str, args: &[String], position: u8, name: &str) -> Exit {
    let line = match args.first().map(String::as_str) {
        Some("receipt") => 406,
        Some("verify-deps") => 465,
        Some("verify" | "verify-part") => 1242,
        Some("merge") => 1783,
        _ => 2155,
    };
    Exit { code: 1, stderr: format!("{argv0}: line {line}: {position}: {name}\n") }
}

/// Splits the process environment into the variables whose names and values are text, and the
/// names whose value is not UTF-8. A value that cannot be read must never read as unset
/// (review rv271 F2), so the caller hands the second list to [`run`].
#[must_use]
pub fn split_env(
    pairs: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> (BTreeMap<String, String>, Vec<String>) {
    let mut vars = BTreeMap::new();
    let mut bad = Vec::new();
    for (name, value) in pairs {
        let Ok(name) = name.into_string() else { continue };
        match value.into_string() {
            Ok(value) => {
                vars.insert(name, value);
            }
            Err(_) => bad.push(name),
        }
    }
    bad.sort();
    (vars, bad)
}

/// The names of variables this program reads.
fn is_read_by_the_gate(name: &str) -> bool {
    name.starts_with("MAINTAINER_")
        || name.starts_with("UPDATE_READBACK_")
        || name.starts_with("VERIFY_")
        || name.starts_with("PREFETCH_")
        || matches!(name, "PROD_GLOBS" | "CLAIM_LABEL" | "CLASSIFY_TIMEOUT" | "HOME" | "PATH")
        // The suites and the container runtime read these (P3W-4).
        || SUITE_ENV_NAMES.contains(&name)
        || RUNTIME_ENV_NAMES.contains(&name)
        // Everything a child receives: a value dropped here would run `gh` or `git` without it
        // (review rv271b N2).
        || CHILD_ENV_NAMES.contains(&name)
        || CHILD_ENV_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// A present value that is not text is a refusal, exit 64, naming the variable (decision P3B-13).
fn refuse_non_utf8(bad: &[String]) -> Result<(), Exit> {
    match bad.iter().find(|name| is_read_by_the_gate(name)) {
        Some(name) => Err(Exit {
            code: 64,
            stderr: format!("maintainer-merge: the value of {name} is not valid UTF-8 text\n"),
        }),
        None => Ok(()),
    }
}

/// The process entry: splits the environment, then runs. `main` only passes `vars_os` in, so
/// the refusal of a value that is not text is tested here and not in the binary (rv271b N3).
#[must_use]
pub fn run_process(
    argv0: &str,
    exe: &Path,
    args: &[String],
    environment: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> i32 {
    let (vars, bad_env) = split_env(environment);
    run(argv0, exe, args, &vars, &bad_env)
}

/// Runs the program. Returns the exit status; everything it prints goes to the real streams.
/// `bad_env` names the variables whose value was not UTF-8 (see [`split_env`]).
#[must_use]
pub fn run(
    argv0: &str,
    exe: &Path,
    args: &[String],
    vars: &BTreeMap<String, String>,
    bad_env: &[String],
) -> i32 {
    if let Err(exit) = refuse_non_utf8(bad_env) {
        drop(std::io::stderr().write_all(exit.stderr.as_bytes()));
        return exit.code;
    }
    let search_path = vars.get("PATH").cloned().unwrap_or_else(|| DEFAULT_PATH.to_string());
    let proc = RealProc::new(search_path.clone(), child_env(vars));
    let code = match run_inner(argv0, exe, args, vars, &proc, &search_path) {
        Ok(code) => code,
        Err(exit) => {
            drop(std::io::stderr().write_all(exit.stderr.as_bytes()));
            exit.code
        }
    };
    drop(std::io::stdout().flush());
    code
}

fn text(profile: &mx_profile::Profile, name: &str) -> String {
    profile.get_str(name).unwrap_or_default().to_string()
}

/// `cmd_receipt` refuses an unattended run after the pull request number and before it looks at
/// the head (bash 404 before 409), so that order is kept here (decision P3B-10).
fn refuse_unattended_receipt(args: &[String], vars: &BTreeMap<String, String>) -> Result<(), Exit> {
    if args.first().map(String::as_str) == Some("receipt")
        && vars.get("MAINTAINER_IN_RUN").is_some_and(|v| !v.is_empty())
        && args.len() >= 4
        && args[1..4].iter().all(|a| !a.is_empty())
        && crate::PrNumber::parse(&args[1]).is_ok()
    {
        return Err(Exit::die(1, crate::UNATTENDED_ASSERT));
    }
    Ok(())
}

/// Only `on` and `off` are values of `MAINTAINER_POST`; anything else is a usage error for every
/// subcommand, show included (bash PB-3). Empty reads as the default.
fn post_setting(
    profile: &mx_profile::Profile,
    vars: &BTreeMap<String, String>,
) -> Result<Option<String>, Exit> {
    let setting = profile
        .get_str("MAINTAINER_POST")
        .map(str::to_string)
        .or_else(|| vars.get("MAINTAINER_POST").cloned())
        .filter(|p| !p.is_empty());
    match setting.as_deref().filter(|p| !matches!(*p, "on" | "off")) {
        Some(value) => Err(Exit {
            code: 64,
            stderr: format!(
                "maintainer-merge: MAINTAINER_POST must be on or off, not {}\n",
                shell_quote(value)
            ),
        }),
        None => Ok(setting),
    }
}

/// The gate's configuration from the loaded profile and the environment.
fn build_config(
    profile: &mx_profile::Profile,
    vars: &BTreeMap<String, String>,
    post: Option<&str>,
) -> Result<GateConfig, Exit> {
    let account = Account::new(&text(profile, "MAINTAINER_ACCOUNT"))
        .map_err(|e| Exit::die(1, &e.to_string()))?;
    let home = vars.get("HOME").cloned().unwrap_or_default();
    let receipt_key = vars
        .get("MAINTAINER_RECEIPT_KEY")
        .cloned()
        .unwrap_or_else(|| format!("{home}/.local/share/maintainer/receipt.key"));
    let prod_value =
        profile.get_str("PROD_GLOBS").or_else(|| vars.get("PROD_GLOBS").map(String::as_str));
    // PB-4: every run of whitespace separates patterns, newlines included (decision P3B-12); the
    // suite crate's parser reads one line, so the lines are joined before it sees the value.
    let prod_value = prod_value.map(one_line);
    let pick =
        |name: &str| profile.get_str(name).map(str::to_string).or_else(|| vars.get(name).cloned());
    Ok(GateConfig {
        slug: text(profile, "MAINTAINER_SLUG"),
        repo: PathBuf::from(text(profile, "MAINTAINER_REPO")),
        state: PathBuf::from(text(profile, "MAINTAINER_STATE")),
        account,
        posting: Posting::from_setting(post),
        prod_globs: prod_value.and_then(|value| ProdGlobs::from_profile_value(&value).ok()),
        claim_label: pick("CLAIM_LABEL").unwrap_or_default(),
        receipt_key: PathBuf::from(receipt_key),
        run_id: std::process::id(),
        has_suite_host: true,
        in_run: vars.get("MAINTAINER_IN_RUN").is_some_and(|v| !v.is_empty()),
        readback_reads: pick("UPDATE_READBACK_READS"),
        readback_wait: pick("UPDATE_READBACK_WAIT"),
    })
}

#[allow(clippy::too_many_lines, reason = "one function: locate, load, configure, wire")]
fn run_inner(
    argv0: &str,
    exe: &Path,
    args: &[String],
    vars: &BTreeMap<String, String>,
    proc: &dyn Proc,
    search_path: &str,
) -> Result<i32, Exit> {
    let tree = locate(argv0, exe, vars)?;
    let env =
        LoadEnv::from_vars(tree.lib_dir.clone(), vars.iter().map(|(n, v)| (n.clone(), v.clone())));
    let loaded = load_profile(proc, &env);
    drop(std::io::stdout().write_all(&loaded.stdout));
    drop(std::io::stderr().write_all(&loaded.stderr));
    let profile = loaded.result.map_err(|error| {
        let code = error.exit_code();
        Exit {
            code: if code == 1 { 64 } else { code },
            stderr: String::from_utf8_lossy(&error.message()).into_owned(),
        }
    })?;
    for (line, name) in [
        (27, "MAINTAINER_SLUG"),
        (27, "MAINTAINER_REPO"),
        (28, "MAINTAINER_STATE"),
        (28, "MAINTAINER_ACCOUNT"),
    ] {
        if text(&profile, name).is_empty() {
            return Err(Exit {
                code: 1,
                stderr: format!("{argv0}: line {line}: {name}: parameter null or not set\n"),
            });
        }
    }
    refuse_unattended_receipt(args, vars)?;
    let post_setting = post_setting(&profile, vars)?;
    let command = parse_command(args).map_err(|error| match error {
        // `${N:?name}`: see `missing_argument`.
        crate::CommandError::Missing { position, name } => {
            missing_argument(argv0, args, position, name)
        }
        other => Exit::from(&other),
    })?;

    let cfg = build_config(&profile, vars, post_setting.as_deref())?;
    let pick =
        |name: &str| profile.get_str(name).map(str::to_string).or_else(|| vars.get(name).cloned());
    let mut settings = VerifySettings {
        verify_timeout: pick("VERIFY_TIMEOUT").unwrap_or_default(),
        prefetch_timeout: pick("PREFETCH_TIMEOUT").unwrap_or_default(),
        ..VerifySettings::default()
    };
    for key in LimitKey::ALL {
        if let Some(value) = vars.get(key.env_name()) {
            settings.env.insert(key.env_name().to_string(), value.clone());
        }
        if let Some(value) = pick(key.defaults_key()) {
            settings.profile.insert(key.defaults_key().to_string(), value);
        }
    }

    let runner = Runner::new(proc, vars);
    let gh = RealGh::new(&runner, &cfg.slug);
    let git = RealGateGit::new(&runner);
    let helper_env = vec![
        ("MAINTAINER_STATE".to_string(), cfg.state.display().to_string()),
        ("MAINTAINER_SLUG".to_string(), cfg.slug.clone()),
        (
            "MAINTAINER_PROFILE".to_string(),
            vars.get("MAINTAINER_PROFILE").cloned().unwrap_or_default(),
        ),
    ];
    let helper = RealHelper::new(&runner, &tree.bin_dir, search_path, helper_env);
    let identity = RealIdentity::new(&runner);
    let stamps = RealStamps { runner: &runner, vars };
    let runtime = PathRuntime { search_path: search_path.to_string() };
    if let Command::VerifyDeps { pr, sha } = &command {
        // `verify-deps` is the p3c module: its own adapters over a `Proc` that also hands the
        // credentials to the registry scripts and `gh`, as the bash does.
        let mut env_map = vars.clone();
        for (name, value) in profile.vars() {
            if let Ok(text) = std::str::from_utf8(value) {
                env_map.insert(name.to_string(), text.to_string());
            }
        }
        let mut deps_env = child_env(vars);
        deps_env.extend(
            TOKENS.iter().filter_map(|n| vars.get(*n).map(|v| ((*n).to_string(), v.clone()))),
        );
        let deps_proc = RealProc::new(search_path, deps_env);
        let scripts = crate::deps::ScriptPaths::from_env(|name| env_map.get(name).cloned());
        let dgh = crate::deps::ProcGh::new(&deps_proc, &cfg.slug);
        let net = crate::deps::RealNet::new(
            &deps_proc,
            scripts.image_resolver.clone(),
            scripts.pypi_verifier.clone(),
        );
        let classifier = crate::deps::RealClassifier::new(&deps_proc, scripts.classifier.clone());
        let whole = crate::deps::RealWholeFile::new(&deps_proc, scripts.pip_files.clone());
        let now = || stamps.stamp();
        let wiring = crate::deps::Wiring {
            ports: crate::deps::DepsPorts {
                gh: &dgh,
                net: &net,
                classifier: &classifier,
                whole_file: &whole,
                fs: &StdFs,
                say: &StdoutSay,
            },
            helper: &helper,
            stamp: &now,
        };
        let exit = crate::deps::run(&env_map, pr, sha, &wiring);
        return if exit.code == 0 { Ok(0) } else { Err(exit) };
    }
    // The suites and the sandbox start their own processes through a `Proc` each, with the
    // variables they read and no credential (decision P3W-4).
    let host = RealSuiteHost::new(
        Arc::new(RealProc::new(search_path, suite_env(vars, &pick))),
        SuiteDirs {
            profile_dir: pick("PROFILE_DIR").unwrap_or_default(),
            home: vars.get("HOME").cloned().unwrap_or_default(),
            profile: pick("MAINTAINER_PROFILE").unwrap_or_default(),
            here: tree.bin_dir.clone(),
        },
    );
    let selector = HostSelector::new(&host);
    // A docker machine is refused by name before the sandbox is used (P3W-3), so the sandbox is
    // always made for podman; the runtime of each run is in its own spec.
    let sandbox = RealSandbox::new(
        Arc::new(RealProc::new(search_path, child_env(vars))),
        Arc::new(RealPipe::new(search_path, None)),
        Runtime::Podman,
        pick_env(&RUNTIME_ENV_NAMES, |name| vars.get(name).cloned()),
    )
    .map_err(|error| Exit::die(1, &error.to_string()))?;
    let stage = RealStage::new(&sandbox, exe);
    let gate = Gate::new(
        &cfg,
        Ports {
            gh: &gh,
            git: &git,
            helper: &helper,
            identity: &identity,
            locks: &FileLocks,
            fs: &StdFs,
            dirs: &StdDirs,
            say: &StdoutSay,
            clock: &SystemClock,
            stamps: &stamps,
            selector: &selector,
            suites: &host,
            sandbox: &sandbox,
            stage: &stage,
            runtime: &runtime,
            verify: &settings,
        },
    );
    let printed = gate.run(command)?;
    drop(std::io::stdout().write_all(&printed.stdout));
    drop(std::io::stderr().write_all(printed.stderr.as_bytes()));
    let _ = USAGE;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
    }

    fn names(env: &[(String, String)]) -> Vec<&str> {
        env.iter().map(|(n, _)| n.as_str()).collect()
    }

    #[test]
    fn a_child_gets_the_allowlist_and_the_maintainer_variables_and_nothing_else() {
        let env = child_env(&vars(&[
            ("HOME", "/h"),
            ("MAINTAINER_STATE", "/s"),
            ("SECRET_THING", "x"),
            ("EDITOR", "vi"),
            ("PATH", "/bin"),
            ("LC_ALL", "de"),
        ]));
        assert_eq!(names(&env), ["HOME", "MAINTAINER_STATE"]);
    }

    #[test]
    fn the_variables_that_point_git_at_another_repository_never_reach_a_child() {
        let env = child_env(&vars(&[
            ("GIT_DIR", "/hook/.git"),
            ("GIT_INDEX_FILE", "/hook/index"),
            ("GIT_WORK_TREE", "/hook"),
            ("GIT_AUTHOR_NAME", "n"),
        ]));
        assert_eq!(names(&env), ["GIT_AUTHOR_NAME"]);
    }

    #[test]
    fn a_credential_is_kept_out_of_the_base_environment_so_only_gh_and_git_get_it() {
        let env = child_env(&vars(&[("GH_TOKEN", "t"), ("GITHUB_TOKEN", "t"), ("GH_HOST", "h")]));
        assert_eq!(names(&env), ["GH_HOST"]);
    }

    #[test]
    fn a_suite_gets_what_a_child_gets_plus_the_variables_the_shipped_suites_read_and_no_credential()
    {
        let env = vars(&[
            ("HOME", "/h"),
            ("GH_TOKEN", "t"),
            ("EDITOR", "vi"),
            ("MAINTAINER_CARGO_JOBS", "2"),
        ]);
        let from_profile = vars(&[("CHARACTERIZATION_DIR", "/c"), ("MAINTAINER_SHELL_IMAGE", "i")]);
        let pick = |name: &str| env.get(name).or_else(|| from_profile.get(name)).cloned();
        let seen = suite_env(&env, &pick);
        assert_eq!(
            names(&seen),
            ["HOME", "MAINTAINER_CARGO_JOBS", "MAINTAINER_SHELL_IMAGE", "CHARACTERIZATION_DIR"]
        );
        assert!(seen.contains(&("CHARACTERIZATION_DIR".to_string(), "/c".to_string())));
    }

    #[test]
    fn a_suite_variable_that_the_environment_holds_is_not_added_twice() {
        let env = vars(&[("HOME", "/h"), ("MAINTAINER_SHELL_IMAGE", "i")]);
        let pick = |name: &str| env.get(name).cloned();
        let seen = suite_env(&env, &pick);
        assert_eq!(seen.iter().filter(|(n, _)| n == "MAINTAINER_SHELL_IMAGE").count(), 1);
    }

    #[test]
    fn a_name_listed_in_maintainer_child_env_passes_too() {
        let env = child_env(&vars(&[
            ("MAINTAINER_CHILD_ENV", "STUB_X STUB_Y"),
            ("STUB_X", "1"),
            ("STUB_Z", "2"),
        ]));
        assert_eq!(names(&env), ["MAINTAINER_CHILD_ENV", "STUB_X"]);
    }

    #[test]
    fn the_program_finds_its_lib_folder_beside_its_bin_folder() {
        let dir = std::env::temp_dir().join(format!("mxgate-app-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("lib/profile.sh"), "").unwrap();
        let tree =
            locate(&format!("{}/bin/maintainer-merge", dir.display()), Path::new("/x"), &vars(&[]))
                .unwrap();
        assert!(tree.lib_dir.ends_with("bin/../lib"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_argument_names_the_program_the_bash_line_and_the_position() {
        let line = |sub: &str| {
            missing_argument("bin/maintainer-merge", &[sub.to_string()], 1, "pr").stderr
        };
        assert_eq!(line("receipt"), "bin/maintainer-merge: line 406: 1: pr\n");
        assert_eq!(line("verify-deps"), "bin/maintainer-merge: line 465: 1: pr\n");
        assert_eq!(line("verify"), "bin/maintainer-merge: line 1242: 1: pr\n");
        assert_eq!(line("verify-part"), "bin/maintainer-merge: line 1242: 1: pr\n");
        assert_eq!(line("merge"), "bin/maintainer-merge: line 1783: 1: pr\n");
        assert_eq!(line("update"), "bin/maintainer-merge: line 2155: 1: pr\n");
        assert_eq!(missing_argument("m", &["merge".into()], 1, "pr").code, 1);
    }

    #[test]
    fn the_unattended_refusal_text_is_the_bash_one() {
        assert!(crate::UNATTENDED_ASSERT.starts_with(
            "an unattended run may not assert a receipt.\n  Only `maintainer-merge verify`"
        ));
    }

    #[test]
    fn a_tree_without_a_profile_loader_exits_64_with_the_bash_two_lines() {
        let exit = locate(
            "maintainer-merge",
            Path::new("/nonexistent/bin/maintainer-merge"),
            &vars(&[("HOME", "/nonexistent-home")]),
        )
        .unwrap_err();
        assert_eq!(exit.code, 64);
        assert!(exit.stderr.contains("maintainer_load_profile: command not found"), "{exit:?}");
    }

    #[test]
    fn without_a_home_the_bash_stops_on_the_unbound_variable_with_exit_1() {
        let exit =
            locate("maintainer-merge", Path::new("/nonexistent/bin/maintainer-merge"), &vars(&[]))
                .unwrap_err();
        assert_eq!(exit.code, 1);
        assert!(exit.stderr.contains("HOME: unbound variable"));
    }
}

#[cfg(test)]
mod post_tests {
    use super::{one_line, shell_quote};

    #[test]
    fn patterns_on_later_lines_and_after_tabs_still_count_pb_4() {
        let globs =
            mx_suite::ProdGlobs::from_profile_value(&one_line("  crates/*/src/*\t\n\n  lib/*  \n"))
                .unwrap();
        assert_eq!(globs.as_slice(), ["crates/*/src/*", "lib/*"]);
    }

    #[test]
    fn a_word_is_quoted_as_printf_q_quotes_it() {
        assert_eq!(shell_quote("maybe"), "maybe");
        assert_eq!(shell_quote(" off"), "\\ off");
        assert_eq!(shell_quote("off "), "off\\ ");
        assert_eq!(shell_quote("a\nb"), "$'a\\nb'");
        assert_eq!(shell_quote("it's"), "it\\'s");
    }
}

/// `run_inner` and the configuration it builds, on a real profile load (the bash loader, through
/// the real `Proc`) over a scratch tree, so the unit suite itself holds the guards of the
/// environment: an unattended run, `MAINTAINER_POST`, and variables that are not text.
#[cfg(test)]
mod run_tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Scratch {
        dir: PathBuf,
        vars: BTreeMap<String, String>,
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            drop(std::fs::remove_dir_all(&self.dir));
        }
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    fn scratch(extra: &[(&str, &str)]) -> Scratch {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let exe = std::env::current_exe().unwrap();
        let base = exe.parent().and_then(Path::parent).unwrap().join("mx-gate-scratch");
        let dir = base.join(format!(
            "run-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        drop(std::fs::remove_dir_all(&dir));
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        copy_dir(&root.join("lib"), &dir.join("lib"));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        let mut vars: BTreeMap<String, String> = [
            ("HOME", dir.join("home").display().to_string()),
            ("PATH", "/usr/bin:/bin".to_string()),
            ("MAINTAINER_SLUG", "o/r".to_string()),
            ("MAINTAINER_REPO", dir.join("repo").display().to_string()),
            ("MAINTAINER_STATE", dir.join("state").display().to_string()),
            ("MAINTAINER_ACCOUNT", "testuser".to_string()),
            ("MAINTAINER_PROFILE", "t".to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        for (k, v) in extra {
            vars.insert((*k).to_string(), (*v).to_string());
        }
        Scratch { dir, vars }
    }

    fn exec(s: &Scratch, args: &[&str]) -> Result<i32, Exit> {
        let args: Vec<String> = args.iter().map(ToString::to_string).collect();
        let argv0 = s.dir.join("bin/maintainer-merge").display().to_string();
        let search = "/usr/bin:/bin";
        let proc = RealProc::new(search, child_env(&s.vars));
        run_inner(&argv0, Path::new("/x"), &args, &s.vars, &proc, search)
    }

    fn profile_of(s: &Scratch) -> mx_profile::Profile {
        let proc = RealProc::new("/usr/bin:/bin", child_env(&s.vars));
        let env = LoadEnv::from_vars(
            s.dir.join("lib"),
            s.vars.iter().map(|(n, v)| (n.clone(), v.clone())),
        );
        load_profile(&proc, &env).result.expect("the scratch profile loads")
    }

    fn config(extra: &[(&str, &str)]) -> GateConfig {
        let s = scratch(extra);
        let profile = profile_of(&s);
        let post = post_setting(&profile, &s.vars).unwrap();
        build_config(&profile, &s.vars, post.as_deref()).unwrap()
    }

    #[test]
    fn show_runs_end_to_end_through_the_loader_and_lists_no_receipts() {
        let s = scratch(&[]);
        assert_eq!(exec(&s, &["show"]).unwrap(), 0);
        assert!(s.dir.join("state/receipts").is_dir());
    }

    #[test]
    fn only_the_exact_words_on_and_off_are_values_of_maintainer_post_for_every_subcommand() {
        for bad in ["OFF", "Off", "false", "0", "no", "yes", "true", " off", "off ", "maybe"] {
            let s = scratch(&[("MAINTAINER_POST", bad)]);
            for args in [&["show"][..], &["merge", "7"], &["update", "7"]] {
                let exit = exec(&s, args).unwrap_err();
                assert_eq!(exit.code, 64, "{bad:?} {args:?}");
                assert_eq!(
                    exit.stderr,
                    format!(
                        "maintainer-merge: MAINTAINER_POST must be on or off, not {}\n",
                        shell_quote(bad)
                    ),
                );
            }
        }
        for good in ["on", "off", ""] {
            let s = scratch(&[("MAINTAINER_POST", good)]);
            assert_eq!(exec(&s, &["show"]).unwrap(), 0, "{good:?}");
        }
    }

    #[test]
    fn posting_is_off_only_for_the_exact_word_off_and_on_otherwise() {
        assert_eq!(config(&[("MAINTAINER_POST", "off")]).posting, Posting::Off);
        assert_eq!(config(&[("MAINTAINER_POST", "on")]).posting, Posting::On);
        assert_eq!(config(&[("MAINTAINER_POST", "")]).posting, Posting::On);
        assert_eq!(config(&[]).posting, Posting::On);
    }

    #[test]
    fn an_unattended_run_is_one_whose_in_run_variable_is_set_and_not_empty() {
        assert!(config(&[("MAINTAINER_IN_RUN", "review")]).in_run);
        assert!(!config(&[("MAINTAINER_IN_RUN", "")]).in_run);
        assert!(!config(&[]).in_run);
    }

    #[test]
    fn an_unattended_receipt_is_refused_after_the_number_and_before_the_head_p3b_10() {
        let s = scratch(&[("MAINTAINER_IN_RUN", "review")]);
        let exit = exec(&s, &["receipt", "7", "abc", "proof"]).unwrap_err();
        assert_eq!(
            exit,
            Exit::die(1, crate::UNATTENDED_ASSERT),
            "a short head does not come first"
        );
        let exit = exec(&s, &["receipt", "x", "abc", "proof"]).unwrap_err();
        assert!(exit.stderr.contains("pull request must be a positive integer"), "{exit:?}");
        let attended = scratch(&[]);
        let exit = exec(&attended, &["receipt", "7", "abc", "proof"]).unwrap_err();
        assert_eq!(exit.stderr, "maintainer-merge: head sha looks truncated: abc\n");
    }

    #[test]
    fn the_prod_globs_of_a_profile_count_on_every_line_p3b_12() {
        let globs = config(&[("PROD_GLOBS", "bin/*\n\tlib/*  \n")]).prod_globs.unwrap();
        assert_eq!(globs.as_slice(), ["bin/*", "lib/*"]);
        assert!(config(&[("PROD_GLOBS", "")]).prod_globs.is_none());
    }

    #[test]
    fn the_claim_label_comes_from_the_environment_and_the_readback_reads_from_the_defaults() {
        let c = config(&[("CLAIM_LABEL", "claimed")]);
        assert_eq!(c.claim_label, "claimed");
        assert_eq!(
            c.readback_reads.as_deref(),
            Some("6"),
            "lib/defaults.env, which the loader reads"
        );
        assert!(config(&[]).claim_label.is_empty());
    }

    #[test]
    fn a_variable_that_is_not_text_is_split_out_and_never_read_as_unset_p3b_13() {
        let pairs = vec![
            (OsString::from("MAINTAINER_IN_RUN"), OsString::from_vec(vec![0xff])),
            (OsString::from("HOME"), OsString::from("/h")),
            (OsString::from_vec(vec![0xfe]), OsString::from("x")),
        ];
        let (vars, bad) = split_env(pairs);
        assert_eq!(bad, ["MAINTAINER_IN_RUN"]);
        assert_eq!(vars.get("HOME").map(String::as_str), Some("/h"));
        assert!(!vars.contains_key("MAINTAINER_IN_RUN"));
    }

    #[test]
    fn a_non_text_value_of_a_name_the_gate_reads_refuses_with_exit_64_naming_it() {
        for name in [
            "MAINTAINER_IN_RUN",
            "MAINTAINER_POST",
            "PROD_GLOBS",
            "CLAIM_LABEL",
            "UPDATE_READBACK_READS",
            "VERIFY_MEMORY",
            "PATH",
            "HOME",
        ] {
            let exit = refuse_non_utf8(&[name.to_string()]).unwrap_err();
            assert_eq!(exit.code, 64, "{name}");
            assert_eq!(
                exit.stderr,
                format!("maintainer-merge: the value of {name} is not valid UTF-8 text\n")
            );
        }
        assert!(
            refuse_non_utf8(&["LS_COLORS".to_string()]).is_ok(),
            "an unrelated name is ignored"
        );
        assert!(refuse_non_utf8(&[]).is_ok());
    }

    #[test]
    fn a_non_text_value_of_a_name_a_suite_or_the_container_runtime_reads_refuses_too_p3w_4() {
        for name in [
            "CHARACTERIZATION_DIR",
            "MAINTAINER_SHELL_IMAGE",
            "DBUS_SESSION_BUS_ADDRESS",
            "CONTAINERS_CONF",
            "CONTAINERS_STORAGE_CONF",
            "XDG_RUNTIME_DIR",
        ] {
            let exit = refuse_non_utf8(&[name.to_string()]).unwrap_err();
            assert_eq!(exit.code, 64, "{name}");
            assert!(exit.stderr.contains(name), "{exit:?}");
        }
    }

    #[test]
    fn the_program_is_built_with_a_suite_host_and_a_sandbox_p3w_4() {
        assert!(config(&[]).has_suite_host);
    }

    #[test]
    fn a_non_text_value_of_a_name_a_child_receives_refuses_too_n2() {
        for name in [
            "GH_TOKEN",
            "GH_HOST",
            "GIT_AUTHOR_NAME",
            "XDG_CONFIG_HOME",
            "LC_CTYPE",
            "TMPDIR",
            "SSL_CERT_FILE",
            "HTTPS_PROXY",
            "PYTHONPATH",
        ] {
            let exit = refuse_non_utf8(&[name.to_string()]).unwrap_err();
            assert_eq!(exit.code, 64, "{name}");
        }
    }

    #[test]
    fn the_process_entry_hands_the_non_text_names_to_run_so_the_guard_cannot_be_dropped_n3() {
        let s = scratch(&[]);
        // Every variable the profile load needs, so only the refusal can end the run with 64:
        // were the unreadable value dropped, the receipt would go on and fail on the missing key
        // with exit 1.
        let mut environment: Vec<(OsString, OsString)> =
            s.vars.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect();
        environment.push((OsString::from("MAINTAINER_IN_RUN"), OsString::from_vec(vec![0xff])));
        let code = run_process(
            &s.dir.join("bin/maintainer-merge").display().to_string(),
            Path::new("/x"),
            &["receipt".into(), "7".into(), "abcdef0".into(), "p".into()],
            environment,
        );
        assert_eq!(code, 64);
        assert!(!s.dir.join("state").exists(), "nothing was read or written");
    }

    #[test]
    fn run_refuses_before_it_loads_anything_when_a_gate_variable_is_not_text() {
        let s = scratch(&[]);
        let code = run(
            &s.dir.join("bin/maintainer-merge").display().to_string(),
            Path::new("/x"),
            &["receipt".into(), "7".into(), "abcdef0".into(), "p".into()],
            &s.vars,
            &["MAINTAINER_IN_RUN".to_string()],
        );
        assert_eq!(code, 64);
        assert!(!s.dir.join("state").exists(), "nothing was read or written");
    }
}
