//! What `install.sh` does, as data: facts in, a list of steps out, and no effect.
//!
//! The bash interleaves reading the machine with changing it. Here the reads come first
//! (the probes in [`crate::probe`]) and fill an [`InstallFacts`] or an [`UninstallFacts`];
//! [`decide_install`] and [`decide_uninstall`] turn the facts into a [`Plan`] or a
//! [`Refusal`], and [`crate::apply`] performs the plan through a sink that either does
//! each step or prints `would:`. `--dry-run` therefore runs the code of a real run up to
//! the one place where a step touches the machine.
//!
//! A refusal comes before the first step, so a run that refuses has written nothing
//! (the bash pins that: the check used to sit above the profiles and a refused install
//! had already replaced the rest).

use crate::args::Opts;
use crate::refusal::Refusal;
use crate::sedsub::{self, SedFailure};
use crate::util::{basename, before_last_hyphen, cat, concat, os, parent_name, raw};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// The files of `lib/` that `run.sh` reads, in the order they are copied.
pub const LIB_FILES: [&str; 6] = [
    "run.sh",
    "prose-style.md",
    "preamble-core.md",
    "profile.sh",
    "failure-shapes.json",
    "defaults.env",
];

/// The five commands of `bin/`.
pub const COMMANDS: [&str; 5] =
    ["maintainer", "maintainer-merge", "maintainer-doctor", "maintainer-repo", "maintainer-mcp"];

/// Where the pieces of a deployment go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The checkout the program was started from.
    pub root: PathBuf,
    pub home: OsString,
    /// `$HOME/.local/share/maintainer`.
    pub share: PathBuf,
    /// `$HOME/.local/bin`.
    pub bin: PathBuf,
    /// `$HOME/.config/systemd/user`.
    pub units: PathBuf,
    /// `$HOME/.local/share/systemd/timers`.
    pub stamps: PathBuf,
}

impl Layout {
    #[must_use]
    pub fn new(root: PathBuf, home: &OsStr) -> Self {
        Self {
            root,
            home: home.to_os_string(),
            share: concat(home, "/.local/share/maintainer"),
            bin: concat(home, "/.local/bin"),
            units: concat(home, "/.config/systemd/user"),
            stamps: concat(home, "/.local/share/systemd/timers"),
        }
    }
}

/// One command of the bash, as `run` would execute it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    MkdirP(Vec<PathBuf>),
    Cp {
        src: PathBuf,
        dst: PathBuf,
    },
    CpR {
        src: PathBuf,
        dst: PathBuf,
    },
    RmRf(Vec<PathBuf>),
    RmF(PathBuf),
    ChmodX(Vec<PathBuf>),
    Touch(PathBuf),
    Systemctl(Vec<String>),
    Launchctl(Vec<String>),
    /// `env MAINTAINER_PROFILE=<profile> <script> --remove`.
    CronRemove {
        script: PathBuf,
        profile: OsString,
    },
    /// `--rollback`: put the saved bash original back.
    Mv {
        src: PathBuf,
        dst: PathBuf,
    },
    /// `--rust`: put a prebuilt binary in place (decision P4A-add-1).
    InstallBinary {
        src: PathBuf,
        dst: PathBuf,
    },
}

impl Cmd {
    /// The words of the command, as `$*` joins them in `would: ...` and in the list of
    /// failed steps.
    #[must_use]
    pub fn words(&self) -> Vec<OsString> {
        fn paths(head: &[&str], list: &[PathBuf]) -> Vec<OsString> {
            let mut words: Vec<OsString> = head.iter().map(OsString::from).collect();
            words.extend(list.iter().map(|p| p.as_os_str().to_os_string()));
            words
        }
        match self {
            Self::MkdirP(list) => paths(&["mkdir", "-p"], list),
            Self::Cp { src, dst } => paths(&["cp"], &[src.clone(), dst.clone()]),
            Self::CpR { src, dst } => paths(&["cp", "-r"], &[src.clone(), dst.clone()]),
            Self::RmRf(list) => paths(&["rm", "-rf"], list),
            Self::RmF(path) => paths(&["rm", "-f"], std::slice::from_ref(path)),
            Self::ChmodX(list) => paths(&["chmod", "+x"], list),
            Self::Touch(path) => paths(&["touch"], std::slice::from_ref(path)),
            Self::Systemctl(args) => {
                let mut words = vec![OsString::from("systemctl")];
                words.extend(args.iter().map(OsString::from));
                words
            }
            Self::Launchctl(args) => {
                let mut words = vec![OsString::from("launchctl")];
                words.extend(args.iter().map(OsString::from));
                words
            }
            Self::CronRemove { script, profile } => {
                let mut name = OsString::from("MAINTAINER_PROFILE=");
                name.push(profile);
                vec![
                    OsString::from("env"),
                    name,
                    script.as_os_str().to_os_string(),
                    OsString::from("--remove"),
                ]
            }
            Self::Mv { src, dst } => paths(&["mv"], &[src.clone(), dst.clone()]),
            Self::InstallBinary { src, dst } => {
                paths(&["install", "-m", "755"], &[src.clone(), dst.clone()])
            }
        }
    }

    /// The words joined by single spaces, as bytes.
    #[must_use]
    pub fn text(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (at, word) in self.words().iter().enumerate() {
            if at > 0 {
                out.push(b' ');
            }
            out.extend_from_slice(raw(word));
        }
        out
    }
}

/// What a failing command does to the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnFail {
    /// `set -e`: the run ends with the command's status.
    Abort,
    /// `step`: the failure is kept and listed at the end, and the run goes on.
    Record,
}

/// One thing the run does or says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `say text`, on standard output, in a real run and a dry one.
    Say(Vec<u8>),
    /// A command, through `run`: a dry run prints `would: ...` and nothing else.
    Run(Cmd, OnFail),
    /// The VERSION stamp.
    Stamp,
    /// The receipt-signing key, minted when it is absent.
    MintKey,
    /// The deny walls of every deployed profile that has a `deny.json`.
    RenderWalls,
    /// A real run refuses when no profile landed where `run.sh` looks.
    LandingCheck,
    /// `--rust`: one program replaced by its Rust build, the bash original kept beside it.
    Swap(RustProgram),
    /// `--rust`: the stamp that records what was installed.
    WriteRustStamp(Vec<RustProgram>),
    /// `--timers` found nothing it could enable.
    NoTimersEnabled,
    /// The steps that failed, listed on standard error.
    FailureReport,
    /// A line of "kept: the audit trail", which may carry a sed error.
    KeptLine(Result<Vec<u8>, SedFailure>),
}

/// The ordered steps of one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// The status when every step went through.
    pub exit: i32,
}

/// The answer of a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Refuse(Refusal),
    Proceed(Plan),
}

/// A program that has a Rust build (decision P4A-add-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustProgram {
    /// The file name the Rust build has in the directory given to `--rust-bin`.
    pub build: &'static str,
    /// The deployed file the Rust build replaces.
    pub installed: PathBuf,
    /// Where the bash original is kept: the installed path and `.sh`.
    pub backup: PathBuf,
    /// The Rust build.
    pub source: PathBuf,
    /// Deployed only on Linux.
    pub linux_only: bool,
}

/// A timer unit of the checkout and the profile its name points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timer {
    pub path: PathBuf,
    pub name: OsString,
    pub profile: OsString,
    /// `[ -f "$root/profiles/$profile/profile.env" ]`.
    pub profile_present: bool,
}

/// The profile a timer name stands for: `maintainer@<profile>-<task>.timer`, cut at the
/// last hyphen (`run-instance.sh` splits the same way, because a profile may be named
/// after a hyphenated repository).
#[must_use]
pub fn timer_profile(name: &OsStr) -> OsString {
    let text = raw(name);
    let rest = text.strip_prefix(b"maintainer@").unwrap_or(text);
    os(before_last_hyphen(rest))
}

/// Everything an install decision reads.
#[derive(Debug, Clone)]
pub struct InstallFacts {
    pub layout: Layout,
    pub opts: Opts,
    pub flock_on_path: bool,
    /// Profiles whose run lock is held. Filled only when the lock was asked.
    pub busy: Vec<OsString>,
    /// `uname -s`, without its trailing newline.
    pub uname: String,
    /// `$root/scripts/*.py`.
    pub scripts: Vec<PathBuf>,
    /// `$root/platform/linux/*.service` then `*.timer`, those that exist.
    pub linux_units: Vec<PathBuf>,
    /// `$root/platform/linux/*.timer`, those that exist, with the profile each names.
    pub timers: Vec<Timer>,
    /// The Rust builds `--rust-bin` holds.
    pub rust: Vec<RustProgram>,
}

fn say(text: &str) -> Step {
    Step::Say(text.as_bytes().to_vec())
}

fn run(cmd: Cmd) -> Step {
    Step::Run(cmd, OnFail::Abort)
}

fn cp(src: &Path, dst: &Path) -> Step {
    run(Cmd::Cp { src: src.to_path_buf(), dst: dst.to_path_buf() })
}

fn chmod_x(list: Vec<PathBuf>) -> Step {
    run(Cmd::ChmodX(list))
}

/// Decides what `install.sh` (without `--uninstall`) does.
#[must_use]
pub fn decide_install(f: &InstallFacts) -> Decision {
    if !f.flock_on_path {
        return Decision::Refuse(Refusal::FlockMissing);
    }
    // The lock is asked before the first write. A run in flight reads the files this
    // replaces from start to finish.
    if !f.opts.dry && !f.opts.force && !f.busy.is_empty() {
        return Decision::Refuse(Refusal::RunInFlight {
            share: f.layout.share.as_os_str().to_os_string(),
            profiles: f.busy.clone(),
        });
    }
    let mut steps = deploy_steps(f);
    let timers = platform_steps(f, &mut steps);
    if f.opts.rust {
        rust_steps(f, &mut steps);
    }
    if timers {
        timer_steps(f, &mut steps);
    } else {
        steps.push(say("timers not touched (pass --timers to enable them)"));
    }
    steps.push(say("done"));
    steps.push(say(""));
    steps.push(say(
        "next: maintainer-doctor        # checks the install by running it, not by listing files",
    ));
    if !timers {
        steps.push(say(match f.uname.as_str() {
            "Linux" => "      ./install.sh --timers     # enable the systemd timers",
            "Darwin" => "      platform/macos/install-launchd.sh <profile>",
            _ => "      MAINTAINER_PROFILE=<profile> platform/posix/install-cron.sh   # works anywhere with crontab",
        }));
    }
    Decision::Proceed(Plan { steps, exit: 0 })
}

/// From the banner to the landing check: everything that is the same on every platform.
fn deploy_steps(f: &InstallFacts) -> Vec<Step> {
    let l = &f.layout;
    let lib = l.root.join("lib");
    let mut steps = vec![Step::Say(cat(&[b"deploying from ", raw(l.root.as_os_str())]))];
    steps.push(run(Cmd::MkdirP(vec![l.share.clone(), l.bin.clone(), l.units.clone()])));
    for name in LIB_FILES {
        steps.push(cp(&lib.join(name), &l.share.join(name)));
    }
    steps.push(Step::Stamp);
    steps.push(Step::MintKey);
    steps.push(run(Cmd::MkdirP(vec![l.share.join("scripts")])));
    if f.scripts.is_empty() {
        // No `nullglob`: the loop runs once on the pattern itself.
        let pattern = l.root.join("scripts").join("*.py");
        steps.push(cp(&pattern, &l.share.join("scripts").join("*.py")));
    }
    for script in &f.scripts {
        steps.push(cp(script, &l.share.join("scripts").join(basename(script))));
    }
    steps.push(run(Cmd::RmRf(vec![l.share.join("backends"), l.share.join("profiles")])));
    steps.push(run(Cmd::CpR { src: lib.join("backends"), dst: l.share.join("backends") }));
    steps.push(run(Cmd::CpR { src: l.root.join("profiles"), dst: l.share.join("profiles") }));
    steps.push(run(Cmd::RmRf(vec![l.share.join("profiles").join("_template")])));
    steps.push(Step::RenderWalls);
    for name in COMMANDS {
        steps.push(cp(&l.root.join("bin").join(name), &l.bin.join(name)));
    }
    let mut executable = vec![l.share.join("run.sh")];
    executable.extend(COMMANDS.iter().map(|name| l.bin.join(name)));
    steps.push(chmod_x(executable));
    steps.push(Step::LandingCheck);
    steps
}

/// The core is the same everywhere; only scheduling differs. Returns whether `--timers`
/// still stands after the platform has had its say.
fn platform_steps(f: &InstallFacts, steps: &mut Vec<Step>) -> bool {
    let l = &f.layout;
    let uname = f.uname.as_str();
    if uname == "Linux" {
        for unit in &f.linux_units {
            steps.push(cp(unit, &l.units.join(basename(unit))));
        }
        let linux = l.root.join("platform").join("linux");
        for name in ["run-instance.sh", "alert.sh"] {
            steps.push(cp(&linux.join(name), &l.share.join(name)));
            steps.push(chmod_x(vec![l.share.join(name)]));
        }
        steps.push(say("platform: Linux (systemd user units)"));
        return f.opts.timers;
    }
    if uname == "Darwin" {
        steps.push(say("platform: macOS (launchd)"));
        steps.push(say("run platform/macos/install-launchd.sh to register the agents"));
        steps.push(say("note: launchd has no Persistent=true; a missed run does not catch up"));
    } else if ["MINGW", "MSYS", "CYGWIN"].iter().any(|p| uname.starts_with(p)) {
        steps.push(say("platform: Windows"));
        steps.push(say("files deployed; register the tasks from PowerShell:"));
        steps.push(say("  platform\\windows\\Install-Maintainer.ps1"));
    } else {
        steps.push(say(&format!(
            "platform: {uname} is unrecognised; files deployed, scheduling is yours to wire"
        )));
    }
    false
}

/// `--timers`: reload, then stamp and enable each timer whose profile is in the checkout.
fn timer_steps(f: &InstallFacts, steps: &mut Vec<Step>) {
    let l = &f.layout;
    steps.push(run(Cmd::Systemctl(vec!["--user".into(), "daemon-reload".into()])));
    let mut enabled = 0;
    for timer in &f.timers {
        let name = timer.name.to_string_lossy();
        if !timer.profile_present {
            let mut line = cat(&[b"skipped ", raw(&timer.name), b" (profile "]);
            line.extend_from_slice(raw(&timer.profile));
            line.extend_from_slice(b" is not deployed)");
            steps.push(Step::Say(line));
            continue;
        }
        // The stamp comes first: a fresh Persistent=true timer treats "never run" as a
        // missed slot and fires a catch-up run the instant it is enabled.
        steps.push(run(Cmd::MkdirP(vec![l.stamps.clone()])));
        steps.push(run(Cmd::Touch(l.stamps.join(format!("stamp-{name}")))));
        steps.push(run(Cmd::Systemctl(vec![
            "--user".into(),
            "enable".into(),
            "--now".into(),
            name.to_string(),
        ])));
        steps.push(Step::Say(cat(&[b"enabled ", raw(&timer.name)])));
        enabled += 1;
    }
    if enabled == 0 {
        steps.push(Step::NoTimersEnabled);
    }
}

fn rust_steps(f: &InstallFacts, steps: &mut Vec<Step>) {
    let mut swapped = Vec::new();
    for program in &f.rust {
        if program.linux_only && f.uname != "Linux" {
            steps.push(Step::Say(cat(&[
                b"rust: skipped ",
                program.build.as_bytes(),
                b" (it is deployed on Linux only)",
            ])));
            continue;
        }
        steps.push(Step::Swap(program.clone()));
        swapped.push(program.clone());
    }
    steps.push(Step::WriteRustStamp(swapped));
}

/// Everything an uninstall decision reads.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools, reason = "each bool is one fact the bash reads")]
pub struct UninstallFacts {
    pub layout: Layout,
    pub opts: Opts,
    /// `MAINTAINER_UNINSTALL_YES=1`.
    pub assume_yes: bool,
    /// How many `maintainer@` lines `systemctl --user list-timers` printed, when it was
    /// asked (only a real run without the yes answers asks).
    pub live_timers: Option<usize>,
    /// `systemctl` exists and `systemctl --user show-environment` succeeds.
    pub systemd_user: bool,
    /// `$units/maintainer@*.timer`.
    pub timer_units: Vec<PathBuf>,
    /// `maintainer*.service`, `maintainer@*.timer` and `podman-userns-warmup.service`
    /// under `$units`, those that exist, in the order the loop visits them.
    pub unit_files: Vec<PathBuf>,
    /// `$HOME/.local/share/systemd/timers/stamp-maintainer@*.timer`.
    pub stamps: Vec<PathBuf>,
    pub darwin: bool,
    /// `$HOME/Library/LaunchAgents/dev.maintainer.*.plist`.
    pub plists: Vec<PathBuf>,
    /// `command -v crontab` and an executable `install-cron.sh` in the checkout.
    pub cron_removal_possible: bool,
    pub cron_script: PathBuf,
    /// `$share/profiles/*/profile.env`.
    pub deployed_profiles: Vec<PathBuf>,
    /// Which of the five commands exist in `$bin`.
    pub installed_commands: Vec<PathBuf>,
    /// `$bin/NAME.sh` copies a `--rust` install left (read from the stamp).
    pub rollback_copies: Vec<PathBuf>,
    /// The line of each checkout profile's `STATE_DIR`, as the sed pipeline renders it.
    pub kept: Vec<Result<Vec<u8>, SedFailure>>,
}

/// Decides what `install.sh --uninstall` does.
#[must_use]
pub fn decide_uninstall(f: &UninstallFacts) -> Decision {
    // A live deployment is not removed without being asked twice. A dry run asks nothing
    // and so is never refused.
    if !f.opts.dry && !f.assume_yes {
        let count = f.live_timers.unwrap_or(0);
        if count > 0 {
            return Decision::Refuse(Refusal::LiveTimers { count });
        }
    }
    let l = &f.layout;
    let record = |cmd: Cmd| Step::Run(cmd, OnFail::Record);
    let mut steps = vec![say("removing the maintainer agent")];
    if f.systemd_user {
        for timer in &f.timer_units {
            steps.push(record(Cmd::Systemctl(vec![
                "--user".into(),
                "disable".into(),
                "--now".into(),
                basename(timer).to_string_lossy().into_owned(),
            ])));
        }
    }
    // Removing files does not need a live systemd user session.
    for unit in &f.unit_files {
        steps.push(record(Cmd::RmF(unit.clone())));
    }
    for stamp in &f.stamps {
        steps.push(record(Cmd::RmF(stamp.clone())));
    }
    if f.systemd_user {
        steps.push(run(Cmd::Systemctl(vec!["--user".into(), "daemon-reload".into()])));
    }
    if f.darwin {
        for plist in &f.plists {
            steps.push(run(Cmd::Launchctl(vec![
                "unload".into(),
                plist.to_string_lossy().into_owned(),
            ])));
            steps.push(run(Cmd::RmF(plist.clone())));
        }
    }
    if f.cron_removal_possible {
        for env in &f.deployed_profiles {
            steps.push(record(Cmd::CronRemove {
                script: f.cron_script.clone(),
                profile: parent_name(env),
            }));
        }
    }
    for command in &f.installed_commands {
        steps.push(run(Cmd::RmF(command.clone())));
    }
    for copy in &f.rollback_copies {
        steps.push(run(Cmd::RmF(copy.clone())));
    }
    steps.push(record(Cmd::RmRf(vec![l.share.clone()])));
    steps.push(say(""));
    steps.push(Step::FailureReport);
    steps.push(Step::Say(cat(&[
        b"removed: ",
        raw(l.share.as_os_str()),
        b", the five commands in ",
        raw(l.bin.as_os_str()),
        b", and every scheduler entry.",
    ])));
    steps.push(say("kept:    the audit trail. Delete it yourself if you want it gone:"));
    for line in &f.kept {
        steps.push(Step::KeptLine(line.clone()));
    }
    Decision::Proceed(Plan { steps, exit: 0 })
}

/// What `--rollback` reads.
#[derive(Debug, Clone)]
pub struct RollbackFacts {
    pub layout: Layout,
    pub opts: Opts,
    pub flock_on_path: bool,
    pub busy: Vec<OsString>,
    /// The programs the stamp lists, as `(installed, backup, backup_exists)`.
    pub stamped: Option<Vec<(PathBuf, PathBuf, bool)>>,
}

/// Decides what `--rollback` does: every saved bash original goes back and the stamp goes.
#[must_use]
pub fn decide_rollback(f: &RollbackFacts) -> Decision {
    if !f.flock_on_path {
        return Decision::Refuse(Refusal::FlockMissing);
    }
    if !f.opts.dry && !f.opts.force && !f.busy.is_empty() {
        return Decision::Refuse(Refusal::RunInFlight {
            share: f.layout.share.as_os_str().to_os_string(),
            profiles: f.busy.clone(),
        });
    }
    let stamp = f.layout.share.join("RUST_STAMP");
    let Some(stamped) = &f.stamped else {
        return Decision::Refuse(Refusal::NothingToRollBack { stamp: stamp.into_os_string() });
    };
    // Every backup is checked before the first is moved: a half-restored deployment runs
    // some programs from each build.
    for (_, backup, present) in stamped {
        if !present {
            let mut detail = backup.as_os_str().to_os_string();
            detail.push(" is missing");
            return Decision::Refuse(Refusal::RollbackBlocked { detail });
        }
    }
    let mut steps = vec![Step::Say(cat(&[b"rolling back from ", raw(f.layout.share.as_os_str())]))];
    for (installed, backup, _) in stamped {
        steps.push(run(Cmd::Mv { src: backup.clone(), dst: installed.clone() }));
    }
    steps.push(run(Cmd::RmF(stamp)));
    steps.push(Step::Say(
        format!("rolled back {} program(s) to the bash originals", stamped.len()).into_bytes(),
    ));
    Decision::Proceed(Plan { steps, exit: 0 })
}

/// The "kept" line for one checkout profile: what `grep -m1 '^STATE_DIR=' | sed ...`
/// prints, where the sed script is `s/.*="//; s/"$//; s|\$HOME|<HOME>|`.
pub fn kept_line(state_dir_line: Option<&[u8]>, home: &OsStr) -> Result<Vec<u8>, SedFailure> {
    const PREFIX: &str = "s/.*=\"//; s/\"$//; s|\\$HOME|";
    let replacement = sedsub::parse_replacement(raw(home), 1, PREFIX.len(), b"|")?;
    let Some(line) = state_dir_line else {
        return Ok(Vec::new());
    };
    // `s/.*="//` removes everything up to the last `="`.
    let mut text = match line.windows(2).rposition(|pair| pair == b"=\"") {
        Some(at) => &line[at + 2..],
        None => line,
    };
    if let Some(stripped) = text.strip_suffix(b"\"") {
        text = stripped;
    }
    Ok(sedsub::replace_first(text, b"$HOME", &replacement))
}
