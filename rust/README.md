# magent in Rust

A Cargo workspace for the programs that replace magent's shell scripts, and the typed foundation and gate they share.

- `crates/mx-core`: `Fact<T>`, `Refusal`, `NonEmpty<T>`. No I/O.
- `crates/mx-ports`: the `Gh`, `Git`, `Containers`, `Clock`, `Fs`, `Proc` and `Pipe` traits, an in-memory fake for each, and the two real adapters: `RealProc` (a short program whose output the caller reads) and `RealPipe` (a program or a pipeline whose streams go to files, in process groups of their own under one deadline).
- `crates/mx-repo`: `bin/maintainer-repo` (`prune`, `release-check`, `fixup`) as a library and a binary named `maintainer-repo`. `tests/differential.sh` runs the bash program and the Rust binary on generated scenarios and compares everything they print, call and write; `tests/mutants.py` is the mutation proof.

- `crates/mx-run`: `lib/run.sh` as a library and a binary named `run`, which replaces `lib/run.sh` in an assembled tree. The cadence gate, the identity verdicts, the toast text and the prompt are pure modules; `pass` is the lifecycle, step for step. It calls the Python helper `maintainer` through the `Proc` port exactly as the bash does.

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

## mx-backends

`crates/mx-backends` is `lib/backends/*.sh` in Rust. A backend answers the six questions of the bash file (its name, whether it can rehearse, where its rehearsal wall is, which tasks it may run, whether it can run, and what to run) and returns the last answer as a `RunPlan`: the programs, their arguments and environment, the file each reads and the log each appends to. It starts no process. The orchestrator runs the plan through the `Pipe` port. `backend_by_name` matches the four names exactly, so a name like `../x` finds nothing (plan decision D18). `RealHost` answers file questions from the machine and finds a program the way `command -v` does for an executable regular file, and it refuses what `command -v` accepts in two cases: a non-executable file and a FIFO in the search path read as found in the bash and as not found here, so `check` refuses a run that would have failed later with status 126. `FakeHost` answers from a table and panics on a question nobody declared. Its tests are in `crates/mx-backends/tests/`:

- `differential.sh`: for each backend, 40 or more hand-written and seeded scenarios run through the bash function in a child bash and through the Rust plan, both starting the same stubs. The sides must agree on every printed line, the exit status, both output streams, each stub call (argv, the names of every exported variable, the working directory, whether standard output and standard error share one open file description, standard input), the variables the plan exports, and the bytes of every file written. Differences that stay are named decisions and asserted on both sides.
- `mutants.py`: the mutation table. Each mutant changes one line in a copy of the tree and must turn a named test red.

The plan names the opencode key only when the backend read it from `~/.groq-staging-key`, and then only in the environment of the opencode stage. A key the run inherited is the executor's to pass on. No argument, export, log note or `Debug` print holds a key. A path that is not UTF-8 cannot be an argument or an environment value, so a plan that needs one refuses.

## mx-checks

`crates/mx-checks` is the five repository checks in Rust: `check-claims` (`scripts/check_claims.sh`), `shellcheck-sweep` (`scripts/shellcheck-sweep.sh`), `verify-action-pins` (`scripts/verify-action-pins.sh`), `run-evals` (`evals/run-evals.sh`) and `containment-probe` (`tests/containment-probe.sh`). Each is a library function over facts read through two ports, the process port of `mx-ports` and the `Host` trait of this crate (the file system and `command -v`), plus a binary of three lines. `scripts/with-hook-lock.sh` and `.githooks/pre-commit` stay bash: the gate that tests the Rust build must not depend on it.

The sweep keeps the memory cap: the program waits for `MemAvailable`, probes `systemd-run --user --scope`, and runs each linter as `systemd-run ... -p MemoryMax=12G -p MemorySwapMax=0 nice -n 15 shellcheck --severity=warning FILE`. A test pins that chain, in that order, for the probe and for every run.

`verify-action-pins` is the one program that does not repeat its script on a hostile input (decision `P4B-16`). The script puts the repository and the tag of a pin, text from a workflow file, into the path of `gh api` as they are, so `uses: o/..@SHA # v1` or a tag of `v1?x=1` changes the request. The port refuses such a pin without a call, names it on a `FAIL` line, exits 1, and percent-encodes a tag byte that a URL path does not take. Tests, a differential group of ten explained cases and twenty-two mutants pin it.

Its tests are in `crates/mx-checks/tests/`:

- `claims.rs`, `pins.rs`, `sweep.rs`, `evals.rs` and `probe.rs`: each program over fakes (`FakeProc`, `FakeHost`, `FakeTagRefs`), named for the behaviour.
- `blackbox.sh`: the characterization cases `tests/characterization/checks-*.sh` run against an assembled tree three times, with the case count compared to the bash tree; `--with-suite` also runs `tests/run-tests.sh`.
- `differential.sh` and `differential.py`: generated sandboxes run through the bash script and the Rust binary, comparing standard output, standard error, the exit code, the order and environment of every external call and the state tree, over 350 seeded comparisons plus the explained ones.
- `mutants.py`: the mutation table. Each mutant changes one line in a copy of the tree and must turn a named test red.

Where a script does something wrong that nothing asked to change, the port keeps it, a characterization case pins it, and the module docs list it under "Found, not fixed". The `P4B-n` decisions are in `lib.rs` and in each module.

## Running the bash suites against a Rust program

`rust/scripts/assemble-tree.sh DEST [PROGRAM...]` copies the tree to `DEST`, builds each workspace binary whose name is also a file under `bin/` (`maintainer-repo`, `maintainer-doctor`), `run`, which stands for `lib/run.sh`, and the five checks (`check-claims`, `shellcheck-sweep`, `verify-action-pins`, `run-evals`, `containment-probe`), each standing for its script, and puts a wrapper in its place that execs the binary with the wrapper's own path as `argv[0]`. The characterization suite then drives the Rust program:

    rust/scripts/assemble-tree.sh /scratch/tree
    MX_TREE=/scratch/tree bash tests/run-characterization.sh repo-prune.sh repo-release.sh repo-fixup.sh

The helper never writes outside `DEST`: it refuses a tree in which a tracked symlink stands where a directory of the copy must be, and it removes a symlink before it writes in its place. `rust/scripts/test-assemble-tree.sh` builds three throwaway repositories with such symlinks and checks that the files outside stay byte for byte the same.
