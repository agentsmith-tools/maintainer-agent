//! `platform/posix/install-cron.sh` and `platform/macos/install-launchd.sh`.

use crate::console::{Console, Exit, Flow};
use crate::cron;
use crate::env::Env;
use crate::launchd::{Plist, loaded_line};
use crate::machine::Machine;
use crate::sourced::{self, Mode, Sourced, Stopped};
use crate::tools::{self, COMMAND, Ran};
use crate::util::{cat, os, raw, words};
use mx_ports::{Proc, ProcSpec};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// `$(cd "$share/profiles" 2>/dev/null && printf '%s ' */ | tr -d /)`: the names of the
/// deployed profiles, each followed by a space. A `profiles` directory with no
/// subdirectory prints the pattern itself, `* `.
fn deployed_names(machine: &dyn Machine, share: &Path) -> Vec<u8> {
    let dir = share.join("profiles");
    let Ok(names) = machine.dir_names(&dir) else { return Vec::new() };
    let mut dirs: Vec<Vec<u8>> = names
        .iter()
        .filter(|name| !raw(name).starts_with(b".") && machine.is_dir(&dir.join(name)))
        .map(|name| raw(name).to_vec())
        .collect();
    if dirs.is_empty() {
        return b"* ".to_vec();
    }
    dirs.sort();
    dirs.iter().flat_map(|name| cat(&[name, b" "])).collect()
}

fn bash_error(con: &mut Console, env: &Env, line: u32, what: &str) -> Flow {
    let text = format!("{}: line {line}: {what}", env.argv0.to_string_lossy());
    con.echo_err(text.as_bytes())
}

/// Reports a profile the shell stopped on, and ends with its status.
fn stopped(con: &mut Console, ended: &Stopped) -> Exit {
    // The run ends with the profile's status; a closed pipe cannot change that.
    let _ = con.out(&ended.printed);
    let _ = con.err(&ended.stderr);
    Exit(ended.code)
}

fn finish(result: Flow<i32>) -> i32 {
    match result {
        Ok(code) | Err(Exit(code)) => code,
    }
}

// --- install-cron.sh ------------------------------------------------------------------

/// `install-cron.sh [--dry-run|--remove]`.
pub fn install_cron(env: &Env, machine: &dyn Machine, proc: &dyn Proc, con: &mut Console) -> i32 {
    finish(cron_flow(env, machine, proc, con))
}

#[allow(
    clippy::too_many_lines,
    reason = "a line-for-line port of one script: the order of its steps is the contract, and splitting it hides that order"
)]
fn cron_flow(env: &Env, machine: &dyn Machine, proc: &dyn Proc, con: &mut Console) -> Flow<i32> {
    let Some(home) = env.get("HOME") else {
        bash_error(con, env, 26, "HOME: unbound variable")?;
        return Ok(1);
    };
    let share = crate::util::concat(home, "/.local/share/maintainer");
    let profile = env.get("MAINTAINER_PROFILE").filter(|p| !p.is_empty());
    let Some(profile) = profile else {
        con.echo_err(b"install-cron: no profile name given. Set MAINTAINER_PROFILE=<name>.")?;
        con.echo_err(&cat(&[b"              deployed here: ", &deployed_names(machine, &share)]))?;
        return Ok(64);
    };
    let marker = cron::marker(profile);
    // `${1:-install}`: an empty word is the same as none.
    let verb = match env.args.first() {
        Some(word) if !word.is_empty() => word.as_os_str(),
        _ => OsStr::new("install"),
    };

    let run_sh = share.join("run.sh");
    if !machine.is_executable(&run_sh) {
        let text = format!("install-cron: {} not found; run ./install.sh first", run_sh.display());
        con.echo_err(text.as_bytes())?;
        return Ok(1);
    }
    if !crate::probe::on_path(machine, env, "crontab") {
        con.echo_err(b"install-cron: no crontab on this system")?;
        return Ok(1);
    }
    let profile_env = share.join("profiles").join(profile).join("profile.env");
    let sourced = match sourced::source(proc, env, &profile_env, Mode::Cron) {
        Ok(sourced) => sourced,
        Err(ended) => return Err(stopped(con, &ended)),
    };
    con.out(&sourced.printed)?;
    if sourced.status != 0 || !machine.exists(&profile_env) {
        let text = cat(&[b"install-cron: no deployed profile '", raw(profile), b"'"]);
        con.echo_err(&text)?;
        return Ok(1);
    }

    let list = tools::call(proc, &tools::spec(env, "crontab", &["-l"], tools::QUICK));
    let current: Vec<u8> = match &list {
        Ran::Done(output) if output.exit == mx_ports::ProcExit::Code(0) => {
            crate::util::strip_trailing_newlines(
                output.stdout.iter().copied().filter(|b| *b != 0).collect(),
            )
        }
        Ran::Done(output) if contains(&output.stderr, b"no crontab for") => Vec::new(),
        other => {
            if let Ran::Done(output) = other {
                con.err(&output.stderr)?;
            }
            con.echo_err(b"install-cron: crontab -l failed; leaving the crontab unchanged")?;
            return Ok(1);
        }
    };
    let stripped = cron::strip_block(&current, &marker);

    // The block is made one piece at a time and the variables are read when they are needed,
    // so a profile that leaves one unset has produced what came before it.
    let mut fault: Option<(u32, &str)> = None;
    let mut block = cat(&[&marker, b"\n", b"PATH=", &cron::cron_path(home), b"\n"]);
    match (&sourced.state_dir, &sourced.tasks) {
        (None, _) => fault = Some((56, "STATE_DIR: unbound variable")),
        (Some(_), None) => fault = Some((60, "TASKS: unbound variable")),
        (Some(state_dir), Some(tasks)) => {
            block = cron::generate(home, share.as_os_str(), profile, &words(tasks), state_dir);
        }
    }

    match verb.to_string_lossy().as_ref() {
        "--dry-run" => {
            con.echo(b"--- crontab that would be installed ---")?;
            con.out(&block)?;
            if let Some((line, what)) = fault {
                bash_error(con, env, line, what)?;
                return Ok(1);
            }
            Ok(0)
        }
        "--remove" => {
            let status = write_crontab(proc, env, con, &cat(&[&stripped, b"\n"]))?;
            if status != 0 {
                return Ok(status);
            }
            con.say(b"maintainer entries removed")?;
            Ok(0)
        }
        "install" => {
            let Some(state_dir) = &sourced.state_dir else {
                bash_error(con, env, 99, "STATE_DIR: unbound variable")?;
                return Ok(1);
            };
            let logs = PathBuf::from(os(&cat(&[state_dir, b"/logs"])));
            let created = machine.exec(&crate::plan::Cmd::MkdirP(vec![logs]));
            con.err(&created.diag)?;
            if created.status != 0 {
                return Ok(created.status);
            }
            // `{ printf '%s\n' "$stripped"; gen; } | crontab -`: `gen` runs in a subshell of
            // the pipeline, so a profile with no TASKS ends that subshell with its message,
            // crontab still receives what came before it (a block with no closing line), and
            // the status is crontab's. The count below runs `gen` again and says it again.
            let status = write_crontab(proc, env, con, &cat(&[&stripped, b"\n", &block]))?;
            if let Some((line, what)) = fault {
                bash_error(con, env, line, what)?;
            }
            if status != 0 {
                return Ok(status);
            }
            if let Some((line, what)) = fault {
                bash_error(con, env, line, what)?;
            }
            let count = cron::count_run_sh(&block);
            con.say(
                format!(
                    "installed {count} cron entries for profile '{}'",
                    profile.to_string_lossy()
                )
                .as_bytes(),
            )?;
            con.say(b"inspect with: crontab -l")?;
            con.say(b"note: cron does not catch up a missed run; see the header of this script")?;
            Ok(0)
        }
        _ => {
            con.echo_err(b"usage: install-cron.sh [--dry-run|--remove]")?;
            Ok(64)
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// `... | crontab -`: the status of the pipeline is the status of `crontab`.
fn write_crontab(proc: &dyn Proc, env: &Env, con: &mut Console, input: &[u8]) -> Flow<i32> {
    let spec: ProcSpec = tools::spec(env, "crontab", &["-"], COMMAND).stdin(input.to_vec());
    match tools::call(proc, &spec) {
        Ran::Done(output) => {
            con.out(&output.stdout)?;
            con.err(&output.stderr)?;
            Ok(Ran::Done(output).status())
        }
        other => Ok(other.status()),
    }
}

// --- install-launchd.sh -----------------------------------------------------------------

/// `install-launchd.sh [--remove] [PROFILE]`.
pub fn install_launchd(
    env: &Env,
    machine: &dyn Machine,
    proc: &dyn Proc,
    con: &mut Console,
) -> i32 {
    finish(launchd_flow(env, machine, proc, con))
}

#[allow(
    clippy::too_many_lines,
    reason = "a line-for-line port of one script: the order of its steps is the contract, and splitting it hides that order"
)]
fn launchd_flow(env: &Env, machine: &dyn Machine, proc: &dyn Proc, con: &mut Console) -> Flow<i32> {
    let Some(home) = env.get("HOME") else {
        bash_error(con, env, 18, "HOME: unbound variable")?;
        return Ok(1);
    };
    let share = crate::util::concat(home, "/.local/share/maintainer");
    let agents = crate::util::concat(home, "/Library/LaunchAgents");
    let mut args = env.args.iter();
    let mut remove = false;
    let mut first = args.next();
    if first.is_some_and(|a| a == "--remove") {
        remove = true;
        first = args.next();
    }
    let from_env = env.get("MAINTAINER_PROFILE").filter(|p| !p.is_empty());
    let profile = first.map(std::ffi::OsString::as_os_str).filter(|p| !p.is_empty()).or(from_env);
    let Some(profile) = profile else {
        con.echo_err(b"install-launchd: no profile name given. Pass it as an argument or set MAINTAINER_PROFILE.")?;
        con.echo_err(&cat(&[
            b"                 deployed here: ",
            &deployed_names(machine, &share),
        ]))?;
        return Ok(64);
    };
    let label_prefix = cat(&[b"dev.maintainer.", raw(profile)]);
    let env_file = share.join("profiles").join(profile).join("profile.env");
    if !machine.is_file(&env_file) {
        let text = cat(&[
            b"install-launchd: no deployed profile '",
            raw(profile),
            b"'; run ./install.sh first",
        ]);
        con.echo_err(&text)?;
        return Ok(1);
    }
    let sourced: Sourced = match sourced::source(proc, env, &env_file, Mode::Launchd) {
        Ok(sourced) => sourced,
        Err(ended) => return Err(stopped(con, &ended)),
    };
    con.out(&sourced.printed)?;
    con.err(&sourced.stderr)?;
    let Some(state_dir) = &sourced.state_dir else {
        bash_error(con, env, 35, "STATE_DIR: unbound variable")?;
        return Ok(1);
    };
    let created = machine.exec(&crate::plan::Cmd::MkdirP(vec![
        PathBuf::from(&agents),
        PathBuf::from(os(&cat(&[state_dir, b"/logs"]))),
    ]));
    con.err(&created.diag)?;
    if created.status != 0 {
        return Ok(created.status);
    }
    if remove {
        let prefix = cat(&[&label_prefix, b"."]);
        let found = crate::machine::glob(machine, Path::new(&agents), &|name| {
            name.len() >= prefix.len() + 6 && name.starts_with(&prefix) && name.ends_with(b".plist")
        });
        for path in found.into_iter().filter(|p| machine.exists(p)) {
            drop(tools::call(
                proc,
                &tools::spec(env, "launchctl", &["unload", &path.to_string_lossy()], tools::QUICK),
            ));
            let removed = machine.exec(&crate::plan::Cmd::RmF(path.clone()));
            con.err(&removed.diag)?;
            if removed.status != 0 {
                return Ok(removed.status);
            }
            let name = crate::util::basename(&path);
            con.echo(&cat(&[b"  removed ", raw(&name)]))?;
        }
        return Ok(0);
    }
    let Some(tasks) = &sourced.tasks else {
        bash_error(con, env, 48, "TASKS: unbound variable")?;
        return Ok(1);
    };
    let _ = tasks;
    for (hour, (task, minimum)) in (9_u32..).zip(&sourced.minimums) {
        let label = cat(&[&label_prefix, b".", task]);
        let file = PathBuf::from(&agents).join(os(&cat(&[&label, b".plist"])));
        // `cat > "$f" <<PLIST` opens the file before the text is expanded, so a profile with
        // no REPO_PATH leaves an empty plist behind and ends the script.
        let Some(repo_path) = &sourced.repo_path else {
            if let Err(error) = machine.write(&file, b"") {
                let why = crate::coreutils::strerror(&error);
                bash_error(con, env, 46, &format!("{}: {why}", file.display()))?;
                return Ok(1);
            }
            bash_error(con, env, 51, "REPO_PATH: unbound variable")?;
            return Ok(1);
        };
        let text = Plist {
            label: &label,
            share: share.as_os_str(),
            profile,
            task,
            repo_path,
            home,
            hour,
            state_dir,
        }
        .render();
        if let Err(error) = machine.write(&file, &text) {
            let line = format!(
                "{}: line 46: {}: {}",
                env.argv0.to_string_lossy(),
                file.display(),
                crate::coreutils::strerror(&error)
            );
            con.echo_err(line.as_bytes())?;
            return Ok(1);
        }
        drop(tools::call(
            proc,
            &tools::spec(env, "launchctl", &["unload", &file.to_string_lossy()], tools::QUICK),
        ));
        let status = tools::run_visible(
            proc,
            con,
            &tools::spec(env, "launchctl", &["load", &file.to_string_lossy()], COMMAND),
            &env.argv0.to_string_lossy(),
            78,
        )?;
        if status != 0 {
            return Ok(status);
        }
        let Some(minimum) = minimum else {
            let name = format!("MIN_HOURS_{}", String::from_utf8_lossy(task));
            bash_error(con, env, 80, &format!("{name}: invalid variable name"))?;
            return Ok(1);
        };
        con.out(&loaded_line(&label, hour, minimum))?;
    }
    con.out(&cat(&[b"  done. list with: launchctl list | grep ", &label_prefix, b"\n"]))?;
    Ok(0)
}
