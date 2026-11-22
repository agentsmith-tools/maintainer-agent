#!/usr/bin/env bash
# Proves that assemble-tree.sh never writes outside DEST.
#
# It builds a throwaway repository (a one-crate workspace whose binary is called
# `prog`, and a bin/prog) three times, each with a tracked symlink that points at
# a file or a directory outside the repository, runs the real helper from that
# repository, and checks that the outside file and directory are byte for byte
# what they were:
#
#   file-link   bin/prog is a symlink to an outside file. The helper replaces the
#               link in the copy and leaves the outside file alone.
#   rust-bin    .rust-bin is a symlink to an outside directory. The helper refuses.
#   bin-link    bin is a symlink to an outside directory. The helper refuses.
#   run-wrapper the crate is called `run`: its wrapper replaces lib/run.sh, carries
#               no MAINTAINER_CHILD_ENV line, and the binary is in the copy.
#   run-lib     lib is a symlink to an outside directory. Building `run` refuses.
#   install-wrapper the crate is called `install`: its wrapper replaces install.sh at the
#               root of the tree and carries the MAINTAINER_CHILD_ENV line.
#   platform-link platform is a symlink to an outside directory. Building `alert` refuses.
#
# Prints `assemble-tree test: N checks, F failed`; exit 0 only with N > 0 and F = 0.
# MX_SCRATCH moves the scratch directory; CARGO_TARGET_DIR is shared with the caller.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
helper="$here/assemble-tree.sh"
[ -f "$helper" ] || { echo "test-assemble-tree: no assemble-tree.sh beside this file" >&2; exit 2; }

scratch="$(mktemp -d "${MX_SCRATCH:-${TMPDIR:-/tmp}}/assemble-test.XXXXXX")" || exit 2
# shellcheck disable=SC2317  # the trap runs it
cleanup() { chmod -R u+rwX "$scratch" 2>/dev/null; rm -rf "$scratch"; }
trap cleanup EXIT
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$scratch/target}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"

N=0; F=0
check() {  # $1 = label, then a command that must succeed
    local label="$1"; shift
    N=$((N + 1))
    if "$@"; then printf 'ok    %s\n' "$label"; else F=$((F + 1)); printf 'FAIL  %s\n' "$label"; fi
}

# fixture NAME: a repository under $scratch/NAME with an outside directory beside it.
fixture() {
    local d="$scratch/$1"
    mkdir -p "$d/repo/rust/scripts" "$d/repo/rust/crates/prog/src" "$d/repo/bin" "$d/repo/lib" "$d/outside/dir" || exit 2
    cp "$helper" "$d/repo/rust/scripts/assemble-tree.sh"
    printf '[workspace]\nresolver = "3"\nmembers = ["crates/*"]\n' > "$d/repo/rust/Cargo.toml"
    printf '[package]\nname = "prog"\nversion = "0.0.0"\nedition = "2024"\n\n[[bin]]\nname = "prog"\npath = "src/main.rs"\n' > "$d/repo/rust/crates/prog/Cargo.toml"
    printf 'fn main() {}\n' > "$d/repo/rust/crates/prog/src/main.rs"
    (cd "$d/repo/rust" && cargo generate-lockfile --offline -q) || exit 2
    printf '#!/usr/bin/env bash\necho bash prog\n' > "$d/repo/bin/prog"
    printf '#!/usr/bin/env bash\n' > "$d/repo/bin/maintainer-merge"
    printf 'VICTIM\n' > "$d/outside/victim"
    printf 'VICTIM\n' > "$d/outside/dir/victim"
}
commit_fixture() {
    ( cd "$scratch/$1/repo" && git init -q -b main && git add -A \
        && git -c user.name=t -c user.email=t@example.invalid commit -q -m fixture ) || exit 2
}
digest() { find "$1" -type f -print0 | sort -z | xargs -0 sha256sum | sha256sum; }

run_helper() {  # $1 = fixture name, $2 = the program (default prog); sets RC and OUT
    OUT="$(cd "$scratch/$1/repo" && bash rust/scripts/assemble-tree.sh "$scratch/$1/dest" "${2:-prog}" 2>&1)"; RC=$?
}
# runfix NAME: the fixture with its crate renamed to `run`, which stands for lib/run.sh.
runfix() {
    fixture "$1"
    sed -i 's/prog/run/g' "$scratch/$1/repo/rust/crates/prog/Cargo.toml"
    (cd "$scratch/$1/repo/rust" && cargo generate-lockfile --offline -q) || exit 2
    printf '#!/usr/bin/env bash\necho bash run\n' > "$scratch/$1/repo/lib/run.sh"
}

# 1. file-link
fixture file-link
rm "$scratch/file-link/repo/bin/prog"
ln -s "$scratch/file-link/outside/victim" "$scratch/file-link/repo/bin/prog"
commit_fixture file-link
before="$(digest "$scratch/file-link/outside")"
run_helper file-link
check "file-link: the helper finished (exit $RC: ${OUT: -120})" test "$RC" = 0
check "file-link: the outside file is unchanged" test "$(digest "$scratch/file-link/outside")" = "$before"
check "file-link: the copy holds a regular wrapper, not the link" test -f "$scratch/file-link/dest/bin/prog" -a ! -L "$scratch/file-link/dest/bin/prog"
check "file-link: the wrapper names the binary in the copy" grep -q "dest/.rust-bin/prog" "$scratch/file-link/dest/bin/prog"
check "file-link: the binary is in the copy" test -x "$scratch/file-link/dest/.rust-bin/prog"

# 2. rust-bin
fixture rust-bin
ln -s "$scratch/rust-bin/outside/dir" "$scratch/rust-bin/repo/.rust-bin"
commit_fixture rust-bin
before="$(digest "$scratch/rust-bin/outside")"
run_helper rust-bin
check "rust-bin: the helper refused (exit $RC)" test "$RC" != 0
check "rust-bin: it says why" grep -q "symlink" <<<"$OUT"
check "rust-bin: the outside directory is unchanged" test "$(digest "$scratch/rust-bin/outside")" = "$before"
check "rust-bin: no binary was written outside" test ! -e "$scratch/rust-bin/outside/dir/prog"

# 3. bin-link
fixture bin-link
rm -r "$scratch/bin-link/repo/bin"
cp -r "$scratch/bin-link/outside/dir" "$scratch/bin-link/outside/bindir"
printf '#!/usr/bin/env bash\n' > "$scratch/bin-link/outside/bindir/prog"
printf '#!/usr/bin/env bash\n' > "$scratch/bin-link/outside/bindir/maintainer-merge"
ln -s "$scratch/bin-link/outside/bindir" "$scratch/bin-link/repo/bin"
commit_fixture bin-link
before="$(digest "$scratch/bin-link/outside")"
run_helper bin-link
check "bin-link: the helper refused (exit $RC)" test "$RC" != 0
check "bin-link: it says why" grep -q "symlink" <<<"$OUT"
check "bin-link: the outside directory is unchanged" test "$(digest "$scratch/bin-link/outside")" = "$before"

# 4. run-wrapper
runfix run-wrapper
commit_fixture run-wrapper
run_helper run-wrapper run
check "run-wrapper: the helper finished (exit $RC: ${OUT: -120})" test "$RC" = 0
check "run-wrapper: lib/run.sh is the wrapper" grep -q "dest/.rust-bin/run" "$scratch/run-wrapper/dest/lib/run.sh"
check "run-wrapper: the wrapper sets no MAINTAINER_CHILD_ENV" test "$(grep -c MAINTAINER_CHILD_ENV "$scratch/run-wrapper/dest/lib/run.sh")" = 0
check "run-wrapper: the binary is in the copy" test -x "$scratch/run-wrapper/dest/.rust-bin/run"
check "run-wrapper: bin/prog is untouched" grep -q "echo bash prog" "$scratch/run-wrapper/dest/bin/prog"

# 5. run-lib
runfix run-lib
rm -r "$scratch/run-lib/repo/lib"
mkdir -p "$scratch/run-lib/outside/libdir"
printf '#!/usr/bin/env bash\n' > "$scratch/run-lib/outside/libdir/run.sh"
ln -s "$scratch/run-lib/outside/libdir" "$scratch/run-lib/repo/lib"
commit_fixture run-lib
before="$(digest "$scratch/run-lib/outside")"
run_helper run-lib run
check "run-lib: the helper refused (exit $RC)" test "$RC" != 0
check "run-lib: it says why" grep -q "symlink" <<<"$OUT"
check "run-lib: the outside directory is unchanged" test "$(digest "$scratch/run-lib/outside")" = "$before"

# 6. install-wrapper: the crate is called `install`, which stands for install.sh at the root of
# the tree. The wrapper replaces it, names the two variables the suites' stubs read, and
# leaves bin/prog alone.
progfix() {  # progfix NAME CRATE: the fixture with its crate renamed
    fixture "$1"
    sed -i "s/prog/$2/g" "$scratch/$1/repo/rust/crates/prog/Cargo.toml"
    (cd "$scratch/$1/repo/rust" && cargo generate-lockfile --offline -q) || exit 2
}
progfix install-wrapper install
printf '#!/usr/bin/env bash\necho bash install\n' > "$scratch/install-wrapper/repo/install.sh"
commit_fixture install-wrapper
run_helper install-wrapper install
check "install-wrapper: the helper finished (exit $RC: ${OUT: -120})" test "$RC" = 0
check "install-wrapper: install.sh is the wrapper" grep -q "dest/.rust-bin/install" "$scratch/install-wrapper/dest/install.sh"
check "install-wrapper: the wrapper names the two variables for the children" grep -q "MAINTAINER_CHILD_ENV" "$scratch/install-wrapper/dest/install.sh"
check "install-wrapper: the binary is in the copy" test -x "$scratch/install-wrapper/dest/.rust-bin/install"
check "install-wrapper: bin/prog is untouched" grep -q "echo bash prog" "$scratch/install-wrapper/dest/bin/prog"

# 7. platform-link: the crate is called `alert`, which stands for platform/linux/alert.sh, and
# platform is a symlink to an outside directory. The helper refuses and writes nothing there.
progfix platform-link alert
rm -rf "$scratch/platform-link/repo/platform"
mkdir -p "$scratch/platform-link/outside/platdir/linux"
printf '#!/usr/bin/env bash\n' > "$scratch/platform-link/outside/platdir/linux/alert.sh"
ln -s "$scratch/platform-link/outside/platdir" "$scratch/platform-link/repo/platform"
commit_fixture platform-link
before="$(digest "$scratch/platform-link/outside")"
run_helper platform-link alert
check "platform-link: the helper refused (exit $RC)" test "$RC" != 0
check "platform-link: it says why" grep -q "symlink" <<<"$OUT"
check "platform-link: the outside directory is unchanged" test "$(digest "$scratch/platform-link/outside")" = "$before"

printf 'assemble-tree test: %d checks, %d failed\n' "$N" "$F"
[ "$N" -gt 0 ] && [ "$F" -eq 0 ]
