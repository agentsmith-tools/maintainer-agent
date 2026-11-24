#!/usr/bin/env bash
# Characterization: scripts/cutover.sh rollback. It puts the saved bash originals back and
# removes RUST_STAMP, from the stamp alone: no Rust binary, no build directory, no cargo.
#
# The expected text of every case is what `install --rollback` prints for the same state, with
# the program's own name in front of a refusal. rust/crates/mx-install/tests/cutover-rollback.sh
# runs the same states through the Rust install and compares the two: that is the proof that
# the two ways back are one way. A case marked "loose" there compares the exit status and the
# files only, because the words of an operating-system error are not the same in both.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib-cutover.sh"
cu_init

# A deployed machine, a build, and a full Rust install on top; returns with the machine as
# it was before the swap in $CZ_CASE/home.plain, for comparison.
swapped() {
    cu_deploy; cu_build $CU_PROGRAMS
    cu_snap "$CU_HOME" > "$CZ_CASE/home.plain"
    cu_swap
}
STAMP() { printf '%s' "$CU_SHARE/RUST_STAMP"; }
rolling() { printf '  rolling back from %s' "$CU_SHARE"; }

case_all_swapped() {
    swapped
    cu_run rollback
    cz_expect_rc 0
    cz_expect_stdout "$(rolling)" "  rolled back 6 program(s) to the bash originals"
    cz_expect_stderr_empty
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine after the rollback against the machine before the swap" "$CZ_CASE/home.plain" "$CZ_CASE/home.after"
}
cu_case "rollback: a full Rust install is undone file for file and RUST_STAMP is removed" case_all_swapped

case_dry_run() {
    swapped
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_run rollback --dry-run
    cz_expect_rc 0
    cz_expect_stdout "$(rolling)" \
        "  would: mv $CU_BIN/maintainer-merge.sh $CU_BIN/maintainer-merge" \
        "  would: mv $CU_BIN/maintainer-doctor.sh $CU_BIN/maintainer-doctor" \
        "  would: mv $CU_BIN/maintainer-repo.sh $CU_BIN/maintainer-repo" \
        "  would: mv $CU_SHARE/run.sh.sh $CU_SHARE/run.sh" \
        "  would: mv $CU_SHARE/run-instance.sh.sh $CU_SHARE/run-instance.sh" \
        "  would: mv $CU_SHARE/alert.sh.sh $CU_SHARE/alert.sh" \
        "  would: rm -f $(STAMP)" \
        "  rolled back 6 program(s) to the bash originals"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine after a dry run" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
}
cu_case "rollback: --dry-run prints each move in stamp order and changes nothing" case_dry_run

case_no_stamp() {
    cu_deploy
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr "cutover.sh: nothing to roll back: $(STAMP) does not exist"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
}
cu_case "rollback: with no RUST_STAMP it refuses and changes nothing" case_no_stamp

case_twice() {
    swapped
    cu_run rollback
    cz_expect_rc 0
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stderr "cutover.sh: nothing to roll back: $(STAMP) does not exist"
}
cu_case "rollback: a second rollback finds nothing to do and refuses" case_twice

case_no_rust_no_cargo() {
    swapped
    rm -rf "$CU_BUILD"
    printf 'not a program\n' > "$CU_FX/install.sh"
    cu_run rollback
    cz_expect_rc 0
    cz_expect_stdout "$(rolling)" "  rolled back 6 program(s) to the bash originals"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.plain" "$CZ_CASE/home.after"
}
cu_case "rollback: works with no Rust build, no cargo and an install.sh that is not a program" case_no_rust_no_cargo

case_half_swapped() {
    cu_deploy; cu_build $CU_PROGRAMS
    cu_snap "$CU_HOME" > "$CZ_CASE/home.plain"
    cu_stamp $CU_PROGRAMS
    cu_swap_files maintainer-merge maintainer-doctor maintainer-repo
    cu_run rollback
    cz_expect_rc 0
    cz_expect_stdout "$(rolling)" "  rolled back 3 program(s) to the bash originals"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.plain" "$CZ_CASE/home.after"
}
cu_case "rollback: a stamp that lists six programs of which three were swapped restores three" case_half_swapped

case_swap_never_started() {
    cu_deploy; cu_build $CU_PROGRAMS
    cu_snap "$CU_HOME" > "$CZ_CASE/home.plain"
    cu_stamp $CU_PROGRAMS
    # the copy of the bash original was made and the Rust build was never installed over it
    local p="$CU_BIN/maintainer-repo"; cp -p "$p" "$p.sh"
    cu_run rollback
    cz_expect_rc 0
    cz_expect_stdout "$(rolling)" "  rolled back 0 program(s) to the bash originals"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.plain" "$CZ_CASE/home.after"
}
cu_case "rollback: a saved copy of a program whose swap never began is dropped" case_swap_never_started

case_missing_backup() {
    swapped
    rm "$CU_BIN/maintainer-repo.sh"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr "cutover.sh: cannot roll back: $CU_BIN/maintainer-repo.sh is missing"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine (nothing moves when one original is gone)" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
}
cu_case "rollback: a missing saved original refuses before it moves anything" case_missing_backup

case_changed_binary() {
    swapped
    printf 'a newer build\n' > "$CU_SHARE/run.sh"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr "cutover.sh: cannot roll back: $CU_SHARE/run.sh no longer matches the stamp, so it is not the build this installer wrote"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
}
cu_case "rollback: an installed file that is no longer the stamped build refuses and moves nothing" case_changed_binary

case_restored_already() {
    swapped
    # a rollback that failed after restoring alert.sh: the original is back, its copy is gone
    mv "$CU_SHARE/alert.sh.sh" "$CU_SHARE/alert.sh"
    cu_run rollback
    cz_expect_rc 0
    cz_expect_stdout "$(rolling)" "  rolled back 5 program(s) to the bash originals"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.plain" "$CZ_CASE/home.after"
}
cu_case "rollback: a program restored by an earlier, interrupted rollback is skipped" case_restored_already

case_part_way_failure_then_retry() {
    swapped
    local mode; mode="$(stat -c %a "$CU_SHARE")"
    chmod 555 "$CU_SHARE"
    cu_run rollback
    local rc="$CZ_RC"
    chmod "$mode" "$CU_SHARE"
    [ "$rc" != 0 ] || cz_note_fail "a rollback that cannot move into a read-only directory exited 0"
    [ -f "$(STAMP)" ] || cz_note_fail "the stamp was removed although the rollback failed"
    cz_check "the bin programs restored before the failure" "$([ ! -e "$CU_BIN/maintainer-repo.sh" ] && echo restored)" restored
    cu_run rollback
    cz_expect_rc 0
    cz_expect_stdout "$(rolling)" "  rolled back 3 program(s) to the bash originals"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine after the retry" "$CZ_CASE/home.plain" "$CZ_CASE/home.after"
}
cu_case "rollback: a rollback that fails part way can run again and finishes" case_part_way_failure_then_retry

case_uppercase_hash() {
    swapped
    local up; up="$(tr 'a-f' 'A-F' < "$(STAMP)")"
    # only the hash is upper-cased: the path stays as it is
    : > "$(STAMP)"
    local n
    for n in $CU_PROGRAMS; do printf '%s  %s\n' "$(sha256sum < "$CU_BUILD/$n" | cut -d' ' -f1 | tr 'a-f' 'A-F')" "$(cu_installed "$n")" >> "$(STAMP)"; done
    [ -n "$up" ] || cz_note_fail "no stamp text"
    cu_run rollback
    cz_expect_rc 0
    cz_expect_stdout "$(rolling)" "  rolled back 6 program(s) to the bash originals"
}
cu_case "rollback: a stamp with upper-case hex is read as the same hash" case_uppercase_hash

case_unterminated_last_line() {
    swapped
    printf '%s' "$(cat "$(STAMP)")" > "$CZ_CASE/stamp.new"; mv "$CZ_CASE/stamp.new" "$(STAMP)"
    cu_run rollback
    cz_expect_rc 0
    cz_expect_stdout "$(rolling)" "  rolled back 6 program(s) to the bash originals"
}
cu_case "rollback: a stamp whose last line has no newline is read in full" case_unterminated_last_line

# --- a stamp that is not one -----------------------------------------------------------------------------------

stamp_refused() {  # $1 = the stamp text, $2 = the message after "cannot roll back: "
    swapped
    printf '%s' "$1" > "$(STAMP)"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr "cutover.sh: cannot roll back: $2"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
}

case_stamp_bad_line() {
    stamp_refused $'this is not a stamp\n' 'RUST_STAMP has a line that is not `<sha256>  <path>`: this is not a stamp'
}
cu_case "rollback: a stamp line that is not <sha256>  <path> is refused and quoted" case_stamp_bad_line

case_stamp_short_hash() {
    stamp_refused $'abc123  /x/y\n' 'RUST_STAMP has a line that is not `<sha256>  <path>`: abc123  /x/y'
}
cu_case "rollback: a stamp line with a short hash is refused" case_stamp_short_hash

case_stamp_one_space() {
    local h; h="$(printf 'a%.0s' $(seq 1 64))"
    stamp_refused "$h /x/y"$'\n' "RUST_STAMP has a line that is not \`<sha256>  <path>\`: $h /x/y"
}
cu_case "rollback: a stamp line with one space after the hash is refused" case_stamp_one_space

case_stamp_other_program() {
    local h; h="$(printf 'a%.0s' $(seq 1 64))"
    stamp_refused "$h  $CU_HOME/.local/bin/maintainer"$'\n' "RUST_STAMP lists $CU_HOME/.local/bin/maintainer, which is not a program this installer swaps"
}
cu_case "rollback: a stamp that names a program the installer does not swap is refused" case_stamp_other_program

case_stamp_other_home() {
    local h; h="$(printf 'a%.0s' $(seq 1 64))"
    stamp_refused "$h  /other/home/.local/bin/maintainer-merge"$'\n' "RUST_STAMP lists /other/home/.local/bin/maintainer-merge, which is not a program this installer swaps"
}
cu_case "rollback: a stamp written for another HOME is refused" case_stamp_other_home

case_stamp_nul() {
    swapped
    printf '%s\0x\n' "$(head -c 64 "$(STAMP)")" > "$(STAMP)"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr "cutover.sh: cannot roll back: RUST_STAMP holds a NUL byte"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
}
cu_case "rollback: a stamp with a NUL byte is refused by name and nothing moves" case_stamp_nul

case_stamp_empty() {
    stamp_refused "" "RUST_STAMP lists no program"
}
cu_case "rollback: an empty stamp is refused" case_stamp_empty

case_stamp_blank_lines() {
    stamp_refused $'\n\n' "RUST_STAMP lists no program"
}
cu_case "rollback: a stamp of blank lines lists no program" case_stamp_blank_lines

case_stamp_duplicate() {
    swapped
    local first; first="$(head -1 "$(STAMP)")"
    printf '%s\n' "$first" >> "$(STAMP)"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stderr "cutover.sh: cannot roll back: RUST_STAMP lists $CU_BIN/maintainer-merge twice"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
}
cu_case "rollback: a program listed twice refuses before it moves anything" case_stamp_duplicate

case_stamp_is_directory() {
    swapped
    rm "$(STAMP)"; mkdir "$(STAMP)"
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr_has "cutover.sh: cannot roll back: $(STAMP) cannot be read"
}
cu_case "rollback: a RUST_STAMP that is a directory is refused as unreadable" case_stamp_is_directory

# --- links -------------------------------------------------------------------------------------------------------

LINKMSG() { printf 'cutover.sh: cannot roll back: %s is a symbolic link; rollback never follows or replaces one' "$1"; }

case_link_installed() {
    swapped
    cp "$CU_BIN/maintainer-doctor" "$CZ_CASE/copy-of-the-build"
    rm "$CU_BIN/maintainer-doctor"; ln -s "$CZ_CASE/copy-of-the-build" "$CU_BIN/maintainer-doctor"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"; cu_snap "$CZ_CASE/copy-of-the-build" > /dev/null 2>&1 || true
    local before; before="$(sha256sum < "$CZ_CASE/copy-of-the-build")"
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr "$(LINKMSG "$CU_BIN/maintainer-doctor")"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
    cz_check "what the link points at" "$(sha256sum < "$CZ_CASE/copy-of-the-build")" "$before"
}
cu_case "rollback: a link planted where an installed program sits is refused and left alone" case_link_installed

case_link_installed_elsewhere() {
    swapped
    printf 'other\n' > "$CZ_CASE/other"
    rm "$CU_BIN/maintainer-doctor"; ln -s "$CZ_CASE/other" "$CU_BIN/maintainer-doctor"
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stderr "$(LINKMSG "$CU_BIN/maintainer-doctor")"
    cz_check "the file the link points at" "$(cat "$CZ_CASE/other")" other
}
cu_case "rollback: a link to some other file at an installed program is refused" case_link_installed_elsewhere

case_link_backup() {
    swapped
    mv "$CU_BIN/maintainer-repo.sh" "$CZ_CASE/real-original"
    ln -s "$CZ_CASE/real-original" "$CU_BIN/maintainer-repo.sh"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr "$(LINKMSG "$CU_BIN/maintainer-repo.sh")"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
}
cu_case "rollback: a link planted where a saved original sits is refused, not moved into place" case_link_backup

case_link_backup_dangling() {
    swapped
    rm "$CU_BIN/maintainer-repo.sh"; ln -s /nonexistent/original "$CU_BIN/maintainer-repo.sh"
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stderr "$(LINKMSG "$CU_BIN/maintainer-repo.sh")"
}
cu_case "rollback: a dangling link at a saved original is refused" case_link_backup_dangling

case_link_stamp() {
    swapped
    mv "$(STAMP)" "$CZ_CASE/stamp-copy"; ln -s "$CZ_CASE/stamp-copy" "$(STAMP)"
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stderr "cutover.sh: cannot roll back: $(STAMP) is a symbolic link; rollback never follows or replaces one"
    [ -f "$CZ_CASE/stamp-copy" ] || cz_note_fail "the file the stamp link pointed at was removed"
}
cu_case "rollback: a RUST_STAMP that is a link is refused and its target is kept" case_link_stamp

case_link_in_unlisted_program() {
    # a link at a program the stamp does not list is none of rollback's business
    cu_deploy; cu_build $CU_PROGRAMS
    cu_stamp maintainer-doctor
    cu_swap_files maintainer-doctor
    rm "$CU_BIN/maintainer-repo"; ln -s "$CU_FX/bin/maintainer-repo" "$CU_BIN/maintainer-repo"
    cu_run rollback
    cz_expect_rc 0
    cz_expect_stdout "$(rolling)" "  rolled back 1 program(s) to the bash originals"
    [ -L "$CU_BIN/maintainer-repo" ] || cz_note_fail "a link at a program the stamp does not list was replaced"
}
cu_case "rollback: a link at a program the stamp does not list is left as it is" case_link_in_unlisted_program

# --- a run in flight ---------------------------------------------------------------------------------------------

hold_lock() {  # $1 = lock file: holds it for 8 s from a process of its own
    mkdir -p "$(dirname "$1")"
    ( exec 9> "$1"; flock -n 9 || exit 3; : > "$CZ_CASE/lock-ready"; exec sleep 8 ) &
    CU_HOLDER=$!
    for _ in $(seq 1 50); do [ -e "$CZ_CASE/lock-ready" ] && break; sleep 0.1; done
    [ "$(readlink "/proc/$CU_HOLDER/exe")" = "$(readlink -f "$(type -P sleep)")" ] || cz_note_fail "the lock holder is not the sleep that holds it"
}
release_lock() { kill "$CU_HOLDER" 2>/dev/null; wait "$CU_HOLDER" 2>/dev/null; }

case_busy() {
    swapped
    local state="$CU_HOME/.local/state/alpha-maint"
    hold_lock "$state/run.lock"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_run rollback
    release_lock
    cz_expect_rc 69
    cz_expect_stdout_empty
    cz_expect_stderr_first "cutover.sh: a run is in flight for: alpha"
    cz_expect_stderr_has "Wait for it (maintainer status), or --force if you are sure."
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
}
cu_case "rollback: a profile whose run lock is held refuses with 69 and changes nothing" case_busy

case_busy_force() {
    swapped
    local state="$CU_HOME/.local/state/alpha-maint"
    hold_lock "$state/run.lock"
    cu_run rollback --force
    release_lock
    cz_expect_rc 0
    cz_expect_stdout "$(rolling)" "  rolled back 6 program(s) to the bash originals"
}
cu_case "rollback: --force goes on past a held run lock" case_busy_force

case_busy_dry_run() {
    swapped
    hold_lock "$CU_HOME/.local/state/alpha-maint/run.lock"
    cu_run rollback --dry-run
    release_lock
    cz_expect_rc 0
    cz_expect_stdout_has "  would: rm -f $(STAMP)"
}
cu_case "rollback: --dry-run does not ask for the lock" case_busy_dry_run

case_lock_file_free() {
    swapped
    mkdir -p "$CU_HOME/.local/state/alpha-maint"; : > "$CU_HOME/.local/state/alpha-maint/run.lock"
    cu_run rollback
    cz_expect_rc 0
}
cu_case "rollback: a lock file nobody holds is not a run in flight" case_lock_file_free

case_home_with_pipe() {
    cu_home_at "$CZ_CASE/ho|me"
    cu_deploy; cu_build $CU_PROGRAMS
    cu_stamp $CU_PROGRAMS
    cu_swap_files maintainer-merge maintainer-doctor maintainer-repo run run-instance
    # a swap that never began: a rollback drops the saved copy of alert.sh
    cp -p "$(cu_installed alert)" "$(cu_installed alert).sh"
    printf 'decoy\n' > "$CZ_CASE/ho"
    cu_run rollback
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr "cutover.sh: HOME holds a '|'; refused"
    cz_check "the file a path cut at the pipe would name" "$(cat "$CZ_CASE/ho" 2>/dev/null)" decoy
    cz_check "the saved copy of alert.sh" "$([ -f "$CU_SHARE/alert.sh.sh" ] && echo kept)" kept
}
cu_case "rollback: a HOME with a pipe is refused by name before a step can be cut at it" case_home_with_pipe

case_flock_missing_leaves_machine() {
    swapped
    rm -f "$CU_TOOLS/flock"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_run rollback
    cz_expect_rc 1
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
}
cu_case "rollback: with no flock it refuses and the machine is as it was" case_flock_missing_leaves_machine

cz_finish
