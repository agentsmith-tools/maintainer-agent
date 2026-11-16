//! `load_for_doctor`: what `bin/maintainer-doctor` does before its first check.
//! It names its own profile, so a profile with no `profile.env` is not a
//! refusal, and it never derives the five exported names.

mod common;

use common::{Scratch, real_proc};
use mx_profile::{LoadEnv, LoadError, load_for_doctor};
use std::path::PathBuf;

const DEFAULTS: &str = "LOCK_WAIT=3600\nRETENTION_DAYS=90\nDENY_WALL_MIN_RULES=100\n";

/// An environment the way the doctor builds it: HOME, and every other variable
/// of the caller in the `maintainer` list, which the loader passes to the child.
fn env(scratch: &Scratch, extra: &[(&str, &str)]) -> LoadEnv {
    LoadEnv {
        lib_dir: scratch.path().join("lib"),
        home: Some(scratch.path().join("home").display().to_string()),
        profile_dir: None,
        maintainer: extra.iter().map(|(n, v)| ((*n).to_string(), (*v).to_string())).collect(),
        eval_timeout: std::time::Duration::from_secs(30),
    }
}

fn tree(tag: &str) -> (Scratch, PathBuf) {
    let scratch = Scratch::new(tag);
    scratch.write("lib/defaults.env", DEFAULTS);
    std::fs::create_dir_all(scratch.path().join("home")).unwrap();
    let profile_env = scratch.path().join("profile.env");
    (scratch, profile_env)
}

#[test]
fn a_missing_profile_env_loads_the_defaults_and_is_not_a_refusal() {
    let (scratch, profile_env) = tree("doc-missing");
    let loaded = load_for_doctor(&real_proc(), &env(&scratch, &[]), &profile_env);
    let profile = loaded.result.unwrap();
    assert_eq!(profile.get_str("LOCK_WAIT"), Some("3600"));
    assert!(profile.exported().is_empty(), "the doctor derives no exported names");
}

#[test]
fn the_profile_overrides_a_default_and_a_maintainer_variable_overrides_the_profile() {
    let (scratch, profile_env) = tree("doc-override");
    scratch.write("profile.env", "LOCK_WAIT=10\nGH_ACCOUNT=me\nRETENTION_DAYS=5\n");
    let loaded = load_for_doctor(
        &real_proc(),
        &env(&scratch, &[("MAINTAINER_RETENTION_DAYS", "7"), ("MAINTAINER_GH_ACCOUNT", "ignored")]),
        &profile_env,
    );
    let profile = loaded.result.unwrap();
    assert_eq!(profile.get_str("LOCK_WAIT"), Some("10"));
    assert_eq!(profile.get_str("RETENTION_DAYS"), Some("7"));
    assert_eq!(
        profile.get_str("GH_ACCOUNT"),
        Some("me"),
        "an override applies only to a key defaults.env declares"
    );
}

#[test]
fn an_ambient_variable_the_files_do_not_set_is_visible_to_the_report() {
    let (scratch, profile_env) = tree("doc-ambient");
    scratch.write("profile.env", "POST=on\n");
    let loaded = load_for_doctor(
        &real_proc(),
        &env(&scratch, &[("BACKEND", "codex"), ("POST", "off")]),
        &profile_env,
    );
    let profile = loaded.result.unwrap();
    assert_eq!(profile.get_str("BACKEND"), Some("codex"));
    assert_eq!(profile.get_str("POST"), Some("on"), "the profile file wins over the ambient value");
}

#[test]
fn no_defaults_env_anywhere_is_the_library_refusal() {
    let scratch = Scratch::new("doc-nodefaults");
    std::fs::create_dir_all(scratch.path().join("home")).unwrap();
    std::fs::create_dir_all(scratch.path().join("lib")).unwrap();
    let loaded =
        load_for_doctor(&real_proc(), &env(&scratch, &[]), &scratch.path().join("profile.env"));
    assert!(matches!(loaded.result, Err(LoadError::DefaultsMissing)));
}

#[test]
fn a_defaults_file_whose_last_command_fails_ends_the_load_without_a_word() {
    let (scratch, profile_env) = tree("doc-defaults-fail");
    scratch.write("lib/defaults.env", "A=1\nfalse\n");
    let loaded = load_for_doctor(&real_proc(), &env(&scratch, &[]), &profile_env);
    assert!(matches!(loaded.result, Err(LoadError::DefaultsFailed { status: 1 })));
}

#[test]
fn what_the_profile_prints_comes_back_for_the_caller_to_replay() {
    let (scratch, profile_env) = tree("doc-prints");
    scratch.write("profile.env", "echo hello\necho oops >&2\n");
    let loaded = load_for_doctor(&real_proc(), &env(&scratch, &[]), &profile_env);
    assert_eq!(loaded.stdout, b"hello\n");
    assert_eq!(loaded.stderr, b"oops\n");
    assert!(loaded.result.is_ok());
}
