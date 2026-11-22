//! A sandbox for the flow tests: a checkout, a HOME and a search path made of real files
//! in a scratch directory, and a `Proc` that answers every tool the installer starts.
//!
//! The installer's own file work runs for real, inside the scratch directory, and the
//! tools (`systemctl`, `flock`, `git`, `date`, `python3`, `crontab`, `uname`) are scripted:
//! a test shows what it was asked, in order, and what it answered. A call nobody scripted
//! panics, so a guard cannot skip in silence.

#![allow(dead_code)]

use mx_install::console::Console;
use mx_install::env::Env;
use mx_ports::{Proc, ProcError, ProcExit, ProcOutput, ProcSpec};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// What a scripted tool answered.
#[derive(Clone)]
pub struct Answer {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub missing: bool,
}

impl Answer {
    pub fn ok(stdout: &str) -> Self {
        Self { code: 0, stdout: stdout.into(), stderr: String::new(), missing: false }
    }
    pub fn code(code: i32) -> Self {
        Self { code, stdout: String::new(), stderr: String::new(), missing: false }
    }
    pub fn err(code: i32, stderr: &str) -> Self {
        Self { code, stdout: String::new(), stderr: stderr.into(), missing: false }
    }
    pub fn missing() -> Self {
        Self { code: 127, stdout: String::new(), stderr: String::new(), missing: true }
    }
}

/// Answers by the exact words of the call, or by the program name when no exact rule fits.
#[derive(Default)]
pub struct ScriptedProc {
    real: Option<mx_ports::RealProc>,
    exact: Mutex<BTreeMap<String, Answer>>,
    program: Mutex<BTreeMap<String, Answer>>,
    pub calls: Mutex<Vec<String>>,
    pub specs: Mutex<Vec<ProcSpec>>,
}

impl ScriptedProc {
    pub fn new() -> Self {
        let proc = Self {
            real: Some(mx_ports::RealProc::new("/usr/bin:/bin", Vec::new())),
            ..Self::default()
        };
        proc.on_program("uname", Answer::ok("Linux\n"));
        proc.on_program("git", Answer::ok("v9.9.9\n"));
        proc.on_program("date", Answer::ok("2026-01-01T00:00:00+00:00\n"));
        proc.on_program("systemctl", Answer::code(0));
        proc.on_program("flock", Answer::code(0));
        proc.on_program("python3", Answer::ok("  rendered\n"));
        proc.on_program("crontab", Answer::code(0));
        proc.on_program("launchctl", Answer::code(0));
        proc.on_program("notify-send", Answer::code(0));
        proc.on_program("install-cron.sh", Answer::code(0));
        proc
    }

    /// An answer for the call whose words, joined by spaces, are `words`.
    pub fn on(&self, words: &str, answer: Answer) {
        self.exact.lock().unwrap().insert(words.to_string(), answer);
    }

    /// An answer for every call of `program` that has no exact rule.
    pub fn on_program(&self, program: &str, answer: Answer) {
        self.program.lock().unwrap().insert(program.to_string(), answer);
    }

    /// The calls so far, as joined words.
    pub fn log(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// The calls of one program.
    pub fn of(&self, program: &str) -> Vec<String> {
        self.log().into_iter().filter(|c| c.split(' ').next() == Some(program)).collect()
    }
}

impl Proc for ScriptedProc {
    fn run(&self, spec: &ProcSpec) -> Result<ProcOutput, ProcError> {
        let words = spec.argv.as_slice().join(" ");
        self.calls.lock().unwrap().push(words.clone());
        self.specs.lock().unwrap().push(spec.clone());
        let program = spec.argv.first().clone();
        let exact = self.exact.lock().unwrap().get(&words).cloned();
        // A program called by path is a script of the checkout: it answers by its file name.
        let program = if program.starts_with('/') {
            program.rsplit('/').next().unwrap_or("").to_string()
        } else {
            program
        };
        // The child that sources a profile is the real bash unless a test scripted it: it
        // is the shell's own answer that the install reads.
        if exact.is_none()
            && program == "bash"
            && let Some(real) = &self.real
        {
            return real.run(spec);
        }
        let answer = exact.or_else(|| self.program.lock().unwrap().get(&program).cloned());
        let Some(answer) = answer else {
            panic!("unscripted call: {words}");
        };
        if answer.missing {
            return Err(ProcError::NotFound { program });
        }
        Ok(ProcOutput {
            exit: ProcExit::Code(answer.code),
            stdout: answer.stdout.into_bytes(),
            stderr: answer.stderr.into_bytes(),
        })
    }
}

/// Real files in a scratch directory: `co/` is the checkout, `home/` the HOME, `tools/` the
/// search path (an executable file per tool that must be found).
pub struct Sandbox {
    pub dir: PathBuf,
    pub co: PathBuf,
    pub home: PathBuf,
    pub tools: PathBuf,
}

pub fn write(path: &Path, text: &str, mode: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

pub const PROFILE_ENV: &str = "PROFILE_NAME=\"sk\"\nSTATE_DIR=\"${MAINTAINER_STATE_DIR:-$HOME/.local/state/sk-maint}\"\nTASKS=\"review issues\"\nMIN_HOURS_review=6\nMIN_HOURS_issues=36\n";

impl Sandbox {
    /// A checkout with one profile (`sk`, tasks `review` and `issues`) and a template.
    pub fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("mx-install-{tag}-{}", std::process::id()));
        drop(fs::remove_dir_all(&dir));
        let dir = {
            fs::create_dir_all(&dir).unwrap();
            dir.canonicalize().unwrap()
        };
        let (co, home, tools) = (dir.join("co"), dir.join("home"), dir.join("tools"));
        fs::create_dir_all(&home).unwrap();
        for name in [
            "run.sh",
            "prose-style.md",
            "preamble-core.md",
            "profile.sh",
            "failure-shapes.json",
            "defaults.env",
        ] {
            write(
                &co.join("lib").join(name),
                &format!("lib {name}\n"),
                if name == "run.sh" { 0o755 } else { 0o644 },
            );
        }
        write(&co.join("lib/backends/claude.sh"), "backend\n", 0o755);
        for name in ["a.py", "b.py", "render-settings.py"] {
            write(&co.join("scripts").join(name), &format!("print('{name}')\n"), 0o755);
        }
        for name in [
            "maintainer",
            "maintainer-merge",
            "maintainer-doctor",
            "maintainer-repo",
            "maintainer-mcp",
        ] {
            write(
                &co.join("bin").join(name),
                &format!("#!/usr/bin/env bash\necho {name}\n"),
                0o755,
            );
        }
        write(&co.join("profiles/sk/profile.env"), PROFILE_ENV, 0o644);
        write(&co.join("profiles/sk/deny.json"), "{}\n", 0o644);
        write(&co.join("profiles/sk/prompts/review.md"), "review\n", 0o644);
        write(
            &co.join("profiles/_template/profile.env"),
            "PROFILE_NAME=\"__PROFILE__\"\nREPO_SLUG=\"__SLUG__\"\nREPO_PATH=\"${MAINTAINER_REPO_PATH:-__REPO_PATH__}\"\nGH_ACCOUNT=\"__GH_ACCOUNT__\"\nMAINTAINER_NAME=\"__MAINTAINER_NAME__\"\nSTATE_DIR=\"${MAINTAINER_STATE_DIR:-$HOME/.local/state/__PROFILE__-maint}\"\nTASKS=\"review issues\"\n",
            0o644,
        );
        write(
            &co.join("profiles/_template/prompts/review.md"),
            "Review __SLUG__ as __MAINTAINER_NAME__ (__GH_ACCOUNT__).\n",
            0o644,
        );
        write(&co.join("platform/linux/maintainer@.service"), "[Unit]\n", 0o644);
        write(&co.join("platform/linux/maintainer@sk-review.timer"), "[Timer]\n", 0o644);
        write(&co.join("platform/linux/maintainer@sk-issues.timer"), "[Timer]\n", 0o644);
        write(&co.join("platform/linux/run-instance.sh"), "#!/usr/bin/env bash\n", 0o755);
        write(&co.join("platform/linux/alert.sh"), "#!/usr/bin/env bash\n", 0o755);
        write(&co.join("platform/posix/install-cron.sh"), "#!/usr/bin/env bash\n", 0o755);
        for tool in ["flock", "systemctl", "crontab", "launchctl"] {
            write(&tools.join(tool), "#!/bin/sh\n", 0o755);
        }
        Self { dir, co, home, tools }
    }

    pub fn share(&self) -> PathBuf {
        self.home.join(".local/share/maintainer")
    }

    pub fn bin(&self) -> PathBuf {
        self.home.join(".local/bin")
    }

    pub fn units(&self) -> PathBuf {
        self.home.join(".config/systemd/user")
    }

    /// An environment that runs `program` from the checkout with these arguments.
    pub fn env(&self, program: &str, args: &[&str]) -> Env {
        self.env_with(program, args, &[])
    }

    pub fn env_with(&self, program: &str, args: &[&str], extra: &[(&str, &str)]) -> Env {
        let home = self.home.to_string_lossy().into_owned();
        let path = self.tools.to_string_lossy().into_owned();
        let mut vars: Vec<(&str, &str)> = vec![("HOME", &home), ("PATH", &path)];
        vars.extend_from_slice(extra);
        Env::new(&self.co.join(program).to_string_lossy(), args, &home, &vars)
    }

    /// Removes a tool from the search path.
    pub fn drop_tool(&self, tool: &str) {
        fs::remove_file(self.tools.join(tool)).unwrap();
    }

    /// Every path below `dir` with its type, mode and the size of its contents.
    #[allow(clippy::unused_self, reason = "a method reads better beside the other helpers")]
    pub fn listing(&self, dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        walk_listing(dir, dir, &mut out);
        out
    }
}

fn walk_listing(dir: &Path, base: &Path, out: &mut Vec<String>) {
    let mut names: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    names.sort();
    for path in names {
        let meta = fs::symlink_metadata(&path).unwrap();
        let rel = path.strip_prefix(base).unwrap().display().to_string();
        if meta.is_dir() {
            out.push(format!("{rel}/ {:o}", meta.permissions().mode() & 0o7777));
            walk_listing(&path, base, out);
        } else if meta.file_type().is_symlink() {
            out.push(format!("{rel} -> {}", fs::read_link(&path).unwrap().display()));
        } else {
            out.push(format!(
                "{rel} {:o} {}",
                meta.permissions().mode() & 0o7777,
                fs::read(&path).unwrap().len()
            ));
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Directories a test made unreadable would stop the removal.
        make_writable(&self.dir);
        drop(fs::remove_dir_all(&self.dir));
    }
}

fn make_writable(dir: &Path) {
    drop(fs::set_permissions(dir, fs::Permissions::from_mode(0o755)));
    if let Ok(read) = fs::read_dir(dir) {
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() && !fs::symlink_metadata(&path).unwrap().file_type().is_symlink() {
                make_writable(&path);
            }
        }
    }
}

/// The streams of one run.
pub struct Run {
    pub code: i32,
    pub out: String,
    pub err: String,
}

/// Runs a program function with captured streams.
pub fn capture(f: impl FnOnce(&mut Console) -> i32) -> Run {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = {
        let mut con = Console::new(&mut out, &mut err);
        f(&mut con)
    };
    Run {
        code,
        out: String::from_utf8_lossy(&out).into_owned(),
        err: String::from_utf8_lossy(&err).into_owned(),
    }
}

/// The caller's umask.
pub fn umask() -> u32 {
    mx_install::coreutils::umask()
}
