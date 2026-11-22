#!/usr/bin/env bash
# Assemble a copy of this repository's tree in which the ported programs run as
# their Rust builds. The black-box suites (tests/run-characterization.sh,
# tests/run-tests.sh) take the tree under test from MX_TREE, so
#
#   rust/scripts/assemble-tree.sh /scratch/tree
#   MX_TREE=/scratch/tree bash tests/run-characterization.sh repo-prune.sh
#
# runs the bash suites against the Rust programs.
#
#   assemble-tree.sh [--plain] DEST [PROGRAM...]
#
# DEST must not exist, or must be an empty directory. A PROGRAM is the name of a
# file under bin/ whose Rust build is a [[bin]] of the workspace under rust/
# (maintainer-repo, maintainer-doctor, ...), or `run`, whose Rust build replaces
# lib/run.sh. With no PROGRAM every such program is built and swapped in; a named
# program that has no Rust build is an error. With --plain no program is built or
# swapped in and none may be named: DEST is the bash tree, made the same way, so a
# suite run against it and one run against the Rust tree are comparable.
#
# DEST is a repository of its own: a fresh one-commit git repository of the copied
# files, so the cases that ask `git ls-files` or run a gate have a work tree to ask.
# The commit is made after the wrappers are written, so it holds what runs. It is
# never the source's .git (that one is not copied and is not touched, even when the
# caller exports GIT_DIR and friends, as a git hook does), and .rust-bin is ignored
# there, so `git ls-files` names the same files in both trees.
#
# For each program the script
#   1. builds it with `cargo build --release --locked --offline`, in the cargo
#      target directory the caller chose (CARGO_TARGET_DIR, CARGO_BUILD_JOBS),
#   2. copies the binary to DEST/.rust-bin/NAME, and
#   3. replaces DEST/bin/NAME (DEST/lib/run.sh for `run`) with a short wrapper that execs that binary
#      and hands it the wrapper's own path as argv[0], after naming every variable
#      the caller exported in MAINTAINER_CHILD_ENV so the program passes them all to
#      its children, as the bash program does (the programs' own allowlist stays
#      covered by their unit tests; `run` passes its whole environment and gets no
#      such line). The Rust program finds its tree (lib/, bin/maintainer) from
#      argv[0] the way the bash program finds it from $0, so a suite that copies the
#      wrapper to another folder still points at that folder.
# The wrapper is a bash file because the suites run `bash bin/NAME`. It names the
# binary by an absolute path spelled in hex, so no file of the tree spells the
# directory the tree was built in: tests/run-tests.sh refuses a tree that spells
# the user's name, and that name is usually part of the path.
#
# The builds run with --remap-path-prefix for the home directory, the cargo home and
# the toolchain, so a binary carries no path of the machine it was built on either
# (see path_free_flags). Workspace members need none: cargo hands rustc their paths
# relative to the workspace.
#
# The copy holds every tracked file and every untracked file that git does not
# ignore, so uncommitted work is under test.
#
# Nothing is written outside DEST. A tracked symlink is copied as a symlink and
# never followed: the script refuses a tree in which a symlink stands where a
# directory of the copy must be (an entry below a symlink, or a symlink named bin,
# .rust-bin), and it removes a symlink before it writes a wrapper or a binary in
# its place, so a link pointing outside DEST cannot redirect the write.
# Prints `assembled: DEST (programs: a b; HEAD abc1234)` on success.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ws="$root/rust"

usage() { printf 'usage: assemble-tree.sh [--plain] DEST [PROGRAM...]  (--plain takes no PROGRAM)\n' >&2; exit 64; }
die() { printf 'assemble-tree: %s\n' "$1" >&2; exit 1; }

plain=0
if [ "${1:-}" = "--plain" ]; then plain=1; shift; fi
[ "$#" -ge 1 ] || usage
dest_arg="$1"; shift
[ -n "$dest_arg" ] || usage
[ "$plain" = 0 ] || [ "$#" -eq 0 ] || usage
for want in "$@"; do
    case "$want" in
        ''|*/*|.*) die "'$want' is not a program name" ;;
    esac
done

[ "$plain" = 1 ] || command -v cargo >/dev/null 2>&1 || die "cargo is not installed"
command -v python3 >/dev/null 2>&1 || die "python3 is not installed"
command -v git >/dev/null 2>&1 || die "git is not installed"

# DEST: absolute, empty or new, and outside the tree it copies.
mkdir -p -- "$dest_arg" || die "cannot create $dest_arg"
dest="$(cd "$dest_arg" && pwd)" || die "cannot enter $dest_arg"
case "$dest/" in
    "$root"/*) die "$dest is inside $root; pick a folder outside the tree" ;;
esac
if [ -n "$(ls -A -- "$dest")" ]; then die "$dest is not empty"; fi

# Which programs have a Rust build: the [[bin]] targets of the workspace whose
# name is also a file under bin/, and `run`, which stands for lib/run.sh. One line
# each: `package name path`, where path is the file the wrapper replaces. A plain
# tree builds nothing and needs none of this.
pkgs=(); names=(); rels=()
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export CARGO_TERM_COLOR=never
target_dir="${CARGO_TARGET_DIR:-$ws/target}"
export CARGO_TARGET_DIR="$target_dir"
if [ "$plain" = 0 ]; then
    meta="$(cd "$ws" && cargo metadata --no-deps --format-version 1 --locked --offline 2>&1)"; rc=$?
    [ "$rc" = 0 ] || die "cargo metadata exited $rc: $(tr '\n' ' ' <<<"$meta" | cut -c1-300)"
    listing="$(python3 -I -c '
import json, os, sys
root = sys.argv[1]
meta = json.loads(sys.stdin.read())
rows = []
for pkg in meta["packages"]:
    for target in pkg["targets"]:
        if "bin" not in target["kind"]:
            continue
        name = target["name"]
        if os.path.isfile(os.path.join(root, "bin", name)):
            rows.append((pkg["name"], name, "bin/" + name))
        elif name == "run" and os.path.isfile(os.path.join(root, "lib", "run.sh")):
            rows.append((pkg["name"], name, "lib/run.sh"))
for pkg, name, rel in sorted(rows):
    print(pkg, name, rel)
' "$root" <<<"$meta")"; rc=$?
    [ "$rc" = 0 ] || die "could not read the workspace's binaries (exit $rc)"
    [ -n "$listing" ] || die "the workspace has no binary that replaces a bin/ program"

    # The selection, kept as parallel lists.
    pkgs=(); names=(); rels=()
    if [ "$#" -eq 0 ]; then
        while read -r pkg name rel; do pkgs+=("$pkg"); names+=("$name"); rels+=("$rel"); done <<<"$listing"
    else
        for want in "$@"; do
            pkg="$(awk -v n="$want" '$2 == n { print $1 }' <<<"$listing")"
            rel="$(awk -v n="$want" '$2 == n { print $3 }' <<<"$listing")"
            [ -n "$pkg" ] || die "$want has no Rust build in the workspace (ported: $(awk '{print $2}' <<<"$listing" | paste -sd' ' -))"
            pkgs+=("$pkg"); names+=("$want"); rels+=("$rel")
        done
    fi
    [ "${#names[@]}" -gt 0 ] || die "no program selected"
fi

# path_free_flags: the --remap-path-prefix flags that keep this machine's paths out of a
# release binary. rustc embeds the path of every source file whose code can panic or
# call file!(): the toolchain's standard library when rust-src is installed, the crates
# of the cargo registry, a path dependency outside the workspace. Almost all of them sit
# below the builder's home directory, and tests/run-tests.sh refuses a tree that spells
# the user's name. Prefixes that are empty or `/` are skipped: rustc takes an empty one
# and writes /build/home/ in front of every path, and `/` rewrites every absolute path.
# rustc applies the LAST mapping that matches a path, so the broad prefix (the home
# directory) comes first and the specific ones after it.
#
# The flags travel in CARGO_ENCODED_RUSTFLAGS, which has no word splitting, so a path
# with a space survives. The caller's RUSTFLAGS or CARGO_ENCODED_RUSTFLAGS stay in front;
# rustflags set in a cargo config file are replaced, because cargo lets the environment
# win over the files.
path_free_flags() {
    local sep=$'\x1f' cargo_home="${CARGO_HOME:-}" sysroot="" i
    [ -n "$cargo_home" ] || cargo_home="${HOME:-}/.cargo"
    sysroot="$(cd "$ws" && rustc --print sysroot 2>/dev/null)" || sysroot=""
    local flags=()
    if [ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]; then
        IFS="$sep" read -r -a flags <<<"$CARGO_ENCODED_RUSTFLAGS"
    elif [ -n "${RUSTFLAGS:-}" ]; then
        read -r -a flags <<<"$RUSTFLAGS"
    fi
    local from=("${HOME:-}" "$cargo_home" "$sysroot")
    local to=(/build/home /build/cargo /build/rustc)
    for i in "${!from[@]}"; do
        case "${from[$i]}" in ''|/) continue ;; esac
        flags+=("--remap-path-prefix=${from[$i]}=${to[$i]}")
    done
    local joined="" flag
    for flag in "${flags[@]}"; do joined="${joined:+$joined$sep}$flag"; done
    printf '%s' "$joined"
}

# 1. build
if [ "${#names[@]}" -gt 0 ]; then
    CARGO_ENCODED_RUSTFLAGS="$(path_free_flags)"
    export CARGO_ENCODED_RUSTFLAGS
fi
for i in "${!names[@]}"; do
    out="$(cd "$ws" && cargo build --release --locked --offline -p "${pkgs[$i]}" --bin "${names[$i]}" 2>&1)"; rc=$?
    [ "$rc" = 0 ] || die "cargo build of ${names[$i]} exited $rc: $(tail -n 15 <<<"$out")"
    [ -x "$target_dir/release/${names[$i]}" ] || die "the build left no $target_dir/release/${names[$i]}"
done

# 2. the tree copy. A tracked file that is deleted in the working tree is
# skipped by tar's --ignore-failed-read; both tar exit codes are checked.
list="$(mktemp "${TMPDIR:-/tmp}/assemble-list.XXXXXX")" || die "cannot make a scratch file"
trap 'rm -f "$list"' EXIT
git -C "$root" ls-files -z --cached --others --exclude-standard > "$list"; rc=$?
[ "$rc" = 0 ] || die "git ls-files exited $rc"
[ -s "$list" ] || die "git ls-files listed nothing, so there is no tree to copy"
# No listed path may sit below a symlink of the source tree: extracting it would
# write through the link. python3 -I reads the NUL-separated list.
bad="$(python3 -I -c '
import os, sys
root = sys.argv[1]
for rel in sys.stdin.buffer.read().split(b"\0"):
    if not rel:
        continue
    parts = os.fsdecode(rel).split("/")
    for i in range(1, len(parts)):
        if os.path.islink(os.path.join(root, *parts[:i])):
            print("/".join(parts[:i]))
            break
' "$root" < "$list" | sort -u | head -n 5)"; rc=$?
[ "$rc" = 0 ] || die "could not scan the file list for symlinks"
[ -z "$bad" ] || die "a tracked symlink stands where a directory is expected, so the copy would write through it: $(tr '\n' ' ' <<<"$bad")"
for top in bin lib .rust-bin; do
    if [ -L "$root/$top" ]; then die "$top is a symlink in the source tree; refusing to copy it"; fi
done
tar -C "$root" --null --ignore-failed-read -T "$list" -cf - | tar -C "$dest" -xf -
rcs=("${PIPESTATUS[@]}")
if [ "${rcs[0]}" != 0 ] || [ "${rcs[1]}" != 0 ]; then die "copying the tree failed (tar exited ${rcs[*]})"; fi
[ -f "$dest/bin/maintainer-merge" ] || die "the copy has no bin/maintainer-merge"

# 3. the binaries and their wrappers. Every directory written into must be a real
# directory inside DEST, and a file in the way is removed (a symlink is removed,
# never followed) before the new one is created with noclobber. A plain tree has none.
if [ "${#names[@]}" -gt 0 ]; then
    mkdir -p "$dest/.rust-bin" || die "cannot make $dest/.rust-bin"
    real_dest="$(cd "$dest" && pwd -P)" || die "cannot resolve $dest"
    for sub in bin lib .rust-bin; do
        # lib/ holds a wrapper only when `run` is built, so a tree with no lib/ is fine for the others.
        if [ "$sub" = lib ] && [ ! -e "$dest/lib" ] && [ ! -L "$dest/lib" ]; then continue; fi
        if [ -L "$dest/$sub" ]; then die "$dest/$sub is a symlink; refusing to write through it"; fi
        real_sub="$(cd "$dest/$sub" && pwd -P)" || die "cannot resolve $dest/$sub"
        case "$real_sub/" in "$real_dest"/*) ;; *) die "$dest/$sub resolves outside $dest" ;; esac
    done
fi
for i in "${!names[@]}"; do
    name="${names[$i]}"; rel="${rels[$i]}"
    [ -f "$dest/$rel" ] || die "the copy has no $rel to replace"
    rm -f -- "$dest/.rust-bin/$name" "$dest/$rel" || die "cannot remove the old $name"
    ( set -C; cat -- "$target_dir/release/$name" > "$dest/.rust-bin/$name" ) || die "cannot copy the $name binary"
    chmod 755 "$dest/.rust-bin/$name"
    # The binary's path as \xHH escapes, one per byte, for a bash $'...' word.
    hex="$(printf '%s' "$dest/.rust-bin/$name" | od -An -v -tx1 | tr -d ' \n' | sed 's/\(..\)/\\x\1/g')" \
        || die "cannot spell the path of the $name binary"
    [ -n "$hex" ] || die "the path of the $name binary came out empty"
    (
        set -C
        {
        printf '#!/usr/bin/env bash\n'
        printf '# Written by rust/scripts/assemble-tree.sh: the Rust build of %s.\n' "$name"
        printf '# argv[0] is this file, so the program finds lib/ and bin/maintainer beside it.\n'
        if [ "$rel" != "lib/run.sh" ]; then
            # The suites run the program under `env -i` with stubs that read variables of
            # their own (CZ_CASE_DIR, BASH_ENV, DOC_LOG, FX, ...). A child of the Rust program
            # gets an allowlisted environment, plus the names in MAINTAINER_CHILD_ENV; the
            # bash program passes everything. So the wrapper names everything the caller
            # exported. `run` passes its whole environment on, as lib/run.sh does.
            cat <<'WRAPPER_EOF'
# Every variable the caller exported is named, after the names already there, so the
# program hands all of them to its children as the bash program does.
MAINTAINER_CHILD_ENV="${MAINTAINER_CHILD_ENV:+$MAINTAINER_CHILD_ENV }$(compgen -e | tr '\n' ' ')"
export MAINTAINER_CHILD_ENV
WRAPPER_EOF
        fi
        printf '# The binary'"'"'s path is spelled in hex so that no file of this tree spells the directory\n'
        printf '# it was assembled in (tests/run-tests.sh refuses a tree that spells the user'"'"'s name).\n'
        # shellcheck disable=SC2016  # the wrapper expands this when it runs
        printf 'exec -a "${BASH_SOURCE[0]}" $%s%s%s "$@"\n' "'" "$hex" "'"
        } > "$dest/$rel"
    ) || die "cannot write the $name wrapper"
    chmod 755 "$dest/$rel"
    bash -n "$dest/$rel" || die "the $name wrapper does not parse"
done

# 4. DEST is a repository of its own. The caller's GIT_DIR, GIT_WORK_TREE, GIT_INDEX_FILE and
# the rest of git's local variables (a hook exports them) would send every command below
# into the caller's repository, so they are removed from each call. So are the user's and
# the system's git configuration (hooks, signing, templates), which the commit must not
# trigger; the identity is given by -c.
git_env=(env)
local_vars="$(git -C "$root" rev-parse --local-env-vars)"; rc=$?
[ "$rc" = 0 ] || die "git rev-parse --local-env-vars exited $rc"
[ -n "$local_vars" ] || die "git rev-parse --local-env-vars listed nothing"
for var in $local_vars; do git_env+=(-u "$var"); done
git_env+=(GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1)
dgit() {
    "${git_env[@]}" git -C "$dest" -c user.name=assemble-tree -c user.email=assemble-tree@example.invalid "$@"
}
head="$(git -C "$root" rev-parse --short HEAD 2>/dev/null || printf unknown)"
dgit init -q -b main --template= -- . || die "git init failed in $dest"
real_dest="$(cd "$dest" && pwd -P)" || die "cannot resolve $dest"
[ "$(dgit rev-parse --show-toplevel 2>/dev/null)" = "$real_dest" ] || die "the repository made in $dest is not rooted there"
# The binaries stay out of the commit: they are not source, and `git ls-files` should name
# the files the bash tree names.
mkdir -p "$dest/.git/info" || die "cannot make $dest/.git/info"
printf '/.rust-bin/\n' > "$dest/.git/info/exclude" || die "cannot write the exclude file"
dgit add -A -f -- . ':(exclude).rust-bin' || die "git add failed in $dest"
dgit commit -q -m "assembled tree of $head" || die "git commit failed in $dest"
status="$(dgit status --porcelain 2>&1)"; rc=$?
[ "$rc" = 0 ] || die "git status exited $rc in $dest: $(head -n 3 <<<"$status" | tr '\n' ' ')"
[ -z "$status" ] || die "the repository in $dest is not clean after its commit: $(head -n 3 <<<"$status" | tr '\n' ' ')"

if [ "${#names[@]}" -gt 0 ]; then shown="${names[*]}"; else shown=none; fi
printf 'assembled: %s (programs: %s; HEAD %s)\n' "$dest" "$shown" "$head"
