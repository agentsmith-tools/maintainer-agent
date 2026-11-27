#!/usr/bin/env python3
"""_claim_in() must match a quoted command only as one whole recorded command.

A claim without `...` used to pass as a substring of the flattened record, so
`gh pr merge 12` passed against a run that merged 123, and a claim that dropped
a trailing `--admin` passed too. Run: python3 -B tests/test_audit_claims.py
The tree under test is MX_TREE when it is set, and this checkout otherwise.
"""
import importlib.machinery
import importlib.util
import os
import sys
import unittest
from pathlib import Path
from unittest import mock

ROOT = (Path(os.environ["MX_TREE"]).resolve() if os.environ.get("MX_TREE")
        else Path(__file__).resolve().parent.parent)


def load_maintainer():
    sys.argv = ["maintainer", "--help"]  # skip profile resolution
    path = str(ROOT / "bin" / "maintainer")
    loader = importlib.machinery.SourceFileLoader("maintainer_claims_under_test", path)
    spec = importlib.util.spec_from_loader("maintainer_claims_under_test", loader)
    mod = importlib.util.module_from_spec(spec)
    loader.exec_module(mod)
    return mod


class ClaimWholeCommand(unittest.TestCase):
    def setUp(self):
        env = mock.patch.dict(os.environ)
        env.start()
        self.addCleanup(env.stop)
        argv = mock.patch.object(sys, "argv", list(sys.argv))
        argv.start()
        self.addCleanup(argv.stop)
        self.mod = load_maintainer()

    def test_different_pr_number_is_not_a_match(self):
        self.assertFalse(self.mod._claim_in("gh pr merge 12", "$ gh pr merge 123 --rebase"))

    def test_dropped_trailing_flag_is_not_a_match(self):
        self.assertFalse(self.mod._claim_in("gh pr merge 77 --squash",
                                            "$ gh pr merge 77 --squash --admin"))

    def test_exact_command_is_a_match_in_each_position(self):
        first = "$ gh pr merge 77 --squash $ make test"
        middle = "$ make lint $ gh pr merge 77 --squash $ make test"
        last = "$ make lint $ gh pr merge 77 --squash"
        for record in (first, middle, last):
            self.assertTrue(self.mod._claim_in("gh pr merge 77 --squash", record), record)

    def test_ellipsis_abridgement_still_matches(self):
        self.assertTrue(self.mod._claim_in(
            "gh run list --limit 5 ... --json databaseId,status \\",
            "$ gh run list --limit 5 --json databaseId,status,conclusion"))


if __name__ == "__main__":
    unittest.main()
