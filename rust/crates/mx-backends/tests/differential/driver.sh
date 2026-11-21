#!/usr/bin/env bash
# One backend function, run the way lib/run.sh runs it: the backend file is
# sourced, `set -uo pipefail` is on, PATH has $HOME/.local/bin and
# $HOME/.cargo/bin in front, and backend_run starts in $REPO_PATH.
#
# The differential test (tests/differential.rs) calls this once per operation.
# It hands over every input as DRV_<NAME>. Only the variables the run exports for
# every child (MAINTAINER_SETTINGS, MAINTAINER_CURSOR_DIR, GROQ_API_KEY) are
# exported here. Everything else (HOME, PROFILE_DIR, REPO_PATH, MAINTAINER_TASK,
# THINKING_<task>, OPENCODE_MODEL, ...) is a plain shell variable, so the agent's
# environment holds exactly what the backend and the run put there, and a stage
# that gains or loses a variable shows in the names the stub logs.
#
# Usage: driver.sh OP BACKEND_FILE [PROMPT MODEL LOG EXTRA RC_FILE EXPORTS_FILE]
#   OP is declare, wall, check or run. RC_FILE receives the exit status of
#   backend_run and EXPORTS_FILE the names backend_run exported (files, so the
#   standard output of the run stays the agent's).
#   DRV_REPO_NAME_ESC, when set, is a printf format that builds the last component
#   of REPO_PATH, under DRV_REPO_PARENT, for a name that is not UTF-8.
set -uo pipefail

op="$1"
file="$2"
prompt="${3:-}"
model="${4:-}"
log="${5:-}"
extra="${6:-}"
rc_file="${7:-}"
exports_file="${8:-}"

run_exported=" MAINTAINER_SETTINGS MAINTAINER_CURSOR_DIR GROQ_API_KEY "
for var in $(compgen -v DRV_); do
    name="${var#DRV_}"
    value="${!var}"
    unset "$var"
    # A name such as THINKING_readonly-review cannot be a shell variable, and the
    # bash backend never sees it either.
    [[ "$name" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || continue
    if [[ "$run_exported" == *" $name "* ]]; then
        export "$name=$value"
    else
        printf -v "$name" '%s' "$value"
    fi
done
if [ -n "${REPO_NAME_ESC:-}" ]; then
    # shellcheck disable=SC2059
    REPO_PATH="$REPO_PARENT/$(printf "$REPO_NAME_ESC")"
fi
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"

# shellcheck source=/dev/null
. "$file" || exit 97

case "$op" in
    declare)
        printf 'name=%s\n' "$(backend_name)"
        if declare -F backend_rehearsal >/dev/null; then echo "rehearsal=yes"; else echo "rehearsal=no"; fi
        if declare -F backend_allowed_tasks >/dev/null; then
            printf 'allowed=%s\n' "$(backend_allowed_tasks)"
        else
            echo "allowed=any"
        fi
        ;;
    wall)
        if declare -F backend_rehearsal_wall >/dev/null; then
            backend_rehearsal_wall
            printf 'rc=%s\n' "$?"
        else
            echo "rc=none"
        fi
        printf 'MAINTAINER_SETTINGS=%s\n' "${MAINTAINER_SETTINGS-<unset>}"
        printf 'MAINTAINER_CURSOR_DIR=%s\n' "${MAINTAINER_CURSOR_DIR-<unset>}"
        ;;
    check)
        msg="$(backend_check)"
        printf 'rc=%s\n' "$?"
        printf 'msg=%s\n' "$msg"
        ;;
    run)
        cd "$REPO_PATH" || exit 98
        declare -A before=()
        for n in $(compgen -e); do before[$n]=1; done
        backend_run "$prompt" "$model" "$log" "$extra"
        printf '%s\n' "$?" >"$rc_file"
        for n in $(compgen -e); do
            if [ -z "${before[$n]:-}" ]; then printf '%s\n' "$n"; fi
        done | sort >"$exports_file"
        ;;
    *)
        echo "driver.sh: unknown operation $op" >&2
        exit 99
        ;;
esac
