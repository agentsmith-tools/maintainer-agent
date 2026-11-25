#!/usr/bin/env bash
# Part of the offline suite: the setup package, run records, failure taxonomy, deny walls and the first merge gate receipts.
# tests/run-tests.sh sources this file in one shell, after its own preamble and after
# the parts before it, so every helper and variable it reads comes from there. It is
# sourced, never run on its own, and carries a shebang so the shell sweeps pick it up.

# The parts share one shell. These names are set by the driver or by an earlier part;
# declare only tells the linter they are read here, and assigns and changes nothing.
declare gate_settings
declare root
declare stub_dir
declare suite_origin
declare suite_root

echo "== packages/setup has a suite, because 127 tests had no gate at all =="
# sysknife#468 raised the sysknife-setup Node floor and no suite covered a line
# of it. `packages/setup` is an npm package with 127 tests of its own, it ships
# to users, and the rust, shell and docs suites between them claimed none of it,
# so the gate refused every pull request touching it. That is issue #11 again in
# a fourth language.
nsuite="$root/profiles/sysknife/verify.d/node.sh"
[ -f "$nsuite" ] && ok "sysknife has a node suite" || bad "packages/setup is still uncoverable"
for fn in suite_covers suite_image suite_needs suite_command suite_ran; do
    # shellcheck disable=SC1090
    ( . "$nsuite" >/dev/null 2>&1; declare -F "$fn" >/dev/null ) \
        && ok "node suite defines $fn" || bad "node suite has no $fn"
done
# shellcheck disable=SC1090
( . "$nsuite" >/dev/null 2>&1; suite_covers packages/setup/node-preflight.js ) \
    && ok "and it claims the JavaScript nothing else did" \
    || bad "the node suite does not cover packages/setup"
# shellcheck disable=SC1090
( . "$nsuite" >/dev/null 2>&1; suite_covers crates/sysknife-daemon/src/lib.rs ) \
    && bad "the node suite claims Rust it cannot run" \
    || ok "and claims nothing it cannot run"
# setup-contract.test.mjs probes a Unix socket with netcat, and the bare
# node:24-slim image has none, so the clean run failed and sysknife#749 sat
# unmergeable for two review runs. Sourced and asked, not grepped: the doctor
# resolves exactly this list inside the image.
# shellcheck disable=SC1090
nneeds="$( . "$nsuite" >/dev/null 2>&1; suite_needs )"
case " $nneeds " in
    *" nc "*) ok "the node suite's image is checked for the netcat its contract test drives" ;;
    *) bad "the node suite no longer requires nc, so the image can lose it unnoticed" ;;
esac
# wizard-harness.mjs runs `git init` in a scratch directory, so a test that
# drives the wizard fails in an image without git for the reason the netcat test
# did. Sourced and asked, like nc above.
case " $nneeds " in
    *" git "*) ok "and for the git its wizard harness runs" ;;
    *) bad "the node suite no longer requires git, so the image can lose it unnoticed" ;;
esac
# The suite claims every file under packages/setup, the README included. A
# pull request that changes only the README (sysknife#821) has nothing else for
# the gate to break, so a mutate glob with no Markdown in it leaves that change
# unreceiptable. The answer comes from the plugin, matched the way the gate
# matches a file name against a glob.
# shellcheck disable=SC1090
nglobs="$( . "$nsuite" >/dev/null 2>&1; suite_mutate_glob )"
nmd=0
nreadme="README.md"
while IFS= read -r nglob; do
    # shellcheck disable=SC2254
    case "$nreadme" in $nglob) nmd=1 ;; esac
done <<<"$nglobs"
[ "$nmd" = 1 ] \
    && ok "the node suite can mutate the README it claims" \
    || bad "the node suite claims packages/setup/README.md and cannot mutate it, so a README-only change gets no receipt"
# A count that cannot be read is a vacuous pass. node --test prints "# pass N".
npass="$stub_dir/npass"; rm -rf "$npass"; mkdir -p "$npass"
# node --test prints `ℹ pass N` under its default reporter and `# pass N` under
# TAP. The first version of suite_ran read only the TAP form, so a clean run of
# ten passing tests counted zero and the gate refused it as vacuous. Measured
# in the real image against sysknife#468, not assumed from the docs.
printf 'ok 1 - a\nok 2 - b\n# tests 2\n# pass 2\n# fail 0\n' > "$npass/clean.log"
printf '\xe2\x84\xb9 tests 10\n\xe2\x84\xb9 pass 10\n\xe2\x84\xb9 fail 0\n' > "$npass/spec.log"
printf '# tests 0\n# pass 0\n# fail 0\n' > "$npass/empty.log"
# shellcheck disable=SC1090
n1=$( . "$nsuite" >/dev/null 2>&1; suite_ran "$npass/clean.log" )
# shellcheck disable=SC1090
n0=$( . "$nsuite" >/dev/null 2>&1; suite_ran "$npass/empty.log" )
# shellcheck disable=SC1090
nspec=$( . "$nsuite" >/dev/null 2>&1; suite_ran "$npass/spec.log" )
{ [ "$n1" -ge 1 ] && [ "$n0" = 0 ]; } \
    && ok "it counts executed tests, and a filter matching nothing counts zero ($n1 / $n0)" \
    || bad "suite_ran cannot tell a real run from a vacuous one ($n1 / $n0)"
[ "${nspec:-0}" -ge 1 ] \
    && ok "and it reads the default reporter too, not only TAP ($nspec)" \
    || bad "a clean run under node's default reporter counts zero and is refused ($nspec)"
# The filter is a file, not a name pattern. --test-name-pattern selects which
# tests report and still runs every file, which in this package means
# setup-contract.test.mjs wanting a daemon socket the container has not got.
# shellcheck disable=SC1090
ncmd=$( . "$nsuite" >/dev/null 2>&1; suite_command 'packages/setup/tests/x.test.mjs' )
grep -q 'test-name-pattern' <<<"$ncmd" \
    && bad "the node filter still selects names, so every file runs: $ncmd" \
    || ok "the node filter names one file, so an unrelated file cannot fail the run"
# The package ships to npm, so it is production and the merge gate has to
# re-check it. A suite with no matching PROD_GLOBS entry lets the same change
# through the no-production narrowing instead.
grep -q 'packages/setup' "$root/profiles/sysknife/profile.env" \
    && ok "packages/setup is declared production" \
    || bad "a published npm package is not in PROD_GLOBS"

echo "== an aborted run records its cause in the index, not just its symptom =="
# index.md is the document a reader trusts, and for thirty-one aborted runs it
# said "the run wrote no usable report" and nothing else. That is the symptom.
# The failure marker had the cause and the index did not, so the trail could
# tell you a week was quiet but never why.
abrt="$stub_dir/aborthome"; rm -rf "$abrt"; mkdir -p "$abrt/.local/bin"
abrt_repo="$abrt/repo"; git init -q -b main "$abrt_repo"
abrt_state="$abrt/state"; mkdir -p "$abrt_state/runs" "$abrt_state/state"
cat > "$abrt/.local/bin/maintainer" <<'AEOF'
#!/usr/bin/env bash
case "$1" in
  start) echo "=== maintainer 2031-05-06T07-08-review ==="; echo "context";
         echo "write the run report to $MAINTAINER_STATE/runs/2031-05-06T07-08-review.md";;
  finish) exit 1 ;;
  abort) printf '%s\n' "$3" > "$ABORT_REASON_FILE"; exit 0 ;;
  *) exit 0 ;;
esac
AEOF
chmod +x "$abrt/.local/bin/maintainer"
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'auth token'*) echo TOKENTOKENTOKENTOKEN;; *'api user'*) echo vladimirrott;; esac"
make_stub claude 'echo "API Error: Cannot reach the API server - check your internet or DNS (EAI_AGAIN)" >&2; exit 1'
: > "$abrt/reason.txt"
PATH="$stub_dir:$abrt/.local/bin:$PATH" ABORT_REASON_FILE="$abrt/reason.txt" HOME="$abrt" \
    MAINTAINER_NOTIFY=off MAINTAINER_SETTINGS="$gate_settings" MAINTAINER_STATE_DIR="$abrt_state" \
    MAINTAINER_REPO_PATH="$abrt_repo" MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review >/dev/null 2>&1
if grep -qE 'transient|needs_human|unclassified' "$abrt/reason.txt" 2>/dev/null; then
    ok "the abort recorded in the index carries a classified cause"
else
    bad "the index still records only the symptom: '$(cat "$abrt/reason.txt" 2>/dev/null)'"
fi
grep -qi 'eai_again\|reach the api' "$abrt/reason.txt" 2>/dev/null \
    && ok "and the evidence line that produced it" \
    || bad "the recorded cause carries no evidence: '$(cat "$abrt/reason.txt" 2>/dev/null)'"
rm -f "$stub_dir/claude"; make_stub claude 'exit 0'

echo "== a report that finish refuses is recorded as refused, not as missing =="
# 2026-10-25: three magent runs were indexed "ABORTED, no report" while each
# report existed. finish had refused them for length (#133), and the index line
# sent the reader looking for a report that was sitting in runs/.
cat > "$abrt/.local/bin/maintainer" <<'AEOF'
#!/usr/bin/env bash
case "$1" in
  start) echo "=== maintainer 2031-05-06T07-08-review ==="; echo "context";
         echo "write the run report to $MAINTAINER_STATE/runs/2031-05-06T07-08-review.md";;
  finish) echo "maintainer: x.md has 2 non-empty line(s) below its heading, which is a fixture rather than a report. It is NOT being indexed." >&2; exit 1 ;;
  abort) printf '%s\n' "$3" > "$ABORT_REASON_FILE"; exit 0 ;;
  *) exit 0 ;;
esac
AEOF
rm -rf "$abrt_state/logs"; : > "$abrt/reason.txt"
PATH="$stub_dir:$abrt/.local/bin:$PATH" ABORT_REASON_FILE="$abrt/reason.txt" HOME="$abrt" \
    MAINTAINER_NOTIFY=off MAINTAINER_SETTINGS="$gate_settings" MAINTAINER_STATE_DIR="$abrt_state" \
    MAINTAINER_REPO_PATH="$abrt_repo" MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review >/dev/null 2>&1
grep -q '^report refused' "$abrt/reason.txt" 2>/dev/null \
    && ok "a refused report is indexed as refused" \
    || bad "a report finish refused is indexed as missing: '$(cat "$abrt/reason.txt" 2>/dev/null)'"
grep -q 'fixture rather than a report' "$abrt/reason.txt" 2>/dev/null \
    && ok "and the index carries finish's own reason" \
    || bad "the refused-report line drops finish's reason: '$(cat "$abrt/reason.txt" 2>/dev/null)'"

echo "== when the gate blames its own container it says which line said so =="
# The verdict was unfalsifiable. On 2026-09-21 a verify of sysknife#461 refused
# with "the unmutated test failed on my suite environment", tailed five
# Compiling lines that said nothing, and named no shape. Deciding whether the
# gate was right meant reading the pattern list and guessing. A refusal nobody
# can act on is the shape lessons.md keeps recording.
envd="$stub_dir/envdiag"; rm -rf "$envd"; mkdir -p "$envd"
printf 'Compiling foo v1.0\nerror: could not find system library '"'"'glib-2.0'"'"'\nCompiling bar v2.0\n' > "$envd/glib.log"
printf 'Compiling foo v1.0\nassertion failed: left != right\n' > "$envd/real.log"
# Drive the predicate straight out of the file: it is a pure function of a log
# and the tool around it needs a whole profile to start.
env_mg="$root/bin/maintainer-merge"
if sc_case 5; then
hit=$(bash -c 'eval "$(sed -n "/^looks_environmental()/,/^}/p" "$1")"; looks_environmental "$2"' _ "$env_mg" "$envd/glib.log") || true
grep -qi 'system library' <<<"$hit" \
    && ok "an environmental refusal names the line that made it environmental" \
    || bad "looks_environmental still answers yes or no and nothing else: '$hit'"
neg=$(bash -c 'eval "$(sed -n "/^looks_environmental()/,/^}/p" "$1")"; looks_environmental "$2"' _ "$env_mg" "$envd/real.log") || true
[ -z "$neg" ] \
    && ok "and a genuine assertion failure is still not called environmental" \
    || bad "a real test failure was excused as the container's fault: '$neg'"
grep -q 'the line that says so' "$env_mg" \
    && ok "the refusal quotes that line back to the reader" \
    || bad "the refusal prints a tail that may not contain the reason"
# And the reason to print it: the shape list held the bare token `pkg-config`,
# which matches an ordinary cargo line compiling a CRATE of that name. Every
# rust verify that failed for any reason was reported as the container's fault.
# Measured on sysknife#438 on 2026-09-21, the matched line was
#   Compiling pkg-config v0.3.33
printf 'Compiling serde v1.0\n   Compiling pkg-config v0.3.33\nerror: test failed\n' > "$envd/crate.log"
fp=$(bash -c 'eval "$(sed -n "/^looks_environmental()/,/^}/p" "$1")"; looks_environmental "$2"' _ "$env_mg" "$envd/crate.log") || true
[ -z "$fp" ] \
    && ok "a crate named pkg-config in ordinary build output is not environmental" \
    || bad "the gate blames its own container whenever pkg-config is compiled: '$fp'"
# The shape it was written for must still bite. This is the real glib wording.
printf 'The system library `glib-2.0` required by crate `glib-sys` was not found.\nerror: failed to run custom build command for `glib-sys v0.18.1`\n' > "$envd/realglib.log"
gp=$(bash -c 'eval "$(sed -n "/^looks_environmental()/,/^}/p" "$1")"; looks_environmental "$2"' _ "$env_mg" "$envd/realglib.log") || true
grep -qi 'system library\|custom build command' <<<"$gp" \
    && ok "and a genuinely missing system library still is" \
    || bad "narrowing the shape lost the case it was written for: '$gp'"
sc_end; fi

echo "== the failure taxonomy is configuration, not a regex buried in a function =="
# Two instructions met by one change. The shapes that decide whether a run is
# retried, escalated or left unexplained were an alternation inside a shell
# function, so adding a cause meant editing code and nothing could enumerate
# what the agent knows how to recognise. They are a table now.
shapes="$root/lib/failure-shapes.json"
[ -f "$shapes" ] && ok "the failure shapes live in a file" \
    || bad "the taxonomy is still inline in lib/profile.sh"
python3 - "$shapes" <<'PYEOF' && ok "every shape names an id, a class, a pattern and what it means" \
    || bad "a shape is missing a field, so a classified failure cannot be explained"
import json, sys, re
d = json.load(open(sys.argv[1]))
classes = set(d["classes"])
assert classes == {"transient", "needs_human"}, classes
seen = set()
for sh in d["shapes"]:
    for k in ("id", "class", "pattern", "means"):
        assert sh.get(k), f"{sh.get('id')} has no {k}"
    assert sh["class"] in classes, sh
    assert sh["id"] not in seen, f"duplicate id {sh['id']}"
    seen.add(sh["id"])
    re.compile(sh["pattern"])            # it has to be a usable pattern
    if sh["class"] == "needs_human":
        assert sh.get("fix"), f"{sh['id']} tells nobody what to do"
PYEOF
# No number in the classifier that the table could carry instead.
if sc_case 1; then
grep -nE 'grep -aoiE "you' "$root/lib/profile.sh" >/dev/null 2>&1 \
    && bad "the alternation is still hardcoded in backend_transient_reason" \
    || ok "backend_transient_reason reads the table rather than carrying one"
sc_end; fi

echo "== every failure is classified, or says out loud that it was not =="
tcx="$stub_dir/classify2"; rm -rf "$tcx"; mkdir -p "$tcx"
printf 'API Error: Can%st reach the API server - check your internet or DNS (EAI_AGAIN)\n' "'" > "$tcx/dns.log"
printf 'Failed to authenticate: OAuth session expired and could not be refreshed\n' > "$tcx/oauth.log"
printf 'thread panicked at src/main.rs:12: assertion failed\n' > "$tcx/odd.log"
# shellcheck disable=SC1090
if sc_case 5; then
cls=$( . "$root/lib/profile.sh" >/dev/null 2>&1; maintainer_classify_failure "$tcx/dns.log" ); crc=$?
[ "$crc" = 0 ] && grep -q '^transient' <<<"$cls" \
    && ok "a DNS blip classifies as transient, with a cause id: $(cut -f2 <<<"$cls")" \
    || bad "the DNS shape is not classified (rc=$crc, got '$cls')"
# shellcheck disable=SC1090
cls=$( . "$root/lib/profile.sh" >/dev/null 2>&1; maintainer_classify_failure "$tcx/oauth.log" ); crc=$?
[ "$crc" = 0 ] && grep -q '^needs_human' <<<"$cls" \
    && ok "an expired login classifies as needing a person: $(cut -f2 <<<"$cls")" \
    || bad "the expired-login shape is not classified (rc=$crc, got '$cls')"
# The one that matters most: an unknown failure must SAY it is unknown rather
# than being silently dropped into the default branch. Three runs aborted in
# five days and failed-review.json recorded "the run is not auditable" with no
# cause at all.
# shellcheck disable=SC1090
cls=$( . "$root/lib/profile.sh" >/dev/null 2>&1; maintainer_classify_failure "$tcx/odd.log" ); crc=$?
[ "$crc" != 0 ] && grep -q '^unclassified' <<<"$cls" \
    && ok "an unrecognised failure returns 'unclassified' rather than nothing" \
    || bad "an unrecognised failure returns silence (rc=$crc, got '$cls')"
grep -q 'panicked' <<<"$cls" \
    && ok "and carries the evidence line, so the run is explainable by reading it" \
    || bad "the unclassified verdict carries no evidence: $cls"
# A missing or unreadable table must refuse, not classify everything as fine.
# shellcheck disable=SC1090
cls=$( . "$root/lib/profile.sh" >/dev/null 2>&1; MAINTAINER_FAILURE_SHAPES=/nonexistent/x.json maintainer_classify_failure "$tcx/dns.log" ); crc=$?
[ "$crc" != 0 ] \
    && ok "an unreadable taxonomy refuses rather than reporting nothing wrong" \
    || bad "with no table every failure classified as clean"
sc_end; fi

echo "== one broken run raises one critical toast, not two =="
# The second alert says "produced no report", which is the SAME event described
# a second way: the backend died, so of course it wrote nothing. Two criticals
# for one cause is how the important one gets dismissed.
tb="$stub_dir/twotoast"; rm -rf "$tb"; mkdir -p "$tb/.local/bin"
tb_repo="$tb/repo"; git init -q -b main "$tb_repo"
tb_state="$tb/state"; mkdir -p "$tb_state/runs" "$tb_state/state"
cat > "$stub_dir/notify-send" <<'NEOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$NOTIFY_LOG"
NEOF
chmod +x "$stub_dir/notify-send"
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'auth token'*) echo TOKENTOKENTOKENTOKEN;; *'api user'*) echo vladimirrott;; esac"
cat > "$tb/.local/bin/maintainer" <<'MEOF'
#!/usr/bin/env bash
case "$1" in
  start) echo "=== maintainer 2031-02-03T04-05-review ==="; echo "context";
         echo "write the run report to $MAINTAINER_STATE/runs/2031-02-03T04-05-review.md";;
  finish) exit 1 ;;
  *) exit 0 ;;
esac
MEOF
chmod +x "$tb/.local/bin/maintainer"
# A backend failure with a message no classifier knows.
make_stub claude 'echo "Segmentation fault in the model client" >&2; exit 1'
: > "$tb/notify.log"
PATH="$stub_dir:$tb/.local/bin:$PATH" NOTIFY_LOG="$tb/notify.log" HOME="$tb" MAINTAINER_NOTIFY=on \
    MAINTAINER_SETTINGS="$gate_settings" MAINTAINER_STATE_DIR="$tb_state" \
    MAINTAINER_REPO_PATH="$tb_repo" MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review >/dev/null 2>&1
crits=$(grep -c 'u critical' "$tb/notify.log" 2>/dev/null || true)
[ "${crits:-0}" -le 1 ] \
    && ok "an unrecognised backend failure raises one critical toast (got ${crits:-0})" \
    || bad "one broken run raised ${crits} critical toasts for the same cause"
[ "${crits:-0}" -ge 1 ] \
    && ok "and it does still raise one, rather than going quiet" \
    || bad "the run failed and told nobody"
rm -f "$stub_dir/notify-send" "$stub_dir/claude" "$stub_dir/gh"
make_stub notify-send 'exit 0'
make_stub claude 'exit 0'
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'auth token'*) echo ghp_OracleStubTokenAAAAAAAAAAAAAAAAAAAAA;; *'api user'*) echo vladimirrott; exit 0;; esac"
# And the OnFailure script must write to the failing profile's trail only. It
# globbed every *-maint directory, so a magent failure appeared in sysknife's
# alerts.log and an operator reading either one saw the other's incidents.
if sc_case 1; then
grep -q 'instance%%-\*\|profile=' "$root/platform/linux/alert.sh" \
    && ok "the alert resolves which profile failed" \
    || bad "the alert still writes every profile's trail"
sc_end; fi

echo "== the suite provisions from a local origin, never from the network =="
grep -q 'export MAINTAINER_REPO_ORIGIN=' "$suite_root/tests/run-tests.sh" \
    && ok "every case clones from a bare repo created here" \
    || bad "a case that reaches provisioning would clone from github.com"
[ -d "$suite_origin" ] \
    && ok "and that origin exists before any case runs" \
    || bad "the local origin was never created"

echo "== a run provisions its own checkout, isolated from any dev tree =="
# A review bot must not operate in a working copy a human edits. REPO_PATH now
# points at a checkout the profile owns; if it is missing, run.sh clones it from
# REPO_ORIGIN. Driven against a real local origin, so the clone is real.
prov_origin="$stub_dir/origin.git"; rm -rf "$prov_origin"
git init -q --bare "$prov_origin"
prov_seed="$stub_dir/seed"; rm -rf "$prov_seed"; mkdir -p "$prov_seed"
( cd "$prov_seed" && git init -q && git config user.email t@t && git config user.name t \
  && echo hi > f && git add f && git commit -qm init \
  && git branch -M main && git remote add origin "$prov_origin" && git push -q origin main ) >/dev/null 2>&1
prov_checkout="$stub_dir/owned-checkout"; rm -rf "$prov_checkout"
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'auth token'*) echo ghp_OracleStubTokenAAAAAAAAAAAAAAAAAAAAA;; *'api user'*) echo vladimirrott; exit 0;; esac"
make_stub claude "exit 0"
out=$(PATH="$stub_dir:$PATH" HOME="$stub_dir" MAINTAINER_SETTINGS="$gate_settings" \
    MAINTAINER_STATE_DIR="$stub_dir/prov-state" \
    MAINTAINER_REPO_PATH="$prov_checkout" MAINTAINER_REPO_ORIGIN="$prov_origin" \
    MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review 2>&1)
[ -d "$prov_checkout/.git" ] \
    && ok "a missing checkout is cloned from REPO_ORIGIN before the run reviews" \
    || bad "run.sh did not provision the checkout: $(printf '%s' "$out"|tr '\n' ' '|cut -c1-90)"
# The profiles that ship must not point their review at a Desktop dev tree.
for pe in "$root"/profiles/sysknife/profile.env "$root"/profiles/magent/profile.env; do
    if grep -qE '^REPO_PATH=.*Desktop' "$pe"; then
        bad "$(basename "$(dirname "$pe")") reviews a Desktop dev tree"
    else
        ok "$(basename "$(dirname "$pe")") reviews an owned checkout, not a Desktop dev tree"
    fi
    grep -q '^REPO_ORIGIN=' "$pe" \
        && ok "$(basename "$(dirname "$pe")") declares an origin to provision from" \
        || bad "$(basename "$(dirname "$pe")") has no REPO_ORIGIN, so a missing checkout cannot self-heal"
done

echo "== every profile's deny wall names the verbs that matter =="
# Generated, then asserted. The tests read the artifact the agent is handed, not
# the spec it came from: a generator bug that drops every absolute spelling is
# invisible to a test that reads the input.
#
# EVERY profile. This block pinned profiles/sysknife by name, so deleting a rule
# from a second profile's deny.json left all three gates green and that
# profile's containment was unguarded.
for pdj in "$root"/profiles/*/deny.json; do
    pn="$(basename "$(dirname "$pdj")")"
    wd="$stub_dir/wall-$pn"; mkdir -p "$wd"
    cp "$pdj" "$wd/"
    [ -f "$(dirname "$pdj")/opencode.json" ] && cp "$(dirname "$pdj")/opencode.json" "$wd/"
    python3 "$root/scripts/render-settings.py" "$wd" /home/fakeuser >/dev/null 2>&1 \
        && ok "$pn: the deny wall generates from deny.json" || bad "$pn: render-settings.py failed"
    w="$wd/settings.json"
    for verb in "git push" "gh pr merge" "cargo publish" "npm publish" "gh release" "git tag" \
                "gh repo delete" "gh secret" "maintainer-merge receipt"; do
        grep -qF "Bash($verb:*)" "$w" && ok "$pn: denies '$verb'" || bad "$pn: LOST '$verb'"
        grep -qF "Bash(/usr/bin/$verb:*)" "$w" \
            && ok "$pn: denies '/usr/bin/$verb'" || bad "$pn: no absolute spelling of '$verb'"
    done
    for path in ".ssh" ".config/gh/" ".aws" ".gnupg" ".netrc" ".credentials.json"; do
        grep -qF "$path" "$w" && ok "$pn: protects $path" || bad "$pn: LOST $path"
    done
done
# The primary profile's artifact stays available to the assertions below.
wall_dir="$stub_dir/wall-sysknife"
s="$wall_dir/settings.json"
echo "== the Codex backend documents its weaker containment =="
# Codex has no per-command deny list. If someone swaps the default backend
# without reading that, publishing verbs stop being blocked. Pin the warning.
if sc_case 1; then
if grep -q 'COARSER' "$root/lib/backends/codex.sh"; then
    ok "codex backend states its containment is coarser"
else
    bad "codex backend no longer warns that it cannot express a deny list"
fi
sc_end; fi
if grep -q 'BACKEND="\${MAINTAINER_BACKEND:-claude}"' "$root/profiles/sysknife/profile.env"; then
    ok "default backend is claude"
else
    bad "default backend is no longer claude"
fi

echo "== the wall covers the directories the tools are really installed in =="
# The wall listed three FHS prefixes. On this machine `cargo` is in ~/.cargo/bin,
# `npm` under ~/.local/lib/nodejs/.../bin and `maintainer-merge` in ~/.local/bin,
# so the publishing verbs had no absolute rule and the single rule protecting
# the merge gate's receipt was bypassable by writing the full path.
real_wall="$stub_dir/wall-real"; mkdir -p "$real_wall"
cp "$root/profiles/sysknife/deny.json" "$real_wall/"
python3 "$root/scripts/render-settings.py" "$real_wall" "$HOME" >/dev/null 2>&1
python3 - "$real_wall/settings.json" <<'PYEOF' && ok "every verb is denied at its real install directory" || bad "a verb has no rule where its binary actually lives"
import json, os, shutil, sys
deny = set(json.load(open(sys.argv[1]))["permissions"]["deny"])
missing = []
for verb in ("cargo publish", "npm publish", "git push", "gh pr merge",
             "gh repo delete", "maintainer-merge receipt"):
    real = shutil.which(verb.split()[0])
    if not real:
        continue
    rule = f"Bash({os.path.dirname(real)}/{verb}:*)"
    if rule not in deny:
        missing.append(rule)
for m in missing:
    print("  missing:", m)
sys.exit(1 if missing else 0)
PYEOF

echo "== the generated wall is valid, absolute, and home-relative where it must be =="
if python3 -c "import json;json.load(open('$s'))" 2>/dev/null; then ok "generated deny wall is valid JSON"; else bad "generated deny wall is not valid JSON"; fi
grep -q '__HOME__' "$s" && bad "a placeholder survived generation" || ok "no placeholder survived"
grep -q '/home/fakeuser/.ssh' "$s" && ok "credential paths are spelled expanded" || bad "expanded spelling missing"
grep -q '\$HOME/.ssh' "$s" && ok "credential paths are spelled with \$HOME" || bad "\$HOME spelling missing"
# MEASURED 2026-09-03: Read(/abs/path) anchors to the SETTINGS directory and
# denies nothing; Read(~/path) denies. Normalising the tilde away, which looks
# like a tidy-up, would disable every credential rule at once and silently.
grep -q '"Read(~/.ssh/\*\*)"' "$s" && ok "Read rules use the home-relative form that denies" \
    || bad "Read rules lost the ~/ form; credential paths are unprotected"
python3 - "$root/profiles/sysknife/deny.json" <<'PYEOF' && ok "deny.json commits no literal home path" || bad "deny.json contains a home path"
import json,sys
spec=open(sys.argv[1]).read()
sys.exit(1 if "/home/" in spec or "/Users/" in spec else 0)
PYEOF

echo "== no home path or username is committed =="
# The needle is assembled at runtime so this file does not match itself, which
# is how the check first "found" a leak that was only its own source line.
# --exclude=.git as well as --exclude-dir: a linked worktree's .git is a FILE
# holding the absolute path of the main checkout, which is the author's home.
needle="entro""pia"
if grep -rq "$needle" --exclude-dir=.git --exclude=.git --exclude=run-tests.sh "$root"; then
    bad "a home path leaked into the repository"
    grep -rn "$needle" --exclude-dir=.git --exclude=.git --exclude=run-tests.sh "$root" | head -3
else
    ok "no home path committed"
fi

echo "== install.sh --dry-run changes nothing =="
before=$(find "$HOME/.local/share/maintainer" -type f 2>/dev/null | wc -l)
"$root/install.sh" --dry-run >/dev/null 2>&1
after=$(find "$HOME/.local/share/maintainer" -type f 2>/dev/null | wc -l)
check "dry run leaves the filesystem alone" "$after" "$before"

echo "== every prompt carries the reservation rule =="
for p in review issues audit; do
    if grep -q 'twir-listed' "$root/profiles/sysknife/prompts/$p.md"; then
        ok "prompts/$p.md carries the reserved-issue rule"
    else
        bad "prompts/$p.md lost the reserved-issue rule"
    fi
done

echo "== the helper parses and keeps its report sentinel =="
python3 -c "import ast;ast.parse(open('$root/bin/maintainer').read())" 2>/dev/null \
  && ok "bin/maintainer parses" || bad "bin/maintainer does not parse"
grep -q 'report not yet written' "$root/bin/maintainer" \
  && ok "report sentinel intact" || bad "report sentinel gone (finish can no longer reject an empty run)"

echo "== the merge gate refuses without a valid receipt =="
mg="$root/bin/maintainer-merge"
export MAINTAINER_STATE="$stub_dir/state" MAINTAINER_ACCOUNT="testuser" MAINTAINER_SLUG="o/r" MAINTAINER_REPO="$stub_dir/repo"
# These cases are about the other conditions, so they need a profile that has
# declared its production paths. A merge with none declared is refused, which is
# asserted on its own just below.
export PROD_GLOBS="crates/*/src/*"
mkdir -p "$MAINTAINER_STATE" "$MAINTAINER_REPO"

# Identity is checked before anything else.
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'api user'*) echo wronguser; exit 0;; esac"
out=$(PATH="$stub_dir:$PATH" bash "$mg" merge 1 2>&1); rc=$?
if [ "$rc" != 0 ] && grep -q 'not testuser' <<<"$out"; then ok "merge refuses under the wrong gh account"; else bad "merge did not refuse a wrong account (rc=$rc)"; fi

# Right account, but no receipt exists.
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'api user'*) echo testuser; exit 0;; esac"
out=$(PATH="$stub_dir:$PATH" bash "$mg" merge 1 2>&1); rc=$?
if [ "$rc" != 0 ] && grep -q 'no verification receipt' <<<"$out"; then ok "merge refuses with no receipt"; else bad "merge did not refuse a missing receipt (rc=$rc)"; fi

# Receipt exists, but the review is not approved.
PATH="$stub_dir:$PATH" bash "$mg" receipt 1 abcdef1234 "drift guard mutated, went red" >/dev/null 2>&1
make_stub gh "case \"\$*\" in
  *'auth switch'*) exit 0;;
  *'api user'*) echo testuser;;
  *reviewDecision*) echo CHANGES_REQUESTED;;
  *headRefOid*) echo abcdef1234;;
  *mergeStateStatus*) echo CLEAN;;
esac"
out=$(PATH="$stub_dir:$PATH" bash "$mg" merge 1 2>&1); rc=$?
if [ "$rc" != 0 ] && grep -q 'not APPROVED' <<<"$out"; then ok "merge refuses an unapproved PR"; else bad "merge did not refuse an unapproved PR (rc=$rc)"; fi

# Approved, but a check is failing.
make_stub gh "case \"\$*\" in
  *'auth switch'*) exit 0;;
  *'api user'*) echo testuser;;
  *reviewDecision*) echo APPROVED;;
  *headRefOid*) echo abcdef1234;;
  *mergeStateStatus*) echo CLEAN;;
  *'pr checks'*) echo '[{\"name\":\"rust\",\"bucket\":\"fail\"}]';;
esac"
out=$(PATH="$stub_dir:$PATH" bash "$mg" merge 1 2>&1); rc=$?
if [ "$rc" != 0 ] && grep -q 'failing check' <<<"$out"; then ok "merge refuses a failing board"; else bad "merge did not refuse a failing board (rc=$rc)"; fi

# Approved, board green, but zero checks reported at all.
make_stub gh "case \"\$*\" in
  *'auth switch'*) exit 0;;
  *'api user'*) echo testuser;;
  *reviewDecision*) echo APPROVED;;
  *headRefOid*) echo abcdef1234;;
  *mergeStateStatus*) echo CLEAN;;
  *'pr checks'*) echo '';;
esac"
out=$(PATH="$stub_dir:$PATH" bash "$mg" merge 1 2>&1); rc=$?
if [ "$rc" != 0 ] && grep -q 'no checks at all' <<<"$out"; then ok "merge refuses when no checks reported (empty is not green)"; else bad "merge treated an empty board as green (rc=$rc)"; fi

# A receipt is signed over its bytes, and the bytes name the PR it was earned
# for. cmd_merge selected it by filename and read back only `head` and `kind`,
# so copying 1.json to 2.json produced a receipt that verifies perfectly and
# proves something about a different pull request. The agent found this on
# 2026-09-04 and rated it BLOCKING; nothing fixed it.
PATH="$stub_dir:$PATH" bash "$mg" receipt 7 abcdef1234 "guard mutated, went red" >/dev/null 2>&1
cp "$MAINTAINER_STATE/receipts/7.json" "$MAINTAINER_STATE/receipts/8.json"
make_stub gh "case \"\$*\" in
  *'auth switch'*) exit 0;;
  *'api user'*) echo testuser;;
  *reviewDecision*) echo APPROVED;;
  *headRefOid*) echo abcdef1234;;
  *mergeStateStatus*) echo CLEAN;;
  *'pr checks'*) echo '[{\"name\":\"rust\",\"bucket\":\"pass\"}]';;
esac"
out=$(PATH="$stub_dir:$PATH" bash "$mg" merge 8 2>&1); rc=$?
if [ "$rc" != 0 ] && grep -qi 'earned for #7\|names #7\|different pull request' <<<"$out"; then
    ok "a receipt copied onto another PR number is refused"
else
    bad "a receipt earned for #7 was accepted for #8 (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-110)"
fi
# And the signature check must not be what catches it: the bytes are untouched,
# so a signature failure here would mean the test is proving the wrong thing.
grep -q 'does not match its signature' <<<"$out" \
    && bad "the copied receipt was caught by the signature, which does not cover the filename" \
    || ok "and it is refused on the PR it names, not by a signature that still verifies"
rm -f "$MAINTAINER_STATE/receipts/7.json" "$MAINTAINER_STATE/receipts/8.json"

echo "== the merge gate validates the PR number before it becomes a path =="
# $pr builds "$RECEIPTS/$pr.json" and that path was interpolated raw into a
# python3 -c string, so a non-integer $pr was a path-traversal and code-injection
# surface in the security boundary. GitHub only issues integer PR numbers.
for bad in "1;rm -rf x" "../../etc/passwd" "1'\''))__import__('\''os'\'')" "1 2" "x"; do
    out=$(PATH="$stub_dir:$PATH" bash "$mg" merge "$bad" 2>&1); rc=$?
    if [ "$rc" != 0 ] && grep -q 'must be a positive integer' <<<"$out"; then
        ok "merge rejects a non-integer PR ('$bad')"
    else
        bad "merge accepted a non-integer PR ('$bad') rc=$rc: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-70)"
    fi
done
# Empty is rejected by ${1:?} before pr_or_die is reached; still a refusal.
out=$(PATH="$stub_dir:$PATH" bash "$mg" merge "" 2>&1); rc=$?
[ "$rc" != 0 ] && ok "merge rejects an empty PR argument" || bad "merge ran with no PR"
# And a real integer still reaches the receipt check rather than being rejected.
out=$(PATH="$stub_dir:$PATH" bash "$mg" merge 12345 2>&1); rc=$?
grep -q 'no verification receipt' <<<"$out" \
    && ok "a real integer PR passes validation and reaches the receipt check" \
    || bad "a valid PR number was rejected or mis-routed: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-70)"

echo "== the mutation cannot execute on the host =="
# verify applies $mutation with `sed -i` on the host, and GNU sed's e/w/r
# commands run shell and touch arbitrary files. A prompt-injected PR could steer
# the agent to pass `s/x/id/e`. sed --sandbox rejects e/w/r, so a mutation is a
# transform and nothing else. Asserted on the source because the exec path needs
# a real container run to exercise end to end; the flag is the guarantee.
if sc_case 1; then
grep -q 'sed --sandbox -i -e "$mutation" --' "$root/bin/maintainer-merge" \
    && ok "the host-side mutation runs sed in --sandbox mode" \
    || bad "the mutation runs sed without --sandbox, so an e-flag mutation execs on the host"
sc_end; fi
# And prove --sandbox actually blocks it on this machine, so the flag is not
# cargo-culted from a version that lacks it.
sbx="$stub_dir/sbx"; rm -rf "$sbx"; mkdir -p "$sbx"; echo hello > "$sbx/f"; rm -f "$sbx/SBXPWN"
sed --sandbox -i "s#hello#touch $sbx/SBXPWN#e" "$sbx/f" >/dev/null 2>&1
[ -f "$sbx/SBXPWN" ] && bad "sed --sandbox executed on this host; the guarantee does not hold here" \
    || ok "sed --sandbox refuses the e command on this host"

echo "== a receipt dies when production code moves under it =="
# The condition the whole gate rests on. Built against a real git repo, because
# the check is a real `git diff` and a stub would prove nothing.
gr="$stub_dir/repo"
git -C "$gr" init -q -b main 2>/dev/null
git -C "$gr" config user.email t@t; git -C "$gr" config user.name t
mkdir -p "$gr/crates/x/src" "$gr/docs"
echo "fn main() {}" > "$gr/crates/x/src/lib.rs"; echo "hello" > "$gr/docs/a.md"
git -C "$gr" add -A >/dev/null; git -C "$gr" commit -qm base
verified=$(git -C "$gr" rev-parse HEAD)

# Case A: only docs moved after the verified head. The receipt must survive.
echo "changed" > "$gr/docs/a.md"; git -C "$gr" add -A >/dev/null; git -C "$gr" commit -qm docs-only
docs_head=$(git -C "$gr" rev-parse HEAD)
git -C "$gr" branch -f "pr-7" "$docs_head" >/dev/null 2>&1
# Case B: production moved. The receipt must die.
echo "fn main() { changed() }" > "$gr/crates/x/src/lib.rs"; git -C "$gr" add -A >/dev/null; git -C "$gr" commit -qm prod
prod_head=$(git -C "$gr" rev-parse HEAD)
git -C "$gr" branch -f "pr-8" "$prod_head" >/dev/null 2>&1

# `git fetch origin refs/pull/N/head` has no origin here, so point the gate at
# local refs by giving it an origin that is the repo itself.
git -C "$gr" remote add origin "$gr" 2>/dev/null
git -C "$gr" update-ref "refs/pull/7/head" "$docs_head"
git -C "$gr" update-ref "refs/pull/8/head" "$prod_head"

gate_case() {  # $1 pr, $2 head, $3 expect-substring, $4 label
    PATH="$stub_dir:$PATH" bash "$mg" receipt "$1" "$verified" "mutation proved" >/dev/null 2>&1
    make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *reviewDecision*) echo APPROVED;;
      *headRefOid*) echo $2;;
      *mergeStateStatus*) echo CLEAN;;
      *'pr checks'*) echo '[{\"name\":\"rust\",\"bucket\":\"pass\"}]';;
      *'pr merge'*) echo MERGED_STUB;;
    esac"
    out=$(PATH="$stub_dir:$PATH" bash "$mg" merge "$1" 2>&1)
    if grep -q "$3" <<<"$out"; then ok "$4"; else bad "$4 (got: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90))"; fi
}
gate_case 7 "$docs_head" "receipt still applies" "docs-only movement keeps the receipt valid"
gate_case 8 "$prod_head" "production code changed" "production movement invalidates the receipt"


echo "== the merge gate reads mergeStateStatus, which it used to discard =="
# It fetched mergeStateStatus and never read the variable, which shellcheck
# found by noticing it was unused. BEHIND is the one that matters: the branch is
# not up to date with its base, so every green check describes a tree that is
# not the one being merged.
# The CLEAN case reaches the fetch, so #9 needs a ref like the others.
git -C "$gr" update-ref "refs/pull/9/head" "$verified"
ms_case() {  # $1 = mergeStateStatus, $2 = expected substring
    PATH="$stub_dir:$PATH" bash "$mg" receipt 9 "$verified" "mutation proved" >/dev/null 2>&1
    make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *reviewDecision*) echo APPROVED;;
      *headRefOid*) echo $verified;;
      *mergeStateStatus*) echo $1;;
      *'pr checks'*) echo '[{\"name\":\"rust\",\"bucket\":\"pass\"}]';;
      *'pr merge'*) echo MERGED_STUB;;
    esac"
    out=$(PATH="$stub_dir:$PATH" bash "$mg" merge 9 2>&1)
    if grep -q "$2" <<<"$out"; then ok "mergeStateStatus $1 -> $2"
    else bad "mergeStateStatus $1 did not produce '$2' (got: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90))"; fi
}
ms_case BEHIND  "BEHIND its base"
ms_case DIRTY   "conflicts"
ms_case BLOCKED "BLOCKED"
ms_case UNKNOWN "only CLEAN and HAS_HOOKS"
ms_case CLEAN   "receipt valid"
ms_case BEHIND  "maintainer-merge update 9"

echo "== maintainer-merge update brings an approved BEHIND pull request up to date, and nothing else =="
# The rule (maintainer decision, 2026-11-09): an approved pull request whose only
# missing step is "Update branch" is updated by the agent. Until then `merge`
# died on BEHIND, the deny wall denies `gh pr update-branch`, and the pull
# request waited for a human click the agent was able to make. The command is
# the GitHub API and nothing else: no checkout, no push, no hook. Every refusal
# below is a case where the answer is "do not touch the branch", and a fact the
# command cannot read refuses instead of passing.
real_git="$(command -v git)"
ut="$stub_dir/ut"; ut_bin="$ut/bin"
ut_head="a1b2c3d4e5f60718293a4b5c6d7e8f9012345678"   # what the head becomes after the update
mkdir -p "$ut_bin"
cat > "$ut_bin/gh" <<'GHEOF'
#!/usr/bin/env bash
# A data-driven gh: every answer is a file under $UT_DIR. Anything it has no
# file for exits 99, loudly, so a call the code makes that nobody planned for
# fails the case instead of returning an empty string.
D="${UT_DIR:?}"
printf '%s\n' "$*" >> "$D/calls"
case "$*" in
  *"auth switch"*) exit 0 ;;
  *"api user"*) echo "${UT_WHO:-testuser}"; exit 0 ;;
  *update-branch*)
      case "$*" in *"-X PUT"*) ;; *) echo "stub: update-branch without -X PUT: $*" >&2; exit 99 ;; esac
      if [ -f "$D/put-rc" ]; then cat "$D/put-out"; exit "$(cat "$D/put-rc")"; fi
      : > "$D/put-done"; echo '{"message":"Updating pull request branch."}'; exit 0 ;;
  *"/reviews"*)
      if [ -f "$D/reviews-rc" ]; then echo "gh: HTTP 502" >&2; exit 1; fi
      cat "$D/reviews"; exit 0 ;;
  *"--json files"*) cat "$D/files" 2>/dev/null || echo "docs/a.md"; exit 0 ;;
  *"pr view"*)
      if [ -f "$D/put-done" ]; then
          if [ -f "$D/view-fail-after" ]; then echo "gh: HTTP 502" >&2; exit 1; fi
          n=$(( $(cat "$D/n" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$D/n"
          f="$D/after.$n"; [ -f "$f" ] || f="$(find "$D" -name 'after.*' | sort -V | tail -1)"
          cat "$f"
      else
          if [ -f "$D/view-fail" ]; then echo "gh: HTTP 502" >&2; exit 1; fi
          cat "$D/pr.json"
      fi
      exit 0 ;;
  *) echo "stub gh: unplanned call: $*" >&2; exit 99 ;;
esac
GHEOF
cat > "$ut_bin/git" <<GITEOF
#!/usr/bin/env bash
printf '%s\n' "\$*" >> "\${UT_DIR:?}/git.log"
exec "$real_git" "\$@"
GITEOF
chmod +x "$ut_bin/gh" "$ut_bin/git"

# pr.json: state decision login is_bot cross can_modify head
ut_pr() {
    printf '{"headRefOid":"%s","mergeStateStatus":"%s","reviewDecision":"%s","author":{"login":"%s","is_bot":%s},"isCrossRepository":%s,"maintainerCanModify":%s,"state":"OPEN"}\n' \
        "$7" "$1" "$2" "$3" "$4" "$5" "$6"
}
ut_reset() {  # $1 = pr number, $2 = the head the receipt is for and the PR is at
    local pr="$1" h="${2:-$verified}"
    rm -rf "$ut/d"; mkdir -p "$ut/d"; export UT_DIR="$ut/d"
    ut_pr BEHIND APPROVED contrib false false false "$h" > "$ut/d/pr.json"
    printf '{"user":{"login":"testuser"},"state":"APPROVED","commit_id":"%s"}\n' "$h" > "$ut/d/reviews"
    ut_pr BLOCKED APPROVED contrib false false false "$ut_head" > "$ut/d/after.1"
    git -C "$gr" update-ref "refs/pull/$pr/head" "$h"
    rm -rf "$MAINTAINER_STATE/leases" "$MAINTAINER_STATE/receipts/$pr.json"
    PATH="$ut_bin:$stub_dir:$PATH" bash "$mg" receipt "$pr" "$h" "mutation proved" >/dev/null 2>&1
    : > "$ut/d/calls"; : > "$ut/d/git.log"
}
ut_run() {  # $1 = pr number; extra env goes in front of the call
    PATH="$ut_bin:$stub_dir:$PATH" MAINTAINER_UPDATE_READBACK_WAIT=0 \
        MAINTAINER_UPDATE_READBACK_READS=4 timeout 60 bash "$mg" update "$1" 2>&1
}
ut_put_calls() { grep -c 'update-branch' "$ut/d/calls" || true; }
ut_expect() {  # $1 label, $2 expected rc, $3 grep pattern in the output ('' for none), $4 puts expected
    local label="$1" want="$2" pat="$3" puts="$4"
    [ "$rc" = "$want" ] || { bad "$label: exit $rc, wanted $want: $(tr '\n' ' ' <<<"$out" | cut -c1-200)"; return; }
    if [ -n "$pat" ] && ! grep -qE -- "$pat" <<<"$out"; then
        bad "$label: output lacks '$pat': $(tr '\n' ' ' <<<"$out" | cut -c1-200)"; return
    fi
    [ "$(ut_put_calls)" = "$puts" ] || { bad "$label: $(ut_put_calls) update-branch call(s), wanted $puts"; return; }
    ok "$label"
}
ut_run_rc() { out="$(ut_run "$1")"; rc=$?; }

# --- the happy path, and every fact it must report -------------------------------
ut_reset 31
ut_run_rc 31
ut_expect "an approved BEHIND PR with a receipt and no lease is updated" 0 "new head ${ut_head:0:8}" 1
grep -qF "expected_head_sha=$verified" "$ut/d/calls" \
    && ok "the PUT carries expected_head_sha, so a head that moved refuses at GitHub" \
    || bad "the PUT has no expected_head_sha of the head that was checked: $(cat "$ut/d/calls")"
grep -qF -- "-X PUT repos/o/r/pulls/31/update-branch" "$ut/d/calls" \
    && ok "it is the REST update-branch call on this repository's pull request" \
    || bad "wrong endpoint: $(cat "$ut/d/calls")"
grep -qiE 'required checks.*re-?run|re-?run' <<<"$out" && grep -qiE 'merge.*refuse.*green|until.*green' <<<"$out" \
    && ok "it says the required checks must re-run and that merge refuses until they are green" \
    || bad "the report does not say checks must re-run: $(tr '\n' ' ' <<<"$out" | cut -c1-200)"
grep -qiE 'still APPROVED|approval stands' <<<"$out" \
    && ok "it prints that reviewDecision is still APPROVED after the update" \
    || bad "no word on the approval after the update: $(tr '\n' ' ' <<<"$out" | cut -c1-200)"
grep -q "${verified:0:8}" <<<"$out" && grep -q "${ut_head:0:8}" <<<"$out" \
    && ok "and it prints the old and the new head, read back from GitHub" \
    || bad "old/new head not both printed"
# The shim logs "-C <path> <subcommand> ...", so strip the -C pair first.
git_subs="$(sed -E 's/^(-C [^ ]+ )?([^ ]+).*/\2/' "$ut/d/git.log" | sort -u)"
subs="$(tr '\n' ' ' <<<"$git_subs")"
[ -n "$git_subs" ] || bad "the git shim saw nothing, so the no-write check below is vacuous"
bad_git="$(grep -xE 'push|checkout|switch|commit|clone|worktree|reset|rebase|pull|merge|cherry-pick|apply|am|add|restore|stash|branch-write' <<<"$git_subs" || true)"
[ -z "$bad_git" ] && ok "update runs no git that writes or checks out ($subs)" \
    || bad "update ran git write verbs: $bad_git"
grep -qE 'pr merge|pr update-branch' "$ut/d/calls" \
    && bad "update called gh pr merge or gh pr update-branch" \
    || ok "and it never calls gh pr merge or gh pr update-branch"

# --- 1. mergeStateStatus is exactly BEHIND --------------------------------------
for st in CLEAN HAS_HOOKS; do
    ut_reset 32; ut_pr "$st" APPROVED contrib false false false "$verified" > "$ut/d/pr.json"
    ut_run_rc 32; ut_expect "$st is a no-op success: nothing to update" 0 "nothing to update" 0
done
for st in DIRTY BLOCKED UNKNOWN UNSTABLE DRAFT ""; do
    ut_reset 32; ut_pr "$st" APPROVED contrib false false false "$verified" > "$ut/d/pr.json"
    ut_run_rc 32
    ut_expect "mergeStateStatus '$st' refuses and updates nothing" 1 "mergeStateStatus|conflict" 0
done
ut_reset 32; ut_pr DIRTY APPROVED contrib false false false "$verified" > "$ut/d/pr.json"
ut_run_rc 32; ut_expect "DIRTY names the conflicts, which are never resolved by us" 1 "conflicts" 0

# --- 2. Dependabot's pull requests are rebased by Dependabot --------------------
for who in "app/dependabot" "dependabot[bot]" "Dependabot"; do
    ut_reset 33; ut_pr BEHIND APPROVED "$who" true false false "$verified" > "$ut/d/pr.json"
    ut_run_rc 33
    ut_expect "a $who pull request is refused, naming maintainer deps rebase" 1 "maintainer deps rebase 33" 0
done
ut_reset 33; ut_pr BEHIND APPROVED "dependabot[bot]" false false false "$verified" > "$ut/d/pr.json"
ut_run_rc 33; ut_expect "the bot login is enough even when is_bot is not reported" 1 "maintainer deps rebase 33" 0

# --- 3. this account approved this head, and the overall decision is APPROVED ---
for d in REVIEW_REQUIRED CHANGES_REQUESTED ""; do
    ut_reset 34; ut_pr BEHIND "$d" contrib false false false "$verified" > "$ut/d/pr.json"
    ut_run_rc 34; ut_expect "reviewDecision '$d' is not APPROVED: refused" 1 "reviewDecision" 0
done
ut_reset 34
printf '{"user":{"login":"testuser"},"state":"APPROVED","commit_id":"%s"}\n' "$docs_head" > "$ut/d/reviews"
ut_run_rc 34; ut_expect "an approval of an older head does not count" 1 "approv" 0
ut_reset 34
printf '{"user":{"login":"somebody"},"state":"APPROVED","commit_id":"%s"}\n' "$verified" > "$ut/d/reviews"
ut_run_rc 34; ut_expect "an approval by another account does not count" 1 "testuser" 0
ut_reset 34; : > "$ut/d/reviews"
ut_run_rc 34; ut_expect "no review at all from this account refuses" 1 "approv" 0
ut_reset 34
{ printf '{"user":{"login":"testuser"},"state":"APPROVED","commit_id":"%s"}\n' "$verified"
  printf '{"user":{"login":"testuser"},"state":"CHANGES_REQUESTED","commit_id":"%s"}\n' "$verified"; } > "$ut/d/reviews"
ut_run_rc 34; ut_expect "this account's latest verdict being CHANGES_REQUESTED refuses" 1 "approv" 0
ut_reset 34
{ printf '{"user":{"login":"testuser"},"state":"CHANGES_REQUESTED","commit_id":"%s"}\n' "$verified"
  printf '{"user":{"login":"testuser"},"state":"APPROVED","commit_id":"%s"}\n' "$verified"; } > "$ut/d/reviews"
ut_run_rc 34; ut_expect "a later APPROVED over an earlier CHANGES_REQUESTED stands" 0 "new head" 1
ut_reset 34
printf '{"user":{"login":"TestUser"},"state":"APPROVED","commit_id":"%s"}\n' "$verified" > "$ut/d/reviews"
ut_run_rc 34; ut_expect "GitHub logins compare without case" 0 "new head" 1
ut_reset 34; : > "$ut/d/reviews-rc"; touch "$ut/d/reviews-rc"
ut_run_rc 34; ut_expect "reviews that cannot be read refuse instead of passing" 1 "review" 0
ut_reset 34; printf 'this is not json\n' > "$ut/d/reviews"
ut_run_rc 34; ut_expect "reviews that do not parse refuse instead of passing" 1 "review" 0
ut_reset 34; printf 'not json at all' > "$ut/d/pr.json"
ut_run_rc 34; ut_expect "a pull request that does not parse refuses" 1 "from GitHub, so I will not touch its branch" 0
ut_reset 34; touch "$ut/d/view-fail"
ut_run_rc 34; ut_expect "a gh that fails on the pull request view refuses" 1 "from GitHub, so I will not touch its branch" 0
ut_reset 34; ut_pr BEHIND APPROVED contrib false false false "not-a-sha" > "$ut/d/pr.json"
ut_run_rc 34; ut_expect "a head that is not 40 hex refuses, so expected_head_sha is never a guess" 1 "not a 40-hex" 0
ut_reset 34
out="$(UT_WHO=someone-else PATH="$ut_bin:$stub_dir:$PATH" timeout 60 bash "$mg" update 34 2>&1)"; rc=$?
ut_expect "the wrong gh account refuses" 1 "not testuser" 0

# --- 4. a receipt for this head, through the same check merge uses -------------
ut_reset 35; rm -f "$MAINTAINER_STATE/receipts/35.json"
ut_run_rc 35; ut_expect "no receipt: refused, since nobody verified this branch" 1 "no verification receipt" 0
ut_reset 35; sed -i 's/mutation proved/edited by hand/' "$MAINTAINER_STATE/receipts/35.json"
ut_run_rc 35; ut_expect "a receipt edited after it was signed is refused" 1 "signature" 0
ut_reset 35
out="$(MAINTAINER_IN_RUN=x/review PATH="$ut_bin:$stub_dir:$PATH" timeout 60 bash "$mg" update 35 2>&1)"; rc=$?
ut_expect "inside a run an asserted receipt is refused, as merge refuses it" 1 "not 'observed'" 0
# The receipt was earned at $verified. The pull request now sits at a head whose
# production code moved: updating a branch whose code nobody verified helps nobody.
ut_reset 35 "$verified"; ut_pr BEHIND APPROVED contrib false false false "$prod_head" > "$ut/d/pr.json"
printf '{"user":{"login":"testuser"},"state":"APPROVED","commit_id":"%s"}\n' "$prod_head" > "$ut/d/reviews"
git -C "$gr" update-ref "refs/pull/35/head" "$prod_head"
ut_run_rc 35; ut_expect "production code moved since the receipt: refused" 1 "production code changed" 0
ut_reset 35 "$verified"; ut_pr BEHIND APPROVED contrib false false false "$docs_head" > "$ut/d/pr.json"
printf '{"user":{"login":"testuser"},"state":"APPROVED","commit_id":"%s"}\n' "$docs_head" > "$ut/d/reviews"
git -C "$gr" update-ref "refs/pull/35/head" "$docs_head"
ut_run_rc 35; ut_expect "a docs-only head move keeps the receipt valid, as it does for merge" 0 "receipt still applies" 1

# The receipt must reach every production path the pull request changes. This
# receipt was written by hand, so it names no suite, and a diff that touches
# production is then covered by nothing.
# Only a part-receipt (verify-part) can fall short of the whole diff; a whole
# receipt is covered by construction. This one names no suite.
if sc_case 1; then
ut_reset 35; printf 'crates/x/src/lib.rs\n' > "$ut/d/files"
python3 - "$MAINTAINER_STATE/receipts/35.json" <<'PYEOF2'
import json, sys
d = json.load(open(sys.argv[1])); d.pop("signature", None); d["partial"] = True
json.dump(d, open(sys.argv[1], "w"), indent=2)
PYEOF2
bash -c '. "$1"; receipt_seal "$2"' _ "$mg" "$MAINTAINER_STATE/receipts/35.json" >/dev/null 2>&1
ut_run_rc 35; ut_expect "a part-receipt that does not reach the production paths is refused" 1 "was earned by" 0
sc_end; fi
if sc_case 1; then
ut_reset 35; printf 'docs/a.md\n' > "$ut/d/files"
python3 - "$MAINTAINER_STATE/receipts/35.json" <<'PYEOF2'
import json, sys
d = json.load(open(sys.argv[1])); d.pop("signature", None); d["partial"] = True
json.dump(d, open(sys.argv[1], "w"), indent=2)
PYEOF2
bash -c '. "$1"; receipt_seal "$2"' _ "$mg" "$MAINTAINER_STATE/receipts/35.json" >/dev/null 2>&1
ut_run_rc 35; ut_expect "and a part-receipt that names no suite speaks for nothing, whatever the diff" 1 "names no suite" 0
sc_end; fi
# `merge` asks the same question through the same function.
if sc_case 1; then
ut_reset 35; printf 'crates/x/src/lib.rs\n' > "$ut/d/files"
python3 - "$MAINTAINER_STATE/receipts/35.json" <<'PYEOF2'
import json, sys
d = json.load(open(sys.argv[1])); d.pop("signature", None); d["partial"] = True
json.dump(d, open(sys.argv[1], "w"), indent=2)
PYEOF2
bash -c '. "$1"; receipt_seal "$2"' _ "$mg" "$MAINTAINER_STATE/receipts/35.json" >/dev/null 2>&1
out="$(PATH="$ut_bin:$stub_dir:$PATH" timeout 60 bash "$mg" merge 35 2>&1)"; rc=$?
ut_expect "merge refuses the same part-receipt for the same reason" 1 "was earned by" 0
sc_end; fi

# --- 5. another actor's lease ----------------------------------------------------
mkdir_leases() { mkdir -p "$MAINTAINER_STATE/leases"; }
ut_reset 36; mkdir_leases
printf '{"holder":"run:o/review","kind":"run","note":"reviewing","acquired":%s,"renewed":%s,"ttl_seconds":3600}\n' \
    "$(date +%s)" "$(date +%s)" > "$MAINTAINER_STATE/leases/pr-36.json"
ut_run_rc 36; ut_expect "a live lease held by another actor (lease check exit 1) refuses" 1 "leased" 0
ut_reset 36; mkdir_leases; printf '{not json' > "$MAINTAINER_STATE/leases/pr-36.json"
ut_run_rc 36; ut_expect "an unreadable lease (lease check exit 2) refuses, never read as free" 1 "lease" 0
ut_reset 36; mkdir_leases
printf '{"holder":"run:o/review","kind":"run","note":"old","acquired":1,"renewed":%s,"ttl_seconds":100}\n' \
    "$(( $(date +%s) - 5000 ))" > "$MAINTAINER_STATE/leases/pr-36.json"
ut_run_rc 36; ut_expect "an expired lease does not block" 0 "new head" 1
rm -rf "$MAINTAINER_STATE/leases"

# --- 6. can GitHub let us push to the head? --------------------------------------
ut_reset 37; ut_pr BEHIND APPROVED contrib false true false "$verified" > "$ut/d/pr.json"
ut_run_rc 37
ut_expect "a fork PR that forbids maintainer edits exits 3, and never tries the API" 3 "the author must press Update branch; they have not allowed maintainer edits" 0
ut_reset 37; ut_pr BEHIND APPROVED contrib false true true "$verified" > "$ut/d/pr.json"
ut_run_rc 37; ut_expect "a fork PR that allows maintainer edits is updated" 0 "new head" 1
ut_reset 37; ut_pr BEHIND APPROVED contrib false false false "$verified" > "$ut/d/pr.json"
ut_run_rc 37; ut_expect "a same-repo branch needs no maintainerCanModify" 0 "new head" 1
ut_reset 37; ut_pr BEHIND APPROVED contrib false true null "$verified" > "$ut/d/pr.json"
ut_run_rc 37; ut_expect "maintainerCanModify null on a fork is not true: exit 3" 3 "the author must press Update branch" 0
# Runtime refusal by the API itself: the same answer, the same distinct code.
ut_reset 38
echo 1 > "$ut/d/put-rc"; printf '%s\n' '{"message":"Validation Failed: user doesn'"'"'t have permission to update head repository"}' 'gh: Validation Failed (HTTP 422)' > "$ut/d/put-out"
ut_run_rc 38; ut_expect "the API refusing for want of permission on the head repository exits 3" 3 "the author must press Update branch" 1
ut_reset 38
echo 1 > "$ut/d/put-rc"; printf '%s\n' '{"message":"Resource not accessible by integration"}' 'gh: Resource not accessible by integration (HTTP 403)' > "$ut/d/put-out"
ut_run_rc 38; ut_expect "an HTTP 403 from the update call exits 3" 3 "the author must press Update branch" 1
ut_reset 38
echo 1 > "$ut/d/put-rc"; printf '%s\n' '{"message":"Validation Failed: expected head sha didn'"'"'t match current head ref."}' 'gh: Validation Failed (HTTP 422)' > "$ut/d/put-out"
ut_run_rc 38; ut_expect "a head that moved (422 on expected_head_sha) is exit 1, not a permission refusal" 1 "moved after I read it" 1
ut_reset 38
echo 1 > "$ut/d/put-rc"; printf '%s\n' 'gh: Server Error (HTTP 500)' > "$ut/d/put-out"
ut_run_rc 38; ut_expect "any other failure of the update call is exit 1 and says the update did not happen" 1 "HTTP 500" 1

# --- read-back: the PUT's 202 is not the update --------------------------------
ut_reset 39
ut_pr BEHIND APPROVED contrib false false false "$verified" > "$ut/d/after.1"
ut_pr BEHIND APPROVED contrib false false false "$verified" > "$ut/d/after.2"
ut_run_rc 39; ut_expect "a head that never changes is never reported as updated" 1 "not confirmed|could not confirm|never" 1
grep -qiE '^ *updated|brought up to date|new head' <<<"$out" \
    && bad "reported success from the PUT's own answer: $(tr '\n' ' ' <<<"$out" | cut -c1-160)"
ut_reset 39
ut_pr CLEAN APPROVED contrib false false false "$verified" > "$ut/d/after.1"
ut_run_rc 39; ut_expect "a CLEAN state on the OLD head is not an update either: the head must move" 1 "not confirmed" 1
ut_reset 39
ut_pr BEHIND APPROVED contrib false false false "$ut_head" > "$ut/d/after.1"
ut_run_rc 39; ut_expect "a new head that GitHub still calls BEHIND is not success" 1 "BEHIND|not confirmed|could not confirm" 1
ut_reset 39
ut_pr UNKNOWN APPROVED contrib false false false "$ut_head" > "$ut/d/after.1"
ut_run_rc 39; ut_expect "UNKNOWN after the update means not yet, and never settles into success" 1 "UNKNOWN|not confirmed|could not confirm" 1
ut_reset 39
ut_pr UNKNOWN APPROVED contrib false false false "$ut_head" > "$ut/d/after.1"
ut_pr BEHIND APPROVED contrib false false false "$ut_head" > "$ut/d/after.2"
ut_pr CLEAN APPROVED contrib false false false "$ut_head" > "$ut/d/after.3"
ut_run_rc 39; ut_expect "the read-back is retried: UNKNOWN, then BEHIND, then CLEAN is success" 0 "new head ${ut_head:0:8}" 1
[ "$(cat "$ut/d/n")" = 3 ] && ok "it read GitHub three times to get there" || bad "read count $(cat "$ut/d/n"), wanted 3"
ut_reset 39
ut_pr "" APPROVED contrib false false false "$ut_head" > "$ut/d/after.1"
ut_run_rc 39; ut_expect "a new head with an empty merge state is not success either" 1 "not confirmed" 1
ut_reset 39; touch "$ut/d/view-fail-after"
ut_run_rc 39; ut_expect "a read-back that cannot be read is unconfirmed, not success" 1 "not confirmed|could not confirm|could not read" 1
ut_reset 39
for i in 1 2 3 4 5 6; do ut_pr UNKNOWN APPROVED contrib false false false "$ut_head" > "$ut/d/after.$i"; done
ut_pr CLEAN APPROVED contrib false false false "$ut_head" > "$ut/d/after.7"
ut_run_rc 39; ut_expect "the retries are bounded: a seventh read is never made" 1 "not confirmed|could not confirm" 1
[ "$(cat "$ut/d/n")" = 4 ] && ok "it stopped after MAINTAINER_UPDATE_READBACK_READS reads" || bad "read $(cat "$ut/d/n") times, wanted 4"

# The bound comes from lib/defaults.env when nothing overrides it.
ut_reset 39
for i in 1 2 3 4 5 6 7 8; do ut_pr UNKNOWN APPROVED contrib false false false "$ut_head" > "$ut/d/after.$i"; done
out="$(PATH="$ut_bin:$stub_dir:$PATH" MAINTAINER_UPDATE_READBACK_WAIT=0 timeout 60 bash "$mg" update 39 2>&1)"; rc=$?
want_reads="$(sed -n 's/^UPDATE_READBACK_READS=//p' "$root/lib/defaults.env")"
case "$want_reads" in ''|*[!0-9]*) bad "lib/defaults.env has no numeric UPDATE_READBACK_READS ('$want_reads')" ;; esac
[ "$rc" = 1 ] && [ -n "$want_reads" ] && [ "$(cat "$ut/d/n" 2>/dev/null)" = "$want_reads" ] \
    && ok "without an override it reads UPDATE_READBACK_READS times from lib/defaults.env, then refuses" \
    || bad "the default read count is not honoured (rc=$rc, reads $(cat "$ut/d/n"))"

# --- an update can dismiss the approval; say so, and approve nothing ------------
ut_reset 40
ut_pr BLOCKED REVIEW_REQUIRED contrib false false false "$ut_head" > "$ut/d/after.1"
ut_run_rc 40
ut_expect "a dismissed approval after the update is reported, and the update still counts" 0 "re-?post" 1
grep -qiE 'three-dot|tree-merge|git diff' <<<"$out" && grep -qiE 'production|workflow' <<<"$out" \
    && ok "it says the review may be re-posted only if git diff of the verified head shows no production or workflow change" \
    || bad "no condition on re-posting the review: $(tr '\n' ' ' <<<"$out" | cut -c1-240)"
grep -qE 'review.*--approve|pr review' "$ut/d/calls" \
    && bad "update approved on its own" || ok "and it never approves"

# --- POST=off rehearses ---------------------------------------------------------
ut_reset 41
out="$(MAINTAINER_POST=off PATH="$ut_bin:$stub_dir:$PATH" timeout 60 bash "$mg" update 41 2>&1)"; rc=$?
ut_expect "POST=off rehearses: every condition holds, and nothing is changed" 0 "POST=off" 0
grep -q 'update-branch' <<<"$out" && ok "the rehearsal prints what it would do" || bad "the rehearsal says nothing of the call"
ut_reset 41; ut_pr DIRTY APPROVED contrib false false false "$verified" > "$ut/d/pr.json"
out="$(MAINTAINER_POST=off PATH="$ut_bin:$stub_dir:$PATH" timeout 60 bash "$mg" update 41 2>&1)"; rc=$?
ut_expect "a rehearsal still refuses what a live run would refuse" 1 "conflicts" 0

# --- the argument, the dispatcher and the pointer from merge ---------------------
for badpr in "1;rm -rf x" "../x" "x" ""; do
    out="$(PATH="$ut_bin:$stub_dir:$PATH" timeout 60 bash "$mg" update "$badpr" 2>&1)"; rc=$?
    if [ -z "$badpr" ]; then
        [ "$rc" != 0 ] && ok "update rejects an empty pull request argument" || bad "update ran with no pull request"
    elif [ "$rc" != 0 ] && grep -q 'must be a positive integer' <<<"$out"; then
        ok "update rejects the pull request argument '$badpr'"
    else bad "update did not reject '$badpr' by name (rc=$rc): $(tr '\n' ' ' <<<"$out" | cut -c1-100)"; fi
done
out="$(PATH="$ut_bin:$stub_dir:$PATH" timeout 60 bash "$mg" 2>&1)"; rc=$?
grep -q 'update <pr>' <<<"$out" && [ "$rc" = 64 ] \
    && ok "the usage line lists update" || bad "usage does not list update (rc=$rc)"
rm -rf "$ut" "$MAINTAINER_STATE/receipts/3"[1-9].json "$MAINTAINER_STATE/receipts/4"[01].json
ut_reset 42
out="$(PROD_GLOBS="" PATH="$ut_bin:$stub_dir:$PATH" timeout 60 bash "$mg" merge 42 2>&1)"; rc=$?
ut_expect "and merge refuses the same way (the check is shared)" 1 "PROD_GLOBS" 0
out="$(PROD_GLOBS="" PATH="$ut_bin:$stub_dir:$PATH" timeout 60 bash "$mg" update 42 2>&1)"; rc=$?
ut_expect "a profile that declares no PROD_GLOBS gets no update, as it gets no merge" 1 "PROD_GLOBS" 0
rm -rf "$ut" "$MAINTAINER_STATE/receipts/42.json" "$MAINTAINER_STATE/leases"
# Later sections count on `refs/pull/31/head` not existing ("cannot fetch #31"),
# so leave the scratch repository as this section found it.
for n in 31 32 33 34 35 36 37 38 39 40 41 42; do git -C "$gr" update-ref -d "refs/pull/$n/head" 2>/dev/null; done
unset UT_DIR

