#!/usr/bin/env bash
# Black-box proof: the install characterization cases of the bash programs, run against
# the Rust builds.
#
#   blackbox.sh                 assemble a tree with rust/scripts/assemble-tree.sh and test it
#   blackbox.sh TREE            test an assembled tree
#   blackbox.sh --with-suite    also run tests/run-tests.sh against a plain copy of the bash
#                               tree and against the Rust tree, and list the cases that fail
#                               only on the Rust tree
#
# Phase 1. tests/run-characterization.sh runs the five install-*.sh case files that drive
# install.sh, new-profile.sh, install-cron.sh, install-launchd.sh, alert.sh and
# run-instance.sh (install-evals.sh drives evals/run-evals.sh, which stays bash). The bash
# tree must pass every case. The Rust tree is run three times; every run must report the
# same case count as the bash tree and fail exactly the cases in PINNED, no more and no
# fewer, and the three runs must agree. A pinned case that starts passing is as much a
# failure as a new red one, because the difference it records has gone.
#
# The pinned cases are decisions of the port. Each pins the bash behaviour, so it turns red
# on purpose; each has a Rust test of the behaviour that replaces it, named for its decision.
#
# --with-suite also runs the whole bash suite (tests/run-tests.sh) on a plain copy of the
# bash tree and on the Rust tree and lists what fails only on the Rust tree. That list is
# information, not a gate: cases that grep the text of install.sh, new-profile.sh or a
# platform script cannot pass against a wrapper of two lines.
#
# Exit 0 when every check holds.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
scratch="${MX_SCRATCH:-$HOME/.cache/magent-wt-scratch/p4a}"
files=(install-busy.sh install-deploy.sh install-evals.sh install-isolation.sh install-newprofile.sh install-platform.sh install-uninstall.sh)

# case name | decision and reason
PINNED=(
"key: without od the install stops with 127 and leaves an empty receipt.key (pins current behaviour)|P4A-key: the key is read from the system's random source in this process and written whole, so no od runs and no empty file is left"
"uninstall: a share directory that cannot be removed is listed as a failed step, the rest is removed, exit 0 (pins current behaviour)|P4A-rm: the share directory is removed in this process, so a stub rm on PATH never runs; the failed-step listing is pinned by a Rust test with a machine that fails the removal"
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
    out="$(bash "$root/rust/scripts/assemble-tree.sh" "$tree" install new-profile install-cron install-launchd alert run-instance 2>&1)"; rc=$?
    [ "$rc" = 0 ] || die "assemble-tree.sh exited $rc: $(tail -3 <<<"$out")"
    printf '%s\n' "$(tail -1 <<<"$out")"
fi
for prog in install new-profile install-cron install-launchd alert run-instance; do
    [ -x "$tree/.rust-bin/$prog" ] || die "$tree has no .rust-bin/$prog: not an assembled tree"
done
for rel in install.sh new-profile.sh platform/posix/install-cron.sh platform/macos/install-launchd.sh platform/linux/alert.sh platform/linux/run-instance.sh; do
    grep -q 'exec -a' <<<"$(head -c 2000 "$tree/$rel")" || die "$tree/$rel is not the wrapper of a Rust build"
done
echo "tree under test: $tree  HEAD: $(git -C "$root" rev-parse --short HEAD)"

summary_of() { grep -E '^[0-9]+ cases, [0-9]+ passed, [0-9]+ failed$' <<<"$1" | tail -1; }
failing_of() { grep -E '^FAIL  ' <<<"$1" | sed 's/^FAIL  //' | sort; }

run_set() {  # run_set TREE: runs the case files, prints the runner output
    # HOME and TMPDIR are scratch directories that do not hold one another: a case asserts
    # that the real home path (this HOME) appears in no output, and case folders sit in TMPDIR.
    mkdir -p "$scratch/czhome" "$scratch/cztmp"
    HOME="$scratch/czhome" TMPDIR="$scratch/cztmp" MX_TREE="$1" PYTHONDONTWRITEBYTECODE=1 \
        bash "$root/tests/run-characterization.sh" "${files[@]}" 2>&1
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

# --- tests/run-tests.sh, plain tree and Rust tree --------------------------------------------
# Four runs. The first pair is the whole suite and is information: its cases that grep the
# text of install.sh, new-profile.sh or a platform script fail against a two-line wrapper.
# The second pair skips the source-coupled blocks (MX_SKIP_SOURCE_COUPLED=1, the suite's own
# switch), so what still fails only on the Rust tree is behaviour, and that list is a gate:
# it may hold only the cases named in SUITE_PINNED.
SUITE_PINNED=()   # the assembled binaries carry no build path since main remaps them, so the leak case passes
if [ "$with_suite" = 1 ]; then
    plain="$scratch/bb-plain"
    rm -rf "$plain"; mkdir -p "$plain"
    list="$scratch/bb-list"
    git -C "$root" ls-files -z --cached --others --exclude-standard > "$list" || die "git ls-files failed"
    tar -C "$root" --null --ignore-failed-read -T "$list" -cf - | tar -C "$plain" -xf - || die "copying the plain tree failed"
    mkdir -p "$scratch/czhome" "$scratch/cztmp"
    suite() {  # suite TREE [ENV...]: prints the suite's output
        local t="$1"; shift
        env HOME="$scratch/czhome" TMPDIR="$scratch/cztmp" MX_TREE="$t" PYTHONDONTWRITEBYTECODE=1 "$@" bash "$root/tests/run-tests.sh" 2>&1
    }
    sb="$(suite "$plain")"; sr="$(suite "$tree")"
    sbs="$(suite "$plain" MX_SKIP_SOURCE_COUPLED=1)"; srs="$(suite "$tree" MX_SKIP_SOURCE_COUPLED=1)"
    tally() { grep -E '[0-9]+ passed, [0-9]+ failed' <<<"$1" | tail -1 | sed 's/^ *//'; }
    fails() { grep -E '^ *FAIL' <<<"$1" | sed 's/^ *//' | LC_ALL=C sort -u; }
    printf 'suite, bash tree (no .git)                  : %s\n' "$(tally "$sb")"
    printf 'suite, Rust tree                            : %s\n' "$(tally "$sr")"
    printf 'suite without source-coupled, bash tree     : %s\n' "$(tally "$sbs")"
    printf 'suite without source-coupled, Rust tree     : %s\n' "$(tally "$srs")"
    echo "cases that fail only on the Rust tree (the whole suite; source-coupled cases included):"
    LC_ALL=C comm -13 <(fails "$sb") <(fails "$sr") | sed 's/^/  /'
    behaviour="$(LC_ALL=C comm -13 <(fails "$sbs") <(fails "$srs"))"
    echo "cases that fail only on the Rust tree with the source-coupled blocks skipped (behaviour):"
    printf '%s\n' "$behaviour" | sed 's/^/  /'
    want_suite=""
    for row in "${SUITE_PINNED[@]}"; do want_suite+="FAIL  ${row%%|*}"$'\n'; done
    want_suite="$(printf '%s' "$want_suite" | LC_ALL=C sort -u)"
    if [ "$behaviour" != "$want_suite" ]; then
        echo "  !! the behavioural failures are not the pinned ones"; status=1
    fi
    for row in "${SUITE_PINNED[@]}"; do printf '  - %s\n      %s\n' "${row%%|*}" "${row#*|}"; done
fi

[ "$status" = 0 ] || { echo "RESULT: FAIL"; exit 1; }
echo "RESULT: PASS ($base_total cases; ${#PINNED[@]} pinned differences)"
