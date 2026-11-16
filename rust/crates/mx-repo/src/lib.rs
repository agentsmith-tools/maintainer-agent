//! `maintainer-repo`: repository housekeeping a maintainer does after a merge.
//!
//! This crate is the Rust build of `bin/maintainer-repo`. Its three commands
//! produce the bash program's standard output, standard error and exit code
//! byte for byte, make the same `gh` and `git` calls in the same order with the
//! same arguments, and write the same files:
//!
//! - [`prune`]: delete merged branches, local and remote.
//! - [`release`]: say whether a release is due and which digit moves.
//! - [`fixup`]: finish a stalled pull request's mechanical review items.
//!
//! Every process runs through the [`mx_ports::Proc`] port: argument vectors,
//! `PATH` lookup, no shell, an allowlisted environment ([`ctx::child_env`]). The
//! profile comes from [`mx_profile`], which this crate calls and does not copy.
//! `fixup` asks the Python tool `maintainer fixup-check` for every decision, as
//! the bash does (plan decision D1): this crate pins how it is called and how
//! its answer is read, and decides nothing the tool decides.
//!
//! Quirks of the bash program are kept, and each has a test whose name says so
//! and whose comment names the decision: the plan's F5 (the open-pull-request
//! guard of `prune` never fires) and the numbered P1H decisions of the pull
//! request that added this crate. Where the bash does something risky that the
//! plan does not list, the module docs say it under "Found, not fixed".
//!
//! # Reading the outside world
//!
//! A read that can fail is a [`mx_core::Fact`]: a `git merge-base` that could
//! not run is `Unreadable`, and the decision functions in [`prune`] never turn
//! that into "merged". Where the bash itself reads a failure as an empty answer
//! (a failed `gh pr list`), the function takes the `Fact` and the module says
//! what it does with `Unreadable`, so the choice is visible and tested.

pub mod ctx;
pub mod fixup;
pub mod prune;
pub mod release;
pub mod shellish;

use ctx::{Ctx, R, Stop};
use mx_profile::{LoadEnv, load_profile};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// The usage line, on standard error with exit code 64.
pub const USAGE: &str =
    "usage: maintainer-repo {prune [--dry-run]|release-check|fixup <pr> [--dry-run]}";

/// Where the program finds its tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    /// The directory of the program itself, with symlinks resolved. `maintainer`
    /// (the Python tool) lives here.
    pub bin_dir: PathBuf,
    /// The directory holding `profile.sh` and `defaults.env`.
    pub lib_dir: PathBuf,
}

/// What the profile gave: the five names every tool reads, and every other
/// variable the profile and `defaults.env` set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub slug: String,
    pub repo_path: PathBuf,
    pub account: String,
    pub state: String,
    /// `MAINTAINER_POST`, or `on` when it is empty.
    pub post: String,
    tunables: BTreeMap<String, String>,
}

impl Settings {
    /// Settings with the five names and the tunables the profile set. An empty
    /// `post` reads as `on`, as the bash's `${MAINTAINER_POST:-on}` does.
    #[must_use]
    pub fn new(
        slug: &str,
        repo_path: impl Into<PathBuf>,
        account: &str,
        state: &str,
        post: &str,
        tunables: BTreeMap<String, String>,
    ) -> Self {
        Self {
            slug: slug.to_string(),
            repo_path: repo_path.into(),
            account: account.to_string(),
            state: state.to_string(),
            post: if post.is_empty() { "on".to_string() } else { post.to_string() },
            tunables,
        }
    }

    /// A tunable of the profile (`PROD_GLOBS`, `GH_TRIES`, ...): the profile's
    /// value, else the caller's environment, else `None`. Bash reads a variable
    /// the profile did not set from the environment the program was started in.
    #[must_use]
    pub fn tunable(&self, vars: &BTreeMap<String, String>, name: &str) -> Option<String> {
        self.tunables.get(name).or_else(|| vars.get(name)).cloned()
    }

    /// True for a rehearsal: only the exact word `off`.
    #[must_use]
    pub fn rehearsal(&self) -> bool {
        self.post == "off"
    }
}

/// Runs the program with `args` (without the program name) and returns its exit code.
pub fn run(ctx: &mut Ctx<'_>, args: &[OsString]) -> i32 {
    match run_inner(ctx, args) {
        Ok(code) | Err(Stop(code)) => code,
    }
}

fn run_inner(ctx: &mut Ctx<'_>, args: &[OsString]) -> R<i32> {
    let tree = locate(ctx)?;
    let settings = load(ctx, &tree)?;
    let command = args.first().map_or(&b""[..], |arg| arg.as_bytes());
    let rest = args.get(1..).unwrap_or(&[]);
    match command {
        b"prune" => prune::cmd_prune(ctx, &settings, rest),
        b"release-check" => release::cmd_release_check(ctx, &settings),
        b"fixup" => fixup::cmd_fixup(ctx, &settings, &tree, rest),
        _ => {
            ctx.put_err(format!("{USAGE}\n").as_bytes());
            Err(Stop(64))
        }
    }
}

/// `readlink -f` of the program: argv[0] when it holds a slash (as bash's
/// `${BASH_SOURCE[0]}` does), else the running executable.
fn program_path(ctx: &Ctx<'_>) -> PathBuf {
    let given = PathBuf::from(&ctx.argv0);
    let source = if ctx.argv0.as_bytes().contains(&b'/') { given } else { ctx.exe.clone() };
    std::fs::canonicalize(&source).unwrap_or(source)
}

/// Line 15 and 16 of `bin/maintainer-repo`: the two lines bash prints a
/// diagnostic for when `lib/profile.sh` cannot be sourced.
const LINE_SOURCE_SHARE: u32 = 15;
const LINE_LOAD_PROFILE: u32 = 16;

/// Finds `lib/profile.sh` beside the program, else in the share directory.
fn locate(ctx: &mut Ctx<'_>) -> R<Tree> {
    let program = program_path(ctx);
    let bin_dir = program.parent().map(Path::to_path_buf).unwrap_or_default();
    let beside = bin_dir.join("../lib");
    if std::fs::File::open(beside.join("profile.sh"))
        .is_ok_and(|file| file.metadata().is_ok_and(|m| m.is_file()))
    {
        return Ok(Tree { bin_dir, lib_dir: beside });
    }
    let name = ctx.script_name();
    let Some(home) = ctx.vars.get("HOME").cloned() else {
        ctx.put_err(
            format!("{name}: line {LINE_SOURCE_SHARE}: HOME: unbound variable\n").as_bytes(),
        );
        // A failed expansion ends the non-interactive bash with status 1.
        return Err(Stop(1));
    };
    let share = PathBuf::from(format!("{home}/.local/share/maintainer"));
    let shared = share.join("profile.sh");
    match std::fs::File::open(&shared).and_then(|file| file.metadata().map(|m| m.is_file())) {
        Ok(true) => Ok(Tree { bin_dir, lib_dir: share }),
        other => {
            let reason = match other {
                Ok(_) => "Is a directory".to_string(),
                Err(error) => os_text(&error),
            };
            ctx.put_err(
                format!(
                    "{name}: line {LINE_SOURCE_SHARE}: {}: {reason}\n{name}: line {LINE_LOAD_PROFILE}: maintainer_load_profile: command not found\n",
                    shared.display()
                )
                .as_bytes(),
            );
            Err(Stop(64))
        }
    }
}

/// An OS error as `strerror` words it: Rust adds ` (os error N)`.
#[must_use]
pub fn os_text(error: &std::io::Error) -> String {
    let text = error.to_string();
    match text.rfind(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

/// `maintainer_load_profile || exit 64`.
fn load(ctx: &mut Ctx<'_>, tree: &Tree) -> R<Settings> {
    let env = LoadEnv::from_vars(
        tree.lib_dir.clone(),
        ctx.vars.iter().map(|(name, value)| (name.clone(), value.clone())),
    );
    let loaded = load_profile(ctx.proc(), &env);
    ctx.put_out(&loaded.stdout);
    ctx.put_err(&loaded.stderr);
    let profile = match loaded.result {
        Ok(profile) => profile,
        Err(error) => {
            ctx.put_err(&error.message());
            // The function returns 1, and the caller turns that into 64. A
            // status the shell itself ended with (an unbound variable, an
            // `exit` in a sourced file) is the program's own.
            // An unbound variable ends a bash script file with 1 (bash 5.3.9;
            // only `bash -c` gives 127), whatever `LoadError::exit_code` says.
            if matches!(error, mx_profile::LoadError::Unbound { .. }) {
                return Err(Stop(1));
            }
            let code = error.exit_code();
            return Err(Stop(if code == 1 { 64 } else { code }));
        }
    };
    let mut text = |name: &str| -> R<String> {
        match profile.get(name).map(|bytes| std::str::from_utf8(bytes).map(str::to_string)) {
            Some(Ok(value)) => Ok(value),
            Some(Err(_)) => {
                ctx.put_err(
                    format!("maintainer-repo: the profile value {name} is not UTF-8 text\n")
                        .as_bytes(),
                );
                Err(Stop(1))
            }
            None => Ok(String::new()),
        }
    };
    let slug = text("MAINTAINER_SLUG")?;
    let repo_path = text("MAINTAINER_REPO")?;
    let account = text("MAINTAINER_ACCOUNT")?;
    let state = text("MAINTAINER_STATE")?;
    let post = text("MAINTAINER_POST")?;
    // The function exports the five names, so every child sees them. A tunable
    // the profile sets is a shell variable, not an exported one, and stays out.
    for name in profile.exported() {
        if let Some(value) = profile.get_str(name) {
            ctx.exports.push((name.clone(), value.to_string()));
        }
    }
    let tunables = profile
        .vars()
        .map(|(name, value)| (name.to_string(), String::from_utf8_lossy(value).into_owned()))
        .collect();
    Ok(Settings::new(&slug, repo_path, &account, &state, &post, tunables))
}

/// `cd "$REPO_PATH" || die "no repo at $REPO_PATH"`: on success children run in
/// the repository. The bash prints its own `cd` diagnostic first, with the
/// script name and the line of the `cd` (32 in `prune`, 74 in `release-check`).
pub(crate) fn enter_repo(ctx: &mut Ctx<'_>, settings: &Settings, line: u32) -> R<()> {
    match std::fs::metadata(&settings.repo_path) {
        Ok(meta) if meta.is_dir() => {
            ctx.cwd = Some(settings.repo_path.clone());
            Ok(())
        }
        other => {
            let reason = match other {
                Ok(_) => "Not a directory".to_string(),
                Err(error) => os_text(&error),
            };
            let name = ctx.script_name();
            ctx.put_err(
                format!("{name}: line {line}: cd: {}: {reason}\n", settings.repo_path.display())
                    .as_bytes(),
            );
            Err(ctx.die(&format!("no repo at {}", settings.repo_path.display())))
        }
    }
}
