#!/usr/bin/env bash
# Move this machine from the bash maintainer programs to the Rust builds, and back.
#
#   scripts/cutover.sh build    [--out DIR] [--allow-dirty]
#   scripts/cutover.sh shadow   [--out DIR] [--accept-skip NAME]... [--timeout SECONDS]
#   scripts/cutover.sh live     [--build-dir DIR]
#   scripts/cutover.sh status   [--build-dir DIR]
#   scripts/cutover.sh rollback [--dry-run] [--force]
#
# The order is build, shadow, live, status. rollback works from the RUST_STAMP that
# `install --rust` writes, and needs no Rust binary: this file is bash on purpose
# (plan decision P4-D23), because the way back must not depend on the thing that
# may be broken. docs/deploy/cutover.md says what each step proves and what it does not.
#
# Exit status: 0 done, 1 refused (the line on standard error names what), 64 usage.
# Every refusal comes before the first change to the machine.
set -euo pipefail
# `git status` refreshes the index of the checkout unless told not to. Nothing here writes to it.
export GIT_OPTIONAL_LOCKS=0

PROG="cutover.sh"
META_FILES="SHA256SUMS BUILD_INFO shadow-ok shadow-report.txt"
# The programs `install --rust` swaps, in the order RUST_STAMP lists them.
SWAP_NAMES="maintainer-merge maintainer-doctor maintainer-repo run run-instance alert"

die() { printf '%s: %s\n' "$PROG" "$*" >&2; exit 1; }

usage() {
    cat >&2 <<'EOF'
usage: scripts/cutover.sh build    [--out DIR] [--allow-dirty]
       scripts/cutover.sh shadow   [--out DIR] [--accept-skip NAME]... [--timeout SECONDS]
       scripts/cutover.sh live     [--build-dir DIR]
       scripts/cutover.sh status   [--build-dir DIR]
       scripts/cutover.sh rollback [--dry-run] [--force]
EOF
}
usage_die() { printf '%s: %s\n' "$PROG" "$*" >&2; usage; exit 64; }

# The directory the EXIT trap removes. The trap names this function and no path, so no path is ever
# part of a command line that the shell reads again.
CLEAN_DIR=""
clean_up() { [ -z "$CLEAN_DIR" ] || rm -rf -- "$CLEAN_DIR"; }

# What a hostile argument looks like in a message: quoted, one line.
shown() { printf '%q' "$1"; }

need() {
    local t
    for t in "$@"; do
        command -v "$t" >/dev/null 2>&1 || die "the tool '$t' is not on PATH; nothing was done"
    done
}

nl_free() { case "$1" in *$'\n'*) return 1 ;; esac; return 0; }

# A path that reaches argv, a PATH entry, a table line or the list of steps of a rollback: absolute, one
# line, no control character, no colon and no pipe.
plain_path() {  # $1 = what it is, $2 = the path
    nl_free "$2" || die "$1 holds a newline; refused"
    case "$2" in
        *\|*) die "$1 holds a '|'; refused" ;;
        *\'*) die "$1 holds a single quote; refused" ;;
    esac
    [[ "$2" =~ [[:cntrl:]] ]] && die "$1 holds a control character; refused"
    case "$2" in
        *:*) die "$1 holds a colon, which would split a PATH entry; refused" ;;
        /*) ;;
        *) die "$1 is not an absolute path; refused" ;;
    esac
}

self="$(readlink -f -- "${BASH_SOURCE[0]}")" || die "cannot resolve the path of this script"
root="$(cd -- "$(dirname -- "$self")/.." && pwd -P)" || die "cannot enter the checkout"
[ -f "$root/install.sh" ] && [ -d "$root/rust" ] \
    || die "$root is not a maintainer-agent checkout (no install.sh and rust/ beside scripts/)"
plain_path "the checkout path" "$root"

[ -n "${HOME:-}" ] || die "HOME is not set"
plain_path "HOME" "$HOME"
share="$HOME/.local/share/maintainer"
bindir="$HOME/.local/bin"
stamp_file="$share/RUST_STAMP"

# --- the programs `install --rust` swaps -------------------------------------------------

installed_path() {  # $1 = program name
    case "$1" in
        maintainer-merge|maintainer-doctor|maintainer-repo) printf '%s' "$bindir/$1" ;;
        run) printf '%s' "$share/run.sh" ;;
        run-instance) printf '%s' "$share/run-instance.sh" ;;
        alert) printf '%s' "$share/alert.sh" ;;
        *) return 1 ;;
    esac
}

# The bash program in the checkout that `install` copies to the installed path.
source_path() {  # $1 = program name
    case "$1" in
        maintainer-merge|maintainer-doctor|maintainer-repo) printf '%s' "$root/bin/$1" ;;
        run) printf '%s' "$root/lib/run.sh" ;;
        run-instance) printf '%s' "$root/platform/linux/run-instance.sh" ;;
        alert) printf '%s' "$root/platform/linux/alert.sh" ;;
        *) return 1 ;;
    esac
}

# The program a stamp line names, by its installed path. Empty when it is none of the six.
name_of_installed() {  # $1 = installed path
    local n
    for n in $SWAP_NAMES; do
        [ "$(installed_path "$n")" = "$1" ] && { printf '%s' "$n"; return 0; }
    done
    return 1
}

sha256_of() {  # $1 = file -> 64 lower-case hex; refuses when it cannot read the file
    local out
    out="$(sha256sum < "$1" 2>/dev/null)" || die "cannot read $1 to hash it"
    out="${out%% *}"
    [[ "$out" =~ ^[0-9a-f]{64}$ ]] || die "sha256sum printed no hash for $1"
    printf '%s' "$out"
}

# --- the build directory ----------------------------------------------------------------

default_dir() { printf '%s' "${XDG_CACHE_HOME:-$HOME/.cache}/magent-cutover/build"; }

absolute_dir() {  # $1 = what it is, $2 = the directory as given
    local d="$2"
    [ -n "$d" ] || die "$1 is empty; refused"
    nl_free "$d" || die "$1 holds a newline; refused"
    case "$d" in
        /*) ;;
        *) nl_free "$PWD" || die "the working directory holds a newline; refused"; d="$PWD/$d" ;;
    esac
    while [ "${d%/}" != "$d" ] && [ "$d" != "/" ]; do d="${d%/}"; done
    plain_path "$1" "$d"
    printf '%s' "$d"
}

# The directory must exist, be a real directory of ours, and not be writable by the world:
# what it holds becomes a program on this machine.
check_dir() {  # $1 = absolute path
    local d="$1" perm owner
    [ ! -L "$d" ] || die "$d is a symbolic link; the build directory must be a real directory"
    [ -d "$d" ] || die "$d is not a directory"
    perm="$(stat -c %a -- "$d")" || die "cannot read the mode of $d"
    owner="$(stat -c %u -- "$d")" || die "cannot read the owner of $d"
    [ "$owner" = "$(id -u)" ] || die "$d belongs to another user; refused"
    [ $(( ${perm: -1} & 2 )) -eq 0 ] || die "$d is writable by everyone (mode $perm); refused"
}

is_meta() { case " $META_FILES " in *" $1 "*) return 0 ;; esac; return 1; }

# `<sha256>  <name>` for every program in the directory, ordered by name. A directory
# that holds anything but regular files and the four files of this script is refused.
table_of() {  # $1 = directory
    local dir="$1" f name list=() sorted
    for f in "$dir"/* "$dir"/.[!.]*; do
        { [ -e "$f" ] || [ -L "$f" ]; } || continue
        name="${f##*/}"
        is_meta "$name" && continue
        # The scratch names of build and shadow are not programs, and the install never reads them.
        case "$name" in .stage.*|.shadow.*|.shadow-ok.*) continue ;; esac
        [ ! -L "$f" ] || die "$f is a symbolic link; the build directory holds regular files only"
        [ -f "$f" ] || die "$f is not a regular file"
        [[ "$name" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || die "$name is not a plain program name; refused"
        list+=("$name")
    done
    [ "${#list[@]}" -gt 0 ] || die "$dir holds no program"
    sorted="$(printf '%s\n' "${list[@]}" | LC_ALL=C sort)" || die "cannot sort the program names"
    while IFS= read -r name; do
        printf '%s  %s\n' "$(sha256_of "$dir/$name")" "$name"
    done <<<"$sorted"
}

# A table read from a file: the `<sha256>  <name>` lines, comments dropped.
table_lines() {  # $1 = file
    local line out=""
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in '#'*|'') continue ;; esac
        out="$out$line"$'\n'
    done < "$1"
    printf '%s' "$out"
}

# The programs a build directory holds, one per line.
table_names() {  # $1 = table text
    local line
    while IFS= read -r line; do
        [ -z "$line" ] || printf '%s\n' "${line#*  }"
    done <<<"$1"
}

# The hash a table gives a program; empty when the table does not list it.
table_hash() {  # $1 = table text, $2 = program name
    local line
    while IFS= read -r line; do
        if [ "${line#*  }" = "$2" ]; then printf '%s' "${line%%  *}"; return 0; fi
    done <<<"$1"
    return 0
}

has_name() {  # $1 = newline-separated names, $2 = a name
    local n
    while IFS= read -r n; do [ "$n" = "$2" ] && return 0; done <<<"$1"
    return 1
}

# --- RUST_STAMP --------------------------------------------------------------------------

# Reads $stamp_file into ST_PATH[] and ST_HASH[] (lower case). Prints nothing; returns 1 with
# the reason in ST_ERR when the file is not a stamp this script can act on. The format is the
# one `install --rust` writes: `<64 hex>  <installed path>` per line, and only the six
# installed paths of the programs it swaps.
ST_PATH=(); ST_HASH=(); ST_ERR=""
read_stamp() {
    ST_PATH=(); ST_HASH=(); ST_ERR=""
    local line hash path n i
    local text size bare
    # A NUL byte cannot be told from the end of a path by the tools below, so it is asked first.
    size="$(wc -c < "$stamp_file" 2>/dev/null)" || { ST_ERR="$stamp_file cannot be read"; return 1; }
    bare="$(tr -d '\0' < "$stamp_file" 2>/dev/null | wc -c)" || { ST_ERR="$stamp_file cannot be read"; return 1; }
    if [ "$size" != "$bare" ]; then ST_ERR="RUST_STAMP holds a NUL byte"; return 1; fi
    text="$(cat -- "$stamp_file" 2>/dev/null; printf x)" || { ST_ERR="$stamp_file cannot be read"; return 1; }
    text="${text%x}"
    # The here-string ends the last line, so a stamp whose last line has no newline is read in full.
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        if ! [[ "$line" =~ ^[0-9A-Fa-f]{64}\ \ . ]]; then
            ST_ERR="RUST_STAMP has a line that is not \`<sha256>  <path>\`: $line"; return 1
        fi
        hash="${line:0:64}"; path="${line:66}"
        hash="$(printf '%s' "$hash" | tr 'A-F' 'a-f')"
        n="$(name_of_installed "$path")" || { ST_ERR="RUST_STAMP lists $path, which is not a program this installer swaps"; return 1; }
        for i in "${ST_PATH[@]}"; do
            [ "$i" != "$path" ] || { ST_ERR="RUST_STAMP lists $path twice"; return 1; }
        done
        ST_PATH+=("$path"); ST_HASH+=("$hash")
    done <<<"$text"
    [ "${#ST_PATH[@]}" -gt 0 ] || { ST_ERR="RUST_STAMP lists no program"; return 1; }
    return 0
}

# --- build ---------------------------------------------------------------------------------

# The programs that stand for a script outside bin/. rust/scripts/assemble-tree.sh keeps the
# same table; tests/characterization/cutover-build.sh compares the two texts, so a program
# added to one and not the other fails a case.
read -r -d '' WORKSPACE_PROGRAMS_PY <<'PYEOF' || true
import json, os, sys
root = sys.argv[1]
meta = json.loads(sys.stdin.read())
# The programs that stand for a script outside bin/: name and the file the wrapper replaces.
OUTSIDE_BIN = {
    "run": "lib/run.sh",
    "install": "install.sh",
    "new-profile": "new-profile.sh",
    "install-cron": "platform/posix/install-cron.sh",
    "install-launchd": "platform/macos/install-launchd.sh",
    "alert": "platform/linux/alert.sh",
    "run-instance": "platform/linux/run-instance.sh",
    # The five repository checks.
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
            rows.append((pkg["name"], name))
        elif name in OUTSIDE_BIN and os.path.isfile(os.path.join(root, OUTSIDE_BIN[name])):
            rows.append((pkg["name"], name))
for pkg, name in sorted(rows):
    print(pkg, name)
PYEOF

# The --remap-path-prefix flags that keep this machine's paths out of a release binary. The
# same function as rust/scripts/assemble-tree.sh (a case compares the two texts).
path_free_flags() {
    local sep=$'\x1f' cargo_home="${CARGO_HOME:-}" sysroot="" i
    [ -n "$cargo_home" ] || cargo_home="${HOME:-}/.cargo"
    sysroot="$(cd "$root/rust" && rustc --print sysroot 2>/dev/null)" || sysroot=""
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

cmd_build() {
    local out="" allow_dirty=0
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --out) [ "$#" -ge 2 ] || usage_die "build: --out needs a directory"; out="$2"; shift 2 ;;
            --allow-dirty) allow_dirty=1; shift ;;
            *) usage_die "build: unknown argument $(shown "$1")" ;;
        esac
    done
    need cargo git python3 sha256sum mktemp stat id sort cp mv rm mkdir chmod
    [ -n "$out" ] || out="$(default_dir)"
    out="$(absolute_dir "the build directory" "$out")"

    # The tree must be the one a commit names, so the table can be traced to it.
    local status head dirty=0
    head="$(git -C "$root" rev-parse HEAD 2>/dev/null)" || die "cannot read HEAD of $root"
    status="$(git -C "$root" status --porcelain 2>/dev/null)" || die "cannot read the git status of $root"
    if [ -n "$status" ]; then
        [ "$allow_dirty" = 1 ] || die "the working tree is dirty; commit or stash, or pass --allow-dirty (the build is then marked dirty and live refuses it)"
        dirty=1
    fi

    if [ -e "$out" ] || [ -L "$out" ]; then
        check_dir "$out"
        # Only what an earlier build wrote may be replaced.
        local f name known=""
        if [ -f "$out/SHA256SUMS" ]; then known="$(table_names "$(table_lines "$out/SHA256SUMS")")"; fi
        for f in "$out"/* "$out"/.[!.]*; do
            { [ -e "$f" ] || [ -L "$f" ]; } || continue
            name="${f##*/}"
            is_meta "$name" && continue
            # A build that was killed leaves its scratch directory; it is not a foreign file.
            case "$name" in .stage.*|.shadow.*|.shadow-ok.*) continue ;; esac
            has_name "$known" "$name" || die "$out holds $name, which an earlier build did not write; use an empty directory"
        done
    else
        mkdir -p -- "$out" || die "cannot create $out"
        chmod 755 -- "$out" || die "cannot set the mode of $out"
        check_dir "$out"
    fi

    local listing
    listing="$(cd "$root/rust" && cargo metadata --no-deps --format-version 1 --locked --offline 2>&1)" \
        || die "cargo metadata failed: $(printf '%s' "$listing" | tr '\n' ' ' | cut -c1-300)"
    listing="$(python3 -I -c "$WORKSPACE_PROGRAMS_PY" "$root" <<<"$listing")" \
        || die "could not read the binaries of the workspace"
    [ -n "$listing" ] || die "the workspace has no binary that stands for a program of this repository"

    local pkgs=() names=() pkg name
    while read -r pkg name; do
        [[ "$name" =~ ^[a-z][a-z0-9-]*$ ]] || die "the workspace names a binary $(shown "$name"), which is not a plain name"
        pkgs+=("$pkg"); names+=("$name")
    done <<<"$listing"

    export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
    export CARGO_TERM_COLOR=never
    local target_dir="${CARGO_TARGET_DIR:-$root/rust/target}"
    export CARGO_TARGET_DIR="$target_dir"
    CARGO_ENCODED_RUSTFLAGS="$(path_free_flags)"
    export CARGO_ENCODED_RUSTFLAGS

    local stage
    stage="$(mktemp -d "$out/.stage.XXXXXX")" || die "cannot make a scratch directory inside $out"
    # shellcheck disable=SC2064
    CLEAN_DIR="$stage"
    trap clean_up EXIT
    trap 'exit 143' TERM
    trap 'exit 130' INT
    trap 'exit 129' HUP
    local i built
    for i in "${!names[@]}"; do
        printf 'building %s\n' "${names[$i]}" >&2
        built="$(cd "$root/rust" && cargo build --release --locked --offline -p "${pkgs[$i]}" --bin "${names[$i]}" 2>&1)" \
            || die "cargo build of ${names[$i]} failed: $(printf '%s' "$built" | tail -n 15 | tr '\n' ' ' | cut -c1-400)"
        [ -f "$target_dir/release/${names[$i]}" ] && [ -x "$target_dir/release/${names[$i]}" ] \
            || die "the build left no $target_dir/release/${names[$i]}"
        [ -s "$target_dir/release/${names[$i]}" ] || die "the build of ${names[$i]} is empty"
        cp -- "$target_dir/release/${names[$i]}" "$stage/${names[$i]}" || die "cannot copy ${names[$i]}"
        chmod 755 -- "$stage/${names[$i]}" || die "cannot set the mode of ${names[$i]}"
    done

    # The old programs go only now, with the new ones ready beside them.
    local old
    for f in "$out"/* "$out"/.[!.]*; do
        { [ -e "$f" ] || [ -L "$f" ]; } || continue
        old="${f##*/}"
        case "$old" in .stage.*) continue ;; esac
        [ ! -d "$f" ] || [ -L "$f" ] || die "$f is a directory; refused"
        rm -f -- "$f" || die "cannot remove $f"
    done
    for name in "${names[@]}"; do
        mv -- "$stage/$name" "$out/$name" || die "cannot move $name into $out"
    done
    local table
    table="$(table_of "$out")"
    printf '%s\n' "$table" > "$out/SHA256SUMS" || die "cannot write $out/SHA256SUMS"
    printf 'head=%s\ndirty=%s\nprograms=%s\n' "$head" "$dirty" "$(table_names "$table" | tr '\n' ' ' | sed 's/ $//')" > "$out/BUILD_INFO" \
        || die "cannot write $out/BUILD_INFO"
    printf 'built %d programs from %s in %s\n' "${#names[@]}" "${head:0:12}" "$out"
    printf '%s\n' "$table"
}

# --- rollback ------------------------------------------------------------------------------

# Whether another process holds the run lock of a profile of this checkout: the same question
# `install` asks before it replaces the files a run reads.
busy_profiles() {
    local env state lock name busy=""
    for env in "$root"/profiles/*/profile.env; do
        [ -f "$env" ] || continue
        state="$(bash -c 'set -u; set -o pipefail; . "$1" >/dev/null 2>&1; printf "%s" "${STATE_DIR:-}"' bash "$env" 2>/dev/null || true)"
        [ -n "$state" ] || continue
        lock="$state/run.lock"
        [ -e "$lock" ] || continue
        if flock -n "$lock" true 2>/dev/null; then continue; fi
        name="${env%/profile.env}"; name="${name##*/}"
        busy="$busy $name"
    done
    printf '%s' "$busy"
}

cmd_rollback() {
    local dry=0 force=0
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --dry-run) dry=1; shift ;;
            --force) force=1; shift ;;
            *) usage_die "rollback: unknown argument $(shown "$1")" ;;
        esac
    done
    need sha256sum mv rm cat wc tr bash
    command -v flock >/dev/null 2>&1 \
        || die "flock is required to install and protect active runs; install flock and ensure it is on PATH."
    if [ "$dry" = 0 ] && [ "$force" = 0 ]; then
        local busy
        busy="$(busy_profiles)"
        if [ -n "$busy" ]; then
            printf '%s: a run is in flight for:%s\n' "$PROG" "$busy" >&2
            printf '            This replaces %s/profiles, which that run reads for\n' "$share" >&2
            printf '            the whole of its pass.\n\n' >&2
            printf '            Wait for it (maintainer status), or --force if you are sure.\n' >&2
            exit 69
        fi
    fi

    if [ ! -e "$stamp_file" ] && [ ! -L "$stamp_file" ]; then
        die "nothing to roll back: $stamp_file does not exist"
    fi
    [ ! -L "$stamp_file" ] || die "cannot roll back: $stamp_file is a symbolic link; rollback never follows or replaces one"
    [ -f "$stamp_file" ] || die "cannot roll back: $stamp_file cannot be read: it is not a regular file"
    if ! read_stamp; then die "cannot roll back: $ST_ERR"; fi

    # Every program is judged before the first is moved. A saved original or an installed
    # file that is a link is refused: a rollback never follows one and never replaces one.
    local i installed backup ihash bhash restored=0 n
    local skind=() ssrc=() sdst=()   # the steps, one entry in each array: no field is cut out of a joined string
    for i in "${!ST_PATH[@]}"; do
        installed="${ST_PATH[$i]}"; backup="$installed.sh"
        if [ -L "$installed" ]; then die "cannot roll back: $installed is a symbolic link; rollback never follows or replaces one"; fi
        if [ -L "$backup" ]; then die "cannot roll back: $backup is a symbolic link; rollback never follows or replaces one"; fi
        ihash=""; bhash=""
        if [ -f "$installed" ]; then ihash="$({ sha256sum < "$installed"; } 2>/dev/null)" || ihash=""; ihash="${ihash%% *}"; fi
        if [ -f "$backup" ]; then bhash="$({ sha256sum < "$backup"; } 2>/dev/null)" || bhash=""; bhash="${bhash%% *}"; fi
        # A file that is there and cannot be read is not "restored already": the stamp stays.
        if [ -e "$installed" ] && [ -z "$ihash" ]; then die "cannot roll back: $installed cannot be read"; fi
        if [ -e "$backup" ] && [ -z "$bhash" ]; then die "cannot roll back: $backup cannot be read"; fi
        if [ -n "$ihash" ] && [ "$ihash" = "${ST_HASH[$i]}" ]; then
            [ -n "$bhash" ] || die "cannot roll back: $backup is missing"
            skind+=(mv); ssrc+=("$backup"); sdst+=("$installed"); restored=$((restored + 1))
        elif [ -z "$bhash" ]; then
            :   # restored already: a rollback that failed part way can run again
        elif [ "$bhash" = "$ihash" ]; then
            skind+=(rm); ssrc+=("$backup"); sdst+=("")
        else
            die "cannot roll back: $installed no longer matches the stamp, so it is not the build this installer wrote"
        fi
    done

    printf '  rolling back from %s\n' "$share"
    local j
    for j in "${!skind[@]}"; do
        case "${skind[$j]}" in
            mv)
                if [ "$dry" = 1 ]; then printf '  would: mv %s %s\n' "${ssrc[$j]}" "${sdst[$j]}"
                else mv -f -- "${ssrc[$j]}" "${sdst[$j]}" || exit "$?"; fi ;;
            rm)
                if [ "$dry" = 1 ]; then printf '  would: rm -f %s\n' "${ssrc[$j]}"
                else rm -f -- "${ssrc[$j]}" || exit "$?"; fi ;;
        esac
    done
    if [ "$dry" = 1 ]; then printf '  would: rm -f %s\n' "$stamp_file"
    else rm -f -- "$stamp_file" || exit "$?"; fi
    n="$restored"
    printf '  rolled back %s program(s) to the bash originals\n' "$n"
}

# Whether shadow-ok in $1 is the marker of a clean shadow of what is in $1 and of this checkout. It binds
# three things: the table of the programs, the commit of the checkout and the sha256 of the report. The
# report must hold passes only (a skip only where the marker accepts it), a summary with comparisons in it,
# a deployed tree from the checkout's commit, and be no older than the build. Sets MARK_ERR and MARK_KIND.
MARK_ERR=""; MARK_KIND=""
verify_marker() {  # $1 = directory
    local dir="$1" marker current head line rhead="" rsum="" skips="" rep="$1/shadow-report.txt"
    local npass=0 ndiff=0 nskip=0 total="" spass="" sdiff="" ssk="" hdr_dep="" hdr_chk="" prog w
    MARK_ERR=""; MARK_KIND=""
    marker="$(table_lines "$dir/shadow-ok")"
    if [ -z "$marker" ]; then MARK_KIND=empty; MARK_ERR="$dir/shadow-ok holds no table"; return 1; fi
    current="$(table_of "$dir")"
    if [ "$marker"$'\n' != "$current"$'\n' ]; then MARK_KIND=table; MARK_ERR="the programs in $dir are not the ones shadow compared"; return 1; fi
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in
            "# checkout: "*) rhead="${line#\# checkout: }" ;;
            "# report: "*) rsum="${line#\# report: }" ;;
            "# accepted-skip: "*) skips="$skips${line#\# accepted-skip: }"$'\n' ;;
        esac
    done < "$dir/shadow-ok"
    head="$(git -C "$root" rev-parse HEAD 2>/dev/null)" || { MARK_ERR="cannot read HEAD of $root"; return 1; }
    if [ "$rhead" != "$head" ]; then MARK_ERR="shadow-ok was made for the checkout at ${rhead:0:12} and the checkout is at ${head:0:12}; run shadow again"; return 1; fi
    if [ ! -f "$rep" ] || [ -L "$rep" ]; then MARK_ERR="there is no shadow-report.txt beside shadow-ok"; return 1; fi
    if [ "$(sha256_of "$rep")" != "$rsum" ]; then MARK_ERR="shadow-report.txt is not the report shadow-ok was made from"; return 1; fi
    if [ "$dir/SHA256SUMS" -nt "$rep" ]; then MARK_ERR="the build is newer than the shadow report; run shadow again"; return 1; fi
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in
            "PASS "*) npass=$((npass + 1)) ;;
            "DIFF "*) ndiff=$((ndiff + 1)) ;;
            "SKIP "*)
                nskip=$((nskip + 1))
                prog="${line#*: no Rust build of }"; prog="${prog%% in *}"
                if ! has_name "$skips" "$prog"; then MARK_ERR="the report has a SKIP for $prog that shadow-ok does not accept"; return 1; fi ;;
            "SUMMARY comparisons="*)
                total="${line#SUMMARY comparisons=}"; total="${total%% *}"
                spass="${line#*pass=}"; spass="${spass%% *}"; sdiff="${line#*diff=}"; sdiff="${sdiff%% *}"; ssk="${line#*skip=}"; ssk="${ssk%% *}" ;;
            "SHADOW "*)
                for w in $line; do
                    case "$w" in deployed=*) hdr_dep="${w#deployed=}" ;; checkout=*) hdr_chk="${w#checkout=}" ;; esac
                done ;;
        esac
    done < "$rep"
    if [ "$ndiff" != 0 ]; then MARK_ERR="the shadow report has $ndiff DIFF line(s)"; return 1; fi
    if [[ ! "$total" =~ ^[0-9]+$ ]] || [ "$total" -le 0 ] || [ "$npass" -le 0 ]; then MARK_ERR="the shadow report holds no comparison"; return 1; fi
    if [ "$spass" != "$npass" ] || [ "$sdiff" != 0 ] || [ "$ssk" != "$nskip" ] || [ "$total" != "$((npass + nskip))" ]; then MARK_ERR="the shadow report's summary does not match its lines"; return 1; fi
    if [ -z "$hdr_dep" ] || [ "${head:0:${#hdr_dep}}" != "$hdr_dep" ] || [ -z "$hdr_chk" ] || [ "${head:0:${#hdr_chk}}" != "$hdr_chk" ]; then
        MARK_ERR="shadow compared a deployed tree from ${hdr_dep:-unknown} and the checkout is at ${head:0:7}; install deploys the checkout"; return 1
    fi
    return 0
}

# --- status --------------------------------------------------------------------------------

cmd_status() {
    local dir=""
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --build-dir) [ "$#" -ge 2 ] || usage_die "status: --build-dir needs a directory"; dir="$2"; shift 2 ;;
            *) usage_die "status: unknown argument $(shown "$1")" ;;
        esac
    done
    need sha256sum
    [ -n "$dir" ] || dir="$(default_dir)"
    dir="$(absolute_dir "the build directory" "$dir")"

    local have_stamp=0
    if [ -e "$stamp_file" ] || [ -L "$stamp_file" ]; then
        [ ! -L "$stamp_file" ] || die "$stamp_file is a symbolic link; refused"
        [ -f "$stamp_file" ] || die "$stamp_file is not a regular file; refused"
        read_stamp || die "$ST_ERR"
        have_stamp=1
    fi

    local n path k=-1 i state detail
    printf 'programs on this machine (%s):\n' "$HOME"
    for n in $SWAP_NAMES; do
        path="$(installed_path "$n")"
        k=-1
        if [ "$have_stamp" = 1 ]; then
            for i in "${!ST_PATH[@]}"; do [ "${ST_PATH[$i]}" = "$path" ] && k="$i"; done
        fi
        if [ -L "$path" ]; then state="link"; detail="a symbolic link, not read"
        elif [ ! -e "$path" ]; then state="missing"; detail=""
        elif [ ! -f "$path" ]; then state="other"; detail="not a regular file"
        elif [ "$k" -ge 0 ]; then
            if [ "$(sha256_of "$path")" = "${ST_HASH[$k]}" ]; then state="rust"; detail="matches RUST_STAMP"
            else state="changed"; detail="listed in RUST_STAMP and no longer the stamped build"; fi
        else
            state="bash"; detail=""
        fi
        printf '  %-18s %-8s %s\n' "$n" "$state" "$detail"
    done
    if [ "$have_stamp" = 1 ]; then printf 'RUST_STAMP: present, %d program(s)\n' "${#ST_PATH[@]}"
    else printf 'RUST_STAMP: absent\n'; fi

    if [ ! -d "$dir" ] || [ -L "$dir" ]; then
        printf 'shadow-ok: absent (no build directory %s)\n' "$dir"
    elif [ ! -f "$dir/shadow-ok" ] || [ -L "$dir/shadow-ok" ]; then
        printf 'shadow-ok: absent (%s)\n' "$dir"
    else
        if ( verify_marker "$dir" ) >/dev/null 2>&1; then
            printf 'shadow-ok: current (the marker lists the programs now in %s)\n' "$dir"
        else
            ( verify_marker "$dir" >/dev/null 2>&1 || :; printf 'shadow-ok: stale (%s)\n' "${MARK_ERR:-the marker cannot be verified}" )
        fi
    fi
}

# --- live ----------------------------------------------------------------------------------

# Runs a Rust program under the name of the bash program it stands for: the tree it works on
# is found from that name, as the wrapper of rust/scripts/assemble-tree.sh does.
as_program() {  # $1 = argv[0], $2 = the Rust binary, then its arguments
    bash -c 'exec -a "$1" "$2" "${@:3}"' _ "$@"
}

cmd_live() {
    local dir=""
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --build-dir) [ "$#" -ge 2 ] || usage_die "live: --build-dir needs a directory"; dir="$2"; shift 2 ;;
            *) usage_die "live: unknown argument $(shown "$1")" ;;
        esac
    done
    need sha256sum git stat id sort cat bash
    [ -n "$dir" ] || dir="$(default_dir)"
    dir="$(absolute_dir "the build directory" "$dir")"
    check_dir "$dir"

    [ -f "$dir/shadow-ok" ] && [ ! -L "$dir/shadow-ok" ] \
        || die "no shadow-ok in $dir: run scripts/cutover.sh shadow on this build first"

    [ -f "$dir/BUILD_INFO" ] && [ ! -L "$dir/BUILD_INFO" ] || die "$dir/BUILD_INFO is missing"
    local info_head info_dirty line
    info_head=""; info_dirty=""
    while IFS= read -r line; do
        case "$line" in head=*) info_head="${line#head=}" ;; dirty=*) info_dirty="${line#dirty=}" ;; esac
    done < "$dir/BUILD_INFO"
    [ "$info_dirty" = 0 ] || die "the build in $dir was made from a dirty tree; live deploys committed code only"
    local head status
    head="$(git -C "$root" rev-parse HEAD 2>/dev/null)" || die "cannot read HEAD of $root"
    [ "$head" = "$info_head" ] || die "the build in $dir is from ${info_head:0:12} and the checkout is at ${head:0:12}; install deploys the checkout, so build and shadow again"
    status="$(git -C "$root" status --porcelain 2>/dev/null)" || die "cannot read the git status of $root"
    [ -z "$status" ] || die "the checkout is dirty; install would deploy files no build was made from"

    local marker current
    if ! verify_marker "$dir"; then
        [ "$MARK_KIND" != table ] || die "$MARK_ERR (shadow-ok lists another table); run shadow again"
        die "$MARK_ERR"
    fi
    marker="$(table_lines "$dir/shadow-ok")"
    current="$(table_of "$dir")"

    if [ -e "$stamp_file" ] || [ -L "$stamp_file" ]; then
        die "$stamp_file exists: a Rust install is already in place; run scripts/cutover.sh rollback first"
    fi

    # What the install must leave behind.
    local names have_install=0 n expected=() uname_s
    names="$(table_names "$marker")"
    has_name "$names" install && have_install=1
    [ "$have_install" = 1 ] || die "the build in $dir holds no install program"
    uname_s="$(uname -s)"
    for n in $SWAP_NAMES; do
        has_name "$names" "$n" || continue
        case "$n" in run-instance|alert) [ "$uname_s" = Linux ] || continue ;; esac
        [ -f "$(source_path "$n")" ] || die "the checkout has no $(source_path "$n"), the bash program $n stands for"
        expected+=("$n")
    done
    [ "${#expected[@]}" -gt 0 ] || die "the build in $dir holds none of: $SWAP_NAMES"

    printf 'live: dry run of the install (nothing changes)\n'
    local rc=0
    as_program "$root/install.sh" "$dir/install" --rust --rust-bin "$dir" --dry-run || rc=$?
    [ "$rc" = 0 ] || die "the dry run of the install exited $rc; nothing was installed"

    printf 'live: installing\n'
    rc=0
    as_program "$root/install.sh" "$dir/install" --rust --rust-bin "$dir" || rc=$?
    if [ "$rc" != 0 ]; then
        printf '%s: the install exited %s; if it stopped part way, put the bash programs back with: scripts/cutover.sh rollback (it refuses when there is nothing to roll back)\n' "$PROG" "$rc" >&2
        exit 1
    fi

    # Read it back from the machine: what the install printed is not evidence.
    local problems=() path src want got k i
    if [ ! -f "$stamp_file" ] || [ -L "$stamp_file" ]; then
        problems+=("$stamp_file is missing after the install")
    elif ! read_stamp; then
        problems+=("$ST_ERR")
    else
        [ "${#ST_PATH[@]}" = "${#expected[@]}" ] \
            || problems+=("RUST_STAMP lists ${#ST_PATH[@]} program(s) and ${#expected[@]} were expected")
    fi
    for n in "${expected[@]}"; do
        path="$(installed_path "$n")"; src="$(source_path "$n")"
        want="$(table_hash "$current" "$n")"
        if [ -L "$path" ] || [ ! -f "$path" ]; then
            problems+=("$path is not a regular file after the install"); continue
        fi
        got="$(sha256_of "$path")"
        [ "$got" = "$want" ] || problems+=("$path is not the Rust build ($n): sha256 ${got:0:12}, wanted ${want:0:12}")
        if [ -L "$path.sh" ] || [ ! -f "$path.sh" ]; then
            problems+=("the bash original $path.sh is missing")
        else
            got="$(sha256_of "$path.sh")"; want="$(sha256_of "$src")"
            [ "$got" = "$want" ] || problems+=("$path.sh is not the bash original (sha256 ${got:0:12}, the checkout's $src is ${want:0:12})")
        fi
        k=-1
        for i in "${!ST_PATH[@]}"; do [ "${ST_PATH[$i]}" = "$path" ] && k="$i"; done
        if [ "$k" -lt 0 ]; then problems+=("RUST_STAMP does not list $path")
        else
            want="$(table_hash "$current" "$n")"
            [ "${ST_HASH[$k]}" = "$want" ] || problems+=("RUST_STAMP gives $path another hash than the build's")
        fi
    done
    if [ "${#problems[@]}" -gt 0 ]; then
        for n in "${problems[@]}"; do printf '%s: read-back: %s\n' "$PROG" "$n" >&2; done
        printf '%s: the machine does not hold what the install reported. Put the bash programs back with: scripts/cutover.sh rollback\n' "$PROG" >&2
        exit 1
    fi
    printf 'live: read back %d program(s): each is the Rust build, each bash original is intact, RUST_STAMP lists exactly those\n' "${#expected[@]}"
}

# --- shadow --------------------------------------------------------------------------------

# Every shim below is written by this script, never taken from the machine, and logs each call
# to $CUTOVER_LOG_DIR. A call the shadow must not make is answered with 98 (a write) or 99 (a
# call nobody planned) and logged in `refused` or `unplanned`; the summary counts both.
write_shims() {  # $1 = directory for the shim sources
    local d="$1"
    mkdir -p -- "$d"
    cat > "$d/gh" <<'SHIM'
#!/usr/bin/env bash
# cutover shadow: gh. Answers a few reads from canned values, refuses every write, and logs
# every call. It never reaches GitHub.
log="${CUTOVER_LOG_DIR:?}"
line=""; for a in "$@"; do line="$line$(printf '%q' "$a") "; done
printf 'gh %s\n' "${line% }" >> "$log/calls"
refuse() { printf 'gh %s\n' "${line% }" >> "$log/refused"; echo "cutover shadow: gh must not write: $*" >&2; exit 98; }
plan_none() { printf 'gh %s\n' "${line% }" >> "$log/unplanned"; echo "cutover shadow: gh call nobody planned: $*" >&2; exit 99; }
prev=""
for a in "$@"; do
    case "$a" in
        -f|-F|--field|--raw-field|--input) refuse "$*" ;;
        POST|PUT|PATCH|DELETE) [ "$prev" = -X ] || [ "$prev" = --method ] && refuse "$*" ;;
    esac
    prev="$a"
done
case "${1:-}" in
    pr|issue)
        case "${2:-}" in
            comment|merge|review|close|reopen|edit|create|ready|lock|unlock|update-branch|delete|transfer|pin|unpin|checkout) refuse "$*" ;;
        esac ;;
    release|label|repo|secret|variable|workflow|gist|codespace|ssh-key|gpg-key|ruleset|cache|extension)
        if [ "${1:-}" = repo ] && [ "${2:-}" = view ]; then :; else refuse "$*"; fi ;;
esac
case "${1:-} ${2:-}" in
    "auth switch") exit 0 ;;
    "auth token"|"auth status"|"auth login") echo "cutover shadow: no credentials here" >&2; exit 1 ;;
    "api user")
        [ -n "${CUTOVER_GH_ACCOUNT:-}" ] || { echo "cutover shadow: no account planned" >&2; exit 1; }
        printf '%s\n' "$CUTOVER_GH_ACCOUNT"; exit 0 ;;
    "pr list") exit 0 ;;
    "run list") echo '[]'; exit 0 ;;
    "pr view")
        for a in "$@"; do
            case "$a" in
                headRefOid) [ -n "${CUTOVER_GH_HEAD:-}" ] && { printf '%s\n' "$CUTOVER_GH_HEAD"; exit 0; } ;;
                baseRefName) printf 'main\n'; exit 0 ;;
            esac
        done
        plan_none "$*" ;;
    "pr diff")
        [ -n "${CUTOVER_GH_DIFF:-}" ] && [ -f "$CUTOVER_GH_DIFF" ] && { cat -- "$CUTOVER_GH_DIFF"; exit 0; }
        plan_none "$*" ;;
esac
if [ "${1:-}" = api ]; then
    case "${2:-}" in
        repos/*/git/ref/tags/*)
            if [ -n "${CUTOVER_GH_TAG_SHA:-}" ]; then
                tag="${2#*/git/ref/tags/}"
                printf 'refs/tags/%s\tcommit\t%s\n' "$tag" "$CUTOVER_GH_TAG_SHA"; exit 0
            fi ;;
    esac
fi
plan_none "$*"
SHIM
    cat > "$d/git" <<'SHIM'
#!/usr/bin/env bash
# cutover shadow: git. Reads pass through to the real git. `fetch` is swallowed (it would write
# refs and reach the network). A command that changes a repository runs only when the repository is
# below the TMPDIR of this run (the doctor's sandbox probe builds one); every other command that
# writes or reaches the network is refused.
log="${CUTOVER_LOG_DIR:?}"
line=""; for a in "$@"; do line="$line$(printf '%q' "$a") "; done
printf 'git %s\n' "${line% }" >> "$log/calls"
sub=""; skip=0; dir="$PWD"; takes_dir=0; unsafe=0
for a in "$@"; do
    if [ "$skip" = 1 ]; then
        skip=0
        if [ "$takes_dir" = 1 ]; then
            takes_dir=0
            case "$a" in /*) dir="$a" ;; *) dir="$dir/$a" ;; esac
        fi
        continue
    fi
    case "$a" in
        -C) skip=1; takes_dir=1 ;;
        -c|--namespace|--exec-path) skip=1 ;;
        --git-dir|--work-tree) skip=1; unsafe=1 ;;
        --git-dir=*|--work-tree=*) unsafe=1 ;;
        -*) ;;
        *) sub="$a"; break ;;
    esac
done
inside_tmp() {  # $1 = a path: true when it is below the TMPDIR of this run
    local p t
    p="$(readlink -m -- "$1")" || return 1
    t="$(readlink -m -- "${TMPDIR:-/nonexistent}")" || return 1
    case "$p/" in "$t"/*) return 0 ;; esac
    return 1
}
case "$sub" in
    fetch) printf 'git %s\n' "${line% }" >> "$log/swallowed"; exit 0 ;;
    rev-parse|rev-list|describe|log|diff|diff-tree|show|status|merge-base|for-each-ref|ls-files|ls-tree|cat-file|show-ref|name-rev|shortlog|blame|grep|version|help|--version)
        exec "${CUTOVER_REAL_GIT:?}" "$@" ;;
    config)
        for a in "$@"; do case "$a" in --get|--get-all|--get-regexp|--list|-l) exec "${CUTOVER_REAL_GIT:?}" "$@" ;; esac; done ;;
    "") exec "${CUTOVER_REAL_GIT:?}" "$@" ;;
esac
case "$sub" in
    pull|push|clone|ls-remote|remote|submodule) ;;
    *)
        if [ "$unsafe" = 0 ]; then
            target="$dir"
            if [ "$sub" = init ]; then
                last=""; for a in "$@"; do last="$a"; done
                case "$last" in -*|init) ;; /*) target="$last" ;; *) target="$dir/$last" ;; esac
            fi
            if inside_tmp "$target"; then exec "${CUTOVER_REAL_GIT:?}" "$@"; fi
        fi ;;
esac
printf 'git %s\n' "${line% }" >> "$log/refused"
echo "cutover shadow: git $sub must not run here" >&2
exit 98
SHIM
    cat > "$d/date" <<'SHIM'
#!/usr/bin/env bash
# cutover shadow: date. `date -Is` answers a fixed instant, so a prompt that carries its start
# time can be compared byte for byte; every other call goes to the real date.
if [ "$*" = "-Is" ]; then printf '%s\n' "${CUTOVER_FIXED_DATE:?}"; exit 0; fi
exec "${CUTOVER_REAL_DATE:?}" "$@"
SHIM
    cat > "$d/notify-send" <<'SHIM'
#!/usr/bin/env bash
# cutover shadow: notify-send. Logs the call and the two variables that decide where it goes.
log="${CUTOVER_LOG_DIR:?}"
line=""; for a in "$@"; do line="$line$(printf '%q' "$a") "; done
printf 'notify-send %s\nDISPLAY=%s\nDBUS=%s\n' "${line% }" "${DISPLAY-<unset>}" "${DBUS_SESSION_BUS_ADDRESS-<unset>}" >> "$log/calls"
exit 0
SHIM
    cat > "$d/crontab" <<'SHIM'
#!/usr/bin/env bash
# cutover shadow: crontab. Listing answers "no crontab"; anything that would write is refused.
log="${CUTOVER_LOG_DIR:?}"
line=""; for a in "$@"; do line="$line$(printf '%q' "$a") "; done
printf 'crontab %s\n' "${line% }" >> "$log/calls"
if [ "${1:-}" = -l ] && [ "$#" -eq 1 ]; then echo "no crontab for ${USER:-user}" >&2; exit 1; fi
printf 'crontab %s\n' "${line% }" >> "$log/refused"
echo "cutover shadow: crontab must not write" >&2
exit 98
SHIM
    cat > "$d/systemctl" <<'SHIM'
#!/usr/bin/env bash
# cutover shadow: systemctl. Read-only verbs of the user manager pass through; the rest is refused.
log="${CUTOVER_LOG_DIR:?}"
line=""; for a in "$@"; do line="$line$(printf '%q' "$a") "; done
printf 'systemctl %s\n' "${line% }" >> "$log/calls"
if [ "${1:-}" = --user ]; then
    case "${2:-}" in
        show-environment|list-timers|list-unit-files|is-active|is-enabled|is-failed|show|cat|status|list-units)
            exec "${CUTOVER_REAL_SYSTEMCTL:?}" "$@" ;;
    esac
fi
printf 'systemctl %s\n' "${line% }" >> "$log/refused"
echo "cutover shadow: systemctl must only read" >&2
exit 98
SHIM
    cat > "$d/systemd-run" <<'SHIM'
#!/usr/bin/env bash
# cutover shadow: systemd-run. The doctor probes the sandbox with a transient user unit it waits for
# (--wait), and the linter sweep runs shellcheck in a transient scope (--scope). Both end with their
# command, and they are the uses allowed. A unit that is left running is refused.
log="${CUTOVER_LOG_DIR:?}"
line=""; for a in "$@"; do line="$line$(printf '%q' "$a") "; done
printf 'systemd-run %s\n' "${line% }" >> "$log/calls"
if [ "${1:-}" = --user ]; then
    for a in "$@"; do
        case "$a" in --wait|--scope) exec "${CUTOVER_REAL_SYSTEMD_RUN:?}" "$@" ;; esac
    done
fi
printf 'systemd-run %s\n' "${line% }" >> "$log/refused"
echo "cutover shadow: systemd-run may only run a unit it waits for or a scope" >&2
exit 98
SHIM
    cat > "$d/loginctl" <<'SHIM'
#!/usr/bin/env bash
# cutover shadow: loginctl. Reads pass through; the rest is refused.
log="${CUTOVER_LOG_DIR:?}"
line=""; for a in "$@"; do line="$line$(printf '%q' "$a") "; done
printf 'loginctl %s\n' "${line% }" >> "$log/calls"
case "${1:-}" in
    show-user|show-session) exec "${CUTOVER_REAL_LOGINCTL:?}" "$@" ;;
esac
printf 'loginctl %s\n' "${line% }" >> "$log/refused"
echo "cutover shadow: loginctl must only read" >&2
exit 98
SHIM
    cat > "$d/podman" <<'SHIM'
#!/usr/bin/env bash
# cutover shadow: podman, for the containment probe only. It may inspect an image and run a
# container that removes itself; it may not pull, build, push or change anything else.
log="${CUTOVER_LOG_DIR:?}"
line=""; for a in "$@"; do line="$line$(printf '%q' "$a") "; done
printf 'podman %s\n' "${line% }" >> "$log/calls"
case "${1:-} ${2:-}" in
    "image inspect") exec "${CUTOVER_REAL_PODMAN:?}" "$@" ;;
esac
if [ "${1:-}" = run ]; then
    rm_seen=0
    for a in "$@"; do [ "$a" = --rm ] && rm_seen=1; done
    [ "$rm_seen" = 1 ] && exec "${CUTOVER_REAL_PODMAN:?}" "$@"
fi
printf 'podman %s\n' "${line% }" >> "$log/refused"
echo "cutover shadow: podman may only inspect an image or run a container with --rm" >&2
exit 98
SHIM
    cat > "$d/nothing" <<'SHIM'
#!/usr/bin/env bash
# cutover shadow: a program that must not run at all (a model backend, the network, a container
# runtime other than the probe's). Logs the call and ends with 99.
log="${CUTOVER_LOG_DIR:?}"
name="$(basename "$0")"
line=""; for a in "$@"; do line="$line$(printf '%q' "$a") "; done
printf '%s %s\n' "$name" "${line% }" >> "$log/calls"
printf '%s %s\n' "$name" "${line% }" >> "$log/unplanned"
echo "cutover shadow: $name must not run" >&2
exit 99
SHIM
    chmod 755 "$d"/*
}

# The names answered by `nothing`.
NOTHING_NAMES="claude codex cursor-agent opencode curl wget ssh scp sudo docker nc"

# Builds one search-path directory of links to the shims. $2 names the shims it holds.
shim_path_dir() {  # $1 = the directory to fill, $2 = the shim sources, $3... = names that link to `nothing`
    local d="$1" src="$2" n; shift 2
    mkdir -p -- "$d"
    for n in gh git date notify-send crontab systemctl loginctl systemd-run; do
        [ -e "$src/$n" ] || continue
        ln -s -- "$src/$n" "$d/$n"
    done
    for n in "$@"; do ln -s -- "$src/nothing" "$d/$n"; done
}

# Normalisers: only what a clock or a process id changes, and nothing else.
normalise_doctor() {  # stdin -> stdout
    sed -E \
        -e 's/[0-9]{4}-[0-9]{2}-[0-9]{2}[T ][0-9]{2}:[0-9]{2}:[0-9]{2}([.,][0-9]+)?(Z|[+-][0-9]{2}:?[0-9]{2})?/<TIME>/g' \
        -e 's/\b(pid|PID)[ =:]+[0-9]+/\1 <PID>/g'
}

# The random part of a name mktemp made, in the log of the calls a program made (the offline suite
# mounts such directories into a container): not a difference between two runs.
normalise_calls() {  # stdin -> stdout
    sed -E \
        -e 's#/tmp\.[A-Za-z0-9]{6,12}\b#/tmp.<RAND>#g' \
        -e 's#/verify-work/[A-Za-z0-9]{6,12}\b#/verify-work/<RAND>#g'
}

# first_diff BASH_FILE RUST_FILE: where two streams part, as one line.
first_diff() {
    local a="$1" b="$2" report n la lb
    report="$(cmp -- "$a" "$b" 2>&1 || true)"
    n="$(sed -n 's/.*line \([0-9][0-9]*\).*/\1/p' <<<"$report" | head -n 1)"
    if [ -z "$n" ]; then n=1   # one file is empty: it parts at the first line
    else
        # `EOF on FILE after byte N, line M`: the shorter file ends after line M, so the longer one parts at M+1.
        case "$report" in *EOF*) n=$((n + 1)) ;; esac
    fi
    la="$(sed -n "${n}p" -- "$a" | head -c 400 | tr -d '\n' | tr -c '[:print:]' '?' | cut -c1-110)"
    lb="$(sed -n "${n}p" -- "$b" | head -c 400 | tr -d '\n' | tr -c '[:print:]' '?' | cut -c1-110)"
    printf "line %s: bash='%s' rust='%s'" "$n" "$la" "$lb"
}

# --- the comparison runner -----------------------------------------------------------------

SH_DIR=""        # the build directory
SH_SCRATCH=""    # the scratch directory inside it
SH_TIMEOUT=300   # seconds, per side, per comparison
SH_CHECK_TIMEOUT=3600
SH_REPORT=""
SH_PASS=0; SH_DIFF=0; SH_SKIP=0
SH_PROMPTS=0
SH_REFUSED=0; SH_UNPLANNED=0
SH_SKIPPED_NAMES=""
SH_SIDE_PREP=""  # a function called as `fn SIDE_DIR` before each side runs
SH_XENV=()       # extra VAR=value for the program; %SIDE% becomes the side's own directory
SH_EXTRA=()      # files under the side's directory whose bytes are compared with the calls
SH_SEQ=0
SH_SIDE_PID=""   # the side that is running, so that a signal can stop it
SH_RAN=0         # 1 when the last comparison ran both sides, 0 when it was skipped
SH_KEEP="${CUTOVER_KEEP_SCRATCH:-}"  # a person reading the scratch directory afterwards sets this

report_line() { printf '%s\n' "$1" | tee -a "$SH_REPORT"; }

stop_side() {  # $1 = the exit status
    [ -z "$SH_SIDE_PID" ] || kill -TERM "$SH_SIDE_PID" 2>/dev/null || true
    exit "$1"
}

# Runs the program on one side. Sets nothing global except the files under $sd.
run_side() {  # $1 = bash|rust, $2 = side directory, $3 = path variable, $4 = cwd, $5 = timeout, $6 = bash program, $7 = argv0, $8 = rust binary, rest = args
    local side="$1" sd="$2" shpath="$3" cwd="$4" tmo="$5" bashprog="$6" argv0="$7" rustbin="$8"; shift 8
    local -a envv cmd
    # TMPDIR has one path for both sides, so a path a program prints from it is the same on both.
    rm -rf -- "$sd" "${sd%/*}/tmp"; mkdir -p -- "$sd/log" "${sd%/*}/tmp"
    [ -z "$SH_SIDE_PREP" ] || "$SH_SIDE_PREP" "$sd"
    envv=("HOME=$HOME" "USER=${USER:-$(id -un)}" "LOGNAME=${USER:-$(id -un)}" "PATH=$shpath" "LC_ALL=C" "TZ=UTC"
          "PYTHONDONTWRITEBYTECODE=1" "GIT_TERMINAL_PROMPT=0" "GIT_OPTIONAL_LOCKS=0" "TMPDIR=${sd%/*}/tmp"
          "CUTOVER_LOG_DIR=$sd/log" "CUTOVER_FIXED_DATE=2026-01-01T00:00:00+00:00"
          "CUTOVER_REAL_DATE=$SH_REAL_DATE" "CUTOVER_REAL_GIT=$SH_REAL_GIT")
    [ -z "$SH_REAL_SYSTEMCTL" ] || envv+=("CUTOVER_REAL_SYSTEMCTL=$SH_REAL_SYSTEMCTL")
    [ -z "$SH_REAL_LOGINCTL" ] || envv+=("CUTOVER_REAL_LOGINCTL=$SH_REAL_LOGINCTL")
    [ -z "$SH_REAL_SYSTEMD_RUN" ] || envv+=("CUTOVER_REAL_SYSTEMD_RUN=$SH_REAL_SYSTEMD_RUN")
    [ -z "$SH_REAL_PODMAN" ] || envv+=("CUTOVER_REAL_PODMAN=$SH_REAL_PODMAN")
    [ -z "${XDG_RUNTIME_DIR:-}" ] || envv+=("XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR")
    [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ] || envv+=("DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS")
    local x
    for x in "${SH_XENV[@]}"; do envv+=("${x//%SIDE%/$sd}"); done
    if [ "$side" = rust ]; then
        # The Rust programs hand a child only an allowlist of variables, plus the names listed here.
        local names="" e
        for e in "${envv[@]}"; do names="$names ${e%%=*}"; done
        envv+=("MAINTAINER_CHILD_ENV=${names# }")
        cmd=(bash -c 'exec -a "$1" "$2" "${@:3}"' _ "$argv0" "$rustbin" "$@")
    else
        cmd=("$bashprog" "$@")
    fi
    local rc=0
    # In the background so that a signal reaches this shell while the side runs; the subshell becomes
    # `timeout`, which signals the whole group of the program when it is told to stop.
    ( cd -- "$cwd" && exec env -i "${envv[@]}" timeout -k 5 "$tmo" "${cmd[@]}" ) > "$sd/stdout" 2> "$sd/stderr" < /dev/null &
    SH_SIDE_PID=$!
    wait "$SH_SIDE_PID" || rc=$?
    SH_SIDE_PID=""
    printf '%s' "$rc" > "$sd/rc"
    # Everything the shims logged, in file-name order.
    local f
    : > "$sd/calls.txt"
    for f in "$sd"/log/*; do
        [ -e "$f" ] || continue
        printf '== %s\n' "${f##*/}" >> "$sd/calls.txt"
        cat -- "$f" >> "$sd/calls.txt"
    done
    for f in "${SH_EXTRA[@]}"; do
        [ -f "$sd/$f" ] || continue
        printf '== %s\n' "$f" >> "$sd/calls.txt"
        cat -- "$sd/$f" >> "$sd/calls.txt"
    done
}

count_lines() {  # $1 = file; prints 0 when it is absent
    if [ -f "$1" ]; then wc -l < "$1" | tr -d ' '; else printf 0; fi
}

# compare NAME PROGRAM ARGV0 BASH_PROGRAM NORM SHIMS CWD TIMEOUT -- ARGS...
# PROGRAM is the file name in the build directory. NORM is ident or doctor. SHIMS names a search path.
compare() {
    local name="$1" prog="$2" argv0="$3" bashprog="$4" norm="$5" shims="$6" cwd="$7" tmo="$8"; shift 9
    SH_SEQ=$((SH_SEQ + 1)); SH_RAN=0
    local base="$SH_SCRATCH/cmp" rustbin="$SH_DIR/$prog" shpath rc_b rc_r why=""
    [ -z "$SH_KEEP" ] || base="$SH_SCRATCH/cmp-$SH_SEQ"
    case "$shims" in
        full) shpath="$SH_PATH_FULL" ;;
        nogit) shpath="$SH_PATH_NOGIT" ;;
        probe) shpath="$SH_PATH_PROBE" ;;
        *) die "internal: no shim set $shims" ;;
    esac
    if [ ! -f "$rustbin" ]; then
        SH_SKIP=$((SH_SKIP + 1))
        has_name "$SH_SKIPPED_NAMES" "$prog" || SH_SKIPPED_NAMES="$SH_SKIPPED_NAMES$prog"$'\n'
        report_line "SKIP $name: no Rust build of $prog in $SH_DIR (the bash program stays)"
        SH_XENV=(); SH_SIDE_PREP=""; SH_EXTRA=()
        return 0
    fi
    SH_RAN=1
    [ -f "$bashprog" ] && [ -x "$bashprog" ] || die "the deployed bash program $bashprog is missing or not executable, so there is nothing to compare $name with"
    rm -rf -- "$base"; mkdir -p -- "$base"
    run_side bash "$base/bash" "$shpath" "$cwd" "$tmo" "$bashprog" "$argv0" "$rustbin" "$@"
    run_side rust "$base/rust" "$shpath" "$cwd" "$tmo" "$bashprog" "$argv0" "$rustbin" "$@"
    SH_XENV=(); SH_SIDE_PREP=""; SH_EXTRA=()
    rc_b="$(cat "$base/bash/rc")"; rc_r="$(cat "$base/rust/rc")"
    case "$rc_b" in 124|137) why="the bash side timed out after ${tmo}s" ;; esac
    case "$rc_r" in 124|137) why="${why:+$why; }the Rust side timed out after ${tmo}s" ;; esac
    local s
    for s in stdout stderr; do
        case "$norm" in
            doctor) normalise_doctor < "$base/bash/$s" > "$base/bash/$s.n"; normalise_doctor < "$base/rust/$s" > "$base/rust/$s.n" ;;
            *) cp -- "$base/bash/$s" "$base/bash/$s.n"; cp -- "$base/rust/$s" "$base/rust/$s.n" ;;
        esac
    done
    normalise_calls < "$base/bash/calls.txt" > "$base/bash/calls.n"
    normalise_calls < "$base/rust/calls.txt" > "$base/rust/calls.n"
    SH_REFUSED=$((SH_REFUSED + $(count_lines "$base/bash/log/refused") + $(count_lines "$base/rust/log/refused")))
    SH_UNPLANNED=$((SH_UNPLANNED + $(count_lines "$base/bash/log/unplanned") + $(count_lines "$base/rust/log/unplanned")))
    if [ -n "$why" ]; then
        SH_DIFF=$((SH_DIFF + 1)); report_line "DIFF $name: $why"
    elif [ "$rc_b" != "$rc_r" ]; then
        SH_DIFF=$((SH_DIFF + 1)); report_line "DIFF $name: exit code: bash=$rc_b rust=$rc_r"
    elif ! cmp -s "$base/bash/stdout.n" "$base/rust/stdout.n"; then
        SH_DIFF=$((SH_DIFF + 1)); report_line "DIFF $name: stdout $(first_diff "$base/bash/stdout.n" "$base/rust/stdout.n")"
    elif ! cmp -s "$base/bash/stderr.n" "$base/rust/stderr.n"; then
        SH_DIFF=$((SH_DIFF + 1)); report_line "DIFF $name: stderr $(first_diff "$base/bash/stderr.n" "$base/rust/stderr.n")"
    elif ! cmp -s "$base/bash/calls.n" "$base/rust/calls.n"; then
        SH_DIFF=$((SH_DIFF + 1)); report_line "DIFF $name: calls to the shims $(first_diff "$base/bash/calls.n" "$base/rust/calls.n")"
    else
        SH_PASS=$((SH_PASS + 1)); report_line "PASS $name"
    fi
}

# The tasks a profile declares, read the way run.sh reads them: by sourcing the profile.
profile_tasks() {  # $1 = profile name
    local out
    out="$(env -i HOME="$HOME" PATH="$PATH" LC_ALL=C bash -c 'set -u; . "$1" >/dev/null 2>&1 || exit 3; printf "%s" "${TASKS:-}"' _ "$share/profiles/$1/profile.env" 2>/dev/null)" \
        || die "cannot read TASKS from the profile $1"
    printf '%s' "$out"
}

plain_profile() { [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; }
plain_task() { [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9_]*$ ]]; }

# A digest of the deployed tree and of the checkout's git state: shadow must leave both as they were.
machine_digest() {
    local sum
    sum="$({
        find "$share" -type f -printf '%p %s %T@\n' 2>/dev/null | LC_ALL=C sort
        local n; for n in $SWAP_NAMES; do local p; p="$(installed_path "$n")"; [ -e "$p" ] && stat -c '%n %s %Y' -- "$p"; done
        git -C "$root" rev-parse HEAD 2>/dev/null
        git -C "$root" status --porcelain 2>/dev/null
    } | sha256sum)" || die "cannot digest the machine state"
    printf '%s' "${sum%% *}"
}

cmd_shadow() {
    local dir="" accepted=() tmo_override=""
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --out) [ "$#" -ge 2 ] || usage_die "shadow: --out needs a directory"; dir="$2"; shift 2 ;;
            --accept-skip) [ "$#" -ge 2 ] || usage_die "shadow: --accept-skip needs a program name"; accepted+=("$2"); shift 2 ;;
            --timeout) [ "$#" -ge 2 ] || usage_die "shadow: --timeout needs a number of seconds"; tmo_override="$2"; shift 2 ;;
            *) usage_die "shadow: unknown argument $(shown "$1")" ;;
        esac
    done
    need sha256sum git python3 bash timeout cmp sed tr cut head wc find sort stat id mktemp env cat
    local a
    for a in "${accepted[@]}"; do
        [[ "$a" =~ ^[a-z][a-z0-9-]*$ ]] || die "--accept-skip $(shown "$a") is not a program name"
    done
    if [ -n "$tmo_override" ]; then
        [[ "$tmo_override" =~ ^[1-9][0-9]*$ ]] || die "--timeout wants a whole number of seconds, not $(shown "$tmo_override")"
        SH_TIMEOUT="$tmo_override"; SH_CHECK_TIMEOUT="$tmo_override"
    fi
    [ -n "$dir" ] || dir="$(default_dir)"
    dir="$(absolute_dir "the build directory" "$dir")"
    check_dir "$dir"
    SH_DIR="$dir"

    # The build must be what `build` made: the table it wrote is the table now.
    [ -f "$dir/SHA256SUMS" ] && [ ! -L "$dir/SHA256SUMS" ] || die "no SHA256SUMS in $dir: run scripts/cutover.sh build first"
    local table recorded
    table="$(table_of "$dir")"
    recorded="$(table_lines "$dir/SHA256SUMS")"
    [ "$table"$'\n' = "$recorded"$'\n' ] || die "the programs in $dir are not the ones build recorded in SHA256SUMS; build again"

    # The old verdict goes first: a run that ends early cannot leave a marker that says ok.
    rm -f -- "$dir/shadow-ok" "$dir/shadow-report.txt"

    # What is deployed, and what it holds.
    [ -d "$share/profiles" ] || die "no deployed profiles directory $share/profiles; shadow compared nothing"
    local profiles=() d p
    for d in "$share"/profiles/*/profile.env; do
        [ -f "$d" ] || continue
        p="${d%/profile.env}"; p="${p##*/}"
        plain_profile "$p" || die "the deployed profile name $(shown "$p") is not a plain name; refused before anything ran"
        profiles+=("$p")
    done
    [ "${#profiles[@]}" -gt 0 ] || die "no profile is deployed under $share/profiles; shadow compared nothing"
    local tasks_of=() t ts tl words
    for p in "${profiles[@]}"; do
        ts="$(profile_tasks "$p")"
        # Split on white space (a newline too) without expanding a glob character against the working directory.
        words=()
        read -r -d '' -a words <<<"$ts" || true
        [ "${#words[@]}" -gt 0 ] || die "the profile $p declares no TASKS; shadow would compare nothing for it"
        tl=""
        for t in "${words[@]}"; do
            plain_task "$t" || die "the task name $(shown "$t") of profile $p is not a plain name; refused before anything ran"
            tl="$tl $t"
        done
        tasks_of+=("${tl# }")
    done

    SH_REAL_DATE="$(command -v date)"; SH_REAL_GIT="$(command -v git)"
    SH_REAL_SYSTEMCTL="$(command -v systemctl 2>/dev/null || true)"
    SH_REAL_LOGINCTL="$(command -v loginctl 2>/dev/null || true)"
    SH_REAL_SYSTEMD_RUN="$(command -v systemd-run 2>/dev/null || true)"
    SH_REAL_PODMAN="$(command -v podman 2>/dev/null || true)"
    for a in "$SH_REAL_DATE" "$SH_REAL_GIT"; do case "$a" in /*) ;; *) die "date and git must be programs on PATH, not shell functions" ;; esac; done
    for a in "$SH_REAL_SYSTEMCTL" "$SH_REAL_LOGINCTL" "$SH_REAL_PODMAN" "$SH_REAL_SYSTEMD_RUN"; do case "$a" in ''|/*) ;; *) die "a tool on PATH is not a program" ;; esac; done

    SH_SCRATCH="$(mktemp -d "$dir/.shadow.XXXXXX")" || die "cannot make a scratch directory inside $dir"
    # A signal stops the side that is running and ends the run, and the exit removes the scratch directory.
    trap 'stop_side 143' TERM
    trap 'stop_side 130' INT
    trap 'stop_side 129' HUP
    if [ -z "$SH_KEEP" ]; then
        CLEAN_DIR="$SH_SCRATCH"
        trap clean_up EXIT
    else
        printf 'shadow: keeping the scratch directory %s\n' "$SH_SCRATCH" >&2
    fi
    SH_REPORT="$dir/shadow-report.txt"
    : > "$SH_REPORT" || die "cannot write $SH_REPORT"

    write_shims "$SH_SCRATCH/shim-src"
    local real_path="$PATH"
    shim_path_dir "$SH_SCRATCH/shims-full" "$SH_SCRATCH/shim-src" $NOTHING_NAMES podman
    shim_path_dir "$SH_SCRATCH/shims-nogit" "$SH_SCRATCH/shim-src" $NOTHING_NAMES podman
    rm -f -- "$SH_SCRATCH/shims-nogit/git"
    shim_path_dir "$SH_SCRATCH/shims-probe" "$SH_SCRATCH/shim-src" $NOTHING_NAMES
    rm -f -- "$SH_SCRATCH/shims-probe/git"
    if [ -n "$SH_REAL_PODMAN" ]; then ln -s -- "$SH_SCRATCH/shim-src/podman" "$SH_SCRATCH/shims-probe/podman"; fi
    [ -n "$SH_REAL_SYSTEMCTL" ] || rm -f -- "$SH_SCRATCH"/shims-*/systemctl
    [ -n "$SH_REAL_LOGINCTL" ] || rm -f -- "$SH_SCRATCH"/shims-*/loginctl
    [ -n "$SH_REAL_SYSTEMD_RUN" ] || rm -f -- "$SH_SCRATCH"/shims-*/systemd-run
    SH_PATH_FULL="$SH_SCRATCH/shims-full:$bindir:$real_path"
    SH_PATH_NOGIT="$SH_SCRATCH/shims-nogit:$bindir:$real_path"
    SH_PATH_PROBE="$SH_SCRATCH/shims-probe:$bindir:$real_path"

    local deployed_commit="unknown" build_head="unknown" line
    if [ -f "$share/VERSION" ]; then
        while IFS= read -r line; do case "$line" in commit=*) deployed_commit="${line#commit=}" ;; esac; done < "$share/VERSION"
    fi
    if [ -f "$dir/BUILD_INFO" ]; then
        while IFS= read -r line; do case "$line" in head=*) build_head="${line#head=}" ;; esac; done < "$dir/BUILD_INFO"
    fi
    report_line "SHADOW home=$HOME checkout=$(git -C "$root" rev-parse --short HEAD) deployed=$deployed_commit build=${build_head:0:7} profiles=${#profiles[@]}"
    local before; before="$(machine_digest)"

    shadow_comparisons "${profiles[@]}" -- "${tasks_of[@]}"

    local after; after="$(machine_digest)"
    local unchanged=yes
    [ "$before" = "$after" ] || unchanged=no
    local total=$((SH_PASS + SH_DIFF + SH_SKIP))
    report_line "SUMMARY comparisons=$total pass=$SH_PASS diff=$SH_DIFF skip=$SH_SKIP"
    report_line "PROMPTS compared=$SH_PROMPTS expected=$((SH_PROMPT_EXPECTED))"
    report_line "SHIMS refused=$SH_REFUSED unplanned=$SH_UNPLANNED (both sides together)"
    report_line "DEPLOYED tree and checkout unchanged by the run: $unchanged"
    if [ -n "$SH_SKIPPED_NAMES" ]; then
        report_line "SKIPPED programs (no Rust build): $(printf '%s' "$SH_SKIPPED_NAMES" | tr '\n' ' ')"
    fi

    # The marker: only a run in which every comparison passed. A skip counts unless it was
    # accepted by name, and an accepted skip is written into the marker.
    local why_not="" left=0 skipped_used=() n head_full
    head_full="$(git -C "$root" rev-parse HEAD 2>/dev/null)" || head_full=""
    [ -z "$(git -C "$root" status --porcelain 2>/dev/null)" ] || why_not="the checkout is dirty, so shadow compared nothing that install would deploy"
    if [ "$deployed_commit" = unknown ] || [ -z "$head_full" ] || [ "${head_full:0:${#deployed_commit}}" != "$deployed_commit" ]; then
        why_not="${why_not:+$why_not; }the deployed tree is from $deployed_commit and the checkout is at ${head_full:0:7}; run ./install.sh first, because live installs the checkout"
    fi
    if [ "$SH_DIFF" != 0 ]; then why_not="${why_not:+$why_not; }$SH_DIFF comparison(s) differ"; fi
    [ "$SH_PASS" -gt 0 ] || why_not="${why_not:+$why_not; }no comparison passed"
    [ "$SH_PROMPTS" -gt 0 ] || why_not="${why_not:+$why_not; }no prompt was compared"
    [ "$SH_PROMPTS" = "$SH_PROMPT_EXPECTED" ] || why_not="${why_not:+$why_not; }$SH_PROMPTS prompts were compared and $SH_PROMPT_EXPECTED were expected"
    [ "$SH_REFUSED" = 0 ] || why_not="${why_not:+$why_not; }a program tried $SH_REFUSED call(s) the shims refuse as writes"
    [ "$unchanged" = yes ] || why_not="${why_not:+$why_not; }the deployed tree or the checkout changed during the run"
    if [ -n "$SH_SKIPPED_NAMES" ]; then
        while IFS= read -r n; do
            [ -n "$n" ] || continue
            local ok=0
            for a in "${accepted[@]}"; do [ "$a" = "$n" ] && ok=1; done
            if [ "$ok" = 1 ]; then skipped_used+=("$n"); else left=$((left + 1)); fi
        done <<<"$SH_SKIPPED_NAMES"
        [ "$left" = 0 ] || why_not="${why_not:+$why_not; }$left program(s) had no Rust build to compare (name one with --accept-skip to go on without it)"
    fi
    # The programs must still be what was compared.
    [ "$(table_of "$dir")" = "$table" ] || why_not="${why_not:+$why_not; }the programs in $dir changed during the run"

    if [ -n "$why_not" ]; then
        report_line "MARKER not written: $why_not"
        return 1
    fi
    local tmp
    tmp="$(mktemp "$dir/.shadow-ok.XXXXXX")" || die "cannot write the marker"
    { printf '%s\n' "$table"; printf '# checkout: %s\n# report: %s\n' "$head_full" "$(sha256_of "$SH_REPORT")"
      for n in "${skipped_used[@]}"; do printf '# accepted-skip: %s\n' "$n"; done; } > "$tmp" \
        || { rm -f -- "$tmp"; die "cannot write the marker"; }
    chmod 644 -- "$tmp"
    mv -f -- "$tmp" "$dir/shadow-ok" || die "cannot place the marker"
    # Not a line of the report: the marker holds the report's sha256.
    printf 'MARKER written: %s\n' "$dir/shadow-ok"
}

SH_PROMPT_EXPECTED=0
SH_ALERT_PROFILE=""
SH_REAL_DATE=""; SH_REAL_GIT=""; SH_REAL_SYSTEMCTL=""; SH_REAL_LOGINCTL=""; SH_REAL_PODMAN=""; SH_REAL_SYSTEMD_RUN=""
SH_PATH_FULL=""; SH_PATH_NOGIT=""; SH_PATH_PROBE=""

# Every comparison. The profile names come first, then `--`, then one task list per profile.
shadow_comparisons() {
    local profiles=() lists=() seen=0 x
    for x in "$@"; do
        if [ "$x" = -- ]; then seen=1; continue; fi
        if [ "$seen" = 0 ]; then profiles+=("$x"); else lists+=("$x"); fi
    done
    local i p t style
    local T="$SH_TIMEOUT" TC="$SH_CHECK_TIMEOUT"

    # run --show-prompt: every deployed profile, each of its tasks, both prose styles.
    for i in "${!profiles[@]}"; do
        p="${profiles[$i]}"
        for t in ${lists[$i]}; do
            for style in default raw; do
                SH_XENV=()
                [ "$style" = raw ] && SH_XENV=("MAINTAINER_PROSE_STYLE=raw" "PROSE_STYLE=raw")
                SH_PROMPT_EXPECTED=$((SH_PROMPT_EXPECTED + 1))
                compare "run --show-prompt $p $t ($style)" run "$share/run.sh" "$share/run.sh" ident full "$HOME" "$T" -- --show-prompt "$p" "$t"
                # Only a comparison that ran both sides counts as a prompt compared.
                if [ "$SH_RAN" = 1 ]; then SH_PROMPTS=$((SH_PROMPTS + 1)); fi
            done
        done
    done
    SH_XENV=()
    compare "run --show-prompt (no profile)" run "$share/run.sh" "$share/run.sh" ident full "$HOME" "$T" -- --show-prompt
    compare "run --show-prompt ${profiles[0]} (no such task)" run "$share/run.sh" "$share/run.sh" ident full "$HOME" "$T" -- --show-prompt "${profiles[0]}" nosuchtask
    compare "run --show-prompt (no such profile)" run "$share/run.sh" "$share/run.sh" ident full "$HOME" "$T" -- --show-prompt nosuchprofile review

    # maintainer-doctor --quick: the container probe is the one part that touches podman state.
    for p in "${profiles[@]}"; do
        SH_XENV=("MAINTAINER_PROFILE=$p")
        compare "maintainer-doctor --quick $p" maintainer-doctor "$bindir/maintainer-doctor" "$bindir/maintainer-doctor" doctor full "$HOME" "$T" -- --quick
    done
    SH_XENV=()
    compare "maintainer-doctor --quick (no profile named)" maintainer-doctor "$bindir/maintainer-doctor" "$bindir/maintainer-doctor" doctor full "$HOME" "$T" -- --quick
    SH_XENV=("MAINTAINER_PROFILE=not a name")
    compare "maintainer-doctor --quick (bad profile name)" maintainer-doctor "$bindir/maintainer-doctor" "$bindir/maintainer-doctor" doctor full "$HOME" "$T" -- --quick

    # maintainer-repo: prune --dry-run and release-check read the repository (the shim swallows
    # the fetch each of them starts with); fixup needs a clone over the network and is only asked
    # for its usage refusals.
    for p in "${profiles[@]}"; do
        SH_XENV=("MAINTAINER_PROFILE=$p" "MAINTAINER_POST=off")
        compare "maintainer-repo prune --dry-run $p" maintainer-repo "$bindir/maintainer-repo" "$bindir/maintainer-repo" ident full "$HOME" "$T" -- prune --dry-run
        compare "maintainer-repo release-check $p" maintainer-repo "$bindir/maintainer-repo" "$bindir/maintainer-repo" ident full "$HOME" "$T" -- release-check
    done
    SH_XENV=("MAINTAINER_PROFILE=${profiles[0]}" "MAINTAINER_POST=off")
    compare "maintainer-repo (no verb)" maintainer-repo "$bindir/maintainer-repo" "$bindir/maintainer-repo" ident full "$HOME" "$T" --
    compare "maintainer-repo fixup (no pull request)" maintainer-repo "$bindir/maintainer-repo" "$bindir/maintainer-repo" ident full "$HOME" "$T" -- fixup

    # maintainer-merge: show reads receipts, verify-deps reads a pull request through gh. Both
    # run against a stub gh, a scratch state directory and a scratch receipt key.
    merge_comparisons "${profiles[0]}"

    # run-instance splits "<profile>-<task>" and hands both to run.sh. A profile that does not
    # exist is refused by run.sh before it does anything.
    SH_XENV=()
    compare "run-instance (no instance)" run-instance "$share/run-instance.sh" "$share/run-instance.sh" ident full "$HOME" "$T" --
    compare "run-instance nosuch-review (no such profile)" run-instance "$share/run-instance.sh" "$share/run-instance.sh" ident full "$HOME" "$T" -- nosuch-review
    # alert writes a line into the failed profile's alerts.log and sends a notification: it runs
    # against a scratch HOME, never this one.
    alert_comparisons "${profiles[0]}"

    # install, as the bash install.sh and the Rust install read the checkout and this machine.
    compare "install --dry-run" install "$root/install.sh" "$root/install.sh" ident full "$HOME" "$T" -- --dry-run
    compare "install --dry-run --timers" install "$root/install.sh" "$root/install.sh" ident full "$HOME" "$T" -- --dry-run --timers
    compare "install --uninstall --dry-run" install "$root/install.sh" "$root/install.sh" ident full "$HOME" "$T" -- --uninstall --dry-run
    compare "install --bogus" install "$root/install.sh" "$root/install.sh" ident full "$HOME" "$T" -- --bogus
    for p in "${profiles[@]}"; do
        SH_XENV=("MAINTAINER_PROFILE=$p")
        compare "install-cron --dry-run $p" install-cron "$root/platform/posix/install-cron.sh" "$root/platform/posix/install-cron.sh" ident full "$HOME" "$T" -- --dry-run
    done
    SH_XENV=()

    # The five repository checks, on the checkout.
    compare "check-claims" check-claims "$root/scripts/check_claims.sh" "$root/scripts/check_claims.sh" ident nogit "$root" "$TC" --
    compare "shellcheck-sweep" shellcheck-sweep "$root/scripts/shellcheck-sweep.sh" "$root/scripts/shellcheck-sweep.sh" ident nogit "$root" "$TC" --
    compare "verify-action-pins" verify-action-pins "$root/scripts/verify-action-pins.sh" "$root/scripts/verify-action-pins.sh" ident nogit "$root" "$T" --
    for p in "${profiles[@]}"; do
        SH_XENV=("MAINTAINER_PROFILE=$p")
        compare "run-evals $p (static)" run-evals "$root/evals/run-evals.sh" "$root/evals/run-evals.sh" ident nogit "$root" "$T" --
    done
    SH_XENV=()
    compare "containment-probe" containment-probe "$root/tests/containment-probe.sh" "$root/tests/containment-probe.sh" ident probe "$root" "$TC" --
}

merge_comparisons() {  # $1 = a profile name
    local T="$SH_TIMEOUT" mm="$bindir/maintainer-merge" args label
    for args in "show" "show 7" "show 8" "verify-deps" "verify-deps abc 1234567" "verify-deps 7 cccccccccccc"; do
        SH_SIDE_PREP=prep_merge_state
        SH_XENV=("MAINTAINER_STATE=%SIDE%/state" "MAINTAINER_SLUG=owner/repo" "MAINTAINER_REPO=%SIDE%/repo"
                 "MAINTAINER_ACCOUNT=shadow-account" "MAINTAINER_RECEIPT_KEY=%SIDE%/state/receipt.key"
                 "MAINTAINER_SCRIPTS=$share/scripts" "MAINTAINER_POST=off" "MAINTAINER_PROFILE=$1"
                 "CUTOVER_GH_ACCOUNT=shadow-account" "CUTOVER_GH_HEAD=cccccccccccccccccccccccccccccccccccccccc"
                 "CUTOVER_GH_DIFF=%SIDE%/pull.diff" "CUTOVER_GH_TAG_SHA=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        label="$args"
        case "$args" in
            show) label="show (all receipts)" ;;
            "show 8") label="show 8 (no receipt)" ;;
            verify-deps) label="verify-deps (no arguments)" ;;
            "verify-deps abc 1234567") label="verify-deps abc (not a pull request number)" ;;
            "verify-deps 7 cccccccccccc") label="verify-deps 7 (a pin bump, stub gh)" ;;
        esac
        # shellcheck disable=SC2086  # the words are fixed literals above
        compare "maintainer-merge $label" maintainer-merge "$mm" "$mm" ident full "$HOME" "$T" -- $args
    done
}

# State for the maintainer-merge comparisons, made fresh for each side: one receipt, a key, a repository
# directory and the diff of a pull request that bumps one action pin. verify-deps writes its receipt into
# this scratch state and never into the real one.
prep_merge_state() {  # $1 = the side's directory
    mkdir -p -- "$1/state/receipts" "$1/repo"
    printf '%s\n' '{"pr": 7, "head": "0123456789abcdef0123456789abcdef01234567", "kind": "observed", "proof": "cutover shadow fixture", "signature": "00"}' > "$1/state/receipts/7.json"
    printf '%s' "0000000000000000000000000000000000000000000000000000000000000000" > "$1/state/receipt.key"
    cat > "$1/pull.diff" <<'DIFF'
diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml
index 1111111..2222222 100644
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -10,7 +10,7 @@ jobs:
     runs-on: ubuntu-latest
     steps:
-      - uses: actions/checkout@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  # v4.1.0
+      - uses: actions/checkout@bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  # v4.2.2
DIFF
}

# alert writes a line into the failed profile's alerts.log and sends a notification, so it runs
# against a scratch HOME that holds that profile's state directory: never this machine's.
alert_comparisons() {  # $1 = a profile name
    local T="$SH_TIMEOUT" notify
    for notify in on off; do
        SH_SIDE_PREP=prep_alert_home
        SH_ALERT_PROFILE="$1"
        SH_XENV=("HOME=%SIDE%/home" "MAINTAINER_NOTIFY=$notify")
        SH_EXTRA=("home/.local/state/$1-maint/logs/alerts.log")
        local label="alert $1-review"
        [ "$notify" = on ] || label="alert $1-review (notifications off)"
        compare "$label" alert "$share/alert.sh" "$share/alert.sh" ident full "$SH_SCRATCH" "$T" -- "$1-review"
    done
}
prep_alert_home() {  # $1 = the side's directory
    mkdir -p -- "$1/home/.local/state/$SH_ALERT_PROFILE-maint"
}

# --- dispatch ------------------------------------------------------------------------------

[ "$#" -ge 1 ] || { usage; exit 64; }
sub="$1"; shift
case "$sub" in
    build) cmd_build "$@" ;;
    shadow) cmd_shadow "$@" ;;
    live) cmd_live "$@" ;;
    status) cmd_status "$@" ;;
    rollback) cmd_rollback "$@" ;;
    -h|--help|help) usage; exit 0 ;;
    *) usage_die "unknown subcommand $(shown "$sub")" ;;
esac
