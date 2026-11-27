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
import tempfile
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

    def test_complete_statements_match_inside_chains_and_pipelines(self):
        record = "$ cargo fmt --check && git diff --check | head -20; make test"
        for claim in ("cargo fmt --check", "git diff --check", "head -20",
                      "git diff --check | head -20", "make test"):
            with self.subTest(claim=claim):
                self.assertTrue(self.mod._claim_in(claim, record))

    def test_shell_separator_boundaries_include_or_redirect_and_background(self):
        cases = (
            ("exact", "$ before || exact"),
            ("exact", "$ before | exact"),
            ("echo done", "$ echo done > output"),
            ("echo done", "$ echo done < input"),
            ("echo done", "$ echo done &"),
            ("echo done", "$ echo done)"),
        )
        for claim, record in cases:
            with self.subTest(claim=claim, record=record):
                self.assertTrue(self.mod._claim_in(claim, record))

    def test_line_boundaries_and_write_entries_do_not_hide_bash_calls(self):
        record = ("$ git status --short\n"
                  "Write: /tmp/audit-report.md\n"
                  "$ printf '%s\\n' done\n")
        self.assertTrue(self.mod._claim_in("git status --short", record))
        self.assertTrue(self.mod._claim_in("printf '%s\\n' done", record))

    def test_audit_run_preserves_line_boundary_before_write_entry(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            runs, logs = root / "runs", root / "logs"
            runs.mkdir()
            logs.mkdir()
            run_id = "2026-11-27T00-00-review"
            (runs / f"{run_id}.md").write_text(
                "# review\n\n```bash\n$ git status --short\n```\n")
            (logs / f"{run_id}.commands").write_text(
                "$ git status --short\nWrite: /tmp/audit-report.md\n")
            with mock.patch.object(self.mod, "RUNS", runs), \
                 mock.patch.object(self.mod, "LOGS", logs):
                verdict, missing, count = self.mod.audit_run(run_id)
        self.assertEqual(("all 1 quoted command(s) appear in the record", [], 1),
                         (verdict, missing, count))

    def test_claim_starting_mid_command_is_not_a_match(self):
        self.assertFalse(self.mod._claim_in("merge 77 --squash",
                                            "$ gh pr merge 77 --squash"))

    def test_claim_that_is_a_prefix_of_extra_arguments_is_not_a_match(self):
        self.assertFalse(self.mod._claim_in("echo ok", "$ echo ok $HOME"))

    def test_ellipsis_abridgement_still_matches(self):
        self.assertTrue(self.mod._claim_in(
            "gh run list --limit 5 ... --json databaseId,status \\",
            "$ gh run list --limit 5 --json databaseId,status,conclusion"))

    def test_a_redirect_with_a_file_descriptor_ends_the_statement(self):
        # `make test 2>&1 | tail -5` is how agents run most commands, and a report quotes `make test`.
        self.assertTrue(self.mod._claim_in("make test", "$ make test 2>&1 | tail -5"))
        self.assertTrue(self.mod._claim_in("make test", "$ make test 2> err.log"))

    def test_a_statement_inside_a_command_substitution_matches(self):
        self.assertTrue(self.mod._claim_in("make test", '$ out="$(make test 2>&1)"; echo $?'))

    def test_a_line_of_a_multi_line_command_is_a_statement(self):
        record = "$ for n in 1 2; do\n  echo $n\n  gh pr merge 77 --squash\ndone\n"
        self.assertTrue(self.mod._claim_in("gh pr merge 77 --squash", record))

    def test_whitespace_the_agent_typed_in_the_record_is_collapsed(self):
        self.assertTrue(self.mod._claim_in("gh pr merge 77", "$ gh  pr\tmerge   77\n"))

    def test_ellipsis_fragments_match_across_that_whitespace(self):
        self.assertTrue(self.mod._claim_in(
            "gh run list --limit 5 ... --json databaseId",
            "$ gh  run list  --limit 5 --json  databaseId,status\n"))


if __name__ == "__main__":
    unittest.main()
