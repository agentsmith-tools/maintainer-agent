//! The first four sections: Deployment, Profile and containment, Backend, and
//! the commands the prompts name.

use crate::ctx::{Ctx, Out, Stop, limit};
use crate::pyval;
use crate::report::Item;
use crate::text;
use mx_core::Fact;

/// What the deny wall file says: the rule count, when Python could read it.
#[must_use]
pub fn rule_count(contents: &Fact<Vec<u8>>) -> Option<usize> {
    let Fact::Known(bytes) = contents else { return None };
    let value = pyval::parse(bytes).ok()?;
    pyval::py_len(pyval::index(&value, &["permissions", "deny"])?)
}

/// The rule count a `python3 -c` snippet prints, or nothing when python3 is absent.
fn count_rules(ctx: &Ctx<'_>, contents: &Fact<Vec<u8>>) -> Option<usize> {
    if ctx.python_available() { rule_count(contents) } else { None }
}

/// Deployment.
pub fn deployment(ctx: &Ctx<'_>, out: &mut Out) {
    out.add(Item::head("Deployment"));
    let share = &ctx.share;
    if ctx.host.stat(share).is_dir() {
        out.add(Item::ok(format!("installed at {share}")));
    } else {
        out.add(Item::fail("not installed", "run ./install.sh from the repository"));
    }
    // Lesson 2: cp -r nests on reinstall, and the nested copy is the one that
    // gets rendered while the live one goes stale.
    let nested = ctx.host.count_dirs_named(share, &["profiles", "backends"]);
    if nested <= 2 {
        out.add(Item::ok("no nested profiles/ or backends/"));
    } else {
        out.add(Item::fail(
            format!("{nested} profiles/backends dirs; a reinstall nested them"),
            format!("rm -rf {share} && ./install.sh"),
        ));
    }
    // Lesson 1: executability, not existence.
    for name in ["run.sh", "run-instance.sh"] {
        let stat = ctx.host.stat(&format!("{share}/{name}"));
        if !stat.exists() {
            if name == "run-instance.sh" && ctx.uname() != "Linux" {
                out.add(Item::ok(format!("{name} not needed on {}", ctx.uname())));
            } else {
                out.add(Item::fail(format!("{name} missing"), "./install.sh"));
            }
        } else if stat.executable {
            out.add(Item::ok(format!("{name} is executable")));
        } else {
            out.add(Item::fail(
                format!("{name} is NOT executable"),
                format!("chmod +x {share}/{name}, then fix install.sh"),
            ));
        }
    }
    let bin = format!("{}/.local/bin", ctx.env.home);
    for name in ["maintainer", "maintainer-merge"] {
        if ctx.host.stat(&format!("{bin}/{name}")).executable {
            out.add(Item::ok(format!("{name} on PATH and executable")));
        } else {
            out.add(Item::fail(format!("{bin}/{name} missing or not executable"), "./install.sh"));
        }
    }
}

/// Profile and containment.
pub fn profile_and_containment(ctx: &Ctx<'_>, out: &mut Out) -> Result<(), Stop> {
    out.add(Item::head("Profile and containment"));
    let share = &ctx.share;
    let profile = ctx.profile;
    let pdir = format!("{share}/profiles/{profile}");
    if ctx.host.stat(&format!("{pdir}/profile.env")).is_file() {
        out.add(Item::ok(format!("profile '{profile}' present")));
    } else {
        out.add(Item::fail(format!("no profile '{profile}'"), "check profiles/ in the repository"));
    }
    let settings_path = format!("{pdir}/settings.json");
    if ctx.host.stat(&format!("{pdir}/settings.json.template")).is_file() {
        out.add(Item::fail(
            "an old settings.json.template is still here",
            "rm it and re-run ./install.sh; walls are generated from deny.json now",
        ));
    } else if !ctx.host.stat(&format!("{pdir}/deny.json")).is_file() {
        out.add(Item::fail(
            format!("no deny.json for '{profile}'"),
            "every profile needs one; copy profiles/_template/deny.json",
        ));
    } else if ctx.host.stat(&settings_path).is_file() {
        let contents = ctx.host.read(&settings_path);
        match count_rules(ctx, &contents) {
            None => out.add(Item::fail("deny wall is not valid JSON", "reinstall")),
            Some(n) => {
                let floor = ctx.settings.need("DENY_WALL_MIN_RULES", 92, out)?;
                // `[ N -lt FLOOR ]` with a floor that is not a number prints an error
                // and is false, so the wall reads as generated.
                let under = if let Some(floor) = text::test_int(floor) {
                    i64::try_from(n).is_ok_and(|n| n < floor)
                } else {
                    out.warn_text(&text::integer_expected(92, floor));
                    false
                };
                if under {
                    out.add(Item::fail(
                        format!(
                            "deny wall has only {n} rules, under the floor of {floor}; it is stale"
                        ),
                        format!("rm -rf {share} && ./install.sh"),
                    ));
                } else {
                    out.add(Item::ok(format!("deny wall generated, {n} rules")));
                }
            }
        }
        let bytes = contents.known().unwrap_or_default();
        let has = |needle: &[u8]| bytes.windows(needle.len()).any(|window| window == needle);
        if has(b"__HOME__") {
            out.add(Item::fail("unrendered __HOME__ left in the deny wall", "reinstall"));
        }
        // Measured: a single leading slash anchors to the settings directory, not to
        // the filesystem root, so Read(/home/you/.ssh/**) denies nothing. The ~/ form
        // is the one that works.
        if has(b"\"Read(~/") {
            out.add(Item::ok(
                "credential Read rules use the home-relative form that actually denies",
            ));
        } else {
            out.add(Item::fail(
                "no Read(~/...) rule; credential paths are not protected",
                "check credential_paths in deny.json and re-run ./install.sh",
            ));
        }
    } else {
        out.add(Item::fail("no deny wall at all", "./install.sh"));
    }
    // POST=off is the wall that makes a first week safe. If the profile asks for
    // it, the wall it needs must exist.
    let rehearsal = format!("{pdir}/settings-rehearsal.json");
    if ctx.host.stat(&rehearsal).is_file() {
        let count =
            count_rules(ctx, &ctx.host.read(&rehearsal)).map(|n| n.to_string()).unwrap_or_default();
        out.add(Item::ok(format!(
            "rehearsal wall present, {count} rules (POST=off posts nothing)"
        )));
    } else {
        out.add(Item::warn(
            "no rehearsal wall; POST=off would refuse to start",
            "./install.sh generates it",
        ));
    }
    if ctx.host.stat(&format!("{share}/profiles/_template")).is_dir() {
        out.add(Item::warn(
            "the scaffolding template was deployed",
            "harmless, but ./install.sh should have skipped it",
        ));
    }
    Ok(())
}

/// Backend.
pub fn backend(ctx: &Ctx<'_>, out: &mut Out) {
    out.add(Item::head("Backend"));
    // bash PR #243: runs are serialised with flock, so a host without it cannot run.
    if ctx.command_v("flock").is_some() {
        out.add(Item::ok("flock on PATH"));
    } else {
        out.add(Item::fail(
            "flock is required to serialize runs and is NOT on PATH",
            "install flock for this platform",
        ));
    }
    let b = ctx.settings.get_or("BACKEND", "claude");
    if ctx.host.stat(&format!("{}/backends/{b}.sh", ctx.share)).is_file() {
        out.add(Item::ok(format!("backend '{b}' installed")));
    } else {
        out.add(Item::fail(format!("backend '{b}' not installed"), "./install.sh"));
    }
    match b {
        "claude" => {
            if ctx.command_v("claude").is_some() {
                let version = ctx
                    .run(&["claude", "--version"], limit::QUICK)
                    .map(|call| {
                        let first = call.stdout.split(|byte| *byte == b'\n').next().unwrap_or(&[]);
                        text::captured(first)
                    })
                    .unwrap_or_default();
                out.add(Item::ok(format!("claude on PATH ({version})")));
            } else {
                out.add(Item::fail("claude not on PATH", "install Claude Code"));
            }
        }
        "codex" => on_path(ctx, out, "codex", "codex", "npm i -g @openai/codex"),
        "cursor" => on_path(ctx, out, "cursor-agent", "cursor-agent", "install the Cursor CLI"),
        "opencode" => on_path(
            ctx,
            out,
            "opencode",
            "opencode",
            "curl -fsSL https://opencode.ai/install | bash",
        ),
        _ => {}
    }
}

fn on_path(ctx: &Ctx<'_>, out: &mut Out, command: &str, shown: &str, fix: &str) {
    if ctx.command_v(command).is_some() {
        out.add(Item::ok(format!("{shown} on PATH")));
    } else {
        out.add(Item::fail(format!("{shown} not on PATH"), fix));
    }
}

/// Commands the prompts tell the agent to run.
pub fn commands(ctx: &Ctx<'_>, out: &mut Out) {
    out.add(Item::head("Commands the prompts tell the agent to run"));
    let list = ctx.settings.get_or("REQUIRED_COMMANDS", "maintainer maintainer-merge gh git");
    for name in text::ifs_words(list) {
        if ctx.command_v(name).is_some() {
            out.add(Item::ok(format!("{name} resolves")));
        } else {
            out.add(Item::fail(
                format!("{name} is named in the prompts and is NOT on PATH"),
                "install it, or remove it from the prompts and REQUIRED_COMMANDS",
            ));
        }
    }
}
