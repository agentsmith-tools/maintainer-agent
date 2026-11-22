//! Helpers shared by the integration tests: a run with fakes, and the answers a fake process gives.
#![allow(dead_code, reason = "each test file uses some of these helpers")]

use mx_checks::ctx::Ctx;
use mx_checks::host::FakeHost;
use mx_ports::{Effect, EffectLog, FakeProc, ProcEffect, ProcExit, ProcOutput, Script};
use std::collections::BTreeMap;

/// What a program printed and returned.
#[derive(Debug)]
pub struct Outcome {
    pub code: i32,
    pub out: String,
    pub err: String,
}

impl Outcome {
    pub fn out_lines(&self) -> Vec<&str> {
        self.out.lines().collect()
    }

    pub fn err_lines(&self) -> Vec<&str> {
        self.err.lines().collect()
    }
}

/// The fakes of one run.
pub struct Rig {
    pub log: EffectLog,
    pub proc: FakeProc,
    pub host: FakeHost,
}

impl Rig {
    pub fn new() -> Self {
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        Self { log, proc, host: FakeHost::new() }
    }

    /// Scripts every call of exactly `argv` to give `answer`.
    pub fn always(&self, argv: &[&str], answer: ProcOutput) {
        self.proc.script(argv, Script::always(Ok(answer)));
    }

    /// Scripts the calls of exactly `argv` to give these answers, one per call.
    pub fn sequence(&self, argv: &[&str], answers: Vec<ProcOutput>) {
        self.proc.script(argv, Script::sequence(answers.into_iter().map(Ok)));
    }

    /// Runs `body` as the program `argv0` with the environment `env`, started in `/w`.
    pub fn run(
        &self,
        argv0: &str,
        env: &[(&str, &str)],
        body: impl FnOnce(&mut Ctx<'_>) -> i32,
    ) -> Outcome {
        let vars: BTreeMap<String, String> =
            env.iter().map(|(name, value)| ((*name).to_string(), (*value).to_string())).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = {
            let mut ctx = Ctx::new(
                &self.proc,
                &self.host,
                argv0.into(),
                "/x/prog".into(),
                vars,
                "/w".into(),
                &mut out,
                &mut err,
            );
            body(&mut ctx)
        };
        Outcome {
            code,
            out: String::from_utf8_lossy(&out).into_owned(),
            err: String::from_utf8_lossy(&err).into_owned(),
        }
    }

    /// The argument vectors of every process call so far, in order, each joined with a space.
    pub fn calls(&self) -> Vec<String> {
        self.log
            .entries()
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::Proc(ProcEffect::Run(spec)) => Some(spec.argv.as_slice().join(" ")),
                _ => None,
            })
            .collect()
    }

    /// The environment variable names of the call that made `argv`.
    pub fn env_names_of(&self, argv_start: &str) -> Vec<String> {
        for effect in self.log.entries() {
            if let Effect::Proc(ProcEffect::Run(spec)) = effect
                && spec.argv.as_slice().join(" ").starts_with(argv_start)
            {
                return spec.env.iter().map(|(name, _)| name.clone()).collect();
            }
        }
        panic!("no call starts with {argv_start}");
    }
}

pub fn exited(code: i32, stdout: &str, stderr: &str) -> ProcOutput {
    ProcOutput {
        exit: ProcExit::Code(code),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

pub fn ok(stdout: &str) -> ProcOutput {
    exited(0, stdout, "")
}

pub fn timed_out() -> ProcOutput {
    ProcOutput { exit: ProcExit::TimedOut, stdout: Vec::new(), stderr: Vec::new() }
}
