//! The agent backends of `lib/backends/*.sh`: `claude`, `codex`, `cursor`, `opencode`.
//!
//! A backend in the bash is a file the orchestrator sources. It answers six
//! questions and then runs one command. Here each answer is a method of
//! [`Backend`], and the command is a [`RunPlan`]: data that says which programs
//! to run, with which environment, reading which file and writing which log.
//! The orchestrator (`mx-run`) executes the plan through the [`mx_ports::Pipe`]
//! port. A backend therefore starts no process and reads nothing but what it is
//! handed, so every backend is tested without a process, a file or a network.
//!
//! This crate holds the interface and the four implementations
//! ([`backend_by_name`], [`RealHost`], [`FakeHost`]). The executor that consumes
//! a plan is the work of the orchestrator agent (brief `p2a`), so a change to a
//! signature here is a change to both briefs.
//!
//! # What each bash function becomes
//!
//! | bash | Rust |
//! |---|---|
//! | `backend_name` | [`Backend::name`] |
//! | `backend_rehearsal` (declared or not) | [`Backend::declares_rehearsal`] |
//! | `backend_rehearsal_wall` | [`Backend::rehearsal_wall`] |
//! | `backend_allowed_tasks` (declared or not) | [`Backend::allowed_tasks`] |
//! | `backend_check` | [`Backend::check`] |
//! | `backend_run` | [`Backend::plan`] |
//! | `backend_transient_reason`, `backend_needs_human_reason` | `mx_profile::classify`, shared by every backend |

mod claude;
mod codex;
mod cursor;
mod host;
mod opencode;
mod util;

pub use host::{FakeHost, RealHost};

use mx_ports::PipelineSpec;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// What a backend function reads from the shell it is sourced into.
///
/// The bash backends read `PROFILE_DIR`, `REPO_PATH`, `MAINTAINER_TASK`,
/// `MAINTAINER_SCRIPTS`, `HOME` and a set of variables by name (`THINKING_<task>`,
/// `SUBAGENT_MODEL_<task>`, `OPENCODE_MODEL`, `MAINTAINER_SETTINGS`,
/// `MAINTAINER_CURSOR_DIR`, `GROQ_API_KEY`, ...). This struct carries the same
/// inputs, so no backend reads the environment itself.
#[derive(Clone, PartialEq, Eq)]
pub struct BackendCtx {
    /// `PROFILE_DIR`: where the profile's walls and settings live.
    pub profile_dir: PathBuf,
    /// `REPO_PATH`: the review checkout, the working directory of the agent.
    pub repo_path: PathBuf,
    /// `MAINTAINER_TASK`: the task this run performs.
    pub task: String,
    /// `HOME`.
    pub home: PathBuf,
    /// `MAINTAINER_SCRIPTS`, when `scripts/` exists in the tree.
    pub scripts_dir: Option<PathBuf>,
    /// The variables the bash functions read by name: the profile's values with
    /// the `MAINTAINER_<KEY>` overrides applied, plus whatever the run exported
    /// (the rehearsal wall variable, for one). Values here may be secrets, so
    /// this type has no `Display` and tests print names only.
    pub vars: BTreeMap<String, String>,
}

impl fmt::Debug for BackendCtx {
    /// Prints the names of the variables and never their values, because one of
    /// them can be `GROQ_API_KEY`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.vars.keys().map(String::as_str).collect();
        f.debug_struct("BackendCtx")
            .field("profile_dir", &self.profile_dir)
            .field("repo_path", &self.repo_path)
            .field("task", &self.task)
            .field("home", &self.home)
            .field("scripts_dir", &self.scripts_dir)
            .field("var_names", &names)
            .finish()
    }
}

impl BackendCtx {
    /// The value of a variable, as `${NAME:-}` would read it, or `None` when it
    /// is unset. A variable that is set and empty is `Some("")`.
    #[must_use]
    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }
}

/// What `backend_check` and `backend_run` ask of the machine: `command -v`, `[ -f ]`
/// and reading a file. A real implementation answers from the file system and
/// the search path, and a fake answers from a table, so no test touches the host.
pub trait Host {
    /// `command -v program` succeeds: the program is on the search path.
    fn on_path(&self, program: &str) -> bool;
    /// `[ -f path ]`.
    fn is_file(&self, path: &Path) -> bool;
    /// The size of a file in bytes, or `None` when it cannot be read.
    fn file_len(&self, path: &Path) -> Option<u64>;
    /// The bytes of a file, or `None` when it cannot be read.
    fn read(&self, path: &Path) -> Option<Vec<u8>>;
}

/// An environment variable the orchestrator exports for the rest of the run
/// once a backend asks for it. `backend_rehearsal_wall` does this in the bash
/// (`export MAINTAINER_SETTINGS=...`): later calls, including `backend_check`
/// and `backend_run`, read the variable by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    pub name: String,
    pub value: String,
}

/// The arguments of `backend_run <prompt-file> <model> <log-file> <extra-readable-dir>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRequest {
    /// The assembled prompt, in a file.
    pub prompt_file: PathBuf,
    /// The model the profile configured for this task.
    pub model: String,
    /// The run log. Everything the agent prints is appended here.
    pub log: PathBuf,
    /// The state directory, which the agent may read besides the checkout.
    pub extra_dir: PathBuf,
}

/// What a backend decided to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPlan {
    /// Lines the bash appends to the log before it starts the agent (the Claude
    /// backend says when `transcript.py` is missing and the run records prose
    /// only). The executor appends each line, plus a newline, in order.
    pub log_notes: Vec<String>,
    /// Variables exported before the pipeline starts. They stay exported for the
    /// rest of the run, as a bash `export` inside `backend_run` does.
    pub exports: Vec<Export>,
    /// The programs to run. Their environment is exactly their `env`: the plan
    /// names every variable the agent should see that the executor does not
    /// already pass to every child.
    pub pipeline: PipelineSpec,
}

/// A backend that cannot make a plan, after saying why in the log. The bash
/// functions that do this append one line to the log and return 1; the
/// executor does the same and treats the run as a backend failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{log_note}")]
pub struct PlanRefusal {
    /// The line for the log, without a newline.
    pub log_note: String,
}

/// One agent backend.
pub trait Backend {
    /// `backend_name`: the name printed in the provenance line and the log.
    fn name(&self) -> &'static str;

    /// Whether the bash file defines `backend_rehearsal`. The orchestrator
    /// refuses `POST=off` for a backend that does not: a rehearsal that
    /// silently posts is worse than no rehearsal. Only `codex` answers false.
    fn declares_rehearsal(&self) -> bool;

    /// `backend_rehearsal_wall`: where this backend's rehearsal wall is, and
    /// the variable it reads to find it. `None` means the wall is not rendered
    /// (the bash returns 1), and the orchestrator refuses a real run.
    fn rehearsal_wall(&self, ctx: &BackendCtx, host: &dyn Host) -> Option<Export>;

    /// `backend_allowed_tasks`: the only tasks this backend may run, or `None`
    /// when it declares no restriction. `cursor` answers `["readonly-review"]`.
    fn allowed_tasks(&self) -> Option<&'static [&'static str]>;

    /// `backend_check`: `Ok` when the backend can run, else the message the bash
    /// prints on standard output before it returns 1, byte for byte.
    fn check(&self, ctx: &BackendCtx, host: &dyn Host) -> Result<(), String>;

    /// `backend_run`: what to run.
    fn plan(
        &self,
        ctx: &BackendCtx,
        host: &dyn Host,
        request: &RunRequest,
    ) -> Result<RunPlan, PlanRefusal>;
}

/// The backend named `name`, or `None` for a name no file in `lib/backends`
/// defines. The bash sources `$BACKEND_DIR/$BACKEND.sh`, so a name with a slash
/// or a dot-dot reaches any file (decision D18); this function matches the four
/// names exactly and refuses every other string.
///
#[must_use]
pub fn backend_by_name(name: &str) -> Option<Box<dyn Backend>> {
    match name {
        "claude" => Some(Box::new(claude::Claude)),
        "codex" => Some(Box::new(codex::Codex)),
        "cursor" => Some(Box::new(cursor::Cursor)),
        "opencode" => Some(Box::new(opencode::Opencode)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_variable_that_is_set_and_empty_differs_from_one_that_is_unset() {
        let mut ctx = BackendCtx {
            profile_dir: PathBuf::from("/p"),
            repo_path: PathBuf::from("/r"),
            task: "review".to_string(),
            home: PathBuf::from("/h"),
            scripts_dir: None,
            vars: BTreeMap::new(),
        };
        assert_eq!(ctx.var("GROQ_API_KEY"), None);
        ctx.vars.insert("GROQ_API_KEY".to_string(), String::new());
        assert_eq!(ctx.var("GROQ_API_KEY"), Some(""));
    }

    #[test]
    fn a_plan_refusal_prints_its_log_note() {
        let refusal = PlanRefusal { log_note: "too large".to_string() };
        assert_eq!(refusal.to_string(), "too large");
    }
}
