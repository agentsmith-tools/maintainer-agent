//! Reading the machine into facts, before anything is decided or changed.

use crate::args::Opts;
use crate::env::Env;
use crate::machine::{Machine, glob_affixes, glob_children};
use crate::plan::{
    COMMANDS, InstallFacts, Layout, RollbackFacts, RollbackItem, RustProgram, Timer,
    UninstallFacts, kept_line, timer_profile,
};
use crate::refusal::Refusal;
use crate::tools::{self, QUICK};
use crate::util::{basename, concat, os, strip_trailing_newlines};
use mx_ports::Proc;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// `command -v name`: an executable regular file of that name in a directory of the path.
pub fn on_path(machine: &dyn Machine, env: &Env, name: &str) -> bool {
    env.path_dirs().iter().any(|dir| {
        let candidate = dir.join(name);
        machine.is_file(&candidate) && machine.is_executable(&candidate)
    })
}

/// `uname -s`. A call that fails reads as an empty name, which no platform matches.
pub fn uname(proc: &dyn Proc, env: &Env) -> String {
    let ran = tools::call(proc, &tools::spec(env, "uname", &["-s"], QUICK));
    String::from_utf8_lossy(&strip_trailing_newlines(ran.stdout().to_vec())).into_owned()
}

/// What `. profile.env; printf '%s' "${STATE_DIR:-}"` prints in a subshell that has
/// `set -u` and `pipefail` (a command substitution does not inherit `set -e`). A profile
/// that cannot be read, or that stops before it sets `STATE_DIR`, gives the empty string,
/// and the busy check then skips it: the check fails open for a profile it cannot read.
pub fn state_dir_of(proc: &dyn Proc, env: &Env, profile_env: &Path) -> Vec<u8> {
    const SCRIPT: &str =
        "set -u; set -o pipefail; . \"$1\" >/dev/null 2>&1; printf '%s' \"${STATE_DIR:-}\"";
    let spec =
        tools::spec(env, "bash", &["-c", SCRIPT, "bash", &profile_env.to_string_lossy()], QUICK);
    let ran = tools::call(proc, &spec);
    strip_trailing_newlines(ran.stdout().to_vec())
}

/// The profiles of the checkout whose run lock another process holds, in glob order.
/// The lock is asked, not guessed from a process list: `flock -n LOCK true`.
pub fn busy_profiles(
    machine: &dyn Machine,
    proc: &dyn Proc,
    env: &Env,
    layout: &Layout,
) -> Vec<OsString> {
    let mut busy = Vec::new();
    for profile_env in glob_children(machine, &layout.root.join("profiles"), "profile.env") {
        if !machine.is_file(&profile_env) {
            continue;
        }
        let state = state_dir_of(proc, env, &profile_env);
        if state.is_empty() {
            continue;
        }
        let lock = concat(&os(&state), "/run.lock");
        if !machine.exists(&lock) {
            continue;
        }
        let ran = tools::call(
            proc,
            &tools::spec(env, "flock", &["-n", &lock.to_string_lossy(), "true"], QUICK),
        );
        if ran.succeeded() {
            continue;
        }
        busy.push(crate::util::parent_name(&profile_env));
    }
    busy
}

/// The Rust builds: file name in the directory of `--rust-bin`, installed place, and
/// whether the place is a command or a file of the share directory.
const RUST_TABLE: [(&str, bool, &str, bool); 6] = [
    ("maintainer-merge", true, "maintainer-merge", false),
    ("maintainer-doctor", true, "maintainer-doctor", false),
    ("maintainer-repo", true, "maintainer-repo", false),
    ("run", false, "run.sh", false),
    ("run-instance", false, "run-instance.sh", true),
    ("alert", false, "alert.sh", true),
];

/// Every program that can have a Rust build, with the paths it would use under `layout`.
#[must_use]
pub fn rust_candidates(layout: &Layout, dir: &Path) -> Vec<RustProgram> {
    RUST_TABLE
        .iter()
        .map(|(build, in_bin, file, linux_only)| {
            let installed = if *in_bin { layout.bin.join(file) } else { layout.share.join(file) };
            let mut backup = installed.as_os_str().to_os_string();
            backup.push(".sh");
            RustProgram {
                build,
                installed,
                backup: PathBuf::from(backup),
                source: dir.join(build),
                linux_only: *linux_only,
            }
        })
        .collect()
}

/// The Rust builds `dir` holds, or why it cannot be used.
pub fn rust_builds(
    machine: &dyn Machine,
    layout: &Layout,
    dir: &Path,
) -> Result<Vec<RustProgram>, Refusal> {
    let unusable = |detail: String| Refusal::RustBinUnusable { detail: OsString::from(detail) };
    if !machine.is_dir(dir) {
        return Err(unusable(format!("{} is not a directory", dir.display())));
    }
    let mut found = Vec::new();
    for program in rust_candidates(layout, dir) {
        if !machine.lexists(&program.source) {
            continue;
        }
        let usable = machine.is_file(&program.source)
            && machine.is_executable(&program.source)
            && machine.read(&program.source).is_ok_and(|bytes| !bytes.is_empty());
        if !usable {
            return Err(unusable(format!(
                "{} is not an executable file with content",
                program.source.display()
            )));
        }
        found.push(program);
    }
    if found.is_empty() {
        let names: Vec<&str> = RUST_TABLE.iter().map(|row| row.0).collect();
        return Err(unusable(format!("none of {} is in {}", names.join(", "), dir.display())));
    }
    Ok(found)
}

/// Everything an install reads. A `--rust` directory that cannot be used is a refusal.
pub fn install_facts(
    machine: &dyn Machine,
    proc: &dyn Proc,
    env: &Env,
    layout: Layout,
    opts: Opts,
) -> Result<InstallFacts, Refusal> {
    let flock_on_path = on_path(machine, env, "flock");
    // The lock is asked only when it can matter: a missing flock refuses first, and a
    // dry run and a forced run never look.
    let busy = if flock_on_path && !opts.dry && !opts.force {
        busy_profiles(machine, proc, env, &layout)
    } else {
        Vec::new()
    };
    let rust = match (&opts.rust, &opts.rust_bin) {
        (true, Some(dir)) => rust_builds(machine, &layout, dir)?,
        _ => Vec::new(),
    };
    let linux = layout.root.join("platform").join("linux");
    let exists = |paths: Vec<PathBuf>| -> Vec<PathBuf> {
        paths.into_iter().filter(|path| machine.exists(path)).collect()
    };
    let mut linux_units = exists(glob_affixes(machine, &linux, "", ".service"));
    let timer_paths = exists(glob_affixes(machine, &linux, "", ".timer"));
    linux_units.extend(timer_paths.iter().cloned());
    let timers = timer_paths
        .into_iter()
        .map(|path| {
            let name = basename(&path);
            let profile = timer_profile(&name);
            let env_file = layout.root.join("profiles").join(&profile).join("profile.env");
            Timer { profile_present: machine.is_file(&env_file), path, name, profile }
        })
        .collect();
    let rust_stamp = Some(layout.share.join("RUST_STAMP")).filter(|stamp| machine.is_file(stamp));
    let rust_leftovers = if rust_stamp.is_some() {
        rust_candidates(&layout, Path::new(""))
            .into_iter()
            .map(|program| program.backup)
            .filter(|backup| machine.lexists(backup) && !machine.is_symlink(backup))
            .collect()
    } else {
        Vec::new()
    };
    // The saved original goes to a name this installer invents. It must not be a link, which
    // `cp` would write through, nor a file of the user's: only a regular file an earlier
    // `--rust` install left (the stamp says there was one) may be replaced.
    for program in &rust {
        let backup = &program.backup;
        let ours = rust_stamp.is_some() && machine.is_file(backup) && !machine.is_symlink(backup);
        if machine.lexists(backup) && !ours {
            return Err(Refusal::RustBinUnusable {
                detail: OsString::from(format!(
                    "{} exists and is not a saved original of this installer; move it away first",
                    backup.display()
                )),
            });
        }
    }
    Ok(InstallFacts {
        rust_stamp,
        rust_leftovers,
        scripts: exists(glob_affixes(machine, &layout.root.join("scripts"), "", ".py")),
        linux_units,
        timers,
        uname: uname(proc, env),
        flock_on_path,
        busy,
        rust,
        layout,
        opts,
    })
}

/// The `maintainer@` lines `systemctl --user list-timers` prints.
fn live_timer_count(proc: &dyn Proc, env: &Env) -> usize {
    let ran = tools::call(
        proc,
        &tools::spec(
            env,
            "systemctl",
            &["--user", "list-timers", "maintainer@*", "--no-pager"],
            QUICK,
        ),
    );
    ran.stdout()
        .split(|byte| *byte == b'\n')
        .filter(|line| line.windows(11).any(|window| window == b"maintainer@"))
        .count()
}

/// `command -v systemctl && systemctl --user show-environment`.
fn systemd_user_session(machine: &dyn Machine, proc: &dyn Proc, env: &Env) -> bool {
    on_path(machine, env, "systemctl")
        && tools::call(proc, &tools::spec(env, "systemctl", &["--user", "show-environment"], QUICK))
            .succeeded()
}

/// Everything an uninstall reads, in the order the bash asks: the guard first, and
/// nothing else when it refuses.
pub fn uninstall_facts(
    machine: &dyn Machine,
    proc: &dyn Proc,
    env: &Env,
    layout: Layout,
    opts: Opts,
) -> Result<UninstallFacts, Refusal> {
    let assume_yes = env.is("MAINTAINER_UNINSTALL_YES", "1");
    let mut live_timers = None;
    if !opts.dry && !assume_yes {
        let count =
            if systemd_user_session(machine, proc, env) { live_timer_count(proc, env) } else { 0 };
        if count > 0 {
            return Err(Refusal::LiveTimers { count });
        }
        live_timers = Some(count);
    }
    let systemd_user = systemd_user_session(machine, proc, env);
    let exists = |paths: Vec<PathBuf>| -> Vec<PathBuf> {
        paths.into_iter().filter(|path| machine.exists(path)).collect()
    };
    let units = &layout.units;
    let timer_units = exists(glob_affixes(machine, units, "maintainer@", ".timer"));
    let mut unit_files = exists(glob_affixes(machine, units, "maintainer", ".service"));
    unit_files.extend(timer_units.iter().cloned());
    unit_files.extend(exists(vec![units.join("podman-userns-warmup.service")]));
    let stamps = exists(glob_affixes(machine, &layout.stamps, "stamp-maintainer@", ".timer"));
    let darwin = uname(proc, env) == "Darwin";
    let plists = exists(glob_affixes(
        machine,
        &concat(&layout.home, "/Library/LaunchAgents"),
        "dev.maintainer.",
        ".plist",
    ));
    let cron_script = layout.root.join("platform").join("posix").join("install-cron.sh");
    let cron_removal_possible =
        on_path(machine, env, "crontab") && machine.is_executable(&cron_script);
    let deployed_profiles =
        exists(glob_children(machine, &layout.share.join("profiles"), "profile.env"));
    let installed_commands: Vec<PathBuf> = COMMANDS
        .iter()
        .map(|name| layout.bin.join(name))
        .filter(|path| machine.exists(path))
        .collect();
    let rollback_copies = if machine.is_file(&layout.share.join("RUST_STAMP")) {
        rust_candidates(&layout, Path::new(""))
            .into_iter()
            .filter(|program| program.installed.starts_with(&layout.bin))
            .map(|program| program.backup)
            .filter(|path| machine.lexists(path) && !machine.is_symlink(path))
            .collect()
    } else {
        Vec::new()
    };
    let kept = exists(glob_children(machine, &layout.root.join("profiles"), "profile.env"))
        .iter()
        .map(|profile_env| {
            let line = machine.read(profile_env).ok().and_then(|text| {
                text.split(|b| *b == b'\n')
                    .find(|l| l.starts_with(b"STATE_DIR="))
                    .map(<[u8]>::to_vec)
            });
            kept_line(line.as_deref(), &layout.home)
        })
        .collect();
    Ok(UninstallFacts {
        layout,
        opts,
        assume_yes,
        live_timers,
        systemd_user,
        timer_units,
        unit_files,
        stamps,
        darwin,
        plists,
        cron_removal_possible,
        cron_script,
        deployed_profiles,
        installed_commands,
        rollback_copies,
        kept,
    })
}

/// What `--rollback` reads: the stamp, and for each program it lists whether the saved
/// bash original is there. A stamp that names a path this installer does not swap is a
/// refusal: a rollback must not move a file a tampered stamp points it at.
pub fn rollback_facts(
    machine: &dyn Machine,
    proc: &dyn Proc,
    env: &Env,
    layout: Layout,
    opts: Opts,
) -> Result<RollbackFacts, Refusal> {
    let flock_on_path = on_path(machine, env, "flock");
    let busy = if flock_on_path && !opts.dry && !opts.force {
        busy_profiles(machine, proc, env, &layout)
    } else {
        Vec::new()
    };
    let stamp_path = layout.share.join("RUST_STAMP");
    let stamped = match machine.read(&stamp_path) {
        Err(_) if !machine.lexists(&stamp_path) => None,
        Err(error) => {
            let detail = format!("{} cannot be read: {error}", stamp_path.display());
            return Err(Refusal::RollbackBlocked { detail: OsString::from(detail) });
        }
        Ok(text) => Some(parse_rust_stamp(&text, &layout)?),
    };
    Ok(RollbackFacts {
        stamped: stamped.map(|rows| {
            rows.into_iter()
                .map(|(installed, backup, stamp_hash)| RollbackItem {
                    installed_hash: machine.sha256(&installed).ok(),
                    backup_hash: machine.sha256(&backup).ok(),
                    installed,
                    backup,
                    stamp_hash,
                })
                .collect()
        }),
        flock_on_path,
        busy,
        layout,
        opts,
    })
}

/// The `(installed, backup)` pairs a stamp lists: lines of `<64 hex>  <path>`.
pub fn parse_rust_stamp(
    text: &[u8],
    layout: &Layout,
) -> Result<Vec<(PathBuf, PathBuf, String)>, Refusal> {
    let known = rust_candidates(layout, Path::new(""));
    let blocked = |detail: String| Refusal::RollbackBlocked { detail: OsString::from(detail) };
    let mut pairs = Vec::new();
    for line in text.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let shape_ok = line.len() > 66
            && line[..64].iter().all(u8::is_ascii_hexdigit)
            && &line[64..66] == b"  ";
        if !shape_ok {
            return Err(blocked(format!(
                "RUST_STAMP has a line that is not `<sha256>  <path>`: {}",
                String::from_utf8_lossy(line)
            )));
        }
        let path = PathBuf::from(os(&line[66..]));
        let Some(program) = known.iter().find(|program| program.installed == path) else {
            return Err(blocked(format!(
                "RUST_STAMP lists {}, which is not a program this installer swaps",
                path.display()
            )));
        };
        pairs.push((
            program.installed.clone(),
            program.backup.clone(),
            String::from_utf8_lossy(&line[..64]).to_ascii_lowercase(),
        ));
    }
    if pairs.is_empty() {
        return Err(blocked("RUST_STAMP lists no program".to_string()));
    }
    Ok(pairs)
}
