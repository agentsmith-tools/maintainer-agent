#!/usr/bin/env bash
# Characterization: tests/containment-probe.sh. The probe runs eleven attacks inside a
# container with the gate's flags, then checks that bin/maintainer-merge still passes those
# flags. The runtime here is a stub that prints what a contained or an uncontained run would
# print for each attack, so every verdict and every refusal is reached without a container.
#
# The cases pin: the exact flags and the exact payload of every attack, the order of the
# calls, the environment the runtime sees, each verdict in both directions, the messages
# and the exit codes, and what the probe does when the runtime says nothing at all.
# The two refusals that stop before any container (no runtime, no image) are also pinned in
# install-evals.sh; they are repeated here with the order of calls.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib-checks.sh"
cc_init

IMG=docker.io/library/bash:5
HEAD1="== what an adversarial pull request would try, inside the gate =="
HEAD2="== the probe still describes the real gate =="
GATE_FLAGS=(--network=none --cap-drop=ALL no-new-privileges --read-only noexec --pids-limit --memory --cpus)

stopped() { printf '  \033[32mstopped\033[0m  %s' "$1"; }
through() { printf '  \033[31mGOT THROUGH\033[0m  %s' "$1"; }

# The eleven attack lines when everything is contained, in order.
ATTACKS_OK=(
    "$(stopped 'outbound TCP (--network=none)')"
    "$(stopped 'DNS resolution')"
    "$(stopped 'writing to the image (--read-only)')"
    "$(stopped 'executing from /tmp (noexec)')"
    "$(stopped 'capabilities (CapEff is empty)')"
    "$(stopped 'privilege escalation (NoNewPrivs=1)')"
    "$(stopped 'fork bomb (stopped at 500 processes, cap is 512)')"
    "$(stopped 'memory balloon')"
    "$(stopped 'infinite loop (--timeout, and no container left behind)')"
    "$(stopped 'host credentials (podman passes no host environment)')"
    "$(stopped 'writing outside the bind mount')"
)
flag_ok() { stopped "bin/maintainer-merge still passes $1"; }
FLAGS_OK=(); for f in "${GATE_FLAGS[@]}"; do FLAGS_OK+=("$(flag_ok "$f")"); done

# pr_expect KEY LINE COUNTS: stdout is the all-contained output with the attack KEY's line replaced
# by LINE; the last line reads COUNTS. KEY is the position (1 to 11) of the attack.
pr_expect_with() {  # $1 = attack number, $2 = replacement line, $3 = the totals line
    local i lines=("$HEAD1")
    for i in "${!ATTACKS_OK[@]}"; do
        if [ "$((i + 1))" = "$1" ]; then lines+=("$2"); else lines+=("${ATTACKS_OK[$i]}"); fi
    done
    lines+=("" "$HEAD2" "${FLAGS_OK[@]}" "" "$3")
    cz_expect_stdout "${lines[@]}"
}
cz_expect_calls_has() { grep -qxF -- "$1" "$CZ_CASE/calls" || cz_note_fail "the order log has no line '$1'"; }
pr_start() { cc_probe_tree; cc_probe_runtime; cc_logstub sleep 'true'; }
pr_norm_runs() { sed "s|$CZ_TMP/tmp\.[A-Za-z0-9]\{10\}|@WORK@|" "$CZ_CASE/rt.runs"; }

FLAGS="--rm --timeout 15 --network=none --cap-drop=ALL --security-opt=no-new-privileges --read-only --tmpfs /tmp:rw,noexec,nosuid,size=256m --pids-limit 512 --memory 2g --cpus 2 -v @WORK@:/repo:z -w /repo $IMG sh -c"

case_all_contained() {
    pr_start
    cc_probe
    cz_expect_rc 0
    cz_expect_stderr_empty
    pr_expect_with 0 "" "  19 stopped, 0 got through"
    cz_check "temporary files left" "$(cc_ls "$CZ_TMP" | wc -l | tr -d ' ')" 0
}
cc_case "a runtime that contains every attack gives eleven 'stopped' lines, eight gate-flag lines, '19 stopped, 0 got through' and exit 0" case_all_contained

case_call_order_exact() {
    pr_start
    cc_probe
    cz_expect_rc 0
    # podman and the sleep stub both log to the order log: the verb of each podman call and the one sleep.
    local verbs
    verbs="$(awk '{ if ($1 == "sleep") print "sleep"; else print $2 }' "$CZ_CASE/calls" | tr '\n' ' ')"
    cz_check "verbs and the sleep in order" "$verbs" "image run run run run run run run run ps run sleep ps run run "
}
cc_case "the order is: image inspect, eight attacks, ps, the infinite loop, sleep 3, ps, the credential and escape attacks" case_call_order_exact

case_sleep_is_three_seconds() {
    pr_start
    cc_probe
    cz_check "sleep calls" "$(grep -c '^sleep ' "$CZ_CASE/calls")" 1
    cz_expect_calls_has "sleep 3"
}
cc_case "the probe sleeps 3 seconds once, between the loop attack and the second container count" case_sleep_is_three_seconds

case_inspect_and_ps_argv() {
    pr_start
    cc_probe
    cz_expect_calls_has "podman image inspect $IMG"
    cz_check "ps calls" "$(grep -cxF 'podman ps -q' "$CZ_CASE/calls")" 2
    cz_check "pull calls" "$(grep -c '^podman pull' "$CZ_CASE/calls")" 0
}
cc_case "the image is inspected once, containers are counted with 'ps -q' twice, and nothing is pulled when the image is there" case_inspect_and_ps_argv

case_run_flags() {
    pr_start
    cc_probe
    cz_expect_rc 0
    pr_norm_runs > "$CZ_CASE/runs.norm"
    cz_check "run calls" "$(cc_count_lines "$CZ_CASE/runs.norm")" 11
    cz_check "distinct flag lines" "$(sort -u "$CZ_CASE/runs.norm" | wc -l | tr -d ' ')" 1
    cz_check "the flags of every run" "$(sort -u "$CZ_CASE/runs.norm")" "$FLAGS"
}
cc_case "every attack runs with the gate's flags, in this order: timeout 15, no network, no capabilities, no new privileges, read-only, a noexec tmpfs, pid, memory and cpu limits, the work dir mounted at /repo" case_run_flags

case_payloads() {
    pr_start
    cc_probe
    cz_expect_rc 0
    # Written out one per line, newlines in a payload as \n.
    cc_expect_file_bytes "$CZ_CASE/rt.payloads" "$(printf '%s\n' \
        'timeout 5 bash -c "echo > /dev/tcp/1.1.1.1/53" 2>&1; echo rc=$?' \
        'getent hosts github.com >/dev/null 2>&1; echo rc=$?' \
        'echo pwned > /etc/passwd 2>&1; echo rc=$?' \
        'printf "#!/bin/sh\necho ran\n" > /tmp/x && chmod +x /tmp/x && /tmp/x 2>&1; echo rc=$?' \
        'grep -c . /proc/self/status >/dev/null; capsh --print 2>/dev/null | grep -i "current:" || grep CapEff /proc/self/status' \
        'grep NoNewPrivs /proc/self/status' \
        'n=0; while [ $n -lt 4000 ]; do sleep 30 & n=$((n+1)); done 2>/dev/null; echo spawned=$n' \
        'head -c 4G /dev/zero > /tmp/balloon 2>&1; echo rc=$?' \
        'while :; do :; done' \
        'env; cat /proc/1/environ 2>/dev/null | tr "\0" "\n"' \
        'ls /repo; echo "escaped" > /repo/../escape 2>&1; echo rc=$?')
"
}
cc_case "the eleven payloads, byte for byte, in order" case_payloads

case_mount_holds_the_probe_file() {
    pr_start
    cc_probe
    cz_check "distinct mount listings" "$(sort -u "$CZ_CASE/rt.mounts")" "mount holds: repofile  | host file"
    cz_check "mount listings" "$(cc_count_lines "$CZ_CASE/rt.mounts")" 11
}
cc_case "the mounted work directory holds one file, repofile, containing 'host file'" case_mount_holds_the_probe_file

case_environment_seen_by_runtime() {
    pr_start
    cc_probe
    cz_expect_rc 0
    cz_check "env of the creds attack (the tenth run)" "$(sed -n 10p "$CZ_CASE/rt.env")" "env: GH_TOKEN=ghp_PROBECANARY0000 ANTHROPIC_API_KEY=sk-PROBECANARY"
    cz_check "runs that see no canary" "$(grep -c 'GH_TOKEN=<unset> ANTHROPIC_API_KEY=<unset>' "$CZ_CASE/rt.env")" 10
}
cc_case "only the credential attack hands the runtime the two canary variables" case_environment_seen_by_runtime

case_ambient_environment_passes_through() {
    pr_start
    cz_set GH_TOKEN ghp_AMBIENT; cz_set ANTHROPIC_API_KEY sk-AMBIENT
    cc_probe
    cz_expect_rc 0
    # Pins current behaviour: the runtime inherits the caller's environment on every run but the
    # tenth, where the canaries replace it. Only the container's isolation keeps it out.
    cz_check "env of the first run" "$(sed -n 1p "$CZ_CASE/rt.env")" "env: GH_TOKEN=ghp_AMBIENT ANTHROPIC_API_KEY=sk-AMBIENT"
    cz_check "env of the creds attack" "$(sed -n 10p "$CZ_CASE/rt.env")" "env: GH_TOKEN=ghp_PROBECANARY0000 ANTHROPIC_API_KEY=sk-PROBECANARY"
    cz_check "env of the last run" "$(sed -n 11p "$CZ_CASE/rt.env")" "env: GH_TOKEN=ghp_AMBIENT ANTHROPIC_API_KEY=sk-AMBIENT"
}
cc_case "the caller's own variables reach the runtime on every run except the credential attack (pins current behaviour)" case_ambient_environment_passes_through

# --- each attack that gets through ---------------------------------------------------------

attack_through() {  # $1 = attack number, $2 = key, $3 = runtime output, $4 = the GOT THROUGH text
    pr_start
    cc_rt "$2" "$3"
    cc_probe
    cz_expect_rc 1
    cz_expect_stderr_empty
    pr_expect_with "$1" "$(through "$4")" "  18 stopped, 1 got through"
}
case_tcp()     { attack_through 1 tcp $'rc=0\n' "outbound TCP to 1.1.1.1:53"; }
case_dns()     { attack_through 2 dns $'rc=0\n' "DNS resolution"; }
case_etc()     { attack_through 3 etc $'rc=0\n' "writing to /etc inside the image"; }
case_exec()    { attack_through 4 exec $'ran\nrc=0\n' "executing from /tmp"; }
case_balloon() { attack_through 8 balloon $'rc=0\n' "allocating 4G in a 256m tmpfs"; }
case_creds()   { attack_through 10 creds $'PATH=/usr/bin\nGH_TOKEN=ghp_PROBECANARY0000\n' "a host credential reached the container"; }
cc_case "outbound TCP that connects (rc=0) is reported as got through, exit 1" case_tcp
cc_case "DNS that resolves (rc=0) is reported as got through" case_dns
cc_case "a write to /etc inside the image (rc=0) is reported as got through" case_etc
cc_case "a script that runs from /tmp (a line starting 'ran') is reported as got through" case_exec
cc_case "a balloon write that succeeds (rc=0) is reported as got through" case_balloon
cc_case "a canary in the container's environment is reported as got through" case_creds

case_exec_ran_not_at_line_start() {
    pr_start
    cc_rt exec $'sh: /tmp/x: Permission denied (it never ran)\nrc=126\n'
    cc_probe
    cz_expect_rc 0
    cz_expect_stdout_has "$(stopped 'executing from /tmp (noexec)')"
}
cc_case "the word 'ran' elsewhere than at the start of a line does not count as a run" case_exec_ran_not_at_line_start

case_rc_zero_anywhere() {
    pr_start
    cc_rt tcp $'connected? no. exit status rc=0 was only a label\n'
    cc_probe
    cz_expect_rc 1
    cz_expect_stdout_has "$(through 'outbound TCP to 1.1.1.1:53')"
}
cc_case "'rc=0' anywhere in the output counts as a success of the attack (pins current behaviour)" case_rc_zero_anywhere

case_rc_ten_is_not_zero() {
    pr_start
    cc_rt tcp $'rc=10\n'
    cc_probe
    cz_expect_rc 0
}
cc_case "'rc=10' does not hold the text 'rc=0', so it is not a success of the attack" case_rc_ten_is_not_zero

case_stderr_counts() {
    pr_start
    cc_rt dns ""
    printf 'rc=0\n' > "$CZ_CASE/rt/dns.err"
    cc_probe
    cz_expect_rc 1
    cz_expect_stdout_has "$(through 'DNS resolution')"
}
cc_case "the runtime's stderr is part of what is read (2>&1)" case_stderr_counts

case_runtime_exit_code_ignored() {
    pr_start
    printf 125 > "$CZ_CASE/rt/tcp.rc"
    printf 0 > "$CZ_CASE/rt/dns.rc"
    cc_rt dns $'rc=2\n'
    cc_probe
    cz_expect_rc 0
    pr_expect_with 0 "" "  19 stopped, 0 got through"
}
cc_case "the runtime's own exit status is not read: only its output decides" case_runtime_exit_code_ignored

# --- capabilities and privileges: the two attacks that must SHOW something ----------------------------------

case_caps_nonzero() {
    pr_start
    cc_rt caps $'CapEff:\t00000000a80425fb\n'
    cc_probe
    cz_expect_rc 1
    pr_expect_with 5 "$(through 'capabilities: CapEff:	00000000a80425fb')" "  18 stopped, 1 got through"
}
cc_case "a non-zero CapEff is reported with the first 60 characters of the output, newlines removed" case_caps_nonzero

case_caps_message_cut() {
    pr_start
    cc_rt caps "$(printf 'Current: cap_chown,cap_dac_override,cap_fowner,cap_fsetid,cap_kill,cap_setgid,cap_setuid\nextra line')"
    cc_probe
    cz_expect_rc 1
    cz_expect_stdout_has "$(through 'capabilities: Current: cap_chown,cap_dac_override,cap_fowner,cap_fsetid,ca')"
}
cc_case "the capabilities message is cut at 60 characters after the newlines are removed" case_caps_message_cut

case_caps_trailing_space() {
    pr_start
    cc_rt caps $'CapEff:\t0000000000000000 \n'
    cc_probe
    cz_expect_rc 1
    cz_expect_stdout_has "$(through 'capabilities: CapEff:	0000000000000000 ')"
}
cc_case "CapEff must be all zeros up to the end of the line: a trailing space fails it (pins current behaviour)" case_caps_trailing_space

case_caps_marker_with_no_digits() {
    pr_start
    cc_rt caps $'CapEff:\n'
    cc_probe
    cz_expect_rc 1
    cz_expect_stdout_has "$(through 'capabilities: CapEff:')"
}
cc_case "a CapEff line with no digits at all is not an empty capability set" case_caps_marker_with_no_digits

case_nul_in_an_attack_output() {
    pr_start
    mkdir -p "$CZ_CASE/rt"; printf 'r\0c=0\n' > "$CZ_CASE/rt/dns.out"
    cc_probe
    cz_expect_rc 1
    # The shell drops the NUL from the command substitution (and warns, naming line 50), so 'rc=0' joins up.
    cz_expect_stderr "$CZ_TREE/tests/containment-probe.sh: line 50: warning: command substitution: ignored null byte in input"
    cz_expect_stdout_has "$(through 'DNS resolution')"
}
cc_case "a NUL byte in an attack's output is dropped with the shell's warning and the text joins up (pins current behaviour)" case_nul_in_an_attack_output

case_caps_no_output() {
    pr_start
    : > "$CZ_CASE/rt/caps.out"
    cc_probe
    cz_expect_rc 1
    cz_expect_stdout_has "$(through 'capabilities: ')"
}
cc_case "no output at all from the capabilities check is a failure, not a pass" case_caps_no_output

case_caps_spaces_ok() {
    pr_start
    cc_rt caps $'CapEff:   0000000000000000\n'
    cc_probe
    cz_expect_rc 0
}
cc_case "CapEff may be separated from its zeros by spaces" case_caps_spaces_ok

case_nnp_zero() {
    pr_start
    cc_rt nnp $'NoNewPrivs:\t0\n'
    cc_probe
    cz_expect_rc 1
    pr_expect_with 6 "$(through $'NoNewPrivs is not set: NoNewPrivs:\t0')" "  18 stopped, 1 got through"
}
cc_case "NoNewPrivs 0 is reported with the whole output" case_nnp_zero

case_nnp_empty() {
    pr_start
    : > "$CZ_CASE/rt/nnp.out"
    cc_probe
    cz_expect_rc 1
    cz_expect_stdout_has "$(through 'NoNewPrivs is not set: ')"
}
cc_case "no output from the NoNewPrivs check is a failure" case_nnp_empty

case_nnp_any_one() {
    pr_start
    cc_rt nnp $'NoNewPrivs:\t0 (see 1)\n'
    cc_probe
    cz_expect_rc 0
}
cc_case "any '1' after NoNewPrivs: on the line counts as set (pins current behaviour)" case_nnp_any_one

# --- the fork bomb ------------------------------------------------------------------------------------------

fork_case() {  # $1 = runtime output, $2 = rc, $3 = expected line text, $4 = totals
    pr_start
    cc_rt fork "$1"
    cc_probe
    cz_expect_rc "$2"
    pr_expect_with 7 "$3" "$4"
}
case_fork_999()  { fork_case $'spawned=999\n' 0 "$(stopped 'fork bomb (stopped at 999 processes, cap is 512)')" "  19 stopped, 0 got through"; }
case_fork_1000() { fork_case $'spawned=1000\n' 1 "$(through 'fork bomb spawned 1000 processes; the pid cap is not in force')" "  18 stopped, 1 got through"; }
case_fork_4000() { fork_case $'spawned=4000\n' 1 "$(through 'fork bomb spawned 4000 processes; the pid cap is not in force')" "  18 stopped, 1 got through"; }
case_fork_dead() { fork_case "" 0 "$(stopped 'fork bomb (the container died before it could report)')" "  19 stopped, 0 got through"; }
case_fork_last_wins() { fork_case $'spawned=3000\nspawned=12\n' 0 "$(stopped 'fork bomb (stopped at 12 processes, cap is 512)')" "  19 stopped, 0 got through"; }
case_fork_noise() { fork_case $'sh: fork: retry: Resource temporarily unavailable\nspawned=511\n' 0 "$(stopped 'fork bomb (stopped at 511 processes, cap is 512)')" "  19 stopped, 0 got through"; }
case_fork_empty_number() { fork_case $'spawned=\n' 0 "$(stopped 'fork bomb (the container died before it could report)')" "  19 stopped, 0 got through"; }
cc_case "a fork bomb that reached 999 processes is stopped" case_fork_999
cc_case "a fork bomb that reached 1000 processes got through: the boundary is 1000" case_fork_1000
cc_case "a fork bomb that reached 4000 processes got through" case_fork_4000
cc_case "a container that died before reporting counts as stopped" case_fork_dead
cc_case "the last spawned= line is the one read" case_fork_last_wins
cc_case "lines before the spawned= line are ignored" case_fork_noise
cc_case "'spawned=' with no digits reads as no report" case_fork_empty_number

# --- the leftover container ------------------------------------------------------------------------------------

ps_case() {  # $1 = before, $2 = after, $3 = rc, $4 = expected line, $5 = totals
    pr_start
    printf %s "$1" > "$CZ_CASE/rt.ps.before"; printf %s "$2" > "$CZ_CASE/rt.ps.after"
    cc_probe
    cz_expect_rc "$3"
    pr_expect_with 9 "$4" "$5"
}
case_ps_same() { ps_case 1 1 0 "${ATTACKS_OK[8]}" "  19 stopped, 0 got through"; }
case_ps_fewer() { ps_case 3 1 0 "${ATTACKS_OK[8]}" "  19 stopped, 0 got through"; }
case_ps_more() { ps_case 0 2 1 "$(through 'an infinite loop left 2 container(s) running after the gate gave up')" "  18 stopped, 1 got through"; }
case_ps_one_more() { ps_case 4 5 1 "$(through 'an infinite loop left 1 container(s) running after the gate gave up')" "  18 stopped, 1 got through"; }
cc_case "the same number of containers before and after the infinite loop is stopped" case_ps_same
cc_case "fewer containers after the loop than before is stopped" case_ps_fewer
cc_case "two containers left behind by the infinite loop got through, and the message counts them" case_ps_more
cc_case "one container more than before is one left behind" case_ps_one_more

case_loop_output_discarded() {
    pr_start
    cc_rt loop $'rc=0\nspawned=5000\n'
    cc_probe
    cz_expect_rc 0
}
cc_case "the infinite loop's own output is discarded" case_loop_output_discarded

case_escape() {
    pr_start
    : > "$CZ_CASE/rt.escape"
    cc_probe
    cz_expect_rc 1
    pr_expect_with 11 "$(through 'writing outside the mount')" "  18 stopped, 1 got through"
}
cc_case "a file that appears beside the work directory (next to TMPDIR's entry) is reported as a write outside the mount" case_escape

case_escape_file_left_in_place() {
    pr_start
    : > "$CZ_CASE/rt.escape"
    cc_probe
    cz_check "TMPDIR holds" "$(cc_ls "$CZ_TMP")" "escape"
}
cc_case "the probe removes its work directory and leaves a planted escape file alone" case_escape_file_left_in_place

# --- a runtime that says nothing ----------------------------------------------------------------------------------

case_silent_runtime() {
    pr_start
    : > "$CZ_CASE/rt.silent"
    cc_probe
    cz_expect_rc 1
    cz_expect_stderr_empty
    # Pins current behaviour: a runtime that fails every run prints nothing, and every attack
    # that is judged by the ABSENCE of a success reads as stopped. Only the two that need to see
    # something (capabilities and NoNewPrivs) fail.
    local lines=("$HEAD1" "${ATTACKS_OK[@]}") i
    lines[5]="$(through 'capabilities: ')"
    lines[6]="$(through 'NoNewPrivs is not set: ')"
    lines[7]="$(stopped 'fork bomb (the container died before it could report)')"
    lines+=("" "$HEAD2" "${FLAGS_OK[@]}" "" "  17 stopped, 2 got through")
    cz_expect_stdout "${lines[@]}"
}
cc_case "a runtime that prints nothing makes nine attacks read as stopped and fails only the two checks that need output (pins current behaviour)" case_silent_runtime

# --- starting up --------------------------------------------------------------------------------------------------

case_image_pulled() {
    pr_start
    printf 1 > "$CZ_CASE/rt.inspect.rc"
    cc_probe
    cz_expect_rc 0
    cz_expect_calls_has "podman image inspect $IMG"
    cz_expect_calls_has "podman pull -q $IMG"
    cz_check "the pull comes right after the inspect" "$(sed -n '1,2p' "$CZ_CASE/calls" | tr '\n' '|')" "podman image inspect $IMG|podman pull -q $IMG|"
    cz_check "run calls" "$(cc_count_lines "$CZ_CASE/rt.runs")" 11
}
cc_case "an image that is not there is pulled quietly and the probe goes on" case_image_pulled

case_image_cannot_be_had() {
    pr_start
    printf 1 > "$CZ_CASE/rt.inspect.rc"; printf 1 > "$CZ_CASE/rt.pull.rc"
    cc_probe
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr "cannot obtain $IMG"
    cc_expect_calls "podman image inspect $IMG" "podman pull -q $IMG"
    cz_check "temporary files" "$(cc_ls "$CZ_TMP" | wc -l | tr -d ' ')" 0
}
cc_case "an image that cannot be had stops the probe with exit 1 after one inspect and one pull, with nothing on stdout" case_image_cannot_be_had

case_no_runtime() {
    cc_probe_tree
    cc_drop_tool podman; cc_drop_tool docker
    cc_probe
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr "no container runtime; this probe measured nothing"
    cc_expect_calls
}
cc_case "with no runtime the probe exits 1 saying it measured nothing, and runs nothing" case_no_runtime

case_docker_when_no_podman() {
    cc_probe_tree
    cc_drop_tool podman
    cc_probe_runtime docker
    cc_logstub sleep 'true'
    cc_probe
    cz_expect_rc 0
    pr_expect_with 0 "" "  19 stopped, 0 got through"
    cz_check "docker calls" "$(grep -c '^docker ' "$CZ_CASE/calls")" 14
}
cc_case "docker runs the whole probe when podman is absent" case_docker_when_no_podman

case_podman_preferred() {
    pr_start
    cc_probe_runtime docker
    cc_probe
    cz_expect_rc 0
    cz_check "docker calls" "$(grep -c '^docker ' "$CZ_CASE/calls")" 0
    cz_check "podman calls" "$(grep -c '^podman ' "$CZ_CASE/calls")" 14
}
cc_case "podman is used when both exist" case_podman_preferred

case_image_override() {
    pr_start
    cz_set PROBE_IMAGE example.invalid/x:1
    cc_probe
    cz_expect_rc 0
    cz_expect_calls_has "podman image inspect example.invalid/x:1"
    cz_check "flag lines" "$(pr_norm_runs | sort -u)" "$(printf '%s' "$FLAGS" | sed "s|$IMG|example.invalid/x:1|")"
}
cc_case "PROBE_IMAGE names the image in the inspect and in every run" case_image_override

case_timeout_override() {
    pr_start
    cz_set PROBE_TIMEOUT 7
    cc_probe
    cz_expect_rc 0
    cz_check "flag lines" "$(pr_norm_runs | sort -u)" "$(printf '%s' "$FLAGS" | sed 's|--timeout 15|--timeout 7|')"
}
cc_case "PROBE_TIMEOUT sets the runtime's --timeout on every run" case_timeout_override

# --- the probe still describes the real gate --------------------------------------------------------------------------

gate_flag_case() {  # GF = the flag to remove from the gate
    pr_start
    grep -v -- "$GF" "$CZ_TREE/bin/maintainer-merge" > "$CZ_CASE/gate.new"
    mv "$CZ_CASE/gate.new" "$CZ_TREE/bin/maintainer-merge"
    cc_probe
    cz_expect_rc 1
    cz_expect_stderr_empty
    local f lines=("$HEAD1" "${ATTACKS_OK[@]}" "" "$HEAD2")
    for f in "${GATE_FLAGS[@]}"; do
        if [ "$f" = "$GF" ]; then lines+=("$(through "the gate no longer passes $f; this probe is measuring something else")")
        else lines+=("$(flag_ok "$f")"); fi
    done
    lines+=("" "  18 stopped, 1 got through")
    cz_expect_stdout "${lines[@]}"
}
for GF in "${GATE_FLAGS[@]}"; do
    cc_case "a gate that no longer passes $GF is reported as got through, naming the flag" gate_flag_case
done

case_gate_flag_in_comment() {
    pr_start
    printf '# --network=none --cap-drop=ALL no-new-privileges --read-only noexec --pids-limit --memory --cpus\n' > "$CZ_TREE/bin/maintainer-merge"
    cc_probe
    cz_expect_rc 0
    pr_expect_with 0 "" "  19 stopped, 0 got through"
}
cc_case "a flag named only in a comment of the gate satisfies the check (pins current behaviour)" case_gate_flag_in_comment

case_gate_missing() {
    pr_start
    rm "$CZ_TREE/bin/maintainer-merge"
    cc_probe
    cz_expect_rc 1
    local i lines=("$HEAD1" "${ATTACKS_OK[@]}" "" "$HEAD2")
    for f in "${GATE_FLAGS[@]}"; do lines+=("$(through "the gate no longer passes $f; this probe is measuring something else")"); done
    lines+=("" "  11 stopped, 8 got through")
    cz_expect_stdout "${lines[@]}"
    local g="$CZ_TREE/tests/../bin/maintainer-merge" want=()
    for f in "${GATE_FLAGS[@]}"; do want+=("grep: $g: No such file or directory"); done
    cz_expect_stderr "${want[@]}"
}
cc_case "with no bin/maintainer-merge every flag is reported missing, and grep's own error is on stderr once per flag" case_gate_missing

case_gate_path_is_relative_to_the_script() {
    pr_start
    mkdir -p "$CZ_CASE/elsewhere/tests" "$CZ_CASE/elsewhere/bin"
    cp "$CZ_TREE/tests/containment-probe.sh" "$CZ_CASE/elsewhere/tests/"
    printf 'nothing here\n' > "$CZ_CASE/elsewhere/bin/maintainer-merge"
    cc_run "$CZ_CASE/elsewhere/tests/containment-probe.sh"
    cz_expect_rc 1
    cz_expect_stdout_has "$(through 'the gate no longer passes --network=none; this probe is measuring something else')"
}
cc_case "the gate is the ../bin/maintainer-merge of the probe's own directory, wherever the probe is run from" case_gate_path_is_relative_to_the_script

case_probe_writes_nothing() {
    pr_start
    cc_snap "$CZ_TREE" > "$CZ_CASE/before"
    cc_probe
    cc_snap "$CZ_TREE" > "$CZ_CASE/after"
    cz_expect_rc 0
    cc_expect_unchanged "the tree" "$CZ_CASE/before" "$CZ_CASE/after"
    cz_check "files in cwd" "$(cc_ls "$CZ_CWD" | wc -l | tr -d ' ')" 0
}
cc_case "the probe writes nothing in the tree or in the working directory" case_probe_writes_nothing

cz_finish
