//! The shell and Python text rules the doctor leans on.

use mx_doctor::pyval::{float_repr, iso_epoch, py_len, py_repr};
use mx_doctor::text::*;
use serde_json::json;

#[test]
fn command_substitution_drops_every_trailing_newline_and_no_other_byte() {
    assert_eq!(strip_trailing_newlines(b"a\n\n\n"), b"a");
    assert_eq!(strip_trailing_newlines(b"a\n b \n"), b"a\n b ");
    assert_eq!(strip_trailing_newlines(b"\n\n"), b"");
}

#[test]
fn unquoted_expansion_splits_on_space_tab_and_newline_only() {
    assert_eq!(ifs_words("gh  git\tmaintainer\nx"), ["gh", "git", "maintainer", "x"]);
    assert_eq!(ifs_words("   "), Vec::<&str>::new());
    assert_eq!(ifs_words("a\u{a0}b"), ["a\u{a0}b"]);
}

#[test]
fn bash_test_reads_blanks_a_sign_and_digits() {
    assert_eq!(test_int("7"), Some(7));
    assert_eq!(test_int(" 7 "), Some(7));
    assert_eq!(test_int("-3"), Some(-3));
    assert_eq!(test_int("+3"), Some(3));
    for bad in ["", "abc", "1.5", "0x10", "1 2", "-", "+"] {
        assert_eq!(test_int(bad), None, "{bad:?}");
    }
}

#[test]
fn a_dot_in_a_profile_name_matches_any_one_byte() {
    assert!(line_has(b"x maintainer@aXb-review.timer y", "maintainer@a.b-", false));
    assert!(!line_has(b"x maintainer@ab-review.timer y", "maintainer@a.b-", false));
}

#[test]
fn an_anchored_pattern_must_start_the_line() {
    assert!(line_has(b"maintainer@magent-review.timer", "maintainer@magent-", true));
    assert!(!line_has(b" maintainer@magent-review.timer", "maintainer@magent-", true));
}

#[test]
fn counting_lines_counts_lines_not_matches() {
    let text = b"maintainer@m- maintainer@m-\nnothing\nmaintainer@m-\n";
    assert_eq!(count_lines_with(text, "maintainer@m-", false), 2);
    assert_eq!(count_lines_with(b"", "x", false), 0);
}

#[test]
fn a_timer_row_fires_within_seconds_only_when_the_number_has_a_seconds_unit() {
    assert!(fires_in_seconds(b"x   30s left rest"));
    assert!(fires_in_seconds(b"Mon 5s left"));
    assert!(!fires_in_seconds(b"x   30m left"));
    assert!(!fires_in_seconds(b"x   30s"), "the unit needs a space after it");
    assert!(!fires_in_seconds(b"Mon 2026-01-05 10:00:00 UTC 3h left"));
    assert!(!fires_in_seconds(b" x 30s left"), "the row must start with a word");
}

#[test]
fn reading_tab_separated_fields_collapses_runs_and_trims_the_ends() {
    assert_eq!(read_tab_fields("red\tCI\t10\thttp://x", 4), ["red", "CI", "10", "http://x"]);
    assert_eq!(read_tab_fields("red\tCI\t10\t", 4), ["red", "CI", "10", ""]);
    assert_eq!(read_tab_fields("red\tCI\t\t10", 4), ["red", "CI", "10", ""]);
    assert_eq!(read_tab_fields("red\tA\tB\t10\turl", 4), ["red", "A", "B", "10\turl"]);
}

#[test]
fn the_first_two_lines_of_output_become_one_line_with_spaces() {
    assert_eq!(head_lines_spaced("a\nb\nc", 2), "a b ");
    assert_eq!(head_lines_spaced("a\nb", 2), "a b");
    assert_eq!(head_lines_spaced("only", 2), "only");
}

#[test]
fn printf_rounds_a_half_to_the_even_neighbour() {
    assert_eq!(round0(0.5), "0");
    assert_eq!(round0(1.5), "2");
    assert_eq!(round0(2.5), "2");
    assert_eq!(round0(9.99), "10");
    assert_eq!(round0(8.98), "9");
}

#[test]
fn python_float_accepts_signs_exponents_underscores_and_the_named_values() {
    assert_eq!(py_float("6"), Some(6.0));
    assert_eq!(py_float(" 1.5 "), Some(1.5));
    assert_eq!(py_float("1_0"), Some(10.0));
    assert_eq!(py_float("1e1"), Some(10.0));
    assert_eq!(py_float(".5"), Some(0.5));
    assert_eq!(py_float("-2"), Some(-2.0));
    assert!(py_float("inf").is_some_and(f64::is_infinite));
    assert!(py_float("nan").is_some_and(f64::is_nan));
    for bad in ["", "abc", "1__0", "_1", "1_", "1e", "--1", ".", "1.2.3"] {
        assert_eq!(py_float(bad), None, "{bad:?}");
    }
}

#[test]
fn python_int_of_text_takes_underscored_digits_and_no_fraction() {
    assert_eq!(py_int_text(" 123 "), Some(123.0));
    assert_eq!(py_int_text("1_000"), Some(1000.0));
    assert_eq!(py_int_text("-5"), Some(-5.0));
    for bad in ["", "1.5", "abc", "1e3", "1__0"] {
        assert_eq!(py_int_text(bad), None, "{bad:?}");
    }
}

#[test]
fn a_slash_and_a_dot_both_become_a_dash() {
    assert_eq!(dashes_for_slash_and_dot("/home/u/my.repo"), "-home-u-my-repo");
}

#[test]
fn json_lengths_follow_python() {
    assert_eq!(py_len(&json!([1, 2, 3])), Some(3));
    assert_eq!(py_len(&json!({"a": 1})), Some(1));
    assert_eq!(py_len(&json!("héllo")), Some(5));
    assert_eq!(py_len(&json!(5)), None);
    assert_eq!(py_len(&json!(null)), None);
}

#[test]
fn python_repr_matches_for_the_values_a_failure_reason_can_hold() {
    assert_eq!(py_repr(&json!(null)), "None");
    assert_eq!(py_repr(&json!(true)), "True");
    assert_eq!(py_repr(&json!(42)), "42");
    assert_eq!(py_repr(&json!(-0)), "0");
    assert_eq!(py_repr(&json!([1, "a", {"b": 1.5, "c": null}])), "[1, 'a', {'b': 1.5, 'c': None}]");
    assert_eq!(py_repr(&json!("it's")), "\"it's\"");
    assert_eq!(py_repr(&json!("a\nb")), "'a\\nb'");
}

#[test]
fn float_repr_switches_to_an_exponent_outside_the_positional_range() {
    assert_eq!(float_repr(1.0), "1.0");
    assert_eq!(float_repr(1.5), "1.5");
    assert_eq!(float_repr(0.0001), "0.0001");
    assert_eq!(float_repr(0.00001), "1e-05");
    assert_eq!(float_repr(1e15), "1000000000000000.0");
    assert_eq!(float_repr(1e16), "1e+16");
    assert_eq!(float_repr(123_456_789.25), "123456789.25");
}

#[test]
fn an_iso_time_with_an_offset_reads_and_a_naive_one_does_not() {
    let at = iso_epoch("2026-01-05T12:00:00Z").unwrap();
    assert!((at - 1_767_614_400.0).abs() < f64::EPSILON);
    assert_eq!(iso_epoch("2026-01-05T12:00:00+00:00"), Some(at));
    assert_eq!(iso_epoch("2026-01-05T13:00:00+01:00"), Some(at));
    assert_eq!(iso_epoch("2026-01-05T12:00:00"), None, "naive times cannot be subtracted");
    assert_eq!(iso_epoch("yesterday"), None);
    assert_eq!(iso_epoch("2026-13-05T12:00:00Z"), None);
}
