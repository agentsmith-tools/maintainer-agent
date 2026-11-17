//! `maintainer-repo fixup <pr> [--dry-run]`: finish a stalled pull request's
//! mechanical review items.
//!
//! Every decision is `maintainer fixup-check`'s, a Python tool this crate runs
//! as `python3 BIN_DIR/maintainer fixup-check PR` (plan decision D1). Its
//! standard output is read as `key=value` lines, and a non-zero exit ends the
//! program with 1 after the tool's own message. This crate does the git and
//! decides nothing the tool decides.
//!
//! What happens, in the bash's order:
//!
//! 1. The command line: digits only, the last number wins, `--dry-run` anywhere.
//! 2. `POST=off` makes the run a rehearsal and says so.
//! 3. `fixup-check` answers. An answer without a state, head, branch, fork or
//!    items is refused.
//! 4. The account: `gh auth switch`, then `gh api user`, which must name the
//!    profile's account.
//! 5. State `notice`: post a heads-up comment from a temp file, ask
//!    `fixup-check` again, and say so only when the answer shows it was posted.
//! 6. Any other state: a scratch clone of the contributor's branch with hooks
//!    off, an optional rebase onto the base branch, a patch applied, a commit
//!    credited to the author, a push (leased when the branch was rewritten), and a
//!    read-back of the head from the pull request.
//!
//! # Kept as the bash has it
//!
//! - Any state other than `notice` takes the push path, including a state the
//!   tool never prints today.
//! - Each `key=value` field is every matching line of the answer, so a repeated
//!   key makes a multi-line value.
//! - `git remote add base` is run once per `rebase` item, so a second one fails
//!   with git's own message on standard error and the run goes on.
//! - The read-back asks GitHub `GH_TRIES` times with `GH_BACKOFF` seconds
//!   between asks. A `GH_TRIES` that is not a bash integer runs no read at all,
//!   so the run ends as "NOT confirmed".
//! - A push that succeeds and a read-back that fails leaves the branch pushed.
//!   The message says NOT confirmed and tells the maintainer to look.
//!
//! # Found, not fixed
//!
//! - `git push --force-with-lease` rewrites a contributor's branch after a
//!   rebase. The lease names the head that was checked, so a push made since
//!   is never overwritten, and the case is pinned by a test. The rewrite
//!   itself is the program's purpose and is not behind a second prompt.
//! - `FIXUP_COUNT_CMD` is profile text run with `bash -c` inside the scratch
//!   clone of an untrusted branch. The profile is the maintainer's own, but
//!   the clone's files are the contributor's.
//! - The scratch clone is removed on SIGHUP, SIGINT and SIGTERM as the bash's
//!   `EXIT` trap removes it, and the program then dies of the signal (status
//!   129, 130, 143). A child that git started keeps running when the signal
//!   names this process alone, as it does in the bash; a terminal's Ctrl-C
//!   reaches the bash's children and does not reach a child here (decision
//!   P1H-7).

use crate::ctx::{Call, Ctx, R, Stop};
use crate::shellish::{
    bash_integer, cmd_subst, decimal_integer, indent4, is_whole_number, lines, sed_field,
};
use crate::{Settings, Tree};
use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The command line of `fixup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// The pull request number, as typed (`007` stays `007`).
    pub pr: Option<String>,
    pub dry_run: bool,
}

/// `for a in "$@"`: `--dry-run`, else a word of digits sets the pull request,
/// and any other word clears it and stops reading.
#[must_use]
pub fn parse_args(args: &[OsString]) -> Args {
    let mut parsed = Args { pr: None, dry_run: false };
    for arg in args {
        let bytes = arg.as_bytes();
        if bytes == b"--dry-run" {
            parsed.dry_run = true;
        } else if bytes.is_empty() || bytes.iter().any(|byte| !byte.is_ascii_digit()) {
            parsed.pr = None;
            break;
        } else {
            parsed.pr = Some(String::from_utf8_lossy(bytes).into_owned());
        }
    }
    parsed
}

/// The answer of `fixup-check`, field by field.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Check {
    pub state: String,
    pub head: String,
    pub base: String,
    pub branch: String,
    pub fork: String,
    pub author: String,
    pub items: String,
    pub patch: String,
    pub marker: String,
    pub notice_due: String,
    pub notice_days: String,
    pub review_age: String,
}

impl Check {
    /// `[ -n "$state" ] && [ -n "$head" ] && [ -n "$branch" ] && [ -n "$fork" ]
    /// && [ -n "$items" ]`.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        [&self.state, &self.head, &self.branch, &self.fork, &self.items]
            .iter()
            .all(|field| !field.is_empty())
    }
}

/// Reads the answer. `Err` names a field whose text is not UTF-8, which a
/// process argument here cannot carry (decision P1H-8).
pub fn parse_check(out: &[u8]) -> Result<Check, String> {
    let get = |key: &str| {
        String::from_utf8(sed_field(out, key)).map_err(|_| format!("{key} is not UTF-8 text"))
    };
    Ok(Check {
        state: get("state")?,
        head: get("head")?,
        base: get("base")?,
        branch: get("branch")?,
        fork: get("fork")?,
        author: get("author")?,
        items: get("items")?,
        patch: get("patch")?,
        marker: get("notice_marker")?,
        notice_due: get("notice_due")?,
        notice_days: get("notice_days")?,
        review_age: get("review_age")?,
    })
}

/// `tr ' ' '\n' <<<"$items" | sed 's/:.*//'`: one kind per line, an empty line
/// for each extra space.
#[must_use]
pub fn item_kinds(items: &str) -> Vec<String> {
    let spread = [items.replace(' ', "\n").as_bytes(), b"\n"].concat();
    lines(&spread)
        .into_iter()
        .map(|line| {
            let text = String::from_utf8_lossy(line);
            text.split_once(':').map_or_else(|| text.to_string(), |(kind, _)| kind.to_string())
        })
        .collect()
}

/// `paste -sd, - | sed 's/,/, /g'`.
#[must_use]
pub fn join_kinds(kinds: &[String]) -> String {
    kinds.join(",").replace(',', ", ")
}

/// The heads-up comment, as `$(printf ...)` leaves it: no trailing newline.
#[must_use]
pub fn notice_body(review_age: &str, kinds: &str, due: &str, marker: &str) -> String {
    let text = format!(
        "This has had no reply for {review_age} days since my review, and what is left is mechanical ({kinds}).\n\nI will push a fixup for it to your branch after {due} unless you push first. Your commits stay yours, and I will credit you on anything I add. If you would rather do it yourself, push or reply and I will leave it alone.\n\n{marker}\n"
    );
    String::from_utf8_lossy(&cmd_subst(text.as_bytes())).into_owned()
}

/// Does an added line of the patch state the count? The bash's
/// `awk '/^\+[^+]/ && $0 ~ ("(^|[^0-9])" n "([^0-9]|$)")'`: a line that starts
/// with `+` and a second character that is not `+`, holding `n` as a whole number.
#[must_use]
pub fn patch_states_count(added: &[u8], n: &str) -> bool {
    let needle = n.as_bytes();
    if needle.is_empty() {
        return false;
    }
    lines(added).into_iter().any(|line| {
        line.len() >= 2
            && line[0] == b'+'
            && line[1] != b'+'
            && (0..line.len()).any(|at| {
                line[at..].starts_with(needle)
                    && (at == 0 || !line[at - 1].is_ascii_digit())
                    && line.get(at + needle.len()).is_none_or(|next| !next.is_ascii_digit())
            })
    })
}

/// Splits `gh api ... --jq '[.id,.login]|@tsv'` output at the first tab:
/// `${me%%$'\t'*}` and `${me#*$'\t'}`. With no tab both are the whole text.
#[must_use]
pub fn split_account(me: &str) -> (&str, &str) {
    match me.split_once('\t') {
        Some((id, login)) => (id, login),
        None => (me, me),
    }
}

/// `[ "${a,,}" = "${b,,}" ]` for account names (ASCII case folding, decision P1H-9).
#[must_use]
pub fn same_login(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Does the read-back confirm a heads-up? `grep -qE 'heads-up is not ripe|^state=ready$'`.
#[must_use]
pub fn heads_up_confirmed(again: &[u8]) -> bool {
    lines(again).into_iter().any(|line| {
        line == b"state=ready"
            || line.windows(b"heads-up is not ripe".len()).any(|w| w == b"heads-up is not ripe")
    })
}

fn path_text(ctx: &mut Ctx<'_>, path: &Path) -> R<String> {
    match path.to_str() {
        Some(text) => Ok(text.to_string()),
        None => Err(ctx.die(&format!("a path is not UTF-8 text: {}", path.display()))),
    }
}

/// `NAME: unbound variable` under `set -u`: the diagnostic names the script and the line, and a
/// script file ends with status 1 (bash 5.3.9; only `bash -c` gives 127).
fn unbound(ctx: &mut Ctx<'_>, name: &str, line: u32) -> Stop {
    let script = ctx.script_name();
    ctx.put_err(format!("{script}: line {line}: {name}: unbound variable\n").as_bytes());
    Stop(1)
}

/// `${MAINTAINER_STATE:?}`: the state directory, or the shell's own error when it is empty.
fn require_state(ctx: &mut Ctx<'_>, settings: &Settings, line: u32) -> R<String> {
    if settings.state.is_empty() {
        let name = ctx.script_name();
        ctx.put_err(
            format!("{name}: line {line}: MAINTAINER_STATE: parameter null or not set\n")
                .as_bytes(),
        );
        return Err(Stop(1));
    }
    Ok(settings.state.clone())
}

/// `python3 BIN_DIR/maintainer fixup-check PR`.
fn fixup_check(ctx: &Ctx<'_>, here: &str, pr: &str, line: u32) -> crate::ctx::Done {
    ctx.exec(&Call::new(["python3", &format!("{here}/maintainer"), "fixup-check", pr]).at(line))
}

/// `$(gh api ... 2>/dev/null)` for an account lookup.
fn lookup(ctx: &Ctx<'_>, endpoint: &str) -> Option<String> {
    let done = ctx.exec(&Call::new(["gh", "api", endpoint, "--jq", "[.id,.login]|@tsv"]));
    done.ok().then(|| String::from_utf8_lossy(&cmd_subst(&done.stdout)).into_owned())
}

/// `maintainer-repo fixup`.
pub fn cmd_fixup(ctx: &mut Ctx<'_>, settings: &Settings, tree: &Tree, args: &[OsString]) -> R<i32> {
    let parsed = parse_args(args);
    let Some(pr) = parsed.pr else {
        ctx.put_err(b"usage: maintainer-repo fixup <pr> [--dry-run]\n");
        return Err(Stop(64));
    };
    let mut dry = parsed.dry_run;
    if settings.rehearsal() {
        dry = true;
        ctx.say("POST=off: rehearsing, nothing will be posted or pushed");
    }
    let here = path_text(ctx, &tree.bin_dir)?;
    let answered = fixup_check(ctx, &here, &pr, 221);
    ctx.show_err(&answered);
    if !answered.ok() {
        return Err(Stop(1));
    }
    let out = cmd_subst(&answered.stdout);
    // Eleven `$(sed -n 's/^key=//p')` reads. Without `sed` each prints its message and every
    // field is empty, so the check below refuses (decision P1H-16).
    if !ctx.has_tool("sed") {
        for line in 223..=233 {
            ctx.missing_tool("sed", line);
        }
        return Err(ctx.die("fixup-check answered without a state, head, branch, fork or items; refusing to act on that"));
    }
    let check = match parse_check(&out) {
        Ok(check) => check,
        Err(why) => return Err(ctx.die(&format!(
            "fixup-check answered with text that cannot be used ({why}); refusing to act on that"
        ))),
    };
    if !check.is_complete() {
        return Err(ctx.die("fixup-check answered without a state, head, branch, fork or items; refusing to act on that"));
    }

    // Who is acting. A write that succeeds under the wrong account is worse
    // than one that fails.
    drop(ctx.exec(&Call::new([
        "gh",
        "auth",
        "switch",
        "--hostname",
        "github.com",
        "--user",
        &settings.account,
    ])));
    let Some(me) = lookup(ctx, "user") else {
        return Err(ctx.die("gh could not confirm an account"));
    };
    let (me_id, me_login) = split_account(&me);
    if !is_whole_number(me_id) {
        return Err(ctx.die("gh did not return a numeric account id"));
    }
    if !same_login(me_login, &settings.account) {
        return Err(
            ctx.die(&format!("gh is authenticated as '{me_login}', not {}", settings.account))
        );
    }
    let (me_id, me_login) = (me_id.to_string(), me_login.to_string());

    if check.state == "notice" {
        return notice(ctx, settings, &here, &pr, &check, dry);
    }
    let work = {
        let state = require_state(ctx, settings, 270)?;
        let made = ctx.exec(&Call::new(["mktemp", "-d", &format!("{state}/fixup-XXXXXX")]).at(270));
        ctx.show_err(&made);
        if !made.ok() {
            return Err(ctx.die("no scratch directory"));
        }
        PathBuf::from(String::from_utf8_lossy(&cmd_subst(&made.stdout)).into_owned())
    };
    // The bash's EXIT trap: `rm -rf` of the scratch clone however the run ends,
    // a fatal signal included (decision P1H-7).
    // Without `rm` the trap removes nothing and says so when the run ends.
    let removable = ctx.has_tool("rm");
    let scratch = removable.then(|| crate::cleanup::Scratch::arm(&work));
    let result = push_path(ctx, settings, &pr, &check, &work, (&me_id, &me_login), dry);
    drop(scratch);
    if !removable {
        ctx.missing_tool("rm", 1); // the trap string is line 1 of its own text
    }
    result
}

/// The heads-up path: post a comment and prove it is there.
fn notice(
    ctx: &mut Ctx<'_>,
    settings: &Settings,
    here: &str,
    pr: &str,
    check: &Check,
    dry: bool,
) -> R<i32> {
    // `tr | sed | paste | sed`: a host without `tr` or `paste` leaves the list empty.
    let no_tr = ctx.missing_tool("tr", 247);
    let no_paste = ctx.missing_tool("paste", 247);
    let kinds =
        if no_tr || no_paste { String::new() } else { join_kinds(&item_kinds(&check.items)) };
    let body = notice_body(&check.review_age, &kinds, &check.notice_due, &check.marker);
    if dry {
        ctx.say(&format!(
            "would post this heads-up on #{pr} (ripe after {}, {} days):",
            check.notice_due, check.notice_days
        ));
        ctx.put_out(&indent4(format!("{body}\n").as_bytes()));
        return Ok(0);
    }
    let state = require_state(ctx, settings, 255)?;
    let made = ctx.exec(&Call::new(["mktemp", &format!("{state}/fixup-notice.XXXXXX")]).at(255));
    ctx.show_err(&made);
    if !made.ok() {
        return Err(ctx.die("no scratch file"));
    }
    let file = PathBuf::from(String::from_utf8_lossy(&cmd_subst(&made.stdout)).into_owned());
    let written = std::fs::write(&file, body.as_bytes());
    let file_text = path_text(ctx, &file)?;
    let posted = ctx.exec(&Call::new([
        "gh",
        "pr",
        "comment",
        pr,
        "--repo",
        &settings.slug,
        "--body-file",
        &file_text,
    ]));
    // Without `rm` the file stays where it is.
    if !ctx.missing_tool("rm", 258) {
        drop(std::fs::remove_file(&file));
    }
    if !posted.ok() || written.is_err() {
        return Err(ctx.die(&format!("could not post the heads-up on #{pr}")));
    }
    // Posting is not the same as it being there. Ask again.
    let again = fixup_check(ctx, here, pr, 262);
    let seen = cmd_subst(&[again.stdout.as_slice(), again.stderr.as_slice()].concat());
    if ctx.missing_tool("grep", 263) || !heads_up_confirmed(&seen) {
        return Err(ctx.die(&format!(
            "posted the heads-up but could not read it back on #{pr}, so I cannot say the author was told"
        )));
    }
    ctx.say(&format!(
        "posted the heads-up on #{pr}; a fixup may follow after {}",
        check.notice_due
    ));
    Ok(0)
}

/// The scratch clone's git: `git -C WORK/r -c core.hooksPath=/dev/null ...`.
struct Clone {
    repo: String,
}

impl Clone {
    fn argv(&self, args: &[&str]) -> Vec<String> {
        let mut words: Vec<String> = [
            "git",
            "-C",
            self.repo.as_str(),
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.fsmonitor=false",
        ]
        .map(String::from)
        .to_vec();
        words.extend(args.iter().map(|arg| (*arg).to_string()));
        words
    }

    fn run(&self, ctx: &Ctx<'_>, args: &[&str]) -> crate::ctx::Done {
        ctx.exec(&Call::new(self.argv(args)))
    }

    /// `$(g ARGS)` with git's standard error let through.
    fn capture(&self, ctx: &mut Ctx<'_>, args: &[&str]) -> (bool, String) {
        let done = self.run(ctx, args);
        ctx.show_err(&done);
        (done.ok(), String::from_utf8_lossy(&cmd_subst(&done.stdout)).into_owned())
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one function per bash branch would hide the order of the calls"
)]
fn push_path(
    ctx: &mut Ctx<'_>,
    settings: &Settings,
    pr: &str,
    check: &Check,
    work: &Path,
    me: (&str, &str),
    dry: bool,
) -> R<i32> {
    let (me_id, me_login) = me;
    let url = format!("https://github.com/{}.git", check.fork);
    let base_url = format!("https://github.com/{}.git", settings.slug);
    ctx.exports.push(("GIT_TERMINAL_PROMPT".into(), "0".into()));
    ctx.exports.push(("GIT_EDITOR".into(), "true".into()));
    let repo = path_text(ctx, &work.join("r"))?;
    let g = Clone { repo: repo.clone() };
    // -c core.hooksPath=/dev/null on every command: a hook is contributor-controlled
    // text the moment it is in the tree, and this runs git only.
    let cloned = ctx.exec(&Call::new([
        "git",
        "-c",
        "core.hooksPath=/dev/null",
        "clone",
        "-q",
        "--no-tags",
        "--branch",
        &check.branch,
        "--single-branch",
        "--",
        &url,
        &repo,
    ]));
    ctx.show_out(&cloned);
    if !cloned.ok() {
        return Err(ctx.die(&format!("could not clone {} {}", check.fork, check.branch)));
    }
    let (_, at) = g.capture(ctx, &["rev-parse", "HEAD"]);
    if at != check.head {
        let (_, short) = g.capture(ctx, &["rev-parse", "--short", "HEAD"]);
        return Err(ctx.die(&format!(
            "{} has moved since the check (now {short}, checked {}); nothing was pushed",
            check.branch, check.head
        )));
    }
    let committer = format!("{me_login} <{me_id}+{me_login}@users.noreply.github.com>");
    let email = format!("{me_id}+{me_login}@users.noreply.github.com");
    for (name, value) in [
        ("GIT_COMMITTER_NAME", me_login),
        ("GIT_COMMITTER_EMAIL", email.as_str()),
        ("GIT_AUTHOR_NAME", me_login),
        ("GIT_AUTHOR_EMAIL", email.as_str()),
    ] {
        ctx.exports.push((name.into(), value.into()));
    }

    let items: Vec<&str> =
        check.items.split([' ', '\t', '\n']).filter(|item| !item.is_empty()).collect();
    let mut rewriting = false;
    for item in &items {
        let kind = item.split(':').next().unwrap_or("");
        if kind == "rebase" {
            rewriting = true;
            let added = g.run(ctx, &["remote", "add", "base", &base_url]);
            ctx.show_both(&added);
            let fetched = g.run(ctx, &["fetch", "-q", "base", &check.base]);
            ctx.show_out(&fetched);
            if !fetched.ok() {
                return Err(
                    ctx.die(&format!("could not fetch {} from {}", check.base, settings.slug))
                );
            }
            let target = format!("refs/remotes/base/{}", check.base);
            if !g.run(ctx, &["rebase", "-q", &target]).ok() {
                drop(g.run(ctx, &["rebase", "--abort"]));
                return Err(ctx.die(&format!(
                    "the rebase onto {} conflicts. Resolving conflicts is not automated; nothing was pushed",
                    check.base
                )));
            }
        }
    }
    let nonrebase = non_rebase_kinds(ctx, &check.items);
    if !nonrebase.is_empty() {
        if check.patch.is_empty() || !Path::new(&check.patch).is_file() {
            return Err(ctx.die(&format!("no patch for: {nonrebase}")));
        }
        if !g.run(ctx, &["apply", "--check", &check.patch]).ok() {
            return Err(ctx.die(&format!(
                "the patch does not apply to {} after any rebase; nothing was pushed",
                check.branch
            )));
        }
        if !g.run(ctx, &["apply", "--index", &check.patch]).ok() {
            return Err(ctx.die("git apply failed on the patch; nothing was pushed"));
        }
        if check.items.contains("test-count") {
            check_count(ctx, settings, work, &g)?;
        }
        let Some(author) = lookup(ctx, &format!("users/{}", check.author)) else {
            return Err(ctx.die(&format!("could not look up {}", check.author)));
        };
        let (au_id, au_login) = split_account(&author);
        if !is_whole_number(au_id) {
            return Err(ctx.die(&format!("gh did not return a numeric id for {}", check.author)));
        }
        if !same_login(au_login, &check.author) {
            return Err(
                ctx.die(&format!("the API answered for '{au_login}', not {}", check.author))
            );
        }
        let (au_id, au_login) = (au_id.to_string(), au_login.to_string());
        // Captured first: piped straight into `grep -q`, the match SIGPIPEs git
        // and pipefail reports it as no match, so the sign-off came and went.
        let history = g.run(ctx, &["log", "-n", "50", "--format=%B", &check.head]);
        ctx.show_err(&history);
        if !history.ok() {
            return Err(ctx.die(&format!("could not read {}'s commit messages", check.branch)));
        }
        let signed = lines(&cmd_subst(&history.stdout))
            .into_iter()
            .any(|line| line.starts_with(b"Signed-off-by:"));
        let subject = format!("fixup: {nonrebase} from the review");
        let credit =
            format!("Co-authored-by: {au_login} <{au_id}+{au_login}@users.noreply.github.com>");
        let mut commit = vec!["commit", "-q"];
        if signed {
            commit.push("-s");
        }
        commit.extend(["-m", &subject, "-m", &credit]);
        if !g.run(ctx, &commit).ok() {
            return Err(ctx.die("could not commit the patch"));
        }
    }
    let (_, new) = g.capture(ctx, &["rev-parse", "HEAD"]);
    if new == check.head {
        return Err(ctx.die(&format!("the fixup would change nothing on {}", check.branch)));
    }

    let (_, from) = g.capture(ctx, &["rev-parse", "--short", &check.head]);
    let (_, to) = g.capture(ctx, &["rev-parse", "--short", &new]);
    ctx.say(&format!(
        "plan: {} on {} {}, {from} -> {to}, as {committer}",
        check.items, check.fork, check.branch
    ));
    if dry {
        if rewriting {
            ctx.say(&format!(
                "the branch would be rewritten (force-with-lease against {})",
                check.head
            ));
        }
        let shown = g.run(ctx, &["show", "--stat", "--patch", "--format=%B", "HEAD"]);
        ctx.show_err(&shown);
        ctx.put_out(&indent4(&shown.stdout));
        ctx.say("rehearsal: nothing was pushed");
        return Ok(0);
    }

    // A rewrite is leased against the head that was checked, so a push made
    // while this ran is never overwritten. Adding a commit is a plain push and
    // refuses by itself when the branch moved.
    let target = format!("HEAD:refs/heads/{}", check.branch);
    if rewriting {
        let lease = format!("--force-with-lease=refs/heads/{}:{}", check.branch, check.head);
        let pushed = g.run(ctx, &["push", "-q", &lease, &url, &target]);
        ctx.show_out(&pushed);
        if !pushed.ok() {
            return Err(ctx.die("the push was refused (the branch moved, or the lease failed); nothing was overwritten"));
        }
    } else {
        let pushed = g.run(ctx, &["push", "-q", &url, &target]);
        ctx.show_out(&pushed);
        if !pushed.ok() {
            return Err(ctx.die("the push was refused (the branch moved); nothing was overwritten"));
        }
    }
    confirm(ctx, settings, pr, check, &new)
}

/// `grep -v '^rebase$' | sort -u | paste -sd, - | sed 's/,/, /g'` over the kinds.
/// The sort runs through the `sort` program under the caller's locale, which
/// decides the order of kinds that are not plain ASCII words.
fn non_rebase_kinds(ctx: &mut Ctx<'_>, items: &str) -> String {
    // `tr | sed | grep | sort | paste | sed`: a host without one of these leaves the
    // list empty, so no patch is applied (decision P1H-16). That also ends the run before
    // the `grep`s of the count check (303) and of the sign-off (322) could be asked.
    let mut gone = false;
    for tool in ["tr", "grep", "sort", "paste"] {
        gone |= ctx.missing_tool(tool, 298);
    }
    if gone {
        return String::new();
    }
    let kept: Vec<String> = item_kinds(items).into_iter().filter(|kind| kind != "rebase").collect();
    if kept.is_empty() {
        return String::new();
    }
    let input = [kept.join("\n").as_bytes(), b"\n"].concat();
    let sorted = ctx.exec(&Call::new(["sort", "-u"]).stdin(input));
    let kinds: Vec<String> = lines(&sorted.stdout)
        .into_iter()
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .collect();
    String::from_utf8_lossy(&cmd_subst(join_kinds(&kinds).as_bytes())).into_owned()
}

/// The test-count check: the count comes from the profile's own command run in
/// the scratch clone, and the patch must state it.
fn check_count(ctx: &mut Ctx<'_>, settings: &Settings, work: &Path, g: &Clone) -> R<()> {
    let command = settings.tunable(&ctx.vars, "FIXUP_COUNT_CMD").unwrap_or_default();
    if command.is_empty() {
        return Err(ctx.die("this profile declares no FIXUP_COUNT_CMD, so a test count would be typed rather than derived"));
    }
    let run = ctx.exec(
        &Call::new([
            "env",
            "-u",
            "GH_TOKEN",
            "-u",
            "GITHUB_TOKEN",
            "timeout",
            "120",
            "bash",
            "-c",
            &command,
        ])
        .cwd(Some(&work.join("r")))
        .timeout(Duration::from_secs(150)),
    );
    // `2>/dev/null | tr -d '[:space:]'`: the status of the pipeline is tr's.
    let digits: Vec<u8> = run
        .stdout
        .iter()
        .copied()
        .filter(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
        .filter(|byte| *byte != 0)
        .collect();
    let n = String::from_utf8_lossy(&digits).into_owned();
    if !is_whole_number(&n) {
        return Err(
            ctx.die("FIXUP_COUNT_CMD did not print a whole number, so no count could be derived")
        );
    }
    let staged = g.run(ctx, &["diff", "--cached", "-U0"]);
    ctx.show_err(&staged);
    if !staged.ok() {
        return Err(ctx.die("could not read the staged patch"));
    }
    if ctx.missing_tool("awk", 311) || !patch_states_count(&cmd_subst(&staged.stdout), &n) {
        return Err(ctx.die(&format!(
            "the patch does not state {n}, the count FIXUP_COUNT_CMD derives; a typed count is refused"
        )));
    }
    Ok(())
}

/// Success is what the pull request reports, not what the push returned.
fn confirm(ctx: &mut Ctx<'_>, settings: &Settings, pr: &str, check: &Check, new: &str) -> R<i32> {
    // `tries="$GH_TRIES"` under `set -u`: a profile that unsets it ends a script file with 1,
    // after the push (decision P1H-18).
    let Some(raw_tries) = settings.tunable(&ctx.vars, "GH_TRIES") else {
        return Err(unbound(ctx, "GH_TRIES", 349));
    };
    let tries = bash_integer(&raw_tries);
    let backoff = settings.tunable(&ctx.vars, "GH_BACKOFF");
    let mut got = String::new();
    let mut i = 1_i64;
    while tries.is_some_and(|limit| i <= limit) {
        let viewed = ctx.exec(&Call::new([
            "gh",
            "pr",
            "view",
            pr,
            "--repo",
            &settings.slug,
            "--json",
            "headRefOid",
            "--jq",
            ".headRefOid",
        ]));
        got = String::from_utf8_lossy(&cmd_subst(&viewed.stdout)).into_owned();
        if got == new {
            break;
        }
        // `[ "$i" -lt "$tries" ]` reads the text as a decimal number, so "010" is
        // ten here and eight in the loop's arithmetic.
        if decimal_integer(&raw_tries).is_some_and(|limit| i < limit) {
            let Some(backoff) = &backoff else {
                return Err(unbound(ctx, "GH_BACKOFF", 353));
            };
            let wait = backoff.trim().parse::<u64>().unwrap_or(0);
            let slept = ctx.exec(
                &Call::new(["sleep", backoff])
                    .at(353)
                    .timeout(Duration::from_secs(wait.saturating_add(60))),
            );
            ctx.show_err(&slept);
        }
        i += 1;
    }
    if got != new {
        let seen = if got.is_empty() { "nothing" } else { got.as_str() };
        return Err(ctx.die(&format!(
            "pushed {new} but #{pr} reports {seen} as its head: NOT confirmed, look at it before anything else"
        )));
    }
    ctx.say(&format!(
        "fixup pushed to {} {}; #{pr} is now at {new} (read back from GitHub)",
        check.fork, check.branch
    ));
    Ok(0)
}
