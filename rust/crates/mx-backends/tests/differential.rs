//! Differential proof: the four backends against `lib/backends/*.sh`.
//!
//! For each backend and each scenario the bash function runs in a child bash
//! (`tests/differential/driver.sh` sources the backend file, as `lib/run.sh`
//! does), and the Rust backend answers the same four operations (`declare`,
//! `wall`, `check`, `run`). The Rust plan is executed by a small stand-in for
//! the orchestrator: it renders the `RunPlan` as a bash pipeline, runs it through
//! `mx_ports::RealProc`, and so starts the same stubs the bash run starts.
//!
//! The stubs (`tests/differential/stub.sh`, installed as `claude`, `codex`,
//! `cursor-agent`, `opencode` and `python3`) log argv, the working directory, the
//! backend-relevant environment names, and the length and sha256 of standard
//! input, in call order. Both sides must agree on every printed line, the exit
//! status, standard output and standard error, and on the bytes of every file the
//! run left in the scenario's tree (the log, the commands file, the call log).
//!
//! Scenarios are written out by hand for each question and then generated from a
//! seed (`DIFF_SEED`, default fixed), so a run repeats exactly. A difference that
//! cannot be removed is a decision, named in `Divergence` and asserted on both
//! sides, never normalised away.
//!
//! Environment: `MX_TREE` (the tree whose `lib/backends` is tested; default this
//! checkout), `DIFF_SEED`.

use mx_backends::{Backend, BackendCtx, Export, RealHost, RunPlan, RunRequest, backend_by_name};
use mx_ports::{Proc, ProcExit, ProcOutput, ProcSpec, RealProc, Sink, Source, StageSpec};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Fewer scenarios than this for a backend means the generator did not run.
const FLOOR: usize = 40;
const DEFAULT_SEED: u64 = 20_261_120;
const RANDOM_PER_BACKEND: usize = 48;
const SYSTEM_TOOLS: [&str; 8] = ["bash", "cat", "sha256sum", "wc", "rm", "tr", "env", "sort"];
const OPENCODE_DEFAULT_MODEL: &str = "groq/openai/gpt-oss-120b";
/// What the run exports for every child before a backend is asked: the wall
/// variables the backends name, and the key the run inherited. The executor passes
/// them; a plan names none of them (decisions P2B-7 and P2B-10).
const RUN_EXPORTED: [&str; 3] = ["MAINTAINER_SETTINGS", "MAINTAINER_CURSOR_DIR", "GROQ_API_KEY"];

// --- the scenario --------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scripts {
    /// No `scripts/` directory: `MAINTAINER_SCRIPTS` is unset.
    NoDir,
    /// `MAINTAINER_SCRIPTS` is set and empty.
    Empty,
    /// `scripts/transcript.py` exists.
    WithFilter,
    /// `scripts/` exists and holds no `transcript.py`.
    WithoutFilter,
}

/// A difference between the bash and the port that stays, asserted on both sides.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Divergence {
    /// P2B-1: `${OPENCODE_MODEL:?}` ends the bash process; the port refuses.
    ModelUnset,
    /// P2B-3: the filter's own output goes to the run's streams in the bash and to
    /// the log in the port.
    FilterNoise,
    /// P2B-5: a key file that cannot be read makes the shell print an error that
    /// the port does not.
    UnreadableKey,
    /// P2B-6: a task name that is not a shell identifier ends the bash backend
    /// (`${!THINKING_<task>}`); the port looks the name up and finds nothing.
    /// `lib/run.sh` dies earlier on the same name (plan decision F7).
    TaskNotAName,
    /// P2B-2 and P2B-5: a NUL byte in the cursor prompt or in the opencode key file
    /// makes the shell warn on standard error (`ignored null byte in input`); the
    /// port drops the byte as the shell does and says nothing.
    NulWarning,
    /// P2B-12: a checkout path that is not UTF-8 is an argument of the codex plan,
    /// so the port refuses where the bash runs. Every other backend only runs in
    /// that directory, and the differential shows they agree with the bash.
    NonUtf8Path,
}

#[derive(Clone, Debug)]
struct Sc {
    label: String,
    backend: &'static str,
    task: String,
    model: String,
    prompt: Vec<u8>,
    profile_name: String,
    repo_name: String,
    /// A checkout directory name that is not UTF-8, in place of `repo_name`.
    repo_raw: Option<Vec<u8>>,
    prompt_symlink: bool,
    home_name: String,
    log_rel: String,
    prior_log: Vec<u8>,
    profile_files: BTreeSet<String>,
    scripts: Scripts,
    programs: BTreeSet<&'static str>,
    local_bin: BTreeSet<&'static str>,
    cargo_bin: BTreeSet<&'static str>,
    vars: BTreeMap<String, String>,
    key_file: Option<Vec<u8>>,
    key_unreadable: bool,
    auth_json: bool,
    rcs: BTreeMap<&'static str, i32>,
    noise: BTreeMap<&'static str, (String, String)>,
    divergence: Option<Divergence>,
}

fn program_of(backend: &str) -> &'static str {
    match backend {
        "claude" => "claude",
        "codex" => "codex",
        "cursor" => "cursor-agent",
        "opencode" => "opencode",
        other => panic!("no program for backend {other}"),
    }
}

fn live_file_of(backend: &str) -> Option<&'static str> {
    match backend {
        "claude" => Some("settings.json"),
        "cursor" => Some("cursor/cli-config.json"),
        "opencode" => Some("opencode.json"),
        _ => None,
    }
}

fn xs(count: usize) -> Vec<u8> {
    vec![b'x'; count]
}

impl Sc {
    fn new(backend: &'static str, label: &str) -> Self {
        let mut programs = BTreeSet::from([program_of(backend)]);
        if backend == "claude" {
            programs.insert("python3");
        }
        let mut vars = BTreeMap::new();
        if backend == "opencode" {
            vars.insert("OPENCODE_MODEL".to_string(), OPENCODE_DEFAULT_MODEL.to_string());
            vars.insert("GROQ_API_KEY".to_string(), "env-key-value".to_string());
        }
        Self {
            label: format!("{backend}: {label}"),
            backend,
            task: "review".to_string(),
            model: "opus".to_string(),
            prompt: b"the prompt text\nsecond line\n".to_vec(),
            profile_name: "profile".to_string(),
            repo_name: "repo".to_string(),
            repo_raw: None,
            prompt_symlink: false,
            home_name: "home".to_string(),
            log_rel: "logs/run.log".to_string(),
            prior_log: b"PRIOR\n".to_vec(),
            profile_files: live_file_of(backend).map(str::to_string).into_iter().collect(),
            scripts: Scripts::WithFilter,
            programs,
            local_bin: BTreeSet::new(),
            cargo_bin: BTreeSet::new(),
            vars,
            key_file: None,
            key_unreadable: false,
            auth_json: false,
            rcs: BTreeMap::new(),
            noise: BTreeMap::new(),
            divergence: None,
        }
    }

    fn model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }
    fn task(mut self, task: &str) -> Self {
        self.task = task.to_string();
        self
    }
    fn prompt(mut self, prompt: impl Into<Vec<u8>>) -> Self {
        self.prompt = prompt.into();
        self
    }
    fn profile(mut self, name: &str) -> Self {
        self.profile_name = name.to_string();
        self
    }
    fn repo(mut self, name: &str) -> Self {
        self.repo_name = name.to_string();
        self
    }
    fn repo_raw(mut self, bytes: &[u8]) -> Self {
        self.repo_raw = Some(bytes.to_vec());
        self
    }
    fn prompt_symlink(mut self) -> Self {
        self.prompt_symlink = true;
        self
    }
    fn home(mut self, name: &str) -> Self {
        self.home_name = name.to_string();
        self
    }
    fn log(mut self, rel: &str) -> Self {
        self.log_rel = rel.to_string();
        self
    }
    fn prior(mut self, bytes: &[u8]) -> Self {
        self.prior_log = bytes.to_vec();
        self
    }
    fn var(mut self, name: &str, value: &str) -> Self {
        self.vars.insert(name.to_string(), value.to_string());
        self
    }
    fn no_var(mut self, name: &str) -> Self {
        self.vars.remove(name);
        self
    }
    fn file(mut self, rel: &str) -> Self {
        self.profile_files.insert(rel.to_string());
        self
    }
    fn no_file(mut self, rel: &str) -> Self {
        self.profile_files.remove(rel);
        self
    }
    fn scripts(mut self, scripts: Scripts) -> Self {
        self.scripts = scripts;
        self
    }
    fn no_program(mut self, program: &'static str) -> Self {
        self.programs.remove(program);
        self
    }
    fn only_in_local_bin(mut self, program: &'static str) -> Self {
        self.programs.remove(program);
        self.local_bin.insert(program);
        self
    }
    fn only_in_cargo_bin(mut self, program: &'static str) -> Self {
        self.programs.remove(program);
        self.cargo_bin.insert(program);
        self
    }
    fn key_file(mut self, bytes: &[u8]) -> Self {
        self.key_file = Some(bytes.to_vec());
        self
    }
    fn auth_json(mut self) -> Self {
        self.auth_json = true;
        self
    }
    fn rc(mut self, program: &'static str, rc: i32) -> Self {
        self.rcs.insert(program, rc);
        self
    }
    fn noise(mut self, program: &'static str, out: &str, err: &str) -> Self {
        self.noise.insert(program, (out.to_string(), err.to_string()));
        self
    }
    fn diverges(mut self, divergence: Divergence) -> Self {
        self.divergence = Some(divergence);
        self
    }

    /// Marks the scenarios whose difference is a named decision.
    fn finalize(mut self) -> Self {
        let key_nul = self.backend == "opencode"
            && self.key_file.as_ref().is_some_and(|key| key.contains(&0))
            && !self.key_unreadable
            && self.vars.get("GROQ_API_KEY").is_none_or(String::is_empty);
        let prompt_nul =
            self.backend == "cursor" && self.prompt.contains(&0) && self.prompt.len() < 131_072;
        if key_nul || prompt_nul {
            self.divergence = Some(Divergence::NulWarning);
        }
        let opencode_model_missing = self.backend == "opencode"
            && !self.model.contains('/')
            && self.vars.get("OPENCODE_MODEL").is_none_or(String::is_empty);
        if opencode_model_missing {
            self.divergence = Some(Divergence::ModelUnset);
        }
        if self.backend == "claude" && self.task.contains('-') {
            self.divergence = Some(Divergence::TaskNotAName);
        }
        if self.backend == "codex" && self.repo_raw.is_some() {
            self.divergence = Some(Divergence::NonUtf8Path);
        }
        self
    }
}

// --- the world a scenario runs in ----------------------------------------------------

struct World {
    root: PathBuf,
    ctl: PathBuf,
    bin: PathBuf,
    sys: PathBuf,
    profile: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    state: PathBuf,
    scripts: PathBuf,
    prompt: PathBuf,
    log: PathBuf,
}

impl World {
    fn new(sc: &Sc, scratch: &Path) -> Self {
        let root = scratch.join("w");
        let state = root.join("state");
        Self {
            ctl: scratch.join("ctl"),
            bin: root.join("bin"),
            sys: root.join("sys"),
            profile: root.join(&sc.profile_name),
            repo: state.join(
                sc.repo_raw.as_deref().map_or_else(|| OsStr::new(&sc.repo_name), OsStr::from_bytes),
            ),
            home: root.join(&sc.home_name),
            scripts: root.join("tree/scripts"),
            prompt: state.join("prompt.txt"),
            log: state.join(&sc.log_rel),
            state,
            root,
        }
    }

    fn subst(&self, text: &str) -> String {
        text.replace("@PROFILE@", &self.profile.to_string_lossy())
            .replace("@ROOT@", &self.root.to_string_lossy())
    }
}

fn system_tool(name: &str) -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH is set");
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!("the host has no {name}, so the differential cannot run");
}

fn write_stub(path: &Path, cfg: &Path) {
    let template = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/differential/stub.sh"),
    )
    .expect("the stub template");
    assert!(template.contains("CFG='@CFG@'"), "the stub template lost its placeholder");
    let cfg_text = cfg.to_str().expect("a UTF-8 path");
    assert!(!cfg_text.contains('\''), "the call-log directory must not hold a quote");
    fs::create_dir_all(path.parent().expect("a parent")).expect("a bin directory");
    fs::write(path, template.replace("@CFG@", cfg_text)).expect("a stub");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("a mode");
}

fn build(sc: &Sc, w: &World) {
    let _ = fs::remove_dir_all(&w.root);
    fs::create_dir_all(&w.ctl).expect("a control directory");
    fs::create_dir_all(&w.sys).expect("sys");
    for tool in SYSTEM_TOOLS {
        symlink(system_tool(tool), w.sys.join(tool)).expect("a tool link");
    }
    fs::create_dir_all(&w.bin).expect("bin");
    for program in &sc.programs {
        write_stub(&w.bin.join(program), &w.bin);
    }
    for program in &sc.local_bin {
        write_stub(&w.home.join(".local/bin").join(program), &w.bin);
    }
    for program in &sc.cargo_bin {
        write_stub(&w.home.join(".cargo/bin").join(program), &w.bin);
    }
    for (program, rc) in &sc.rcs {
        fs::write(w.bin.join(format!("{program}.rc")), format!("{rc}\n")).expect("an rc file");
    }
    for (program, (out, err)) in &sc.noise {
        fs::write(w.bin.join(format!("{program}.out")), out).expect("an out file");
        fs::write(w.bin.join(format!("{program}.err")), err).expect("an err file");
    }
    for rel in &sc.profile_files {
        let path = w.profile.join(rel);
        fs::create_dir_all(path.parent().expect("a parent")).expect("a profile directory");
        fs::write(path, "{}\n").expect("a wall file");
    }
    fs::create_dir_all(&w.profile).expect("the profile directory");
    fs::create_dir_all(&w.home).expect("home");
    if let Some(bytes) = &sc.key_file {
        let path = w.home.join(".groq-staging-key");
        fs::write(&path, bytes).expect("a key file");
        if sc.key_unreadable {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("a mode");
        }
    }
    if sc.auth_json {
        let dir = w.home.join(".local/share/opencode");
        fs::create_dir_all(&dir).expect("an opencode data directory");
        fs::write(dir.join("auth.json"), "{}\n").expect("auth.json");
    }
    if sc.scripts != Scripts::NoDir && sc.scripts != Scripts::Empty {
        fs::create_dir_all(&w.scripts).expect("scripts");
        if sc.scripts == Scripts::WithFilter {
            fs::write(w.scripts.join("transcript.py"), "# the filter is a stub\n")
                .expect("a filter");
        }
    }
    fs::create_dir_all(&w.repo).expect("the checkout");
    if sc.prompt_symlink {
        let real = w.state.join("prompt.real");
        fs::write(&real, &sc.prompt).expect("the prompt");
        symlink(&real, &w.prompt).expect("the prompt link");
    } else {
        fs::write(&w.prompt, &sc.prompt).expect("the prompt");
    }
    fs::create_dir_all(w.log.parent().expect("a parent")).expect("the log directory");
    fs::write(&w.log, &sc.prior_log).expect("the log");
}

/// Every regular file under the root, except the tool links, with its bytes.
fn snapshot(w: &World) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let mut entries: Vec<_> =
            fs::read_dir(dir).expect("a directory").map(|e| e.expect("an entry")).collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let kind = entry.file_type().expect("a file type");
            let rel =
                path.strip_prefix(root).expect("under the root").to_string_lossy().into_owned();
            if rel == "sys" {
                continue;
            }
            if kind.is_dir() {
                walk(&path, root, out);
            } else if kind.is_file() {
                let mode = entry.metadata().expect("metadata").permissions().mode();
                // A stub is the same text on both sides; its bytes only add noise.
                if mode & 0o111 != 0 {
                    out.insert(rel, b"<executable>".to_vec());
                } else {
                    out.insert(rel, fs::read(&path).unwrap_or_else(|_| b"<unreadable>".to_vec()));
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&w.root, &w.root, &mut out);
    out
}

// --- running things ------------------------------------------------------------------

fn proc_for(w: &World) -> RealProc {
    RealProc::new(std::env::join_paths([&w.bin, &w.sys]).expect("a search path"), Vec::new())
}

fn run_proc(proc: &RealProc, spec: &ProcSpec) -> ProcOutput {
    let out = proc.run(spec).unwrap_or_else(|error| panic!("could not run {spec:?}: {error}"));
    assert_ne!(out.exit, ProcExit::TimedOut, "timed out: {spec:?}");
    out
}

fn exit_text(exit: ProcExit) -> String {
    match exit {
        ProcExit::Code(code) => code.to_string(),
        ProcExit::TimedOut => "timeout".to_string(),
    }
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn tree_root() -> PathBuf {
    std::env::var_os("MX_TREE")
        .map_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."), PathBuf::from)
}

#[derive(Default)]
struct Obs {
    fields: BTreeMap<String, String>,
    tree: BTreeMap<String, Vec<u8>>,
}

const TIMEOUT: Duration = Duration::from_mins(2);

fn path_str(path: &Path) -> String {
    path.to_str().expect("a UTF-8 path").to_string()
}

/// What the bash does, one operation at a time, in a child bash.
fn bash_side(sc: &Sc, w: &World) -> Obs {
    build(sc, w);
    let proc = proc_for(w);
    let driver = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/differential/driver.sh");
    let file = tree_root().join("lib/backends").join(format!("{}.sh", sc.backend));
    assert!(file.is_file(), "no backend file at {}", file.display());
    let rc_file = w.ctl.join("bash.rc");
    let exports_file = w.ctl.join("bash.exports");
    let mut obs = Obs::default();

    let env_for = |wall: bool| {
        // Every input goes in as DRV_<NAME>; driver.sh exports only the three names
        // the run exports for every child and keeps the rest as shell variables.
        let mut env: Vec<(String, String)> = vec![
            ("DRV_HOME".into(), path_str(&w.home)),
            ("DRV_PROFILE_DIR".into(), path_str(&w.profile)),
            ("DRV_MAINTAINER_TASK".into(), sc.task.clone()),
        ];
        if let Some(raw) = &sc.repo_raw {
            let mut escaped = String::new();
            for byte in raw {
                let _ = write!(escaped, "\\{byte:03o}");
            }
            env.push(("DRV_REPO_PARENT".into(), path_str(&w.state)));
            env.push(("DRV_REPO_NAME_ESC".into(), escaped));
        } else {
            env.push(("DRV_REPO_PATH".into(), path_str(&w.repo)));
        }
        match sc.scripts {
            Scripts::NoDir => {}
            Scripts::Empty => env.push(("DRV_MAINTAINER_SCRIPTS".into(), String::new())),
            Scripts::WithFilter | Scripts::WithoutFilter => {
                env.push(("DRV_MAINTAINER_SCRIPTS".into(), path_str(&w.scripts)));
            }
        }
        for (name, value) in &sc.vars {
            if wall && (name == "MAINTAINER_SETTINGS" || name == "MAINTAINER_CURSOR_DIR") {
                continue;
            }
            env.push((format!("DRV_{name}"), w.subst(value)));
        }
        env
    };
    let call = |op: &str, wall: bool, extra: &[String]| {
        let mut args = vec![path_str(&driver), op.to_string(), path_str(&file)];
        args.extend(extra.iter().cloned());
        let mut spec = ProcSpec::command("bash", args, TIMEOUT).cwd(&w.root);
        spec.env = env_for(wall);
        run_proc(&proc, &spec)
    };

    for (op, wall) in [("declare", false), ("wall", true), ("check", false)] {
        let out = call(op, wall, &[]);
        assert_eq!(out.exit, ProcExit::Code(0), "{op}: {}", lossy(&out.stderr));
        obs.fields.insert(op.to_string(), lossy(&out.stdout));
    }

    if !runnable(sc) {
        obs.fields.insert("run".into(), "skipped: a program of the plan is not installed".into());
        obs.tree = snapshot(w);
        return obs;
    }
    let args = [
        path_str(&w.prompt),
        sc.model.clone(),
        path_str(&w.log),
        path_str(&w.state),
        path_str(&rc_file),
        path_str(&exports_file),
    ];
    let _ = fs::remove_file(&rc_file);
    let _ = fs::remove_file(&exports_file);
    let out = call("run", false, &args);
    obs.fields.insert("run.exit".into(), exit_text(out.exit));
    obs.fields.insert("run.stdout".into(), lossy(&out.stdout));
    obs.fields.insert("run.stderr".into(), lossy(&out.stderr));
    obs.fields.insert(
        "run.rc".into(),
        fs::read_to_string(&rc_file)
            .map_or_else(|_| "<none>".to_string(), |t| t.trim().to_string()),
    );
    obs.fields.insert(
        "run.exports".into(),
        fs::read_to_string(&exports_file).unwrap_or_else(|_| "<none>".to_string()),
    );
    obs.tree = snapshot(w);
    obs
}

/// The plan is only run when every program it starts is installed. The shell's
/// "command not found" line and the adapter's spawn error are different texts for
/// the same fault, and `backend_check` already refuses a missing agent before the
/// orchestrator asks for a plan. A missing `python3` is the executor's to report.
fn runnable(sc: &Sc) -> bool {
    let installed = |program: &str| {
        sc.programs.contains(program)
            || sc.local_bin.contains(program)
            || sc.cargo_bin.contains(program)
    };
    installed(program_of(sc.backend))
        && (sc.backend != "claude" || sc.scripts != Scripts::WithFilter || installed("python3"))
}

/// One name per line, each ended by a newline: the shape of the bash export list.
fn name_lines<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for name in names {
        out.push_str(name);
        out.push('\n');
    }
    out
}

fn quote(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![b'\''];
    for &byte in bytes {
        if byte == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(byte);
        }
    }
    out.push(b'\'');
    out
}

fn quote_path(path: &Path) -> Vec<u8> {
    quote(path.as_os_str().as_bytes())
}

fn redirect(stage: &StageSpec) -> Vec<u8> {
    let mut out = Vec::new();
    match &stage.stdin {
        Source::Empty => out.extend_from_slice(b" </dev/null"),
        Source::File(path) => {
            out.extend_from_slice(b" <");
            out.extend(quote_path(path));
        }
        Source::PreviousStage => {}
    }
    match &stage.stdout {
        Sink::Null => out.extend_from_slice(b" >/dev/null"),
        Sink::AppendFile(path) => {
            out.extend_from_slice(b" >>");
            out.extend(quote_path(path));
        }
        Sink::NextStage => {}
        Sink::SameAsStdout => panic!("stdout cannot be the same as stdout"),
    }
    match &stage.stderr {
        Sink::Null => out.extend_from_slice(b" 2>/dev/null"),
        Sink::AppendFile(path) => {
            out.extend_from_slice(b" 2>>");
            out.extend(quote_path(path));
        }
        Sink::SameAsStdout => out.extend_from_slice(b" 2>&1"),
        Sink::NextStage => panic!("stderr cannot go to the next stage"),
    }
    out
}

/// The orchestrator stand-in: a `RunPlan` as a bash pipeline, as bytes because a
/// path need not be UTF-8. Every stage starts with an empty environment plus
/// `PATH`, `LC_ALL=C`, what the run exported and what the stage names, the way the
/// real adapter starts it.
fn render(plan: &RunPlan, log: &Path, run_env: &[(String, String)], rc_file: &Path) -> Vec<u8> {
    let mut script = b"set -uo pipefail\n".to_vec();
    script.extend_from_slice(b"export PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"\n");
    for note in &plan.log_notes {
        script.extend_from_slice(b"printf '%s\\n' ");
        script.extend(quote(note.as_bytes()));
        script.extend_from_slice(b" >>");
        script.extend(quote_path(log));
        script.push(b'\n');
    }
    let mut stages: Vec<Vec<u8>> = Vec::new();
    for stage in &plan.pipeline.stages {
        let mut text = b"( ".to_vec();
        if let Some(cwd) = &stage.cwd {
            text.extend_from_slice(b"cd ");
            text.extend(quote_path(cwd));
            text.extend_from_slice(b" && ");
        }
        text.extend_from_slice(b"exec env -i PATH=\"$PATH\" LC_ALL=C");
        let exported = plan.exports.iter().map(|e| (e.name.clone(), e.value.clone()));
        for (name, value) in
            run_env.iter().cloned().chain(exported).chain(stage.env.iter().cloned())
        {
            text.push(b' ');
            text.extend(quote(format!("{name}={value}").as_bytes()));
        }
        for arg in &stage.argv {
            text.push(b' ');
            text.extend(quote(arg.as_bytes()));
        }
        text.extend(redirect(stage));
        text.extend_from_slice(b" )");
        stages.push(text);
    }
    script.extend(stages.join(&b" | "[..]));
    script.extend_from_slice(b"\nrc=$?\nprintf '%s\\n' \"$rc\" >");
    script.extend(quote_path(rc_file));
    script.push(b'\n');
    script
}

fn context(sc: &Sc, w: &World) -> BackendCtx {
    BackendCtx {
        profile_dir: w.profile.clone(),
        repo_path: w.repo.clone(),
        task: sc.task.clone(),
        home: w.home.clone(),
        scripts_dir: match sc.scripts {
            Scripts::NoDir => None,
            Scripts::Empty => Some(PathBuf::new()),
            Scripts::WithFilter | Scripts::WithoutFilter => Some(w.scripts.clone()),
        },
        vars: sc.vars.iter().map(|(k, v)| (k.clone(), w.subst(v))).collect(),
    }
}

type Mutator<'a> = Option<&'a dyn Fn(&mut RunPlan)>;

/// What the port does for the same four operations.
#[allow(clippy::too_many_lines, reason = "the four operations read top to bottom")]
fn rust_side(sc: &Sc, w: &World, mutate: Mutator<'_>) -> Obs {
    build(sc, w);
    let proc = proc_for(w);
    let backend: Box<dyn Backend> = backend_by_name(sc.backend).expect("a backend");
    let ctx = context(sc, w);
    let host = RealHost::for_orchestrator(
        &w.home,
        &std::env::join_paths([&w.bin, &w.sys]).expect("a path"),
    );
    let mut obs = Obs::default();

    obs.fields.insert(
        "declare".into(),
        format!(
            "name={}\nrehearsal={}\nallowed={}\n",
            backend.name(),
            if backend.declares_rehearsal() { "yes" } else { "no" },
            backend.allowed_tasks().map_or_else(|| "any".to_string(), |tasks| tasks.join(" "))
        ),
    );

    let (mut settings, mut cursor) = ("<unset>".to_string(), "<unset>".to_string());
    let rc = if backend.declares_rehearsal() {
        match backend.rehearsal_wall(&ctx, &host) {
            Some(Export { name, value }) => {
                match name.as_str() {
                    "MAINTAINER_SETTINGS" => settings = value,
                    "MAINTAINER_CURSOR_DIR" => cursor = value,
                    other => panic!("a wall exported {other}"),
                }
                "0"
            }
            None => "1",
        }
    } else {
        "none"
    };
    obs.fields.insert(
        "wall".into(),
        format!("rc={rc}\nMAINTAINER_SETTINGS={settings}\nMAINTAINER_CURSOR_DIR={cursor}\n"),
    );

    let check = match backend.check(&ctx, &host) {
        Ok(()) => "rc=0\nmsg=\n".to_string(),
        Err(message) => format!("rc=1\nmsg={}\n", message.trim_end_matches('\n')),
    };
    obs.fields.insert("check".into(), check);

    if !runnable(sc) {
        obs.fields.insert("run".into(), "skipped: a program of the plan is not installed".into());
        obs.tree = snapshot(w);
        return obs;
    }
    let request = RunRequest {
        prompt_file: w.prompt.clone(),
        model: sc.model.clone(),
        log: w.log.clone(),
        extra_dir: w.state.clone(),
    };
    let rc_file = w.ctl.join("rust.rc");
    let _ = fs::remove_file(&rc_file);
    // The executor passes what the run exported, and a key the run inherited,
    // before the backend is asked (decisions P2B-7, P2B-10).
    let run_env: Vec<(String, String)> = ctx
        .vars
        .iter()
        .filter(|(name, _)| RUN_EXPORTED.contains(&name.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut exported_names: Vec<String> = Vec::new();
    let script = match backend.plan(&ctx, &host, &request) {
        Ok(mut plan) => {
            if let Some(mutate) = mutate {
                mutate(&mut plan);
            }
            exported_names = plan.exports.iter().map(|e| e.name.clone()).collect();
            render(&plan, &w.log, &run_env, &rc_file)
        }
        Err(refusal) => {
            let mut script = b"printf '%s\n' ".to_vec();
            script.extend(quote(refusal.log_note.as_bytes()));
            script.extend_from_slice(b" >>");
            script.extend(quote_path(&w.log));
            script.extend_from_slice(b"\nprintf '1\\n' >");
            script.extend(quote_path(&rc_file));
            script.push(b'\n');
            script
        }
    };
    exported_names.sort();
    obs.fields.insert("run.exports".into(), name_lines(exported_names.iter().map(String::as_str)));
    let script_file = w.ctl.join("exec.sh");
    fs::write(&script_file, script).expect("the script");
    let mut spec = ProcSpec::command("bash", [path_str(&script_file)], TIMEOUT).cwd(&w.root);
    spec.env = vec![("HOME".to_string(), path_str(&w.home))];
    let out = run_proc(&proc, &spec);
    obs.fields.insert("run.exit".into(), exit_text(out.exit));
    obs.fields.insert("run.stdout".into(), lossy(&out.stdout));
    obs.fields.insert("run.stderr".into(), lossy(&out.stderr));
    obs.fields.insert(
        "run.rc".into(),
        fs::read_to_string(&rc_file)
            .map_or_else(|_| "<none>".to_string(), |t| t.trim().to_string()),
    );
    obs.tree = snapshot(w);
    obs
}

// --- comparing -----------------------------------------------------------------------

fn preview(bytes: &[u8]) -> String {
    let text = lossy(bytes);
    if text.len() > 300 { format!("{}... ({} bytes)", &text[..300], text.len()) } else { text }
}

/// The first line of `text` that differs from the same line of `other`, so a long
/// file that differs in its last record shows that record.
fn first_difference(text: &[u8], other: &[u8]) -> String {
    let (a, b) = (lossy(text), lossy(other));
    let mut theirs = b.lines();
    for line in a.lines() {
        if theirs.next() != Some(line) {
            return line.chars().take(300).collect();
        }
    }
    format!("(the same lines; {} bytes against {})", text.len(), other.len())
}

fn log_key(w: &World) -> String {
    w.log.strip_prefix(&w.root).expect("the log is under the root").to_string_lossy().into_owned()
}

/// Asserts the pinned difference on both sides, then makes the two sides equal
/// for the fields it explains. A failed assertion is a difference.
#[allow(clippy::too_many_lines, reason = "one arm per named difference")]
fn apply_divergence(sc: &Sc, w: &World, bash: &mut Obs, rust: &mut Obs, diffs: &mut Vec<String>) {
    let Some(kind) = sc.divergence else { return };
    if !runnable(sc) {
        return; // the run was not made, so there is nothing pinned to assert
    }
    let log = log_key(w);
    let mut expect = |ok: bool, what: &str| {
        if !ok {
            diffs.push(format!("pinned difference {kind:?} does not hold: {what}"));
        }
    };
    let bash_log = bash.tree.get(&log).cloned().unwrap_or_default();
    match kind {
        Divergence::ModelUnset => {
            expect(bash.fields["run.exit"] != "0", "the bash process should have ended");
            expect(
                bash.fields["run.stderr"]
                    .contains("OPENCODE_MODEL: lib/defaults.env was not loaded"),
                "the bash should name OPENCODE_MODEL on stderr",
            );
            expect(bash.fields["run.rc"] == "<none>", "the bash never returned from backend_run");
            expect(!bash.tree.contains_key("bin/calls.log"), "the bash started no program");
            let mut want = sc.prior_log.clone();
            want.extend_from_slice(b"OPENCODE_MODEL: lib/defaults.env was not loaded\n");
            expect(rust.tree.get(&log) == Some(&want), "the port should log the refusal");
            expect(rust.fields["run.rc"] == "1", "the port should fail the backend");
            expect(!rust.tree.contains_key("bin/calls.log"), "the port started no program");
            for key in ["run.exit", "run.rc", "run.stderr", "run.exports"] {
                let value = bash.fields[key].clone();
                rust.fields.insert(key.to_string(), value);
            }
            rust.tree.insert(log, bash_log);
        }
        Divergence::FilterNoise => {
            let (out, err) = sc.noise.get("python3").expect("a noisy filter").clone();
            expect(
                bash.fields["run.stdout"] == out,
                "the bash filter's stdout should reach the run",
            );
            expect(
                bash.fields["run.stderr"] == err,
                "the bash filter's stderr should reach the run",
            );
            expect(rust.fields["run.stdout"].is_empty(), "the port discards the filter's stdout");
            expect(
                rust.fields["run.stderr"].is_empty(),
                "the port keeps the filter's stderr off the run",
            );
            let mut want = bash_log.clone();
            want.extend_from_slice(err.as_bytes());
            expect(
                rust.tree.get(&log) == Some(&want),
                "the port appends the filter's stderr to the log",
            );
            for key in ["run.stdout", "run.stderr"] {
                let value = bash.fields[key].clone();
                rust.fields.insert(key.to_string(), value);
            }
            rust.tree.insert(log, bash_log);
        }
        Divergence::TaskNotAName => {
            expect(bash.fields["run.exit"] == "1", "the bash process should have ended with 1");
            expect(
                bash.fields["run.stderr"].contains("invalid variable name"),
                "the bash should say the variable name is invalid",
            );
            expect(bash.fields["run.rc"] == "<none>", "the bash never returned from backend_run");
            expect(!bash.tree.contains_key("bin/calls.log"), "the bash started no program");
            expect(
                rust.tree.contains_key("bin/calls.log"),
                "the port plans the run and starts the agent",
            );
            for key in ["run.exit", "run.rc", "run.stdout", "run.stderr", "run.exports"] {
                let value = bash.fields[key].clone();
                rust.fields.insert(key.to_string(), value);
            }
            rust.tree = bash.tree.clone();
        }
        Divergence::NonUtf8Path => {
            expect(
                bash.tree.contains_key("bin/calls.log"),
                "the bash runs codex in a checkout path that is not UTF-8",
            );
            let note = rust.tree.get(&log).map(|bytes| lossy(bytes)).unwrap_or_default();
            expect(note.contains("not valid UTF-8"), "the port logs why it refused the path");
            expect(rust.fields["run.rc"] == "1", "the port fails the backend");
            expect(!rust.tree.contains_key("bin/calls.log"), "the port started no program");
            for key in ["run.exit", "run.rc", "run.stdout", "run.stderr", "run.exports"] {
                let value = bash.fields[key].clone();
                rust.fields.insert(key.to_string(), value);
            }
            rust.tree = bash.tree.clone();
        }
        Divergence::NulWarning => {
            expect(
                bash.fields["run.stderr"].contains("ignored null byte in input"),
                "the bash should warn about the NUL byte",
            );
            expect(rust.fields["run.stderr"].is_empty(), "the port drops the NUL byte silently");
            bash.fields.insert("run.stderr".into(), String::new());
        }
        Divergence::UnreadableKey => {
            expect(
                bash.fields["run.stderr"].contains("Permission denied"),
                "the bash should report the unreadable key file",
            );
            expect(
                rust.fields["run.stderr"].is_empty(),
                "the port says nothing about the key file",
            );
            bash.fields.insert("run.stderr".into(), String::new());
        }
    }
}

/// True when the opencode backend reads the key from the key file: no key or an
/// empty one in the environment, and a key file that can be read.
fn reads_key_file(sc: &Sc) -> bool {
    sc.backend == "opencode"
        && sc.key_file.is_some()
        && !sc.key_unreadable
        && sc.vars.get("GROQ_API_KEY").is_none_or(String::is_empty)
}

/// P2B-7: the bash exports `GROQ_API_KEY` into the whole run after it reads the key
/// file, and the port gives the key to the opencode stage only. Assert that the
/// bash exported it only in that case, then drop it from the bash side so the
/// remaining names are compared.
fn normalise_key_export(sc: &Sc, bash: &mut Obs, diffs: &mut Vec<String>) {
    let Some(text) = bash.fields.get_mut("run.exports") else { return };
    if !text.lines().any(|name| name == "GROQ_API_KEY") {
        return;
    }
    if !reads_key_file(sc) {
        diffs.push("P2B-7: the bash exported a key it did not read from the key file".to_string());
    }
    let kept = name_lines(text.lines().filter(|name| *name != "GROQ_API_KEY"));
    *text = kept;
}

fn compare(sc: &Sc, w: &World, mut bash: Obs, mut rust: Obs) -> (usize, Vec<String>) {
    let mut diffs = Vec::new();
    normalise_key_export(sc, &mut bash, &mut diffs);
    apply_divergence(sc, w, &mut bash, &mut rust, &mut diffs);
    let mut compared = 0;
    let keys: BTreeSet<&String> = bash.fields.keys().chain(rust.fields.keys()).collect();
    for key in keys {
        compared += 1;
        let (b, r) = (bash.fields.get(key), rust.fields.get(key));
        if b != r {
            diffs.push(format!(
                "{key}: bash {:?} port {:?}",
                b.map(|t| preview(t.as_bytes())),
                r.map(|t| preview(t.as_bytes()))
            ));
        }
    }
    let paths: BTreeSet<&String> = bash.tree.keys().chain(rust.tree.keys()).collect();
    for path in paths {
        compared += 1;
        let (b, r) = (bash.tree.get(path), rust.tree.get(path));
        if b != r {
            diffs.push(format!(
                "file {path}: first differing line: bash {:?} port {:?}",
                b.map(|t| first_difference(t, r.map_or(&[][..], Vec::as_slice))),
                r.map(|t| first_difference(t, b.map_or(&[][..], Vec::as_slice)))
            ));
        }
    }
    (compared, diffs)
}

/// How much of the behaviour the scenarios reached. A run that compared only
/// refusals, or only successes, would still print zero differences.
#[derive(Default)]
struct Tally {
    scenarios: usize,
    comparisons: usize,
    check_ok: usize,
    check_refused: usize,
    ran: usize,
    ran_nonzero: usize,
    skipped: usize,
    pinned: usize,
    failures: Vec<String>,
}

fn run_all(backend: &'static str, scenarios: Vec<Sc>) -> Tally {
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("differential-{backend}"));
    let mut tally = Tally::default();
    for sc in scenarios {
        let w = World::new(&sc, &scratch);
        let bash = bash_side(&sc, &w);
        let rust = rust_side(&sc, &w, None);
        if bash.fields["check"].starts_with("rc=0") {
            tally.check_ok += 1;
        } else {
            tally.check_refused += 1;
        }
        match bash.fields.get("run.rc").map(String::as_str) {
            None => tally.skipped += 1,
            Some("0") => tally.ran += 1,
            Some(_) => {
                tally.ran += 1;
                tally.ran_nonzero += 1;
            }
        }
        tally.pinned += usize::from(sc.divergence.is_some());
        let (compared, diffs) = compare(&sc, &w, bash, rust);
        tally.scenarios += 1;
        tally.comparisons += compared;
        for diff in diffs {
            tally.failures.push(format!("[{}] {diff}", sc.label));
        }
    }
    let _ = fs::remove_dir_all(&scratch);
    tally
}

fn seed() -> u64 {
    std::env::var("DIFF_SEED").ok().and_then(|text| text.parse().ok()).unwrap_or(DEFAULT_SEED)
}

fn report(backend: &str, scenarios: usize, tally: &Tally) {
    println!(
        "differential[{backend}]: seed={} scenarios={} comparisons={} differences={} \
         (check ok {}, check refused {}, run compared {}, run exit non-zero {}, run skipped {}, pinned {})",
        seed(),
        scenarios,
        tally.comparisons,
        tally.failures.len(),
        tally.check_ok,
        tally.check_refused,
        tally.ran,
        tally.ran_nonzero,
        tally.skipped,
        tally.pinned
    );
}

fn check(backend: &'static str, scenarios: Vec<Sc>) {
    let count = scenarios.len();
    assert!(count >= FLOOR, "only {count} scenarios for {backend}; at least {FLOOR} must run");
    let tally = run_all(backend, scenarios);
    report(backend, count, &tally);
    assert_eq!(tally.scenarios, count, "every scenario must run");
    assert!(tally.comparisons > count * 8, "too few comparisons: {}", tally.comparisons);
    assert!(tally.check_ok >= 15, "only {} scenarios pass backend_check", tally.check_ok);
    assert!(tally.check_refused >= 2, "only {} scenarios fail backend_check", tally.check_refused);
    assert!(tally.ran >= FLOOR, "only {} scenarios ran the agent", tally.ran);
    assert!(
        tally.ran_nonzero >= 4,
        "only {} scenarios ran an agent that failed",
        tally.ran_nonzero
    );
    assert!(
        tally.failures.is_empty(),
        "{} differences between the bash and the port:\n{}",
        tally.failures.len(),
        tally.failures.iter().take(12).cloned().collect::<Vec<_>>().join("\n")
    );
}

// --- a seeded generator --------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).expect("a small bound")).expect("fits")
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

fn random_prompt(rng: &mut Rng) -> Vec<u8> {
    let sizes = [0usize, 1, 17, 500, 4096, 131_070, 131_071, 131_072, 131_073, 140_000];
    let fragments: [&[u8]; 13] = [
        b"plain",
        b" spaced  out ",
        b"it's",
        b"\"quoted\"",
        b"$HOME `id` $(x)",
        b"back\\slash",
        b"\n",
        "caf\u{e9} \u{2713}".as_bytes(),
        b"\t",
        b"\r",
        b"\r\n",
        b"nul\0byte",
        b"  ",
    ];
    let mut out = Vec::new();
    if rng.chance(55) {
        for _ in 0..=rng.below(6) {
            out.extend_from_slice(rng.pick(&fragments));
        }
    } else {
        out = xs(*rng.pick(&sizes));
    }
    if rng.chance(45) {
        let endings: [&[u8]; 6] = [b"\n", b"\n\n", b"\r\n", b"\r", b"\n\r", b"\r\n\r\n"];
        out.extend_from_slice(rng.pick(&endings));
    }
    out
}

#[allow(clippy::too_many_lines, reason = "one generator, read from top to bottom")]
fn random_scenario(rng: &mut Rng, backend: &'static str, index: usize) -> Sc {
    let mut sc = Sc::new(backend, &format!("random {index}"));
    sc.task = (*rng.pick(&["review", "issues", "triage", "readonly-review"])).to_string();
    sc.model = (*rng.pick(&[
        "opus",
        "sonnet",
        "groq/other",
        "a b",
        "it's",
        "",
        "/x",
        "x/y/z",
        "m\"q",
        "gpt-5",
    ]))
    .to_string();
    sc.prompt = random_prompt(rng);
    sc.profile_name = (*rng.pick(&["profile", "pro file", "pr'ofile"])).to_string();
    sc.repo_name = (*rng.pick(&["repo", "re po", "re'po"])).to_string();
    if rng.chance(7) {
        sc.repo_raw = Some(rng.pick(&[&b"re\xffpo"[..], b"r\xe9po", b"\xc0\xaf"]).to_vec());
    }
    sc.prompt_symlink = rng.chance(12);
    sc.home_name = (*rng.pick(&["home", "ho me"])).to_string();
    sc.log_rel = (*rng.pick(&[
        "logs/run.log",
        "lo gs/run 1.log",
        "logs/run.txt",
        "logs/a.log.log",
        "logs/run",
        "logs/it's.log",
    ]))
    .to_string();
    sc.prior_log = rng.pick(&[&b""[..], b"PRIOR\n", b"no newline"]).to_vec();
    sc.scripts = *rng.pick(&[
        Scripts::WithFilter,
        Scripts::WithFilter,
        Scripts::WithoutFilter,
        Scripts::NoDir,
        Scripts::Empty,
    ]);

    for program in sc.programs.clone() {
        let roll = rng.below(100);
        if roll < 28 {
            sc.programs.remove(program);
        }
        if (12..20).contains(&roll) {
            sc.local_bin.insert(program);
        } else if (20..28).contains(&roll) {
            sc.cargo_bin.insert(program);
        }
    }
    for rel in [
        "settings.json",
        "settings-rehearsal.json",
        "opencode.json",
        "opencode-rehearsal.json",
        "cursor/cli-config.json",
        "cursor-rehearsal/cli-config.json",
    ] {
        if rng.chance(55) {
            sc.profile_files.insert(rel.to_string());
        } else {
            sc.profile_files.remove(rel);
        }
    }

    let settings = rng.below(10);
    if settings < 8 {
        let value = [
            None,
            None,
            None,
            None,
            Some(""),
            Some("@PROFILE@/settings-rehearsal.json"),
            Some("@PROFILE@/opencode-rehearsal.json"),
            Some(" "),
        ][settings];
        if let Some(value) = value {
            sc.vars.insert("MAINTAINER_SETTINGS".to_string(), value.to_string());
        }
    }
    let cursor = rng.below(10);
    if cursor < 5 {
        let value = [
            "",
            "@PROFILE@/cursor-rehearsal",
            "@PROFILE@/cursor",
            " ",
            "@PROFILE@/cursor-rehearsal",
        ][cursor];
        sc.vars.insert("MAINTAINER_CURSOR_DIR".to_string(), value.to_string());
    }
    let task = sc.task.clone();
    if rng.chance(50) {
        sc.vars
            .insert(format!("THINKING_{task}"), (*rng.pick(&["31999", "", "5", " "])).to_string());
    }
    if rng.chance(40) {
        sc.vars
            .insert("MAINTAINER_THINKING".to_string(), (*rng.pick(&["777", "", "\t"])).to_string());
    }
    if rng.chance(45) {
        sc.vars.insert(
            format!("SUBAGENT_MODEL_{task}"),
            (*rng.pick(&["sonnet", "", "haiku", " "])).to_string(),
        );
    }
    if backend == "opencode" {
        sc.vars.remove("GROQ_API_KEY");
        match rng.below(10) {
            0..=3 => {
                sc.vars.insert("GROQ_API_KEY".to_string(), "env-key-1".to_string());
            }
            4..=5 => {
                sc.vars.insert("GROQ_API_KEY".to_string(), String::new());
            }
            6 => {
                sc.vars.insert("GROQ_API_KEY".to_string(), " ".to_string());
            }
            _ => {}
        }
        sc.vars.remove("OPENCODE_MODEL");
        match rng.below(10) {
            0..=7 => {
                sc.vars.insert("OPENCODE_MODEL".to_string(), "groq/x".to_string());
            }
            8 => {
                sc.vars.insert("OPENCODE_MODEL".to_string(), String::new());
            }
            9 if rng.chance(50) => {
                sc.vars.insert("OPENCODE_MODEL".to_string(), " ".to_string());
            }
            _ => {}
        }
        if rng.chance(55) {
            let contents: [&[u8]; 9] = [
                b"key-1\n",
                b"a\nb\n\n",
                b"",
                b"\n",
                b" sp\tk \r\n",
                b"nonewline",
                b"ke\0y\n",
                b"\0",
                b"crlf\r\nkey\r\n",
            ];
            sc.key_file = Some(rng.pick(&contents).to_vec());
        }
        sc.auth_json = rng.chance(30);
    }
    let main = program_of(backend);
    if rng.chance(35) {
        sc.rcs.insert(main, *rng.pick(&[1, 3, 127, 255]));
    }
    if backend == "claude" && rng.chance(30) {
        sc.rcs.insert("python3", *rng.pick(&[1, 5]));
    }
    sc.finalize()
}

fn random_scenarios(backend: &'static str) -> Vec<Sc> {
    let mut rng =
        Rng(seed() ^ (backend.len() as u64 * 0x1000_0001) ^ u64::from(backend.as_bytes()[2]));
    (0..RANDOM_PER_BACKEND).map(|i| random_scenario(&mut rng, backend, i)).collect()
}

// --- the hand-written scenarios ------------------------------------------------------

/// Scenarios every backend shares: the shape of the log, the checkout path, the
/// program's exit status, the prompt sizes, and where the program is found.
fn common(backend: &'static str) -> Vec<Sc> {
    let program = program_of(backend);
    let mut list = vec![
        Sc::new(backend, "base"),
        Sc::new(backend, "program missing").no_program(program),
        Sc::new(backend, "program only in ~/.local/bin").only_in_local_bin(program),
        Sc::new(backend, "program only in ~/.cargo/bin").only_in_cargo_bin(program),
        Sc::new(backend, "profile dir with spaces").profile("pro file"),
        Sc::new(backend, "profile dir with a quote").profile("pr'ofile"),
        Sc::new(backend, "checkout path with a quote").repo("re'po"),
        Sc::new(backend, "checkout path with spaces").repo("re po"),
        Sc::new(backend, "home with spaces").home("ho me"),
        Sc::new(backend, "log path with spaces").log("lo gs/run 1.log"),
        Sc::new(backend, "log without a .log suffix").log("logs/run"),
        Sc::new(backend, "log ending .log.log").log("logs/a.log.log"),
        Sc::new(backend, "log with a quote").log("logs/it's.log"),
        Sc::new(backend, "empty log before the run").prior(b""),
        Sc::new(backend, "log without a trailing newline").prior(b"no newline"),
        Sc::new(backend, "agent exits 1").rc(program, 1),
        Sc::new(backend, "agent exits 3").rc(program, 3),
        Sc::new(backend, "agent exits 127").rc(program, 127),
        Sc::new(backend, "agent exits 255").rc(program, 255),
        Sc::new(backend, "prompt of 0 bytes").prompt(Vec::new()),
        Sc::new(backend, "prompt of 1 byte").prompt(b"x".to_vec()),
        Sc::new(backend, "prompt of 131071 bytes").prompt(xs(131_071)),
        Sc::new(backend, "prompt of 131072 bytes").prompt(xs(131_072)),
        Sc::new(backend, "prompt of 140000 bytes").prompt(xs(140_000)),
        Sc::new(backend, "prompt with shell metacharacters")
            .prompt(b"$HOME `id` $(rm -rf /) \"q\" 'q' \\n ; | & * ?\n".to_vec()),
        Sc::new(backend, "prompt with unicode")
            .prompt("caf\u{e9} \u{2713} \u{1f600}\n".as_bytes().to_vec()),
        Sc::new(backend, "model with spaces and quotes").model("m o\"d'el"),
        Sc::new(backend, "empty model").model(""),
        Sc::new(backend, "model with a slash").model("groq/other-model"),
        Sc::new(backend, "prompt with CRLF line endings").prompt(b"one\r\ntwo\r\n".to_vec()),
        Sc::new(backend, "prompt ending in a lone CR").prompt(b"one\r".to_vec()),
        Sc::new(backend, "prompt ending in CR LF CR LF").prompt(b"one\r\n\r\n".to_vec()),
        Sc::new(backend, "prompt ending in LF CR").prompt(b"one\n\r".to_vec()),
        Sc::new(backend, "prompt file that is a symlink").prompt_symlink(),
        Sc::new(backend, "prompt of 131072 bytes behind a symlink")
            .prompt(xs(131_072))
            .prompt_symlink(),
        Sc::new(backend, "checkout directory name that is not UTF-8").repo_raw(b"re\xffpo"),
        Sc::new(backend, "checkout directory name in Latin-1").repo_raw(b"r\xe9po"),
        Sc::new(backend, "prompt with a NUL byte").prompt(b"a\0b\n".to_vec()),
        Sc::new(backend, "another task").task("issues"),
        Sc::new(backend, "scripts directory missing").scripts(Scripts::NoDir),
    ];
    // A wall rendered or not never changes what the other backends run.
    list.push(
        Sc::new(backend, "every wall file present")
            .file("settings-rehearsal.json")
            .file("opencode-rehearsal.json")
            .file("cursor-rehearsal/cli-config.json"),
    );
    list
}

fn claude_scenarios() -> Vec<Sc> {
    let mut list = common("claude");
    let c = |label: &str| Sc::new("claude", label);
    list.extend(vec![
        c("no settings file").no_file("settings.json"),
        c("rehearsal wall rendered").file("settings-rehearsal.json"),
        c("run holds the rehearsal wall")
            .file("settings-rehearsal.json")
            .var("MAINTAINER_SETTINGS", "@PROFILE@/settings-rehearsal.json"),
        c("run holds a wall that is missing").var("MAINTAINER_SETTINGS", "@PROFILE@/gone.json"),
        c("run holds an empty wall variable").var("MAINTAINER_SETTINGS", ""),
        c("opencode wall variable pointing at a file that exists")
            .file("opencode-rehearsal.json")
            .var("MAINTAINER_SETTINGS", "@PROFILE@/opencode-rehearsal.json"),
        c("scripts directory without transcript.py").scripts(Scripts::WithoutFilter),
        c("empty MAINTAINER_SCRIPTS").scripts(Scripts::Empty),
        c("python3 missing").no_program("python3"),
        c("thinking budget of one space").var("THINKING_review", " "),
        c("general thinking budget of a tab").var("MAINTAINER_THINKING", "\t"),
        c("empty task budget and a general budget of a space")
            .var("THINKING_review", "")
            .var("MAINTAINER_THINKING", " "),
        c("subagent model of one space").var("SUBAGENT_MODEL_review", " "),
        c("wall variable of one space").var("MAINTAINER_SETTINGS", " "),
        c("task name that is not a shell identifier (P2B-6)").task("readonly-review"),
        c("thinking for the task").var("THINKING_review", "31999"),
        c("thinking for the general fallback").var("MAINTAINER_THINKING", "777"),
        c("thinking for both").var("THINKING_review", "31999").var("MAINTAINER_THINKING", "777"),
        c("empty task thinking with a general one")
            .var("THINKING_review", "")
            .var("MAINTAINER_THINKING", "777"),
        c("thinking for another task").var("THINKING_issues", "5"),
        c("empty thinking everywhere").var("THINKING_review", "").var("MAINTAINER_THINKING", ""),
        c("subagent model").var("SUBAGENT_MODEL_review", "sonnet"),
        c("empty subagent model").var("SUBAGENT_MODEL_review", ""),
        c("subagent model of another task").var("SUBAGENT_MODEL_issues", "haiku"),
        c("thinking and subagent together")
            .var("THINKING_review", "9")
            .var("SUBAGENT_MODEL_review", "opus"),
        c("another task with its own thinking").task("issues").var("THINKING_issues", "5000"),
        c("claude exits 3, filter exits 0").rc("claude", 3),
        c("filter exits 5").rc("python3", 5),
        c("claude 3 and filter 5").rc("claude", 3).rc("python3", 5),
        c("claude 0 and filter 7").rc("python3", 7),
        c("no filter, claude exits 4").scripts(Scripts::WithoutFilter).rc("claude", 4),
        c("no filter, log with spaces").scripts(Scripts::WithoutFilter).log("lo gs/run 1.log"),
        c("no filter, no scripts, large prompt").scripts(Scripts::NoDir).prompt(xs(140_000)),
        c("filter output is not the run's own (P2B-3)")
            .noise("python3", "filter says out\n", "filter says err\n")
            .diverges(Divergence::FilterNoise),
    ]);
    list
}

fn codex_scenarios() -> Vec<Sc> {
    let mut list = common("codex");
    let c = |label: &str| Sc::new("codex", label);
    list.extend(vec![
        c("settings files present change nothing")
            .file("settings.json")
            .file("settings-rehearsal.json"),
        c("thinking variables change nothing")
            .var("THINKING_review", "9")
            .var("MAINTAINER_THINKING", "5")
            .var("SUBAGENT_MODEL_review", "sonnet"),
        c("run holds a wall variable").var("MAINTAINER_SETTINGS", "@PROFILE@/settings.json"),
        c("run holds a cursor wall variable").var("MAINTAINER_CURSOR_DIR", "@PROFILE@/cursor"),
        c("agent is noisy on both streams").noise("codex", "line one\nline two\n", "warn\n"),
        c("agent prints nothing").noise("codex", "", ""),
        c("agent exits 2 with output").rc("codex", 2).noise("codex", "partial\n", "boom\n"),
        c("model with a slash").model("openai/gpt-5"),
        c("checkout with spaces and quote").repo("re 'po"),
    ]);
    list
}

fn cursor_scenarios() -> Vec<Sc> {
    let mut list = common("cursor");
    let c = |label: &str| Sc::new("cursor", label);
    let line = |n: usize, tail: &[u8]| {
        let mut bytes = xs(n);
        bytes.extend_from_slice(tail);
        bytes
    };
    list.extend(vec![
        c("no wall file").no_file("cursor/cli-config.json"),
        c("wall variable of one space").var("MAINTAINER_CURSOR_DIR", " "),
        c("prompt that is a symlink to 131071 bytes").prompt(xs(131_071)).prompt_symlink(),
        c("prompt with a lone CR in the middle").prompt(b"a\rb\n".to_vec()),
        c("prompt of only a NUL byte").prompt(b"\0".to_vec()),
        c("prompt with CRLF and a NUL byte").prompt(b"a\r\n\0b\r\n".to_vec()),
        c("rehearsal wall rendered").file("cursor-rehearsal/cli-config.json"),
        c("run holds the rehearsal wall")
            .file("cursor-rehearsal/cli-config.json")
            .var("MAINTAINER_CURSOR_DIR", "@PROFILE@/cursor-rehearsal"),
        c("run holds a wall that is missing").var("MAINTAINER_CURSOR_DIR", "@PROFILE@/nowhere"),
        c("run holds an empty wall variable").var("MAINTAINER_CURSOR_DIR", ""),
        c("run holds the live wall explicitly").var("MAINTAINER_CURSOR_DIR", "@PROFILE@/cursor"),
        c("rehearsal wall without a config file")
            .file("cursor-rehearsal/other.json")
            .var("MAINTAINER_CURSOR_DIR", "@PROFILE@/cursor-rehearsal"),
        c("readonly-review task").task("readonly-review"),
        c("prompt of 131070 bytes plus a newline").prompt(line(131_070, b"\n")),
        c("prompt of 131071 bytes plus a newline").prompt(line(131_071, b"\n")),
        c("prompt of 131071 bytes plus two newlines").prompt(line(131_071, b"\n\n")),
        c("prompt of only newlines").prompt(b"\n\n\n".to_vec()),
        c("prompt with leading newlines and inner blank lines")
            .prompt(b"\n\nlead\n\n\ninner\n\n".to_vec()),
        c("prompt with trailing spaces").prompt(b"line  \t \n".to_vec()),
        c("prompt that looks like an option").prompt(b"--force\n".to_vec()),
        c("prompt that is one dash").prompt(b"-\n".to_vec()),
        c("agent is noisy on both streams").noise("cursor-agent", "a\nb\n", "w\n"),
        c("agent exits 2 with output").rc("cursor-agent", 2).noise(
            "cursor-agent",
            "partial\n",
            "boom\n",
        ),
        c("oversized prompt with the program missing")
            .no_program("cursor-agent")
            .prompt(xs(140_000)),
        c("oversized prompt with a noisy agent").prompt(xs(131_072)).noise(
            "cursor-agent",
            "x\n",
            "y\n",
        ),
    ]);
    list
}

fn opencode_scenarios() -> Vec<Sc> {
    let mut list = common("opencode");
    let c = |label: &str| Sc::new("opencode", label);
    list.extend(vec![
        c("no config file").no_file("opencode.json"),
        c("OPENCODE_MODEL of one space with a bare model")
            .model("sonnet")
            .var("OPENCODE_MODEL", " "),
        c("OPENCODE_MODEL of a tab and a space with a bare model")
            .model("sonnet")
            .var("OPENCODE_MODEL", "\t "),
        c("key of one space in the environment").var("GROQ_API_KEY", " ").key_file(b"file-key\n"),
        c("wall variable of one space").var("MAINTAINER_SETTINGS", " "),
        c("key file with a NUL byte").no_var("GROQ_API_KEY").key_file(b"ke\0y\n"),
        c("key file of only a NUL byte").no_var("GROQ_API_KEY").key_file(b"\0"),
        c("key file with a NUL byte and an environment key").key_file(b"ke\0y\n"),
        c("key file with CRLF lines").no_var("GROQ_API_KEY").key_file(b"one\r\ntwo\r\n"),
        c("rehearsal wall rendered").file("opencode-rehearsal.json"),
        c("run holds the rehearsal wall")
            .file("opencode-rehearsal.json")
            .var("MAINTAINER_SETTINGS", "@PROFILE@/opencode-rehearsal.json"),
        c("run holds a wall that is missing").var("MAINTAINER_SETTINGS", "@PROFILE@/gone.json"),
        c("run holds an empty wall variable").var("MAINTAINER_SETTINGS", ""),
        c("claude wall variable reaches opencode")
            .file("settings-rehearsal.json")
            .var("MAINTAINER_SETTINGS", "@PROFILE@/settings-rehearsal.json"),
        c("key only in the environment"),
        c("key only in the key file").no_var("GROQ_API_KEY").key_file(b"file-key\n"),
        c("login only in auth.json").no_var("GROQ_API_KEY").auth_json(),
        c("no credential at all").no_var("GROQ_API_KEY"),
        c("empty key and no files").var("GROQ_API_KEY", ""),
        c("empty key and a key file").var("GROQ_API_KEY", "").key_file(b"file-key\n"),
        c("empty key and auth.json").var("GROQ_API_KEY", "").auth_json(),
        c("environment key beats the key file").key_file(b"file-key\n"),
        c("key file with inner newlines").no_var("GROQ_API_KEY").key_file(b"key-from\nfile\n\n"),
        c("key file with CRLF and spaces").no_var("GROQ_API_KEY").key_file(b" k\r\n\tv \r\n"),
        c("key file with no trailing newline").no_var("GROQ_API_KEY").key_file(b"nonewline"),
        c("empty key file").no_var("GROQ_API_KEY").key_file(b""),
        c("key file of one newline").no_var("GROQ_API_KEY").key_file(b"\n"),
        c("empty key and an empty key file").var("GROQ_API_KEY", "").key_file(b""),
        c("long key file").no_var("GROQ_API_KEY").key_file(&xs(5000)),
        c("key file and auth.json together").no_var("GROQ_API_KEY").key_file(b"k\n").auth_json(),
        c("key in the environment that has spaces and quotes").var("GROQ_API_KEY", "a b'c\"d"),
        c("model without a slash uses OPENCODE_MODEL").model("sonnet"),
        c("model with a leading slash").model("/x"),
        c("model with a trailing slash").model("x/"),
        c("OPENCODE_MODEL is another pair").var("OPENCODE_MODEL", "groq/override-model"),
        c("OPENCODE_MODEL unset with a bare model (P2B-1)")
            .no_var("OPENCODE_MODEL")
            .diverges(Divergence::ModelUnset),
        c("OPENCODE_MODEL empty with a bare model (P2B-1)")
            .var("OPENCODE_MODEL", "")
            .diverges(Divergence::ModelUnset),
        c("OPENCODE_MODEL unset with a slash model").no_var("OPENCODE_MODEL").model("groq/ok"),
        c("OPENCODE_MODEL unset with an empty model (P2B-1)")
            .no_var("OPENCODE_MODEL")
            .model("")
            .diverges(Divergence::ModelUnset),
        c("agent is noisy on both streams").noise("opencode", "a\nb\n", "w\n"),
        c("agent exits 2 with output").rc("opencode", 2).noise("opencode", "partial\n", "boom\n"),
        c("home with spaces and a key file").home("ho me").no_var("GROQ_API_KEY").key_file(b"k\n"),
    ]);
    if key_file_can_be_unreadable() {
        let mut unreadable = c("key file that cannot be read (P2B-5)")
            .no_var("GROQ_API_KEY")
            .key_file(b"fixture-key\n")
            .diverges(Divergence::UnreadableKey);
        unreadable.key_unreadable = true;
        list.push(unreadable);
    } else {
        println!(
            "differential[opencode]: running as a user that reads mode 000 files; the unreadable-key scenario is skipped"
        );
    }
    list
}

/// A file with mode 000 is unreadable for every user but root.
fn key_file_can_be_unreadable() -> bool {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("probe-unreadable");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a probe directory");
    let path = dir.join("f");
    fs::write(&path, b"x").expect("a probe file");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("a mode");
    let unreadable = fs::read(&path).is_err();
    let _ = fs::remove_dir_all(&dir);
    unreadable
}

fn all_scenarios(backend: &'static str) -> Vec<Sc> {
    let mut list = match backend {
        "claude" => claude_scenarios(),
        "codex" => codex_scenarios(),
        "cursor" => cursor_scenarios(),
        "opencode" => opencode_scenarios(),
        other => panic!("no scenarios for {other}"),
    };
    list.extend(random_scenarios(backend));
    list.into_iter().map(Sc::finalize).collect()
}

fn precondition() {
    assert!(
        !Path::new("/transcript.py").exists(),
        "/transcript.py exists on this machine, so the 'no scripts directory' scenarios mean nothing"
    );
}

#[test]
fn claude_matches_the_bash() {
    precondition();
    check("claude", all_scenarios("claude"));
}

#[test]
fn codex_matches_the_bash() {
    precondition();
    check("codex", all_scenarios("codex"));
}

#[test]
fn cursor_matches_the_bash() {
    precondition();
    check("cursor", all_scenarios("cursor"));
}

#[test]
fn opencode_matches_the_bash() {
    precondition();
    check("opencode", all_scenarios("opencode"));
}

// --- the harness itself must be able to fail -----------------------------------------

/// Runs one scenario with the plan altered and returns the differences found.
fn differences_with(sc: &Sc, mutate: &dyn Fn(&mut RunPlan)) -> Vec<String> {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("differential-control-{n}"));
    let w = World::new(sc, &scratch);
    let bash = bash_side(sc, &w);
    let rust = rust_side(sc, &w, Some(mutate));
    let (_, diffs) = compare(sc, &w, bash, rust);
    let _ = fs::remove_dir_all(&scratch);
    diffs
}

#[test]
fn the_harness_sees_a_dropped_argument_in_every_backend() {
    precondition();
    for backend in ["claude", "codex", "cursor", "opencode"] {
        let sc = Sc::new(backend, "control");
        let diffs = differences_with(&sc, &|plan| {
            let stage = plan.pipeline.stages.first();
            let mut argv: Vec<String> = stage.argv.iter().cloned().collect();
            argv.pop();
            let mut changed = stage.clone();
            changed.argv = mx_core::NonEmpty::from_vec(argv).expect("not empty");
            let mut stages: Vec<StageSpec> = plan.pipeline.stages.iter().cloned().collect();
            stages[0] = changed;
            plan.pipeline.stages = mx_core::NonEmpty::from_vec(stages).expect("not empty");
        });
        assert!(!diffs.is_empty(), "{backend}: a dropped last argument went unnoticed");
    }
}

#[test]
fn the_harness_sees_a_changed_environment_a_sink_and_a_working_directory() {
    precondition();
    // The key comes from the key file, so the plan itself names it.
    let sc = Sc::new("opencode", "control").no_var("GROQ_API_KEY").key_file(b"file-key\n");
    let env_changed = differences_with(&sc, &|plan| {
        let mut stages: Vec<StageSpec> = plan.pipeline.stages.iter().cloned().collect();
        stages[0].env.retain(|(name, _)| name != "GROQ_API_KEY");
        plan.pipeline.stages = mx_core::NonEmpty::from_vec(stages).expect("not empty");
    });
    assert!(env_changed.iter().any(|d| d.contains("calls.log")), "{env_changed:?}");

    let sink_changed = differences_with(&sc, &|plan| {
        let mut stages: Vec<StageSpec> = plan.pipeline.stages.iter().cloned().collect();
        stages[0].stderr = Sink::Null;
        plan.pipeline.stages = mx_core::NonEmpty::from_vec(stages).expect("not empty");
    });
    assert!(!sink_changed.is_empty(), "a stderr sink change went unnoticed");

    let cwd_changed = differences_with(&sc, &|plan| {
        let mut stages: Vec<StageSpec> = plan.pipeline.stages.iter().cloned().collect();
        stages[0].cwd = None;
        plan.pipeline.stages = mx_core::NonEmpty::from_vec(stages).expect("not empty");
    });
    assert!(cwd_changed.iter().any(|d| d.contains("calls.log")), "a changed cwd went unnoticed");
}

/// Runs one scenario with the first stage altered and returns the differences found.
fn first_stage_differences(sc: &Sc, change: impl Fn(&mut StageSpec)) -> Vec<String> {
    differences_with(sc, &|plan| {
        let mut stages: Vec<StageSpec> = plan.pipeline.stages.iter().cloned().collect();
        change(&mut stages[0]);
        plan.pipeline.stages = mx_core::NonEmpty::from_vec(stages).expect("not empty");
    })
}

#[test]
fn the_harness_sees_a_variable_the_plan_adds_to_a_stage_that_no_stub_logs_by_value() {
    precondition();
    for backend in ["claude", "codex", "cursor", "opencode"] {
        let sc = Sc::new(backend, "control");
        let diffs = first_stage_differences(&sc, |stage| {
            stage.env.push(("EVIL_EXTRA".to_string(), "1".to_string()));
        });
        assert!(diffs.iter().any(|d| d.contains("calls.log")), "{backend}: {diffs:?}");
    }
}

#[test]
fn the_harness_sees_home_in_the_stage_environment() {
    precondition();
    for backend in ["claude", "codex", "cursor", "opencode"] {
        let sc = Sc::new(backend, "control");
        let diffs = first_stage_differences(&sc, |stage| {
            stage.env.push(("HOME".to_string(), "/root".to_string()));
        });
        assert!(diffs.iter().any(|d| d.contains("calls.log")), "{backend}: {diffs:?}");
    }
}

#[test]
fn the_harness_sees_standard_error_opened_a_second_time_instead_of_shared() {
    precondition();
    for backend in ["codex", "cursor", "opencode"] {
        let sc = Sc::new(backend, "control");
        let diffs = differences_with(&sc, &|plan| {
            let mut stages: Vec<StageSpec> = plan.pipeline.stages.iter().cloned().collect();
            let Sink::AppendFile(path) = stages[0].stdout.clone() else {
                panic!("a single-stage plan appends its output to the log");
            };
            stages[0].stderr = Sink::AppendFile(path);
            plan.pipeline.stages = mx_core::NonEmpty::from_vec(stages).expect("not empty");
        });
        assert!(
            diffs.iter().any(|d| d.contains("calls.log")),
            "{backend}: two opens of the log read as one: {diffs:?}"
        );
    }
}

#[test]
fn the_harness_sees_the_key_exported_to_the_whole_run() {
    precondition();
    let sc = Sc::new("opencode", "control").no_var("GROQ_API_KEY").key_file(b"file-key\n");
    let diffs = differences_with(&sc, &|plan| {
        plan.exports.push(Export { name: "GROQ_API_KEY".to_string(), value: "x".to_string() });
    });
    assert!(diffs.iter().any(|d| d.starts_with("run.exports")), "{diffs:?}");
}

#[test]
fn the_harness_sees_a_variable_the_plan_forgets_to_export() {
    precondition();
    let sc = Sc::new("claude", "control").var("THINKING_review", "9");
    let diffs = differences_with(&sc, &|plan| plan.exports.clear());
    assert!(diffs.iter().any(|d| d.starts_with("run.exports")), "{diffs:?}");
}

#[test]
fn the_harness_sees_a_changed_exit_status_of_the_agent() {
    precondition();
    let sc = Sc::new("codex", "control").rc("codex", 3);
    let diffs = differences_with(&sc, &|plan| {
        let mut stages: Vec<StageSpec> = plan.pipeline.stages.iter().cloned().collect();
        stages[0].argv = mx_core::NonEmpty::new("true".to_string(), []);
        plan.pipeline.stages = mx_core::NonEmpty::from_vec(stages).expect("not empty");
    });
    assert!(diffs.iter().any(|d| d.starts_with("run.rc")), "{diffs:?}");
}

#[test]
fn the_pristine_control_finds_no_difference() {
    precondition();
    for backend in ["claude", "codex", "cursor", "opencode"] {
        let sc = Sc::new(backend, "control");
        let diffs = differences_with(&sc, &|_| {});
        assert!(diffs.is_empty(), "{backend}: {diffs:?}");
    }
}

// --- an oracle that does not depend on the port --------------------------------------

/// The call log the bash run leaves, read as text. The two sides agreeing is a
/// claim about equality; this pins what each backend starts, in words.
fn bash_calls(sc: &Sc) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(1000);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("differential-oracle-{n}"));
    let w = World::new(sc, &scratch);
    let obs = bash_side(sc, &w);
    let _ = fs::remove_dir_all(&scratch);
    let calls = obs.tree.get("bin/calls.log").expect("the bash run started a program");
    lossy(calls)
}

#[test]
fn the_bash_run_of_each_backend_starts_the_programs_the_port_plans() {
    precondition();
    let claude = bash_calls(&Sc::new("claude", "oracle").var("THINKING_review", "9"));
    for line in [
        "CALL claude\nARGC 12\nARG -p\nARG --settings\n",
        "ARG bypassPermissions\nARG --model\nARG opus\nARG --add-dir\n",
        "ARG stream-json\nARG --verbose\n",
        "ENV MAX_THINKING_TOKENS=9\nENV CLAUDE_CODE_SUBAGENT_MODEL=<unset>\n",
        "ENVNAMES LC_ALL MAX_THINKING_TOKENS PATH\n",
        "FDS stdout=pipe stderr=file n/a\n",
        "CALL python3\nARGC 3\n",
    ] {
        assert!(claude.contains(line), "claude log lacks {line:?}:\n{claude}");
    }
    let codex = bash_calls(&Sc::new("codex", "oracle"));
    assert!(
        codex
            .contains("CALL codex\nARGC 9\nARG exec\nARG --sandbox\nARG workspace-write\nARG -C\n"),
        "{codex}"
    );
    assert!(codex.contains("ARG --skip-git-repo-check\nARG -\n"), "{codex}");
    // The agent's whole environment is PATH and LC_ALL: the run owns the rest.
    assert!(codex.contains("ENVNAMES LC_ALL PATH\n"), "{codex}");
    assert!(codex.contains("FDS stdout=file stderr=file shared\n"), "{codex}");
    let cursor = bash_calls(&Sc::new("cursor", "oracle"));
    assert!(
        cursor.contains(
            "CALL cursor-agent\nARGC 6\nARG -p\nARG --output-format\nARG text\nARG -m\nARG opus\n"
        ),
        "{cursor}"
    );
    assert!(cursor.contains("ARG $'the prompt text\\nsecond line'\n"), "{cursor}");
    assert!(cursor.contains("ENV CURSOR_CONFIG_DIR="), "{cursor}");
    assert!(cursor.contains("ENVNAMES CURSOR_CONFIG_DIR LC_ALL PATH\n"), "{cursor}");
    assert!(
        cursor
            .contains("STDIN 0 e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n"),
        "{cursor}"
    );
    let opencode = bash_calls(&Sc::new("opencode", "oracle"));
    assert!(
        opencode.contains(
            "CALL opencode\nARGC 6\nARG run\nARG --model\nARG groq/openai/gpt-oss-120b\nARG --auto\nARG --format\nARG default\n"
        ),
        "{opencode}"
    );
    assert!(opencode.contains("ENV OPENCODE_CONFIG="), "{opencode}");
    assert!(opencode.contains("ENVNAMES GROQ_API_KEY LC_ALL OPENCODE_CONFIG PATH\n"), "{opencode}");
    let from_file =
        bash_calls(&Sc::new("opencode", "oracle").no_var("GROQ_API_KEY").key_file(b"file-key\n"));
    assert!(
        from_file.contains("ENVNAMES GROQ_API_KEY LC_ALL OPENCODE_CONFIG PATH\n"),
        "{from_file}"
    );
    let raw = bash_calls(&Sc::new("claude", "oracle").repo_raw(b"re\xffpo"));
    assert!(raw.contains("CWD ") && raw.contains("re\u{fffd}po\n"), "{raw}");
    assert!(opencode.contains("ENV GROQ_API_KEY=len:13 "), "{opencode}");
}
