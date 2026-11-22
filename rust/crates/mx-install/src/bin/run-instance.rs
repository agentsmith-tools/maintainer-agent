//! `run-instance`: `platform/linux/run-instance.sh` in Rust. It replaces itself with
//! `run.sh` beside it, so the unit's process is the run.

use mx_install::console::Console;
use mx_install::env::Env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let env = Env::from_process();
    let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
    let mut con = Console::new(&mut out, &mut err);
    let code = mx_install::units::run_instance(&env, &mut con);
    ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1))
}
