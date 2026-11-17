# shellcheck shell=bash
# The characterization suite, in a container: tests/run-characterization.sh,
# which runs the case files under tests/characterization/.
#
# Why it is a suite of its own. tests/run-tests.sh does not pin what the
# orchestrator, the backends, the failure-shape classifier, the installers, the
# doctor or the repo tool do. The characterization cases pin that, and a change
# to lib/backends/*, lib/failure-shapes.json or lib/run.sh could break a pinned
# behaviour with every other check green. Under the shell suite the mutated run
# of such a change stays green and the gate refuses with THE GUARD DOES NOT
# BITE, so no receipt was reachable for the class. Name this suite to prove one:
#
#   maintainer-merge verify <pr> <sha> run-backends.sh '<sed>' characterization
#
# It is never inferred. pick_suite and coverage_map skip a suite that defines
# suite_named_only, because this suite covers the same bin/ and lib/ paths the
# shell suite does, and an inferred choice would move every shell pull request
# onto a 155 s suite that does not run the shell suite's cases.

suite_named_only() { return 0; }

# The paths the cases exercise: the five programs under bin/ (the gate, the
# orchestrator, the doctor, the repo tool, the MCP server), everything under
# lib/ (run.sh, the classifier's failure-shapes.json, the profile loader and
# every backend in lib/backends/), the installers, the platform units the
# installer cases read, and the evals the doctor cases run. A path outside this
# list is not run by any case, so a receipt for it would prove nothing.
suite_covers() {
    case "$1" in
        bin/*|lib/*|install.sh|new-profile.sh|platform/*|evals/*) return 0 ;;
    esac
    return 1
}

# The same image as the shell suite, for the same reason: the cases build real
# git repositories and drive python for the JSON walls, and -slim has no git.
suite_image() { printf 'docker.io/library/python:3.12'; }

suite_needs() { printf 'bash python3 git'; }

# Same globs as the shell suite. Every command in bin/ has no extension, so
# 'maintainer*' is what lets a mutation reach them.
suite_mutate_glob() { printf '%s\n' '*.sh' '*.py' 'maintainer*' '*.json' '*.env' '*.md'; }

# The one rule for the gate's filter: a case file name, a family word, a dash,
# then letters, digits and dashes, ending in .sh. It cannot start with a dash,
# contain a slash, a space, a quote, a semicolon or a newline.
CHARACTERIZATION_FILTER_RULE='^[a-z]+-[a-z0-9-]+\.sh$'

# $1 = filter. $2 = the directory of case files, when the caller knows it.
# Returns 0 for a name that matches the rule and, when a directory is known,
# names a file there. The raw value is never printed.
suite_filter_ok() {
    local name="$1" dir="${2:-}"
    case "$name" in *$'\n'*|*$'\r'*) return 1 ;; esac
    [[ "$name" =~ $CHARACTERIZATION_FILTER_RULE ]] || return 1
    [ -z "$dir" ] || [ -f "$dir/$name" ] || return 1
    return 0
}

# Copied out of the read-only mount first: the cases write sandboxes under /tmp,
# and the runner is called with exactly one case file name. A refused filter
# becomes a command that prints the rule and exits 64, so the clean run fails
# and the gate shows the rule in the log tail. The name reaches the command
# only after suite_filter_ok has shown it holds nothing but [a-z0-9.-], and it
# is single-quoted there as well.
suite_command() {  # $1 = the case file name
    local dir="${CHARACTERIZATION_DIR:-}"
    [ -n "$dir" ] || [ -z "${work:-}" ] || dir="$work/tests/characterization"
    if ! suite_filter_ok "$1" "$dir"; then
        printf '%s\n' "printf 'characterization suite: the filter must name one case file matching ^[a-z]+-[a-z0-9-]+\\.sh\$ that exists under tests/characterization/\\n' >&2; exit 64"
        return 0
    fi
    printf 'cp -r /repo /tmp/w && cd /tmp/w && bash tests/run-characterization.sh %s\n' "'$1'"
}

# The runner prints "N cases, P passed, F failed". The count is P, the cases
# that passed, and a log without that line counts 0 so a run that printed
# nothing cannot read as evidence.
suite_ran() {  # $1 = the clean log
    local n; n="$(sed -n 's/^[0-9][0-9]* cases, \([0-9][0-9]*\) passed, [0-9][0-9]* failed.*/\1/p' "$1" | tail -1)"
    printf '%s' "${n:-0}"
}
