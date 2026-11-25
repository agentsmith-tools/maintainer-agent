//! `shellcheck-sweep`: lint every shell script of the checkout, one linter process per script, in
//! a memory-capped scope when the host has one.
//!
//! One `shellcheck` over the old single-file `tests/run-tests.sh` peaked near 8 GB (it is now a
//! driver and parts under `tests/suites/`, each below 0.5 GB). Two sweeps at once, or one beside
//! a build, took a laptop to a global out-of-memory kill. So the program waits until the free
//! memory covers the job and refuses (75) when it never does, runs each linter in a `systemd-run`
//! scope with `MemoryMax` and no swap so an overrun kills the linter and nothing else, and runs
//! it at low priority. The flag that reaches the child is part of the contract: the order of the
//! chain `systemd-run ... nice -n 15 shellcheck --severity=warning FILE` is pinned by the cases
//! and by [`linter_argv`].
//!
//! # Decisions
//!
//! - `P4B-4`: a linter that runs past two hours is killed and reads as 124.
//! - `P4B-5`: the linter's two streams arrive separately and are passed on in that order
//!   (standard output, then standard error) once the linter has finished.
//! - `P4B-6`: file names sort by bytes.
//! - `P4B-3`: a child gets the caller's whole environment but `PATH` and `LC_ALL`, as the script's
//!   children do, and every call runs in the checkout, where the script's `cd` left the shell.
//! - `P4B-8`: `MAGENT_SWEEP_WAIT` and `MAGENT_SWEEP_POLL` must be whole numbers of seconds when the
//!   program has to wait. The script loops for ever on `MAGENT_SWEEP_POLL=abc` (the counter never
//!   moves) and on a value with a leading zero (the arithmetic refuses it every time), and fails
//!   `[` with an error line on a non-numeric `MAGENT_SWEEP_WAIT`. Here each refuses with 75.
//!
//! - `P4B-11`: the shell's own message for a number it cannot compare is bash 5.3's
//!   (`[: abc: integer expected`); bash 5.2 and older word it `integer expression expected`.
//!
//! # Found, not fixed
//!
//! A `MAGENT_SWEEP_POLL` with a leading zero (`08`) makes the script's arithmetic fail, which aborts the
//! whole `if` block that holds the memory gate, so the sweep then runs with no memory check at all: it fails
//! open. The port refuses (`P4B-8`) and does not copy that.
//!
//! The file list is `for f in $(git ls-files)`: a name is cut at white space and a name git quotes
//! (a non-ASCII byte, a tab) fails the file test, so neither is linted and nothing says so. A name
//! that starts with a dash is hidden from `head -c 80`, which reads it as an option. All preserved,
//! and pinned by `tests/characterization/checks-sweep.sh`.
//!
//! `P4B-15`: the same unquoted `$(git ls-files)` is also subject to pathname expansion, so a tracked
//! name that holds `*`, `?` or `[` is replaced by the files in the checkout it matches, tracked or
//! not. The port takes each name as it is. The differential records the one visible effect: an
//! untracked `gx.sh` is linted by the script when a tracked file is named `g*.sh`.

use crate::ctx::{Call, Ctx};
use crate::host::Kind;
use crate::shell;
use mx_core::Fact;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// How much of a file the shebang test reads (`head -c 80`).
pub const HEAD_BYTES: usize = 80;

/// The three strings that make a file a shell script when they occur in its first bytes.
const SHEBANGS: [&[u8]; 3] = [b"#!/usr/bin/env bash", b"#!/bin/bash", b"#!/bin/sh"];

/// The settings of a run, as the environment gives them. An unset or empty variable is its default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub need_kb: Vec<u8>,
    pub wait_s: Vec<u8>,
    pub poll_s: Vec<u8>,
    pub cap: String,
    pub meminfo: PathBuf,
    /// `CI` is set and not empty.
    pub ci: bool,
    /// `MAGENT_SWEEP_NO_CAP` is set and not empty.
    pub no_cap: bool,
}

impl Settings {
    /// Reads the settings from the caller's environment.
    #[must_use]
    pub fn from_env(ctx: &Ctx<'_>) -> Self {
        let var = |name: &str, default: &str| -> String {
            match ctx.vars.get(name) {
                Some(value) if !value.is_empty() => value.clone(),
                _ => default.to_string(),
            }
        };
        let set = |name: &str| ctx.vars.get(name).is_some_and(|value| !value.is_empty());
        Self {
            need_kb: var("MAGENT_SWEEP_MIN_AVAIL_KB", "10485760").into_bytes(),
            wait_s: var("MAGENT_SWEEP_WAIT", "300").into_bytes(),
            poll_s: var("MAGENT_SWEEP_POLL", "10").into_bytes(),
            cap: var("MAGENT_SWEEP_MEM_MAX", "18G"),
            meminfo: PathBuf::from(var("MAGENT_MEMINFO", "/proc/meminfo")),
            ci: set("CI"),
            no_cap: set("MAGENT_SWEEP_NO_CAP"),
        }
    }
}

/// `case "$(head -c 80 FILE | tr -d '\0')" in *"#!/usr/bin/env bash"*|*"#!/bin/bash"*|*"#!/bin/sh"*)`.
#[must_use]
pub fn is_script(head: &[u8]) -> bool {
    let text: Vec<u8> = head.iter().copied().filter(|byte| *byte != 0).collect();
    SHEBANGS.iter().any(|shebang| shell::contains(&text, shebang))
}

/// `case "$f" in profiles/*/verify.d/*.sh)`. A `*` in a case pattern matches slashes and the empty
/// string, so `profiles//verify.d/.sh` and `profiles/a/b/verify.d/c/d.sh` both match.
#[must_use]
pub fn is_fragment(name: &[u8]) -> bool {
    let Some(rest) = name.strip_prefix(b"profiles/") else { return false };
    let marker = b"/verify.d/";
    (0..rest.len())
        .any(|at| rest[at..].starts_with(marker) && rest[at + marker.len()..].ends_with(b".sh"))
}

/// The names the sweep lints out of what `git ls-files` printed, as the script's `for` loop and
/// `sort -u` give them: the words of the listing that are regular files here, whose head names a
/// shell or that sit under a `verify.d`, sorted and without duplicates.
#[must_use]
pub fn select(ctx: &Ctx<'_>, root: &Path, listing: &[u8]) -> Vec<Vec<u8>> {
    let mut files: Vec<Vec<u8>> = Vec::new();
    for word in shell::split_words(listing) {
        let path = root.join(OsStr::from_bytes(word));
        if ctx.host().kind(&path) != Kind::File {
            continue;
        }
        // `head -c 80 -x.sh` reads the name as an option and prints nothing.
        let head = if word.starts_with(b"-") {
            Vec::new()
        } else {
            match ctx.host().read_head(&path, HEAD_BYTES) {
                Fact::Known(bytes) => bytes,
                Fact::Absent | Fact::Unreadable(_) => Vec::new(),
            }
        };
        if is_script(&head) {
            files.push(word.to_vec());
        }
        if is_fragment(word) {
            files.push(word.to_vec());
        }
    }
    files.sort();
    files.dedup();
    files
}

/// `avail_kb`: the second word of the first `MemAvailable:` line that ends in a newline. `Some("")`
/// for a line with no second word, `None` when there is no such line. An unterminated last line is
/// not read: `read` fails on it, so the `while read` body never runs for it.
#[must_use]
pub fn mem_available(meminfo: &[u8]) -> Option<Vec<u8>> {
    for line in shell::lines(meminfo, false) {
        let mut words =
            line.split(|byte| *byte == b' ' || *byte == b'\t').filter(|word| !word.is_empty());
        if words.next() == Some(b"MemAvailable:") {
            return Some(words.next().unwrap_or_default().to_vec());
        }
    }
    None
}

/// The program and arguments of one linter run: the scope when there is one, then `nice`, then the
/// linter with its severity flag and exactly one file.
#[must_use]
pub fn linter_argv(runner: &[String], file: &[u8]) -> Vec<String> {
    let mut argv: Vec<String> = runner.to_vec();
    argv.push("shellcheck".to_string());
    argv.push("--severity=warning".to_string());
    argv.push(String::from_utf8_lossy(file).into_owned());
    argv
}

/// The `systemd-run` words for a memory cap: the same for the probe and for every run.
#[must_use]
pub fn scope_words(cap: &str) -> Vec<String> {
    ["systemd-run", "--user", "--scope", "--quiet", "-p"]
        .iter()
        .map(|word| (*word).to_string())
        .chain([format!("MemoryMax={cap}"), "-p".to_string(), "MemorySwapMax=0".to_string()])
        .collect()
}

/// A whole number of seconds: ASCII digits, no leading zero unless it is `0`, at most nine of them.
#[must_use]
pub fn is_whole_seconds(text: &[u8]) -> bool {
    !text.is_empty()
        && text.len() <= 9
        && text.iter().all(u8::is_ascii_digit)
        && (text.len() == 1 || text[0] != b'0')
}

fn bash_test_error(ctx: &mut Ctx<'_>, line: u32, operand: &[u8]) {
    let mut text = ctx.script_name().as_bytes().to_vec();
    text.extend_from_slice(format!(": line {line}: [: ").as_bytes());
    text.extend_from_slice(operand);
    text.extend_from_slice(b": integer expected\n");
    ctx.put_err(&text);
}

const NO_LINTER: [&str; 7] = [
    "shellcheck is not installed, so this gate inspected nothing.",
    "",
    "  Debian/Ubuntu:  sudo apt-get install -y shellcheck",
    "  No sudo:        install the static binary from",
    "                  https://github.com/koalaman/shellcheck/releases into ~/.local/bin",
    "",
    "CI runs it either way, so a push will find what this did not.",
];

/// The memory gate: waits until the free memory covers the job. `Err(code)` when it gives up.
fn wait_for_memory(ctx: &mut Ctx<'_>, settings: &Settings, root: &Path) -> Result<(), i32> {
    if settings.ci {
        return Ok(());
    }
    if !ctx.host().readable(&settings.meminfo) {
        let mut text = b"sweep: ".to_vec();
        text.extend_from_slice(settings.meminfo.as_os_str().as_bytes());
        text.extend_from_slice(b" is not readable here; not checking free memory.\n");
        ctx.put_err(&text);
        return Ok(());
    }
    let mut waited: i64 = 0;
    loop {
        let have = match ctx.host().read(&settings.meminfo) {
            Fact::Known(bytes) => mem_available(&bytes).unwrap_or_default(),
            Fact::Absent | Fact::Unreadable(_) => Vec::new(),
        };
        if have.is_empty() || !have.iter().all(u8::is_ascii_digit) {
            let mut text = b"sweep: no usable MemAvailable in ".to_vec();
            text.extend_from_slice(settings.meminfo.as_os_str().as_bytes());
            text.extend_from_slice(b"; not starting a job that needs ");
            text.extend_from_slice(&settings.need_kb);
            text.extend_from_slice(b" KB.\n");
            ctx.put_err(&text);
            return Err(75);
        }
        // `[ "$have" -ge "$need_kb" ] && break`, line 81.
        match (shell::legal_number(&have), shell::legal_number(&settings.need_kb)) {
            (Some(have_n), Some(need_n)) if have_n >= need_n => return Ok(()),
            (Some(_), Some(_)) => {}
            (None, _) => bash_test_error(ctx, 81, &have),
            (_, None) => bash_test_error(ctx, 81, &settings.need_kb),
        }
        if !is_whole_seconds(&settings.wait_s) || !is_whole_seconds(&settings.poll_s) {
            ctx.err_line(
                "sweep: MAGENT_SWEEP_WAIT and MAGENT_SWEEP_POLL must be whole numbers of seconds.",
            );
            return Err(75);
        }
        let wait = shell::legal_number(&settings.wait_s).unwrap_or(0);
        let poll = shell::legal_number(&settings.poll_s).unwrap_or(0);
        if waited >= wait {
            let mut text = b"sweep: MemAvailable is ".to_vec();
            text.extend_from_slice(&have);
            text.extend_from_slice(b" KB and the linter needs ");
            text.extend_from_slice(&settings.need_kb);
            text.extend_from_slice(format!(" KB; gave up after {waited}s.\n").as_bytes());
            text.extend_from_slice(
                b"sweep: close something heavy and retry, or set MAGENT_SWEEP_MIN_AVAIL_KB.\n",
            );
            ctx.put_err(&text);
            return Err(75);
        }
        let slept = ctx.exec(
            &Call::new([
                "sleep".to_string(),
                String::from_utf8_lossy(&settings.poll_s).into_owned(),
            ])
            .cwd(Some(root))
            .at(88),
        );
        ctx.show_both(&slept);
        waited += poll;
    }
}

/// The program, from its working directory to its exit code. It takes no argument.
pub fn run(ctx: &mut Ctx<'_>) -> i32 {
    // cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/.."
    let given = if ctx.argv0.as_bytes().contains(&b'/') {
        PathBuf::from(&ctx.argv0)
    } else {
        ctx.exe.clone()
    };
    let given = if given.is_absolute() { given } else { ctx.cwd.join(given) };
    let real = ctx.host().real_path(&given).unwrap_or(given);
    let root = shell::logical_parent(
        Path::new(OsStr::from_bytes(&shell::dirname(real.as_os_str().as_bytes()))),
        &ctx.cwd,
    );
    if ctx.host().kind(&root) != Kind::Dir {
        return 1;
    }

    let listed = ctx.exec(&Call::new(["git", "ls-files"]).cwd(Some(&root)).at(12));
    ctx.show_err(&listed);
    let files = select(ctx, &root, &listed.stdout);
    ctx.put_out(format!("checking {} scripts\n", files.len()).as_bytes());
    if files.is_empty() {
        ctx.err_line("no scripts found; this gate inspected nothing");
        return 1;
    }

    if !ctx.on_path("shellcheck") {
        for line in NO_LINTER {
            ctx.err_line(line);
        }
        return 127;
    }

    let settings = Settings::from_env(ctx);
    if let Err(code) = wait_for_memory(ctx, &settings, &root) {
        return code;
    }

    let mut runner: Vec<String> = Vec::new();
    if !settings.ci && !settings.no_cap {
        let scope = scope_words(&settings.cap);
        let mut probe = scope.clone();
        probe.push("true".to_string());
        let usable =
            ctx.on_path("systemd-run") && ctx.exec(&Call::new(probe).cwd(Some(&root)).at(99)).ok();
        if usable {
            runner = scope;
        } else {
            ctx.err_line("sweep: no memory cap (no systemd user scope on this host); relying on the free-memory check.");
        }
    }
    if ctx.on_path("nice") {
        runner.extend(["nice".to_string(), "-n".to_string(), "15".to_string()]);
    }

    // One linter process per script. Without -x it analyses each file on its own.
    let mut rc: i32 = 0;
    for file in &files {
        let done = ctx.exec(&Call::new(linter_argv(&runner, file)).cwd(Some(&root)).at(113));
        ctx.show_both(&done);
        if done.rc == 124 {
            let mut text = b"sweep: the linter did not finish within two hours on ".to_vec();
            text.extend_from_slice(file);
            text.push(b'\n');
            ctx.put_err(&text);
        }
        let r = done.rc;
        if r > rc {
            rc = r;
        }
        if r == 137 {
            let mut text = b"sweep: shellcheck was killed (rc=137) on ".to_vec();
            text.extend_from_slice(file);
            text.extend_from_slice(
                format!(
                    ", most likely by the MemoryMax={} cap. Raise MAGENT_SWEEP_MEM_MAX.\n",
                    settings.cap
                )
                .as_bytes(),
            );
            ctx.put_err(&text);
            break;
        }
    }
    rc
}

/// The words of a path as an `OsString`, for tests.
#[must_use]
pub fn os(text: &str) -> OsString {
    OsString::from_vec(text.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shebang_anywhere_in_the_first_80_bytes_makes_a_script() {
        assert!(is_script(b"#!/usr/bin/env bash\necho hi\n"));
        assert!(is_script(b"#!/bin/bash -eu\n"));
        assert!(is_script(b"#!/bin/sh\n"));
        assert!(is_script(b"# see #!/bin/bash for the rules\n"));
        assert!(is_script(b"#!/bin/shell\n"));
    }

    #[test]
    fn other_interpreters_are_not_scripts() {
        for head in [
            &b"#!/usr/bin/env sh\n"[..],
            b"#!/usr/bin/env zsh\n",
            b"#!/bin/dash\n",
            b"#!/usr/bin/env python3\n",
            b"plain\n",
            b"",
        ] {
            assert!(!is_script(head), "{}", String::from_utf8_lossy(head));
        }
    }

    #[test]
    fn nul_bytes_are_removed_before_the_test_so_they_can_join_two_halves() {
        assert!(is_script(b"\0\0\0#!/bin/bash\n"));
        assert!(is_script(b"#!/usr/bin/env\0 bash\n"));
    }

    #[test]
    fn the_test_sees_only_what_head_gave_it() {
        // The caller passes at most 80 bytes; a shebang cut by the limit is not a shebang.
        let mut text = vec![b'x'; 72];
        text.extend_from_slice(b"#!/bin/sh\n");
        assert!(!is_script(&text[..HEAD_BYTES]));
        let mut edge = vec![b'x'; 71];
        edge.extend_from_slice(b"#!/bin/sh\n");
        assert!(is_script(&edge[..HEAD_BYTES]));
    }

    #[test]
    fn the_verify_d_pattern_matches_across_slashes_and_needs_the_suffix() {
        assert!(is_fragment(b"profiles/p/verify.d/x.sh"));
        assert!(is_fragment(b"profiles/p/q/verify.d/deep/z.sh"));
        assert!(is_fragment(b"profiles//verify.d/.sh"));
        assert!(!is_fragment(b"profiles/verify.d/x.sh"));
        assert!(!is_fragment(b"profiles/p/verify.d/x.bash"));
        assert!(!is_fragment(b"other/verify.d/x.sh"));
        assert!(!is_fragment(b"profiles/p/verify.d/x.sh.bak"));
    }

    #[test]
    fn mem_available_reads_the_second_word_of_the_first_terminated_line() {
        assert_eq!(
            mem_available(b"MemTotal: 1 kB\nMemAvailable:   20000000 kB\n"),
            Some(b"20000000".to_vec())
        );
        assert_eq!(mem_available(b"MemAvailable: 1 kB\nMemAvailable: 2 kB\n"), Some(b"1".to_vec()));
        assert_eq!(mem_available(b"MemTotal: 1 kB\n"), None);
        assert_eq!(mem_available(b"MemAvailable: lots kB\n"), Some(b"lots".to_vec()));
        assert_eq!(mem_available(b"MemAvailable:\n"), Some(Vec::new()));
        assert_eq!(mem_available(b"\tMemAvailable:\t5\tkB\n"), Some(b"5".to_vec()));
    }

    #[test]
    fn an_unterminated_last_line_is_not_read() {
        assert_eq!(mem_available(b"MemTotal: 1 kB\nMemAvailable: 5 kB"), None);
    }

    #[test]
    fn the_scope_words_are_the_probe_and_the_run() {
        assert_eq!(
            scope_words("18G").join(" "),
            "systemd-run --user --scope --quiet -p MemoryMax=18G -p MemorySwapMax=0"
        );
    }

    #[test]
    fn the_linter_runs_through_the_runner_with_one_file_and_the_severity_flag() {
        let runner: Vec<String> =
            scope_words("3G").into_iter().chain(["nice", "-n", "15"].map(String::from)).collect();
        let argv = linter_argv(&runner, b"scripts/a.sh");
        assert_eq!(
            argv.join(" "),
            "systemd-run --user --scope --quiet -p MemoryMax=3G -p MemorySwapMax=0 nice -n 15 shellcheck --severity=warning scripts/a.sh"
        );
        assert_eq!(linter_argv(&[], b"x").join(" "), "shellcheck --severity=warning x");
    }

    #[test]
    fn whole_seconds_refuse_what_the_script_loops_on() {
        for ok in ["0", "1", "10", "300", "999999999"] {
            assert!(is_whole_seconds(ok.as_bytes()), "{ok}");
        }
        for bad in ["", "abc", "08", "-1", "1.5", "1000000000", " 1"] {
            assert!(!is_whole_seconds(bad.as_bytes()), "{bad}");
        }
    }
}
