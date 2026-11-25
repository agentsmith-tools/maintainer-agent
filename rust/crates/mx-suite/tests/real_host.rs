//! The real suite host against real `bash` and real suite files.
//!
//! Every test writes its suites into a directory of its own under the target directory and
//! asks them through `RealProc`, so what is proved is what the gate does: one bash process per
//! question, started with an environment the caller chose.

use mx_ports::RealProc;
use mx_suite::{
    FakeSuiteHost, ImageRef, LimitKey, Prefetch, RealSuiteHost, Runtime, SUITE_ENV_NAMES,
    SuiteDirs, SuiteError, SuiteHost, SuiteName, TestFilter,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let base =
            std::env::var_os("CARGO_TARGET_TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);
        let dir = base.join(format!("real-host-{name}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(dir.join("verify.d")).unwrap();
        Self(dir)
    }

    fn suites(&self) -> PathBuf {
        self.0.join("verify.d")
    }

    fn suite(&self, name: &str, body: &str) {
        std::fs::write(self.suites().join(format!("{name}.sh")), body).unwrap();
    }

    fn host(&self) -> RealSuiteHost {
        host_over(self.0.to_str().unwrap(), Vec::new())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.0));
    }
}

fn host_over(profile_dir: &str, env: Vec<(String, String)>) -> RealSuiteHost {
    let proc = RealProc::new("/usr/bin:/bin", env);
    RealSuiteHost::new(
        Arc::new(proc),
        SuiteDirs {
            profile_dir: profile_dir.to_string(),
            home: "/nonexistent-home".into(),
            profile: "p".into(),
            here: PathBuf::from("/nonexistent-here"),
        },
    )
}

fn name(text: &str) -> SuiteName {
    SuiteName::parse(text).unwrap()
}

const RUST_LIKE: &str = r#"
suite_covers() { case "$1" in *.rs|Cargo.toml) return 0 ;; esac; return 1; }
suite_image() { printf 'docker.io/library/rust:1-slim'; }
suite_needs() { printf 'bash cargo rustc'; }
suite_mutate_glob() { printf '%s\n' '*.rs' '' 'Makefile'; }
suite_prefetch_command() { printf 'cargo fetch --locked\n'; }
suite_prefetch_guard() { [ -f "$1/Cargo.lock" ]; }
"#;

#[test]
fn the_first_candidate_that_is_a_directory_is_the_suite_dir() {
    let scratch = Scratch::new("dirs");
    let home = scratch.0.join("home");
    let second = home.join(".local/share/maintainer/profiles/p/verify.d");
    std::fs::create_dir_all(&second).unwrap();
    let dirs = |profile_dir: &str| SuiteDirs {
        profile_dir: profile_dir.into(),
        home: home.to_str().unwrap().into(),
        profile: "p".into(),
        here: scratch.0.join("bin"),
    };
    assert_eq!(dirs("").find().unwrap(), second, "an empty PROFILE_DIR skips the first candidate");
    assert_eq!(
        dirs(scratch.0.to_str().unwrap()).find().unwrap(),
        scratch.suites(),
        "PROFILE_DIR wins when its verify.d exists"
    );
    assert_eq!(dirs("/nonexistent").find().unwrap(), second);
    std::fs::remove_dir_all(&second).unwrap();
    assert_eq!(dirs("").find(), Err(SuiteError::NoSuiteDir));
    let tool = scratch.0.join("profiles/p/verify.d");
    std::fs::create_dir_all(&tool).unwrap();
    std::fs::create_dir_all(scratch.0.join("bin")).unwrap();
    assert_eq!(dirs("").find().unwrap(), scratch.0.join("bin/../profiles/p/verify.d"));
}

#[test]
fn names_are_in_byte_order_and_leave_out_what_is_not_a_suite() {
    let scratch = Scratch::new("names");
    let mut expected = vec!["Zed".to_string(), "docs".into(), "rust".into()];
    for i in 0..12 {
        expected.push(format!("s{i:02}"));
    }
    expected.sort();
    for stem in &expected {
        scratch.suite(stem, "");
    }
    std::fs::write(scratch.suites().join("notes.txt"), "").unwrap();
    std::fs::write(scratch.suites().join(".hidden.sh"), "").unwrap();
    std::fs::write(scratch.suites().join("bad name.sh"), "").unwrap();
    std::fs::write(scratch.suites().join("a.b.sh"), "").unwrap();
    std::fs::create_dir(scratch.suites().join("dir.sh")).unwrap();
    let names: Vec<String> =
        scratch.host().names().unwrap().iter().map(ToString::to_string).collect();
    assert_eq!(names, expected);
    assert_eq!(names.first().map(String::as_str), Some("Zed"), "uppercase sorts before lowercase");
}

#[test]
fn a_loaded_suite_hands_back_its_declarations() {
    let scratch = Scratch::new("spec");
    scratch.suite("rust", RUST_LIKE);
    let host = scratch.host();
    let suite = host.load(&name("rust")).unwrap();
    let spec = suite.spec();
    assert_eq!(spec.name, name("rust"));
    assert_eq!(spec.image, ImageRef::parse("docker.io/library/rust:1-slim").unwrap());
    assert_eq!(spec.needs, ["bash", "cargo", "rustc"]);
    let globs: Vec<&str> = spec.mutate_globs.iter().map(mx_suite::MutateGlob::as_str).collect();
    assert_eq!(globs, ["*.rs", "Makefile"], "an empty line is dropped, as apply_mutation drops it");
    assert!(!spec.named_only);
    assert_eq!(
        spec.prefetch,
        Some(Prefetch { command: "cargo fetch --locked".into(), guarded: true }),
        "the prefetch command loses its trailing newline, as $(...) strips it"
    );
}

#[test]
fn a_suite_with_no_prefetch_and_no_mutate_glob_declares_neither() {
    let scratch = Scratch::new("bare");
    scratch.suite(
        "docs",
        "suite_covers() { return 0; }\nsuite_image() { printf 'img:1'; }\nsuite_named_only() { return 0; }\n",
    );
    let host = scratch.host();
    let suite = host.load(&name("docs")).unwrap();
    assert_eq!(suite.spec().prefetch, None);
    assert!(suite.spec().mutate_globs.is_empty());
    assert!(suite.spec().mutate_globs().is_err(), "a suite with no glob cannot be mutated");
    assert!(suite.spec().named_only);
    assert!(suite.spec().needs.is_empty());
}

#[test]
fn a_prefetch_command_without_a_guard_is_not_guarded() {
    let scratch = Scratch::new("unguarded");
    scratch.suite(
        "x",
        "suite_image() { printf 'img:1'; }\nsuite_prefetch_command() { printf 'fetch'; }\n",
    );
    let host = scratch.host();
    let loaded = host.load(&name("x")).unwrap();
    assert_eq!(loaded.spec().prefetch, Some(Prefetch { command: "fetch".into(), guarded: false }));
    assert_eq!(loaded.prefetch_guard(Path::new("/w")), Ok(()), "no guard, nothing refuses");
}

#[test]
fn every_suite_this_repository_ships_loads_with_its_declarations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut loaded = 0;
    for profile in ["magent", "sysknife", "_template"] {
        let dir = root.join("profiles").join(profile);
        let host = host_over(dir.to_str().unwrap(), Vec::new());
        for suite in host.names().unwrap() {
            let suite = host.load(&suite).unwrap_or_else(|e| panic!("{profile}/{suite}: {e}"));
            assert!(!suite.spec().image.as_str().is_empty());
            loaded += 1;
        }
    }
    assert!(loaded >= 7, "loaded {loaded} suites");
    let sysknife = host_over(root.join("profiles/sysknife").to_str().unwrap(), Vec::new());
    let rust = sysknife.load(&name("rust")).unwrap();
    assert_eq!(rust.spec().image.as_str(), "docker.io/library/rust:1-slim");
    assert_eq!(rust.spec().needs, ["bash", "cargo", "rustc"]);
    assert_eq!(rust.spec().mutate_globs().unwrap().first().as_str(), "*.rs");
    assert_eq!(
        rust.spec().prefetch,
        Some(Prefetch {
            command: "CARGO_NET_OFFLINE=false cargo fetch --locked".into(),
            guarded: true
        })
    );
    assert_eq!(rust.limit(LimitKey::Memory), Ok(Some("6g".into())));
    assert_eq!(rust.limit(LimitKey::Cpus), Ok(None));
    assert_eq!(
        rust.requires(Runtime::Docker).unwrap_err().to_string(),
        "the 'rust' suite needs podman (for the :O overlay mount on ~/.cargo), and this machine has docker"
    );
    assert_eq!(rust.requires(Runtime::Podman), Ok(()));
    let magent = host_over(root.join("profiles/magent").to_str().unwrap(), Vec::new());
    assert!(magent.load(&name("characterization")).unwrap().spec().named_only);
    assert_eq!(magent.inferable(&name("characterization")), Ok(false));
    assert_eq!(magent.inferable(&name("shell")), Ok(true));
}

#[test]
fn covers_is_the_status_of_suite_covers_and_anything_else_is_no() {
    let scratch = Scratch::new("covers");
    scratch.suite("rust", RUST_LIKE);
    scratch.suite("nofn", "suite_image() { printf 'i'; }\n");
    scratch.suite("exits", "exit 0\n");
    scratch.suite("syntax", "suite_covers() { return 0; }\nif then fi (\n");
    let host = scratch.host();
    assert_eq!(host.covers(&name("rust"), "src/a.rs"), Ok(true));
    assert_eq!(host.covers(&name("rust"), "README.md"), Ok(false));
    assert_eq!(host.covers(&name("nofn"), "src/a.rs"), Ok(false), "no function: 127 is no");
    assert_eq!(
        host.covers(&name("exits"), "a"),
        Ok(true),
        "a file that exits 0 while sourced answers for the whole subshell, as in the bash"
    );
    assert_eq!(
        host.covers(&name("syntax"), "a"),
        Ok(true),
        "the function defined before the error is used"
    );
    let loaded = host.load(&name("rust")).unwrap();
    assert_eq!(loaded.covers("Cargo.toml"), Ok(true));
}

#[test]
fn a_path_with_shell_syntax_reaches_the_suite_as_one_word() {
    let scratch = Scratch::new("quote");
    scratch.suite(
        "q",
        "suite_covers() { [ \"$1\" = 'a b; $(touch HIT) `x` *' ]; }\nsuite_image() { printf i; }\n",
    );
    let host = scratch.host();
    assert_eq!(host.covers(&name("q"), "a b; $(touch HIT) `x` *"), Ok(true));
    assert_eq!(host.covers(&name("q"), "a"), Ok(false));
}

#[test]
fn a_suite_runs_under_nounset_as_the_gate_does() {
    // bin/maintainer-merge starts with `set -uo pipefail`, so a suite that reads an unset
    // variable fails there, and has to fail here.
    let scratch = Scratch::new("nounset");
    scratch.suite(
        "u",
        "suite_image() { printf i; }\nsuite_covers() { [ -z \"$nothing_set\" ]; return 0; }\n",
    );
    let host = scratch.host();
    assert_eq!(host.covers(&name("u"), "a"), Ok(false));
}

#[test]
fn a_suite_that_kills_its_shell_cannot_stop_the_host() {
    let scratch = Scratch::new("kill");
    scratch.suite("k", "suite_image() { printf i; }\nsuite_covers() { kill -9 $$; }\n");
    let host = scratch.host();
    assert_eq!(host.covers(&name("k"), "a"), Ok(false));
    assert_eq!(host.covers(&name("k"), "b"), Ok(false), "and the host answers again");
}

#[test]
fn the_command_loses_every_trailing_newline_and_takes_the_filter_as_one_word() {
    let scratch = Scratch::new("command");
    scratch.suite(
        "c",
        "suite_image() { printf i; }\nsuite_command() { printf 'run %s \\n\\n\\n' \"$1\"; }\n",
    );
    let host = scratch.host();
    let suite = host.load(&name("c")).unwrap();
    assert_eq!(suite.command(&TestFilter::parse("a b").unwrap()).unwrap(), "run a b ");
    assert_eq!(suite.command(&TestFilter::parse("x_y.z").unwrap()).unwrap(), "run x_y.z ");
}

#[test]
fn a_suite_with_no_command_function_gives_an_empty_command() {
    let scratch = Scratch::new("nocommand");
    scratch.suite("c", "suite_image() { printf i; }\n");
    let host = scratch.host();
    let suite = host.load(&name("c")).unwrap();
    assert_eq!(suite.command(&TestFilter::parse("f").unwrap()).unwrap(), "");
}

#[test]
fn ran_reads_a_whole_number_and_counts_anything_else_as_zero() {
    let scratch = Scratch::new("ran");
    scratch.suite("r", "suite_image() { printf i; }\nsuite_ran() { cat \"$1\"; }\n");
    let host = scratch.host();
    let suite = host.load(&name("r")).unwrap();
    let log = scratch.0.join("log");
    for (printed, want) in
        [("12\n", 12), ("0", 0), ("x", 0), ("", 0), ("3 4", 0), ("-2", 0), ("007", 7)]
    {
        std::fs::write(&log, printed).unwrap();
        assert_eq!(suite.ran(&log), Ok(want), "{printed:?}");
    }
}

#[test]
fn extra_args_are_what_suite_podman_args_prints_and_none_when_it_is_not_defined() {
    let scratch = Scratch::new("extra");
    scratch.suite(
        "e",
        "suite_image() { printf i; }\nsuite_podman_args() { printf '%s\\n' -v \"$2/x:/y:O\" -e \"RT=$1\"; }\n",
    );
    scratch.suite("none", "suite_image() { printf i; }\n");
    let host = scratch.host();
    let args = host.load(&name("e")).unwrap().extra_args(Runtime::Podman, Path::new("/s")).unwrap();
    assert_eq!(args, "-v\n/s/x:/y:O\n-e\nRT=podman");
    assert_eq!(
        host.load(&name("none")).unwrap().extra_args(Runtime::Podman, Path::new("/s")),
        Ok(String::new())
    );
}

#[test]
fn requires_says_no_with_the_suites_own_words_and_yes_when_the_suite_is_silent() {
    let scratch = Scratch::new("requires");
    scratch.suite(
        "p",
        "suite_image() { printf i; }\nsuite_requires() { [ \"$1\" = podman ]; }\nsuite_requires_name() { printf 'podman (overlay)'; }\n",
    );
    scratch.suite("silent", "suite_image() { printf i; }\n");
    let host = scratch.host();
    let p = host.load(&name("p")).unwrap();
    assert_eq!(p.requires(Runtime::Podman), Ok(()));
    assert_eq!(
        p.requires(Runtime::Docker),
        Err(SuiteError::NeedsOtherRuntime {
            suite: name("p"),
            needs: "podman (overlay)".into(),
            runtime: Runtime::Docker
        })
    );
    assert_eq!(host.load(&name("silent")).unwrap().requires(Runtime::Docker), Ok(()));
}

#[test]
fn a_limit_is_what_the_function_prints_and_none_when_it_is_not_defined() {
    let scratch = Scratch::new("limit");
    scratch.suite(
        "l",
        "suite_image() { printf i; }\nsuite_memory() { printf '6g\\n'; }\nsuite_pids() { printf ''; }\n",
    );
    let host = scratch.host();
    let suite = host.load(&name("l")).unwrap();
    assert_eq!(suite.limit(LimitKey::Memory), Ok(Some("6g".into())));
    assert_eq!(
        suite.limit(LimitKey::Pids),
        Ok(Some(String::new())),
        "defined and empty is not undefined"
    );
    assert_eq!(suite.limit(LimitKey::Cpus), Ok(None));
    assert_eq!(suite.limit(LimitKey::Tmpfs), Ok(None));
}

#[test]
fn the_prefetch_guard_refuses_with_standard_output_and_error_merged() {
    let scratch = Scratch::new("guard");
    scratch.suite("rust", RUST_LIKE);
    scratch.suite(
        "loud",
        "suite_image() { printf i; }\nsuite_prefetch_command() { printf f; }\nsuite_prefetch_guard() { printf 'out line\\n'; printf 'err line\\n' >&2; return 1; }\n",
    );
    let host = scratch.host();
    let tree = scratch.0.join("tree");
    std::fs::create_dir_all(&tree).unwrap();
    let rust = host.load(&name("rust")).unwrap();
    assert!(matches!(rust.prefetch_guard(&tree), Err(SuiteError::PrefetchRefused { .. })));
    std::fs::write(tree.join("Cargo.lock"), "").unwrap();
    assert_eq!(rust.prefetch_guard(&tree), Ok(()));
    let loud = host.load(&name("loud")).unwrap().prefetch_guard(&tree).unwrap_err();
    assert_eq!(loud, SuiteError::PrefetchRefused { output: "out line\nerr line".into() });
    assert_eq!(
        loud.to_string(),
        "the dependency fetch was refused before it got a network:\n\n    out line\n    err line"
    );
}

#[test]
fn a_suite_sees_the_allowed_variables_and_work_and_nothing_else() {
    let scratch = Scratch::new("env");
    scratch.suite(
        "env",
        "suite_image() { printf i; }\nsuite_command() { env | sed 's/=.*//' | grep -v -e '^_$' -e '^PWD$' -e '^SHLVL$' -e '^OLDPWD$' | sort | paste -sd' '; }\n",
    );
    let env: Vec<(String, String)> =
        SUITE_ENV_NAMES.iter().map(|n| ((*n).to_string(), format!("v-{n}"))).collect();
    let host = host_over(scratch.0.to_str().unwrap(), env);
    host.set_work(Path::new("/the/tree"));
    let seen = host.load(&name("env")).unwrap().command(&TestFilter::parse("f").unwrap()).unwrap();
    assert_eq!(
        seen,
        "CHARACTERIZATION_DIR HOME LC_ALL MAINTAINER_CARGO_JOBS MAINTAINER_SHELL_IMAGE PATH work"
    );
}

#[test]
fn work_is_visible_to_the_suite_after_set_work_and_not_before() {
    let scratch = Scratch::new("work");
    scratch.suite(
        "w",
        "suite_image() { printf i; }\nsuite_command() { printf '%s' \"${work:-unset}\"; }\n",
    );
    let host = scratch.host();
    let suite = host.load(&name("w")).unwrap();
    let f = TestFilter::parse("f").unwrap();
    assert_eq!(suite.command(&f).unwrap(), "unset");
    host.set_work(Path::new("/w/tree"));
    assert_eq!(suite.command(&f).unwrap(), "/w/tree", "a suite loaded earlier sees it too");
}

#[test]
fn a_missing_suite_file_is_not_on_disk_before_bash_is_asked() {
    let scratch = Scratch::new("missing");
    let host = scratch.host();
    let refused = host.load(&name("nope")).err().unwrap();
    assert_eq!(
        refused,
        SuiteError::NotOnDisk { name: name("nope"), dir: scratch.0.join("verify.d") }
    );
    assert!(matches!(host.covers(&name("nope"), "a"), Err(SuiteError::NotOnDisk { .. })));
    assert!(matches!(host.inferable(&name("nope")), Err(SuiteError::NotOnDisk { .. })));
}

#[test]
fn a_suite_with_no_image_does_not_load_but_still_answers_covers() {
    let scratch = Scratch::new("noimage");
    scratch.suite("bare", "suite_covers() { return 0; }\n");
    let host = scratch.host();
    let refused = host.load(&name("bare")).err().unwrap().to_string();
    assert!(refused.contains("defines no suite_image"), "{refused}");
    assert_eq!(host.covers(&name("bare"), "x"), Ok(true));
}

#[test]
fn an_image_the_runtime_would_read_as_an_option_is_refused_at_load() {
    let scratch = Scratch::new("badimage");
    scratch.suite("a", "suite_image() { printf -- '--privileged'; }\n");
    let refused = scratch.host().load(&name("a")).err().unwrap().to_string();
    assert!(refused.contains("cannot start with '-'"), "{refused}");
}

#[test]
fn a_directory_without_a_verify_d_answers_no_suite_dir() {
    let host = host_over("/nonexistent-profile-dir", Vec::new());
    assert_eq!(host.dir(), Err(SuiteError::NoSuiteDir));
    assert_eq!(host.names(), Err(SuiteError::NoSuiteDir));
    assert!(matches!(host.load(&name("rust")), Err(SuiteError::NoSuiteDir)));
}

#[test]
fn the_fake_host_and_the_real_host_answer_the_trait_defaults_alike() {
    // covers and inferable have defaults for hosts that load from memory.
    let fake = FakeSuiteHost::new(
        "/d",
        vec![
            mx_suite::FakeSuite::new("a", "i").covering(&["x"]),
            mx_suite::FakeSuite::new("b", "i").named_only(),
        ],
    );
    assert_eq!(fake.covers(&name("a"), "x"), Ok(true));
    assert_eq!(fake.covers(&name("a"), "y"), Ok(false));
    assert_eq!(fake.inferable(&name("a")), Ok(true));
    assert_eq!(fake.inferable(&name("b")), Ok(false));
}
