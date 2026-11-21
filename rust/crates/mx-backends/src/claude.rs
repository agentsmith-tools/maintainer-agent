//! `lib/backends/claude.sh`.

use crate::util::{dir_file, nonempty, pipeline, settings_or, shown, stage, text, words};
use crate::{Backend, BackendCtx, Export, Host, PlanRefusal, RunPlan, RunRequest};
use mx_ports::{Sink, Source};
use std::path::PathBuf;

/// Claude Code. `--settings` carries a deny list that outranks
/// `--permission-mode bypassPermissions`, which is why it is the default backend.
pub(crate) struct Claude;

impl Claude {
    /// `$MAINTAINER_SCRIPTS/transcript.py`, where an unset or empty
    /// `MAINTAINER_SCRIPTS` gives `/transcript.py` (the bash expands
    /// `${MAINTAINER_SCRIPTS:-}/transcript.py`).
    fn filter(ctx: &BackendCtx) -> PathBuf {
        let mut path = ctx.scripts_dir.clone().unwrap_or_default().into_os_string();
        path.push("/transcript.py");
        PathBuf::from(path)
    }
}

impl Backend for Claude {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn declares_rehearsal(&self) -> bool {
        true
    }

    fn rehearsal_wall(&self, ctx: &BackendCtx, host: &dyn Host) -> Option<Export> {
        let wall = dir_file(&ctx.profile_dir, "settings-rehearsal.json");
        if !host.is_file(&wall) {
            return None;
        }
        // A wall whose path is not UTF-8 cannot be exported as a String, so it
        // counts as not rendered and the orchestrator refuses the run.
        Some(Export { name: "MAINTAINER_SETTINGS".to_string(), value: wall.to_str()?.to_string() })
    }

    fn allowed_tasks(&self) -> Option<&'static [&'static str]> {
        None
    }

    fn check(&self, ctx: &BackendCtx, host: &dyn Host) -> Result<(), String> {
        if !host.on_path("claude") {
            return Err("claude not on PATH".to_string());
        }
        let settings = settings_or(ctx, "settings.json");
        if !host.is_file(&settings) {
            return Err(format!("missing {}", shown(&settings)));
        }
        Ok(())
    }

    fn plan(
        &self,
        ctx: &BackendCtx,
        host: &dyn Host,
        request: &RunRequest,
    ) -> Result<RunPlan, PlanRefusal> {
        // Reasoning effort is not a CLI flag: Claude Code reads the budget from
        // MAX_THINKING_TOKENS. THINKING_<task> wins over MAINTAINER_THINKING, and
        // an empty value counts as unset (`${!var:-${MAINTAINER_THINKING:-}}`).
        let think = nonempty(ctx, &format!("THINKING_{}", ctx.task))
            .or_else(|| nonempty(ctx, "MAINTAINER_THINKING"));
        let subagent = nonempty(ctx, &format!("SUBAGENT_MODEL_{}", ctx.task));
        let mut exports = Vec::new();
        if let Some(value) = think {
            exports
                .push(Export { name: "MAX_THINKING_TOKENS".to_string(), value: value.to_string() });
        }
        if let Some(value) = subagent {
            exports.push(Export {
                name: "CLAUDE_CODE_SUBAGENT_MODEL".to_string(),
                value: value.to_string(),
            });
        }

        let settings = text(&settings_or(ctx, "settings.json"))?;
        let extra_dir = text(&request.extra_dir)?;
        let filter = Self::filter(ctx);
        let filter_present = host.is_file(&filter);
        let mut argv = words(&["claude", "-p", "--settings"]);
        argv.extend([settings, "--permission-mode".to_string(), "bypassPermissions".to_string()]);
        argv.extend([
            "--model".to_string(),
            request.model.clone(),
            "--add-dir".to_string(),
            extra_dir,
        ]);

        let mut agent;
        let mut log_notes = Vec::new();
        let mut filter_stage = None;
        if filter_present {
            // stream-json, so the log records what the agent RAN and not only
            // what it said. `--verbose` is required with it in print mode.
            argv.extend(words(&["--output-format", "stream-json", "--verbose"]));
            agent = stage(argv, ctx);
            agent.stdin = Source::File(request.prompt_file.clone());
            agent.stdout = Sink::NextStage;
            agent.stderr = Sink::AppendFile(request.log.clone());
            let log = text(&request.log)?;
            let stem = log.strip_suffix(".log").unwrap_or(&log);
            let mut second = stage(
                vec![
                    "python3".to_string(),
                    text(&filter)?,
                    log.clone(),
                    format!("{stem}.commands"),
                ],
                ctx,
            );
            second.stdin = Source::PreviousStage;
            second.stdout = Sink::Null;
            second.stderr = Sink::AppendFile(request.log.clone());
            filter_stage = Some(second);
        } else {
            // Say so. A silent fallback is how the transcript went missing for a
            // whole deployment without anyone noticing.
            log_notes.push(format!(
                "transcript.py not found at {}; this run records prose only",
                shown(&filter)
            ));
            agent = stage(argv, ctx);
            agent.stdin = Source::File(request.prompt_file.clone());
            agent.stdout = Sink::AppendFile(request.log.clone());
            agent.stderr = Sink::SameAsStdout;
        }
        agent.env = exports.iter().map(|e| (e.name.clone(), e.value.clone())).collect();

        Ok(RunPlan {
            log_notes,
            exports,
            pipeline: pipeline(agent, filter_stage.into_iter().collect()),
        })
    }
}
