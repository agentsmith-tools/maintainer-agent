#!/usr/bin/env bash
# Part of the offline suite: pin comments, the verify container, coverage, receipts, thinking budget and the MCP server.
# tests/run-tests.sh sources this file in one shell, after its own preamble and after
# the parts before it, so every helper and variable it reads comes from there. It is
# sourced, never run on its own, and carries a shebang so the shell sweeps pick it up.

# The parts share one shell. These names are set by the driver or by an earlier part;
# declare only tells the linter they are read here, and assigns and changes nothing.
declare dep_state
declare dp
declare mg
declare stub_dir

echo "== a pin comment names a tag GitHub has, not a path GitHub normalises =="
# The same class as sysknife's GHSA-j754. The classifier took a pin's comment as
# `\S+` and verify-deps asked `repos/<action>/commits/<comment>`. GitHub resolves
# `commits/pull/1/head` to a pull request's head, which anyone can push, and
# normalises `..` in the path before it resolves anything, so a pin to a
# stranger's commit commented `pull/1/head`, or `v4.2.2/../pull/1/head`, passed
# as the tag it never was. Measured 2026-10-28 against actions/checkout:
# `commits/pull/1/head` answers bf8f6208, and `git/ref/tags/../pull/1/head`
# answers `refs/pull/1/head`. So the comment is checked for shape, and the tag
# is asked for as a tag and must come back as exactly that tag.
pin_patch() {  # $1 = file to write, $2 = the comment after `#` on the added pin, $3 = the action
    cat > "$1" <<PATCH
diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -10,7 +10,7 @@ jobs:
-        uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683  # v4.2.2
+        uses: ${3:-actions/checkout}@bf8f62083c41b3cb36f52c4100ad20ba98400ea6  # $2
PATCH
}
for c in 'v4.2.2/../../pull/1/head' '../pull/1/head' 'v4.2.2/..' '/v4.2.2' 'v4.2//2' \
         'v4.2.2/' './v4' 'v4.2.2;id' 'v4.2.2%2F..' '-v4' '../pull/1/head (branch)'; do
    pin_patch "$dp/tag-shape.patch" "$c"
    vout="$(dep_case "$dp/tag-shape.patch")"
    if grep -q '"verdict": "refused"' <<<"$vout" && grep -q '"path": ".github/workflows/ci.yml"' <<<"$vout" \
       && grep -qF -- "${c% (branch)}" <<<"$(grep '"line"' <<<"$vout")"; then
        ok "the classifier refuses the pin comment '$c' and names the file and the line"
    else
        bad "the pin comment '$c' was not refused with its file and line: $(tr -d '\n' <<<"$vout" | cut -c1-160)"
    fi
done
# The shapes a real tag or branch takes, so the refusal above is about the path
# and not about slashes or dots in general.
for c in 'v4.2.2' 'v2.87.14' '3.99.4' 'codeql-bundle/v2.20.0' 'v1.0.0+build.7' 'stable (branch)' 'release/2026 (branch)'; do
    pin_patch "$dp/tag-shape.patch" "$c"
    vout="$(dep_case "$dp/tag-shape.patch")"
    grep -q '"verdict": "dependency-bump"' <<<"$vout" \
        && ok "the classifier accepts the ordinary pin comment '$c'" \
        || bad "an ordinary pin comment '$c' was refused: $(tr -d '\n' <<<"$vout" | cut -c1-160)"
done
# The action half of the same line is interpolated into the same API path.
for a in 'actions/checkout/../../evil/x' '../evil' 'actions/./checkout' 'actions//checkout' 'actions/check%2Fout'; do
    pin_patch "$dp/tag-shape.patch" 'v4.2.2' "$a"
    vout="$(dep_case "$dp/tag-shape.patch")"
    grep -q '"verdict": "refused"' <<<"$vout" && grep -qF -- "$a" <<<"$(grep '"line"' <<<"$vout")" \
        && ok "the classifier refuses the action path '$a'" \
        || bad "the action path '$a' passed the classifier: $(tr -d '\n' <<<"$vout" | cut -c1-160)"
done

# End to end through verify-deps, with a gh stub that answers each endpoint the
# way GitHub does, logs every call, and answers `.ref` only when the jq asks for
# it: the old code asked `commits/<tag> --jq .sha` and must fail by passing the
# pin, not by choking on an answer meant for another question.
pin_calls="$stub_dir/pin-calls"
pin_sha=bf8f62083c41b3cb36f52c4100ad20ba98400ea6
pin_scripts="$stub_dir/pinscripts"; mkdir -p "$pin_scripts"
# A classifier that lets one pin through as declared, for the cases that prove
# verify-deps does not depend on the classifier having caught the shape.
cat > "$pin_scripts/classify_dependency_bump.py" <<'STUB'
import json, os, sys
sys.stdin.read()
json.dump({"verdict": "dependency-bump", "files": {".github/workflows/ci.yml": "workflow"},
           "changed_lines": 2, "pins": json.loads(os.environ["FAKE_PINS"]), "images": []}, sys.stdout)
STUB
pin_gate() {  # $1 = patch; PG_REF, PG_TAGOBJ, PG_HEAD, PG_SHA, PG_CMP = what each endpoint answers (404 = not found)
    rm -f "$dep_state/receipts/9.json"; : > "$pin_calls"
    cat > "$stub_dir/gh" <<GHEOF
#!/usr/bin/env bash
printf '%s\n' "\$*" >> '$pin_calls'
nf() { echo 'gh: Not Found (HTTP 404)' >&2; exit 1; }
case "\$*" in
  *'auth switch'*) exit 0 ;;
  *'api user'*) echo testuser ;;
  *headRefOid*) echo cafe1234cafe1234 ;;
  *'pr diff'*'--patch'*) echo 'the gate asked for --patch' >&2; exit 9 ;;
  *'pr diff'*) cat '$1' ;;
  *'/git/ref/tags/'*'.ref'*) [ '${PG_REF:-404}' = 404 ] && nf; printf '%b\n' '${PG_REF:-}' ;;
  *'/git/tags/'*) [ '${PG_TAGOBJ:-404}' = 404 ] && nf; printf '%b\n' '${PG_TAGOBJ:-}' ;;
  *'/git/ref/heads/'*'.ref'*) [ '${PG_HEAD:-404}' = 404 ] && nf; printf '%s\n' '${PG_HEAD:-}' ;;
  *'/commits/'*'.sha'*) printf '%s\n' '${PG_SHA:-}' ;;
  *'/compare/'*) printf '%s\n' '${PG_CMP:-}' ;;
  *) exit 1 ;;
esac
GHEOF
    chmod +x "$stub_dir/gh"
    # Bounded, because a mutant of the annotated-tag loop never ends on its own.
    PATH="$stub_dir:$PATH" MAINTAINER_STATE="$dep_state" MAINTAINER_ACCOUNT=testuser \
        MAINTAINER_SLUG=o/r MAINTAINER_REPO="$stub_dir/repo" \
        MAINTAINER_SCRIPTS="${PG_SCRIPTS:-$root/scripts}" FAKE_PINS="${FAKE_PINS:-[]}" \
        timeout 60 bash "$root/bin/maintainer-merge" verify-deps 9 cafe1234cafe1234 2>&1
}

# The shape the real classifier still lets through: `pull/1/head` is a legal ref
# name. GitHub has no TAG by that name, which is the question that settles it.
pin_patch "$dp/pr-head.patch" 'pull/1/head'
pout="$(PG_REF=404 PG_SHA="$pin_sha" pin_gate "$dp/pr-head.patch")"; prc=$?
if [ "$prc" != 0 ] && [ ! -f "$dep_state/receipts/9.json" ] && grep -q 'pull/1/head' <<<"$pout" \
   && grep -q '/git/ref/tags/pull/1/head' "$pin_calls"; then
    ok "verify-deps asks for the comment as a tag, and a pull request head is not one"
else
    bad "a pin to a pull request head, commented pull/1/head, earned a receipt (rc=$prc): $(tr '\n' ' ' <<<"$pout" | cut -c1-160)"
fi
grep -q '/commits/' "$pin_calls" \
    && bad "verify-deps still resolves a tag through commits/<tag>, which follows any ref" \
    || ok "and it never asks commits/<tag>, which resolves branches and pull heads alike"

# A ref that comes back as something other than the tag asked for. This is what
# GitHub answers live for `git/ref/tags/../pull/1/head`, reached here through a
# classifier that let the shape through.
pout="$(PG_SCRIPTS="$pin_scripts" FAKE_PINS="[{\"action\":\"actions/checkout\",\"sha\":\"$pin_sha\",\"tag\":\"../pull/1/head\",\"branch\":false}]" \
        PG_REF="refs/pull/1/head\\tcommit\\t$pin_sha" PG_SHA="$pin_sha" pin_gate "$dp/pr-head.patch")"; prc=$?
if [ "$prc" != 0 ] && [ ! -f "$dep_state/receipts/9.json" ] && grep -q 'refs/pull/1/head' <<<"$pout"; then
    ok "verify-deps refuses a tag GitHub answers with a ref that is not refs/tags/<tag>"
else
    bad "a ref GitHub resolved to refs/pull/1/head passed as the tag (rc=$prc): $(tr '\n' ' ' <<<"$pout" | cut -c1-160)"
fi

# An annotated tag names a tag object, and the tag object names the commit.
pin_patch "$dp/annotated.patch" 'v9.9.9'
pout="$(PG_REF="refs/tags/v9.9.9\\ttag\\t$(printf 'a%.0s' {1..40})" PG_TAGOBJ="commit\\t$pin_sha" PG_SHA="$pin_sha" \
        pin_gate "$dp/annotated.patch")"; prc=$?
if [ "$prc" = 0 ] && [ -f "$dep_state/receipts/9.json" ] && grep -q '/git/tags/a\{40\}' "$pin_calls"; then
    ok "an annotated tag is dereferenced to its commit, and the pin on that commit earns a receipt"
else
    bad "an annotated tag on the pinned commit earned no receipt (rc=$prc): $(tr '\n' ' ' <<<"$pout" | cut -c1-160)"
fi
pout="$(PG_REF="refs/tags/v9.9.9\\ttag\\t$(printf 'a%.0s' {1..40})" PG_TAGOBJ="commit\\t$(printf '0%.0s' {1..40})" PG_SHA="$pin_sha" \
        pin_gate "$dp/annotated.patch")"; prc=$?
[ "$prc" != 0 ] && [ ! -f "$dep_state/receipts/9.json" ] && grep -q 'pin comment is not true' <<<"$pout" \
    && ok "an annotated tag on another commit is refused" \
    || bad "an annotated tag on another commit earned a receipt (rc=$prc)"
pout="$(PG_REF="refs/tags/v9.9.9\\ttag\\t$(printf 'a%.0s' {1..40})" PG_TAGOBJ="tag\\t$(printf 'a%.0s' {1..40})" PG_SHA="$pin_sha" \
        pin_gate "$dp/annotated.patch")"; prc=$?
[ "$prc" != 0 ] && [ "$prc" != 124 ] && [ ! -f "$dep_state/receipts/9.json" ] && grep -qi 'nested' <<<"$pout" \
    && ok "a tag that keeps naming tags is refused after a bounded number of reads" \
    || bad "a cycle of tag objects was not refused in bounds (rc=$prc): $(tr '\n' ' ' <<<"$pout" | cut -c1-120)"
pout="$(PG_REF="refs/tags/v9.9.9\\ttree\\t$pin_sha" PG_SHA="$pin_sha" pin_gate "$dp/annotated.patch")"; prc=$?
[ "$prc" != 0 ] && [ ! -f "$dep_state/receipts/9.json" ] && grep -q 'tree' <<<"$pout" \
    && ok "a tag that names a tree rather than a commit is refused" \
    || bad "a tag naming a tree earned a receipt (rc=$prc)"

# A branch pin: the branch has to exist under exactly that name before the
# compare is asked, or the compare answers about whatever the name resolved to.
pout="$(PG_SCRIPTS="$pin_scripts" FAKE_PINS="[{\"action\":\"actions/checkout\",\"sha\":\"$pin_sha\",\"tag\":\"../pull/1/head\",\"branch\":true}]" \
        PG_HEAD=refs/pull/1/head PG_CMP=identical pin_gate "$dp/pr-head.patch")"; prc=$?
if [ "$prc" != 0 ] && [ ! -f "$dep_state/receipts/9.json" ] && grep -q 'refs/pull/1/head' <<<"$pout"; then
    ok "a branch pin whose name GitHub resolves to another ref is refused"
else
    bad "a branch pin resolved to refs/pull/1/head earned a receipt (rc=$prc): $(tr '\n' ' ' <<<"$pout" | cut -c1-160)"
fi
grep -q '/compare/' "$pin_calls" \
    && bad "verify-deps asked compare before it knew the branch was a branch" \
    || ok "and the compare is never asked about a name that is not a branch"
pin_patch "$dp/branch-ok.patch" 'stable (branch)'
pout="$(PG_HEAD=refs/heads/stable PG_CMP=behind pin_gate "$dp/branch-ok.patch")"; prc=$?
[ "$prc" = 0 ] && [ -f "$dep_state/receipts/9.json" ] && grep -q '/git/ref/heads/stable' "$pin_calls" \
    && ok "a branch pin on a real branch is checked by name, then by compare, and earns a receipt" \
    || bad "a branch pin on a real branch earned no receipt (rc=$prc): $(tr '\n' ' ' <<<"$pout" | cut -c1-160)"
rm -f "$stub_dir/gh"

echo "== the built suite image on this host is the one the tree describes =="
# sysknife#471 could not earn a receipt because the shell suite's image had no
# yamllint. Adding it to the Containerfile changes nothing until somebody
# rebuilds, and doctor could not tell an image built from today's recipe from
# one built in August: `podman image exists` answers yes to both. So the build
# stamps the recipe's hash on the image and doctor compares it.
ih="$stub_dir/imghome"
mkdir -p "$ih/.local/share/maintainer/profiles/p/verify.d" "$ih/.local/bin"
cat > "$ih/.local/share/maintainer/profiles/p/verify.d/shell.sh" <<'SUITE'
suite_image() { printf 'localhost/probe:1'; }
suite_needs() { printf 'bash'; }
SUITE
printf 'FROM docker.io/library/busybox:latest
'     > "$ih/.local/share/maintainer/profiles/p/verify.d/shell.Containerfile"
: > "$ih/.local/share/maintainer/profiles/p/profile.env"
img_recipe="$(sha256sum "$ih/.local/share/maintainer/profiles/p/verify.d/shell.Containerfile" | cut -d' ' -f1)"
img_doctor() {  # $1 = what `podman image inspect` reports as the fingerprint
    make_stub podman "case \"\$*\" in
        *'image exists'*) exit 0 ;;
        *'image inspect'*) printf '%s\\n' '$1'; exit 0 ;;
        *) exit 0 ;;
    esac"
    PATH="$stub_dir:$PATH" HOME="$ih" MAINTAINER_PROFILE=p \
        bash "$root/bin/maintainer-doctor" 2>&1
}
iout="$(img_doctor "0000000000000000000000000000000000000000000000000000000000000000")"
grep -q 'was built from a different' <<<"$iout" \
    && ok "doctor catches an image built from a recipe the tree no longer has" \
    || bad "an image built from a stale Containerfile reads as healthy"
iout="$(img_doctor '<no value>')"
grep -qi 'fingerprint' <<<"$iout" \
    && ok "doctor says so when an image carries no recipe fingerprint at all" \
    || bad "an unlabelled image reads as in sync, which is the un-rebuilt case"
iout="$(img_doctor "$img_recipe")"
grep -q 'was built from a different' <<<"$iout" \
    && bad "doctor reports drift for an image built from exactly this recipe" \
    || ok "an image whose fingerprint matches the recipe raises nothing"
if sc_case 1; then
grep -q 'maintainer.recipe=' "$root/bin/maintainer-doctor" \
    && ok "doctor's rebuild command stamps the fingerprint it will later check" \
    || bad "doctor tells you to build an image it cannot later recognise"
sc_end; fi
# An environment with no USER. `podman run` sets none, and `set -u` then killed
# maintainer-doctor at the lingering check, halfway through its report and
# before Container verification ever printed. The two image-recipe cases above
# are exactly what fails when that happens, so the offline suite reported
# `923 passed, 2 failed` inside its own verify container and neither failure
# said anything about what those cases test. `maintainer-merge verify` read the
# failing clean run and refused, correctly, which is how every pull request
# against this repository's production code became unverifiable. Found on #107.
#
# Through the stubbed podman above: a real one here would build container
# storage under the test's temporary HOME and leave it behind.
iout="$(env -u USER PATH="$stub_dir:$PATH" HOME="$ih" MAINTAINER_PROFILE=p \
    bash "$root/bin/maintainer-doctor" 2>&1)"
grep -q 'unbound variable' <<<"$iout" \
    && bad "doctor dies on an unset USER instead of reporting" \
    || ok "doctor survives an environment that exports no USER"
grep -q 'Container verification' <<<"$iout" \
    && ok "and reaches the end of its report with USER unset" \
    || bad "doctor stops before Container verification when USER is unset"
# The same read sits on the unattended runner's notification path, which is the
# worst place for a crash: it fires only when something has already failed.
if sc_case 2; then
bare_user="$(grep -rn '[$]USER' "$root/bin" "$root/lib" "$root/scripts" "$root/install.sh" 2>/dev/null \
    | grep -v 'USER:-' || true)"
[ -z "$bare_user" ] \
    && ok "no shipped script reads USER without a fallback" \
    || bad "an unset USER would kill these under set -u: $(printf '%s' "$bare_user" | tr '\n' ' ' | cut -c1-160)"
grep -rq 'USER:-' "$root/bin" "$root/lib" \
    && ok "and the fallback form is present, so the check above can match" \
    || bad "no USER:- anywhere, so the anti-pattern grep proves nothing"
sc_end; fi
rm -f "$stub_dir/podman"
command -v podman >/dev/null 2>&1 \
    && ok "the podman stub is gone, so the cases below see the real one" \
    || noenv "no podman on this host; the container cases below will skip"

echo "== every production file a suite covers can be mutated =="
# apply_mutation runs `find -name "<glob>"`, so a mutate glob matches a
# BASENAME, not a path. magent's shell suite covered `bin/*`, `lib/*` and
# `scripts/*` and mutated `*.sh`, and every command in this repository lives in
# bin/ with no extension. A pull request touching one passed the coverage check,
# changed nothing when the mutation ran, and cmd_verify refused with "the
# mutation changed nothing" - a correct refusal that made the class
# unverifiable. Driven against this repository, which is what the magent profile
# points at.
mg_suite="$root/profiles/magent/verify.d/shell.sh"
# shellcheck disable=SC1090  # the suite file is chosen at runtime, by design
mg_globs_raw="$( . "$mg_suite"; suite_mutate_glob )"
mapfile -t mg_globs <<< "$mg_globs_raw"
mg_prod_raw="$( . "$root/profiles/magent/profile.env" >/dev/null 2>&1; printf '%s' "${PROD_GLOBS:-}" )"
read -r -a mg_prod <<< "$mg_prod_raw"
mg_files="$(git -C "$root" ls-files 2>/dev/null)"
[ -n "$mg_files" ] && [ "${#mg_prod[@]}" -gt 0 ] \
    && ok "the mutate-glob check has a file list and a PROD_GLOBS to read" \
    || bad "no tracked files or no PROD_GLOBS; the check below would be vacuous"
mg_checked=0; mg_unreachable=""
while IFS= read -r f; do
    [ -n "$f" ] || continue
    isprod=0
    # shellcheck disable=SC2254  # PROD_GLOBS entries ARE globs; matching them literally would defeat the check
    for pat in "${mg_prod[@]}"; do case "$f" in $pat) isprod=1; break ;; esac; done
    [ "$isprod" = 1 ] || continue
    # shellcheck disable=SC1090
    ( . "$mg_suite"; suite_covers "$f" ) || continue
    mg_checked=$((mg_checked + 1))
    base="${f##*/}"; hit=0
    # shellcheck disable=SC2254  # a mutate glob is a glob
    for g in "${mg_globs[@]}"; do [ -n "$g" ] || continue; case "$base" in $g) hit=1; break ;; esac; done
    [ "$hit" = 1 ] || mg_unreachable="$mg_unreachable $f"
done <<< "$mg_files"
[ "$mg_checked" -gt 0 ] \
    && ok "$mg_checked production file(s) are covered by the shell suite" \
    || bad "no production file matched both PROD_GLOBS and suite_covers; the check is vacuous"
[ -z "$mg_unreachable" ] \
    && ok "and every one of them can be reached by a mutation" \
    || bad "no mutation can land on:$(printf '%s' "$mg_unreachable" | cut -c1-200)"

echo "== the verify container can run a script the test itself wrote =="
# sysknife#410's release test writes an "echoing scanner" stub into mktemp -d,
# chmod +x's it, and runs it to prove the real scanner never prints the
# credential it found. The gate mounted /tmp with `noexec`, so the stub could
# not run, the assertion never fired, and the gate reported:
#
#   maintainer-merge: the test does not pass unmutated (rc=1)
#       FAIL: the no-echo mutation did not make the assertion fail
#
# The same test passes in the same image with the same tree when /tmp is not
# noexec. That is lesson 62 for the third time: the gate could not run, and it
# reported the contributor as failing.
#
# noexec buys nothing here. The container has no network, so nothing can be
# fetched, and the suite command is the pull request's own code by design, so
# the PR can already execute whatever it likes. What noexec removes is the
# ordinary write-a-stub-and-run-it pattern that both this repo's suite and
# sysknife's use.
if sc_case 3; then
grep -q 'tmpfs "\?/tmp' "$root/bin/maintainer-merge" \
    && ok "the verify container gives the suite a writable /tmp" \
    || bad "no tmpfs for /tmp, so a test that needs scratch space cannot run"
grep -qE 'tmpfs "?/tmp:[^"]*noexec' "$root/bin/maintainer-merge" \
    && bad "/tmp is mounted noexec, so a test that writes and runs a stub reports the contributor as failing" \
    || ok "/tmp is executable, so a stub the test wrote can run"
grep -qE 'tmpfs "?/tmp:[^"]*nosuid' "$root/bin/maintainer-merge" \
    && ok "and /tmp is still nosuid" \
    || bad "nosuid was dropped along with noexec"
sc_end; fi
for _f in --cap-drop=ALL --security-opt=no-new-privileges --read-only --network=none; do
    if sc_case 1; then
    grep -q -- "$_f" "$root/bin/maintainer-merge" \
        && ok "the verify container still passes $_f" \
        || bad "$_f was dropped from the verify container"
    sc_end; fi
done

echo "== a clean run that dies on the environment is not the pull request's fault =="
# Twice the gate has reported a contributor as failing their own test, in public
# and under a real name, when the truth was that the verify image lacked
# something: python:3.12-slim had no git, /tmp was mounted noexec. The third
# instance was a dependency's build script, which the old pattern missed
# entirely: rust:1-slim has no pkg-config and no glib, so a --workspace build
# reaching the Tauri crate dies before a test runs.
envlog="$stub_dir/envfail.log"
cat > "$envlog" <<'LOG'
The system library `glib-2.0` required by crate `glib-sys` was not found.
The file `glib-2.0.pc` needs to be installed and the PKG_CONFIG_PATH environment
variable must contain its parent directory.
error: failed to run custom build command for `glib-sys v0.18.1`
warning: build failed, waiting for other jobs to finish...
LOG
# shellcheck disable=SC1090
if sc_case 9; then
( . "$mg" 2>/dev/null; looks_environmental "$envlog" ) \
    && ok "a missing system library reads as the suite environment" \
    || bad "a dependency build failure is reported as the pull request failing its own test"
for _shape in 'bash: git: command not found' \
              '/tmp/x.sh: Permission denied' \
              'error while loading shared libraries: libssl.so.3' \
              'PermissionError: [Errno 1] Operation not permitted' \
              'ERROR: required tool is missing: cargo'; do
    printf '%s\n' "$_shape" > "$envlog"
    # shellcheck disable=SC1090
    ( . "$mg" 2>/dev/null; looks_environmental "$envlog" ) \
        && ok "and so does: $_shape" \
        || bad "an environmental failure was read as the pull request's: $_shape"
done
# The other direction matters more. A real failing assertion must stay the pull
# request's, or the gate excuses every red run it cannot parse.
cat > "$envlog" <<'LOG'
test result: FAILED. 1 passed; 1 failed
assertion `left == right` failed: GetAptPins: Ubuntu-only description must match
LOG
# shellcheck disable=SC1090
( . "$mg" 2>/dev/null; looks_environmental "$envlog" ) \
    && bad "a failing assertion mentioning nothing environmental was excused" \
    || ok "and a plain assertion failure is not excused"
printf 'FAIL: docs.yml action is not pinned to a 40-hex SHA\n' > "$envlog"
# shellcheck disable=SC1090
( . "$mg" 2>/dev/null; looks_environmental "$envlog" ) \
    && bad "a real pin failure was excused as an environment problem" \
    || ok "and a real supply-chain failure is still the pull request's"
cat > "$envlog" <<'LOG'
test result: FAILED. 1 passed; 1 failed
assertion `left == right` failed: GetAptPins: Ubuntu-only description must match
LOG
# shellcheck disable=SC1090
( . "$mg" 2>/dev/null; looks_environmental "$envlog" ) \
    && bad "a failing assertion was excused as an environment problem" \
    || ok "and a failing assertion is still the pull request's"
sc_end; fi

echo "== coverage is required of production paths, which is what a receipt binds =="
# Issue #19. Six of seven merges on 2026-09-10 fell back to an asserted receipt
# for the same reason: the pull request touched .rs and .md, no single suite runs
# both, and the gate refused to write one. Meanwhile cmd_merge already declares,
# through PROD_GLOBS, which paths invalidate a receipt, and .md is not one of
# them. The tool was refusing to prove something on the grounds that it could not
# prove something else it does not need.
#
# Driven through pick_suite rather than grepped out of its source. A grep for
# "is_production" passes against a function that returns the wrong answer, which
# is the shape this file exists to stop.
u9="$stub_dir/union19"; rm -rf "$u9"; mkdir -p "$u9/verify.d"
cat > "$u9/verify.d/rust.sh" <<'SUITE'
# shellcheck shell=bash
suite_covers() { case "$1" in *.rs|Cargo.lock) return 0 ;; esac; return 1; }
SUITE
cat > "$u9/verify.d/docs.sh" <<'SUITE'
# shellcheck shell=bash
suite_covers() { case "$1" in *.md) return 0 ;; esac; return 1; }
SUITE
u9run() {  # $1 = space-separated PROD_GLOBS, $2... = changed paths -> stdout, stderr in $u9err
    u9named "" "$@"
}
u9named() {  # $1 = suite name or empty, $2 = PROD_GLOBS, $3... = changed paths
    local pick="$1" globs="$2"; shift 2
    PROFILE_DIR="$u9" MAINTAINER_PROFILE=u9 MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
        MAINTAINER_ACCOUNT=t MAINTAINER_MERGE_SOURCE_ONLY=1 PROD_GLOBS="$globs" \
        bash -c '. "'"$root"'/bin/maintainer-merge"; pick_suite "$@"' _ "$pick" "$@" \
        2>"$u9err"
}
u9err="$u9/stderr"
pg='crates/*/src/* Cargo.lock'

# The case from issue #19: production .rs plus documentation, no single suite.
if sc_case 14; then
out="$(u9run "$pg" crates/x/src/lib.rs README.md)"; rc=$?
[ "$rc" = 0 ] && [ "$out" = rust ] \
    && ok "a receipt is earnable when one suite covers every production path" \
    || bad "pick_suite still refuses the .rs + .md union (rc=$rc, stdout='$out')"
grep -qE 'not covered by the receipt|proves nothing about' "$u9err" \
    && ok "and it says on stderr that the receipt does not speak for the rest" \
    || bad "a narrower receipt would be written with nothing saying it is narrower"
# Named INSIDE the narrowing message, not merely present somewhere in stderr:
# the refusal also lists every path, so a bare grep for the filename passes
# against a tool that never narrowed anything.
u9tail="$(sed -n '/not covered by the receipt/,$p' "$u9err")"
grep -q 'README\.md' <<<"$u9tail" \
    && ok "and names the paths it did not prove" \
    || bad "the unproven paths are not named: $(tr '\n' ' ' < "$u9err" | cut -c1-110)"
[ "$(printf '%s' "$out" | wc -l)" = 0 ] && [ "$out" = rust ] \
    && ok "and the suite name reaches stdout on one line, unpolluted by the explanation" \
    || bad "stdout carries more than the suite name, which its caller captures"

# A non-production path the chosen suite DOES run is proved like any other, so
# listing it as unproven would understate the receipt as badly as the old refusal
# overstated what it needed.
out="$(u9run "$pg" crates/x/src/lib.rs tests/helper.rs README.md)"; rc=$?
u9tail="$(sed -n '/not covered by the receipt/,$p' "$u9err")"
[ "$rc" = 0 ] && grep -q 'README\.md' <<<"$u9tail" && ! grep -q 'tests/helper\.rs' <<<"$u9tail" \
    && ok "and a non-production path the suite does run is not called unproven" \
    || bad "the unproven list does not match what the chosen suite covers: $(tr '\n' ' ' < "$u9err" | cut -c1-120)"

# Naming the suite by hand takes the same narrowing. The unattended run on
# 2026-09-14 passed `rust` explicitly and was refused by a branch that had its
# own copy of the coverage rule, so the fix reached one caller and not the other.
out="$(u9named rust "$pg" crates/x/src/lib.rs README.md)"; rc=$?
[ "$rc" = 0 ] && [ "$out" = rust ] \
    && ok "and a named suite is narrowed the same way an inferred one is" \
    || bad "naming the suite still refuses the union (rc=$rc, stdout='$out')"
out="$(u9named docs "$pg" crates/x/src/lib.rs README.md)"; rc=$?
[ "$rc" != 0 ] \
    && ok "and a named suite that misses a production path is still refused" \
    || bad "naming a suite bought coverage it does not have (returned '$out')"

# The invariant this function exists for. A path NO suite runs must still refuse,
# so an incomplete PROD_GLOBS cannot become a way to smuggle a file past the gate.
out="$(u9run "$pg" crates/x/src/lib.rs weird.xyz)"; rc=$?
[ "$rc" != 0 ] \
    && ok "and a path no suite covers at all is still refused" \
    || bad "the narrowing let an unknown file through (returned '$out')"

# A production path that no suite covers is the other half of the same rule.
out="$(u9run "packaging/*" packaging/thing README.md)"; rc=$?
[ "$rc" != 0 ] \
    && ok "and an uncovered production path is refused" \
    || bad "a receipt was issued for production code nothing ran (returned '$out')"

# A diff with NOTHING production in it is the same rule with the other sign.
# sysknife#422 changes one test file, the evidence artifact the test count lives
# in, and the three prose figures that have to move with it. No path is in
# PROD_GLOBS, the rust suite runs the .rs and the json, the docs suite runs the
# .md, and the gate refused to write any receipt at all:
#
#   no single suite covers every changed path:
#       crates/sysknife-daemon/tests/action_reference_doc.rs   rust
#       README.md                                              docs
#
# Issue #19's lesson is that the tool must not refuse to prove something on the
# grounds that it cannot prove something it does not need. With zero production
# paths that applies harder, not less: cmd_merge re-checks PROD_GLOBS and this
# diff touches none of it, so there is nothing a narrower receipt fails to
# speak for. The old `${#prod[@]} -gt 0` precondition switched the narrowing
# off in exactly the case that needs it most.
out="$(u9named rust "$pg" crates/x/tests/helper.rs README.md)"; rc=$?
[ "$rc" = 0 ] && [ "$out" = rust ] \
    && ok "a named suite narrows a diff that touches no production path at all" \
    || bad "pick_suite refused a named suite on a diff with no production path (rc=$rc, stdout='$out')"
u9tail="$(sed -n '/not covered by the receipt/,$p' "$u9err")"
grep -q 'README\.md' <<<"$u9tail" \
    && ok "and still names the paths that receipt does not speak for" \
    || bad "a narrower receipt was written with nothing saying it is narrower"

# Inference stays refused here, on purpose. With a production path there is one
# right answer: the suite that runs it. With none, rust covers the test file and
# docs covers the prose, both qualify, and picking for the caller would write a
# receipt about the claim screen when the change under review is test code.
# Refusing and naming the candidates is the honest answer.
out="$(u9run "$pg" crates/x/tests/helper.rs README.md)"; rc=$?
[ "$rc" != 0 ] && grep -qE 'docs|rust' "$u9err" \
    && ok "and an ambiguous diff refuses to guess a suite, naming the candidates" \
    || bad "pick_suite guessed a suite with nothing to anchor the choice (returned '$out')"

# The orphan rule is what stops that becoming a way through. This is the case
# the assertion below used to cover by accident: it passed because nothing was
# production, and it has to pass because notes.rst is a file no suite runs.
out="$(u9run "$pg" README.md notes.rst)"; rc=$?
[ "$rc" != 0 ] \
    && ok "and a diff with no production path is still refused when a path is covered by nothing" \
    || bad "pick_suite invented a scope for a diff it cannot narrow (returned '$out')"

# Narrowing must not become "any suite will do". The chosen suite has to run
# something, or the receipt is about a file the pull request did not change.
out="$(u9named docs "$pg" crates/x/tests/helper.rs)"; rc=$?
[ "$rc" != 0 ] \
    && ok "and a suite that runs none of the changed paths is refused even with no production path" \
    || bad "a suite bought a receipt for a diff it does not touch (returned '$out')"
sc_end; fi

echo "== a receipt may be earned by several suites when no single one covers production =="
# sysknife#429 changed crates/*/src/*.rs, packaging/*, Makefile and
# .github/workflows/ci.yml in one pull request. The rust suite runs the .rs and,
# through helper_install_coverage.rs, the packaging helpers and the Makefile.
# Nothing in rust reads a workflow; the shell suite's release tests do. So every
# production path was covered by SOME suite and no single suite covered them
# all, and pick_suite refused, exactly as it refused the .rs + .md union before
# issue #19. That fix taught one suite to ignore paths it does not need. This is
# the other half: production paths it DOES need, spread across two suites.
#
# The union is decided where the receipt is consumed, at merge, not at verify.
# A verify proves one part; merge is the step that has to know the parts add up.
u10="$stub_dir/union429"; rm -rf "$u10"; mkdir -p "$u10/verify.d"
cat > "$u10/verify.d/rust.sh" <<'SUITE'
# shellcheck shell=bash
suite_covers() { case "$1" in *.rs|packaging/*|Makefile) return 0 ;; esac; return 1; }
SUITE
cat > "$u10/verify.d/shell.sh" <<'SUITE'
# shellcheck shell=bash
suite_covers() { case "$1" in *.sh|.github/*) return 0 ;; esac; return 1; }
SUITE
cat > "$u10/verify.d/docs.sh" <<'SUITE'
# shellcheck shell=bash
suite_covers() { case "$1" in *.md) return 0 ;; esac; return 1; }
SUITE
u10err="$u10/stderr"
u10cover() {  # $1 = PROD_GLOBS, $2 = suite names, $3... = changed paths
    local globs="$1" suites="$2"; shift 2
    PROFILE_DIR="$u10" MAINTAINER_PROFILE=u10 MAINTAINER_SLUG=o/r MAINTAINER_REPO=/tmp \
        MAINTAINER_ACCOUNT=t MAINTAINER_MERGE_SOURCE_ONLY=1 PROD_GLOBS="$globs" \
        bash -c '. "'"$root"'/bin/maintainer-merge"; suites_cover_production "$@"' _ "$suites" "$@" \
        2>"$u10err"
}
pg429='crates/*/src/* packaging/* Makefile .github/*'
p429=(crates/d/src/actions/ssh.rs packaging/sysknife-sudoers Makefile .github/workflows/ci.yml CHANGELOG.md)

if sc_case 7; then
u10cover "$pg429" "rust shell" "${p429[@]}"; rc=$?
[ "$rc" = 0 ] \
    && ok "two suites together cover a production diff neither covers alone" \
    || bad "the union of rust and shell was refused (rc=$rc): $(tr '\n' ' ' < "$u10err" | cut -c1-140)"

# Each half alone must still be refused, or the union check is decorative.
u10cover "$pg429" rust "${p429[@]}"; rc=$?
[ "$rc" != 0 ] && grep -q '\.github/workflows/ci\.yml' "$u10err" \
    && ok "and the rust half alone is refused, naming the workflow it never runs" \
    || bad "rust alone bought coverage of .github (rc=$rc): $(tr '\n' ' ' < "$u10err" | cut -c1-140)"
u10cover "$pg429" shell "${p429[@]}"; rc=$?
[ "$rc" != 0 ] && grep -qE 'ssh\.rs' "$u10err" \
    && ok "and the shell half alone is refused, naming the daemon source it never runs" \
    || bad "shell alone bought coverage of crates/ (rc=$rc): $(tr '\n' ' ' < "$u10err" | cut -c1-140)"

# The invariant the whole gate rests on: a production path NO named suite runs
# is refused however many suites are named. Adding suites must never become a
# way to accumulate permission rather than coverage.
u10cover "$pg429 build/*" "rust shell docs" "${p429[@]}" build/out.bin; rc=$?
[ "$rc" != 0 ] && grep -q 'build/out\.bin' "$u10err" \
    && ok "and a production path no named suite covers is refused whatever else is named" \
    || bad "three suites waved through a file none of them run (rc=$rc)"

# A suite named in the receipt that does not exist on disk must fail closed.
# Reading a missing suite file as "covers nothing" would be harmless; reading it
# as "covers everything" would not, and a `.` of a missing file leaves the
# PREVIOUS suite's suite_covers defined, which is exactly that failure.
u10cover "$pg429" "rust ghost" "${p429[@]}"; rc=$?
[ "$rc" != 0 ] \
    && ok "and a suite named in a receipt but missing on disk refuses rather than inherits" \
    || bad "a missing suite file was read as coverage (rc=$rc)"

# A profile with no PROD_GLOBS cannot answer the question, and a refusal that
# prints nothing is indistinguishable from one that looked and found nothing.
u10cover "" "rust shell" "${p429[@]}"; rc=$?
[ "$rc" != 0 ] && grep -q 'PROD_GLOBS' "$u10err" \
    && ok "and a profile declaring no PROD_GLOBS refuses out loud rather than in silence" \
    || bad "the empty-PROD_GLOBS refusal said nothing (rc=$rc)"

# Docs-only: nothing production, so there is nothing for the union to cover and
# the receipt's scope question does not arise.
u10cover "$pg429" docs README.md docs/x.md; rc=$?
[ "$rc" = 0 ] \
    && ok "and a diff with no production path needs no production coverage" \
    || bad "a docs-only diff was refused for not covering production (rc=$rc)"
sc_end; fi

echo "== the shell suite really runs, fails on a mutation, and writes a receipt =="
# End to end against a repository built here, because forcing this through a
# real pull request proved only that a badly chosen mutation is refused.
# podman or docker, whichever this machine has. Requiring podman made CI red for
# a reason unrelated to the change: the runners have docker.
rt=""
command -v podman >/dev/null 2>&1 && rt=podman
[ -z "$rt" ] && command -v docker >/dev/null 2>&1 && rt=docker
if [ -n "$rt" ] && ( "$rt" image inspect docker.io/library/bash:5 >/dev/null 2>&1 \
                     || "$rt" pull -q docker.io/library/bash:5 >/dev/null 2>&1 ); then
    vr="$stub_dir/verifyrepo"; rm -rf "$vr"; mkdir -p "$vr"
    git -C "$vr" init -q -b main; git -C "$vr" config user.email t@t; git -C "$vr" config user.name t
    printf '#!/usr/bin/env bash\nGUARD=on\nif [ "$GUARD" = on ]; then echo "guard held"; exit 0; fi\necho "guard gone"; exit 1\n' \
        > "$vr/check.sh"
    git -C "$vr" add -A; git -C "$vr" commit -qm base
    verify_head="$(git -C "$vr" rev-parse HEAD)"
    git -C "$vr" update-ref "refs/pull/42/head" "$verify_head"
    git -C "$vr" remote add origin "$vr"
    make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *files*) echo check.sh;;
    esac"
    out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$stub_dir/vstate" MAINTAINER_ACCOUNT=testuser \
          MAINTAINER_SLUG=o/r MAINTAINER_REPO="$vr" PROFILE_DIR="$root/profiles/sysknife" \
          bash "$mg" verify 42 "$verify_head" check.sh 's/GUARD=on/GUARD=off/' shell 2>&1)
    if grep -q 'receipt recorded\|observed' <<<"$out"; then
        ok "the shell suite produced an observed receipt"
    else
        bad "shell verify failed: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-120)"
    fi
    # A python helper and a test that needs the repository, because both were
    # gaps the gate had in anger on 2026-09-14: sysknife#401's production change
    # is one regex in a .py file that no suite covered and no mutate glob could
    # reach, and sysknife#425's clean run died on `fatal: not a git repository`
    # because the tree arrives through `git archive` with no index.
    printf 'GUARD = "on"\nimport sys\nif GUARD == "on":\n    print("guard held"); sys.exit(0)\nprint("guard gone"); sys.exit(1)\n' \
        > "$vr/helper.py"
    printf '#!/usr/bin/env bash\ngit ls-files >/dev/null 2>&1 || { echo "git could not enumerate tracked files"; exit 1; }\npython3 helper.py\n' \
        > "$vr/pycheck.sh"
    git -C "$vr" add -A; git -C "$vr" commit -qm python
    verify_head2="$(git -C "$vr" rev-parse HEAD)"
    git -C "$vr" update-ref "refs/pull/43/head" "$verify_head2"
    make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *files*) printf 'pycheck.sh\nhelper.py\n';;
    esac"
    out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$stub_dir/vstate3" MAINTAINER_ACCOUNT=testuser \
          MAINTAINER_SLUG=o/r MAINTAINER_REPO="$vr" PROFILE_DIR="$root/profiles/sysknife" \
          bash "$mg" verify 43 "$verify_head2" pycheck.sh 's/GUARD = "on"/GUARD = "off"/' shell 2>&1)
    grep -q 'not a git repository\|could not enumerate tracked files' <<<"$out" \
        && bad "the extracted tree has no git index, so a test that asks git what is tracked fails on the gate" \
        || ok "a test that asks git what is tracked runs on the extracted tree"
    grep -q 'receipt recorded\|observed' <<<"$out" \
        && ok "a mutation reaches a python file, so a .py production change can be proved" \
        || bad "the python mutation earned no receipt: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-140)"
    # Restore the stub the cases below expect.
    make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *files*) echo check.sh;;
    esac"
    # And the same run must refuse when the mutation changes nothing.
    out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$stub_dir/vstate2" MAINTAINER_ACCOUNT=testuser \
          MAINTAINER_SLUG=o/r MAINTAINER_REPO="$vr" PROFILE_DIR="$root/profiles/sysknife" \
          bash "$mg" verify 42 "$verify_head" check.sh 's/NOTHING/MATCHES/' shell 2>&1)
    # "The mutation changed nothing" and "the guard does not bite" are
    # different findings, and this assertion used to accept the wrong one for
    # the right outcome. The sysknife review of 2026-09-23 11:16 hit the real
    # shape: a sed aimed at CONTRIBUTING.md, which the shell suite's
    # suite_mutate_glob does not list, so no file was touched, the mutated run
    # passed, and the gate reported the contributor's guard as not biting. That
    # is MISTAKES.md rule 4, a mutation that did not apply, inside the tool that
    # exists to enforce it.
    grep -q 'changed nothing' <<<"$out" \
        && ok "a mutation that matches no line is refused as a no-op"
    grep -q 'changed nothing' <<<"$out" \
        || bad "a no-op mutation is not named as one: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-140)"
    grep -q 'THE GUARD DOES NOT BITE' <<<"$out" \
        && bad "a no-op mutation is still reported as a guard that does not bite" \
        || ok "and it is not blamed on the guard"

    # WHERE the mutation landed. A suite's mutate glob is about file types, not
    # about trust: sysknife's rust suite mutates '*.rs', which is every test file
    # as well as every source file, and the shell suite mutates '*.sh' the same
    # way. So a mutation aimed at the contributor's own test used to satisfy
    # every check above and earn a receipt, having proved that a test notices its
    # own edit.
    #
    # Observed twice in one sysknife session on 2026-09-29: an MCP boundary test
    # called the sanitiser directly instead of the route, so deleting the wiring
    # left it green, and the same review flagged the identical shape in a
    # contributor's pull request an hour earlier. Neither showed up in a test
    # count or a diff read.
    mkdir -p "$vr/scripts"
    printf '#!/usr/bin/env bash\nPROD_GUARD=on\necho "thing ran"\n[ "$PROD_GUARD" = on ] || exit 1\n' \
        > "$vr/scripts/thing.sh"
    # A separate entry point, because check.sh is shared with the cases below
    # and overwriting it made one of them fail for a reason that had nothing to
    # do with what it tests.
    printf '#!/usr/bin/env bash\nTEST_MARKER=one\necho "prodcheck ran"\nbash scripts/thing.sh\n' \
        > "$vr/prodcheck.sh"
    git -C "$vr" add -A; git -C "$vr" commit -qm prod
    verify_head_prod="$(git -C "$vr" rev-parse HEAD)"
    git -C "$vr" update-ref "refs/pull/44/head" "$verify_head_prod"
    make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *files*) printf 'prodcheck.sh\nscripts/thing.sh\n';;
    esac"
    # scripts/* is production for the sysknife profile; check.sh at the root is
    # not. A mutation that only reaches the test must be refused.
    # PROFILE_DIR does not source profile.env, and the suite exports
    # PROD_GLOBS="crates/*/src/*" globally, which nothing in this fixture
    # matches. Declare the set this case is about, or the check under test is
    # skipped and the assertion below passes for the wrong reason.
    out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$stub_dir/vstate5" MAINTAINER_ACCOUNT=testuser \
          MAINTAINER_SLUG=o/r MAINTAINER_REPO="$vr" PROFILE_DIR="$root/profiles/sysknife" \
          PROD_GLOBS="scripts/*" \
          bash "$mg" verify 44 "$verify_head_prod" prodcheck.sh 's/TEST_MARKER=one/TEST_MARKER=two/' shell 2>&1)
    grep -q 'landed only outside production code' <<<"$out" \
        && ok "a mutation that only touches a test earns no receipt" \
        || bad "a test-only mutation was accepted: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-400)"
    grep -q 'receipt recorded' <<<"$out" \
        && bad "a test-only mutation wrote a receipt" \
        || ok "and no receipt was written for it"
    # The refusal must name what it saw, or it is one more unexplained no.
    grep -q 'prodcheck.sh' <<<"$out" \
        && ok "the refusal names the file the mutation did reach" \
        || bad "the refusal does not say what was mutated"
    # And the same run, aimed at the production file, still works. Without this
    # the guard above could be a blanket refusal and the test would not know.
    out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$stub_dir/vstate6" MAINTAINER_ACCOUNT=testuser \
          MAINTAINER_SLUG=o/r MAINTAINER_REPO="$vr" PROFILE_DIR="$root/profiles/sysknife" \
          PROD_GLOBS="scripts/*" \
          bash "$mg" verify 44 "$verify_head_prod" prodcheck.sh 's/PROD_GUARD=on/PROD_GUARD=off/' shell 2>&1)
    grep -q 'receipt recorded\|observed' <<<"$out" \
        && ok "a mutation that breaks the production file still earns a receipt" \
        || bad "the production mutation was refused: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"
    # Production code, but not the code this pull request changed. The check above
    # asked only whether the mutation hit production; a mutation of any other
    # production file proves a test notices a break somewhere in the repository,
    # and the receipt would then speak for a change nothing here broke. The pull
    # request below changes scripts/thing.sh and prodcheck2.sh; the test guards
    # scripts/other.sh, which it did not touch.
    printf '#!/usr/bin/env bash\nOTHER_GUARD=on\n[ "$OTHER_GUARD" = on ] || exit 1\n' > "$vr/scripts/other.sh"
    printf '#!/usr/bin/env bash\necho "prodcheck2 ran"\nbash scripts/other.sh\n' > "$vr/prodcheck2.sh"
    git -C "$vr" add -A; git -C "$vr" commit -qm prod2
    verify_head_prod2="$(git -C "$vr" rev-parse HEAD)"
    git -C "$vr" update-ref "refs/pull/44/head" "$verify_head_prod2"
    make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *files*) printf 'prodcheck2.sh\nscripts/thing.sh\n';;
    esac"
    out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$stub_dir/vstate7" MAINTAINER_ACCOUNT=testuser \
          MAINTAINER_SLUG=o/r MAINTAINER_REPO="$vr" PROFILE_DIR="$root/profiles/sysknife" \
          PROD_GLOBS="scripts/*" \
          bash "$mg" verify 44 "$verify_head_prod2" prodcheck2.sh 's/OTHER_GUARD=on/OTHER_GUARD=off/' shell 2>&1)
    grep -q 'production code this pull request did not change' <<<"$out" \
        && ok "a mutation of production code the pull request did not change earns no receipt" \
        || bad "an unrelated production mutation was accepted: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-300)"
    grep -q 'receipt recorded' <<<"$out" \
        && bad "an unrelated production mutation wrote a receipt" \
        || ok "and no receipt was written for it"
    grep -q 'scripts/other.sh' <<<"$out" && grep -q 'scripts/thing.sh' <<<"$out" \
        && ok "the refusal names the file that was mutated and the files the pull request changed" \
        || bad "the refusal does not name both sides: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-300)"
    # The same mutation, once the pull request does change that file, still earns
    # a receipt: the check is an intersection, not a blanket refusal.
    make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *files*) printf 'prodcheck2.sh\nscripts/other.sh\n';;
    esac"
    out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$stub_dir/vstate8" MAINTAINER_ACCOUNT=testuser \
          MAINTAINER_SLUG=o/r MAINTAINER_REPO="$vr" PROFILE_DIR="$root/profiles/sysknife" \
          PROD_GLOBS="scripts/*" \
          bash "$mg" verify 44 "$verify_head_prod2" prodcheck2.sh 's/OTHER_GUARD=on/OTHER_GUARD=off/' shell 2>&1)
    grep -q 'receipt recorded\|observed' <<<"$out" \
        && ok "the same mutation earns a receipt once the pull request changes that file" \
        || bad "a mutation of a changed production file was refused: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-200)"
    # Restore the stub the cases below expect.
    make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *files*) echo check.sh;;
    esac"
    # The shape that actually happened: the file exists, the sed matches it, and
    # the suite's mutate glob does not list it.
    printf 'the guard holds\n' > "$vr/NOTES.md"
    git -C "$vr" add -A; git -C "$vr" commit -qm notes
    verify_head4="$(git -C "$vr" rev-parse HEAD)"
    git -C "$vr" update-ref "refs/pull/44/head" "$verify_head4"
    out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$stub_dir/vstate4" MAINTAINER_ACCOUNT=testuser \
          MAINTAINER_SLUG=o/r MAINTAINER_REPO="$vr" PROFILE_DIR="$root/profiles/sysknife" \
          bash "$mg" verify 44 "$verify_head4" check.sh 's/the guard holds/the guard is gone/' shell 2>&1)
    grep -q 'changed nothing' <<<"$out" \
        && ok "a mutation aimed outside the suite's mutate glob is refused as a no-op" \
        || bad "a mutation that touched no file was read as a verdict about the guard: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-140)"
else
    # One skip per case the if-branch would have run, so the suite reports the
    # same number of cases on every machine. Emitting a single line here made
    # the total differ by one between this laptop and a container, and
    # scripts/check_claims.sh compares that total against the README.
    noenv "the shell suite produced an observed receipt (needs podman or docker)"
    noenv "a test that asks git what is tracked runs on the extracted tree (needs podman or docker)"
    noenv "a mutation reaches a python file, so a .py production change can be proved (needs podman or docker)"
    noenv "a mutation that matches no line is refused as a no-op (needs podman or docker)"
    noenv "and it is not blamed on the guard (needs podman or docker)"
    noenv "a mutation aimed outside the suite's mutate glob is refused as a no-op (needs podman or docker)"
fi
echo "== verify with a container runtime that runs the suite command on the host: pinned and unpinned mutations =="
# The gate needs a container only to run the suite's command in a copy of the tree. This stub
# runs that command in the directory mounted at /repo, so the whole verify (extraction, the symlink
# check, the clean run, the mutation, the mutated run, the receipt) runs here against a repository
# built in this shell, with no image to pull. A test that fails with the mutation applied is
# pinned and earns a signed receipt; one that still passes is refused by name. The same cases run
# against the Rust gate when MX_TREE holds one.
sv="$stub_dir/stubverify"; rm -rf "$sv"; mkdir -p "$sv/profile/verify.d" "$sv/repo" "$sv/bin"
sv_stub() { printf '#!/usr/bin/env bash\n%s\n' "$2" > "$sv/bin/$1"; chmod +x "$sv/bin/$1"; }
cat > "$sv/profile/verify.d/stub.sh" <<'SUITE'
suite_covers() { case "$1" in check.sh) return 0 ;; esac; return 1; }
suite_image() { printf 'docker.io/library/bash:5'; }
suite_needs() { printf 'bash'; }
suite_mutate_glob() { printf '%s\n' 'check.sh'; }
suite_command() { printf 'bash check.sh\n'; }
suite_ran() { local n; n="$(grep -c '^guard held' "$1" 2>/dev/null || true)"; printf '%s' "${n:-0}"; }
SUITE
sv_stub podman 'work=""; prev=""; cmd=""
for a in "$@"; do
  case "$prev" in -v) case "$a" in *:/repo:z) work="${a%:/repo:z}" ;; esac ;; esac
  prev="$a"; cmd="$a"
done
cd "$work" || exit 125
exec sh -c "$cmd"'
sv_stub gh "case \"\$*\" in
  *'auth switch'*) exit 0;;
  *'api user'*) echo testuser;;
  *files*) echo check.sh;;
esac"
sr="$sv/repo"
git -C "$sr" init -q -b main; git -C "$sr" config user.email t@t; git -C "$sr" config user.name t
git -C "$sr" remote add origin "$sr"
printf '#!/usr/bin/env bash\nGUARD=on\nif [ "$GUARD" = on ]; then echo "guard held"; exit 0; fi\necho "guard gone"; exit 1\n' > "$sr/check.sh"
git -C "$sr" add -A; git -C "$sr" commit -qm pinned
sv_pinned="$(git -C "$sr" rev-parse HEAD)"; git -C "$sr" update-ref refs/pull/51/head "$sv_pinned"
printf '#!/usr/bin/env bash\nGUARD=on\necho "guard held"; exit 0\n' > "$sr/check.sh"
git -C "$sr" add -A; git -C "$sr" commit -qm unpinned
sv_unpinned="$(git -C "$sr" rev-parse HEAD)"; git -C "$sr" update-ref refs/pull/52/head "$sv_unpinned"
sv_run() {  # $1 = pr, $2 = head, $3 = state suffix
    PATH="$sv/bin:$stub_dir:$PATH" MAINTAINER_STATE="$sv/state$3" MAINTAINER_ACCOUNT=testuser \
        MAINTAINER_SLUG=o/r MAINTAINER_REPO="$sr" PROFILE_DIR="$sv/profile" PROD_GLOBS=check.sh \
        bash "$mg" verify "$1" "$2" guard 's/GUARD=on/GUARD=off/' stub 2>&1
}
out="$(sv_run 51 "$sv_pinned" a)"
if grep -q "receipt recorded for #51 at ${sv_pinned:0:8} (observed: clean pass, mutated fail)" <<<"$out"; then
    ok "a test that fails with the mutation applied earns an observed receipt"
else
    bad "a pinned mutation earned no receipt: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"
fi
if grep -q '"kind": "observed"' "$sv/statea/receipts/51.json" 2>/dev/null \
   && grep -q '"observed_failure": "guard gone"' "$sv/statea/receipts/51.json"; then
    ok "and the receipt records the line the mutated run failed on"
else
    bad "the pinned receipt is missing or does not carry the observed failure"
fi
out="$(sv_run 52 "$sv_unpinned" b)"
if grep -q "THE GUARD DOES NOT BITE: 'guard' still passes with the mutation applied." <<<"$out"; then
    ok "a test that still passes with the mutation applied is refused by name"
else
    bad "an unpinned mutation was not refused as one: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"
fi
[ ! -e "$sv/stateb/receipts/52.json" ] \
    && ok "and no receipt is written for it" \
    || bad "a receipt exists for a guard that does not bite"
out="$(sv_run 51 deadbee c)"
if grep -q "head is $sv_pinned, not the deadbee you named" <<<"$out" && [ ! -e "$sv/statec/receipts/51.json" ]; then
    ok "a head that is not the pull request's head is refused and nothing is recorded"
else
    bad "a wrong head was not refused as one: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"
fi
echo "== a dependency bump can earn a receipt, and the test run still has no network =="
# #437, #438 and #439 sat six days each. The rust suite runs `cargo test
# --offline` against the host cache, and a lockfile bump is by definition a
# version that cache does not hold, so the clean run died on "attempting to
# make an HTTP request, but --offline was specified" and no dependabot pull
# request could ever be verified. Fetching on the host was the wrong fix:
# `maintainer screen 438` names that risk by name.
rsuite="$root/profiles/sysknife/verify.d/rust.sh"
# shellcheck disable=SC1090
( . "$rsuite" >/dev/null 2>&1; declare -F suite_prefetch_command >/dev/null ) \
    && ok "the rust suite declares how to fetch what the lockfile pins" \
    || bad "the rust suite has no prefetch, so no dependency bump can be verified"
# shellcheck disable=SC1090
( . "$rsuite" >/dev/null 2>&1; declare -F suite_prefetch_guard >/dev/null ) \
    && ok "and a guard that runs before the container gets network" \
    || bad "the prefetch reaches the network with nothing screening what it may fetch"
# The guard decides on the lockfile, which lists every source the fetch can
# reach. crates.io only: a pull request cannot point the fetch somewhere else.
pfx="$stub_dir/prefetch"; rm -rf "$pfx"; mkdir -p "$pfx/clean" "$pfx/git" "$pfx/nolock"
printf '[[package]]\nname = "uuid"\nversion = "1.26.1"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n' \
    > "$pfx/clean/Cargo.lock"
printf '[[package]]\nname = "uuid"\nversion = "1.26.1"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n\n[[package]]\nname = "sneaky"\nversion = "0.1.0"\nsource = "git+https://example.invalid/sneaky.git#deadbeef"\n' \
    > "$pfx/git/Cargo.lock"
# shellcheck disable=SC1090
gout=$( . "$rsuite" >/dev/null 2>&1; suite_prefetch_guard "$pfx/clean" 2>&1 ); grc=$?
[ "$grc" = 0 ] && ok "a crates.io-only lockfile is allowed to fetch" \
    || bad "the guard refused an ordinary lockfile (rc=$grc): $gout"
# shellcheck disable=SC1090
gout=$( . "$rsuite" >/dev/null 2>&1; suite_prefetch_guard "$pfx/git" 2>&1 ); grc=$?
if [ "$grc" != 0 ] && grep -q 'example.invalid' <<<"$gout"; then
    ok "a git source in the lockfile refuses the fetch, and the refusal quotes it"
else
    bad "a lockfile pointing the fetch at an arbitrary git host was allowed (rc=$grc)"
fi
# Could not ask is not an answer: no lockfile means nothing pins the download.
# shellcheck disable=SC1090
gout=$( . "$rsuite" >/dev/null 2>&1; suite_prefetch_guard "$pfx/nolock" 2>&1 ); grc=$?
if [ "$grc" != 0 ] && grep -q 'Cargo.lock' <<<"$gout"; then
    ok "no lockfile refuses the fetch rather than fetching whatever resolves"
else
    bad "a tree with no Cargo.lock was allowed to fetch (rc=$grc)"
fi
# The host cache must stay out of it: downloaded crates go to an overlay
# upperdir that dies with the verify.
# shellcheck disable=SC1090
oargs=$( . "$rsuite" >/dev/null 2>&1; suite_podman_args podman "$pfx/scratch" | tr '\n' ' ' )
grep -q 'upperdir=' <<<"$oargs" \
    && ok "new crates land in an overlay upperdir, not in ~/.cargo" \
    || bad "the fetch would write into the host cargo cache: $oargs"
# And every path the suite asks for has to be under the scratch it was handed.
# The first version put the upperdir inside the extracted tree, where the
# mutation glob is *.rs and the upperdir is full of other people's .rs files:
# a mutation could land in a dependency. podman also creates the overlay
# workdir mode 000, so `find` over the tree died with Permission denied and
# #438 came back "mutation failed to apply". Measured, not reasoned about.
stray=""
for _tok in $oargs; do
    case "$_tok" in
        *upperdir=*|*workdir=*)
            for _kv in ${_tok//,/ }; do
                case "$_kv" in
                    upperdir=*|workdir=*)
                        case "${_kv#*=}" in "$pfx/scratch"/*) ;; *) stray="$stray ${_kv}" ;; esac ;;
                esac
            done ;;
    esac
done
[ -z "$stray" ] \
    && ok "the overlay's upperdir and workdir stay inside the scratch the gate handed over" \
    || bad "the suite writes outside its scratch, where the mutation glob can reach:$stray"
# The gate must hand over a scratch, never the tree the mutation walks.
if sc_case 2; then
grep -q 'suite_extra_args "$rt" "$work"' "$mg" \
    && bad "the gate passes the extracted tree as suite scratch" \
    || ok "the gate passes a scratch directory, not the extracted tree"

grep -q 'container_runtime()' "$mg" && ok "the gate accepts podman or docker" \
    || bad "the gate requires one specific container runtime"
sc_end; fi
grep -q 'suite_requires' "$root/profiles/sysknife/verify.d/rust.sh" \
    && ok "a suite that needs a specific runtime says so" \
    || bad "the rust suite does not declare its podman requirement"
# A fragment with no shebang leaves shellcheck unable to pick a dialect, which
# made the lint gate red rather than telling anyone what was wrong with the code.
for f in "$root"/profiles/*/verify.d/*.sh; do
    head -1 "$f" | grep -q 'shellcheck shell=' \
        && ok "$(basename "$(dirname "$(dirname "$f")")")/$(basename "$f") declares its shell" \
        || bad "$f has no shebang and no shellcheck shell directive"
done

echo "== the thinking budget reaches the process, per task =="
# Reasoning effort is not a CLI flag: Claude Code reads MAX_THINKING_TOKENS from
# the environment. A setting nobody confirmed reached the process is the same
# shape as the MAINTAINER_FORCE leak, so this measures rather than greps.
tb="$stub_dir/thinkstub"; mkdir -p "$tb"
printf '#!/usr/bin/env bash\nenv | grep -E "^MAX_THINKING_TOKENS=" >&2\nexit 0\n' > "$tb/claude"
chmod +x "$tb/claude"
tlog="$stub_dir/think.log"
think_for() {  # $1 = task -> the budget the backend exported
    : > "$tlog"
    ( export PATH="$tb:$PATH" MAINTAINER_TASK="$1" PROFILE_DIR="$root/profiles/sysknife"
      # shellcheck disable=SC1091
      . "$root/profiles/sysknife/profile.env"
      # shellcheck disable=SC1091
      . "$root/lib/backends/claude.sh"
      backend_run /dev/null opus "$tlog" /tmp >/dev/null 2>&1 )
    sed -n 's/^MAX_THINKING_TOKENS=//p' "$tlog" | head -1
}
if sc_case 3; then
r_think="$(think_for review)"; c_think="$(think_for ci)"
[ -n "$r_think" ] && ok "review exports a thinking budget ($r_think)" \
    || bad "no MAX_THINKING_TOKENS reached the process for review"
[ -n "$c_think" ] && [ "$c_think" != "$r_think" ] \
    && ok "ci gets its own, smaller budget ($c_think)" \
    || bad "every task got the same budget; the per-task setting does nothing"
grep -q 'MAX_THINKING_TOKENS' "$root/lib/backends/claude.sh" \
    && ok "the backend names the mechanism rather than implying a flag" \
    || bad "the thinking budget is not wired into the claude backend"
sc_end; fi

echo "== the MCP server speaks the protocol, and exposes no way round the gate =="
mcp="$root/bin/maintainer-mcp"
[ -x "$mcp" ] && ok "maintainer-mcp is present and executable" || bad "no MCP server"
session() { printf '%s\n' "$@" | python3 "$mcp" 2>/dev/null; }
out=$(session '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
              '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
              '{"jsonrpc":"2.0","id":3,"method":"resources/list"}')
grep -q '"protocolVersion"' <<<"$out" && ok "initialize answers with a protocol version" \
    || bad "initialize did not answer"
grep -q '"tools"' <<<"$out" && ok "tools/list answers" || bad "tools/list did not answer"
grep -q '"resources"' <<<"$out" && ok "resources/list answers" || bad "resources/list did not answer"

# The refusals must survive the change of interface. An MCP client is driven by
# a model, so anything exposed here is exposed to a model.
tools=$(session '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | python3 -c "
import json,sys
for l in sys.stdin:
    m=json.loads(l)
    if 'result' in m and 'tools' in m['result']:
        print(' '.join(t['name'] for t in m['result']['tools']))")
# Named exactly, not by substring: the first version of this check flagged
# maintainer_release_check, which only reports whether a release is owed and
# cannot cut one. A crude needle produces a finding about the needle.
for forbidden in maintainer_receipt maintainer_release maintainer_publish \
                 maintainer_push maintainer_tag maintainer_exec maintainer_shell; do
    printf ' %s ' "$tools" | grep -q " $forbidden " \
        && bad "MCP exposes $forbidden" \
        || ok "MCP exposes no $forbidden"
done
# And the tools it does expose must not be able to publish. release_check reads
# the CHANGELOG and says which digit moves; it never tags.
grep -q '"release-check"' "$mcp" && ok "release_check only asks maintainer-repo, which never tags" \
    || bad "the release tool does something other than release-check"
printf ' %s ' "$tools" | grep -q ' maintainer_verify ' \
    && ok "verify is exposed (it is the only way to earn a receipt)" \
    || bad "verify is missing, so a receipt can never be earned over MCP"
# A model supplies these arguments. They are validated, not interpolated.
out=$(session '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"maintainer_merge","arguments":{"pr":"7; rm -rf /"}}}')
grep -q 'positive integer' <<<"$out" && ok "a non-integer pull request number is refused" \
    || bad "MCP accepted a non-integer pr"
out=$(session '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"maintainer_status","arguments":{"profile":"../../etc"}}}')
grep -q 'plain name' <<<"$out" && ok "a profile name that is a path is refused" \
    || bad "MCP accepted a path as a profile name"
out=$(session '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nope","arguments":{}}}')
grep -q 'no tool named' <<<"$out" && ok "an unknown tool is refused" || bad "unknown tool not refused"
# prune over MCP is always a dry run: deleting is a decision.
grep -q '"prune", "--dry-run"' "$mcp" && ok "prune over MCP is always a dry run" \
    || bad "MCP could delete branches"
grep -q 'maintainer-merge", "merge"' "$mcp" \
    && ok "merge shells out to the gate rather than reimplementing it" \
    || bad "MCP does not route merges through maintainer-merge"

echo "== the receipt dies on the paths THIS repository calls production =="
# The globs were sysknife's directory names, so on any other repository they
# matched nothing: a pull request could earn a receipt at one head, push a
# rewritten bin/ at the next, and the gate would say "no production diff,
# receipt still applies". A merge against a receipt describing a tree that is
# gone is the exact failure the receipt exists to prevent.
for pe in "$root"/profiles/*/profile.env; do
    pn="$(basename "$(dirname "$pe")")"
    grep -q '^PROD_GLOBS=' "$pe" && ok "$pn declares which paths are production" \
        || bad "$pn declares no PROD_GLOBS, so a receipt can survive a rewrite"
done
if sc_case 1; then
grep -q 'PROD_GLOBS:-' "$root/bin/maintainer-merge" \
    && ok "the gate reads the globs from the profile" \
    || bad "the gate still carries a hardcoded production path list"
sc_end; fi
# Executed, not grepped: a merge with no production paths declared must refuse.
out=$(env -u PROD_GLOBS PATH="$stub_dir:$PATH" MAINTAINER_STATE="$stub_dir/state" \
      MAINTAINER_ACCOUNT=testuser MAINTAINER_SLUG=o/r MAINTAINER_REPO="$stub_dir/repo" \
      bash "$mg" merge 1 2>&1); rc=$?
if [ "$rc" != 0 ] && grep -q 'declares no PROD_GLOBS' <<<"$out"; then
    ok "a profile with no globs is refused a merge, not given a default"
else
    bad "the gate merged, or failed for another reason, with no production paths declared (rc=$rc)"
fi

# Executed, against a real repository, on a path THIS profile calls production.
pg="$stub_dir/prodglobs"; rm -rf "$pg"; mkdir -p "$pg/bin" "$pg/docs"
git -C "$pg" init -q -b main; git -C "$pg" config user.email t@t; git -C "$pg" config user.name t
echo "code" > "$pg/bin/tool"; echo "words" > "$pg/docs/x.md"
git -C "$pg" add -A; git -C "$pg" commit -qm base
pg_verified="$(git -C "$pg" rev-parse HEAD)"
echo "changed" > "$pg/docs/x.md"; git -C "$pg" add -A; git -C "$pg" commit -qm docs
pg_docs="$(git -C "$pg" rev-parse HEAD)"
echo "rewritten" > "$pg/bin/tool"; git -C "$pg" add -A; git -C "$pg" commit -qm prod
pg_prod="$(git -C "$pg" rev-parse HEAD)"
git -C "$pg" remote add origin "$pg"
git -C "$pg" update-ref refs/pull/21/head "$pg_docs"
git -C "$pg" update-ref refs/pull/22/head "$pg_prod"
pg_case() {  # $1 pr, $2 head, $3 expected substring, $4 label
    PATH="$stub_dir:$PATH" MAINTAINER_STATE="$stub_dir/pgstate" MAINTAINER_ACCOUNT=testuser \
        MAINTAINER_SLUG=o/r MAINTAINER_REPO="$pg" PROD_GLOBS="bin/*" \
        bash "$mg" receipt "$1" "$pg_verified" "proved" >/dev/null 2>&1
    make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0;;
      *'api user'*) echo testuser;;
      *reviewDecision*) echo APPROVED;;
      *headRefOid*) echo $2;;
      *mergeStateStatus*) echo CLEAN;;
      *'pr checks'*) echo '[{\"name\":\"x\",\"bucket\":\"pass\"}]';;
      *'pr merge'*) echo MERGED_STUB;;
    esac"
    out=$(PATH="$stub_dir:$PATH" MAINTAINER_STATE="$stub_dir/pgstate" MAINTAINER_ACCOUNT=testuser \
          MAINTAINER_SLUG=o/r MAINTAINER_REPO="$pg" PROD_GLOBS="bin/*" MAINTAINER_POST=off \
          bash "$mg" merge "$1" 2>&1)
    grep -q "$3" <<<"$out" && ok "$4" \
        || bad "$4 (got: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-90))"
}
pg_case 21 "$pg_docs" "receipt still applies" "a docs-only move keeps the receipt"
pg_case 22 "$pg_prod" "production code changed" "a rewrite of bin/ kills the receipt"

