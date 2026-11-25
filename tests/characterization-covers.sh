#!/usr/bin/env bash
# The characterization suite lists every path its cases characterize (FU1-2).
#
# profiles/magent/verify.d/characterization.sh names, in suite_covers, the paths a receipt from
# that suite may cover. The list is read from the tree here and not typed a second time: the
# opening sentence of each case file under tests/characterization/ says what the file
# characterizes ("# Characterization: scripts/check_claims.sh. It runs the suite once, ..."), and
# a tracked path named there is a path a case runs. Four check scripts gained Rust builds and
# cases while suite_covers listed none of them, so a receipt for a change to one of them could not
# be earned from the suite that runs it.
#
# It is a file of its own because tests/run-tests.sh was one 12,000 line file that shellcheck
# needed about 9 GB for, and 45 more lines of data flow took that past the 12 GB cap of the lint
# sweep. The cases now live in tests/suites/, a part lints in under 0.5 GB, and this file stays
# separate. tests/run-tests.sh runs this file and counts one case for it. MX_TREE names the tree under test,
# as for the rest of the suite; the case files are read from this checkout.
set -uo pipefail
suite_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
root="${MX_TREE:-$suite_root}"
cs="$root/profiles/magent/verify.d/characterization.sh"
cdir="$suite_root/tests/characterization"
pass=0; fail=0
ok() { printf '  PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf '  FAIL  %s\n' "$1" >&2; fail=$((fail + 1)); }

declare -A tracked=()
while IFS= read -r -d '' p; do tracked["$p"]=1; done < <(git -C "$root" ls-files -z 2>/dev/null)
paths=""; files=0; nomarker=""; unnamed=""
for f in "$cdir"/*-*.sh; do
    case "${f##*/}" in lib-*) continue ;; esac
    files=$((files + 1))
    text="$(awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); printf "%s ", $0; next } { exit }' "$f")"
    case "$text" in
        "Characterization: "*) ;;
        *) nomarker="$nomarker ${f##*/}"; continue ;;
    esac
    text="${text#Characterization: }"
    text="${text%%. *}"   # the first sentence: a period and a space end it
    named=0
    while IFS= read -r tok; do
        tok="${tok%.}"
        [ -n "$tok" ] && [ -n "${tracked[$tok]:-}" ] || continue
        named=$((named + 1))
        case $'\n'"$paths" in *$'\n'"$tok"$'\n'*) ;; *) paths="$paths$tok"$'\n' ;; esac
    done < <(printf '%s\n' "$text" | tr -c 'A-Za-z0-9_./@+-' '\n')
    # A repository check is a script, so its case file must name it. A header reworded so that no
    # path is found would otherwise derive nothing for it and the test below would pass anyway.
    case "${f##*/}" in checks-*) [ "$named" -gt 0 ] || unnamed="$unnamed ${f##*/}" ;; esac
done
[ "$files" -gt 0 ] && [ -z "$nomarker" ] \
    && ok "every one of the $files characterization case files opens with 'Characterization:'" \
    || bad "case files with no 'Characterization:' opening (or no case files at all):${nomarker:- none found}"
[ -z "$unnamed" ] \
    && ok "every repository-check case file names the tracked script it characterizes" \
    || bad "no tracked path is named in the opening sentence of:$unnamed"
count="$(printf '%s' "$paths" | grep -c .)"
# shellcheck disable=SC1090
missing="$( . "$cs" >/dev/null 2>&1; while IFS= read -r p; do [ -n "$p" ] || continue; suite_covers "$p" || printf ' %s' "$p"; done <<<"$paths" )"
[ "${count:-0}" -gt 0 ] && [ -z "$missing" ] \
    && ok "the characterization suite lists all $count paths its cases characterize" \
    || bad "suite_covers does not list ${missing:-anything}: $count path(s) derived from $files case files"

# The list stays exact: a path no case runs is not covered, so widening an entry to a whole
# directory turns this red.
for p in scripts/with-hook-lock.sh tests/run-tests.sh; do
    # shellcheck disable=SC1090
    ( . "$cs" >/dev/null 2>&1; suite_covers "$p" ) \
        && bad "the characterization suite claims $p, which no case runs" \
        || ok "the characterization suite does not cover $p"
done

printf '%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ] && [ "$pass" -gt 0 ]
