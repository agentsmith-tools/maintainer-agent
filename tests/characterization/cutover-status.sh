#!/usr/bin/env bash
# Characterization: scripts/cutover.sh status. Which of the six programs the Rust install swaps
# are bash and which are Rust on this machine, judged by sha256 against RUST_STAMP, and whether
# shadow-ok still describes the build directory. It reads, never writes.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib-cutover.sh"
cu_init

swapped() { cu_deploy; cu_build $CU_PROGRAMS; cu_swap; }
programs_block() {  # prints the six lines of the table, from a state per program
    printf '  %-18s %-8s %s\n' maintainer-merge "$1" "${7:-}" maintainer-doctor "$2" "${8:-}" maintainer-repo "$3" "${9:-}" \
        run "$4" "${10:-}" run-instance "$5" "${11:-}" alert "$6" "${12:-}"
}
HEAD1() { printf 'programs on this machine (%s):' "$CU_HOME"; }

case_all_bash() {
    cu_deploy; cu_build
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 0
    cz_expect_stdout "$(HEAD1)" "$(programs_block bash bash bash bash bash bash)" "RUST_STAMP: absent" "shadow-ok: absent ($CU_BUILD)"
}
cu_case "status: a machine with no RUST_STAMP shows six bash programs" case_all_bash

case_all_rust() {
    swapped
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 0
    cz_expect_stdout "$(HEAD1)" \
        "$(programs_block rust rust rust rust rust rust 'matches RUST_STAMP' 'matches RUST_STAMP' 'matches RUST_STAMP' 'matches RUST_STAMP' 'matches RUST_STAMP' 'matches RUST_STAMP')" \
        "RUST_STAMP: present, 6 program(s)" "shadow-ok: absent ($CU_BUILD)"
}
cu_case "status: after a full Rust install each program is rust and matches the stamp" case_all_rust

case_mixed() {
    cu_deploy; cu_build $CU_PROGRAMS
    cu_swap maintainer-doctor run
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 0
    cz_expect_stdout_has "  maintainer-merge   bash"
    cz_expect_stdout_has "  maintainer-doctor  rust     matches RUST_STAMP"
    cz_expect_stdout_has "  run                rust     matches RUST_STAMP"
    cz_expect_stdout_has "  alert              bash"
    cz_expect_stdout_has "RUST_STAMP: present, 2 program(s)"
}
cu_case "status: a partial install shows the swapped programs as rust and the others as bash" case_mixed

case_changed() {
    swapped
    printf 'someone edited this\n' > "$CU_BIN/maintainer-repo"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 0
    cz_expect_stdout_has "  maintainer-repo    changed  listed in RUST_STAMP and no longer the stamped build"
    cz_expect_stdout_has "  maintainer-merge   rust     matches RUST_STAMP"
}
cu_case "status: a stamped program whose bytes changed is reported as changed" case_changed

case_bash_with_stamp_of_others() {
    cu_deploy; cu_build $CU_PROGRAMS
    cu_swap run
    # a bash program that was never swapped keeps its hash: it must not read as rust
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_stdout_has "  maintainer-merge   bash"
}
cu_case "status: a program the stamp does not list is bash even beside a stamped one" case_bash_with_stamp_of_others

case_link() {
    swapped
    cp "$CU_BIN/maintainer-merge" "$CZ_CASE/elsewhere"; rm "$CU_BIN/maintainer-merge"; ln -s "$CZ_CASE/elsewhere" "$CU_BIN/maintainer-merge"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 0
    cz_expect_stdout_has "  maintainer-merge   link     a symbolic link, not read"
}
cu_case "status: a link where a program sits is reported and not followed" case_link

case_missing() {
    swapped
    rm "$CU_SHARE/alert.sh"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 0
    cz_expect_stdout_has "  alert              missing"
}
cu_case "status: a missing program is reported as missing" case_missing

case_stamp_unreadable() {
    swapped
    rm "$CU_SHARE/RUST_STAMP"; mkdir "$CU_SHARE/RUST_STAMP"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr_first "cutover.sh: $CU_SHARE/RUST_STAMP is not a regular file; refused"
}
cu_case "status: a RUST_STAMP that cannot be read is refused by name" case_stamp_unreadable

case_stamp_link() {
    swapped
    mv "$CU_SHARE/RUST_STAMP" "$CZ_CASE/stamp"; ln -s "$CZ_CASE/stamp" "$CU_SHARE/RUST_STAMP"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: $CU_SHARE/RUST_STAMP is a symbolic link; refused"
}
cu_case "status: a RUST_STAMP that is a link is refused" case_stamp_link

case_stamp_malformed() {
    swapped
    printf 'garbage\n' > "$CU_SHARE/RUST_STAMP"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr_first 'cutover.sh: RUST_STAMP has a line that is not `<sha256>  <path>`: garbage'
}
cu_case "status: a malformed RUST_STAMP is refused and quoted" case_stamp_malformed

case_stamp_other_home() {
    swapped
    sed -i "s|$CU_HOME|/other/home|" "$CU_SHARE/RUST_STAMP"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_has "which is not a program this installer swaps"
}
cu_case "status: a stamp written for another HOME is refused" case_stamp_other_home

case_marker_current() {
    cu_deploy; cu_build
    cu_table > "$CU_BUILD/shadow-ok"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 0
    cz_expect_stdout_has "shadow-ok: current (the marker lists the programs now in $CU_BUILD)"
}
cu_case "status: shadow-ok that lists the programs now in the directory is current" case_marker_current

case_marker_with_comment_current() {
    cu_deploy; cu_build
    { cu_table; echo '# accepted-skip: maintainer-merge'; } > "$CU_BUILD/shadow-ok"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_stdout_has "shadow-ok: current"
}
cu_case "status: comment lines of the marker do not make it stale" case_marker_with_comment_current

case_marker_stale() {
    cu_deploy; cu_build
    cu_table > "$CU_BUILD/shadow-ok"
    printf 'a newer build\n' >> "$CU_BUILD/run"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 0
    cz_expect_stdout_has "shadow-ok: stale (the programs in $CU_BUILD are not the ones shadow compared)"
}
cu_case "status: shadow-ok is stale when a program changed after shadow" case_marker_stale

case_marker_stale_extra_program() {
    cu_deploy; cu_build
    cu_table > "$CU_BUILD/shadow-ok"
    cp "$CU_BUILD/run" "$CU_BUILD/maintainer-extra"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_stdout_has "shadow-ok: stale"
}
cu_case "status: shadow-ok is stale when a program was added after shadow" case_marker_stale_extra_program

case_marker_stale_removed_program() {
    cu_deploy; cu_build
    cu_table > "$CU_BUILD/shadow-ok"
    rm "$CU_BUILD/alert"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_stdout_has "shadow-ok: stale"
}
cu_case "status: shadow-ok is stale when a program was removed after shadow" case_marker_stale_removed_program

case_no_build_dir() {
    cu_deploy
    cu_run status --build-dir "$CZ_CASE/nothing-here"
    cz_expect_rc 0
    cz_expect_stdout_has "shadow-ok: absent (no build directory $CZ_CASE/nothing-here)"
}
cu_case "status: a build directory that does not exist is reported, not refused" case_no_build_dir

case_default_dir() {
    cu_deploy
    cu_run status
    cz_expect_rc 0
    cz_expect_stdout_has "shadow-ok: absent (no build directory $CU_HOME/.cache/magent-cutover/build)"
}
cu_case "status: with no --build-dir it looks in ~/.cache/magent-cutover/build" case_default_dir

case_xdg_default_dir() {
    cu_deploy
    cz_set XDG_CACHE_HOME "$CZ_CASE/xdg"
    cu_run status
    cz_expect_stdout_has "(no build directory $CZ_CASE/xdg/magent-cutover/build)"
}
cu_case "status: XDG_CACHE_HOME moves the default build directory" case_xdg_default_dir

case_marker_link() {
    cu_deploy; cu_build
    cu_table > "$CZ_CASE/marker"; ln -s "$CZ_CASE/marker" "$CU_BUILD/shadow-ok"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 0
    cz_expect_stdout_has "shadow-ok: absent ($CU_BUILD)"
}
cu_case "status: a shadow-ok that is a link does not count" case_marker_link

case_writes_nothing() {
    swapped
    cu_table > "$CU_BUILD/shadow-ok"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"; cu_snap "$CU_BUILD" > "$CZ_CASE/build.before"
    cu_snap "$CU_FX" > "$CZ_CASE/fx.before"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 0
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"; cu_snap "$CU_BUILD" > "$CZ_CASE/build.after"
    cu_snap "$CU_FX" > "$CZ_CASE/fx.after"
    cu_expect_same_snap "home" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
    cu_expect_same_snap "build directory" "$CZ_CASE/build.before" "$CZ_CASE/build.after"
    cu_expect_same_snap "checkout" "$CZ_CASE/fx.before" "$CZ_CASE/fx.after"
}
cu_case "status: it writes nothing to HOME, the build directory or the checkout" case_writes_nothing

case_status_needs_no_rust() {
    swapped
    rm -rf "$CU_BUILD"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_rc 0
    cz_expect_stdout_has "  run                rust     matches RUST_STAMP"
}
cu_case "status: it needs no build directory to say what is installed" case_status_needs_no_rust

cz_finish
