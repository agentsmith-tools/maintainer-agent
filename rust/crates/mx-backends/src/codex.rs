//! `lib/backends/codex.sh`.

use crate::util::{pipeline, stage, text, to_log, words};
use crate::{Backend, BackendCtx, Export, Host, PlanRefusal, RunPlan, RunRequest};

/// Codex CLI. Its containment is coarser than Claude's (a sandbox mode, no
/// per-command deny list), so it declares no rehearsal: a `POST=off` run on this
/// backend is refused rather than run with nothing to stop a post.
pub(crate) struct Codex;

impl Backend for Codex {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn declares_rehearsal(&self) -> bool {
        false
    }

    fn rehearsal_wall(&self, _ctx: &BackendCtx, _host: &dyn Host) -> Option<Export> {
        None
    }

    fn allowed_tasks(&self) -> Option<&'static [&'static str]> {
        None
    }

    fn check(&self, _ctx: &BackendCtx, host: &dyn Host) -> Result<(), String> {
        if host.on_path("codex") { Ok(()) } else { Err("codex not on PATH".to_string()) }
    }

    fn plan(
        &self,
        ctx: &BackendCtx,
        _host: &dyn Host,
        request: &RunRequest,
    ) -> Result<RunPlan, PlanRefusal> {
        // `codex exec` takes the prompt on stdin when no positional argument is
        // given. `-C` sets the working root and `--sandbox` bounds writes to it.
        let mut argv = words(&["codex", "exec", "--sandbox", "workspace-write", "-C"]);
        argv.push(text(&ctx.repo_path)?);
        argv.extend(["-m".to_string(), request.model.clone()]);
        argv.extend(words(&["--skip-git-repo-check", "-"]));
        let agent = to_log(stage(argv, ctx), Some(&request.prompt_file), &request.log);
        Ok(RunPlan {
            log_notes: Vec::new(),
            exports: Vec::new(),
            pipeline: pipeline(agent, Vec::new()),
        })
    }
}
