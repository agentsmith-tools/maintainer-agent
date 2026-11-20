//! Lines 624 to 828 of `run.sh`: the prompt, the agent, and closing the run.

#[allow(
    clippy::wildcard_imports,
    reason = "the steps of one pass are split over files and share one vocabulary"
)]
use super::*;

/// The message of `cat file` when it cannot read the file: the program name, the
/// path and the system's words.
fn cat_error(path: &Path, error: &std::io::Error) -> String {
    format!("cat: {}: {}\n", path.display(), lock::os_text(error))
}

/// `awk -F'\t' '{printf "%s%s: %s", $1, ($2==""?"":"/"$2), $3}'` over the
/// classifier's one-line answer: `class/id: text`, the text cut at its first tab.
#[must_use]
pub fn abort_cause(rendered: &[u8]) -> String {
    let text = text_of(rendered);
    let mut fields = text.split('\t');
    let class = fields.next().unwrap_or_default();
    let id = fields.next().unwrap_or_default();
    let detail = fields.next().unwrap_or_default();
    let slash = if id.is_empty() { String::new() } else { format!("/{id}") };
    format!("{class}{slash}: {detail}")
}

impl Pass<'_, '_> {
    /// A preview has no context to refresh; a real run has the helper's output.
    pub fn preview_context(&mut self) {
        self.context =
            "(preview only: no refresh ran, so nothing here reflects the tracker)".to_string();
        self.run_id = "preview".to_string();
    }

    /// `libfile`.
    fn libfile(&self, name: &str) -> Option<PathBuf> {
        tree::libfile(&self.tree.root, name)
    }

    /// Preamble, doctrine and task, in that order. The prompt is written to a file
    /// rather than piped from a variable so the backend can choose stdin or an
    /// argument without the caller caring (lines 624 to 731).
    pub fn assemble_prompt(&mut self) -> Flow {
        let made = self.exec.capture(&["mktemp"], &[]).stdout_text();
        let prompt_file = PathBuf::from(&made);
        shared(&self.shared).prompt_file = Some(prompt_file.clone());
        // A run already has the scrub trap and must keep it, so only a preview,
        // which never installs that trap, gets its own.
        if self.show {
            self.arm_preview_trap();
        }
        // The core preamble carries the injection rule, the screen rule and the
        // evidence rule. A run without it is an unattended agent with no doctrine,
        // so this fails closed rather than continuing with a shorter prompt.
        let Some(core) = self.libfile("preamble-core.md") else {
            self.alert("preamble-core.md is missing; refusing to run without the doctrine");
            return Err(Stop(1));
        };
        let prose = self.libfile("prose-style.md");
        // Fail before constructing even a partial prompt: the doctrine and the task
        // are required inputs, and a write-capable backend must never see a prompt
        // that silently lost either one.
        let common = self.profile_dir.join("prompts/common-preamble.md");
        let task_prompt = self.profile_dir.join(format!("prompts/{}.md", self.task));
        for required in [&common, &core, &task_prompt] {
            if std::fs::File::open(required).is_err() {
                self.alert(&format!(
                    "required prompt input is missing or unreadable: {}; refusing to run",
                    required.display()
                ));
                return Err(Stop(1));
            }
        }
        if self.build_prompt(&common, &core, prose.as_deref(), &task_prompt, &prompt_file).is_err()
        {
            self.alert("prompt assembly failed; refusing to start the backend");
            return Err(Stop(1));
        }
        Ok(())
    }

    /// The subshell of lines 667 to 731. `Err` is its `exit 1`, after the message
    /// of whatever failed has gone to standard error.
    fn build_prompt(
        &mut self,
        common: &Path,
        core: &Path,
        prose: Option<&Path>,
        task_prompt: &Path,
        prompt_file: &Path,
    ) -> Result<(), ()> {
        let started = self.date_iso();
        let common_bytes = self.read_for_cat(common)?;
        let name = self.var_or("MAINTAINER_NAME", "the maintainer").to_string();
        let sed = prompt::sed_arguments(&name, &self.repo_slug, &core.to_string_lossy());
        let mut words = vec!["sed"];
        words.extend(sed.iter().map(String::as_str));
        let done = self.exec.capture(&words, &[]);
        put(self.svc.err, &done.stderr);
        if !done.ok() {
            return Err(());
        }
        // Opt-OUT, not opt-in: anything other than an explicit "raw" gets the prose
        // discipline, so a missing or misspelled value still writes well.
        let prose_bytes = match prose {
            Some(path) if self.var_or("PROSE_STYLE", "stop-slop") != "raw" => {
                Some(self.read_for_cat(path)?)
            }
            _ => None,
        };
        let task_bytes = self.read_for_cat(task_prompt)?;
        let backend_name = self.backend().name();
        let parts = PromptParts {
            version: self.version.as_bytes(),
            commit: self.commit.as_bytes(),
            profile: &self.profile,
            task: &self.task,
            backend: backend_name,
            started: started.as_bytes(),
            common: &common_bytes,
            core: &done.stdout,
            prose: prose_bytes.as_deref(),
            post: &self.post,
            repo_slug: &self.repo_slug,
            gh_account: &self.gh_account,
            task_prompt: &task_bytes,
            context: self.context.as_bytes(),
        };
        let bytes = prompt::assemble(&parts);
        std::fs::write(prompt_file, bytes).map_err(|error| {
            let line =
                format!("{}: {}: {}\n", self.argv0, prompt_file.display(), lock::os_text(&error));
            put(self.svc.err, line.as_bytes());
        })
    }

    fn read_for_cat(&mut self, path: &Path) -> Result<Vec<u8>, ()> {
        std::fs::read(path).map_err(|error| {
            put(self.svc.err, cat_error(path, &error).as_bytes());
        })
    }

    /// `--show-prompt`: print the assembled prompt and leave nothing behind.
    pub fn print_prompt(&mut self) {
        let file = shared(&self.shared).prompt_file.clone();
        if let Some(file) = file {
            let bytes = std::fs::read(&file).unwrap_or_default();
            put(self.svc.out, &bytes);
        }
        remove_prompt_file(&self.shared);
    }

    /// Enter the checkout the agent works in (line 740).
    pub fn enter_checkout(&mut self) -> Flow {
        if !std::fs::metadata(&self.repo_path).is_ok_and(|meta| meta.is_dir()) {
            let path = self.repo_path.display().to_string();
            self.alert(&format!("cannot cd to {path}"));
            return Err(Stop(1));
        }
        self.exec.cwd = Some(self.repo_path.clone());
        shared(&self.shared).cwd = Some(self.repo_path.clone());
        Ok(())
    }

    /// `backend_run`: what the backend plans, run through the pipeline port.
    /// `true` is a zero status.
    fn run_backend(&mut self) -> bool {
        let prompt_file = shared(&self.shared).prompt_file.clone().unwrap_or_default();
        let request = RunRequest {
            prompt_file,
            model: self.model.clone(),
            log: self.log(),
            extra_dir: self.state_dir.clone(),
        };
        let planned = self.backend().plan(&self.backend_ctx(), &self.host(), &request);
        let plan = match planned {
            Ok(plan) => plan,
            Err(refusal) => {
                self.append_log(&format!("{}\n", refusal.log_note));
                return false;
            }
        };
        for note in &plan.log_notes {
            self.append_log(&format!("{note}\n"));
        }
        for export in &plan.exports {
            self.export(&export.name, &export.value);
        }
        match self.exec.pipeline(&plan.pipeline) {
            Ok(outcome) => outcome.pipefail().success(),
            Err(error) => {
                self.append_log(&format!("{error}\n"));
                false
            }
        }
    }

    /// The classifier's answer for the log as it stands now.
    fn classify(&self) -> mx_profile::classify::Classification {
        let env = ClassifyEnv {
            failure_shapes: self.vars.get("MAINTAINER_FAILURE_SHAPES").map(OsString::from),
            lib_dir: self.tree.profile_lib.parent().map(Path::to_path_buf).unwrap_or_default(),
            home: OsString::from(self.exec.env.get("HOME").unwrap_or_default()),
            locale: effective_locale(
                self.exec.env.get("LC_ALL"),
                self.exec.env.get("LC_CTYPE"),
                self.exec.env.get("LANG"),
            ),
        };
        let answer = classify_failure(self.exec.adapters.proc.as_ref(), &env, &self.log());
        trap::checkpoint();
        answer
    }

    /// Hand the prompt to the backend, and report a failure that it ends with.
    /// Wall-clock is bounded by systemd, not here (lines 739 to 763).
    pub fn run_agent(&mut self) -> AgentEnd {
        let mut end = AgentEnd::default();
        if self.run_backend() {
            return end;
        }
        // A model usage window that resets on a clock, or an overloaded upstream, is
        // the same class as an unreachable GitHub: recorded, reported by doctor,
        // retried by the next slot, and worth nobody's attention at `critical`.
        end.why = self
            .classify()
            .transient_reason()
            .map(|bytes| trimmed_text(&bytes))
            .unwrap_or_default();
        end.human = self
            .classify()
            .needs_human_reason()
            .map(|bytes| trimmed_text(&bytes))
            .unwrap_or_default();
        let (name, run_id) = (self.backend_name.clone(), self.run_id.clone());
        let log_text = self.log().to_string_lossy().into_owned();
        if !end.why.is_empty() {
            let why = end.why.clone();
            self.alert_transient(&format!(
                "the {name} backend stopped on something that clears itself ({why}); the next slot retries this task"
            ));
        } else if !end.human.is_empty() {
            // Between the two: no retry clears it, and one command does.
            let human = end.human.clone();
            self.alert(&format!(
                "the {name} backend stopped on something only you can clear ({human}), and every slot after this one fails the same way until it is. See {log_text}"
            ));
        } else {
            self.alert(&format!("{name} exited non-zero for {run_id}. See {log_text}"));
        }
        end.alerted = true;
        end
    }

    /// Close the run. `finish` refuses an empty report, which is the point
    /// (lines 765 to 828).
    pub fn close_run(&mut self, end: &AgentEnd) -> Flow {
        let (helper, run_id) = (self.helper.clone(), self.run_id.clone());
        let done = self.exec.capture(&[&helper, "finish", &run_id], &[]);
        let finish_out = done.merged();
        self.append_log(&format!("{finish_out}\n"));
        if !done.ok() {
            return self.report_missing(end, &finish_out);
        }
        // Say what happened, to a person: the digest is built from what the run
        // MEASURED, not from what it claims.
        let digest = {
            let answer = self.exec.capture(&[&helper, "digest", &run_id, "--compact"], &[]);
            if answer.ok() { answer.stdout_text() } else { String::new() }
        };
        if !digest.is_empty() {
            self.append_log(&format!("\n=== digest ===\n{digest}\n"));
            let title = format!("maintainer · {} {}", self.profile, self.task);
            self.notify("normal", &title, &digest);
        }
        let when = self.date_iso();
        self.append_log(&format!("=== done {when} ===\n"));
        Ok(())
    }

    /// `finish` failed: write the absence into the index before alerting, so the
    /// record exists whatever happens to the alert (lines 768 to 815).
    fn report_missing(&mut self, end: &AgentEnd, finish_out: &str) -> Flow {
        let abort_why = abort_cause(&self.classify().render());
        // A report that exists but that finish turned down is a different event from
        // no report at all: the reader should open runs/, not the log.
        let refused = finish_out.contains("fixture rather than a report");
        let (helper, run_id) = (self.helper.clone(), self.run_id.clone());
        let reason = if refused {
            format!("report refused by finish: fixture rather than a report (see runs/{run_id}.md)")
        } else {
            let cause = if abort_why.is_empty() { "cause not determined" } else { &abort_why };
            format!("no report; {cause}")
        };
        let log = self.log();
        let _ = self.exec.to_log(&[&helper, "abort", &run_id, &reason], &[], &log);
        let (name, log_text) = (self.backend_name.clone(), log.to_string_lossy().into_owned());
        if !end.why.is_empty() {
            // The backend never got far enough to write anything, and the reason
            // heals on its own. Exit 75 so SuccessExitStatus absorbs it and
            // OnFailure does not raise the third popup for one condition.
            let why = end.why.clone();
            self.alert_transient(&format!(
                "{run_id} wrote no report: the {name} backend stopped on {why}"
            ));
            return Err(Stop(75));
        }
        if end.alerted {
            // The backend already said why, at critical, naming the cause. "Produced
            // no report" is that same event described a second time.
            self.tee(&format!(
                "{run_id} produced no report, and the {name} failure above is why. See {log_text}"
            ));
            return Err(Stop(1));
        }
        if refused {
            self.alert(&format!(
                "{run_id} wrote a report that finish refused (fixture rather than a report). Read runs/{run_id}.md and {log_text}"
            ));
        } else {
            self.alert(&format!(
                "{run_id} produced no report; the run is not auditable. See {log_text}"
            ));
        }
        Err(Stop(1))
    }
}

/// How the agent's run ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentEnd {
    /// The transient reason, empty when there was none.
    pub why: String,
    /// The reason only a person can clear, empty when there was none.
    pub human: String,
    /// The failure was already reported.
    pub alerted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_abort_cause_joins_class_id_and_text_with_a_slash_and_a_colon() {
        assert_eq!(
            abort_cause(b"transient\tsession-limit\tUsage limit reached"),
            "transient/session-limit: Usage limit reached"
        );
    }

    #[test]
    fn an_empty_id_leaves_out_the_slash() {
        assert_eq!(
            abort_cause(b"unclassified\t\terror: report is empty"),
            "unclassified: error: report is empty"
        );
    }

    #[test]
    fn the_text_is_cut_at_its_first_tab() {
        assert_eq!(abort_cause(b"c\ti\tone\ttwo"), "c/i: one");
    }

    #[test]
    fn an_answer_with_no_tabs_has_empty_fields() {
        assert_eq!(abort_cause(b"odd"), "odd: ");
    }
}
