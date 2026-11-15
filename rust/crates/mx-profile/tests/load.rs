//! Profile resolution against a real child bash and real files. Each case
//! builds a small tree (a lib directory with `defaults.env`, a home with
//! deployed profiles), loads, and checks the variables, the printed text and the
//! refusal by value. The cases mirror what `lib/profile.sh` does today.

mod common;

use common::{RecordingProc, Scratch, real_proc};
use mx_core::Refusal;
use mx_profile::load::{EXPORTED_FROM, Load, LoadEnv, LoadError, Profile, load_profile};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A lib directory and a home, built per case.
struct Tree {
    scratch: Scratch,
}

const DEFAULTS: &str =
    "# tunables\nLOCK_WAIT=3600\nRETENTION_DAYS=90\nGH_TRIES=3\nVERIFY_MEMORY=2g\n";

impl Tree {
    fn new(tag: &str) -> Self {
        let scratch = Scratch::new(tag);
        scratch.write("lib/defaults.env", DEFAULTS);
        std::fs::create_dir_all(scratch.path().join("home")).unwrap();
        Self { scratch }
    }

    fn lib(&self) -> PathBuf {
        self.scratch.path().join("lib")
    }

    fn home(&self) -> PathBuf {
        self.scratch.path().join("home")
    }

    fn share(&self) -> PathBuf {
        self.home().join(".local/share/maintainer/profiles")
    }

    fn profile(&self, name: &str, body: &str) -> PathBuf {
        self.scratch
            .write(&format!("home/.local/share/maintainer/profiles/{name}/profile.env"), body)
    }

    /// An environment with HOME set and these extra variables.
    fn env(&self, extra: &[(&str, &str)]) -> LoadEnv {
        let mut vars = vec![("HOME".to_string(), self.home().display().to_string())];
        vars.extend(extra.iter().map(|(n, v)| ((*n).to_string(), (*v).to_string())));
        LoadEnv::from_vars(self.lib(), vars)
    }
}

const PROFILE_FX: &str = "REPO_SLUG=\"o/r\"\nREPO_PATH=\"/work/r\"\nSTATE_DIR=\"/state\"\nGH_ACCOUNT=\"fxacct\"\nPOST=\"off\"\nLOCK_WAIT=10\n";

/// The profile of a load that must have worked. The check is an assertion, so a
/// load that fails reads as a failed assertion and names what it printed.
fn ok_of(loaded: Load) -> Profile {
    assert!(
        loaded.result.is_ok(),
        "the load failed with {:?}; stderr: {}",
        loaded.result.as_ref().err(),
        String::from_utf8_lossy(&loaded.stderr)
    );
    loaded.result.expect("checked by the assertion above")
}

/// The error of a load that must have failed.
fn err_of(loaded: Load) -> LoadError {
    assert!(loaded.result.is_err(), "the load was expected to fail, but it gave a profile");
    loaded.result.expect_err("checked by the assertion above")
}

fn load_ok(tree: &Tree, extra: &[(&str, &str)]) -> Profile {
    ok_of(load_profile(&real_proc(), &tree.env(extra)))
}

fn load_err(tree: &Tree, extra: &[(&str, &str)]) -> (LoadError, Vec<u8>, Vec<u8>) {
    let loaded = load_profile(&real_proc(), &tree.env(extra));
    let (stdout, stderr) = (loaded.stdout.clone(), loaded.stderr.clone());
    (err_of(loaded), stdout, stderr)
}

fn s(profile: &Profile, name: &str) -> String {
    profile.get_str(name).unwrap_or("<unset>").to_string()
}

// ----- resolution ----------------------------------------------------------------

#[test]
fn one_deployed_profile_needs_no_name_and_gives_defaults_profile_values_and_the_five_exports() {
    let tree = Tree::new("one");
    tree.profile("fx", PROFILE_FX);
    let profile = load_ok(&tree, &[]);
    assert_eq!(s(&profile, "RETENTION_DAYS"), "90", "a default the profile leaves alone");
    assert_eq!(s(&profile, "LOCK_WAIT"), "10", "the profile beats the default");
    assert_eq!(s(&profile, "MAINTAINER_SLUG"), "o/r");
    assert_eq!(s(&profile, "MAINTAINER_REPO"), "/work/r");
    assert_eq!(s(&profile, "MAINTAINER_STATE"), "/state");
    assert_eq!(s(&profile, "MAINTAINER_ACCOUNT"), "fxacct");
    assert_eq!(s(&profile, "MAINTAINER_POST"), "off");
    let mut exported: Vec<&str> = profile.exported().iter().map(String::as_str).collect();
    exported.sort_unstable();
    let mut want: Vec<&str> = EXPORTED_FROM.iter().map(|(name, _)| *name).collect();
    want.sort_unstable();
    assert_eq!(exported, want);
    assert_eq!(profile.deployed_name(), Some(b"fx".as_slice()));
}

#[test]
fn a_named_profile_is_used_when_several_are_deployed() {
    let tree = Tree::new("named");
    tree.profile(
        "alpha",
        "REPO_SLUG=\"a/a\"\nREPO_PATH=/a\nSTATE_DIR=/sa\nGH_ACCOUNT=aa\nPOST=on\n",
    );
    tree.profile(
        "beta",
        "REPO_SLUG=\"b/b\"\nREPO_PATH=/b\nSTATE_DIR=/sb\nGH_ACCOUNT=bb\nPOST=off\n",
    );
    let profile = load_ok(&tree, &[("MAINTAINER_PROFILE", "beta")]);
    assert_eq!(s(&profile, "MAINTAINER_SLUG"), "b/b");
    assert_eq!(profile.deployed_name(), Some(b"beta".as_slice()));
}

// Plan decision D16-class aside, MAINTAINER_<KEY> is the highest precedence for
// every key defaults.env declares. docs/configuration.md states the order.
#[test]
fn a_maintainer_override_beats_the_profile_and_the_default() {
    let tree = Tree::new("override");
    tree.profile("fx", PROFILE_FX);
    let profile =
        load_ok(&tree, &[("MAINTAINER_LOCK_WAIT", "7"), ("MAINTAINER_RETENTION_DAYS", "30")]);
    assert_eq!(s(&profile, "LOCK_WAIT"), "7", "over the profile's 10");
    assert_eq!(s(&profile, "RETENTION_DAYS"), "30", "over the default 90");
    assert_eq!(s(&profile, "GH_TRIES"), "3", "untouched");
}

#[test]
fn an_override_applies_only_to_a_key_defaults_env_declares_and_only_when_not_empty() {
    let tree = Tree::new("override-edge");
    tree.profile("fx", &format!("{PROFILE_FX}EXTRA=\"kept\"\n"));
    let profile = load_ok(
        &tree,
        &[
            ("MAINTAINER_EXTRA", "overridden"),
            ("MAINTAINER_GH_TRIES", ""),
            ("MAINTAINER_UNDECLARED", "x"),
        ],
    );
    assert_eq!(s(&profile, "EXTRA"), "kept", "EXTRA is not a key of defaults.env");
    assert_eq!(s(&profile, "GH_TRIES"), "3", "an empty override does not apply");
    assert_eq!(s(&profile, "MAINTAINER_EXTRA"), "overridden");
    assert_eq!(profile.get("UNDECLARED"), None);
}

// Only a key at the start of a line counts, as the sed expression has it.
#[test]
fn a_default_key_is_one_that_starts_a_line_with_capitals_digits_and_underscores() {
    let tree = Tree::new("keys");
    tree.scratch.write("lib/defaults.env", "FIRST=1\n  INDENTED=2\nlower=3\nUNDER_SCORE_9=5\nA=\n");
    tree.profile("fx", PROFILE_FX);
    let profile = load_ok(
        &tree,
        &[
            ("MAINTAINER_FIRST", "o1"),
            ("MAINTAINER_INDENTED", "o2"),
            ("MAINTAINER_lower", "o3"),
            ("MAINTAINER_UNDER_SCORE_9", "o5"),
            ("MAINTAINER_A", "o6"),
        ],
    );
    assert_eq!(s(&profile, "FIRST"), "o1");
    assert_eq!(s(&profile, "INDENTED"), "2", "an indented line is not a declared key");
    assert_eq!(s(&profile, "lower"), "3", "a lower-case name is not a declared key");
    assert_eq!(s(&profile, "UNDER_SCORE_9"), "o5");
    assert_eq!(s(&profile, "A"), "o6", "an empty default is still a declared key");
}

#[test]
fn the_five_names_take_a_value_already_set_over_their_profile_default() {
    let tree = Tree::new("five-set");
    tree.profile("fx", PROFILE_FX);
    // Three of the four early-return names are set, so this is the full branch.
    let profile = load_ok(
        &tree,
        &[
            ("MAINTAINER_SLUG", "kept/slug"),
            ("MAINTAINER_REPO", "/kept"),
            ("MAINTAINER_ACCOUNT", "keptacct"),
        ],
    );
    assert_eq!(s(&profile, "MAINTAINER_SLUG"), "kept/slug");
    assert_eq!(s(&profile, "MAINTAINER_REPO"), "/kept");
    assert_eq!(s(&profile, "MAINTAINER_ACCOUNT"), "keptacct");
    assert_eq!(s(&profile, "MAINTAINER_STATE"), "/state", "not set, so derived from STATE_DIR");
    assert_eq!(s(&profile, "MAINTAINER_POST"), "off");
}

#[test]
fn a_profile_that_declares_nothing_leaves_the_five_empty_and_still_exports_them() {
    let tree = Tree::new("five-empty");
    tree.profile("fx", "# nothing\n");
    let profile = load_ok(&tree, &[]);
    for (name, _) in EXPORTED_FROM {
        assert_eq!(profile.get(name), Some(b"".as_slice()), "{name}");
    }
    assert_eq!(profile.exported().len(), 5);
}

// ----- the early return ----------------------------------------------------------

fn run_env(profile_dir: &str) -> Vec<(&str, &str)> {
    vec![
        ("MAINTAINER_SLUG", "run/slug"),
        ("MAINTAINER_REPO", "/run/repo"),
        ("MAINTAINER_ACCOUNT", "runacct"),
        ("MAINTAINER_STATE", "/run/state"),
        ("PROFILE_DIR", profile_dir),
    ]
}

#[test]
fn when_a_run_exported_the_four_names_the_profile_in_profile_dir_is_read_and_nothing_is_re_derived()
{
    let tree = Tree::new("early");
    let dir = tree.scratch.path().join("elsewhere/fx");
    tree.scratch.write("elsewhere/fx/profile.env", "LOCK_WAIT=11\nREPO_SLUG=\"from/profile\"\n");
    let profile = load_ok(&tree, &run_env(dir.to_str().unwrap()));
    assert_eq!(s(&profile, "LOCK_WAIT"), "11");
    assert_eq!(s(&profile, "MAINTAINER_SLUG"), "run/slug", "REPO_SLUG does not replace it");
    assert_eq!(profile.exported().len(), 0, "this branch exports nothing itself");
    assert_eq!(profile.deployed_name(), None);
}

#[test]
fn the_early_return_applies_the_overrides_to_what_the_profile_set() {
    let tree = Tree::new("early-override");
    let dir = tree.scratch.path().join("p/fx");
    tree.scratch.write("p/fx/profile.env", "LOCK_WAIT=11\n");
    let mut extra = run_env(dir.to_str().unwrap());
    extra.push(("MAINTAINER_LOCK_WAIT", "99"));
    let profile = load_ok(&tree, &extra);
    assert_eq!(s(&profile, "LOCK_WAIT"), "99");
}

#[test]
fn the_early_return_without_a_profile_file_gives_the_defaults_only() {
    let tree = Tree::new("early-nofile");
    for dir in ["/nonexistent", ""] {
        let extra = if dir.is_empty() {
            let mut extra = run_env("");
            extra.retain(|(name, _)| *name != "PROFILE_DIR");
            extra
        } else {
            run_env(dir)
        };
        let profile = load_ok(&tree, &extra);
        assert_eq!(s(&profile, "LOCK_WAIT"), "3600");
        assert_eq!(s(&profile, "MAINTAINER_SLUG"), "run/slug");
    }
}

// Three of the four names are not enough: the function falls through to
// the deployed profiles, and with none deployed it refuses.
#[test]
fn the_early_return_needs_all_four_names() {
    for missing in ["MAINTAINER_SLUG", "MAINTAINER_REPO", "MAINTAINER_ACCOUNT", "MAINTAINER_STATE"]
    {
        let tree = Tree::new("early-three");
        let mut extra = run_env("/nonexistent");
        extra.retain(|(name, _)| *name != missing);
        let (error, _, _) = load_err(&tree, &extra);
        assert_eq!(error, LoadError::NoProfileDeployed, "without {missing}");
    }
    let tree = Tree::new("early-empty");
    let mut extra = run_env("/nonexistent");
    extra.retain(|(name, _)| *name != "MAINTAINER_STATE");
    extra.push(("MAINTAINER_STATE", ""));
    assert_eq!(
        load_err(&tree, &extra).0,
        LoadError::NoProfileDeployed,
        "an empty value counts as unset"
    );
}

// ----- the refusals ----------------------------------------------------------------

#[test]
fn no_deployed_profile_is_refused_with_the_two_line_message_and_return_code_1() {
    let tree = Tree::new("none");
    let (error, stdout, stderr) = load_err(&tree, &[]);
    assert_eq!(error, LoadError::NoProfileDeployed);
    assert_eq!(error.exit_code(), 1);
    assert_eq!(
        error.message(),
        b"maintainer: no profile is deployed. Run ./install.sh, or set\n            MAINTAINER_PROFILE, MAINTAINER_SLUG and MAINTAINER_REPO.\n"
    );
    assert!(stdout.is_empty() && stderr.is_empty());
}

#[test]
fn several_deployed_profiles_without_a_name_are_refused_naming_all_of_them() {
    let tree = Tree::new("many");
    tree.profile("beta", PROFILE_FX);
    tree.profile("alpha", PROFILE_FX);
    tree.profile("gamma", PROFILE_FX);
    let (error, _, _) = load_err(&tree, &[]);
    assert_eq!(error.exit_code(), 1);
    assert_eq!(
        String::from_utf8(error.message()).unwrap(),
        "maintainer: 3 profiles are deployed (alpha beta gamma).\n            Set MAINTAINER_PROFILE to say which one you mean.\n"
    );
}

#[test]
fn a_named_profile_that_is_not_deployed_is_refused_and_the_message_names_the_share() {
    let tree = Tree::new("missing");
    tree.profile("fx", PROFILE_FX);
    let (error, _, _) = load_err(&tree, &[("MAINTAINER_PROFILE", "nope")]);
    assert_eq!(error.exit_code(), 1);
    let want = format!(
        "maintainer: no deployed profile named nope (looked in {})\n",
        tree.share().display()
    );
    assert_eq!(String::from_utf8(error.message()).unwrap(), want);
}

// Decision P1F-8: the name is joined into the path as given, so a name with
// `..` leaves the share directory. The bash does the same.
#[test]
fn a_profile_name_with_dot_dot_is_joined_into_the_path_unchecked() {
    let tree = Tree::new("traversal");
    tree.scratch.write("elsewhere/profile.env", "REPO_SLUG=\"out/side\"\n");
    let share = tree.share();
    std::fs::create_dir_all(&share).unwrap();
    let up = "../../../../../elsewhere";
    assert!(share.join(up).join("profile.env").is_file(), "the traversal does not reach the file");
    let profile = load_ok(&tree, &[("MAINTAINER_PROFILE", up)]);
    assert_eq!(s(&profile, "MAINTAINER_SLUG"), "out/side");
}

// The glob `profiles/*/profile.env` skips dot directories, plain files and
// directories without a profile.env, and `-e` follows symlinks.
#[test]
fn only_directories_holding_a_profile_env_count_as_deployed_and_dot_names_are_skipped() {
    let tree = Tree::new("discover");
    tree.profile("real", PROFILE_FX);
    tree.profile(".hidden", PROFILE_FX);
    std::fs::create_dir_all(tree.share().join("empty-dir")).unwrap();
    tree.scratch.write("home/.local/share/maintainer/profiles/a-file", "x");
    let profile = load_ok(&tree, &[]);
    assert_eq!(profile.deployed_name(), Some(b"real".as_slice()));
}

#[test]
fn a_profile_env_that_is_a_directory_counts_as_deployed_and_then_fails_the_file_check() {
    let tree = Tree::new("envdir");
    std::fs::create_dir_all(tree.share().join("odd/profile.env")).unwrap();
    let (error, _, _) = load_err(&tree, &[]);
    assert_eq!(error, LoadError::ProfileMissing { name: b"odd".to_vec(), share: tree.share() });
}

#[test]
fn a_symlinked_profile_directory_counts_as_deployed_and_a_broken_one_does_not() {
    let tree = Tree::new("symlinks");
    tree.scratch.write("store/linked/profile.env", PROFILE_FX);
    std::fs::create_dir_all(tree.share()).unwrap();
    std::os::unix::fs::symlink(
        tree.scratch.path().join("store/linked"),
        tree.share().join("linked"),
    )
    .unwrap();
    std::os::unix::fs::symlink(tree.scratch.path().join("store/gone"), tree.share().join("broken"))
        .unwrap();
    let profile = load_ok(&tree, &[]);
    assert_eq!(profile.deployed_name(), Some(b"linked".as_slice()));
}

// The glob sorts the whole path, `name/profile.env`, so `a-b` sorts before `a`
// because `-` is less than `/`. Under a C locale; decision P1F-10 notes that a
// UTF-8 locale collates differently.
#[test]
fn deployed_names_are_listed_in_the_order_the_glob_sorts_whole_paths() {
    let tree = Tree::new("order");
    for name in ["a", "a-b", "B", "b", "a b"] {
        tree.profile(name, PROFILE_FX);
    }
    let (error, _, _) = load_err(&tree, &[]);
    let LoadError::ManyProfiles { names } = error else {
        panic!("expected the several-profiles refusal")
    };
    let names: Vec<String> = names.into_iter().map(|n| String::from_utf8(n).unwrap()).collect();
    assert_eq!(names, ["B", "a b", "a-b", "a", "b"]);
}

// ----- defaults.env ------------------------------------------------------------------

#[test]
fn a_missing_defaults_env_is_refused_with_its_message() {
    let tree = Tree::new("nodefaults");
    tree.profile("fx", PROFILE_FX);
    std::fs::remove_file(tree.lib().join("defaults.env")).unwrap();
    let (error, _, _) = load_err(&tree, &[]);
    assert_eq!(error, LoadError::DefaultsMissing);
    assert_eq!(
        error.message(),
        b"maintainer: lib/defaults.env not found, so no tunable has a value. Run ./install.sh.\n"
    );
    assert_eq!(error.exit_code(), 1);
}

#[test]
fn defaults_env_is_found_beside_the_library_first_then_in_the_installed_share() {
    let tree = Tree::new("defaults-search");
    tree.profile("fx", PROFILE_FX);
    tree.scratch
        .write("home/.local/share/maintainer/defaults.env", "LOCK_WAIT=111\nRETENTION_DAYS=222\n");
    // Beside the library wins.
    let profile = load_ok(&tree, &[]);
    assert_eq!(s(&profile, "RETENTION_DAYS"), "90");
    // Without it, the installed copy is used.
    std::fs::remove_file(tree.lib().join("defaults.env")).unwrap();
    let profile = load_ok(&tree, &[]);
    assert_eq!(s(&profile, "RETENTION_DAYS"), "222");
}

// `maintainer_load_defaults` ends with `. "$f"`, so its status is that of the
// last command in the file. A failing last command ends the load with return
// code 1 and no message.
#[test]
fn a_defaults_env_whose_last_command_fails_ends_the_load_silently_with_1() {
    let tree = Tree::new("defaults-fail");
    tree.profile("fx", PROFILE_FX);
    tree.scratch.write("lib/defaults.env", "LOCK_WAIT=1\nfalse\n");
    let error = err_of(load_profile(&real_proc(), &tree.env(&[])));
    assert_eq!(error, LoadError::DefaultsFailed { status: 1 });
    assert_eq!((error.exit_code(), error.message()), (1, Vec::new()));
}

// ----- what a profile can do ---------------------------------------------------------

#[test]
fn a_profile_that_prints_has_its_output_returned_and_the_load_goes_on() {
    let tree = Tree::new("prints");
    tree.profile("fx", &format!("echo to-stdout\necho to-stderr >&2\n{PROFILE_FX}"));
    let loaded = load_profile(&real_proc(), &tree.env(&[]));
    assert_eq!(loaded.stdout, b"to-stdout\n");
    assert_eq!(loaded.stderr, b"to-stderr\n");
    assert_eq!(s(&ok_of(loaded), "MAINTAINER_SLUG"), "o/r");
}

#[test]
fn a_profile_that_fails_its_last_command_still_loads() {
    let tree = Tree::new("last-fails");
    tree.profile("fx", &format!("{PROFILE_FX}false\n"));
    let profile = load_ok(&tree, &[]);
    assert_eq!(s(&profile, "MAINTAINER_SLUG"), "o/r");
}

#[test]
fn a_profile_that_calls_exit_ends_the_load_with_that_status_and_keeps_what_it_printed() {
    let tree = Tree::new("exits");
    tree.profile("fx", "echo before\nexit 3\nREPO_SLUG=never\n");
    let loaded = load_profile(&real_proc(), &tree.env(&[]));
    assert_eq!(loaded.stdout, b"before\n");
    let error = err_of(loaded);
    assert_eq!(error, LoadError::ShellExited { code: 3 });
    assert_eq!(error.exit_code(), 3);
}

#[test]
fn a_profile_that_exits_zero_still_ends_the_load_without_a_profile() {
    let tree = Tree::new("exits-zero");
    tree.profile("fx", "exit 0\n");
    let error = err_of(load_profile(&real_proc(), &tree.env(&[])));
    assert_eq!(error, LoadError::ShellExited { code: 0 });
}

#[test]
fn a_profile_that_is_killed_by_a_signal_reports_128_plus_the_signal() {
    let tree = Tree::new("killed");
    tree.profile("fx", "kill -9 $$\n");
    let error = err_of(load_profile(&real_proc(), &tree.env(&[])));
    assert_eq!(error, LoadError::ShellExited { code: 137 });
}

#[test]
fn a_profile_that_returns_stops_sourcing_there_and_the_load_goes_on() {
    let tree = Tree::new("returns");
    tree.profile("fx", "REPO_SLUG=\"before/return\"\nreturn 5\nREPO_PATH=\"/after\"\n");
    let profile = load_ok(&tree, &[]);
    assert_eq!(s(&profile, "MAINTAINER_SLUG"), "before/return");
    assert_eq!(profile.get("REPO_PATH"), None);
}

#[test]
fn a_profile_that_sets_an_unrelated_variable_leaves_it_in_the_result() {
    let tree = Tree::new("unrelated");
    tree.profile(
        "fx",
        &format!(
            "{PROFILE_FX}PROD_GLOBS=\"bin/* lib/*\"\nCLAIM_LABEL=\"\"\nTASKS=\"review issues\"\n"
        ),
    );
    let profile = load_ok(&tree, &[]);
    assert_eq!(s(&profile, "PROD_GLOBS"), "bin/* lib/*", "a glob is data, never expanded");
    assert_eq!(profile.get("CLAIM_LABEL"), Some(b"".as_slice()));
    assert_eq!(s(&profile, "TASKS"), "review issues");
}

#[test]
fn values_come_back_byte_exact_with_newlines_tabs_and_high_bytes() {
    let tree = Tree::new("bytes");
    tree.profile("fx", "V1=$'a\\nb\\tc'\nV2=$'\\xc3\\xa9\\xff'\nV3=''\nV4=\"  padded  \"\n");
    let profile = load_ok(&tree, &[]);
    assert_eq!(profile.get("V1"), Some(b"a\nb\tc".as_slice()));
    assert_eq!(profile.get("V2"), Some(b"\xc3\xa9\xff".as_slice()));
    assert_eq!(profile.get("V3"), Some(b"".as_slice()));
    assert_eq!(profile.get("V4"), Some(b"  padded  ".as_slice()));
}

#[test]
fn a_profile_sees_no_positional_parameters() {
    let tree = Tree::new("params");
    tree.profile("fx", "NARGS=$#\nFIRST=\"${1-unset}\"\n");
    let profile = load_ok(&tree, &[]);
    assert_eq!(s(&profile, "NARGS"), "0");
    assert_eq!(s(&profile, "FIRST"), "unset");
}

// ----- what the child can see ---------------------------------------------------------

// Decision P1F-7: HOME, PROFILE_DIR and MAINTAINER_* reach the child, and so
// does the adapter's PATH. Nothing else does, so `$USER` expands to nothing.
#[test]
fn the_profile_sees_home_the_maintainer_variables_and_the_adapters_path_and_nothing_else() {
    let tree = Tree::new("env");
    tree.profile(
        "fx",
        "SEEN_HOME=\"$HOME\"\nSEEN_PATH=\"$PATH\"\nSEEN_USER=\"${USER-unset}\"\nSEEN_TOKEN=\"${GH_TOKEN-unset}\"\nSEEN_M=\"$MAINTAINER_CUSTOM\"\nSEEN_LC=\"$LC_ALL\"\n",
    );
    let vars = vec![
        ("HOME".to_string(), tree.home().display().to_string()),
        ("USER".to_string(), "someone".to_string()),
        ("GH_TOKEN".to_string(), "ghp_must_not_reach_the_profile".to_string()),
        ("MAINTAINER_CUSTOM".to_string(), "custom".to_string()),
    ];
    let env = LoadEnv::from_vars(tree.lib(), vars);
    let profile = ok_of(load_profile(&real_proc(), &env));
    assert_eq!(s(&profile, "SEEN_HOME"), tree.home().display().to_string());
    assert_eq!(s(&profile, "SEEN_PATH"), "/usr/bin:/bin");
    assert_eq!(s(&profile, "SEEN_USER"), "unset");
    assert_eq!(s(&profile, "SEEN_TOKEN"), "unset");
    assert_eq!(s(&profile, "SEEN_M"), "custom");
    assert_eq!(s(&profile, "SEEN_LC"), "C");
    assert_eq!(profile.get("GH_TOKEN"), None);
    assert_eq!(profile.get("USER"), None);
}

// The proof that no value travels in argv: every MAINTAINER_* value and every
// path goes in the environment or as a file argument, and the call's argv holds
// only the shell script, the frame token and the files to source.
#[test]
fn no_maintainer_value_reaches_the_child_in_its_argument_vector() {
    let tree = Tree::new("argv");
    tree.profile("fx", PROFILE_FX);
    let recorder = RecordingProc::new(real_proc());
    let secret = "ghp_SECRETVALUE0123456789abcdef";
    let env = tree.env(&[("MAINTAINER_SOME_TOKEN", secret), ("MAINTAINER_NAME", "Vladimir Rott")]);
    let profile = ok_of(load_profile(&recorder, &env));
    assert_eq!(s(&profile, "MAINTAINER_SOME_TOKEN"), secret, "the value did reach the child");
    let calls = recorder.specs();
    assert!(!calls.is_empty());
    for spec in &calls {
        let argv = spec.argv.as_slice();
        assert_eq!(argv[0], "bash");
        for part in argv {
            assert!(!part.contains(secret), "the secret is in argv: {part}");
            assert!(!part.contains("Vladimir Rott"), "a value is in argv: {part}");
        }
        assert!(
            spec.env.iter().any(|(name, value)| name == "MAINTAINER_SOME_TOKEN" && value == secret),
            "the value was not passed through the environment"
        );
        let names: Vec<&str> = spec.env.iter().map(|(name, _)| name.as_str()).collect();
        for name in &names {
            assert!(
                *name == "HOME" || *name == "PROFILE_DIR" || name.starts_with("MAINTAINER_"),
                "{name} is outside the allowlist"
            );
        }
    }
    println!("{} child calls checked", calls.len());
}

#[test]
fn a_profile_that_never_finishes_is_killed_at_the_timeout() {
    let tree = Tree::new("hang");
    tree.profile("fx", "echo started\nsleep 30\n");
    let mut env = tree.env(&[]);
    env.eval_timeout = Duration::from_secs(1);
    let started = Instant::now();
    let loaded = load_profile(&real_proc(), &env);
    let elapsed = started.elapsed();
    assert_eq!(loaded.stdout, b"started\n", "what it printed before the kill is kept");
    assert_eq!(err_of(loaded), LoadError::EvalTimedOut);
    assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
}

// Decision P1F-12: after the profile, the bash override step runs `sed` and
// `printf`. A profile that sets PATH to nothing, or defines a function called
// printf, makes the bash skip the overrides without a word. The port applies
// them in Rust, so nothing a profile does to its shell can stop them.
#[test]
fn a_profile_that_empties_path_or_shadows_printf_does_not_stop_the_overrides() {
    let tree = Tree::new("shadow");
    tree.profile(
        "fx",
        &format!(
            "PATH=/nonexistent\nprintf() {{ echo wrong; }}\nsed() {{ echo wrong; }}\n{PROFILE_FX}"
        ),
    );
    let profile = load_ok(&tree, &[("MAINTAINER_LOCK_WAIT", "7")]);
    assert_eq!(s(&profile, "LOCK_WAIT"), "7");
    assert_eq!(s(&profile, "MAINTAINER_SLUG"), "o/r");
}

// ----- errors as refusals ------------------------------------------------------------

#[test]
fn every_load_error_maps_to_a_refusal_with_exit_code_64() {
    let errors = [
        LoadError::DefaultsMissing,
        LoadError::DefaultsFailed { status: 1 },
        LoadError::NoProfileDeployed,
        LoadError::ManyProfiles { names: vec![b"a".to_vec(), b"b".to_vec()] },
        LoadError::ProfileMissing { name: b"x".to_vec(), share: PathBuf::from("/s") },
        LoadError::ShellExited { code: 3 },
        LoadError::EvalTimedOut,
        LoadError::Bash("x".into()),
        LoadError::Malformed("x".into()),
    ];
    assert_eq!(errors.len(), 9);
    for error in errors {
        let shown = format!("{error:?}");
        assert_eq!(error.into_refusal().exit_code(), 64, "{shown}");
    }
    assert_eq!(
        LoadError::ProfileMissing { name: b"x".to_vec(), share: PathBuf::from("/s") }
            .into_refusal(),
        Refusal::ProfileNotFound { profile: "x".into() }
    );
}

#[test]
fn from_vars_keeps_home_profile_dir_and_maintainer_names_and_drops_the_rest() {
    let env = LoadEnv::from_vars(
        "/lib",
        [
            ("HOME", "/h"),
            ("PROFILE_DIR", "/p"),
            ("MAINTAINER_A", "1"),
            ("MAINTAINERX", "dropped"),
            ("PATH", "/dropped"),
            ("GH_TOKEN", "dropped"),
        ]
        .map(|(n, v)| (n.to_string(), v.to_string())),
    );
    assert_eq!(env.home.as_deref(), Some("/h"));
    assert_eq!(env.profile_dir.as_deref(), Some("/p"));
    assert_eq!(env.maintainer, vec![("MAINTAINER_A".to_string(), "1".to_string())]);
}
