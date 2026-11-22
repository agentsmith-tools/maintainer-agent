#!/usr/bin/env bash
# Black-box proof: the characterization cases of the five bash checks, run against the Rust build.
#
#   blackbox.sh                 assemble a tree with rust/scripts/assemble-tree.sh and test it
#   blackbox.sh TREE            test an assembled tree
#   blackbox.sh --with-suite    also run tests/run-tests.sh against this checkout and against the Rust tree,
#                               and list the cases that fail only on the Rust tree
#   blackbox.sh --suite-only    only that last phase
#
# Phase 1. tests/run-characterization.sh runs checks-claims.sh, checks-pins.sh, checks-sweep.sh,
# checks-evals.sh and checks-probe.sh. The bash tree must pass every case. The Rust tree is run three
# times; every run must pass every case, report the same case count as the bash tree, and the three
# runs must print the same lines.
#
# Phase 1b runs signals.sh against the same tree.
#
# Phase 2 (--with-suite). tests/run-tests.sh runs against the bash tree and against the Rust tree with
# MX_SKIP_SOURCE_COUPLED=1 (the Rust tree holds wrappers where the cases grep the text of a script).
# Run it with MX_SCRATCH outside $HOME (for instance /tmp/mx): the suite greps the tree for the user name,
# and the wrappers of the Rust tree hold the tree's own path.
# The bash tree must pass every case. A case that fails only on the Rust tree is a difference, except the one
# whose reason is checked: the suite greps the tree for the user name, and a Rust binary holds the toolchain's
# source paths (see the code). Anything else makes the run fail.
#
# Exit 0 when every check holds.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
scratch="${MX_SCRATCH:-$HOME/.cache/magent-wt-scratch}/mx-checks-blackbox"
files=(checks-claims.sh checks-pins.sh checks-sweep.sh checks-evals.sh checks-probe.sh)
programs=(check-claims shellcheck-sweep verify-action-pins run-evals containment-probe)
with_suite=0
skip_cases=0
tree=""
for a in "$@"; do
    case "$a" in
        --with-suite) with_suite=1 ;;
        --suite-only) with_suite=1; skip_cases=1 ;;
        *) tree="$a" ;;
    esac
done

fail=0
say() { printf '%s\n' "$*"; }
bad() { printf 'blackbox: FAIL: %s\n' "$*" >&2; fail=1; }

for f in "${files[@]}"; do
    [ -f "$root/tests/characterization/$f" ] || { echo "blackbox: no tests/characterization/$f in $root" >&2; exit 2; }
done
rm -rf "$scratch"; mkdir -p "$scratch" || exit 2
trap 'rm -rf "$scratch"' EXIT

if [ -z "$tree" ]; then
    tree="$scratch/tree"
    built="$(bash "$root/rust/scripts/assemble-tree.sh" "$tree" "${programs[@]}" 2>&1)"; rc=$?
    [ "$rc" = 0 ] || { printf '%s\nblackbox: assemble-tree.sh exited %s\n' "$built" "$rc" >&2; exit 2; }
    say "$built"
fi
for p in "${programs[@]}"; do
    [ -x "$tree/.rust-bin/$p" ] || { echo "blackbox: $tree has no .rust-bin/$p, so this would test the bash" >&2; exit 2; }
done

summary() {  # $1 = a runner log -> "N P F"
    sed -n 's/^\([0-9]*\) cases, \([0-9]*\) passed, \([0-9]*\) failed$/\1 \2 \3/p' <<<"$1" | tail -1
}

TMPDIR="$scratch/t"; mkdir -p "$TMPDIR"; export TMPDIR
if [ "$skip_cases" = 0 ]; then
    out="$(MX_TREE="$root" bash "$root/tests/run-characterization.sh" "${files[@]}" 2>&1)"; rc=$?
    read -r bn bp bf <<<"$(summary "$out")"
    say "bash tree: ${bn:-?} cases, ${bp:-?} passed, ${bf:-?} failed (exit $rc)"
    [ "$rc" = 0 ] && [ "${bn:-0}" -gt 0 ] && [ "${bf:-1}" = 0 ] || { bad "the bash tree does not pass its own cases"; printf '%s\n' "$out" | grep -E '^(FAIL|    )' | head -20 >&2; }

    first=""
    for run in 1 2 3; do
        out="$(MX_TREE="$tree" bash "$root/tests/run-characterization.sh" "${files[@]}" 2>&1)"; rc=$?
        read -r n p f <<<"$(summary "$out")"
        say "rust tree, run $run: ${n:-?} cases, ${p:-?} passed, ${f:-?} failed (exit $rc)"
        [ "$rc" = 0 ] && [ "${n:-0}" -gt 0 ] && [ "${f:-1}" = 0 ] || { bad "run $run of the Rust tree has failing cases"; printf '%s\n' "$out" | grep -E '^(FAIL|    )' | head -20 >&2; }
        [ "${n:-x}" = "${bn:-y}" ] || bad "run $run ran ${n:-?} cases and the bash tree ran ${bn:-?}"
        lines="$(grep -E '^(PASS|FAIL)  ' <<<"$out")"
        if [ -z "$first" ]; then first="$lines"; elif [ "$lines" != "$first" ]; then bad "run $run printed different case lines than run 1"; fi
    done

    # Phase 1b. A SIGTERM that ends containment-probe removes its work directory and ends the program with 143,
    # on the bash script and on the Rust build (decision P4B-12).
    sigout="$(bash "$here/signals.sh" "$tree" 2>&1)"; src=$?
    say "signals.sh: exit $src"
    printf '%s\n' "$sigout"
    [ "$src" = 0 ] || bad "the signal proof failed"

fi

if [ "$with_suite" = 1 ]; then
    # The assembled tree holds no .git, and the sweep cases and the PROD_GLOBS checks of the suite need the
    # tree to be a git checkout, so it becomes one (the wrappers and binaries are what it tracks).
    ( cd "$tree" && git init -q -b main && git add -A && git -c user.name=blackbox -c user.email=blackbox@example.invalid commit -q -m tree ) \
        || { echo "blackbox: could not make the Rust tree a git checkout" >&2; exit 2; }
    bashlog="$scratch/suite-bash.log"; rustlog="$scratch/suite-rust.log"
    suite="${BLACKBOX_SUITE:-$root/tests/run-tests.sh}"
    MX_TREE="$root" bash "$suite" > "$bashlog" 2>&1; brc=$?
    MX_TREE="$tree" MX_SKIP_SOURCE_COUPLED=1 bash "$suite" > "$rustlog" 2>&1; rrc=$?
    say "run-tests.sh: bash tree exit $brc, Rust tree exit $rrc"
    LC_ALL=C grep -E '^  FAIL  ' "$bashlog" | LC_ALL=C sort -u > "$scratch/fail-bash.txt"
    LC_ALL=C grep -E '^  FAIL  ' "$rustlog" | LC_ALL=C sort -u > "$scratch/fail-rust.txt"
    say "failing cases on the bash tree: $(wc -l < "$scratch/fail-bash.txt" | tr -d ' ')"
    cat "$scratch/fail-bash.txt"
    LC_ALL=C comm -13 "$scratch/fail-bash.txt" "$scratch/fail-rust.txt" > "$scratch/only-rust.txt"
    n="$(wc -l < "$scratch/only-rust.txt" | tr -d ' ')"
    say "cases that fail only on the Rust tree: $n"
    cat "$scratch/only-rust.txt"
    [ "$brc" = 0 ] && [ -s "$bashlog" ] || bad "the suite does not pass on the bash tree (exit $brc)"
    # One case may fail only on the Rust tree: the suite greps the whole tree for the user name, and a Rust
    # binary holds the toolchain's source paths (std's panic locations) under $HOME. The reason is checked, not
    # assumed: the grep must find the name in .rust-bin and nowhere else. Any other case is a difference.
    leak='  FAIL  a home path leaked into the repository'
    others="$(LC_ALL=C grep -vxF "$leak" "$scratch/only-rust.txt")"
    [ -z "$others" ] || bad "cases fail only on the Rust tree and have no recorded reason: $others"
    if LC_ALL=C grep -qxF "$leak" "$scratch/only-rust.txt"; then
        user="${HOME##*/}"
        outside="$(cd "$tree" && LC_ALL=C grep -rIl -e "$user" --exclude-dir=.git --exclude-dir=.rust-bin .)"; grc=$?
        { [ "$grc" = 0 ] || [ "$grc" = 1 ]; } || bad "could not search the Rust tree for the user name (grep exit $grc)"
        [ -z "$outside" ] || bad "the user name is in the Rust tree outside .rust-bin (the wrappers hold the tree's own path, so MX_SCRATCH must be outside \$HOME for --with-suite): $outside"
        inside="$(cd "$tree" && LC_ALL=C grep -rl -e "$user" .rust-bin | wc -l | tr -d ' ')"
        [ "${inside:-0}" -gt 0 ] || bad "the leak case failed on the Rust tree and .rust-bin does not hold the user name: the reason is not the binaries"
        [ "$fail" = 0 ] && say "the home-path case: the user name is in $inside binaries of .rust-bin (toolchain source paths) and in no other file"
    fi
    tail -3 "$bashlog" | sed 's/^/bash tree: /'
    tail -3 "$rustlog" | sed 's/^/Rust tree: /'
fi

[ "$fail" = 0 ] && say "blackbox: ok" || exit 1
