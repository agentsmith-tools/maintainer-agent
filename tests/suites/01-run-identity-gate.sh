#!/usr/bin/env bash
# Part of the offline suite: the run program: stub tokens, identity gate, pinned token, prompt, cadence, notifications, audit, install guards.
# tests/run-tests.sh sources this file in one shell, after its own preamble and after
# the parts before it, so every helper and variable it reads comes from there. It is
# sourced, never run on its own, and carries a shebang so the shell sweeps pick it up.

# The parts share one shell. These names are set by the driver or by an earlier part;
# declare only tells the linter they are read here, and assigns and changes nothing.
declare root
declare run_is_rust
declare stub_dir
declare suite_files
declare suite_root

echo "== every stub token is one the exit scrub can remove =="
# Every `gh auth token` stub in this file prints a token the exit scrub can remove.
# A shorter one makes the Rust run stop at the pin (decision D15) and the case never
# reaches what it is about, while the bash run carries on, so the two builds fail
# in different places for a reason nobody reads. The count proves the scan looked.
# The one stub that prints a short token on purpose is the D15 case, and it says `shorttoken`.
stub_token_total="$(grep -hE "auth token['\"]\*\) echo " "${suite_files[@]}" | grep -vc 'shorttoken')"
stub_token_short="$(grep -hE "auth token['\"]\*\) echo [^;[:space:]'\"]{1,15};;" "${suite_files[@]}" | grep -vc 'shorttoken')"
if [ "${stub_token_total:-0}" -ge 10 ] && [ "${stub_token_short:-1}" = 0 ]; then
    ok "every gh auth token stub prints a token of 16 characters or more ($stub_token_total stubs scanned)"
else
    bad "a gh auth token stub prints a token under 16 characters, or the scan found only ${stub_token_total:-0} stubs (short: ${stub_token_short:-?})"
fi

echo "== argument handling =="
out=$(PATH="$stub_dir:$PATH" bash "$root/lib/run.sh" nosuchprofile review 2>&1); rc=$?
check "unknown profile is rejected" "$rc" "64"
out=$(PATH="$stub_dir:$PATH" bash "$root/lib/run.sh" sysknife nosuchtask 2>&1); rc=$?
check "unknown task is rejected" "$rc" "64"

echo "== the GitHub identity gate =="
# The gate is the single most important refusal: a write under the wrong
# account stamps an employer-linked identity onto personal open-source work.
# The gate sits BEHIND backend_check, and profiles/*/settings.json is generated
# at install time rather than committed. So this block used to run with no wall
# on disk, die at "backend claude unusable: missing .../settings.json", and
# collect the rc=1 that backend_check returns. Both cases asserted rc=1, so both
# passed without the identity gate ever executing. The tell was already in the
# code: a conditional `grep -q 'refusing to run' && printf` that printed
# nothing for the life of the test. A wall is rendered here, and the message is
# asserted, because an exit code alone can come from anywhere upstream.
gate_settings="$stub_dir/gate-settings.json"
printf '{"permissions":{"deny":[]}}\n' > "$gate_settings"
gate_test() {  # $1 = what `gh api user` reports, $2 = expected rc, $3 = message
    make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'api user'*) [ -n \"$1\" ] && echo \"$1\"; exit 0;; esac"
    out=$(PATH="$stub_dir:$PATH" HOME="$stub_dir" MAINTAINER_SETTINGS="$gate_settings" \
        MAINTAINER_GH_TRIES=2 MAINTAINER_GH_BACKOFF=0 \
        bash "$root/lib/run.sh" sysknife review 2>&1); rc=$?
    if [ "$rc" = "$2" ]; then ok "gh='${1:-<empty>}' -> rc=$2"; else bad "gh='${1:-<empty>}' expected rc=$2, got $rc"; fi
    if grep -qi "$3" <<<"$out"; then
        ok "gh='${1:-<empty>}' -> the gate's own message, not an upstream failure"
    else
        bad "gh='${1:-<empty>}' exited $rc without saying '$3': $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-100)"
    fi
}
gate_test "someone-else" 1  "refusing to run"
gate_test ""             75 "could not reach"

# GitHub logins are case-insensitive, and no two accounts differ only in case.
# So an API answer of `VladimirRott` for a profile naming `vladimirrott` is the
# right account, and both identity checks refused it as the wrong one. The
# refusal of a genuinely different account is asserted above and must still
# hold; this is the other direction.
make_stub gh "case \"\$*\" in
  *'auth switch'*) exit 0;;
  *'auth token'*) echo ghp_OracleStubTokenAAAAAAAAAAAAAAAAAAAAA;;
  *'api user'*) echo VladimirRott;;
esac"
pin_repo="$stub_dir/pin-repo-case"; rm -rf "$pin_repo"; git init -q -b main "$pin_repo"
out=$(PATH="$stub_dir:$PATH" HOME="$stub_dir" MAINTAINER_SETTINGS="$gate_settings" \
    MAINTAINER_STATE_DIR="$stub_dir/pin-state-case" MAINTAINER_REPO_PATH="$pin_repo" \
    MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review 2>&1)
if ! grep -q 'refusing to run\|could not pin\|resolves to' <<<"$out" \
   && grep -q 'refresh failed' <<<"$out"; then
    ok "a login differing only in case passes both identity checks"
else
    bad "the right account in another case was refused: $(printf '%s' "$out"|tr '\n' ' '|cut -c1-110)"
fi

# The run pins its identity by TOKEN, not just by the active-account switch. The
# keyring holds more than one account, one of them a work account, and the active
# one flipped mid-run and posted to a personal repo under the wrong name. With
# GH_TOKEN exported for the whole run, no gh call can select another account.
# Proven on the host: `gh auth switch` to another user is ignored while GH_TOKEN
# is set. These cases drive the guard.
#
# Happy path: a token is available and resolves to the account, so the run pins
# and proceeds past the gate.
make_stub gh "case \"\$*\" in
  *'auth switch'*) exit 0;;
  *'auth token'*) echo ghp_OracleStubTokenAAAAAAAAAAAAAAAAAAAAA;;
  *'api user'*) echo vladimirrott;;
esac"
# A checkout that already exists, so run.sh skips provisioning. Naming neither
# REPO_PATH nor REPO_ORIGIN made this case `git clone` lacs-project/sysknife
# from github.com on every run of a suite whose own README calls it offline: it
# cost minutes, and on 2026-09-08 a slow network turned it into a failure that
# had nothing to do with the identity gate it tests.
pin_repo="$stub_dir/pin-repo-1"; rm -rf "$pin_repo"; git init -q -b main "$pin_repo"
out=$(PATH="$stub_dir:$PATH" HOME="$stub_dir" MAINTAINER_SETTINGS="$gate_settings" \
    MAINTAINER_STATE_DIR="$stub_dir/pin-state-1" MAINTAINER_REPO_PATH="$pin_repo" \
    MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review 2>&1)
# The pin message lands in the run log; the observable on stdout is that the run
# got PAST the identity gate (a token was pinned and verified) and on to refresh,
# rather than refusing at the pin.
if ! grep -q 'could not pin\|resolves to' <<<"$out" \
   && grep -q 'refresh failed' <<<"$out"; then
    ok "the run pins its gh identity by token and proceeds past the gate"
else
    bad "the run refused at the identity pin: $(printf '%s' "$out"|tr '\n' ' '|cut -c1-90)"
fi

# No token available: the run cannot guarantee its identity, so it refuses
# rather than risk posting as whoever the active account flips to.
make_stub gh "case \"\$*\" in
  *'auth switch'*) exit 0;;
  *'auth token'*) exit 1;;
  *'api user'*) echo vladimirrott;;
esac"
out=$(PATH="$stub_dir:$PATH" HOME="$stub_dir" MAINTAINER_SETTINGS="$gate_settings" \
    MAINTAINER_STATE_DIR="$stub_dir/pin-state-2" \
    MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review 2>&1); rc=$?
[ "$rc" != 0 ] && grep -q 'could not pin a token' <<<"$out" \
    && ok "with no token to pin, the run refuses rather than trust the active account" \
    || bad "the run proceeded without a pinned token (rc=$rc)"

# A token that resolves to a DIFFERENT account than expected is refused: this is
# the wrong-identity-mid-config case the whole guard exists for.
make_stub gh "case \"\$*\" in
  *'auth switch'*) exit 0;;
  *'auth token'*) echo ghp_SomeoneElsesStubTokenBBBBBBBBBBBBBBB;;
  *'api user'*) if [ \"\$GH_TOKEN\" = ghp_SomeoneElsesStubTokenBBBBBBBBBBBBBBB ]; then echo someoneelse; else echo vladimirrott; fi;;
esac"
out=$(PATH="$stub_dir:$PATH" HOME="$stub_dir" MAINTAINER_SETTINGS="$gate_settings" \
    MAINTAINER_STATE_DIR="$stub_dir/pin-state-3" \
    MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review 2>&1); rc=$?
[ "$rc" != 0 ] && grep -q "resolves to 'someoneelse'" <<<"$out" \
    && ok "a pinned token that resolves to another account is refused" \
    || bad "a token for the wrong account was accepted (rc=$rc): $(printf '%s' "$out"|tr '\n' ' '|cut -c1-90)"

echo "== a pinned token the exit scrub cannot remove stops the run (decision D15) =="
# The exit scrub refuses a token under 16 characters, because a needle that short
# would rewrite every file it walks. lib/run.sh ignored the refusal, ran on and left
# the token in the state directory. Plan decision D15 stops the Rust run at the pin
# instead, before the helper or the agent starts. The bash program keeps the old
# behaviour until it is retired, so the case holds both builds: each side fails when
# the build under test behaves like the other one. Every other stub in this file
# prints a token of 16 characters or more, so a case reaches what it is about.
d15_home="$stub_dir/d15home"; rm -rf "$d15_home"; mkdir -p "$d15_home/.local/bin" "$d15_home/state/state" "$d15_home/state/runs"
d15_repo="$d15_home/repo"; git init -q -b main "$d15_repo"
d15_calls="$d15_home/helper.calls"; d15_agent="$d15_home/agent.calls"; : > "$d15_calls"; : > "$d15_agent"
cat > "$d15_home/.local/bin/maintainer" <<HELPERSTUB
#!/bin/sh
printf '%s\n' "\$1" >> "$d15_calls"
case "\$1" in
  start) echo "=== stub 2031-02-03T04-05-review ==="; echo "write the run report to \$MAINTAINER_STATE/runs/2031-02-03T04-05-review.md" ;;
esac
exit 0
HELPERSTUB
chmod +x "$d15_home/.local/bin/maintainer"
make_stub claude "echo called >> $d15_agent; exit 0"
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'auth token'*) echo shorttoken;; *'api user'*) echo vladimirrott;; esac"
printf 'before shorttoken after\n' > "$d15_home/state/leak.txt"
out=$(PATH="$stub_dir:$PATH" HOME="$d15_home" MAINTAINER_SETTINGS="$gate_settings" \
    MAINTAINER_STATE_DIR="$d15_home/state" MAINTAINER_REPO_PATH="$d15_repo" \
    MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review 2>&1); rc=$?
set -- "$d15_home"/state/logs/*-review.log; d15_log="$1"
d15_msg="the token pinned for vladimirrott is shorter than 16 characters, so the exit scrub could not remove it from what this run leaves behind; refusing to run. See $d15_log"
if [ "$run_is_rust" = 1 ]; then
    [ "$rc" = 1 ] && [ -f "$d15_log" ] && grep -qF -- "$d15_msg" <<<"$out" \
        && ok "a pinned token under 16 characters is refused with the scrub's own message (D15)" \
        || bad "a short pinned token was not refused with the D15 message (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-140)"
    ! grep -qx 'start' "$d15_calls" && [ ! -s "$d15_agent" ] \
        && ok "and the run stopped before the helper started a run or the agent ran (D15)" \
        || bad "a refused short token still started a run or the agent: $(tr '\n' ' ' < "$d15_calls") $(tr '\n' ' ' < "$d15_agent")"
else
    ! grep -q 'shorter than 16 characters' <<<"$out" && grep -qx 'start' "$d15_calls" \
        && ok "the bash run goes on past a pinned token under 16 characters (D15, fixed in the Rust build)" \
        || bad "the bash run no longer ignores a short pinned token (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-140)"
    grep -q 'shorttoken' "$d15_home/state/leak.txt" \
        && ok "and the short token stays in what the run left behind (D15)" \
        || bad "the bash run scrubbed a token under 16 characters, which its scrubber refuses"
fi
if [ ! -f "$d15_log" ]; then
    bad "the short-token run left no log to check for the token (D15)"
elif grep -q 'shorttoken' <<<"$out" || grep -q 'shorttoken' "$d15_log"; then
    bad "a run printed or logged the short pinned token (D15)"
else
    ok "and neither the output nor the log carries the token (D15)"
fi
make_stub claude 'echo "claude stub invoked" >&2; exit 0'
# A network outage and a wrong account are different emergencies with different
# fixes, and the gate said the same sentence for both. Measured 2026-09-05: the
# 09:15 sysknife review and the 09:20 magent review both died on
# "error connecting to api.github.com", and the alert that reached the human
# read "gh is authenticated as 'unknown', not vladimirrott", which sends you
# hunting for a token problem during a network outage.
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'api user'*) echo 'error connecting to api.github.com' >&2; exit 1;; esac"
out=$(PATH="$stub_dir:$PATH" HOME="$stub_dir" MAINTAINER_SETTINGS="$gate_settings" \
    MAINTAINER_GH_TRIES=2 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review 2>&1); rc=$?
[ "$rc" = 75 ] && ok "an unreachable API exits 75 (try again), not 1 (refused)" \
    || bad "an unreachable API exits $rc, indistinguishable from a wrong account"
grep -qi 'could not reach\|unreachable' <<<"$out" \
    && ok "and the alert names the network rather than the identity" \
    || bad "the alert blames the identity for a network failure: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-100)"
grep -q "authenticated as" <<<"$out" \
    && bad "the unreachable path still reports an account it never read" \
    || ok "and it does not name an identity it never got an answer about"
# A transient outage is transient, so the gate retries before giving up. A wrong
# account is not transient and must never be retried into.
tries="$stub_dir/tries"; rm -f "$tries"
cat > "$stub_dir/gh" <<GHSTUB
#!/bin/sh
case "\$*" in
  *'auth switch'*) exit 0 ;;
  *'api user'*)
      echo x >> "$tries"
      if [ "\$(wc -l < "$tries")" -ge 2 ]; then echo vladimirrott; exit 0; fi
      exit 1 ;;
esac
GHSTUB
chmod +x "$stub_dir/gh"
PATH="$stub_dir:$PATH" HOME="$stub_dir" MAINTAINER_SETTINGS="$gate_settings" \
    MAINTAINER_GH_TRIES=3 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review >/dev/null 2>&1
[ -f "$tries" ] && [ "$(wc -l < "$tries")" -ge 2 ] \
    && ok "a first failure is retried rather than ending the run" \
    || bad "the gate gave up on the first network error"
wrong="$stub_dir/wrong"; rm -f "$wrong"
cat > "$stub_dir/gh" <<GHSTUB
#!/bin/sh
case "\$*" in
  *'auth switch'*) exit 0 ;;
  *'api user'*) echo x >> "$wrong"; echo someone-else; exit 0 ;;
esac
GHSTUB
chmod +x "$stub_dir/gh"
PATH="$stub_dir:$PATH" HOME="$stub_dir" MAINTAINER_SETTINGS="$gate_settings" \
    MAINTAINER_GH_TRIES=3 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review >/dev/null 2>&1
[ "$(wc -l < "$wrong")" = 1 ] \
    && ok "and a wrong account is refused at once, never retried into" \
    || bad "the gate retried a wrong account $(wc -l < "$wrong") times"
# A third emergency, and the gate reported it as the first. On 2026-10-19 the
# keyring token for vladimirrott was revoked. `gh api user --jq .login` then
# prints GitHub's JSON error body on STDOUT and exits 1, and the gate read
# stdout without the exit code, so the alert said
#   gh is authenticated as '{ "message": "Bad credentials", ... }', not vladimirrott
# which names a JSON blob as an account. The fix is a re-login, not a wait and
# not an account switch, so the alert must say that.
rejected="$stub_dir/rejected"; rm -f "$rejected"
cat > "$stub_dir/gh" <<GHSTUB
#!/bin/sh
case "\$*" in
  *'auth switch'*) exit 0 ;;
  *'api user'*)
      echo x >> "$rejected"
      printf '{\n  "message": "Bad credentials",\n  "status": "401"\n}'
      echo 'gh: Bad credentials (HTTP 401)' >&2
      exit 1 ;;
esac
GHSTUB
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" HOME="$stub_dir" MAINTAINER_SETTINGS="$gate_settings" \
    MAINTAINER_GH_TRIES=3 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review 2>&1); rc=$?
[ "$rc" = 1 ] && ok "a rejected token is refused (1), not retried as an outage (75)" \
    || bad "a rejected token exits $rc"
grep -q 'Bad credentials\|authenticated as .{' <<<"$out" \
    && bad "the alert quotes GitHub's error body as if it were a login: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-100)" \
    || ok "and the alert does not quote the error body as an account"
grep -q 'rejected' <<<"$out" && grep -q 'gh auth login' <<<"$out" \
    && ok "and it says the token was rejected and how to log in again" \
    || bad "the alert does not name the rejected token or the fix: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-100)"
[ "$(wc -l < "$rejected")" = 1 ] \
    && ok "and a rejected token is asked about once, never retried into" \
    || bad "the gate asked about a rejected token $(wc -l < "$rejected") times"



echo "== the prompt states the posting state it was given, never a written-in one =="
# 2026-09-07, magent review. `profiles/magent/prompts/common-preamble.md` said
# "This profile is at **POST=off**: publish nothing", and profile.env says
# POST="${MAINTAINER_POST:-on}". run.sh injected no notice, because it only
# injected one for POST=off, so the static sentence was the only thing in the
# prompt that spoke about posting and it was wrong. The run believed it, called
# `maintainer file-issue` five times, and filed issues #27 to #31 for real. It
# caught itself by reading the variable afterwards.
#
# A prompt may describe what POST=off does. It may not assert which one is in
# force: that is the environment's to say, and only run.sh can read it.
for _pf in "$root"/profiles/*/prompts/*.md "$root/lib/preamble-core.md"; do
    [ -e "$_pf" ] || continue
    # Newlines folded first. The sentence that caused this wrapped between
    # "This profile" and "is at **POST=off**", so a line-based grep passed it
    # and the check reported clean over the exact file it was written for.
    if tr '\n' ' ' < "$_pf" | grep -qiE 'this profile +is at [^.]*POST='; then
        bad "$(basename "$(dirname "$(dirname "$_pf")")")/$(basename "$_pf") asserts a posting state the environment decides"
    else
        ok "$(basename "$(dirname "$(dirname "$_pf")")")/$(basename "$_pf") claims no posting state of its own"
    fi
done
# Driven through the real assembly, both ways. --show-prompt exists so the
# preview is the same assembly the run uses.
pp_settings="$stub_dir/postprompt.json"
printf '{"permissions":{"deny":[]}}\n' > "$pp_settings"
pp() { MAINTAINER_POST="$1" MAINTAINER_SETTINGS="$pp_settings" \
       bash "$root/lib/run.sh" --show-prompt magent review 2>&1; }
pp_on="$(pp on)"
grep -qi 'this run POSTS' <<<"$pp_on" \
    && ok "at POST=on the prompt says so in its own section" \
    || bad "a posting run is told nothing about posting"
grep -q 'REHEARSAL' <<<"$pp_on" \
    && bad "a posting run is handed the rehearsal notice" \
    || ok "and it is not handed the rehearsal notice"
pp_off="$(pp off)"
grep -q 'This run is a REHEARSAL' <<<"$pp_off" \
    && ok "at POST=off the rehearsal notice is still injected" \
    || bad "the rehearsal notice was lost"
grep -qi 'this run POSTS' <<<"$pp_off" \
    && bad "a rehearsal is told it posts" \
    || ok "and a rehearsal is not told it posts"
# The two must never both appear, which is the failure the run actually hit:
# one source saying off while the other said on.
grep -ciE 'this run POSTS|This run is a REHEARSAL' <<<"$pp_on" | grep -qx 1 \
    && ok "exactly one posting notice reaches the prompt" \
    || bad "the prompt carries more than one answer to whether it posts"

echo "== the cadence gate is re-read after the lock, not only before it =="
# 2026-09-07, measured. `install.sh --timers` fired a Persistent=true catch-up
# for `issues` at 20:09. A second run launched at 20:23 passed the cadence gate
# (the stamp was still two days old), then waited eleven minutes on the lock,
# acquired it at 20:34 one second after the first run promoted the stamp, and
# started a full duplicate pass. It only stopped because the backend hit a
# session limit.
#
# The gate is checked at run.sh:123 and the lock at run.sh:301, so every run
# queued behind another decides its cadence on a stamp that the run ahead of it
# is about to replace. Adding a catch-up slot to every timer makes this more
# likely, not less, which is what turned it up.
rh="$stub_dir/racehome"; rm -rf "$rh"; mkdir -p "$rh/state" "$rh/repo"
git -C "$rh/repo" init -q -b main 2>/dev/null
# The lock sits behind backend_check, and profiles/*/settings.json is generated
# at install time rather than committed, so without a wall on disk this dies at
# "backend claude unusable" and never reaches the code under test. The identity
# gate block above records the same trap.
race_settings="$rh/settings.json"
printf '{"permissions":{"deny":[]}}\n' > "$race_settings"
# A stamp old enough to pass the gate on entry.
touch -d "100 hours ago" "$rh/state/last-review.json"
# This block needs the REAL flock: $stub_dir carries `make_stub flock 'exit 0'`,
# which returns success without taking anything, so a run under it never queues
# and the case it is meant to reproduce cannot happen. Its own stub directory
# holds only the gh stub, which exists so that a run reaching past the gate
# fails locally instead of touching the network.
rbin="$rh/bin"; mkdir -p "$rbin"
printf '#!/usr/bin/env bash\ncase "$*" in *"auth switch"*) exit 0;; *"api user"*) echo someone-else;; esac\n' \
    > "$rbin/gh"; chmod +x "$rbin/gh"
# And the backend, because backend_check runs before the lock and CI's
# no-deployed-tree job has no claude on PATH. This passed locally on a machine
# that happens to have one installed, which is the whole reason that job exists.
printf '#!/usr/bin/env bash\nexit 0\n' > "$rbin/claude"; chmod +x "$rbin/claude"
# Hold the lock, so the run gets past the gate and then blocks exactly where the
# real one did.
exec 8>"$rh/run.lock"
flock 8
# `timeout` so a release that does not happen costs a minute rather than the
# profile's a-hour LOCK_WAIT.
( PATH="$rbin:$PATH" MAINTAINER_STATE_DIR="$rh" MAINTAINER_REPO_PATH="$rh/repo" \
  MAINTAINER_SETTINGS="$race_settings" MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
  timeout 60 bash "$root/lib/run.sh" sysknife review > "$rh/out" 2>&1 ) &
race_pid=$!
# Give it time to clear the gate and reach the lock. Not a race of its own: if
# it has not blocked yet the stamp below is simply newer still.
sleep 3
# The run ahead of it finishes and promotes the stamp.
touch "$rh/state/last-review.json"
flock -u 8; exec 8>&-
wait "$race_pid"; race_rc=$?
race_out="$(cat "$rh/out")"
grep -q '^skipped: review last ran' <<<"$race_out" \
    && ok "a run that waited on the lock re-reads the cadence gate and skips" \
    || bad "a queued run duplicates the pass that just finished: $(printf '%s' "$race_out" | tr '\n' ' ' | cut -c1-100)"
[ "$race_rc" = 0 ] \
    && ok "and it exits 0, because being superseded is not a failure" \
    || bad "a superseded run exits $race_rc, which the audit reads as a broken task"
# MAINTAINER_FORCE has to survive the second check, or the override stops working
# for exactly the runs that need it.
touch "$rh/state/last-review.json"
out=$(PATH="$rbin:$PATH" MAINTAINER_STATE_DIR="$rh" MAINTAINER_REPO_PATH="$rh/repo" \
      MAINTAINER_SETTINGS="$race_settings" MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 MAINTAINER_FORCE=1 \
      bash "$root/lib/run.sh" sysknife review 2>&1)
grep -q '^skipped: review last ran' <<<"$out" \
    && bad "MAINTAINER_FORCE is defeated by the second gate check" \
    || ok "MAINTAINER_FORCE still overrides both checks"

echo "== a coverage refusal names the suites that would have covered it =="
# The gate refused sysknife#370 because no single suite covers *.md + *.py +
# *.sh, which is correct: a receipt from a suite that does not run the changed
# code proves nothing. The message listed only the changed paths, so the
# reviewer had to grep every verify.d/*.sh by hand to learn that docs.sh covers
# the .md files, shell.sh the .sh file, and nothing at all covers .py. A refusal
# that does not say what would fix it is a refusal people work around.
covp="$stub_dir/covprof"; rm -rf "$covp"; mkdir -p "$covp/verify.d"
cat > "$covp/verify.d/docs.sh" <<'SUITE'
# shellcheck shell=bash
suite_covers() { case "$1" in *.md) return 0 ;; esac; return 1; }
SUITE
cat > "$covp/verify.d/shell.sh" <<'SUITE'
# shellcheck shell=bash
suite_covers() { case "$1" in *.sh) return 0 ;; esac; return 1; }
SUITE
msg="$(PROFILE_DIR="$covp" MAINTAINER_PROFILE=cov MAINTAINER_SLUG=o/r \
    MAINTAINER_REPO=/tmp MAINTAINER_ACCOUNT=t MAINTAINER_MERGE_SOURCE_ONLY=1 \
    bash -c '. "'"$root"'/bin/maintainer-merge"; pick_suite "" a.md b.sh c.py' 2>&1 || true)"
grep -q 'a\.md.*docs' <<<"$msg" \
    && ok "the refusal says which suite covers each path it can place" \
    || bad "the refusal lists paths without naming the suites that cover them: $(printf '%s' "$msg" | tr '\n' ' ' | cut -c1-110)"
grep -qE 'c\.py.*(no suite|NONE|nothing)' <<<"$msg" \
    && ok "and singles out the path no suite covers at all" \
    || bad "the uncovered path is not distinguished from the covered ones"
grep -q 'docs.*shell\|shell.*docs' <<<"$msg" \
    && ok "and names the union that would cover the rest" \
    || bad "the message does not say that two suites together would cover the others"

echo "== the log and the run it belongs to carry the same name =="
# run.sh stamps the log from its own start time, before it takes the lock.
# `maintainer start` mints the run id after the lock is acquired. With no
# contention both land in the same minute and nothing shows. On 2026-09-05 a
# review waited nine minutes behind an issues sweep and wrote
# logs/2026-09-05T10-47-review.log beside runs/2026-09-05T10-56-review.md, and
# the audit read that as two broken runs: one that started and never reported,
# and one whose report quotes no executed command, because its transcript was
# filed under the other name. Driven end to end, not grepped for a `mv`.
lh="$stub_dir/loghome"; rm -rf "$lh"; mkdir -p "$lh/.local/bin"
cat > "$lh/.local/bin/maintainer" <<'HELPERSTUB'
#!/bin/sh
case "$1" in
  start)
      echo "=== stub 2031-02-03T04-05-review ==="
      echo "write the run report to $MAINTAINER_STATE/runs/2031-02-03T04-05-review.md"
      ;;
  finish|abort|failed|clear) exit 0 ;;
esac
HELPERSTUB
chmod +x "$lh/.local/bin/maintainer"
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'auth token'*) echo ghp_OracleStubTokenAAAAAAAAAAAAAAAAAAAAA;; *'api user'*) echo vladimirrott; exit 0;; esac"
make_stub claude "exit 0"
# Its OWN state dir, not the suite's shared one: other run.sh-driven tests write
# real-timestamped review logs into the shared root, and this asserts an exact
# log name, so a neighbour's log made it flake under load.
lognm_state="$lh/state"; rm -rf "$lognm_state"; mkdir -p "$lognm_state"
# A checkout that already exists, so step 0b skips provisioning. Without this the
# run `git clone`s lacs-project/sysknife from github.com, and on 2026-09-09 a
# slow link made it die before `maintainer start` ever minted a run id, which
# reads here as the rename having failed.
lognm_repo="$lh/repo"; rm -rf "$lognm_repo"; git init -q -b main "$lognm_repo"
PATH="$stub_dir:$PATH" HOME="$lh" MAINTAINER_SETTINGS="$gate_settings" \
    MAINTAINER_STATE_DIR="$lognm_state" MAINTAINER_REPO_PATH="$lognm_repo" \
    MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review >/dev/null 2>&1
lgs="$(find "$lognm_state/logs" -name '*review.log' -printf '%f\n' 2>/dev/null | sort | tr '\n' ' ')"
if [ "$lgs" = "2031-02-03T04-05-review.log " ]; then
    ok "the log is filed under the run id the helper minted"
else
    bad "a lock wait splits one run across two names; logs present: ${lgs:-none}"
fi


echo "== an identity that could not be read is not an identity that was verified =="
# Issue #28. Two identity checks landed in the same range. Both refuse when `gh`
# answers with the wrong login, and both PROCEEDED when `gh` could not answer at
# all, which is the shape docs/lessons.md keeps recording: a guard that answers
# when it could not ask.
#
# Site one, lib/run.sh, after the token is pinned. The stub below fails
# `api user` only once GH_TOKEN is exported, which is exactly the call made
# after the pin, so the run reaches the second check having passed the first.
idh="$stub_dir/idhome"; rm -rf "$idh"; mkdir -p "$idh"
id_repo="$idh/repo"; git init -q -b main "$id_repo"
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'auth switch'*) exit 0;;
  *'auth token'*) echo ghp_OracleStubTokenAAAAAAAAAAAAAAAAAAAAA;;
  *'api user'*)
      # Before the pin GH_TOKEN is unset and the answer is good; after it, the
      # call cannot be answered at all.
      if [ -n "${GH_TOKEN:-}" ]; then echo "error connecting to api.github.com" >&2; exit 1; fi
      echo vladimirrott;;
esac
GHEOF
chmod +x "$stub_dir/gh"
idlog="$idh/state"; mkdir -p "$idlog"
out=$(PATH="$stub_dir:$PATH" HOME="$idh" MAINTAINER_SETTINGS="$gate_settings" \
      MAINTAINER_STATE_DIR="$idlog" MAINTAINER_REPO_PATH="$id_repo" \
      MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
      bash "$root/lib/run.sh" sysknife review 2>&1); rc=$?
[ "$rc" != 0 ] && ok "a pinned token that cannot be resolved stops the run" \
    || bad "the run proceeded on an identity nothing confirmed (rc=$rc)"
grep -qi 'identity' <<<"$out" \
    && ok "and the refusal names the identity rather than blaming something downstream" \
    || bad "the refusal does not mention the identity: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90)"
# The log must not claim a verification that did not happen. This is the line
# that made the defect invisible: it printed unconditionally.
if grep -rq 'pinned by token for the whole run' "$idlog"/logs/*.log 2>/dev/null; then
    bad "the log records the identity as pinned after a check that never ran"
else
    ok "and the log does not record a pin it could not confirm"
fi
# Site two, `maintainer file-issue`, whose comment says it re-checks rather than
# trusting the inherited state. Driven at POST=on against a fake slug.
fih="$stub_dir/fihome"; rm -rf "$fih"; mkdir -p "$fih"
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'api user'*) echo "error connecting to api.github.com" >&2; exit 1;;
  *'issue list'*) echo '[]';;
  *'issue create'*) echo 'https://github.com/o/r/issues/5678';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$fih" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=vladimirrott MAINTAINER_PROFILE=fileissue MAINTAINER_POST=on \
      python3 "$root/bin/maintainer" file-issue --title "probe" --body "b" 2>&1); rc=$?
[ "$rc" != 0 ] && ok "file-issue refuses when it cannot confirm whose name goes on the issue" \
    || bad "an issue was filed under a name nothing verified (rc=$rc)"
grep -q 'issues/5678' <<<"$out" \
    && bad "the issue was created despite the unverifiable identity" \
    || ok "and nothing was created"
# The twin, so the check is not simply refusing everything: a gh that answers
# with the right account still files.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'api user'*) echo vladimirrott;;
  *'issue list'*) echo '[]';;
  *'issue create'*) echo 'https://github.com/o/r/issues/5679';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$fih" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=vladimirrott MAINTAINER_PROFILE=fileissue MAINTAINER_POST=on \
      python3 "$root/bin/maintainer" file-issue --title "probe two" --body "b" 2>&1)
grep -q 'issues/5679' <<<"$out" \
    && ok "and a confirmed identity still files the issue" \
    || bad "the identity check now blocks a legitimate filing: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90)"
rm -f "$stub_dir/gh"


echo "== the test suite cannot reach the real desktop notifier =="
# Reported with a screenshot on 2026-09-10: a critical toast reading
#   maintainer · sysknife review · FAILED
#   gh is authenticated as 'someone-else', not vladimirrott; refusing to run.
#   See /tmp/tmp.ZBwRBxn8fL/racehome/logs/2026-09-10T10-14-review.log
# That is not a run. `someone-else` is this file's gh stub and `racehome` is a
# fixture under $stub_dir, so every single execution of this suite fired a real
# critical popup at whoever was sitting in front of the machine. Two blocks
# build a minimal PATH ($rbin) that deliberately excludes $stub_dir, and the
# notify-send stub lives in $stub_dir, so the real /usr/bin/notify-send won.
#
# A stub in $rbin would fix those two and the next minimal-PATH block would
# bring it back. This is the second toast complaint in one session, so the
# pattern changes rather than the site.
grep -qE '^export MAINTAINER_NOTIFY=off' "$suite_root/tests/run-tests.sh" \
    && ok "the suite turns notifications off for the whole process" \
    || bad "nothing stops a case with a minimal PATH from reaching the real notifier"
if sc_case 1; then
grep -q 'MAINTAINER_NOTIFY' "$root/lib/run.sh" \
    && ok "and run.sh honours it" \
    || bad "run.sh notifies regardless of what the caller asked for"
sc_end; fi
# systemd's OnFailure handler is a second notifier, and lesson 66 is that one
# failure firing two popups is how somebody learns to dismiss both.
if sc_case 1; then
grep -q 'MAINTAINER_NOTIFY' "$root/platform/linux/alert.sh" \
    && ok "and so does the systemd OnFailure alert, so one setting covers both" \
    || bad "alert.sh still pops a critical toast on a host that asked for none"
sc_end; fi
# The audit trail is not optional. Turning the popup off must not turn off the
# record of what failed.
if sc_case 1; then
grep -q 'alerts.log' "$root/platform/linux/alert.sh" \
    && ok "and the alert trail is written before the popup is even considered" \
    || bad "the switch could silence the record as well as the desktop"
sc_end; fi

# Behaviourally, in both directions, because a switch that silences everything
# would be worse than the popups: the timers are supposed to notify.
ntf="$stub_dir/notifyhome"; rm -rf "$ntf"; mkdir -p "$ntf/state"
git init -q -b main "$ntf/repo"
cat > "$stub_dir/notify-send" <<'NEOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$NOTIFY_LOG"
NEOF
chmod +x "$stub_dir/notify-send"
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'api user'*) echo someone-else;; esac"
: > "$ntf/off.log"
NOTIFY_LOG="$ntf/off.log" PATH="$stub_dir:$PATH" HOME="$ntf" MAINTAINER_NOTIFY=off \
    MAINTAINER_SETTINGS="$gate_settings" MAINTAINER_STATE_DIR="$ntf/state" \
    MAINTAINER_REPO_PATH="$ntf/repo" MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review >/dev/null 2>&1
[ ! -s "$ntf/off.log" ] && ok "with notifications off a refusing run reaches no desktop" \
    || bad "MAINTAINER_NOTIFY=off still fired: $(head -1 "$ntf/off.log")"
: > "$ntf/on.log"
NOTIFY_LOG="$ntf/on.log" PATH="$stub_dir:$PATH" HOME="$ntf" MAINTAINER_NOTIFY=on \
    MAINTAINER_SETTINGS="$gate_settings" MAINTAINER_STATE_DIR="$ntf/state" \
    MAINTAINER_REPO_PATH="$ntf/repo" MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review >/dev/null 2>&1
[ -s "$ntf/on.log" ] && ok "and with it unset the same run still notifies, so the timers keep working" \
    || bad "notifications are off by default now, which silences the unattended runs"

echo "== a failure that retries itself does not raise a critical alert =="
# Reported 2026-09-09: constant critical toasts from the magent timers. Six unit
# failures on 2026-09-08 produced up to twelve, because each one alerts twice:
# lib/run.sh calls `alert` (notify-send -u critical) and then exits non-zero, and
# systemd's OnFailure fires platform/linux/alert.sh, which sends a second one.
#
# Every one of those six was `gh could not reach GitHub in 3 tries` or a lock
# wait expiring: transient, already recorded in failed-<task>.json, already
# reported by maintainer-doctor, and already retried by the catch-up slot each
# timer now carries. A critical popup six times a day for a condition that heals
# itself is how somebody learns to dismiss the one that matters.
tn="$stub_dir/toasthome"; rm -rf "$tn"; mkdir -p "$tn"
tn_repo="$tn/repo"; git init -q -b main "$tn_repo"
tn_state="$tn/state"; mkdir -p "$tn_state"
# Lock contention is a transient skip, not a broken run. This drives the
# lock-timeout branch all the way through run.sh instead of counting its text.
make_stub flock 'exit 1'
out=$(PATH="$stub_dir:$PATH" HOME="$tn" MAINTAINER_NOTIFY=off \
      MAINTAINER_SETTINGS="$gate_settings" MAINTAINER_STATE_DIR="$tn_state" \
      MAINTAINER_REPO_PATH="$tn_repo" MAINTAINER_FORCE=1 \
      bash "$root/lib/run.sh" sysknife review 2>&1); rc=$?
[ "$rc" = 75 ] && ok "a held run lock exits 75 so the next slot can retry" \
    || bad "lock contention no longer exits 75 (rc=$rc): $out"
make_stub flock 'exit 0'
# notify-send records its arguments instead of reaching a desktop.
cat > "$stub_dir/notify-send" <<'NEOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$NOTIFY_LOG"
NEOF
chmod +x "$stub_dir/notify-send"
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'api user'*) echo 'error connecting to api.github.com' >&2; exit 1;; esac"
: > "$tn/notify.log"
out=$(NOTIFY_LOG="$tn/notify.log" PATH="$stub_dir:$PATH" HOME="$tn" MAINTAINER_NOTIFY=on \
      MAINTAINER_SETTINGS="$gate_settings" MAINTAINER_STATE_DIR="$tn_state" \
      MAINTAINER_REPO_PATH="$tn_repo" MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
      bash "$root/lib/run.sh" sysknife review 2>&1); rc=$?
[ "$rc" = 75 ] && ok "an unreachable API still exits 75" \
    || bad "the transient path no longer exits 75 (rc=$rc)"
grep -q 'u critical' "$tn/notify.log" \
    && bad "a transient failure still raises a critical desktop alert" \
    || ok "and raises no critical desktop alert"
[ -s "$tn/notify.log" ] \
    && ok "it still says something, at a lower urgency" \
    || bad "the failure now reaches the desktop not at all, which is the other failure mode"
# The record has to survive, or this traded noise for silence. Asserted through
# a stub helper rather than the marker file, because the real `maintainer failed`
# needs a deployed profile and this case is about run.sh still calling it.
mkdir -p "$tn/.local/bin"
cat > "$tn/.local/bin/maintainer" <<'MEOF'
#!/usr/bin/env bash
[ "$1" = failed ] && printf '%s
' "$*" >> "$MARKER_LOG"
exit 0
MEOF
chmod +x "$tn/.local/bin/maintainer"
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'api user'*) echo 'error connecting to api.github.com' >&2; exit 1;; esac"
: > "$tn/marker.log"; : > "$tn/notify.log"
MARKER_LOG="$tn/marker.log" NOTIFY_LOG="$tn/notify.log" PATH="$stub_dir:$PATH" HOME="$tn" MAINTAINER_NOTIFY=on \
    MAINTAINER_SETTINGS="$gate_settings" MAINTAINER_STATE_DIR="$tn_state" \
    MAINTAINER_REPO_PATH="$tn_repo" MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review >/dev/null 2>&1
grep -q '^failed review' "$tn/marker.log" \
    && ok "and the failure marker is still recorded" \
    || bad "suppressing the toast also suppressed the record"
grep -q 'could not reach GitHub' "$tn/marker.log" \
    && ok "with the reason the run recorded" \
    || bad "the marker does not carry the reason"
# A valid first identity check followed by an unreadable pinned-token identity
# is also transient. The GH_TOKEN check distinguishes the two API calls.
make_stub gh 'case "$*" in *"auth switch"*) exit 0;; *"auth token"*) echo ghp_OracleStubTokenAAAAAAAAAAAAAAAAAAAAA;; *"api user"*) if [ -n "${GH_TOKEN:-}" ]; then echo "error connecting to api.github.com" >&2; exit 1; else echo vladimirrott; fi;; esac'
out=$(PATH="$stub_dir:$PATH" HOME="$tn" MAINTAINER_NOTIFY=off \
      MAINTAINER_SETTINGS="$gate_settings" MAINTAINER_STATE_DIR="$tn_state" \
      MAINTAINER_REPO_PATH="$tn_repo" MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
      bash "$root/lib/run.sh" sysknife review 2>&1); rc=$?
[ "$rc" = 75 ] && ok "an unresolved pinned-token identity exits 75" \
    || bad "an unresolved pinned-token identity no longer exits 75 (rc=$rc): $out"
# A real refusal is NOT transient and must still be critical: a wrong identity is
# somebody else's name on this account and no retry fixes it.
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'api user'*) echo someone-else;; esac"
: > "$tn/notify.log"
NOTIFY_LOG="$tn/notify.log" PATH="$stub_dir:$PATH" HOME="$tn" MAINTAINER_NOTIFY=on \
    MAINTAINER_SETTINGS="$gate_settings" MAINTAINER_STATE_DIR="$tn_state" \
    MAINTAINER_REPO_PATH="$tn_repo" MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review >/dev/null 2>&1
grep -q 'u critical' "$tn/notify.log" \
    && ok "a wrong identity is still critical, because no retry fixes it" \
    || bad "the wrong-account refusal was downgraded along with the transient ones"
# A backend usage-window failure with no report reaches the post-backend
# transient branch. Keep the helper local and make finish refuse the empty run.
mkdir -p "$tn/.local/bin"
cat > "$tn/.local/bin/maintainer" <<'MEOF'
case "$1" in
    start) printf '=== start 206-transient ===\n' ;;
    finish) printf 'no report\n'; exit 1 ;;
    *) exit 0 ;;
esac
MEOF
chmod +x "$tn/.local/bin/maintainer"
make_stub gh 'case "$*" in *"auth switch"*) exit 0;; *"auth token"*) echo ghp_OracleStubTokenAAAAAAAAAAAAAAAAAAAAA;; *"api user"*) echo vladimirrott;; esac'
make_stub claude 'printf "You\047ve hit your session limit; resets at 10:10pm\\n" >&2; exit 1'
out=$(PATH="$stub_dir:$PATH" HOME="$tn" MAINTAINER_NOTIFY=off \
      MAINTAINER_SETTINGS="$gate_settings" MAINTAINER_STATE_DIR="$tn_state" \
      MAINTAINER_REPO_PATH="$tn_repo" MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
      MAINTAINER_SCRIPTS="$tn/no-scripts" \
      bash "$root/lib/run.sh" sysknife review 2>&1); rc=$?
[ "$rc" = 75 ] && ok "a transient backend failure without a report exits 75" \
    || bad "the transient no-report branch no longer exits 75 (rc=$rc): $out"
make_stub claude 'echo "claude stub invoked" >&2; exit 0'
rm -f "$stub_dir/notify-send" "$stub_dir/gh"
make_stub notify-send 'exit 0'
# systemd must not fire OnFailure for the transient code either, or the second
# toast arrives whatever run.sh does about the first.
grep -q '^SuccessExitStatus=75$' "$root/platform/linux/maintainer@.service" \
    && ok "the unit treats 75 as success, so OnFailure does not fire a second alert" \
    || bad "systemd still calls a transient failure a unit failure"

echo "== the toasts three real aborted runs actually fired =="
# Measured, not imagined. Three sysknife review runs aborted between 17 and 21
# September and every one of them raised THREE critical popups: run.sh alerts
# for the backend exit, alerts again because no report was written, then exits
# 1 and systemd's OnFailure sends a third. Nine critical toasts in five days,
# for two distinct conditions, one of which healed by itself.
#
#   2026-09-17T21-14  Failed to authenticate: OAuth session expired and could not be refreshed
#   2026-09-18T11-15  (the same, the next morning, nothing suppressed the repeat)
#   2026-09-21T11-17  API Error: Can't reach the API server - check your internet or DNS (EAI_AGAIN)
tcl="$stub_dir/classify"; rm -rf "$tcl"; mkdir -p "$tcl"
printf 'API Error: Can%st reach the API server - check your internet or DNS (EAI_AGAIN)\n' "'" > "$tcl/dns.log"
printf 'Failed to authenticate: OAuth session expired and could not be refreshed\n' > "$tcl/oauth.log"
printf 'thread panicked at src/main.rs:12: assertion failed\n' > "$tcl/real.log"
# shellcheck disable=SC1090
if sc_case 1; then
tr_out=$( . "$root/lib/profile.sh" >/dev/null 2>&1; backend_transient_reason "$tcl/dns.log" ); tr_rc=$?
{ [ "$tr_rc" = 0 ] && grep -qi 'EAI_AGAIN' <<<"$tr_out"; } \
    && ok "a DNS failure reaching the API is classified as transient, quoting what matched" \
    || bad "EAI_AGAIN was called permanent, so it raised three critical toasts (rc=$tr_rc, '$tr_out')"
sc_end; fi
# shellcheck disable=SC1090
if sc_case 1; then
( . "$root/lib/profile.sh" >/dev/null 2>&1; backend_transient_reason "$tcl/oauth.log" >/dev/null ) \
    && bad "an expired OAuth session was called transient, so nobody is told to log in" \
    || ok "an expired login is NOT transient, because no retry fixes it"
sc_end; fi
# shellcheck disable=SC1090
if sc_case 1; then
( . "$root/lib/profile.sh" >/dev/null 2>&1; backend_transient_reason "$tcl/real.log" >/dev/null ) \
    && bad "a genuine panic was waved through as transient" \
    || ok "a genuine failure is still not transient"
sc_end; fi
# An expired login needs a person, so it stays critical. It also repeats on
# every timer until that person acts, which is the part that becomes noise.
# shellcheck disable=SC1090
if sc_case 1; then
( . "$root/lib/profile.sh" >/dev/null 2>&1; declare -F backend_needs_human_reason >/dev/null ) \
    && ok "there is a class for a failure that needs a person rather than a retry" \
    || bad "an expired login is lumped in with an unknown crash"
sc_end; fi
# shellcheck disable=SC1090
if sc_case 1; then
nh=$( . "$root/lib/profile.sh" >/dev/null 2>&1; backend_needs_human_reason "$tcl/oauth.log" 2>/dev/null ); nh_rc=$?
{ [ "$nh_rc" = 0 ] && grep -qi 'oauth' <<<"$nh"; } \
    && ok "and an expired OAuth session is in it, quoting what matched" \
    || bad "the expired-login shape is not recognised (rc=$nh_rc, '$nh')"
sc_end; fi
# shellcheck disable=SC1090
if sc_case 1; then
( . "$root/lib/profile.sh" >/dev/null 2>&1; backend_needs_human_reason "$tcl/dns.log" >/dev/null 2>&1 ) \
    && bad "a DNS blip is being reported as something a person must fix" \
    || ok "and a DNS blip is not"
sc_end; fi
# The toast has to say what to do. "Failed to authenticate" is the vocabulary of
# the thing that broke, not a message to a person.
if sc_case 1; then
grep -q 'login' <<< "$( . "$root/lib/run.sh" 2>/dev/null; true )" 2>/dev/null || true
grep -qE 'OAuth|authenticate' "$root/lib/run.sh" \
    && ok "run.sh translates an expired login into an instruction" \
    || bad "the raw 'Failed to authenticate' string is what reaches the desktop"
sc_end; fi

echo "== the audit names the cause, not whatever the log said last =="
# _why_it_died returned the last non-header line, which is a heuristic and was
# wrong for every aborted run in September: all three ended with
#   run.sh: line 441: maintainer_scrub_secret: command not found
# so `maintainer audit` reported a missing shell function as the reason three
# separate runs died, when two were an expired login and one was DNS. A wrong
# cause is worse than no cause, because somebody acts on it.
aud="$stub_dir/audithome"; rm -rf "$aud"; mkdir -p "$aud/logs" "$aud/runs"
cat > "$aud/logs/2031-01-01T00-00-review.log" <<'ALOG'
=== run.sh sysknife/review 2031-01-01T00:00:00-06:00 ===
maintainer=v0.0.0 backend=claude
API Error: Can't reach the API server - check your internet or DNS (EAI_AGAIN)
claude exited non-zero for 2031-01-01T00-00-review.
/home/x/.local/share/maintainer/run.sh: line 441: something_unrelated: command not found
ALOG
aout=$(MAINTAINER_STATE="$aud" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp MAINTAINER_PROFILE=t \
       python3 "$root/bin/maintainer" audit --all 2>&1)
grep -q 'api-unreachable\|transient' <<<"$aout" \
    && ok "the audit names the classified cause of a dead run" \
    || bad "the audit does not classify: $(printf '%s' "$aout" | tr '\n' ' ' | cut -c1-140)"
grep -q 'something_unrelated' <<<"$aout" \
    && bad "the audit still blames the last line of the log" \
    || ok "and does not blame the last line of the log"
# An unknown death must still be reported, and reported AS unknown.
cat > "$aud/logs/2031-01-02T00-00-review.log" <<'ALOG'
=== run.sh sysknife/review 2031-01-02T00:00:00-06:00 ===
the machine caught fire in a way nothing has a pattern for
ALOG
aout=$(MAINTAINER_STATE="$aud" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp MAINTAINER_PROFILE=t \
       python3 "$root/bin/maintainer" audit --all 2>&1)
grep -q 'unclassified' <<<"$aout" \
    && ok "a death matching no shape is reported as unclassified, not guessed at" \
    || bad "an unknown death was given a confident cause: $(printf '%s' "$aout" | tr '\n' ' ' | cut -c1-140)"
grep -q 'caught fire' <<<"$aout" \
    && ok "and carries its evidence line so somebody can add a shape for it" \
    || bad "the unclassified death carries no evidence"
# A list of thirty-one identical lines is one nobody reads twice; the same
# thirty-one grouped by cause is a decision. Sixteen of the real ones were a
# single unreachable host.
grep -q 'by cause:' <<<"$aout" \
    && ok "the audit groups the dead runs by cause" \
    || bad "the audit lists dead runs and never totals them by cause"
grep -qE 'match no shape|every one of them carries a named cause' <<<"$aout" \
    && ok "and says out loud how many it could not explain" \
    || bad "the audit does not report how many deaths are unexplained"

echo "== release-check says so when it read a CHANGELOG that is not the one you tag =="
# On 2026-09-22 release-check answered `digit: last` for a release whose
# Unreleased section drops Node 18 and 20 support. The heuristic was fine; the
# TREE was four commits behind, so the section it read did not contain the
# entry. Following that answer would have published a breaking change as a
# patch to everyone pinned to ^0.16.
#
# The existing guard asks whether HEAD is an ancestor of origin/main, which is
# TRUE for a stale checkout. Being behind is the common case and it read as
# healthy.
if sc_case 1; then
grep -q 'HEAD\.\.origin/main\|rev-list --count' "$root/bin/maintainer-repo" \
    && ok "release-check counts how far behind the tree is" \
    || bad "a checkout behind origin/main still reports a confident digit"
sc_end; fi
if sc_case 1; then
grep -qi 'behind' "$root/bin/maintainer-repo" \
    && ok "and says so in words, next to the verdict it affects" \
    || bad "nothing tells the reader the CHANGELOG is not the one being tagged"
sc_end; fi

echo "== a container the kernel killed is not a test that failed =="
# sysknife#470's verify came back `the test does not pass unmutated (rc=137)`.
# 137 is 128+9: the container was SIGKILLed, which on this path means the
# memory cap or the OOM killer, and the pull request had nothing to do with it.
# looks_environmental reads the LOG, and a killed process writes no diagnostic
# at all, so no shape could ever match. The signal is in the exit code.
if sc_case 1; then
grep -qE '\b137\b' "$root/bin/maintainer-merge" \
    && ok "the gate knows what exit 137 means" \
    || bad "a SIGKILLed container is still reported as the contributor's test failing"
sc_end; fi
# And it must stay narrow: an ordinary non-zero is still the pull request's.
kexit="$stub_dir/killexit"; rm -rf "$kexit"; mkdir -p "$kexit"
: > "$kexit/empty.log"
if sc_case 3; then
kf=$(bash -c 'eval "$(sed -n "/^looks_killed()/,/^}/p" "$1")"; looks_killed 137 && echo yes || echo no' _ "$root/bin/maintainer-merge" 2>/dev/null)
[ "$kf" = yes ] && ok "137 is recognised as a kill" || bad "137 is not recognised (got '$kf')"
kf=$(bash -c 'eval "$(sed -n "/^looks_killed()/,/^}/p" "$1")"; looks_killed 101 && echo yes || echo no' _ "$root/bin/maintainer-merge" 2>/dev/null)
[ "$kf" = no ] && ok "and an ordinary cargo failure (101) is still the pull request's" \
    || bad "a real test failure would be excused as a kill (got '$kf')"
kf=$(bash -c 'eval "$(sed -n "/^looks_killed()/,/^}/p" "$1")"; looks_killed 143 && echo yes || echo no' _ "$root/bin/maintainer-merge" 2>/dev/null)
[ "$kf" = yes ] && ok "and SIGTERM (143), which is how the timeout arrives" \
    || bad "a timed-out container reads as a failing test (got '$kf')"
sc_end; fi
# The direction that matters most. The mutated run is EXPECTED to fail, so any
# non-zero was read as the guard biting, and a container the memory cap killed
# would mint a receipt on no evidence at all. A kill is not a proof.
if sc_case 1; then
grep -q 'looks_killed "$mut_rc"' "$root/bin/maintainer-merge" \
    && ok "a killed mutated run is refused rather than counted as the guard biting" \
    || bad "an OOM-killed mutation still buys a receipt"
sc_end; fi
if sc_case 1; then
grep -q 'looks_killed "$clean_rc"' "$root/bin/maintainer-merge" \
    && ok "and a killed clean run is blamed on the container, not the contributor" \
    || bad "a killed clean run still reads as the pull request failing"
sc_end; fi

echo "== install.sh refuses to replace a tree a run is reading =="
# install.sh does `rm -rf "$share/profiles"` and recopies. On 2026-09-21 a
# sysknife review run held its lock for over two hours while I was one command
# away from running this over it, and nothing in install.sh looked. The run
# reads those files the whole way through; deleting them mid-run is a failure
# nobody would be able to explain afterwards, because the evidence goes with
# the directory.
if sc_case 1; then
grep -qE 'flock|run\.lock' "$root/install.sh" \
    && ok "install.sh asks whether a run is in flight" \
    || bad "install.sh replaces the deployed tree without checking for a live run"
sc_end; fi
ilk="$stub_dir/instlock"; rm -rf "$ilk"; mkdir -p "$ilk/state"
: > "$ilk/state/run.lock"
# Hold the lock the way a run does, then drive the check out of install.sh.
( flock 9; sleep 6 ) 9<>"$ilk/state/run.lock" &
ilk_pid=$!
# Wait for the holder to ACQUIRE it rather than sleeping a fixed second. Under
# load that sleep is a race, and this suite ran with a rust build beside it on
# 2026-09-21 and failed once for exactly that reason. A gate that fails at
# random teaches everyone to re-run until green, which is how a real failure
# gets waved through: MISTAKES rule 2.
ilk_wait=0
while flock -n "$ilk/state/run.lock" true 2>/dev/null; do
    ilk_wait=$((ilk_wait + 1))
    [ "$ilk_wait" -gt 200 ] && break
    sleep 0.05
done
if flock -n "$ilk/state/run.lock" true 2>/dev/null; then
    bad "the fixture lock is not actually held, so this case proves nothing"
else
    ok "the fixture lock is held, so the refusal has something to detect"
fi
wait "$ilk_pid" 2>/dev/null || true
flock -n "$ilk/state/run.lock" true 2>/dev/null \
    && ok "and the lock frees when the holder exits" \
    || bad "the fixture lock never released"
# And the refusal has to come before the first write, not just before the
# profiles. On 2026-10-29 an install refused with a magent run in flight, after
# it had already replaced run.sh, defaults.env and every script and stamped
# VERSION with the new release: a refused install left a half-deployed tool
# whose stamp claimed the version it had not finished installing.
ihome="$stub_dir/ihome"; rm -rf "$ihome"; mkdir -p "$ihome"
: > "$MAINTAINER_STATE_DIR/run.lock"
( flock 9; sleep 8 ) 9<>"$MAINTAINER_STATE_DIR/run.lock" &
ih_pid=$!
ih_wait=0
while flock -n "$MAINTAINER_STATE_DIR/run.lock" true 2>/dev/null; do
    ih_wait=$((ih_wait + 1)); [ "$ih_wait" -gt 200 ] && break; sleep 0.05
done
ih_out="$(HOME="$ihome" "$root/install.sh" 2>&1)"; ih_rc=$?
ih_files="$(find "$ihome" -type f 2>/dev/null | wc -l)"
wait "$ih_pid" 2>/dev/null || true
if [ "$ih_rc" = 69 ] && grep -q 'in flight' <<<"$ih_out" && [ "$ih_files" = 0 ]; then
    ok "an install refused for a run in flight writes nothing at all"
else
    bad "a refused install still wrote $ih_files file(s) (rc=$ih_rc): $(find "$ihome" -type f | head -3 | tr '\n' ' ')"
fi

echo "== install.sh accepts a checkout with no sysknife profile (#207) =="
# install.sh used to test for profiles/sysknife by name after copying, and a fork
# that carries only its own profile stopped with "profiles did not land where
# run.sh expects" and never installed the units. Each case installs from a
# scratch copy of this checkout into a scratch HOME. Offline.
ins_dir="$(mktemp -d)"
ins_home="$ins_dir/home"; mkdir -p "$ins_home"
# Unset the caller's state dir: a held run lock there makes install exit 69.
ins_src="$ins_dir/repo"; mkdir -p "$ins_src"
cp -r "$root/." "$ins_src/" && rm -rf "$ins_src/.git" "$ins_src/profiles/sysknife"
if [ -e "$ins_src/profiles/sysknife" ]; then
    bad "the fixture still has profiles/sysknife, so the case proves nothing"
else
    ok "the fixture has no sysknife profile"
fi
ins_out="$(env -u MAINTAINER_STATE_DIR HOME="$ins_home" bash "$ins_src/install.sh" 2>&1)"; ins_rc=$?
if [ "$ins_rc" = 0 ]; then
    ok "install.sh exits 0 without a sysknife profile"
else
    bad "install.sh exited $ins_rc without a sysknife profile (expected 0): $(tail -n 3 <<<"$ins_out" | tr '\n' ' ')"
fi
ins_deployed="$ins_home/.local/share/maintainer/profiles/magent/profile.env"
if [ -f "$ins_deployed" ]; then
    ok "the magent profile is deployed"
else
    bad "the magent profile was not deployed to $ins_deployed"
fi
if [ "$(uname -s)" = Linux ]; then
    if ls "$ins_home"/.config/systemd/user/maintainer@*.service >/dev/null 2>&1; then
        ok "the systemd units are copied without a sysknife profile"
    else
        bad "no systemd units were copied without a sysknife profile"
    fi
fi
# A checkout with no profile at all is still refused, before anything is written.
ins2_src="$ins_dir/empty"; mkdir -p "$ins2_src"
cp -r "$root/." "$ins2_src/" && rm -rf "$ins2_src/.git" "$ins2_src/profiles"
mkdir -p "$ins2_src/profiles"
ins2_home="$ins_dir/home2"; mkdir -p "$ins2_home"
ins2_out="$(env -u MAINTAINER_STATE_DIR HOME="$ins2_home" bash "$ins2_src/install.sh" 2>&1)"; ins2_rc=$?
if [ "$ins2_rc" = 1 ] && grep -q 'profiles did not land' <<<"$ins2_out"; then
    ok "install.sh refuses a checkout with no profile at all"
else
    bad "a checkout with no profile: expected exit 1 and 'profiles did not land' (rc=$ins2_rc)"
fi
# flock is required to distinguish an active run from a broken lock tool. Keep
# only dirname on PATH so the installer reaches the prerequisite check without
# finding the host's flock or modifying the scratch HOME.
ins3_home="$ins_dir/home3"; ins3_path="$ins_dir/no-flock-bin"
mkdir -p "$ins3_home" "$ins3_path"
ln -s "$(command -v dirname)" "$ins3_path/dirname"
ins_bash="$(command -v bash)"
ins3_out="$(env -u MAINTAINER_STATE_DIR HOME="$ins3_home" PATH="$ins3_path" "$ins_bash" "$ins_src/install.sh" 2>&1)"; ins3_rc=$?
if [ "$ins3_rc" = 1 ] && grep -q 'flock is required to install and protect active runs' <<<"$ins3_out" \
   && [ ! -e "$ins3_home/.local/share/maintainer" ]; then
    ok "install.sh explains that flock is required and refuses before writing"
else
    bad "install.sh without flock did not refuse before writing (rc=$ins3_rc): $(tail -n 2 <<<"$ins3_out" | tr '\n' ' ')"
fi
rm -rf "$ins_dir"

