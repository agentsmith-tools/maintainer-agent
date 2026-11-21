#!/usr/bin/env bash
# Differential proof: the Rust `run` against lib/run.sh.
#
# Each scenario (differential.py) builds a sandbox with a tree copy, a fixture
# profile, a state directory and a checkout, and a stub for every external tool
# (gh, the helper, flock, notify-send, loginctl, sleep, date and the four agent
# CLIs). It runs the bash program on a copy, restores the sandbox, runs the Rust
# build on a second copy, and compares the exit code, standard output and standard
# error, the global call log (every call with its argument vector, working
# directory, descriptor 9, standard input, environment names and the exported
# values, in order), the state tree and TMPDIR. Scenarios that exercise a plan
# decision fixed on purpose (D10, D11, D12, D13, D15, D18) are compared against the
# exact difference the decision names. Any other difference fails the run.
#
# Usage: differential.sh [TREE]
#   TREE           an assembled tree (rust/scripts/assemble-tree.sh DEST run); built
#                  into the scratch directory when not given
# Env:   MX_TREE       the bash side (default: this checkout)
#        MX_SCRATCH    where the scratch directory goes (default ~/.cache/magent-wt-scratch/p2a)
#        DIFF_SEED     the seed (default 20261120; printed, so a run repeats)
#        DIFF_RANDOM   how many generated scenarios follow the named ones (default 260)
#        DIFF_ONLY     comma-separated scenario name fragments
#        DIFF_VERBOSE  1 prints a line for every scenario
# Exit:  0 when every comparison matched or was explained, 1 on an unexplained
#        difference, 2 on a harness fault.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
MX_TREE="${MX_TREE:-$root}"
seed="${DIFF_SEED:-20261120}"
scratch_base="${MX_SCRATCH:-$HOME/.cache/magent-wt-scratch/p2a}"

for f in lib/run.sh lib/profile.sh lib/defaults.env lib/failure-shapes.json scripts/transcript.py; do
    [ -f "$MX_TREE/$f" ] || { echo "differential: no $f under MX_TREE=$MX_TREE" >&2; exit 2; }
done
for tool in python3 git timeout bash flock sha256sum; do
    command -v "$tool" >/dev/null 2>&1 || { echo "differential: the host has no '$tool'" >&2; exit 2; }
done

tree="${1:-}"
mkdir -p "$scratch_base" || exit 2
if [ -z "$tree" ]; then
    tree="$scratch_base/diff-tree"
    rm -rf "$tree"
    out="$(bash "$root/rust/scripts/assemble-tree.sh" "$tree" run 2>&1)"; rc=$?
    [ "$rc" = 0 ] || { printf '%s\ndifferential: assemble-tree.sh exited %s\n' "$out" "$rc" >&2; exit 2; }
fi
[ -f "$tree/lib/run.sh" ] && [ -x "$tree/.rust-bin/run" ] || { echo "differential: $tree is not an assembled tree" >&2; exit 2; }

work="$(mktemp -d "$scratch_base/diff-work.XXXXXX")" || exit 2
# The trap runs this function; shellcheck cannot see that (SC2317).
# shellcheck disable=SC2317
cleanup() { chmod -R u+rwX "$work" 2>/dev/null; rm -rf "$work"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

printf 'differential: seed %s (repeat with DIFF_SEED=%s)\n' "$seed" "$seed"
printf 'differential: bash %s, python %s\n' "$BASH_VERSION" "$(python3 -c 'import sys; print(sys.version.split()[0])')"
printf 'differential: bash side %s, rust side %s (HEAD %s)\n' "$MX_TREE" "$tree" "$(git -C "$root" rev-parse --short HEAD)"
python3 -I "$here/differential.py" "$MX_TREE" "$tree" "$work" "$seed"
