#!/usr/bin/env python3
"""Mutation table for mx-doctor.

Each mutant changes one guarded line in a COPY of the tree, then runs the crate's
tests. A mutant is KILLED when at least one test fails by assertion (or by the
fake's refusal to answer a call nobody scripted) and the build still compiles.
It SURVIVES when every test passes, and it is a BUILD_ERR (counted separately,
never as a kill) when it does not compile.

Procedure, per the repository's verification rules:
  * the work copy is made from a pristine copy, every file is touched before
    each run so no cache reuses an older result, and each run is capped by time
    and by address space;
  * the mutation is read back from the file (the old text must occur exactly
    once and the new text must be there afterwards) before the run;
  * the file is restored by `cp` from the pristine copy and its sha256 is
    compared before moving on;
  * the pristine control runs before the batch and after it, and must pass
    with the same count of tests.

Usage: mutants.py --scratch DIR [--only ID] [--list]
Exit 0 when the controls pass, no mutant survived and none failed to build.
"""
import argparse
import hashlib
import os
import re
import resource
import shutil
import signal
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
RUST = os.path.abspath(os.path.join(HERE, "../../.."))      # .../rust
REPO = os.path.abspath(os.path.join(RUST, ".."))

C = "crates/mx-doctor/src/"

PROFILE_TESTS = ["cargo", "test", "-p", "mx-profile", "--offline", "--locked", "--no-fail-fast", "--test", "load", "--test", "load_for_doctor"]

# (id, category, file, old, new)
MUTANTS = [
    ("thr-nested", "threshold", C + "install.rs", "if nested <= 2 {", "if nested < 2 {"),
    ("thr-floor", "threshold", C + "install.rs", "i64::try_from(n).is_ok_and(|n| n < floor)",
     "i64::try_from(n).is_ok_and(|n| n <= floor)"),
    ("thr-age-grace", "threshold", C + "github.rs", "if age >= grace {", "if age > grace {"),
    ("thr-lease", "threshold", C + "audit.rs", "if age > limit {", "if age >= limit {"),
    ("thr-token-len", "threshold", C + "audit.rs", "if token.len() < 16 {", "if token.len() <= 16 {"),
    ("thr-five", "threshold", C + "audit.rs", "found.iter().take(5)", "found.iter().take(6)"),
    ("thr-tie", "boundary", C + "audit.rs", "failed.mtime <= last.mtime", "failed.mtime < last.mtime"),
    ("thr-min-hours", "threshold", C + "audit.rs", "min > 0 && since < min", "min > 0 && since <= min"),
    ("msg-invalid-json", "message", C + "install.rs", '"deny wall is not valid JSON"',
     '"deny wall is invalid JSON"'),
    ("msg-not-installed", "message", C + "install.rs", 'Item::fail("not installed"',
     'Item::fail("not installed here"'),
    ("msg-fix-reinstall", "message", C + "hostchecks.rs", '"./install.sh; without it a hardened timer cannot start a container"',
     '"./install.sh"'),
    ("msg-linger-fix", "message", C + "hostchecks.rs", '"sudo loginctl enable-linger {who}"',
     '"sudo loginctl enable-linger"'),
    ("msg-release-space", "message", C + "audit.rs", 'format!("maintainer lease release {numbers}")',
     'format!("maintainer lease release {}", numbers.trim_end())'),
    ("msg-summary", "message", C + "report.rs", '{ok} ok, {warn} warning(s), {fail} failure(s)',
     '{ok} ok, {warn} warnings, {fail} failure(s)'),
    ("exit-fail", "exit-code", C + "report.rs", "i32::from(self.counts().2 != 0)", "i32::from(self.counts().2 == 0)"),
    ("exit-unbound", "exit-code", C + "ctx.rs", "Err(Stop::Exit(1))", "Err(Stop::Exit(127))"),
    ("exit-name", "exit-code", C + "run.rs", "is not a plain profile name\\n\"), 64));", "is not a plain profile name\\n\"), 1));"),
    ("order-audit", "ordering", C + "audit.rs", "    receipts_check(ctx, out, &st);\n    leases_check(ctx, out, &st)?;",
     "    leases_check(ctx, out, &st)?;\n    receipts_check(ctx, out, &st);"),
    ("order-sort-glob", "ordering", C + "run.rs", 'names.sort_by_key(|name| format!("{name}/profile.env"));',
     "names.sort();"),
    ("order-leases", "ordering", C + "audit.rs", "forgotten_leases(hours, &files, now)",
     "{ let mut reversed = files; reversed.reverse(); forgotten_leases(hours, &reversed, now) }"),
    ("os-run-instance", "per-os", C + "install.rs", 'name == "run-instance.sh" && ctx.uname() != "Linux"',
     'name == "run-instance.sh"'),
    ("os-darwin-literal", "per-os", C + "hostchecks.rs", "line.windows(needle.len()).any(|w| w == needle)",
     "line.len() >= needle.len()"),
    ("os-linux-warmup", "per-os", C + "hostchecks.rs", 'if ctx.uname() == "Linux" && ctx.host.stat(&unit).is_file() {',
     'if ctx.host.stat(&unit).is_file() {'),
    ("os-sandbox-linux", "per-os", C + "hostchecks.rs", 'if ctx.uname() != "Linux"\n        || !ctx.host.stat(&unit).is_file()',
     'if !ctx.host.stat(&unit).is_file()'),
    ("tok-skip-drafts", "token-scan", C + "audit.rs", '["logs", "runs", "drafts", "state", "receipts"]',
     '["logs", "runs", "state", "receipts"]'),
    ("tok-skip-session", "token-scan", C + "audit.rs", "if ctx.host.stat(&session).is_dir() {",
     "if false {"),
    ("tok-skip-receipts", "token-scan", C + "audit.rs", '["logs", "runs", "drafts", "state", "receipts"]',
     '["logs", "runs", "drafts", "state"]'),
    ("tok-argv", "token-scan", C + "audit.rs", "let found = ctx.host.scan_for(&roots, &patterns);",
     'drop(ctx.run(&["grep", "-rlaF", "-e", &String::from_utf8_lossy(&token), "/dev/null"], limit::GH));\n    let found = ctx.host.scan_for(&roots, &patterns);'),
    ("tok-newline-patterns", "token-scan", C + "audit.rs", "token.split(|byte| *byte == b'\\n')",
     "std::iter::once(token.as_slice())"),
    ("tok-empty-fallback", "token-scan", C + "audit.rs", "if places.is_empty() {\n        places = st.to_string();\n    }",
     ""),
    ("lease-bool", "lease-boundary", C + "audit.rs", "Some(Value::Bool(flag)) => Some(f64::from(u8::from(*flag))),",
     "Some(Value::Bool(_)) => Some(0.0),"),
    ("lease-string-int", "lease-boundary", C + "audit.rs", "Some(Value::String(text)) => text::py_int_text(text),",
     "Some(Value::String(_)) => None,"),
    ("stale-hours", "stale-run", C + "audit.rs", "(now - then) / 3600", "(now - then) / 3600 + 1"),
    ("stale-newest", "stale-run", C + "github.rs", "if created(run)?.newer_than(&created(slot.1)?)? {",
     "if !created(run)?.newer_than(&created(slot.1)?)? {"),
    ("stale-completed", "stale-run", C + "github.rs", "if !completed || !failed {", "if !completed && !failed {"),
    ("stale-empty-list", "stale-run", C + "github.rs", ".filter(|list| !list.is_empty())", ".filter(|_| true)"),
    ("img-stamp", "image", C + "hostchecks.rs", "if recipe.stamp == recipe.hash {", "if recipe.stamp != recipe.hash {"),
    ("img-probe-code", "image", C + "hostchecks.rs", "if probe.code != 0 {", "if probe.code > 200 {"),
    ("img-trailing-space", "image", C + "hostchecks.rs", "probe.missing.strip_suffix(' ').unwrap_or(&probe.missing)",
     "probe.missing.as_str()"),
    ("img-novalue", "image", C + "hostchecks.rs", 'recipe.stamp.is_empty() || recipe.stamp == "<no value>"',
     "recipe.stamp.is_empty()"),
    ("sbx-cleanup", "sandbox", C + "hostchecks.rs", "ctx.host.remove_tree(&sb);", ""),
    ("sbx-no-directive", "sandbox", C + "hostchecks.rs", "(text::captured(&joined), i32::from(found.is_empty()))",
     "(text::captured(&joined), 0)"),
    ("sbx-head2", "sandbox", C + "text.rs", "if seen == n {", "if seen == n + 1 {"),
    ("txt-strip", "text", C + "text.rs", "while end > 0 && bytes[end - 1] == b'\\n' {",
     "while end > 0 && bytes[end - 1] == b'\\n' && end > 1 {"),
    ("txt-seconds-unit", "text", C + "text.rs", "rest.get(digits) == Some(&b's')", "rest.get(digits) == Some(&b'm')"),
    ("txt-dot-wildcard", "text", C + "text.rs", "*want == b'.' || line[at + i] == *want", "line[at + i] == *want"),
    ("txt-round-half", "text", C + "text.rs", 'format!("{value:.0}")', 'format!("{:.0}", value + 0.01)'),
    ("py-zone-sign", "text", C + "pyval.rs", "- sign * off_secs;", "+ sign * off_secs;"),
    ("host-nested-root", "per-os", C + "host.rs",
     "if root_path.file_name().is_some_and(|base| names.iter().any(|name| base == *name)) {\n                count += 1;\n            }",
     ""),
    ("prof-ambient", "profile", C + "run.rs", '"HOME" && valid_name(name)', '"HOME" && false && valid_name(name)'),
    # --- the flock check (bash PR #243), added after the port ---
    ("flock-ok-text", "message", C + "install.rs", 'Item::ok("flock on PATH")', 'Item::ok("flock on the PATH")'),
    ("flock-fail-text", "message", C + "install.rs", '"flock is required to serialize runs and is NOT on PATH"',
     '"flock is required to serialise runs and is NOT on PATH"'),
    ("flock-fix-text", "message", C + "install.rs", '"install flock for this platform"', '"install flock"'),
    ("flock-inverted", "branch", C + "install.rs", 'ctx.command_v("flock").is_some()', 'ctx.command_v("flock").is_none()'),
    ("flock-always-ok", "branch", C + "install.rs", 'ctx.command_v("flock").is_some()',
     'ctx.command_v("flock").is_some() || true'),
    ("flock-wrong-name", "branch", C + "install.rs", 'ctx.command_v("flock")', 'ctx.command_v("flocks")'),
    ("flock-fail-to-warn", "severity", C + "install.rs",
     'Item::fail(\n            "flock is required', 'Item::warn(\n            "flock is required'),
    ("flock-ok-to-warn", "severity", C + "install.rs", 'Item::ok("flock on PATH")', 'Item::warn("flock on PATH", "x")'),
    ("flock-ok-to-fail", "severity", C + "install.rs", 'Item::ok("flock on PATH")', 'Item::fail("flock on PATH", "x")'),
    ("flock-early-return", "order", C + "install.rs",
     '"install flock for this platform",\n        ));\n', '"install flock for this platform",\n        ));\n        return;\n'),
    ("flock-heading-gone", "order", C + "install.rs",
     '    out.add(Item::head("Backend"));\n    // bash PR #243', '    // bash PR #243'),
    # --- stderr line numbers moved by two when bash gained the flock lines ---
    ("flockline-run-limit", "unbound", C + "github.rs", 'line 189: DOCTOR_RUN_LIMIT', 'line 187: DOCTOR_RUN_LIMIT'),
    ("flockline-grace", "unbound", C + "github.rs", 'need("RED_GRACE_HOURS", 187,', 'need("RED_GRACE_HOURS", 185,'),
    ("flockline-lease-warn", "unbound", C + "audit.rs", 'line 454: DOCTOR_LEASE_HOURS', 'line 452: DOCTOR_LEASE_HOURS'),
    ("flockline-invalid-name", "unbound", C + "audit.rs", 'line 532: {variable}', 'line 530: {variable}'),
    ("flockline-min-hours", "unbound", C + "audit.rs", 'integer_expected(533,', 'integer_expected(531,'),
    # --- the review's fixes ---
    ("rv-exit-home", "exit-code", C + "run.rs", 'HOME: unbound variable\\n", 1)', 'HOME: unbound variable\\n", 127)'),
    ("rv-run-limit", "unbound", C + "github.rs", 'if ctx.settings.is_set("DOCTOR_RUN_LIMIT") {', "if true {"),
    ("rv-lease-limit-line", "unbound", C + "audit.rs", "if found.is_empty() { 463 } else { 460 }", "if found.is_empty() { 440 } else { 460 }"),
    ("rv-scan-overlap", "scan-stream", C + "host.rs", "max().unwrap_or(0).saturating_sub(1);", "max().unwrap_or(0).saturating_sub(2);"),
    ("rv-scan-chunk", "scan-stream", C + "host.rs", "pub const SCAN_CHUNK: usize = 64 * 1024;", "pub const SCAN_CHUNK: usize = 1024 * 1024;"),
    ("rv-py-wall", "python", C + "install.rs", "if ctx.python_available() { rule_count(contents) } else { None }", "rule_count(contents)"),
    ("rv-py-runs", "python", C + "github.rs", "if ctx.python_available() {\n        verdicts(", "if true {\n        verdicts("),
    ("rv-py-leases", "python", C + "audit.rs", "let read = if ctx.python_available() {", "let read = if true {"),
    ("rv-py-reason", "python", C + "audit.rs", "let mut why = if ctx.python_available() {", "let mut why = if true {"),
    ("rv-live-stdout", "stream", C + "ctx.rs", "self.send(Stream::Stdout, &bytes);", "let _ = &bytes;"),
    ("rv-live-stderr", "stream", C + "ctx.rs", "        self.send(Stream::Stderr, bytes);", "        let _ = bytes;"),
    ("rv-closed", "stream", C + "ctx.rs", "self.closed = true;", "self.closed = false;"),
    ("rv-141", "exit-code", C + "run.rs", "if out.closed() { 141 } else { code }", "if out.closed() { 142 } else { code }"),
    ("rv-bytes", "bytes", C + "text.rs", "char::from_u32(ESCAPE_BASE + u32::from(*byte)).unwrap_or('\\u{FFFD}')", "'\\u{FFFD}'"),
    ("rv-mkdir-msg", "stderr", C + "hostchecks.rs", 'out.warn_text(&format!("mkdir: {why}\\n"));', ""),
    ("rv-git-stderr", "stderr", C + "hostchecks.rs", "out.raw_stderr(&call.stderr);", ""),
    ("rv-mktemp-msg", "stderr", C + "hostchecks.rs", '"mktemp: failed to create directory via template', '"mktemp: failed to make a directory via template'),
    ("rv-sha256sum", "image", C + "hostchecks.rs", 'if present && ctx.command_v("sha256sum").is_some() =>', "if present =>"),
    ("rv-nan", "json", C + "pyval.rs", 'if rest.starts_with("NaN") {', "if false {"),
    ("rv-date-year0", "date", C + "pyval.rs", "if year < 1 {", "if year < 0 {"),
    ("rv-date-24", "date", C + "pyval.rs", "(days + 1, 0)", "(days, 0)"),
    ("rv-date-leap", "date", C + "pyval.rs", "2 => 28 + i64::from(leap),", "2 => 29,"),
    ("rv-date-week", "date", C + "pyval.rs", "if !(1..=53).contains(&week) ||", "if false ||"),
    ("rv-bash-env", "profile-env", C + "run.rs", '"BASH_ENV",', '"BASH_ENV_",'),
    ("rv-lease-glob", "lease-boundary", C + "audit.rs", 'glob_names(ctx.host, &dir, "pr-", ".json")', 'glob_names(ctx.host, &dir, "pr", ".json")'),
    ("rv-linger", "per-os", C + "hostchecks.rs", 'call.text() == "yes"', 'call.text().starts_with("y")'),
    ("rv-noting", "missing-log", C + "run.rs", "if let Err(mx_ports::ProcError::NotFound { program }) = &result {", "if let Err(mx_ports::ProcError::Spawn { program, .. }) = &result {"),
    ("rv-lease-first-line", "unbound", C + "audit.rs", "line 454: DOCTOR_LEASE_HOURS", "line 455: DOCTOR_LEASE_HOURS"),
    ("rv-leap-400", "date", C + "pyval.rs", "year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)", "year % 4 == 0"),
    ("rv-leap-100", "date", C + "pyval.rs", "year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)", "year % 4 == 0 && year % 100 != 0"),
    ("rv-interrupted", "scan-stream", C + "host.rs", "Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,", "Err(error) if false => continue,"),
    ("rv-profile-exit", "exit-code", "crates/mx-profile/src/shell.rs", "let code = if code == 127 && unbound { 1 } else { code };", "let code = if code == 127 && unbound { 127 } else { code };", PROFILE_TESTS),
    ("rv-profile-exit-enum", "exit-code", "crates/mx-profile/src/load.rs", "            Self::Unbound { .. }\n            | Self::DefaultsMissing", "            Self::Unbound { .. } => 127,\n            Self::DefaultsMissing", PROFILE_TESTS),
    ("rv-profile-home", "unbound", "crates/mx-profile/src/load.rs", "    // `$HOME` is the first expansion of the defaults search, as in `resolve`.\n    if env.home.is_none() {\n        return Err(LoadError::HomeUnbound);\n    }\n    let defaults = defaults_path(env).ok_or(LoadError::DefaultsMissing)?;\n    let first = evaluate(proc, env, std::slice::from_ref(&defaults));\n    let after_defaults = match &first.outcome {\n        Ok(dump) if dump.last_status == 0 => dump.vars.clone(),\n        Ok(dump) => {\n            shown.keep(&first);\n            return Err(LoadError::DefaultsFailed { status: dump.last_status });", "    let defaults = defaults_path(env).ok_or(LoadError::DefaultsMissing)?;\n    let first = evaluate(proc, env, std::slice::from_ref(&defaults));\n    let after_defaults = match &first.outcome {\n        Ok(dump) if dump.last_status == 0 => dump.vars.clone(),\n        Ok(dump) => {\n            shown.keep(&first);\n            return Err(LoadError::DefaultsFailed { status: dump.last_status });", PROFILE_TESTS),
]


TEST_CMD = ["cargo", "test", "-p", "mx-doctor", "--offline", "--locked", "--no-fail-fast"]


def sha(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def touch_all(root):
    now = time.time()
    for dirpath, _dirs, files in os.walk(root):
        for name in files:
            os.utime(os.path.join(dirpath, name), (now, now))


def limits():
    resource.setrlimit(resource.RLIMIT_AS, (8 << 30, 8 << 30))
    os.setsid()


def run_tests(work, env, cmd=None, timeout=1500):
    proc = subprocess.Popen(cmd or TEST_CMD, cwd=work + "/rust", env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, preexec_fn=limits)
    try:
        out, _ = proc.communicate(timeout=timeout)
        return proc.returncode, out.decode("utf-8", "replace"), False
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        out, _ = proc.communicate()
        return -9, out.decode("utf-8", "replace"), True


def summarize(out):
    passed = sum(int(m) for m in re.findall(r"test result: \w+\. (\d+) passed", out))
    failed_names = re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M) + re.findall(r"^FAIL  (.*)$", out, re.M)
    panics = re.findall(r"panicked at [^\n]*\n([^\n]*)", out)
    build_err = "could not compile" in out or re.search(r"^error(\[E\d+\])?:", out, re.M) is not None
    return passed, failed_names, panics, build_err


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scratch", required=True)
    ap.add_argument("--only", help="run the mutants whose id starts with this text")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--check", action="store_true", help="only verify that every old text is unique")
    a = ap.parse_args()
    if a.list:
        for m in MUTANTS:
            print(m[0], m[1], m[2])
        print(len(MUTANTS), "mutants")
        return 0
    pristine = a.scratch + "/pristine"
    work = a.scratch + "/work"
    target = a.scratch + "/target"
    for d in (pristine, work):
        shutil.rmtree(d, ignore_errors=True)
    os.makedirs(pristine)
    shutil.copytree(RUST, pristine + "/rust", ignore=shutil.ignore_patterns("target"))
    shutil.copytree(REPO + "/lib", pristine + "/lib")
    # every mutant's old text must occur exactly once in the pristine file
    bad = []
    for mid, _cat, rel, old, new, *_rest in MUTANTS:
        text = open(pristine + "/rust/" + rel).read()
        if text.count(old) != 1:
            bad.append((mid, text.count(old)))
    if bad:
        print("mutants whose old text is not unique in the pristine file:", bad)
        return 2
    if a.check:
        print("every mutant's old text is unique: %d mutants" % len(MUTANTS))
        return 0
    env = dict(os.environ)
    env.update({"CARGO_BUILD_JOBS": "2", "CARGO_TARGET_DIR": target, "CARGO_TERM_COLOR": "never"})
    results = []

    def control(label):
        shutil.rmtree(work, ignore_errors=True)
        shutil.copytree(pristine, work)
        touch_all(work)
        rc, out, timed_out = run_tests(work, env)
        passed, failed, _p, build_err = summarize(out)
        ok = rc == 0 and not failed and passed > 100 and not timed_out
        print("CONTROL %s: rc=%s tests passed=%d failed=%d -> %s" % (label, rc, passed, len(failed), "ok" if ok else "BAD"))
        return ok, passed

    ok_before, n_before = control("before")
    if not ok_before:
        return 1
    survivors = build_errs = 0
    for mid, cat, rel, old, new, *rest in MUTANTS:
        cmd = rest[0] if rest else None
        if a.only and not mid.startswith(a.only):
            continue
        shutil.rmtree(work, ignore_errors=True)
        shutil.copytree(pristine, work)
        path = work + "/rust/" + rel
        pristine_sha = sha(pristine + "/rust/" + rel)
        text = open(path).read()
        assert text.count(old) == 1
        mutated = text.replace(old, new, 1)
        assert mutated != text, mid
        with open(path, "w") as f:
            f.write(mutated)
        readback = open(path).read()
        assert new in readback and readback == mutated and sha(path) != pristine_sha, mid + ": mutation did not land"
        touch_all(work)
        rc, out, timed_out = run_tests(work, env, cmd)
        passed, failed, panics, build_err = summarize(out)
        if timed_out:
            outcome = "CAPPED"
        elif build_err and not failed and passed == 0:
            outcome = "BUILD_ERR"
            build_errs += 1
        elif failed:
            outcome = "KILLED"
        else:
            outcome = "SURVIVED"
            survivors += 1
        kinds = set()
        for p in panics:
            # An assert! with a custom message prints that message, not "assertion failed".
            if "unscripted" in p:
                kinds.add("unscripted-call")
            elif "unwrap" in p or "expect" in p or "called `" in p:
                kinds.add("panic")
            else:
                kinds.add("assertion")
        results.append((mid, cat, outcome, len(failed), sorted(kinds), failed[:2]))
        print("%-22s %-14s %-9s failing=%d %s %s" % (mid, cat, outcome, len(failed), sorted(kinds), failed[:2]))
        sys.stdout.flush()
        # restore from the pristine copy and prove it
        shutil.copyfile(pristine + "/rust/" + rel, path)
        assert sha(path) == pristine_sha, mid + ": restore failed"
    ok_after, n_after = control("after")
    killed = sum(1 for r in results if r[2] == "KILLED")
    print("mutants: %d, killed: %d, survived: %d, build errors: %d" % (len(results), killed, survivors, build_errs))
    ok = ok_before and ok_after and n_before == n_after and survivors == 0 and build_errs == 0
    print("RESULT: %s" % ("PASS" if ok else "FAIL"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
