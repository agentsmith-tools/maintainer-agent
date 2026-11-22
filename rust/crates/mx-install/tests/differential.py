#!/usr/bin/env python3
"""Differential driver: bash scripts against the Rust programs. See differential.sh.

Run through differential.sh; arguments are BASH_TREE RUST_TREE WORK SEED.
"""

import hashlib
import os
import random
import re
import shutil
import stat
import subprocess
import sys
from pathlib import Path

BASH_TREE, RUST_TREE, WORK, SEED = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3]), int(sys.argv[4])
ONLY = [x for x in os.environ.get("DIFF_ONLY", "").split(",") if x]
RANDOM_COUNT = int(os.environ.get("DIFF_RANDOM", "330"))
VERBOSE = os.environ.get("DIFF_VERBOSE") == "1"
REAL_HOME = Path(os.environ["HOME"])

TOOL_NAMES = ("bash cp rm mkdir mv sed grep awk cat chmod touch find sort tr od head tail dirname basename readlink "
              "git python3 date uname id env flock timeout ln ls cut wc stat sha256sum mktemp tee diff cmp sleep true "
              "false xargs").split()
TREE_FILES = ["install.sh", "new-profile.sh", "lib", "scripts", "bin", "profiles", "platform", "evals"]

LOG = r'''d="${CZ_CASE_DIR:?}"
{ printf '%s' "$CZ_STUB"; for a in "$@"; do printf ' %q' "$a"; done
  printf ' | cwd=%s | env=' "$PWD"
  env | grep -E '^(HOME|LC_ALL|TZ|DISPLAY|DBUS_SESSION_BUS_ADDRESS|USER|LOGNAME)=' | LC_ALL=C sort | tr '\n' ' '
  printf '| names='
  env | cut -d= -f1 | grep -vE '^(_|SHLVL|PWD|OLDPWD|BASH_ENV|MAINTAINER_[A-Z_]*)$' | LC_ALL=C sort | tr '\n' ' '
  printf '\n'; } >> "$d/calls.log"
'''
STUBS = {
    "systemctl": '''CZ_STUB=systemctl
''' + LOG + '''verb="${2:-}"
if [ "$verb" = enable ]; then
    n="${*: -1}"
    if [ -e "$HOME/.local/share/systemd/timers/stamp-$n" ]; then s=yes; else s=no; fi
    printf 'stamp %s=%s\\n' "$n" "$s" >> "$d/calls.log"
fi
case "$verb" in
    show-environment) [ -e "$d/systemctl.noenv" ] && exit 1; exit 0 ;;
    list-timers) [ -e "$d/systemctl.timers" ] && cat "$d/systemctl.timers"; exit 0 ;;
esac
if [ -e "$d/systemctl.fail.$verb" ]; then echo "stub systemctl: $verb failed" >&2; exit "$(cat "$d/systemctl.fail.$verb")"; fi
exit 0
''',
    "crontab": '''CZ_STUB=crontab
''' + LOG + '''case "${1:-}" in
    -l) if [ -e "$d/crontab.lerr" ]; then cat "$d/crontab.lerr" >&2; exit 1; fi
        if [ -e "$d/crontab.store" ]; then cat "$d/crontab.store"; exit 0; fi
        echo "no crontab for fixture-user" >&2; exit 1 ;;
    -)  cat > "$d/crontab.in"
        if [ -e "$d/crontab.w.rc" ]; then echo "stub crontab: cannot write" >&2; exit "$(cat "$d/crontab.w.rc")"; fi
        cp "$d/crontab.in" "$d/crontab.store" ;;
    *)  echo "stub crontab: unexpected argv: $*" >&2; exit 2 ;;
esac
''',
    "launchctl": '''CZ_STUB=launchctl
''' + LOG + '''if [ -e "$d/launchctl.fail.${1:-}" ]; then echo "stub launchctl: ${1:-} failed" >&2; exit 1; fi
exit 0
''',
    "flock": '''CZ_STUB=flock
''' + LOG + '''rc=0; [ -e "$d/flock.rc" ] && rc="$(cat "$d/flock.rc")"
exit "$rc"
''',
    "notify-send": '''CZ_STUB=notify-send
''' + LOG + '''[ -e "$d/notify.rc" ] && exit "$(cat "$d/notify.rc")"
exit 0
''',
    "date": '''if [ "$*" = "-Is" ]; then echo "2026-01-01T00:00:00+00:00"; exit 0; fi
exec "${CZ_REAL_DATE:?}" "$@"
''',
    "uname": '''if [ "$*" = "-s" ] && [ -e "${CZ_CASE_DIR:?}/uname.out" ]; then cat "$CZ_CASE_DIR/uname.out"; exit 0; fi
exec "${CZ_REAL_UNAME:?}" "$@"
''',
}
RUNSH_STUB = '''#!/usr/bin/env bash
printf '%s\\n' "$#:$*" >> "${CZ_CASE_DIR:?}/run.calls"
exit "${RUN_RC:-0}"
'''


def real(name):
    out = subprocess.run(["bash", "-c", "type -P " + name], capture_output=True, text=True).stdout.strip()
    if not out:
        sys.exit("differential: the host has no '%s'" % name)
    return out


REAL_TOOLS = {n: real(n) for n in TOOL_NAMES}


def sha(data):
    return hashlib.sha256(data).hexdigest()


class Side:
    """One copy of everything a program touches: tree, home, stubs, tools."""

    def __init__(self, base, which, tree_src):
        self.which = which
        self.case = base / which
        self.home = self.case / "home"
        self.tree = self.case / "tree"
        self.bin = self.case / "bin"
        self.tools = self.case / "tools"
        self.case.mkdir(parents=True)
        self.home.mkdir()
        self.tree.mkdir()
        for name in TREE_FILES:
            src = tree_src / name
            if src.is_dir():
                shutil.copytree(src, self.tree / name, symlinks=True)
            else:
                shutil.copy2(src, self.tree / name, follow_symlinks=False)
        (self.tree / "tests").mkdir()
        shutil.copy2(BASH_TREE / "tests/containment-probe.sh", self.tree / "tests/containment-probe.sh")
        # The bash installer, kept for the setups that need an existing deployment.
        shutil.copy2(BASH_TREE / "install.sh", self.tree / ".bash-install.sh")
        self.bin.mkdir()
        self.tools.mkdir()
        for name, path in REAL_TOOLS.items():
            (self.tools / name).symlink_to(path)
        for name, text in STUBS.items():
            p = self.bin / name
            p.write_text("#!/usr/bin/env bash\n" + text)
            p.chmod(0o755)
        for name in ["podman", "docker", "curl", "wget", "ssh", "gh", "claude", "codex"]:
            p = self.bin / name
            p.write_text('#!/usr/bin/env bash\necho "$0 $*" >> "${CZ_CASE_DIR:?}/unplanned-tools"\nexit 99\n')
            p.chmod(0o755)

    def env(self, extra):
        e = {
            "PATH": "%s:%s" % (self.bin, self.tools),
            "HOME": str(self.home),
            "LC_ALL": "C",
            "TZ": "UTC",
            "LOGNAME": "fixture-user",
            "USER": "fixture-user",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_TERMINAL_PROMPT": "0",
            "CZ_CASE_DIR": str(self.case),
            "CZ_REAL_DATE": REAL_TOOLS["date"],
            "CZ_REAL_UNAME": REAL_TOOLS["uname"],
            "MAINTAINER_CHILD_ENV": "CZ_REAL_DATE CZ_REAL_UNAME",
        }
        e.update(extra)
        return e

    def run(self, argv, extra_env, umask=0o022, timeout=120):
        env = self.env(extra_env)
        cmd = ["bash", "-c", 'umask "$1"; shift; exec env -i "$@"', "bash", "%04o" % umask]
        cmd += ["%s=%s" % kv for kv in env.items()] + ["timeout", str(timeout), "bash"] + [str(a) for a in argv]
        proc = subprocess.run(cmd, cwd=self.home, stdin=subprocess.DEVNULL, capture_output=True)
        return proc.returncode, proc.stdout, proc.stderr

    def norm_bytes(self, data):
        return data.replace(str(self.case).encode(), b"@CASE@")

    def calls(self):
        p = self.case / "calls.log"
        return self.norm_bytes(p.read_bytes()).decode("utf-8", "replace") if p.exists() else ""

    def snapshot(self, root):
        rows = []
        for path in sorted(root.rglob("*")):
            rel = path.relative_to(root).as_posix()
            if rel.startswith(".bash-install") or "__pycache__" in rel or rel == ".git" or rel.startswith(".git/"):
                continue
            st = path.lstat()
            if rel in WRAPPER_PATHS or (rel.endswith(("/alert.sh", "/run-instance.sh")) and not rel.startswith("platform/")):
                # The Rust tree holds a wrapper where the bash tree holds the script, and a
                # deployed copy of either: only whether the file is executable can agree.
                rows.append("%s F wrapper x=%s" % (rel, bool(st.st_mode & 0o111)))
            elif stat.S_ISLNK(st.st_mode):
                rows.append("%s L -> %s" % (rel, self.norm_bytes(os.readlink(path).encode()).decode()))
            elif stat.S_ISDIR(st.st_mode):
                rows.append("%s D %o" % (rel, st.st_mode & 0o7777))
            else:
                data = path.read_bytes()
                if rel.endswith("receipt.key") and re.fullmatch(rb"[0-9a-f]{64}", data):
                    digest = "<key>"
                else:
                    digest = sha(self.norm_bytes(data))
                rows.append("%s F %o %d %s" % (rel, st.st_mode & 0o7777, len(data), digest))
        return rows

    def case_files(self):
        out = {}
        for name in ["crontab.store", "crontab.in", "run.calls", "unplanned-tools"]:
            p = self.case / name
            out[name] = self.norm_bytes(p.read_bytes()).decode("utf-8", "replace") if p.exists() else None
        return out


# ---------------------------------------------------------------------------------------
# fixtures

PROFILE_ENV = {
    "sysknife": ('PROFILE_NAME="sysknife"\nREPO_PATH="${MAINTAINER_REPO_PATH:-$HOME/.local/state/sysknife-maint/checkout}"\n'
                 'STATE_DIR="${MAINTAINER_STATE_DIR:-$HOME/.local/state/sysknife-maint}"\nTASKS="review issues ci audit"\n'
                 'MIN_HOURS_review=6\nMIN_HOURS_issues=36\nMIN_HOURS_ci=60\nMIN_HOURS_audit=108\n'),
    "magent": ('PROFILE_NAME="magent"\nREPO_PATH="${MAINTAINER_REPO_PATH:-$HOME/.local/state/magent-maint/checkout}"\n'
               'STATE_DIR="${MAINTAINER_STATE_DIR:-$HOME/.local/state/magent-maint}"\nTASKS="review issues"\n'
               'MIN_HOURS_review=6\nMIN_HOURS_issues=36\n'),
}
DENY = '{"spelled_everywhere": ["cargo publish", "maintainer-merge receipt"], "credential_paths": ["~/.ssh/**"]}\n'
TEMPLATE = {
    "profile.env": ('PROFILE_NAME="__PROFILE__"\nREPO_PATH="${MAINTAINER_REPO_PATH:-__REPO_PATH__}"\nREPO_SLUG="__SLUG__"\n'
                    'MAINTAINER_NAME="__MAINTAINER_NAME__"\nGH_ACCOUNT="__GH_ACCOUNT__"\n'
                    'STATE_DIR="${MAINTAINER_STATE_DIR:-$HOME/.local/state/__PROFILE__-maint}"\nTASKS="review issues"\n'
                    'MIN_HOURS_review=6\nMIN_HOURS_issues=36\n'),
    "deny.json": '{"spelled_everywhere": ["gh pr merge", "__PROFILE__ publish"], "credential_paths": ["~/.ssh/**"]}\n',
    "prompts/review.md": 'Review __SLUG__ as __MAINTAINER_NAME__ (__GH_ACCOUNT__), twice: __SLUG__.\n',
    "notes.txt": 'nothing to substitute here\n',
}


def fixture_profiles(side, names=("sysknife", "magent")):
    p = side.tree / "profiles"
    shutil.rmtree(p)
    for name in names:
        (p / name).mkdir(parents=True)
        (p / name / "deny.json").write_text(DENY)
        (p / name / "profile.env").write_text(PROFILE_ENV[name])
    if "sysknife" in names:
        (p / "sysknife/prompts").mkdir()
        (p / "sysknife/prompts/review.md").write_text("sysknife prompt\n")
    for rel, text in TEMPLATE.items():
        f = p / "_template" / rel
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_text(text)


def write(path, text, mode=None):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    if mode is not None:
        path.chmod(mode)


def deploy_bash(side, args=()):
    """A real install by the bash script into this side's home (stubs and all)."""
    rc, out, err = side.run([side.tree / ".bash-install.sh", *args], {})
    if rc != 0:
        sys.exit("differential: the setup install exited %d: %s" % (rc, err[:300]))
    for p in side.case.glob("*.calls"):
        p.unlink()
    for name in ("calls.log",):
        (side.case / name).unlink(missing_ok=True)


class Case:
    def __init__(self, name, prog, args=(), env=None, setups=(), umask=0o022, check=None):
        self.name, self.prog, self.args, self.env = name, prog, list(args), env or {}
        self.setups, self.umask, self.check = list(setups), umask, check


SCRIPT = {
    "install": "install.sh", "new-profile": "new-profile.sh", "cron": "platform/posix/install-cron.sh",
    "launchd": "platform/macos/install-launchd.sh", "alert": "platform/linux/alert.sh",
    "instance": "inst/run-instance.sh",
}

WRAPPER_PATHS = {
    "install.sh", "new-profile.sh", "platform/posix/install-cron.sh", "platform/macos/install-launchd.sh",
    "platform/linux/alert.sh", "platform/linux/run-instance.sh", "inst/run-instance.sh",
    ".local/share/maintainer/alert.sh", ".local/share/maintainer/run-instance.sh",
}

TOOL_DIAG = re.compile(r"^mkdir: .*$", re.M)


def norm_text(side, data):
    text = side.norm_bytes(data).decode("utf-8", "replace")
    return TOOL_DIAG.sub("mkdir: <diagnostic>", text)


def run_case(case, idx):
    base = WORK / ("%03d" % idx)
    sides = {"bash": Side(base, "bash", BASH_TREE), "rust": Side(base, "rust", RUST_TREE)}
    results = {}
    for which, side in sides.items():
        fixture_profiles(side)
        for setup in case.setups:
            setup(side)
        script = SCRIPT[case.prog]
        rc, out, err = side.run([side.tree / script, *case.args], case.env, case.umask)
        results[which] = {
            "rc": rc, "stdout": norm_text(side, out), "stderr": norm_text(side, err), "calls": side.calls(),
            "home": side.snapshot(side.home), "tree": side.snapshot(side.tree), "files": side.case_files(),
        }
    return results


def first_diff(a, b):
    if isinstance(a, str):
        al, bl = a.splitlines(), b.splitlines()
    else:
        al, bl = list(a), list(b)
    for i in range(max(len(al), len(bl))):
        x = al[i] if i < len(al) else "<end>"
        y = bl[i] if i < len(bl) else "<end>"
        if x != y:
            if len(str(x)) > 200 or len(str(y)) > 200:
                x, y = str(x), str(y)
                at = next((k for k in range(min(len(x), len(y))) if x[k] != y[k]), min(len(x), len(y)))
                return "line %d, char %d: bash ...%r | rust ...%r" % (i + 1, at, x[max(0, at - 60):at + 80], y[max(0, at - 60):at + 80])
            return "line %d: bash %r | rust %r" % (i + 1, x, y)
    return "(equal)"


FIELDS = ["rc", "stdout", "stderr", "calls", "home", "tree", "files"]


def compare(case, results):
    problems = []
    for field in FIELDS:
        a, b = results["bash"][field], results["rust"][field]
        if a != b:
            problems.append("%s: %s" % (field, first_diff(a, b) if field != "rc" else "bash %s | rust %s" % (a, b)))
    return problems


# ---------------------------------------------------------------------------------------
# setups

def s_deploy(*args):
    return lambda side: deploy_bash(side, args)


def s_file(rel, text, mode=None, under="home"):
    return lambda side: write(getattr(side, under) / rel, text, mode)


def s_case(name, text=""):
    return lambda side: write(side.case / name, text)


def s_uname(name):
    return s_case("uname.out", name + "\n")


def s_drop(*tools):
    def go(side):
        for t in tools:
            (side.tools / t).unlink(missing_ok=True)
            (side.bin / t).unlink(missing_ok=True)
    return go


def s_lock(profile="sysknife"):
    return s_file(".local/state/%s-maint/run.lock" % profile, "")


def s_rm_tree(rel):
    def go(side):
        p = side.tree / rel
        if p.is_dir():
            shutil.rmtree(p)
        else:
            p.unlink(missing_ok=True)
    return go


def s_tree_file(rel, text, mode=None):
    return s_file(rel, text, mode, under="tree")


def s_chmod(rel, mode, under="home"):
    return lambda side: os.chmod(getattr(side, under) / rel, mode)


def s_symlink(rel, target, under="home"):
    def go(side):
        p = getattr(side, under) / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        if p.exists() or p.is_symlink():
            p.unlink()
        t = target if os.path.isabs(target) else str(side.home / target)
        p.symlink_to(t)
    return go


def s_mkdir(rel, mode=None, under="home"):
    def go(side):
        p = getattr(side, under) / rel
        p.mkdir(parents=True, exist_ok=True)
        if mode is not None:
            p.chmod(mode)
    return go


def s_profiles(*names):
    return lambda side: fixture_profiles(side, names)


def s_inst(side):
    """inst/ holds run-instance.sh (the file under test) beside a run.sh that logs its argv."""
    rel = SCRIPT["instance"]
    src = BASH_TREE / "platform/linux/run-instance.sh"
    dst = side.tree / rel
    dst.parent.mkdir(parents=True, exist_ok=True)
    if side.which == "bash":
        shutil.copy2(src, dst)
    else:
        shutil.copy2(RUST_TREE / "platform/linux/run-instance.sh", dst)
    write(side.tree / "inst/run.sh", RUNSH_STUB, 0o755)


def s_state(profile):
    return s_mkdir(".local/state/%s-maint" % profile)


# ---------------------------------------------------------------------------------------
# the named scenarios

def named():
    c = []
    A = c.append
    for args in (["--bogus"], ["--dry-run", "--bogus"], ["install"], ["--help"], []):
        A(Case("install flags %s" % args, "install", args))
    A(Case("install dry run", "install", ["--dry-run"]))
    A(Case("install dry run timers", "install", ["--dry-run", "--timers"]))
    A(Case("install dry run timers reversed", "install", ["--timers", "--dry-run"]))
    A(Case("install plain", "install"))
    A(Case("install timers", "install", ["--timers"]))
    A(Case("install twice", "install", [], setups=[s_deploy()]))
    A(Case("install over an existing deployment with timers", "install", ["--timers"], setups=[s_deploy("--timers")]))
    A(Case("install real profiles tree", "install", [], setups=[lambda side: shutil.rmtree(side.tree / "profiles") or shutil.copytree(BASH_TREE / "profiles", side.tree / "profiles")]))
    A(Case("install one profile magent", "install", [], setups=[s_profiles("magent")]))
    A(Case("install one profile sysknife", "install", [], setups=[s_profiles("sysknife")]))
    A(Case("install no profile", "install", [], setups=[s_profiles()]))
    A(Case("install dry run no profile", "install", ["--dry-run"], setups=[s_profiles()]))
    A(Case("install profile dir without env", "install", [], setups=[s_rm_tree("profiles/magent/profile.env"), s_rm_tree("profiles/sysknife")]))
    A(Case("install profile without deny json", "install", [], setups=[s_rm_tree("profiles/magent/deny.json")]))
    A(Case("install busy", "install", [], setups=[s_lock(), s_case("flock.rc", "1")]))
    A(Case("install busy two", "install", [], setups=[s_lock(), s_lock("magent"), s_case("flock.rc", "1")]))
    A(Case("install busy over deployment", "install", [], setups=[s_deploy(), s_lock(), s_case("flock.rc", "1")]))
    A(Case("install busy force", "install", ["--force"], setups=[s_lock(), s_case("flock.rc", "1")]))
    A(Case("install busy dry run", "install", ["--dry-run"], setups=[s_lock(), s_case("flock.rc", "1")]))
    A(Case("install free lock", "install", [], setups=[s_lock()]))
    A(Case("install state dir override", "install", [], env={"MAINTAINER_STATE_DIR": "@HOME@/elsewhere"}, setups=[]))
    A(Case("install without flock", "install", [], setups=[s_lock(), s_drop("flock")]))
    A(Case("install without git", "install", [], setups=[s_drop("git")]))
    A(Case("install without python", "install", [], setups=[s_drop("python3")]))
    A(Case("install dry run without python od git", "install", ["--dry-run"], setups=[s_drop("python3", "od", "git")]))
    A(Case("install timers without systemctl", "install", ["--timers"], setups=[s_drop("systemctl")]))
    A(Case("install timers enable fails", "install", ["--timers"], setups=[s_case("systemctl.fail.enable", "1")]))
    A(Case("install timers reload fails", "install", ["--timers"], setups=[s_case("systemctl.fail.daemon-reload", "4")]))
    A(Case("install timers none found", "install", ["--timers"], setups=[lambda side: [p.unlink() for p in (side.tree / "platform/linux").glob("*.timer")]]))
    A(Case("install timer for a missing profile", "install", ["--timers"], setups=[s_tree_file("platform/linux/maintainer@gone-ci.timer", "[Timer]\n")]))
    A(Case("install timer with no hyphen", "install", ["--timers"], setups=[s_tree_file("platform/linux/maintainer@solo.timer", "[Timer]\n")]))
    for u in ("Darwin", "MINGW64_NT-10.0-19045", "MSYS_NT-10.0", "CYGWIN_NT-10.0", "FreeBSD", "Linux", ""):
        A(Case("install uname %r timers" % u, "install", ["--timers"], setups=[s_uname(u)]))
        A(Case("install uname %r dry run" % u, "install", ["--dry-run"], setups=[s_uname(u)]))
    A(Case("install new script", "install", [], setups=[s_tree_file("scripts/zz_new.py", "print(1)\n")]))
    A(Case("install no scripts", "install", [], setups=[s_rm_tree("scripts/check_drift.py"), lambda side: [p.unlink() for p in (side.tree / "scripts").glob("*.py")]]))
    A(Case("install wall fails", "install", [], setups=[s_tree_file("scripts/render-settings.py", "import sys\nsys.exit(3)\n")]))
    A(Case("install existing key kept", "install", [], setups=[s_file(".local/share/maintainer/receipt.key", "existing-key-material", 0o640)]))
    A(Case("install existing key empty dir", "install", [], setups=[s_mkdir(".local/share/maintainer/receipt.key")]))
    A(Case("install stale files", "install", [], setups=[s_deploy(), s_file(".local/share/maintainer/profiles/stale.txt", ""), s_file(".local/share/maintainer/backends/stale.sh", "")]))
    A(Case("install umask 077", "install", [], umask=0o077))
    A(Case("install umask 027 timers", "install", ["--timers"], umask=0o027))
    A(Case("install symlinked command", "install", [], setups=[s_file("elsewhere", "old"), s_symlink(".local/bin/maintainer", "elsewhere")]))
    A(Case("install dangling symlinked command", "install", [], setups=[s_symlink(".local/bin/maintainer", "nowhere")]))
    A(Case("install command is a directory", "install", [], setups=[s_mkdir(".local/bin/maintainer-merge")]))
    A(Case("install symlinked share", "install", [], setups=[s_mkdir("real-share"), s_symlink(".local/share/maintainer", "real-share")]))
    A(Case("install symlinked bin dir", "install", [], setups=[s_mkdir("real-bin"), s_symlink(".local/bin", "real-bin")]))
    A(Case("install unit is a symlink", "install", [], setups=[s_file("elsewhere.service", "x"), s_symlink(".config/systemd/user/maintainer@.service", "elsewhere.service")]))
    A(Case("install read-only bin", "install", [], setups=[s_mkdir(".local/bin", 0o555)]))
    A(Case("install read-only units", "install", [], setups=[s_mkdir(".config/systemd/user", 0o555)]))
    A(Case("install read-only share", "install", [], setups=[s_mkdir(".local/share/maintainer", 0o555)]))
    A(Case("install read-only deployed command", "install", [], setups=[s_deploy(), s_chmod(".local/bin/maintainer", 0o444)]))
    A(Case("install version file read-only", "install", [], setups=[s_deploy(), s_chmod(".local/share/maintainer/VERSION", 0o444)]))
    A(Case("install tagged git tree", "install", [], setups=[git_tree("v1.2.3")]))
    A(Case("install dirty git tree", "install", [], setups=[git_tree("v1.2.3", dirty=True)]))
    A(Case("install untagged git tree", "install", [], setups=[git_tree("")]))
    A(Case("install ahead of tag", "install", [], setups=[git_tree("v1.2.3", ahead=True)]))
    # uninstall
    D = [s_deploy("--timers")]
    A(Case("uninstall dry run", "install", ["--uninstall", "--dry-run"], setups=D))
    A(Case("uninstall guard refuses", "install", ["--uninstall"], setups=D + [s_case("systemctl.timers", "maintainer@a.timer n/a\nmaintainer@b.timer n/a\n")]))
    A(Case("uninstall guard one", "install", ["--uninstall"], setups=D + [s_case("systemctl.timers", "maintainer@a.timer n/a\n")]))
    A(Case("uninstall guard counts lines", "install", ["--uninstall"], setups=D + [s_case("systemctl.timers", "maintainer@a.timer\nsomething else\nmaintainer@b.timer maintainer@c.timer\n")]))
    A(Case("uninstall guard yes", "install", ["--uninstall"], env={"MAINTAINER_UNINSTALL_YES": "1"}, setups=D + [s_case("systemctl.timers", "maintainer@a.timer n/a\n")]))
    for v in ("yes", "true", "0", ""):
        A(Case("uninstall guard value %r" % v, "install", ["--uninstall"], env={"MAINTAINER_UNINSTALL_YES": v}, setups=D + [s_case("systemctl.timers", "maintainer@a.timer n/a\n")]))
    A(Case("uninstall guard no session", "install", ["--uninstall"], setups=D + [s_case("systemctl.timers", "maintainer@a.timer\n"), s_case("systemctl.noenv")]))
    A(Case("uninstall plain", "install", ["--uninstall"], setups=D))
    A(Case("uninstall nothing deployed", "install", ["--uninstall"]))
    A(Case("uninstall no systemctl", "install", ["--uninstall"], setups=D + [s_drop("systemctl")]))
    A(Case("uninstall no crontab", "install", ["--uninstall"], setups=D + [s_drop("crontab")]))
    A(Case("uninstall disable fails", "install", ["--uninstall"], setups=D + [s_case("systemctl.fail.disable", "1")]))
    A(Case("uninstall daemon reload fails", "install", ["--uninstall"], setups=D + [s_case("systemctl.fail.daemon-reload", "4")]))
    A(Case("uninstall cron write fails", "install", ["--uninstall"], setups=D + [s_case("crontab.w.rc", "1")]))
    A(Case("uninstall cron blocks", "install", ["--uninstall"], setups=D + [s_case("crontab.store", "0 1 * * * /usr/bin/backup\n# maintainer-agent (magent)\nx\n# maintainer-agent (magent) end\n")]))
    A(Case("uninstall darwin", "install", ["--uninstall"], setups=D + [s_uname("Darwin"), s_file("Library/LaunchAgents/dev.maintainer.sysknife.review.plist", ""), s_file("Library/LaunchAgents/com.other.plist", "")]))
    A(Case("uninstall darwin dry run", "install", ["--uninstall", "--dry-run"], setups=D + [s_uname("Darwin"), s_file("Library/LaunchAgents/dev.maintainer.sysknife.review.plist", "")]))
    A(Case("uninstall darwin unload fails", "install", ["--uninstall"], setups=D + [s_uname("Darwin"), s_file("Library/LaunchAgents/dev.maintainer.sysknife.review.plist", ""), s_case("launchctl.fail.unload")]))
    A(Case("uninstall keeps audit trail", "install", ["--uninstall"], setups=D + [s_file(".local/state/sysknife-maint/audit.log", "published\n"), s_file(".local/share/systemd/timers/stamp-other.timer", "")]))
    A(Case("uninstall unit globs", "install", ["--uninstall"], setups=D + [s_file(".config/systemd/user/other.service", ""), s_file(".config/systemd/user/mine.timer", ""), s_file(".config/systemd/user/maintainer-custom.service", ""), s_file(".config/systemd/user/maintainer@x-y.timer", "")]))
    A(Case("uninstall dangling unit link", "install", ["--uninstall"], setups=D + [s_symlink(".config/systemd/user/maintainer-dangling.service", "nowhere")]))
    A(Case("uninstall profile without state dir", "install", ["--uninstall"], setups=D + [s_tree_file("profiles/magent/profile.env", 'TASKS="review"\n')]))
    A(Case("uninstall command missing", "install", ["--uninstall"], setups=D + [s_rm_home(".local/bin/maintainer-doctor")]))
    A(Case("uninstall read-only bin", "install", ["--uninstall"], setups=D + [s_chmod(".local/bin", 0o555)]))
    A(Case("uninstall read-only share parent", "install", ["--uninstall"], setups=D + [s_chmod(".local/share", 0o555)]))
    A(Case("uninstall symlinked share", "install", ["--uninstall"], setups=[s_mkdir("real-share"), s_symlink(".local/share/maintainer", "real-share")]))
    A(Case("uninstall timers flag ignored", "install", ["--uninstall", "--timers", "--force"], setups=D))
    A(Case("uninstall dry run with a pipe in the profile state line", "install", ["--uninstall", "--dry-run"], setups=D + [s_tree_file("profiles/magent/profile.env", 'STATE_DIR="$HOME/a|b"\n')]))
    # new-profile
    for args in ([], ["a", "b/c", "/x"]):
        A(Case("new-profile usage %s" % args, "new-profile", args))
    NP = ["alpha", "acme/widgets", "@HOME@/code/widgets", "alice", "Alice Example"]
    A(Case("new-profile alpha", "new-profile", NP, setups=[s_mkdir("code/widgets")]))
    A(Case("new-profile beta", "new-profile", ["beta-2", "org/proj-x", "@CASE@/co", "bob"], setups=[s_mkdir("../co/.git")]))
    A(Case("new-profile real template", "new-profile", NP, setups=[lambda side: (shutil.rmtree(side.tree / "profiles"), shutil.copytree(BASH_TREE / "profiles", side.tree / "profiles", symlinks=True))]))
    A(Case("new-profile home itself", "new-profile", ["alpha", "a/b", "@HOME@", "alice"]))
    A(Case("new-profile missing checkout", "new-profile", ["alpha", "a/b", "/nonexistent/path", "alice"]))
    for bad in ["_x", "_template", "a/b", "", "al|pha"]:
        A(Case("new-profile bad profile %r" % bad, "new-profile", [bad, "a/b", "/srv/x", "acct"]))
    A(Case("new-profile bad slug", "new-profile", ["alpha", "widgets", "/srv/x", "acct"]))
    for i, ch in itertools_product([0, 1, 2, 3, 4], ["|", "&"]):
        args = ["alpha", "acme/widgets", "/srv/x", "acct", "Name"]
        args[i] += ch
        A(Case("new-profile %s in argument %d" % (ch, i), "new-profile", args))
    A(Case("new-profile exists", "new-profile", ["magent", "a/b", "/srv/x", "acct"]))
    A(Case("new-profile exists empty dir", "new-profile", ["alpha", "a/b", "/srv/x", "acct"], setups=[s_mkdir("../tree/profiles/alpha")]))
    A(Case("new-profile warning then refusal", "new-profile", ["alpha", "a/b", "/nonexistent/x", "acct", "bad|name"]))
    for name in ["Zoe O'Brien/Dev Team", "$(echo hi)", "Al\\nice", "x\\ty", "a\\\\b", "C:\\Users\\me", "\\Ubig\\E", "\\ulow", "x\\0y", "a\\1b", "a\\9b", "ab\\", "a\nb", "a\\\nb", "\\x41", "\\x26", "\\d065", "\\o101", "\\cA", "\\c", "\\c\\", "a`b`c", "$HOME", "é ü 日本", "tab\there", " leading", "trailing ", "a'b\"c"]:
        A(Case("new-profile name %r" % name, "new-profile", ["alpha", "acme/widgets", "/srv/w", "alice", name]))
    for name in ["Al\\1", "\\U", "a&b"]:
        A(Case("new-profile slug-like value %r" % name, "new-profile", ["alpha", "o/" + name, "/srv/w", "alice"]))
    A(Case("new-profile path with backslash", "new-profile", ["alpha", "a/b", "/srv/C:\\dir", "alice"]))
    A(Case("new-profile account with backslash", "new-profile", ["alpha", "a/b", "/srv/w", "al\\nice"]))
    A(Case("new-profile profile with backslash", "new-profile", ["al\\npha", "a/b", "/srv/w", "alice"]))
    A(Case("new-profile no name uses account", "new-profile", ["alpha", "a/b", "/srv/w", "carol"]))
    A(Case("new-profile empty name uses account", "new-profile", ["alpha", "a/b", "/srv/w", "carol", ""]))
    A(Case("new-profile extra arguments", "new-profile", ["alpha", "a/b", "/srv/w", "carol", "N", "extra", "more"]))
    A(Case("new-profile three tasks", "new-profile", NP, setups=[tpl_env('TASKS="review issues audit"\n')]))
    A(Case("new-profile ten tasks", "new-profile", NP, setups=[tpl_env('TASKS="t1 t2 t3 t4 t5 t6 t7 t8 t9 t10"\n')]))
    A(Case("new-profile sixteen tasks", "new-profile", NP, setups=[tpl_env('TASKS="a b c d e f g h i j k l m n o p"\n')]))
    A(Case("new-profile no tasks line", "new-profile", NP, setups=[tpl_env('X=1\n')]))
    A(Case("new-profile tasks with a comment", "new-profile", NP, setups=[tpl_env('TASKS="a b" # c\n')]))
    A(Case("new-profile tasks unclosed", "new-profile", NP, setups=[tpl_env('TASKS="a b\n')]))
    A(Case("new-profile tasks twice", "new-profile", NP, setups=[tpl_env('TASKS="a"\nTASKS="b c"\n')]))
    A(Case("new-profile task with slash", "new-profile", NP, setups=[tpl_env('TASKS="a/b"\n')]))
    A(Case("new-profile timer overwritten", "new-profile", NP, setups=[s_tree_file("platform/linux/maintainer@alpha-review.timer", "old content\n")]))
    A(Case("new-profile surviving placeholder", "new-profile", NP, setups=[s_tree_file("profiles/_template/prompts/review.md", "Hello __EXTRA__ and __SLUG__\n")]))
    A(Case("new-profile surviving placeholder underscore", "new-profile", NP, setups=[s_tree_file("profiles/_template/notes.txt", "Owner: __MY_OWNER__\n")]))
    A(Case("new-profile four underscores", "new-profile", NP, setups=[s_tree_file("profiles/_template/notes.txt", "x ____ y\n")]))
    A(Case("new-profile three underscores", "new-profile", NP, setups=[s_tree_file("profiles/_template/notes.txt", "x ___ y __a__ __A_ _B__\n")]))
    A(Case("new-profile binary file", "new-profile", NP, setups=[s_tree_file("profiles/_template/blob.bin", "a\0__X__\0b\n")]))
    A(Case("new-profile no trailing newline", "new-profile", NP, setups=[s_tree_file("profiles/_template/notes.txt", "end __SLUG__")]))
    A(Case("new-profile empty file", "new-profile", NP, setups=[s_tree_file("profiles/_template/empty.txt", "")]))
    A(Case("new-profile template modes", "new-profile", NP, setups=[s_chmod("profiles/_template/notes.txt", 0o755, "tree"), s_chmod("profiles/_template/prompts/review.md", 0o640, "tree")]))
    A(Case("new-profile template symlink", "new-profile", NP, setups=[s_symlink("profiles/_template/link.txt", "notes.txt", "tree")]))
    A(Case("new-profile no template", "new-profile", NP, setups=[s_rm_tree("profiles/_template")]))
    A(Case("new-profile empty template dir", "new-profile", NP, setups=[s_rm_tree("profiles/_template"), s_mkdir("profiles/_template", None, "tree")]))
    A(Case("new-profile unwritable profiles dir", "new-profile", NP, setups=[s_chmod("profiles", 0o555, "tree")]))
    A(Case("new-profile unwritable units dir", "new-profile", NP, setups=[s_chmod("platform/linux", 0o555, "tree")]))
    A(Case("new-profile umask 077", "new-profile", NP, umask=0o077))
    # install-cron
    DEP = [s_deploy()]
    for prof in (None, "sysknife", "magent", "zzz"):
        for mode in ([], ["--dry-run"], ["--remove"], ["--bogus"]):
            env = {"MAINTAINER_PROFILE": prof} if prof else {}
            A(Case("cron %s %s" % (prof, mode), "cron", mode, env=env, setups=DEP))
    A(Case("cron nothing deployed", "cron", [], env={}))
    A(Case("cron no run.sh", "cron", [], env={"MAINTAINER_PROFILE": "sysknife"}, setups=[s_deploy(), s_rm_home(".local/share/maintainer/run.sh")]))
    A(Case("cron no crontab", "cron", [], env={"MAINTAINER_PROFILE": "sysknife"}, setups=DEP + [s_drop("crontab")]))
    A(Case("cron existing crontab", "cron", [], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_case("crontab.store", "0 1 * * * /usr/bin/backup\n# maintainer-agent (magent)\nold\n# maintainer-agent (magent) end\n5 5 * * * /bin/x\n")]))
    A(Case("cron crontab without trailing newline", "cron", [], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_case("crontab.store", "0 1 * * * /usr/bin/backup")]))
    A(Case("cron open block without end", "cron", [], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_case("crontab.store", "a\n# maintainer-agent (magent)\nb\nc\n")]))
    A(Case("cron end without open", "cron", [], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_case("crontab.store", "a\n# maintainer-agent (magent) end\nb\n")]))
    A(Case("cron blank lines", "cron", [], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_case("crontab.store", "\n\na\n\n\nb\n\n\n")]))
    A(Case("cron spool error", "cron", [], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_case("crontab.lerr", "crontab: temporary failure reading spool\n")]))
    A(Case("cron write fails", "cron", [], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_case("crontab.w.rc", "3")]))
    A(Case("cron remove write fails", "cron", ["--remove"], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_case("crontab.w.rc", "3")]))
    A(Case("cron state dir not a directory", "cron", [], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_file("blocker", ""), s_file(".local/share/maintainer/profiles/magent/profile.env", 'TASKS="review"\nSTATE_DIR="$HOME/blocker"\n')]))
    A(Case("cron profile name looks like run sh", "cron", ["--dry-run"], env={"MAINTAINER_PROFILE": "x_run_sh"}, setups=DEP + [s_file(".local/share/maintainer/profiles/x_run_sh/profile.env", 'TASKS="t"\nSTATE_DIR=/s\n')]))
    A(Case("cron profile env fails last command", "cron", ["--dry-run"], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'TASKS="a"\nSTATE_DIR=/s\nfalse\n')]))
    A(Case("cron profile env exits", "cron", ["--dry-run"], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'exit 5\n')]))
    A(Case("cron profile env prints", "cron", ["--dry-run"], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'echo hi; echo err >&2\nTASKS="a"\nSTATE_DIR=/s\n')]))
    A(Case("cron profile env no state dir", "cron", ["--dry-run"], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'TASKS="a"\n')]))
    A(Case("cron profile env no tasks", "cron", ["--dry-run"], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'STATE_DIR=/s\n')]))
    A(Case("cron install no state dir", "cron", [], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'TASKS="a"\n')]))
    A(Case("cron remove no state dir", "cron", ["--remove"], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'TASKS="a"\n')]))
    A(Case("cron empty tasks", "cron", [], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'TASKS=""\nSTATE_DIR=/tmp/zz\n')]))
    A(Case("cron profiles dir empty", "cron", [], env={}, setups=[s_mkdir(".local/share/maintainer/profiles")]))
    # install-launchd
    for prof in (None, "sysknife", "magent", "nope"):
        env = {"MAINTAINER_PROFILE": prof} if prof else {}
        A(Case("launchd env %s" % prof, "launchd", [], env=env, setups=DEP))
    A(Case("launchd arg", "launchd", ["sysknife"], setups=DEP))
    A(Case("launchd arg beats env", "launchd", ["sysknife"], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP))
    A(Case("launchd remove", "launchd", ["--remove", "sysknife"], setups=DEP + [s_file("Library/LaunchAgents/dev.maintainer.sysknife.review.plist", ""), s_file("Library/LaunchAgents/dev.maintainer.sysknife.audit.plist", ""), s_file("Library/LaunchAgents/dev.maintainer.magent.review.plist", ""), s_file("Library/LaunchAgents/other.plist", ""), s_file("Library/LaunchAgents/dev.maintainer.sysknife.plist", "")]))
    A(Case("launchd remove env", "launchd", ["--remove"], env={"MAINTAINER_PROFILE": "magent"}, setups=DEP))
    A(Case("launchd remove no profile", "launchd", ["--remove"], setups=DEP))
    A(Case("launchd load fails", "launchd", ["sysknife"], setups=DEP + [s_case("launchctl.fail.load")]))
    A(Case("launchd unload fails", "launchd", ["sysknife"], setups=DEP + [s_case("launchctl.fail.unload")]))
    A(Case("launchd no min hours", "launchd", ["magent"], setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'TASKS="review nomin"\nSTATE_DIR="$HOME/st"\nREPO_PATH=/r\n')]))
    A(Case("launchd invalid min name", "launchd", ["magent"], setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'TASKS="a-b"\nSTATE_DIR="$HOME/st"\nREPO_PATH=/r\n')]))
    A(Case("launchd no state dir", "launchd", ["magent"], setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'TASKS="a"\nREPO_PATH=/r\n')]))
    A(Case("launchd no tasks", "launchd", ["magent"], setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'STATE_DIR="$HOME/st"\nREPO_PATH=/r\n')]))
    A(Case("launchd no repo path", "launchd", ["magent"], setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'TASKS="a"\nSTATE_DIR="$HOME/st"\n')]))
    A(Case("launchd empty tasks no repo path", "launchd", ["magent"], setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'TASKS=""\nSTATE_DIR="$HOME/st"\n')]))
    A(Case("launchd env fails", "launchd", ["magent"], setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'TASKS="a"\nSTATE_DIR="$HOME/st"\nREPO_PATH=/r\nfalse\n')]))
    A(Case("launchd env prints", "launchd", ["magent"], setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'echo hi\nTASKS="a"\nSTATE_DIR="$HOME/st"\nREPO_PATH=/r\n')]))
    A(Case("launchd state dir blocked", "launchd", ["magent"], setups=DEP + [s_file("blocker", ""), s_file(".local/share/maintainer/profiles/magent/profile.env", 'TASKS="a"\nSTATE_DIR="$HOME/blocker"\nREPO_PATH=/r\n')]))
    A(Case("launchd special characters", "launchd", ["magent"], setups=DEP + [s_file(".local/share/maintainer/profiles/magent/profile.env", 'TASKS="a"\nSTATE_DIR="$HOME/st"\nREPO_PATH="/r & <x>"\n')]))
    # alert
    for inst in ("sysknife-ci", "my-repo-review", "solo", "maintainer@sysknife-ci.service", "ghost-review", "", "a b-c", "-x", "x-"):
        A(Case("alert %r" % inst, "alert", [inst] if inst else [], setups=[s_state("sysknife"), s_state("magent"), s_state("my-repo"), s_state("my"), s_state("solo")]))
    A(Case("alert no state", "alert", ["sysknife-ci"]))
    A(Case("alert no argument", "alert", [], setups=[s_state("sysknife")]))
    A(Case("alert twice", "alert", ["sysknife-ci"], setups=[s_state("sysknife"), s_file(".local/state/sysknife-maint/logs/alerts.log", "earlier\n")]))
    A(Case("alert notify env kept", "alert", ["sysknife-ci"], env={"DISPLAY": ":7", "DBUS_SESSION_BUS_ADDRESS": "unix:path=/x/bus"}, setups=[s_state("sysknife")]))
    A(Case("alert notify off", "alert", ["sysknife-ci"], env={"MAINTAINER_NOTIFY": "off"}, setups=[s_state("sysknife")]))
    A(Case("alert notify not off", "alert", ["sysknife-ci"], env={"MAINTAINER_NOTIFY": "OFF"}, setups=[s_state("sysknife")]))
    A(Case("alert notify missing", "alert", ["sysknife-ci"], setups=[s_state("sysknife"), s_drop("notify-send")]))
    A(Case("alert notify fails", "alert", ["sysknife-ci"], setups=[s_state("sysknife"), s_case("notify.rc", "1")]))
    A(Case("alert date missing", "alert", ["sysknife-ci"], setups=[s_state("sysknife"), s_drop("date")]))
    A(Case("alert logs dir unwritable", "alert", ["sysknife-ci"], setups=[s_state("sysknife"), s_chmod(".local/state/sysknife-maint", 0o555)]))
    A(Case("alert log file read-only", "alert", ["sysknife-ci"], setups=[s_state("sysknife"), s_file(".local/state/sysknife-maint/logs/alerts.log", "x\n", 0o444)]))
    A(Case("alert state dir is a symlink", "alert", ["sysknife-ci"], setups=[s_mkdir("real"), s_symlink(".local/state/sysknife-maint", "real")]))
    A(Case("alert glob dirs with dots and dashes", "alert", ["zz-ci"], setups=[s_state("a-b"), s_state(".hidden"), s_state("c")]))
    # run-instance
    for inst in ("sysknife-ci", "my-repo-review", "solo", "a-b", "-", "a--b", ""):
        A(Case("instance %r" % inst, "instance", [inst] if inst else [], setups=[s_inst]))
    A(Case("instance exit code", "instance", ["a-b"], env={"RUN_RC": "75"}, setups=[s_inst]))
    A(Case("instance symlink", "instance", ["a-b"], setups=[s_inst, lambda side: (side.tree / "elsewhere").mkdir(), lambda side: (side.tree / "elsewhere/ri.sh").symlink_to(side.tree / SCRIPT["instance"])]))
    A(Case("instance no run.sh", "instance", ["a-b"], setups=[s_inst, s_rm_tree("inst/run.sh")]))
    A(Case("instance run.sh not executable", "instance", ["a-b"], setups=[s_inst, s_chmod("inst/run.sh", 0o644, "tree")]))
    A(Case("instance run.sh is a directory", "instance", ["a-b"], setups=[s_inst, s_rm_tree("inst/run.sh"), s_mkdir("inst/run.sh", None, "tree")]))
    return c


def itertools_product(a, b):
    return [(x, y) for x in a for y in b]


def s_rm_home(rel):
    def go(side):
        p = side.home / rel
        if p.is_dir() and not p.is_symlink():
            shutil.rmtree(p)
        else:
            p.unlink(missing_ok=True)
    return go


def tpl_env(text):
    return lambda side: write(side.tree / "profiles/_template/profile.env", PROFILE_ENV_PREFIX + text)


PROFILE_ENV_PREFIX = 'PROFILE_NAME="__PROFILE__"\nREPO_PATH="${MAINTAINER_REPO_PATH:-__REPO_PATH__}"\nREPO_SLUG="__SLUG__"\nMAINTAINER_NAME="__MAINTAINER_NAME__"\nGH_ACCOUNT="__GH_ACCOUNT__"\nSTATE_DIR="${MAINTAINER_STATE_DIR:-$HOME/.local/state/__PROFILE__-maint}"\n'


def git_tree(tag, dirty=False, ahead=False):
    def go(side):
        env = {"PATH": str(side.tools), "HOME": str(side.home), "LC_ALL": "C", "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null",
               "GIT_AUTHOR_NAME": "f", "GIT_AUTHOR_EMAIL": "f@example.invalid", "GIT_COMMITTER_NAME": "f", "GIT_COMMITTER_EMAIL": "f@example.invalid",
               "GIT_AUTHOR_DATE": "1767225600 +0000", "GIT_COMMITTER_DATE": "1767225600 +0000"}
        g = lambda *a: subprocess.run([REAL_TOOLS["git"], "-C", str(side.tree), *a], env=env, check=True, capture_output=True)
        # Both sides commit the bash scripts, so the commit (and the VERSION stamp that names it)
        # is the same; on the Rust side the wrappers go back in place and git is told to ignore them.
        wrapped = sorted(r for r in WRAPPER_PATHS if (side.tree / r).is_file() and (BASH_TREE / r).is_file() and not r.startswith(".local") and not r.startswith("inst/"))
        kept = {}
        for rel in wrapped:
            kept[rel] = (side.tree / rel).read_bytes()
            (side.tree / rel).write_bytes((BASH_TREE / rel).read_bytes())
        g("init", "-q", "-b", "main")
        g("add", "-A")
        g("commit", "-q", "-m", "base")
        for rel in wrapped:
            (side.tree / rel).write_bytes(kept[rel])
            g("update-index", "--assume-unchanged", rel)
        if tag:
            g("tag", tag)
        if ahead:
            (side.tree / "extra.txt").write_text("x\n")
            g("add", "-A")
            g("commit", "-q", "-m", "two")
        if dirty:
            with open(side.tree / "lib/defaults.env", "a") as f:
                f.write("# edit\n")
    return go


# ---------------------------------------------------------------------------------------
# generated scenarios

def generated(rng, n):
    cases = []
    unames = ["Linux"] * 6 + ["Darwin", "FreeBSD", "MINGW64_NT-10.0", ""]
    hostile = ["plain", "Zoe O'Brien", "$(touch x)", "a\\nb", "a\\\\b", "x\\ty", "\\Uab\\E", "é", "a b  c", "'q'", "\"d\"", "a\\1", "\\", "x\\0y", "C:\\w", "a`b`", "\\cA", "\\x41\\d066\\o103"]
    task_sets = ['review issues', 'a', 'a b c d e f g h i j k', 'x\ty', ' lead  trail ', 'a-b c', '#', 'r*', '']
    for i in range(n):
        kind = rng.choices(["install", "uninstall", "newprofile", "cron", "launchd", "alert", "instance"], [28, 14, 22, 14, 10, 8, 4])[0]
        setups = []
        env = {}
        name = "gen%03d %s" % (i, kind)
        umask = rng.choice([0o022, 0o022, 0o022, 0o077, 0o027, 0o002])
        if rng.random() < 0.25:
            setups.append(s_uname(rng.choice(unames)))
        if kind in ("install", "uninstall"):
            args = []
            if kind == "uninstall":
                args.append("--uninstall")
            for flag in ("--dry-run", "--timers", "--force"):
                if rng.random() < 0.3:
                    args.append(flag)
            rng.shuffle(args)
            home_state = rng.choice(["empty", "deployed", "deployed-timers", "stale", "key", "symlink-cmd", "ro-bin", "ro-units", "dir-cmd", "dangling-unit", "deployed-ro-file"])
            if kind == "uninstall" and home_state in ("empty", "key", "stale"):
                home_state = "deployed-timers"
            if home_state == "deployed":
                setups.append(s_deploy())
            elif home_state == "deployed-timers":
                setups.append(s_deploy("--timers"))
            elif home_state == "stale":
                setups += [s_deploy(), s_file(".local/share/maintainer/profiles/stale%d.txt" % i, ""), s_file(".local/share/maintainer/backends/stale.sh", ""), s_file(".local/bin/extra", "x")]
            elif home_state == "key":
                setups.append(s_file(".local/share/maintainer/receipt.key", "k" * rng.randint(1, 80), rng.choice([0o600, 0o640, 0o400])))
            elif home_state == "symlink-cmd":
                setups += [s_file("elsewhere", "old"), s_symlink(".local/bin/%s" % rng.choice(["maintainer", "maintainer-merge", "maintainer-mcp"]), "elsewhere")]
            elif home_state == "ro-bin":
                setups.append(s_mkdir(".local/bin", 0o555))
            elif home_state == "ro-units":
                setups.append(s_mkdir(".config/systemd/user", 0o555))
            elif home_state == "dir-cmd":
                setups.append(s_mkdir(".local/bin/maintainer-repo"))
            elif home_state == "dangling-unit":
                setups.append(s_symlink(".config/systemd/user/maintainer-dangling.service", "nowhere"))
            elif home_state == "deployed-ro-file":
                setups += [s_deploy(), s_chmod(rng.choice([".local/bin/maintainer", ".local/share/maintainer/VERSION", ".local/share/maintainer/run.sh"]), 0o444)]
            lock = rng.choice([None, None, None, "free", "held", "held2"])
            if lock:
                setups.append(s_lock())
                if lock == "held2":
                    setups.append(s_lock("magent"))
                if lock != "free":
                    setups.append(s_case("flock.rc", "1"))
            for t in rng.sample(["git", "python3", "systemctl", "crontab", "flock", "date", "notify-send", "od"], rng.choice([0, 0, 0, 1, 1, 2])):
                if t != "od":
                    setups.append(s_drop(t))
            if rng.random() < 0.15:
                setups.append(s_case("systemctl.fail.%s" % rng.choice(["enable", "disable", "daemon-reload", "show-environment"]), str(rng.choice([1, 2, 4]))))
            if kind == "uninstall":
                if rng.random() < 0.4:
                    setups.append(s_case("systemctl.timers", "maintainer@x.timer n/a\n" * rng.choice([1, 2])))
                if rng.random() < 0.3:
                    env["MAINTAINER_UNINSTALL_YES"] = rng.choice(["1", "1", "yes", ""])
                if rng.random() < 0.2:
                    setups.append(s_case("systemctl.noenv"))
                if rng.random() < 0.2:
                    setups.append(s_case("crontab.w.rc", "1"))
                if rng.random() < 0.2:
                    setups.append(s_file("Library/LaunchAgents/dev.maintainer.sysknife.%s.plist" % rng.choice(["review", "ci"]), ""))
            profiles = rng.choice([("sysknife", "magent"), ("sysknife", "magent"), ("magent",), ("sysknife",), ()])
            setups.append(s_profiles(*profiles))
            if rng.random() < 0.15:
                setups.append(s_tree_file("platform/linux/maintainer@%s-ci.timer" % rng.choice(["gone", "magent", "my-repo"]), "[Timer]\n"))
            if rng.random() < 0.1:
                setups.append(s_tree_file("scripts/zz%d.py" % i, "print(1)\n", rng.choice([0o644, 0o755])))
            if rng.random() < 0.1:
                setups.append(git_tree(rng.choice(["", "v0.%d.0" % i]), dirty=rng.random() < 0.3))
            cases.append(Case(name + " " + " ".join(args), "install", args, env, setups, umask))
        elif kind == "newprofile":
            prof = rng.choice(["alpha", "beta-2", "x_y", "a.b", "gamma"] + ["_u", "a/b", "sysknife"] * 1)
            slug = rng.choice(["acme/widgets", "o/r", "nowhere", "a/b/c"])
            path = rng.choice(["@HOME@/code/w", "/srv/w", "@HOME@", "/srv/C:\\w", "@HOME@/", "/a b/c"])
            account = rng.choice(hostile)
            nm = rng.choice(hostile + [None, None, ""])
            args = [prof, slug, path, account] + ([nm] if nm is not None else [])
            if rng.random() < 0.1:
                args = args[: rng.randint(0, 3)]
            if rng.random() < 0.15:
                args[rng.randrange(len(args))] += rng.choice(["|", "&"]) if args else ""
            tasks = rng.choice(task_sets)
            setups.append(tpl_env('TASKS="%s"\n' % tasks if rng.random() < 0.9 else 'X=1\n'))
            if rng.random() < 0.15:
                setups.append(s_tree_file("profiles/_template/notes.txt", rng.choice(["x __Q__ y\n", "____\n", "__a__\n", "no placeholder\n", "end __SLUG__"])))
            if rng.random() < 0.1:
                setups.append(s_mkdir("code/w"))
            if rng.random() < 0.1:
                setups.append(s_chmod("profiles/_template/notes.txt", rng.choice([0o755, 0o640, 0o444]), "tree"))
            cases.append(Case(name + " " + repr(args)[:60], "new-profile", args, env, setups, umask))
        elif kind in ("cron", "launchd"):
            setups.append(s_deploy())
            prof = rng.choice(["sysknife", "magent", "zzz", None])
            if prof:
                env["MAINTAINER_PROFILE"] = prof
            pe = rng.choice(["ok", "ok", "ok", "notasks", "nostate", "empty", "weird", "fails", "exit", "prints", "hostile-state"])
            if pe != "ok" and prof in ("sysknife", "magent"):
                text = {
                    "notasks": 'STATE_DIR="$HOME/st"\nREPO_PATH=/r\n', "nostate": 'TASKS="a b"\nREPO_PATH=/r\n', "empty": 'TASKS=""\nSTATE_DIR="$HOME/st"\n',
                    "weird": 'TASKS="x-y  z"\nSTATE_DIR="$HOME/st dir"\nREPO_PATH="/r s"\nMIN_HOURS_z=7\n', "fails": 'TASKS="a"\nSTATE_DIR=/tmp/zzs\nREPO_PATH=/r\nfalse\n',
                    "exit": 'exit 3\n', "prints": 'echo out\necho err >&2\nTASKS="a"\nSTATE_DIR="$HOME/st"\nREPO_PATH=/r\n', "hostile-state": 'TASKS="a"\nSTATE_DIR="$HOME/a&b"\nREPO_PATH="/r&q"\n',
                }[pe]
                setups.append(s_file(".local/share/maintainer/profiles/%s/profile.env" % prof, text))
            if kind == "cron":
                args = [rng.choice(["--dry-run", "--remove", "", "--bogus"])]
                args = [a for a in args if a]
                store = rng.choice([None, "0 1 * * * /usr/bin/backup\n", "# maintainer-agent (magent)\nx\n# maintainer-agent (magent) end\n", "a\n# maintainer-agent (sysknife)\nb\n", "\n\nz\n\n", "no newline"])
                if store is not None:
                    setups.append(s_case("crontab.store", store))
                if rng.random() < 0.1:
                    setups.append(s_case("crontab.lerr", "crontab: temporary failure reading spool\n"))
                if rng.random() < 0.1:
                    setups.append(s_case("crontab.w.rc", str(rng.choice([1, 3]))))
                if rng.random() < 0.05:
                    setups.append(s_drop("crontab"))
                if rng.random() < 0.05:
                    setups.append(s_rm_home(".local/share/maintainer/run.sh"))
            else:
                args = rng.choice([[], ["--remove"], [prof or "magent"], ["--remove", prof or "sysknife"]])
                if rng.random() < 0.2:
                    setups.append(s_file("Library/LaunchAgents/dev.maintainer.%s.review.plist" % (prof or "sysknife"), ""))
                if rng.random() < 0.15:
                    setups.append(s_case("launchctl.fail.%s" % rng.choice(["load", "unload"])))
            cases.append(Case(name + " %s %s" % (prof, args), kind, args, env, setups, umask))
        elif kind == "alert":
            states = rng.sample(["sysknife", "magent", "my-repo", "my", "solo", "a b"], rng.randint(0, 4))
            setups += [s_state(s) for s in states]
            inst = rng.choice(["sysknife-ci", "my-repo-review", "solo", "maintainer@sysknife-ci.service", "ghost-x", "a b-c", "x-y-z", ""])
            args = [inst] if inst or rng.random() < 0.5 else []
            if rng.random() < 0.3:
                env["MAINTAINER_NOTIFY"] = rng.choice(["off", "on", "OFF", ""])
            if rng.random() < 0.3:
                env["DISPLAY"] = rng.choice([":7", "", ":0"])
            if rng.random() < 0.2:
                env["DBUS_SESSION_BUS_ADDRESS"] = rng.choice(["unix:path=/x/bus", ""])
            for t in rng.sample(["notify-send", "date"], rng.choice([0, 0, 1])):
                setups.append(s_drop(t))
            if rng.random() < 0.15 and states:
                setups.append(s_chmod(".local/state/%s-maint" % states[0], rng.choice([0o555, 0o500])))
            cases.append(Case(name + " %r" % args, "alert", args, env, setups, umask))
        else:
            inst = rng.choice(["sysknife-ci", "a-b", "solo", "x--y", "-", "a-b-c", ""])
            setups.append(s_inst)
            if rng.random() < 0.2:
                env["RUN_RC"] = str(rng.choice([0, 1, 75]))
            if rng.random() < 0.15:
                setups.append(rng.choice([s_rm_tree("inst/run.sh"), s_chmod("inst/run.sh", 0o644, "tree")]))
            cases.append(Case(name + " %r" % inst, "instance", [inst] if inst else [], env, setups, umask))
    return cases


def expand(case, side):
    """@HOME@ and @CASE@ in the arguments and the environment name this side's paths."""
    return case


def substitute(case):
    """Returns a copy of the case whose arguments are written for one side (see run_case)."""
    return case


# ---------------------------------------------------------------------------------------

def main():
    real_before = real_digests()
    rng = random.Random(SEED)
    cases = named() + generated(rng, RANDOM_COUNT)
    if ONLY:
        cases = [c for c in cases if any(o in c.name for o in ONLY)]
    names = set()
    for c in cases:
        if c.name in names:
            c.name += " #dup"
        names.add(c.name)
    ok = failed = 0
    failures = []
    for idx, case in enumerate(cases):
        results = run_case_with_paths(case, idx)
        problems = compare(case, results)
        if problems:
            failed += 1
            failures.append((case, problems))
            print("DIFF  %s" % case.name)
            for p in problems[:4]:
                print("      " + p[:900])
        else:
            ok += 1
            if VERBOSE:
                print("same  %s (rc %d)" % (case.name, results["bash"]["rc"]))
        shutil.rmtree(WORK / ("%03d" % idx), ignore_errors=True) if os.environ.get("DIFF_KEEP") != "1" else None
    after = real_digests()
    untouched = True
    print("differential: %d comparisons, %d identical, %d different" % (len(cases), ok, failed))
    for name in real_before:
        if after[name] == real_before[name]:
            print("differential: real %s: unchanged" % name)
            continue
        if not isinstance(after[name], dict):
            untouched = False
            print("differential: real %s: CHANGED" % name)
            continue
        moved = sorted(k for k in set(after[name]) | set(real_before[name]) if after[name].get(k) != real_before[name].get(k))
        # A path the installer can reach is a failure. A path of another program that changed
        # while this ran (a timer's log, say) is listed and does not fail the run.
        ours = [k for k in moved if reachable(k)]
        foreign = [k for k in moved if not reachable(k)]
        print("differential: real %s: %s" % (name, "CHANGED" if ours else "unchanged where the installer writes"))
        if ours:
            untouched = False
            print("differential:   paths the installer can write that differ: %s" % ours[:12])
        if foreign:
            print("differential:   changed by another program meanwhile, not reachable by the installer: %s" % foreign[:12])
    if not untouched or failed or len(cases) < 300:
        if len(cases) < 300 and not ONLY:
            print("differential: only %d comparisons ran, 300 or more are required" % len(cases))
        sys.exit(1)
    sys.exit(0)


def tree_digest(root, content=True, depth=None):
    """A hash of every path below root with its mode and size, and of its bytes when
    `content` is set. `depth` limits the walk (the real ~/.local holds 111 GB and 743 000
    files, so only the places the installer writes are hashed by content)."""
    h = hashlib.sha256()
    if not root.exists():
        return "absent"
    base = len(root.parts)
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        level = len(Path(dirpath).parts) - base
        if depth is not None and level >= depth:
            dirnames[:] = []
        if "containers" in dirnames and root.name == "share":
            dirnames.remove("containers")
        for name in sorted(filenames) + list(dirnames):
            p = Path(dirpath) / name
            try:
                st = p.lstat()
            except OSError:
                continue
            h.update(("%s %o %d\n" % (p.relative_to(root), st.st_mode, 0 if stat.S_ISDIR(st.st_mode) else st.st_size)).encode())
            if content and stat.S_ISREG(st.st_mode):
                try:
                    h.update(p.read_bytes())
                except OSError:
                    pass
    return h.hexdigest()


def shallow_rows(root, depth):
    """Name, mode and size of every path below root down to `depth` levels."""
    out = {}
    base = len(root.parts)
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        if len(Path(dirpath).parts) - base >= depth:
            dirnames[:] = []
        if "containers" in dirnames and root.name == "share":
            dirnames.remove("containers")
        for name in sorted(filenames) + list(dirnames):
            p = Path(dirpath) / name
            try:
                st = p.lstat()
            except OSError:
                continue
            out[str(p.relative_to(root))] = (st.st_mode, 0 if stat.S_ISDIR(st.st_mode) else st.st_size)
    return out


def reachable(rel):
    """Whether the installer, uninstaller or an installed command writes here: ~/.local/bin,
    share/maintainer, share/systemd, a state directory named *-maint, Library/LaunchAgents."""
    parts = Path(rel).parts
    return (parts[:1] == ("bin",) or parts[:2] in (("share", "maintainer"), ("share", "systemd"))
            or (parts[:1] == ("state",) and len(parts) > 1 and parts[1].endswith("-maint")))


def real_digests():
    """What must not change: the places the installer writes, by content, and the rest of
    ~/.local to a depth of three by name, mode and size."""
    deep = [REAL_HOME / ".local/bin", REAL_HOME / ".local/share/maintainer", REAL_HOME / ".local/share/systemd", REAL_HOME / ".config/systemd"]
    out = {str(p): tree_digest(p) for p in deep}
    out["~/.local, depth 3, names modes sizes"] = shallow_rows(REAL_HOME / ".local", 3)
    return out


def run_case_with_paths(case, idx):
    base = WORK / ("%03d" % idx)
    sides = {"bash": Side(base, "bash", BASH_TREE), "rust": Side(base, "rust", RUST_TREE)}
    results = {}
    for which, side in sides.items():
        fixture_profiles(side)
        for setup in case.setups:
            setup(side)
        args = [a.replace("@HOME@", str(side.home)).replace("@CASE@", str(side.case)) for a in case.args]
        env = {k: v.replace("@HOME@", str(side.home)).replace("@CASE@", str(side.case)) for k, v in case.env.items()}
        script = SCRIPT[case.prog]
        rc, out, err = side.run([side.tree / script, *args], env, case.umask)
        results[which] = {
            "rc": rc, "stdout": norm_text(side, out), "stderr": norm_text(side, err), "calls": side.calls(),
            "home": side.snapshot(side.home), "tree": side.snapshot(side.tree), "files": side.case_files(),
        }
    return results


if __name__ == "__main__":
    main()
