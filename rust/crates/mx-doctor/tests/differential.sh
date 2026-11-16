#!/usr/bin/env bash
# Differential proof: the Rust maintainer-doctor against bin/maintainer-doctor.
# A thin wrapper around differential.py (read its header for what is compared).
#
#   differential.sh [TREE]     TREE is an assembled tree (rust/scripts/assemble-tree.sh);
#                              without one the script assembles its own.
#
# Env:  DIFF_SEED   the seed of the generated hosts (default 20261115; printed)
#       DIFF_CASES  the number of hosts (default 520: 5 fields each, 2600 comparisons)
#       MX_SCRATCH  where scratch directories go (default ~/.cache/magent-wt-scratch/p1i)
# Exit 0 when more than 400 comparisons ran and none differed, apart from the pinned
# F4 argument vector of the bash grep (counted and printed, never silently skipped).
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
scratch="${MX_SCRATCH:-$HOME/.cache/magent-wt-scratch/p1i}"
mkdir -p "$scratch" || { echo "differential: cannot make $scratch" >&2; exit 1; }
tree="${1:-}"
if [ -z "$tree" ]; then
    tree="$scratch/diff-tree"
    rm -rf "$tree"
    out="$(bash "$root/rust/scripts/assemble-tree.sh" "$tree" maintainer-doctor 2>&1)"; rc=$?
    [ "$rc" = 0 ] || { echo "differential: assemble-tree.sh exited $rc: $(tail -3 <<<"$out")" >&2; exit 1; }
fi
echo "HEAD: $(git -C "$root" rev-parse --short HEAD)  tree: $tree"
exec python3 -I "$here/differential.py" --seed "${DIFF_SEED:-20261115}" --cases "${DIFF_CASES:-520}" \
    --rust-tree "$tree" --bash-tree "$root" --scratch "$scratch/diff"
