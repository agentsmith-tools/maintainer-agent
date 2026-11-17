//! A thin command line over the library, used by `tests/differential.sh` to
//! prove the crate equal to `lib/profile.sh`. It is a test helper: the programs
//! that use this crate come later.
//!
//! ```text
//! mx-profile-oracle classify --lib-dir DIR LOG
//! mx-profile-oracle predicate transient|needs-human --lib-dir DIR LOG
//! mx-profile-oracle scrub SECRET_ENV_NAME PATH...
//! mx-profile-oracle load --lib-dir DIR [--dump FILE] [--exports FILE]
//! ```
//!
//! Each subcommand prints what the bash function prints and exits with the
//! status the function returns. `classify` and `predicate` read
//! `MAINTAINER_FAILURE_SHAPES`, `HOME` and `PATH` from the environment. `scrub`
//! reads the secret from the environment variable it is told the NAME of, so the
//! secret is never in argv. `load` reads `HOME`, `PROFILE_DIR`, `PATH` and the
//! `MAINTAINER_*` variables, prints what the sourced files printed and the
//! refusal text, and writes the resulting variables to `--dump` as
//! `NAME NUL VALUE NUL` records sorted by name, and the names the function
//! exported to `--exports`, one per line. A file that calls `exit` ends the
//! oracle with the same status, as it would end a bash tool.
//!
//! Exit 64 is a usage error. The oracle does not restore the default `SIGPIPE`
//! action, because that needs `unsafe`, which this workspace forbids; a write to
//! a closed pipe is an error and exits 1.

use mx_ports::{RealProc, Secret};
use mx_profile::classify::{ClassifyEnv, classify_failure, effective_locale};
use mx_profile::scrub::scrub_secret;
use mx_profile::{LoadEnv, LoadError, load_profile};
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::process::ExitCode;

fn usage(message: &str) -> ExitCode {
    eprintln!("mx-profile-oracle: {message}");
    ExitCode::from(64)
}

fn write_stdout(bytes: &[u8]) -> bool {
    std::io::stdout().lock().write_all(bytes).is_ok()
}

fn write_stderr(bytes: &[u8]) {
    drop(std::io::stderr().lock().write_all(bytes));
}

/// A real process adapter whose search path is this process's own `PATH`.
fn real_proc() -> RealProc {
    RealProc::new(std::env::var_os("PATH").unwrap_or_default(), Vec::new())
}

/// Takes `--lib-dir DIR` out of `args` and returns the directory.
fn take_lib_dir(args: &mut Vec<OsString>) -> Result<PathBuf, String> {
    let at = args
        .iter()
        .position(|arg| arg == "--lib-dir")
        .ok_or_else(|| "missing --lib-dir DIR".to_string())?;
    if at + 1 >= args.len() {
        return Err("--lib-dir needs a directory".into());
    }
    let dir = args.remove(at + 1);
    args.remove(at);
    Ok(PathBuf::from(dir))
}

fn take_flag(args: &mut Vec<OsString>, flag: &str) -> Result<Option<PathBuf>, String> {
    let Some(at) = args.iter().position(|arg| arg == flag) else { return Ok(None) };
    if at + 1 >= args.len() {
        return Err(format!("{flag} needs a file"));
    }
    let value = args.remove(at + 1);
    args.remove(at);
    Ok(Some(PathBuf::from(value)))
}

/// A variable as text, when set and UTF-8.
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn classify_env(lib_dir: PathBuf) -> ClassifyEnv {
    ClassifyEnv {
        failure_shapes: std::env::var_os("MAINTAINER_FAILURE_SHAPES"),
        lib_dir,
        home: std::env::var_os("HOME").unwrap_or_default(),
        locale: effective_locale(
            var("LC_ALL").as_deref(),
            var("LC_CTYPE").as_deref(),
            var("LANG").as_deref(),
        ),
    }
}

fn cmd_classify(mut args: Vec<OsString>) -> ExitCode {
    let lib_dir = match take_lib_dir(&mut args) {
        Ok(dir) => dir,
        Err(message) => return usage(&message),
    };
    let log = args.first().map(PathBuf::from).unwrap_or_default();
    let got = classify_failure(&real_proc(), &classify_env(lib_dir), &log);
    if !write_stdout(&got.render()) {
        return ExitCode::from(1);
    }
    ExitCode::from(u8::try_from(got.exit_code()).unwrap_or(1))
}

fn cmd_predicate(mut args: Vec<OsString>) -> ExitCode {
    let lib_dir = match take_lib_dir(&mut args) {
        Ok(dir) => dir,
        Err(message) => return usage(&message),
    };
    let which = args.first().and_then(|arg| arg.to_str()).map(str::to_string);
    let log = args.get(1).map(PathBuf::from).unwrap_or_default();
    let got = classify_failure(&real_proc(), &classify_env(lib_dir), &log);
    let reason = match which.as_deref() {
        Some("transient") => got.transient_reason(),
        Some("needs-human") => got.needs_human_reason(),
        _ => return usage("predicate takes transient or needs-human"),
    };
    match reason {
        Some(text) => {
            if !write_stdout(&text) {
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        None => ExitCode::from(1),
    }
}

fn cmd_scrub(args: &[OsString]) -> ExitCode {
    let Some(name) = args.first() else {
        return usage("scrub needs the NAME of the secret's variable");
    };
    let secret = match std::env::var(name) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => String::new(),
        Err(std::env::VarError::NotUnicode(_)) => {
            eprintln!("mx-profile-oracle: the secret is not UTF-8 text");
            return ExitCode::from(1);
        }
    };
    let targets: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();
    match scrub_secret(&Secret::new(secret), &targets) {
        Ok(report) => {
            if !write_stdout(&report.render()) {
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Err(refusal) => ExitCode::from(u8::try_from(refusal.exit_code()).unwrap_or(1)),
    }
}

fn cmd_load(mut args: Vec<OsString>) -> ExitCode {
    let lib_dir = match take_lib_dir(&mut args) {
        Ok(dir) => dir,
        Err(message) => return usage(&message),
    };
    let dump = match take_flag(&mut args, "--dump") {
        Ok(path) => path,
        Err(message) => return usage(&message),
    };
    let exports = match take_flag(&mut args, "--exports") {
        Ok(path) => path,
        Err(message) => return usage(&message),
    };
    let mut vars: Vec<(String, String)> = Vec::new();
    for (name, value) in std::env::vars_os() {
        let wanted =
            name == "HOME" || name == "PROFILE_DIR" || name.as_bytes().starts_with(b"MAINTAINER_");
        if !wanted {
            continue;
        }
        let (Some(name), Some(value)) = (name.to_str(), value.to_str()) else {
            return usage("a variable the loader reads is not UTF-8 text");
        };
        vars.push((name.to_string(), value.to_string()));
    }
    let loaded = load_profile(&real_proc(), &LoadEnv::from_vars(lib_dir, vars));
    let mut ok = write_stdout(&loaded.stdout);
    write_stderr(&loaded.stderr);
    match loaded.result {
        Ok(profile) => {
            if let Some(path) = dump {
                let mut bytes = Vec::new();
                for (name, value) in profile.vars() {
                    bytes.extend_from_slice(name.as_bytes());
                    bytes.push(0);
                    bytes.extend_from_slice(value);
                    bytes.push(0);
                }
                ok &= std::fs::write(path, bytes).is_ok();
            }
            if let Some(path) = exports {
                let mut names: Vec<&str> = profile.exported().iter().map(String::as_str).collect();
                names.sort_unstable();
                let mut text = names.join("\n");
                if !text.is_empty() {
                    text.push('\n');
                }
                ok &= std::fs::write(path, text).is_ok();
            }
            if ok { ExitCode::SUCCESS } else { ExitCode::from(1) }
        }
        Err(error) => {
            // The bash says nothing when `defaults.env` fails or a file calls
            // `exit`, so neither does the oracle. The failures that exist only
            // because bash is a child here get a line of their own.
            let message = error.message();
            if !message.is_empty() {
                write_stderr(&message);
            } else if matches!(
                error,
                LoadError::Bash(_) | LoadError::Malformed(_) | LoadError::EvalTimedOut
            ) {
                eprintln!("mx-profile-oracle: {error}");
            }
            ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1))
        }
    }
}

fn main() -> ExitCode {
    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.is_empty() {
        return usage("missing command");
    }
    let command = args.remove(0);
    match command.to_str() {
        Some("classify") => cmd_classify(args),
        Some("predicate") => cmd_predicate(args),
        Some("scrub") => cmd_scrub(&args),
        Some("load") => cmd_load(args),
        _ => usage(&format!("unknown command '{}'", String::from_utf8_lossy(&command.into_vec()))),
    }
}
