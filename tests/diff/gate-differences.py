#!/usr/bin/env python3
"""Sort the differences of two recordings of maintainer-merge into named decisions.

    python3 tests/diff/gate-differences.py <out-bash> <out-rust> [scenario-dir]

compare.sh prints every difference as a diff. For the merge gate most of them are the same few
decisions repeated in hundreds of scenarios, so this tool applies the decisions as named rules, in
order, and reports what is left. A scenario is `same` when every field agrees after the rules of
the decisions that apply to it, `explained` when a rule rewrote a field of it, and `UNEXPLAINED`
otherwise; the diff of an unexplained field is printed in full.

Exit: 0 when nothing is unexplained, 1 when something is, 2 for a harness error (a missing
recording, a recording that holds no scenarios, two recordings that do not hold the same scenarios).
A rule never reads a field it does not name, so a difference that no rule names stays unexplained.

With the scenario directory the tool also reads each scenario's own declarations: a line
`# decision: ID` in the file, or content that names a decision (a non-UTF-8 variable, a null
`isCrossRepository`). A declared decision explains the differences of that scenario only when the
Rust recording shows the decision's own refusal (exit status and text), so the decision cannot hide
a different failure.

The rules are the decisions of the pull requests that built the Rust gate. Each rule rewrites BOTH
recordings of a field to the same text where the decision says the two programs differ, and nothing
else:

  P3B-7   the Rust gate locks with the standard library, so no `flock` program runs: the lines of
          the flock stub are dropped from calls.log and every flock.log field.
  P3B-W   the assembled Rust tree puts a wrapper in front of the binary, and the wrapper names the
          caller's variables in MAINTAINER_CHILD_ENV: that name is dropped from the env logs.
  N4      a `receipt` that fails on the head leaves no empty receipts directory in Rust (rv271b N4).
  P3W-5   the container runtime gets only the variables it needs (HOME, USER, LOGNAME, XDG_*,
          DBUS_SESSION_BUS_ADDRESS, CONTAINERS_*, TMPDIR, LC_ALL), where bash hands it the whole
          environment: LANG, TZ and the MAINTAINER_* names are dropped from the podman env log.
          GH_TOKEN, GITHUB_TOKEN, GH_HOST and GH_CONFIG_DIR stay in the comparison.
  P30-12  bash prints the suite's globs with a trailing blank in "sed matched no line"; Rust does not.
  P3A-6   a tool's own stderr (sed's refusal text) is not passed through in Rust.
  P3A-17  the paths a mutation changed are sorted in Rust; bash lists them in the order find walked.

Scenario decisions (the Rust recording must show the refusal):

  P30-3   a resource limit that is not a positive amount is refused (bash hands 0 to the runtime).
  P30-4   a timeout that is not a whole number of seconds above 0 is refused.
  P3W-6   a `gh` that failed after printing the changed files is an unreadable list in Rust (bash
          ignores the status of that call).
  P3B-13  a gate variable that is not UTF-8 text is a refusal naming it (bash reads the bytes).
  P3B-15  `isCrossRepository` that is null is unreadable (bash reads it as same-repository).
"""
import difflib
import os
import re
import sys

KNOWN_FIELD_RULES = []


def rule(name, reason, pair=False):
    """A rule is a function of one text, applied to both recordings, or (pair=True) of both."""
    def wrap(fn):
        both = fn if pair else (lambda field, ta, tb: (fn(field, ta), fn(field, tb)))
        KNOWN_FIELD_RULES.append((name, reason, both))
        return fn
    return wrap


@rule("P3B-7", "the Rust gate takes its locks without running flock")
def drop_flock(field, text):
    if field.endswith(".flock.log"):
        return ""
    if field == "calls.log":
        return "".join(l for l in text.splitlines(True) if not l.startswith("flock:"))
    return text


@rule("P3B-W", "the wrapper of the assembled tree sets MAINTAINER_CHILD_ENV")
def drop_child_env(field, text):
    if field.startswith("env.") and field.endswith(".log"):
        return re.sub(r" MAINTAINER_CHILD_ENV='[^']*'", "", text)
    return text


@rule("N4", "the empty receipts directory bash leaves after a refused receipt", pair=True)
def drop_empty_receipts_dir(field, ta, tb):
    """Only when that one line is the whole difference, so it never hides another."""
    line = "state/receipts\td\t755\t-\n"
    if field == "state.tree" and ta != tb and ta.replace(line, "") == tb.replace(line, ""):
        return ta.replace(line, ""), tb.replace(line, "")
    return ta, tb


@rule("P3W-5", "the container runtime gets an allowlist of variables, bash the whole environment")
def trim_podman_env(field, text):
    if field != "env.podman.log":
        return text
    keep = ("GH_HOST", "GH_CONFIG_DIR", "HOME", "LC_ALL", "GH_TOKEN", "GITHUB_TOKEN")
    out = []
    for line in text.splitlines(True):
        words = re.findall(r"[A-Z_]+=(?:'[^']*'|<[a-z]+>)", line)
        out.append("env: " + " ".join(w for w in words if w.split("=")[0] in keep) + "\n")
    return "".join(out)


@rule("P30-12", "the globs of the no-op refusal end in a blank in bash")
def strip_glob_blank(field, text):
    if field != "stderr":
        return text
    return re.sub(r"(this suite mutates:\n    [^\n]*?) +\n", r"\1\n", text)


@rule("P3A-6", "sed's own refusal text is not passed through in Rust")
def drop_sed_stderr(field, text):
    if field != "stderr":
        return text
    return "".join(l for l in text.splitlines(True) if not l.startswith("sed: "))


@rule("P3A-17", "the changed paths are sorted in Rust")
def sort_changed_paths(field, text):
    if field != "stderr":
        return text
    lines = text.splitlines(True)
    out, i = [], 0
    while i < len(lines):
        out.append(lines[i])
        if lines[i].rstrip("\n").endswith("mutation:") or lines[i].rstrip("\n").endswith("this pull request changed:"):
            j = i + 1
            while j < len(lines) and lines[j].startswith("    ") and lines[j].strip():
                j += 1
            out.extend(sorted(lines[i + 1:j]))
            i = j
            continue
        i += 1
    return "".join(out)


def stderr_has(*needles):
    return lambda rc, err: rc == "1" and all(n in err for n in needles)


SCENARIO_DECISIONS = {
    "P30-3": ("a limit that is not a positive amount is refused", stderr_has("which is not a positive")),
    "P30-4": ("a timeout that is not a whole number of seconds is refused", stderr_has("the container timeout is '", "which is not a whole number of seconds above 0")),
    "P3W-6": ("a gh that failed after printing the changed files is an unreadable list", stderr_has("could not read the changed files for #")),
    "P3B-13": ("a gate variable that is not text is a refusal naming it", lambda rc, err: rc == "64" and ("is not valid UTF-8 text" in err or "MAINTAINER_POST must be on or off" in err)),
    "P3B-15": ("a null isCrossRepository is unreadable", stderr_has("could not read 'head' of #")),
}


def declared(script):
    found = set(re.findall(r"^# decision: (\S+)", script, re.M))
    if "mx_env_bytes" in script:
        found.add("P3B-13")
    if 'isCrossRepository\\":null' in script:
        found.add("P3B-15")
    return found


def read(path):
    with open(path, encoding="utf-8", errors="surrogateescape", newline="") as fh:
        return fh.read()


def fields(root):
    found = {}
    for dirpath, _dirs, files in os.walk(root):
        for name in files:
            full = os.path.join(dirpath, name)
            found[os.path.relpath(full, root)] = full
    return found


def check_dir(label, path):
    if not os.path.isfile(os.path.join(path, ".complete")):
        sys.exit("gate-differences: %s recording '%s' has no .complete marker" % (label, path))
    names = sorted(n for n in os.listdir(path) if os.path.isdir(os.path.join(path, n)))
    if not names:
        sys.exit("gate-differences: %s recording '%s' holds no scenarios" % (label, path))
    return names


def main(argv):
    if len(argv) not in (2, 3):
        sys.exit(__doc__)
    old_dir, new_dir = argv[0], argv[1]
    scen_dir = argv[2] if len(argv) == 3 else None
    old_names, new_names = check_dir("old", old_dir), check_dir("new", new_dir)
    if old_names != new_names:
        sys.exit("gate-differences: the recordings hold different scenarios")
    tally = {"same": 0, "explained": 0, "UNEXPLAINED": 0}
    used = {}
    unexplained = []
    for name in old_names:
        a, b = fields(os.path.join(old_dir, name)), fields(os.path.join(new_dir, name))
        rewrote = set()
        bad = []
        for field in sorted(set(a) | set(b)):
            if field not in a or field not in b:
                bad.append((field, "present in only one recording"))
                continue
            ta, tb = read(a[field]), read(b[field])
            if ta == tb:
                continue
            for rname, _reason, fn in KNOWN_FIELD_RULES:
                na, nb = fn(field, ta, tb)
                if (na, nb) != (ta, tb):
                    rewrote.add(rname)
                ta, tb = na, nb
            if ta != tb:
                diff = "".join(difflib.unified_diff(
                    ta.splitlines(True), tb.splitlines(True), "bash/%s/%s" % (name, field), "rust/%s/%s" % (name, field)))
                bad.append((field, diff))
        if bad and scen_dir:
            script = read(os.path.join(scen_dir, name + ".sh"))
            ids = declared(script)
            rc, err = read(b["rc"]).strip(), read(b["stderr"])
            held = [d for d in sorted(ids) if d in SCENARIO_DECISIONS and SCENARIO_DECISIONS[d][1](rc, err)]
            if held:
                bad = []
                rewrote.update(held)
        if bad:
            tally["UNEXPLAINED"] += 1
            unexplained.append((name, bad))
        elif rewrote:
            tally["explained"] += 1
        else:
            tally["same"] += 1
        for r in rewrote:
            used[r] = used.get(r, 0) + 1
    for name, bad in unexplained:
        for field, diff in bad:
            print("UNEXPLAINED scenario=%s field=%s" % (name, field))
            print(diff.rstrip("\n"))
    print("scenarios %d: same %d, explained %d, UNEXPLAINED %d" % (
        len(old_names), tally["same"], tally["explained"], tally["UNEXPLAINED"]))
    for rname, reason, _fn in KNOWN_FIELD_RULES:
        print("  rule %-6s rewrote a field in %4d scenarios: %s" % (rname, used.get(rname, 0), reason))
    for did, (reason, _check) in SCENARIO_DECISIONS.items():
        print("  decision %-6s explained %4d scenarios: %s" % (did, used.get(did, 0), reason))
    sys.exit(1 if tally["UNEXPLAINED"] else 0)


if __name__ == "__main__":
    main(sys.argv[1:])
