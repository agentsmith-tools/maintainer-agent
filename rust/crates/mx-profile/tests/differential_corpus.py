#!/usr/bin/env python3
"""Writes the inputs of tests/differential.sh: logs, failure-shape tables,
trees to scrub, and profile trees to load.

    differential_corpus.py OUTDIR SEED [PROFILES_DIR]

Everything that is not a fixed fixture comes from random.Random(SEED), so a run
that finds a difference can be repeated with the seed the harness printed.

Layout under OUTDIR:

    classify/cases.tsv     id TAB label TAB table TAB flags
    classify/<id>.arg      the log argument, byte for byte (no newline added)
    classify/<id>.log      the log, when the case has one
    classify/tables/<name> failure-shapes tables
    scrub/cases.tsv        id TAB label
    scrub/<id>/{a,b}       two identical trees; <id>/secret, <id>/targets
    load/cases.tsv         id TAB label TAB lib TAB flags
    load/<id>/             home/, env (NAME=VALUE lines), a file `lib`

`table` is `-` (the shipped table beside the library), `none` (no table
anywhere), `missing` (an override naming a file that does not exist),
`empty-env` (an override that is set and empty), or a file under tables/.
`flags` hold `p` (compare the two predicates too) and `cwd` (run in the
classify directory with the relative argument).
"""
import json
import os
import random
import stat
import sys

OUT = sys.argv[1]
SEED = int(sys.argv[2])
PROFILES_DIR = sys.argv[3] if len(sys.argv) > 3 else None
rng = random.Random(SEED)

SECRET = "ghp_FIXTURE0123456789abcdefghij"
NEAR = "ghp_FIXTURE0123456789abcdefghiK"


def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if isinstance(data, str):
        data = data.encode()
    with open(path, "wb") as f:
        f.write(data)


# ---------------------------------------------------------------------------
# classify
# ---------------------------------------------------------------------------

CLS = os.path.join(OUT, "classify")
cases = []  # (label, arg bytes, table, flags)


def add_log(label, log_bytes, table="-", flags="p"):
    idx = len(cases)
    path = os.path.join(CLS, "%04d.log" % idx)
    write(path, log_bytes)
    cases.append((label, path.encode(), table, flags))


def add_arg(label, arg, table="-", flags="", log_bytes=None, name=None):
    idx = len(cases)
    if log_bytes is not None:
        write(os.path.join(CLS, "%04d.log" % idx), log_bytes)
    cases.append((label, arg, table, flags))


def add_table(name, data):
    write(os.path.join(CLS, "tables", name), data)
    return name


# One sample line per shipped shape, the same as tests/characterization/run-classify.sh.
SAMPLES = [
    ("session-limit", "You've hit your session limit, resets 10:10pm"),
    ("usage-limit-reached", "Usage limit reached. Try again later"),
    ("upstream-overloaded", 'API Error: {"type":"overloaded_error"}'),
    ("rate-limited", "API Error: 429 Too Many Requests"),
    ("upstream-5xx", "API Error: 503 Service Unavailable"),
    ("connection-error", "read ECONNRESET"),
    ("api-unreachable", "Can't reach the API server. Check your network"),
    ("dns-failure", "getaddrinfo EAI_AGAIN api.example.test"),
    ("gh-unreachable", "error connecting to api.github.com"),
    ("oauth-expired", "OAuth session expired. Please log in again"),
    ("invalid-key", "Invalid API key. Fix external API key"),
    ("out-of-credit", "Your credit balance is too low to access the API"),
    ("login-required", "Please run /login"),
    ("backend-unconfigured", "backend claude unusable: missing /x/settings.json"),
    ("wrong-gh-account", "gh is authenticated as 'someone', not fxacct; refusing"),
    ("token-unpinnable", "could not pin a token for fxacct, so"),
    ("checkout-refresh-failed", "refresh failed, no agent run started. See x"),
    ("lock-held", "another run held the lock for over 3600s, so"),
    ("killed-silently", "Killed"),
    ("no-doctrine", "preamble-core.md is missing; refusing"),
    # The table calls this shape "never matched by pattern". On GNU grep 3.12 the
    # pattern (?!x)x matches the text !xx, so this line triggers it.
    ("died-without-saying", "boom!xx"),
]
for sid, text in SAMPLES:
    add_log("shape:" + sid, (text + "\n").encode())

add_log("shape:died-without-saying own id and pattern text", b"died-without-saying (?!x)x\n")
add_log("table order beats log order", b'OAuth session expired\nAPI Error: {"type":"overloaded_error"}\n')
add_log("transient listed before needs_human wins", b"Invalid API key. Fix external API key\nconnection error while reading\n")
add_log("last match of one shape", b"You've hit your session limit, resets 8am\nother line\nYou've hit your session limit, resets 10pm\n")
add_log("ignore case", b"USAGE LIMIT REACHED NOW\n")
add_log("bounded tail 80 pad", b"usage limit reached " + b"x" * 80 + b"\n")
for pad in (58, 59, 60, 61, 62):
    add_log("bounded tail pad %d" % pad, b"usage limit reached " + b"x" * pad + b"\n")
add_log("bounded tail multibyte", ("usage limit reached " + "é" * 80 + "\n").encode())
add_log("binary log", b"binary\0prefix\nKilled\n")
add_log("nul inside a match", b"usage limit reached\0 tail here\n")
add_log("no trailing newline", b"all fine\nKilled")
add_log("crlf lines", b"all fine\r\nKilled\r\nthe end\r\n")
add_log("gigantic matching line", b"z" * 2_000_000 + b" Killed " + b"z" * 2_000_000)
add_log("gigantic unmatched line with an error word", b"z" * 500_000 + b" an error " + b"y" * 500_000)
add_log("gigantic unmatched line, no error word", b"z" * 1_000_000)
add_log("unclassified error line", b"all fine\nsomething failed here\nthe last line\n")
add_log("unclassified last line", b"all fine\nthe last line\n")
add_log("unclassified empty log", b"")
add_log("unclassified blank last line", b"all fine\nfine too\n\n")
add_log("unclassified whitespace last line", b"all fine\n   \n")
add_log("unclassified one newline", b"\n")
add_log("unclassified uppercase ERROR", b"ok\nERROR in the thing\nlater\n")
add_log("unclassified tab in line", b"ok\nan\terror\there\n")
add_log("match text holds a tab", b"usage limit reached\there and there\n")
add_log("shape text spans a nul in the table text", b"Usage limit\0 reached\n")
add_log("died id text without the pattern", b"died-without-saying\n")
add_log("three shapes in one log", b"Killed\nAPI Error: 503\nPlease run /login\n")

add_arg("missing log", (os.path.join(OUT, "classify", "nope.log")).encode(), flags="p")
add_arg("empty log argument", b"", flags="p")
add_arg("log is a directory", CLS.encode(), flags="p")
spaced = os.path.join(CLS, "dir with spaces", "x y.log")
write(spaced, b"Killed\n")
add_arg("log path with spaces", spaced.encode(), flags="p")
newline_name = os.path.join(CLS, "name\nwith newline.log")
write(newline_name, b"Killed\n")
add_arg("log path with a newline", newline_name.encode(), flags="p")
write(os.path.join(CLS, "-v"), b"an error here\nsecond line\n")
add_arg("relative log named -v", b"-v", flags="cwd")
write(os.path.join(CLS, "-n"), b"Killed\n")
add_arg("relative log named -n, a shape matches", b"-n", flags="cwd")

# tables
T = add_table
LOG_BOOM = b"it went boom 42 today\nKilled\n"


def table_case(label, name, data, log=LOG_BOOM, flags="p"):
    add_log("table: " + label, log, add_table(name, data), flags)


table_case("override replaces the shipped table", "override.json",
           '{"shapes": [{"id": "my-shape", "class": "needs_human", "pattern": "boom [0-9]+"}]}')
table_case("invalid ERE then a valid shape", "invalid-ere.json",
           '{"shapes": [{"id": "broken", "class": "needs_human", "pattern": "("}, {"id": "next", "class": "transient", "pattern": "boom [0-9]+"}]}')
table_case("pattern starting with a dash", "dash.json",
           '{"shapes": [{"id": "dash", "class": "transient", "pattern": "-fail+"}]}', b"status -failll now\n")
table_case("alternation longest match", "alt.json",
           '{"shapes": [{"id": "alt", "class": "transient", "pattern": "a|ab|abc"}]}', b"xx abcd\n")
table_case("alternation longest match 2", "alt2.json",
           '{"shapes": [{"id": "alt", "class": "transient", "pattern": "(bo|boo)+m?"}]}', b"a booom b\n")
table_case("weird class", "weird-class.json",
           '{"shapes": [{"id": "odd", "class": "weird-class", "pattern": "boom"}]}')
table_case("class that starts with transient", "transientish.json",
           '{"shapes": [{"id": "odd", "class": "transient_dns", "pattern": "boom"}]}')
table_case("shapes missing", "no-shapes-key.json", '{"version": 1}')
table_case("shapes empty list", "empty-list.json", '{"shapes": []}')
table_case("shapes empty dict", "empty-dict.json", '{"shapes": {}}')
table_case("shapes empty string", "empty-string.json", '{"shapes": ""}')
for nm, body in [("null", "null"), ("number", "7"), ("true", "true"), ("string", '"abc"'),
                 ("dict", '{"id": "a", "class": "b", "pattern": "c"}')]:
    table_case("shapes is " + nm, "shapes-%s.json" % nm, '{"shapes": %s}' % body)
for nm, entry in [("no id", '{"class": "t", "pattern": "boom"}'), ("no class", '{"id": "a", "pattern": "boom"}'),
                  ("no pattern", '{"id": "a", "class": "t"}'), ("empty object", "{}"),
                  ("id is a number", '{"id": 1, "class": "t", "pattern": "boom"}'),
                  ("class is null", '{"id": "a", "class": null, "pattern": "boom"}'),
                  ("pattern is a list", '{"id": "a", "class": "t", "pattern": ["boom"]}'),
                  ("entry is a list", '["a", "t", "boom"]'), ("entry is a string", '"abc"'),
                  ("entry is null", "null")]:
    table_case("entry: " + nm, "entry-%s.json" % nm.replace(" ", "-"), '{"shapes": [%s]}' % entry)
table_case("one bad entry after good ones", "bad-after-good.json",
           '{"shapes": [{"id": "a", "class": "t", "pattern": "boom"}, {"id": "b"}]}')
table_case("bad entry after a match would have been found", "bad-late.json",
           '{"shapes": [{"id": "a", "class": "t", "pattern": "Killed"}, {"id": 5, "class": "t", "pattern": "x"}]}')
table_case("not json", "not-json.json", "this is not json\n")
table_case("empty file", "empty-file.json", "")
table_case("whitespace file", "ws-file.json", "   \n")
table_case("top level list", "top-list.json", '[{"id": "a", "class": "t", "pattern": "boom"}]')
table_case("top level string", "top-string.json", '"shapes"')
table_case("top level null", "top-null.json", "null")
table_case("trailing comma", "trailing-comma.json", '{"shapes": [],}')
table_case("truncated", "truncated.json", '{"shapes": [')
table_case("bom", "bom.json", b'\xef\xbb\xbf{"shapes": [{"id": "a", "class": "t", "pattern": "boom"}]}')
table_case("invalid utf-8 in an unused field", "bad-utf8.json",
           b'{"x": "\xff", "shapes": [{"id": "a", "class": "t", "pattern": "boom"}]}')
table_case("duplicate keys, last wins", "dup.json",
           '{"shapes": [], "shapes": [{"id": "a", "class": "t", "pattern": "boom"}]}')
table_case("duplicate id key, last wins", "dup-id.json",
           '{"shapes": [{"id": "a", "id": "b", "class": "t", "pattern": "boom"}]}')
table_case("extra keys ignored", "extra.json",
           '{"v": 1, "classes": {"x": [1, 2]}, "shapes": [{"id": "a", "class": "t", "pattern": "boom", "means": "m", "fix": null}]}')
table_case("big number in an unused field", "bignum.json",
           '{"v": 1e999, "w": 123456789012345678901234567890, "shapes": [{"id": "a", "class": "t", "pattern": "boom"}]}')
table_case("tab in the id shifts the fields", "tab-id.json",
           json.dumps({"shapes": [{"id": "x\ty", "class": "needs_human", "pattern": "boom"}]}))
table_case("tab in the id, pattern holds a tab", "tab-id2.json",
           json.dumps({"shapes": [{"id": "x", "class": "c\td", "pattern": "boom"}]}), b"boom\tx\n")
table_case("tab in pattern matches a tab in the log", "tab-pat.json",
           json.dumps({"shapes": [{"id": "x", "class": "c", "pattern": "a\tb"}]}), b"xx a\tb yy\n")
table_case("empty class", "empty-class.json",
           json.dumps({"shapes": [{"id": "a", "class": "", "pattern": "boom"}]}))
table_case("empty id", "empty-id.json",
           json.dumps({"shapes": [{"id": "", "class": "t", "pattern": "boom"}]}))
table_case("empty pattern", "empty-pat.json",
           json.dumps({"shapes": [{"id": "a", "class": "t", "pattern": ""}]}))
table_case("all fields empty", "all-empty.json",
           json.dumps({"shapes": [{"id": "", "class": "", "pattern": ""}]}))
table_case("newline in a pattern", "nl-pat.json",
           json.dumps({"shapes": [{"id": "a", "class": "t", "pattern": "nomatch\nboom"}]}))
table_case("blank line in a pattern", "nl2-pat.json",
           json.dumps({"shapes": [{"id": "a", "class": "t", "pattern": "nomatch\n\nboom"}]}))
table_case("newline in an id", "nl-id.json",
           json.dumps({"shapes": [{"id": "a\nboom", "class": "t", "pattern": "zzz"}]}))
table_case("nul in a pattern", "nul-pat.json",
           json.dumps({"shapes": [{"id": "a", "class": "t", "pattern": "bo\u0000om"}]}))
table_case("nul in the id", "nul-id.json",
           json.dumps({"shapes": [{"id": "a\u0000b", "class": "t", "pattern": "boom"}]}))
table_case("leading and trailing spaces in a pattern", "spaces.json",
           json.dumps({"shapes": [{"id": "a", "class": "t", "pattern": "  boom  "}]}), b"x  boom  y\n")
table_case("trailing tabs in a pattern", "trail-tab.json",
           json.dumps({"shapes": [{"id": "a", "class": "t", "pattern": "boom\t\t"}]}), b"boom\t\t\n")
table_case("leading tabs in a pattern", "lead-tab.json",
           json.dumps({"shapes": [{"id": "a", "class": "t", "pattern": "\t\tboom"}]}))
table_case("backslash patterns", "backslash.json",
           json.dumps({"shapes": [{"id": "a", "class": "t", "pattern": "b\\.om|go\\+n"}]}), b"b.om and go+n\n")
table_case("unicode pattern, c locale", "unicode.json",
           json.dumps({"shapes": [{"id": "a", "class": "t", "pattern": "café.{0,3}"}]}), "un café éé ok\n".encode())
table_case("pattern that matches everything", "dotstar.json",
           json.dumps({"shapes": [{"id": "a", "class": "t", "pattern": ".*"}]}))
table_case("pattern anchors", "anchors.json",
           json.dumps({"shapes": [{"id": "a", "class": "t", "pattern": "^Killed$"}]}))
table_case("two shapes, second matches", "two.json",
           json.dumps({"shapes": [{"id": "a", "class": "t", "pattern": "nomatch"}, {"id": "b", "class": "u", "pattern": "Killed"}]}))
table_case("three hundred shapes", "many.json",
           json.dumps({"shapes": [{"id": "s%d" % i, "class": "t", "pattern": "nomatch%d" % i} for i in range(300)] +
                       [{"id": "last", "class": "u", "pattern": "Killed"}]}), flags="")

add_log("table: none anywhere", b"Killed\n", "none")
add_log("table: override naming a missing file", b"Killed\n", "missing")
add_log("table: empty override is unset", b"Killed\n", "empty-env")
add_arg("table: none anywhere and a missing log", (os.path.join(CLS, "nope.log")).encode(), "none", "p")

# multibyte text, which a UTF-8 locale counts in characters and C in bytes
E = "\u00e9"
add_log("utf8: multibyte after the pattern, 60 whole characters", ("usage limit reached " + E * 70 + "\n").encode())
add_log("utf8: multibyte text before the match", ("caf\u00e9 " + E * 5 + " usage limit reached ok\n").encode())
add_log("utf8: multibyte text inside the match window", ("usage limit reached " + "ab" + E + "cd" + E * 3 + "\n").encode())
add_log("utf8: invalid bytes inside the window", b"usage limit reached ab\xff\xfecd and more\n")
add_log("utf8: invalid bytes before the match", b"\xff\xfe junk\nusage limit reached fine\n")
add_log("utf8: a lone continuation byte after the pattern", b"usage limit reached \x80 tail\n")
add_log("utf8: four-byte characters near the cut", ("usage limit reached " + "\U0001F600" * 70 + "\n").encode())
table_case("utf8 fold of a non-ASCII upper case letter", "fold.json",
           json.dumps({"shapes": [{"id": "a", "class": "t", "pattern": "caf\u00e9"}]}), "un CAF\u00c9 noir\n".encode())
table_case("utf8 character class and bound", "charclass.json",
           json.dumps({"shapes": [{"id": "a", "class": "t", "pattern": "[[:alpha:]]{3,5}\u00e9"}]}), "xx \u00e9\u00e9\u00e9\u00e9z\u00e9 yy\n".encode())
add_log("utf8: error word fallback with multibyte text", ("\u00e9\u00e9 an error \u00e9\u00e9\nlast\n").encode())
FIXED = len(cases)
write(os.path.join(CLS, "fixed_count"), str(FIXED))

# ---- generated logs --------------------------------------------------------

NOISE = ["starting run", "backend=claude model=opus", "- PR #12 fix the thing", "=== result ===", "ok", "done",
         "tool call: gh api repos/x/y", "thinking about it", "wrote the report", "skill review loaded", "",
         "  indented line", "\tTabbed line", "a very long line " + "w" * 300]
ERRORISH = ["something failed here", "cannot open the file", "request denied", "operation refused", "ERROR: boom",
            "could not resolve host", "panic: oh no", "Error: spawn ENOENT", "it did not fail at all"]
UNICODE = ["café ééé", "\U0001F600 emoji", "日本語 text", "ÿþ latin"]


def case_flip(text):
    out = []
    for ch in text:
        r = rng.random()
        out.append(ch.upper() if r < 0.15 else ch.lower() if r < 0.30 else ch)
    return "".join(out)


def gen_log():
    lines = []
    for _ in range(rng.randint(0, 14)):
        r = rng.random()
        if r < 0.30:
            lines.append(rng.choice(NOISE).encode())
        elif r < 0.45:
            lines.append(rng.choice(ERRORISH).encode())
        elif r < 0.75:
            sid, text = rng.choice(SAMPLES)
            if rng.random() < 0.3:
                text = case_flip(text)
            text = text + " " * rng.randint(0, 1) + "x" * rng.choice([0, 0, 5, 20, 58, 59, 60, 61, 100])
            if rng.random() < 0.25:
                text = rng.choice(NOISE) + " " + text
            lines.append(text.encode())
        elif r < 0.82:
            lines.append(rng.choice(UNICODE).encode())
        elif r < 0.88:
            lines.append(b"bin\0ary " + rng.choice(NOISE).encode() + b"\0")
        elif r < 0.93:
            lines.append(b"long " + b"q" * rng.choice([1000, 5000, 70000]) + b" " + rng.choice(ERRORISH).encode())
        else:
            lines.append(b"crlf line\r")
    body = b"\n".join(lines)
    if rng.random() < 0.8:
        body += b"\n"
    if rng.random() < 0.1:
        body += b"\n" * rng.randint(1, 3)
    return body


for i in range(200):
    add_log("generated log %03d" % i, gen_log(), "-", "p" if i % 5 == 0 else "")

# ---- generated tables --------------------------------------------------------

PATS = ["boom", "bang", "a|ab", "alpha[a-z]*", "fail.{0,5}", "^beta", "delta$", "gamma|gam", "(", "[", "*x",
        "\\bfoo", "x{2,}", "-dash", "a.c", "ALPHA", "(?!x)x", "$^", "limit [0-9]+", "ab|abc|abcd", "(bo|boo)+m?",
        "b.m", "[[:digit:]]+ items", "\\(paren\\)", "x\ty", "  pad  ", "", "boom\nbang", "tab\t", "\tlead"]
CLASSES = ["transient", "needs_human", "transient", "needs_human", "weird", "", "a b", "x\ty", "transient_x", "needs_human2"]
IDS = ["id1", "id2", "id3", "", "x y", "t\tab", "nl\nid", "dup", "dup", "last"]
LOGWORDS = ["alpha beta", "boom", "bang", "fail now", "gamma", "delta", "limit 42", "abc", "x\ty", "tab\t", "a b",
            "boom 7", "3 items", "(paren)", "-dash", "!xx", "bom", "BOOM", "ALPHA"]


def gen_table(i):
    shapes = []
    for _ in range(rng.randint(1, 6)):
        entry = {"id": rng.choice(IDS), "class": rng.choice(CLASSES), "pattern": rng.choice(PATS)}
        r = rng.random()
        if r < 0.04:
            del entry[rng.choice(["id", "class", "pattern"])]
        elif r < 0.07:
            entry[rng.choice(["id", "class", "pattern"])] = rng.choice([1, None, ["x"], True])
        shapes.append(entry)
    doc = {"shapes": shapes}
    if rng.random() < 0.1:
        doc["extra"] = {"k": [1, 2, 3]}
    return json.dumps(doc)


for i in range(80):
    name = "gen-%03d.json" % i
    log = b"\n".join(rng.choice(LOGWORDS).encode() for _ in range(rng.randint(1, 8))) + b"\n"
    add_log("generated table %03d" % i, log, add_table(name, gen_table(i)), "p" if i % 4 == 0 else "")

with open(os.path.join(CLS, "cases.tsv"), "w") as f:
    for idx, (label, arg, table, flags) in enumerate(cases):
        write(os.path.join(CLS, "%04d.arg" % idx), arg)
        f.write("%04d\t%s\t%s\t%s\n" % (idx, label, table, flags or "-"))
print("classify cases:", len(cases))

# ---------------------------------------------------------------------------
# scrub
# ---------------------------------------------------------------------------

SCR = os.path.join(OUT, "scrub")
scrub_cases = []


def mk(root, rel, content=None, mode=None, link=None, fifo=False):
    path = os.path.join(root, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if link is not None:
        os.symlink(link, path)
        return
    if fifo:
        os.mkfifo(path)
        return
    write(path, content if content is not None else b"")
    if mode is not None:
        os.chmod(path, mode)


def leak(text=None):
    return ("before %s after\n" % SECRET).encode() if text is None else text


def scrub_case(label, builder, targets=("tree",), secret=SECRET):
    idx = len(scrub_cases)
    base = os.path.join(SCR, "%04d" % idx)
    for side in ("a", "b"):
        root = os.path.join(base, side)
        os.makedirs(root)
        builder(root)
    write(os.path.join(base, "secret"), secret)
    write(os.path.join(base, "targets"), "\n".join(targets))
    scrub_cases.append(label)


def b_text(root):
    mk(root, "tree/leak.txt", leak())


scrub_case("one text file in a directory", b_text)
scrub_case("the file itself as the target", b_text, targets=("tree/leak.txt",))
scrub_case("a target that does not exist", b_text, targets=("nope", "tree"))
scrub_case("an empty target name", b_text, targets=("", "tree"))
scrub_case("no target at all", b_text, targets=())
scrub_case("secret of 15 characters", b_text, secret="x" * 15)
scrub_case("secret of 16 characters that is absent", b_text, secret="y" * 16)
scrub_case("empty secret", b_text, secret="")
scrub_case("secret of 16 characters that is present", lambda r: mk(r, "tree/f.txt", b"a 0123456789abcdef b\n"), secret="0123456789abcdef")
scrub_case("secret of 8 four-byte characters (32 bytes)", lambda r: mk(r, "tree/f.txt", ("x " + "\U0001F600" * 8).encode()), secret="\U0001F600" * 8)
scrub_case("secret of 16 two-byte characters", lambda r: mk(r, "tree/f.txt", ("x " + "é" * 16 + " y").encode()), secret="é" * 16)
scrub_case("secret with regex characters", lambda r: mk(r, "tree/f.txt", b"a [.*+?(]{}x\\1234567 b\n"), secret="[.*+?(]{}x\\1234567")


def b_many(root):
    mk(root, "tree/f.txt", ("%s\n%s%s x %s" % (SECRET, SECRET, SECRET, SECRET)).encode())


scrub_case("every occurrence in a file", b_many, )
scrub_case("overlapping occurrences", lambda r: mk(r, "tree/f.txt", b"a" * 40), secret="a" * 16)
scrub_case("overlapping occurrences, exact multiple", lambda r: mk(r, "tree/f.txt", b"ab" * 32), secret="ab" * 8)
scrub_case("secret at the start and the end of a file",
           lambda r: mk(r, "tree/f.txt", (SECRET + " middle " + SECRET).encode()))
scrub_case("file that is exactly the secret", lambda r: mk(r, "tree/f.txt", SECRET.encode()))
scrub_case("empty file", lambda r: mk(r, "tree/f.txt", b""))
scrub_case("near miss of the secret", lambda r: mk(r, "tree/f.txt", ("x %s y\n" % NEAR).encode()))
scrub_case("binary file with the secret", lambda r: mk(r, "tree/d.img", b"bin\0" + SECRET.encode() + b"\0"))
scrub_case("nul after the secret", lambda r: mk(r, "tree/d.img", SECRET.encode() + b" then\0"))
scrub_case("binary file without the secret", lambda r: mk(r, "tree/d.img", b"abc\0def"))
scrub_case("a binary file twice by two names", lambda r: (mk(r, "tree/d.img", b"x\0" + SECRET.encode()), mk(r, "tree/e.img", b"", link="d.img")))


def b_skips(root):
    for d in (".git", "node_modules", "target", "ctargets"):
        mk(root, "tree/%s/inside.txt" % d, leak())
        mk(root, "tree/deep/%s/inside.txt" % d, leak())
        mk(root, "tree/deep/%s/more/inside.txt" % d, leak())
    mk(root, "tree/other/inside.txt", leak())
    mk(root, "tree/top.txt", leak())
    mk(root, "tree/targets/inside.txt", leak())
    mk(root, "tree/.github/inside.txt", leak())
    mk(root, "tree/Target/inside.txt", leak())


scrub_case("the four skipped directory names, at two depths", b_skips)
scrub_case("a root named target", lambda r: (mk(r, "target/inner.txt", leak()), mk(r, "target/target/nested.txt", leak()),
                                              mk(r, "target/sub/.git", leak())), targets=("target",))
scrub_case("a root named .git", lambda r: (mk(r, ".git/inner.txt", leak()), mk(r, ".git/.git/nested.txt", leak())), targets=(".git",))
scrub_case("files named like skipped directories", lambda r: [mk(r, "tree/%s" % n, leak()) for n in (".git", "node_modules", "target", "ctargets")])
scrub_case("files before subdirectories", lambda r: (mk(r, "tree/a/deep.txt", leak()), mk(r, "tree/z.txt", leak()), mk(r, "tree/m/n/o.txt", leak())))
scrub_case("several targets in order", lambda r: (mk(r, "one/f.txt", leak()), mk(r, "two/f.txt", leak())), targets=("two", "one", "two/f.txt"))
scrub_case("the same file named twice", b_text, targets=("tree/leak.txt", "tree", "tree/leak.txt"))
scrub_case("trailing slash on the root", b_text, targets=("tree/",))
scrub_case("dot as the root", b_text, targets=(".",))
scrub_case("dot dot in the root", b_text, targets=("tree/../tree",))
scrub_case("double slashes in the root", b_text, targets=("tree//",))


def b_links(root):
    mk(root, "outside/dir/f.txt", leak())
    mk(root, "outside/file.txt", leak())
    mk(root, "tree/dirlink", link="../outside/dir")
    mk(root, "tree/filelink", link="../outside/file.txt")
    mk(root, "tree/broken", link="../outside/nowhere")
    mk(root, "tree/selfdir/f.txt", leak())
    mk(root, "tree/loop", link=".")


scrub_case("symlinks to a directory, a file, nowhere and the root", b_links)
scrub_case("a symlink to a directory as the target", lambda r: (mk(r, "real/f.txt", leak()), mk(r, "dirlink", link="real")), targets=("dirlink",))
scrub_case("a symlink to a file as the target", lambda r: (mk(r, "real.txt", leak()), mk(r, "filelink", link="real.txt")), targets=("filelink",))
scrub_case("a broken symlink as the target", lambda r: mk(r, "broken", link="nowhere"), targets=("broken",))
scrub_case("modes are kept", lambda r: (mk(r, "tree/a.txt", leak(), mode=0o640), mk(r, "tree/b.txt", leak(), mode=0o755), mk(r, "tree/c.txt", leak(), mode=0o600)))
scrub_case("a read-only file is skipped", lambda r: (mk(r, "tree/ro.txt", leak(), mode=0o444), mk(r, "tree/rw.txt", leak())))
scrub_case("an unreadable file is skipped", lambda r: (mk(r, "tree/none.txt", leak(), mode=0o000), mk(r, "tree/rw.txt", leak())))
scrub_case("an unreadable directory is skipped", lambda r: (mk(r, "tree/sealed/f.txt", leak()), mk(r, "tree/rw.txt", leak()), os.chmod(os.path.join(r, "tree/sealed"), 0o000)))
scrub_case("a large file", lambda r: mk(r, "tree/big.txt", b"x" * 3_000_000 + SECRET.encode() + b"y" * 3_000_000))
scrub_case("many small files", lambda r: [mk(r, "tree/d%d/f%d.txt" % (i % 7, i), leak(("n%d %s\n" % (i, SECRET)).encode() if i % 3 else b"clean\n")) for i in range(90)])
scrub_case("non-utf-8 file name", lambda r: mk(r, "tree/" + "leak-\udcff\udcfe.txt", leak()))
scrub_case("file names with spaces, newlines and dashes", lambda r: [mk(r, "tree/%s" % n, leak()) for n in ("a b.txt", "line\nbreak.txt", "-dash.txt", "--", "tab\there")])
scrub_case("a deep tree", lambda r: mk(r, "tree/" + "/".join("d%d" % i for i in range(40)) + "/f.txt", leak()))
scrub_case("secret split across a line break", lambda r: mk(r, "tree/f.txt", (SECRET[:10] + "\n" + SECRET[10:]).encode()))
scrub_case("secret next to the mask", lambda r: mk(r, "tree/f.txt", (SECRET + "REDACTED_BY_MAINTAINER_ROTATE_THIS_SECRET" + SECRET).encode()))
scrub_case("a hard link", lambda r: (mk(r, "tree/f.txt", leak()), os.link(os.path.join(r, "tree/f.txt"), os.path.join(r, "tree/g.txt"))))

# ---- generated trees ---------------------------------------------------------

DIRS = [".git", "node_modules", "target", "ctargets", "src", "a b", "docs", "Target", "x.y"]
FILES = ["a.txt", "b.log", "c.json", ".git", "target", "notes", "d e.txt", "f.bin", "g"]


def random_content():
    parts = []
    for _ in range(rng.randint(0, 4)):
        r = rng.random()
        if r < 0.4:
            parts.append(SECRET.encode())
        elif r < 0.5:
            parts.append(NEAR.encode())
        elif r < 0.9:
            parts.append(("text %d\n" % rng.randint(0, 99)).encode())
        else:
            parts.append(b"\0")
    return b" ".join(parts)


def random_tree(root):
    made = []

    def fill(rel, depth):
        for name in rng.sample(FILES, rng.randint(0, 4)):
            rel_path = rel + "/" + name
            r = rng.random()
            if r < 0.08 and made:
                mk(root, rel_path, link=os.path.relpath(os.path.join(root, rng.choice(made)), os.path.dirname(os.path.join(root, rel_path))))
            elif r < 0.12:
                mk(root, rel_path, link="nowhere")
            else:
                mk(root, rel_path, random_content(), mode=rng.choice([None, None, None, 0o600, 0o644, 0o755]))
                made.append(rel_path)
        if depth < 3:
            for name in rng.sample(DIRS, rng.randint(0, 3)):
                if os.path.lexists(os.path.join(root, rel, name)):
                    continue
                os.makedirs(os.path.join(root, rel, name))
                fill(rel + "/" + name, depth + 1)

    os.makedirs(os.path.join(root, "tree"), exist_ok=True)
    fill("tree", 0)


for i in range(40):
    state = rng.getstate()

    def builder(root, state=state):
        local = random.Random()
        local.setstate(state)
        global rng
        keep = rng
        rng = local
        try:
            random_tree(root)
        finally:
            rng = keep

    scrub_case("generated tree %02d" % i, builder)
    rng.random()  # advance the shared generator so each tree differs

with open(os.path.join(SCR, "cases.tsv"), "w") as f:
    for idx, label in enumerate(scrub_cases):
        f.write("%04d\t%s\n" % (idx, label))
print("scrub cases:", len(scrub_cases))

# ---------------------------------------------------------------------------
# load
# ---------------------------------------------------------------------------

LOAD = os.path.join(OUT, "load")
load_cases = []

DEFAULTS_FIXTURE = "# tunables\nLOCK_WAIT=3600\nRETENTION_DAYS=90\nGH_TRIES=3\nVERIFY_MEMORY=2g\nEMPTY_DEFAULT=\n  INDENTED=2\nlower=3\n"
PROFILE_FX = 'REPO_SLUG="o/r"\nREPO_PATH="/work/r"\nSTATE_DIR="/state"\nGH_ACCOUNT="fxacct"\nPOST="off"\nLOCK_WAIT=10\n'


def load_case(label, profiles=None, env=None, lib="std", flags="-", extra=None, home_files=None):
    idx = len(load_cases)
    base = os.path.join(LOAD, "%04d" % idx)
    home = os.path.join(base, "home")
    os.makedirs(home)
    for name, body in (profiles or {}).items():
        if body is None:
            os.makedirs(os.path.join(home, ".local/share/maintainer/profiles", name, "profile.env"))
        else:
            write(os.path.join(home, ".local/share/maintainer/profiles", name, "profile.env"), body)
    for rel, body in (extra or {}).items():
        write(os.path.join(base, rel), body)
    for rel, body in (home_files or {}).items():
        write(os.path.join(home, rel), body)
    env = [(k, v.replace("@BASE@", base)) for k, v in (env or [])]
    write(os.path.join(base, "env"), "".join("%s=%s\n" % (k, v) for k, v in env))
    load_cases.append((label, lib, flags))
    return base


RUN4 = [("MAINTAINER_SLUG", "run/slug"), ("MAINTAINER_REPO", "/run/repo"), ("MAINTAINER_ACCOUNT", "runacct"), ("MAINTAINER_STATE", "/run/state")]

load_case("one deployed profile", {"fx": PROFILE_FX})
load_case("one deployed profile, a profile that declares nothing", {"fx": "# nothing\n"})
load_case("named profile among several", {"alpha": PROFILE_FX, "beta": PROFILE_FX.replace("o/r", "b/b")}, [("MAINTAINER_PROFILE", "beta")])
load_case("several profiles, no name", {"beta": PROFILE_FX, "alpha": PROFILE_FX, "gamma": PROFILE_FX})
load_case("no profile deployed", {})
load_case("named profile not deployed", {"fx": PROFILE_FX}, [("MAINTAINER_PROFILE", "nope")])
load_case("profile name with dot dot", {"fx": PROFILE_FX}, [("MAINTAINER_PROFILE", "../../../../../../../../../tmp/mx-no-such")])
load_case("glob order of profile names", {n: PROFILE_FX for n in ("a", "a-b", "B", "b", "a b", "a.b", "_x", "1")})
load_case("dot directory and files are not profiles", {"real": PROFILE_FX, ".hidden": PROFILE_FX}, home_files={".local/share/maintainer/profiles/a-file": "x"})
load_case("profile.env that is a directory", {"odd": None})
load_case("override beats profile beats default", {"fx": PROFILE_FX}, [("MAINTAINER_LOCK_WAIT", "7"), ("MAINTAINER_RETENTION_DAYS", "30"), ("MAINTAINER_GH_TRIES", "")])
load_case("override for a key that is not declared", {"fx": PROFILE_FX + 'EXTRA="kept"\n'}, [("MAINTAINER_EXTRA", "overridden"), ("MAINTAINER_UNDECLARED", "x")])
load_case("override of an indented, a lower-case and an empty key", {"fx": PROFILE_FX}, [("MAINTAINER_INDENTED", "o2"), ("MAINTAINER_lower", "o3"), ("MAINTAINER_EMPTY_DEFAULT", "o4")])
load_case("three of the four early-return names", {"fx": PROFILE_FX}, [RUN4[0], RUN4[1], RUN4[2]])
load_case("the five names set beforehand", {"fx": PROFILE_FX}, [("MAINTAINER_SLUG", "kept/slug"), ("MAINTAINER_REPO", "/kept"), ("MAINTAINER_ACCOUNT", "keptacct")])
load_case("a named profile that sets MAINTAINER_NAME", {"fx": PROFILE_FX + 'MAINTAINER_NAME="Vladimir"\nPROD_GLOBS="bin/* lib/*"\n'})
load_case("early return with a profile in PROFILE_DIR", None, RUN4 + [("PROFILE_DIR", "@BASE@/elsewhere/fx")], extra={"elsewhere/fx/profile.env": 'LOCK_WAIT=11\nREPO_SLUG="from/profile"\n'})
load_case("early return, overrides apply", None, RUN4 + [("PROFILE_DIR", "@BASE@/elsewhere/fx"), ("MAINTAINER_LOCK_WAIT", "99")], extra={"elsewhere/fx/profile.env": "LOCK_WAIT=11\n"})
load_case("early return, PROFILE_DIR has no profile.env", None, RUN4 + [("PROFILE_DIR", "/nonexistent")])
load_case("early return, PROFILE_DIR unset", None, RUN4)
load_case("early return, PROFILE_DIR empty", None, RUN4 + [("PROFILE_DIR", "")])
load_case("early return, an empty value counts as unset", {"fx": PROFILE_FX}, [RUN4[0], RUN4[1], RUN4[2], ("MAINTAINER_STATE", "")])
load_case("early return does not export the five", None, RUN4 + [("PROFILE_DIR", "@BASE@/elsewhere/fx")], extra={"elsewhere/fx/profile.env": "X=1\n"})
load_case("defaults.env missing", {"fx": PROFILE_FX}, lib="nodefaults")
load_case("defaults.env in the installed share", {"fx": PROFILE_FX}, lib="nodefaults", home_files={".local/share/maintainer/defaults.env": "LOCK_WAIT=111\nRETENTION_DAYS=222\n"})
load_case("defaults.env whose last command fails", {"fx": PROFILE_FX}, lib="failing")
load_case("defaults.env that prints", {"fx": PROFILE_FX}, lib="printing")
load_case("defaults.env that prints, early return without a profile file", None, RUN4, lib="printing")
load_case("profile that prints to both streams", {"fx": "echo to-stdout\necho to-stderr >&2\n" + PROFILE_FX})
load_case("profile that fails its last command", {"fx": PROFILE_FX + "false\n"})
load_case("profile that exits 3", {"fx": "echo before\nexit 3\nREPO_SLUG=never\n"})
load_case("profile that exits 0", {"fx": "echo before\nexit 0\n"})
load_case("profile that returns", {"fx": 'REPO_SLUG="before/return"\nreturn 5\nREPO_PATH="/after"\n'})
load_case("profile that is killed", {"fx": "kill -9 $$\n"})
load_case("profile that sets unrelated variables", {"fx": PROFILE_FX + 'PROD_GLOBS="bin/* lib/*"\nCLAIM_LABEL=""\nTASKS="review issues"\n'})
load_case("profile with odd values", {"fx": PROFILE_FX + "V1=$'a\\nb\\tc'\nV2=$'\\xc3\\xa9\\xff'\nV3=''\nV4=\"  padded  \"\n"})
load_case("profile with no positional parameters", {"fx": "NARGS=$#\nFIRST=\"${1-unset}\"\n"})
load_case("profile reads HOME and PATH", {"fx": 'SEEN_HOME="$HOME"\nSEEN_PATH="$PATH"\nSEEN_LC="$LC_ALL"\n' + PROFILE_FX})
load_case("profile uses the MAINTAINER_ variables", {"fx": 'REPO_PATH="${MAINTAINER_REPO_PATH:-$HOME/.local/state/x/checkout}"\nPOST="${MAINTAINER_POST:-on}"\nBACKEND="${MAINTAINER_BACKEND:-claude}"\nREPO_SLUG=o/r\nSTATE_DIR=/s\nGH_ACCOUNT=a\n'},
          [("MAINTAINER_POST", "off"), ("MAINTAINER_REPO_PATH", "/over/ride")])
load_case("profile with unset, readonly, arrays and functions", {"fx": PROFILE_FX + "unset LOCK_WAIT\nARR=(1 2 3)\nf() { echo hi; }\nreadonly RO=1\n"})
load_case("profile that turns allexport off", {"fx": "set +a\nAFTER=1\nset -a\nBEFORE=2\n" + PROFILE_FX})
load_case("profile that changes IFS", {"fx": 'IFS=:\n' + PROFILE_FX})
load_case("profile that defines a compgen function", {"fx": 'compgen() { echo wrong; }\n' + PROFILE_FX})
load_case("profile that sets PATH to nothing", {"fx": 'PATH=/nonexistent\n' + PROFILE_FX + 'POST=changed\n'}, [("MAINTAINER_LOCK_WAIT", "7")], flags="path")
load_case("profile that sets MAINTAINER_ overrides itself", {"fx": PROFILE_FX + "MAINTAINER_LOCK_WAIT=55\n"})
load_case("profile that clears an env override", {"fx": PROFILE_FX + "unset MAINTAINER_LOCK_WAIT\n"}, [("MAINTAINER_LOCK_WAIT", "7")])
load_case("MAINTAINER_PROFILE empty means unnamed", {"fx": PROFILE_FX}, [("MAINTAINER_PROFILE", "")])

for _src in ("REPO_SLUG", "REPO_PATH", "STATE_DIR", "GH_ACCOUNT", "POST"):
    load_case("profile without %s, under set -u" % _src, {"fx": "".join(l + "\n" for l in PROFILE_FX.split("\n") if l and not l.startswith(_src + "="))})
load_case("profile with every source set and empty", {"fx": "REPO_SLUG=\nREPO_PATH=\nSTATE_DIR=\nGH_ACCOUNT=\nPOST=\n"})
load_case("profile without a source, the name preset", {"fx": "REPO_SLUG=x\n"}, [("MAINTAINER_REPO", "/r"), ("MAINTAINER_STATE", "/s"), ("MAINTAINER_ACCOUNT", "a"), ("MAINTAINER_POST", "on")])
load_case("profile that reads an unset variable", {"fx": 'X="$NOT_SET_ANYWHERE"\n' + PROFILE_FX})
load_case("HOME is unset", {"fx": PROFILE_FX}, flags="nohome")
load_case("profile directory name ends in a newline", {"a\n": PROFILE_FX})
load_case("two profile directories, one name ends in a newline", {"a\n": PROFILE_FX, "b": PROFILE_FX})
load_case("defaults.env that sets HOME", None, lib="homesetting", home_files={"alt/.local/share/maintainer/profiles/moved/profile.env": PROFILE_FX})

# ---- the profiles this repository ships ----------------------------------------

if PROFILES_DIR:
    for name in sorted(os.listdir(PROFILES_DIR)):
        env_path = os.path.join(PROFILES_DIR, name, "profile.env")
        if os.path.isfile(env_path):
            body = open(env_path, encoding="utf-8").read()
            load_case("shipped profile %s" % name, {name: body})
            load_case("shipped profile %s with overrides" % name, {name: body},
                      [("MAINTAINER_POST", "off"), ("MAINTAINER_BACKEND", "opencode"),
                       ("MAINTAINER_REPO_PATH", "/over/ride"), ("MAINTAINER_STATE_DIR", "/over/state"),
                       ("MAINTAINER_LOCK_WAIT", "9"), ("MAINTAINER_DEPS_AUTOMERGE", "patch")])

# ---- generated profiles --------------------------------------------------------

KEYS = ["REPO_SLUG", "REPO_PATH", "STATE_DIR", "GH_ACCOUNT", "POST", "LOCK_WAIT", "RETENTION_DAYS", "GH_TRIES", "VERIFY_MEMORY",
        "PROD_GLOBS", "TASKS", "BACKEND", "MODEL_review", "NAME_X", "EMPTY_DEFAULT", "CLAIM_LABEL", "MAINTAINER_NAME", "MIN_HOURS_review"]
ENVPOOL = [("MAINTAINER_LOCK_WAIT", "7"), ("MAINTAINER_RETENTION_DAYS", "30"), ("MAINTAINER_GH_TRIES", ""), ("MAINTAINER_POST", "off"),
           ("MAINTAINER_REPO_PATH", "/over/ride"), ("MAINTAINER_BACKEND", "opencode"), ("MAINTAINER_VERIFY_MEMORY", "4g"),
           ("MAINTAINER_EMPTY_DEFAULT", "x"), ("MAINTAINER_NAME_X", "ignored"), ("MAINTAINER_SLUG", "pre/slug"), ("MAINTAINER_STATE", "/pre/state")]


def rand_value(assigned):
    r = rng.random()
    if r < 0.2:
        return rng.choice(['"o/r"', "plain", '"two words"', "''", '"a$b"', "'single $quoted'", "$'tab\\there'", "$'nl\\nhere'"])
    if r < 0.4:
        return rng.choice(['"$HOME/.local/state/x"', '"${HOME}/y"', '"${MAINTAINER_REPO_PATH:-$HOME/.local/state/q}"',
                           '"${MAINTAINER_POST:-on}"', '"${MAINTAINER_BACKEND:-claude}"', '"${MAINTAINER_NAME_X-unset}"'])
    if r < 0.55 and assigned:
        return '"$%s-suffix"' % rng.choice(assigned)
    if r < 0.65:
        return rng.choice(["$((1+2))", '"$(printf hi)"', '"$(echo a b)"', "$RANDOM_UNSET", '"${UNSET_X:-fallback}"'])
    if r < 0.8:
        return str(rng.randint(0, 5000))
    return rng.choice(['"bin/* lib/* scripts/*"', '"review issues"', '"*"', '"~/x"', '"a;b"', '"a|b"', '"$(printf %s \\"quoted\\")"'])


def rand_profile():
    lines = []
    assigned = []
    for _ in range(rng.randint(0, 14)):
        r = rng.random()
        key = rng.choice(KEYS)
        if r < 0.7:
            lines.append("%s=%s" % (key, rand_value(assigned)))
            assigned.append(key)
        elif r < 0.78 and not key.startswith("MAINTAINER_"):
            lines.append("export %s=%s" % (key, rand_value(assigned)))
            assigned.append(key)
        elif r < 0.84:
            lines.append(": ${%s:=%s}" % (key, rng.choice(["dflt", "5"])))
            assigned.append(key)
        elif r < 0.9:
            lines.append("unset %s" % key)
        elif r < 0.94:
            lines.append("# comment %d" % rng.randint(0, 99))
        elif r < 0.97:
            lines.append("if [ -n \"${%s:-}\" ]; then OTHER_%s=yes; fi" % (key, key))
        else:
            lines.append("")
    return "\n".join(lines) + "\n"


for i in range(60):
    env = rng.sample(ENVPOOL, rng.randint(0, 4))
    names = {n for n, _ in env}
    if rng.random() < 0.2:
        env = [(n, v) for n, v in RUN4 if n not in names] + env + [("PROFILE_DIR", "@BASE@/p")]
        load_case("generated profile %02d (early return)" % i, None, env, extra={"p/profile.env": rand_profile()})
    else:
        load_case("generated profile %02d" % i, {"fx": rand_profile()}, env)

with open(os.path.join(LOAD, "cases.tsv"), "w") as f:
    for idx, (label, lib, flags) in enumerate(load_cases):
        f.write("%04d\t%s\t%s\t%s\n" % (idx, label, lib, flags))
print("load cases:", len(load_cases))
