//! `verify-action-pins`: `scripts/verify-action-pins.sh` in Rust. This file holds no logic.

fn main() -> std::process::ExitCode {
    mx_checks::stdio::main_with(|ctx, args| {
        mx_checks::pins::run(ctx, &mx_checks::pins::GhTagRefs, args)
    })
}
