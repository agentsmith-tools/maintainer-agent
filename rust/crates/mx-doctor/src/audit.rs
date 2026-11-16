//! The Audit trail and Deployed code sections.

use crate::ctx::{Ctx, Out, Stop, limit};
use crate::host::{glob_names, ls_count};
use crate::pyval;
use crate::report::Item;
use crate::text;
use mx_core::Fact;
use serde_json::Value;

pub use crate::pyval::Crash;

/// One lease file as the doctor read it.
#[derive(Debug, Clone)]
pub struct LeaseFile {
    pub name: String,
    pub contents: Fact<Vec<u8>>,
}

/// The lease reader: `float(HOURS)` first, then each `pr-*.json` in order. A lease
/// that cannot be read prints `#PR (unreadable)`; one past the limit prints its age.
pub fn forgotten_leases(hours: &str, files: &[LeaseFile], now: i64) -> Result<Vec<String>, Crash> {
    let limit = text::py_float(hours).ok_or(Crash)? * 3600.0;
    let mut lines = Vec::new();
    for file in files {
        let pr = file.name.get(3..file.name.len() - 5).unwrap_or("");
        let unreadable = || format!("#{pr} (unreadable)");
        let Fact::Known(bytes) = &file.contents else {
            lines.push(unreadable());
            continue;
        };
        let Ok(value) = pyval::parse(bytes) else {
            lines.push(unreadable());
            continue;
        };
        let renewed = match value.as_object().and_then(|object| object.get("renewed")) {
            Some(Value::Number(number)) => {
                let text = number.to_string();
                let as_float = text.parse::<f64>().map_err(|_| Crash)?;
                if !as_float.is_finite() {
                    return Err(Crash);
                }
                Some(as_float.trunc())
            }
            Some(Value::String(text)) => text::py_int_text(text),
            Some(Value::Bool(flag)) => Some(f64::from(u8::from(*flag))),
            _ => None,
        };
        let Some(renewed) = renewed else {
            lines.push(unreadable());
            continue;
        };
        #[allow(clippy::cast_precision_loss)]
        let age = now as f64 - renewed;
        if age > limit {
            lines.push(format!("#{pr} (renewed {}h ago)", text::round0(age / 3600.0)));
        }
    }
    Ok(lines)
}

/// The findings for the forgotten leases the reader returned.
#[must_use]
pub fn decide_leases(found: &[String], hours: &str) -> Item {
    if found.is_empty() {
        return Item::ok(format!("no lease has gone {hours}h without a renewal"));
    }
    let shown = found.join(",").replace(',', ", ");
    // `sed -n 's/^#\([0-9]*\) .*/\1/p'`, then `tr '\n' ' '`.
    let mut numbers = String::new();
    for line in found {
        if let Some(rest) = line.strip_prefix('#') {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if rest.as_bytes().get(digits) == Some(&b' ') {
                numbers.push_str(&rest[..digits]);
                numbers.push(' ');
            }
        }
    }
    Item::warn(
        format!("lease(s) nobody has renewed for over {hours}h: {shown}"),
        format!("maintainer lease release {numbers}"),
    )
}

/// What a failed-task record reads: the reason Python would print.
#[must_use]
pub fn failure_reason(contents: &Fact<Vec<u8>>) -> String {
    let Fact::Known(bytes) = contents else { return String::new() };
    let Ok(value) = pyval::parse(bytes) else { return String::new() };
    let Some(object) = value.as_object() else { return String::new() };
    match object.get("reason") {
        Some(found) if pyval::truthy(found) => text::captured(pyval::py_str(found).as_bytes()),
        _ => String::new(),
    }
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// The age in whole hours the bash computes: integer division truncating toward zero.
fn hours_between(now: i64, then: i64) -> i64 {
    (now - then) / 3600
}

/// Audit trail.
pub fn audit_trail(ctx: &Ctx<'_>, out: &mut Out) -> Result<(), Stop> {
    out.add(Item::head("Audit trail"));
    let st = ctx.settings.get("STATE_DIR").to_string();
    let profile = ctx.profile;
    if st.is_empty() {
        out.add(Item::fail(
            "the profile names no STATE_DIR",
            format!("set it in profiles/{profile}/profile.env"),
        ));
        return Ok(());
    }
    if !ctx.host.stat(&st).is_dir() {
        out.add(Item::warn("no state yet; the first run creates it", ""));
        return Ok(());
    }
    let runs = ls_count(ctx.host, &format!("{st}/runs"));
    let receipts = ls_count(ctx.host, &format!("{st}/receipts"));
    out.add(Item::ok(format!("{runs} run report(s), {receipts} open receipt(s)")));
    receipts_check(ctx, out, &st);
    leases_check(ctx, out, &st)?;
    if ctx.host.stat(&st).writable {
        out.add(Item::ok("state directory writable"));
    } else {
        out.add(Item::fail("state directory not writable", "chown it"));
    }
    token_check(ctx, out, &st);
    tasks_check(ctx, out, &st)
}

/// A receipt whose pull request already merged is not open, it is stale.
fn receipts_check(ctx: &Ctx<'_>, out: &mut Out, st: &str) {
    let mut stale = String::new();
    for name in glob_names(ctx.host, &format!("{st}/receipts"), "", ".json") {
        if !ctx.host.stat(&format!("{st}/receipts/{name}")).exists() {
            continue;
        }
        let pr = name.strip_suffix(".json").unwrap_or(&name);
        if pr.is_empty() || !pr.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let slug = ctx.settings.get("REPO_SLUG");
        let Some(call) = ctx
            .run(
                &["gh", "pr", "view", pr, "--repo", slug, "--json", "state", "--jq", ".state"],
                limit::GH,
            )
            .filter(crate::ctx::Call::ok)
        else {
            continue;
        };
        let verdict = call.text();
        if verdict == "MERGED" || verdict == "CLOSED" {
            stale = format!("{stale} {pr} ({verdict})");
        }
    }
    if stale.is_empty() {
        out.add(Item::ok("no receipt outlives its pull request"));
    } else {
        out.add(Item::warn(
            format!("receipt(s) for an already merged pull request:{stale}"),
            format!("rm {st}/receipts/<pr>.json once you have checked the work really landed"),
        ));
    }
}

/// A lease nobody released and nobody renewed. It blocks nothing once it has
/// expired, so this is housekeeping, but a lease that old means a session ended
/// without letting go, and a file the doctor cannot read is not a pass.
fn leases_check(ctx: &Ctx<'_>, out: &mut Out, st: &str) -> Result<(), Stop> {
    let dir = format!("{st}/leases");
    let mut found = Vec::new();
    let hours;
    if ctx.host.stat(&dir).is_dir() {
        hours = ctx.settings.need("DOCTOR_LEASE_HOURS", 440, out)?.to_string();
        let files: Vec<LeaseFile> = glob_names(ctx.host, &dir, "pr-", ".json")
            .into_iter()
            .map(|name| {
                let contents = ctx.host.read(&format!("{dir}/{name}"));
                LeaseFile { name, contents }
            })
            .collect();
        let now = i64::try_from(ctx.clock.now_unix()).unwrap_or(0);
        match forgotten_leases(&hours, &files, now) {
            Ok(lines) => found = lines,
            Err(Crash) => out.add(Item::warn(
                format!("could not read the leases under {dir}, so forgotten ones are unknown"),
                "python3 and a readable state directory",
            )),
        }
    } else {
        hours = ctx.settings.need("DOCTOR_LEASE_HOURS", 461, out)?.to_string();
    }
    out.add(decide_leases(&found, &hours));
    Ok(())
}

/// A pinned token that outlived the run that used it. run.sh scrubs on exit, so
/// anything found here was written by a build older than that trap, by a run that
/// was killed before its trap fired, or into a file the scrubber will not rewrite.
///
/// Decision F4: the token is searched for in this process. It never reaches the
/// argument vector or the environment of a child.
fn token_check(ctx: &Ctx<'_>, out: &mut Out, st: &str) {
    let account = ctx.settings.get("GH_ACCOUNT");
    let token = ctx
        .run(&["gh", "auth", "token", "--user", account], limit::GH)
        .map(|call| text::strip_trailing_newlines(&call.stdout).to_vec())
        .unwrap_or_default();
    if token.len() < 16 {
        out.add(Item::warn(
            "cannot read the pinned token, so nothing checked whether one was left on disk",
            format!("gh auth switch --user {}", ctx.settings.get_or("GH_ACCOUNT", "<account>")),
        ));
        return;
    }
    let repo = ctx.settings.get_or("REPO_PATH", "/nonexistent");
    let session =
        format!("{}/.claude/projects/{}", ctx.env.home, text::dashes_for_slash_and_dot(repo));
    // Named subdirectories rather than $st, because the state directory also holds
    // container build trees: gigabytes of object files that a doctor run must not
    // walk, and that no token is ever written into.
    let mut places = String::new();
    for sub in ["logs", "runs", "drafts", "state", "receipts"] {
        if ctx.host.stat(&format!("{st}/{sub}")).is_dir() {
            places = format!("{places} {st}/{sub}");
        }
    }
    if ctx.host.stat(&session).is_dir() {
        places = format!("{places} {session}");
    }
    if places.is_empty() {
        places = st.to_string();
    }
    let roots: Vec<String> = text::ifs_words(&places).into_iter().map(str::to_string).collect();
    // `grep -F -e "$token"`: a token with newlines is one pattern per line.
    let patterns: Vec<Vec<u8>> = token.split(|byte| *byte == b'\n').map(<[u8]>::to_vec).collect();
    let found = ctx.host.scan_for(&roots, &patterns);
    let first_five: Vec<&String> = found.iter().take(5).collect();
    if let Some(first) = first_five.first() {
        out.add(Item::fail(
            format!(
                "the pinned token is on disk in {} file(s), starting {first}",
                first_five.len()
            ),
            format!(
                "rotate the token, then: {}/run.sh {} <task> rescrubs on exit",
                ctx.share, ctx.profile
            ),
        ));
    } else {
        out.add(Item::ok("no pinned token left on disk by an earlier run"));
    }
}

/// A task that could not start leaves failed-<task>.json, and `finish` promotes
/// last-<task>.json only on success. So "the newest failure is newer than the
/// newest success" is exactly the condition for a task that has stopped running.
fn tasks_check(ctx: &Ctx<'_>, out: &mut Out, st: &str) -> Result<(), Stop> {
    let dir = format!("{st}/state");
    let mut dead = 0u32;
    for name in glob_names(ctx.host, &dir, "failed-", ".json") {
        let path = format!("{dir}/{name}");
        let failed = ctx.host.stat(&path);
        if !failed.exists() {
            continue;
        }
        let task = name.strip_prefix("failed-").and_then(|n| n.strip_suffix(".json")).unwrap_or("");
        let last_path = format!("{dir}/last-{task}.json");
        let last = ctx.host.stat(&last_path);
        // -nt is false when they share a timestamp, so a tie counts as recovered.
        if last.exists() && failed.mtime <= last.mtime {
            continue;
        }
        dead += 1;
        let now = |ctx: &Ctx<'_>| i64::try_from(ctx.clock.now_unix()).unwrap_or(0);
        let age = hours_between(now(ctx), failed.mtime.0);
        // An unreadable reason must not silence the report: the failure is the
        // finding, the reason is detail.
        let mut why = failure_reason(&ctx.host.read(&path));
        if why.is_empty() {
            why = format!("no reason recorded in {name}");
        }
        let run_fix = format!("{}/run.sh {} {task}", ctx.share, ctx.profile);
        if last.exists() {
            // "Has this task stopped?" is the question, and the answer is no while
            // its last success is younger than its own MIN_HOURS.
            let since = hours_between(now(ctx), last.mtime.0);
            let variable = format!("MIN_HOURS_{task}");
            if !is_identifier(&variable) {
                out.warn_text(&format!(
                    "maintainer-doctor: line 530: {variable}: invalid variable name\n"
                ));
                return Err(Stop::AbortSection);
            }
            let min = ctx.settings.get_or(&variable, "0").to_string();
            let within = if let Some(min) = text::test_int(&min) {
                min > 0 && since < min
            } else {
                out.warn_text(&text::integer_expected(531, &min));
                false
            };
            if within {
                out.add(Item::warn(
                    format!("{task} failed {age}h ago, but it completed {since}h ago and its interval is {min}h: {why}"),
                    "nothing is overdue; read the log if you want to know why that attempt died",
                ));
            } else {
                out.add(Item::fail(
                    format!("{task} failed {age}h ago and has not completed since: {why}"),
                    run_fix,
                ));
            }
        } else {
            out.add(Item::fail(
                format!(
                    "{task} has never completed; its only record is a failure {age}h ago: {why}"
                ),
                run_fix,
            ));
        }
    }
    if dead == 0 {
        out.add(Item::ok("every task has completed since its last failure"));
    }
    Ok(())
}

/// What VERSION and the tree it names say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployFacts {
    pub commit: String,
    pub from: String,
    /// False when the source tree has no `.git` directory.
    pub from_is_repo: bool,
    pub head: String,
    pub behind: String,
    pub dirty: usize,
}

/// The finding for the deployed code.
#[must_use]
pub fn decide_deployed(facts: &DeployFacts) -> Item {
    if facts.commit.is_empty() || facts.from.is_empty() {
        return Item::warn(
            "VERSION does not record what was installed or where from",
            "reinstall with ./install.sh so the stamp is written",
        );
    }
    if !facts.from_is_repo {
        return Item::warn(
            format!("the tree it was installed from is gone: {}", facts.from),
            "reinstall from a checkout so drift can be measured",
        );
    }
    let same = facts.commit == facts.head;
    if same && facts.dirty == 0 {
        Item::ok(format!("deployed code is that tree's HEAD ({})", facts.commit))
    } else if same {
        Item::warn(
            format!(
                "deployed matches HEAD, but {} has {} uncommitted change(s)",
                facts.from, facts.dirty
            ),
            "commit them, then ./install.sh, or the next run uses neither version",
        )
    } else {
        Item::fail(
            format!(
                "deployed {} is {} commit(s) behind {} ({}); every timer is running old code",
                facts.commit, facts.behind, facts.from, facts.head
            ),
            format!("cd {} && ./install.sh", facts.from),
        )
    }
}

/// The values `sed -n 's/^KEY=//p'` prints from a file's lines.
fn stamped(bytes: &[u8], key: &str) -> String {
    let prefix = format!("{key}=");
    let found: Vec<&[u8]> =
        text::lines(bytes).filter_map(|line| line.strip_prefix(prefix.as_bytes())).collect();
    text::captured(&found.join(&b'\n'))
}

/// Deployed code. The timers run what is in ~/.local/bin, not what is in the
/// working tree, so a merge nobody reinstalled leaves every unattended run on old code.
pub fn deployed_code(ctx: &Ctx<'_>, out: &mut Out) {
    out.add(Item::head("Deployed code"));
    let bytes = ctx.host.read(&format!("{}/VERSION", ctx.share)).known().unwrap_or_default();
    let commit = stamped(&bytes, "commit");
    let from = stamped(&bytes, "installed_from");
    let mut facts = DeployFacts {
        commit,
        from,
        from_is_repo: false,
        head: String::new(),
        behind: String::new(),
        dirty: 0,
    };
    if !facts.commit.is_empty() && !facts.from.is_empty() {
        facts.from_is_repo = ctx.host.stat(&format!("{}/.git", facts.from)).is_dir();
        if facts.from_is_repo {
            let from = facts.from.clone();
            facts.head = ctx
                .run(&["git", "-C", &from, "rev-parse", "--short", "HEAD"], limit::GIT)
                .map(|call| call.text())
                .unwrap_or_default();
            let range = format!("{}..HEAD", facts.commit);
            facts.behind =
                match ctx.run(&["git", "-C", &from, "rev-list", "--count", &range], limit::GIT) {
                    Some(call) if call.ok() => call.text(),
                    _ => "?".to_string(),
                };
            facts.dirty = ctx
                .run(&["git", "-C", &from, "status", "--porcelain"], limit::GIT)
                .map_or(0, |call| text::newlines(&call.stdout));
        }
    }
    out.add(decide_deployed(&facts));
}
