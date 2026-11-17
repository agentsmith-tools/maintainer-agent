#!/usr/bin/env python3
"""Mutation proof for the mx-repo crate.

  python3 mutants.py            every mutant
  python3 mutants.py M03 M17    only these

Each mutant is applied to a fresh copy of the workspace (the checkout is never
edited): every file of the copy is touched, so cargo cannot reuse an older
build; the old text must occur exactly once; the edit is read back and the
file's hash must differ from the pristine one; then `cargo test -p mx-repo
--no-fail-fast` runs in a private target directory. A mutant is KILLED when the
crate builds, the run reports failed tests, and every failure is an assertion
or a refused call (a panic), never a compile error. BUILD_ERR, TIMEOUT and
SURVIVED are reported as such and are never counted as kills.

The pristine copy runs before the batch and after it (the same test count, no
failure), and the hash of every source file of the checkout is compared before
and after.

Environment: MX_SCRATCH (default ~/.cache/magent-wt-scratch), CARGO_BUILD_JOBS.
"""

import hashlib
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
RUST = HERE.parent.parent.parent  # rust/

S = "crates/mx-repo/src/"
# id, file, old text (exactly once), new text, what it breaks
MUTANTS = [
    ("M01", S + "prune.rs", 'args.first().is_some_and(|arg| arg == "--dry-run")',
     'args.first().is_some_and(|arg| arg.to_string_lossy().starts_with("--dry"))', "a mistyped --dryrun counts as a dry run"),
    ("M02", S + "prune.rs", "!open_heads.is_empty() && branch == open_heads",
     "!open_heads.is_empty() && open_heads.split('|').any(|head| head == branch)", "F5 fixed: the open-PR guard works"),
    ("M03", S + "prune.rs", 'matches!(name, "main" | "master") ||', 'matches!(name, "main") ||', "master is a candidate"),
    ("M04", S + "prune.rs", '(remote && name == "HEAD")', "false", "origin/HEAD is a candidate"),
    ("M05", S + "prune.rs", "        return Ok(1);", "        return Ok(0);", "a dry run exits 0"),
    ("M06", S + "prune.rs", "        dry = true;\n        ctx.say(\"POST=off", "        ctx.say(\"POST=off", "POST=off no longer makes the run a listing"),
    ("M07", S + "prune.rs", '"--delete"', '"-d"', "remote delete argv"),
    ("M08", S + "prune.rs", "                deletable += 1;\n", "", "the remote section never counts, so it always says none"),
    ("M09", S + "prune.rs", "        1 => Fact::Known(false),", "        1 => Fact::Known(true),", "not merged reads as merged"),
    ("M10", S + "prune.rs", "        Fact::Known(false) | Fact::Absent | Fact::Unreadable(_) => Verdict::Skip,\n    }\n}\n\n/// `git merge-base",
     "        Fact::Known(false) | Fact::Absent => Verdict::Skip,\n        Fact::Unreadable(_) => Verdict::Delete,\n    }\n}\n\n/// `git merge-base",
     "a merge test that could not run deletes the branch"),
    ("M11", S + "prune.rs", "if *byte == b'\\n' { b'|' }", "if *byte == b'\\n' { b' ' }", "open heads joined with a space"),
    ("M12", S + "prune.rs", "    if !fetch.ok() {", "    if false {", "a failed fetch is ignored"),
    ("M13", S + "prune.rs", '&["branch", "-D", branch]', '&["branch", "-d", branch]', "local delete is -d"),
    ("M14", S + "prune.rs", '    drop(ctx.git(&["remote", "prune", "origin"]));\n', "", "remote refs are never pruned"),
    ("M15", S + "release.rs", "is_some_and(u8::is_ascii_digit)", "is_some_and(u8::is_ascii_alphanumeric)", "a heading with a letter ends the section"),
    ("M16", S + "release.rs", "    if prod == 0 {", "    if prod <= 1 {", "one production file reads as docs only"),
    ("M17", S + "release.rs", "    breaking > 0 || security > 0\n", "    breaking > 0 && security > 0\n", "digit needs both a Security entry and wording"),
    ("M18", S + "release.rs", ', b"refuses"]', "]", "refuses is not removed-capability wording"),
    ("M19", S + "release.rs", 'line.starts_with(b"## [Unreleased]")', 'line == b"## [Unreleased]"', "a heading with trailing text does not open the section"),
    ("M20", S + "release.rs", "b' ' | b'\\t' | b'\\n' | 0x0b | 0x0c | b'\\r'", "b' ' | b'\\n'", "tab and carriage return count as text"),
    ("M21", S + "release.rs", "    Some(done.stdout.iter().filter(|byte| **byte == b'\\n').count())", "    Some(done.stdout.iter().filter(|byte| **byte == b'\\n').count() + 1)", "path count is one too high"),
    ("M22", S + "release.rs", "is_some_and(|count| count > 0)", "is_some_and(|count| count > 1)", "one commit behind is not reported"),
    ("M23", S + "release.rs", '    if n == "0" {', '    if n == "1" {', "zero commits is not nothing to release"),
    ("M24", S + "shellish.rs", "let first = text.split('\\n').next().unwrap_or(\"\");", "let first = text;", "a multi-line glob list reads past its first line"),
    ("M25", S + "fixup.rs", "            parsed.pr = None;\n            break;", "            parsed.pr = None;", "a bad word no longer stops the reading"),
    ("M26", S + "fixup.rs", "bytes.iter().any(|byte| !byte.is_ascii_digit())", "bytes.iter().any(|byte| byte.is_ascii_alphabetic())", "12.5 is a pull request number"),
    ("M27", S + "fixup.rs", "[&self.state, &self.head, &self.branch, &self.fork, &self.items]", "[&self.state, &self.branch, &self.fork, &self.items]", "an answer with no head is complete"),
    ("M28", S + "fixup.rs", "    a.eq_ignore_ascii_case(b)", "    a == b", "login comparison is case sensitive"),
    ("M29", S + "fixup.rs", ".is_none_or(|next| !next.is_ascii_digit())", ".is_none_or(|_| true)", "30 states 3"),
    ("M30", S + "fixup.rs", "            && line[1] != b'+'\n", "", "a ++ line states a count"),
    ("M31", S + "fixup.rs", '        line == b"state=ready"\n            || ', "        ", "a ready state does not confirm the heads-up"),
    ("M32", S + "fixup.rs", "This has had no reply for", "This has had no answer for", "the heads-up wording"),
    ("M33", S + "fixup.rs", '"--force-with-lease=refs/heads/{}:{}"', '"--force=refs/heads/{}:{}"', "the rewrite is forced, not leased"),
    ("M34", S + "fixup.rs", "            rewriting = true;\n", "", "a rebase is pushed without a lease"),
    ("M35", S + "fixup.rs", "decimal_integer(&raw_tries).is_some_and(|limit| i < limit)", "decimal_integer(&raw_tries).is_some_and(|limit| i <= limit)", "a sleep after the last read"),
    ("M36", S + "fixup.rs", '.any(|line| line.starts_with(b"Signed-off-by:"))', '.any(|line| line.windows(14).any(|w| w == b"Signed-off-by:"))', "a mid-line mention turns the sign-off on"),
    ("M37", S + "fixup.rs", '            "-u",\n            "GH_TOKEN",\n            "-u",\n            "GITHUB_TOKEN",\n', '            "-u",\n            "GITHUB_TOKEN",\n', "the count command sees GH_TOKEN"),
    ("M38", S + "fixup.rs", '        "git",\n        "-c",\n        "core.hooksPath=/dev/null",\n        "clone",', '        "git",\n        "clone",', "the clone runs the contributor's hooks"),
    ("M39", S + "cleanup.rs", "        drop(std::fs::remove_dir_all(&self.path));\n", "", "the scratch clone is left behind"),
    ("M40", S + "fixup.rs", '    ctx.exports.push(("GIT_EDITOR".into(), "true".into()));\n', "", "git may open an editor"),
    ("M41", S + "fixup.rs", "NOT confirmed, look at it", "confirmed, look at it", "a missing read-back reads as confirmed"),
    ("M42", S + "ctx.rs", "call.cwd.as_ref().or(self.cwd.as_ref())", "call.cwd.as_ref()", "children do not run in the repository"),
    ("M43", S + "ctx.rs", '["MAINTAINER_", "GH_", "GITHUB_"', '["MAINTAINER_", "GITHUB_"', "GH_TOKEN does not reach gh"),
    ("M44", S + "lib.rs", "            ctx.exports.push((name.clone(), value.to_string()));\n", "", "a deployed profile's names do not reach children"),
    ("M45", S + "prune.rs", "enter_repo(ctx, settings, 32)?;", "enter_repo(ctx, settings, 33)?;", "the cd diagnostic names the wrong line"),
    ("M46", S + "lib.rs", '        self.post == "off"', '        self.post.eq_ignore_ascii_case("off")', "POST=OFF is a rehearsal"),
    ("M47", S + "ctx.rs", "ProcExit::TimedOut => 124,", "ProcExit::TimedOut => 0,", "a timed-out child reads as success"),
    ("M48", S + "fixup.rs", "        if kind == \"rebase\" {", "        if kind == \"typo\" {", "the rebase item is ignored"),
    ("M49", S + "fixup.rs", "decimal_integer(&raw_tries).is_some_and", "bash_integer(&raw_tries).is_some_and", "the sleep test reads 010 as octal"),
    ("M50", S + "release.rs", "let room = MAX_LINE.saturating_sub(buf.len());", "let room = usize::MAX;", "a line is read whole, without a cap"),
    ("M51", S + "lib.rs", "            if matches!(error, mx_profile::LoadError::Unbound { .. }) {\n                return Err(Stop(1));", "            if matches!(error, mx_profile::LoadError::Unbound { .. }) {\n                return Err(Stop(127));", "an unbound profile variable ends with 127"),
    ("M52", S + "lib.rs", "        // A failed expansion ends the non-interactive bash with status 1.\n        return Err(Stop(1));", "        // A failed expansion ends the non-interactive bash with status 1.\n        return Err(Stop(127));", "an unset HOME ends with 127"),
    ("M53", S + "release.rs", "        Ok(file) => changelog_facts_from(std::io::BufReader::new(file)),", "        Ok(_) => changelog_facts(std::fs::read(settings.repo_path.join(\"CHANGELOG.md\")).ok().as_deref()),", "the CHANGELOG is read whole into memory"),
    ("M54", S + "cleanup.rs", "        armed().retain(|armed_path| *armed_path != self.path);\n", "", "a dropped guard stays armed for a signal"),
    ("M55", S + "cleanup.rs", "            match std::fs::remove_dir_all(&path) {", "            match Ok::<(), std::io::Error>(()) {", "a signal removes nothing"),
    ("M56", S + "ctx.rs", "meta.is_file() && meta.permissions().mode() & 0o111 != 0", "meta.is_file()", "a file that is not executable is a tool"),
    ("M57", S + "ctx.rs", 'const TOKEN_CHILDREN: &[&str] = &["gh", "git", "python3"];', 'const TOKEN_CHILDREN: &[&str] = &["gh", "git"];', "python3 gets no token, so fixup-check cannot start gh"),
    ("M58", S + "ctx.rs", '&["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"];', '&["GH_TOKEN", "GITHUB_TOKEN", "GITHUB_ENTERPRISE_TOKEN"];', "an enterprise token reaches every child"),
    ("M59", S + "ctx.rs", "if !may_hold_tokens && TOKEN_NAMES.contains(&name.as_str()) {", "if false && TOKEN_NAMES.contains(&name.as_str()) {", "every child gets the tokens"),
    ("M60", S + "prune.rs", 'ctx.missing_tool("tr", 38)', 'ctx.missing_tool("tr", 39)', "the tr diagnostic names the wrong line"),
    ("M61", S + "prune.rs", "                    && !is_protected(branch, true)\n", "", "grep is asked for a protected branch"),
    ("M62", S + "prune.rs", "                if !heads.is_empty()\n", "                if true\n", "grep is asked with no open pull request"),
    ("M63", S + "release.rs", "count_paths(ctx, &range, &prod_globs, 101)", "count_paths(ctx, &range, &prod_globs, 102)", "the wc diagnostic names the wrong line"),
    ("M64", S + "release.rs", "count_paths(ctx, &range, &shipped_globs, 119)", "count_paths(ctx, &range, &shipped_globs, 118)", "the shipped wc diagnostic names the wrong line"),
    ("M65", S + "release.rs", "    let yes_added = gt_zero(ctx, added, 169);", "    let yes_added = gt_zero(ctx, added, 168);", "the integer diagnostic names the wrong line"),
    ("M66", S + "release.rs", "    let section_empty = tr_missing || !section_ok || facts.empty;", "    let section_empty = !section_ok || facts.empty;", "a host without tr reads the section as present"),
    ("M67", S + "release.rs", '    if !ctx.missing_tool("sed", 83) {', '    if true {', "a host without sed prints the log"),
    ("M68", S + "fixup.rs", "        for line in 223..=233 {", "        for line in 223..=232 {", "one sed read is not reported"),
    ("M69", S + "fixup.rs", "if no_tr || no_paste {", "if no_tr {", "a host without paste still lists the kinds"),
    ("M70", S + "fixup.rs", '    if !ctx.missing_tool("rm", 258) {', '    if true {', "a host without rm removes the temp file"),
    ("M71", S + "fixup.rs", 'ctx.missing_tool("grep", 263) || !heads_up_confirmed(&seen)', '!heads_up_confirmed(&seen)', "a host without grep confirms the heads-up"),
    ("M72", S + "fixup.rs", '    for tool in ["tr", "grep", "sort", "paste"] {', '    for tool in ["tr", "paste", "sort", "grep"] {', "the kinds pipeline reports its tools in another order"),
    ("M73", S + "fixup.rs", 'ctx.missing_tool("awk", 311) || !patch_states_count', '!patch_states_count', "a host without awk still checks the count"),
    ("M74", S + "fixup.rs", '.at(270)', '.at(271)', "the mktemp diagnostic names the wrong line"),
    ("M75", S + "fixup.rs", 'return Err(unbound(ctx, "GH_TRIES", 349));', 'return Err(unbound(ctx, "GH_TRIES", 348));', "the GH_TRIES diagnostic names the wrong line"),
    ("M76", S + "fixup.rs", '    Stop(1)\n}\n\n/// `${MAINTAINER_STATE:?}`', '    Stop(127)\n}\n\n/// `${MAINTAINER_STATE:?}`', "an unbound GH_TRIES ends with 127"),
    ("M77", S + "fixup.rs", "    let removable = ctx.has_tool(\"rm\");", "    let removable = true;", "a host without rm says nothing about the clone it keeps"),
    ("M78", S + "fixup.rs", 'return Err(unbound(ctx, "GH_BACKOFF", 353));', 'return Err(unbound(ctx, "GH_BACKOFF", 352));', "the GH_BACKOFF diagnostic names the wrong line"),
]


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def tree_hash(root):
    digest = hashlib.sha256()
    for path in sorted(Path(root).rglob("*")):
        if path.is_file() and "target" not in path.parts:
            digest.update(str(path.relative_to(root)).encode())
            digest.update(sha(path).encode())
    return digest.hexdigest()


def copy_workspace(dest):
    """dest/rust is the workspace; dest/lib is lib/, which the startup tests copy from."""
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(RUST, dest / "rust", ignore=shutil.ignore_patterns("target"), symlinks=True)
    shutil.copytree(RUST.parent / "lib", dest / "lib", symlinks=True)


def touch_all(root):
    now = time.time()
    for path in Path(root).rglob("*"):
        if path.is_file():
            os.utime(path, (now, now))


def cargo_test(copy, target, timeout):
    env = dict(os.environ)
    env.update({"CARGO_TARGET_DIR": str(target), "CARGO_TERM_COLOR": "never", "CARGO_BUILD_JOBS": os.environ.get("CARGO_BUILD_JOBS", "2")})
    try:
        cp = subprocess.run(["cargo", "test", "-p", "mx-repo", "--locked", "--offline", "--no-fail-fast"], cwd=copy / "rust", env=env,
                            capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired as error:
        return "TIMEOUT", [], 0, 0, (error.stdout or "")
    out = cp.stdout + cp.stderr
    if "could not compile" in out or re.search(r"^error(\[E\d+\])?: (?!test failed|\d+ targets? failed)", out, re.M):
        return "BUILD_ERR", [], 0, 0, out
    passed = sum(int(n) for n in re.findall(r"test result: \w+\. (\d+) passed", out))
    failed = sum(int(n) for n in re.findall(r"; (\d+) failed", out))
    names = re.findall(r"^test (\S+) \.\.\. FAILED$", out, re.M)
    if "Compiling mx-repo" not in out:
        return "NOT_REBUILT", names, passed, failed, out
    return ("RED" if failed else "GREEN"), names, passed, failed, out


def main():
    want = sys.argv[1:]
    scratch = Path(os.environ.get("MX_SCRATCH", Path.home() / ".cache" / "magent-wt-scratch")) / "mx-repo-mutants"
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
        status, names, passed, failed, out = cargo_test(work, target, 1800)
        print("control %-6s %s: %d passed, %d failed" % (label, status, passed, failed), flush=True)
        if status != "GREEN" or passed < 100:
            print(out[-2000:])
            sys.exit("mutants: the pristine control is not green with the full suite")
        return passed

    before = control("before")
    rows, survivors, killed = [], [], 0
    chosen = [m for m in MUTANTS if not want or m[0] in want]
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
        status, names, passed, failed, out = cargo_test(work, target, 1500)
        if status == "RED":
            outcome = "KILLED"
            killed += 1
        else:
            outcome = status
            survivors.append((mid, status))
        first = names[0] if names else "-"
        rows.append((mid, rel.split("/")[-1], note, outcome, failed, first))
        print("%-4s %-10s %-8s %3d red  %s  [%s]" % (mid, rel.split("/")[-1], outcome, failed, first, note), flush=True)
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
