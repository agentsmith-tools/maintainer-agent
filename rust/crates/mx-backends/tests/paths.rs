//! Paths that are not UTF-8 (decision P2B-12).
//!
//! The bash runs with any bytes in a path. An argument or an environment value in
//! the pipeline types is a `String`, and a lossy conversion would start the agent
//! on a path the profile never named. So a plan that has to put such a path in an
//! argument refuses, a wall whose path cannot be exported is not rendered, and a
//! stage that only runs in such a directory runs.

mod common;

use common::{PROFILE, backend, ctx, request};
use mx_backends::FakeHost;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

fn raw(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes.to_vec()))
}

fn bad_repo() -> mx_backends::BackendCtx {
    let mut ctx = ctx();
    ctx.repo_path = raw(b"/state/re\xffpo");
    ctx
}

fn bad_profile() -> mx_backends::BackendCtx {
    let mut ctx = ctx();
    ctx.profile_dir = raw(b"/p/f\xffx");
    ctx
}

#[test]
fn codex_refuses_a_checkout_path_that_is_not_utf8_because_it_is_an_argument() {
    let refusal =
        backend("codex").plan(&bad_repo(), &FakeHost::new(), &request()).expect_err("a refusal");
    assert!(refusal.log_note.contains("not valid UTF-8"), "{}", refusal.log_note);
}

#[test]
fn claude_runs_in_a_checkout_path_that_is_not_utf8_because_it_is_only_a_working_directory() {
    let host = FakeHost::new().with_file("/tree/scripts/transcript.py", "");
    let plan = backend("claude").plan(&bad_repo(), &host, &request()).expect("a plan");
    for stage in &plan.pipeline.stages {
        assert_eq!(stage.cwd, Some(raw(b"/state/re\xffpo")));
    }
}

#[test]
fn cursor_and_opencode_run_in_a_checkout_path_that_is_not_utf8() {
    let host = FakeHost::new()
        .with_file("/state/prompt.txt", "p")
        .without_file("/home/u/.groq-staging-key");
    let cursor = backend("cursor").plan(&bad_repo(), &host, &request()).expect("a cursor plan");
    assert_eq!(cursor.pipeline.stages.first().cwd, Some(raw(b"/state/re\xffpo")));
    let mut with_model = bad_repo();
    with_model.vars.insert("OPENCODE_MODEL".to_string(), "groq/x".to_string());
    let opencode =
        backend("opencode").plan(&with_model, &host, &request()).expect("an opencode plan");
    assert_eq!(opencode.pipeline.stages.first().cwd, Some(raw(b"/state/re\xffpo")));
}

#[test]
fn claude_refuses_a_profile_directory_that_is_not_utf8_because_the_settings_are_an_argument() {
    let host = FakeHost::new().with_file("/tree/scripts/transcript.py", "");
    let refusal = backend("claude").plan(&bad_profile(), &host, &request()).expect_err("a refusal");
    assert!(refusal.log_note.contains("not valid UTF-8"), "{}", refusal.log_note);
}

#[test]
fn opencode_refuses_a_profile_directory_that_is_not_utf8_because_the_config_is_an_environment_value()
 {
    let mut ctx = bad_profile();
    ctx.vars.insert("OPENCODE_MODEL".to_string(), "groq/x".to_string());
    let host = FakeHost::new().without_file("/home/u/.groq-staging-key");
    let refusal = backend("opencode").plan(&ctx, &host, &request()).expect_err("a refusal");
    assert!(refusal.log_note.contains("not valid UTF-8"), "{}", refusal.log_note);
}

#[test]
fn a_wall_under_a_profile_directory_that_is_not_utf8_is_not_rendered() {
    // The wall path would have to become a `String` for MAINTAINER_SETTINGS or
    // MAINTAINER_CURSOR_DIR. A lossy path names a file that does not exist, so the
    // wall counts as missing and the orchestrator refuses a rehearsal.
    let profile = raw(b"/p/f\xffx");
    let host = FakeHost::new()
        .with_file(profile.join("settings-rehearsal.json"), "{}")
        .with_file(profile.join("opencode-rehearsal.json"), "{}")
        .with_file(profile.join("cursor-rehearsal/cli-config.json"), "{}");
    let mut ctx = ctx();
    ctx.profile_dir = profile;
    for name in ["claude", "cursor", "opencode"] {
        assert_eq!(backend(name).rehearsal_wall(&ctx, &host), None, "{name}");
    }
}

#[test]
fn the_same_walls_under_a_utf8_profile_directory_are_rendered() {
    // The control for the test above: the fake does hold these files.
    let host = FakeHost::new()
        .with_file(format!("{PROFILE}/settings-rehearsal.json"), "{}")
        .with_file(format!("{PROFILE}/opencode-rehearsal.json"), "{}")
        .with_file(format!("{PROFILE}/cursor-rehearsal/cli-config.json"), "{}");
    for name in ["claude", "cursor", "opencode"] {
        assert!(backend(name).rehearsal_wall(&ctx(), &host).is_some(), "{name}");
    }
}

#[test]
fn a_message_that_names_a_path_that_is_not_utf8_shows_the_replacement_character() {
    // Only messages for a person show a lossy path; arguments never do.
    let host = FakeHost::new().with_program("claude").without_file(raw(b"/p/f\xffx/settings.json"));
    let err = backend("claude").check(&bad_profile(), &host).expect_err("a refusal");
    assert_eq!(err, "missing /p/f\u{fffd}x/settings.json");
}
