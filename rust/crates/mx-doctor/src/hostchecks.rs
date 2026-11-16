//! Scheduler, Container verification and Sandbox: the sections that ask the
//! machine what it runs.

use crate::ctx::{Ctx, Out, limit};
use crate::host::glob_names;
use crate::report::Item;
use crate::text;
use mx_core::Fact;
use sha2::{Digest, Sha256};
use std::fmt::Write;

/// A launchd listing counts the lines that name `dev.maintainer`, with a literal dot.
fn count_literal(text: &[u8], needle: &str) -> usize {
    let needle = needle.as_bytes();
    text::lines(text).filter(|line| line.windows(needle.len()).any(|w| w == needle)).count()
}

/// What the Linux scheduler probes read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimerFacts {
    /// Lines of `list-timers` that name this profile's timers.
    pub listed: usize,
    /// Lines of `list-unit-files` that start with this profile's timer names.
    pub files: usize,
    /// True when `loginctl` said the user lingers.
    pub lingering: bool,
    /// True when a listed timer elapses within seconds.
    pub fires_soon: bool,
}

/// The findings for a Linux user session.
#[must_use]
pub fn decide_timers(facts: &TimerFacts, profile: &str, who: &str) -> Vec<Item> {
    let mut items = Vec::new();
    // `list-timers` shows loaded units only, so a disabled timer is absent from it
    // rather than listed as off, which is why a wrong glob read as healthy.
    if facts.listed > 0 {
        items.push(Item::ok(format!("{} systemd timer(s) enabled for {profile}", facts.listed)));
    } else if facts.files > 0 {
        items.push(Item::warn(
            format!(
                "{} timer file(s) for {profile}, none enabled: nothing runs unattended",
                facts.files
            ),
            "./install.sh --timers",
        ));
    } else {
        items.push(Item::warn(format!("no timers for {profile}"), "./install.sh --timers"));
    }
    if facts.lingering {
        items.push(Item::ok("lingering on: timers run without a login session"));
    } else {
        items.push(Item::warn(
            "lingering off: timers stop when you log out",
            format!("sudo loginctl enable-linger {who}"),
        ));
    }
    if facts.fires_soon {
        items.push(Item::warn(
            "a timer fires within seconds; Persistent=true may be catching up",
            "expected after enabling; otherwise check the stamp files",
        ));
    }
    items
}

/// Scheduler.
pub fn scheduler(ctx: &Ctx<'_>, out: &mut Out) {
    out.add(Item::head("Scheduler"));
    let profile = ctx.profile;
    match ctx.uname().as_str() {
        "Linux" => {
            let session = ctx.command_v("systemctl").is_some()
                && ctx
                    .run(&["systemctl", "--user", "show-environment"], limit::QUICK)
                    .is_some_and(|call| call.ok());
            if !session {
                out.add(Item::warn(
                    "no systemd user session",
                    "use platform/posix/install-cron.sh",
                ));
                return;
            }
            let pattern = format!("maintainer@{profile}-*");
            let list_timers = || {
                ctx.run(
                    &["systemctl", "--user", "list-timers", &pattern, "--all", "--no-pager"],
                    limit::QUICK,
                )
                .map(|call| call.stdout)
                .unwrap_or_default()
            };
            let prefix = format!("maintainer@{profile}-");
            let listed = text::count_lines_with(&list_timers(), &prefix, false);
            let unit_pattern = format!("maintainer@{profile}-*.timer");
            let files_out = ctx
                .run(
                    &["systemctl", "--user", "list-unit-files", &unit_pattern, "--no-pager"],
                    limit::QUICK,
                )
                .map(|call| call.stdout)
                .unwrap_or_default();
            let files = text::count_lines_with(&files_out, &prefix, true);
            // `podman run` exports no USER, and `set -u` turned that into a dead
            // report halfway down. id(1) answers without the environment.
            let who = match &ctx.env.user {
                Some(user) => user.clone(),
                None => ctx
                    .run(&["id", "-un"], limit::QUICK)
                    .map(|call| call.text())
                    .unwrap_or_default(),
            };
            let lingering = ctx
                .run(&["loginctl", "show-user", &who, "-p", "Linger", "--value"], limit::QUICK)
                .is_some_and(|call| call.text() == "yes");
            // Lesson 8: a timer about to fire immediately is usually a catch-up.
            let timers = list_timers();
            let fires_soon =
                text::lines(text::strip_trailing_newlines(&timers)).any(text::fires_in_seconds);
            out.report.extend(decide_timers(
                &TimerFacts { listed, files, lingering, fires_soon },
                profile,
                &who,
            ));
        }
        "Darwin" => {
            let listing = ctx
                .run(&["launchctl", "list"], limit::QUICK)
                .map(|call| call.stdout)
                .unwrap_or_default();
            let n = count_literal(&listing, "dev.maintainer");
            if n > 0 {
                out.add(Item::ok(format!("{n} launchd agent(s) loaded")));
            } else {
                out.add(Item::warn("no launchd agents", "platform/macos/install-launchd.sh"));
            }
            out.add(Item::warn(
                "launchd has no Persistent=true",
                "a run missed while asleep does not catch up",
            ));
        }
        _ => out.add(Item::warn(
            format!("{}: no native scheduler wired", ctx.uname()),
            "platform/posix/install-cron.sh",
        )),
    }
}

/// What one verify suite declares and what the image answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuiteFacts {
    pub name: String,
    pub image: String,
    pub needs: String,
    /// For a `localhost/` image: the recipe beside the suite.
    pub recipe: Option<RecipeFacts>,
    /// The probe, when the decisions got that far.
    pub probe: Option<Probe>,
}

/// The `Containerfile` beside a suite and what the host holds of its image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeFacts {
    pub containerfile: Option<String>,
    pub hash: String,
    pub build: String,
    pub image_exists: bool,
    pub stamp: String,
}

/// The answer of the container that checks which binaries the image has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub code: i32,
    pub missing: String,
}

/// The findings for one suite, and whether the needs probe is reached.
#[must_use]
pub fn decide_suite(facts: &SuiteFacts, profile: &str) -> (Vec<Item>, bool) {
    let sn = &facts.name;
    let img = &facts.image;
    if facts.needs.is_empty() {
        return (
            vec![Item::fail(
                format!("verify suite '{sn}' declares no suite_needs, so its image is unchecked"),
                "add suite_needs() naming the binaries its tests invoke",
            )],
            false,
        );
    }
    let mut items = Vec::new();
    if img.starts_with("localhost/") {
        let Some(recipe) = &facts.recipe else { return (items, false) };
        if recipe.containerfile.is_none() {
            items.push(Item::fail(
                format!("verify suite '{sn}' runs {img} and no {sn}.Containerfile builds it"),
                format!("add the recipe beside profiles/{profile}/verify.d/{sn}.sh"),
            ));
            return (items, false);
        }
        if !recipe.image_exists {
            items.push(Item::fail(
                format!("verify suite '{sn}': {img} is not on this host, so nothing can earn a receipt there"),
                recipe.build.clone(),
            ));
            return (items, false);
        }
        // An image that exists and an image built from the recipe in this tree are
        // different claims, so the build stamps the recipe's hash on the image and
        // this compares it. The first arm is the recipe itself: an empty recipe
        // equals an empty stamp (pinned, decision P1I-12).
        if recipe.stamp == recipe.hash {
        } else if recipe.stamp.is_empty() || recipe.stamp == "<no value>" {
            items.push(Item::warn(
                format!("verify suite '{sn}': {img} carries no recipe fingerprint, so drift from {sn}.Containerfile is invisible"),
                recipe.build.clone(),
            ));
        } else {
            items.push(Item::fail(
                format!("verify suite '{sn}': {img} was built from a different {sn}.Containerfile"),
                recipe.build.clone(),
            ));
        }
    }
    let Some(probe) = &facts.probe else { return (items, true) };
    if probe.code != 0 {
        // Asked and could not get an answer. That is not the same as an answer of
        // "nothing missing", and it must not print like one.
        items.push(Item::warn(
            format!("verify suite '{sn}': could not probe {img} (podman exited {})", probe.code),
            format!("podman pull {img}, then re-run; until then this suite's image is unverified"),
        ));
    } else if !probe.missing.is_empty() {
        let shown = probe.missing.strip_suffix(' ').unwrap_or(&probe.missing);
        items.push(Item::fail(
            format!("verify suite '{sn}' runs in {img}, which has no: {shown}"),
            format!(
                "no receipt is earnable for that suite; change suite_image or suite_needs in profiles/{profile}/verify.d/{sn}.sh"
            ),
        ));
    } else {
        items.push(Item::ok(format!("verify suite '{sn}': {img} provides {}", facts.needs)));
    }
    (items, true)
}

const SUITE_SCRIPT: &str =
    "set -uo pipefail; . \"$1\" >/dev/null 2>&1; declare -F suite_image >/dev/null && suite_image";
const NEEDS_SCRIPT: &str =
    "set -uo pipefail; . \"$1\" >/dev/null 2>&1; declare -F suite_needs >/dev/null && suite_needs";
const PROBE_SCRIPT: &str =
    "for b in \"$@\"; do command -v \"$b\" >/dev/null 2>&1 || printf \"%s \" \"$b\"; done";

/// Reads one suite: what it declares, the recipe beside it, and what the image answers.
fn gather_suite(ctx: &Ctx<'_>, out: &mut Out, path: &str, file: &str) -> SuiteFacts {
    let name = file.strip_suffix(".sh").unwrap_or(file).to_string();
    // Sourced in a child each time: these files define functions with the same
    // names, and one leaking into the next would report the previous suite's
    // image under this suite's name.
    let ask = |script: &str, out: &mut Out| -> String {
        let argv = ["bash", "-c", script, "_", path];
        let mut spec =
            mx_ports::ProcSpec::command(argv[0], argv[1..].iter().copied(), limit::SOURCING);
        for (key, value) in ctx.suite_env {
            spec = spec.env(key.as_str(), value.as_str());
        }
        match ctx.proc.run(&spec) {
            Ok(output) => {
                out.stderr.extend_from_slice(&output.stderr);
                text::captured(&output.stdout)
            }
            Err(_) => String::new(),
        }
    };
    let image = ask(SUITE_SCRIPT, out);
    let needs = ask(NEEDS_SCRIPT, out);
    let mut facts = SuiteFacts { name, image, needs, recipe: None, probe: None };
    if !facts.needs.is_empty() && facts.image.starts_with("localhost/") {
        let containerfile = format!("{}.Containerfile", path.strip_suffix(".sh").unwrap_or(path));
        let present = ctx.host.stat(&containerfile).is_file();
        let hash = match ctx.host.read(&containerfile) {
            Fact::Known(bytes) if present => hex(&Sha256::digest(&bytes)),
            _ => String::new(),
        };
        let parent = containerfile.rsplit_once('/').map_or(".", |(parent, _)| parent);
        let build = format!(
            "podman build --label maintainer.recipe={hash} -t {} -f {containerfile} {parent}",
            facts.image
        );
        let mut recipe = RecipeFacts {
            containerfile: present.then_some(containerfile),
            hash,
            build,
            image_exists: false,
            stamp: String::new(),
        };
        if present {
            recipe.image_exists = ctx
                .run(&["podman", "image", "exists", &facts.image], limit::PODMAN)
                .is_some_and(|call| call.ok());
            if recipe.image_exists {
                recipe.stamp = ctx
                    .run(
                        &[
                            "podman",
                            "image",
                            "inspect",
                            "--format",
                            "{{index .Config.Labels \"maintainer.recipe\"}}",
                            &facts.image,
                        ],
                        limit::PODMAN,
                    )
                    .map(|call| call.text())
                    .unwrap_or_default();
            }
        }
        facts.recipe = Some(recipe);
    }
    let (_, reaches_probe) = decide_suite(&facts, ctx.profile);
    if reaches_probe {
        // $needs is a space-separated list from a file in this repository and
        // is split on purpose; it is passed as arguments rather than
        // interpolated into the shell the container runs.
        let mut argv: Vec<&str> = vec![
            "podman",
            "run",
            "--rm",
            "--network=none",
            &facts.image,
            "sh",
            "-c",
            PROBE_SCRIPT,
            "sh",
        ];
        argv.extend(text::ifs_words(&facts.needs));
        let (code, missing) = match ctx.run(&argv, limit::PODMAN) {
            Some(call) => (call.code, call.text()),
            None => (127, String::new()),
        };
        facts.probe = Some(Probe { code, missing });
    }
    facts
}

/// Container verification.
pub fn container_verification(ctx: &Ctx<'_>, out: &mut Out) {
    out.add(Item::head("Container verification"));
    if ctx.quick {
        out.add(Item::warn("skipped (--quick)", ""));
        return;
    }
    if ctx.command_v("podman").is_none() {
        out.add(Item::warn(
            "podman not installed; the merge gate cannot observe a mutation",
            "install podman",
        ));
        return;
    }
    // Lesson 7: what matters is whether it works COLD, because that is the state a
    // timer finds after a reboot.
    drop(ctx.run(&["podman", "system", "migrate"], limit::PODMAN));
    let cold = ctx
        .run(
            &[
                "podman",
                "run",
                "--rm",
                "--network=none",
                "docker.io/library/busybox:latest",
                "true",
            ],
            limit::PODMAN,
        )
        .is_some_and(|call| call.ok());
    if cold {
        out.add(Item::ok("podman runs from cold"));
    } else {
        out.add(Item::fail(
            "podman cannot start a container from cold",
            "check newuidmap and /etc/subuid",
        ));
    }
    let dir = format!("{}/profiles/{}/verify.d", ctx.share, ctx.profile);
    for file in glob_names(ctx.host, &dir, "", ".sh") {
        let path = format!("{dir}/{file}");
        if !ctx.host.stat(&path).exists() {
            continue;
        }
        let facts = gather_suite(ctx, out, &path, &file);
        out.report.extend(decide_suite(&facts, ctx.profile).0);
    }
    let unit = format!("{}/.config/systemd/user/podman-userns-warmup.service", ctx.env.home);
    if ctx.uname() == "Linux" && ctx.host.stat(&unit).is_file() {
        out.add(Item::ok("userns warm-up unit installed (hardened units can reuse the namespace)"));
    } else if ctx.uname() == "Linux" {
        out.add(Item::fail(
            "no podman-userns-warmup.service",
            "./install.sh; without it a hardened timer cannot start a container",
        ));
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// True for a unit line `grep -E` selects: one of the four fixed directives, or
/// `Protect`, `Restrict` or `SystemCall` followed by letters, then `=`.
#[must_use]
pub fn is_hardening_line(line: &[u8]) -> bool {
    for fixed in ["NoNewPrivileges", "LockPersonality", "MemoryDenyWriteExecute", "PrivateTmp"] {
        if line.strip_prefix(fixed.as_bytes()).is_some_and(|rest| rest.first() == Some(&b'=')) {
            return true;
        }
    }
    for prefix in ["Protect", "Restrict", "SystemCall"] {
        if let Some(rest) = line.strip_prefix(prefix.as_bytes()) {
            let letters = rest.iter().take_while(|byte| byte.is_ascii_alphabetic()).count();
            if letters > 0 && rest.get(letters) == Some(&b'=') {
                return true;
            }
        }
    }
    false
}

/// Sandbox. The timers run under the hardening in maintainer@.service, and an
/// interactive shell does not, so every check above can pass while an unattended
/// run cannot do something it needs. This probes the extraction `verify` uses,
/// inside a transient unit carrying the directives read from the installed unit.
pub fn sandbox(ctx: &Ctx<'_>, out: &mut Out) {
    out.add(Item::head("Sandbox"));
    let unit = format!("{}/.config/systemd/user/maintainer@.service", ctx.env.home);
    if ctx.uname() != "Linux"
        || !ctx.host.stat(&unit).is_file()
        || ctx.command_v("systemd-run").is_none()
    {
        out.add(Item::ok("no systemd user unit to probe on this machine"));
        return;
    }
    let (dirs, grc) = match ctx.host.read(&unit) {
        Fact::Known(bytes) => {
            let found: Vec<&[u8]> =
                text::lines(&bytes).filter(|line| is_hardening_line(line)).collect();
            let joined = found.join(&b'\n');
            (text::captured(&joined), i32::from(found.is_empty()))
        }
        _ => (String::new(), 2),
    };
    let merge = ctx.command_v("maintainer-merge").unwrap_or_default();
    if grc != 0 || dirs.is_empty() {
        out.add(Item::warn(
            format!("no hardening directive found in {unit} (grep exited {grc})"),
            "the sandbox probe has nothing to apply; read the unit by hand",
        ));
    } else if merge.is_empty() {
        out.add(Item::warn(
            "maintainer-merge is not on PATH, so the sandbox probe cannot run",
            "./install.sh",
        ));
    } else if !ctx
        .run(&["systemd-run", "--user", "--wait", "--pipe", "--quiet", "true"], limit::SYSTEMD_RUN)
        .is_some_and(|call| call.ok())
    {
        out.add(Item::warn(
            "systemd-run --user cannot start a transient unit, so the sandbox is unprobed",
            "log in to a user session, then re-run",
        ));
    } else {
        probe_extraction(ctx, out, &unit, &dirs, &merge);
    }
}

fn probe_extraction(ctx: &Ctx<'_>, out: &mut Out, unit: &str, dirs: &str, merge: &str) {
    let lines: Vec<&str> = dirs.split('\n').collect();
    let tmp = ctx.env.tmpdir.clone().unwrap_or_else(|| "/tmp".to_string());
    // `mktemp -d` failing leaves `_sb` empty and the bash goes on building the
    // probe under `/` (`/r`, `/x`, `/s`). That is dangerous for root and harmless
    // for anyone else, and it is not on the plan, so it is kept and pinned by a
    // test (found, not fixed).
    let sb = ctx.host.make_temp_dir(&tmp).unwrap_or_default();
    let repo = format!("{sb}/r");
    let git_ok =
        ctx.run(&["git", "init", "-q", "-b", "main", &repo], limit::GIT).is_some_and(|c| c.ok())
            && ctx.host.write(&format!("{repo}/f"), b"x\n").is_ok()
            && ctx.run(&["git", "-C", &repo, "add", "f"], limit::GIT).is_some_and(|c| c.ok())
            && ctx
                .run(
                    &[
                        "git",
                        "-C",
                        &repo,
                        "-c",
                        "user.email=t@t",
                        "-c",
                        "user.name=t",
                        "-c",
                        "commit.gpgsign=false",
                        "-c",
                        "core.hooksPath=/dev/null",
                        "commit",
                        "-qm",
                        "probe",
                    ],
                    limit::GIT,
                )
                .is_some_and(|c| c.ok());
    drop(ctx.host.make_dirs(&format!("{sb}/x")));
    drop(ctx.host.make_dirs(&format!("{sb}/s")));
    if git_ok {
        let repo_env = format!("MAINTAINER_REPO={repo}");
        let state_env = format!("MAINTAINER_STATE={sb}/s");
        let target = format!("{sb}/x");
        let mut argv: Vec<&str> = vec!["systemd-run", "--user", "--wait", "--pipe", "--quiet"];
        for line in &lines {
            argv.push("-p");
            argv.push(line);
        }
        argv.extend(["-E", &repo_env, "-E", &state_env]);
        argv.extend(["bash", "-c", ". \"$1\" && extract_tree HEAD \"$2\"", "_", merge, &target]);
        let call = ctx.run(&argv, limit::SYSTEMD_RUN);
        let (code, combined) = match &call {
            Some(call) => {
                let mut all = call.stdout.clone();
                all.extend_from_slice(&call.stderr);
                (call.code, text::captured(&all))
            }
            None => (127, String::new()),
        };
        if code == 0 {
            out.add(Item::ok(format!(
                "verify extracts a tree under the unit's hardening ({} directive(s) applied)",
                lines.len()
            )));
        } else {
            out.add(Item::fail(
                format!(
                    "verify cannot extract a tree under the unit's hardening: {}",
                    text::head_lines_spaced(&combined, 2)
                ),
                format!(
                    "a directive in {unit} blocks a call the extraction needs; every unattended verify fails until it is fixed"
                ),
            ));
        }
    } else {
        out.add(Item::warn("could not build the probe repository", format!("check git and {tmp}")));
    }
    if !sb.is_empty() {
        ctx.host.remove_tree(&sb);
    }
}
