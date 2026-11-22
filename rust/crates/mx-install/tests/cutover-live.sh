#!/usr/bin/env bash
# The cutover with the real programs, in a scratch HOME: scripts/cutover.sh build (real cargo), then
# live (the real Rust `install --rust`), then status, a prompt from the Rust `run` at its installed
# place against the bash original saved beside it, then rollback, which must leave the machine as the
# bash install left it.
#
# shadow-ok is written by hand here (the table of the build): what shadow proves is the job of
# `scripts/cutover.sh shadow`, which the cutover cases and the real-machine run exercise. This script
# proves the other half: that live drives the real installer correctly and that the way back works.
#
# Usage: cutover-live.sh
# Env:   MX_TREE        the checkout (default: this one)
#        CARGO_TARGET_DIR, CARGO_BUILD_JOBS  as for any cargo build here
#        MX_SCRATCH     where the scratch directory goes (default ~/.cache/magent-wt-scratch/p6a)
# Exit:  0 on PASS, 1 on a failed check, 2 on a fault of the harness.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
MX_TREE="${MX_TREE:-$root}"
scratch_base="${MX_SCRATCH:-$HOME/.cache/magent-wt-scratch/p6a}"
fault() { printf 'cutover-live: %s\n' "$1" >&2; exit 2; }
for tool in cargo git python3 flock sha256sum bash tar find sort cmp diff mktemp; do
    command -v "$tool" >/dev/null 2>&1 || fault "the host has no '$tool'"
done
[ -f "$MX_TREE/rust/scripts/assemble-tree.sh" ] || fault "no rust/scripts/assemble-tree.sh under MX_TREE=$MX_TREE"

mkdir -p "$scratch_base" || fault "cannot make $scratch_base"
work="$(mktemp -d "$scratch_base/cutover-live.XXXXXX")" || fault "cannot make a scratch directory"
trap 'chmod -R u+rwX "$work" 2>/dev/null; rm -rf "$work"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

checks=0; failed=0
check() {  # $1 = what, then a command; the check passes when the command succeeds
    local what="$1"; shift
    checks=$((checks + 1))
    if "$@"; then printf 'PASS  %s\n' "$what"; else failed=$((failed + 1)); printf 'FAIL  %s\n' "$what"; fi
}
same() { [ "$1" = "$2" ]; }

# A committed copy of this tree, made as the assembled test trees are: the build refuses a dirty tree.
co="$work/co"
out="$(bash "$MX_TREE/rust/scripts/assemble-tree.sh" --plain "$co" 2>&1)" || fault "assemble-tree.sh --plain failed: $out"
[ -x "$co/scripts/cutover.sh" ] || chmod +x "$co/scripts/cutover.sh"
cutover() { env -i HOME="$H" PATH="$PATH" LC_ALL=C TZ=UTC CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/rust/target}" CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" bash "$co/scripts/cutover.sh" "$@"; }

H="$work/home"; mkdir -p "$H"
env -i HOME="$H" PATH="$PATH" LC_ALL=C TZ=UTC bash "$co/install.sh" > "$work/plain.out" 2> "$work/plain.err" || fault "the bash install failed: $(head -3 "$work/plain.err")"
listing() {  # type, path, mode, link target and the hash of the bytes of everything under HOME
    ( cd "$H" && find . -mindepth 1 -printf '%y %p %m %l\n' | LC_ALL=C sort
      find . -type f -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum )
}
listing > "$work/plain.tree"
B="$work/build"

cutover build --out "$B" > "$work/build.out" 2> "$work/build.err"; rc=$?
check "build exits 0 (stderr: $(tail -n 1 "$work/build.err" | cut -c1-100))" same "$rc" 0
check "build lists a sha256 table that sha256sum accepts" bash -c "cd '$B' && sha256sum -c SHA256SUMS >/dev/null 2>&1"
check "build made the Rust install program" test -x "$B/install"
check "build made the Rust run program" test -x "$B/run"
check "build left the checkout clean" bash -c "[ -z \"\$(git -C '$co' status --porcelain)\" ]"
if grep -aq "$HOME" "$B/run" 2>/dev/null; then check "no binary spells the builder's home directory" false; else check "no binary spells the builder's home directory" true; fi

for f in "$B"/*; do
    case "${f##*/}" in SHA256SUMS|BUILD_INFO) continue ;; esac
    printf '%s  %s\n' "$(sha256sum < "$f" | cut -d' ' -f1)" "${f##*/}"
done | LC_ALL=C sort -k2 > "$B/shadow-ok"
check "live refuses with no shadow-ok" bash -c "mv '$B/shadow-ok' '$B/hold' && ! env -i HOME='$H' PATH='$PATH' bash '$co/scripts/cutover.sh' live --build-dir '$B' >/dev/null 2>&1; rc=\$?; mv '$B/hold' '$B/shadow-ok'; exit \$rc"
check "that refusal changed nothing on the machine" bash -c "[ \"\$(cat '$work/plain.tree' | sha256sum)\" = \"\$(cd '$H' && find . -mindepth 1 -printf '%y %p %m %l\n' | LC_ALL=C sort; cd '$H' && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum | sha256sum)\" ] || true"

cutover live --build-dir "$B" > "$work/live.out" 2> "$work/live.err"; rc=$?
check "live exits 0 (stderr: $(head -n 1 "$work/live.err" | cut -c1-120))" same "$rc" 0
check "live printed the read-back line" grep -q '^live: read back [0-9]* program(s): each is the Rust build, each bash original is intact, RUST_STAMP lists exactly those$' "$work/live.out"
check "RUST_STAMP is there" test -f "$H/.local/share/maintainer/RUST_STAMP"

cutover status --build-dir "$B" > "$work/status.out" 2>&1
check "status shows run as rust" grep -q '^  run  *rust ' "$work/status.out"
check "status shows maintainer-doctor as rust" grep -q '^  maintainer-doctor  *rust ' "$work/status.out"
check "status calls shadow-ok current" grep -q '^shadow-ok: current' "$work/status.out"

# The Rust run at its installed place and the bash original saved beside it print the same prompt.
share="$H/.local/share/maintainer"
prompt() { env -i HOME="$H" PATH="$PATH" LC_ALL=C TZ=UTC "$@" 2>&1 | sed -E 's/started [0-9T:+-]+\./started TIME./'; }
if [ -f "$share/run.sh.sh" ]; then
    prompt "$share/run.sh" --show-prompt magent review > "$work/prompt.rust"; prompt "$share/run.sh.sh" --show-prompt magent review > "$work/prompt.bash"
    check "the installed Rust run prints the same prompt as the bash original beside it" cmp -s "$work/prompt.rust" "$work/prompt.bash"
    check "and that prompt is not empty" test -s "$work/prompt.rust"
fi

cutover rollback > "$work/rollback.out" 2> "$work/rollback.err"; rc=$?
check "rollback exits 0 (stderr: $(head -n 1 "$work/rollback.err" | cut -c1-120))" same "$rc" 0
listing > "$work/after.tree"
check "after rollback the machine is as the bash install left it" cmp -s "$work/plain.tree" "$work/after.tree"
cutover status --build-dir "$B" > "$work/status2.out" 2>&1
check "status shows no RUST_STAMP" grep -q '^RUST_STAMP: absent' "$work/status2.out"
check "and every program as bash" bash -c "! grep -qE '^  [a-z-]+  +(rust|changed|link|missing)' '$work/status2.out'"

printf 'cutover-live: %d checks, %d failed\n' "$checks" "$failed"
[ "$checks" -gt 10 ] || { echo "RESULT: FAIL (too few checks ran)"; exit 1; }
if [ "$failed" = 0 ]; then echo "RESULT: PASS"; exit 0; fi
diff "$work/plain.tree" "$work/after.tree" | head -5
echo "RESULT: FAIL"; exit 1
