//! The lifecycle of a pass, driven in process against a scripted world.
//!
//! Every external program except `bash`, `sed`, `grep`, `tail` and `mktemp` (the
//! profile loader, the doctrine substitution, the failure classifier and the
//! prompt file, which are real) is answered by [`World`], which records each call
//! in one global order with its argument vector, its environment and its working
//! directory. The agent is a closure. No test starts `gh`, `git`, `flock`, a
//! backend or a notification.
//!
//! The exit trap is process-global, so the tests take turns.

use mx_backends::{Backend, BackendCtx, Export, Host, PlanRefusal, RunPlan, RunRequest};
use mx_core::NonEmpty;
use mx_ports::{
    Pipe, PipelineOutcome, PipelineSpec, Proc, ProcError, ProcExit, ProcOutput, ProcSpec, RealProc,
    Sink, Source, StageSpec,
};
use mx_run::env::Env;
use mx_run::exec::Adapters;
use mx_run::lock::{LockOpenError, RunLock};
use mx_run::{Invocation, Services};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

const TOKEN: &str = "ghp_FIXTURE0123456789abcdefghij";
const EPOCH: i64 = 1_767_225_600;
const STAMP: &str = "2026-01-01T00-00";
const ISO: &str = "2026-01-01T00:00:00+00:00";
const RUN_ID: &str = "2026-01-01T00-00-review";
const MASK: &str = "REDACTED_BY_MAINTAINER_ROTATE_THIS_SECRET";

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

// ----- the world ------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Reply {
    rc: i32,
    out: String,
    err: String,
}

fn reply(rc: i32, out: &str) -> Reply {
    Reply { rc, out: out.to_string(), err: String::new() }
}

#[derive(Debug, Clone)]
struct Call {
    key: String,
    argv: Vec<String>,
    env: BTreeMap<String, String>,
    cwd: Option<PathBuf>,
    stdin: Option<PathBuf>,
    /// Which port the call came through: `proc` or `pipe`.
    via: &'static str,
}

type Agent = Box<dyn FnMut(&PipelineSpec) -> Result<PipelineOutcome, ProcError> + Send>;

#[derive(Default)]
struct Inner {
    calls: Vec<Call>,
    queues: HashMap<String, VecDeque<Reply>>,
    fixed: HashMap<String, Reply>,
}

type Hook = Box<dyn FnMut() + Send>;

struct World {
    real: RealProc,
    inner: Mutex<Inner>,
    agent: Mutex<Agent>,
    hooks: Mutex<HashMap<String, Hook>>,
    /// Once set, the pipe port refuses every call, as the live set does after a stop.
    pipe_closed: std::sync::atomic::AtomicBool,
}

const REAL_TOOLS: [&str; 5] = ["bash", "sed", "grep", "tail", "mktemp"];

fn tool_of(argv0: &str) -> String {
    Path::new(argv0).file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn key_of(argv: &[String]) -> String {
    let tool = tool_of(&argv[0]);
    let word = |index: usize| argv.get(index).cloned().unwrap_or_default();
    match tool.as_str() {
        "gh" => format!("gh {} {}", word(1), word(2)),
        "maintainer" | "date" | "id" | "loginctl" => format!("{tool} {}", word(1)),
        _ => tool,
    }
}

impl World {
    fn new() -> Arc<Self> {
        let agent: Agent = Box::new(|spec| {
            let stage = spec.stages.first();
            if let Sink::AppendFile(log) = &stage.stdout {
                append(log, "agent says hello\n");
            }
            Ok(PipelineOutcome { exits: vec![ProcExit::Code(0)] })
        });
        let world = Self {
            real: RealProc::new("/usr/bin:/bin", Vec::new()),
            inner: Mutex::new(Inner::default()),
            agent: Mutex::new(agent),
            hooks: Mutex::new(HashMap::new()),
            pipe_closed: std::sync::atomic::AtomicBool::new(false),
        };
        world.defaults();
        Arc::new(world)
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn fix(&self, key: &str, r: Reply) {
        self.inner().fixed.insert(key.to_string(), r);
        self.inner().queues.remove(key);
    }

    fn queue(&self, key: &str, replies: Vec<Reply>) {
        self.inner().queues.insert(key.to_string(), replies.into());
    }

    fn defaults(&self) {
        self.fix("gh auth switch", reply(0, ""));
        self.fix("gh api user", reply(0, "fxacct\n"));
        self.fix("gh auth token", reply(0, &format!("{TOKEN}\n")));
        self.fix(
            "maintainer start",
            reply(0, &format!("=== maintainer {RUN_ID} ===\nskill        fixture-skill\n")),
        );
        self.fix("maintainer finish", reply(0, &format!("finished {RUN_ID}\n")));
        self.fix("maintainer digest", reply(0, "digest-line-from-helper\n"));
        self.fix("maintainer run-lease", reply(0, ""));
        self.fix("maintainer failed", reply(0, ""));
        self.fix("maintainer abort", reply(0, ""));
        self.fix("date +%s", reply(0, &format!("{EPOCH}\n")));
        self.fix("date +%Y-%m-%dT%H-%M", reply(0, &format!("{STAMP}\n")));
        self.fix("date -Is", reply(0, &format!("{ISO}\n")));
        for key in [
            "flock",
            "sleep",
            "notify-send",
            "git",
            "sh",
            "loginctl show-user",
            "loginctl show-session",
        ] {
            self.fix(key, reply(0, ""));
        }
        self.fix("id -u", reply(0, "1000\n"));
        self.fix("id -un", reply(0, "fixture-user\n"));
    }

    /// Runs `hook` each time `key` is answered, before the answer returns.
    fn on_call(&self, key: &str, hook: Hook) {
        self.hooks.lock().unwrap_or_else(PoisonError::into_inner).insert(key.to_string(), hook);
    }

    fn take(&self, key: &str) -> Reply {
        if let Some(hook) = self.hooks.lock().unwrap_or_else(PoisonError::into_inner).get_mut(key) {
            hook();
        }
        let mut inner = self.inner();
        if let Some(queue) = inner.queues.get_mut(key)
            && let Some(next) = queue.pop_front()
        {
            return next;
        }
        inner.fixed.get(key).cloned().unwrap_or_else(|| panic!("unscripted call: {key}"))
    }

    fn set_agent(&self, agent: Agent) {
        *self.agent.lock().unwrap_or_else(PoisonError::into_inner) = agent;
    }

    fn calls(&self) -> Vec<Call> {
        self.inner().calls.clone()
    }

    fn record(
        &self,
        key: &str,
        argv: &[String],
        env: &[(String, String)],
        cwd: Option<PathBuf>,
        stdin: Option<PathBuf>,
        via: &'static str,
    ) {
        self.inner().calls.push(Call {
            key: key.to_string(),
            argv: argv.to_vec(),
            env: env.iter().cloned().collect(),
            cwd,
            stdin,
            via,
        });
    }
}

fn append(path: &Path, text: &str) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

impl Proc for World {
    fn run(&self, spec: &ProcSpec) -> Result<ProcOutput, ProcError> {
        let argv: Vec<String> = spec.argv.as_slice().to_vec();
        if REAL_TOOLS.contains(&tool_of(&argv[0]).as_str()) {
            return self.real.run(spec);
        }
        let key = key_of(&argv);
        self.record(&key, &argv, &spec.env, spec.cwd.clone(), None, "proc");
        let r = self.take(&key);
        Ok(ProcOutput {
            exit: ProcExit::Code(r.rc),
            stdout: r.out.into_bytes(),
            stderr: r.err.into_bytes(),
        })
    }
}

impl Pipe for World {
    fn run_pipeline(&self, spec: &PipelineSpec) -> Result<PipelineOutcome, ProcError> {
        let stage = spec.stages.first();
        let argv: Vec<String> = stage.argv.as_slice().to_vec();
        let key = key_of(&argv);
        let stdin = match &stage.stdin {
            Source::File(path) => Some(path.clone()),
            _ => None,
        };
        if self.pipe_closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(ProcError::Spawn {
                program: argv[0].clone(),
                reason: "the orchestrator is stopping, so no stage starts".to_string(),
            });
        }
        self.record(&key, &argv, &stage.env, stage.cwd.clone(), stdin, "pipe");
        if tool_of(&argv[0]) == "claude" {
            let mut agent = self.agent.lock().unwrap_or_else(PoisonError::into_inner);
            return (*agent)(spec);
        }
        let r = self.take(&key);
        if let Sink::AppendFile(log) = &stage.stdout {
            append(log, &r.out);
            append(log, &r.err);
        }
        Ok(PipelineOutcome { exits: vec![ProcExit::Code(r.rc)] })
    }
}

// ----- the backend ------------------------------------------------------------------

#[derive(Clone)]
struct FakeBackend {
    rehearsal: bool,
    wall: Option<Export>,
    allowed: Option<&'static [&'static str]>,
    check: Result<(), String>,
    refuse_plan: Option<String>,
    /// Lines the plan asks the executor to append to the log before the pipeline.
    notes: Vec<String>,
    /// What the plan adds to its stage's environment, as `p2b`'s opencode adds its key.
    stage_env: Vec<(String, String)>,
    /// The working directory the plan names for its stage.
    stage_cwd: Option<PathBuf>,
    /// What `check` and `plan` saw in the variable set, as `check: NAME=value`.
    seen: Arc<Mutex<Vec<String>>>,
}

impl FakeBackend {
    fn look(&self, step: &str, ctx: &BackendCtx) {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        for name in ["MAINTAINER_SETTINGS", "MAINTAINER_CURSOR_DIR"] {
            seen.push(format!("{step}: {name}={}", ctx.var(name).unwrap_or("<unset>")));
        }
    }
}

impl Backend for FakeBackend {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn declares_rehearsal(&self) -> bool {
        self.rehearsal
    }

    fn rehearsal_wall(&self, _ctx: &BackendCtx, _host: &dyn Host) -> Option<Export> {
        self.wall.clone()
    }

    fn allowed_tasks(&self) -> Option<&'static [&'static str]> {
        self.allowed
    }

    fn check(&self, ctx: &BackendCtx, _host: &dyn Host) -> Result<(), String> {
        self.look("check", ctx);
        self.check.clone()
    }

    fn plan(
        &self,
        ctx: &BackendCtx,
        _host: &dyn Host,
        request: &RunRequest,
    ) -> Result<RunPlan, PlanRefusal> {
        self.look("plan", ctx);
        if let Some(note) = &self.refuse_plan {
            return Err(PlanRefusal { log_note: note.clone() });
        }
        let mut stage = StageSpec::new(NonEmpty::new("claude".to_string(), ["-p".to_string()]));
        stage.env =
            vec![("PLAN_VAR".to_string(), ctx.var("MAINTAINER_POST").unwrap_or("?").to_string())];
        stage.env.extend(self.stage_env.clone());
        stage.cwd.clone_from(&self.stage_cwd);
        stage.stdin = Source::File(request.prompt_file.clone());
        stage.stdout = Sink::AppendFile(request.log.clone());
        stage.stderr = Sink::SameAsStdout;
        Ok(RunPlan {
            log_notes: self.notes.clone(),
            exports: vec![Export { name: "BACKEND_EXPORT".to_string(), value: "1".to_string() }],
            pipeline: PipelineSpec {
                stages: NonEmpty::new(stage, []),
                timeout: Duration::from_hours(1),
            },
        })
    }
}

// ----- the fixture ------------------------------------------------------------------

struct Fx {
    root: PathBuf,
    home: PathBuf,
    state: PathBuf,
    tree: PathBuf,
    world: Arc<World>,
    env: BTreeMap<String, String>,
    backend: FakeBackend,
    lock_fails: bool,
    /// The search paths the process adapters were built with, in order.
    paths: Arc<Mutex<Vec<String>>>,
    /// The descriptor and the canonical path of the lock file the run opened.
    lock_fd: Arc<Mutex<Option<(i32, PathBuf)>>>,
}

struct Out {
    rc: i32,
    stdout: String,
    stderr: String,
}

fn real_lib(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../lib").join(name)
}

impl Fx {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("mx-run-life-{tag}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&root));
        let home = root.join("home");
        let state = home.join(".local/state/fx-maint");
        let tree = root.join("tree");
        for dir in [
            home.join(".local/bin"),
            state.join("checkout/.git"),
            tree.join("lib/backends"),
            tree.join("scripts"),
            tree.join("profiles/fx/prompts"),
            root.join("tmp"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let put = |path: PathBuf, text: &str| std::fs::write(path, text).unwrap();
        put(tree.join("lib/profile.sh"), "maintainer_scrub_secret() { :; }\n");
        std::fs::copy(real_lib("defaults.env"), tree.join("lib/defaults.env")).unwrap();
        std::fs::copy(real_lib("failure-shapes.json"), tree.join("lib/failure-shapes.json"))
            .unwrap();
        put(
            tree.join("lib/preamble-core.md"),
            "CORE doctrine for __MAINTAINER__ working on __SLUG__ (again __MAINTAINER__)\n",
        );
        put(tree.join("lib/prose-style.md"), "PROSE-STYLE rules\n");
        put(tree.join("VERSION"), "version=9.9.9\ncommit=abc1234\n");
        put(
            tree.join("profiles/fx/profile.env"),
            "PROFILE_NAME=\"fx\"\nREPO_PATH=\"${MAINTAINER_REPO_PATH:-$HOME/.local/state/fx-maint/checkout}\"\nREPO_ORIGIN=\"${MAINTAINER_REPO_ORIGIN:-}\"\nREPO_SLUG=\"fx/repo\"\nMAINTAINER_NAME=\"Fixture Maintainer\"\nGH_ACCOUNT=\"fxacct\"\nSTATE_DIR=\"${MAINTAINER_STATE_DIR:-$HOME/.local/state/fx-maint}\"\nHELPER=\"$HOME/.local/bin/maintainer\"\nTASKS=\"review issues\"\nMODEL_review=\"opus\"\nMODEL_issues=\"opus\"\nMIN_HOURS_review=6\nMIN_HOURS_issues=36\nPOST=\"${MAINTAINER_POST:-on}\"\nPROSE_STYLE=\"${MAINTAINER_PROSE_STYLE:-stop-slop}\"\nBACKEND=\"${MAINTAINER_BACKEND:-claude}\"\nCLAIM_LABEL=\"\"\nPROD_GLOBS=\"bin/* lib/*\"\n",
        );
        put(
            tree.join("profiles/fx/prompts/common-preamble.md"),
            "COMMON-PREAMBLE for fx\nsecond preamble line\n",
        );
        put(tree.join("profiles/fx/prompts/review.md"), "TASK review body\n");
        put(tree.join("profiles/fx/prompts/issues.md"), "TASK issues body\n");
        let env: BTreeMap<String, String> = [
            ("HOME", home.to_string_lossy().into_owned()),
            ("PATH", "/usr/bin:/bin".to_string()),
            ("LC_ALL", "C".to_string()),
            ("TZ", "UTC".to_string()),
            ("USER", "fixture-user".to_string()),
            ("TMPDIR", root.join("tmp").to_string_lossy().into_owned()),
            ("MAINTAINER_GH_BACKOFF", "0".to_string()),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect();
        Self {
            root,
            home,
            state,
            tree,
            world: World::new(),
            env,
            backend: FakeBackend {
                rehearsal: true,
                wall: None,
                allowed: None,
                check: Ok(()),
                refuse_plan: None,
                notes: Vec::new(),
                stage_env: Vec::new(),
                stage_cwd: None,
                seen: Arc::new(Mutex::new(Vec::new())),
            },
            lock_fails: false,
            paths: Arc::new(Mutex::new(Vec::new())),
            lock_fd: Arc::new(Mutex::new(None)),
        }
    }

    fn profile_add(&self, line: &str) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(self.tree.join("profiles/fx/profile.env"))
            .unwrap();
        writeln!(file, "{line}").unwrap();
    }

    fn set(&mut self, name: &str, value: &str) {
        self.env.insert(name.to_string(), value.to_string());
    }

    fn logs(&self) -> PathBuf {
        self.state.join("logs")
    }

    fn log(&self) -> PathBuf {
        self.logs().join(format!("{RUN_ID}.log"))
    }

    fn read_log(&self) -> String {
        std::fs::read_to_string(self.log())
            .unwrap_or_else(|e| panic!("no log {}: {e}", self.log().display()))
    }

    fn last_ran(&self, task: &str, seconds_ago: i64) {
        let dir = self.state.join("state");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("last-{task}.json"));
        std::fs::write(&file, "{}\n").unwrap();
        let when = std::time::UNIX_EPOCH
            + Duration::from_secs(u64::try_from(EPOCH - seconds_ago).unwrap());
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(when).unwrap();
    }

    fn leak(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("before {TOKEN} after\n")).unwrap();
    }

    fn run(&self, args: &[&str]) -> Out {
        let invocation = Invocation {
            argv0: self.tree.join("lib/run.sh").into_os_string(),
            exe: PathBuf::from("/nonexistent/run"),
            args: args.iter().map(|a| (*a).to_string()).collect(),
            cwd: self.root.clone(),
            env: Env::from_pairs(self.env.clone()),
        };
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let world = Arc::clone(&self.world);
        let backend = self.backend.clone();
        let paths = Arc::clone(&self.paths);
        let adapters = move |path: &std::ffi::OsStr, _locale: Option<String>| {
            paths.lock().unwrap().push(path.to_string_lossy().into_owned());
            Adapters { proc: world.clone(), pipe: world.clone() }
        };
        let pick = move |name: &str| -> Option<Box<dyn Backend>> {
            (name == "claude").then(|| Box::new(backend.clone()) as Box<dyn Backend>)
        };
        let fails = self.lock_fails;
        let lock_fd = Arc::clone(&self.lock_fd);
        let open_lock = move |path: &Path| -> Result<RunLock, LockOpenError> {
            if fails {
                return Err(LockOpenError::Open("Is a directory".to_string()));
            }
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)
                .map_err(|error| LockOpenError::Open(error.to_string()))?;
            *lock_fd.lock().unwrap() =
                Some((std::os::fd::AsRawFd::as_raw_fd(&file), path.canonicalize().unwrap()));
            Ok(RunLock::from_file(file))
        };
        let rc = {
            let mut services = Services {
                out: &mut out,
                err: &mut err,
                adapters: &adapters,
                backend: &pick,
                open_lock: &open_lock,
            };
            mx_run::run(&invocation, &mut services)
        };
        Out {
            rc,
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.world.calls().into_iter().map(|c| c.key).collect()
    }

    fn calls_of(&self, key: &str) -> Vec<Call> {
        self.world.calls().into_iter().filter(|c| c.key == key).collect()
    }

    fn helper_args(&self, verb: &str) -> Vec<Vec<String>> {
        self.calls_of(&format!("maintainer {verb}"))
            .into_iter()
            .map(|c| c.argv[1..].to_vec())
            .collect()
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.root));
    }
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_string()).collect()
}

// ----- a clean pass -----------------------------------------------------------------------

#[test]
fn a_clean_pass_makes_its_calls_in_the_order_of_the_bash_and_exits_zero() {
    let _turn = serial();
    let fx = Fx::new("clean");
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, "");
    assert_eq!(
        fx.keys(),
        [
            "date +%Y-%m-%dT%H-%M",
            "date -Is",
            "flock",
            "maintainer run-lease",
            "gh auth switch",
            "gh api user",
            "gh auth token",
            "gh api user",
            "maintainer start",
            "maintainer run-lease",
            "date -Is",
            "claude",
            "maintainer finish",
            "maintainer digest",
            "loginctl show-user",
            "loginctl show-session",
            "id -u",
            "notify-send",
            "date -Is",
            "maintainer run-lease",
        ]
    );
}

#[test]
fn the_helper_is_called_with_the_arguments_of_the_bash() {
    let _turn = serial();
    let fx = Fx::new("helper");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let pid = std::process::id().to_string();
    assert_eq!(
        fx.helper_args("run-lease"),
        vec![
            vec!["run-lease".to_string(), "review".to_string(), pid.clone()],
            vec!["run-lease".to_string(), "review".to_string(), pid, RUN_ID.to_string()],
            strings(&["run-lease", "--clear"]),
        ]
    );
    assert_eq!(fx.helper_args("start"), vec![strings(&["start", "review"])]);
    assert_eq!(fx.helper_args("finish"), vec![strings(&["finish", RUN_ID])]);
    assert_eq!(fx.helper_args("digest"), vec![strings(&["digest", RUN_ID, "--compact"])]);
}

#[test]
fn gh_is_called_with_the_arguments_of_the_bash() {
    let _turn = serial();
    let fx = Fx::new("gh");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let argv: Vec<Vec<String>> = fx
        .world
        .calls()
        .into_iter()
        .filter(|c| c.argv[0] == "gh")
        .map(|c| c.argv[1..].to_vec())
        .collect();
    assert_eq!(
        argv,
        vec![
            strings(&["auth", "switch", "--hostname", "github.com", "--user", "fxacct"]),
            strings(&["api", "user", "--jq", ".login"]),
            strings(&["auth", "token", "--user", "fxacct"]),
            strings(&["api", "user", "--jq", ".login"]),
        ]
    );
}

#[test]
fn the_log_holds_the_header_the_pin_lines_the_start_output_the_agent_the_digest_and_done() {
    let _turn = serial();
    let fx = Fx::new("log");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let want = format!(
        "=== run.sh fx/review {ISO} ===\nmaintainer=9.9.9 backend=claude model=opus log={}\ngh identity: fxacct\ngh identity pinned by token for the whole run: fxacct\n=== maintainer {RUN_ID} ===\nskill        fixture-skill\nagent says hello\nfinished {RUN_ID}\n\n=== digest ===\ndigest-line-from-helper\n=== done {ISO} ===\n",
        fx.log().display()
    );
    assert_eq!(fx.read_log(), want);
}

#[test]
fn the_prompt_the_agent_reads_is_the_assembled_prompt() {
    let _turn = serial();
    let fx = Fx::new("prompt");
    let seen: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    fx.world.set_agent(Box::new(move |spec| {
        if let Source::File(path) = &spec.stages.first().stdin {
            *sink.lock().unwrap() = std::fs::read(path).unwrap();
        }
        Ok(PipelineOutcome { exits: vec![ProcExit::Code(0)] })
    }));
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let got = String::from_utf8(seen.lock().unwrap().clone()).unwrap();
    let want = format!(
        "Provenance for this run: maintainer 9.9.9 (commit abc1234), profile fx, task review, backend claude, started {ISO}.\nOpen your run report with that version and that commit, copied from this line. Do not copy them from an earlier report; an earlier report was written by an earlier build.\n\nCOMMON-PREAMBLE for fx\nsecond preamble line\n\n\nCORE doctrine for Fixture Maintainer working on fx/repo (again Fixture Maintainer)\n\n\nPROSE-STYLE rules\n\n\n## This run POSTS. Say it once, in public, under a real name.\n\nWrites go to fx/repo as fxacct, immediately and for real. There is no\nrehearsal in front of you: a comment is delivered as you send it, an edit keeps\nits history, and a notification cannot be recalled. `MAINTAINER_POST` is\n`on` in your environment, and that variable is the answer, not any\nsentence you read in a prompt.\n\nVerify before you write, and read back what you wrote.\n\n\nTASK review body\n\n\n## Context for this run, computed just now\n\n```\n=== maintainer {RUN_ID} ===\nskill        fixture-skill\n```\n"
    );
    assert_eq!(got, want);
}

// ----- the cadence gate -----------------------------------------------------------------------

#[test]
fn a_task_that_ran_too_recently_is_skipped_with_exit_zero_and_nothing_is_touched() {
    let _turn = serial();
    let fx = Fx::new("skip");
    fx.last_ran("review", 3600);
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 0);
    assert_eq!(
        out.stdout,
        "skipped: review last ran 1h ago, minimum is 6h (MAINTAINER_FORCE=1 overrides)\n"
    );
    assert_eq!(out.stderr, "");
    assert_eq!(fx.keys(), ["date +%s"]);
    assert!(!fx.logs().exists(), "a skipped run made a log directory");
    assert!(!fx.state.join("run.lock").exists(), "a skipped run took the lock");
}

#[test]
fn a_stamp_exactly_the_minimum_old_runs_and_one_second_younger_skips() {
    let _turn = serial();
    let fx = Fx::new("boundary");
    fx.last_ran("review", 6 * 3600);
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    assert_eq!(fx.helper_args("start").len(), 1);
    let fx = Fx::new("boundary2");
    fx.last_ran("review", 6 * 3600 - 1);
    let out = fx.run(&["fx", "review"]);
    assert_eq!(
        out.stdout,
        "skipped: review last ran 5h ago, minimum is 6h (MAINTAINER_FORCE=1 overrides)\n"
    );
    assert!(fx.helper_args("start").is_empty());
}

#[test]
fn force_set_to_one_overrides_the_gate_and_the_helper_never_sees_it() {
    let _turn = serial();
    let mut fx = Fx::new("force");
    fx.last_ran("review", 60);
    fx.set("MAINTAINER_FORCE", "1");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    for call in fx.world.calls() {
        assert!(!call.env.contains_key("MAINTAINER_FORCE"), "{} saw MAINTAINER_FORCE", call.key);
    }
}

#[test]
fn force_set_to_anything_but_one_does_not_force() {
    let _turn = serial();
    for value in ["true", "0", "yes", ""] {
        let mut fx = Fx::new("force-other");
        fx.last_ran("review", 60);
        fx.set("MAINTAINER_FORCE", value);
        let out = fx.run(&["fx", "review"]);
        assert!(out.stdout.starts_with("skipped: review"), "{value}: {}", out.stdout);
    }
}

#[test]
fn the_gate_is_read_again_after_the_lock_and_a_stamp_promoted_meanwhile_skips() {
    let _turn = serial();
    let fx = Fx::new("second-gate");
    fx.last_ran("review", 72_000);
    // The run ahead of this one promotes the stamp while this one waits for flock.
    let stamp = fx.state.join("state/last-review.json");
    fx.world.on_call(
        "flock",
        Box::new(move || {
            let now = std::time::UNIX_EPOCH + Duration::from_secs(u64::try_from(EPOCH).unwrap());
            std::fs::File::options().write(true).open(&stamp).unwrap().set_modified(now).unwrap();
        }),
    );
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 0, "{}", out.stderr);
    assert_eq!(
        out.stdout,
        "skipped: review last ran 0h ago, minimum is 6h, after waiting for the lock (MAINTAINER_FORCE=1 overrides)\n"
    );
    assert_eq!(out.stderr, "");
    assert!(fx.helper_args("start").is_empty());
    assert!(fx.calls_of("gh api user").is_empty());
    assert!(fx.state.join("run.lock").exists(), "the lock was opened before the second gate");
}

// Plan decision D11: a minimum that is not a number refuses where the bash fell open.
#[test]
fn a_minimum_that_is_not_a_number_refuses_with_exit_64_plan_decision_d11() {
    let _turn = serial();
    let fx = Fx::new("d11");
    fx.profile_add("MIN_HOURS_review=soon");
    fx.last_ran("review", 3600);
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 64);
    assert_eq!(out.stdout, "");
    assert!(
        out.stderr.contains("MIN_HOURS_review='soon' is not a whole number of hours"),
        "{}",
        out.stderr
    );
    assert!(fx.helper_args("start").is_empty());
}

// Plan decision D12: a stamp from the future does not skip the run.
#[test]
fn a_stamp_from_the_future_does_not_skip_the_run_plan_decision_d12() {
    let _turn = serial();
    let fx = Fx::new("d12");
    fx.last_ran("review", -36_000);
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "");
    assert_eq!(fx.helper_args("start").len(), 1);
}

#[test]
fn a_preview_does_not_consult_the_gate_or_the_clock() {
    let _turn = serial();
    let fx = Fx::new("preview-gate");
    fx.last_ran("review", 60);
    let out = fx.run(&["--show-prompt", "fx", "review"]);
    assert_eq!(out.rc, 0, "{}", out.stderr);
    assert!(!fx.keys().contains(&"date +%s".to_string()));
}

// ----- the identity ------------------------------------------------------------------------------

#[test]
fn a_different_account_exits_1_on_the_first_answer_with_no_retry_and_no_token_call() {
    let _turn = serial();
    let fx = Fx::new("wrong");
    fx.world.fix("gh api user", reply(0, "someone-else\n"));
    let out = fx.run(&["fx", "review"]);
    let msg = format!(
        "gh is authenticated as 'someone-else', not fxacct; refusing to run. See {}/{STAMP}-review.log",
        fx.logs().display()
    );
    assert_eq!(out.rc, 1);
    assert_eq!(out.stderr, format!("{msg}\n"));
    assert_eq!(fx.calls_of("gh api user").len(), 1);
    assert!(fx.calls_of("gh auth token").is_empty());
    assert!(fx.helper_args("start").is_empty());
    let failed = fx.helper_args("failed");
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0][..3], strings(&["failed", "review", &msg])[..]);
}

#[test]
fn the_login_is_compared_without_regard_to_case_and_logged_as_github_spelled_it() {
    let _turn = serial();
    let fx = Fx::new("case");
    fx.world.fix("gh api user", reply(0, "FXAcct\n"));
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    assert!(fx.read_log().contains("gh identity: FXAcct\n"));
}

#[test]
fn an_http_401_exits_1_at_once_with_the_relogin_text_and_the_error_body_in_the_log() {
    let _turn = serial();
    let fx = Fx::new("401");
    fx.world.fix(
        "gh api user",
        Reply {
            rc: 1,
            out: "{\"message\": \"Bad credentials\"}\n".to_string(),
            err: "gh: Bad credentials (HTTP 401)\n".to_string(),
        },
    );
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert!(
        out.stderr
            .starts_with("GitHub rejected the stored token for fxacct (HTTP 401), so nothing ran."),
        "{}",
        out.stderr
    );
    assert_eq!(fx.calls_of("gh api user").len(), 1);
    let log = std::fs::read_to_string(fx.logs().join(format!("{STAMP}-review.log"))).unwrap();
    assert!(
        log.contains("gh: Bad credentials (HTTP 401)\n{\"message\": \"Bad credentials\"}\n"),
        "{log}"
    );
}

#[test]
fn an_outage_is_tried_three_times_sleeping_backoff_times_the_attempt_and_exits_75() {
    let _turn = serial();
    let mut fx = Fx::new("outage");
    fx.set("MAINTAINER_GH_BACKOFF", "7");
    fx.world.fix("gh api user", reply(1, ""));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 75);
    assert!(out.stderr.starts_with("gh could not reach GitHub in 3 tries"), "{}", out.stderr);
    assert_eq!(fx.calls_of("gh api user").len(), 3);
    let sleeps: Vec<String> = fx.calls_of("sleep").into_iter().map(|c| c.argv[1].clone()).collect();
    assert_eq!(sleeps, ["7", "14"]);
    let log = std::fs::read_to_string(fx.logs().join(format!("{STAMP}-review.log"))).unwrap();
    assert!(log.contains("gh api user gave no answer (try 1 of 3); retrying\n"));
    assert!(log.contains("gh api user gave no answer (try 2 of 3); retrying\n"));
    assert!(!log.contains("try 3 of 3"));
}

#[test]
fn a_zero_backoff_never_calls_sleep() {
    let _turn = serial();
    let fx = Fx::new("nosleep");
    fx.world.fix("gh api user", reply(1, ""));
    assert_eq!(fx.run(&["fx", "review"]).rc, 75);
    assert!(fx.calls_of("sleep").is_empty());
}

#[test]
fn a_second_answer_that_names_the_account_is_accepted() {
    let _turn = serial();
    let fx = Fx::new("retry-ok");
    fx.world.queue("gh api user", vec![reply(1, ""), reply(0, "fxacct\n"), reply(0, "fxacct\n")]);
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    assert_eq!(fx.calls_of("gh api user").len(), 3);
}

#[test]
fn a_tries_setting_that_is_not_a_number_is_refused_plan_decision_p2a_x1() {
    let _turn = serial();
    let mut fx = Fx::new("tries");
    fx.set("MAINTAINER_GH_TRIES", "many");
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 64);
    assert!(out.stderr.contains("GH_TRIES='many' is not a whole number"), "{}", out.stderr);
    // It refuses next to the cadence minimum, before the lock, the lease and gh.
    let keys = fx.keys();
    assert!(
        !keys.iter().any(|k| k == "flock" || k.starts_with("maintainer") || k.starts_with("gh")),
        "{keys:?}"
    );
}

#[test]
fn a_token_that_cannot_be_pinned_exits_1_and_the_run_never_starts() {
    let _turn = serial();
    let fx = Fx::new("notoken");
    fx.world.fix("gh auth token", reply(0, ""));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert!(out.stderr.starts_with("could not pin a token for fxacct"), "{}", out.stderr);
    assert!(fx.helper_args("start").is_empty());
}

#[test]
fn a_pinned_token_that_resolves_to_another_account_exits_1_and_scrubs() {
    let _turn = serial();
    let fx = Fx::new("pinned-wrong");
    fx.world.queue("gh api user", vec![reply(0, "fxacct\n"), reply(0, "someone-else\n")]);
    Fx::leak(&fx.state.join("leak.txt"));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert!(
        out.stderr.starts_with("the pinned token resolves to 'someone-else', not fxacct"),
        "{}",
        out.stderr
    );
    let leaked = std::fs::read_to_string(fx.state.join("leak.txt")).unwrap();
    assert_eq!(leaked, format!("before {MASK} after\n"));
}

#[test]
fn a_pinned_token_that_gh_cannot_resolve_exits_75_and_names_the_gh_status() {
    let _turn = serial();
    let fx = Fx::new("pinned-error");
    fx.world.queue("gh api user", vec![reply(0, "fxacct\n"), reply(3, "")]);
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 75);
    assert!(out.stderr.contains("(gh exited 3)"), "{}", out.stderr);
}

// Plan decision D13: a GH_TOKEN inherited from the caller never reaches a call before the pin.
#[test]
fn an_inherited_gh_token_reaches_no_call_plan_decision_d13() {
    let _turn = serial();
    let mut fx = Fx::new("d13");
    fx.set("GH_TOKEN", "caller-token-value");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let calls = fx.world.calls();
    let pin_at = calls.iter().position(|c| c.key == "gh auth token").unwrap();
    for call in &calls[..=pin_at] {
        assert!(!call.env.contains_key("GH_TOKEN"), "{} saw an inherited GH_TOKEN", call.key);
    }
    for call in &calls[pin_at + 1..] {
        if call.key == "date -Is" || call.key.starts_with("date") {
            continue;
        }
        assert_eq!(call.env.get("GH_TOKEN").map(String::as_str), Some(TOKEN), "{}", call.key);
    }
}

#[test]
fn gh_token_is_pinned_for_the_helper_and_the_agent_and_github_token_is_gone() {
    let _turn = serial();
    let mut fx = Fx::new("pin-env");
    fx.set("GITHUB_TOKEN", "other-token");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    // Decision D22: an inherited GITHUB_TOKEN reaches no call before the pin either.
    let calls = fx.world.calls();
    let pin_at = calls.iter().position(|c| c.key == "gh auth token").unwrap();
    for call in &calls[..=pin_at] {
        assert!(
            !call.env.contains_key("GITHUB_TOKEN"),
            "{} saw an inherited GITHUB_TOKEN",
            call.key
        );
    }
    for key in ["maintainer start", "claude", "maintainer finish"] {
        let call = fx.calls_of(key).remove(0);
        assert_eq!(call.env.get("GH_TOKEN").map(String::as_str), Some(TOKEN), "{key}");
        assert!(!call.env.contains_key("GITHUB_TOKEN"), "{key} saw GITHUB_TOKEN");
    }
}

// Plan decision D15: a token the scrub cannot remove stops the run.
#[test]
fn a_pinned_token_under_16_characters_refuses_the_run_plan_decision_d15() {
    let _turn = serial();
    let fx = Fx::new("d15");
    fx.world.fix("gh auth token", reply(0, "shorttoken\n"));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert!(out.stderr.contains("shorter than 16 characters"), "{}", out.stderr);
    assert!(!out.stderr.contains("shorttoken"), "the alert printed the token");
    assert!(fx.helper_args("start").is_empty());
    assert!(fx.calls_of("claude").is_empty());
}

// ----- the exit scrub ------------------------------------------------------------------------------------

#[test]
fn on_success_the_token_is_removed_from_state_files_and_the_session_directory_and_the_log_says_so()
{
    let _turn = serial();
    let fx = Fx::new("scrub");
    Fx::leak(&fx.state.join("leak.txt"));
    let session = fx
        .home
        .join(".claude/projects")
        .join(fx.state.join("checkout").to_string_lossy().replace(['/', '.'], "-"));
    Fx::leak(&session.join("session.jsonl"));
    let elsewhere = fx.home.join("elsewhere/leak.txt");
    Fx::leak(&elsewhere);
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let masked = format!("before {MASK} after\n");
    assert_eq!(std::fs::read_to_string(fx.state.join("leak.txt")).unwrap(), masked);
    assert_eq!(std::fs::read_to_string(session.join("session.jsonl")).unwrap(), masked);
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), format!("before {TOKEN} after\n"));
    let log = fx.read_log();
    assert!(
        log.contains(&format!(
            "scrubbed the pinned token from {}",
            fx.state.join("leak.txt").display()
        )),
        "{log}"
    );
}

#[test]
fn a_panic_in_the_pass_still_scrubs_the_token_and_clears_the_lease() {
    let _turn = serial();
    let fx = Fx::new("panic");
    Fx::leak(&fx.state.join("leak.txt"));
    fx.world.set_agent(Box::new(|spec| {
        if let Sink::AppendFile(log) = &spec.stages.first().stdout {
            append(log, &format!("the agent printed {TOKEN}\n"));
        }
        panic!("the agent blew up");
    }));
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.run(&["fx", "review"])));
    assert!(outcome.is_err(), "the panic did not propagate");
    assert_eq!(
        std::fs::read_to_string(fx.state.join("leak.txt")).unwrap(),
        format!("before {MASK} after\n")
    );
    let log = std::fs::read_to_string(fx.logs().join(format!("{RUN_ID}.log"))).unwrap();
    assert!(!log.contains(TOKEN), "the token survives in the log");
    assert_eq!(fx.helper_args("run-lease").last().unwrap(), &strings(&["run-lease", "--clear"]));
    assert!(fx.helper_args("finish").is_empty(), "finish ran after the panic");
}

#[test]
fn a_refusal_before_the_token_is_pinned_scrubs_nothing() {
    let _turn = serial();
    let fx = Fx::new("noscrub");
    fx.world.fix("gh api user", reply(0, "someone-else\n"));
    Fx::leak(&fx.state.join("leak.txt"));
    assert_eq!(fx.run(&["fx", "review"]).rc, 1);
    assert_eq!(
        std::fs::read_to_string(fx.state.join("leak.txt")).unwrap(),
        format!("before {TOKEN} after\n")
    );
}

#[test]
fn the_exit_trap_keeps_the_status_the_run_was_exiting_with() {
    let _turn = serial();
    let fx = Fx::new("trap-status");
    fx.world.fix("maintainer finish", reply(1, ""));
    assert_eq!(fx.run(&["fx", "review"]).rc, 1);
    assert_eq!(fx.calls_of("maintainer run-lease").len(), 3, "the lease was cleared once");
}

#[test]
fn a_binary_file_holding_the_token_is_left_alone_and_reported() {
    let _turn = serial();
    let fx = Fx::new("binary");
    std::fs::write(fx.state.join("disk.img"), format!("bin\0{TOKEN}\0")).unwrap();
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    assert_eq!(
        std::fs::read(fx.state.join("disk.img")).unwrap(),
        format!("bin\0{TOKEN}\0").into_bytes()
    );
    assert!(fx.read_log().contains("disk.img  BINARY, not modified: rotate the secret"));
}

// ----- the lease and the lock --------------------------------------------------------------------------------

#[test]
fn the_lease_is_written_before_the_identity_is_asked_and_cleared_once_on_an_early_refusal() {
    let _turn = serial();
    let fx = Fx::new("lease-order");
    fx.world.fix("gh api user", reply(0, "someone-else\n"));
    fx.run(&["fx", "review"]);
    let keys = fx.keys();
    let lease = keys.iter().position(|k| k == "maintainer run-lease").unwrap();
    let first_gh = keys.iter().position(|k| k == "gh auth switch").unwrap();
    assert!(lease < first_gh, "{keys:?}");
    assert_eq!(fx.calls_of("maintainer run-lease").len(), 2);
    assert_eq!(fx.helper_args("run-lease").last().unwrap(), &strings(&["run-lease", "--clear"]));
}

#[test]
fn a_failing_lease_write_is_logged_twice_and_never_stops_the_run() {
    let _turn = serial();
    let fx = Fx::new("lease-fail");
    fx.world.fix("maintainer run-lease", reply(1, ""));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 0);
    let log = fx.read_log();
    assert!(
        log.contains("run-lease: could not write leases/run.json; the run goes on without it\n")
    );
    assert!(log.contains("run-lease: could not add the run id to leases/run.json\n"));
}

#[test]
fn a_held_lock_exits_75_with_a_low_urgency_toast_and_a_recorded_marker() {
    let _turn = serial();
    let fx = Fx::new("held");
    fx.world.fix("flock", reply(1, ""));
    let out = fx.run(&["fx", "review"]);
    let msg = "another run held the lock for over 3600s, so this one never started";
    assert_eq!(out.rc, 75);
    assert_eq!(out.stderr, format!("{msg}\n"));
    assert_eq!(fx.calls_of("flock")[0].argv, strings(&["flock", "-w", "3600", "9"]));
    let toast = fx.calls_of("notify-send").remove(0);
    assert_eq!(toast.argv[1..5], strings(&["-u", "low", "-a", "maintainer"])[..]);
    assert_eq!(toast.argv[5], "maintainer · fx review · will retry");
    assert!(toast.argv[6].starts_with("Another run held the shared tree for over 3600s.\nfix  maintainer status, then wait or kill it\nlog  "));
    assert!(fx.calls_of("gh api user").is_empty());
}

#[test]
fn flock_failures_other_than_contention_exit_1() {
    let _turn = serial();
    let fx = Fx::new("flock-other");
    fx.world.fix("flock", reply(2, ""));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert!(
        out.stderr.contains(
            "flock failed to acquire the run lock (exit 2); check flock and the run.lock file"
        ),
        "{}",
        out.stderr
    );
    let fx = Fx::new("flock-64");
    fx.world.fix("flock", reply(64, ""));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert!(out.stderr.contains("flock rejected MAINTAINER_LOCK_WAIT=3600"), "{}", out.stderr);
}

#[test]
fn lock_wait_from_the_environment_is_passed_to_flock() {
    let _turn = serial();
    let mut fx = Fx::new("lock-wait");
    fx.set("MAINTAINER_LOCK_WAIT", "5");
    fx.world.fix("flock", reply(1, ""));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.stderr, "another run held the lock for over 5s, so this one never started\n");
    assert_eq!(fx.calls_of("flock")[0].argv, strings(&["flock", "-w", "5", "9"]));
}

// Plan decision D10: a lock file that cannot be opened is its own refusal.
#[test]
fn a_lock_file_that_cannot_be_opened_is_its_own_refusal_plan_decision_d10() {
    let _turn = serial();
    let mut fx = Fx::new("d10");
    fx.lock_fails = true;
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert!(out.stderr.contains("could not open the run lock"), "{}", out.stderr);
    assert!(out.stderr.contains("Is a directory"), "{}", out.stderr);
    assert!(!out.stderr.contains("held the lock"), "{}", out.stderr);
    assert!(fx.calls_of("flock").is_empty(), "flock was asked for a lock that never opened");
}

#[test]
fn a_preview_takes_no_lock_asks_nothing_of_gh_and_leaves_no_file() {
    let _turn = serial();
    let fx = Fx::new("preview");
    let out = fx.run(&["--show-prompt", "fx", "review"]);
    assert_eq!(out.rc, 0, "{}", out.stderr);
    assert_eq!(out.stderr, "");
    assert!(out.stdout.starts_with("Provenance for this run: maintainer 9.9.9 (commit abc1234), profile fx, task review, backend claude, started"));
    assert!(
        out.stdout.ends_with(
            "(preview only: no refresh ran, so nothing here reflects the tracker)\n```\n"
        )
    );
    assert!(!fx.state.join("run.lock").exists());
    assert!(!fx.logs().exists());
    let keys = fx.keys();
    assert!(
        !keys.iter().any(|k| k.starts_with("gh") || k.starts_with("maintainer") || k == "flock"),
        "{keys:?}"
    );
    let left: Vec<_> = std::fs::read_dir(fx.root.join("tmp")).unwrap().collect();
    assert!(left.is_empty(), "a preview left {} files in TMPDIR", left.len());
}

// ----- the run id, the log ----------------------------------------------------------------------------------------

#[test]
fn a_run_id_different_from_the_start_time_name_renames_the_log_and_keeps_its_lines() {
    let _turn = serial();
    let fx = Fx::new("rename");
    fx.world.fix(
        "maintainer start",
        reply(0, "=== maintainer 2026-02-02T02-02-review ===\nskill        x\n"),
    );
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let new = fx.logs().join("2026-02-02T02-02-review.log");
    assert!(!fx.logs().join(format!("{STAMP}-review.log")).exists());
    let log = std::fs::read_to_string(&new).unwrap();
    assert!(log.starts_with(&format!("=== run.sh fx/review {ISO} ===\n")), "{log}");
    assert!(log.contains("log renamed to match run id 2026-02-02T02-02-review\n"));
    assert_eq!(
        fx.helper_args("run-lease")[1],
        vec![
            "run-lease".to_string(),
            "review".to_string(),
            std::process::id().to_string(),
            "2026-02-02T02-02-review".to_string()
        ]
    );
}

#[test]
fn an_existing_log_under_the_run_id_is_never_replaced_the_lines_are_appended_to_it() {
    let _turn = serial();
    let fx = Fx::new("fold");
    std::fs::create_dir_all(fx.logs()).unwrap();
    std::fs::write(fx.logs().join("2026-02-02T02-02-review.log"), "an earlier run wrote this\n")
        .unwrap();
    fx.world.fix("maintainer start", reply(0, "=== maintainer 2026-02-02T02-02-review ===\n"));
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let log = std::fs::read_to_string(fx.logs().join("2026-02-02T02-02-review.log")).unwrap();
    assert!(log.starts_with("an earlier run wrote this\n=== run.sh fx/review"), "{log}");
    assert!(!fx.logs().join(format!("{STAMP}-review.log")).exists());
}

#[test]
fn start_output_with_no_header_line_exits_1_naming_the_helper() {
    let _turn = serial();
    let fx = Fx::new("noid");
    fx.world.fix("maintainer start", reply(0, "no header here\n"));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert!(
        out.stderr.starts_with(&format!(
            "could not parse a run id from '{}/.local/bin/maintainer start'",
            fx.home.display()
        )),
        "{}",
        out.stderr
    );
    assert!(fx.calls_of("claude").is_empty());
}

#[test]
fn a_failing_start_exits_1_and_keeps_its_output_in_the_log() {
    let _turn = serial();
    let fx = Fx::new("start-fails");
    fx.world.fix(
        "maintainer start",
        Reply {
            rc: 1,
            out: "refresh exploded\n".to_string(),
            err: "and a stderr line\n".to_string(),
        },
    );
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert!(out.stderr.starts_with("refresh failed, no agent run started. See "), "{}", out.stderr);
    let log = std::fs::read_to_string(fx.logs().join(format!("{STAMP}-review.log"))).unwrap();
    assert!(log.contains("refresh exploded\nand a stderr line\n"), "{log}");
}

// ----- failures of the agent and of finish ------------------------------------------------------------------------------

#[test]
fn a_failed_agent_whose_report_finish_accepts_alerts_once_and_still_exits_zero() {
    let _turn = serial();
    let fx = Fx::new("agent-fails");
    fx.world.set_agent(Box::new(|spec| {
        if let Sink::AppendFile(log) = &spec.stages.first().stdout {
            append(log, "something unknown broke\n");
        }
        Ok(PipelineOutcome { exits: vec![ProcExit::Code(1)] })
    }));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 0);
    assert_eq!(
        out.stderr,
        format!("claude exited non-zero for {RUN_ID}. See {}\n", fx.log().display())
    );
    assert_eq!(fx.helper_args("finish").len(), 1);
}

#[test]
fn a_session_limit_stop_is_transient_at_low_urgency_and_a_finished_report_keeps_exit_zero() {
    let _turn = serial();
    let fx = Fx::new("transient");
    fx.world.set_agent(Box::new(|spec| {
        if let Sink::AppendFile(log) = &spec.stages.first().stdout {
            append(log, "You've hit your session limit, resets 10:10pm\n");
        }
        Ok(PipelineOutcome { exits: vec![ProcExit::Code(1)] })
    }));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 0);
    assert_eq!(
        out.stderr,
        "the claude backend stopped on something that clears itself (You've hit your session limit, resets 10:10pm); the next slot retries this task\n"
    );
    assert_eq!(fx.calls_of("notify-send").remove(0).argv[2], "low");
}

#[test]
fn a_transient_stop_with_no_report_aborts_with_the_class_and_exits_75() {
    let _turn = serial();
    let fx = Fx::new("transient-no-report");
    fx.world.set_agent(Box::new(|spec| {
        if let Sink::AppendFile(log) = &spec.stages.first().stdout {
            append(log, "You've hit your session limit, resets 10:10pm\n");
        }
        Ok(PipelineOutcome { exits: vec![ProcExit::Code(1)] })
    }));
    fx.world.fix("maintainer finish", reply(1, ""));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 75);
    let why = "You've hit your session limit, resets 10:10pm";
    assert!(
        out.stderr
            .contains(&format!("{RUN_ID} wrote no report: the claude backend stopped on {why}")),
        "{}",
        out.stderr
    );
    let abort = fx.helper_args("abort").remove(0);
    assert_eq!(abort[..2], strings(&["abort", RUN_ID])[..]);
    assert_eq!(
        abort[2],
        format!("no report; transient/session-limit: {why}); the next slot retries this task")
    );
}

#[test]
fn a_cause_only_the_owner_can_clear_is_critical_and_names_the_login_fix() {
    let _turn = serial();
    let fx = Fx::new("human");
    fx.world.set_agent(Box::new(|spec| {
        if let Sink::AppendFile(log) = &spec.stages.first().stdout {
            append(log, "Invalid API key. Fix external API key\n");
        }
        Ok(PipelineOutcome { exits: vec![ProcExit::Code(1)] })
    }));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 0);
    let msg = format!(
        "the claude backend stopped on something only you can clear (Invalid API key. Fix external API key), and every slot after this one fails the same way until it is. See {}",
        fx.log().display()
    );
    assert_eq!(out.stderr, format!("{msg}\n"));
    let toast = fx.calls_of("notify-send").remove(0);
    assert_eq!(toast.argv[2], "critical");
    assert!(
        toast.argv[6].contains("fix  claude /login, then: maintainer-doctor"),
        "{}",
        toast.argv[6]
    );
}

#[test]
fn a_failed_agent_and_no_report_give_one_alert_and_one_plain_line_and_exit_1() {
    let _turn = serial();
    let fx = Fx::new("both");
    fx.world.set_agent(Box::new(|spec| {
        if let Sink::AppendFile(log) = &spec.stages.first().stdout {
            append(log, "something unknown broke\n");
        }
        Ok(PipelineOutcome { exits: vec![ProcExit::Code(1)] })
    }));
    fx.world.fix("maintainer finish", reply(1, ""));
    let out = fx.run(&["fx", "review"]);
    let first = format!("claude exited non-zero for {RUN_ID}. See {}", fx.log().display());
    let second = format!(
        "{RUN_ID} produced no report, and the claude failure above is why. See {}",
        fx.log().display()
    );
    assert_eq!(out.rc, 1);
    assert_eq!(out.stderr, format!("{first}\n{second}\n"));
    assert_eq!(fx.helper_args("failed").len(), 1);
    assert_eq!(fx.calls_of("notify-send").len(), 1);
}

#[test]
fn finish_refusing_an_empty_report_aborts_with_the_classified_cause_and_exits_1() {
    let _turn = serial();
    let fx = Fx::new("no-report");
    fx.world.fix("maintainer finish", reply(1, "error: report is empty\n"));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert_eq!(
        out.stderr,
        format!(
            "{RUN_ID} produced no report; the run is not auditable. See {}\n",
            fx.log().display()
        )
    );
    assert_eq!(fx.helper_args("abort")[0][2], "no report; unclassified: error: report is empty");
    assert!(fx.helper_args("digest").is_empty());
    assert!(!fx.read_log().contains("=== done"));
}

#[test]
fn a_report_finish_calls_a_fixture_is_aborted_with_that_reason_and_alerted_as_refused() {
    let _turn = serial();
    let fx = Fx::new("fixture");
    fx.world.fix("maintainer finish", reply(1, "this is a fixture rather than a report\n"));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert_eq!(
        out.stderr,
        format!(
            "{RUN_ID} wrote a report that finish refused (fixture rather than a report). Read runs/{RUN_ID}.md and {}\n",
            fx.log().display()
        )
    );
    assert_eq!(
        fx.helper_args("abort")[0][2],
        format!("report refused by finish: fixture rather than a report (see runs/{RUN_ID}.md)")
    );
}

#[test]
fn a_backend_that_cannot_make_a_plan_leaves_its_note_in_the_log_and_the_run_goes_to_finish() {
    let _turn = serial();
    let mut fx = Fx::new("plan-refused");
    fx.backend.refuse_plan = Some("prompt too large".to_string());
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 0);
    assert!(fx.read_log().contains("prompt too large\n"));
    assert!(fx.calls_of("claude").is_empty());
    assert!(out.stderr.starts_with("claude exited non-zero for "), "{}", out.stderr);
}

#[test]
fn the_agent_runs_in_the_checkout_with_the_plan_environment_over_the_run_environment() {
    let _turn = serial();
    let fx = Fx::new("agent-env");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let agent = fx.calls_of("claude").remove(0);
    assert_eq!(agent.cwd, Some(fx.state.join("checkout")));
    assert_eq!(agent.env.get("PLAN_VAR").map(String::as_str), Some("on"));
    assert_eq!(agent.env.get("BACKEND_EXPORT").map(String::as_str), Some("1"));
    assert_eq!(agent.env.get("MAINTAINER_IN_RUN").map(String::as_str), Some("fx/review"));
    assert_eq!(agent.env.get("MAINTAINER_POST").map(String::as_str), Some("on"));
    let finish = fx.calls_of("maintainer finish").remove(0);
    assert_eq!(finish.cwd, Some(fx.state.join("checkout")));
    assert_eq!(finish.env.get("BACKEND_EXPORT").map(String::as_str), Some("1"));
    assert_eq!(
        agent.stdin.as_ref().map(|p| p.exists()),
        Some(false),
        "the prompt file outlived the run"
    );
}

// ----- the digest and the alert hook ---------------------------------------------------------------------------------------------

#[test]
fn the_digest_becomes_one_normal_toast_titled_with_profile_and_task() {
    let _turn = serial();
    let fx = Fx::new("digest");
    fx.world.fix("maintainer digest", reply(0, "line one\nline two\n"));
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let toast = fx.calls_of("notify-send").remove(0);
    assert_eq!(
        toast.argv[1..6],
        strings(&["-u", "normal", "-a", "maintainer", "maintainer · fx review"])[..]
    );
    assert_eq!(toast.argv[6], "line one\nline two");
    assert_eq!(toast.env.get("DISPLAY").map(String::as_str), Some(":0"));
    assert_eq!(
        toast.env.get("DBUS_SESSION_BUS_ADDRESS").map(String::as_str),
        Some("unix:path=/run/user/1000/bus")
    );
}

#[test]
fn an_empty_digest_sends_no_toast_and_a_failing_digest_is_dropped() {
    let _turn = serial();
    let fx = Fx::new("digest-empty");
    fx.world.fix("maintainer digest", reply(0, ""));
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    assert!(fx.calls_of("notify-send").is_empty());
    assert!(!fx.read_log().contains("=== digest ==="));
    let fx = Fx::new("digest-fails");
    fx.world.fix("maintainer digest", reply(1, "half a digest\n"));
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    assert!(fx.calls_of("notify-send").is_empty());
    assert!(!fx.read_log().contains("half a digest"));
}

#[test]
fn notification_off_sends_nothing_and_does_not_ask_loginctl() {
    let _turn = serial();
    let mut fx = Fx::new("notify-off");
    fx.set("MAINTAINER_NOTIFY", "off");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    assert!(fx.calls_of("notify-send").is_empty());
    assert!(fx.calls_of("loginctl show-user").is_empty());
}

#[test]
fn the_display_comes_from_loginctl_then_the_caller_then_zero() {
    let _turn = serial();
    let mut fx = Fx::new("display");
    fx.set("DISPLAY", ":7");
    fx.world.fix("loginctl show-session", reply(0, ":9\n"));
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    assert_eq!(
        fx.calls_of("notify-send").remove(0).env.get("DISPLAY").map(String::as_str),
        Some(":9")
    );
    let mut fx = Fx::new("display-env");
    fx.set("DISPLAY", ":7");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    assert_eq!(
        fx.calls_of("notify-send").remove(0).env.get("DISPLAY").map(String::as_str),
        Some(":7")
    );
}

#[test]
fn with_user_unset_the_user_name_comes_from_id() {
    let _turn = serial();
    let mut fx = Fx::new("no-user");
    fx.env.remove("USER");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    assert_eq!(fx.calls_of("loginctl show-user").remove(0).argv[2], "fixture-user");
}

#[test]
fn the_alert_hook_runs_under_sh_with_the_task_the_log_the_program_and_the_message() {
    let _turn = serial();
    let mut fx = Fx::new("hook");
    fx.set("MAINTAINER_ALERT_CMD", "echo hook");
    fx.world.fix("gh api user", reply(0, "someone-else\n"));
    fx.world.fix("sh", reply(0, "hook-output\n"));
    fx.run(&["fx", "review"]);
    let hook = fx.calls_of("sh").remove(0);
    assert_eq!(hook.argv[..3], strings(&["sh", "-c", "echo hook"])[..]);
    assert_eq!(hook.argv[3], fx.tree.join("lib/run.sh").to_string_lossy());
    assert!(hook.argv[4].starts_with("gh is authenticated as 'someone-else'"));
    assert_eq!(hook.env.get("MAINTAINER_ALERT_TASK").map(String::as_str), Some("fx/review"));
    assert!(hook.env.get("MAINTAINER_ALERT_LOG").is_some_and(|log| log.ends_with("-review.log")));
    let log = std::fs::read_to_string(fx.logs().join(format!("{STAMP}-review.log"))).unwrap();
    assert!(log.contains("hook-output\n"));
}

#[test]
fn a_failing_alert_hook_is_noted_in_the_log_and_never_changes_the_exit_code() {
    let _turn = serial();
    let mut fx = Fx::new("hook-fails");
    fx.set("MAINTAINER_ALERT_CMD", "exit 3");
    fx.world.fix("gh api user", reply(0, "someone-else\n"));
    fx.world.fix("sh", reply(3, ""));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    let log = std::fs::read_to_string(fx.logs().join(format!("{STAMP}-review.log"))).unwrap();
    assert!(
        log.contains("run.sh: MAINTAINER_ALERT_CMD failed; the marker and the log still stand\n")
    );
}

#[test]
fn a_transient_failure_does_not_call_the_alert_hook() {
    let _turn = serial();
    let mut fx = Fx::new("hook-transient");
    fx.set("MAINTAINER_ALERT_CMD", "echo hook");
    fx.world.fix("flock", reply(1, ""));
    assert_eq!(fx.run(&["fx", "review"]).rc, 75);
    assert!(fx.calls_of("sh").is_empty());
}

#[test]
fn a_helper_that_cannot_record_the_marker_adds_one_stderr_line_and_the_exit_code_stays() {
    let _turn = serial();
    let fx = Fx::new("marker");
    fx.world.fix("gh api user", reply(0, "someone-else\n"));
    fx.world.fix("maintainer failed", reply(1, "marker stdout\n"));
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert!(
        out.stderr.ends_with("run.sh: could not even record the failure marker\n"),
        "{}",
        out.stderr
    );
    assert!(!out.stderr.contains("marker stdout"));
}

#[test]
fn a_preview_refusal_records_no_marker_and_sends_no_toast() {
    let _turn = serial();
    let fx = Fx::new("preview-refusal");
    std::fs::remove_file(fx.tree.join("profiles/fx/prompts/review.md")).unwrap();
    let out = fx.run(&["--show-prompt", "fx", "review"]);
    assert_eq!(out.rc, 1);
    assert!(
        out.stderr.starts_with("required prompt input is missing or unreadable: "),
        "{}",
        out.stderr
    );
    assert_eq!(out.stdout, "");
    assert!(fx.helper_args("failed").is_empty());
    assert!(fx.calls_of("notify-send").is_empty());
}

// ----- the backend checks and the rehearsal --------------------------------------------------------------------------------------------

#[test]
fn a_backend_that_is_unusable_exits_1_with_its_message_and_the_install_fix_in_the_toast() {
    let _turn = serial();
    let mut fx = Fx::new("unusable");
    fx.backend.check = Err("claude not on PATH".to_string());
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert_eq!(out.stderr, format!("backend claude unusable: claude not on PATH\n"));
    let toast = fx.calls_of("notify-send").remove(0);
    assert!(toast.argv[6].starts_with("backend claude unusable: claude not on PATH\nfix  install it, then: maintainer-doctor\nlog  "), "{}", toast.argv[6]);
    assert!(!fx.state.join("run.lock").exists(), "the lock was taken before the backend check");
}

#[test]
fn post_off_on_a_backend_that_cannot_rehearse_exits_78() {
    let _turn = serial();
    let mut fx = Fx::new("no-rehearsal");
    fx.backend.rehearsal = false;
    fx.set("MAINTAINER_POST", "off");
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 78);
    assert_eq!(
        out.stderr,
        "backend 'claude' cannot enforce POST=off (no per-command deny list); use claude or opencode\n"
    );
}

#[test]
fn post_off_with_no_wall_rendered_exits_78_with_two_lines_and_no_marker() {
    let _turn = serial();
    let mut fx = Fx::new("no-wall");
    fx.set("MAINTAINER_POST", "off");
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 78);
    assert_eq!(
        out.stderr,
        "run.sh: POST=off but backend 'claude' has no rehearsal wall rendered for 'fx'\n        re-run ./install.sh, which renders it beside the live wall\n"
    );
    assert!(fx.helper_args("failed").is_empty());
    assert!(fx.calls_of("notify-send").is_empty());
    assert!(fx.logs().exists(), "the logs directory is made before the wall is checked");
}

#[test]
fn post_off_exports_the_wall_the_backend_names_and_the_prompt_says_rehearsal() {
    let _turn = serial();
    let mut fx = Fx::new("wall");
    fx.backend.wall = Some(Export {
        name: "MAINTAINER_SETTINGS".to_string(),
        value: "/the/wall.json".to_string(),
    });
    fx.set("MAINTAINER_POST", "off");
    let seen: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&seen);
    fx.world.set_agent(Box::new(move |spec| {
        if let Source::File(path) = &spec.stages.first().stdin {
            *sink.lock().unwrap() = std::fs::read_to_string(path).unwrap();
        }
        Ok(PipelineOutcome { exits: vec![ProcExit::Code(0)] })
    }));
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let agent = fx.calls_of("claude").remove(0);
    assert_eq!(agent.env.get("MAINTAINER_SETTINGS").map(String::as_str), Some("/the/wall.json"));
    assert_eq!(agent.env.get("MAINTAINER_POST").map(String::as_str), Some("off"));
    assert!(seen.lock().unwrap().contains("## This run is a REHEARSAL. Post nothing."));
    assert_eq!(fx.helper_args("start").len(), 1);
}

#[test]
fn a_preview_with_post_off_and_no_wall_says_so_and_goes_on() {
    let _turn = serial();
    let mut fx = Fx::new("preview-wall");
    fx.set("MAINTAINER_POST", "off");
    let out = fx.run(&["--show-prompt", "fx", "review"]);
    assert_eq!(out.rc, 0);
    assert_eq!(
        out.stderr,
        "run.sh: (preview) no rehearsal wall is rendered in this tree; a real\n        run would refuse here. ./install.sh renders it.\n"
    );
    assert!(out.stdout.contains("## This run is a REHEARSAL. Post nothing."));
}

#[test]
fn a_task_the_backend_may_not_run_exits_78() {
    let _turn = serial();
    let mut fx = Fx::new("allowed");
    fx.backend.allowed = Some(&["readonly-review"]);
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 78);
    assert_eq!(out.stderr, "backend 'claude' may only run: readonly-review (refused 'review')\n");
}

// Plan decision D18: a backend name with a slash or a dot-dot is refused.
#[test]
fn a_backend_name_with_a_slash_or_a_dot_dot_is_refused_plan_decision_d18() {
    let _turn = serial();
    for name in ["../backends/codex", "a/b", "..", "x..y"] {
        let mut fx = Fx::new("d18");
        fx.set("MAINTAINER_BACKEND", name);
        let out = fx.run(&["fx", "review"]);
        assert_eq!(out.rc, 64, "{name}");
        assert!(out.stderr.contains(&format!("no backend '{name}'")), "{name}: {}", out.stderr);
        assert!(!out.stderr.contains("No such file or directory"), "{name}: {}", out.stderr);
        assert!(fx.calls_of("claude").is_empty());
    }
}

#[test]
fn an_unknown_backend_name_exits_64_with_the_shells_source_error_and_the_alert() {
    let _turn = serial();
    let mut fx = Fx::new("nosuch");
    fx.set("MAINTAINER_BACKEND", "nosuch");
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 64);
    assert!(out.stderr.contains("nosuch.sh: No such file or directory"), "{}", out.stderr);
    assert!(out.stderr.contains("no backend 'nosuch'"), "{}", out.stderr);
    assert_eq!(fx.helper_args("failed").len(), 1);
}

#[test]
fn a_preview_of_an_unknown_backend_records_nothing() {
    let _turn = serial();
    let mut fx = Fx::new("nosuch-preview");
    fx.set("MAINTAINER_BACKEND", "nosuch");
    let out = fx.run(&["--show-prompt", "fx", "review"]);
    assert_eq!(out.rc, 64);
    assert!(fx.helper_args("failed").is_empty());
    assert!(fx.calls_of("notify-send").is_empty());
}

// ----- the arguments, the nested-run guard, the prompt inputs ----------------------------------------------------------------------------

#[test]
fn a_missing_argument_is_the_shells_usage_message_and_exit_1() {
    let _turn = serial();
    let fx = Fx::new("usage");
    let script = fx.tree.join("lib/run.sh").display().to_string();
    let out = fx.run(&[]);
    assert_eq!(out.rc, 1);
    assert_eq!(
        out.stderr,
        format!("{script}: line 58: 1: usage: run.sh [--show-prompt] <profile> <task>\n")
    );
    let out = fx.run(&["fx"]);
    assert_eq!(out.rc, 1);
    assert_eq!(
        out.stderr,
        format!("{script}: line 59: 2: usage: run.sh [--show-prompt] <profile> <task>\n")
    );
    let out = fx.run(&["--show-prompt"]);
    assert_eq!(out.rc, 1);
    assert!(out.stderr.contains("line 58: 1: usage"), "{}", out.stderr);
    let out = fx.run(&["", "review"]);
    assert_eq!(out.rc, 1);
    assert!(fx.keys().is_empty(), "{:?}", fx.keys());
}

#[test]
fn show_prompt_after_the_profile_is_read_as_the_task_name() {
    let _turn = serial();
    let fx = Fx::new("show-late");
    let out = fx.run(&["fx", "--show-prompt"]);
    assert_eq!(out.rc, 64);
    assert_eq!(out.stderr, "run.sh: unknown task '--show-prompt'\n");
}

#[test]
fn an_unknown_profile_and_an_unknown_task_exit_64_and_touch_nothing() {
    let _turn = serial();
    let fx = Fx::new("unknown");
    let out = fx.run(&["nope", "review"]);
    assert_eq!((out.rc, out.stderr.as_str()), (64, "run.sh: no profile 'nope'\n"));
    let out = fx.run(&["fx", "bogus"]);
    assert_eq!((out.rc, out.stderr.as_str()), (64, "run.sh: unknown task 'bogus'\n"));
    let out = fx.run(&["fx", "rev"]);
    assert_eq!(out.rc, 64);
    assert!(fx.keys().is_empty());
    assert!(!fx.logs().exists());
}

#[test]
fn starting_a_run_inside_a_run_exits_78_and_a_preview_inside_a_run_is_allowed() {
    let _turn = serial();
    let mut fx = Fx::new("nested");
    fx.set("MAINTAINER_IN_RUN", "other/task");
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 78);
    assert_eq!(
        out.stderr,
        "run.sh: already inside the run 'other/task'; refusing to start 'fx/review' from within it\n"
    );
    assert!(fx.keys().is_empty());
    let out = fx.run(&["nope", "review"]);
    assert_eq!(out.rc, 78, "the refusal comes before the profile check");
    assert_eq!(fx.run(&["--show-prompt", "fx", "review"]).rc, 0);
    fx.set("MAINTAINER_IN_RUN", "");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0, "an empty marker is not a marker");
}

#[test]
fn a_missing_task_file_exits_1_before_any_prompt_is_printed_and_a_missing_doctrine_after_start() {
    let _turn = serial();
    let fx = Fx::new("missing");
    std::fs::remove_file(fx.tree.join("lib/preamble-core.md")).unwrap();
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert_eq!(out.stderr, "preamble-core.md is missing; refusing to run without the doctrine\n");
    assert_eq!(fx.helper_args("start").len(), 1, "the doctrine is checked after start");
    assert!(fx.calls_of("claude").is_empty());
    assert!(fx.helper_args("finish").is_empty());
}

#[test]
fn the_maintainer_name_reaches_the_doctrine_literally_including_ampersand_and_pipe() {
    let _turn = serial();
    for name in ["A&B", "A|B", "A\\B"] {
        let fx = Fx::new("name");
        fx.profile_add(&format!("MAINTAINER_NAME='{name}'"));
        let out = fx.run(&["--show-prompt", "fx", "review"]);
        assert_eq!(out.rc, 0, "{name}: {}", out.stderr);
        assert!(
            out.stdout
                .contains(&format!("CORE doctrine for {name} working on fx/repo (again {name})")),
            "{name}: {}",
            out.stdout
        );
    }
}

// Plan decision D16 is preserved: the slug is not escaped, so `&` in it means the match.
#[test]
fn an_ampersand_in_the_slug_corrupts_the_doctrine_as_it_does_in_the_bash_plan_decision_d16() {
    let _turn = serial();
    let fx = Fx::new("d16");
    fx.profile_add("REPO_SLUG='fx/a&b'");
    let out = fx.run(&["--show-prompt", "fx", "review"]);
    assert_eq!(out.rc, 0, "{}", out.stderr);
    assert!(out.stdout.contains("working on fx/a__SLUG__b (again"), "{}", out.stdout);
}

#[test]
fn prose_style_raw_leaves_the_prose_out_and_any_other_value_keeps_it() {
    let _turn = serial();
    let fx = Fx::new("prose");
    fx.profile_add("PROSE_STYLE=raw");
    assert!(!fx.run(&["--show-prompt", "fx", "review"]).stdout.contains("PROSE-STYLE"));
    let fx = Fx::new("prose2");
    fx.profile_add("PROSE_STYLE=other");
    assert!(fx.run(&["--show-prompt", "fx", "review"]).stdout.contains("PROSE-STYLE rules"));
    // Only the exact word raw switches the prose off.
    let fx = Fx::new("prose-upper");
    fx.profile_add("PROSE_STYLE=RAW");
    assert!(fx.run(&["--show-prompt", "fx", "review"]).stdout.contains("PROSE-STYLE rules"));
    let fx = Fx::new("prose3");
    std::fs::remove_file(fx.tree.join("lib/prose-style.md")).unwrap();
    let out = fx.run(&["--show-prompt", "fx", "review"]);
    assert_eq!(out.rc, 0);
    assert!(!out.stdout.contains("PROSE-STYLE"));
}

#[test]
fn a_task_name_that_is_not_a_shell_variable_name_dies_on_the_model_lookup_decision_f7() {
    let _turn = serial();
    let fx = Fx::new("hyphen");
    fx.profile_add("TASKS=\"review readonly-review\"");
    let script = fx.tree.join("lib/run.sh").display().to_string();
    let out = fx.run(&["fx", "readonly-review"]);
    assert_eq!(out.rc, 1);
    assert_eq!(
        out.stderr,
        format!("{script}: line 106: MODEL_readonly-review: invalid variable name\n")
    );
    let fx = Fx::new("nomodel");
    fx.profile_add("unset MODEL_review");
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert!(
        out.stderr.ends_with(": line 106: !model_var: no model configured for review\n"),
        "{}",
        out.stderr
    );
}

#[test]
fn the_helper_sees_the_exports_of_the_bash() {
    let _turn = serial();
    let mut fx = Fx::new("exports");
    fx.profile_add("SKILL_review=\"my-skill\"");
    fx.set("MAINTAINER_FORCE", "1");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let start = fx.calls_of("maintainer start").remove(0);
    let want = [
        ("MAINTAINER_PROFILE", "fx"),
        ("MAINTAINER_TASK", "review"),
        ("MAINTAINER_TASKS", "review issues"),
        ("MAINTAINER_SLUG", "fx/repo"),
        ("MAINTAINER_ACCOUNT", "fxacct"),
        ("MAINTAINER_POST", "on"),
        ("MAINTAINER_IN_RUN", "fx/review"),
        ("MAINTAINER_VERSION", "9.9.9"),
        ("MAINTAINER_COMMIT", "abc1234"),
        ("MAINTAINER_SKILL", "my-skill"),
        ("PROD_GLOBS", "bin/* lib/*"),
        ("CLAIM_LABEL", ""),
    ];
    for (name, value) in want {
        assert_eq!(start.env.get(name).map(String::as_str), Some(value), "{name}");
    }
    assert_eq!(start.env.get("MAINTAINER_STATE"), Some(&fx.state.to_string_lossy().into_owned()));
    assert_eq!(
        start.env.get("MAINTAINER_REPO"),
        Some(&fx.state.join("checkout").to_string_lossy().into_owned())
    );
    assert_eq!(
        start.env.get("MAINTAINER_SCRIPTS"),
        Some(&fx.tree.join("scripts").to_string_lossy().into_owned())
    );
    assert_eq!(
        start.env.get("PROFILE_DIR"),
        Some(&fx.tree.join("profiles/fx").to_string_lossy().into_owned())
    );
    assert_eq!(
        start.env.get("REPO_PATH"),
        Some(&fx.state.join("checkout").to_string_lossy().into_owned())
    );
}

#[test]
fn a_missing_checkout_is_cloned_from_the_origin_and_a_failing_clone_exits_1() {
    let _turn = serial();
    let mut fx = Fx::new("clone");
    std::fs::remove_dir_all(fx.state.join("checkout")).unwrap();
    fx.set("MAINTAINER_REPO_ORIGIN", "https://example.invalid/fx.git");
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1, "{}", out.stderr);
    let clone = fx.calls_of("git").remove(0);
    assert_eq!(
        clone.argv,
        vec![
            "git".to_string(),
            "clone".to_string(),
            "--quiet".to_string(),
            "https://example.invalid/fx.git".to_string(),
            fx.state.join("checkout").to_string_lossy().into_owned(),
        ]
    );
    let fx = Fx::new("clone-fails");
    std::fs::remove_dir_all(fx.state.join("checkout")).unwrap();
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 1);
    assert!(
        out.stderr
            .contains("is not a git checkout and the profile sets no REPO_ORIGIN to clone from"),
        "{}",
        out.stderr
    );
    assert!(fx.calls_of("git").is_empty());
}

#[test]
fn the_exit_trap_runs_in_the_checkout_with_what_the_backend_exported_after_the_trap_was_set() {
    let _turn = serial();
    let fx = Fx::new("trap-env");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let leases = fx.calls_of("maintainer run-lease");
    let clear = leases.last().unwrap();
    assert_eq!(clear.argv[1..], strings(&["run-lease", "--clear"])[..]);
    assert_eq!(clear.cwd, Some(fx.state.join("checkout")));
    assert_eq!(clear.env.get("BACKEND_EXPORT").map(String::as_str), Some("1"));
    assert_eq!(clear.env.get("GH_TOKEN").map(String::as_str), Some(TOKEN));
    assert!(!clear.env.contains_key("MAINTAINER_FORCE"));
}

#[test]
fn a_preview_with_post_off_on_a_backend_that_cannot_rehearse_is_not_refused() {
    let _turn = serial();
    let mut fx = Fx::new("preview-no-rehearsal");
    fx.backend.rehearsal = false;
    fx.set("MAINTAINER_POST", "off");
    let out = fx.run(&["--show-prompt", "fx", "review"]);
    assert_eq!(out.rc, 0, "{}", out.stderr);
    assert!(
        out.stderr.starts_with("run.sh: (preview) no rehearsal wall is rendered in this tree"),
        "{}",
        out.stderr
    );
    assert!(out.stdout.contains("## This run is a REHEARSAL. Post nothing."));
}

#[test]
fn a_minimum_with_a_trailing_blank_makes_printf_complain_as_it_does_in_the_bash() {
    let _turn = serial();
    let fx = Fx::new("printf");
    fx.profile_add("MIN_HOURS_review=\" 6 \"");
    fx.last_ran("review", 3600);
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 0);
    assert_eq!(
        out.stdout,
        "skipped: review last ran 1h ago, minimum is 6h (MAINTAINER_FORCE=1 overrides)\n"
    );
    assert_eq!(
        out.stderr,
        format!(
            "{}: line 142: printf:  6 : invalid number\n",
            fx.tree.join("lib/run.sh").display()
        )
    );
}

// ----- what the review of the backends asked of the executor -------------------------------------------

#[test]
fn the_agent_inherits_the_whole_run_environment_name_by_name() {
    let _turn = serial();
    let mut fx = Fx::new("agent-inherits");
    let inherited = [
        ("XDG_DATA_HOME", "/xdg/data"),
        ("XDG_CONFIG_HOME", "/xdg/config"),
        ("HTTPS_PROXY", "http://proxy.invalid:3128"),
        ("NO_PROXY", "localhost"),
        ("ANTHROPIC_API_KEY", "sk-ant-fixture"),
        ("CODEX_HOME", "/codex"),
        ("CURSOR_API_KEY", "cursor-fixture"),
        ("SSH_AUTH_SOCK", "/run/user/1000/agent"),
        ("LANG", "en_US.UTF-8"),
        ("DISPLAY", ":3"),
    ];
    for (name, value) in inherited {
        fx.set(name, value);
    }
    fx.set("GH_TOKEN", "caller-token-value");
    fx.set("GITHUB_TOKEN", "caller-github-token");
    fx.set("MAINTAINER_FORCE", "1");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let agent = fx.calls_of("claude").remove(0);
    for (name, value) in inherited {
        assert_eq!(
            agent.env.get(name).map(String::as_str),
            Some(value),
            "{name} did not reach the agent"
        );
    }
    for name in ["HOME", "USER", "TZ", "TMPDIR", "MAINTAINER_GH_BACKOFF"] {
        assert_eq!(agent.env.get(name), fx.env.get(name), "{name}");
    }
    assert_eq!(
        agent.env.get("GH_TOKEN").map(String::as_str),
        Some(TOKEN),
        "GH_TOKEN is the pinned token"
    );
    assert!(!agent.env.contains_key("GITHUB_TOKEN"), "GITHUB_TOKEN is unset after the pin");
    assert!(!agent.env.contains_key("MAINTAINER_FORCE"), "the override is consumed");
    for name in [
        "PROFILE_DIR",
        "REPO_PATH",
        "MAINTAINER_REPO",
        "MAINTAINER_SLUG",
        "MAINTAINER_STATE",
        "MAINTAINER_ACCOUNT",
        "PROD_GLOBS",
        "CLAIM_LABEL",
        "MAINTAINER_PROFILE",
        "MAINTAINER_TASK",
        "MAINTAINER_TASKS",
        "MAINTAINER_SKILL",
        "MAINTAINER_IN_RUN",
        "MAINTAINER_POST",
        "MAINTAINER_VERSION",
        "MAINTAINER_COMMIT",
        "MAINTAINER_SCRIPTS",
    ] {
        assert!(agent.env.contains_key(name), "{name} did not reach the agent");
    }
    // PATH and LC_ALL belong to the process adapter, which gives the child the search path and the locale.
    assert!(!agent.env.contains_key("PATH") && !agent.env.contains_key("LC_ALL"));
}

#[test]
fn the_wall_a_backend_returns_is_in_the_variable_set_before_check_and_plan_and_reaches_every_stage()
{
    let _turn = serial();
    for name in ["MAINTAINER_SETTINGS", "MAINTAINER_CURSOR_DIR"] {
        let mut fx = Fx::new("wall-before-check");
        fx.backend.wall = Some(Export { name: name.to_string(), value: "/the/wall".to_string() });
        fx.set("MAINTAINER_POST", "off");
        assert_eq!(fx.run(&["fx", "review"]).rc, 0);
        let seen = fx.backend.seen.lock().unwrap().clone();
        let check_at = seen.iter().position(|line| *line == format!("check: {name}=/the/wall"));
        let plan_at = seen.iter().position(|line| *line == format!("plan: {name}=/the/wall"));
        assert!(check_at.is_some() && check_at < plan_at, "{seen:?}");
        assert!(seen.contains(&format!("plan: {name}=/the/wall")), "{seen:?}");
        assert_eq!(
            fx.calls_of("claude").remove(0).env.get(name).map(String::as_str),
            Some("/the/wall")
        );
        assert_eq!(
            fx.calls_of("maintainer finish").remove(0).env.get(name).map(String::as_str),
            Some("/the/wall")
        );
    }
}

#[test]
fn a_real_run_with_posting_on_exports_no_wall_variable() {
    let _turn = serial();
    let mut fx = Fx::new("no-wall-on");
    fx.backend.wall =
        Some(Export { name: "MAINTAINER_SETTINGS".to_string(), value: "/the/wall".to_string() });
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    assert_eq!(fx.backend.seen.lock().unwrap()[0], "check: MAINTAINER_SETTINGS=<unset>");
    assert!(!fx.calls_of("claude").remove(0).env.contains_key("MAINTAINER_SETTINGS"));
}

#[test]
fn the_log_notes_of_a_plan_are_appended_one_per_line_before_the_pipeline_starts() {
    let _turn = serial();
    let mut fx = Fx::new("log-notes");
    fx.backend.notes = vec!["first note".to_string(), "second note".to_string()];
    let before = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&before);
    fx.world.set_agent(Box::new(move |spec| {
        if let Sink::AppendFile(log) = &spec.stages.first().stdout {
            *sink.lock().unwrap() = std::fs::read_to_string(log).unwrap();
        }
        Ok(PipelineOutcome { exits: vec![ProcExit::Code(0)] })
    }));
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    assert!(
        before.lock().unwrap().ends_with("first note\nsecond note\n"),
        "{}",
        before.lock().unwrap()
    );
}

#[test]
fn a_stage_starts_in_the_working_directory_the_plan_names() {
    let _turn = serial();
    let mut fx = Fx::new("stage-cwd");
    fx.backend.stage_cwd = Some(fx.root.join("tmp"));
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    assert_eq!(fx.calls_of("claude").remove(0).cwd, Some(fx.root.join("tmp")));
}

#[test]
fn a_key_a_plan_puts_in_its_stage_environment_does_not_reach_finish_the_digest_or_the_lease() {
    let _turn = serial();
    let mut fx = Fx::new("stage-key");
    fx.backend.stage_env = vec![("GROQ_API_KEY".to_string(), "gsk_stage_only_secret".to_string())];
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 0);
    assert_eq!(
        fx.calls_of("claude").remove(0).env.get("GROQ_API_KEY").map(String::as_str),
        Some("gsk_stage_only_secret")
    );
    for call in fx.world.calls() {
        if call.key != "claude" {
            assert!(!call.env.contains_key("GROQ_API_KEY"), "{} saw the key", call.key);
        }
    }
    assert!(
        !out.stderr.contains("gsk_stage_only_secret")
            && !fx.read_log().contains("gsk_stage_only_secret")
    );
}

#[test]
fn the_search_path_gets_the_local_bin_then_the_cargo_bin_then_the_callers_path() {
    let _turn = serial();
    let fx = Fx::new("path-prefix");
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let home = fx.home.display();
    let paths = fx.paths.lock().unwrap().clone();
    assert_eq!(paths.first().map(String::as_str), Some("/usr/bin:/bin"), "{paths:?}");
    assert_eq!(
        paths.last().map(String::as_str),
        Some(format!("{home}/.local/bin:{home}/.cargo/bin:/usr/bin:/bin").as_str()),
        "{paths:?}"
    );
}

#[test]
fn a_stamp_that_is_a_directory_is_not_a_stamp_and_the_run_goes_ahead() {
    let _turn = serial();
    let fx = Fx::new("stamp-dir");
    let stamp = fx.state.join("state/last-review.json");
    std::fs::create_dir_all(&stamp).unwrap();
    // Dated an hour before the fixture clock, so that a directory read as a stamp would skip.
    let touch = ProcSpec::command(
        "touch",
        ["-d", &format!("@{}", EPOCH - 3600), stamp.to_str().unwrap()],
        Duration::from_secs(10),
    );
    assert_eq!(
        RealProc::new("/usr/bin:/bin", Vec::new()).run(&touch).unwrap().exit,
        ProcExit::Code(0)
    );
    let out = fx.run(&["fx", "review"]);
    assert_eq!(out.rc, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "");
    assert_eq!(fx.helper_args("start").len(), 1);
}

/// Records, at every call of the helper's `run-lease`, whether descriptor `fd` still
/// names the lock file. The last call is the one that clears the lease.
fn watch_the_lock(fx: &Fx) -> Arc<Mutex<Vec<bool>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (record, lock) = (Arc::clone(&seen), Arc::clone(&fx.lock_fd));
    fx.world.on_call(
        "maintainer run-lease",
        Box::new(move || {
            if let Some((fd, path)) = lock.lock().unwrap().clone() {
                let open = std::fs::read_link(format!("/proc/self/fd/{fd}")).ok() == Some(path);
                record.lock().unwrap().push(open);
            }
        }),
    );
    seen
}

#[test]
fn the_run_lock_is_held_until_the_lease_is_cleared_on_a_normal_return() {
    let _turn = serial();
    let fx = Fx::new("lock-held-normal");
    let seen = watch_the_lock(&fx);
    assert_eq!(fx.run(&["fx", "review"]).rc, 0);
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(seen.iter().all(|open| *open), "the lock was closed before a lease call: {seen:?}");
}

#[test]
fn the_run_lock_is_held_until_the_scrub_has_finished_when_the_pass_panics() {
    let _turn = serial();
    let fx = Fx::new("lock-held-panic");
    let seen = watch_the_lock(&fx);
    fx.world.set_agent(Box::new(|_| panic!("the agent blew up")));
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.run(&["fx", "review"])));
    assert!(outcome.is_err());
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(
        seen.iter().all(|open| *open),
        "the lock was released before the lease was cleared: {seen:?}"
    );
}

#[test]
fn the_exit_trap_still_clears_the_lease_after_the_pipe_port_has_closed_to_new_stages() {
    let _turn = serial();
    let fx = Fx::new("trap-after-stop");
    let world = Arc::clone(&fx.world);
    fx.world.set_agent(Box::new(move |_| {
        // What the signal thread does first: no stage starts from here on. Then the pass dies.
        world.pipe_closed.store(true, std::sync::atomic::Ordering::SeqCst);
        panic!("the pass ends here");
    }));
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.run(&["fx", "review"])));
    assert!(outcome.is_err());
    let leases = fx.calls_of("maintainer run-lease");
    let clear = leases.last().unwrap();
    assert_eq!(
        clear.argv[1..],
        strings(&["run-lease", "--clear"])[..],
        "the lease was not cleared"
    );
    assert_eq!(clear.via, "proc", "the exit trap called the helper through the pipe port");
}
