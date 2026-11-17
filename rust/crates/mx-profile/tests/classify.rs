//! The failure classifier against the real `grep`, the real shipped table and
//! logs on disk. Each case checks the triple `class TAB id TAB text` and the
//! return code by value. The cases mirror `tests/characterization/run-classify.sh`,
//! which pins the same behaviour on the bash.

mod common;

use common::{Scratch, real_proc, repo_lib};
use mx_ports::Proc;
use mx_profile::classify::{ClassifyEnv, classify_failure, failure_shapes_path};
use std::ffi::OsString;
use std::path::Path;

fn env_with_table(table: Option<&Path>) -> ClassifyEnv {
    ClassifyEnv {
        failure_shapes: table.map(|path| path.as_os_str().to_owned()),
        lib_dir: repo_lib(),
        home: OsString::from("/nonexistent/home"),
        locale: None,
    }
}

/// Classifies `log_text` against the shipped table and returns the printed
/// bytes and the return code.
fn classify_text(log_text: &[u8]) -> (String, i32) {
    let scratch = Scratch::new("cls");
    let log = scratch.write("x.log", log_text);
    let got = classify_failure(&real_proc(), &env_with_table(None), &log);
    (String::from_utf8_lossy(&got.render()).into_owned(), got.exit_code())
}

/// Classifies under a UTF-8 locale, as production runs.
fn classify_utf8(table: Option<&str>, log_text: &[u8]) -> Vec<u8> {
    let scratch = Scratch::new("utf8");
    let log = scratch.write("x.log", log_text);
    let table = table.map(|text| scratch.write("t.json", text));
    let env = ClassifyEnv { locale: Some("C.UTF-8".into()), ..env_with_table(table.as_deref()) };
    classify_failure(&real_proc(), &env, &log).render()
}

fn with_table(table: &str, log_text: &[u8]) -> (String, i32) {
    let scratch = Scratch::new("tbl");
    let log = scratch.write("x.log", log_text);
    let table = scratch.write("t.json", table);
    let got = classify_failure(&real_proc(), &env_with_table(Some(&table)), &log);
    (String::from_utf8_lossy(&got.render()).into_owned(), got.exit_code())
}

/// One case per shipped shape: id, class, a log line that triggers it, the text
/// grep reports.
const SHAPES: [(&str, &str, &str, &str); 21] = [
    (
        "session-limit",
        "transient",
        "You've hit your session limit, resets 10:10pm",
        "You've hit your session limit, resets 10:10pm",
    ),
    (
        "usage-limit-reached",
        "transient",
        "Usage limit reached. Try again later",
        "Usage limit reached. Try again later",
    ),
    (
        "upstream-overloaded",
        "transient",
        "API Error: {\"type\":\"overloaded_error\"}",
        "overloaded_error",
    ),
    ("rate-limited", "transient", "API Error: 429 Too Many Requests", "API Error: 429"),
    ("upstream-5xx", "transient", "API Error: 503 Service Unavailable", "API Error: 503"),
    ("connection-error", "transient", "read ECONNRESET", "ECONNRESET"),
    (
        "api-unreachable",
        "transient",
        "Can't reach the API server. Check your network",
        "Can't reach the API server. Check your network",
    ),
    ("dns-failure", "transient", "getaddrinfo EAI_AGAIN api.example.test", "EAI_AGAIN"),
    (
        "gh-unreachable",
        "transient",
        "error connecting to api.github.com",
        "error connecting to api.github.com",
    ),
    (
        "oauth-expired",
        "needs_human",
        "OAuth session expired. Please log in again",
        "OAuth session expired. Please log in again",
    ),
    (
        "invalid-key",
        "needs_human",
        "Invalid API key. Fix external API key",
        "Invalid API key. Fix external API key",
    ),
    (
        "out-of-credit",
        "needs_human",
        "Your credit balance is too low to access the API",
        "Your credit balance is too low to access the API",
    ),
    ("login-required", "needs_human", "Please run /login", "Please run /login"),
    (
        "backend-unconfigured",
        "needs_human",
        "backend claude unusable: missing /x/settings.json",
        "backend claude unusable: missing /x/settings.json",
    ),
    (
        "wrong-gh-account",
        "needs_human",
        "gh is authenticated as 'someone', not fxacct; refusing",
        "gh is authenticated as 'someone', not fxacct",
    ),
    (
        "token-unpinnable",
        "needs_human",
        "could not pin a token for fxacct, so",
        "could not pin a token for fxacct",
    ),
    (
        "checkout-refresh-failed",
        "transient",
        "refresh failed, no agent run started. See x",
        "refresh failed, no agent run started",
    ),
    (
        "lock-held",
        "transient",
        "another run held the lock for over 3600s, so",
        "another run held the lock for over 3600s",
    ),
    ("killed-silently", "needs_human", "Killed", "Killed"),
    (
        "argument-list-too-long",
        "needs_human",
        "cursor prompt is too large for a single argument (argument list too long); reduce the context",
        "argument list too long",
    ),
    (
        "no-doctrine",
        "needs_human",
        "preamble-core.md is missing; refusing",
        "preamble-core.md is missing",
    ),
];

// Plan decision F8: the characterization pins the output of the bash classifier
// on every shipped shape; the port must print the same triple.
#[test]
fn each_shipped_shape_classifies_its_sample_log_as_the_bash_does() {
    let mut ran = 0;
    for (id, class, log, hit) in SHAPES {
        let (printed, code) = classify_text(format!("{log}\n").as_bytes());
        assert_eq!((printed.as_str(), code), (format!("{class}\t{id}\t{hit}").as_str(), 0), "{id}");
        ran += 1;
    }
    assert_eq!(ran, 21);
    println!("{ran} shipped shapes classified");
}

// The sample list above must cover the shipped table, id for id, so a shape
// added to the table without a case fails here and not silently.
#[test]
fn the_sample_list_covers_every_id_of_the_shipped_table_but_the_never_matching_one() {
    let bytes = std::fs::read(repo_lib().join("failure-shapes.json")).unwrap();
    let shapes = mx_profile::table::parse_shapes(&bytes).unwrap();
    let mut table_ids: Vec<&str> = shapes.iter().map(|shape| shape.id.as_str()).collect();
    table_ids.sort_unstable();
    let mut sample_ids: Vec<&str> = SHAPES.iter().map(|case| case.0).collect();
    sample_ids.push("died-without-saying");
    sample_ids.sort_unstable();
    assert_eq!(sample_ids, table_ids);
    assert_eq!(table_ids.len(), 22);
}

// The table's own note says died-without-saying "is never matched by pattern".
// Plan decision F8 pins what the bash does. On GNU grep 3.12 the pattern
// `(?!x)x` is accepted with a warning and matches the text `!xx`, so a log that
// holds `!xx` is classified needs_human. The shape's own id and pattern text
// do not hold `!xx`, and match nothing. Decision P1F-11 records the finding.
#[test]
fn the_shape_died_without_saying_does_not_match_its_own_id_and_pattern_text() {
    let (printed, code) = classify_text(b"died-without-saying (?!x)x\n");
    assert_eq!((printed.as_str(), code), ("unclassified\t\tdied-without-saying (?!x)x", 1));
}

#[test]
fn the_first_shape_in_the_table_wins_whichever_line_of_the_log_came_first() {
    let (printed, code) =
        classify_text(b"OAuth session expired\nAPI Error: {\"type\":\"overloaded_error\"}\n");
    assert_eq!((printed.as_str(), code), ("transient\tupstream-overloaded\toverloaded_error", 0));
}

// The table decides, not severity: connection-error comes before invalid-key in
// the shipped order.
#[test]
fn a_transient_shape_listed_before_a_needs_human_one_wins_over_it() {
    let (printed, code) =
        classify_text(b"Invalid API key. Fix external API key\nconnection error while reading\n");
    assert_eq!((printed.as_str(), code), ("transient\tconnection-error\tconnection error", 0));
}

#[test]
fn within_one_shape_the_last_matching_text_in_the_log_is_reported() {
    let (printed, _) = classify_text(
        b"You've hit your session limit, resets 8am\nother line\nYou've hit your session limit, resets 10pm\n",
    );
    assert_eq!(printed, "transient\tsession-limit\tYou've hit your session limit, resets 10pm");
}

#[test]
fn matching_ignores_case_and_reports_the_text_as_written() {
    let (printed, _) = classify_text(b"USAGE LIMIT REACHED NOW\n");
    assert_eq!(printed, "transient\tusage-limit-reached\tUSAGE LIMIT REACHED NOW");
}

#[test]
fn the_trailing_bound_of_a_shape_cuts_the_text_at_60_characters_after_the_phrase() {
    let log = format!("usage limit reached {}\n", "x".repeat(80));
    let (printed, _) = classify_text(log.as_bytes());
    let want = format!("transient\tusage-limit-reached\tusage limit reached {}", "x".repeat(59));
    assert_eq!(printed, want);
}

// The reason this crate calls grep: leftmost-longest. An in-process leftmost-first
// engine reports `a` for `a|ab` on `ab`. Plan decision F8.
#[test]
fn an_alternation_reports_the_longest_match_at_the_leftmost_position() {
    let table = r#"{"shapes": [{"id": "alt", "class": "transient", "pattern": "a|ab|abc"}]}"#;
    let (printed, code) = with_table(table, b"xx abcd\n");
    assert_eq!((printed.as_str(), code), ("transient\talt\tabc", 0));
}

#[test]
fn a_log_with_a_nul_byte_is_still_read_as_text() {
    let (printed, code) = classify_text(b"binary\0prefix\nKilled\n");
    assert_eq!((printed.as_str(), code), ("needs_human\tkilled-silently\tKilled", 0));
}

#[test]
fn a_log_without_a_trailing_newline_is_read_to_its_last_byte() {
    let (printed, code) = classify_text(b"all fine\nKilled");
    assert_eq!((printed.as_str(), code), ("needs_human\tkilled-silently\tKilled", 0));
}

// One line of two megabytes. grep -o reports only the match, so the text stays small.
#[test]
fn a_log_of_one_gigantic_line_reports_only_the_matched_text() {
    let mut log = "z".repeat(2_000_000);
    log.push_str(" Killed ");
    log.push_str(&"z".repeat(2_000_000));
    let (printed, code) = classify_text(log.as_bytes());
    assert_eq!((printed.as_str(), code), ("needs_human\tkilled-silently\tKilled", 0));
}

// When nothing matches, the evidence is the whole error-shaped LINE, however
// long. The bash prints it into the alert. Pinned. Plan decision D14 covers the
// same hazard for the alert text.
#[test]
fn an_unclassified_log_of_one_gigantic_line_reports_the_whole_line_as_evidence() {
    let line = format!("{} an error {}", "z".repeat(500_000), "y".repeat(500_000));
    let (printed, code) = classify_text(line.as_bytes());
    assert_eq!(code, 1);
    assert_eq!(printed.len(), "unclassified\t\t".len() + line.len());
    assert!(printed.ends_with(&line));
}

#[test]
fn a_table_pattern_that_is_not_valid_ere_matches_nothing_and_the_next_shape_is_tried() {
    let table = r#"{"shapes": [
        {"id": "broken", "class": "needs_human", "pattern": "("},
        {"id": "next", "class": "transient", "pattern": "boom [0-9]+"}]}"#;
    let (printed, code) = with_table(table, b"it went boom 42 today\n");
    assert_eq!((printed.as_str(), code), ("transient\tnext\tboom 42", 0));
}

#[test]
fn a_pattern_that_starts_with_a_dash_is_a_pattern_and_not_an_option() {
    let table = r#"{"shapes": [{"id": "dash", "class": "transient", "pattern": "-fail+"}]}"#;
    let (printed, code) = with_table(table, b"status -failll now\n");
    assert_eq!((printed.as_str(), code), ("transient\tdash\t-failll", 0));
}

#[test]
fn matching_with_no_error_word_in_any_line_falls_back_to_the_last_line() {
    let (printed, code) = classify_text(b"all fine\nthe last line\n");
    assert_eq!((printed.as_str(), code), ("unclassified\t\tthe last line", 1));
}

#[test]
fn no_shape_matching_reports_the_last_line_holding_an_error_word_with_status_1() {
    let (printed, code) = classify_text(b"all fine\nsomething failed here\nthe last line\n");
    assert_eq!((printed.as_str(), code), ("unclassified\t\tsomething failed here", 1));
}

#[test]
fn an_empty_log_is_unclassified_with_the_log_says_nothing() {
    let (printed, code) = classify_text(b"");
    assert_eq!((printed.as_str(), code), ("unclassified\t\tthe log says nothing", 1));
}

// `tail -1` prints the blank last line, `$(...)` empties it, and the fallback
// reports nothing although earlier lines exist. Plan decision F8.
#[test]
fn a_log_that_ends_in_a_blank_line_reports_the_log_says_nothing() {
    let (printed, code) = classify_text(b"all fine\nfine too\n\n");
    assert_eq!((printed.as_str(), code), ("unclassified\t\tthe log says nothing", 1));
}

#[test]
fn a_log_that_does_not_exist_is_unreadable_with_status_2() {
    let scratch = Scratch::new("nolog");
    let log = scratch.path().join("nope.log");
    let got = classify_failure(&real_proc(), &env_with_table(None), &log);
    let want = format!("unreadable\t\tno readable log at '{}'", log.display());
    assert_eq!((String::from_utf8(got.render()).unwrap(), got.exit_code()), (want, 2));
}

#[test]
fn an_empty_log_path_is_unreadable_with_an_empty_name() {
    let got = classify_failure(&real_proc(), &env_with_table(None), Path::new(""));
    assert_eq!(
        (got.render(), got.exit_code()),
        (b"unreadable\t\tno readable log at ''".to_vec(), 2)
    );
}

#[test]
fn a_log_that_is_a_directory_has_no_text_to_classify() {
    let scratch = Scratch::new("dirlog");
    let got = classify_failure(&real_proc(), &env_with_table(None), scratch.path());
    assert_eq!(
        (got.render(), got.exit_code()),
        (b"unclassified\t\tthe log says nothing".to_vec(), 1)
    );
}

#[test]
fn no_table_anywhere_is_unreadable_and_never_nothing_matched() {
    let scratch = Scratch::new("notable");
    let log = scratch.write("x.log", "Killed\n");
    let env = ClassifyEnv {
        failure_shapes: None,
        lib_dir: scratch.path().join("lib"),
        home: scratch.path().join("home").into_os_string(),
        locale: None,
    };
    let got = classify_failure(&real_proc(), &env, &log);
    assert_eq!(
        (got.render(), got.exit_code()),
        (b"unreadable\t\tno failure-shapes table on this host".to_vec(), 2)
    );
}

#[test]
fn a_table_that_is_not_json_is_unreadable_with_status_2() {
    let scratch = Scratch::new("badjson");
    let log = scratch.write("x.log", "Killed\n");
    let table = scratch.write("bad.json", "this is not json\n");
    let got = classify_failure(&real_proc(), &env_with_table(Some(&table)), &log);
    let want =
        format!("unreadable\t\tfailure-shapes table at {} could not be parsed", table.display());
    assert_eq!((String::from_utf8(got.render()).unwrap(), got.exit_code()), (want, 2));
}

#[test]
fn an_override_naming_a_missing_file_is_used_as_given_and_reads_as_unparsable() {
    let scratch = Scratch::new("gone");
    let log = scratch.write("x.log", "Killed\n");
    let gone = scratch.path().join("gone.json");
    let got = classify_failure(&real_proc(), &env_with_table(Some(&gone)), &log);
    let want =
        format!("unreadable\t\tfailure-shapes table at {} could not be parsed", gone.display());
    assert_eq!((String::from_utf8(got.render()).unwrap(), got.exit_code()), (want, 2));
}

#[test]
fn a_table_with_no_shapes_is_unreadable_and_says_it_lists_none() {
    let scratch = Scratch::new("noshapes");
    let log = scratch.write("x.log", "Killed\n");
    let table = scratch.write("empty.json", "{\"shapes\": []}\n");
    let got = classify_failure(&real_proc(), &env_with_table(Some(&table)), &log);
    let want = format!("unreadable\t\tfailure-shapes table at {} lists no shapes", table.display());
    assert_eq!((String::from_utf8(got.render()).unwrap(), got.exit_code()), (want, 2));
}

#[test]
fn an_override_table_replaces_the_shipped_table_whole() {
    let table =
        r#"{"shapes": [{"id": "my-shape", "class": "needs_human", "pattern": "boom [0-9]+"}]}"#;
    let (printed, code) = with_table(table, b"it went boom 42 today\nKilled\n");
    assert_eq!((printed.as_str(), code), ("needs_human\tmy-shape\tboom 42", 0));
}

#[test]
fn a_table_class_outside_the_two_known_ones_is_printed_as_written() {
    let table = r#"{"shapes": [{"id": "odd", "class": "weird-class", "pattern": "boom"}]}"#;
    let (printed, code) = with_table(table, b"boom\n");
    assert_eq!((printed.as_str(), code), ("weird-class\todd\tboom", 0));
}

// The search order of the table: the override, then the file beside the library,
// then the installed copy. `[ -f ]` skips a directory of that name.
#[test]
fn the_table_is_found_by_override_then_beside_the_library_then_installed() {
    let scratch = Scratch::new("order");
    let lib = scratch.path().join("lib");
    let home = scratch.path().join("home");
    let beside = scratch.write("lib/failure-shapes.json", "{}");
    let installed = scratch.write("home/.local/share/maintainer/failure-shapes.json", "{}");
    let over = scratch.write("over.json", "{}");
    let mut env = ClassifyEnv {
        failure_shapes: Some(over.as_os_str().to_owned()),
        lib_dir: lib,
        home: home.into_os_string(),
        locale: None,
    };
    assert_eq!(failure_shapes_path(&env), Some(over.clone()));
    env.failure_shapes = Some(OsString::new());
    assert_eq!(
        failure_shapes_path(&env),
        Some(beside.clone()),
        "an empty override counts as unset"
    );
    env.failure_shapes = None;
    assert_eq!(failure_shapes_path(&env), Some(beside.clone()));
    std::fs::remove_file(&beside).unwrap();
    std::fs::create_dir(&beside).unwrap();
    assert_eq!(failure_shapes_path(&env), Some(installed.clone()), "a directory is not a table");
    std::fs::remove_file(&installed).unwrap();
    assert_eq!(failure_shapes_path(&env), None);
}

#[test]
fn the_two_predicates_answer_from_the_same_classification() {
    let scratch = Scratch::new("pred");
    let env = env_with_table(None);
    let case = |text: &str| {
        let log = scratch.write("p.log", text);
        classify_failure(&real_proc(), &env, &log)
    };
    let transient = case("API Error: 503 Service Unavailable\n");
    assert_eq!(transient.transient_reason(), Some(b"API Error: 503".to_vec()));
    assert_eq!(transient.needs_human_reason(), None);
    let human = case("Invalid API key. Fix external API key\n");
    assert_eq!(human.transient_reason(), None);
    assert_eq!(human.needs_human_reason(), Some(b"Invalid API key. Fix external API key".to_vec()));
    let plain = case("just a plain failure\n");
    assert_eq!((plain.transient_reason(), plain.needs_human_reason()), (None, None));
    let missing = classify_failure(&real_proc(), &env, &scratch.path().join("none.log"));
    assert_eq!((missing.transient_reason(), missing.needs_human_reason()), (None, None));
}

// Decision P1F-5. Under a UTF-8 locale `.{0,60}` counts characters, so the cut
// lands on a character boundary and the evidence is valid UTF-8. Under C it
// counts bytes and can end inside a character.
#[test]
fn under_a_utf8_locale_the_60_character_cut_ends_on_a_whole_character() {
    let log = format!("usage limit reached {}\n", "\u{e9}".repeat(70));
    let utf8 = classify_utf8(None, log.as_bytes());
    let want =
        format!("transient\tusage-limit-reached\tusage limit reached {}", "\u{e9}".repeat(59));
    assert_eq!(String::from_utf8(utf8).expect("valid UTF-8"), want);
    let (c_text, _) = classify_text(log.as_bytes());
    assert!(c_text.ends_with('\u{fffd}'), "C cuts inside a character: {c_text}");
}

#[test]
fn under_a_utf8_locale_multibyte_text_before_and_inside_the_match_is_kept() {
    let table =
        r#"{"shapes": [{"id": "x", "class": "transient", "pattern": "caf\u00e9 .{0,3}ok"}]}"#;
    let log = "\u{e9}\u{e9} avant caf\u{e9} \u{e9}\u{e9}ok apr\u{e8}s\n";
    let got = classify_utf8(Some(table), log.as_bytes());
    assert_eq!(String::from_utf8(got).unwrap(), "transient\tx\tcaf\u{e9} \u{e9}\u{e9}ok");
}

#[test]
fn under_a_utf8_locale_case_folding_covers_non_ascii_letters() {
    let table = r#"{"shapes": [{"id": "x", "class": "transient", "pattern": "caf\u00e9"}]}"#;
    let got = classify_utf8(Some(table), "un CAF\u{c9} noir\n".as_bytes());
    assert_eq!(String::from_utf8(got).unwrap(), "transient\tx\tCAF\u{c9}");
    let (c_text, code) = with_table(table, "un CAF\u{c9} noir\n".as_bytes());
    assert_eq!(code, 1, "under C the fold is ASCII only: {c_text}");
}

// Invalid bytes in the window stop a `.{0,n}` match under a UTF-8 locale.
#[test]
fn under_a_utf8_locale_an_invalid_byte_ends_the_match_window() {
    let mut log = b"usage limit reached ab".to_vec();
    log.extend_from_slice(&[0xff, 0xfe]);
    log.extend_from_slice(b"cd\n");
    let got = classify_utf8(None, &log);
    assert_eq!(got, b"transient\tusage-limit-reached\tusage limit reached ab");
    let (c_text, _) = classify_text(&log);
    assert!(c_text.len() > got.len(), "C carries the bytes through: {c_text:?}");
}

// Decision P1F-6, corrected: opening a log that is a named pipe blocks without
// bound, as the bash does. The case is preserved and bounded here by a wait.
#[test]
fn a_named_pipe_as_the_log_blocks_the_open_as_the_bash_does() {
    let scratch = Scratch::new("fifolog");
    let fifo = scratch.path().join("pipe.log");
    let made = real_proc()
        .run(&mx_ports::ProcSpec::command(
            "mkfifo",
            [fifo.to_str().unwrap()],
            std::time::Duration::from_secs(10),
        ))
        .expect("mkfifo can run");
    assert!(made.exit.success());
    let (sender, receiver) = std::sync::mpsc::channel();
    let log = fifo.clone();
    std::thread::spawn(move || {
        drop(sender.send(classify_failure(&real_proc(), &env_with_table(None), &log)));
    });
    let outcome = receiver.recv_timeout(std::time::Duration::from_secs(2));
    assert!(outcome.is_err(), "the classifier returned for a pipe with no writer: {outcome:?}");
}
