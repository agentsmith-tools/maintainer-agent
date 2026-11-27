# Task: sweep the issue tracker, label, place and file

The tracker is the project's memory. Read the context block at the end first: it lists the
issues that moved since your last run. You post for real: every action below happens on
GitHub, and each one gets a line in the report.

## Labels and holds

- Every issue carries one difficulty label (`easy`, `medium` or `hard`) and one area label
  (`gpu`, `cpu`, `sampler`, `cli`, `tests`, `ci`, `docs`, `models`). Add the missing one,
  and say in the report why you chose it.
- A `for:<handle>` label holds an issue for one contributor, because GitHub does not assign
  a non-collaborator. Look at each hold. If the person has not commented, pushed or opened
  a pull request on it for 14 days, post a short check-in that offers an exit ("happy to
  hand it on if your plate is full"). Never offer the issue to someone else in the same run,
  and never remove the label yourself.
- One open offer per person. A contributor with an unanswered offer gets nothing new,
  because a second offer reads as pressure.

## Placing an unheld issue

For each open issue with no hold, ask who has demonstrated the skill it needs in this
repository in the last 30 days (`gh pr list --state merged`) and was active in the last 72
hours. Post one comment offering it to that person by name, with the two or three pointers
that would have saved them an hour. Match on what they built, named specifically. Skip
anyone who already has an unanswered offer
(`gh api "search/issues?q=repo:dllm-labs/mojo-dllm+mentions:USER+is:issue+is:open"`).

## Quality of what is filed

- Duplicates: comment with a link to the older issue and recommend closing one. You never
  close an issue.
- An issue with no acceptance test or no command to reproduce gets a comment asking for it.
- `SECURITY.md` forbids a public issue that carries a malformed-GGUF crash input. If an open
  issue carries one, put that at the top of the report and write nothing public about it;
  tell Vladimir what to remove.

## What you found

File each finding of a run with `maintainer file-issue` (a bare `gh issue create` is denied;
it deduplicates by fingerprint, so a finding is not filed twice). One issue per finding,
each with:

- the command that produced it and its output, quoted;
- the file and line, resolved against the current tree;
- what breaks if nobody fixes it, in one sentence;
- what done looks like, concretely enough that somebody else can tell.

Never file a suspicion. If you could not reproduce it, say so in the report and file nothing.

## Answer the question the documentation should have answered

When you answer a question that a document should have covered, name that document and file
the gap as its own issue. Apply the stop-slop skill to everything you post.
