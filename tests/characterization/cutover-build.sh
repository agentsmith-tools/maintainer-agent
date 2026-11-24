#!/usr/bin/env bash
# Characterization: scripts/cutover.sh build. A path-free release build of every shipped Rust
# program into one directory, with a sha256 table. It refuses a dirty tree unless told, never
# writes outside the directory and the cargo target directory, and replaces only what an earlier
# build wrote. cargo is a recorder here: the cases pin what it was asked and what the script
# made of the answer.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib-cutover.sh"
cu_init

fresh() { cu_fake_cargo; cu_default_metadata; }
built_names() { printf '%s\n' $CU_BUILT | LC_ALL=C sort; }
# The table the build must print: computed here from the bytes the fake cargo writes.
# The environment cargo saw on its first `build` call (the `metadata` call before it has none of the exports).
build_env_line() { grep -A1 -m1 '^cargo build' "$CZ_CASE/cargo.calls" | grep -m1 '^env '; }
expected_table() {
    local n
    for n in $(built_names); do printf '%s  %s\n' "$(printf 'rust build of %s\n' "$n" | sha256sum | cut -d' ' -f1)" "$n"; done
}

case_builds_every_program() {
    fresh
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 0
    cz_check "the table in SHA256SUMS" "$(cat "$CU_BUILD/SHA256SUMS" 2>/dev/null)" "$(expected_table)"
    cz_check "the files in the directory" "$(ls -A "$CU_BUILD" | LC_ALL=C sort | tr '\n' ' ')" "$( { built_names; echo BUILD_INFO; echo SHA256SUMS; } | LC_ALL=C sort | tr '\n' ' ')"
    local n
    for n in $CU_BUILT; do
        [ -x "$CU_BUILD/$n" ] || cz_note_fail "$n is missing or not executable"
    done
}
cu_case "build: every binary that stands for a program of the repository is built and installed in the directory" case_builds_every_program

case_prints_table() {
    fresh
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 0
    cz_expect_stdout_has "built 12 programs from $(cu_head | cut -c1-12) in $CU_BUILD"
    local out; out="$(grep -E '^[0-9a-f]{64}  ' "$CZ_CASE/stdout")"
    cz_check "the table printed" "$out" "$(expected_table)"
}
cu_case "build: it prints the sha256 table, one line per program" case_prints_table

case_build_info() {
    fresh
    cu_run build --out "$CU_BUILD"
    cz_check "BUILD_INFO" "$(cat "$CU_BUILD/BUILD_INFO")" "head=$(cu_head)
dirty=0
programs=$(built_names | tr '\n' ' ' | sed 's/ $//')"
}
cu_case "build: BUILD_INFO names the commit, a clean tree and the programs" case_build_info

case_ignores_unmapped_bin() {
    fresh
    cu_run build --out "$CU_BUILD"
    [ ! -e "$CU_BUILD/helper-tool" ] || cz_note_fail "a binary with no program of the repository behind it was built"
    cz_check "cargo asked to build helper-tool" "$(grep -c -- '--bin helper-tool' "$CZ_CASE/cargo.calls")" 0
}
cu_case "build: a binary with no bash program behind it is not built" case_ignores_unmapped_bin

case_gate_binary_joins() {
    fresh
    cu_metadata mx-gate:maintainer-merge mx-run:run mx-install:install
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 0
    [ -x "$CU_BUILD/maintainer-merge" ] || cz_note_fail "the merge gate's binary was not built although bin/maintainer-merge exists"
    cz_expect_stdout_has "built 3 programs"
}
cu_case "build: a binary named for a file of bin/ is built as soon as the workspace has it" case_gate_binary_joins

case_cargo_arguments() {
    fresh
    cu_run build --out "$CU_BUILD"
    local calls="$CZ_CASE/cargo.calls"
    cz_check "the metadata call" "$(grep -m1 '^cargo metadata' "$calls")" "cargo metadata --no-deps --format-version 1 --locked --offline"
    cz_check "the call that builds run" "$(grep '^cargo build' "$calls" | grep -- '--bin run$')" "cargo build --release --locked --offline -p mx-run --bin run"
    cz_check "the number of build calls" "$(grep -c '^cargo build' "$calls")" 12
    cz_check "every build call is release, locked and offline" "$(grep '^cargo build' "$calls" | grep -c -- '--release --locked --offline -p ')" 12
}
cu_case "build: cargo is asked for a locked, offline release build of one binary at a time" case_cargo_arguments

case_cargo_environment() {
    fresh
    cu_run build --out "$CU_BUILD"
    local env_line; env_line="$(build_env_line)"
    local sep=$'\x1f'
    case "$env_line" in
        *"--remap-path-prefix=$CU_HOME=/build/home"*) ;;
        *) cz_note_fail "no remap of HOME in: $env_line" ;;
    esac
    case "$env_line" in
        *"--remap-path-prefix=$CU_HOME/.cargo=/build/cargo"*) ;;
        *) cz_note_fail "no remap of the cargo home in: $env_line" ;;
    esac
    case "$env_line" in
        *"--remap-path-prefix=/fake/toolchain=/build/rustc"*) ;;
        *) cz_note_fail "no remap of the toolchain in: $env_line" ;;
    esac
    case "$env_line" in *"$sep"*) ;; *) cz_note_fail "the flags are not separated by the unit separator" ;; esac
    case "$env_line" in *"jobs=2 "*) ;; *) cz_note_fail "CARGO_BUILD_JOBS is not 2 by default: $env_line" ;; esac
    case "$env_line" in *"color=never"*) ;; *) cz_note_fail "colour is not off: $env_line" ;; esac
    case "$env_line" in *"cwd=$CU_FX/rust"*) ;; *) cz_note_fail "cargo did not run in rust/: $env_line" ;; esac
}
cu_case "build: the path-free flags remap HOME, the cargo home and the toolchain, and jobs default to 2" case_cargo_environment

case_cargo_target_dir_default() {
    fresh
    cu_run build --out "$CU_BUILD"
    case "$(build_env_line)" in
        *"target=$CU_FX/rust/target "*) ;;
        *) cz_note_fail "the cargo target directory is not rust/target by default" ;;
    esac
    [ -f "$CU_FX/rust/target/release/run" ] || cz_note_fail "the build did not land in the default target directory"
}
cu_case "build: without CARGO_TARGET_DIR the target directory is rust/target of the checkout" case_cargo_target_dir_default

case_cargo_target_dir_env() {
    fresh
    cz_set CARGO_TARGET_DIR "$CZ_CASE/target-elsewhere"
    cz_set CARGO_BUILD_JOBS 3
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 0
    case "$(build_env_line)" in
        *"jobs=3 target=$CZ_CASE/target-elsewhere "*) ;;
        *) cz_note_fail "CARGO_TARGET_DIR or CARGO_BUILD_JOBS was not kept: $(build_env_line)" ;;
    esac
    [ ! -e "$CU_FX/rust/target" ] || cz_note_fail "rust/target was written although CARGO_TARGET_DIR named another place"
}
cu_case "build: CARGO_TARGET_DIR and CARGO_BUILD_JOBS from the caller are kept" case_cargo_target_dir_env

case_keeps_callers_rustflags() {
    fresh
    cz_set RUSTFLAGS "-C opt-level=3"
    cu_run build --out "$CU_BUILD"
    case "$(build_env_line)" in
        *"flags=-C"*"opt-level=3"*"--remap-path-prefix="*) ;;
        *) cz_note_fail "the caller's RUSTFLAGS did not stay in front: $(build_env_line)" ;;
    esac
}
cu_case "build: the caller's RUSTFLAGS stay in front of the path-free flags" case_keeps_callers_rustflags

case_dirty_untracked() {
    fresh
    printf 'x\n' > "$CU_FX/stray.txt"
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stdout_empty
    cz_expect_stderr_first "cutover.sh: the working tree is dirty; commit or stash, or pass --allow-dirty (the build is then marked dirty and live refuses it)"
    [ ! -e "$CU_BUILD" ] || cz_note_fail "the directory was created for a refused build"
    [ ! -s "$CZ_CASE/cargo.calls" ] || cz_note_fail "cargo was called for a refused build"
}
cu_case "build: an untracked file makes the tree dirty and the build is refused before cargo runs" case_dirty_untracked

case_dirty_modified() {
    fresh
    printf '# edited\n' >> "$CU_FX/lib/run.sh"
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_has "the working tree is dirty"
}
cu_case "build: a modified tracked file makes the tree dirty" case_dirty_modified

case_dirty_staged_deleted() {
    fresh
    git -C "$CU_FX" rm -q bin/maintainer
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_has "the working tree is dirty"
}
cu_case "build: a staged deletion makes the tree dirty" case_dirty_staged_deleted

case_allow_dirty() {
    fresh
    printf 'x\n' > "$CU_FX/stray.txt"
    cu_run build --out "$CU_BUILD" --allow-dirty
    cz_expect_rc 0
    cz_check "BUILD_INFO dirty line" "$(sed -n 2p "$CU_BUILD/BUILD_INFO")" "dirty=1"
}
cu_case "build: --allow-dirty builds and marks the build dirty" case_allow_dirty

case_ignored_target_is_clean() {
    fresh
    mkdir -p "$CU_FX/rust/target/release"; printf 'x\n' > "$CU_FX/rust/target/release/junk"
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 0
    cz_check "BUILD_INFO dirty line" "$(sed -n 2p "$CU_BUILD/BUILD_INFO")" "dirty=0"
}
cu_case "build: files under the ignored rust/target do not make the tree dirty" case_ignored_target_is_clean

case_writes_only_dir_and_target() {
    fresh
    cz_set CARGO_TARGET_DIR "$CZ_CASE/target-elsewhere"
    cu_snap "$CU_HOME" > "$CZ_CASE/home.before"; cu_snap "$CU_FX" > "$CZ_CASE/fx.before"
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 0
    cu_snap "$CU_HOME" > "$CZ_CASE/home.after"; cu_snap "$CU_FX" > "$CZ_CASE/fx.after"
    cu_expect_same_snap "HOME" "$CZ_CASE/home.before" "$CZ_CASE/home.after"
    cu_expect_same_snap "the checkout" "$CZ_CASE/fx.before" "$CZ_CASE/fx.after"
    cz_check "entries left in the directory besides the programs" "$(cu_dot_entries "$CU_BUILD")" 0
}
cu_case "build: it writes only the build directory and the target directory" case_writes_only_dir_and_target

case_foreign_file() {
    fresh
    mkdir -p "$CU_BUILD"; chmod 755 "$CU_BUILD"; printf 'mine\n' > "$CU_BUILD/notes.txt"
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: $CU_BUILD holds notes.txt, which an earlier build did not write; use an empty directory"
    cz_check "the foreign file" "$(cat "$CU_BUILD/notes.txt")" mine
    [ ! -s "$CZ_CASE/cargo.calls" ] || cz_note_fail "cargo was called"
}
cu_case "build: a directory that holds a file no build wrote is refused and the file is kept" case_foreign_file

case_foreign_directory() {
    fresh
    mkdir -p "$CU_BUILD/sub"; chmod 755 "$CU_BUILD"
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: $CU_BUILD holds sub, which an earlier build did not write; use an empty directory"
}
cu_case "build: a sub-directory in the build directory is refused" case_foreign_directory

case_rebuild_replaces() {
    fresh
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 0
    # the workspace drops a program and the next build must not leave the old one behind
    cu_metadata mx-install:install mx-run:run
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 0
    cz_check "the files after a smaller build" "$(ls -A "$CU_BUILD" | LC_ALL=C sort | tr '\n' ' ')" "BUILD_INFO SHA256SUMS install run "
    cz_check "SHA256SUMS lists two programs" "$(grep -c . "$CU_BUILD/SHA256SUMS")" 2
}
cu_case "build: a rebuild replaces the old programs and leaves none behind" case_rebuild_replaces

case_killed_build_leaves_stage() {
    fresh
    cu_run build --out "$CU_BUILD"
    mkdir "$CU_BUILD/.stage.abc123"; : > "$CU_BUILD/.stage.abc123/half-copied"
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 0
}
cu_case "build: the scratch directory of an earlier, killed build is not a foreign file" case_killed_build_leaves_stage

case_sigterm_removes_stage() {
    fresh
    cat > "$CZ_CASE/bin/cargo.slow" <<'SLOW'
SLOW
    # a cargo that waits on the first build call, so the script can be signalled with its stage in place
    sed -i 's|    build)|    build)\n        : > "${CZ_CASE_DIR:?}/cargo.building"; for _ in $(seq 1 100); do [ -e "${CZ_CASE_DIR:?}/cargo.release" ] \&\& break; sleep 0.1; done|' "$CZ_CASE/bin/cargo"
    local pairs=() k
    for k in "${!CZ_E[@]}"; do pairs+=("$k=${CZ_E[$k]}"); done
    cd "$CU_HOME" || return
    env -i "${pairs[@]}" bash "$(cu_script)" build --out "$CU_BUILD" > "$CZ_CASE/stdout" 2> "$CZ_CASE/stderr" < /dev/null &
    local pid=$!
    for _ in $(seq 1 100); do [ -e "$CZ_CASE/cargo.building" ] && break; sleep 0.1; done
    case "$(tr '\0' ' ' < "/proc/$pid/cmdline")" in
        *cutover.sh*build*) ;;
        *) cz_note_fail "the pid I am about to signal is not the build" ;;
    esac
    kill -TERM "$pid"
    : > "$CZ_CASE/cargo.release"
    wait "$pid"; CZ_RC=$?
    cz_expect_rc 143
    cz_check "scratch directories left in the build directory" "$(cu_dot_entries "$CU_BUILD")" 0
}
cu_case "build: SIGTERM ends the build with 143 and removes its scratch directory" case_sigterm_removes_stage

case_shadow_scratch_is_not_foreign() {
    fresh
    cu_run build --out "$CU_BUILD"
    mkdir "$CU_BUILD/.shadow.abc123"; : > "$CU_BUILD/.shadow-ok.abc123"
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 0
}
cu_case "build: the scratch of a killed shadow is not a foreign file" case_shadow_scratch_is_not_foreign

case_rebuild_removes_old_marker() {
    fresh
    cu_run build --out "$CU_BUILD"
    printf 'old\n' > "$CU_BUILD/shadow-ok"; printf 'old\n' > "$CU_BUILD/shadow-report.txt"
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 0
    [ ! -e "$CU_BUILD/shadow-ok" ] || cz_note_fail "a shadow-ok of the old build survived the new build"
}
cu_case "build: a new build removes the shadow-ok of the old one" case_rebuild_removes_old_marker

case_build_failure_keeps_old() {
    fresh
    cu_run build --out "$CU_BUILD"
    cu_snap "$CU_BUILD" > "$CZ_CASE/build.before"
    : > "$CZ_CASE/cargo.fail.run"
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_has "cutover.sh: cargo build of run failed: error: could not compile run"
    cu_snap "$CU_BUILD" > "$CZ_CASE/build.after"
    cu_expect_same_snap "the build directory after a failed build" "$CZ_CASE/build.before" "$CZ_CASE/build.after"
}
cu_case "build: a failed compile exits 1, names the program and leaves the old build whole" case_build_failure_keeps_old

case_empty_binary() {
    fresh
    : > "$CZ_CASE/cargo.empty.run"
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_has "the build of run is empty"
}
cu_case "build: an empty binary is refused" case_empty_binary

case_not_executable() {
    fresh
    : > "$CZ_CASE/cargo.noexec.run"
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_has "the build left no $CU_FX/rust/target/release/run"
}
cu_case "build: a binary that is not executable is refused" case_not_executable

case_metadata_fails() {
    fresh
    printf '1' > "$CZ_CASE/cargo.metadata.rc"
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_has "cutover.sh: cargo metadata failed: error: failed to read the manifest"
}
cu_case "build: a workspace cargo cannot read is refused" case_metadata_fails

case_no_programs() {
    fresh
    cu_metadata mx-probe:helper-tool
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: the workspace has no binary that stands for a program of this repository"
}
cu_case "build: a workspace with no program of the repository is refused" case_no_programs

case_hostile_binary_name() {
    fresh
    cu_metadata mx-run:run mx-install:install
    # a binary whose file exists in bin/ under a name with a space
    printf '#!/usr/bin/env bash\n' > "$CU_FX/bin/evil name"; git -C "$CU_FX" add -A >/dev/null; git -C "$CU_FX" -c user.name=f -c user.email=f@e.invalid commit -q -m x
    cu_metadata "mx-evil:evil name" mx-run:run
    cu_run build --out "$CU_BUILD"
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: the workspace names a binary evil\\ name, which is not a plain name"
    [ ! -s "$CZ_CASE/cargo.calls" ] || [ "$(grep -c '^cargo build' "$CZ_CASE/cargo.calls")" = 0 ] || cz_note_fail "cargo built something before the name was refused"
}
cu_case "build: a binary name with a space is refused before anything is built" case_hostile_binary_name

case_default_out() {
    fresh
    cu_run build
    cz_expect_rc 0
    [ -f "$CU_HOME/.cache/magent-cutover/build/SHA256SUMS" ] || cz_note_fail "no build in ~/.cache/magent-cutover/build"
}
cu_case "build: with no --out the directory is ~/.cache/magent-cutover/build" case_default_out

case_relative_out() {
    fresh
    mkdir -p "$CZ_CASE/work"
    ( cd "$CZ_CASE/work" && cu_run_in_cwd build --out rel-out )
    [ -f "$CZ_CASE/work/rel-out/SHA256SUMS" ] || cz_note_fail "a relative --out was not made absolute against the working directory"
}
cu_run_in_cwd() {
    local pairs=() k
    for k in "${!CZ_E[@]}"; do pairs+=("$k=${CZ_E[$k]}"); done
    env -i "${pairs[@]}" bash "$(cu_script)" "$@" > "$CZ_CASE/stdout" 2> "$CZ_CASE/stderr" < /dev/null
    # shellcheck disable=SC2034  # read by the assertions of lib.sh
    CZ_RC=$?
}
cu_case "build: a relative --out is taken from the working directory" case_relative_out

case_out_mode() {
    fresh
    cu_run build --out "$CZ_CASE/new-out"
    cz_check "mode of the new directory" "$(stat -c %a "$CZ_CASE/new-out")" 755
}
cu_case "build: a new directory is made with mode 755" case_out_mode

case_out_symlink() {
    fresh
    mkdir "$CZ_CASE/real-out"; chmod 755 "$CZ_CASE/real-out"; ln -s "$CZ_CASE/real-out" "$CZ_CASE/link-out"
    cu_run build --out "$CZ_CASE/link-out"
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: $CZ_CASE/link-out is a symbolic link; the build directory must be a real directory"
    [ -z "$(ls -A "$CZ_CASE/real-out")" ] || cz_note_fail "the link was followed and the target written"
}
cu_case "build: an --out that is a symbolic link is refused and its target stays empty" case_out_symlink

case_out_world_writable() {
    fresh
    mkdir "$CZ_CASE/open-out"; chmod 777 "$CZ_CASE/open-out"
    cu_run build --out "$CZ_CASE/open-out"
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: $CZ_CASE/open-out is writable by everyone (mode 777); refused"
    [ ! -s "$CZ_CASE/cargo.calls" ] || cz_note_fail "cargo was called"
}
cu_case "build: a world-writable --out is refused before cargo runs" case_out_world_writable

case_out_is_a_file() {
    fresh
    printf 'x\n' > "$CZ_CASE/afile"
    cu_run build --out "$CZ_CASE/afile"
    cz_expect_rc 1
    cz_expect_stderr_first "cutover.sh: $CZ_CASE/afile is not a directory"
}
cu_case "build: an --out that is a file is refused" case_out_is_a_file

# --- the two tables that must not drift apart --------------------------------------------------------------------

case_table_matches_assemble_tree() {
    local theirs ours
    theirs="$(sed -n '/^OUTSIDE_BIN = {/,/^}/p' "$MX_TREE/rust/scripts/assemble-tree.sh" | sed 's/^ *//')"
    ours="$(sed -n '/^OUTSIDE_BIN = {/,/^}/p' "$CU_SRC" | sed 's/^ *//')"
    [ -n "$theirs" ] || cz_note_fail "assemble-tree.sh has no OUTSIDE_BIN table"
    [ -n "$ours" ] || cz_note_fail "cutover.sh has no OUTSIDE_BIN table"
    cz_check "the programs that stand for a script outside bin/" "$ours" "$theirs"
}
cu_case "build: the table of programs equals the one in rust/scripts/assemble-tree.sh" case_table_matches_assemble_tree

case_flags_match_assemble_tree() {
    local theirs ours
    theirs="$(sed -n '/^path_free_flags() {/,/^}/p' "$MX_TREE/rust/scripts/assemble-tree.sh")"
    ours="$(sed -n '/^path_free_flags() {/,/^}/p' "$CU_SRC" | sed 's|"\$root/rust"|"$ws"|')"
    [ -n "$theirs" ] || cz_note_fail "assemble-tree.sh has no path_free_flags"
    cz_check "the path-free flags function" "$ours" "$theirs"
}
cu_case "build: the path-free flags function is the one of rust/scripts/assemble-tree.sh" case_flags_match_assemble_tree

case_cargo_flags_identical() {
    local theirs ours
    theirs="$(grep -o 'cargo build --release --locked --offline -p' "$MX_TREE/rust/scripts/assemble-tree.sh" | head -1)"
    ours="$(grep -o 'cargo build --release --locked --offline -p' "$CU_SRC" | head -1)"
    cz_check "the cargo build line" "$ours" "$theirs"
}
cu_case "build: the cargo build line is the one assemble-tree.sh uses" case_cargo_flags_identical

cz_finish
