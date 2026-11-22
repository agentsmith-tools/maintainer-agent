//! `containment-probe` over fakes: the flags and payload of every run, the order of the calls, each
//! verdict in both directions and what a runtime that says nothing does.

mod common;

use common::{Rig, exited, ok};
use mx_checks::host::Host;
use mx_checks::probe::{self, ATTACKS, GATE_FLAGS};
use mx_ports::{ProcOutput, Script};

const SCRIPT: &str = "/t/tests/containment-probe.sh";
const IMG: &str = "docker.io/library/bash:5";
const WORK: &str = "/tmp/tmp.fake000001";

fn run_argv(payload: &str) -> Vec<String> {
    let mut argv: Vec<String> = [
        "podman",
        "run",
        "--rm",
        "--timeout",
        "15",
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
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    argv.push(format!("{WORK}:/repo:z"));
    argv.extend(["-w", "/repo", IMG, "sh", "-c"].iter().map(ToString::to_string));
    argv.push(payload.to_string());
    argv
}

fn script_run(rig: &Rig, attack: usize, answer: ProcOutput) {
    let argv = run_argv(ATTACKS[attack]);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    rig.always(&refs, answer);
}

/// What a contained run prints for each attack.
const CONTAINED: [&str; 11] = [
    "rc=1\n",
    "rc=2\n",
    "rc=1\n",
    "rc=126\n",
    "CapEff:\t0000000000000000\n",
    "NoNewPrivs:\t1\n",
    "spawned=500\n",
    "rc=1\n",
    "",
    "PATH=/usr/bin\n",
    "rc=1\n",
];

/// A rig where podman exists, the image is there and the gate holds every flag, and the runtime
/// answers each attack with what a contained run prints, except where `overrides` say otherwise.
fn contained_with(overrides: Vec<(usize, ProcOutput)>) -> Rig {
    let rig = Rig::new();
    rig.host.put_program("/bin/podman");
    rig.host.put_file("/t/bin/maintainer-merge", GATE_FLAGS.join("\n"));
    rig.always(&["podman", "image", "inspect", IMG], ok(""));
    rig.always(&["podman", "ps", "-q"], ok(""));
    rig.always(&["sleep", "3"], ok(""));
    let mut overrides = overrides;
    for (attack, normal) in CONTAINED.iter().enumerate() {
        let answer = match overrides.iter().position(|(index, _)| *index == attack) {
            Some(at) => overrides.remove(at).1,
            None => ok(normal),
        };
        script_run(&rig, attack, answer);
    }
    rig
}

fn contained() -> Rig {
    contained_with(Vec::new())
}

fn run(rig: &Rig) -> common::Outcome {
    rig.run(SCRIPT, &[("PATH", "/bin")], probe::run)
}

fn stopped(text: &str) -> String {
    format!("  \u{1b}[32mstopped\u{1b}[0m  {text}")
}

fn through(text: &str) -> String {
    format!("  \u{1b}[31mGOT THROUGH\u{1b}[0m  {text}")
}

fn flag_lines() -> Vec<String> {
    GATE_FLAGS
        .iter()
        .map(|flag| stopped(&format!("bin/maintainer-merge still passes {flag}")))
        .collect()
}

fn all_contained_lines() -> Vec<String> {
    let mut lines = vec![
        "== what an adversarial pull request would try, inside the gate ==".to_string(),
        stopped("outbound TCP (--network=none)"),
        stopped("DNS resolution"),
        stopped("writing to the image (--read-only)"),
        stopped("executing from /tmp (noexec)"),
        stopped("capabilities (CapEff is empty)"),
        stopped("privilege escalation (NoNewPrivs=1)"),
        stopped("fork bomb (stopped at 500 processes, cap is 512)"),
        stopped("memory balloon"),
        stopped("infinite loop (--timeout, and no container left behind)"),
        stopped("host credentials (podman passes no host environment)"),
        stopped("writing outside the bind mount"),
        String::new(),
        "== the probe still describes the real gate ==".to_string(),
    ];
    lines.extend(flag_lines());
    lines.push(String::new());
    lines.push("  19 stopped, 0 got through".to_string());
    lines
}

#[test]
fn a_runtime_that_contains_every_attack_gives_nineteen_stopped_lines_and_exit_zero() {
    let rig = contained();
    let outcome = run(&rig);
    assert_eq!(outcome.code, 0, "{outcome:?}");
    assert_eq!(outcome.out_lines(), all_contained_lines());
    assert_eq!(outcome.err, "");
}

#[test]
fn the_calls_come_in_the_probes_order_with_the_gates_flags_and_the_payloads_byte_for_byte() {
    let rig = contained();
    run(&rig);
    let mut want = vec![format!("podman image inspect {IMG}")];
    for payload in &ATTACKS[..8] {
        want.push(run_argv(payload).join(" "));
    }
    want.push("podman ps -q".to_string());
    want.push(run_argv(ATTACKS[8]).join(" "));
    want.push("sleep 3".to_string());
    want.push("podman ps -q".to_string());
    want.push(run_argv(ATTACKS[9]).join(" "));
    want.push(run_argv(ATTACKS[10]).join(" "));
    assert_eq!(rig.calls(), want);
}

#[test]
fn the_mounted_file_holds_host_file_and_the_gate_flags_are_the_eight_the_script_looks_for() {
    let rig = contained();
    run(&rig);
    let writes = rig.host.writes();
    assert_eq!(
        writes,
        vec![(std::path::PathBuf::from(format!("{WORK}/repofile")), b"host file\n".to_vec())]
    );
    assert_eq!(
        GATE_FLAGS,
        [
            "--network=none",
            "--cap-drop=ALL",
            "no-new-privileges",
            "--read-only",
            "noexec",
            "--pids-limit",
            "--memory",
            "--cpus"
        ]
    );
}

#[test]
fn the_work_directory_holds_one_file_while_the_attacks_run_and_is_gone_after() {
    let rig = contained();
    run(&rig);
    let log = rig.host.log();
    assert_eq!(log[0], format!("mktemp -d {WORK}"));
    assert_eq!(log[1], format!("write {WORK}/repofile"));
    assert_eq!(log.last().map(String::as_str), Some(format!("rm -rf {WORK}").as_str()));
    assert!(rig.host.paths().iter().all(|path| !path.starts_with(WORK)));
}

#[test]
fn only_the_credential_attack_hands_the_runtime_the_canary_variables() {
    let rig = contained();
    rig.run(SCRIPT, &[("PATH", "/bin"), ("GH_TOKEN", "ghp_AMBIENT")], probe::run);
    let canary_call = run_argv(ATTACKS[9]).join(" ");
    for effect in rig.log.entries() {
        if let mx_ports::Effect::Proc(mx_ports::ProcEffect::Run(spec)) = effect {
            let line = spec.argv.as_slice().join(" ");
            let token = spec
                .env
                .iter()
                .rev()
                .find(|(name, _)| name == "GH_TOKEN")
                .map(|(_, value)| value.as_str());
            let key = spec
                .env
                .iter()
                .rev()
                .find(|(name, _)| name == "ANTHROPIC_API_KEY")
                .map(|(_, value)| value.as_str());
            if line == canary_call {
                assert_eq!((token, key), (Some("ghp_PROBECANARY0000"), Some("sk-PROBECANARY")));
            } else if line.starts_with("podman run") {
                assert_eq!((token, key), (Some("ghp_AMBIENT"), None), "{line}");
            }
        }
    }
}

fn attack_through(attack: usize, answer: ProcOutput, line: usize, text: &str) {
    let rig = contained_with(vec![(attack, answer)]);
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1, "{outcome:?}");
    let mut want = all_contained_lines();
    want[line] = through(text);
    let last = want.len() - 1;
    want[last] = "  18 stopped, 1 got through".to_string();
    assert_eq!(outcome.out_lines(), want);
}

#[test]
fn outbound_tcp_that_connects_got_through() {
    attack_through(0, ok("rc=0\n"), 1, "outbound TCP to 1.1.1.1:53");
}

#[test]
fn dns_that_resolves_got_through() {
    attack_through(1, ok("rc=0\n"), 2, "DNS resolution");
}

#[test]
fn a_write_to_etc_got_through() {
    attack_through(2, ok("rc=0\n"), 3, "writing to /etc inside the image");
}

#[test]
fn a_script_that_ran_from_tmp_got_through() {
    attack_through(3, ok("ran\nrc=0\n"), 4, "executing from /tmp");
}

#[test]
fn a_nonzero_capability_set_got_through_and_the_message_is_cut_at_60_bytes() {
    attack_through(
        4,
        ok("CapEff:\t00000000a80425fb\n"),
        5,
        "capabilities: CapEff:\t00000000a80425fb",
    );
}

#[test]
fn no_new_privs_zero_got_through_with_the_whole_output() {
    attack_through(5, ok("NoNewPrivs:\t0\n"), 6, "NoNewPrivs is not set: NoNewPrivs:\t0");
}

#[test]
fn a_fork_bomb_at_1000_got_through() {
    attack_through(
        6,
        ok("spawned=1000\n"),
        7,
        "fork bomb spawned 1000 processes; the pid cap is not in force",
    );
}

#[test]
fn a_balloon_that_succeeded_got_through() {
    attack_through(7, ok("rc=0\n"), 8, "allocating 4G in a 256m tmpfs");
}

#[test]
fn a_canary_in_the_container_got_through() {
    attack_through(
        9,
        ok("GH_TOKEN=ghp_PROBECANARY0000\n"),
        10,
        "a host credential reached the container",
    );
}

#[test]
fn the_runtimes_stderr_is_read_too_after_its_stdout() {
    attack_through(1, exited(0, "", "rc=0\n"), 2, "DNS resolution");
}

#[test]
fn the_runtimes_exit_status_is_never_read() {
    let rig = contained_with(vec![(0, exited(125, "rc=1\n", ""))]);
    let outcome = run(&rig);
    assert_eq!(outcome.code, 0, "{outcome:?}");
}

#[test]
fn a_fork_bomb_that_reported_nothing_died_before_it_could_and_999_is_stopped() {
    for (answer, text) in [
        ("", "fork bomb (the container died before it could report)"),
        ("spawned=999\n", "fork bomb (stopped at 999 processes, cap is 512)"),
        ("spawned=3000\nspawned=12\n", "fork bomb (stopped at 12 processes, cap is 512)"),
    ] {
        let outcome = run(&contained_with(vec![(6, ok(answer))]));
        assert_eq!(outcome.code, 0, "{outcome:?}");
        assert_eq!(outcome.out_lines()[7], stopped(text));
    }
}

#[test]
fn a_container_left_behind_by_the_infinite_loop_got_through_and_the_message_counts_them() {
    let rig = Rig::new();
    rig.host.put_program("/bin/podman");
    rig.host.put_file("/t/bin/maintainer-merge", GATE_FLAGS.join("\n"));
    rig.always(&["podman", "image", "inspect", IMG], ok(""));
    rig.proc.script(&["podman", "ps", "-q"], Script::sequence([Ok(ok("")), Ok(ok("c1\nc2\n"))]));
    rig.always(&["sleep", "3"], ok(""));
    for (attack, answer) in CONTAINED.iter().enumerate() {
        script_run(&rig, attack, ok(answer));
    }
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        outcome.out_lines()[9],
        through("an infinite loop left 2 container(s) running after the gate gave up")
    );
}

#[test]
fn a_file_beside_the_work_directory_is_a_write_outside_the_mount() {
    let rig = contained();
    rig.host.put_file("/tmp/escape", "");
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1);
    assert_eq!(outcome.out_lines()[11], through("writing outside the mount"));
}

#[test]
fn a_gate_without_one_flag_reports_it_and_a_missing_gate_reports_all_with_greps_error() {
    for (missing, flag) in GATE_FLAGS.iter().enumerate() {
        let rig = contained();
        let kept: Vec<&str> =
            GATE_FLAGS.iter().enumerate().filter(|(i, _)| *i != missing).map(|(_, f)| *f).collect();
        rig.host.put_file("/t/bin/maintainer-merge", kept.join("\n"));
        let outcome = run(&rig);
        assert_eq!(outcome.code, 1, "{flag}");
        let line = outcome.out_lines()[14 + missing].to_string();
        assert_eq!(
            line,
            through(&format!(
                "the gate no longer passes {flag}; this probe is measuring something else"
            ))
        );
    }
    let rig = contained();
    rig.host.remove_file(std::path::Path::new("/t/bin/maintainer-merge"));
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1);
    assert_eq!(outcome.err_lines().len(), 8);
    assert_eq!(
        outcome.err_lines()[0],
        "grep: /t/tests/../bin/maintainer-merge: No such file or directory"
    );
    assert!(outcome.out.ends_with("  11 stopped, 8 got through\n"), "{}", outcome.out);
}

#[test]
fn a_flag_named_only_in_a_comment_of_the_gate_satisfies_the_check() {
    let rig = contained();
    rig.host.put_file("/t/bin/maintainer-merge", format!("# {}\n", GATE_FLAGS.join(" ")));
    assert_eq!(run(&rig).code, 0);
}

#[test]
fn a_runtime_that_prints_nothing_makes_nine_attacks_read_as_stopped_and_fails_only_two_checks() {
    let rig = contained_with((0..ATTACKS.len()).map(|attack| (attack, ok(""))).collect());
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1);
    let lines = outcome.out_lines();
    assert_eq!(lines[5], through("capabilities: "));
    assert_eq!(lines[6], through("NoNewPrivs is not set: "));
    assert_eq!(lines[7], stopped("fork bomb (the container died before it could report)"));
    assert!(outcome.out.ends_with("  17 stopped, 2 got through\n"), "{}", outcome.out);
}

#[test]
fn an_image_that_is_not_there_is_pulled_quietly_and_one_that_cannot_be_had_stops_the_probe() {
    let missing = Rig::new();
    missing.host.put_program("/bin/podman");
    missing.proc.script(
        &["podman", "image", "inspect", IMG],
        Script::always(Ok(exited(1, "", "no such image"))),
    );
    missing.always(&["podman", "pull", "-q", IMG], exited(1, "", "denied"));
    let outcome = run(&missing);
    assert_eq!(outcome.code, 1);
    assert_eq!(outcome.out, "");
    assert_eq!(outcome.err, format!("cannot obtain {IMG}\n"));
    assert_eq!(
        missing.calls(),
        [format!("podman image inspect {IMG}"), format!("podman pull -q {IMG}")]
    );
}

#[test]
fn with_no_runtime_the_probe_says_it_measured_nothing_and_runs_nothing() {
    let rig = Rig::new();
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1);
    assert_eq!(outcome.err, "no container runtime; this probe measured nothing\n");
    assert!(rig.calls().is_empty());
}

#[test]
fn docker_is_used_when_podman_is_absent_and_podman_is_preferred_when_both_exist() {
    let rig = Rig::new();
    rig.host.put_program("/bin/docker");
    rig.always(&["docker", "image", "inspect", IMG], exited(1, "", ""));
    rig.always(&["docker", "pull", "-q", IMG], exited(1, "", ""));
    run(&rig);
    assert_eq!(rig.calls()[0], format!("docker image inspect {IMG}"));
    let both = Rig::new();
    both.host.put_program("/bin/docker");
    both.host.put_program("/bin/podman");
    both.always(&["podman", "image", "inspect", IMG], exited(1, "", ""));
    both.always(&["podman", "pull", "-q", IMG], exited(1, "", ""));
    run(&both);
    assert_eq!(both.calls()[0], format!("podman image inspect {IMG}"));
}

#[test]
fn the_image_and_the_runtime_timeout_come_from_the_environment() {
    let rig = Rig::new();
    rig.host.put_program("/bin/podman");
    rig.always(&["podman", "image", "inspect", "example.invalid/x:1"], exited(1, "", ""));
    rig.always(&["podman", "pull", "-q", "example.invalid/x:1"], exited(1, "", ""));
    rig.run(SCRIPT, &[("PATH", "/bin"), ("PROBE_IMAGE", "example.invalid/x:1")], probe::run);
    assert_eq!(rig.calls()[0], "podman image inspect example.invalid/x:1");
}

#[test]
fn a_nul_byte_in_an_attacks_output_is_dropped_with_the_shells_warning_and_the_text_joins_up() {
    let rig = contained_with(vec![(1, ok("r\0c=0\n"))]);
    let outcome = run(&rig);
    assert_eq!(outcome.code, 1, "{outcome:?}");
    assert_eq!(
        outcome.err,
        "/t/tests/containment-probe.sh: line 50: warning: command substitution: ignored null byte in input\n"
    );
    assert_eq!(outcome.out_lines()[2], through("DNS resolution"));
}
