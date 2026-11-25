#!/usr/bin/env bash
# Part of the offline suite: timers, cadence, rehearsal, status, the screen that decides what may execute, public repository files.
# tests/run-tests.sh sources this file in one shell, after its own preamble and after
# the parts before it, so every helper and variable it reads comes from there. It is
# sourced, never run on its own, and carries a shebang so the shell sweeps pick it up.

# The parts share one shell. These names are set by the driver or by an earlier part;
# declare only tells the linter they are read here, and assigns and changes nothing.
declare gate_settings
declare root
declare s
declare stub_dir
declare suite_root
declare wall_dir

echo "== --timers enables unit files that exist =="
# The bug: the loop globbed $root/systemd, a directory this repository has never
# had. With nullglob off it ran once on the literal pattern, and
# `systemctl enable '*.timer'` aborted the install having enabled nothing. A
# file-existence check cannot see this; only running the install can.
th="$stub_dir/timerhome"; mkdir -p "$th"
make_stub systemctl 'printf "%s\n" "$*" >> "$SYSTEMCTL_LOG"; exit 0'
make_stub loginctl 'echo yes'
SYSTEMCTL_LOG="$stub_dir/systemctl.log"; : > "$SYSTEMCTL_LOG"
PATH="$stub_dir:$PATH" HOME="$th" SYSTEMCTL_LOG="$SYSTEMCTL_LOG" "$root/install.sh" --timers >/dev/null 2>&1
enabled=$(grep -c 'enable --now maintainer@' "$SYSTEMCTL_LOG" 2>/dev/null || echo 0)
units_in_repo=$(find "$root/platform/linux" -name '*.timer' | wc -l)
if [ "$enabled" = "$units_in_repo" ] && [ "$enabled" -gt 0 ]; then
    ok "--timers enabled all $enabled timer units"
else
    bad "--timers enabled $enabled of $units_in_repo units"
fi
grep -q "enable --now \*\.timer" "$SYSTEMCTL_LOG" && bad "--timers passed an unexpanded glob to systemctl" \
    || ok "--timers never passes a literal glob"
[ -e "$th/.local/share/systemd/timers" ] && ok "a stamp is written before enabling (no catch-up storm)" \
    || bad "no stamp written; enabling fires every missed slot at once"

# A checkout may carry timer units for profiles it does not deploy. Only the
# deployed profile's units may be enabled; the others are reported and skipped.
timer_src="$stub_dir/timer-src"; timer_home="$stub_dir/timer-home"
mkdir -p "$timer_src" "$timer_home"
cp -r "$root/." "$timer_src/"
rm -rf "$timer_src/.git" "$timer_src/profiles/sysknife"
SYSTEMCTL_LOG="$stub_dir/systemctl-filter.log"; : > "$SYSTEMCTL_LOG"
PATH="$stub_dir:$PATH" HOME="$timer_home" SYSTEMCTL_LOG="$SYSTEMCTL_LOG" \
    "$timer_src/install.sh" --timers > "$stub_dir/timer-filter.out" 2>&1
if ! grep -q 'enable --now maintainer@sysknife-' "$SYSTEMCTL_LOG" \
    && [ "$(grep -c 'enable --now maintainer@magent-' "$SYSTEMCTL_LOG")" = 2 ] \
    && grep -q 'skipped maintainer@sysknife-' "$stub_dir/timer-filter.out"; then
    ok "--timers enables only deployed profiles and reports skipped profile units"
else
    bad "--timers enabled an undeployed profile or failed to report skipped units: $(cat "$SYSTEMCTL_LOG")"
fi

# A profile named after a hyphenated repository: the task is the text after the
# LAST hyphen, as in platform/linux/run-instance.sh.
hy_src="$stub_dir/hy-src"; hy_home="$stub_dir/hy-home"
mkdir -p "$hy_src" "$hy_home"
cp -r "$root/." "$hy_src/"
rm -rf "$hy_src/.git" "$hy_src/profiles/sysknife"
mv "$hy_src/profiles/magent" "$hy_src/profiles/my-repo"
cp "$hy_src/platform/linux/maintainer@magent-review.timer" "$hy_src/platform/linux/maintainer@my-repo-review.timer"
SYSTEMCTL_LOG="$stub_dir/systemctl-hy.log"; : > "$SYSTEMCTL_LOG"
PATH="$stub_dir:$PATH" HOME="$hy_home" SYSTEMCTL_LOG="$SYSTEMCTL_LOG" \
    "$hy_src/install.sh" --timers > "$stub_dir/timer-hy.out" 2>&1
if grep -q 'enable --now maintainer@my-repo-review.timer' "$SYSTEMCTL_LOG" \
    && ! grep -q 'enable --now maintainer@magent-' "$SYSTEMCTL_LOG"; then
    ok "--timers splits a hyphenated profile name at the last hyphen"
else
    bad "--timers mis-split a hyphenated profile name: $(cat "$SYSTEMCTL_LOG") $(cat "$stub_dir/timer-hy.out")"
fi
# A dry run has copied nothing to the share directory yet, so it must still
# report every unit as one it would enable.
dr_home="$stub_dir/dry-home"; mkdir -p "$dr_home"
dr_out="$(PATH="$stub_dir:$PATH" HOME="$dr_home" "$root/install.sh" --dry-run --timers 2>&1)"; dr_rc=$?
if [ "$dr_rc" = 0 ] && ! grep -q 'skipped maintainer@' <<<"$dr_out" && grep -q 'enable --now maintainer@magent-' <<<"$dr_out"; then
    ok "--dry-run --timers lists the units it would enable and skips none"
else
    bad "--dry-run --timers skipped units or failed (rc=$dr_rc): $(printf '%s' "$dr_out" | head -5 | tr '\n' ' ')"
fi

echo "== every command the prompt names is declared, and resolves =="
# A prompt told an unattended run to call `sysknife-maint screen <pr>` after that
# command had been renamed to `maintainer screen`. The run report carried a
# screen verdict for a command that could not have produced one. Nothing caught
# it: the evals check that a RULE is present, not that a tool exists.
#
# Scanned over the ASSEMBLED prompt, because the doctrine and the task prompt
# come from different files and only the assembly is what the agent reads. Every
# task of every profile: a stale name in the one prompt nobody assembled is the
# case this exists for, and a second profile's prompts are no less able to name
# a command that is gone.
for pe in "$root"/profiles/*/profile.env; do
  pf="$(basename "$(dirname "$pe")")"
  [ "$pf" = "_template" ] && continue
  rm -f "$stub_dir"/p-*.md
  # shellcheck disable=SC1090,SC1091
  ( . "$pe"
    for tk in $TASKS; do
        PATH="$stub_dir:$PATH" bash "$root/lib/run.sh" --show-prompt "$pf" "$tk" \
            >"$stub_dir/p-$tk.md" 2>/dev/null
    done )
  # shellcheck disable=SC1090,SC1091
  ( . "$pe"
    python3 - "$stub_dir" "$REQUIRED_COMMANDS" "$KNOWN_NAMES" <<'PYEOF'
import glob, os, re, sys
d, required, known = sys.argv[1], sys.argv[2].split(), sys.argv[3].split()
allowed = set(required) | set(known)
# Hyphenated lowercase words inside backticks, taken as the first token of the
# span. Precise enough that the current prompts yield a handful of names rather
# than ninety; calibrated by checking it flags a renamed command and nothing else.
span = re.compile(r"`([^`\n]+)`")
name = re.compile(r"^[a-z][a-z0-9]*(?:-[a-z0-9]+)+$")
seen, undeclared = set(), {}
for f in sorted(glob.glob(os.path.join(d, "p-*.md"))):
    for m in span.findall(open(f).read()):
        tok = m.split()[0] if m.split() else ""
        if not name.match(tok):
            continue
        seen.add(tok)
        if tok not in allowed:
            undeclared.setdefault(tok, os.path.basename(f))
for tok, f in undeclared.items():
    print(f"  {tok} (in {f}) is neither a REQUIRED_COMMAND nor a KNOWN_NAME")
# A declaration nobody uses is padding, and padding is how this check would go
# quiet: add every plausible name and it can never fail again.
unused = [k for k in known if k not in seen]
for k in unused:
    print(f"  KNOWN_NAMES lists '{k}', which no assembled prompt uses")
sys.exit(1 if undeclared or unused else 0)
PYEOF
  ) && ok "$pf: every hyphenated name is declared, and none is padding" \
    || bad "$pf: an undeclared or unused name in its prompts (see above)"
done

echo "== the cadence gate holds a task that ran too recently =="
# launchd cannot express "every N days" and cron's day-of-month stepping fires
# on the 31st and again on the 1st. Before this gate, the macOS installer said
# the since-last-run state enforced the cadence; nothing did, so `audit` would
# have run five times a week.
cad="$stub_dir/cadence"; mkdir -p "$cad/state"
age_last() { touch -d "@$(( $(date +%s) - $1 ))" "$cad/state/last-review.json"; }
echo '{"run_id":"x"}' > "$cad/state/last-review.json"
cad_run() { MAINTAINER_STATE_DIR="$cad" PATH="$stub_dir:$PATH" \
            env "$@" bash "$root/lib/run.sh" sysknife review 2>&1; }
# MIN_HOURS_review is 6.
age_last 7200
out=$(cad_run X=1)
grep -q '^skipped' <<<"$out" && ok "a task inside its minimum interval is skipped" \
    || bad "the gate did not hold a 2h-old task (got: $(printf '%s' "$out" | head -1))"
grep -q 'MAINTAINER_FORCE' <<<"$out" && ok "the skip names the override" || bad "the skip does not say how to override"
age_last 32400
out=$(cad_run X=1)
grep -q '^skipped' <<<"$out" && bad "a task past its interval was skipped anyway" \
    || ok "a task past its minimum interval proceeds"
age_last 60
out=$(cad_run MAINTAINER_FORCE=1)
grep -q '^skipped' <<<"$out" && bad "MAINTAINER_FORCE did not override the gate" \
    || ok "MAINTAINER_FORCE overrides the gate"
# The gate must read the profile's number, not a constant compiled into run.sh.
if sc_case 1; then
grep -q 'MIN_HOURS_\$task' "$root/lib/run.sh" && ok "the interval comes from the profile" \
    || bad "run.sh no longer reads MIN_HOURS_<task> from the profile"
sc_end; fi

echo "== rehearsal: POST=off does the work and reaches nobody =="
reh="$wall_dir/settings-rehearsal.json"
[ -f "$reh" ] && ok "a rehearsal wall is generated beside the live one" || bad "no rehearsal wall generated"
python3 - "$s" "$reh" <<'PYEOF' && ok "the rehearsal wall is a strict superset of the live wall" || bad "the rehearsal wall is not a superset"
import json, sys
live = set(json.load(open(sys.argv[1]))["permissions"]["deny"])
reh = set(json.load(open(sys.argv[2]))["permissions"]["deny"])
sys.exit(0 if live < reh else 1)
PYEOF
# One assertion per verb. An alternation would let a lost rule hide behind one
# that still matched, which is how a guard goes vacuous.
for verb in "gh pr review" "gh pr comment" "gh issue comment" "gh issue create" "gh label" "gh api -X POST"; do
    grep -qF "Bash($verb:*)" "$reh" && ok "rehearsal blocks '$verb'" || bad "rehearsal does NOT block '$verb'"
done
grep -qF "Bash(/usr/bin/gh issue comment:*)" "$reh" && ok "rehearsal blocks the absolute spelling too" \
    || bad "rehearsal misses /usr/bin spellings"
# The prompt must say so as well. A wall alone produces a confused run that
# keeps trying; the sentence is what makes it stop.
banner=$(PATH="$stub_dir:$PATH" MAINTAINER_POST=off bash "$root/lib/run.sh" --show-prompt sysknife review 2>/dev/null)
grep -q 'REHEARSAL' <<<"$banner" && ok "POST=off puts the rehearsal notice in the prompt" \
    || bad "POST=off does not tell the agent it is rehearsing"
grep -q 'list what you WOULD have posted' <<<"$banner" && ok "the rehearsal asks for the drafts it withheld" \
    || bad "a rehearsal that withholds without recording is not reviewable"
default=$(PATH="$stub_dir:$PATH" bash "$root/lib/run.sh" --show-prompt sysknife review 2>/dev/null)
grep -q 'REHEARSAL' <<<"$default" && bad "the rehearsal notice appears when POST is on" \
    || ok "POST=on carries no rehearsal notice"
if sc_case 3; then
grep -q 'backend_rehearsal' "$root/lib/backends/claude.sh" && ok "claude declares it can enforce POST=off" \
    || bad "claude no longer declares rehearsal support"
grep -q 'backend_rehearsal' "$root/lib/backends/codex.sh" && bad "codex claims rehearsal support it cannot enforce" \
    || ok "codex does not claim rehearsal support (it has no deny list)"
grep -q 'backend_rehearsal' "$root/lib/run.sh" && ok "run.sh refuses a rehearsal on a backend that cannot enforce it" \
    || bad "run.sh would hand POST=off to a backend with no per-command control"
sc_end; fi
# A new profile must start silent.
grep -q 'MAINTAINER_POST:-off' "$root/profiles/_template/profile.env" && ok "a scaffolded profile starts at POST=off" \
    || bad "a new profile would post on its first run"

echo "== the doctrine is not optional =="
# lib/preamble-core.md carries the injection rule, the screen rule and the
# evidence rule. A run assembled without it is an unattended agent with none of
# them, so its absence must stop the run rather than shorten the prompt.
lay2="$stub_dir/nodoctrine"; mkdir -p "$lay2/profiles/sysknife/prompts"
cp "$root/lib/run.sh" "$lay2/run.sh"; cp "$root/lib/profile.sh" "$lay2/profile.sh"; cp "$root/lib/defaults.env" "$lay2/defaults.env"
cp -r "$root/lib/backends" "$lay2/backends"
cp "$root/profiles/sysknife/profile.env" "$lay2/profiles/sysknife/"
cp "$root/profiles/sysknife/prompts/"*.md "$lay2/profiles/sysknife/prompts/"
cp "$root/lib/prose-style.md" "$lay2/prose-style.md"
out=$(PATH="$stub_dir:$PATH" HOME="$stub_dir" bash "$lay2/run.sh" --show-prompt sysknife review 2>&1); rc=$?
if [ "$rc" != 0 ] && grep -q 'preamble-core' <<<"$out"; then
    ok "a tree with no core preamble refuses to run"
else
    bad "a run assembled a prompt with no doctrine in it (rc=$rc)"
fi
cp "$root/lib/preamble-core.md" "$lay2/preamble-core.md"
PATH="$stub_dir:$PATH" HOME="$stub_dir" bash "$lay2/run.sh" --show-prompt sysknife review >/dev/null 2>&1 \
    && ok "and runs once the core preamble is restored" || bad "the core preamble is not found in the installed layout"

echo "== --show-prompt shows, and changes nothing =="
lock_before=$(ls "$HOME/.local/state/sysknife-maint/state" 2>/dev/null | wc -l)
PATH="$stub_dir:$PATH" bash "$root/lib/run.sh" --show-prompt sysknife issues >/dev/null 2>&1
lock_after=$(ls "$HOME/.local/state/sysknife-maint/state" 2>/dev/null | wc -l)
check "--show-prompt writes no state" "$lock_after" "$lock_before"
PATH="$stub_dir:$PATH" bash "$root/lib/run.sh" --show-prompt sysknife issues 2>/dev/null | grep -q 'preview only' \
    && ok "--show-prompt marks its context as a preview" || bad "--show-prompt passes off a fake context as real"
# A preview must leave no trace at all. It opened a log file per call before this
# was asserted, and 22 empty logs from one test run landed in the live trail.
fresh="$stub_dir/fresh-state"; rm -rf "$fresh"; mkdir -p "$fresh"
MAINTAINER_STATE_DIR="$fresh" PATH="$stub_dir:$PATH" \
    bash "$root/lib/run.sh" --show-prompt sysknife review >/dev/null 2>&1
left=$(find "$fresh" -type f 2>/dev/null | wc -l)
check "--show-prompt writes no log into the audit trail" "$left" "0"

echo "== uninstall removes the tool and keeps the audit trail =="
if bash "$suite_root/tests/install-uninstall-artifacts.sh" "$root"; then
    ok "uninstall removes installed artifacts without a systemd user session"
else
    bad "uninstall leaves installed artifacts when systemctl is unavailable"
fi
uh="$stub_dir/unin"; mkdir -p "$uh"
HOME="$uh" "$root/install.sh" >/dev/null 2>&1
mkdir -p "$uh/.local/state/sysknife-maint/runs"; echo "a run" > "$uh/.local/state/sysknife-maint/runs/keep.md"
PATH="$stub_dir:$PATH" HOME="$uh" "$root/install.sh" --uninstall >/dev/null 2>&1
[ -d "$uh/.local/share/maintainer" ] && bad "uninstall left the deployed tree" || ok "uninstall removes the deployed tree"
[ -e "$uh/.local/bin/maintainer" ] && bad "uninstall left commands on PATH" || ok "uninstall removes the commands"
[ -f "$uh/.local/state/sysknife-maint/runs/keep.md" ] \
    && ok "uninstall keeps the audit trail (it is the record of what was posted in your name)" \
    || bad "uninstall deleted the audit trail"

echo "== new-profile.sh makes CONTRIBUTING's promise true =="
np="$stub_dir/np"; mkdir -p "$np"; cp -r "$root"/* "$np/" 2>/dev/null
rm -rf "$np/.git"
( cd "$np" && ./new-profile.sh demo acme/widget /tmp/widget acmebot "Ada" ) >/dev/null 2>&1
# A checkout under $HOME must be written as $HOME/..., because profile.env gets
# committed and the adopter's username would go with it. This repository's own
# leak check caught it the first time a profile was scaffolded here.
( cd "$np" && ./new-profile.sh underhome acme/uh "$HOME/src/uh" acmebot "Ada" ) >/dev/null 2>&1
if grep -q 'REPO_PATH=.*\$HOME/src/uh' "$np/profiles/underhome/profile.env"; then
    ok "a checkout under \$HOME is written relative, not expanded"
else
    bad "new-profile wrote a literal home path into a file meant to be committed"
    grep -n 'REPO_PATH' "$np/profiles/underhome/profile.env" | head -1
fi
[ -f "$np/profiles/demo/profile.env" ] && ok "new-profile scaffolds a profile" || bad "new-profile wrote no profile"
grep -rq '__[A-Z_]*__' "$np/profiles/demo" && bad "a placeholder survived scaffolding" || ok "every placeholder is substituted"
grep -q 'REPO_SLUG="acme/widget"' "$np/profiles/demo/profile.env" && ok "the slug lands in profile.env" || bad "the slug did not substitute"
tasks=$(sed -n 's/^TASKS="\(.*\)"/\1/p' "$np/profiles/demo/profile.env" | wc -w)
made=$(find "$np/platform/linux" -name 'maintainer@demo-*.timer' | wc -l)
check "one timer per task" "$made" "$tasks"
( cd "$np" && ./new-profile.sh demo acme/widget /tmp/widget acmebot ) >/dev/null 2>&1 \
    && bad "new-profile overwrote an existing profile" || ok "new-profile refuses to overwrite"
# The closing instructions must name commands that exist. They told the adopter
# to call run.sh by its deployed path after `maintainer run` had replaced it.
nextsteps=$( cd "$np" && ./new-profile.sh steps acme/s /tmp/s acmebot "A" 2>&1 )
grep -q 'maintainer run' <<<"$nextsteps" && ok "the next steps name the command that exists" \
    || bad "new-profile still points at a path instead of a command"
grep -q 'MAINTAINER_PROFILE' <<<"$nextsteps" && ok "the next steps say to export the profile" \
    || bad "the next steps would have the adopter querying the wrong profile"
# The scaffolded profile must produce a real prompt, not a template with holes.
( cd "$np" && PATH="$stub_dir:$PATH" bash lib/run.sh --show-prompt demo review 2>/dev/null ) | grep -q 'acme/widget' \
    && ok "a scaffolded profile assembles a prompt naming its own repository" \
    || bad "a scaffolded profile cannot assemble a prompt"
# Two carve-outs an adopter had to discover by hitting them. Both were written
# for profiles/magent first and stayed there, so the next adopter would have hit
# them again: the screen rule reading as a ban on running your own gates, and a
# first run having no previous baseline to diff against.
scaffolded=$( cd "$np" && PATH="$stub_dir:$PATH" bash lib/run.sh --show-prompt demo review 2>/dev/null )
grep -q 'about code arriving' <<<"$scaffolded" \
    && ok "the template says whose code the agent may run" \
    || bad "the template leaves the screen rule reading as a ban on its own gates"
grep -q 'YOU HAVE NOT REVIEWED' <<<"$scaffolded" \
    && ok "the template points at the unreviewed range, not the fetch delta" \
    || bad "the template still points the agent at the fetch delta"
grep -q 'first run there is no previous baseline' <<<"$scaffolded" \
    && ok "the template handles a first run with no baseline" \
    || bad "a scaffolded first run has no instruction for an empty baseline"
# And no code file may carry a repository's name for it to work.
if sc_case 1; then
if grep -q 'MAINTAINER_TASKS' "$root/bin/maintainer" && grep -q 'MAINTAINER_TASKS' "$root/lib/run.sh"; then
    ok "the task list comes from the profile, not from bin/maintainer"
else
    bad "bin/maintainer still hardcodes the task list"
fi
sc_end; fi

echo "== status answers the question a maintainer actually has =="
# Built here rather than read off this machine. These cases used to run against
# whatever profile happened to be deployed and whatever timers happened to be
# registered, so they passed locally and failed the first time CI ran them: on a
# runner there is no deployment and no systemd, and status correctly said so
# while the assertions expected my laptop's answer.
sh_home="$stub_dir/statushome"
mkdir -p "$sh_home/.local/share/maintainer/profiles/sk" \
         "$sh_home/.local/state/sk-maint/state" "$sh_home/.local/state/sk-maint/runs"
cat > "$sh_home/.local/share/maintainer/profiles/sk/profile.env" <<'ENVSK'
PROFILE_NAME="sk"
REPO_SLUG="acme/sk"
REPO_PATH="$HOME/src/sk"
STATE_DIR="$HOME/.local/state/sk-maint"
TASKS="review issues"
MIN_HOURS_review=6
POST="on"
GH_ACCOUNT="acmebot"
ENVSK
printf '{"run_id":"2026-01-01T00-00-review","main_sha":"abc"}\n' \
    > "$sh_home/.local/state/sk-maint/state/last-review.json"
printf '# report\nsecond line\n' > "$sh_home/.local/state/sk-maint/runs/2026-01-01T00-00-review.md"
# A timer that fires in an hour, in the shape systemctl --output=json emits:
# `next` is an absolute timestamp in microseconds, which is what tripped the
# duration column into printing "in 20700 days" the first time.
make_stub systemctl 'case "$*" in
  *list-timers*json*) python3 -c "import json,time;print(json.dumps([{\"unit\":\"maintainer@sk-review.timer\",\"next\":int((time.time()+3600)*1e6)}]))";;
  *) exit 0;;
esac'
out=$(PATH="$stub_dir:$PATH" HOME="$sh_home" MAINTAINER_PROFILE=sk \
      env -u MAINTAINER_STATE -u MAINTAINER_REPO -u MAINTAINER_SLUG -u MAINTAINER_TASKS \
          -u MAINTAINER_STATE_DIR python3 "$root/bin/maintainer" status 2>&1)
for want in "acme/sk" "posting" "review" "issues"; do
    grep -q "$want" <<<"$out" && ok "status reports $want" || bad "status does not report $want"
done
grep -qE 'in [0-9]+' <<<"$out" && ok "status converts the next-run time to a duration" \
    || bad "status does not show when the next run is"
grep -q '2 lines' <<<"$out" && ok "status reports how long the last report was" \
    || bad "status does not size the last report"
grep -qi 'never' <<<"$out" && ok "status marks a task that has never run" \
    || bad "status does not distinguish a task that never ran"
grep -q 'def cmd_run' "$root/bin/maintainer" && ok "maintainer run wraps the deployed orchestrator" \
    || bad "no maintainer run subcommand"
grep -q 'os.execv' "$root/bin/maintainer" && ok "maintainer run execs run.sh rather than reimplementing it" \
    || bad "maintainer run does not delegate to run.sh"
out=$(python3 "$root/bin/maintainer" run 2>&1); rc=$?
[ "$rc" != 0 ] && ok "maintainer run with no task is refused" || bad "maintainer run accepted an empty task"

echo "== a second profile reads its own settings, not the first one's =="
# Found by scaffolding a profile for this repository and running status against
# it. It printed the new profile's name and POST setting above the FIRST
# profile's repository path and run history, because those came from module
# defaults. An adopter would have read another project's runs as their own.
two="$stub_dir/twoprofiles"; mkdir -p "$two/.local/share/maintainer/profiles/other"
cat > "$two/.local/share/maintainer/profiles/other/profile.env" <<'ENV'
PROFILE_NAME="other"
REPO_PATH="$HOME/src/otherrepo"
REPO_SLUG="someone/otherrepo"
STATE_DIR="$HOME/.local/state/other-maint"
TASKS="triage"
MIN_HOURS_triage=12
POST="off"
ENV
mkdir -p "$two/.local/state/other-maint/runs" "$two/.local/state/other-maint/state"
out=$(env -u MAINTAINER_STATE -u MAINTAINER_REPO -u MAINTAINER_SLUG -u MAINTAINER_TASKS \
      -u MAINTAINER_STATE_DIR HOME="$two" MAINTAINER_PROFILE=other \
      python3 "$root/bin/maintainer" status 2>&1)
grep -q 'someone/otherrepo' <<<"$out" && ok "status reports the second profile's slug" \
    || bad "status did not read the second profile's slug"
grep -q 'src/otherrepo' <<<"$out" && ok "status reports the second profile's repository" \
    || bad "status showed the wrong repository for a second profile"
grep -q 'triage' <<<"$out" && ok "status reports the second profile's task list" \
    || bad "status showed the wrong task list for a second profile"
grep -qi 'lacs\|sysknife' <<<"$out" && bad "the first profile's data leaked into the second profile's status" \
    || ok "no first-profile data leaks into a second profile"
grep -qi 'OFF, rehearsal' <<<"$out" && ok "status reads POST from the deployed profile" \
    || bad "status did not read POST"

echo "== the screen decides what may execute on this host =="
# Containment layer 3, and it had no test at all until now. `cargo test` on a
# fork PR runs a stranger's code as this user and `build.rs` runs it at compile
# time, so a screen that errs toward SAFE launders an unknown into a
# reassurance. Every case here is driven through the real cmd_screen.
# $4 = a head SHA this branch force-pushed away, or empty. $5 = that commit's
# files json. The three calls after 'pr diff' are the force-push history check:
# without answers for them every screen case would exercise only its
# could-not-ask branch, and the all-clear it prints would be untested.
screen_stub() {  # $1 = files json, $2 = changedFiles count, $3 = diff text, $4 = dropped sha, $5 = its files
    local dropped="${4:-}" dropped_files="${5:-[]}"
    local runs='{"workflow_runs":[{"head_sha":"abcdef1234567890","status":"completed"}]}'
    [ -n "$dropped" ] && runs="{\"workflow_runs\":[{\"head_sha\":\"abcdef1234567890\",\"status\":\"completed\"},{\"head_sha\":\"$dropped\",\"status\":\"action_required\"}]}"
    make_stub gh "case \"\$*\" in
      *'--json headRefName'*) printf '%s\n' '{\"headRefName\":\"feature\"}' ;;
      *'pr view'*) cat <<'J'
{\"files\":$1,\"headRefOid\":\"abcdef1234567890\",\"author\":{\"login\":\"stranger\"},\"title\":\"t\",\"changedFiles\":$2}
J
        ;;
      *'pr diff'*'--patch'*) echo 'screen asked for --patch' >&2; exit 9 ;;
      *'pr diff'*) printf '%s\n' '$3' ;;
      *actions/runs*) printf '%s\n' '$runs' ;;
      *pulls/*/commits*) printf '%s\n' '[{\"sha\":\"abcdef1234567890\"}]' ;;
      *commits/$dropped) printf '%s\n' '{\"files\":$dropped_files}' ;;
      *commits/*) printf '%s\n' '{\"files\":[]}' ;;
      *) exit 1 ;;
    esac"
}
# MAINTAINER_PROFILE is named rather than left to a default: `maintainer` used
# to fall back to "sysknife" when nothing said otherwise, and now enumerates the
# deployed profiles and refuses. That refusal has its own test; these cases are
# about how the screen classifies files, so they pin the profile.
screen_run() { PATH="$stub_dir:$PATH" MAINTAINER_PROFILE=sysknife MAINTAINER_REPO="$stub_dir" python3 "$root/bin/maintainer" screen 1 2>&1; }
screen_case() {  # $1 label, $2 expected substring
    # `--` before the pattern: an expected substring that begins with a dash is
    # read by grep as an option, and the case then fails with a usage message
    # rather than a verdict about the screen.
    out="$(screen_run)"
    if grep -qF -- "$2" <<<"$out"; then ok "$1"
    else bad "$1 (got: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-100))"; fi
}

screen_stub '[{"path":"README.md"},{"path":"docs/a.md"}]' 2 ''
screen_case "docs-only change is INERT" "VERDICT: INERT"

screen_stub '[{"path":"crates/x/src/lib.rs"}]' 1 ''
screen_case "a .rs file is DO NOT EXECUTE (cargo test runs it)" "VERDICT: DO NOT EXECUTE"

screen_stub '[{"path":"crates/x/build.rs"}]' 1 ''
screen_case "build.rs is DO NOT EXECUTE (it runs at compile time)" "runs at compile time"

screen_stub '[{"path":".github/workflows/ci.yml"}]' 1 ''
screen_case "a workflow change warns against approving its run" "Do NOT approve its queued workflow"

screen_stub '[{"path":"scripts/x.sh"}]' 1 ''
screen_case "a shell script is DO NOT EXECUTE (the gates run it)" "VERDICT: DO NOT EXECUTE"

# A clean branch says so, rather than saying nothing. Silence and an all-clear
# are the same on screen, and only one of them is true.
screen_stub '[{"path":"scripts/x.sh"}]' 1 ''
screen_case "a branch with no workflow history says approving its run is only what the diff says" \
    "none in this branch's force-push"

# The head is not the whole question. sysknife#527 on 2026-09-29 arrived as one
# commit touching Cargo.lock, and its branch had been force-pushed over
# 441f163c, which edited .github/workflows/ci.yml. Screening the head alone
# reported nothing to be careful about, and the author's stated intent was to
# restore that commit. Approving a run on the strength of that screen, and then
# approving the next one by habit, is how a fork's workflow gets to run with
# this repository's permissions.
screen_stub '[{"path":"Cargo.lock"}]' 1 '' '441f163cf376346e' '[{"filename":".github/workflows/ci.yml"}]'
screen_case "a workflow edit force-pushed away is still reported" \
    "force-pushed over a commit that edited a workflow"
screen_case "and the dropped head is named" "441f163c dropped"

# The same history, with nothing dangerous in it, must not cry wolf.
screen_stub '[{"path":"Cargo.lock"}]' 1 '' '441f163cf376346e' '[{"filename":"README.md"}]'
screen_case "a force-push over an ordinary commit is not reported as a workflow risk" \
    "none in this branch's force-push"

# INERT pull requests get the judgement too. A docs-only change is inert and its
# queued run is exactly as dangerous as anyone else's if the branch ever carried
# a workflow edit; the warning used to print only under DO NOT EXECUTE.
screen_stub '[{"path":"README.md"}]' 1 '' '441f163cf376346e' '[{"filename":".github/workflows/ci.yml"}]'
screen_case "an INERT pull request is still screened for workflow exposure" \
    "force-pushed over a commit that edited a workflow"

# What DO NOT EXECUTE actually forbids, and what it does not.
#
# The verdict used to end with "In an unattended run there is no fallback: do
# not build, do not test ... Interactively, you can execute it in podman". That
# sentence was measured on 2026-09-02, when every hardening directive on the
# systemd unit blocked rootless podman with `newuidmap: write to uid_map failed`
# and only an unhardened unit could start a container.
#
# It is no longer true. A `podman-userns-warmup.service` the unit orders itself
# after establishes the user namespace, and the hardened unit reuses it. The
# 2026-09-07 11:27 `ci` run, a real timer run under NoNewPrivileges,
# RestrictSUIDSGID, LockPersonality and RestrictRealtime, started
# postgres:17-alpine and read PostgreSQL 17.10 out of it.
#
# Leaving the stale sentence in place cost more than a wrong fact: the reviews
# were already running mutation proofs in podman on PRs screened DO NOT EXECUTE,
# so the doctrine contradicted the practice, and a doctrine nobody follows
# teaches an agent that doctrine is optional. The invariant is unchanged and is
# now stated as the invariant: never on this host.
screen_stub '[{"path":"crates/x/src/lib.rs"}]' 1 ''
screen_case "the verdict forbids the host, which is what it has always meant" "Never on this host"
screen_case "and it says a container is available to an unattended run" "unattended"
screen_case "and names the probe that answers whether it is" "--network=none docker.io/library/busybox"
screen_case "and keeps the read-only, no-network shape for the proof itself" "/repo:ro"
out="$(screen_run)"
grep -q 'Interactively, you can execute' <<<"$out" \
    && bad "the screen still says a container is available only to a human" \
    || ok "the screen no longer calls the container interactive-only"
grep -q 'there is no fallback' <<<"$out" \
    && bad "the screen still claims an unattended run has no sandbox" \
    || ok "and no longer claims an unattended run has no sandbox"
# The one thing that is NOT about local execution and must survive untouched.
screen_stub '[{"path":".github/workflows/ci.yml"}]' 1 ''
screen_case "a workflow change still forbids approving its queued run" "Do NOT approve its queued workflow"


# A dependency line moved inside Cargo.toml: new third-party code would be built.
screen_stub '[{"path":"Cargo.toml"}]' 1 'diff --git a/Cargo.toml b/Cargo.toml
+[dependencies]
+evil = "1.0"'
screen_case "a dependency change is DO NOT EXECUTE" "dependency lines changed"

# A package.json edit is DO NOT EXECUTE: npm install runs its lifecycle scripts.
# The magent review of 2026-09-04 found the screen calling a hostile
# package.json INERT because it ends in .json.
screen_stub '[{"path":"packages/setup/package.json"}]' 1 ''
screen_case "a package.json is DO NOT EXECUTE (npm runs its lifecycle scripts)" "VERDICT: DO NOT EXECUTE"

# A package-lock.json too: a crafted lock redirects dependency resolution.
screen_stub '[{"path":"packages/setup/package-lock.json"}]' 1 ''
screen_case "a package-lock.json is DO NOT EXECUTE (it steers what npm fetches)" "VERDICT: DO NOT EXECUTE"

# But a plain .json the gates only read as data stays inert, so the fix did not
# turn every config file into a block.
screen_stub '[{"path":"tests/fixtures/sample.json"}]' 1 ''
screen_case "an ordinary .json fixture is still INERT" "VERDICT: INERT"

# gh returns fewer files than it says exist: the rest were never screened.
screen_stub '[{"path":"README.md"}]' 300 ''
screen_case "a truncated file list fails closed" "never screened"

# An extension the screen does not classify must not fall through to INERT.
screen_stub '[{"path":"assets/blob.bin"}]' 1 ''
screen_case "an unclassifiable file fails closed" "failing closed"

# And if gh cannot answer at all, the screen must refuse rather than guess.
make_stub gh 'exit 1'
out="$(screen_run)"
if grep -q 'READ-ONLY' <<<"$out"; then ok "an unreachable gh fails closed"
else bad "the screen produced a verdict with no data (got: $(printf '%s' "$out" | head -1))"; fi

echo "== a run cannot start inside another run, and the override does not travel =="
# The agent inherits run.sh's environment. MAINTAINER_FORCE=1 was in it, so
# every run.sh the agent invoked skipped its cadence gate: a repro that should
# have printed "skipped" started a real pass against another repository's
# checkout instead. Nothing forbade the nesting either, so a rehearsal could
# have launched a full run of a posting profile.
out=$(MAINTAINER_IN_RUN="other/review" PATH="$stub_dir:$PATH" \
      bash "$root/lib/run.sh" sysknife review 2>&1); rc=$?
if [ "$rc" = 78 ] && grep -q "already inside the run 'other/review'" <<<"$out"; then
    ok "a nested run is refused, naming the run it is inside"
else
    bad "a nested run was not refused (rc=$rc)"
fi
# A preview from inside a run stays allowed: reading the prompt harms nothing.
MAINTAINER_IN_RUN="other/review" PATH="$stub_dir:$PATH" \
    bash "$root/lib/run.sh" --show-prompt sysknife review >/dev/null 2>&1 \
    && ok "a preview from inside a run is still allowed" \
    || bad "the nested guard also blocks --show-prompt"

# What the backend actually receives, measured by running one. The backend is a file
# the case writes next to a copy of run.sh, so this block depends on the shell program's
# way of finding backends. A tree whose backends are compiled in knows four names (decision
# D18) and refuses this one, so the block is source-coupled. The block after it makes the
# same measurement through a shipped backend, and holds for any tree.
if sc_case 4; then
envh="$stub_dir/envrun"; mkdir -p "$envh/backends" "$envh/profiles/envp/prompts" "$envh/bin"
cp "$root/lib/run.sh" "$envh/run.sh"; cp "$root/lib/profile.sh" "$envh/profile.sh"; cp "$root/lib/defaults.env" "$envh/defaults.env"
cp "$root/lib/preamble-core.md" "$root/lib/prose-style.md" "$envh/"
cat > "$envh/backends/envdump.sh" <<'BACKEND'
backend_name() { printf 'envdump'; }
backend_check() { return 0; }
backend_run() { env | sort > "$ENVDUMP_OUT"; return 0; }
BACKEND
cat > "$envh/profiles/envp/profile.env" <<'ENVP'
PROFILE_NAME="envp"
REPO_PATH="$HOME/repo"
REPO_SLUG="o/r"
MAINTAINER_NAME="Tester"
GH_ACCOUNT="testuser"
STATE_DIR="${MAINTAINER_STATE_DIR:-$HOME/state}"
HELPER="$HOME/bin/maintainer"
TASKS="review"
MODEL_review="m"
MIN_HOURS_review=6
POST="on"
PROSE_STYLE="raw"
BACKEND="envdump"
LOCK_WAIT=5
REQUIRED_COMMANDS="gh"
KNOWN_NAMES=""
ENVP
printf '# task\n' > "$envh/profiles/envp/prompts/review.md"
printf '# site\n' > "$envh/profiles/envp/prompts/common-preamble.md"
# A helper that behaves like `maintainer start` and `finish` without a repo.
cat > "$envh/bin/maintainer" <<'HELPER'
#!/usr/bin/env bash
case "$1" in
  start)  echo "=== maintainer testrun-review ==="; echo "context";;
  finish) exit 0;;
esac
HELPER
chmod +x "$envh/bin/maintainer"
mkdir -p "$envh/state/state" "$envh/state/runs" "$envh/repo"
# run.sh now provisions a missing checkout; this test's repo is a plain dir, so
# make it a real checkout to skip provisioning (there is no origin to clone).
git init -q "$envh/repo"
make_stub gh "case \"\$*\" in *'auth token'*) echo ghp_OracleStubTokenAAAAAAAAAAAAAAAAAAAAA;; *'api user'*) echo testuser;; esac; exit 0"
printf '# r\n' > "$envh/state/runs/testrun-review.md"
ENVDUMP_OUT="$stub_dir/backend.env" PATH="$stub_dir:$envh/bin:$PATH" HOME="$envh" \
    MAINTAINER_FORCE=1 MAINTAINER_STATE_DIR="$envh/state" \
    bash "$envh/run.sh" envp review >/dev/null 2>&1
if [ -f "$stub_dir/backend.env" ]; then
    ok "the backend ran, so its environment can be measured"
    grep -q '^MAINTAINER_FORCE=' "$stub_dir/backend.env" \
        && bad "MAINTAINER_FORCE reached the backend; nested runs skip their cadence gate" \
        || ok "MAINTAINER_FORCE does not reach the backend"
    grep -q '^MAINTAINER_IN_RUN=envp/review$' "$stub_dir/backend.env" \
        && ok "the backend is told which run it is inside" \
        || bad "MAINTAINER_IN_RUN is not set for the backend"
    grep -q '^MAINTAINER_POST=' "$stub_dir/backend.env" \
        && ok "MAINTAINER_POST does reach the tools (prune has to honour it)" \
        || bad "MAINTAINER_POST is not exported"
else
    bad "the env-dump backend never ran, so this block measured nothing"
    bad "so MAINTAINER_FORCE was not measured"
    bad "so MAINTAINER_IN_RUN was not measured"
    bad "so MAINTAINER_POST was not measured"
fi
sc_end; fi

# The same three facts through the shipped claude backend, which every tree has: a claude
# stub that writes the environment it received where the case can read it.
envc="$stub_dir/envclaude"; rm -rf "$envc"; mkdir -p "$envc/.local/bin" "$envc/state/state" "$envc/state/runs"
envc_repo="$envc/repo"; git init -q -b main "$envc_repo"
cat > "$envc/.local/bin/maintainer" <<'HELPER'
#!/usr/bin/env bash
case "$1" in
  start)  echo "=== maintainer testrun-review ==="; echo "context";;
  finish) exit 0;;
esac
HELPER
chmod +x "$envc/.local/bin/maintainer"
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'auth token'*) echo ghp_OracleStubTokenAAAAAAAAAAAAAAAAAAAAA;; *'api user'*) echo vladimirrott;; esac"
make_stub claude "env | sort > $envc/claude.env; exit 0"
PATH="$stub_dir:$PATH" HOME="$envc" MAINTAINER_FORCE=1 MAINTAINER_SETTINGS="$gate_settings" \
    MAINTAINER_STATE_DIR="$envc/state" MAINTAINER_REPO_PATH="$envc_repo" \
    MAINTAINER_GH_TRIES=1 MAINTAINER_GH_BACKOFF=0 \
    bash "$root/lib/run.sh" sysknife review >/dev/null 2>&1
if [ -f "$envc/claude.env" ]; then
    ok "the agent ran, so its environment can be measured"
    grep -q '^MAINTAINER_FORCE=' "$envc/claude.env" \
        && bad "MAINTAINER_FORCE reached the agent; nested runs skip their cadence gate" \
        || ok "MAINTAINER_FORCE does not reach the agent"
    grep -q '^MAINTAINER_IN_RUN=sysknife/review$' "$envc/claude.env" \
        && ok "the agent is told which run it is inside" \
        || bad "MAINTAINER_IN_RUN is not set for the agent"
    grep -q '^MAINTAINER_POST=' "$envc/claude.env" \
        && ok "MAINTAINER_POST does reach the agent's tools (prune has to honour it)" \
        || bad "MAINTAINER_POST is not exported to the agent"
else
    bad "the claude stub never ran, so this block measured nothing"
    bad "so MAINTAINER_FORCE was not measured"
    bad "so MAINTAINER_IN_RUN was not measured"
    bad "so MAINTAINER_POST was not measured"
fi
make_stub claude 'echo "claude stub invoked" >&2; exit 0'

echo "== three guards that shipped without a test =="
# All three were found by a run that mutated them and watched every gate stay
# green. A guard nothing tests is a guard that will be deleted by someone tidying
# up, and nobody will know.

# 1. prune must not delete during a rehearsal.
pr="$stub_dir/prunerepo"; rm -rf "$pr"; mkdir -p "$pr"
git -C "$pr" init -q -b main; git -C "$pr" config user.email t@t; git -C "$pr" config user.name t
echo a > "$pr/a"; git -C "$pr" add -A; git -C "$pr" commit -qm base
git -C "$pr" branch merged-one; git -C "$pr" remote add origin "$pr"; git -C "$pr" fetch -q origin 2>/dev/null
make_stub gh 'exit 0'
MAINTAINER_POST=off MAINTAINER_REPO="$pr" MAINTAINER_SLUG="o/r" PATH="$stub_dir:$PATH" \
    bash "$root/bin/maintainer-repo" prune >/dev/null 2>&1
if git -C "$pr" show-ref --verify -q refs/heads/merged-one; then
    ok "prune deletes nothing while POST=off"
else
    bad "a rehearsal deleted a branch"
fi
MAINTAINER_POST=on MAINTAINER_REPO="$pr" MAINTAINER_SLUG="o/r" PATH="$stub_dir:$PATH" \
    bash "$root/bin/maintainer-repo" prune >/dev/null 2>&1
if git -C "$pr" show-ref --verify -q refs/heads/merged-one; then
    bad "prune deleted nothing with POST=on either; the test proves nothing"
else
    ok "prune does delete with POST=on (so the POST=off case means something)"
fi

# 2. the merge gate must not merge during a rehearsal. Same shape as prune:
#    `gh pr merge` runs inside the script, where no deny rule can see it.
if sc_case 2; then
grep -q 'POST="\${MAINTAINER_POST:-on}"' "$root/bin/maintainer-merge" \
    && ok "the merge gate reads POST" || bad "the merge gate ignores POST"
# Located by line number rather than by a context window, which was set to four
# lines and missed a five-line guard.
python3 - "$root/bin/maintainer-merge" <<'PYEOF' && ok "the merge gate returns before gh pr merge when POST=off" || bad "a rehearsal could merge a pull request"
import sys
lines = open(sys.argv[1]).read().splitlines()
merge = next((i for i, l in enumerate(lines) if "gh pr merge " in l and not l.strip().startswith("#")), None)
if merge is None:
    print("  no gh pr merge call found at all"); sys.exit(1)
window = lines[max(0, merge - 12):merge]
if not any('"$POST" = off' in l for l in window):
    print("  no POST=off check in the 12 lines before the merge call"); sys.exit(1)
if not any("return 0" in l for l in window):
    print("  the POST=off branch does not return before the merge"); sys.exit(1)
sys.exit(0)
PYEOF
sc_end; fi

# 3. the profile name must not reach a shell unquoted.
inj="$stub_dir/injhome"; rm -rf "$inj"
evil='pwn"; touch '"$stub_dir"'/INJECTED; :"'
mkdir -p "$inj/.local/share/maintainer/profiles/$evil"
printf 'REPO_SLUG="o/r"\nTASKS="review"\n' > "$inj/.local/share/maintainer/profiles/$evil/profile.env"
rm -f "$stub_dir/INJECTED"
out=$(HOME="$inj" MAINTAINER_PROFILE="$evil" python3 "$root/bin/maintainer" status 2>&1); rc=$?
if [ -e "$stub_dir/INJECTED" ]; then
    bad "a profile name reached the shell: the injection ran"
elif [ "$rc" != 0 ] && grep -q 'not a plain word' <<<"$out"; then
    ok "a profile name that is not a plain word is refused"
else
    bad "the injection did not run, but nothing refused it either (rc=$rc)"
fi
# And the ordinary case must still work, or the refusal is just a broken tool.
ok2="$stub_dir/okhome"; mkdir -p "$ok2/.local/share/maintainer/profiles/plain"
printf 'REPO_SLUG="o/r"\nTASKS="review"\nSTATE_DIR="$HOME/st"\nPOST="off"\n' \
    > "$ok2/.local/share/maintainer/profiles/plain/profile.env"
HOME="$ok2" MAINTAINER_PROFILE=plain python3 "$root/bin/maintainer" status 2>&1 | grep -q 'o/r' \
    && ok "a plain profile name still works" || bad "the name check rejects a valid profile"
# And it must survive a host with no systemd. `_next_runs` called systemctl by
# name with no guard, so on macOS, Windows or in any container `status` printed
# three lines and died with an uncaught FileNotFoundError. Three of the four
# schedulers this project documents are not systemd. Found by running this suite
# inside python:3.12, where it is not the test that fails but the tool.
nosysd="$stub_dir/nosystemd"; mkdir -p "$nosysd"
for t in git bash python3 grep sed; do
    src="$(command -v "$t" 2>/dev/null)" && ln -sf "$src" "$nosysd/$t"
done
out=$(PATH="$nosysd" HOME="$ok2" MAINTAINER_PROFILE=plain \
      python3 "$root/bin/maintainer" status 2>&1); rc=$?
[ "$rc" = 0 ] && ok "status runs on a host with no systemctl" \
    || bad "status died without a scheduler (rc=$rc)"
grep -q 'Traceback' <<<"$out" \
    && bad "status printed a Python traceback at the user" \
    || ok "and it does so without a traceback"
grep -q 'task .*next' <<<"$out" \
    && ok "and still prints the task table, with no next-run column to fill" \
    || bad "status stopped before the task table"

echo "== the social preview is the size GitHub actually accepts =="
# GitHub's own docs: at least 640x320, 1280x640 for best display, under 1MB,
# cropped to 2:1. Read from the PNG's IHDR chunk so this needs no image library:
# an optional dependency that is absent turns a gate green, which this project
# has been bitten by before.
python3 - "$root/assets/social-preview.png" <<'PYEOF' && ok "social preview is 1280x640 and under 1MB" || bad "the social preview would be rejected or cropped badly"
import struct, sys, os
p = sys.argv[1]
if not os.path.exists(p):
    print("  no social preview at", p); sys.exit(1)
b = open(p, "rb").read()
if b[:8] != b"\x89PNG\r\n\x1a\n":
    print("  not a PNG; GitHub takes PNG, JPG or GIF"); sys.exit(1)
w, h = struct.unpack(">II", b[16:24])
bad = []
if (w, h) != (1280, 640):
    bad.append(f"{w}x{h}, wanted 1280x640")
if len(b) > 1_000_000:
    bad.append(f"{len(b)} bytes, over the 1MB limit")
for m in bad:
    print("  " + m)
sys.exit(1 if bad else 0)
PYEOF
# assets/README.md promises every pixel that matters sits at least 130px from
# an edge; GitHub's own template asks for 80. Decode the PNG with zlib alone
# (same reason as above) and measure where the non-background pixels are.
python3 - "$root/assets/social-preview.png" <<'PYEOF' && ok "social preview content keeps 130px clear of every edge" || bad "social preview content runs into the area GitHub crops"
import struct, sys, zlib
b = open(sys.argv[1], "rb").read()
w, h, depth, ctype = struct.unpack(">IIBB", b[16:26])
if depth != 8 or ctype not in (2, 6):
    print(f"  unexpected PNG format depth={depth} type={ctype}"); sys.exit(1)
bpp = 3 if ctype == 2 else 4
idat, i = b"", 8
while i < len(b):
    n, kind = struct.unpack(">I4s", b[i:i + 8])
    if kind == b"IDAT":
        idat += b[i + 8:i + 8 + n]
    i += 12 + n
raw, stride, prev, rows, o = zlib.decompress(idat), w * bpp, bytearray(w * bpp), [], 0
for _ in range(h):
    f, line, o = raw[o], bytearray(raw[o + 1:o + 1 + stride]), o + 1 + stride
    for x in range(stride):
        a = line[x - bpp] if x >= bpp else 0
        c = prev[x - bpp] if x >= bpp else 0
        if f == 1: line[x] = (line[x] + a) & 255
        elif f == 2: line[x] = (line[x] + prev[x]) & 255
        elif f == 3: line[x] = (line[x] + (a + prev[x]) // 2) & 255
        elif f == 4:
            p = a + prev[x] - c
            pa, pb, pc = abs(p - a), abs(p - prev[x]), abs(p - c)
            line[x] = (line[x] + (a if pa <= pb and pa <= pc else prev[x] if pb <= pc else c)) & 255
    rows.append(line); prev = line
bg = rows[0][0:3]
xs, ys = [], []
for y, row in enumerate(rows):
    for x in range(w):
        if max(abs(row[x * bpp + k] - bg[k]) for k in range(3)) > 12:
            xs.append(x); ys.append(y)
if not xs:
    print("  the preview is blank"); sys.exit(1)
m = 130
box = (min(xs), min(ys), max(xs), max(ys))
if box[0] < m or box[1] < m or box[2] > w - 1 - m or box[3] > h - 1 - m:
    print(f"  content spans x {box[0]}..{box[2]}, y {box[1]}..{box[3]}; wanted inside {m}..{w - 1 - m}, {m}..{h - 1 - m}")
    sys.exit(1)
PYEOF
[ -f "$root/assets/logo.svg" ] && ok "the mark is in the tree" || bad "assets/logo.svg is missing"
grep -q 'assets/logo.svg' "$root/README.md" && ok "the README shows the mark" || bad "the README does not reference the mark"
# The PNG is generated from the SVG. If someone edits the PNG alone the two
# drift, and the thing people see is the one nobody reviewed.
grep -q 'social-preview.svg' "$root/assets/social-preview.html" \
    && ok "the preview page renders the SVG, so the PNG has one source" \
    || bad "the PNG has no reproducible source"

echo "== no tool defaults to a person or a repository =="
# Every bash tool used to carry personal fallbacks: REPO_SLUG to one project,
# ACCOUNT to one GitHub login, REPO_PATH to a directory on one laptop. Harmless
# with one user, wrong with two: a stranger running `maintainer-repo prune` with
# no profile would have queried somebody else's repository.
needle_repo="lacs-project""/sysknife"
needle_user="vladimir""rott"
for f in "$root"/bin/* "$root"/lib/*.sh "$root"/install.sh "$root"/new-profile.sh; do
    [ -f "$f" ] || continue
    # Executable lines only. The files explain the defaults they used to have,
    # and a check that reads its own rationale is the vacuous kind.
    if sc_case 1; then
    code=$(sed 's/#.*//' "$f")
    if grep -qF "$needle_repo" <<<"$code"; then
        bad "$(basename "$f") still defaults to a specific repository"
    elif grep -qF "$needle_user" <<<"$code"; then
        bad "$(basename "$f") still names a specific GitHub account"
    else
        ok "$(basename "$f") names no repository or account in code"
    fi
    sc_end; fi
done
# And it must refuse rather than guess.
nohome="$stub_dir/noprofile"; mkdir -p "$nohome"
for tool in maintainer-repo maintainer-merge; do
    out=$(env -u MAINTAINER_SLUG -u MAINTAINER_REPO -u MAINTAINER_PROFILE -u MAINTAINER_STATE \
              -u MAINTAINER_ACCOUNT HOME="$nohome" bash "$root/bin/$tool" prune 2>&1); rc=$?
    if [ "$rc" != 0 ] && grep -q 'no profile is deployed' <<<"$out"; then
        ok "$tool refuses when no profile says which repository"
    else
        bad "$tool ran without knowing which repository (rc=$rc)"
    fi
done
# Two deployed profiles must be an error, not a coin toss.
two="$stub_dir/twoprof"; mkdir -p "$two/.local/share/maintainer/profiles/alpha" \
                                  "$two/.local/share/maintainer/profiles/beta"
for p in alpha beta; do
    printf 'REPO_SLUG="o/%s"\nREPO_PATH="/tmp/%s"\nSTATE_DIR="/tmp/%s-st"\nGH_ACCOUNT="a"\n' \
        "$p" "$p" "$p" > "$two/.local/share/maintainer/profiles/$p/profile.env"
done
cp "$root/lib/profile.sh" "$two/.local/share/maintainer/profile.sh"
cp "$root/lib/defaults.env" "$two/.local/share/maintainer/defaults.env"
out=$(env -u MAINTAINER_SLUG -u MAINTAINER_REPO -u MAINTAINER_PROFILE -u MAINTAINER_STATE \
          -u MAINTAINER_ACCOUNT HOME="$two" bash "$root/bin/maintainer-repo" release-check 2>&1)
grep -q '2 profiles are deployed' <<<"$out" \
    && ok "two deployed profiles must be disambiguated, not guessed" \
    || bad "a tool picked a profile on its own"

echo "== a public repository has the files a public repository needs =="
# Checked because they are easy to intend and easy to forget, and because a
# licence that is absent is a licence nobody can rely on.
for f in LICENSE SECURITY.md CODE_OF_CONDUCT.md CODEOWNERS CONTRIBUTING.md CHANGELOG.md \
         .github/PULL_REQUEST_TEMPLATE.md .github/dependabot.yml \
         .github/ISSUE_TEMPLATE/bug.yml .github/ISSUE_TEMPLATE/feature.yml \
         .github/ISSUE_TEMPLATE/config.yml .github/workflows/ci.yml; do
    [ -s "$root/$f" ] && ok "$f is present and not empty" || bad "$f is missing or empty"
done
# Actions are the supply chain here. A tag is mutable; a SHA is not.
unpinned=$(grep -hoE 'uses: [^ ]+' "$root"/.github/workflows/*.yml | grep -vE '@[0-9a-f]{40}' || true)
if [ -z "$unpinned" ]; then ok "every action is pinned by SHA"; else
    bad "an action is pinned by tag, which is mutable:"; printf '%s\n' "$unpinned" | sed 's/^/        /'; fi
# Every SHA pin needs a comment naming what it claims to be. Whether the claim
# is TRUE needs the network, so scripts/verify-action-pins.sh does that in CI.
missing=0
while read -r line; do
    grep -qE '# *\S' <<<"$line" || { bad "a SHA pin carries no version comment: $line"; missing=1; }
done < <(grep -hE 'uses: [^ ]+@[0-9a-f]{40}' "$root"/.github/workflows/*.yml)
[ "$missing" = 0 ] && ok "every SHA pin says which version it claims to be"
[ -x "$root/scripts/verify-action-pins.sh" ] \
    && ok "a script exists that checks those claims against the API" \
    || bad "nothing verifies that a pin is the tag it claims"

echo "== the pin verifier fails on a uses: line it cannot parse =="
# Issue #31. The script's own second line says "Every `uses:` in
# .github/workflows must be pinned to a SHA", and its extraction was
#   grep -hoE 'uses: [^@ ]+@[0-9a-f]{40} *# *\S+'
# which requires a line to be pinned already. A floating tag, a short SHA or a
# pin with no comment never entered the loop, `fail` stayed 0, and the all-clear
# printed. A loop cannot fail on a line it never saw.
vp="$root/scripts/verify-action-pins.sh"
pinroot="$stub_dir/pinroot"; rm -rf "$pinroot"; mkdir -p "$pinroot/.github/workflows"
printf 'jobs:\n  a:\n    steps:\n      - uses: actions/deploy-pages@v5\n' \
    > "$pinroot/.github/workflows/x.yml"
out="$(bash "$vp" "$pinroot" 2>&1)"; rc=$?
[ "$rc" != 0 ] && ok "an unpinned uses: line fails the verifier" \
    || bad "a floating tag passes the guard that exists to forbid it (rc=$rc)"
grep -q 'actions/deploy-pages@v5' <<<"$out" \
    && ok "and the line it could not parse is named" \
    || bad "the refusal does not say which line: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90)"
# A short SHA is the same defect wearing a different length.
printf 'jobs:\n  a:\n    steps:\n      - uses: actions/checkout@3d3c42e  # v7\n' \
    > "$pinroot/.github/workflows/x.yml"
bash "$vp" "$pinroot" >/dev/null 2>&1 \
    && bad "a short SHA passes as a pin" \
    || ok "a short SHA fails too"
# A pin with no version comment: the half the script DID check is meaningless
# without knowing the claim.
printf 'jobs:\n  a:\n    steps:\n      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1\n' \
    > "$pinroot/.github/workflows/x.yml"
bash "$vp" "$pinroot" >/dev/null 2>&1 \
    && bad "a SHA pin with no version comment passes" \
    || ok "a SHA pin with no version comment fails"
# Structural twin, no network: a workflow with no uses: at all is not an error.
printf 'jobs:\n  a:\n    steps:\n      - run: echo hi\n' \
    > "$pinroot/.github/workflows/x.yml"
bash "$vp" "$pinroot" >/dev/null 2>&1 \
    && ok "a workflow with no actions is not reported as unpinned" \
    || bad "the new count check rejects a workflow that uses no actions"
# Behavioural twin, gh stubbed: a correctly pinned line still passes end to end,
# or the check is just rejecting everything.
printf 'jobs:\n  a:\n    steps:\n      - uses: acme/act@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  # v1\n' \
    > "$pinroot/.github/workflows/x.yml"
make_stub gh "case \"\$*\" in
      *git/ref/tags*object.type*) echo commit;;
      *git/ref/tags*object.sha*) echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;;
    esac"
out="$(PATH="$stub_dir:$PATH" bash "$vp" "$pinroot" 2>&1)"; rc=$?
[ "$rc" = 0 ] && ok "a correctly pinned line whose comment is true still passes" \
    || bad "the verifier now fails a good pin: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90)"
rm -f "$stub_dir/gh"

# CI must run the gates rather than merely exist.
for gate in "tests/run-tests.sh" "evals/run-evals.sh" "scripts/check_claims.sh" "shellcheck"; do
    grep -q "$gate" "$root/.github/workflows/ci.yml" \
        && ok "CI runs $gate" || bad "CI does not run $gate"
done
# The workflow needs no write access and no secrets.
grep -q 'contents: read' "$root/.github/workflows/ci.yml" \
    && ok "CI asks for read-only permissions" || bad "CI does not restrict its permissions"
# Comments stripped first. This grep ran over the whole file, so writing the
# words `secrets.GITHUB_TOKEN` in a comment that EXPLAINS the policy failed the
# check that enforces it. GITHUB_TOKEN itself is allowed: it is the automatic
# token, scoped by `permissions:` above, and GitHub hands a fork's pull request
# a read-only copy. Any other secret is a repository secret, which a fork's
# `pull_request` run must never see.
othersecret=$(sed 's/#.*//' "$root/.github/workflows/ci.yml" \
    | grep -oE 'secrets\.[A-Za-z_][A-Za-z0-9_]*' | grep -v '^secrets\.GITHUB_TOKEN$' || true)
if [ -z "$othersecret" ]; then ok "CI reads no repository secret, so a fork's PR is safe to run"
else bad "CI reads a secret a fork's pull request must not be able to: $othersecret"; fi

echo "== the merge gate verifies more than one language =="
# It assumed Rust in four places: the image, the command, the mutation glob and
# the way it counts what ran. A pull request touching only shell could not earn
# a receipt and therefore could never merge, which took a live contributor PR to
# notice because refusing to merge is the safe outcome. Issue #11.
vs="$root/profiles/sysknife/verify.d"
[ -d "$vs" ] && ok "the profile declares verification suites" || bad "no verify.d/ for sysknife"
for suite in rust shell; do
    [ -f "$vs/$suite.sh" ] && ok "suite '$suite' exists" || bad "suite '$suite' is missing"
done
# Every suite must answer all four questions. A suite that cannot say how to
# count what ran is a suite that can write a receipt for a vacuous pass.
for suite in "$vs"/*.sh; do
    n="$(basename "$suite" .sh)"
    missing=""
    for fn in suite_covers suite_image suite_mutate_glob suite_command suite_ran; do
        grep -q "^$fn()" "$suite" || missing="$missing $fn"
    done
    [ -z "$missing" ] && ok "suite '$n' answers every question" \
        || bad "suite '$n' is missing:$missing"
done
# Path inference: each suite must claim its own and disclaim the others.
( . "$vs/rust.sh";  suite_covers "crates/x/src/lib.rs" ) && ok "rust claims a .rs path" || bad "rust does not claim .rs"
( . "$vs/rust.sh";  suite_covers "scripts/x.sh" )        && bad "rust claims a .sh path" || ok "rust disclaims .sh"
( . "$vs/shell.sh"; suite_covers "scripts/x.sh" )        && ok "shell claims a .sh path" || bad "shell does not claim .sh"
( . "$vs/shell.sh"; suite_covers "crates/x/src/lib.rs" ) && bad "shell claims a .rs path" || ok "shell disclaims .rs"
# A path no suite covers must be refused rather than silently run under Rust.
# This was a grep for the refusal's exact wording, which is the weak shape this
# project keeps recording: rewording the message to be more useful broke the
# test while the behaviour was unchanged. Driven through the function now.
if sc_case 2; then
uncov="$(PROFILE_DIR="$(dirname "$vs")" MAINTAINER_PROFILE=vs MAINTAINER_SLUG=o/r \
    MAINTAINER_REPO=/tmp MAINTAINER_ACCOUNT=t \
    bash -c '. "'"$root"'/bin/maintainer-merge"; pick_suite "" crates/x/src/lib.rs weird.xyz' 2>&1)"
urc=$?
[ "$urc" != 0 ] \
    && ok "an uncovered path is refused, not guessed at" \
    || bad "verify fell back to a suite that does not run the changed code (returned '$uncov')"
grep -q 'weird\.xyz' <<<"$uncov" \
    && ok "and the refusal names the path it could not place" \
    || bad "the refusal does not say which path had no suite"
sc_end; fi
# The receipt has to say which suite proved it.
if sc_case 1; then
grep -q '"suite": suite' "$root/bin/maintainer-merge" \
    && ok "the receipt records which suite produced it" \
    || bad "a receipt does not say what verified it"
sc_end; fi

