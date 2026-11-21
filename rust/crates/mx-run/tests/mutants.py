#!/usr/bin/env python3
"""Mutation proof for the mx-run crate and the RealPipe adapter.

  python3 mutants.py            every mutant
  python3 mutants.py M03 M17    only these
  python3 mutants.py --check    only check that every anchor occurs exactly once

Each mutant is applied to a fresh copy of the workspace (the checkout is never
edited): every file of the copy is touched, so cargo cannot reuse an older
build; the old text must occur exactly once; the edit is read back and the
file's hash must differ from the pristine one; then `cargo test -p mx-run -p
mx-ports --no-fail-fast` runs in a private target directory under a time limit
and an address-space cap. A mutant is KILLED when the crate builds, the run
reports failed tests, and every failure is an assertion or a refused call (a
panic), never a compile error. BUILD_ERR, TIMEOUT, CAPPED and SURVIVED are
reported as such and are never counted as kills.

The pristine copy runs before the batch and after it (the same test count, no
failure), and the hash of every source file of the checkout is compared before
and after.

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

R = "crates/mx-run/src/"
P = "crates/mx-ports/src/"
# id, file, old text (exactly once), new text, what it breaks
MUTANTS = [
    ("M01", R + "cadence.rs", "    if last > now {", "    if false {", "D12 undone: a stamp from the future skips the run"),
    ("M02", R + "cadence.rs", "if age_hours < min_hours {", "if age_hours <= min_hours {", "a stamp exactly as old as the minimum skips"),
    ("M03", R + "cadence.rs", "let age_hours = (now - last) / SECONDS_PER_HOUR;", "let age_hours = (now - last + 3599) / SECONDS_PER_HOUR;", "the age rounds up instead of down"),
    ("M04", R + "cadence.rs", "    if facts.forced {", "    if false {", "MAINTAINER_FORCE=1 does not force"),
    ("M05", R + "cadence.rs", "        return Decision::Refuse { min_hours: facts.min_hours.clone() };", "        return Decision::Run;", "D11 undone: a minimum that is not a number runs"),
    ("M06", R + "cadence.rs", "Some(b'-') => (true, &trimmed[1..]),", "Some(b'-') => (false, &trimmed[1..]),", "a negative minimum reads as positive"),
    ("M07", R + "identity.rs", "Answer::NoAnswer if attempt >= tries => Next::GiveUp,", "Answer::NoAnswer if attempt > tries => Next::GiveUp,", "one try too many"),
    ("M08", R + "identity.rs", "login.eq_ignore_ascii_case(account)", "login == account", "the login compare is case sensitive"),
    ("M09", R + "identity.rs", 'if contains(stderr, b"HTTP 401") {', "if false {", "the 401 branch is gone, so a rejected token is retried"),
    ("M10", R + "identity.rs", "token.chars().count() >= MIN_SECRET_CHARS", "token.chars().count() > MIN_SECRET_CHARS", "D15 boundary: a token of exactly 16 characters is refused"),
    ("M11", R + "identity.rs", "attempt.saturating_mul(backoff)", "backoff", "the backoff does not grow with the attempt"),
    ("M12", R + "identity.rs", "if rc != 0 || login.is_empty() {", "if login.is_empty() {", "a pinned token whose gh failed but printed a login is confirmed"),
    ("M13", R + "humanise.rs", '"Another run held the shared tree for over {lock_wait}s.', '"Another run held the shared tree for over {lock_wait} s.', "the lock toast wording"),
    ("M14", R + "prompt.rs", "    out.extend_from_slice(parts.core);", "    out.extend_from_slice(parts.task_prompt);", "the doctrine is dropped from the prompt"),
    ("M15", R + "prompt.rs", '.replace(\'&\', "\\\\&").replace(\'|\', "\\\\|")', '.replace(\'&\', "\\\\&")', "a | in the maintainer name is not escaped"),
    ("M16", R + "prompt.rs", '    post == "off"', '    post.eq_ignore_ascii_case("off")', "POST=OFF is a rehearsal"),
    ("M17", R + "prompt.rs", "    if let Some(prose) = parts.prose {", "    if let Some(prose) = parts.prose.filter(|_| false) {", "the prose section is dropped"),
    ("M18", R + "prompt.rs", 'out.extend_from_slice(b"\\n\\n## Context for this run, computed just now\\n\\n```\\n");', 'out.extend_from_slice(b"\\n## Context for this run, computed just now\\n\\n```\\n");', "one blank line short before the context"),
    ("M19", R + "pass.rs", 'self.forced = self.inv.env.get("MAINTAINER_FORCE") == Some("1");', 'self.forced = self.inv.env.get("MAINTAINER_FORCE").is_some();', "any MAINTAINER_FORCE forces"),
    ("M20", R + "pass.rs", '        self.unset("MAINTAINER_FORCE");\n', "", "unset MAINTAINER_FORCE removed: the override travels to the agent"),
    ("M21", R + "pass.rs", '        self.unset("GH_TOKEN");\n', "", "D13 undone: an inherited GH_TOKEN reaches the first calls"),
    ("M22", R + "pass.rs", 'self.bash_error(58, &format!("1: {USAGE}"));', 'self.bash_error(59, &format!("1: {USAGE}"));', "the usage message names the wrong line"),
    ("M23", R + "pass.rs", "        Err(Stop(78))", "        Err(Stop(1))", "a nested run exits 1, not 78"),
    ("M24", R + "pass.rs", 'let Some(marker) = self.inv.env.non_empty("MAINTAINER_IN_RUN")', 'let Some(marker) = self.inv.env.get("MAINTAINER_IN_RUN")', "an empty MAINTAINER_IN_RUN counts as a marker"),
    ("M25", R + "pass.rs", ".contains(&format!(\" {} \", self.task))", ".contains(&self.task)", "a prefix of a task is a task"),
    ("M26", R + "pass.rs", "|| byte == b'_')", "|| byte == b'_' || byte == b'-')", "a hyphenated task passes the model lookup"),
    ("M27", R + "pass.rs", "            self.gate(None)?;", "            let _ = self.gate(None);", "the first cadence gate does not skip"),
    ("M28", R + "pass/locked.rs", 'self.gate(Some("after waiting for the lock"))?;', 'let _ = self.gate(Some("after waiting for the lock"));', "the second cadence gate does not skip"),
    ("M29", R + "pass/locked.rs", "        self.write_lease();\n        self.confirm_account()?;\n        self.pin_token()?;\n", "        self.confirm_account()?;\n        self.pin_token()?;\n        self.write_lease();\n", "the lease is written after the pin instead of before the identity"),
    ("M30", R + "pass/locked.rs", "        self.arm_scrub_trap(&token);\n", "", "the exit scrub is never armed"),
    ("M31", R + "pass/locked.rs", "if !identity::token_can_be_scrubbed(&token) {", "if false {", "D15 undone: a short token runs"),
    ("M32", R + "pass/locked.rs", '        self.export("GH_TOKEN", &token);\n', "", "the token is never pinned into GH_TOKEN"),
    ("M33", R + "pass/locked.rs", '        self.unset("GITHUB_TOKEN");\n', "", "GITHUB_TOKEN survives the pin"),
    ("M34", R + "pass/locked.rs", "            if by_id.exists() {", "            if false {", "an existing log under the run id is replaced"),
    ("M35", R + "pass/locked.rs", ".take_while(|b| b.is_ascii_lowercase() || *b == b'-')", ".take_while(|b| b.is_ascii_alphabetic() || *b == b'-')", "the run id accepts a capital in the first word"),
    ("M36", R + "pass/locked.rs", '"log renamed to match run id {}\\n"', '"log renamed to {}\\n"', "the rename line wording"),
    ("M37", R + "pass/locked.rs", "            64 => {", "            65 => {", "flock status 64 is not the bad-wait refusal"),
    ("M38", R + "pass/locked.rs", "                Err(Stop(75))\n            }\n            64 => {", "                Err(Stop(1))\n            }\n            64 => {", "a held lock exits 1, not 75"),
    ("M39", R + "pass/locked.rs", "            PinnedVerdict::Unresolved { rc } => {\n                // 75, not 1: nothing is wrong with the credentials, the run could not ask.\n                self.alert_transient(&identity::pinned_unresolved_message(rc, &log_text));\n                Err(Stop(75))", "            PinnedVerdict::Unresolved { rc } => {\n                // 75, not 1: nothing is wrong with the credentials, the run could not ask.\n                self.alert_transient(&identity::pinned_unresolved_message(rc, &log_text));\n                Err(Stop(1))", "an unresolved pinned token exits 1, not 75"),
    ("M40", R + "pass/locked.rs", "c == '/' || c == '.'", "c == '/'", "a dot of the repository path stays a dot in the session directory name"),
    ("M41", R + "pass/close.rs", 'format!("no report; {cause}")', 'format!("no report")', "the abort reason loses its cause"),
    ("M42", R + "pass/close.rs", 'let refused = finish_out.contains("fixture rather than a report");', "let refused = false;", "a refused report is aborted as a missing one"),
    ("M43", R + "pass/close.rs", "        if end.alerted {", "        if false {", "a backend failure with no report alerts twice"),
    ("M44", R + "pass/close.rs", "            return Err(Stop(75));", "            return Err(Stop(1));", "a transient stop with no report exits 1, not 75"),
    ("M45", R + "pass/close.rs", "if answer.ok() { answer.stdout_text() } else { String::new() }", "answer.stdout_text()", "a failing digest is not dropped"),
    ("M46", R + "pass/setup.rs", "let refused_name = name.contains('/') || name.contains(\"..\");", "let refused_name = false;", "D18 undone: a BACKEND with a slash is looked up"),
    ("M47", R + "pass/setup.rs", "if !self.show && self.post == \"off\" && !self.backend().declares_rehearsal() {", "if self.post == \"off\" && !self.backend().declares_rehearsal() {", "a preview refuses POST=off on a backend that cannot rehearse"),
    ("M48", R + "pass/setup.rs", "            return Err(Stop(78));\n        }\n        Ok(())\n    }\n}\n", "            return Err(Stop(1));\n        }\n        Ok(())\n    }\n}\n", "a task the backend may not run exits 1, not 78"),
    ("M49", R + "lock.rs", "dup2(&file, &mut target)", "rustix::io::dup3(&file, &mut target, rustix::io::DupFlags::CLOEXEC)", "descriptor 9 is close-on-exec, so the agent loses the lock"),
    ("M50", R + "trap.rs", "            let _ = mx_ports::kill_live_pipelines();\n", "", "the signal thread leaves the agent running"),
    ("M51", R + "trap.rs", "            run_now();\n            // The default action", "            // The default action", "the signal thread does not run the exit trap"),
    ("M52", R + "trap.rs", "    if !has_signalled() {\n        return;\n    }", "    if true {\n        return;\n    }", "the checkpoint never waits, so the pass goes on after a signal"),
    ("M53", R + "trap.rs", "    fn drop(&mut self) {\n        run_now();\n        // A signal", "    fn drop(&mut self) {\n        // A signal", "a panic does not run the exit trap"),
    ("M61", R + "trap.rs", "                state = CHANGED.wait(state).unwrap_or_else(PoisonError::into_inner);", "                return;", "P2A-trap: run_now returns while another thread still runs the work"),
    ("M62", R + "trap.rs", "    run_now();\n    give_up();", "    give_up();", "P2A-trap: the checkpoint leaves while the work is running"),
    ("M63", R + "trap.rs", "            // Returns after the work has finished, also when the main thread started it.\n            run_now();\n", "", "P2A-trap: the signal thread ends the process before the work is done"),
    ("M64", R + "trap.rs", "    if state.phase == Phase::Done {\n        state.phase = Phase::NotRun;\n    }\n", "", "work armed after an earlier run never runs"),
    ("M65", R + "pass/setup.rs", "        self.check_rehearsal()?;\n        self.stamp_version();\n        self.write_header();\n        self.check_backend()?;", "        self.check_backend()?;\n        self.check_rehearsal()?;\n        self.stamp_version();\n        self.write_header();", "the backend is checked before its wall is in the variable set"),
    ("M66", R + "pass/close.rs", '            self.append_log(&format!("{note}\\n"));', '            self.append_log(&format!("{note}"));', "log notes run together on one line"),
    ("M67", R + "pass/close.rs", "        for export in &plan.exports {\n            self.export(&export.name, &export.value);\n        }", "        for export in &plan.exports {\n            self.export(&export.name, &export.value);\n        }\n        for (name, value) in plan.pipeline.stages.first().env.clone() {\n            self.export(&name, &value);\n        }", "a key in the stage environment is exported to finish and the digest"),
    ("M68", R + "exec.rs", "            if stage.cwd.is_none() {", "            if true {", "the executor overrides the working directory the plan names"),
    ("M69", R + "exec.rs", "            stage.env = self.child_env(&plan_env);", "            stage.env = plan_env.iter().map(|(a, b)| ((*a).to_string(), (*b).to_string())).collect();", "the agent gets only what the plan names, not the run environment"),
    ("M70", P + "real_pipe.rs", "        #[cfg(test)]\n        run_hook();\n        live.leaders.push(child.id());\n        drop(live);", "        drop(live);\n        #[cfg(test)]\n        run_hook();\n        self.live.lock().leaders.push(child.id());", "P2A-live: the stage is registered after an unlocked spawn"),
    ("M71", P + "real_pipe.rs", "        if live.closing {", "        if false {", "P2A-live: a stage starts after the stop began"),
    ("M72", P + "real_pipe.rs", "        live.closing = true;\n", "", "P2A-live: the stop does not refuse later spawns"),
    ("M73", R + "exec.rs", "    fn before_call(&self) {\n        if self.checkpoint {\n            crate::trap::checkpoint();\n        }\n    }", "    fn before_call(&self) {}", "P2A-live: the executor starts a program after the signal flag is up"),
    ("M74", R + "lib.rs", "            let _guard = trap::ExitGuard;\n            let flow = pass.go();", "            let _guard = trap::ExitGuard;\n            let mut pass = pass;\n            let flow = pass.go();", "the run lock closes before the scrub when the pass panics"),
    ("M75", R + "pass.rs", '        self.unset("GITHUB_TOKEN");\n', "", "D22 undone: an inherited GITHUB_TOKEN reaches the first calls"),
    ("M76", R + "pass.rs", "            Ok(meta) if !meta.is_file() => Stamp::Missing,\n", "", "a directory named like the stamp skips the run"),
    ("M77", R + "env.rs", '.field("names", &self.vars.keys().collect::<Vec<_>>())', '.field("vars", &self.vars)', "Env prints its values, GH_TOKEN among them"),
    ("M78", R + "reexec.rs", " || take_argv0().is_some()", "", "the second image starts again when descriptor 9 is still open"),
    ("M79", R + "reexec.rs", 'std::fs::read_link("/proc/self/fd/9").is_ok()', "false", "an open descriptor 9 is not noticed, so the run lock refuses"),
    ("M80", R + "pass.rs", "            self.retry_settings()?;\n", "", "a GH_TRIES that is not a number refuses only after the lock and the lease"),
    ("M81", R + "trap.rs", "[SIGHUP, SIGINT, SIGTERM, SIGUSR1, SIGUSR2, SIGALRM]", "[SIGHUP, SIGINT, SIGTERM, SIGUSR1, SIGUSR2]", "SIGALRM is not handled"),
    ("M82", R + "trap.rs", "            drop(emulate_default_handler(signal));\n", "", "the process exits with 128 + n instead of dying of the signal"),
    ("M83", R + "exec.rs", "pub const CALL_TIMEOUT: Duration = Duration::from_hours(3);", "pub const CALL_TIMEOUT: Duration = Duration::from_hours(30);", "one call may run for 30 hours"),
    ("M84", P + "real_pipe.rs", "        let deadline = Instant::now() + spec.timeout;", "        let deadline = Instant::now() + spec.timeout * 2;", "the deadline is doubled"),
    ("M85", R + "pass/setup.rs", 'format!("{home}/.local/bin:{home}/.cargo/bin:{old}")', 'format!("{home}/.cargo/bin:{home}/.local/bin:{old}")', "the cargo bin comes before the local bin on PATH"),
    ("M86", R + "pass/close.rs", 'self.var_or("PROSE_STYLE", "stop-slop") != "raw"', 'self.var_or("PROSE_STYLE", "stop-slop").to_lowercase() != "raw"', "RAW counts as raw"),
    ("M87", R + "exec.rs", "    text.trim_end_matches('\\n').to_string()", "    text.trim_end().to_string()", "command substitution strips trailing blanks too"),
    ("M88", R + "humanise.rs", "lines.join(\"\\n\").trim_end_matches('\\n').to_string()", "lines.join(\"\\n\").trim_end().to_string()", "the toast body is stripped of blanks too"),
    ("M89", R + "host.rs", "meta.permissions().mode() & 0o111 != 0", "meta.permissions().mode() & 0o100 != 0", "only the owner's execute bit makes a command"),
    ("M90", R + "exec.rs", "        if !self.checkpoint {\n            let done = self.capture(words, extra);", "        if false {\n            let done = self.capture(words, extra);", "the exit trap calls the helper through a pipe port that is closed after a stop"),
    ("M54", R + "exec.rs", "            if stage.cwd.is_none() {", "            if false {", "the agent does not run in the checkout"),
    ("M55", R + "exec.rs", "            vars.insert((*name).to_string(), (*value).to_string());\n", "", "the extra variables of a call are dropped"),
    ("M56", R + "env.rs", '.filter(|(name, _)| name.as_str() != "PATH" && name.as_str() != "LC_ALL")', '.filter(|(name, _)| name.as_str() != "PATH")', "a child gets LC_ALL from the environment"),
    ("M57", P + "real_pipe.rs", "            .process_group(0)\n", "", "stages share the caller's process group"),
    ("M58", P + "real_pipe.rs", "OpenOptions::new().create(true).append(true).open(path)", "OpenOptions::new().create(true).write(true).truncate(true).open(path)", "an appended file is truncated"),
    ("M59", P + "real_pipe.rs", '.env("LC_ALL", plain_locale(self.locale.as_deref()));', '.env("LC_ALL", "C");', "the locale of the caller is not passed"),
    ("M60", P + "real_pipe.rs", "        command.arg0(name).args(&stage.argv.as_slice()[1..]).env_clear();", "        command.arg0(name).args(&stage.argv.as_slice()[1..]);", "a stage inherits the whole environment"),
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
    """dest/rust is the workspace; dest/lib is lib/, which the lifecycle tests copy from."""
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(RUST, dest / "rust", ignore=shutil.ignore_patterns("target", "__pycache__"), symlinks=True)
    shutil.copytree(RUST.parent / "lib", dest / "lib", symlinks=True)


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
    # leaves a hung test binary alive (the reviewer of the first version found it).
    proc = subprocess.Popen(["cargo", "test", "-p", "mx-run", "-p", "mx-ports", "--locked", "--offline", "--no-fail-fast"], cwd=copy / "rust",
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
    if "Compiling mx-run" not in out and "Compiling mx-ports" not in out:
        return "NOT_REBUILT", names, passed, failed, out
    return ("RED" if failed else "GREEN"), names, passed, failed, out


def check_anchors():
    bad = 0
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
    scratch = Path(os.environ.get("MX_SCRATCH", Path.home() / ".cache" / "magent-wt-scratch")) / "mx-run-mutants"
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
