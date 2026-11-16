#!/usr/bin/env python3
"""The scenarios and the runner behind differential.sh. Not meant to run alone.

Every scenario builds one fixture (real git repositories, a bare origin, stubs
for gh, a logging git, a fake `maintainer fixup-check`), snapshots it, runs the
bash program on a restored copy, restores the snapshot, runs the Rust program,
and compares what each produced:

  stdout, stderr and the exit code,
  the call log: one global file, one line per call to gh, git or the fake
    fixup-check, with argv, the environment names that matter and stdin,
  the state tree: every file of the case (refs, config, working tree, state
    directory) except what git rewrites on its own (objects, index, logs).

Both programs run as `bash <tree>/bin/maintainer-repo ARGS`, under `env -i`,
from the same case directory, with the same PATH, locale and dates, so the
fixture needs no path rewriting; only the two tree paths, and the random
suffix mktemp picks, are normalised.

Scenarios are generated from a seed. The name of a scenario and the seed fix
its content, so a failing run repeats with DIFF_SEED and DIFF_ONLY=<name>.
"""

import hashlib
import os
import random
import re
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path

TOOLS = ("bash sh env python3 git sed grep awk sort uniq cut tr head tail wc cat ls find dirname basename readlink "
         "stat date mktemp rm mkdir rmdir touch cp mv chmod sha256sum paste sleep timeout true false tee tar xargs ln").split()
ENV_NAMES = ("HOME LANG LC_ALL TZ GH_TOKEN GITHUB_TOKEN GH_HOST GIT_TERMINAL_PROMPT GIT_EDITOR GIT_AUTHOR_NAME "
             "GIT_AUTHOR_EMAIL GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL GIT_AUTHOR_DATE GIT_COMMITTER_DATE "
             "GIT_CONFIG_COUNT GIT_CONFIG_KEY_0 GIT_CONFIG_VALUE_0 GIT_CONFIG_KEY_1 GIT_CONFIG_VALUE_1 "
             "GIT_CONFIG_NOSYSTEM GIT_CONFIG_GLOBAL GIT_TEMPLATE_DIR").split()
SECRETS = ("GH_TOKEN", "GITHUB_TOKEN")
SKIP_TOP = {"log", "ans", "tmp", "snapshot"}
SKIP_GIT = ("objects", "logs", "index", "FETCH_HEAD", "ORIG_HEAD", "COMMIT_EDITMSG")


class Fx:
    """One fixture directory and the means to build it."""

    def __init__(self, work, realgit):
        self.work = Path(work)
        self.dir = self.work / "case"
        self.tools = self.work / "tools"
        self.stubs = self.work / "stubs"
        self.realgit = realgit
        self.tick = 0
        self.env = {}
        self.reset()

    # -- the case directory ---------------------------------------------------
    def reset(self):
        if self.dir.exists():
            subprocess.run(["chmod", "-R", "u+rwX", str(self.dir)], check=False)
            shutil.rmtree(self.dir)
        for sub in ("home", "state", "log", "ans", "tmp", "cwd"):
            (self.dir / sub).mkdir(parents=True)
        (self.dir / "log" / "calls.log").write_text("")
        self.tick = 0
        self.repo = self.dir / "repo"
        self.env = {
            "PATH": "%s:%s" % (self.stubs, self.tools),
            "HOME": str(self.dir / "home"),
            "LC_ALL": "C",
            "TZ": "UTC",
            "LOGNAME": "fixture-user",
            "USER": "fixture-user",
            "TMPDIR": str(self.dir / "tmp"),
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_TERMINAL_PROMPT": "0",
            "MAINTAINER_SLUG": "o/r",
            "MAINTAINER_REPO": str(self.repo),
            "MAINTAINER_ACCOUNT": "testuser",
            "MAINTAINER_STATE": str(self.dir / "state"),
            "MAINTAINER_POST": "on",
            "MAINTAINER_GH_BACKOFF": "0",
        }

    @property
    def state(self):
        return self.dir / "state"

    # -- git, with fixed identity and dates ------------------------------------
    def git(self, *args, repo=None, check=True, bare=None):
        self.tick += 1
        stamp = "%d +0000" % (1767225600 + self.tick)
        env = {
            "PATH": "/usr/bin:/bin", "HOME": str(self.dir / "home"), "LC_ALL": "C",
            "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_AUTHOR_NAME": "fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
            "GIT_COMMITTER_NAME": "fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
            "GIT_AUTHOR_DATE": stamp, "GIT_COMMITTER_DATE": stamp,
        }
        where = ["--git-dir=%s" % bare] if bare else ["-C", str(repo or self.repo)]
        cp = subprocess.run([self.realgit] + where + list(args), env=env, capture_output=True, text=True)
        if check and cp.returncode != 0:
            raise RuntimeError("fixture git %s failed: %s" % (args, cp.stderr))
        return cp.stdout.strip()

    def bare(self, path):
        path = Path(path)
        path.mkdir(parents=True, exist_ok=True)
        subprocess.run([self.realgit, "init", "-q", "--bare", "-b", "main", str(path)], check=True,
                       env={"PATH": "/usr/bin:/bin", "HOME": str(self.dir / "home"), "GIT_CONFIG_NOSYSTEM": "1",
                            "GIT_CONFIG_GLOBAL": "/dev/null"})
        return path

    def init_repo(self, origin=None):
        self.repo.mkdir(parents=True, exist_ok=True)
        self.git("init", "-q", "-b", "main")
        self.git("remote", "add", "origin", str(origin or self.repo))

    def write(self, rel, text, repo=None):
        path = Path(repo or self.repo) / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text + "\n")

    def commit(self, message, repo=None):
        self.git("add", "-A", repo=repo)
        self.git("commit", "-q", "-m", message, repo=repo)
        return self.git("rev-parse", "HEAD", repo=repo)

    # -- answers for the stubs -----------------------------------------------------
    def ans(self, key, out="", rc=0, err=None):
        base = self.dir / "ans" / key
        Path(str(base) + ".out").write_text(out + ("\n" if out and not out.endswith("\n") else ""))
        Path(str(base) + ".rc").write_text(str(rc))
        if err is not None:
            Path(str(base) + ".err").write_text(err)

    def ans_file(self, key, suffix, text):
        Path(str(self.dir / "ans" / key) + suffix).write_text(text)

    def gh(self, key, out="", rc=0, err=None):
        self.ans("gh." + key, out, rc, err)


def stub_files(work, realgit):
    """The static stubs: gh, a logging git, and the fake fixup-check."""
    work = Path(work)
    case = work / "case"
    (work / "stubs").mkdir(parents=True, exist_ok=True)
    (work / "tools").mkdir(parents=True, exist_ok=True)
    for name in TOOLS:
        found = subprocess.run(["bash", "-c", 'PATH=/usr/local/bin:/usr/bin:/bin type -P "$1"', "x", name],
                               capture_output=True, text=True).stdout.strip()
        if not found:
            sys.exit("differential: the host has no %s" % name)
        link = work / "tools" / name
        if not link.exists():
            link.symlink_to(found)
    log_fn = r'''
_log() {
  local name="$1"; shift
  local rec="$name" a v e=""
  for a in "$@"; do rec="$rec $(printf '%q' "$a")"; done
  for v in ''' + " ".join(ENV_NAMES) + r'''; do
    case "$v" in
      GH_TOKEN|GITHUB_TOKEN) if [ -n "${!v:-}" ]; then e="$e $v=<set>"; else e="$e $v=<unset>"; fi ;;
      *) if [ -n "${!v+x}" ]; then e="$e $v=${!v@Q}"; else e="$e $v=<unset>"; fi ;;
    esac
  done
  for v in $(compgen -v MAINTAINER_ | LC_ALL=C sort); do e="$e $v=${!v@Q}"; done
  # Reading a pipe here would eat what the program meant for itself (its `while
  # read` loop), so the log says what stdin IS, never what it holds.
  _in="$(readlink /proc/$$/fd/0 2>/dev/null)"
  case "$_in" in pipe:*) _in=pipe ;; esac
  printf '%s | cwd:%s | env:%s | stdin:%s\n' "$rec" "$(pwd -P)" "$e" "$_in" >> "''' + str(case / "log" / "calls.log") + r'''"
}
'''
    gh = "#!/usr/bin/env bash\n" + log_fn + r'''
CASE="''' + str(case) + r'''"
_log gh "$@"
case "${1:-} ${2:-}" in
    "auth switch") key=auth-switch ;;
    "api user")    key=api-user ;;
    "api users/"*) key=api-author ;;
    "pr list")     key=pr-list ;;
    "pr view")     key=pr-view ;;
    "pr comment")  key=pr-comment
        prev=""
        for a in "$@"; do
            if [ "$prev" = "--body-file" ]; then printf 'body: %q\n' "$(cat "$a"; printf x)" >> "$CASE/log/calls.log"; fi
            prev="$a"
        done ;;
    *) key="" ;;
esac
d="$CASE/ans/gh.$key"
if [ -z "$key" ] || { [ ! -e "$d.out" ] && [ ! -e "$d.rc" ] && [ ! -e "$d.fromgit" ]; }; then
    echo "UNPLANNED gh $*" >> "$CASE/log/calls.log"
    echo "stub gh: unplanned call: $*" >&2
    exit 99
fi
[ -e "$d.run" ] && bash "$d.run" >&2
if [ -e "$d.fromgit" ]; then
    read -r repo ref < "$d.fromgit"
    ''' + realgit + r''' -C "$repo" rev-parse "$ref"
    exit $?
fi
[ -e "$d.out" ] && cat "$d.out"
[ -e "$d.err" ] && cat "$d.err" >&2
rc=0; [ -e "$d.rc" ] && rc="$(cat "$d.rc")"
exit "$rc"
'''
    git = "#!/usr/bin/env bash\n" + log_fn + r'''
_log git "$@"
exec ''' + realgit + r''' "$@"
'''
    for name, text in (("gh", gh), ("git", git)):
        path = work / "stubs" / name
        path.write_text(text)
        path.chmod(0o755)
        subprocess.run(["bash", "-n", str(path)], check=True)
    fake = '''#!/usr/bin/env python3
import os, shlex, sys
d = %r
env = " ".join("%%s=%%s" %% (k, "<set>" if k in ("GH_TOKEN", "GITHUB_TOKEN") else shlex.quote(os.environ[k]))
               for k in sorted(os.environ) if k.startswith("MAINTAINER_") or k in ("HOME", "LC_ALL", "GH_TOKEN", "GITHUB_TOKEN"))
with open(d + "/log/calls.log", "a") as f:
    f.write("maintainer " + shlex.join(sys.argv[1:]) + " | env: " + env + "\\n")
cp = d + "/log/fixup-check.count"
n = (len(open(cp).read()) if os.path.exists(cp) else 0) + 1
open(cp, "a").write("x")
name = "fixup-check.out"
if n > 1 and os.path.exists(d + "/ans/fixup-check.again.out"):
    name = "fixup-check.again.out"
if os.path.exists(d + "/ans/fixup-check.err"):
    sys.stderr.write(open(d + "/ans/fixup-check.err").read())
sys.stdout.write(open(d + "/ans/" + name).read())
sys.exit(int(open(d + "/ans/fixup-check.rc").read()) if os.path.exists(d + "/ans/fixup-check.rc") else 0)
''' % str(case)
    (work / "fake-maintainer").write_text(fake)


def build_trees(work, root, rust_bin):
    """tree-bash and tree-rust: bin/ and lib/ only, the fake fixup-check in both."""
    work = Path(work)
    lib = ["profile.sh", "defaults.env", "failure-shapes.json"]
    for side in ("bash", "rust"):
        tree = work / ("tree-" + side)
        if tree.exists():
            shutil.rmtree(tree)
        (tree / "bin").mkdir(parents=True)
        (tree / "lib").mkdir()
        for name in lib:
            shutil.copy(root / "lib" / name, tree / "lib" / name)
        shutil.copy(work / "fake-maintainer", tree / "bin" / "maintainer")
        (tree / "bin" / "maintainer").chmod(0o755)
        if side == "bash":
            shutil.copy(root / "bin" / "maintainer-repo", tree / "bin" / "maintainer-repo")
        else:
            (tree / ".rust-bin").mkdir()
            shutil.copy(rust_bin, tree / ".rust-bin" / "maintainer-repo")
            wrapper = tree / "bin" / "maintainer-repo"
            wrapper.write_text('#!/usr/bin/env bash\nexec -a "${BASH_SOURCE[0]}" %s "$@"\n'
                               % shlex.quote(str(tree / ".rust-bin" / "maintainer-repo")))
            wrapper.chmod(0o755)
    return work / "tree-bash", work / "tree-rust"


# ---------------------------------------------------------------------------------------------
# Running one side and reading what it left
# ---------------------------------------------------------------------------------------------

def normalise(text, trees, case):
    for tree in trees:
        text = text.replace(str(tree), "<TREE>")
    text = re.sub(r"fixup-notice\.[A-Za-z0-9]{6}", "fixup-notice.X", text)
    text = re.sub(r"fixup-[A-Za-z0-9]{6}\b", "fixup-X", text)
    return text


def snapshot(case):
    """One line per file of the case, except what git rewrites on its own."""
    rows = []
    for base, dirs, files in os.walk(case):
        rel_base = os.path.relpath(base, case)
        parts = Path(rel_base).parts
        if parts and parts[0] in SKIP_TOP:
            dirs[:] = []
            continue
        in_git = any(part == ".git" or part.endswith(".git") for part in parts)
        if in_git:
            dirs[:] = [d for d in dirs if d not in SKIP_GIT]
        dirs.sort()
        for name in sorted(files):
            if name.endswith(".lock") or (in_git and name in SKIP_GIT):
                continue
            path = Path(base) / name
            rel = os.path.relpath(path, case)
            if path.is_symlink():
                rows.append("%s link %s" % (rel, os.readlink(path)))
            else:
                rows.append("%s file %o %s" % (rel, path.lstat().st_mode & 0o777,
                                                hashlib.sha256(path.read_bytes()).hexdigest()))
        for name in dirs:
            rows.append("%s dir" % os.path.join(rel_base, name))
    return "\n".join(sorted(rows)) + "\n"


def variant_tree(work, tree, variant):
    """A copy of `tree` for a scenario that needs a different tree (no lib/profile.sh)."""
    copy = work / ("%s-%s" % (tree.name, variant))
    if copy.exists():
        shutil.rmtree(copy)
    shutil.copytree(tree, copy, symlinks=True)
    if variant == "nolib":
        (copy / "lib" / "profile.sh").unlink()
        # the rust wrapper names its binary by absolute path, which the copy keeps
    return copy


def run_side(fx, tree, args, extra_env, trees, timeout=90, merge=False):
    env = dict(fx.env)
    env.update(extra_env)
    prog = tree / "bin" / "maintainer-repo"
    pairs = ["%s=%s" % (k, v) for k, v in env.items()]
    cp = subprocess.run(["env", "-i"] + pairs + ["timeout", str(timeout), "bash", str(prog)] + list(args),
                        cwd=fx.dir / "cwd", stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                        stderr=subprocess.STDOUT if merge else subprocess.PIPE)
    raw_calls = (fx.dir / "log" / "calls.log").read_text(errors="replace")
    calls = raw_calls
    if args and args[0] == "prune":
        # Decision P1H-2: the bash runs these inside `while read ... done < <(git for-each-ref)`, so
        # their standard input is the pipe that holds the rest of the branch list; a child here gets
        # /dev/null. The pinned scenario pinned-loop-stdin asserts the difference on both sides.
        calls = re.sub(r"^(git (?:merge-base|push|branch) .*\| stdin:)pipe$", r"\1/dev/null", calls, flags=re.M)
    return {
        "raw_calls": normalise(raw_calls, trees, fx.dir),
        "rc": str(cp.returncode),
        "stdout": normalise(cp.stdout.decode(errors="replace"), trees, fx.dir),
        "stderr": normalise((cp.stderr or b"").decode(errors="replace"), trees, fx.dir),
        "calls": normalise(calls, trees, fx.dir),
        "state": normalise(snapshot(fx.dir), trees, fx.dir),
    }


def clip(text, n=600):
    return text if len(text) <= n else text[:n] + "...(%d more bytes)" % (len(text) - n)


# ---------------------------------------------------------------------------------------------
# pinned differences: where the port differs from the bash on purpose, each asserted on BOTH sides
# ---------------------------------------------------------------------------------------------

def farm(fx, name, exclude):
    path = fx.work / name
    if path.exists():
        shutil.rmtree(path)
    path.mkdir()
    for tool in fx.tools.iterdir():
        if tool.name not in exclude:
            (path / tool.name).symlink_to(os.readlink(tool))
    return path


def pinned_nonutf8_branch(fx):
    from differential_scenarios import remote_repo
    remote_repo(fx)
    fx.gh("pr-list", "")
    raw = b"bad\xffname"
    env = {"PATH": "/usr/bin:/bin", "HOME": str(fx.dir / "home"), "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null",
           "GIT_AUTHOR_NAME": "f", "GIT_AUTHOR_EMAIL": "f@e.invalid", "GIT_COMMITTER_NAME": "f", "GIT_COMMITTER_EMAIL": "f@e.invalid"}
    sha = fx.git("rev-parse", "HEAD")
    subprocess.run([fx.realgit, "-C", str(fx.repo), "update-ref", b"refs/heads/" + raw, sha], env=env, check=True)
    subprocess.run([fx.realgit, "-C", str(fx.repo), "push", "-q", "origin", b"refs/heads/" + raw + b":refs/heads/" + raw],
                   env=env, check=True)
    return ["prune", "--dry-run"], {}


def check_nonutf8_branch(bash, rust):
    problems = []
    if "would delete" not in bash["stdout"]:
        problems.append("bash did not list the branch: %r" % bash["stdout"])
    if "a branch name is not UTF-8 text, so nothing was deleted" not in rust["stderr"] or rust["rc"] != "1":
        problems.append("rust did not refuse (P1H-8): rc=%s %r" % (rust["rc"], rust["stderr"]))
    return problems


def pinned_python_missing(fx):
    from differential_scenarios import fixup_scenario
    rng = random.Random("pin-python")
    fixup_scenario(fx, rng)
    fx.env["PATH"] = "%s:%s" % (fx.stubs, farm(fx, "tools-nopython", {"python3"}))
    return ["fixup", "7", "--dry-run"], {}


def check_python_missing(bash, rust):
    problems = []
    if "python3: command not found" not in bash["stderr"] or bash["rc"] != "1":
        problems.append("bash: %s %r" % (bash["rc"], bash["stderr"]))
    if rust["stderr"].strip().endswith("python3: command not found") is False or rust["rc"] != "1" or rust["stdout"] != bash["stdout"]:
        problems.append("rust: %s %r" % (rust["rc"], rust["stderr"]))
    return problems


def pinned_git_missing(fx):
    from differential_scenarios import remote_repo
    remote_repo(fx)
    fx.gh("pr-list", "")
    fx.env["PATH"] = farm(fx, "tools-nogit", {"git"}).as_posix()
    return ["prune", "--dry-run"], {}


def check_git_missing(bash, rust):
    problems = []
    for side, got in (("bash", bash), ("rust", rust)):
        if got["rc"] != "1" or "git: command not found" not in got["stderr"] or "maintainer-repo: fetch failed" not in got["stderr"]:
            problems.append("%s: %s %r" % (side, got["rc"], got["stderr"]))
    return problems


def pinned_loop_stdin(fx):
    from differential_scenarios import add_branch, remote_repo
    remote_repo(fx)
    for name in ("one", "two", "three"):
        add_branch(fx, name, "both-merged")
    fx.gh("pr-list", "")
    return ["prune"], {}


def check_loop_stdin(bash, rust):
    problems = []
    if "stdin:pipe" not in bash["raw_calls"]:
        problems.append("bash children no longer inherit the loop's pipe: P1H-2 is stale")
    if "stdin:pipe" in rust["raw_calls"] or "stdin:/dev/null" not in rust["raw_calls"]:
        problems.append("a rust child has a pipe on stdin")
    for field in ("rc", "stdout", "stderr", "state"):
        if bash[field] != rust[field]:
            problems.append("%s differs" % field)
    return problems


def pinned_unbound_profile(fx):
    from differential_scenarios import remote_repo
    remote_repo(fx)
    share = fx.dir / "home" / ".local" / "share" / "maintainer" / "profiles" / "magent"
    share.mkdir(parents=True)
    (share / "profile.env").write_text("REPO_SLUG=o/r\nREPO_PATH=%s\nSTATE_DIR=%s\nPOST=on\n" % (fx.repo, fx.state))
    for name in ("MAINTAINER_SLUG", "MAINTAINER_REPO", "MAINTAINER_ACCOUNT", "MAINTAINER_STATE", "MAINTAINER_POST"):
        fx.env.pop(name, None)
    return ["prune", "--dry-run"], {}


def check_unbound_profile(bash, rust):
    # The bash prints `profile.sh: line N:` in front; the port cannot know the line. Both end with 1.
    problems = []
    for side, got in (("bash", bash), ("rust", rust)):
        if got["rc"] != "1" or "GH_ACCOUNT: unbound variable" not in got["stderr"] or got["stdout"]:
            problems.append("%s: rc=%s %r" % (side, got["rc"], got["stderr"]))
    return problems


PINNED = [
    ("pinned-unbound-profile", pinned_unbound_profile, check_unbound_profile),
    ("pinned-loop-stdin", pinned_loop_stdin, check_loop_stdin),
    ("pinned-nonutf8-branch", pinned_nonutf8_branch, check_nonutf8_branch),
    ("pinned-python-missing", pinned_python_missing, check_python_missing),
    ("pinned-git-missing", pinned_git_missing, check_git_missing),
]

FIELDS = ("rc", "stdout", "stderr", "calls", "state")

# What the corpus has to reach on the bash side, as (label, field, marker). A full run in
# which one of these never happens proves nothing about that path, so it fails.
COVERAGE = [
    ("prune: a remote branch deleted", "calls", "git push -q origin --delete"),
    ("prune: a local branch deleted", "calls", "git branch -D"),
    ("prune: a protected delete", "stdout", "could not delete"),
    ("prune: a branch kept for an open PR (F5 alive)", "stdout", "(open PR)"),
    ("prune: a dry run", "stdout", "would delete"),
    ("prune: POST=off", "stdout", "POST=off: listing only"),
    ("prune: fetch fails", "stderr", "maintainer-repo: fetch failed"),
    ("release: nothing to release", "stdout", "nothing to release"),
    ("release: release due", "stdout", "RELEASE DUE"),
    ("release: docs only", "stdout", "docs and tests only"),
    ("release: behind warning", "stdout", "BEHIND origin/main"),
    ("release: not on origin/main", "stdout", "HEAD is not on origin/main"),
    ("release: uncommitted changelog", "stdout", "uncommitted, so origin/main"),
    ("release: no PROD_GLOBS", "stdout", "declares no PROD_GLOBS"),
    ("release: no tags", "stderr", "maintainer-repo: no tags"),
    ("fixup: a heads-up posted", "stdout", "posted the heads-up"),
    ("fixup: a heads-up dry run", "stdout", "would post this heads-up"),
    ("fixup: a push confirmed", "stdout", "(read back from GitHub)"),
    ("fixup: a leased rewrite pushed", "calls", "--force-with-lease=refs/heads/fix-1"),
    ("fixup: a rebase plan", "stdout", "plan: rebase"),
    ("fixup: a rebase conflict", "stderr", "conflicts. Resolving conflicts is not automated"),
    ("fixup: a count check", "stdout", "fixup: test-count"),
    ("fixup: a refused count", "stderr", "a typed count is refused"),
    ("fixup: NOT confirmed", "stderr", "NOT confirmed"),
    ("fixup: a refused push", "stderr", "the push was refused"),
    ("fixup: a moved branch", "stderr", "has moved since the check"),
    ("fixup: a sign-off", "stdout", "Signed-off-by: testuser"),
    ("fixup: a wrong account", "stderr", "gh is authenticated as"),
    ("fixup: usage", "stderr", "usage: maintainer-repo fixup"),
    ("profile: no lib/profile.sh", "stderr", "maintainer_load_profile: command not found"),
    ("profile: unbound HOME", "stderr", "HOME: unbound variable"),
    ("profile: two deployed", "stderr", "2 profiles are deployed"),
]


def restore(fx, snap):
    subprocess.run(["chmod", "-R", "u+rwX", str(fx.dir)], check=False)
    shutil.rmtree(fx.dir)
    shutil.copytree(snap, fx.dir, symlinks=True)


def diff_report(field, a, b):
    import difflib
    shorten = lambda text: [line if len(line) <= 160 else line[:160] + "..." for line in text.splitlines()]
    delta = difflib.unified_diff(shorten(a), shorten(b), "bash", "rust", lineterm="", n=1)
    return "      %s differs:\n%s" % (field, "\n".join("        " + line for line in list(delta)[:14]))


def main():
    root, work, rust_bin, seed, only = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3], int(sys.argv[4]), sys.argv[5]
    verbose = os.environ.get("DIFF_VERBOSE", "0") == "1"
    realgit = subprocess.run(["bash", "-c", "PATH=/usr/bin:/bin type -P git"], capture_output=True, text=True).stdout.strip()
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import differential_scenarios as sc
    stub_files(work, realgit)
    tree_bash, tree_rust = build_trees(work, root, rust_bin)
    trees = (tree_bash, tree_rust)
    fx = Fx(work, realgit)
    counts = {"prune": 150, "release": 110, "fixup": 140, "usage": 18, "profile": 24, "notag": 6, "notice": 20}
    scenarios = [(n, b, None) for n, b in sc.all_scenarios(counts)]
    scenarios += [(n, b, c) for n, b, c in PINNED]
    wanted = [w for w in only.split(",") if w]
    if wanted:
        scenarios = [s for s in scenarios if any(w in s[0] for w in wanted)]
        if not scenarios:
            sys.exit("differential: DIFF_ONLY=%s matched no scenario" % only)
    snap = work / "snapshot"
    total = fails = pinned_checked = merged_compared = 0
    started = time.time()
    rc_seen = {}
    reached = {label: 0 for label, _, _ in COVERAGE}
    with_calls = with_stdout = 0
    for name, builder, check in scenarios:
        rng = random.Random("%s/%s" % (seed, name))
        fx.reset()
        fx.variant = None
        try:
            args, extra = builder(fx, rng) if check is None else builder(fx)
        except Exception as error:  # a fixture that cannot be built is a harness fault, not a pass
            print("differential: cannot build %s: %s" % (name, error), file=sys.stderr)
            sys.exit(2)
        if snap.exists():
            shutil.rmtree(snap)
        shutil.copytree(fx.dir, snap, symlinks=True)
        side_bash, side_rust = tree_bash, tree_rust
        variant = getattr(fx, "variant", None)
        if variant:
            side_bash, side_rust = variant_tree(work, tree_bash, variant), variant_tree(work, tree_rust, variant)
        side_trees = trees + ((side_bash, side_rust) if variant else ())
        got_bash = run_side(fx, side_bash, args, extra, side_trees)
        restore(fx, snap)
        got_rust = run_side(fx, side_rust, args, extra, side_trees)
        total += 1
        for label, field, marker in COVERAGE:
            if marker in got_bash[field]:
                reached[label] += 1
        rc_seen[got_bash["rc"]] = rc_seen.get(got_bash["rc"], 0) + 1
        with_calls += bool(got_bash["calls"].strip())
        with_stdout += bool(got_bash["stdout"])
        if check is not None:
            problems = check(got_bash, got_rust)
            pinned_checked += 1
            if problems:
                fails += 1
                print("FAIL  %s (pinned)\n      %s" % (name, "\n      ".join(problems)))
            elif verbose:
                print("pin   %s" % name)
            continue
        bad = [f for f in FIELDS if got_bash[f] != got_rust[f]]
        if total % 3 == 0 and not bad:
            # The same scenario with both streams into one pipe: the order of the lines is compared too.
            restore(fx, snap)
            merged_bash = run_side(fx, side_bash, args, extra, side_trees, merge=True)
            restore(fx, snap)
            merged_rust = run_side(fx, side_rust, args, extra, side_trees, merge=True)
            merged_compared += 1
            if merged_bash["stdout"] != merged_rust["stdout"] or merged_bash["rc"] != merged_rust["rc"]:
                fails += 1
                print("FAIL  %s  args=%s (interleaved capture)" % (name, shlex.join(args)))
                print(diff_report("interleaved output", merged_bash["stdout"], merged_rust["stdout"]))
                continue
        if bad:
            fails += 1
            print("FAIL  %s  args=%s" % (name, shlex.join(args)))
            for field in bad:
                print(diff_report(field, got_bash[field], got_rust[field])); print("      bash %s: %r" % (field, clip(got_bash[field], 400))); print("      rust %s: %r" % (field, clip(got_rust[field], 400)))
        elif verbose:
            print("ok    %s  rc=%s args=%s" % (name, got_bash["rc"], shlex.join(args)))
            if os.environ.get("DIFF_SHOW") == "1":
                print("      stderr: %s" % clip(got_bash["stderr"], 300).replace("\n", " | ")); print("      stdout: %s" % clip(got_bash["stdout"], 200).replace("\n", " | "))
    free = total - pinned_checked
    print("differential: bash rc histogram %s" % ", ".join("%s:%d" % kv for kv in sorted(rc_seen.items())))
    print("differential: %d of %d scenarios made at least one gh, git or fixup-check call; %d printed on stdout"
          % (with_calls, total, with_stdout))
    if not wanted:
        unreached = [label for label, count in reached.items() if count == 0]
        print("differential: coverage %s" % "; ".join("%s %d" % (label.split(": ")[1], count) for label, count in sorted(reached.items())))
        if unreached:
            print("differential: the corpus never reached: %s" % ", ".join(unreached))
            fails += 1
        if len(rc_seen) < 4 or with_calls < total * 0.6 or with_stdout < total * 0.4:
            print("differential: the corpus is too uniform to prove anything (rc kinds %d, calls %d, stdout %d)"
                  % (len(rc_seen), with_calls, with_stdout))
            fails += 1
    print("differential: %d scenarios also compared with stdout and stderr in one pipe, in order" % merged_compared)
    print("differential: %d comparisons (%d scenarios + %d pinned, %d fields each), %d differences, %.0f s (seed %d)"
          % (total, free, pinned_checked, len(FIELDS), fails, time.time() - started, seed))
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
