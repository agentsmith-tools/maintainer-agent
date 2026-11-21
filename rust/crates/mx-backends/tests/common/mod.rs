//! Fixtures shared by the unit tests of the four backends.
#![allow(dead_code, reason = "each test binary uses part of this module")]

use mx_backends::{Backend, BackendCtx, RunPlan, RunRequest, backend_by_name};
use mx_ports::{Sink, Source, StageSpec};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const PROFILE: &str = "/p/fx";
pub const REPO: &str = "/state/checkout";
pub const HOME: &str = "/home/u";
pub const SCRIPTS: &str = "/tree/scripts";
pub const PROMPT: &str = "/state/prompt.txt";
pub const LOG: &str = "/state/logs/run.log";
pub const STATE: &str = "/state";

/// The context of a `review` run with no variable set.
pub fn ctx() -> BackendCtx {
    BackendCtx {
        profile_dir: PathBuf::from(PROFILE),
        repo_path: PathBuf::from(REPO),
        task: "review".to_string(),
        home: PathBuf::from(HOME),
        scripts_dir: Some(PathBuf::from(SCRIPTS)),
        vars: BTreeMap::new(),
    }
}

pub fn with(mut ctx: BackendCtx, name: &str, value: &str) -> BackendCtx {
    ctx.vars.insert(name.to_string(), value.to_string());
    ctx
}

pub fn request() -> RunRequest {
    RunRequest {
        prompt_file: PathBuf::from(PROMPT),
        model: "opus".to_string(),
        log: PathBuf::from(LOG),
        extra_dir: PathBuf::from(STATE),
    }
}

pub fn backend(name: &str) -> Box<dyn Backend> {
    backend_by_name(name).unwrap_or_else(|| panic!("backend_by_name({name:?}) knows no backend"))
}

pub fn argv(stage: &StageSpec) -> Vec<&str> {
    stage.argv.iter().map(String::as_str).collect()
}

pub fn env(stage: &StageSpec) -> Vec<(&str, &str)> {
    stage.env.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect()
}

pub fn exports(plan: &RunPlan) -> Vec<(&str, &str)> {
    plan.exports.iter().map(|export| (export.name.as_str(), export.value.as_str())).collect()
}

/// The one stage of a single-stage plan, after checking there is only one.
pub fn only_stage(plan: &RunPlan) -> &StageSpec {
    assert_eq!(plan.pipeline.stages.len(), 1, "expected a one-stage pipeline");
    plan.pipeline.stages.first()
}

pub fn from_file(path: &str) -> Source {
    Source::File(PathBuf::from(path))
}

pub fn append(path: &str) -> Sink {
    Sink::AppendFile(PathBuf::from(path))
}
