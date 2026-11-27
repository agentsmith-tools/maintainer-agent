#!/usr/bin/env bash
# Part of the offline suite: the audit trail, finding filing, state growth, the tree under test and tunables.
# tests/run-tests.sh sources this file in one shell, after its own preamble and after
# the parts before it, so every helper and variable it reads comes from there. It is
# sourced, never run on its own, and carries a shebang so the shell sweeps pick it up.

# The parts share one shell. These names are set by the driver or by an earlier part;
# declare only tells the linter they are read here, and assigns and changes nothing.
declare covp
declare forge
declare mg
declare root
declare stub_dir
declare suite_files
declare suite_root

echo "== the trail says what happened, including when nothing did =="
tr1="$stub_dir/trail"; mkdir -p "$tr1/runs" "$tr1/logs" "$tr1/state" "$tr1/drafts"
trrun() { MAINTAINER_STATE="$tr1" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
    MAINTAINER_ACCOUNT=t MAINTAINER_PROFILE=tr MAINTAINER_TASKS="review ci" \
    python3 "$root/bin/maintainer" "$@" 2>&1; }

# 1. A fixture is not a report. runs/2026-09-02T14-39-review.md in this
# project's own trail is "baseline promotion test", indexed like a real run.
printf '# 2026-01-01T00-00-review\n\nbaseline promotion test\n' \
    > "$tr1/runs/2026-01-01T00-00-review.md"
out="$(trrun finish 2026-01-01T00-00-review)"; rc=$?
[ "$rc" != 0 ] && grep -q 'fixture rather than a report' <<<"$out" \
    && ok "a three-line fixture is refused rather than indexed" \
    || bad "a fixture was indexed as a run (rc=$rc)"
grep -q '2026-01-01T00-00-review' "$tr1/index.md" 2>/dev/null \
    && bad "the refused fixture reached the index anyway" \
    || ok "and it did not reach the index"
# A real report still passes.
printf '# 2026-01-02T00-00-review\n\nQueue empty. Checked five PRs.\n\nNothing posted.\n\nNothing merged.\n' \
    > "$tr1/runs/2026-01-02T00-00-review.md"
trrun finish 2026-01-02T00-00-review >/dev/null 2>&1
grep -q '2026-01-02T00-00-review' "$tr1/index.md" 2>/dev/null \
    && ok "a real report is still indexed" || bad "the fixture check refuses real reports"

# The preamble's quiet-run instruction and finish's floor are one number. They
# disagreed from 2026-09-04 to 2026-10-25 (#133): the preamble asked for two
# lines, finish refused anything under three, and 12 of 32 magent runs aborted.
min_lines="$(sed -n 's/^REPORT_MIN_LINES=//p' "$root/lib/defaults.env")"
case "$min_lines" in ''|*[!0-9]*) bad "REPORT_MIN_LINES missing from lib/defaults.env: '$min_lines'"; min_lines=3 ;; esac
grep -q "at least $min_lines non-empty lines" "$root/lib/preamble-core.md" \
    && ok "the preamble asks a quiet run for the line count finish enforces ($min_lines)" \
    || bad "the preamble's quiet-run report length is not REPORT_MIN_LINES=$min_lines"
grep -qiE 'two-line report|one-line report' "$root/lib/preamble-core.md" \
    && bad "the preamble still asks for a report finish will refuse" \
    || ok "and no shorter report is asked for anywhere in it"
{ printf '# 2026-01-05T00-00-review\n\n'; for i in $(seq 1 "$min_lines"); do printf 'line %s\n' "$i"; done; } \
    > "$tr1/runs/2026-01-05T00-00-review.md"
trrun finish 2026-01-05T00-00-review >/dev/null 2>&1
grep -q '2026-01-05T00-00-review' "$tr1/index.md" 2>/dev/null \
    && ok "a quiet report of exactly the asked length is indexed" \
    || bad "finish refuses a report of exactly the length the preamble asks for"
{ printf '# 2026-01-06T00-00-review\n\n'; for i in $(seq 2 "$min_lines"); do printf 'line %s\n' "$i"; done; } \
    > "$tr1/runs/2026-01-06T00-00-review.md"
trrun finish 2026-01-06T00-00-review >/dev/null 2>&1
grep -q '2026-01-06T00-00-review' "$tr1/index.md" 2>/dev/null \
    && bad "a report one line short of REPORT_MIN_LINES was indexed" \
    || ok "and one line fewer is still refused"
out="$(MAINTAINER_REPORT_MIN_LINES=1 trrun finish 2026-01-06T00-00-review)"
grep -q '2026-01-06T00-00-review' "$tr1/index.md" 2>/dev/null \
    && ok "finish reads its floor from REPORT_MIN_LINES, not a literal" \
    || bad "finish ignores MAINTAINER_REPORT_MIN_LINES: $out"

# 2. A run that died leaves an ABORTED line, not a gap. A gap reads as a run
# that never started; 2026-09-03T07-52-review is the real case, and fifteen
# comments went out under the same account in the hour after it.
trrun abort 2026-01-03T00-00-review "the run wrote no usable report" >/dev/null 2>&1
grep -q 'ABORTED' "$tr1/index.md" 2>/dev/null \
    && ok "a run that produced nothing is recorded as ABORTED" \
    || bad "a dead run leaves the index silent"

# 3. The failure marker, the layer that cannot fail. notify-send guesses a
# display and ends in `|| true`, so on a headless box a nightly failure reaches
# nobody and says nothing about it.
trrun failed review "backend claude unusable" /tmp/x.log >/dev/null 2>&1
[ -f "$tr1/state/failed-review.json" ] && ok "a failure is written to disk" \
    || bad "the only record of a failure is a desktop popup"
trrun status 2>/dev/null | head -8 | grep -q 'FAILED' \
    && ok "and maintainer status prints it without being asked" \
    || bad "a recorded failure is invisible to the front door"
# Driven through `finish`, not through `ok`. The first version of this called
# `maintainer ok review`, which is a different code path, so deleting the
# clear_failure call out of cmd_finish left the test green.
printf '# 2026-01-04T00-00-review\n\nQueue empty.\n\nNothing posted.\n\nNothing merged.\n' \
    > "$tr1/runs/2026-01-04T00-00-review.md"
trrun finish 2026-01-04T00-00-review >/dev/null 2>&1
[ -f "$tr1/state/failed-review.json" ] \
    && bad "a run that finished properly left the previous failure marker standing" \
    || ok "a run that finishes clears its own task's failure marker"
# And only its own. A review succeeding says nothing about the ci sweep.
trrun failed ci "postgres contract went red" >/dev/null 2>&1
printf '# 2026-01-05T00-00-review\n\nQueue empty.\n\nNothing posted.\n\nNothing merged.\n' \
    > "$tr1/runs/2026-01-05T00-00-review.md"
trrun finish 2026-01-05T00-00-review >/dev/null 2>&1
[ -f "$tr1/state/failed-ci.json" ] \
    && ok "and leaves another task's failure alone" \
    || bad "a review clearing the ci failure hides a red sweep"
trrun ok ci >/dev/null 2>&1
if sc_case 3; then
grep -q 'HELPER" failed' "$root/lib/run.sh" \
    && ok "run.sh records the marker before it tries any channel" \
    || bad "run.sh still bets everything on notify-send"
grep -v '^[[:space:]]*#' "$root/lib/run.sh" | grep -q 'DISPLAY:-:1' \
    && bad "run.sh still hardcodes DISPLAY=:1" \
    || ok "the display is read from the session rather than assumed"
grep -q 'MAINTAINER_ALERT_CMD' "$root/lib/run.sh" \
    && ok "a profile can name its own escalation channel" \
    || bad "there is no way to reach ntfy, mail or a webhook"
sc_end; fi

echo "== a report is a claim; the transcript is the record =="
# lessons.md 12: a report read `sysknife-maint screen 348 -> DO NOT EXECUTE` for
# a command that had been renamed out of existence. Nothing compared the two.
au="$stub_dir/audit"; mkdir -p "$au/runs" "$au/logs"
aurun() { MAINTAINER_STATE="$au" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
    MAINTAINER_ACCOUNT=t MAINTAINER_PROFILE=au python3 "$root/bin/maintainer" audit "$@" 2>&1; }
printf '# r\n\n```\n$ gh pr list --state open\n$ cargo nextest run --workspace\n```\n' \
    > "$au/runs/2026-01-01T00-00-review.md"
printf '$ gh pr list --state open\n' > "$au/logs/2026-01-01T00-00-review.commands"
out="$(aurun 2026-01-01T00-00-review)"
grep -q '1 of 2 quoted command' <<<"$out" \
    && ok "a command the report quotes and the record lacks is named" \
    || bad "the audit did not notice a quoted command that never ran: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-80)"
grep -q 'cargo nextest run --workspace' <<<"$out" \
    && ok "and it prints which one" || bad "the audit says a count and not which command"
# Commands the report RECOMMENDS are not claims. Only `$ ` inside a fence is.
printf '# r\n\nRun `gh pr merge 7` yourself.\n\n```\n$ gh pr list --state open\n```\n' \
    > "$au/runs/2026-01-02T00-00-review.md"
printf '$ gh pr list --state open\n' > "$au/logs/2026-01-02T00-00-review.commands"
aurun 2026-01-02T00-00-review | grep -q 'all 1 quoted command' \
    && ok "a command the report recommends is not counted as one it ran" \
    || bad "the audit treats a recommendation as a claim, which fires on every report"
# A claim over 40 characters is matched whole, not on its first 40: a changed
# flag in the tail is exactly what the audit exists to catch. An abridged claim
# with `...` must still have each fragment in the record, so a changed tail
# after the ellipsis is caught too.
printf '# r\n\n```\n$ gh pr merge 5 --repo owner/name --squash --admin\n$ gh run list --repo owner/name ... --json databaseId,status\n```\n' \
    > "$au/runs/2026-01-09T00-00-review.md"
printf '$ gh pr merge 5 --repo owner/name --squash --rebase\n$ gh run list --repo owner/name --json databaseId,conclusion\n' \
    > "$au/logs/2026-01-09T00-00-review.commands"
out="$(aurun 2026-01-09T00-00-review)"
grep -q 'gh pr merge 5 --repo owner/name --squash --admin' <<<"$out" \
    && grep -q 'gh run list --repo owner/name ... --json databaseId,status' <<<"$out" \
    && ok "a long claim with a changed tail, and an abridged claim with a changed tail, are both reported as missing" \
    || bad "a claim whose tail differs from the record passes the audit: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"
# A shell continuation at the end of an abridged command is report formatting,
# not a required fragment in the transcript.
printf '# r\n\n```\n$ gh run list --repo owner/name ... --json databaseId,status,... \\\n```\n' \
    > "$au/runs/2026-01-10T00-00-review.md"
printf '$ gh run list --repo owner/name --json databaseId,status,conclusion\n' \
    > "$au/logs/2026-01-10T00-00-review.commands"
out="$(aurun 2026-01-10T00-00-review)"
grep -q 'all 1 quoted command(s) appear' <<<"$out" \
    && ok "an abridged claim ending in a shell continuation matches the complete command" \
    || bad "the shell continuation is treated as a required command fragment: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"
# Require both fragments and their original order.
printf '# r\n\n```\n$ gh run list --limit 5 ... --repo owner/name\n```\n' \
    > "$au/runs/2026-01-11T00-00-review.md"
printf '$ --repo owner/name gh run list --limit 5\n' \
    > "$au/logs/2026-01-11T00-00-review.commands"
out="$(aurun 2026-01-11T00-00-review)"
grep -q 'gh run list --limit 5 ... --repo owner/name' <<<"$out" \
    && ok "abridged command fragments must appear in order" \
    || bad "a claim with out-of-order fragments passed the audit"
# A short head would let any record holding the tail pass: the 12-character floor stays.
printf '# r\n\n```\n$ gh ... --force-with-lease\n```\n' > "$au/runs/2026-01-14T00-00-review.md"
printf '$ gh pr list\n$ git push --force-with-lease\n' > "$au/logs/2026-01-14T00-00-review.commands"
out="$(aurun 2026-01-14T00-00-review)"
grep -q 'gh ... --force-with-lease' <<<"$out" \
    && ok "an abridged claim with a head shorter than 12 characters is not matched" \
    || bad "a claim with a short head passed the audit"
# An ellipsis-only claim has no evidence and must not pass as an empty sequence.
printf '# r\n\n```\n$ ...\n```\n' > "$au/runs/2026-01-12T00-00-review.md"
printf '$ gh pr list --state open\n' > "$au/logs/2026-01-12T00-00-review.commands"
out="$(aurun 2026-01-12T00-00-review)"
grep -q '1 of 1 quoted command(s) are not in the record' <<<"$out" \
    && ok "an ellipsis-only claim is rejected" \
    || bad "an empty fragment list passed the audit"
# The first fragment is evidence too; matching only a later fragment is not a
# match for the command the report claims it ran.
printf '# r\n\n```\n$ gha ... --json databaseId\n```\n' \
    > "$au/runs/2026-01-13T00-00-review.md"
printf '$ gh run list --json databaseId\n' \
    > "$au/logs/2026-01-13T00-00-review.commands"
out="$(aurun 2026-01-13T00-00-review)"
grep -q 'gha ... --json databaseId' <<<"$out" \
    && ok "the first abridged fragment must also appear" \
    || bad "a claim with a missing first fragment passed the audit"
# A claim without `...` must equal one whole recorded command, so a prefix of
# `gh pr merge 123` or a dropped `--admin` is not a match. tests/test_audit_claims.py
# was written to be run by hand, so nothing ran it: a revert of the fix would have
# passed CI. MX_TREE makes it judge the tree under test.
echo "== a quoted command without ... matches one whole recorded command =="
MX_TREE="$root" python3 -B "$suite_root/tests/test_audit_claims.py" >/dev/null 2>&1 \
    && ok "a claim without ... is not a prefix of a longer recorded command, and matches whole in any position" \
    || bad "_claim_in matches a prefix of a recorded command (tests/test_audit_claims.py)"

# No record at all must read differently from a record that matched.
rm -f "$au/logs/2026-01-02T00-00-review.commands"
aurun 2026-01-02T00-00-review | grep -q 'no command record' \
    && ok "a run with no transcript says so rather than passing" \
    || bad "a missing transcript read as a clean audit"
# And a clean audit ends clean. The first version printed the `?` legend
# unconditionally, so auditing one good run finished on a sentence about a
# symbol that never appeared, which reads as a finding.
aurun 2026-01-01T00-00-review | grep -q '^?' \
    && bad "a legend printed for a symbol no run produced" \
    || ok "only the legends that apply are printed"

# A run that died leaves a log and no report, and `audit --all` walked runs/
# only, so the dead run was invisible to the one tool whose job is to say what
# happened. lessons.md calls this shape absence recorded as absence: a gap in
# the index reads as a quiet week. Measured on the real trail: 13 logs with no
# report, one of them the 2026-09-04T08:14 alert, and no command ever said so.
printf '=== run.sh au/review 2026-01-03T00:00:00Z ===\n' > "$au/logs/2026-01-03T00-00-review.log"
out="$(aurun --all)"
grep -q '2026-01-03T00-00-review' <<<"$out" \
    && ok "a run that started and never reported is named by the audit" \
    || bad "a dead run is invisible to --all, so a gap reads as a quiet week"
grep -qi 'never reported\|no report' <<<"$out" \
    && ok "and the audit says what happened to it" \
    || bad "the dead run is listed with no verdict"
# Auditing one named run must not drag in every orphan.
aurun 2026-01-01T00-00-review | grep -q '2026-01-03T00-00-review' \
    && bad "auditing one run reported an unrelated orphan" \
    || ok "one named run audits only that run"
# Thirteen identical "started and never reported" lines is a warning people
# stop reading, which is lesson 41's failure at a different address. The logs
# already hold the reason; the audit prints it, so an orphan with NO reason is
# the one that stands out.
printf '=== run.sh au/review 2026-01-04T00:00:00Z ===\nmaintainer=v1 backend=claude model=opus log=x\nbackend claude unusable: missing settings.json\n' \
    > "$au/logs/2026-01-04T00-00-review.log"
out="$(aurun --all)"
grep -q 'backend claude unusable' <<<"$out" \
    && ok "an orphan carries the reason its log recorded" \
    || bad "the audit names a dead run and drops the reason sitting in its log"
# The wording moved when the causes became a table: a log that says nothing is
# `died-without-saying` now, which is a named class with a place to look rather
# than a shrug. What has to hold is that the two orphans do not read the same.
r_none="$(grep '2026-01-03T00-00-review' <<<"$out" | sed 's/.*reported: //')"
r_some="$(grep '2026-01-04T00-00-review' <<<"$out" | sed 's/.*reported: //')"
[ -n "$r_none" ] && [ -n "$r_some" ] && [ "$r_none" != "$r_some" ] \
    && ok "an orphan whose log says nothing reads differently from one that explains itself" \
    || bad "a log with a reason and a log with none read the same"
grep -q 'died-without-saying' <<<"$r_none" \
    && ok "and the silent one is named as such, with somewhere to look" \
    || bad "the silent orphan is not classified: '$r_none'"

echo "== a finding becomes a tracker issue, exactly once =="
# The magent loop: a review finds a defect, and instead of leaving it as prose
# in ~/.local/state, it files an issue. `maintainer file-issue` is the one path
# issue creation takes, and it deduplicates by fingerprint so re-finding a defect
# does not file it twice. That is also the fix for the sysknife #342/#343 pair:
# identical titles, filed nine minutes apart.
fi="$stub_dir/fissue"; rm -rf "$fi"; mkdir -p "$fi"
firun() { MAINTAINER_STATE="$fi" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
    MAINTAINER_ACCOUNT=testuser MAINTAINER_PROFILE=fp \
    PATH="$stub_dir:$PATH" python3 "$root/bin/maintainer" file-issue "$@" 2>&1; }

# An empty tracker: gh issue list returns [] so nothing is deduped against.
make_stub gh "case \"\$*\" in
  *'issue list'*) echo '[]';;
  *'api user'*) echo testuser;;
  *'issue create'*) echo 'https://github.com/o/r/issues/1';;
  *'auth switch'*) exit 0;;
esac"

# POST unset -> rehearsal: it says what it would file and posts nothing.
out=$(firun --title 'A guard exits 1 with no output' --body 'body text')
grep -qi 'would file' <<<"$out" \
    && ok "with POST off, file-issue rehearses rather than posting" \
    || bad "file-issue posted or errored at POST=off: $(printf '%s' "$out"|tr '\n' ' '|cut -c1-80)"

# POST=on, empty tracker -> it creates the issue and prints the URL.
out=$(MAINTAINER_POST=on firun --title 'A guard exits 1 with no output' --body 'body text')
grep -q 'issues/1' <<<"$out" \
    && ok "with POST on, a new finding is filed and its URL returned" \
    || bad "file-issue did not create the issue: $(printf '%s' "$out"|tr '\n' ' '|cut -c1-80)"

# The marker the body carries, so we can simulate an existing issue.
fp=$(python3 -c "import hashlib,re; t='a guard exits 1 with no output'; print(hashlib.sha1(t.encode()).hexdigest()[:16])")
marker="<!-- maintainer-finding: $fp -->"
make_stub gh "case \"\$*\" in
  *'issue list'*) printf '[{\"number\":7,\"state\":\"OPEN\",\"title\":\"A guard exits 1 with no output\",\"body\":\"old body $marker\"}]';;
  *'api user'*) echo testuser;;
  *'issue create'*) echo 'https://github.com/o/r/issues/99';;
  *'auth switch'*) exit 0;;
esac"

# Same title again -> deduped by fingerprint, even at POST=on, even worded anew.
out=$(MAINTAINER_POST=on firun --title 'A guard exits 1 with no output' --body 'reworded body')
if grep -q 'already filed as #7' <<<"$out" && ! grep -q 'issues/99' <<<"$out"; then
    ok "a finding already on the tracker is not filed twice (the #342/#343 case)"
else
    bad "file-issue filed a duplicate: $(printf '%s' "$out"|tr '\n' ' '|cut -c1-90)"
fi

# An explicit fingerprint dedups two DIFFERENT titles that are the same defect.
fp2=$(python3 -c "import hashlib; print(hashlib.sha1('the-defect'.encode()).hexdigest()[:16])")
m2="<!-- maintainer-finding: $fp2 -->"
make_stub gh "case \"\$*\" in
  *'issue list'*) printf '[{\"number\":8,\"state\":\"CLOSED\",\"title\":\"first wording\",\"body\":\"b $m2\"}]';;
  *'api user'*) echo testuser;;
  *'issue create'*) echo 'https://github.com/o/r/issues/100';;
  *'auth switch'*) exit 0;;
esac"
out=$(MAINTAINER_POST=on firun --title 'a completely different wording' --fingerprint the-defect --body 'b')
grep -q 'already filed as #8 (closed)' <<<"$out" \
    && ok "an explicit fingerprint dedups across different titles, and reports the closed one" \
    || bad "the explicit-fingerprint dedup did not fire: $(printf '%s' "$out"|tr '\n' ' '|cut -c1-90)"

# A missing body is refused rather than filing a titled-but-empty issue.
out=$(MAINTAINER_POST=on firun --title 'x'); rc=$?
[ "$rc" != 0 ] && grep -qi 'body' <<<"$out" \
    && ok "a finding with no body is refused" || bad "file-issue filed an issue with no body"

# Wrong identity refuses rather than filing under the wrong name.
make_stub gh "case \"\$*\" in *'issue list'*) echo '[]';; *'api user'*) echo somebodyelse;; *'auth switch'*) exit 0;; esac"
out=$(MAINTAINER_POST=on firun --title 'y' --body 'b'); rc=$?
[ "$rc" != 0 ] && grep -q 'refusing to file' <<<"$out" \
    && ok "file-issue refuses to post under the wrong gh identity" \
    || bad "file-issue filed under an unverified identity: $(printf '%s' "$out"|tr '\n' ' '|cut -c1-80)"

echo "== the state directory does not grow forever =="
gcd="$stub_dir/gc"; mkdir -p "$gcd/logs" "$gcd/drafts/old" "$gcd/runs"
printf 'x\n' > "$gcd/logs/ancient.log"; printf 'x\n' > "$gcd/logs/fresh.log"
printf 'x\n' > "$gcd/drafts/old/d.md"; printf 'keep\n' > "$gcd/runs/report.md"
touch -d '200 days ago' "$gcd/logs/ancient.log" "$gcd/drafts/old"
gcrun() { MAINTAINER_STATE="$gcd" MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
    MAINTAINER_ACCOUNT=t MAINTAINER_PROFILE=gc python3 "$root/bin/maintainer" gc "$@" 2>&1; }
gcrun --dry-run | grep -q 'would remove.*ancient.log' \
    && ok "a dry run names what it would remove" || bad "gc --dry-run said nothing"
[ -f "$gcd/logs/ancient.log" ] && ok "and removes nothing" || bad "the dry run deleted a file"
gcrun >/dev/null 2>&1
[ -f "$gcd/logs/ancient.log" ] && bad "gc kept a log past the window" \
    || ok "a log past the window is removed"
[ -f "$gcd/logs/fresh.log" ] && ok "a log inside the window is kept" \
    || bad "gc deleted a recent log"
[ -d "$gcd/drafts/old" ] && bad "gc kept a drafts directory past the window" \
    || ok "a drafts directory past the window is removed"
[ -f "$gcd/runs/report.md" ] \
    && ok "runs/ is never pruned: it records what was published in your name" \
    || bad "gc DELETED A RUN REPORT, which is the record of what was posted"
# The default is chosen once, in lib/defaults.env, and a profile that declares
# nothing gets it. A profile may still override it.
grep -q '^RETENTION_DAYS=' "$root/lib/defaults.env" && ok "gc's retention window has a declared default" \
    || bad "no RETENTION_DAYS in lib/defaults.env, so gc falls back to a default nobody chose"

echo "== a task that fans out declares the model its subagents get =="
# A review fans out across files and dimensions. Running that on the main
# loop's model pays opus prices for independent work sonnet does as well, and
# the thinking budget is spent in the main loop, not in the fan-out.
# Comments stripped first, for the fourth time today. The paragraph explaining
# why _FORCE is NOT used contains the word _FORCE, and a bare grep failed the
# check that enforces the decision. docs/lessons.md 31 is this exact shape.
code() { grep -v '^[[:space:]]*#' "$1"; }
if sc_case 2; then
code "$root/lib/backends/claude.sh" | grep -q 'export CLAUDE_CODE_SUBAGENT_MODEL=' \
    && ok "the claude backend exports a subagent model" \
    || bad "SUBAGENT_MODEL_<task> is declared and never reaches the process"
code "$root/lib/backends/claude.sh" | grep -q 'CLAUDE_CODE_SUBAGENT_MODEL_FORCE' \
    && bad "the backend FORCES every subagent onto one model, overriding definitions that chose their own" \
    || ok "it sets a default, so an agent definition's own model still wins"
sc_end; fi
for pe in "$root"/profiles/*/profile.env; do
    pn="$(basename "$(dirname "$pe")")"
    tasks="$(sed -n 's/^TASKS="\(.*\)"/\1/p' "$pe")"
    # Declared only where it means something. A sweep that spawns nothing must
    # not carry a setting that reads as though it does.
    bogus=""
    while read -r line; do
        tk="${line#SUBAGENT_MODEL_}"; tk="${tk%%=*}"
        case " $tasks " in *" $tk "*) ;; *) bogus="$bogus $tk" ;; esac
    done < <(grep '^SUBAGENT_MODEL_' "$pe" || true)
    [ -z "$bogus" ] && ok "$pn declares a subagent model only for tasks it runs" \
        || bad "$pn sets SUBAGENT_MODEL for$bogus, which is not in its TASKS"
done
# Driven, not grepped: the backend must actually put it in the environment.
sm="$stub_dir/submodel"; mkdir -p "$sm"
cat > "$sm/probe.sh" <<'PROBE'
PROFILE_DIR=/dev/null
MAINTAINER_TASK=review
THINKING_review=31999
SUBAGENT_MODEL_review=sonnet
MAINTAINER_SETTINGS=/dev/null
backend_run() { :; }
PROBE
if sc_case 2; then
out=$(bash -c '
    . "'"$sm"'/probe.sh"
    . "'"$root"'/lib/backends/claude.sh"
    # Re-run just the exporting prologue by calling backend_run with a stub claude.
    export PATH="'"$stub_dir"':$PATH"
    backend_run /dev/null opus /dev/null /tmp >/dev/null 2>&1
    echo "MAX_THINKING_TOKENS=$MAX_THINKING_TOKENS"
    echo "CLAUDE_CODE_SUBAGENT_MODEL=$CLAUDE_CODE_SUBAGENT_MODEL"' 2>&1)
grep -q 'CLAUDE_CODE_SUBAGENT_MODEL=sonnet' <<<"$out" \
    && ok "SUBAGENT_MODEL_review reaches the process as sonnet" \
    || bad "the subagent model never reached the environment: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90)"
grep -q 'MAX_THINKING_TOKENS=31999' <<<"$out" \
    && ok "and the main loop keeps its own budget" \
    || bad "the thinking budget was lost"
sc_end; fi

echo "== published text is written AS the maintainer, not about them =="
# 2026-09-09. Five sysknife pull request reviews posted under vladimirrott went
# out in the first person and then referred to Vladimir in the THIRD person in
# the same paragraph: "so it needs a person. Vladimir approves it.", "Vladimir
# has the report and the exact commands.", "Vladimir has to run four commands".
# Read from a contributor's side that says the account is being written by
# somebody who is not its owner, which is the disclosure the rule above forbids,
# arrived at sideways.
#
# The preamble said whose name is on the comment. It never said to write as
# them. Reviews go out through `gh pr review --body-file` and never pass through
# this tool, so the control is the doctrine and the review skill; there is no
# wall that can see a pronoun.
pc2="$root/lib/preamble-core.md"
grep -qi 'first person' "$pc2" \
    && ok "the doctrine says published text is written in the first person" \
    || bad "nothing tells the agent to write as the maintainer rather than about them"
grep -qi 'third person' "$pc2" \
    && ok "and names the third-person slip it is forbidding" \
    || bad "the rule does not name the mistake it prevents"
# Driven through the real assembly, so a rule that never reaches a prompt fails.
pv_settings="$stub_dir/personvoice.json"
printf '{"permissions":{"deny":[]}}\n' > "$pv_settings"
pv="$(MAINTAINER_SETTINGS="$pv_settings" bash "$root/lib/run.sh" --show-prompt sysknife review 2>&1)"
grep -qi 'first person' <<<"$pv" \
    && ok "and the rule reaches the assembled prompt" \
    || bad "the voice rule is in a file the prompt never carries"
# The substitution must not hand the agent its own maintainer's name as the
# thing to write, which is how the third person got there in the first place.
grep -q '__MAINTAINER__' "$pc2" \
    && ok "the doctrine still uses the placeholder rather than a hardcoded name" \
    || bad "a maintainer's name is hardcoded into the shared doctrine"

echo "== a subagent finding is a lead, and the doctrine says so =="
# The env var alone would make the agent WORSE: fan-out produces findings that
# arrive already written up, in the agent's own voice, and one taken at face
# value would have published a retraction of a correct result.
pc="$root/lib/preamble-core.md"
grep -q 'lead, not a result' "$pc" \
    && ok "the core preamble carries the subagent rule" \
    || bad "subagents are configured and the doctrine never mentions them"
grep -q 'Never delegate the decision' "$pc" \
    && ok "and says the decision is never delegated" \
    || bad "nothing stops a subagent's conclusion becoming a merge"
# Every profile gets it, because the preamble is shared and run.sh concatenates it.
if sc_case 1; then
grep -q 'preamble-core.md' "$root/lib/run.sh" \
    && ok "run.sh assembles that preamble into every prompt" \
    || bad "the doctrine is a file nothing reads"
sc_end; fi

echo "== the doctor reports on the profile it was asked about =="
# It globbed `maintainer@*` in systemctl, so with MAINTAINER_PROFILE=magent it
# counted sysknife's four timers and printed "4 systemd timer(s) registered, ok"
# while magent had two timer files, both disabled, and had never fired once.
# Same shape as the hardcoded profile default in bin/maintainer.
md="$root/bin/maintainer-doctor"
if sc_case 4; then
grep -q "maintainer@\$profile-\*" "$md" \
    && ok "the scheduler check names the profile" \
    || bad "the doctor counts every profile's timers and calls them this one's"
grep -q "list-timers 'maintainer@\*'" "$md" \
    && bad "an unscoped timer glob is still in maintainer-doctor" \
    || ok "no unscoped timer glob remains"
# list-timers shows loaded units only, so a disabled timer is absent rather than
# listed as off. Counting the unit FILES is what separates "none installed" from
# "installed and never enabled", and those need different fixes.
grep -q 'list-unit-files' "$md" \
    && ok "it separates a missing timer from a disabled one" \
    || bad "a disabled timer reads the same as a missing one"
grep -q 'none enabled: nothing runs unattended' "$md" \
    && ok "and it says out loud that nothing runs unattended" \
    || bad "a profile with no enabled timer is not told so"
sc_end; fi

echo "== a verdict is read from output, not from a pipeline's exit code =="
# `doctor | grep -q` returns doctor's status under pipefail, not the match. One
# case in this file passed for two runs that way while the check it tested did
# not exist. The rule already appears as a comment beside the maintainer-merge
# cases; a comment did not stop the repeat.
piped="$(grep -nE '^\s*(vdoc|bash "\$(d|md|mg)")[^|]*\| *grep' "${suite_files[@]}" || true)"
if [ -z "$piped" ]; then
    ok "no case decides from a pipeline whose left side can fail"
else
    bad "a case pipes a failing-capable command into grep: $(printf '%s' "$piped" | head -1)"
fi

echo "== a skip is not a pass =="
# The container cases used to `bad` when no runtime was installed, so the suite
# could never be green on a machine without podman or docker, which is every
# container. Counting them as passes would be worse: docs/lessons.md already has
# an entry where a missing optional dependency turned a gate green over seven
# real errors. They are their own count, printed, and CI requires it to be zero.
grep -q 'skip=$((skip+1))' "$suite_root/tests/run-tests.sh" \
    && ok "a skip has its own counter" || bad "a skip is folded into pass or fail"
grep -q 'skipped' "$root/.github/workflows/ci.yml" \
    && ok "CI fails when the suite skips anything" \
    || bad "CI would accept a run that skipped half the cases"
# Proved by driving the summary: a suite with a skip must say so in its last line.
probe="$stub_dir/skipprobe.sh"
{ sed -n '1,/^check(){/p' "$suite_root/tests/run-tests.sh"
  printf 'noenv "a deliberate skip"\n'
  sed -n '/^if \[ "\$skip" -gt 0 \]/,$p' "$suite_root/tests/run-tests.sh"; } > "$probe"
out=$(env -u MX_SKIP_SOURCE_COUPLED MX_TREE="$root" bash "$probe" 2>&1)
grep -qE '0 passed, 0 failed, 1 skipped' <<<"$out" \
    && ok "and the summary line names the skipped count" \
    || bad "a skipped case left no trace in the summary: $(printf '%s' "$out" | tail -1)"

echo "== the suite tests the tree it is told to test, and says which =="
# MX_TREE points the suite at any tree with this layout, so a build whose programs
# are not shell can be held to the same cases. suite_root is the checkout that
# holds this file, and only the suite's own text goes through it. These cases drive
# the head of the suite, the part that picks the tree, the way the skip case above
# drives the summary. A shell tree and a Rust tree meet the same head.
mxn=0
mx_probe() {  # $1 = where the probe lives, $2 = lines to run between head and summary, then env args
    local at="$1" body="$2"; shift 2
    mkdir -p "$(dirname "$at")"
    { sed -n '1,/^check(){/p' "$suite_root/tests/run-tests.sh"
      printf '%s\n' "$body"
      sed -n '/^if \[ "\$skip" -gt 0 \]/,$p' "$suite_root/tests/run-tests.sh"; } > "$at"
    env -u MX_TREE -u MX_SKIP_SOURCE_COUPLED "$@" bash "$at" 2>&1
}
mx_fake_tree() {  # $1 = dir, then the members to leave out
    local d="$1"; shift
    rm -rf "$d"; mkdir -p "$d/bin" "$d/lib" "$d/profiles"
    : > "$d/bin/maintainer-merge"; : > "$d/lib/run.sh"
    local gone; for gone in "$@"; do rm -rf "${d:?}/$gone"; done
}
mxp="$stub_dir/mxprobe"
out="$(mx_probe "$mxp/probe.sh" 'ok "probe case"' MX_TREE="$root")"; rc=$?
[ "$rc" = 0 ] && [ "$(head -1 <<<"$out")" = "tree under test: $root" ] \
    && ok "the first line of output names the tree under test" \
    || bad "the output opens with '$(head -1 <<<"$out")' (rc=$rc), not the tree under test"
mx_fake_tree "$mxp/checkout"; mkdir -p "$mxp/checkout/tests"
out="$(mx_probe "$mxp/checkout/tests/probe.sh" 'ok "probe case"')"; rc=$?
[ "$rc" = 0 ] && [ "$(head -1 <<<"$out")" = "tree under test: $mxp/checkout" ] \
    && ok "with no MX_TREE the tree under test is the checkout that holds the suite" \
    || bad "the default tree is '$(head -1 <<<"$out")' (rc=$rc), not the checkout holding the suite"
mx_fake_tree "$mxp/other"
out="$(cd "$mxp" && mx_probe "$mxp/checkout/tests/probe.sh" 'ok "probe case"' MX_TREE=other)"; rc=$?
[ "$rc" = 0 ] && [ "$(head -1 <<<"$out")" = "tree under test: $mxp/other" ] \
    && ok "a relative MX_TREE is resolved to an absolute path before any case runs" \
    || bad "a relative MX_TREE gave '$(head -1 <<<"$out")' (rc=$rc)"
for gone in bin/maintainer-merge lib/run.sh profiles; do
    mxn=$((mxn+1)); mx_fake_tree "$mxp/short$mxn" "$gone"
    out="$(mx_probe "$mxp/probe.sh" 'ok "probe case"' MX_TREE="$mxp/short$mxn")"; rc=$?
    [ "$rc" != 0 ] && [ "$(wc -l <<<"$out")" = 1 ] && grep -q "$mxp/short$mxn" <<<"$out" && ! grep -q 'PASS' <<<"$out" \
        && ok "a tree without $gone is refused in one line, before any case runs" \
        || bad "a tree without $gone was not refused cleanly (rc=$rc): $(printf '%s' "$out" | head -2 | tr '\n' ' ' | cut -c1-160)"
done
mkdir -p "$mxp/empty"
out="$(mx_probe "$mxp/probe.sh" 'ok "probe case"' MX_TREE="$mxp/empty")"; rc=$?
[ "$rc" != 0 ] && [ "$(wc -l <<<"$out")" = 1 ] && ! grep -q 'PASS' <<<"$out" \
    && ok "an empty directory is refused in one line, before any case runs" \
    || bad "an empty directory was not refused cleanly (rc=$rc): $(printf '%s' "$out" | head -2 | tr '\n' ' ' | cut -c1-160)"
out="$(mx_probe "$mxp/probe.sh" 'ok "probe case"' MX_TREE="$mxp/no-such-dir")"; rc=$?
[ "$rc" != 0 ] && [ "$(wc -l <<<"$out")" = 1 ] && ! grep -q 'PASS' <<<"$out" \
    && ok "a path that does not exist is refused in one line, before any case runs" \
    || bad "a missing tree was not refused cleanly (rc=$rc): $(printf '%s' "$out" | head -2 | tr '\n' ' ' | cut -c1-160)"
# The source-coupled blocks. A tree whose programs are not shell text cannot be
# grepped or sourced, so MX_SKIP_SOURCE_COUPLED=1 must not run the block at all.
mxm="$stub_dir/mx-block-ran"; rm -f "$mxm"
mxblock="if sc_case 2; then : > '$mxm'; ok 'never runs'; ok 'never runs either'; sc_end; fi; ok 'runs'"
out="$(mx_probe "$mxp/probe.sh" "$mxblock" MX_TREE="$root" MX_SKIP_SOURCE_COUPLED=1)"; rc=$?
grep -qE '^ +1 passed, 0 failed, 2 skipped \(2 source-coupled cases\)' <<<"$out" && [ "$rc" = 0 ] \
    && ok "MX_SKIP_SOURCE_COUPLED=1 counts a skipped block as one skip per case" \
    || bad "the skip summary is wrong (rc=$rc): $(tail -3 <<<"$out" | tr '\n' ' ' | cut -c1-200)"
[ ! -e "$mxm" ] \
    && ok "and the skipped block does not run" \
    || bad "a source-coupled block ran although MX_SKIP_SOURCE_COUPLED=1 was set"
rm -f "$mxm"
out="$(mx_probe "$mxp/probe.sh" "$mxblock" MX_TREE="$root")"; rc=$?
grep -qE '^ +3 passed, 0 failed \(2 source-coupled cases\)' <<<"$out" && [ "$rc" = 0 ] && [ -e "$mxm" ] \
    && ok "without the variable the block runs, and the summary names how many cases are source-coupled" \
    || bad "the normal summary is wrong (rc=$rc): $(tail -3 <<<"$out" | tr '\n' ' ' | cut -c1-200)"
out="$(mx_probe "$mxp/probe.sh" "if sc_case 2; then ok 'one'; sc_end; fi" MX_TREE="$root")"; rc=$?
[ "$rc" != 0 ] && grep -q 'declares 2 case(s) and recorded 1' <<<"$out" \
    && ok "a block that records fewer cases than it declares fails" \
    || bad "a short block passed (rc=$rc): $(tail -3 <<<"$out" | tr '\n' ' ' | cut -c1-200)"
out="$(mx_probe "$mxp/probe.sh" "if sc_case 1; then ok 'one'; ok 'two'; sc_end; fi" MX_TREE="$root")"; rc=$?
[ "$rc" != 0 ] && grep -q 'declares 1 case(s) and recorded 2' <<<"$out" \
    && ok "a block that records more cases than it declares fails" \
    || bad "an overfull block passed (rc=$rc): $(tail -3 <<<"$out" | tr '\n' ' ' | cut -c1-200)"
out="$(mx_probe "$mxp/probe.sh" "if sc_case 1; then ok 'one'; fi; if sc_case 1; then ok 'two'; sc_end; fi" MX_TREE="$root")"; rc=$?
[ "$rc" != 0 ] && grep -q 'never closed with sc_end' <<<"$out" \
    && ok "a block that is never closed fails at the next one" \
    || bad "an unclosed block passed (rc=$rc): $(tail -3 <<<"$out" | tr '\n' ' ' | cut -c1-200)"
out="$(mx_probe "$mxp/probe.sh" "if sc_case 0; then ok 'one'; sc_end; fi" MX_TREE="$root")"; rc=$?
[ "$rc" != 0 ] && grep -q 'needs a positive case count' <<<"$out" && ! grep -q 'PASS' <<<"$out" \
    && ok "a block that declares no cases fails and does not run" \
    || bad "a block with a zero count was accepted (rc=$rc): $(tail -3 <<<"$out" | tr '\n' ' ' | cut -c1-200)"
out="$(mx_probe "$mxp/probe.sh" "sc_end; ok 'one'" MX_TREE="$root")"; rc=$?
[ "$rc" != 0 ] && grep -q 'closes no sc_case' <<<"$out" \
    && ok "an sc_end with no block open fails" \
    || bad "a stray sc_end was accepted (rc=$rc): $(tail -3 <<<"$out" | tr '\n' ' ' | cut -c1-200)"
rm -rf "$mxp"
# Two roots stay apart. The first needle is written so that this line does not match
# it: the check reads the suite's own text, and a pattern that matches itself would
# fail the day it was added.
# The suite is this driver and every part, so each check reads all of them.
stray="$(grep -nE '^[^#]*\$\{?roo[t]\}?/tests/' "${suite_files[@]}" || true)"
[ -z "$stray" ] \
    && ok "the suite reads its own text through suite_root, never through the tree under test" \
    || bad "a case reads the suite through the tree under test: $(printf '%s' "$stray" | head -1 | cut -c1-120)"
stray="$(grep -nE '^[^#]*\$\{?suite_roo[t]\}?/(bin|lib|profiles|platform|scripts|evals|install\.sh|new-profile\.sh)' "${suite_files[@]}" || true)"
[ -z "$stray" ] \
    && ok "and no program, profile or script is reached through suite_root" \
    || bad "a case runs or reads a program from the suite's own checkout: $(printf '%s' "$stray" | head -1 | cut -c1-120)"

echo "== a part of the suite cannot be dropped without the suite going red and naming it =="
# run-tests.sh sources its parts from an explicit list. Each case below copies the driver next
# to stand-in parts, damages one thing, and runs the copy against the real tree under test.
# The first case is the control: the undamaged copy must pass and report its total, so the
# damaged runs below fail for the damage and not for the harness.
pg="$stub_dir/part-guard"
pg_list="$(grep -E '^    [0-9][0-9]-[a-z0-9-]+\.sh$' "$suite_root/tests/run-tests.sh" | tr -d ' ')"
pg_n="$(grep -c . <<<"$pg_list")"
pg_first="$(head -n 1 <<<"$pg_list")"
pg_second="$(sed -n 2p <<<"$pg_list")"
pg_build() {  # a fresh copy of the driver with a stand-in part of three cases for every listed part
    rm -rf "$pg"; mkdir -p "$pg/tests/suites"
    cp "$suite_root/tests/run-tests.sh" "$pg/tests/run-tests.sh"
    local name
    while read -r name; do
        [ -n "$name" ] || continue
        printf '#!/usr/bin/env bash\nfor _i in 1 2 3; do ok "stand-in case"; done\n' > "$pg/tests/suites/$name"
    done <<<"$pg_list"
}
pg_run() { env -u MX_SKIP_SOURCE_COUPLED MX_TREE="$root" bash "$pg/tests/run-tests.sh" 2>&1; }
pg_expect() {  # $1 = case title, $2 = fixed text the output must carry, with a nonzero exit
    [ "$rc" != 0 ] && grep -qF -- "$2" <<<"$out" \
        && ok "$1" \
        || bad "$1: not refused by name (rc=$rc): $(tail -n 3 <<<"$out" | tr '\n' ' ' | cut -c1-200)"
}
pg_build; out="$(pg_run)"; rc=$?
[ "${pg_n:-0}" -ge 2 ] && [ "$rc" = 0 ] && grep -qE "^ +$((pg_n*3)) passed, 0 failed" <<<"$out" \
    && ok "an undamaged copy of the driver with stand-in parts passes and reports its total ($pg_n parts)" \
    || bad "the stand-in copy did not pass with $pg_n parts (rc=$rc): $(tail -n 3 <<<"$out" | tr '\n' ' ' | cut -c1-200)"
pg_build; rm "$pg/tests/suites/$pg_first"; out="$(pg_run)"; rc=$?
pg_expect "a deleted part fails the suite and names it" "tests/suites/$pg_first is missing"
pg_build; : > "$pg/tests/suites/$pg_first"; out="$(pg_run)"; rc=$?
pg_expect "an emptied part fails the suite and names it" "tests/suites/$pg_first does not start with a bash shebang"
pg_build; printf '#!/usr/bin/env bash\n' > "$pg/tests/suites/$pg_first"; out="$(pg_run)"; rc=$?
pg_expect "a part that records no case fails the suite and names it" "tests/suites/$pg_first recorded no case"
pg_build; mv "$pg/tests/suites/$pg_first" "$pg/tests/suites/99-renamed.sh"; out="$(pg_run)"; rc=$?
[ "$rc" != 0 ] && grep -qF "tests/suites/$pg_first is missing" <<<"$out" && grep -qF 'tests/suites/99-renamed.sh is not in the part list' <<<"$out" \
    && ok "a renamed part fails the suite and names both the old and the new file" \
    || bad "a renamed part was not refused by name (rc=$rc): $(tail -n 3 <<<"$out" | tr '\n' ' ' | cut -c1-200)"
pg_build; sed -i "/^    $pg_first\$/d" "$pg/tests/run-tests.sh"
out="$(pg_run)"; rc=$?
pg_expect "a part left out of the list fails the suite and names it" "tests/suites/$pg_first is not in the part list"
pg_build; sed -i "s/^    $pg_first\$/    $pg_first\n    $pg_first/" "$pg/tests/run-tests.sh"
out="$(pg_run)"; rc=$?
pg_expect "a part listed twice fails the suite and names it" "tests/suites/$pg_first is listed twice"
pg_build; printf '# no shebang here\nok "stand-in case"\n' > "$pg/tests/suites/$pg_first"; out="$(pg_run)"; rc=$?
pg_expect "a part with no bash shebang fails the suite and names it (the sweeps would never lint it)" "tests/suites/$pg_first does not start with a bash shebang"
pg_build; printf 'ok "one"\nexit 0\n' >> "$pg/tests/suites/$pg_second"; out="$(pg_run)"; rc=$?
pg_expect "a part that ends the shell with exit 0 fails the suite instead of ending it green" "the suite ended before its summary"
pg_build; printf 'for _i in 1 2 3 4; do ok "extra"; done\n' >> "$pg/tests/suites/$pg_second"; out="$(pg_run)"; rc=$?
[ "$rc" = 0 ] && grep -qE "^ +$((pg_n*3+4)) passed, 0 failed" <<<"$out" \
    && ok "a part that records more cases than before does not trip the guard, so data-driven loops can grow" \
    || bad "a part with extra cases was refused (rc=$rc): $(tail -n 3 <<<"$out" | tr '\n' ' ' | cut -c1-200)"
rm -rf "$pg"

echo "== the gate needs nothing that is not already required =="
# Run inside python:3.12, five tests failed with `jq: command not found`, and
# the refusal read "#7 has  failing check(s)" with a blank where the number
# goes. jq was the only dependency the gate had that nothing else here needs.
# Whole-line comments dropped first. The paragraph explaining why jq is gone
# matched a grep for jq, which is the same over-match that made the "CI reads no
# secrets" check fail on the comment describing the policy.
if sc_case 1; then
if grep -v '^[[:space:]]*#' "$mg" | grep -qE '\| *jq |\$\( *jq |^ *jq '; then
    bad "maintainer-merge still shells out to jq"
else
    ok "maintainer-merge needs no jq; gh's own --jq is not a second binary"
fi
sc_end; fi
# And a count that is not a number must say so rather than interpolate a blank.
make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *reviewDecision*) echo APPROVED;;
      *headRefOid*) echo abcdef1234567890;;
      *mergeStateStatus*) echo CLEAN;;
      *'pr checks'*) printf 'not json at all';;
    esac"
PATH="$stub_dir:$PATH" MAINTAINER_STATE="$forge" MAINTAINER_ACCOUNT=testuser \
    MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
    bash "$mg" receipt 9 abcdef1234567890 'proved' >/dev/null 2>&1
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$forge" MAINTAINER_ACCOUNT=testuser \
      MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp PROD_GLOBS="bin/*" \
      bash "$mg" merge 9 2>&1); rc=$?
[ "$rc" != 0 ] && ok "an unreadable check list refuses the merge" \
    || bad "the gate merged on a check list it could not read"
grep -qE 'could not read the check list|not a number' <<<"$out" \
    && ok "and it names the check list rather than reporting a blank count" \
    || bad "the refusal blamed something else: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90)"

echo "== a receipt for an already-merged pull request is stale, not open =="
# sysknife#399 was merged outside the gate (BEHIND, and its fork forbade the
# branch update), so `gh pr merge --admin` landed it and the gate never consumed
# its receipt. The receipt then sat in receipts/ describing work that had already
# shipped, and doctor counted it as open. I removed it by hand after noticing.
# A receipt that outlives its merge is indistinguishable from one guarding work
# still in flight, which is the only thing receipts exist to tell apart.
if sc_case 1; then
grep -qE 'already merged|merged pull request|stale receipt' "$root/bin/maintainer-doctor" \
    && ok "doctor can tell an open receipt from one whose pull request already merged" \
    || bad "doctor counts a receipt for merged work as though the work were pending"
sc_end; fi

echo "== the preamble warns that a two-dot diff blames the branch for the base =="
# On 2026-09-10 `git diff main..399` showed a markdown-link fix rolling a third
# party GitHub Action back four patch releases, six days older, unmentioned in
# its title. The supply-chain reading was the obvious one and it was wrong: the
# author branched before a dependabot bump landed, their commits touch zero pin
# lines, and the three-way merge keeps main's newer pin. I came close to saying
# it out loud to a contributor.
_pc="$root/lib/preamble-core.md"
grep -q 'main\.\.\.' "$_pc" \
    && ok "the preamble names the three-dot form for reading what a branch did" \
    || bad "nothing tells a review which diff range answers 'what did this PR change'"
grep -qiE 'two-dot|\.\.pr|blames the (branch|pull request)' "$_pc" \
    && ok "and it names the two-dot trap rather than only the remedy" \
    || bad "the preamble gives the right command without saying what goes wrong"
grep -qiE 'merge base|merged tree|log -S' "$_pc" \
    && ok "and it says how to confirm who introduced a line before accusing anyone" \
    || bad "nothing says to check authorship before calling a diff a revert"

echo "== a clean run that fails on the environment does not blame the pull request =="
# Three times now the gate could not run and reported the contributor as failing:
# an image with no python3 (sysknife#386), an image with no git (sysknife#410),
# and /tmp mounted noexec (sysknife#410 again). Each surfaced as
#   maintainer-merge: the test does not pass unmutated (rc=1)
# which reads as "your test is broken" and is the most expensive sentence this
# tool can say to somebody who did nothing wrong.
envlog="$stub_dir/envfail"; mkdir -p "$envlog"
printf 'running the suite\n/repo/check.sh: line 3: git: command not found\n' > "$envlog/a.log"
printf 'bash: /tmp/tmp.XyZ/stub.sh: Permission denied\n' > "$envlog/b.log"
printf 'FAIL: the guard did not fire\nassertion failed\n' > "$envlog/c.log"
if sc_case 2; then
grep -q 'looks_environmental\|environment_hint' "$mg" \
    && ok "maintainer-merge has a way to tell an environment failure from a test failure" \
    || bad "nothing distinguishes 'your test failed' from 'my container lacks a binary'"
# The message the contributor would read.
grep -qE 'suite environment|not the pull request|my (container|image)' "$mg" \
    && ok "and it says the failure is the gate's environment, not the contributor" \
    || bad "the refusal still reads as though the pull request failed"
sc_end; fi
# And behaviourally, in a real container, because the three times this happened
# the grep-able words were all present and the message was still wrong.
if [ -n "$rt" ]; then
    er="$stub_dir/envrepo"; rm -rf "$er"; mkdir -p "$er"
    git -C "$er" init -q -b main; git -C "$er" config user.email t@t; git -C "$er" config user.name t
    # A test that invokes a binary the suite image does not carry. This is
    # sysknife#410's `git init` fixture in miniature.
    printf '#!/usr/bin/env bash\nset -e\ndefinitely-not-a-real-binary --version\necho "guard held"\n' \
        > "$er/check.sh"
    git -C "$er" add -A; git -C "$er" commit -qm base
    ehead="$(git -C "$er" rev-parse HEAD)"
    git -C "$er" update-ref "refs/pull/43/head" "$ehead"
    git -C "$er" remote add origin "$er"
    make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *files*) echo check.sh;;
    esac"
    out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$stub_dir/estate" MAINTAINER_ACCOUNT=testuser \
          MAINTAINER_SLUG=o/r MAINTAINER_REPO="$er" PROFILE_DIR="$root/profiles/sysknife" \
          bash "$mg" verify 43 "$ehead" check.sh 's/guard held/guard gone/' shell 2>&1)
    grep -qiE 'my suite environment|not the pull request' <<<"$out" \
        && ok "a missing binary in the suite image is reported as the gate's problem" \
        || bad "the gate blamed the contributor for its own missing binary: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-110)"
    grep -qi 'does not pass unmutated' <<<"$out" \
        && bad "it still leads with 'the test does not pass unmutated', which reads as the PR failing" \
        || ok "and it does not lead with the sentence that reads as the contributor failing"
    grep -qiE 'verify\.d|suite_image|suite_needs' <<<"$out" \
        && ok "and it points at the suite definition, which is where the fix goes" \
        || bad "the message does not say where to fix it"
else
    ok "no container runtime here, so the environment-failure case is not exercised (skipped)"
fi

echo "== the merge says when it closed no issue =="
# Three of QinXi-ai's four pull requests on 2026-09-10 declared no closing
# keyword, so merging them left #238 and #239 open with the work already landed.
# I closed #238 by hand after noticing. The gate reads closingIssuesReferences
# already; it just never said the list was empty.
mnothing="$stub_dir/mnothing"; rm -rf "$mnothing"; mkdir -p "$mnothing"
mnrepo="$stub_dir/mnrepo"; rm -rf "$mnrepo"; mkdir -p "$mnrepo"
git -C "$mnrepo" init -q -b main
git -C "$mnrepo" config user.email t@t; git -C "$mnrepo" config user.name t
printf 'x\n' > "$mnrepo/thing.txt"; git -C "$mnrepo" add -A; git -C "$mnrepo" commit -qm base
mnhead="$(git -C "$mnrepo" rev-parse HEAD)"
git -C "$mnrepo" update-ref refs/pull/9/head "$mnhead"
git -C "$mnrepo" remote add origin "$mnrepo"
make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *closingIssuesReferences*) echo '';;
      *reviewDecision*) echo APPROVED;;
      *headRefOid*) echo $mnhead;;
      *mergeStateStatus*) echo CLEAN;;
      *'pr checks'*) echo '[{\"name\":\"x\",\"bucket\":\"pass\"}]';;
      *'pr view'*'files'*) echo 'thing.txt';;
      *'pr merge'*) exit 0;;
    esac"
PATH="$stub_dir:$PATH" MAINTAINER_STATE="$mnothing" MAINTAINER_ACCOUNT=testuser \
    MAINTAINER_SLUG=o/r MAINTAINER_REPO="$mnrepo" \
    bash "$mg" receipt 9 "$mnhead" 'proved' >/dev/null 2>&1
out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$mnothing" MAINTAINER_ACCOUNT=testuser \
      MAINTAINER_SLUG=o/r MAINTAINER_REPO="$mnrepo" PROD_GLOBS="bin/*" \
      bash "$mg" merge 9 2>&1)
grep -qiE 'closes no issue|closed no issue|no issue was closed' <<<"$out" \
    && ok "a merge that closes nothing says so, so the issue is not left open by accident" \
    || bad "the gate merged silently and nothing warned that no issue closes: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90)"

echo "== two merges of one pull request cannot both run =="
# On 2026-09-10 I left five background shells watching one merge and two of them
# would each have run `maintainer-merge merge 415`. GitHub serialised the last
# duplicate pair by luck, which MISTAKES rule 9 already calls luck rather than
# design. One watcher per action is a habit; a lock is a mechanism.
mlock="$stub_dir/mlock"; rm -rf "$mlock"; mkdir -p "$mlock"
if sc_case 1; then
grep -qE 'flock' "$mg" \
    && ok "the merge takes a lock rather than trusting one caller at a time" \
    || bad "nothing stops two concurrent merges of the same pull request"
sc_end; fi
# And behaviourally, because a grep for `flock` proves the word is present, not
# that a second caller is stopped.
#
# This needs the REAL flock. $stub_dir carries `make_stub flock 'exit 0'` from
# the top of this file, which returns success without taking anything, so the
# gate's lock always appears free and the case cannot happen. The first version
# of this test ran under that stub, reported "two merges both proceeded", and
# the lock it was accusing was working perfectly. Same shape as the permissive
# podman stub that once manufactured a receipt two hundred cases later.
# One directory holding only the real flock, ahead of $stub_dir. Putting
# /usr/bin ahead instead pulls in the real gh and the refusal becomes an
# identity error, which is a different test failing for a different reason.
lkbin="$stub_dir/lkbin"; rm -rf "$lkbin"; mkdir -p "$lkbin"
ln -sf "$(PATH=/usr/local/bin:/usr/bin:/bin command -v flock)" "$lkbin/flock"
lkrepo="$stub_dir/lkrepo"; rm -rf "$lkrepo"; mkdir -p "$lkrepo"
git -C "$lkrepo" init -q -b main
git -C "$lkrepo" config user.email t@t; git -C "$lkrepo" config user.name t
printf 'x\n' > "$lkrepo/thing.txt"; git -C "$lkrepo" add -A; git -C "$lkrepo" commit -qm base
lkhead="$(git -C "$lkrepo" rev-parse HEAD)"
git -C "$lkrepo" update-ref refs/pull/9/head "$lkhead"
git -C "$lkrepo" remote add origin "$lkrepo"
make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *closingIssuesReferences*) echo '';;
      *reviewDecision*) echo APPROVED;;
      *headRefOid*) echo $lkhead;;
      *mergeStateStatus*) echo CLEAN;;
      *'pr checks'*) echo '[]';;
      *'pr view'*'files'*) echo 'thing.txt';;
      *'pr merge'*) exit 0;;
    esac"
PATH="$stub_dir:$PATH" MAINTAINER_STATE="$mlock" MAINTAINER_ACCOUNT=testuser \
    MAINTAINER_SLUG=o/r MAINTAINER_REPO="$lkrepo" \
    bash "$mg" receipt 9 "$lkhead" 'proved' >/dev/null 2>&1
mkdir -p "$mlock/receipts"
# Hold the lock the way a first merge would, then try a second. Waiting on the
# condition rather than on a fixed sleep: the first version of this test slept
# one second, the merge took longer than the holder's lifetime, and the test
# reported the lock as broken when the lock was fine.
# Releases on a sentinel rather than a timer: killing a subshell that is
# sleeping leaves the sleep child holding the descriptor, so the lock outlives
# the kill and the "it is free again" assertion fails for the wrong reason.
( if flock -n 201 2>/dev/null; then
      touch "$mlock/held"
      while [ ! -e "$mlock/release" ]; do sleep 0.1; done
  else touch "$mlock/nolock"; fi ) 201>"$mlock/receipts/.merge-9.lock" &
lkpid=$!
lkwait=0
until [ -e "$mlock/held" ] || [ "$lkwait" -ge 100 ]; do sleep 0.1; lkwait=$((lkwait+1)); done
until [ -e "$mlock/held" ] || [ -e "$mlock/nolock" ] || [ "$lkwait" -ge 100 ]; do sleep 0.1; lkwait=$((lkwait+1)); done
[ -e "$mlock/held" ] && ok "the first holder really took the lock before the second call ran" \
    || bad "the holder did not get the lock (nolock=$([ -e "$mlock/nolock" ] && echo yes || echo no)), so the next assertion would be vacuous"
# /usr/bin ahead of $stub_dir so the real flock wins; the gh stub is still
# reached because $stub_dir stays on the path behind it.
out=$(PATH="$lkbin:$stub_dir:$PATH" MAINTAINER_STATE="$mlock" MAINTAINER_ACCOUNT=testuser \
      MAINTAINER_SLUG=o/r MAINTAINER_REPO="$lkrepo" PROD_GLOBS="bin/*" \
      bash "$mg" merge 9 2>&1); lrc=$?
[ "$lrc" != 0 ] && ok "a second merge of the same pull request is refused while the first holds the lock" \
    || bad "two merges of one pull request both proceeded"
grep -qiE 'already running|merge lock' <<<"$out" \
    && ok "and the refusal names the concurrent merge rather than something else" \
    || bad "the refusal blamed something else: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90)"
touch "$mlock/release"; wait "$lkpid" 2>/dev/null
# With the lock free again the same call gets past it, so the lock is what
# stopped it rather than the stub.
out=$(PATH="$lkbin:$stub_dir:$PATH" MAINTAINER_STATE="$mlock" MAINTAINER_ACCOUNT=testuser \
      MAINTAINER_SLUG=o/r MAINTAINER_REPO="$lkrepo" PROD_GLOBS="bin/*" \
      bash "$mg" merge 9 2>&1)
grep -qiE 'already running' <<<"$out" \
    && bad "the lock stayed held after the first caller exited" \
    || ok "and once the first caller exits the lock is free again"

echo "== one verify per pull request, because it is the expensive half =="
# On 2026-09-14 a hand-run `verify 426` and the unattended review timer's
# `verify 426` ran side by side on one host: two full cargo builds of the same
# workspace against two extracted trees. The supervisor read the page cache they
# filled as memory exhaustion and killed five tasks while MemAvailable was
# 21.9 GiB. Two actors for one action, MISTAKES rule 9, and cmd_merge had taken
# a lock for this four days earlier while verify, which builds twice, had not.
vlock="$stub_dir/vlock"; rm -rf "$vlock"; mkdir -p "$vlock/receipts"
make_stub podman 'echo ok'
( if flock -n 202 2>/dev/null; then
      touch "$vlock/held"
      while [ ! -e "$vlock/release" ]; do sleep 0.1; done
  else touch "$vlock/nolock"; fi ) 202>"$vlock/receipts/.verify-9.lock" &
vpid=$!
vwait=0
until [ -e "$vlock/held" ] || [ -e "$vlock/nolock" ] || [ "$vwait" -ge 100 ]; do sleep 0.1; vwait=$((vwait+1)); done
[ -e "$vlock/held" ] \
    && ok "the first verify really took the lock before the second call ran" \
    || bad "the holder did not get the lock, so the next assertion would be vacuous"
out=$(PATH="$lkbin:$stub_dir:$PATH" MAINTAINER_STATE="$vlock" MAINTAINER_ACCOUNT=testuser \
      MAINTAINER_SLUG=o/r MAINTAINER_REPO="$lkrepo" PROFILE_DIR="$covp" MAINTAINER_PROFILE=cov \
      bash "$mg" verify 9 "$lkhead" somefilter 's/a/b/' 2>&1); vrc=$?
# One assertion, not two. A bare "it exited non-zero" passes without the lock,
# because this fixture profile refuses the changed path anyway, and a check that
# passes for the wrong reason is the shape this file exists to catch. Measured:
# with the lock removed, the rc-only assertion still said PASS.
[ "$vrc" != 0 ] && grep -qiE 'another verify of #9 is already running' <<<"$out" \
    && ok "a second verify is refused, and the refusal names the concurrent verify" \
    || bad "two verifies of one pull request both proceeded, or it failed for another reason: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-100)"
touch "$vlock/release"; wait "$vpid" 2>/dev/null
out=$(PATH="$lkbin:$stub_dir:$PATH" MAINTAINER_STATE="$vlock" MAINTAINER_ACCOUNT=testuser \
      MAINTAINER_SLUG=o/r MAINTAINER_REPO="$lkrepo" PROFILE_DIR="$covp" MAINTAINER_PROFILE=cov \
      bash "$mg" verify 9 "$lkhead" somefilter 's/a/b/' 2>&1)
grep -qiE 'another verify of #9 is already running' <<<"$out" \
    && bad "the lock stayed held after the first caller exited" \
    || ok "and once the first caller exits the verify lock is free again"

echo "== the merge is pinned to the head that was checked =="
if sc_case 1; then
grep -q 'match-head-commit' "$mg" && ok "gh pr merge is pinned to the verified head" \
    || bad "a push landing mid-merge would be merged unchecked"
sc_end; fi
echo "== naming a suite does not skip the coverage check =="
if sc_case 1; then
grep -q 'does not cover every changed path' "$mg" \
    && ok "an explicitly named suite must still cover the changed paths" \
    || bad "naming a suite bypasses coverage, and the name is reachable from MCP"
sc_end; fi

echo "== the suite never writes into the index of the commit that runs it =="
# 2026-10-21: `git commit -a` runs pre-commit with GIT_INDEX_FILE pointing at a
# temporary index for the commit in progress. This suite inherited it, and its
# scratch repositories' `git add -A -f` wrote their files into that index. The
# lint sweep after it then saw "0 scripts" and failed closed, which is the only
# reason a commit of scratch content did not go through.
leaked=""
for v in $(git rev-parse --local-env-vars); do
    [ -n "${!v+x}" ] && leaked="$leaked $v"
done
[ -z "$leaked" ] \
    && ok "no per-repository git variable reaches the cases (git rev-parse --local-env-vars)" \
    || bad "the cases inherit$leaked, so their scratch repos write into the caller's repository"

echo "== doctor says when the default branch has been red for hours =="
# 2026-10-19: RustSec published four rustls advisories on 10-14, the next push
# to sysknife's main turned security-audit red on 10-16, and main stayed red
# until 10-19. Every open pull request showed a failing required check that was
# not its author's, and nothing this agent runs said a word: the doctor looked
# at timers, receipts and identity, never at the repository it maintains.
rh="$stub_dir/redhome"; rm -rf "$rh"
mkdir -p "$rh/.local/share/maintainer/profiles/p" "$rh/.local/bin"
printf 'REPO_SLUG="o/r"\n' > "$rh/.local/share/maintainer/profiles/p/profile.env"
red_doctor() {  # $1 = conclusion of main's latest ci run, $2 = its age in hours, $3 = gh exit code
    local created; created="$(date -u -d "-$2 hours" +%Y-%m-%dT%H:%M:%SZ)"
    make_stub gh "case \"\$*\" in
        *'run list'*) [ '$3' = 0 ] || { echo 'error connecting to api.github.com' >&2; exit $3; }
            printf '%s\\n' '[{\"workflowName\":\"ci\",\"status\":\"completed\",\"conclusion\":\"$1\",\"createdAt\":\"$created\",\"url\":\"https://example.test/run/1\"},{\"workflowName\":\"ci\",\"status\":\"completed\",\"conclusion\":\"success\",\"createdAt\":\"2020-01-01T00:00:00Z\",\"url\":\"https://example.test/run/0\"}]' ;;
        *) exit 0 ;;
    esac"
    PATH="$stub_dir:$PATH" HOME="$rh" MAINTAINER_PROFILE=p bash "$root/bin/maintainer-doctor" --quick 2>&1
}
rout="$(red_doctor failure 70 0)"
grep -q "main's latest 'ci' run failed" <<<"$rout" && grep -q 'FAIL.*main has been red' <<<"$rout" \
    && ok "a ci run red for 70 hours is a doctor failure that names the workflow" \
    || bad "main red for 70 hours reads as healthy: $(grep -i 'red\|main' <<<"$rout" | tr '\n' ' ' | cut -c1-120)"
rout="$(red_doctor failure 1 0)"
grep -q 'FAIL.*main has been red' <<<"$rout" \
    && bad "a run that failed an hour ago is already an alarm; a fix may be on its way" \
    || ok "a failure younger than the 6-hour grace raises no alarm"
rout="$(red_doctor success 70 0)"
grep -q 'FAIL.*main has been red' <<<"$rout" \
    && bad "a green main is reported red, so the latest run is not the one being read" \
    || ok "a green latest run is not reported red, even with an older failure behind it"
rout="$(red_doctor success 1 1)"
grep -q 'could not read' <<<"$rout" \
    && ok "when gh cannot list the runs, doctor says it could not ask instead of passing" \
    || bad "an unreadable run list reads as a healthy main: $(grep -i 'main\|run' <<<"$rout" | tr '\n' ' ' | cut -c1-120)"
# gh answering 0 with nothing usable is the other way not to know. The mutation
# that turned this branch green survived the cases above, because each of them
# either parsed or had gh exit non-zero.
for body in '[]' 'not json'; do
    make_stub gh "case \"\$*\" in *'run list'*) printf '%s\\n' '$body' ;; *) exit 0 ;; esac"
    rout="$(PATH="$stub_dir:$PATH" HOME="$rh" MAINTAINER_PROFILE=p bash "$root/bin/maintainer-doctor" --quick 2>&1)"
    grep -q 'could not read' <<<"$rout" && ! grep -q 'no workflow on main has been red' <<<"$rout" \
        && ok "a run list of '$body' is reported as unreadable, not as a green main" \
        || bad "a run list of '$body' reads as a healthy main"
done
rm -f "$stub_dir/gh"

echo "== every tunable has one home: env var > profile.env > lib/defaults.env =="
# The defaults were literals scattered through bin/ and lib/, some written twice.
# 2g of container memory OOM-killed rustc twice on 2026-10-25, and the fix was a
# different number in three places. One file now holds every default with a
# comment, bin/ and lib/ read it, and a suite may declare its own resources.
dflt="$root/lib/defaults.env"
[ -f "$dflt" ] && ok "lib/defaults.env exists" || bad "there is no lib/defaults.env"
dget() { sed -n "s/^$1=//p" "$dflt" | head -1; }
# Plain KEY=value so bash can source it and python can parse it with one regex,
# and a comment line above every key so a default is never a bare number.
python3 - "$dflt" <<'PYEOF' && ok "every default is a plain KEY=value with a comment above it" \
    || bad "lib/defaults.env has a line that is not KEY=value, or a key with no comment"
import re, sys
lines = open(sys.argv[1]).read().splitlines()
bad = []
for i, ln in enumerate(lines):
    if not ln.strip() or ln.startswith("#"):
        continue
    if not re.fullmatch(r"[A-Z][A-Z0-9_]*=[^\s\"'$`]+", ln):
        bad.append(f"line {i+1}: {ln!r}")
    elif i == 0 or not lines[i-1].startswith("#"):
        bad.append(f"line {i+1}: no comment above {ln.split('=')[0]}")
sys.exit("; ".join(bad) if bad else 0)
PYEOF
# No numeric default may be written inline in a tool again.
if sc_case 1; then
lits="$(grep -nE '\$\{MAINTAINER_[A-Z_]+:-[1-9]|size=[0-9]+m|--limit "[0-9]+"|--limit [0-9]+ ' \
    "$root"/bin/* "$root"/lib/*.sh "$root"/lib/backends/* 2>/dev/null | grep -v '^[^:]*:[0-9]*:[[:space:]]*#' || true)"
[ -z "$lits" ] && ok "bin/ and lib/ carry no inline numeric default" \
    || bad "a default is written inline again: $(printf '%s' "$lits" | head -3 | tr '\n' ' ' | cut -c1-200)"
sc_end; fi

# Behaviour: drive `maintainer-merge verify` against a podman stub that records
# its argv, so the flags a real container would get are the thing under test.
tvh="$stub_dir/tunhome"; rm -rf "$tvh"
tvp="$tvh/.local/share/maintainer/profiles/tv"
mkdir -p "$tvp/verify.d" "$tvh/.local/bin"
printf 'REPO_SLUG="o/r"\nGH_ACCOUNT="testuser"\nPOST="off"\nSTATE_DIR="%s/state"\n' "$tvh" > "$tvp/profile.env"
cat > "$tvp/verify.d/s.sh" <<'SUITE'
# shellcheck shell=bash
suite_covers() { case "$1" in *.txt) return 0 ;; esac; return 1; }
suite_mutate_glob() { printf '*.txt'; }
suite_image() { printf 'img:x'; }
suite_command() { printf 'true'; }
suite_ran() { printf 1; }
suite_prefetch_command() { printf 'true'; }
SUITE
tvrepo="$stub_dir/tunrepo"; rm -rf "$tvrepo"; mkdir -p "$tvrepo"
git -C "$tvrepo" init -q -b main
git -C "$tvrepo" config user.email t@t; git -C "$tvrepo" config user.name t
printf 'x\n' > "$tvrepo/thing.txt"; git -C "$tvrepo" add -A; git -C "$tvrepo" commit -qm base
tvhead="$(git -C "$tvrepo" rev-parse HEAD)"
git -C "$tvrepo" update-ref refs/pull/9/head "$tvhead"; git -C "$tvrepo" remote add origin "$tvrepo"
printf 'REPO_PATH="%s"\n' "$tvrepo" >> "$tvp/profile.env"
make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *'pr view'*'files'*) echo 'thing.txt';;
    esac"
make_stub podman 'printf "%s\n" "$@" >> "$TUN_ARGS"; printf -- "--END--\n" >> "$TUN_ARGS"; exit 0'
export TUN_ARGS="$stub_dir/tun.args"
# The suite exports MAINTAINER_STATE/ACCOUNT/SLUG/REPO from line ~1279 on, which
# would send the gate down its "called from inside a run" branch. Unset them, so
# the deployed profile is what this reads, and pass them back for the in-run case.
tun_verify() {  # env assignments as arguments; leaves the recorded argv in $TUN_ARGS
    : > "$TUN_ARGS"; rm -rf "$tvh/state"
    env -u MAINTAINER_SLUG -u MAINTAINER_REPO -u MAINTAINER_ACCOUNT -u MAINTAINER_STATE \
        PATH="$stub_dir:$PATH" HOME="$tvh" MAINTAINER_PROFILE=tv PROD_GLOBS="thing.txt" "$@" \
        bash "$root/bin/maintainer-merge" verify 9 "$tvhead" f 's/x/y/' >"$TUN_ARGS.out" 2>&1
}
# The value after each occurrence of a flag, one per container run, as "3x 6g".
tun_flag() {  # $1 = flag
    awk -v k="$1" 'p{print; p=0} $0==k{p=1}' "$TUN_ARGS" | sort | uniq -c | sed 's/^ *\([0-9]*\) /\1x /' | paste -sd';'
}
tun_verify
runs="$(grep -c '^--memory$' "$TUN_ARGS")"
[ "$runs" = 3 ] && ok "the harness reaches all three container runs (fetch, clean, mutated)" \
    || bad "the podman stub saw $runs container run(s), not 3, so the cases below are vacuous: $(tr '\n' ' ' < "$TUN_ARGS.out" | cut -c1-200)"
# Where a verify unpacks the pull request, and that it never leaves it there.
# On 2026-10-27 two refused verifies of sysknife#627 left 12 GB of extracted
# tree and cargo target in /tmp. That /tmp was a tmpfs: the leftovers sat in
# RAM, the next verify's rustc was SIGKILLed inside its 6g container, and the
# disk-quota error that followed cost a third run (#111). A RETURN trap never
# fires on `die`, and nearly every refusal is a `die`.
vw_tmp="$stub_dir/vw-tmpdir"; rm -rf "$vw_tmp"; mkdir -p "$vw_tmp"
mounted="$(awk 'p{print; p=0} $0=="-v"{p=1}' "$TUN_ARGS" | grep -m1 ':/repo' || true)"
case "$mounted" in
    "$tvh/state/verify-work/"*) ok "the pull request is unpacked under the state directory, not in TMPDIR" ;;
    *) bad "the verify tree is mounted from '${mounted%%:*}', not from \$STATE/verify-work" ;;
esac
tun_verify TMPDIR="$vw_tmp"
left="$(find "$vw_tmp" "$tvh/state/verify-work" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)"
[ "$left" = 0 ] && ok "a verify that ran to its verdict leaves no scratch directory behind" \
    || bad "a finished verify left $left scratch director(ies): $(find "$vw_tmp" "$tvh/state/verify-work" -mindepth 1 -maxdepth 1 2>/dev/null | head -2 | tr '\n' ' ')"
: > "$TUN_ARGS"; rm -rf "$tvh/state" "$vw_tmp"; mkdir -p "$vw_tmp"
env -u MAINTAINER_SLUG -u MAINTAINER_REPO -u MAINTAINER_ACCOUNT -u MAINTAINER_STATE \
    PATH="$stub_dir:$PATH" HOME="$tvh" MAINTAINER_PROFILE=tv PROD_GLOBS="thing.txt" TMPDIR="$vw_tmp" \
    bash "$root/bin/maintainer-merge" verify 9 0000000000000000000000000000000000000000 f 's/x/y/' >"$TUN_ARGS.out" 2>&1
vrc=$?
grep -q 'not the 0000' "$TUN_ARGS.out" && [ "$vrc" != 0 ] \
    && ok "the refusal case reaches the head check, so the case below is not vacuous" \
    || bad "the wrong-head verify did not refuse where expected (rc=$vrc): $(tr '\n' ' ' < "$TUN_ARGS.out" | cut -c1-160)"
left="$(find "$vw_tmp" "$tvh/state/verify-work" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)"
[ "$left" = 0 ] && ok "a refused verify (die) leaves no scratch directory behind" \
    || bad "a refused verify left $left scratch director(ies) behind (#111)"
tun_verify   # restore the recorded argv the resource cases below read

# No shipped script may pipe into `grep -q` (MISTAKES rule 2). grep exits on its
# first match, the writer takes SIGPIPE, and under `pipefail` the MATCH reads as
# a failure. v0.8.0 shipped three: fixup's sign-off check (seen flaking in the
# suite on 2026-10-27), prune's open-PR keep test, and the doctor's timer check.
if sc_case 1; then
pq_files=( "$root"/bin/* "$root"/lib/*.sh "$root"/lib/backends/* "$root"/install.sh "$root"/new-profile.sh \
           "$root"/platform/*/*.sh "$root"/profiles/*/verify.d/*.sh )
pq_n=0; for f in "${pq_files[@]}"; do [ -f "$f" ] && pq_n=$((pq_n + 1)); done
pq="$(grep -nE '\|[[:space:]]*grep[[:space:]]+-[a-zA-Z]*q' "${pq_files[@]}" 2>/dev/null \
    | grep -vE '^[^:]*:[0-9]+:[[:space:]]*#' || true)"
[ "$pq_n" -ge 10 ] || bad "the pipe-into-grep -q scan found only $pq_n shipped scripts; the glob is wrong"
[ -z "$pq" ] && ok "no shipped script pipes into grep -q ($pq_n scanned)" \
    || bad "a shipped script pipes into grep -q, which turns a match into a failure under pipefail: $(printf '%s' "$pq" | head -3 | tr '\n' ' ' | cut -c1-220)"
sc_end; fi
for pair in "memory:VERIFY_MEMORY" "cpus:VERIFY_CPUS" "pids-limit:VERIFY_PIDS" "timeout:VERIFY_TIMEOUT"; do
    flag="${pair%%:*}"; key="${pair##*:}"; want="$(dget "$key")"
    [ -n "$want" ] || { bad "lib/defaults.env has no $key"; continue; }
    got="$(tun_flag "--$flag")"
    case "$flag" in
        cpus) [ "$got" = "2x $want" ] ;;                       # clean and mutated only
        timeout) [ "$got" = "2x $want;1x $(dget PREFETCH_TIMEOUT)" ] ;;
        *) [ "$got" = "3x $want" ] ;;
    esac && ok "with nothing set, --$flag is the default from lib/defaults.env ($want)" \
          || bad "--$flag is '$got', not the default $want from lib/defaults.env"
done
grep -qx -- "--tmpfs" "$TUN_ARGS" \
    && [ "$(grep -c "^/tmp:rw,nosuid,size=$(dget VERIFY_TMPFS)\$" "$TUN_ARGS")" = 3 ] \
    && ok "and the /tmp tmpfs size comes from VERIFY_TMPFS" \
    || bad "the tmpfs size is not read from lib/defaults.env"

printf 'VERIFY_PIDS=77\n' >> "$tvp/profile.env"
tun_verify
[ "$(tun_flag --pids-limit)" = "3x 77" ] && ok "profile.env beats lib/defaults.env" \
    || bad "profile.env VERIFY_PIDS=77 did not reach the container: $(tun_flag --pids-limit)"
tun_verify MAINTAINER_VERIFY_PIDS=33
[ "$(tun_flag --pids-limit)" = "3x 33" ] && ok "and an env var beats profile.env" \
    || bad "MAINTAINER_VERIFY_PIDS=33 lost to the profile: $(tun_flag --pids-limit)"
# Every numeric key reaches its own flag. Values differ from the defaults and
# from each other, so a call site that still writes a literal cannot hide.
printf 'VERIFY_CPUS=3\nVERIFY_TIMEOUT=111\nPREFETCH_TIMEOUT=22\nVERIFY_TMPFS=64m\n' >> "$tvp/profile.env"
tun_verify
[ "$(tun_flag --cpus)" = "2x 3" ] && ok "profile VERIFY_CPUS reaches --cpus" || bad "--cpus ignored the profile: $(tun_flag --cpus)"
[ "$(tun_flag --timeout)" = "2x 111;1x 22" ] \
    && ok "profile VERIFY_TIMEOUT and PREFETCH_TIMEOUT reach their containers" \
    || bad "--timeout ignored the profile: $(tun_flag --timeout)"
[ "$(grep -c '^/tmp:rw,nosuid,size=64m$' "$TUN_ARGS")" = 3 ] && ok "profile VERIFY_TMPFS reaches --tmpfs" \
    || bad "--tmpfs ignored the profile"
tun_verify MAINTAINER_VERIFY_TIMEOUT=55 MAINTAINER_PREFETCH_TIMEOUT=44
[ "$(tun_flag --timeout)" = "1x 44;2x 55" ] && ok "and MAINTAINER_VERIFY_TIMEOUT, MAINTAINER_PREFETCH_TIMEOUT beat the profile" \
    || bad "the env timeouts lost to the profile: $(tun_flag --timeout)"
sed -i '/^VERIFY_\(CPUS\|TIMEOUT\|TMPFS\)=/d;/^PREFETCH_TIMEOUT=/d' "$tvp/profile.env"

# A suite declares what ITS build needs. The rust suite wants 6g and the docs
# suite wants nothing, and raising the default for both would tax every suite.
printf 'suite_memory() { printf 6g; }\n' >> "$tvp/verify.d/s.sh"
tun_verify
[ "$(tun_flag --memory)" = "3x 6g" ] && ok "a suite's suite_memory reaches the fetch, the clean run and the mutated run" \
    || bad "suite_memory is not what the containers got: $(tun_flag --memory)"
printf 'VERIFY_MEMORY=4g\n' >> "$tvp/profile.env"
tun_verify
[ "$(tun_flag --memory)" = "3x 6g" ] && ok "the suite is more specific than profile.env, so it wins" \
    || bad "profile.env VERIFY_MEMORY overrode the suite's own figure: $(tun_flag --memory)"
tun_verify MAINTAINER_VERIFY_MEMORY=3g
[ "$(tun_flag --memory)" = "3x 3g" ] && ok "and MAINTAINER_VERIFY_MEMORY beats the suite" \
    || bad "the env override lost to the suite: $(tun_flag --memory)"
# Called from inside a run, run.sh has exported the identity and not the
# tunables, so the gate reads the profile the run came from (PROFILE_DIR).
printf 'POST="off"\nVERIFY_PIDS=77\n' > "$tvp/profile.env"
tun_verify MAINTAINER_SLUG=o/r MAINTAINER_REPO="$tvrepo" MAINTAINER_ACCOUNT=testuser \
    MAINTAINER_STATE="$tvh/state" PROFILE_DIR="$tvp"
[ "$(tun_flag --pids-limit)" = "3x 77" ] && ok "a gate called from inside a run still reads the profile's tunables" \
    || bad "inside a run, the profile's VERIFY_PIDS=77 was lost: $(tun_flag --pids-limit)"
rm -f "$stub_dir/podman" "$stub_dir/gh"

# The python tools read the same file through _setting().
