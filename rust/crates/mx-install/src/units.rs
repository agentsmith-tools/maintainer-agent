//! `platform/linux/alert.sh` and `platform/linux/run-instance.sh`: the two scripts a
//! systemd unit runs by name.

use crate::alert::{log_line, notification, profile_of, split_instance};
use crate::console::{Console, Exit, Flow};
use crate::env::Env;
use crate::machine::{Machine, glob_affixes};
use crate::tools::{self, QUICK};
use crate::util::{concat, os, raw, words};
use mx_ports::Proc;
use std::ffi::OsStr;
use std::path::PathBuf;

/// `alert.sh INSTANCE`. It always ends with 0: the alert is the last resort and nothing it
/// does may fail the unit that runs it.
pub fn alert(env: &Env, machine: &dyn Machine, proc: &dyn Proc, con: &mut Console) -> i32 {
    match alert_flow(env, machine, proc, con) {
        Ok(()) => 0,
        Err(Exit(code)) => code,
    }
}

fn alert_flow(env: &Env, machine: &dyn Machine, proc: &dyn Proc, con: &mut Console) -> Flow {
    let script = env.argv0.to_string_lossy().into_owned();
    let Some(home) = env.get("HOME") else {
        con.echo_err(format!("{script}: line 22: HOME: unbound variable").as_bytes())?;
        return Err(Exit(1));
    };
    // `${1:-unknown}`: an empty argument is the same as none.
    let instance: Vec<u8> = match env.args.first() {
        Some(arg) if !arg.is_empty() => raw(arg).to_vec(),
        _ => b"unknown".to_vec(),
    };
    let profile = profile_of(&instance);

    // The trail of the profile that failed and no other. When the profile has no directory,
    // every `*-maint` directory gets the line: an alert that lands somewhere is worth more
    // than one that lands nowhere.
    let state = concat(home, "/.local/state");
    let own = state.join(os(&crate::util::cat(&[profile, b"-maint"])));
    let dirs: Vec<u8> = if machine.is_dir(&own) {
        raw(own.as_os_str()).to_vec()
    } else {
        let mut all = glob_affixes(machine, &state, "", "-maint");
        all.sort();
        all.dedup();
        if all.is_empty() {
            raw(state.join("*-maint").as_os_str()).to_vec()
        } else {
            all.iter().flat_map(|p| crate::util::cat(&[raw(p.as_os_str()), b"\n"])).collect()
        }
    };
    // The bash splits `$dirs` on blanks as well as newlines, so a home directory with a
    // space in its name finds no directory and the alert is lost (kept, and listed).
    for word in words(&dirs) {
        let dir = PathBuf::from(os(word));
        if !machine.is_dir(&dir) {
            continue;
        }
        let logs = dir.join("logs");
        let made = machine.exec(&crate::plan::Cmd::MkdirP(vec![logs.clone()]));
        con.err(&made.diag)?;
        // `$(date -Is)` runs once for each directory, after `mkdir`, and a missing `date`
        // says so each time.
        let date = match tools::call(proc, &tools::spec(env, "date", &["-Is"], QUICK)) {
            tools::Ran::Done(output) => {
                con.err(&output.stderr)?;
                crate::util::strip_trailing_newlines(output.stdout)
            }
            tools::Ran::Missing => {
                con.echo_err(format!("{script}: line 31: date: command not found").as_bytes())?;
                Vec::new()
            }
            _ => Vec::new(),
        };
        let file = logs.join("alerts.log");
        if let Err(error) = machine.append(&file, &log_line(&date, &instance)) {
            let text = format!(
                "{script}: line 31: {}: {}",
                file.display(),
                crate::coreutils::strerror(&error)
            );
            con.echo_err(text.as_bytes())?;
        }
    }

    // The log is written either way; only the popup is optional. One setting turns this
    // and run.sh's notifier off together.
    if env.is("MAINTAINER_NOTIFY", "off") {
        return Ok(());
    }
    let (title, body) = notification(&instance);
    // `${DISPLAY:-:1}`: an empty value is the same as none.
    let display = env
        .get("DISPLAY")
        .filter(|d| !d.is_empty())
        .map_or_else(|| ":1".to_string(), |d| d.to_string_lossy().into_owned());
    let bus = env.get("DBUS_SESSION_BUS_ADDRESS").filter(|b| !b.is_empty()).map_or_else(
        || format!("unix:path=/run/user/{}/bus", env.uid),
        |b| b.to_string_lossy().into_owned(),
    );
    let spec = tools::spec(
        env,
        "notify-send",
        &[
            "-u",
            "critical",
            "-a",
            "maintainer",
            &String::from_utf8_lossy(&title),
            &String::from_utf8_lossy(&body),
        ],
        QUICK,
    )
    .env("DISPLAY", display)
    .env("DBUS_SESSION_BUS_ADDRESS", bus);
    // Standard error is discarded and the status is ignored; standard output is not.
    if let tools::Ran::Done(output) = tools::call(proc, &spec) {
        con.out(&output.stdout)?;
    }
    Ok(())
}

/// `run-instance.sh <profile>-<task>`: splits the instance at its last hyphen and runs
/// `run.sh` beside this script. Returns only when it could not start `run.sh`.
pub fn run_instance(env: &Env, con: &mut Console) -> i32 {
    match instance_flow(env, con) {
        Ok(code) | Err(Exit(code)) => code,
    }
}

fn instance_flow(env: &Env, con: &mut Console) -> Flow<i32> {
    let script = env.argv0.to_string_lossy().into_owned();
    let instance = match env.args.first() {
        Some(arg) if !arg.is_empty() => raw(arg).to_vec(),
        _ => {
            con.echo_err(
                format!("{script}: line 5: 1: usage: run-instance.sh <profile>-<task>").as_bytes(),
            )?;
            return Ok(1);
        }
    };
    let (profile, task) = split_instance(&instance);
    // `readlink -f` of this script, then its directory: run.sh is found beside the real file.
    let real = readlink_f(&env.argv0, &env.cwd);
    let dir = real.parent().map_or_else(|| PathBuf::from("."), PathBuf::from);
    let dir = if dir.as_os_str().is_empty() { PathBuf::from(".") } else { dir };
    let run_sh = dir.join("run.sh");
    let shown = run_sh.display();
    match std::fs::metadata(&run_sh) {
        Err(error) => {
            let why = crate::coreutils::strerror(&error);
            con.echo_err(format!("{script}: line 12: {shown}: {why}").as_bytes())?;
            return Ok(127);
        }
        Ok(meta) if meta.is_dir() => {
            con.echo_err(format!("{script}: line 12: {shown}: Is a directory").as_bytes())?;
            return Ok(126);
        }
        Ok(meta) => {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o111 == 0 {
                con.echo_err(format!("{script}: line 12: {shown}: Permission denied").as_bytes())?;
                return Ok(126);
            }
        }
    }
    let args = [os(profile), os(task)];
    // The call replaces this process with run.sh. It closes a descriptor first, and `sh`
    // reads only a single digit there; descriptor 7 is not open in a unit.
    let error = mx_ports::exec_with_descriptor_closed(7, &run_sh, &args, &[]);
    con.echo_err(format!("{script}: line 12: {shown}: {error}").as_bytes())?;
    Ok(126)
}

/// `readlink -f`: the canonical path, and for a file that is not there the canonical
/// parent and the name.
fn readlink_f(path: &OsStr, cwd: &std::path::Path) -> PathBuf {
    let path = cwd.join(path);
    if let Ok(real) = std::fs::canonicalize(&path) {
        return real;
    }
    match (path.parent().and_then(|p| std::fs::canonicalize(p).ok()), path.file_name()) {
        (Some(parent), Some(name)) => parent.join(name),
        _ => PathBuf::new(),
    }
}
