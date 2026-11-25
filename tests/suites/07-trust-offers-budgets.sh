#!/usr/bin/env bash
# Part of the offline suite: receipt writers, symlinks, extraction, budgets, digests and the one-offer rule.
# tests/run-tests.sh sources this file in one shell, after its own preamble and after
# the parts before it, so every helper and variable it reads comes from there. It is
# sourced, never run on its own, and carries a shebang so the shell sweeps pick it up.

# The parts share one shell. These names are set by the driver or by an earlier part;
# declare only tells the linter they are read here, and assigns and changes nothing.
declare mg
declare root
declare stub_dir

echo "== a receipt cannot be written by anything but the gate =="
# It used to be an unsigned JSON file whose only protection was a deny rule on
# one CLI verb, while the deny wall emits only Bash() and Read() rules. An agent
# with an ordinary write tool could create the file by hand, with
# "kind": "observed", and merge without ever running a mutation. That is
# SECURITY.md's own first named threat.
rk="$stub_dir/rkey"; printf 'deadbeefcafe\n' > "$rk"
forge="$stub_dir/forged"; mkdir -p "$forge/receipts"
make_stub gh 'case "$*" in *"api user"*) echo testuser;; esac; exit 0'
printf '{"pr":1,"head":"abcdef1234567890","kind":"observed","proof":"written by hand"}\n' \
    > "$forge/receipts/1.json"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_RECEIPT_KEY="$rk" MAINTAINER_STATE="$forge" \
      MAINTAINER_ACCOUNT=testuser MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp PROD_GLOBS="bin/*" \
      bash "$mg" merge 1 2>&1); rc=$?
if [ "$rc" != 0 ] && grep -q 'carries no signature' <<<"$out"; then
    ok "a hand-written receipt is refused"
else
    bad "a hand-written receipt merged or failed for another reason (rc=$rc)"
fi
# And one with a signature that does not match the payload.
printf '{"pr":2,"head":"abcdef1234567890","kind":"observed","proof":"edited","signature":"00"}\n' \
    > "$forge/receipts/2.json"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_RECEIPT_KEY="$rk" MAINTAINER_STATE="$forge" \
      MAINTAINER_ACCOUNT=testuser MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp PROD_GLOBS="bin/*" \
      bash "$mg" merge 2 2>&1)
grep -q 'does not match its signature' <<<"$out" \
    && ok "a receipt edited after signing is refused" \
    || bad "an edited receipt was accepted"

echo "== an unattended run cannot assert; it can only observe =="
# Signing alone does not separate an agent from a human: both run as this user,
# so both can reach the key. What separates them is that `verify` records
# something re-checkable and `receipt` records a sentence. So a run may not use
# `receipt`, and may not merge on what `receipt` writes. Three layers, and only
# the third is un-evadable by an agent that edits its own environment.
out=$(PATH="$stub_dir:$PATH" MAINTAINER_IN_RUN="sysknife/review" MAINTAINER_STATE="$forge" \
      MAINTAINER_ACCOUNT=testuser MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      bash "$mg" receipt 3 abcdef1234567890 'I promise' 2>&1); rc=$?
if [ "$rc" != 0 ] && grep -q 'may not assert a receipt' <<<"$out"; then
    ok "a run is refused the receipt command"
else
    bad "a run wrote an asserted receipt (rc=$rc)"
fi
[ -f "$forge/receipts/3.json" ] && bad "the refusal still left a receipt on disk" \
    || ok "the refusal wrote nothing"
# Layer 2: the deny wall spells the command, in every profile.
for pdj in "$root"/profiles/*/deny.json; do
    pn="$(basename "$(dirname "$pdj")")"
    grep -q 'maintainer-merge receipt' "$pdj" \
        && ok "$pn denies the receipt command to the agent" \
        || bad "$pn lets an agent run maintainer-merge receipt"
done
# Layer 3, the one that holds when the other two are evaded: a properly signed
# asserted receipt, written outside a run, is still refused inside one.
PATH="$stub_dir:$PATH" MAINTAINER_STATE="$forge" MAINTAINER_ACCOUNT=testuser \
    MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
    bash "$mg" receipt 4 abcdef1234567890 'proved by hand' >/dev/null 2>&1
if [ -f "$forge/receipts/4.json" ]; then
    ok "outside a run the receipt command still works"
    python3 -c "import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d.get('signature') and d['kind']=='asserted' else 1)" \
        "$forge/receipts/4.json" && ok "and what it writes is signed and marked asserted" \
        || bad "the asserted receipt is unsigned or mislabelled"
    out=$(PATH="$stub_dir:$PATH" MAINTAINER_IN_RUN="sysknife/review" MAINTAINER_STATE="$forge" \
          MAINTAINER_ACCOUNT=testuser MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp PROD_GLOBS="bin/*" \
          bash "$mg" merge 4 2>&1)
    grep -q "is 'asserted', not 'observed'" <<<"$out" \
        && ok "a run may not merge on an asserted receipt" \
        || bad "a run merged on a claim nothing checked"
else
    bad "the receipt command wrote nothing outside a run"
fi
# A proof string is a human sentence, so it contains quotes. The heredoc that
# used to write this file interpolated it raw and produced JSON merge could not
# parse, and the refusal then blamed the receipt rather than the quoting.
PATH="$stub_dir:$PATH" MAINTAINER_STATE="$forge" MAINTAINER_ACCOUNT=testuser \
    MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
    bash "$mg" receipt 5 abcdef1234567890 'reverting `|| true` leaves it "green": 0 bytes' >/dev/null 2>&1
python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$forge/receipts/5.json" 2>/dev/null \
    && ok "a proof containing quotes and backticks still writes valid JSON" \
    || bad "a quoted proof string produced an unparseable receipt"

# The key must be denied to readers, in every profile.
for pdj in "$root"/profiles/*/deny.json; do
    pn="$(basename "$(dirname "$pdj")")"
    grep -q 'receipt.key' "$pdj" && ok "$pn denies the receipt key to readers" \
        || bad "$pn leaves the receipt-signing key readable"
done

echo "== a symlink may not leave the tree the gate edits =="
# The mutation step runs on the HOST, outside the container. A tracked symlink
# harness.sh -> ~/.ssh/id_ed25519 survives git archive, and `sed -i` follows it:
# the key's plaintext lands in the extracted tree, which is then bind-mounted
# into the container where the pull request's own test can print it, and 200
# characters of it land in the receipt. Every container flag is irrelevant,
# because the read happens before the container exists.
# Driven through the real function rather than grepped out of the source. The
# first two cases here were `grep -q 'ships symlink' "$mg"`, which passes for any
# file that contains the words and says nothing about what the gate does.
# One helper for every call into maintainer-merge's own functions. $mg is
# resolved at runtime, which shellcheck cannot follow, so the directive lives
# here once instead of above each of the five call sites.
# shellcheck disable=SC1090
mgfn() { local fn="$1"; shift; ( . "$mg" >/dev/null 2>&1; "$fn" "$@" ) 2>/dev/null; }
esc() { mgfn tree_escapes "$1"; }
tr1="$stub_dir/tree1"; mkdir -p "$tr1/docs/images" "$tr1/assets"
printf 'png\n' > "$tr1/assets/social.png"
ln -sf ../../assets/social.png "$tr1/docs/images/social.png"
if sc_case 4; then
[ -z "$(esc "$tr1")" ] && ok "an in-tree relative symlink is allowed" \
    || bad "a repository's own docs symlink is refused: $(esc "$tr1")"
outside="$stub_dir/outside-key"; printf 'CANARY\n' > "$outside"
ln -sf "$outside" "$tr1/harness.sh"
printf '%s' "$(esc "$tr1")" | grep -q 'harness.sh' \
    && ok "a symlink to an absolute path outside the tree is reported" \
    || bad "an escaping symlink was not reported"
rm -f "$tr1/harness.sh"
ln -sf ../../../../etc/passwd "$tr1/docs/images/trav.sh"
printf '%s' "$(esc "$tr1")" | grep -q 'trav.sh' \
    && ok "a ../ traversal out of the tree is reported" \
    || bad "a traversing symlink was not reported"
rm -f "$tr1/docs/images/trav.sh"
[ -z "$(esc "$tr1")" ] && ok "and the tree reads clean once it is removed" \
    || bad "the scan reports an escape that is no longer there"
sc_end; fi
# Driven, not grepped. The grep this replaces matched the literal string
# `find . -type f -name`, so widening the mutation to several globs broke the
# test while the property it guards was untouched: a check that reads the source
# instead of the behaviour fails on a refactor and passes on a regression.
if sc_case 4; then
mt="$stub_dir/mutatetree"; rm -rf "$mt"; mkdir -p "$mt/sub"
printf 'GUARD=on\n' > "$mt/real.sh"
printf 'GUARD=on\n' > "$mt/helper.py"
printf 'GUARD=on\n' > "$mt/sysknife-thing"
printf 'GUARD=on\n' > "$mt/untouched.md"
printf 'GUARD=on\n' > "$stub_dir/mutate-target-outside"
ln -sf "$stub_dir/mutate-target-outside" "$mt/link.sh"
( # shellcheck disable=SC1090
  . "$mg" 2>/dev/null
  suite_mutate_glob() { printf '%s\n' '*.sh' '*.py' 'sysknife-*'; }
  apply_mutation "$mt" 's/GUARD=on/GUARD=off/' ) >/dev/null 2>&1
grep -q 'GUARD=off' "$mt/real.sh" \
    && ok "the mutation reaches a file matching the first glob" \
    || bad "the mutation did not apply at all"
grep -q 'GUARD=off' "$mt/helper.py" && grep -q 'GUARD=off' "$mt/sysknife-thing" \
    && ok "and files matching the second and third globs, so one suffix is not assumed" \
    || bad "only the first glob was applied; a multi-glob suite mutates nothing else"
grep -q 'GUARD=on' "$mt/untouched.md" \
    && ok "and nothing outside the declared globs is touched" \
    || bad "the mutation reached a file no glob names"
[ -L "$mt/link.sh" ] && grep -q 'GUARD=on' "$stub_dir/mutate-target-outside" \
    && ok "and a symlink is neither followed nor replaced with its content" \
    || bad "the mutation step wrote through a symlink, out of the tree"
sc_end; fi
# Demonstrated: sed -i through a symlink materialises the target.
sl="$stub_dir/symlinkdemo"; mkdir -p "$sl"; secret="$stub_dir/fake-key"
printf 'CANARY-KEY-MATERIAL\n' > "$secret"; ln -sf "$secret" "$sl/evil.sh"
( cd "$sl" && find . -type f -name '*.sh' -print0 | xargs -0 -r sed -i 's/x/y/' )
grep -q CANARY "$sl/evil.sh" && [ -L "$sl/evil.sh" ] \
    && ok "with -type f the symlink is left alone" \
    || bad "the symlink was dereferenced even with -type f"

echo "== the pull request's tree is extracted without tar =="
# The timer unit sets RestrictSUIDSGID=yes. Its seccomp filter cannot read the
# struct openat2 takes and answers ENOSYS, and tar 1.35 creates every file
# through openat2, so under the unit `git archive | tar -x` wrote nothing:
#     tar: .github/FUNDING.yml: Cannot open: Function not implemented
# The pipe dropped tar's status, the suite then failed with a message about the
# container's network, and no unattended run could verify or merge for five days
# (2026-10-29 to 11-02, magent#158). A tar that fails the way the unit's did
# stands in for the sandbox here, because CI has no user session to run it in;
# `maintainer-doctor` probes the real hardening on the machine that has one.
xt="$stub_dir/xtreerepo"; rm -rf "$xt"; mkdir -p "$xt/sub/deep"
git -C "$xt" init -q -b main; git -C "$xt" config user.email t@t; git -C "$xt" config user.name t
printf 'one\n' > "$xt/a.txt"; printf 'two\n' > "$xt/sub/deep/b.txt"
printf '#!/bin/sh\n' > "$xt/run.sh"; chmod +x "$xt/run.sh"; ln -s a.txt "$xt/link"
git -C "$xt" add -A; git -C "$xt" commit -qm tree
git -C "$xt" update-index --add --cacheinfo "160000,$(git -C "$xt" rev-parse HEAD),vendor/mod"
git -C "$xt" commit -qm submodule
xt_et="$(git -C "$xt" hash-object -w -t tree /dev/null)"
xt_empty="$(git -C "$xt" commit-tree "$xt_et" -m empty)"
notar="$stub_dir/notar"; mkdir -p "$notar"
cat > "$notar/tar" <<'TAREOF'
#!/bin/sh
: > "$XT_MARK"
echo "tar: x: Cannot open: Function not implemented" >&2
exit 2
TAREOF
chmod +x "$notar/tar"
xtree() { # $1 ref, $2 dir: the real extract_tree, tar broken, stderr kept
    ( export MAINTAINER_REPO="$xt" MAINTAINER_STATE="$stub_dir/xtstate" XT_MARK="$stub_dir/tar-called"
      # shellcheck disable=SC1090
      . "$mg" >/dev/null 2>&1; PATH="$notar:$PATH" extract_tree "$@" ) 2>&1
}
rm -f "$stub_dir/tar-called"
xo="$stub_dir/xt-out"; rm -rf "$xo"; mkdir -p "$xo"
if sc_case 8; then
xout="$(xtree HEAD "$xo")"; xrc=$?
[ "$xrc" -eq 0 ] && [ -z "$xout" ] \
    && ok "a tree extracts although tar answers ENOSYS" \
    || bad "extraction failed with tar broken (rc=$xrc): $(printf '%s' "$xout" | head -c 160)"
[ ! -e "$stub_dir/tar-called" ] \
    && ok "and tar was never run" \
    || bad "the extraction still calls tar, which the unit's seccomp filter breaks"
want_files="$(git -C "$xt" ls-tree -r --name-only HEAD | grep -vx 'vendor/mod' | sort)"
got_files="$(cd "$xo" && find . \( -type f -o -type l \) | sed 's#^\./##' | sort)"
[ -n "$want_files" ] && [ "$want_files" = "$got_files" ] \
    && ok "every tracked file arrives, a submodule excepted" \
    || bad "the extracted files differ from the tracked ones: $(printf '%s' "$got_files" | tr '\n' ' ')"
[ -x "$xo/run.sh" ] && [ "$(readlink "$xo/link")" = a.txt ] && [ -d "$xo/vendor/mod" ] \
    && ok "the exec bit, a symlink and a submodule directory survive" \
    || bad "a mode, a link or a gitlink was lost in extraction"
# Each refusal below is its own guard, and each case is shaped so that only that
# guard stands between the input and a clean exit.
xo2="$stub_dir/xt-out2"; rm -rf "$xo2"; mkdir -p "$xo2"; printf 'stray\n' > "$xo2/stray.txt"
xout="$(xtree HEAD "$xo2")"; xrc=$?
[ "$xrc" -ne 0 ] && grep -q 'extracted' <<<"$xout" \
    && ok "a directory holding a file the tree does not track is refused" \
    || bad "an extra file in the target passed the count check (rc=$xrc)"
xo3="$stub_dir/xt-out3"; rm -rf "$xo3"; mkdir -p "$xo3"; printf 'mine\n' > "$xo3/a.txt"
xout="$(xtree HEAD "$xo3")"; xrc=$?
[ "$xrc" -ne 0 ] \
    && ok "a file that checkout-index will not overwrite is refused, not skipped" \
    || bad "checkout-index failed and the extraction still reported success"
xo4="$stub_dir/xt-out4"; rm -rf "$xo4"; mkdir -p "$xo4"
xout="$(xtree no-such-ref "$xo4")"; xrc=$?
[ "$xrc" -ne 0 ] && [ -z "$(ls -A "$xo4")" ] \
    && ok "an unknown ref is refused and writes nothing" \
    || bad "an unreadable ref was accepted (rc=$xrc)"
xo5="$stub_dir/xt-out5"; rm -rf "$xo5"; mkdir -p "$xo5"
xout="$(xtree "$xt_empty" "$xo5")"; xrc=$?
[ "$xrc" -ne 0 ] && grep -q 'lists no files' <<<"$xout" \
    && ok "a ref with no files is refused, since zero of zero is not an extraction" \
    || bad "an empty tree was accepted (rc=$xrc)"
sc_end; fi

echo "== doctor probes extraction under the unit's own hardening =="
# Driven against a stand-in for systemd-run: a control call with no properties
# succeeds, and under SR_MODE=blocked any call that carries one fails the way the
# real unit did. A probe that never applied the directives would pass both runs.
doc="$root/bin/maintainer-doctor"
sr="$stub_dir/sysrun"; mkdir -p "$sr"; ln -sf "$mg" "$sr/maintainer-merge"
cat > "$sr/systemd-run" <<'SREOF'
#!/usr/bin/env bash
props=0; envs=()
while [ $# -gt 0 ]; do
    case "$1" in
        -p) props=$((props+1)); shift 2 ;;
        -E) envs+=("$2"); shift 2 ;;
        --*) shift ;;
        *) break ;;
    esac
done
if [ "${SR_MODE:-}" = blocked ] && [ "$props" -gt 0 ]; then
    echo "tar: f: Cannot open: Function not implemented" >&2; exit 2
fi
exec env ${envs[@]+"${envs[@]}"} "$@"
SREOF
chmod +x "$sr/systemd-run"
sbh="$stub_dir/sandboxhome"; rm -rf "$sbh"; mkdir -p "$sbh/.local/share/maintainer/profiles/p" "$sbh/.config/systemd/user" "$sbh/state/state" "$sbh/state/runs"
printf 'PROFILE_NAME=p\nTASKS="review"\nSTATE_DIR=%s\nREPO_PATH=%s\n' "$sbh/state" "$sbh" \
    > "$sbh/.local/share/maintainer/profiles/p/profile.env"
printf '[Service]\nNoNewPrivileges=yes\nRestrictSUIDSGID=yes\nExecStart=/bin/true\n' \
    > "$sbh/.config/systemd/user/maintainer@.service"
sbout="$(PATH="$sr:$stub_dir:$PATH" HOME="$sbh" bash "$doc" --quick 2>&1 || true)"
grep -q 'verify extracts a tree under the unit.s hardening (2 directive' <<<"$sbout" \
    && ok "doctor reports a working extraction and counts the directives it applied" \
    || bad "the sandbox probe did not pass on a sandbox that allows extraction: $(grep -a -A2 Sandbox <<<"$sbout" | tr '\n' ' ' | cut -c1-200)"
sbout="$(SR_MODE=blocked PATH="$sr:$stub_dir:$PATH" HOME="$sbh" bash "$doc" --quick 2>&1 || true)"
grep -aq 'FAIL.*verify cannot extract a tree under the unit.s hardening' <<<"$sbout" \
    && ok "and goes red when the hardening breaks it" \
    || bad "a sandbox that blocks extraction was reported as fine"
printf '[Service]\nExecStart=/bin/true\n' > "$sbh/.config/systemd/user/maintainer@.service"
sbout="$(SR_MODE=blocked PATH="$sr:$stub_dir:$PATH" HOME="$sbh" bash "$doc" --quick 2>&1 || true)"
grep -aq 'warn.*no hardening directive found' <<<"$sbout" \
    && ok "a unit with no directives is a warning, not a pass" \
    || bad "a probe with nothing to apply reported success"

echo "== the shellcheck sweep runs locally, or says it did not =="
# It lived inline in ci.yml, so the pre-commit hook could not run it and a push
# was the first thing to report SC1090 on four lines that had passed every gate
# this machine knows about. A local-first project whose linter is CI-only is not
# local-first.
sw="$root/scripts/shellcheck-sweep.sh"
[ -x "$sw" ] && ok "the sweep is a script, not a workflow step" \
    || bad "the shellcheck sweep is not runnable outside CI"
if sc_case 1; then
grep -q 'shellcheck-sweep.sh' "$root/.githooks/pre-commit" \
    && ok "the pre-commit hook runs the same sweep" \
    || bad "the local gate still skips shellcheck"
sc_end; fi
grep -q 'shellcheck-sweep.sh' "$root/.github/workflows/ci.yml" \
    && ok "CI runs the same sweep, not a second copy of it" \
    || bad "CI has its own copy of the sweep, which will drift"
# And it must fail when the linter is absent. A gate that reports success over
# code nothing inspected is the shape docs/lessons.md keeps recording.
# A PATH with everything the sweep needs EXCEPT shellcheck. Stripping PATH
# entirely proves nothing: the shell then cannot find `bash` either.
noshell="$stub_dir/nolinter"; mkdir -p "$noshell"
for t in git head tr sort bash readlink dirname grep sed; do
    src="$(command -v "$t" 2>/dev/null)" && ln -sf "$src" "$noshell/$t"
done
out=$(PATH="$noshell" bash "$sw" 2>&1); rc=$?
[ "$rc" = 127 ] && ok "with no shellcheck on PATH the sweep exits 127" \
    || bad "the sweep reported rc=$rc with no linter installed"
grep -q 'inspected nothing' <<<"$out" \
    && ok "and it says the gate inspected nothing" \
    || bad "the sweep failed silently: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-80)"

echo "== the container runtime's own noise is not evidence =="
# Measured on sysknife#365: podman writes
#   time="..." level=warning msg="Error validating CNI config file ..."
# to the same stderr the container uses, on every run. It became the receipt's
# `observed_failure`, and it is enough bytes on its own to satisfy the shell
# suite's proof that the test ran at all.
noise="$stub_dir/noise.log"
{ printf 'time="2026-09-04T06:43:41-06:00" level=warning msg="Error validating CNI config file"\n'
  printf 'time="2026-09-04T06:43:41-06:00" level=error msg="failed to find plugin bridge"\n'
  printf 'FAIL  story-7.sh header claims story 97\n'
  printf 'some later error\n'; } > "$noise"
if sc_case 4; then
ffl="$(mgfn first_failure_line "$noise")"
[ "$ffl" = "FAIL  story-7.sh header claims story 97" ] \
    && ok "the runtime's logfmt is skipped and the test's own line is taken" \
    || bad "the receipt would record '$ffl'"
printf 'time="x" level=warning msg="Error validating CNI config"\n' > "$noise"
[ -z "$(mgfn first_failure_line "$noise")" ] \
    && ok "a log that is only runtime noise yields no observed failure" \
    || bad "runtime noise alone was recorded as the observed failure"
# A failure that matches no Rust-shaped keyword still has to leave evidence.
# sysknife's prose claim screen reports a mismatched figure in plain English, and
# the receipt for #366 recorded an empty observed_failure the first time.
{ printf 'Published figures match the evidence artifacts.\n'
  printf 'README.md claims 9,999 Rust tests; the artifact records 1,837.\n'; } > "$noise"
[ "$(mgfn first_failure_line "$noise")" = \
  "README.md claims 9,999 Rust tests; the artifact records 1,837." ] \
    && ok "a plain-English failure falls back to the last line printed" \
    || bad "a failure with no Rust keyword left the receipt with no evidence"
grep -q '"\$rt" --log-level=error run' "$mg" \
    && ok "the runtime is told to keep its warnings out of the log" \
    || bad "runtime warnings still land in the log the byte count reads"
sc_end; fi

echo "== every profile declares a budget for every task it runs =="
# The template grew THINKING_<task> after profiles/magent had been generated
# from it, so magent ran without one and the agent maintaining the merge gate
# thought less about it than the agent maintaining the target repository did.
# Nothing said so: a missing budget is not an error, it is a default.
for pe in "$root"/profiles/*/profile.env; do
    pn="$(basename "$(dirname "$pe")")"
    tasks="$(sed -n 's/^TASKS="\(.*\)"/\1/p' "$pe")"
    [ -n "$tasks" ] || { bad "$pn declares no TASKS"; continue; }
    missing=""
    for tk in $tasks; do
        grep -q "^THINKING_$tk=" "$pe" || missing="$missing $tk"
        grep -q "^MODEL_$tk=" "$pe" || missing="$missing $tk(model)"
    done
    [ -z "$missing" ] && ok "$pn budgets every task it runs" \
        || bad "$pn runs$missing with no declared thinking budget or model"
done

echo "== what a run cost is recorded, not estimated =="
# The first version of this logged `in=<input_tokens>` and nothing else, so a
# full review of a Rust workspace was recorded as "in=48". Almost every input
# token in an agent run is a cache read or a cache write, and both are separate
# fields, so 48 was true and useless. It read as a measurement, which is worse
# than printing nothing.
#
# Not tiktoken. It is OpenAI's tokenizer, it undercounts Claude by 15-20% on
# prose and by more on code, and Anthropic publishes none. Counting is the wrong
# move anyway: the exact figures are already in the stream Claude Code emits.
tw="$stub_dir/usagerun"; mkdir -p "$tw"
cat > "$tw/ev.jsonl" <<'EVENTS'
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}
{"type":"result","subtype":"success","is_error":false,"num_turns":42,"duration_ms":1085000,"total_cost_usd":4.486383,"result":"done","usage":{"input_tokens":98,"output_tokens":38343,"cache_creation_input_tokens":412000,"cache_read_input_tokens":9800000},"modelUsage":{"opus":{"inputTokens":98,"outputTokens":38343,"cacheCreationInputTokens":412000,"cacheReadInputTokens":9800000,"costUSD":4.30},"haiku":{"inputTokens":10,"outputTokens":900,"cacheCreationInputTokens":0,"cacheReadInputTokens":52000,"costUSD":0.186383}}}
EVENTS
python3 "$root/scripts/transcript.py" "$tw/r.log" "$tw/r.commands" < "$tw/ev.jsonl"
[ -f "$tw/r.usage.json" ] && ok "the transcript writes a usage record beside the log" \
    || bad "nothing records what the run spent"
python3 - "$tw/r.usage.json" <<'PYEOF'
import json, sys
d = json.load(open(sys.argv[1])); t = d["totals"]
fail = []
if t["cache_read_input_tokens"] != 9852000:
    fail.append(f"cache reads not summed across models: {t['cache_read_input_tokens']}")
if t["cache_creation_input_tokens"] != 412000:
    fail.append("cache writes missing")
if t["input_tokens"] == 98:
    fail.append("only the main loop counted; subagent models are excluded from usage")
if t["billable_input_tokens"] <= t["input_tokens"]:
    fail.append("billable input ignores the cache fields, which is the in=48 bug")
if d["subtype"] != "success" or d["num_turns"] != 42:
    fail.append("the result subtype or turn count was dropped")
sys.exit("; ".join(fail) if fail else 0)
PYEOF
[ $? = 0 ] && ok "cache reads, cache writes and every model are in the totals" \
    || bad "the usage record repeats the in=48 mistake"
# An error result carries usage too. A failed run that spent four dollars must
# not be recorded as having spent nothing.
printf '%s\n' '{"type":"result","subtype":"error_max_budget_usd","is_error":true,"total_cost_usd":2.5,"usage":{"input_tokens":5,"output_tokens":10,"cache_read_input_tokens":900}}' \
    > "$tw/err.jsonl"
python3 "$root/scripts/transcript.py" "$tw/e.log" "$tw/e.commands" < "$tw/err.jsonl"
python3 -c "
import json,sys
d=json.load(open('$tw/e.usage.json'))
sys.exit(0 if d['is_error'] and d['cost_usd_estimate']==2.5 and d['subtype']=='error_max_budget_usd' else 1)" \
    && ok "a failed run still records what it spent" \
    || bad "a run that errored was recorded as having spent nothing"

echo "== the digest reads THIS run's files, and says so when they are absent =="
# Same shape as lessons.md 30, for the third time: the first version took the
# newest *.commands in the directory and printed another run's command count
# under this one.
dg="$stub_dir/dgstate"; mkdir -p "$dg/runs" "$dg/logs" "$dg/state"
printf '# report\nline two\n' > "$dg/runs/2026-01-01T00-00-review.md"
printf '$ a\n$ b\n$ c\n' > "$dg/logs/2026-01-01T00-00-review.commands"
printf '$ x\n' > "$dg/logs/2026-09-09T09-09-review.commands"
cp "$tw/r.usage.json" "$dg/logs/2026-01-01T00-00-review.usage.json"
dgrun() { MAINTAINER_STATE="$dg" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
    MAINTAINER_ACCOUNT=t MAINTAINER_PROFILE=dg python3 "$root/bin/maintainer" digest "$@" 2>&1; }
out="$(dgrun 2026-01-01T00-00-review --compact)"
grep -q '3 commands run' <<<"$out" \
    && ok "the digest counts this run's commands" \
    || bad "the digest read the wrong commands file: $(printf '%s' "$out" | tr '\n' ' ')"
grep -q '4.49' <<<"$out" \
    && ok "and reports the run's cost estimate" || bad "the cost is missing from the digest"
grep -qE 'cache read 9.9M' <<<"$out" \
    && ok "in units a person reads, not 9852000" || bad "raw token counts reached the digest"
# A run with no usage record must say unknown, never zero.
printf '# r\n' > "$dg/runs/2026-02-02T00-00-review.md"
out="$(dgrun 2026-02-02T00-00-review --compact)"
grep -q 'spend unknown' <<<"$out" \
    && ok "a run with no usage record reports unknown, not zero" \
    || bad "an unrecorded cost was printed as a number: $(printf '%s' "$out" | tr '\n' ' ')"
grep -q 'command record missing' <<<"$out" \
    && ok "and says its command record is missing rather than borrowing one" \
    || bad "the digest borrowed another run's command count"

echo "== a notification is written for a person =="
# What used to reach the desktop was the raw shell error, absolute path and all,
# with no next step: "backend claude unusable: missing /home/.../settings.json".
# And nothing was sent at all when a run SUCCEEDED, so the agent only ever spoke
# to its owner to complain.
if sc_case 5; then
grep -q 'humanise()' "$root/lib/run.sh" && ok "failures are translated before they are shown" \
    || bad "the raw shell error still goes straight to the desktop"
grep -q 'notify normal' "$root/lib/run.sh" \
    && ok "a successful run notifies too" \
    || bad "the agent only speaks when it fails"
grep -q 'HELPER" digest' "$root/lib/run.sh" \
    && ok "and what it says is the measured digest" \
    || bad "the success notification is not built from the run's own record"
# The last-resort alert must leave a trace on disk. A popup at 03:00 is gone by
# morning, and it was the only record.
grep -q 'alerts.log' "$root/platform/linux/alert.sh" \
    && ok "the systemd OnFailure alert writes to the state directory" \
    || bad "a failure nobody was awake for leaves no trace"
grep -q 'sort -u' "$root/platform/linux/alert.sh" \
    && ok "and writes it once, not once per matching glob" \
    || bad "both state globs match the same directory, so alerts are double-counted"
sc_end; fi
grep -q 'ExecStart=%h/.local/share/maintainer/alert.sh' "$root/platform/linux/maintainer-alert@.service" \
    && ok "the unit calls a script rather than quoting shell into ExecStart" \
    || bad "systemd refuses the inline version with Unbalanced quoting"
if sc_case 1; then
grep -q 'alert.sh' "$root/install.sh" \
    && ok "install.sh deploys it, so the unit's ExecStart resolves" \
    || bad "the alert unit points at a file the installer never copies"
sc_end; fi

echo "== the one-offer rule is arithmetic, not a sentence in a skill file =="
# The rule exists because it was measured: of thirteen offers, five people got
# two each and every one answered exactly one. It lived as prose, so nothing
# enforced it, and on 2026-09-03 a single run made three offers to one person.
ofd="$stub_dir/offers"; mkdir -p "$ofd"
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'issue list'*) cat <<'J'
[{"number":10,"labels":[]},{"number":11,"labels":[]},{"number":12,"labels":[]},
 {"number":13,"labels":[{"name":"twir-listed"}]},{"number":14,"labels":[]}]
J
    ;;
  *issues/10/comments*) echo '[{"u":"owner","b":"@greedy this one is yours"}]';;
  *issues/11/comments*) echo '[{"u":"owner","b":"@greedy and this one"}]';;
  *issues/12/comments*) echo '[{"u":"owner","b":"@busy yours"},{"u":"busy","b":"on it"}]';;
  *issues/13/comments*) echo '[]';;
  *issues/14/comments*) echo '[{"u":"owner","b":"@gone yours"},{"u":"owner","b":"@gone releasing this <!-- maintainer: claim-released -->"}]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of \
      python3 "$root/bin/maintainer" offers 2>&1)
grep -q 'OVER-OFFERED: 2 unanswered' <<<"$out" \
    && ok "two unanswered offers to one person is reported as a violation" \
    || bad "the one-offer rule is still unenforced: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90)"
grep -qE 'busy .*working on #12' <<<"$out" \
    && ok "an ANSWERED open issue counts as a full queue, not as availability" \
    || bad "somebody mid-task reads as eligible for another"
grep -qE '[0-9]+ issue\(s\) free to offer' <<<"$out" \
    && ok "a reserved issue is not counted as free" \
    || bad "twir-listed or maintainer-only issues were offered as free"
grep -q 'holds nothing does not' <<<"$out" \
    && ok "and it says what an eligible person looks like" \
    || bad "an empty list of blocked people reads as nobody being available"
# A release and an offer are the same sentence shape to the same person. Reading
# any @mention as an offer made a claim released minutes earlier read as
# "working on #272" and held the issue out of the free pool. Found by running
# this against a release posted an hour after the tool shipped.
grep -q '^gone ' <<<"$out" \
    && bad "a released claim still reads as held" \
    || ok "a released claim frees the person"
# #14 is the only free one: #10 and #11 hold unanswered offers, #12 is answered
# and in progress, #13 is reserved. It is free only because its mentions carry
# the release marker, so this number IS the assertion.
grep -q '1 issue(s) free to offer' <<<"$out" \
    && ok "and puts the issue back in the free pool" \
    || bad "the released issue is still counted as spoken for: $(printf '%s' "$out" | head -1)"

echo "== an action pin or a git ref is not a GitHub handle =="
# `maintainer offers` read every @token in issue text as a person. #369 quoted
# `dependency-review-action@<40-hex>`, `trufflehog@<40-hex>` and
# `dtolnay/rust-toolchain@stable`, so the issue counted as offered to three
# accounts that do not exist and dropped out of the free-to-offer pool while no
# real person was ever offered it. A real mention sits at a word boundary; a pin
# or ref has an alphanumeric immediately before the @.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'issue list'*) echo '[{"number":20,"labels":[]}]';;
  *issues/20/comments*) cat <<'J'
[{"u":"owner","b":"Bumped the pins. `actions/dependency-review-action@a1d282b36b6f3519aa1f3fc636f609c47dddb294` and dtolnay/rust-toolchain@stable stay pinned. @be-student can you take this one?"}]
J
    ;;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of \
      python3 "$root/bin/maintainer" offers 2>&1)
grep -qE '^be-student ' <<<"$out" \
    && ok "a real @mention beside the pins is still read" \
    || bad "the real handle was lost: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90)"
grep -qiE '^stable |^a1d282|^05a583' <<<"$out" \
    && bad "an action pin or git ref was counted as a person: $(grep -iE 'stable|a1d282|05a583' <<<"$out" | head -1)" \
    || ok "action pins and git refs do not become phantom people"
grep -q '1 issue(s) free to offer' <<<"$out" \
    && bad "the issue is free, but a phantom mention held it out of the pool" \
    || ok "the issue with one real unanswered offer is not counted free"

echo "== a free issue is named, and one issue offered twice is a collision =="
# The campaign on 2026-09-07 offered #267 to QinXi-ai while VedantMadane had
# held an unanswered offer on it since 2026-09-01. Two people were aimed at one
# issue and neither was told. `offers` knew #267 was taken and printed only a
# COUNT of free issues, so choosing from its output was impossible. It now names
# them, and names any issue pointed at two people.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'issue list'*) echo '[{"number":30,"labels":[]},{"number":31,"labels":[]},{"number":32,"labels":[]}]';;
  *issues/30/comments*) echo '[]';;
  *issues/31/comments*) echo '[{"u":"owner","b":"@ana this one is yours"},{"u":"owner","b":"@bruno reserved for you as a follow-up"}]';;
  *issues/32/comments*) echo '[{"u":"owner","b":"@ana yours"},{"u":"ana","b":"on it"}]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of \
      python3 "$root/bin/maintainer" offers 2>&1)
grep -qE 'free to offer:.*#30' <<<"$out" \
    && ok "the free issue is named, so a campaign can pick from the output" \
    || bad "offers still prints only a count, so nothing can choose from it"
grep -q 'DOUBLE-BOOKED' <<<"$out" \
    && ok "one issue offered to two people is reported as a collision" \
    || bad "#31 is pointed at two people and offers reports no collision"
grep -qE 'DOUBLE-BOOKED.*#31' <<<"$out" \
    && ok "and the collision names the issue" \
    || bad "the collision does not name which issue is double-booked"


echo "== one comment naming two people on an unassigned issue is ambiguous, not two offers =="
# My own prose caused this. Offering sysknife#406 to @ITSMERNB I wrote "On 6
# September I offered it to @be-student, who took it and opened #377", talking
# about a DIFFERENT issue. `offers` read both handles as holders of #406 and
# would have reported a double-booking. The assignee rule happened to mask it,
# which is luck: on an unassigned issue nothing would have.
#
# Prose cannot be parsed for intent. Saying "I cannot tell" is honest and cheap;
# recording a hold nobody made is neither.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'pr list'*'--state merged'*) echo '[{"author":{"login":"vet"},"mergedAt":"2026-09-09T00:00:00Z"}]';;
  *'pr list'*) echo '[]';;
  *'issue list'*) echo '[{"number":70,"labels":[],"assignees":[]},{"number":71,"labels":[],"assignees":[{"login":"clear"}]}]';;
  *issues/70/comments*) echo '[{"u":"owner","b":"@newbie this one is yours. I offered the last one to @veteran, who took it."}]';;
  *issues/71/comments*) echo '[{"u":"owner","b":"@clear yours, and I mentioned @someone else here too"}]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of \
      python3 "$root/bin/maintainer" offers 2>&1)
grep -qiE 'AMBIGUOUS #70|cannot tell.*#70|#70.*two people' <<<"$out" \
    && ok "one comment naming two people on an unassigned issue is reported as ambiguous" \
    || bad "the tool silently recorded two holders from one sentence: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-100)"
# The assigned twin. #71 names two people too, but GitHub says who has it, so
# there is nothing to be unsure about and no noise is worth printing.
grep -qiE 'AMBIGUOUS #71' <<<"$out" \
    && bad "an assigned issue was called ambiguous, which makes the warning noise" \
    || ok "an assigned issue is not ambiguous, because the assignee settles it"

echo "== an ambiguous issue does not count against anybody it names =="
# The tool said "who holds it cannot be read from the thread" and then counted
# every name in that unreadable sentence as a live hold. Saying "I cannot tell"
# and then acting on a guess is worse than either one alone: the guess is
# invisible, and it lands on the people the sentence happened to name.
#
# sysknife 2026-09-22: @QinXi-ai was reported OVER-OFFERED with three unanswered
# offers. #428 and #433 were withdrawn from them in public comments that also
# named the contributor the issue went back to, and #460 named them only to say
# "read #458 first, @QinXi-ai has an offer out on it". All three were ambiguous
# by this tool's own reading, and all three counted. The campaign that followed
# would have skipped the one contributor it was told to offer work to.
#
# An ambiguous issue stays out of the free pool, because somebody does hold it.
# It attributes to nobody, because which body is the thing that cannot be read.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'pr list'*'--state merged'*) echo '[{"author":{"login":"vet"},"mergedAt":"2026-09-09T00:00:00Z"}]';;
  *'pr list'*) echo '[]';;
  *'issue list'*) echo '[{"number":70,"labels":[],"assignees":[]}]';;
  *issues/70/comments*) echo '[{"u":"owner","b":"@newbie this one is yours. I offered the last one to @veteran, who took it."}]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of \
      python3 "$root/bin/maintainer" offers 2>&1)
grep -qE '^(newbie|veteran) ' <<<"$out" \
    && bad "an ambiguous issue was still charged to somebody: $(grep -E '^(newbie|veteran) ' <<<"$out" | tr '\n' ' ')" \
    || ok "an ambiguous issue is charged to nobody, so neither name loses eligibility"
grep -q 'DOUBLE-BOOKED' <<<"$out" \
    && bad "one unreadable sentence was reported as a double-booking" \
    || ok "an ambiguous issue is not reported as a double-booking"
grep -qE 'free to offer.*#70|#70.*free' <<<"$out" \
    && bad "an ambiguous issue was offered as free, so it can be handed to a second person" \
    || ok "an ambiguous issue stays out of the free pool, because somebody does hold it"

echo "== a release comment clears the ambiguity it names somebody out of =="
# The AMBIGUOUS line used to say "say so in a comment naming only them", and a
# comment naming only them did nothing: `crowded` took the maximum across the
# whole thread, so a count that reached two could never come back down. The
# issue stayed ambiguous however many times I clarified it, which teaches the
# reader to skip the warning rather than act on it.
#
# Prose cannot settle it, and this tool says so two hundred lines up: a second
# single-name comment is as likely to be a second offer as a correction, and
# the double-booking case above depends on reading it that way. The marker is
# what settles it, because it is the one thing in a thread that carries intent
# the machine can read.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'pr list'*'--state merged'*) echo '[{"author":{"login":"vet"},"mergedAt":"2026-09-09T00:00:00Z"}]';;
  *'pr list'*) echo '[]';;
  *'issue list'*) echo '[{"number":72,"labels":[],"assignees":[]}]';;
  *issues/72/comments*) printf '%s' '[{"u":"owner","b":"@newbie this one is yours. I offered the last one to @veteran, who took it."},{"u":"owner","b":"@veteran releasing you from this one, it was never yours. <!-- maintainer: claim-released -->"}]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of \
      python3 "$root/bin/maintainer" offers 2>&1)
grep -q 'AMBIGUOUS #72' <<<"$out" \
    && bad "a release comment did not clear the ambiguity, so nothing a maintainer can write does" \
    || ok "a release comment clears the ambiguity, because the marker carries intent a machine can read"
grep -qE '^newbie +1' <<<"$out" \
    && ok "and the person who still holds it is charged with it" \
    || bad "the remaining holder is charged to nobody: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"
grep -qE '^veteran ' <<<"$out" \
    && bad "the released handle is still charged with the issue" \
    || ok "and the released handle is not charged with it"

echo "== offer posts on the issue's own thread, and refuses a double booking =="
# 2026-10-01, sysknife: three offers made in one day lived only in prose on a
# pull request's thread (#530 on #528, #474 on #514, #411 on #515). `holders`
# reads the issue's thread, so it saw none of them, and two of the three were
# offered again to somebody else within hours: #474 to a second contributor
# while the first still held it, and #411 five hours after its holder had
# accepted it. An offer has to be written where the tool reads, by the tool
# that knows who is already holding what.
offer_gh() {  # $1 = issue list JSON, $2 = #40's comments JSON, $3 = open PRs JSON or "FAIL"
    rm -f "$ofd/posted40"
    cat > "$stub_dir/gh" <<GHEOF
#!/usr/bin/env bash
case "\$*" in
  *'issue list'*) echo '$1';;
  *'pr list'*) [ '$3' = FAIL ] && exit 1; echo '$3';;
  *'-X POST'*issues/40/comments*) printf '%s\n' "\$*" > "$ofd/posted40"; echo '{}';;
  *issues/40/comments*'[.[].body]'*) [ -f "$ofd/posted40" ] && sed -n 's/.*body=//p' "$ofd/posted40" | python3 -c 'import json,sys;print(json.dumps([sys.stdin.read().strip()]))' || echo '[]';;
  *issues/40/comments*) echo '$2';;
  *issues/41/comments*) echo '[{"u":"owner","b":"@cara this one is yours"}]';;
  *issues/*/comments*) echo '[]';;
esac
GHEOF
    chmod +x "$stub_dir/gh"
}
offer() {  # $@ = arguments after `offer`; POST defaults to on
    PATH="$stub_dir:$PATH" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
        MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of MAINTAINER_POST="${OFFER_POST:-on}" \
        python3 "$root/bin/maintainer" offer "$@" 2>&1
}
free40='[{"number":40,"labels":[],"assignees":[]}]'

offer_gh "$free40" '[]' '[]'
out=$(offer 40 dana "this one fits what you did on #12."); rc=$?
if [ "$rc" = 0 ] && grep -q '@dana' "$ofd/posted40" 2>/dev/null && grep -q 'offered to dana' <<<"$out"; then
    ok "a free issue is offered on its own thread, and the offer is read back"
else
    bad "a clean offer did not land on #40 (rc=$rc): $(tr '\n' ' ' <<<"$out" | cut -c1-120)"
fi

offer_gh "$free40" '[{"u":"owner","b":"@ana this one is yours"}]' '[]'
out=$(offer 40 dana "yours if you want it."); rc=$?
[ "$rc" != 0 ] && [ ! -f "$ofd/posted40" ] && grep -q 'ana' <<<"$out" \
    && ok "an issue somebody was already offered is refused, naming them" \
    || bad "a second person was offered an issue already offered to ana (rc=$rc)"

offer_gh '[{"number":40,"labels":[],"assignees":[]},{"number":41,"labels":[],"assignees":[]}]' '[]' '[]'
out=$(offer 40 cara "yours if you want it."); rc=$?
[ "$rc" != 0 ] && [ ! -f "$ofd/posted40" ] && grep -q '#41' <<<"$out" \
    && ok "somebody already holding an open offer is not handed a second" \
    || bad "cara holds #41 and was offered #40 as well (rc=$rc)"

offer_gh "$free40" '[]' '[{"number":77,"author":{"login":"erin"},"title":"fix","body":"Closes #40"}]'
out=$(offer 40 dana "yours if you want it."); rc=$?
[ "$rc" != 0 ] && [ ! -f "$ofd/posted40" ] && grep -q '#77' <<<"$out" \
    && ok "an issue an open pull request already closes is refused, naming the PR" \
    || bad "an issue with an open PR against it was offered (rc=$rc)"

# Rule 6: could not ask is not permission. Unreadable pull requests mean an
# issue somebody is already fixing cannot be told apart from a free one.
offer_gh "$free40" '[]' 'FAIL'
out=$(offer 40 dana "yours if you want it."); rc=$?
[ "$rc" != 0 ] && [ ! -f "$ofd/posted40" ] && grep -q 'could not read the open pull requests' <<<"$out" \
    && ok "an offer refuses when the open pull requests cannot be read" \
    || bad "an offer went out without knowing whether somebody had a PR for it (rc=$rc)"

offer_gh "$free40" '[]' '[]'
out=$(offer 40 dana "you and @erin could pair on it."); rc=$?
[ "$rc" != 0 ] && [ ! -f "$ofd/posted40" ] && grep -q '@erin' <<<"$out" \
    && ok "an offer that names a second person is refused" \
    || bad "an offer naming two people went out, which is the AMBIGUOUS shape (rc=$rc)"

offer_gh "$free40" '[]' '[]'
out=$(OFFER_POST=off offer 40 dana "yours if you want it."); rc=$?
[ "$rc" = 0 ] && [ ! -f "$ofd/posted40" ] && grep -q 'would offer' <<<"$out" \
    && ok "with POST off it says what it would post and posts nothing" \
    || bad "POST=off still posted, or said nothing (rc=$rc)"

echo "== release writes the marker that offers reads =="
# The marker that retracts an offer has existed since the release/offer
# confusion was first fixed, and nothing ever wrote it. Every withdrawal I
# posted was hand-typed prose, so `offers` went on counting the person as a
# holder. sysknife 2026-09-22: #428 and #433 were withdrawn from @QinXi-ai in
# public, in as many words, and the tool still reported them OVER-OFFERED with
# both. A marker with no command behind it is a marker nobody uses.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
if [ "$1" = "api" ] && [ "$2" = "-X" ]; then printf '%s\n' "$*" >> "$CAPTURE"; echo '{}'; exit 0; fi
echo '[]'
GHEOF
chmod +x "$stub_dir/gh"
cap="$ofd/release-capture.txt"; : > "$cap"
out=$(PATH="$stub_dir:$PATH" CAPTURE="$cap" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r \
      MAINTAINER_REPO=/tmp MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of MAINTAINER_POST=on \
      python3 "$root/bin/maintainer" release 70 veteran "it was never yours" 2>&1)
grep -q 'maintainer: claim-released' "$cap" \
    && ok "release posts a comment carrying the marker offers reads" \
    || bad "the posted comment has no marker, so offers still counts them: $(cat "$cap" | cut -c1-140)"
grep -q '@veteran' "$cap" \
    && ok "and the comment names the person being released" \
    || bad "the comment does not name the person"
handles="$(grep -oE '@[A-Za-z0-9-]+' "$cap" | sort -u | tr '\n' ' ')"
[ "$handles" = "@veteran " ] \
    && ok "and it names nobody else, so it cannot create a fresh ambiguity" \
    || bad "the release comment names more than one handle: $handles"
grep -q 'it was never yours' "$cap" \
    && ok "and it carries the reason, because a bare retraction reads as a rebuke" \
    || bad "the reason was dropped"
# POST=off rehearses rather than posting, like every other writing command.
: > "$cap"
out=$(PATH="$stub_dir:$PATH" CAPTURE="$cap" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r \
      MAINTAINER_REPO=/tmp MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of MAINTAINER_POST=off \
      python3 "$root/bin/maintainer" release 70 veteran "no longer needed" 2>&1)
[ -s "$cap" ] \
    && bad "release posted with POST=off" \
    || ok "release rehearses at POST=off and writes nothing"
grep -qi 'would release' <<<"$out" \
    && ok "and says what it would have posted" \
    || bad "the rehearsal says nothing: $out"

echo "== file-issue --dry-run rehearses even where posting is on =="
# 2026-10-01, sysknife: I ran `maintainer file-issue --title x` to see what it
# would do, on a profile with POST=on, and it filed sysknife#542 titled "x" for
# real. The only rehearsal was POST=off, which is a profile setting, not
# something a person checking one command reaches for. A dry run has to be a
# flag on the command, and it has to win over the profile.
fid="$stub_dir/fileissue"; mkdir -p "$fid"; rm -f "$fid/created"
cat > "$stub_dir/gh" <<GHEOF
#!/usr/bin/env bash
case "\$*" in
  *'issue create'*) touch "$fid/created"; echo 'https://github.com/o/r/issues/9';;
  *'issue list'*) echo '[]';;
  *'api user'*) echo owner;;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$fid" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=fiss MAINTAINER_POST=on \
      python3 "$root/bin/maintainer" file-issue --dry-run --title "probe" --body "b" 2>&1); rc=$?
[ "$rc" = 0 ] && [ ! -f "$fid/created" ] && grep -q 'would file' <<<"$out" \
    && ok "--dry-run prints what it would file and files nothing, with POST=on" \
    || bad "--dry-run filed an issue or failed (rc=$rc): $(tr '\n' ' ' <<<"$out" | cut -c1-120)"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$fid" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=fiss MAINTAINER_POST=on \
      python3 "$root/bin/maintainer" file-issue --title "probe" --body "b" 2>&1); rc=$?
[ "$rc" = 0 ] && [ -f "$fid/created" ] \
    && ok "and without the flag the same call files, so the flag is what stopped it" \
    || bad "the control case did not file (rc=$rc), so the dry-run assertion proves nothing"

echo "== file-issue refuses when it cannot read the tracker to deduplicate =="
# Rule 6. `_existing_finding` returned None for an unreadable tracker, which is
# the answer for "no duplicate", so a gh failure filed the same finding twice.
rm -f "$fid/created"
cat > "$stub_dir/gh" <<GHEOF
#!/usr/bin/env bash
case "\$*" in
  *'issue create'*) touch "$fid/created"; echo 'https://github.com/o/r/issues/9';;
  *'issue list'*) exit 1;;
  *'api user'*) echo owner;;
esac
GHEOF
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$fid" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=fiss MAINTAINER_POST=on \
      python3 "$root/bin/maintainer" file-issue --title "probe" --body "b" 2>&1); rc=$?
[ "$rc" != 0 ] && [ ! -f "$fid/created" ] && grep -q 'could not read the tracker' <<<"$out" \
    && ok "an unreadable tracker stops the filing instead of reading as no duplicate" \
    || bad "file-issue filed without knowing whether the finding was already filed (rc=$rc)"

echo "== offer --second lets the maintainer decide, and still refuses a taken issue =="
# 2026-10-01: the maintainer asked for half of a 22-issue batch to go to one
# contributor. The one-offer rule refused, so the offers went out as bare gh
# comments, which is the thing `offer` exists to stop: no read-back, no label,
# no check that the issue was free. The rule stays the default. Overriding it
# is a decision a person makes per call, and the other checks still hold.
offer_gh '[{"number":40,"labels":[],"assignees":[]},{"number":41,"labels":[],"assignees":[]}]' '[]' '[]'
out=$(offer 40 cara "yours too, if you want it." --second); rc=$?
[ "$rc" = 0 ] && grep -q '@cara' "$ofd/posted40" 2>/dev/null \
    && ok "--second offers a person who already holds one" \
    || bad "--second did not post (rc=$rc): $(tr '\n' ' ' <<<"$out" | cut -c1-120)"
offer_gh "$free40" '[{"u":"owner","b":"@ana this one is yours"}]' '[]'
out=$(offer 40 dana "yours if you want it." --second); rc=$?
[ "$rc" != 0 ] && [ ! -f "$ofd/posted40" ] \
    && ok "--second does not let an issue somebody else holds be offered" \
    || bad "--second double-booked #40 over ana (rc=$rc)"

echo "== an offer reserves the issue with the claim label =="
# GitHub drops an assignee who is not a collaborator and has not posted on the
# issue, so a fresh issue cannot be assigned to the person it was offered to.
# The label is the only reservation anybody else sees on the tracker.
offer_gh "$free40" '[]' '[]'
cat >> "$stub_dir/gh" <<GHEOF
GHEOF
python3 - "$stub_dir/gh" "$ofd" <<'PY'
import sys
p, d = sys.argv[1], sys.argv[2]
s = open(p).read()
s = s.replace('case "$*" in\n', 'case "$*" in\n  *\'issue edit 40\'*\'--add-label claimed\'*) touch "%s/labelled40"; echo ok;;\n' % d, 1)
open(p, 'w').write(s)
PY
rm -f "$ofd/labelled40"
out=$(CLAIM_LABEL=claimed offer 40 dana "this one fits what you did on #12."); rc=$?
[ "$rc" = 0 ] && [ -f "$ofd/labelled40" ] && grep -q 'claimed' <<<"$out" \
    && ok "the offered issue gets the claim label, and the output says so" \
    || bad "the offer left the issue unlabelled (rc=$rc): $(tr '\n' ' ' <<<"$out" | cut -c1-120)"

echo "== settle assigns an offer once its holder has posted on the issue =="
# The moment GitHub will accept the assignment is the holder's first comment on
# that issue. Nothing watched for it, so answered offers sat unassigned and the
# dashboard showed nobody on them.
std="$stub_dir/settle"; mkdir -p "$std"; rm -f "$std"/assigned*
cat > "$stub_dir/gh" <<GHEOF
#!/usr/bin/env bash
case "\$*" in
  *'pr list'*) echo '[]';;
  *'issue list'*) echo '[{"number":50,"labels":[],"assignees":[]},{"number":51,"labels":[],"assignees":[]},{"number":52,"labels":[],"assignees":[{"login":"gus"}]}]';;
  *'-X POST'*issues/50/assignees*) touch "$std/assigned50"; echo '{}';;
  *'-X POST'*issues/51/assignees*) touch "$std/assigned51"; echo '{}';;
  *'-X POST'*issues/52/assignees*) touch "$std/assigned52"; echo '{}';;
  *issues/50*'.assignees'*) [ -f "$std/assigned50" ] && echo '["fay"]' || echo '[]';;
  *issues/50/comments*) echo '[{"u":"owner","b":"@fay this one is yours"},{"u":"fay","b":"on it"}]';;
  *issues/51/comments*) echo '[{"u":"owner","b":"@hal this one is yours"}]';;
  *issues/52/comments*) echo '[{"u":"owner","b":"@gus this one is yours"},{"u":"gus","b":"thanks"}]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$std" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=st MAINTAINER_POST=on \
      python3 "$root/bin/maintainer" settle 2>&1); rc=$?
[ "$rc" = 0 ] && [ -f "$std/assigned50" ] && grep -q '#50 assigned to fay' <<<"$out" \
    && ok "an answered offer is assigned, and read back" \
    || bad "settle did not assign #50 to fay (rc=$rc): $(tr '\n' ' ' <<<"$out" | cut -c1-140)"
[ ! -f "$std/assigned51" ] \
    && ok "an unanswered offer is left alone, because GitHub would drop the assignee" \
    || bad "settle tried to assign an offer nobody answered"
[ ! -f "$std/assigned52" ] \
    && ok "an issue already assigned to its holder is not assigned again" \
    || bad "settle re-assigned an issue that already had its holder"

echo "== stale names holds nobody has touched for N days =="
# 2026-10-01: three holds were nine or more days old with nothing from their
# holders (#440 assigned on 09-16, #428 and #433 offered on 09-15), and finding
# them took a timeline read per issue by hand.
std2="$stub_dir/stale"; mkdir -p "$std2"
cat > "$stub_dir/gh" <<GHEOF
#!/usr/bin/env bash
case "\$*" in
  *'pr list'*) echo '[{"number":90,"author":{"login":"ivy"},"title":"fix","body":"Closes #62"}]';;
  *'issue list'*) echo '[{"number":60,"labels":[],"assignees":[]},{"number":61,"labels":[],"assignees":[]},{"number":62,"labels":[],"assignees":[]}]';;
  *issues/60/comments*'created_at'*) echo '[{"u":"owner","b":"@jo this one is yours","t":"2026-01-01T00:00:00Z"}]';;
  *issues/61/comments*'created_at'*) echo '[{"u":"owner","b":"@kim this one is yours","t":"2026-01-01T00:00:00Z"},{"u":"kim","b":"started","t":"2099-01-01T00:00:00Z"}]';;
  *issues/62/comments*'created_at'*) echo '[{"u":"owner","b":"@ivy this one is yours","t":"2026-01-01T00:00:00Z"}]';;
  *issues/60/comments*) echo '[{"u":"owner","b":"@jo this one is yours"}]';;
  *issues/61/comments*) echo '[{"u":"owner","b":"@kim this one is yours"},{"u":"kim","b":"started"}]';;
  *issues/62/comments*) echo '[{"u":"owner","b":"@ivy this one is yours"}]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$std2" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=st \
      python3 "$root/bin/maintainer" stale 9 2>&1); rc=$?
grep -qE 'STALE #60 +jo' <<<"$out" \
    && ok "an offer untouched for longer than N days is named" \
    || bad "the untouched hold on #60 was not reported (rc=$rc): $(tr '\n' ' ' <<<"$out" | cut -c1-140)"
grep -q '#61' <<<"$out" \
    && bad "a holder who posted recently was reported stale" \
    || ok "a holder who posted since is not stale"
grep -q '#62' <<<"$out" \
    && bad "a holder with an open pull request was reported stale" \
    || ok "a holder with an open pull request is not stale"

echo "== release unassigns the person and lifts the claim label =="
# #440 on 2026-10-01: the release comment went out, and the assignee and the
# claimed label stayed until I removed both by hand. A release that leaves the
# person assigned tells the tracker they still hold it.
rld="$stub_dir/release"; mkdir -p "$rld"; rm -f "$rld"/*
cat > "$stub_dir/gh" <<GHEOF
#!/usr/bin/env bash
case "\$*" in
  *'-X POST'*issues/70/comments*) printf '%s\n' "\$*" > "$rld/posted"; echo '{}';;
  *issues/70/comments*'[.[].body]'*) [ -f "$rld/posted" ] && echo '["@lee freeing this. <!-- maintainer: claim-released -->"]' || echo '[]';;
  *'issue edit 70'*'--remove-assignee lee'*'--remove-label claimed'*) touch "$rld/unassigned"; echo ok;;
  *issues/70*'.assignees'*) [ -f "$rld/unassigned" ] && echo '[]' || echo '["lee"]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$rld" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=rl MAINTAINER_POST=on CLAIM_LABEL=claimed \
      python3 "$root/bin/maintainer" release 70 lee "freeing this." 2>&1); rc=$?
[ "$rc" = 0 ] && [ -f "$rld/unassigned" ] && grep -q 'unassigned' <<<"$out" \
    && ok "a release of an assigned person unassigns them and removes the claim label" \
    || bad "the released person is still assigned (rc=$rc): $(tr '\n' ' ' <<<"$out" | cut -c1-140)"

echo "== an issue with an open pull request closing it is not free to offer =="
# The 2026-09-07 20:09 issues run reported it in its own process notes:
# "`maintainer offers` cannot see pull request threads ... #371 was printed as
# free while an open PR closed it." Offering that issue to somebody is the
# sysknife#252 shape again, two people aimed at one fix, and this time the tool
# that exists to prevent it would have caused it.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'pr list'*) echo '[{"number":88,"author":{"login":"carla"},"title":"fix","body":"Closes #40"},{"number":89,"author":{"login":"drive"},"title":"x","body":"related to #41"}]';;
  *'issue list'*) echo '[{"number":40,"labels":[]},{"number":41,"labels":[]},{"number":42,"labels":[]}]';;
  *issues/40/comments*) echo '[]';;
  *issues/41/comments*) echo '[]';;
  *issues/42/comments*) echo '[]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of \
      python3 "$root/bin/maintainer" offers 2>&1)
grep -qE 'free to offer:[^\n]*#40' <<<"$out" \
    && bad "an issue an open PR closes is offered to somebody else" \
    || ok "an issue an open PR closes is kept out of the free list"
grep -qE '#40.*(PR|#88)' <<<"$out" \
    && ok "and it says which pull request took it, rather than dropping it silently" \
    || bad "the issue vanishes from the count with no reason given"
# The negative twin. A PR that merely mentions an issue closes nothing, so #41
# stays offerable; treating a mention as a claim would starve the free pool.
grep -qE 'free to offer:[^\n]*#41' <<<"$out" \
    && ok "an issue a PR only mentions stays free" \
    || bad "a bare mention removed an issue from the free pool"
grep -qE 'free to offer:[^\n]*#42' <<<"$out" \
    && ok "an issue with no pull request at all stays free" \
    || bad "the free pool collapsed"


