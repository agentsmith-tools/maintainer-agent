# Task: sweep the issue tracker, and draft what it needs

The tracker is the project's memory. Read the context block at the end first: it
lists the issues that moved since your last run. This profile posts nothing, so
every action below ends as a draft file in this run's drafts directory plus one
line in the report.

## Labels and holds

- Every issue carries one difficulty label (`easy`, `medium` or `hard`) and one
  area label (`gpu`, `cpu`, `sampler`, `cli`, `tests`, `ci`, `docs`, `models`).
  List the issues missing either, with the label you would add and why.
- A `for:<handle>` label holds an issue for one contributor, because GitHub does
  not assign a non-collaborator. Look at each hold. If the person has not
  commented, pushed or opened a pull request on it for 14 days, draft a short
  check-in that offers an exit ("happy to hand it on if your plate is full").
  Never offer the issue to someone else in the same run.
- One open offer per person. A contributor with an unanswered offer gets nothing
  new, because a second offer reads as pressure.

## Placing an unheld issue

For each open issue with no hold, ask who has demonstrated the skill it needs in
this repository in the last 30 days (`gh pr list --state merged`). Draft one
comment offering it to that person by name, with the two or three pointers that
would have saved them an hour. Match on what they built, named specifically.

## Quality of what is filed

- Duplicates: link the older issue in a draft and recommend closing one.
- An issue with no acceptance test or no command to reproduce gets a draft
  comment asking for it.
- `SECURITY.md` forbids a public issue that carries a malformed-GGUF crash input.
  If an open issue carries one, put that at the top of the report. Draft nothing
  public about it.

## What you found

File each finding of a run as one issue draft, named `issue-<slug>.md`, with:

- the command that produced it and its output, quoted;
- the file and line, resolved against the current tree;
- what breaks if nobody fixes it, in one sentence;
- what done looks like, concretely enough that somebody else can tell.

Never draft a suspicion. If you could not reproduce it, say so in the report and
draft nothing. Run `maintainer file-issue` only when POST is on; with POST off
it has nothing to send and the draft is the output.

## Answer the question the documentation should have answered

When you answer a question that a document should have covered, name that
document and draft the gap as its own issue. Apply the stop-slop skill to every
draft.
