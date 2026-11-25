#!/usr/bin/env bash
# Part of the offline suite: issue claims, offers, settle, release and the claim gate.
# tests/run-tests.sh sources this file in one shell, after its own preamble and after
# the parts before it, so every helper and variable it reads comes from there. It is
# sourced, never run on its own, and carries a shebang so the shell sweeps pick it up.

# The parts share one shell. These names are set by the driver or by an earlier part;
# declare only tells the linter they are read here, and assigns and changes nothing.
declare gate_settings
declare mg
declare ofd
declare root
declare stub_dir

echo "== somebody who claimed an issue themselves is holding it =="
# The person side of the #371 bug. `offers` derived "who holds something" from
# maintainer @mentions alone, so a contributor who wrote "I'll take this",
# received the claim label and the assignment, and opened a pull request, never
# appeared in the table at all. The table's own footer says an absent person is
# an eligible one, so the tool positively recommended double-booking the person
# it had just correctly refused to double-book on the issue side.
#
# Measured on sysknife 2026-09-09: @sonalisrisivani held #371 by assignment and
# by open PR #379, and `maintainer offers` printed seventeen people, none of
# them her.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'pr list'*'--state merged'*) echo '[{"author":{"login":"carla"}},{"author":{"login":"carla"}},{"author":{"login":"veteran"}},{"author":{"login":"veteran"}},{"author":{"login":"owner"}},{"author":{"login":"app/dependabot"}}]';;
  *'pr list'*) echo '[{"number":88,"author":{"login":"carla"},"title":"fix","body":"Closes #40"}]';;
  *'issue list'*) echo '[{"number":40,"labels":[{"name":"claimed"}],"assignees":[{"login":"carla"}]},{"number":43,"labels":[],"assignees":[{"login":"selfclaim"}]},{"number":44,"labels":[],"assignees":[]}]';;
  *issues/40/comments*) echo '[{"u":"carla","b":"I will take this one"}]';;
  *issues/43/comments*) echo '[{"u":"selfclaim","b":"mine"}]';;
  *issues/44/comments*) echo '[]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of \
      python3 "$root/bin/maintainer" offers 2>&1)
grep -qE '^carla ' <<<"$out" \
    && ok "a contributor holding an issue by their own pull request is listed as holding it" \
    || bad "somebody with an open PR against an issue reads as eligible: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-100)"
grep -qE '^carla .*#40' <<<"$out" \
    && ok "and the table names the issue they are on" \
    || bad "carla is listed with no issue against her name"
grep -qE '^selfclaim ' <<<"$out" \
    && ok "an assignee with no maintainer mention is holding it too" \
    || bad "a self-claimed assignment leaves the person invisible"
grep -qE 'free to offer:[^\n]*#43' <<<"$out" \
    && bad "an assigned issue was offered to somebody else" \
    || ok "an assigned issue is not free"
grep -qE 'free to offer:[^\n]*#44' <<<"$out" \
    && ok "an issue nobody holds is still free" \
    || bad "the free pool collapsed once assignees were counted"

echo "== the table names who IS eligible, not only who is not =="
# Absence is not an answer a reader can act on. On 2026-09-09 this table listed
# seventeen blocked people and was read, by me, as "zero eligible contributors",
# and that went to the user as a finding. @Georgefifth had five merged pull
# requests, the most recent merged the previous day, and held nothing at all. He
# was eligible and warm, and the only evidence of it was a name that was not
# printed.
grep -qE '^(eligible|[0-9]+ (person|people) (are|is) free)' <<<"$out" \
    && ok "offers names the people who can take one" \
    || bad "the tool still asks the reader to notice an absence"
grep -qE 'veteran' <<<"$out" \
    && ok "a past contributor holding nothing is named" \
    || bad "veteran has two merged PRs and holds nothing, and is not named"
grep -qE '^owner |eligible.*owner' <<<"$out" \
    && bad "the maintainer's own account is offered work" \
    || ok "the maintainer is not in their own eligible list"
grep -qE 'dependabot' <<<"$out" \
    && bad "a bot is listed as an eligible contributor" \
    || ok "bots are not people"

echo "== an unreadable merged listing refuses rather than reporting nobody =="
# MISTAKES rule 6. "I could not ask" and "nobody is eligible" are the same empty
# list, and the second is the one that reads as permission to stop looking.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'pr list'*'--state merged'*) echo 'gh: rate limited' >&2; exit 1;;
  *'pr list'*) echo '[]';;
  *'issue list'*) echo '[{"number":50,"labels":[],"assignees":[]}]';;
  *issues/50/comments*) echo '[]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of \
      python3 "$root/bin/maintainer" offers 2>&1)
grep -qiE 'could not read the merged' <<<"$out" \
    && ok "an unreadable merged listing says so instead of printing an empty roster" \
    || bad "a rate limit reads as nobody having ever contributed"

echo "== an assignment to somebody else retires an older mention =="
# sysknife #331, live on 2026-09-09. @ITSMERNB was offered it on 09-01 and
# answered "I will verify the gate wiring path first". On 09-06 it was offered
# to @be-student, who took it, got the assignment and opened PR #377. `offers`
# still printed "ITSMERNB  no: working on #331", so a contributor with five
# merged pull requests was held out of the eligible pool by an issue that had
# been handed to somebody else four days earlier.
#
# GitHub's assignee is the tiebreak, because it is the field the project uses to
# say out loud who has it.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'pr list'*'--state merged'*) echo '[{"author":{"login":"first"},"mergedAt":"2026-09-08T00:00:00Z"}]';;
  *'pr list'*) echo '[]';;
  *'issue list'*) echo '[{"number":60,"labels":[{"name":"claimed"}],"assignees":[{"login":"second"}]}]';;
  *issues/60/comments*) echo '[{"u":"owner","b":"@first this one is yours"},{"u":"first","b":"looking"},{"u":"owner","b":"@second holding this for you"},{"u":"second","b":"taking it"}]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of \
      python3 "$root/bin/maintainer" offers 2>&1)
grep -qE '^second .*#60' <<<"$out" \
    && ok "the assignee is reported as holding the issue" \
    || bad "the person GitHub says owns it is not listed"
grep -qE '^first ' <<<"$out" \
    && bad "an earlier mention still holds an issue assigned to somebody else: $(grep -E '^first ' <<<"$out")" \
    || ok "an earlier mention is retired by a later assignment to somebody else"
grep -qE 'free to take one' <<<"$out" \
    && ok "and the person it freed is offered back to the reader" \
    || bad "freeing the person did not put them in the eligible list"
grep -qE '#60 .*(superseded|reassigned|assigned to)' <<<"$out" \
    && ok "the retirement is stated, not silent" \
    || bad "a hold disappeared with no line saying why"


echo "== a model usage window that resets on a clock is transient =="
# 2026-09-07T20-34-issues: the backend stopped with
#   You've hit your session limit · resets 10:10pm (America/Mexico_City)
# run.sh alerted at `critical` for the backend exit, alerted again because the
# run wrote no report, and exited 1 so systemd's OnFailure alerted a third time.
# Three popups for a condition that healed itself ninety minutes later, and a
# permanent red in maintainer-doctor until somebody re-ran the task by hand.
# This is the same class as the unreachable-API path, arriving through the
# backend instead of through gh.
tfix="$stub_dir/transient"; rm -rf "$tfix"; mkdir -p "$tfix"
if sc_case 1; then
( . "$root/lib/profile.sh" 2>/dev/null
  declare -F backend_transient_reason >/dev/null ) \
    && ok "the transient classifier is a sourceable function, so it can be tested" \
    || bad "no backend_transient_reason in lib/profile.sh"
sc_end; fi

printf "I'll start by loading the skill.\nYou've hit your session limit · resets 10:10pm (America/Mexico_City)\n" > "$tfix/limit.log"
printf 'API Error: 529 {"type":"overloaded_error"}\n' > "$tfix/overloaded.log"
printf 'API Error: 429 rate_limit_error\n' > "$tfix/rate.log"
printf 'Credit balance is too low to run this request\n' > "$tfix/credit.log"
printf 'thread panicked at src/main.rs:12: assertion failed\n' > "$tfix/real.log"
printf 'wrote the report and finished cleanly\n' > "$tfix/clean.log"

check_transient() { ( . "$root/lib/profile.sh" 2>/dev/null; backend_transient_reason "$1" ); }

if sc_case 8; then
r="$(check_transient "$tfix/limit.log")"; rc=$?
[ "$rc" = 0 ] && ok "a session limit that names its reset time is transient" \
    || bad "the 2026-09-07 session limit still reads as a real failure"
grep -qi 'session limit' <<<"$r" \
    && ok "and the reason quotes what the backend actually said" \
    || bad "the reason does not name the limit: '$r'"
check_transient "$tfix/overloaded.log" >/dev/null \
    && ok "a 529 overloaded_error is transient" || bad "an overloaded upstream reads as a defect"
check_transient "$tfix/rate.log" >/dev/null \
    && ok "a 429 rate_limit_error is transient" || bad "a rate limit reads as a defect"
# The negative twins. A guard that calls everything transient silences the
# alerts it exists to raise, which is worse than the popups it removes.
check_transient "$tfix/credit.log" >/dev/null \
    && bad "an empty credit balance was called transient; no retry pays an invoice" \
    || ok "an empty credit balance stays critical, because no retry fixes it"
check_transient "$tfix/real.log" >/dev/null \
    && bad "a panic was classified as transient" || ok "a real failure stays a real failure"
check_transient "$tfix/clean.log" >/dev/null \
    && bad "a clean log was classified as transient" || ok "a clean log is not transient"
check_transient "$tfix/does-not-exist.log" >/dev/null \
    && bad "a missing log was called transient" || ok "a log that cannot be read is not transient"
sc_end; fi

# run.sh has to USE it, on both alert paths, or the classifier is decoration.
if sc_case 1; then
grep -q 'backend_transient_reason' "$root/lib/run.sh" \
    && ok "run.sh consults the classifier" \
    || bad "the classifier exists and run.sh never calls it"
sc_end; fi

echo "== the pinned token never survives the run that used it =="
# Found by the magent issues run on 2026-09-09, in its own report: it wrote
#   echo "GH_TOKEN set? ${GH_TOKEN:+yes}${GH_TOKEN:-no}"
# and the `:-` arm expands to the VALUE when the variable is set, so the live
# PAT printed. It went nowhere public, and it landed in two Claude session
# transcripts on disk, from two separate runs. run.sh exports the pinned token
# for the whole run on purpose, so the token being reachable is by design and
# the token OUTLIVING the run is the defect.
scr="$stub_dir/scrub"; rm -rf "$scr"; mkdir -p "$scr/sub"
if sc_case 1; then
( . "$root/lib/profile.sh" 2>/dev/null; declare -F maintainer_scrub_secret >/dev/null ) \
    && ok "the scrubber is a sourceable function" \
    || bad "no maintainer_scrub_secret in lib/profile.sh"
sc_end; fi

SECRET='ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'
printf 'line one\nGH_TOKEN set? yes%s\nline three\n' "$SECRET" > "$scr/a.log"
printf '{"t":"x","stdout":"len=40 %s"}\n' "$SECRET" > "$scr/sub/b.jsonl"
printf 'nothing sensitive here\n' > "$scr/clean.log"
head -c 512 /dev/urandom > "$scr/sub/binary.bin"
before_a=$(wc -l < "$scr/a.log"); before_b=$(wc -l < "$scr/sub/b.jsonl")

if sc_case 5; then
( . "$root/lib/profile.sh" 2>/dev/null; maintainer_scrub_secret "$SECRET" "$scr" ) > "$scr/report.txt" 2>&1
grep -qF "$SECRET" "$scr/a.log" \
    && bad "the secret survived in a.log" || ok "the secret is gone from the log"
grep -qF "$SECRET" "$scr/sub/b.jsonl" \
    && bad "the secret survived in a nested jsonl" || ok "it recurses into subdirectories"
[ "$(wc -l < "$scr/a.log")" = "$before_a" ] && [ "$(wc -l < "$scr/sub/b.jsonl")" = "$before_b" ] \
    && ok "the surrounding records are intact, line for line" \
    || bad "scrubbing changed the file structure"
grep -q 'line three' "$scr/a.log" \
    && ok "and the content around the secret survives" \
    || bad "the scrubber ate more than the secret"
grep -qE 'a\.log|b\.jsonl' "$scr/report.txt" \
    && ok "it names the files it changed" \
    || bad "the scrubber cleans in silence, so nothing can act on it"
sc_end; fi
# The negative twin that matters most. An empty secret matches everywhere, and a
# scrubber that accepts one rewrites every file it walks.
if sc_case 2; then
: > "$scr/canary.log"; printf 'untouched\n' > "$scr/canary.log"
( . "$root/lib/profile.sh" 2>/dev/null; maintainer_scrub_secret "" "$scr" ) >/dev/null 2>&1
[ "$(cat "$scr/canary.log")" = "untouched" ] \
    && ok "an empty secret is refused rather than matched against everything" \
    || bad "an empty secret rewrote a file"
( . "$root/lib/profile.sh" 2>/dev/null; maintainer_scrub_secret "short" "$scr" ) >/dev/null 2>&1
[ "$(cat "$scr/canary.log")" = "untouched" ] \
    && ok "a too-short secret is refused, because a common substring is not a credential" \
    || bad "a five-character string was scrubbed out of every file"
sc_end; fi
# The secret must not travel in argv, where /proc and any ps make it readable.
if sc_case 1; then
grep -nE 'maintainer_scrub_secret\(\)' -A 24 "$root/lib/profile.sh" 2>/dev/null \
    | grep -qE '(sed|perl|grep)[^\n]*"\$(secret|1)"' \
    && bad "the scrubber passes the secret as a command-line argument" \
    || ok "the secret reaches the helper through the environment, not argv"
sc_end; fi

# A grep for the NAME used to stand here, and it passed for the whole life of
# the feature while the trap was a no-op. lib/run.sh calls
# maintainer_scrub_secret and never sources the file that defines it, so every
# exit logged "maintainer_scrub_secret: command not found", the `|| true` turned
# that into success, and the token stayed on disk. Seen in the wild at the tail
# of 2026-09-21T11-17-review. Drive the real script and look at the file.
scrubrun="$stub_dir/scrubrun"; rm -rf "$scrubrun"; mkdir -p "$scrubrun/state"
sr_repo="$scrubrun/repo"; git init -q -b main "$sr_repo"
SR_TOKEN='ghp_ScrubRunTokenAAAAAAAAAAAAAAAAAAAA'
# `api user` answers before the pin and fails after it, so the run gets past the
# first identity check, exports GH_TOKEN, installs the trap, and then refuses.
# That is the shortest real path through the trap.
cat > "$stub_dir/gh" <<GHEOF
#!/usr/bin/env bash
case "\$*" in
  *'auth switch'*) exit 0;;
  *'auth token'*) echo '$SR_TOKEN';;
  *'api user'*)
      if [ -n "\${GH_TOKEN:-}" ]; then echo "error connecting to api.github.com" >&2; exit 1; fi
      echo vladimirrott;;
esac
GHEOF
chmod +x "$stub_dir/gh"
printf 'an earlier line\nGH_TOKEN=%s\ntrailing line\n' "$SR_TOKEN" > "$scrubrun/state/leaked.log"
PATH="$stub_dir:$PATH" HOME="$scrubrun" MAINTAINER_SETTINGS="$gate_settings" \
    MAINTAINER_STATE_DIR="$scrubrun/state" MAINTAINER_REPO_PATH="$sr_repo" \
    MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review >/dev/null 2>&1
if grep -qF "$SR_TOKEN" "$scrubrun/state/leaked.log" 2>/dev/null; then
    bad "run.sh exited with the pinned token still on disk; the EXIT trap scrubbed nothing"
else
    ok "run.sh's EXIT trap really removes the pinned token from what the run left behind"
fi
grep -q 'trailing line' "$scrubrun/state/leaked.log" 2>/dev/null \
    && ok "and the scrub on exit leaves the rest of the file alone" \
    || bad "the scrub on exit ate more than the token"

# The class, not the one instance. Any shell script that CALLS a helper defined
# in lib/profile.sh has to source lib/profile.sh, and naming one without
# sourcing it fails open every time, because the callers all use `|| true`.
if sc_case 1; then
scrub_helpers="$(grep -oE '^maintainer_[a-z_]+' "$root/lib/profile.sh" | sort -u)"
scrub_unsourced=""
for _f in "$root"/lib/run.sh "$root"/lib/run-instance.sh "$root"/bin/*; do
    [ -f "$_f" ] || continue
    case "$_f" in *profile.sh) continue ;; esac
    head -1 "$_f" | grep -q 'bash' || continue
    for _h in $scrub_helpers; do
        # a call site: the name followed by whitespace, never `name()`
        grep -qE "(^|[^-_[:alnum:]])${_h}[[:space:]]" "$_f" || continue
        grep -qE "^$_h\(\)" "$_f" && continue
        grep -q 'profile\.sh' "$_f" && continue
        scrub_unsourced="$scrub_unsourced $(basename "$_f"):$_h"
    done
done
[ -z "$scrub_unsourced" ] \
    && ok "every shell script that calls a profile.sh helper also sources it" \
    || bad "these call a helper whose definition they never load:$scrub_unsourced"
sc_end; fi
# The other direction. A tree missing profile.sh has to refuse at the start,
# not reach the exit trap and find nothing there. Failing open on the way out is
# how this went unnoticed; failing open on the way in would be the same defect
# wearing the repair.
nolib="$stub_dir/noproflib"; rm -rf "$nolib"; mkdir -p "$nolib/profiles/sysknife/prompts"
cp "$root/lib/run.sh" "$nolib/run.sh"; cp -r "$root/lib/backends" "$nolib/backends"
cp "$root/profiles/sysknife/profile.env" "$nolib/profiles/sysknife/"
out=$(PATH="$stub_dir:$PATH" HOME="$nolib" bash "$nolib/run.sh" sysknife review 2>&1); rc=$?
if [ "$rc" = 64 ] && grep -q 'profile\.sh' <<<"$out"; then
    ok "a tree with no profile.sh refuses to start, and the refusal names the file"
else
    bad "a tree with no profile.sh started anyway (rc=$rc)"
fi
if sc_case 1; then
grep -q 'maintainer_scrub_secret\|token still on disk\|pinned token' "$root/bin/maintainer-doctor" \
    && ok "doctor looks for a token left behind by an earlier run" \
    || bad "nothing notices a token that a previous run left on disk"
sc_end; fi
# Could not ask is not an answer. With the listing unreadable the free list must
# say it is unverified rather than silently claim every issue is offerable.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'pr list'*) exit 1;;
  *'issue list'*) echo '[{"number":40,"labels":[]}]';;
  *issues/40/comments*) echo '[]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of \
      python3 "$root/bin/maintainer" offers 2>&1)
grep -q 'could not read the open pull requests' <<<"$out" \
    && ok "an unreadable PR listing is said out loud before anything is offered" \
    || bad "offers claims a free list it could not verify"
rm -f "$stub_dir/gh"

echo "== somebody working an issue, and somebody else turning up on it =="
# The double-booked check only sees issues offered to two people who have NOT
# answered. On 2026-09-07 that missed the case that cost the most: #252 was
# offered to one contributor who ANSWERED and was working it, while a second
# contributor nobody had pointed anywhere implemented it independently and
# opened a PR. The first finished their evening against an issue that had
# already been closed. An issue somebody is actively working deserves more
# protection than one that has been ignored for a week, not less.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'issue list'*) echo '[{"number":40,"labels":[]},{"number":41,"labels":[]}]';;
  # 40: ana holds it and answered; bruno turns up independently.
  *issues/40/comments*) echo '[{"u":"owner","b":"@ana this one is yours"},{"u":"ana","b":"taking it"},{"u":"bruno","b":"I have this implemented now"}]';;
  # 41: ana holds it and answered; nobody else has posted.
  *issues/41/comments*) echo '[{"u":"owner","b":"@ana yours"},{"u":"ana","b":"on it"}]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of \
      python3 "$root/bin/maintainer" offers 2>&1)
grep -qE 'CONTENDED.*#40' <<<"$out" \
    && ok "an issue whose holder is working it flags a second person turning up" \
    || bad "a second contributor on a live issue goes unreported (this cost #252)"
grep -qE 'CONTENDED.*#40.*bruno|CONTENDED.*#40[^\n]*bruno' <<<"$out" \
    && ok "and it names who turned up" \
    || bad "the contention does not name the second person"
grep -qE 'CONTENDED.*#41' <<<"$out" \
    && bad "an issue with only its holder posting was reported as contended" \
    || ok "the holder posting on their own issue is not contention"
# A person the maintainer mentioned and then RELEASED is a resolved case, not
# an unknown second party. #219 was claimed in August, released on 2026-09-02
# with the marker, and reserved for somebody else on 2026-09-07; the first
# claimant still shows in the comment history and must not read as contention.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'issue list'*) echo '[{"number":42,"labels":[]}]';;
  *issues/42/comments*) cat <<'J'
[{"u":"owner","b":"@early this one is yours"},
 {"u":"early","b":"on it"},
 {"u":"owner","b":"@early releasing this, no hard feelings <!-- maintainer: claim-released -->"},
 {"u":"owner","b":"@later reserved for you"},
 {"u":"later","b":"taking it"}]
J
    ;;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$ofd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of \
      python3 "$root/bin/maintainer" offers 2>&1)
grep -qE 'CONTENDED.*#42' <<<"$out" \
    && bad "a released earlier claimant reads as contention on a reassigned issue" \
    || ok "a released claimant is a resolved case, not a second party"

echo "== a profile variable the tool reads must be in the allowlist =="
# _profile_env sources profile.env and reads back a literal tuple of key names.
# A key the code reads but the tuple omits comes back empty, which is
# indistinguishable from the profile not declaring it: `maintainer claims`
# shipped reporting "this profile declares no CLAIM_LABEL" against a profile
# that declares it on line 74.
python3 - "$root/bin/maintainer" <<'PYEOF'
import re, sys
src = open(sys.argv[1]).read()
m = re.search(r'keys = \((.*?)\)', src, re.S)
allow = set(re.findall(r'"([A-Z_]+)"', m.group(1))) if m else set()
read = set(re.findall(r'_ENV\.get\(\s*"([A-Z_]+)"', src))
read |= set(re.findall(r'_setting\(\s*"[A-Z_]*"\s*,\s*"([A-Z_]+)"', src))
missing = sorted(read - allow)
sys.exit(f"keys read from _ENV but absent from the allowlist: {missing}" if missing else 0)
PYEOF
[ $? = 0 ] && ok "every profile key the tool reads is in the allowlist" \
    || bad "a profile variable is read and can never be seen"

echo "== claims are visible, and staleness survives the maintainer touching them =="
cm="$stub_dir/claimsview"; mkdir -p "$cm"
# Dates are computed from today, never written down. The first version of this
# fixture hardcoded 2026-08-24 and asserted the idle column matched 1[0-9]d, so
# it passed for nine days and went red for good on the tenth, reporting a bug in
# the tool that did not exist. A fixture that expires is a fixture that will one
# day be "fixed" by loosening the assertion it was written to make.
export CLAIMS_NOW CLAIMS_CLAIMANT_AT CLAIMS_OTHER_AT
CLAIMS_NOW="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
CLAIMS_CLAIMANT_AT="$(date -u -d '14 days ago - 1 hour' +%Y-%m-%dT%H:%M:%SZ)"
CLAIMS_OTHER_AT="$(date -u -d '3 days ago - 1 hour' +%Y-%m-%dT%H:%M:%SZ)"
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'issue list'*) cat <<J
[{"number":272,"title":"t","assignees":[{"login":"atanishka308"}],"updatedAt":"${CLAIMS_NOW}","labels":[]},
 {"number":229,"title":"u","assignees":[],"updatedAt":"${CLAIMS_NOW}","labels":[]}]
J
    ;;
  *issues/272/comments*) echo "[{\"u\":\"atanishka308\",\"at\":\"${CLAIMS_CLAIMANT_AT}\"}]";;
  *issues/229/comments*) echo "[{\"u\":\"someone\",\"at\":\"${CLAIMS_OTHER_AT}\"}]";;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$cm" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=t MAINTAINER_PROFILE=cv CLAIM_LABEL=claimed \
      python3 "$root/bin/maintainer" claims 2>&1)
# updatedAt on BOTH rows is today, because assigning them moved it. Idle must
# come from the claimant's own last comment or the check is blind to exactly
# the claims it exists to find.
grep -qE '#272 +atanishka308 +14d' <<<"$out" \
    && ok "idle is measured from the claimant, not from updatedAt" \
    || bad "the maintainer touching an issue resets its staleness: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90)"
grep -q 'stale, a check-in is due' <<<"$out" \
    && ok "and an old claim is called stale" || bad "a stale claim is reported as active"
grep -q 'NOT assigned' <<<"$out" \
    && ok "a claim with no assignee is named as invisible to their dashboard" \
    || bad "a label-only claim looks the same as an assigned one"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$cm" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=t MAINTAINER_PROFILE=cv CLAIM_LABEL="" \
      python3 "$root/bin/maintainer" claims 2>&1)
grep -q 'declares no CLAIM_LABEL' <<<"$out" \
    && ok "a profile that tracks no claims says so" || bad "claims assumes every project uses a label"


echo "== a pull request answers an offer more strongly than a comment does =="
# On 2026-09-07 `claims` reported sysknife#390 as "OFFERED 0d ago and never
# answered; do not assign", while Georgefifth had an open PR #389 whose body
# said `Closes #390`. Opening the pull request is the strongest answer anybody
# can give to an offer, and the check that decides whether to assign them read
# only the comment thread. The offer looked unanswered for as long as the
# contributor did the work instead of replying.
cp2="$stub_dir/claimspr"; mkdir -p "$cp2"
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'pr list'*) cat <<'J'
[{"number":389,"author":{"login":"georgefifth"},"title":"fix(e2e): per-word tags","body":"Closes #390"},
 {"number":401,"author":{"login":"driveby"},"title":"unrelated","body":"see #391 for context"}]
J
    ;;
  *'issue list'*) cat <<'J'
[{"number":390,"title":"t","assignees":[],"updatedAt":"2026-09-07T19:00:00Z","labels":[]},
 {"number":391,"title":"u","assignees":[],"updatedAt":"2026-09-07T19:00:00Z","labels":[]}]
J
    ;;
  *issues/390/comments*) echo '[{"u":"maint","b":"@georgefifth this one is yours","at":"2026-09-01T10:00:00Z"}]';;
  *issues/391/comments*) echo '[{"u":"maint","b":"@driveby this one is yours","at":"2026-09-01T10:00:00Z"}]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$cp2" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=maint MAINTAINER_PROFILE=cp CLAIM_LABEL=claimed \
      python3 "$root/bin/maintainer" claims 2>&1)
grep -qE '#390.*never answered' <<<"$out" \
    && bad "an offer answered by a pull request is still reported as unanswered" \
    || ok "an offer answered by a pull request is not reported as unanswered"
grep -qE '#390.*#389' <<<"$out" \
    && ok "and the pull request that answered it is named" \
    || bad "the answer is accepted without saying where it came from"
grep -q 'maintainer assign 390 georgefifth' <<<"$out" \
    && ok "and the assignment is recommended, because they did answer" \
    || bad "somebody who answered with a PR gets no assignment suggestion"
# The negative twin, and it is the whole point: a bare mention of the issue is
# not a closing reference. #401 says "see #391", which GitHub would not act on
# either, so neither does this.
grep -qE '#391.*never answered' <<<"$out" \
    && ok "a PR that merely mentions an issue does not answer the offer" \
    || bad "any mention of the number counts as a claim, which is worse than the bug"
grep -q 'maintainer assign 391' <<<"$out" \
    && bad "a drive-by mention produced an assignment recommendation" \
    || ok "and it recommends no assignment for a mere mention"
# Could not ask is not an answer of "nobody". A failed PR listing must leave the
# offer looking unanswered rather than inventing an answer, and must say so.
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'pr list'*) exit 1;;
  *'issue list'*) cat <<'J'
[{"number":390,"title":"t","assignees":[],"updatedAt":"2026-09-07T19:00:00Z","labels":[]}]
J
    ;;
  *issues/390/comments*) echo '[{"u":"maint","b":"@georgefifth this one is yours","at":"2026-09-01T10:00:00Z"}]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$cp2" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=maint MAINTAINER_PROFILE=cp CLAIM_LABEL=claimed \
      python3 "$root/bin/maintainer" claims 2>&1)
grep -q 'could not read the open pull requests' <<<"$out" \
    && ok "a failed PR listing is reported rather than read as 'nobody answered'" \
    || bad "an unreadable PR list is indistinguishable from an empty one"
grep -q 'maintainer assign 390' <<<"$out" \
    && bad "an unreadable PR list still produced an assignment recommendation" \
    || ok "and it recommends no assignment it could not justify"
rm -f "$stub_dir/gh"

echo "== a claimed issue is a promise the gate has to keep =="
# Measured on lacs-project/sysknife, 2026-09-04. A contributor said "I am taking
# this" on #355; the maintainer replied "it is yours", applied `claimed`, and
# said in the same comment that the label "is what the other contributors read".
# Ten hours later a different contributor opened a pull request closing it. The
# review approved it, the gate merged it, and the claimant had nothing.
cl="$stub_dir/claimstate"; mkdir -p "$cl/receipts"
# A real repository, because the gate fetches refs/pull/N/head before it compares
# the verified head. The pass-path cases died there against /tmp and read as the
# claim check firing on every PR, which is the opposite of what they measure.
clr="$stub_dir/clrepo"; git init -q -b main "$clr"
git -C "$clr" config user.email t@t; git -C "$clr" config user.name t
printf 'x\n' > "$clr/f"; git -C "$clr" add -A >/dev/null; git -C "$clr" commit -qm base
clsha="$(git -C "$clr" rev-parse HEAD)"
git -C "$clr" update-ref refs/pull/1/head "$clsha"
git -C "$clr" remote add origin "$clr"
# The stub is written as a plain heredoc, not through make_stub. Nested quoting
# through a `make_stub "..."` argument mangled the case patterns, so the stub
# answered nothing, the gate sailed past the check, and all five cases failed
# while the guard itself was working.
cl_gh() {  # $1 = label index (0 = claimed, null = not), $2 = commenter logins
    cat > "$stub_dir/gh" <<GHEOF
#!/usr/bin/env bash
case "\$*" in
  *'auth switch'*) exit 0;;
  *'api user'*) echo testuser;;
  *reviewDecision*) echo APPROVED;;
  *closingIssuesReferences*) echo 355;;
  *'pr view'*author*) echo newcomer;;
  # The gate asks \`maintainer holders\` before it looks at the label. These
  # cases measure the LABEL path, so the thread says nobody was pointed here.
  *issues/355/comments*) echo '[]';;
  # gh --jq prints NOTHING for a JSON null, and an index when the label is
  # present. Echoing the literal "null" is a shape gh never produces.
  *'issue view'*labels*) [ "$1" = null ] || echo $1;;
  # The gate reads an assignee list and a list of commenter logins now. The old
  # stub answered both with an index, which modelled "is the author present"
  # and could not express WHO else was, which is the question that matters.
  *'issue view'*assignees*) echo "";;
  *'issue view'*comments*) printf '%s\n' $2;;
  *headRefOid*) echo $clsha;;
  *mergeStateStatus*) echo CLEAN;;
  *'pr checks'*) echo '[{"name":"x","bucket":"pass"}]';;
  *'pr merge'*) echo MERGED_STUB;;
esac
GHEOF
    chmod +x "$stub_dir/gh"
}
# MAINTAINER_PROFILE is set here on purpose. A real run.sh exports it, and the
# gate now shells out to `maintainer holders`, which refuses to guess a profile
# when more than one is deployed. Leaving it unset made that call fail, and the
# first version of the holder check swallowed the failure and merged anyway.
cl_merge() { PATH="$stub_dir:$PATH" MAINTAINER_STATE="$cl" MAINTAINER_ACCOUNT=testuser \
    MAINTAINER_SLUG=o/r MAINTAINER_REPO="$clr" PROD_GLOBS="bin/*" CLAIM_LABEL="${1-claimed}" \
    MAINTAINER_PROFILE=of MAINTAINER_POST=off bash "$mg" merge 1 2>&1; }
cl_gh 0 claimant
PATH="$stub_dir:$PATH" MAINTAINER_STATE="$cl" MAINTAINER_ACCOUNT=testuser \
    MAINTAINER_SLUG=o/r MAINTAINER_REPO="$clr" \
    bash "$mg" receipt 1 "$clsha" 'proved' >/dev/null 2>&1

# Claimed by someone else, and this author never posted on it.
out="$(cl_merge)"
grep -q "closes #355, which carries the 'claimed' label" <<<"$out" \
    && ok "a PR closing another person's claimed issue is refused" \
    || bad "the gate merged over a claim: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90)"
grep -q 'takes their work away' <<<"$out" \
    && ok "and says why, rather than naming a rule" || bad "the refusal does not say what it protects"
# The three cases below are about the claim guard LETTING a merge through, so
# they assert its refusal text is absent rather than that the whole merge
# succeeds. The success path prints "receipt valid" only after a real git fetch,
# which flakes under load; whether the claim guard blocked is decided before it.
claim_allowed() {  # stdin = gate output -> 0 if the claim guard did not block
    ! grep -q "carries the '.*' label" ; }
# Labelled, unassigned, and the author is the only voice on the thread: there is
# nobody whose claim this could be taking. Note this used to pass merely because
# the author had commented, which is the flaw that let a stranger clear somebody
# else's claim by saying "I have this implemented".
cl_gh 0 newcomer
cl_merge | claim_allowed \
    && ok "the author's own claim does not block them" \
    || bad "a contributor is refused their own claimed issue"
# No claim label on the issue.
cl_gh null claimant
cl_merge | claim_allowed \
    && ok "an unclaimed issue is not blocked by the claim guard" || bad "the check fires on every PR"
# A project that does not use claims switches it off.
cl_gh 0 claimant
cl_merge "" | claim_allowed \
    && ok "an empty CLAIM_LABEL disables the check" || bad "the check cannot be turned off"
for pe in "$root"/profiles/*/profile.env; do
    pn="$(basename "$(dirname "$pe")")"
    grep -q '^CLAIM_LABEL=' "$pe" && ok "$pn states whether it uses a claim label" \
        || bad "$pn leaves CLAIM_LABEL undeclared, so the check silently does nothing"
done

echo "== an offer nobody answered is not a claim, and claims must not conflate them =="
# The agent found this in its own tooling on 2026-09-07: `claims` printed
# "NOT assigned, so it is invisible to their dashboard" and told the maintainer
# to assign, for two issues that `offers` called "offered and never answered".
# Assigning somebody who never replied commits them in public to work they have
# not accepted. The label was doing double duty as "offered" and "taken".
clm="$stub_dir/claimstate2"; mkdir -p "$clm"
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
case "$*" in
  *'issue list'*) cat <<'J'
[{"number":50,"title":"never answered","assignees":[],"updatedAt":"2026-09-01T00:00:00Z","labels":[{"name":"claimed"}]},
 {"number":51,"title":"they replied","assignees":[],"updatedAt":"2026-09-01T00:00:00Z","labels":[{"name":"claimed"}]}]
J
    ;;
  # 50: offered, silence. 51: offered, and they answered.
  *issues/50/comments*) echo '[{"u":"owner","b":"@quiet this one is yours","at":"2026-09-01T00:00:00Z"}]';;
  *issues/51/comments*) echo '[{"u":"owner","b":"@keen this one is yours","at":"2026-09-01T00:00:00Z"},{"u":"keen","b":"taking it","at":"2026-09-02T00:00:00Z"}]';;
  *comments*) echo '[]';;
esac
GHEOF
chmod +x "$stub_dir/gh"
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$clm" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
      MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=of CLAIM_LABEL=claimed \
      python3 "$root/bin/maintainer" claims 2>&1)
grep -qE '#50.*(unanswered|never answered|no reply)' <<<"$out" \
    && ok "an unanswered offer is reported as an offer, not as a claim" \
    || bad "claims still calls an unanswered offer a claim: $(grep '#50' <<<"$out" | cut -c1-70)"
grep -q 'gh issue edit N --repo <slug> --add-assignee' <<<"$out" \
    && bad "claims still recommends the command that 404s for a non-collaborator" \
    || ok "claims no longer recommends gh issue edit --add-assignee"
grep -q 'maintainer assign' <<<"$out" \
    && ok "claims points at the verb that verifies the assignment took" \
    || bad "claims does not mention maintainer assign"

echo "== a run can find the toolchain it is asked to use =="
# The unattended ci run on 2026-09-07 hit `cargo: command not found` (rc=127) on
# its first invocation and had to prefix PATH by hand for every later command.
# rustup installs into ~/.cargo/bin, which is on an interactive shell's PATH via
# the profile and absent from a systemd user unit's.
if sc_case 1; then
grep -q 'cargo/bin' "$root/lib/run.sh" \
    && ok "run.sh puts the cargo toolchain on PATH" \
    || bad "a run cannot find cargo; every Rust gate it is asked to run fails 127"
sc_end; fi

echo "== the gate reads the thread, not only the label =="
# sysknife#252, 2026-09-07. The maintainer pointed a contributor at the issue
# with an @mention on 1 September and never applied the label. A second
# contributor read the same public thread, implemented it, and opened a PR. The
# gate skipped the issue entirely, because its only notion of a claim was a
# label nobody had applied, and the merge closed the issue about twenty minutes
# before the first contributor posted their finished implementation.
#
# `offers` derives holders from the thread and the gate derived them from a
# label: two sources of truth for one fact, and the fragile one is the one a
# human has to remember. The gate now asks `maintainer holders`, which is the
# same derivation `offers` uses, rather than carrying a second copy of it.
thread_gh() {  # $1 = JSON array of {u,b} for the comments API, $2 = PR author (default newcomer)
    cat > "$stub_dir/gh" <<GHEOF
#!/usr/bin/env bash
case "\$*" in
  *'auth switch'*) exit 0;;
  *'api user'*) echo testuser;;
  *reviewDecision*) echo APPROVED;;
  *closingIssuesReferences*) echo 355;;
  *'pr view'*author*) echo ${2:-newcomer};;
  *issues/355/comments*) echo '$1';;
  *'issue view'*labels*) echo null;;
  *'issue view'*comments*) echo 0;;
  *headRefOid*) echo $clsha;;
  *mergeStateStatus*) echo CLEAN;;
  *'pr checks'*) echo '[{"name":"x","bucket":"pass"}]';;
  *'pr merge'*) echo MERGED_STUB;;
esac
GHEOF
    chmod +x "$stub_dir/gh"
}
# The output is captured before it is inspected, never piped into grep. The
# suite runs with `set -o pipefail` and the gate exits non-zero when it refuses,
# so `cl_merge | grep -q ...` reports the gate's exit code rather than whether
# the text matched. Every assertion below would have been decided by the die,
# not by the message, which is the same trap as reading `$?` after a pipe.

# Somebody else was pointed at it, and no label was ever applied.
thread_gh '[{"u":"testuser","b":"@holder this one is yours"}]'
out="$(cl_merge)"
grep -q 'was pointed at #355' <<<"$out" \
    && ok "the gate refuses a PR closing an issue pointed at somebody else" \
    || bad "an unlabelled issue somebody was pointed at is merged out from under them"
grep -q 'holder' <<<"$out" \
    && ok "and it names who was pointed there" \
    || bad "the refusal does not name the person who was promised the issue"

# The author is the person who was pointed there.
thread_gh '[{"u":"testuser","b":"@newcomer this one is yours"}]'
out="$(cl_merge)"
grep -q 'was pointed at #355' <<<"$out" \
    && bad "a contributor is refused the issue the maintainer gave them" \
    || ok "the author being the person pointed there is not blocked"

# Pointed there, then released out loud.
thread_gh '[{"u":"testuser","b":"@holder yours"},{"u":"testuser","b":"@holder releasing <!-- maintainer: claim-released -->"}]'
out="$(cl_merge)"
grep -q 'was pointed at #355' <<<"$out" \
    && bad "a released claim still blocks a merge" \
    || ok "a released holder does not block anybody"

# Nobody was ever pointed at it.
thread_gh '[{"u":"someone","b":"is this still open?"}]'
out="$(cl_merge)"
grep -q 'was pointed at #355' <<<"$out" \
    && bad "the thread check fires when there is no holder" \
    || ok "an issue nobody was pointed at is not blocked"

# GitHub logins are case-insensitive, and a mention is whatever case the
# maintainer typed. sysknife#521, 2026-09-30: #519 was offered to
# `@TayfurYldz`, the pull request's author came back from the API as
# `tayfuryldz`, and the gate refused to merge the offered person's own work
# because it compared the two strings exactly. Same account, id 238304586.
# Each case below changes the case on one side of one comparison.
thread_gh '[{"u":"testuser","b":"@NewComer this one is yours"}]'
out="$(cl_merge)"
grep -q 'was pointed at #355' <<<"$out" \
    && bad "the person offered the issue is refused because the offer spelled their handle in another case" \
    || ok "an offer to @NewComer is the same person as the author newcomer"

# A release has to retract the offer it answers. Keyed by the handle as typed,
# `@Holder yours` and `@holder releasing` were two people, so the release
# retracted nothing and the offer stood for ever.
thread_gh '[{"u":"testuser","b":"@Holder yours"},{"u":"testuser","b":"@holder releasing <!-- maintainer: claim-released -->"}]'
out="$(cl_merge)"
grep -q 'was pointed at #355' <<<"$out" \
    && bad "a release spelled in another case left the offer standing" \
    || ok "a release retracts an offer whatever case either one used"

# The other boundary. MAINTAINER_ACCOUNT is typed by a person and the comment's
# author comes from the API. Compared exactly, every maintainer comment reads as
# somebody else's, the gate finds no holders, and it lets the merge through:
# this one fails OPEN, which is the direction that costs somebody their work.
thread_gh '[{"u":"testuser","b":"@holder this one is yours"}]'
out="$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$cl" MAINTAINER_ACCOUNT=TestUser \
    MAINTAINER_SLUG=o/r MAINTAINER_REPO="$clr" PROD_GLOBS="bin/*" CLAIM_LABEL=claimed \
    MAINTAINER_PROFILE=of MAINTAINER_POST=off bash "$mg" merge 1 2>&1)"
grep -q 'was pointed at #355' <<<"$out" \
    && ok "the maintainer's own offers still count when the configured account is in another case" \
    || bad "a MAINTAINER_ACCOUNT in another case hid every offer, and the gate merged over one"

# And the refusal has to be readable. The holder list was printed with no
# separator after the last name, so the message read "TayfurYldzwas pointed at".
thread_gh '[{"u":"testuser","b":"@holder this one is yours"}]'
out="$(cl_merge)"
grep -q 'holder was pointed at #355' <<<"$out" \
    && ok "the refusal separates the holder's name from the sentence after it" \
    || bad "the refusal runs the handle into the next word: $(grep -o '[a-z]*was pointed at' <<<"$out")"

# The other side of the same comparison. Mentions are canonicalised to lower
# case at the source, so the gate's match against the author is the one place
# left where a login keeps the case it was registered in. QinXi-ai is a real
# contributor on sysknife: offered as @qinxi-ai, their pull request would have
# been refused as somebody else's.
thread_gh '[{"u":"testuser","b":"@newcomer this one is yours"}]' NewComer
out="$(cl_merge)"
grep -q 'was pointed at #355' <<<"$out" \
    && bad "an author registered with capitals is refused the issue offered to them in lower case" \
    || ok "the author NewComer is the person offered it as @newcomer"

# Two holders, so the list itself is on show.
thread_gh '[{"u":"testuser","b":"@alpha this one is yours"},{"u":"testuser","b":"@beta this one too"}]'
out="$(cl_merge)"
grep -q 'alpha, beta was pointed at #355' <<<"$out" \
    && ok "a refusal naming two holders separates them" \
    || bad "two holders ran together: $(grep -o '[a-z, ]*was pointed at' <<<"$out")"

# A labelled issue WITH an assignee: posting on it is not the same as being the
# person it was given to. The old check let any author through who had commented,
# so "I've got this implemented" from a stranger cleared a claim somebody else
# held. That is the half of sysknife#252 the thread check does not cover, for
# projects that reserve with labels and assignments rather than mentions.
assigned_gh() {   # $1 = assignee login
    cat > "$stub_dir/gh" <<GHEOF
#!/usr/bin/env bash
case "\$*" in
  *'auth switch'*) exit 0;;
  *'api user'*) echo testuser;;
  *reviewDecision*) echo APPROVED;;
  *closingIssuesReferences*) echo 355;;
  *'pr view'*author*) echo newcomer;;
  *issues/355/comments*) echo '[]';;
  *'issue view'*labels*) echo 0;;
  *'issue view'*assignees*) echo $1;;
  *'issue view'*comments*) echo 0;;
  *headRefOid*) echo $clsha;;
  *mergeStateStatus*) echo CLEAN;;
  *'pr checks'*) echo '[{"name":"x","bucket":"pass"}]';;
  *'pr merge'*) echo MERGED_STUB;;
esac
GHEOF
    chmod +x "$stub_dir/gh"
}
assigned_gh someone-else
out="$(cl_merge)"
grep -qE 'assigned to|is assigned' <<<"$out" \
    && ok "a labelled issue assigned to somebody else refuses another author's PR" \
    || bad "posting on a claimed issue still clears a claim held by its assignee"
assigned_gh newcomer
out="$(cl_merge)"
grep -qE 'assigned to|is assigned' <<<"$out" \
    && bad "the assignee is refused their own assigned issue" \
    || ok "the assignee of a claimed issue is not blocked"

# And the question failing is not the same as the answer being "nobody".
thread_gh_broken() {   # the comments call fails outright
    cat > "$stub_dir/gh" <<GHEOF
#!/usr/bin/env bash
case "\$*" in
  *'auth switch'*) exit 0;;
  *'api user'*) echo testuser;;
  *reviewDecision*) echo APPROVED;;
  *closingIssuesReferences*) echo 355;;
  *'pr view'*author*) echo newcomer;;
  *issues/355/comments*) echo "gh: server error" >&2; exit 1;;
  *'issue view'*labels*) echo null;;
  *'issue view'*comments*) echo 0;;
  *headRefOid*) echo $clsha;;
  *mergeStateStatus*) echo CLEAN;;
  *'pr checks'*) echo '[{"name":"x","bucket":"pass"}]';;
  *'pr merge'*) echo MERGED_STUB;;
esac
GHEOF
    chmod +x "$stub_dir/gh"
}
thread_gh_broken
out="$(cl_merge)"
grep -q 'could not read who was pointed at it' <<<"$out" \
    && ok "an unreadable thread refuses the merge instead of assuming nobody" \
    || bad "the holder check fails OPEN when it cannot read the issue"


echo "== every read the claim check depends on fails closed, not only the innermost one =="
# Issue #29. The innermost check (`maintainer holders`) was hardened to fail
# closed and its comment says so. The reads that decide whether it runs at all
# were not. Each of these swallowed its error into an empty string, and empty
# read as permission:
#
#   closingIssuesReferences   -> `for iss in $closes` runs zero times
#   the missing-helper break  -> leaves the loop before rc is ever set
#   issue view --json labels  -> `|| continue` skips the whole claim branch
#   issue view --json assignees -> reads as "nobody is assigned"
#   issue view --json comments  -> reads as "nobody else spoke"
#   pr view --json author       -> an empty author, compared with grep -qxF
#
# `maintainer holders` reads REST and these read GraphQL, so a partial outage on
# one and not the other left the claim protection off with nothing printed.
cl_gh_broken() {  # $1 = the case pattern whose gh call must fail
    cat > "$stub_dir/gh" <<GHEOF
#!/usr/bin/env bash
case "\$*" in
  $1) echo "gh: could not reach api.github.com" >&2; exit 1;;
  *'auth switch'*) exit 0;;
  *'api user'*) echo testuser;;
  *reviewDecision*) echo APPROVED;;
  *closingIssuesReferences*) echo 355;;
  *'pr view'*author*) echo newcomer;;
  *issues/355/comments*) echo '[]';;
  *'issue view'*labels*) echo 0;;
  *'issue view'*assignees*) echo "";;
  *'issue view'*comments*) printf '%s\n' claimant;;
  *headRefOid*) echo $clsha;;
  *mergeStateStatus*) echo CLEAN;;
  *'pr checks'*) echo '[{"name":"x","bucket":"pass"}]';;
  *'pr merge'*) echo MERGED_STUB;;
esac
GHEOF
    chmod +x "$stub_dir/gh"
}
# Asserted on the refusal TEXT, not on the exit code. The gate refuses this
# fixture later for an unrelated reason (the receipt is asserted rather than
# observed), so a non-zero exit proves nothing about the claim reads; the
# existing `claim_allowed` helper above is written the same way and for the same
# reason. Every new refusal below says "could not read", so that phrase is the
# observable.
broken_read_refuses() {  # $1 = case pattern, $2 = what it is
    cl_gh_broken "$1"
    local o; o="$(cl_merge)"
    if grep -q 'could not read' <<<"$o"; then
        ok "an unreadable $2 refuses the merge"
    else
        bad "an unreadable $2 let the claim check run on nothing: $(printf '%s' "$o" | tr '\n' ' ' | cut -c1-80)"
    fi
}
# The regression this pair exists to prevent. `gh --jq` prints NOTHING for a
# JSON null, so "this issue does not carry the claim label" arrives as an empty
# string with rc=0. Hardening the read to refuse on empty turned the ordinary
# case into a hard refusal and blocked every merge whose closed issue is
# unlabelled, which is most of them. Measured against the live tracker:
# `[.labels[].name]|index("claimed")` gave [] for #396 and [3] for #336.
cl_gh null claimant
out="$(cl_merge)"
grep -q 'could not read its labels' <<<"$out" \
    && bad "an unlabelled issue is refused as unreadable, which blocks most merges" \
    || ok "an issue without the claim label is not mistaken for an unreadable one"
grep -q 'carries the .* label' <<<"$out" \
    && bad "the claim branch ran on an issue that has no claim label" \
    || ok "and the claim branch is skipped for it"

broken_read_refuses "*closingIssuesReferences*"   "list of issues the PR closes"
broken_read_refuses "*'pr view'*author*"          "pull request author"
broken_read_refuses "*'issue view'*labels*"       "claim label"
broken_read_refuses "*'issue view'*assignees*"    "assignee list"
broken_read_refuses "*'issue view'*comments*"     "comment list"
# The helper that answers the thread question. Absent, the loop used to `break`
# before `rc` was ever set, so the fail-closed branch below it never ran.
# A whole little tree, because maintainer-merge sources ../lib/profile.sh
# relative to itself and falls back to the deployed copy under HOME. CI runs
# this job with no deployed tree on purpose, so a copy sitting next to no lib/
# died at line 15 and the case reported the gate as having merged. What must be
# absent here is `maintainer`, not everything.
nohelp="$stub_dir/nohelperroot"; rm -rf "$nohelp"; mkdir -p "$nohelp/bin" "$nohelp/lib"
cp "$mg" "$nohelp/bin/maintainer-merge"
cp "$root/lib/profile.sh" "$nohelp/lib/profile.sh"; cp "$root/lib/defaults.env" "$nohelp/lib/defaults.env"
cl_gh 0 claimant
out="$(PATH="$stub_dir:/usr/bin:/bin" MAINTAINER_STATE="$cl" MAINTAINER_ACCOUNT=testuser \
    MAINTAINER_SLUG=o/r MAINTAINER_REPO="$clr" PROD_GLOBS="bin/*" CLAIM_LABEL=claimed \
    MAINTAINER_PROFILE=of MAINTAINER_POST=off bash "$nohelp/bin/maintainer-merge" merge 1 2>&1)"; rc=$?
# The lease gate looks for the same helper first and refuses first, with its own words;
# either refusal is the helper's absence being treated as "cannot ask", never as "free".
grep -q 'could not read\|cannot tell whether' <<<"$out" \
    && ok "a missing holders helper refuses the merge" \
    || bad "with no helper on PATH the gate merged without asking who holds the issue: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-80)"
grep -q 'maintainer' <<<"$out" \
    && ok "and the refusal names the helper it could not run" \
    || bad "the refusal does not say what was missing: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-80)"
# The twin, and it is the one that matters: with every read answering, an
# unclaimed issue still merges. A guard that refuses everything protects nothing
# because it gets turned off.
cl_gh null claimant
printf '%s' "$(cl_merge)" | claim_allowed \
    && ok "with every read answering, an unclaimed issue is not blocked" \
    || bad "the hardened reads now block a merge they should allow"

echo "== the agent cannot uninstall the thing that runs it =="
# On 2026-09-04 an unattended run of the magent profile, reviewing THIS
# repository at POST=off, ran `./install.sh --uninstall` against the real HOME
# while root-causing a failing uninstall test. It disabled all six timers,
# including the four maintaining a different project, then died before removing
# anything. Nothing ran unattended for three and a half hours and nothing said
# so. The rehearsal wall stopped it reaching GitHub and had nothing to say about
# the scheduler.
uh="$stub_dir/unins"; mkdir -p "$uh/.config/systemd/user" "$uh/.local/bin"
# A "live" deployment: systemctl is stubbed to report an enabled timer.
make_stub systemctl 'case "$*" in
  *"show-environment"*) exit 0;;
  *"list-timers"*) echo "Fri 2026-09-05 09:13 maintainer@sysknife-review.timer"; exit 0;;
  *) exit 0;; esac'
out=$(PATH="$stub_dir:$PATH" HOME="$uh" bash "$root/install.sh" --uninstall 2>&1); rc=$?
[ "$rc" = 3 ] && ok "uninstall refuses while timers are enabled" \
    || bad "uninstall proceeded against a live deployment (rc=$rc)"
grep -q 'maintains a repository other than this one' <<<"$out" \
    && ok "and says it would stop another project's profile too" \
    || bad "the refusal does not say what else it would stop"
out=$(PATH="$stub_dir:$PATH" HOME="$uh" MAINTAINER_UNINSTALL_YES=1 \
      bash "$root/install.sh" --uninstall 2>&1); rc=$?
[ "$rc" = 0 ] && ok "and proceeds when explicitly told to" \
    || bad "the override does not work, so uninstall is unreachable (rc=$rc)"
# It must also never stop half way. The previous version put cron removal at the
# end of an && list, so a crontab that exists but cannot write killed the script
# under set -e after the timers were off and before a file was removed.
if sc_case 2; then
grep -v '^[[:space:]]*#' "$root/install.sh" \
    | grep -q 'command -v crontab.*&&.*&&.*run' \
    && bad "cron removal is still an && list; set -e aborts the uninstall mid-way" \
    || ok "no uninstall step can abort the ones after it"
grep -q '_failed' "$root/install.sh" \
    && ok "and a failed step is reported rather than silently skipped" \
    || bad "a failing uninstall step leaves no trace"
sc_end; fi
# The wall spells it too, in every profile.
for pdj in "$root"/profiles/*/deny.json; do
    pn="$(basename "$(dirname "$pdj")")"
    grep -q 'install.sh --uninstall' "$pdj" \
        && ok "$pn denies install.sh --uninstall" \
        || bad "$pn lets the agent uninstall the maintainer"
    grep -q 'systemctl --user disable' "$pdj" \
        && ok "$pn denies disabling a timer" \
        || bad "$pn lets the agent turn off its own scheduler"
done

