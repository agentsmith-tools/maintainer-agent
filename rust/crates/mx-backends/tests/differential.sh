#!/usr/bin/env bash
# Differential proof: the mx-backends crate against lib/backends/*.sh.
#
# For each of the four backends the bash function (sourced in a child bash by
# tests/differential/driver.sh) and the Rust backend answer the same four
# questions (declare, wall, check, run) over hand-written and seeded scenarios.
# The Rust plan is run by a stand-in executor that starts the same stubs. The two
# sides must agree on every printed line, the exit status, standard output and
# standard error, the order of the stub calls with their argv, environment names,
# working directory and standard input, and the bytes of every file the run wrote.
#
# The work is done by the integration test (tests/differential.rs); this script
# runs it and refuses a run that printed no summary line, ran fewer than 40
# scenarios for a backend, or found a difference.
#
# Usage: differential.sh
# Env:   DIFF_SEED     seed of the generated scenarios (default 20261120; printed)
#        MX_TREE       tree whose lib/backends is tested (default: this checkout)
#        CARGO_TARGET_DIR, CARGO_BUILD_JOBS as for any cargo run
# Exit:  0 when four summary lines each show differences=0 and at least 40 scenarios.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ws="$(cd "$here/../../.." && pwd)"

command -v cargo >/dev/null 2>&1 || { echo "differential: cargo is not installed, so nothing was checked" >&2; exit 2; }
command -v bash >/dev/null 2>&1 || { echo "differential: no bash" >&2; exit 2; }

cd "$ws" || { echo "differential: cannot enter $ws" >&2; exit 2; }
out="$(cargo test -p mx-backends --locked --test differential -- --nocapture 2>&1)"; rc=$?
if [ "$rc" != 0 ]; then
    printf '%s\n' "$out" >&2
    echo "differential: cargo test exited $rc" >&2
    exit 1
fi

lines="$(grep '^differential\[' <<<"$out")"
n="$(grep -c '^differential\[' <<<"$lines")"
if [ "$n" != 4 ]; then
    printf '%s\n' "$out" >&2
    echo "differential: expected 4 summary lines (one per backend), saw $n" >&2
    exit 1
fi

total=0
bad=0
for backend in claude codex cursor opencode; do
    line="$(grep "^differential\\[$backend\\]" <<<"$lines")"
    scenarios="$(sed -n 's/.* scenarios=\([0-9][0-9]*\) .*/\1/p' <<<"$line")"
    diffs="$(sed -n 's/.* differences=\([0-9][0-9]*\) .*/\1/p' <<<"$line")"
    printf '%s\n' "$line"
    if [ "${scenarios:-0}" -lt 40 ] || [ "${diffs:-1}" != 0 ]; then
        echo "differential: $backend: scenarios=${scenarios:-none} differences=${diffs:-none}" >&2
        bad=1
    fi
    total=$((total + ${scenarios:-0}))
done
passed="$(grep -c '^test .* ok$' <<<"$out")"
if [ "$bad" != 0 ]; then
    echo "differential: FAILED" >&2
    exit 1
fi
echo "differential: $total scenarios in 4 backends, 0 differences ($passed test cases passed)"
