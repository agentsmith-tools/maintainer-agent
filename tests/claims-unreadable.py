"""`claims` must not count an unreadable comment thread as an idle claim.

Offline: gh_json is replaced in the loaded module, so nothing touches GitHub.
The issue list answers with #9 held by bob, and the comments of #9 cannot be
read (gh exits non-zero, which gh_json reports as None).

Run from the repository root:  python tests/claims-unreadable.py
"""
import contextlib
import importlib.machinery
import importlib.util
import io
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.environ.update({"MAINTAINER_ACCOUNT": "t", "MAINTAINER_PROFILE": "cv",
                   "MAINTAINER_SLUG": "o/r", "MAINTAINER_REPO": ROOT,
                   "CLAIM_LABEL": "claimed"})

loader = importlib.machinery.SourceFileLoader("maintainer_under_test",
                                              os.path.join(ROOT, "bin", "maintainer"))
spec = importlib.util.spec_from_loader(loader.name, loader)
mod = importlib.util.module_from_spec(spec)
loader.exec_module(mod)


def fake_gh_json(*args):
    text = " ".join(str(a) for a in args)
    if "issue" in args and "list" in args:
        return [{"number": 9, "title": "t", "assignees": [{"login": "bob"}],
                 "updatedAt": "2026-01-01T00:00:00Z", "labels": []}]
    if "/issues/9/comments" in text:
        return None  # the 502: unreadable, not an empty thread
    return None


mod.gh_json = fake_gh_json
mod._owner = lambda: "maintainer"
mod._open_prs = lambda: None

buf = io.StringIO()
code = 0
with contextlib.redirect_stdout(buf):
    try:
        mod.cmd_claims()
    except SystemExit as exc:
        code = exc.code if isinstance(exc.code, int) else 1
out = buf.getvalue()

ok = "UNREADABLE #9" in out and code == 1 and "active" not in out
if ok:
    print("  PASS  an unreadable comment thread is reported UNREADABLE and exits 1")
    sys.exit(0)
print(f"  FAIL  an unreadable comment thread is counted as an idle claim "
      f"(exit {code}): {out.strip()!r}")
sys.exit(1)
