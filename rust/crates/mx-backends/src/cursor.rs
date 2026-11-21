//! `lib/backends/cursor.sh`.

use crate::util::{dir_file, nonempty, pipeline, shown, stage, text, to_log, words};
use crate::{Backend, BackendCtx, Export, Host, PlanRefusal, RunPlan, RunRequest};
use std::path::PathBuf;

/// The first prompt size that is refused, in bytes. Linux caps one argument at
/// 131072 bytes including its terminator, and this backend passes the prompt as
/// one argument.
const ARGUMENT_LIMIT: u64 = 131_072;

/// Written to the log, word for word as the bash writes it.
const TOO_LARGE: &str = "cursor prompt is too large for a single argument (argument list too long); reduce the context or use another backend";

/// Cursor CLI. `--force` is never passed, so Cursor proposes changes and does
/// not write them, whatever the permissions file says.
pub(crate) struct Cursor;

impl Cursor {
    /// `${MAINTAINER_CURSOR_DIR:-$PROFILE_DIR/cursor}`.
    fn wall_dir(ctx: &BackendCtx) -> PathBuf {
        nonempty(ctx, "MAINTAINER_CURSOR_DIR")
            .map_or_else(|| dir_file(&ctx.profile_dir, "cursor"), PathBuf::from)
    }
}

fn refuse(note: &str) -> PlanRefusal {
    PlanRefusal { log_note: note.to_string() }
}

impl Backend for Cursor {
    fn name(&self) -> &'static str {
        "cursor"
    }

    fn declares_rehearsal(&self) -> bool {
        true
    }

    fn rehearsal_wall(&self, ctx: &BackendCtx, host: &dyn Host) -> Option<Export> {
        let dir = dir_file(&ctx.profile_dir, "cursor-rehearsal");
        if !host.is_file(&dir_file(&dir, "cli-config.json")) {
            return None;
        }
        Some(Export { name: "MAINTAINER_CURSOR_DIR".to_string(), value: dir.to_str()?.to_string() })
    }

    /// Still restricted, and gated on proof (tests/containment-probe.sh, issue
    /// #5): nothing has watched cursor-agent refuse a denied command.
    fn allowed_tasks(&self) -> Option<&'static [&'static str]> {
        Some(&["readonly-review"])
    }

    fn check(&self, ctx: &BackendCtx, host: &dyn Host) -> Result<(), String> {
        if !host.on_path("cursor-agent") {
            return Err("cursor-agent not on PATH".to_string());
        }
        let dir = Self::wall_dir(ctx);
        if !host.is_file(&dir_file(&dir, "cli-config.json")) {
            return Err(format!("no cursor wall at {}; run ./install.sh", shown(&dir)));
        }
        Ok(())
    }

    fn plan(
        &self,
        ctx: &BackendCtx,
        host: &dyn Host,
        request: &RunRequest,
    ) -> Result<RunPlan, PlanRefusal> {
        // Refuse before Linux's per-argument limit, as the bash does, counting the
        // bytes of the file (`wc -c`) and not of the argument built from it.
        let Some(len) = host.file_len(&request.prompt_file) else {
            return Err(refuse(&format!(
                "cursor cannot read the prompt file {}",
                shown(&request.prompt_file)
            )));
        };
        if len >= ARGUMENT_LIMIT {
            return Err(refuse(TOO_LARGE));
        }
        let Some(bytes) = host.read(&request.prompt_file) else {
            return Err(refuse(&format!(
                "cursor cannot read the prompt file {}",
                shown(&request.prompt_file)
            )));
        };
        let prompt = command_substitution(bytes)?;

        let mut argv = words(&["cursor-agent", "-p", "--output-format", "text", "-m"]);
        argv.extend([request.model.clone(), prompt]);
        let mut agent = to_log(stage(argv, ctx), None, &request.log);
        // The wall, chosen by the orchestrator: the rehearsal directory adds every
        // GitHub write verb on top of the live one.
        agent.env.push(("CURSOR_CONFIG_DIR".to_string(), text(&Self::wall_dir(ctx))?));
        Ok(RunPlan {
            log_notes: Vec::new(),
            exports: Vec::new(),
            pipeline: pipeline(agent, Vec::new()),
        })
    }
}

/// What `"$(cat file)"` gives the shell: the bytes without NUL, without the
/// newlines at the end. A prompt that is not UTF-8 cannot be an argument here
/// (decision P2B-2), so it refuses and nothing is passed in a changed form.
fn command_substitution(mut bytes: Vec<u8>) -> Result<String, PlanRefusal> {
    bytes.retain(|byte| *byte != 0);
    while bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    String::from_utf8(bytes).map_err(|_| {
        refuse("cursor cannot pass a prompt that is not valid UTF-8 as an argument; use another backend")
    })
}
