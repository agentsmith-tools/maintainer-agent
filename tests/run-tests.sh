#!/usr/bin/env bash
# Offline test suite. No network, no GitHub, no model call.
#
# Everything here tests a REFUSAL. This agent's safety comes from what it
# declines to do, so those are the paths worth pinning.
set -uo pipefail
# A git hook runs this suite with GIT_INDEX_FILE, GIT_DIR and friends set for
# the commit in progress. Every scratch repository below would inherit them and
# write into the caller's index (lesson 110), so none of them survive this line.
# shellcheck disable=SC2046  # the names are git's own fixed list, one per word
unset $(git rev-parse --local-env-vars)

# Two roots, because two different things are being named.
#
#   suite_root  the checkout that holds this file. The suite's own text goes
#               through it and nothing else does.
#   root        the tree under test: every program, profile, script and document
#               the cases run or read. It is suite_root unless MX_TREE names
#               another directory with the same layout, which is how a build whose
#               programs are not shell is held to the same cases.
#
# MX_TREE is checked before anything runs. A tree that is missing what every case
# needs would otherwise fail a thousand cases for one reason, and a wrong path
# would read as a tree that fails.
suite_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ -n "${MX_TREE:-}" ]; then
    root="$(cd -- "$MX_TREE" 2>/dev/null && pwd)" || root="$MX_TREE"
else
    root="$suite_root"
fi
if [ ! -f "$root/bin/maintainer-merge" ] || [ ! -f "$root/lib/run.sh" ] || [ ! -d "$root/profiles" ]; then
    printf 'run-tests.sh: MX_TREE=%s is not a tree under test: it needs bin/maintainer-merge, lib/run.sh and profiles/\n' "$root" >&2
    exit 2
fi
printf 'tree under test: %s\n' "$root"

# Plan decisions D10 to D22 changed behaviour on purpose in the Rust programs, so a
# case that pins one of them holds both builds and picks its side by what the tree
# is. rust/scripts/assemble-tree.sh puts the Rust `run` at .rust-bin/run, behind
# the wrapper that replaces lib/run.sh. A tree without it runs the bash program.
# shellcheck disable=SC2034  # read by the parts in tests/suites/, which share this shell
if [ -x "$root/.rust-bin/run" ]; then run_is_rust=1; else run_is_rust=0; fi

# Clear anything a run exports before measuring anything.
#
# Invoked from inside a run, this suite reported 236 passed / 6 failed, and
# worse, two cases in the cadence block PASSED because MAINTAINER_FORCE=1 was
# inherited and the gate they test was disabled. MAINTAINER_POST=off sent every
# run.sh the suite starts down the rehearsal branch, which exits 78 in a
# checkout that has no rendered wall, so the identity gate cases never reached
# the gate they exist to prove.
#
# A suite whose answer depends on who called it is not a measurement.
unset MAINTAINER_FORCE MAINTAINER_POST MAINTAINER_PROFILE MAINTAINER_SETTINGS \
      MAINTAINER_STATE MAINTAINER_STATE_DIR MAINTAINER_SLUG MAINTAINER_REPO \
      MAINTAINER_TASKS MAINTAINER_SKILL MAINTAINER_IN_RUN MAINTAINER_SCRIPTS \
      MAINTAINER_BACKEND MAINTAINER_PROSE_STYLE MAINTAINER_ACCOUNT
pass=0; fail=0; skip=0
ok()   { printf '  PASS  %s\n' "$1"; pass=$((pass+1)); sc_count; }
bad()  { printf '  FAIL  %s\n' "$1" >&2; fail=$((fail+1)); sc_count; }
# A skip is not a pass. It is counted separately, printed in the summary, and CI
# requires the count to be zero, so a case that stops running because a tool went
# missing shows up as a gap rather than as silence. The only user today is the
# container verification, which needs podman or docker and cannot be faked.
noenv(){ printf '  SKIP  %s\n' "$1"; skip=$((skip+1)); }

# Source-coupled cases. A case is source-coupled when its verdict depends on the
# TEXT of a shell program in the tree under test: it greps or sed-extracts the
# program, sources it and calls an internal function, or runs a checker that
# scans program text. A black-box case runs a program and reads what it prints,
# writes and exits with, so it holds for any tree with this layout, a Rust build
# included. A source-coupled case cannot: a binary has no text to grep and no
# function to source, and a grep that happens to match a binary proves nothing.
#
#     if sc_case 2; then
#     ...two cases: ok, bad or check calls...
#     sc_end
#     fi
#
# sc_case N opens a block that records exactly N outcomes. With
# MX_SKIP_SOURCE_COUPLED=1 it returns 1, nothing in the block runs, and the N
# cases count as N skips. sc_end proves the block recorded N outcomes, so the
# number a skipped run reports is the number a normal run executes. The body is
# not indented, so a heredoc inside it keeps its bytes. CI requires zero skips:
# the only user of MX_SKIP_SOURCE_COUPLED is a run against a tree that is not
# shell, never the gate for this repository.
sc_total=0; sc_open=0; sc_want=0; sc_got=0; sc_line=0; sc_file=run-tests.sh
sc_count() { [ "$sc_open" = 1 ] || return 0; sc_got=$((sc_got+1)); sc_total=$((sc_total+1)); }
# A malformed block is a defect in the suite, and it is counted as a failure, not
# as an outcome of the block it is in.
sc_flag() { printf '  FAIL  %s\n' "$1" >&2; fail=$((fail+1)); }
sc_case() {
    local n="$1" line="${BASH_LINENO[0]}" file="${BASH_SOURCE[1]##*/}"
    if [ "$sc_open" = 1 ]; then
        sc_flag "the source-coupled block at $sc_file:$sc_line was never closed with sc_end"; sc_open=0
    fi
    case "$n" in ''|*[!0-9]*|0) sc_flag "sc_case at $file:$line needs a positive case count, not '$n'"; return 1 ;; esac
    if [ "${MX_SKIP_SOURCE_COUPLED:-}" = 1 ]; then
        sc_total=$((sc_total+n)); skip=$((skip+n))
        printf '  SKIP  source-coupled, %d case(s) at %s:%d\n' "$n" "$file" "$line"
        return 1
    fi
    sc_open=1; sc_want=$n; sc_got=0; sc_line=$line; sc_file=$file
}
sc_end() {
    if [ "$sc_open" != 1 ]; then sc_flag "sc_end at ${BASH_SOURCE[1]##*/}:${BASH_LINENO[0]} closes no sc_case"; return 0; fi
    sc_open=0
    [ "$sc_got" = "$sc_want" ] \
        || sc_flag "the source-coupled block at $sc_file:$sc_line declares $sc_want case(s) and recorded $sc_got"
}
sc_note() { printf ' (%d source-coupled cases)' "$sc_total"; }

check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (expected '$3', got '$2')"; fi; }

# The sysknife shell suite builds its own image, because the release rehearsal
# it runs needs cargo, node and npm. This suite drives maintainer-merge end to
# end against that profile on a four-line check.sh, so it needs the gate's
# machinery and none of those tools, and it runs on GitHub runners where a
# locally built image does not exist. Pin those cases to a registry image.
export MAINTAINER_SHELL_IMAGE=docker.io/library/python:3.12

# A fake PATH: gh, claude, codex and notify-send never reach the real ones.
stub_dir="$(mktemp -d)"
# And a scratch state directory, so a test run cannot append to the real audit
# trail. It did: 22 empty logs from --show-prompt calls landed in the live one.
export MAINTAINER_STATE_DIR="$stub_dir/state-root"
mkdir -p "$MAINTAINER_STATE_DIR/state"
# And a scratch signing key. Without this the suite signs with the key
# install.sh minted on the developer's laptop, so it passed here and failed in
# CI, where no key exists at all. The forgery cases below override it again with
# a key of their own.
export MAINTAINER_RECEIPT_KEY="$stub_dir/receipt.key"
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$MAINTAINER_RECEIPT_KEY"
# The EXIT trap also fails the suite when the shell ends before the summary: a part that
# runs `exit 0` would otherwise end the run green with most of the cases never run.
mx_done=0
mx_exit() {
    local rc=$?
    rm -rf "$stub_dir"
    if [ "$mx_done" != 1 ]; then
        printf '  FAIL  the suite ended before its summary (exit %s), so the cases after that point never ran\n' "$rc" >&2
        [ "$rc" -ne 0 ] || exit 1
    fi
}
trap mx_exit EXIT
make_stub() { printf '#!/usr/bin/env bash\n%s\n' "$2" > "$stub_dir/$1"; chmod +x "$stub_dir/$1"; }
# No case may reach the real desktop. The notify-send stub below only helps a
# case whose PATH includes $stub_dir, and two of them deliberately build a
# minimal PATH without it, which is how this suite spent 2026-09-10 firing
# critical popups about a gh stub at whoever was using the machine.
export MAINTAINER_NOTIFY=off
make_stub notify-send 'exit 0'
make_stub claude 'echo "claude stub invoked" >&2; exit 0'
make_stub codex  'echo "codex stub invoked" >&2; exit 0'
make_stub flock  'exit 0'

# Every run.sh case provisions from HERE, never from github.com.
#
# run.sh step 0b clones REPO_ORIGIN when REPO_PATH has no .git, and the sysknife
# profile defaults that to https://github.com/lacs-project/sysknife.git. Two
# cases reached it, so a suite whose own README calls it offline cloned a real
# repository twice per run: minutes of wall clock, and on 2026-09-09 a slow link
# turned it into two failures about the identity gate and a log rename, neither
# of which was the thing under test.
#
# Exported once here rather than fixed per case, so a case added later cannot
# reintroduce it by forgetting. A case that wants its own origin still sets one.
suite_origin="$stub_dir/suite-origin.git"
git init -q --bare "$suite_origin"
export MAINTAINER_REPO_ORIGIN="$suite_origin"

# The verbs bin/maintainer dispatches, read from its COMMANDS table by importing
# it, which `--help` lets it do with no profile deployed. Prints "<verb> <0|1>",
# the second field being whether run.sh calls it and a person does not. Every
# test that needs the verb list reads it here, so there is one source in the
# tests as well as in the tool. Fails loudly on an unreadable table.
cli_table() {
    python3 -B - "$root/bin/maintainer" <<'PYEOF' || return 1
import importlib.machinery, importlib.util, sys
path = sys.argv[1]
sys.argv = ["maintainer", "--help"]
loader = importlib.machinery.SourceFileLoader("maintainer_cli", path)
spec = importlib.util.spec_from_loader("maintainer_cli", loader)
m = importlib.util.module_from_spec(spec)
loader.exec_module(m)
for verb, c in m.COMMANDS.items():
    print(verb, int(c.internal))
PYEOF
}

# The cases live in tests/suites/, one file per topic, and this file sources them in
# the order below, in THIS process, so the counters, the helpers, $stub_dir and every
# variable a case leaves behind are shared exactly as they were when the cases were
# one file. The reason for the split is the linter: one shellcheck over a single
# 12,000 line file peaked near 10 GB, and each part stays far below that.
#
# The list is explicit, never a glob. The driver fails the suite, naming the part, when a
# listed part is missing, is listed twice, does not start with a bash shebang (the shell
# sweeps select files by it, so a part without one is never linted), records no case, or
# when a file in tests/suites/ is not listed. The order the parts ran in must equal the
# list. A part that ends the shell (`exit`, in any form) never reaches the summary, and the
# EXIT trap set above turns that into a failure. How many cases a part records is not
# pinned here, because loops over profiles, suites and programs change it whenever that
# data changes; the exact total is pinned by the `tests` figure that check_claims.sh derives.
mx_parts=(
    01-run-identity-gate.sh
    02-setup-walls-merge-gate.sh
    03-prompts-installers-walls.sh
    04-timers-status-screen.sh
    05-merge-gate-dependency-bump.sh
    06-verify-receipts-mcp.sh
    07-trust-offers-budgets.sh
    08-issue-claims.sh
    09-audit-trail-tree-under-test.sh
    10-tunables-docs-stalled.sh
    11-leases-deps-characterization.sh
)
# Every file whose text the suite reads about itself: this driver and each part.
suite_files=("$suite_root/tests/run-tests.sh")
for _mxp_name in "${mx_parts[@]}"; do suite_files+=("$suite_root/tests/suites/$_mxp_name"); done

# A file in tests/suites/ that the list does not name never runs, so it is a failure.
for _mxp_file in "$suite_root"/tests/suites/*.sh; do
    _mxp_known=0
    for _mxp_name in "${mx_parts[@]}"; do [ "${_mxp_file##*/}" = "$_mxp_name" ] && _mxp_known=1; done
    [ "$_mxp_known" = 1 ] || bad "tests/suites/${_mxp_file##*/} is not in the part list of run-tests.sh, so none of its cases would run"
done

mx_ran=(); mx_seen=" "
for _mxp_name in "${mx_parts[@]}"; do
    case "$mx_seen" in
        *" $_mxp_name "*) bad "the suite part tests/suites/$_mxp_name is listed twice in run-tests.sh, and a second run of it proves nothing"; continue ;;
    esac
    mx_seen="$mx_seen$_mxp_name "
    if [ ! -f "$suite_root/tests/suites/$_mxp_name" ]; then
        bad "the suite part tests/suites/$_mxp_name is missing, so its cases did not run"
        continue
    fi
    case "$(head -c 19 "$suite_root/tests/suites/$_mxp_name")" in
        '#!/usr/bin/env bash'|'#!/bin/bash'*) ;;
        *) bad "the suite part tests/suites/$_mxp_name does not start with a bash shebang, so the shell sweeps skip it" ;;
    esac
    _mxp_before=$((pass+fail+skip))
    # shellcheck source=/dev/null
    . "$suite_root/tests/suites/$_mxp_name"
    mx_ran+=("$_mxp_name")
    [ $((pass+fail+skip-_mxp_before)) -ge 1 ] \
        || bad "the suite part tests/suites/$_mxp_name recorded no case"
done
_mxp_a="${mx_ran[*]}"; _mxp_b="${mx_parts[*]}"
if [ "$_mxp_a" != "$_mxp_b" ] && [ "${#mx_ran[@]}" -eq "${#mx_parts[@]}" ]; then
    bad "the suite parts ran in a different order than run-tests.sh lists them"
fi
unset _mxp_a _mxp_b _mxp_file _mxp_known _mxp_name _mxp_before
mx_done=1

if [ "$skip" -gt 0 ]; then
    printf '\n  %d passed, %d failed, %d skipped%s\n' "$pass" "$fail" "$skip" "$(sc_note)"
    printf '  A skip is a case this machine could not run. CI requires zero.\n'
    [ "${MX_SKIP_SOURCE_COUPLED:-}" != 1 ] \
        || printf '  MX_SKIP_SOURCE_COUPLED=1 skipped the source-coupled cases on purpose; only a tree that is not shell may be run this way.\n'
else
    printf '\n  %d passed, %d failed%s\n' "$pass" "$fail" "$(sc_note)"
fi
[ "$fail" -eq 0 ]
