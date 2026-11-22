//! `install`: `install.sh` in Rust. The wrapper that stands in for the script gives it the
//! script's path as `argv[0]`, and the checkout is the directory that path names.

use mx_install::console::Console;
use mx_install::env::Env;
use mx_install::machine::RealMachine;
use mx_ports::RealProc;
use std::process::ExitCode;

fn main() -> ExitCode {
    let env = Env::from_process();
    let proc = RealProc::new(env.path(), Vec::new());
    let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
    let mut con = Console::new(&mut out, &mut err);
    let code = mx_install::install::run(&env, &RealMachine, &proc, &mut con);
    ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1))
}
