//! `install.sh` in Rust: read the machine into facts, decide, then apply the plan.

use crate::apply::{DrySink, LiveSink, Sink, apply};
use crate::args;
use crate::console::{Console, Exit, Flow};
use crate::env::Env;
use crate::machine::Machine;
use crate::plan::{Decision, Layout, decide_install, decide_rollback, decide_uninstall};
use crate::probe;
use crate::refusal::Refusal;
use mx_ports::Proc;

/// Runs `install` and returns the status it ends with.
pub fn run(env: &Env, machine: &dyn Machine, proc: &dyn Proc, con: &mut Console) -> i32 {
    match flow(env, machine, proc, con) {
        Ok(code) | Err(Exit(code)) => code,
    }
}

fn refuse(con: &mut Console, refusal: &Refusal) -> Flow<i32> {
    con.err(&refusal.stderr())?;
    Ok(refusal.exit_code())
}

fn flow(env: &Env, machine: &dyn Machine, proc: &dyn Proc, con: &mut Console) -> Flow<i32> {
    // `share="$HOME/..."` is line 13, above the loop over the flags, so an unset HOME ends
    // the run before an unknown flag does.
    let Some(home) = env.get("HOME") else {
        return refuse(con, &Refusal::HomeUnset { script: env.argv0.clone(), line: 13 });
    };
    let opts = match args::parse(&env.args) {
        Ok(opts) => opts,
        Err(refusal) => return refuse(con, &refusal),
    };
    let layout = Layout::new(env.script_dir(), home);
    let dry = opts.dry;
    let (decision, layout) = if opts.rollback {
        match probe::rollback_facts(machine, proc, env, layout.clone(), opts) {
            Ok(facts) => (decide_rollback(&facts), layout),
            Err(refusal) => return refuse(con, &refusal),
        }
    } else if opts.uninstall {
        match probe::uninstall_facts(machine, proc, env, layout.clone(), opts) {
            Ok(facts) => (decide_uninstall(&facts), layout),
            Err(refusal) => return refuse(con, &refusal),
        }
    } else {
        match probe::install_facts(machine, proc, env, layout.clone(), opts) {
            Ok(facts) => (decide_install(&facts), layout),
            Err(refusal) => return refuse(con, &refusal),
        }
    };
    let plan = match decision {
        Decision::Refuse(refusal) => return refuse(con, &refusal),
        Decision::Proceed(plan) => plan,
    };
    let mut live;
    let mut dry_sink;
    let sink: &mut dyn Sink = if dry {
        dry_sink = DrySink { machine, layout: &layout };
        &mut dry_sink
    } else {
        live = LiveSink { machine, proc, env, layout: &layout };
        &mut live
    };
    apply(&plan, sink, &layout, con)
}
