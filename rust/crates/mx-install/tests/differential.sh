#!/usr/bin/env bash
# Differential proof: the Rust install programs against the bash scripts.
#
# Each scenario (differential.py) builds a sandbox with a copy of the tree, a scratch HOME,
# a search path of stubs (systemctl, crontab, launchctl, flock, notify-send, date, uname:
# each logs one line per call to a global call log, with its argument vector, working
# directory, an allowlist of environment values and the names of every variable it saw) and
# the few real tools the scripts use. It runs the bash script on one copy and the Rust build
# on a second, identical one, and compares the exit status, standard output and standard
# error, the global call log in order, and the state of HOME and of the tree afterwards
# (every path with its type, mode, link target and the hash of its bytes). The real HOME of
# the person running this is hashed before and after the whole run. A difference that a
# plan decision names is compared against the exact difference the decision states; any
# other difference fails the run.
#
# Usage: differential.sh [TREE]
#   TREE           an assembled tree (rust/scripts/assemble-tree.sh DEST install new-profile ...);
#                  built into the scratch directory when not given
# Env:   MX_TREE       the bash side (default: this checkout)
#        MX_SCRATCH    where the scratch directory goes (default ~/.cache/magent-wt-scratch/p4a)
#        DIFF_SEED     the seed (default 20261122; printed, so a run repeats)
#        DIFF_RANDOM   how many generated scenarios follow the named ones (default 330)
#        DIFF_ONLY     comma-separated scenario name fragments
#        DIFF_VERBOSE  1 prints a line for every scenario
# Exit:  0 when every comparison matched or was explained, 1 on an unexplained difference,
#        2 on a harness fault.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
MX_TREE="${MX_TREE:-$root}"
seed="${DIFF_SEED:-20261122}"
scratch_base="${MX_SCRATCH:-$HOME/.cache/magent-wt-scratch/p4a}"

for f in install.sh new-profile.sh lib/run.sh platform/posix/install-cron.sh platform/macos/install-launchd.sh platform/linux/alert.sh platform/linux/run-instance.sh scripts/render-settings.py; do
    [ -f "$MX_TREE/$f" ] || { echo "differential: no $f under MX_TREE=$MX_TREE" >&2; exit 2; }
done
for tool in python3 git timeout bash flock sha256sum sed awk grep od; do
    command -v "$tool" >/dev/null 2>&1 || { echo "differential: the host has no '$tool'" >&2; exit 2; }
done

tree="${1:-}"
mkdir -p "$scratch_base" || exit 2
if [ -z "$tree" ]; then
    tree="$scratch_base/diff-tree"
    rm -rf "$tree"
    out="$(bash "$root/rust/scripts/assemble-tree.sh" "$tree" install new-profile install-cron install-launchd alert run-instance 2>&1)"; rc=$?
    [ "$rc" = 0 ] || { printf '%s\ndifferential: assemble-tree.sh exited %s\n' "$out" "$rc" >&2; exit 2; }
fi
if [ ! -x "$tree/.rust-bin/install" ] || [ ! -f "$tree/install.sh" ]; then
    echo "differential: $tree is not an assembled tree" >&2; exit 2
fi

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
