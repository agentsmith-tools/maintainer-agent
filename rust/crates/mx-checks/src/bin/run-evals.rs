//! `run-evals`: `evals/run-evals.sh` in Rust. This file holds no logic.

fn main() -> std::process::ExitCode {
    mx_checks::stdio::main_with(mx_checks::evals::run)
}
