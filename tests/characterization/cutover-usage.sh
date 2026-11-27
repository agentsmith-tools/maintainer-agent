#!/usr/bin/env bash
# Characterization: scripts/cutover.sh, what it refuses before it does anything. The usage
# errors (exit 64), the refusals every subcommand shares (an unusable HOME, a directory that is a
# link, is world-writable, holds a newline or a colon, a missing tool, a tree that is not a
# checkout), and the check that a refusal leaves the machine as it was.
#
# Exit codes: 64 for usage, 1 for a refusal. A refusal names what it refused on standard error.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib-cutover.sh"
cu_init

# --- usage (64) --------------------------------------------------------------------------------------------

case_no_subcommand() {
    cu_run
    cz_expect_rc 64
    cz_expect_stdout_empty
    cz_expect_stderr_has "usage: scripts/cutover.sh build"
}
cu_case "usage: no subcommand exits 64 and prints the usage" case_no_subcommand

case_unknown_subcommand() {
    cu_run frobnicate
    cz_expect_rc 64
    cz_expect_stdout_empty
    cz_expect_stderr_first "cutover.sh: unknown subcommand frobnicate"
}
cu_case "usage: an unknown subcommand exits 64 and names it" case_unknown_subcommand

case_hostile_subcommand() {
    cu_run $'evil\nname'
    cz_expect_rc 64
    cz_expect_stdout_empty
    cz_expect_stderr_first "cutover.sh: unknown subcommand \$'evil\\nname'"
}
cu_case "usage: a subcommand with a newline is shown on one quoted line" case_hostile_subcommand

case_help() {
    cu_run help
    cz_expect_rc 0
    cz_expect_stderr_has "scripts/cutover.sh shadow   [--out DIR] [--accept-skip NAME]... [--timeout SECONDS]"
}
cu_case "usage: help prints the usage and exits 0" case_help

case_help_names_only_flag() {
    cu_run help
    cz_expect_rc 0
    cz_expect_stderr_has "scripts/cutover.sh build    [--out DIR] [--allow-dirty] [--only NAME]..."
}
cu_case "usage: help shows the --only flag of build" case_help_names_only_flag

case_unknown_flag() {
    local sub
    for sub in build shadow live status rollback; do
        cu_run "$sub" --bogus
        cz_check "$sub --bogus exits" "$CZ_RC" 64
        cz_expect_stderr_has "cutover.sh: $sub: unknown argument --bogus"
        cz_expect_stdout_empty
    done
}
cu_case "usage: an unknown flag exits 64 for each of the five subcommands" case_unknown_flag

case_flag_without_value() {
    local sub flag
    for sub in "build --out" "shadow --out" "shadow --accept-skip" "shadow --timeout" "live --build-dir" "status --build-dir"; do
        flag="${sub#* }"
        # shellcheck disable=SC2086
        cu_run $sub
        cz_check "$sub with no value exits" "$CZ_RC" 64
        cz_expect_stderr_has "$flag needs"
    done
}
cu_case "usage: a flag that needs a value and has none exits 64" case_flag_without_value

case_flag_hostile_value() {
    cu_run status --bogus $'x\ny'
    cz_expect_rc 64
    cz_expect_stderr_first "cutover.sh: status: unknown argument --bogus"
    cz_check "stderr lines before the usage" "$(grep -c . "$CZ_CASE/stderr")" "6"
}
cu_case "usage: a hostile argument is printed quoted, never raw" case_flag_hostile_value

case_rollback_positional() {
    cu_run rollback now
    cz_expect_rc 64
    cz_expect_stderr_first "cutover.sh: rollback: unknown argument now"
}
cu_case "usage: a bare word after rollback is refused" case_rollback_positional

# --- the environment ---------------------------------------------------------------------------------------

cu_run_nohome() {
    local pairs=() k
    for k in "${!CZ_E[@]}"; do [ "$k" = HOME ] || pairs+=("$k=${CZ_E[$k]}"); done
    ( cd "$CU_FX" && env -i "${pairs[@]}" bash "$(cu_script)" "$@" > "$CZ_CASE/stdout" 2> "$CZ_CASE/stderr" < /dev/null )
    CZ_RC=$?
}
case_home_unset() {
    cu_run_nohome status
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr_first "cutover.sh: HOME is not set"
}
cu_case "refusal: HOME unset is refused by name" case_home_unset

case_home_newline() {
    cz_set HOME $'/tmp/a\nb'
    cu_run status
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: HOME holds a newline; refused"
}
cu_case "refusal: a HOME with a newline is refused" case_home_newline

case_home_relative() {
    cz_set HOME "relative/home"
    cu_run status
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: HOME is not an absolute path; refused"
}
cu_case "refusal: a relative HOME is refused" case_home_relative

case_home_colon() {
    cz_set HOME "/tmp/a:b"
    cu_run status
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: HOME holds a colon, which would split a PATH entry; refused"
}
cu_case "refusal: a HOME with a colon is refused" case_home_colon

case_home_pipe_every_subcommand() {
    local sub
    for sub in "build" "shadow" "live" "status" "rollback --dry-run"; do
        cz_set HOME "/tmp/a|b"
        # shellcheck disable=SC2086
        cu_run $sub
        cz_check "$sub with a pipe in HOME exits" "$CZ_RC" 1
        cz_expect_stderr_first "cutover.sh: HOME holds a '|'; refused"
        cz_expect_stdout_empty
    done
}
cu_case "refusal: a HOME with a pipe is refused by every subcommand" case_home_pipe_every_subcommand

case_home_control_character() {
    cz_set HOME $'/tmp/a\tb'
    cu_run status
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: HOME holds a control character; refused"
    cz_set HOME $'/tmp/a\x1fb'
    cu_run status
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: HOME holds a control character; refused"
}
cu_case "refusal: a HOME with a tab or a unit separator is refused" case_home_control_character

case_dir_pipe() {
    local sub
    for sub in live status; do
        cu_run "$sub" --build-dir "/tmp/a|b"
        cz_check "$sub with a pipe in the directory exits" "$CZ_RC" 1
        cz_expect_stderr_first "cutover.sh: the build directory holds a '|'; refused"
    done
    cu_fake_cargo
    cu_run build --out "$CZ_CASE/o|ut"
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: the build directory holds a '|'; refused"
    [ ! -e "$CZ_CASE/o|ut" ] || cz_note_fail "build created a directory with a pipe in its name"
}
cu_case "refusal: a build directory with a pipe is refused by build, live and status" case_dir_pipe

case_checkout_pipe() {
    mkdir -p "$CZ_CASE/fx|x"
    cp -a "$CU_FX/." "$CZ_CASE/fx|x/"
    CU_FX="$CZ_CASE/fx|x"
    cu_run status
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: the checkout path holds a '|'; refused"
}
cu_case "refusal: a checkout whose path holds a pipe is refused" case_checkout_pipe

case_single_quote_in_a_path() {
    local sub q="$CZ_CASE/q';touch PWN;'"
    mkdir -p "$q"; chmod 755 "$q"
    for sub in build shadow live status; do
        local arg=--build-dir; case "$sub" in build|shadow) arg=--out ;; esac
        cu_fake_cargo
        cu_run "$sub" "$arg" "$q"
        cz_check "$sub with a quote in the directory exits" "$CZ_RC" 1
        cz_expect_stderr_first "cutover.sh: the build directory holds a single quote; refused"
    done
    [ ! -e "$CU_HOME/PWN" ] && [ ! -e PWN ] || cz_note_fail "a command in a quoted path ran"
    cz_set HOME "/tmp/a'b"
    cu_run status
    cz_expect_stderr_first "cutover.sh: HOME holds a single quote; refused"
}
cu_case "refusal: a single quote in HOME or a build directory is refused, and no command in it runs" case_single_quote_in_a_path

case_not_a_checkout_without_rust() {
    mkdir -p "$CZ_CASE/half/scripts"
    cp "$CU_FX/scripts/cutover.sh" "$CZ_CASE/half/scripts/cutover.sh"; : > "$CZ_CASE/half/install.sh"
    CU_FX="$CZ_CASE/half"
    cu_run status
    cz_expect_rc 1
    cz_expect_stderr_has "is not a maintainer-agent checkout"
}
cu_case "refusal: a tree with install.sh and no rust/ is not a checkout" case_not_a_checkout_without_rust

case_not_a_checkout_without_install() {
    mkdir -p "$CZ_CASE/half/scripts" "$CZ_CASE/half/rust"
    cp "$CU_FX/scripts/cutover.sh" "$CZ_CASE/half/scripts/cutover.sh"
    CU_FX="$CZ_CASE/half"
    cu_run status
    cz_expect_rc 1
    cz_expect_stderr_has "is not a maintainer-agent checkout"
}
cu_case "refusal: a tree with rust/ and no install.sh is not a checkout" case_not_a_checkout_without_install

case_not_a_checkout() {
    mkdir -p "$CZ_CASE/loose/scripts"
    cp "$CU_FX/scripts/cutover.sh" "$CZ_CASE/loose/scripts/cutover.sh"
    CU_FX="$CZ_CASE/loose"
    cu_run status
    cz_expect_rc 1
    cz_expect_stderr_has "is not a maintainer-agent checkout"
}
cu_case "refusal: a copy of the script outside a checkout is refused" case_not_a_checkout

# --- the build directory -----------------------------------------------------------------------------------

case_dir_newline() {
    local sub
    for sub in live status; do
        cu_run "$sub" --build-dir $'/tmp/a\nb'
        cz_check "$sub with a newline in the directory exits" "$CZ_RC" 1
        cz_expect_stderr_first "cutover.sh: the build directory holds a newline; refused"
    done
}
cu_case "refusal: a build directory with a newline is refused by live and status" case_dir_newline

case_dir_newline_build() {
    cu_fake_cargo
    cu_run build --out $'/tmp/a\nb'
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: the build directory holds a newline; refused"
}
cu_case "refusal: build refuses a directory with a newline before it builds" case_dir_newline_build

case_dir_colon() {
    cu_run status --build-dir "/tmp/a:b"
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: the build directory holds a colon, which would split a PATH entry; refused"
}
cu_case "refusal: a build directory with a colon is refused" case_dir_colon

case_dir_symlink() {
    cu_deploy; cu_build
    ln -s "$CU_BUILD" "$CZ_CASE/link"
    local sub
    for sub in live shadow; do
        local arg=--build-dir; [ "$sub" = shadow ] && arg=--out
        cu_run "$sub" "$arg" "$CZ_CASE/link"
        cz_check "$sub through a symbolic link exits" "$CZ_RC" 1
        cz_expect_stderr_first "cutover.sh: $CZ_CASE/link is a symbolic link; the build directory must be a real directory"
    done
}
cu_case "refusal: a build directory that is a symbolic link is refused" case_dir_symlink

case_dir_world_writable() {
    cu_deploy; cu_build
    chmod 777 "$CU_BUILD"
    cu_run live --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: $CU_BUILD is writable by everyone (mode 777); refused"
}
cu_case "refusal: a world-writable build directory is refused" case_dir_world_writable

case_dir_world_writable_sticky() {
    cu_deploy; cu_build
    chmod 1777 "$CU_BUILD"
    cu_run shadow --out "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: $CU_BUILD is writable by everyone (mode 1777); refused"
}
cu_case "refusal: a sticky world-writable build directory is refused too" case_dir_world_writable_sticky

case_dir_writable_by_others_only() {
    cu_deploy; cu_build
    chmod 772 "$CU_BUILD"
    cu_run live --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: $CU_BUILD is writable by everyone (mode 772); refused"
    chmod 770 "$CU_BUILD"
    cu_run live --build-dir "$CU_BUILD"
    cz_expect_stderr_lacks_writable
}
cz_expect_stderr_lacks_writable() { cz_expect_lacks stderr "writable by everyone"; }
cu_case "refusal: a build directory writable by others but not readable (mode 772) is refused as writable" case_dir_writable_by_others_only

case_dir_group_writable_ok() {
    cu_deploy; cu_build
    chmod 775 "$CU_BUILD"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 0
}
cu_case "usage: a group-writable build directory is not refused by status" case_dir_group_writable_ok

# --- tools ---------------------------------------------------------------------------------------------------

case_missing_tool_status() {
    cu_deploy
    rm -f "$CU_TOOLS/sha256sum"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr_first "cutover.sh: the tool 'sha256sum' is not on PATH; nothing was done"
}
cu_case "refusal: status without sha256sum refuses by name" case_missing_tool_status

case_missing_tool_build() {
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: the tool 'cargo' is not on PATH; nothing was done"
    [ ! -e "$CU_BUILD" ] || cz_note_fail "build created its directory before it found a tool missing"
}
cu_case "refusal: build without cargo refuses by name and creates nothing" case_missing_tool_build

case_missing_tool_rollback() {
    cu_deploy
    rm -f "$CU_TOOLS/flock"
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: flock is required to install and protect active runs; install flock and ensure it is on PATH."
}
cu_case "refusal: rollback without flock refuses by name" case_missing_tool_rollback

case_refusal_writes_nothing() {
    cu_deploy; cu_build
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"; cu_snap "$CU_BUILD" > "$CZ_CASE/build.before"
    cu_snap "$CU_FX" > "$CZ_CASE/fx.before"
    chmod 777 "$CU_BUILD"
    cu_run live --build-dir "$CU_BUILD"
    cu_run status --build-dir "$CZ_CASE/none:colon"
    chmod 755 "$CU_BUILD"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"; cu_snap "$CU_BUILD" > "$CZ_CASE/build.after"
    cu_snap "$CU_FX" > "$CZ_CASE/fx.after"
    cu_expect_same_snap "home" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
    cu_expect_same_snap "build directory" "$CZ_CASE/build.before" "$CZ_CASE/build.after"
    cu_expect_same_snap "checkout" "$CZ_CASE/fx.before" "$CZ_CASE/fx.after"
}
cu_case "refusal: a refused run leaves HOME, the build directory and the checkout as they were" case_refusal_writes_nothing

cz_finish
