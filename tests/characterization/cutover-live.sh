#!/usr/bin/env bash
# Characterization: scripts/cutover.sh live. It refuses unless shadow-ok describes the build in the
# directory and the build is the checkout's commit, shows the dry run of `install --rust`, runs the
# real one, and reads the machine back. The installer is a stand-in that leaves the same files the
# Rust installer leaves and can be made to go wrong in one named way; the real installer is
# exercised by rust/crates/mx-install/tests/cutover-live.sh.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib-cutover.sh"
cu_init

# A machine with the bash programs deployed, a build directory (with the fake installer) and a marker.
ready() {
    cu_deploy; cu_build $CU_PROGRAMS install
    cu_fake_install
    cu_marker
}
installs() { cat "$CZ_CASE/install.calls" 2>/dev/null; }

case_happy() {
    ready
    cu_run live --build-dir "$CU_BUILD"
    cz_expect_rc 0
    cz_expect_stdout "live: dry run of the install (nothing changes)" \
        "  would: install the Rust builds from $CU_BUILD" \
        "live: installing" \
        "  rust: maintainer-merge is the Rust build" \
        "  rust: maintainer-doctor is the Rust build" \
        "  rust: maintainer-repo is the Rust build" \
        "  rust: run is the Rust build" \
        "  rust: run-instance is the Rust build" \
        "  rust: alert is the Rust build" \
        "live: read back 6 program(s): each is the Rust build, each bash original is intact, RUST_STAMP lists exactly those"
    cz_expect_stderr_empty
}
cu_case "live: a marked build is installed, shown first as a dry run, and read back" case_happy

case_install_arguments() {
    ready
    cu_run live --build-dir "$CU_BUILD"
    cz_check "the calls to install" "$(installs)" "--rust --rust-bin $CU_BUILD --dry-run
--rust --rust-bin $CU_BUILD"
}
cu_case "live: install runs twice, --dry-run first, with --rust --rust-bin DIR and nothing else" case_install_arguments

case_status_after_live() {
    ready
    cu_run live --build-dir "$CU_BUILD"
    cu_run status --build-dir "$CU_BUILD"
    cz_expect_stdout_has "  maintainer-merge   rust     matches RUST_STAMP"
    cz_expect_stdout_has "  alert              rust     matches RUST_STAMP"
    cz_expect_stdout_has "shadow-ok: current"
}
cu_case "live: status afterwards shows the programs as rust and shadow-ok as current" case_status_after_live

case_round_trip() {
    ready
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_run live --build-dir "$CU_BUILD"; cz_expect_rc 0
    cu_run rollback; cz_expect_rc 0
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine after live and rollback" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
}
cu_case "live: rollback after live leaves the machine as it was before live" case_round_trip

# --- what it refuses first ---------------------------------------------------------------------------------------

refused_before_install() {  # $1 = the first stderr line expected
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_run live --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr_first "$1"
    [ ! -s "$CZ_CASE/install.calls" ] || cz_note_fail "install was run: $(installs | head -1)"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
}

case_no_marker() {
    ready; rm "$CU_BUILD/shadow-ok"
    refused_before_install "cutover.sh: no shadow-ok in $CU_BUILD: run scripts/cutover.sh shadow on this build first"
}
cu_case "live: without shadow-ok it refuses and runs nothing" case_no_marker

case_marker_link() {
    ready; mv "$CU_BUILD/shadow-ok" "$CZ_CASE/marker"; ln -s "$CZ_CASE/marker" "$CU_BUILD/shadow-ok"
    refused_before_install "cutover.sh: no shadow-ok in $CU_BUILD: run scripts/cutover.sh shadow on this build first"
}
cu_case "live: a shadow-ok that is a link is not a marker" case_marker_link

case_marker_empty() {
    ready; : > "$CU_BUILD/shadow-ok"
    refused_before_install "cutover.sh: $CU_BUILD/shadow-ok holds no table"
}
cu_case "live: an empty shadow-ok is refused" case_marker_empty

case_marker_only_comments() {
    ready; printf '# accepted-skip: maintainer-merge\n' > "$CU_BUILD/shadow-ok"
    refused_before_install "cutover.sh: $CU_BUILD/shadow-ok holds no table"
}
cu_case "live: a shadow-ok of comments only holds no table" case_marker_only_comments

case_changed_after_shadow() {
    ready; printf 'one more byte' >> "$CU_BUILD/run"
    refused_before_install "cutover.sh: the programs in $CU_BUILD are not the ones shadow compared (shadow-ok lists another table); run shadow again"
}
cu_case "live: a program that changed after shadow refuses (the sha256 compare)" case_changed_after_shadow

case_changed_one_byte_same_size() {
    ready
    local before; before="$(wc -c < "$CU_BUILD/run")"
    sed -i '1s/^#/@/' "$CU_BUILD/run"
    cz_check "the size did not change" "$(wc -c < "$CU_BUILD/run")" "$before"
    refused_before_install "cutover.sh: the programs in $CU_BUILD are not the ones shadow compared (shadow-ok lists another table); run shadow again"
}
cu_case "live: a program changed by one byte, same size, still refuses" case_changed_one_byte_same_size

case_added_after_shadow() {
    ready; cp "$CU_BUILD/run" "$CU_BUILD/maintainer-extra"
    refused_before_install "cutover.sh: the programs in $CU_BUILD are not the ones shadow compared (shadow-ok lists another table); run shadow again"
}
cu_case "live: a program added after shadow refuses" case_added_after_shadow

case_removed_after_shadow() {
    ready; rm "$CU_BUILD/alert"
    refused_before_install "cutover.sh: the programs in $CU_BUILD are not the ones shadow compared (shadow-ok lists another table); run shadow again"
}
cu_case "live: a program removed after shadow refuses" case_removed_after_shadow

case_marker_for_other_table() {
    ready
    printf '%s  run\n' "$(printf 'a%.0s' $(seq 1 64))" > "$CU_BUILD/shadow-ok"
    refused_before_install "cutover.sh: the programs in $CU_BUILD are not the ones shadow compared (shadow-ok lists another table); run shadow again"
}
cu_case "live: a shadow-ok that lists some other table refuses" case_marker_for_other_table

reseal() {  # the marker names the report as it is now
    sed -i "s/^# report: .*/# report: $(sha256sum < "$CU_BUILD/shadow-report.txt" | cut -d' ' -f1)/" "$CU_BUILD/shadow-ok"
}

case_forged_marker() {
    ready; cp "$CU_BUILD/SHA256SUMS" "$CU_BUILD/shadow-ok"; rm -f "$CU_BUILD/shadow-report.txt"
    refused_before_install "cutover.sh: shadow-ok was made for the checkout at  and the checkout is at $(cu_head | cut -c1-12); run shadow again"
}
cu_case "live: a shadow-ok made by copying SHA256SUMS is refused, because shadow never ran" case_forged_marker

case_report_edited() {
    ready; echo "PASS one more" >> "$CU_BUILD/shadow-report.txt"
    refused_before_install "cutover.sh: shadow-report.txt is not the report shadow-ok was made from"
}
cu_case "live: a shadow report that changed after the marker was made is refused" case_report_edited

case_report_missing() {
    ready; rm "$CU_BUILD/shadow-report.txt"
    refused_before_install "cutover.sh: there is no shadow-report.txt beside shadow-ok"
}
cu_case "live: a marker with no report beside it is refused" case_report_missing

case_report_with_diff() {
    ready; sed -i 's/^SUMMARY.*/DIFF run x: stdout line 1\nSUMMARY comparisons=3 pass=2 diff=1 skip=0/' "$CU_BUILD/shadow-report.txt"; reseal
    refused_before_install "cutover.sh: the shadow report has 1 DIFF line(s)"
}
cu_case "live: a report with a DIFF line is refused even when the marker names it" case_report_with_diff

case_report_without_comparisons() {
    ready; printf 'SHADOW home=x checkout=%s deployed=%s build=x profiles=1\nSUMMARY comparisons=0 pass=0 diff=0 skip=0\n' "$(cu_head | cut -c1-7)" "$(cu_head | cut -c1-7)" > "$CU_BUILD/shadow-report.txt"; reseal
    refused_before_install "cutover.sh: the shadow report holds no comparison"
}
cu_case "live: a report with no comparison in it is refused" case_report_without_comparisons

case_summary_disagrees() {
    ready; sed -i 's/^SUMMARY.*/SUMMARY comparisons=9 pass=9 diff=0 skip=0/' "$CU_BUILD/shadow-report.txt"; reseal
    refused_before_install "cutover.sh: the shadow report's summary does not match its lines"
}
cu_case "live: a report whose summary does not match its lines is refused" case_summary_disagrees

case_report_older_than_build() {
    ready; touch -d "now + 1 hour" "$CU_BUILD/SHA256SUMS"
    refused_before_install "cutover.sh: the build is newer than the shadow report; run shadow again"
}
cu_case "live: a report older than the build is refused" case_report_older_than_build

case_skip_not_accepted() {
    ready; cu_marker maintainer-merge; sed -i '/^# accepted-skip/d' "$CU_BUILD/shadow-ok"
    refused_before_install "cutover.sh: the report has a SKIP for maintainer-merge that shadow-ok does not accept"
}
cu_case "live: a SKIP the marker does not accept is refused" case_skip_not_accepted

case_marker_for_other_checkout() {
    ready; sed -i "s/^# checkout: .*/# checkout: $(printf 'b%.0s' $(seq 1 40))/" "$CU_BUILD/shadow-ok"
    refused_before_install "cutover.sh: shadow-ok was made for the checkout at bbbbbbbbbbbb and the checkout is at $(cu_head | cut -c1-12); run shadow again"
}
cu_case "live: a marker made for another commit of the checkout is refused" case_marker_for_other_checkout

case_shadow_compared_other_deployed_tree() {
    ready; sed -i 's/deployed=[0-9a-f]*/deployed=0000000/' "$CU_BUILD/shadow-report.txt"; reseal
    refused_before_install "cutover.sh: shadow compared a deployed tree from 0000000 and the checkout is at $(cu_head | cut -c1-7); install deploys the checkout"
}
cu_case "live: a report of a shadow against a deployed tree that is not the checkout is refused" case_shadow_compared_other_deployed_tree

case_symlink_in_dir() {
    ready; rm "$CU_BUILD/alert"; ln -s "$CU_BUILD/run" "$CU_BUILD/alert"
    refused_before_install "cutover.sh: $CU_BUILD/alert is a symbolic link; the build directory holds regular files only"
}
cu_case "live: a program of the build directory that is a link refuses" case_symlink_in_dir

case_subdir_in_dir() {
    ready; mkdir "$CU_BUILD/stray"
    refused_before_install "cutover.sh: $CU_BUILD/stray is not a regular file"
}
cu_case "live: a sub-directory in the build directory refuses" case_subdir_in_dir

case_no_build_info() {
    ready; rm "$CU_BUILD/BUILD_INFO"
    refused_before_install "cutover.sh: $CU_BUILD/BUILD_INFO is missing"
}
cu_case "live: a build directory with no BUILD_INFO refuses" case_no_build_info

case_dirty_build() {
    ready; sed -i 's/^dirty=0/dirty=1/' "$CU_BUILD/BUILD_INFO"
    refused_before_install "cutover.sh: the build in $CU_BUILD was made from a dirty tree; live deploys committed code only"
}
cu_case "live: a build made from a dirty tree refuses" case_dirty_build

case_other_commit() {
    ready
    printf 'more\n' > "$CU_FX/NEWFILE"; git -C "$CU_FX" add -A; git -C "$CU_FX" -c user.name=f -c user.email=f@e.invalid commit -q -m two
    refused_before_install "cutover.sh: the build in $CU_BUILD is from $(sed -n 's/^head=//p' "$CU_BUILD/BUILD_INFO" | cut -c1-12) and the checkout is at $(cu_head | cut -c1-12); install deploys the checkout, so build and shadow again"
}
cu_case "live: a build from another commit than the checkout's refuses" case_other_commit

case_dirty_checkout() {
    ready; printf 'edit\n' >> "$CU_FX/lib/run.sh"
    refused_before_install "cutover.sh: the checkout is dirty; install would deploy files no build was made from"
}
cu_case "live: a dirty checkout refuses" case_dirty_checkout

case_stamp_exists() {
    ready; cu_stamp run
    refused_before_install "cutover.sh: $CU_SHARE/RUST_STAMP exists: a Rust install is already in place; run scripts/cutover.sh rollback first"
}
cu_case "live: with a RUST_STAMP already there it refuses and points at rollback" case_stamp_exists

case_no_install_program() {
    ready; rm "$CU_BUILD/install"; cu_table > "$CU_BUILD/SHA256SUMS"; cu_marker
    refused_before_install "cutover.sh: the build in $CU_BUILD holds no install program"
}
cu_case "live: a build with no install program refuses" case_no_install_program

case_nothing_to_swap() {
    cu_deploy; cu_build run-evals install
    cu_fake_install; cu_marker
    refused_before_install "cutover.sh: the build in $CU_BUILD holds none of: maintainer-merge maintainer-doctor maintainer-repo run run-instance alert"
}
cu_case "live: a build with none of the six swappable programs refuses" case_nothing_to_swap

case_dry_run_fails() {
    ready; printf 'dry-fail' > "$CZ_CASE/standin/install.mode"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_run live --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_has "cutover.sh: the dry run of the install exited 3; nothing was installed"
    cz_check "the calls to install" "$(installs)" "--rust --rust-bin $CU_BUILD --dry-run"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
}
cu_case "live: a dry run that fails stops before the real install" case_dry_run_fails

# --- the read-back -----------------------------------------------------------------------------------------------

readback_fails() {  # $1 = install mode, $2 = the read-back line expected
    ready; printf '%s' "$1" > "$CZ_CASE/standin/install.mode"
    cu_run live --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_has "cutover.sh: read-back: $2"
    cz_expect_stderr_has "cutover.sh: the machine does not hold what the install reported. Put the bash programs back with: scripts/cutover.sh rollback"
}

case_readback_no_stamp() {
    readback_fails no-stamp "$CU_SHARE/RUST_STAMP is missing after the install"
}
cu_case "live: a stamp the install did not write fails the read-back" case_readback_no_stamp

case_readback_short_stamp() {
    readback_fails short-stamp "RUST_STAMP lists 2 program(s) and 6 were expected"
    cz_expect_stderr_has "cutover.sh: read-back: RUST_STAMP does not list $CU_SHARE/run.sh"
}
cu_case "live: a stamp that lists fewer programs than were swapped fails the read-back" case_readback_short_stamp

case_readback_bad_stamp_hash() {
    readback_fails bad-stamp-hash "RUST_STAMP gives $CU_BIN/maintainer-merge another hash than the build's"
}
cu_case "live: a stamp hash that is not the build's fails the read-back" case_readback_bad_stamp_hash

case_readback_not_rust() {
    ready; printf 'corrupt' > "$CZ_CASE/standin/install.mode"
    cu_run live --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_has "cutover.sh: read-back: $CU_SHARE/alert.sh is not the Rust build (alert): sha256 "
}
cu_case "live: an installed program that is not the Rust build fails the read-back" case_readback_not_rust

case_readback_not_swapped() {
    ready; printf 'no-swap' > "$CZ_CASE/standin/install.mode"
    cu_run live --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_has "cutover.sh: read-back: $CU_SHARE/alert.sh is not the Rust build (alert): sha256 "
}
cu_case "live: a program the install left as bash fails the read-back" case_readback_not_swapped

case_readback_no_backup() {
    readback_fails no-backup "the bash original $CU_SHARE/run.sh.sh is missing"
}
cu_case "live: a missing saved original fails the read-back" case_readback_no_backup

case_readback_bad_backup() {
    readback_fails bad-backup "$CU_SHARE/run.sh.sh is not the bash original (sha256 "
}
cu_case "live: a saved original that is not the checkout's bash program fails the read-back" case_readback_bad_backup

case_install_fails() {
    ready; printf 'fail-first' > "$CZ_CASE/standin/install.mode"
    cu_run live --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_has "cutover.sh: the install exited 1; if it stopped part way, put the bash programs back with: scripts/cutover.sh rollback (it refuses when there is nothing to roll back)"
}
cu_case "live: an install that exits non-zero is reported with the way back" case_install_fails

case_install_fails_midway_then_rollback() {
    ready; printf 'fail-after-2' > "$CZ_CASE/standin/install.mode"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"
    cu_run live --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_has "the install exited 1"
    cu_run rollback
    cz_expect_rc 0
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"
    cu_expect_same_snap "the machine after the failed install and the rollback" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
}
cu_case "live: an install that fails after two swaps is undone by rollback" case_install_fails_midway_then_rollback

case_exit_late() {
    ready; printf 'exit-late' > "$CZ_CASE/standin/install.mode"
    cu_run live --build-dir "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_has "the install exited 1"
}
cu_case "live: an install that did everything and then exits 1 is still a failure" case_exit_late

case_partial_build() {
    cu_deploy; cu_build maintainer-doctor maintainer-repo run install
    cu_fake_install; cu_marker
    cu_run live --build-dir "$CU_BUILD"
    cz_expect_rc 0
    cz_expect_stdout_has "live: read back 3 program(s)"
    cz_expect_lacks stdout "maintainer-merge"
    [ ! -e "$CU_BIN/maintainer-merge.sh" ] || cz_note_fail "a program with no Rust build was saved and replaced"
}
cu_case "live: a build without some of the six swaps only the ones it holds" case_partial_build

case_not_linux() {
    ready
    printf '#!/usr/bin/env bash\necho Darwin\n' > "$CZ_CASE/bin/uname"; chmod +x "$CZ_CASE/bin/uname"
    cu_run live --build-dir "$CU_BUILD"
    cz_expect_stdout_has "live: read back 4 program(s)"
}
cu_case "live: off Linux the two systemd helpers are not expected" case_not_linux

case_checkout_source_missing() {
    ready
    git -C "$CU_FX" rm -q platform/linux/alert.sh; git -C "$CU_FX" -c user.name=f -c user.email=f@e.invalid commit -q -m gone
    sed -i "s/^head=.*/head=$(cu_head)/" "$CU_BUILD/BUILD_INFO"
    cu_marker
    refused_before_install "cutover.sh: the checkout has no $CU_FX/platform/linux/alert.sh, the bash program alert stands for"
}
cu_case "live: a swappable program with no bash source in the checkout refuses" case_checkout_source_missing

cz_finish
