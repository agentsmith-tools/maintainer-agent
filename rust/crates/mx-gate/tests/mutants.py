#!/usr/bin/env python3
"""Mutation proof for the wiring of the Rust merge gate (lane p3w).

  python3 mutants.py          every mutant
  python3 mutants.py W03 A07  only these
  python3 mutants.py --check  only check that every anchor occurs exactly once

Each mutant is applied to a fresh copy of the workspace (the checkout is never edited): every file
is touched, the old text must occur exactly once, the edit is read back, and the tests of mx-gate
(library and verify_end_to_end) and mx-suite (library and real_host) run in a private target
directory under a time limit and an address-space cap. KILLED means the crates built and a test
failed; BUILD_ERR, TIMEOUT, CAPPED, NOT_REBUILT and SURVIVED are never counted as kills. The pristine
copy runs before and after, and the checkout's tree hash is compared before and after.

Environment: MX_SCRATCH (default ~/.cache/magent-wt-scratch), CARGO_BUILD_JOBS (default 2).
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
V = "crates/mx-gate/src/verify_run.rs"
A = "crates/mx-gate/src/app.rs"
R = "crates/mx-gate/src/real.rs"
SR = "crates/mx-suite/src/real.rs"

RUN_FIRST = "        self.stage_gate(tree)?;\n        match self.p.sandbox.run_container(spec, log) {"
MUTANTS = [
 ("W01", V, "        if runtime == Runtime::Docker {", "        if runtime == Runtime::Docker && false {", "a docker machine is not refused by name"),
 ("W02", V, "        if runtime == Runtime::Docker {", "        if runtime == Runtime::Podman {", "a podman machine is refused as docker"),
 ("W03", V, "        self.p.suites.set_work(&work);\n", "", "the suite host is never told the extracted tree"),
 ("W04", V, "            self.stage_gate(&tree)?;\n", "", "the prefetch container runs over an unstaged tree"),
 ("W05", V, RUN_FIRST, "        match self.p.sandbox.run_container(spec, log) {", "no container run is staged for"),
 ("W06", V, RUN_FIRST, '        if log.ends_with("clean.log") {\n            self.stage_gate(tree)?;\n        }\n        match self.p.sandbox.run_container(spec, log) {', "only the unmutated run is staged for"),
 ("W07", V, RUN_FIRST + "\n            Ok(exit) => Ok(exit.code()),\n            Err(error) => Err(die(&error.to_string())),\n        }", "        let ran = match self.p.sandbox.run_container(spec, log) {\n            Ok(exit) => Ok(exit.code()),\n            Err(error) => Err(die(&error.to_string())),\n        };\n        self.stage_gate(tree)?;\n        ran", "the staging comes after the container run"),
 ("W08", V, "self.p.stage.stage(tree).map_err(|error| die(&error.to_string()))", "self.p.stage.stage(tree).or_else(|_| Ok(()))", "a staging that failed does not stop the run"),
 ("W09", R, "        self.sandbox.stage_executable(tree, &self.exe, STAGED_NAME).map(drop)", "        let _ = (tree, &self.exe, STAGED_NAME, self.sandbox);\n        Ok(())", "the real stage copies nothing"),
 ("W10", R, 'pub const STAGED_NAME: &str = "maintainer-merge";', 'pub const STAGED_NAME: &str = "gate";', "the binary is staged under another name"),
 ("W11", R, "        Self { sandbox, exe: exe.to_path_buf() }", '        Self { sandbox, exe: PathBuf::from("/nonexistent") }', "the real stage copies another file"),
 ("A01", A, "        has_suite_host: true,", "        has_suite_host: false,", "the program is built without a suite host"),
 ("A02", A, "    for name in SUITE_ENV_NAMES {", "    for name in SUITE_ENV_NAMES.into_iter().take(0) {", "a suite sees none of the variables the shipped suites read"),
 ("A03", A, "        if env.iter().all(|(have, _)| have != name)\n            && let Some(value) = pick(name)", "        if true\n            && let Some(value) = pick(name)", "a suite variable the environment holds is added twice"),
 ("A04", A, "            env.push((name.to_string(), value));", "            drop(value);\n            env.push((name.to_string(), String::new()));", "a suite variable reaches the suite empty"),
 ("A05", A, "        || SUITE_ENV_NAMES.contains(&name)\n", "", "a suite variable that is not text is ignored"),
 ("A06", A, "        || RUNTIME_ENV_NAMES.contains(&name)\n", "", "a container runtime variable that is not text is ignored"),
 ("A07", A, '            profile_dir: pick("PROFILE_DIR").unwrap_or_default(),', "            profile_dir: String::new(),", "PROFILE_DIR is not read for the suite folder"),
 ("A08", A, '            home: vars.get("HOME").cloned().unwrap_or_default(),', "            home: String::new(),", "the suite folder under HOME is not looked in"),
 ("A09", A, '            profile: pick("MAINTAINER_PROFILE").unwrap_or_default(),', "            profile: String::new(),", "the profile name is not part of the suite folder"),
 ("A10", A, "            here: tree.bin_dir.clone(),", "            here: PathBuf::new(),", "the folder beside the gate is not looked in"),
 ("A11", A, "        pick_env(&RUNTIME_ENV_NAMES, |name| vars.get(name).cloned()),", "        pick_env(&RUNTIME_ENV_NAMES, |_| None),", "the container runtime gets no variables"),
 ("A12", A, "        Runtime::Podman,\n        pick_env", "        Runtime::Docker,\n        pick_env", "the sandbox is built for docker"),
 ("A13", A, "        Arc::new(RealPipe::new(search_path, None)),", '        Arc::new(RealPipe::new("", None)),', "the container runtime is not found on the search path"),
 ("A14", A, "    let stage = RealStage::new(&sandbox, exe);", '    let stage = RealStage::new(&sandbox, Path::new("/nonexistent"));', "the gate stages a file that is not itself"),
 ("A15", A, "        Arc::new(RealProc::new(search_path, child_env(vars))),\n        Arc::new(RealPipe", "        Arc::new(RealProc::new(search_path, Vec::new())),\n        Arc::new(RealPipe", "the sandbox's git gets no environment"),
 ("A16", A, "        Arc::new(RealProc::new(search_path, suite_env(vars, &pick))),", "        Arc::new(RealProc::new(search_path, Vec::new())),", "the suites get no environment"),
 ("S01", SR, "        *self.shared.work.lock().unwrap_or_else(PoisonError::into_inner) = Some(work.to_path_buf());", "        *self.shared.work.lock().unwrap_or_else(PoisonError::into_inner) = None;\n        let _ = work;", "the real suite host forgets the tree it was told"),
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
    """dest/rust is the workspace. lib/ (the profile loader the end-to-end tests run), profiles/ (the
    shipped suites a test loads), scripts/ (the helpers the dependency tests run) and bin/maintainer-merge (read for the texts of its refusals) are
    copied beside it, where the tests look for them."""
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(RUST, dest / "rust", ignore=shutil.ignore_patterns("target", "__pycache__"), symlinks=True)
    shutil.copytree(RUST.parent / "lib", dest / "lib", symlinks=True)
    shutil.copytree(RUST.parent / "profiles", dest / "profiles", symlinks=True)
    shutil.copytree(RUST.parent / "scripts", dest / "scripts", symlinks=True)
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
    env.update({"CARGO_TARGET_DIR": str(target), "CARGO_TERM_COLOR": "never",
                "CARGO_BUILD_JOBS": os.environ.get("CARGO_BUILD_JOBS", "2"), "PYTHONDONTWRITEBYTECODE": "1"})
    cmd = ["cargo", "test", "-p", "mx-gate", "-p", "mx-suite", "--lib", "--test", "verify_end_to_end",
           "--test", "real_host", "--locked", "--offline", "--no-fail-fast"]
    # A session of its own, so a timeout kills the whole group and not only cargo.
    proc = subprocess.Popen(cmd, cwd=copy / "rust", env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, preexec_fn=cap, start_new_session=True)
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
    if "Compiling mx-gate" not in out and "Compiling mx-suite" not in out:
        return "NOT_REBUILT", names, passed, failed, out
    return ("RED" if failed else "GREEN"), names, passed, failed, out


def check_anchors():
    bad = 0
    ids = [m[0] for m in MUTANTS]
    if len(set(ids)) != len(ids):
        print("duplicate mutant ids")
        bad += 1
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
    if not check_anchors():
        sys.exit("mutants: fix the anchors first")
    scratch = Path(os.environ.get("MX_SCRATCH", Path.home() / ".cache" / "magent-wt-scratch")) / "mx-p3w-mutants"
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
        if status != "GREEN" or passed < 400:
            print(out[-2500:])
            sys.exit("mutants: the pristine control is not green with the full set")
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
