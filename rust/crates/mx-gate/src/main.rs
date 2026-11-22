//! The `maintainer-merge` binary. It reads the process environment and the command line and hands
//! both to [`mx_gate::app::run`]; it holds no logic.

use std::collections::BTreeMap;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> =
        std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let argv0 = std::env::args_os()
        .next()
        .map_or_else(|| "maintainer-merge".to_string(), |a| a.to_string_lossy().into_owned());
    let vars: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(n, v)| Some((n.into_string().ok()?, v.into_string().ok()?)))
        .collect();
    let exe = std::env::current_exe().unwrap_or_default();
    let code = mx_gate::app::run(&argv0, &exe, &args, &vars);
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
