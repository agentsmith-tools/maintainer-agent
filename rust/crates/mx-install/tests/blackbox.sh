#!/usr/bin/env bash
# Black-box proof: the install characterization cases of the bash programs, run against
# the Rust builds.
#
#   blackbox.sh                 assemble a tree with rust/scripts/assemble-tree.sh and test it
#   blackbox.sh TREE            test an assembled tree
#   blackbox.sh --with-suite    also run tests/run-tests.sh against a plain copy of the bash
#                               tree and against the Rust tree, and list the cases that fail
#                               only on the Rust tree
#   blackbox.sh --self-test     prove that the comparison of failure lists gives one verdict
#                               whatever order the suite prints its FAIL lines in (FU1-5);
#                               builds no tree and runs no suite of this repository
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

with_suite=0; tree=""; self_test=0
for arg in "$@"; do
    case "$arg" in
        --with-suite) with_suite=1 ;;
        --self-test) self_test=1 ;;
        -*) printf 'blackbox: unknown option %s\n' "$arg" >&2; exit 64 ;;
        *) tree="$arg" ;;
    esac
done

die() { printf 'blackbox: %s\n' "$1" >&2; exit 1; }
mkdir -p "$scratch" || die "cannot make $scratch"

# --- comparing lists of failing cases ---------------------------------------------------
# `comm` wants both inputs sorted in the order it compares in. The order a suite prints its
# FAIL lines in follows how the suite walks the file system (a directory listing is in one
# order on tmpfs and in another on ext4), and the order a `sort` gives follows the locale of
# the shell that runs it. So every list is sorted here, under the C locale, before anything
# compares it, `comm` is told to refuse an input that is not in that order, and every
# difference is printed, not only the first (FU1-5). `--self-test` proves the verdict does
# not move when the suite prints the same failures in the opposite order.

# fail_lines OUTPUT: the FAIL lines of a suite run, one case each, sorted and unique.
fail_lines() { grep -E '^ *FAIL' <<<"$1" | sed 's/^ *//' | LC_ALL=C sort -u; }

# sorted_set TEXT: the lines of TEXT, blank lines dropped, sorted and unique.
sorted_set() { printf '%s\n' "$1" | sed '/^$/d' | LC_ALL=C sort -u; }

# emit TEXT: TEXT and a newline, or nothing at all when TEXT is empty. `comm` would read one
# empty line from `printf '%s\n' ""` and report it as a difference.
emit() { [ -z "$1" ] || printf '%s\n' "$1"; }

# report_differences WANT GOT: prints every line that only one of the two lists holds and
# returns 1 when there is one (2 when comm refuses a list). A line of WANT that GOT lacks is
# a case that was expected to fail and did not, a pinned difference that has gone. A line of
# GOT that WANT lacks is a failure nobody pinned.
report_differences() {
    local want got gone fresh line rc=0
    want="$(sorted_set "$1")"; got="$(sorted_set "$2")"
    gone="$(LC_ALL=C comm --check-order -23 <(emit "$want") <(emit "$got") 2>&1)" \
        || { printf '  !! comm refused the lists: %s\n' "$gone"; return 2; }
    fresh="$(LC_ALL=C comm --check-order -13 <(emit "$want") <(emit "$got") 2>&1)" \
        || { printf '  !! comm refused the lists: %s\n' "$fresh"; return 2; }
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        printf '  - expected to fail and did not: %s\n' "$line"; rc=1
    done <<<"$gone"
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        printf '  + failed and is not pinned: %s\n' "$line"; rc=1
    done <<<"$fresh"
    return "$rc"
}

# The suite that the Rust tree is held to. The self-test points it at a stand-in.
suite_runner="$root/tests/run-tests.sh"
SUITE_PINNED=()   # the assembled binaries carry no build path since main remaps them, so the leak case passes

# suite TREE [ENV...]: runs the suite against TREE and prints its output.
suite() {
    local t="$1"; shift
    env HOME="$scratch/czhome" TMPDIR="$scratch/cztmp" MX_TREE="$t" PYTHONDONTWRITEBYTECODE=1 "$@" bash "$suite_runner" 2>&1
}
tally() { grep -E '[0-9]+ passed, [0-9]+ failed' <<<"$1" | tail -1 | sed 's/^ *//'; }

# suite_comparison PLAIN TREE: runs the suite four times and prints what differs. Four runs.
# The first pair is the whole suite and is information: its cases that grep the text of
# install.sh, new-profile.sh or a platform script fail against a two-line wrapper. The second
# pair skips the source-coupled blocks (MX_SKIP_SOURCE_COUPLED=1, the suite's own switch), so
# what still fails only on the Rust tree is behaviour, and that list is a gate: it may hold
# only the cases named in SUITE_PINNED. Returns 1 when it holds anything else, or when a pinned
# case no longer fails.
suite_comparison() {
    local plain="$1" tree="$2" sb sr sbs srs only behaviour want_suite row diffs
    sb="$(suite "$plain")"; sr="$(suite "$tree")"
    sbs="$(suite "$plain" MX_SKIP_SOURCE_COUPLED=1)"; srs="$(suite "$tree" MX_SKIP_SOURCE_COUPLED=1)"
    printf 'suite, bash tree (no .git)                  : %s\n' "$(tally "$sb")"
    printf 'suite, Rust tree                            : %s\n' "$(tally "$sr")"
    printf 'suite without source-coupled, bash tree     : %s\n' "$(tally "$sbs")"
    printf 'suite without source-coupled, Rust tree     : %s\n' "$(tally "$srs")"
    echo "cases that fail only on the Rust tree (the whole suite; source-coupled cases included):"
    only="$(LC_ALL=C comm --check-order -13 <(emit "$(fail_lines "$sb")") <(emit "$(fail_lines "$sr")") 2>&1)" \
        || { printf '  !! comm refused the lists: %s\n' "$only"; return 1; }
    emit "$only" | sed 's/^/  /'
    behaviour="$(LC_ALL=C comm --check-order -13 <(emit "$(fail_lines "$sbs")") <(emit "$(fail_lines "$srs")") 2>&1)" \
        || { printf '  !! comm refused the lists: %s\n' "$behaviour"; return 1; }
    echo "cases that fail only on the Rust tree with the source-coupled blocks skipped (behaviour):"
    emit "$behaviour" | sed 's/^/  /'
    want_suite=""
    for row in "${SUITE_PINNED[@]}"; do want_suite+="FAIL  ${row%%|*}"$'\n'; done
    if ! diffs="$(report_differences "$want_suite" "$behaviour")"; then
        echo "  !! the behavioural failures are not the pinned ones"
        emit "$diffs"
        return 1
    fi
    for row in "${SUITE_PINNED[@]}"; do printf '  - %s\n      %s\n' "${row%%|*}" "${row#*|}"; done
    return 0
}

# --- --self-test ------------------------------------------------------------------------------
# A stand-in suite lists its cases with `find ... -print0`, so it prints them in directory
# order. The same stand-in runs twice, once with the real find and once with a find that
# reverses its NUL-separated records, which is what another file system does. The raw output
# must differ (or the stub proves nothing) and every verdict and report must be identical.
self_test() {
    local st="$scratch/self-test" checks=0 failed=0 name
    rm -rf "$st"; mkdir -p "$st/stubfind" "$st/plain/cases" "$st/rust/cases" "$scratch/czhome" "$scratch/cztmp" \
        || die "cannot make $st"
    check() {  # check DESCRIPTION COMMAND [ARG...]: passes when the command succeeds
        local description="$1"; shift
        checks=$((checks + 1))
        if "$@"; then
            printf '  PASS  %s\n' "$description"
        else
            printf '  FAIL  %s\n' "$description"; failed=$((failed + 1))
        fi
    }
    cat > "$st/fake-suite.sh" <<'RUNNER'
#!/usr/bin/env bash
# Stands for tests/run-tests.sh: one case per file under $MX_TREE/cases, in the order find lists them.
set -uo pipefail
passed=0; failed=0
while IFS= read -r -d '' f; do
    name="$(basename "$f" .case)"
    if [ "$(cat "$f")" = pass ]; then
        printf '  PASS  %s\n' "$name"; passed=$((passed + 1))
    else
        printf '  FAIL  %s\n' "$name" >&2; failed=$((failed + 1))
    fi
done < <(find "${MX_TREE:?}/cases" -name '*.case' -print0)
printf '%s passed, %s failed\n' "$passed" "$failed"
RUNNER
    cat > "$st/stubfind/find" <<'STUB'
#!/usr/bin/env bash
# The real find, except that a listing made with -print0 comes back with its records reversed.
real=""
for candidate in /usr/bin/find /bin/find; do [ -x "$candidate" ] && { real="$candidate"; break; }; done
[ -n "$real" ] || { echo "stub find: no real find" >&2; exit 127; }
case " $* " in *" -print0 "*) ;; *) exec "$real" "$@" ;; esac
tmp="$(mktemp)" || exit 1
"$real" "$@" > "$tmp" || { rc=$?; rm -f "$tmp"; exit "$rc"; }
recs=()
while IFS= read -r -d '' rec; do recs+=("$rec"); done < "$tmp"
rm -f "$tmp"
for (( i = ${#recs[@]} - 1; i >= 0; i-- )); do printf '%s\0' "${recs[i]}"; done
STUB
    bash -n "$st/fake-suite.sh" && bash -n "$st/stubfind/find" || die "a self-test script does not parse"
    chmod +x "$st/stubfind/find"
    # Names that sort differently under the C locale and under a language locale.
    local all=(Zeta alpha Beta a-b a_c Éclair)
    for name in "${all[@]}"; do printf 'pass\n' > "$st/plain/cases/$name.case"; printf 'pass\n' > "$st/rust/cases/$name.case"; done
    printf 'fail\n' > "$st/plain/cases/alpha.case"; printf 'fail\n' > "$st/plain/cases/Zeta.case"
    for name in alpha Zeta Beta a-b; do printf 'fail\n' > "$st/rust/cases/$name.case"; done
    suite_runner="$st/fake-suite.sh"
    local saved_pins=("${SUITE_PINNED[@]}") saved_path="$PATH"

    # The stub must change the order, or running twice proves nothing.
    local normal reversed
    normal="$(MX_TREE="$st/rust" bash "$suite_runner" 2>&1)"
    reversed="$(PATH="$st/stubfind:$PATH" MX_TREE="$st/rust" bash "$suite_runner" 2>&1)"
    check "the stub find changes the order the stand-in suite prints its cases in" \
        test "$normal" != "$reversed"
    check "fail_lines gives one list for both orders" \
        test "$(fail_lines "$normal")" = "$(fail_lines "$reversed")"
    check "fail_lines sorts under the C locale: capitals first" \
        test "$(fail_lines "$normal")" = "$(printf '%s\n' 'FAIL  Beta' 'FAIL  Zeta' 'FAIL  a-b' 'FAIL  alpha')"

    # scenario PINS EXPECTED_RC LINES...: the comparison under the real find, under the reversing
    # find and, when the machine has the locale, under a language locale. All three give the same
    # report and the same verdict, and the report holds exactly the expected lines.
    scenario() {
        local label="$1" want_rc="$2" want_lines="$3" out1 rc1 out2 rc2 out3 rc3
        out1="$(suite_comparison "$st/plain" "$st/rust")"; rc1=$?
        out2="$(PATH="$st/stubfind:$saved_path"; suite_comparison "$st/plain" "$st/rust")"; rc2=$?
        check "$label: verdict $want_rc under the real listing" test "$rc1" = "$want_rc"
        check "$label: verdict $want_rc under the reversed listing" test "$rc2" = "$want_rc"
        check "$label: the same report under both listings" test "$out1" = "$out2"
        local have_lines
        have_lines="$(grep -E '^  [-+] (expected|failed)' <<<"$out1" || true)"
        check "$label: the report holds every difference and nothing else" test "$have_lines" = "$want_lines"
        if locale -a 2>/dev/null | grep -qix 'en_us.utf-\?8'; then
            out3="$(export LC_ALL=en_US.UTF-8; PATH="$st/stubfind:$saved_path"; suite_comparison "$st/plain" "$st/rust")"; rc3=$?
            check "$label: verdict $want_rc under en_US.UTF-8 with the reversed listing" test "$rc3" = "$want_rc"
            check "$label: the same report under en_US.UTF-8 with the reversed listing" test "$out3" = "$out1"
        else
            printf '  note  %s: en_US.UTF-8 is not installed here, the language-locale run is not made\n' "$label"
        fi
    }
    SUITE_PINNED=()
    scenario "no case is pinned: both new failures are reported" 1 \
        "$(printf '%s\n' '  + failed and is not pinned: FAIL  Beta' '  + failed and is not pinned: FAIL  a-b')"
    SUITE_PINNED=("Beta|reason" "a-b|reason")
    scenario "both new failures are pinned: no difference" 0 ""
    SUITE_PINNED=("Beta|reason" "a-b|reason" "a_c|reason")
    scenario "a pinned case that passes is reported" 1 \
        "$(printf '%s\n' '  - expected to fail and did not: FAIL  a_c')"
    SUITE_PINNED=("a-b|reason" "Beta|reason")
    scenario "pinned cases listed out of order: no difference" 0 ""
    SUITE_PINNED=("Beta|reason" "a-b|reason" "Éclair|reason" "a_c|reason")
    scenario "several pinned cases that pass are all reported, in C order" 1 \
        "$(printf '%s\n' '  - expected to fail and did not: FAIL  a_c' '  - expected to fail and did not: FAIL  Éclair')"
    SUITE_PINNED=("Beta|reason" "a_c|reason")
    scenario "an unpinned failure and a pinned case that passes are both reported" 1 \
        "$(printf '%s\n' '  - expected to fail and did not: FAIL  a_c' '  + failed and is not pinned: FAIL  a-b')"

    # The guard against an unsorted list can only fire if this machine's comm refuses one.
    local rc
    LC_ALL=C comm --check-order -13 <(printf 'b\na\n') <(printf 'a\n') >/dev/null 2>&1; rc=$?
    check "comm --check-order refuses an unsorted list on this machine" test "$rc" != 0

    SUITE_PINNED=("${saved_pins[@]}"); PATH="$saved_path"
    printf 'self-test: %d checks passed, %d failed\n' "$((checks - failed))" "$failed"
    [ "$failed" = 0 ] && [ "$checks" -gt 0 ]
}
if [ "$self_test" = 1 ]; then self_test; exit $?; fi

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
failing_of() { grep -E '^FAIL  ' <<<"$1" | sed 's/^FAIL  //' | LC_ALL=C sort -u; }

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
want="$(printf '%s\n' "${names[@]}" | LC_ALL=C sort -u)"
prev=""; status=0
for n in 1 2 3; do
    out="$(run_set "$tree")"; rc=$?
    sum="$(summary_of "$out")"
    [ -n "$sum" ] || die "run $n of the Rust tree printed no summary (exit $rc)"
    total="${sum%% cases*}"
    got="$(failing_of "$out")"
    printf 'rust tree run %s: %s\n' "$n" "$sum"
    if [ "$total" != "$base_total" ]; then echo "  !! $total cases ran, the bash tree ran $base_total"; status=1; fi
    if ! diffs="$(report_differences "$want" "$got")"; then
        echo "  !! the failing cases are not the pinned ones"
        emit "$diffs" | sed 's/^/  /'
        status=1
    fi
    if [ -n "$prev" ] && [ "$prev" != "$sum|$got" ]; then echo "  !! run $n differs from run 1"; status=1; fi
    [ -n "$prev" ] || prev="$sum|$got"
done
printf 'pinned differences (%d), the only failures allowed:\n' "${#PINNED[@]}"
for row in "${PINNED[@]}"; do printf '  - %s\n      %s\n' "${row%%|*}" "${row#*|}"; done

# --- tests/run-tests.sh, plain tree and Rust tree --------------------------------------------
# suite_comparison, above, runs the suite four times on a plain copy of the bash tree and on
# the Rust tree. Its list of behavioural failures is a gate: it may hold only SUITE_PINNED.
if [ "$with_suite" = 1 ]; then
    plain="$scratch/bb-plain"
    rm -rf "$plain"; mkdir -p "$plain"
    list="$scratch/bb-list"
    git -C "$root" ls-files -z --cached --others --exclude-standard > "$list" || die "git ls-files failed"
    tar -C "$root" --null --ignore-failed-read -T "$list" -cf - | tar -C "$plain" -xf - || die "copying the plain tree failed"
    mkdir -p "$scratch/czhome" "$scratch/cztmp"
    suite_comparison "$plain" "$tree" || status=1
fi

[ "$status" = 0 ] || { echo "RESULT: FAIL"; exit 1; }
echo "RESULT: PASS ($base_total cases; ${#PINNED[@]} pinned differences)"
