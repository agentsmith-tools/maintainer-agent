//! The Claude Code backend: the six questions of `lib/backends/claude.sh`.

mod common;

use common::{
    LOG, PROMPT, REPO, STATE, append, argv, backend, ctx, env, exports, from_file, only_stage,
    request, with,
};
use mx_backends::FakeHost;
use mx_ports::{Sink, Source};
use std::path::PathBuf;

const SETTINGS: &str = "/p/fx/settings.json";
const REHEARSAL: &str = "/p/fx/settings-rehearsal.json";
const FILTER: &str = "/tree/scripts/transcript.py";
const LIVE_ARGV: [&str; 11] = [
    "claude",
    "-p",
    "--settings",
    SETTINGS,
    "--permission-mode",
    "bypassPermissions",
    "--model",
    "opus",
    "--add-dir",
    STATE,
    "--output-format",
];

fn host_with_filter() -> FakeHost {
    FakeHost::new().with_file(FILTER, "")
}

// --- name, rehearsal, allowed tasks --------------------------------------------------

#[test]
fn claude_is_named_claude() {
    assert_eq!(backend("claude").name(), "claude");
}

#[test]
fn claude_declares_it_can_rehearse() {
    assert!(backend("claude").declares_rehearsal());
}

#[test]
fn claude_may_run_any_task() {
    assert_eq!(backend("claude").allowed_tasks(), None);
}

#[test]
fn the_claude_wall_is_settings_rehearsal_json_in_the_profile_and_the_variable_is_maintainer_settings()
 {
    let host = FakeHost::new().with_file(REHEARSAL, "{}");
    let wall = backend("claude").rehearsal_wall(&ctx(), &host).expect("a wall");
    assert_eq!(wall.name, "MAINTAINER_SETTINGS");
    assert_eq!(wall.value, REHEARSAL);
}

#[test]
fn a_profile_with_no_rehearsal_settings_has_no_claude_wall() {
    let host = FakeHost::new().without_file(REHEARSAL);
    assert_eq!(backend("claude").rehearsal_wall(&ctx(), &host), None);
}

#[test]
fn the_claude_wall_follows_a_profile_dir_that_ends_in_a_slash() {
    let mut ctx = ctx();
    ctx.profile_dir = PathBuf::from("/p/fx/");
    let host = FakeHost::new().with_file("/p/fx//settings-rehearsal.json", "{}");
    let wall = backend("claude").rehearsal_wall(&ctx, &host).expect("a wall");
    assert_eq!(wall.value, "/p/fx//settings-rehearsal.json");
}

// --- check ---------------------------------------------------------------------------

#[test]
fn claude_check_passes_when_claude_and_the_settings_exist() {
    let host = FakeHost::new().with_program("claude").with_file(SETTINGS, "{}");
    assert_eq!(backend("claude").check(&ctx(), &host), Ok(()));
}

#[test]
fn claude_check_names_a_missing_program() {
    let host = FakeHost::new().without_program("claude");
    assert_eq!(backend("claude").check(&ctx(), &host), Err("claude not on PATH".to_string()));
}

#[test]
fn claude_check_names_the_missing_settings_file() {
    let host = FakeHost::new().with_program("claude").without_file(SETTINGS);
    assert_eq!(
        backend("claude").check(&ctx(), &host),
        Err("missing /p/fx/settings.json".to_string())
    );
}

#[test]
fn claude_check_reads_the_wall_the_run_exported_instead_of_the_live_settings() {
    let host = FakeHost::new().with_program("claude").without_file(REHEARSAL);
    let ctx = with(ctx(), "MAINTAINER_SETTINGS", REHEARSAL);
    assert_eq!(
        backend("claude").check(&ctx, &host),
        Err("missing /p/fx/settings-rehearsal.json".to_string())
    );
}

#[test]
fn claude_check_treats_an_empty_maintainer_settings_as_unset() {
    let host = FakeHost::new().with_program("claude").with_file(SETTINGS, "{}");
    let ctx = with(ctx(), "MAINTAINER_SETTINGS", "");
    assert_eq!(backend("claude").check(&ctx, &host), Ok(()));
}

// --- plan: the pipeline with the transcript filter -----------------------------------

#[test]
fn claude_runs_stream_json_through_the_filter_when_transcript_py_exists() {
    let plan = backend("claude").plan(&ctx(), &host_with_filter(), &request()).expect("a plan");
    assert!(plan.log_notes.is_empty());
    assert_eq!(plan.pipeline.stages.len(), 2);
    let first = &plan.pipeline.stages.as_slice()[0];
    let mut want = LIVE_ARGV.to_vec();
    want.extend(["stream-json", "--verbose"]);
    assert_eq!(argv(first), want);
}

#[test]
fn claude_reads_the_prompt_file_and_pipes_stdout_to_the_filter_and_stderr_to_the_log() {
    let plan = backend("claude").plan(&ctx(), &host_with_filter(), &request()).expect("a plan");
    let first = &plan.pipeline.stages.as_slice()[0];
    assert_eq!(first.stdin, from_file(PROMPT));
    assert_eq!(first.stdout, Sink::NextStage);
    assert_eq!(first.stderr, append(LOG));
}

#[test]
fn the_filter_stage_gets_python3_the_filter_the_log_and_the_commands_file() {
    let plan = backend("claude").plan(&ctx(), &host_with_filter(), &request()).expect("a plan");
    let second = &plan.pipeline.stages.as_slice()[1];
    assert_eq!(argv(second), ["python3", FILTER, LOG, "/state/logs/run.commands"]);
    assert_eq!(second.stdin, Source::PreviousStage);
}

#[test]
fn the_filter_stage_keeps_its_standard_output_off_the_log_and_puts_its_errors_in_it() {
    // Decision P2B-3: the bash lets the filter inherit the run's streams, and no
    // sink means "inherit". Its output is discarded, and a crash lands in the log.
    let plan = backend("claude").plan(&ctx(), &host_with_filter(), &request()).expect("a plan");
    let second = &plan.pipeline.stages.as_slice()[1];
    assert_eq!(second.stdout, Sink::Null);
    assert_eq!(second.stderr, append(LOG));
}

#[test]
fn every_claude_stage_runs_in_the_review_checkout() {
    let plan = backend("claude").plan(&ctx(), &host_with_filter(), &request()).expect("a plan");
    for stage in &plan.pipeline.stages {
        assert_eq!(stage.cwd, Some(PathBuf::from(REPO)));
    }
}

#[test]
fn the_commands_file_replaces_the_dot_log_suffix_only_at_the_end() {
    let mut req = request();
    for (log, want) in [
        ("/state/logs/run.log", "/state/logs/run.commands"),
        ("/state/logs/run.txt", "/state/logs/run.txt.commands"),
        ("/state/logs/a.log.log", "/state/logs/a.log.commands"),
        ("/state/logs/a.log.x", "/state/logs/a.log.x.commands"),
        ("/state/logs/log", "/state/logs/log.commands"),
        ("/state/log dir/my run.log", "/state/log dir/my run.commands"),
    ] {
        req.log = PathBuf::from(log);
        let plan = backend("claude").plan(&ctx(), &host_with_filter(), &req).expect("a plan");
        let second = &plan.pipeline.stages.as_slice()[1];
        assert_eq!(argv(second)[3], want, "log {log}");
    }
}

#[test]
fn a_log_path_with_spaces_and_quotes_stays_one_argument() {
    let mut req = request();
    req.log = PathBuf::from("/state/it's a \"log\"/run.log");
    let plan = backend("claude").plan(&ctx(), &host_with_filter(), &req).expect("a plan");
    let second = &plan.pipeline.stages.as_slice()[1];
    assert_eq!(argv(second).len(), 4);
    assert_eq!(argv(second)[2], "/state/it's a \"log\"/run.log");
    assert_eq!(argv(second)[3], "/state/it's a \"log\"/run.commands");
}

// --- plan: the fallback with no filter -----------------------------------------------

#[test]
fn claude_without_transcript_py_says_so_in_the_log_and_runs_one_text_stage() {
    let host = FakeHost::new().without_file(FILTER);
    let plan = backend("claude").plan(&ctx(), &host, &request()).expect("a plan");
    assert_eq!(
        plan.log_notes,
        vec!["transcript.py not found at /tree/scripts/transcript.py; this run records prose only"]
    );
    let stage = only_stage(&plan);
    assert_eq!(argv(stage), LIVE_ARGV[..10].to_vec());
    assert_eq!(stage.stdin, from_file(PROMPT));
    assert_eq!(stage.stdout, append(LOG));
    assert_eq!(stage.stderr, Sink::SameAsStdout);
    assert_eq!(stage.cwd, Some(PathBuf::from(REPO)));
}

#[test]
fn claude_with_no_scripts_directory_names_a_filter_at_the_filesystem_root() {
    let mut ctx = ctx();
    ctx.scripts_dir = None;
    let host = FakeHost::new().without_file("/transcript.py");
    let plan = backend("claude").plan(&ctx, &host, &request()).expect("a plan");
    assert_eq!(
        plan.log_notes,
        vec!["transcript.py not found at /transcript.py; this run records prose only"]
    );
}

#[test]
fn claude_with_an_empty_scripts_directory_looks_for_the_filter_at_the_filesystem_root() {
    let mut ctx = ctx();
    ctx.scripts_dir = Some(PathBuf::new());
    let host = FakeHost::new().with_file("/transcript.py", "");
    let plan = backend("claude").plan(&ctx, &host, &request()).expect("a plan");
    assert_eq!(plan.pipeline.stages.len(), 2);
    assert_eq!(argv(&plan.pipeline.stages.as_slice()[1])[1], "/transcript.py");
}

// --- plan: arguments that vary -------------------------------------------------------

#[test]
fn claude_passes_the_model_and_the_state_directory_it_was_given() {
    let mut req = request();
    req.model = "sonnet".to_string();
    req.extra_dir = PathBuf::from("/var/lib/m state");
    let plan = backend("claude").plan(&ctx(), &host_with_filter(), &req).expect("a plan");
    let args = argv(&plan.pipeline.stages.as_slice()[0]);
    assert_eq!(args[6..10], ["--model", "sonnet", "--add-dir", "/var/lib/m state"]);
}

#[test]
fn claude_uses_the_settings_the_run_exported_for_a_rehearsal() {
    let ctx = with(ctx(), "MAINTAINER_SETTINGS", REHEARSAL);
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert_eq!(argv(&plan.pipeline.stages.as_slice()[0])[3], REHEARSAL);
}

#[test]
fn claude_falls_back_to_the_live_settings_when_the_exported_one_is_empty() {
    let ctx = with(ctx(), "MAINTAINER_SETTINGS", "");
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert_eq!(argv(&plan.pipeline.stages.as_slice()[0])[3], SETTINGS);
}

#[test]
fn a_prompt_file_and_a_model_with_odd_characters_pass_through_unchanged() {
    let mut req = request();
    req.prompt_file = PathBuf::from("/state/it's a prompt.txt");
    req.model = "m o\"d'el".to_string();
    let plan = backend("claude").plan(&ctx(), &host_with_filter(), &req).expect("a plan");
    let first = &plan.pipeline.stages.as_slice()[0];
    assert_eq!(first.stdin, from_file("/state/it's a prompt.txt"));
    assert_eq!(argv(first)[7], "m o\"d'el");
}

// --- plan: thinking budget and subagent model ----------------------------------------

#[test]
fn the_task_thinking_budget_becomes_max_thinking_tokens() {
    let ctx = with(ctx(), "THINKING_review", "31999");
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert_eq!(env(&plan.pipeline.stages.as_slice()[0]), [("MAX_THINKING_TOKENS", "31999")]);
}

#[test]
fn the_task_thinking_budget_is_exported_to_the_run_as_well_as_set_on_the_stage() {
    let ctx = with(ctx(), "THINKING_review", "31999");
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert_eq!(exports(&plan), [("MAX_THINKING_TOKENS", "31999")]);
}

#[test]
fn the_general_thinking_budget_is_the_fallback() {
    let ctx = with(ctx(), "MAINTAINER_THINKING", "777");
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert_eq!(env(&plan.pipeline.stages.as_slice()[0]), [("MAX_THINKING_TOKENS", "777")]);
}

#[test]
fn the_task_thinking_budget_wins_over_the_general_one() {
    let ctx = with(with(ctx(), "MAINTAINER_THINKING", "777"), "THINKING_review", "31999");
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert_eq!(exports(&plan), [("MAX_THINKING_TOKENS", "31999")]);
}

#[test]
fn an_empty_task_thinking_budget_falls_back_to_the_general_one() {
    let ctx = with(with(ctx(), "MAINTAINER_THINKING", "777"), "THINKING_review", "");
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert_eq!(exports(&plan), [("MAX_THINKING_TOKENS", "777")]);
}

#[test]
fn a_thinking_budget_for_another_task_is_ignored() {
    let ctx = with(ctx(), "THINKING_issues", "5000");
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert!(exports(&plan).is_empty());
    assert!(env(&plan.pipeline.stages.as_slice()[0]).is_empty());
}

#[test]
fn with_no_thinking_budget_nothing_is_exported() {
    let plan = backend("claude").plan(&ctx(), &host_with_filter(), &request()).expect("a plan");
    assert!(exports(&plan).is_empty());
}

#[test]
fn an_empty_thinking_budget_everywhere_exports_nothing() {
    let ctx = with(with(ctx(), "MAINTAINER_THINKING", ""), "THINKING_review", "");
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert!(exports(&plan).is_empty());
}

#[test]
fn the_task_subagent_model_becomes_claude_code_subagent_model() {
    let ctx = with(ctx(), "SUBAGENT_MODEL_review", "sonnet");
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert_eq!(
        env(&plan.pipeline.stages.as_slice()[0]),
        [("CLAUDE_CODE_SUBAGENT_MODEL", "sonnet")]
    );
    assert_eq!(exports(&plan), [("CLAUDE_CODE_SUBAGENT_MODEL", "sonnet")]);
}

#[test]
fn a_subagent_model_for_another_task_and_an_empty_one_export_nothing() {
    let ctx = with(with(ctx(), "SUBAGENT_MODEL_issues", "haiku"), "SUBAGENT_MODEL_review", "");
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert!(exports(&plan).is_empty());
}

#[test]
fn thinking_comes_before_the_subagent_model_in_the_environment() {
    let ctx = with(with(ctx(), "THINKING_review", "9"), "SUBAGENT_MODEL_review", "sonnet");
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert_eq!(
        env(&plan.pipeline.stages.as_slice()[0]),
        [("MAX_THINKING_TOKENS", "9"), ("CLAUDE_CODE_SUBAGENT_MODEL", "sonnet")]
    );
}

#[test]
fn the_variables_are_looked_up_under_the_name_of_the_task_being_run() {
    let mut ctx = with(ctx(), "THINKING_issues", "5000");
    ctx.task = "issues".to_string();
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert_eq!(exports(&plan), [("MAX_THINKING_TOKENS", "5000")]);
}

#[test]
fn the_claude_pipeline_has_a_deadline() {
    let plan = backend("claude").plan(&ctx(), &host_with_filter(), &request()).expect("a plan");
    assert_eq!(plan.pipeline.timeout.as_secs(), 10_800);
}

#[test]
fn claude_asks_the_host_about_the_filter_and_nothing_else_when_planning() {
    // FakeHost panics on an undeclared path, so a plan that read another file
    // (the prompt, the settings) would fail here.
    let host = FakeHost::new().with_file(FILTER, "");
    let _ = backend("claude").plan(&ctx(), &host, &request()).expect("a plan");
}

// --- whitespace is a value: the shell's `${NAME:-}` tests for empty only ---------------

#[test]
fn a_thinking_budget_of_whitespace_is_exported_as_it_is() {
    let ctx = with(ctx(), "THINKING_review", " ");
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert_eq!(exports(&plan), [("MAX_THINKING_TOKENS", " ")]);
}

#[test]
fn a_general_thinking_budget_of_a_tab_is_the_fallback_for_an_empty_task_budget() {
    let ctx = with(with(ctx(), "THINKING_review", ""), "MAINTAINER_THINKING", "\t");
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert_eq!(exports(&plan), [("MAX_THINKING_TOKENS", "\t")]);
}

#[test]
fn a_subagent_model_of_whitespace_is_exported_as_it_is() {
    let ctx = with(ctx(), "SUBAGENT_MODEL_review", " ");
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert_eq!(exports(&plan), [("CLAUDE_CODE_SUBAGENT_MODEL", " ")]);
}

#[test]
fn a_maintainer_settings_of_whitespace_is_a_path_and_not_the_default() {
    let host = FakeHost::new().with_program("claude").without_file(" ");
    let ctx = with(ctx(), "MAINTAINER_SETTINGS", " ");
    assert_eq!(backend("claude").check(&ctx, &host), Err("missing  ".to_string()));
    let plan = backend("claude").plan(&ctx, &host_with_filter(), &request()).expect("a plan");
    assert_eq!(argv(&plan.pipeline.stages.as_slice()[0])[3], " ");
}
