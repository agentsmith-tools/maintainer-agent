#!/usr/bin/env python3
"""Seeded differential: `maintainer-merge verify-deps` (bash) against `mx-verify-deps` (Rust).

    deps_differential.py --repo <checkout> --rust-bin <mx-verify-deps> --work <dir> [--seed N] [--cases N]

Every case builds one scenario (a diff, the answers of a stubbed `gh`, a stubbed registry, a
stubbed `maintainer` lease helper), runs the bash gate and the Rust program on it in turn in the
same directory, and compares exit code, standard output, standard error, the argument vector of
every call either program made to a stub, and the receipt (without the timestamp, the user and
the signature, which the script verifies itself). The classifier and the whole-file pip check are
the real helper scripts; no stub reaches the network.

A case is `same` when everything is equal. A case tagged with a decision id (`P3C-n`) must differ
in exactly the way that decision says, and that is checked as strictly as equality is. Exit 0
when no case is unexplained, 1 otherwise.
"""

import argparse
import difflib
import hashlib
import hmac
import json
import os
import random
import re
import shutil
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

SEP = "\x1f"
HEAD = "0123456789abcdef0123456789abcdef01234567"
MERGE_BASE = "c" * 40
PIN_OLD = "a" * 40
PIN_NEW = "b" * 40
UPSTREAM_OTHER = "e" * 40
DIGEST_OLD = "sha256:" + "1" * 64
DIGEST_NEW = "sha256:" + "2" * 64
NOT_FOUND = "gh: Not Found (HTTP 404)"
KEY = "differential receipt key\n"


def esc(text):
    """Text for `printf '%b'`: every byte that is not plain ASCII becomes an octal escape."""
    out = []
    for byte in text.encode("utf-8", "surrogateescape"):
        ch = chr(byte)
        if ch == "\\":
            out.append("\\\\")
        elif ch == "\n":
            out.append("\\n")
        elif byte < 0x20 or byte >= 0x7F:
            out.append("\\0%03o" % byte)
        else:
            out.append(ch)
    return "".join(out)


class Gh:
    """The table of a `gh` stub: exact argument line -> exit code, standard output or error."""

    def __init__(self):
        self.rows = []

    def ok(self, key, out=""):
        self.rows.append((key, 0, out, ""))

    def fail(self, key, err=NOT_FOUND, rc=1):
        self.rows.append((key, rc, "", err))

    def table(self):
        return "".join(f"{k}{SEP}{rc}{SEP}{esc(o)}{SEP}{esc(e)}\n" for k, rc, o, e in self.rows)


def api(path, jq):
    return f"api {path} --jq {jq}"


TAGS = "[.ref, .object.type, .object.sha] | @tsv"
TAGOBJ = "[.object.type, .object.sha] | @tsv"
RAW = "api -H Accept: application/vnd.github.raw"


def view(field, jq):
    return f"pr view 7 --repo o/r --json {field} --jq {jq}"


# ----- diffs the real classifier reads -----

def cargo_diff(bad=False):
    new = '+source = "git+https://evil.example/x"' if bad else \
        '+source = "registry+https://github.com/rust-lang/crates.io-index"'
    return (
        "diff --git a/Cargo.lock b/Cargo.lock\n--- a/Cargo.lock\n+++ b/Cargo.lock\n"
        '@@ -126,9 +126,9 @@ dependencies = [\n name = "async-openai"\n-version = "0.41.3"\n'
        '+version = "0.42.0"\n-source = "registry+https://github.com/rust-lang/crates.io-index"\n'
        f"{new}\n"
    )


def workflow_diff(pins):
    lines = []
    for action, tag, branch in pins:
        mark = f"# {tag} (branch)" if branch else f"# {tag}"
        lines.append(f"-        uses: {action}@{PIN_OLD} {mark}\n")
        lines.append(f"+        uses: {action}@{PIN_NEW} {mark}\n")
    n = len(pins)
    return (
        "diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml\n"
        "--- a/.github/workflows/ci.yml\n+++ b/.github/workflows/ci.yml\n"
        f"@@ -1,{n} +1,{n} @@\n" + "".join(lines)
    )


def docker_diff(images):
    body = ""
    for image, tag in images:
        body += f"-FROM {image}:{tag}@{DIGEST_OLD} AS builder\n+FROM {image}:{tag}@{DIGEST_NEW} AS builder\n"
    n = len(images)
    return f"diff --git a/Dockerfile b/Dockerfile\n--- a/Dockerfile\n+++ b/Dockerfile\n@@ -1,{n} +1,{n} @@\n{body}"


def req_file(version, hashes, extra=True):
    text = "pathspec==1.1.1 \\\n    --hash=sha256:" + "7" * 64 + "\n"
    text += f"PyYAML=={version} \\\n" + " \\\n".join(f"    --hash=sha256:{h}" for h in hashes) + "\n"
    if extra:
        text += "yamllint==1.38.0 \\\n    --hash=sha256:" + "8" * 64 + "\n"
    return text


def req_diff(path, old, new):
    body = list(difflib.unified_diff(
        old.splitlines(True), new.splitlines(True), f"a/{path}", f"b/{path}", n=3))
    return f"diff --git a/{path} b/{path}\n" + "".join(body)


class Case:
    def __init__(self, name):
        self.name = name
        self.diff = cargo_diff()
        self.gh = Gh()
        self.head = HEAD
        self.arg = HEAD[:7]
        self.lease = (0, "")
        self.classify_timeout = "60"
        self.classifier = "real"          # real | missing | sleep | json
        self.classifier_json = None       # what a stub classifier prints, for classifier == "json"
        self.receipts_blocked = False     # a file where the receipts directory should be
        self.resolver = "present"         # present | missing
        self.verifier = "present"
        self.wholefile = "present"
        self.registry = {}                # "image tag" -> (rc, out); "name version hashes" -> (rc, out)
        self.env_drop = []
        self.key = "present"
        self.decision = None              # P3C-n: the case is expected to differ
        self.expect_diff = None           # substring the Rust side must have and bash must not (or reverse)

    def base_gh(self):
        self.gh.ok(view("headRefOid", ".headRefOid"), self.head + "\n")
        self.gh.ok("pr diff 7 --repo o/r", self.diff)


# ----- scenario generators -----

def tag_pin(case, rnd, repo, tag, mode):
    key = api(f"repos/{repo}/git/ref/tags/{tag}", TAGS)
    good = f"refs/tags/{tag}\tcommit\t{PIN_NEW}\n"
    if mode == "ok":
        case.gh.ok(key, good)
    elif mode == "wrong_sha":
        case.gh.ok(key, f"refs/tags/{tag}\tcommit\t{UPSTREAM_OTHER}\n")
    elif mode == "404":
        case.gh.fail(key)
    elif mode == "wrong_ref":
        case.gh.ok(key, f"refs/pull/1/head\tcommit\t{PIN_NEW}\n")
    elif mode == "empty":
        case.gh.ok(key, "\n")
    elif mode == "blob":
        case.gh.ok(key, f"refs/tags/{tag}\tblob\t{PIN_NEW}\n")
    elif mode == "short":
        case.gh.ok(key, f"refs/tags/{tag}\n")
    elif mode == "tabs":
        case.gh.ok(key, f"refs/tags/{tag}\t\tcommit\t\t{PIN_NEW}\n")
    elif mode == "extra":
        case.gh.ok(key, f"refs/tags/{tag}\tcommit\t{PIN_NEW}\textra\n")
    elif mode.startswith("annotated") and mode[len("annotated"):].isdigit():
        depth = int(mode[len("annotated"):])
        objs = [("%040x" % (0x1000 + i)) for i in range(depth + 1)]
        case.gh.ok(key, f"refs/tags/{tag}\ttag\t{objs[0]}\n")
        for i in range(min(depth, 5)):
            last = i == depth - 1
            answer = f"commit\t{PIN_NEW}\n" if last else f"tag\t{objs[i + 1]}\n"
            case.gh.ok(api(f"repos/{repo}/git/tags/{objs[i]}", TAGOBJ), answer)
        if depth > 5:
            pass
    elif mode == "annotated_404":
        case.gh.ok(key, f"refs/tags/{tag}\ttag\t{'1' * 40}\n")
        case.gh.fail(api(f"repos/{repo}/git/tags/{'1' * 40}", TAGOBJ))
    elif mode == "annotated_wrong":
        case.gh.ok(key, f"refs/tags/{tag}\ttag\t{'1' * 40}\n")
        case.gh.ok(api(f"repos/{repo}/git/tags/{'1' * 40}", TAGOBJ), f"commit\t{UPSTREAM_OTHER}\n")
    elif mode == "annotated_tree":
        case.gh.ok(key, f"refs/tags/{tag}\ttag\t{'1' * 40}\n")
        case.gh.ok(api(f"repos/{repo}/git/tags/{'1' * 40}", TAGOBJ), f"tree\t{PIN_NEW}\n")
    else:
        raise ValueError(mode)


def branch_pin(case, repo, tag, ref_mode, cmp_mode):
    rkey = api(f"repos/{repo}/git/ref/heads/{tag}", ".ref")
    ckey = api(f"repos/{repo}/compare/{tag}...{PIN_NEW}", ".status")
    if ref_mode == "ok":
        case.gh.ok(rkey, f"refs/heads/{tag}\n")
    elif ref_mode == "404":
        case.gh.fail(rkey)
    elif ref_mode == "pull":
        case.gh.ok(rkey, "refs/pull/1/head\n")
    elif ref_mode == "empty":
        case.gh.ok(rkey, "\n")
    if cmp_mode == "404":
        case.gh.fail(ckey)
    else:
        case.gh.ok(ckey, cmp_mode + "\n")


def image_answer(case, image, tag, mode):
    k = f"{image} {tag}"
    if mode == "ok":
        case.registry[k] = (0, DIGEST_NEW + "\n")
    elif mode == "mismatch":
        case.registry[k] = (0, "sha256:" + "9" * 64 + "\n")
    elif mode == "fail":
        case.registry[k] = (1, "resolve_image_digest: could not reach registry\n")
    elif mode == "empty":
        case.registry[k] = (0, "\n")


def pip_registry(case, name, version, hashes, mode):
    k = f"{name} {version} {','.join(hashes)}"
    if mode == "ok":
        case.registry[k] = (0, f"{len(hashes)} of 5 files\n")
    else:
        case.registry[k] = (1, f"verify_pypi_release: sha256:{hashes[0][:12]} is not a file PyPI publishes\n")


def pip_case(case, rnd, mode):
    hashes_old = ["a" * 64, "b" * 64]
    hashes_new = ["c" * 64, "d" * 64] if rnd.random() < 0.5 else ["d" * 64, "c" * 64]
    old, new = req_file("6.0.2", hashes_old), req_file("6.0.3", hashes_new)
    path = "requirements.txt"
    case.diff = req_diff(path, old, new)
    case.base_head()
    pip_registry(case, "PyYAML", "6.0.3", hashes_new, "ok" if mode != "pypi_bad" else "bad")
    case.gh.ok(view("baseRefName", ".baseRefName"), "main\n")
    mb = api(f"repos/o/r/compare/main...{case.head}", ".merge_base_commit.sha")
    base_key = f"{RAW} repos/o/r/contents/{path}?ref={MERGE_BASE}"
    head_key = f"{RAW} repos/o/r/contents/{path}?ref={case.head}"
    served_new = new
    if mode == "ok":
        case.gh.ok(mb, MERGE_BASE + "\n")
        case.gh.ok(base_key, old)
        case.gh.ok(head_key, new)
    elif mode == "pypi_bad":
        case.gh.ok(mb, MERGE_BASE + "\n")
    elif mode == "disagree_version":
        case.gh.ok(mb, MERGE_BASE + "\n")
        case.gh.ok(base_key, old)
        case.gh.ok(head_key, req_file("6.0.4", hashes_new))
    elif mode == "disagree_hash":
        case.gh.ok(mb, MERGE_BASE + "\n")
        case.gh.ok(base_key, old)
        case.gh.ok(head_key, req_file("6.0.3", ["c" * 64, "e" * 64]))
    elif mode == "not_a_bump":
        case.gh.ok(mb, MERGE_BASE + "\n")
        case.gh.ok(base_key, old)
        case.gh.ok(head_key, new + "evilpkg==1.0 \\\n    --hash=sha256:" + "0" * 64 + "\n")
    elif mode == "mb_fail":
        case.gh.fail(mb)
    elif mode == "mb_garbage":
        case.gh.ok(mb, "not-a-sha\n")
    elif mode == "mb_short":
        case.gh.ok(mb, "c" * 39 + "\n")
    elif mode == "mb_upper":
        case.gh.ok(mb, "C" * 40 + "\n")
    elif mode == "base_fail":
        case.gh.ok(mb, MERGE_BASE + "\n")
        case.gh.fail(base_key)
    elif mode == "head_fail":
        case.gh.ok(mb, MERGE_BASE + "\n")
        case.gh.ok(base_key, old)
        case.gh.fail(head_key)
    elif mode == "baseref_fail":
        case.gh.rows = [r for r in case.gh.rows if r[0] != view("baseRefName", ".baseRefName")]
        case.gh.fail(view("baseRefName", ".baseRefName"), "gh: boom")
    elif mode == "baseref_empty":
        case.gh.rows = [r for r in case.gh.rows if r[0] != view("baseRefName", ".baseRefName")]
        case.gh.ok(view("baseRefName", ".baseRefName"), "\n")
    elif mode == "wholefile_missing":
        case.wholefile = "missing"
    else:
        raise ValueError(mode)
    del served_new


def _base_head(self):
    self.gh.ok(view("headRefOid", ".headRefOid"), self.head + "\n")
    self.gh.ok("pr diff 7 --repo o/r", self.diff)


Case.base_head = _base_head


def verdict_json(files, pins=(), images=(), pip=(), lines=2, **more):
    body = {"verdict": "dependency-bump", "files": files, "changed_lines": lines,
            "pins": list(pins), "images": list(images), "pip": list(pip)}
    body.update(more)
    return json.dumps(body)


STUB_VARIANTS = ["plain_path", "bad_path", "pins_missing_key", "images_missing_key", "pip_missing_key",
                 "files_not_object", "refused_exit_zero", "garbage", "list_top", "no_pins_key",
                 "extra_key", "bad_action", "hostile_tag", "hostile_branch", "hostile_base_ref",
                 "receipts_blocked"]


def stubbed(case, rnd, variant):
    """A stub classifier prints the verdict, so shapes the real one never emits reach the gate."""
    case.classifier = "json"
    case.base_head()
    pin = {"action": "actions/checkout", "sha": PIN_NEW, "tag": "v4", "branch": False}
    if variant == "plain_path":
        case.classifier_json = verdict_json({"Cargo.lock": "cargo-lock"})
    elif variant == "bad_path":
        case.classifier_json = verdict_json({"../x/requirements.txt": "pip-requirements"})
        case.gh.ok(view("baseRefName", ".baseRefName"), "main\n")
        case.gh.ok(api(f"repos/o/r/compare/main...{HEAD}", ".merge_base_commit.sha"), MERGE_BASE + "\n")
    elif variant == "pins_missing_key":
        case.classifier_json = verdict_json({}, pins=[{"action": "actions/checkout"}])
    elif variant == "images_missing_key":
        case.classifier_json = verdict_json({}, pins=[], images=[{"image": "i"}])
    elif variant == "pip_missing_key":
        case.classifier_json = verdict_json({}, pip=[{"name": "n"}])
    elif variant == "files_not_object":
        case.classifier_json = json.dumps({"verdict": "dependency-bump", "files": [], "changed_lines": 1,
                                           "pins": [], "images": [], "pip": []})
    elif variant == "refused_exit_zero":
        case.classifier_json = json.dumps({"verdict": "refused", "reason": "x"})
        case.decision = "P3C-3"
    elif variant == "garbage":
        case.classifier_json = rnd.choice(["not json", "", "[1, 2]", "{"])
    elif variant == "list_top":
        case.classifier_json = "[]"
    elif variant == "no_pins_key":
        case.classifier_json = json.dumps({"verdict": "dependency-bump", "files": {"Cargo.lock": "cargo-lock"},
                                           "changed_lines": 3, "images": [], "pip": []})
        case.decision = "P3C-11"
    elif variant == "extra_key":
        case.classifier_json = verdict_json({"Cargo.lock": "cargo-lock"}, note="x")
        case.decision = "P3C-11"
    elif variant == "bad_action":
        case.classifier_json = verdict_json({"w": "workflow"}, pins=[dict(pin, action="checkout")])
        case.gh.ok(api("repos/checkout/checkout/git/ref/tags/v4", TAGS), f"refs/tags/v4\tcommit\t{PIN_NEW}\n")
        case.decision = "P3C-9"
    elif variant == "hostile_tag":
        case.classifier_json = verdict_json({"w": "workflow"}, pins=[dict(pin, tag="v4/../pull/1/head")])
        case.gh.ok(api("repos/actions/checkout/git/ref/tags/v4/../pull/1/head", TAGS),
                   f"refs/pull/1/head\tcommit\t{PIN_NEW}\n")
        case.decision = "P3C-10"
    elif variant == "hostile_branch":
        case.classifier_json = verdict_json({"w": "workflow"}, pins=[dict(pin, tag="a?b", branch=True)])
        case.gh.ok(api("repos/actions/checkout/git/ref/heads/a?b", ".ref"), "refs/heads/a?b\n")
        case.gh.ok(api(f"repos/actions/checkout/compare/a?b...{PIN_NEW}", ".status"), "identical\n")
        case.decision = "P3C-10"
    elif variant == "hostile_base_ref":
        case.classifier_json = verdict_json({"requirements.txt": "pip-requirements"})
        case.gh.ok(view("baseRefName", ".baseRefName"), "main?x=1\n")
        case.gh.ok(api(f"repos/o/r/compare/main?x=1...{HEAD}", ".merge_base_commit.sha"), "z\n")
        case.decision = "P3C-10"
    elif variant == "receipts_blocked":
        case.classifier_json = verdict_json({"Cargo.lock": "cargo-lock"})
        case.receipts_blocked = True


def generate(seed, count):
    rnd = random.Random(seed)
    cases = []
    tag_modes = ["ok"] * 4 + ["wrong_sha", "404", "wrong_ref", "empty", "blob", "short", "tabs", "extra",
                              "annotated1", "annotated2", "annotated4", "annotated5", "annotated6",
                              "annotated_404", "annotated_wrong", "annotated_tree"]
    repos = ["actions/checkout", "actions/cache", "github/codeql-action/analyze", "dtolnay/rust-toolchain"]
    tags = ["v4.2.2", "v3", "release/1.x", "stable", "v1+build", "v10.0.0-rc.1"]
    pip_modes = ["ok"] * 3 + ["pypi_bad", "disagree_version", "disagree_hash", "not_a_bump", "mb_fail",
                              "mb_garbage", "mb_short", "mb_upper", "base_fail", "head_fail",
                              "baseref_fail", "baseref_empty", "wholefile_missing"]
    turn = {"tag": 0, "pip": 0, "stub": 0, "branch": 0}

    def next_of(name, modes):
        turn[name] += 1
        return modes[(turn[name] - 1) % len(modes)]

    while len(cases) < count:
        i = len(cases)
        kind = rnd.choice(["cargo", "workflow", "workflow", "workflow", "branch", "docker", "pip", "pip",
                           "pip", "mixed", "head", "diffs", "classifier", "lease", "script", "stub", "stub"])
        case = Case(f"{i:03d}-{kind}")
        if kind == "cargo":
            case.diff = cargo_diff(bad=rnd.random() < 0.3)
            case.base_head()
        elif kind == "workflow":
            n = rnd.choice([1, 1, 2, 3])
            pins, modes = [], []
            for k in range(n):
                repo, tag = rnd.choice(repos), rnd.choice(tags)
                pins.append((repo, tag, False))
                modes.append(next_of("tag", tag_modes) if k == 0 else rnd.choice(tag_modes))
            case.diff = workflow_diff(pins)
            case.base_head()
            for (repo, tag, _), mode in zip(pins, modes):
                owner_repo = "/".join(repo.split("/")[:2])
                tag_pin(case, rnd, owner_repo, tag, mode)
        elif kind == "branch":
            repo, tag = rnd.choice(repos), rnd.choice(["stable", "main", "release/1.x"])
            case.diff = workflow_diff([(repo, tag, True)])
            case.base_head()
            ref_mode, cmp_mode = next_of("branch", [
                ("ok", "identical"), ("ok", "behind"), ("ok", "ahead"), ("ok", "diverged"), ("ok", ""),
                ("ok", "404"), ("404", "identical"), ("pull", "identical"), ("empty", "identical")])
            branch_pin(case, "/".join(repo.split("/")[:2]), tag, ref_mode, cmp_mode)
        elif kind == "docker":
            n = rnd.choice([1, 1, 2])
            images = [("docker.io/library/rust", "1-bookworm"), ("docker.io/library/debian", "12")][:n]
            case.diff = docker_diff(images)
            case.base_head()
            for image, tag in images:
                image_answer(case, image, tag, rnd.choice(["ok", "ok", "mismatch", "fail", "empty"]))
            if rnd.random() < 0.15:
                case.resolver = "missing"
        elif kind == "pip":
            pip_case(case, rnd, next_of("pip", pip_modes))
        elif kind == "stub":
            stubbed(case, rnd, next_of("stub", STUB_VARIANTS))
        elif kind == "mixed":
            case.diff = workflow_diff([("actions/checkout", "v4", False)]) \
                + docker_diff([("docker.io/library/rust", "1")])
            case.base_head()
            tag_pin(case, rnd, "actions/checkout", "v4", rnd.choice(["ok", "ok", "wrong_sha"]))
            image_answer(case, "docker.io/library/rust", "1", rnd.choice(["ok", "mismatch"]))
        elif kind == "head":
            case.diff = cargo_diff()
            mode = rnd.choice(["moved", "fail", "empty", "longer_arg", "full_arg", "newline"])
            if mode == "moved":
                case.base_head()
                case.arg = "fedcba98765"
            elif mode == "fail":
                case.gh.fail(view("headRefOid", ".headRefOid"), "gh: HTTP 502")
                case.gh.ok("pr diff 7 --repo o/r", case.diff)
            elif mode == "empty":
                case.gh.ok(view("headRefOid", ".headRefOid"), "\n")
                case.gh.ok("pr diff 7 --repo o/r", case.diff)
            elif mode == "longer_arg":
                case.base_head()
                case.arg = HEAD[:rnd.randint(7, 40)]
            elif mode == "full_arg":
                case.base_head()
                case.arg = HEAD
            else:
                case.gh.ok(view("headRefOid", ".headRefOid"), HEAD + "\n\n")
                case.gh.ok("pr diff 7 --repo o/r", case.diff)
        elif kind == "diffs":
            mode = rnd.choice(["fail", "empty", "newlines", "multicommit", "no_trailing", "crlf", "bom"])
            case.gh.ok(view("headRefOid", ".headRefOid"), HEAD + "\n")
            if mode == "fail":
                case.gh.fail("pr diff 7 --repo o/r", "HTTP 502: Bad Gateway\nsecond line")
            elif mode == "empty":
                case.gh.ok("pr diff 7 --repo o/r", "")
            elif mode == "newlines":
                case.gh.ok("pr diff 7 --repo o/r", "\n\n\n")
            elif mode == "multicommit":
                case.gh.ok("pr diff 7 --repo o/r",
                           "From " + "9" * 40 + " Mon Sep 17 00:00:00 2001\nSubject: x\n---\n" + cargo_diff())
            elif mode == "no_trailing":
                case.gh.ok("pr diff 7 --repo o/r", cargo_diff().rstrip("\n"))
            elif mode == "crlf":
                case.gh.ok("pr diff 7 --repo o/r", cargo_diff().replace("\n", "\r\n"))
            else:
                case.gh.ok("pr diff 7 --repo o/r", "﻿" + cargo_diff())
        elif kind == "classifier":
            case.diff = cargo_diff()
            case.base_head()
            mode = rnd.choice(["missing", "timeout", "tiny_ok", "bad_limit"])
            if mode == "missing":
                case.classifier = "missing"
            elif mode == "timeout":
                case.classifier, case.classify_timeout = "sleep", "1"
            elif mode == "tiny_ok":
                case.classify_timeout = rnd.choice(["30", "30s", "1m", "0.5m", "1e2", "+30", ".5e2"])
            else:
                case.classify_timeout = rnd.choice(["abc", "5x"])
                case.decision = "P3C-4b"
        elif kind == "lease":
            case.diff = cargo_diff()
            case.base_head()
            case.lease = rnd.choice([(1, "held by another actor (amy) until 12:00"), (2, "state locked"),
                                     (3, ""), (1, "")])
        elif kind == "script":
            case.diff = workflow_diff([("actions/checkout", "v4", False)])
            case.base_head()
            tag_pin(case, rnd, "actions/checkout", "v4", "ok")
            pick = rnd.choice(["verifier", "resolver", "wholefile", "key"])
            if pick == "verifier":
                case.verifier = "missing"
            elif pick == "resolver":
                case.resolver = "missing"
            elif pick == "wholefile":
                case.wholefile = "missing"
            else:
                case.key = "missing"
                case.decision = "P3C-7"
        cases.append(case)
    # The decision cases: each differs from the bash on purpose, in the way its decision says.
    cases += decision_cases(len(cases))
    return cases


def decision_cases(start):
    out = []

    def add(name, decision, build):
        case = Case(f"{start + len(out):03d}-{name}")
        case.decision = decision
        build(case)
        out.append(case)

    def head_not_hex(case):
        case.head = "0123456/../pull/1/head"
        case.arg = "0123456"
        case.base_head()
    add("head-not-hex", "P3C-2", head_not_hex)

    def zero_limit(case):
        case.base_head()
        case.classify_timeout = "0"
    add("limit-zero", "P3C-4", zero_limit)

    def non_utf8(case):
        case.gh.ok(view("headRefOid", ".headRefOid"), HEAD + "\n")
        case.gh.rows.append(("pr diff 7 --repo o/r", 0, "diff --git a/x b/x\n\xff\xfe\n", ""))
        case.diff = None
    add("diff-not-utf8", "P3C-6", non_utf8)

    def no_key(case):
        case.base_head()
        case.key = "missing"
    add("no-receipt-key", "P3C-7", no_key)

    def hostile_tag(case):
        case.diff = workflow_diff([("actions/checkout", "v4", False)])
        case.base_head()
        case.gh.ok(api("repos/actions/checkout/git/ref/tags/v4", TAGS), f"refs/tags/v4\tcommit\t{PIN_NEW}\n")
    add("control-characters-in-an-answer", "P3C-8", lambda c: (hostile_tag(c), c.gh.rows.pop(),
        c.gh.ok(api("repos/actions/checkout/git/ref/tags/v4", TAGS), "\x1b[31mrefs/tags/v4\x1b[0m\tcommit\tx\n")))
    return out


# ----- running one case -----

STUB_GH = r'''#!/usr/bin/env bash
d="$(cd "$(dirname "$0")" && pwd)"
key="$*"
printf 'gh %s\n' "$key" >> "$d/calls.log"
while IFS=$'\x1f' read -r k rc out err; do
    if [ "$k" = "$key" ]; then
        printf '%b' "$err" >&2
        printf '%b' "$out"
        exit "$rc"
    fi
done < "$d/table"
printf 'gh: no answer for: %s\n' "$key" >&2
exit 99
'''

STUB_MAINTAINER = r'''#!/usr/bin/env bash
d="$(cd "$(dirname "$0")/../stub" && pwd)"
printf 'maintainer %s\n' "$*" >> "$d/calls.log"
[ -f "$d/lease" ] || exit 0
IFS=$'\x1f' read -r rc msg < "$d/lease"
[ -z "$msg" ] || printf '%b\n' "$msg"
exit "$rc"
'''

STUB_REGISTRY = r'''#!/usr/bin/env python3
import json, os, sys
here = os.path.dirname(os.path.abspath(__file__))
name = os.path.basename(__file__)
args = " ".join(sys.argv[1:])
with open(os.path.join(here, "..", "stub", "calls.log"), "a") as log:
    log.write(f"{name} {args}\n")
table = json.load(open(os.path.join(here, "..", "stub", "registry.json")))
rc, out = table.get(args, [1, f"{name}: no answer for {args}\n"])
(sys.stdout if rc == 0 else sys.stderr).write(out)
raise SystemExit(rc)
'''

SLOW_CLASSIFIER = "import time\ntime.sleep(30)\n"


def write(path, text, mode=None):
    with open(path, "w", encoding="utf-8", errors="surrogateescape", newline="") as handle:
        handle.write(text)
    if mode:
        os.chmod(path, mode)


def prepare(case, root, tree, repo):
    d = os.path.join(root, case.name)
    if os.path.exists(d):
        shutil.rmtree(d)
    stub, scripts = os.path.join(d, "stub"), os.path.join(d, "scripts")
    os.makedirs(stub)
    os.makedirs(scripts)
    write(os.path.join(stub, "gh"), STUB_GH, 0o755)
    table = case.gh.table()
    if case.diff is None:   # a diff that is not UTF-8: the table's own bytes
        table = table.replace(esc("diff --git a/x b/x\n\xff\xfe\n"), "diff --git a/x b/x\\n\\0377\\0376\\n")
    write(os.path.join(stub, "table"), table)
    write(os.path.join(stub, "registry.json"), json.dumps({k: list(v) for k, v in case.registry.items()}))
    if case.lease[0]:
        write(os.path.join(stub, "lease"), f"{case.lease[0]}{SEP}{esc(case.lease[1])}\n")
    for name in ("resolve_image_digest.py", "verify_pypi_release.py"):
        write(os.path.join(scripts, name), STUB_REGISTRY, 0o755)
    shutil.copy(os.path.join(repo, "scripts", "verify_pip_files.py"), scripts)
    if case.classifier == "real":
        shutil.copy(os.path.join(repo, "scripts", "classify_dependency_bump.py"), scripts)
    elif case.classifier == "json":
        write(os.path.join(scripts, "classify_dependency_bump.py"),
              f"import sys\nsys.stdin.read()\nsys.stdout.write({case.classifier_json!r})\n")
    elif case.classifier == "sleep":
        write(os.path.join(scripts, "classify_dependency_bump.py"), SLOW_CLASSIFIER)
    if case.resolver == "missing":
        os.remove(os.path.join(scripts, "resolve_image_digest.py"))
    if case.verifier == "missing":
        os.remove(os.path.join(scripts, "verify_pypi_release.py"))
    if case.wholefile == "missing":
        os.remove(os.path.join(scripts, "verify_pip_files.py"))
    if case.key == "present":
        write(os.path.join(d, "receipt.key"), KEY, 0o600)
    return d


def run_program(case, d, tree, which, rust_bin):
    state = os.path.join(d, "state")
    for leftover in (state, os.path.join(d, "stub", "calls.log")):
        if os.path.isdir(leftover):
            shutil.rmtree(leftover)
        elif os.path.exists(leftover):
            os.remove(leftover)
    os.makedirs(state)
    if case.receipts_blocked:
        open(os.path.join(state, "receipts"), "w").close()
    env = {
        "PATH": f"{os.path.join(d, 'stub')}:/usr/local/bin:/usr/bin:/bin",
        "HOME": d, "LC_ALL": "C", "TZ": "UTC", "LOGNAME": "tester",
        "MAINTAINER_SLUG": "o/r", "MAINTAINER_REPO": os.path.join(d, "repo"), "MAINTAINER_STATE": state,
        "MAINTAINER_ACCOUNT": "tester", "MAINTAINER_SCRIPTS": os.path.join(d, "scripts"),
        "MAINTAINER_RECEIPT_KEY": os.path.join(d, "receipt.key"),
        "MAINTAINER_CLASSIFY_TIMEOUT": case.classify_timeout, "MAINTAINER_NOTIFY": "off",
        "CLASSIFY_TIMEOUT": case.classify_timeout,
    }
    for name in case.env_drop:
        env.pop(name, None)
    os.makedirs(env["MAINTAINER_REPO"], exist_ok=True)
    if which == "bash":
        argv = ["bash", os.path.join(tree, "bin", "maintainer-merge"), "verify-deps", "7", case.arg]
    else:
        argv = [os.path.join(tree, "bin", "mx-verify-deps"), "verify-deps", "7", case.arg]
    done = subprocess.run(argv, env=env, capture_output=True, timeout=120, cwd=d)
    calls = ""
    log = os.path.join(d, "stub", "calls.log")
    if os.path.exists(log):
        calls = open(log, encoding="utf-8", errors="replace").read()
    receipt, mode = None, None
    path = os.path.join(state, "receipts", "7.json")
    if os.path.exists(path):
        raw = open(path, "rb").read()
        try:
            receipt = json.loads(raw)
        except ValueError:
            receipt = {"unparsable": raw.decode("utf-8", "replace")}
    return {
        "rc": done.returncode,
        "out": done.stdout.decode("utf-8", "replace"),
        "err": done.stderr.decode("utf-8", "replace"),
        "calls": calls,
        "receipt": receipt,
    }


def receipt_ok(receipt):
    """The signature the gate would recompute over the receipt as written."""
    if not receipt or "signature" not in receipt:
        return False
    body = {k: v for k, v in receipt.items() if k != "signature"}
    expect = hmac.new(KEY.strip().encode(), json.dumps(body, indent=2).encode(), hashlib.sha256).hexdigest()
    return hmac.compare_digest(expect, receipt["signature"])


def drop_tool_noise(text):
    """What the bash's helper tools print on their own: a Python traceback from a `python3 -c`
    reader that died (the gate's own `die` line follows it) and `mkdir -p`'s complaint."""
    marker = "Traceback (most recent call last):"
    while marker in text:
        head, _, rest = text.partition(marker)
        _, found, tail = rest.partition("\nmaintainer-merge:")
        text = head + (("maintainer-merge:" + tail) if found else "")
    return "".join(line for line in text.splitlines(True) if not line.startswith("mkdir: "))


def drop_head_rereads(calls):
    """P3C-12: the program reads the head again after each stage, the bash reads it once. Every
    read but the first is dropped from the program's call log, and nothing else is."""
    seen, kept = False, []
    for line in calls.splitlines(True):
        if line == "gh pr view 7 --repo o/r --json headRefOid --jq .headRefOid\n":
            if seen:
                continue
            seen = True
        kept.append(line)
    return "".join(kept)


def normalise(result):
    result = dict(result)
    r = result["receipt"]
    if r is not None:
        result["signed"] = receipt_ok(r)
        r = {k: v for k, v in r.items() if k not in ("recorded_at", "recorded_by", "signature")}
        result["receipt"] = r
    result["err"] = drop_tool_noise(result["err"])
    return result


def run_case(case, root, tree, repo, rust_bin):
    d = prepare(case, root, tree, repo)
    bash = normalise(run_program(case, d, tree, "bash", rust_bin))
    rust = normalise(run_program(case, d, tree, "rust", rust_bin))
    rust["calls"] = drop_head_rereads(rust["calls"])
    return case, bash, rust, d


FIELDS = ("rc", "out", "err", "calls", "receipt", "signed")


def differences(bash, rust):
    return [f for f in FIELDS if bash.get(f) != rust.get(f)]


def judge(case, bash, rust):
    """Returns (verdict, detail): same | explained | UNEXPLAINED."""
    diff = differences(bash, rust)
    if case.decision is None:
        if not diff:
            return "same", ""
        return "UNEXPLAINED", ", ".join(diff)
    # Decision cases: the divergence must be exactly the decided one.
    d = case.decision
    if d == "P3C-2":
        ok = rust["rc"] == 1 and "could not read the head of #7" in rust["err"] and rust["receipt"] is None \
            and not any("compare" in c or "contents" in c for c in rust["calls"].splitlines())
    elif d == "P3C-4":
        ok = rust["rc"] == 1 and "is not a pure dependency bump" in rust["err"] and rust["receipt"] is None \
            and "classify" not in rust["calls"] and bash["rc"] == 0 and bash["receipt"] is not None
    elif d == "P3C-3":
        ok = bash["rc"] == rust["rc"] == 1 and rust["receipt"] is None \
            and "could not read the pip files out of the classifier verdict" in bash["err"] \
            and "is not a pure dependency bump" in rust["err"]
    elif d == "P3C-9":
        ok = rust["rc"] == 1 and "is not owner/repo[/dir]" in rust["err"] and rust["receipt"] is None \
            and "git/ref/tags" not in rust["calls"] and "git/ref/tags/v4" in bash["calls"]
    elif d == "P3C-10":
        ok = rust["rc"] == 1 and rust["receipt"] is None \
            and not any(("?" in c or ".." in c) for c in rust["calls"].splitlines()) \
            and any(("?" in c or ".." in c) for c in bash["calls"].splitlines())
    elif d == "P3C-11":
        ok = bash["rc"] == 0 and rust["rc"] == 1 and rust["receipt"] is None \
            and "could not read the action pins out of the classifier verdict" in rust["err"]
    elif d == "P3C-4b":
        # `timeout` complains about its own argument on standard error; the program has no `timeout`.
        said = bash["err"].split("\n", 2)
        ok = said[0].startswith("timeout: invalid time interval") and said[2] == rust["err"] \
            and bash["rc"] == rust["rc"] == 1 and bash["out"] == rust["out"]
    elif d == "P3C-6":
        ok = rust["rc"] == 1 and "the diff is not UTF-8" in rust["err"] and rust["receipt"] is None \
            and bash["rc"] == 1
    elif d == "P3C-7":
        ok = rust["rc"] == 1 and rust["receipt"] is None and bash["rc"] == 1 \
            and rust["err"] == bash["err"] and rust["out"] == bash["out"] \
            and bash["receipt"] is not None and "signature" not in bash["receipt"]
        if not ok and bash["rc"] != 1:
            ok = not diff
    elif d == "P3C-8":
        ok = rust["rc"] == 1 and "\x1b" not in rust["err"] and "\x1b" in bash["err"]
    else:
        ok = False
    return ("explained", d) if ok else ("UNEXPLAINED", f"{d}: {', '.join(diff)}")


def refusal_patterns(repo):
    """The text of every refusal, from the table the Rust crate tests against, as one regex each."""
    source = open(os.path.join(repo, "rust/crates/mx-gate/src/deps/refusal.rs"), encoding="utf-8").read()
    table = source.split("pub(crate) fn samples()", 1)[1].split("#[cfg(test)]\nmod tests", 1)[0]
    patterns = {}
    for row in table.splitlines():
        found = re.match(r'\s*\(DepsRefusal::(\w+)(?: \{.*?\})?, "(.*)"\.to_string\(\)\),?$', row)
        if not found:
            continue
        text = json.loads('"' + found.group(2) + '"')
        parts = re.split("\u27e6[^\u27e7]*\u27e7", text)
        patterns[found.group(1)] = re.compile(".*?".join(re.escape(p) for p in parts), re.S)
    return patterns


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True)
    ap.add_argument("--rust-bin", required=True)
    ap.add_argument("--work", required=True)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--cases", type=int, default=330)
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--keep", action="store_true")
    args = ap.parse_args()
    repo = os.path.abspath(args.repo)
    work = os.path.abspath(args.work)
    shutil.rmtree(work, ignore_errors=True)
    tree = os.path.join(work, "tree")
    os.makedirs(os.path.join(tree, "bin"))
    for sub in ("lib", "scripts"):
        shutil.copytree(os.path.join(repo, sub), os.path.join(tree, sub), ignore=shutil.ignore_patterns("__pycache__"))
    shutil.copy(os.path.join(repo, "bin", "maintainer-merge"), os.path.join(tree, "bin"))
    write(os.path.join(tree, "bin", "maintainer"), STUB_MAINTAINER, 0o755)
    shutil.copy(args.rust_bin, os.path.join(tree, "bin", "mx-verify-deps"))
    cases = generate(args.seed, args.cases)
    results = []
    with ThreadPoolExecutor(args.jobs) as pool:
        futures = [pool.submit(run_case, c, os.path.join(work, "cases"), tree, repo, args.rust_bin) for c in cases]
        for future in futures:
            results.append(future.result())
    tally, bad = {}, 0
    outcomes = {}
    for case, bash, rust, d in results:
        verdict, detail = judge(case, bash, rust)
        tally[verdict] = tally.get(verdict, 0) + 1
        shape = (bash["rc"], rust["rc"])
        outcomes[shape] = outcomes.get(shape, 0) + 1
        if verdict == "UNEXPLAINED":
            bad += 1
            print(f"UNEXPLAINED {case.name}: {detail}")
            for f in differences(bash, rust):
                print(f"  {f}:\n    bash: {bash.get(f)!r}\n    rust: {rust.get(f)!r}")
    patterns = refusal_patterns(repo)
    reached = {name: 0 for name in patterns}
    for case, bash, rust, d in results:
        for name, pattern in patterns.items():
            if pattern.search(bash["err"]) and pattern.search(rust["err"]):
                reached[name] += 1
    unreached = sorted(name for name, n in reached.items() if n == 0)
    print(f"refusals reached by both programs: {sum(1 for n in reached.values() if n)} of {len(patterns)}")
    print("not reached:", ", ".join(unreached) or "none")
    refusals = sum(1 for _, b, r, _ in results if b["rc"] != 0)
    successes = sum(1 for _, b, r, _ in results if b["rc"] == 0)
    print(f"seed={args.seed} comparisons={len(results)} {tally} bash_exit0={successes} bash_refused={refusals}")
    print("exit-code pairs (bash, rust):", dict(sorted(outcomes.items())))
    if not args.keep:
        shutil.rmtree(work, ignore_errors=True)
    return 1 if bad or not results else 0


if __name__ == "__main__":
    sys.exit(main())
