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


def gh_stub(stderr_line: str, code: int = 1) -> str:
    """A gh that lists no issues and fails `issue create` with `stderr_line` on stderr."""
    say = f'echo "{stderr_line}" >&2; ' if stderr_line else ""
    return f"""#!/usr/bin/env bash
case "$*" in
  *'issue list'*) echo '[]';;
  *'issue create'*) {say}exit {code};;
esac
exit 0
"""


GH_STUB = gh_stub(STUB_ERR)


class FileIssueCreateFailure(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="file-issue-stderr-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        self.gh = self.bin / "gh"
        self.install_gh(GH_STUB)
        self.state = self.tmp / "state"
        self.state.mkdir()
        self.body = self.tmp / "body.md"
        self.body.write_text("a finding\n")

    def install_gh(self, text):
        self.gh.write_text(text)
        self.gh.chmod(0o755)

    def run_file_issue(self):
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
        return out

    def test_gh_stderr_reaches_the_error(self):
        out = self.run_file_issue()
        self.assertIn(STUB_ERR, out,
                      f"gh's error line is missing from the failure: {out!r}")

    def test_the_exit_code_of_gh_is_named(self):
        self.install_gh(gh_stub(STUB_ERR, code=7))
        out = self.run_file_issue()
        self.assertIn("(exit 7)", out, f"gh's exit code is missing from the failure: {out!r}")

    def test_a_silent_failure_says_so_instead_of_ending_at_the_colon(self):
        self.install_gh(gh_stub("", code=1))
        out = self.run_file_issue()
        self.assertIn("gh printed no reason on stderr", out,
                      f"an empty stderr left no explanation: {out!r}")


if __name__ == "__main__":
    unittest.main()
