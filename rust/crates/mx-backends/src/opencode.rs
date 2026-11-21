//! `lib/backends/opencode.sh`.

use crate::util::{dir_file, nonempty, pipeline, settings_or, shown, stage, text, to_log, words};
use crate::{Backend, BackendCtx, Export, Host, PlanRefusal, RunPlan, RunRequest};

/// opencode, provider-agnostic and wired to Groq. Its permission rules are
/// last-match-wins, so the shipped config denies by default.
pub(crate) struct Opencode;

impl Opencode {
    /// `$HOME/.groq-staging-key`.
    fn key_file(ctx: &BackendCtx) -> std::path::PathBuf {
        dir_file(&ctx.home, ".groq-staging-key")
    }
}

impl Backend for Opencode {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn declares_rehearsal(&self) -> bool {
        true
    }

    fn rehearsal_wall(&self, ctx: &BackendCtx, host: &dyn Host) -> Option<Export> {
        let wall = dir_file(&ctx.profile_dir, "opencode-rehearsal.json");
        if !host.is_file(&wall) {
            return None;
        }
        Some(Export { name: "MAINTAINER_SETTINGS".to_string(), value: wall.to_str()?.to_string() })
    }

    fn allowed_tasks(&self) -> Option<&'static [&'static str]> {
        None
    }

    fn check(&self, ctx: &BackendCtx, host: &dyn Host) -> Result<(), String> {
        if !host.on_path("opencode") {
            return Err(
                "opencode not on PATH (curl -fsSL https://opencode.ai/install | bash)".to_string()
            );
        }
        let config = settings_or(ctx, "opencode.json");
        if !host.is_file(&config) {
            return Err(format!("missing {}", shown(&config)));
        }
        if nonempty(ctx, "GROQ_API_KEY").is_none()
            && !host.is_file(&Self::key_file(ctx))
            && !host.is_file(&dir_file(&ctx.home, ".local/share/opencode/auth.json"))
        {
            return Err(
                "no Groq credential: set GROQ_API_KEY, or ~/.groq-staging-key, or run 'opencode auth login'"
                    .to_string(),
            );
        }
        Ok(())
    }

    fn plan(
        &self,
        ctx: &BackendCtx,
        host: &dyn Host,
        request: &RunRequest,
    ) -> Result<RunPlan, PlanRefusal> {
        // The profile names a Claude model. Map it onto a Groq one unless it
        // already is a provider/model pair (any slash counts, as in `*/*`).
        let model = if request.model.contains('/') {
            request.model.clone()
        } else if let Some(default) = nonempty(ctx, "OPENCODE_MODEL") {
            default.to_string()
        } else {
            // Decision P2B-1: `${OPENCODE_MODEL:?...}` ends the whole bash run.
            return Err(PlanRefusal {
                log_note: "OPENCODE_MODEL: lib/defaults.env was not loaded".to_string(),
            });
        };

        // A key in the environment is the executor's to pass on, as the bash agent
        // inherits it, so the plan never names it (decision P2B-10). The plan
        // names the key only when this backend read it from the key file, the case
        // where the bash runs `export GROQ_API_KEY`: the environment holds no key
        // or an empty one, and the file exists. The file is read with every newline
        // deleted (`tr -d '\n'`). A file that exists and cannot be read adds nothing.
        let mut key_from_file: Option<String> = None;
        let key_file = Self::key_file(ctx);
        if ctx.var("GROQ_API_KEY").is_none_or(str::is_empty)
            && host.is_file(&key_file)
            && let Some(mut bytes) = host.read(&key_file)
        {
            // The shell drops NUL bytes from a command substitution.
            bytes.retain(|byte| *byte != b'\n' && *byte != 0);
            key_from_file = Some(String::from_utf8(bytes).map_err(|_| PlanRefusal {
                log_note: "the Groq key file is not valid UTF-8".to_string(),
            })?);
        }

        // --auto approves what the config does not explicitly deny. That is only
        // safe because the config denies by default.
        let argv = {
            let mut argv = words(&["opencode", "run", "--model"]);
            argv.push(model);
            argv.extend(words(&["--auto", "--format", "default"]));
            argv
        };
        let mut agent = to_log(stage(argv, ctx), Some(&request.prompt_file), &request.log);
        agent.env.push(("OPENCODE_CONFIG".to_string(), text(&settings_or(ctx, "opencode.json"))?));
        // The key goes to this stage only, through the environment. It is in no
        // argument and no export, and `StageSpec`'s `Debug` prints names only.
        if let Some(key) = key_from_file {
            agent.env.push(("GROQ_API_KEY".to_string(), key));
        }
        Ok(RunPlan {
            log_notes: Vec::new(),
            exports: Vec::new(),
            pipeline: pipeline(agent, Vec::new()),
        })
    }
}
