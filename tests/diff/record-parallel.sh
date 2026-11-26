#!/usr/bin/env bash
# record.sh in several processes at once, for a large scenario set.
#
#   record-parallel.sh <shards> <tree> <scenario-dir> <out-dir>
#
# Deals the scenarios (by their sorted position) into <shards> directories, runs record.sh on each
# in its own process and scratch directory, waits for every one by its pid, and merges the result
# directories into <out-dir> with the .complete marker compare.sh wants. Every shard must exit 0
# and the merged count must equal the scenario count, or this exits 2 and writes no marker.
set -uo pipefail
here="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)"
die() { printf 'record-parallel.sh: %s\n' "$1" >&2; exit 2; }
[ $# -eq 4 ] || die "usage: record-parallel.sh <shards> <tree> <scenario-dir> <out-dir>"
shards="$1"; tree="$2"; scen="$3"; out="$4"
case "$shards" in ''|*[!0-9]*|0) die "shards must be a positive number" ;; esac
[ -d "$scen" ] || die "scenario directory '$scen' does not exist"
if [ -e "$out" ] && [ -n "$(ls -A "$out")" ]; then die "out-dir '$out' is not empty"; fi
base="${MX_SCRATCH:-${XDG_CACHE_HOME:-$HOME/.cache}/magent-diff-scratch}"
mkdir -p "$base" "$out" || die "cannot create $base or $out"
work="$(mktemp -d "$base/par.XXXXXX")" || die "cannot create a scratch directory in $base"
trap 'rm -rf "$work"' EXIT

list="$(LC_ALL=C find "$scen" -maxdepth 1 -type f -name '*.sh' | LC_ALL=C sort)" || die "cannot list $scen"
[ -n "$list" ] || die "no scenarios (*.sh) in '$scen'"
mapfile -t files <<<"$list"
total="${#files[@]}"
for ((k = 0; k < shards; k++)); do mkdir -p "$work/in.$k"; done
for i in "${!files[@]}"; do cp "${files[$i]}" "$work/in.$((i % shards))/" || die "cannot copy ${files[$i]}"; done

pids=()
for ((k = 0; k < shards; k++)); do
    [ -n "$(ls -A "$work/in.$k")" ] || continue
    MX_SCRATCH="$work/scratch.$k" bash "$here/record.sh" "$tree" "$work/in.$k" "$work/out.$k" >"$work/log.$k" 2>&1 &
    pids+=("$!")
done
failed=0
for pid in "${pids[@]}"; do
    wait "$pid" || failed=1
done
if [ "$failed" != 0 ]; then
    cat "$work"/log.* >&2
    die "a shard failed; nothing was merged"
fi
count=0
for dir in "$work"/out.*; do
    [ -f "$dir/.complete" ] || die "shard $dir has no .complete marker"
    for scenario in "$dir"/*/; do
        [ -d "$scenario" ] || continue
        cp -a "${scenario%/}" "$out/" || die "cannot merge $scenario"
        count=$((count + 1))
    done
    [ -f "$dir/tree.abspath" ] && cp "$dir/tree.abspath" "$out/tree.abspath"
done
[ "$count" = "$total" ] || die "merged $count scenarios, expected $total"
printf '%s\n' "$count" > "$out/.complete" || die "cannot write $out/.complete"
printf 'recorded %d scenarios from %s into %s (%d shards)\n' "$count" "$tree" "$out" "$shards"
