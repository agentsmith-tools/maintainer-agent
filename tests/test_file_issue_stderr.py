#!/usr/bin/env python3
"""file-issue must show gh's error when `gh issue create` fails.

gh writes its errors to stderr. The failure message used to print stdout only,
so it ended at the colon and the reason was lost.
Run: python3 -B tests/test_file_issue_stderr.py
The tree under test is MX_TREE when it is set (tests/run-tests.sh sets it), and
this checkout otherwise.
"""
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = (Path(os.environ["MX_TREE"]).resolve() if os.environ.get("MX_TREE")
        else Path(__file__).resolve().parent.parent)

STUB_ERR = "could not add label: 'triage' not found"

GH_STUB = f"""#!/usr/bin/env bash
case "$*" in
  *'issue list'*) echo '[]';;
  *'issue create'*) echo "{STUB_ERR}" >&2; exit 1;;
esac
exit 0
"""


class FileIssueCreateFailure(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="file-issue-stderr-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        gh = self.bin / "gh"
        gh.write_text(GH_STUB)
        gh.chmod(0o755)
        self.state = self.tmp / "state"
        self.state.mkdir()
        self.body = self.tmp / "body.md"
        self.body.write_text("a finding\n")

    def test_gh_stderr_reaches_the_error(self):
        env = {
            "PATH": f"{self.bin}:{os.environ.get('PATH', '')}",
            "HOME": str(self.tmp),
            "MAINTAINER_STATE": str(self.state),
            "MAINTAINER_SLUG": "o/r",
            "MAINTAINER_REPO": str(self.tmp),
            "MAINTAINER_PROFILE": "fileissue",
            "MAINTAINER_POST": "on",
        }
        proc = subprocess.run(
            ["python3", "-B", str(ROOT / "bin" / "maintainer"), "file-issue",
             "--title", "x finding", "--body-file", str(self.body), "--label", "triage"],
            env=env, capture_output=True, text=True)
        out = proc.stdout + proc.stderr
        self.assertNotEqual(proc.returncode, 0,
                            f"file-issue exited 0 when gh failed: {out!r}")
        self.assertIn(STUB_ERR, out,
                      f"gh's error line is missing from the failure: {out!r}")


if __name__ == "__main__":
    unittest.main()
