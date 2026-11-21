//! One unattended pass: the lifecycle of `lib/run.sh`, step for step.
//!
//! The steps run in the bash's order and each one's comment names the lines of
//! `run.sh` it ports. Where the port differs on purpose the comment carries the
//! decision number (`D10` to `D19` from the plan, `P2A-n` for this crate).
//!
//! A step that ends the program returns [`Stop`] with the exit code, after it has
//! printed what the bash prints. The exit trap runs from [`crate::run`] on every
//! way out.

use crate::cadence::{self, CadenceFacts, Decision, Stamp};
use crate::env::Env;
use crate::exec::{Adapters, Exec, append, trimmed_text};
use crate::host::RealHost;
use crate::humanise::{humanise, toast_body};
use crate::identity::{self, Answer, Next, PinnedVerdict};
use crate::lock::{self, LockOpenError};
use crate::prompt::{self, PromptParts};
use crate::trap;
use crate::tree::{self, Tree};
use mx_backends::{Backend, BackendCtx, Export, Host, RunRequest};
use mx_ports::Secret;
use mx_profile::classify::{ClassifyEnv, classify_failure, effective_locale};
use mx_profile::load::{LoadEnv, LoadError, load_for_doctor};
use mx_profile::scrub::scrub_secret;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

mod close;
mod locked;
mod setup;

pub use close::AgentEnd;
use locked::remove_prompt_file;

/// bash's compiled-in default when `PATH` is unset.
const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// The program ends with this exit code. What the bash prints is already out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stop(pub i32);

/// The result of a step that can end the program.
pub type Flow<T = ()> = Result<T, Stop>;

/// What the program was started with.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// `argv[0]`, which names the program in messages and says where the tree is.
    pub argv0: OsString,
    /// The running executable, used when `argv0` has no slash.
    pub exe: PathBuf,
    /// The arguments after `argv[0]`.
    pub args: Vec<String>,
    /// The working directory at start.
    pub cwd: PathBuf,
    /// The environment at start.
    pub env: Env,
}

/// What the program reaches the world through.
pub struct Services<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    /// Builds the process adapters for a search path and a locale. Called at the
    /// start and again after `PATH` gets its prefix.
    pub adapters: &'a dyn Fn(&OsStr, Option<String>) -> Adapters,
    /// `mx_backends::backend_by_name`, or a fake in a test.
    pub backend: &'a dyn Fn(&str) -> Option<Box<dyn Backend>>,
    /// `exec 9>"$path"`: [`lock::open_run_lock`], or a fake in a test that cannot
    /// claim descriptor 9.
    pub open_lock: &'a dyn Fn(&Path) -> Result<lock::RunLock, LockOpenError>,
}

/// The names the child bash that evaluates the profile must never inherit: they
/// make a shell read a file or change its options before the profile runs.
const SHELL_STARTUP_NAMES: [&str; 8] =
    ["BASH_ENV", "ENV", "SHELLOPTS", "BASHOPTS", "PS4", "PROMPT_COMMAND", "PATH", "LC_ALL"];

/// The usage text of the bash's `${N:?}` expansion.
const USAGE: &str = "usage: run.sh [--show-prompt] <profile> <task>";

/// What the exit trap reads at the moment it runs, as the bash trap reads shell
/// variables then: the log can be renamed and the prompt file made after the trap
/// is set, and the working directory changes when the run enters the checkout.
#[derive(Debug, Clone, Default)]
struct Shared {
    log: PathBuf,
    prompt_file: Option<PathBuf>,
    cwd: Option<PathBuf>,
    /// The exported environment as it stands: a trap runs with what the shell had
    /// exported when it ran, including what a backend exported after the trap was set.
    env: Env,
}

type SharedCell = Arc<Mutex<Shared>>;

fn shared(cell: &SharedCell) -> MutexGuard<'_, Shared> {
    cell.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Everything one pass knows.
pub struct Pass<'a, 'b> {
    inv: &'a Invocation,
    svc: &'a mut Services<'b>,
    exec: Exec,
    shared: SharedCell,
    /// The name messages use for the program.
    argv0: String,
    tree: Tree,
    show: bool,
    profile: String,
    task: String,
    /// The shell variables the profile left, with the `MAINTAINER_<KEY>` overrides.
    vars: BTreeMap<String, String>,
    /// What `run.sh` itself exported, which wins over the profile in a backend's view.
    exported: BTreeMap<String, String>,
    profile_dir: PathBuf,
    state_dir: PathBuf,
    repo_path: PathBuf,
    repo_slug: String,
    gh_account: String,
    helper: String,
    lock_wait: String,
    model: String,
    post: String,
    /// `BACKEND` as the profile wrote it.
    backend_name: String,
    backend: Option<Box<dyn Backend>>,
    forced: bool,
    logs_dir: Option<PathBuf>,
    version: String,
    commit: String,
    /// What `maintainer start` printed, or the preview's stand-in.
    context: String,
    run_id: String,
    /// Descriptor 9, held until the pass ends.
    lock: Option<lock::RunLock>,
}

fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

impl<'a, 'b> Pass<'a, 'b> {
    /// Starts a pass: the tree, the arguments, the profile and the task. Ends the
    /// program for the refusals that come before any state is touched.
    pub fn begin(inv: &'a Invocation, svc: &'a mut Services<'b>) -> Flow<Self> {
        let locale = inv.env.get("LC_ALL").map(str::to_string);
        let path = OsString::from(inv.env.get("PATH").unwrap_or(DEFAULT_PATH));
        let exec = Exec {
            adapters: (svc.adapters)(&path, locale.clone()),
            env: inv.env.clone(),
            cwd: None,
            locale,
            checkpoint: true,
        };
        let argv0 = inv.argv0.to_string_lossy().into_owned();
        let program = if argv0.contains('/') { PathBuf::from(&argv0) } else { inv.exe.clone() };
        let tree = match tree::locate(&program, &inv.cwd) {
            Ok(tree) => tree,
            Err(error) => {
                put(svc.err, error.message().as_bytes());
                return Err(Stop(64));
            }
        };
        let mut pass = Self {
            inv,
            svc,
            exec,
            shared: Arc::new(Mutex::new(Shared { env: inv.env.clone(), ..Shared::default() })),
            argv0,
            tree,
            show: false,
            profile: String::new(),
            task: String::new(),
            vars: BTreeMap::new(),
            exported: BTreeMap::new(),
            profile_dir: PathBuf::new(),
            state_dir: PathBuf::new(),
            repo_path: PathBuf::new(),
            repo_slug: String::new(),
            gh_account: String::new(),
            helper: String::new(),
            lock_wait: String::new(),
            model: String::new(),
            post: String::new(),
            backend_name: String::new(),
            backend: None,
            forced: false,
            logs_dir: None,
            version: String::new(),
            commit: String::new(),
            context: String::new(),
            run_id: String::new(),
            lock: None,
        };
        pass.check_profile_library()?;
        pass.read_arguments()?;
        pass.refuse_nested_run()?;
        pass.load_profile()?;
        pass.export_profile()?;
        pass.read_task()?;
        Ok(pass)
    }

    // ----- output ---------------------------------------------------------------

    fn say_err(&mut self, text: &str) {
        put(self.svc.err, text.as_bytes());
    }

    fn say_out(&mut self, text: &str) {
        put(self.svc.out, text.as_bytes());
    }

    /// `line N: ` messages of the bash, which name the script and a line.
    fn bash_error(&mut self, line: u32, what: &str) {
        let text = format!("{}: line {line}: {what}\n", self.argv0);
        self.say_err(&text);
    }

    fn export(&mut self, name: &str, value: &str) {
        self.exec.env.set(name, value);
        self.exported.insert(name.to_string(), value.to_string());
        self.sync_env();
    }

    /// `unset NAME`.
    fn unset(&mut self, name: &str) {
        self.exec.env.unset(name);
        self.exported.remove(name);
        self.sync_env();
    }

    /// Lets the exit trap see the environment as it is now.
    fn sync_env(&self) {
        shared(&self.shared).env = self.exec.env.clone();
    }

    fn var(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }

    /// `${NAME:-default}`.
    fn var_or<'s>(&'s self, name: &str, default: &'s str) -> &'s str {
        self.var(name).filter(|value| !value.is_empty()).unwrap_or(default)
    }

    /// `"$NAME"` under `set -u`: an unset name ends the program with the shell's message.
    fn required(&mut self, name: &str, line: u32) -> Flow<String> {
        if let Some(value) = self.var(name) {
            return Ok(value.to_string());
        }
        self.bash_error(line, &format!("{name}: unbound variable"));
        Err(Stop(1))
    }

    fn log(&self) -> PathBuf {
        shared(&self.shared).log.clone()
    }

    fn append_log(&self, text: &str) {
        append(&self.log(), text.as_bytes());
    }

    // ----- lines 23 to 73: the tree, the arguments, the nested-run guard ---------

    /// Lines 34 to 51: the exit scrub must exist, or the log would read as though
    /// it cleaned up.
    fn check_profile_library(&mut self) -> Flow {
        let text = std::fs::read(&self.tree.profile_lib).unwrap_or_default();
        if tree::defines_scrub_function(&text) {
            return Ok(());
        }
        let line = format!(
            "{}: {} defines no maintainer_scrub_secret, so the exit scrub would be a no-op\n",
            "run.sh",
            self.tree.profile_lib.display()
        );
        self.say_err(&line);
        Err(Stop(64))
    }

    /// Lines 56 to 59. A missing argument is the shell's own `${N:?}` message and
    /// exit 1 (decision D19, preserved).
    fn read_arguments(&mut self) -> Flow {
        let mut args = self.inv.args.iter();
        let mut rest: Vec<&String> = Vec::new();
        let first = args.next();
        if first.map(String::as_str) == Some("--show-prompt") {
            self.show = true;
        } else if let Some(first) = first {
            rest.push(first);
        }
        rest.extend(args);
        let word = |index: usize| {
            rest.get(index).map(|text| (*text).clone()).filter(|text| !text.is_empty())
        };
        let (profile, task) = (word(0), word(1));
        let Some(profile) = profile else {
            self.bash_error(58, &format!("1: {USAGE}"));
            return Err(Stop(1));
        };
        let Some(task) = task else {
            self.bash_error(59, &format!("2: {USAGE}"));
            return Err(Stop(1));
        };
        self.profile = profile;
        self.task = task;
        // Decision D13: a GH_TOKEN inherited from the caller must not reach any
        // call before the pin, so the identity check cannot run under it.
        self.unset("GH_TOKEN");
        Ok(())
    }

    /// Lines 61 to 73. A preview is exempt: reading the prompt from inside a run
    /// is harmless.
    fn refuse_nested_run(&mut self) -> Flow {
        let Some(marker) = self.inv.env.non_empty("MAINTAINER_IN_RUN").map(str::to_string) else {
            return Ok(());
        };
        if self.show {
            return Ok(());
        }
        let line = format!(
            "run.sh: already inside the run '{marker}'; refusing to start '{}/{}' from within it\n",
            self.profile, self.task
        );
        self.say_err(&line);
        Err(Stop(78))
    }

    // ----- lines 75 to 112: the profile, its exports, the task and the model -----

    fn load_profile(&mut self) -> Flow {
        self.profile_dir = self.tree.root.join("profiles").join(&self.profile);
        let profile_env = self.profile_dir.join("profile.env");
        if !std::fs::metadata(&profile_env).is_ok_and(|meta| meta.is_file()) {
            let line = format!("run.sh: no profile '{}'\n", self.profile);
            self.say_err(&line);
            return Err(Stop(64));
        }
        let lib_dir = self.tree.profile_lib.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut env = LoadEnv::from_vars(lib_dir, Vec::new());
        env.home = self.exec.env.get("HOME").map(str::to_string);
        env.profile_dir = Some(self.profile_dir.to_string_lossy().into_owned());
        env.maintainer = self
            .exec
            .env
            .iter()
            .filter(|(name, _)| {
                *name != "HOME" && *name != "PROFILE_DIR" && !SHELL_STARTUP_NAMES.contains(name)
            })
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        let load = load_for_doctor(self.exec.adapters.proc.as_ref(), &env, &profile_env);
        trap::checkpoint();
        put(self.svc.out, &load.stdout);
        put(self.svc.err, &load.stderr);
        match load.result {
            Ok(profile) => {
                self.vars = profile
                    .vars()
                    .map(|(name, value)| (name.to_string(), text_of(value)))
                    .collect();
                Ok(())
            }
            Err(error) => {
                put(self.svc.err, &error.message());
                Err(Stop(match error {
                    LoadError::ShellExited { code } => code,
                    _ => 64,
                }))
            }
        }
    }

    /// Lines 83 to 103.
    fn export_profile(&mut self) -> Flow {
        let repo_path = self.required("REPO_PATH", 86)?;
        let repo_slug = self.required("REPO_SLUG", 86)?;
        let state_dir = self.required("STATE_DIR", 86)?;
        let gh_account = self.required("GH_ACCOUNT", 87)?;
        self.repo_path = PathBuf::from(&repo_path);
        self.repo_slug.clone_from(&repo_slug);
        self.state_dir = PathBuf::from(&state_dir);
        self.gh_account.clone_from(&gh_account);
        let profile_dir = self.profile_dir.to_string_lossy().into_owned();
        self.export("PROFILE_DIR", &profile_dir);
        self.export("REPO_PATH", &repo_path);
        self.export("MAINTAINER_REPO", &repo_path);
        self.export("MAINTAINER_SLUG", &repo_slug);
        self.export("MAINTAINER_STATE", &state_dir);
        self.export("MAINTAINER_ACCOUNT", &gh_account);
        let prod_globs = self.var("PROD_GLOBS").unwrap_or_default().to_string();
        self.export("PROD_GLOBS", &prod_globs);
        let claim_label = self.var("CLAIM_LABEL").unwrap_or_default().to_string();
        self.export("CLAIM_LABEL", &claim_label);
        let profile = self.profile.clone();
        self.export("MAINTAINER_PROFILE", &profile);
        let scripts = self.tree.root.join("scripts");
        if std::fs::metadata(&scripts).is_ok_and(|meta| meta.is_dir()) {
            self.export("MAINTAINER_SCRIPTS", &scripts.to_string_lossy());
        }
        Ok(())
    }

    /// Lines 105 to 112, and the first gate.
    fn read_task(&mut self) -> Flow {
        let tasks = self.required("TASKS", 105)?;
        if !format!(" {tasks} ").contains(&format!(" {} ", self.task)) {
            let line = format!("run.sh: unknown task '{}'\n", self.task);
            self.say_err(&line);
            return Err(Stop(64));
        }
        // Decision F7 (preserved): a task that is not a shell variable name cannot
        // be looked up, so the run ends at the model lookup, as the bash does.
        let model_var = format!("MODEL_{}", self.task);
        if !model_var.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
            self.bash_error(106, &format!("{model_var}: invalid variable name"));
            return Err(Stop(1));
        }
        let Some(model) = self.var(&model_var).filter(|value| !value.is_empty()) else {
            let task = self.task.clone();
            self.bash_error(106, &format!("!model_var: no model configured for {task}"));
            return Err(Stop(1));
        };
        self.model = model.to_string();
        let skill = self.var(&format!("SKILL_{}", self.task)).unwrap_or_default().to_string();
        self.export("MAINTAINER_TASKS", &tasks);
        self.export("MAINTAINER_SKILL", &skill);
        let task = self.task.clone();
        self.export("MAINTAINER_TASK", &task);
        Ok(())
    }

    // ----- the cadence gate (lines 125 to 155) -----------------------------------

    /// `date +%s`. A host whose `date` answers nothing reads as the epoch.
    fn now(&self) -> i64 {
        self.exec.capture(&["date", "+%s"], &[]).stdout_text().trim().parse().unwrap_or(0)
    }

    /// `min_gate`. `suffix` says where the call happened.
    fn gate(&mut self, suffix: Option<&str>) -> Flow {
        let min_text = self.var_or(&format!("MIN_HOURS_{}", self.task), "0").to_string();
        let stamp_file = self.state_dir.join("state").join(format!("last-{}.json", self.task));
        let stamp = match std::fs::metadata(&stamp_file) {
            Err(_) => Stamp::Missing,
            Ok(meta) => Stamp::ModifiedAt(
                meta.modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .and_then(|since| i64::try_from(since.as_secs()).ok())
                    .unwrap_or(0),
            ),
        };
        let facts = CadenceFacts { forced: self.forced, min_hours: min_text, stamp };
        // `date` is asked only when the gate has something to decide, as in the bash.
        let needs_clock = !self.forced && facts.min_hours != "0" && stamp != Stamp::Missing;
        let now = if needs_clock { self.now() } else { 0 };
        match cadence::decide(&facts, now) {
            Decision::Run => Ok(()),
            Decision::Skip { age_hours, min_hours } => {
                // `[` reads " 6 " as a number, but `printf %d` takes no trailing blank and
                // says so on standard error before it prints the line.
                if facts.min_hours.ends_with([' ', '\t']) {
                    self.bash_error(142, &format!("printf: {}: invalid number", facts.min_hours));
                }
                let line = cadence::skip_line(&self.task, age_hours, min_hours, suffix);
                self.say_out(&line);
                Err(Stop(0))
            }
            Decision::Refuse { min_hours } => {
                let line = cadence::refusal_line(&self.task, &min_hours);
                self.say_err(&line);
                Err(Stop(64))
            }
        }
    }

    /// Lines 147 to 157: the first gate, the consumed override, the in-run marker.
    pub fn first_gate(&mut self) -> Flow {
        // Read once, because the gate is consulted again after the lock and
        // MAINTAINER_FORCE is unset before then.
        self.forced = self.inv.env.get("MAINTAINER_FORCE") == Some("1");
        if !self.show {
            self.gate(None)?;
        }
        // The override is consumed here and must not travel any further.
        self.unset("MAINTAINER_FORCE");
        let marker = format!("{}/{}", self.profile, self.task);
        self.export("MAINTAINER_IN_RUN", &marker);
        Ok(())
    }
}

/// Writes to a stream, ignoring a failure the way a shell ignores a closed pipe
/// it has nothing better to do about.
fn put(stream: &mut dyn Write, bytes: &[u8]) {
    drop(stream.write_all(bytes));
    drop(stream.flush());
}

impl Pass<'_, '_> {
    /// The whole pass after `begin`, in the bash's order.
    pub fn go(&mut self) -> Flow {
        self.first_gate()?;
        self.prepare()?;
        self.choose_backend()?;
        if self.show {
            self.preview_context();
        } else {
            self.lock_and_identify()?;
        }
        self.assemble_prompt()?;
        if self.show {
            self.print_prompt();
            return Ok(());
        }
        self.enter_checkout()?;
        let end = self.run_agent();
        self.close_run(&end)
    }
}
