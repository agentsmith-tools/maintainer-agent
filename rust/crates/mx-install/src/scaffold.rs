//! `new-profile.sh`.

use crate::console::{Console, Exit, Flow};
use crate::env::Env;
use crate::machine::Machine;
use crate::newprofile::{
    Refusal, Substitutions, check_names, has_bad_character, has_placeholder, next_steps,
    path_written, tasks_of, timer_unit,
};
use crate::plan::Cmd;
use crate::util::{cat, os, raw};
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

/// `new-profile.sh <profile> <owner/repo> <path-to-checkout> <gh-account> ["<your name>"]`.
pub fn new_profile(env: &Env, machine: &dyn Machine, con: &mut Console) -> i32 {
    match scaffold(env, machine, con) {
        Ok(code) | Err(Exit(code)) => code,
    }
}

fn refuse(con: &mut Console, refusal: &Refusal) -> Flow<i32> {
    con.err(&refusal.stderr())?;
    Ok(refusal.exit_code())
}

#[allow(
    clippy::too_many_lines,
    reason = "a line-for-line port of one script: the order of its steps is the contract, and splitting it hides that order"
)]
fn scaffold(env: &Env, machine: &dyn Machine, con: &mut Console) -> Flow<i32> {
    let script = env.argv0.to_string_lossy().into_owned();
    let root = env.script_dir();
    if env.args.len() < 4 {
        return refuse(con, &Refusal::Usage);
    }
    let profile = env.args[0].as_os_str();
    let slug = env.args[1].as_os_str();
    let path = env.args[2].as_os_str();
    let account = env.args[3].as_os_str();
    let name: &OsStr = match env.args.get(4) {
        Some(name) if !name.is_empty() => name,
        _ => account,
    };
    let Some(home) = env.get("HOME") else {
        con.echo_err(format!("{script}: line 27: HOME: unbound variable").as_bytes())?;
        return Ok(1);
    };
    let written = path_written(path, home);

    if let Err(refusal) = check_names(profile, slug) {
        return refuse(con, &refusal);
    }
    let target = root.join("profiles").join(profile);
    if machine.is_dir(&target) {
        return refuse(con, &Refusal::Exists(profile.to_os_string()));
    }
    if !machine.is_dir(&PathBuf::from(path).join(".git")) {
        con.echo(&cat(&[b"  warning: ", raw(path), b" is not a git checkout yet"]))?;
    }
    if [profile, slug, path, account, name].iter().any(|v| has_bad_character(v)) {
        return refuse(con, &Refusal::BadCharacter);
    }

    let copied = machine
        .exec(&Cmd::CpR { src: root.join("profiles").join("_template"), dst: target.clone() });
    con.err(&copied.diag)?;
    if copied.status != 0 {
        return Ok(copied.status);
    }
    let files = machine.files_below(&target).unwrap_or_default();
    if !files.is_empty() {
        let values = [raw(profile), raw(slug), written.as_slice(), raw(account), raw(name)];
        let subs = match Substitutions::parse(values) {
            Ok(subs) => subs,
            Err(failure) => {
                con.echo_err(failure.message().as_bytes())?;
                return Ok(1);
            }
        };
        for file in &files {
            let text = match machine.read(file) {
                Ok(text) => text,
                Err(error) => {
                    let line = format!(
                        "sed: can't read {}: {}",
                        file.display(),
                        crate::coreutils::strerror(&error)
                    );
                    con.echo_err(line.as_bytes())?;
                    return Ok(2);
                }
            };
            if let Err(error) = machine.rewrite(file, &subs.apply(&text)) {
                let line = format!(
                    "sed: couldn't open temporary file {}: {}",
                    file.display(),
                    crate::coreutils::strerror(&error)
                );
                con.echo_err(line.as_bytes())?;
                return Ok(4);
            }
        }
    }

    // A placeholder that survived means a template this script does not know: the half-made
    // profile stays and no timer is written.
    let mut survivors: Vec<Vec<u8>> = Vec::new();
    let mut found = false;
    for file in &files {
        let Ok(text) = machine.read(file) else { continue };
        let binary = text.contains(&0);
        for (at, line) in text.split(|b| *b == b'\n').enumerate() {
            if !has_placeholder(line) {
                continue;
            }
            found = true;
            if binary {
                survivors
                    .push(format!("grep: {}: binary file matches", file.display()).into_bytes());
                break;
            }
            survivors.push(cat(&[raw(file.as_os_str()), format!(":{}:", at + 1).as_bytes(), line]));
        }
    }
    if found {
        con.echo_err(b"new-profile: a placeholder survived substitution:")?;
        for line in survivors {
            con.echo_err(&line)?;
        }
        return Ok(1);
    }

    let env_path = target.join("profile.env");
    let tasks = match machine.read(&env_path) {
        Ok(text) => tasks_of(&text),
        Err(error) => {
            let line = format!(
                "sed: can't read {}: {}",
                env_path.display(),
                crate::coreutils::strerror(&error)
            );
            con.echo_err(line.as_bytes())?;
            return Ok(2);
        }
    };
    for (hour, task) in (9_u32..).zip(&tasks) {
        let unit_name = cat(&[b"maintainer@", raw(profile), b"-", task, b".timer"]);
        let unit: PathBuf = root.join("platform").join("linux").join(os(&unit_name));
        if let Err(error) = machine.write(&unit, &timer_unit(profile, task, hour)) {
            let line = format!(
                "{script}: line 70: {}: {}",
                unit.display(),
                crate::coreutils::strerror(&error)
            );
            con.echo_err(line.as_bytes())?;
            return Ok(1);
        }
        let shown = unit.file_name().map_or_else(OsString::new, OsStr::to_os_string);
        con.echo(&cat(&[b"  wrote ", raw(&shown)]))?;
    }
    con.out(&next_steps(profile))?;
    Ok(0)
}
