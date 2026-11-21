//! Lines 159 to 368 of `run.sh`: the stamp, the log, `PATH`, the notification and
//! the alert, the choice of backend, the rehearsal wall, the version stamp and the
//! checks a backend must pass before anything is locked.

#[allow(
    clippy::wildcard_imports,
    reason = "the steps of one pass are split over files and share one vocabulary"
)]
use super::*;

/// `sed -n 's/^KEY=//p'` over a file's text, read by a command substitution: the
/// matching lines joined by newlines, NUL bytes dropped, trailing newlines gone.
#[must_use]
pub fn stamp_value(text: &[u8], key: &str) -> String {
    let prefix = format!("{key}=");
    let lines: Vec<String> = text
        .split(|byte| *byte == b'\n')
        .filter_map(|line| line.strip_prefix(prefix.as_bytes()))
        .map(text_of)
        .collect();
    lines.join("\n").replace('\0', "").trim_end_matches('\n').to_string()
}

impl Pass<'_, '_> {
    /// The time stamp, the log path and the `PATH` prefix (lines 159 to 173).
    ///
    /// `~/.cargo/bin` too: rustup puts the toolchain there and a systemd user unit
    /// does not read the shell profile that adds it.
    pub fn prepare(&mut self) -> Flow {
        let stamp = self.exec.capture(&["date", "+%Y-%m-%dT%H-%M"], &[]).stdout_text();
        let log = if self.show {
            // A preview leaves no trace.
            PathBuf::from("/dev/null")
        } else {
            let logs = self.state_dir.join("logs");
            drop(std::fs::create_dir_all(&logs));
            let log = logs.join(format!("{stamp}-{}.log", self.task));
            self.logs_dir = Some(logs);
            log
        };
        shared(&self.shared).log = log;
        let home = self.exec.env.get("HOME").unwrap_or_default().to_string();
        let old = self.exec.env.get("PATH").unwrap_or(DEFAULT_PATH).to_string();
        let path = format!("{home}/.local/bin:{home}/.cargo/bin:{old}");
        self.export("PATH", &path);
        let locale = self.exec.locale.clone();
        self.exec.adapters = (self.svc.adapters)(OsStr::new(&path), locale);
        self.lock_wait = self.var_or("LOCK_WAIT", "3600").to_string();
        self.helper = self.var("HELPER").unwrap_or_default().to_string();
        Ok(())
    }

    /// `session_display`: the display, asked for rather than assumed.
    fn session_display(&self) -> String {
        let who = match self.exec.env.non_empty("USER") {
            Some(user) => user.to_string(),
            None => self.exec.capture(&["id", "-un"], &[]).stdout_text(),
        };
        let user_display =
            self.exec.capture(&["loginctl", "show-user", &who, "-p", "Display", "--value"], &[]);
        let session = user_display.stdout_text();
        let display = self
            .exec
            .capture(&["loginctl", "show-session", &session, "-p", "Display", "--value"], &[])
            .stdout_text();
        if display.is_empty() {
            self.exec.env.non_empty("DISPLAY").unwrap_or(":0").to_string()
        } else {
            display
        }
    }

    /// `notify`: one shape for the failure and the success alike. A headless host
    /// has nobody to tell, and the test suite must never reach a real desktop.
    pub(super) fn notify(&mut self, urgency: &str, title: &str, body: &str) {
        if self.exec.env.get("MAINTAINER_NOTIFY").unwrap_or("on") == "off" {
            return;
        }
        let display = self.session_display();
        let uid = self.exec.capture(&["id", "-u"], &[]).stdout_text();
        let bus = format!("unix:path=/run/user/{uid}/bus");
        let done = self.exec.capture(
            &["notify-send", "-u", urgency, "-a", "maintainer", title, body],
            &[("DISPLAY", &display), ("DBUS_SESSION_BUS_ADDRESS", &bus)],
        );
        put(self.svc.out, &done.stdout);
    }

    /// `printf '%s\n' "$1" | tee -a "$log" >&2`.
    pub(super) fn tee(&mut self, message: &str) {
        let line = format!("{message}\n");
        self.say_err(&line);
        self.append_log(&line);
    }

    /// `$HELPER failed task message log`, whose own output is discarded.
    fn record_failure(&mut self, message: &str) {
        let log = self.log().to_string_lossy().into_owned();
        let (helper, task) = (self.helper.clone(), self.task.clone());
        let done = self.exec.capture(&[&helper, "failed", &task, message, &log], &[]);
        if !done.ok() {
            self.say_err("run.sh: could not even record the failure marker\n");
        }
    }

    fn body_for(&self, message: &str) -> String {
        let human = humanise(message, &self.lock_wait, &self.gh_account);
        let log = format!("log  {}", self.log().display());
        toast_body(&[&human, &log])
    }

    /// A failure that retries itself, reported without interrupting anybody.
    ///
    /// 2026-09-08 produced six unit failures and up to twelve critical popups,
    /// because each alerted twice. The record is unchanged and only the urgency
    /// drops, because a critical popup six times a day for a condition that heals
    /// itself teaches you to dismiss the one that matters.
    pub(super) fn alert_transient(&mut self, message: &str) {
        self.tee(message);
        self.record_failure(message);
        let title = format!("maintainer · {} {} · will retry", self.profile, self.task);
        let body = self.body_for(message);
        self.notify("low", &title, &body);
    }

    /// The alert: the disk first, then the desktop, then whatever the site uses.
    pub(super) fn alert(&mut self, message: &str) {
        self.tee(message);
        // A preview reports the refusal and stops there: recording it would mark
        // the task failed until the next real success.
        if self.show {
            return;
        }
        self.record_failure(message);
        let title = format!("maintainer · {} {} · FAILED", self.profile, self.task);
        let body = self.body_for(message);
        self.notify("critical", &title, &body);
        let hook = self.var_or("MAINTAINER_ALERT_CMD", "").to_string();
        if hook.is_empty() {
            return;
        }
        let task = format!("{}/{}", self.profile, self.task);
        let log = self.log();
        let log_text = log.to_string_lossy().into_owned();
        let argv0 = self.argv0.clone();
        let rc = self.exec.to_log(
            &["sh", "-c", &hook, &argv0, message],
            &[("MAINTAINER_ALERT_TASK", &task), ("MAINTAINER_ALERT_LOG", &log_text)],
            &log,
        );
        if rc != 0 {
            // Failure here is reported and never fatal: an alert path that can
            // abort a run is worse than no alert path.
            self.append_log(
                "run.sh: MAINTAINER_ALERT_CMD failed; the marker and the log still stand\n",
            );
        }
    }

    // ----- lines 302 to 368 ------------------------------------------------------

    pub(super) fn backend_ctx(&self) -> BackendCtx {
        let mut vars = self.vars.clone();
        for (name, value) in &self.exported {
            vars.insert(name.clone(), value.clone());
        }
        let scripts = self.tree.root.join("scripts");
        BackendCtx {
            profile_dir: self.profile_dir.clone(),
            repo_path: self.repo_path.clone(),
            task: self.task.clone(),
            home: PathBuf::from(self.exec.env.get("HOME").unwrap_or_default()),
            scripts_dir: std::fs::metadata(&scripts).is_ok_and(|m| m.is_dir()).then_some(scripts),
            vars,
        }
    }

    pub(super) fn host(&self) -> RealHost {
        RealHost::new(OsStr::new(self.exec.env.get("PATH").unwrap_or_default()))
    }

    pub(super) fn backend(&self) -> &dyn Backend {
        self.backend.as_deref().expect("the backend was chosen before it is used")
    }

    /// Chooses the backend and checks what a rehearsal and the task need of it.
    pub fn choose_backend(&mut self) -> Flow {
        self.post = self.var_or("POST", "on").to_string();
        let post = self.post.clone();
        // Exported, because the wall governs what the agent types and not what the
        // tools it may call then do: `maintainer-repo prune` pushes branch
        // deletions from inside a script, which no deny rule can see.
        self.export("MAINTAINER_POST", &post);
        let name = self.required("BACKEND", 311)?;
        self.backend_name.clone_from(&name);
        // Decision D18: the bash joins BACKEND into a path and sources it, so a
        // name with a slash or a dot-dot reaches any file. A backend here is one of
        // the names the backends crate knows, and every other string is refused.
        let refused_name = name.contains('/') || name.contains("..");
        let found = if refused_name { None } else { (self.svc.backend)(&name) };
        let Some(backend) = found else {
            if !refused_name {
                let dir = self.tree.backend_dir.display().to_string();
                self.bash_error(311, &format!("{dir}/{name}.sh: No such file or directory"));
            }
            self.alert(&format!("no backend '{name}'"));
            return Err(Stop(64));
        };
        self.backend = Some(backend);
        self.check_rehearsal()?;
        self.stamp_version();
        self.write_header();
        self.check_backend()?;
        self.check_allowed_tasks()
    }

    /// A backend with no per-command control cannot honour POST=off, and a
    /// rehearsal needs its deny wall (lines 313 to 339).
    fn check_rehearsal(&mut self) -> Flow {
        let name = self.backend_name.clone();
        if !self.show && self.post == "off" && !self.backend().declares_rehearsal() {
            self.alert(&format!(
                "backend '{name}' cannot enforce POST=off (no per-command deny list); use claude or opencode"
            ));
            return Err(Stop(78));
        }
        if self.post != "off" {
            return Ok(());
        }
        let wall = self.backend().rehearsal_wall(&self.backend_ctx(), &self.host());
        match wall {
            Some(Export { name, value }) => {
                self.export(&name, &value);
                Ok(())
            }
            None if self.show => {
                self.say_err(
                    "run.sh: (preview) no rehearsal wall is rendered in this tree; a real\n",
                );
                self.say_err("        run would refuse here. ./install.sh renders it.\n");
                Ok(())
            }
            None => {
                let line = format!(
                    "run.sh: POST=off but backend '{name}' has no rehearsal wall rendered for '{}'\n        re-run ./install.sh, which renders it beside the live wall\n",
                    self.profile
                );
                self.say_err(&line);
                Err(Stop(78))
            }
        }
    }

    /// The deployed tree stamps itself at install time (lines 341 to 347).
    fn stamp_version(&mut self) {
        let file = std::fs::read(self.tree.root.join("VERSION")).unwrap_or_default();
        let version = stamp_value(&file, "version");
        let commit = stamp_value(&file, "commit");
        self.version = if version.is_empty() { "unstamped".to_string() } else { version };
        self.commit = if commit.is_empty() { "unknown".to_string() } else { commit };
        let (version, commit) = (self.version.clone(), self.commit.clone());
        self.export("MAINTAINER_VERSION", &version);
        self.export("MAINTAINER_COMMIT", &commit);
    }

    pub(super) fn date_iso(&self) -> String {
        self.exec.capture(&["date", "-Is"], &[]).stdout_text()
    }

    fn write_header(&mut self) {
        let when = self.date_iso();
        let header = format!(
            "=== run.sh {}/{} {when} ===\nmaintainer={} backend={} model={} log={}\n",
            self.profile,
            self.task,
            self.version,
            self.backend().name(),
            self.model,
            self.log().display()
        );
        self.append_log(&header);
    }

    fn check_backend(&mut self) -> Flow {
        if self.show {
            return Ok(());
        }
        let checked = self.backend().check(&self.backend_ctx(), &self.host());
        if let Err(message) = checked {
            let message = message.trim_end_matches('\n').to_string();
            let name = self.backend_name.clone();
            self.alert(&format!("backend {name} unusable: {message}"));
            return Err(Stop(1));
        }
        Ok(())
    }

    /// A backend may declare the only tasks it is fit for (lines 358 to 368).
    fn check_allowed_tasks(&mut self) -> Flow {
        let Some(allowed) = self.backend().allowed_tasks() else {
            return Ok(());
        };
        let allowed = allowed.join(" ");
        if !format!(" {allowed} ").contains(&format!(" {} ", self.task)) {
            let (name, task) = (self.backend_name.clone(), self.task.clone());
            self.alert(&format!("backend '{name}' may only run: {allowed} (refused '{task}')"));
            return Err(Stop(78));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_comes_from_the_matching_lines_only() {
        let text = b"# comment\nversion=1.2.3\ncommit=abc\nversionx=no\n";
        assert_eq!(stamp_value(text, "version"), "1.2.3");
        assert_eq!(stamp_value(text, "commit"), "abc");
        assert_eq!(stamp_value(text, "missing"), "");
    }

    #[test]
    fn two_version_lines_are_joined_as_the_command_substitution_joins_them() {
        assert_eq!(stamp_value(b"version=1\nversion=2\n", "version"), "1\n2");
    }

    #[test]
    fn an_empty_value_is_empty_and_so_reads_as_unstamped() {
        assert_eq!(stamp_value(b"version=\n", "version"), "");
    }
}
