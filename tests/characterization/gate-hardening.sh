#!/usr/bin/env bash
# Characterization: the hardening of bin/maintainer-merge from the review of the
# Rust port (found-not-fixed items 1 to 5 and 9). Plan decisions PB-1 to PB-6.
#
# Each case here fails on the tree before the fix. Where a case pins what the
# old tree did and the new tree refuses, the refusal text is the contract the
# Rust gate matches.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
cz_init

# --- PB-1: the test filter reaches `sh -c` in the container ----------------------------

FILTER_MSG="maintainer-merge: the test filter may hold only letters, digits and _ . : / - and spaces, and may not start with -:"

case_filter_refused() {
    cz_mm verify 7 abcdef0123 "$FILTER" 's/a/b/'
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr_has "$FILTER_MSG"
    cz_expect_gh_count "pr view" 0
}
# The first one is the forgery the review reproduced: it prints a passing
# summary for the clean run and a failing one for the mutated run.
for FILTER in \
    'x; grep -q OLD f && echo "test result: ok. 3 passed; 0 failed" || { echo "test result: FAILED"; exit 101; }' \
    'a;b' 'a$(id)' 'a`id`' 'a|b' 'a&b' 'a>b' 'a<b' 'a"b' "a'b" $'a\nb' 'a*b' 'a?b' 'a(b)' 'a{b}' 'a\b' 'a#b' \
    '-f' '--help' '-x some_test'; do
    cz_case "verify: the filter '${FILTER:0:40}' is refused before any container or gh call" case_filter_refused
done

case_filter_accepted() {
    cz_gh pr-view.files ""
    cz_mm verify 7 abcdef0123 "$FILTER" 's/a/b/'
    cz_expect_rc 1
    cz_expect_stderr_first "maintainer-merge: could not read the changed files for #7"
}
for FILTER in 'tests::guard_bites' 'a b' 'crates/x/src/lib.rs' 'run-backends.sh' 'T_1.2-3:x'; do
    cz_case "verify: the filter '$FILTER' passes the allow-list and goes on to read the changed files" case_filter_accepted
done

# verify-part takes the same four arguments through the same function.
case_filter_refused_part() {
    cz_mm verify-part 7 abcdef0123 'a;b' 's/a/b/' rust
    cz_expect_rc 1
    cz_expect_stderr_has "$FILTER_MSG"
}
cz_case "verify-part: a filter with a metacharacter is refused too" case_filter_refused_part

# --- PB-2: show with no argument built Python code from a receipt file name ----------------

case_show_list_hostile_name() {
    cz_repo_basic
    cz_receipt 7 "$CZ_C0" observed
    local name="7.json'));import os;os.system('touch '+os.environ['CZ_CASE_DIR']+chr(47)+'MARK');(('x.json"
    cp "$CZ_RECEIPTS/7.json" "$CZ_RECEIPTS/$name"
    cz_mm show
    cz_expect_rc 0
    cz_expect_no_file "$CZ_CASE/MARK"
    cz_expect_stdout_has "  7: ${CZ_C0:0:8} characterization fixture"
    cz_check "lines listed" "$(wc -l < "$CZ_CASE/stdout" | tr -d ' ')" "2"
}
cz_case "show: a receipt file name holding Python code is listed as text and never run" case_show_list_hostile_name

case_show_arg_refused() {
    printf '{"outside": "the receipts directory"}\n' > "$CZ_STATE/x.json"
    cz_mm show "$ARG"
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr "maintainer-merge: pull request must be a positive integer, not '$ARG'"
}
for ARG in abc ../x 1/../../y ../notes 7x -7 '7 8'; do
    cz_case "show $ARG: a non-number is refused as a pull request number, never read as a path" case_show_arg_refused
done

# --- PB-3: only on and off are values of MAINTAINER_POST -----------------------------------

case_post_unknown() {
    cz_merge_ready 7
    cz_set MAINTAINER_POST "$POST_VALUE"
    cz_mm merge 7
    cz_expect_rc 64
    cz_expect_stdout_empty
    cz_expect_stderr "maintainer-merge: MAINTAINER_POST must be on or off, not $(printf '%q' "$POST_VALUE")"
    cz_expect_gh_count "pr merge" 0
}
for POST_VALUE in OFF Off false 0 no yes true ' off' 'off '; do
    cz_case "POST='$POST_VALUE': refused with exit 64 and a named message, no merge" case_post_unknown
done

case_post_unknown_show() {
    cz_set MAINTAINER_POST maybe
    cz_mm show
    cz_expect_rc 64
    cz_expect_stderr "maintainer-merge: MAINTAINER_POST must be on or off, not maybe"
}
cz_case "POST=maybe: every subcommand refuses, show included" case_post_unknown_show

case_post_on() {
    cz_merge_ready 7
    cz_set MAINTAINER_POST on
    cz_mm merge 7
    cz_expect_rc 0
    cz_expect_gh_count "pr merge" 1
}
cz_case "POST=on: the merge is made" case_post_on

case_post_empty() {
    cz_merge_ready 7
    cz_set MAINTAINER_POST ""
    cz_mm merge 7
    cz_expect_rc 0
    cz_expect_gh_count "pr merge" 1
}
cz_case "POST empty: reads as the default, on, and the merge is made (unchanged)" case_post_empty

# --- PB-4: PROD_GLOBS after its first line were dropped ------------------------------------

case_globs_newline() {
    cz_repo_basic
    cz_write lib/a.sh "echo a"; cz_commit lib-added C1
    cz_pr_ref 7 "$C1"; cz_gh_baseline "$C1"; cz_receipt 7 "$CZ_C0" observed
    cz_set PROD_GLOBS $'crates/*/src/*\nlib/*'
    cz_mm merge 7
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr_first "maintainer-merge: production code changed since the verified head $CZ_C0:"
    cz_expect_stderr_has "    lib/a.sh"
    cz_expect_gh_count "pr merge" 0
}
cz_case "PROD_GLOBS on two lines: the second line counts, so a lib/ change after the receipt is refused" case_globs_newline

case_globs_mixed_blanks() {
    cz_repo_basic
    cz_write lib/a.sh "echo a"; cz_commit lib-added C1
    cz_pr_ref 7 "$C1"; cz_gh_baseline "$C1"; cz_receipt 7 "$CZ_C0" observed
    cz_set PROD_GLOBS $'  crates/*/src/*\t\n\n  lib/*  \n'
    cz_mm merge 7
    cz_expect_rc 1
    cz_expect_stderr_has "    lib/a.sh"
}
cz_case "PROD_GLOBS with tabs, blank lines and padding: every pattern still counts" case_globs_mixed_blanks

case_globs_one_line() {
    cz_repo_basic
    cz_write lib/a.sh "echo a"; cz_commit lib-added C1
    cz_pr_ref 7 "$C1"; cz_gh_baseline "$C1"; cz_receipt 7 "$CZ_C0" observed
    cz_set PROD_GLOBS 'crates/*/src/* lib/*'
    cz_mm merge 7
    cz_expect_rc 1
    cz_expect_stderr_has "    lib/a.sh"
}
cz_case "PROD_GLOBS on one line, blank separated: unchanged" case_globs_one_line

# --- PB-5: a mutation that starts with a dash was read as a sed option -------------------

# Drives apply_mutation itself, as tests/run-tests.sh does, because the real
# path needs a container.
case_mutation_dash() {
    local t="$CZ_CASE/tree"; mkdir -p "$t"
    printf 'GUARD=on\n' > "$t/real.sh"
    printf 's/GUARD=on/GUARD=SCRIPT/\n' > "$CZ_CASE/script.sed"
    local rc=0
    # shellcheck source=/dev/null
    ( . "$CZ_MM" >/dev/null 2>&1
      suite_mutate_glob() { printf '%s\n' '*.sh'; }
      apply_mutation "$t" "-f$CZ_CASE/script.sed" ) >/dev/null 2>&1 || rc=$?
    cz_check "the tree after a -f mutation" "$(cat "$t/real.sh")" "GUARD=on"
    [ "$rc" != 0 ] || cz_note_fail "apply_mutation accepted a mutation that is a sed option"
}
cz_case "apply_mutation: a mutation starting with -f is an expression, so a host script never runs" case_mutation_dash

case_mutation_dash_n() {
    local t="$CZ_CASE/tree"; mkdir -p "$t"
    printf 'GUARD=on\n' > "$t/real.sh"
    local rc=0
    # shellcheck source=/dev/null
    ( . "$CZ_MM" >/dev/null 2>&1
      suite_mutate_glob() { printf '%s\n' '*.sh'; }
      apply_mutation "$t" "-n" ) >/dev/null 2>&1 || rc=$?
    cz_check "the tree after a -n mutation" "$(cat "$t/real.sh")" "GUARD=on"
    [ "$rc" != 0 ] || cz_note_fail "apply_mutation accepted -n"
}
cz_case "apply_mutation: a mutation of -n is refused and nothing changes (control, green before the fix)" case_mutation_dash_n

case_mutation_plain() {
    local t="$CZ_CASE/tree"; mkdir -p "$t"
    printf 'GUARD=on\n' > "$t/real.sh"
    local rc=0
    # shellcheck source=/dev/null
    ( . "$CZ_MM" >/dev/null 2>&1
      suite_mutate_glob() { printf '%s\n' '*.sh'; }
      apply_mutation "$t" 's/GUARD=on/GUARD=off/' ) >/dev/null 2>&1 || rc=$?
    cz_check "exit code" "$rc" "0"
    cz_check "the tree after an ordinary mutation" "$(cat "$t/real.sh")" "GUARD=off"
}
cz_case "apply_mutation: an ordinary expression still applies (control)" case_mutation_plain

# --- PB-6: suite_extra_args was split on every blank ---------------------------------------

case_extra_args() {
    local n
    # shellcheck source=/dev/null
    n="$( . "$CZ_MM" >/dev/null 2>&1
      suite_podman_args() { printf '%s\n' -v "$HOME/h o/.cargo:/cargo:O" -e X=1; }
      split_extra_args podman "$CZ_CASE/scratch"
      printf '%s|%s|%s' "${#EXTRA_ARGS[@]}" "${EXTRA_ARGS[1]}" "${EXTRA_ARGS[3]}" )"
    cz_check "count, the argument holding a blank, the last" "$n" "4|$HOME/h o/.cargo:/cargo:O|X=1"
}
cz_case "suite_extra_args: one argument per line, a blank inside an argument stays inside it" case_extra_args

case_extra_args_none() {
    local n
    # shellcheck source=/dev/null
    n="$( . "$CZ_MM" >/dev/null 2>&1
      split_extra_args podman "$CZ_CASE/scratch"
      printf '%s' "${#EXTRA_ARGS[@]}" )"
    cz_check "count for a suite with no suite_podman_args" "$n" "0"
}
cz_case "suite_extra_args: a suite that declares no extra arguments gives none" case_extra_args_none

cz_finish
