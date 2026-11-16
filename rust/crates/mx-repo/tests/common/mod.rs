//! Helpers shared by the integration tests: a scripted process fake, a scratch
//! directory, and a way to run a command with its streams captured.
#![allow(dead_code, reason = "each test file uses some of these helpers")]

use mx_ports::{
    Effect, EffectLog, FakeProc, Proc, ProcEffect, ProcExit, ProcOutput, ProcSpec, Script,
};
use mx_repo::Settings;
use mx_repo::ctx::Ctx;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// A directory under the cargo target directory, removed on drop.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(tag: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let exe = std::env::current_exe().expect("the test binary has a path");
        let base = exe
            .parent()
            .and_then(Path::parent)
            .map_or_else(std::env::temp_dir, |target| target.join("mx-repo-scratch"));
        let dir = base.join(format!("{tag}-{}-{unique}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).expect("the scratch directory can be made");
        Self(dir)
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    pub fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, contents).expect("the scratch file can be written");
        path
    }

    pub fn dir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(&path).expect("the scratch directory can be made");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.0));
    }
}

/// What a command printed and returned.
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
}

/// A scripted process fake and the log of what it was asked.
pub struct Rig {
    pub log: EffectLog,
    pub proc: FakeProc,
}

pub fn output(code: i32, stdout: &str, stderr: &str) -> ProcOutput {
    ProcOutput {
        exit: ProcExit::Code(code),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

impl Rig {
    pub fn new() -> Self {
        let log = EffectLog::new();
        let proc = FakeProc::new(&log);
        Self { log, proc }
    }

    /// The call `argv` exits 0 and prints `stdout`, every time it is made.
    pub fn ok(&self, argv: &[&str], stdout: &str) {
        self.reply(argv, 0, stdout, "");
    }

    /// The call `argv` exits `code`, prints `stdout` and `stderr`, every time.
    pub fn reply(&self, argv: &[&str], code: i32, stdout: &str, stderr: &str) {
        self.proc.script(argv, Script::always(Ok(output(code, stdout, stderr))));
    }

    /// The call `argv` gets these answers, one per call, in order.
    pub fn sequence(&self, argv: &[&str], answers: Vec<ProcOutput>) {
        self.proc.script(argv, Script::sequence(answers.into_iter().map(Ok)));
    }

    /// Every process call so far, as argument vectors.
    pub fn argvs(&self) -> Vec<Vec<String>> {
        self.specs().into_iter().map(|spec| spec.argv.as_slice().to_vec()).collect()
    }

    pub fn specs(&self) -> Vec<ProcSpec> {
        self.log
            .entries()
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::Proc(ProcEffect::Run(spec)) => Some(spec),
                _ => None,
            })
            .collect()
    }

    /// The calls whose first words are `prefix`.
    pub fn calls_starting(&self, prefix: &[&str]) -> Vec<Vec<String>> {
        self.argvs()
            .into_iter()
            .filter(|argv| {
                argv.len() >= prefix.len() && argv.iter().zip(prefix).all(|(a, b)| a == b)
            })
            .collect()
    }

    /// Runs `command` in a context over this rig. `vars` is the caller's environment.
    pub fn run(&self, vars: &[(&str, &str)], command: impl FnOnce(&mut Ctx<'_>) -> i32) -> Outcome {
        Self::run_over(&self.proc, vars, command)
    }

    /// Like `run`, over another process port (one that wraps this rig's fake).
    pub fn run_over(
        proc: &dyn Proc,
        vars: &[(&str, &str)],
        command: impl FnOnce(&mut Ctx<'_>) -> i32,
    ) -> Outcome {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let vars: BTreeMap<String, String> =
            vars.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
        let code = {
            let mut ctx = Ctx::new(
                proc,
                OsString::from("/tree/bin/maintainer-repo"),
                PathBuf::from("/tree/bin/maintainer-repo"),
                vars,
                &mut out,
                &mut err,
            );
            command(&mut ctx)
        };
        Outcome {
            code,
            out: String::from_utf8_lossy(&out).into_owned(),
            err: String::from_utf8_lossy(&err).into_owned(),
        }
    }
}

/// Settings for a repository at `repo`, with `post` as `MAINTAINER_POST`.
pub fn settings(repo: &Path, post: &str) -> Settings {
    Settings::new("o/r", repo, "testuser", "/state", post, BTreeMap::new())
}

/// Settings with tunables.
pub fn settings_with(repo: &Path, state: &str, post: &str, tunables: &[(&str, &str)]) -> Settings {
    let tunables = tunables.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    Settings::new("o/r", repo, "testuser", state, post, tunables)
}

pub fn args(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}
