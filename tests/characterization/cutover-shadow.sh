#!/usr/bin/env bash
# Characterization: scripts/cutover.sh shadow. A read-only comparison of the deployed bash programs
# with the programs of a build directory, on this machine's profiles. The "Rust builds" here are
# stand-ins that run the matching bash program, so a clean shadow must pass everywhere, and a
# stand-in told to change one byte, one exit code or one call must make exactly that comparison
# differ. Nothing may reach the real gh, git remote, podman or notify-send: the shadow answers with
# its own shims, and a case fails when the recorder behind them was reached.
#
# Comparisons per shadow of two profiles with two tasks each (see N_* below): 8 prompts, 3 refusals
# of run, 4 doctor, 6 repo, 6 merge, 2 run-instance, 2 alert, 4 install, 2 install-cron, 6 checks.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib-cutover.sh"
cu_init

N_PROMPTS=8; N_RUN_REFUSALS=3; N_DOCTOR=4; N_REPO=6; N_MERGE=6; N_RI=2; N_ALERT=2; N_INSTALL=4; N_CRON=2; N_CHECKS=6
N_ALL=$((N_PROMPTS + N_RUN_REFUSALS + N_DOCTOR + N_REPO + N_MERGE + N_RI + N_ALERT + N_INSTALL + N_CRON + N_CHECKS))

ready() { cu_deploy alpha beta; cu_build; }
mode() { printf '%s' "$2" > "$CZ_CASE/standin/$1.mode"; }
hook() { printf '%s\n' "$2" > "$CU_HOME/.fx-hook-$1"; }
sealed() { cz_check "the number of $1" "$(cu_report_count "$1")" "$2"; }
KEEP() { cz_set CUTOVER_KEEP_SCRATCH 1; }
# A tool that stands for the real one on the machine: it logs its arguments to $CZ_CASE/NAME.log. The path is
# written into the script, because the shadow runs its programs under env -i and no variable would reach it.
fake_tool() {
    cat > "$CZ_CASE/bin/$1" <<FAKE
#!/usr/bin/env bash
printf '$1 %s\n' "\$*" >> "$CZ_CASE/$1.log"
exit 0
FAKE
    chmod +x "$CZ_CASE/bin/$1"
}
# Every file of one kind the kept scratch directory holds, for a case that must look inside it.
kept() { cat "$CU_BUILD"/.shadow.*/cmp-*/*/"$1" 2>/dev/null; }

case_clean() {
    ready
    cu_shadow
    cz_expect_rc 0
    cu_expect_marker
    cz_expect_stdout_has "SUMMARY comparisons=$N_ALL pass=$N_ALL diff=0 skip=0"
    cz_expect_stdout_has "MARKER written: $CU_BUILD/shadow-ok"
    sealed PASS "$N_ALL"; sealed DIFF 0; sealed SKIP 0
}
cu_case "shadow: a build whose programs behave as the bash ones passes every comparison and is marked" case_clean

case_report_header_and_counts() {
    ready
    cu_shadow
    cu_expect_report_has "SHADOW home=$CU_HOME checkout=$(cu_head | cut -c1-7) deployed=abc1234 build=$(cu_head | cut -c1-7) profiles=2"
    cu_expect_report_has "PROMPTS compared=$N_PROMPTS expected=$N_PROMPTS"
    cu_expect_report_has "SHIMS refused=0 unplanned=0 (both sides together)"
    cu_expect_report_has "DEPLOYED tree and checkout unchanged by the run: yes"
    cz_check "the report is also on standard output" "$(grep -c '^PASS ' "$CZ_CASE/stdout")" "$N_ALL"
}
cu_case "shadow: the report states the commits, the prompts compared and what the shims saw" case_report_header_and_counts

case_marker_is_the_table() {
    ready
    cu_shadow
    cz_check "shadow-ok" "$(cat "$CU_BUILD/shadow-ok")" "$(cu_table)"
    cz_check "mode of shadow-ok" "$(stat -c %a "$CU_BUILD/shadow-ok")" 644
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_stdout_has "shadow-ok: current"
}
cu_case "shadow: shadow-ok holds the sha256 table of the build it compared, and status calls it current" case_marker_is_the_table

case_each_prompt_both_styles() {
    ready
    cu_shadow
    local p t
    for p in alpha beta; do for t in review issues; do
        cu_expect_report_has "PASS run --show-prompt $p $t (default)"
        cu_expect_report_has "PASS run --show-prompt $p $t (raw)"
    done; done
}
cu_case "shadow: every profile and each of its tasks is compared with the default and the raw prose style" case_each_prompt_both_styles

case_style_and_date_reach_the_program() {
    ready; KEEP
    cu_shadow
    cz_check "raw comparisons that saw style=raw" "$(kept stdout | grep -c '^style=raw$')" "$((N_PROMPTS / 2 * 2))"
    cz_check "default comparisons that saw the default style" "$(kept stdout | grep -c '^style=stop-slop$')" "$((N_PROMPTS / 2 * 2))"
    cz_check "prompts with the fixed start time" "$(kept stdout | grep -c 'started 2026-01-01T00:00:00+00:00\.$')" "$((N_PROMPTS * 2))"
}
cu_case "shadow: the prose style reaches both sides and date -Is answers one fixed instant" case_style_and_date_reach_the_program

case_one_byte_in_one_prompt() {
    ready; mode run tweak
    cu_shadow
    cz_expect_rc 1
    cu_expect_no_marker
    cu_expect_report_has "DIFF run --show-prompt beta issues (default): stdout line 3: bash='issues prompt of beta' rust='issuez prompt of beta'"
    cu_expect_report_has "DIFF run --show-prompt beta issues (raw): stdout line 3: bash='issues prompt of beta' rust='issuez prompt of beta'"
    sealed DIFF 2; sealed PASS "$((N_ALL - 2))"
    cu_expect_report_has "MARKER not written: 2 comparison(s) differ"
}
cu_case "shadow: a build that differs in one byte of one prompt makes exactly those comparisons DIFF" case_one_byte_in_one_prompt

case_exit_code() {
    ready; mode maintainer-repo rc
    cu_shadow
    cz_expect_rc 1
    cu_expect_report_has "DIFF maintainer-repo release-check alpha: exit code: bash=0 rust=1"
    cu_expect_no_marker
}
cu_case "shadow: another exit code is a DIFF that names both codes" case_exit_code

case_stderr() {
    ready; mode maintainer-repo stderr
    cu_shadow
    cu_expect_report_has "DIFF maintainer-repo release-check beta: stderr line 1: bash='' rust='an extra line on stderr'"
}
cu_case "shadow: an extra line on standard error is a DIFF" case_stderr

case_stdout_longer() {
    ready; mode install append
    cu_shadow
    cu_expect_report_has "DIFF install --dry-run: stdout line 2: bash='' rust='X'"
}
cu_case "shadow: a byte appended to standard output is a DIFF at the next line" case_stdout_longer

case_control_bytes_are_not_shown() {
    ready; mode install esc
    cu_shadow
    cu_expect_report_has "DIFF install --dry-run: stdout line 2: bash='' rust='?[31mred?[0m'"
    if grep -q "$(printf '\033')" "$CU_BUILD/shadow-report.txt"; then cz_note_fail "a raw escape byte reached the report"; fi
}
cu_case "shadow: control bytes in a differing line are shown as ? and never reach the report raw" case_control_bytes_are_not_shown

case_stale_marker_removed() {
    ready
    cu_table > "$CU_BUILD/shadow-ok"; printf 'old\n' > "$CU_BUILD/shadow-report.txt"
    mode run rc
    cu_shadow
    cz_expect_rc 1
    cu_expect_no_marker
    cu_expect_report_lacks "old"
}
cu_case "shadow: the marker and the report of an earlier run are removed before the new run starts" case_stale_marker_removed

case_doctor_normalised() {
    ready; mode maintainer-doctor delay
    cu_shadow
    cz_expect_rc 0
    cu_expect_report_has "PASS maintainer-doctor --quick alpha"
    cu_expect_report_has "PASS maintainer-doctor --quick beta"
}
cu_case "shadow: doctor output that differs only in a timestamp and a pid passes" case_doctor_normalised

case_doctor_not_over_normalised() {
    ready; mode maintainer-doctor firstbyte
    cu_shadow
    cz_expect_rc 1
    cu_expect_report_has "DIFF maintainer-doctor --quick alpha: stdout line 1: bash='Deployment' rust='Zeployment'"
}
cu_case "shadow: the doctor's normaliser hides times and pids and nothing else" case_doctor_not_over_normalised

case_other_programs_not_normalised() {
    ready; mode maintainer-repo delay
    cu_shadow
    # the repo fixture prints no time; a delay changes nothing there. The doctor line proves the contrast.
    cz_expect_rc 0
    mode maintainer-doctor delay
    hook maintainer-repo 'echo "ts $(date +%Y-%m-%dT%H:%M:%S%z)"'
    cu_shadow
    cz_expect_rc 1
    cu_expect_report_has "DIFF maintainer-repo prune --dry-run alpha: stdout line 1: bash='ts "
}
cu_case "shadow: only the doctor's output is normalised: a timestamp from another program is a DIFF" case_other_programs_not_normalised

# --- skips --------------------------------------------------------------------------------------------------------

case_skip() {
    cu_deploy alpha beta; cu_build_without maintainer-merge
    cu_shadow
    cz_expect_rc 1
    cu_expect_no_marker
    cu_expect_report_has "SKIP maintainer-merge show (all receipts): no Rust build of maintainer-merge in $CU_BUILD (the bash program stays)"
    sealed SKIP "$N_MERGE"
    cu_expect_report_has "SUMMARY comparisons=$N_ALL pass=$((N_ALL - N_MERGE)) diff=0 skip=$N_MERGE"
    cu_expect_report_has "SKIPPED programs (no Rust build): maintainer-merge"
    cu_expect_report_has "MARKER not written: 1 program(s) had no Rust build to compare (name one with --accept-skip to go on without it)"
}
cu_case "shadow: a program with no Rust build is a SKIP per comparison, counted, listed, and withholds the marker" case_skip

case_accept_skip() {
    cu_deploy alpha beta; cu_build_without maintainer-merge
    cu_shadow --accept-skip maintainer-merge
    cz_expect_rc 0
    cz_check "shadow-ok" "$(cat "$CU_BUILD/shadow-ok")" "$(cu_table)
# accepted-skip: maintainer-merge"
    cu_expect_report_has "SKIP maintainer-merge show 7: no Rust build"
    cu_expect_report_has "MARKER written: $CU_BUILD/shadow-ok"
}
cu_case "shadow: --accept-skip NAME lets the run be marked, and the marker says which skip was accepted" case_accept_skip

case_accept_skip_other_name() {
    cu_deploy alpha beta; cu_build_without maintainer-merge
    cu_shadow --accept-skip alert
    cz_expect_rc 1
    cu_expect_no_marker
    cu_expect_report_has "MARKER not written: 1 program(s) had no Rust build to compare"
}
cu_case "shadow: accepting the skip of one program does not excuse another" case_accept_skip_other_name

case_accept_skip_unused() {
    ready
    cu_shadow --accept-skip maintainer-merge
    cz_expect_rc 0
    cz_check "shadow-ok" "$(cat "$CU_BUILD/shadow-ok")" "$(cu_table)"
}
cu_case "shadow: accepting a skip that did not happen adds nothing to the marker" case_accept_skip_unused

case_no_run_no_marker_even_accepted() {
    cu_deploy alpha beta; cu_build_without run
    cu_shadow --accept-skip run
    cz_expect_rc 1
    cu_expect_no_marker
    cu_expect_report_has "PROMPTS compared=0 expected=$N_PROMPTS"
    cu_expect_report_has "MARKER not written: no prompt was compared; 0 prompts were compared and $N_PROMPTS were expected"
}
cu_case "shadow: with no Rust run there is no prompt compared, and accepting the skip does not give a marker" case_no_run_no_marker_even_accepted

case_accept_skip_bad_name() {
    ready
    cu_shadow --accept-skip 'Run Now'
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: --accept-skip Run\\ Now is not a program name"
}
cu_case "shadow: --accept-skip with something that is not a program name is refused" case_accept_skip_bad_name

# --- nothing to compare -----------------------------------------------------------------------------------------------

case_no_profiles() {
    ready; rm -rf "$CU_SHARE/profiles"/*
    cu_shadow
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr_first "cutover.sh: no profile is deployed under $CU_SHARE/profiles; shadow compared nothing"
    cu_expect_no_marker
}
cu_case "shadow: an empty profiles directory is refused, not passed with zero comparisons" case_no_profiles

case_no_profiles_dir() {
    ready; rm -rf "$CU_SHARE/profiles"
    cu_shadow
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: no deployed profiles directory $CU_SHARE/profiles; shadow compared nothing"
}
cu_case "shadow: a machine with no profiles directory is refused" case_no_profiles_dir

case_profile_without_env_file() {
    ready; rm "$CU_SHARE/profiles/beta/profile.env"
    cu_shadow
    cz_expect_rc 0
    cu_expect_report_has "PROMPTS compared=4 expected=4"
}
cu_case "shadow: a profile directory with no profile.env is not a profile" case_profile_without_env_file

case_profile_without_tasks() {
    ready; printf 'STATE_DIR=x\n' > "$CU_SHARE/profiles/beta/profile.env"
    cu_shadow
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: the profile beta declares no TASKS; shadow would compare nothing for it"
}
cu_case "shadow: a profile that declares no tasks is refused by name" case_profile_without_tasks

case_profile_unreadable() {
    ready; printf 'exit 7\n' > "$CU_SHARE/profiles/beta/profile.env"
    cu_shadow
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: cannot read TASKS from the profile beta"
}
cu_case "shadow: a profile whose profile.env fails when read is refused by name" case_profile_unreadable

case_hostile_profile_names() {
    local name safe
    for name in 'a b' 'a&b' 'a|b' 'a\b' '-dash' 'semi;colon' 'dollar$x' 'quote"q'; do
        cu_sandbox_new; ready
        cp -a "$CU_SHARE/profiles/beta" "$CU_SHARE/profiles/$name"
        cu_shadow
        cz_check "exit for the profile name '$name'" "$CZ_RC" 1
        cz_expect_stderr_has "is not a plain name; refused before anything ran"
        cu_expect_no_marker
        [ "$(cu_report_count PASS)" = 0 ] || cz_note_fail "a comparison ran for the profile name '$name'"
    done
    safe=1; [ "$safe" = 1 ]
}
cu_case "shadow: a profile name with a space, &, |, \\, a leading dash, ; \$ or \" is refused before any comparison" case_hostile_profile_names

case_hostile_profile_name_newline() {
    ready
    cp -a "$CU_SHARE/profiles/beta" "$CU_SHARE/profiles/$(printf 'a\nb')"
    cu_shadow
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: the deployed profile name \$'a\\nb' is not a plain name; refused before anything ran"
}
cu_case "shadow: a profile name with a newline is refused and shown on one line" case_hostile_profile_name_newline

case_hostile_task_names() {
    local t
    for t in 'a&b' 'a|b' 'a\b' '-x' 're-view' 'a;b'; do
        cu_sandbox_new; ready
        printf 'TASKS="review %s"\n' "$t" > "$CU_SHARE/profiles/beta/profile.env"
        cu_shadow
        cz_check "exit for the task name '$t'" "$CZ_RC" 1
        cz_expect_stderr_has "of profile beta is not a plain name; refused before anything ran"
        cu_expect_no_marker
    done
}
cu_case "shadow: a task name with &, |, \\, a leading dash, a hyphen or ; is refused before any comparison" case_hostile_task_names

case_tasks_on_two_lines() {
    ready
    printf 'TASKS="review\nissues"\n' > "$CU_SHARE/profiles/beta/profile.env"
    cu_shadow
    cz_expect_rc 0
    cu_expect_report_has "PASS run --show-prompt beta issues (raw)"
}
cu_case "shadow: tasks separated by a newline are two tasks" case_tasks_on_two_lines

case_build_dir_checks() {
    ready; rm "$CU_BUILD/SHA256SUMS"
    cu_shadow
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: no SHA256SUMS in $CU_BUILD: run scripts/cutover.sh build first"
    ready2() { :; }
}
cu_case "shadow: a build directory without SHA256SUMS is refused" case_build_dir_checks

case_build_changed_since_build() {
    ready; printf 'tampered' >> "$CU_BUILD/run"
    cu_shadow
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: the programs in $CU_BUILD are not the ones build recorded in SHA256SUMS; build again"
    cu_expect_no_marker
}
cu_case "shadow: programs that changed since build are refused" case_build_changed_since_build

case_deployed_program_missing() {
    ready; rm "$CU_BIN/maintainer-doctor"
    cu_shadow
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: the deployed bash program $CU_BIN/maintainer-doctor is missing or not executable, so there is nothing to compare maintainer-doctor --quick alpha with"
    cu_expect_no_marker
}
cu_case "shadow: a deployed bash program that is missing is refused, not skipped" case_deployed_program_missing

case_bad_timeout() {
    ready
    cu_shadow --timeout 0
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: --timeout wants a whole number of seconds, not 0"
}
cu_case "shadow: a timeout that is not a positive whole number is refused" case_bad_timeout

case_timeout() {
    ready; mode check-claims sleep
    cu_shadow --timeout 2
    cz_expect_rc 1
    cu_expect_report_has "DIFF check-claims: the Rust side timed out after 2s"
    sealed DIFF 1
    cu_expect_no_marker
}
cu_case "shadow: a side that outlives the timeout is a DIFF and never a PASS" case_timeout

# --- read-only ------------------------------------------------------------------------------------------------------------

case_real_tools_never_reached() {
    ready
    mode maintainer-doctor gh-extra
    cu_shadow
    cz_expect_rc 1
    # the shims answered; the recorders behind them (gh, claude, podman) are checked by cu_case_end
    cu_expect_report_has "DIFF maintainer-doctor --quick alpha: calls to the shims"
}
cu_case "shadow: a call to gh goes to the shim, never to the gh on PATH" case_real_tools_never_reached

case_gh_write_refused() {
    ready; mode run gh-write
    cu_shadow
    cz_expect_rc 1
    cu_expect_no_marker
    cu_expect_report_has "DIFF run --show-prompt alpha review (default): calls to the shims"
    cu_expect_report_has "SHIMS refused=$((N_PROMPTS + N_RUN_REFUSALS)) unplanned=0"
    cu_expect_report_has "a program tried $((N_PROMPTS + N_RUN_REFUSALS)) call(s) the shims refuse as writes"
}
cu_case "shadow: a gh write is refused by the shim, counted, and withholds the marker" case_gh_write_refused

case_git_push_refused() {
    ready; mode maintainer-repo git-push
    cu_shadow
    cz_expect_rc 1
    cu_expect_report_has "DIFF maintainer-repo release-check alpha: calls to the shims"
    cu_expect_report_has "a program tried 6 call(s) the shims refuse as writes"
}
cu_case "shadow: a git push is refused by the shim and counted" case_git_push_refused

case_both_sides_write() {
    ready
    hook maintainer-repo 'gh pr comment 1 --body hello >/dev/null 2>&1'
    cu_shadow
    cz_expect_rc 1
    cu_expect_no_marker
    cu_expect_report_has "PASS maintainer-repo release-check alpha"
    cu_expect_report_has "SHIMS refused=12 unplanned=0"
    cu_expect_report_has "MARKER not written: a program tried 12 call(s) the shims refuse as writes"
}
cu_case "shadow: when the bash program writes too, the comparison passes and the marker is still withheld" case_both_sides_write

case_unplanned_gh_counted() {
    ready
    hook maintainer-repo 'gh issue list >/dev/null 2>&1'
    cu_shadow
    cz_expect_rc 0
    cu_expect_report_has "SHIMS refused=0 unplanned=12"
}
cu_case "shadow: a gh read nobody planned is answered with 99 and counted in the report" case_unplanned_gh_counted

case_fetch_swallowed() {
    ready; KEEP
    git -C "$CU_HOME" init -q 2>/dev/null; mkdir -p "$CZ_CASE/repo"; git -C "$CZ_CASE/repo" init -q -b main
    git -C "$CZ_CASE/repo" remote add origin "$CZ_CASE/does-not-exist"
    cz_set MAINTAINER_REPO "$CZ_CASE/repo"
    cu_shadow
    cz_expect_rc 0
    cz_check "comparisons whose fetch the shim swallowed" "$(kept stdout | grep -c '^  fetch rc=0$')" 4
    cz_check "comparisons whose fetch really ran and failed" "$(kept stdout | grep -c '^  fetch rc=128$')" 0
}
cu_case "shadow: git fetch is swallowed by the shim, so no ref is written and no network is reached" case_fetch_swallowed

case_no_gh_token_in_children() {
    ready; KEEP
    cz_set GH_TOKEN "ghp_SECRETSECRETSECRET"
    cz_set GITHUB_TOKEN "ghp_OTHERSECRET"
    hook maintainer-doctor 'echo "token: ${GH_TOKEN-unset} ${GITHUB_TOKEN-unset}"'
    cu_shadow
    cz_expect_rc 0
    cz_check "doctor comparisons that saw no token" "$(kept stdout | grep -c '^token: unset unset$')" 8
    if grep -rq "SECRET" "$CU_BUILD" "$CZ_CASE/stdout" "$CZ_CASE/stderr"; then cz_note_fail "a token reached a file or a stream"; fi
}
cu_case "shadow: a token in the caller's environment reaches neither side nor any file" case_no_gh_token_in_children

case_machine_unchanged() {
    ready
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"; cu_snap "$CU_FX" > "$CZ_CASE/fx.before"
    cu_shadow
    cz_expect_rc 0
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"; cu_snap "$CU_FX" > "$CZ_CASE/fx.after"
    cu_expect_same_snap "HOME" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
    cu_expect_same_snap "the checkout" "$CZ_CASE/fx.before" "$CZ_CASE/fx.after"
}
cu_case "shadow: HOME and the checkout are byte for byte as they were after a run" case_machine_unchanged

case_write_into_deployed_tree() {
    ready
    hook maintainer-doctor 'echo changed > "$HOME/.local/share/maintainer/stray"'
    cu_shadow
    cz_expect_rc 1
    cu_expect_no_marker
    cu_expect_report_has "DEPLOYED tree and checkout unchanged by the run: no"
    cu_expect_report_has "MARKER not written: the deployed tree or the checkout changed during the run"
}
cu_case "shadow: a program that writes into the deployed tree withholds the marker" case_write_into_deployed_tree

case_write_into_checkout() {
    ready
    hook check-claims 'echo changed > "$HOME/../fx/stray-file"'
    cu_shadow
    cz_expect_rc 1
    cu_expect_report_has "DEPLOYED tree and checkout unchanged by the run: no"
}
cu_case "shadow: a program that writes into the checkout withholds the marker" case_write_into_checkout

case_run_lock_not_taken() {
    ready
    local lock="$CU_HOME/.local/state/alpha-maint/run.lock"
    mkdir -p "$(dirname "$lock")"
    ( exec 9> "$lock"; flock -n 9 || exit 3; : > "$CZ_CASE/lock-ready"; exec sleep 25 ) &
    local holder=$!
    for _ in $(seq 1 50); do [ -e "$CZ_CASE/lock-ready" ] && break; sleep 0.1; done
    local start=$SECONDS
    cu_shadow
    local took=$((SECONDS - start))
    cz_expect_rc 0
    [ "$took" -lt 20 ] || cz_note_fail "the shadow waited for a held run lock ($took s)"
    kill -0 "$holder" 2>/dev/null || cz_note_fail "the process holding the run lock is gone"
    kill "$holder" 2>/dev/null; wait "$holder" 2>/dev/null
    cz_check "the lock file is still empty" "$(wc -c < "$lock")" 0
}
cu_case "shadow: a held run lock is neither waited for nor taken" case_run_lock_not_taken

case_scratch_removed() {
    ready
    hook maintainer-doctor 'echo x > "$TMPDIR/leftover"'
    cu_shadow
    cz_expect_rc 0
    cz_check "entries of the build directory beginning with a dot" "$(cu_dot_entries "$CU_BUILD")" 0
}
cu_case "shadow: its scratch directory, and the TMPDIR its programs used, are removed at the end" case_scratch_removed

case_scratch_kept_on_request() {
    ready; KEEP
    cu_shadow
    cz_check "kept scratch directories" "$(ls -d "$CU_BUILD"/.shadow.* 2>/dev/null | wc -l)" 1
}
cu_case "shadow: CUTOVER_KEEP_SCRATCH keeps the scratch directory for a person to read" case_scratch_kept_on_request

# --- the other groups ------------------------------------------------------------------------------------------------------

case_merge_group_uses_scratch_state() {
    ready; KEEP
    cu_shadow
    cz_expect_rc 0
    cu_expect_report_has "PASS maintainer-merge show 7"
    cu_expect_report_has "PASS maintainer-merge show 8 (no receipt)"
    cz_check "the receipt the fixture shows" "$(kept stdout | grep -c '"proof": "cutover shadow fixture"')" 2
    [ ! -e "$CU_HOME/.local/state/receipts" ] || cz_note_fail "state written under HOME"
}
cu_case "shadow: maintainer-merge show reads a scratch state directory with a scratch receipt" case_merge_group_uses_scratch_state

case_alert_in_scratch_home() {
    ready; KEEP
    mkdir -p "$CU_HOME/.local/state/alpha-maint"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_shadow
    cz_expect_rc 0
    cu_expect_report_has "PASS alert alpha-review"
    cu_expect_report_has "PASS alert alpha-review (notifications off)"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the real HOME (alert wrote into a scratch HOME)" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
    cz_check "alerts.log lines of the two sides" "$(kept calls.txt | grep -c 'ALERT alpha-review$')" 4
}
cu_case "shadow: alert runs against a scratch HOME and its log is compared" case_alert_in_scratch_home

case_install_and_cron_compared() {
    ready
    cu_shadow
    cu_expect_report_has "PASS install --dry-run"
    cu_expect_report_has "PASS install --dry-run --timers"
    cu_expect_report_has "PASS install --uninstall --dry-run"
    cu_expect_report_has "PASS install --bogus"
    cu_expect_report_has "PASS install-cron --dry-run alpha"
    cu_expect_report_has "PASS install-cron --dry-run beta"
}
cu_case "shadow: the install plans and the cron dry run are compared for every profile" case_install_and_cron_compared

case_checks_on_the_checkout() {
    ready
    cu_shadow
    local c
    for c in check-claims shellcheck-sweep verify-action-pins "run-evals alpha (static)" "run-evals beta (static)" containment-probe; do
        cu_expect_report_has "PASS $c"
    done
}
cu_case "shadow: the five repository checks run on the checkout, run-evals once per profile" case_checks_on_the_checkout

case_podman_probe_policy() {
    ready
    fake_tool podman
    hook containment-probe 'podman image inspect img >/dev/null; podman run --rm img true; podman pull img >/dev/null 2>&1; podman build . >/dev/null 2>&1'
    cu_shadow
    cz_expect_rc 1
    cz_check "calls the real podman saw" "$(cat "$CZ_CASE/podman.log" 2>/dev/null)" "podman image inspect img
podman run --rm img true
podman image inspect img
podman run --rm img true"
    cu_expect_report_has "a program tried 4 call(s) the shims refuse as writes"
}
cu_case "shadow: the probe may inspect an image and run a container that removes itself; pull and build are refused" case_podman_probe_policy

case_podman_elsewhere_refused() {
    ready
    fake_tool podman
    hook maintainer-doctor 'podman run --rm img true >/dev/null 2>&1'
    cu_shadow
    cz_check "calls the real podman saw" "$(cat "$CZ_CASE/podman.log" 2>/dev/null | wc -l)" 0
    cu_expect_report_has "SHIMS refused=0 unplanned=8"
}
cu_case "shadow: outside the probe podman is never run, not even with --rm" case_podman_elsewhere_refused

case_systemctl_reads_only() {
    ready
    fake_tool systemctl
    hook maintainer-doctor 'systemctl --user list-timers >/dev/null; systemctl --user enable --now x.timer >/dev/null 2>&1; systemctl --user daemon-reload >/dev/null 2>&1'
    cu_shadow
    cz_check "calls the real systemctl saw" "$(sort -u "$CZ_CASE/systemctl.log")" "systemctl --user list-timers"
    cu_expect_report_has "a program tried 16 call(s) the shims refuse as writes"
}
cu_case "shadow: systemctl may list timers and nothing else" case_systemctl_reads_only

case_systemd_run_waits() {
    ready
    fake_tool systemd-run
    hook maintainer-doctor 'systemd-run --user --wait --pipe --quiet true >/dev/null; systemd-run --user --scope --quiet true >/dev/null; systemd-run --user sleep 100 >/dev/null 2>&1; systemd-run --system --wait true >/dev/null 2>&1'
    cu_shadow
    cz_check "calls the real systemd-run saw" "$(sort -u "$CZ_CASE/systemd-run.log")" "systemd-run --user --scope --quiet true
systemd-run --user --wait --pipe --quiet true"
    # 4 doctor comparisons x 2 sides x 2 refused calls
    cu_expect_report_has "a program tried 16 call(s) the shims refuse as writes"
}
cu_case "shadow: systemd-run may run a transient unit it waits for or a scope, and nothing that is left running" case_systemd_run_waits

case_sigterm() {
    ready; mode check-claims sleep
    local pairs=() k
    for k in "${!CZ_E[@]}"; do pairs+=("$k=${CZ_E[$k]}"); done
    cd "$CU_HOME" || return
    env -i "${pairs[@]}" bash "$(cu_script)" shadow --out "$CU_BUILD" > "$CZ_CASE/stdout" 2> "$CZ_CASE/stderr" < /dev/null &
    local pid=$!
    for _ in $(seq 1 100); do [ -s "$CZ_CASE/standin/check-claims.pid" ] && break; sleep 0.2; done
    local sleeper; sleeper="$(cat "$CZ_CASE/standin/check-claims.pid" 2>/dev/null)"
    [ -n "$sleeper" ] && [ "$(readlink "/proc/$sleeper/exe")" = "$(readlink -f "$(type -P sleep)")" ] || cz_note_fail "the stand-in never reached its sleep"
    case "$(tr '\0' ' ' < "/proc/$pid/cmdline")" in
        *cutover.sh*shadow*) ;;
        *) cz_note_fail "the pid I am about to signal is not the shadow: $(tr '\0' ' ' < "/proc/$pid/cmdline" | cut -c1-80)" ;;
    esac
    kill -TERM "$pid"
    wait "$pid"; CZ_RC=$?
    cz_expect_rc 143
    cz_check "entries of the build directory beginning with a dot" "$(cu_dot_entries "$CU_BUILD")" 0
    cu_expect_no_marker
    # the side that was running is gone: the sleeping stand-in is a child of a timeout in the same group
    sleep 0.5
    if [ -n "$sleeper" ] && kill -0 "$sleeper" 2>/dev/null; then cz_note_fail "the sleeping stand-in ($sleeper) outlived the shadow"; kill "$sleeper" 2>/dev/null; fi
}
cu_case "shadow: SIGTERM ends the run with 143, stops the side that was running and removes the scratch directory" case_sigterm

case_crontab_list_only() {
    ready
    hook install-cron 'crontab -l 2>/dev/null; echo "listed rc=$?"; echo job | crontab - 2>/dev/null; echo "wrote rc=$?"'
    cu_shadow
    cu_expect_report_has "a program tried 4 call(s) the shims refuse as writes"
    cu_expect_report_has "PASS install-cron --dry-run alpha"
}
cu_case "shadow: crontab answers a list with no crontab and refuses a write" case_crontab_list_only

case_backends_never_run() {
    ready
    hook maintainer-doctor 'claude --version >/dev/null 2>&1; codex exec x >/dev/null 2>&1; curl https://example.invalid >/dev/null 2>&1'
    cu_shadow
    # 4 doctor comparisons x 2 sides x 3 calls
    cu_expect_report_has "SHIMS refused=0 unplanned=24"
}
cu_case "shadow: a model backend or the network is a call nobody planned: answered with 99, counted, never run" case_backends_never_run

cz_finish
