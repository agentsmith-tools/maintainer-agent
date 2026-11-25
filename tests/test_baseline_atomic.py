#!/usr/bin/env python3
"""A truncated last-<task>.json must not stop `start`, and `finish` must write it atomically.

`finish` promotes the baseline with write_text, so a kill mid-write leaves
truncated JSON there, and `start` used to raise JSONDecodeError on it for every
later run. `finish` now writes a temp file and os.replace()s it into place.
Run: python3 -B tests/test_baseline_atomic.py
The tree under test is MX_TREE when it is set (tests/run-tests.sh sets it), and
this checkout otherwise.
"""
import contextlib
import io
import json
import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_refresh_pr_fetch import load_maintainer  # noqa: E402


class BaselineAtomic(unittest.TestCase):
    def setUp(self):
        env = mock.patch.dict(os.environ)
        env.start()
        self.addCleanup(env.stop)
        self.tmp = Path(tempfile.mkdtemp(prefix="baseline-atomic-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        state = self.tmp / "state"
        os.environ.update(MAINTAINER_PROFILE="t", MAINTAINER_SLUG="o/r",
                          MAINTAINER_ACCOUNT="me", MAINTAINER_TASKS="review")
        self.mod = load_maintainer(self.tmp, state)
        self.mod.refresh = lambda: ("a" * 40, "b" * 40, [], [])
        # snapshot() asks gh for open PRs and issues. The test is about the baseline
        # file, not GitHub, and gh is not installed on every host that runs the suite.
        self.mod.snapshot = lambda main_sha: {"main_sha": main_sha, "workspace_tests": None,
                                              "open_prs": {}, "open_issues": {}}
        self.mod.STATE.mkdir(parents=True, exist_ok=True)
        self.baseline = self.mod.STATE / "last-review.json"

    def test_start_survives_truncated_baseline(self):
        self.baseline.write_text('{"run_id": "x", "heads')
        buf = io.StringIO()
        try:
            with contextlib.redirect_stdout(buf):
                self.mod.cmd_start("review")
        except Exception as e:  # the bug: start died here
            self.fail(f"start raised {type(e).__name__}: {e}")
        self.assertIn("last-review.json", buf.getvalue(),
                      "start did not name the unreadable baseline file")
        self.assertTrue((self.mod.STATE / "pending-review.json").exists(),
                        "start stopped before writing the pending baseline")

    def test_finish_promotes_baseline_via_os_replace(self):
        with contextlib.redirect_stdout(io.StringIO()):
            self.mod.cmd_start("review")
        run_id = json.loads((self.mod.STATE / "pending-review.json").read_text())["run_id"]
        self.baseline.write_text('{"run_id": "old"}\n')
        report = self.mod.RUNS / f"{run_id}.md"
        report.write_text("# " + run_id + "\n\n" + "\n".join(f"line {i}" for i in range(20)) + "\n")
        # An interrupted promotion: the replace that would publish the new baseline fails.
        with mock.patch.object(self.mod.os, "replace",
                               side_effect=OSError("interrupted")) as rep:
            try:
                with contextlib.redirect_stdout(io.StringIO()):
                    self.mod.cmd_finish(run_id)
            except OSError:
                pass
        self.assertEqual(self.baseline.read_text(), '{"run_id": "old"}\n',
                         "finish overwrote the baseline without os.replace")
        self.assertTrue(rep.called, "finish never used os.replace for the baseline")
        self.assertEqual(rep.call_args[0][1], self.baseline,
                         "os.replace did not target last-review.json")


if __name__ == "__main__":
    unittest.main()
