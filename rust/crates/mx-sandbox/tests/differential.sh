#!/usr/bin/env bash
# Differential proof: the Rust suite host, selector and sandbox against the bash functions of
# bin/maintainer-merge (sourced, so the text compared is the shipped text).
#
# differential.py runs each comparison: one bash function and the same operation of the probe
# (examples/p3a-probe.rs) on identical inputs, with the exit code, standard output and standard
# error compared. A difference passes only when it is a decision the pull request records
# (P30-10, P30-11, P3A-8, P3A-9); any other difference fails the run.
#
# Usage: differential.sh
# Env:   MX_TREE      the bash side: a tree with bin/maintainer-merge (default: this checkout)
#        MX_SCRATCH   where scratch goes (default ~/.cache/magent-wt-scratch/p3a-diff)
#        CARGO_TARGET_DIR, CARGO_BUILD_JOBS   for the probe build
#        DIFF_SEED    the seed (default 20261122; printed, so a run repeats)
#        DIFF_ONLY    comma-separated fragments of section or case names
#        DIFF_VERBOSE 1 prints a line for every comparison
#        N_GLOB N_ESC N_SEL N_MUT N_EXT N_FLAGS   how many cases per section
# Exit:  0 when every comparison matched or was explained, 1 on an unexplained difference,
#        2 on a harness fault.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
MX_TREE="${MX_TREE:-$root}"
seed="${DIFF_SEED:-20261122}"
scratch="${MX_SCRATCH:-$HOME/.cache/magent-wt-scratch/p3a-diff}"

[ -f "$MX_TREE/bin/maintainer-merge" ] || { echo "differential: no bin/maintainer-merge under $MX_TREE" >&2; exit 2; }
for tool in python3 git bash sed find sha256sum; do
    command -v "$tool" >/dev/null 2>&1 || { echo "differential: the host has no '$tool'" >&2; exit 2; }
done
mkdir -p "$scratch" || exit 2
target="${CARGO_TARGET_DIR:-$root/rust/target}"

out="$(cd "$root/rust" && cargo build -p mx-sandbox --examples --offline 2>&1)"; rc=$?
[ "$rc" = 0 ] || { printf '%s\ndifferential: building the probe exited %s\n' "$out" "$rc" >&2; exit 2; }
probe="$target/debug/examples/p3a-probe"
[ -x "$probe" ] || { echo "differential: no probe at $probe" >&2; exit 2; }

printf 'differential: seed %s (repeat with DIFF_SEED=%s)\n' "$seed" "$seed"
printf 'differential: bash %s, python %s, tree %s\n' "$BASH_VERSION" "$(python3 -c 'import sys; print(sys.version.split()[0])')" "$MX_TREE"
MX_MM="$MX_TREE/bin/maintainer-merge" MX_PROBE="$probe" MX_SCRATCH="$scratch" DIFF_SEED="$seed" \
    python3 -I "$here/differential.py"
rc=$?
rm -rf "$scratch"
exit "$rc"
