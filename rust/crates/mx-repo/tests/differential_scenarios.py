"""The scenarios of the maintainer-repo differential (see differential.py).

Each builder takes the fixture and a seeded random generator, builds the case
directory, and returns (args, extra_env). Scenarios are generated from the
seed; the names say what they exercise, so a failure can be repeated with
DIFF_ONLY=<name>.
"""

import random
from pathlib import Path

# Branch names a maintainer might meet. All are valid git ref names. The pipe
# and the quoting characters are here because the bash joins and quotes names.
PRUNE_NAMES = ["feat-a", "feat-b", "fix/x", "deep/er/name", "dep|x", "x$y", "back`tick", "ünïcode", "UPPER",
               "dot.name", "semi;colon", "amp&er", "q'uote", 'dq"uote', "hash#1", "at@x", "pct%s", "tilde-ok"]
PRUNE_MODES = ["both-merged", "both-merged", "remote-merged", "local-merged", "unmerged-both", "unmerged-remote",
               "local-unmerged"]

CL_HEAD = "# Changelog\n\n"
CL_OLD = "## [1.0.0] - 2026-01-01\n\n- first\n\n## [0.9.0] - 2025-12-01\n\n- older\n"
UNREL_LINES = ["### Added", "### Security", "### Changed", "### Fixed", "- a thing", "- a fix", "- closed a hole",
               "- Node 18 is no longer supported", "- a call that used to succeed now fails",
               "- a call that used to work is gone", "- the gate refuses stale input", "- the gate REFUSES odd input",
               "- one line says no longer and no longer", "", "   ", "\t", "## [Unreleased]", "### Security notes",
               "### Addedum", "- No Longer spelled oddly", "- used to  work (two spaces)"]
PROD_GLOB_CHOICES = [None, "crates/*/src/*", "crates/*", "docs/* crates/*", "", "   ", "  crates/*  ", "*", "crates/*/src/*\nzzz/*",
                     "apps/*", ":(exclude)docs/* crates/*", "crates/x/src/lib.rs"]
SHIPPED_CHOICES = [None, "crates/*", "crates/*/src/*", "apps/*", "", "docs/*", "crates/x/src/lib.rs docs/*"]


# ---------------------------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------------------------

def deployed_profile(fx, rng, extra=""):
    """Run from a deployed profile.env instead of MAINTAINER_* names."""
    pdir = fx.dir / "home" / ".local" / "share" / "maintainer" / "profiles" / "magent"
    pdir.mkdir(parents=True, exist_ok=True)
    post = fx.env.pop("MAINTAINER_POST")
    text = "REPO_SLUG=o/r\nREPO_PATH=%s\nSTATE_DIR=%s\nGH_ACCOUNT=testuser\nPOST=%s\n%s" % (
        fx.repo, fx.state, post, extra)
    (pdir / "profile.env").write_text(text)
    for name in ("MAINTAINER_SLUG", "MAINTAINER_REPO", "MAINTAINER_ACCOUNT", "MAINTAINER_STATE"):
        fx.env.pop(name, None)


def remote_repo(fx):
    origin = fx.bare(fx.dir / "origin.git")
    fx.init_repo(origin)
    fx.write("README.md", "hello")
    fx.commit("base")
    fx.git("push", "-q", "origin", "main")
    return origin


def add_branch(fx, name, mode):
    if mode == "local-at-main":
        fx.git("branch", name)
        return
    remote = mode in ("both-merged", "remote-merged", "unmerged-both", "unmerged-remote")
    local = mode in ("both-merged", "local-merged", "unmerged-both", "local-unmerged")
    merged = mode in ("both-merged", "remote-merged", "local-merged")
    fx.git("checkout", "-q", "-b", name)
    fx.write("b-%s.txt" % name.replace("/", "-"), name)
    fx.commit("work on " + name)
    if remote:
        fx.git("push", "-q", "origin", name)
    fx.git("checkout", "-q", "main")
    if merged:
        fx.git("merge", "-q", "--ff-only", name)
        fx.git("push", "-q", "origin", "main")
    if not local:
        fx.git("branch", "-q", "-D", name)


def reject_deletes(fx, patterns):
    hook = fx.dir / "origin.git" / "hooks" / "pre-receive"
    body = "#!/bin/sh\nwhile read old new ref; do\n  case \"$new\" in 0000000000000000000000000000000000000000)\n    case \"$ref\" in\n"
    for pattern in patterns:
        body += "      refs/heads/%s) exit 1 ;;\n" % pattern
    body += "    esac ;;\n  esac\ndone\nexit 0\n"
    hook.write_text(body)
    hook.chmod(0o755)


# ---------------------------------------------------------------------------------------------
# prune
# ---------------------------------------------------------------------------------------------

def prune_scenario(fx, rng):
    remote_repo(fx)
    pool = rng.sample(PRUNE_NAMES, rng.randint(0, 5))
    names = []
    for name in pool:
        add_branch(fx, name, rng.choice(PRUNE_MODES))
        names.append(name)
    if rng.random() < 0.25:
        add_branch(fx, "master", "both-merged")
        fx.git("remote", "set-head", "origin", "main")
        names.append("master")
    if rng.random() < 0.15:
        fx.git("branch", "at-main")
        names.append("at-main")
    # open pull requests
    heads = [n for n in names if rng.random() < 0.4]
    if len(heads) >= 2 and rng.random() < 0.3:
        joined = "|".join(heads) + "|"
        if joined not in names:
            add_branch(fx, joined, "both-merged")  # F5: a branch named like the joined heads
    kind = rng.choice(["plain", "plain", "plain", "fails", "partial", "empty"])
    out = "\n".join(heads) + ("\n" if heads else "")
    if kind == "fails":
        fx.gh("pr-list", "", 1, "gh: not authenticated\n")
    elif kind == "partial":
        fx.gh("pr-list", out, 1)
    elif kind == "empty":
        fx.gh("pr-list", "", 0)
    else:
        fx.gh("pr-list", out, 0)
    # trouble at the remote or in the checkout
    trouble = rng.choice([None, None, "protected", "protected", "fetch", "norepo", "worktree", "behind", "checkedout"])
    if trouble == "protected" and names:
        reject_deletes(fx, [n for n in names if rng.random() < 0.6])
    elif trouble == "fetch":
        fx.git("remote", "set-url", "origin", str(fx.dir / "not-a-repo"))
    elif trouble == "worktree":
        fx.git("worktree", "add", "-q", "--detach", str(fx.dir / "wt"))
        import shutil
        shutil.rmtree(fx.dir / "wt")
    elif trouble == "behind":
        fx.git("reset", "-q", "--hard", "HEAD~1", check=False)
    elif trouble == "checkedout" and names:
        fx.git("checkout", "-q", names[0], check=False)
    elif trouble == "norepo":
        fx.env["MAINTAINER_REPO"] = str(fx.dir / "nowhere")
    flag = rng.choice([[], [], ["--dry-run"], ["--dry-run"], ["--dryrun"], ["--verbose", "--dry-run"], ["--dry-run", "x"],
                       ["-n"], [""]])
    post = rng.choice(["on", "on", "off", "OFF", "", None])
    if post is None:
        fx.env.pop("MAINTAINER_POST")
    else:
        fx.env["MAINTAINER_POST"] = post
    if rng.random() < 0.2:
        if "MAINTAINER_POST" not in fx.env:
            fx.env["MAINTAINER_POST"] = "on"
        deployed_profile(fx, rng)
    return ["prune"] + flag, {}


# ---------------------------------------------------------------------------------------------
# release-check
# ---------------------------------------------------------------------------------------------

def changelog_text(rng, mode):
    unrel = [rng.choice(UNREL_LINES) for _ in range(rng.randint(0, 6))]
    if mode == "empty":
        unrel = []
    body = "\n".join(unrel)
    text = CL_HEAD + "## [Unreleased]\n\n" + body + "\n\n" + CL_OLD
    if mode == "crlf":
        text = text.replace("\n", "\r\n")
    elif mode == "no-trailing-newline":
        text = text.rstrip("\n")
    elif mode == "twice":
        text = text + "\n## [Unreleased]\n\n### Security\n\n- again\n"
    elif mode == "no-heading":
        text = CL_HEAD + "### Security\n\n- loose\n\n" + CL_OLD
    elif mode == "no-versions":
        text = CL_HEAD + "## [Unreleased]\n\n" + body + "\n"
    elif mode == "old-security":
        text = text + "\n## [0.8.0] - 2025-01-01\n\n### Security\n\n- ancient fix that no longer applies\n"
    return text


def release_scenario(fx, rng):
    remote_repo(fx)
    fx.write("crates/x/src/lib.rs", "fn a() {}")
    fx.write("CHANGELOG.md", CL_HEAD + "## [Unreleased]\n\n" + CL_OLD)
    fx.commit("release 1.0.0")
    fx.git("tag", "v1.0.0")
    fx.git("push", "-q", "origin", "main", "--tags")
    n = 0
    for _ in range(rng.randint(0, 4)):
        n += 1
        kind = rng.choice(["prod", "doc", "deep", "tests", "other", "apps"])
        if kind == "prod":
            fx.write("crates/x/src/lib.rs", "fn a() { %d }" % n)
        elif kind == "doc":
            fx.write("docs/a.md", "doc %d" % n)
        elif kind == "deep":
            fx.write("crates/y/src/deep/m%d.rs" % n, "fn m() {}")
        elif kind == "tests":
            fx.write("crates/y/tests/t%d.rs" % n, "fn t() {}")
        elif kind == "apps":
            fx.write("apps/z/src/main.rs", "fn z() { %d }" % n)
        else:
            fx.write("notes%d.txt" % n, "x")
        fx.commit("commit %d (%s)" % (n, kind))
        if rng.random() < 0.15:
            fx.git("tag", "v1.%d.0" % n)
            n += 0
    fx.git("push", "-q", "origin", "main", "--tags")
    cl = rng.choice(["empty", "plain", "plain", "plain", "crlf", "no-trailing-newline", "twice", "no-heading", "no-versions",
                     "old-security", "none"])
    if cl != "none":
        fx.write("CHANGELOG.md", changelog_text(rng, cl))
        if rng.random() < 0.8:
            fx.commit("changelog")
            fx.git("push", "-q", "origin", "main")
    elif (fx.repo / "CHANGELOG.md").exists():
        (fx.repo / "CHANGELOG.md").unlink()
        fx.commit("drop changelog")
        fx.git("push", "-q", "origin", "main")
    tree = rng.choice(["clean", "clean", "dirty", "untracked", "ahead", "behind", "detached", "branch", "other-dirty"])
    if tree == "dirty":
        fx.write("CHANGELOG.md", changelog_text(rng, "plain") + "\n- local edit\n")
    elif tree == "untracked":
        fx.write("CHANGELOG.md", changelog_text(rng, "plain"))
    elif tree == "ahead":
        fx.write("local.txt", "local")
        fx.commit("local only")
    elif tree == "behind":
        fx.git("reset", "-q", "--hard", rng.choice(["v1.0.0", "HEAD~1"]), check=False)
    elif tree == "detached":
        fx.git("checkout", "-q", "--detach")
    elif tree == "branch":
        fx.git("checkout", "-q", "-b", rng.choice(["feat/x", "topic"]))
    elif tree == "other-dirty":
        fx.write("scratch.txt", "scratch")
    if rng.random() < 0.08:
        fx.git("remote", "set-url", "origin", str(fx.dir / "not-a-repo"))
    if rng.random() < 0.06:
        fx.env["MAINTAINER_REPO"] = str(fx.dir / "nowhere")
    env = {}
    prod = rng.choice(PROD_GLOB_CHOICES)
    ship = rng.choice(SHIPPED_CHOICES)
    if rng.random() < 0.2:
        # from the deployed profile instead of the environment
        extra = ""
        if prod is not None:
            extra += 'PROD_GLOBS="%s"\n' % prod.replace("\n", " ")
        if ship is not None:
            extra += 'SHIPPED_GLOBS="%s"\n' % ship
        deployed_profile(fx, rng, extra)
    else:
        if prod is not None:
            env["PROD_GLOBS"] = prod
        if ship is not None:
            env["SHIPPED_GLOBS"] = ship
    return rng.choice([["release-check"], ["release-check"], ["release-check", "extra"]]), env


# ---------------------------------------------------------------------------------------------
# fixup
# ---------------------------------------------------------------------------------------------

MARKER = "<!-- maintainer-fixup-notice -->"
PATCHES = {
    "typo": "diff --git a/docs.md b/docs.md\n--- a/docs.md\n+++ b/docs.md\n@@ -1 +1,2 @@\n one\n+two\n",
    "count3": "diff --git a/docs.md b/docs.md\n--- a/docs.md\n+++ b/docs.md\n@@ -1 +1,2 @@\n one\n+there are 3 tests\n",
    "count13": "diff --git a/docs.md b/docs.md\n--- a/docs.md\n+++ b/docs.md\n@@ -1 +1,2 @@\n one\n+there are 13 tests\n",
    "count30": "diff --git a/docs.md b/docs.md\n--- a/docs.md\n+++ b/docs.md\n@@ -1 +1,2 @@\n one\n+there are 30 tests\n",
    "plusplus": "diff --git a/docs.md b/docs.md\n--- a/docs.md\n+++ b/docs.md\n@@ -1 +1,2 @@\n one\n++3 tests\n",
    "bad": "diff --git a/docs.md b/docs.md\n--- a/docs.md\n+++ b/docs.md\n@@ -1 +1,2 @@\n nothing\n+two\n",
}


def fixup_fixture(fx, signed):
    remotes = fx.dir / "remotes"
    for rel in ("o/r.git", "alice/r.git"):
        fx.bare(remotes / rel)
    fx.init_repo()
    fx.write("README.md", "base")
    b0 = fx.commit("base")
    fx.git("push", "-q", str(remotes / "o" / "r.git"), "main")
    fx.git("push", "-q", str(remotes / "alice" / "r.git"), "main")
    fx.git("checkout", "-q", "-b", "fix-1")
    fx.write("docs.md", "one")
    message = {"SIGNED": "docs: add one\n\nSigned-off-by: Bob <bob@example.invalid>",
               "MENTION": "docs: add one\n\nreminder: Signed-off-by: is still to come"}.get(signed, "docs: add one")
    f1 = fx.commit(message)
    fx.git("push", "-q", str(remotes / "alice" / "r.git"), "fix-1")
    fx.git("checkout", "-q", "main")
    fx.env["GIT_CONFIG_COUNT"] = "1"
    fx.env["GIT_CONFIG_KEY_0"] = "url.%s/.insteadOf" % remotes
    fx.env["GIT_CONFIG_VALUE_0"] = "https://github.com/"
    fx.env["GIT_AUTHOR_DATE"] = "1767225700 +0000"
    fx.env["GIT_COMMITTER_DATE"] = "1767225700 +0000"
    fx.gh("auth-switch")
    fx.gh("api-user", "42\ttestuser")
    fx.gh("api-author", "7\tbob")
    fx.gh("pr-comment")
    fork = remotes / "alice" / "r.git"
    fx.ans_file("gh.pr-view", ".fromgit", "%s refs/heads/fix-1\n" % fork)
    return b0, f1, remotes


def fixup_answer(fx, state, items, f1, patch_path, overrides):
    fields = {"state": state, "head": f1, "base": "main", "branch": "fix-1", "fork": "alice/r", "author": "bob",
              "items": items, "patch": patch_path, "notice_marker": MARKER, "notice_due": "2026-11-20",
              "notice_days": "5", "review_age": "12"}
    fields.update(overrides)
    order = ["state", "head", "base", "branch", "fork", "author", "items", "patch", "notice_marker", "notice_due",
             "notice_days", "review_age"]
    text = "".join("%s=%s\n" % (k, fields[k]) for k in order if fields[k] != "")
    fx.ans_file("fixup-check", ".out", text)
    fx.ans_file("fixup-check", ".rc", "0")


def fixup_scenario(fx, rng):
    happy = rng.random() < 0.55  # a run with few faults, so the push and read-back paths are reached
    signed = rng.choice([None, None, "SIGNED", "MENTION"])
    b0, f1, remotes = fixup_fixture(fx, signed)
    state = ("ready" if rng.random() < 0.8 else "notice") if happy else rng.choice(["ready"] * 8 + ["notice"] * 4 + ["skip"])
    count_cmd = None
    if happy:
        items, patch_kind = rng.choice([
            ("typo:docs.md", "typo"), ("typo:docs.md", "typo"), ("docs:docs.md typo:other", "typo"),
            ("rebase", "unset"), ("rebase typo:docs.md", "typo"), ("rebase rebase typo:docs.md", "typo"),
            ("test-count:docs.md", "count3"), ("typo:docs.md test-count:docs.md", "count3"),
            ("test-count:docs.md", "count13"), ("docs:a docs:b typo:c", "typo"), ("typo:docs.md  docs:x", "typo")])
        if "test-count" in items:
            count_cmd = rng.choice(["echo 3", "printf ' 3 \\n'", '[ -z "${GH_TOKEN-}" ] && echo 3', "echo many"])
    else:
        items = rng.choice(["typo:docs.md", "docs:docs.md typo:other", "rebase", "rebase typo:docs.md",
                            "test-count:docs.md", "typo:docs.md test-count:docs.md", "typo:docs.md  docs:x",
                            "typo:docs.md\tdocs:x", "rebase rebase typo:docs.md", "docs:a docs:b typo:c", "Typo:docs.md",
                            "a:b:c typo:docs.md"])
        patch_kind = rng.choice(["typo", "typo", "count3", "count13", "count30", "plusplus", "bad", "missing", "unset"])
    patch_path = str(fx.dir / "fix.patch")
    if patch_kind in PATCHES:
        Path(patch_path).write_text(PATCHES[patch_kind])
    elif patch_kind == "unset":
        patch_path = ""
    overrides = {}
    if happy and rng.random() < 0.1:
        overrides["head"] = "0123456789012345678901234567890123456789"
    if not happy:
        if rng.random() < 0.08:
            overrides[rng.choice(["state", "head", "branch", "fork", "items"])] = ""
        if rng.random() < 0.08:
            overrides["head"] = "0123456789012345678901234567890123456789"
        if rng.random() < 0.05:
            overrides["fork"] = "nobody/none"
    fixup_answer(fx, state, items, f1, patch_path, overrides)
    if not happy and rng.random() < 0.07:
        fx.ans_file("fixup-check", ".rc", "3")
        fx.ans_file("fixup-check", ".err", "refused: not ripe\n")
    if not happy and rng.random() < 0.1:
        fx.gh("pr-comment", "", 1, "boom\n")
    if state == "notice" or rng.random() < 0.2:
        fx.ans_file("fixup-check", ".again.out", rng.choice(["state=ready\n", "state=skip\nreason=heads-up is not ripe yet\n",
                                                             "state=notice\n", ""]))
    moves = rng.choice([None, "clean", "conflict"] if happy else [None, None, "clean", "conflict"])
    if moves:
        fx.git("checkout", "-q", "main")
        fx.write("docs.md" if moves == "conflict" else "README.md", "zero" if moves == "conflict" else "base moved")
        fx.commit("base moves")
        fx.git("push", "-q", str(remotes / "o" / "r.git"), "main")
    account = None if happy else rng.choice([None, None, None, "non-numeric", "empty-id", "wrong", "case", "fails", "switch-fails", "no-tab"])
    if account == "non-numeric":
        fx.gh("api-user", "abc\ttestuser")
    elif account == "empty-id":
        fx.gh("api-user", "\ttestuser")
    elif account == "wrong":
        fx.gh("api-user", "42\tsomeoneelse")
    elif account == "case":
        fx.gh("api-user", "42\tTestUser")
    elif account == "fails":
        fx.gh("api-user", "", 1, "gh: boom\n")
    elif account == "switch-fails":
        fx.gh("auth-switch", "", 1, "no such account\n")
    elif account == "no-tab":
        fx.gh("api-user", "42")
    author = rng.choice([None, "case"]) if happy else rng.choice([None, None, None, "non-numeric", "other", "fails", "case"])
    if author == "non-numeric":
        fx.gh("api-author", "xx\tbob")
    elif author == "other":
        fx.gh("api-author", "7\trobert")
    elif author == "fails":
        fx.gh("api-author", "", 1)
    elif author == "case":
        fx.gh("api-author", "7\tBob")
    push = rng.choice([None, None, None, None, "refused", "second-writer"])
    if push == "refused":
        hook = remotes / "alice" / "r.git" / "hooks" / "pre-receive"
        hook.write_text("#!/bin/sh\nexit 1\n")
        hook.chmod(0o755)
    elif push == "second-writer":
        fx.git("checkout", "-q", "-b", "other", f1)
        fx.write("other.txt", "somebody else")
        f2 = fx.commit("second writer")
        fx.git("push", "-q", str(remotes / "alice" / "r.git"), "other")
        fx.git("checkout", "-q", "main")
        run = fx.dir / "ans" / "gh.api-author.run"
        run.write_text("#!/bin/bash\n%s --git-dir=%s update-ref refs/heads/fix-1 %s\n"
                       % (fx.realgit, remotes / "alice" / "r.git", f2))
    readback = rng.choice([None, None, None, None, "mismatch", "empty"])
    if readback == "mismatch":
        (fx.dir / "ans" / "gh.pr-view.fromgit").unlink()
        fx.gh("pr-view", f1)
    elif readback == "empty":
        (fx.dir / "ans" / "gh.pr-view.fromgit").unlink()
        fx.gh("pr-view", "", 0)
    fx.env["MAINTAINER_GH_BACKOFF"] = rng.choice(["0", "0", "0", "0.01", "abc"])
    tries = rng.choice([None, None, "1", "2", "5", "0", "010", "-1"])
    if tries is not None:
        fx.env["MAINTAINER_GH_TRIES"] = tries
    if count_cmd is None and not happy:
        count_cmd = rng.choice([None, None, "echo 3", "echo many", "printf ' 3 \\n'", '[ -z "${GH_TOKEN-}" ] && echo 3',
                                "exit 5", "echo 3; echo 4", "echo 13", "true"])
    env = {}
    if count_cmd is not None:
        fx.env["FIXUP_COUNT_CMD"] = count_cmd
        env["FIXUP_COUNT_CMD"] = count_cmd
    if rng.random() < 0.3:
        fx.env["GH_TOKEN"] = "ghp_fixturefixturefixture0000"
    if not happy and rng.random() < 0.1:
        fx.env["GIT_CONFIG_COUNT"] = "2"
        fx.env["GIT_CONFIG_KEY_1"] = "commit.cleanup"
        fx.env["GIT_CONFIG_VALUE_1"] = "bogus"
    if not happy and rng.random() < 0.05:
        fx.env["MAINTAINER_STATE"] = str(fx.dir / "no-such-state-dir")
    if rng.random() < 0.12:
        fx.env["MAINTAINER_POST"] = "off"
    elif not happy and rng.random() < 0.05:
        fx.env.pop("MAINTAINER_POST")
    if happy:
        args = rng.choice([["7"], ["7", "--dry-run"], ["7"], ["--dry-run", "7"]])
    else:
        args = rng.choice([["7"]] * 8 + [["7", "--dry-run"]] * 8 + [["--dry-run", "7"]] * 2 + [["8", "7"], ["007"], ["7", "abc"],
                           ["abc", "7"], [], ["--dry-run"], ["-7"], ["1.5"], [""]])
    return ["fixup"] + args, env


# ---------------------------------------------------------------------------------------------
# the command line and the profile
# ---------------------------------------------------------------------------------------------

def usage_scenario(fx, rng):
    remote_repo(fx)
    fx.gh("pr-list", "")
    word = rng.choice(["", "frobnicate", "--help", "Prune", "prune ", "-h", "release", "fixup-check", "pru"])
    args = [word] if word != "" or rng.random() < 0.5 else []
    if rng.random() < 0.3:
        args += ["--dry-run"]
    return args, {}


def profile_scenario(fx, rng):
    remote_repo(fx)
    fx.gh("pr-list", "")
    kind = rng.choice(["none", "two", "missing", "named", "one", "one-post-off", "bad-defaults", "nolib", "nolib-home"])
    if kind.startswith("nolib"):
        # no lib/profile.sh beside the program: the bash falls back to the installed copy, which is absent
        fx.variant = "nolib"
        if kind == "nolib-home":
            fx.env.pop("HOME")
    share = fx.dir / "home" / ".local" / "share" / "maintainer" / "profiles"
    for name in ("MAINTAINER_SLUG", "MAINTAINER_REPO", "MAINTAINER_ACCOUNT", "MAINTAINER_STATE"):
        fx.env.pop(name, None)
    text = "REPO_SLUG=o/r\nREPO_PATH=%s\nSTATE_DIR=%s\nGH_ACCOUNT=testuser\nPOST=%s\n" % (
        fx.repo, fx.state, "off" if kind == "one-post-off" else "on")
    if kind == "two":
        for n in ("a", "b"):
            (share / n).mkdir(parents=True)
            (share / n / "profile.env").write_text(text)
    elif kind == "missing":
        (share / "magent").mkdir(parents=True)
        (share / "magent" / "profile.env").write_text(text)
        fx.env["MAINTAINER_PROFILE"] = "ghost"
    elif kind == "named":
        for n in ("a", "b"):
            (share / n).mkdir(parents=True)
            (share / n / "profile.env").write_text(text)
        fx.env["MAINTAINER_PROFILE"] = "b"
    elif kind in ("one", "one-post-off"):
        (share / "magent").mkdir(parents=True)
        (share / "magent" / "profile.env").write_text(text)
    elif kind == "bad-defaults":
        (share / "magent").mkdir(parents=True)
        (share / "magent" / "profile.env").write_text(text + "exit 7\n")
    return rng.choice([["prune", "--dry-run"], ["release-check"], ["frobnicate"], []]), {}


def release_notag_scenario(fx, rng):
    """A repository whose history has no tag: release-check refuses."""
    remote_repo(fx)
    fx.write("crates/x/src/lib.rs", "fn a() {}")
    fx.commit("first")
    fx.git("push", "-q", "origin", "main")
    if rng.random() < 0.5:
        fx.env["PROD_GLOBS"] = "crates/*"
    return ["release-check"], {}


def fixup_notice_scenario(fx, rng):
    """A heads-up, posted or rehearsed, with each read-back the tool can give."""
    b0, f1, remotes = fixup_fixture(fx, None)
    items = rng.choice(["typo:docs.md", "typo:docs.md test-count:docs.md", "docs:a  docs:b"])
    fixup_answer(fx, "notice", items, f1, str(fx.dir / "fix.patch"), {})
    fx.ans_file("fixup-check", ".again.out", rng.choice(["state=ready\n", "state=skip\nreason=heads-up is not ripe yet\n",
                                                         "state=notice\n", ""]))
    if rng.random() < 0.2:
        fx.gh("pr-comment", "", 1, "boom\n")
    if rng.random() < 0.08:
        fx.env["MAINTAINER_POST"] = "off"
    return ["fixup", "7"] + (["--dry-run"] if rng.random() < 0.2 else []), {}


def all_scenarios(counts):
    """[(name, builder)] with `counts` of each family."""
    out = []
    for family, builder in (("prune", prune_scenario), ("release", release_scenario), ("fixup", fixup_scenario),
                            ("usage", usage_scenario), ("profile", profile_scenario),
                            ("notag", release_notag_scenario), ("notice", fixup_notice_scenario)):
        for i in range(counts[family]):
            out.append(("%s-%03d" % (family, i), builder))
    return out
