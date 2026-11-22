# Moving a machine to the Rust programs

The maintainer's programs have Rust builds next to the bash originals: the run orchestrator, the doctor,
`maintainer-repo`, the installers, the five repository checks and, once the gate port lands,
`maintainer-merge`. `scripts/cutover.sh` moves one machine from the bash programs to the Rust ones in
controlled steps and moves it back in one. It is bash on purpose: the way back must not depend on the
programs it may have to back away from.

The order is **build, shadow, live, status, rollback**. Every step refuses before it changes anything,
exits 64 for a usage error and 1 for a refusal, and names what it refused on standard error.

## 1. build

```sh
scripts/cutover.sh build [--out DIR] [--allow-dirty]
```

Builds every Rust program of the workspace that stands for a program of this repository, as a release
binary with the flags `rust/scripts/assemble-tree.sh` uses (`cargo build --release --locked --offline`,
and `--remap-path-prefix` for the home directory, the cargo home and the toolchain, so no binary spells
the path of the machine that built it). The programs land in `DIR` (default
`~/.cache/magent-cutover/build`) with three more files: `SHA256SUMS`, the table of every program's hash,
and `BUILD_INFO`, which names the commit and says whether the tree was clean. The table is printed.

It refuses a dirty tree unless you pass `--allow-dirty`, and `live` refuses a build made from one. It
writes only `DIR` and the cargo target directory (`CARGO_TARGET_DIR`, `rust/target` by default), replaces
only what an earlier build wrote, and refuses a `DIR` that is a symbolic link, is writable by everyone or
holds a file no build wrote.

## 2. shadow

```sh
scripts/cutover.sh shadow [--out DIR] [--accept-skip NAME]... [--timeout SECONDS]
```

Runs each deployed bash program and its Rust build side by side on this machine's profiles and compares
the exit status, standard output, standard error and every call the program makes to a tool that could
reach beyond it. Those calls go to shims the script writes: a `gh` that answers a few planned reads and
refuses every write, a `git` that passes reads through and swallows `fetch`, a `date` that answers one
fixed instant for `date -Is`, and shims for `crontab`, `systemctl`, `systemd-run`, `loginctl`,
`notify-send`, `podman`, the model backends and the network tools. `systemctl` passes read verbs of the
user manager through, and `systemd-run` passes a transient unit it waits for (the doctor's sandbox probe)
or a scope (the linter sweep's memory cap). Each shim logs its calls, and shadow compares the two logs as
well. A call that a shim refuses as a write is counted, and a run with one has no marker.

Shadow decides by reading the code which verbs are read-only. What it runs, and why that is safe:

| Program | What runs | Why it reads only |
|---|---|---|
| `run` | `--show-prompt` for every deployed profile and each of its tasks, with the default prose style and with `raw`; an unknown profile and task | A preview assembles the prompt and prints it. It takes no lock, opens no log and calls no backend. |
| `maintainer-doctor` | `--quick` for every profile, with no profile named and with a bad name | `--quick` skips the container probe, the one part that starts containers. The `gh` calls reach the shim. Only timestamps and process ids are normalised. |
| `maintainer-repo` | `prune --dry-run` and `release-check` for every profile; the usage refusals of the bare command and of `fixup` | Both verbs begin with `git fetch`, which writes refs and reaches the network: the shim swallows it. `fixup` clones a fork, so only its refusals run. |
| `maintainer-merge` | `show`, `show 7`, `show 8`, and `verify-deps` with no arguments, with a bad number and on a pin-bump diff | `gh` is the shim. State, receipt key and repository are scratch directories, so the receipt `verify-deps` writes never lands in the real state. |
| `run-instance` | no instance, and an instance whose profile does not exist | `run.sh` refuses an unknown profile before it does anything. |
| `alert` | an alert and one with notifications off, in a scratch `HOME` | It writes `alerts.log` and calls `notify-send`: both go to the scratch `HOME` and the shim. |
| `install` | `--dry-run`, `--dry-run --timers`, `--uninstall --dry-run`, an unknown flag | A dry run prints `would:` lines and writes nothing. |
| `install-cron` | `--dry-run` for every profile | It lists the crontab through the shim and prints the block it would write. |
| the five checks | `check-claims`, `shellcheck-sweep`, `verify-action-pins`, `run-evals` for every profile, `containment-probe`, on the checkout | They read the tree. `check-claims` runs the offline suite and the sweep runs the linter, so each takes minutes, twice. The probe may inspect an image and run a container that removes itself; the `podman` shim refuses a pull or a build. |

Not compared, because they act: `new-profile` (writes a profile), `install-launchd` (macOS), a real run
(posts, locks, calls a backend), `maintainer-doctor` without `--quick`, `maintainer-repo fixup` on a pull
request, and the real `install`, which `live` exercises.

Shadow writes `shadow-report.txt` in `DIR`: one line per comparison, `PASS`, `DIFF` with the first line
where the two parted, or `SKIP` with the reason, then a summary. Each side of a comparison has five
minutes (the checks have an hour); `--timeout` changes both, and a side that runs out of time is a `DIFF`.
`SIGTERM` stops the side that is running, ends the run and removes the scratch directory.
`CUTOVER_KEEP_SCRATCH=1` keeps that directory, with every comparison's streams and shim logs, for reading. A `SKIP` happens when the build holds no
Rust program for a comparison. It is counted and listed, and it stops the marker unless you name the
program with `--accept-skip`.

The marker, `shadow-ok`, holds the sha256 table of the build that was compared. Shadow writes it only
when every comparison passed, at least one prompt was compared and every expected prompt was, no shim
refused a write, and the deployed tree and the checkout were as they were when the run began. It
removes an old marker before the first comparison, so a run that stops early leaves none.

### What shadow proves, and what it does not

It proves that on this machine's profiles and state, for the verbs above, the Rust build and the bash
program print the same bytes, end with the same status and ask the same tools the same questions. It
compares against the **deployed** bash: if `SHADOW deployed=` and `checkout=` in the report differ, run
`./install.sh` first so that what shadow compared is what `live` will leave beside the Rust programs.

It does not prove anything about a real run: the cadence gate, the lock, the lease, the identity pin, the
backends, the signals and the scrub guard. Those are pinned by the characterization cases and the
differential runs in CI, not here. It does not exercise a real scheduler, a real `gh` or a pull request
on GitHub. A prompt preview does not refresh the tracker, so the context section of the prompt is the
preview's one line.

## 3. live

```sh
scripts/cutover.sh live [--build-dir DIR]
```

Refuses unless `shadow-ok` exists and holds the table of the programs now in `DIR`. A program changed,
added or removed after shadow makes the tables differ. It also refuses a build made from a dirty tree,
from another commit than the checkout's, or a checkout that is dirty, because `install` deploys the
checkout beside the Rust programs; and it refuses when `RUST_STAMP` is already there.

Then it runs `install --rust --rust-bin DIR --dry-run` and prints it, runs the real install, and reads the
machine back: each swapped program is the Rust build by sha256, each saved original (`NAME.sh`, so
`run.sh.sh` for `run.sh`) is the checkout's bash program byte for byte, and `RUST_STAMP` lists exactly
those programs with the build's hashes. Anything else is a failure, printed with the command that undoes it.
`live` does not touch the timers: they run the same unit files, which now start the Rust `run`.

## 4. status

```sh
scripts/cutover.sh status [--build-dir DIR]
```

Prints, for each of the six programs the install swaps, `bash`, `rust` (the file's sha256 matches
`RUST_STAMP`), `changed` (listed in the stamp and no longer the stamped build), `link` (a symbolic link,
not read) or `missing`; whether `RUST_STAMP` is there; and whether `shadow-ok` is `current` for `DIR`
or `stale`. It writes nothing, and it refuses an unreadable or malformed stamp.

## 5. rollback

```sh
scripts/cutover.sh rollback [--dry-run] [--force]
```

Puts each saved bash original back in place of its Rust build and removes `RUST_STAMP`. It needs no
Rust binary, no build directory and no cargo: only bash, `sha256sum`, `mv`, `rm` and `flock`. It reads
the stamp, judges every program before it moves the first, and refuses (changing nothing) when:

- a run holds a profile's lock (exit 69; `--force` goes on),
- the stamp is missing, unreadable, malformed, lists a program twice or lists a path the installer does
  not swap,
- a saved original is missing,
- an installed file is neither the stamped build nor already restored,
- the stamp, an installed program or a saved original is a symbolic link: a rollback never follows a
  link and never replaces one.

A rollback that fails part way can run again: a program whose original is already back is skipped.
`rust/crates/mx-install/tests/cutover-rollback.sh` runs each of these states, and more, through the Rust
`install --rollback` and through `rollback`, and compares the exit status, both streams and every file:
the two ways back are one way.

## How long the bash programs stay

The bash programs stay in the tree, and each swapped program keeps its original beside it, until the
Rust programs have run on the timers for two weeks with `maintainer-doctor` and the merge gate quiet.
The gate's own trial is separate: it runs next to the bash gate, and either one refusing means refuse.

`rust/crates/mx-install/tests/cutover-live.sh` runs build, live, status and rollback with the real
programs in a scratch `HOME`.
