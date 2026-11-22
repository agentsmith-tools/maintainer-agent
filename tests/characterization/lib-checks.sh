#!/usr/bin/env bash
# Helpers for the characterization of the repository checks: scripts/check_claims.sh,
# scripts/verify-action-pins.sh, scripts/shellcheck-sweep.sh, evals/run-evals.sh and
# tests/containment-probe.sh. Sourced by checks-*.sh after lib.sh, never run alone.
#
# Isolation. Every case gets a fresh sandbox:
#   tree/   the files the program reads, generated for the case (a few lines of
#           workflow, a handful of shell scripts, a fixture profile). Only the
#           program under test is copied from MX_TREE, so the cases do not move
#           when a real workflow or document changes. check_claims_e2e below is
#           the one place that copies the real documents, on purpose.
#   cwd/    the working directory of the program, outside the tree.
#   tmp/    TMPDIR, so a leaked temporary file shows up as a file here.
#   bin/    stubs first on PATH. gh, podman, docker, curl, wget and ssh fail the
#           case when they run, unless the case installs its own stub for them.
#   tools/  symlinks to the few real tools the programs need, and nothing else.
# The program runs under `env -i`.
#
# The order log. Every logging stub appends one line, `name argv`, to
# $CZ_CASE/calls, so a case can assert the order of tool calls across stubs.

# shellcheck disable=SC1091,SC2034  # SC2034: the variables are read by lib.sh and the case files
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# The real tools the programs use. A missing one stops the run: a case that
# quietly lacked a tool would be testing the missing-tool path.
CC_TOOL_NAMES="bash cp rm mkdir mv sed grep awk cat chmod touch find sort tr head tail dirname basename readlink git python3 date uname id env timeout ln ls cut wc stat sha256sum mktemp tee diff cmp sleep true false xargs paste nice uniq"

cc_init() {
    cz_init
    CC_TOOLS_ALL="$CZ_ROOT/tools-all"; mkdir -p "$CC_TOOLS_ALL"
    local t p
    for t in $CC_TOOL_NAMES; do
        # type -P names a file. `command -v true` names the builtin, and a link to that name points nowhere.
        p="$(PATH="$CZ_BASE_PATH" type -P "$t")" || { echo "characterization: tool '$t' not found on this machine" >&2; exit 2; }
        [ -x "$p" ] || { echo "characterization: '$t' resolved to '$p', which is not an executable file" >&2; exit 2; }
        ln -s "$p" "$CC_TOOLS_ALL/$t"
    done
    CC_REAL_PYTHON3="$(PATH="$CZ_BASE_PATH" type -P python3)"
    CC_REAL_NICE="$(PATH="$CZ_BASE_PATH" type -P nice)"
    CC_REAL_SLEEP="$(PATH="$CZ_BASE_PATH" type -P sleep)"
    local f
    for f in scripts/check_claims.sh scripts/verify-action-pins.sh scripts/shellcheck-sweep.sh scripts/render-settings.py \
             scripts/render_figures.py evals/run-evals.sh tests/containment-probe.sh; do
        [ -f "$MX_TREE/$f" ] || { echo "characterization: MX_TREE=$MX_TREE has no $f" >&2; exit 2; }
    done
}

# --- the sandbox ---------------------------------------------------------------

cc_sandbox_new() {
    CZ_SEQ=$((CZ_SEQ + 1))
    CZ_CASE="$CZ_ROOT/case-$CZ_SEQ"
    CZ_TREE="$CZ_CASE/tree"; CZ_TOOLS="$CZ_CASE/tools"; CZ_CWD="$CZ_CASE/cwd"; CZ_TMP="$CZ_CASE/tmp"
    mkdir -p "$CZ_CASE/bin" "$CZ_CASE/gh" "$CZ_CASE/home" "$CZ_TREE" "$CZ_CWD" "$CZ_TMP"
    cp -a "$CC_TOOLS_ALL" "$CZ_TOOLS"
    : > "$CZ_CASE/calls"; : > "$CZ_CASE/gh/calls"
    local t
    for t in gh podman docker curl wget ssh; do cz_stub_tool "$t"; done
    CZ_E=()
    CZ_E[PATH]="$CZ_CASE/bin:$CZ_TOOLS"
    CZ_E[HOME]="$CZ_CASE/home"
    CZ_E[LC_ALL]=C
    CZ_E[TZ]=UTC
    CZ_E[LOGNAME]=fixture-user
    CZ_E[USER]=fixture-user
    CZ_E[GIT_CONFIG_NOSYSTEM]=1
    CZ_E[GIT_CONFIG_GLOBAL]=/dev/null
    CZ_E[GIT_TERMINAL_PROMPT]=0
    CZ_E[CZ_CASE_DIR]="$CZ_CASE"
    CZ_E[TMPDIR]="$CZ_TMP"
}

# cc_case NAME FUNCTION: runs FUNCTION in a fresh sandbox. The case fails when an
# assertion fails or when a tool nobody planned ran.
cc_case() {
    CZ_NAME="$1"; CZ_CASE_FAILS=0
    cc_sandbox_new
    "$2"
    cz_case_end
}

# cc_run PROGRAM ARGS...: runs `bash PROGRAM ARGS` under env -i from the case's cwd.
# Sets CZ_RC and leaves $CZ_CASE/stdout and $CZ_CASE/stderr apart.
cc_run() {
    local prog="$1"; shift
    local pairs=() k
    for k in "${!CZ_E[@]}"; do pairs+=("$k=${CZ_E[$k]}"); done
    # Appending, so a program that writes to /dev/stdout through a second open (the live backends do)
    # lands after what the first descriptor already wrote instead of over it.
    : > "$CZ_CASE/stdout"; : > "$CZ_CASE/stderr"
    ( cd "$CZ_CWD" && env -i "${pairs[@]}" timeout 120 bash "$prog" "$@" \
        >> "$CZ_CASE/stdout" 2>> "$CZ_CASE/stderr" < /dev/null )
    CZ_RC=$?
}

cc_drop_tool() { rm -f "$CZ_TOOLS/$1" "$CZ_CASE/bin/$1"; }  # $1 = a tool the program must not find

# cc_logstub NAME BODY: a stub that appends `NAME argv` (argv %q-quoted) to the
# order log, then runs BODY. The first lines of BODY may use $d (the case
# directory) and $line (the logged text).
cc_logstub() {
    {
        cat <<'HEAD'
#!/usr/bin/env bash
d="${CZ_CASE_DIR:?}"
line="$(basename "$0")"; for a in "$@"; do line="$line $(printf '%q' "$a")"; done
printf '%s\n' "$line" >> "$d/calls"
HEAD
        printf '%s\n' "$2"
    } > "$CZ_CASE/bin/$1"
    bash -n "$CZ_CASE/bin/$1" || { echo "characterization: stub $1 does not parse" >&2; exit 2; }
    chmod +x "$CZ_CASE/bin/$1"
}

# cc_expect_calls LINES...: the whole order log, line for line. With no lines the
# log must be empty.
cc_expect_calls() {
    local want="$CZ_CASE/want.calls"
    if [ "$#" -eq 0 ]; then : > "$want"; else printf '%s\n' "$@" > "$want"; fi
    cmp -s "$want" "$CZ_CASE/calls" \
        || cz_note_fail "calls differ. expected: $(tr '\n' '|' < "$want" | cut -c1-400) got: $(tr '\n' '|' < "$CZ_CASE/calls" | cut -c1-400)"
}

# cc_norm_calls: the order log with the case's paths replaced by @CASE@ and the
# program's own temporary names by @TMP@ (mktemp's ten random characters).
cc_norm_calls() {
    sed -e "s|$CZ_CASE|@CASE@|g" -e 's|@CASE@/tmp/tmp\.[A-Za-z0-9]\{10\}|@TMP@|g' "$CZ_CASE/calls"
}
cc_expect_norm_calls() {  # LINES...: the order log, paths normalised, line for line
    local want="$CZ_CASE/want.calls"
    if [ "$#" -eq 0 ]; then : > "$want"; else printf '%s\n' "$@" > "$want"; fi
    cc_norm_calls > "$CZ_CASE/got.calls"
    cmp -s "$want" "$CZ_CASE/got.calls" \
        || cz_note_fail "calls differ. expected: $(tr '\n' '|' < "$want" | cut -c1-500) got: $(tr '\n' '|' < "$CZ_CASE/got.calls" | cut -c1-500)"
}

# cc_count_lines FILE: the number of lines, 0 for an absent file.
cc_count_lines() { if [ -e "$1" ]; then wc -l < "$1" | tr -d ' '; else printf 0; fi; }

# cc_snap DIR: every path under DIR with type, mode and size, and the sha256 of
# each file, sorted. Equal snapshots mean nothing under DIR was created, removed
# or rewritten.
cc_snap() {
    ( cd "$1" && find . -printf '%p %y %m %s\n' | LC_ALL=C sort
      find . -type f | LC_ALL=C sort | while IFS= read -r f; do printf '%s %s\n' "$(sha256sum < "$f" | cut -c1-64)" "$f"; done )
}
cc_expect_unchanged() {  # $1 = label, $2 = snapshot file before, $3 = after
    cmp -s "$2" "$3" || cz_note_fail "$1 changed: $(diff "$2" "$3" | head -6 | tr '\n' '|' | cut -c1-300)"
}
cc_ls() {  # the names directly under $1, sorted, one per line
    ( cd "$1" && ls -A | LC_ALL=C sort )
}

# --- check_claims.sh ---------------------------------------------------------------

# cc_claims_tree: the program, the real renderer of deny walls, a stub suite and a
# stub figure renderer, and the profiles named (default: sysknife).
#
# The suite stub prints $CZ_CASE/suite.out and exits with the code in suite.rc
# (default 0), after logging its working directory and argument count. The
# figure renderer stub logs its argv, prints $CZ_CASE/figs.<mode>.out on stdout
# and figs.<mode>.err on stderr, and exits with figs.<mode>.rc (default 0), where
# mode is check or values.
cc_claims_tree() {
    mkdir -p "$CZ_TREE/scripts" "$CZ_TREE/tests"
    cp "$MX_TREE/scripts/check_claims.sh" "$CZ_TREE/scripts/"
    cp "$MX_TREE/scripts/render-settings.py" "$CZ_TREE/scripts/"
    cat > "$CZ_TREE/tests/run-tests.sh" <<'STUB'
#!/usr/bin/env bash
d="${CZ_CASE_DIR:?}"
printf 'suite cwd=%s args=%d\n' "$PWD" "$#" >> "$d/suite.calls"
printf 'suite %s\n' "$PWD" >> "$d/calls"
[ -e "$d/suite.out" ] && cat "$d/suite.out"
[ -e "$d/suite.err" ] && cat "$d/suite.err" >&2
rc=0; [ -e "$d/suite.rc" ] && rc="$(cat "$d/suite.rc")"
exit "$rc"
STUB
    cat > "$CZ_TREE/scripts/render_figures.py" <<'STUB'
#!/usr/bin/env python3
import os, shlex, sys
d = os.environ["CZ_CASE_DIR"]
mode = "values" if "--values" in sys.argv else "check"
with open(os.path.join(d, "figs.calls"), "a") as f:
    f.write(" ".join(shlex.quote(a) for a in sys.argv[1:]) + "\n")
def read(name):
    p = os.path.join(d, name)
    return open(p).read() if os.path.exists(p) else ""
sys.stdout.write(read("figs.%s.out" % mode))
sys.stderr.write(read("figs.%s.err" % mode))
rc = read("figs.%s.rc" % mode).strip()
sys.exit(int(rc) if rc else 0)
STUB
    python3 -I -c 'import ast,sys; ast.parse(open(sys.argv[1]).read())' "$CZ_TREE/scripts/render_figures.py" || exit 2
    bash -n "$CZ_TREE/tests/run-tests.sh" || exit 2
    cc_profile sysknife
    # python3 logs `python3 [arg] [arg]...` with a newline inside an argument written as \n, then
    # runs the real one. Each argument sits in brackets so an empty one still shows.
    cat > "$CZ_CASE/bin/python3" <<'STUB'
#!/usr/bin/env bash
d="${CZ_CASE_DIR:?}"
line="python3"; for a in "$@"; do a="${a//$'\n'/\\n}"; line="$line [$a]"; done
printf '%s\n' "$line" >> "$d/calls"
exec "${CC_REAL_PYTHON3:?}" "$@"
STUB
    bash -n "$CZ_CASE/bin/python3" && chmod +x "$CZ_CASE/bin/python3" || exit 2
    CZ_E[CC_REAL_PYTHON3]="$CC_REAL_PYTHON3"
}

# cc_profile NAME [DENY-JSON]: profiles/NAME/deny.json. The default is a deny
# list that renders: a verb, the receipt verb the renderer insists on, and a credential path.
cc_profile() {
    mkdir -p "$CZ_TREE/profiles/$1"
    if [ "$#" -ge 2 ]; then printf '%s\n' "$2" > "$CZ_TREE/profiles/$1/deny.json"
    else printf '%s\n' '{"spelled_everywhere": ["cargo publish", "maintainer-merge receipt"], "credential_paths": ["~/.ssh/**"]}' > "$CZ_TREE/profiles/$1/deny.json"; fi
}

cc_suite() {  # $1 = the suite's output text, $2 = its exit code (default 0)
    printf '%s\n' "$1" > "$CZ_CASE/suite.out"; printf '%s' "${2:-0}" > "$CZ_CASE/suite.rc"
}
cc_figs() {  # $1 = check|values, $2 = stdout text, $3 = exit code (default 0), $4 = stderr text
    if [ -n "$2" ]; then printf '%s\n' "$2" > "$CZ_CASE/figs.$1.out"; else : > "$CZ_CASE/figs.$1.out"; fi
    printf '%s' "${3:-0}" > "$CZ_CASE/figs.$1.rc"
    if [ -n "${4:-}" ]; then printf '%s\n' "$4" > "$CZ_CASE/figs.$1.err"; fi
}
cc_claims() { cc_run "$CZ_TREE/scripts/check_claims.sh" "$@"; }

# --- verify-action-pins.sh -------------------------------------------------------------

# cc_pins_tree: the program and an empty .github/workflows.
cc_pins_tree() {
    mkdir -p "$CZ_TREE/scripts" "$CZ_TREE/.github/workflows"
    cp "$MX_TREE/scripts/verify-action-pins.sh" "$CZ_TREE/scripts/"
}

# cc_workflow PATH LINES...: writes a workflow file under the tree; each argument is one line.
cc_workflow() {
    local p="$CZ_TREE/$1"; shift
    mkdir -p "$(dirname "$p")"; printf '%s\n' "$@" > "$p"
}

# cc_gh_pins: the gh stub of the pin verifier. It answers by the exact path and --jq
# filter of an `gh api` call, from $CZ_CASE/gh/answers (one answer per line, fields
# separated by the unit separator: path, filter, exit code, output). A call with no
# answer exits 99 and is logged in gh/unplanned.
cc_gh_pins() {
    cat > "$CZ_CASE/bin/gh" <<'STUB'
#!/usr/bin/env bash
d="${CZ_CASE_DIR:?}/gh"
line=""; for a in "$@"; do line="$line$(printf '%q' "$a") "; done
printf '%s\n' "${line% }" >> "$d/calls"
printf 'gh %s\n' "${line% }" >> "${CZ_CASE_DIR}/calls"
if [ "${1:-}" = api ] && [ "${3:-}" = --jq ]; then
    while IFS=$'\x1f' read -r p j rc out; do
        if [ "$p" = "$2" ] && [ "$j" = "$4" ]; then
            [ -n "$out" ] && printf '%s' "$out"
            [ "$rc" != 0 ] && echo "gh: Not Found (HTTP 404)" >&2
            exit "$rc"
        fi
    done < "$d/answers"
fi
printf '%s\n' "${line% }" >> "$d/unplanned"
echo "stub gh: unplanned call: $*" >&2
exit 99
STUB
    bash -n "$CZ_CASE/bin/gh" && chmod +x "$CZ_CASE/bin/gh" || exit 2
    : > "$CZ_CASE/gh/answers"
}
cc_gh_answer() {  # $1 = api path, $2 = --jq filter, $3 = exit code, $4 = output (printed without a newline)
    printf '%s\x1f%s\x1f%s\x1f%s\n' "$1" "$2" "$3" "$4" >> "$CZ_CASE/gh/answers"
}
# cc_tag REPO TAG SHA: a lightweight tag. A ref whose object is a commit.
cc_tag() {
    cc_gh_answer "repos/$1/git/ref/tags/$2" .object.sha 0 "$3"
    cc_gh_answer "repos/$1/git/ref/tags/$2" .object.type 0 commit
}
# cc_atag REPO TAG OBJ SHA: an annotated tag. The ref names a tag object OBJ, and the object names commit SHA.
cc_atag() {
    cc_gh_answer "repos/$1/git/ref/tags/$2" .object.sha 0 "$3"
    cc_gh_answer "repos/$1/git/ref/tags/$2" .object.type 0 tag
    cc_gh_answer "repos/$1/git/tags/$3" .object.sha 0 "$4"
}
cc_pins() { cc_run "$CZ_TREE/scripts/verify-action-pins.sh" "$@"; }

# --- shellcheck-sweep.sh -------------------------------------------------------------------

# cc_sweep_tree: a git checkout holding the program. cc_sweep_add then adds files; cc_sweep_commit commits them.
cc_sweep_tree() {
    mkdir -p "$CZ_TREE/scripts"
    cp "$MX_TREE/scripts/shellcheck-sweep.sh" "$CZ_TREE/scripts/"
    CC_GIT=(env -i PATH="$CZ_TOOLS" HOME="$CZ_CASE/home" LC_ALL=C GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
        GIT_AUTHOR_NAME=fixture GIT_AUTHOR_EMAIL=fixture@example.invalid
        GIT_COMMITTER_NAME=fixture GIT_COMMITTER_EMAIL=fixture@example.invalid
        GIT_AUTHOR_DATE="1767225600 +0000" GIT_COMMITTER_DATE="1767225600 +0000"
        git -C "$CZ_TREE")
    "${CC_GIT[@]}" init -q -b main || { echo "characterization: git init failed" >&2; exit 2; }
}
# cc_sweep_file PATH CONTENT: writes a file (printf %b, so \0 and \n work) and leaves it untracked.
cc_sweep_file() {
    mkdir -p "$(dirname "$CZ_TREE/$1")"; printf '%b' "$2" > "$CZ_TREE/$1"
}
cc_sweep_commit() {
    "${CC_GIT[@]}" add -A >/dev/null && "${CC_GIT[@]}" commit -q -m base || { echo "characterization: git commit failed" >&2; exit 2; }
}

# cc_sweep_stubs [systemd]: the linter, nice and, with `systemd`, systemd-run, all logging to the order log.
#   the linter  prints $CZ_CASE/sc.out.<basename> on stdout and sc.err.<basename> on stderr when
#               they exist, and exits with sc.rc.<basename>, else sc.rc, else 0.
#   systemd-run logs, fails when sd.fail exists and the command is the probe (it ends in `true`),
#               and otherwise skips its options and runs the command.
#   nice        skips `-n N` and runs the command.
cc_sweep_stubs() {
    cc_logstub shellcheck 'f="${!#}"; b="$(basename "$f")"
printf "%s\n" "$PWD" >> "$d/sc.pwd"
[ -e "$d/sc.out.$b" ] && cat "$d/sc.out.$b"
[ -e "$d/sc.err.$b" ] && cat "$d/sc.err.$b" >&2
if [ -e "$d/sc.rc.$b" ]; then exit "$(cat "$d/sc.rc.$b")"; fi
if [ -e "$d/sc.rc" ]; then exit "$(cat "$d/sc.rc")"; fi
exit 0'
    cc_logstub nice 'shift 2; exec "$@"'
    if [ "${1:-}" = systemd ]; then
        cc_logstub systemd-run '[ -e "$d/sd.fail" ] && [ "${!#}" = true ] && exit 1
while [ $# -gt 0 ]; do case "$1" in -p) shift 2 ;; --*) shift ;; *) break ;; esac; done
exec "$@"'
    fi
}
cc_sweep() { cc_run "$CZ_TREE/scripts/shellcheck-sweep.sh" "$@"; }
# cc_meminfo KB: a /proc/meminfo with MemAvailable of KB, named in MAGENT_MEMINFO.
cc_meminfo() {
    printf 'MemTotal:       32000000 kB\nMemAvailable:   %s kB\n' "$1" > "$CZ_CASE/meminfo"
    CZ_E[MAGENT_MEMINFO]="$CZ_CASE/meminfo"
}

# --- containment-probe.sh ----------------------------------------------------------------------

# cc_probe_tree: the probe, and a stand-in for the merge gate that names every flag the
# probe's last section looks for (one per line).
cc_probe_tree() {
    mkdir -p "$CZ_TREE/tests" "$CZ_TREE/bin"
    cp "$MX_TREE/tests/containment-probe.sh" "$CZ_TREE/tests/"
    printf '%s\n' '--network=none' '--cap-drop=ALL' '--security-opt=no-new-privileges' '--read-only' \
        '--tmpfs /tmp:rw,noexec,nosuid,size=256m' '--pids-limit 512' '--memory 2g' '--cpus 2' > "$CZ_TREE/bin/maintainer-merge"
}

# cc_probe_runtime [NAME]: a container runtime stub named NAME (default podman). Every call is logged
# (argv, %q) in the order log. `image inspect` and `pull` exit with rt.inspect.rc and rt.pull.rc
# (default 0). `ps -q` prints rt.ps.before lines the first time and rt.ps.after the second
# (default 0). `run` logs its flags (everything between the verb and `sh -c PAYLOAD`) in rt.runs
# and the payload, newlines written as \n, in rt.payloads; checks that the mount holds the probe's
# file (rt.mounts); logs the two credential variables it saw (rt.env); then prints
# $CZ_CASE/rt/KEY.out, where KEY is the attack the payload carries, and KEY.err on stderr, and
# exits with rt/KEY.rc (default 0). A key with no .out file prints what a contained run prints
# (the table in the stub). rt.silent makes every run print nothing. rt.escape makes the escape
# attack leave a file next to the mount's parent, the way a runtime that let it through would.
cc_probe_runtime() {
    mkdir -p "$CZ_CASE/rt"
    cc_logstub "${1:-podman}" '
mkdir -p "$d/rt"
case "${1:-}" in
    image) rc=0; [ -e "$d/rt.inspect.rc" ] && rc="$(cat "$d/rt.inspect.rc")"; exit "$rc" ;;
    pull)  rc=0; [ -e "$d/rt.pull.rc" ] && rc="$(cat "$d/rt.pull.rc")"; exit "$rc" ;;
    ps)
        n=1; [ -e "$d/rt.ps.n" ] && n=$(( $(cat "$d/rt.ps.n") + 1 )); printf %s "$n" > "$d/rt.ps.n"
        want=0
        if [ "$n" = 1 ]; then [ -e "$d/rt.ps.before" ] && want="$(cat "$d/rt.ps.before")"
        else [ -e "$d/rt.ps.after" ] && want="$(cat "$d/rt.ps.after")"; fi
        i=0; while [ "$i" -lt "$want" ]; do echo "container$i"; i=$((i+1)); done
        exit 0 ;;
    run) ;;
    *) exit 0 ;;
esac
cmd="${!#}"
args=("$@"); unset "args[${#args[@]}-1]"
printf "%s\n" "${args[*]:1}" >> "$d/rt.runs"
printf "%s\n" "${cmd//$'"'"'\n'"'"'/\\n}" >> "$d/rt.payloads"
mount=""; prev=""
for a in "$@"; do [ "$prev" = -v ] && mount="${a%%:*}"; prev="$a"; done
printf "mount holds: %s | %s\n" "$(ls "$mount" 2>&1 | tr "\n" " ")" "$(cat "$mount/repofile" 2>&1)" >> "$d/rt.mounts"
printf "env: GH_TOKEN=%s ANTHROPIC_API_KEY=%s\n" "${GH_TOKEN-<unset>}" "${ANTHROPIC_API_KEY-<unset>}" >> "$d/rt.env"
case "$cmd" in
    *"/dev/tcp/1.1.1.1/53"*) key=tcp; dflt="rc=1" ;;
    *"getent hosts"*) key=dns; dflt="rc=2" ;;
    *"echo pwned > /etc/passwd"*) key=etc; dflt="rc=1" ;;
    *"chmod +x /tmp/x"*) key=exec; dflt="rc=126" ;;
    *"capsh --print"*) key=caps; dflt="CapEff:	0000000000000000" ;;
    *"grep NoNewPrivs"*) key=nnp; dflt="NoNewPrivs:	1" ;;
    *"sleep 30 &"*) key=fork; dflt="spawned=500" ;;
    *"head -c 4G"*) key=balloon; dflt="rc=1" ;;
    *"while :; do :; done"*) key=loop; dflt="" ;;
    *"cat /proc/1/environ"*) key=creds; dflt="PATH=/usr/bin" ;;
    *"/repo/../escape"*) key=escape; dflt="rc=1"
        [ -e "$d/rt.escape" ] && : > "$TMPDIR/escape" ;;
    *) key=other; dflt="" ;;
esac
printf "%s\n" "$key" >> "$d/rt.keys"
[ -e "$d/rt.silent" ] && exit 0
if [ -e "$d/rt/$key.out" ]; then cat "$d/rt/$key.out"; elif [ -n "$dflt" ]; then printf "%s\n" "$dflt"; fi
[ -e "$d/rt/$key.err" ] && cat "$d/rt/$key.err" >&2
rc=0; [ -e "$d/rt/$key.rc" ] && rc="$(cat "$d/rt/$key.rc")"
exit "$rc"'
}
cc_rt() {  # KEY TEXT: what the runtime prints for the attack KEY (no newline is added)
    mkdir -p "$CZ_CASE/rt"; printf '%s' "$2" > "$CZ_CASE/rt/$1.out"
}
cc_probe() { cc_run "$CZ_TREE/tests/containment-probe.sh" "$@"; }

# --- run-evals.sh ------------------------------------------------------------------------------

# cc_evals_tree: the program, a fixture profile `fx`, three scenarios, a stub orchestrator that
# prints a known prompt and logs how it was called, and a stub backend for live mode.
#
# profiles/fx/evals.json maps 01-alpha and 02-beta to needles the prompt holds, and
# 05-reserved-issue to an explicit "n/a". The scenario files carry a "Where the rule lives"
# section, which live mode cuts off.
cc_evals_tree() {
    mkdir -p "$CZ_TREE/evals/scenarios" "$CZ_TREE/lib/backends" "$CZ_TREE/profiles/fx/prompts"
    cp "$MX_TREE/evals/run-evals.sh" "$CZ_TREE/evals/"
    local id
    for id in 01-alpha 02-beta 05-reserved-issue; do
        printf '# Scenario %s\n\nThe situation of %s.\n\n## Must not\n\nDo the wrong thing.\n\n## Where the rule lives\n\nprompts/review.md\n\n## Why this scenario\n\nBecause.\n' "$id" "$id" > "$CZ_TREE/evals/scenarios/$id.md"
    done
    cat > "$CZ_TREE/lib/run.sh" <<'STUB'
#!/usr/bin/env bash
d="${CZ_CASE_DIR:?}"
line="run.sh"; for a in "$@"; do line="$line $(printf '%q' "$a")"; done
printf '%s\n' "$line" >> "$d/calls"
[ -e "$d/prompt.err" ] && cat "$d/prompt.err" >&2
[ -e "$d/prompt.out" ] && cat "$d/prompt.out"
rc=0; [ -e "$d/prompt.rc" ] && rc="$(cat "$d/prompt.rc")"
exit "$rc"
STUB
    printf 'Doctrine: alpha-rule and beta-rule live here.\n' > "$CZ_CASE/prompt.out"
    cat > "$CZ_TREE/lib/backends/claude.sh" <<'STUB'
# A stand-in backend: prints the prompt file it is given, after one line naming the call.
backend_run() {
    {
        printf 'backend_run model=%s dest=%s root=%s task=%s profile_dir=%s repo=%s\n' "$2" "$3" "$4" "${MAINTAINER_TASK-<unset>}" "${PROFILE_DIR-<unset>}" "${REPO_PATH-<unset>}"
        cat "$1"
    } >> "$3"
    printf 'backend_run\n' >> "${CZ_CASE_DIR:?}/calls"
    [ -e "${CZ_CASE_DIR:?}/backend.fail" ] && return 1
    return 0
}
STUB
    printf 'PROFILE_NAME="fx"\nREPO_PATH="/fixture/repo"\nMODEL_review="fixture-model"\n' > "$CZ_TREE/profiles/fx/profile.env"
    printf 'Common preamble of fx.\n' > "$CZ_TREE/profiles/fx/prompts/common-preamble.md"
    printf 'Prose style rules.\n' > "$CZ_TREE/lib/prose-style.md"
    printf '{"permissions": {"deny": ["Bash(rm -rf:*)"]}}\n' > "$CZ_TREE/profiles/fx/deny.json"
    cc_evals_map '{"01-alpha": {"needle": "alpha-rule"}, "02-beta": {"needle": "beta-rule"}, "05-reserved-issue": {"n/a": "no reserved issues here"}}'
    CZ_E[MAINTAINER_PROFILE]=fx
}
cc_evals_map() { printf '%s\n' "$1" > "$CZ_TREE/profiles/fx/evals.json"; }
cc_evals() { cc_run "$CZ_TREE/evals/run-evals.sh" "$@"; }

# --- the ordering of two files' bytes -------------------------------------------------------------

# cc_expect_file_bytes FILE TEXT: FILE holds exactly TEXT, every byte, newlines included.
cc_expect_file_bytes() {
    printf '%s' "$2" > "$CZ_CASE/want.bytes"
    cmp -s "$CZ_CASE/want.bytes" "$1" || cz_note_fail "$(basename "$1") differs. expected: $(od -c < "$CZ_CASE/want.bytes" | head -3 | tr -s ' ' | tr '\n' '|' | cut -c1-240) got: $(od -c < "$1" | head -3 | tr -s ' ' | tr '\n' '|' | cut -c1-240)"
}
