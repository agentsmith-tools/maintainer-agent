#!/usr/bin/env bash
# Every `compile_fail,E....` doc example of the gate crates must fail with exactly
# the error code in its tag. On stable, rustdoc accepts the tag for ANY compile
# error, so a typo in an example keeps the test green while it tests nothing
# (rv264 B4). This compiles each example alone as a binary of a scratch project
# that depends on the crates by path, and reads the codes rustc printed.
#
# Prints `compile-fail: N examples, F failed` and exits 1 when any code differs or
# when no example was found (a check that ran nothing is not a pass).
set -uo pipefail
ws="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/cf-codes.XXXXXX")" || { echo "cannot make a scratch directory" >&2; exit 1; }
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/src/bin"
cat > "$scratch/Cargo.toml" <<TOML
[package]
name = "cf"
version = "0.0.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
mx-gate = { path = "$ws/crates/mx-gate" }
mx-receipt = { path = "$ws/crates/mx-receipt" }
TOML
cp "$ws/Cargo.lock" "$scratch/Cargo.lock"

# name<TAB>code per example; the body goes to src/bin/<name>.rs
if ! list="$(python3 - "$ws" "$scratch" <<'PY'
import glob, re, sys
ws, scratch = sys.argv[1:3]
n = 0
for path in sorted(glob.glob(ws + "/crates/mx-gate/src/**/*.rs", recursive=True)):
    block, code = None, None
    for line in open(path, encoding="utf-8").read().split("\n"):
        m = re.match(r"\s*//[!/] ?(.*)$", line)
        text = m.group(1) if m else None
        if block is None:
            t = re.match(r"```compile_fail,(E\d{4})\s*$", text or "")
            if t:
                block, code = [], t.group(1)
        elif text is not None and text.strip() == "```":
            n += 1
            name = "ex%d" % n
            body = "\n".join(block)
            open("%s/src/bin/%s.rs" % (scratch, name), "w").write("fn main() {\n%s\n}\n" % body)
            print("%s\t%s\t%s" % (name, code, path.split("/crates/")[1]))
            block = None
        elif text is not None:
            block.append(text)
PY
)"; then echo "compile-fail: the extractor failed" >&2; exit 1; fi
[ -n "$list" ] || { echo "compile-fail: no example found, so nothing was checked" >&2; exit 1; }

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$scratch/target}" CARGO_TERM_COLOR=never
n=0; failed=0
while IFS=$'\t' read -r name want where; do
    n=$((n+1))
    out="$(cd "$scratch" && cargo check --offline --bin "$name" 2>&1)"; rc=$?
    got="$(grep -oE '^error\[E[0-9]{4}\]' <<<"$out" | sort -u | tr -d '\n')"
    if [ "$rc" -ne 0 ] && [ "$got" = "error[$want]" ]; then
        printf 'compile-fail: ok   %s %s (%s)\n' "$name" "$want" "$where"
    else
        failed=$((failed+1))
        printf 'compile-fail: FAIL %s %s (%s): rc=%s codes=%s\n' "$name" "$want" "$where" "$rc" "${got:-none}"
        [ "$rc" -ne 0 ] || printf '%s\n' "$out" | tail -5
    fi
done <<<"$list"
printf 'compile-fail: %s examples, %s failed\n' "$n" "$failed"
[ "$n" -gt 0 ] && [ "$failed" -eq 0 ]
