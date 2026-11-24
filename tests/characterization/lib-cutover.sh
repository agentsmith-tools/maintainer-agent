#!/usr/bin/env bash
# Helpers for the characterization tests of scripts/cutover.sh. Sourced by cutover-*.sh after
# lib.sh, never run alone.
#
# Isolation. Every case gets a fresh sandbox, and the real HOME is never named:
#   home/     the HOME of the program: a deployed bash install of the fixture checkout.
#   fx/       a copy of a small fixture checkout (a git repository) that holds the script under
#             test and bash programs that print what they were given.
#   build/    a build directory whose programs are stand-ins for the Rust builds. A stand-in
#             runs the matching bash program, or, when standin/NAME.mode says so, a changed one:
#             one byte of output, another exit code, an extra stderr line, a write to gh.
#   bin/      stubs first on PATH: a recorder `gh` that must never be reached (the shadow puts
#             its own shim in front), cargo and rustc for the build cases, notify-send.
#   tools/    symlinks to the few real tools the script needs, and nothing else.
# The script runs under `env -i` with that PATH, so a tool it needs and the case did not
# provide is missing, not ambient.

# shellcheck disable=SC1091,SC2034  # SC2034: variables are read by lib.sh and the case files
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

CU_TOOL_NAMES="bash cp rm mkdir mv sed grep awk cat chmod touch find sort tr od head tail dirname basename readlink git python3 date uname id env flock timeout ln ls cut wc stat sha256sum mktemp tee diff cmp sleep true false xargs printf expr seq"
CU_PROGRAMS="maintainer-merge maintainer-doctor maintainer-repo run run-instance alert"
# Every program of the build directory the shadow looks for.
CU_ALL_PROGRAMS="$CU_PROGRAMS install install-cron check-claims shellcheck-sweep verify-action-pins run-evals containment-probe"

cu_init() {
    cz_init
    CU_SRC="$MX_TREE/scripts/cutover.sh"
    [ -f "$CU_SRC" ] || { echo "characterization: no scripts/cutover.sh under MX_TREE=$MX_TREE" >&2; exit 2; }
    CU_TOOLS_ALL="$CZ_ROOT/tools-all"; mkdir -p "$CU_TOOLS_ALL"
    local t p
    for t in $CU_TOOL_NAMES; do
        p="$(type -P "$t")" || { echo "characterization: tool '$t' not found on this machine" >&2; exit 2; }
        ln -s "$p" "$CU_TOOLS_ALL/$t"
    done
    CU_REAL_DATE="$(type -P date)"
    cu_make_fixture
}

# --- the fixture checkout -------------------------------------------------------------------------

# A program of the fixture: prints its name and arguments, and runs a hook the case may leave in
# HOME (a hook is how a case makes the bash program call gh, or fail).
cu_fx_program() {  # $1 = file, $2 = name, $3 = extra body
    {
        printf '#!/usr/bin/env bash\n'
        printf 'set -u\n'
        printf '[ -f "$HOME/.fx-hook-%s" ] && . "$HOME/.fx-hook-%s"\n' "$2" "$2"
        printf '%s\n' "$3"
    } > "$1"
    chmod 755 "$1"
}

cu_make_fixture() {
    local d="$CZ_ROOT/fx-pristine"
    mkdir -p "$d/bin" "$d/lib" "$d/scripts" "$d/evals" "$d/tests" "$d/platform/linux" "$d/platform/posix" \
             "$d/rust/scripts" "$d/profiles/alpha/prompts" "$d/profiles/beta/prompts"
    cp "$CU_SRC" "$d/scripts/cutover.sh"
    printf '#!/usr/bin/env bash\necho "fixture install.sh"\n' > "$d/install.sh"; chmod 755 "$d/install.sh"
    printf '[workspace]\nmembers = []\n' > "$d/rust/Cargo.toml"
    printf '/rust/target\n' > "$d/.gitignore"
    cu_fx_program "$d/bin/maintainer" maintainer 'echo "maintainer helper"'
    cu_fx_program "$d/bin/maintainer-merge" maintainer-merge '
case "${1:-}" in
    show)
        shift
        if [ "$#" -ge 1 ]; then
            cat "$MAINTAINER_STATE/receipts/$1.json" 2>/dev/null || { echo "maintainer-merge: no receipt for #$1" >&2; exit 1; }
        else
            for f in "$MAINTAINER_STATE"/receipts/*.json; do [ -e "$f" ] && echo "  $(basename "$f" .json): present"; done
        fi ;;
    verify-deps)
        head="$(gh pr view "${2:?pr}" --repo "$MAINTAINER_SLUG" --json headRefOid --jq .headRefOid)" || exit 1
        echo "head of #$2 is $head" ;;
    *) echo "usage: maintainer-merge show|verify-deps" >&2; exit 64 ;;
esac'
    cu_fx_program "$d/bin/maintainer-doctor" maintainer-doctor '
echo "Deployment"
echo "  ok    profile ${MAINTAINER_PROFILE:-none} checked at $(date +%Y-%m-%dT%H:%M:%S%z)"
echo "  ok    pid $$ is the doctor"
echo "  ok    the last run ended 2026-11-20T10:00:00+00:00"
command -v gh >/dev/null && echo "  ok    gh resolves"'
    cu_fx_program "$d/bin/maintainer-repo" maintainer-repo '
case "${1:-}" in
    prune) git -C "${MAINTAINER_REPO:-.}" fetch -q --prune origin; echo "  fetch rc=$?"; echo "  prune ${2:-} for ${MAINTAINER_PROFILE:-none}" ;;
    release-check) echo "  release-check for ${MAINTAINER_PROFILE:-none}" ;;
    fixup) [ -n "${2:-}" ] || { echo "usage: maintainer-repo fixup <pr>" >&2; exit 64; }; echo "fixup $2" ;;
    *) echo "usage: maintainer-repo {prune|release-check|fixup}" >&2; exit 64 ;;
esac'
    cu_fx_program "$d/lib/run.sh" run '
share="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
[ "${1:-}" = --show-prompt ] || { echo "run.sh: only --show-prompt in this fixture" >&2; exit 2; }
profile="${2:?usage: run.sh [--show-prompt] <profile> <task>}"
task="${3:?usage: run.sh [--show-prompt] <profile> <task>}"
[ -f "$share/profiles/$profile/profile.env" ] || { echo "run.sh: no profile $profile" >&2; exit 64; }
. "$share/profiles/$profile/profile.env"
case " $TASKS " in *" $task "*) ;; *) echo "run.sh: unknown task $task" >&2; exit 64 ;; esac
echo "Provenance for this run: profile $profile, task $task, started $(date -Is)."
echo "style=${MAINTAINER_PROSE_STYLE:-stop-slop}"
cat "$share/profiles/$profile/prompts/$task.md"'
    cu_fx_program "$d/platform/linux/run-instance.sh" run-instance '
instance="${1:?usage: run-instance.sh <profile>-<task>}"
echo "instance $instance"'
    cu_fx_program "$d/platform/linux/alert.sh" alert '
instance="${1:-unknown}"; profile="${instance%-*}"; dirs="$HOME/.local/state/${profile}-maint"
if [ -d "$dirs" ]; then mkdir -p "$dirs/logs"; printf "%s  ALERT %s\n" "$(date -Is)" "$instance" >> "$dirs/logs/alerts.log"; fi
[ "${MAINTAINER_NOTIFY:-on}" = off ] && exit 0
notify-send -u critical -a maintainer "maintainer: $instance FAILED" "fix maintainer-doctor"'
    cu_fx_program "$d/platform/posix/install-cron.sh" install-cron 'echo "cron ${MAINTAINER_PROFILE:-none} ${1:-}"'
    cu_fx_program "$d/scripts/check_claims.sh" check-claims 'echo "claims ok"'
    cu_fx_program "$d/scripts/shellcheck-sweep.sh" shellcheck-sweep 'echo "sweep ok"'
    cu_fx_program "$d/scripts/verify-action-pins.sh" verify-action-pins 'echo "pins ok"'
    cu_fx_program "$d/evals/run-evals.sh" run-evals 'echo "evals ok for ${MAINTAINER_PROFILE:-none}"'
    cu_fx_program "$d/tests/containment-probe.sh" containment-probe 'echo "probe: no container runtime in this fixture"'
    local p
    for p in alpha beta; do
        printf 'TASKS="review issues"\nSTATE_DIR="$HOME/.local/state/%s-maint"\n' "$p" > "$d/profiles/$p/profile.env"
        printf 'review prompt of %s\n' "$p" > "$d/profiles/$p/prompts/review.md"
        printf 'issues prompt of %s\n' "$p" > "$d/profiles/$p/prompts/issues.md"
    done
    git -C "$d" init -q -b main
    git -C "$d" add -A
    git -C "$d" -c user.name=fixture -c user.email=fixture@example.invalid commit -q -m fixture
}

# --- the sandbox ---------------------------------------------------------------------------------

cu_sandbox_new() {
    CZ_SEQ=$((CZ_SEQ + 1))
    CZ_CASE="$CZ_ROOT/case-$CZ_SEQ"
    CU_HOME="$CZ_CASE/home"; CU_FX="$CZ_CASE/fx"; CU_TOOLS="$CZ_CASE/tools"; CU_BUILD="$CZ_CASE/build"
    CU_SHARE="$CU_HOME/.local/share/maintainer"; CU_BIN="$CU_HOME/.local/bin"
    mkdir -p "$CZ_CASE/bin" "$CZ_CASE/standin" "$CU_HOME"
    cp -a "$CZ_ROOT/fx-pristine" "$CU_FX"
    cp -a "$CU_TOOLS_ALL" "$CU_TOOLS"
    cu_stub_recorder gh
    cu_stub_recorder claude
    cu_stub_recorder podman
    cat > "$CZ_CASE/bin/notify-send" <<'STUB'
#!/usr/bin/env bash
printf 'notify-send %s\n' "$*" >> "${CZ_CASE_DIR:?}/real-notify-send"
exit 0
STUB
    chmod +x "$CZ_CASE/bin/notify-send"
    CZ_E=()
    CZ_E[PATH]="$CZ_CASE/bin:$CU_TOOLS"
    CZ_E[HOME]="$CU_HOME"
    CZ_E[LC_ALL]=C
    CZ_E[TZ]=UTC
    CZ_E[LOGNAME]=fixture-user
    CZ_E[USER]=fixture-user
    CZ_E[GIT_CONFIG_NOSYSTEM]=1
    CZ_E[GIT_CONFIG_GLOBAL]=/dev/null
    CZ_E[GIT_TERMINAL_PROMPT]=0
    CZ_E[CZ_CASE_DIR]="$CZ_CASE"
    CZ_E[PYTHONDONTWRITEBYTECODE]=1
}

# A tool the real machine has and a case must never reach: it logs the call, and the case checks
# that the log is empty. The shadow puts its own shims in front of this one.
cu_stub_recorder() {  # $1 = name
    cat > "$CZ_CASE/bin/$1" <<'STUB'
#!/usr/bin/env bash
name="$(basename "$0")"
printf '%s %s\n' "$name" "$*" >> "${CZ_CASE_DIR:?}/reached-real-$name"
echo "stub: the real $name was reached" >&2
exit 97
STUB
    chmod +x "$CZ_CASE/bin/$1"
}

# CU_ONLY=text runs the cases whose name holds the text, for a person working on one of them. The
# summary then counts only those, and the runner of a full run never sets it.
cu_case() {  # $1 = name, $2 = function
    if [ -n "${CU_ONLY:-}" ] && [[ "$1" != *"$CU_ONLY"* ]]; then return 0; fi
    CZ_NAME="$1"; CZ_CASE_FAILS=0
    cu_sandbox_new
    "$2"
    cu_case_end
}

# The real tools must not have been reached, in any case.
cu_case_end() {
    local f
    for f in "$CZ_CASE"/reached-real-*; do
        [ -e "$f" ] || continue
        cz_note_fail "the real ${f##*reached-real-} was reached: $(head -1 "$f")"
    done
    [ ! -e "$CZ_CASE/real-notify-send" ] || cz_note_fail "the real notify-send was reached"
    cz_case_end
}

# cu_home_at PATH: the sandbox HOME is PATH (a path the case chose) instead of home/.
cu_home_at() {
    rmdir "$CU_HOME" 2>/dev/null
    CU_HOME="$1"; CU_SHARE="$CU_HOME/.local/share/maintainer"; CU_BIN="$CU_HOME/.local/bin"
    mkdir -p "$CU_HOME"
    CZ_E[HOME]="$CU_HOME"
}

# The checkout the script lives in, as a case sees it.
cu_script() { printf '%s' "$CU_FX/scripts/cutover.sh"; }

# cu_run ARGS...: runs the script under env -i. Sets CZ_RC; streams in $CZ_CASE/stdout, stderr.
cu_run() {
    local pairs=() k
    for k in "${!CZ_E[@]}"; do pairs+=("$k=${CZ_E[$k]}"); done
    ( cd "$CU_HOME" && env -i "${pairs[@]}" timeout 300 bash "$(cu_script)" "$@" \
        > "$CZ_CASE/stdout" 2> "$CZ_CASE/stderr" < /dev/null )
    CZ_RC=$?
}

# --- the deployed bash install ----------------------------------------------------------------------

# cu_deploy [PROFILE...]: what ./install.sh leaves in HOME, for the fixture checkout.
cu_deploy() {
    local profiles=("$@") p
    [ "${#profiles[@]}" -gt 0 ] || profiles=(alpha)
    mkdir -p "$CU_BIN" "$CU_SHARE/profiles" "$CU_SHARE/scripts"
    local n
    for n in maintainer maintainer-merge maintainer-doctor maintainer-repo; do cp "$CU_FX/bin/$n" "$CU_BIN/$n"; done
    cp "$CU_FX/lib/run.sh" "$CU_SHARE/run.sh"
    cp "$CU_FX/platform/linux/run-instance.sh" "$CU_SHARE/run-instance.sh"
    cp "$CU_FX/platform/linux/alert.sh" "$CU_SHARE/alert.sh"
    chmod 755 "$CU_BIN"/* "$CU_SHARE"/*.sh
    for p in "${profiles[@]}"; do cp -a "$CU_FX/profiles/$p" "$CU_SHARE/profiles/$p"; done
    printf 'version=fixture\ncommit=%s\ninstalled_at=2026-01-01T00:00:00+00:00\ninstalled_from=%s\n' "$(git -C "$CU_FX" rev-parse --short HEAD)" "$CU_FX" > "$CU_SHARE/VERSION"
}

# The deployed path of a program, and the bash program in the fixture checkout it comes from.
cu_installed() {
    case "$1" in
        maintainer-merge|maintainer-doctor|maintainer-repo) printf '%s' "$CU_BIN/$1" ;;
        run) printf '%s' "$CU_SHARE/run.sh" ;;
        run-instance) printf '%s' "$CU_SHARE/run-instance.sh" ;;
        alert) printf '%s' "$CU_SHARE/alert.sh" ;;
    esac
}
cu_source() {
    case "$1" in
        maintainer-merge|maintainer-doctor|maintainer-repo) printf '%s' "$CU_FX/bin/$1" ;;
        run) printf '%s' "$CU_FX/lib/run.sh" ;;
        run-instance) printf '%s' "$CU_FX/platform/linux/run-instance.sh" ;;
        alert) printf '%s' "$CU_FX/platform/linux/alert.sh" ;;
    esac
}
# The bash program a stand-in stands for: the deployed one, or the checkout's for the checks.
cu_target() {
    case "$1" in
        maintainer-merge|maintainer-doctor|maintainer-repo|run|run-instance|alert) cu_installed "$1" ;;
        install) printf '%s' "$CU_FX/install.sh" ;;
        install-cron) printf '%s' "$CU_FX/platform/posix/install-cron.sh" ;;
        check-claims) printf '%s' "$CU_FX/scripts/check_claims.sh" ;;
        shellcheck-sweep) printf '%s' "$CU_FX/scripts/shellcheck-sweep.sh" ;;
        verify-action-pins) printf '%s' "$CU_FX/scripts/verify-action-pins.sh" ;;
        run-evals) printf '%s' "$CU_FX/evals/run-evals.sh" ;;
        containment-probe) printf '%s' "$CU_FX/tests/containment-probe.sh" ;;
    esac
}

# --- the build directory with stand-ins ---------------------------------------------------------------

cu_standin() {  # $1 = program name: writes $CU_BUILD/NAME
    local name="$1" target; target="$(cu_target "$1")"
    cat > "$CU_BUILD/$name" <<STANDIN
#!/usr/bin/env bash
# Stand-in for the Rust build of $name. standin/$name.mode changes what it does.
mode=same; [ -f "$CZ_CASE/standin/$name.mode" ] && mode="\$(cat "$CZ_CASE/standin/$name.mode")"
case "\$mode" in
    same) exec "$target" "\$@" ;;
    append) "$target" "\$@"; rc=\$?; printf 'X'; exit "\$rc" ;;
    firstbyte) "$target" "\$@" | sed '1s/^./Z/'; exit "\${PIPESTATUS[0]}" ;;
    rc) "$target" "\$@"; rc=\$?; exit \$((rc + 1)) ;;
    stderr) "$target" "\$@"; rc=\$?; echo "an extra line on stderr" >&2; exit "\$rc" ;;
    sleep) echo \$\$ > "$CZ_CASE/standin/$name.pid"; exec sleep 30 ;;
    delay) sleep 1.2; exec "$target" "\$@" ;;
    gh-write) gh pr comment 1 --body x >/dev/null 2>&1; exec "$target" "\$@" ;;
    gh-extra) gh api user --jq .login >/dev/null 2>&1; exec "$target" "\$@" ;;
    git-push) git push origin main >/dev/null 2>&1; exec "$target" "\$@" ;;
    tweak) out="\$("$target" "\$@")"; rc=\$?; case "\$*" in *"beta issues"*) out="\${out/issues prompt/issuez prompt}" ;; esac; [ -z "\$out" ] || printf '%s\n' "\$out"; exit "\$rc" ;;
    esc) "$target" "\$@"; rc=\$?; printf '\033[31mred\033[0m\n'; exit "\$rc" ;;
    *) echo "stand-in: unknown mode \$mode" >&2; exit 98 ;;
esac
STANDIN
    chmod 755 "$CU_BUILD/$name"
}

# cu_build [NAME...]: a build directory holding stand-ins, SHA256SUMS and BUILD_INFO, as `build` leaves one.
cu_build() {
    local names=("$@") n
    [ "${#names[@]}" -gt 0 ] || read -r -a names <<<"$CU_ALL_PROGRAMS"
    mkdir -p "$CU_BUILD"; chmod 755 "$CU_BUILD"
    for n in "${names[@]}"; do cu_standin "$n"; done
    cu_table > "$CU_BUILD/SHA256SUMS"
    printf 'head=%s\ndirty=0\nprograms=%s\n' "$(cu_head)" "${names[*]}" > "$CU_BUILD/BUILD_INFO"
}

# The table of the programs now in the build directory, computed here and not by the script.
cu_table() {
    local f name
    for f in "$CU_BUILD"/*; do
        name="${f##*/}"
        case "$name" in SHA256SUMS|BUILD_INFO|shadow-ok|shadow-report.txt) continue ;; esac
        printf '%s  %s\n' "$(sha256sum < "$f" | cut -d' ' -f1)" "$name"
    done | LC_ALL=C sort -k2
}

cu_head() { git -C "$CU_FX" rev-parse HEAD; }

# cu_marker [SKIPPED_PROGRAM...]: the shadow-ok and shadow-report.txt a clean shadow of the build directory leaves.
cu_marker() {
    local rep="$CU_BUILD/shadow-report.txt" short n; short="$(cu_head | cut -c1-7)"
    { printf 'SHADOW home=%s checkout=%s deployed=%s build=%s profiles=1\n' "$CU_HOME" "$short" "$short" "$short"
      printf 'PASS run --show-prompt alpha review (default)\nPASS install --dry-run\n'
      for n in "$@"; do printf 'SKIP %s show: no Rust build of %s in %s (the bash program stays)\n' "$n" "$n" "$CU_BUILD"; done
      printf 'SUMMARY comparisons=%d pass=2 diff=0 skip=%d\n' "$(( 2 + $# ))" "$#"; } > "$rep"
    { cu_table; printf '# checkout: %s\n# report: %s\n' "$(cu_head)" "$(sha256sum < "$rep" | cut -d' ' -f1)"
      for n in "$@"; do printf '# accepted-skip: %s\n' "$n"; done; } > "$CU_BUILD/shadow-ok"
}

# cu_build_without NAME...: a build directory with every program except these.
cu_build_without() {
    local keep=() n skip w
    for n in $CU_ALL_PROGRAMS; do
        skip=0
        for w in "$@"; do [ "$w" = "$n" ] && skip=1; done
        [ "$skip" = 1 ] || keep+=("$n")
    done
    cu_build "${keep[@]}"
}

# How many entries of a directory begin with a dot.
cu_dot_entries() {
    local n=0 f
    for f in "$1"/.[!.]*; do [ -e "$f" ] || [ -L "$f" ] || continue; n=$((n + 1)); done
    printf '%s' "$n"
}

# --- running the shadow -----------------------------------------------------------------------------------

# cu_shadow [ARGS...]: the shadow on the case's build directory.
cu_shadow() { cu_run shadow --out "$CU_BUILD" "$@"; }
cu_report() { cat "$CU_BUILD/shadow-report.txt" 2>/dev/null; }

# --- assertions --------------------------------------------------------------------------------------------

cu_expect_report_has() {  # $1 = a fixed string
    local text; text="$(cu_report)"
    grep -qF -- "$1" <<<"$text" || cz_note_fail "the report lacks '$1': $(tr '\n' '|' <<<"$text" | cut -c1-300)"
}
cu_expect_report_lacks() {
    local text; text="$(cu_report)"
    if grep -qF -- "$1" <<<"$text"; then cz_note_fail "the report holds '$1' and must not: $(tr '\n' '|' <<<"$text" | cut -c1-300)"; fi
}
cu_expect_marker() { [ -f "$CU_BUILD/shadow-ok" ] || cz_note_fail "no shadow-ok was written"; }
cu_expect_no_marker() { [ ! -e "$CU_BUILD/shadow-ok" ] || cz_note_fail "shadow-ok exists and must not"; }
# The number of lines of the report that start with a word.
cu_report_count() {  # $1 = PASS|DIFF|SKIP
    local n; n="$(grep -c "^$1 " "$CU_BUILD/shadow-report.txt" 2>/dev/null || true)"
    printf '%s' "${n:-0}"
}

# A listing of a tree: every path with its type, mode, link target and the hash of its bytes.
cu_snap() {  # $1 = directory
    ( cd "$1" && find . -mindepth 1 \( -type f -o -type l -o -type d \) -print0 | LC_ALL=C sort -z | while IFS= read -r -d '' p; do
        if [ -L "$p" ]; then printf 'l %s -> %s\n' "$p" "$(readlink "$p")"
        elif [ -d "$p" ]; then printf 'd %s %s\n' "$p" "$(stat -c %a "$p")"
        else printf 'f %s %s %s\n' "$p" "$(stat -c %a "$p")" "$(sha256sum < "$p" | cut -d' ' -f1)"; fi
    done )
}
cu_expect_same_snap() {  # $1 = label, $2 = before file, $3 = after file
    cmp -s "$2" "$3" || cz_note_fail "$1 changed: $(diff "$2" "$3" | head -4 | tr '\n' '|' | cut -c1-240)"
}

# --- the state `install --rust` leaves ------------------------------------------------------------------------

# cu_stamp NAME...: RUST_STAMP listing these programs, each with the hash of its Rust build.
cu_stamp() {
    local n
    : > "$CU_SHARE/RUST_STAMP"
    for n in "$@"; do
        printf '%s  %s\n' "$(sha256sum < "$CU_BUILD/$n" | cut -d' ' -f1)" "$(cu_installed "$n")" >> "$CU_SHARE/RUST_STAMP"
    done
}

# cu_swap_files NAME...: the bash original saved beside the program, the Rust build in its place.
cu_swap_files() {
    local n p
    for n in "$@"; do
        p="$(cu_installed "$n")"
        cp -p "$p" "$p.sh"
        cp "$CU_BUILD/$n" "$p"; chmod 755 "$p"
    done
}

# cu_swap [NAME...]: a full `--rust` install of these programs (every program by default).
cu_swap() {
    local names=("$@")
    [ "${#names[@]}" -gt 0 ] || read -r -a names <<<"$CU_PROGRAMS"
    cu_stamp "${names[@]}"
    cu_swap_files "${names[@]}"
}

# --- cargo for the build cases ---------------------------------------------------------------------------------

# A cargo that records its calls and its environment, answers `metadata` from cargo.metadata.json and
# writes a small file for each `build ... --bin NAME`. A case makes one fail or come out empty by creating
# cargo.fail.NAME or cargo.empty.NAME in the case directory.
cu_fake_cargo() {
    cat > "$CZ_CASE/bin/cargo" <<'STUB'
#!/usr/bin/env bash
d="${CZ_CASE_DIR:?}"
{ printf 'cargo'; for a in "$@"; do printf ' %s' "$a"; done; printf '\n'
  printf 'env flags=%s jobs=%s target=%s color=%s cwd=%s\n' "${CARGO_ENCODED_RUSTFLAGS-<unset>}" "${CARGO_BUILD_JOBS-<unset>}" "${CARGO_TARGET_DIR-<unset>}" "${CARGO_TERM_COLOR-<unset>}" "$PWD"; } >> "$d/cargo.calls"
case "${1:-}" in
    metadata)
        if [ -e "$d/cargo.metadata.rc" ]; then echo "error: failed to read the manifest" >&2; exit "$(cat "$d/cargo.metadata.rc")"; fi
        cat "$d/cargo.metadata.json"; exit 0 ;;
    build)
        bin=""; prev=""; for a in "$@"; do [ "$prev" = --bin ] && bin="$a"; prev="$a"; done
        if [ -e "$d/cargo.fail.$bin" ]; then echo "error: could not compile $bin" >&2; exit 101; fi
        mkdir -p "${CARGO_TARGET_DIR:?}/release"
        if [ -e "$d/cargo.empty.$bin" ]; then : > "$CARGO_TARGET_DIR/release/$bin"
        else printf 'rust build of %s\n' "$bin" > "$CARGO_TARGET_DIR/release/$bin"; fi
        if [ -e "$d/cargo.noexec.$bin" ]; then chmod 644 "$CARGO_TARGET_DIR/release/$bin"; else chmod 755 "$CARGO_TARGET_DIR/release/$bin"; fi
        exit 0 ;;
esac
echo "fake cargo: unplanned call: $*" >&2
exit 99
STUB
    cat > "$CZ_CASE/bin/rustc" <<'STUB'
#!/usr/bin/env bash
if [ "$*" = "--print sysroot" ]; then echo /fake/toolchain; exit 0; fi
exit 99
STUB
    chmod +x "$CZ_CASE/bin/cargo" "$CZ_CASE/bin/rustc"
}

# cu_metadata PACKAGE:BIN...: what `cargo metadata` prints for a workspace with these binaries.
cu_metadata() {
    local spec pkg bin first=1
    {
        printf '{"packages":['
        for spec in "$@"; do
            pkg="${spec%%:*}"; bin="${spec#*:}"
            [ "$first" = 1 ] || printf ','
            first=0
            printf '{"name":"%s","targets":[{"kind":["bin"],"name":"%s"},{"kind":["lib"],"name":"%s"}]}' "$pkg" "$bin" "${pkg//-/_}"
        done
        printf ']}\n'
    } > "$CZ_CASE/cargo.metadata.json"
}

# The programs a build of the fixture makes, the way main builds them today (no gate binary yet).
CU_BUILT="install alert run-instance install-cron check-claims shellcheck-sweep verify-action-pins run-evals containment-probe maintainer-doctor maintainer-repo run"
cu_default_metadata() {
    cu_metadata mx-install:install mx-install:alert mx-install:run-instance mx-install:install-cron \
        mx-checks:check-claims mx-checks:shellcheck-sweep mx-checks:verify-action-pins mx-checks:run-evals \
        mx-checks:containment-probe mx-doctor:maintainer-doctor mx-repo:maintainer-repo mx-run:run \
        mx-probe:helper-tool
}

# --- a stand-in for `install --rust --rust-bin DIR` -------------------------------------------------------------------

# Does what the Rust installer does to the machine, in the order it does it: the bash programs of the
# checkout are deployed, RUST_STAMP is written, then each program is saved as NAME.sh and replaced by its
# build. standin/install.mode makes it go wrong in one named way. It records its arguments.
cu_fake_install() {
    cat > "$CU_BUILD/install" <<STANDIN
#!/usr/bin/env bash
case_dir="$CZ_CASE"; root="$CU_FX"
mode=ok; [ -f "\$case_dir/standin/install.mode" ] && mode="\$(cat "\$case_dir/standin/install.mode")"
printf '%s\n' "\$*" >> "\$case_dir/install.calls"
dry=0; rustbin=""
while [ "\$#" -gt 0 ]; do
    case "\$1" in --dry-run) dry=1 ;; --rust-bin) rustbin="\$2"; shift ;; esac
    shift
done
share="\$HOME/.local/share/maintainer"; bin="\$HOME/.local/bin"
names="maintainer-merge maintainer-doctor maintainer-repo run run-instance alert"
inst() { case "\$1" in run|run-instance|alert) echo "\$share/\$1.sh" ;; *) echo "\$bin/\$1" ;; esac; }
src() { case "\$1" in run) echo "\$root/lib/run.sh" ;; run-instance|alert) echo "\$root/platform/linux/\$1.sh" ;; *) echo "\$root/bin/\$1" ;; esac; }
if [ "\$dry" = 1 ]; then
    echo "  would: install the Rust builds from \$rustbin"
    [ "\$mode" = dry-fail ] && { echo "  the dry run found a problem" >&2; exit 3; }
    exit 0
fi
for n in \$names; do [ -f "\$(src "\$n")" ] && cp "\$(src "\$n")" "\$(inst "\$n")"; done
[ "\$mode" = fail-first ] && { echo "install: cannot write" >&2; exit 1; }
present=""
for n in \$names; do
    [ -f "\$rustbin/\$n" ] || continue
    case "\$n" in run-instance|alert) [ "\$(uname -s)" = Linux ] || continue ;; esac
    present="\$present \$n"
done
if [ "\$mode" != no-stamp ]; then
    : > "\$share/RUST_STAMP"
    k=0
    for n in \$present; do
        k=\$((k + 1))
        [ "\$mode" = short-stamp ] && [ "\$k" -gt 2 ] && continue
        h="\$(sha256sum < "\$rustbin/\$n" | cut -d' ' -f1)"
        [ "\$mode" = bad-stamp-hash ] && [ "\$k" = 1 ] && h="\$(printf 'f%.0s' \$(seq 1 64))"
        printf '%s  %s\n' "\$h" "\$(inst "\$n")" >> "\$share/RUST_STAMP"
    done
fi
k=0
for n in \$present; do
    k=\$((k + 1)); p="\$(inst "\$n")"
    [ "\$mode" = fail-after-2 ] && [ "\$k" -gt 2 ] && { echo "install: cannot install \$n" >&2; exit 1; }
    [ "\$mode" = no-backup ] && [ "\$n" = run ] || cp -p "\$p" "\$p.sh"
    [ "\$mode" = bad-backup ] && [ "\$n" = run ] && printf 'tampered\n' > "\$p.sh"
    [ "\$mode" = no-swap ] && [ "\$n" = alert ] && continue
    cp "\$rustbin/\$n" "\$p"; chmod 755 "\$p"
    [ "\$mode" = corrupt ] && [ "\$n" = alert ] && printf 'corrupt\n' > "\$p"
    echo "  rust: \$n is the Rust build"
done
[ "\$mode" = exit-late ] && exit 1
exit 0
STANDIN
    chmod 755 "$CU_BUILD/install"
    cu_table > "$CU_BUILD/SHA256SUMS"
}
