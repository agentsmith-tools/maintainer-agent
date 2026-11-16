//! `maintainer-repo release-check`: is a release due, and which digit moves.
//!
//! The report reads the commit range `LAST_TAG..origin/main` with `git`, counts
//! files under the profile's `PROD_GLOBS` and `SHIPPED_GLOBS`, and reads the
//! `CHANGELOG.md` of the *working tree*, which is the file a human is about to
//! tag. The text of every line is the bash's.
//!
//! The CHANGELOG logic is pure ([`unreleased_section`], [`ChangelogFacts`],
//! [`verdict`]) and is tested without a process. It does what the bash's
//! `sed | grep` pipeline does: the Unreleased section runs from a line that
//! starts `## [Unreleased]` to the next line that starts `## [` and a digit,
//! every `## [` line is dropped, and counts are of lines.
//!
//! Kept as the bash has it, each pinned by a test:
//!
//! - A `Security` entry with no production file says a release is optional and
//!   still names the middle digit: the digit rule does not read the production
//!   count.
//! - The wording count is of *lines*, so two phrases on one line count once.
//! - A failed tag fetch is not checked: git's error goes to standard error and
//!   the report is built from the refs on disk.
//! - A failed `git rev-list --count` leaves the count empty and the report goes
//!   on, printing `commits since: ` with nothing after it.
//! - The wording match is ASCII case-insensitive. `grep -i` under a UTF-8
//!   locale also folds a few non-ASCII letters (the Kelvin sign for `k`, the
//!   long s for `s`); this port does not (decision P1H-9). A CHANGELOG with a
//!   NUL byte makes `grep -v` print `Binary file ... matches` where this port
//!   reads the lines (decision P1H-9).

use crate::ctx::{Call, Ctx, R};
use crate::shellish::{cmd_subst, indent4, lines, read_words};
use crate::{Settings, enter_repo};
use std::io::BufRead;

/// The longest line the scan keeps. A longer line is cut to this length: the
/// patterns the scan looks for sit at the start of a line or are short phrases
/// (decision P1H-15), and a CHANGELOG is read with a buffer of this size, not whole.
pub const MAX_LINE: usize = 1 << 20;

/// What the report reads out of the CHANGELOG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangelogFacts {
    /// Lines of the Unreleased section that start `### Security`.
    pub security: usize,
    /// Lines that start `### Added`.
    pub added: usize,
    /// Lines that hold removed-capability wording.
    pub breaking: usize,
    /// The section holds only white space, or there is no CHANGELOG.
    pub empty: bool,
    /// The first line that starts `## [` and a digit, read from the whole file.
    pub newest_heading: Option<String>,
}

/// The scan the bash's `sed -n '/^## \[Unreleased\]/,/^## \[[0-9]/p' | grep -v '^## \['` and
/// the greps after it make, one line at a time: the Unreleased section runs from
/// a line that starts `## [Unreleased]` to the next line that starts `## [` and
/// a digit, and can open again later.
struct Scan {
    inside: bool,
    security: usize,
    added: usize,
    breaking: usize,
    empty: bool,
    newest_heading: Option<String>,
    kept: Option<Vec<u8>>,
}

impl Scan {
    fn new(collect: bool) -> Self {
        Self {
            inside: false,
            security: 0,
            added: 0,
            breaking: 0,
            empty: true,
            newest_heading: None,
            kept: collect.then(Vec::new),
        }
    }

    /// One line without its newline.
    fn feed(&mut self, line: &[u8]) {
        if self.newest_heading.is_none() && is_version_heading(line) {
            self.newest_heading = Some(String::from_utf8_lossy(&cmd_subst(line)).into_owned());
        }
        let in_section = if self.inside {
            if is_version_heading(line) {
                self.inside = false;
            }
            true
        } else if line.starts_with(b"## [Unreleased]") {
            self.inside = true;
            true
        } else {
            false
        };
        if !in_section || line.starts_with(b"## [") {
            return;
        }
        // `$(...)` drops NUL bytes from the whole section.
        let text: Vec<u8> = line.iter().copied().filter(|byte| *byte != 0).collect();
        if text.starts_with(b"### Security") {
            self.security += 1;
        }
        if text.starts_with(b"### Added") {
            self.added += 1;
        }
        if has_removed_wording(&text) {
            self.breaking += 1;
        }
        if !text.iter().all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')) {
            self.empty = false;
        }
        if let Some(kept) = &mut self.kept {
            kept.extend_from_slice(&text);
            kept.push(b'\n');
        }
    }

    fn facts(self) -> ChangelogFacts {
        ChangelogFacts {
            security: self.security,
            added: self.added,
            breaking: self.breaking,
            empty: self.empty,
            newest_heading: self.newest_heading,
        }
    }
}

/// Reads one line into `buf` (without the newline), keeping at most [`MAX_LINE`]
/// bytes and discarding the rest of a longer one. `Ok(false)` at end of input.
fn read_line_bounded(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> std::io::Result<bool> {
    buf.clear();
    let mut seen_any = false;
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(seen_any);
        }
        seen_any = true;
        let (take, done) = match chunk.iter().position(|byte| *byte == b'\n') {
            Some(at) => (at, true),
            None => (chunk.len(), false),
        };
        let room = MAX_LINE.saturating_sub(buf.len());
        buf.extend_from_slice(&chunk[..take.min(room)]);
        reader.consume(take + usize::from(done));
        if done {
            return Ok(true);
        }
    }
}

/// Reads the facts from a CHANGELOG stream with a bounded buffer. A read error
/// ends the scan with what was counted so far, as `sed` prints what it read
/// before its error.
pub fn changelog_facts_from(mut reader: impl BufRead) -> ChangelogFacts {
    let mut scan = Scan::new(false);
    let mut line = Vec::new();
    while let Ok(true) = read_line_bounded(&mut reader, &mut line) {
        scan.feed(&line);
    }
    scan.facts()
}

/// The Unreleased section of a CHANGELOG, as the bash leaves it after `$(...)`.
#[must_use]
pub fn unreleased_section(changelog: &[u8]) -> Vec<u8> {
    let mut scan = Scan::new(true);
    for line in lines(changelog) {
        scan.feed(line);
    }
    cmd_subst(&scan.kept.unwrap_or_default())
}

/// `^## \[[0-9]`.
fn is_version_heading(line: &[u8]) -> bool {
    line.strip_prefix(b"## [").is_some_and(|rest| rest.first().is_some_and(u8::is_ascii_digit))
}

/// Reads the facts from a CHANGELOG held in memory. `None` is a file that could
/// not be read: the bash sends `sed`'s and `grep`'s errors to /dev/null and goes
/// on as if the section were empty.
#[must_use]
pub fn changelog_facts(changelog: Option<&[u8]>) -> ChangelogFacts {
    changelog_facts_from(std::io::Cursor::new(changelog.unwrap_or(b"")))
}

/// `grep -iE 'no longer|used to (succeed|work)|refuses'` on one line.
fn has_removed_wording(line: &[u8]) -> bool {
    let lower = line.to_ascii_lowercase();
    [&b"no longer"[..], b"used to succeed", b"used to work", b"refuses"]
        .iter()
        .any(|phrase| lower.windows(phrase.len()).any(|window| window == *phrase))
}

/// The two verdict lines of the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// No production file changed.
    DocsOnly,
    /// A Security entry or removed-capability wording is unreleased.
    ReleaseDue,
    /// Production code changed.
    Worthwhile,
}

/// The verdict: the production count first, then a Security entry or removed
/// wording, else worthwhile.
#[must_use]
pub fn verdict(prod: usize, security: usize, breaking: usize) -> Verdict {
    if prod == 0 {
        Verdict::DocsOnly
    } else if security > 0 || breaking > 0 {
        Verdict::ReleaseDue
    } else {
        Verdict::Worthwhile
    }
}

/// Which digit moves: the middle one when a Security entry or removed wording
/// is present, whatever the production count says.
#[must_use]
pub fn digit_is_middle(security: usize, breaking: usize) -> bool {
    breaking > 0 || security > 0
}

fn yes_no(count: usize) -> &'static str {
    if count > 0 { "yes" } else { "no" }
}

/// The lines after the empty line that follows the counts.
#[must_use]
pub fn closing_lines(prod: usize, facts: &ChangelogFacts) -> Vec<String> {
    let mut out = Vec::new();
    match verdict(prod, facts.security, facts.breaking) {
        Verdict::DocsOnly => {
            out.push("verdict: docs and tests only. A release is optional.".into());
        }
        Verdict::ReleaseDue => {
            out.push("verdict: RELEASE DUE. A fix that removes a capability, or a security".into());
            out.push(
                "         fix, is sitting unreleased, so every installed copy is still".into(),
            );
            out.push("         affected.".into());
        }
        Verdict::Worthwhile => {
            out.push("verdict: release worthwhile; production code changed.".into());
        }
    }
    if digit_is_middle(facts.security, facts.breaking) {
        out.push("digit:   middle. Something that used to succeed now refuses, which".into());
        out.push("         docs/release.md counts as a break in the 0.y series.".into());
    } else {
        out.push("digit:   last. Compatible additions and fixes.".into());
    }
    out.push(String::new());
    out.push("Cutting one is a human decision: crates.io and npm versions can never be".into());
    out.push("replaced.".into());
    out
}

/// `git diff --name-only RANGE -- GLOBS | wc -l`, with git's error let through.
#[allow(
    clippy::naive_bytecount,
    reason = "a path listing is short, and a new dependency for it is not worth it"
)]
fn count_paths(ctx: &mut Ctx<'_>, range: &str, globs: &[String]) -> usize {
    let mut argv: Vec<String> =
        ["git", "diff", "--name-only", range, "--"].map(String::from).to_vec();
    argv.extend(globs.iter().cloned());
    let done = ctx.exec(&Call::new(argv).cwd(ctx.cwd.as_deref()));
    ctx.show_err(&done);
    done.stdout.iter().filter(|byte| **byte == b'\n').count()
}

/// `$(git ARGS)` with git's standard error let through: the stdout without trailing newlines.
fn capture(ctx: &mut Ctx<'_>, args: &[&str]) -> (bool, String) {
    let done = ctx.git(args);
    ctx.show_err(&done);
    (done.ok(), String::from_utf8_lossy(&cmd_subst(&done.stdout)).into_owned())
}

/// Which tree the CHANGELOG comes from, and whether the counts and the file can disagree.
fn changelog_source(ctx: &mut Ctx<'_>) {
    let (_, branch) = capture(ctx, &["rev-parse", "--abbrev-ref", "HEAD"]);
    ctx.say(&format!("CHANGELOG read from:        the working tree ({branch})"));
    let (_, status) = capture(ctx, &["status", "--porcelain", "CHANGELOG.md"]);
    if !status.is_empty() {
        ctx.say("                           uncommitted, so origin/main does not have it yet");
        return;
    }
    let ancestor = ctx.git(&["merge-base", "--is-ancestor", "HEAD", "origin/main"]);
    ctx.show_out(&ancestor);
    if !ancestor.ok() {
        ctx.say("                           HEAD is not on origin/main; the counts above are");
        return;
    }
    // Being an ancestor is exactly the stale case.
    let behind = ctx.git(&["rev-list", "--count", "HEAD..origin/main"]);
    let text = if behind.ok() {
        String::from_utf8_lossy(&cmd_subst(&behind.stdout)).into_owned()
    } else {
        String::from_utf8_lossy(&cmd_subst(&[behind.stdout.as_slice(), b"0"].concat())).into_owned()
    };
    let text = if text.is_empty() { "0".to_string() } else { text };
    if crate::shellish::bash_integer(&text).is_some_and(|count| count > 0) {
        ctx.say(&format!(
            "                           HEAD is {text} commit(s) BEHIND origin/main, so this"
        ));
        ctx.say("                           is NOT the CHANGELOG you are about to tag. Pull, then");
        ctx.say("                           run this again; the digit below may be wrong.");
    }
}

/// The counts, the warnings and the verdict.
fn print_report(
    ctx: &mut Ctx<'_>,
    tag: &str,
    prod: usize,
    shipped: Option<usize>,
    facts: &ChangelogFacts,
) {
    ctx.say(&format!("production files touched:  {prod}"));
    if let Some(count) = shipped {
        ctx.say(&format!("shipped surface touched:    {count}"));
        if count == 0 {
            ctx.say(&format!(
                "                           nothing that installs has changed since {tag}, so a"
            ));
            ctx.say(
                "                           release would publish byte-identical artifacts and",
            );
            ctx.say(
                "                           any break named below is in CI, not in the product.",
            );
        }
    }
    ctx.say(&format!("CHANGELOG Security section: {}", yes_no(facts.security)));
    ctx.say(&format!("CHANGELOG Added section:    {}", yes_no(facts.added)));
    ctx.say(&format!("removed-capability wording: {} occurrence(s)", facts.breaking));
    if facts.empty {
        ctx.say("WARNING: the Unreleased section is empty, so the three lines above read");
        ctx.say("         nothing and this verdict rests on file counts alone.");
        if let Some(top) = &facts.newest_heading {
            let heading = top.strip_prefix("## ").unwrap_or(top);
            ctx.say(&format!(
                "         The newest version heading is '{heading}'. If you have already"
            ));
            ctx.say("         promoted Unreleased into it, read that section instead.");
        }
    }
    ctx.put_out(b"\n");
    for line in closing_lines(prod, facts) {
        ctx.say(&line);
    }
}

/// `maintainer-repo release-check`.
pub fn cmd_release_check(ctx: &mut Ctx<'_>, settings: &Settings) -> R<i32> {
    enter_repo(ctx, settings, 74)?;
    let fetch = ctx.git(&["fetch", "-q", "--tags", "origin"]);
    ctx.show_both(&fetch);
    let described = ctx.git(&["describe", "--tags", "--abbrev=0", "origin/main"]);
    if !described.ok() {
        return Err(ctx.die("no tags"));
    }
    let tag = String::from_utf8_lossy(&cmd_subst(&described.stdout)).into_owned();
    let range = format!("{tag}..origin/main");
    let (_, n) = capture(ctx, &["rev-list", "--count", &range]);
    ctx.say(&format!("last tag: {tag}"));
    ctx.say(&format!("commits since: {n}"));
    if n == "0" {
        ctx.say("verdict: nothing to release");
        return Ok(0);
    }

    ctx.put_out(b"\n");
    let log = ctx.git(&["log", "--oneline", &range]);
    ctx.show_err(&log);
    ctx.put_out(&indent4(&log.stdout));
    ctx.put_out(b"\n");

    let prod_globs = read_words(&settings.tunable(&ctx.vars, "PROD_GLOBS").unwrap_or_default());
    let prod = if prod_globs.is_empty() {
        ctx.say("WARNING: this profile declares no PROD_GLOBS, so the production-file");
        ctx.say("         count below is meaningless. Set it in profile.env.");
        0
    } else {
        count_paths(ctx, &range, &prod_globs)
    };
    let shipped_globs =
        read_words(&settings.tunable(&ctx.vars, "SHIPPED_GLOBS").unwrap_or_default());
    let shipped = if shipped_globs.is_empty() {
        ctx.say("WARNING: this profile declares no SHIPPED_GLOBS, so nothing below can say");
        ctx.say("         whether the change reaches an installed copy. Set it in");
        ctx.say("         profile.env. Unset is not the same as untouched.");
        None
    } else {
        Some(count_paths(ctx, &range, &shipped_globs))
    };

    changelog_source(ctx);
    let facts =
        changelog_facts(std::fs::read(settings.repo_path.join("CHANGELOG.md")).ok().as_deref());
    print_report(ctx, &tag, prod, shipped, &facts);
    Ok(0)
}
