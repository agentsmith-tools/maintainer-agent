//! The `maintainer-merge` program: locate the tree, load the profile through `bash` as the bash
//! program does, build the real adapters and run the gate. `main.rs` only reads the process
//! environment and exits with the status this returns.

use crate::real::{
    FileLocks, PathRuntime, RealGateGit, RealGh, RealHelper, RealIdentity, RealStamps, Runner,
    StdDirs, StdFs, StdoutSay, SystemClock,
};
use crate::{
    Account, Exit, Gate, GateConfig, Ports, Posting, USAGE, VerifySettings, parse_command,
};
use mx_ports::{Proc, RealProc};
use mx_profile::{LoadEnv, load_profile};
use mx_suite::{LimitKey, ProdGlobs};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

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

/// `${N:?name}`: bash names its own file and the line of the read (bash 396, 455, 1222, 1748 and
/// 2120), so the Rust program does the same with the line of each.
fn missing_argument(argv0: &str, args: &[String], position: u8, name: &str) -> Exit {
    let line = match args.first().map(String::as_str) {
        Some("receipt") => 396,
        Some("verify-deps") => 455,
        Some("verify" | "verify-part") => 1222,
        Some("merge") => 1748,
        _ => 2120,
    };
    Exit { code: 1, stderr: format!("{argv0}: line {line}: {position}: {name}\n") }
}

/// Runs the program. Returns the exit status; everything it prints goes to the real streams.
#[must_use]
pub fn run(argv0: &str, exe: &Path, args: &[String], vars: &BTreeMap<String, String>) -> i32 {
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
    // `cmd_receipt` refuses an unattended run after the pull request number and before it looks at
    // the head (bash 404 before 409), so that order is kept here (decision P3B-10).
    if args.first().map(String::as_str) == Some("receipt")
        && vars.get("MAINTAINER_IN_RUN").is_some_and(|v| !v.is_empty())
        && args.len() >= 4
        && args[1..4].iter().all(|a| !a.is_empty())
        && crate::PrNumber::parse(&args[1]).is_ok()
    {
        return Err(Exit::die(1, crate::UNATTENDED_ASSERT));
    }
    let command = parse_command(args).map_err(|error| match error {
        // `${N:?name}`: bash names its own file and the line of the read (bash 396, 455, 1222,
        // 1748 and 2120), so the Rust program does the same with the line of each.
        crate::CommandError::Missing { position, name } => {
            missing_argument(argv0, args, position, name)
        }
        other => Exit::from(&other),
    })?;

    let state = PathBuf::from(text(&profile, "MAINTAINER_STATE"));
    let account = Account::new(&text(&profile, "MAINTAINER_ACCOUNT"))
        .map_err(|e| Exit::die(1, &e.to_string()))?;
    let home = vars.get("HOME").cloned().unwrap_or_default();
    let receipt_key = vars
        .get("MAINTAINER_RECEIPT_KEY")
        .cloned()
        .unwrap_or_else(|| format!("{home}/.local/share/maintainer/receipt.key"));
    let prod_value =
        profile.get_str("PROD_GLOBS").or_else(|| vars.get("PROD_GLOBS").map(String::as_str));
    let post = vars
        .get("MAINTAINER_POST")
        .cloned()
        .or_else(|| profile.get_str("MAINTAINER_POST").map(str::to_string));
    let pick =
        |name: &str| profile.get_str(name).map(str::to_string).or_else(|| vars.get(name).cloned());
    let cfg = GateConfig {
        slug: text(&profile, "MAINTAINER_SLUG"),
        repo: PathBuf::from(text(&profile, "MAINTAINER_REPO")),
        state: state.clone(),
        account,
        posting: Posting::from_setting(post.as_deref().filter(|p| !p.is_empty()).or(None)),
        prod_globs: prod_value.and_then(|value| ProdGlobs::from_profile_value(value).ok()),
        claim_label: pick("CLAIM_LABEL").unwrap_or_default(),
        receipt_key: PathBuf::from(receipt_key),
        run_id: std::process::id(),
        in_run: vars.get("MAINTAINER_IN_RUN").is_some_and(|v| !v.is_empty()),
        readback_reads: pick("UPDATE_READBACK_READS"),
        readback_wait: pick("UPDATE_READBACK_WAIT"),
    };
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
    let pending = crate::pending::Pending;
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
            selector: &pending,
            suites: &pending,
            sandbox: &pending,
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
        assert_eq!(line("receipt"), "bin/maintainer-merge: line 396: 1: pr\n");
        assert_eq!(line("verify-deps"), "bin/maintainer-merge: line 455: 1: pr\n");
        assert_eq!(line("verify"), "bin/maintainer-merge: line 1222: 1: pr\n");
        assert_eq!(line("verify-part"), "bin/maintainer-merge: line 1222: 1: pr\n");
        assert_eq!(line("merge"), "bin/maintainer-merge: line 1748: 1: pr\n");
        assert_eq!(line("update"), "bin/maintainer-merge: line 2120: 1: pr\n");
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
