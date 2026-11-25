#!/usr/bin/env python3
"""Mutation proof for mx-suite and mx-sandbox (p3a).

  python3 mutants.py          every mutant
  python3 mutants.py M03 M17  only these
  python3 mutants.py --check  only check that every anchor occurs exactly once

Each mutant is applied to a fresh copy of the workspace (the checkout is never edited): every
file is touched, the old text must occur exactly once, the edit is read back, and `cargo test -p
mx-suite -p mx-sandbox` runs in a private target directory under a time limit and an
address-space cap. KILLED means the crate built and tests failed; BUILD_ERR, TIMEOUT, CAPPED,
NOT_REBUILT and SURVIVED are never counted as kills. The pristine copy runs before and after, and
the checkout's tree hash is compared before and after.
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
RUST = HERE.parent.parent.parent
S = "crates/mx-suite/src/"
B = "crates/mx-sandbox/src/"
MUTANTS = [
 ("M01", S+"choose.rs", "            if !self.covered_by_any(all, q)? {\n                return Ok(None);\n            }\n        }\n        let paths", "            if false {\n                return Ok(None);\n            }\n        }\n        let paths", "no-production narrowing skips the any-suite check"),
 ("M02", S+"choose.rs", "        if runs < 1 {", "        if runs < 0 {", "a suite that runs none of the diff is accepted"),
 ("M03", S+"choose.rs", "        if changed.iter().any(|p| globs.matches(p)) {", "        if false {", "no-production narrowing applies to a diff with production paths"),
 ("M04", S+"choose.rs", "Ok(name) if on_disk.contains(&name) => chosen.push(name),", "Ok(name) => chosen.push(name),", "a receipt suite that is not on disk is not refused as such"),
 ("M05", S+"choose.rs", "        if names.is_empty() {", "        if false {", "a receipt naming no suite is not refused"),
 ("M06", S+"choose.rs", "            if self.host.inferable(name).map_err(PickRefusal::Suite)? {", "            if self.host.inferable(name).map_err(PickRefusal::Suite)? || true {", "inference picks a named-only suite"),
 ("M07", S+"choose.rs", "            if self.host.inferable(name)? {", "            if self.host.inferable(name)? || true {", "the coverage map lists a named-only suite"),
 ("M08", S+"choose.rs", "        between.sort();\n", "", "the suites between them are not sorted"),
 ("M09", S+"choose.rs", "        between.dedup();\n", "", "the suites between them repeat"),
 ("M10", S+"choose.rs", "r.suite.is_none()).count()", "r.suite.is_some()).count()", "orphans counts the covered paths"),
 ("M11", S+"choose.rs", "                unproven.len()\n            );", "                rest.len()\n            );", "the unproven count is the whole rest"),
 ("M12", S+"choose.rs", "let runs = paths.len() - unproven.len();", "let runs = paths.len();", "runs counts every path"),
 ("M13", S+"choose.rs", "            if !self.host.covers(suite, path)? {\n                return Ok(false);", "            if !self.host.covers(suite, path)? {\n                return Ok(true);", "covers_all stops at a miss with yes"),
 ("M14", S+"choose.rs", '            notes.push_str("  CI still gates them.\\n");', "", "the no-production note loses its last line"),
 ("M15", S+"real.rs", "digits.parse::<i64>().map_or(0, i64::unsigned_abs)", "digits.parse::<u64>().unwrap_or(0)", "a count above i64 is accepted"),
 ("M16", S+"real.rs", "printed.trim_matches([' ', '\\t'])", "printed", "blanks around the count are not trimmed"),
 ("M17", S+"real.rs", 'const QUESTION: &str = r#"set -uo pipefail', 'const QUESTION: &str = r#"set -o pipefail', "suites run without nounset"),
 ("M18", S+"real.rs", "declare -F suite_image >/dev/null || exit 79\n", "", "a suite with no image is not named as such"),
 ("M19", S+"real.rs", "        if file.is_file() {\n            Ok((dir, file))", "        if true {\n            Ok((dir, file))", "a missing file is asked about"),
 ("M20", S+"real.rs", "                && entry.path().is_file()", "", "a directory named x.sh is a suite"),
 ("M21", S+"real.rs", "        names.sort();\n        Ok(names)", "        Ok(names)", "names are not sorted"),
 ("M22", S+"real.rs", '        all.retain(|d| d != "/verify.d");\n', "", "an empty PROFILE_DIR is a candidate"),
 ("M23", S+"real.rs", "text.trim_end_matches('\\n')", "text.trim_end()", "command substitution strips blanks too"),
 ("M24", S+"real.rs", "const EXIT_NEEDS_OTHER: i32 = 77;", "const EXIT_NEEDS_OTHER: i32 = 76;", "requires says no with another code"),
 ("M25", S+"real.rs", "EXIT_UNDEFINED => Ok(None),", "EXIT_UNDEFINED => Ok(Some(String::new())),", "an undefined limit reads as empty"),
 ("M26", S+"real.rs", "        if code(&out) == 0 {\n            return Ok(());", "        if code(&out) != 0 {\n            return Ok(());", "a refusing guard passes"),
 ("M27", S+"real.rs", 'guarded: flag == "2",', 'guarded: flag == "1",', "a guarded prefetch reads as unguarded"),
 ("M28", S+"real.rs", 'let named_only = text(3, "named_only")? == "1";', 'let named_only = text(3, "named_only")? == "0";', "named-only inverted"),
 ("M29", S+"real.rs", '            spec = spec.env("work", work.display().to_string());', "", "work is not passed to suites"),
 ("M30", S+"real.rs", "        if out.exit == ProcExit::TimedOut {", "        if false {", "a timed-out question reads as an answer"),
 ("M31", B+"real.rs", '!record.starts_with(b"160000 ")', "true", "a submodule counts as a file"),
 ("M32", B+"real.rs", "kind.is_file() || kind.is_symlink()", "kind.is_file()", "links are not counted on disk"),
 ("M33", B+"real.rs", "        if rev.starts_with('-') {", "        if false {", "a revision that looks like an option reaches git"),
 ("M34", B+"real.rs", "        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))\n            .map_err(|_| parent_err())?;", "", "the scratch parent is not locked to 700"),
 ("M35", B+"real.rs", "DirBuilder::new().mode(0o700)", "DirBuilder::new().mode(0o755)", "scratch directories are world readable"),
 ("M36", B+"real.rs", "let inside = target == root || target.starts_with(root);", "let inside = true;", "no link is an escape"),
 ("M37", B+"real.rs", '["find", ".", "-type", "f", "("]', '["find", ".", "-type", "l", "("]', "the mutation reaches links, not files"),
 ("M38", B+"real.rs", '"./.container-target/*"', '"./.container-targetx/*"', "the container target is mutated"),
 ("M39", B+"real.rs", '"./.git/*"', '"./.gitx/*"', "the git directory is mutated"),
 ("M40", B+"real.rs", '["sed", "--sandbox", "-i", "-e"]', '["sed", "-i", "-e"]', "sed runs without --sandbox"),
 ("M41", B+"real.rs", "before.get(*n) != after.get(*n)", "before.get(*n) == after.get(*n)", "unchanged files are reported"),
 ("M43", B+"real.rs", "Permissions::from_mode(0o755))\n            .map_err(|e| stage_err(&dest, e))?;", "Permissions::from_mode(0o644))\n            .map_err(|e| stage_err(&dest, e))?;", "the staged file is not executable"),
 ("M44", B+"real.rs", "            Ok(_) => std::fs::remove_file(&dest).map_err(|e| stage_err(&dest, e))?,", "            Ok(_) => {}", "a leftover staged file or link is not removed"),
 ("M45", B+"real.rs", "            && !name.starts_with(['.', '-']);", "            ;", "a staged name may start with a dot or dash"),
 ("M46", B+"real.rs", "        if spec.log_mode() == LogMode::Truncate {", "        if false {", "the prefetch log is not started over"),
 ("M47", B+"real.rs", "ProcExit::TimedOut => Ok(ContainerExit(TIMED_OUT)),", "ProcExit::TimedOut => Ok(ContainerExit(0)),", "a hung client reads as success"),
 ("M48", B+"real.rs", "        stage.env.clone_from(&self.runtime_env);\n", "", "the runtime gets no environment"),
 ("M49", B+"real.rs", "stage.stderr = Sink::SameAsStdout;", "stage.stderr = Sink::Null;", "standard error leaves the log"),
 ("M50", B+"real.rs", "+ self.grace,", "+ Duration::from_secs(0),", "the grace is ignored"),
 ("M51", B+"real.rs", "self.scratch.lock().unwrap_or_else(PoisonError::into_inner).push(dir.clone());", "", "scratch dirs are never removed"),
 ("M52", B+"real.rs", "if found(\"podman\") {\n        Some(Runtime::Podman)\n    } else if found(\"docker\") {\n        Some(Runtime::Docker)", "if found(\"docker\") {\n        Some(Runtime::Docker)\n    } else if found(\"podman\") {\n        Some(Runtime::Podman)", "docker preferred to podman"),
 ("M53", B+"real.rs", "m.is_file() && m.permissions().mode() & 0o111 != 0", "m.is_file()", "a non-executable file is a runtime"),
 ("M54", B+"real.rs", '"add", "-A", "-f"', '"add", "-A"', "the index honours .gitignore"),
 ("M55", B+"real.rs", '                "init.templateDir=",\n', '                "init.x=",\n', "host git templates reach the tree"),
 ("M56", B+"real.rs", "        std::fs::create_dir_all(tree.root().join(\".container-target\")).map_err(|e| {", "        std::fs::create_dir_all(tree.root().join(\".container-targetx\")).map_err(|e| {", "no container target directory"),
 ("M57", B+"real.rs", '("GIT_INDEX_FILE".to_string(), text(&index)?.to_string())', '("GIT_INDEX_FILEX".to_string(), text(&index)?.to_string())', "extraction uses the repository index"),
 ("M58", B+"realpath.rs", "        if target.first() == Some(&b'/') {", "        if target.first() == Some(&b'.') {", "an absolute target does not restart the path"),
 ("M59", B+"real.rs", "found.sort_by_key(ToString::to_string);", "", "escapes are not sorted"),
 ("M61", B+"real.rs", "            Ok(meta) if !meta.is_file() => {", "            Ok(meta) if false && !meta.is_file() => {", "a link at the log path is written through"),
 ("M62", B+"real.rs", "        ensure_plain_dir(&target)?;\n", "", "a linked .container-target is followed"),
 ("M63", B+"real.rs", "        ensure_plain_dir(&dir)?;\n", "", "a linked stage directory is followed"),
 ("M64", B+"real.rs", "Ok(meta) if meta.is_dir() => Ok(()),", "Ok(_) => Ok(()),", "any entry counts as a plain directory"),
 ("M65", B+"real.rs", "            if !plain || sha256_of(&path).ok() != Some(hash) {", "            if false && (!plain || sha256_of(&path).ok() != Some(hash)) {", "a replaced staged binary is run"),
 ("M66", B+"real.rs", "        staged.retain(|(path, _)| path != &dest);\n", "", "staging again leaves the old hash recorded"),
 ("M67", B+"real.rs", "        changed.sort();\n", "", "changed paths are in find order"),
 ("M68", B+"real.rs", '.field("runtime_env_names", &names)', '.field("runtime_env", &self.runtime_env)', "Debug prints the values of the runtime environment"),
 ("M69", B+"error.rs", "that\\n  asks git what is tracked will fail for that reason and not for the code's.\"", "that\\n  asks git what is tracked will fail for that reason.\"", "the index note loses its end"),
 ("M70", S+"prod.rs", "        b'?' => !text.is_empty() && full(&pattern[1..], &text[1..], m),", "        b'?' => !text.is_empty() && full(&pattern[1..], &text[1..], m) && false,", "? never matches"),
 ("M71", S+"prod.rs", '        b"alpha" => byte.is_ascii_alphabetic(),', '        b"alpha" => byte.is_ascii_alphanumeric(),', "[:alpha:] holds digits"),
 ("M72", S+"prod.rs", "        _ => (0..=text.len()).any(|k| !one(&text[..k], m) && full(rest, &text[k..], m)),", "        _ => (0..=text.len()).any(|k| one(&text[..k], m) && full(rest, &text[k..], m)),", "!(..) matches what the group matches"),
 ("M73", S+"prod.rs", "    if at_least_one {\n        // One round", "    if false {\n        // One round", "+(..) matches zero rounds"),
 ("M74", S+"prod.rs", "                        None => return Class::Unknown,", "                        None => return Class::Miss,", "an unknown class name answers not production"),
 ("M75", S+"prod.rs", "            .split([' ', '\\t', '\\n', '\\x0b', '\\x0c', '\\r'])", "            .split([' ', '\\t'])", "only blanks separate patterns"),
 ("M76", S+"prod.rs", "    m.steps += 1;\n    if m.steps > STEP_LIMIT || m.depth >= DEPTH_LIMIT {\n        m.tripped = true;\n        return true;", "    m.steps += 1;\n    if m.steps > STEP_LIMIT || m.depth >= DEPTH_LIMIT {\n        return false;", "a match past the step limit answers not production"),
 ("M77", S+"prod.rs", "    negations == 1 && outside_group_has_star(pattern)", "    false", "a negation beside a star is decided"),
 ("M78", S+"prod.rs", "    if negations >= 2 {", "    if negations >= 3 {", "two negations are decided"),
 ("M79", S+"prod.rs", "    if !text.is_ascii() && pattern.iter()", "    if false && pattern.iter()", "a wildcard against non-ASCII text is decided on bytes"),
 ("M80", S+"prod.rs", "        b\"blank\" => matches!(byte, b' ' | b'\\t'),", "        b\"blank\" => matches!(byte, b' '),", "[:blank:] loses the tab"),
 ("M81", S+"prod.rs", "        b\"word\" => byte.is_ascii_alphanumeric() || byte == b'_',", "        b\"word\" => byte.is_ascii_alphanumeric(),", "[:word:] loses the underscore"),
 ("M82", S+"prod.rs", "        b\"xdigit\" => byte.is_ascii_hexdigit(),", "        b\"xdigit\" => byte.is_ascii_digit(),", "[:xdigit:] is digits only"),
 ("M83", S+"prod.rs", "        b\"punct\" => byte.is_ascii_punctuation(),", "        b\"punct\" => byte.is_ascii_graphic(),", "[:punct:] holds letters"),
 ("M84", S+"prod.rs", "            b'\\\\' => i += 1,\n            b'[' => {\n                // a bracket expression inside the group", "            b'[' => {\n                // a bracket expression inside the group", "a backslash does not protect the next byte inside a group"),
 ("M85", S+"prod.rs", "        b'?' => !text.is_empty() && full(&pattern[1..], &text[1..], m),", "        b'?' => text.len() > 1 && full(&pattern[1..], &text[1..], m),", "? needs two bytes"),
 ("M86", S+"prod.rs", "        b\"cntrl\" => byte < 0x20 || byte == 0x7f,", "        b\"cntrl\" => byte < 0x20,", "[:cntrl:] loses DEL"),
 ("M60", B+"real.rs", "let mut bytes = [0_u8; 8];", "let mut bytes = [0_u8; 7];", "scratch names are 7 characters"),
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
    """dest/rust is the workspace; profiles/ is copied for the shipped-suite test."""
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(RUST, dest / "rust", ignore=shutil.ignore_patterns("target", "__pycache__"), symlinks=True)
    shutil.copytree(RUST.parent / "profiles", dest / "profiles", symlinks=True)
    (dest / "bin").mkdir()
    shutil.copy2(RUST.parent / "bin" / "maintainer-merge", dest / "bin" / "maintainer-merge")


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
    proc = subprocess.Popen(["cargo", "test", "-p", "mx-suite", "-p", "mx-sandbox", "--locked", "--offline", "--no-fail-fast"], cwd=copy / "rust",
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
    if "Compiling mx-suite" not in out and "Compiling mx-sandbox" not in out:
        return "NOT_REBUILT", names, passed, failed, out
    return ("RED" if failed else "GREEN"), names, passed, failed, out



def check_anchors():
    bad = 0
    for mid, rel, old, new, note in MUTANTS:
        n = (RUST / rel).read_text().count(old)
        if n != 1 or old == new:
            print("%s: the old text occurs %d times in %s" % (mid, n, rel))
            bad += 1
    print("anchors: %d mutants, %d bad" % (len(MUTANTS), bad))
    return bad == 0

def main():
    args = sys.argv[1:]
    if args == ["--check"]:
        sys.exit(0 if check_anchors() else 1)
    scratch = Path(os.environ.get("MX_SCRATCH", Path.home() / ".cache" / "magent-wt-scratch")) / "mx-p3a-mutants"
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
