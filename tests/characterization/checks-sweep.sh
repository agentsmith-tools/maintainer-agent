#!/usr/bin/env bash
# Characterization: scripts/shellcheck-sweep.sh. It lists the shell scripts of the git
# checkout it lives in, waits for free memory, runs one linter process per script (inside a
# memory-capped systemd scope when the host has one, at low priority) and exits with the
# worst linter status. The cases pin the file selection (including the names it silently
# skips), every message and exit code, the memory gate, and the exact chain of programs each
# linter run goes through (systemd-run, nice, shellcheck) in order.
#
# The linter, systemd-run, nice and sleep are logging stubs; the other tools are the real
# ones. git is real: each case builds a small checkout.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib-checks.sh"
cc_init

SELF=scripts/shellcheck-sweep.sh
SC="shellcheck --severity=warning"
NO_CAP_MSG="sweep: no memory cap (no systemd user scope on this host); relying on the free-memory check."
SD_PROBE="systemd-run --user --scope --quiet -p MemoryMax=18G -p MemorySwapMax=0 true"
SHEBANG='#!/usr/bin/env bash\necho hi\n'

# sw_base [systemd]: the tree, the stubs, plenty of memory, no waiting.
sw_base() {
    cc_sweep_tree; cc_sweep_stubs "$@"
    cc_logstub sleep 'true'
    cc_meminfo 20000000
    cz_set MAGENT_SWEEP_WAIT 0; cz_set MAGENT_SWEEP_POLL 1
}
# sw_files PATH...: the order log of an uncapped run over these files: nice, then the linter.
sw_plain() {
    local f
    for f in "$@"; do printf '%s\n' "nice -n 15 $SC $f" "$SC $f"; done
}
# sw_capped CAP PATH...: the order log of a capped run, probe first.
sw_capped() {
    local cap="$1" f; shift
    printf '%s\n' "systemd-run --user --scope --quiet -p MemoryMax=$cap -p MemorySwapMax=0 true"
    for f in "$@"; do
        printf '%s\n' "systemd-run --user --scope --quiet -p MemoryMax=$cap -p MemorySwapMax=0 nice -n 15 $SC $f" \
            "nice -n 15 $SC $f" "$SC $f"
    done
}

case_selection() {
    sw_base
    cc_sweep_file a.sh "$SHEBANG"
    cc_sweep_file b '#!/bin/bash\necho hi\n'
    cc_sweep_file c.sh '#!/bin/sh\necho hi\n'
    cc_sweep_file d.py '#!/usr/bin/env python3\nprint(1)\n'
    cc_sweep_file e.txt 'plain text\n'
    cc_sweep_file profiles/p/verify.d/x.sh 'echo a fragment with no shebang\n'
    cc_sweep_file profiles/p/verify.d/y.md 'not a script\n'
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cz_expect_stdout "checking 5 scripts"
    cz_expect_stderr "$NO_CAP_MSG"
    cc_expect_calls "$(sw_plain a.sh b c.sh profiles/p/verify.d/x.sh "$SELF")"
}
cc_case "the sweep lists the tracked files whose first bytes name bash or sh and the verify.d fragments, sorted, and lints each once" case_selection

case_shebang_forms() {
    sw_base
    cc_sweep_file env-sh '#!/usr/bin/env sh\n'
    cc_sweep_file zsh '#!/usr/bin/env zsh\n'
    cc_sweep_file dash '#!/bin/dash\n'
    cc_sweep_file shell '#!/bin/shell\n'
    cc_sweep_file bashbang '#!/bin/bash -eu\n'
    cc_sweep_file comment '# see #!/bin/bash for the rules\n'
    cc_sweep_file edge "$(printf 'x%.0s' $(seq 1 71))"'#!/bin/sh\n'
    cc_sweep_file late "$(printf 'x%.0s' $(seq 1 72))"'#!/bin/sh\n'
    cc_sweep_file early "$(printf 'x%.0s' $(seq 1 50))"'#!/bin/sh\n'
    cc_sweep_file nulls '\0\0\0#!/bin/bash\n'
    cc_sweep_file nuls-inside '#!/usr/bin/env\0 bash\n'
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    # The test is a substring match over the first 80 bytes with the NULs removed: '#!/bin/shell'
    # contains '#!/bin/sh', a shebang after 50 characters of text still counts, one that ends at
    # byte 80 counts and one that ends at byte 81 does not, and '#!/usr/bin/env zsh' and '#!/usr/bin/env sh' name neither.
    cz_expect_stdout "checking 8 scripts"
    cz_expect_stderr "$NO_CAP_MSG"
    cc_expect_calls "$(sw_plain bashbang comment early edge nulls nuls-inside "$SELF" shell)"
}
cc_case "a file counts when its first 80 bytes, NULs removed, hold one of the three shebang strings anywhere" case_shebang_forms

case_verify_d_pattern() {
    sw_base
    cc_sweep_file profiles/p/q/verify.d/deep/z.sh 'echo deep\n'
    cc_sweep_file profiles/verify.d/x.sh 'echo no profile name\n'
    cc_sweep_file profiles/p/verify.d/x.bash 'echo wrong extension\n'
    cc_sweep_file other/verify.d/x.sh 'echo wrong root\n'
    cc_sweep_file profiles/p/verify.d/dup.sh '#!/bin/bash\necho both rules\n'
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    # In a case pattern '*' matches '/', so the profile and the file name may hold slashes.
    cz_expect_stdout "checking 3 scripts"
    cc_expect_calls "$(sw_plain profiles/p/q/verify.d/deep/z.sh profiles/p/verify.d/dup.sh "$SELF")"
}
cc_case "profiles/*/verify.d/*.sh matches across slashes, a file that matches both rules is linted once, and near misses are not" case_verify_d_pattern

case_untracked_and_deleted() {
    sw_base
    cc_sweep_file tracked.sh "$SHEBANG"
    cc_sweep_file gone.sh "$SHEBANG"
    cc_sweep_commit
    rm "$CZ_TREE/gone.sh"
    cc_sweep_file untracked.sh "$SHEBANG"
    cc_sweep
    cz_expect_rc 0
    cz_expect_stdout "checking 2 scripts"
    cc_expect_calls "$(sw_plain "$SELF" tracked.sh)"
}
cc_case "an untracked file is not swept and a tracked file that is gone from the disk is skipped" case_untracked_and_deleted

# Pins current behaviour. The list comes from `for f in $(git ls-files)`, so a name is split at
# whitespace and a name git quotes (non-ASCII characters) fails the file test. Neither file is
# linted and nothing says so. The same word-splitting hides a name that starts with '-' from
# `head`, which reads it as an option.
case_names_silently_skipped() {
    sw_base
    cc_sweep_file 'my script.sh' "$SHEBANG"
    cc_sweep_file 'tab	name.sh' "$SHEBANG"
    cc_sweep_file 'caf'$'\303\251''.sh' "$SHEBANG"
    cc_sweep_file -x.sh "$SHEBANG"
    cc_sweep_file ok.sh "$SHEBANG"
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cz_expect_stdout "checking 2 scripts"
    cz_expect_stderr "$NO_CAP_MSG"
    cc_expect_calls "$(sw_plain ok.sh "$SELF")"
}
cc_case "names with whitespace, names git quotes and names that start with a dash are silently left out (pins current behaviour)" case_names_silently_skipped

case_no_scripts() {
    sw_base
    cc_sweep_file notes.txt 'text\n'
    cc_sweep_commit
    "${CC_GIT[@]}" rm -q --cached "$SELF" && "${CC_GIT[@]}" commit -q -m drop || exit 2
    cc_sweep
    cz_expect_rc 1
    cz_expect_stdout "checking 0 scripts"
    cz_expect_stderr "no scripts found; this gate inspected nothing"
    cc_expect_calls
}
cc_case "a checkout with no shell script prints 'checking 0 scripts', says it inspected nothing on stderr and exits 1" case_no_scripts

case_not_a_git_checkout() {
    sw_base
    cc_sweep_file a.sh "$SHEBANG"
    rm -rf "$CZ_TREE/.git"
    cc_sweep
    cz_expect_rc 1
    cz_expect_stdout "checking 0 scripts"
    # git words its own error differently by version and by mount layout, so only its first
    # word is pinned, and that it comes before the sweep's own line.
    cz_expect_matches stderr '^fatal: not a git repository'
    cz_check "last stderr line" "$(tail -1 "$CZ_CASE/stderr")" "no scripts found; this gate inspected nothing"
}
cc_case "outside a git checkout git's own error reaches stderr before the 'inspected nothing' line, exit 1" case_not_a_git_checkout

# --- the linter is missing ----------------------------------------------------------------------

case_no_linter() {
    cc_sweep_tree
    cc_sweep_file a.sh "$SHEBANG"; cc_sweep_commit
    cc_sweep
    cz_expect_rc 127
    cz_expect_stdout "checking 2 scripts"
    cz_expect_stderr \
        "shellcheck is not installed, so this gate inspected nothing." \
        "" \
        "  Debian/Ubuntu:  sudo apt-get install -y shellcheck" \
        "  No sudo:        install the static binary from" \
        "                  https://github.com/koalaman/shellcheck/releases into ~/.local/bin" \
        "" \
        "CI runs it either way, so a push will find what this did not."
}
cc_case "with no shellcheck on PATH the sweep prints the install hint on stderr and exits 127, before it looks at memory" case_no_linter

case_no_linter_no_scripts_order() {
    cc_sweep_tree
    cc_sweep_file notes.txt 'text\n'; cc_sweep_commit
    "${CC_GIT[@]}" rm -q --cached "$SELF" && "${CC_GIT[@]}" commit -q -m drop || exit 2
    cc_sweep
    cz_expect_rc 1
    cz_expect_stderr "no scripts found; this gate inspected nothing"
}
cc_case "the empty-list refusal (exit 1) comes before the missing-linter refusal (exit 127)" case_no_linter_no_scripts_order

# --- the memory gate ---------------------------------------------------------------------------------

case_memory_enough_exact() {
    sw_base
    cc_meminfo 10485760
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cz_expect_stdout "checking 1 scripts"
    cc_expect_calls "$(sw_plain "$SELF")"
}
cc_case "MemAvailable equal to the default need of 10485760 KB starts the sweep" case_memory_enough_exact

case_memory_one_short() {
    sw_base
    cc_meminfo 10485759
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 75
    cz_expect_stdout "checking 1 scripts"
    cz_expect_stderr \
        "sweep: MemAvailable is 10485759 KB and the linter needs 10485760 KB; gave up after 0s." \
        "sweep: close something heavy and retry, or set MAGENT_SWEEP_MIN_AVAIL_KB."
    cc_expect_calls
}
cc_case "one kilobyte short of the need with no wait gives up at once: two stderr lines, exit 75, no sleep, no linter" case_memory_one_short

case_memory_waits_then_gives_up() {
    sw_base
    cc_meminfo 1000
    cz_set MAGENT_SWEEP_WAIT 3; cz_set MAGENT_SWEEP_POLL 1
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 75
    cz_expect_stderr_has "sweep: MemAvailable is 1000 KB and the linter needs 10485760 KB; gave up after 3s."
    cc_expect_calls "sleep 1" "sleep 1" "sleep 1"
}
cc_case "it sleeps one poll interval at a time until the wait is spent, then gives up, naming the wait" case_memory_waits_then_gives_up

case_memory_poll_size() {
    sw_base
    cc_meminfo 1000
    cz_set MAGENT_SWEEP_WAIT 25; cz_set MAGENT_SWEEP_POLL 10
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 75
    # waited counts 0, 10, 20, 30: the check is >=, so it gives up after 30 s, past the 25 s asked for.
    cz_expect_stderr_has "gave up after 30s."
    cc_expect_calls "sleep 10" "sleep 10" "sleep 10"
}
cc_case "the wait is counted in whole polls and can overshoot the limit (pins current behaviour)" case_memory_poll_size

case_memory_arrives() {
    sw_base
    cc_meminfo 1000
    cz_set MAGENT_SWEEP_WAIT 60; cz_set MAGENT_SWEEP_POLL 1
    cc_logstub sleep 'n="$(grep -c "^sleep" "$d/calls")"
if [ "$n" = 2 ]; then printf "MemTotal: 1 kB\nMemAvailable: 20000000 kB\n" > "$d/meminfo"; fi'
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cc_expect_calls "sleep 1" "sleep 1" "$(sw_plain "$SELF")"
    cz_expect_stderr "$NO_CAP_MSG"
}
cc_case "when MemAvailable rises during the wait the sweep starts without a message about it" case_memory_arrives

case_memory_need_override() {
    sw_base
    cc_meminfo 1000
    cz_set MAGENT_SWEEP_MIN_AVAIL_KB 1000
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cc_expect_calls "$(sw_plain "$SELF")"
}
cc_case "MAGENT_SWEEP_MIN_AVAIL_KB sets the need" case_memory_need_override

case_memory_no_line() {
    sw_base
    printf 'MemTotal: 32000000 kB\n' > "$CZ_CASE/meminfo"
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 75
    cz_expect_stdout "checking 1 scripts"
    cz_expect_stderr "sweep: no usable MemAvailable in $CZ_CASE/meminfo; not starting a job that needs 10485760 KB."
    cc_expect_calls
}
cc_case "a meminfo with no MemAvailable line refuses with exit 75 and names the file and the need" case_memory_no_line

case_memory_not_a_number() {
    sw_base
    printf 'MemAvailable: lots kB\n' > "$CZ_CASE/meminfo"
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 75
    cz_expect_stderr "sweep: no usable MemAvailable in $CZ_CASE/meminfo; not starting a job that needs 10485760 KB."
}
cc_case "a MemAvailable that is not a number refuses the same way" case_memory_not_a_number

case_memory_first_line_wins() {
    sw_base
    printf 'MemAvailable: 1 kB\nMemAvailable: 20000000 kB\n' > "$CZ_CASE/meminfo"
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 75
    cz_expect_stderr_has "sweep: MemAvailable is 1 KB and the linter needs 10485760 KB; gave up after 0s."
}
cc_case "the first MemAvailable line is the one read, not the last" case_memory_first_line_wins

case_unterminated_last_line_is_not_read() {
    sw_base
    printf 'MemTotal: 1 kB\nMemAvailable: 20000000 kB' > "$CZ_CASE/meminfo"
    cc_sweep_commit
    cc_sweep
    # Pins current behaviour: `read` fails on a last line with no newline, so the loop body never sees it.
    cz_expect_rc 75
    cz_expect_stderr_first "sweep: no usable MemAvailable in $CZ_CASE/meminfo; not starting a job that needs 10485760 KB."
}
cc_case "a MemAvailable line with no newline at the end of the file is not read (pins current behaviour)" case_unterminated_last_line_is_not_read

case_meminfo_unreadable() {
    sw_base
    cz_set MAGENT_MEMINFO "$CZ_CASE/absent-meminfo"
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cz_expect_stderr "sweep: $CZ_CASE/absent-meminfo is not readable here; not checking free memory." "$NO_CAP_MSG"
    cc_expect_calls "$(sw_plain "$SELF")"
}
cc_case "an unreadable meminfo is a warning on stderr and the sweep goes on" case_meminfo_unreadable

case_ci_skips_gate_and_cap() {
    sw_base systemd
    cc_meminfo 1
    cz_set CI 1
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cz_expect_stdout "checking 1 scripts"
    cz_expect_stderr_empty
    cc_expect_calls "$(sw_plain "$SELF")"
}
cc_case "under CI neither the memory gate nor the cap runs and nice still does, with no message" case_ci_skips_gate_and_cap

case_ci_empty_is_not_ci() {
    sw_base
    cc_meminfo 1
    cz_set CI ""
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 75
}
cc_case "CI set to the empty string does not count as CI (pins current behaviour)" case_ci_empty_is_not_ci

# --- the cap -------------------------------------------------------------------------------------------

case_capped() {
    sw_base systemd
    cc_sweep_file a.sh "$SHEBANG"; cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cz_expect_stdout "checking 2 scripts"
    cz_expect_stderr_empty
    cc_expect_calls "$(sw_capped 18G a.sh "$SELF")"
}
cc_case "with a user scope the host is probed once with 'true', then every linter run goes systemd-run, nice, shellcheck" case_capped

case_cap_override() {
    sw_base systemd
    cz_set MAGENT_SWEEP_MEM_MAX 3G
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cc_expect_calls "$(sw_capped 3G "$SELF")"
}
cc_case "MAGENT_SWEEP_MEM_MAX sets MemoryMax for the probe and for every run" case_cap_override

case_cap_probe_fails() {
    sw_base systemd
    : > "$CZ_CASE/sd.fail"
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cz_expect_stderr "$NO_CAP_MSG"
    cc_expect_calls "$SD_PROBE" "$(sw_plain "$SELF")"
}
cc_case "a failing probe runs the linter uncapped and says so once on stderr" case_cap_probe_fails

case_no_cap_variable() {
    sw_base systemd
    cz_set MAGENT_SWEEP_NO_CAP 1
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cz_expect_stderr_empty
    cc_expect_calls "$(sw_plain "$SELF")"
}
cc_case "MAGENT_SWEEP_NO_CAP skips the probe and the cap without a message" case_no_cap_variable

case_no_cap_variable_empty() {
    sw_base systemd
    cz_set MAGENT_SWEEP_NO_CAP ""
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cc_expect_calls "$(sw_capped 18G "$SELF")"
}
cc_case "MAGENT_SWEEP_NO_CAP set to the empty string leaves the cap on (pins current behaviour)" case_no_cap_variable_empty

case_no_nice() {
    sw_base systemd
    cc_drop_tool nice
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cc_expect_calls "$SD_PROBE" "systemd-run --user --scope --quiet -p MemoryMax=18G -p MemorySwapMax=0 $SC $SELF" "$SC $SELF"
}
cc_case "with no nice on PATH the linter runs without it" case_no_nice

case_no_nice_no_cap() {
    sw_base
    cc_drop_tool nice
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cz_expect_stderr "$NO_CAP_MSG"
    cc_expect_calls "$SC $SELF"
}
cc_case "with neither a cap nor nice the linter is called directly" case_no_nice_no_cap

# --- the linter's results ------------------------------------------------------------------------------

case_worst_status() {
    sw_base
    cc_sweep_file a.sh "$SHEBANG"; cc_sweep_file b.sh "$SHEBANG"; cc_sweep_file c.sh "$SHEBANG"; cc_sweep_commit
    printf 2 > "$CZ_CASE/sc.rc.a.sh"; printf 1 > "$CZ_CASE/sc.rc.b.sh"; printf 0 > "$CZ_CASE/sc.rc.c.sh"
    cc_sweep
    cz_expect_rc 2
    cc_expect_calls "$(sw_plain a.sh b.sh c.sh "$SELF")"
}
cc_case "a failing script does not stop the sweep, and the exit code is the largest linter status" case_worst_status

case_linter_output_passes_through() {
    sw_base
    cc_sweep_file a.sh "$SHEBANG"; cc_sweep_commit
    printf 'In a.sh line 2: warning on stdout\n' > "$CZ_CASE/sc.out.a.sh"
    printf 'linter diagnostic on stderr\n' > "$CZ_CASE/sc.err.a.sh"
    printf 1 > "$CZ_CASE/sc.rc.a.sh"
    cc_sweep
    cz_expect_rc 1
    cz_expect_stdout "checking 2 scripts" "In a.sh line 2: warning on stdout"
    cz_expect_stderr "$NO_CAP_MSG" "linter diagnostic on stderr"
}
cc_case "the linter's own stdout and stderr reach the matching streams, after the sweep's own lines" case_linter_output_passes_through

case_killed_stops_the_sweep() {
    sw_base systemd
    cc_sweep_file a.sh "$SHEBANG"; cc_sweep_file b.sh "$SHEBANG"; cc_sweep_commit
    printf 1 > "$CZ_CASE/sc.rc.a.sh"; printf 137 > "$CZ_CASE/sc.rc.b.sh"
    cc_sweep
    cz_expect_rc 137
    cz_expect_stderr "sweep: shellcheck was killed (rc=137) on b.sh, most likely by the MemoryMax=18G cap. Raise MAGENT_SWEEP_MEM_MAX."
    cc_expect_calls "$(sw_capped 18G a.sh b.sh)"
}
cc_case "a linter killed with status 137 stops the sweep after that script, names it and the cap, and exits 137" case_killed_stops_the_sweep

case_killed_message_without_cap() {
    sw_base
    cz_set CI 1
    cc_sweep_commit
    printf 137 > "$CZ_CASE/sc.rc"
    cc_sweep
    cz_expect_rc 137
    cz_expect_stderr "sweep: shellcheck was killed (rc=137) on $SELF, most likely by the MemoryMax=18G cap. Raise MAGENT_SWEEP_MEM_MAX."
}
cc_case "the kill message names the default cap even when no cap was applied (pins current behaviour)" case_killed_message_without_cap

case_status_above_137() {
    sw_base
    cc_sweep_file a.sh "$SHEBANG"; cc_sweep_commit
    printf 255 > "$CZ_CASE/sc.rc.a.sh"
    cc_sweep
    cz_expect_rc 255
    cz_expect_lacks stderr "killed"
}
cc_case "a status above 137 is passed on and is not a kill" case_status_above_137

case_killed_cap_text_follows_override() {
    sw_base systemd
    cz_set MAGENT_SWEEP_MEM_MAX 2G
    cc_sweep_commit
    printf 137 > "$CZ_CASE/sc.rc"
    cc_sweep
    cz_expect_stderr "sweep: shellcheck was killed (rc=137) on $SELF, most likely by the MemoryMax=2G cap. Raise MAGENT_SWEEP_MEM_MAX."
}
cc_case "the kill message names the cap in force" case_killed_cap_text_follows_override

# --- where it runs --------------------------------------------------------------------------------------------

case_runs_in_the_tree() {
    sw_base
    cc_sweep_file a.sh "$SHEBANG"; cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    cz_check "working directories of the linter" "$(sort -u "$CZ_CASE/sc.pwd")" "$CZ_TREE"
    cz_check "linter runs" "$(cc_count_lines "$CZ_CASE/sc.pwd")" 2
}
cc_case "the linter runs with the checkout as its working directory, whatever the caller's" case_runs_in_the_tree

case_every_tool_runs_in_the_checkout() {
    sw_base systemd
    cc_meminfo 1000
    cz_set MAGENT_SWEEP_WAIT 2; cz_set MAGENT_SWEEP_POLL 1
    cc_logstub sleep 'pwd >> "$d/cwd.sleep"; n="$(grep -c "^sleep" "$d/calls")"
if [ "$n" = 1 ]; then printf "MemAvailable: 20000000 kB\n" > "$d/meminfo"; fi'
    cc_logstub systemd-run 'pwd >> "$d/cwd.systemd-run"
[ -e "$d/sd.fail" ] && [ "${!#}" = true ] && exit 1
while [ $# -gt 0 ]; do case "$1" in -p) shift 2 ;; --*) shift ;; *) break ;; esac; done
exec "$@"'
    cc_sweep_commit
    cc_sweep
    cz_expect_rc 0
    # The script's `cd` leaves every later command in the checkout: the wait's sleep, the scope probe and each run.
    cz_check "sleep working directories" "$(sort -u "$CZ_CASE/cwd.sleep")" "$CZ_TREE"
    cz_check "systemd-run working directories" "$(sort -u "$CZ_CASE/cwd.systemd-run")" "$CZ_TREE"
    cz_check "systemd-run calls" "$(cc_count_lines "$CZ_CASE/cwd.systemd-run")" 2
}
cc_case "the memory wait's sleep, the scope probe and every linter run start in the checkout, whatever the caller's directory" case_every_tool_runs_in_the_checkout

case_through_symlink() {
    sw_base
    cc_sweep_file a.sh "$SHEBANG"; cc_sweep_commit
    mkdir -p "$CZ_CASE/link"
    ln -s "$CZ_TREE/$SELF" "$CZ_CASE/link/sweep.sh"
    cc_run "$CZ_CASE/link/sweep.sh"
    cz_expect_rc 0
    cz_expect_stdout "checking 2 scripts"
    cz_check "linter working directory" "$(sort -u "$CZ_CASE/sc.pwd")" "$CZ_TREE"
}
cc_case "started through a symlink it sweeps the checkout that holds the real script" case_through_symlink

case_writes_nothing() {
    sw_base
    cc_sweep_file a.sh "$SHEBANG"; cc_sweep_commit
    ( cd "$CZ_TREE" && find . -path ./.git -prune -o -printf '%p %y %m %s\n' | LC_ALL=C sort ) > "$CZ_CASE/before"
    cc_sweep
    ( cd "$CZ_TREE" && find . -path ./.git -prune -o -printf '%p %y %m %s\n' | LC_ALL=C sort ) > "$CZ_CASE/after"
    cz_expect_rc 0
    cc_expect_unchanged "the tree" "$CZ_CASE/before" "$CZ_CASE/after"
    cz_check "files in TMPDIR" "$(cc_ls "$CZ_TMP" | wc -l | tr -d ' ')" 0
    cz_check "files in cwd" "$(cc_ls "$CZ_CWD" | wc -l | tr -d ' ')" 0
}
cc_case "the sweep writes no file in the tree, in TMPDIR or in the working directory" case_writes_nothing

cz_finish
