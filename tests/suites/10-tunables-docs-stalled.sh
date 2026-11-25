#!/usr/bin/env bash
# Part of the offline suite: tunable tiers, list limits, hard-coded names, help, the documentation checker, figures and stalled pull requests.
# tests/run-tests.sh sources this file in one shell, after its own preamble and after
# the parts before it, so every helper and variable it reads comes from there. It is
# sourced, never run on its own, and carries a shebang so the shell sweeps pick it up.

# The parts share one shell. These names are set by the driver or by an earlier part;
# declare only tells the linter they are read here, and assigns and changes nothing.
declare root
declare stub_dir

echo "== the python tools resolve the same three tiers =="
gch="$stub_dir/gchome"; rm -rf "$gch"; mkdir -p "$gch/.local/share/maintainer/profiles/gp" "$gch/state/logs"
: > "$gch/.local/share/maintainer/profiles/gp/profile.env"
gcdays() { env HOME="$gch" MAINTAINER_PROFILE=gp MAINTAINER_STATE="$gch/state" MAINTAINER_SLUG=o/r \
    MAINTAINER_REPO=/tmp MAINTAINER_ACCOUNT=t "$@" python3 "$root/bin/maintainer" gc --dry-run 2>&1 \
    | sed -n 's/.*older than \([0-9]*\) days.*/\1/p'; }
[ "$(gcdays)" = "$(dget RETENTION_DAYS)" ] && ok "gc's window defaults to RETENTION_DAYS in lib/defaults.env" \
    || bad "gc used '$(gcdays)' days, not the default $(dget RETENTION_DAYS)"
printf 'RETENTION_DAYS=45\n' > "$gch/.local/share/maintainer/profiles/gp/profile.env"
[ "$(gcdays)" = 45 ] && ok "profile.env beats the default for a python tool" \
    || bad "profile RETENTION_DAYS=45 was ignored: '$(gcdays)'"
[ "$(gcdays MAINTAINER_RETENTION_DAYS=12)" = 12 ] && ok "and MAINTAINER_RETENTION_DAYS beats the profile" \
    || bad "the env override was ignored: '$(gcdays MAINTAINER_RETENTION_DAYS=12)'"

# A listing that comes back exactly as long as its limit may have been cut off,
# and a roster missing people reads as "nobody else is available".
echo "== a list that fills its limit is refused, not believed =="
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
# `issue list` answers with as many rows as --limit asks for, less $LIST_SHORT.
printf '%s\n' "$*" >> "$LIST_LOG"
case "$*" in
  *'issue list'*) n=0
      while [ $# -gt 0 ]; do [ "$1" = --limit ] && n="$2"; shift; done
      n=$((n - ${LIST_SHORT:-0}))
      python3 -c 'import json,sys; print(json.dumps([{"number":i+1,"labels":[],"assignees":[]} for i in range(int(sys.argv[1]))]))' "$n" ;;
  *'pr list'*) echo '[]' ;;
  *'/comments'*) echo '[]' ;;
esac
GHEOF
chmod +x "$stub_dir/gh"
export LIST_LOG="$stub_dir/list.log"
list_offers() { : > "$LIST_LOG"
    env PATH="$stub_dir:$PATH" HOME="$gch" MAINTAINER_STATE="$gch/state" MAINTAINER_SLUG=o/r \
        MAINTAINER_REPO=/tmp MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=gp "$@" \
        python3 "$root/bin/maintainer" offers 2>&1; }
out="$(list_offers MAINTAINER_GH_LIST_LIMIT=4 LIST_SHORT=1)"; rc=$?
grep -q -- '--limit 4' "$LIST_LOG" && ok "MAINTAINER_GH_LIST_LIMIT reaches gh as --limit" \
    || bad "gh was not asked for the configured limit: $(head -2 "$LIST_LOG" | tr '\n' ' ')"
grep -q 'issue(s) free to offer' <<<"$out" && ok "a listing shorter than its limit is used" \
    || bad "a listing of limit-1 rows was refused: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-120)"
out="$(list_offers MAINTAINER_GH_LIST_LIMIT=4)"; rc=$?
[ "$rc" != 0 ] && grep -q 'may be cut off' <<<"$out" && grep -q 'MAINTAINER_GH_LIST_LIMIT' <<<"$out" \
    && ok "a listing exactly as long as its limit is refused, naming the setting to raise" \
    || bad "a full page was believed (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-120)"
rm -f "$stub_dir/gh"
# The stale window has a default of its own, declared where the others are, and
# the prompt no longer carries a second copy of it.
list_stale() { : > "$LIST_LOG"
    env PATH="$stub_dir:$PATH" HOME="$gch" MAINTAINER_STATE="$gch/state" MAINTAINER_SLUG=o/r \
        MAINTAINER_REPO=/tmp MAINTAINER_ACCOUNT=owner MAINTAINER_PROFILE=gp \
        MAINTAINER_GH_LIST_LIMIT=3 LIST_SHORT=1 python3 "$root/bin/maintainer" stale 2>&1; }
cat > "$stub_dir/gh" <<'GHEOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$LIST_LOG"
case "$*" in
  *'issue list'*) n=0
      while [ $# -gt 0 ]; do [ "$1" = --limit ] && n="$2"; shift; done
      python3 -c 'import json,sys; print(json.dumps([{"number":i+1,"labels":[],"assignees":[]} for i in range(int(sys.argv[1])-1)]))' "$n" ;;
  *) echo '[]' ;;
esac
GHEOF
chmod +x "$stub_dir/gh"
: > "$gch/.local/share/maintainer/profiles/gp/profile.env"
grep -q "older than $(dget STALE_HOLD_DAYS) days" <<<"$(list_stale)" \
    && ok "'maintainer stale' with no argument uses STALE_HOLD_DAYS" \
    || bad "stale's default window is not STALE_HOLD_DAYS ($(dget STALE_HOLD_DAYS)): $(list_stale | tr '\n' ' ' | cut -c1-100)"
printf 'STALE_HOLD_DAYS=21\n' > "$gch/.local/share/maintainer/profiles/gp/profile.env"
grep -q 'older than 21 days' <<<"$(list_stale)" \
    && ok "and a profile can change it" || bad "profile STALE_HOLD_DAYS=21 was ignored"
rm -f "$stub_dir/gh"
grep -q 'maintainer stale 9' "$root"/profiles/*/prompts/*.md \
    && bad "a prompt still hard-codes the stale window" \
    || ok "no prompt hard-codes the stale window"

# memory and jobs move together: a rustc holds about a gigabyte while it links.
python3 - "$root/profiles/sysknife/verify.d/rust.sh" <<'PYEOF' \
    && ok "the rust suite's memory (GB) covers its cargo jobs, one gigabyte each" \
    || bad "the rust suite declares fewer gigabytes than cargo jobs, which is the OOM it was raised to fix"
import re, subprocess, sys
def call(fn):
    return subprocess.run(["bash", "-c", f'. "$1"; {fn}', "_", sys.argv[1]],
                          capture_output=True, text=True, check=True).stdout.strip()
mem, jobs = call("suite_memory"), int(call("suite_jobs"))
m = re.fullmatch(r"(\d+)g", mem)
sys.exit(0 if m and int(m.group(1)) >= jobs else f"suite_memory={mem} suite_jobs={jobs}")
PYEOF

# LOCK_WAIT sits inside systemd's start budget, so it must stay below it.
tsec="$(sed -n 's/^TimeoutStartSec=//p' "$root/platform/linux/maintainer@.service")"
[ -n "$tsec" ] && [ "$(dget LOCK_WAIT)" -lt "$tsec" ] \
    && ok "LOCK_WAIT ($(dget LOCK_WAIT)s) is below the unit's TimeoutStartSec (${tsec}s)" \
    || bad "LOCK_WAIT=$(dget LOCK_WAIT) is not below TimeoutStartSec=$tsec: a waiting run would be killed by systemd first"

# The MCP verify deadline covers what the containers may legitimately use.
mcp_deadline() {  # env assignments as arguments -> the timeout maintainer_verify really runs under
    env HOME="$gch" "$@" python3 -B -I - "$root/bin/maintainer-mcp" <<'PYEOF'
import importlib.machinery, importlib.util, sys
loader = importlib.machinery.SourceFileLoader("mcp", sys.argv[1])
spec = importlib.util.spec_from_loader("mcp", loader)
m = importlib.util.module_from_spec(spec); loader.exec_module(m)
seen = {}
class P:
    returncode, stdout, stderr = 0, "", ""
def fake_run(cmd, **kw):
    seen["timeout"] = kw.get("timeout"); return P()
m.subprocess.run = fake_run
m.call_tool("maintainer_verify", {"pr": 1, "head_sha": "abcdef1", "test_filter": "f", "mutation": "s/a/b/"})
print(seen["timeout"])
PYEOF
}
: > "$gch/.local/share/maintainer/profiles/gp/profile.env"
want=$(( 2 * $(dget VERIFY_TIMEOUT) + $(dget PREFETCH_TIMEOUT) + $(dget MCP_VERIFY_SLACK) ))
[ "$(mcp_deadline)" = "$want" ] && [ "$want" -gt $(( 2 * $(dget VERIFY_TIMEOUT) + $(dget PREFETCH_TIMEOUT) )) ] \
    && ok "the MCP verify deadline is 2 x VERIFY_TIMEOUT + PREFETCH_TIMEOUT + slack, longer than a legitimate verify" \
    || bad "the MCP verify deadline is '$(mcp_deadline)', not $want"
[ "$(mcp_deadline MAINTAINER_VERIFY_TIMEOUT=100 MAINTAINER_PREFETCH_TIMEOUT=10 MAINTAINER_MCP_VERIFY_SLACK=1)" = 211 ] \
    && ok "and it follows the environment overrides of the settings it derives from" \
    || bad "the MCP deadline ignored MAINTAINER_VERIFY_TIMEOUT: $(mcp_deadline MAINTAINER_VERIFY_TIMEOUT=100 MAINTAINER_PREFETCH_TIMEOUT=10 MAINTAINER_MCP_VERIFY_SLACK=1)"
printf 'VERIFY_TIMEOUT=50\n' > "$gch/.local/share/maintainer/profiles/gp/profile.env"
[ "$(mcp_deadline MAINTAINER_PROFILE=gp)" = "$(( 100 + $(dget PREFETCH_TIMEOUT) + $(dget MCP_VERIFY_SLACK) ))" ] \
    && ok "and the profile's own VERIFY_TIMEOUT" \
    || bad "the MCP deadline ignored the profile's VERIFY_TIMEOUT: $(mcp_deadline MAINTAINER_PROFILE=gp)"

# run.sh reads LOCK_WAIT and the GitHub retry knobs from the same file. A wait
# of one second against a held lock proves the value reached flock; the default
# is never exercised here, because it is an hour.
lw="$stub_dir/lockwait"; rm -rf "$lw"; mkdir -p "$lw/state" "$lw/repo" "$lw/bin"
git -C "$lw/repo" init -q -b main 2>/dev/null
printf '{"permissions":{"deny":[]}}\n' > "$lw/settings.json"
touch -d "100 hours ago" "$lw/state/last-review.json"
printf '#!/usr/bin/env bash\nexit 0\n' > "$lw/bin/claude"; chmod +x "$lw/bin/claude"
ln -sf "$(PATH=/usr/local/bin:/usr/bin:/bin command -v flock)" "$lw/bin/flock"
printf '#!/usr/bin/env bash\nprintf "%%s\\n" "$*" >> "$LW_GH_LOG"\nexit 1\n' > "$lw/bin/gh"; chmod +x "$lw/bin/gh"
export LW_GH_LOG="$lw/gh.log"
lw_run() {  # env assignments as arguments
    env PATH="$lw/bin:$PATH" MAINTAINER_STATE_DIR="$lw/state" MAINTAINER_REPO_PATH="$lw/repo" \
        MAINTAINER_SETTINGS="$lw/settings.json" MAINTAINER_GH_BACKOFF=0 "$@" \
        timeout 60 bash "$root/lib/run.sh" sysknife review 2>&1; }
exec 8>"$lw/state/run.lock"; flock 8
out="$(lw_run MAINTAINER_LOCK_WAIT=1)"; rc=$?
flock -u 8; exec 8>&-
[ "$rc" = 75 ] && grep -q 'over 1s' <<<"$out" \
    && ok "MAINTAINER_LOCK_WAIT reaches the lock wait in run.sh" \
    || bad "run.sh ignored MAINTAINER_LOCK_WAIT=1 (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-120)"
: > "$LW_GH_LOG"; lw_run MAINTAINER_GH_TRIES=2 >/dev/null
[ "$(grep -c 'api user' "$LW_GH_LOG")" = 2 ] && ok "MAINTAINER_GH_TRIES sets how many times run.sh asks GitHub who it is" \
    || bad "run.sh asked gh $(grep -c 'api user' "$LW_GH_LOG") times with MAINTAINER_GH_TRIES=2"
: > "$LW_GH_LOG"; lw_run >/dev/null
[ "$(grep -c 'api user' "$LW_GH_LOG")" = "$(dget GH_TRIES)" ] && ok "and with nothing set it asks GH_TRIES times" \
    || bad "run.sh asked gh $(grep -c 'api user' "$LW_GH_LOG") times, not the default $(dget GH_TRIES)"

# opencode's model is a default like the others.
ocd="$stub_dir/ocm"; rm -rf "$ocd"; mkdir -p "$ocd"
make_stub opencode 'printf "%s\n" "$@" > "$OC_ARGS"'
oc_model() {  # env assignments -> the --model the backend passed
    : > "$ocd/args"; printf 'p\n' > "$ocd/prompt"
    env PATH="$stub_dir:$PATH" HOME="$ocd" OC_ARGS="$ocd/args" MAINTAINER_SETTINGS=/dev/null "$@" bash -c '
        . "$1/lib/profile.sh"; maintainer_load_defaults; maintainer_apply_overrides
        . "$1/lib/backends/opencode.sh"; backend_run "$2/prompt" opus "$2/log" "$2"' _ "$root" "$ocd" >/dev/null 2>&1
    awk 'p{print; exit} $0=="--model"{p=1}' "$ocd/args"
}
if sc_case 2; then
[ "$(oc_model)" = "$(dget OPENCODE_MODEL)" ] && ok "opencode's default model is OPENCODE_MODEL from lib/defaults.env" \
    || bad "opencode ran '$(oc_model)', not '$(dget OPENCODE_MODEL)'"
[ "$(oc_model MAINTAINER_OPENCODE_MODEL=a/b)" = "a/b" ] && ok "and MAINTAINER_OPENCODE_MODEL overrides it" \
    || bad "MAINTAINER_OPENCODE_MODEL was ignored: '$(oc_model MAINTAINER_OPENCODE_MODEL=a/b)'"
sc_end; fi
rm -f "$stub_dir/opencode"

# maintainer-doctor reads its thresholds the same way.
dh="$stub_dir/dochome"; rm -rf "$dh"
mkdir -p "$dh/.local/share/maintainer/profiles/p" "$dh/.local/bin"
printf 'REPO_SLUG="o/r"\n' > "$dh/.local/share/maintainer/profiles/p/profile.env"
created="$(date -u -d '-70 hours' +%Y-%m-%dT%H:%M:%SZ)"
make_stub gh "printf '%s\\n' \"\$*\" >> \"\$DOC_LOG\"
case \"\$*\" in
    *'run list'*) printf '%s\\n' '[{\"workflowName\":\"ci\",\"status\":\"completed\",\"conclusion\":\"failure\",\"createdAt\":\"$created\",\"url\":\"u\"}]' ;;
    *) exit 0 ;;
esac"
export DOC_LOG="$stub_dir/doc.log"
doc_run() { : > "$DOC_LOG"
    env PATH="$stub_dir:$PATH" HOME="$dh" MAINTAINER_PROFILE=p "$@" bash "$root/bin/maintainer-doctor" --quick 2>&1; }
dout="$(doc_run)"
grep -q -- "--limit $(dget DOCTOR_RUN_LIMIT) " "$DOC_LOG" && ok "doctor asks gh for DOCTOR_RUN_LIMIT runs" \
    || bad "doctor's run limit is not DOCTOR_RUN_LIMIT: $(grep 'run list' "$DOC_LOG" | head -1)"
grep -q 'FAIL.*main has been red' <<<"$dout" && ok "70h of red is a failure under the default grace" \
    || bad "70h of red was not a failure with the default RED_GRACE_HOURS"
grep -q 'FAIL.*main has been red' <<<"$(doc_run MAINTAINER_RED_GRACE_HOURS=100)" \
    && bad "MAINTAINER_RED_GRACE_HOURS=100 did not move the grace" \
    || ok "MAINTAINER_RED_GRACE_HOURS moves the grace"
printf 'REPO_SLUG="o/r"\nRED_GRACE_HOURS=100\n' > "$dh/.local/share/maintainer/profiles/p/profile.env"
grep -q 'FAIL.*main has been red' <<<"$(doc_run)" \
    && bad "profile RED_GRACE_HOURS=100 did not move the grace" \
    || ok "and so does the profile"
# The deny wall floor.
printf 'REPO_SLUG="o/r"\n' > "$dh/.local/share/maintainer/profiles/p/profile.env"
printf '{}\n' > "$dh/.local/share/maintainer/profiles/p/deny.json"
python3 -c 'import json; json.dump({"permissions":{"deny":["Bash(x)"]*5}}, open("'"$dh"'/.local/share/maintainer/profiles/p/settings.json","w"))'
grep -q 'it is stale' <<<"$(doc_run)" && ok "a 5-rule wall is under the default floor" \
    || bad "a 5-rule wall was not called stale under DENY_WALL_MIN_RULES=$(dget DENY_WALL_MIN_RULES)"
grep -q 'it is stale' <<<"$(doc_run MAINTAINER_DENY_WALL_MIN_RULES=3)" \
    && bad "MAINTAINER_DENY_WALL_MIN_RULES=3 did not lower the floor" \
    || ok "and MAINTAINER_DENY_WALL_MIN_RULES moves it"
rm -f "$stub_dir/gh"

echo "== no repository, profile or account is written into bin/ and lib/ =="
# `maintainer run` read `os.environ.get("MAINTAINER_PROFILE", "sysknife")` while
# _resolve_profile() already answered the question for the whole file. With one
# other profile deployed and the variable unset, `run review` started a real
# run of the wrong repository. A default that names a project is a guess, and
# the guess is wrong the first time somebody adopts this for a second one.
r2h="$stub_dir/runhome"; rm -rf "$r2h"
mkdir -p "$r2h/.local/share/maintainer/profiles/solo" "$r2h/.local/bin"
printf 'REPO_SLUG="o/r"\nTASKS="alpha beta"\n' > "$r2h/.local/share/maintainer/profiles/solo/profile.env"
cat > "$r2h/.local/share/maintainer/run.sh" <<'EOF'
#!/usr/bin/env bash
printf 'run.sh args: %s\n' "$*"
EOF
r2() {  # env assignments, then the maintainer arguments after --
    local -a envs=(); while [ "$1" != -- ]; do envs+=("$1"); shift; done; shift
    env -u MAINTAINER_PROFILE -u MAINTAINER_TASKS MAINTAINER_STATE_DIR="$r2h/state" \
        PATH="$stub_dir:$PATH" HOME="$r2h" "${envs[@]}" python3 "$root/bin/maintainer" "$@" 2>&1; }
out="$(r2 -- run alpha)"
[ "$out" = "run.sh args: solo alpha" ] \
    && ok "'maintainer run' acts on the one deployed profile, not on a name written in the code" \
    || bad "'maintainer run' passed '$out' to run.sh, not 'solo alpha'"
out="$(r2 MAINTAINER_PROFILE=named -- run alpha)"
[ "$out" = "run.sh args: named alpha" ] && ok "and on the profile the environment names" \
    || bad "MAINTAINER_PROFILE=named was not what run.sh received: $out"
out="$(r2 HOME="$stub_dir/nohome" -- run alpha)"; rc=$?
[ "$rc" != 0 ] && ! grep -q 'run.sh args' <<<"$out" && grep -qi 'no profile' <<<"$out" \
    && ok "with no profile deployed it refuses instead of running a default one" \
    || bad "with no profile deployed 'run' went ahead (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-100)"

# A profile that declares no TASKS runs nothing. It used to inherit another
# project's four task names.
printf 'REPO_SLUG="o/r"\n' > "$r2h/.local/share/maintainer/profiles/solo/profile.env"
out="$(r2 -- start review)"; rc=$?
[ "$rc" != 0 ] && grep -q 'declares no TASKS' <<<"$out" \
    && ok "a profile with no TASKS refuses to start a task, saying so" \
    || bad "a profile with no TASKS still started 'review' (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-100)"
out="$(r2 -- status)"
grep -q 'declares no TASKS' <<<"$out" && ! grep -qE '^(review|issues|ci|audit) ' <<<"$out" \
    && ok "and 'status' says it declares none instead of listing another project's tasks" \
    || bad "status listed tasks for a profile that declares none: $(grep -E '^(review|issues|ci|audit) ' <<<"$out" | head -2 | tr '\n' ' ')"

# The three installers took `sysknife` as the profile when told nothing.
echo "== the installers do not guess a profile =="
ih2="$stub_dir/instprof"; rm -rf "$ih2"; mkdir -p "$ih2/.local/share/maintainer/profiles/sysknife" "$ih2/.local/bin"
printf 'TASKS="review"\nSTATE_DIR="%s/state"\n' "$ih2" > "$ih2/.local/share/maintainer/profiles/sysknife/profile.env"
printf '#!/usr/bin/env bash\nexit 0\n' > "$ih2/.local/share/maintainer/run.sh"; chmod +x "$ih2/.local/share/maintainer/run.sh"
make_stub crontab 'echo "crontab called: $*" >> "$CRON_LOG"; exit 0'
make_stub launchctl 'echo "launchctl called: $*" >> "$CRON_LOG"; exit 0'
export CRON_LOG="$stub_dir/cron.log"; : > "$CRON_LOG"
for inst in platform/posix/install-cron.sh platform/macos/install-launchd.sh; do
    out="$(env -u MAINTAINER_PROFILE PATH="$stub_dir:$PATH" HOME="$ih2" bash "$root/$inst" 2>&1)"; rc=$?
    [ "$rc" != 0 ] && grep -qi 'profile name' <<<"$out" && [ ! -s "$CRON_LOG" ] \
        && ok "$(basename "$inst") refuses to run without a profile name, and registers nothing" \
        || bad "$(basename "$inst") ran without being told a profile (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-110)"
done
rm -f "$stub_dir/crontab" "$stub_dir/launchctl"
# Uninstall used to lean on that default, so with two profiles deployed it
# stripped one cron block and left the other. It removes each deployed profile's.
uh2="$stub_dir/unin2"; rm -rf "$uh2"; mkdir -p "$uh2"
HOME="$uh2" "$root/install.sh" >/dev/null 2>&1
make_stub crontab 'case "$1" in -l) exit 0 ;; *) echo "removed for ${MAINTAINER_PROFILE:-NONE}" >> "$CRON_LOG"; cat >/dev/null ;; esac'
# A systemctl stub is not optional here: without one, uninstall reads the REAL
# user timers and, with MAINTAINER_UNINSTALL_YES=1, would disable them (the
# 2026-09-04 incident the guard in install.sh exists for).
make_stub systemctl 'exit 0'
: > "$CRON_LOG"
PATH="$stub_dir:$PATH" HOME="$uh2" MAINTAINER_UNINSTALL_YES=1 "$root/install.sh" --uninstall >/dev/null 2>&1
rm -f "$stub_dir/systemctl"
[ "$(sort "$CRON_LOG" | paste -sd,)" = "removed for magent,removed for sysknife" ] \
    && ok "uninstall strips the cron block of every deployed profile, by name" \
    || bad "uninstall's cron removal did not cover each profile: $(paste -sd, "$CRON_LOG")"
rm -f "$stub_dir/crontab"
grep -qE "ProfileName *= *'" "$root/platform/windows/Install-Maintainer.ps1" \
    && bad "Install-Maintainer.ps1 still defaults ProfileName to a project" \
    || ok "Install-Maintainer.ps1 gives ProfileName no default"
grep -B1 -E '\[string\]\$ProfileName' "$root/platform/windows/Install-Maintainer.ps1" | grep -q 'Mandatory' \
    && ok "and requires it" || bad "Install-Maintainer.ps1 does not require ProfileName"

# The prompt told the agent to work in the maintainer's own dev tree, while the
# profile's REPO_PATH is a checkout this profile owns and run.sh cd's into.
grep -rq 'Desktop/lacs' "$root"/profiles/*/prompts \
    && bad "a prompt still names a path on the author's laptop" \
    || ok "no prompt names a working directory on the author's laptop"
grep -q 'MAINTAINER_REPO' "$root/profiles/sysknife/prompts/review.md" \
    && ok "the review prompt names the profile's checkout by variable" \
    || bad "the review prompt does not say where the checkout is"
[ "$(head -3 "$root/bin/maintainer" | grep -ci 'sysknife')" = 0 ] \
    && ok "bin/maintainer's own description names no project" \
    || bad "bin/maintainer's docstring still describes one project"

# The README's claim, held by a checker. Executable lines only: comments cite
# the incidents that taught each rule, and those name the repositories.
chk="$root/scripts/check_drift.py"
if sc_case 5; then
python3 "$chk" --root "$root" --only repo-names >"$stub_dir/chk.out" 2>&1 \
    && ok "no executable line in bin/ or lib/ names a deployed profile, repository or account" \
    || bad "bin/ or lib/ carries a name: $(head -3 "$stub_dir/chk.out" | tr '\n' ' ' | cut -c1-200)"
# The checker must bite: plant one name in code and one in a comment.
cm="$stub_dir/driftroot"; rm -rf "$cm"; mkdir -p "$cm"; cp -r "$root/bin" "$root/lib" "$root/profiles" "$cm/"
printf '# sysknife#1 is cited here, in a comment, which is allowed\n' >> "$cm/lib/run.sh"
python3 "$chk" --root "$cm" --only repo-names >/dev/null 2>&1 \
    && ok "a repository named in a comment is allowed" || bad "the checker rejects a comment"
printf 'echo sysknife\n' >> "$cm/lib/run.sh"
python3 "$chk" --root "$cm" --only repo-names >"$stub_dir/chk.out" 2>&1 \
    && bad "the checker passed a bash line that echoes a project name" \
    || ok "the checker fails on a project name in a bash line"
cp "$root/lib/run.sh" "$cm/lib/run.sh"
printf 'x = "lacs-project/sysknife"\n' >> "$cm/bin/maintainer"
python3 "$chk" --root "$cm" --only repo-names >/dev/null 2>&1 \
    && bad "the checker passed a python string that names a repository" \
    || ok "and on a repository in a python string"
cp "$root/bin/maintainer" "$cm/bin/maintainer"
# An account no repository slug shares, so only the GH_ACCOUNT read can catch it.
mkdir -p "$cm/profiles/zz"; printf 'REPO_SLUG="zzorg/zzrepo"\nGH_ACCOUNT="zzacct"\n' > "$cm/profiles/zz/profile.env"
printf 'owner = "zzacct"\n' >> "$cm/bin/maintainer"
python3 "$chk" --root "$cm" --only repo-names >/dev/null 2>&1 \
    && bad "the checker passed an account name" || ok "and on a GitHub account"
sc_end; fi
rm -rf "$cm"

echo "== --help tells the truth, from the table main() dispatches on =="
# The docstring listed seven verbs and main() dispatched twenty-two, so --help
# hid fifteen commands. It also was not handled at all: it raised SystemExit
# with the text, to stderr, exit 1, and _resolve_profile() ran at import, so with
# two profiles deployed even that failed before it printed.
hh="$stub_dir/helphome"; rm -rf "$hh"
mkdir -p "$hh/.local/share/maintainer/profiles/one" "$hh/.local/share/maintainer/profiles/two"
: > "$hh/.local/share/maintainer/profiles/one/profile.env"; : > "$hh/.local/share/maintainer/profiles/two/profile.env"
for flag in --help -h help; do
    hout="$(env -u MAINTAINER_PROFILE HOME="$hh" python3 "$root/bin/maintainer" "$flag" 2>"$stub_dir/help.err")"; hrc=$?
    [ "$hrc" = 0 ] && [ -n "$hout" ] && [ ! -s "$stub_dir/help.err" ] \
        && ok "maintainer $flag prints to stdout and exits 0, with two profiles deployed and none named" \
        || bad "maintainer $flag: rc=$hrc, stderr: $(head -c 120 "$stub_dir/help.err")"
done
hout="$(env -u MAINTAINER_PROFILE HOME="$hh" python3 "$root/bin/maintainer" --help 2>&1)"
verbs="$(cli_table | awk '{print $1}')"
nverbs="$(wc -w <<<"$verbs")"
[ "$nverbs" -ge 20 ] && ok "the dispatch table has $nverbs verbs" \
    || bad "could not read the dispatch table from bin/maintainer ($nverbs verbs)"
missing=""
for v in $verbs; do grep -qE "^ +maintainer $v( |\$)" <<<"$hout" || missing="$missing $v"; done
[ -z "$missing" ] && ok "--help lists every verb the table dispatches" \
    || bad "--help does not list:$missing"
grep -q 'maintainer run \[--show-prompt\] <task>' <<<"$hout" \
    && ok "and spells the arguments each one takes" || bad "--help drops the argument shapes"
# One source: main() no longer carries its own chain of verb comparisons.
grep -qE '^\s+(el)?if cmd == "' "$root/bin/maintainer" \
    && bad "main() still dispatches on its own if-chain, which is a second list of verbs" \
    || ok "main() dispatches through the table, not a second list"
# A wrong argument count prints the same text to stderr and exits non-zero.
herr="$(env HOME="$hh" MAINTAINER_PROFILE=one python3 "$root/bin/maintainer" assign 1 2>&1 >/dev/null)"; hrc=$?
[ "$hrc" != 0 ] && grep -q 'usage: maintainer assign <issue> <user>' <<<"$herr" \
    && ok "a wrong argument count names the usage of that verb" \
    || bad "assign with one argument: rc=$hrc, $(printf '%s' "$herr" | head -c 120)"
herr="$(env HOME="$hh" MAINTAINER_PROFILE=one python3 "$root/bin/maintainer" nosuchverb 2>&1 >/dev/null)"; hrc=$?
[ "$hrc" != 0 ] && grep -q 'nosuchverb' <<<"$herr" && grep -q 'maintainer status' <<<"$herr" \
    && ok "an unknown verb is named and the verb list follows" || bad "an unknown verb gave: $(printf '%s' "$herr" | head -c 120)"
# Internal verbs are listed, but apart from the ones a person types.
grep -n 'Called by run.sh' <<<"$hout" >/dev/null \
    && [ "$(grep -n 'Called by run.sh' <<<"$hout" | head -1 | cut -d: -f1)" -gt "$(grep -n 'maintainer status' <<<"$hout" | head -1 | cut -d: -f1)" ] \
    && [ "$(grep -n 'maintainer start' <<<"$hout" | head -1 | cut -d: -f1)" -gt "$(grep -n 'Called by run.sh' <<<"$hout" | head -1 | cut -d: -f1)" ] \
    && ok "the verbs run.sh calls are listed apart, under their own heading" \
    || bad "internal verbs are mixed in with the ones a person types"

# maintainer-merge's header and dispatcher agree.
if sc_case 1; then
mg_head="$(sed -n '1,16p' "$root/bin/maintainer-merge" | grep -oE '^#   maintainer-merge [a-z-]+' | awk '{print $3}' | sort | paste -sd' ')"
mg_case="$(sed -n '/^_executed || return 0/,$p' "$root/bin/maintainer-merge" | grep -oE '^    [a-z-]+\)' | tr -d ' )' | sort | paste -sd' ')"
[ -n "$mg_case" ] && [ "$mg_head" = "$mg_case" ] \
    && ok "maintainer-merge's header lists the verbs its dispatcher takes ($mg_case)" \
    || bad "maintainer-merge's header says '$mg_head', its dispatcher takes '$mg_case'"
sc_end; fi

echo "== the documentation is held to the code, by one checker =="
# README said three backends while four existed, "twenty-five" lessons beside
# 110, two docs the code names did not exist, and a dozen environment variables
# nobody had written down. Each was found by hand. scripts/check_drift.py makes
# the class a failure: the checks below run it on the real tree, then plant one
# defect at a time in a copy and demand that the named check, and no other
# reason, is what fails. A bare non-zero exit is not evidence: argparse exits
# non-zero for a check it has never heard of.
chk="$root/scripts/check_drift.py"
for name in cli-verbs config-docs env-vars doc-paths readme-counts readme-profiles changelog-links; do
    if sc_case 1; then
    out="$(python3 "$chk" --root "$root" --only "$name" 2>&1)"; rc=$?
    [ "$rc" = 0 ] && grep -q "ok: $name" <<<"$out" \
        && ok "drift check '$name' passes on the tree" \
        || bad "drift check '$name' fails or does not exist: $(printf '%s' "$out" | head -3 | tr '\n' ' ' | cut -c1-220)"
    sc_end; fi
done
dr="$stub_dir/driftdocs"
drift_fresh() { rm -rf "$dr"; mkdir -p "$dr"
    cp -r "$root/bin" "$root/lib" "$root/docs" "$root/profiles" "$root/scripts" "$root/platform" "$dr/"
    cp "$root/README.md" "$root/CONTRIBUTING.md" "$root/CHANGELOG.md" "$root/install.sh" "$root/new-profile.sh" "$dr/"
    drift_pristine="$(drift_sum)"; }
drift_sum() { (cd "$dr" && find . -type f -print0 | sort -z | xargs -0 sha256sum | sha256sum); }
# drift_bites <check> <regex the failure must contain> <label>
drift_bites() {
    # A mutation that matched nothing leaves the copy pristine, and a check
    # that passes on a pristine tree reads exactly like one that missed the
    # mutation. The lesson-count cases did that when lessons went 110 -> 113.
    if [ "$(drift_sum)" = "$drift_pristine" ]; then bad "$3: the mutation did not apply, so nothing was tested"; return; fi
    if sc_case 1; then
    local out; out="$(python3 "$chk" --root "$dr" --only "$1" 2>&1)"; local rc=$?
    [ "$rc" != 0 ] && grep -qE "FAIL \[$1\].*$2" <<<"$out" \
        && ok "$3" || bad "$3 (rc=$rc): $(printf '%s' "$out" | head -2 | tr '\n' ' ' | cut -c1-200)"
    sc_end; fi
}
drift_fresh; sed -i '/^| `maintainer offer <issue>/d' "$dr/docs/cli.md"
drift_bites cli-verbs 'offer' "a verb missing from the cli.md command table is caught"
drift_fresh; sed -i 's/`start`, `finish`, `abort`, `failed`, `ok`/`start`, `finish`, `abort`, `failed`/' "$dr/docs/cli.md"
drift_bites cli-verbs '\bok\b' "an internal verb missing from the run.sh section is caught"
drift_fresh; sed -i '/^| `maintainer-merge verify-part/d' "$dr/docs/cli.md"
drift_bites cli-verbs 'verify-part' "a maintainer-merge verb missing from cli.md is caught"
drift_fresh; sed -i '/^| `maintainer-repo release-check`/d' "$dr/docs/cli.md"
drift_bites cli-verbs 'release-check' "a maintainer-repo verb missing from cli.md is caught"
drift_fresh; python3 - "$dr/bin/maintainer" <<'PYEOF'
import sys
p = sys.argv[1]; s = open(p).read()
old = "for c in COMMANDS.values():\n            if c.internal != internal:"
assert old in s
open(p, "w").write(s.replace(old, "for c in list(COMMANDS.values())[1:]:\n            if c.internal != internal:"))
PYEOF
drift_bites cli-verbs 'status.*--help|--help.*status' "a verb --help does not print is caught"
drift_fresh; sed -i 's/^| `maintainer gc \[--dry-run\]` |/| `maintainer frobnicate <x>` | invented |\n&/' "$dr/docs/cli.md"
drift_bites cli-verbs 'frobnicate' "a command the docs describe and the code does not have is caught"
drift_fresh; sed -i 's/^| `maintainer-merge show \[pr\]` |/| `maintainer-merge frob <pr>` | invented |\n&/' "$dr/docs/cli.md"
drift_bites cli-verbs 'maintainer-merge frob' "a maintainer-merge row the dispatcher does not take is caught"
drift_fresh; sed -i 's/        raise SystemExit(0)$/        raise SystemExit(1)/' "$dr/bin/maintainer"
drift_bites cli-verbs 'exits 1' "a --help that exits non-zero is caught"
drift_fresh; sed -i 's/^| `MAINTAINER_NOTIFY` |/| `MAINTAINER_NOTIFYX` |/' "$dr/docs/configuration.md"
drift_bites env-vars 'MAINTAINER_NOTIFY' "an environment variable the code reads and no doc lists is caught"
drift_fresh; printf '\n# reads ${MAINTAINER_ZZ_NEW:-}\nx="${MAINTAINER_ZZ_NEW:-}"\n' >> "$dr/lib/run.sh"
drift_bites env-vars 'MAINTAINER_ZZ_NEW' "a new MAINTAINER_ variable in code is caught"
drift_fresh; sed -i '/^| `GH_BACKOFF` |/d' "$dr/docs/configuration.md"
drift_bites config-docs 'GH_BACKOFF' "a key in lib/defaults.env with no row in docs/configuration.md is caught"
drift_fresh; sed -i 's/^VERIFY_PIDS=512$/VERIFY_PIDS=999/' "$dr/lib/defaults.env"
drift_bites config-docs 'VERIFY_PIDS.*999|999.*VERIFY_PIDS' "a documented default that no longer matches lib/defaults.env is caught"
drift_fresh; printf '\n# see docs/nonexistent-page.md\n' >> "$dr/bin/maintainer-repo"
drift_bites doc-paths 'nonexistent-page' "a docs/*.md path named in code that does not exist is caught"
drift_fresh; rm "$dr/docs/release.md"
drift_bites doc-paths 'release.md' "deleting a doc the code points at is caught"
drift_fresh; sed -i 's/\(<!-- fig:backends -->\)[^<]*/\13/' "$dr/README.md"
drift_bites readme-counts 'backends' "a README backend count that disagrees with lib/backends is caught"
drift_fresh; cp "$dr/lib/backends/codex.sh" "$dr/lib/backends/zz.sh"
drift_bites readme-counts 'backends' "adding a backend without updating the README is caught"
drift_fresh; sed -i 's/\(<!-- fig:lessons[^>]*-->\)[^<]*/\125+/' "$dr/README.md"
drift_bites readme-counts 'lessons' "a README lesson count that disagrees with docs/lessons.md is caught"
drift_fresh; sed -i 's/<!-- fig:lessons[^>]*-->[^<]*<!-- \/fig -->/forty-one/' "$dr/CONTRIBUTING.md"
drift_bites readme-counts 'lessons' "CONTRIBUTING's lesson count is held too, in words as well as digits"
drift_fresh; sed -i 's/\[<!-- fig:lessons[^>]*-->[^<]*<!-- \/fig --> lessons,/[Thirty-six lessons,/' "$dr/docs/SUMMARY.md"
drift_bites readme-counts 'lessons' "and so is the book's table of contents"
drift_fresh; sed -i 's/^## 50\. /## 51. /' "$dr/docs/lessons.md"
drift_bites readme-counts 'out of order' "a gap in the lesson numbering is caught, since the count would be a guess"
# The lesson count is a floor of ten, so ONE new entry must leave every document
# current (that is the point: no per-pull-request edit), and entries that carry
# the count over a multiple of ten must make every stated figure stale.
drift_fresh; n="$(grep -c '^## [0-9]*\. ' "$dr/docs/lessons.md")"
if [ $(( (n + 1) % 10 )) = 0 ]; then
    # From 119 the next entry crosses a multiple of ten, which is the other case
    # below; this one failed the day the repository reached 119. Drop the last
    # entry first, so the one appended keeps the count where it is.
    sed -i "/^## $n\. /,\$d" "$dr/docs/lessons.md"; n=$((n - 1))
fi
printf '\n## %s. a lesson the floor absorbs\n' "$((n + 1))" >> "$dr/docs/lessons.md"
out="$(python3 "$chk" --root "$dr" --only readme-counts 2>&1)"; rc=$?
[ "$(drift_sum)" != "$drift_pristine" ] && [ "$rc" = 0 ] \
    && ok "one new lesson entry changes no stated figure, because lessons are shown as a floor" \
    || bad "one new lesson entry moved a figure or the mutation did not apply (rc=$rc)"
drift_fresh; for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    printf '\n## %s. a lesson the counts do not know about\n' \
        "$(( $(grep -c '^## [0-9]*\. ' "$dr/docs/lessons.md") + 1 ))" >> "$dr/docs/lessons.md"; done
drift_bites readme-counts 'lessons' "enough new entries to cross a multiple of ten make every stated lesson count stale"
drift_fresh; sed -i 's/^POST="${MAINTAINER_POST:-on}"/POST="${MAINTAINER_POST:-off}"/' "$dr/profiles/magent/profile.env"
drift_bites readme-profiles 'magent' "a profile whose POST default disagrees with the README table is caught"
drift_fresh; sed -i '/^\[0.6.1\]:/d' "$dr/CHANGELOG.md"
drift_bites changelog-links '0.6.1' "a CHANGELOG version heading with no link reference is caught"
drift_fresh; sed -i 's#compare/v[0-9.]*\.\.\.HEAD#compare/v0.4.1...HEAD#' "$dr/CHANGELOG.md"
drift_bites changelog-links 'Unreleased' "an [Unreleased] link that stops at an old version is caught"
rm -rf "$dr"
# A checker nothing runs is decoration (MISTAKES 7): the hook, CI and the suite
# all have to reach it.
[ -x "$chk" ] && ok "check_drift.py is executable, as the README says to run it" || bad "check_drift.py is not executable"
if sc_case 1; then
grep -q 'check_drift.py' "$root/.githooks/pre-commit" \
    && ok "the pre-commit hook runs check_drift.py" || bad "the hook never runs the docs checker"
sc_end; fi
grep -q 'check_drift.py' "$root/.github/workflows/ci.yml" \
    && ok "and CI runs it" || bad "CI never runs the docs checker"

echo "== every figure in the docs is rendered from its source, never typed =="
# README said 1324 tests, "the 114 entries", "37 denied verbs" in prose a person
# had to remember to edit, and two drift tests went vacuous because they matched
# the literal 110. scripts/render_figures.py owns every such number: a marker in
# the prose, a registry that counts the tree, and a check that refuses a stale
# marker, an unknown name, a figure typed outside a marker and a source it could
# not read. The cases run it on the real tree, then plant one defect at a time in
# a copy and demand the named reason (not merely a non-zero exit) is what fails.
rf="$root/scripts/render_figures.py"
fg="$stub_dir/figs"
fig_sum() { (cd "$fg" && find . -type f -print0 | sort -z | xargs -0 sha256sum | sha256sum); }
fig_fresh() { rm -rf "$fg"; mkdir -p "$fg"
    cp -r "$root/bin" "$root/lib" "$root/docs" "$root/profiles" "$root/scripts" "$root/evals" "$fg/"
    cp "$root/README.md" "$root/CONTRIBUTING.md" "$root/CHANGELOG.md" "$fg/"
    # The copy's test count is pinned to the 1,300 band, so these cases measure
    # the renderer and not how big the real suite is today. Copied as it stood,
    # every case below went red the commit the suite crossed 1,400.
    sed -i 's/\(<!-- fig:tests floor=100 -->\)[^<]*/\11,300+/' "$fg/README.md"
    fig_pristine="$(fig_sum)"; }
# 1350 sits inside the copy's "1,300+" floor, so the pristine copy passes.
fig_bites() {  # $1 = regex a FAIL line must match, $2 = label, rest = extra args
    local re="$1" label="$2"; shift 2
    # A mutation that applied to nothing leaves the copy pristine, and a checker
    # that passes a pristine tree reads exactly like one that missed the mutation.
    if [ "$(fig_sum)" = "$fig_pristine" ]; then bad "$label: the mutation did not apply, so nothing was tested"; return; fi
    [ "$#" -gt 0 ] || set -- --check --tests 1350
    # Capped by time and address space: a mutant that loops would otherwise grow
    # without bound and take the machine with it, and read as a hang, not a failure.
    local out rc; out="$( (ulimit -v 4000000; timeout 60 python3 "$rf" --root "$fg" "$@") 2>&1)"; rc=$?
    [ "$rc" != 0 ] && grep -qE "^FAIL.*($re)" <<<"$out" \
        && ok "$label" || bad "$label (rc=$rc): $(printf '%s' "$out" | head -2 | tr '\n' ' ' | cut -c1-220)"
}
[ -x "$rf" ] && ok "render_figures.py is executable" || bad "render_figures.py is missing or not executable"
out="$(python3 "$rf" --check --no-tests 2>&1)"; rc=$?
[ "$rc" = 0 ] && ok "every figure on the real tree is current" \
    || bad "the real tree has a stale or malformed figure (rc=$rc): $(printf '%s' "$out" | head -3 | tr '\n' ' ' | cut -c1-220)"
fig_fresh
out="$(python3 "$rf" --root "$fg" --check --tests 1350 2>&1)"; rc=$?
[ "$rc" = 0 ] && ok "control: the copy passes with a measured count inside the floor" \
    || bad "control: the pristine copy fails, so no refusal below is evidence (rc=$rc): $(printf '%s' "$out" | head -2 | tr '\n' ' ' | cut -c1-200)"
out="$(python3 "$rf" --root "$fg" --check 2>&1)"; rc=$?
[ "$rc" = 2 ] && grep -q -- '--tests' <<<"$out" \
    && ok "a check with no test count and no --no-tests is refused, not skipped" \
    || bad "a check that was given no test count passed or died for another reason (rc=$rc)"
# Floor arithmetic at a boundary: v = K*n and v = K*n - 1.
fl="$(python3 -B -c "
import sys; sys.path.insert(0, '$root/scripts')
import render_figures as r
print(r.render(1300, 100), r.render(1299, 100), r.render(1400, 100), r.render(1399, 100), r.render(114, 10), r.render(1324, None), r.render(9, None))")"
check "floor renders floor(v/K)*K with a trailing + and thousands separators, exact otherwise" \
    "$fl" "1,300+ 1,200+ 1,400+ 1,300+ 110+ 1,324 9"
fig_fresh; out="$(python3 "$rf" --root "$fg" --check --tests 1300 2>&1)"; rc1=$?
out="$(python3 "$rf" --root "$fg" --check --tests 1399 2>&1)"; rc2=$?
[ "$rc1" = 0 ] && [ "$rc2" = 0 ] && ok "the README's 1,300+ holds at 1300 and at 1399" \
    || bad "the test floor is wrong at the edges of its band (rc=$rc1,$rc2)"
for n in 1299 1400; do
    out="$(python3 "$rf" --root "$fg" --check --tests "$n" 2>&1)"; rc=$?
    [ "$rc" != 0 ] && grep -qE "^FAIL README.md:[0-9]+: stale marker 'tests'" <<<"$out" \
        && ok "a test count of $n makes the 1,300+ marker stale" \
        || bad "a test count of $n left the 1,300+ marker current (rc=$rc)"
done
# Each refusal below plants one defect in a fresh copy.
fig_fresh; sed -i 's/\(<!-- fig:lessons[^>]*-->\)[^<]*/\1999+/' "$fg/CONTRIBUTING.md"
fig_bites 'CONTRIBUTING.md:[0-9]+.*stale' "a stale marker is refused, naming file and line"
fig_fresh; sed -i 's/<!-- fig:tests floor=100 -->/<!-- fig:tests -->/' "$fg/README.md"
fig_bites 'README.md:[0-9]+.*tests.*takes floor=100' "a test count marker with no floor is refused, so it cannot go stale every pull request"
fig_fresh; sed -i 's/<!-- fig:backends/<!-- fig:backendz/' "$fg/README.md"
fig_bites 'README.md:[0-9]+.*unknown figure .backendz' "an unknown figure name is refused"
fig_fresh; sed -i 's/<!-- fig:tests[^>]*-->[^<]*<!-- \/fig -->/1,324/' "$fg/README.md"
fig_bites 'README.md:[0-9]+.*tests.*not in a marker' "a test count typed outside a marker is refused"
fig_fresh; sed -i 's/<!-- fig:denied_verbs -->[^<]*<!-- \/fig -->/37/' "$fg/README.md"
fig_bites 'README.md:[0-9]+.*denied.*not in a marker' "a typed denied-verb count is refused"
fig_fresh; sed -i 's/<!-- fig:evals -->[^<]*<!-- \/fig -->/9/' "$fg/README.md"
fig_bites 'README.md:[0-9]+.*evals.*not in a marker' "a typed eval count is refused"
fig_fresh; sed -i 's/<!-- fig:lessons[^>]*-->[^<]*<!-- \/fig -->/forty-one/' "$fg/docs/SUMMARY.md"
fig_bites 'SUMMARY.md:[0-9]+.*lessons.*not in a marker' "a lesson count typed in words outside a marker is refused"
fig_fresh; sed -i 's/<!-- fig:backends -->[^<]*<!-- \/fig -->/Four/' "$fg/README.md"
fig_bites 'README.md:[0-9]+.*backends.*not in a marker' "a backend count typed in words outside a marker is refused"
fig_fresh; sed -i 's/<!-- fig:profiles -->[^<]*<!-- \/fig -->/2/' "$fg/README.md"
fig_bites 'README.md:[0-9]+.*profiles.*not in a marker' "a typed profile count is refused"
fig_fresh; sed -i 's/<!-- fig:lessons[^>]*-->[^<]*<!-- \/fig -->//' "$fg/README.md"
fig_bites 'README.md.*lessons.*(must|required)' "a site that stops stating its figure is refused, not left inspecting nothing"
fig_fresh; sed -i 's/<!-- fig:lessons\( floor=10\)\? -->[^<]*<!-- \/fig -->/<!-- fig:lessons floor=10 -->110+/' "$fg/CONTRIBUTING.md"
fig_bites 'CONTRIBUTING.md:[0-9]+.*malformed' "a marker with no closing tag is refused as malformed"
fig_fresh; sed -i 's/<!-- fig:lessons\( floor=10\)\? -->/<!-- fig:lessons floor=10 --><!-- fig:evals -->/' "$fg/CONTRIBUTING.md"
fig_bites 'CONTRIBUTING.md:[0-9]+.*(nested|malformed)' "a nested marker is refused"
fig_fresh; sed -i 's/<!-- fig:lessons floor=10 -->/<!-- fig:lessons floor=abc -->/' "$fg/CONTRIBUTING.md"
fig_bites 'CONTRIBUTING.md:[0-9]+.*malformed' "a floor that is not a number is refused as malformed"
fig_fresh; printf '\n```\n<!-- fig:evals -->9<!-- /fig -->\n```\n' >> "$fg/CONTRIBUTING.md"
fig_bites 'CONTRIBUTING.md:[0-9]+.*code fence' "a marker inside a code fence is refused"
fig_fresh; printf '\n~~~\n<!-- fig:evals -->9<!-- /fig -->\n~~~\n' >> "$fg/docs/roadmap.md"
fig_bites 'roadmap.md:[0-9]+.*code fence' "a marker inside a tilde fence is refused too"
# The glob is expanded inside the copy: relative to the caller's directory it
# matched nothing when the suite was run from elsewhere, and the case then
# stripped only two files.
fig_fresh; (cd "$fg" && sed -i 's/<!-- fig:[^>]*-->\([^<]*\)<!-- \/fig -->/\1/g' README.md CONTRIBUTING.md docs/*.md)
fig_bites 'no figure markers' "a tree with no markers at all is refused, not passed"
fig_fresh; rm -rf "$fg/lib/backends"
fig_bites 'backends.*(cannot read|unreadable)' "an unreadable backends source fails closed"
fig_fresh; rm -rf "$fg/evals/scenarios"
fig_bites 'evals.*(cannot read|unreadable)' "an unreadable eval source fails closed"
fig_fresh; : > "$fg/docs/lessons.md"
fig_bites 'lessons.*(cannot read|unreadable|no numbered)' "an empty lessons file fails closed"
fig_fresh; printf '{ not json' > "$fg/profiles/sysknife/deny.json"
fig_bites 'denied_verbs.*(cannot read|unreadable)' "a deny spec that does not parse fails closed"
fig_fresh; : > "$fg/docs/user-stories.md"
fig_bites 'stories.*(cannot read|unreadable|no stories)' "an unreadable user-stories file fails closed"
fig_fresh; sed -i 's/^\*\*[A-Za-z0-9 <>!\/:=_,-]* have no governing rule\*\*: [0-9]*,/**have no governing rule**: 3,/' "$fg/docs/user-stories.md"
fig_bites 'gap' "a finding that no longer states its form is refused"
fig_fresh; sed -i 's/^23, 24, 26, 31, 32\./23, 24, 26, 31./' "$fg/docs/user-stories.md"
fig_bites 'listed numbers' "a gap list that disagrees with the marked stories is refused"
# --write: fixes values, is idempotent, and never edits around a structural error.
fig_fresh; sed -i 's/\(<!-- fig:lessons[^>]*-->\)[^<]*/\1999+/; s/\(<!-- fig:evals -->\)[^<]*/\177/' "$fg/README.md"
out="$(python3 "$rf" --root "$fg" --write --tests 1350 2>&1)"; rc=$?
out2="$(python3 "$rf" --root "$fg" --check --tests 1350 2>&1)"; rc2=$?
[ "$rc" = 0 ] && [ "$rc2" = 0 ] && ok "--write repairs stale markers and --check then passes" \
    || bad "--write then --check did not converge (rc=$rc,$rc2): $(printf '%s%s' "$out" "$out2" | head -2 | tr '\n' ' ' | cut -c1-200)"
h1="$(fig_sum)"; python3 "$rf" --root "$fg" --write --tests 1350 >/dev/null 2>&1; rcw=$?
[ "$rcw" = 0 ] && [ "$(fig_sum)" = "$h1" ] && ok "a second --write changes no byte" \
    || bad "a second --write changed the tree or failed (rc=$rcw)"
# --write restores the original BYTES, not merely a passing tree: it may touch
# the marker's value and nothing else (no added newline, no reflow).
fig_fresh; hr="$(sha256sum < "$fg/README.md")"; hc="$(sha256sum < "$fg/CONTRIBUTING.md")"
sed -i 's/\(<!-- fig:lessons[^>]*-->\)[^<]*/\1999+/; s/\(<!-- fig:tests[^>]*-->\)[^<]*/\10+/' "$fg/README.md"
sed -i 's/\(<!-- fig:lessons[^>]*-->\)[^<]*/\1999+/' "$fg/CONTRIBUTING.md"
if [ "$(sha256sum < "$fg/README.md")" = "$hr" ] || [ "$(sha256sum < "$fg/CONTRIBUTING.md")" = "$hc" ]; then
    bad "--write byte-exactness: the mutation did not apply, so nothing was tested"
else
    python3 "$rf" --root "$fg" --write --tests 1350 >/dev/null 2>&1
    [ "$(sha256sum < "$fg/README.md")" = "$hr" ] && [ "$(sha256sum < "$fg/CONTRIBUTING.md")" = "$hc" ] \
        && ok "--write restores the original bytes: only marker values change" \
        || bad "--write changed bytes outside the marker values"
fi
fig_fresh; sed -i 's/\(<!-- fig:lessons[^>]*-->\)[^<]*/\1999+/' "$fg/CONTRIBUTING.md"; sed -i 's/<!-- fig:backends/<!-- fig:backendz/' "$fg/README.md"
h1="$(fig_sum)"; out="$(python3 "$rf" --root "$fg" --write --tests 1350 2>&1)"; rc=$?
[ "$rc" != 0 ] && [ "$(fig_sum)" = "$h1" ] && grep -q 'unknown figure' <<<"$out" \
    && ok "--write refuses and writes nothing while a structural error stands" \
    || bad "--write edited files around a structural error, or passed (rc=$rc)"
# --write without --tests runs the suite itself, through the tree's own runner.
fig_fresh; mkdir -p "$fg/tests"
printf '#!/usr/bin/env bash\necho "  1397 passed, 0 failed, 3 skipped"\n' > "$fg/tests/run-tests.sh"
out="$(python3 "$rf" --root "$fg" --write 2>&1)"; rc=$?
grep -q 'fig:tests floor=100 -->1,400+<!-- /fig' "$fg/README.md" && [ "$rc" = 0 ] \
    && ok "--write with no --tests measures passed plus skipped through the suite" \
    || bad "--write did not measure the suite (rc=$rc): $(printf '%s' "$out" | head -2 | tr '\n' ' ' | cut -c1-160)"
fig_fresh; mkdir -p "$fg/tests"
printf '#!/usr/bin/env bash\necho "  1450 passed, 0 failed, 3 skipped"\nexit 1\n' > "$fg/tests/run-tests.sh"
h1="$(fig_sum)"; out="$(python3 "$rf" --root "$fg" --write 2>&1)"; rc=$?
[ "$rc" != 0 ] && [ "$(fig_sum)" = "$h1" ] \
    && ok "--write on a red suite refuses instead of trusting its printed count" \
    || bad "--write believed a red suite's count (rc=$rc)"
rm -rf "$fg"
# The consumers reach it, and the old regexes are gone from the claim checker.
if sc_case 1; then
grep -q 'render_figures.py' "$root/.githooks/pre-commit" \
    && ok "the pre-commit hook runs render_figures.py" || bad "the hook never runs the figure check"
sc_end; fi
grep -q 'render_figures.py' "$root/.github/workflows/ci.yml" \
    && ok "CI runs render_figures.py" || bad "CI never runs the figure check"
if sc_case 1; then
grep -qE 'render_figures.py.*--check.*--tests' "$root/scripts/check_claims.sh" \
    && ok "check_claims.sh hands render_figures.py the count it measured" || bad "check_claims.sh does not pass --tests"
sc_end; fi
grep -q 'render_figures' "$root/scripts/check_drift.py" \
    && ok "check_drift.py counts through the same module" || bad "check_drift.py keeps its own counting code"
if sc_case 1; then
if grep -q 'claim ' "$root/scripts/check_claims.sh"; then bad "check_claims.sh still compares typed README numbers"; else ok "check_claims.sh no longer compares typed README numbers"; fi
sc_end; fi

echo "== the check board must be entirely pass or skipping =="
# gh buckets a check as pass, fail, pending, skipping or cancel. The gate counted
# fail and pending and ignored the rest, and never compared what it accepted with
# the total, so a cancelled check passed, an unrecognised bucket passed, and an
# empty list (`[]`, which is a non-empty string) passed as a green board with
# nothing on it. Nothing had ever fed it a pending or a cancel bucket.
mgb="$root/bin/maintainer-merge"
board_gate() {  # $1 = the JSON gh prints for `pr checks`
    make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *reviewDecision*) echo APPROVED;;
      *headRefOid*) echo abcdef1234;;
      *mergeStateStatus*) echo CLEAN;;
      *'pr checks'*) printf '%s\\n' '$1';;
    esac"
    PATH="$stub_dir:$PATH" bash "$mgb" merge 31 2>&1
}
# The refusal has to be the board's. The receipt check runs before the board and
# the pull-request fetch runs after it, so a recorded receipt plus a positive
# control that reaches "cannot fetch" shows a refusal below is the board's own.
mkdir -p "$MAINTAINER_STATE/receipts"
make_stub gh "case \"\$*\" in *'auth switch'*) exit 0;; *'api user'*) echo testuser;; esac"
PATH="$stub_dir:$PATH" bash "$mgb" receipt 31 abcdef1234 "guard mutated, went red" >/dev/null 2>&1
[ -f "$MAINTAINER_STATE/receipts/31.json" ] && ok "a receipt for #31 exists, so the board is the next gate" \
    || bad "no receipt for #31 could be recorded, so the cases below prove nothing"
out="$(board_gate '[{"name":"a","bucket":"pass"},{"name":"b","bucket":"skipping"}]')"; rc=$?
grep -q 'cannot fetch #31' <<<"$out" \
    && ok "pass plus skipping is a green board, and the gate moves on to fetching the pull request" \
    || bad "a board of pass and skipping did not get past the board: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-120)"
out="$(board_gate '[{"name":"a","bucket":"pass"},{"name":"b","bucket":"pending"}]')"; rc=$?
[ "$rc" != 0 ] && grep -q '1 pending check' <<<"$out" \
    && ok "a pending check is refused, and counted" || bad "a pending check was not refused (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-120)"
out="$(board_gate '[{"name":"a","bucket":"pass"},{"name":"b","bucket":"cancel"}]')"; rc=$?
[ "$rc" != 0 ] && grep -qi 'cancel' <<<"$out" && ! grep -q 'cannot fetch' <<<"$out" \
    && ok "a cancelled check is refused: it never reported a verdict" \
    || bad "a cancelled check passed the board (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-120)"
out="$(board_gate '[{"name":"a","bucket":"pass"},{"name":"b","bucket":"timed_out"}]')"; rc=$?
[ "$rc" != 0 ] && grep -q '1 of 2' <<<"$out" && ! grep -q 'cannot fetch' <<<"$out" \
    && ok "a bucket the gate does not know is refused, with pass+skipping out of total in the message" \
    || bad "an unrecognised bucket passed the board (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-120)"
out="$(board_gate '[{"name":"a","bucket":"pass"},{"name":"b"}]')"; rc=$?
[ "$rc" != 0 ] && ! grep -q 'cannot fetch' <<<"$out" \
    && ok "a check with no bucket at all is refused" || bad "a check with no bucket passed (rc=$rc)"
out="$(board_gate '[]')"; rc=$?
[ "$rc" != 0 ] && ! grep -q 'cannot fetch' <<<"$out" && grep -qi 'no checks\|empty' <<<"$out" \
    && ok "an empty list is refused as an empty board, which the README promises" \
    || bad "an empty check list passed the board (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-120)"
out="$(board_gate '[{"name":"a","bucket":"skipping"},{"name":"b","bucket":"skipping"}]')"; rc=$?
[ "$rc" != 0 ] && ! grep -q 'cannot fetch' <<<"$out" \
    && ok "a board where every check was skipped is refused: nothing ran" \
    || bad "a board of only skipped checks passed (rc=$rc)"
out="$(board_gate '[{"name":"a","bucket":"fail"},{"name":"b","bucket":"pending"}]')"; rc=$?
grep -q 'failing check' <<<"$out" && ok "failing is still reported ahead of pending" \
    || bad "a failing and a pending check: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-100)"
rm -f "$stub_dir/gh" "$MAINTAINER_STATE/receipts/31.json"

# ===== Z: stalled pull requests (begin) =====
echo "== stalled pull requests: a fixup the author left undone =="
# Z block. A pull request that has sat on mechanical review items (a rebase, a
# test count, one doc line) is finished by a SCRIPT, never by the model, so the
# cases below are all refusals plus the one path that pushes. Nothing here
# reaches GitHub: gh is a stub that logs every call and exits 97 on one it does
# not know, the fork is a local bare repository, and git is told to allow no
# transport but file, so a URL the rewrite missed fails instead of connecting.
zf="$stub_dir/zfix"; rm -rf "$zf"
zprof="$zf/home/.local/share/maintainer/profiles/fx"
mkdir -p "$zprof" "$zf/bin" "$zf/state/drafts/r1" "$zf/checkout"
cat > "$zprof/profile.env" <<ZEOF
REPO_PATH="$zf/checkout"
REPO_SLUG="o/r"
GH_ACCOUNT="owner"
STATE_DIR="$zf/state"
PROD_GLOBS="src/* Makefile"
FIXUP_COUNT_CMD="grep -c '^fn test_' src/lib.rs"
ZEOF
cat > "$zf/gitconfig" <<ZEOF
[url "$zf/fork.git"]
	insteadOf = https://github.com/contrib/repo.git
[url "$zf/origin.git"]
	insteadOf = https://github.com/o/r.git
ZEOF
cat > "$zf/bin/gh" <<'GHEOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$FX/calls.log"
case "$*" in
  *'auth switch'*) exit 0 ;;
  'api user --jq'*) [ -x "$FX/on_me" ] && "$FX/on_me"; cat "$FX/me"; exit 0 ;;
  'api users/'*) [ -x "$FX/on_author" ] && "$FX/on_author"; cat "$FX/author"; exit 0 ;;
  'pr view'*'--json headRefOid --jq'*)
      if [ -f "$FX/readback" ]; then cat "$FX/readback"
      else git --git-dir="$FX/fork.git" rev-parse refs/heads/fix-x; fi; exit 0 ;;
  'pr view'*) [ -f "$FX/pr.fail" ] && exit 1; cat "$FX/pr.json"; exit 0 ;;
  'api repos/o/r/pulls/'*'/comments'*) [ -f "$FX/threads.fail" ] && exit 1; cat "$FX/threads"; exit 0 ;;
  'api repos/o/r/actions/runs'*) [ -f "$FX/runs.fail" ] && exit 1; cat "$FX/runs.json"; exit 0 ;;
  'pr list'*) cat "$FX/list.json"; exit 0 ;;
  'pr comment'*)
      f=""; prev=""; for a in "$@"; do [ "$prev" = --body-file ] && f="$a"; prev="$a"; done
      [ -n "$f" ] && cp "$f" "$FX/notice.body"
      # What GitHub would do next: the comment appears on the pull request.
      [ -f "$FX/nocomment" ] || python3 - "$FX" "$f" <<'PY'
import datetime, json, sys
fx, f = sys.argv[1:3]
pr = json.load(open(fx + "/pr.json"))
pr["comments"].append({"author": {"login": "owner"}, "body": open(f).read(),
                       "createdAt": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")})
json.dump(pr, open(fx + "/pr.json", "w"))
PY
      exit 0 ;;
esac
echo "unstubbed gh call: $*" >&2; exit 97
GHEOF
chmod +x "$zf/bin/gh"
printf '1234\towner\n' > "$zf/me"
printf '5678\tcontrib\n' > "$zf/author"
zenv() { env -u PROD_GLOBS -u FIXUP_COUNT_CMD PATH="$zf/bin:$PATH" HOME="$zf/home" FX="$zf" GIT_CONFIG_GLOBAL="$zf/gitconfig" \
    GIT_CONFIG_NOSYSTEM=1 GIT_ALLOW_PROTOCOL=file GIT_TERMINAL_PROMPT=0 MAINTAINER_PROFILE=fx \
    MAINTAINER_SLUG=o/r MAINTAINER_REPO="$zf/checkout" MAINTAINER_ACCOUNT=owner \
    MAINTAINER_STATE="$zf/state" PROFILE_DIR="$zprof" MAINTAINER_POST=on MAINTAINER_GH_BACKOFF=0 "$@"; }
cat > "$zf/gen.py" <<'PYEOF'
import json, os, datetime as D
e = os.environ.get
now = D.datetime.now(D.timezone.utc)
def ago(days):
    return (now - D.timedelta(days=float(days))).strftime("%Y-%m-%dT%H:%M:%SZ")
fx = e("FX")
head = e("HEAD")
age = e("REVIEW_AGE", "15")
items = e("ITEMS", '[{"kind":"rebase"}]')
mh = e("MARKER_HEAD", head)
marker = f"<!-- maintainer: fixup-items v1 head={mh} {items} -->"
mode = e("MARKER_MODE", "normal")
body = "Two mechanical items remain.\n\n"
if mode == "normal":   body += marker
elif mode == "none":   body += "Please rebase."
elif mode == "quoted": body = marker + "\n\nI quoted my own marker above and then wrote more words after it."
elif mode == "prose":  body += marker + "\nthen a closing line"
own = {"author": {"login": "owner"}, "state": "CHANGES_REQUESTED", "submittedAt": ago(age), "body": body}
reviews = [own]
if mode == "other":
    reviews = [dict(own, author={"login": "someone"}, state="COMMENTED", body=marker),
               {"author": {"login": "owner"}, "state": "CHANGES_REQUESTED", "submittedAt": ago(age), "body": "see my comments"}]
if e("OTHER_CR"):
    reviews.append({"author": {"login": "reviewer2"}, "state": "CHANGES_REQUESTED", "submittedAt": ago(float(age) + 1), "body": "also no"})
if e("OWN_LATER"):
    reviews.append({"author": {"login": "owner"}, "state": e("OWN_LATER"), "submittedAt": ago(float(age) - 1), "body": "later"})
if e("REVIEW_AFTER"):
    reviews.append({"author": {"login": "contrib"}, "state": "COMMENTED", "submittedAt": ago(e("REVIEW_AFTER")), "body": "pushing soon"})
comments = []
if e("COMMENT_AFTER"):
    comments.append({"author": {"login": "contrib"}, "createdAt": ago(e("COMMENT_AFTER")), "body": "will do"})
if e("NOTICE_AGE"):
    nh = e("NOTICE_HEAD", head)
    comments.append({"author": {"login": e("NOTICE_BY", "owner")}, "createdAt": ago(e("NOTICE_AGE")),
                     "body": f"No reply for 10 days.\n\n<!-- maintainer: fixup-notice v1 head={nh} -->"})
commits = [{"oid": "1" * 40, "committedDate": ago(float(age) + 3), "authoredDate": ago(float(age) + 3)}]
if e("COMMIT_AFTER"):
    commits.append({"oid": "2" * 40, "committedDate": ago(e("COMMIT_AFTER")), "authoredDate": ago(e("COMMIT_AFTER"))})
pr = {"number": 12, "state": e("STATE", "OPEN"), "isDraft": e("DRAFT") == "1",
      "author": {"login": "contrib", "is_bot": e("BOT") == "1"},
      "reviewDecision": e("DECISION", "CHANGES_REQUESTED"),
      "maintainerCanModify": e("CAN_MODIFY", "1") == "1",
      "headRefOid": head, "headRefName": "fix-x", "baseRefName": "main", "isCrossRepository": True,
      "headRepository": {"name": "repo"}, "headRepositoryOwner": {"login": "contrib"},
      "reviews": reviews, "comments": comments, "commits": commits}
open(f"{fx}/pr.json", "w").write(json.dumps(pr))
th = []
if e("THREAD_AFTER"):
    th.append({"u": "contrib", "t": ago(e("THREAD_AFTER"))})
open(f"{fx}/threads", "w").write("".join(json.dumps(x) + "\n" for x in th))
runs = [{"s": "completed", "c": e("RUN_CONCL", "success")}]
open(f"{fx}/runs.json", "w").write(json.dumps(runs))
open(f"{fx}/list.json", "w").write(json.dumps([{"number": 12, "author": {"login": "contrib"},
    "reviewDecision": pr["reviewDecision"], "isDraft": pr["isDraft"], "maintainerCanModify": pr["maintainerCanModify"]}]))
for f in ("pr.fail", "threads.fail", "runs.fail"):
    p = f"{fx}/{f}"
    if os.path.exists(p): os.remove(p)
for k in ("PR_FAIL", "THREADS_FAIL", "RUNS_FAIL"):
    if e(k): open(f"{fx}/{k.lower().replace('_f', '.f')}", "w").write("")
PYEOF
# The fork and the base. The contributor branch fix-x is one commit behind main,
# so a rebase has something to do. ZCONFLICT=1 makes main touch the same file.
zrepo() {
    rm -rf "$zf/origin.git" "$zf/fork.git" "$zf/seed"
    git init -q --bare -b main "$zf/origin.git"; git init -q --bare -b main "$zf/fork.git"
    git init -q -b main "$zf/seed"
    ( cd "$zf/seed" || exit 1
      export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
      git config user.email m@m; git config user.name maint
      mkdir -p src docs .github/workflows
      printf 'It has 7 tests.\n' > README.md
      printf 'guide line one\nguide line two\n' > docs/guide.md
      for i in 1 2 3 4 5 6 7 8 9; do printf 'fn test_%s() {}\n' "$i"; done > src/lib.rs
      printf 'all:\n\ttrue\n' > Makefile
      printf 'name: ci\n' > .github/workflows/ci.yml
      printf 'version: 1\n' > .github/dependabot.yml
      printf 'base\n' > notes.txt
      printf '{"name":"x"}\n' > package.json
      printf 'key=1\n' > settings.cfg
      git add -A; git commit -qm base
      git push -q "$zf/origin.git" main; git push -q "$zf/fork.git" main
      git checkout -q -b fix-x
      printf 'contrib work\n' >> notes.txt
      GIT_AUTHOR_NAME=contrib GIT_AUTHOR_EMAIL=5678+contrib@users.noreply.github.com git commit -qam "contrib change${ZSIGN:+

Signed-off-by: contrib <c@c>}"
      git push -q "$zf/fork.git" fix-x
      git checkout -q main
      if [ "${ZCONFLICT:-0}" = 1 ]; then printf 'main change\n' >> notes.txt; git commit -qam "main moves notes"
      else printf 'elsewhere\n' > other.txt; git add other.txt; git commit -qm "main moves elsewhere"; fi
      git push -q "$zf/origin.git" main )
}
zhead() { git --git-dir="$zf/fork.git" rev-parse refs/heads/fix-x; }
# Regenerate the pull request the stub serves. Every knob is an env var.
zmk() { env FX="$zf" HEAD="$(zhead)" "$@" python3 "$zf/gen.py"; }
zpatch() {  # $1 = path in the tree, $2 = sed expression: write the run's patch for #12
    local t; t="$(mktemp -d "$zf/pt.XXXXXX")"
    git clone -q --branch fix-x "$zf/fork.git" "$t/w" 2>/dev/null
    ( cd "$t/w" || exit 1; sed -i "$2" "$1"; git diff > "$zf/state/drafts/r1/fixup-12.patch" )
    rm -rf "$t"
}
zcheck() { zenv "$@" python3 "$root/bin/maintainer" fixup-check 12 2>&1; }
zrefused() {  # $1 label, $2 regex the refusal must match. The fixture is already built.
    local out rc; out="$(zcheck)"; rc=$?
    if [ "$rc" != 0 ] && grep -q '^maintainer: fixup-check #12: ' <<<"$out" && grep -qiE -- "$2" <<<"$out"; then ok "$1"
    else bad "$1 (rc=$rc, wanted /$2/): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-150)"; fi
}
zstate() {  # $1 label, $2 expected state
    local out rc; out="$(zcheck)"; rc=$?
    if [ "$rc" = 0 ] && grep -q "^state=$2\$" <<<"$out"; then ok "$1"
    else bad "$1 (rc=$rc, wanted state=$2): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-150)"; fi
}
zrepo
rm -f "$zf/calls.log"

# --- fixup-check: the preconditions, one refusal each -------------------------
# Positive control first. Every refusal below changes ONE knob of this fixture,
# so a case that refuses for an unrelated reason cannot pass for the wrong one.
zmk; zstate "a stalled pull request with a rebase item is accepted, and a notice is due first" notice
zmk NOTICE_AGE=6; zstate "a notice older than FIXUP_NOTICE_DAYS makes it ready" ready
out="$(zcheck)"
grep -q '^items=rebase$' <<<"$out" && grep -q "^head=$(zhead)\$" <<<"$out" && grep -q '^author=contrib$' <<<"$out" \
    && ok "the check reports the items, the head and the author for the script to use" \
    || bad "fixup-check output lacks items, head or author: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-150)"

zmk NOTICE_AGE=1; zrefused "a notice younger than FIXUP_NOTICE_DAYS is refused, with the date it ripens" 'notice.*(wait|until|not before)'
zmk NOTICE_AGE=6 NOTICE_HEAD="$(printf 'b%.0s' $(seq 40))"; zstate "a notice written for a different head does not count, so a fresh one is due" notice
zmk NOTICE_AGE=6 NOTICE_BY=stranger; zstate "a notice posted by anyone but the maintainer's account does not count" notice

zmk DECISION=APPROVED;           zrefused "a pull request that is not CHANGES_REQUESTED is refused" 'CHANGES_REQUESTED'
zmk STATE=CLOSED;                zrefused "a closed pull request is refused" 'not open'
zmk DRAFT=1;                     zrefused "a draft is refused" 'draft'
zmk BOT=1;                       zrefused "a bot's pull request is refused" 'bot'
zmk CAN_MODIFY=0;                zrefused "maintainerCanModify=false is refused: the push would be refused by GitHub anyway" 'maintainerCanModify'
zmk RUN_CONCL=action_required;   zrefused "a first-time contributor's runs sitting at action_required are refused" 'action_required'
zmk REVIEW_AGE=9;                zrefused "a review nine days old is refused when the window is ten" '(younger|only 9|not yet 10|quiet)'
zmk REVIEW_AGE=11 NOTICE_AGE=6;  zstate "a review eleven days old passes the window" ready
zmk MARKER_HEAD="$(printf 'c%.0s' $(seq 40))"; zrefused "a marker written for another head is refused: the PR moved since the review" 'head'
zmk ITEMS='[{"kind":"rebase"},{"kind":"rename-everything"}]'; zrefused "a kind outside the enum makes the whole review substantive" 'kind'
zmk ITEMS='[]';                  zrefused "an empty item list is refused" '(empty|no item)'
zmk ITEMS='[{"kind":"rebase","extra":1}]'; zrefused "an item with an unknown key is refused" 'key'
zmk ITEMS='[{"kind":"test-count"}]'; zrefused "a test-count item that names no file is refused" 'file'
zmk ITEMS='[{"kind":"doc-line","file":"../etc/passwd"}]'; zrefused "an item naming a path outside the tree is refused" 'file'
zmk MARKER_MODE=none;            zrefused "a review with no marker is substantive" 'marker'
zmk MARKER_MODE=other;           zrefused "a marker in somebody else's review is contributor text, not an instruction" 'marker|CHANGES_REQUESTED'
zmk MARKER_MODE=quoted;          zrefused "a marker quoted earlier in the body is ignored: only the final line counts" 'marker'
zmk MARKER_MODE=prose;           zrefused "a marker followed by more text is refused" 'marker'
zmk OTHER_CR=1;                  zrefused "another reviewer's standing CHANGES_REQUESTED is substantive and blocks the fixup" 'reviewer2|another reviewer'
zmk OWN_LATER=APPROVED;          zrefused "the maintainer's own later approval means the request is no longer standing" 'CHANGES_REQUESTED|latest'
zmk COMMIT_AFTER=2;              zrefused "an author commit after the review is refused" 'activity|commit'
zmk COMMENT_AFTER=2;             zrefused "an author comment after the review is refused" 'activity|comment'
zmk REVIEW_AFTER=2;              zrefused "an author review after the review is refused" 'activity|review'
zmk THREAD_AFTER=2;              zrefused "an author reply in a review thread after the review is refused" 'activity|thread'
zmk COMMENT_AFTER=40 NOTICE_AGE=6; zstate "an author comment from BEFORE the review is not activity" ready
# An unreadable pull request is an error, never "not stalled".
zmk PR_FAIL=1;                   zrefused "a pull request gh cannot read is an error that says so" 'could not read'
zmk THREADS_FAIL=1;              zrefused "unreadable review threads are an error, not silence" 'could not read'
zmk RUNS_FAIL=1;                 zrefused "unreadable workflow runs are an error, not a green board" 'could not read'

# --- stalled: the listing ------------------------------------------------------
zmk REVIEW_AGE=15
out="$(zenv python3 "$root/bin/maintainer" stalled 2>&1)"; rc=$?
[ "$rc" = 0 ] && grep -q '#12' <<<"$out" && grep -q 'contrib' <<<"$out" && grep -q 'rebase' <<<"$out" \
    && ok "stalled lists a pull request that passes every precondition" \
    || bad "stalled did not list the positive control (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-150)"
zmk REVIEW_AGE=9
out="$(zenv python3 "$root/bin/maintainer" stalled 2>&1)"
grep -q '#12' <<<"$out" && bad "stalled listed a review nine days old under the default window" \
    || ok "stalled's default window is FIXUP_QUIET_DAYS (10): a nine-day-old review is not listed"
out="$(zenv python3 "$root/bin/maintainer" stalled 7 2>&1)"
grep -q '#12' <<<"$out" && ok "and an explicit window overrides it" || bad "stalled 7 did not list a nine-day-old review: $out"
out="$(zenv MAINTAINER_FIXUP_QUIET_DAYS=8 python3 "$root/bin/maintainer" stalled 2>&1)"
grep -q '#12' <<<"$out" && ok "MAINTAINER_FIXUP_QUIET_DAYS changes the default" || bad "the env override of the window was ignored: $out"
zmk REVIEW_AGE=15 COMMENT_AFTER=1
out="$(zenv python3 "$root/bin/maintainer" stalled 2>&1)"
grep -q '#12' <<<"$out" && bad "stalled listed a pull request whose author answered" || ok "stalled leaves out a pull request whose author answered"
zmk REVIEW_AGE=15 PR_FAIL=1
out="$(zenv python3 "$root/bin/maintainer" stalled 2>&1)"; rc=$?
[ "$rc" != 0 ] && grep -qi 'could not read' <<<"$out" && ok "stalled exits non-zero and names a pull request it could not read" \
    || bad "an unreadable pull request was reported as not stalled (rc=$rc): $out"

# --- patch policy: what a run's patch may touch --------------------------------
# Each case is ready to push (aged notice) so the refusal is the patch's own.
zready() { zmk NOTICE_AGE=6 "$@"; }
zrepo
ZDOC='[{"kind":"doc-line","file":"docs/guide.md"}]'
zready ITEMS="$ZDOC"; zpatch docs/guide.md 's/line one/line 1/'
zstate "a one-line doc patch on a doc the item names passes the policy" ready
zrm() { rm -f "$zf/state/drafts/r1/fixup-12.patch"; }
zrm; zready ITEMS="$ZDOC"; zrefused "a doc-line item with no patch written by the run is refused" 'patch'
zready ITEMS='[{"kind":"doc-line","file":"src/lib.rs"}]'; zpatch src/lib.rs 's/test_1()/test_one()/'
zrefused "a patch to a PROD_GLOBS path is refused" 'production'
zready ITEMS='[{"kind":"doc-line","file":"Makefile"}]'; zpatch Makefile 's/true/false/'
zrefused "a patch to the Makefile is refused" 'production'
zready ITEMS='[{"kind":"doc-line","file":".github/dependabot.yml"}]'; zpatch .github/dependabot.yml 's/1/2/'
zrefused "a patch under .github/ is refused even when the profile does not call it production" 'under \.github/'
sed -i '/^PROD_GLOBS/d' "$zprof/profile.env"
zready ITEMS="$ZDOC"; zpatch docs/guide.md 's/line one/line 1/'
zrefused "a profile that declares no PROD_GLOBS gets no patch: the check cannot say what is production" 'PROD_GLOBS'
printf 'PROD_GLOBS="src/* Makefile"\n' >> "$zprof/profile.env"
zstate "and the same patch passes once the profile declares them (so that refusal was the declaration's)" ready
zready ITEMS='[{"kind":"doc-line","file":"package.json"}]'; zpatch package.json 's/x/y/'
zrefused "a patch to package.json is refused: the screen says npm would run its scripts" 'screen|build-time|executes'
zready ITEMS='[{"kind":"doc-line","file":"settings.cfg"}]'; zpatch settings.cfg 's/1/2/'
zrefused "a patch to a file the screen cannot classify is refused, not assumed inert" 'classif|inert'
zready ITEMS='[{"kind":"doc-line","file":"README.md"}]'; zpatch docs/guide.md 's/line one/line 1/'
zrefused "a patch to a file no item names is refused" 'not named|no item names'
zready ITEMS="$ZDOC"
( t="$(mktemp -d "$zf/pt.XXXXXX")"; git clone -q --branch fix-x "$zf/fork.git" "$t/w" 2>/dev/null; cd "$t/w" || exit 1
  for i in $(seq 1 25); do printf 'extra %s\n' "$i" >> docs/guide.md; done; git diff > "$zf/state/drafts/r1/fixup-12.patch"; rm -rf "$t" )
zrefused "a patch over FIXUP_MAX_LINES is refused" 'lines|cap|exceeds'
out="$(zcheck MAINTAINER_FIXUP_MAX_LINES=40)"; grep -q '^state=ready$' <<<"$out" \
    && ok "MAINTAINER_FIXUP_MAX_LINES raises the cap, so the limit is not a literal" \
    || bad "the cap override was ignored: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-120)"
zready ITEMS='[{"kind":"doc-line","file":"docs/new.md"}]'
( t="$(mktemp -d "$zf/pt.XXXXXX")"; git clone -q --branch fix-x "$zf/fork.git" "$t/w" 2>/dev/null; cd "$t/w" || exit 1
  printf 'new\n' > docs/new.md; git add -N docs/new.md; git diff > "$zf/state/drafts/r1/fixup-12.patch"; rm -rf "$t" )
zrefused "a patch that creates a file is refused" 'create|new file|rename|summary|mode'
zready ITEMS='[{"kind":"doc-line","file":"docs/moved.md"}]'
( t="$(mktemp -d "$zf/pt.XXXXXX")"; git clone -q --branch fix-x "$zf/fork.git" "$t/w" 2>/dev/null; cd "$t/w" || exit 1
  git mv docs/guide.md docs/moved.md; git add -A; git diff --cached -M > "$zf/state/drafts/r1/fixup-12.patch"; rm -rf "$t" )
zrefused "a patch that renames a file is refused" 'create|rename|summary|mode'
zready ITEMS="$ZDOC"; zpatch docs/guide.md 's/line one/line 1/'
zrm; zready ITEMS="$ZDOC"; zpatch docs/guide.md 's/line one/line 1/'
mv "$zf/state/drafts/r1/fixup-12.patch" "$zf/state/drafts/r1/real.patch"; ln -s real.patch "$zf/state/drafts/r1/fixup-12.patch"
zrefused "a patch that is a symlink is refused" 'symlink|regular'
rm -f "$zf/state/drafts/r1/fixup-12.patch" "$zf/state/drafts/r1/real.patch"

# --- maintainer-repo fixup: the write ------------------------------------------
zfix() { zenv "$@" bash "$root/bin/maintainer-repo" fixup 12 $ZARGS 2>&1; }
ZARGS=""
ztip() { git --git-dir="$zf/fork.git" rev-parse refs/heads/fix-x; }
zhasprefix() { grep -q -- "$1" <<<"$2"; }

# 1. no notice yet: the first call posts one, and pushes nothing.
zrepo; zmk ITEMS='[{"kind":"rebase"}]'; : > "$zf/calls.log"; before="$(ztip)"
out="$(zfix)"; rc=$?
[ "$rc" = 0 ] && grep -q 'pr comment' "$zf/calls.log" && [ "$(ztip)" = "$before" ] \
    && ok "with no notice on the PR, fixup posts the heads-up and pushes nothing" \
    || bad "first call (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"
grep -q "<!-- maintainer: fixup-notice v1 head=$before -->" "$zf/notice.body" 2>/dev/null \
    && grep -q 'rebase' "$zf/notice.body" && grep -Eq '[0-9]{4}-[0-9]{2}-[0-9]{2}' "$zf/notice.body" \
    && ok "the notice carries its marker (with the head), the items and a date" \
    || bad "the notice body is wrong: $(tr '\n' ' ' < "$zf/notice.body" 2>/dev/null | cut -c1-200)"
[ -s "$zf/notice.body" ] && ! grep -qiE 'claude|generated|assistant|\bAI\b' "$zf/notice.body" \
    && ok "the notice says nothing about how it was written" || bad "the notice is missing or discloses automation"
out="$(zfix)"; rc=$?
[ "$rc" != 0 ] && [ "$(ztip)" = "$before" ] && ok "a second call right after is refused: the notice has not aged" \
    || bad "a second call did not wait (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"
# The notice must be visible on the PR afterwards, or "I told them" is a claim.
zrepo; zmk; : > "$zf/nocomment"; : > "$zf/calls.log"
out="$(zfix)"; rc=$?; rm -f "$zf/nocomment"
[ "$rc" != 0 ] && grep -qi 'read it back' <<<"$out" \
    && ok "a notice that does not show up on the PR afterwards is reported as a failure" \
    || bad "an unconfirmed notice was reported as posted (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"
# POST=off rehearses: no comment either.
zrepo; zmk; : > "$zf/calls.log"
out="$(zfix MAINTAINER_POST=off)"; rc=$?
[ "$rc" = 0 ] && ! grep -q 'pr comment' "$zf/calls.log" && grep -q 'POST=off' <<<"$out" \
    && ok "POST=off posts no notice and says it is rehearsing" || bad "POST=off still wrote (rc=$rc): $out"

# 2. ready, rebase only: the branch is rewritten, the contributor stays the author.
zrepo; zmk NOTICE_AGE=6 ITEMS='[{"kind":"rebase"}]'; : > "$zf/calls.log"; before="$(ztip)"
mainhead="$(git --git-dir="$zf/origin.git" rev-parse main)"
out="$(zfix)"; rc=$?; after="$(ztip)"
if [ "$rc" = 0 ] && [ "$after" != "$before" ] && git --git-dir="$zf/fork.git" merge-base --is-ancestor "$mainhead" "$after"; then
    ok "a rebase fixup rewrites the contributor's branch onto the base, and reads the head back"
else bad "rebase fixup (rc=$rc, tip $before -> $after): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-200)"; fi
[ "$after" != "$before" ] && [ "$(git --git-dir="$zf/fork.git" log -1 --format=%an "$after")" = contrib ] \
    && ok "the rebased commit is still the contributor's: credit is kept" || bad "the rebase changed the author"
grep -q "$after" <<<"$out" && ok "success names the new head" || bad "success did not name the head: $out"
# Every gh call the whole thing made, against an allowlist. Nothing here may
# label, close, approve, dismiss, edit or merge, and nothing may touch claimed
# or twir-listed.
zgh_ok() { [ -s "$zf/calls.log" ] && ! grep -qiE '(^| )(pr (close|merge|edit|review|ready|reopen)|issue |label|release|api -X|api --method|--approve|dismiss|claimed|twir-listed)' "$zf/calls.log" \
    && ! grep -qvE '^(auth switch|api user --jq|api users/|pr view|pr comment|api repos/o/r/(pulls|actions)|pr list)' "$zf/calls.log"; }
zgh_ok && ok "the only gh calls were reads and the one notice comment; no label, close, approval or merge" \
    || bad "an unexpected gh call: $(grep -viE '^(auth switch|api user|api users/|pr view|pr comment|api repos/o/r/(pulls|actions)|pr list)' "$zf/calls.log" | head -3 | tr '\n' ' ')"
compgen -G "$zf/state/fixup-*" >/dev/null && bad "the scratch clone was left behind" || ok "the scratch clone is removed afterwards"

# 3. rebase conflict: abort the whole fixup, remote untouched.
ZCONFLICT=1 zrepo; zmk NOTICE_AGE=6 ITEMS='[{"kind":"rebase"}]'; before="$(ztip)"
out="$(zfix)"; rc=$?
[ "$rc" != 0 ] && [ "$(ztip)" = "$before" ] && grep -qi 'conflict' <<<"$out" \
    && ok "a rebase conflict aborts the fixup and the remote is untouched" \
    || bad "a conflicted rebase (rc=$rc, tip moved: $([ "$(ztip)" = "$before" ] && echo no || echo YES)): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"

# 4. test-count patch: derived by the profile's command, committed as the maintainer.
zrepo; ZCOUNT='[{"kind":"test-count","file":"README.md"}]'
zmk NOTICE_AGE=6 ITEMS="$ZCOUNT"; zpatch README.md 's/7 tests/9 tests/'; before="$(ztip)"
out="$(zfix)"; rc=$?; after="$(ztip)"
msg="$(git --git-dir="$zf/fork.git" log -1 --format=%B "$after" 2>/dev/null)"
if [ "$rc" = 0 ] && [ "$after" != "$before" ] && git --git-dir="$zf/fork.git" merge-base --is-ancestor "$before" "$after"; then
    ok "adding a commit is a plain push: the old head is still an ancestor"
else bad "test-count fixup (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-200)"; fi
git --git-dir="$zf/fork.git" show "$after:README.md" 2>/dev/null | grep -q '9 tests' \
    && ok "the patch landed: README says 9 tests, the count the profile's own command derived" || bad "README was not updated"
grep -q '^Co-authored-by: contrib <5678+contrib@users.noreply.github.com>$' <<<"$msg" \
    && ok "the commit credits the author with a Co-authored-by trailer built from the API's id" \
    || bad "no Co-authored-by trailer: $(printf '%s' "$msg" | tr '\n' ' ')"
[ "$(git --git-dir="$zf/fork.git" log -1 --format='%an <%ae>' "$after")" = "owner <1234+owner@users.noreply.github.com>" ] \
    && ok "the commit is the maintainer's, under the account the identity check confirmed" \
    || bad "the commit author is $(git --git-dir="$zf/fork.git" log -1 --format='%an <%ae>' "$after")"
[ -n "$msg" ] && ! grep -qiE 'claude|generated|assistant|co-authored-by: .*(noreply@anthropic|bot)|\bAI\b' <<<"$msg" \
    && ok "no AI attribution in the commit" || bad "the commit message is missing or discloses automation"
[ -n "$msg" ] && ! grep -q '^Signed-off-by:' <<<"$msg" && ok "a repo with no sign-off convention gets none" \
    || bad "a Signed-off-by appeared in a repo that does not use them"

# 5. the count is re-derived, never typed.
zrepo; zmk NOTICE_AGE=6 ITEMS="$ZCOUNT"; zpatch README.md 's/7 tests/8 tests/'; before="$(ztip)"
out="$(zfix)"; rc=$?
[ "$rc" != 0 ] && [ "$(ztip)" = "$before" ] && grep -qiE 'derive|count' <<<"$out" \
    && ok "a count the profile's command does not produce is refused: 8 typed, 9 derived" \
    || bad "a typed count was pushed (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"
sed -i '/^FIXUP_COUNT_CMD/d' "$zprof/profile.env"
zrepo; zmk NOTICE_AGE=6 ITEMS="$ZCOUNT"; zpatch README.md 's/7 tests/9 tests/'; before="$(ztip)"
out="$(zfix)"; rc=$?
[ "$rc" != 0 ] && [ "$(ztip)" = "$before" ] && grep -q 'declares no FIXUP_COUNT_CMD' <<<"$out" \
    && ok "with no FIXUP_COUNT_CMD a count item is refused, and the message names the key" \
    || bad "a count was pushed with no way to derive it (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"
printf 'FIXUP_COUNT_CMD="grep -c '"'"'^fn test_'"'"' src/lib.rs"\n' >> "$zprof/profile.env"

# 6. a patch that does not apply leaves the remote alone.
zrepo; zmk NOTICE_AGE=6 ITEMS="$ZDOC"; zpatch docs/guide.md 's/line one/line 1/'
sed -i 's/^ guide line two/ something else entirely/' "$zf/state/drafts/r1/fixup-12.patch"
sed -i 's/^-guide line one/-not the real line/' "$zf/state/drafts/r1/fixup-12.patch"; before="$(ztip)"
out="$(zfix)"; rc=$?
[ "$rc" != 0 ] && [ "$(ztip)" = "$before" ] && grep -qi 'apply' <<<"$out" && ok "a patch that does not apply is refused and the remote is untouched" \
    || bad "a stale patch (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"

# 7. rehearsals push nothing and show the diff.
zrepo; zmk NOTICE_AGE=6 ITEMS="$ZDOC"; zpatch docs/guide.md 's/line one/line 1/'; before="$(ztip)"; : > "$zf/calls.log"
ZARGS="--dry-run"; out="$(zfix)"; rc=$?; ZARGS=""
[ "$rc" = 0 ] && [ "$(ztip)" = "$before" ] && grep -q '+guide line 1' <<<"$out" \
    && ok "--dry-run prints the plan and the diff and pushes nothing" \
    || bad "--dry-run (rc=$rc, moved: $([ "$(ztip)" = "$before" ] || echo YES)): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"
out="$(zfix MAINTAINER_POST=off)"; rc=$?
[ "$rc" = 0 ] && [ "$(ztip)" = "$before" ] && grep -q 'POST=off' <<<"$out" && grep -q '+guide line 1' <<<"$out" \
    && ok "POST=off is the same rehearsal" || bad "POST=off pushed or printed no diff (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"

# 8. the lease: the branch moved between the check and the push.
zrepo; zmk NOTICE_AGE=6 ITEMS='[{"kind":"rebase"},{"kind":"doc-line","file":"docs/guide.md"}]'; zpatch docs/guide.md 's/line one/line 1/'
cat > "$zf/on_author" <<'HOOK'
#!/usr/bin/env bash
# The contributor pushes while the fixup is being prepared.
t="$(mktemp -d)"; export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
git clone -q --branch fix-x "$FX/fork.git" "$t/w" 2>/dev/null; cd "$t/w" || exit 1
git config user.email c@c; git config user.name c; printf 'late push\n' >> notes.txt
git commit -qam "late push"; git push -q origin fix-x; rm -rf "$t"
HOOK
chmod +x "$zf/on_author"
out="$(zfix)"; rc=$?; rm -f "$zf/on_author"
git --git-dir="$zf/fork.git" log -1 --format=%s refs/heads/fix-x | grep -q 'late push' \
    && [ "$rc" != 0 ] && ok "a rewrite refuses when the remote moved (force-with-lease), and the late push survives" \
    || bad "the fixup overwrote a push it never saw (rc=$rc, tip: $(git --git-dir="$zf/fork.git" log -1 --format=%s refs/heads/fix-x)): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-120)"
# And a move before the clone is caught at the clone.
zrepo; zmk NOTICE_AGE=6 ITEMS='[{"kind":"rebase"}]'
cat > "$zf/on_me" <<'HOOK'
#!/usr/bin/env bash
t="$(mktemp -d)"; export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
git clone -q --branch fix-x "$FX/fork.git" "$t/w" 2>/dev/null; cd "$t/w" || exit 1
git config user.email c@c; git config user.name c; printf 'early push\n' >> notes.txt
git commit -qam "early push"; git push -q origin fix-x; rm -rf "$t"
HOOK
chmod +x "$zf/on_me"
out="$(zfix)"; rc=$?; rm -f "$zf/on_me"
[ "$rc" != 0 ] && git --git-dir="$zf/fork.git" log -1 --format=%s refs/heads/fix-x | grep -q 'early push' && grep -qi 'has moved since the check' <<<"$out" \
    && ok "a head that moved after the check is refused at the clone" \
    || bad "a moved head was rewritten (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-140)"

# 9. success is the pull request's head, not the push's exit code.
zrepo; zmk NOTICE_AGE=6 ITEMS='[{"kind":"rebase"}]'
printf '%s\n' "$(printf 'd%.0s' $(seq 40))" > "$zf/readback"
out="$(zfix)"; rc=$?; rm -f "$zf/readback"
[ "$rc" != 0 ] && grep -qiE 'readback|read back|does not match|reports' <<<"$out" && ! grep -qi 'fixup pushed' <<<"$out" \
    && ok "a head the PR does not report back is a failure, not a success" \
    || bad "an unconfirmed push was reported as done (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"

# 10. the repo's own sign-off convention is followed.
ZSIGN=1 zrepo; zmk NOTICE_AGE=6 ITEMS="$ZDOC"; zpatch docs/guide.md 's/line one/line 1/'
out="$(zfix)"; rc=$?
git --git-dir="$zf/fork.git" log -1 --format=%B refs/heads/fix-x | grep -q '^Signed-off-by: owner <1234+owner@users.noreply.github.com>$' \
    && ok "when the contributor signs off, the fixup signs off too" \
    || bad "the sign-off convention was not followed (rc=$rc): $(git --git-dir="$zf/fork.git" log -1 --format=%B refs/heads/fix-x | tr '\n' ' ')"

# 10b. the account must be the maintainer's, before a comment or a push.
zrepo; zmk; printf '1234\tsomeone\n' > "$zf/me"; : > "$zf/calls.log"; before="$(ztip)"
out="$(zfix)"; rc=$?
[ "$rc" != 0 ] && ! grep -q 'pr comment' "$zf/calls.log" && grep -q 'not owner' <<<"$out" \
    && ok "authenticated as somebody else: no heads-up is posted" || bad "a wrong account got as far as a comment (rc=$rc): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-140)"
zmk NOTICE_AGE=6 ITEMS='[{"kind":"rebase"}]'; out="$(zfix)"; rc=$?
[ "$rc" != 0 ] && [ "$(ztip)" = "$before" ] && grep -q 'not owner' <<<"$out" \
    && ok "and no push is made either" || bad "a wrong account pushed (rc=$rc)"
printf '1234\towner\n' > "$zf/me"

# 11. no contributor code and no hook runs: a hook path set globally must not reach the clone.
zrepo; mkdir -p "$zf/evilhooks"
printf '#!/bin/sh\ntouch "%s/HOOK-RAN"\n' "$zf" > "$zf/evilhooks/pre-commit"; cp "$zf/evilhooks/pre-commit" "$zf/evilhooks/pre-rebase"
cp "$zf/evilhooks/pre-commit" "$zf/evilhooks/post-checkout"; cp "$zf/evilhooks/pre-commit" "$zf/evilhooks/post-rewrite"
chmod +x "$zf"/evilhooks/*
printf '[core]\n\thooksPath = %s/evilhooks\n' "$zf" >> "$zf/gitconfig"
zmk NOTICE_AGE=6 ITEMS='[{"kind":"rebase"},{"kind":"doc-line","file":"docs/guide.md"}]'; zpatch docs/guide.md 's/line one/line 1/'
out="$(zfix)"; rc=$?
[ "$rc" = 0 ] && [ ! -e "$zf/HOOK-RAN" ] && ok "no git hook ran in the scratch clone, though the global config names a hook path" \
    || bad "a hook ran or the control failed (rc=$rc, hook ran: $([ -e "$zf/HOOK-RAN" ] && echo yes || echo no)): $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-140)"
head -n -2 "$zf/gitconfig" > "$zf/gitconfig.new"; mv "$zf/gitconfig.new" "$zf/gitconfig"

# 12. arguments.
out="$(zenv bash "$root/bin/maintainer-repo" fixup 2>&1)"; rc=$?
[ "$rc" != 0 ] && grep -q 'usage: maintainer-repo.*fixup' <<<"$out" && ok "fixup with no pull request number prints usage" || bad "fixup with no number (rc=$rc): $out"
out="$(zenv bash "$root/bin/maintainer-repo" fixup '12; rm -rf x' 2>&1)"; rc=$?
[ "$rc" != 0 ] && grep -q 'usage: maintainer-repo.*fixup' <<<"$out" && ok "a pull request number that is not a number is refused" || bad "a non-numeric number was accepted (rc=$rc): $out"
zrepo

# --- the prompts, the defaults and the docs agree with the code ----------------
for zp in sysknife magent _template; do
    zf_prompt="$root/profiles/$zp/prompts/review.md"
    grep -q 'maintainer: fixup-items v1 head=' "$zf_prompt" && grep -q '`maintainer stalled`' "$zf_prompt" \
        && grep -q 'maintainer-repo fixup <pr> --dry-run' "$zf_prompt" \
        && ok "$zp: the review prompt tells the run to write the marker and to run stalled and fixup" \
        || bad "$zp: its review prompt lacks the fixup marker or the stalled/fixup steps"
done
# The marker the prompts show must be one the parser accepts, or every review
# written from them is substantive and the whole path is dead.
python3 -B - "$root" <<'PYEOF' && ok "the marker example in the prompts parses, kinds and all" || bad "the prompts' marker example is rejected by the parser"
import importlib.machinery, importlib.util, re, sys
root = sys.argv[1]
sys.argv = ["maintainer", "--help"]
loader = importlib.machinery.SourceFileLoader("m", root + "/bin/maintainer")
m = importlib.util.module_from_spec(importlib.util.spec_from_loader("m", loader))
loader.exec_module(m)
for prof in ("sysknife", "magent", "_template"):
    t = open(f"{root}/profiles/{prof}/prompts/review.md").read()
    line = next(l for l in t.splitlines() if l.startswith("    <!-- maintainer: fixup-items v1"))
    line = line.strip().replace("<the 40-hex head you reviewed>", "a" * 40)
    head, items = m._fixup_items("Words.\n\n" + line)
    assert head == "a" * 40 and [k for k, _ in items] == ["rebase", "test-count"], (prof, items)
PYEOF
for zk in FIXUP_QUIET_DAYS FIXUP_NOTICE_DAYS FIXUP_MAX_LINES; do
    if sc_case 1; then
    grep -q "^$zk=[0-9]\+\$" "$root/lib/defaults.env" && ! grep -nE "(^|[^A-Z_])$zk *= *[0-9]" "$root/bin/maintainer" "$root/bin/maintainer-repo" >/dev/null \
        && ok "$zk has its default in lib/defaults.env and no literal in the code" \
        || bad "$zk is missing from lib/defaults.env or has a literal in bin/"
    sc_end; fi
done

# ===== Z: stalled pull requests (end) =====
# --- branch Y: leases (begin) ---------------------------------------------------
