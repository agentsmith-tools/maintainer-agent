#!/usr/bin/env python3
"""Replay the mutation proof for the install-family characterization cases.

  python3 tests/characterization/mutants-install.py                 # every mutant
  python3 tests/characterization/mutants-install.py busy-exit-code key-mode

Each mutant is applied to a fresh, touched copy of the checkout files these cases
read (install.sh, new-profile.sh, lib, scripts, bin, profiles, platform, evals and
tests; the checkout itself is never edited). The old text must occur exactly once,
the new file is read back and parsed, and the mutant is run against the case file
named for its behaviour group. A mutant is CAUGHT when at least one case fails.
The pristine copy is run first and last and its tree hash is compared.

Each run takes 20 to 40 seconds, so the full list takes about an hour. Run it one
heavy job at a time (scripts/with-hook-lock.sh).

Mutants never call a real scheduler: the absolute-path mutant names a path that
does not exist on a normal machine.
"""
import hashlib, os, re, shutil, subprocess, sys, tempfile, time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
ITEMS = ["install.sh", "new-profile.sh", "lib", "scripts", "bin", "profiles", "platform", "evals", "tests"]
I, N, C, L, A, R = "install.sh", "new-profile.sh", "platform/posix/install-cron.sh", "platform/macos/install-launchd.sh", "platform/linux/alert.sh", "platform/linux/run-instance.sh"
E, P = "evals/run-evals.sh", "tests/containment-probe.sh"
DEP, BUS, UNI, NEW, PLT, EVA, ISO = ("install-deploy.sh", "install-busy.sh", "install-uninstall.sh", "install-newprofile.sh",
                                     "install-platform.sh", "install-evals.sh", "install-isolation.sh")
GUARD = 'if [ "$dry" = 0 ] && [ "$force" = 0 ]; then\n    _busy=""'
GREP_NEEDLE = 'grep -qiF -- "$needle"'

# id, file, old text (exactly once), new text, case file
MUTANTS = [
    ("flags-exit-code", I, '*) echo "install.sh: unknown flag $a" >&2; exit 64 ;;', '*) echo "install.sh: unknown flag $a" >&2; exit 2 ;;', DEP),
    ("flags-accept-bogus", I, '        --force)     force=1 ;;', '        --force)     force=1 ;;\n        --bogus) ;;', DEP),
    ("dryrun-runs-commands", I, 'run() { if [ "$dry" = 1 ]', 'run() { if [ "$dry" = 7 ]', DEP),
    ("dryrun-no-stamp-line", I, 'if [ "$dry" = 1 ]; then say "would stamp $share/VERSION"; else', 'if [ "$dry" = 7 ]; then say "would stamp $share/VERSION"; else', DEP),
    ("key-mode", I, '    chmod 600 "$share/receipt.key"', '    chmod 644 "$share/receipt.key"', DEP),
    ("key-length", I, 'head -c 32 /dev/urandom', 'head -c 16 /dev/urandom', DEP),
    ("key-overwrite", I, 'elif [ ! -f "$share/receipt.key" ]; then', 'elif true; then', DEP),
    ("key-constant", I, 'head -c 32 /dev/urandom', 'printf abcdefghijklmnopqrstuvwxyz012345', DEP),
    ("version-dirty", I, 'describe --tags --always --dirty', 'describe --tags --always', DEP),
    ("version-from", I, '"$(date -Is)" "$root"', '"$(date -Is)" "$share"', DEP),
    ("template-deployed", I, 'run rm -rf "$share/profiles/_template"', 'true', DEP),
    ("nest-profiles", I, 'run rm -rf "$share/backends" "$share/profiles"', 'true', DEP),
    ("wall-home-arg", I, 'render-settings.py" "$pd" "$HOME"', 'render-settings.py" "$pd" "/x"', DEP),
    ("wall-fail-exit", I, 'echo "install.sh: generating the deny wall failed" >&2; exit 1', 'echo "install.sh: generating the deny wall failed" >&2; exit 0', DEP),
    ("landing-check-off", I, 'if [ "$dry" = 0 ] && ! ls "$share"/profiles/*/profile.env >/dev/null 2>&1; then', 'if false; then', DEP),
    ("landing-check-needs-sysknife", I, 'if [ "$dry" = 0 ] && ! ls "$share"/profiles/*/profile.env >/dev/null 2>&1; then', 'if [ "$dry" = 0 ] && [ ! -d "$share/profiles/sysknife" ]; then', DEP),
    ("exec-bit", I, '    run chmod +x "$share/alert.sh"', '    true', DEP),
    ("darwin-timers", I, '    say "note: launchd has no Persistent=true; a missed run does not catch up"\n    timers=0', '    say "note: launchd has no Persistent=true; a missed run does not catch up"', DEP),
    ("timers-stamp", I, '        run touch "$HOME/.local/share/systemd/timers/stamp-$name"', '        true', DEP),
    ("timers-none-exit", I, '--timers matched no unit files under $root/platform/linux" >&2\n        exit 1', '--timers matched no unit files under $root/platform/linux" >&2\n        exit 0', DEP),
    ("timers-daemon-reload", I, '    run systemctl --user daemon-reload\n    # The units', '    true\n    # The units', DEP),
    ("busy-exit-code", I, '        exit 69', '        exit 70', BUS),
    ("busy-check-off", I, GUARD, 'if false; then\n    _busy=""', BUS),
    ("busy-force-ignored", I, GUARD, 'if [ "$dry" = 0 ]; then\n    _busy=""', BUS),
    ("busy-writes-first", I, GUARD, 'mkdir -p "$share"\n' + GUARD, BUS),
    ("busy-dry-run", I, GUARD, 'if [ "$force" = 0 ]; then\n    _busy=""', BUS),
    ("busy-flock-argv", I, 'flock -n "$_sd/run.lock" true', 'flock -w 1 "$_sd/run.lock" true', BUS),
    ("busy-names", I, '_busy="$_busy $(basename "$(dirname "$_env")")"', '_busy=" x"', BUS),
    ("uninstall-guard-code", I, '            exit 3', '            exit 4', UNI),
    ("uninstall-yes-value", I, '"${MAINTAINER_UNINSTALL_YES:-}" != 1 ]', '"${MAINTAINER_UNINSTALL_YES:-}" = 0 ]', UNI),
    ("uninstall-guard-count", I, 'if [ "${_live:-0}" -gt 0 ]; then', 'if [ "${_live:-0}" -gt 5 ]; then', UNI),
    ("uninstall-one-timer", I, 'if [ "${_live:-0}" -gt 0 ]; then', 'if [ "${_live:-0}" -gt 1 ]; then', UNI),
    ("uninstall-removes-mcp", I, 'for f in maintainer maintainer-merge maintainer-doctor maintainer-repo; do', 'for f in maintainer maintainer-merge maintainer-doctor maintainer-repo maintainer-mcp; do', UNI),
    ("uninstall-keeps-share", I, 'step run rm -rf "$share"', 'true', UNI),
    ("uninstall-share-not-a-step", I, 'step run rm -rf "$share"', 'run rm -rf "$share"', UNI),
    ("uninstall-exit-on-fail", I, '    say "removed: $share', '    [ -z "$_failed" ] || exit 1\n    say "removed: $share', UNI),
    ("uninstall-plist-glob", I, 'dev.maintainer.*.plist', 'dev.maintainer.sysknife.*.plist', UNI),
    ("uninstall-cron-step", I, '"$root/platform/posix/install-cron.sh" --remove', '"$root/platform/posix/install-cron.sh" --dry-run', UNI),
    ("uninstall-disable", I, 'step run systemctl --user disable --now', 'step run systemctl --user stop', UNI),
    ("np-usage-count", N, '[ $# -ge 4 ] || usage', '[ $# -ge 3 ] || usage', NEW),
    ("np-slug-subst", N, '        -e "s|__SLUG__|$slug|g" \\', '        -e "s|__SLUG__|x|g" \\', NEW),
    ("np-home-abbrev", N, '    "$HOME"/*) path_written', '    "$HOME") path_written', NEW),
    ("np-hour-step", N, '    hour=$((hour + 1))', '    hour=$((hour + 2))', NEW),
    ("np-exists-code", N, 'already exists" >&2; exit 1', 'already exists" >&2; exit 2', NEW),
    ("np-underscore-allowed", N, '    _*|*/*|"") echo', '    */*|"") echo', NEW),
    ("np-pipe-guard", N, 'case "$v" in *[\\|\\&]*) echo', 'case "$v" in *[\\&]*) echo', NEW),
    ("np-name-default", N, 'name="${5:-$account}"', 'name="${5:-x}"', NEW),
    ("np-placeholder-check-off", N, "if grep -rq '__[A-Z_]*__' \"$root/profiles/$profile\"; then", 'if false; then', NEW),
    ("np-placeholder-no-underscore", N, "if grep -rq '__[A-Z_]*__' \"$root/profiles/$profile\"; then", "if grep -rq '__[A-Z]*__' \"$root/profiles/$profile\"; then", NEW),
    ("np-timer-appended", N, '    cat > "$unit" <<UNIT', '    cat >> "$unit" <<UNIT', NEW),
    ("cron-hour-step", C, '        hour=$((hour + 1))', '        hour=$((hour + 2))', PLT),
    ("cron-no-profile-code", C, '    exit 64\nfi\nmarker', '    exit 1\nfi\nmarker', PLT),
    ("cron-strip", C, '    $0 == m " end" {skip=0}', '    $0 == m "X" {skip=0}', PLT),
    ("cron-path", C, 'cron_path="$HOME/.local/bin:$HOME/.cargo/bin:', 'cron_path="$HOME/.local/bin:', PLT),
    ("cron-no-logs-dir", C, '    mkdir -p "$STATE_DIR/logs"\n    { printf', '    { printf', PLT),
    ("cron-count", C, 'installed $(gen | grep -c run.sh)', 'installed $(gen | grep -c PATH)', PLT),
    ("launchd-runatload", L, '<key>RunAtLoad</key><false/>', '<key>RunAtLoad</key><true/>', PLT),
    ("launchd-unload-first", L, '    launchctl unload "$f" >/dev/null 2>&1 || true', '    true', PLT),
    ("launchd-minhours", L, '"${!min_var:-0}"', '"${!min_var:-1}"', PLT),
    ("launchd-remove-keeps-plist", L, '        rm -f "$f"\n        printf \'  removed', '        true\n        printf \'  removed', PLT),
    ("alert-profile-split", A, 'profile="${instance%-*}"', 'profile="${instance%%-*}"', PLT),
    ("alert-notify-default", A, 'MAINTAINER_NOTIFY:-on', 'MAINTAINER_NOTIFY:-off', PLT),
    ("alert-fallback", A, '[ -d "$dirs" ] || dirs=', '[ -d "$dirs" ] || dirs=x', PLT),
    ("alert-display", A, 'DISPLAY="${DISPLAY:-:1}"', 'DISPLAY="${DISPLAY:-:0}"', PLT),
    ("run-instance-split", R, 'task="${instance##*-}"', 'task="${instance#*-}"', PLT),
    ("run-instance-usage-text", R, 'usage: run-instance.sh <profile>-<task>}"', 'usage: run-instance.sh <profile>-<task> now}"', PLT),
    ("unit-onfailure", "platform/linux/maintainer@.service", 'OnFailure=maintainer-alert@%n.service', 'OnFailure=maintainer-alert@%i.service', PLT),
    ("evals-retirable", E, 'RETIRABLE="05-reserved-issue"', 'RETIRABLE="05-reserved-issue 01-prompt-injection"', EVA),
    ("evals-case-sensitive", E, GREP_NEEDLE, 'grep -qF -- "$needle"', EVA),
    ("evals-regex-needle", E, GREP_NEEDLE, 'grep -qi -- "$needle"', EVA),
    ("evals-ghost-ok", E, "bad \"mapping '$id' has no scenario file\"", 'true', EVA),
    ("evals-usage-code", E, 'usage: run-evals.sh [--static|--live [id]]" >&2; exit 64', 'usage: run-evals.sh [--static|--live [id]]" >&2; exit 2', EVA),
    ("evals-no-scenarios-threshold", E, '[ "$n" -gt 0 ] || bad "no scenarios found', '[ "$n" -gt 1 ] || bad "no scenarios found', EVA),
    ("probe-no-runtime-text", P, 'no container runtime; this probe measured nothing', 'no runtime', EVA),
    ("probe-prefers-docker", P, 'command -v podman >/dev/null 2>&1 && rt=podman', 'command -v docker >/dev/null 2>&1 && rt=docker', EVA),
    ("absolute-scheduler-path", I, '    run systemctl --user daemon-reload\n    # The units', '    run /opt/homebrew/bin/systemctl --user daemon-reload\n    # The units', ISO),
    ("harness-no-env-i", "tests/characterization/lib-inst.sh", 'env -i "${pairs[@]}"', 'env "${pairs[@]}"', ISO),
    ("harness-tripwire-off", "tests/characterization/lib.sh", '    if [ -s "$CZ_CASE/unplanned-tools" ]; then', '    if false; then', ISO),
]


def tree_hash(root):
    h = hashlib.sha256()
    for it in ITEMS:
        p = os.path.join(root, it)
        files = [p] if os.path.isfile(p) else sorted(os.path.join(d, f) for d, _, fs in os.walk(p) for f in fs)
        for f in files:
            if os.path.islink(f): continue
            h.update(f[len(root):].encode()); h.update(open(f, "rb").read())
    return h.hexdigest()


def fresh(work):
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(work)
    for it in ITEMS:
        src, dst = os.path.join(ROOT, it), os.path.join(work, it)
        if os.path.isdir(src): shutil.copytree(src, dst, symlinks=True)
        else: shutil.copy2(src, dst)
    shutil.rmtree(os.path.join(work, "tests", "diff"), ignore_errors=True)
    for base, _, fs in os.walk(work):
        for f in fs:
            if not os.path.islink(os.path.join(base, f)): os.utime(os.path.join(base, f))
    time.sleep(1.1)


def run(work, case_file):
    env = {k: v for k, v in os.environ.items() if k != "MX_TREE"}
    cp = subprocess.run(["bash", "-c", "ulimit -v 8000000; exec bash tests/run-characterization.sh " + case_file], cwd=work,
                        capture_output=True, text=True, env=env, timeout=900)
    out = cp.stdout + cp.stderr
    m = re.findall(r"^(\d+) cases, (\d+) passed, (\d+) failed", out, re.M)
    fails = [l[6:] for l in out.splitlines() if l.startswith("FAIL  ")]
    return (tuple(map(int, m[-1])) if m else None), fails


def main():
    want = sys.argv[1:]
    unknown = [w for w in want if w not in {m[0] for m in MUTANTS}]
    if unknown: sys.exit("unknown mutant: " + ", ".join(unknown))
    tmp = tempfile.mkdtemp(prefix="mut-install-")
    work = os.path.join(tmp, "w")
    fresh(work)
    pristine_hash = tree_hash(work)
    res, _ = run(work, DEP)
    print("pristine control before: %s" % (res,)); bad = 0
    if not res or res[2] != 0: bad += 1
    print("%-30s %-34s %s" % ("mutant", "file:line", "result (first red case)"))
    for mid, path, old, new, case_file in MUTANTS:
        if want and mid not in want: continue
        fresh(work)
        p = os.path.join(work, path); s = open(p).read()
        assert s.count(old) == 1, "%s: old text occurs %d times in %s" % (mid, s.count(old), path)
        line = s[:s.index(old)].count("\n") + 1
        open(p, "w").write(s.replace(old, new))
        back = open(p).read()
        assert back != s and new in back, mid + ": mutation did not land"
        if path.endswith((".sh", ".py")) or "/" not in path:
            assert subprocess.run(["bash", "-n", p]).returncode == 0, mid + ": does not parse"
        r, fails = run(work, case_file)
        caught = bool(r) and r[2] > 0 and bool(fails)
        if not caught: bad += 1
        print("%-30s %-34s %s" % (mid, "%s:%d" % (path.split("/")[-1], line),
              ("CAUGHT by %s (%d red) %s" % (case_file, r[2], fails[0][:60])) if caught else "SURVIVED (%s)" % (r,)), flush=True)
    fresh(work)
    res, _ = run(work, DEP)
    print("pristine control after: %s, tree hash unchanged: %s" % (res, tree_hash(work) == pristine_hash))
    if not res or res[2] != 0 or tree_hash(work) != pristine_hash: bad += 1
    shutil.rmtree(tmp, ignore_errors=True)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
