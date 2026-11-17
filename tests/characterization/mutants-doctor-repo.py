#!/usr/bin/env python3
"""Replay the mutation proof for the doctor and repo characterization cases.

  python3 tests/characterization/mutants-doctor-repo.py            # every mutant
  python3 tests/characterization/mutants-doctor-repo.py D-lease-limit R-fix-signoff

Each mutant is applied to a fresh, touched copy of bin/, lib/ and tests/ (the
checkout is never edited), read back, syntax-checked, and run against the case
files of its program. A mutant is CAUGHT when at least one case fails by
assertion. The checkout's two programs are hashed before and after.
Also runs each program replaced by `exit 0` and by `exit 1`: no case may pass.
"""
import hashlib, os, re, shutil, subprocess, sys, tempfile

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
DOC = ["doctor-core.sh", "doctor-ci-scheduler-podman.sh", "doctor-sandbox-audit.sh"]
REP = ["repo-prune.sh", "repo-release.sh", "repo-fixup.sh"]
D, R = "bin/maintainer-doctor", "bin/maintainer-repo"

# id, program, old text (must occur exactly once), new text
MUTANTS = [
    ("D-other-os-warmup", D, 'if [ "$(uname -s)" = Linux ] && [ -f "$HOME/.config/systemd/user/podman-userns', 'if [ "$(uname -s)" != Darwin ] && [ -f "$HOME/.config/systemd/user/podman-userns'),
    ("D-other-os-run-instance", D, '[ "$f" = run-instance.sh ] && [ "$(uname -s)" != Linux ]', '[ "$f" = run-instance.sh ] && [ "$(uname -s)" = Darwin ]'),
    ("D-lease-limit", D, 'limit = float(os.environ["LEASE_H"]) * 3600', 'limit = float(os.environ["LEASE_H"]) * 3000'),
    ("D-lease-typeerror", D, 'except (OSError, ValueError, KeyError, TypeError):', 'except (OSError, ValueError, KeyError):'),
    ("D-token-drafts", D, 'for _d in logs runs drafts state receipts; do', 'for _d in logs runs state receipts; do'),
    ("D-token-state", D, 'for _d in logs runs drafts state receipts; do', 'for _d in logs runs drafts receipts; do'),
    ("R-behind-one", R, 'if [ "${_behind:-0}" -gt 0 ]; then', 'if [ "${_behind:-0}" -gt 1 ]; then'),
    ("R-fix-break", R, '*[!0-9]*|"") pr="" ; break ;;', '*[!0-9]*|"") pr="" ;;'),
    ("R-fix-signoff", R, "grep -q '^Signed-off-by:' <<<\"$msgs\"", "grep -q 'Signed-off-by:' <<<\"$msgs\""),
    ("R-fix-count-trailing", R, '("(^|[^0-9])" n "([^0-9]|$)")', '("(^|[^0-9])" n)'),
    ("R-open-pr-guard-alive", R, 'grep -qx -F -- "$br" <<<"$open_heads"', 'grep -q -F -- "$br" <<<"$open_heads"'),
    ("D-token-argv-free", D, 'grep -rlaF -e "$_tok"', 'grep -rlF -e "$_tok"'),
    ("R-since-count", R, 'say "commits since: $n"', 'say "commits since: ${n}0"'),
]


def sha(p): return hashlib.sha256(open(p, "rb").read()).hexdigest()

def fresh(work):
    shutil.rmtree(work, ignore_errors=True)
    for d in ("bin", "lib", "tests"):
        shutil.copytree(os.path.join(ROOT, d), os.path.join(work, d), symlinks=True)
    for base, _, fs in os.walk(work):
        for f in fs: os.utime(os.path.join(base, f))

def run(work, files):
    env = {k: v for k, v in os.environ.items() if k != "MX_TREE"}
    cp = subprocess.run(["bash", work + "/tests/run-characterization.sh"] + files, capture_output=True, text=True, env=env, timeout=900)
    m = re.findall(r"^(\d+) cases, (\d+) passed, (\d+) failed", cp.stdout, re.M)
    fails = [l[6:] for l in cp.stdout.splitlines() if l.startswith("FAIL  ")]
    return (tuple(map(int, m[-1])) if m else None), fails

def main():
    want = sys.argv[1:]
    before = {p: sha(os.path.join(ROOT, p)) for p in (D, R)}
    tmp = tempfile.mkdtemp(prefix="mut-")
    work = os.path.join(tmp, "w")
    bad = 0
    print("%-26s %-22s %s" % ("mutant", "file:line", "result (first red case)"))
    for mid, prog, old, new in MUTANTS:
        if want and mid not in want: continue
        fresh(work)
        p = os.path.join(work, prog); s = open(p).read()
        assert s.count(old) == 1, mid + ": old text occurs %d times" % s.count(old)
        line = s[:s.index(old)].count("\n") + 1
        open(p, "w").write(s.replace(old, new))
        assert new in open(p).read() and open(p).read() != s, mid + ": mutation did not land"
        assert subprocess.run(["bash", "-n", p]).returncode == 0, mid + ": does not parse"
        res, fails = run(work, DOC if prog == D else REP)
        caught = bool(res) and res[2] > 0
        if not caught: bad += 1
        print("%-26s %-22s %s" % (mid, "%s:%d" % (prog.split("/")[-1], line), ("CAUGHT (%d red) %s" % (res[2], fails[0][:70])) if caught else "SURVIVED"))
    for prog, files in ((D, DOC), (R, REP)):
        for code in (0, 1):
            if want: continue
            fresh(work)
            p = os.path.join(work, prog); open(p, "w").write("#!/usr/bin/env bash\nexit %d\n" % code)
            res, fails = run(work, files)
            if res and res[1] > 0: bad += 1
            print("do-nothing %-22s exit %d: %s cases pass" % (prog.split("/")[-1], code, res[1] if res else "?"))
    after = {p: sha(os.path.join(ROOT, p)) for p in (D, R)}
    print("checkout programs unchanged:", before == after)
    shutil.rmtree(tmp, ignore_errors=True)
    return 1 if (bad or before != after) else 0

if __name__ == "__main__":
    sys.exit(main())
