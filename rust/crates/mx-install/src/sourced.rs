//! Reading `profile.env` the way a bash script that sources it would, in a child bash.
//!
//! `profile.env` is shell code, so a child `bash` evaluates it (the same rule as
//! `mx-profile`): nothing here parses shell. The child sees the caller's environment, as the
//! bash scripts that source the file do (decision P4A-env), and answers with the few
//! variables the schedulers read, framed so that anything the profile itself prints stays apart.

use crate::env::Env;
use crate::tools::{self, QUICK, Ran};
use mx_ports::Proc;
use std::path::Path;

const FRAME: &[u8] = b"\0MX-INSTALL-FRAME\0";

/// Run as `bash -c SCRIPT bash FILE MODE`. `cron` is `set -u; . FILE 2>/dev/null` and
/// `launchd` is `set -euo pipefail; . FILE`: the two scripts differ in how they source.
const SCRIPT: &str = r#"f=$1; mode=$2
if [ "$mode" = launchd ]; then set -euo pipefail; . "$f"; st=0; else set -u; . "$f" 2>/dev/null; st=$?; fi
printf '\0MX-INSTALL-FRAME\0'
printf 'S\0%s\0' "$st"
for n in TASKS STATE_DIR REPO_PATH; do
    if [ -n "${!n+x}" ]; then printf 'V\0%s\0%s\0' "$n" "${!n}"; else printf 'U\0%s\0' "$n"; fi
done
if [ -n "${TASKS+x}" ]; then
    for t in $TASKS; do
        v="MIN_HOURS_$t"
        case "$v" in
            *[!A-Za-z0-9_]*|[0-9]*) printf 'M\0%s\0\0' "$t" ;;
            *) printf 'M\0%s\0V%s\0' "$t" "${!v:-0}" ;;
        esac
    done
fi
exit 0
"#;

/// How the script sources the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `. file 2>/dev/null` under `set -u` (install-cron.sh).
    Cron,
    /// `. file` under `set -euo pipefail` (install-launchd.sh).
    Launchd,
}

/// What sourcing the profile left.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sourced {
    /// What the profile printed on standard output, and on standard error.
    pub printed: Vec<u8>,
    pub stderr: Vec<u8>,
    /// The status of the `.` command.
    pub status: i32,
    /// `None` when the variable is unset.
    pub tasks: Option<Vec<u8>>,
    pub state_dir: Option<Vec<u8>>,
    pub repo_path: Option<Vec<u8>>,
    /// Each task of `$TASKS` and its `MIN_HOURS_<task>`: `None` when the name is not a
    /// valid variable name, and `Some(text)` (`0` when unset or empty) otherwise.
    pub minimums: Vec<(Vec<u8>, Option<Vec<u8>>)>,
}

/// The shell ended before it reported: the profile called `exit`, or `set -u` or `set -e`
/// stopped it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stopped {
    pub code: i32,
    pub printed: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Sources `file` and reads what the schedulers need.
pub fn source(proc: &dyn Proc, env: &Env, file: &Path, mode: Mode) -> Result<Sourced, Stopped> {
    let mode_word = match mode {
        Mode::Cron => "cron",
        Mode::Launchd => "launchd",
    };
    let spec = tools::spec(
        env,
        "bash",
        &["-c", SCRIPT, "bash", &file.to_string_lossy(), mode_word],
        QUICK,
    );
    let output = match tools::call(proc, &spec) {
        Ran::Done(output) => output,
        other => {
            return Err(Stopped { code: other.status(), printed: Vec::new(), stderr: Vec::new() });
        }
    };
    let code = tools::Ran::Done(output.clone()).status();
    let Some(at) = rfind(&output.stdout, FRAME) else {
        // `bash -c` ends with 127 where a script ends with 1 on an unbound variable.
        let code = if code == 127 { 1 } else { code };
        return Err(Stopped { code, printed: output.stdout, stderr: output.stderr });
    };
    let mut sourced = Sourced {
        printed: output.stdout[..at].to_vec(),
        stderr: output.stderr,
        ..Sourced::default()
    };
    let tokens: Vec<&[u8]> = output.stdout[at + FRAME.len()..].split(|b| *b == 0).collect();
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i] {
            b"S" if i + 1 < tokens.len() => {
                sourced.status = std::str::from_utf8(tokens[i + 1])
                    .ok()
                    .and_then(|t| t.parse().ok())
                    .unwrap_or(1);
                i += 2;
            }
            b"V" if i + 2 < tokens.len() => {
                let value = Some(tokens[i + 2].to_vec());
                match tokens[i + 1] {
                    b"TASKS" => sourced.tasks = value,
                    b"STATE_DIR" => sourced.state_dir = value,
                    b"REPO_PATH" => sourced.repo_path = value,
                    _ => {}
                }
                i += 3;
            }
            b"U" if i + 1 < tokens.len() => i += 2,
            b"M" if i + 2 < tokens.len() => {
                let value = tokens[i + 2].strip_prefix(b"V").map(<[u8]>::to_vec);
                sourced.minimums.push((tokens[i + 1].to_vec(), value));
                i += 3;
            }
            _ => i += 1,
        }
    }
    Ok(sourced)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len())
        .rev()
        .find(|at| &haystack[*at..*at + needle.len()] == needle)
}
