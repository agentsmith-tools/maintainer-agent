//! The real suite host: every question to a suite is one `bash` process.
//!
//! The bash gate sources `verify.d/<name>.sh` into its own shell and calls the functions
//! the file defines. This host starts `bash` through [`mx_ports::Proc`] for each question,
//! so a suite runs in a process of its own and never inside the gate. Each process runs
//! the same two steps the gate runs in a subshell, `. "$file"` and then the function, under
//! the same `set -uo pipefail` as `bin/maintainer-merge`.
//!
//! The three rules of [`crate::SuiteHost`] hold here:
//!
//! 1. The file and every argument reach bash as positional parameters (`bash -c SCRIPT bash
//!    OP FILE ARG...`). Nothing is spliced into the script text, which is one constant.
//! 2. The file is `<dir>/<name>.sh` for a [`SuiteName`], so it stays in `verify.d/`.
//! 3. A missing file is [`SuiteError::NotOnDisk`] before bash starts.
//!
//! # What a suite can see
//!
//! The environment is the one of the [`Proc`] adapter: `RealProc` clears it and passes
//! only the names its owner allowed. The shipped suites read [`SUITE_ENV_NAMES`], so a
//! caller that builds the adapter with [`mx_ports::RealProc::from_process_env`] puts those
//! names in its allowlist. `work` is the extracted tree: the characterization suite reads
//! it as a variable of its caller (the bash gate defines it as a local of `cmd_verify`),
//! so [`SuiteHost::set_work`] sets it for every later question.

use crate::{
    ImageRef, LimitKey, MutateGlob, Prefetch, Runtime, Suite, SuiteError, SuiteHost, SuiteName,
    SuiteSpec, TestFilter,
};
use mx_ports::{Proc, ProcError, ProcExit, ProcOutput, ProcSpec};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// The variables the shipped `verify.d` suites read, besides the ones every shell has.
/// `work` is not here: it is set by [`SuiteHost::set_work`].
pub const SUITE_ENV_NAMES: [&str; 4] =
    ["HOME", "MAINTAINER_CARGO_JOBS", "MAINTAINER_SHELL_IMAGE", "CHARACTERIZATION_DIR"];

/// How long one question may take. A suite function reads a log or a lockfile and prints a
/// line, so two minutes is generous; the bash has no bound at all, and a hung suite would
/// hold the gate for ever.
const QUESTION_TIMEOUT: Duration = Duration::from_mins(2);

/// `suite_requires` said no.
const EXIT_NEEDS_OTHER: i32 = 77;
/// The suite does not define the function asked for.
const EXIT_UNDEFINED: i32 = 78;
/// The suite does not define `suite_image`.
const EXIT_NO_IMAGE: i32 = 79;

/// The one script every question runs. `$1` is the operation, `$2` the suite file.
const QUESTION: &str = r#"set -uo pipefail
op=$1; file=$2; shift 2
case "$op" in
covers)
    . "$file"; suite_covers "$@" ;;
inferable)
    . "$file" >/dev/null 2>&1; ! declare -F suite_named_only >/dev/null ;;
spec)
    . "$file" >/dev/null
    declare -F suite_image >/dev/null || exit 79
    img="$(suite_image)"
    needs=""; if declare -F suite_needs >/dev/null; then needs="$(suite_needs)"; fi
    globs=""; if declare -F suite_mutate_glob >/dev/null; then globs="$(suite_mutate_glob)"; fi
    named=0; if declare -F suite_named_only >/dev/null; then named=1; fi
    pf=0; pcmd=""
    if declare -F suite_prefetch_command >/dev/null; then
        pf=1; pcmd="$(suite_prefetch_command)"
        if declare -F suite_prefetch_guard >/dev/null; then pf=2; fi
    fi
    printf '%s\0' "$img" "$needs" "$globs" "$named" "$pf" "$pcmd" ;;
command)
    . "$file" >/dev/null
    out="$(suite_command "$1")"; printf '%s' "$out" ;;
ran)
    . "$file" >/dev/null
    out="$(suite_ran "$1")"; printf '%s' "$out" ;;
extra)
    . "$file" >/dev/null
    out=""; if declare -F suite_podman_args >/dev/null; then out="$(suite_podman_args "$1" "${2:-}")"; fi
    printf '%s' "$out" ;;
requires)
    . "$file" >/dev/null
    if declare -F suite_requires >/dev/null && ! suite_requires "$1"; then
        name=""; if declare -F suite_requires_name >/dev/null; then name="$(suite_requires_name)"; fi
        printf '%s' "$name"; exit 77
    fi
    exit 0 ;;
limit)
    . "$file" >/dev/null
    declare -F "$1" >/dev/null || exit 78
    out="$("$1")"; printf '%s' "$out" ;;
guard)
    . "$file" >/dev/null
    out="$(suite_prefetch_guard "$1" 2>&1)"; rc=$?; printf '%s' "$out"; exit "$rc" ;;
*)
    exit 64 ;;
esac"#;

/// Where `suite_dir` looks, in the bash gate's order.
///
/// The three candidates are `$PROFILE_DIR/verify.d`,
/// `$HOME/.local/share/maintainer/profiles/$MAINTAINER_PROFILE/verify.d` and
/// `<here>/../profiles/$MAINTAINER_PROFILE/verify.d`, where `<here>` is the directory
/// of the gate's own program after `readlink -f`. The first that is a directory wins,
/// and it is printed as written (the `..` stays).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuiteDirs {
    /// `$PROFILE_DIR`, empty when unset. An empty value skips the first candidate
    /// (`case "$d" in /verify.d) continue`).
    pub profile_dir: String,
    /// `$HOME`.
    pub home: String,
    /// `$MAINTAINER_PROFILE`, empty when unset.
    pub profile: String,
    /// The directory holding the gate's program, symlinks resolved.
    pub here: PathBuf,
}

impl SuiteDirs {
    /// The candidates, in order, with the first one dropped when `PROFILE_DIR` is empty.
    #[must_use]
    pub fn candidates(&self) -> Vec<PathBuf> {
        let mut all = vec![
            format!("{}/verify.d", self.profile_dir),
            format!("{}/.local/share/maintainer/profiles/{}/verify.d", self.home, self.profile),
            format!("{}/../profiles/{}/verify.d", self.here.display(), self.profile),
        ];
        all.retain(|d| d != "/verify.d");
        all.into_iter().map(PathBuf::from).collect()
    }

    /// `suite_dir`: the first candidate that is a directory.
    pub fn find(&self) -> Result<PathBuf, SuiteError> {
        self.candidates().into_iter().find(|d| d.is_dir()).ok_or(SuiteError::NoSuiteDir)
    }
}

/// What a host and the suites it loaded share: the process port and the `work` variable.
struct Shared {
    proc: Arc<dyn Proc + Send + Sync>,
    work: Mutex<Option<PathBuf>>,
}

impl Shared {
    fn ask(&self, op: &str, file: &Path, args: &[&str]) -> Result<ProcOutput, SuiteError> {
        let mut words: Vec<String> = vec![
            "-c".into(),
            QUESTION.into(),
            "bash".into(),
            op.into(),
            file.display().to_string(),
        ];
        words.extend(args.iter().map(|a| (*a).to_string()));
        let mut spec = ProcSpec::command("bash", words, QUESTION_TIMEOUT);
        if let Some(work) = self.work.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            spec = spec.env("work", work.display().to_string());
        }
        let out = self.proc.run(&spec).map_err(|e: ProcError| SuiteError::Bash {
            reason: format!("{op} on {}: {e}", file.display()),
        })?;
        if out.exit == ProcExit::TimedOut {
            return Err(SuiteError::Bash {
                reason: format!(
                    "{op} on {} ran past {} s",
                    file.display(),
                    QUESTION_TIMEOUT.as_secs()
                ),
            });
        }
        Ok(out)
    }
}

fn code(out: &ProcOutput) -> i32 {
    match out.exit {
        ProcExit::Code(c) => c,
        ProcExit::TimedOut => -1,
    }
}

fn failed(op: &str, file: &Path, out: &ProcOutput) -> SuiteError {
    let err = String::from_utf8_lossy(&out.stderr);
    let tail: Vec<&str> = err.lines().rev().take(3).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    SuiteError::Bash {
        reason: format!("{op} on {} exited {}: {}", file.display(), code(out), tail.join(" / ")),
    }
}

/// What `$(...)` leaves of a command's output: every trailing newline removed.
fn command_substitution(bytes: &[u8], what: &str) -> Result<String, SuiteError> {
    let text = std::str::from_utf8(bytes).map_err(|_| SuiteError::Bash {
        reason: format!("{what} printed bytes that are not UTF-8"),
    })?;
    Ok(text.trim_end_matches('\n').to_string())
}

/// The suites of one profile, asked through `bash`.
pub struct RealSuiteHost {
    shared: Arc<Shared>,
    dirs: SuiteDirs,
}

impl RealSuiteHost {
    /// A host that finds `verify.d` by `dirs` and asks `proc`.
    #[must_use]
    pub fn new(proc: Arc<dyn Proc + Send + Sync>, dirs: SuiteDirs) -> Self {
        Self { shared: Arc::new(Shared { proc, work: Mutex::new(None) }), dirs }
    }

    fn file(&self, name: &SuiteName) -> Result<(PathBuf, PathBuf), SuiteError> {
        let dir = self.dirs.find()?;
        let file = dir.join(name.file_name());
        if file.is_file() {
            Ok((dir, file))
        } else {
            Err(SuiteError::NotOnDisk { name: name.clone(), dir })
        }
    }
}

impl SuiteHost for RealSuiteHost {
    fn dir(&self) -> Result<PathBuf, SuiteError> {
        self.dirs.find()
    }

    fn names(&self) -> Result<Vec<SuiteName>, SuiteError> {
        let dir = self.dirs.find()?;
        let entries = std::fs::read_dir(&dir).map_err(|e| SuiteError::Bash {
            reason: format!("cannot list {}: {e}", dir.display()),
        })?;
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| SuiteError::Bash {
                reason: format!("cannot list {}: {e}", dir.display()),
            })?;
            let file_name = entry.file_name();
            let Some(text) = file_name.to_str() else { continue };
            let Some(stem) = text.strip_suffix(".sh") else { continue };
            // `[ -e ]` in the bash follows links; a directory or a dangling link named
            // `x.sh` sources to an error and covers nothing, so leaving it out is the same.
            if let Ok(name) = SuiteName::parse(stem)
                && entry.path().is_file()
            {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    fn load(&self, name: &SuiteName) -> Result<Box<dyn Suite>, SuiteError> {
        let (_, file) = self.file(name)?;
        let out = self.shared.ask("spec", &file, &[])?;
        match code(&out) {
            0 => {}
            EXIT_NO_IMAGE => {
                return Err(SuiteError::Bash {
                    reason: format!("{} defines no suite_image", file.display()),
                });
            }
            _ => return Err(failed("spec", &file, &out)),
        }
        let spec = parse_spec(name, &file, &out.stdout)?;
        Ok(Box::new(RealSuite { shared: Arc::clone(&self.shared), file, spec }))
    }

    fn covers(&self, name: &SuiteName, path: &str) -> Result<bool, SuiteError> {
        let (_, file) = self.file(name)?;
        Ok(code(&self.shared.ask("covers", &file, &[path])?) == 0)
    }

    fn inferable(&self, name: &SuiteName) -> Result<bool, SuiteError> {
        let (_, file) = self.file(name)?;
        Ok(code(&self.shared.ask("inferable", &file, &[])?) == 0)
    }

    /// Sets the `work` variable every later question sees: the extracted tree.
    fn set_work(&self, work: &Path) {
        *self.shared.work.lock().unwrap_or_else(PoisonError::into_inner) = Some(work.to_path_buf());
    }
}

fn parse_spec(name: &SuiteName, file: &Path, stdout: &[u8]) -> Result<SuiteSpec, SuiteError> {
    let bad = |why: String| SuiteError::Bash { reason: format!("{}: {why}", file.display()) };
    let fields: Vec<&[u8]> = stdout.split(|b| *b == 0).collect();
    // Six fields, each ended by a NUL, so the split ends in one empty piece.
    if fields.len() != 7 || !fields[6].is_empty() {
        return Err(bad(format!(
            "the declarations came back as {} fields, not 6",
            fields.len() - 1
        )));
    }
    let text = |i: usize, what: &str| {
        std::str::from_utf8(fields[i]).map_err(|_| bad(format!("{what} is not UTF-8")))
    };
    let image = ImageRef::parse(text(0, "suite_image")?).map_err(|e| bad(e.to_string()))?;
    let needs = text(1, "suite_needs")?.split_whitespace().map(str::to_string).collect();
    let mutate_globs = text(2, "suite_mutate_glob")?
        .split('\n')
        .filter(|line| !line.is_empty())
        .map(|line| MutateGlob::parse(line).map_err(|e| bad(e.to_string())))
        .collect::<Result<Vec<_>, _>>()?;
    let named_only = text(3, "named_only")? == "1";
    let prefetch = match text(4, "prefetch")? {
        "0" => None,
        flag => Some(Prefetch {
            command: command_substitution(fields[5], "suite_prefetch_command")?,
            guarded: flag == "2",
        }),
    };
    Ok(SuiteSpec { name: name.clone(), image, needs, mutate_globs, named_only, prefetch })
}

/// One suite file, loaded: its declarations were read once, every other question starts a
/// new `bash`.
struct RealSuite {
    shared: Arc<Shared>,
    file: PathBuf,
    spec: SuiteSpec,
}

impl Suite for RealSuite {
    fn spec(&self) -> &SuiteSpec {
        &self.spec
    }

    fn covers(&self, path: &str) -> Result<bool, SuiteError> {
        Ok(code(&self.shared.ask("covers", &self.file, &[path])?) == 0)
    }

    fn command(&self, filter: &TestFilter) -> Result<String, SuiteError> {
        let out = self.shared.ask("command", &self.file, &[filter.as_str()])?;
        command_substitution(&out.stdout, "suite_command")
    }

    fn ran(&self, log: &Path) -> Result<u64, SuiteError> {
        let log = log.display().to_string();
        let out = self.shared.ask("ran", &self.file, &[log.as_str()])?;
        let printed = command_substitution(&out.stdout, "suite_ran").unwrap_or_default();
        Ok(whole_number(&printed))
    }

    fn extra_args(&self, runtime: Runtime, scratch: &Path) -> Result<String, SuiteError> {
        let scratch = scratch.display().to_string();
        let out = self.shared.ask("extra", &self.file, &[runtime.as_str(), scratch.as_str()])?;
        command_substitution(&out.stdout, "suite_podman_args")
    }

    fn requires(&self, runtime: Runtime) -> Result<(), SuiteError> {
        let out = self.shared.ask("requires", &self.file, &[runtime.as_str()])?;
        match code(&out) {
            0 => Ok(()),
            EXIT_NEEDS_OTHER => Err(SuiteError::NeedsOtherRuntime {
                suite: self.spec.name.clone(),
                needs: command_substitution(&out.stdout, "suite_requires_name")?,
                runtime,
            }),
            _ => Err(failed("requires", &self.file, &out)),
        }
    }

    fn limit(&self, key: LimitKey) -> Result<Option<String>, SuiteError> {
        let out = self.shared.ask("limit", &self.file, &[key.suite_function()])?;
        match code(&out) {
            0 => command_substitution(&out.stdout, key.suite_function()).map(Some),
            EXIT_UNDEFINED => Ok(None),
            _ => Err(failed("limit", &self.file, &out)),
        }
    }

    fn prefetch_guard(&self, tree: &Path) -> Result<(), SuiteError> {
        let Some(Prefetch { guarded: true, .. }) = self.spec.prefetch else {
            return Ok(());
        };
        let tree = tree.display().to_string();
        let out = self.shared.ask("guard", &self.file, &[tree.as_str()])?;
        if code(&out) == 0 {
            return Ok(());
        }
        Err(SuiteError::PrefetchRefused {
            output: command_substitution(&out.stdout, "suite_prefetch_guard")?,
        })
    }
}

/// What `[ "${ran:-0}" -ge 1 ]` reads of `suite_ran`'s output. A whole number counts as itself;
/// text that is not one, or a number beyond `i64` (the range of the shell's `test`), counts as
/// 0, so the gate refuses it as "no evidence it ran anything".
fn whole_number(printed: &str) -> u64 {
    let text = printed.trim_matches([' ', '\t']);
    let digits = text.strip_prefix('+').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return 0;
    }
    digits.parse::<i64>().map_or(0, i64::unsigned_abs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_number_counts_as_itself_and_nothing_else_does() {
        assert_eq!(whole_number("12"), 12);
        assert_eq!(whole_number("0"), 0);
        assert_eq!(whole_number("007"), 7);
        assert_eq!(whole_number(" 5 "), 5);
        assert_eq!(whole_number("+5"), 5);
        assert_eq!(whole_number("9223372036854775807"), i64::MAX as u64);
        for not in
            ["", "x", "1.5", "-3", "12 13", "1e3", "99999999999999999999999", "9223372036854775808"]
        {
            assert_eq!(whole_number(not), 0, "{not:?}");
        }
    }

    #[test]
    fn suite_dir_candidates_follow_the_bash_order_and_skip_an_empty_profile_dir() {
        let dirs = SuiteDirs {
            profile_dir: String::new(),
            home: "/h".into(),
            profile: "sysknife".into(),
            here: PathBuf::from("/opt/magent/bin"),
        };
        assert_eq!(
            dirs.candidates(),
            [
                PathBuf::from("/h/.local/share/maintainer/profiles/sysknife/verify.d"),
                PathBuf::from("/opt/magent/bin/../profiles/sysknife/verify.d"),
            ]
        );
        let with = SuiteDirs { profile_dir: "/p".into(), ..dirs };
        assert_eq!(with.candidates()[0], PathBuf::from("/p/verify.d"));
        assert_eq!(with.candidates().len(), 3);
    }

    fn dir_with_suite() -> (PathBuf, SuiteDirs) {
        let dir = std::env::temp_dir().join(format!("mx-suite-fake-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(dir.join("verify.d")).unwrap();
        std::fs::write(dir.join("verify.d/x.sh"), "").unwrap();
        let dirs = SuiteDirs {
            profile_dir: dir.display().to_string(),
            home: "/h".into(),
            profile: String::new(),
            here: PathBuf::from("/nowhere"),
        };
        (dir, dirs)
    }

    fn covers_argv(dir: &Path, path: &str) -> Vec<String> {
        let file = dir.join("verify.d/x.sh").display().to_string();
        ["bash", "-c", QUESTION, "bash", "covers", file.as_str(), path]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }

    #[test]
    fn a_question_that_times_out_or_cannot_start_is_an_error_and_not_a_no() {
        use mx_ports::{EffectLog, FakeProc, Script};
        let (dir, dirs) = dir_with_suite();
        let log = EffectLog::new();
        let fake = FakeProc::new(&log);
        let argv = covers_argv(&dir, "a");
        let words: Vec<&str> = argv.iter().map(String::as_str).collect();
        fake.script(
            &words,
            Script::sequence([
                Ok(ProcOutput { exit: ProcExit::TimedOut, stdout: Vec::new(), stderr: Vec::new() }),
                Err(ProcError::NotFound { program: "bash".into() }),
                Ok(FakeProc::exited(1, "")),
            ]),
        );
        let host = RealSuiteHost::new(Arc::new(fake), dirs);
        let name = SuiteName::parse("x").unwrap();
        let timed_out = host.covers(&name, "a").unwrap_err().to_string();
        assert!(timed_out.contains("ran past 120 s"), "{timed_out}");
        let missing = host.covers(&name, "a").unwrap_err().to_string();
        assert!(missing.contains("not found on the search path"), "{missing}");
        assert_eq!(host.covers(&name, "a"), Ok(false), "a plain status 1 is the answer no");
        drop(std::fs::remove_dir_all(dir));
    }

    #[test]
    fn the_question_script_names_every_operation_the_host_asks() {
        for op in
            ["covers", "inferable", "spec", "command", "ran", "extra", "requires", "limit", "guard"]
        {
            assert!(QUESTION.contains(&format!("\n{op})")), "{op}");
        }
    }
}
