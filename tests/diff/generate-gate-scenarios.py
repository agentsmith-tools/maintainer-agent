#!/usr/bin/env python3
"""Seeded generator of differential scenarios for maintainer-merge (bash against Rust).

    python3 tests/diff/generate-gate-scenarios.py <out-dir> [seed [count]]

Writes `count` scenario files (default 330, seed 20261122) for tests/diff/record.sh: merge with
random receipts, heads, merge states, review decisions, check boards and identities; update with
random facts and reviews; receipt, show and usage. Run record.sh on the bash tree and on the tree
assembled with the Rust binary, then compare.sh. The differences that remain are the decisions of
the gate port: the flock stub calls, receipt signatures over a timestamp, and the wrapper's
MAINTAINER_CHILD_ENV in the gh environment log.
"""
import random, sys, os

seed = int(sys.argv[2]) if len(sys.argv) > 2 else 20261122
count = int(sys.argv[3]) if len(sys.argv) > 3 else 330
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
print("generated", count, "scenarios in", out)
