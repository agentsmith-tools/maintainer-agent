//! `maintainer-repo prune [--dry-run]`: delete the branches that are merged into
//! `origin/main`, on the remote and locally.
//!
//! The order of calls is the bash's: `git fetch -q --prune origin`, one
//! `gh pr list`, `git for-each-ref` on the remote refs, a `git merge-base
//! --is-ancestor` and a `git push -q origin --delete` per branch, the same for
//! the local branches with `git branch -D`, and on a real run `git worktree
//! prune` and `git remote prune origin`.
//!
//! # Kept as the bash has it
//!
//! - **F5.** The open-pull-request guard never fires. The bash joins the open
//!   heads with `|` and asks `grep -qx` for a line equal to the branch, so a
//!   branch matches only when its whole name is the joined string. [`decide_remote`]
//!   keeps that test, and a pull request's own head is listed as deletable.
//!   Output change proposed in the pull request that added this crate: fix the
//!   test (split the heads, compare each) and print `keep` lines.
//! - **Dry runs exit 1.** The last command of the bash function is
//!   `[ "$dry" = 0 ] && { ... }`, so a dry run (or `POST=off`) ends with that
//!   test's status.
//! - Only a *first* argument equal to `--dry-run` is a dry run. `--dryrun` and
//!   `--verbose --dry-run` delete for real.
//! - A failed `gh pr list` reads as "no open pull requests" and `prune` goes on
//!   to delete. That is fail-open, listed under "Found, not fixed" in the pull
//!   request: with the guard dead it changes nothing today, and it matters the
//!   day F5 is fixed.
//! - A merged local branch that is checked out cannot be deleted. `git branch
//!   -D` fails, nothing is printed, and the section does not say `none`.
//! - `git branch -D`, not `-d`: a branch merged into `origin/main` is deleted
//!   even when the local `main` has not caught up.
//!
//! # Found, not fixed
//!
//! `git push -q origin --delete BRANCH` deletes a remote branch on the strength
//! of an ancestry test against `origin/main` alone. There is no check that the
//! branch has no open pull request (F5), no confirmation, and `POST=off` is the
//! only brake. The plan lists F5; the rest is preserved.

use crate::ctx::{Call, Ctx, R};
use crate::shellish::{cmd_subst, lines, read_trim};
use crate::{Settings, enter_repo};
use mx_core::Fact;
use std::ffi::OsString;

/// What the first argument asks for. Only the exact word `--dry-run` counts.
#[must_use]
pub fn is_dry_run(args: &[OsString]) -> bool {
    args.first().is_some_and(|arg| arg == "--dry-run")
}

/// The open pull requests' head names as the bash holds them: the output of
/// `gh pr list` with every newline turned into `|` (`tr '\n' '|'`), then
/// `$(...)`.
#[must_use]
pub fn join_open_heads(gh_output: &[u8]) -> String {
    let joined: Vec<u8> =
        gh_output.iter().map(|byte| if *byte == b'\n' { b'|' } else { *byte }).collect();
    String::from_utf8_lossy(&cmd_subst(&joined)).into_owned()
}

/// What one remote branch asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Not a candidate: a protected name, not merged, or merge state unknown.
    Skip,
    /// Has an open pull request, so it stays and `keep` is printed.
    KeepOpenPr,
    /// Merged into `origin/main`: delete it, or list it on a dry run.
    Delete,
}

/// The names `prune` never touches. `HEAD` appears only among remote refs
/// (`origin/HEAD`).
#[must_use]
pub fn is_protected(name: &str, remote: bool) -> bool {
    matches!(name, "main" | "master") || (remote && name == "HEAD")
}

/// F5: the bash's open-pull-request test. `grep -qx -F -- "$br" <<<"$open_heads"`
/// is true when a whole line of the joined string equals the branch, and the
/// joined string is one line, so only a branch named like the whole string
/// matches. Pinned: see the plan's decision F5.
#[must_use]
pub fn open_pr_guard_fires(branch: &str, open_heads: &str) -> bool {
    !open_heads.is_empty() && branch == open_heads
}

/// Decides one remote branch.
///
/// `open_heads` is what `gh pr list` printed. An `Unreadable` one (`gh` could
/// not be started) is read as no open pull requests, as the bash reads it
/// (fail-open, pinned). `merged` is asked only when the name passes the two
/// tests that come first, as the bash runs `git merge-base` only then. Its
/// answer is a fact: `Known(true)` for status 0, `Known(false)` for 1, and
/// `Unreadable` for any other status. Only a known `true` deletes.
pub fn decide_remote(
    branch: &str,
    open_heads: &Fact<String>,
    merged: impl FnOnce() -> Fact<bool>,
) -> Verdict {
    if is_protected(branch, true) {
        return Verdict::Skip;
    }
    if let Fact::Known(heads) = open_heads
        && open_pr_guard_fires(branch, heads)
    {
        return Verdict::KeepOpenPr;
    }
    delete_if_merged(&merged())
}

/// Decides one local branch. There is no open-pull-request test for local ones.
pub fn decide_local(branch: &str, merged: impl FnOnce() -> Fact<bool>) -> Verdict {
    if is_protected(branch, false) {
        return Verdict::Skip;
    }
    delete_if_merged(&merged())
}

fn delete_if_merged(merged: &Fact<bool>) -> Verdict {
    match merged {
        Fact::Known(true) => Verdict::Delete,
        Fact::Known(false) | Fact::Absent | Fact::Unreadable(_) => Verdict::Skip,
    }
}

/// `git merge-base --is-ancestor REV origin/main`, as a fact. Its standard
/// output goes through, and its standard error is dropped.
fn is_ancestor(ctx: &mut Ctx<'_>, rev: &str) -> Fact<bool> {
    let done = ctx.git(&["merge-base", "--is-ancestor", rev, "origin/main"]);
    ctx.show_out(&done);
    match done.rc {
        0 => Fact::Known(true),
        1 => Fact::Known(false),
        rc => Fact::Unreadable(format!("git merge-base exited {rc}")),
    }
}

/// The branch names a `for-each-ref` printed, as the bash's `while read -r`
/// reads them. A name that is not UTF-8 ends the program before anything is
/// deleted (decision P1H-8).
fn names(ctx: &mut Ctx<'_>, stdout: &[u8]) -> R<Vec<String>> {
    let mut found = Vec::new();
    for line in lines(stdout) {
        let name = read_trim(line);
        if name.is_empty() {
            continue;
        }
        match std::str::from_utf8(name) {
            Ok(text) => found.push(text.to_string()),
            Err(_) => {
                return Err(ctx.die(&format!(
                    "a branch name is not UTF-8 text, so nothing was deleted: {}",
                    String::from_utf8_lossy(name)
                )));
            }
        }
    }
    Ok(found)
}

/// `maintainer-repo prune`.
pub fn cmd_prune(ctx: &mut Ctx<'_>, settings: &Settings, args: &[OsString]) -> R<i32> {
    let mut dry = is_dry_run(args);
    if settings.rehearsal() {
        dry = true;
        ctx.say("POST=off: listing only, nothing will be deleted");
    }
    enter_repo(ctx, settings, 32)?;
    let fetch = ctx.git(&["fetch", "-q", "--prune", "origin"]);
    ctx.show_both(&fetch);
    if !fetch.ok() {
        return Err(ctx.die("fetch failed"));
    }

    // gh's own errors go to /dev/null, and its status is not read.
    let listing = ctx.exec(&Call::new([
        "gh",
        "pr",
        "list",
        "--repo",
        settings.slug.as_str(),
        "--state",
        "open",
        "--json",
        "headRefName",
        "--jq",
        ".[].headRefName",
    ]));
    // Without `tr` the pipeline prints nothing and the list reads empty (decision P1H-16).
    let open_heads = if ctx.missing_tool("tr", 38) {
        Fact::Known(String::new())
    } else {
        listing.stdout_fact().map(|bytes| join_open_heads(&bytes))
    };

    ctx.say("remote branches merged into main:");
    let refs = ctx.git(&["for-each-ref", "--format=%(refname:strip=3)", "refs/remotes/origin"]);
    ctx.show_err(&refs);
    let remote = names(ctx, &refs.stdout)?;
    let mut deletable = 0_u32;
    for branch in &remote {
        // `[ -n "$open_heads" ] && grep -qx ...` runs `grep` only for a branch that is
        // not protected, and a host without it reads as "no match" (decision P1H-16).
        let heads = match &open_heads {
            Fact::Known(heads)
                if !heads.is_empty()
                    && !is_protected(branch, true)
                    && ctx.missing_tool("grep", 45) =>
            {
                Fact::Known(String::new())
            }
            other => other.clone(),
        };
        match decide_remote(branch, &heads, || is_ancestor(ctx, &format!("origin/{branch}"))) {
            Verdict::Skip => {}
            Verdict::KeepOpenPr => ctx.say(&format!("  keep   {branch} (open PR)")),
            Verdict::Delete => {
                deletable += 1;
                if dry {
                    ctx.say(&format!("  would delete {branch}"));
                } else {
                    let push = ctx.git(&["push", "-q", "origin", "--delete", branch]);
                    ctx.show_out(&push);
                    if push.ok() {
                        ctx.say(&format!("  deleted {branch}"));
                    } else {
                        ctx.say(&format!(
                            "  could not delete {branch} (protected or already gone)"
                        ));
                    }
                }
            }
        }
    }
    if deletable == 0 {
        ctx.say("  none");
    }

    ctx.say("local branches fully merged into main:");
    let heads = ctx.git(&["for-each-ref", "--format=%(refname:strip=2)", "refs/heads"]);
    ctx.show_err(&heads);
    let local = names(ctx, &heads.stdout)?;
    let mut mergeable = 0_u32;
    for branch in &local {
        if decide_local(branch, || is_ancestor(ctx, branch)) == Verdict::Delete {
            mergeable += 1;
            if dry {
                ctx.say(&format!("  would delete {branch}"));
            } else if ctx.git(&["branch", "-D", branch]).ok() {
                ctx.say(&format!("  deleted {branch}"));
            }
        }
    }
    if mergeable == 0 {
        ctx.say("  none");
    }
    if dry {
        // `[ "$dry" = 0 ] && { ...; }` is the last command: false, status 1.
        return Ok(1);
    }
    let worktrees = ctx.git(&["worktree", "prune"]);
    ctx.show_both(&worktrees);
    drop(ctx.git(&["remote", "prune", "origin"]));
    ctx.say("worktrees and remote refs pruned");
    Ok(0)
}
