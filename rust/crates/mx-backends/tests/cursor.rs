//! The Cursor backend: the six questions of `lib/backends/cursor.sh`.

mod common;

use common::{
    LOG, PROMPT, REPO, append, argv, backend, ctx, env, exports, only_stage, request, with,
};
use mx_backends::FakeHost;
use mx_ports::{Sink, Source};
use std::path::PathBuf;

const WALL_FILE: &str = "/p/fx/cursor-rehearsal/cli-config.json";
const LIVE_DIR: &str = "/p/fx/cursor";
const LIVE_FILE: &str = "/p/fx/cursor/cli-config.json";
const TOO_LARGE: &str = "cursor prompt is too large for a single argument (argument list too long); reduce the context or use another backend";

fn prompt_of(bytes: Vec<u8>) -> FakeHost {
    FakeHost::new().with_file(PROMPT, bytes)
}

#[test]
fn cursor_is_named_cursor() {
    assert_eq!(backend("cursor").name(), "cursor");
}

#[test]
fn cursor_declares_it_can_rehearse() {
    assert!(backend("cursor").declares_rehearsal());
}

#[test]
fn cursor_may_only_run_readonly_review() {
    assert_eq!(backend("cursor").allowed_tasks(), Some(&["readonly-review"][..]));
}

#[test]
fn the_cursor_wall_is_the_cursor_rehearsal_directory_exported_as_maintainer_cursor_dir() {
    let host = FakeHost::new().with_file(WALL_FILE, "{}");
    let wall = backend("cursor").rehearsal_wall(&ctx(), &host).expect("a wall");
    assert_eq!(wall.name, "MAINTAINER_CURSOR_DIR");
    assert_eq!(wall.value, "/p/fx/cursor-rehearsal");
}

#[test]
fn a_cursor_rehearsal_directory_without_cli_config_json_is_no_wall() {
    let host = FakeHost::new().without_file(WALL_FILE);
    assert_eq!(backend("cursor").rehearsal_wall(&ctx(), &host), None);
}

#[test]
fn cursor_check_passes_with_the_program_and_the_live_wall() {
    let host = FakeHost::new().with_program("cursor-agent").with_file(LIVE_FILE, "{}");
    assert_eq!(backend("cursor").check(&ctx(), &host), Ok(()));
}

#[test]
fn cursor_check_names_a_missing_program_before_it_looks_at_the_wall() {
    let host = FakeHost::new().without_program("cursor-agent");
    assert_eq!(backend("cursor").check(&ctx(), &host), Err("cursor-agent not on PATH".to_string()));
}

#[test]
fn cursor_check_names_the_missing_wall_directory_and_the_installer() {
    let host = FakeHost::new().with_program("cursor-agent").without_file(LIVE_FILE);
    assert_eq!(
        backend("cursor").check(&ctx(), &host),
        Err("no cursor wall at /p/fx/cursor; run ./install.sh".to_string())
    );
}

#[test]
fn cursor_check_looks_in_the_directory_the_run_exported() {
    let host = FakeHost::new()
        .with_program("cursor-agent")
        .without_file("/p/fx/cursor-rehearsal/cli-config.json");
    let ctx = with(ctx(), "MAINTAINER_CURSOR_DIR", "/p/fx/cursor-rehearsal");
    assert_eq!(
        backend("cursor").check(&ctx, &host),
        Err("no cursor wall at /p/fx/cursor-rehearsal; run ./install.sh".to_string())
    );
}

#[test]
fn cursor_check_treats_an_empty_maintainer_cursor_dir_as_unset() {
    let host = FakeHost::new().with_program("cursor-agent").with_file(LIVE_FILE, "{}");
    let ctx = with(ctx(), "MAINTAINER_CURSOR_DIR", "");
    assert_eq!(backend("cursor").check(&ctx, &host), Ok(()));
}

#[test]
fn cursor_passes_the_prompt_as_one_argument_and_no_force_flag() {
    let plan = backend("cursor")
        .plan(&ctx(), &prompt_of(b"the prompt text\nsecond line\n".to_vec()), &request())
        .expect("a plan");
    let stage = only_stage(&plan);
    assert_eq!(
        argv(stage),
        [
            "cursor-agent",
            "-p",
            "--output-format",
            "text",
            "-m",
            "opus",
            "the prompt text\nsecond line"
        ]
    );
    assert!(!argv(stage).contains(&"--force"));
}

#[test]
fn cursor_gets_an_empty_standard_input_because_the_prompt_is_an_argument() {
    let plan =
        backend("cursor").plan(&ctx(), &prompt_of(b"p\n".to_vec()), &request()).expect("a plan");
    assert_eq!(only_stage(&plan).stdin, Source::Empty);
}

#[test]
fn cursor_appends_both_streams_to_the_log_from_the_checkout() {
    let plan =
        backend("cursor").plan(&ctx(), &prompt_of(b"p".to_vec()), &request()).expect("a plan");
    let stage = only_stage(&plan);
    assert_eq!(stage.stdout, append(LOG));
    assert_eq!(stage.stderr, Sink::SameAsStdout);
    assert_eq!(stage.cwd, Some(PathBuf::from(REPO)));
}

#[test]
fn cursor_points_the_cli_at_the_live_wall_by_default() {
    let plan =
        backend("cursor").plan(&ctx(), &prompt_of(b"p".to_vec()), &request()).expect("a plan");
    assert_eq!(env(only_stage(&plan)), [("CURSOR_CONFIG_DIR", LIVE_DIR)]);
    assert!(exports(&plan).is_empty());
}

#[test]
fn cursor_points_the_cli_at_the_wall_the_run_exported() {
    let ctx = with(ctx(), "MAINTAINER_CURSOR_DIR", "/p/fx/cursor-rehearsal");
    let plan = backend("cursor").plan(&ctx, &prompt_of(b"p".to_vec()), &request()).expect("a plan");
    assert_eq!(env(only_stage(&plan)), [("CURSOR_CONFIG_DIR", "/p/fx/cursor-rehearsal")]);
}

#[test]
fn cursor_falls_back_to_the_live_wall_when_the_exported_directory_is_empty() {
    let ctx = with(ctx(), "MAINTAINER_CURSOR_DIR", "");
    let plan = backend("cursor").plan(&ctx, &prompt_of(b"p".to_vec()), &request()).expect("a plan");
    assert_eq!(env(only_stage(&plan)), [("CURSOR_CONFIG_DIR", LIVE_DIR)]);
}

#[test]
fn a_prompt_of_131071_bytes_is_passed_whole() {
    let plan = backend("cursor")
        .plan(&ctx(), &prompt_of(vec![b'x'; 131_071]), &request())
        .expect("a plan");
    let last = argv(only_stage(&plan)).last().copied().expect("an argument").len();
    assert_eq!(last, 131_071);
}

#[test]
fn a_prompt_of_131072_bytes_is_refused_with_the_log_line_of_the_bash() {
    let refusal = backend("cursor")
        .plan(&ctx(), &prompt_of(vec![b'x'; 131_072]), &request())
        .expect_err("a refusal");
    assert_eq!(refusal.log_note, TOO_LARGE);
    assert_eq!(refusal.to_string(), TOO_LARGE);
}

#[test]
fn a_prompt_of_140000_bytes_is_refused() {
    let result = backend("cursor").plan(&ctx(), &prompt_of(vec![b'x'; 140_000]), &request());
    assert_eq!(result.expect_err("a refusal").log_note, TOO_LARGE);
}

#[test]
fn the_size_limit_counts_file_bytes_not_the_argument_after_trailing_newlines_are_cut() {
    // 131071 x then one newline is 131072 bytes: refused although the argument
    // would be 131071 bytes long once the shell strips the newline.
    let mut bytes = vec![b'x'; 131_071];
    bytes.push(b'\n');
    let result = backend("cursor").plan(&ctx(), &prompt_of(bytes), &request());
    assert_eq!(result.expect_err("a refusal").log_note, TOO_LARGE);
}

#[test]
fn an_empty_prompt_is_one_empty_argument() {
    let plan = backend("cursor").plan(&ctx(), &prompt_of(Vec::new()), &request()).expect("a plan");
    let args = argv(only_stage(&plan));
    assert_eq!(args.len(), 7);
    assert_eq!(args[6], "");
}

#[test]
fn a_one_byte_prompt_is_one_argument() {
    let plan =
        backend("cursor").plan(&ctx(), &prompt_of(b"x".to_vec()), &request()).expect("a plan");
    assert_eq!(argv(only_stage(&plan))[6], "x");
}

#[test]
fn trailing_newlines_of_the_prompt_are_cut_the_way_command_substitution_cuts_them() {
    let plan = backend("cursor")
        .plan(&ctx(), &prompt_of(b"a\n\nb\n\n\n".to_vec()), &request())
        .expect("a plan");
    assert_eq!(argv(only_stage(&plan))[6], "a\n\nb");
}

#[test]
fn a_prompt_of_only_newlines_becomes_an_empty_argument() {
    let plan =
        backend("cursor").plan(&ctx(), &prompt_of(b"\n\n\n".to_vec()), &request()).expect("a plan");
    assert_eq!(argv(only_stage(&plan))[6], "");
}

#[test]
fn leading_and_inner_whitespace_of_the_prompt_is_kept() {
    let plan = backend("cursor")
        .plan(&ctx(), &prompt_of(b"\n  a \t b\n".to_vec()), &request())
        .expect("a plan");
    assert_eq!(argv(only_stage(&plan))[6], "\n  a \t b");
}

#[test]
fn null_bytes_in_the_prompt_are_dropped_as_the_shell_drops_them() {
    let plan =
        backend("cursor").plan(&ctx(), &prompt_of(b"a\0b".to_vec()), &request()).expect("a plan");
    assert_eq!(argv(only_stage(&plan))[6], "ab");
}

#[test]
fn a_prompt_that_is_not_utf8_is_refused_and_not_changed() {
    // Decision P2B-2: the bash passes the raw bytes, and an argument here is a
    // String. Refusing is the choice that does not hand the agent altered text.
    let result = backend("cursor").plan(&ctx(), &prompt_of(vec![0xff, 0xfe, b'x']), &request());
    let refusal = result.expect_err("a refusal");
    assert!(refusal.log_note.contains("not valid UTF-8"), "{}", refusal.log_note);
}

#[test]
fn a_prompt_file_that_cannot_be_read_is_refused() {
    // Decision P2B-2: the bash would run the agent with an empty prompt.
    let host = FakeHost::new().with_unreadable_file(PROMPT);
    let refusal = backend("cursor").plan(&ctx(), &host, &request()).expect_err("a refusal");
    assert!(refusal.log_note.contains("cannot read the prompt file"), "{}", refusal.log_note);
}

#[test]
fn the_model_is_passed_after_dash_m_unchanged() {
    let mut req = request();
    req.model = "auto".to_string();
    let plan = backend("cursor").plan(&ctx(), &prompt_of(b"p".to_vec()), &req).expect("a plan");
    assert_eq!(argv(only_stage(&plan))[4..6], ["-m", "auto"]);
}

// --- the shell cuts newlines at the end of a command substitution and nothing else ------

#[test]
fn a_carriage_return_before_the_final_newline_stays_in_the_argument() {
    let plan =
        backend("cursor").plan(&ctx(), &prompt_of(b"a\r\n".to_vec()), &request()).expect("a plan");
    assert_eq!(argv(only_stage(&plan))[6], "a\r");
}

#[test]
fn a_prompt_that_ends_in_a_carriage_return_keeps_it() {
    let plan =
        backend("cursor").plan(&ctx(), &prompt_of(b"a\r".to_vec()), &request()).expect("a plan");
    assert_eq!(argv(only_stage(&plan))[6], "a\r");
}

#[test]
fn only_the_trailing_newlines_of_a_crlf_prompt_are_cut() {
    let plan = backend("cursor")
        .plan(&ctx(), &prompt_of(b"a\r\n\r\n".to_vec()), &request())
        .expect("a plan");
    assert_eq!(argv(only_stage(&plan))[6], "a\r\n\r");
}

#[test]
fn a_newline_then_a_carriage_return_at_the_end_cuts_nothing() {
    let plan =
        backend("cursor").plan(&ctx(), &prompt_of(b"a\n\r".to_vec()), &request()).expect("a plan");
    assert_eq!(argv(only_stage(&plan))[6], "a\n\r");
}

#[test]
fn a_maintainer_cursor_dir_of_whitespace_is_a_directory_and_not_the_default() {
    let host = FakeHost::new().with_program("cursor-agent").without_file(" /cli-config.json");
    let ctx = with(ctx(), "MAINTAINER_CURSOR_DIR", " ");
    assert_eq!(
        backend("cursor").check(&ctx, &host),
        Err("no cursor wall at  ; run ./install.sh".to_string())
    );
    let plan = backend("cursor").plan(&ctx, &prompt_of(b"p".to_vec()), &request()).expect("a plan");
    assert_eq!(env(only_stage(&plan)), [("CURSOR_CONFIG_DIR", " ")]);
}
