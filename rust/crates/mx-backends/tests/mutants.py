#!/usr/bin/env python3
"""Mutation proof for the mx-backends crate.

  python3 mutants.py            every mutant
  python3 mutants.py M03 M17    only these
  python3 mutants.py --check    only verify that every mutant applies (old text found once)

Each mutant is applied to a fresh copy of the workspace (the checkout is never
edited): every file of the copy is touched, so cargo cannot reuse an older
build; the old text must occur exactly once; the edit is read back and the
file's hash must differ from the pristine one; then `cargo test -p mx-backends
--no-fail-fast` runs in a private target directory under a time limit and an
address-space limit. A mutant is KILLED when the crate builds, the run reports
failed tests, and no failure is a compile error. BUILD_ERR, TIMEOUT, LIMIT,
NOT_REBUILT and SURVIVED are reported as such and never counted as kills.

The pristine copy runs before the batch and after it (the same test count, no
failure), and the hash of every source file of the checkout is compared before
and after. The copy holds rust/ and lib/backends, which the differential reads.

Environment: MX_SCRATCH (default ~/.cache/magent-wt-scratch), CARGO_BUILD_JOBS,
MUT_CARGO_ARGS (extra cargo test arguments, for example `--test differential`),
MUT_MIN_PASSED (the test count the pristine control must reach; default 150).
"""

import hashlib
import os
import re
import resource
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
RUST = HERE.parent.parent.parent  # rust/
ROOT = RUST.parent

S = "crates/mx-backends/src/"
# id, file, old text (exactly once), new text, what it breaks
MUTANTS = [
    # --- backend_by_name -----------------------------------------------------------
    ("M01", S + "lib.rs", '"claude" => Some(Box::new(claude::Claude)),',
     'n if n.starts_with("claude") => Some(Box::new(claude::Claude)),', "the name table matches a prefix"),
    ("M02", S + "lib.rs", '"codex" => Some(Box::new(codex::Codex)),',
     'n if n.trim() == "codex" => Some(Box::new(codex::Codex)),', "a name with spaces around it finds a backend"),
    ("M03", S + "lib.rs", '"opencode" => Some(Box::new(opencode::Opencode)),',
     'n if n.eq_ignore_ascii_case("opencode") => Some(Box::new(opencode::Opencode)),', "OPENCODE is a backend"),
    ("M04", S + "lib.rs", '"cursor" => Some(Box::new(cursor::Cursor)),',
     'n if n.contains("cursor") => Some(Box::new(cursor::Cursor)),', "any name that holds cursor is the cursor backend"),
    ("M05", S + "lib.rs", '.field("var_names", &names)', '.field("vars", &self.vars)', "the context prints variable values"),
    # --- the host ------------------------------------------------------------------
    ("M06", S + "host.rs", "if program.is_empty() || program.contains('/') {", "if program.is_empty() {",
     "a program name with a slash is searched for"),
    ("M07", S + "host.rs", "meta.is_file() && meta.permissions().mode() & 0o111 != 0", "meta.is_file()",
     "a file without an execute bit is a program"),
    ("M08", S + "host.rs", "meta.is_file() && meta.permissions().mode() & 0o111 != 0", "meta.permissions().mode() & 0o111 != 0",
     "a directory with an execute bit is a program"),
    ("M09", S + "host.rs", "self.dirs.iter().any(|dir| is_executable_file(&dir.join(program)))",
     "self.dirs.iter().all(|dir| is_executable_file(&dir.join(program)))", "every search path entry must hold the program"),
    ("M10", S + "host.rs", 'joined.push("/.cargo/bin:");', 'joined.push("/.cargo/binx:");', "$HOME/.cargo/bin is not searched"),
    ("M11", S + "host.rs", 'joined.push("/.local/bin:");', 'joined.push("/.local/binx:");', "$HOME/.local/bin is not searched"),
    ("M12", S + "host.rs", '!matches!(self.declared(path, "is_file"), Declared::Absent)',
     'matches!(self.declared(path, "is_file"), Declared::File(_))', "an unreadable file is not a file"),
    ("M13", S + "host.rs", 'Declared::File(bytes) => Some(bytes.len() as u64),', "Declared::File(bytes) => Some(bytes.len() as u64 + 1),",
     "the fake reports one byte too many"),
    # --- claude --------------------------------------------------------------------
    ("M14", S + "claude.rs", '"--permission-mode".to_string(), "bypassPermissions".to_string()',
     '"--permission-mode".to_string(), "default".to_string()', "the permission mode flag value"),
    ("M15", S + "claude.rs", '"--add-dir".to_string(),', '"--add-dirs".to_string(),', "a flag renamed"),
    ("M16", S + "claude.rs", 'words(&["--output-format", "stream-json", "--verbose"])', 'words(&["--output-format", "stream-json"])',
     "--verbose dropped"),
    ("M17", S + "claude.rs", "agent.stdout = Sink::NextStage;", "agent.stdout = Sink::Null;", "claude's output no longer reaches the filter"),
    ("M18", S + "claude.rs", "agent.stderr = Sink::AppendFile(request.log.clone());\n            let log",
     "agent.stderr = Sink::Null;\n            let log", "claude's standard error is lost"),
    ("M19", S + "claude.rs", 'log.strip_suffix(".log")', 'log.strip_suffix("log")', "the commands file keeps a dot from the log name"),
    ("M20", S + "claude.rs", 'format!("{stem}.commands")', 'format!("{stem}.cmds")', "the commands file name"),
    ("M21", S + "claude.rs", "second.stdin = Source::PreviousStage;", "second.stdin = Source::Empty;", "the filter reads nothing"),
    ("M22", S + "claude.rs", "second.stderr = Sink::AppendFile(request.log.clone());", "second.stderr = Sink::Null;",
     "a filter crash is lost"),
    ("M23", S + "claude.rs",
     'nonempty(ctx, &format!("THINKING_{}", ctx.task))\n            .or_else(|| nonempty(ctx, "MAINTAINER_THINKING"))',
     'nonempty(ctx, "MAINTAINER_THINKING")\n            .or_else(|| nonempty(ctx, &format!("THINKING_{}", ctx.task)))',
     "the general thinking budget wins over the task's"),
    ("M24", S + "claude.rs", 'name: "MAX_THINKING_TOKENS".to_string()', 'name: "MAX_THINKING_TOKEN".to_string()', "wrong variable name"),
    ("M25", S + "claude.rs", 'name: "CLAUDE_CODE_SUBAGENT_MODEL".to_string(),', 'name: "CLAUDE_CODE_SUBAGENT_MODEL_FORCE".to_string(),',
     "the forced subagent variable is set"),
    ("M26", S + "claude.rs", 'let subagent = nonempty(ctx, &format!("SUBAGENT_MODEL_{}", ctx.task));',
     'let subagent = ctx.var(&format!("SUBAGENT_MODEL_{}", ctx.task));', "an empty subagent model is exported"),
    ("M27", S + "claude.rs", "agent.env = exports.iter().map(|e| (e.name.clone(), e.value.clone())).collect();", "agent.env = Vec::new();",
     "the stage does not carry the variables"),
    ("M28", S + "claude.rs", "this run records prose only", "this run records no prose", "the fallback note wording"),
    ("M29", S + "claude.rs", "agent.stderr = Sink::SameAsStdout;", "agent.stderr = Sink::Null;", "the text fallback loses standard error"),
    ("M30", S + "claude.rs", 'path.push("/transcript.py");', 'path.push("transcript.py");', "the filter path loses its slash"),
    ("M31", S + "claude.rs", 'return Err("claude not on PATH".to_string());', 'return Err("claude is not on PATH".to_string());',
     "the check message"),
    ("M32", S + "claude.rs", 'return Err(format!("missing {}", shown(&settings)));', 'return Err(format!("absent {}", shown(&settings)));',
     "the missing settings message"),
    ("M33", S + "claude.rs", 'dir_file(&ctx.profile_dir, "settings-rehearsal.json")', 'dir_file(&ctx.profile_dir, "settings-rehearsal.jsn")',
     "the wrong wall file"),
    ("M34", S + "claude.rs", "fn declares_rehearsal(&self) -> bool {\n        true", "fn declares_rehearsal(&self) -> bool {\n        false",
     "claude declares no rehearsal"),
    ("M35", S + "claude.rs", 'let settings = text(&settings_or(ctx, "settings.json"))?;',
     'let settings = text(&dir_file(&ctx.profile_dir, "settings.json"))?;', "the exported wall is ignored"),
    ("M36", S + "claude.rs", 'Some(Export { name: "MAINTAINER_SETTINGS".to_string()', 'Some(Export { name: "MAINTAINER_SETTING".to_string()',
     "the wall variable name"),
    # --- shared plan shapes ---------------------------------------------------------
    ("M37", S + "util.rs", "stage.cwd = Some(ctx.repo_path.clone());", "stage.cwd = None;", "the agent does not start in the checkout"),
    ("M38", S + "util.rs", "stage.stderr = Sink::SameAsStdout;", "stage.stderr = Sink::Null;", "standard error is lost"),
    ("M39", S + "util.rs", "stage.stdout = Sink::AppendFile(log.to_path_buf());", "stage.stdout = Sink::Null;", "standard output is lost"),
    ("M40", S + "util.rs", "stage.stdin = Source::File(prompt.to_path_buf());", "stage.stdin = Source::Empty;", "the prompt is not read"),
    ("M41", S + "util.rs", '    let mut joined = dir.as_os_str().to_owned();\n    joined.push("/");\n    joined.push(rest);\n    PathBuf::from(joined)',
     "    dir.join(rest)", "a directory ending in a slash loses its double slash"),
    ("M42", S + "util.rs", "Duration::from_hours(3)", "Duration::from_hours(2)", "the deadline"),
    ("M43", S + "util.rs", ".filter(|value| !value.is_empty())", ".filter(|_| true)", "an empty variable counts as set"),
    # --- cursor ---------------------------------------------------------------------
    ("M44", S + "cursor.rs", "if len >= ARGUMENT_LIMIT {", "if len > ARGUMENT_LIMIT {", "the size limit is off by one"),
    ("M45", S + "cursor.rs", "const ARGUMENT_LIMIT: u64 = 131_072;", "const ARGUMENT_LIMIT: u64 = 131_071;", "the size limit constant"),
    ("M46", S + "cursor.rs", "while bytes.last() == Some(&b'\\n') {", "if bytes.last() == Some(&b'\\n') {", "only one trailing newline is cut"),
    ("M47", S + "cursor.rs", "bytes.retain(|byte| *byte != 0);", "bytes.retain(|_| true);", "NUL bytes stay in the prompt"),
    ("M48", S + "cursor.rs", 'Some(&["readonly-review"])', 'Some(&["readonly-review", "review"])', "allowed tasks widened"),
    ("M49", S + "cursor.rs", '.map_or_else(|| dir_file(&ctx.profile_dir, "cursor"), PathBuf::from)',
     '.map_or_else(|| dir_file(&ctx.profile_dir, "cursor-rehearsal"), PathBuf::from)', "the live wall is the rehearsal directory"),
    ("M50", S + "cursor.rs", 'let dir = dir_file(&ctx.profile_dir, "cursor-rehearsal");', 'let dir = dir_file(&ctx.profile_dir, "cursor");',
     "the wall is the live directory"),
    ("M51", S + "cursor.rs", '("CURSOR_CONFIG_DIR".to_string()', '("CURSOR_CONFIG".to_string()', "wrong variable name"),
    ("M52", S + "cursor.rs", '"--output-format", "text", "-m"]', '"--output-format", "json", "-m"]', "the output format"),
    ("M53", S + "cursor.rs", "argv.extend([request.model.clone(), prompt]);", "argv.extend([prompt, request.model.clone()]);",
     "the model and the prompt swap places"),
    ("M54", S + "cursor.rs", "fn declares_rehearsal(&self) -> bool {\n        true", "fn declares_rehearsal(&self) -> bool {\n        false",
     "cursor declares no rehearsal"),
    ("M55", S + "cursor.rs", "return Err(refuse(TOO_LARGE));", 'return Err(refuse("too large"));', "the refusal line"),
    ("M56", S + "cursor.rs", 'dir_file(&dir, "cli-config.json")) {\n            return Err(format!("no cursor wall',
     'dir_file(&dir, "cli-config.jsn")) {\n            return Err(format!("no cursor wall', "the check looks for the wrong wall file"),
    ("M57", S + "cursor.rs", "run ./install.sh", "run install.sh", "the check message"),
    # --- opencode -------------------------------------------------------------------
    ("M58", S + "opencode.rs", "if request.model.contains('/') {", "if !request.model.contains('/') {", "the slash test is inverted"),
    ("M59", S + "opencode.rs", "bytes.retain(|byte| *byte != b'\\n' && *byte != 0);", "bytes.retain(|byte| *byte != 0);",
     "the newline strip is removed"),
    ("M60", S + "opencode.rs", ".is_none()\n            && !host.is_file(&Self::key_file(ctx))", ".is_none()\n            || !host.is_file(&Self::key_file(ctx))",
     "the credential check is weakened to or"),
    ("M61", S + "opencode.rs", '            && !host.is_file(&dir_file(&ctx.home, ".local/share/opencode/auth.json"))\n', "",
     "auth.json is not a credential"),
    ("M62", S + "opencode.rs", 'if nonempty(ctx, "GROQ_API_KEY").is_none()', 'if ctx.var("GROQ_API_KEY").is_none()',
     "an empty key counts as a credential"),
    ("M63", S + "opencode.rs", 'ctx.var("GROQ_API_KEY").is_none_or(str::is_empty)', 'ctx.var("GROQ_API_KEY").is_none()',
     "an empty environment key is not replaced by the file"),
    ("M64", S + "opencode.rs", '("GROQ_API_KEY".to_string(), key)', '("GROQ_KEY".to_string(), key)', "wrong variable name for the key"),
    ("M65", S + "opencode.rs", '("OPENCODE_CONFIG".to_string()', '("OPENCODE_CONFIGS".to_string()', "wrong variable name for the config"),
    ("M66", S + "opencode.rs", 'dir_file(&ctx.profile_dir, "opencode-rehearsal.json")', 'dir_file(&ctx.profile_dir, "opencode-rehearsal.jso")',
     "the wrong wall file"),
    ("M67", S + "opencode.rs", 'words(&["--auto", "--format", "default"])', 'words(&["--format", "default"])', "--auto dropped"),
    ("M68", S + "opencode.rs", 'words(&["opencode", "run", "--model"])', 'words(&["opencode", "run", "-m"])', "a flag renamed"),
    ("M69", S + "opencode.rs", 'nonempty(ctx, "OPENCODE_MODEL")', 'ctx.var("OPENCODE_MODEL")', "an empty OPENCODE_MODEL is accepted"),
    ("M70", S + "opencode.rs", 'dir_file(&ctx.home, ".groq-staging-key")', 'dir_file(&ctx.home, ".groq-staging-keys")', "the wrong key file"),
    ("M71", S + "opencode.rs", "no Groq credential: set GROQ_API_KEY", "no Groq credential: set GROQ_KEY", "the check message"),
    ("M72", S + "opencode.rs", "fn declares_rehearsal(&self) -> bool {\n        true", "fn declares_rehearsal(&self) -> bool {\n        false",
     "opencode declares no rehearsal"),
    ("M73", S + "opencode.rs", "            argv.push(model);\n", "            argv.push(model);\n            if let Some(k) = &key_from_file {\n                argv.push(k.clone());\n            }\n",
     "the key is passed in argv"),
    ("M74", S + "opencode.rs", "Err(PlanRefusal {\n                log_note: \"OPENCODE_MODEL: lib/defaults.env was not loaded\".to_string(),",
     "Err(PlanRefusal {\n                log_note: \"OPENCODE_MODEL is unset\".to_string(),", "the refusal text"),
    # --- codex ----------------------------------------------------------------------
    ("M75", S + "codex.rs", '"workspace-write", "-C"]', '"read-only", "-C"]', "the sandbox mode"),
    ("M76", S + "codex.rs", 'words(&["--skip-git-repo-check", "-"])', 'words(&["--skip-git-repo-check"])', "the stdin marker is dropped"),
    ("M77", S + "codex.rs", "argv.push(text(&ctx.repo_path)?);", "argv.push(text(&request.extra_dir)?);", "-C names the state directory"),
    ("M78", S + "codex.rs", "fn declares_rehearsal(&self) -> bool {\n        false", "fn declares_rehearsal(&self) -> bool {\n        true",
     "declares_rehearsal flipped: codex claims it can rehearse"),
    ("M79", S + "codex.rs", 'Err("codex not on PATH".to_string())', 'Err("codex is not on PATH".to_string())', "the check message"),
    ("M80", S + "codex.rs", 'argv.extend(["-m".to_string(), request.model.clone()]);', 'argv.extend(["--model".to_string(), request.model.clone()]);',
     "a flag renamed"),
    # --- more argument, stream and host mutants ---------------------------------------
    ("M81", S + "claude.rs", 'words(&["claude", "-p", "--settings"])', 'words(&["claude", "--settings"])', "-p dropped"),
    ("M82", S + "cursor.rs", 'words(&["cursor-agent", "-p", "--output-format", "text", "-m"])',
     'words(&["cursor-agent", "-p", "--force", "--output-format", "text", "-m"])', "--force is passed"),
    ("M83", S + "opencode.rs", "bytes.retain(|byte| *byte != b'\\n' && *byte != 0);", "bytes.retain(|byte| *byte != b'\\n');",
     "NUL bytes stay in the key"),
    ("M84", S + "opencode.rs", "to_log(stage(argv, ctx), Some(&request.prompt_file), &request.log)", "to_log(stage(argv, ctx), None, &request.log)",
     "the prompt is not on stdin"),
    ("M85", S + "codex.rs", "to_log(stage(argv, ctx), Some(&request.prompt_file), &request.log)", "to_log(stage(argv, ctx), None, &request.log)",
     "the prompt is not on stdin"),
    ("M86", S + "cursor.rs", "to_log(stage(argv, ctx), None, &request.log)", "to_log(stage(argv, ctx), Some(&request.prompt_file), &request.log)",
     "the prompt is also on stdin"),
    ("M87", S + "host.rs", "std::fs::metadata(path).ok().map(|meta| meta.len())", "std::fs::metadata(path).ok().map(|meta| meta.len() + 1)",
     "the real host reports one byte too many"),
    ("M88", S + "host.rs", "std::fs::read(path).ok()", "None", "the real host reads nothing"),
    ("M89", S + "host.rs", "std::fs::metadata(path).is_ok_and(|meta| meta.is_file())", "std::fs::metadata(path).is_ok()",
     "a directory is a file"),
    ("M90", S + "claude.rs", "let filter_present = host.is_file(&filter);", "let filter_present = !host.is_file(&filter);",
     "the filter test is inverted"),
    # --- from the review of #263: mutants its scenarios killed and these tests did not -----
    ("M91", S + "cursor.rs", "while bytes.last() == Some(&b'\\n') {", "while matches!(bytes.last(), Some(b'\\n' | b'\\r')) {",
     "a trailing CR is cut with the newlines"),
    ("M92", S + "util.rs", ".filter(|value| !value.is_empty())", ".filter(|value| !value.trim().is_empty())",
     "a variable set to whitespace reads as unset"),
    ("M93", S + "host.rs", "std::fs::metadata(path).ok().map(|meta| meta.len())", "std::fs::symlink_metadata(path).ok().map(|meta| meta.len())",
     "the length of a symlinked prompt is the length of the link"),
    ("M94", S + "util.rs", "path.to_str().map(str::to_string).ok_or_else(|| PlanRefusal {",
     "Some(path.to_string_lossy().into_owned()).ok_or_else(|| PlanRefusal {", "a path that is not UTF-8 is passed lossy"),
    ("M95", S + "claude.rs", "value: wall.to_str()?.to_string()", "value: wall.to_string_lossy().into_owned()",
     "a wall path that is not UTF-8 is exported lossy"),
    ("M96", S + "opencode.rs", 'nonempty(ctx, "OPENCODE_MODEL")', 'ctx.var("OPENCODE_MODEL").filter(|v| !v.trim().is_empty())',
     "an OPENCODE_MODEL of whitespace reads as unset"),
    ("M97", S + "codex.rs", "let agent = to_log(stage(argv, ctx), Some(&request.prompt_file), &request.log);",
     'let mut agent = to_log(stage(argv, ctx), Some(&request.prompt_file), &request.log);\n        agent.env.push(("EVIL_EXTRA".to_string(), "1".to_string()));',
     "the stage gains a variable no stub logs"),
    ("M98", S + "util.rs", "stage.stderr = Sink::SameAsStdout;", "stage.stderr = Sink::AppendFile(log.to_path_buf());",
     "2>&1 becomes a second append to the log"),
    ("M99", S + "claude.rs", "agent.env = exports.iter().map(|e| (e.name.clone(), e.value.clone())).collect();",
     'agent.env = exports.iter().map(|e| (e.name.clone(), e.value.clone())).collect();\n        agent.env.push(("HOME".to_string(), "/root".to_string()));',
     "HOME is added to the stage environment"),
    ("M100", S + "opencode.rs", ["if let Some(key) = key_from_file {", "exports: Vec::new(),"],
     ["if let Some(key) = key_from_file.clone() {",
      'exports: key_from_file.iter().map(|k| Export { name: "GROQ_API_KEY".to_string(), value: k.clone() }).collect(),'],
     "the key is also exported to the whole run"),
    # --- from the follow-up to the review: the key is planned only when the backend read it --
    ("M101", S + "opencode.rs", 'ctx.var("GROQ_API_KEY").is_none_or(str::is_empty)', "true",
     "the key file is read although the environment holds a key"),
    ("M102", S + "opencode.rs", "if let Some(key) = key_from_file {",
     'if let Some(key) = key_from_file.or_else(|| ctx.var("GROQ_API_KEY").map(str::to_string)) {',
     "a key from the environment is named in the plan"),
    ("M103", S + "cursor.rs", "bytes.retain(|byte| *byte != 0);", "bytes.retain(|byte| *byte != 0 && *byte != b'\\r');",
     "every CR is cut from the prompt"),
    ("M104", S + "util.rs", "stage.cwd = Some(ctx.repo_path.clone());", "stage.cwd = Some(PathBuf::from(shown(&ctx.repo_path)));",
     "the working directory is a lossy copy of the checkout path"),
    ("M105", S + "claude.rs", '"python3".to_string(),', '"python".to_string(),', "the filter runs under another program name"),
]

LIMIT_BYTES = 24 << 30  # address space of one cargo run; rustc maps a lot, so this is generous


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
    """dest/rust is the workspace; dest/lib/backends is what the differential sources."""
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(RUST, dest / "rust", ignore=shutil.ignore_patterns("target", "__pycache__"), symlinks=True)
    shutil.copytree(ROOT / "lib" / "backends", dest / "lib" / "backends", symlinks=True)


def touch_all(root):
    now = time.time()
    for path in Path(root).rglob("*"):
        if path.is_file():
            os.utime(path, (now, now))


def limits():
    resource.setrlimit(resource.RLIMIT_AS, (LIMIT_BYTES, LIMIT_BYTES))


def cargo_test(copy, target, timeout):
    env = dict(os.environ)
    env.update({"CARGO_TARGET_DIR": str(target), "CARGO_TERM_COLOR": "never", "PYTHONDONTWRITEBYTECODE": "1",
                "CARGO_BUILD_JOBS": os.environ.get("CARGO_BUILD_JOBS", "2")})
    env.pop("DIFF_SEED", None)
    env.pop("MX_TREE", None)
    try:
        cp = subprocess.run(["cargo", "test", "-p", "mx-backends", "--locked", "--offline", "--no-fail-fast"] + os.environ.get("MUT_CARGO_ARGS", "").split(), cwd=copy / "rust", env=env,
                            capture_output=True, text=True, timeout=timeout, preexec_fn=limits)
    except subprocess.TimeoutExpired as error:
        return "TIMEOUT", [], 0, 0, str(error.stdout or "")
    out = cp.stdout + cp.stderr
    if "memory allocation" in out and "failed" in out or "Cannot allocate memory" in out:
        return "LIMIT", [], 0, 0, out
    if "could not compile" in out or re.search(r"^error(\[E\d+\])?: (?!test failed|\d+ targets? failed)", out, re.M):
        return "BUILD_ERR", [], 0, 0, out
    passed = sum(int(n) for n in re.findall(r"test result: \w+\. (\d+) passed", out))
    failed = sum(int(n) for n in re.findall(r"; (\d+) failed", out))
    names = re.findall(r"^test (\S+) \.\.\. FAILED$", out, re.M)
    if "Compiling mx-backends" not in out:
        return "NOT_REBUILT", names, passed, failed, out
    return ("RED" if failed else "GREEN"), names, passed, failed, out


def apply(work, rel, old, new, mid):
    """old and new are strings, or lists of strings of one length (several edits to one file)."""
    olds = old if isinstance(old, list) else [old]
    news = new if isinstance(new, list) else [new]
    assert len(olds) == len(news), "%s: old and new differ in length" % mid
    path = work / "rust" / rel
    text = path.read_text()
    for one in olds:
        if text.count(one) != 1:
            sys.exit("mutants: %s: the old text occurs %d times in %s: %r" % (mid, text.count(one), rel, one))
    before_hash = sha(path)
    assert sha(RUST / rel) == before_hash, "the copy differs from the checkout before the edit"
    for one, other in zip(olds, news):
        text = text.replace(one, other)
    path.write_text(text)
    back = path.read_text()
    if any(other not in back for other in news) or sha(path) == before_hash:
        sys.exit("mutants: %s: the edit did not land" % mid)


def main():
    args = sys.argv[1:]
    check_only = "--check" in args
    want = [a for a in args if a != "--check"]
    scratch = Path(os.environ.get("MX_SCRATCH", Path.home() / ".cache" / "magent-wt-scratch")) / "mx-backends-mutants"
    scratch.mkdir(parents=True, exist_ok=True)
    work = scratch / "work"
    chosen = [m for m in MUTANTS if not want or m[0] in want]
    if len({m[0] for m in MUTANTS}) != len(MUTANTS):
        sys.exit("mutants: duplicate ids")
    if check_only:
        for mid, rel, old, new, note in chosen:
            copy_workspace(work)
            apply(work, rel, old, new, mid)
            print("%s applies to %s" % (mid, rel))
        print("mutants: %d apply" % len(chosen))
        return
    target = scratch / "target"
    pristine = scratch / "pristine"
    copy_workspace(pristine)
    live_before = tree_hash(RUST)
    pristine_hash = tree_hash(pristine)

    def control(label):
        copy_workspace(work)
        touch_all(work)
        status, names, passed, failed, out = cargo_test(work, target, 1800)
        print("control %-6s %s: %d passed, %d failed" % (label, status, passed, failed), flush=True)
        if status != "GREEN" or passed < int(os.environ.get("MUT_MIN_PASSED", "150")):
            print(out[-3000:])
            sys.exit("mutants: the pristine control is not green with the full suite")
        return passed

    before = control("before")
    rows, survivors, killed = [], [], 0
    for mid, rel, old, new, note in chosen:
        copy_workspace(work)
        apply(work, rel, old, new, mid)
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
        print("%-4s %-11s %-8s %3d red  %s  [%s]" % (mid, rel.split("/")[-1], outcome, failed, first, note), flush=True)
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
