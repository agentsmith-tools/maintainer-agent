#!/usr/bin/env bash
# Characterization: scripts/check_claims.sh. It runs the suite once, reads the
# count off the suite's summary line, generates the deny wall of every profile and
# hands the count to scripts/render_figures.py, which holds the figures. The cases
# pin every message, stream and exit code, the order and argv of the python3 calls,
# and the temporary files left behind.
#
# The suite and the figure renderer are stubs in most cases (the real suite takes
# 155 s and the real renderer reads the real documents). check_claims_e2e at the
# end runs the real renderer on a copy of the real documents, with a stub suite.
# The deny wall renderer is always the real scripts/render-settings.py.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib-checks.sh"
cc_init

# cl_rules DENY-JSON-FILE: how many rules the real renderer generates for that file under
# the program's own PATH. The oracle for the "N rules" notes: it does not read the program.
cl_rules() {
    local scratch="$CZ_CASE/oracle"; rm -rf "$scratch"; mkdir -p "$scratch"
    cp "$1" "$scratch/deny.json"
    env -i PATH="${CZ_E[PATH]}" "$CC_REAL_PYTHON3" "$MX_TREE/scripts/render-settings.py" "$scratch" /home/claimcheck >/dev/null 2>&1 \
        || { echo "characterization: the oracle could not render $1" >&2; exit 2; }
    "$CC_REAL_PYTHON3" -I -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["permissions"]["deny"]))' "$scratch/settings.json"
}

# The five python3 calls a green run makes for one profile P, in order. N is the test count.
cl_py_profile() {  # $1 = profile
    printf '%s\n' \
        "python3 [@CASE@/tree/scripts/render-settings.py] [@TMP@/$1] [/home/claimcheck]" \
        "python3 [-c] [\\nimport json;print(len(json.load(open('@TMP@/$1/settings.json'))['permissions']['deny']))]"
}
cl_py_tail() {  # $1 = N: what follows the per-profile calls
    printf '%s\n' \
        "python3 [@CASE@/tree/scripts/render-settings.py] [@TMP@] [/home/claimcheck]" \
        "python3 [-c] [\\nimport json\\nprint(len(json.load(open('@TMP@/settings.json'))['permissions']['deny']))]" \
        "python3 [@CASE@/tree/scripts/render_figures.py] [--root] [@CASE@/tree] [--check] [--tests] [$1]"
}
cl_py_values() { printf '%s\n' "python3 [@CASE@/tree/scripts/render_figures.py] [--root] [@CASE@/tree] [--values] [--tests] [$1]"; }

case_green() {
    cc_claims_tree
    cc_suite "  10 passed, 0 failed" 0
    cc_figs check "  ok: 12 figure markers in 3 files are current"
    cc_figs values "$(printf 'tests 10\nevals 9')"
    cc_claims
    local n; n="$(cl_rules "$CZ_TREE/profiles/sysknife/deny.json")"
    cz_expect_rc 0
    cz_expect_stderr_empty
    cz_expect_stdout \
        "  profile sysknife: $n rules" \
        "  those verbs generate $n rules on this machine" \
        "    ok: 12 figure markers in 3 files are current" \
        "  measured: tests 10, evals 9"
    cc_expect_norm_calls "suite @CASE@/cwd" "$(cl_py_profile sysknife)" "$(cl_py_tail 10)" "$(cl_py_values 10)"
    cz_check "suite runs" "$(cc_count_lines "$CZ_CASE/suite.calls")" 1
    cz_check "suite working directory and argument count" "$(cat "$CZ_CASE/suite.calls")" "suite cwd=$CZ_CWD args=0"
}
cc_case "green: the suite count and the figures pass, so it prints the profile, rule and figure lines and exits 0" case_green

case_skipped_counted() {
    cc_claims_tree
    cc_suite "$(printf '  10 passed, 0 failed, 3 skipped\n  A skip is a case this machine could not run.')" 0
    cc_figs check "ok"; cc_figs values "tests 13"
    cc_claims
    cz_expect_rc 0
    cz_expect_stdout_has "  counting 10 passed + 3 skipped as the suite size"
    cz_expect_stdout_has "  measured: tests 13"
    cz_check "the count handed to the renderer" "$(cat "$CZ_CASE/figs.calls" | head -1)" "--root $CZ_TREE --check --tests 13"
}
cc_case "skipped cases count towards the suite size, and the run says so" case_skipped_counted

case_no_skip_line() {
    cc_claims_tree
    cc_suite "  7 passed, 0 failed" 0
    cc_figs check "ok"; cc_figs values "tests 7"
    cc_claims
    cz_expect_rc 0
    cz_expect_lacks stdout "counting"
    cz_check "the count handed to the renderer" "$(head -1 "$CZ_CASE/figs.calls")" "--root $CZ_TREE --check --tests 7"
}
cc_case "a summary with no skipped part prints no counting line and hands over the passed count" case_no_skip_line

case_zero_skipped() {
    cc_claims_tree
    cc_suite "  5 passed, 0 failed, 0 skipped" 0
    cc_figs check "ok"; cc_figs values "tests 5"
    cc_claims
    cz_expect_rc 0
    cz_expect_stdout_has "  counting 5 passed + 0 skipped as the suite size"
    cz_check "the count handed to the renderer" "$(head -1 "$CZ_CASE/figs.calls")" "--root $CZ_TREE --check --tests 5"
}
cc_case "a summary that says 0 skipped still prints the counting line" case_zero_skipped

# The count is read with two sed expressions over the whole output, and the LAST match of each
# wins. A stray line that looks like a summary after the real one moves the count. Pins current
# behaviour.
case_last_match_wins() {
    cc_claims_tree
    cc_suite "$(printf '  10 passed, 0 failed, 3 skipped\nnote: 99 passed in the sub-run, 4 skipped\n   40 passed in a nested run')" 0
    cc_figs check "ok"; cc_figs values "tests 0"
    cc_claims
    cz_expect_rc 0
    cz_expect_stdout_has "  counting 40 passed + 4 skipped as the suite size"
    cz_check "the count handed to the renderer" "$(head -1 "$CZ_CASE/figs.calls")" "--root $CZ_TREE --check --tests 44"
}
cc_case "the last 'N passed' line and the last ', N skipped' match win, wherever they sit in the output" case_last_match_wins

case_passed_anchored() {
    cc_claims_tree
    cc_suite "$(printf '  8 passed, 0 failed\nall 12 passed')" 0
    cc_figs check "ok"; cc_figs values "tests 8"
    cc_claims
    cz_check "the count handed to the renderer" "$(head -1 "$CZ_CASE/figs.calls")" "--root $CZ_TREE --check --tests 8"
}
cc_case "'N passed' counts only at the start of a line, after spaces" case_passed_anchored

case_stderr_in_suite_output() {
    cc_claims_tree
    cc_suite "  6 passed, 0 failed" 0
    printf 'a line on stderr, 70 passed\n' > "$CZ_CASE/suite.err"
    cc_figs check "ok"; cc_figs values "tests 6"
    cc_claims
    cz_expect_rc 0
    cz_expect_stderr_empty
    cz_check "the count handed to the renderer" "$(head -1 "$CZ_CASE/figs.calls")" "--root $CZ_TREE --check --tests 6"
}
cc_case "the suite's stderr goes to the same capture as its stdout and never reaches the caller's stderr" case_stderr_in_suite_output

# --- a red suite ------------------------------------------------------------------------------

case_red_suite() {
    cc_claims_tree
    cc_suite "$(printf '  PASS a\n  FAIL the first\n  FAIL the second\n    FAIL indented four\n  FAIL the third\n  1 passed, 3 failed')" 1
    cc_claims
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr \
        "FAIL: the test suite is RED; fix that before believing any number here" \
        "  FAIL the first" "  FAIL the second" "  FAIL the third"
    cc_expect_norm_calls "suite @CASE@/cwd"
    cz_check "temporary files left" "$(cc_ls "$CZ_TMP" | wc -l | tr -d ' ')" 0
}
cc_case "a red suite prints one FAIL line and its failing cases on stderr, exits 1, runs nothing else and leaves no temporary file" case_red_suite

case_red_suite_ten_lines() {
    cc_claims_tree
    local i out=""
    for i in $(seq 1 14); do out="$out  FAIL case $i"$'\n'; done
    cc_suite "$out" 3
    cc_claims
    cz_expect_rc 1
    cz_check "FAIL lines on stderr" "$(grep -c '^  FAIL case' "$CZ_CASE/stderr")" 10
    cz_expect_stderr_has "  FAIL case 10"
    cz_expect_lacks stderr "case 11"
}
cc_case "a red suite shows at most its first ten failing cases" case_red_suite_ten_lines

case_red_exit_code_any() {
    cc_claims_tree
    cc_suite "  99 passed, 0 failed" 2
    cc_claims
    cz_expect_rc 1
    cz_expect_stderr_first "FAIL: the test suite is RED; fix that before believing any number here"
}
cc_case "a suite that prints a clean summary and exits non-zero is red, whatever the code" case_red_exit_code_any

case_missing_suite() {
    cc_claims_tree
    rm "$CZ_TREE/tests/run-tests.sh"
    cc_claims
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr_first "FAIL: the test suite is RED; fix that before believing any number here"
    cz_check "stderr lines" "$(wc -l < "$CZ_CASE/stderr" | tr -d ' ')" 1
    cz_check "temporary files left" "$(cc_ls "$CZ_TMP" | wc -l | tr -d ' ')" 0
}
cc_case "a missing tests/run-tests.sh reads as a red suite" case_missing_suite

case_no_summary() {
    cc_claims_tree
    cc_suite "everything is fine, no summary line" 0
    cc_claims
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr "FAIL: could not measure the test count; refusing to pass over nothing"
    cc_expect_norm_calls "suite @CASE@/cwd"
    cz_check "temporary files left" "$(cc_ls "$CZ_TMP" | wc -l | tr -d ' ')" 0
}
cc_case "a green suite with no summary line is refused with exit 1 before any profile is rendered" case_no_summary

case_zero_passed() {
    cc_claims_tree
    cc_suite "  0 passed, 0 failed" 0
    cc_figs check "refused"; cc_figs values "x 1"
    cc_claims
    cz_expect_rc 0
    cz_check "the count handed to the renderer" "$(head -1 "$CZ_CASE/figs.calls")" "--root $CZ_TREE --check --tests 0"
}
cc_case "a summary that says 0 passed is a measured count of 0, and the renderer decides what that is worth" case_zero_passed

# --- the deny walls -------------------------------------------------------------------------------

case_profiles_in_order() {
    cc_claims_tree
    cc_profile _template; cc_profile magent
    mkdir -p "$CZ_TREE/profiles/empty"
    cc_suite "  3 passed, 0 failed" 0
    cc_figs check "ok"; cc_figs values "tests 3"
    cc_claims
    local n; n="$(cl_rules "$CZ_TREE/profiles/sysknife/deny.json")"
    cz_expect_rc 0
    cz_expect_stdout \
        "  profile _template: $n rules" \
        "  profile magent: $n rules" \
        "  profile sysknife: $n rules" \
        "  those verbs generate $n rules on this machine" \
        "  ok" \
        "  measured: tests 3"
    cc_expect_norm_calls "suite @CASE@/cwd" "$(cl_py_profile _template)" "$(cl_py_profile magent)" "$(cl_py_profile sysknife)" "$(cl_py_tail 3)" "$(cl_py_values 3)"
}
cc_case "every profile with a deny.json is rendered in byte order, _template first, and a directory with none is skipped" case_profiles_in_order

case_profile_does_not_render() {
    cc_claims_tree
    cc_profile broken '{"spelled_everywhere": ["cargo publish"], "credential_paths": []}'
    cc_suite "  3 passed, 0 failed" 0
    cc_figs check "ok"; cc_figs values "tests 3"
    cc_claims
    local n; n="$(cl_rules "$CZ_TREE/profiles/sysknife/deny.json")"
    cz_expect_rc 1
    cz_expect_stderr "FAIL: profile 'broken' does not generate a deny wall"
    cz_expect_stdout \
        "  profile broken:  rules" \
        "  profile sysknife: $n rules" \
        "  those verbs generate $n rules on this machine" \
        "  ok" \
        "  measured: tests 3"
}
cc_case "a profile whose wall does not render is named on stderr, prints an empty rule count, does not stop the run, and fails it" case_profile_does_not_render

case_no_sysknife_deny() {
    cc_claims_tree
    rm "$CZ_TREE/profiles/sysknife/deny.json"
    cc_profile other
    cc_suite "  3 passed, 0 failed" 0
    cc_figs check "ok"; cc_figs values "tests 3"
    cc_claims
    local n; n="$(cl_rules "$CZ_TREE/profiles/other/deny.json")"
    # Pins current behaviour: the combined count is read from profiles/sysknife/deny.json by name.
    # Without it the cp fails silently, the render fails silently and the line carries no number.
    cz_expect_rc 0
    cz_expect_stderr_empty
    cz_expect_stdout \
        "  profile other: $n rules" \
        "  those verbs generate  rules on this machine" \
        "  ok" \
        "  measured: tests 3"
}
cc_case "with no profiles/sysknife/deny.json the combined rule line carries no number and the run still passes (pins current behaviour)" case_no_sysknife_deny

case_no_profiles_at_all() {
    cc_claims_tree
    rm -rf "$CZ_TREE/profiles"
    cc_suite "  3 passed, 0 failed" 0
    cc_figs check "ok"; cc_figs values "tests 3"
    cc_claims
    # Pins current behaviour: with no profile the glob stays literal, `cp` and every render fail
    # in silence except the one named profile, and the run passes.
    cz_expect_stderr \
        "cp: cannot stat '$CZ_TREE/profiles/*/deny.json': No such file or directory" \
        "FAIL: profile '*' does not generate a deny wall"
    cz_expect_rc 1
    cz_expect_stdout_has "  those verbs generate  rules on this machine"
}
cc_case "with no profiles directory the literal glob is rendered as a profile named * and fails the run (pins current behaviour)" case_no_profiles_at_all

case_deny_json_unreadable_profile_with_home() {
    cc_claims_tree
    cc_profile leaky '{"spelled_everywhere": ["cargo publish"], "credential_paths": ["~/.ssh/**", "/home/claimcheck/x"]}'
    cc_suite "  3 passed, 0 failed" 0
    cc_figs check "ok"; cc_figs values "tests 3"
    cc_claims
    cz_expect_rc 1
    cz_expect_stderr "FAIL: profile 'leaky' does not generate a deny wall"
}
cc_case "a deny.json that holds the fixed home path /home/claimcheck is refused by the renderer and so by the check" case_deny_json_unreadable_profile_with_home

case_tree_untouched() {
    cc_claims_tree
    cc_suite "  3 passed, 0 failed" 0
    cc_figs check "ok"; cc_figs values "tests 3"
    cc_snap "$CZ_TREE" > "$CZ_CASE/before"
    cc_claims
    cc_snap "$CZ_TREE" > "$CZ_CASE/after"
    cz_expect_rc 0
    cc_expect_unchanged "the tree" "$CZ_CASE/before" "$CZ_CASE/after"
    cz_check "cwd contents" "$(cc_ls "$CZ_CWD" | wc -l | tr -d ' ')" 0
}
cc_case "the run writes nothing in the tree or in the working directory" case_tree_untouched

# Pins current behaviour. The second `trap ... EXIT` replaces the first, so the file that holds the
# suite's output is never removed once the run gets past the suite.
case_suite_output_file_leaks() {
    cc_claims_tree
    cc_suite "  3 passed, 0 failed" 0
    cc_figs check "ok"; cc_figs values "tests 3"
    cc_claims
    cz_expect_rc 0
    cz_check "files left in TMPDIR" "$(cc_ls "$CZ_TMP" | wc -l | tr -d ' ')" 1
    local left; left="$(cc_ls "$CZ_TMP")"
    if [ -f "$CZ_TMP/$left" ]; then
        cz_check "the leftover is the suite's output" "$(cat "$CZ_TMP/$left")" "  3 passed, 0 failed"
    else
        cz_note_fail "the leftover is not a file: $left"
    fi
    [[ "$left" =~ ^tmp\.[A-Za-z0-9]{10}$ ]] || cz_note_fail "the leftover is not a mktemp name: $left"
}
cc_case "a run that passes the suite leaves the suite's output file in TMPDIR and removes the generated walls (pins current behaviour)" case_suite_output_file_leaks

# --- the figures --------------------------------------------------------------------------------

case_stale_figure() {
    cc_claims_tree
    cc_suite "  10 passed, 0 failed" 0
    cc_figs check "" 1 "FAIL README.md:639: stale marker 'tests': it says 1,700+ and the source says 1,800+"
    cc_claims
    local n; n="$(cl_rules "$CZ_TREE/profiles/sysknife/deny.json")"
    cz_expect_rc 1
    cz_expect_stdout \
        "  profile sysknife: $n rules" \
        "  those verbs generate $n rules on this machine"
    cz_expect_stderr \
        "FAIL README.md:639: stale marker 'tests': it says 1,700+ and the source says 1,800+" \
        "FAIL: a figure in the documents is stale, typed outside a marker, or unreadable; run scripts/render_figures.py --write"
    cz_check "renderer calls" "$(cc_count_lines "$CZ_CASE/figs.calls")" 1
}
cc_case "a figure the renderer refuses prints its report and the FAIL line on stderr, exits 1, and never asks for the values" case_stale_figure

case_figure_report_one_line() {
    cc_claims_tree
    cc_suite "  10 passed, 0 failed" 0
    printf '%s\n' "FAIL README.md:1: x" > "$CZ_CASE/figs.check.err"; printf 2 > "$CZ_CASE/figs.check.rc"
    cc_claims
    cz_expect_rc 1
    cz_expect_stderr "FAIL README.md:1: x" "FAIL: a figure in the documents is stale, typed outside a marker, or unreadable; run scripts/render_figures.py --write"
}
cc_case "the renderer's stderr joins its stdout in the report, and any non-zero code is a failure" case_figure_report_one_line

case_figures_multiline_note() {
    cc_claims_tree
    cc_suite "  10 passed, 0 failed" 0
    cc_figs check "$(printf 'first line\nsecond line')"
    cc_figs values "$(printf 'a 1\nb 2\nc 3')"
    cc_claims
    cz_expect_rc 0
    cz_expect_stdout_has "  first line"
    # note prints its argument after two spaces, so only the first line of a report is indented.
    cz_check "second report line" "$(grep -n '^second line$' "$CZ_CASE/stdout" | cut -d: -f2)" "second line"
    cz_expect_stdout_has "  measured: a 1, b 2, c 3"
}
cc_case "a multi-line report is indented on its first line only, and the measured figures are joined with ', '" case_figures_multiline_note

case_measured_commas() {
    cc_claims_tree
    cc_suite "  10 passed, 0 failed" 0
    cc_figs check "ok"
    cc_figs values "$(printf 'tests 1,800\nbackends 4')"
    cc_claims
    cz_expect_rc 0
    cz_expect_stdout_has "  measured: tests 1, 800, backends 4"
}
cc_case "a comma inside a measured value gains a space too (pins current behaviour)" case_measured_commas

case_values_fail() {
    cc_claims_tree
    cc_suite "  10 passed, 0 failed" 0
    cc_figs check "  ok"
    cc_figs values "cannot read docs/lessons.md" 1
    cc_claims
    cz_expect_rc 1
    cz_expect_stdout_has "    ok"
    cz_expect_lacks stdout "measured"
    cz_expect_stderr "cannot read docs/lessons.md" "FAIL: could not print the measured figures"
}
cc_case "a --values run that fails prints its output and a FAIL line on stderr and fails the run, after the ok line" case_values_fail

case_figures_args() {
    cc_claims_tree
    cc_suite "  10 passed, 0 failed, 1 skipped" 0
    cc_figs check "ok"; cc_figs values "tests 11"
    cc_claims
    cz_check "renderer argv, both calls" "$(tr '\n' '|' < "$CZ_CASE/figs.calls")" "--root $CZ_TREE --check --tests 11|--root $CZ_TREE --values --tests 11|"
}
cc_case "the renderer is called twice, --check then --values, each with --root TREE and --tests N" case_figures_args

case_root_is_the_script_parent() {
    cc_claims_tree
    mkdir -p "$CZ_CASE/elsewhere"
    local d
    for d in scripts tests profiles; do ln -s "$CZ_TREE/$d" "$CZ_CASE/elsewhere/$d"; done
    cc_suite "  10 passed, 0 failed" 0
    cc_figs check "ok"; cc_figs values "tests 10"
    cc_claims_via_symlink() { cc_run "$CZ_CASE/elsewhere/scripts/check_claims.sh"; }
    cc_claims_via_symlink
    cz_expect_rc 0
    # Pins current behaviour: the tree is the logical parent of the script's directory, so a
    # symlinked scripts directory makes the parent the folder holding the link.
    cz_check "renderer --root" "$(head -1 "$CZ_CASE/figs.calls" | cut -d' ' -f1-2)" "--root $CZ_CASE/elsewhere"
}
cc_case "the tree is the logical parent of the script's directory, symlinks not resolved (pins current behaviour)" case_root_is_the_script_parent

case_root_through_a_linked_prefix() {
    cc_claims_tree
    mkdir -p "$CZ_CASE/via"
    ln -s "$CZ_TREE" "$CZ_CASE/via/linked"
    cc_suite "  10 passed, 0 failed" 0
    cc_figs check "ok"; cc_figs values "tests 10"
    cc_run "$CZ_CASE/via/linked/scripts/check_claims.sh"
    cz_expect_rc 0
    # Pins current behaviour: `cd DIR/.. && pwd` is logical, so a link anywhere in the program's own path
    # stays in the tree's name. The renderer is handed the linked name, not the real one.
    cz_check "renderer --root" "$(head -1 "$CZ_CASE/figs.calls" | cut -d' ' -f1-2)" "--root $CZ_CASE/via/linked"
}
cc_case "the tree keeps the name of a link in the program's own path (pins current behaviour)" case_root_through_a_linked_prefix

case_nul_in_a_figure_report() {
    cc_claims_tree
    cc_suite "  10 passed, 0 failed" 0
    printf 'ok\0nul\n' > "$CZ_CASE/figs.check.out"; printf 0 > "$CZ_CASE/figs.check.rc"
    printf 'a\0b 1\n' > "$CZ_CASE/figs.values.out"; printf 0 > "$CZ_CASE/figs.values.rc"
    cc_claims
    cz_expect_rc 0
    # The shell drops a NUL byte from a command substitution and says so once per substitution, with its line.
    cz_expect_stderr \
        "$CZ_TREE/scripts/check_claims.sh: line 65: warning: command substitution: ignored null byte in input" \
        "$CZ_TREE/scripts/check_claims.sh: line 71: warning: command substitution: ignored null byte in input"
    cz_expect_stdout_has "  oknul"
    cz_expect_stdout_has "  measured: ab 1"
}
cc_case "a NUL byte in a renderer's output is dropped with the shell's warning, once per substitution (pins current behaviour)" case_nul_in_a_figure_report

# --- the real renderer on a copy of the real documents -------------------------------------------------------

# cl_e2e_tree: the program, the real renderers, and a copy of the real prose and the files the
# figures are counted from, with a stub suite. A first run of the real renderer in write mode makes the
# copy consistent with a suite of 9999 cases, so the cases do not move when a real document does.
cl_e2e_tree() {
    cc_claims_tree
    local f
    for f in README.md CONTRIBUTING.md docs lib/backends evals/scenarios profiles scripts/render_figures.py; do
        [ -e "$MX_TREE/$f" ] || { echo "characterization: MX_TREE has no $f" >&2; exit 2; }
        mkdir -p "$(dirname "$CZ_TREE/$f")"; rm -rf "${CZ_TREE:?}/$f"; cp -a "$MX_TREE/$f" "$CZ_TREE/$f"
    done
    local out
    out="$(env -i PATH="${CZ_E[PATH]}" "$CC_REAL_PYTHON3" "$CZ_TREE/scripts/render_figures.py" --root "$CZ_TREE" --write --tests 9999 2>&1)" \
        || { echo "characterization: could not make the document copy consistent: $out" >&2; exit 2; }
    : > "$CZ_CASE/calls"
}

case_e2e_consistent() {
    cl_e2e_tree
    cc_suite "  9999 passed, 0 failed" 0
    cc_claims
    cz_expect_rc 0
    cz_expect_stderr_empty
    cz_expect_matches stdout '^    ok: [0-9]+ figure markers in [0-9]+ files are current$'
    cz_expect_matches stdout '^  measured: tests 9999, evals [0-9]+, '
    cz_expect_matches stdout '^  profile sysknife: [0-9]+ rules$'
}
cc_case "real renderer: documents that state the measured count pass and the measured figures are printed" case_e2e_consistent

case_e2e_drifted_count() {
    cl_e2e_tree
    cc_suite "  9899 passed, 0 failed" 0
    cc_claims
    cz_expect_rc 1
    cz_expect_lacks stdout "ok:"
    cz_expect_matches stderr '^FAIL README\.md:[0-9]+: .*tests'
    cz_expect_stderr_has "FAIL: a figure in the documents is stale, typed outside a marker, or unreadable; run scripts/render_figures.py --write"
}
cc_case "real renderer: a suite one floor below what the README states fails with a FAIL line naming README.md and the tests figure" case_e2e_drifted_count

case_e2e_typed_figure() {
    cl_e2e_tree
    printf '\nThe suite holds 4242 offline tests.\n' >> "$CZ_TREE/README.md"
    cc_suite "  9999 passed, 0 failed" 0
    cc_claims
    cz_expect_rc 1
    cz_expect_matches stderr '^FAIL README\.md:[0-9]+: .*4242'
    cz_expect_stderr_has "FAIL: a figure in the documents is stale, typed outside a marker, or unreadable; run scripts/render_figures.py --write"
}
cc_case "real renderer: a count typed outside a marker fails the check even when it equals nothing" case_e2e_typed_figure

case_e2e_no_markers() {
    cl_e2e_tree
    printf 'A README with no numbers.\n' > "$CZ_TREE/README.md"
    cc_suite "  9999 passed, 0 failed" 0
    cc_claims
    cz_expect_rc 1
    cz_expect_stderr_has "FAIL: a figure in the documents is stale, typed outside a marker, or unreadable; run scripts/render_figures.py --write"
    cz_expect_matches stderr '^FAIL README\.md'
}
cc_case "real renderer: a README with no figure markers fails, because the required sites stopped stating their figures" case_e2e_no_markers

cz_finish
