#!/usr/bin/env python3
"""Mutation proof for the mx-checks crate.

  python3 mutants.py            every mutant
  python3 mutants.py M03 M17    only these
  python3 mutants.py --check    validate the list (each old text occurs exactly once) and stop

Each mutant is applied to a fresh copy of the workspace (the checkout is never edited): every
file of the copy is touched, so cargo cannot reuse an older build; the old text must occur
exactly once; the edit is read back and the file's hash must differ from the pristine one; then
`cargo test -p mx-checks --no-fail-fast` runs in a private target directory. A mutant is KILLED
when the crate builds, the run reports failed tests, and every failure is an assertion or a
refused call (a panic), never a compile error. BUILD_ERR, TIMEOUT, NOT_REBUILT and SURVIVED are
reported as such and are never counted as kills.

The pristine copy runs before the batch and after it (the same test count, no failure), and the
hash of every source file of the checkout is compared before and after.

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

S = "crates/mx-checks/src/"
SH, CT, HO = S + "shell.rs", S + "ctx.rs", S + "host.rs"
CL, PN, SW, EV, PR = S + "claims.rs", S + "pins.rs", S + "sweep.rs", S + "evals.rs", S + "probe.rs"

# id, file, old text (exactly once), new text, what it breaks
MUTANTS = [
    # --- shell.rs and ctx.rs: the shell constructs ---
    ("M01", SH, "matches!(byte, b' ' | b'\\t' | b'\\n' | 0x0b | 0x0c | b'\\r')", "matches!(byte, b' ' | b'\\t' | b'\\n' | 0x0b | 0x0c)", "a carriage return is not white space"),
    ("M02", SH, "window.eq_ignore_ascii_case(needle)", "window == needle", "the needle search is case sensitive"),
    ("M03", SH, "        && keep_unterminated\n", "        && !keep_unterminated\n", "an unterminated last line is read when it must not be"),
    ("M04", SH, "    if negative { value.checked_neg() } else { Some(value) }", "    Some(value)", "a negative number loses its sign"),
    ("M05", SH, "    while out.len() < width {", "    while out.len() < width.saturating_sub(1) {", "the column is one narrow"),
    ("M06", CT, "ProcExit::TimedOut => 124,", "ProcExit::TimedOut => 1,", "a timeout reads as 1"),
    ("M07", CT, '.filter(|(name, _)| name.as_str() != "PATH" && name.as_str() != "LC_ALL")', '.filter(|(name, _)| name.as_str() != "PATH")', "LC_ALL reaches a child as a variable"),
    ("M08", CT, "(127, shell::command_not_found(&name, line, program))", "(126, shell::command_not_found(&name, line, program))", "a program that is not found reads as 126"),
    # --- pins.rs ---
    ("M09", PN, 'if !text[start..].starts_with(b"uses: ") {', 'if !text[start..].starts_with(b"uses:") {', "a missing space after uses: is accepted in a pin"),
    ("M10", PN, "let sha = text.get(at..at + 40)?;", "let sha = text.get(at..at + 39)?;", "a 39-character SHA is a pin"),
    ("M11", PN, "if !sha.iter().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {", "if !sha.iter().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F')) {", "an upper-case SHA is a pin"),
    ("M12", PN, "(at > word).then_some(at)", "Some(at)", "an empty comment is a pin"),
    ("M13", PN, "    pin_end(line, 0) == Some(line.len())", "    pin_end(line, 0).is_some()", "a longer comment is a strict pin"),
    ("M14", PN, "    tails.dedup();\n", "", "an unparsed line in two files is named twice"),
    ("M15", PN, "    found.dedup();\n", "", "a pin in two files is checked twice"),
    ("M16", PN, "    found.sort();\n", "", "pins are checked in file order"),
    ("M17", PN, "    tails.retain(|line| !is_strict_pin(line));", "    tails.retain(|line| is_strict_pin(line));", "the pinned lines are the unparsed ones"),
    ("M18", PN, 'if kind == b"tag" { refs.tag_object_sha(ctx, &pin.repo, &object) } else { object }', 'if kind == b"commit" { object } else { refs.tag_object_sha(ctx, &pin.repo, &object) }', "a blob is followed as a tag"),
    ("M19", PN, 'if pin.claim == b"main" || pin.claim == b"master" {', 'if pin.claim == b"main" {', "master is not a branch"),
    ("M20", PN, "    } else if real == pin.sha {", "    } else if real.starts_with(&pin.sha[..8]) {", "a SHA that shares eight characters is the same"),
    ("M21", PN, "let action = shell::pad_right(&pin.action, 46);", "let action = shell::pad_right(&pin.action, 40);", "the action column is 40 wide"),
    ("M22", PN, "let head8 = |bytes: &[u8]| bytes[..bytes.len().min(8)].to_vec();", "let head8 = |bytes: &[u8]| bytes[..bytes.len().min(7)].to_vec();", "SHAs print seven characters"),
    ("M23", PN, "        fail |= failed;", "        fail = failed;", "only the last pin decides the verdict"),
    ("M24", PN, "    i32::from(fail)\n}", "    0\n}", "a failing pin exits zero"),
    ("M25", PN, "let mut sources = find_files(ctx, &workflows, Some(1), &[b\"*.yml\", b\"*.yaml\"]);", "let mut sources = find_files(ctx, &workflows, None, &[b\"*.yml\", b\"*.yaml\"]);", "workflows in a subdirectory are scanned"),
    ("M26", PN, "    sources.extend(find_files(ctx, &under(&root, \"examples\"), Some(1), &[b\"*.yml\"]));", "    sources.extend(find_files(ctx, &under(&root, \"examples\"), None, &[b\"*.yml\"]));", "examples in a subdirectory are scanned"),
    ("M27", PN, '            ctx.put_err(&message(&root, "", "/.github/actions exists and holds no action.yml, so the composite scan read nothing"));', '            ctx.put_err(&message(&root, "", "/.github/actions exists and holds no action.yml"));', "the composite refusal loses its reason"),
    # --- claims.rs ---
    ("M28", CL, 'if at > digits && line[at..].starts_with(b" passed") {', 'if at > digits && line[at..].starts_with(b" pass") {', "'N pass' counts as a summary"),
    ("M29", CL, 'if end > digits && line[end..].starts_with(b" skipped") {', 'if end > digits && line[end..].starts_with(b" skip") {', "'N skip' counts"),
    ("M30", CL, "        .take(10)\n", "        .take(11)\n", "eleven failing cases are shown"),
    ("M31", CL, "Ok(number(&passed).saturating_add(skipped.as_deref().map_or(0, number)))", "Ok(number(&passed))", "skipped cases do not count"),
    ("M32", CL, '    values.argv[4] = "--values".to_string();', '    values.argv[4] = "--check".to_string();', "the values call is the check call"),
    ("M33", CL, "    if !suite.ok() {", "    if suite.rc == 1 {", "a suite that exits 2 is green"),
    ("M34", CL, "            if name.first() != Some(&b'.')\n", "            if true\n", "a hidden directory is a profile"),
    ("M35", CL, 'const HOME: &str = "/home/claimcheck";', 'const HOME: &str = "/home/someone";', "the renderer is given another home"),
    ("M36", CL, '    line.extend_from_slice(b" rules on this machine\\n");', '    line.extend_from_slice(b" rules on this host\\n");', "the combined line says host"),
    # --- sweep.rs ---
    ("M37", SW, 'const SHEBANGS: [&[u8]; 3] = [b"#!/usr/bin/env bash", b"#!/bin/bash", b"#!/bin/sh"];', 'const SHEBANGS: [&[u8]; 3] = [b"#!/usr/bin/env bash", b"#!/bin/bash", b"#!/bin/zsh"];', "sh scripts are not scripts"),
    ("M38", SW, "pub const HEAD_BYTES: usize = 80;", "pub const HEAD_BYTES: usize = 120;", "the shebang test reads 120 bytes"),
    ("M39", SW, 'let Some(rest) = name.strip_prefix(b"profiles/") else { return false };', 'let Some(rest) = name.strip_prefix(b"profile/") else { return false };', "verify.d fragments are not under profiles"),
    ("M40", SW, "    files.dedup();\n", "", "a file that matches both rules is linted twice"),
    ("M41", SW, "        let head = if word.starts_with(b\"-\") {", "        let head = if word.starts_with(b\"+\") {", "a name that starts with a dash is read"),
    ("M42", SW, "(Some(have_n), Some(need_n)) if have_n >= need_n => return Ok(()),", "(Some(have_n), Some(need_n)) if have_n > need_n => return Ok(()),", "memory equal to the need waits"),
    ("M43", SW, "        if waited >= wait {", "        if waited > wait {", "the wait gives up one poll late"),
    ("M44", SW, "        waited += poll;", "        waited += 1;", "a poll of 10 counts as 1"),
    ("M45", SW, "            return Err(75);\n        }\n        // `[ \"$have\" -ge", "            return Err(74);\n        }\n        // `[ \"$have\" -ge", "no usable MemAvailable exits 74"),
    ("M46", SW, "    argv.push(\"--severity=warning\".to_string());", "    argv.push(\"--severity=error\".to_string());", "the linter runs at severity error"),
    ("M47", SW, '.chain([format!("MemoryMax={cap}"), "-p".to_string(), "MemorySwapMax=0".to_string()])', '.chain([format!("MemoryMax={cap}")])', "the cap leaves swap on"),
    ("M48", SW, "    if settings.ci {\n        return Ok(());", "    if !settings.ci {\n        return Ok(());", "CI waits for memory and the rest do not"),
    ("M49", SW, "    if !settings.ci && !settings.no_cap {", "    if !settings.ci {", "the no-cap variable is ignored"),
    ("M50", SW, 'runner.extend(["nice".to_string(), "-n".to_string(), "15".to_string()]);', 'runner.extend(["nice".to_string(), "-n".to_string(), "10".to_string()]);', "nice 10"),
    ("M51", SW, "        if r > rc {\n            rc = r;\n        }", "        rc = r;", "the last status decides"),
    ("M52", SW, "        if r == 137 {", "        if r >= 137 {", "a status above 137 is a kill"),
    ("M53", SW, "            break;\n        }\n    }\n    rc", "            continue;\n        }\n    }\n    rc", "the sweep goes on after a kill"),
    ("M54", SW, "        return 127;", "        return 126;", "a missing linter exits 126"),
    ("M55", SW, '    ctx.put_out(format!("checking {} scripts\\n", files.len()).as_bytes());', '    ctx.put_out(format!("checking {} script(s)\\n", files.len()).as_bytes());', "the count line changes"),
    ("M56", SW, 'let listed = ctx.exec(&Call::new(["git", "ls-files"]).cwd(Some(&root)).at(12));', 'let listed = ctx.exec(&Call::new(["git", "ls-files", "-z"]).cwd(Some(&root)).at(12));', "git is asked for NUL-separated names"),
    ("M57", SW, '            .cwd(Some(root))\n            .at(88),', '            .at(88),', "sleep runs outside the checkout"),
    ("M58", SW, 'ctx.exec(&Call::new(probe).cwd(Some(&root)).at(99)).ok();', 'ctx.exec(&Call::new(probe).at(99)).ok();', "the scope probe runs outside the checkout"),
    # --- evals.rs ---
    ("M59", EV, 'pub const RETIRABLE: &str = "05-reserved-issue";', 'pub const RETIRABLE: &str = "05-reserved-issue 01-prompt-injection";', "a shared-doctrine scenario can be retired"),
    ("M60", EV, "        if !needle.is_empty() {", "        if needle.is_empty() {", "empty needles are rules"),
    ("M61", EV, "    if shell::contains_ignore_case(corpus, needle) {", "    if shell::contains(corpus, needle) {", "needles are case sensitive"),
    ("M62", EV, '            if name.first() != Some(&b\'.\') && name.ends_with(b".md") {', '            if name.ends_with(b".md") {', "a dot file is a scenario"),
    ("M63", EV, "    i32::from(failed > 0)", "    0", "static mode exits zero"),
    ("M64", EV, "        if !only.is_empty() && !shell::contains(&id, only) {", "        if !only.is_empty() && !id.starts_with(only) {", "the live filter is a prefix"),
    ("M65", EV, '            text.extend_from_slice(b": backend failed");', '            text.extend_from_slice(b": failed");', "the backend failure line changes"),
    ("M66", EV, "        return if sourced.rc == 0 { 1 } else { sourced.rc };", "        return 0;", "a shell that died while sourcing exits zero"),
    ("M67", EV, '        _ => "sysknife".to_string(),', '        _ => "magent".to_string(),', "the default profile is magent"),
    ("M68", EV, "            64\n        }\n    }\n}", "            2\n        }\n    }\n}", "a bad argument exits 2"),
    ("M69", EV, '                if piece.starts_with(b"## Where the rule lives") {', '                if piece.starts_with(b"## Why this scenario") {', "live mode cuts at another heading"),
    # --- probe.rs ---
    ("M70", PR, '    shell::contains(out, b"rc=0")', '    shell::contains(out, b"rc=1")', "the success status is rc=1"),
    ("M71", PR, '.any(|line| line.starts_with(b"ran"))', '.any(|line| shell::contains(line, b"ran"))', "ran anywhere in a line is a run"),
    ("M72", PR, "            !rest.is_empty() && rest.iter().all(|byte| *byte == b'0')", "            rest.iter().all(|byte| *byte == b'0')", "an empty capability field is empty"),
    ("M73", PR, ".any(|at| line[at + marker.len()..].contains(&b'1'))", ".any(|at| line[at + marker.len()..].contains(&b'0'))", "NoNewPrivs 0 is set"),
    ("M74", PR, "is_some_and(|n| n < 1000)", "is_some_and(|n| n <= 1000)", "1000 processes is stopped"),
    ("M75", PR, ".take(60).collect()", ".take(40).collect()", "the capabilities message is cut at 40"),
    ("M76", PR, "    if after <= before {", "    if after < before {", "the same number of containers is a leak"),
    ("M77", PR, '        shell::contains(&out, b"PROBECANARY"),', '        shell::contains(&out, b"PROBECANARX"),', "the canary is never found"),
    ("M78", PR, '    let through = ctx.host().kind(&escaped) != Kind::Missing;', '    let through = ctx.host().kind(&escaped) == Kind::Missing;', "a missing escape file is an escape"),
    ("M79", PR, '    let slept = ctx.exec(&Call::new(["sleep", "3"]).at(93));', '    let slept = ctx.exec(&Call::new(["sleep", "2"]).at(93));', "sleep 2"),
    ("M80", PR, '        _ => "15".to_string(),', '        _ => "16".to_string(),', "the runtime timeout is 16"),
    ("M81", PR, '            "--cap-drop=ALL",\n            "--security-opt=no-new-privileges",', '            "--security-opt=no-new-privileges",', "the run drops no capabilities"),
    ("M82", PR, '            "/tmp:rw,noexec,nosuid,size=256m",', '            "/tmp:rw,nosuid,size=256m",', "the tmpfs is not noexec"),
    ("M83", PR, '            "512",\n', '            "513",\n', "the pid cap is 513"),
    ("M84", PR, '        argv.push(format!("{}:/repo:z", lossy(&self.work)));', '        argv.push(format!("{}:/repo", lossy(&self.work)));', "the mount has no :z"),
    ("M85", PR, '    let runtime = if ctx.on_path("podman") {', '    let runtime = if ctx.on_path("docker") {', "docker is preferred"),
    ("M86", PR, "    i32::from(probe.failed > 0)", "    0", "a got-through attack exits zero"),
    ("M87", PR, '        ctx.err_line("no container runtime; this probe measured nothing");\n        return 1;', '        ctx.err_line("no container runtime; this probe measured nothing");\n        return 0;', "no runtime exits zero"),
    ("M88", PR, '            ctx.err_line(&format!("cannot obtain {image}"));\n            return 1;', '            ctx.err_line(&format!("cannot obtain {image}"));\n            return 0;', "no image exits zero"),
    ("M89", PR, '    "--cpus",\n];', '    "--cpux",\n];', "the gate section looks for --cpux"),
    ("M90", PR, "    drop(ctx.host().write(&work.join(\"repofile\"), b\"host file\\n\"));", "    drop(ctx.host().write(&work.join(\"repofile\"), b\"other\\n\"));", "the mounted file holds something else"),
    # --- pins.rs: the lookup refusal and the tag encoding (P4B-16) ---
    ("M91", PN, 'if part.is_empty() || part == b"." || part == b".." || !part.iter().all(name_byte) {', 'if part.is_empty() || part == b"." || !part.iter().all(name_byte) {', "a repository part of .. is allowed"),
    ("M92", PN, 'if shell::contains(tag, b"..") {', 'if tag == b".." {', "only a tag that is exactly .. is refused"),
    ("M93", PN, "if tag.iter().any(|byte| matches!(byte, b'?' | b'#' | b'%')) {", "if tag.iter().any(|byte| matches!(byte, b'?' | b'#')) {", "a % in a tag is allowed"),
    ("M94", PN, "if tag.iter().any(|byte| *byte < 0x20 || *byte == 0x7f) {", "if tag.iter().any(|byte| *byte < 0x20) {", "DEL in a tag is allowed"),
    ("M95", PN, "b\"-._~!$&'()*+,;=:@/\".contains(byte)", "b\"-._~!$&'()*+,;=:@\".contains(byte)", "a slash in a tag is percent-encoded"),
    ("M96", PN, "if lookup_refusal(repo, b\"x\").is_some() || !is_object_id(object) {", "if lookup_refusal(repo, b\"x\").is_some() {", "a tag object id that is not hexadecimal is asked about"),
    ("M97", PN, "    if let Some(reason) = lookup_refusal(&pin.repo, &pin.claim) {", "    if let Some(reason) = None::<&'static str> {", "a pin is judged without the refusal"),
    ("M98", PN, 'if tag.starts_with(b"/") || tag.ends_with(b"/") || shell::contains(tag, b"//") {', 'if tag.ends_with(b"/") || shell::contains(tag, b"//") {', "a tag with a leading slash is allowed"),
    ("M99", PN, "        if lookup_refusal(repo, tag).is_some() {\n            return None;", "        if false {\n            return None;", "the adapter trusts its caller"),
    ("M100", PN, "(40..=64).contains(&object.len())", "(1..=64).contains(&object.len())", "a short hexadecimal object id is accepted"),
    ("M101", PN, "if *byte < 0x20 || *byte == 0x7f { b'?' } else { *byte }", "*byte", "control bytes of a refused pin reach the terminal"),
    ("M102", PN, "if tag.iter().any(|byte| matches!(byte, b'?' | b'#' | b'%')) {", "if tag.iter().any(|byte| matches!(byte, b'#' | b'%')) {", "a ? in a tag is allowed"),
    ("M103", PN, "if tag.iter().any(|byte| matches!(byte, b'?' | b'#' | b'%')) {", "if tag.iter().any(|byte| matches!(byte, b'?' | b'%')) {", "a # in a tag is allowed"),
    ("M104", PN, 'if tag.starts_with(b"/") || tag.ends_with(b"/") || shell::contains(tag, b"//") {', 'if tag.starts_with(b"/") || shell::contains(tag, b"//") {', "a tag with a trailing slash is allowed"),
    ("M105", PN, 'if tag.starts_with(b"/") || tag.ends_with(b"/") || shell::contains(tag, b"//") {', 'if tag.starts_with(b"/") || tag.ends_with(b"/") {', "a tag with a doubled slash is allowed"),
    ("M106", PN, "    if tag.is_empty() {\n        return Some(\"the tag is empty\");", "    if false {\n        return Some(\"the tag is empty\");", "an empty tag is looked up"),
    ("M107", PN, "if tag.iter().any(|byte| *byte < 0x20 || *byte == 0x7f) {", "if tag.iter().any(|byte| *byte < 0x10 || *byte == 0x7f) {", "a control byte from 0x10 to 0x1f in a tag is allowed"),
    ("M108", PN, 'if part.is_empty() || part == b"." || part == b".." || !part.iter().all(name_byte) {', 'if part.is_empty() || part == b".." || !part.iter().all(name_byte) {', "a repository part of . is allowed"),
    ("M109", PN, "let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {", "let (Some(owner), Some(name), _) = (parts.next(), parts.next(), parts.next()) else {", "a repository of three parts is allowed"),
    ("M110", PN, "matches!(byte, b'_' | b'.' | b'-')", "matches!(byte, b'_' | b'.' | b'-' | b'%')", "a % in a repository name is allowed"),
    ("M111", PN, 'const HEX: &[u8; 16] = b"0123456789ABCDEF";', 'const HEX: &[u8; 16] = b"0123456789abcdef";', "the hex digits of an encoded byte are lower case"),
    ("M112", PN, "b\"-._~!$&'()*+,;=:@/\".contains(byte)", "b\"-._~!$&'()*,;=:@/\".contains(byte)", "a + in a tag is percent-encoded"),
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
    """dest/rust is the workspace; the tests of other crates are not needed, so the copy holds
    only what `cargo test -p mx-checks` builds."""
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(RUST, dest / "rust", ignore=shutil.ignore_patterns("target"), symlinks=True)


def touch_all(root):
    now = time.time()
    for path in Path(root).rglob("*"):
        if path.is_file():
            os.utime(path, (now, now))


def cargo_test(copy, target, timeout):
    env = dict(os.environ)
    env.update({"CARGO_TARGET_DIR": str(target), "CARGO_TERM_COLOR": "never", "CARGO_BUILD_JOBS": os.environ.get("CARGO_BUILD_JOBS", "2")})
    try:
        cp = subprocess.run(["cargo", "test", "-p", "mx-checks", "--locked", "--offline", "--no-fail-fast"], cwd=copy / "rust", env=env,
                            capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired as error:
        return "TIMEOUT", [], 0, 0, (error.stdout or "")
    out = cp.stdout + cp.stderr
    if "could not compile" in out or re.search(r"^error(\[E\d+\])?: (?!test failed|\d+ targets? failed)", out, re.M):
        return "BUILD_ERR", [], 0, 0, out
    passed = sum(int(n) for n in re.findall(r"test result: \w+\. (\d+) passed", out))
    failed = sum(int(n) for n in re.findall(r"; (\d+) failed", out))
    names = re.findall(r"^test (\S+) \.\.\. FAILED$", out, re.M)
    if "Compiling mx-checks" not in out:
        return "NOT_REBUILT", names, passed, failed, out
    return ("RED" if failed else "GREEN"), names, passed, failed, out


def validate():
    bad = 0
    ids = [m[0] for m in MUTANTS]
    for dupe in {i for i in ids if ids.count(i) > 1}:
        print("duplicate id", dupe)
        bad += 1
    for mid, rel, old, new, note in MUTANTS:
        text = (RUST / rel).read_text()
        n = text.count(old)
        if n != 1:
            print("!! %s: the old text occurs %d times in %s: %r" % (mid, n, rel, old[:70]))
            bad += 1
        elif old == new:
            print("!! %s: the new text equals the old" % mid)
            bad += 1
    return bad


def main():
    args = sys.argv[1:]
    only_check = "--check" in args
    want = [a for a in args if not a.startswith("--")]
    bad = validate()
    if bad:
        sys.exit("mutants: %d definitions are wrong; nothing was run" % bad)
    print("mutants: %d defined, every old text occurs exactly once" % len(MUTANTS))
    if only_check:
        return
    scratch = Path(os.environ.get("MX_SCRATCH", Path.home() / ".cache" / "magent-wt-scratch")) / "mx-checks-mutants"
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
