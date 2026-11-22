#!/usr/bin/env bash
# Proof for decision P4B-12: a SIGTERM that ends containment-probe removes its work directory, and the
# program ends with the status a shell reports for a death by that signal (143), as the script does.
#
#   signals.sh [TREE]    TREE is an assembled tree (rust/scripts/assemble-tree.sh); default: assembled here
#
# For each side (the bash script of this checkout, the Rust binary of the tree) it runs the probe with a
# runtime stub whose `run` blocks, waits until the stub holds the work directory, sends SIGTERM to the
# program by its pid (after reading the process's executable and command line), and checks the exit
# status and that TMPDIR holds nothing. The stub's own process is ended by its pid at the end.
# A second pair of runs sends SIGINT to the whole process group, as a terminal's Ctrl-C does (the run starts under
# a launcher of its own that gives the program a session and restores SIGINT, which a background job of a shell
# ignores): the stub's child must be gone afterwards on both sides, exit 130.
# A signal that names the Rust program alone must end the child too (decision P4B-12); for the bash script the
# line only reports what happened.
# Exit 0 when both sides agree and the directory is gone on both.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
scratch="${MX_SCRATCH:-$HOME/.cache/magent-wt-scratch}/mx-checks-signals"
rm -rf "$scratch"; mkdir -p "$scratch/tmp" "$scratch/bin" "$scratch/tools" || exit 2
trap 'rm -rf "$scratch"' EXIT
tree="${1:-}"
if [ -z "$tree" ]; then
    tree="$scratch/tree"
    built="$(bash "$root/rust/scripts/assemble-tree.sh" "$tree" containment-probe 2>&1)" || { printf '%s\n' "$built" >&2; exit 2; }
fi
for t in python3 bash sleep rm ls cat tr mkdir dirname grep sed env timeout mktemp wc tail cut head; do
    p="$(PATH="$PATH" type -P "$t")" && ln -s "$p" "$scratch/tools/$t"
done
cat > "$scratch/bin/podman" <<'STUB'
#!/usr/bin/env bash
# image inspect succeeds at once; the first run records its pid and the mount, then blocks.
case "${1:-}" in
    image) exit 0 ;;
    run)
        prev=""; for a in "$@"; do [ "$prev" = -v ] && echo "${a%%:*}" > "$SIG_DIR/mount"; prev="$a"; done
        echo $$ > "$SIG_DIR/stub.pid"
        exec sleep 60 ;;
esac
exit 0
STUB
chmod +x "$scratch/bin/podman"
cat > "$scratch/launch.py" <<'PY'
import signal, subprocess, sys
prog, pidfile, rcfile = sys.argv[1:4]
child = subprocess.Popen(["bash", prog], start_new_session=True, stdin=subprocess.DEVNULL,
                         preexec_fn=lambda: signal.signal(signal.SIGINT, signal.SIG_DFL))
open(pidfile, "w").write(str(child.pid))
code = child.wait()
open(rcfile, "w").write(str(128 - code if code < 0 else code))
PY
fail=0
one() {  # $1 = label, $2 = the program path (run as `bash PATH`), $3 = pid or group, $4 = the signal
    local label="$1" prog="$2" mode="${3:-pid}" sig="${4:-TERM}" pid stub
    rm -rf "$scratch/run"; mkdir -p "$scratch/run" "$scratch/tmp"; rm -rf "${scratch:?}/tmp"/*
    local launcher=""
    if [ "$mode" = group ]; then
        env -i PATH="$scratch/bin:$scratch/tools" HOME="$scratch" TMPDIR="$scratch/tmp" SIG_DIR="$scratch/run" LC_ALL=C \
            python3 "$scratch/launch.py" "$prog" "$scratch/run/gpid" "$scratch/run/grc" > "$scratch/run/out" 2> "$scratch/run/err" < /dev/null &
        launcher=$!
        for _ in $(seq 1 100); do [ -s "$scratch/run/gpid" ] && break; sleep 0.1; done
        pid="$(cat "$scratch/run/gpid" 2>/dev/null)"
        [ -n "$pid" ] || { echo "signals: $label: the launcher never started the program" >&2; fail=1; return; }
    else
        env -i PATH="$scratch/bin:$scratch/tools" HOME="$scratch" TMPDIR="$scratch/tmp" SIG_DIR="$scratch/run" LC_ALL=C \
            bash "$prog" > "$scratch/run/out" 2> "$scratch/run/err" < /dev/null &
        pid=$!
    fi
    for _ in $(seq 1 100); do [ -s "$scratch/run/stub.pid" ] && break; sleep 0.1; done
    if [ ! -s "$scratch/run/stub.pid" ]; then echo "signals: $label: the stub never started" >&2; fail=1; kill -KILL "$pid" 2>/dev/null; return; fi
    stub="$(cat "$scratch/run/stub.pid")"
    [ -d "$(cat "$scratch/run/mount")" ] || { echo "signals: $label: the work directory $(cat "$scratch/run/mount") is not there before the signal; TMPDIR holds [$(ls -A "$scratch/tmp" | tr "\n" " ")]" >&2; fail=1; }
    # The program is the process we started: its command line names the program path.
    if ! tr '\0' ' ' < "/proc/$pid/cmdline" | grep -qF -- "$prog"; then echo "signals: $label: pid $pid is not the program" >&2; fail=1; kill -KILL "$pid" 2>/dev/null; return; fi
    if [ "$mode" = group ]; then kill -"$sig" -- "-$pid"; else kill -"$sig" "$pid"; fi
    if [ "$mode" = group ]; then
        for _ in $(seq 1 150); do [ -s "$scratch/run/grc" ] && break; sleep 0.1; done
        rc="$(cat "$scratch/run/grc" 2>/dev/null)"; rc="${rc:-none}"; wait "$launcher"
    else
        wait "$pid"; rc=$?
    fi
    sleep 0.5
    RESULT_STUB=gone; kill -0 "$stub" 2>/dev/null && RESULT_STUB=ALIVE
    left="$(ls -A "$scratch/tmp" | tr '\n' ' ')"
    # End the stub's sleep, by its pid, after checking what it is.
    if [ "$(basename "$(readlink "/proc/$stub/exe" 2>/dev/null)")" = sleep ]; then kill -TERM "$stub" 2>/dev/null; fi
    printf '%s: exit %s, TMPDIR holds [%s], the stub child is %s\n' "$label" "$rc" "$left" "$RESULT_STUB"
    RESULT_RC="$rc"; RESULT_LEFT="$left"
}
one bash "$root/tests/containment-probe.sh"; brc="$RESULT_RC"; bleft="$RESULT_LEFT"
one rust "$tree/tests/containment-probe.sh"; rrc="$RESULT_RC"; rleft="$RESULT_LEFT"
[ "$brc" = 143 ] || { echo "signals: the bash script ended with $brc, not 143" >&2; fail=1; }
[ "$rrc" = "$brc" ] || { echo "signals: the Rust program ended with $rrc and the bash script with $brc" >&2; fail=1; }
[ -z "$bleft" ] || { echo "signals: the bash script left [$bleft]" >&2; fail=1; }
[ -z "$rleft" ] || { echo "signals: the Rust program left [$rleft]" >&2; fail=1; }
one bash-group "$root/tests/containment-probe.sh" group INT; gbrc="$RESULT_RC"; gbleft="$RESULT_LEFT"; gbstub="$RESULT_STUB"
one rust-group "$tree/tests/containment-probe.sh" group INT; grrc="$RESULT_RC"; grleft="$RESULT_LEFT"; grstub="$RESULT_STUB"
one rust-alone "$tree/tests/containment-probe.sh" pid TERM; arrc="$RESULT_RC"; arstub="$RESULT_STUB"
[ "$gbrc" = 130 ] || { echo "signals: the bash script ended with $gbrc after a group SIGINT, not 130" >&2; fail=1; }
[ "$grrc" = "$gbrc" ] || { echo "signals: the Rust program ended with $grrc after a group SIGINT, the bash script with $gbrc" >&2; fail=1; }
[ "$gbstub" = gone ] || { echo "signals: the bash script left its child alive after a group SIGINT" >&2; fail=1; }
[ "$grstub" = gone ] || { echo "signals: the Rust program left its child alive after a group SIGINT" >&2; fail=1; }
[ "$arstub" = gone ] || { echo "signals: the Rust program left its child alive after a SIGTERM that names it alone" >&2; fail=1; }
[ "$arrc" = 143 ] || { echo "signals: the Rust program ended with $arrc after its own SIGTERM, not 143" >&2; fail=1; }
[ -z "$gbleft$grleft" ] || { echo "signals: a group SIGINT left [$gbleft$grleft] in TMPDIR" >&2; fail=1; }
[ "$fail" = 0 ] && echo "signals: ok" || exit 1
