//! Finding the tree, loading the profile and choosing the command: what happens
//! before `prune`, `release-check` or `fixup` starts. The profile loader runs a
//! real `bash` (the crate does not parse `profile.env`), so these tests route
//! `bash` to the real adapter and every other program to the scripted fake.

mod common;

use common::{Outcome, Rig, Scratch, args};
use mx_ports::{Proc, ProcError, ProcOutput, ProcSpec, RealProc};
use mx_repo::ctx::Ctx;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

struct Split<'a> {
    real: RealProc,
    fake: &'a mx_ports::FakeProc,
}

impl Proc for Split<'_> {
    fn run(&self, spec: &ProcSpec) -> Result<ProcOutput, ProcError> {
        if spec.argv.first() == "bash" { self.real.run(spec) } else { self.fake.run(spec) }
    }
}

/// A tree with bin/, lib/ (profile.sh and defaults.env copied from the repository) and a home directory.
struct Tree {
    scratch: Scratch,
}

impl Tree {
    fn new(tag: &str) -> Self {
        let scratch = Scratch::new(tag);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        scratch.dir("bin");
        scratch.dir("lib");
        scratch.dir("home");
        for name in ["profile.sh", "defaults.env"] {
            std::fs::copy(root.join("lib").join(name), scratch.path("lib").join(name))
                .expect("the repository has lib/");
        }
        Self { scratch }
    }

    fn program(&self) -> OsString {
        self.scratch.path("bin/maintainer-repo").into_os_string()
    }

    fn run(&self, rig: &Rig, vars: &[(&str, &str)], words: &[&str]) -> Outcome {
        let split = Split { real: RealProc::new("/usr/bin:/bin", Vec::new()), fake: &rig.proc };
        let mut out = Vec::new();
        let mut err = Vec::new();
        let home = self.scratch.path("home");
        let mut all: BTreeMap<String, String> =
            vars.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
        all.entry("HOME".to_string()).or_insert_with(|| home.to_string_lossy().into_owned());
        let code = {
            let mut ctx = Ctx::new(
                &split,
                self.program(),
                PathBuf::from("/nonexistent/exe"),
                all,
                &mut out,
                &mut err,
            );
            mx_repo::run(&mut ctx, &args(words))
        };
        Outcome {
            code,
            out: String::from_utf8_lossy(&out).into_owned(),
            err: String::from_utf8_lossy(&err).into_owned(),
        }
    }
}

#[test]
fn with_no_environment_and_no_deployed_profile_the_program_exits_64_before_it_reads_the_subcommand()
{
    let tree = Tree::new("startup-noprofile");
    let got = tree.run(&Rig::new(), &[], &["frobnicate"]);
    assert_eq!(got.code, 64);
    assert_eq!(
        got.err,
        "maintainer: no profile is deployed. Run ./install.sh, or set\n            MAINTAINER_PROFILE, MAINTAINER_SLUG and MAINTAINER_REPO.\n"
    );
    assert_eq!(got.out, "");
}

#[test]
fn an_unknown_subcommand_prints_the_usage_line_and_exits_64() {
    let tree = Tree::new("startup-usage");
    let vars = [
        ("MAINTAINER_SLUG", "o/r"),
        ("MAINTAINER_REPO", "/r"),
        ("MAINTAINER_ACCOUNT", "a"),
        ("MAINTAINER_STATE", "/s"),
    ];
    for word in ["frobnicate", "--help", "", "Prune", "prune "] {
        let got = tree.run(&Rig::new(), &vars, &[word]);
        assert_eq!(got.code, 64, "{word:?}");
        assert_eq!(
            got.err,
            "usage: maintainer-repo {prune [--dry-run]|release-check|fixup <pr> [--dry-run]}\n",
            "{word:?}"
        );
    }
    let none = tree.run(&Rig::new(), &vars, &[]);
    assert_eq!(none.code, 64);
}

#[test]
fn a_tree_with_no_profile_sh_beside_it_and_none_installed_is_refused_with_the_shell_s_two_lines() {
    let tree = Tree::new("startup-nolib");
    std::fs::remove_file(tree.scratch.path("lib/profile.sh")).unwrap();
    let got = tree.run(&Rig::new(), &[], &["prune"]);
    let program = tree.program().to_string_lossy().into_owned();
    let share = tree.scratch.path("home/.local/share/maintainer/profile.sh");
    assert_eq!(got.code, 64);
    assert_eq!(
        got.err,
        format!(
            "{program}: line 15: {}: No such file or directory\n{program}: line 16: maintainer_load_profile: command not found\n",
            share.display()
        )
    );
}

#[test]
fn with_no_home_and_no_profile_sh_beside_the_program_the_shell_s_unbound_variable_ends_it_with_1() {
    let tree = Tree::new("startup-nohome");
    std::fs::remove_file(tree.scratch.path("lib/profile.sh")).unwrap();
    let split = Split { real: RealProc::new("/usr/bin:/bin", Vec::new()), fake: &Rig::new().proc };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = {
        let mut ctx = Ctx::new(
            &split,
            tree.program(),
            PathBuf::from("/x"),
            BTreeMap::new(),
            &mut out,
            &mut err,
        );
        mx_repo::run(&mut ctx, &args(&["prune"]))
    };
    assert_eq!(code, 1);
    assert!(String::from_utf8_lossy(&err).ends_with("line 15: HOME: unbound variable\n"));
}

#[test]
fn a_deployed_profile_supplies_the_repository_and_exports_its_five_names_to_every_child() {
    // The bash function exports the five names, so a child (gh, git) sees them
    // even when the caller set none. The differential found this one.
    let tree = Tree::new("startup-deployed");
    let repo = tree.scratch.dir("repo");
    let share = tree.scratch.path("home/.local/share/maintainer/profiles/magent");
    std::fs::create_dir_all(&share).unwrap();
    std::fs::write(
        share.join("profile.env"),
        format!("REPO_SLUG=o/r\nREPO_PATH={}\nSTATE_DIR=/state\nGH_ACCOUNT=testuser\nPOST=on\nPROD_GLOBS='a/*'\n", repo.display()),
    )
    .unwrap();
    let rig = Rig::new();
    rig.ok(&["git", "fetch", "-q", "--prune", "origin"], "");
    rig.ok(
        &[
            "gh",
            "pr",
            "list",
            "--repo",
            "o/r",
            "--state",
            "open",
            "--json",
            "headRefName",
            "--jq",
            ".[].headRefName",
        ],
        "",
    );
    rig.ok(&["git", "for-each-ref", "--format=%(refname:strip=3)", "refs/remotes/origin"], "");
    rig.ok(&["git", "for-each-ref", "--format=%(refname:strip=2)", "refs/heads"], "");
    let got = tree.run(&rig, &[], &["prune", "--dry-run"]);
    assert_eq!(got.code, 1, "{}", got.err);
    assert_eq!(
        got.out_lines(),
        [
            "  remote branches merged into main:",
            "    none",
            "  local branches fully merged into main:",
            "    none"
        ]
    );
    for spec in rig.specs() {
        let value =
            |name: &str| spec.env.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        assert_eq!(value("MAINTAINER_SLUG").as_deref(), Some("o/r"), "{spec:?}");
        assert_eq!(value("MAINTAINER_ACCOUNT").as_deref(), Some("testuser"));
        assert_eq!(value("MAINTAINER_STATE").as_deref(), Some("/state"));
        assert_eq!(value("MAINTAINER_POST").as_deref(), Some("on"));
        assert_eq!(value("MAINTAINER_REPO"), Some(repo.to_string_lossy().into_owned()));
        assert_eq!(
            value("PROD_GLOBS"),
            None,
            "a tunable the profile sets is a shell variable, not an export"
        );
    }
}

#[test]
fn the_program_finds_its_tree_from_argv0_when_it_has_a_slash_and_from_the_executable_when_it_has_none()
 {
    let tree = Tree::new("startup-argv0");
    let vars = [
        ("MAINTAINER_SLUG", "o/r"),
        ("MAINTAINER_REPO", "/r"),
        ("MAINTAINER_ACCOUNT", "a"),
        ("MAINTAINER_STATE", "/s"),
    ];
    let rig = Rig::new();
    let split = Split { real: RealProc::new("/usr/bin:/bin", Vec::new()), fake: &rig.proc };
    let all: BTreeMap<String, String> =
        vars.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    // No slash in argv0: the tree is the executable's, here the scratch tree.
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = {
        let mut ctx = Ctx::new(
            &split,
            OsString::from("maintainer-repo"),
            tree.scratch.path("bin/maintainer-repo"),
            all.clone(),
            &mut out,
            &mut err,
        );
        let mut vars = all;
        vars.insert("HOME".into(), "/nonexistent-home".into());
        ctx.vars = vars;
        mx_repo::run(&mut ctx, &args(&["x"]))
    };
    assert_eq!(code, 64, "{}", String::from_utf8_lossy(&err));
    assert!(String::from_utf8_lossy(&err).starts_with("usage: maintainer-repo"));
}

#[test]
fn a_profile_that_leaves_a_source_variable_unset_ends_the_program_with_1_as_a_bash_script_file_does()
 {
    // bash 5.3.9 exits 1 on an unbound variable when the program is a script file
    // (127 only under `bash -c`), and the program is always run as a file.
    let tree = Tree::new("startup-unbound");
    let share = tree.scratch.path("home/.local/share/maintainer/profiles/magent");
    std::fs::create_dir_all(&share).unwrap();
    std::fs::write(
        share.join("profile.env"),
        "REPO_SLUG=o/r\nREPO_PATH=/r\nSTATE_DIR=/s\nPOST=on\n",
    )
    .unwrap();
    let got = tree.run(&Rig::new(), &[], &["prune"]);
    assert_eq!(got.code, 1);
    assert!(got.err.contains("GH_ACCOUNT: unbound variable"), "{}", got.err);
}
