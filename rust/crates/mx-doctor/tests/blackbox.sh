#!/usr/bin/env bash
# Black-box proof: the characterization cases of the bash maintainer-doctor, run
# against the Rust build.
#
#   blackbox.sh                 assemble a tree with rust/scripts/assemble-tree.sh and test it
#   blackbox.sh TREE            test an assembled tree
#   blackbox.sh --with-suite    also run tests/run-tests.sh against a plain copy of the
#                               bash tree and against the Rust tree, and list the cases
#                               that fail only on the Rust tree
#
# Phase 1. tests/run-characterization.sh runs doctor-core.sh,
# doctor-ci-scheduler-podman.sh and doctor-sandbox-audit.sh. The bash tree must pass
# every case. The Rust tree is run three times; every run must report the same case
# count as the bash tree and fail exactly the cases in PINNED below, no more and no
# fewer, and the three runs must agree. A pinned case that starts passing is as much
# a failure as a new red one, because the difference it records has gone.
#
# The cases pinned (decision P1I-F4 and one harness artifact):
#   F4 (the token scan reads files in process; no child gets the token). Three
#      cases read grep's argument vector and demand the token in it. The brief named
#      one; two more cases (the whole state directory searched, and the Claude session
#      directory searched) assert the same grep line, so they differ for the same reason.
#   the missing-gh case asserts a record that only bash's command_not_found_handle
#      (installed through BASH_ENV by the case harness) writes. Rust starts the
#      program through the Proc port and gets NotFound; the doctor's output, exit code
#      and call order for that case are the same as the bash's, which the differential
#      proves (differential.py, case "nogh").
#
# Exit 0 when every check holds.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
scratch="${MX_SCRATCH:-$HOME/.cache/magent-wt-scratch/p1i}"
files=(doctor-core.sh doctor-ci-scheduler-podman.sh doctor-sandbox-audit.sh)

PINNED=(
"identity: no gh on PATH fails with 'gh not installed' and skips the workflow check"
"token: the scan puts the token in grep's argv (pins current behaviour, plan decision F4)"
"token: with none of the five subdirectories the whole state directory is searched"
"token: the Claude session directory of REPO_PATH is searched, and replaces the state directory when no subdirectory exists"
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
    out="$(bash "$root/rust/scripts/assemble-tree.sh" "$tree" maintainer-doctor 2>&1)"; rc=$?
    [ "$rc" = 0 ] || die "assemble-tree.sh exited $rc: $(tail -3 <<<"$out")"
    printf '%s\n' "$(tail -1 <<<"$out")"
fi
[ -x "$tree/.rust-bin/maintainer-doctor" ] || die "$tree has no .rust-bin/maintainer-doctor: not an assembled tree"
echo "tree under test: $tree  HEAD: $(git -C "$root" rev-parse --short HEAD)"

# cases_of OUTPUT: the case count and the failing names, from the runner's own lines.
summary_of() { grep -E '^[0-9]+ cases, [0-9]+ passed, [0-9]+ failed$' <<<"$1" | tail -1; }
failing_of() { grep -E '^FAIL  ' <<<"$1" | sed 's/^FAIL  //' | sort; }

run_set() {  # run_set TREE: runs the three case files, prints the runner output
    MX_TREE="$1" TMPDIR="$scratch" bash "$root/tests/run-characterization.sh" "${files[@]}" 2>&1
}

# --- the bash tree: every case passes -------------------------------------------------
out="$(run_set "$root")"; rc=$?
base="$(summary_of "$out")"
[ -n "$base" ] || die "the bash tree run printed no summary (exit $rc)"
base_total="${base%% cases*}"
printf 'bash tree : %s (exit %s)\n' "$base" "$rc"
[ "$rc" = 0 ] || die "the bash tree does not pass its own characterization cases"
[ "$base_total" -gt 100 ] || die "only $base_total cases ran on the bash tree: the proof would be vacuous"

# --- the Rust tree, three times ------------------------------------------------------------
want="$(printf '%s\n' "${PINNED[@]}" | sort)"
prev=""; status=0
for n in 1 2 3; do
    out="$(run_set "$tree")"; rc=$?
    sum="$(summary_of "$out")"
    [ -n "$sum" ] || die "run $n of the Rust tree printed no summary (exit $rc)"
    total="${sum%% cases*}"; failed="${sum##*, }"; failed="${failed%% failed}"
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
printf 'pinned differences (%d), each with its reason in this script'"'"'s header:\n' "${#PINNED[@]}"
printf '  - %s\n' "${PINNED[@]}"

# --- optional: the whole bash suite, against both trees ---------------------------------------
if [ "$with_suite" = 1 ]; then
    plain="$scratch/bb-plain"
    rm -rf "$plain"; mkdir -p "$plain"
    list="$scratch/bb-list"
    git -C "$root" ls-files -z --cached --others --exclude-standard > "$list" || die "git ls-files failed"
    tar -C "$root" --null --ignore-failed-read -T "$list" -cf - | tar -C "$plain" -xf - || die "copying the plain tree failed"
    # Cases that stub the tools read these variables; the Rust program passes children an allowlist.
    export MAINTAINER_CHILD_ENV="DOC_LOG SR_MODE"
    sb="$(MX_TREE="$plain" TMPDIR="$scratch" bash "$root/tests/run-tests.sh" 2>&1)"
    sr="$(MX_TREE="$tree" TMPDIR="$scratch" bash "$root/tests/run-tests.sh" 2>&1)"
    printf 'suite, bash tree (no .git): %s\n' "$(grep -E '[0-9]+ passed, [0-9]+ failed' <<<"$sb" | tail -1 | sed 's/^ *//')"
    printf 'suite, Rust tree          : %s\n' "$(grep -E '[0-9]+ passed, [0-9]+ failed' <<<"$sr" | tail -1 | sed 's/^ *//')"
    only_rust="$(comm -13 <(grep -E '^ *FAIL' <<<"$sb" | sed 's/^ *//' | sort -u) <(grep -E '^ *FAIL' <<<"$sr" | sed 's/^ *//' | sort -u))"
    echo "cases that fail only on the Rust tree:"
    printf '%s\n' "$only_rust" | sed 's/^/  /'
fi

[ "$status" = 0 ] || { echo "RESULT: FAIL"; exit 1; }
echo "RESULT: PASS ($base_total cases; ${#PINNED[@]} pinned differences)"
