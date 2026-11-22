#!/usr/bin/env bash
# Differential proof: the five Rust checks against the five bash scripts.
#
# differential.py builds, for each case, one generated sandbox (a tree, a HOME, a TMPDIR, a farm
# of real tools and logging stubs for every tool the program starts), runs the bash script in it,
# rebuilds the same sandbox in the same place, runs the Rust binary through a wrapper that stands
# where the script stood, and compares standard output, standard error, the exit code, the global
# call log (every stub call across all stubs: argv, working directory, whole environment, in
# order) and the state tree. Cases whose difference is a recorded decision are asserted on both
# sides and counted as explained.
#
# Usage: differential.sh
# Env:   MX_TREE           tree under test (default: this checkout); its scripts are the bash side
#        MX_SCRATCH        where the scratch directory goes (default ~/.cache/magent-wt-scratch)
#        DIFF_SEED         the seed (default: from the clock; printed, so a run repeats)
#        DIFF_PER_PROGRAM  generated cases per program (default 70: 350 comparisons and the explained ones)
#        DIFF_ONLY         comma-separated case name fragments (pins-0,probe-01)
#        DIFF_RUST_BIN_DIR a directory with the five release binaries (default: built from this checkout)
# Exit:  0 when 300 or more comparisons ran and every one matched or is explained, 1 on a
#        difference or a short run, 2 on a harness fault.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ws="$(cd "$here/../../.." && pwd)"
root="$(cd "$ws/.." && pwd)"
MX_TREE="${MX_TREE:-$root}"
seed="${DIFF_SEED:-$(date +%s)}"
seed=$(( seed % 1000000 ))
per="${DIFF_PER_PROGRAM:-70}"

for f in scripts/check_claims.sh scripts/shellcheck-sweep.sh scripts/verify-action-pins.sh scripts/render-settings.py evals/run-evals.sh tests/containment-probe.sh; do
    [ -f "$MX_TREE/$f" ] || { echo "differential: no $f under MX_TREE=$MX_TREE" >&2; exit 2; }
done
for tool in python3 git timeout bash; do
    command -v "$tool" >/dev/null 2>&1 || { echo "differential: the host has no '$tool'" >&2; exit 2; }
done

bins="${DIFF_RUST_BIN_DIR:-}"
if [ -z "$bins" ]; then
    export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ws/target}"
    built="$(cd "$ws" && cargo build --release --locked --offline -q -p mx-checks 2>&1)"; rc=$?
    [ "$rc" = 0 ] || { printf '%s\ndifferential: cargo build failed (exit %s)\n' "$built" "$rc" >&2; exit 2; }
    bins="$CARGO_TARGET_DIR/release"
fi

printf 'differential: bash %s, %s, python %s\n' "$BASH_VERSION" "$(git --version)" "$(python3 -c 'import sys; print(sys.version.split()[0])')"
scratch="${MX_SCRATCH:-$HOME/.cache/magent-wt-scratch}/mx-checks-diff"
out="$(python3 "$here/differential.py" --rust-bin-dir "$bins" --tree "$MX_TREE" --seed "$seed" --per-program "$per" \
        --only "${DIFF_ONLY:-}" --scratch "$scratch" 2>&1)"; rc=$?
printf '%s\n' "$out"
n="$(sed -n 's/^differential: \([0-9]*\) comparisons.*/\1/p' <<<"$out" | tail -1)"
if [ "${DIFF_ONLY:-}" = "" ] && [ "${n:-0}" -lt 300 ]; then
    echo "differential: only ${n:-0} comparisons ran; 300 are required" >&2
    exit 1
fi
[ "$rc" = 0 ] || exit 1
grep -q ', 0 unexplained$' <<<"$out" || { echo "differential: the run did not report 0 unexplained" >&2; exit 1; }
