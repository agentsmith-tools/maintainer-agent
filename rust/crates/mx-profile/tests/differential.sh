#!/usr/bin/env bash
# Differential proof: the mx-profile crate against lib/profile.sh.
#
# For every input the bash function (sourced in a clean `env -i` shell) and the
# Rust oracle (src/bin/mx-profile-oracle.rs) run side by side. The two must print
# the same bytes and exit with the same code.
#
#   classify   maintainer_classify_failure, backend_transient_reason and
#              backend_needs_human_reason: every characterization fixture, one log
#              for every shipped failure shape, hostile tables and logs, and 200
#              generated logs plus 80 generated tables (seeded).
#   scrub      maintainer_scrub_secret on two identical trees per case. The report,
#              the exit code and the whole resulting tree (paths, types, modes,
#              link targets, file hashes) must match.
#   load       maintainer_load_profile: stdout, stderr, exit code, every variable
#              the sourced files left and the names the function exported. The
#              profiles this repository ships are loaded too, with and without
#              MAINTAINER_* overrides.
#   pinned     the known differences between the bash and the port (decisions
#              P1F-3 to P1F-7 and P1F-12 in the pull request). Each is asserted on BOTH sides,
#              so a difference that changes, or goes away, is noticed.
#
# Both sides run under LC_ALL=C with the same PATH: the Proc port forces LC_ALL=C
# on every child it starts, so the bash is held to the same locale (P1F-5).
#
# Usage: differential.sh
# Env:   MX_TREE       tree whose lib/ is tested (default: this checkout)
#        MX_SCRATCH    where the scratch directory goes (default: TMPDIR, then /tmp)
#        DIFF_SEED     the seed of the generated cases (default: from the clock;
#                      it is printed, so a failing run can be repeated)
#        DIFF_VERBOSE  1 prints a line for every comparison
# Exit:  0 when every comparison matched and more than MIN_COMPARISONS ran.
# SC2015: ok and bad both return 0, so `A && ok || bad` is an if/else here.
# SC2016: the single-quoted bash -c programs expand their own $1 and $@.
# shellcheck disable=SC2015,SC2016
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ws="$(cd "$here/../../.." && pwd)"
root="$(cd "$ws/.." && pwd)"
MX_TREE="${MX_TREE:-$root}"
src_lib="$MX_TREE/lib"
MIN_COMPARISONS=300
SAFE_PATH=/usr/bin:/bin
verbose="${DIFF_VERBOSE:-0}"
seed="${DIFF_SEED:-$(date +%s)}"
seed=$(( seed % 1000000 ))

for f in profile.sh defaults.env failure-shapes.json; do
    [ -f "$src_lib/$f" ] || { echo "differential: no lib/$f under MX_TREE=$MX_TREE" >&2; exit 2; }
done
for tool in python3 timeout cmp find sort xargs sha256sum; do
    command -v "$tool" >/dev/null 2>&1 || { echo "differential: the host has no '$tool'" >&2; exit 2; }
done
[ -x /usr/bin/grep ] && [ -x /usr/bin/sort ] || { echo "differential: no /usr/bin/grep or /usr/bin/sort" >&2; exit 2; }

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ws/target}"
built="$(cd "$ws" && cargo build --locked -q -p mx-profile --bin mx-profile-oracle 2>&1)"; rc=$?
[ "$rc" = 0 ] || { printf '%s\ndifferential: cargo build failed (exit %s)\n' "$built" "$rc" >&2; exit 2; }
ORACLE="$CARGO_TARGET_DIR/debug/mx-profile-oracle"
[ -x "$ORACLE" ] || { echo "differential: $ORACLE was not built" >&2; exit 2; }

scratch="$(mktemp -d "${MX_SCRATCH:-${TMPDIR:-/tmp}}/mx-profile-diff.XXXXXX")" || exit 2
# The trap runs this function; shellcheck cannot see that (SC2317).
# shellcheck disable=SC2317
cleanup() { chmod -R u+rwX "$scratch" 2>/dev/null; rm -rf "$scratch"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

printf 'differential: seed %s (repeat with DIFF_SEED=%s)\n' "$seed" "$seed"
printf 'differential: bash %s, %s\n' "${BASH_VERSION}" "$(/usr/bin/grep --version | head -1)"

# ---- the trees the cases run against ---------------------------------------------
mkdir -p "$scratch/home" "$scratch/lib" "$scratch/lib-notable" "$scratch/libs"
cp "$src_lib/profile.sh" "$src_lib/defaults.env" "$src_lib/failure-shapes.json" "$scratch/lib/" || exit 2
cp "$src_lib/profile.sh" "$src_lib/defaults.env" "$scratch/lib-notable/" || exit 2
mklib() {  # $1 = name, $2 = defaults.env text, or - for none
    mkdir -p "$scratch/libs/$1"
    cp "$src_lib/profile.sh" "$scratch/libs/$1/profile.sh"
    [ "$2" = "-" ] || printf '%s' "$2" > "$scratch/libs/$1/defaults.env"
}
mklib std $'# tunables\nLOCK_WAIT=3600\nRETENTION_DAYS=90\nGH_TRIES=3\nVERIFY_MEMORY=2g\nEMPTY_DEFAULT=\n  INDENTED=2\nlower=3\n'
mklib nodefaults -
mklib failing $'LOCK_WAIT=1\nfalse\n'
mklib printing $'echo defaults-stdout\necho defaults-stderr >&2\nLOCK_WAIT=3600\n'

corpus_out="$(python3 -I "$here/differential_corpus.py" "$scratch/gen" "$seed" "$MX_TREE/profiles" 2>&1)"; rc=$?
[ "$rc" = 0 ] || { printf '%s\ndifferential: the corpus generator failed (exit %s)\n' "$corpus_out" "$rc" >&2; exit 2; }
gen="$scratch/gen"
printf 'differential: corpus: %s\n' "$(tr '\n' ';' <<<"$corpus_out")"

# ---- counters -----------------------------------------------------------------------
N=0; F=0; K=0
ok()  { N=$((N + 1)); [ "$verbose" != 1 ] || printf 'ok    %s\n' "$1"; }
bad() {
    N=$((N + 1)); F=$((F + 1)); printf 'FAIL  %s\n' "$1"
    [ -z "${2:-}" ] || printf '%s\n' "$2" | sed -n '1,12p' | sed 's/^/      /'
}
pin_ok()  { K=$((K + 1)); [ "$verbose" != 1 ] || printf 'pin   %s\n' "$1"; }
pin_bad() { K=$((K + 1)); F=$((F + 1)); printf 'FAIL  pinned: %s\n' "$1"; [ -z "${2:-}" ] || printf '%s\n' "$2" | sed -n '1,12p' | sed 's/^/      /'; }

show() {  # first bytes of a file, escaped, for a failure message
    local f="$1"
    [ -e "$f" ] || { printf '(absent)'; return; }
    head -c 300 "$f" | od -An -c | tr -s ' ' | head -4 | tr '\n' ' '
}

# compare LABEL BASH_OUT BASH_RC ORACLE_OUT ORACLE_RC
compare() {
    local label="$1" bo="$2" brc="$3" oo="$4" orc="$5"
    if cmp -s "$bo" "$oo" && [ "$brc" = "$orc" ]; then
        ok "$label"
    else
        bad "$label" "bash   rc=$brc out: $(show "$bo")"$'\n'"oracle rc=$orc out: $(show "$oo")"
    fi
}

# ======================================================================================
# classify
# ======================================================================================
declare -A ids_seen=()
cls_n0=$N; cls_f0=$F

one_classify() {  # id label table flags
    local id="$1" label="$2" table="$3" flags="$4"
    local dir="$gen/classify" arg lib="$scratch/lib"
    IFS= read -r -d '' arg < "$dir/$id.arg" || true
    local -a envs=("LC_ALL=C" "PATH=$SAFE_PATH" "HOME=$scratch/home")
    case "$table" in
        -) ;;
        none) lib="$scratch/lib-notable" ;;
        missing) envs+=("MAINTAINER_FAILURE_SHAPES=$scratch/nonexistent.json") ;;
        empty-env) envs+=("MAINTAINER_FAILURE_SHAPES=") ;;
        *) envs+=("MAINTAINER_FAILURE_SHAPES=$dir/tables/$table") ;;
    esac
    local w="$scratch/w-classify"; mkdir -p "$w"
    ( cd "$dir" && env -i "${envs[@]}" timeout 120 bash -c '. "$1/profile.sh"; maintainer_classify_failure "$2"; exit $?' bash "$lib" "$arg" ) >"$w/b.out" 2>"$w/b.err" </dev/null
    local brc=$?
    ( cd "$dir" && env -i "${envs[@]}" timeout 120 "$ORACLE" classify --lib-dir "$lib" "$arg" ) >"$w/o.out" 2>"$w/o.err" </dev/null
    local orc=$?
    compare "classify $id: $label" "$w/b.out" "$brc" "$w/o.out" "$orc"
    case "$label" in
        shape:*) local _cls sid _rest; IFS=$'\t' read -r _cls sid _rest < "$w/b.out" || true
                 [ "$brc" != 0 ] || ids_seen["$sid"]=1 ;;
    esac
    case "$flags" in *p*)
        local which fn
        for which in transient needs-human; do
            case "$which" in transient) fn=backend_transient_reason ;; *) fn=backend_needs_human_reason ;; esac
            ( cd "$dir" && env -i "${envs[@]}" timeout 120 bash -c '. "$1/profile.sh"; "$3" "$2"; exit $?' bash "$lib" "$arg" "$fn" ) >"$w/b.out" 2>"$w/b.err" </dev/null
            brc=$?
            ( cd "$dir" && env -i "${envs[@]}" timeout 120 "$ORACLE" predicate "$which" --lib-dir "$lib" "$arg" ) >"$w/o.out" 2>"$w/o.err" </dev/null
            orc=$?
            compare "predicate $which $id: $label" "$w/b.out" "$brc" "$w/o.out" "$orc"
        done ;;
    esac
}

while IFS=$'\t' read -r id label table flags; do
    one_classify "$id" "$label" "$table" "$flags"
done < "$gen/classify/cases.tsv"

# Every shape id of the shipped table must have been produced by the bash on some case.
table_ids="$(python3 -I - "$src_lib/failure-shapes.json" <<'PYEOF'
import json, sys
print(" ".join(sorted(s["id"] for s in json.load(open(sys.argv[1]))["shapes"])))
PYEOF
)" || { echo "differential: cannot read the shipped table" >&2; exit 2; }
missing_ids=""; n_ids=0; n_hit=0
for sid in $table_ids; do
    n_ids=$((n_ids + 1))
    if [ -n "${ids_seen[$sid]:-}" ]; then n_hit=$((n_hit + 1)); else missing_ids="$missing_ids $sid"; fi
done
printf 'differential: classify: %s comparisons, %s differences, %s s; the bash produced %s of the %s shipped shape ids\n' \
    "$((N - cls_n0))" "$((F - cls_f0))" "$SECONDS" "$n_hit" "$n_ids"
# died-without-saying is triggered by the text `!xx` only on a grep that reads
# (?!x)x leniently (GNU grep 3.12 does). Every other shape must be triggered.
case "$missing_ids" in
    "") ok "coverage: every shipped shape id was produced by the bash" ;;
    " died-without-saying") ok "coverage: every shipped shape id but died-without-saying was produced (this grep rejects its pattern)" ;;
    *) bad "coverage: shape ids never produced by the bash:$missing_ids" ;;
esac

# ======================================================================================
# scrub
# ======================================================================================
scr_n0=$N; scr_f0=$F
manifest() {  # a tree's paths, types, modes, link targets and file hashes
    ( cd "$1" && {
        find . -printf '%p\t%y\t%m\t%l\n' 2>/dev/null | LC_ALL=C sort
        find . -type f -print0 2>/dev/null | LC_ALL=C sort -z | xargs -0 -r sha256sum 2>/dev/null
    } )
}

one_scrub() {  # id label
    local id="$1" label="$2" base="$gen/scrub/$1" secret
    IFS= read -r -d '' secret < "$base/secret" || true
    local -a targets=()
    mapfile -t targets < "$base/targets"
    local w="$scratch/w-scrub"; mkdir -p "$w"
    ( cd "$base/a" && env -i LC_ALL=C PATH="$SAFE_PATH" HOME="$scratch/home" MX_DIFF_SECRET="$secret" timeout 120 \
        bash -c '. "$1/profile.sh"; shift; maintainer_scrub_secret "$MX_DIFF_SECRET" "$@"; exit $?' bash "$scratch/lib" "${targets[@]}" ) >"$w/b.out" 2>"$w/b.err" </dev/null
    local brc=$?
    ( cd "$base/b" && env -i LC_ALL=C PATH="$SAFE_PATH" HOME="$scratch/home" MX_DIFF_SECRET="$secret" timeout 120 \
        "$ORACLE" scrub MX_DIFF_SECRET "${targets[@]}" ) >"$w/o.out" 2>"$w/o.err" </dev/null
    local orc=$?
    manifest "$base/a" > "$w/b.tree"
    manifest "$base/b" > "$w/o.tree"
    if cmp -s "$w/b.out" "$w/o.out" && [ "$brc" = "$orc" ] && cmp -s "$w/b.tree" "$w/o.tree"; then
        ok "scrub $id: $label"
    else
        local detail
        detail="bash   rc=$brc out: $(show "$w/b.out")"$'\n'"oracle rc=$orc out: $(show "$w/o.out")"
        cmp -s "$w/b.tree" "$w/o.tree" || detail="$detail"$'\n'"trees differ: $(diff "$w/b.tree" "$w/o.tree" | head -6 | tr '\n' ' ')"
        bad "scrub $id: $label" "$detail"
    fi
}

while IFS=$'\t' read -r id label; do
    one_scrub "$id" "$label"
done < "$gen/scrub/cases.tsv"
printf 'differential: scrub: %s comparisons, %s differences (%s s elapsed)\n' "$((N - scr_n0))" "$((F - scr_f0))" "$SECONDS"

# ======================================================================================
# load
# ======================================================================================
ld_n0=$N; ld_f0=$F

read -r -d '' BASH_LOAD_VALUES <<'EOF'
set -a
. "$1/profile.sh"
maintainer_load_profile
__h_rc=$?
set +a
[ "$__h_rc" = 0 ] || exit "$__h_rc"
__h_names="$(builtin compgen -e | /usr/bin/sort)"
{
    while IFS= builtin read -r __h_n; do
        case $__h_n in PWD|OLDPWD|SHLVL|_|__h_*) continue ;; esac
        builtin printf '%s\0%s\0' "$__h_n" "${!__h_n}"
    done <<< "$__h_names"
} > "$2"
exit 0
EOF
read -r -d '' BASH_LOAD_EXPORTS <<'EOF'
for __h_n in $(builtin compgen -v); do
    case $__h_n in MAINTAINER_*) export -n "$__h_n" ;; esac
done
. "$1/profile.sh"
maintainer_load_profile
__h_rc=$?
[ "$__h_rc" = 0 ] || exit "$__h_rc"
# A pipeline into a file: its status is not read, and no match writes an empty file.
builtin compgen -e | /usr/bin/grep '^MAINTAINER_' | /usr/bin/sort > "$2"
exit 0
EOF
bash -n <<<"$BASH_LOAD_VALUES" || { echo "differential: the values script does not parse" >&2; exit 2; }
bash -n <<<"$BASH_LOAD_EXPORTS" || { echo "differential: the exports script does not parse" >&2; exit 2; }

run_load_pair() {  # id lib -> leaves $w/{b,o}.{out,err,dump,exports} and b_rc o_rc
    local id="$1" lib="$2" base="$gen/load/$1"
    local -a kv=()
    mapfile -t kv < "$base/env"
    local -a envs=("LC_ALL=C" "PATH=$SAFE_PATH" "HOME=$base/home")
    [ "${#kv[@]}" = 0 ] || envs+=("${kv[@]}")
    w="$scratch/w-load"; rm -rf "$w"; mkdir -p "$w"
    # The braces catch the notice this shell prints when a profile kills its own shell.
    { ( cd "$base" && env -i "${envs[@]}" timeout 120 bash -c "$BASH_LOAD_VALUES" bash "$scratch/libs/$lib" "$w/b.dump" ) >"$w/b.out" 2>"$w/b.err" </dev/null; } 2>/dev/null
    b_rc=$?
    { ( cd "$base" && env -i "${envs[@]}" timeout 120 bash -c "$BASH_LOAD_EXPORTS" bash "$scratch/libs/$lib" "$w/b.exports" ) >/dev/null 2>&1 </dev/null; } 2>/dev/null
    { ( cd "$base" && env -i "${envs[@]}" timeout 120 "$ORACLE" load --lib-dir "$scratch/libs/$lib" --dump "$w/o.dump" --exports "$w/o.exports" ) >"$w/o.out" 2>"$w/o.err" </dev/null; } 2>/dev/null
    o_rc=$?
}

one_load() {  # id label lib flags
    local id="$1" label="$2" lib="$3" flags="$4" w b_rc o_rc
    run_load_pair "$id" "$lib"
    case "$flags" in *path*) pinned_path "$id" "$label" "$w"; return ;; esac
    local same=1 what=""
    cmp -s "$w/b.out" "$w/o.out" || { same=0; what="$what stdout"; }
    cmp -s "$w/b.err" "$w/o.err" || { same=0; what="$what stderr"; }
    [ "$b_rc" = "$o_rc" ] || { same=0; what="$what rc"; }
    if [ -e "$w/b.dump" ] || [ -e "$w/o.dump" ]; then
        cmp -s "$w/b.dump" "$w/o.dump" || { same=0; what="$what variables"; }
    fi
    if [ -e "$w/b.exports" ] || [ -e "$w/o.exports" ]; then
        cmp -s "$w/b.exports" "$w/o.exports" || { same=0; what="$what exports"; }
    fi
    if [ "$same" = 1 ]; then
        ok "load $id: $label"
    else
        local detail="differs in:$what; bash rc=$b_rc, oracle rc=$o_rc"
        detail="$detail"$'\n'"bash   stdout: $(show "$w/b.out") stderr: $(show "$w/b.err")"
        detail="$detail"$'\n'"oracle stdout: $(show "$w/o.out") stderr: $(show "$w/o.err")"
        cmp -s "$w/b.dump" "$w/o.dump" || detail="$detail"$'\n'"variables: $(diff <(tr '\0' '\n' < "$w/b.dump" 2>/dev/null) <(tr '\0' '\n' < "$w/o.dump" 2>/dev/null) | head -8 | tr '\n' ' ')"
        bad "load $id: $label" "$detail"
    fi
}

# A profile that sets PATH to nothing: the bash override step runs `sed`, which it
# then cannot find, so MAINTAINER_<KEY> is not applied. The port applies it.
pinned_path() {  # id label workdir
    local id="$1" label="$2" w="$3"
    local bv ov
    bv="$(tr '\0' '\n' < "$w/b.dump" 2>/dev/null | /usr/bin/grep -A1 -x 'LOCK_WAIT' | tail -1)"
    ov="$(tr '\0' '\n' < "$w/o.dump" 2>/dev/null | /usr/bin/grep -A1 -x 'LOCK_WAIT' | tail -1)"
    if [ "$bv" = "10" ] && [ "$ov" = "7" ]; then
        pin_ok "load $id: $label: bash keeps the profile's LOCK_WAIT=10 because sed is not found, the port applies MAINTAINER_LOCK_WAIT=7"
    else
        pin_bad "load $id: $label" "expected bash LOCK_WAIT=10 and oracle LOCK_WAIT=7, got bash '$bv' and oracle '$ov'"
    fi
}

while IFS=$'\t' read -r id label lib flags; do
    one_load "$id" "$label" "$lib" "$flags"
done < "$gen/load/cases.tsv"
printf 'differential: load: %s comparisons, %s differences (%s s elapsed)\n' "$((N - ld_n0))" "$((F - ld_f0))" "$SECONDS"

# ======================================================================================
# pinned differences (decisions P1F-3 to P1F-7 and P1F-12)
# ======================================================================================
pn="$scratch/pinned"; mkdir -p "$pn"
penv=(LC_ALL=C PATH="$SAFE_PATH" HOME="$scratch/home")

# P1F-4: python accepts the literals NaN and Infinity and a lone surrogate escape in
# a field nobody prints. serde_json refuses them, so the port cannot read the table.
printf 'it went boom\n' > "$pn/boom.log"
for spec in 'nan|{"v": NaN, "shapes": [{"id": "a", "class": "t", "pattern": "boom"}]}' \
            'infinity|{"v": -Infinity, "shapes": [{"id": "a", "class": "t", "pattern": "boom"}]}' \
            'surrogate|{"v": "\ud800", "shapes": [{"id": "a", "class": "t", "pattern": "boom"}]}'; do
    name="${spec%%|*}"; printf '%s\n' "${spec#*|}" > "$pn/$name.json"
    b="$(env -i "${penv[@]}" MAINTAINER_FAILURE_SHAPES="$pn/$name.json" bash -c '. "$1/profile.sh"; maintainer_classify_failure "$2"; echo " rc=$?"' bash "$scratch/lib" "$pn/boom.log" 2>/dev/null)"
    o="$(env -i "${penv[@]}" MAINTAINER_FAILURE_SHAPES="$pn/$name.json" "$ORACLE" classify --lib-dir "$scratch/lib" "$pn/boom.log" 2>/dev/null; echo " rc=$?")"
    if [ "$b" = $'t\ta\tboom rc=0' ] && [ "$o" = "unreadable"$'\t\t'"failure-shapes table at $pn/$name.json could not be parsed rc=2" ]; then
        pin_ok "P1F-4 json $name: bash reads the table, the port says it could not parse it"
    else
        pin_bad "P1F-4 json $name" "bash: $b"$'\n'"oracle: $o"
    fi
done

# P1F-3: the bash opens a named pipe and waits for a writer, so a scrub over a tree
# with one never returns. The port reads regular files only.
mkdir -p "$pn/fifo/a" "$pn/fifo/b"
for side in a b; do mkfifo "$pn/fifo/$side/pipe"; printf 'x ghp_FIXTURE0123456789abcdefghij y\n' > "$pn/fifo/$side/f.txt"; done
( cd "$pn/fifo/a" && env -i "${penv[@]}" MX_DIFF_SECRET=ghp_FIXTURE0123456789abcdefghij timeout 4 bash -c '. "$1/profile.sh"; maintainer_scrub_secret "$MX_DIFF_SECRET" .' bash "$scratch/lib" ) >"$pn/fifo.b.out" 2>&1 </dev/null
b_rc=$?
( cd "$pn/fifo/b" && env -i "${penv[@]}" MX_DIFF_SECRET=ghp_FIXTURE0123456789abcdefghij timeout 20 "$ORACLE" scrub MX_DIFF_SECRET . ) >"$pn/fifo.o.out" 2>&1 </dev/null
o_rc=$?
# The pipe may sort before or after f.txt, so the bash may have scrubbed f.txt first.
if [ "$b_rc" = 124 ] && [ "$o_rc" = 0 ] && [ "$(cat "$pn/fifo.o.out")" = "scrubbed the pinned token from ./f.txt" ]; then
    pin_ok "P1F-3 fifo: the bash hangs on a named pipe (timeout, exit 124), the port skips it and reports f.txt"
else
    pin_bad "P1F-3 fifo" "bash rc=$b_rc out: $(show "$pn/fifo.b.out"); oracle rc=$o_rc out: $(show "$pn/fifo.o.out")"
fi

# P1F-6: a log path that is not UTF-8 text cannot be handed to grep through the
# Proc port, so the port says it could not ask where the bash classifies.
bad_log="$pn/bad-$(printf '\xff').log"
printf 'Killed\n' > "$bad_log"
b="$(env -i "${penv[@]}" bash -c '. "$1/profile.sh"; maintainer_classify_failure "$2"; echo " rc=$?"' bash "$scratch/lib" "$bad_log" 2>/dev/null)"
o="$(env -i "${penv[@]}" "$ORACLE" classify --lib-dir "$scratch/lib" "$bad_log" 2>/dev/null; echo " rc=$?")"
if [ "$b" = $'needs_human\tkilled-silently\tKilled rc=0' ] && [ "$o" = $'unreadable\t\tthe log path is not UTF-8 text, so grep cannot be given it rc=2' ]; then
    pin_ok "P1F-6 non-UTF-8 log path: the bash classifies it, the port says it could not ask"
else
    pin_bad "P1F-6 non-UTF-8 log path" "bash: $b"$'\n'"oracle: $o"
fi

# P1F-5: the bash runs grep under the caller's locale. Under a UTF-8 locale `.{0,60}`
# counts characters, so the text is longer; under C it counts bytes, as the port does.
{ printf 'usage limit reached '; for _ in $(seq 1 80); do printf '\xc3\xa9'; done; printf '\n'; } > "$pn/utf8.log"
bu="$(env -i LC_ALL=C.UTF-8 PATH="$SAFE_PATH" HOME="$scratch/home" bash -c '. "$1/profile.sh"; maintainer_classify_failure "$2"' bash "$scratch/lib" "$pn/utf8.log" 2>/dev/null | wc -c)"
bc="$(env -i "${penv[@]}" bash -c '. "$1/profile.sh"; maintainer_classify_failure "$2"' bash "$scratch/lib" "$pn/utf8.log" 2>/dev/null | wc -c)"
oc="$(env -i "${penv[@]}" "$ORACLE" classify --lib-dir "$scratch/lib" "$pn/utf8.log" 2>/dev/null | wc -c)"
if [ "$bu" -gt "$bc" ] && [ "$bc" = "$oc" ] && [ "$oc" -gt 0 ]; then
    pin_ok "P1F-5 locale: the bash under C.UTF-8 prints $bu bytes, under C and in the port $oc"
else
    pin_bad "P1F-5 locale" "bash C.UTF-8: $bu bytes, bash C: $bc, oracle: $oc"
fi

# P1F-7: the child sees HOME, PROFILE_DIR, MAINTAINER_* and the adapter's PATH. A
# profile that expands another variable gets the empty string.
mkdir -p "$pn/home7/.local/share/maintainer/profiles/fx"
printf 'REPO_SLUG=o/r\nSEEN_USER="${USER-unset}"\nSEEN_TOKEN="${GH_TOKEN-unset}"\n' > "$pn/home7/.local/share/maintainer/profiles/fx/profile.env"
env7=(LC_ALL=C PATH="$SAFE_PATH" HOME="$pn/home7" USER=someone GH_TOKEN=ghp_caller_token)
env -i "${env7[@]}" bash -c "$BASH_LOAD_VALUES" bash "$scratch/libs/std" "$pn/b7.dump" >/dev/null 2>&1 </dev/null
env -i "${env7[@]}" "$ORACLE" load --lib-dir "$scratch/libs/std" --dump "$pn/o7.dump" >/dev/null 2>&1 </dev/null
bu="$(tr '\0' '\n' < "$pn/b7.dump" | /usr/bin/grep -A1 -x 'SEEN_USER' | tail -1)"
ou="$(tr '\0' '\n' < "$pn/o7.dump" | /usr/bin/grep -A1 -x 'SEEN_USER' | tail -1)"
bt="$(tr '\0' '\n' < "$pn/b7.dump" | /usr/bin/grep -A1 -x 'SEEN_TOKEN' | tail -1)"
ot="$(tr '\0' '\n' < "$pn/o7.dump" | /usr/bin/grep -A1 -x 'SEEN_TOKEN' | tail -1)"
if [ "$bu" = someone ] && [ "$ou" = unset ] && [ "$bt" = ghp_caller_token ] && [ "$ot" = unset ]; then
    pin_ok "P1F-7 environment: the bash profile sees USER and GH_TOKEN, the port's child does not"
else
    pin_bad "P1F-7 environment" "bash USER='$bu' GH_TOKEN='$bt'; oracle USER='$ou' GH_TOKEN='$ot'"
fi

# ======================================================================================
printf 'differential: %s comparisons, %s differences, %s pinned differences checked (seed %s)\n' "$N" "$F" "$K" "$seed"
[ "$N" -gt 0 ] || { echo "differential: nothing was compared" >&2; exit 1; }
[ "$N" -gt "$MIN_COMPARISONS" ] || { echo "differential: only $N comparisons ran; more than $MIN_COMPARISONS must" >&2; exit 1; }
[ "$K" -ge 7 ] || { echo "differential: only $K pinned differences were checked; 7 must be" >&2; exit 1; }
[ "$F" = 0 ] || exit 1
exit 0
