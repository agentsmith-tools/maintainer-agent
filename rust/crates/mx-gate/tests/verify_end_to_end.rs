//! The wired gate from the outside: the `maintainer-merge` binary, a real git repository, the real
//! suite host, selector and sandbox, and a container runtime that is a script. The script runs the
//! suite's command on the host, in the directory the gate mounted at `/repo`, so a pinned mutation
//! (a test that fails with the mutation applied) and an unpinned one (a test that still passes)
//! behave as they do in a container, without one.
//!
//! One test, `with_podman_a_pinned_mutation_records_a_receipt_and_an_unpinned_one_is_refused`, uses
//! the real `podman` and the image `docker.io/library/python:3-slim`, which must already be on the
//! machine (nothing is pulled). It is compiled only with the feature `podman-e2e`:
//! `cargo test -p mx-gate --features podman-e2e --test verify_end_to_end`. The Rust gate refuses an
//! ignored test, and a test that skips itself reads as green, so there is no skipped form.
//!
//! Every process is started through `RealProc`, because only `mx-ports` starts processes.

use mx_ports::{Proc, ProcExit, ProcSpec, RealProc};
use mx_receipt::{Key, Stored};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SYSTEM_PATH: &str = "/usr/bin:/bin";
const KEY_TEXT: &[u8] = b"test-key-material\n";

/// A test that passes when the mutation is applied to `prog.sh`: it does not look at the answer.
const UNPINNED: &str = "sh prog.sh >/dev/null && echo '1 passed'\n";
/// A test that fails when `prog.sh` stops printing 5.
const PINNED: &str = "out=$(sh prog.sh)\nif [ \"$out\" = 5 ]; then echo '1 passed'; else echo \"assertion failed: got $out\"; exit 1; fi\n";

const SUITE: &str = r#"suite_covers() { case "$1" in prog.sh|test.sh) return 0 ;; esac; return 1; }
suite_image() { printf 'docker.io/library/python:3-slim'; }
suite_needs() { printf 'sh'; }
suite_mutate_glob() { printf '%s\n' 'prog.sh'; }
suite_command() { printf 'sh test.sh %s\n' "$1"; }
suite_ran() { local n; n="$(sed -n 's/^ *\([0-9]\+\) passed.*/\1/p' "$1" | tail -1)"; printf '%s' "${n:-0}"; }
"#;

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

struct World {
    root: PathBuf,
    tree: PathBuf,
    stubs: PathBuf,
    repo: PathBuf,
    state: PathBuf,
    profile: PathBuf,
    podman_log: PathBuf,
    merge_log: PathBuf,
    head_file: PathBuf,
    tamper: PathBuf,
    env_log: PathBuf,
    search: String,
    extra_env: Vec<(String, String)>,
    profile_dir_in_env: bool,
}

fn git_env() -> Vec<(String, String)> {
    [
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_AUTHOR_NAME", "t"),
        ("GIT_AUTHOR_EMAIL", "t@example.invalid"),
        ("GIT_COMMITTER_NAME", "t"),
        ("GIT_COMMITTER_EMAIL", "t@example.invalid"),
        ("GIT_TERMINAL_PROMPT", "0"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
    .collect()
}

fn run_program(search: &str, env: Vec<(String, String)>, program: &str, args: &[&str]) -> Run {
    let proc = RealProc::new(search, env);
    let spec = ProcSpec::command(program, args.iter().copied(), Duration::from_mins(5));
    let out = proc.run(&spec).expect("the program started");
    let code = match out.exit {
        ProcExit::Code(code) => code,
        ProcExit::TimedOut => panic!("{program} ran past its limit"),
    };
    Run {
        code,
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn write_exec(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

impl World {
    /// A repository whose pull request #7 holds `prog.sh` (prints 5) and `test.sh` (`test_sh`),
    /// a profile with one suite, and stubs for `gh` and `podman`.
    fn new(name: &str, test_sh: &str) -> Self {
        let base =
            std::env::var_os("CARGO_TARGET_TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);
        let root = base.join(format!("e2e-{name}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&root));
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let world = Self {
            tree: root.join("tree"),
            stubs: root.join("stubs"),
            repo: root.join("repo"),
            state: root.join("state"),
            profile: root.join("profile"),
            podman_log: root.join("podman.log"),
            merge_log: root.join("merge.log"),
            head_file: root.join("gh-head"),
            tamper: root.join("tamper"),
            env_log: root.join("podman-env.log"),
            search: format!("{}:{SYSTEM_PATH}", root.join("stubs").display()),
            extra_env: Vec::new(),
            profile_dir_in_env: true,
            root,
        };
        std::fs::create_dir_all(world.tree.join("bin")).unwrap();
        std::fs::create_dir_all(&world.stubs).unwrap();
        std::fs::create_dir_all(world.profile.join("verify.d")).unwrap();
        std::fs::create_dir_all(world.root.join("home")).unwrap();
        std::fs::copy(
            env!("CARGO_BIN_EXE_maintainer-merge"),
            world.tree.join("bin/maintainer-merge"),
        )
        .unwrap();
        // The helper `maintainer lease check` is free.
        write_exec(&world.tree.join("bin/maintainer"), "#!/bin/sh\nexit 0\n");
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../lib"),
            &world.tree.join("lib"),
        );
        std::fs::write(world.root.join("receipt.key"), KEY_TEXT).unwrap();
        std::fs::write(world.profile.join("verify.d/shell.sh"), SUITE).unwrap();
        world.write_stubs();
        world.make_repo(test_sh);
        world
    }

    fn write_stubs(&self) {
        let gh = format!(
            r#"#!/bin/sh
case "$*" in
  *'auth switch'*) exit 0 ;;
  *'api user'*) echo testuser ;;
  *'--json files'*) echo prog.sh ;;
  *headRefOid*) cat '{head}' ;;
  *mergeStateStatus*) echo CLEAN ;;
  *reviewDecision*) echo APPROVED ;;
  *closingIssuesReferences*) ;;
  *'pr checks'*) printf '%s\n' '[{{"name":"ci","bucket":"pass"}}]' ;;
  *'pr merge'*) echo merged >> '{merge}' ;;
esac
"#,
            head = self.head_file.display(),
            merge = self.merge_log.display()
        );
        write_exec(&self.stubs.join("gh"), &gh);
        // A container runtime: it logs the hash of the staged gate binary it finds in the mounted
        // tree, runs the command on the host in that tree, and (when the marker exists) overwrites
        // the staged file after its first run, as a container with a writable mount could.
        let podman = format!(
            r#"#!/bin/sh
log='{log}'
n=0
[ -f "$log" ] && n=$(wc -l < "$log")
n=$((n + 1))
work=''
prev=''
cmd=''
for a in "$@"; do
  case "$prev" in -v) case "$a" in *:/repo:z) work="${{a%:/repo:z}}" ;; esac ;; esac
  prev="$a"
  cmd="$a"
done
staged="$work/.container-target/stage/maintainer-merge"
sum=$(sha256sum "$staged" 2>/dev/null | cut -d' ' -f1)
echo "call $n staged=${{sum:-none}}" >> "$log"
echo "xdg=${{XDG_RUNTIME_DIR:-unset}} dbus=${{DBUS_SESSION_BUS_ADDRESS:-unset}} conf=${{CONTAINERS_CONF:-unset}} token=${{GH_TOKEN:-unset}} other=${{LEAKED:-unset}}" >> '{env_log}'
cd "$work" || exit 125
sh -c "$cmd"
rc=$?
if [ -e '{tamper}' ] && [ "$n" = 1 ] && [ -f "$staged" ]; then echo tampered >> "$staged"; fi
exit $rc
"#,
            log = self.podman_log.display(),
            env_log = self.env_log.display(),
            tamper = self.tamper.display()
        );
        write_exec(&self.stubs.join("podman"), &podman);
    }

    /// Moves the suites to `place` under the world's root and takes `PROFILE_DIR` out of the
    /// environment, so the gate has to find them in the second or third place it looks.
    fn suites_at(&mut self, place: &str) {
        let to = self.root.join(place);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::rename(self.profile.join("verify.d"), to).unwrap();
        self.profile_dir_in_env = false;
    }

    /// Leaves the machine with a `docker` and no `podman`: a stub `docker`, and a tools folder of
    /// links to the programs the gate runs, so the real `podman` in `/usr/bin` is off the path.
    fn only_docker(&mut self) {
        std::fs::rename(self.stubs.join("podman"), self.stubs.join("docker")).unwrap();
        let tools = self.root.join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        for name in [
            "bash",
            "sh",
            "env",
            "git",
            "sed",
            "find",
            "sort",
            "cut",
            "wc",
            "cat",
            "sha256sum",
            "tail",
            "head",
            "grep",
            "awk",
            "mktemp",
            "rm",
            "cp",
            "mkdir",
            "ls",
            "tr",
            "basename",
            "dirname",
            "readlink",
            "chmod",
            "python3",
        ] {
            for dir in ["/usr/bin", "/bin"] {
                let from = Path::new(dir).join(name);
                if from.exists() {
                    std::os::unix::fs::symlink(from, tools.join(name)).unwrap();
                    break;
                }
            }
        }
        self.search = format!("{}:{}", self.stubs.display(), tools.display());
    }

    fn git(&self, args: &[&str]) -> String {
        let mut words = vec!["-C", self.repo.to_str().unwrap()];
        words.extend_from_slice(args);
        let run = run_program(SYSTEM_PATH, git_env(), "/usr/bin/git", &words);
        assert_eq!(run.code, 0, "git {args:?}: {}", run.stderr);
        run.stdout.trim().to_string()
    }

    fn make_repo(&self, test_sh: &str) {
        let origin = self.root.join("origin.git");
        let run = run_program(
            SYSTEM_PATH,
            git_env(),
            "/usr/bin/git",
            &["init", "-q", "-b", "main", self.repo.to_str().unwrap()],
        );
        assert_eq!(run.code, 0, "{}", run.stderr);
        let run = run_program(
            SYSTEM_PATH,
            git_env(),
            "/usr/bin/git",
            &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()],
        );
        assert_eq!(run.code, 0, "{}", run.stderr);
        std::fs::write(self.repo.join("prog.sh"), "echo 5\n").unwrap();
        std::fs::write(self.repo.join("test.sh"), test_sh).unwrap();
        self.git(&["add", "prog.sh", "test.sh"]);
        self.git(&["commit", "-q", "-m", "base"]);
        self.git(&["remote", "add", "origin", origin.to_str().unwrap()]);
        self.publish();
    }

    /// Pushes the repository's head as the pull request's head and tells the `gh` stub.
    fn publish(&self) -> String {
        self.git(&["push", "-q", "origin", "main:refs/pull/7/head"]);
        let head = self.git(&["rev-parse", "HEAD"]);
        std::fs::write(&self.head_file, format!("{head}\n")).unwrap();
        head
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    fn env(&self) -> Vec<(String, String)> {
        let mut env = git_env();
        let set = |name: &str, value: String| (name.to_string(), value);
        env.extend([
            set("HOME", self.root.join("home").display().to_string()),
            set("MAINTAINER_SLUG", "o/r".to_string()),
            set("MAINTAINER_REPO", self.repo.display().to_string()),
            set("MAINTAINER_STATE", self.state.display().to_string()),
            set("MAINTAINER_ACCOUNT", "testuser".to_string()),
            set("MAINTAINER_PROFILE", "t".to_string()),
            set("MAINTAINER_RECEIPT_KEY", self.root.join("receipt.key").display().to_string()),
            set("PROD_GLOBS", "prog.sh".to_string()),
        ]);
        if self.profile_dir_in_env {
            env.push(set("PROFILE_DIR", self.profile.display().to_string()));
        }
        env.extend(self.extra_env.clone());
        env
    }

    fn gate(&self, args: &[&str]) -> Run {
        let program = self.tree.join("bin/maintainer-merge");
        run_program(&self.search, self.env(), program.to_str().unwrap(), args)
    }

    fn verify(&self, filter: &str, mutation: &str) -> Run {
        let head = self.head();
        self.gate(&["verify", "7", &head, filter, mutation])
    }

    fn verify_part(&self, filter: &str, mutation: &str, suite: &str) -> Run {
        let head = self.head();
        self.gate(&["verify-part", "7", &head, filter, mutation, suite])
    }

    fn receipt_path(&self) -> PathBuf {
        self.state.join("receipts/7.json")
    }

    fn receipt(&self) -> Option<Stored> {
        std::fs::read(self.receipt_path()).ok().map(|bytes| Stored::parse(&bytes).unwrap())
    }

    fn containers(&self) -> Vec<String> {
        std::fs::read_to_string(&self.podman_log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn merges(&self) -> usize {
        std::fs::read_to_string(&self.merge_log).map_or(0, |text| text.lines().count())
    }

    /// The sha256 of the gate binary the containers are expected to find.
    fn binary_hash(&self) -> String {
        let run = run_program(
            SYSTEM_PATH,
            Vec::new(),
            "/usr/bin/sha256sum",
            &[self.tree.join("bin/maintainer-merge").to_str().unwrap()],
        );
        run.stdout.split_whitespace().next().unwrap().to_string()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.root));
    }
}

fn key() -> Key {
    Key::from_file_bytes(KEY_TEXT).unwrap()
}

const BITES: &str = "s/echo 5/echo 6/";

#[test]
fn a_pinned_mutation_records_a_signed_observed_receipt_at_the_pull_request_head() {
    let w = World::new("pinned", PINNED);
    let run = w.verify("guard", BITES);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let head = w.head();
    assert_eq!(
        run.stdout,
        format!(
            "  suite: shell (docker.io/library/python:3-slim under podman)\n  running 'guard' unmutated\n  shell suite: evidence of 1 executed unit(s), passing unmutated\n  applying the mutation and re-running\n  receipt recorded for #7 at {} (observed: clean pass, mutated fail)\n",
            &head[..8]
        )
    );
    let stored = w.receipt().expect("a receipt was written");
    assert_eq!(stored.field_text("kind", ""), "observed");
    assert_eq!(stored.field_text("head", ""), head);
    assert_eq!(stored.field_text("suite", ""), "shell");
    assert_eq!(stored.field_text("units_run", ""), "1");
    assert_eq!(stored.field_text("observed_failure", ""), "assertion failed: got 6");
    assert!(
        stored
            .field_text("proof", "")
            .contains("`guard` passes clean and fails under `s/echo 5/echo 6/` (rc=1)"),
        "{}",
        stored.field_text("proof", "")
    );
    stored.check(&key(), "7", true).expect("the receipt carries a valid signature");
    assert_eq!(w.containers().len(), 2, "the clean run and the mutated run");
    assert_eq!(
        std::fs::read_to_string(w.repo.join("prog.sh")).unwrap(),
        "echo 5\n",
        "the mutation never touched the repository, only the extracted tree"
    );
}

#[test]
fn an_unpinned_mutation_is_refused_with_the_bash_text_and_writes_no_receipt() {
    let w = World::new("unpinned", UNPINNED);
    let run = w.verify("guard", BITES);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "maintainer-merge: THE GUARD DOES NOT BITE: 'guard' still passes with the mutation applied.\n  This is the defect the gate exists for. No receipt written.\n"
    );
    assert!(w.receipt().is_none(), "no receipt for a guard that does not bite");
    assert_eq!(w.containers().len(), 2, "both runs happened before the verdict");
}

#[test]
fn a_head_that_is_not_the_pull_request_head_is_refused_before_any_container_starts() {
    let w = World::new("headname", PINNED);
    let run = w.gate(&["verify", "7", "deadbee", "guard", BITES]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        format!("maintainer-merge: head is {}, not the deadbee you named\n", w.head())
    );
    assert!(w.containers().is_empty());
    assert!(w.receipt().is_none());
}

#[test]
fn a_symlink_that_leaves_the_tree_refuses_the_verify_before_any_container_starts() {
    let w = World::new("symlink", PINNED);
    std::os::unix::fs::symlink("/etc/passwd", w.repo.join("lnk")).unwrap();
    w.git(&["add", "lnk"]);
    w.git(&["commit", "-q", "-m", "link"]);
    w.publish();
    let run = w.verify("guard", BITES);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "maintainer-merge: the tree for #7 contains symlink(s) pointing outside itself, and the\n  mutation step runs on the host:\n    lnk -> /etc/passwd\n  Refusing. Review this by reading.\n"
    );
    assert!(w.containers().is_empty());
    assert!(w.receipt().is_none());
}

#[test]
fn a_verified_pull_request_that_has_not_moved_merges() {
    let w = World::new("control", PINNED);
    assert_eq!(w.verify("guard", BITES).code, 0);
    let run = w.gate(&["merge", "7"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(w.merges(), 1, "the merge reached gh");
}

#[test]
fn a_head_that_moved_production_code_after_the_verify_refuses_the_merge() {
    let w = World::new("moved", PINNED);
    assert_eq!(w.verify("guard", BITES).code, 0);
    let verified = w.head();
    std::fs::write(w.repo.join("prog.sh"), "echo 5\necho extra\n").unwrap();
    w.git(&["commit", "-q", "-am", "later"]);
    w.publish();
    let run = w.gate(&["merge", "7"]);
    assert_eq!(run.code, 1);
    assert!(
        run.stderr.contains(&format!(
            "production code changed since the verified head {verified}:\n    prog.sh\n"
        )),
        "{}",
        run.stderr
    );
    assert_eq!(w.merges(), 0);
}

#[test]
fn a_forged_receipt_refuses_the_merge() {
    let w = World::new("forged", PINNED);
    assert_eq!(w.verify("guard", BITES).code, 0);
    let text = std::fs::read_to_string(w.receipt_path()).unwrap();
    assert!(text.contains("assertion failed: got 6"));
    std::fs::write(w.receipt_path(), text.replace("got 6", "got 5")).unwrap();
    let run = w.gate(&["merge", "7"]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "maintainer-merge: the receipt for #7 does not match its signature: it was edited\n  after it was written, or written by something other than the gate.\n"
    );
    assert_eq!(w.merges(), 0);
}

#[test]
fn a_suite_file_that_stopped_covering_the_changed_file_refuses_a_merge_on_a_partial_receipt() {
    let w = World::new("suitefile", PINNED);
    let run = w.verify_part("guard", BITES, "shell");
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        w.receipt().unwrap().field_text("partial", ""),
        "True",
        "verify-part writes a receipt proved in parts"
    );
    std::fs::write(
        w.profile.join("verify.d/shell.sh"),
        SUITE.replace("prog.sh|test.sh", "test.sh"),
    )
    .unwrap();
    let run = w.gate(&["merge", "7"]);
    assert_eq!(run.code, 1);
    assert!(
        run.stderr.contains("the receipt for #7 was earned by: shell\n  Those suites do not run all of the production code this merge would land"),
        "{}",
        run.stderr
    );
    assert_eq!(w.merges(), 0);
}

#[test]
fn a_partial_receipt_whose_suite_still_covers_the_change_merges() {
    let w = World::new("partial", PINNED);
    assert_eq!(w.verify_part("guard", BITES, "shell").code, 0);
    let run = w.gate(&["merge", "7"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(w.merges(), 1);
}

#[test]
fn a_partial_receipt_whose_suite_file_was_deleted_refuses_the_merge() {
    let w = World::new("nosuite", PINNED);
    assert_eq!(w.verify_part("guard", BITES, "shell").code, 0);
    std::fs::remove_file(w.profile.join("verify.d/shell.sh")).unwrap();
    let run = w.gate(&["merge", "7"]);
    assert_eq!(run.code, 1);
    assert_eq!(w.merges(), 0);
}

#[test]
fn the_gate_binary_is_staged_again_before_the_second_container_run_after_the_first_changed_it() {
    let w = World::new("restage", PINNED);
    std::fs::write(&w.tamper, "x").unwrap();
    let run = w.verify("guard", BITES);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let want = w.binary_hash();
    assert_eq!(
        w.containers(),
        [format!("call 1 staged={want}"), format!("call 2 staged={want}")],
        "each container found the gate binary, the second after the first overwrote it"
    );
}

#[test]
fn a_machine_with_only_docker_is_refused_by_name_and_nothing_is_extracted() {
    let mut w = World::new("docker", PINNED);
    w.only_docker();
    let run = w.verify("guard", BITES);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "maintainer-merge: docker run has no --timeout, so the verify container cannot be bounded under docker; use podman\n"
    );
    assert!(w.containers().is_empty());
    assert!(
        !w.state.join("verify-work").exists()
            || std::fs::read_dir(w.state.join("verify-work")).unwrap().count() == 0
    );
}

#[test]
fn the_suite_sees_the_tree_the_verify_extracted_as_work_and_the_variables_it_reads() {
    let mut w = World::new("work", PINNED);
    // The suite's command refuses (exit 7 or 8) unless `work` is the mounted tree and the
    // characterization directory reached it: the clean run then fails and no receipt is written.
    std::fs::write(
        w.profile.join("verify.d/shell.sh"),
        SUITE.replace(
            "printf 'sh test.sh %s\\n' \"$1\"",
            "printf 'test -d %s/.container-target || exit 7; test %s = /cases || exit 8; sh test.sh %s\\n' \"${work:-unset}\" \"${CHARACTERIZATION_DIR:-none}\" \"$1\"",
        ),
    )
    .unwrap();
    w.extra_env.push(("CHARACTERIZATION_DIR".to_string(), "/cases".to_string()));
    let run = w.verify("guard", BITES);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(w.receipt().is_some());
}

#[cfg(feature = "podman-e2e")]
#[test]
fn with_podman_a_pinned_mutation_records_a_receipt_and_an_unpinned_one_is_refused() {
    let mut w = World::new("podman", PINNED);
    // The real runtime needs its own storage and socket variables, which the gate hands on
    // through `RUNTIME_ENV_NAMES`.
    let home = std::env::var("HOME").expect("HOME");
    w.extra_env.push(("HOME".to_string(), home));
    for name in ["XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "DBUS_SESSION_BUS_ADDRESS"]
    {
        if let Ok(value) = std::env::var(name) {
            w.extra_env.push((name.to_string(), value));
        }
    }
    // No stub runtime: the gate finds the real podman on the system path, behind the `gh` stub.
    std::fs::remove_file(w.stubs.join("podman")).unwrap();
    let run = w.verify("guard", BITES);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    w.receipt().expect("a receipt").check(&key(), "7", true).unwrap();

    std::fs::write(w.repo.join("test.sh"), UNPINNED).unwrap();
    w.git(&["commit", "-q", "-am", "weaker test"]);
    w.publish();
    drop(std::fs::remove_file(w.receipt_path()));
    let run = w.verify("guard", BITES);
    assert_eq!(run.code, 1);
    assert!(run.stderr.contains("THE GUARD DOES NOT BITE"), "{}", run.stderr);
    assert!(w.receipt().is_none());
}

#[test]
fn suites_under_the_home_folder_are_found_when_the_profile_dir_names_none() {
    let mut w = World::new("homesuites", PINNED);
    w.suites_at("home/.local/share/maintainer/profiles/t/verify.d");
    let run = w.verify("guard", BITES);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(w.receipt().is_some());
}

#[test]
fn suites_in_the_profiles_folder_beside_the_gate_are_found_last() {
    let mut w = World::new("treesuites", PINNED);
    w.suites_at("tree/profiles/t/verify.d");
    let run = w.verify("guard", BITES);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(w.receipt().is_some());
}

#[test]
fn a_profile_with_no_suites_folder_anywhere_refuses_by_name() {
    let mut w = World::new("nosuitedir", PINNED);
    w.suites_at("elsewhere/verify.d");
    let run = w.verify("guard", BITES);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "maintainer-merge: this profile declares no verify.d/ suites; see docs/verifying.md\n"
    );
    assert!(w.containers().is_empty());
}

#[test]
fn the_container_runtime_gets_its_own_variables_and_no_credential() {
    let mut w = World::new("runtimeenv", PINNED);
    for (name, value) in [
        ("XDG_RUNTIME_DIR", "/run/user/x"),
        ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/dbus-x"),
        ("CONTAINERS_CONF", "/c.conf"),
        ("GH_TOKEN", "not-a-real-token"),
        ("LEAKED", "1"),
    ] {
        w.extra_env.push((name.to_string(), value.to_string()));
    }
    assert_eq!(w.verify("guard", BITES).code, 0);
    let seen = std::fs::read_to_string(&w.env_log).unwrap();
    let line = "xdg=/run/user/x dbus=unix:path=/dbus-x conf=/c.conf token=unset other=unset";
    assert_eq!(seen, format!("{line}\n{line}\n"), "both container runs");
}

#[test]
fn the_git_variables_of_the_caller_reach_the_sandbox_git_that_extracts_the_tree() {
    let mut w = World::new("gitenv", PINNED);
    let trace = w.root.join("git-trace.log");
    w.extra_env.push(("GIT_TRACE".to_string(), trace.display().to_string()));
    assert_eq!(w.verify("guard", BITES).code, 0);
    let seen = std::fs::read_to_string(&trace).unwrap_or_default();
    assert!(
        seen.contains("read-tree") && seen.contains("checkout-index"),
        "the extraction's own git calls are in the trace the caller asked for: {seen}"
    );
}
