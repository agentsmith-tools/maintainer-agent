#!/usr/bin/env bash
# Proves what assemble-tree.sh promises: it never writes outside DEST, DEST is a
# repository of its own, the wrappers pass the caller's variables on, and neither the
# binaries nor the wrappers carry a home path.
#
# It builds throwaway repositories (a one-crate workspace whose binary is called
# `prog`, and a bin/prog) and runs the real helper from each:
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
#   git-init    DEST is a fresh repository with one commit that holds the copied
#               files, and the source repository is byte for byte what it was.
#   git-env     the same with GIT_DIR, GIT_WORK_TREE and GIT_INDEX_FILE exported, as
#               a git hook exports them: the helper still writes only into DEST.
#   git-config  the user's git configuration (signing, hooks, templates) does not reach the
#               commit of DEST.
#   plain       `--plain` copies the tree and builds nothing; DEST is still a repository.
#   child-env   the wrapper names every variable the caller exported in
#               MAINTAINER_CHILD_ENV, and keeps the names already there.
#   wrapper-path DEST's own path appears nowhere in DEST, whatever characters it holds.
#   binary-path a binary built with dependencies under a home directory and under a cargo
#               home holds no path under either, and no path of the toolchain (an
#               unflagged build of the same crate holds all three). The caller's
#               RUSTFLAGS and CARGO_ENCODED_RUSTFLAGS still reach the compiler, and a
#               build with HOME unset works.
#   check-wrapper the crate is called `check-claims`: its wrapper replaces scripts/check_claims.sh
#               and carries no MAINTAINER_CHILD_ENV line.
#   check-link  scripts is a symlink to an outside directory. Building `check-claims` refuses.
#
# Prints `assemble-tree test: N checks, F failed`; exit 0 only with N > 0 and F = 0.
# MX_SCRATCH moves the scratch directory; CARGO_TARGET_DIR is shared with the caller.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
helper="$here/assemble-tree.sh"
[ -f "$helper" ] || { echo "test-assemble-tree: no assemble-tree.sh beside this file" >&2; exit 2; }

# A git hook runs this with GIT_DIR, GIT_INDEX_FILE and friends set for the commit in
# progress; every repository below would inherit them and write into the caller's index.
# shellcheck disable=SC2046  # the names are git's own fixed list, one per word
unset $(git rev-parse --local-env-vars)

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
    # The program reports how it was started, so a wrapper can be checked by running it.
    # The BTreeMap is there on purpose: std code that can panic carries the toolchain's
    # source path into the binary, which the path-free build must remove.
    cat > "$d/repo/rust/crates/prog/src/main.rs" <<'RSEOF'
use std::collections::BTreeMap;
fn main() {
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in std::env::vars() {
        vars.insert(name, value);
    }
    println!("argv0={}", std::env::args().next().unwrap_or_default());
    println!("child_env={}", vars.get("MAINTAINER_CHILD_ENV").map_or("", String::as_str));
}
RSEOF
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
# wrapper_target FILE: the binary a wrapper execs. The wrapper spells the path as \xHH escapes.
wrapper_target() { printf '%b' "$(sed -n "s/^exec -a .* \$'\(.*\)' \"\$@\"\$/\1/p" "$1")"; }

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
check "file-link: the wrapper names the binary in the copy" test "$(wrapper_target "$scratch/file-link/dest/bin/prog")" = "$scratch/file-link/dest/.rust-bin/prog"
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
check "run-wrapper: lib/run.sh is the wrapper" test "$(wrapper_target "$scratch/run-wrapper/dest/lib/run.sh")" = "$scratch/run-wrapper/dest/.rust-bin/run"
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
check "install-wrapper: install.sh is the wrapper" test "$(wrapper_target "$scratch/install-wrapper/dest/install.sh")" = "$scratch/install-wrapper/dest/.rust-bin/install"
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
# lines_of DIR: the files git lists for a work tree, sorted, in the byte order of C.
src_files() { git -C "$1" ls-files --cached --others --exclude-standard | LC_ALL=C sort; }
dest_files() { git -C "$1" ls-files | LC_ALL=C sort; }

# 6. git-init: DEST is a repository of its own with one commit of the copied files.
fixture git-init
printf '*.log\n' > "$scratch/git-init/repo/.gitignore"
printf 'forced\n' > "$scratch/git-init/repo/forced.log"
commit_fixture git-init
# forced.log matches .gitignore and is tracked anyway (added with -f); the copy must track it too.
( cd "$scratch/git-init/repo" && git add -f forced.log \
    && git -c user.name=t -c user.email=t@example.invalid commit -q -m forced ) || exit 2
printf 'untracked\n' > "$scratch/git-init/repo/untracked.txt"
printf 'ignored\n' > "$scratch/git-init/repo/ignored.log"
gi_src="$scratch/git-init/repo"; gi_dest="$scratch/git-init/dest"
before="$(digest "$gi_src/.git")"; src_head="$(git -C "$gi_src" rev-parse HEAD)"
want_files="$(src_files "$gi_src")"
run_helper git-init
check "git-init: the helper finished (exit $RC: ${OUT: -120})" test "$RC" = 0
check "git-init: the source repository is unchanged" test "$(digest "$gi_src/.git")" = "$before"
check "git-init: the source repository still has its one commit" test "$(git -C "$gi_src" rev-parse HEAD)" = "$src_head"
check "git-init: DEST is the top of a repository of its own" \
    test "$(git -C "$gi_dest" rev-parse --show-toplevel 2>/dev/null)" = "$(cd "$gi_dest" && pwd -P)"
check "git-init: the repository has exactly one commit" test "$(git -C "$gi_dest" rev-list --count HEAD 2>/dev/null)" = 1
check "git-init: it is not the source's commit" test "$(git -C "$gi_dest" rev-parse HEAD 2>/dev/null)" != "$src_head"
check "git-init: it lists the files the source lists (tracked and untracked, not ignored)" \
    test -n "$want_files" -a "$(dest_files "$gi_dest" 2>/dev/null)" = "$want_files"
check "git-init: a file the source tracks although .gitignore names it is tracked in the copy" \
    test "$(git -C "$gi_dest" ls-files -- forced.log 2>/dev/null)" = forced.log
check "git-init: an untracked source file is tracked in the copy" test "$(git -C "$gi_dest" ls-files -- untracked.txt 2>/dev/null)" = untracked.txt
check "git-init: an ignored source file is not in the copy" test ! -e "$gi_dest/ignored.log"
check "git-init: the work tree is clean, the binaries included" test -z "$(git -C "$gi_dest" status --porcelain 2>&1)"
check "git-init: the commit holds the wrapper, not the bash program" \
    cmp -s <(git -C "$gi_dest" show HEAD:bin/prog 2>/dev/null) "$gi_dest/bin/prog"
check "git-init: the binary is in the copy and no repository tracks it" \
    test -x "$gi_dest/.rust-bin/prog" -a -z "$(git -C "$gi_dest" ls-files -- .rust-bin 2>/dev/null)"

# 7. git-env: a hook exports GIT_DIR, GIT_WORK_TREE and GIT_INDEX_FILE. The helper must write
# only into DEST even then, and the source repository must come out as it went in.
fixture git-env
commit_fixture git-env
ge_src="$scratch/git-env/repo"; ge_dest="$scratch/git-env/dest"
before="$(digest "$ge_src/.git")"; src_head="$(git -C "$ge_src" rev-parse HEAD)"
OUT="$(cd "$ge_src" && env GIT_DIR="$ge_src/.git" GIT_WORK_TREE="$ge_src" GIT_INDEX_FILE="$ge_src/.git/index" \
    bash rust/scripts/assemble-tree.sh "$ge_dest" prog 2>&1)"; RC=$?
check "git-env: the helper finished (exit $RC: ${OUT: -120})" test "$RC" = 0
check "git-env: the source repository is unchanged" test "$(digest "$ge_src/.git")" = "$before"
check "git-env: the source repository still has its one commit" \
    test "$(git -C "$ge_src" rev-parse HEAD)" = "$src_head" -a "$(git -C "$ge_src" rev-list --count HEAD)" = 1
check "git-env: DEST is the top of a repository of its own" \
    test "$(git -C "$ge_dest" rev-parse --show-toplevel 2>/dev/null)" = "$(cd "$ge_dest" && pwd -P)"
check "git-env: that repository has exactly one commit, and it is not the source's" \
    test "$(git -C "$ge_dest" rev-list --count HEAD 2>/dev/null)" = 1 -a "$(git -C "$ge_dest" rev-parse HEAD 2>/dev/null)" != "$src_head"

# 7b. git-config: the user's git configuration asks for signed commits, a hooks directory whose
# pre-commit hook leaves a marker, and a template directory. None of it may run for the commit
# of an assembled tree, and the commit must not need the user's identity.
fixture git-config
commit_fixture git-config
gc_dir="$scratch/git-config/user"; mkdir -p "$gc_dir/hooks" "$gc_dir/template/hooks"
printf '#!/bin/sh\ntouch "%s/hook-ran"\nexit 1\n' "$gc_dir" > "$gc_dir/hooks/pre-commit"
cp "$gc_dir/hooks/pre-commit" "$gc_dir/template/hooks/pre-commit"; chmod +x "$gc_dir/hooks/pre-commit" "$gc_dir/template/hooks/pre-commit"
printf '[core]\n\thooksPath = %s/hooks\n[commit]\n\tgpgsign = true\n[gpg]\n\tprogram = %s/no-such-gpg\n[init]\n\ttemplateDir = %s/template\n[user]\n\tuseConfigOnly = true\n' \
    "$gc_dir" "$gc_dir" "$gc_dir" > "$gc_dir/gitconfig"
# The same file once as the user's configuration and once as the system's (GIT_CONFIG_SYSTEM moves
# the system file, so no privilege is needed to write one), and the template directory through
# GIT_TEMPLATE_DIR, whose pre-commit hook would become active in a repository made from it.
for gc_assign in "GIT_CONFIG_GLOBAL=$gc_dir/gitconfig" "GIT_CONFIG_SYSTEM=$gc_dir/gitconfig" "GIT_TEMPLATE_DIR=$gc_dir/template"; do
    gc_var="${gc_assign%%=*}"; gc_dest="$scratch/git-config/dest-$gc_var"; rm -f "$gc_dir/hook-ran"
    OUT="$(cd "$scratch/git-config/repo" && env "$gc_assign" bash rust/scripts/assemble-tree.sh "$gc_dest" prog 2>&1)"; RC=$?
    check "git-config: $gc_var: the helper finished with a hostile configuration (exit $RC: ${OUT: -120})" test "$RC" = 0
    check "git-config: $gc_var: no hook of the user ran" test ! -e "$gc_dir/hook-ran"
    check "git-config: $gc_var: DEST has its one commit, unsigned" \
        test "$(git -C "$gc_dest" rev-list --count HEAD 2>/dev/null)" = 1 -a -z "$(git -C "$gc_dest" log -1 --format=%GS 2>/dev/null)"
    check "git-config: $gc_var: the repository holds no hook" test -z "$(find "$gc_dest/.git/hooks" -type f 2>/dev/null)"
done

# 8. plain: the bash tree, copied the same way. No build, no wrapper, still a repository.
fixture plain
commit_fixture plain
pl_src="$scratch/plain/repo"; pl_dest="$scratch/plain/dest"; mkdir -p "$scratch/plain/target"
OUT="$(cd "$pl_src" && CARGO_TARGET_DIR="$scratch/plain/target" bash rust/scripts/assemble-tree.sh --plain "$pl_dest" 2>&1)"; RC=$?
check "plain: the helper finished (exit $RC: ${OUT: -120})" test "$RC" = 0
check "plain: it says which tree it assembled" grep -q "assembled: $pl_dest (programs: none" <<<"$OUT"
check "plain: bin/prog is still the bash program" grep -q "echo bash prog" "$pl_dest/bin/prog"
check "plain: no binary was built or copied" test ! -e "$pl_dest/.rust-bin" -a -z "$(ls -A "$scratch/plain/target")"
check "plain: DEST is a repository with one commit" test "$(git -C "$pl_dest" rev-list --count HEAD 2>/dev/null)" = 1
check "plain: it lists the files the source lists" \
    test -n "$(src_files "$pl_src")" -a "$(dest_files "$pl_dest" 2>/dev/null)" = "$(src_files "$pl_src")"
check "plain: the work tree is clean" test -z "$(git -C "$pl_dest" status --porcelain 2>&1)"
OUT="$(cd "$pl_src" && bash rust/scripts/assemble-tree.sh --plain "$scratch/plain/dest2" prog 2>&1)"; RC=$?
check "plain: a program name next to --plain is a usage error (exit $RC)" test "$RC" = 64
check "plain: and it wrote nothing" test ! -e "$scratch/plain/dest2/bin"

# 9. child-env: the wrapper names every variable the caller exported, and keeps the names
# already in MAINTAINER_CHILD_ENV. The program prints the value it received.
ce_wrapper="$gi_dest/bin/prog"
out="$(env -i PATH="$PATH" FOO_VAR=1 BAR_VAR=two ML_VAR=$'a\nb' bash "$ce_wrapper" 2>&1)"; rc=$?
ce_names=" $(sed -n 's/^child_env=//p' <<<"$out") "
check "child-env: the wrapper ran the program (exit $rc)" test "$rc" = 0
check "child-env: the program was started under the wrapper's own path" grep -qxF "argv0=$ce_wrapper" <<<"$out"
check "child-env: FOO_VAR is named" grep -qF ' FOO_VAR ' <<<"$ce_names"
check "child-env: BAR_VAR is named" grep -qF ' BAR_VAR ' <<<"$ce_names"
check "child-env: a variable whose value holds a newline is named" grep -qF ' ML_VAR ' <<<"$ce_names"
check "child-env: PATH is named too (the program decides what to do with it)" grep -qF ' PATH ' <<<"$ce_names"
check "child-env: only names, no values" test "$(grep -c -e = -e two <<<"$ce_names")" = 0
out="$(env -i PATH="$PATH" MAINTAINER_CHILD_ENV="PRIOR_A PRIOR_B" FOO_VAR=1 bash "$ce_wrapper" 2>&1)"
ce_value="$(sed -n 's/^child_env=//p' <<<"$out")"
check "child-env: names already in MAINTAINER_CHILD_ENV stay, first" test "${ce_value%% FOO_VAR*}" != "$ce_value" -a "${ce_value#PRIOR_A PRIOR_B }" != "$ce_value"
check "child-env: and the caller's other names follow" grep -qF ' FOO_VAR ' <<<" $ce_value "
out="$(env -i PATH="$PATH" bash "$ce_wrapper" 2>&1)"
check "child-env: with nothing exported but PATH the list is not empty and starts with a name" test -n "$(sed -n 's/^child_env=\([A-Za-z_]\)/\1/p' <<<"$out")"

# 10. wrapper-path: DEST sits below a directory named after the user, and its own name holds a
# space, both quotes and a dollar sign. Nothing in DEST (the repository included) may spell
# the user's name, and the wrapper must still start the program.
fixture needleuser-xyz
commit_fixture needleuser-xyz
wp_dest="$scratch/needleuser-xyz/d \$e\"st'x"
OUT="$(cd "$scratch/needleuser-xyz/repo" && bash rust/scripts/assemble-tree.sh "$wp_dest" prog 2>&1)"; RC=$?
check "wrapper-path: the helper finished (exit $RC: ${OUT: -120})" test "$RC" = 0
wp_hits="$(grep -r -a -l -F -- needleuser-xyz "$wp_dest" 2>&1)"
check "wrapper-path: no file below DEST spells a directory name above it (${wp_hits:-none})" test -z "$wp_hits"
out="$(env -i PATH="$PATH" bash "$wp_dest/bin/prog" 2>&1)"; rc=$?
check "wrapper-path: the wrapper still starts the program (exit $rc)" test "$rc" = 0
check "wrapper-path: and hands it the wrapper's own path" grep -qxF "argv0=$wp_dest/bin/prog" <<<"$out"

# 11. binary-path: the build runs with a stand-in HOME and a stand-in CARGO_HOME, each holding a
# path dependency whose file!() is an absolute path below it. An unflagged build of the same crate
# carries both paths and the toolchain's; the assembled binary carries none, and runs.
bp_home="$scratch/needleuser-home"; bp_cargo="$scratch/needleuser-cargo"
bp_real_home="$HOME"; bp_rustup="${RUSTUP_HOME:-$bp_real_home/.rustup}"
fixture binary-path
for dep in "$bp_home/dep" "$bp_cargo/cdep"; do
    mkdir -p "$dep/src"
    printf '[package]\nname = "%s"\nversion = "0.0.0"\nedition = "2024"\n' "${dep##*/}" > "$dep/Cargo.toml"
    printf 'pub fn here() -> &'"'"'static str {\n    file!()\n}\n' > "$dep/src/lib.rs"
done
printf '\n[dependencies]\ndep = { path = "%s" }\ncdep = { path = "%s" }\n' "$bp_home/dep" "$bp_cargo/cdep" \
    >> "$scratch/binary-path/repo/rust/crates/prog/Cargo.toml"
cat > "$scratch/binary-path/repo/rust/crates/prog/src/main.rs" <<'RSEOF'
use std::collections::BTreeMap;
fn main() {
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in std::env::vars() {
        vars.insert(name, value);
    }
    println!("argv0={}", std::env::args().next().unwrap_or_default());
    println!("child_env={}", vars.get("MAINTAINER_CHILD_ENV").map_or("", String::as_str));
    println!("file={}", file!());
    println!("dep={}", dep::here());
    println!("cdep={}", cdep::here());
    println!("probe={}", cfg!(mx_probe));
}
RSEOF
bp_env() { env HOME="$bp_home" RUSTUP_HOME="$bp_rustup" CARGO_HOME="$bp_cargo" "$@"; }
(cd "$scratch/binary-path/repo/rust" && bp_env cargo generate-lockfile --offline -q) || exit 2
commit_fixture binary-path
bp_sysroot="$(rustc --print sysroot 2>/dev/null)"
ctl_out="$(cd "$scratch/binary-path/repo/rust" && bp_env env CARGO_TARGET_DIR="$scratch/binary-path/ctl" cargo build --release --locked --offline 2>&1)"; rc=$?
ctl="$scratch/binary-path/ctl/release/prog"
check "binary-path: the unflagged control built (exit $rc: ${ctl_out: -120})" test "$rc" = 0 -a -x "$ctl"
check "binary-path: the control carries a path under HOME, so this test can fail" test "$(grep -a -c -F -- "$bp_home" "$ctl")" != 0
check "binary-path: the control carries a path under CARGO_HOME, so this test can fail" test "$(grep -a -c -F -- "$bp_cargo" "$ctl")" != 0
bp_has_sysroot=0
if [ -n "$bp_sysroot" ] && [ "$(grep -a -c -F -- "$bp_sysroot" "$ctl")" != 0 ]; then bp_has_sysroot=1; fi
if [ "$bp_has_sysroot" = 0 ]; then
    printf 'note  binary-path: this toolchain embeds no sysroot path in the control, so the toolchain check has nothing to prove here\n'
fi
# bp_clean BIN LABEL [nohome]: the binary holds none of the prefixes. With HOME unset or `/` there
# is no home prefix to remap, so a dependency below the stand-in HOME is expected to show.
bp_clean() {
    if [ "${3:-}" != nohome ]; then
        check "binary-path: $2: no path under HOME" test -x "$1" -a "$(grep -a -c -F -- "$bp_home" "$1")" = 0
        check "binary-path: $2: no path under the real home directory" test -x "$1" -a "$(grep -a -c -F -- "$bp_real_home/" "$1")" = 0
    fi
    check "binary-path: $2: no path under CARGO_HOME" test -x "$1" -a "$(grep -a -c -F -- "$bp_cargo" "$1")" = 0
    if [ "$bp_has_sysroot" = 1 ]; then
        check "binary-path: $2: no path of the toolchain" test -x "$1" -a "$(grep -a -c -F -- "$bp_sysroot" "$1")" = 0
    fi
}
# bp_build NAME ENV...: assemble the fixture into NAME with ENV added, then run its wrapper into BP_RUN.
bp_build() {
    local name="$1"; shift
    OUT="$(cd "$scratch/binary-path/repo" && bp_env env "$@" bash rust/scripts/assemble-tree.sh "$scratch/binary-path/$name" prog 2>&1)"; RC=$?
    check "binary-path: $name: the helper finished (exit $RC: ${OUT: -120})" test "$RC" = 0
    BP_RUN="$(env -i PATH="$PATH" bash "$scratch/binary-path/$name/bin/prog" 2>&1)"
    check "binary-path: $name: the program runs, and its own file!() is still the workspace-relative path" \
        grep -qx 'file=crates/prog/src/main.rs' <<<"$BP_RUN"
}
bp_build dest
bp_clean "$scratch/binary-path/dest/.rust-bin/prog" "assembled"
check "binary-path: it reports both dependencies' files, remapped, none under a stand-in directory" \
    test -n "$(sed -n 's/^dep=\(.*lib\.rs\)$/\1/p' <<<"$BP_RUN")" -a -n "$(sed -n 's/^cdep=\(.*lib\.rs\)$/\1/p' <<<"$BP_RUN")" \
    -a -z "$(grep -e needleuser-home -e needleuser-cargo <<<"$BP_RUN")"
check "binary-path: with no flags in the caller's environment the probe cfg is off" grep -qx 'probe=false' <<<"$BP_RUN"
# The caller's flags stay: --cfg mx_probe reaches the compiler through either variable.
bp_build dest-rustflags "RUSTFLAGS=--cfg mx_probe"
check "binary-path: RUSTFLAGS: the caller's flags reached the compiler" grep -qx 'probe=true' <<<"$BP_RUN"
bp_clean "$scratch/binary-path/dest-rustflags/.rust-bin/prog" "RUSTFLAGS"
bp_build dest-encoded "CARGO_ENCODED_RUSTFLAGS=--cfg"$'\x1f'"mx_probe"
check "binary-path: CARGO_ENCODED_RUSTFLAGS: the caller's flags reached the compiler" grep -qx 'probe=true' <<<"$BP_RUN"
bp_clean "$scratch/binary-path/dest-encoded/.rust-bin/prog" "CARGO_ENCODED_RUSTFLAGS"
# A cargo home below HOME: the more specific prefix wins, so the path reads /build/cargo/..., not /build/home/....
bp_build dest-nested "HOME=$scratch"
check "binary-path: a cargo home below HOME is mapped by the cargo-home prefix, not the home one" \
    grep -qx 'cdep=/build/cargo/cdep/src/lib.rs' <<<"$BP_RUN"
bp_clean "$scratch/binary-path/dest-nested/.rust-bin/prog" "nested" nohome
# With no usable home there is no home prefix: nothing is invented, and the other prefixes still work.
OUT="$(cd "$scratch/binary-path/repo" && env -u HOME RUSTUP_HOME="$bp_rustup" CARGO_HOME="$bp_cargo" \
    bash rust/scripts/assemble-tree.sh "$scratch/binary-path/dest-nohome" prog 2>&1)"; RC=$?
check "binary-path: dest-nohome: the helper finishes with HOME unset (exit $RC: ${OUT: -120})" test "$RC" = 0
out="$(env -i PATH="$PATH" bash "$scratch/binary-path/dest-nohome/bin/prog" 2>&1)"
check "binary-path: dest-nohome: no prefix was invented: the program's own file!() stays relative" grep -qx 'file=crates/prog/src/main.rs' <<<"$out"
bp_clean "$scratch/binary-path/dest-nohome/.rust-bin/prog" "HOME unset" nohome
bp_build dest-rootHome "HOME=/"
check "binary-path: HOME=/ invents no prefix: the dependency's absolute path is unchanged" \
    grep -qx "dep=$bp_home/dep/src/lib.rs" <<<"$BP_RUN"
bp_clean "$scratch/binary-path/dest-rootHome/.rust-bin/prog" "HOME=/" nohome

# 12. check-wrapper: a crate called `check-claims` stands for scripts/check_claims.sh, which is a
# wrapper without MAINTAINER_CHILD_ENV; a tree that holds a `scripts` symlink is refused (7).
checkfix() {  # $1 = fixture name: the fixture with its crate renamed to check-claims
    fixture "$1"
    sed -i 's/prog/check-claims/g' "$scratch/$1/repo/rust/crates/prog/Cargo.toml"
    (cd "$scratch/$1/repo/rust" && cargo generate-lockfile --offline -q) || exit 2
    mkdir -p "$scratch/$1/repo/scripts"
    printf '#!/usr/bin/env bash\necho bash check_claims\n' > "$scratch/$1/repo/scripts/check_claims.sh"
}
checkfix check-wrapper
commit_fixture check-wrapper
run_helper check-wrapper check-claims
check "check-wrapper: the helper finished (exit $RC: ${OUT: -120})" test "$RC" = 0
check "check-wrapper: scripts/check_claims.sh is the wrapper" test "$(wrapper_target "$scratch/check-wrapper/dest/scripts/check_claims.sh")" = "$scratch/check-wrapper/dest/.rust-bin/check-claims"
check "check-wrapper: the wrapper sets no MAINTAINER_CHILD_ENV" test "$(grep -c MAINTAINER_CHILD_ENV "$scratch/check-wrapper/dest/scripts/check_claims.sh")" = 0
check "check-wrapper: the binary is in the copy" test -x "$scratch/check-wrapper/dest/.rust-bin/check-claims"
check "check-wrapper: bin/prog is untouched" grep -q "echo bash prog" "$scratch/check-wrapper/dest/bin/prog"

# 13. check-scripts-link
checkfix check-link
rm -r "$scratch/check-link/repo/scripts"
mkdir -p "$scratch/check-link/outside/scriptsdir"
printf '#!/usr/bin/env bash\n' > "$scratch/check-link/outside/scriptsdir/check_claims.sh"
ln -s "$scratch/check-link/outside/scriptsdir" "$scratch/check-link/repo/scripts"
commit_fixture check-link
before="$(digest "$scratch/check-link/outside")"
run_helper check-link check-claims
check "check-link: the helper refused (exit $RC)" test "$RC" != 0
check "check-link: it says why" grep -q "symlink" <<<"$OUT"
check "check-link: the outside directory is unchanged" test "$(digest "$scratch/check-link/outside")" = "$before"

printf 'assemble-tree test: %d checks, %d failed\n' "$N" "$F"
[ "$N" -gt 0 ] && [ "$F" -eq 0 ]
