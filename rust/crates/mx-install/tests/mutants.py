#!/usr/bin/env python3
"""Mutation proof for the mx-install crate.

  python3 mutants.py            every mutant
  python3 mutants.py M03 M17    only these
  python3 mutants.py --check    only check that every anchor occurs exactly once

Each mutant is applied to a fresh copy of the workspace (the checkout is never edited):
every file of the copy is touched, so cargo cannot reuse an older build; the old text must
occur exactly once; the edit is read back and the file's hash must differ from the pristine
one; then `cargo test -p mx-install --no-fail-fast` runs in a private target directory under
a time limit and an address-space cap. A mutant is KILLED when the crate builds, the run
reports failed tests, and every failure is an assertion or a refused call (a panic), never a
compile error. BUILD_ERR, TIMEOUT, CAPPED, NOT_REBUILT and SURVIVED are reported as such and
are never counted as kills.

The pristine copy runs before the batch and after it (the same test count, no failure), and
the hash of every source file of the checkout is compared before and after.

Environment: MX_SCRATCH (default ~/.cache/magent-wt-scratch), CARGO_BUILD_JOBS.
"""

import hashlib
import os
import re
import resource
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
RUST = HERE.parent.parent.parent  # rust/
ROOT = RUST.parent

S = "crates/mx-install/src/"
# id, file, old text (exactly once), new text, what it breaks
MUTANTS = [
    # --- the plan: refusals, order, platform --------------------------------------------
    ("M01", S + "plan.rs", "    if !f.flock_on_path {\n        return Decision::Refuse(Refusal::FlockMissing);\n    }\n    // The lock is asked before the first write.", "    if false {\n        return Decision::Refuse(Refusal::FlockMissing);\n    }\n    // The lock is asked before the first write.", "an install without flock goes ahead"),
    ("M02", S + "plan.rs", "    if !f.opts.dry && !f.opts.force && !f.busy.is_empty() {\n        return Decision::Refuse(Refusal::RunInFlight {\n            share: f.layout.share.as_os_str().to_os_string(),\n            profiles: f.busy.clone(),\n        });\n    }\n    let mut steps = deploy_steps(f);", "    if false {\n        return Decision::Refuse(Refusal::RunInFlight {\n            share: f.layout.share.as_os_str().to_os_string(),\n            profiles: f.busy.clone(),\n        });\n    }\n    let mut steps = deploy_steps(f);", "a run in flight does not stop an install: the zero-writes-before-refusal guard is gone"),
    ("M03", S + "plan.rs", "    if !f.opts.dry && !f.opts.force && !f.busy.is_empty() {\n        return Decision::Refuse(Refusal::RunInFlight {\n            share: f.layout.share.as_os_str().to_os_string(),\n            profiles: f.busy.clone(),\n        });\n    }\n    let mut steps = deploy_steps(f);", "    if !f.opts.force && !f.busy.is_empty() {\n        return Decision::Refuse(Refusal::RunInFlight {\n            share: f.layout.share.as_os_str().to_os_string(),\n            profiles: f.busy.clone(),\n        });\n    }\n    let mut steps = deploy_steps(f);", "a dry run is refused by a held lock"),
    ("M04", S + "plan.rs", "    steps.push(Step::Stamp);\n    steps.push(Step::MintKey);", "    steps.push(Step::MintKey);\n    steps.push(Step::Stamp);", "the key is minted before the VERSION stamp"),
    ("M05", S + "plan.rs", "    steps.push(Step::LandingCheck);\n    steps\n}", "    steps\n}", "no check that a profile landed where run.sh looks"),
    ("M06", S + "plan.rs", "        steps.push(say(&format!(\n            \"platform: {uname} is unrecognised; files deployed, scheduling is yours to wire\"\n        )));\n    }\n    false\n}", "        steps.push(say(&format!(\n            \"platform: {uname} is unrecognised; files deployed, scheduling is yours to wire\"\n        )));\n    }\n    f.opts.timers\n}", "--timers is honoured on a platform with no systemd"),
    ("M07", S + "plan.rs", "        enabled += 1;\n", "", "a run that enabled timers still ends in the no-timers refusal"),
    ("M08", S + "plan.rs", "        if !timer.profile_present {", "        if false {", "a timer whose profile is not in the checkout is enabled"),
    ("M09", S + "plan.rs", "        steps.push(run(Cmd::MkdirP(vec![l.stamps.clone()])));\n", "", "the timer stamp directory is not made before the stamp"),
    ("M10", S + "plan.rs", "        if count > 0 {\n            return Decision::Refuse(Refusal::LiveTimers { count });", "        if count > 1 {\n            return Decision::Refuse(Refusal::LiveTimers { count });", "a single enabled timer does not stop an uninstall"),
    ("M77", S + "probe.rs", "        if count > 0 {\n            return Err(Refusal::LiveTimers { count });", "        if count > 1 {\n            return Err(Refusal::LiveTimers { count });", "a single enabled timer does not stop an uninstall (the probe's guard; M10 is the plan's own)"),
    ("M11", S + "plan.rs", "    if !f.opts.dry && !f.assume_yes {\n        let count = f.live_timers.unwrap_or(0);", "    if !f.opts.dry {\n        let count = f.live_timers.unwrap_or(0);", "MAINTAINER_UNINSTALL_YES=1 does not skip the question"),
    ("M12", S + "plan.rs", "    let record = |cmd: Cmd| Step::Run(cmd, OnFail::Record);", "    let record = |cmd: Cmd| Step::Run(cmd, OnFail::Abort);", "a failed disable or removal ends the uninstall instead of being listed"),
    ("M13", S + "plan.rs", "    for command in &f.installed_commands {\n        steps.push(run(Cmd::RmF(command.clone())));", "    for command in &f.installed_commands {\n        steps.push(record(Cmd::RmF(command.clone())));", "a failed command removal is listed instead of ending the run"),
    ("M14", S + "plan.rs", "    steps.push(record(Cmd::RmRf(vec![l.share.clone()])));", "    steps.push(record(Cmd::RmRf(vec![l.bin.clone()])));", "the uninstall removes the bin directory and not the share directory"),
    ("M15", S + "plan.rs", "    if f.systemd_user {\n        steps.push(run(Cmd::Systemctl(vec![\"--user\".into(), \"daemon-reload\".into()])));\n    }\n    if f.darwin {", "    if f.systemd_user {\n    }\n    if f.darwin {", "the uninstall never reloads systemd"),
    ("M16", S + "plan.rs", 'Cmd::Mv { src: item.backup.clone(), dst: item.installed.clone() }', 'Cmd::Mv { src: item.installed.clone(), dst: item.backup.clone() }', "a rollback moves the Rust build over the saved original"),
    ("M17", S + "plan.rs", 'if item.backup_hash.is_none() {\n                return refuse(format!("{} is missing"', 'if item.backup_hash.is_some() {\n                return refuse(format!("{} is missing"', "a rollback with a missing saved original moves the others first"),
    ("M18", S + "plan.rs", "    steps.push(run(Cmd::RmF(stamp)));\n    steps.push(Step::Say(", "    steps.push(Step::Say(", "a rollback leaves the stamp behind"),
    ("M19", S + "plan.rs", "    \"failure-shapes.json\",\n    \"defaults.env\",\n];", "    \"failure-shapes.json\",\n    \"failure-shapes.json\",\n];", "defaults.env is not deployed"),
    ("M20", S + "plan.rs", "    if let Some(stamp) = f.rust_stamp.as_ref().filter(|_| !f.opts.rust) {", "    if let Some(stamp) = f.rust_stamp.as_ref().filter(|_| f.opts.rust) {", "a bash install leaves the Rust stamp that no longer tells the truth"),
    ("M21", S + "plan.rs", "    if let Some(stripped) = text.strip_suffix(b\"\\\"\") {\n        text = stripped;\n    }", "", "the kept-trail line keeps its closing quote"),
    ("M22", S + "plan.rs", "    os(before_last_hyphen(rest))", "    os(rest)", "a timer's profile is the whole name after maintainer@"),
    # --- refusals and arguments -----------------------------------------------------------
    ("M23", S + "refusal.rs", "            Self::RunInFlight { .. } => 69,", "            Self::RunInFlight { .. } => 70,", "a run in flight exits 70"),
    ("M24", S + "refusal.rs", "            Self::LiveTimers { .. } => 3,", "            Self::LiveTimers { .. } => 1,", "enabled timers exit 1 instead of 3"),
    ("M25", S + "refusal.rs", "b\"install.sh: unknown flag \"", "b\"install: unknown flag \"", "the unknown-flag line loses its program name"),
    ("M26", S + "refusal.rs", "            | Self::RustWithUninstall => 64,", "            | Self::RustWithUninstall => 1,", "a bad flag combination exits 1 instead of 64"),
    ("M27", S + "refusal.rs", "b\"            Wait for it (maintainer status), or --force if you are sure.\"", "b\"            Wait for it.\"", "the busy refusal drops its last line"),
    ("M28", S + "args.rs", "            Some(\"--force\") => opts.force = true,", "            Some(\"--force\") => opts.dry = true,", "--force is read as --dry-run"),
    ("M29", S + "args.rs", "    if opts.rollback && (opts.rust || opts.uninstall || opts.timers) {", "    if opts.rollback && (opts.rust || opts.uninstall) {", "--rollback --timers is accepted"),
    ("M30", S + "args.rs", "    if opts.rust && opts.uninstall {", "    if false {", "--rust --uninstall is accepted"),
    # --- applying the plan -----------------------------------------------------------------
    ("M31", S + "apply.rs", "                        OnFail::Abort => return Err(Exit(status)),", "                        OnFail::Abort => {}", "a failing command no longer ends the run (set -e is gone)"),
    ("M32", S + "apply.rs", "        if self.machine.is_file(&path) {\n            return Ok(());\n        }\n        let key", "        if false {\n            return Ok(());\n        }\n        let key", "an existing receipt key is replaced"),
    ("M33", S + "apply.rs", "            b\"\\ncommit=\",", "            b\"\\nrev=\",", "the VERSION stamp names its commit rev"),
    ("M34", S + "apply.rs", "        if landed {\n            return Ok(());\n        }", "        if true {\n            return Ok(());\n        }", "an install with no profile.env still says done"),
    ("M35", S + "apply.rs", "            if status != 0 {\n                con.echo_err(b\"install.sh: generating the deny wall failed\")?;", "            if false {\n                con.echo_err(b\"install.sh: generating the deny wall failed\")?;", "a failing deny wall does not stop the install"),
    ("M36", S + "apply.rs", "                con.echo_err(&line)?;\n                return Err(Exit(1));", "                con.echo_err(&line)?;\n                return Err(Exit(0));", "--timers with nothing to enable exits 0"),
    ("M37", S + "apply.rs", "                if !failed.is_empty() {", "                if failed.is_empty() {", "the list of failed steps is printed when there is none"),
    ("M38", S + "apply.rs", "        con.say(&cat(&[b\"would: \", &cmd.text()]))?;\n        Ok(0)", "        con.say(&cat(&[b\"would: \", &cmd.text()]))?;\n        Ok(1)", "a dry run reports a failing command"),
    ("M39", S + "apply.rs", "        Cmd::ChmodX(vec![program.backup.clone()]),", "        Cmd::ChmodX(vec![program.installed.clone()]),", "the saved original of a Rust install is not made executable"),
    ("M40", S + "apply.rs", 'let hash = match self.machine.sha256(&program.source) {', 'let hash = match self.machine.sha256(&program.installed) {', "the Rust stamp records the hash of the file on disk and not of the build about to be installed"),
    ("M41", S + "apply.rs", "        if let Err(error) = self.machine.write(&path, b\"\") {\n            return self.redirect_failed(con, 190, &path, &error);\n        }\n", "", "git and date are asked before the VERSION file is opened"),
    # --- the machine and the file commands --------------------------------------------------
    ("M42", S + "machine.rs", "        file.write_all(bytes)?;\n        fs::set_permissions(path, fs::Permissions::from_mode(0o600))", "        file.write_all(bytes)?;\n        fs::set_permissions(path, fs::Permissions::from_mode(0o644))", "the receipt key is readable by others"),
    ("M44", S + "machine.rs", "        let mut bytes = [0_u8; 32];\n        File::open(\"/dev/urandom\")?.read_exact(&mut bytes)?;", "        let bytes = [0_u8; 32];\n        File::open(\"/dev/urandom\")?;", "the receipt key is 64 zeros"),
    ("M45", S + "coreutils.rs", "    let opened = if exists_following(dst) {", "    let opened = if false {", "cp cannot replace a file that is there"),
    ("M46", S + "coreutils.rs", ".create_new(true).mode(meta.mode() & 0o777).open(dst)", ".create_new(true).mode(0o600).open(dst)", "a new file takes mode 600 and not its source's mode"),
    ("M47", S + "coreutils.rs", "    let target = if is_dir_following(dst) {\n        dst.join(src.file_name().unwrap_or_else(|| OsStr::new(\"\")))\n    } else {\n        dst.to_path_buf()\n    };\n    copy_file(src, &meta, &target, &mut out);", "    let target = dst.to_path_buf();\n    copy_file(src, &meta, &target, &mut out);", "cp does not put a file inside a directory destination"),
    ("M48", S + "coreutils.rs", "    let meta = match fs::symlink_metadata(path) {\n        Ok(meta) => meta,\n        Err(error) if error.kind() == ErrorKind::NotFound => return true,", "    let meta = match fs::metadata(path) {\n        Ok(meta) => meta,\n        Err(error) if error.kind() == ErrorKind::NotFound => return true,", "rm -rf follows a symbolic link and removes what it points at"),
    ("M49", S + "coreutils.rs", "        Ok(meta) if meta.file_type().is_dir() => {\n            out.say(&format!(\"rm: cannot remove", "        Ok(meta) if meta.file_type().is_dir() => {\n            drop(fs::remove_dir_all(path));\n            out.say(&format!(\"rm: cannot remove", "rm -f removes a directory"),
    ("M50", S + "coreutils.rs", "    match opened.and_then(|file| file.set_modified(SystemTime::now())) {", "    match opened.map(|_| ()) {", "touch does not update the time of a file that is there"),
    ("M51", S + "coreutils.rs", "    let add = 0o111 & !umask();", "    let add = 0o111;", "chmod +x ignores the umask"),
    # --- sed -----------------------------------------------------------------------------------
    ("M52", S + "sedsub.rs", "            b'n' | b'\\n' => items.push(Item::Literal(b'\\n')),", "            b'\\n' => items.push(Item::Literal(b'\\n')),", "a backslash-n in an argument is the letter n"),
    ("M53", S + "sedsub.rs", "            b'1'..=b'9' => return Err(fail(end, SedFault::InvalidReference { digit: next })),", "            b'1'..=b'9' => items.push(Item::Literal(next)),", "a numbered group is accepted"),
    ("M54", S + "sedsub.rs", "                Item::Mode(case) => {\n                    mode = *case;\n                    once = Case::Keep;\n                }", "                Item::Mode(case) => {\n                    mode = *case;\n                }", "a pending single-character case change survives a mode change"),
    # --- new-profile ---------------------------------------------------------------------------
    ("M55", S + "newprofile.rs", "    if name.is_empty() || name.starts_with(b\"_\") || name.contains(&b'/') {", "    if name.is_empty() || name.contains(&b'/') {", "a profile name that starts with an underscore is accepted"),
    ("M56", S + "newprofile.rs", "    raw(value).iter().any(|byte| matches!(byte, b'|' | b'&'))", "    raw(value).iter().any(|byte| matches!(byte, b'|'))", "an ampersand in an argument is accepted"),
    ("M57", S + "newprofile.rs", "    if !raw(slug).contains(&b'/') {", "    if raw(slug).is_empty() {", "a slug with no slash is accepted"),
    ("M58", S + "scaffold.rs", "    for (hour, task) in (9_u32..).zip(&tasks) {", "    for (hour, task) in (10_u32..).zip(&tasks) {", "the first timer is at 10:17"),
    ("M59", S + "scaffold.rs", "    if found {\n        con.echo_err(b\"new-profile: a placeholder survived substitution:\")?;", "    if false {\n        con.echo_err(b\"new-profile: a placeholder survived substitution:\")?;", "a surviving placeholder is not refused"),
    ("M60", S + "scaffold.rs", "    if machine.is_dir(&target) {", "    if machine.is_file(&target) {", "an existing profile directory is overwritten"),
    # --- the renderers -------------------------------------------------------------------------
    ("M61", S + "cron.rs", "    for (hour, task) in (9_u32..).zip(tasks) {", "    for (hour, task) in (10_u32..).zip(tasks) {", "the first cron entry is at 10:17"),
    ("M62", S + "cron.rs", "        if line == marker {\n            skip = true;\n        }", "", "the old block is not stripped before a new one is written"),
    ("M63", S + "cron.rs", "    crate::util::strip_trailing_newlines(out)\n}", "    out\n}", "the stripped crontab keeps its trailing newlines"),
    ("M64", S + "alert.rs", "    before_last_hyphen(instance)\n}", "    instance\n}", "the alert's profile is the whole instance name"),
    ("M65", S + "alert.rs", "        None => instance,\n    };\n    (profile, task)", "        None => b\"\",\n    };\n    (profile, task)", "an instance with no hyphen has no task"),
    ("M66", S + "units.rs", "    if env.is(\"MAINTAINER_NOTIFY\", \"off\") {", "    if false {", "MAINTAINER_NOTIFY=off still pops a notification"),
    ("M73", S + "plan.rs", '    ["maintainer", "maintainer-merge", "maintainer-doctor", "maintainer-repo", "maintainer-mcp"];', '    ["maintainer", "maintainer-merge", "maintainer-doctor", "maintainer-repo", "maintainer-repo"];', "maintainer-mcp is neither deployed nor removed"),
    ("M74", S + "apply.rs", 'b"\\ninstalled_from=",', 'b"\\nfrom=",', "the VERSION stamp does not name the checkout as installed_from"),
    ("M75", S + "probe.rs", 'glob_affixes(machine, &layout.stamps, "stamp-maintainer@", ".timer")', 'glob_affixes(machine, &layout.stamps, "stamp-", ".timer")', "the uninstall removes the timer stamps of other software"),
    ("M76", S + "probe.rs", 'glob_affixes(machine, units, "maintainer", ".service")', 'glob_affixes(machine, units, "maintainer-", ".service")', "the uninstall leaves maintainer@.service in place"),
    ("M78", S + "console.rs", "        Err(error) if error.kind() == ErrorKind::BrokenPipe => Err(Exit(BROKEN_PIPE)),", "        Err(error) if error.kind() == ErrorKind::NotFound => Err(Exit(BROKEN_PIPE)),", "a closed pipe is not noticed"),
    ("M79", S + "console.rs", "        line.extend_from_slice(text);\n        line.push(b'\\n');\n        self.out(&line)\n    }\n}", "        line.extend_from_slice(text);\n        self.out(&line)\n    }\n}", "say prints no newline"),
    ("M80", S + "plan.rs", "    steps.push(Step::WriteRustStamp(swapped.clone()));\n    for program in swapped {\n        steps.push(Step::Swap(program));\n    }", "    for program in swapped.clone() {\n        steps.push(Step::Swap(program));\n    }\n    steps.push(Step::WriteRustStamp(swapped));", "the Rust stamp is written after the swaps, so a failed swap cannot be rolled back"),
    ("M81", S + "plan.rs", "        } else if item.backup_hash.is_none() {\n            // Restored already: nothing to do for this program.", "        } else if item.backup_hash.is_none() {\n            return refuse(format!(\"{} is missing\", item.backup.display()));", "a rollback cannot run again after a partial one"),
    ("M82", S + "plan.rs", "        } else if item.backup_hash == item.installed_hash {\n            steps.push(run(Cmd::RmF(item.backup.clone())));", "        } else if item.backup_hash == item.installed_hash {\n            return refuse(String::from(\"swap not started\"));", "a swap that never started blocks the rollback"),
    ("M83", S + "plan.rs", "        } else {\n            return refuse(format!(\n                \"{} no longer matches the stamp", "        } else if false {\n            return refuse(format!(\n                \"{} no longer matches the stamp", "a rollback ignores the stamped sha256"),
    ("M84", S + "probe.rs", "        if machine.lexists(backup) && !ours {", "        if machine.lexists(backup) && false && !ours {", "a link or a user file at a backup name is overwritten"),
    ("M85", S + "probe.rs", "let ours = rust_stamp.is_some() && machine.is_file(backup) && !machine.is_symlink(backup);", "let ours = rust_stamp.is_some() && machine.is_file(backup);", "a link left at a backup name after a stamped install is followed"),
    # --- a rollback never follows or replaces a link (decision P6A-1) ------------------------------
    ("M86", S + "plan.rs", "            if is_link {\n                return refuse(link_detail(path));\n            }", "            if false {\n                return refuse(link_detail(path));\n            }", "a rollback accepts a link at an installed program or a saved original"),
    ("M87", S + "probe.rs", "                    installed_is_link: machine.is_symlink(&installed),", "                    installed_is_link: false,", "a link at an installed program is not seen, so a link to the stamped build is replaced"),
    ("M88", S + "probe.rs", "                    backup_is_link: machine.is_symlink(&backup),", "                    backup_is_link: false,", "a link at a saved original is not seen, so it is moved into place"),
    ("M89", S + "probe.rs", "    if machine.is_symlink(&stamp_path) {", "    if false {", "a stamp that is a link is followed"),
    ("M90", S + "probe.rs", "        if pairs.iter().any(|(installed, _, _)| *installed == program.installed) {", "        if false {", "a stamp that lists a program twice moves its saved original twice"),
    # --- probes ----------------------------------------------------------------------------------
    ("M67", S + "probe.rs", "        if ran.succeeded() {\n            continue;\n        }\n        busy.push(", "        if true {\n            continue;\n        }\n        busy.push(", "a held lock is never reported"),
    ("M68", S + "probe.rs", "        if !machine.exists(&lock) {\n            continue;\n        }", "", "the lock is asked for a profile that has no lock file"),
    ("M69", S + "probe.rs", "        machine.is_file(&candidate) && machine.is_executable(&candidate)", "        machine.is_file(&candidate)", "a file with no execute bit counts as a command on the path"),
    ("M70", S + "probe.rs", "            if systemd_user_session(machine, proc, env) { live_timer_count(proc, env) } else { 0 };", "            live_timer_count(proc, env);", "enabled timers are listed on a machine with no user session"),
    ("M71", S + "probe.rs", "            && machine.read(&program.source).is_ok_and(|bytes| !bytes.is_empty());", "            && machine.read(&program.source).is_ok();", "an empty file is accepted as a Rust build"),
    ("M72", S + "sourced.rs", "        let code = if code == 127 { 1 } else { code };", "        let code = if code == 127 { 127 } else { code };", "a profile that stops on an unbound variable ends with 127"),
]


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def tree_hash(root):
    digest = hashlib.sha256()
    for path in sorted(Path(root).rglob("*")):
        if path.is_file() and "target" not in path.parts and "__pycache__" not in path.parts:
            digest.update(str(path.relative_to(root)).encode())
            digest.update(sha(path).encode())
    return digest.hexdigest()


def copy_workspace(dest):
    """dest/rust is the workspace; dest/tests holds the golden files the scaffold tests read."""
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(RUST, dest / "rust", ignore=shutil.ignore_patterns("target", "__pycache__"), symlinks=True)
    golden = dest / "tests/characterization/golden/install"
    shutil.copytree(ROOT / "tests/characterization/golden/install", golden, symlinks=True)


def touch_all(root):
    now = time.time()
    for path in Path(root).rglob("*"):
        if path.is_file():
            os.utime(path, (now, now))


def cap():
    resource.setrlimit(resource.RLIMIT_AS, (12 << 30, 12 << 30))


def cargo_test(copy, target, timeout):
    env = dict(os.environ)
    env.update({"CARGO_TARGET_DIR": str(target), "CARGO_TERM_COLOR": "never", "CARGO_BUILD_JOBS": os.environ.get("CARGO_BUILD_JOBS", "2"),
                "PYTHONDONTWRITEBYTECODE": "1"})
    # Popen in a session of its own, so a timeout kills the whole group: killing only cargo
    # leaves a hung test binary alive.
    proc = subprocess.Popen(["cargo", "test", "-p", "mx-install", "--locked", "--offline", "--no-fail-fast"], cwd=copy / "rust",
                            env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, preexec_fn=cap, start_new_session=True)
    try:
        out, _ = proc.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        out, _ = proc.communicate()
        return "TIMEOUT", [], 0, 0, out or ""
    if "memory allocation" in out and "failed" in out:
        return "CAPPED", [], 0, 0, out
    if "could not compile" in out or re.search(r"^error(\[E\d+\])?: (?!test failed|\d+ targets? failed)", out, re.M):
        return "BUILD_ERR", [], 0, 0, out
    passed = sum(int(n) for n in re.findall(r"test result: \w+\. (\d+) passed", out))
    failed = sum(int(n) for n in re.findall(r"; (\d+) failed", out))
    names = re.findall(r"^test (\S+) \.\.\. FAILED$", out, re.M)
    if "Compiling mx-install" not in out:
        return "NOT_REBUILT", names, passed, failed, out
    return ("RED" if failed else "GREEN"), names, passed, failed, out


def check_anchors():
    bad = 0
    ids = [m[0] for m in MUTANTS]
    if len(ids) != len(set(ids)):
        print("duplicate mutant ids")
        bad += 1
    for mid, rel, old, new, note in MUTANTS:
        text = (RUST / rel).read_text()
        n = text.count(old)
        if n != 1 or old == new:
            print("%s: the old text occurs %d times in %s" % (mid, n, rel))
            bad += 1
    print("anchors: %d mutants, %d bad" % (len(MUTANTS), bad))
    return bad == 0


def main():
    args = sys.argv[1:]
    if args == ["--check"]:
        sys.exit(0 if check_anchors() else 1)
    scratch = Path(os.environ.get("MX_SCRATCH", Path.home() / ".cache" / "magent-wt-scratch")) / "mx-install-mutants"
    scratch.mkdir(parents=True, exist_ok=True)
    target = scratch / "target"
    pristine = scratch / "pristine"
    copy_workspace(pristine)
    live_before = tree_hash(RUST)
    pristine_hash = tree_hash(pristine)
    work = scratch / "work"

    def control(label):
        copy_workspace(work)
        touch_all(work)
        status, names, passed, failed, out = cargo_test(work, target, 3000)
        print("control %-6s %s: %d passed, %d failed" % (label, status, passed, failed), flush=True)
        if status != "GREEN" or passed < 150:
            print(out[-2500:])
            sys.exit("mutants: the pristine control is not green with the full suite")
        return passed

    before = control("before")
    rows, survivors, killed = [], [], 0
    chosen = [m for m in MUTANTS if not args or m[0] in args]
    for mid, rel, old, new, note in chosen:
        copy_workspace(work)
        path = work / "rust" / rel
        text = path.read_text()
        if text.count(old) != 1:
            sys.exit("mutants: %s: the old text occurs %d times in %s" % (mid, text.count(old), rel))
        before_hash = sha(path)
        assert sha(RUST / rel) == before_hash
        path.write_text(text.replace(old, new))
        back = path.read_text()
        if new not in back or sha(path) == before_hash:
            sys.exit("mutants: %s: the edit did not land" % mid)
        touch_all(work)
        status, names, passed, failed, out = cargo_test(work, target, 2400)
        if status == "BUILD_ERR":
            print(out[-1500:])
        if status == "RED":
            outcome = "KILLED"
            killed += 1
        else:
            outcome = status
            survivors.append((mid, status))
        first = names[0] if names else "-"
        rows.append((mid, rel.split("/")[-1], note, outcome, failed, first))
        print("%-4s %-14s %-8s %3d red  %s  [%s]" % (mid, rel.split("/")[-1], outcome, failed, first, note), flush=True)
    after = control("after")
    live_after = tree_hash(RUST)
    pristine_after = tree_hash(pristine)
    print()
    print("| # | File | Mutation | Result | Tests red | First named test that went red |")
    print("|---|---|---|---|---|---|")
    for mid, name, note, outcome, failed, first in rows:
        print("| %s | %s | %s | %s | %d | `%s` |" % (mid, name, note, outcome, failed, first))
    print()
    print("mutants: %d run, %d killed, %d not killed %s" % (len(rows), killed, len(survivors), survivors))
    print("mutants: pristine control %d passed before, %d after; checkout tree hash %s; pristine copy hash %s"
          % (before, after, "unchanged" if live_before == live_after else "CHANGED", "unchanged" if pristine_hash == pristine_after else "CHANGED"))
    ok = live_before == live_after and pristine_hash == pristine_after and before == after and not survivors
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
