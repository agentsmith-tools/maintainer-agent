#!/usr/bin/env bash
# Part of the offline suite: prompt assembly, installers, backend walls, doctor, timers and the cron fallback.
# tests/run-tests.sh sources this file in one shell, after its own preamble and after
# the parts before it, so every helper and variable it reads comes from there. It is
# sourced, never run on its own, and carries a shebang so the shell sweeps pick it up.

# The parts share one shell. These names are set by the driver or by an earlier part;
# declare only tells the linter they are read here, and assigns and changes nothing.
declare mg
declare root
declare s
declare stub_dir
declare suite_root

echo "== the deny wall lets maintainer-merge update through and keeps every raw spelling of update-branch out =="
# `update` is the only path to an Update branch. The wall must therefore match
# nothing the wrapper is called with, and must still match the raw verb in every
# flag order, because a deny rule keyed to `gh api -X PUT` alone is walked past
# by `gh api <path> -X PUT`. Rule COUNTS are machine-dependent (they follow where
# `which` finds binaries), so this pins verbs and decisions, never a total.
wallchk() {  # $1 = settings json -> prints one verdict line per probe
    python3 - "$1" <<'PYEOF2'
import json, re, sys
rules = json.load(open(sys.argv[1]))["permissions"]["deny"]
def hit(cmd):
    for r in rules:
        m = re.fullmatch(r"Bash\((.*)\)", r)
        if not m:
            continue
        body = m.group(1)
        prefix = body.endswith(":*")
        pat = re.escape(body[:-2] if prefix else body).replace(r"\*", ".*")
        if (re.match(pat, cmd) if prefix else re.fullmatch(pat, cmd)):
            return True
    return False
for tag, want, cmd in (
    ("allow", False, "maintainer-merge update 31"),
    ("allow", False, "/home/fakeuser/.local/bin/maintainer-merge update 31"),
    ("allow", False, "~/.local/bin/maintainer-merge update 31"),
    ("allow", False, "./bin/maintainer-merge update 31"),
    ("deny", True, "gh pr update-branch 31"),
    ("deny", True, "/usr/bin/gh pr update-branch 31"),
    ("deny", True, "gh api -X PUT repos/o/r/pulls/31/update-branch -f expected_head_sha=abc"),
    ("deny", True, "gh api --method PUT repos/o/r/pulls/31/update-branch"),
    ("deny", True, "gh api repos/o/r/pulls/31/update-branch -X PUT"),
    ("deny", True, "gh api repos/o/r/pulls/31/update-branch --method PUT -f x=y"),
    ("deny", True, "gh api graphql -f query='mutation{updatePullRequestBranch(input:{pullRequestId:\"x\"}){clientMutationId}}'"),
    ("deny", True, "maintainer-merge receipt 31 abc proof"),
):
    print(("ok " if hit(cmd) == want else "BAD ") + f"{tag} {cmd}")
PYEOF2
}
for wp in _template magent sysknife; do
    wd="$stub_dir/updwall-$wp"; rm -rf "$wd"; mkdir -p "$wd"; cp "$root/profiles/$wp/deny.json" "$wd/"
    python3 "$root/scripts/render-settings.py" "$wd" /home/fakeuser >/dev/null 2>&1 \
        && ok "$wp: the deny wall renders" || bad "$wp: render-settings failed on its deny.json"
    verdicts="$(wallchk "$wd/settings.json")"
    badv="$(grep '^BAD ' <<<"$verdicts" || true)"
    [ -z "$badv" ] && [ "$(grep -c '^ok ' <<<"$verdicts")" = 12 ] \
        && ok "$wp: update is matched by no deny rule, and every raw update-branch spelling is denied (12 probes)" \
        || bad "$wp: wall verdicts wrong: $(tr '\n' ';' <<<"${badv:-$verdicts}")"
    python3 - "$wd/settings.json" "$wd/settings-rehearsal.json" <<'PYEOF2' && ok "$wp: a rehearsal denies maintainer-merge update, the live wall does not" || bad "$wp: the rehearsal wall does not deny maintainer-merge update"
import json, sys
live = set(json.load(open(sys.argv[1]))["permissions"]["deny"])
reh = set(json.load(open(sys.argv[2]))["permissions"]["deny"])
want = "Bash(maintainer-merge update:*)"
sys.exit(0 if want in reh and want not in live else 1)
PYEOF2
    rm -rf "$wd"
done

for wp in _template magent sysknife; do
    python3 - "$root/profiles/$wp/opencode.json" <<'PYEOF2' && ok "$wp: opencode allows maintainer-merge update and denies both raw update-branch spellings" || bad "$wp: opencode.json decides update-branch wrongly"
import fnmatch, json, sys
b = json.load(open(sys.argv[1]))["permission"]["bash"]
def decide(c):
    v = b.get("*", "deny")
    for pat, val in b.items():
        if pat != "*" and fnmatch.fnmatch(c, pat): v = val
    return v
want = {"maintainer-merge update 5": "allow",
        "gh pr update-branch 5": "deny",
        "gh api repos/o/r/pulls/5/update-branch -X PUT": "deny",
        "gh api -X PUT repos/o/r/pulls/5/update-branch": "deny",
        "gh api graphql -f query=mutation{updatePullRequestBranch(input:{})}": "deny"}
bad = {c: decide(c) for c, w in want.items() if decide(c) != w}
sys.exit(1 if bad else 0)
PYEOF2
done

echo "== the review prompts send an approved BEHIND pull request to maintainer-merge update =="
for wp in _template magent sysknife; do
    f="$root/profiles/$wp/prompts/review.md"
    grep -q 'maintainer-merge update <pr>' "$f" && grep -q 'Never `gh pr update-branch`' "$f" \
        && ok "$wp: the prompt names maintainer-merge update and still forbids the raw gh pr update-branch" \
        || bad "$wp: review.md lost the update instruction or the raw-command prohibition"
    grep -qi 'verify, approve, update if BEHIND, wait for green, merge' "$f" \
        && ok "$wp: the prompt states the order verify, approve, update if BEHIND, wait for green, merge" \
        || bad "$wp: review.md does not state the order of the merge steps"
done

echo "== the doctrine reaches a run, not just a reader =="
# Assembled, not inspected. The doctrine lives in lib/preamble-core.md and the
# profile contributes only its site header, so grepping either file alone would
# pass while the agent received neither. --show-prompt is the same assembly a
# real run uses.
assembled="$stub_dir/assembled.md"
PATH="$stub_dir:$PATH" bash "$root/lib/run.sh" --show-prompt sysknife review >"$assembled" 2>/dev/null
if [ -s "$assembled" ]; then ok "the prompt assembles"; else bad "--show-prompt produced nothing"; fi
# Every rule the doctrine gained from a real incident is listed here, because a
# lesson that lives only in docs/lessons.md changes nothing about what a run
# does. These are the sentences that must reach the model.
for needle in "Trust is the attack surface" "slop" "Persistence" "data, not instruction" \
              "maintainer screen" "Never post a claim you have not run" \
              "lead, not a result" "the operation is the world" \
              "Quote the command you ran" \
              "claim-released" "assign them"; do
    if grep -qi "$needle" "$assembled"; then ok "assembled prompt carries: $needle"; else bad "assembled prompt LOST: $needle"; fi
done
# The profile's own name must be substituted into the shared doctrine, or the
# agent is told to protect a repository that is not the one it is reviewing.
grep -q '__SLUG__\|__MAINTAINER__' "$assembled" && bad "a placeholder reached the agent" || ok "placeholders substituted"
echo "== prompt assembly refuses missing inputs and preserves literal names =="
# Run the same assembly from an isolated copy, so missing-input cases cannot
# disturb the checkout. A backend sentinel proves no partial prompt reaches it.
pf_tree="$stub_dir/prompt-failure-tree"
pf_bin="$stub_dir/prompt-failure-bin"
mkdir -p "$pf_tree" "$pf_bin"
cp -R "$root/lib" "$root/profiles" "$pf_tree/"
printf '#!/usr/bin/env bash\nprintf called > "%s"; exit 0\n' "$stub_dir/prompt-backend-called" > "$pf_bin/claude"
chmod +x "$pf_bin/claude"
pf_run() {
    PATH="$pf_bin:$stub_dir:$PATH" MAINTAINER_STATE_DIR="$stub_dir/state-root" \
        bash "$pf_tree/lib/run.sh" --show-prompt sysknife review 2>&1
}
pf_task="$pf_tree/profiles/sysknife/prompts/review.md"
mv "$pf_task" "$pf_task.saved"
pf_out="$(pf_run)"; pf_rc=$?
if [ "$pf_rc" -ne 0 ] && grep -Fq "$pf_task" <<<"$pf_out" \
   && [ ! -e "$stub_dir/prompt-backend-called" ]; then
    ok "a missing task prompt stops assembly before the backend"
else
    bad "a missing task prompt did not fail closed before the backend (rc=$pf_rc)"
fi
mv "$pf_task.saved" "$pf_task"
pf_common="$pf_tree/profiles/sysknife/prompts/common-preamble.md"
mv "$pf_common" "$pf_common.saved"
pf_out="$(pf_run)"; pf_rc=$?
if [ "$pf_rc" -ne 0 ] && grep -Fq "$pf_common" <<<"$pf_out" \
   && [ ! -e "$stub_dir/prompt-backend-called" ]; then
    ok "a missing common preamble stops assembly before the backend"
else
    bad "a missing common preamble did not fail closed before the backend (rc=$pf_rc)"
fi
mv "$pf_common.saved" "$pf_common"
sed -i 's/^MAINTAINER_NAME=.*/MAINTAINER_NAME="a|b"/' "$pf_tree/profiles/sysknife/profile.env"
pf_out="$(pf_run)"; pf_rc=$?
if [ "$pf_rc" = 0 ] && grep -Fq 'a|b reads the reports' <<<"$pf_out" \
   && [ ! -e "$stub_dir/prompt-backend-called" ]; then
    ok "a maintainer name containing the sed delimiter reaches the prompt literally"
else
    bad "a maintainer name containing the sed delimiter was damaged (rc=$pf_rc)"
fi
sed -i 's/^MAINTAINER_NAME=.*/MAINTAINER_NAME="Smith \& Sons"/' "$pf_tree/profiles/sysknife/profile.env"
pf_out="$(pf_run)"; pf_rc=$?
if [ "$pf_rc" = 0 ] && grep -Fq 'Smith & Sons reads the reports' <<<"$pf_out" \
   && [ ! -e "$stub_dir/prompt-backend-called" ]; then
    ok "an ampersand in the maintainer name is preserved literally"
else
    bad "an ampersand in the maintainer name expanded in the prompt (rc=$pf_rc)"
fi
echo "== the run stamps its own build into the prompt =="
# A report is only auditable if it names the build that wrote it, and the
# doctrine tells the agent to take that from the first line of the prompt.
# It was not there. Measured on 2026-09-04T21-17-review: the run executed
# v0.4.0 and stamped its report `v0.3.0 (commit 10a7e79)`, because the only
# version visible anywhere was the one in the previous report, so it copied
# that. A wrong stamp is worse than none; it points an audit at code that did
# not run.
if grep -q 'unstamped' "$assembled"; then
    ok "the prompt names the build (unstamped, since the repo tree carries no VERSION)"
else
    bad "the assembled prompt names no build, so the report's version can only be copied"
fi
# And it must be the deployed stamp, not a constant. Render a VERSION beside a
# copy of the tree and check the prompt follows it.
vlay="$stub_dir/versioned"
rm -rf "$vlay"; mkdir -p "$vlay"
cp -r "$root/lib" "$root/profiles" "$vlay/"
cp "$root/lib/run.sh" "$vlay/run.sh"
printf 'version=v9.9.9\ncommit=deadbee\ninstalled_at=2026-01-01T00:00:00Z\ninstalled_from=/nowhere\n' >"$vlay/VERSION"
vout=$(PATH="$stub_dir:$PATH" bash "$vlay/run.sh" --show-prompt sysknife review 2>/dev/null)
grep -q 'v9.9.9' <<<"$vout" \
    && ok "the prompt carries the deployed version" \
    || bad "the prompt ignores VERSION, so every run is stamped the same"
grep -q 'deadbee' <<<"$vout" \
    && ok "the prompt carries the deployed commit" \
    || bad "the prompt names a version with no commit, which cannot be checked out"
grep -qi 'not copy\|do not copy' <<<"$vout" \
    && ok "the prompt forbids copying the stamp from an earlier report" \
    || bad "nothing tells the agent where NOT to get the version from"

echo "== the prompt prescribes an assignment verb that works =="
# `gh issue edit --add-assignee` resolves the login through GraphQL first and
# answers "'USER' not found" for an outside contributor, leaving the label
# applied and the assignment missing. Measured 2026-09-05 on #272: the edit
# refused atanishka308 and the REST POST assigned them on the next line. The
# doctrine had the working call; the sysknife task prompt still named the
# broken one, and the task prompt is the later, more proximate instruction.
# Scope: a prompt that PRESCRIBES the broken verb, not one that names it to
# forbid it. The first version of this check failed on the sentence that fixed
# the defect, which is the shape docs/lessons.md records four times over: a
# grep that matches its own documentation. A line carrying "not" is telling the
# agent what to avoid, so it is out of scope.
if grep -rn -- '--add-assignee' "$root"/profiles/*/prompts/*.md | grep -v '\bnot\b' | grep -q .; then
    bad "a task prompt still prescribes 'gh issue edit --add-assignee', which 404s for outside contributors"
else
    ok "no task prompt prescribes the assignment verb that refuses outside contributors"
fi
grep -q 'issues/336/assignees' "$assembled" \
    && ok "the assembled prompt carries the assignment call that works" \
    || bad "the assembled prompt lost the REST assignment call"

grep -q 'lacs-project/sysknife' "$assembled" && ok "the doctrine names this profile's repository" || bad "slug substitution did not happen"
if grep -q 'maintainer-merge' "$root/profiles/sysknife/prompts/review.md"; then
    ok "review prompt routes merges through the gate"
else
    bad "review prompt no longer names the merge gate"
fi
if grep -q 'Bash(gh pr merge:\*)' "$s"; then
    ok "direct gh pr merge is still denied (the gate is the only path)"
else
    bad "gh pr merge is no longer denied; the gate can be bypassed"
fi

echo "== a suite image built here needs its recipe in the tree =="
# The shell suite's image has now been wrong four times, each time picked by
# hand for what its scripts happened to need that week: bash:5 with no python3,
# python:3.12-slim with no git, python:3.12 with no cargo, and uid 0 in all
# three. It is built from a Containerfile now. An image that exists only on the
# host that built it is not a gate anybody else can reproduce, and the failure
# mode is the one this repository keeps hitting: the gate blames the pull
# request for its own container.
for _v in "$root"/profiles/*/verify.d/*.sh; do
    [ -e "$_v" ] || continue
    # shellcheck disable=SC1090
    _img="$( . "$_v" >/dev/null 2>&1; declare -F suite_image >/dev/null && suite_image )"
    _sn="$(basename "$_v" .sh)"
    case "$_img" in
        localhost/*)
            if [ -f "${_v%.sh}.Containerfile" ]; then
                ok "suite '$_sn' builds $_img from $_sn.Containerfile"
            else
                bad "suite '$_sn' runs $_img and nothing in the tree builds it"
            fi
            ;;
        "") bad "suite '$_sn' declares no suite_image" ;;
        *)  ok "suite '$_sn' runs a registry image ($_img)" ;;
    esac
done
# And doctor has to say "build", not "pull", for an image no registry holds.
if sc_case 1; then
if grep -qE 'podman build .*-t ' "$root/bin/maintainer-doctor"; then
    ok "doctor tells you how to build a missing locally-built suite image"
else
    bad "doctor would tell you to pull an image no registry has"
fi
sc_end; fi

echo "== the installed layout resolves, not just the repo layout =="
# The bug this catches: run.sh looked for profiles at ../profiles, which is right
# in the repo (lib/run.sh) and wrong once installed beside them. The unit exited
# 64 and the timer logged nothing, because a file-existence check is not a start.
lay="$stub_dir/layout"; mkdir -p "$lay/profiles/sysknife/prompts"
cp "$root/lib/run.sh" "$lay/run.sh"; cp "$root/lib/profile.sh" "$lay/profile.sh"; cp "$root/lib/defaults.env" "$lay/defaults.env"
cp -r "$root/lib/backends" "$lay/backends"
cp "$root/profiles/sysknife/profile.env" "$lay/profiles/sysknife/"
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'api user'*) echo nobody; exit 0;; esac"
out=$(PATH="$stub_dir:$PATH" HOME="$stub_dir" bash "$lay/run.sh" sysknife review 2>&1); rc=$?
# It must get PAST profile resolution and fail on the identity gate (rc=1),
# not on "no profile" (rc=64).
if [ "$rc" = "1" ]; then ok "installed layout resolves the profile (reached the identity gate)"
elif [ "$rc" = "64" ]; then bad "installed layout cannot find its profile (exit 64) -- the deployment bug"
else bad "installed layout: unexpected rc=$rc"; fi

echo "== prose discipline is opt-OUT, and reaches the prompt =="
ps="$root/lib/prose-style.md"
for needle in "em dashes" "throat-clearing" "not X, it" "adverbs" "Never disclose"; do
    if grep -qi "$needle" "$ps"; then ok "prose style covers: $needle"; else bad "prose style lost: $needle"; fi
done
if grep -q 'PROSE_STYLE="${MAINTAINER_PROSE_STYLE:-stop-slop}"' "$root/profiles/sysknife/profile.env"; then
    ok "default is stop-slop (opt-out, not opt-in)"
else
    bad "prose style is no longer on by default"
fi
# A misspelled or absent value must still get the discipline. Only "raw" opts out.
if sc_case 1; then
if grep -q '!= "raw"' "$root/lib/run.sh"; then
    ok "only an explicit \"raw\" disables it; a typo still writes well"
else
    bad "the opt-out is not fail-safe"
fi
sc_end; fi
# And it must actually land in the assembled prompt.
if sc_case 1; then
if grep -q 'prose-style.md' "$root/lib/run.sh" && grep -q 'prose-style.md' "$root/install.sh"; then
    ok "prose style is assembled into the prompt and shipped by install"
else
    bad "prose style is not wired into the prompt or not installed"
fi
sc_end; fi

echo "== cross-platform: only scheduling forks, the core does not =="
[ -f "$root/platform/linux/maintainer@.service" ]        && ok "linux: systemd units present"   || bad "linux units missing"
[ -x "$root/platform/macos/install-launchd.sh" ]         && ok "macos: launchd installer present" || bad "macos installer missing"
[ -f "$root/platform/windows/Install-Maintainer.ps1" ]   && ok "windows: PowerShell installer present" || bad "windows installer missing"
if sc_case 1; then
if grep -q 'uname -s' "$root/install.sh"; then ok "install.sh dispatches by platform"; else bad "install.sh is not platform-aware"; fi
sc_end; fi
# Only one implementation of the gate may exist.
# Count only executable implementations. Prose that describes the gate (docs,
# eval scenarios, this file) is not a second implementation, and an earlier
# version of this check counted it as one.
if sc_case 1; then
gates=$(grep -rl 'no verification receipt' "$root/bin" "$root/lib" "$root/platform" 2>/dev/null | wc -l)
if [ "$gates" = "1" ]; then ok "exactly one executable implementation of the merge gate"; else bad "$gates executable implementations of the merge gate (they will drift)"; fi
sc_end; fi

echo "== the PowerShell installer =="
ps1="$root/platform/windows/Install-Maintainer.ps1"
# The real parse runs in CI, in a job whose container IS powershell, so it
# cannot be skipped. This used to attempt it here when the image happened to be
# local and report a PASS when it did not, which made the case count differ
# between machines and reported a check that had not run as one that had. A
# suite that says "no network" should not contain a case that needs a 350 MB
# pull. What is checked here is that the job which cannot be skipped still
# exists and is still pinned to a PowerShell image.
psjob="$root/.github/workflows/ci.yml"
grep -q 'container: mcr.microsoft.com/powershell' "$psjob" \
    && ok "PowerShell: CI parses the installer inside a real pwsh container" \
    || bad "nothing parses Install-Maintainer.ps1 with a real PowerShell"
grep -q 'Parser\]::ParseFile' "$psjob" \
    && ok "PowerShell: and it parses rather than counting braces" \
    || bad "the Windows job no longer calls the parser"
# $Profile is an automatic PowerShell variable. Shadowing it in a param block is
# legal and confusing, so the parameter is named ProfileName.
grep -qE '\[string\]\$Profile\b' "$ps1" && bad "the param shadows PowerShell's automatic \$Profile" \
    || ok "PowerShell: no parameter shadows an automatic variable"
grep -q 'TASKS=' "$ps1" && ok "PowerShell reads the task list from the profile" \
    || bad "PowerShell hardcodes a task list"
for cmdlet in New-ScheduledTaskAction New-ScheduledTaskTrigger New-ScheduledTaskSettingsSet New-ScheduledTaskPrincipal Register-ScheduledTask; do
    grep -q "$cmdlet" "$ps1" && ok "PowerShell uses $cmdlet" || bad "PowerShell lost $cmdlet"
done
grep -q 'StartWhenAvailable' "$ps1" && ok "PowerShell sets StartWhenAvailable (the Persistent=true analogue)" || bad "missed-run catch-up not configured"
grep -q 'RunLevel Limited' "$ps1" && ok "PowerShell task runs unelevated" || bad "PowerShell task may run elevated"

echo "== every command a person can type is documented =="
# Six subcommands had accumulated with no mention in any doc, because nothing
# compared the two. A command nobody can find is a command nobody uses, and the
# drift is silent in both directions: a doc naming a command that was renamed
# out of existence is the defect in docs/lessons.md 12.
if sc_case 1; then
CLI_TABLE="$(cli_table)" python3 - "$root" <<'PYEOF'
import os, pathlib, re, sys
root = pathlib.Path(sys.argv[1])
src = (root / "bin/maintainer").read_text()
table = [ln.split() for ln in os.environ["CLI_TABLE"].splitlines()]
internal = {v for v, i in table if i == "1"}
cmds = {v for v, _ in table}
for f, pat in (("bin/maintainer-merge", r'^\s+(receipt|verify|merge|update|show)\)'),
               ("bin/maintainer-repo", r'^\s+(prune|release-check)\)')):
    tool = f.split("/")[-1]
    for m in re.findall(pat, (root / f).read_text(), re.M):
        cmds.add(f"{tool} {m}")
docs = "\n".join(p.read_text() for p in
                  [root / "README.md", root / "CONTRIBUTING.md"] + list((root / "docs").rglob("*.md")))
# Whole words: as a substring, `maintainer offer` was "documented" by every
# mention of `maintainer offers`, and shipped with no entry of its own.
missing = sorted(c for c in cmds - internal
                 if not re.search(r"\b" + re.escape(c if " " in c else f"maintainer {c}") + r"\b", docs))
# An internal command must not be advertised where a person is told what to
# type. Narrative prose legitimately discusses internals: docs/lessons.md
# explains what `maintainer start` computes, and the first version of this check
# read that as advertising. Fourth time today a check over prose over-matched;
# the fix is always to say which prose is in scope.
reference = "\n".join(p.read_text() for p in (root / "README.md", root / "docs/cli.md"))
told_to_type = reference.split("## Called by run.sh")[0]
leaked = sorted(c for c in internal if f"`maintainer {c}`" in told_to_type)
errs = []
if missing:
    errs.append(f"undocumented: {missing}")
if leaked:
    errs.append(f"internal commands presented as user-facing: {leaked}")
sys.exit("; ".join(errs) if errs else 0)
PYEOF
[ $? = 0 ] && ok "every non-internal subcommand appears in the docs" \
    || bad "a command exists that no document mentions"
sc_end; fi
[ -f "$root/docs/cli.md" ] && ok "there is a command reference" || bad "no docs/cli.md"
grep -q 'cli.md' "$root/docs/SUMMARY.md" \
    && ok "and the book links it, so it is reachable" \
    || bad "docs/cli.md exists and the site does not link it"

echo "== cursor has a wall now, and is still restricted until it is proven =="
# The backend used to say Cursor has "no per-command deny list" and restrict
# itself on that basis. The claim was stale: the documented configuration
# carries permissions.allow/deny with Shell(), Read() and Write() patterns, and
# CURSOR_CONFIG_DIR points the CLI at a directory we choose. The wall is
# generated from the same deny.json as every other backend.
#
# The restriction stays until somebody watches cursor-agent refuse a denied
# command. A guard lifted because a configuration ought to work is the shape
# docs/lessons.md keeps recording.
cb="$root/lib/backends/cursor.sh"
if sc_case 3; then
grep -q 'backend_allowed_tasks' "$cb" && ok "cursor is still restricted while the wall is unproven" \
    || bad "cursor dropped its restriction; has the containment probe actually run?"
grep -q 'CURSOR_CONFIG_DIR' "$cb" && ok "and it points cursor at a per-profile wall" \
    || bad "cursor takes no config directory, so its wall cannot be per-profile"
grep -v '^[[:space:]]*#' "$cb" | grep -q 'no per-command deny list' \
    && bad "the stale claim is back in the cursor backend" \
    || ok "the stale no-deny-list claim is gone"
sc_end; fi
# Rendered into a scratch directory from the repository, never read out of the
# deployed tree. The first version read $HOME/.local/share/maintainer/..., which
# exists on the machine that wrote it and on no CI runner: it passed here and
# failed the release workflow, which is a test measuring this laptop rather than
# the repository. Same shape as the deny-wall cases above, which render into
# $stub_dir with a fake home for exactly this reason.
for prof in magent sysknife; do
    cw="$stub_dir/cursorwall-$prof"; rm -rf "$cw"; mkdir -p "$cw"
    cp "$root/profiles/$prof/deny.json" "$cw/deny.json"
    python3 "$root/scripts/render-settings.py" "$cw" /home/fakeuser >/dev/null 2>&1
    f="$cw/cursor/cli-config.json"
    [ -f "$f" ] && ok "$prof renders a cursor wall" || bad "$prof has no cursor wall"
    python3 - "$f" <<'PYEOF'
import json, sys
d = json.load(open(sys.argv[1]))["permissions"]["deny"]
# Shell(git) would deny `git log` and `git diff`, which a review is made of. The
# bare form belongs only to a single-word verb.
bad = [x for x in ("Shell(git)", "Shell(gh)", "Shell(cargo)", "Shell(npm)") if x in d]
sys.exit(f"blanket denies that would break every run: {bad}" if bad else 0)
PYEOF
    [ $? = 0 ] && ok "$prof denies verbs, not whole commands" \
        || bad "$prof would block the work rather than the damage"
done
if sc_case 1; then
grep -q 'backend_allowed_tasks' "$root/lib/run.sh" && ok "run.sh enforces the declaration" || bad "run.sh ignores backend task restrictions"
sc_end; fi
# Only executable lines count. The file explains at length why --force is
# absent, and an earlier version of this check read those comments as usage.
if sc_case 1; then
if sed 's/#.*//' "$cb" | grep -qE '(^|[[:space:]])--(force|yolo)([[:space:]]|$)'; then
    bad "cursor backend passes --force (full write access)"
else
    ok "cursor backend never passes --force outside comments"
fi
sc_end; fi

echo "== the receipt cannot be asserted by the agent, only observed =="
if grep -q 'Bash(maintainer-merge receipt:\*)' "$s"; then
    ok "hand-written receipts are denied to the agent"
else
    bad "the agent can write its own receipt; the merge gate proves nothing"
fi
if sc_case 1; then
if grep -q 'cmd_verify' "$mg" && grep -q 'THE GUARD DOES NOT BITE' "$mg"; then
    ok "verify runs the mutation and refuses a guard that stays green"
else
    bad "there is no observed-verification path"
fi
sc_end; fi
if sc_case 1; then
if grep -q '"kind": "observed"' "$mg" || grep -q 'observed' "$mg"; then
    ok "receipts record whether they were observed or merely asserted"
else
    bad "receipts do not distinguish observation from assertion"
fi
sc_end; fi

echo "== absolute-path spellings of every blocked verb are denied =="
# MEASURED: /usr/bin/touch evaded a rule written for touch. The matcher keys on
# the command as written, so each verb needs its absolute forms too.
for verb in "git push" "git tag" "gh pr merge" "cargo publish" "npm publish"; do
    if grep -q "Bash(/usr/bin/$verb:\*)" "$s"; then
        ok "absolute form denied: /usr/bin/$verb"
    else
        bad "absolute form NOT denied: /usr/bin/$verb"
    fi
done
grep -q 'Bash(git -C:\*)' "$s" && ok "git -C (flags before the verb) is denied" || bad "git -C evades the push/tag rules"

echo "== install is idempotent (a second run must replace, not accumulate) =="
ih="$stub_dir/inst"; mkdir -p "$ih"
HOME="$ih" "$root/install.sh" >/dev/null 2>&1
HOME="$ih" "$root/install.sh" >/dev/null 2>&1
nested=$(find "$ih/.local/share/maintainer" -type d \( -name profiles -o -name backends \) 2>/dev/null | wc -l)
if [ "$nested" = "2" ]; then ok "two installs leave one profiles/ and one backends/"; else bad "two installs left $nested such dirs (cp -r nested them)"; fi
want=$(python3 -c "import json;print(len(json.load(open('$s'))['permissions']['deny']))")
got=$(python3 -c "import json;print(len(json.load(open('$ih/.local/share/maintainer/profiles/sysknife/settings.json'))['permissions']['deny']))" 2>/dev/null)
if [ "$got" = "$want" ]; then ok "reinstall refreshes the deny wall ($got rules)"; else bad "deployed deny wall has $got rules, the repo has $want"; fi
if [ -d "$ih/.local/share/maintainer/profiles/_template" ]; then bad "the scaffolding template was deployed as a runnable profile"; else ok "_template is not deployed"; fi
# Every verify.d suite of the magent profile reaches the deployment, byte for
# byte: the merge gate reads suites from there, and a suite that stayed behind
# would make `verify ... characterization` fail with "no suite" on the machine
# that runs the timers.
inst_missing=""
for _vf in "$root"/profiles/magent/verify.d/*.sh; do
    cmp -s "$_vf" "$ih/.local/share/maintainer/profiles/magent/verify.d/$(basename "$_vf")" \
        || inst_missing="$inst_missing $(basename "$_vf")"
done
[ -f "$root/profiles/magent/verify.d/characterization.sh" ] && [ -z "$inst_missing" ] \
    && ok "install deploys every magent verify.d suite, the characterization suite included" \
    || bad "install did not deploy these magent suites intact:${inst_missing:- characterization.sh is not in the tree}"
# Every deployed entry point must be executable. A clean install once left
# run-instance.sh unexecutable because chmod ran before the platform dispatch
# that copies it, and systemd would have failed with a permission error.
for f in run.sh run-instance.sh; do
    d="$ih/.local/share/maintainer/$f"
    if [ ! -e "$d" ]; then bad "$f was not deployed"; elif [ -x "$d" ]; then ok "$f deployed executable"; else bad "$f deployed NOT executable"; fi
done
for f in maintainer maintainer-merge; do
    d="$ih/.local/bin/$f"
    if [ -x "$d" ]; then ok "$f deployed executable"; else bad "$f missing or not executable"; fi
done
# Every script the repository ships must reach the deployment. transcript.py did
# not, and the backend's fallback hid it: runs kept working and stopped
# recording which commands the agent ran.
for f in "$root"/scripts/*.py; do
    n="$(basename "$f")"
    [ -f "$ih/.local/share/maintainer/scripts/$n" ] && ok "scripts/$n deployed" \
        || bad "scripts/$n was never deployed"
done
# The files run.sh LOADS, as opposed to the ones it calls. profile.sh was the
# gap: run.sh named maintainer_scrub_secret, never sourced the file defining it,
# and the exit-path token scrub silently did nothing for the whole life of the
# trap. Nothing here compared what run.sh loads against what install.sh ships.
for f in profile.sh preamble-core.md prose-style.md failure-shapes.json defaults.env; do
    [ -e "$ih/.local/share/maintainer/$f" ] && ok "$f deployed, and run.sh loads it" \
        || bad "run.sh loads $f and install.sh never deployed it"
done

echo "== opencode: default-deny closes the spelling hole a denylist cannot =="
oc="$root/profiles/sysknife/opencode.json"
python3 - "$oc" <<'PYEOF' && ok "opencode config decides all 17 probe commands correctly" || bad "opencode permission config lets something through"
import json, fnmatch, collections, sys
b = json.load(open(sys.argv[1]), object_pairs_hook=collections.OrderedDict)["permission"]["bash"]
def decide(c):
    v = b.get("*", "deny")
    for pat, val in b.items():
        if pat != "*" and fnmatch.fnmatch(c, pat): v = val
    return v
must_deny = ["cargo publish", "cd /x && cargo publish", "/usr/bin/cargo publish",
             "git push origin main", "/usr/bin/git push origin main", "gh pr merge 1",
             "curl http://x", "cat ~/.ssh/id_ed25519", "maintainer-merge receipt 1 a b",
             "gh pr update-branch 5", "gh api repos/o/r/pulls/5/update-branch -X PUT"]
must_allow = ["git status", "cargo test -p x", "maintainer-merge verify 1 a b c",
              "maintainer-merge update 5",
              "gh pr review 1 --approve", "podman run --rm x"]
bad = [c for c in must_deny if decide(c) != "deny"] + [c for c in must_allow if decide(c) != "allow"]
if bad: print("  wrong verdicts:", bad); sys.exit(1)
sys.exit(0)
PYEOF
grep -q '"\*": "deny"' "$oc" && ok "opencode config is default-deny" || bad "opencode config is not default-deny"
if sc_case 1; then
grep -q 'default-deny' "$root/lib/backends/opencode.sh" && ok "opencode backend explains its posture" || bad "opencode backend does not state its containment"
sc_end; fi


echo "== a denied verb is denied when written with a relative path =="
# Issue #27. The Claude matcher keys on the command AS WRITTEN, which is the
# premise render-settings.py's own docstring records. It spelled "", seven FHS
# directories, six home directories three ways each, and whatever `which`
# resolved. It never spelled `./`.
#
# That matters most for the one verb nobody types any other way. `install.sh
# --uninstall` is in deny.json because a magent run once ran it against this
# machine and turned off every timer, including another project's. `install.sh`
# sits at a repository root and is never on PATH, so `./install.sh --uninstall`
# is the only spelling anybody uses, and all 31 rendered rules missed it.
rp="$stub_dir/relpath"; rm -rf "$rp"; mkdir -p "$rp"
cat > "$rp/deny.json" <<'DENYEOF'
{"spelled_everywhere": ["install.sh --uninstall", "curl"], "bare_exact": [], "credential_paths": ["~/.ssh/**"]}
DENYEOF
python3 "$root/scripts/render-settings.py" "$rp" /home/fakeuser >/dev/null 2>&1 \
    && ok "the probe wall renders" || bad "render-settings.py failed on the probe spec"
relrules() { python3 -c "
import json,sys
print('\n'.join(json.load(open(sys.argv[1]))['permissions']['deny']))" "$rp/settings.json"; }
rules="$(relrules)"
grep -qxF 'Bash(./install.sh --uninstall:*)' <<<"$rules" \
    && ok "the relative spelling of a script verb is denied" \
    || bad "./install.sh --uninstall walks past the wall that names it"
grep -qxF 'Bash(./curl:*)' <<<"$rules" \
    && ok "and the relative spelling of an ordinary verb is denied" \
    || bad "a copied binary run as ./curl is not denied"
# The interpreter is a second way past a rule keyed on the file name.
grep -qxF 'Bash(bash install.sh --uninstall:*)' <<<"$rules" \
    && ok "a .sh verb is denied when handed to bash" \
    || bad "bash install.sh --uninstall walks past"
grep -qxF 'Bash(sh ./install.sh --uninstall:*)' <<<"$rules" \
    && ok "and when handed to sh with a relative path" \
    || bad "sh ./install.sh --uninstall walks past"
# The twin: an interpreter prefix belongs only on scripts. Spelling `bash curl`
# would be noise, and noise in a wall is how nobody reads it.
grep -qxF 'Bash(bash curl:*)' <<<"$rules" \
    && bad "a non-script verb got an interpreter spelling it can never be run with" \
    || ok "a non-script verb gets no interpreter spelling"
# And the live wall on this machine carries it, since that is the one that runs.
grep -q 'install.sh --uninstall' "$root/profiles/magent/deny.json" \
    && ok "the magent profile still denies the uninstall verb" \
    || bad "the verb this case exists for is no longer in deny.json"


echo "== POST=off selects the rehearsal wall by asking the backend, not by guessing filenames =="
# Issue #30. run.sh chose the rehearsal wall from a hardcoded list of two
# filenames, settings-rehearsal.json and opencode-rehearsal.json. The cursor
# backend reads CURSOR_CONFIG_DIR through MAINTAINER_CURSOR_DIR, which nothing
# in the tree ever set, so a POST=off run on that backend was handed
# profiles/<p>/cursor, the LIVE wall, while cursor-rehearsal sat rendered and
# unread beside it. The difference is every verb that reaches a person.
#
# `backend_rehearsal` existed and looked like the gate, but only its EXISTENCE
# was ever tested; the value it returns is read nowhere. A backend now says
# where its own rehearsal wall is, because the backend is the only thing that
# knows which variable it reads.
for b in claude opencode cursor; do
    bf="$root/lib/backends/$b.sh"
    if sc_case 1; then
    if grep -q 'backend_rehearsal()' "$bf" && ! grep -q 'backend_rehearsal_wall()' "$bf"; then
        bad "$b claims it can honour POST=off and cannot say where its rehearsal wall is"
    else
        ok "$b says where its rehearsal wall is"
    fi
    sc_end; fi
done
# run.sh must not carry the enumeration that went stale.
# Comments stripped first. The paragraph explaining why the enumeration is gone
# names the files it used to test for, and matching that is the same over-match
# this suite already records for `jq`.
if sc_case 1; then
if grep -v '^[[:space:]]*#' "$root/lib/run.sh" | grep -q 'opencode-rehearsal.json'; then
    bad "run.sh still hardcodes a backend's rehearsal filename"
else
    ok "run.sh names no backend's rehearsal file"
fi
sc_end; fi
if sc_case 1; then
grep -q 'backend_rehearsal_wall' "$root/lib/run.sh" \
    && ok "run.sh asks the backend for it" \
    || bad "nothing asks the backend where its rehearsal wall is"
sc_end; fi
# Driven, per backend, against a scratch profile directory. The function must
# export the variable ITS backend reads and refuse when the wall is absent.
rw="$stub_dir/rehearsalwall"; rm -rf "$rw"; mkdir -p "$rw"
rw_probe() {  # $1 backend, $2 expected env var, $3 what to create
    ( rm -rf "$rw"; mkdir -p "$rw"
      case "$3" in
          dir)  mkdir -p "$rw/cursor-rehearsal"; : > "$rw/cursor-rehearsal/cli-config.json" ;;
          file) : > "$rw/$4" ;;
          none) : ;;
      esac
      # shellcheck disable=SC2034  # read by the backend file sourced on the next line
      PROFILE_DIR="$rw"
      # shellcheck disable=SC1090
      . "$root/lib/backends/$1.sh"
      if backend_rehearsal_wall; then
          printf 'OK %s\n' "${!2:-<unset>}"
      else
          printf 'REFUSED\n'
      fi )
}
if sc_case 5; then
printf '%s' "$(rw_probe claude MAINTAINER_SETTINGS file settings-rehearsal.json)" \
    | grep -q 'OK .*settings-rehearsal.json' \
    && ok "claude points MAINTAINER_SETTINGS at its rehearsal wall" \
    || bad "claude does not select its rehearsal wall"
printf '%s' "$(rw_probe claude MAINTAINER_SETTINGS none)" | grep -q REFUSED \
    && ok "and refuses when that wall is not rendered" \
    || bad "claude accepted a rehearsal with no wall on disk"
printf '%s' "$(rw_probe opencode MAINTAINER_SETTINGS file opencode-rehearsal.json)" \
    | grep -q 'OK .*opencode-rehearsal.json' \
    && ok "opencode points MAINTAINER_SETTINGS at its rehearsal config" \
    || bad "opencode does not select its rehearsal config"
# The one the issue is about.
printf '%s' "$(rw_probe cursor MAINTAINER_CURSOR_DIR dir)" \
    | grep -q 'OK .*cursor-rehearsal' \
    && ok "cursor points MAINTAINER_CURSOR_DIR at cursor-rehearsal" \
    || bad "POST=off on cursor still hands the agent the live wall"
printf '%s' "$(rw_probe cursor MAINTAINER_CURSOR_DIR none)" | grep -q REFUSED \
    && ok "and refuses when the rehearsal directory is not rendered" \
    || bad "cursor accepted a rehearsal with no wall on disk"
sc_end; fi
# The rehearsal wall has to be the STRICTER one, or selecting it buys nothing.
# Measured from the rendered pair rather than asserted.
cw2="$stub_dir/cursorwalls"; rm -rf "$cw2"; mkdir -p "$cw2"
cp "$root/profiles/magent/deny.json" "$cw2/deny.json"
python3 "$root/scripts/render-settings.py" "$cw2" /home/fakeuser >/dev/null 2>&1
if [ -f "$cw2/cursor/cli-config.json" ] && [ -f "$cw2/cursor-rehearsal/cli-config.json" ]; then
    n_live=$(python3 -c "import json,sys;d=json.load(open(sys.argv[1]));print(len(json.dumps(d)))" "$cw2/cursor/cli-config.json")
    n_reh=$(python3 -c "import json,sys;d=json.load(open(sys.argv[1]));print(len(json.dumps(d)))" "$cw2/cursor-rehearsal/cli-config.json")
    [ "$n_reh" -gt "$n_live" ] \
        && ok "the cursor rehearsal wall denies strictly more than the live one" \
        || bad "the cursor rehearsal wall is not stricter, so selecting it changes nothing"
else
    bad "render-settings did not write both cursor walls"
fi

echo "== the universal cron fallback =="
cr="$root/platform/posix/install-cron.sh"
[ -x "$cr" ] && ok "cron installer present and executable" || bad "cron installer missing"
if sc_case 2; then
grep -q 'PATH=' "$cr" && ok "cron entries pin PATH (cron gives almost none)" || bad "cron entries do not pin PATH"
grep -q 'awk -v m=' "$cr" && ok "cron install strips its old block (idempotent)" || bad "cron install would accumulate entries"
sc_end; fi
# Cron output routing is in its own file so it can run alone (the full suite is
# slower than a single-case gate allows). It reads the tree under test from MX_TREE.
if MX_TREE="$root" bash "$suite_root/tests/cron-output.sh"; then ok "cron output routing (tests/cron-output.sh)"; else bad "cron output routing (tests/cron-output.sh)"; fi
grep -qF '[ "$fail" -eq 0 ]' "$suite_root/tests/cron-output.sh" && ok "cron output routing ends by exiting with its failure count" || bad "cron output routing no longer exits non-zero when a case fails"
if MX_TREE="$root" bash "$suite_root/tests/install-cron-crontab-errors.sh" >/dev/null 2>&1; then
    ok "cron install keeps the crontab when crontab -l fails, and installs when there is none"
else
    bad "cron install overwrites the crontab when crontab -l fails (run tests/install-cron-crontab-errors.sh)"
fi

echo "== doctor executes rather than inspects =="
d="$root/bin/maintainer-doctor"
[ -x "$d" ] && ok "doctor present and executable" || bad "doctor missing"
if sc_case 3; then
grep -q 'podman run' "$d" && ok "doctor actually starts a container" || bad "doctor only checks that podman exists"
grep -q 'gh api user' "$d" && ok "doctor resolves the real gh identity" || bad "doctor does not check identity"
grep -q 'fix:' "$d" && ok "doctor prints a fix, not just a symptom" || bad "doctor reports problems without remedies"
sc_end; fi
if PATH="$stub_dir:$PATH" HOME="$stub_dir/nothing" bash "$d" --quick >/dev/null 2>&1; then
    bad "doctor passed against an empty home; it inspects nothing"
else
    ok "doctor fails against an empty home"
fi

echo "== the timers run what was installed, not what is in the tree =="
# The agent was edited, merged and left undeployed several times in one session,
# so unattended runs kept executing old code while the repository said the bug
# was fixed. Nothing surfaced the gap, which made it depend on somebody
# remembering at the end of a long day.
dh="$stub_dir/drifthome"; mkdir -p "$dh/.local/share/maintainer"
dsrc="$stub_dir/driftsrc"; git init -q -b main "$dsrc"
git -C "$dsrc" config user.email t@t; git -C "$dsrc" config user.name t
printf 'one\n' > "$dsrc/f"; git -C "$dsrc" add -A >/dev/null; git -C "$dsrc" commit -qm one
old_sha="$(git -C "$dsrc" rev-parse --short HEAD)"
printf 'two\n' > "$dsrc/f"; git -C "$dsrc" add -A >/dev/null; git -C "$dsrc" commit -qm two
printf 'version=v0.1.0\ncommit=%s\ninstalled_from=%s\n' "$old_sha" "$dsrc" \
    > "$dh/.local/share/maintainer/VERSION"
dout="$(PATH="$stub_dir:$PATH" HOME="$dh" bash "$d" --quick 2>&1 || true)"
grep -q 'commit(s) behind' <<<"$dout" \
    && ok "doctor reports deployed code that is behind the tree it came from" \
    || bad "a stale deployment is invisible; timers keep running old code"
grep -q 'install.sh' <<<"$dout" \
    && ok "and it names the command that fixes it" \
    || bad "the drift report gives no remedy"
# Deployed AT head, clean tree: no complaint.
printf 'version=v0.1.0\ncommit=%s\ninstalled_from=%s\n' "$(git -C "$dsrc" rev-parse --short HEAD)" "$dsrc" \
    > "$dh/.local/share/maintainer/VERSION"
PATH="$stub_dir:$PATH" HOME="$dh" bash "$d" --quick 2>&1 | grep -q 'commit(s) behind' \
    && bad "doctor calls an up-to-date deployment stale" \
    || ok "a deployment at HEAD is not reported as drifted"

echo "== a task whose last attempt failed is reported, not left in a log =="
# The 09:13 sysknife review fired into a nightly network outage on four
# consecutive days (2026-09-04 through 09-07) and exited 75 every time. The
# only trace was a line in alerts.log that nobody reads, so the morning pass
# had never once run and nothing said so. `finish` promotes last-<task>.json
# only on success, so "the newest failure is newer than the newest success" is
# exactly the condition, and it needs no new bookkeeping.
fh="$stub_dir/failhome"; mkdir -p "$fh/.local/share/maintainer/profiles/p"
fst="$fh/state"; mkdir -p "$fst/state" "$fst/runs"
printf 'PROFILE_NAME=p\nTASKS="review issues"\nSTATE_DIR=%s\nREPO_PATH=%s\n' "$fst" "$fh" \
    > "$fh/.local/share/maintainer/profiles/p/profile.env"
mkfail() { # $1 task, $2 age of the success in hours, $3 age of the failure in hours
    touch -d "$2 hours ago" "$fst/state/last-$1.json"
    printf '{"task":"%s","reason":"gh could not reach GitHub in 3 tries","at":"x"}\n' "$1" \
        > "$fst/state/failed-$1.json"
    touch -d "$3 hours ago" "$fst/state/failed-$1.json"
}
# review: succeeded 40h ago, failed 2h ago -> the newest attempt failed.
mkfail review 40 2
# issues: failed 40h ago, succeeded 2h ago -> recovered, must stay quiet.
mkfail issues 2 40
touch -d "2 hours ago" "$fst/state/last-issues.json"
fout="$(PATH="$stub_dir:$PATH" HOME="$fh" MAINTAINER_PROFILE=p bash "$d" --quick 2>&1 || true)"
grep -q "review.*failed" <<<"$fout" \
    && ok "doctor names a task whose newest attempt failed" \
    || bad "a task that has not succeeded since its last failure is invisible"
grep -qE 'FAIL.*review' <<<"$fout" \
    && ok "and it is a failure, not a note" \
    || bad "a dead task is reported at a severity nobody acts on"
grep -q "issues" <<<"$fout" \
    && bad "a task that recovered is still reported as failed" \
    || ok "a task that succeeded after its failure is not reported"
# The reason the run recorded must reach the report, or the reader goes hunting
# for a revoked token during a network outage.
grep -q 'could not reach GitHub' <<<"$fout" \
    && ok "the recorded reason is carried into the report" \
    || bad "doctor says a task failed and not why"
# A failure that follows a success on a task still inside its cadence is a
# warning, not a failure. On 2026-09-07 a superseded duplicate `issues` run died
# one second AFTER the real run promoted the stamp, and the check called the
# task dead while it had completed two hours earlier. The question doctor should
# answer is "has this task stopped", and the answer is no while its last success
# is younger than its own MIN_HOURS.
touch -d "40 hours ago" "$fst/state/last-review.json"
printf 'PROFILE_NAME=p\nTASKS="review issues"\nSTATE_DIR=%s\nREPO_PATH=%s\nMIN_HOURS_review=100\n' "$fst" "$fh" \
    > "$fh/.local/share/maintainer/profiles/p/profile.env"
cout="$(PATH="$stub_dir:$PATH" HOME="$fh" MAINTAINER_PROFILE=p bash "$d" --quick 2>&1 || true)"
grep -qE 'warn.*review.*failed' <<<"$cout" \
    && ok "a failed attempt on a task still inside its cadence is a warning" \
    || bad "a task that completed inside its own interval is called dead"
grep -qE 'FAIL.*review' <<<"$cout" \
    && bad "a superseded attempt turns the whole report red" \
    || ok "and it does not turn the report red"
# Overdue AND its last attempt failed is the original emergency, still red.
printf 'PROFILE_NAME=p\nTASKS="review issues"\nSTATE_DIR=%s\nREPO_PATH=%s\nMIN_HOURS_review=6\n' "$fst" "$fh" \
    > "$fh/.local/share/maintainer/profiles/p/profile.env"
oout="$(PATH="$stub_dir:$PATH" HOME="$fh" MAINTAINER_PROFILE=p bash "$d" --quick 2>&1 || true)"
grep -qE 'FAIL.*review' <<<"$oout" \
    && ok "a task that is overdue and whose last attempt failed is still a failure" \
    || bad "the four-day outage this check was written for would not be reported"
printf 'PROFILE_NAME=p\nTASKS="review issues"\nSTATE_DIR=%s\nREPO_PATH=%s\n' "$fst" "$fh" \
    > "$fh/.local/share/maintainer/profiles/p/profile.env"
touch -d "40 hours ago" "$fst/state/last-review.json"
# Never ran at all is the same emergency: a failure with no success behind it.
rm -f "$fst/state/last-review.json"
nout="$(PATH="$stub_dir:$PATH" HOME="$fh" MAINTAINER_PROFILE=p bash "$d" --quick 2>&1 || true)"
grep -qE 'FAIL.*review' <<<"$nout" \
    && ok "a task that has never succeeded is reported too" \
    || bad "a failure with no prior success reads as healthy"
# And a clean profile stays clean: no failure files, no complaint.
rm -f "$fst"/state/failed-*.json
touch "$fst/state/last-review.json"
PATH="$stub_dir:$PATH" HOME="$fh" MAINTAINER_PROFILE=p bash "$d" --quick 2>&1 \
    | grep -qE 'FAIL.*(review|issues).*failed' \
    && bad "doctor invents a failure for a profile that has none" \
    || ok "no failure files, no failure report"

echo "== a verify suite's image must carry what its tests invoke =="
# sysknife's shell suite ran in bash:5, which has no python3. When sysknife#386
# made tests/e2e/story-metadata.test.sh drive check_evidence_claims.py, the
# clean run started failing and NO receipt was earnable for that suite: the
# merge gate had stopped working and reported it as the pull request failing its
# own test. Nothing connected the image to what the suite runs inside it.
for _vs in "$root"/profiles/*/verify.d/*.sh; do
    [ -e "$_vs" ] || continue
    _vn="$(basename "$(dirname "$(dirname "$_vs")")")/$(basename "$_vs" .sh)"
    # shellcheck disable=SC1090  # the path is the loop variable, by design
    if (. "$_vs" >/dev/null 2>&1; declare -F suite_needs >/dev/null) \
       && [ -n "$(. "$_vs" >/dev/null 2>&1; suite_needs)" ]; then
        ok "$_vn declares the binaries its image must provide"
    else
        bad "$_vn names an image and nothing says what that image has to contain"
    fi
done
# Driven against a stubbed podman, both directions, because the whole point is
# that the check must go red rather than quietly pass.
vh="$stub_dir/verifyhome"; vd="$vh/.local/share/maintainer/profiles/p"
mkdir -p "$vd/verify.d" "$vh/.config/systemd/user"
printf 'PROFILE_NAME=p\nTASKS="review"\nMIN_HOURS_review=6\nSTATE_DIR=%s\nREPO_PATH=%s\n' "$vh/st" "$vh" \
    > "$vd/profile.env"
printf 'suite_image() { printf "img:one"; }\nsuite_needs() { printf "bash python3"; }\n' \
    > "$vd/verify.d/shell.sh"
vdoc() { PATH="$stub_dir:$PATH" HOME="$vh" MAINTAINER_PROFILE=p bash "$d" 2>&1; }
# The image is missing python3: the probe prints the missing name.
make_stub podman 'case "$*" in *"img:one"*) echo "python3 "; exit 0;; esac; exit 0'
vout="$(vdoc)"
grep -qE 'FAIL.*shell.*img:one' <<<"$vout" \
    && ok "a suite whose image lacks a declared binary is a failure" \
    || bad "an image that cannot run the suite reads as healthy"
grep -q 'python3' <<<"$vout" \
    && ok "and the missing binary is named" \
    || bad "the report does not say what is missing"
# The image has everything: the probe prints nothing.
#
# Captured, never piped. The suite runs under `set -o pipefail` and doctor exits
# non-zero whenever anything is red, so `vdoc | grep -q` reports doctor's exit
# rather than the match: this case passed before the check existed, and its twin
# failed with the check working. Same trap as the one already recorded at the
# maintainer-merge cases below.
make_stub podman 'exit 0'
vout="$(vdoc)"
grep -qE 'FAIL.*shell.*img:one' <<<"$vout" \
    && bad "a sufficient image is reported as failing" \
    || ok "an image that provides everything declared is not reported"
grep -qE "verify suite 'shell'.*provides" <<<"$vout" \
    && ok "and it says which image satisfied the suite" \
    || bad "a satisfied suite leaves no positive record"
# The probe itself could not run. That must not read as permission: a guard that
# answers when it could not ask is worse than no guard.
make_stub podman 'case "$*" in *"img:one"*) exit 3;; esac; exit 0'
vout="$(vdoc)"
grep -q 'could not probe' <<<"$vout" \
    && ok "a probe that could not run says so rather than passing the suite" \
    || bad "an unrunnable probe is indistinguishable from a clean one"
grep -qE "verify suite 'shell'.*provides" <<<"$vout" \
    && bad "the suite was reported provisioned on a probe that never ran" \
    || ok "and it does not claim the image provides anything"
# A suite that declares no needs is a suite nothing can check.
printf 'suite_image() { printf "img:one"; }\n' > "$vd/verify.d/shell.sh"
make_stub podman 'exit 0'
vout="$(vdoc)"
grep -qE 'FAIL.*suite_needs' <<<"$vout" \
    && ok "a suite that declares no needs is reported, not skipped" \
    || bad "an undeclared suite passes by having nothing to check"
# Remove the stub rather than leaving a permissive one. A `podman` that always
# exits 0 sitting first on PATH made the real shell-suite case below report a
# receipt it never earned, two hundred cases later and with no visible link.
rm -f "$stub_dir/podman"

echo "== every timer carries a catch-up slot the cadence gate makes free =="
# A transient failure costs a whole cycle when a task fires once. On 2026-09-07
# the sysknife issues run exited 75 into a network outage at 10:43 and its next
# tick was two days out; ci would have been three days, audit five. Five of the
# six timers had a single OnCalendar line, and the one that self-healed daily,
# sysknife-review, did so only because it happened to have a second, 12h away
# and so no use as a retry either.
#
# A second slot is free on a good day: min_gate in run.sh skips a task whose
# last-<task>.json is younger than MIN_HOURS, and `finish` writes that file only
# on success. So a redundant fire costs one "skipped:" line, and a fire after a
# failure runs for real.
#
# One checker, three invocations. Extracting the logic a second time to test it
# is how a proof ends up proving a copy: the first attempt at this test re-cut
# the script with sed, the end anchor missed, and the malformed 2149-line result
# failed to compile, which read as the check correctly rejecting a bad timer.
tcheck="$stub_dir/timercheck.py"
cat > "$tcheck" <<'PYEOF'
import glob, os, re, sys
root = sys.argv[1]
problems = []
for f in sorted(glob.glob(os.path.join(root, "platform/linux/maintainer@*.timer"))):
    inst = re.sub(r".*maintainer@(.*)\.timer$", r"\1", f)
    profile, _, task = inst.partition("-")
    env = os.path.join(root, "profiles", profile, "profile.env")
    if not os.path.exists(env):
        problems.append(inst + ": no profiles/" + profile + "/profile.env"); continue
    m = re.search(r"^MIN_HOURS_" + re.escape(task) + r"=(\d+)", open(env).read(), re.M)
    if not m:
        problems.append(inst + ": profile.env declares no MIN_HOURS_" + task); continue
    minh = int(m.group(1))
    mins = []
    for line in open(f):
        c = re.match(r"\s*OnCalendar=.*?(\d{2}):(\d{2}):\d{2}\s*$", line)
        if c:
            mins.append(int(c.group(1)) * 60 + int(c.group(2)))
    if len(mins) < 2:
        problems.append("%s: %d OnCalendar slot(s); a refused start waits %dh"
                        % (inst, len(mins), minh)); continue
    mins.sort()
    gaps = [(b - a) / 60.0 for a, b in zip(mins, mins[1:])]
    if not any(g < minh for g in gaps):
        problems.append("%s: closest slots are %gh apart, MIN_HOURS_%s=%d; "
                        "no slot can retry the one before it"
                        % (inst, min(gaps), task, minh))
if problems:
    print("  " + "\n  ".join(problems))
    sys.exit(1)
PYEOF
if python3 -c "compile(open('$tcheck').read(), 'timercheck', 'exec')" 2>/dev/null; then
    ok "the timer checker is valid python (a malformed one would reject everything)"
else
    bad "the timer checker does not compile; its verdicts mean nothing"
fi
if python3 "$tcheck" "$root"; then
    ok "every timer has a slot closer than its own MIN_HOURS, so a failure retries in the same cycle"
else
    bad "a timer fires once per cycle; one refused start loses the whole interval"
fi
# Mutation, both directions, against the same checker the real case used.
tprobe="$stub_dir/timerprobe"; mkdir -p "$tprobe/platform/linux" "$tprobe/profiles/p"
printf 'MIN_HOURS_review=6\n' > "$tprobe/profiles/p/profile.env"
printf '[Timer]\nOnCalendar=*-*-* 11:13:00\n' > "$tprobe/platform/linux/maintainer@p-review.timer"
python3 "$tcheck" "$tprobe" >/dev/null 2>&1 \
    && bad "the catch-up check passes a timer with a single slot" \
    || ok "and it fails a timer with a single slot"
printf '[Timer]\nOnCalendar=*-*-* 11:13:00\nOnCalendar=*-*-* 21:13:00\n' \
    > "$tprobe/platform/linux/maintainer@p-review.timer"
python3 "$tcheck" "$tprobe" >/dev/null 2>&1 \
    && bad "two slots 10h apart pass a 6h MIN_HOURS; that is a second run, not a retry" \
    || ok "and it fails two slots too far apart to retry each other"
printf '[Timer]\nOnCalendar=*-*-* 11:13:00\nOnCalendar=*-*-* 15:13:00\n' \
    > "$tprobe/platform/linux/maintainer@p-review.timer"
python3 "$tcheck" "$tprobe" >/dev/null 2>&1 \
    && ok "and it passes a timer whose second slot lands inside MIN_HOURS" \
    || bad "the catch-up check rejects a correctly scheduled timer"

echo "== housekeeping: prune and release-check =="
mr="$root/bin/maintainer-repo"
[ -x "$mr" ] && ok "maintainer-repo present and executable" || bad "maintainer-repo missing"
if sc_case 4; then
grep -q 'gh pr list' "$mr" && ok "prune consults open PRs before deleting a branch" || bad "prune could delete a branch that still has an open PR"
grep -q 'merge-base --is-ancestor' "$mr" && ok "prune defines merged as an ancestor of origin/main" || bad "prune uses a weaker merged test"
grep -qE 'case "\$br" in main\|master' "$mr" && ok "prune never touches main" || bad "prune does not exclude main"
# The release verdict must come from the CHANGELOG, not from commit subjects.
# An earlier version read subject lines for the word "security" and scored a
# genuine authorization fix as zero, because its subject said "gate mutating
# query actions".
if grep -q "format=%s.*grep -ciE 'security" "$mr"; then bad "release-check still guesses from commit subjects"; else ok "release-check does not guess from commit subjects"; fi
sc_end; fi
# Driven, not grepped. A real repository with a tag, an origin, and a CHANGELOG
# this test controls.
rcrepo="$stub_dir/rcrepo"; rcorigin="$stub_dir/rcorigin"
git init -q --bare "$rcorigin"
git init -q -b main "$rcrepo"
git -C "$rcrepo" config user.email t@t; git -C "$rcrepo" config user.name t
mkdir -p "$rcrepo/bin"
printf 'v1\n' > "$rcrepo/bin/tool"; printf '# Changelog\n\n## [Unreleased]\n\n## [0.1.0] - 2026-01-01\n' > "$rcrepo/CHANGELOG.md"
git -C "$rcrepo" add -A >/dev/null; git -C "$rcrepo" commit -qm base
git -C "$rcrepo" tag v0.1.0
printf 'v2\n' > "$rcrepo/bin/tool"
git -C "$rcrepo" commit -qam 'change production code'
git -C "$rcrepo" remote add origin "$rcorigin"; git -C "$rcrepo" push -q --tags origin main
rc_run() { PATH="$stub_dir:$PATH" MAINTAINER_SLUG=o/r MAINTAINER_REPO="$rcrepo" \
    MAINTAINER_STATE="$stub_dir/rcstate" MAINTAINER_ACCOUNT=t PROD_GLOBS="bin/*" \
    bash "$mr" release-check 2>&1; }
out="$(rc_run)"
grep -q 'rests on file counts alone' <<<"$out" \
    && ok "an empty Unreleased section is reported as read nothing" \
    || bad "release-check read an empty Unreleased section without saying so"
# The delimiter bug: sed prints BOTH headings, so an empty section came back as
# two lines and -z was false. The warning above never printed once in practice.
grep -q 'newest version heading' <<<"$out" \
    && ok "and it names the heading the entries were probably promoted into" \
    || bad "the warning does not say where to look instead"
grep -q 'digit:   last' <<<"$out" \
    && ok "with nothing to read it does not claim a breaking change" \
    || bad "release-check guessed a digit from an unread CHANGELOG"
# Now a populated section that removes a capability.
printf '# Changelog\n\n## [Unreleased]\n\n### Removed\n\n- A run can no longer assert a receipt.\n\n## [0.1.0] - 2026-01-01\n' \
    > "$rcrepo/CHANGELOG.md"
out="$(rc_run)"
grep -q 'RELEASE DUE' <<<"$out" \
    && ok "a removed capability makes the release due" \
    || bad "a removed capability did not make the release due"
grep -q 'digit:   middle' <<<"$out" \
    && ok "and it moves the middle digit" \
    || bad "a removed capability was scored as a patch"
grep -q 'CHANGELOG read from' <<<"$out" \
    && ok "it names which tree the CHANGELOG came from" \
    || bad "the CHANGELOG source is unstated while the counts come from origin/main"

# A removed capability that lives in CI rather than in what installs.
# sysknife's 2026-09-28 batch was five docs-and-CI pull requests. Three
# CHANGELOG entries said a gate now "refuses", so the wording heuristic scored a
# break and release-check answered RELEASE DUE, middle digit, while
# `git diff v0.22.0..main -- crates apps packages` named zero files. Cutting
# that release would have published byte-identical crates to crates.io, and a
# version there can never be replaced. The wording cannot tell a gate that
# refuses from a binary that refuses, so the report has to carry the
# shipped-surface count beside it.
mkdir -p "$rcrepo/ci"; printf 'g1\n' > "$rcrepo/ci/gate"
git -C "$rcrepo" add -A >/dev/null; git -C "$rcrepo" commit -qm 'add a gate'
git -C "$rcrepo" tag v0.2.0
printf 'g2\n' > "$rcrepo/ci/gate"
git -C "$rcrepo" commit -qam 'the gate refuses more than it used to'
git -C "$rcrepo" push -q --tags origin main
rc_run_shipped() { PATH="$stub_dir:$PATH" MAINTAINER_SLUG=o/r MAINTAINER_REPO="$rcrepo" \
    MAINTAINER_STATE="$stub_dir/rcstate" MAINTAINER_ACCOUNT=t \
    PROD_GLOBS="bin/* ci/*" SHIPPED_GLOBS="bin/*" bash "$mr" release-check 2>&1; }
out="$(rc_run_shipped)"
grep -qE 'shipped surface touched: +0' <<<"$out" \
    && ok "release-check counts the shipped surface apart from production" \
    || bad "release-check does not say how much of the diff actually ships"
grep -q 'byte-identical' <<<"$out" \
    && ok "and says a release would publish byte-identical artifacts" \
    || bad "a release with an untouched shipped surface is not flagged as such"
# Unset must be loud. A silent zero here reads as "nothing ships", which is the
# opposite of what an unconfigured profile means.
out="$(PATH="$stub_dir:$PATH" MAINTAINER_SLUG=o/r MAINTAINER_REPO="$rcrepo" \
    MAINTAINER_STATE="$stub_dir/rcstate" MAINTAINER_ACCOUNT=t PROD_GLOBS="bin/* ci/*" \
    bash "$mr" release-check 2>&1)"
grep -q 'declares no SHIPPED_GLOBS' <<<"$out" \
    && ok "an unset SHIPPED_GLOBS is reported rather than scored as zero" \
    || bad "an unset SHIPPED_GLOBS is indistinguishable from an untouched one"


