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
# (maintainer-repo, maintainer-doctor, ...), `run`, whose Rust build replaces
# lib/run.sh, or one of the five repository checks, whose Rust builds replace the
# script of the same purpose:
#   scripts/check_claims.sh          is built from check-claims
#   scripts/shellcheck-sweep.sh      is built from shellcheck-sweep
#   scripts/verify-action-pins.sh    is built from verify-action-pins
#   evals/run-evals.sh               is built from run-evals
#   tests/containment-probe.sh       is built from containment-probe
# With no PROGRAM every such program is built and swapped in; a named program that
# has no Rust build is an error.
#
# For each program the script
#   1. builds it with `cargo build --release --locked --offline`, in the cargo
#      target directory the caller chose (CARGO_TARGET_DIR, CARGO_BUILD_JOBS),
#   2. copies the binary to DEST/.rust-bin/NAME, and
#   3. replaces DEST/bin/NAME (DEST/lib/run.sh for `run`) with a short wrapper that execs that binary
#      and hands it the wrapper's own path as argv[0], after naming the two
#      variables the suites' stubs read (CZ_CASE_DIR, BASH_ENV) in
#      MAINTAINER_CHILD_ENV so the program passes them to its children (`run` and the
#      repository checks pass their whole environment and get no such line). The Rust program finds
#      its tree (lib/, bin/maintainer) from argv[0] the way the bash program
#      finds it from $0, so a suite that copies the wrapper to another folder
#      still points at that folder.
# The wrapper is a bash file because the suites run `bash bin/NAME`.
#
# The copy holds every tracked file and every untracked file that git does not
# ignore, so uncommitted work is under test. It never holds .git.
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
# name is also a file under bin/, `run`, which stands for lib/run.sh, and the five
# repository checks, each of which stands for one script. One line each:
# `package name path`, where path is the file the wrapper replaces.
meta="$(cd "$ws" && cargo metadata --no-deps --format-version 1 --locked --offline 2>&1)"; rc=$?
[ "$rc" = 0 ] || die "cargo metadata exited $rc: $(tr '\n' ' ' <<<"$meta" | cut -c1-300)"
listing="$(python3 -I -c '
import json, os, sys
root = sys.argv[1]
meta = json.loads(sys.stdin.read())
# The repository checks: the binary name and the script it replaces.
SCRIPTS = {
    "check-claims": "scripts/check_claims.sh",
    "shellcheck-sweep": "scripts/shellcheck-sweep.sh",
    "verify-action-pins": "scripts/verify-action-pins.sh",
    "run-evals": "evals/run-evals.sh",
    "containment-probe": "tests/containment-probe.sh",
}
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
        elif name in SCRIPTS and os.path.isfile(os.path.join(root, SCRIPTS[name])):
            rows.append((pkg["name"], name, SCRIPTS[name]))
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
for top in bin lib scripts evals tests .rust-bin; do
    if [ -L "$root/$top" ]; then die "$top is a symlink in the source tree; refusing to copy it"; fi
done
tar -C "$root" --null --ignore-failed-read -T "$list" -cf - | tar -C "$dest" -xf -
rcs=("${PIPESTATUS[@]}")
if [ "${rcs[0]}" != 0 ] || [ "${rcs[1]}" != 0 ]; then die "copying the tree failed (tar exited ${rcs[*]})"; fi
[ -f "$dest/bin/maintainer-merge" ] || die "the copy has no bin/maintainer-merge"

# 3. the binaries and their wrappers. Every directory written into must be a real
# directory inside DEST, and a file in the way is removed (a symlink is removed,
# never followed) before the new one is created with noclobber.
mkdir -p "$dest/.rust-bin" || die "cannot make $dest/.rust-bin"
real_dest="$(cd "$dest" && pwd -P)" || die "cannot resolve $dest"
for sub in bin lib scripts evals tests .rust-bin; do
    # lib/, scripts/, evals/ and tests/ hold a wrapper only when `run` or a check is built, so a
    # tree with none of them is fine for the others.
    if [ "$sub" != bin ] && [ "$sub" != .rust-bin ] && [ ! -e "$dest/$sub" ] && [ ! -L "$dest/$sub" ]; then continue; fi
    if [ -L "$dest/$sub" ]; then die "$dest/$sub is a symlink; refusing to write through it"; fi
    real_sub="$(cd "$dest/$sub" && pwd -P)" || die "cannot resolve $dest/$sub"
    case "$real_sub/" in "$real_dest"/*) ;; *) die "$dest/$sub resolves outside $dest" ;; esac
done
for i in "${!names[@]}"; do
    name="${names[$i]}"; rel="${rels[$i]}"
    [ -f "$dest/$rel" ] || die "the copy has no $rel to replace"
    rm -f -- "$dest/.rust-bin/$name" "$dest/$rel" || die "cannot remove the old $name"
    ( set -C; cat -- "$target_dir/release/$name" > "$dest/.rust-bin/$name" ) || die "cannot copy the $name binary"
    chmod 755 "$dest/.rust-bin/$name"
    (
        set -C
        {
        printf '#!/usr/bin/env bash\n'
        printf '# Written by rust/scripts/assemble-tree.sh: the Rust build of %s.\n' "$name"
        printf '# argv[0] is this file, so the program finds lib/ and bin/maintainer beside it.\n'
        # The suites run the program under `env -i` with stubs that read
        # CZ_CASE_DIR and BASH_ENV. A child of the Rust program gets an
        # allowlisted environment, and MAINTAINER_CHILD_ENV names two more. `run` and the
        # repository checks pass their whole environment on, as the scripts do, so they need neither.
        if [ "${rel#bin/}" != "$rel" ]; then
            # shellcheck disable=SC2016  # the wrapper expands these when it runs
            printf 'export MAINTAINER_CHILD_ENV="${MAINTAINER_CHILD_ENV:+$MAINTAINER_CHILD_ENV }CZ_CASE_DIR BASH_ENV"\n'
        fi
        # shellcheck disable=SC2016  # the wrapper expands these when it runs
        printf 'exec -a "${BASH_SOURCE[0]}" %q "$@"\n' "$dest/.rust-bin/$name"
        } > "$dest/$rel"
    ) || die "cannot write the $name wrapper"
    chmod 755 "$dest/$rel"
    bash -n "$dest/$rel" || die "the $name wrapper does not parse"
done

head="$(git -C "$root" rev-parse --short HEAD 2>/dev/null || printf unknown)"
printf 'assembled: %s (programs: %s; HEAD %s)\n' "$dest" "${names[*]}" "$head"
