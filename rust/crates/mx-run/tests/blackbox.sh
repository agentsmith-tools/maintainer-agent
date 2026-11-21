#!/usr/bin/env bash
# Black-box proof: the characterization cases of the bash orchestrator, run against
# the Rust build.
#
#   blackbox.sh                 assemble a tree with rust/scripts/assemble-tree.sh and test it
#   blackbox.sh TREE            test an assembled tree
#   blackbox.sh --with-suite    also run tests/run-tests.sh against a plain copy of the
#                               bash tree and against the Rust tree, and list the cases
#                               that fail only on the Rust tree
#
# Phase 1. tests/run-characterization.sh runs the seven run-*.sh case files
# (lib/run.sh, lib/profile.sh and the four backends). The bash tree must pass every
# case. The Rust tree is run three times; every run must report the same case count
# as the bash tree and fail exactly the cases in PINNED below, no more and no fewer,
# and the three runs must agree. A pinned case that starts passing is as much a
# failure as a new red one, because the difference it records has gone.
#
# The six pinned cases are the plan decisions that fix a behaviour which fails open
# in the bash (decisions D10, D11, D12, D13, D15 and D18 of the plan). Each case
# pins the bash behaviour, so it turns red on purpose. Each has a Rust test of the
# fixed behaviour in crates/mx-run/tests/lifecycle.rs, named for its decision.
#
# --with-suite also runs the whole bash suite (tests/run-tests.sh) on a plain copy of
# the bash tree and on the Rust tree and lists what fails only on the Rust tree. That
# list is information, not a gate, because four kinds of case cannot pass against a
# wrapper: source-coupled cases that grep the text of lib/run.sh (the wrapper is two
# lines), cases whose stub token is under 16 characters (decision D15 refuses it), a
# case that writes its own shell backend file (a backend is one of four names, decision
# D18), and cases that read the path of the tree (the wrapper names the binary).
#
# Exit 0 when every check holds.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
scratch="${MX_SCRATCH:-$HOME/.cache/magent-wt-scratch/p2a}"
files=(run-backends.sh run-cadence-lock.sh run-classify.sh run-identity.sh run-lifecycle.sh run-prompt.sh run-start.sh)

# case name | decision and reason
PINNED=(
"backend: BACKEND is joined into a path unchecked, so ../backends/codex still resolves (no plan decision yet)|D18: a BACKEND name with a slash or a dot-dot is refused (exit 64), where the bash sources any file"
"cadence: a non-numeric minimum falls open, the run goes ahead and bash complains on stderr|D11: a MIN_HOURS_<task> that is not a whole number refuses (exit 64), where the bash gate disappears"
"cadence: a stamp in the future reads as a negative age and skips|D12: a stamp from the future does not skip the run, where the bash skips it with a negative age"
"lock: a run.lock that cannot be opened is reported as a lock error, exit 1|D10: a failed open of run.lock is its own refusal, where the bash reports a lock error through a failing flock"
"token: a GH_TOKEN inherited from the caller reaches the first gh calls and is replaced by the pin|D13: an inherited GH_TOKEN reaches no call before the pin"
"scrub: a pinned token under 16 characters is never scrubbed and the run does not notice|D15: a pinned token under 16 characters refuses the run, because the exit scrub cannot remove it"
)

with_suite=0; tree=""
for arg in "$@"; do
    case "$arg" in
        --with-suite) with_suite=1 ;;
        -*) printf 'blackbox: unknown option %s\n' "$arg" >&2; exit 64 ;;
        *) tree="$arg" ;;
    esac
done

die() { printf 'blackbox: %s\n' "$1" >&2; exit 1; }
mkdir -p "$scratch" || die "cannot make $scratch"

if [ -z "$tree" ]; then
    tree="$scratch/bb-tree"
    rm -rf "$tree"
    out="$(bash "$root/rust/scripts/assemble-tree.sh" "$tree" run 2>&1)"; rc=$?
    [ "$rc" = 0 ] || die "assemble-tree.sh exited $rc: $(tail -3 <<<"$out")"
    printf '%s\n' "$(tail -1 <<<"$out")"
fi
[ -x "$tree/.rust-bin/run" ] || die "$tree has no .rust-bin/run: not an assembled tree"
[ -f "$tree/lib/run.sh" ] || die "$tree has no lib/run.sh"
wrapper="$(head -c 600 "$tree/lib/run.sh")"
grep -q '[.]rust-bin/run' <<<"$wrapper" || die "$tree/lib/run.sh is not the wrapper of the Rust build"
echo "tree under test: $tree  HEAD: $(git -C "$root" rev-parse --short HEAD)"

summary_of() { grep -E '^[0-9]+ cases, [0-9]+ passed, [0-9]+ failed$' <<<"$1" | tail -1; }
failing_of() { grep -E '^FAIL  ' <<<"$1" | sed 's/^FAIL  //' | sort; }

run_set() {  # run_set TREE: runs the case files, prints the runner output
    MX_TREE="$1" TMPDIR="$scratch" bash "$root/tests/run-characterization.sh" "${files[@]}" 2>&1
}

# --- the bash tree: every case passes -------------------------------------------------
out="$(run_set "$root")"; rc=$?
base="$(summary_of "$out")"
[ -n "$base" ] || die "the bash tree run printed no summary (exit $rc)"
base_total="${base%% cases*}"
printf 'bash tree : %s (exit %s)\n' "$base" "$rc"
[ "$rc" = 0 ] || die "the bash tree does not pass its own characterization cases"
[ "$base_total" -gt 200 ] || die "only $base_total cases ran on the bash tree: the proof would be vacuous"

# --- the Rust tree, three times ------------------------------------------------------------
names=(); for row in "${PINNED[@]}"; do names+=("${row%%|*}"); done
want="$(printf '%s\n' "${names[@]}" | sort)"
prev=""; status=0
for n in 1 2 3; do
    out="$(run_set "$tree")"; rc=$?
    sum="$(summary_of "$out")"
    [ -n "$sum" ] || die "run $n of the Rust tree printed no summary (exit $rc)"
    total="${sum%% cases*}"
    got="$(failing_of "$out")"
    printf 'rust tree run %s: %s\n' "$n" "$sum"
    if [ "$total" != "$base_total" ]; then echo "  !! $total cases ran, the bash tree ran $base_total"; status=1; fi
    if [ "$got" != "$want" ]; then
        echo "  !! the failing cases are not the pinned ones"
        diff <(printf '%s\n' "$want") <(printf '%s\n' "$got") | sed 's/^/     /'
        status=1
    fi
    if [ -n "$prev" ] && [ "$prev" != "$sum|$got" ]; then echo "  !! run $n differs from run 1"; status=1; fi
    [ -n "$prev" ] || prev="$sum|$got"
done
printf 'pinned differences (%d), the only failures allowed:\n' "${#PINNED[@]}"
for row in "${PINNED[@]}"; do printf '  - %s\n      %s\n' "${row%%|*}" "${row#*|}"; done

# --- optional: the whole bash suite, against both trees ---------------------------------------
if [ "$with_suite" = 1 ]; then
    plain="$scratch/bb-plain"
    rm -rf "$plain"; mkdir -p "$plain"
    list="$scratch/bb-list"
    git -C "$root" ls-files -z --cached --others --exclude-standard > "$list" || die "git ls-files failed"
    tar -C "$root" --null --ignore-failed-read -T "$list" -cf - | tar -C "$plain" -xf - || die "copying the plain tree failed"
    sb="$(MX_TREE="$plain" TMPDIR="$scratch" bash "$root/tests/run-tests.sh" 2>&1)"
    sr="$(MX_TREE="$tree" TMPDIR="$scratch" bash "$root/tests/run-tests.sh" 2>&1)"
    printf 'suite, bash tree (no .git): %s\n' "$(grep -E '[0-9]+ passed, [0-9]+ failed' <<<"$sb" | tail -1 | sed 's/^ *//')"
    printf 'suite, Rust tree          : %s\n' "$(grep -E '[0-9]+ passed, [0-9]+ failed' <<<"$sr" | tail -1 | sed 's/^ *//')"
    only_rust="$(LC_ALL=C comm -13 <(grep -E '^ *FAIL' <<<"$sb" | sed 's/^ *//' | LC_ALL=C sort -u) <(grep -E '^ *FAIL' <<<"$sr" | sed 's/^ *//' | LC_ALL=C sort -u))"
    echo "cases that fail only on the Rust tree:"
    printf '%s\n' "$only_rust" | sed 's/^/  /'
fi

[ "$status" = 0 ] || { echo "RESULT: FAIL"; exit 1; }
echo "RESULT: PASS ($base_total cases; ${#PINNED[@]} pinned differences)"
