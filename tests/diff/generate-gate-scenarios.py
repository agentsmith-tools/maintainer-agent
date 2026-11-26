#!/usr/bin/env python3
"""Seeded generator of differential scenarios for maintainer-merge (bash against Rust).

    python3 tests/diff/generate-gate-scenarios.py <out-dir> [seed [count [verify-count]]]

Writes `count` scenario files (default 330, seed 20261122) for tests/diff/record.sh: merge with
random receipts, heads, merge states, review decisions, check boards and identities; update with
random facts and reviews; receipt, show and usage. Then it writes `verify-count` more (default 300,
files v*.sh, a random stream of its own so the first `count` never move): verify, verify-part and
merge on a receipt proved in parts, against a verify.d the scenario writes and a podman stub that
logs its whole argv. Run record.sh on the bash tree and on the tree assembled with the Rust binary,
then compare.sh. The differences that remain are the decisions of the gate port: the flock stub
calls, the wrapper's MAINTAINER_CHILD_ENV in the gh environment log, and the P3W decisions of the
pull request that added the verify scenarios.
"""
import random, shlex, sys, os

seed = int(sys.argv[2]) if len(sys.argv) > 2 else 20261122
count = int(sys.argv[3]) if len(sys.argv) > 3 else 330
verify_count = int(sys.argv[4]) if len(sys.argv) > 4 else 300
out = sys.argv[1]
os.makedirs(out, exist_ok=True)
rnd = random.Random(seed)

STATES = ["CLEAN", "CLEAN", "CLEAN", "HAS_HOOKS", "BEHIND", "DIRTY", "BLOCKED", "UNSTABLE", "UNKNOWN", "DRAFT", ""]
REVIEWS = ["APPROVED"] * 6 + ["REVIEW_REQUIRED", "CHANGES_REQUESTED", "", "approved"]
BUCKETS = ["pass"] * 6 + ["skipping", "fail", "pending", "cancel", "weird"]
KINDS = ["observed"] * 5 + ["asserted", "dependency"]
LOGINS = ["testuser"] * 7 + ["TESTUSER", "mallory", ""]


def checks_json():
    n = rnd.choice([0, 1, 1, 2, 3, 4])
    rows = [{"name": "c%d" % i, "bucket": rnd.choice(BUCKETS)} for i in range(n)]
    import json
    return json.dumps(rows, separators=(",", ":")), any(r["bucket"] == "pending" for r in rows)


def gh_stub(pr, head, state, review, login, login_rc, checks, checks_rc, merge_rc):
    return f"""mx_stub gh <<'STUB'
case "$*" in
  *'auth switch'*) exit 0 ;;
  *'api user'*) {'exit ' + str(login_rc) if login_rc else "echo '" + login + "'"} ;;
  *headRefOid*) echo '{head}' ;;
  *mergeStateStatus*) echo '{state}' ;;
  *reviewDecision*) echo '{review}' ;;
  *closingIssuesReferences*) ;;
  *'pr checks'*) printf '%s\\n' '{checks}'; exit {checks_rc} ;;
  *'pr merge'*) echo 'stub gh: pr merge ran'; exit {merge_rc} ;;
esac
STUB
"""


def repo_block(pr, shape):
    """Builds the repository; sets C_RECEIPT (verified head) and C_HEAD (the PR head)."""
    lines = ["mx_git_repo", 'mx_commit "$MX_REPO" base bin/a v0 docs/d v0']
    if shape == "same":
        lines += ['C_HEAD="$(git -C "$MX_REPO" rev-parse HEAD)"', 'C_RECEIPT="$C_HEAD"', f"mx_pr_origin {pr}"]
    elif shape in ("moved-doc", "moved-prod"):
        f = ("docs/d", "doc") if shape == "moved-doc" else ("bin/a", "prod")
        lines += ['C_RECEIPT="$(git -C "$MX_REPO" rev-parse HEAD)"',
                  f'mx_commit "$MX_REPO" later {f[0]} {f[1]}',
                  'C_HEAD="$(git -C "$MX_REPO" rev-parse HEAD)"', f"mx_pr_origin {pr}"]
    elif shape in ("tree-doc", "tree-prod"):
        w = ("docs/w", "w") if shape == "tree-doc" else ("bin/w", "w")
        lines += ['git -C "$MX_REPO" checkout -q -b pr',
                  'mx_commit "$MX_REPO" verified bin/pr 1',
                  'C_RECEIPT="$(git -C "$MX_REPO" rev-parse HEAD)"',
                  f'mx_commit "$MX_REPO" work {w[0]} {w[1]}',
                  'C_HEAD="$(git -C "$MX_REPO" rev-parse HEAD)"',
                  'git -C "$MX_REPO" checkout -q main',
                  'mx_commit "$MX_REPO" mainmove bin/m 1',
                  'git init -q --bare -b main "$MX_WORK/origin.git"',
                  'git -C "$MX_REPO" remote add origin "$MX_WORK/origin.git"',
                  f'git -C "$MX_REPO" push -q origin "main" "pr:refs/pull/{pr}/head"']
    elif shape == "unknown-verified":
        lines += ['C_HEAD="$(git -C "$MX_REPO" rev-parse HEAD)"',
                  'C_RECEIPT=0000000000000000000000000000000000000000', f"mx_pr_origin {pr}"]
    return "\n".join(lines) + "\n"


SHAPES = ["same"] * 8 + ["moved-doc", "moved-prod", "tree-doc", "tree-prod"]


def merge_scenario(i):
    pr = rnd.randint(1, 99)
    shape = rnd.choice(SHAPES)
    state, review = rnd.choice(STATES), rnd.choice(REVIEWS)
    login = rnd.choice(LOGINS)
    login_rc = 4 if rnd.random() < 0.05 else 0
    checks, pending = checks_json()
    kind = rnd.choice(KINDS)
    tamper = rnd.random() < 0.08
    missing = rnd.random() < 0.05
    in_run = rnd.random() < 0.2
    post_off = rnd.random() < 0.15
    merge_rc = 1 if rnd.random() < 0.08 else 0
    short_head = rnd.random() < 0.08
    prod = rnd.choice(["bin/* lib/*", "bin/* lib/*", "bin/*", ""])
    s = ["#!/usr/bin/env bash", f"# generated merge scenario {i}", "mx_merge_env"]
    s.append(f"mx_env PROD_GLOBS '{prod}'" if prod else "mx_env PROD_GLOBS ''")
    if in_run:
        s.append("mx_env MAINTAINER_IN_RUN review")
    if post_off:
        s.append("mx_env MAINTAINER_POST " + rnd.choice(["off", "off", "OFF", "0"]))
    s.append(repo_block(pr, shape).rstrip())
    head_expr = '"$C_HEAD"'
    rec_head = '"$C_RECEIPT"' if not short_head else '"${C_RECEIPT:0:7}"'
    if not missing:
        s.append(f"mx_receipt {pr} {rec_head} {kind} 'guard mutated, went red'")
        if tamper:
            s.append(f"sed -i 's/went red/stayed green/' \"$MX_STATE/receipts/{pr}.json\"")
    stub = gh_stub(pr, "'\"$C_HEAD\"'", state, review, login, login_rc, checks, 8 if pending else 0, merge_rc)
    # the head is only known at run time, so the stub heredoc is unquoted for that one value
    stub = stub.replace("<<'STUB'", "<<STUB").replace("'\"$C_HEAD\"'", "$C_HEAD").replace('$*', '\\$*').replace('"$*"', '"\\$*"')
    stub = stub.replace("printf '%s\\n'", "printf '%s\\\\n'")
    s.append(stub.rstrip())
    s.append(f"mx_run bin/maintainer-merge merge {pr}")
    return "\n".join(s) + "\n"


def update_scenario(i):
    pr = rnd.randint(1, 99)
    state = rnd.choice(["BEHIND"] * 5 + ["CLEAN", "HAS_HOOKS", "DIRTY", "BLOCKED", "UNKNOWN"])
    decision = rnd.choice(["APPROVED"] * 5 + ["REVIEW_REQUIRED", "-"])
    author = rnd.choice(["alice"] * 4 + ["app/dependabot", "Dependabot[bot]", "dependabot"])
    rv = rnd.choice([
        "APPROVED testuser $MX_UPDATE_HEAD", "APPROVED testuser $MX_UPDATE_HEAD",
        "APPROVED TestUser $MX_UPDATE_HEAD", "CHANGES_REQUESTED testuser $MX_UPDATE_HEAD",
        "APPROVED other $MX_UPDATE_HEAD", "APPROVED testuser 2222222222222222222222222222222222222222", "- x x"])
    cross = rnd.choice(["false", "false", "true"])
    modify = rnd.choice(["false", "true"])
    post_off = rnd.random() < 0.3
    lines = ["#!/usr/bin/env bash", f"# generated update scenario {i}", "mx_merge_env", "mx_env PROD_GLOBS 'bin/* lib/*'"]
    if post_off:
        lines.append("mx_env MAINTAINER_POST off")
    lines += ["mx_git_repo", 'mx_commit "$MX_REPO" base bin/a v0', f"mx_pr_origin {pr}"]
    # the head the stub reports is the commit the pull request ref really holds
    lines.append('MX_UPDATE_HEAD="$(git -C "$MX_REPO" rev-parse HEAD)"')
    lines += [f'export MX_U_REVIEW="{rv}"', f"export MX_U_CROSS={cross} MX_U_MODIFY={modify}"]
    if rnd.random() < 0.85:
        lines.append(f'mx_receipt {pr} "$MX_UPDATE_HEAD" observed "guard mutated, went red"')
    lines.append(f'mx_update_gh {pr} {state} {decision} "{author}"')
    # a quarter of the scenarios answer `gh pr view` with a mistyped or missing fact, which the
    # python helper of the bash gate refuses and the Rust gate must refuse too
    if rnd.random() < 0.25:
        author_json, cross_json = rnd.choice([
            ('{"login":"alice"}', "true"), ('"dependabot"', "false"), ('{"login":5}', "false"),
            ('{"login":true}', "false"), ('{"login":[1]}', "false"), ('{"login":0}', "false"),
            ('{"login":{}}', "false"), ('null', "false"), ('{"login":"alice"}', '"false"'),
            ('{"login":"alice"}', "null"), ('{"login":"alice"}', "1"), ('{"login":"alice"}', None),
            ('[]', "false"), ('{"login":"app/dependabot"}', "false"),
        ])
        fields = [f'"headRefOid":"$MX_UPDATE_HEAD"', f'"mergeStateStatus":"{state}"',
                  '"reviewDecision":' + ('null' if decision == "-" else f'"{decision}"'),
                  f'"author":{author_json}']
        if cross_json is not None:
            fields.append(f'"isCrossRepository":{cross_json}')
        fields.append('"maintainerCanModify":false')
        lines.append("printf '%s\\n' \"{" + ",".join(f.replace('"', '\\"') for f in fields) + "}\" > \"$MX_WORK/gh-answers/pr-view.json\"")
    lines.append(f"mx_run bin/maintainer-merge update {pr}")
    return "\n".join(lines) + "\n"


def show_scenario(i):
    pr = rnd.randint(1, 99)
    lines = ["#!/usr/bin/env bash", f"# generated show scenario {i}", "mx_merge_env"]
    for k in range(rnd.randint(0, 3)):
        lines.append(f"mx_receipt {rnd.randint(1, 9)}{k} abcdef1234567 observed 'proof number {k} " + "x" * rnd.choice([0, 10, 70]) + "'")
    arg = rnd.choice(["", f" {pr}", " 10", " 20"])
    lines.append(f"mx_run bin/maintainer-merge show{arg}")
    return "\n".join(lines) + "\n"


def receipt_scenario(i):
    pr = rnd.randint(1, 99)
    sha = rnd.choice(["abcdef1", "abcdef1234567890", "abc"])
    in_run = rnd.random() < 0.25
    lines = ["#!/usr/bin/env bash", f"# generated receipt scenario {i}", "mx_merge_env"]
    if in_run:
        lines.append("mx_env MAINTAINER_IN_RUN review")
    lines.append(f"mx_run bin/maintainer-merge receipt {pr} {sha} 'a proof'")
    return "\n".join(lines) + "\n"


def badenv_scenario(i):
    """A gate variable whose value is not UTF-8: bash reads the bytes, the Rust gate refuses."""
    name, value, args = rnd.choice([
        ("MAINTAINER_IN_RUN", "'\\xff'", "receipt 7 abcdef1 'a proof'"),
        ("MAINTAINER_POST", "'o\\xff'", "show"),
        ("PROD_GLOBS", "'bin/*\\xff'", "show"),
        ("CLAIM_LABEL", "'\\xfe'", "show"),
    ])
    return ("#!/usr/bin/env bash\n# generated non-UTF-8 environment scenario %d\nmx_merge_env\nmx_env_bytes %s %s\n"
            "mx_run bin/maintainer-merge %s\n" % (i, name, value, args))


def usage_scenario(i):
    arg = rnd.choice(["", "frobnicate", "--help", "Merge 7", "merge", "update", "merge abc", "update 7x", "receipt 7", "verify 7 1234567"])
    return "#!/usr/bin/env bash\n# generated usage scenario %d\nmx_merge_env\nmx_run bin/maintainer-merge %s\n" % (i, arg)


makers = [(merge_scenario, "merge", 60), (update_scenario, "update", 20), (show_scenario, "show", 8),
          (receipt_scenario, "receipt", 6), (usage_scenario, "usage", 6), (badenv_scenario, "badenv", 4)]
weights = [w for _, _, w in makers]
for i in range(count):
    fn, name, _ = rnd.choices(makers, weights)[0]
    with open(os.path.join(out, f"g{i:03d}-{name}.sh"), "w") as f:
        f.write(fn(i))
# --------------------------------------------------------------------------------------------------
# verify, verify-part and merge on a partial receipt (lane p3w)
#
# These scenarios run the gate against a verify.d the scenario writes, a repository whose pull
# request ref holds the head, a `gh` stub that answers the changed-file list, and a `podman` stub that
# answers each call in order and logs its whole argv (the recorder's own stub log). The container
# never runs, so the comparison covers what the gate decides and what it asks the runtime to do:
# exit code, both streams, the container command line (flags, limits, mounts, order), the receipt it
# writes and the state it leaves. They use their own random stream, so the 330 scenarios above do
# not move.

FIXTURE = [
    "bin/a", "v0", "lib/b", "v0", "docs/d", "v0", "bin/x.sh", "v0", "docs/d.md", "v0", "tests/t", "v0",
]

PATHSETS = [
    ["bin/a"], ["bin/a"], ["bin/a"], ["bin/a", "lib/b"], ["bin/a", "lib/b"], ["lib/b"], ["lib/b"], ["docs/d"],
    ["bin/a", "docs/d"], ["docs/d", "docs/d.md"], ["bin/x.sh"], ["bin/a", "lib/b", "docs/d"], ["unknown/x"],
    ["tests/t"], ["bin/a", "unknown/x"],
]

SUITES = {
    "unit": {"covers": ["bin/a", "bin/x.sh", "lib/*"], "globs": ["a", "b"]},
    "lib": {"covers": ["lib/*"], "globs": ["b"]},
    "docs": {"covers": ["docs/*"], "globs": ["d"]},
    "wide": {"covers": ["bin/*", "lib/*", "docs/*", "tests/*"], "globs": ["a", "b", "d", "t"]},
    "named": {"covers": ["*"], "globs": ["a", "b", "d"], "named_only": True},
}


def suite_text(name, spec, rnd2, flavour):
    """The bash text of one verify.d file; `flavour` picks the optional declarations."""
    pats = "|".join(spec["covers"])
    out = [
        '# generated suite %s' % name,
        'suite_covers() { case "$1" in %s) return 0 ;; esac; return 1; }' % pats,
        "suite_needs() { printf 'bash git'; }",
        "suite_mutate_glob() { printf '%s\\n' " + " ".join("'%s'" % g for g in spec["globs"]) + "; }",
        r"""suite_ran() { local n; n="$(sed -n 's/^ *\([0-9]\+\) passed.*/\1/p' "$1" | tail -1)"; printf '%s' "${n:-0}"; }""",
    ]
    image = "docker.io/library/rust:1"
    if "image-env" in flavour:
        out.append("suite_image() { printf '%s' \"${MAINTAINER_SHELL_IMAGE:-" + image + "}\"; }")
    else:
        out.append("suite_image() { printf '%s' '" + image + "'; }")
    if "work" in flavour:
        out.append("suite_command() { printf 'run %s in %s dir %s\\n' \"$1\" \"${work:-unset}\" \"${CHARACTERIZATION_DIR:-none}\"; }")
    else:
        out.append("suite_command() { printf 'cargo test %s\\n' \"$1\"; }")
    if spec.get("named_only"):
        out.append("suite_named_only() { return 0; }")
    if "requires-never" in flavour:
        out.append("suite_requires() { [ \"$1\" = nonesuch ]; }")
        out.append("suite_requires_name() { printf 'a runtime that does not exist'; }")
    if "requires-podman" in flavour:
        out.append("suite_requires() { [ \"$1\" = podman ]; }")
    if "extra" in flavour:
        out.append("suite_podman_args() { printf '%s\\n' '-v' \"$HOME/.cargo:/cargo:O\" '-e' \"CARGO_BUILD_JOBS=${MAINTAINER_CARGO_JOBS:-4}\"; }")
    if "extra-rt" in flavour:
        out.append("suite_podman_args() { printf '%s\\n' '-e' \"RT=$1\" '-v' \"$2:/scratch\"; }")
    if "limits" in flavour:
        out.append("suite_memory() { printf '2g'; }")
        out.append("suite_cpus() { printf '3'; }")
    if "limits-all" in flavour:
        out.append("suite_pids() { printf '99'; }")
        out.append("suite_tmpfs() { printf '64m'; }")
    if "prefetch" in flavour:
        out.append("suite_prefetch_command() { printf 'cargo fetch --locked\\n'; }")
    if "guard-ok" in flavour:
        out.append("suite_prefetch_guard() { [ -e \"$1/lib/b\" ]; }")
    if "guard-refuses" in flavour:
        out.append("suite_prefetch_guard() { printf 'Cargo.lock names a git source\\n'; return 1; }")
    return "\n".join(out) + "\n"


CLEAN_OK = "running 3 tests\\n3 passed; 0 failed\\n"
MUT_FAIL = "thread x panicked at src/lib.rs:7\\nassertion failed: guard holds\\nFAILED\\n"
RUNTIME_WARN = 'time="2026-01-01T00:00:00Z" level=warning msg="cni config"'


def podman_calls(rnd2, prefetch):
    """The script of the podman stub: (rc, text) per call, and the name of the outcome."""
    kind = rnd2.choice([
        "bites", "bites", "bites", "bites", "bites", "bites", "bites", "bites", "bites", "bites", "no-bite", "clean-fails", "clean-killed",
        "clean-env", "clean-zero", "mut-killed", "mut-125", "mut-warning", "mut-long", "clean-143",
        "clean-124", "clean-segv", "mut-no-failure-word",
    ])
    clean = (0, CLEAN_OK)
    mut = (1, MUT_FAIL)
    if kind == "no-bite":
        mut = (0, "3 passed\\n")
    elif kind == "clean-fails":
        clean = (1, "assertion failed: guard\\n")
    elif kind == "clean-killed":
        clean = (137, "")
    elif kind == "clean-143":
        clean = (143, "Terminated\\n")
    elif kind == "clean-124":
        clean = (124, "")
    elif kind == "clean-segv":
        clean = (139, "Segmentation fault\\n")
    elif kind == "clean-env":
        clean = (127, "bash: python3: command not found\\n")
    elif kind == "clean-zero":
        clean = (0, "running 0 tests\\n")
    elif kind == "mut-killed":
        mut = (137, "")
    elif kind == "mut-125":
        mut = (125, "Error: crun: the container never started\\n")
    elif kind == "mut-warning":
        mut = (1, RUNTIME_WARN + "\\nthread x panicked at src/lib.rs:9\\n")
    elif kind == "mut-long":
        mut = (101, "FAILED " + "y" * 260 + "\\n")
    elif kind == "mut-no-failure-word":
        mut = (3, "claim mismatch: 4 != 5\\nnothing else\\n")
    calls = [clean, mut]
    if prefetch:
        fetch = rnd2.choice(["ok", "ok", "ok", "fail", "killed"])
        calls.insert(0, {"ok": (0, "fetched\\n"), "fail": (101, "1\\n2\\n3\\n4\\n5\\n6\\nerror: net down\\n"),
                         "killed": (137, "")}[fetch])
        kind += "+fetch-" + fetch
    return calls, kind


def podman_stub(calls):
    arms = []
    for i, (rc, text) in enumerate(calls, 1):
        arms.append("  %d) printf '%%b' '%s'; exit %d ;;" % (i, text, rc))
    return ("mx_stub podman <<STUB\nn=\\$(cat \"$MX_WORK/podman.count\" 2>/dev/null || echo 0); n=\\$((n+1)); "
            "echo \\$n > \"$MX_WORK/podman.count\"\ncase \\$n in\n" + "\n".join(arms) + "\n  *) exit 0 ;;\nesac\nSTUB")


def gh_files_stub(paths, rc=0, login="testuser"):
    arm = ("printf '%s\\n' " + " ".join("'%s'" % p for p in paths)) if paths else "true"
    return ("mx_stub gh <<'STUB'\ncase \"$*\" in\n  *'auth switch'*) exit 0 ;;\n  *'api user'*) echo %s ;;\n"
            "  *'--json files'*) %s; exit %d ;;\nesac\nSTUB\n") % (login, arm, rc)


def suite_dir_lines(where, suites, rnd2, flavours):
    """Writes the suites where the gate looks, in the form `where` says. Returns the lines."""
    lines = []
    if where in ("profile", "empty"):
        target = '"$MX_WORK/profile/verify.d"'
    elif where == "home":
        target = '"$MX_HOME/.local/share/maintainer/profiles/magent/verify.d"'
    else:
        return lines
    lines.append("mkdir -p %s" % target)
    for name in (suites if where != "empty" else []):
        lines.append("cat > %s/%s.sh <<'SUITE'\n%sSUITE" % (target, name, suite_text(name, SUITES[name], rnd2, flavours.get(name, []))))
    if where in ("profile", "empty"):
        lines.append('mx_env PROFILE_DIR "$MX_WORK/profile"')
    return lines


def pick_flavour(rnd2):
    fl = []
    if rnd2.random() < 0.3:
        fl.append("extra")
    if rnd2.random() < 0.08:
        fl.append("extra-rt")
    if rnd2.random() < 0.25:
        fl.append("limits")
        if rnd2.random() < 0.5:
            fl.append("limits-all")
    if rnd2.random() < 0.15:
        fl.append("work")
    if rnd2.random() < 0.1:
        fl.append("image-env")
    if rnd2.random() < 0.06:
        fl.append("requires-podman")
    elif rnd2.random() < 0.06:
        fl.append("requires-never")
    return fl


HEADS = ["full"] * 14 + ["short7", "short7", "short12", "wrong", "tooshort", "upper"]
FILTERS = ["guard"] * 14 + ["unit::guard_holds", "a b", "a.b/c-d_e:f", "x" * 40, "a;b", "-x", "a$b", "a'b"]
MUTATIONS = ["s/v0/v1/"] * 6 + ["s/v0/v1/g", "s/v0/v1/;s/v1/v2/", "s/zzz/y/", "s/v0/v0/", "s/v0/v1", "w /tmp/x", "s/v0/v1/e", "/v0/d"]


# A "deep" scenario draws only well-formed inputs, so it reaches the container and the receipt and
# varies what happens there; the others draw from the full lists above and mostly end in an early
# refusal, which is what those lists are for.
DEEP_VERIFY_PATHS = [["bin/a"], ["bin/a"], ["lib/b"], ["bin/a", "lib/b"], ["bin/a", "lib/b"], ["docs/d"], ["docs/d", "docs/d.md"]]
DEEP_PART_PATHS = [["bin/a"], ["lib/b"], ["bin/a", "lib/b"], ["bin/a", "docs/d"], ["bin/a", "lib/b", "docs/d"]]
DEEP_HEADS = ["full"] * 8 + ["short7", "short12"]
DEEP_FILTERS = ["guard"] * 10 + ["unit::guard_holds", "a b", "a.b/c-d_e:f", "x" * 40]
DEEP_MUTATIONS = ["s/v0/v1/"] * 8 + ["s/v0/v1/g", "s/v0/v1/;s/v1/v2/", "s/zzz/y/", "s/v0/v0/"]
UNIT_COVERS = ("bin/a", "bin/x.sh")


def unit_covers(paths):
    return all(p in UNIT_COVERS or p.startswith("lib/") for p in paths)


def docs_cover(paths):
    return all(p.startswith("docs/") for p in paths)


def head_expr(kind):
    return {
        "full": '"$C_HEAD"', "short7": '"${C_HEAD:0:7}"', "short12": '"${C_HEAD:0:12}"',
        "wrong": "deadbee", "tooshort": "abc123", "upper": '"${C_HEAD^^}"',
    }[kind]


def verify_scenario(i, rnd2, part):
    pr = rnd2.randint(1, 99)
    deep = rnd2.random() < 0.7
    where = rnd2.choice(["profile"] * 8 + ["home", "home"]) if deep else rnd2.choice(["profile"] * 6 + ["home", "home", "tree", "none", "empty"])
    if deep:
        paths = rnd2.choice(DEEP_PART_PATHS if part else DEEP_VERIFY_PATHS)
        suite_names = rnd2.choice([["unit", "docs"], ["unit", "lib", "docs"], ["unit", "docs", "named"]])
    else:
        paths = rnd2.choice(PATHSETS)
        suite_names = rnd2.choice([["unit", "docs"], ["unit", "lib", "docs"], ["wide", "docs"], ["unit", "docs", "named"],
                                   ["wide", "named"], ["lib", "docs"], ["unit"]])
    flavours = {n: pick_flavour(rnd2) for n in suite_names}
    named = None
    if deep:
        if part:
            named = rnd2.choice(["unit", "unit", "unit", "lib", "docs"])
        else:
            covering = "unit" if unit_covers(paths) else "docs" if docs_cover(paths) else None
            named = rnd2.choice([None, None, covering]) if covering else None
    elif part or rnd2.random() < 0.3:
        named = rnd2.choice(suite_names + ["nosuch", "nosuch", None] if part else suite_names + ["nosuch"])
    if where == "tree":
        paths = rnd2.choice([["bin/x.sh"], ["docs/d.md"], ["bin/x.sh", "docs/d.md"], ["bin/a"]])
        suite_names = ["shell", "docs", "characterization"]
        if named:
            named = rnd2.choice(["shell", "docs", "characterization", "nosuch"])
    # The prefetch belongs to the suite the run will use when it can be known (a deep scenario),
    # so the stub's first answer is the fetch's and not the unmutated run's.
    prefetch_suite = None
    if rnd2.random() < 0.3:
        if deep:
            used = named or ("unit" if unit_covers(paths) else "docs" if docs_cover(paths) else None)
            prefetch_suite = used if used in suite_names else None
        else:
            prefetch_suite = rnd2.choice(suite_names)
        if prefetch_suite:
            flavours[prefetch_suite] = flavours.get(prefetch_suite, []) + ["prefetch"] + rnd2.choice([[], [], ["guard-ok"], ["guard-refuses"]])
    head = rnd2.choice(DEEP_HEADS if deep else HEADS)
    flt = rnd2.choice(DEEP_FILTERS if deep else FILTERS)
    mut = rnd2.choice(DEEP_MUTATIONS if deep else MUTATIONS)
    calls, outcome = podman_calls(rnd2, prefetch_suite is not None)
    s = ["#!/usr/bin/env bash", "# generated %s scenario %d (%s, suites %s, podman %s)" % (
        "verify-part" if part else "verify", i, where, ",".join(suite_names), outcome),
        "mx_merge_env", "mx_env PROD_GLOBS '%s'" % rnd2.choice(["bin/* lib/*"] * (9 if deep else 5) + ["bin/* lib/* docs/*", "*"])]
    s += suite_dir_lines(where, suite_names if where != "tree" else [], rnd2, flavours)
    if where == "none":
        s.append("mx_env MAINTAINER_PROFILE nosuchprofile")
    if where == "profile" and rnd2.random() < 0.1:
        # a second directory behind the first: the first one that exists wins
        s.append('mkdir -p "$MX_HOME/.local/share/maintainer/profiles/magent/verify.d"')
    if where == "home" and rnd2.random() < 0.1:
        s.append('mkdir -p "$MX_WORK/profile/verify.d"; mx_env PROFILE_DIR "$MX_WORK/profile"')
    # the repository: a pull request ref that holds the head, tracked files to mutate
    s.append("mx_git_repo")
    s.append('mx_commit "$MX_REPO" base ' + " ".join('%s %s' % (FIXTURE[k], FIXTURE[k + 1]) for k in range(0, len(FIXTURE), 2)))
    shape = rnd2.choice(["plain"] * 12 + ["escape", "inside-link", "nested-glob"])
    if shape == "escape":
        s.append('ln -s /etc/hostname "$MX_REPO/bin/lnk"; git -C "$MX_REPO" add bin/lnk; mx_commit "$MX_REPO" link')
    elif shape == "inside-link":
        s.append('ln -s a "$MX_REPO/bin/ok"; git -C "$MX_REPO" add bin/ok; mx_commit "$MX_REPO" link')
    elif shape == "nested-glob":
        s.append('mx_commit "$MX_REPO" nested deep/dir/a v0 .container-target/a v0')
    fetch = (not deep) and rnd2.random() < 0.1
    if fetch:
        s.append('git -C "$MX_REPO" remote add origin "$MX_WORK/does-not-exist.git"')
    else:
        s.append("mx_pr_origin %d" % pr)
    s.append('C_HEAD="$(git -C "$MX_REPO" rev-parse HEAD)"')
    # a receipt already on disk
    prior = rnd2.choice(["none"] * 8 + ["same-head", "same-head", "other-head", "tampered", "garbage"])
    if prior == "same-head":
        s.append('mx_receipt %d "$C_HEAD" observed "earlier proof"' % pr)
    elif prior == "other-head":
        s.append('mx_receipt %d 0000000000000000000000000000000000000000 observed "earlier proof"' % pr)
    elif prior == "tampered":
        s.append('mx_receipt %d "$C_HEAD" observed "earlier proof"; sed -i \'s/earlier/edited/\' "$MX_STATE/receipts/%d.json"' % (pr, pr))
    elif prior == "garbage":
        s.append('mkdir -p "$MX_STATE/receipts"; printf "not json\\n" > "$MX_STATE/receipts/%d.json"' % pr)
    files_rc = 0 if deep or rnd2.random() > 0.08 else 1
    if files_rc:
        s.append("# decision: P3W-6")
    s.append(gh_files_stub(paths, files_rc).rstrip())
    s.append(podman_stub(calls))
    if (not deep) and rnd2.random() < 0.06:
        s.append('rm -f "$MX_STUBS/podman"')
    # environment knobs the limits and timeouts read
    for name, values in (("MAINTAINER_VERIFY_MEMORY", ["8g", "512m", "1g"]), ("MAINTAINER_VERIFY_CPUS", ["1", "4", "0.5"]),
                         ("MAINTAINER_VERIFY_PIDS", ["64", "1000"]), ("MAINTAINER_VERIFY_TMPFS", ["16m", "2g"]),
                         ("MAINTAINER_VERIFY_TIMEOUT", ["30", "900"]), ("MAINTAINER_PREFETCH_TIMEOUT", ["45", "120"]),
                         ("MAINTAINER_CARGO_JOBS", ["1", "3"]), ("MAINTAINER_SHELL_IMAGE", ["docker.io/library/python:3.11"]),
                         ("CHARACTERIZATION_DIR", ["/somewhere/cases"])):
        if rnd2.random() < 0.12:
            s.append("mx_env %s %s" % (name, rnd2.choice(values)))
    if rnd2.random() < 0.03:
        s.append("# decision: P30-3\nmx_env MAINTAINER_VERIFY_MEMORY 0")
    if rnd2.random() < 0.03:
        s.append("# decision: P30-4\nmx_env MAINTAINER_VERIFY_TIMEOUT soon")
    if rnd2.random() < 0.08:
        s.append("mx_env MAINTAINER_IN_RUN review")
    verb = "verify-part" if part else "verify"
    parts = [verb, str(pr), head_expr(head), shlex.quote(flt), shlex.quote(mut)]
    if named is not None:
        parts.append(named)
    s.append("mx_run bin/maintainer-merge " + " ".join(parts))
    return "\n".join(s) + "\n"


def partial_receipt_scenario(i, rnd2):
    """merge on a receipt proved in parts: the suites it names must cover the production paths."""
    pr = rnd2.randint(1, 99)
    deep = rnd2.random() < 0.7
    where = rnd2.choice(["profile"] * 8 + ["home", "home"]) if deep else rnd2.choice(["profile"] * 6 + ["home", "home", "tree", "none", "empty"])
    suite_names = rnd2.choice([["unit", "docs"], ["unit", "lib", "docs"]] if deep else [["unit", "docs"], ["unit", "lib", "docs"], ["wide", "docs"], ["lib", "docs"], ["unit"]])
    if where == "tree":
        suite_names = ["shell", "docs"]
    if deep:
        receipt_suites = rnd2.choice([["unit"], ["unit", "docs"], ["lib"], ["docs"], ["unit", "lib"], ["unit", "unit"], ["unit", "lib", "docs"]])
        paths = rnd2.choice(DEEP_PART_PATHS + [["docs/d"], ["bin/x.sh"], ["lib/b", "tests/t"]])
        partial = rnd2.random() < 0.9
        prod = rnd2.choice(["bin/* lib/*"] * 7 + ["bin/* lib/* docs/*", "bin/a", "tests/*"])
        state = rnd2.choice(["CLEAN"] * 12 + ["BEHIND"])
        review = rnd2.choice(["APPROVED"] * 12 + ["REVIEW_REQUIRED"])
        files_rc = 0
    else:
        receipt_suites = rnd2.choice([
            ["unit"], ["unit", "docs"], ["lib"], ["docs"], ["unit", "lib"], ["wide"], ["nosuch"], ["unit", "nosuch"], [],
            ["shell"], ["docs", "shell"], ["unit", "unit"], ["named"],
        ])
        paths = rnd2.choice(PATHSETS + [["bin/x.sh"], ["docs/d.md"], [], ["lib/b", "tests/t"]])
        partial = rnd2.random() < 0.8
        prod = rnd2.choice(["bin/* lib/*"] * 4 + ["bin/* lib/* docs/*", "bin/a", "tests/*", "*", ""])
        state = rnd2.choice(["CLEAN"] * 8 + ["BEHIND", "BLOCKED"])
        review = rnd2.choice(["APPROVED"] * 8 + ["REVIEW_REQUIRED"])
        files_rc = 0 if rnd2.random() > 0.06 else 1
    s = ["#!/usr/bin/env bash", "# generated partial-receipt merge scenario %d (%s)" % (i, where), "mx_merge_env",
         "mx_env PROD_GLOBS '%s'" % prod]
    if rnd2.random() < 0.15:
        s.append("mx_env MAINTAINER_IN_RUN review")
    if rnd2.random() < 0.2:
        s.append("mx_env MAINTAINER_POST off")
    s += suite_dir_lines(where, suite_names if where != "tree" else [], rnd2, {})
    if where == "none":
        s.append("mx_env MAINTAINER_PROFILE nosuchprofile")
    s.append("mx_git_repo")
    s.append('mx_commit "$MX_REPO" base ' + " ".join('%s %s' % (FIXTURE[k], FIXTURE[k + 1]) for k in range(0, len(FIXTURE), 2)))
    s.append("mx_pr_origin %d" % pr)
    s.append('C_HEAD="$(git -C "$MX_REPO" rev-parse HEAD)"')
    s.append("mx_receipt_parts %d \"$C_HEAD\" %s %s" % (pr, "true" if partial else "false", " ".join(receipt_suites) or "''"))
    if rnd2.random() < 0.08:
        s.append("sed -i 's/units_run/units_ran/' \"$MX_STATE/receipts/%d.json\"" % pr)
    gh = [
        "mx_stub gh <<STUB", 'case "\\$*" in', "  *'auth switch'*) exit 0 ;;", "  *'api user'*) echo testuser ;;",
        "  *'--json files'*) %s; exit %d ;;" % (("printf '%s\\n' " + " ".join("'%s'" % p for p in paths)) if paths else "true", files_rc),
        "  *headRefOid*) echo $C_HEAD ;;", "  *mergeStateStatus*) echo %s ;;" % state,
        "  *reviewDecision*) echo %s ;;" % review, "  *closingIssuesReferences*) ;;",
        "  *'pr checks'*) printf '%s\\\\n' '[{\"name\":\"c0\",\"bucket\":\"pass\"}]' ;;",
        "  *'pr merge'*) echo 'stub gh: pr merge ran' ;;", "esac", "STUB",
    ]
    s.append("\n".join(gh))
    s.append("mx_run bin/maintainer-merge merge %d" % pr)
    return "\n".join(s) + "\n"


def directed_scenario(i, kind, rnd2):
    """One scenario that exists to show one difference the random ones reach only by luck."""
    pr = 70 + i % 20
    head = '"$C_HEAD"'
    calls = [(0, CLEAN_OK), (1, MUT_FAIL)]
    mutation, verb, named, files = "s/v0/v1/", "verify", "", ["bin/a"]
    if kind == "empty-part":
        verb = "verify-part"
    elif kind.startswith("sed-"):
        mutation = {"sed-w": "w /tmp/x", "sed-e": "s/v0/v1/e", "sed-open": "s/v0/v1", "sed-r": "r /etc/hostname"}[kind]
    s = ["#!/usr/bin/env bash", "# generated directed scenario %d (%s)" % (i, kind), "mx_merge_env",
         "mx_env PROD_GLOBS 'bin/* lib/*'"]
    s += suite_dir_lines("empty" if kind == "empty-part" else "profile", ["unit"], rnd2, {})
    s.append("mx_git_repo")
    s.append('mx_commit "$MX_REPO" base ' + " ".join('%s %s' % (FIXTURE[k], FIXTURE[k + 1]) for k in range(0, len(FIXTURE), 2)))
    s.append("mx_pr_origin %d" % pr)
    s.append('C_HEAD="$(git -C "$MX_REPO" rev-parse HEAD)"')
    s.append(gh_files_stub(files).rstrip())
    s.append(podman_stub(calls))
    s.append("mx_run bin/maintainer-merge %s %d %s guard %s%s" % (verb, pr, head, shlex.quote(mutation), named))
    return "\n".join(s) + "\n"


rnd2 = random.Random(seed + 1)
vmakers = [
    (lambda i: verify_scenario(i, rnd2, False), "verify", 100),
    (lambda i: verify_scenario(i, rnd2, True), "verify-part", 55),
    (lambda i: partial_receipt_scenario(i, rnd2), "partial", 45),
]
vweights = [w for _, _, w in vmakers]
for i in range(verify_count):
    fn, name, _ = rnd2.choices(vmakers, vweights)[0]
    with open(os.path.join(out, f"v{i:03d}-{name}.sh"), "w") as f:
        f.write(fn(i))
DIRECTED = ["empty-part", "sed-w", "sed-e", "sed-open", "sed-r"]
for k, kind in enumerate(DIRECTED):
    with open(os.path.join(out, f"v{verify_count + k:03d}-directed-{kind}.sh"), "w") as f:
        f.write(directed_scenario(verify_count + k, kind, rnd2))
print("generated", count, "scenarios and", verify_count + len(DIRECTED), "verify scenarios in", out)
