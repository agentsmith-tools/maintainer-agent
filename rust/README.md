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

## mx-suite, mx-sandbox, mx-gate

The merge-gate port starts with its interface: types, traits, fakes and tests, and no behaviour the bash does not already have in one line. Three agents write against it in parallel, so a signature changes only in a pull request all three read. The pull request that added it carries the decision register (`P30-n`) and the bash functions each type replaces.

- `crates/mx-suite`: the suite plugin interface. A suite is a bash file in `verify.d/`; the Rust gate asks it questions by starting `bash` through `Proc`, one question per process, and never sources one in its own address space. `SuiteName::parse` is plan decision F3: a name from the command line or from a receipt is validated, so it cannot reach outside `verify.d/`. `SuiteHost` and `Suite` are the ports; `FakeSuiteHost` and `FakeSuite` answer from a table and record every call. `ProdGlobs` holds the profile's production paths.
- `crates/mx-sandbox`: the steps of `verify` that touch files and containers. `ContainerSpec` carries the exact `podman run` flag set of the verify run and of the networked prefetch as data, `Limits::resolve` the precedence of the resource figures, `ExtractedTree` then `ContainedTree` the proof that a tree arrived whole and holds no link that leaves it, and `Mutated` the proof that the sed mutation changed a file. `Sandbox` is the port and `FakeSandbox` records its calls. No process starts here.
- `crates/mx-gate`: the pull request typestate `Pr<Unverified>`, `Pr<Verified>`, `Pr<Receipted>`, `Pr<Mergeable>`, with private fields and a `compile_fail` example for each illegal transition; `Merger::merge` takes a `Pr<Mergeable>` and exists only when the run posts. `GateDecision` is every refusal of `maintainer-merge merge` with its exact text, and a test reads `bin/maintainer-merge` to check each line of each message against a `die` there. `PrNumber::parse` is decision F2, `ProductionDiff::from_git` is F1 (a failed `git diff` is not an empty one). `GhPort`, `IdentityPort` and `Helper` are the ports `merge` reads through, each with a fake. `deps` holds the types of `verify-deps`: the classifier's verdict, the `Net` port, and `Checked`, the proof that every pin, image and package the classifier declared was inspected.

## Running the bash suites against a Rust program

`rust/scripts/assemble-tree.sh DEST [PROGRAM...]` copies the tree to `DEST`, builds each workspace binary whose name is also a file under `bin/` (`maintainer-repo`, `maintainer-doctor`), and `run`, which stands for `lib/run.sh`, and puts a wrapper in its place that execs the binary with the wrapper's own path as `argv[0]`. The characterization suite then drives the Rust program:

    rust/scripts/assemble-tree.sh /scratch/tree
    MX_TREE=/scratch/tree bash tests/run-characterization.sh repo-prune.sh repo-release.sh repo-fixup.sh

The helper never writes outside `DEST`: it refuses a tree in which a tracked symlink stands where a directory of the copy must be, and it removes a symlink before it writes in its place. `rust/scripts/test-assemble-tree.sh` builds three throwaway repositories with such symlinks and checks that the files outside stay byte for byte the same.
