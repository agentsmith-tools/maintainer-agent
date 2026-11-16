#!/usr/bin/env python3
"""Differential proof: the Rust maintainer-doctor against bin/maintainer-doctor.

Every case builds one generated host (a fixture directory with its own HOME,
state directory, git repository and a farm of stubs for every external tool),
runs the bash doctor in it, rebuilds the same host in the same place, runs the
Rust doctor, and compares:

  stdout, stderr, exit code, the global call log (one line per stub call across
  all stubs: tool, argv, a fixed list of environment variables and the size of
  stdin), and the state tree left behind (paths, types, modes, content hashes).

Both runs happen at the same path, so no path needs rewriting; only the random
name of the sandbox probe directory (tmp.XXXXXXXXXX) is normalised. Ages are
placed a margin of 20 minutes past each whole hour, so the few seconds between
the two runs cannot move a result.

One difference is expected and listed: the bash doctor passes the pinned token
to `grep` in argv (plan decision F4) and the Rust one scans in process. A case
whose only difference is that grep call is counted as `pinned`; the token must
then be absent from every stub log of the Rust run, and present in the grep log
of the bash run, or the case is a difference.

Usage: differential.py --seed N --cases N --rust-tree DIR [--bash-tree DIR]
"""
import argparse
import hashlib
import json
import os
import random
import re
import shutil
import subprocess
import sys
import time

TOKEN = "ghp_FAKEFAKEFAKEFAKEFAKE0000"
PLAIN_TOOLS = ("bash sh env python3 sed grep awk sort uniq cut tr head tail wc cat ls find dirname basename "
               "readlink stat date mktemp rm mkdir rmdir touch cp mv chmod sha256sum paste sleep timeout true "
               "false tee tar xargs ln").split()
LOGGED_ENV = ["HOME", "USER", "TZ", "LC_ALL", "TMPDIR", "CZ_CASE_DIR", "GH_TOKEN"]
GIT_DATE = "2026-01-05T12:00:00+00:00"


def real(name):
    for d in ("/usr/local/bin", "/usr/bin", "/bin"):
        p = os.path.join(d, name)
        if os.path.exists(p):
            return p
    raise SystemExit("no " + name)


REAL_GIT = real("git")
REAL_GREP = real("grep")

STUB = r'''#!/usr/bin/env bash
t="$(basename "$0")"
d="${CZ_CASE_DIR:?}"
line="$t"; for a in "$@"; do line="$line $(printf '%q' "$a")"; done
env_part=""; for v in @LOGGED@; do
    if [ "${!v+x}" = x ]; then
        case "$v" in GH_TOKEN) val=set ;; *) val="${!v}" ;; esac
    else val="<unset>"; fi
    env_part="$env_part $v=$val"
done
stdin_len="$(head -c 4096 | wc -c)"
printf '%s ENV%s STDIN=%s\n' "$line" "$env_part" "$stdin_len" >> "$d/calls.log"
printf '%s\n' "$line" >> "$d/stubs.all"
ans() {  # ans KEY: print the answer file and exit with its code; no file: unplanned
    local k="$1"
    if [ ! -e "$d/ans/$k.rc" ]; then printf '%s\n' "unplanned: $line" >> "$d/unplanned"; exit 99; fi
    [ -e "$d/ans/$k.out" ] && cat "$d/ans/$k.out"
    [ -e "$d/ans/$k.err" ] && cat "$d/ans/$k.err" >&2
    exit "$(cat "$d/ans/$k.rc")"
}
san() { printf '%s' "$1" | tr -c 'A-Za-z0-9' '_'; }
case "$t" in
gh)
    case "${1:-} ${2:-}" in
        "api user") ans api-user ;;
        "auth token") ans auth-token ;;
        "run list") ans run-list ;;
        "pr view") ans "pr-view.${3:-}" ;;
        *) ans unplanned-gh ;;
    esac ;;
git) exec "@GIT@" "$@" ;;
podman)
    case "${1:-} ${2:-}" in
        "system migrate") ans migrate ;;
        "image exists") ans "exists.$(san "${3:-}")" ;;
        "image inspect") ans "inspect.$(san "${!#}")" ;;
        "run --rm")
            case " $* " in
                *" docker.io/library/busybox:latest true "*) ans cold ;;
                *) ans "probe.$(san "${4:-}")" ;;
            esac ;;
        *) ans unplanned-podman ;;
    esac ;;
systemctl)
    case "${1:-} ${2:-}" in
        "--user show-environment") ans sc-env ;;
        "--user list-timers") ans sc-timers ;;
        "--user list-unit-files") ans sc-units ;;
        *) ans unplanned-systemctl ;;
    esac ;;
loginctl) ans loginctl ;;
systemd-run)
    if [ "$#" -eq 5 ] && [ "${5:-}" = true ]; then ans systemd-run-bare; else ans systemd-run-probe; fi ;;
launchctl) ans launchctl ;;
uname) if [ -e "$d/ans/uname.out" ]; then cat "$d/ans/uname.out"; else echo Linux; fi ;;
id) echo idfallback ;;
claude|codex|cursor-agent|opencode)
    if [ "${1:-}" = "--version" ]; then printf '2.0.0 (%s)\nsecond line\n' "$t"; exit 0; fi
    exit 99 ;;
maintainer-merge|maintainer|gh-present) exit 99 ;;
*) printf '%s\n' "unplanned: $line" >> "$d/unplanned"; exit 99 ;;
esac
'''

GREP_STUB = r'''#!/usr/bin/env bash
line=""; for x in "$@"; do line="$line $(printf '%q' "$x")"; done
printf '%s\n' "grep$line" >> "${CZ_CASE_DIR:?}/grepcalls"
exec "@GREP@" "$@"
'''


def rm_rf(path):
    """rmtree that first makes every directory writable (a case can leave a read-only state directory)."""
    for root, dirs, _files in os.walk(path):
        for d in dirs:
            try:
                os.chmod(os.path.join(root, d), 0o755)
            except OSError:
                pass
    try:
        os.chmod(path, 0o755)
    except OSError:
        pass
    shutil.rmtree(path, ignore_errors=True)


def ago_epoch(t0, hours, minutes=20):
    return t0 - hours * 3600 - minutes * 60


def iso(epoch):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(epoch))


class Fx:
    """One generated host, written to disk by build()."""

    def __init__(self, case, t0):
        self.case = case
        self.t0 = t0
        self.files = []        # (path, content bytes, mode, mtime or None)
        self.dirs = []
        self.profile_vars = {}  # ordered: written to profile.env
        self.extra_env = {}
        self.unset_env = set()
        self.ans = {}           # key -> (out or None, rc, err or None)
        self.tools = set()      # names present as stubs on PATH
        self.args = []
        self.git_setup = None   # callable(repo_dir)
        self.version_repo = None
        self.link_targets = []
        self.notes = []
        self.chmods = []        # (path, mode) applied after files
        self.tmpdir_missing = False
        self.profile = "magent"
        self.git_init_fails = False

    # paths
    @property
    def home(self): return self.case + "/home"
    @property
    def share(self): return self.home + "/.local/share/maintainer"
    @property
    def state(self): return self.case + "/state"
    @property
    def repo(self): return self.case + "/repo"
    @property
    def pdir(self): return self.share + "/profiles/" + self.profile


    def put(self, path, content="", mode=0o644, mtime=None):
        self.files.append((path, content if isinstance(content, bytes) else content.encode(), mode, mtime))

    def mkdir(self, path):
        self.dirs.append(path)

    def answer(self, key, out=None, rc=0, err=None):
        self.ans[key] = (out, rc, err)


def settings_json(n, extra=()):
    rules = ["Bash(rule%d:*)" % i for i in range(max(n - 1 - len(extra), 0))] + ["Read(~/.ssh/**)"] + list(extra)
    return json.dumps({"permissions": {"deny": rules}})


def healthy(fx, rng):
    """A healthy Linux deployment; later steps break one thing each."""
    s = fx.share
    fx.mkdir(s + "/backends")
    fx.put(s + "/run.sh", "#!/bin/sh\n", 0o755)
    fx.put(s + "/run-instance.sh", "#!/bin/sh\n", 0o755)
    fx.put(s + "/backends/claude.sh", "backend\n")
    fx.put(fx.home + "/.local/bin/maintainer", "#!/bin/sh\n", 0o755)
    fx.put(fx.home + "/.local/bin/maintainer-merge", "#!/bin/sh\n", 0o755)
    fx.profile_vars.update({"REPO_SLUG": "o/r", "REPO_PATH": fx.repo, "GH_ACCOUNT": "testuser",
                            "STATE_DIR": fx.state, "POST": "off", "REQUIRED_COMMANDS": "gh git"})
    fx.put(fx.pdir + "/deny.json", "{}\n")
    fx.put(fx.pdir + "/settings.json", settings_json(120))
    fx.put(fx.pdir + "/settings-rehearsal.json", settings_json(120))
    fx.put(fx.home + "/.config/systemd/user/podman-userns-warmup.service", "[Service]\n")
    fx.mkdir(fx.state)
    for t in ("gh", "git", "claude", "podman", "systemctl", "loginctl", "systemd-run", "uname", "id",
              "maintainer-merge"):
        fx.tools.add(t)
    fx.answer("api-user", "testuser\n")
    fx.answer("auth-token", TOKEN + "\n")
    fx.answer("run-list", runs_json(fx, [("CI", "success", "completed", 3)]))
    fx.answer("sc-env", "", 0)
    fx.answer("sc-timers", "Mon 2026-01-05 10:00:00 UTC 3h left Mon 2026-01-05 04:00:00 UTC 3h ago "
              "maintainer@%s-review.timer maintainer@%s-review.service\n" % (fx.profile, fx.profile), 0)
    fx.answer("sc-units", "", 0)
    fx.answer("loginctl", "yes\n", 0)
    fx.answer("cold", "", 0)
    fx.answer("migrate", "", 0)
    fx.git_setup = "plain"
    fx.version_repo = "clean"


def runs_json(fx, specs):
    rows = []
    for name, concl, status, hours in specs:
        rows.append({"workflowName": name, "status": status, "conclusion": concl,
                     "createdAt": iso(ago_epoch(fx.t0, hours)), "url": "https://example.invalid/run/%s" % hours})
    return json.dumps(rows)


# --- the dimensions: each is a list of (label, function(fx, rng)) ------------------

def d_os(fx, rng):
    name = rng.choice(["Linux"] * 6 + ["Darwin", "FreeBSD", "SunOS"])
    fx.answer_os = name
    if name != "Linux":
        fx.ans["uname"] = (name + "\n", 0, None)
    if name == "Darwin":
        fx.tools.add("launchctl")
        fx.answer("launchctl", rng.choice(["", "x\n", "dev.maintainer.a\ndev.maintainer.b\nother\n"]), 0)
    fx.notes.append("os=" + name)


def d_args(fx, rng):
    fx.args = rng.choice([[], [], ["--quick"], ["--quick"], ["x", "--quick"], ["--bogus"]])
    fx.notes.append("args=%s" % fx.args)


def d_profiles(fx, rng):
    k = rng.choice(["one"] * 6 + ["none", "two", "two-named", "badname", "template"])
    fx.notes.append("profiles=" + k)
    if k == "none":
        fx.drop_profile_env = True
    elif k in ("two", "two-named"):
        fx.put(fx.share + "/profiles/zeta/profile.env", "REPO_SLUG=z/z\n")
        if k == "two-named":
            fx.extra_env["MAINTAINER_PROFILE"] = fx.profile
    elif k == "badname":
        fx.extra_env["MAINTAINER_PROFILE"] = rng.choice(["a;b", "a b", "../x", "a/b", "bad$name"])
    elif k == "template":
        fx.mkdir(fx.share + "/profiles/_template")
    if rng.random() < 0.08:
        fx.extra_env["MAINTAINER_PROFILE"] = rng.choice(["magent", "other.name", "x-y_z"])


def d_install(fx, rng):
    k = rng.choice(["ok"] * 5 + ["no-share", "nested", "run-noexec", "run-missing", "inst-noexec",
                                  "inst-missing", "bin-missing", "bin-noexec", "backend-missing"])
    fx.notes.append("install=" + k)
    s = fx.share
    if k == "nested":
        fx.mkdir(s + "/profiles/magent/profiles")
        fx.mkdir(s + "/x/backends")
    elif k == "run-noexec":
        fx.chmods.append((s + "/run.sh", 0o644))
    elif k == "run-missing":
        fx.remove = getattr(fx, "remove", []) + [s + "/run.sh"]
    elif k == "inst-noexec":
        fx.chmods.append((s + "/run-instance.sh", 0o644))
    elif k == "inst-missing":
        fx.remove = getattr(fx, "remove", []) + [s + "/run-instance.sh"]
    elif k == "bin-missing":
        fx.remove = getattr(fx, "remove", []) + [fx.home + "/.local/bin/maintainer"]
    elif k == "bin-noexec":
        fx.chmods.append((fx.home + "/.local/bin/maintainer-merge", 0o644))
    elif k == "backend-missing":
        fx.remove = getattr(fx, "remove", []) + [s + "/backends/claude.sh"]
    elif k == "no-share":
        fx.no_share = True


def d_wall(fx, rng):
    k = rng.choice(["ok"] * 4 + ["template", "no-deny", "no-settings", "invalid", "n99", "n100", "home",
                                  "no-read", "str-deny", "num-deny", "not-obj", "empty-obj", "dict-deny"])
    fx.notes.append("wall=" + k)
    p = fx.pdir
    if k == "template":
        fx.put(p + "/settings.json.template", "{}")
    elif k == "no-deny":
        fx.remove = getattr(fx, "remove", []) + [p + "/deny.json"]
    elif k == "no-settings":
        fx.remove = getattr(fx, "remove", []) + [p + "/settings.json"]
    elif k == "invalid":
        fx.put(p + "/settings.json", "not json\n")
    elif k == "n99":
        fx.put(p + "/settings.json", settings_json(99))
    elif k == "n100":
        fx.put(p + "/settings.json", settings_json(100))
    elif k == "home":
        fx.put(p + "/settings.json", settings_json(120, ["Read(__HOME__/.ssh/**)"]))
    elif k == "no-read":
        fx.put(p + "/settings.json", json.dumps({"permissions": {"deny": ["Bash(x)"] * 120}}))
    elif k == "str-deny":
        fx.put(p + "/settings.json", json.dumps({"permissions": {"deny": "Read(~/" * 20}}))
    elif k == "num-deny":
        fx.put(p + "/settings.json", json.dumps({"permissions": {"deny": 5}}))
    elif k == "not-obj":
        fx.put(p + "/settings.json", "[1,2,3]")
    elif k == "empty-obj":
        fx.put(p + "/settings.json", "{}")
    elif k == "dict-deny":
        fx.put(p + "/settings.json", json.dumps({"permissions": {"deny": {str(i): 1 for i in range(130)}}}))
    r = rng.choice(["ok", "ok", "missing", "invalid", "ok"])
    fx.notes.append("rehearsal=" + r)
    if r == "missing":
        fx.remove = getattr(fx, "remove", []) + [p + "/settings-rehearsal.json"]
    elif r == "invalid":
        fx.put(p + "/settings-rehearsal.json", "not json")
    f = rng.choice(["default"] * 4 + ["150", "100", "abc", "env150", "empty"])
    fx.notes.append("floor=" + f)
    if f in ("150", "100", "abc"):
        fx.profile_vars["DENY_WALL_MIN_RULES"] = f
    elif f == "env150":
        fx.extra_env["MAINTAINER_DENY_WALL_MIN_RULES"] = "150"
    elif f == "empty":
        fx.profile_vars["DENY_WALL_MIN_RULES"] = ""


def d_backend(fx, rng):
    b = rng.choice(["claude"] * 3 + ["codex", "cursor", "opencode", "weird", "env-codex"])
    fx.notes.append("backend=" + b)
    present = rng.random() < 0.7
    if b == "env-codex":
        fx.extra_env["BACKEND"] = "codex"
        b = "codex"
    elif b != "claude":
        fx.profile_vars["BACKEND"] = b
    fx.put(fx.share + "/backends/%s.sh" % b, "backend\n")
    tool = {"codex": "codex", "cursor": "cursor-agent", "opencode": "opencode", "claude": "claude"}.get(b)
    if tool:
        if present:
            fx.tools.add(tool)
        else:
            fx.tools.discard(tool)


def d_commands(fx, rng):
    k = rng.choice(["gh git"] * 3 + ["", "gh frobnicate git", "maintainer maintainer-merge gh git", "echo",
                                      "  gh   git  ", "git\tgh"])
    fx.notes.append("commands=%r" % k)
    if k == "":
        fx.profile_vars.pop("REQUIRED_COMMANDS", None)
    else:
        fx.profile_vars["REQUIRED_COMMANDS"] = k
    fx.put(fx.home + "/.local/bin/x", "")  # keeps the directory present
    if rng.random() < 0.2:
        fx.tools.discard("gh")


def d_identity(fx, rng):
    k = rng.choice(["ok"] * 4 + ["no-account", "bad-cred", "case", "fail", "empty", "other", "nogh"])
    fx.notes.append("identity=" + k)
    if k == "no-account":
        fx.profile_vars.pop("GH_ACCOUNT", None)
    elif k == "bad-cred":
        fx.answer("api-user", '{"message":"Bad credentials","status":"401"}\n', 1)
    elif k == "case":
        fx.answer("api-user", "TestUser\n")
    elif k == "fail":
        fx.answer("api-user", "boom\n", 1)
    elif k == "empty":
        fx.answer("api-user", "", 0)
    elif k == "other":
        fx.answer("api-user", "someoneelse\n")
    elif k == "nogh":
        fx.tools.discard("gh")


def d_post(fx, rng):
    k = rng.choice(["off"] * 3 + ["unset", "on", "xoffx", "OFF", ""])
    fx.notes.append("post=" + k)
    if k == "unset":
        fx.profile_vars.pop("POST", None)
        fx.profile_vars["POST"] = ""  # defaults.env carries POST; empty reads as on
    else:
        fx.profile_vars["POST"] = k


def d_repo(fx, rng):
    k = rng.choice(["clean"] * 3 + ["none-set", "missing", "no-git", "dirty", "ahead", "no-origin", "dirty-ahead"])
    fx.notes.append("repo=" + k)
    fx.repo_kind = k
    if k == "none-set":
        fx.profile_vars.pop("REPO_PATH", None)
    elif k == "missing":
        fx.git_setup = None
    elif k == "no-git":
        fx.git_setup = "nogit"
    else:
        fx.git_setup = k


def d_runs(fx, rng):
    k = rng.choice(["ok"] * 2 + ["no-slug", "gh-fails", "empty", "object", "garbage", "red-old", "red-young",
                                  "superseded", "newest-fail", "two", "progress", "queued", "cancelled",
                                  "no-name", "threshold-under", "threshold-at", "threshold-over", "no-created",
                                  "naive", "bad-date", "str-int", "nonstr-name"])
    fx.notes.append("runs=" + k)
    g = rng.choice(["default"] * 5 + ["9", "1", "abc", "0.5", "1_0", "inf", "nan", "env"])
    fx.notes.append("grace=" + g)
    if g in ("9", "1", "abc", "0.5", "1_0", "inf", "nan"):
        fx.profile_vars["RED_GRACE_HOURS"] = g
    elif g == "env":
        fx.extra_env["MAINTAINER_RED_GRACE_HOURS"] = "1"
    if rng.random() < 0.15:
        fx.extra_env["MAINTAINER_DOCTOR_RUN_LIMIT"] = rng.choice(["5", "x y", "0"])
    R = lambda *specs: fx.answer("run-list", runs_json(fx, list(specs)))
    if k == "no-slug":
        fx.profile_vars.pop("REPO_SLUG", None)
    elif k == "gh-fails":
        fx.answer("run-list", "boom", 1)
    elif k == "empty":
        fx.answer("run-list", "[]")
    elif k == "object":
        fx.answer("run-list", "{}")
    elif k == "garbage":
        fx.answer("run-list", "not json at all")
    elif k == "red-old":
        R(("CI", "failure", "completed", 10))
    elif k == "red-young":
        R(("CI", "failure", "completed", 2))
    elif k == "superseded":
        R(("CI", "failure", "completed", 12), ("CI", "success", "completed", 3))
    elif k == "newest-fail":
        R(("CI", "success", "completed", 12), ("CI", "failure", "completed", 8))
    elif k == "two":
        R(("Zeta", "failure", "completed", 10), ("Alpha", "failure", "completed", 11), ("Mid", "success", "completed", 1))
    elif k == "progress":
        R(("CI", "", "in_progress", 30))
    elif k == "queued":
        R(("CI", "failure", "queued", 30))
    elif k == "cancelled":
        R(("CI", "cancelled", "completed", 30))
    elif k == "no-name":
        rows = json.loads(runs_json(fx, [("x", "failure", "completed", 10)]))
        rows[0]["workflowName"] = rng.choice([None, ""])
        del rows[0]["url"]
        fx.answer("run-list", json.dumps(rows))
    elif k.startswith("threshold"):
        # grace 9 hours; the run is 8h40m, 9h20m minus 20, etc: minutes chosen explicitly.
        minutes = {"threshold-under": 8 * 60 + 59 - 20, "threshold-at": 9 * 60 - 20, "threshold-over": 9 * 60 + 1 - 20}[k]
        fx.profile_vars["RED_GRACE_HOURS"] = "9"
        rows = [{"workflowName": "CI", "status": "completed", "conclusion": "failure",
                 "createdAt": iso(fx.t0 - minutes * 60 - 20 * 60), "url": "u"}]
        fx.answer("run-list", json.dumps(rows))
    elif k == "no-created":
        rows = json.loads(runs_json(fx, [("CI", "failure", "completed", 10), ("CI", "success", "completed", 3)]))
        del rows[1]["createdAt"]
        fx.answer("run-list", json.dumps(rows))
    elif k == "naive":
        rows = json.loads(runs_json(fx, [("CI", "failure", "completed", 10)]))
        rows[0]["createdAt"] = rows[0]["createdAt"].rstrip("Z")
        fx.answer("run-list", json.dumps(rows))
    elif k == "bad-date":
        rows = json.loads(runs_json(fx, [("CI", "failure", "completed", 10)]))
        rows[0]["createdAt"] = "yesterday"
        fx.answer("run-list", json.dumps(rows))
    elif k == "str-int":
        rows = json.loads(runs_json(fx, [("CI", "failure", "completed", 10)]))
        rows[0]["url"] = 5
        fx.answer("run-list", json.dumps(rows))
    elif k == "nonstr-name":
        rows = json.loads(runs_json(fx, [("CI", "failure", "completed", 10)]))
        rows[0]["workflowName"] = 7
        fx.answer("run-list", json.dumps(rows))


def d_scheduler(fx, rng):
    k = rng.choice(["ok"] * 3 + ["disabled", "none", "other-profile", "no-session", "no-systemctl", "linger-off",
                                  "linger-empty", "no-user", "soon", "not-soon", "one", "dotted"])
    fx.notes.append("sched=" + k)
    row = ("Mon 2026-01-05 10:00:00 UTC 3h left Mon 2026-01-05 04:00:00 UTC 3h ago maintainer@%s-review.timer "
           "maintainer@%s-review.service\n" % (fx.profile, fx.profile))
    if k == "disabled":
        fx.answer("sc-timers", "", 0)
        fx.answer("sc-units", "maintainer@%s-review.timer disabled enabled\nmaintainer@%s-issues.timer disabled enabled\n" % (fx.profile, fx.profile), 0)
    elif k == "none":
        fx.answer("sc-timers", "", 0)
    elif k == "other-profile":
        fx.answer("sc-timers", row.replace(fx.profile, "sysknife"), 0)
        fx.answer("sc-units", "maintainer@sysknife-review.timer disabled\n", 0)
    elif k == "no-session":
        fx.answer("sc-env", "", 1)
    elif k == "no-systemctl":
        fx.tools.discard("systemctl")
    elif k == "linger-off":
        fx.answer("loginctl", "no\n", 0)
    elif k == "linger-empty":
        fx.answer("loginctl", "", 0)
    elif k == "no-user":
        fx.unset_env.add("USER")
    elif k == "soon":
        fx.answer("sc-timers", "x   30s left Mon 2026-01-05 04:00:00 UTC 3h ago maintainer@%s-review.timer\n" % fx.profile, 0)
    elif k == "not-soon":
        fx.answer("sc-timers", "x   30m left Mon 2026-01-05 04:00:00 UTC 3h ago maintainer@%s-review.timer\n" % fx.profile, 0)
    elif k == "one":
        fx.answer("sc-timers", row, 0)
    elif k == "dotted":
        fx.answer("sc-timers", row.replace(fx.profile + "-", "aXb-").replace("magent", "aXb"), 0)


def suite_files(fx, name, image, needs, containerfile=None):
    body = "suite_image() { printf '%s' '" + image + "'; }\n"
    if needs is not None:
        body += "suite_needs() { printf '%s' '" + needs + "'; }\n"
    fx.put(fx.pdir + "/verify.d/%s.sh" % name, body)
    if containerfile is not None:
        fx.put(fx.pdir + "/verify.d/%s.Containerfile" % name, containerfile)


def d_podman(fx, rng):
    k = rng.choice(["ok"] * 3 + ["absent", "cold-fails", "no-warmup", "suites"] * 2)
    fx.notes.append("podman=" + k)
    if k == "absent":
        fx.tools.discard("podman")
    elif k == "cold-fails":
        fx.answer("cold", "", 1)
    elif k == "no-warmup":
        fx.remove = getattr(fx, "remove", []) + [fx.home + "/.config/systemd/user/podman-userns-warmup.service"]
    elif k == "suites":
        for i in range(rng.randint(1, 3)):
            name = "s%d" % i
            kind = rng.choice(["docker-ok", "docker-missing", "docker-fail", "no-needs", "broken", "local-norecipe",
                               "local-absent", "local-current", "local-unstamped", "local-novalue", "local-drift",
                               "local-miss"])
            fx.notes.append("suite=" + kind)
            img = "docker.io/library/python:3.12" if kind.startswith("docker") else "localhost/sk-img:%d" % i
            key = re.sub(r"[^A-Za-z0-9]", "_", img)
            cf = "FROM scratch\n# %d\n" % i
            h = hashlib.sha256(cf.encode()).hexdigest()
            if kind == "no-needs":
                suite_files(fx, name, img, None)
            elif kind == "broken":
                fx.put(fx.pdir + "/verify.d/%s.sh" % name, "this is (( not bash\n")
            elif kind == "local-norecipe":
                suite_files(fx, name, img, "bash git")
            elif kind.startswith("local"):
                suite_files(fx, name, img, "bash git", cf)
                if kind == "local-absent":
                    fx.answer("exists." + key, "", 1)
                else:
                    fx.answer("exists." + key, "", 0)
                    stamp = {"local-current": h, "local-unstamped": "", "local-novalue": "<no value>",
                             "local-drift": "deadbeef", "local-miss": h}[kind]
                    fx.answer("inspect." + key, stamp + "\n", 0)
                    if kind == "local-miss":
                        fx.answer("probe." + key, "cargo node \n", 0)
                    else:
                        fx.answer("probe." + key, "", 0)
            else:
                suite_files(fx, name, img, "bash python3 git")
                if kind == "docker-ok":
                    fx.answer("probe." + key, "", 0)
                elif kind == "docker-missing":
                    fx.answer("probe." + key, "python3 yamllint \n", 0)
                else:
                    fx.answer("probe." + key, "", 125)
        if rng.random() < 0.3:
            fx.put(fx.pdir + "/verify.d/notes.txt", "not a suite")


UNIT = """[Service]
Type=oneshot
ExecStart=/bin/true
NoNewPrivileges=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=read-only
RestrictSUIDSGID=yes
RestrictNamespaces=yes
RestrictAddressFamilies=AF_UNIX
SystemCallFilter=@system-service
SystemCallArchitectures=native
ReadWritePaths=/var/tmp
  PrivateDevices=yes
#ProtectKernelTunables=yes
Nice=5
"""


def d_sandbox(fx, rng):
    k = rng.choice(["no-unit"] * 3 + ["ok"] * 3 + ["no-directives", "no-merge", "no-transient", "probe-fails",
                                                     "no-systemd-run", "git-init-fails", "probe-one-line"])
    fx.notes.append("sandbox=" + k)
    unit = fx.home + "/.config/systemd/user/maintainer@.service"
    if k == "no-unit":
        return
    fx.put(unit, UNIT if k != "no-directives" else "[Service]\nNice=5\n")
    fx.answer("systemd-run-bare", "", 0)
    fx.answer("systemd-run-probe", "", 0)
    if k == "no-merge":
        fx.tools.discard("maintainer-merge")
    elif k == "no-transient":
        fx.answer("systemd-run-bare", "", 1)
    elif k == "probe-fails":
        fx.answer("systemd-run-probe", "line one\nline two\nline three\n", 1)
    elif k == "probe-one-line":
        fx.answer("systemd-run-probe", "only line", 1)
    elif k == "no-systemd-run":
        fx.tools.discard("systemd-run")
    elif k == "git-init-fails":
        fx.git_init_fails = True
        fx.git_stub = "init-fails"
    else:
        fx.tools.add("systemd-run")


def d_audit(fx, rng):
    k = rng.choice(["ok"] * 3 + ["no-state-dir-name", "state-missing", "state-file", "counts", "readonly"])
    fx.notes.append("audit=" + k)
    if k == "no-state-dir-name":
        fx.profile_vars.pop("STATE_DIR", None)
    elif k == "state-missing":
        fx.state_missing = True
    elif k == "state-file":
        fx.state_file = True
    elif k == "counts":
        for i in range(rng.randint(1, 4)):
            fx.put(fx.state + "/runs/r%d.md" % i, "x")
        fx.put(fx.state + "/runs/.hidden", "x")
        fx.mkdir(fx.state + "/receipts")
    elif k == "readonly":
        fx.chmods.append((fx.state, 0o555))
    r = rng.choice(["none"] * 2 + ["merged", "closed", "open", "lookup-fails", "non-numeric", "mixed"])
    fx.notes.append("receipts=" + r)
    if r != "none":
        specs = {"merged": [(12, "MERGED")], "closed": [(13, "CLOSED")], "open": [(14, "OPEN")],
                 "lookup-fails": [(15, None)], "non-numeric": [("abc", "MERGED"), (16, "MERGED")],
                 "mixed": [(12, "MERGED"), (14, "OPEN"), (9, "CLOSED"), (100, "MERGED")]}[r]
        for pr, state in specs:
            fx.put(fx.state + "/receipts/%s.json" % pr, "{}")
            if state is None:
                fx.answer("pr-view.%s" % pr, "boom\n", 1)
            elif str(pr).isdigit():
                fx.answer("pr-view.%s" % pr, state + "\n", 0)
    l = rng.choice(["none"] * 2 + ["fresh", "forgotten", "two", "unreadable", "inside", "hours-env", "null",
                                    "other-files", "string-int", "float", "bool", "bad-hours", "huge", "dir",
                                    "empty-hours"])
    fx.notes.append("leases=" + l)
    if l == "none":
        return
    ln = fx.state + "/leases/"
    mk = lambda pr, renewed: fx.put(ln + "pr-%s.json" % pr, json.dumps({"renewed": renewed, "pr": 1}))
    if l == "fresh":
        mk(1, int(ago_epoch(fx.t0, 2)))
    elif l == "forgotten":
        mk(2, int(ago_epoch(fx.t0, 13)))
    elif l == "two":
        mk(2, int(ago_epoch(fx.t0, 13))); mk(11, int(ago_epoch(fx.t0, 40))); mk(3, int(ago_epoch(fx.t0, 1)))
    elif l == "unreadable":
        fx.put(ln + "pr-4.json", "not json"); fx.put(ln + "pr-5.json", json.dumps({"x": 1}))
        fx.put(ln + "pr-6.json", json.dumps({"renewed": "abc"}))
    elif l == "inside":
        mk(7, int(ago_epoch(fx.t0, 11)))
    elif l == "hours-env":
        mk(8, int(ago_epoch(fx.t0, 3)))
        fx.extra_env["MAINTAINER_DOCTOR_LEASE_HOURS"] = rng.choice(["1", "0.5", "24"])
    elif l == "null":
        mk(9, None); fx.put(ln + "pr-10.json", "[1]")
    elif l == "other-files":
        mk(2, int(ago_epoch(fx.t0, 13))); fx.put(ln + "notes.json", "{}"); fx.put(ln + "pr-x.txt", "{}")
        fx.put(ln + ".pr-hidden.json", "{}")
    elif l == "string-int":
        mk(2, str(int(ago_epoch(fx.t0, 13)))); mk(3, " %d " % int(ago_epoch(fx.t0, 14)))
    elif l == "float":
        mk(2, float(ago_epoch(fx.t0, 13)) + 0.9)
    elif l == "bool":
        mk(2, True)
    elif l == "bad-hours":
        mk(2, int(ago_epoch(fx.t0, 13)))
        fx.extra_env["MAINTAINER_DOCTOR_LEASE_HOURS"] = "abc"
    elif l == "empty-hours":
        mk(2, int(ago_epoch(fx.t0, 13)))
        fx.extra_env["MAINTAINER_DOCTOR_LEASE_HOURS"] = ""
    elif l == "huge":
        fx.put(ln + "pr-2.json", '{"renewed": 1e999}')
    elif l == "dir":
        fx.mkdir(ln + "pr-3.json")


def d_token(fx, rng):
    k = rng.choice(["clean"] * 2 + ["logs", "runs", "drafts", "state", "receipts", "top-level-skipped", "no-subdirs",
                                     "session", "five", "short", "gh-fails", "error-text", "no-account",
                                     "binary", "multiline", "sixteen", "state-subdir-and-session"])
    fx.notes.append("token=" + k)
    st = fx.state
    tok = TOKEN
    if k in ("logs", "runs", "drafts", "state", "receipts"):
        fx.put(st + "/%s/a.log" % k, "GH_TOKEN=%s\n" % tok)
        fx.mkdir(st + "/logs")
    elif k == "clean":
        fx.put(st + "/logs/a.log", "nothing here\n")
    elif k == "top-level-skipped":
        fx.mkdir(st + "/logs"); fx.put(st + "/stray.txt", tok + "\n")
    elif k == "no-subdirs":
        fx.put(st + "/stray.txt", tok + "\n")
    elif k == "session":
        fx.put(fx.home + "/.claude/projects/" + fx.repo.replace("/", "-").replace(".", "-") + "/t.jsonl", tok + "\n")
    elif k == "five":
        for i in range(1, 8):
            fx.put(st + "/logs/f%d.log" % i, tok + "\n")
    elif k == "short":
        fx.answer("auth-token", "shorttoken12345\n")
    elif k == "sixteen":
        fx.answer("auth-token", "abcdefghijklmnop\n")
        fx.put(st + "/logs/a.log", "abcdefghijklmnop\n")
    elif k == "gh-fails":
        fx.answer("auth-token", "", 1)
    elif k == "error-text":
        fx.answer("auth-token", "no oauth token found for account testuser\n", 1)
        fx.put(st + "/logs/a.log", "no oauth token found for account testuser\n")
    elif k == "no-account":
        fx.profile_vars.pop("GH_ACCOUNT", None)
    elif k == "binary":
        fx.put(st + "/logs/b.bin", b"\x00\x01" + tok.encode() + b"\x00")
    elif k == "multiline":
        fx.answer("auth-token", "first-line-of-the-token-0000\nsecond-line-aaaaaaaa\n")
        fx.put(st + "/logs/a.log", "xx second-line-aaaaaaaa yy\n")
    elif k == "state-subdir-and-session":
        fx.put(st + "/logs/a.log", tok)
        fx.put(fx.home + "/.claude/projects/" + fx.repo.replace("/", "-").replace(".", "-") + "/t.jsonl", tok)


def d_tasks(fx, rng):
    k = rng.choice(["none"] * 3 + ["never", "no-reason", "garbage", "null-reason", "newer-failure", "recovered",
                                    "tie", "inside-interval", "outside-interval", "zero-interval", "two",
                                    "hyphen", "num-reason", "list-reason", "min-text", "tied-ns"])
    fx.notes.append("tasks=" + k)
    s = fx.state + "/state/"

    def failed(task, hours, reason="boom"):
        body = json.dumps({"reason": reason}) if reason is not ... else "garbage"
        fx.put(s + "failed-%s.json" % task, body, mtime=ago_epoch(fx.t0, hours))

    def last(task, hours):
        fx.put(s + "last-%s.json" % task, "{}", mtime=ago_epoch(fx.t0, hours))

    if k == "never":
        failed("review", 5)
    elif k == "no-reason":
        fx.put(s + "failed-review.json", "{}", mtime=ago_epoch(fx.t0, 5))
    elif k == "garbage":
        failed("review", 5, ...)
    elif k == "null-reason":
        failed("review", 5, None)
    elif k == "newer-failure":
        last("review", 30); failed("review", 5)
    elif k == "recovered":
        failed("review", 30); last("review", 5)
    elif k == "tie":
        fx.put(s + "failed-review.json", "{}", mtime=ago_epoch(fx.t0, 5))
        fx.put(s + "last-review.json", "{}", mtime=ago_epoch(fx.t0, 5))
    elif k == "tied-ns":
        fx.put(s + "failed-review.json", "{}", mtime=ago_epoch(fx.t0, 5) + 0.5)
        fx.put(s + "last-review.json", "{}", mtime=ago_epoch(fx.t0, 5))
    elif k in ("inside-interval", "outside-interval", "zero-interval"):
        last("review", 3); failed("review", 1)
        fx.put(s + "failed-review.json", json.dumps({"reason": "network down"}), mtime=ago_epoch(fx.t0, 1))
        fx.profile_vars["MIN_HOURS_review"] = {"inside-interval": "6", "outside-interval": "3", "zero-interval": "0"}[k]
        if k == "outside-interval":
            fx.put(s + "last-review.json", "{}", mtime=ago_epoch(fx.t0, 6))
            fx.put(s + "failed-review.json", json.dumps({"reason": "x"}), mtime=ago_epoch(fx.t0, 2))
    elif k == "two":
        failed("issues", 7); failed("alpha", 9)
    elif k == "hyphen":
        failed("read-only", 1); last("read-only", 2)
        fx.put(s + "failed-read-only.json", "{}", mtime=ago_epoch(fx.t0, 1))
        fx.put(s + "last-read-only.json", "{}", mtime=ago_epoch(fx.t0, 40))
    elif k == "num-reason":
        failed("review", 5, 42)
    elif k == "list-reason":
        failed("review", 5, ["a", {"b": 1.5, "c": None, "d": True}])
    elif k == "min-text":
        fx.put(s + "last-review.json", "{}", mtime=ago_epoch(fx.t0, 40))
        fx.put(s + "failed-review.json", "{}", mtime=ago_epoch(fx.t0, 1))
        fx.profile_vars["MIN_HOURS_review"] = "abc"


def d_deployed(fx, rng):
    k = rng.choice(["clean"] * 3 + ["no-stamp", "half", "gone", "dirty", "behind", "unknown", "full-hash",
                                     "multi-line"])
    fx.notes.append("deployed=" + k)
    fx.version_repo = k


DIMENSIONS = [d_os, d_args, d_profiles, d_install, d_wall, d_backend, d_commands, d_identity, d_post, d_repo,
              d_runs, d_scheduler, d_podman, d_sandbox, d_audit, d_token, d_tasks, d_deployed]


# --- writing a host to disk -------------------------------------------------------

def sh(cmd, cwd=None, env=None, check=True):
    e = dict(os.environ)
    e.update({"GIT_AUTHOR_DATE": GIT_DATE, "GIT_COMMITTER_DATE": GIT_DATE, "GIT_CONFIG_NOSYSTEM": "1",
              "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
              "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t", "LC_ALL": "C"})
    for k in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY", "GIT_COMMON_DIR"):
        e.pop(k, None)
    if env:
        e.update(env)
    r = subprocess.run(cmd, cwd=cwd, env=e, capture_output=True, text=True)
    if check and r.returncode != 0:
        raise SystemExit("fixture command failed: %s\n%s" % (cmd, r.stderr))
    return r


def build(fx):
    case = fx.case
    if os.path.lexists(case):
        rm_rf(case)
    for d in ("bin", "tools", "ans", "tmp", "cwd", "home"):
        os.makedirs(case + "/" + d, exist_ok=True)
    # stubs
    stub_text = STUB.replace("@LOGGED@", " ".join(LOGGED_ENV)).replace("@GIT@", REAL_GIT)
    present = set(fx.tools)
    present.add("grep")
    for t in sorted(present):
        p = case + "/bin/" + t
        if t == "grep":
            body = GREP_STUB.replace("@GREP@", REAL_GREP)
        elif t == "git" and getattr(fx, "git_stub", None) == "init-fails":
            body = stub_text.replace('git) exec "%s" "$@" ;;' % REAL_GIT,
                                      'git) case "${1:-}" in init) exit 1 ;; esac; exec "%s" "$@" ;;' % REAL_GIT)
        else:
            body = stub_text
        with open(p, "w") as f:
            f.write(body)
        os.chmod(p, 0o755)
    for t in PLAIN_TOOLS:
        if t == "grep":
            continue
        os.symlink(real(t), case + "/tools/" + t)
    # the cases of the shipped (not generated) tools the doctor looks up by name
    for path, content, mode, mtime in fx.files:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            f.write(content)
        os.chmod(path, mode)
        if mtime is not None:
            os.utime(path, (mtime, mtime))
    for d in fx.dirs:
        os.makedirs(d, exist_ok=True)
    for path in getattr(fx, "remove", []):
        if os.path.isdir(path):
            shutil.rmtree(path)
        elif os.path.exists(path):
            os.remove(path)
    # answers
    for key, (out, rc, err) in fx.ans.items():
        base = case + "/ans/" + key
        if out is not None:
            with open(base + ".out", "w") as f:
                f.write(out)
        if err is not None:
            with open(base + ".err", "w") as f:
                f.write(err)
        with open(base + ".rc", "w") as f:
            f.write(str(rc))
    # profile.env
    if not getattr(fx, "drop_profile_env", False):
        with open(fx.pdir + "/profile.env", "w") as f:
            for k, v in fx.profile_vars.items():
                f.write("%s=%s\n" % (k, shlex_quote(v)))
    else:
        shutil.rmtree(fx.pdir, ignore_errors=True)
    if getattr(fx, "no_share", False):
        shutil.rmtree(fx.share, ignore_errors=True)
    # state shape
    if getattr(fx, "state_missing", False):
        shutil.rmtree(fx.state, ignore_errors=True)
    if getattr(fx, "state_file", False):
        shutil.rmtree(fx.state, ignore_errors=True)
        with open(fx.state, "w") as f:
            f.write("x")
    # the repository the profile points at
    gs = fx.git_setup
    if gs in ("plain", "dirty", "ahead", "no-origin", "dirty-ahead", "clean"):
        make_repo(fx.repo, gs)
    elif gs == "nogit":
        os.makedirs(fx.repo, exist_ok=True)
    # VERSION and the tree it names
    make_version(fx)
    for path, mode in fx.chmods:
        os.chmod(path, mode)


def shlex_quote(v):
    import shlex
    return shlex.quote(str(v))


def make_repo(path, kind):
    os.makedirs(path, exist_ok=True)
    sh(["git", "init", "-q", "-b", "main", path])
    with open(path + "/README.md", "w") as f:
        f.write("hello\n")
    sh(["git", "-C", path, "add", "README.md"])
    sh(["git", "-C", path, "commit", "-qm", "base"])
    if kind != "no-origin":
        sh(["git", "-C", path, "update-ref", "refs/remotes/origin/main", "HEAD"])
    if kind in ("ahead", "dirty-ahead"):
        for i in range(2):
            with open(path + "/a%d.txt" % i, "w") as f:
                f.write("x")
            sh(["git", "-C", path, "add", "."])
            sh(["git", "-C", path, "commit", "-qm", "c%d" % i])
    if kind in ("dirty", "dirty-ahead"):
        for i in range(3):
            with open(path + "/u%d.txt" % i, "w") as f:
                f.write("u")


def make_version(fx):
    k = fx.version_repo
    src = fx.case + "/src"
    version = fx.share + "/VERSION"
    if not os.path.isdir(fx.share):
        return
    if k == "no-stamp":
        return
    make_repo(src, "clean")
    head = sh(["git", "-C", src, "rev-parse", "--short", "HEAD"]).stdout.strip()
    full = sh(["git", "-C", src, "rev-parse", "HEAD"]).stdout.strip()
    text = {"clean": "commit=%s\ninstalled_from=%s\n" % (head, src),
            "half": "commit=%s\n" % head,
            "gone": "commit=%s\ninstalled_from=%s/nonexistent\n" % (head, fx.case),
            "dirty": "commit=%s\ninstalled_from=%s\n" % (head, src),
            "behind": "commit=%s\ninstalled_from=%s\n" % (head, src),
            "unknown": "commit=%s\ninstalled_from=%s\n" % ("0123abc", src),
            "full-hash": "commit=%s\ninstalled_from=%s\n" % (full, src),
            "multi-line": "commit=%s\ncommit=%s\ninstalled_from=%s\n" % (head, head, src)}[k]
    if k == "dirty":
        with open(src + "/dirty.txt", "w") as f:
            f.write("d")
    if k == "behind":
        old = head
        with open(src + "/b.txt", "w") as f:
            f.write("b")
        sh(["git", "-C", src, "add", "."])
        sh(["git", "-C", src, "commit", "-qm", "newer"])
        text = "commit=%s\ninstalled_from=%s\n" % (old, src)
    os.makedirs(os.path.dirname(version), exist_ok=True)
    with open(version, "w") as f:
        f.write(text)


# --- running and comparing -----------------------------------------------------------

def run_doctor(fx, program, mode):
    env = {"PATH": fx.case + "/bin:" + fx.case + "/tools", "HOME": fx.home, "LC_ALL": "C", "TZ": "UTC",
           "LOGNAME": "fixture-user", "USER": "fixture-user", "TMPDIR": fx.case + "/tmp",
           "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_TERMINAL_PROMPT": "0",
           "CZ_CASE_DIR": fx.case}
    for k in fx.unset_env:
        env.pop(k, None)
    env.update(fx.extra_env)
    cmd = ["env", "-i"] + ["%s=%s" % kv for kv in env.items()] + ["timeout", "120", "bash", program] + fx.args
    r = subprocess.run(cmd, cwd=fx.case + "/cwd", capture_output=True, stdin=subprocess.DEVNULL)
    return r.returncode, r.stdout, r.stderr


def snapshot(case):
    rows = []
    skip_top = {"calls.log", "grepcalls", "stubs.all", "ans", "bin", "tools", "unplanned"}
    for root, dirs, files in os.walk(case):
        rel_root = os.path.relpath(root, case)
        if rel_root == ".":
            dirs[:] = [d for d in dirs if d not in skip_top]
            files = [f for f in files if f not in skip_top]
        dirs[:] = sorted(d for d in dirs if d != ".git")
        for name in sorted(files) + dirs:
            p = os.path.join(root, name)
            rp = os.path.relpath(p, case)
            st = os.lstat(p)
            if os.path.islink(p):
                rows.append((rp, "link", os.readlink(p)))
            elif os.path.isdir(p):
                rows.append((rp, "dir", oct(st.st_mode & 0o7777)))
            else:
                with open(p, "rb") as f:
                    rows.append((rp, "file", oct(st.st_mode & 0o7777), hashlib.sha256(f.read()).hexdigest()))
    return rows


TMP_RE = re.compile(rb"tmp\.[A-Za-z0-9]{10}")
LINE_RE = re.compile(rb"^.*?maintainer-doctor: line (\d+): ", re.M)
NOTFOUND_RE = re.compile(rb"^.*command not found\n", re.M)


def norm(data):
    data = TMP_RE.sub(b"tmp.X", data)
    data = LINE_RE.sub(rb"maintainer-doctor: line \1: ", data)
    data = NOTFOUND_RE.sub(b"", data)
    return data


def read(path):
    try:
        with open(path, "rb") as f:
            return f.read()
    except FileNotFoundError:
        return b""


def one_run(fx, program):
    build(fx)
    rc, out, err = run_doctor(fx, program, None)
    calls = norm(read(fx.case + "/calls.log"))
    grep_calls = read(fx.case + "/grepcalls")
    all_logs = read(fx.case + "/stubs.all") + read(fx.case + "/calls.log")
    return {"rc": rc, "stdout": norm(out), "stderr": norm(err), "calls": calls, "state": snapshot(fx.case),
            "grep": grep_calls, "all_logs": all_logs, "raw_out": out + err,
            "unplanned": read(fx.case + "/unplanned")}


def compare(a, b):
    diffs = []
    for key in ("rc", "stdout", "stderr", "calls", "state"):
        if a[key] != b[key]:
            diffs.append(key)
    return diffs


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seed", type=int, default=int(time.time()) % 100000)
    ap.add_argument("--cases", type=int, default=520)
    ap.add_argument("--only", type=int)
    ap.add_argument("--bash-tree", default=os.path.abspath(os.path.join(os.path.dirname(__file__), "../../../..")))
    ap.add_argument("--rust-tree", required=True)
    ap.add_argument("--scratch", default=os.path.expanduser("~/.cache/magent-wt-scratch/p1i/diff"))
    ap.add_argument("--verbose", action="store_true")
    a = ap.parse_args()
    bash_prog = a.bash_tree + "/bin/maintainer-doctor"
    rust_prog = a.rust_tree + "/bin/maintainer-doctor"
    for p in (bash_prog, rust_prog):
        if not os.path.isfile(p):
            raise SystemExit("no program at " + p)
    print("seed=%d cases=%d" % (a.seed, a.cases))
    os.makedirs(a.scratch, exist_ok=True)
    totals = {"cases": 0, "differences": 0, "pinned": 0, "fields": 0}
    shown = 0
    kinds = {}
    for i in range(a.cases):
        if a.only is not None and i != a.only:
            continue
        rng = random.Random("%d-%d" % (a.seed, i))
        case = "%s/c%d" % (a.scratch, i % 4)
        t0 = int(time.time())
        fx = Fx(case, t0)
        fx.profile = rng.choice(["magent"] * 5 + ["a.b", "x-y_z"])
        healthy(fx, rng)
        for dim in DIMENSIONS:
            if rng.random() < 0.55 or dim in (d_os, d_args):
                dim(fx, rng)
        # profile.env is written after every dimension has had its say
        b = one_run(fx, bash_prog)
        r = one_run(fx, rust_prog)
        diffs = compare(b, r)
        totals["cases"] += 1
        totals["fields"] += 5
        token = TOKEN.encode()
        pinned = False
        # F4: the bash argv carries the token; the Rust run carries it nowhere.
        if token in r["all_logs"] or token in r["grep"] or token in r["raw_out"]:
            diffs.append("token-leaked-in-rust-run")
        bash_f4 = any(token in line and b"-rlaF" in line for line in b["grep"].splitlines())
        # the scanned patterns of other tokens (the multi-line and 16-char cases) are pinned the same way
        bash_f4 = bash_f4 or any(b"-rlaF" in line for line in b["grep"].splitlines())
        if bash_f4:
            pinned = True
        if b["unplanned"] != r["unplanned"]:
            diffs.append("unplanned")
        if diffs:
            totals["differences"] += 1
            for d in diffs:
                kinds[d] = kinds.get(d, 0) + 1
            if shown < 12:
                shown += 1
                print("DIFFERENCE case %d fields=%s notes=%s" % (i, diffs, fx.notes))
                if "stdout" in diffs:
                    print("  bash stdout: %r" % b["stdout"][-300:])
                    print("  rust stdout: %r" % r["stdout"][-300:])
                if "stderr" in diffs:
                    print("  bash stderr: %r" % b["stderr"][:300])
                    print("  rust stderr: %r" % r["stderr"][:300])
                if "rc" in diffs:
                    print("  rc bash=%s rust=%s" % (b["rc"], r["rc"]))
                if "calls" in diffs:
                    bl, rl = b["calls"].splitlines(), r["calls"].splitlines()
                    print("  calls bash=%d rust=%d; first diff: %s" % (len(bl), len(rl), next(
                        ((x, y) for x, y in zip(bl + [b""] * 3, rl + [b""] * 3) if x != y), None)))
                if "state" in diffs:
                    sa, sb = set(map(tuple, b["state"])), set(map(tuple, r["state"]))
                    print("  state only bash: %s; only rust: %s" % (sorted(sa - sb)[:3], sorted(sb - sa)[:3]))
        elif pinned:
            totals["pinned"] += 1
        if a.verbose:
            print("case %d %s %s" % (i, "ok" if not diffs else "DIFF", fx.notes))
        rm_rf(case)
    print("comparisons: %d (%d cases x 5 fields: stdout, stderr, exit code, call log, state tree)" % (
        totals["fields"], totals["cases"]))
    print("cases with a difference: %d %s" % (totals["differences"], kinds))
    print("cases where the bash grep argv carried a token scan (pinned, decision P1I-F4): %d" % totals["pinned"])
    ok = totals["differences"] == 0 and totals["fields"] > 400 and a.only is None
    print("RESULT: %s" % ("PASS" if ok else "FAIL"))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
