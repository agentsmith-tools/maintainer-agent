# magent in Rust

A Cargo workspace for the programs that replace magent's shell scripts, and the typed foundation and gate they share.

- `crates/mx-core`: `Fact<T>`, `Refusal`, `NonEmpty<T>`. No I/O.
- `crates/mx-ports`: the `Gh`, `Git`, `Containers`, `Clock`, `Fs` and `Proc` traits, an in-memory fake for each, and `RealProc`, the one real adapter.
- `crates/mx-repo`: `bin/maintainer-repo` (`prune`, `release-check`, `fixup`) as a library and a binary named `maintainer-repo`. `tests/differential.sh` runs the bash program and the Rust binary on generated scenarios and compares everything they print, call and write; `tests/mutants.py` is the mutation proof.

Run the gate from the repository root: `scripts/rust-gate.sh`. It runs `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked` (more than 15 tests must run), and a probe that adds a crate calling `Command::new` and demands clippy refuse it. CI runs the same script as the `rust` job.

Three rules:

1. Only `mx-ports` starts a process. `clippy.toml` bans `std::process::Command::new`, and `RealProc` holds the one `#[allow]`. Everything else asks a `Proc`.
2. `Refusal::exit_code` has no `_` arm. A new variant does not compile until it has an exit code, and a test fails when a code changes.
3. `Fact::Unreadable` never becomes `Fact::Absent`. A read that failed is not a read that found nothing.

A binary's `main` deals with `SIGPIPE`, because Rust ignores it. Restoring the default action needs `unsafe`, which the workspace forbids, so `maintainer-repo` ends with status 141 (128 + `SIGPIPE`) when a write meets a closed pipe.

`maintainer-repo fixup` removes its scratch clone when SIGHUP, SIGINT or SIGTERM arrives, as the bash program's `EXIT` trap does, and then dies of the signal (status 129, 130, 143). Installing a handler needs `unsafe` too, so the workspace takes it from `signal-hook`, whose API is safe, pinned to one version in `rust/Cargo.toml`.

## mx-doctor

`crates/mx-doctor` is `bin/maintainer-doctor` in Rust: the library holds every check as a decision over facts read through the ports, and the binary is named `maintainer-doctor`. Its tests are in `crates/mx-doctor/tests/`:

- `blackbox.sh`: the doctor characterization cases of `tests/characterization/` run against an assembled tree (`rust/scripts/assemble-tree.sh`), three times, with the four pinned differences named in the script.
- `differential.sh`: generated hosts run through both programs, comparing standard output, standard error, exit code, the order of every external call and the state tree.
- `mutants.py`: the mutation table. Each mutant changes one guard in a copy of the tree and must turn a named test red.

The doctor writes each finding and each diagnostic the moment it is made, so a capture of both streams interleaves as the bash does. It cannot restore the default `SIGPIPE` action without `unsafe`, so when standard output is closed it stops at the next section and exits with status 141, which is what a shell reports for a process the signal killed.

## Running the bash suites against a Rust program

`rust/scripts/assemble-tree.sh DEST [PROGRAM...]` copies the tree to `DEST`, builds each workspace binary whose name is also a file under `bin/` (today `maintainer-repo`), and puts a wrapper in its place that execs the binary with the wrapper's own path as `argv[0]`. The characterization suite then drives the Rust program:

    rust/scripts/assemble-tree.sh /scratch/tree
    MX_TREE=/scratch/tree bash tests/run-characterization.sh repo-prune.sh repo-release.sh repo-fixup.sh

The helper never writes outside `DEST`: it refuses a tree in which a tracked symlink stands where a directory of the copy must be, and it removes a symlink before it writes in its place. `rust/scripts/test-assemble-tree.sh` builds three throwaway repositories with such symlinks and checks that the files outside stay byte for byte the same.
