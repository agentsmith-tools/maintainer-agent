# Task: review every open pull request, and draft what you would say

Read the context block at the end first. It lists what moved since your last
review run. A pull request whose head has not moved since you drafted a review
gets nothing. `maintainer reviewed <pr>` lists what this account already said at
the head now, so use it instead of a hand-built loop over reviews and comments.
Run `maintainer lease check <pr>` before you draft on one and skip a pull request
it reports as leased, saying so in the report.

On the very first run there is no baseline and the block says so. Review the open
pull requests as they stand and say in the report that this run set the baseline.

## Pin the head before you read

```sh
git fetch origin refs/pull/N/head:pN --force
git rev-parse pN
```

Put that 40-character SHA first in your draft. Contributors push mid-review, and
a review of a head that no longer exists wastes their time.

## Screen first, and never run the pull request

```sh
maintainer screen <pr>
```

Whatever it prints, you read the diff and run nothing from it. Say so in the
draft in one plain sentence, so the contributor knows how far your confidence
goes: "I read the diff and CI's results; I did not build it."

## State of the pull request

- A draft pull request gets one line in the report and no review. The author
  has not asked yet.
- A first-time contributor's workflow runs wait for the maintainer to approve
  them. If `gh run list --branch <head-branch> --json status,conclusion` shows
  `action_required`, say so. You cannot approve them. Check that the diff touches
  nothing under `.github/` first and say what you found, because a run executes
  the contributor's workflow code.
- `CONFLICTING` goes back to the author: draft a request to merge `main` into
  the branch and resolve it themselves. Name the conflicted files with
  `git merge-tree --write-tree origin/main pN`. The usual conflict is the
  `tests/test-count.txt` pin: take either side, then
  `UPDATE_TEST_COUNT=1 scripts/run-tests.sh` pins the new total.
- `BEHIND` with a clean merge needs nothing from the author. Note that the
  maintainer presses Update branch and waits for CI again.
- Red CI: read the failing log and say whether the change broke it or the check
  is flaky. Never re-run a check blind.

## Read the diff against the rules

1. Three dots: `git diff origin/main...pN`. Two dots blame the pull request for
   what `main` gained.
2. Every number in the body gets recounted with a command whose output you can
   paste. Most contributors are accurate; recounting is what makes your numbers
   worth reading.
3. Tests added or removed move `tests/test-count.txt` (and `tests/test-counts.txt`
   when the file exists). Count the new `def test_` and `fn test_` lines and
   compare with the pin's change.
4. A CHANGELOG bullet belongs under `## [Unreleased]`. A bullet merged from an
   old base lands inside a released section, so check which section the hunk is
   in on today's `main`.
5. One `Closes #N` per line in the body. `Closes #1, #2` closes only the first.
6. No benchmark number typed into the README or `docs/`. Tables come from
   `scripts/bench_table.py`, and a change that moves performance ships a new
   result file from `bench/run_bench.py`. Anything under the CODEOWNERS paths
   gets a line saying the maintainer reads those files himself.
7. `SECURITY.md` forbids a public issue or pull request text that carries an input
   crashing the GGUF reader. If a diff or body carries one, say so first in the
   report and draft nothing public about it.
8. A change under `.github/` gets its own paragraph: what it runs, with which
   permissions, and whether every `uses:` is pinned to a full SHA.

## Name the mutants, do not run them

The Review table in `CONTRIBUTING.md` decides the proof by change type: a guard
gets every clause broken, a bug fix gets the fix reverted, a test-only change
gets one or two mutants of the code its tests cover, docs and CI config get
none. You cannot run them here. For each pull request write the list the
maintainer should run: file, line, the one-line edit, and the test expected to
fail. A guard clause with no test that could fail is already a finding.

## The draft

Write one file per pull request in the drafts directory, named
`review-<pr>-<head8>.md`:

- First line: `Reviewed at <40-hex head>.`
- Open on the specific thing the author did well, named precisely enough that
  they know you read the diff.
- One blocking item, separated from the optional ones, with which is which. A
  long undifferentiated list makes a finished pull request look unfinished.
- The mutant list for the maintainer, in a section he can delete before posting.
- Close by naming one open issue that fits what they just demonstrated. Skip any
  issue carrying another contributor's `for:<handle>` label.
- Apply the stop-slop skill to it.

## Dependabot pull requests

Drive them with `maintainer deps`. It prints one block per pull request and ends
each with one `next:`. Do not run `maintainer deps rebase`, `maintainer-merge` or
`gh pr review`: they write. Report each `verify` or `human` block under
`## Ready for the maintainer`, with the reasons `deps` printed. For a semver-major
bump read the upstream changelog it links, say which breaking changes touch how
this repository uses the package, and name the files that use it
(`git grep -n <name>`).

## Do not run

`maintainer stalled` and `maintainer-repo fixup` push to a contributor's branch.
This profile never does.

## Housekeeping

End every run with `maintainer-repo release-check` and report the verdict in one
line, even when it is `nothing to release`. You never cut the release.
