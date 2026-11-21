//! The opencode backend: the six questions of `lib/backends/opencode.sh`.

mod common;

use common::{
    LOG, PROMPT, REPO, append, argv, backend, ctx, env, exports, from_file, only_stage, request,
    with,
};
use mx_backends::FakeHost;
use mx_ports::Sink;
use std::path::PathBuf;

const CONFIG: &str = "/p/fx/opencode.json";
const REHEARSAL: &str = "/p/fx/opencode-rehearsal.json";
const KEY_FILE: &str = "/home/u/.groq-staging-key";
const AUTH: &str = "/home/u/.local/share/opencode/auth.json";
const NO_CREDENTIAL: &str =
    "no Groq credential: set GROQ_API_KEY, or ~/.groq-staging-key, or run 'opencode auth login'";

fn model_ctx() -> mx_backends::BackendCtx {
    with(ctx(), "OPENCODE_MODEL", "groq/openai/gpt-oss-120b")
}

fn keyed(key: &str) -> FakeHost {
    FakeHost::new().with_file(KEY_FILE, key)
}

// --- name, rehearsal, allowed tasks --------------------------------------------------

#[test]
fn opencode_is_named_opencode() {
    assert_eq!(backend("opencode").name(), "opencode");
}

#[test]
fn opencode_declares_it_can_rehearse() {
    assert!(backend("opencode").declares_rehearsal());
}

#[test]
fn opencode_may_run_any_task() {
    assert_eq!(backend("opencode").allowed_tasks(), None);
}

#[test]
fn the_opencode_wall_is_opencode_rehearsal_json_exported_as_maintainer_settings() {
    let host = FakeHost::new().with_file(REHEARSAL, "{}");
    let wall = backend("opencode").rehearsal_wall(&ctx(), &host).expect("a wall");
    assert_eq!(wall.name, "MAINTAINER_SETTINGS");
    assert_eq!(wall.value, REHEARSAL);
}

#[test]
fn a_profile_with_no_opencode_rehearsal_config_has_no_wall() {
    let host = FakeHost::new().without_file(REHEARSAL);
    assert_eq!(backend("opencode").rehearsal_wall(&ctx(), &host), None);
}

// --- check ---------------------------------------------------------------------------

#[test]
fn opencode_check_passes_with_program_config_and_a_key_in_the_environment() {
    let host = FakeHost::new().with_program("opencode").with_file(CONFIG, "{}");
    let ctx = with(ctx(), "GROQ_API_KEY", "k");
    assert_eq!(backend("opencode").check(&ctx, &host), Ok(()));
}

#[test]
fn opencode_check_names_a_missing_program_and_how_to_install_it() {
    let host = FakeHost::new().without_program("opencode");
    assert_eq!(
        backend("opencode").check(&ctx(), &host),
        Err("opencode not on PATH (curl -fsSL https://opencode.ai/install | bash)".to_string())
    );
}

#[test]
fn opencode_check_names_the_missing_config() {
    let host = FakeHost::new().with_program("opencode").without_file(CONFIG);
    assert_eq!(
        backend("opencode").check(&ctx(), &host),
        Err("missing /p/fx/opencode.json".to_string())
    );
}

#[test]
fn opencode_check_looks_for_the_wall_the_run_exported() {
    let host = FakeHost::new().with_program("opencode").without_file(REHEARSAL);
    let ctx = with(ctx(), "MAINTAINER_SETTINGS", REHEARSAL);
    assert_eq!(
        backend("opencode").check(&ctx, &host),
        Err("missing /p/fx/opencode-rehearsal.json".to_string())
    );
}

#[test]
fn opencode_check_refuses_when_no_credential_exists_anywhere() {
    let host = FakeHost::new()
        .with_program("opencode")
        .with_file(CONFIG, "{}")
        .without_file(KEY_FILE)
        .without_file(AUTH);
    assert_eq!(backend("opencode").check(&ctx(), &host), Err(NO_CREDENTIAL.to_string()));
}

#[test]
fn opencode_check_treats_an_empty_groq_api_key_as_no_key() {
    let host = FakeHost::new()
        .with_program("opencode")
        .with_file(CONFIG, "{}")
        .without_file(KEY_FILE)
        .without_file(AUTH);
    let ctx = with(ctx(), "GROQ_API_KEY", "");
    assert_eq!(backend("opencode").check(&ctx, &host), Err(NO_CREDENTIAL.to_string()));
}

#[test]
fn opencode_check_accepts_the_key_file_alone() {
    let host = FakeHost::new()
        .with_program("opencode")
        .with_file(CONFIG, "{}")
        .with_file(KEY_FILE, "k")
        .without_file(AUTH);
    assert_eq!(backend("opencode").check(&ctx(), &host), Ok(()));
}

#[test]
fn opencode_check_accepts_a_login_in_auth_json_alone() {
    let host = FakeHost::new()
        .with_program("opencode")
        .with_file(CONFIG, "{}")
        .without_file(KEY_FILE)
        .with_file(AUTH, "{}");
    assert_eq!(backend("opencode").check(&ctx(), &host), Ok(()));
}

#[test]
fn opencode_check_accepts_a_key_file_that_exists_but_cannot_be_read() {
    // The bash tests `[ -f ]`, not readability.
    let host = FakeHost::new()
        .with_program("opencode")
        .with_file(CONFIG, "{}")
        .with_unreadable_file(KEY_FILE);
    assert_eq!(backend("opencode").check(&ctx(), &host), Ok(()));
}

#[test]
fn opencode_check_does_not_look_at_the_files_when_the_environment_has_a_key() {
    // The fake panics on an undeclared path, so a check that asked about the key
    // file or auth.json here would fail.
    let host = FakeHost::new().with_program("opencode").with_file(CONFIG, "{}");
    let ctx = with(ctx(), "GROQ_API_KEY", "k");
    assert_eq!(backend("opencode").check(&ctx, &host), Ok(()));
}

#[test]
fn opencode_check_finds_the_credential_files_under_the_home_it_was_given() {
    let mut ctx = ctx();
    ctx.home = PathBuf::from("/root");
    let host = FakeHost::new()
        .with_program("opencode")
        .with_file(CONFIG, "{}")
        .with_file("/root/.groq-staging-key", "k");
    assert_eq!(backend("opencode").check(&ctx, &host), Ok(()));
}

// --- plan ----------------------------------------------------------------------------

#[test]
fn opencode_runs_the_model_with_auto_and_the_default_format_and_reads_the_prompt_from_stdin() {
    let host = FakeHost::new().without_file(KEY_FILE);
    let ctx = with(model_ctx(), "GROQ_API_KEY", "k");
    let plan = backend("opencode").plan(&ctx, &host, &request()).expect("a plan");
    assert!(plan.log_notes.is_empty());
    assert!(exports(&plan).is_empty());
    let stage = only_stage(&plan);
    assert_eq!(
        argv(stage),
        ["opencode", "run", "--model", "groq/openai/gpt-oss-120b", "--auto", "--format", "default"]
    );
    assert_eq!(stage.stdin, from_file(PROMPT));
    assert_eq!(stage.stdout, append(LOG));
    assert_eq!(stage.stderr, Sink::SameAsStdout);
    assert_eq!(stage.cwd, Some(PathBuf::from(REPO)));
}

#[test]
fn a_model_with_a_slash_is_passed_through() {
    let mut req = request();
    req.model = "groq/other-model".to_string();
    let host = FakeHost::new().without_file(KEY_FILE);
    let plan = backend("opencode").plan(&model_ctx(), &host, &req).expect("a plan");
    assert_eq!(argv(only_stage(&plan))[3], "groq/other-model");
}

#[test]
fn a_model_without_a_slash_becomes_opencode_model() {
    let host = FakeHost::new().without_file(KEY_FILE);
    let plan = backend("opencode").plan(&model_ctx(), &host, &request()).expect("a plan");
    assert_eq!(argv(only_stage(&plan))[3], "groq/openai/gpt-oss-120b");
}

#[test]
fn a_slash_anywhere_in_the_model_counts_as_a_provider_pair() {
    let host = FakeHost::new().without_file(KEY_FILE);
    for model in ["/x", "x/", "/", "a/b/c"] {
        let mut req = request();
        req.model = model.to_string();
        let plan = backend("opencode").plan(&model_ctx(), &host, &req).expect("a plan");
        assert_eq!(argv(only_stage(&plan))[3], model);
    }
}

#[test]
fn an_empty_model_becomes_opencode_model() {
    let mut req = request();
    req.model = String::new();
    let host = FakeHost::new().without_file(KEY_FILE);
    let plan = backend("opencode").plan(&model_ctx(), &host, &req).expect("a plan");
    assert_eq!(argv(only_stage(&plan))[3], "groq/openai/gpt-oss-120b");
}

#[test]
fn an_unset_opencode_model_refuses_a_bare_model_name() {
    // Decision P2B-1: the bash `${OPENCODE_MODEL:?...}` ends the whole run.sh
    // process with status 1 and a message on standard error. A plan cannot end
    // the process, so it refuses, and the orchestrator treats that as a backend
    // failure with the reason in the log.
    let host = FakeHost::new().without_file(KEY_FILE);
    let refusal = backend("opencode").plan(&ctx(), &host, &request()).expect_err("a refusal");
    assert_eq!(refusal.log_note, "OPENCODE_MODEL: lib/defaults.env was not loaded");
}

#[test]
fn an_empty_opencode_model_refuses_a_bare_model_name() {
    let host = FakeHost::new().without_file(KEY_FILE);
    let ctx = with(ctx(), "OPENCODE_MODEL", "");
    let refusal = backend("opencode").plan(&ctx, &host, &request()).expect_err("a refusal");
    assert_eq!(refusal.log_note, "OPENCODE_MODEL: lib/defaults.env was not loaded");
}

#[test]
fn an_unset_opencode_model_does_not_matter_when_the_model_has_a_slash() {
    let mut req = request();
    req.model = "groq/x".to_string();
    let host = FakeHost::new().without_file(KEY_FILE);
    assert!(backend("opencode").plan(&ctx(), &host, &req).is_ok());
}

#[test]
fn opencode_reads_its_config_from_the_profile_by_default() {
    let host = FakeHost::new().without_file(KEY_FILE);
    let plan = backend("opencode").plan(&model_ctx(), &host, &request()).expect("a plan");
    assert_eq!(env(only_stage(&plan)), [("OPENCODE_CONFIG", CONFIG)]);
}

#[test]
fn opencode_reads_the_wall_the_run_exported_for_a_rehearsal() {
    let ctx = with(model_ctx(), "MAINTAINER_SETTINGS", REHEARSAL);
    let host = FakeHost::new().without_file(KEY_FILE);
    let plan = backend("opencode").plan(&ctx, &host, &request()).expect("a plan");
    assert_eq!(env(only_stage(&plan)), [("OPENCODE_CONFIG", REHEARSAL)]);
}

#[test]
fn opencode_falls_back_to_the_live_config_when_the_exported_wall_is_empty() {
    let ctx = with(model_ctx(), "MAINTAINER_SETTINGS", "");
    let host = FakeHost::new().without_file(KEY_FILE);
    let plan = backend("opencode").plan(&ctx, &host, &request()).expect("a plan");
    assert_eq!(env(only_stage(&plan)), [("OPENCODE_CONFIG", CONFIG)]);
}

// --- the key -------------------------------------------------------------------------

#[test]
fn a_key_in_the_environment_is_never_planned_and_the_key_file_is_not_read() {
    // Decision P2B-10: the bash agent inherits an exported key, so the executor
    // passes it on. The plan names a key only when this backend read it from the
    // file. The fake panics on a path nobody declared, so a plan that looked at
    // the key file here would fail.
    let ctx = with(model_ctx(), "GROQ_API_KEY", "key-from-env");
    let plan = backend("opencode").plan(&ctx, &FakeHost::new(), &request()).expect("a plan");
    assert_eq!(env(only_stage(&plan)), [("OPENCODE_CONFIG", CONFIG)]);
    assert!(exports(&plan).is_empty());
}

#[test]
fn a_key_of_whitespace_in_the_environment_is_a_key_and_the_file_is_not_read() {
    let ctx = with(model_ctx(), "GROQ_API_KEY", " ");
    let plan = backend("opencode").plan(&ctx, &FakeHost::new(), &request()).expect("a plan");
    assert_eq!(env(only_stage(&plan)), [("OPENCODE_CONFIG", CONFIG)]);
}

#[test]
fn the_key_file_is_read_with_every_newline_deleted_inner_ones_too() {
    let plan = backend("opencode")
        .plan(&model_ctx(), &keyed("key-from\nfile\n\n"), &request())
        .expect("a plan");
    assert_eq!(env(only_stage(&plan))[1], ("GROQ_API_KEY", "key-fromfile"));
}

#[test]
fn only_newlines_are_deleted_from_the_key_file() {
    let plan =
        backend("opencode").plan(&model_ctx(), &keyed(" k\r\n\tv \n"), &request()).expect("a plan");
    assert_eq!(env(only_stage(&plan))[1], ("GROQ_API_KEY", " k\r\tv "));
}

#[test]
fn an_empty_key_in_the_environment_is_replaced_by_the_key_file() {
    let ctx = with(model_ctx(), "GROQ_API_KEY", "");
    let plan = backend("opencode").plan(&ctx, &keyed("filekey\n"), &request()).expect("a plan");
    assert_eq!(env(only_stage(&plan))[1], ("GROQ_API_KEY", "filekey"));
}

#[test]
fn an_empty_key_file_gives_an_empty_key_variable() {
    // The bash assigns the empty output and exports it.
    let plan = backend("opencode").plan(&model_ctx(), &keyed("\n"), &request()).expect("a plan");
    assert_eq!(env(only_stage(&plan))[1], ("GROQ_API_KEY", ""));
}

#[test]
fn with_no_key_anywhere_no_key_variable_reaches_the_stage() {
    let host = FakeHost::new().without_file(KEY_FILE);
    let plan = backend("opencode").plan(&model_ctx(), &host, &request()).expect("a plan");
    assert!(env(only_stage(&plan)).iter().all(|(name, _)| *name != "GROQ_API_KEY"));
}

#[test]
fn an_empty_key_in_the_environment_with_no_file_is_left_to_the_executor() {
    let ctx = with(model_ctx(), "GROQ_API_KEY", "");
    let host = FakeHost::new().without_file(KEY_FILE);
    let plan = backend("opencode").plan(&ctx, &host, &request()).expect("a plan");
    assert!(env(only_stage(&plan)).iter().all(|(name, _)| *name != "GROQ_API_KEY"));
}

#[test]
fn a_key_file_that_cannot_be_read_leaves_the_key_as_the_environment_had_it() {
    let host = FakeHost::new().with_unreadable_file(KEY_FILE);
    let plan = backend("opencode").plan(&model_ctx(), &host, &request()).expect("a plan");
    assert!(env(only_stage(&plan)).iter().all(|(name, _)| *name != "GROQ_API_KEY"));
}

#[test]
fn the_key_file_is_found_under_the_home_the_run_has() {
    let mut ctx = model_ctx();
    ctx.home = PathBuf::from("/root");
    let host = FakeHost::new().with_file("/root/.groq-staging-key", "rootkey\n");
    let plan = backend("opencode").plan(&ctx, &host, &request()).expect("a plan");
    assert_eq!(env(only_stage(&plan))[1], ("GROQ_API_KEY", "rootkey"));
}

#[test]
fn null_bytes_in_the_key_file_are_dropped_as_the_shell_drops_them() {
    let plan =
        backend("opencode").plan(&model_ctx(), &keyed("ab\0cd\n"), &request()).expect("a plan");
    assert_eq!(env(only_stage(&plan))[1], ("GROQ_API_KEY", "abcd"));
}

#[test]
fn a_key_file_that_is_not_utf8_refuses_without_printing_the_key() {
    let host = FakeHost::new().with_file(KEY_FILE, vec![b'g', b's', b'k', 0xff, b'Z', b'Q', b'9']);
    let refusal = backend("opencode").plan(&model_ctx(), &host, &request()).expect_err("a refusal");
    assert!(refusal.log_note.contains("not valid UTF-8"), "{}", refusal.log_note);
    assert!(!refusal.log_note.contains("ZQ9"));
    assert!(!refusal.log_note.contains("gsk"));
}

#[test]
fn the_key_is_in_no_argument_and_no_export() {
    // Decision P2B-7: the bash exports the key it read into the whole run, and
    // the plan gives it to the opencode stage only.
    let plan = backend("opencode")
        .plan(&model_ctx(), &keyed("gsk_supersecret_value\n"), &request())
        .expect("a plan");
    assert_eq!(env(only_stage(&plan))[1], ("GROQ_API_KEY", "gsk_supersecret_value"));
    for stage in &plan.pipeline.stages {
        assert!(stage.argv.iter().all(|arg| !arg.contains("supersecret")));
    }
    assert!(plan.exports.is_empty());
}

#[test]
fn the_key_is_not_in_the_debug_print_of_the_plan_the_context_or_the_host() {
    let key = "gsk_supersecret_value";
    let ctx = with(model_ctx(), "GROQ_API_KEY", key);
    let host = FakeHost::new().with_file(KEY_FILE, key);
    let plan = backend("opencode").plan(&ctx, &host, &request()).expect("a plan");
    for shown in [format!("{plan:?}"), format!("{ctx:?}"), format!("{host:?}")] {
        assert!(!shown.contains("supersecret"), "{shown}");
    }
    assert!(format!("{ctx:?}").contains("GROQ_API_KEY"), "names are still shown");
}

#[test]
fn the_key_read_from_the_file_is_not_in_the_debug_print_of_the_plan() {
    let plan = backend("opencode")
        .plan(&model_ctx(), &keyed("gsk_filesecret_value\n"), &request())
        .expect("a plan");
    assert_eq!(env(only_stage(&plan))[1].1, "gsk_filesecret_value");
    assert!(!format!("{plan:?}").contains("filesecret"));
}

// --- values made of whitespace are values (the shell's `${NAME:-}` tests for empty only) --

#[test]
fn an_opencode_model_of_whitespace_is_a_model() {
    let mut req = request();
    req.model = "sonnet".to_string();
    let ctx = with(ctx(), "OPENCODE_MODEL", " ");
    let host = FakeHost::new().without_file(KEY_FILE);
    let plan = backend("opencode").plan(&ctx, &host, &req).expect("a plan");
    assert_eq!(argv(only_stage(&plan))[3], " ");
}

#[test]
fn an_opencode_model_of_a_tab_and_a_space_is_a_model() {
    let ctx = with(ctx(), "OPENCODE_MODEL", "\t ");
    let host = FakeHost::new().without_file(KEY_FILE);
    let plan = backend("opencode").plan(&ctx, &host, &request()).expect("a plan");
    assert_eq!(argv(only_stage(&plan))[3], "\t ");
}

#[test]
fn a_maintainer_settings_of_whitespace_is_a_path() {
    let host = FakeHost::new().with_program("opencode").without_file(" ");
    let ctx = with(ctx(), "MAINTAINER_SETTINGS", " ");
    assert_eq!(backend("opencode").check(&ctx, &host), Err("missing  ".to_string()));
    let host = FakeHost::new().without_file(KEY_FILE);
    let plan =
        backend("opencode").plan(&with(model_ctx(), "MAINTAINER_SETTINGS", " "), &host, &request());
    assert_eq!(env(only_stage(&plan.expect("a plan")))[0], ("OPENCODE_CONFIG", " "));
}

#[test]
fn a_groq_api_key_of_whitespace_is_a_credential() {
    let host = FakeHost::new().with_program("opencode").with_file(CONFIG, "{}");
    let ctx = with(ctx(), "GROQ_API_KEY", " ");
    assert_eq!(backend("opencode").check(&ctx, &host), Ok(()));
}
