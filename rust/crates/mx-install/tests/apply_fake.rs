//! The sinks of `apply`, against a machine that answers from tables and records every change.

mod common;

use common::{Answer, ScriptedProc};
use mx_install::apply::{DrySink, LiveSink, apply};
use mx_install::args::Opts;
use mx_install::console::{Console, Exit};
use mx_install::env::Env;
use mx_install::machine::{Change, FakeMachine};
use mx_install::plan::{
    Cmd, Decision, Layout, OnFail, Plan, Step, UninstallFacts, decide_uninstall,
};
use std::ffi::OsStr;
use std::path::PathBuf;

fn layout() -> Layout {
    Layout::new(PathBuf::from("/co"), OsStr::new("/h"))
}

fn env() -> Env {
    Env::new("/co/install.sh", &[], "/h", &[("HOME", "/h"), ("PATH", "/bin")])
}

fn ufacts() -> UninstallFacts {
    UninstallFacts {
        layout: layout(),
        opts: Opts { uninstall: true, ..Opts::default() },
        assume_yes: true,
        live_timers: None,
        systemd_user: false,
        timer_units: Vec::new(),
        unit_files: vec![PathBuf::from("/h/.config/systemd/user/maintainer@.service")],
        stamps: Vec::new(),
        darwin: false,
        plists: Vec::new(),
        cron_removal_possible: false,
        cron_script: PathBuf::from("/co/platform/posix/install-cron.sh"),
        deployed_profiles: Vec::new(),
        installed_commands: vec![
            PathBuf::from("/h/.local/bin/maintainer"),
            PathBuf::from("/h/.local/bin/maintainer-merge"),
        ],
        rollback_copies: Vec::new(),
        kept: Vec::new(),
    }
}

fn plan_of(decision: Decision) -> Plan {
    match decision {
        Decision::Proceed(plan) => plan,
        Decision::Refuse(refusal) => panic!("refused: {refusal:?}"),
    }
}

struct Ran {
    result: Result<i32, Exit>,
    out: String,
    err: String,
}

fn live(machine: &FakeMachine, proc: &ScriptedProc, plan: &Plan) -> Ran {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let env = env();
    let layout = layout();
    let result = {
        let mut con = Console::new(&mut out, &mut err);
        let mut sink = LiveSink { machine, proc, env: &env, layout: &layout };
        apply(plan, &mut sink, &layout, &mut con)
    };
    Ran {
        result,
        out: String::from_utf8_lossy(&out).into_owned(),
        err: String::from_utf8_lossy(&err).into_owned(),
    }
}

fn exec_log(machine: &FakeMachine) -> Vec<String> {
    machine
        .changes()
        .into_iter()
        .filter_map(|change| match change {
            Change::Exec(text) => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn a_share_directory_that_cannot_be_removed_is_listed_as_a_failed_step_and_the_run_ends_with_0() {
    // The bash case stubs `rm` on PATH; the port removes in this process, so the failure is
    // injected into the machine (decision P4A-rm).
    let machine = FakeMachine::new();
    machine.script(
        "rm -rf /h/.local/share/maintainer",
        1,
        "rm: cannot remove '/h/.local/share/maintainer': Permission denied\n",
    );
    let ran = live(&machine, &ScriptedProc::new(), &plan_of(decide_uninstall(&ufacts())));
    assert_eq!(ran.result, Ok(0));
    assert_eq!(
        ran.err,
        "rm: cannot remove '/h/.local/share/maintainer': Permission denied\n\n  run rm -rf /h/.local/share/maintainer\n"
    );
    assert!(ran.out.contains("  SOME STEPS FAILED. This deployment is part-removed, and that is the\n  state to fix rather than to leave:\n  \n"));
    assert!(ran.out.contains("  removed: /h/.local/share/maintainer, the five commands in /h/.local/bin, and every scheduler entry.\n"));
    assert!(
        exec_log(&machine).contains(&"rm -f /h/.local/bin/maintainer".to_string()),
        "the commands were removed before"
    );
}

#[test]
fn a_recorded_failure_lets_the_next_step_run_and_an_aborting_one_ends_the_run_with_its_status() {
    let machine = FakeMachine::new();
    machine.script("rm -f /h/.config/systemd/user/maintainer@.service", 1, "rm: nope\n");
    let ran = live(&machine, &ScriptedProc::new(), &plan_of(decide_uninstall(&ufacts())));
    assert_eq!(ran.result, Ok(0));
    assert_eq!(
        exec_log(&machine).len(),
        4,
        "the unit, two commands and the share: {:?}",
        exec_log(&machine)
    );

    let machine = FakeMachine::new();
    machine.script("rm -f /h/.local/bin/maintainer", 3, "rm: boom\n");
    let ran = live(&machine, &ScriptedProc::new(), &plan_of(decide_uninstall(&ufacts())));
    assert_eq!(
        ran.result,
        Err(Exit(3)),
        "a command removal is not a recorded step: set -e ends the script"
    );
    assert_eq!(
        exec_log(&machine),
        vec!["rm -f /h/.config/systemd/user/maintainer@.service", "rm -f /h/.local/bin/maintainer"]
    );
    assert!(!ran.out.contains("removed:"));
}

#[test]
fn a_dry_sink_performs_no_change_on_a_machine_that_forbids_them_and_says_would_for_each_command() {
    let machine = FakeMachine::new().forbid_changes();
    let plan = plan_of(decide_uninstall(&ufacts()));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let layout = layout();
    let result = {
        let mut con = Console::new(&mut out, &mut err);
        let mut sink = DrySink { machine: &machine, layout: &layout };
        apply(&plan, &mut sink, &layout, &mut con)
    };
    assert_eq!(result, Ok(0));
    let out = String::from_utf8(out).unwrap();
    for cmd in [
        "rm -f /h/.config/systemd/user/maintainer@.service",
        "rm -f /h/.local/bin/maintainer",
        "rm -rf /h/.local/share/maintainer",
    ] {
        assert!(out.contains(&format!("  would: {cmd}\n")), "{cmd}: {out}");
    }
    assert!(err.is_empty());
    assert!(machine.changes().is_empty());
}

#[test]
fn a_dry_sink_would_stamp_and_would_mint_and_a_live_sink_does_both_in_that_order() {
    let plan = Plan { steps: vec![Step::Stamp, Step::MintKey], exit: 0 };
    let machine = FakeMachine::new().forbid_changes();
    let layout = layout();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    {
        let mut con = Console::new(&mut out, &mut err);
        let mut sink = DrySink { machine: &machine, layout: &layout };
        apply(&plan, &mut sink, &layout, &mut con).unwrap();
    }
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "  would stamp /h/.local/share/maintainer/VERSION\n  would mint /h/.local/share/maintainer/receipt.key if absent\n"
    );
    let machine = FakeMachine::new();
    machine.set_hex(&"cd".repeat(32));
    let ran = live(&machine, &ScriptedProc::new(), &plan);
    assert_eq!(ran.result, Ok(0));
    let changes = machine.changes();
    // The redirection opens VERSION (empty) before the stamp is made, then the stamp is written.
    assert_eq!(
        changes[0],
        Change::Write(PathBuf::from("/h/.local/share/maintainer/VERSION"), Vec::new())
    );
    assert!(
        matches!(&changes[1], Change::Write(path, text) if path.ends_with("VERSION") && text.starts_with(b"version="))
    );
    assert_eq!(
        changes[2],
        Change::WritePrivate(
            PathBuf::from("/h/.local/share/maintainer/receipt.key"),
            "cd".repeat(32).into_bytes()
        )
    );
    assert_eq!(ran.out, "  minted a receipt-signing key\n");
    assert!(
        !ran.out.contains(&"cd".repeat(32)) && !ran.err.contains(&"cd".repeat(32)),
        "the key is never printed"
    );
}

#[test]
fn a_key_that_exists_is_left_alone_and_nothing_is_minted() {
    let machine = FakeMachine::new();
    machine.put("/h/.local/share/maintainer/receipt.key", "existing");
    let plan = Plan { steps: vec![Step::MintKey], exit: 0 };
    let ran = live(&machine, &ScriptedProc::new(), &plan);
    assert_eq!(ran.result, Ok(0));
    assert!(machine.changes().is_empty());
    assert_eq!(ran.out, "");
}

#[test]
fn a_key_that_cannot_be_written_ends_the_run_with_1_and_leaves_an_empty_file_the_next_run_keeps() {
    // Pins current behaviour: the redirection creates the file before the pipeline fills it,
    // so a failed fill leaves an empty receipt.key, and the next install finds a file and
    // keeps it (listed under "Found, not fixed").
    let machine = FakeMachine::new();
    machine.fail_writes_to("/h/.local/share/maintainer/receipt.key");
    let plan = Plan { steps: vec![Step::MintKey], exit: 0 };
    let ran = live(&machine, &ScriptedProc::new(), &plan);
    assert_eq!(ran.result, Err(Exit(1)));
    assert_eq!(
        ran.err,
        "/co/install.sh: line 200: /h/.local/share/maintainer/receipt.key: Permission denied\n"
    );
    assert!(!ran.out.contains("minted"));
    assert_eq!(
        machine.peek(std::path::Path::new("/h/.local/share/maintainer/receipt.key")),
        Some(Vec::new())
    );
    let again = live(&machine, &ScriptedProc::new(), &plan);
    assert_eq!(again.result, Ok(0), "the empty file counts as a key");
}

#[test]
fn a_stamp_that_cannot_be_written_ends_the_run_with_1_naming_the_file() {
    let machine = FakeMachine::new();
    machine.fail_writes_to("/h/.local/share/maintainer/VERSION");
    let plan = Plan { steps: vec![Step::Stamp], exit: 0 };
    let ran = live(&machine, &ScriptedProc::new(), &plan);
    assert_eq!(ran.result, Err(Exit(1)));
    assert_eq!(
        ran.err,
        "/co/install.sh: line 190: /h/.local/share/maintainer/VERSION: Permission denied\n"
    );
}

#[test]
fn the_landing_check_ends_a_real_run_with_1_when_no_profile_env_is_deployed() {
    let machine = FakeMachine::new();
    machine.put_dir("/h/.local/share/maintainer/profiles/lonely");
    let plan = Plan { steps: vec![Step::LandingCheck], exit: 0 };
    let ran = live(&machine, &ScriptedProc::new(), &plan);
    assert_eq!(ran.result, Err(Exit(1)));
    assert_eq!(ran.err, "install.sh: profiles did not land where run.sh expects\n");
    machine.put("/h/.local/share/maintainer/profiles/lonely/profile.env", "");
    assert_eq!(live(&machine, &ScriptedProc::new(), &plan).result, Ok(0));
}

#[test]
fn a_scheduler_command_that_exits_non_zero_replays_its_output_and_a_missing_one_says_127() {
    let machine = FakeMachine::new();
    let proc = ScriptedProc::new();
    proc.on("systemctl --user enable --now x.timer", Answer::err(5, "stub: no\n"));
    let step = |name: &str| {
        Step::Run(
            Cmd::Systemctl(vec!["--user".into(), "enable".into(), "--now".into(), name.into()]),
            OnFail::Abort,
        )
    };
    let ran = live(&machine, &proc, &Plan { steps: vec![step("x.timer")], exit: 0 });
    assert_eq!((ran.result, ran.err.as_str()), (Err(Exit(5)), "stub: no\n"));
    proc.on_program("systemctl", Answer::missing());
    let ran = live(&machine, &proc, &Plan { steps: vec![step("y.timer")], exit: 0 });
    assert_eq!(
        (ran.result, ran.err.as_str()),
        (Err(Exit(127)), "/co/install.sh: line 28: systemctl: command not found\n")
    );
    assert!(machine.changes().is_empty(), "a scheduler command is a process, not a file change");
}

#[test]
fn a_swap_saves_the_original_then_installs_the_build_and_a_failed_copy_ends_the_run_before_the_install()
 {
    use mx_install::plan::RustProgram;
    let program = RustProgram {
        build: "maintainer-doctor",
        installed: PathBuf::from("/h/.local/bin/maintainer-doctor"),
        backup: PathBuf::from("/h/.local/bin/maintainer-doctor.sh"),
        source: PathBuf::from("/b/maintainer-doctor"),
        linux_only: false,
    };
    let plan = Plan { steps: vec![Step::Swap(program)], exit: 0 };
    let machine = FakeMachine::new();
    machine.put("/h/.local/bin/maintainer-doctor", "rust build");
    let ran = live(&machine, &ScriptedProc::new(), &plan);
    assert_eq!(ran.result, Ok(0));
    assert_eq!(
        exec_log(&machine),
        vec![
            "cp /h/.local/bin/maintainer-doctor /h/.local/bin/maintainer-doctor.sh",
            "chmod +x /h/.local/bin/maintainer-doctor.sh",
            "install -m 755 /b/maintainer-doctor /h/.local/bin/maintainer-doctor",
        ]
    );
    let machine = FakeMachine::new();
    machine.script(
        "cp /h/.local/bin/maintainer-doctor /h/.local/bin/maintainer-doctor.sh",
        1,
        "cp: no\n",
    );
    let ran = live(&machine, &ScriptedProc::new(), &plan);
    assert_eq!(ran.result, Err(Exit(1)));
    assert_eq!(
        exec_log(&machine).len(),
        1,
        "the build is not installed over an original that was not saved"
    );
}

#[test]
fn an_unreadable_profile_step_in_a_kept_line_prints_sed_s_error_then_an_empty_line() {
    let failure = mx_install::plan::kept_line(Some(b"STATE_DIR=\"$HOME/x\""), OsStr::new("/a\\1"))
        .unwrap_err();
    let plan = Plan { steps: vec![Step::KeptLine(Err(failure))], exit: 0 };
    let ran = live(&FakeMachine::new(), &ScriptedProc::new(), &plan);
    assert_eq!(
        ran.err,
        "sed: -e expression #1, char 32: invalid reference \\1 on `s' command's RHS\n"
    );
    assert_eq!(ran.out, "           \n");
}
