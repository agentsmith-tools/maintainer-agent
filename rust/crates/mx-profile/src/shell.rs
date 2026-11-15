//! Evaluating shell-syntax data (`defaults.env`, `profile.env`) in a child bash.
//!
//! Both files are shell code that the maintainer owns and the bash tools
//! `source`. This crate does not parse them. It starts `bash` through the
//! `Proc` port with an empty environment plus an allowlist, sources the files
//! in order under `set -a`, and reads every exported variable back with
//! `compgen -e`, in a framing that survives any byte but NUL.
//!
//! # What a profile file can do to the child
//!
//! A profile is trusted data, in the same sense as the bash tools trust it
//! today. Sourced in the child it can run any command as the maintainer, write
//! any file, print to standard output and standard error, assign any variable,
//! `exit`, and `return`. The read-back sees only what is exported when the files
//! are done, so an array, a function, or an assignment after `set +a` is not in
//! the result. The child sees only `HOME`, `PROFILE_DIR`, the `MAINTAINER_*`
//! variables the caller passed, `PATH` and `LC_ALL=C` from the adapter, so a
//! profile that expands another variable, such as `$USER`, gets an empty string
//! where the bash tools would have given it the caller's value (decision P1F-7).
//!
//! What a profile does to the child, and how this module reports it:
//!
//! | the profile                       | bash today                          | here |
//! |-----------------------------------|-------------------------------------|------|
//! | prints to stdout or stderr        | goes to the tool's own streams      | returned to the caller, which replays it |
//! | fails its last command            | `.` returns non-zero, the function goes on | the same; only `defaults.env`'s last status is read |
//! | runs `return N`                   | stops sourcing, the function goes on | the same |
//! | runs `exit N`                     | the tool exits with N               | `LoadError::ShellExited` with N |
//! | assigns an unrelated variable     | the variable exists in the tool     | it is in the read-back |
//!
//! The child keeps no positional parameters, as a sourced file inside a bash
//! function sees none.

use crate::LoadEnv;
use crate::load::LoadError;
use mx_ports::{Proc, ProcExit, ProcSpec};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The child script. The frame token is `$1` and the files to source follow.
///
/// Everything after the sourcing uses builtins, so a profile that changes
/// `PATH` or defines a function with the name of a command cannot break the
/// read-back. `__mx_*` names belong to this script and are left out of it.
const SCRIPT: &str = r#"__mx_frame=$1
shift
__mx_files=("$@")
set --
set -a
__mx_status=0
for __mx_f in "${__mx_files[@]}"; do
    . "$__mx_f"
    __mx_status=$?
done
set +e +u
__mx_names=$(builtin compgen -e)
builtin printf '\0MX-PROFILE-%s\0' "$__mx_frame"
builtin printf '%s\0%s\0' __mx_status "$__mx_status"
while IFS= builtin read -r __mx_n; do
    case $__mx_n in __mx_*) continue ;; esac
    builtin printf '%s\0%s\0' "$__mx_n" "${!__mx_n}"
done <<< "$__mx_names"
exit 0
"#;

/// Names bash itself exports into a child, which are not the profile's.
const BASH_OWN: [&str; 4] = ["PWD", "OLDPWD", "SHLVL", "_"];

/// Everything the child exported when the files were done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Dump {
    pub vars: BTreeMap<String, Vec<u8>>,
    /// The status of the last file sourced, as the `.` builtin returned it.
    pub last_status: i32,
}

/// What one child run printed, and what it read back.
#[derive(Debug)]
pub(crate) struct Evaluation {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub outcome: Result<Dump, LoadError>,
}

/// A token that no output of the files can hold by accident.
fn frame_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{}-{nanos}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Sources `files` in order in a child bash and reads the exported variables back.
pub(crate) fn evaluate(proc: &dyn Proc, env: &LoadEnv, files: &[PathBuf]) -> Evaluation {
    let token = frame_token();
    let mut args: Vec<String> =
        vec!["-c".into(), SCRIPT.into(), "mx-profile-eval".into(), token.clone()];
    for file in files {
        let Some(text) = file.to_str() else {
            return failed(LoadError::Bash("a file path is not UTF-8 text".into()));
        };
        args.push(text.to_string());
    }
    let mut spec = ProcSpec::command("bash", args, env.eval_timeout);
    if let Some(home) = &env.home {
        spec = spec.env("HOME", home.as_str());
    }
    if let Some(dir) = &env.profile_dir {
        spec = spec.env("PROFILE_DIR", dir.as_str());
    }
    for (name, value) in &env.maintainer {
        spec = spec.env(name.as_str(), value.as_str());
    }
    let output = match proc.run(&spec) {
        Ok(output) => output,
        Err(error) => return failed(LoadError::Bash(error.to_string())),
    };
    let code = match output.exit {
        ProcExit::TimedOut => {
            return Evaluation {
                stdout: output.stdout,
                stderr: output.stderr,
                outcome: Err(LoadError::EvalTimedOut),
            };
        }
        ProcExit::Code(code) => code,
    };
    let marker = [b"\0MX-PROFILE-".as_slice(), token.as_bytes(), b"\0"].concat();
    let Some(at) = rfind(&output.stdout, &marker) else {
        // The shell ended before the read-back: the file called `exit`.
        return Evaluation {
            stdout: output.stdout,
            stderr: output.stderr,
            outcome: Err(LoadError::ShellExited { code }),
        };
    };
    let printed = output.stdout[..at].to_vec();
    let outcome = parse_dump(&output.stdout[at + marker.len()..]);
    Evaluation { stdout: printed, stderr: output.stderr, outcome }
}

fn failed(error: LoadError) -> Evaluation {
    Evaluation { stdout: Vec::new(), stderr: Vec::new(), outcome: Err(error) }
}

/// The last position of `needle` in `haystack`.
fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len())
        .rev()
        .find(|at| &haystack[*at..*at + needle.len()] == needle)
}

/// `name NUL value NUL` records, the first of which is `__mx_status`.
fn parse_dump(bytes: &[u8]) -> Result<Dump, LoadError> {
    let mut parts: Vec<&[u8]> = bytes.split(|byte| *byte == 0).collect();
    // The text ends in a NUL, so the last piece is empty.
    if parts.pop() != Some(b"".as_slice()) {
        return Err(LoadError::Malformed("the read-back does not end in a NUL".into()));
    }
    if !parts.len().is_multiple_of(2) {
        return Err(LoadError::Malformed("the read-back has a name without a value".into()));
    }
    let mut vars = BTreeMap::new();
    let mut last_status = None;
    for pair in parts.chunks_exact(2) {
        let name = std::str::from_utf8(pair[0])
            .map_err(|_| LoadError::Malformed("a variable name is not UTF-8".into()))?;
        if name == "__mx_status" {
            last_status = std::str::from_utf8(pair[1]).ok().and_then(|text| text.parse().ok());
            continue;
        }
        if BASH_OWN.contains(&name) {
            continue;
        }
        vars.insert(name.to_string(), pair[1].to_vec());
    }
    let last_status = last_status
        .ok_or_else(|| LoadError::Malformed("the read-back has no status record".into()))?;
    Ok(Dump { vars, last_status })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfind_returns_the_last_occurrence() {
        assert_eq!(rfind(b"abcabc", b"bc"), Some(4));
        assert_eq!(rfind(b"abc", b"abcd"), None);
        assert_eq!(rfind(b"abc", b""), None);
    }

    #[test]
    fn a_dump_reads_back_names_values_and_the_status() {
        let dump = parse_dump(b"__mx_status\x000\0A\0one\0B\0\0PWD\0/x\0_\0y\0").unwrap();
        assert_eq!(dump.last_status, 0);
        assert_eq!(dump.vars.len(), 2, "PWD and _ are bash's own: {:?}", dump.vars);
        assert_eq!(dump.vars["A"], b"one");
        assert_eq!(dump.vars["B"], b"");
    }

    #[test]
    fn a_value_may_hold_newlines_tabs_and_high_bytes() {
        let dump = parse_dump(b"__mx_status\x000\0K\0a\nb\tc\xff\0").unwrap();
        assert_eq!(dump.vars["K"], b"a\nb\tc\xff");
    }

    #[test]
    fn a_dump_that_is_cut_short_is_malformed_and_not_empty() {
        for bytes in [&b"__mx_status\x000\0A\0one"[..], b"__mx_status\x000\0A\0", b"A\0one\0", b""]
        {
            assert!(
                matches!(parse_dump(bytes), Err(LoadError::Malformed(_))),
                "{bytes:?} read as {:?}",
                parse_dump(bytes)
            );
        }
    }
}
