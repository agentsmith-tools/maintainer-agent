#!/bin/bash
# A stand-in for claude, codex, cursor-agent, opencode and python3. The test copies
# this file under each program name and replaces @CFG@ with the directory that
# holds the call log and the behaviour files.
#
# One record per call goes to $CFG/calls.log. The record holds:
#   argv (one element per line, shell-quoted), the working directory, the names of
#   EVERY exported variable (so a variable the plan adds, or one it forgets, shows),
#   the value of the backend-relevant ones (a key as its length and sha256), whether
#   standard output and standard error are one open file description (`2>&1`) or two
#   (`>>f 2>>f`), and the length and sha256 of standard input.
# The record is written after standard input is read and after standard output is
# written, and before standard error is written, so two stubs of one pipeline log in
# a fixed order: the reader of a pipe logs once the writer has exited.
# Behaviour comes from files beside the log: NAME.rc (exit status), NAME.out
# and NAME.err (what to print). The default for the python3 stub, which stands
# for scripts/transcript.py, is to print nothing.
CFG='@CFG@'
name="${0##*/}"
tmp="$CFG/.in.$$"
cat >"$tmp"
sha="$(sha256sum <"$tmp")"
sha="${sha%% *}"
len="$(wc -c <"$tmp")"
rm -f "$tmp"

# Standard output first: whether the two descriptors share an offset can only be
# read after one of them has written. The kind of a descriptor is read in this
# shell and not inside $(...), where fd 1 would be the substitution's own pipe.
wrote=0
if [ -f "$CFG/$name.out" ]; then
    cat "$CFG/$name.out"
    if [ -s "$CFG/$name.out" ]; then wrote=1; fi
elif [ "$name" != python3 ]; then
    printf '%s-out\n' "$name"
    wrote=1
fi

kind=""
fd_kind() {
    if [ -p "/dev/fd/$1" ]; then kind="pipe"
    elif [ -f "/dev/fd/$1" ]; then kind="file"
    elif [ -c "/dev/fd/$1" ]; then kind="char"
    else kind="other"; fi
}
fd_pos() {
    local k v
    while read -r k v; do
        if [ "$k" = "pos:" ]; then echo "$v"; fi
    done <"/proc/$$/fdinfo/$1"
}
fd_kind 1
kind1="$kind"
fd_kind 2
kind2="$kind"
share="n/a"
if [ "$wrote" = 1 ] && [ "$kind1" = file ] && [ "$kind2" = file ] \
    && [ /dev/fd/1 -ef /dev/fd/2 ]; then
    if [ "$(fd_pos 1)" = "$(fd_pos 2)" ]; then share=shared; else share=separate; fi
fi

names=""
for v in $(compgen -e | sort); do
    case "$v" in _ | SHLVL | PWD | OLDPWD) continue ;; esac
    names="$names $v"
done

{
    printf 'CALL %s\n' "$name"
    printf 'ARGC %s\n' "$#"
    for a in "$@"; do printf 'ARG %q\n' "$a"; done
    printf 'CWD %s\n' "$(pwd -P)"
    printf 'ENVNAMES%s\n' "$names"
    for v in MAX_THINKING_TOKENS CLAUDE_CODE_SUBAGENT_MODEL CURSOR_CONFIG_DIR OPENCODE_CONFIG \
        MAINTAINER_SETTINGS MAINTAINER_CURSOR_DIR; do
        if [ -n "${!v+x}" ]; then printf 'ENV %s=%s\n' "$v" "${!v}"; else printf 'ENV %s=<unset>\n' "$v"; fi
    done
    if [ -n "${GROQ_API_KEY+x}" ]; then
        k="$(printf '%s' "$GROQ_API_KEY" | sha256sum)"
        printf 'ENV GROQ_API_KEY=len:%s sha:%s\n' "${#GROQ_API_KEY}" "${k%% *}"
    else
        printf 'ENV GROQ_API_KEY=<unset>\n'
    fi
    # Where the filter's own streams go is decision P2B-3, pinned in the test.
    if [ "$name" = python3 ]; then
        printf 'FDS n/a\n'
    else
        printf 'FDS stdout=%s stderr=%s %s\n' "$kind1" "$kind2" "$share"
    fi
    printf 'STDIN %s %s\n' "$len" "$sha"
} >>"$CFG/calls.log"

if [ "$name" = python3 ]; then
    # What scripts/transcript.py does with its two arguments: append to the log
    # it was given and to the commands file.
    printf '[filter read %s bytes, sha256 %s]\n' "$len" "$sha" >>"$2"
    printf 'commands for %s\n' "$sha" >>"$3"
fi

if [ -f "$CFG/$name.err" ]; then
    cat "$CFG/$name.err" >&2
elif [ "$name" != python3 ]; then
    printf '%s-err\n' "$name" >&2
fi
rc=0
if [ -f "$CFG/$name.rc" ]; then read -r rc <"$CFG/$name.rc"; fi
exit "$rc"
