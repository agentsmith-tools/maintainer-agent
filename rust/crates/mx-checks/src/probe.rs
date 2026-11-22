//! `containment-probe`: run adversarial payloads inside the verification container and check that
//! each one is stopped.
//!
//! Every control the merge gate claims is measured here with the attack it is supposed to stop,
//! because a flag being present in a command line is not evidence that it does anything. The
//! program runs eleven attacks with the gate's flags, reads each verdict off the output, and then
//! checks that `bin/maintainer-merge` still passes those flags.
//!
//! The eleven payloads and the way each verdict is read are data and pure functions here
//! ([`ATTACKS`], [`rc_zero`], [`caps_empty`] and the others), so a test can feed each one what a contained
//! and an uncontained run print.
//!
//! # Decisions
//!
//! - `P4B-5`: a captured `2>&1` reads standard output and then standard error.
//! - `P4B-4`: a call that runs past two hours is killed and reads as an empty answer.
//!
//! # Found, not fixed
//!
//! A runtime that fails every run prints nothing, and every attack judged by the absence of a
//! success (nine of the eleven) then reads as stopped: only the capabilities and `NoNewPrivs`
//! checks, which need to see something, fail. The caller's own environment reaches the runtime on
//! every run but the credential one. `rc=0` anywhere in the output, even in prose, counts as a
//! success of the attack. A flag named only in a comment of the gate satisfies the last section.
//! The runtime's own exit status is never read. All preserved and pinned by
//! `tests/characterization/checks-probe.sh`.

use crate::ctx::{Call, Ctx};
use crate::host::Kind;
use crate::shell;
use mx_core::Fact;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// The image the attacks run in unless `PROBE_IMAGE` names another.
pub const DEFAULT_IMAGE: &str = "docker.io/library/bash:5";

/// The flags of the merge gate that the last section looks for, in the order it looks.
pub const GATE_FLAGS: [&str; 8] = [
    "--network=none",
    "--cap-drop=ALL",
    "no-new-privileges",
    "--read-only",
    "noexec",
    "--pids-limit",
    "--memory",
    "--cpus",
];

/// One attack: the payload the container runs.
pub const ATTACKS: [&str; 11] = [
    "timeout 5 bash -c \"echo > /dev/tcp/1.1.1.1/53\" 2>&1; echo rc=$?",
    "getent hosts github.com >/dev/null 2>&1; echo rc=$?",
    "echo pwned > /etc/passwd 2>&1; echo rc=$?",
    "printf \"#!/bin/sh\\necho ran\\n\" > /tmp/x && chmod +x /tmp/x && /tmp/x 2>&1; echo rc=$?",
    "grep -c . /proc/self/status >/dev/null; capsh --print 2>/dev/null | grep -i \"current:\" || grep CapEff /proc/self/status",
    "grep NoNewPrivs /proc/self/status",
    "n=0; while [ $n -lt 4000 ]; do sleep 30 & n=$((n+1)); done 2>/dev/null; echo spawned=$n",
    "head -c 4G /dev/zero > /tmp/balloon 2>&1; echo rc=$?",
    "while :; do :; done",
    "env; cat /proc/1/environ 2>/dev/null | tr \"\\0\" \"\\n\"",
    "ls /repo; echo \"escaped\" > /repo/../escape 2>&1; echo rc=$?",
];

/// The text of a `stopped` line.
#[must_use]
pub fn ok_line(what: &str) -> Vec<u8> {
    format!("  \u{1b}[32mstopped\u{1b}[0m  {what}\n").into_bytes()
}

/// The text of a `GOT THROUGH` line.
#[must_use]
pub fn bad_line(what: &[u8]) -> Vec<u8> {
    let mut line = b"  \x1b[31mGOT THROUGH\x1b[0m  ".to_vec();
    line.extend_from_slice(what);
    line.push(b'\n');
    line
}

/// `grep -q 'rc=0'`: the attack's exit status is printed and it is zero.
#[must_use]
pub fn rc_zero(out: &[u8]) -> bool {
    shell::contains(out, b"rc=0")
}

/// `grep -q '^ran'`: a line that starts with `ran`.
#[must_use]
pub fn ran(out: &[u8]) -> bool {
    shell::lines(out, true).iter().any(|line| line.starts_with(b"ran"))
}

/// `grep -qE 'CapEff:\s*0+$'`: a line that ends with `CapEff:`, white space and only zeros.
#[must_use]
pub fn caps_empty(out: &[u8]) -> bool {
    shell::lines(out, true).iter().any(|line| {
        let marker = b"CapEff:";
        (0..line.len()).filter(|at| line[*at..].starts_with(marker)).any(|at| {
            let mut rest = &line[at + marker.len()..];
            while let Some((first, tail)) = rest.split_first() {
                if shell::is_space(*first) {
                    rest = tail;
                } else {
                    break;
                }
            }
            !rest.is_empty() && rest.iter().all(|byte| *byte == b'0')
        })
    })
}

/// `grep -q 'NoNewPrivs:.*1'`: a line with `NoNewPrivs:` and a `1` somewhere after it.
#[must_use]
pub fn no_new_privs_set(out: &[u8]) -> bool {
    shell::lines(out, true).iter().any(|line| {
        let marker = b"NoNewPrivs:";
        (0..line.len())
            .filter(|at| line[*at..].starts_with(marker))
            .any(|at| line[at + marker.len()..].contains(&b'1'))
    })
}

/// `sed -n 's/.*spawned=\([0-9]*\).*/\1/p' | tail -1`: the digits after the last `spawned=` of the
/// last line that has one. Empty when no line has one, and for `spawned=` with no digits.
#[must_use]
pub fn spawned(out: &[u8]) -> Vec<u8> {
    let marker = b"spawned=";
    let mut found = Vec::new();
    for line in shell::lines(out, true) {
        if let Some(at) = (0..line.len()).rev().find(|at| line[*at..].starts_with(marker)) {
            let digits = &line[at + marker.len()..];
            found = digits.iter().copied().take_while(u8::is_ascii_digit).collect();
        }
    }
    found
}

/// How a fork bomb read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fork {
    /// The container died before it could report.
    Silent,
    /// The bomb stopped under 1000 processes.
    Stopped(Vec<u8>),
    /// It reached 1000 or more.
    Through(Vec<u8>),
}

/// Reads a fork bomb's count. A count the shell cannot compare reads as through, as the script's
/// failing `[` leaves the `else` branch.
#[must_use]
pub fn fork_verdict(count: &[u8]) -> Fork {
    if count.is_empty() {
        Fork::Silent
    } else if shell::legal_number(count).is_some_and(|n| n < 1000) {
        Fork::Stopped(count.to_vec())
    } else {
        Fork::Through(count.to_vec())
    }
}

/// The bytes of `printf '%s' "$out" | tr -d '\n' | cut -c1-60`.
#[must_use]
pub fn first_60_without_newlines(out: &[u8]) -> Vec<u8> {
    out.iter().copied().filter(|byte| *byte != b'\n').take(60).collect()
}

fn lossy(path: &Path) -> String {
    String::from_utf8_lossy(path.as_os_str().as_bytes()).into_owned()
}

struct Probe {
    runtime: String,
    image: String,
    timeout: String,
    work: PathBuf,
    passed: u32,
    failed: u32,
}

impl Probe {
    fn ok(&mut self, ctx: &mut Ctx<'_>, what: &str) {
        ctx.put_out(&ok_line(what));
        self.passed += 1;
    }

    fn bad(&mut self, ctx: &mut Ctx<'_>, what: &[u8]) {
        ctx.put_out(&bad_line(what));
        self.failed += 1;
    }

    /// `hardened`: the gate's flags, the work directory mounted at /repo, the payload; the output is
    /// what `$(... 2>&1)` holds.
    fn hardened(
        &self,
        ctx: &mut Ctx<'_>,
        payload: &str,
        env: &[(&str, &str)],
        line: u32,
    ) -> Vec<u8> {
        let mut argv: Vec<String> =
            ["run", "--rm", "--timeout"].iter().map(|word| (*word).to_string()).collect();
        argv.insert(0, self.runtime.clone());
        argv.push(self.timeout.clone());
        for word in [
            "--network=none",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--read-only",
            "--tmpfs",
            "/tmp:rw,noexec,nosuid,size=256m",
            "--pids-limit",
            "512",
            "--memory",
            "2g",
            "--cpus",
            "2",
            "-v",
        ] {
            argv.push(word.to_string());
        }
        argv.push(format!("{}:/repo:z", lossy(&self.work)));
        for word in ["-w", "/repo"] {
            argv.push(word.to_string());
        }
        argv.push(self.image.clone());
        argv.extend(["sh".to_string(), "-c".to_string(), payload.to_string()]);
        let mut call = Call::new(argv);
        for (name, value) in env {
            call = call.with_env(name, value);
        }
        let done = ctx.exec(&call);
        // Line 0 is the loop attack, whose output goes to /dev/null and is never substituted.
        if line == 0 {
            shell::command_substitution(&done.merged())
        } else {
            ctx.substitution(&done.merged(), line)
        }
    }

    /// `"$rt" ps -q 2>/dev/null | wc -l`.
    fn container_count(&self, ctx: &Ctx<'_>) -> usize {
        let done = ctx.exec(&Call::new([self.runtime.clone(), "ps".to_string(), "-q".to_string()]));
        shell::lines(&done.stdout, false).len()
    }
}

/// The program, from its working directory to its exit code. It takes no argument.
pub fn run(ctx: &mut Ctx<'_>) -> i32 {
    let runtime = if ctx.on_path("podman") {
        "podman"
    } else if ctx.on_path("docker") {
        "docker"
    } else {
        ctx.err_line("no container runtime; this probe measured nothing");
        return 1;
    };
    let image = match ctx.vars.get("PROBE_IMAGE") {
        Some(name) if !name.is_empty() => name.clone(),
        _ => DEFAULT_IMAGE.to_string(),
    };
    let inspected = ctx.exec(&Call::new([
        runtime.to_string(),
        "image".to_string(),
        "inspect".to_string(),
        image.clone(),
    ]));
    if !inspected.ok() {
        let pulled = ctx.exec(&Call::new([
            runtime.to_string(),
            "pull".to_string(),
            "-q".to_string(),
            image.clone(),
        ]));
        if !pulled.ok() {
            ctx.err_line(&format!("cannot obtain {image}"));
            return 1;
        }
    }

    let tmp = ctx.tmpdir();
    let work = match ctx.host().make_temp_dir(&tmp) {
        Ok(path) => path,
        Err(error) => {
            ctx.err_line(&format!(
                "containment-probe: cannot make a temporary directory under {}: {}",
                tmp.display(),
                error.reason
            ));
            return 1;
        }
    };
    drop(ctx.host().write(&work.join("repofile"), b"host file\n"));
    let timeout = match ctx.vars.get("PROBE_TIMEOUT") {
        Some(value) if !value.is_empty() => value.clone(),
        _ => "15".to_string(),
    };
    let mut probe = Probe {
        runtime: runtime.to_string(),
        image,
        timeout,
        work: work.clone(),
        passed: 0,
        failed: 0,
    };
    let armed = crate::cleanup::Armed::new(&work);
    let code = attacks(ctx, &mut probe);
    ctx.host().remove_tree(&work);
    drop(armed);
    code
}

/// Prints one verdict: `through` when the attack succeeded, else the `stopped` line.
fn verdict(
    ctx: &mut Ctx<'_>,
    probe: &mut Probe,
    through: bool,
    through_text: &str,
    stopped_text: &str,
) {
    if through {
        probe.bad(ctx, through_text.as_bytes());
    } else {
        probe.ok(ctx, stopped_text);
    }
}

/// The five attacks that read their verdict off a status or a status file.
fn isolation_attacks(ctx: &mut Ctx<'_>, probe: &mut Probe) {
    let out = probe.hardened(ctx, ATTACKS[0], &[], 47);
    verdict(
        ctx,
        probe,
        rc_zero(&out),
        "outbound TCP to 1.1.1.1:53",
        "outbound TCP (--network=none)",
    );
    let out = probe.hardened(ctx, ATTACKS[1], &[], 50);
    verdict(ctx, probe, rc_zero(&out), "DNS resolution", "DNS resolution");
    let out = probe.hardened(ctx, ATTACKS[2], &[], 53);
    verdict(
        ctx,
        probe,
        rc_zero(&out),
        "writing to /etc inside the image",
        "writing to the image (--read-only)",
    );
    let out = probe.hardened(ctx, ATTACKS[3], &[], 56);
    verdict(ctx, probe, ran(&out), "executing from /tmp", "executing from /tmp (noexec)");

    // The two attacks that must SHOW something: no output at all is a failure.
    let out = probe.hardened(ctx, ATTACKS[4], &[], 59);
    if caps_empty(&out) {
        probe.ok(ctx, "capabilities (CapEff is empty)");
    } else {
        let mut what = b"capabilities: ".to_vec();
        what.extend_from_slice(&first_60_without_newlines(&out));
        probe.bad(ctx, &what);
    }
    let out = probe.hardened(ctx, ATTACKS[5], &[], 63);
    if no_new_privs_set(&out) {
        probe.ok(ctx, "privilege escalation (NoNewPrivs=1)");
    } else {
        let mut what = b"NoNewPrivs is not set: ".to_vec();
        what.extend_from_slice(&out);
        probe.bad(ctx, &what);
    }
}

/// The fork bomb, the balloon and the infinite loop.
fn resource_attacks(ctx: &mut Ctx<'_>, probe: &mut Probe) {
    // With a pid cap, a fork bomb cannot reach the cap's worth of processes before the container
    // dies. Count what it managed.
    let out = probe.hardened(ctx, ATTACKS[6], &[], 74);
    match fork_verdict(&spawned(&out)) {
        Fork::Silent => probe.ok(ctx, "fork bomb (the container died before it could report)"),
        Fork::Stopped(n) => {
            probe.ok(
                ctx,
                &format!(
                    "fork bomb (stopped at {} processes, cap is 512)",
                    String::from_utf8_lossy(&n)
                ),
            );
        }
        Fork::Through(n) => {
            if shell::legal_number(&n).is_none() {
                let mut text = ctx.script_name().as_bytes().to_vec();
                text.extend_from_slice(b": line 78: [: ");
                text.extend_from_slice(&n);
                text.extend_from_slice(b": integer expected\n");
                ctx.put_err(&text);
            }
            let mut what = b"fork bomb spawned ".to_vec();
            what.extend_from_slice(&n);
            what.extend_from_slice(b" processes; the pid cap is not in force");
            probe.bad(ctx, &what);
        }
    }

    let out = probe.hardened(ctx, ATTACKS[7], &[], 84);
    verdict(ctx, probe, rc_zero(&out), "allocating 4G in a 256m tmpfs", "memory balloon");

    // GNU `timeout` around the client kills the CLI and leaves the container running. The
    // runtime's own `--timeout` is enforced by conmon, so the container dies whatever happens to
    // the client.
    let before = probe.container_count(ctx);
    drop(probe.hardened(ctx, ATTACKS[8], &[], 0));
    let slept = ctx.exec(&Call::new(["sleep", "3"]).at(93));
    ctx.show_both(&slept);
    let after = probe.container_count(ctx);
    if after <= before {
        probe.ok(ctx, "infinite loop (--timeout, and no container left behind)");
    } else {
        let what = format!(
            "an infinite loop left {} container(s) running after the gate gave up",
            after - before
        );
        probe.bad(ctx, what.as_bytes());
    }
}

/// What crosses from the host: a credential, and a file written outside the mount.
fn host_attacks(ctx: &mut Ctx<'_>, probe: &mut Probe) {
    // The property that matters is whether anything from the HOST crosses, so plant a canary.
    let canaries = [("GH_TOKEN", "ghp_PROBECANARY0000"), ("ANTHROPIC_API_KEY", "sk-PROBECANARY")];
    let out = probe.hardened(ctx, ATTACKS[9], &canaries, 103);
    verdict(
        ctx,
        probe,
        shell::contains(&out, b"PROBECANARY"),
        "a host credential reached the container",
        "host credentials (podman passes no host environment)",
    );
    drop(probe.hardened(ctx, ATTACKS[10], &[], 108));
    let escaped = probe.work.join("../escape");
    let through = ctx.host().kind(&escaped) != Kind::Missing;
    verdict(ctx, probe, through, "writing outside the mount", "writing outside the bind mount");
}

/// The last section: the gate still passes each flag the probe measures.
fn gate_section(ctx: &mut Ctx<'_>, probe: &mut Probe) {
    ctx.put_out(b"\n== the probe still describes the real gate ==\n");
    let gate = {
        let mut path = ctx.script_dir().into_os_string().as_bytes().to_vec();
        path.extend_from_slice(b"/../bin/maintainer-merge");
        PathBuf::from(OsStr::from_bytes(&path))
    };
    for flag in GATE_FLAGS {
        let holds = match ctx.host().read(&gate) {
            Fact::Known(bytes) => shell::contains(&bytes, flag.as_bytes()),
            Fact::Absent => {
                grep_error(ctx, &gate, "No such file or directory");
                false
            }
            Fact::Unreadable(reason) => {
                grep_error(ctx, &gate, &reason);
                false
            }
        };
        if holds {
            probe.ok(ctx, &format!("bin/maintainer-merge still passes {flag}"));
        } else {
            probe.bad(
                ctx,
                format!("the gate no longer passes {flag}; this probe is measuring something else")
                    .as_bytes(),
            );
        }
    }
}

fn attacks(ctx: &mut Ctx<'_>, probe: &mut Probe) -> i32 {
    ctx.put_out(b"== what an adversarial pull request would try, inside the gate ==\n");
    isolation_attacks(ctx, probe);
    resource_attacks(ctx, probe);
    host_attacks(ctx, probe);
    gate_section(ctx, probe);
    ctx.put_out(format!("\n  {} stopped, {} got through\n", probe.passed, probe.failed).as_bytes());
    i32::from(probe.failed > 0)
}

fn grep_error(ctx: &mut Ctx<'_>, path: &Path, reason: &str) {
    let mut text = b"grep: ".to_vec();
    text.extend_from_slice(path.as_os_str().as_bytes());
    text.extend_from_slice(format!(": {reason}\n").as_bytes());
    ctx.put_err(&text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc_zero_is_the_text_rc_zero_anywhere() {
        assert!(rc_zero(b"rc=0\n"));
        assert!(rc_zero(b"connected? no. exit status rc=0 was only a label\n"));
        assert!(!rc_zero(b"rc=10\n"));
        assert!(!rc_zero(b"rc=1\nrc=2\n"));
    }

    #[test]
    fn ran_is_a_line_that_starts_with_ran() {
        assert!(ran(b"ran\nrc=0\n"));
        assert!(!ran(b"sh: /tmp/x: Permission denied (it never ran)\nrc=126\n"));
        assert!(!ran(b" ran\n"));
    }

    #[test]
    fn caps_empty_needs_only_zeros_after_the_marker_to_the_end_of_the_line() {
        assert!(caps_empty(b"CapEff:\t0000000000000000\n"));
        assert!(caps_empty(b"CapEff:   0000000000000000\n"));
        assert!(caps_empty(b"CapEff:0\n"));
        assert!(!caps_empty(b"CapEff:\t00000000a80425fb\n"));
        assert!(!caps_empty(b"CapEff:\t0000000000000000 \n"));
        assert!(!caps_empty(b"CapEff:\n"));
        assert!(!caps_empty(b""));
        assert!(caps_empty(b"x\nprefix CapEff: 000\n"));
    }

    #[test]
    fn no_new_privs_needs_a_one_after_the_marker_on_its_line() {
        assert!(no_new_privs_set(b"NoNewPrivs:\t1\n"));
        assert!(no_new_privs_set(b"NoNewPrivs:\t0 (see 1)\n"));
        assert!(!no_new_privs_set(b"NoNewPrivs:\t0\n"));
        assert!(!no_new_privs_set(b"1 NoNewPrivs:\t0\n"));
        assert!(!no_new_privs_set(b""));
    }

    #[test]
    fn spawned_is_the_digits_after_the_last_marker_of_the_last_line_that_has_one() {
        assert_eq!(spawned(b"spawned=3000\nspawned=12\n"), b"12");
        assert_eq!(spawned(b"sh: fork: retry\nspawned=511\n"), b"511");
        assert_eq!(spawned(b"spawned=\n"), b"");
        assert_eq!(spawned(b"nothing\n"), b"");
        assert_eq!(spawned(b"a spawned=7 spawned=8x\n"), b"8");
        assert_eq!(spawned(b"spawned=5\nlater line\n"), b"5");
    }

    #[test]
    fn the_fork_boundary_is_1000() {
        assert_eq!(fork_verdict(b"999"), Fork::Stopped(b"999".to_vec()));
        assert_eq!(fork_verdict(b"1000"), Fork::Through(b"1000".to_vec()));
        assert_eq!(fork_verdict(b"4000"), Fork::Through(b"4000".to_vec()));
        assert_eq!(fork_verdict(b""), Fork::Silent);
        assert_eq!(fork_verdict(b"0999"), Fork::Stopped(b"0999".to_vec()));
        assert_eq!(
            fork_verdict(b"99999999999999999999"),
            Fork::Through(b"99999999999999999999".to_vec())
        );
    }

    #[test]
    fn the_capabilities_message_is_the_first_60_bytes_without_newlines() {
        let out = b"Current: cap_chown,cap_dac_override,cap_fowner,cap_fsetid,cap_kill,cap_setgid,cap_setuid\nextra line";
        assert_eq!(
            first_60_without_newlines(out),
            b"Current: cap_chown,cap_dac_override,cap_fowner,cap_fsetid,ca"
        );
        assert_eq!(first_60_without_newlines(b"a\nb\n"), b"ab");
    }

    #[test]
    fn the_lines_carry_the_colours_and_two_spaces() {
        assert_eq!(ok_line("x"), b"  \x1b[32mstopped\x1b[0m  x\n");
        assert_eq!(bad_line(b"y"), b"  \x1b[31mGOT THROUGH\x1b[0m  y\n");
    }

    #[test]
    fn there_are_eleven_attacks_and_eight_gate_flags() {
        assert_eq!(ATTACKS.len(), 11);
        assert_eq!(GATE_FLAGS.len(), 8);
    }
}
