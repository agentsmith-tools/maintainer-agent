#!/usr/bin/env bash
# Differential proof: the Rust maintainer-repo against bin/maintainer-repo.
#
# Each scenario (differential_scenarios.py) builds real git repositories, a bare
# origin, a gh stub, a logging git and a fake `maintainer fixup-check`, then runs
# the bash program and the Rust program on identical copies of that fixture and
# compares standard output, standard error, the exit code, the global call log
# (every gh, git and fixup-check call with argv, the environment names that
# matter and stdin, in order) and the state tree (refs, config, working tree and
# state directory). A last group of scenarios pins the differences the port keeps
# on purpose (decisions P1H-1 and P1H-8 in the pull request), asserted on both
# sides.
#
# Usage: differential.sh
# Env:   MX_TREE       tree under test (default: this checkout); its lib/ and
#                      bin/maintainer-repo are the bash side
#        MX_SCRATCH    where the scratch directory goes (default: TMPDIR, then /tmp)
#        DIFF_SEED     the seed (default: from the clock; printed, so a run repeats)
#        DIFF_ONLY     comma-separated scenario name fragments (prune-007,fixup-0)
#        DIFF_RUST_BIN a prebuilt maintainer-repo to test (default: built from this
#                      checkout; the red proof points this at a mutated build)
#        DIFF_VERBOSE  1 prints a line for every scenario
# Exit:  0 when every comparison matched, 1 on a difference, 2 on a harness fault.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ws="$(cd "$here/../../.." && pwd)"
root="$(cd "$ws/.." && pwd)"
MX_TREE="${MX_TREE:-$root}"
seed="${DIFF_SEED:-$(date +%s)}"
seed=$(( seed % 1000000 ))

for f in bin/maintainer-repo lib/profile.sh lib/defaults.env lib/failure-shapes.json; do
    [ -f "$MX_TREE/$f" ] || { echo "differential: no $f under MX_TREE=$MX_TREE" >&2; exit 2; }
done
for tool in python3 git timeout bash; do
    command -v "$tool" >/dev/null 2>&1 || { echo "differential: the host has no '$tool'" >&2; exit 2; }
done

rust_bin="${DIFF_RUST_BIN:-}"
if [ -z "$rust_bin" ]; then
    export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ws/target}"
    built="$(cd "$ws" && cargo build --release --locked --offline -q -p mx-repo --bin maintainer-repo 2>&1)"; rc=$?
    [ "$rc" = 0 ] || { printf '%s\ndifferential: cargo build failed (exit %s)\n' "$built" "$rc" >&2; exit 2; }
    rust_bin="$CARGO_TARGET_DIR/release/maintainer-repo"
fi
[ -x "$rust_bin" ] || { echo "differential: $rust_bin is not an executable" >&2; exit 2; }

scratch="$(mktemp -d "${MX_SCRATCH:-${TMPDIR:-/tmp}}/mx-repo-diff.XXXXXX")" || exit 2
# The trap runs this function; shellcheck cannot see that (SC2317).
# shellcheck disable=SC2317
cleanup() { chmod -R u+rwX "$scratch" 2>/dev/null; rm -rf "$scratch"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

printf 'differential: seed %s (repeat with DIFF_SEED=%s)\n' "$seed" "$seed"
printf 'differential: bash %s, %s, python %s\n' "$BASH_VERSION" "$(git --version)" "$(python3 -c 'import sys; print(sys.version.split()[0])')"
printf 'differential: rust binary %s\n' "$rust_bin"
python3 -I -c 'import sys; sys.exit(0)' || exit 2
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH="$here" python3 "$here/differential.py" "$MX_TREE" "$scratch" "$rust_bin" "$seed" "${DIFF_ONLY:-}"
exit $?
