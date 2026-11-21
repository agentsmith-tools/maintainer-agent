//! The Codex backend: the six questions of `lib/backends/codex.sh`.

mod common;

use common::{
    LOG, PROMPT, REPO, append, argv, backend, ctx, env, exports, from_file, only_stage, request,
    with,
};
use mx_backends::FakeHost;
use mx_ports::Sink;
use std::path::PathBuf;

#[test]
fn codex_is_named_codex() {
    assert_eq!(backend("codex").name(), "codex");
}

#[test]
fn codex_declares_no_rehearsal_so_a_post_off_run_is_refused() {
    assert!(!backend("codex").declares_rehearsal());
}

#[test]
fn codex_has_no_rehearsal_wall_even_when_a_settings_file_exists() {
    // The FakeHost declares nothing: a wall lookup that asked it would panic.
    assert_eq!(backend("codex").rehearsal_wall(&ctx(), &FakeHost::new()), None);
}

#[test]
fn codex_may_run_any_task() {
    assert_eq!(backend("codex").allowed_tasks(), None);
}

#[test]
fn codex_check_passes_when_codex_is_on_the_path() {
    let host = FakeHost::new().with_program("codex");
    assert_eq!(backend("codex").check(&ctx(), &host), Ok(()));
}

#[test]
fn codex_check_names_a_missing_program() {
    let host = FakeHost::new().without_program("codex");
    assert_eq!(backend("codex").check(&ctx(), &host), Err("codex not on PATH".to_string()));
}

#[test]
fn codex_runs_exec_in_the_workspace_write_sandbox_with_the_prompt_on_stdin() {
    let plan = backend("codex").plan(&ctx(), &FakeHost::new(), &request()).expect("a plan");
    assert!(plan.log_notes.is_empty());
    assert!(exports(&plan).is_empty());
    let stage = only_stage(&plan);
    assert_eq!(
        argv(stage),
        [
            "codex",
            "exec",
            "--sandbox",
            "workspace-write",
            "-C",
            REPO,
            "-m",
            "opus",
            "--skip-git-repo-check",
            "-"
        ]
    );
    assert_eq!(stage.stdin, from_file(PROMPT));
}

#[test]
fn codex_appends_both_streams_to_the_log() {
    let plan = backend("codex").plan(&ctx(), &FakeHost::new(), &request()).expect("a plan");
    let stage = only_stage(&plan);
    assert_eq!(stage.stdout, append(LOG));
    assert_eq!(stage.stderr, Sink::SameAsStdout);
}

#[test]
fn codex_adds_nothing_to_the_environment_whatever_the_profile_sets() {
    let ctx = with(with(ctx(), "THINKING_review", "9"), "GROQ_API_KEY", "k");
    let plan = backend("codex").plan(&ctx, &FakeHost::new(), &request()).expect("a plan");
    assert!(env(only_stage(&plan)).is_empty());
    assert!(exports(&plan).is_empty());
}

#[test]
fn codex_takes_the_model_and_the_checkout_it_was_given() {
    let mut ctx = ctx();
    ctx.repo_path = PathBuf::from("/work/it's a repo");
    let mut req = request();
    req.model = "gpt-5".to_string();
    let plan = backend("codex").plan(&ctx, &FakeHost::new(), &req).expect("a plan");
    let args = argv(only_stage(&plan));
    assert_eq!(args[4..8], ["-C", "/work/it's a repo", "-m", "gpt-5"]);
    assert_eq!(only_stage(&plan).cwd, Some(PathBuf::from("/work/it's a repo")));
}

#[test]
fn codex_ignores_the_extra_readable_directory() {
    let mut req = request();
    req.extra_dir = PathBuf::from("/somewhere/else");
    let plan = backend("codex").plan(&ctx(), &FakeHost::new(), &req).expect("a plan");
    assert!(!argv(only_stage(&plan)).contains(&"/somewhere/else"));
}
