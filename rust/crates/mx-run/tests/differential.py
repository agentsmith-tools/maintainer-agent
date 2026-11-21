#!/usr/bin/env python3
"""The scenarios and the runner behind differential.sh. Not meant to run alone.

Every scenario builds one sandbox (a tree copy with a fixture profile, a state
directory, a checkout, stubs for every external tool, and answer files), copies
it, runs the bash `lib/run.sh` on the copy, restores the sandbox, runs the Rust
build on it, and compares what each produced:

  the exit code, standard output and standard error, separately;
  the call log: ONE global file, one line per call to gh, the helper, flock,
    notify-send, loginctl, sleep, date and the four agent CLIs, in order, with the
    argument vector, the working directory, where descriptor 9 points, the sha256
    of standard input, the names of the environment variables (as one digest) and
    the values of the ones the run exports;
  the state tree: every file under the sandbox home, with its bytes, and what the
    run left in TMPDIR.

Both sides run `bash <sandbox>/tree/lib/run.sh ARGS` in the same directory under
`env -i`, with the same PATH, clock and locale, so the sandbox needs no path
rewriting. The Rust side differs in one file: its `lib/run.sh` is the wrapper
that assemble-tree.sh writes, which execs the Rust binary with its own path as
argv[0].

A scenario is either IDENTICAL (everything above must match byte for byte) or
DECISION (it exercises a plan decision that is fixed on purpose, and `explain`
must accept the exact difference). A difference that is not explained is a
failure.

The scenarios are generated from a seed. The seed and the name of a scenario fix
its content, so a failing run repeats with DIFF_SEED and DIFF_ONLY=<name>.
"""

import difflib
import hashlib
import os
import random
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path

TOOLS = ("awk basename bash cat chmod cmp cp cut dash dirname env false find git grep head id ln ls mkdir "
         "mktemp mv python3 readlink rm sed sh sort stat tail tee touch tr true uniq wc xargs timeout "
         "sha256sum").split()
TOKEN = "ghp_FIXTURE0123456789abcdefghij"
EPOCH = 1767225600
STAMP = "2026-01-01T00-00"
ISO = "2026-01-01T00:00:00+00:00"
RUNID = STAMP + "-review"
LOGGED_ENV = ("MAINTAINER_POST MAINTAINER_TASK MAINTAINER_TASKS MAINTAINER_PROFILE MAINTAINER_IN_RUN MAINTAINER_FORCE "
              "MAINTAINER_VERSION MAINTAINER_COMMIT MAINTAINER_SKILL MAINTAINER_SETTINGS MAINTAINER_SCRIPTS "
              "MAINTAINER_REPO MAINTAINER_SLUG MAINTAINER_STATE MAINTAINER_ACCOUNT PROFILE_DIR REPO_PATH CLAIM_LABEL "
              "PROD_GLOBS MAX_THINKING_TOKENS CLAUDE_CODE_SUBAGENT_MODEL OPENCODE_CONFIG MAINTAINER_CURSOR_DIR "
              "CURSOR_CONFIG_DIR DISPLAY DBUS_SESSION_BUS_ADDRESS LC_ALL TZ HOME USER TMPDIR").split()

STUB = r'''#!/usr/bin/env bash
# One stub for every external tool. Logs one line per call to the global call log,
# then answers from the case's answer files.
name="$(basename "$0")"
d="${DC_CASE:?}"
args=("$@")
if [ "$name" = maintainer ] && [ "${1:-}" = run-lease ] && [ "${#args[@]}" -ge 3 ]; then
    if [ "${args[2]}" = "$PPID" ]; then args[2]='<ppid>'; else args[2]='<other-pid>'; fi
fi
q=""; for a in "${args[@]}"; do q="$q$(printf '%q' "$a") "; done
if [ -t 0 ]; then sh_in=tty; else sh_in="$(sha256sum | cut -c1-12)"; fi
tok=unset
if [ -n "${GH_TOKEN+x}" ]; then
    if [ "$GH_TOKEN" = "$(cat "$d/token")" ]; then tok=match; else tok=other; fi
fi
gts=unset; [ -n "${GITHUB_TOKEN+x}" ] && gts=set
sel=""
for v in __LOGGED__; do
    if [ -n "${!v+x}" ]; then sel="$sel$v=${!v};"; else sel="$sel$v=<unset>;"; fi
done
if [ -n "${GROQ_API_KEY+x}" ]; then sel="${sel}GROQ_API_KEY=<set>;"; fi
names="$(env | cut -d= -f1 | grep -v -x -E '_|SHLVL|PWD|OLDPWD' | sort | tr '\n' ',' | sha256sum | cut -c1-12)"
printf '%s|%s|cwd=%s|fd9=%s|stdin=%s|GH_TOKEN=%s|GITHUB_TOKEN=%s|names=%s|%s\n' \
    "$name" "${q% }" "$PWD" "$(readlink "/proc/$$/fd/9" 2>/dev/null || echo none)" "$sh_in" "$tok" "$gts" "$names" "$sel" >> "$d/calls.log"
# answers: ans/KEY.N.{out,err,rc} for the Nth call of a key, else ans/KEY.{out,err,rc}
key=""
case "$name" in
    gh)         key="gh-${1:-}-${2:-}" ;;
    maintainer) key="h-${1:-}" ;;
    claude|codex|opencode|cursor-agent) key=agent ;;
    flock)      key=flock ;;
    loginctl)   key="loginctl-${1:-}" ;;
    notify-send) key=notify ;;
    sleep)      key=sleep ;;
esac
if [ "$name" = date ]; then
    exec "$DC_REAL_DATE" -d "@$(cat "$d/now")" "$@"
fi
n=0; [ -e "$d/ans/$key.count" ] && n="$(cat "$d/ans/$key.count")"
n=$((n + 1)); printf '%s' "$n" > "$d/ans/$key.count"
base="$d/ans/$key"; [ -e "$d/ans/$key.$n.out" ] || [ -e "$d/ans/$key.$n.rc" ] && base="$d/ans/$key.$n"
if [ "$name" = flock ] && [ -e "$d/flock.touch" ]; then
    touch -d "@$(cat "$d/now")" "$(cat "$d/flock.touch")"
fi
if [ -e "$d/agent.behave" ] && [ "$key" = agent ]; then . "$d/agent.behave"; fi
[ -e "$base.out" ] && cat "$base.out"
[ -e "$base.err" ] && cat "$base.err" >&2
rc=0; [ -e "$base.rc" ] && rc="$(cat "$base.rc")"
exit "$rc"
'''.replace("__LOGGED__", " ".join(LOGGED_ENV))

PROFILE = '''PROFILE_NAME="fx"
REPO_PATH="${MAINTAINER_REPO_PATH:-$HOME/.local/state/fx-maint/checkout}"
REPO_ORIGIN="${MAINTAINER_REPO_ORIGIN:-}"
REPO_SLUG="fx/repo"
MAINTAINER_NAME="Fixture Maintainer"
GH_ACCOUNT="fxacct"
STATE_DIR="${MAINTAINER_STATE_DIR:-$HOME/.local/state/fx-maint}"
HELPER="$HOME/.local/bin/maintainer"
TASKS="review issues"
MODEL_review="opus"
MODEL_issues="opus"
THINKING_review=31999
THINKING_issues=16000
SUBAGENT_MODEL_review="sonnet"
MIN_HOURS_review=6
MIN_HOURS_issues=36
POST="${MAINTAINER_POST:-on}"
PROSE_STYLE="${MAINTAINER_PROSE_STYLE:-stop-slop}"
BACKEND="${MAINTAINER_BACKEND:-claude}"
CLAIM_LABEL=""
PROD_GLOBS="bin/* lib/*"
'''


def sha(data):
    return hashlib.sha256(data).hexdigest()


class Sandbox:
    """One case directory and the means to build it."""

    def __init__(self, work, repo, rust_tree, realtools):
        self.work = Path(work)
        self.dir = self.work / "case"
        self.snap = self.work / "snapshot"
        self.repo = Path(repo)
        self.rust_tree = Path(rust_tree)
        self.tools = realtools
        self.env = {}
        self.runner_args = []

    def build(self):
        if self.dir.exists():
            shutil.rmtree(self.dir)
        d = self.dir
        for sub in ("home/.local/bin", "home/.local/state/fx-maint/checkout/.git", "bin", "ans", "tmp",
                    "tree/scripts", "tree/profiles/fx/prompts"):
            (d / sub).mkdir(parents=True)
        shutil.copytree(self.repo / "lib", d / "tree" / "lib", symlinks=True)
        shutil.copy(self.repo / "scripts" / "transcript.py", d / "tree" / "scripts" / "transcript.py")
        (d / "tree" / "lib" / "preamble-core.md").write_text(
            "CORE doctrine for __MAINTAINER__ working on __SLUG__ (again __MAINTAINER__)\n")
        (d / "tree" / "lib" / "prose-style.md").write_text("PROSE-STYLE rules\n")
        (d / "tree" / "VERSION").write_text("version=9.9.9\ncommit=abc1234\n")
        p = d / "tree" / "profiles" / "fx"
        (p / "profile.env").write_text(PROFILE)
        (p / "prompts" / "common-preamble.md").write_text("COMMON-PREAMBLE for fx\nsecond preamble line\n")
        (p / "prompts" / "review.md").write_text("TASK review body\n")
        (p / "prompts" / "issues.md").write_text("TASK issues body\n")
        (p / "settings.json").write_text('{"permissions": {"deny": []}}\n')
        (p / "opencode.json").write_text('{"model": "x"}\n')
        (d / "stub").write_text(STUB)
        os.chmod(d / "stub", 0o755)
        for name in ("gh", "claude", "codex", "opencode", "cursor-agent", "notify-send", "loginctl", "sleep",
                     "date", "flock"):
            (d / "bin" / name).symlink_to("../stub")
        (d / "home" / ".local" / "bin" / "maintainer").symlink_to("../../../stub")
        (d / "tools").mkdir()
        for name, target in self.tools.items():
            (d / "tools" / name).symlink_to(target)
        (d / "token").write_text(TOKEN + "\n")
        (d / "now").write_text("%d\n" % EPOCH)
        (d / "calls.log").write_text("")
        self.env = {
            "PATH": "%s:%s" % (d / "bin", d / "tools"),
            "HOME": str(d / "home"),
            "LC_ALL": "C",
            "TZ": "UTC",
            "USER": "fixture-user",
            "LOGNAME": "fixture-user",
            "TMPDIR": str(d / "tmp"),
            "DC_CASE": str(d),
            "DC_REAL_DATE": self.tools["date"],
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_TERMINAL_PROMPT": "0",
            "MAINTAINER_GH_BACKOFF": "0",
        }
        self.runner_args = []
        # What the real tools would print, so a scenario that does not care gets a
        # run that works from the first call to the last.
        self.ans("gh-auth-switch", "", 0)
        self.ans("gh-api-user", "fxacct", 0)
        self.ans("gh-auth-token", TOKEN, 0)
        self.ans("h-start", "=== maintainer %s ===\nskill        fixture-skill" % RUNID, 0)
        self.ans("h-finish", "finished " + RUNID, 0)
        self.ans("h-digest", "digest-line-from-helper", 0)
        self.ans("agent", '{"type":"assistant","message":{"content":[{"type":"text","text":"agent says hello"},'
                 '{"type":"tool_use","name":"Bash","input":{"command":"echo hi"}}]}}', 0)

    # -- the sandbox ----------------------------------------------------------------
    @property
    def state(self):
        return self.dir / "home" / ".local" / "state" / "fx-maint"

    @property
    def profile_dir(self):
        return self.dir / "tree" / "profiles" / "fx"

    def ans(self, key, out="", rc=0, err=None):
        base = self.dir / "ans" / key
        Path(str(base) + ".out").write_text(out + ("\n" if out and not out.endswith("\n") else ""))
        Path(str(base) + ".rc").write_text(str(rc))
        if err is not None:
            Path(str(base) + ".err").write_text(err)

    def profile_add(self, line):
        with open(self.profile_dir / "profile.env", "a") as f:
            f.write(line + "\n")

    def last_ran(self, task, seconds_ago):
        (self.state / "state").mkdir(parents=True, exist_ok=True)
        f = self.state / "state" / ("last-%s.json" % task)
        f.write_text("{}\n")
        os.utime(f, (EPOCH - seconds_ago, EPOCH - seconds_ago))

    def leak(self, path):
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("before %s after\n" % TOKEN)

    def wall(self):
        p = self.profile_dir
        (p / "settings-rehearsal.json").write_text('{"permissions": {"deny": ["gh issue create"]}}\n')
        (p / "opencode-rehearsal.json").write_text('{"model": "rehearsal"}\n')
        for sub in ("cursor-rehearsal", "cursor"):
            (p / sub).mkdir(exist_ok=True)
            (p / sub / "cli-config.json").write_text("{}\n")

    def rm(self, rel):
        target = self.dir / rel
        if target.is_dir():
            shutil.rmtree(target)
        elif target.exists() or target.is_symlink():
            target.unlink()

    def behave(self, code):
        (self.dir / "agent.behave").write_text(code)

    # -- running ------------------------------------------------------------------------
    def snapshot(self):
        if self.snap.exists():
            shutil.rmtree(self.snap)
        subprocess.run(["cp", "-a", str(self.dir), str(self.snap)], check=True)
        (self.work / "env.json").write_text("")

    def restore(self):
        subprocess.run(["chmod", "-R", "u+rwX", str(self.dir)], check=False)
        shutil.rmtree(self.dir)
        subprocess.run(["cp", "-a", str(self.snap), str(self.dir)], check=True)

    def use_rust(self):
        """The Rust side differs in one file: lib/run.sh is the wrapper of the Rust build."""
        wrapper = self.rust_tree / "lib" / "run.sh"
        target = self.dir / "tree" / "lib" / "run.sh"
        target.unlink()
        shutil.copy(wrapper, target)
        os.chmod(target, 0o755)

    def run(self, args):
        env = ["%s=%s" % (k, v) for k, v in self.env.items()]
        out, err = self.dir / "stdout.txt", self.dir / "stderr.txt"
        cmd = ["env", "-i"] + env + [self.tools["timeout"], "60", self.tools["bash"], str(self.dir / "tree/lib/run.sh")] + list(args)
        with open(out, "wb") as o, open(err, "wb") as e, open(os.devnull, "rb") as i:
            cp = subprocess.run(["bash", "-c", 'exec 9>&-; exec "$@"', "x"] + cmd, stdin=i, stdout=o, stderr=e)
        return cp.returncode

    def collect(self, rc):
        d = self.dir
        files = {}
        home = d / "home"
        for path in sorted(home.rglob("*")):
            rel = str(path.relative_to(d))
            if rel.startswith("home/.local/bin"):
                continue
            if path.is_symlink():
                files[rel] = "link:" + os.readlink(path)
            elif path.is_file():
                files[rel] = "sha:" + sha(path.read_bytes())[:16] + " size:%d" % path.stat().st_size
            elif path.is_dir():
                files[rel] = "dir"
        tmp = sorted(str(p.relative_to(d)) for p in (d / "tmp").rglob("*"))
        return {
            "rc": rc,
            "stdout": (d / "stdout.txt").read_bytes().decode("utf-8", "replace"),
            "stderr": (d / "stderr.txt").read_bytes().decode("utf-8", "replace"),
            "calls": (d / "calls.log").read_bytes().decode("utf-8", "replace"),
            "files": files,
            "tmp": tmp,
            "texts": {rel: (d / rel).read_bytes().decode("utf-8", "replace")
                      for rel in files if files[rel].startswith("sha:") and (d / rel).stat().st_size < 200000},
        }


def calls_of(snapshot, tool):
    return [line for line in snapshot["calls"].splitlines() if line.startswith(tool + "|")]


# -- the scenarios ------------------------------------------------------------------------------
class Scenario:
    def __init__(self, name, build, args=("fx", "review"), explain=None):
        self.name = name
        self.build = build  # fn(sb) mutates the sandbox
        self.args = list(args)
        self.explain = explain  # fn(bash, rust) -> list of problems; None means IDENTICAL


def run_both(sb, sc):
    sb.build()
    sc.build(sb)
    sb.snapshot()
    sb.restore()
    b = sb.collect(sb.run(sc.args))
    sb.restore()
    sb.use_rust()
    r = sb.collect(sb.run(sc.args))
    return b, r


def short_diff(a, b):
    """The first lines that differ, cut to a readable width."""
    lines = []
    left, right = str(a).splitlines(), str(b).splitlines()
    if left and right and all("|" in line for line in left[:1] + right[:1]) and len(left) == len(right):
        # a call log: name the fields that differ, line by line
        for number, (x, y) in enumerate(zip(left, right), 1):
            if x != y:
                fx, fy = x.split("|"), y.split("|")
                for part_x, part_y in zip(fx, fy):
                    if part_x != part_y:
                        lines.append("      call %d (%s): bash %s | rust %s" % (number, fx[0], part_x[:150], part_y[:150]))
                if len(lines) >= 8:
                    break
        if lines:
            return "\n".join(lines)
    for line in difflib.unified_diff(str(a).splitlines(), str(b).splitlines(), "bash", "rust", n=0, lineterm=""):
        if line.startswith(("---", "+++", "@@")):
            continue
        lines.append("      " + line[:260])
        if len(lines) >= 6:
            break
    return "\n".join(lines)


def diff_snapshots(b, r):
    problems = []
    for field in ("rc", "stdout", "stderr", "calls", "tmp"):
        if b[field] != r[field]:
            if field == "rc":
                problems.append("rc differs: bash %s, rust %s" % (b[field], r[field]))
            else:
                problems.append("%s differs\n%s" % (field, short_diff(b[field], r[field])))
    for rel in sorted(set(b["files"]) | set(r["files"])):
        if b["files"].get(rel) != r["files"].get(rel):
            problems.append("file %s differs\n   bash: %s\n   rust: %s" % (rel, b["files"].get(rel), r["files"].get(rel)))
    return problems


def explain_signal(b, r):
    """Decision P2A-sig: on a signal the Rust build stops the agent's process groups at once.

    The transcript filter, the second stage of the Claude pipeline, may or may not have created
    its `.commands` file by then, so that file is not a fact to compare. Everything else must match.
    """
    def strip(snapshot):
        out = dict(snapshot)
        out["files"] = {k: v for k, v in snapshot["files"].items() if not k.endswith(".commands")}
        return out

    bad = diff_snapshots(strip(b), strip(r))
    if b["rc"] not in (-15, -1, 143, 129) or b["rc"] != r["rc"]:
        bad.append("both builds must end by the signal: bash %s, rust %s" % (b["rc"], r["rc"]))
    return bad


def named_scenarios():
    S = []

    def add(name, build=lambda sb: None, args=("fx", "review"), explain=None):
        S.append(Scenario(name, build, args, explain))

    # --- arguments, tree, nested guard (every refusal and its exit code) ---------------------
    for i, args in enumerate([(), ("fx",), ("--show-prompt",), ("", "review"), ("fx", ""), ("fx", "--show-prompt"),
                              ("nope", "review"), ("fx", "bogus"), ("fx", "rev"), ("fx", "review issues"),
                              ("--show-prompt", "nope", "review"), ("--show-prompt", "fx", "bogus"),
                              ("fx", "review", "extra", "args"), ("--show-prompt", "fx", "review", "extra")]):
        add("args-%02d" % i, args=args)
    add("task-no-model", lambda sb: sb.profile_add("unset MODEL_review"))
    add("task-no-model-preview", lambda sb: sb.profile_add("unset MODEL_review"), args=("--show-prompt", "fx", "review"))
    add("task-hyphen", lambda sb: sb.profile_add('TASKS="review readonly-review"'), args=("fx", "readonly-review"))
    add("task-hyphen-preview", lambda sb: sb.profile_add('TASKS="review readonly-review"'),
        args=("--show-prompt", "fx", "readonly-review"))
    add("task-space", args=("fx", "review issues"))
    add("tasks-unbound", lambda sb: sb.profile_add("unset TASKS"))
    add("repo-path-unbound", lambda sb: sb.profile_add("unset REPO_PATH"))
    add("repo-slug-unbound", lambda sb: sb.profile_add("unset REPO_SLUG"))
    add("state-dir-unbound", lambda sb: sb.profile_add("unset STATE_DIR"))
    add("gh-account-unbound", lambda sb: sb.profile_add("unset GH_ACCOUNT"))
    add("backend-unbound", lambda sb: sb.profile_add("unset BACKEND"))
    add("no-profiles-dir", lambda sb: sb.rm("tree/profiles"))
    add("no-backends-dir", lambda sb: sb.rm("tree/lib/backends"))
    add("no-profile-lib", lambda sb: sb.rm("tree/lib/profile.sh"))
    add("no-defaults", lambda sb: sb.rm("tree/lib/defaults.env"))
    add("profile-exits-3", lambda sb: sb.profile_add("exit 3"))
    add("profile-exits-0", lambda sb: sb.profile_add("exit 0"))
    add("profile-prints", lambda sb: sb.profile_add("echo profile-says-hello; echo and-an-error >&2"))
    add("profile-last-command-fails", lambda sb: sb.profile_add("false"))
    for i, marker in enumerate(["other/task", "x", " "]):
        add("nested-%d" % i, lambda sb, m=marker: sb.env.update({"MAINTAINER_IN_RUN": m}))
        add("nested-preview-%d" % i, lambda sb, m=marker: sb.env.update({"MAINTAINER_IN_RUN": m}),
            args=("--show-prompt", "fx", "review"))
    add("nested-empty-marker", lambda sb: sb.env.update({"MAINTAINER_IN_RUN": ""}))
    add("nested-before-profile", lambda sb: sb.env.update({"MAINTAINER_IN_RUN": "a/b"}), args=("nope", "review"))

    # --- the cadence gate: the boundaries ---------------------------------------------------------------
    for label, age in (("min-1h", 6 * 3600 - 3600), ("min-1s", 6 * 3600 - 1), ("min", 6 * 3600), ("min+1s", 6 * 3600 + 1),
                       ("min+1h", 6 * 3600 + 3600), ("young", 60), ("zero", 0), ("huge", 10 ** 9)):
        add("cadence-%s" % label, lambda sb, a=age: sb.last_ran("review", a))
    add("cadence-issues-boundary", lambda sb: sb.last_ran("issues", 36 * 3600 - 1), args=("fx", "issues"))
    add("cadence-issues-runs", lambda sb: sb.last_ran("issues", 36 * 3600), args=("fx", "issues"))
    add("cadence-no-stamp-huge-min", lambda sb: sb.profile_add("MIN_HOURS_review=999999999"))
    add("cadence-min-zero", lambda sb: (sb.profile_add("MIN_HOURS_review=0"), sb.last_ran("review", 60)))
    add("cadence-min-zero-string", lambda sb: (sb.profile_add("MIN_HOURS_review=00"), sb.last_ran("review", 60)))
    add("cadence-min-unset", lambda sb: (sb.profile_add("unset MIN_HOURS_review"), sb.last_ran("review", 60)))
    add("cadence-min-empty", lambda sb: (sb.profile_add("MIN_HOURS_review="), sb.last_ran("review", 60)))
    add("cadence-min-plus", lambda sb: (sb.profile_add("MIN_HOURS_review=+6"), sb.last_ran("review", 3600)))
    add("cadence-min-negative", lambda sb: (sb.profile_add("MIN_HOURS_review=-3"), sb.last_ran("review", 3600)))
    add("cadence-min-spaces", lambda sb: (sb.profile_add('MIN_HOURS_review=" 6 "'), sb.last_ran("review", 3600)))
    add("cadence-other-task-stamp", lambda sb: sb.last_ran("issues", 60))
    add("cadence-override-not-allowed", lambda sb: (sb.last_ran("review", 3600), sb.env.update({"MAINTAINER_MIN_HOURS_review": "0"})))
    add("cadence-preview", lambda sb: sb.last_ran("review", 60), args=("--show-prompt", "fx", "review"))
    for v in ("1", "0", "true", "yes", "", "11"):
        add("force-%s" % (v or "empty"), lambda sb, v=v: (sb.last_ran("review", 3600), sb.env.update({"MAINTAINER_FORCE": v})))
    add("force-gate-two", lambda sb: (sb.last_ran("review", 72000), sb.env.update({"MAINTAINER_FORCE": "1"}),
                                      (sb.dir / "flock.touch").write_text(str(sb.state / "state" / "last-review.json"))))
    add("gate-two-promoted", lambda sb: (sb.last_ran("review", 72000),
                                         (sb.dir / "flock.touch").write_text(str(sb.state / "state" / "last-review.json"))))

    # --- plan decisions D11, D12 (fixed on purpose) ---------------------------------------------------------
    def d11(b, r):
        bad = []
        if b["rc"] != 0 or "run.sh: line" not in b["stderr"] or "integer" not in b["stderr"]:
            bad.append("bash: expected the falling-open run with a [ complaint, got rc=%s %r" % (b["rc"], b["stderr"][:120]))
        if r["rc"] != 64 or "is not a whole number of hours" not in r["stderr"] or calls_of(r, "maintainer"):
            bad.append("rust: expected exit 64, the refusal and no helper call, got rc=%s %r" % (r["rc"], r["stderr"][:120]))
        return bad

    for word in ("soon", "6h", "1.5", "0x10", "--6", "abc def"):
        add("d11-%s" % word.replace(" ", "_"), lambda sb, w=word: (sb.profile_add('MIN_HOURS_review="%s"' % w), sb.last_ran("review", 3600)),
            explain=d11)

    def d12(b, r):
        bad = []
        if b["rc"] != 0 or not b["stdout"].startswith("skipped: review last ran "):
            bad.append("bash: expected a skip with a negative age, got rc=%s %r" % (b["rc"], b["stdout"][:100]))
        if r["rc"] != 0 or r["stdout"] != "" or not calls_of(r, "maintainer"):
            bad.append("rust: expected a run, got rc=%s stdout=%r" % (r["rc"], r["stdout"][:100]))
        return bad

    for secs in (-36000, -7200, -3600, -60):
        add("d12-future-%d" % -secs, lambda sb, s=secs: sb.last_ran("review", s), explain=d12)

    # --- the lock ---------------------------------------------------------------------------------------------
    add("lock-held", lambda sb: sb.ans("flock", "", 1))
    add("lock-held-wait-5", lambda sb: (sb.ans("flock", "", 1), sb.env.update({"MAINTAINER_LOCK_WAIT": "5"})))
    add("lock-flock-rc2", lambda sb: sb.ans("flock", "", 2))
    add("lock-flock-rc64", lambda sb: (sb.ans("flock", "", 64), sb.env.update({"MAINTAINER_LOCK_WAIT": "abc"})))
    add("lock-flock-rc127", lambda sb: sb.ans("flock", "", 127))
    add("lock-flock-prints", lambda sb: sb.ans("flock", "flock says", 0, err="flock warns\n"))
    add("lock-flock-missing", lambda sb: sb.rm("bin/flock"))
    add("lock-preview", args=("--show-prompt", "fx", "review"))

    def lock_real(sb):
        sb.rm("bin/flock")
        (sb.dir / "bin" / "flock").symlink_to(sb.tools["flock"])

    add("lock-real-free", lock_real)

    def d10(b, r):
        bad = []
        if b["rc"] != 1 or "flock failed to acquire the run lock" not in b["stderr"]:
            bad.append("bash: expected a lock error, got rc=%s %r" % (b["rc"], b["stderr"][:160]))
        if r["rc"] != 1 or "could not open the run lock" not in r["stderr"] or "held the lock" in r["stderr"]:
            bad.append("rust: expected its own refusal, got rc=%s %r" % (r["rc"], r["stderr"][:160]))
        return bad

    def lock_dir(sb):
        sb.rm("bin/flock")
        (sb.dir / "bin" / "flock").symlink_to(sb.tools["flock"])
        (sb.state / "run.lock").mkdir(parents=True, exist_ok=True)

    add("d10-lock-is-a-directory", lock_dir, explain=d10)

    # --- the identity -------------------------------------------------------------------------------------------
    add("id-ok")
    add("id-case-differs", lambda sb: sb.ans("gh-api-user", "FXAcct", 0))
    add("id-profile-case", lambda sb: sb.profile_add('GH_ACCOUNT="FxAcct"'))
    add("id-wrong", lambda sb: sb.ans("gh-api-user", "someone-else", 0))
    add("id-wrong-multiline", lambda sb: sb.ans("gh-api-user", "a\nb", 0))
    add("id-empty-x3", lambda sb: sb.ans("gh-api-user", "", 0))
    add("id-fail-x3", lambda sb: (sb.ans("gh-api-user", "", 1, err="error connecting to api.github.com\n")))
    add("id-tries-1", lambda sb: (sb.ans("gh-api-user", "", 1), sb.env.update({"MAINTAINER_GH_TRIES": "1"})))
    add("id-tries-5", lambda sb: (sb.ans("gh-api-user", "", 1), sb.env.update({"MAINTAINER_GH_TRIES": "5"})))
    add("id-backoff-7", lambda sb: (sb.ans("gh-api-user", "", 1), sb.env.update({"MAINTAINER_GH_BACKOFF": "7"})))
    add("id-backoff-default", lambda sb: (sb.ans("gh-api-user", "", 1), sb.env.pop("MAINTAINER_GH_BACKOFF")))
    add("id-retry-then-ok", lambda sb: (sb.ans("gh-api-user.1", "", 1), sb.ans("gh-api-user.2", "fxacct", 0), sb.ans("gh-api-user.3", "fxacct", 0)))
    add("id-401", lambda sb: sb.ans("gh-api-user", '{"message": "Bad credentials"}', 1, err="gh: Bad credentials (HTTP 401)\n"))
    add("id-401-on-retry", lambda sb: (sb.ans("gh-api-user.1", "", 1), sb.ans("gh-api-user.2", "body", 1, err="HTTP 401\n")))
    add("id-switch-fails", lambda sb: sb.ans("gh-auth-switch", "", 1, err="no such account\n"))
    add("id-switch-prints", lambda sb: sb.ans("gh-auth-switch", "switched", 0, err="note\n"))
    add("id-token-empty", lambda sb: sb.ans("gh-auth-token", "", 0))
    add("id-token-fails", lambda sb: sb.ans("gh-auth-token", "", 1, err="no oauth token\n"))
    add("id-pinned-wrong", lambda sb: (sb.ans("gh-api-user.1", "fxacct", 0), sb.ans("gh-api-user.2", "someone-else", 0), sb.leak(sb.state / "leak.txt")))
    add("id-pinned-rc3", lambda sb: (sb.ans("gh-api-user.1", "fxacct", 0), sb.ans("gh-api-user.2", "", 3, err="dial tcp: lookup api.github.com\n")))
    add("id-pinned-empty", lambda sb: (sb.ans("gh-api-user.1", "fxacct", 0), sb.ans("gh-api-user.2", "", 0)))
    add("id-github-token", lambda sb: sb.env.update({"GITHUB_TOKEN": "other-token"}))
    add("id-preview-no-gh", args=("--show-prompt", "fx", "review"))

    def d13(b, r):
        bad = []
        bg = [c for c in calls_of(b, "gh")]
        rg = [c for c in calls_of(r, "gh")]
        if not bg or "GH_TOKEN=other" not in bg[0]:
            bad.append("bash: the first gh call should see the inherited GH_TOKEN: %r" % (bg[:1],))
        if not rg or "GH_TOKEN=unset" not in rg[0]:
            bad.append("rust: the first gh call must not see the inherited GH_TOKEN: %r" % (rg[:1],))
        # Every call made before the pin saw the inherited token in the bash and none in the Rust
        # build, so its token field and its environment-names digest differ. Every other field of
        # the call log is the same.
        bl, rl = b["calls"].splitlines(), r["calls"].splitlines()
        if len(bl) != len(rl):
            bad.append("the call logs have different lengths: %d and %d" % (len(bl), len(rl)))
        else:
            for number, (x, y) in enumerate(zip(bl, rl), 1):
                fx, fy = x.split("|"), y.split("|")
                if "GH_TOKEN=other" in fx:
                    fx = [f for f in fx if not f.startswith(("GH_TOKEN=", "names="))]
                    fy = [f for f in fy if not f.startswith(("GH_TOKEN=", "names="))]
                    if "GH_TOKEN=unset" not in y.split("|"):
                        bad.append("call %d: the Rust build passed the inherited token" % number)
                if fx != fy:
                    bad.append("call %d differs beyond the token: %s" % (number, short_diff(x, y)))
                    break
        if b["rc"] != r["rc"] or b["stdout"] != r["stdout"] or b["stderr"] != r["stderr"]:
            bad.append("exit code or output differs")
        return bad

    add("d13-inherited-gh-token", lambda sb: sb.env.update({"GH_TOKEN": "caller-token-value"}), explain=d13)
    add("d13-inherited-gh-token-and-github-token", lambda sb: sb.env.update({"GH_TOKEN": "caller", "GITHUB_TOKEN": "caller2"}), explain=d13)

    def d15(b, r):
        bad = []
        if b["rc"] != 0:
            bad.append("bash: the run goes ahead with a short token, got rc=%s" % b["rc"])
        if r["rc"] != 1 or "shorter than 16 characters" not in r["stderr"] or any(c.startswith("maintainer|start ") for c in r["calls"].splitlines()):
            bad.append("rust: expected a refusal before start, got rc=%s %r" % (r["rc"], r["stderr"][:160]))
        if calls_of(r, "claude"):
            bad.append("rust: the agent ran")
        if "shorttoken" in r["stderr"] + r["stdout"] + r["calls"]:
            bad.append("rust: the short token was printed")
        return bad

    def short_token(sb, tok="shorttoken"):
        (sb.dir / "token").write_text(tok + "\n")
        sb.ans("gh-auth-token", tok, 0)

    for tok in ("shorttoken", "123456789012345", "a"):
        add("d15-token-%d-chars" % len(tok), lambda sb, t=tok: short_token(sb, t), explain=d15)
    add("token-16-chars", lambda sb: short_token(sb, "1234567890123456"))

    # --- the checkout and the refresh -------------------------------------------------------------------------------
    add("checkout-missing-no-origin", lambda sb: sb.rm("home/.local/state/fx-maint/checkout"))
    add("checkout-missing-clone-fails", lambda sb: (sb.rm("home/.local/state/fx-maint/checkout"), sb.env.update({"MAINTAINER_REPO_ORIGIN": "/nonexistent/origin.git"})))

    def clone_ok(sb):
        sb.rm("home/.local/state/fx-maint/checkout")
        origin = sb.dir / "origin"
        subprocess.run(["git", "init", "-q", "-b", "main", str(origin)], check=True, env={"PATH": "/usr/bin:/bin", "HOME": str(sb.dir)})
        sb.env["MAINTAINER_REPO_ORIGIN"] = str(origin)

    add("checkout-clone-ok", clone_ok)
    add("start-fails", lambda sb: sb.ans("h-start", "refresh exploded", 1, err="and a stderr line\n"))
    add("start-no-id", lambda sb: sb.ans("h-start", "no header here", 0))
    add("start-capital-word", lambda sb: sb.ans("h-start", "=== Maintainer id ===", 0))
    add("start-first-header-wins", lambda sb: sb.ans("h-start", "=== note extra ===\n=== maintainer %s ===" % RUNID, 0))
    add("start-id-differs", lambda sb: sb.ans("h-start", "=== maintainer 2026-02-02T02-02-review ===\nskill        x", 0))
    add("start-id-dot-2", lambda sb: sb.ans("h-start", "=== maintainer %s.2 ===" % RUNID, 0))
    add("start-id-exists", lambda sb: (sb.ans("h-start", "=== maintainer 2026-02-02T02-02-review ===", 0),
                                       (sb.state / "logs").mkdir(parents=True, exist_ok=True),
                                       (sb.state / "logs" / "2026-02-02T02-02-review.log").write_text("an earlier run wrote this\n")))
    add("start-trailing-blank-lines", lambda sb: sb.ans("h-start", "=== maintainer %s ===\nlast line\n\n\n" % RUNID, 0))
    add("start-stderr-merged", lambda sb: sb.ans("h-start", "=== maintainer %s ===\nskill        x" % RUNID, 0, err="a warning from start\n"))
    add("lease-write-fails", lambda sb: sb.ans("h-run-lease", "", 1, err="lease broke\n"))
    add("failed-marker-fails", lambda sb: (sb.ans("gh-api-user", "someone-else", 0), sb.ans("h-failed", "marker stdout", 1, err="marker stderr\n")))

    # --- the agent and finish ------------------------------------------------------------------------------------------
    def agent_says(text, rc=1):
        return lambda sb: (sb.ans("agent", text, rc),)

    add("agent-ok")
    add("agent-fails-unknown", agent_says("something unknown broke"))
    add("agent-fails-silent", agent_says("", 1))
    add("agent-rc-127", agent_says("not found", 127))
    add("agent-transient-session", agent_says("You've hit your session limit, resets 10:10pm"))
    add("agent-transient-overloaded", agent_says("API Error: 529 overloaded_error"))
    add("agent-human-credit", agent_says("Your credit balance is too low to access the API"))
    add("agent-human-oauth", agent_says("OAuth session expired. Please log in"))
    add("agent-human-auth", agent_says("Failed to authenticate. API Error: 401"))
    add("agent-human-key", agent_says("Invalid API key. Fix external API key"))
    add("agent-big-output", lambda sb: sb.ans("agent", "x" * 70000, 0))
    add("agent-prints-token", lambda sb: (sb.behave('printf "agent printed %s\\n" "$GH_TOKEN" >> "$DC_CASE/home/.local/state/fx-maint/note.txt"\n')))
    # The agent prints nothing in the signal scenarios: the Rust build stops the agent's process
    # group when the signal arrives (decision P2A-sig), so what it would print afterwards is not
    # a fact to compare.
    add("agent-sigterm-parent", lambda sb: (sb.ans("agent", "", 0), sb.behave('kill -TERM "$PPID"\n')), explain=explain_signal)
    add("agent-sighup-parent", lambda sb: (sb.ans("agent", "", 0), sb.behave('kill -HUP "$PPID"\n')), explain=explain_signal)

    def psig_int(b, r):
        bad = []
        if b["rc"] != 0:
            bad.append("bash: a SIGINT that the child survives is ignored by a waiting shell, so the run finishes: rc=%s" % b["rc"])
        # The harness reads a death by SIGINT as -2 (timeout passes the signal on to its parent).
        if r["rc"] not in (130, -2) or any(c.startswith("maintainer|finish ") for c in r["calls"].splitlines()):
            bad.append("rust: expected exit 130 and no finish, got rc=%s" % r["rc"])
        if not any("run-lease --clear" in c for c in calls_of(r, "maintainer")):
            bad.append("rust: the lease was not cleared")
        return bad

    add("psig-agent-sigint-parent", lambda sb: (sb.ans("agent", "", 0), sb.behave('kill -INT "$PPID"\n')), explain=psig_int)
    add("finish-fails-empty", lambda sb: sb.ans("h-finish", "error: report is empty", 1))
    add("finish-fails-silent", lambda sb: sb.ans("h-finish", "", 1))
    add("finish-fixture", lambda sb: sb.ans("h-finish", "this is a fixture rather than a report", 1))
    add("finish-stderr", lambda sb: sb.ans("h-finish", "finish stdout line", 0, err="finish stderr line\n"))
    add("finish-fails-after-agent-fails", lambda sb: (sb.ans("agent", "something unknown broke", 1), sb.ans("h-finish", "", 1)))
    add("finish-fails-after-transient", lambda sb: (sb.ans("agent", "You've hit your session limit, resets 10:10pm", 1), sb.ans("h-finish", "", 1)))
    add("finish-fails-after-human", lambda sb: (sb.ans("agent", "Invalid API key. Fix external API key", 1), sb.ans("h-finish", "", 1)))
    add("digest-multiline", lambda sb: sb.ans("h-digest", "line one\nline two", 0))
    add("digest-empty", lambda sb: sb.ans("h-digest", "", 0))
    add("digest-fails", lambda sb: sb.ans("h-digest", "half a digest", 1))
    add("abort-fails", lambda sb: (sb.ans("h-finish", "", 1), sb.ans("h-abort", "", 1)))

    # --- the toast, the display, the alert hook ---------------------------------------------------------------------------
    add("notify-off", lambda sb: sb.env.update({"MAINTAINER_NOTIFY": "off"}))
    add("notify-display-env", lambda sb: sb.env.update({"DISPLAY": ":7"}))
    add("notify-display-session", lambda sb: (sb.env.update({"DISPLAY": ":7"}), sb.ans("loginctl-show-session", ":9", 0), sb.ans("loginctl-show-user", "3", 0)))
    add("notify-no-user", lambda sb: sb.env.pop("USER"))
    add("notify-empty-user", lambda sb: sb.env.update({"USER": ""}))
    add("notify-no-loginctl", lambda sb: sb.rm("bin/loginctl"))
    add("notify-no-notify-send", lambda sb: sb.rm("bin/notify-send"))
    add("notify-send-fails", lambda sb: sb.ans("notify", "toast stdout", 1, err="toast stderr\n"))
    add("alert-cmd-ok", lambda sb: (sb.ans("gh-api-user", "someone-else", 0), sb.env.update({"MAINTAINER_ALERT_CMD": 'printf "%s|%s|%s|%s\\n" "$MAINTAINER_ALERT_TASK" "$MAINTAINER_ALERT_LOG" "$0" "$1" > "$DC_CASE/alert.out"; echo hook-output'})))
    add("alert-cmd-fails", lambda sb: (sb.ans("gh-api-user", "someone-else", 0), sb.env.update({"MAINTAINER_ALERT_CMD": "exit 3"})))
    add("alert-cmd-on-transient", lambda sb: (sb.ans("flock", "", 1), sb.env.update({"MAINTAINER_ALERT_CMD": 'echo ran > "$DC_CASE/alert.out"'})))
    add("alert-cmd-preview", lambda sb: (sb.rm("tree/profiles/fx/prompts/review.md"), sb.env.update({"MAINTAINER_ALERT_CMD": 'echo ran > "$DC_CASE/alert.out"'})), args=("--show-prompt", "fx", "review"))

    # --- the scrub ---------------------------------------------------------------------------------------------------------------
    def plant(sb):
        sb.leak(sb.state / "leak.txt")
        sess = sb.dir / "home" / ".claude" / "projects" / str(sb.state / "checkout").replace("/", "-").replace(".", "-")
        sb.leak(sess / "session.jsonl")
        sb.leak(sb.dir / "home" / "elsewhere" / "leak.txt")
        sb.leak(sb.dir / "home" / ".claude" / "projects" / "-some-other-project" / "leak.jsonl")
        for d in (".git", "node_modules", "target", "ctargets"):
            sb.leak(sb.state / d / "inside.txt")
        (sb.state / "disk.img").write_bytes(b"bin\0" + TOKEN.encode() + b"\0")

    add("scrub-everything", plant)
    add("scrub-backend-fails", lambda sb: (plant(sb), sb.ans("agent", "boom", 1)))
    add("scrub-finish-fails", lambda sb: (plant(sb), sb.ans("h-finish", "", 1)))
    add("scrub-not-before-pin", lambda sb: (plant(sb), sb.ans("gh-api-user", "someone-else", 0)))
    add("scrub-pinned-wrong", lambda sb: (plant(sb), sb.ans("gh-api-user.1", "fxacct", 0), sb.ans("gh-api-user.2", "other", 0)))
    add("scrub-pinned-rc3", lambda sb: (plant(sb), sb.ans("gh-api-user.1", "fxacct", 0), sb.ans("gh-api-user.2", "", 3)))
    add("scrub-signal-term", lambda sb: (plant(sb), sb.ans("agent", "", 0), sb.behave('kill -TERM "$PPID"\n')), explain=explain_signal)
    add("scrub-signal-hup", lambda sb: (plant(sb), sb.ans("agent", "", 0), sb.behave('kill -HUP "$PPID"\n')), explain=explain_signal)
    add("scrub-session-name", lambda sb: (sb.env.update({"MAINTAINER_REPO_PATH": str(sb.dir / "home" / "work.dir" / "repo")}),
                                          (sb.dir / "home" / "work.dir" / "repo" / ".git").mkdir(parents=True),
                                          sb.leak(sb.dir / "home" / ".claude" / "projects" / str(sb.dir / "home" / "work.dir" / "repo").replace("/", "-").replace(".", "-") / "s.jsonl"),
                                          sb.leak(sb.dir / "home" / ".claude" / "projects" / str(sb.dir / "home" / "work.dir" / "repo").replace("/", "-") / "s.jsonl")))

    # --- the prompt -------------------------------------------------------------------------------------------------------------------
    names = {"amp": "A&B", "pipe": "A|B", "backslash": "A\\B", "mix": "A&|\\B", "dollar": "A$HOME`x`B", "quote": "A'B\"C",
             "newline": "A\nB", "empty": "", "space": "Mr  Space", "utf8": "Zoë"}
    for label, name in names.items():
        for mode in ("preview", "real"):
            args = ("--show-prompt", "fx", "review") if mode == "preview" else ("fx", "review")
            add("name-%s-%s" % (label, mode), lambda sb, n=name: sb.profile_add("MAINTAINER_NAME=%s" % shlex.quote(n)), args=args)
    add("name-unset", lambda sb: sb.profile_add("unset MAINTAINER_NAME"), args=("--show-prompt", "fx", "review"))
    for label, slug in {"amp": "fx/a&b", "pipe": "fx/a|b", "backslash": "fx\\repo", "newline": "fx/a\nb", "plain": "other/slug"}.items():
        add("slug-%s" % label, lambda sb, s=slug: sb.profile_add("REPO_SLUG=%s" % shlex.quote(s)), args=("--show-prompt", "fx", "review"))
    for style in ("raw", "stop-slop", "other", ""):
        add("prose-%s" % (style or "empty"), lambda sb, s=style: sb.profile_add("PROSE_STYLE=%s" % shlex.quote(s)), args=("--show-prompt", "fx", "review"))
    add("prose-missing-file", lambda sb: sb.rm("tree/lib/prose-style.md"), args=("--show-prompt", "fx", "review"))
    add("prose-empty-file", lambda sb: (sb.dir / "tree/lib/prose-style.md").write_text(""), args=("--show-prompt", "fx", "review"))
    for post in ("on", "off", "maybe", "OFF", ""):
        for mode in ("preview", "real"):
            args = ("--show-prompt", "fx", "review") if mode == "preview" else ("fx", "review")
            add("post-%s-%s" % (post or "empty", mode), lambda sb, p=post: (sb.wall(), sb.env.update({"MAINTAINER_POST": p})), args=args)
            add("post-%s-%s-nowall" % (post or "empty", mode), lambda sb, p=post: sb.env.update({"MAINTAINER_POST": p}), args=args)
    add("prompt-issues", lambda sb: sb.ans("h-start", "=== maintainer %s-issues ===\nskill        x" % STAMP, 0), args=("fx", "issues"))
    add("prompt-issues-preview", args=("--show-prompt", "fx", "issues"))
    for rel in ("tree/profiles/fx/prompts/review.md", "tree/profiles/fx/prompts/common-preamble.md", "tree/lib/preamble-core.md"):
        for mode in ("preview", "real"):
            args = ("--show-prompt", "fx", "review") if mode == "preview" else ("fx", "review")
            add("missing-%s-%s" % (rel.split("/")[-1], mode), lambda sb, r=rel: sb.rm(r), args=args)
    add("unreadable-task-file-is-a-directory", lambda sb: (sb.rm("tree/profiles/fx/prompts/review.md"), (sb.dir / "tree/profiles/fx/prompts/review.md").mkdir()), args=("--show-prompt", "fx", "review"))
    add("installed-layout-preview", args=("--show-prompt", "fx", "review"))
    add("version-missing", lambda sb: sb.rm("tree/VERSION"), args=("--show-prompt", "fx", "review"))
    add("version-no-commit", lambda sb: (sb.dir / "tree/VERSION").write_text("version=1.2.3\n"), args=("--show-prompt", "fx", "review"))
    add("version-two-lines", lambda sb: (sb.dir / "tree/VERSION").write_text("version=1\nversion=2\ncommit=a\n"), args=("--show-prompt", "fx", "review"))
    add("version-empty-value", lambda sb: (sb.dir / "tree/VERSION").write_text("version=\ncommit=\n"), args=("--show-prompt", "fx", "review"))

    # --- the backends -----------------------------------------------------------------------------------------------------------------------
    for backend in ("claude", "codex", "opencode", "cursor"):
        for post in ("on", "off"):
            add("backend-%s-%s" % (backend, post),
                lambda sb, b=backend, p=post: (sb.wall(), sb.env.update({"MAINTAINER_BACKEND": b, "MAINTAINER_POST": p, "GROQ_API_KEY": "gsk_fixture"})))
            add("backend-%s-%s-preview" % (backend, post),
                lambda sb, b=backend, p=post: (sb.wall(), sb.env.update({"MAINTAINER_BACKEND": b, "MAINTAINER_POST": p})), args=("--show-prompt", "fx", "review"))
        add("backend-%s-not-on-path" % backend,
            lambda sb, b=backend: (sb.env.update({"MAINTAINER_BACKEND": b, "GROQ_API_KEY": "gsk_fixture"}), sb.rm("bin/claude"), sb.rm("bin/codex"), sb.rm("bin/opencode"), sb.rm("bin/cursor-agent")))
        add("backend-%s-readonly-review" % backend,
            lambda sb, b=backend: (sb.wall(), sb.profile_add('TASKS="review readonly-review"'), sb.env.update({"MAINTAINER_BACKEND": b})), args=("fx", "readonly-review"))
    add("backend-claude-no-settings", lambda sb: sb.rm("tree/profiles/fx/settings.json"))
    add("backend-claude-no-transcript", lambda sb: sb.rm("tree/scripts/transcript.py"))
    add("backend-claude-thinking-from-env", lambda sb: (sb.profile_add("unset THINKING_review"), sb.env.update({"MAINTAINER_THINKING": "1234"})))
    add("backend-claude-no-subagent", lambda sb: sb.profile_add("unset SUBAGENT_MODEL_review"))
    add("backend-opencode-no-key", lambda sb: sb.env.update({"MAINTAINER_BACKEND": "opencode"}))
    add("backend-opencode-huge-prompt", lambda sb: (sb.env.update({"MAINTAINER_BACKEND": "opencode", "GROQ_API_KEY": "gsk_fixture"}), (sb.dir / "tree/profiles/fx/prompts/review.md").write_text("x" * 140000)))
    add("backend-unknown", lambda sb: sb.env.update({"MAINTAINER_BACKEND": "nosuch"}))
    add("backend-unknown-preview", lambda sb: sb.env.update({"MAINTAINER_BACKEND": "nosuch"}), args=("--show-prompt", "fx", "review"))

    def d18(b, r):
        bad = []
        if b["rc"] != 0:
            bad.append("bash: a relative BACKEND resolves, got rc=%s %r" % (b["rc"], b["stderr"][:120]))
        if r["rc"] != 64 or "no backend '" not in r["stderr"] or "No such file or directory" in r["stderr"]:
            bad.append("rust: expected the refusal with exit 64, got rc=%s %r" % (r["rc"], r["stderr"][:160]))
        if calls_of(r, "codex"):
            bad.append("rust: the backend ran")
        return bad

    add("d18-backend-relative-path", lambda sb: sb.env.update({"MAINTAINER_BACKEND": "../backends/codex"}), explain=d18)
    add("d18-backend-slash", lambda sb: sb.env.update({"MAINTAINER_BACKEND": "./codex"}), explain=d18)
    return S


DIMENSIONS = {
    "args": [("fx", "review"), ("fx", "review"), ("fx", "issues"), ("--show-prompt", "fx", "review"), ("--show-prompt", "fx", "issues")],
    "age": [None, None, 0, 60, 3600, 5 * 3600, 6 * 3600 - 1, 6 * 3600, 7 * 3600, 36 * 3600, 10 ** 8],
    "force": [None, None, None, "1", "0", "true"],
    "lock": [None, None, None, None, ("rc", 1), ("rc", 2), ("rc", 64), ("promote", 0)],
    "identity": [None, None, None, None, "wrong", "empty", "fail", "401", "retry", "case", "pinwrong", "pinrc3", "tries1", "switchfail", "tokenempty", "tokenfail"],
    "checkout": [None, None, None, None, "missing"],
    "start": [None, None, None, None, "fail", "noid", "other-id", "dot2", "exists"],
    "agent": [None, None, None, "unknown", "transient", "credit", "oauth", "key", "silent"],
    "finish": [None, None, None, None, "fail", "fixture"],
    "digest": [None, None, "multiline", "empty", "fail"],
    "post": [None, None, "off", "maybe"],
    "prose": [None, None, "raw", "other"],
    "name": [None, None, None, "A&B", "A|B", "A\\B", "A\nB", ""],
    "backend": [None, None, None, "codex", "opencode", "cursor", "nosuch"],
    "notify": [None, None, None, "off", "display", "nouser"],
    "hook": [None, None, None, "ok", "fail"],
    "leak": [None, None, "plant"],
    "wall": [True, True, False],
    "files": [None, None, None, None, "no-task", "no-common", "no-core", "no-prose", "no-settings"],
    "lease": [None, None, None, "fail"],
    "alert-prints": [None, None, "marker-fails"],
    "nested": [None, None, None, None, None, "x/y"],
}


def random_scenario(rng, index):
    pick = {k: rng.choice(v) for k, v in DIMENSIONS.items()}

    def build(sb):
        if pick["wall"]:
            sb.wall()
        if pick["age"] is not None:
            sb.last_ran("issues" if "issues" in pick["args"] else "review", pick["age"])
        if pick["force"] is not None:
            sb.env["MAINTAINER_FORCE"] = pick["force"]
        lock = pick["lock"]
        if lock:
            if lock[0] == "rc":
                sb.ans("flock", "", lock[1])
            else:
                (sb.dir / "flock.touch").write_text(str(sb.state / "state" / "last-review.json"))
        ident = pick["identity"]
        if ident == "wrong":
            sb.ans("gh-api-user", "someone-else", 0)
        elif ident == "empty":
            sb.ans("gh-api-user", "", 0)
        elif ident == "fail":
            sb.ans("gh-api-user", "", 1, err="boom\n")
        elif ident == "401":
            sb.ans("gh-api-user", "{}", 1, err="HTTP 401\n")
        elif ident == "retry":
            sb.ans("gh-api-user.1", "", 1)
            sb.ans("gh-api-user.2", "fxacct", 0)
            sb.ans("gh-api-user.3", "fxacct", 0)
        elif ident == "case":
            sb.ans("gh-api-user", "FXACCT", 0)
        elif ident == "pinwrong":
            sb.ans("gh-api-user.1", "fxacct", 0)
            sb.ans("gh-api-user.2", "other", 0)
        elif ident == "pinrc3":
            sb.ans("gh-api-user.1", "fxacct", 0)
            sb.ans("gh-api-user.2", "", 3)
        elif ident == "tries1":
            sb.ans("gh-api-user", "", 1)
            sb.env["MAINTAINER_GH_TRIES"] = "1"
        elif ident == "switchfail":
            sb.ans("gh-auth-switch", "", 1, err="no such account\n")
        elif ident == "tokenempty":
            sb.ans("gh-auth-token", "", 0)
        elif ident == "tokenfail":
            sb.ans("gh-auth-token", "", 1, err="no token\n")
        if pick["checkout"] == "missing":
            sb.rm("home/.local/state/fx-maint/checkout")
        start = pick["start"]
        if start == "fail":
            sb.ans("h-start", "refresh exploded", 1, err="stderr line\n")
        elif start == "noid":
            sb.ans("h-start", "no header", 0)
        elif start == "other-id":
            sb.ans("h-start", "=== maintainer 2026-03-03T03-03-review ===", 0)
        elif start == "dot2":
            sb.ans("h-start", "=== maintainer %s.2 ===" % RUNID, 0)
        elif start == "exists":
            sb.ans("h-start", "=== maintainer 2026-03-03T03-03-review ===", 0)
            (sb.state / "logs").mkdir(parents=True, exist_ok=True)
            (sb.state / "logs" / "2026-03-03T03-03-review.log").write_text("earlier\n")
        agent = pick["agent"]
        texts = {"unknown": "something unknown broke", "transient": "You've hit your session limit, resets 10:10pm",
                 "credit": "Your credit balance is too low to access the API", "oauth": "OAuth session expired. Please log in",
                 "key": "Invalid API key. Fix external API key", "silent": ""}
        if agent:
            sb.ans("agent", texts[agent], 1)
        if pick["finish"] == "fail":
            sb.ans("h-finish", "error: report is empty", 1)
        elif pick["finish"] == "fixture":
            sb.ans("h-finish", "this is a fixture rather than a report", 1)
        digest = pick["digest"]
        if digest == "multiline":
            sb.ans("h-digest", "one\ntwo", 0)
        elif digest == "empty":
            sb.ans("h-digest", "", 0)
        elif digest == "fail":
            sb.ans("h-digest", "half", 1)
        if pick["post"] is not None:
            sb.env["MAINTAINER_POST"] = pick["post"]
        if pick["prose"] is not None:
            sb.env["MAINTAINER_PROSE_STYLE"] = pick["prose"]
        if pick["name"] is not None:
            sb.profile_add("MAINTAINER_NAME=%s" % shlex.quote(pick["name"]))
        if pick["backend"] is not None:
            sb.env["MAINTAINER_BACKEND"] = pick["backend"]
            sb.env["GROQ_API_KEY"] = "gsk_fixture"
        if pick["notify"] == "off":
            sb.env["MAINTAINER_NOTIFY"] = "off"
        elif pick["notify"] == "display":
            sb.env["DISPLAY"] = ":7"
        elif pick["notify"] == "nouser":
            sb.env.pop("USER")
        if pick["hook"] == "ok":
            sb.env["MAINTAINER_ALERT_CMD"] = 'echo hook >> "$DC_CASE/alert.out"'
        elif pick["hook"] == "fail":
            sb.env["MAINTAINER_ALERT_CMD"] = "exit 3"
        if pick["leak"]:
            sb.leak(sb.state / "leak.txt")
            sb.leak(sb.dir / "home" / "elsewhere" / "leak.txt")
        files = pick["files"]
        if files == "no-task":
            sb.rm("tree/profiles/fx/prompts/review.md")
            sb.rm("tree/profiles/fx/prompts/issues.md")
        elif files == "no-common":
            sb.rm("tree/profiles/fx/prompts/common-preamble.md")
        elif files == "no-core":
            sb.rm("tree/lib/preamble-core.md")
        elif files == "no-prose":
            sb.rm("tree/lib/prose-style.md")
        elif files == "no-settings":
            sb.rm("tree/profiles/fx/settings.json")
        if pick["lease"]:
            sb.ans("h-run-lease", "", 1)
        if pick["alert-prints"]:
            sb.ans("h-failed", "m", 1, err="m\n")
        if pick["nested"]:
            sb.env["MAINTAINER_IN_RUN"] = pick["nested"]

    sc = Scenario("rand-%03d" % index, build, pick["args"])
    sc.pick = pick
    return sc


def main():
    repo, rust_tree, work, seed = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
    only = [x for x in os.environ.get("DIFF_ONLY", "").split(",") if x]
    verbose = os.environ.get("DIFF_VERBOSE") == "1"
    rand_count = int(os.environ.get("DIFF_RANDOM", "260"))
    real = {}
    for name in TOOLS + ["date", "flock"]:
        found = shutil.which(name, path="/usr/bin:/bin:/usr/sbin:/sbin")
        if not found:
            sys.exit("differential: the host has no '%s'" % name)
        real[name] = found
    sb = Sandbox(work, repo, rust_tree, real)
    scenarios = named_scenarios()
    rng = random.Random(seed)
    scenarios += [random_scenario(rng, i) for i in range(rand_count)]
    names = [s.name for s in scenarios]
    if len(set(names)) != len(names):
        sys.exit("differential: duplicate scenario names")
    if only:
        scenarios = [s for s in scenarios if any(o in s.name for o in only)]
        if not scenarios:
            sys.exit("differential: DIFF_ONLY matched no scenario")
    identical = explained = 0
    bad = []
    started = time.time()
    for sc in scenarios:
        b, r = run_both(sb, sc)
        problems = diff_snapshots(b, r)
        if sc.explain is not None:
            problems = sc.explain(b, r)
            if not problems:
                explained += 1
        elif not problems:
            identical += 1
        if problems:
            bad.append((sc.name, problems, b, r))
        if verbose:
            print("%-44s %s" % (sc.name, "differs" if problems else ("explained" if sc.explain else "identical")), flush=True)
    total = len(scenarios)
    print("differential: %d scenarios compared in %.0f s: %d identical, %d explained by a plan decision, %d unexplained"
          % (total, time.time() - started, identical, explained, len(bad)))
    # Each scenario compares six fields and the state tree, so the comparison count is larger.
    print("differential: fields compared per scenario: exit code, stdout, stderr, call log, TMPDIR, state tree")
    for name, problems, b, r in bad[:25]:
        print("UNEXPLAINED %s" % name)
        for p in problems[:6]:
            print("  " + p.replace("\n", "\n  "))
        print("  repeat: DIFF_SEED=%d DIFF_ONLY=%s" % (seed, name))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
