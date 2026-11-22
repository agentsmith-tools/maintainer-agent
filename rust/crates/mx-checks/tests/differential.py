#!/usr/bin/env python3
"""Differential proof: the five Rust checks against the five bash scripts.

Every case builds one generated sandbox (a tree, a HOME, a TMPDIR, a farm of real tools and
logging stubs for everything the program starts), runs the bash script in it, rebuilds the same
sandbox in the same place, runs the Rust binary through a wrapper that stands where the script
stood, and compares:

  standard output, standard error, the exit code, the global call log (one line per stub call
  across every stub: the tool, its argv, its working directory and its whole environment, in
  order), and the state tree left behind (paths, types, modes, content hashes, and every file the
  run created in TMPDIR).

Both runs happen at the same path, so no path needs rewriting. The random name `mktemp` gives a
file or a directory (tmp.XXXXXXXXXX) is the only thing normalised.

Cases whose difference is a recorded decision are in the `explained` group: each is asserted on
both sides (what the bash does, what the Rust does) and counted as `explained`, never as a match.
Everything else must match exactly: an unexplained difference fails the run.

Usage: differential.py --rust-bin-dir DIR [--tree DIR] [--seed N] [--only FRAGMENT[,FRAGMENT]] [-v]
  --rust-bin-dir  the directory holding the five release binaries
  --tree          the tree whose scripts are the bash side (default: this checkout)
"""
import argparse
import hashlib
import os
import random
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))

TOOLS = ("bash cp rm mkdir mv sed grep awk cat chmod touch find sort tr head tail dirname basename readlink git "
         "python3 date env timeout ln ls cut wc stat sha256sum mktemp tee diff cmp sleep true false xargs paste "
         "nice uniq").split()
A = "a" * 40
B = "b" * 40
C = "c" * 40
D = "d" * 40
SHAS = [A, B, C, D]
LOG_HEAD = r'''#!/usr/bin/env bash
t="$(basename "$0")"
d="${CZ_CASE_DIR:?}"
line="$t"; for a in "$@"; do line="$line $(printf '%q' "$a")"; done
envs="$(env | grep -vE '^(_|PWD|SHLVL|OLDPWD)=' | sort | tr '\n' ' ')"
printf '%s | cwd=%s | env=%s\n' "$line" "$PWD" "$envs" >> "$d/calls"
'''


def real(name):
    for d in ("/usr/local/bin", "/usr/bin", "/bin"):
        p = os.path.join(d, name)
        if os.path.exists(p):
            return p
    raise SystemExit("differential: no " + name)


class Sandbox:
    """One generated host at a fixed path."""

    def __init__(self, path):
        self.path = path
        self.tree = os.path.join(path, "tree")
        self.bin = os.path.join(path, "bin")
        self.tools = os.path.join(path, "tools")
        self.tmp = os.path.join(path, "tmp")
        self.cwd = os.path.join(path, "cwd")
        self.home = os.path.join(path, "home")
        for d in (self.tree, self.bin, self.tools, self.tmp, self.cwd, self.home):
            os.makedirs(d)
        for t in TOOLS:
            os.symlink(real(t), os.path.join(self.tools, t))
        open(os.path.join(path, "calls"), "w").close()
        self.env = {
            "PATH": self.bin + ":" + self.tools, "HOME": self.home, "LC_ALL": "C", "TZ": "UTC",
            "TMPDIR": self.tmp, "CZ_CASE_DIR": path, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_TERMINAL_PROMPT": "0", "LOGNAME": "fixture-user", "USER": "fixture-user",
        }

    def write(self, rel, text, mode=None, base=None):
        p = os.path.join(base or self.tree, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(text if isinstance(text, bytes) else text.encode())
        if mode:
            os.chmod(p, mode)
        return p

    def stub(self, name, body, logged=True):
        text = (LOG_HEAD if logged else "#!/usr/bin/env bash\n") + body
        self.write(name, text, 0o755, base=self.bin)

    def drop_tool(self, name):
        for d in (self.bin, self.tools):
            p = os.path.join(d, name)
            if os.path.lexists(p):
                os.remove(p)

    def passthrough(self, name):
        """A stub that logs the call and runs the real tool."""
        self.stub(name, 'exec "%s" "$@"\n' % real(name))

    def fail_unplanned(self, *names):
        for n in names:
            self.stub(n, 'echo "stub: unplanned $t" >&2; exit 99\n')


class Case:
    def __init__(self, name, build, explained=None, sort_stderr=False):
        self.name, self.build, self.explained, self.sort_stderr = name, build, explained, sort_stderr


class Spec:
    """What a build produces: the program, its arguments, and what it leaves out of the comparison."""

    def __init__(self, program, args=(), env=None, cwd=None, argv0=None, timeout=60):
        self.program, self.args, self.env, self.cwd, self.argv0, self.timeout = program, list(args), env or {}, cwd, argv0, timeout


TMPNAME = re.compile(r"tmp\.[A-Za-z0-9]{10}")


def norm(text):
    return TMPNAME.sub("tmp.RANDOM", text)


def state(sb, skip):
    rows = []
    for base, dirs, files in os.walk(sb.path):
        dirs.sort()
        for name in sorted(dirs + files):
            p = os.path.join(base, name)
            rel = os.path.relpath(p, sb.path)
            if rel in ("calls", "stdout", "stderr") or p in skip or rel.startswith(".git") or "/.git/" in "/" + rel + "/":
                continue
            if rel.startswith("bin/") or rel.startswith("tools/"):
                continue
            st = os.lstat(p)
            if os.path.islink(p):
                rows.append("%s link -> %s" % (norm(rel), os.readlink(p)))
            elif os.path.isdir(p):
                rows.append("%s dir %o" % (norm(rel), st.st_mode & 0o777))
            else:
                try:
                    with open(p, "rb") as f:
                        h = hashlib.sha256(f.read()).hexdigest()[:16]
                except PermissionError:
                    h = "unreadable"
                rows.append("%s file %o %d %s" % (norm(rel), st.st_mode & 0o777, st.st_size, h))
    return "\n".join(rows)


def run_once(case, path, mode, rust_bins, tree_src):
    shutil.rmtree(path, ignore_errors=True)
    os.makedirs(path)
    sb = Sandbox(path)
    spec = case.build(sb, tree_src)
    program = spec.program
    if mode == "rust":
        name = spec.rust_name
        wrapper = '#!/usr/bin/env bash\nexec -a "${BASH_SOURCE[0]}" %s "$@"\n' % os.path.join(rust_bins, name)
        with open(program, "w") as f:
            f.write(wrapper)
        os.chmod(program, 0o755)
    env = dict(sb.env)
    env.update(spec.env)
    cwd = spec.cwd or sb.cwd
    argv = ["env", "-i"] + ["%s=%s" % kv for kv in sorted(env.items())] + ["timeout", str(spec.timeout), "bash", spec.argv0 or program] + spec.args
    # Appending, so a program that writes to /dev/stdout through a second open lands after what the
    # first descriptor already wrote instead of over it.
    with open(os.path.join(path, "stdout"), "ab") as out, open(os.path.join(path, "stderr"), "ab") as err:
        rc = subprocess.run(argv, cwd=cwd, stdout=out, stderr=err, stdin=subprocess.DEVNULL).returncode
    res = {
        "rc": str(rc),
        "stdout": norm(open(os.path.join(path, "stdout"), errors="replace").read()),
        "stderr": norm(open(os.path.join(path, "stderr"), errors="replace").read()),
        "calls": norm(open(os.path.join(path, "calls"), errors="replace").read()),
        "state": state(sb, {program}),
    }
    if case.sort_stderr:
        res["stderr"] = "\n".join(sorted(res["stderr"].splitlines()))
    return res


def copy_program(sb, tree_src, rel):
    src = os.path.join(tree_src, rel)
    return sb.write(rel, open(src, "rb").read(), 0o755)


# --- check-claims ---------------------------------------------------------------------------------------------

DENY_OK = '{"spelled_everywhere": ["cargo publish", "maintainer-merge receipt"], "credential_paths": ["~/.ssh/**"]}'
DENY_OTHER = '{"spelled_everywhere": ["git push", "maintainer-merge receipt", "rm -rf"], "credential_paths": ["~/.ssh/**", "~/.aws/**"], "bare_exact": ["sudo"]}'
DENY_BAD = ['{"spelled_everywhere": ["x"], "credential_paths": []}', "not json", "{}", '{"credential_paths": ["/home/claimcheck/x"]}', ""]


def claims_case(i, seed):
    def build(sb, tree_src):
        rnd = random.Random("claims-%s-%d" % (seed, i))
        prog = copy_program(sb, tree_src, "scripts/check_claims.sh")
        sb.write("scripts/render-settings.py", open(os.path.join(tree_src, "scripts/render-settings.py"), "rb").read(), 0o755)
        sb.write("scripts/render_figures.py", FIGS_STUB, 0o755)
        sb.write("tests/run-tests.sh", SUITE_STUB, 0o755)
        sb.passthrough("python3")
        sb.fail_unplanned("gh", "podman", "docker")
        n = rnd.choice([0, 1, 1, 2, 3])
        pool = ["sysknife", "magent", "_template", "zeta", "Alpha", "x.y", "a b", "z-9", "it's", "q\"q", "t\u00e9"]
        names = rnd.sample(pool, n)
        for name in names:
            sb.write("profiles/%s/deny.json" % name, rnd.choice([DENY_OK, DENY_OK, DENY_OTHER, rnd.choice(DENY_BAD)]))
        if rnd.random() < 0.3:
            sb.write("profiles/.hidden/deny.json", DENY_OK)
        if rnd.random() < 0.3:
            os.makedirs(os.path.join(sb.tree, "profiles/nodeny"), exist_ok=True)
        if rnd.random() < 0.2:
            sb.write("profiles/README", "text")
        if rnd.random() < 0.2:
            os.makedirs(os.path.join(sb.tree, "profiles/dangling"), exist_ok=True)
            os.symlink("/nonexistent/deny.json", os.path.join(sb.tree, "profiles/dangling/deny.json"))
        if rnd.random() < 0.2 and names:
            os.symlink(os.path.join(sb.tree, "profiles", names[0]), os.path.join(sb.tree, "profiles", "linked"))
        # the suite
        kind = rnd.choice(["green", "green", "green", "skipped", "red", "nosummary", "stray", "zero", "missing"])
        passed = rnd.choice([0, 7, 10, 1650, 99999])
        lines = ["== a section =="] + ["  PASS  case %d" % k for k in range(rnd.randint(0, 3))]
        rc = 0
        if kind == "green":
            lines.append("  %d passed, 0 failed" % passed + rnd.choice(["", "", "\r"]))
        elif kind == "skipped":
            lines.append("  %d passed, 0 failed, %d skipped" % (passed, rnd.choice([0, 1, 3, 40])))
            lines.append("  A skip is a case this machine could not run.")
        elif kind == "red":
            rc = rnd.choice([1, 2, 255])
            lines += ["  FAIL  case %d" % k for k in range(rnd.randint(0, 14))] + ["    FAIL  indented", "  %d passed, 3 failed" % passed]
        elif kind == "nosummary":
            lines.append("all good, no summary")
        elif kind == "stray":
            lines += ["  %d passed, 0 failed, 2 skipped" % passed, "note: 99 passed in a sub-run, 4 skipped", "   40 passed", "x 5 passed"]
        elif kind == "zero":
            lines.append("  0 passed, 0 failed")
        if kind != "missing":
            sb.write("suite.out", "\n".join(lines) + ("\n" if rnd.random() < 0.8 else ""), base=sb.path)
            sb.write("suite.err", rnd.choice(["", "a stderr line, 70 passed\n", "  FAIL  on stderr\n"]), base=sb.path)
            sb.write("suite.rc", str(rc), base=sb.path)
        else:
            os.remove(os.path.join(sb.tree, "tests/run-tests.sh"))
        # the figures
        crc = rnd.choice([0, 0, 0, 1, 2])
        sb.write("figs.check.out", rnd.choice(["  ok: 12 figure markers in 3 files are current\n", "first\nsecond\n", "", "ok\n", "ok\0nul\n"]), base=sb.path)
        sb.write("figs.check.err", rnd.choice(["", "FAIL README.md:639: stale marker\n"]), base=sb.path)
        sb.write("figs.check.rc", str(crc), base=sb.path)
        vrc = rnd.choice([0, 0, 0, 1])
        sb.write("figs.values.out", rnd.choice(["tests 10\nevals 9\n", "tests 1,800\nbackends 4\n", "", "a 1\n\nb 2\n", "only", "a\0b 1\n", "tests 10\r\n"]), base=sb.path)
        sb.write("figs.values.err", rnd.choice(["", "cannot read docs/lessons.md\n"]), base=sb.path)
        sb.write("figs.values.rc", str(vrc), base=sb.path)
        spec = Spec(prog)
        spec.rust_name = "check-claims"
        how = rnd.choice(["abs", "abs", "rel", "link"])
        if how == "rel":
            spec.argv0 = "tree/scripts/check_claims.sh"
            spec.cwd = sb.path
        elif how == "link":
            os.symlink(sb.tree, os.path.join(sb.path, "linked-tree"))
            spec.argv0 = os.path.join(sb.path, "linked-tree/scripts/check_claims.sh")
        return spec

    return Case("claims-%03d" % i, build)


SUITE_STUB = r'''#!/usr/bin/env bash
d="${CZ_CASE_DIR:?}"
printf 'suite %s args=%d\n' "$PWD" "$#" >> "$d/calls"
[ -e "$d/suite.out" ] && cat "$d/suite.out"
[ -e "$d/suite.err" ] && cat "$d/suite.err" >&2
rc=0; [ -e "$d/suite.rc" ] && rc="$(cat "$d/suite.rc")"
exit "$rc"
'''
FIGS_STUB = r'''#!/usr/bin/env python3
import os, shlex, sys
d = os.environ["CZ_CASE_DIR"]
mode = "values" if "--values" in sys.argv else "check"
with open(os.path.join(d, "calls"), "a") as f:
    f.write("figs " + " ".join(shlex.quote(a) for a in sys.argv[1:]) + "\n")
def read(name):
    p = os.path.join(d, name)
    return open(p).read() if os.path.exists(p) else ""
sys.stdout.write(read("figs.%s.out" % mode))
sys.stdout.flush()
sys.stderr.write(read("figs.%s.err" % mode))
sys.stderr.flush()
rc = read("figs.%s.rc" % mode).strip()
sys.exit(int(rc) if rc else 0)
'''

# --- verify-action-pins ----------------------------------------------------------------------------------------

REPOS = ["actions/checkout", "acme/one", "github/codeql-action", "o/r", "Alpha/b", "zeta/a"]
TAGS = ["v1", "v2", "v4", "v1.2.3", "main", "master", "mainline", "release/1", "v1+x"]


def pin_lines(rnd, repo_tags):
    """One workflow's worth of lines, from the shapes the checker must read or refuse."""
    out = []
    for _ in range(rnd.randint(0, 7)):
        repo = rnd.choice(REPOS)
        tag = rnd.choice(TAGS)
        sha = rnd.choice(SHAS)
        action = repo + rnd.choice(["", "", "/sub", "/a/b"])
        shape = rnd.choice(["good", "good", "good", "good", "floating", "short", "nocomment", "upper", "quoted",
                            "twowords", "glued", "nospace", "crlf", "trail", "dash", "plain", "comment", "tab",
                            "double", "hash2", "local", "docker", "empty"])
        repo_tags.add((repo, tag))
        if shape == "good":
            out.append("      - uses: %s@%s # %s" % (action, sha, tag))
        elif shape == "dash":
            out.append("  -   uses: %s@%s # %s" % (action, sha, tag))
        elif shape == "plain":
            out.append("        uses: %s@%s   #   %s" % (action, sha, tag))
        elif shape == "floating":
            out.append("      - uses: %s@%s" % (action, tag))
        elif shape == "short":
            out.append("      - uses: %s@%s # %s" % (action, sha[:7], tag))
        elif shape == "nocomment":
            out.append("      - uses: %s@%s" % (action, sha))
        elif shape == "upper":
            out.append("      - uses: %s@%s # %s" % (action, sha.upper(), tag))
        elif shape == "quoted":
            out.append('      - uses: "%s@%s" # %s' % (action, sha, tag))
        elif shape == "twowords":
            out.append("      - uses: %s@%s # %s extra words" % (action, sha, tag))
        elif shape == "glued":
            out.append("      - uses: %s@%s# %s" % (action, sha, tag))
        elif shape == "nospace":
            out.append("      - uses: %s@%s #%s" % (action, sha, tag))
        elif shape == "crlf":
            out.append("      - uses: %s@%s # %s\r" % (action, sha, tag))
        elif shape == "trail":
            out.append("      - uses: %s@%s # %s   \t " % (action, sha, tag))
        elif shape == "comment":
            out.append("      # - uses: %s@%s # %s" % (action, sha, tag))
        elif shape == "tab":
            out.append("      - uses:\t%s@%s # %s" % (action, sha, tag))
        elif shape == "double":
            out.append("      - uses: %s@%s # %s uses: %s@%s # v9" % (action, sha, tag, repo, sha))
        elif shape == "hash2":
            out.append("      - uses: %s@%s # %s#2" % (action, sha, tag))
        elif shape == "local":
            out.append("      - uses: ./.github/actions/local")
        elif shape == "docker":
            out.append("      - uses: docker://alpine:3")
        elif shape == "empty":
            out.append("      - uses: %s@%s #" % (action, sha))
        if rnd.random() < 0.1:
            out.append("      - run: echo uses: x/y@v1")
    return out


GH_STUB = r'''d="${CZ_CASE_DIR:?}"
if [ "${1:-}" = api ] && [ "${3:-}" = --jq ]; then
    while IFS=$'\x1f' read -r p j rc out; do
        if [ "$p" = "$2" ] && [ "$j" = "$4" ]; then
            [ -n "$out" ] && printf '%s' "$out"
            [ "$rc" != 0 ] && echo "gh: Not Found (HTTP 404)" >&2
            exit "$rc"
        fi
    done < "$d/answers"
fi
echo "stub gh: unplanned call: $*" >&2
exit 99
'''


def pins_case(i, seed):
    def build(sb, tree_src):
        rnd = random.Random("pins-%s-%d" % (seed, i))
        prog = copy_program(sb, tree_src, "scripts/verify-action-pins.sh")
        sb.stub("gh", GH_STUB)
        sb.fail_unplanned("podman", "docker")
        repo_tags = set()
        files = rnd.randint(0, 3)
        for k in range(files):
            name = rnd.choice([".yml", ".yaml", ".yml", ".yml.bak", ".txt"])
            lines = pin_lines(rnd, repo_tags)
            sb.write(".github/workflows/w%d%s" % (k, name), "\n".join(lines) + rnd.choice(["\n", "", "\n\n"]))
        if rnd.random() < 0.3:
            sb.write(".github/workflows/sub/deep.yml", "\n".join(pin_lines(rnd, repo_tags)) + "\n")
        if rnd.random() < 0.15:
            sb.write(".github/workflows/binary.yml", b"\0" + ("\n".join(pin_lines(rnd, repo_tags)) + "\n").encode())
        if rnd.random() < 0.1:
            sb.write(".github/workflows/long.yml", ("      - uses: " + "x" * 20000 + "@v1\n") * 2)
        if rnd.random() < 0.1:
            sb.write(".github/workflows/empty.yml", "")
        if rnd.random() < 0.15:
            os.makedirs(os.path.join(sb.tree, ".github/workflows"), exist_ok=True)
        r = rnd.random()
        if r < 0.3:
            sb.write(".github/actions/setup/action.yml", "\n".join(pin_lines(rnd, repo_tags)) + "\n")
            if rnd.random() < 0.3:
                sb.write(".github/actions/x/y/action.yaml", "\n".join(pin_lines(rnd, repo_tags)) + "\n")
        elif r < 0.4:
            sb.write(".github/actions/readme.md", "text")
        if rnd.random() < 0.3:
            sb.write("examples/one.yml", "\n".join(pin_lines(rnd, repo_tags)) + "\n")
            sb.write("examples/two.yaml", "\n".join(pin_lines(rnd, repo_tags)) + "\n")
        if rnd.random() < 0.15:
            sb.write("elsewhere.yml", "\n".join(pin_lines(rnd, repo_tags)) + "\n")
            os.makedirs(os.path.join(sb.tree, ".github/workflows"), exist_ok=True)
            os.symlink(os.path.join(sb.tree, "elsewhere.yml"), os.path.join(sb.tree, ".github/workflows/link.yml"))
        # the answers
        rows = []
        for repo, tag in sorted(repo_tags):
            kind = rnd.choice(["light", "light", "annotated", "mismatch", "missing", "typefail", "tagfail", "blob", "empty"])
            sha = rnd.choice(SHAS)
            path = "repos/%s/git/ref/tags/%s" % (repo, tag)
            obj = "1" * 40
            if kind == "light":
                rows += [(path, ".object.sha", 0, sha), (path, ".object.type", 0, "commit")]
            elif kind == "annotated":
                rows += [(path, ".object.sha", 0, obj), (path, ".object.type", 0, "tag"), ("repos/%s/git/tags/%s" % (repo, obj), ".object.sha", 0, sha)]
            elif kind == "mismatch":
                rows += [(path, ".object.sha", 0, rnd.choice(SHAS)), (path, ".object.type", 0, "commit")]
            elif kind == "missing":
                rows += [(path, ".object.sha", 1, "")]
            elif kind == "typefail":
                rows += [(path, ".object.sha", 0, sha), (path, ".object.type", 1, "")]
            elif kind == "tagfail":
                rows += [(path, ".object.sha", 0, obj), (path, ".object.type", 0, "tag"), ("repos/%s/git/tags/%s" % (repo, obj), ".object.sha", rnd.choice([0, 1]), rnd.choice(["", "partial"]))]
            elif kind == "blob":
                rows += [(path, ".object.sha", 0, sha), (path, ".object.type", 0, "blob")]
            else:
                rows += [(path, ".object.sha", 0, ""), (path, ".object.type", 0, "")]
        with open(os.path.join(sb.path, "answers"), "w") as f:
            for p, j, rc, out in rows:
                f.write("%s\x1f%s\x1f%d\x1f%s\n" % (p, j, rc, out))
        if rnd.random() < 0.1:
            sb.drop_tool("gh")
        args = []
        if rnd.random() < 0.3:
            args = [rnd.choice([sb.tree, sb.tree + "/", "../tree", "../tree/", os.path.join(sb.path, "missing"), ""])]
        spec = Spec(prog, args)
        spec.rust_name = "verify-action-pins"
        return spec

    return Case("pins-%03d" % i, build)


# --- shellcheck-sweep -------------------------------------------------------------------------------------------

SHEBANGS = ["#!/usr/bin/env bash\n", "#!/bin/bash\n", "#!/bin/sh\n", "#!/usr/bin/env sh\n", "#!/bin/dash\n", "#!/usr/bin/env python3\n",
            "# see #!/bin/bash here\n", "plain text\n", "", "\0\0#!/bin/bash\n", "#!/usr/bin/env\0 bash\n", "#!/bin/shell\n"]
NAMES = ["a.sh", "b", "c.sh", "d.py", "e.txt", "my script.sh", "-x.sh", "café.sh", "tab\tname.sh", "z.sh", "A.sh",
         "dir/f.sh", "profiles/p/verify.d/x.sh", "profiles/p/q/verify.d/deep/z.sh", "profiles/verify.d/y.sh", "other/verify.d/z.sh",
         "profiles/p/verify.d/w.bash", "über", "q[1].sh", "star*.sh", "(paren).sh"]
SC_STUB = r'''f="${!#}"; b="$(basename "$f")"
[ -e "$d/sc.out.$b" ] && cat "$d/sc.out.$b"
[ -e "$d/sc.err.$b" ] && cat "$d/sc.err.$b" >&2
if [ -e "$d/sc.rc.$b" ]; then exit "$(cat "$d/sc.rc.$b")"; fi
exit 0
'''
SD_STUB = r'''[ -e "$d/sd.fail" ] && [ "${!#}" = true ] && exit 1
while [ $# -gt 0 ]; do case "$1" in -p) shift 2 ;; --*) shift ;; *) break ;; esac; done
exec "$@"
'''
SLEEP_STUB = r'''n="$(grep -c "^sleep" "$d/calls")"
if [ -e "$d/mem.after" ] && [ "$n" = "$(cat "$d/mem.after")" ]; then printf 'MemTotal: 1 kB\nMemAvailable: 20000000 kB\n' > "$d/meminfo"; fi
exit 0
'''


def sweep_case(i, seed):
    def build(sb, tree_src):
        rnd = random.Random("sweep-%s-%d" % (seed, i))
        prog = copy_program(sb, tree_src, "scripts/shellcheck-sweep.sh")
        env = {}
        gitenv = dict(os.environ)
        gitenv.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_AUTHOR_NAME": "f", "GIT_AUTHOR_EMAIL": "f@x.invalid",
                       "GIT_COMMITTER_NAME": "f", "GIT_COMMITTER_EMAIL": "f@x.invalid", "GIT_AUTHOR_DATE": "1767225600 +0000", "GIT_COMMITTER_DATE": "1767225600 +0000"})
        gitenv.pop("GIT_DIR", None)
        subprocess.run(["git", "init", "-q", "-b", "main"], cwd=sb.tree, env=gitenv, check=True)
        names = rnd.sample(NAMES, rnd.randint(0, 9))
        for name in names:
            sb.write(name, (rnd.choice(SHEBANGS) + "echo hi\n").encode("utf-8", "surrogateescape"))
        if rnd.random() < 0.5:
            sb.write("long.sh", "x" * rnd.choice([50, 70, 71, 72, 75]) + "#!/bin/sh\n")
        subprocess.run(["git", "add", "-A"], cwd=sb.tree, env=gitenv, check=True, capture_output=True)
        subprocess.run(["git", "commit", "-q", "-m", "base", "--allow-empty"], cwd=sb.tree, env=gitenv, check=True, capture_output=True)
        if rnd.random() < 0.2 and names:
            victim = rnd.choice([n for n in names if "\t" not in n] or ["x"])
            p = os.path.join(sb.tree, victim)
            if os.path.exists(p):
                os.remove(p)
        if rnd.random() < 0.3:
            sb.write("untracked.sh", "#!/bin/bash\n")
        if rnd.random() < 0.2:
            sb.write("link-target.sh", "#!/bin/bash\n")
            os.symlink("link-target.sh", os.path.join(sb.tree, "sym.sh"))
            subprocess.run(["git", "add", "-A"], cwd=sb.tree, env=gitenv, check=True, capture_output=True)
            subprocess.run(["git", "commit", "-q", "-m", "link"], cwd=sb.tree, env=gitenv, check=True, capture_output=True)
        if rnd.random() < 0.15 and os.path.exists(os.path.join(sb.tree, "a.sh")):
            os.chmod(os.path.join(sb.tree, "a.sh"), 0)
        if rnd.random() < 0.08:
            shutil.rmtree(os.path.join(sb.tree, ".git"))
        # stubs
        if rnd.random() < 0.93:
            sb.stub("shellcheck", SC_STUB)
        sb.stub("sleep", SLEEP_STUB)
        if rnd.random() < 0.6:
            sb.stub("systemd-run", SD_STUB)
            if rnd.random() < 0.3:
                open(os.path.join(sb.path, "sd.fail"), "w").close()
        if rnd.random() < 0.3:
            sb.drop_tool("nice")
        else:
            sb.stub("nice", "shift 2; exec \"$@\"\n")
        for name in set(names + ["long.sh", "untracked.sh"]):
            b = os.path.basename(name)
            r = rnd.random()
            if r < 0.15:
                sb.write("sc.rc." + b, rnd.choice(["1", "2", "137", "255", "0"]), base=sb.path)
            if r > 0.85:
                sb.write("sc.out." + b, "In %s line 2: warning\n" % name, base=sb.path)
                sb.write("sc.err." + b, "diag for %s\n" % name, base=sb.path)
        sb.write("scripts/shellcheck-sweep.sh", open(prog, "rb").read(), 0o755)
        # memory
        mem = rnd.choice(["roomy", "roomy", "roomy", "short", "none", "junk", "unterminated", "first", "tab"])
        text = {
            "roomy": "MemTotal: 32000000 kB\nMemAvailable:   20000000 kB\n",
            "short": "MemTotal: 32000000 kB\nMemAvailable: %d kB\n" % rnd.choice([0, 1000, 10485759]),
            "none": "MemTotal: 32000000 kB\n",
            "junk": "MemAvailable: lots kB\n",
            "unterminated": "MemTotal: 1 kB\nMemAvailable: 20000000 kB",
            "first": "MemAvailable: 1 kB\nMemAvailable: 20000000 kB\n",
            "tab": "\tMemAvailable:\t20000000\tkB\n",
        }[mem]
        sb.write("meminfo", text, base=sb.path)
        env["MAGENT_MEMINFO"] = os.path.join(sb.path, "meminfo")
        if rnd.random() < 0.07:
            env["MAGENT_MEMINFO"] = os.path.join(sb.path, "absent-meminfo")
        env["MAGENT_SWEEP_WAIT"] = rnd.choice(["0", "0", "1", "2", "3"])
        env["MAGENT_SWEEP_POLL"] = rnd.choice(["1", "1", "2"])
        if mem in ("short", "none") and rnd.random() < 0.4:
            sb.write("mem.after", str(rnd.choice([1, 2])), base=sb.path)
        for var, vals in (("CI", ["", "1"]), ("MAGENT_SWEEP_NO_CAP", ["", "1"]), ("MAGENT_SWEEP_MEM_MAX", ["3G", "2G"]), ("MAGENT_SWEEP_MIN_AVAIL_KB", ["1000", "10485760", "20000000"])):
            if rnd.random() < 0.25:
                env[var] = rnd.choice(vals)
        spec = Spec(prog, env=env)
        spec.rust_name = "shellcheck-sweep"
        how = rnd.choice(["abs", "abs", "link"])
        if how == "link":
            os.makedirs(os.path.join(sb.path, "elsewhere"))
            os.symlink(os.path.join(sb.tree, "scripts/shellcheck-sweep.sh"), os.path.join(sb.path, "elsewhere/sweep.sh"))
            spec.argv0 = os.path.join(sb.path, "elsewhere/sweep.sh")
        return spec

    return Case("sweep-%03d" % i, build)


# --- run-evals ---------------------------------------------------------------------------------------------------

IDS = ["01-alpha", "02-beta", "03-gamma", "05-reserved-issue", "10-late", "1-early", "x y"]
NEEDLES = ["back\\slash", "alpha-rule", "BETA-RULE", "data, not instruction", "zzz-absent", " lead", "trail ", "a.b", "-dash", "x*y", "café", "Bash(rm -rf:*)"]
PROMPT = "Doctrine: alpha-rule and beta-rule live here. data, not instruction. lead in\nlast line a.b -dash x*y café\n"
LIVE_BACKEND = r'''backend_run() {
    { printf 'backend_run model=%s dest=%s root=%s task=%s profile_dir=%s repo=%s\n' "$2" "$3" "$4" "${MAINTAINER_TASK-<unset>}" "${PROFILE_DIR-<unset>}" "${REPO_PATH-<unset>}"; cat "$1"; } >> "$3"
    printf 'backend_run\n' >> "${CZ_CASE_DIR:?}/calls"
    [ -e "${CZ_CASE_DIR:?}/backend.fail" ] && return 1
    return 0
}
'''
RUN_STUB = r'''#!/usr/bin/env bash
d="${CZ_CASE_DIR:?}"
line="run.sh"; for a in "$@"; do line="$line $(printf '%q' "$a")"; done
printf '%s\n' "$line" >> "$d/calls"
[ -e "$d/prompt.err" ] && cat "$d/prompt.err" >&2
[ -e "$d/prompt.out" ] && cat "$d/prompt.out"
rc=0; [ -e "$d/prompt.rc" ] && rc="$(cat "$d/prompt.rc")"
exit "$rc"
'''


def evals_case(i, seed):
    def build(sb, tree_src):
        rnd = random.Random("evals-%s-%d" % (seed, i))
        prog = copy_program(sb, tree_src, "evals/run-evals.sh")
        sb.write("lib/run.sh", RUN_STUB, 0o755)
        sb.write("lib/prose-style.md", rnd.choice(["Prose style rules.\n", "", "no newline"]))
        profile = rnd.choice(["fx", "fx", "other"])
        sb.write("lib/backends/claude.sh", LIVE_BACKEND)
        sb.write("lib/backends/other.sh", LIVE_BACKEND.replace("backend_run model", "other_backend model"))
        ids = rnd.sample(IDS, rnd.randint(0, 5))
        for sid in ids:
            sb.write("evals/scenarios/%s.md" % sid, "# S %s\n\nSituation.\n\n## Must not\n\nx\n\n## Where the rule lives\n\ny\n\n## Why\n\nz\n" % sid)
        for junk in rnd.sample(["notes.txt", ".hidden.md", "draft.md.bak"], rnd.randint(0, 2)):
            sb.write("evals/scenarios/" + junk, "x\n")
        entries = {}
        for sid in ids + rnd.sample(["99-ghost", "98-ghost"], rnd.randint(0, 2)):
            r = rnd.random()
            if r < 0.55:
                entries[sid] = {"needle": rnd.choice(NEEDLES)}
            elif r < 0.7:
                entries[sid] = {"n/a": rnd.choice(["no reserved issues here", "  spaced  ", "x"])}
            elif r < 0.8:
                entries[sid] = {"needle": rnd.choice(NEEDLES), "n/a": "both"}
            elif r < 0.9:
                entries[sid] = {}
            else:
                entries[sid] = {"needle": None, "n/a": None}
        if rnd.random() < 0.2:
            entries["_comment"] = "a note"
        import json
        form = rnd.choice(["ok", "ok", "ok", "ok", "bad-json", "string-value", "absent", "list"])
        evals_json = json.dumps(entries)
        if form == "bad-json":
            evals_json = evals_json[:-1]
        elif form == "string-value":
            real_keys = sorted(k for k in entries if not k.startswith("_"))
            if real_keys:
                entries[real_keys[0]] = "plain string"
            evals_json = json.dumps(entries)
        elif form == "list":
            evals_json = "[]"
        if form != "absent":
            sb.write("profiles/%s/evals.json" % profile, evals_json)
        sb.write("profiles/%s/profile.env" % profile, rnd.choice(['PROFILE_NAME="%s"\nREPO_PATH="/fixture/repo"\nMODEL_review="m1"\n' % profile,
                                                                     'PROFILE_NAME="x"\nBACKEND="other"\nREPO_PATH="/r"\n', 'PROFILE_NAME="x"\n']))
        if rnd.random() < 0.7:
            sb.write("profiles/%s/prompts/common-preamble.md" % profile, "Common preamble.\n")
        if rnd.random() < 0.5:
            sb.write("profiles/%s/deny.json" % profile, rnd.choice(['{"d": ["Bash(rm -rf:*)"]}\n', "{}", ""]))
        if rnd.random() < 0.3:
            sb.write("profiles/%s/settings.json" % profile, '{"s": "only-in-settings"}\n')
        sb.write("prompt.out", rnd.choice([PROMPT, PROMPT, "", "zzz\n", PROMPT.upper(), PROMPT.replace("alpha", "al\0pha"), PROMPT.replace("\n", "\r\n")]), base=sb.path)
        if rnd.random() < 0.2:
            sb.write("prompt.err", "alpha-rule on stderr\n", base=sb.path)
        if rnd.random() < 0.15:
            sb.write("prompt.rc", "1", base=sb.path)
        if rnd.random() < 0.3:
            open(os.path.join(sb.path, "backend.fail"), "w").close()
        sb.passthrough("python3")
        sb.fail_unplanned("gh", "podman", "docker")
        env = {}
        if profile != "sysknife" or rnd.random() < 0.1:
            env["MAINTAINER_PROFILE"] = profile if rnd.random() < 0.9 else ""
        if rnd.random() < 0.2:
            env["MAINTAINER_TASK"] = "issues"
        args = rnd.choice([[], [], ["--static"], ["--static", "x"], ["--live"], ["--live", "0"], ["--live", "a"], ["--live", "zzz"], ["--live", "-"], ["--bogus"], ["static"], [""], ["--live", "x y"]])
        spec = Spec(prog, args, env)
        spec.rust_name = "run-evals"
        return spec

    return Case("evals-%03d" % i, build, sort_stderr=True)


# --- containment-probe -------------------------------------------------------------------------------------------

POD_STUB = r'''mkdir -p "$d/rt"
case "${1:-}" in
    image) rc=0; [ -e "$d/rt.inspect.rc" ] && rc="$(cat "$d/rt.inspect.rc")"; exit "$rc" ;;
    pull)  rc=0; [ -e "$d/rt.pull.rc" ] && rc="$(cat "$d/rt.pull.rc")"; exit "$rc" ;;
    ps)
        n=1; [ -e "$d/rt.ps.n" ] && n=$(( $(cat "$d/rt.ps.n") + 1 )); printf %s "$n" > "$d/rt.ps.n"
        want=0
        if [ "$n" = 1 ]; then [ -e "$d/rt.ps.before" ] && want="$(cat "$d/rt.ps.before")"
        else [ -e "$d/rt.ps.after" ] && want="$(cat "$d/rt.ps.after")"; fi
        i=0; while [ "$i" -lt "$want" ]; do echo "container$i"; i=$((i+1)); done
        exit 0 ;;
    run) ;;
    *) exit 0 ;;
esac
cmd="${!#}"
mount=""; prev=""
for a in "$@"; do [ "$prev" = -v ] && mount="${a%%:*}"; prev="$a"; done
printf "mount holds: %s | %s\n" "$(ls "$mount" 2>&1 | tr "\n" " ")" "$(cat "$mount/repofile" 2>&1)" >> "$d/rt.mounts"
case "$cmd" in
    *"/dev/tcp/1.1.1.1/53"*) key=tcp ;;
    *"getent hosts"*) key=dns ;;
    *"echo pwned > /etc/passwd"*) key=etc ;;
    *"chmod +x /tmp/x"*) key=exec ;;
    *"capsh --print"*) key=caps ;;
    *"grep NoNewPrivs"*) key=nnp ;;
    *"sleep 30 &"*) key=fork ;;
    *"head -c 4G"*) key=balloon ;;
    *"while :; do :; done"*) key=loop ;;
    *"cat /proc/1/environ"*) key=creds ;;
    *"/repo/../escape"*) key=escape; [ -e "$d/rt.escape" ] && : > "$TMPDIR/escape" ;;
    *) key=other ;;
esac
[ -e "$d/rt/$key.out" ] && cat "$d/rt/$key.out"
[ -e "$d/rt/$key.err" ] && cat "$d/rt/$key.err" >&2
rc=0; [ -e "$d/rt/$key.rc" ] && rc="$(cat "$d/rt/$key.rc")"
exit "$rc"
'''
FLAGS = ["--network=none", "--cap-drop=ALL", "no-new-privileges", "--read-only", "noexec", "--pids-limit", "--memory", "--cpus"]
ATTACK_OUT = {
    "tcp": ["rc=1\n", "rc=0\n", "", "connected? rc=0 label\n", "rc=10\n", "rc=124\n", "rc=\0" + "0\n", "r\0c=0\n"],
    "dns": ["rc=2\n", "rc=0\n", ""],
    "etc": ["rc=1\n", "rc=0\n", ""],
    "exec": ["rc=126\n", "ran\nrc=0\n", "sh: never ran\nrc=1\n", " ran\n", ""],
    "caps": ["CapEff:\t0000000000000000\n", "CapEff:\t00000000a80425fb\n", "CapEff: 000 \n", "Current: =\n", "", "x CapEff: 0\n", "CapEff:\n" + "y" * 80 + "\n"],
    "nnp": ["NoNewPrivs:\t1\n", "NoNewPrivs:\t0\n", "", "NoNewPrivs:\t0 (see 1)\n"],
    "fork": ["spawned=500\n", "spawned=999\n", "spawned=1000\n", "spawned=4000\n", "", "spawned=\n", "spawned=3000\nspawned=12\n", "junk\nspawned=7\nmore\n", "spawned=0999\n", "spawned=99999999999999999999\n"],
    "balloon": ["rc=1\n", "rc=0\n", ""],
    "loop": ["", "rc=0\nspawned=5000\n"],
    "creds": ["PATH=/usr/bin\n", "GH_TOKEN=ghp_PROBECANARY0000\n", ""],
    "escape": ["rc=1\n", ""],
}


def probe_case(i, seed):
    def build(sb, tree_src):
        rnd = random.Random("probe-%s-%d" % (seed, i))
        prog = copy_program(sb, tree_src, "tests/containment-probe.sh")
        kept = [f for f in FLAGS if rnd.random() < 0.85]
        gate = "\n".join(kept)
        form = rnd.choice(["flags", "flags", "flags", "comment", "absent", "dir"])
        if form == "flags":
            sb.write("bin/maintainer-merge", gate + "\n")
        elif form == "comment":
            sb.write("bin/maintainer-merge", "# " + " ".join(FLAGS) + "\n")
        elif form == "dir":
            os.makedirs(os.path.join(sb.tree, "bin/maintainer-merge"))
        r = rnd.random()
        if r < 0.78:
            sb.stub("podman", POD_STUB)
            sb.fail_unplanned("docker")
        elif r < 0.88:
            sb.stub("docker", POD_STUB)
        elif r < 0.94:
            sb.stub("podman", POD_STUB)
            sb.stub("docker", POD_STUB)
        sb.fail_unplanned("gh")
        for key, outs in ATTACK_OUT.items():
            if rnd.random() < 0.5:
                sb.write("rt/%s.out" % key, rnd.choice(outs), base=sb.path)
            elif rnd.random() < 0.2:
                sb.write("rt/%s.err" % key, rnd.choice(outs), base=sb.path)
            if rnd.random() < 0.1:
                sb.write("rt/%s.rc" % key, rnd.choice(["0", "1", "125"]), base=sb.path)
        if rnd.random() < 0.15:
            sb.write("rt.ps.before", str(rnd.randint(0, 3)), base=sb.path)
            sb.write("rt.ps.after", str(rnd.randint(0, 4)), base=sb.path)
        if rnd.random() < 0.1:
            sb.write("rt.inspect.rc", "1", base=sb.path)
            if rnd.random() < 0.5:
                sb.write("rt.pull.rc", "1", base=sb.path)
        if rnd.random() < 0.1:
            open(os.path.join(sb.path, "rt.escape"), "w").close()
        sb.stub("sleep", "exit 0\n")
        env = {}
        if rnd.random() < 0.2:
            env["PROBE_IMAGE"] = rnd.choice(["example.invalid/x:1", "docker.io/library/bash:4"])
        if rnd.random() < 0.2:
            env["PROBE_TIMEOUT"] = rnd.choice(["7", "30"])
        if rnd.random() < 0.2:
            env["GH_TOKEN"] = "ghp_AMBIENT"
            env["ANTHROPIC_API_KEY"] = "sk-AMBIENT"
        spec = Spec(prog, env=env)
        spec.rust_name = "containment-probe"
        return spec

    return Case("probe-%03d" % i, build)


# --- the recorded differences --------------------------------------------------------------------------------------

def explained_cases(seed):
    """Cases whose two sides differ on purpose. Each is (name, build, bash_check, rust_check)."""
    cases = []

    def sweep_bad_poll(poll, wait):
        def build(sb, tree_src):
            prog = copy_program(sb, tree_src, "scripts/shellcheck-sweep.sh")
            env = {}
            subprocess.run(["git", "init", "-q", "-b", "main"], cwd=sb.tree, check=True)
            subprocess.run(["git", "-c", "user.name=f", "-c", "user.email=f@x.invalid", "commit", "-q", "-m", "b", "--allow-empty"], cwd=sb.tree, check=True)
            sb.write("a.sh", "#!/bin/bash\n")
            subprocess.run(["git", "add", "-A"], cwd=sb.tree, check=True)
            sb.stub("shellcheck", "exit 0\n")
            sb.stub("sleep", "exit 0\n")
            sb.write("meminfo", "MemAvailable: 1 kB\n", base=sb.path)
            env.update({"MAGENT_MEMINFO": os.path.join(sb.path, "meminfo"), "MAGENT_SWEEP_POLL": poll, "MAGENT_SWEEP_WAIT": wait})
            spec = Spec(prog, env=env, timeout=4)
            spec.rust_name = "shellcheck-sweep"
            return spec
        return build

    for poll, wait in (("abc", "5"), ("08", "5"), ("1", "x")):
        cases.append(("explained-sweep-poll-%s-wait-%s" % (poll, wait), sweep_bad_poll(poll, wait),
                      lambda r: r["rc"] in ("0", "1", "124"),
                      lambda r: r["rc"] == "75" and "must be whole numbers of seconds" in r["stderr"]))
    def sweep_glob(sb, tree_src):
        prog = copy_program(sb, tree_src, "scripts/shellcheck-sweep.sh")
        subprocess.run(["git", "init", "-q", "-b", "main"], cwd=sb.tree, check=True)
        sb.write("g*.sh", "#!/bin/bash\n")
        subprocess.run(["git", "add", "-A"], cwd=sb.tree, check=True)
        subprocess.run(["git", "-c", "user.name=f", "-c", "user.email=f@x.invalid", "commit", "-q", "-m", "b"], cwd=sb.tree, check=True)
        sb.write("gx.sh", "#!/bin/bash\n")  # untracked: only the shell's pattern expansion reaches it
        sb.stub("shellcheck", "exit 0\n")
        sb.write("meminfo", "MemAvailable: 20000000 kB\n", base=sb.path)
        spec = Spec(prog, env={"MAGENT_MEMINFO": os.path.join(sb.path, "meminfo"), "CI": "1"})
        spec.rust_name = "shellcheck-sweep"
        return spec

    cases.append(("explained-sweep-glob-expansion", sweep_glob,
                  lambda r: "shellcheck --severity=warning gx.sh" in r["calls"] and "checking 3 scripts" in r["stdout"],
                  lambda r: "gx.sh" not in r["calls"] and "checking 2 scripts" in r["stdout"]))

    # P4B-16: a repository or tag taken from workflow text steers the path of `gh api`. The bash sends it
    # as written (the stub answers that path, so the pin verifies); the port refuses before any call.
    # A `#` cannot reach a tag (the claim is the text after the last `#`), so it has no case here.
    def pins_hostile(action, tag, ask_paths):
        def build(sb, tree_src):
            prog = copy_program(sb, tree_src, "scripts/verify-action-pins.sh")
            sb.stub("gh", GH_STUB)
            sb.fail_unplanned("podman", "docker")
            sha = SHAS[0]
            sb.write(".github/workflows/w.yml", b"      - uses: " + action + b"@" + sha.encode() + b" # " + tag + b"\n")
            with open(os.path.join(sb.path, "answers"), "w") as f:
                for path in ask_paths:
                    f.write("%s\x1f.object.sha\x1f0\x1f%s\n" % (path, sha))
                    f.write("%s\x1f.object.type\x1f0\x1fcommit\n" % path)
            spec = Spec(prog)
            spec.rust_name = "verify-action-pins"
            return spec
        return build

    hostile = [
        ("repo-dotdot", b"o/..", b"v1", ["repos/o/../git/ref/tags/v1"]),
        ("repo-dots", b"../..", b"v1", ["repos/../../git/ref/tags/v1"]),
        ("repo-percent", b"o/r%2e", b"v1", ["repos/o/r%2e/git/ref/tags/v1"]),
        ("tag-query", b"o/r", b"v1?x=1", ["repos/o/r/git/ref/tags/v1?x=1"]),
        ("tag-percent", b"o/r", b"v%41", ["repos/o/r/git/ref/tags/v%41"]),
        ("tag-dotdot-encoded", b"o/r", b"..%2f..%2fuser", ["repos/o/r/git/ref/tags/..%2f..%2fuser"]),
        ("tag-dotdot", b"o/r", b"a/../b", ["repos/o/r/git/ref/tags/a/../b"]),
        ("tag-dot-segment", b"o/r", b"a/./b", ["repos/o/r/git/ref/tags/a/./b"]),
        ("tag-dot", b"o/r", b".", ["repos/o/r/git/ref/tags/."]),
        ("tag-leading-slash", b"o/r", b"/v1", ["repos/o/r/git/ref/tags//v1"]),
        ("tag-control", b"o/r", b"v1\x01", ["repos/o/r/git/ref/tags/v1\x01"]),
    ]
    for label, action, tag, paths in hostile:
        cases.append(("explained-pins-hostile-%s" % label, pins_hostile(action, tag, paths),
                      lambda r: "gh api " in r["calls"] and r["rc"] == "0" and "cannot be checked" not in r["stdout"],
                      lambda r: "gh api" not in r["calls"] and "cannot be checked" in r["stdout"] and r["rc"] == "1"))

    # P4B-16: a tag with a byte a URL path does not take is percent-encoded; the pin still verifies.
    def pins_encoded():
        raw, enc = "repos/o/r/git/ref/tags/v1{x}", "repos/o/r/git/ref/tags/v1%7Bx%7D"
        return pins_hostile(b"o/r", b"v1{x}", [raw, enc])

    cases.append(("explained-pins-tag-encoded", pins_encoded(),
                  lambda r: "gh api repos/o/r/git/ref/tags/v1\\{x\\} " in r["calls"] and r["rc"] == "0",
                  lambda r: "gh api repos/o/r/git/ref/tags/v1%7Bx%7D " in r["calls"] and "v1\\{x" not in r["calls"] and r["rc"] == "0"))
    return cases


def compare(case, bash, rust):
    diffs = []
    for key in ("rc", "stdout", "stderr", "calls", "state"):
        if bash[key] != rust[key]:
            diffs.append(key)
    return diffs


def show(case, bash, rust, diffs):
    lines = ["DIFFERENCE in %s (%s)" % (case.name, ", ".join(diffs))]
    for key in diffs:
        b, r = bash[key].splitlines(), rust[key].splitlines()
        import difflib
        d = list(difflib.unified_diff(b, r, "bash", "rust", lineterm="", n=1))
        lines.append("  -- %s:" % key)
        lines += ["     " + x for x in d[:14]]
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rust-bin-dir", required=True)
    ap.add_argument("--tree", default=ROOT)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("--only", default="")
    ap.add_argument("--scratch", default=os.path.expanduser("~/.cache/magent-wt-scratch/p4b-diff"))
    ap.add_argument("--per-program", type=int, default=70)
    ap.add_argument("-v", action="store_true")
    a = ap.parse_args()
    seed = a.seed if a.seed is not None else int(subprocess.check_output(["date", "+%s"]).decode()) % 1000000
    print("differential: seed %d (repeat with --seed %d)" % (seed, seed))
    for name in ("check-claims", "shellcheck-sweep", "verify-action-pins", "run-evals", "containment-probe"):
        if not os.access(os.path.join(a.rust_bin_dir, name), os.X_OK):
            sys.exit("differential: no executable %s in %s" % (name, a.rust_bin_dir))
    cases = []
    for gen in (claims_case, pins_case, sweep_case, evals_case, probe_case):
        cases += [gen(i, seed) for i in range(a.per_program)]
    only = [x for x in a.only.split(",") if x]
    if only:
        cases = [c for c in cases if any(o in c.name for o in only)]
    os.makedirs(a.scratch, exist_ok=True)
    path = os.path.join(a.scratch, "sandbox")
    matched, bad = 0, []
    for c in cases:
        bash = run_once(c, path, "bash", a.rust_bin_dir, a.tree)
        rust = run_once(c, path, "rust", a.rust_bin_dir, a.tree)
        diffs = compare(c, bash, rust)
        if diffs:
            bad.append(show(c, bash, rust, diffs))
        else:
            matched += 1
            if a.v:
                print("match  %s (rc %s)" % (c.name, bash["rc"]))
    explained = 0
    for name, build, bash_ok, rust_ok in explained_cases(seed):
        if only and not any(o in name for o in only):
            continue
        case = Case(name, build)
        bash = run_once(case, path, "bash", a.rust_bin_dir, a.tree)
        rust = run_once(case, path, "rust", a.rust_bin_dir, a.tree)
        if bash_ok(bash) and rust_ok(rust):
            explained += 1
            if a.v:
                print("explained %s: bash rc %s, rust rc %s" % (name, bash["rc"], rust["rc"]))
        else:
            bad.append("EXPLAINED CASE %s no longer shows its recorded difference: bash rc %s stderr %r / rust rc %s stderr %r" % (name, bash["rc"], bash["stderr"][:120], rust["rc"], rust["stderr"][:120]))
    shutil.rmtree(path, ignore_errors=True)
    total = matched + explained + len(bad)
    print("differential: %d comparisons, %d matched exactly, %d explained, %d unexplained" % (total, matched, explained, len(bad)))
    for b in bad[:12]:
        print(b)
    return 1 if bad or total == 0 else 0


if __name__ == "__main__":
    sys.exit(main())
