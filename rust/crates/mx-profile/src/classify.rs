//! The failure classifier: `maintainer_classify_failure`, `backend_transient_reason`,
//! `backend_needs_human_reason` and `maintainer_failure_shapes_path`.
//!
//! # Regex engine: grep, called through the `Proc` port
//!
//! The bash matches with `grep -aoiE`, which is POSIX leftmost-longest. The Rust
//! `regex` crate is leftmost-first, so `a|ab` on `ab` gives `a` there and `ab`
//! here. This crate does not use an in-process engine. It runs the same `grep`
//! with the pattern as one argument after `--`, so a shape matches exactly what
//! it matches today, including the cases where the table's pattern is not valid
//! ERE (grep prints an error, the bash hides it, and the shape matches
//! nothing). A table's `(?!x)x` is one of those. Plan decision F8 unifies the two
//! classifiers on one engine only after both outputs are pinned equal on the
//! shipped table, and this crate is the pin.
//!
//! The Proc port runs every child under `LC_ALL=C`. The bash runs grep under the
//! caller's locale, so `.{0,60}` counts bytes here and characters there when a
//! UTF-8 log holds multibyte text, and `-i` folds only ASCII. Every shipped
//! pattern is ASCII. This is decision P1F-5 in the pull request.
//!
//! # What the bash pipeline does that this port keeps
//!
//! - `grep ... 2>/dev/null | tail -1` reports the status of `tail`. A grep that
//!   exits 2 (an invalid pattern, an unreadable log) but printed a match still
//!   gives that match, and one that printed nothing gives "no match".
//! - `$(...)` drops NUL bytes and trailing newlines, so a match that spans a NUL
//!   loses it.
//! - The fallback `grep -aiE ... "$log"` has no `--`, so a relative log named
//!   like an option is read as an option.
//! - `cut -f3` in the two predicates cuts the matched text at its first tab.
//! - `case "$out" in transient*)` matches any class that starts with
//!   `transient`, and `needs_human*` likewise.
//!
//! # What differs from the bash, on purpose
//!
//! A `grep` or `tail` that cannot start, or that runs longer than
//! [`GREP_TIMEOUT`], is `Unreadable` (return code 2, "could not ask"). The bash
//! reads a missing grep as "nothing matched" and a hung one never returns.
//! Decision P1F-6.

use crate::table::{TableFault, parse_shapes, read_rows};
use mx_ports::{Proc, ProcExit, ProcSpec};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long one `grep` or `tail` may run. The bash has no limit.
pub const GREP_TIMEOUT: Duration = Duration::from_mins(2);

/// What the bash function reads from its surroundings. A library reads no
/// environment, so the caller says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifyEnv {
    /// `MAINTAINER_FAILURE_SHAPES`. Unset and empty both mean "use the search".
    pub failure_shapes: Option<OsString>,
    /// The directory holding `lib/profile.sh`, which is where the shipped table sits.
    pub lib_dir: PathBuf,
    /// `$HOME`, empty when unset.
    pub home: OsString,
}

/// The three outcomes and the fourth, "could not ask".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// A shape matched. Return code 0.
    Matched { class: String, id: String, text: Vec<u8> },
    /// Nothing matched. `evidence` is the most error-shaped line. Return code 1.
    Unclassified { evidence: Vec<u8> },
    /// The classifier could not read the log or the table. Return code 2.
    Unreadable { reason: Vec<u8> },
}

/// The words that make a line "error-shaped", for the fallback when no shape matched.
const ERROR_WORDS: &str = "error|fail|panic|refus|denied|cannot|could not";

impl Classification {
    /// The bash return code: 0 matched, 1 unclassified, 2 could not ask.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Matched { .. } => 0,
            Self::Unclassified { .. } => 1,
            Self::Unreadable { .. } => 2,
        }
    }

    /// The bytes the bash function prints: `class TAB id TAB text`, with no
    /// trailing newline.
    #[must_use]
    pub fn render(&self) -> Vec<u8> {
        let (class, id, text): (&[u8], &[u8], &[u8]) = match self {
            Self::Matched { class, id, text } => (class.as_bytes(), id.as_bytes(), text),
            Self::Unclassified { evidence } => (b"unclassified", b"", evidence),
            Self::Unreadable { reason } => (b"unreadable", b"", reason),
        };
        [class, b"\t", id, b"\t", text].concat()
    }

    /// `backend_transient_reason`: the matched text of a class that starts with
    /// `transient`, cut at its first tab. `None` is the bash's return code 1.
    #[must_use]
    pub fn transient_reason(&self) -> Option<Vec<u8>> {
        self.reason_for("transient")
    }

    /// `backend_needs_human_reason`: the same for a class that starts with
    /// `needs_human`.
    #[must_use]
    pub fn needs_human_reason(&self) -> Option<Vec<u8>> {
        self.reason_for("needs_human")
    }

    /// `out="$(classify)" || return 1`, then `case "$out" in <prefix>*)`, then
    /// `cut -f3`.
    fn reason_for(&self, prefix: &str) -> Option<Vec<u8>> {
        if !matches!(self, Self::Matched { .. }) {
            return None;
        }
        let out = self.render();
        if !out.starts_with(prefix.as_bytes()) {
            return None;
        }
        out.split(|byte| *byte == b'\t').nth(2).map(<[u8]>::to_vec)
    }
}

/// `maintainer_failure_shapes_path`: the override, else the table beside the
/// library, else the installed one. `None` when none of them is a file.
///
/// An override is used as given, whether or not it exists: the bash prints it
/// and the table read then fails, which is "could not be parsed".
#[must_use]
pub fn failure_shapes_path(env: &ClassifyEnv) -> Option<PathBuf> {
    if let Some(path) = env.failure_shapes.as_ref().filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    // The bash joins with a plain string, `"$here/failure-shapes.json"`, so a
    // trailing slash in `home` stays, and an empty `home` gives `/.local/...`.
    search_candidates(env).into_iter().find(|candidate| is_file(candidate))
}

/// The two places the table is looked for, in order.
fn search_candidates(env: &ClassifyEnv) -> [PathBuf; 2] {
    [
        concat_path(env.lib_dir.as_os_str(), "/failure-shapes.json"),
        concat_path(&env.home, "/.local/share/maintainer/failure-shapes.json"),
    ]
}

/// `"$base$tail"` as a path, without normalising anything.
fn concat_path(base: &OsStr, tail: &str) -> PathBuf {
    let mut joined = base.to_os_string();
    joined.push(tail);
    PathBuf::from(joined)
}

/// `[ -f path ]`: a regular file, following symlinks.
fn is_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

/// `[ -n "$log" ] && [ -r "$log" ]`.
fn is_readable(path: &Path) -> bool {
    !path.as_os_str().is_empty() && std::fs::File::open(path).is_ok()
}

/// `grep ... | tail -1` inside `$(...)`: the last line, NUL bytes dropped and
/// trailing newlines removed. An empty answer means "nothing".
fn last_line(output: &[u8]) -> Vec<u8> {
    let body = output.strip_suffix(b"\n").unwrap_or(output);
    let last = body.rsplit(|byte| *byte == b'\n').next().unwrap_or_default();
    last.iter().copied().filter(|byte| *byte != 0).collect()
}

/// Runs `program args...` and returns what it wrote to standard output,
/// whatever its exit status, as the bash does. `Err` is why it could not finish.
fn run_for_stdout(proc: &dyn Proc, program: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let spec = ProcSpec::command(program, args.iter().copied(), GREP_TIMEOUT);
    match proc.run(&spec) {
        Ok(output) if output.exit == ProcExit::TimedOut => {
            Err(format!("{program} did not finish within {} s", GREP_TIMEOUT.as_secs()))
        }
        Ok(output) => Ok(output.stdout),
        Err(error) => Err(format!("{program} could not run: {error}")),
    }
}

fn unreadable(text: &str) -> Classification {
    Classification::Unreadable { reason: text.as_bytes().to_vec() }
}

fn unreadable_bytes(parts: &[&[u8]]) -> Classification {
    Classification::Unreadable { reason: parts.concat() }
}

/// `maintainer_classify_failure`.
#[must_use]
pub fn classify_failure(proc: &dyn Proc, env: &ClassifyEnv, log: &Path) -> Classification {
    if !is_readable(log) {
        return unreadable_bytes(&[b"no readable log at '", log.as_os_str().as_bytes(), b"'"]);
    }
    let Some(table) = failure_shapes_path(env) else {
        return unreadable("no failure-shapes table on this host");
    };
    let table_bytes = table.as_os_str().as_bytes();
    let shapes = match std::fs::read(&table) {
        Ok(bytes) => parse_shapes(&bytes),
        Err(_) => Err(TableFault::NotParsable),
    };
    let shapes = match shapes {
        Ok(shapes) => shapes,
        Err(TableFault::NotParsable) => {
            return unreadable_bytes(&[
                b"failure-shapes table at ",
                table_bytes,
                b" could not be parsed",
            ]);
        }
        Err(TableFault::NoShapes) => {
            return unreadable_bytes(&[
                b"failure-shapes table at ",
                table_bytes,
                b" lists no shapes",
            ]);
        }
    };
    // The Proc port takes text arguments, so a log path that is not UTF-8 cannot
    // be handed to grep. The bash could. Decision P1F-6: say so, do not guess.
    let Some(log_arg) = log.to_str() else {
        return unreadable("the log path is not UTF-8 text, so grep cannot be given it");
    };

    for row in read_rows(&shapes) {
        if row.id.is_empty() {
            continue;
        }
        let output =
            match run_for_stdout(proc, "grep", &["-aoiE", "--", row.pattern.as_str(), log_arg]) {
                Ok(output) => output,
                Err(reason) => return unreadable(&reason),
            };
        let hit = last_line(&output);
        if hit.is_empty() {
            continue;
        }
        return Classification::Matched { class: row.class, id: row.id, text: hit };
    }

    // Nothing matched. The most error-shaped line, else the last line of the log.
    let output = match run_for_stdout(proc, "grep", &["-aiE", ERROR_WORDS, log_arg]) {
        Ok(output) => output,
        Err(reason) => return unreadable(&reason),
    };
    let mut evidence = last_line(&output);
    if evidence.is_empty() {
        evidence = match run_for_stdout(proc, "tail", &["-1", log_arg]) {
            Ok(output) => last_line(&output),
            Err(reason) => return unreadable(&reason),
        };
    }
    if evidence.is_empty() {
        evidence = b"the log says nothing".to_vec();
    }
    Classification::Unclassified { evidence }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Scratch;
    use mx_ports::{
        Effect, EffectLog, FakeProc, ProcEffect, ProcError, ProcExit, ProcOutput, Script,
    };
    use std::time::Duration as Dur;

    fn env_with(table: &Path) -> ClassifyEnv {
        ClassifyEnv {
            failure_shapes: Some(table.as_os_str().to_owned()),
            lib_dir: PathBuf::from("/nonexistent/lib"),
            home: OsString::from("/nonexistent/home"),
        }
    }

    const TABLE_TWO: &str = r#"{"shapes": [
        {"id": "first", "class": "transient", "pattern": "boom"},
        {"id": "second", "class": "needs_human", "pattern": "bang"}]}"#;

    fn out(code: i32, stdout: &str) -> ProcOutput {
        FakeProc::exited(code, stdout)
    }

    /// What a scripted call answers: an output, or an error.
    trait Answer {
        fn into_result(self) -> Result<ProcOutput, ProcError>;
    }

    impl Answer for ProcOutput {
        fn into_result(self) -> Result<ProcOutput, ProcError> {
            Ok(self)
        }
    }

    impl Answer for Result<ProcOutput, ProcError> {
        fn into_result(self) -> Result<ProcOutput, ProcError> {
            self
        }
    }

    fn script_one<S: AsRef<str>>(fake: &FakeProc, argv: &[S], answer: impl Answer) {
        let argv: Vec<&str> = argv.iter().map(AsRef::as_ref).collect();
        fake.script(&argv, Script::always(answer.into_result()));
    }

    fn grep_shape(pattern: &str, log: &Path) -> Vec<String> {
        ["grep", "-aoiE", "--", pattern, log.to_str().unwrap()].map(String::from).to_vec()
    }

    fn argvs(log: &EffectLog) -> Vec<Vec<String>> {
        log.entries()
            .into_iter()
            .map(|effect| match effect {
                Effect::Proc(ProcEffect::Run(spec)) => spec.argv.as_slice().to_vec(),
                other => panic!("not a process call: {other:?}"),
            })
            .collect()
    }

    // The regex decision: grep gets the pattern as ONE argument, after `--`, so a
    // pattern that starts with a dash is a pattern and a pattern with a space is
    // not split. Plan decision F8.
    #[test]
    fn each_shape_is_one_grep_call_with_the_pattern_as_one_argument_after_double_dash() {
        let scratch = Scratch::new("argv");
        let table = scratch.file(
            "t.json",
            r#"{"shapes": [
            {"id": "dash", "class": "transient", "pattern": "-x  y|z"}]}"#,
        );
        let log_path = scratch.file("run.log", "text\n");
        let effects = EffectLog::new();
        let fake = FakeProc::new(&effects);
        script_one(&fake, &grep_shape("-x  y|z", &log_path), out(0, "hit -x  y\n"));
        let got = classify_failure(&fake, &env_with(&table), &log_path);
        assert_eq!(
            got,
            Classification::Matched {
                class: "transient".into(),
                id: "dash".into(),
                text: b"hit -x  y".to_vec()
            }
        );
        assert_eq!(argvs(&effects), vec![grep_shape("-x  y|z", &log_path)]);
        for effect in effects.entries() {
            let Effect::Proc(ProcEffect::Run(spec)) = effect else { panic!("not a call") };
            assert!(spec.env.is_empty(), "grep needs no variable: {:?}", spec.env);
            assert!(spec.stdin.is_none());
            assert!(spec.timeout > Dur::ZERO && spec.timeout <= GREP_TIMEOUT);
        }
    }

    // `grep -aoiE ... | tail -1` takes the status of tail, so grep's own status is
    // not read. A match printed by a grep that then exits 2 still counts. Plan
    // decision F8.
    #[test]
    fn a_match_printed_by_a_grep_that_exits_2_still_counts() {
        let scratch = Scratch::new("exit2");
        let table = scratch.file("t.json", TABLE_TWO);
        let log_path = scratch.file("run.log", "x\n");
        let effects = EffectLog::new();
        let fake = FakeProc::new(&effects);
        script_one(&fake, &grep_shape("boom", &log_path), out(2, "partial boom\n"));
        let got = classify_failure(&fake, &env_with(&table), &log_path);
        assert_eq!(got.exit_code(), 0);
        assert_eq!(got.render(), b"transient\tfirst\tpartial boom");
    }

    // An invalid pattern makes grep print an error and exit 2 with nothing on
    // stdout. The bash hides the error and moves to the next shape. Plan
    // decision F8.
    #[test]
    fn a_grep_that_exits_2_with_no_output_means_no_match_and_the_next_shape_is_tried() {
        let scratch = Scratch::new("invalid");
        let table = scratch.file("t.json", TABLE_TWO);
        let log_path = scratch.file("run.log", "x\n");
        let effects = EffectLog::new();
        let fake = FakeProc::new(&effects);
        script_one(&fake, &grep_shape("boom", &log_path), out(2, ""));
        script_one(&fake, &grep_shape("bang", &log_path), out(0, "bang here\n"));
        let got = classify_failure(&fake, &env_with(&table), &log_path);
        assert_eq!(got.render(), b"needs_human\tsecond\tbang here");
        assert_eq!(argvs(&effects).len(), 2);
    }

    // Plan decision F8: the first shape in the table with a match wins, and the
    // shapes after it are not run at all.
    #[test]
    fn the_first_matching_shape_ends_the_search_and_later_shapes_are_never_run() {
        let scratch = Scratch::new("first");
        let table = scratch.file("t.json", TABLE_TWO);
        let log_path = scratch.file("run.log", "x\n");
        let effects = EffectLog::new();
        let fake = FakeProc::new(&effects);
        script_one(&fake, &grep_shape("boom", &log_path), out(0, "boom\n"));
        let got = classify_failure(&fake, &env_with(&table), &log_path);
        assert_eq!(got.exit_code(), 0);
        assert_eq!(argvs(&effects), vec![grep_shape("boom", &log_path)]);
    }

    // `| tail -1`: of the lines grep -o printed, the last one is the text.
    #[test]
    fn the_matched_text_is_the_last_line_grep_printed() {
        let scratch = Scratch::new("last");
        let table = scratch.file("t.json", TABLE_TWO);
        let log_path = scratch.file("run.log", "x\n");
        let fake = FakeProc::new(&EffectLog::new());
        script_one(&fake, &grep_shape("boom", &log_path), out(0, "boom 1\nboom 2\nboom 3\n"));
        let got = classify_failure(&fake, &env_with(&table), &log_path);
        assert_eq!(got.render(), b"transient\tfirst\tboom 3");
    }

    // `$(...)` drops NUL bytes, so a match across a NUL joins the two halves.
    #[test]
    fn a_nul_inside_the_matched_text_is_dropped() {
        let scratch = Scratch::new("nul");
        let table = scratch.file("t.json", TABLE_TWO);
        let log_path = scratch.file("run.log", "x\n");
        let fake = FakeProc::new(&EffectLog::new());
        script_one(&fake, &grep_shape("boom", &log_path), out(0, "bo\0om\n"));
        let got = classify_failure(&fake, &env_with(&table), &log_path);
        assert_eq!(got.render(), b"transient\tfirst\tboom");
    }

    // Decision P1F-6: the bash reads a grep that cannot start as "no match" and
    // answers "unclassified". The port answers "could not ask".
    #[test]
    fn a_grep_that_cannot_start_is_unreadable_and_not_unclassified() {
        let scratch = Scratch::new("nogrep");
        let table = scratch.file("t.json", TABLE_TWO);
        let log_path = scratch.file("run.log", "x\n");
        let fake = FakeProc::new(&EffectLog::new());
        script_one(
            &fake,
            &grep_shape("boom", &log_path),
            Err(ProcError::NotFound { program: "grep".into() }),
        );
        let got = classify_failure(&fake, &env_with(&table), &log_path);
        assert_eq!(got.exit_code(), 2);
        assert_eq!(
            String::from_utf8(got.render()).unwrap(),
            "unreadable\t\tgrep could not run: grep: not found on the search path"
        );
    }

    // Decision P1F-6: a grep that outlives the timeout is killed by the port.
    #[test]
    fn a_grep_that_times_out_is_unreadable() {
        let scratch = Scratch::new("timeout");
        let table = scratch.file("t.json", TABLE_TWO);
        let log_path = scratch.file("run.log", "x\n");
        let fake = FakeProc::new(&EffectLog::new());
        script_one(
            &fake,
            &grep_shape("boom", &log_path),
            Ok(ProcOutput { exit: ProcExit::TimedOut, stdout: Vec::new(), stderr: Vec::new() }),
        );
        let got = classify_failure(&fake, &env_with(&table), &log_path);
        assert_eq!(got.exit_code(), 2);
        assert_eq!(
            String::from_utf8(got.render()).unwrap(),
            "unreadable\t\tgrep did not finish within 120 s"
        );
    }

    // The fallback: no shape matched, so the last line holding an error word.
    // The grep has no `--` in the bash, and the port keeps that.
    #[test]
    fn when_no_shape_matches_the_last_error_shaped_line_is_the_evidence() {
        let scratch = Scratch::new("fallback");
        let table = scratch.file("t.json", TABLE_TWO);
        let log_path = scratch.file("run.log", "x\n");
        let effects = EffectLog::new();
        let fake = FakeProc::new(&effects);
        script_one(&fake, &grep_shape("boom", &log_path), out(1, ""));
        script_one(&fake, &grep_shape("bang", &log_path), out(1, ""));
        let fallback = [
            "grep",
            "-aiE",
            "error|fail|panic|refus|denied|cannot|could not",
            log_path.to_str().unwrap(),
        ];
        script_one(&fake, &fallback, out(0, "an error\nthe last failure\n"));
        let got = classify_failure(&fake, &env_with(&table), &log_path);
        assert_eq!(got.exit_code(), 1);
        assert_eq!(got.render(), b"unclassified\t\tthe last failure");
        assert_eq!(argvs(&effects).last().unwrap(), &fallback.map(String::from).to_vec());
    }

    // With no error-shaped line the evidence is `tail -1` of the log.
    #[test]
    fn with_no_error_shaped_line_the_evidence_is_the_last_line_of_the_log() {
        let scratch = Scratch::new("tail");
        let table = scratch.file("t.json", TABLE_TWO);
        let log_path = scratch.file("run.log", "x\n");
        let fake = FakeProc::new(&EffectLog::new());
        script_one(&fake, &grep_shape("boom", &log_path), out(1, ""));
        script_one(&fake, &grep_shape("bang", &log_path), out(1, ""));
        let p = log_path.to_str().unwrap();
        script_one(
            &fake,
            &["grep", "-aiE", "error|fail|panic|refus|denied|cannot|could not", p],
            out(1, ""),
        );
        script_one(&fake, &["tail", "-1", p], out(0, "the last line\n"));
        let got = classify_failure(&fake, &env_with(&table), &log_path);
        assert_eq!(got.render(), b"unclassified\t\tthe last line");
    }

    // `tail -1` of a log that ends in an empty line prints that empty line, and
    // `$(...)` makes it empty, so the evidence reads "the log says nothing" even
    // though earlier lines exist. Pinned as the bash does it. Plan decision F8.
    #[test]
    fn a_log_whose_last_line_is_empty_reports_the_log_says_nothing() {
        let scratch = Scratch::new("blank");
        let table = scratch.file("t.json", TABLE_TWO);
        let log_path = scratch.file("run.log", "x\n");
        let fake = FakeProc::new(&EffectLog::new());
        script_one(&fake, &grep_shape("boom", &log_path), out(1, ""));
        script_one(&fake, &grep_shape("bang", &log_path), out(1, ""));
        let p = log_path.to_str().unwrap();
        script_one(
            &fake,
            &["grep", "-aiE", "error|fail|panic|refus|denied|cannot|could not", p],
            out(1, ""),
        );
        script_one(&fake, &["tail", "-1", p], out(0, "\n"));
        let got = classify_failure(&fake, &env_with(&table), &log_path);
        assert_eq!(got.render(), b"unclassified\t\tthe log says nothing");
    }

    // An empty id makes the tab collapse: the line `<TAB>transient<TAB>boom` reads
    // as id `transient`, class `boom` and an empty pattern. That row has an id, so
    // grep runs once with an empty pattern, prints nothing, and the row is
    // skipped after the call. Plan decision F8.
    #[test]
    fn an_empty_id_shifts_the_fields_so_grep_runs_with_an_empty_pattern() {
        let scratch = Scratch::new("emptyid");
        let table = scratch
            .file("t.json", r#"{"shapes": [{"id": "", "class": "transient", "pattern": "boom"}]}"#);
        let log_path = scratch.file("run.log", "x\n");
        let effects = EffectLog::new();
        let fake = FakeProc::new(&effects);
        script_one(&fake, &grep_shape("", &log_path), out(0, ""));
        let p = log_path.to_str().unwrap();
        script_one(
            &fake,
            &["grep", "-aiE", "error|fail|panic|refus|denied|cannot|could not", p],
            out(1, ""),
        );
        script_one(&fake, &["tail", "-1", p], out(0, "x\n"));
        let got = classify_failure(&fake, &env_with(&table), &log_path);
        assert_eq!(got.render(), b"unclassified\t\tx");
        assert_eq!(argvs(&effects)[0], grep_shape("", &log_path));
    }

    // A blank line inside a pattern reads as a line with an empty id, which the
    // bash skips with `[ -n "$id" ] || continue` before any grep runs. Plan
    // decision F8.
    #[test]
    fn a_blank_line_inside_a_pattern_is_skipped_without_a_grep_call() {
        let scratch = Scratch::new("blankrow");
        let table = scratch
            .file("t.json", r#"{"shapes": [{"id": "a", "class": "t", "pattern": "one\n\ntwo"}]}"#);
        let log_path = scratch.file("run.log", "x\n");
        let effects = EffectLog::new();
        let fake = FakeProc::new(&effects);
        script_one(&fake, &grep_shape("one", &log_path), out(1, ""));
        script_one(&fake, &grep_shape("", &log_path), out(1, ""));
        let p = log_path.to_str().unwrap();
        script_one(
            &fake,
            &["grep", "-aiE", "error|fail|panic|refus|denied|cannot|could not", p],
            out(1, ""),
        );
        script_one(&fake, &["tail", "-1", p], out(0, "x\n"));
        let got = classify_failure(&fake, &env_with(&table), &log_path);
        assert_eq!(got.exit_code(), 1);
        let calls = argvs(&effects);
        // one, then the empty pattern of the row `two`; the blank line made no call
        assert_eq!(calls[0], grep_shape("one", &log_path));
        assert_eq!(calls[1], grep_shape("", &log_path));
        assert_eq!(calls.len(), 4, "{calls:?}");
    }

    // The bash joins with a plain string. A trailing slash in HOME stays in the
    // path it prints, and an empty HOME gives a path under the root directory.
    #[test]
    fn the_search_joins_paths_as_strings_the_way_the_bash_does() {
        let env = ClassifyEnv {
            failure_shapes: None,
            lib_dir: PathBuf::from("/opt/maint/lib"),
            home: OsString::from("/home/u/"),
        };
        assert_eq!(
            search_candidates(&env),
            [
                PathBuf::from("/opt/maint/lib/failure-shapes.json"),
                PathBuf::from("/home/u//.local/share/maintainer/failure-shapes.json"),
            ]
        );
        let no_home = ClassifyEnv { home: OsString::new(), ..env };
        assert_eq!(
            search_candidates(&no_home)[1],
            PathBuf::from("/.local/share/maintainer/failure-shapes.json")
        );
    }

    #[test]
    fn render_and_exit_code_for_each_outcome() {
        let matched = Classification::Matched {
            class: "transient".into(),
            id: "a".into(),
            text: b"t t".to_vec(),
        };
        assert_eq!((matched.exit_code(), matched.render()), (0, b"transient\ta\tt t".to_vec()));
        let un = Classification::Unclassified { evidence: b"e".to_vec() };
        assert_eq!((un.exit_code(), un.render()), (1, b"unclassified\t\te".to_vec()));
        let no = Classification::Unreadable { reason: b"r".to_vec() };
        assert_eq!((no.exit_code(), no.render()), (2, b"unreadable\t\tr".to_vec()));
    }

    // `cut -f3` keeps the third tab-separated field, so a tab in the matched text
    // ends the reason there. Plan decision F8.
    #[test]
    fn the_predicates_cut_the_matched_text_at_its_first_tab() {
        let matched = Classification::Matched {
            class: "transient".into(),
            id: "a".into(),
            text: b"before\tafter".to_vec(),
        };
        assert_eq!(matched.transient_reason(), Some(b"before".to_vec()));
        assert_eq!(matched.needs_human_reason(), None);
        let leading = Classification::Matched {
            class: "needs_human".into(),
            id: "a".into(),
            text: b"\tafter".to_vec(),
        };
        assert_eq!(leading.needs_human_reason(), Some(Vec::new()));
    }

    // `case "$out" in transient*)` is a prefix match on the class, so a class a
    // custom table calls `transient_dns` answers the transient predicate.
    #[test]
    fn a_class_that_starts_with_transient_answers_the_transient_predicate() {
        let matched = Classification::Matched {
            class: "transient_dns".into(),
            id: "a".into(),
            text: b"why".to_vec(),
        };
        assert_eq!(matched.transient_reason(), Some(b"why".to_vec()));
        let other = Classification::Matched {
            class: "my_transient".into(),
            id: "a".into(),
            text: b"why".to_vec(),
        };
        assert_eq!(other.transient_reason(), None);
        assert_eq!(other.needs_human_reason(), None);
    }

    #[test]
    fn unclassified_and_unreadable_answer_neither_predicate() {
        for outcome in [
            Classification::Unclassified { evidence: b"transient\t\tx".to_vec() },
            Classification::Unreadable { reason: b"needs_human".to_vec() },
        ] {
            assert_eq!(outcome.transient_reason(), None, "{outcome:?}");
            assert_eq!(outcome.needs_human_reason(), None, "{outcome:?}");
        }
    }
}
