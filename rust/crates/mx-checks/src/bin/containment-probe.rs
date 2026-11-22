//! `containment-probe`: `tests/containment-probe.sh` in Rust. This file holds no logic.

fn main() -> std::process::ExitCode {
    mx_checks::stdio::main_with(|ctx, _args| mx_checks::probe::run(ctx))
}
