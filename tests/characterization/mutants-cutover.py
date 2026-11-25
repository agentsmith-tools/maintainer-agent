#!/usr/bin/env python3
"""Replay the mutation proof for the cutover cases.

  python3 tests/characterization/mutants-cutover.py                # every mutant
  python3 tests/characterization/mutants-cutover.py shadow-marker-no-diff-gate ...
  python3 tests/characterization/mutants-cutover.py --list

Each mutant changes scripts/cutover.sh in a fresh, touched copy of the files the cases read (the
checkout itself is never edited). The old text must occur exactly once, the new file is read back
and parsed with `bash -n`, and the mutant is run against the one case file, and the one case (by
a word of its name), written next to it: a mutant is KILLED when that run reports failed cases and
a FAIL line. Nothing else counts. A run that crashes, prints no summary, hits the time cap or the
address-space cap is CAPPED or BROKEN and is reported as such, never as a kill. The pristine copy
runs first and last, on the whole of its case files, and its tree hash is compared (a copy is
restored from the pristine copy with a file copy and checked by hash, never with git).

Each run is capped: 300 s of wall clock and 8 GB of address space (`ulimit -v`). A mutant that loops
forever or eats memory ends as CAPPED.

The cases that run a mutant never reach a real gh, git remote, podman or notify-send: the case
harness puts a recorder first on PATH and fails a case that reached it.
"""
import hashlib, os, re, shutil, subprocess, sys, tempfile, time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
ITEMS = ["scripts/cutover.sh", "tests/characterization", "tests/run-characterization.sh", "rust/scripts/assemble-tree.sh",
         "install.sh", "rust/Cargo.toml", "bin/maintainer-merge"]
CUT = "scripts/cutover.sh"
U, R, S, B, L, SH = ("cutover-usage.sh", "cutover-rollback.sh", "cutover-status.sh", "cutover-build.sh",
                     "cutover-live.sh", "cutover-shadow.sh")
CAP_SECONDS, CAP_BYTES = 300, 8_000_000_000
CONTROL_CAP_SECONDS = 1800   # a control runs every case of a file, and the shadow file takes minutes

# id, text that occurs once in scripts/cutover.sh, its replacement, case file, a word of the case that must fail
MUTANTS = [
    # usage and the shared refusals
    ("usage-exit-code", 'usage; exit 64; }\n\n# The directory the EXIT trap', 'usage; exit 2; }\n\n# The directory the EXIT trap', U, "an unknown subcommand exits 64"),
    ("refusal-exit-code", "die() { printf '%s: %s\\n' \"$PROG\" \"$*\" >&2; exit 1; }", "die() { printf '%s: %s\\n' \"$PROG\" \"$*\" >&2; exit 2; }", U, "HOME unset"),
    ("need-tool-ignored", "command -v \"$t\" >/dev/null 2>&1 || die \"the tool '$t' is not on PATH; nothing was done\"", "command -v \"$t\" >/dev/null 2>&1 || true", U, "status without sha256sum"),
    ("newline-allowed", "nl_free() { case \"$1\" in *$'\\n'*) return 1 ;; esac; return 0; }", "nl_free() { return 0; }", U, "a HOME with a newline"),
    ("pipe-allowed", "        *\\|*) die \"$1 holds a '|'; refused\" ;;", "", U, "pipe"),
    ("control-character-allowed", '[[ "$2" =~ [[:cntrl:]] ]] && die "$1 holds a control character; refused"', 'true', U, "unit separator"),
    ("colon-allowed", "*:*) die \"$1 holds a colon, which would split a PATH entry; refused\" ;;", "*:*) ;;", U, "a build directory with a colon"),
    ("symlink-dir-followed", "[ ! -L \"$d\" ] || die \"$d is a symbolic link; the build directory must be a real directory\"", "true", U, "symbolic link"),
    ("world-writable-allowed", "[ $(( ${perm: -1} & 2 )) -eq 0 ] || die", "[ $(( ${perm: -1} & 4 )) -eq 0 ] || die", U, "writable"),
    ("not-a-checkout-allowed", '[ -f "$root/install.sh" ] && [ -d "$root/rust" ] \\\n    ||', '[ -f "$root/install.sh" ] \\\n    ||', U, "checkout"),
    # rollback
    ("rollback-busy-check-off", 'if [ "$dry" = 0 ] && [ "$force" = 0 ]; then\n        local busy', 'if false; then\n        local busy', R, "run lock is held refuses"),
    ("rollback-busy-exit-code", "            exit 69", "            exit 70", R, "run lock is held refuses"),
    ("rollback-force-ignored", 'if [ "$dry" = 0 ] && [ "$force" = 0 ]; then\n        local busy', 'if [ "$dry" = 0 ]; then\n        local busy', R, "--force goes on"),
    ("rollback-dry-run-asks-lock", 'if [ "$dry" = 0 ] && [ "$force" = 0 ]; then\n        local busy', 'if [ "$force" = 0 ]; then\n        local busy', R, "--dry-run does not ask"),
    ("rollback-flock-optional", 'command -v flock >/dev/null 2>&1 \\\n        || die "flock is required', 'command -v flock >/dev/null 2>&1 \\\n        || true "flock is required', R, "with no flock"),
    ("rollback-no-stamp-passes", 'die "nothing to roll back: $stamp_file does not exist"', 'exit 0', R, "with no RUST_STAMP"),
    ("rollback-stamp-link-followed", '[ ! -L "$stamp_file" ] || die "cannot roll back: $stamp_file is a symbolic link; rollback never follows or replaces one"', 'true', R, "RUST_STAMP that is a link"),
    ("rollback-installed-link-followed", 'if [ -L "$installed" ]; then die "cannot roll back: $installed is a symbolic link; rollback never follows or replaces one"; fi', 'true', R, "planted where an installed program sits"),
    ("rollback-backup-link-followed", 'if [ -L "$backup" ]; then die "cannot roll back: $backup is a symbolic link; rollback never follows or replaces one"; fi', 'true', R, "where a saved original sits is refused, not moved"),
    ("rollback-hash-compare-inverted", 'if [ -n "$ihash" ] && [ "$ihash" = "${ST_HASH[$i]}" ]; then', 'if [ -n "$ihash" ] && [ "$ihash" != "${ST_HASH[$i]}" ]; then', R, "undone file for file"),
    ("rollback-missing-backup-ignored", '[ -n "$bhash" ] || die "cannot roll back: $backup is missing"', 'true', R, "missing saved original"),
    ("rollback-changed-binary-ignored", 'die "cannot roll back: $installed no longer matches the stamp, so it is not the build this installer wrote"', 'true', R, "no longer the stamped build"),
    ("rollback-restored-already-refused", 'elif [ -z "$bhash" ]; then', 'elif false; then', R, "interrupted rollback is skipped"),
    ("rollback-never-started-kept", 'elif [ "$bhash" = "$ihash" ]; then', 'elif false; then', R, "swap never began"),
    ("rollback-stamp-kept", 'else rm -f -- "$stamp_file" || exit "$?"; fi', 'else true; fi', R, "undone file for file"),
    ("rollback-copies-instead-of-moves", 'else mv -f -- "${ssrc[$j]}" "${sdst[$j]}" || exit "$?"; fi', 'else cp -- "${ssrc[$j]}" "${sdst[$j]}" || exit "$?"; fi', R, "undone file for file"),
    ("rollback-count-zero", 'n="$restored"', 'n=0', R, "undone file for file"),
    ("stamp-hash-not-lowercased", "hash=\"$(printf '%s' \"$hash\" | tr 'A-F' 'a-f')\"", 'hash="$hash"', R, "upper-case hex"),
    ("marker-last-line-dropped", 'while IFS= read -r line || [ -n "$line" ]; do\n        case "$line" in\n            "# checkout: "*)', 'while IFS= read -r line; do\n        case "$line" in\n            "# checkout: "*)', S, "no final newline"),
    ("quote-allowed", "        *\\'*) die \"$1 holds a single quote; refused\" ;;", "", U, "single quote"),
    ("marker-report-link-followed", 'if [ ! -f "$rep" ] || [ -L "$rep" ]; then MARK_ERR=', 'if [ ! -f "$rep" ]; then MARK_ERR=', L, "report that is a symbolic link"),
    ("marker-report-checkout-header-unchecked", '|| [ -z "$hdr_chk" ] || [ "${head:0:${#hdr_chk}}" != "$hdr_chk" ]; then', '|| false; then', L, "header names another checkout"),
    ("shadow-unknown-deployed-allowed", 'if [ -z "$deployed_commit" ] || [ -z "$head_full" ] ||', 'if [ -z "$head_full" ] ||', SH, "empty commit"),
    ("shadow-git-dir-two-words-safe", '--git-dir|--work-tree) skip=1; unsafe=1 ;;', '--git-dir|--work-tree) skip=1 ;;', SH, "git changes nothing outside TMPDIR"),
    ("build-shadow-scratch-foreign", 'case "$name" in .stage.*|.shadow.*|.shadow-ok.*) continue ;; esac\n            has_name', 'case "$name" in .stage.*) continue ;; esac\n            has_name', B, "scratch of a killed shadow"),
    ("build-shadow-scratch-removal-refused", 'case "$old" in .stage.*|.shadow.*|.shadow-ok.*) continue ;; esac', 'case "$old" in .stage.*) continue ;; esac', B, "scratch of a killed shadow"),
    ("marker-checkout-unchecked", 'if [ "$rhead" != "$head" ]; then MARK_ERR=', 'if false; then MARK_ERR=', L, "another commit of the checkout"),
    ("marker-report-missing-allowed", 'if [ ! -f "$rep" ] || [ -L "$rep" ]; then MARK_ERR=', 'if false; then MARK_ERR=', L, "no report beside it"),
    ("marker-report-digest-unchecked", 'if [ "$(sha256_of "$rep")" != "$rsum" ]; then', 'if false; then', L, "report that changed after"),
    ("marker-report-older-allowed", 'if [ "$dir/SHA256SUMS" -nt "$rep" ]; then', 'if false; then', L, "older than the build"),
    ("marker-unaccepted-skip-allowed", 'if ! has_name "$skips" "$prog"; then MARK_ERR=', 'if false; then MARK_ERR=', L, "does not accept"),
    ("marker-diff-allowed", 'if [ "$ndiff" != 0 ]; then MARK_ERR=', 'if false; then MARK_ERR=', L, "DIFF line"),
    ("marker-zero-comparisons-allowed", '|| [ "$total" -le 0 ] || [ "$npass" -le 0 ]; then MARK_ERR=', '|| false; then MARK_ERR=', L, "no comparison in it"),
    ("marker-summary-total-unchecked", '|| [ "$total" != "$((npass + nskip))" ]; then MARK_ERR=', '|| false; then MARK_ERR=', L, "summary total"),
    ("marker-summary-pass-unchecked", 'if [ "$spass" != "$npass" ] ||', 'if false ||', L, "summary pass count"),
    ("marker-deployed-unchecked", 'if [ -z "$hdr_dep" ] || [ "${head:0:${#hdr_dep}}" != "$hdr_dep" ] ||', 'if false ||', L, "deployed tree that is not the checkout"),
    ("shadow-deployed-unchecked", '|| [ "${head_full:0:${#deployed_commit}}" != "$deployed_commit" ]; then', '|| false; then', SH, "deployed tree from another commit"),
    ("shadow-dirty-checkout-allowed", '[ -z "$(git -C "$root" status --porcelain 2>/dev/null)" ] || why_not="the checkout is dirty', 'true || why_not="the checkout is dirty', SH, "dirty checkout"),
    ("shadow-marker-no-report-digest", "printf '# checkout: %s\\n# report: %s\\n' \"$head_full\" \"$(sha256_of \"$SH_REPORT\")\"", "printf '# checkout: %s\\n# report: %s\\n' \"$head_full\" \"0\"", SH, "binds the checkout commit"),
    ("rollback-unreadable-installed-ignored", 'if [ -e "$installed" ] && [ -z "$ihash" ]; then die', 'if false; then die', R, "cannot be read refuse"),
    ("rollback-unreadable-backup-ignored", 'if [ -e "$backup" ] && [ -z "$bhash" ]; then die', 'if false; then die', R, "saved original that cannot be read"),
    ("rollback-dry-run-removes", 'if [ "$dry" = 1 ]; then printf \'  would: rm -f %s\\n\' "${ssrc[$j]}"', 'if false; then printf \'  would: rm -f %s\\n\' "${ssrc[$j]}"', R, "does not remove it"),
    ("rollback-stamp-removal-failure-ignored", 'else rm -f -- "$stamp_file" || exit "$?"; fi', 'else rm -f -- "$stamp_file" || true; fi', R, "cannot be removed"),
    ("rollback-needs-no-sha256sum", 'need sha256sum mv rm cat wc tr bash', 'need mv rm cat wc tr bash', R, "without sha256sum"),
    ("stamp-hash-length", '^[0-9A-Fa-f]{64}\\ \\ .', '^[0-9A-Fa-f]{63}\\ \\ .', R, "undone file for file"),
    ("stamp-duplicate-allowed", '[ "$i" != "$path" ] || { ST_ERR="RUST_STAMP lists $path twice"; return 1; }', 'true', R, "program listed twice"),
    ("stamp-unknown-program-allowed", 'n="$(name_of_installed "$path")" || { ST_ERR="RUST_STAMP lists $path, which is not a program this installer swaps"; return 1; }', 'n=""', R, "does not swap is refused"),
    ("stamp-nul-allowed", 'if [ "$size" != "$bare" ]; then ST_ERR="RUST_STAMP holds a NUL byte"; return 1; fi', 'true', R, "NUL byte"),
    ("stamp-empty-allowed", '[ "${#ST_PATH[@]}" -gt 0 ] || { ST_ERR="RUST_STAMP lists no program"; return 1; }', 'true', R, "an empty stamp"),
    # status
    ("status-marker-always-current", 'if ( verify_marker "$dir" ) >/dev/null 2>&1; then', 'if true; then', S, "stale when a program changed"),
    ("status-hash-not-compared", 'if [ "$(sha256_of "$path")" = "${ST_HASH[$k]}" ]; then state="rust"', 'if true; then state="rust"', S, "bytes changed is reported as changed"),
    ("status-link-followed", 'if [ -L "$path" ]; then state="link"', 'if false; then state="link"', S, "link where a program sits"),
    ("status-stamp-link-followed", '[ ! -L "$stamp_file" ] || die "$stamp_file is a symbolic link; refused"', 'true', S, "RUST_STAMP that is a link"),
    # build
    ("build-dirty-allowed", '[ "$allow_dirty" = 1 ] || die "the working tree is dirty;', 'true || die "the working tree is dirty;', B, "an untracked file makes the tree dirty"),
    ("build-dirty-flag-lost", "        dirty=1\n    fi", "        dirty=0\n    fi", B, "--allow-dirty builds"),
    ("build-foreign-file-replaced", 'has_name "$known" "$name" || die "$out holds $name, which an earlier build did not write; use an empty directory"', 'true', B, "no build wrote is refused"),
    ("build-empty-binary-allowed", '[ -s "$target_dir/release/${names[$i]}" ] || die "the build of ${names[$i]} is empty"', 'true', B, "an empty binary"),
    ("build-locked-dropped", "cargo build --release --locked --offline -p", "cargo build --release --offline -p", B, "locked, offline release build"),
    ("build-home-not-remapped", 'local from=("${HOME:-}" "$cargo_home" "$sysroot")', 'local from=("" "$cargo_home" "$sysroot")', B, "remap HOME"),
    ("build-old-marker-kept", 'rm -f -- "$f" || die "cannot remove $f"', 'case "$old" in shadow-ok) ;; *) rm -f -- "$f" || die "cannot remove $f" ;; esac', B, "removes the shadow-ok of the old one"),
    ("build-name-unchecked", '[[ "$name" =~ ^[a-z][a-z0-9-]*$ ]] || die "the workspace names a binary', 'true || die "the workspace names a binary', B, "a space is refused"),
    ("build-table-sort", 'sorted="$(printf \'%s\\n\' "${list[@]}" | LC_ALL=C sort)"', 'sorted="$(printf \'%s\\n\' "${list[@]}" | LC_ALL=C sort -r)"', B, "sha256 table"),
    ("build-dir-world-writable", 'chmod 755 -- "$out" || die "cannot set the mode of $out"', 'chmod 777 -- "$out" || die "cannot set the mode of $out"', B, "mode 755"),
    # live
    ("live-no-marker-allowed", '[ -f "$dir/shadow-ok" ] && [ ! -L "$dir/shadow-ok" ] \\\n        || die', 'true \\\n        || die', L, "without shadow-ok"),
    ("live-table-compare-off", 'if [ "$marker"$\'\\n\' != "$current"$\'\\n\' ]; then MARK_KIND=table', 'if false; then MARK_KIND=table', L, "changed after shadow"),
    ("live-dirty-build-allowed", '[ "$info_dirty" = 0 ] || die', 'true || die', L, "dirty tree refuses"),
    ("live-head-not-compared", '[ "$head" = "$info_head" ] || die', 'true || die', L, "another commit than the checkout"),
    ("live-dirty-checkout-allowed", '[ -z "$status" ] || die "the checkout is dirty; install would deploy', 'true || die "the checkout is dirty; install would deploy', L, "a dirty checkout"),
    ("live-existing-stamp-allowed", 'if [ -e "$stamp_file" ] || [ -L "$stamp_file" ]; then\n        die "$stamp_file exists', 'if false; then\n        die "$stamp_file exists', L, "already there"),
    ("live-no-dry-run", 'as_program "$root/install.sh" "$dir/install" --rust --rust-bin "$dir" --dry-run || rc=$?', 'rc=0', L, "--dry-run first"),
    ("live-dry-run-failure-ignored", '[ "$rc" = 0 ] || die "the dry run of the install exited $rc; nothing was installed"', 'true', L, "dry run that fails stops"),
    ("live-install-failure-ignored", 'if [ "$rc" != 0 ]; then\n        printf \'%s: the install exited %s;', 'if false; then\n        printf \'%s: the install exited %s;', L, "exits non-zero is reported"),
    ("live-readback-program-hash", '[ "$got" = "$want" ] || problems+=("$path is not the Rust build', 'true || problems+=("$path is not the Rust build', L, "not the Rust build fails"),
    ("live-readback-backup-hash", '[ "$got" = "$want" ] || problems+=("$path.sh is not the bash original', 'true || problems+=("$path.sh is not the bash original', L, "not the checkout's bash program"),
    ("live-readback-stamp-count", '|| problems+=("RUST_STAMP lists ${#ST_PATH[@]} program(s) and ${#expected[@]} were expected")', '|| true', L, "fewer programs than were swapped"),
    ("live-readback-stamp-hash", '[ "${ST_HASH[$k]}" = "$want" ] || problems+=', 'true || problems+=', L, "stamp hash that is not the build"),
    ("live-no-install-allowed", '[ "$have_install" = 1 ] || die', 'true || die', L, "no install program"),
    # shadow
    ("shadow-old-marker-kept", 'rm -f -- "$dir/shadow-ok" "$dir/shadow-report.txt"', 'rm -f -- "$dir/shadow-report.txt"', SH, "marker and the report of an earlier run"),
    ("shadow-diff-gate-off", 'if [ "$SH_DIFF" != 0 ]; then why_not="${why_not:+$why_not; }$SH_DIFF comparison(s) differ"; fi', 'true', SH, "differs in one byte of one prompt"),
    ("shadow-zero-prompts-gate-off", '[ "$SH_PROMPTS" -gt 0 ] || why_not=', 'true || why_not=', SH, "no Rust run there is no prompt"),
    ("shadow-prompt-count-gate-off", '[ "$SH_PROMPTS" = "$SH_PROMPT_EXPECTED" ] || why_not=', 'true || why_not=', SH, "no Rust run there is no prompt"),
    ("shadow-refused-gate-off", '[ "$SH_REFUSED" = 0 ] || why_not=', 'true || why_not=', SH, "when the bash program writes too"),
    ("shadow-skip-gate-off", '[ "$left" = 0 ] || why_not=', 'true || why_not=', SH, "withholds the marker"),
    ("shadow-unchanged-gate-off", '[ "$unchanged" = yes ] || why_not=', 'true || why_not=', SH, "writes into the deployed tree"),
    ("shadow-accept-skip-any", 'for a in "${accepted[@]}"; do [ "$a" = "$n" ] && ok=1; done', 'ok=1', SH, "does not excuse another"),
    ("shadow-build-table-unchecked", '[ "$table"$\'\\n\' = "$recorded"$\'\\n\' ] || die "the programs in $dir are not the ones build recorded', 'true || die "the programs in $dir are not the ones build recorded', SH, "changed since build"),
    ("shadow-no-profiles-allowed", '[ "${#profiles[@]}" -gt 0 ] || die "no profile is deployed under', 'true || die "no profile is deployed under', SH, "an empty profiles directory"),
    ("shadow-profile-name-unchecked", 'plain_profile "$p" || die', 'true || die', SH, "a profile name with a space"),
    ("shadow-task-name-unchecked", 'plain_task "$t" || die', 'true || die', SH, "a task name with"),
    ("shadow-blank-tasks-allowed", '[ "${#words[@]}" -gt 0 ] || die "the profile $p declares no TASKS', 'true || die "the profile $p declares no TASKS', SH, "only blanks"),
    ("shadow-exit-code-not-compared", 'elif [ "$rc_b" != "$rc_r" ]; then', 'elif false; then', SH, "another exit code"),
    ("shadow-stdout-not-compared", 'elif ! cmp -s "$base/bash/stdout.n" "$base/rust/stdout.n"; then', 'elif false; then', SH, "one byte of one prompt"),
    ("shadow-stderr-not-compared", 'elif ! cmp -s "$base/bash/stderr.n" "$base/rust/stderr.n"; then', 'elif false; then', SH, "extra line on standard error"),
    ("shadow-calls-not-compared", 'elif ! cmp -s "$base/bash/calls.n" "$base/rust/calls.n"; then', 'elif false; then', SH, "a gh write is refused"),
    ("shadow-timeout-ignored", '124|137) why="the bash side timed out', '999) why="the bash side timed out', SH, "both sides time out"),
    ("shadow-timeout-rust-ignored", '124|137) why="${why:+$why; }the Rust side timed out', '999) why="${why:+$why; }the Rust side timed out', SH, "outlives the timeout"),
    ("shadow-doctor-not-normalised", "doctor) normalise_doctor < \"$base/bash/$s\" > \"$base/bash/$s.n\"; normalise_doctor < \"$base/rust/$s\" > \"$base/rust/$s.n\" ;;", "doctor) cp -- \"$base/bash/$s\" \"$base/bash/$s.n\"; cp -- \"$base/rust/$s\" \"$base/rust/$s.n\" ;;", SH, "only in a timestamp and a pid"),
    ("shadow-normalise-everything", "doctor) normalise_doctor", "*) normalise_doctor", SH, "only the doctor's output is normalised"),
    ("shadow-git-writes-anywhere", 'if inside_tmp "$target"; then exec', 'if true; then exec', SH, "git changes nothing outside TMPDIR"),
    ("shadow-tmpdir-per-side", '"TMPDIR=${sd%/*}/tmp"', '"TMPDIR=$sd/tmp"', SH, "same TMPDIR path"),
    ("shadow-calls-not-normalised", 'normalise_calls < "$base/bash/calls.txt" > "$base/bash/calls.n"', 'cp -- "$base/bash/calls.txt" "$base/bash/calls.n"', SH, "random part of a mktemp name"),
    ("shadow-fetch-not-swallowed", 'fetch) printf \'git %s\\n\' "${line% }" >> "$log/swallowed"; exit 0 ;;', 'fetch) exec "${CUTOVER_REAL_GIT:?}" "$@" ;;', SH, "git fetch is swallowed"),
    ("shadow-git-push-allowed", 'rev-parse|rev-list|describe|log|diff|diff-tree|show|status|merge-base|for-each-ref|ls-files|ls-tree|cat-file|show-ref|name-rev|shortlog|blame|grep|version|help|--version)', 'push|rev-parse|rev-list|describe|log|diff|diff-tree|show|status|merge-base|for-each-ref|ls-files|ls-tree|cat-file|show-ref|name-rev|shortlog|blame|grep|version|help|--version)', SH, "a git push is refused"),
    ("shadow-gh-write-allowed", 'comment|merge|review|close|reopen|edit|create|ready|lock|unlock|update-branch|delete|transfer|pin|unpin|checkout) refuse "$*" ;;', 'XX) refuse "$*" ;;', SH, "a gh write is refused"),
    ("shadow-date-not-fixed", 'if [ "$*" = "-Is" ]; then printf \'%s\\n\' "${CUTOVER_FIXED_DATE:?}"; exit 0; fi', 'true', SH, "fixed instant"),
    ("shadow-crontab-write-allowed", 'if [ "${1:-}" = -l ] && [ "$#" -eq 1 ]; then echo "no crontab for ${USER:-user}" >&2; exit 1; fi', 'if [ "${1:-}" = -l ] || [ "${1:-}" = - ]; then echo "no crontab for ${USER:-user}" >&2; exit 1; fi', SH, "crontab answers a list"),
    ("shadow-systemctl-enable-allowed", 'show-environment|list-timers|list-unit-files|is-active|is-enabled|is-failed|show|cat|status|list-units)', 'show-environment|list-timers|list-unit-files|is-active|is-enabled|is-failed|show|cat|status|list-units|enable)', SH, "systemctl may list timers"),
    ("shadow-podman-pull-allowed", 'if [ "${1:-}" = run ]; then\n    rm_seen=0', 'if [ "${1:-}" = run ] || [ "${1:-}" = pull ]; then\n    rm_seen=1', SH, "the probe may inspect an image"),
    ("shadow-systemd-run-detached", 'case "$a" in --wait|--scope) exec', 'case "$a" in --user) exec', SH, "systemd-run may run"),
    ("shadow-systemd-run-scope-refused", 'case "$a" in --wait|--scope) exec', 'case "$a" in --wait) exec', SH, "systemd-run may run"),
    ("shadow-sigterm-ignored", "trap 'stop_side 143' TERM", "trap ':' TERM", SH, "SIGTERM ends the run"),
    ("shadow-sigterm-side-left-running", '[ -z "$SH_SIDE_PID" ] || kill -TERM "$SH_SIDE_PID" 2>/dev/null || true', 'true', SH, "SIGTERM ends the run"),
    ("build-sigterm-ignored", "trap 'exit 143' TERM", "trap ':' TERM", B, "SIGTERM ends the build"),
    ("build-stage-leftover-foreign", '# A build that was killed leaves its scratch directory; it is not a foreign file.\n            case "$name" in .stage.*|.shadow.*|.shadow-ok.*) continue ;; esac', '', B, "killed build is not a foreign file"),
    ("shadow-token-passed", '( cd -- "$cwd" && exec env -i "${envv[@]}" timeout', '( cd -- "$cwd" && exec env "${envv[@]}" timeout', SH, "token in the caller"),
    ("shadow-scratch-kept", '    if [ -z "$SH_KEEP" ]; then\n        CLEAN_DIR="$SH_SCRATCH"\n        trap clean_up EXIT', '    if false; then\n        CLEAN_DIR="$SH_SCRATCH"\n        trap clean_up EXIT', SH, "scratch directory"),
    ("shadow-marker-wrong-table", "{ printf '%s\\n' \"$table\"; printf '# checkout", "{ printf '%s\\n' \"$recorded\" | tac; printf '# checkout", SH, "holds the sha256 table"),
    ("shadow-marker-programs-changed", '[ "$(table_of "$dir")" = "$table" ] || why_not=', 'true || why_not=', SH, "changes during the run"),
]


def tree_hash(root):
    h = hashlib.sha256()
    for it in ITEMS:
        p = os.path.join(root, it)
        files = [p] if os.path.isfile(p) else sorted(os.path.join(d, f) for d, _, fs in os.walk(p) for f in fs)
        for f in files:
            if os.path.islink(f):
                continue
            h.update(f[len(root):].encode()); h.update(open(f, "rb").read())
    return h.hexdigest()


def fresh(work, source=None):
    """A touched copy of the items, from the checkout or from a pristine copy."""
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(work)
    base = source or ROOT
    for it in ITEMS:
        src, dst = os.path.join(base, it), os.path.join(work, it)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        if os.path.isdir(src):
            shutil.copytree(src, dst, symlinks=True)
        else:
            shutil.copy2(src, dst)
    for d, _, fs in os.walk(work):
        for f in fs:
            if not os.path.islink(os.path.join(d, f)):
                os.utime(os.path.join(d, f))
    # git needs the checkout to be a repository for the fixture tree the cases copy from the script.
    time.sleep(1.1)


def run(work, case_file, only=None, cap=CAP_SECONDS):
    env = {k: v for k, v in os.environ.items() if k not in ("MX_TREE", "CU_ONLY")}
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    if only:
        env["CU_ONLY"] = only
    cmd = "ulimit -v %d; exec bash tests/run-characterization.sh %s" % (CAP_BYTES // 1024, case_file)
    try:
        cp = subprocess.run(["bash", "-c", cmd], cwd=work, capture_output=True, text=True, env=env, timeout=cap)
    except subprocess.TimeoutExpired:
        return "CAPPED", None, []
    out = cp.stdout + cp.stderr
    m = re.findall(r"^(\d+) cases, (\d+) passed, (\d+) failed", out, re.M)
    fails = [l[6:] for l in out.splitlines() if l.startswith("FAIL  ")]
    if not m:
        return "BROKEN", None, [out[-200:].replace("\n", " ")]
    return "RAN", tuple(map(int, m[-1])), fails


def main():
    args = sys.argv[1:]
    if args == ["--list"]:
        for m in MUTANTS:
            print(m[0])
        return 0
    want = args
    ids = [m[0] for m in MUTANTS]
    unknown = [w for w in want if w not in ids]
    if unknown:
        sys.exit("unknown mutant: " + ", ".join(unknown))
    if len(set(ids)) != len(ids):
        sys.exit("duplicate mutant ids")
    tmp = tempfile.mkdtemp(prefix="mut-cutover-", dir=os.path.expanduser("~/.cache/magent-wt-scratch/p6a"))
    pristine = os.path.join(tmp, "pristine")
    work = os.path.join(tmp, "w")
    fresh(pristine)
    pristine_hash = tree_hash(pristine)
    bad = 0
    # Every old text must occur once in the pristine script, checked before anything runs.
    text = open(os.path.join(pristine, CUT)).read()
    for mid, old, new, case_file, only in MUTANTS:
        if text.count(old) != 1:
            print("%s: the old text occurs %d times" % (mid, text.count(old)))
            bad += 1
    if bad:
        return 2
    controls = {}
    for cf in sorted({m[3] for m in MUTANTS}):
        if want and cf not in {m[3] for m in MUTANTS if m[0] in want}:
            continue
        fresh(work, pristine)
        kind, res, _ = run(work, cf, None, CONTROL_CAP_SECONDS)
        controls[cf] = res
        print("pristine control before, %-20s %s %s" % (cf, kind, res), flush=True)
        if kind != "RAN" or res[2] != 0:
            bad += 1
    print("%-34s %-8s %s" % ("mutant", "result", "case that failed"))
    killed = 0
    for mid, old, new, case_file, only in MUTANTS:
        if want and mid not in want:
            continue
        fresh(work, pristine)
        p = os.path.join(work, CUT)
        s = open(p).read()
        assert s.count(old) == 1, "%s: old text occurs %d times" % (mid, s.count(old))
        with open(p, "w") as fh:
            fh.write(s.replace(old, new))
        back = open(p).read()
        assert back != s and new in back, mid + ": the mutation did not land"
        if subprocess.run(["bash", "-n", p]).returncode != 0:
            print("%-34s BROKEN   the mutant does not parse" % mid, flush=True); bad += 1; continue
        kind, res, fails = run(work, case_file, only)
        hit = [f for f in fails if only in f]
        if kind == "RAN" and res and res[2] > 0 and hit:
            killed += 1
            print("%-34s KILLED   %s" % (mid, hit[0][:90]), flush=True)
        else:
            bad += 1
            print("%-34s %-8s %s fails=%s" % (mid, "SURVIVED" if kind == "RAN" else kind, res, [f[:60] for f in fails][:2]), flush=True)
    for cf in sorted(controls):
        fresh(work, pristine)
        kind, res, _ = run(work, cf, None, CONTROL_CAP_SECONDS)
        same = tree_hash(work) == pristine_hash
        print("pristine control after,  %-20s %s %s, tree hash unchanged: %s" % (cf, kind, res, same), flush=True)
        if kind != "RAN" or res[2] != 0 or not same:
            bad += 1
    print("killed %d of %d" % (killed, len([m for m in MUTANTS if not want or m[0] in want])))
    shutil.rmtree(tmp, ignore_errors=True)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
