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
#   assemble-tree.sh DEST [PROGRAM...]
#
# DEST must not exist, or must be an empty directory. A PROGRAM is the name of a
# file under bin/ whose Rust build is a [[bin]] of the workspace under rust/
# (maintainer-repo, maintainer-doctor, ...). With no PROGRAM every such program
# is built and swapped in; a named program that has no Rust build is an error.
#
# For each program the script
#   1. builds it with `cargo build --release --locked --offline`, in the cargo
#      target directory the caller chose (CARGO_TARGET_DIR, CARGO_BUILD_JOBS),
#   2. copies the binary to DEST/.rust-bin/NAME, and
#   3. replaces DEST/bin/NAME with a short wrapper that execs that binary
#      and hands it the wrapper's own path as argv[0], after naming the two
#      variables the suites' stubs read (CZ_CASE_DIR, BASH_ENV) in
#      MAINTAINER_CHILD_ENV so the program passes them to its children. The Rust program finds
#      its tree (lib/, bin/maintainer) from argv[0] the way the bash program
#      finds it from $0, so a suite that copies the wrapper to another folder
#      still points at that folder.
# The wrapper is a bash file because the suites run `bash bin/NAME`.
#
# The copy holds every tracked file and every untracked file that git does not
# ignore, so uncommitted work is under test. It never holds .git.
# Prints `assembled: DEST (programs: a b; HEAD abc1234)` on success.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ws="$root/rust"

usage() { printf 'usage: assemble-tree.sh DEST [PROGRAM...]\n' >&2; exit 64; }
die() { printf 'assemble-tree: %s\n' "$1" >&2; exit 1; }

[ "$#" -ge 1 ] || usage
dest_arg="$1"; shift
[ -n "$dest_arg" ] || usage
for want in "$@"; do
    case "$want" in
        ''|*/*|.*) die "'$want' is not a program name" ;;
    esac
done

command -v cargo >/dev/null 2>&1 || die "cargo is not installed"
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
# name is also a file under bin/. One line each: `package name`.
meta="$(cd "$ws" && cargo metadata --no-deps --format-version 1 --locked --offline 2>&1)"; rc=$?
[ "$rc" = 0 ] || die "cargo metadata exited $rc: $(tr '\n' ' ' <<<"$meta" | cut -c1-300)"
listing="$(python3 -I -c '
import json, os, sys
root = sys.argv[1]
meta = json.loads(sys.stdin.read())
rows = []
for pkg in meta["packages"]:
    for target in pkg["targets"]:
        if "bin" in target["kind"] and os.path.isfile(os.path.join(root, "bin", target["name"])):
            rows.append((pkg["name"], target["name"]))
for pkg, name in sorted(rows):
    print(pkg, name)
' "$root" <<<"$meta")"; rc=$?
[ "$rc" = 0 ] || die "could not read the workspace's binaries (exit $rc)"
[ -n "$listing" ] || die "the workspace has no binary that replaces a bin/ program"

# The selection, kept as parallel lists.
pkgs=(); names=()
if [ "$#" -eq 0 ]; then
    while read -r pkg name; do pkgs+=("$pkg"); names+=("$name"); done <<<"$listing"
else
    for want in "$@"; do
        pkg="$(awk -v n="$want" '$2 == n { print $1 }' <<<"$listing")"
        [ -n "$pkg" ] || die "$want has no Rust build in the workspace (ported: $(awk '{print $2}' <<<"$listing" | paste -sd' ' -))"
        pkgs+=("$pkg"); names+=("$want")
    done
fi
[ "${#names[@]}" -gt 0 ] || die "no program selected"

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export CARGO_TERM_COLOR=never
target_dir="${CARGO_TARGET_DIR:-$ws/target}"
export CARGO_TARGET_DIR="$target_dir"

# 1. build
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
tar -C "$root" --null --ignore-failed-read -T "$list" -cf - | tar -C "$dest" -xf -
rcs=("${PIPESTATUS[@]}")
if [ "${rcs[0]}" != 0 ] || [ "${rcs[1]}" != 0 ]; then die "copying the tree failed (tar exited ${rcs[*]})"; fi
[ -f "$dest/bin/maintainer-merge" ] || die "the copy has no bin/maintainer-merge"

# 3. the binaries and their wrappers
mkdir -p "$dest/.rust-bin" || die "cannot make $dest/.rust-bin"
for name in "${names[@]}"; do
    [ -f "$dest/bin/$name" ] || die "the copy has no bin/$name to replace"
    cp -- "$target_dir/release/$name" "$dest/.rust-bin/$name" || die "cannot copy the $name binary"
    chmod 755 "$dest/.rust-bin/$name"
    {
        printf '#!/usr/bin/env bash\n'
        printf '# Written by rust/scripts/assemble-tree.sh: the Rust build of %s.\n' "$name"
        printf '# argv[0] is this file, so the program finds lib/ and bin/maintainer beside it.\n'
        # The suites run the program under `env -i` with stubs that read
        # CZ_CASE_DIR and BASH_ENV. A child of the Rust program gets an
        # allowlisted environment, and MAINTAINER_CHILD_ENV names two more.
        # shellcheck disable=SC2016  # the wrapper expands these when it runs
        printf 'export MAINTAINER_CHILD_ENV="${MAINTAINER_CHILD_ENV:+$MAINTAINER_CHILD_ENV }CZ_CASE_DIR BASH_ENV"\n'
        # shellcheck disable=SC2016  # the wrapper expands these when it runs
        printf 'exec -a "${BASH_SOURCE[0]}" %q "$@"\n' "$dest/.rust-bin/$name"
    } > "$dest/bin/$name" || die "cannot write the $name wrapper"
    chmod 755 "$dest/bin/$name"
    bash -n "$dest/bin/$name" || die "the $name wrapper does not parse"
done

head="$(git -C "$root" rev-parse --short HEAD 2>/dev/null || printf unknown)"
printf 'assembled: %s (programs: %s; HEAD %s)\n' "$dest" "${names[*]}" "$head"
