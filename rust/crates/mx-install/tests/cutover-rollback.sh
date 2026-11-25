#!/usr/bin/env bash
# Differential proof: `scripts/cutover.sh rollback` (bash, no Rust needed) against
# `install --rollback` (the Rust installer), on the same stamp.
#
# Each state is built in a scratch HOME by the real installers (the bash install.sh deploys the tree,
# the Rust install writes RUST_STAMP and swaps six stand-in programs), and then changed in one way: a
# program half-swapped, a saved original gone, a Rust binary changed, a symbolic link planted where a
# program, its saved original or the stamp sits, a stamp that is malformed or lists a program twice,
# a run in flight. The state is copied aside. The Rust installer rolls the first copy back; the second
# copy, in the same place (a stamp names absolute paths), is rolled back by the script. What is
# compared is the exit status, standard output, standard error (with the program's own name in front
# of a refusal mapped to the script's) and every path below HOME with its type, mode, link target and
# the hash of its bytes. A state marked "loose" compares the status and the files only, because the
# words of an operating-system error differ between the two programs.
#
# Usage: cutover-rollback.sh
# Env:   MX_TREE       the checkout (default: this one)
#        INSTALL_BIN   a built `install` (default: built here with cargo, release, locked, offline)
#        MX_SCRATCH    where the scratch directory goes (default ~/.cache/magent-wt-scratch/p6a)
# Exit:  0 when every state matched, 1 on a difference, 2 on a fault of the harness.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
MX_TREE="${MX_TREE:-$root}"
scratch_base="${MX_SCRATCH:-$HOME/.cache/magent-wt-scratch/p6a}"

fault() { printf 'cutover-rollback: %s\n' "$1" >&2; exit 2; }
for f in install.sh scripts/cutover.sh lib/run.sh scripts/render-settings.py; do
    [ -f "$MX_TREE/$f" ] || fault "no $f under MX_TREE=$MX_TREE"
done
for tool in python3 flock sha256sum sed find sort cmp diff bash mktemp cp rm mv ln readlink stat; do
    command -v "$tool" >/dev/null 2>&1 || fault "the host has no '$tool'"
done

install_bin="${INSTALL_BIN:-}"
if [ -z "$install_bin" ]; then
    command -v cargo >/dev/null 2>&1 || fault "no INSTALL_BIN and no cargo to build one"
    target="${CARGO_TARGET_DIR:-$root/rust/target}"
    out="$(cd "$root/rust" && CARGO_TARGET_DIR="$target" CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo build --release --locked --offline -p mx-install --bin install 2>&1)"; rc=$?
    [ "$rc" = 0 ] || fault "cargo build exited $rc: $(tail -n 5 <<<"$out")"
    install_bin="$target/release/install"
fi
[ -x "$install_bin" ] || fault "$install_bin is not an executable file"

mkdir -p "$scratch_base" || fault "cannot make $scratch_base"
work="$(mktemp -d "$scratch_base/diff-rollback.XXXXXX")" || fault "cannot make a scratch directory"
holder=""
# shellcheck disable=SC2317  # the trap runs this
cleanup() {
    if [ -n "$holder" ] && kill -0 "$holder" 2>/dev/null; then kill "$holder" 2>/dev/null; wait "$holder" 2>/dev/null; fi
    chmod -R u+rwX "$work" 2>/dev/null; rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

H="$work/home"                      # the HOME of every run: a stamp names absolute paths
SNAP="$work/snap"
RB="$work/rustbin"                  # six stand-in "Rust" programs
mkdir -p "$RB"
NAMES="maintainer-merge maintainer-doctor maintainer-repo run run-instance alert"
for n in $NAMES; do printf '#!/bin/sh\necho rust %s\n' "$n" > "$RB/$n"; chmod 755 "$RB/$n"; done

# The same PATH for both programs. A state may take flock away by using its own.
tools="$work/tools"; mkdir -p "$tools"
for t in bash sh env cp rm mv mkdir ln ls cat chmod touch sed grep awk sort tr od head tail dirname basename readlink git python3 date uname id flock timeout wc stat sha256sum mktemp cut find true false xargs sleep seq printf expr; do
    p="$(type -P "$t")" || continue
    ln -s "$p" "$tools/$t"
done
TOOLS_PATH="$tools"

as_install() {  # ARGS: the Rust installer, under the name of install.sh
    env -i HOME="$H" PATH="${RUN_PATH:-$TOOLS_PATH}" LC_ALL=C TZ=UTC \
        bash -c 'exec -a "$1" "$2" "${@:3}"' _ "$MX_TREE/install.sh" "$install_bin" "$@"
}
as_cutover() {
    env -i HOME="$H" PATH="${RUN_PATH:-$TOOLS_PATH}" LC_ALL=C TZ=UTC \
        bash "$MX_TREE/scripts/cutover.sh" rollback "$@"
}

plain_install() {
    chmod -R u+rwX "$H" 2>/dev/null; rm -rf "$H"; mkdir -p "$H"
    env -i HOME="$H" PATH="$TOOLS_PATH" LC_ALL=C TZ=UTC bash "$MX_TREE/install.sh" > "$work/plain.out" 2> "$work/plain.err" \
        || fault "the bash install.sh failed: $(head -3 "$work/plain.err")"
}
swap() {
    as_install --rust --rust-bin "$RB" > "$work/swap.out" 2> "$work/swap.err" || fault "install --rust failed: $(head -3 "$work/swap.err")"
}

B="" S=""
set_paths() { B="$H/.local/bin"; S="$H/.local/share/maintainer"; }
inst() { case "$1" in run|run-instance|alert) printf '%s' "$S/$1.sh" ;; *) printf '%s' "$B/$1" ;; esac; }

listing() {  # $1 = directory: type, path, mode, link target and the hash of the bytes
    ( cd "$1" && find . -mindepth 1 -printf '%y %p %m %l\n' | LC_ALL=C sort
      find . -type f -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum )
}

# --- the states --------------------------------------------------------------------------------------

# Each state function starts from a plain bash install and ends in the state to roll back.
st_all()            { swap; }
st_nostamp()        { :; }
st_after_rollback() { swap; as_install --rollback >/dev/null 2>&1 || fault "the setup rollback failed"; }
st_half() {
    swap; local n
    for n in maintainer-merge maintainer-doctor maintainer-repo; do mv "$(inst "$n").sh" "$(inst "$n")"; done
}
st_never_started() { swap; cp -p "$(inst alert).sh" "$(inst alert)"; }
st_restored_already() { swap; mv "$(inst alert).sh" "$(inst alert)"; }
st_missing_backup() { swap; rm "$(inst maintainer-repo).sh"; }
st_changed() { swap; printf 'a newer build\n' > "$(inst run)"; }
st_readonly_share() { swap; chmod 555 "$S"; }
st_link_installed_copy() { swap; cp "$(inst maintainer-doctor)" "$work/copy-of-the-build"; rm "$(inst maintainer-doctor)"; ln -s "$work/copy-of-the-build" "$(inst maintainer-doctor)"; }
st_link_installed_other() { swap; printf 'other\n' > "$work/other"; rm "$(inst maintainer-doctor)"; ln -s "$work/other" "$(inst maintainer-doctor)"; }
st_link_backup() { swap; mv "$(inst maintainer-repo).sh" "$work/real-original"; ln -s "$work/real-original" "$(inst maintainer-repo).sh"; }
st_link_backup_dangling() { swap; rm "$(inst maintainer-repo).sh"; ln -s /nonexistent/original "$(inst maintainer-repo).sh"; }
st_link_stamp() { swap; mv "$S/RUST_STAMP" "$work/stamp-copy"; ln -s "$work/stamp-copy" "$S/RUST_STAMP"; }
st_link_unlisted() {
    # the stamp lists one program; a link sits at another, which is none of the rollback's business
    swap; head -1 "$S/RUST_STAMP" > "$S/RUST_STAMP.new"; mv "$S/RUST_STAMP.new" "$S/RUST_STAMP"
    printf 'other\n' > "$work/other"; rm "$(inst run)"; ln -s "$work/other" "$(inst run)"
}
st_stamp_text() { swap; printf '%s' "$STAMP_TEXT" > "$S/RUST_STAMP"; }
st_stamp_duplicate() { swap; head -1 "$S/RUST_STAMP" >> "$S/RUST_STAMP"; }
st_stamp_directory() { swap; rm "$S/RUST_STAMP"; mkdir "$S/RUST_STAMP"; }
st_stamp_uppercase() { swap; sed -E 's/^(.{64})/\U\1/' "$S/RUST_STAMP" > "$S/RUST_STAMP.new"; mv "$S/RUST_STAMP.new" "$S/RUST_STAMP"; }
st_stamp_unterminated() { swap; printf '%s' "$(cat "$S/RUST_STAMP")" > "$S/RUST_STAMP.new"; mv "$S/RUST_STAMP.new" "$S/RUST_STAMP"; }
st_stamp_other_home() { swap; sed -i "s|$H|/other/home|" "$S/RUST_STAMP"; }
st_stamp_nul() { swap; printf '%s\0x\n' "$(head -c 64 "$S/RUST_STAMP")" > "$S/RUST_STAMP"; }
# A state that needs a held run lock sets NEED_LOCK. The lock is taken again before each of the two
# runs: a copy of the state has a new lock file, and a lock on the old one holds nothing.
st_busy() { swap; NEED_LOCK=1; }
hold_lock() {
    local lock="$H/.local/state/magent-maint/run.lock"
    mkdir -p "$(dirname "$lock")"
    rm -f "$work/lock-ready"
    ( exec 9> "$lock"; flock -n 9 || exit 3; : > "$work/lock-ready"; exec sleep 60 ) &
    holder=$!
    for _ in $(seq 1 100); do [ -e "$work/lock-ready" ] && break; sleep 0.1; done
    [ -e "$work/lock-ready" ] || fault "the lock holder did not start"
    [ "$(readlink "/proc/$holder/exe")" = "$(readlink -f "$(type -P sleep)")" ] || fault "the lock holder is not the sleep that holds the lock"
}
release_lock() { if [ -n "$holder" ]; then kill "$holder" 2>/dev/null; wait "$holder" 2>/dev/null; holder=""; fi; }
st_noflock() { swap; mkdir -p "$work/noflock"; local t; for t in "$tools"/*; do [ "${t##*/}" = flock ] || ln -sf "$(readlink "$t")" "$work/noflock/${t##*/}"; done; RUN_PATH="$work/noflock"; }

total=0; failed=0
# check NAME STATE_FUNCTION MODE ARGS...   MODE is exact or loose
check() {
    local name="$1" state="$2" mode="$3"; shift 3
    total=$((total + 1))
    RUN_PATH=""; holder=""; NEED_LOCK=0
    plain_install; set_paths; "$state"
    chmod -R u+rwX "$SNAP" 2>/dev/null; rm -rf "$SNAP"; cp -a "$H" "$SNAP"
    [ "$NEED_LOCK" = 0 ] || hold_lock
    ( as_install --rollback "$@" > "$work/rs.out" 2> "$work/rs.err" < /dev/null ); echo "$?" > "$work/rs.rc"
    release_lock
    listing "$H" > "$work/rs.tree" 2>/dev/null
    chmod -R u+rwX "$H" 2>/dev/null; rm -rf "$H"; cp -a "$SNAP" "$H"
    [ "$NEED_LOCK" = 0 ] || hold_lock
    ( as_cutover "$@" > "$work/cu.out" 2> "$work/cu.err" < /dev/null ); echo "$?" > "$work/cu.rc"
    release_lock
    listing "$H" > "$work/cu.tree" 2>/dev/null
    # the Rust program names itself install.sh; the script names itself cutover.sh
    sed -i '1s/^install\.sh: /cutover.sh: /' "$work/rs.err"
    local why=""
    [ "$(cat "$work/rs.rc")" = "$(cat "$work/cu.rc")" ] || why="exit status: install=$(cat "$work/rs.rc") cutover=$(cat "$work/cu.rc")"
    cmp -s "$work/rs.tree" "$work/cu.tree" || why="${why:+$why; }the files differ: $(diff "$work/rs.tree" "$work/cu.tree" | head -3 | tr '\n' '|')"
    if [ "$mode" = exact ]; then
        cmp -s "$work/rs.out" "$work/cu.out" || why="${why:+$why; }stdout differs: $(diff "$work/rs.out" "$work/cu.out" | head -3 | tr '\n' '|' | cut -c1-200)"
        cmp -s "$work/rs.err" "$work/cu.err" || why="${why:+$why; }stderr differs: $(diff "$work/rs.err" "$work/cu.err" | head -3 | tr '\n' '|' | cut -c1-200)"
    fi
    # a case that ran nothing proves nothing: the Rust side must have said something
    [ -s "$work/rs.out" ] || [ -s "$work/rs.err" ] || why="${why:+$why; }the Rust installer printed nothing"
    if [ -z "$why" ]; then
        printf 'PASS  %s (exit %s)\n' "$name" "$(cat "$work/rs.rc")"
    else
        failed=$((failed + 1)); printf 'FAIL  %s: %s\n' "$name" "$why"
    fi
}

GOOD="$(printf 'a%.0s' $(seq 1 64))"
check "all six swapped"                                     st_all exact
check "all six swapped, --dry-run"                          st_all exact --dry-run
check "no stamp, never swapped"                             st_nostamp exact
check "a second rollback finds no stamp"                    st_after_rollback exact
check "three of six swapped, three restored already"        st_half exact
check "a swap that never began (copy equals installed)"     st_never_started exact
check "a program restored by an interrupted rollback"       st_restored_already exact
check "a saved original is missing"                         st_missing_backup exact
check "an installed file changed since the stamp"           st_changed exact
check "a read-only share directory stops the rollback"      st_readonly_share exact
check "a link at an installed program, to a copy of the build" st_link_installed_copy exact
check "a link at an installed program, to another file"     st_link_installed_other exact
check "a link at a saved original"                          st_link_backup exact
check "a dangling link at a saved original"                 st_link_backup_dangling exact
check "RUST_STAMP is a link"                                st_link_stamp exact
check "a link at a program the stamp does not list"         st_link_unlisted exact
STAMP_TEXT=$'this is not a stamp\n';              check "a stamp line that is not a stamp line"    st_stamp_text exact
STAMP_TEXT=$'abc123  /x/y\n';                     check "a stamp line with a short hash"           st_stamp_text exact
STAMP_TEXT="$GOOD /x/y"$'\n';                     check "a stamp line with one space"              st_stamp_text exact
STAMP_TEXT="$GOOD  /etc/passwd"$'\n';             check "a stamp that names /etc/passwd"           st_stamp_text exact
STAMP_TEXT="";                                    check "an empty stamp"                           st_stamp_text exact
STAMP_TEXT=$'\n\n';                               check "a stamp of blank lines"                   st_stamp_text exact
check "a stamp that lists a program twice"                  st_stamp_duplicate exact
check "RUST_STAMP is a directory"                           st_stamp_directory loose
check "a stamp with upper-case hex"                         st_stamp_uppercase exact
check "a stamp whose last line has no newline"              st_stamp_unterminated exact
check "a stamp written for another HOME"                    st_stamp_other_home exact
check "a stamp with a NUL byte"                             st_stamp_nul loose
check "a run in flight"                                     st_busy exact
check "a run in flight, --force"                            st_busy exact --force
check "a run in flight, --dry-run"                          st_busy exact --dry-run
check "no flock on PATH"                                    st_noflock exact

printf 'cutover-rollback: %d states, %d differ\n' "$total" "$failed"
[ "$total" -gt 0 ] || { echo "RESULT: FAIL (no state ran)"; exit 1; }
if [ "$failed" = 0 ]; then echo "RESULT: PASS"; exit 0; fi
echo "RESULT: FAIL"; exit 1
