//! `check-claims`: hold the documents' numbers to what the tree contains.
//!
//! The program runs the suite once, reads the count off the suite's summary line, generates the
//! deny wall of every profile, and hands the count to `scripts/render_figures.py`, which holds
//! the figures. It compares nothing itself, so there is one counting code path.
//!
//! The two counts are read by [`passed_count`] and [`skipped_count`], the Rust spelling of two
//! `sed` expressions: each takes the last line of the output that matches. The python snippets
//! that count the rules of a wall are run through `python3` with the text the script gives them
//! (decision `P4B-1`), so the call log is the script's.
//!
//! # Decisions
//!
//! - `P4B-1`: the inline `python3 -c` programs run as the script runs them.
//! - `P4B-5`: a captured `2>&1` reads standard output and then standard error.
//! - `P4B-4`: a suite that runs past two hours is killed and reads as red, with one extra line.
//! - `P4B-6`: profiles are listed in byte order. The script's glob sorts by the locale, which
//!   differs only outside `C`.
//!
//! # Found, not fixed
//!
//! - The second `trap ... EXIT` replaces the first, so the file that holds the suite's output
//!   stays in `TMPDIR` after every run that gets past the suite. Preserved and pinned.
//! - The combined rule count reads `profiles/sysknife/deny.json` by name. Without it the line
//!   carries no number and the run still passes. Preserved and pinned.
//! - With no profile the glob stays literal and the program renders a profile named `*`.
//! - A count with a leading zero is octal in the script's arithmetic and decimal here, and a
//!   `mktemp` that fails ends the script in a cascade of shell messages and ends this program
//!   with one line.

use crate::ctx::{Call, Ctx};
use crate::shell;
use mx_core::Fact;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

/// How long the suite may run before it is killed and read as red (decision `P4B-4`).
pub const SUITE_TIMEOUT: Duration = Duration::from_hours(2);

/// The home the renderer is given, so the walls it makes name no real user.
const HOME: &str = "/home/claimcheck";

/// `sed -n 's/^ *\([0-9]\+\) passed.*/\1/p' | tail -1`: the digits before ` passed` on the last
/// line that starts with spaces, digits and ` passed`.
#[must_use]
pub fn passed_count(output: &[u8]) -> Option<Vec<u8>> {
    let mut found = None;
    for line in shell::lines(output, true) {
        let mut at = 0;
        while line.get(at) == Some(&b' ') {
            at += 1;
        }
        let digits = at;
        while line.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if at > digits && line[at..].starts_with(b" passed") {
            found = Some(line[digits..at].to_vec());
        }
    }
    found
}

/// `sed -n 's/.*, \([0-9]\+\) skipped.*/\1/p' | tail -1`: on each line the last `, N skipped`, and
/// of those lines the last one.
#[must_use]
pub fn skipped_count(output: &[u8]) -> Option<Vec<u8>> {
    let mut found = None;
    for line in shell::lines(output, true) {
        let mut best: Option<Vec<u8>> = None;
        let mut at = 0;
        while at + 2 <= line.len() {
            if line[at..].starts_with(b", ") {
                let digits = at + 2;
                let mut end = digits;
                while line.get(end).is_some_and(u8::is_ascii_digit) {
                    end += 1;
                }
                if end > digits && line[end..].starts_with(b" skipped") {
                    best = Some(line[digits..end].to_vec());
                }
            }
            at += 1;
        }
        if best.is_some() {
            found = best;
        }
    }
    found
}

/// A string of digits as the shell's arithmetic reads it, here always as decimal and saturating.
fn number(digits: &[u8]) -> i64 {
    let mut value: i64 = 0;
    for digit in digits {
        value = value.saturating_mul(10).saturating_add(i64::from(digit - b'0'));
    }
    value
}

/// `grep -E '^  FAIL' FILE | head -10`.
#[must_use]
pub fn failing_cases(output: &[u8]) -> Vec<&[u8]> {
    shell::lines(output, true)
        .into_iter()
        .filter(|line| line.starts_with(b"  FAIL"))
        .take(10)
        .collect()
}

/// `paste -sd, <<<"$vals" | sed 's/,/, /g'`: the lines joined with a comma, and every comma
/// then followed by a space.
#[must_use]
pub fn join_measured(values: &[u8]) -> Vec<u8> {
    let lines: Vec<&[u8]> = shell::lines(values, true);
    let joined = lines.join(&b","[..]);
    let mut out = Vec::new();
    for byte in joined {
        out.push(byte);
        if byte == b',' {
            out.push(b' ');
        }
    }
    out
}

/// `bad`: `FAIL: MESSAGE` on standard error, and the run will fail.
fn bad(ctx: &mut Ctx<'_>, fail: &mut bool, message: &str) {
    ctx.put_err(format!("FAIL: {message}\n").as_bytes());
    *fail = true;
}

fn path_text(path: &Path) -> String {
    String::from_utf8_lossy(path.as_os_str().as_bytes()).into_owned()
}

/// Runs the suite and reads its size off its summary line. `Err(1)` when the suite is red or its
/// summary cannot be read; the message is already printed and the output file removed.
fn measure_suite(ctx: &mut Ctx<'_>, root: &Path, tmp: &Path, fail: &mut bool) -> Result<i64, i32> {
    let suite_out = match ctx.host().make_temp_file(tmp) {
        Ok(path) => path,
        Err(error) => {
            ctx.err_line(&format!(
                "check_claims: cannot make a temporary file under {}: {}",
                tmp.display(),
                error.reason
            ));
            return Err(1);
        }
    };
    let armed_file = crate::cleanup::Armed::new(&suite_out);
    let suite = ctx.exec(
        &Call::new(["bash".to_string(), path_text(&root.join("tests/run-tests.sh"))])
            .timeout(SUITE_TIMEOUT),
    );
    let mut captured = suite.stdout.clone();
    captured.extend_from_slice(&suite.stderr);
    if !suite.started {
        // `bash` itself was not found: the redirect still created the file, and the shell
        // reported 127.
        captured.clear();
    }
    drop(ctx.host().write(&suite_out, &captured));
    if !suite.ok() {
        bad(ctx, fail, "the test suite is RED; fix that before believing any number here");
        if suite.rc == 124 {
            ctx.err_line(&format!(
                "check_claims: the suite did not finish within {} seconds",
                SUITE_TIMEOUT.as_secs()
            ));
        }
        for line in failing_cases(&captured) {
            let mut shown = line.to_vec();
            shown.push(b'\n');
            ctx.put_err(&shown);
        }
        ctx.host().remove_file(&suite_out);
        return Err(1);
    }
    let Some(passed) = passed_count(&captured) else {
        bad(ctx, fail, "could not measure the test count; refusing to pass over nothing");
        ctx.host().remove_file(&suite_out);
        return Err(1);
    };
    let skipped = skipped_count(&captured);
    if let Some(skipped) = &skipped {
        let mut line = b"  counting ".to_vec();
        line.extend_from_slice(&passed);
        line.extend_from_slice(b" passed + ");
        line.extend_from_slice(skipped);
        line.extend_from_slice(b" skipped as the suite size\n");
        ctx.put_out(&line);
    }
    // From here the first trap is gone: the suite's output file stays (see "Found, not fixed").
    drop(armed_file);
    Ok(number(&passed).saturating_add(skipped.as_deref().map_or(0, number)))
}

/// Every figure in every document, through the one renderer, which is handed the count measured
/// above instead of running the suite a second time.
fn check_figures(ctx: &mut Ctx<'_>, root: &Path, actual: i64, fail: &mut bool) {
    let check = Call::new([
        "python3".to_string(),
        path_text(&root.join("scripts/render_figures.py")),
        "--root".to_string(),
        path_text(root),
        "--check".to_string(),
        "--tests".to_string(),
        actual.to_string(),
    ]);
    let checked = ctx.exec(&check);
    let report = ctx.substitution(&checked.merged(), 65);
    if !checked.ok() {
        let mut report = report;
        report.push(b'\n');
        ctx.put_err(&report);
        bad(
            ctx,
            fail,
            "a figure in the documents is stale, typed outside a marker, or unreadable; run scripts/render_figures.py --write",
        );
        return;
    }
    let mut line = b"  ".to_vec();
    line.extend_from_slice(&report);
    line.push(b'\n');
    ctx.put_out(&line);
    let mut values = check.clone();
    values.argv[4] = "--values".to_string();
    let printed = ctx.exec(&values);
    let shown = ctx.substitution(&printed.merged(), 71);
    if printed.ok() {
        let mut line = b"  measured: ".to_vec();
        line.extend_from_slice(&join_measured(&shown));
        line.push(b'\n');
        ctx.put_out(&line);
    } else {
        let mut report = shown;
        report.push(b'\n');
        ctx.put_err(&report);
        bad(ctx, fail, "could not print the measured figures");
    }
}

/// The program, from its working directory to its exit code. It takes no argument.
pub fn run(ctx: &mut Ctx<'_>) -> i32 {
    let root = ctx.tree_root();
    let tmp = ctx.tmpdir();
    let mut fail = false;
    // Measured, not asserted. The suite's STATUS is read before its output.
    let actual = match measure_suite(ctx, &root, &tmp, &mut fail) {
        Ok(count) => count,
        Err(code) => return code,
    };
    // Generated, then counted.
    let gen_dir = match ctx.host().make_temp_dir(&tmp) {
        Ok(path) => path,
        Err(error) => {
            ctx.err_line(&format!(
                "check_claims: cannot make a temporary directory under {}: {}",
                tmp.display(),
                error.reason
            ));
            return 1;
        }
    };
    let armed_dir = crate::cleanup::Armed::new(&gen_dir);
    render_walls(ctx, &root, &gen_dir, &mut fail);
    check_figures(ctx, &root, actual, &mut fail);
    ctx.host().remove_tree(&gen_dir);
    drop(armed_dir);
    i32::from(fail)
}

/// Every profile renders, then the verbs of the sysknife profile render together.
fn render_walls(ctx: &mut Ctx<'_>, root: &Path, gen_dir: &Path, fail: &mut bool) {
    let profiles = root.join("profiles");
    let mut names: Vec<Vec<u8>> = Vec::new();
    if let Fact::Known(entries) = ctx.host().list(&profiles) {
        for entry in entries {
            let name = entry.as_bytes();
            if name.first() != Some(&b'.')
                && ctx.host().lexists(&profiles.join(&entry).join("deny.json"))
            {
                names.push(name.to_vec());
            }
        }
    }
    let literal = names.is_empty();
    if literal {
        names.push(b"*".to_vec());
    }
    let script = path_text(&root.join("scripts/render-settings.py"));
    for name in names {
        let shown = String::from_utf8_lossy(&name).into_owned();
        let deny = if literal {
            profiles.join("*").join("deny.json")
        } else {
            profiles.join(OsStr::from_bytes(&name)).join("deny.json")
        };
        let target = gen_dir.join(OsStr::from_bytes(&name));
        drop(ctx.host().make_dirs(&target));
        if let Err(error) = ctx.host().copy_into_dir(&deny, &target) {
            let mut text = error.message(&deny, &target);
            text.push(b'\n');
            ctx.put_err(&text);
        }
        let rendered = ctx.exec(&Call::new([
            "python3".to_string(),
            script.clone(),
            path_text(&target),
            HOME.to_string(),
        ]));
        if !rendered.ok() {
            bad(ctx, fail, &format!("profile '{shown}' does not generate a deny wall"));
        }
        let count = ctx.exec(&Call::new([
            "python3".to_string(),
            "-c".to_string(),
            format!(
                "\nimport json;print(len(json.load(open('{}/settings.json'))['permissions']['deny']))",
                path_text(&target)
            ),
        ]));
        let mut line = format!("  profile {shown}: ").into_bytes();
        line.extend_from_slice(&shell::command_substitution(&count.stdout));
        line.extend_from_slice(b" rules\n");
        ctx.put_out(&line);
    }
    // The README states the verb count, which `render_figures.py` derives; the rule count is
    // printed as an observation.
    drop(ctx.host().copy_into_dir(&profiles.join("sysknife/deny.json"), gen_dir));
    drop(ctx.exec(&Call::new([
        "python3".to_string(),
        script,
        path_text(gen_dir),
        HOME.to_string(),
    ])));
    let here = ctx.exec(&Call::new([
        "python3".to_string(),
        "-c".to_string(),
        format!(
            "\nimport json\nprint(len(json.load(open('{}/settings.json'))['permissions']['deny']))",
            path_text(gen_dir)
        ),
    ]));
    let mut line = b"  those verbs generate ".to_vec();
    line.extend_from_slice(&shell::command_substitution(&here.stdout));
    line.extend_from_slice(b" rules on this machine\n");
    ctx.put_out(&line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passed_count_takes_the_last_matching_line_and_only_at_the_start() {
        let out = b"  8 passed, 0 failed\nall 12 passed\n   40 passed in a nested run\nx 99 passed";
        assert_eq!(passed_count(out), Some(b"40".to_vec()));
        assert_eq!(passed_count(b"all 12 passed\n"), None);
        assert_eq!(passed_count(b"12passed\n"), None);
        assert_eq!(passed_count(b""), None);
    }

    #[test]
    fn only_the_whole_word_passed_makes_a_summary_and_only_the_whole_word_skipped_a_skip_count() {
        assert_eq!(passed_count(b"  8 pass, 3 failed\n"), None);
        assert_eq!(passed_count(b"  8 passing\n"), None);
        assert_eq!(skipped_count(b"x, 5 skip\n"), None);
    }

    #[test]
    fn passed_count_reads_a_final_line_with_no_newline() {
        assert_eq!(passed_count(b"  7 passed"), Some(b"7".to_vec()));
    }

    #[test]
    fn skipped_count_takes_the_last_match_on_each_line_and_the_last_such_line() {
        let out = b"  10 passed, 0 failed, 3 skipped\nnote: 99 passed in the sub-run, 4 skipped\n";
        assert_eq!(skipped_count(out), Some(b"4".to_vec()));
        assert_eq!(skipped_count(b"a, 1 skipped, 2 skipped\n"), Some(b"2".to_vec()));
        assert_eq!(skipped_count(b"3 skipped\n"), None);
        assert_eq!(skipped_count(b", skipped\n"), None);
        assert_eq!(skipped_count(b"x, 5 skippedy\n"), Some(b"5".to_vec()));
    }

    #[test]
    fn failing_cases_are_the_first_ten_lines_that_start_with_two_spaces_and_fail() {
        let mut out = Vec::new();
        for i in 1..=14 {
            out.extend_from_slice(format!("  FAIL case {i}\n").as_bytes());
        }
        out.extend_from_slice(b"    FAIL indented\nFAIL bare\n");
        let got = failing_cases(&out);
        assert_eq!(got.len(), 10);
        assert_eq!(got[9], b"  FAIL case 10");
        assert!(failing_cases(b"    FAIL indented\n").is_empty());
    }

    #[test]
    fn join_measured_joins_with_commas_and_widens_every_comma() {
        assert_eq!(join_measured(b"tests 10\nevals 9"), b"tests 10, evals 9");
        assert_eq!(join_measured(b"tests 1,800\nbackends 4"), b"tests 1, 800, backends 4");
        assert_eq!(join_measured(b""), b"");
        assert_eq!(join_measured(b"a"), b"a");
    }

    #[test]
    fn number_reads_digits_and_saturates() {
        assert_eq!(number(b"0"), 0);
        assert_eq!(number(b"1650"), 1650);
        assert_eq!(number(b"99999999999999999999999"), i64::MAX);
    }
}
