//! A rig for the section tests: a scripted `Proc`, an in-memory host, a fixed
//! clock, and a context built from them. Nothing here touches the machine.
#![allow(dead_code)]

use mx_doctor::ctx::{Ctx, Env, Out, Settings};
use mx_doctor::host::FakeHost;
use mx_doctor::report::{Item, Report};
use mx_ports::{EffectLog, FakeClock, Proc, ProcError, ProcExit, ProcOutput, ProcSpec};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The moment every test starts at: 2026-01-05 12:00:00 UTC.
pub const NOW: u64 = 1_767_614_400;

type Rule = (Vec<String>, Result<ProcOutput, ProcError>);

/// A `Proc` that answers by exact argument vector, records every call with its
/// environment, and panics on a call nobody scripted.
#[derive(Default)]
pub struct ScriptProc {
    rules: Mutex<Vec<Rule>>,
    calls: Mutex<Vec<ProcSpec>>,
    lenient: AtomicBool,
}

fn answer(code: i32, stdout: &str, stderr: &str) -> ProcOutput {
    ProcOutput {
        exit: ProcExit::Code(code),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

impl ScriptProc {
    pub fn on(&self, argv: &[&str], code: i32, stdout: &str) {
        self.on_full(argv, Ok(answer(code, stdout, "")));
    }

    pub fn on_err(&self, argv: &[&str], code: i32, stdout: &str, stderr: &str) {
        self.on_full(argv, Ok(answer(code, stdout, stderr)));
    }

    pub fn missing(&self, argv: &[&str]) {
        self.on_full(argv, Err(ProcError::NotFound { program: argv[0].to_string() }));
    }

    pub fn timed_out(&self, argv: &[&str]) {
        self.on_full(
            argv,
            Ok(ProcOutput { exit: ProcExit::TimedOut, stdout: Vec::new(), stderr: Vec::new() }),
        );
    }

    fn on_full(&self, argv: &[&str], result: Result<ProcOutput, ProcError>) {
        let key: Vec<String> = argv.iter().map(|part| (*part).to_string()).collect();
        let mut rules = self.rules.lock().unwrap();
        rules.retain(|(existing, _)| existing != &key);
        rules.push((key, result));
    }

    /// From now on a call nobody scripted gets an empty success and is recorded,
    /// where it would panic. A test that asserts on what was called uses this, so
    /// a forbidden call fails the assertion instead of the fake.
    pub fn allow_unscripted(&self) {
        self.lenient.store(true, Ordering::Relaxed);
    }

    /// `command -v NAME` resolves to `path`.
    pub fn have(&self, name: &str, path: &str) {
        self.on(&["bash", "-c", "command -v \"$1\"", "_", name], 0, &format!("{path}\n"));
    }

    /// `command -v NAME` finds nothing.
    pub fn lack(&self, name: &str) {
        self.on(&["bash", "-c", "command -v \"$1\"", "_", name], 1, "");
    }

    /// Every call so far, as argument vectors.
    pub fn argvs(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().iter().map(|spec| spec.argv.as_slice().to_vec()).collect()
    }

    /// Calls that are not `command -v` lookups, joined by spaces.
    pub fn tool_calls(&self) -> Vec<String> {
        self.argvs()
            .into_iter()
            .filter(|argv| {
                !(argv.first().is_some_and(|p| p == "bash")
                    && argv.get(2).is_some_and(|s| s.starts_with("command -v")))
            })
            .map(|argv| argv.join(" "))
            .collect()
    }

    /// Every spec, so a test can look at environments and argument vectors.
    pub fn specs(&self) -> Vec<ProcSpec> {
        self.calls.lock().unwrap().clone()
    }
}

impl Proc for ScriptProc {
    fn run(&self, spec: &ProcSpec) -> Result<ProcOutput, ProcError> {
        self.calls.lock().unwrap().push(spec.clone());
        let argv = spec.argv.as_slice();
        for (key, result) in self.rules.lock().unwrap().iter() {
            if key.as_slice() == argv {
                return result.clone();
            }
        }
        if self.lenient.load(Ordering::Relaxed) {
            return Ok(answer(0, "", ""));
        }
        panic!("unscripted call: {argv:?}");
    }
}

/// Everything a section needs.
pub struct Rig {
    pub proc: ScriptProc,
    pub host: FakeHost,
    pub clock: FakeClock,
    pub env: Env,
    pub settings: Settings,
    pub profile: String,
    pub quick: bool,
    pub suite_env: Vec<(String, String)>,
    pub log: EffectLog,
}

pub const HOME: &str = "/home/u";
pub const SHARE: &str = "/home/u/.local/share/maintainer";

pub fn settings(pairs: &[(&str, &str)]) -> Settings {
    let mut vars: BTreeMap<String, String> = [
        ("DENY_WALL_MIN_RULES", "100"),
        ("RED_GRACE_HOURS", "6"),
        ("DOCTOR_RUN_LIMIT", "40"),
        ("DOCTOR_LEASE_HOURS", "12"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
    .collect();
    for (k, v) in pairs {
        vars.insert((*k).to_string(), (*v).to_string());
    }
    Settings::new(vars)
}

impl Rig {
    pub fn new(pairs: &[(&str, &str)]) -> Self {
        let log = EffectLog::new();
        Self {
            proc: ScriptProc::default(),
            host: FakeHost::new(),
            clock: FakeClock::at(NOW, &log),
            env: Env {
                home: HOME.to_string(),
                user: Some("fixture".to_string()),
                tmpdir: Some("/tmp-case".to_string()),
            },
            settings: settings(pairs),
            profile: "magent".to_string(),
            quick: false,
            suite_env: Vec::new(),
            log,
        }
    }

    pub fn ctx(&self) -> Ctx<'_> {
        Ctx {
            proc: &self.proc,
            clock: &self.clock,
            host: &self.host,
            env: &self.env,
            settings: &self.settings,
            profile: &self.profile,
            share: SHARE.to_string(),
            quick: self.quick,
            suite_env: &self.suite_env,
        }
    }

    pub fn pdir(&self) -> String {
        format!("{SHARE}/profiles/{}", self.profile)
    }
}

/// The findings of a run, one string per line the program would print, without colour.
pub fn lines(report: &Report) -> Vec<String> {
    report
        .items()
        .iter()
        .flat_map(|item| {
            Report::render_item(item)
                .replace("\x1b[1m", "")
                .replace("\x1b[0m", "")
                .replace("\x1b[32m", "")
                .replace("\x1b[33m", "")
                .replace("\x1b[31m", "")
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

pub fn out() -> Out {
    Out::default()
}

pub fn has_ok(report: &Report, text: &str) -> bool {
    report.items().contains(&Item::ok(text))
}

pub fn has_warn(report: &Report, text: &str, fix: &str) -> bool {
    report.items().contains(&Item::warn(text, fix))
}

pub fn has_fail(report: &Report, text: &str, fix: &str) -> bool {
    report.items().contains(&Item::fail(text, fix))
}

/// Finds an item by exact message, for tests that assert on it.
pub fn dump(report: &Report) -> String {
    lines(report).join("\n")
}
