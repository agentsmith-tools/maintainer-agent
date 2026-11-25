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
- `flock.rs`: the flock check on a real search path. The check is `command -v flock`, so bash decides: a flock file without the execute bit still counts as on `PATH`, a directory or a dangling link named flock does not, and a flock that exits non-zero counts and is never run. `doctor-core.sh` pins the same five cases for both builds.
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

The sweep keeps the memory cap: the program waits for `MemAvailable`, probes `systemd-run --user --scope`, and runs each linter as `systemd-run ... -p MemoryMax=18G -p MemorySwapMax=0 nice -n 15 shellcheck --severity=warning FILE`. A test pins that chain, in that order, for the probe and for every run.

`verify-action-pins` is the one program that does not repeat its script on a hostile input (decision `P4B-16`). The script puts the repository and the tag of a pin, text from a workflow file, into the path of `gh api` as they are, so `uses: o/..@SHA # v1` or a tag of `v1?x=1` changes the request. The port refuses such a pin without a call, names it on a `FAIL` line, exits 1, and percent-encodes a tag byte that a URL path does not take. Tests, a differential group of twelve explained cases and twenty-three mutants pin it. A lone `.` segment in a tag is refused too.

Its tests are in `crates/mx-checks/tests/`:

- `claims.rs`, `pins.rs`, `sweep.rs`, `evals.rs` and `probe.rs`: each program over fakes (`FakeProc`, `FakeHost`, `FakeTagRefs`), named for the behaviour.
- `blackbox.sh`: the characterization cases `tests/characterization/checks-*.sh` run against an assembled tree three times, with the case count compared to the bash tree; `--with-suite` also runs `tests/run-tests.sh`.
- `differential.sh` and `differential.py`: generated sandboxes run through the bash script and the Rust binary, comparing standard output, standard error, the exit code, the order and environment of every external call and the state tree, over 350 seeded comparisons plus the explained ones.
- `signals.sh`: SIGTERM, a group SIGINT (a terminal's Ctrl-C) and a SIGTERM that names the program alone, sent to `containment-probe` while its runtime stub blocks. The stub's child must be gone afterwards on the Rust side (and, for the group signal, on the bash side), with exit 143 or 130 and `TMPDIR` empty. `mx-ports` keeps a registry of live children (`forward_signal_to_children`, `refuse_new_children`) for it.
- `mutants.py`: the mutation table. Each mutant changes one line in a copy of the tree and must turn a named test red.

Where a script does something wrong that nothing asked to change, the port keeps it, a characterization case pins it, and the module docs list it under "Found, not fixed". The `P4B-n` decisions are in `lib.rs` and in each module.

## mx-install

`crates/mx-install` is `install.sh`, `new-profile.sh` and the four platform scripts in Rust. The library separates deciding from doing: probes read the machine into facts, a pure function turns the facts into a `Plan` or a `Refusal`, and `apply` performs the plan through a sink. The live sink changes the machine. The dry sink answers each step with `would: ...` and changes nothing, so `--dry-run` runs the code of a real run up to the one place where a step touches the machine. A refusal comes before the first step, so a run that refuses has written nothing.

The crate builds six binaries, each standing for a script: `install` (`install.sh`), `new-profile` (`new-profile.sh`), `install-cron` (`platform/posix/install-cron.sh`), `install-launchd` (`platform/macos/install-launchd.sh`), `alert` (`platform/linux/alert.sh`) and `run-instance` (`platform/linux/run-instance.sh`). `rust/scripts/assemble-tree.sh DEST install new-profile ...` swaps them into a copy of the tree.

`cp`, `cp -r`, `rm`, `chmod +x`, `mkdir -p`, `touch` and `mv` run in this process (`coreutils.rs`) with the caller's umask, the way the tools treat links and existing files. The part of GNU sed that `new-profile.sh` hands user text to (a replacement, where `\n` is a newline and `\1` is an error) is reproduced in `sedsub.rs` with the positions sed reports.

Two flags are new and off by default. `install --rust --rust-bin DIR` installs prebuilt Rust binaries from `DIR` over the bash programs, keeps each bash original beside it as `NAME.sh`, and writes `RUST_STAMP` with the sha256 of each installed binary in the format `sha256sum -c` reads. `install --rollback` puts every saved original back and removes the stamp. Its tests are in `tests/uninstall_flow.rs`.

Tests in `crates/mx-install/tests/`:

- `plan.rs`, `apply_fake.rs`: the decisions and the sinks, with no machine and with a machine that records every change (and panics on one when a test forbids them).
- `install_flow.rs`, `uninstall_flow.rs`, `scaffold_flow.rs`, `scheduler_flow.rs`, `coreutils.rs`, `umask.rs`: each program against real files in a scratch directory, with every tool (`systemctl`, `flock`, `git`, `date`, `python3`, `crontab`, `notify-send`) scripted. `scaffold_flow.rs` compares its output with the golden files of the characterization suite.
- `blackbox.sh`: the install characterization cases against an assembled tree, three runs, with the two pinned differences named in the script. Every list of failing cases is sorted under the C locale before `comm` reads it, and every difference is printed, not only the first. `blackbox.sh --self-test` builds no tree: it runs a stand-in suite with the real `find` and with a `find` that reverses its NUL-separated records, and checks that the verdict and the report do not move. `tests/run-tests.sh` runs it.
- `differential.sh`: generated hosts run through both programs, comparing standard output, standard error, exit status, the order of every external call and the state of HOME and of the tree.
- `mutants.py`: the mutation table. Each mutant changes one guard in a copy of the tree and must turn a named test red.

## mx-suite, mx-sandbox, mx-gate

The merge-gate port starts with its interface: types, traits, fakes and tests, and no behaviour the bash does not already have in one line. Three agents write against it in parallel, so a signature changes only in a pull request all three read. The pull request that added it carries the decision register (`P30-n`) and the bash functions each type replaces.

- `crates/mx-suite`: the suite plugin interface. A suite is a bash file in `verify.d/`; the Rust gate asks it questions by starting `bash` through `Proc`, one question per process, and never sources one in its own address space. `SuiteName::parse` is plan decision F3: a name from the command line or from a receipt is validated, so it cannot reach outside `verify.d/`. `SuiteHost` and `Suite` are the ports; `FakeSuiteHost` and `FakeSuite` answer from a table and record every call. `ProdGlobs` holds the profile's production paths.
- `crates/mx-sandbox`: the steps of `verify` that touch files and containers. `ContainerSpec` carries the exact `podman run` flag set of the verify run and of the networked prefetch as data, `Limits::resolve` the precedence of the resource figures, `ExtractedTree` then `ContainedTree` the proof that a tree arrived whole and holds no link that leaves it, and `Mutated` the proof that the sed mutation changed a file. `Sandbox` is the port and `FakeSandbox` records its calls. No process starts here.
- `crates/mx-gate`: the pull request typestate `Pr<Unverified>`, `Pr<Verified>`, `Pr<Receipted>`, `Pr<Mergeable>`, with private fields and a `compile_fail` example for each illegal transition; `Merger::merge` takes a `Pr<Mergeable>` and exists only when the run posts. `GateDecision` is every refusal of `maintainer-merge merge` with its exact text, and a test reads `bin/maintainer-merge` to check each line of each message against a `die` there. `PrNumber::parse` is decision F2, `ProductionDiff::from_git` is F1 (a failed `git diff` is not an empty one). `GhPort`, `IdentityPort` and `Helper` are the ports `merge` reads through, each with a fake. `deps` holds the types of `verify-deps`: the classifier's verdict, the `Net` port, and `Checked`, the proof that every pin, image and package the classifier declared was inspected.

## Running the bash suites against a Rust program

`rust/scripts/assemble-tree.sh [--plain] DEST [PROGRAM...]` copies the tree to `DEST`, builds each workspace binary whose name is also a file under `bin/` (`maintainer-repo`, `maintainer-doctor`), and `run`, which stands for `lib/run.sh`, and puts a wrapper in its place that execs the binary with the wrapper's own path as `argv[0]`. The characterization suite then drives the Rust program, and so does the whole oracle:

    rust/scripts/assemble-tree.sh /scratch/tree
    MX_TREE=/scratch/tree bash tests/run-characterization.sh repo-prune.sh repo-release.sh repo-fixup.sh
    MX_SKIP_SOURCE_COUPLED=1 MX_TREE=/scratch/tree bash tests/run-tests.sh

`--plain` builds nothing and swaps nothing in: `DEST` is the bash tree, made the same way, so the oracle's result on it and on the Rust tree can be compared case by case.

What `DEST` holds besides the copied files:

- **A repository of its own.** A fresh one-commit git repository (`main`), made after the wrappers are written, so `git ls-files`, the shellcheck sweep and the gate cases have a work tree to ask. The source's `.git` is neither copied nor touched, even when the caller exports `GIT_DIR`, `GIT_WORK_TREE` or `GIT_INDEX_FILE` as a git hook does, and the user's git configuration (hooks, signing, templates) does not reach the commit. `.rust-bin/` is ignored there, so both trees list the same tracked files.
- **A wrapper that passes the caller's environment on.** The bash programs hand every variable to the programs they start, and the Rust ones hand on an allowlist plus the names in `MAINTAINER_CHILD_ENV`. The wrapper puts the name of every variable the caller exported in `MAINTAINER_CHILD_ENV` (after the names already there), so a stub that reads `FX`, `DOC_LOG` or `CZ_CASE_DIR` sees it. The allowlist itself stays covered by the crates' own tests. `run` passes its whole environment and needs no such line.
- **No machine path.** The release builds run with `--remap-path-prefix` for the home directory, the cargo home and the toolchain, so a binary carries none of them, and the wrapper spells the binary's absolute path in hex. `tests/run-tests.sh` refuses a tree in which any file spells the user's name, and that name is usually part of `DEST`'s path. A build in another place needs the same flags: `path_free_flags` in the script is the one place that computes them.

The helper never writes outside `DEST`: it refuses a tree in which a tracked symlink stands where a directory of the copy must be, and it removes a symlink before it writes in its place. `rust/scripts/test-assemble-tree.sh` builds throwaway repositories and checks all of the above: the symlink cases leave the files outside byte for byte the same, the repository has one commit and lists the source's files, the source repository is unchanged under a hostile `GIT_*` environment and git configuration, the wrapper names every exported variable, no file below `DEST` spells a directory name above it, and a binary built below a stand-in home and cargo home carries neither (an unflagged build of the same crate carries both).
