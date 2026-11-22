//! GitHub identity, Posting, Repository and the workflow runs on main.

use crate::ctx::{Ctx, Out, Stop, limit};
use crate::pyval::{self, Crash};
use crate::report::Item;
use crate::text;
use serde_json::Value;

/// GitHub identity.
pub fn identity(ctx: &Ctx<'_>, out: &mut Out) {
    out.add(Item::head("GitHub identity"));
    // Lesson 9: the active account changes under you, and a write under the
    // wrong one is worse than a refusal.
    if ctx.command_v("gh").is_none() {
        out.add(Item::fail("gh not installed", "install the GitHub CLI"));
        return;
    }
    // Only a zero exit makes stdout a login: on a 401 gh prints GitHub's JSON
    // error body there, and this used to report that body as the account.
    let (who, who_err) = match ctx.run(&["gh", "api", "user", "--jq", ".login"], limit::GH) {
        Some(call) if call.ok() => (call.text(), String::new()),
        Some(call) => (String::new(), call.text()),
        None => (String::new(), String::new()),
    };
    let want = ctx.settings.get("GH_ACCOUNT");
    if want.is_empty() {
        out.add(Item::fail(
            "the profile names no GH_ACCOUNT",
            "set it in profile.env; a run refuses to start without one",
        ));
    } else if who_err.contains("Bad credentials") {
        out.add(Item::fail(
            format!("GitHub rejects the stored token for {want} (HTTP 401)"),
            format!("gh auth login -h github.com, signed in as {want}"),
        ));
    } else if who.eq_ignore_ascii_case(want) {
        out.add(Item::ok(format!("gh is {who}")));
    } else if who.is_empty() {
        out.add(Item::fail("gh is not authenticated", "gh auth login"));
    } else {
        out.add(Item::warn(
            format!("gh is '{who}', not '{want}'"),
            "run.sh switches automatically, but check ~/.config/gh/hosts.yml",
        ));
    }
}

/// Posting.
pub fn posting(ctx: &Ctx<'_>, out: &mut Out) {
    out.add(Item::head("Posting"));
    if ctx.settings.get_or("POST", "on").contains("off") {
        out.add(Item::ok("POST=off: this profile writes reports and drafts, and posts nothing"));
    } else {
        out.add(Item::warn(
            format!(
                "POST=on: this profile writes to GitHub as {}",
                ctx.settings.get_or("GH_ACCOUNT", "?")
            ),
            "set POST=off in profile.env for a rehearsal week",
        ));
    }
}

/// What `git -C R status --porcelain | wc -l` and the ahead count read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoFacts {
    pub dirty: usize,
    pub ahead: String,
}

/// The decisions of the Repository section for a repository that exists.
#[must_use]
pub fn decide_repo(facts: &RepoFacts) -> Vec<Item> {
    let mut items = Vec::new();
    if facts.dirty == 0 {
        items.push(Item::ok("working tree clean"));
    } else {
        items.push(Item::warn(
            format!("{} uncommitted change(s); a run will refuse to start", facts.dirty),
            "commit or stash",
        ));
    }
    if facts.ahead == "0" {
        items.push(Item::ok("not ahead of origin/main"));
    } else {
        items.push(Item::warn(
            format!("{} commit(s) ahead of origin/main", facts.ahead),
            "push or reset",
        ));
    }
    items
}

/// Repository, then the run list for main.
pub fn repository(ctx: &Ctx<'_>, out: &mut Out) -> Result<(), Stop> {
    out.add(Item::head("Repository"));
    let r = ctx.settings.get("REPO_PATH");
    if r.is_empty() {
        out.add(Item::fail(
            "the profile names no REPO_PATH",
            format!("set it in profiles/{}/profile.env", ctx.profile),
        ));
    } else if ctx.host.stat(&format!("{r}/.git")).is_dir() {
        out.add(Item::ok(format!("repo at {r}")));
        let dirty = ctx
            .run(&["git", "-C", r, "status", "--porcelain"], limit::GIT)
            .map_or(0, |call| text::newlines(&call.stdout));
        let ahead = match ctx
            .run(&["git", "-C", r, "rev-list", "--count", "origin/main..HEAD"], limit::GIT)
        {
            Some(call) if call.ok() => call.text(),
            _ => "0".to_string(),
        };
        out.extend(decide_repo(&RepoFacts { dirty, ahead }));
    } else {
        out.add(Item::fail(
            format!("no repository at {r}"),
            "clone it or set REPO_PATH in profile.env",
        ));
    }
    main_runs(ctx, out)
}

/// Lesson 109: sysknife's main sat red from 10-16 to 10-19 on four rustls
/// advisories, every open pull request inherited the failing required check, and
/// nothing here looked. A run that failed in the last few hours may already have
/// a fix on the way, so only a red latest run older than the grace is an alarm.
fn main_runs(ctx: &Ctx<'_>, out: &mut Out) -> Result<(), Stop> {
    let slug = ctx.settings.get("REPO_SLUG");
    if slug.is_empty() || ctx.command_v("gh").is_none() {
        return Ok(());
    }
    let grace = ctx.settings.need("RED_GRACE_HOURS", 187, out)?.to_string();
    // The expansion sits inside the `$(gh run list ...)` of the bash, so an unset
    // limit kills only that substitution: the doctor warns and goes on (line 189).
    let limit_text = if ctx.settings.is_set("DOCTOR_RUN_LIMIT") {
        Some(ctx.settings.get("DOCTOR_RUN_LIMIT").to_string())
    } else {
        out.warn_text("maintainer-doctor: line 189: DOCTOR_RUN_LIMIT: unbound variable\n");
        None
    };
    let unknown = || {
        Item::warn(
            format!(
                "could not read main's workflow runs on {slug}, so whether main is red is unknown"
            ),
            format!("gh run list --repo {slug} --branch main"),
        )
    };
    let listed = limit_text.as_deref().and_then(|limit_text| {
        ctx.run(
            &[
                "gh",
                "run",
                "list",
                "--repo",
                slug,
                "--branch",
                "main",
                "--event",
                "push",
                "--limit",
                limit_text,
                "--json",
                "workflowName,status,conclusion,createdAt,url",
            ],
            limit::GH,
        )
    });
    let Some(call) = listed.filter(crate::ctx::Call::ok) else {
        out.add(unknown());
        return Ok(());
    };
    let now = i64::try_from(ctx.clock.now_unix()).unwrap_or(0);
    // Without python3 the snippet fails, which reads as unreadable.
    let verdicts = if ctx.python_available() {
        verdicts(&call.stdout, &grace, now).unwrap_or_else(|_| "unreadable".to_string())
    } else {
        "unreadable".to_string()
    };
    if verdicts == "unreadable" {
        out.add(unknown());
    } else if verdicts.is_empty() {
        out.add(Item::ok(format!("no workflow on main has been red for {grace}h or more")));
    } else {
        for line in verdicts.split('\n') {
            let fields = text::read_tab_fields(line, 4);
            out.add(Item::fail(
                format!(
                    "main's latest '{}' run failed {}h ago and main has been red since",
                    fields[1], fields[2]
                ),
                format!("read the failing job: {}", fields[3]),
            ));
        }
    }
    Ok(())
}

/// A `createdAt` as Python holds it: two texts compare, two numbers compare, and a
/// text with a number is a `TypeError`.
enum Created {
    Text(String),
    Number(f64),
}

impl Created {
    fn newer_than(&self, other: &Self) -> Result<bool, Crash> {
        match (self, other) {
            (Self::Text(a), Self::Text(b)) => Ok(a > b),
            (Self::Number(a), Self::Number(b)) => Ok(a > b),
            _ => Err(Crash),
        }
    }
}

/// The key a workflow is filed under.
#[derive(Debug, Clone, PartialEq)]
enum NameKey {
    Text(String),
    Number(f64, String),
}

impl NameKey {
    fn number(value: &Value) -> Self {
        let as_float = match value {
            Value::Bool(flag) => f64::from(u8::from(*flag)),
            other => other.as_f64().unwrap_or(f64::NAN),
        };
        Self::Number(as_float, pyval::py_str(value))
    }

    fn shown(&self) -> &str {
        match self {
            Self::Text(text) | Self::Number(_, text) => text,
        }
    }

    fn compare(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
            (Self::Number(a, _), Self::Number(b, _)) => a.total_cmp(b),
            _ => std::cmp::Ordering::Equal,
        }
    }
}

/// The Python snippet that reads `gh run list`: the text it prints, or `Err` when
/// it would have crashed (which the shell turns into "unreadable").
pub fn verdicts(runs: &[u8], grace: &str, now: i64) -> Result<String, Crash> {
    let value = pyval::parse(runs)?;
    let Some(list) = value.as_array().filter(|list| !list.is_empty()) else {
        return Ok("unreadable".to_string());
    };
    let created = |run: &Value| -> Result<Created, Crash> {
        match run.get("createdAt") {
            Some(Value::String(text)) => Ok(Created::Text(text.clone())),
            Some(found @ (Value::Number(_) | Value::Bool(_))) => Ok(Created::Number(match found {
                Value::Bool(flag) => f64::from(u8::from(*flag)),
                other => other.as_f64().unwrap_or(f64::NAN),
            })),
            _ => Err(Crash),
        }
    };
    // Python keys the latest run by `workflowName or "?"`: a text, or a number
    // (`True` is the number 1). A list or dict is unhashable, and a text and a
    // number cannot be sorted together, so both crash the snippet.
    let mut latest: Vec<(NameKey, &Value)> = Vec::new();
    for run in list {
        let object = run.as_object().ok_or(Crash)?;
        let key = match object.get("workflowName") {
            None => NameKey::Text("?".to_string()),
            Some(found) if !pyval::truthy(found) => NameKey::Text("?".to_string()),
            Some(Value::String(text)) => NameKey::Text(text.clone()),
            Some(found @ (Value::Number(_) | Value::Bool(_))) => NameKey::number(found),
            Some(_) => return Err(Crash),
        };
        if let Some(slot) = latest.iter_mut().find(|(existing, _)| *existing == key) {
            if created(run)?.newer_than(&created(slot.1)?)? {
                slot.1 = run;
            }
        } else {
            latest.push((key, run));
        }
    }
    if latest.len() > 1
        && latest.iter().any(|(key, _)| matches!(key, NameKey::Text(_)))
        && latest.iter().any(|(key, _)| matches!(key, NameKey::Number(..)))
    {
        return Err(Crash);
    }
    latest.sort_by(|(left, _), (right, _)| left.compare(right));
    let mut lines = Vec::new();
    for (key, run) in &latest {
        let name = key.shown();
        let completed = run.get("status").and_then(Value::as_str) == Some("completed");
        let failed = run.get("conclusion").and_then(Value::as_str) == Some("failure");
        if !completed || !failed {
            continue;
        }
        let Created::Text(stamp) = created(run)? else { return Err(Crash) };
        let at = pyval::iso_epoch(&stamp).ok_or(Crash)?;
        #[allow(clippy::cast_precision_loss)]
        let age = (now as f64 - at) / 3600.0;
        let grace = text::py_float(grace).ok_or(Crash)?;
        if age >= grace {
            let url = match run.get("url") {
                Some(found) if pyval::truthy(found) => pyval::py_str(found),
                _ => String::new(),
            };
            lines.push(format!("red\t{name}\t{}\t{url}", text::round0(age)));
        }
    }
    Ok(lines.join("\n"))
}
