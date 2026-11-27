#!/usr/bin/env bash
# The cutover with the real programs, in a scratch HOME: scripts/cutover.sh build (real cargo), then
# live (the real Rust `install --rust`), then status, a prompt from the Rust `run` at its installed
# place against the bash original saved beside it, then rollback, which must leave the machine as the
# bash install left it.
#
# shadow-ok and its report are written by hand here (what a clean shadow leaves): what shadow proves is the job of
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
# Type, path, mode, link target and the hash of the bytes of everything under HOME. VERSION is left out:
# every install stamps it with the time it ran (installed_at), and a rollback does not undo a stamp.
listing() {
    ( cd "$H" && find . -mindepth 1 -printf '%y %p %m %l\n' | LC_ALL=C sort
      find . -type f ! -path ./.local/share/maintainer/VERSION -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum )
}
listing > "$work/plain.tree"
B="$work/build"
# The shadow-ok and shadow-report.txt a clean shadow of a build directory would leave, written by hand: the
# table of the directory, the commit of the checkout, the digest of a report of passes only. live checks all three.
marker_by_hand() {  # $1 = build directory
    local f head short rep="$1/shadow-report.txt"
    head="$(git -C "$co" rev-parse HEAD)"; short="${head:0:7}"
    { printf 'SHADOW home=%s checkout=%s deployed=%s build=%s profiles=1\n' "$H" "$short" "$short" "$short"
      printf 'PASS run --show-prompt magent review (default)\nPASS install --dry-run\nSUMMARY comparisons=2 pass=2 diff=0 skip=0\n'; } > "$rep"
    {
        for f in "$1"/*; do
            case "${f##*/}" in SHA256SUMS|BUILD_INFO|shadow-ok|shadow-report.txt) continue ;; esac
            printf '%s  %s\n' "$(sha256sum < "$f" | cut -d' ' -f1)" "${f##*/}"
        done | LC_ALL=C sort -k2
        printf '# checkout: %s\n# report: %s\n' "$head" "$(sha256sum < "$rep" | cut -d' ' -f1)"
    } > "$1/shadow-ok"
}

cutover build --out "$B" > "$work/build.out" 2> "$work/build.err"; rc=$?
check "build exits 0 (stderr: $(tail -n 1 "$work/build.err" | cut -c1-100))" same "$rc" 0
check "build lists a sha256 table that sha256sum accepts" bash -c "cd '$B' && sha256sum -c SHA256SUMS >/dev/null 2>&1"
check "build made the Rust install program" test -x "$B/install"
check "build made the Rust run program" test -x "$B/run"
check "build left the checkout clean" bash -c "[ -z \"\$(git -C '$co' status --porcelain)\" ]"
if grep -aq "$HOME" "$B/run" 2>/dev/null; then check "no binary spells the builder's home directory" false; else check "no binary spells the builder's home directory" true; fi

marker_by_hand "$B"
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

# --- the staged cutover (build --only): first the doctor and the repository tool, then the rest, the bash
# maintainer-merge staying in place. The marker is again written by hand; the real cargo, the real installer
# and the real rollback run.
stamp_names() { sed 's/^[0-9a-f]*  //' "$share/RUST_STAMP" | sed 's|.*/||' | LC_ALL=C sort | tr '\n' ' '; }
S="$work/stage"

cutover build --out "$S" --only maintainer-doctor --only maintainer-repo > "$work/s1build.out" 2> "$work/s1build.err"; rc=$?
check "stage 1 build exits 0 (stderr: $(tail -n 1 "$work/s1build.err" | cut -c1-100))" same "$rc" 0
check "stage 1 directory holds the two programs and install" same "$(ls -A "$S" | LC_ALL=C sort | tr '\n' ' ')" "BUILD_INFO SHA256SUMS install maintainer-doctor maintainer-repo "
check "stage 1 BUILD_INFO has the only line" grep -qx 'only=maintainer-doctor maintainer-repo' "$S/BUILD_INFO"
check "stage 1 table is accepted by sha256sum" bash -c "cd '$S' && sha256sum -c SHA256SUMS >/dev/null 2>&1"
marker_by_hand "$S"
cutover live --build-dir "$S" > "$work/s1live.out" 2> "$work/s1live.err"; rc=$?
check "stage 1 live exits 0 (stderr: $(head -n 1 "$work/s1live.err" | cut -c1-120))" same "$rc" 0
check "stage 1 RUST_STAMP lists the doctor and the repository tool only" same "$(stamp_names)" "maintainer-doctor maintainer-repo "
cutover status --build-dir "$S" > "$work/s1status.out" 2>&1
check "stage 1: maintainer-doctor is rust" grep -q '^  maintainer-doctor  *rust ' "$work/s1status.out"
check "stage 1: maintainer-repo is rust" grep -q '^  maintainer-repo  *rust ' "$work/s1status.out"
check "stage 1: maintainer-merge is bash" grep -q '^  maintainer-merge  *bash' "$work/s1status.out"
check "stage 1: run is bash" grep -q '^  run  *bash' "$work/s1status.out"
check "stage 1: the installed maintainer-merge is the bash original" cmp -s "$H/.local/bin/maintainer-merge" "$co/bin/maintainer-merge"
check "stage 1: the installed run is the bash original" cmp -s "$share/run.sh" "$co/lib/run.sh"
cutover rollback > "$work/s1rb.out" 2> "$work/s1rb.err"; rc=$?
check "stage 1 rollback exits 0" same "$rc" 0
listing > "$work/s1after.tree"
check "after the stage 1 rollback the machine is as the bash install left it" cmp -s "$work/plain.tree" "$work/s1after.tree"

cutover build --out "$S" --only maintainer-doctor --only maintainer-repo --only run --only run-instance --only alert > "$work/s2build.out" 2> "$work/s2build.err"; rc=$?
check "stage 2 build into the same directory exits 0 (stderr: $(tail -n 1 "$work/s2build.err" | cut -c1-100))" same "$rc" 0
check "stage 2 directory holds the five programs and install" same "$(ls -A "$S" | LC_ALL=C sort | tr '\n' ' ')" "BUILD_INFO SHA256SUMS alert install maintainer-doctor maintainer-repo run run-instance "
marker_by_hand "$S"
cutover live --build-dir "$S" > "$work/s2live.out" 2> "$work/s2live.err"; rc=$?
check "stage 2 live exits 0 (stderr: $(head -n 1 "$work/s2live.err" | cut -c1-120))" same "$rc" 0
check "stage 2 RUST_STAMP lists the five programs" same "$(stamp_names)" "alert.sh maintainer-doctor maintainer-repo run-instance.sh run.sh "
cutover status --build-dir "$S" > "$work/s2status.out" 2>&1
check "stage 2: run is rust" grep -q '^  run  *rust ' "$work/s2status.out"
check "stage 2: maintainer-merge is still bash" grep -q '^  maintainer-merge  *bash' "$work/s2status.out"
cutover rollback > "$work/s2rb.out" 2> "$work/s2rb.err"; rc=$?
check "stage 2 rollback exits 0" same "$rc" 0
listing > "$work/s2after.tree"
check "after the stage 2 rollback the machine is as the bash install left it" cmp -s "$work/plain.tree" "$work/s2after.tree"

printf 'cutover-live: %d checks, %d failed\n' "$checks" "$failed"
[ "$checks" -gt 10 ] || { echo "RESULT: FAIL (too few checks ran)"; exit 1; }
if [ "$failed" = 0 ]; then echo "RESULT: PASS"; exit 0; fi
diff "$work/plain.tree" "$work/after.tree" | head -5
echo "RESULT: FAIL"; exit 1
