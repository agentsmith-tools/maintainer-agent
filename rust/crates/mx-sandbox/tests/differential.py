#!/usr/bin/env python3
"""The comparisons behind differential.sh. Not meant to run alone.

Each comparison runs one function of `bin/maintainer-merge` (sourced, never executed, so the
function is the shipped text) and the same operation of the Rust probe
(`examples/p3a-probe.rs`) on identical inputs, and compares the exit code, standard output
and standard error. Both sides run under `env -i` with `LC_ALL=C`, because bash orders `*.sh`
and matches `?` by the locale and the port decides on bytes.

A comparison is IDENTICAL (every byte equal) or DECISION: it exercises a difference the pull
request records on purpose, and `explain` must accept the exact difference. Anything else is
UNEXPLAINED and fails the run.

Sections: glob (is_production against ProdGlobs::matches), realpath and escapes (tree_escapes
and Python's realpath), selection (pick_suite, suites_cover_production, coverage_map), ran
(the `[ "${ran:-0}" -ge 1 ]` reading of suite_ran), mutation (apply_mutation), extraction
(extract_tree), flags (the text of `run_in_container` and of the prefetch `podman run`, taken
from the file and run against a podman stub that logs its argv).
"""

import hashlib
import os
import random
import re
import shutil
import subprocess
import sys
from pathlib import Path

MM = Path(os.environ["MX_MM"])
TREE = MM.parent.parent
PROBE = os.environ["MX_PROBE"]
SCRATCH = Path(os.environ["MX_SCRATCH"])
SEED = int(os.environ["DIFF_SEED"])
ONLY = [s for s in os.environ.get("DIFF_ONLY", "").split(",") if s]
VERBOSE = os.environ.get("DIFF_VERBOSE") == "1"
RNG = random.Random(SEED)

BASE_PATH = "/usr/bin:/bin"
ENV0 = {"PATH": BASE_PATH, "LC_ALL": "C", "HOME": "/nonexistent-home",
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null"}

counts = {"identical": 0, "decision": 0, "unexplained": 0}
by_section = {}
failures = []


def run(argv, env=None, cwd=None, stdin=None):
    full = dict(ENV0)
    if env:
        full.update(env)
    p = subprocess.run(argv, env=full, cwd=cwd, input=stdin, capture_output=True, timeout=120)
    return p.returncode, p.stdout, p.stderr


def bash(script, args=(), env=None, cwd=None, stdin=None):
    return run(["bash", "-c", script, "bash", *args], env, cwd, stdin)


def probe(args, env=None, cwd=None, stdin=None):
    return run([PROBE, *args], env, cwd, stdin)


def record(section, name, a, b, explain=None):
    """a = bash result, b = rust result; each (rc, stdout, stderr)."""
    if ONLY and not any(o in name or o in section for o in ONLY):
        return
    by_section.setdefault(section, [0, 0])
    if a == b:
        counts["identical"] += 1
        by_section[section][0] += 1
        if VERBOSE:
            print(f"  same      {section}/{name}")
        return
    why = explain(a, b) if explain else None
    if why:
        counts["decision"] += 1
        by_section[section][1] += 1
        if VERBOSE:
            print(f"  decision  {section}/{name}: {why}")
        return
    counts["unexplained"] += 1
    failures.append((section, name, a, b))
    print(f"UNEXPLAINED {section}/{name}")
    for label, r in (("bash", a), ("rust", b)):
        print(f"  {label}: rc={r[0]} out={r[1][:300]!r} err={r[2][:400]!r}")


NOISE = re.compile(rb"^(?:" + re.escape(str(MM).encode()) + rb": line \d+: suite_covers: command not found|[^\n]*\.sh: line \d+: [^\n]*)\n", re.M)


def without_suite_noise(r):
    return (r[0], r[1], NOISE.sub(b"", r[2]))


def noise_only(x, y):
    """The bash printed `suite_covers: command not found`, or the shell's syntax error, for a
    broken suite; the host answers from the exit status and keeps a suite's own standard error to
    itself (P3A-6)."""
    if without_suite_noise(x) == y:
        return "P3A-6: a suite's own stderr (a missing suite_covers, a syntax error) is not passed through"
    return None


def text(b):
    return b.decode("utf-8", "surrogateescape")


# ------------------------------------------------------------------------------------------
# glob: is_production against ProdGlobs::matches
# ------------------------------------------------------------------------------------------

def gen_pattern():
    pieces = ["a", "b", "c", "x", "/", ".", "-", "_", "*", "*", "?", "[a-c]", "[!a]", "[^b]", "[]a]",
              "[a-]", "[z-a]", "[", "\\*", "\\?", "\\[", "[[:alpha:]]", "bin/", "lib/", "src/", ".github/", "a b",
              "[*]", "[?]", "[\\]]", "é", "[é]", "*.rs", "**"]
    return "".join(RNG.choice(pieces) for _ in range(RNG.randint(1, 6)))


def gen_path():
    chars = list("abcx/./-_*?[]\\é01 ")
    return "".join(RNG.choice(chars) for _ in range(RNG.randint(0, 9)))


EXT_PIECES = ["a", "b", "c", ".", "/", "x", "src", "bin/", "*", "?", "-", "_", "1", "é"]


def gen_extended():
    """Patterns with POSIX classes, equivalence classes, collating symbols and extglob, which
    the first matcher did not have (#275)."""
    def group(depth):
        alts = ["".join(RNG.choice(EXT_PIECES) for _ in range(RNG.randint(0, 3))) for _ in range(RNG.randint(1, 3))]
        if depth < 2 and RNG.random() < 0.3:
            alts[0] += RNG.choice("?*+@!") + "(" + group(depth + 1) + ")"
        return "|".join(alts)
    classes = ["[[:alpha:]]", "[[:digit:]]", "[![:digit:]]", "[^[:upper:]]", "[[:alnum:]_]", "[[:punct:]]", "[[:space:]]",
               "[[:lower:][:digit:]]", "[[=a=]]", "[[.a.]]", "[[=b=]c]", "[[:xdigit:]]", "[[:cntrl:]]", "[[:print:]]", "[[:graph:]]",
               "[[:blank:]]", "[a-c[:digit:]]", "[]a]", "[!]a]"]
    out = []
    for _ in range(RNG.randint(1, 4)):
        r = RNG.random()
        if r < 0.45:
            out.append(RNG.choice("?*+@!") + "(" + group(0) + ")")
        elif r < 0.75:
            out.append(RNG.choice(classes))
        else:
            out.append(RNG.choice(EXT_PIECES))
    return "".join(out)


def section_glob_extended(count):
    pairs = []
    for _ in range(count):
        pat = gen_extended().replace(" ", "_")
        path = "".join(RNG.choice(list("abcx/.-_*?1 [](|)@!+é\\") + ["A", "Z", "9", "src/", "bin/"]) for _ in range(RNG.randint(0, 8)))
        pairs.append((pat, path))
    pairs += [("@(a|b)", "a"), ("+(ab)", "abab"), ("!(a)", "b"), ("!(a)", "a"), ("?(a)b", "b"), ("*(a|b)c", "abac"),
              ("[[:alpha:]]", "a"), ("[![:digit:]]", "5"), ("src/@(a|b).rs", "src/a.rs"), ("bin/!(*.md)", "bin/x.md"),
              ("@(a", "@(a"), ("@()", ""), ("!()", "x"), ("[[:nosuch:]]", "a")]
    stdin = "".join(f"{p}\t{q}\n" for p, q in pairs).encode()
    script = 'while IFS=$\'\\t\' read -r g p; do if [[ "$p" == $g ]]; then echo 1; else echo 0; fi; done'
    a = bash(script, stdin=stdin)
    b = probe(["glob"], stdin=stdin)
    la, lb = text(a[1]).split("\n"), text(b[1]).split("\n")
    assert len(la) == len(lb), (len(la), len(lb))
    for i, (x, y) in enumerate(zip(la, lb)):
        pat, path = pairs[i] if i < len(pairs) else ("", "")

        def explain(xa, yb, pat=pat):
            # Fail closed (P3A-16): the port may say production where bash says not, never the reverse.
            if xa[1] == b"0" and yb[1] == b"1":
                return "P3A-16: a construct the matcher does not know answers production"
            return None
        record("glob-extended", f"{pat!r} vs {path!r}", (a[0], x.encode(), b""), (b[0], y.encode(), b""), explain)


def gen_nested():
    """Nested negation and multibyte atoms, the classes of pattern the first extglob matcher got
    wrong or could not decide (rv280 round 2)."""
    atoms = ["a", "b", "é", "?", "*", "[[=a=]]", "[[:alpha:]]", "[!a]", "\\?", ".", "/"]

    def neg(depth):
        inner = "|".join("".join(RNG.choice(atoms) for _ in range(RNG.randint(0, 2))) for _ in range(RNG.randint(1, 3)))
        if depth < 2 and RNG.random() < 0.5:
            inner += RNG.choice(["|", ""]) + RNG.choice("!@?*+") + "(" + neg(depth + 1) + ")"
        return inner
    parts = []
    for _ in range(RNG.randint(1, 4)):
        r = RNG.random()
        if r < 0.5:
            parts.append(RNG.choice("!!@?*+") + "(" + neg(0) + ")")
        else:
            parts.append(RNG.choice(atoms))
    return "".join(parts)


def section_glob_nested(count):
    locales = ["C"] + [l for l in ("en_US.UTF-8", "C.UTF-8") if subprocess.run(["locale", "-a"], capture_output=True).stdout.decode().lower().count(l.lower().replace("-", "")) or l == "C.UTF-8"][:1]
    for locale in locales:
        pairs = []
        for _ in range(count):
            pat = gen_nested().replace(" ", "_")
            path = "".join(RNG.choice(list("abé/.?*") + ["aa", "é"]) for _ in range(RNG.randint(0, 6)))
            pairs.append((pat, path))
        pairs += [("!(a)*!(a|!(b))", "x"), ("*!(@(a|b))*c\\?", "zz"), ("@(a|b|c)*!(a)*[[=a=]]", "q"), ("??", "é"), ("[!]]", "é"), ("[[:alpha:]]", "é")]
        stdin = "".join(f"{p_}\t{q}\n" for p_, q in pairs).encode()
        script = 'while IFS=$\'\\t\' read -r g p; do if [[ "$p" == $g ]]; then echo 1; else echo 0; fi; done'
        a = bash(script, stdin=stdin, env={"LC_ALL": locale})
        b = probe(["glob"], stdin=stdin)
        la, lb = text(a[1]).split("\n"), text(b[1]).split("\n")
        assert len(la) == len(lb), (len(la), len(lb))
        for i, (x, y) in enumerate(zip(la, lb)):
            pat, path = pairs[i] if i < len(pairs) else ("", "")

            def explain(xa, yb):
                # Fail closed (P3A-16): production where bash says not is allowed, never the reverse.
                if xa[1] == b"0" and yb[1] == b"1":
                    return "P3A-16: the matcher answers production where it cannot be sure"
                return None
            record("glob-nested", f"[{locale}] {pat!r} vs {path!r}", (a[0], x.encode(), b""), (b[0], y.encode(), b""), explain)


def section_glob(count):
    pairs = []
    for _ in range(count):
        pat = gen_pattern()
        if " " in pat:
            pat = pat.replace(" ", "_")
        pairs.append((pat, gen_path()))
    pairs += [("bin/*", "bin/a/b"), ("bin/*", "bin/"), ("*", ""), ("*", "a/b"), ("?", "/"), (".github/*", ".github/x.yml"),
              ("[a-c]*", "b/c"), ("a*b*c", "aXbYc"), ("a*b*c", "ac"), ("*.rs", "src/a.rs"), ("\\*", "*"), ("[", "["), ("[a", "[a")]
    stdin = "".join(f"{p}\t{q}\n" for p, q in pairs).encode()
    script = 'while IFS=$\'\\t\' read -r g p; do if [[ "$p" == $g ]]; then echo 1; else echo 0; fi; done'
    a = bash(script, stdin=stdin)
    b = probe(["glob"], stdin=stdin)
    la, lb = text(a[1]).split("\n"), text(b[1]).split("\n")
    assert len(la) == len(lb), (len(la), len(lb), text(b[2])[:300])
    for i, (x, y) in enumerate(zip(la, lb)):
        pat, path = pairs[i] if i < len(pairs) else ("", "")
        def closed(xa, yb):
            # Fail closed (P3A-16, P3A-19): production where bash says not, never the reverse.
            if xa[1] == b"0" and yb[1] == b"1":
                return "P3A-16: the matcher answers production where it cannot be sure"
            return None
        record("glob", f"{pat!r} vs {path!r}", (a[0], x.encode(), b""), (b[0], y.encode(), b""), closed)


# ------------------------------------------------------------------------------------------
# realpath and escapes
# ------------------------------------------------------------------------------------------

def build_link_tree(root, case):
    dirs = ["d1", "d1/d2", "d3", "d1/d2/d4", "x"]
    for d in dirs:
        (root / d).mkdir(parents=True, exist_ok=True)
    (root / "f").write_text("x")
    (root / "d1/g").write_text("y")
    outside = root.parent / "outside"
    outside.mkdir(exist_ok=True)
    (outside / "o").write_text("o")
    targets = ["f", "d1", "d1/g", "../f", "../../f", "../outside", "../outside/o", "../../outside", f"{root}/f",
               f"{root}/d1", f"{outside}/o", "/etc/passwd", "/", "/tmp", "nothing", "d1/nothing/../g",
               "../nothing", "./f", "d1/./d2/../g", "l0", "l1", "l2", "l3", "l4", "..", ".", "../..", "d1/..", f"../{root.name}/f",
               f"../{root.name}/../outside", "d1//g", "d1/", "x/../../outside"]
    names = [f"l{i}" for i in range(5)] + ["d1/m", "d1/d2/n", "d3/p", "x/q", "d1/d2/d4/r", "d3/s"]
    for name in names:
        t = RNG.choice(targets)
        link = root / name
        if not link.parent.exists():
            continue
        os.symlink(t, link)
    return root


def section_escapes(count):
    for i in range(count):
        base = SCRATCH / f"esc{i}"
        shutil.rmtree(base, ignore_errors=True)
        root = base / "tree"
        root.mkdir(parents=True)
        build_link_tree(root, i)
        script = '. "$MM"; tree_escapes "$1"'
        a = bash(script, [str(root)], env={"MM": str(MM), "PYTHONDONTWRITEBYTECODE": "1"})
        b = probe(["escapes", str(root)])
        # bash prints a blank line for an empty list (print of ""), the probe prints one too.
        record("escapes", f"tree{i}", a, b)
        links = [str(p) for p in root.rglob("*") if p.is_symlink()]
        py = subprocess.run(["python3", "-I", "-c", "import os,sys\nfor p in sys.argv[1:]: print(os.path.realpath(p))", *links],
                            capture_output=True, env={"PATH": BASE_PATH})
        rs = probe(["realpath", *links])
        record("realpath", f"tree{i}", (py.returncode, py.stdout, b""), (rs[0], rs[1], b""))
        shutil.rmtree(base, ignore_errors=True)
    # a name that is not UTF-8: bash under a UTF-8 locale dies in print and reads "no escapes".
    base = SCRATCH / "esc-bytes"
    shutil.rmtree(base, ignore_errors=True)
    root = base / "tree"
    root.mkdir(parents=True)
    os.symlink("/etc/passwd", os.fsencode(root) + b"/bad\xff")
    b = probe(["escapes", str(root)])
    for locale in ("C.UTF-8", "en_US.UTF-8"):
        a = bash('. "$MM"; tree_escapes "$1"', [str(root)], env={"MM": str(MM), "LC_ALL": locale, "PYTHONDONTWRITEBYTECODE": "1"})

        def explain(x, y, locale=locale):
            if x[1].strip() == b"" and y[1].strip().endswith(b"/etc/passwd"):
                return "P3A-8: under a strict UTF-8 locale python dies on the name and the bash reads that as no escape"
            if x[1].replace(b"\xff", b"\xef\xbf\xbd") == y[1]:
                return "P3A-8: the name is shown with U+FFFD where it holds a byte that is not UTF-8"
            return None
        record("escapes", f"non-utf8-name-under-{locale}", a, b, explain)
    shutil.rmtree(base, ignore_errors=True)


# ------------------------------------------------------------------------------------------
# selection: pick_suite, suites_cover_production, coverage_map
# ------------------------------------------------------------------------------------------

COVER_PATTERNS = ["*.rs", "bin/*", "lib/*", "docs/*", "*.md", "Makefile", "src/a?.rs", "*/Cargo.toml", "tests/*",
                  ".github/*", "*.sh", "*.py", "a b/*", "Cargo.*", "README.md", "src/*"]
PATHS = ["bin/x", "bin/run", "lib/a.sh", "lib/b/c.py", "docs/a.md", "README.md", "src/a1.rs", "src/a2.rs", "src/b.rs",
         "Makefile", "a b/c.md", ".github/ci.yml", "tests/t.rs", "ünï.rs", "crates/x/Cargo.toml", "Cargo.lock", "x.py", "z.txt"]
GLOBS = ["bin/* lib/*", "src/* Makefile", ".github/*", "*.rs", "", "docs/*", "bin/* src/*", "lib/*"]
SUITE_NAMES = ["rust", "shell", "docs", "aaa", "Zed", "x1", "a_b", "py", "node"]


def gen_suite_dir(base):
    verify = base / "verify.d"
    shutil.rmtree(base, ignore_errors=True)
    verify.mkdir(parents=True)
    names = RNG.sample(SUITE_NAMES, RNG.randint(1, 4))
    for n in names:
        kind = RNG.random()
        pats = RNG.sample(COVER_PATTERNS, RNG.randint(1, 4))
        body = 'suite_covers() { case "$1" in ' + "|".join(f"'{p}'" if "*" not in p and "?" not in p else p.replace(" ", "\\ ") for p in pats) + ') return 0 ;; esac; return 1; }\nsuite_image() { printf img; }\n'
        if kind < 0.1:
            body = "suite_image() { printf img; }\n"            # no suite_covers
        elif kind < 0.17:
            body = "exit 0\n"                                  # exits while sourced
        elif kind < 0.24:
            body = body + "if then (\n"                         # syntax error after the function
        if RNG.random() < 0.25:
            body += "suite_named_only() { return 0; }\n"
        (verify / f"{n}.sh").write_text(body)
    if RNG.random() < 0.2:
        (verify / "notes.txt").write_text("x")
    return verify, names


def section_selection(count):
    for i in range(count):
        verify, names = gen_suite_dir(SCRATCH / f"sel{i}")
        globs = RNG.choice(GLOBS)
        changed = RNG.sample(PATHS, RNG.randint(1, 5))
        named = RNG.choice(names + ["nope", "-", "-", "-"])
        env = {"MM": str(MM), "D": str(verify), "PROD_GLOBS": globs}
        script_pick = '. "$MM"; suite_dir() { printf "%s" "$D"; }; n="$1"; shift; [ "$n" = - ] && n=""; pick_suite "$n" "$@"'
        a = bash(script_pick, [named, *changed], env=env)
        b = probe(["pick", str(verify), globs, named, *changed])
        record("selection", f"pick{i}", a, b, noise_only)
        chosen = RNG.sample(names + ["nope"], RNG.randint(0, min(3, len(names) + 1)))
        script_cover = '. "$MM"; suite_dir() { printf "%s" "$D"; }; n="$1"; shift; suites_cover_production "$n" "$@"'
        a = bash(script_cover, [" ".join(chosen), *changed], env=env)
        b = probe(["cover", str(verify), globs, " ".join(chosen), *changed]) if chosen else None
        if b is None:
            b = probe(["cover", str(verify), globs, " ", *changed])
        record("selection", f"cover{i}", a, b, noise_only)
        a = bash('. "$MM"; suite_dir() { printf "%s" "$D"; }; d="$(suite_dir)"; coverage_map "$d" "$@"', changed, env=env)
        b = probe(["map", str(verify), *changed])
        record("selection", f"map{i}", a, b, noise_only)
        shutil.rmtree(SCRATCH / f"sel{i}", ignore_errors=True)
    # no verify.d at all
    a = bash('. "$MM"; suite_dir() { return 1; }; pick_suite "" bin/x', env={"MM": str(MM), "PROD_GLOBS": "bin/*"})
    b = probe(["pick", "/nonexistent/verify.d", "bin/*", "-", "bin/x"])
    record("selection", "no-verify-d", a, b)


# ------------------------------------------------------------------------------------------
# ran: [ "${ran:-0}" -ge 1 ]
# ------------------------------------------------------------------------------------------

def section_ran():
    verify = SCRATCH / "ran" / "verify.d"
    shutil.rmtree(verify.parent, ignore_errors=True)
    verify.mkdir(parents=True)
    (verify / "r.sh").write_text("suite_image() { printf i; }\nsuite_ran() { cat \"$1\"; }\n")
    log = verify.parent / "log"
    samples = ["0", "1", "12", "12\n", " 5", "5 ", "+5", "05", "007", "x", "", "1.0", "-3", "3 4", "99999999999999999999",
               "9223372036854775807", "9223372036854775808", "0x10", "1e3", "\t7", "7\t", "١٢", "٣"]
    for i, text_ in enumerate(samples):
        log.write_text(text_)
        a = bash('. "$1"; ran="$(suite_ran "$2")"; [ "${ran:-0}" -ge 1 ] 2>/dev/null; echo $?', [str(verify / "r.sh"), str(log)])
        b = probe(["ran", str(verify), "r", str(log)])
        n = int(b[1].decode() or "0") if b[0] == 0 else -1
        rust_says = b"0\n" if n >= 1 else b"1\n"
        # `[ "${ran:-0}" -ge 1 ] || die`: the status is 0 (accepted), 1 (a smaller number) or 2
        # (not an integer), and the gate refuses on any non-zero one.
        bash_says = b"0\n" if a[1] == b"0\n" else b"1\n"
        record("ran", f"{text_!r}", (a[0], bash_says, b""), (b[0], rust_says, b""))
    shutil.rmtree(verify.parent, ignore_errors=True)


# ------------------------------------------------------------------------------------------
# mutation: apply_mutation
# ------------------------------------------------------------------------------------------

EXPRESSIONS = ["s/a/b/", "s/a/a/", "/foo/d", "1d", "s/a/b/g;s/c/d/", "s/zzz/y/", "s/a/id/e", "w /tmp/mx-diff-never", "r /etc/hostname",
               "s/\\(a\\)/\\1x/", "$d", "y/abc/xyz/", "s/b/!/2"]
FILE_NAMES = ["a.rs", "b.rs", "src/c.rs", "src/d/e.py", "Makefile", "maintainer-x", "README.md", "x.sh", ".hidden.rs", "-n.rs",
              ".git/hooks/p.rs", ".container-target/t.rs", "deep/.container-target/u.rs", "docs/a.md", "uni é.rs"]
GLOB_POOL = ["*.rs", "*.py", "Makefile", "maintainer*", "*.sh", "*.md", "x*", "*"]


def build_plain_tree(root):
    for name in RNG.sample(FILE_NAMES, RNG.randint(1, 9)):
        p = root / name
        p.parent.mkdir(parents=True, exist_ok=True)
        lines = [RNG.choice(["a b c", "foo bar", "abc", "zzz", "plain", "a"]) for _ in range(RNG.randint(1, 4))]
        p.write_text("\n".join(lines) + RNG.choice(["", "\n"]))
    if RNG.random() < 0.3:
        os.symlink("a.rs", root / "link.rs")


def tree_digest(root):
    h = hashlib.sha256()
    for p in sorted(root.rglob("*"), key=lambda q: str(q)):
        rel = str(p.relative_to(root))
        if p.is_symlink():
            h.update(f"L {rel} {os.readlink(p)}\n".encode())
        elif p.is_file():
            h.update(f"F {rel} {oct(p.stat().st_mode)} ".encode() + hashlib.sha256(p.read_bytes()).digest())
        else:
            h.update(f"D {rel}\n".encode())
    return h.hexdigest()


def section_mutation(count):
    for i in range(count):
        base = SCRATCH / f"mut{i}"
        shutil.rmtree(base, ignore_errors=True)
        (base / "a").mkdir(parents=True)
        build_plain_tree(base / "a")
        shutil.copytree(base / "a", base / "b", symlinks=True)
        expr = RNG.choice(EXPRESSIONS)
        globs = RNG.sample(GLOB_POOL, RNG.randint(1, 3))
        script = '. "$MM"; g=("$@"); suite_mutate_glob() { printf "%s\\n" "${g[@]}"; }; apply_mutation "$W" "$E"'
        a = bash(script, globs, env={"MM": str(MM), "W": str(base / "a"), "E": expr})
        b = probe(["mutate", str(base / "b"), expr, *globs])
        da, db = tree_digest(base / "a"), tree_digest(base / "b")
        # The probe prints a die text where the bash returns a status; map the two shapes.
        err = text(b[2])
        # The bash lists changed paths in the order `find` visits two separately built trees,
        # which is directory order on the filesystem; the port sorts them (P3A-17), so the
        # harness sorts the bash list too.
        a = (a[0], b"".join(sorted(a[1].splitlines(keepends=True))), a[2])
        if b[0] == 1 and "the mutation changed nothing" in err:
            b = (0, b"", b"")
        elif b[0] == 1 and "mutation failed to apply" in err:
            b = (1, b"", b"")
        elif b[0] != 0:
            pass
        names_with_space = any(" " in str(p) for p in (base / "a").rglob("*"))

        def explain(x, y):
            if names_with_space:
                return "P3A-9: bash reads the changed names through awk $2 and cuts a name at its first blank"
            return None
        record("mutation", f"mut{i} {expr!r} {globs}", (a[0], a[1], b""), (b[0], b[1], b""), explain)
        record("mutation", f"mut{i}-tree", (0, da.encode(), b""), (0, db.encode(), b""), explain)
        shutil.rmtree(base, ignore_errors=True)
    # a leading dash: sed reads it as an option in bash, the type refuses it
    base = SCRATCH / "mut-dash"
    shutil.rmtree(base, ignore_errors=True)
    (base / "a").mkdir(parents=True)
    (base / "a" / "a.rs").write_text("x\n")
    a = bash('. "$MM"; suite_mutate_glob() { printf "%s\\n" "*.rs"; }; apply_mutation "$W" "$E"', env={"MM": str(MM), "W": str(base / "a"), "E": "-n"})
    b = probe(["mutate", str(base / "a"), "-n", "*.rs"])
    record("mutation", "leading-dash", (a[0], a[1], b""), (b[0], b[1], b""),
           lambda x, y: "P30-11: bash hands a leading '-' to sed as an option; the type refuses it" if y[0] != 0 else None)
    shutil.rmtree(base, ignore_errors=True)
    # a file name with a space, on its own
    base = SCRATCH / "mut-space"
    shutil.rmtree(base, ignore_errors=True)
    for side in "ab":
        (base / side).mkdir(parents=True)
        (base / side / "my file.rs").write_text("x\n")
    a = bash('. "$MM"; suite_mutate_glob() { printf "%s\\n" "*.rs"; }; apply_mutation "$W" "s/x/y/"', env={"MM": str(MM), "W": str(base / "a")})
    b = probe(["mutate", str(base / "b"), "s/x/y/", "*.rs"])
    record("mutation", "space-in-name", (a[0], a[1], b""), (b[0], b[1], b""),
           lambda x, y: "P3A-9: bash cuts the reported name at its first blank" if x[1] == b"my\n" and y[1] == b"my file.rs\n" else None)
    shutil.rmtree(base, ignore_errors=True)


# ------------------------------------------------------------------------------------------
# extraction: extract_tree
# ------------------------------------------------------------------------------------------

def git(repo, *args):
    p = subprocess.run(["git", "-C", str(repo), *args], capture_output=True,
                       env={**ENV0, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@x.invalid", "GIT_COMMITTER_NAME": "t",
                            "GIT_COMMITTER_EMAIL": "t@x.invalid"})
    assert p.returncode == 0, (args, p.stderr)
    return p.stdout.decode()


def section_extraction(count):
    for i in range(count):
        base = SCRATCH / f"ext{i}"
        shutil.rmtree(base, ignore_errors=True)
        repo = base / "repo"
        repo.mkdir(parents=True)
        git(repo, "init", "-q", "-b", "main")
        files = RNG.sample(["a.rs", "d/b.rs", "d/e/c.sh", "x y.txt", "é.md", "Makefile", ".gitignore", "d/-dash", "t\tab"], RNG.randint(0, 6))
        for f in files:
            p = repo / f
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(f"content of {f}\n")
            if RNG.random() < 0.3:
                p.chmod(0o755)
        if files and RNG.random() < 0.4:
            os.symlink(RNG.choice(["a.rs", "/etc/hostname", "../x", "d"]), repo / "lnk")
        git(repo, "add", "-A", "-f")
        if RNG.random() < 0.3:
            git(repo, "update-index", "--add", "--cacheinfo", "160000,1234567890123456789012345678901234567890,sub/mod")
        git(repo, "commit", "-q", "-m", "c", "--allow-empty")
        rev = RNG.choice(["HEAD", "HEAD", "HEAD", "main", "nonexistent-ref"])
        work = {}
        for side in "ab":
            work[side] = base / f"work-{side}"
            work[side].mkdir()
        script = '. "$MM"; VERIFY_WORK="$V"; REPO_PATH="$R"; extract_tree "$1" "$2"'
        a = bash(script, [rev, str(work["a"])], env={"MM": str(MM), "V": str(base / "vw-a"), "R": str(repo)})
        b = probe(["extract", str(repo), rev, str(work["b"])])
        la = text(a[2]).rstrip("\n").split("\n")[-1] if a[2] else ""
        lb = text(b[2]).rstrip("\n").split("\n")[-1] if b[2] else ""
        da, db = tree_digest(work["a"]), tree_digest(work["b"])
        record("extraction", f"ext{i} {rev} {files}", (a[0], b"", la.encode()), (b[0], b"", lb.encode().replace(str(work["b"]).encode(), str(work["a"]).encode())))
        record("extraction", f"ext{i}-tree", (0, da.encode(), b""), (0, db.encode(), b""))
        shutil.rmtree(base, ignore_errors=True)


# ------------------------------------------------------------------------------------------
# flags: the text of run_in_container and of the prefetch run
# ------------------------------------------------------------------------------------------

def nested_functions():
    lines = MM.read_text().split("\n")
    start = next(i for i, l in enumerate(lines) if l == "    run_in_container() {")
    end = next(i for i in range(start, len(lines)) if lines[i] == "    }")
    run_in = "\n".join(lines[start:end + 1])
    pt = next(i for i, l in enumerate(lines) if '--timeout "$PREFETCH_TIMEOUT"' in l)
    begin = pt
    while not lines[begin].lstrip().startswith('"$rt"'):
        begin -= 1
    # `split_extra_args` runs on the line before the call and fills EXTRA_ARGS.
    if "split_extra_args" in lines[begin - 1]:
        begin -= 1
    assert "split_extra_args" in "\n".join(lines[begin:begin + 2]), "the prefetch block lost its split_extra_args line"
    stop = next(i for i in range(pt, len(lines)) if 'prefetch.log" 2>&1' in lines[i])
    body = "\n".join(lines[begin:stop + 1]).rstrip(" \\")
    return run_in, "run_prefetch() {\n" + body + "\n}"


DEFAULTS = dict(re.findall(r"^(VERIFY_[A-Z]+|PREFETCH_TIMEOUT)=(\S+)", (TREE / "lib/defaults.env").read_text(), re.M))

SUITES = {
    "rust": (TREE / "profiles/sysknife/verify.d/rust.sh").read_text(),
    "shell": (TREE / "profiles/magent/verify.d/shell.sh").read_text(),
    "bare": "suite_image() { printf 'img:1'; }\nsuite_command() { printf 'make test %s\\n' \"$1\"; }\nsuite_podman_args() { printf '%s\\n' --userns=keep-id -e \"RT=$1\"; }\n",
    "spaced": "suite_image() { printf 'img:2'; }\nsuite_command() { printf 'x\\n'; }\nsuite_podman_args() { printf '%s\\n' -v \"$HOME/.cargo:/cargo:O\"; }\n",
}


def section_flags(count):
    run_in, prefetch = nested_functions()
    stub_dir = SCRATCH / "stubbin"
    shutil.rmtree(stub_dir, ignore_errors=True)
    stub_dir.mkdir(parents=True)
    stub = stub_dir / "podman"
    stub.write_text('#!/bin/bash\nprintf "%s\\0" "$@" > "$STUB_LOG"\nexit 0\n')
    stub.chmod(0o755)
    for i in range(count):
        base = SCRATCH / f"flags{i}"
        shutil.rmtree(base, ignore_errors=True)
        verify = base / "verify.d"
        verify.mkdir(parents=True)
        name = RNG.choice(["rust", "shell", "bare", "spaced"])
        (verify / f"{name}.sh").write_text(SUITES[name])
        work = base / "work"
        scratch = base / "scratch"
        work.mkdir()
        scratch.mkdir()
        env = dict(DEFAULTS)
        valid = {"VERIFY_MEMORY": ["1g", "3g", "512m"], "VERIFY_CPUS": ["1", "2", "1.5"], "VERIFY_PIDS": ["64", "1024"],
                 "VERIFY_TMPFS": ["128m", "64m"]}
        bad = {"VERIFY_MEMORY": ["0", "6x"], "VERIFY_CPUS": ["128m", "0"], "VERIFY_PIDS": ["0", "many"], "VERIFY_TMPFS": ["0", "1 g"]}
        invalid = False
        for key in RNG.sample(sorted(valid), RNG.randint(0, 3)):
            if RNG.random() < 0.15:
                env[f"MAINTAINER_{key}"] = RNG.choice(bad[key])
                invalid = True
            else:
                env[f"MAINTAINER_{key}"] = RNG.choice(valid[key])
        env["VERIFY_TIMEOUT"] = str(RNG.choice([60, 1800, 7200]))
        env["PREFETCH_TIMEOUT"] = str(RNG.choice([30, 900]))
        env["HOME"] = str(base / "home with space") if name == "spaced" else str(base / "home")
        if RNG.random() < 0.4:
            env["MAINTAINER_CARGO_JOBS"] = str(RNG.choice([2, 8]))
        filt = RNG.choice(["guard_bites", "run-backends.sh", "a b", "x_y.z"])
        kinds = ["verify"] + (["prefetch"] if name == "rust" else [])
        for kind in kinds:
            log = base / f"argv-{kind}"
            fn = "run_in_container" if kind == "verify" else "run_prefetch"
            script = (f'. "$MM"; . "$S"; rt=podman; work="$W"; scratch="$SC"; filter="$F"; \n{run_in}\n{prefetch}\n'
                      f'{fn} "$W/out.log"')
            benv = {**env, "MM": str(MM), "S": str(verify / f"{name}.sh"), "W": str(work), "SC": str(scratch), "F": filt,
                    "STUB_LOG": str(log), "PATH": f"{stub_dir}:{BASE_PATH}"}
            a = bash(script, env=benv)
            raw_a = log.read_bytes() if log.exists() else b""
            log.unlink(missing_ok=True)
            renv = {**env}
            b = probe(["flags", str(verify), name, "podman", str(work), str(scratch), filt, kind], env=renv)
            argv_b = b[1].split(b"\0")[1:-1] if b[0] == 0 else None
            argv_a = raw_a.split(b"\0")[:-1] if a[0] == 0 else None

            def explain(x, y, name=name, invalid=invalid, b=b):
                if invalid and y[0] != 0 and b"not a positive" in b[2]:
                    return "P30-3: bash hands a limit that is zero or not an amount to the runtime; the port refuses it"
                return None
            record("flags", f"flags{i} {name} {kind}", (a[0], b"\0".join(argv_a or []), b""), (b[0], b"\0".join(argv_b or []), b""), explain)
        shutil.rmtree(base, ignore_errors=True)


def main():
    SCRATCH.mkdir(parents=True, exist_ok=True)
    plan = [("glob", lambda: section_glob(int(os.environ.get("N_GLOB", "1500")))),
            ("glob-extended", lambda: section_glob_extended(int(os.environ.get("N_GLOBX", "600")))),
            ("glob-nested", lambda: section_glob_nested(int(os.environ.get("N_GLOBN", "700")))),
            ("escapes", lambda: section_escapes(int(os.environ.get("N_ESC", "70")))),
            ("selection", lambda: section_selection(int(os.environ.get("N_SEL", "90")))),
            ("ran", section_ran),
            ("mutation", lambda: section_mutation(int(os.environ.get("N_MUT", "60")))),
            ("extraction", lambda: section_extraction(int(os.environ.get("N_EXT", "40")))),
            ("flags", lambda: section_flags(int(os.environ.get("N_FLAGS", "30"))))]
    for name, fn in plan:
        fn()
    total = sum(counts.values())
    print()
    for section, (same, dec) in by_section.items():
        print(f"differential: {section:10} identical {same:5}  decision {dec:3}")
    print(f"differential: {total} comparisons: {counts['identical']} identical, {counts['decision']} explained by a recorded decision, {counts['unexplained']} unexplained")
    if total < 300:
        print("differential: fewer than 300 comparisons ran", file=sys.stderr)
        return 2
    return 1 if counts["unexplained"] else 0


if __name__ == "__main__":
    sys.exit(main())
