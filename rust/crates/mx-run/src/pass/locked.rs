//! Lines 380 to 622 of `run.sh`: everything between "the run is allowed" and "the
//! agent may start". The lock, the second cadence gate, the lease, the identity
//! pin, the exit scrub, the checkout, `maintainer start` and the log rename.

#[allow(
    clippy::wildcard_imports,
    reason = "the steps of one pass are split over files and share one vocabulary"
)]
use super::*;

/// The name of the session directory the Claude CLI files a run under: the
/// working directory with every `/` and `.` turned into `-`.
#[must_use]
pub fn session_dir_name(repo_path: &str) -> String {
    repo_path.chars().map(|c| if c == '/' || c == '.' { '-' } else { c }).collect()
}

/// The run id of `maintainer start`'s output: the first line shaped like
/// `=== word ... ===`, where the word is lowercase letters and hyphens, as
/// `sed -n 's/^=== [a-z-]* \(.*\) ===$/\1/p' | head -1` reads it.
#[must_use]
pub fn run_id_of(context: &str) -> Option<String> {
    for line in context.split('\n') {
        let Some(rest) = line.strip_prefix("=== ") else { continue };
        let Some(body) = rest.strip_suffix(" ===") else { continue };
        let word_len = body.bytes().take_while(|b| b.is_ascii_lowercase() || *b == b'-').count();
        if body.as_bytes().get(word_len) == Some(&b' ') {
            return Some(body[word_len + 1..].to_string());
        }
    }
    None
}

/// What the exit trap does about the lease: `run-lease --clear`, whose failure
/// changes nothing.
fn clear_lease(exec: &Exec, helper: &str, cell: &SharedCell) {
    let (log, cwd, env) = {
        let state = shared(cell);
        (state.log.clone(), state.cwd.clone(), state.env.clone())
    };
    let mut exec = exec.clone();
    exec.cwd = cwd;
    exec.env = env;
    exec.checkpoint = false;
    let _ = exec.to_log(&[helper, "run-lease", "--clear"], &[], &log);
}

/// `[ -n "$prompt_file" ] && rm -f "$prompt_file"`.
pub(super) fn remove_prompt_file(cell: &SharedCell) {
    let file = shared(cell).prompt_file.clone();
    if let Some(file) = file.filter(|file| !file.as_os_str().is_empty()) {
        drop(std::fs::remove_file(file));
    }
}

/// What the scrub trap needs. The token is a secret: this type has no `Debug`.
struct ScrubJob {
    token: String,
    exec: Exec,
    helper: String,
    cell: SharedCell,
    state_dir: PathBuf,
    repo_path: String,
    home: PathBuf,
}

/// `scrub_pinned_token`: remove the pinned token from what the run left behind,
/// clear the lease, remove the prompt file.
///
/// The word list of the bash (`$where`) is split on blanks, and so is this one: a
/// state directory with a space in its name scrubs the wrong paths in both. It is
/// listed under "Found, not fixed".
fn scrub_pinned_token(job: &ScrubJob) {
    let session = job.home.join(".claude/projects").join(session_dir_name(&job.repo_path));
    let mut words = job.state_dir.to_string_lossy().into_owned();
    if std::fs::metadata(&session).is_ok_and(|meta| meta.is_dir()) {
        words.push(' ');
        words.push_str(&session.to_string_lossy());
    }
    let targets: Vec<PathBuf> = words.split_whitespace().map(PathBuf::from).collect();
    let log = shared(&job.cell).log.clone();
    if let Ok(report) = scrub_secret(&Secret::new(job.token.clone()), &targets) {
        append(&log, &report.render());
    }
    clear_lease(&job.exec, &job.helper, &job.cell);
    remove_prompt_file(&job.cell);
}

impl Pass<'_, '_> {
    /// `trap clear_run_lease EXIT`.
    fn arm_lease_trap(&self) {
        let (exec, helper, cell) =
            (self.exec.clone(), self.helper.clone(), Arc::clone(&self.shared));
        trap::arm(move || clear_lease(&exec, &helper, &cell));
    }

    /// `trap scrub_pinned_token EXIT`, which replaces the lease trap and does its work.
    fn arm_scrub_trap(&self, token: &str) {
        let job = ScrubJob {
            token: token.to_string(),
            exec: self.exec.clone(),
            helper: self.helper.clone(),
            cell: Arc::clone(&self.shared),
            state_dir: self.state_dir.clone(),
            repo_path: self.repo_path.to_string_lossy().into_owned(),
            home: PathBuf::from(self.exec.env.get("HOME").unwrap_or_default()),
        };
        trap::arm(move || scrub_pinned_token(&job));
    }

    /// `trap remove_prompt_file EXIT`, which only a preview installs.
    pub(super) fn arm_preview_trap(&self) {
        let cell = Arc::clone(&self.shared);
        trap::arm(move || remove_prompt_file(&cell));
    }

    /// Every task shares one working tree and `maintainer start` checks out main.
    /// Two at once would fight over HEAD and over target/, and the loser would
    /// review a tree it did not create. Wait rather than fail: a skipped run is a
    /// silent gap in the audit trail (lines 386 to 407).
    fn take_the_lock(&mut self) -> Flow {
        if !self.host().on_path("flock") {
            self.alert("flock is required to run; install flock and ensure it is on PATH");
            return Err(Stop(1));
        }
        let lock_file = self.state_dir.join("run.lock");
        // Decision D10: a failed `exec 9>` is its own refusal. The bash went on
        // and the flock that followed failed on the missing descriptor, so the run
        // reported a held lock.
        match (self.svc.open_lock)(&lock_file) {
            Ok(lock) => self.lock = Some(lock),
            Err(error) => {
                let why = error.to_string();
                self.alert(&format!(
                    "could not open the run lock {}: {why}; refusing to run",
                    lock_file.display()
                ));
                return Err(Stop(1));
            }
        }
        let wait = self.lock_wait.clone();
        let done = self.exec.capture(&["flock", "-w", &wait, "9"], &[]);
        put(self.svc.out, &done.stdout);
        put(self.svc.err, &done.stderr);
        match done.rc {
            0 => Ok(()),
            // 75, not 1. Another run of this profile is still working; this one is
            // superseded rather than broken, and the next catch-up slot picks it up.
            1 => {
                self.alert_transient(&format!(
                    "another run held the lock for over {wait}s, so this one never started"
                ));
                Err(Stop(75))
            }
            64 => {
                self.alert(&format!(
                    "flock rejected MAINTAINER_LOCK_WAIT={wait}; set it to a valid non-negative number"
                ));
                Err(Stop(1))
            }
            rc => {
                self.alert(&format!(
                    "flock failed to acquire the run lock (exit {rc}); check flock and the run.lock file"
                ));
                Err(Stop(1))
            }
        }
    }

    /// A file beside the lock names the task and pid, so an interactive session can
    /// ask `maintainer status` instead of running `ps`. It only labels: whether
    /// the run is live is decided by asking run.lock.
    fn write_lease(&mut self) {
        let (helper, task) = (self.helper.clone(), self.task.clone());
        let pid = std::process::id().to_string();
        let log = self.log();
        let rc = self.exec.to_log(&[&helper, "run-lease", &task, &pid], &[], &log);
        if rc != 0 {
            self.append_log(
                "run-lease: could not write leases/run.json; the run goes on without it\n",
            );
        }
        self.arm_lease_trap();
    }

    /// `GH_TRIES` and `GH_BACKOFF` as whole numbers.
    ///
    /// Decision `P2A-x1`, not on the plan: the bash tests them with `[`, which fails
    /// on a word and leaves the retry loop with no way to end. A word is refused
    /// here.
    fn retry_settings(&mut self) -> Flow<(i64, i64)> {
        let read = |name: &str, default: &str| -> Result<i64, String> {
            let text = self.var_or(name, default).to_string();
            cadence::parse_hours(&text).ok_or(text)
        };
        let tries = read("GH_TRIES", "3");
        let backoff = read("GH_BACKOFF", "20");
        match (tries, backoff) {
            (Ok(tries), Ok(backoff)) => Ok((tries, backoff)),
            (Err(text), _) => {
                self.say_err(&format!(
                    "run.sh: GH_TRIES='{text}' is not a whole number; refusing to run\n"
                ));
                Err(Stop(64))
            }
            (_, Err(text)) => {
                self.say_err(&format!(
                    "run.sh: GH_BACKOFF='{text}' is not a whole number; refusing to run\n"
                ));
                Err(Stop(64))
            }
        }
    }

    /// Ask GitHub who it is, retrying an outage and refusing a wrong account or a
    /// rejected token at the first answer (lines 439 to 493).
    fn confirm_account(&mut self) -> Flow {
        let account = self.gh_account.clone();
        let log = self.log();
        let _ = self.exec.to_log(
            &["gh", "auth", "switch", "--hostname", "github.com", "--user", &account],
            &[],
            &log,
        );
        let (tries, backoff) = self.retry_settings()?;
        let mut attempt: i64 = 1;
        let login = loop {
            let done = self.exec.capture(&["gh", "api", "user", "--jq", ".login"], &[]);
            append(&self.log(), &done.stderr);
            let answer = identity::classify_answer(done.rc, &done.stdout_text(), &done.stderr);
            match identity::decide_attempt(&answer, attempt, tries) {
                Next::Accept(login) => break Some(login),
                Next::Refuse401 => {
                    if let Answer::Unauthorized { body } = &answer {
                        self.append_log(&format!("{body}\n"));
                    }
                    let message =
                        identity::rejected_token_message(&account, &self.log().to_string_lossy());
                    self.alert(&message);
                    return Err(Stop(1));
                }
                Next::GiveUp => break None,
                Next::Retry => {
                    self.append_log(&format!(
                        "gh api user gave no answer (try {attempt} of {tries}); retrying\n"
                    ));
                    if let Some(seconds) = identity::backoff_seconds(attempt, backoff) {
                        drop(self.exec.capture(&["sleep", &seconds.to_string()], &[]));
                    }
                    attempt += 1;
                }
            }
        };
        let log_text = self.log().to_string_lossy().into_owned();
        let Some(login) = login else {
            // 75 is EX_TEMPFAIL: nothing is wrong with the credentials, the run could
            // not ask. The next timer tick is the fix.
            self.alert_transient(&identity::unreachable_message(tries, &log_text));
            return Err(Stop(75));
        };
        if !identity::same_login(&login, &account) {
            self.alert(&identity::wrong_account_message(&login, &account, &log_text));
            return Err(Stop(1));
        }
        self.append_log(&format!("gh identity: {login}\n"));
        Ok(())
    }

    /// Pin the run to this account's token, not just to the active-account switch.
    /// With `GH_TOKEN` exported for the whole run, every gh call, the agent's
    /// included, authenticates with this token and no `gh auth switch` can select
    /// another account (lines 495 to 555).
    fn pin_token(&mut self) -> Flow {
        let account = self.gh_account.clone();
        let done = self.exec.capture(&["gh", "auth", "token", "--user", &account], &[]);
        append(&self.log(), &done.stderr);
        let token = done.stdout_text();
        let log_text = self.log().to_string_lossy().into_owned();
        if token.is_empty() {
            self.alert(&identity::no_token_message(&account, &log_text));
            return Err(Stop(1));
        }
        // Decision D15: the scrub refuses a token under 16 characters, which the
        // bash ignored, so such a token stayed in whatever the run left behind.
        if !identity::token_can_be_scrubbed(&token) {
            self.alert(&identity::short_token_message(&account, &log_text));
            return Err(Stop(1));
        }
        self.export("GH_TOKEN", &token);
        self.unset("GITHUB_TOKEN");
        // Every exit path from here on, including the transient exit 75 and the
        // refusals below, passes through the scrub.
        self.arm_scrub_trap(&token);
        let done = self.exec.capture(&["gh", "api", "user", "--jq", ".login"], &[]);
        append(&self.log(), &done.stderr);
        match identity::judge_pinned(done.rc, &done.stdout_text(), &account) {
            PinnedVerdict::Confirmed => {
                self.append_log(&format!(
                    "gh identity pinned by token for the whole run: {account}\n"
                ));
                Ok(())
            }
            PinnedVerdict::Unresolved { rc } => {
                // 75, not 1: nothing is wrong with the credentials, the run could not ask.
                self.alert_transient(&identity::pinned_unresolved_message(rc, &log_text));
                Err(Stop(75))
            }
            PinnedVerdict::WrongAccount { login } => {
                self.alert(&identity::pinned_wrong_account_message(&login, &account, &log_text));
                Err(Stop(1))
            }
        }
    }

    /// A review bot must own the tree it reviews, so `REPO_PATH` is a checkout this
    /// profile owns, cloned from `REPO_ORIGIN` when it is missing (lines 568 to 579).
    fn provision_checkout(&mut self) -> Flow {
        let git_dir = self.repo_path.join(".git");
        if std::fs::metadata(&git_dir).is_ok_and(|meta| meta.is_dir()) {
            return Ok(());
        }
        let path = self.repo_path.to_string_lossy().into_owned();
        let log_text = self.log().to_string_lossy().into_owned();
        let origin = self.var_or("REPO_ORIGIN", "").to_string();
        if origin.is_empty() {
            self.alert(&format!(
                "{path} is not a git checkout and the profile sets no REPO_ORIGIN to clone from; refusing to run. See {log_text}"
            ));
            return Err(Stop(1));
        }
        self.append_log(&format!("provisioning review checkout: {origin} -> {path}\n"));
        let log = self.log();
        if self.exec.to_log(&["git", "clone", "--quiet", &origin, &path], &[], &log) != 0 {
            self.alert(&format!(
                "could not clone {origin} into {path}; no review checkout, nothing ran. See {log_text}"
            ));
            return Err(Stop(1));
        }
        Ok(())
    }

    /// Refresh the checkout and compute what moved since the last run of this
    /// task, then take the run id from the first line (lines 581 to 593).
    fn start_run(&mut self) -> Flow {
        let (helper, task) = (self.helper.clone(), self.task.clone());
        let done = self.exec.capture(&[&helper, "start", &task], &[]);
        let context = done.merged();
        self.append_log(&format!("{context}\n"));
        let log_text = self.log().to_string_lossy().into_owned();
        if !done.ok() {
            self.alert(&format!("refresh failed, no agent run started. See {log_text}"));
            return Err(Stop(1));
        }
        let Some(run_id) = run_id_of(&context).filter(|id| !id.is_empty()) else {
            self.alert(&format!("could not parse a run id from '{helper} start'. See {log_text}"));
            return Err(Stop(1));
        };
        self.context = context;
        self.run_id = run_id;
        Ok(())
    }

    /// The log was named from this script's start time, minutes before the lock was
    /// taken; the run id is minted by `maintainer start`, after it. Reconciled here,
    /// keeping everything the pre-lock log already recorded (lines 595 to 617).
    fn rename_log(&mut self) {
        let Some(logs) = self.logs_dir.clone() else { return };
        let by_id = logs.join(format!("{}.log", self.run_id));
        let current = self.log();
        if current != by_id {
            if by_id.exists() {
                // Never clobber an existing run's transcript; fold into it instead.
                let bytes = std::fs::read(&current).unwrap_or_default();
                append(&by_id, &bytes);
                drop(std::fs::remove_file(&current));
            } else {
                drop(std::fs::rename(&current, &by_id));
            }
            shared(&self.shared).log = by_id;
            let line = format!("log renamed to match run id {}\n", self.run_id);
            self.append_log(&line);
        }
        let (helper, task) = (self.helper.clone(), self.task.clone());
        let pid = std::process::id().to_string();
        let run_id = self.run_id.clone();
        let log = self.log();
        if self.exec.to_log(&[&helper, "run-lease", &task, &pid, &run_id], &[], &log) != 0 {
            self.append_log("run-lease: could not add the run id to leases/run.json\n");
        }
    }

    /// A real run: the lock, the second gate, the lease, the identity, the checkout,
    /// the refresh and the log name.
    pub fn lock_and_identify(&mut self) -> Flow {
        self.take_the_lock()?;
        // And read the cadence gate again, now that we hold the lock. A run ahead of
        // this one promotes the stamp while this one waits.
        self.gate(Some("after waiting for the lock"))?;
        self.write_lease();
        self.confirm_account()?;
        self.pin_token()?;
        self.provision_checkout()?;
        self.start_run()?;
        self.rename_log();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_id_is_what_the_first_header_line_carries() {
        let context = "=== maintainer 2026-01-01T00-00-review ===\nskill        x\n";
        assert_eq!(run_id_of(context), Some("2026-01-01T00-00-review".to_string()));
    }

    #[test]
    fn the_first_line_shaped_like_a_header_wins_whatever_its_word() {
        let context = "=== note extra ===\n=== maintainer real ===\n";
        assert_eq!(run_id_of(context), Some("extra".to_string()));
    }

    #[test]
    fn a_header_whose_first_word_has_a_capital_is_not_a_run_id() {
        assert_eq!(run_id_of("=== Maintainer id ==="), None);
        assert_eq!(run_id_of("=== maintainer1 id ==="), None);
    }

    #[test]
    fn lines_that_are_not_headers_give_no_run_id() {
        assert_eq!(run_id_of("no header here\n"), None);
        assert_eq!(run_id_of("=== word ===\n"), None);
        assert_eq!(run_id_of("==== word id ===\n"), None);
        assert_eq!(run_id_of(""), None);
    }

    #[test]
    fn a_header_with_an_empty_word_still_has_a_run_id() {
        assert_eq!(run_id_of("===  id ==="), Some("id".to_string()));
    }

    #[test]
    fn a_header_whose_id_is_empty_gives_an_empty_run_id() {
        assert_eq!(run_id_of("=== word  ==="), Some(String::new()));
    }

    #[test]
    fn the_run_id_keeps_everything_up_to_the_closing_marker() {
        assert_eq!(run_id_of("=== a b c ==="), Some("b c".to_string()));
        assert_eq!(run_id_of("=== a b === ==="), Some("b ===".to_string()));
    }

    #[test]
    fn the_session_directory_name_turns_slashes_and_dots_into_hyphens() {
        assert_eq!(session_dir_name("/home/u/work.dir/repo"), "-home-u-work-dir-repo");
    }
}
