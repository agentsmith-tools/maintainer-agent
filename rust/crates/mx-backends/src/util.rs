//! What the four backends share: the shell's string rules, written once.

use crate::{BackendCtx, PlanRefusal};
use mx_core::NonEmpty;
use mx_ports::{PipelineSpec, Sink, Source, StageSpec};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The deadline of a pipeline. The bash sets none: "wall-clock is bounded by
/// systemd" (`TimeoutStartSec=10800` in the shipped unit). The `Pipe` port wants
/// a deadline, so the plan carries the unit's own value, and the orchestrator
/// may replace it (decision P2B-4).
pub(crate) const RUN_TIMEOUT: Duration = Duration::from_hours(3);

/// `"$dir/rest"`: the shell joins with a slash and normalises nothing, so a
/// directory that ends in a slash gives a double slash. `Path::join` would
/// swallow it, and the message and the argument would no longer match the bash.
pub(crate) fn dir_file(dir: &Path, rest: &str) -> PathBuf {
    let mut joined = dir.as_os_str().to_owned();
    joined.push("/");
    joined.push(rest);
    PathBuf::from(joined)
}

/// A path for a message printed to a person: invalid UTF-8 shows as U+FFFD.
pub(crate) fn shown(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A path for an argument or an environment value. Those are `String`s in the
/// pipeline types, and a lossy conversion would run the agent on a different
/// path from the one the profile named, so a path that is not UTF-8 refuses.
pub(crate) fn text(path: &Path) -> Result<String, PlanRefusal> {
    path.to_str().map(str::to_string).ok_or_else(|| PlanRefusal {
        log_note: format!(
            "path is not valid UTF-8, so it cannot be passed to the agent: {}",
            shown(path)
        ),
    })
}

/// `${NAME:-}`: the value, or nothing when the variable is unset or empty.
pub(crate) fn nonempty<'a>(ctx: &'a BackendCtx, name: &str) -> Option<&'a str> {
    ctx.var(name).filter(|value| !value.is_empty())
}

/// `${MAINTAINER_SETTINGS:-$PROFILE_DIR/<default_name>}`.
pub(crate) fn settings_or(ctx: &BackendCtx, default_name: &str) -> PathBuf {
    nonempty(ctx, "MAINTAINER_SETTINGS")
        .map_or_else(|| dir_file(&ctx.profile_dir, default_name), PathBuf::from)
}

/// A stage that runs in the review checkout (`run.sh` does `cd "$REPO_PATH"`
/// before it calls `backend_run`), reading nothing and writing nowhere until
/// the caller says so.
pub(crate) fn stage(argv: Vec<String>, ctx: &BackendCtx) -> StageSpec {
    let mut iter = argv.into_iter();
    let first = iter.next().unwrap_or_default();
    let mut stage = StageSpec::new(NonEmpty::new(first, iter));
    stage.cwd = Some(ctx.repo_path.clone());
    stage
}

/// `< prompt >> log 2>&1`: the shape of codex, cursor and opencode.
pub(crate) fn to_log(mut stage: StageSpec, prompt: Option<&Path>, log: &Path) -> StageSpec {
    if let Some(prompt) = prompt {
        stage.stdin = Source::File(prompt.to_path_buf());
    }
    stage.stdout = Sink::AppendFile(log.to_path_buf());
    stage.stderr = Sink::SameAsStdout;
    stage
}

/// One pipeline under the default deadline.
pub(crate) fn pipeline(first: StageSpec, rest: Vec<StageSpec>) -> PipelineSpec {
    PipelineSpec { stages: NonEmpty::new(first, rest), timeout: RUN_TIMEOUT }
}

/// Strings for `argv`: `["claude", "-p"]` from literals.
pub(crate) fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}
