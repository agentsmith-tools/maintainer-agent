//! The real sandbox against real `git`, `find` and `sed`, and a stub container runtime.
//!
//! Fixture repositories are made through `RealProc` as well, because only `mx-ports` may start
//! a process. Git runs with the user's and the system's configuration switched off, so a
//! developer's `.gitconfig` cannot change a result.

use mx_core::NonEmpty;
use mx_ports::{Proc, ProcError, ProcOutput, ProcSpec, RealPipe, RealProc};
use mx_sandbox::{
    ContainedTree, ContainerExit, ContainerInputs, ContainerSpec, ExtraArgs, Limit, Limits,
    RealSandbox, Sandbox, SandboxError, SedExpression, Timeout, container_runtime, pick_env,
};
use mx_suite::{ImageRef, LimitKey, MutateGlob, Runtime};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const PATHS: &str = "/usr/bin:/bin";

fn git_env() -> Vec<(String, String)> {
    [
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_AUTHOR_NAME", "t"),
        ("GIT_AUTHOR_EMAIL", "t@example.invalid"),
        ("GIT_COMMITTER_NAME", "t"),
        ("GIT_COMMITTER_EMAIL", "t@example.invalid"),
        ("HOME", "/nonexistent"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
    .collect()
}

fn real_proc() -> RealProc {
    RealProc::new(PATHS, git_env())
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let base =
            std::env::var_os("CARGO_TARGET_TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);
        let dir = base.join(format!("real-sandbox-{name}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        Self(std::fs::canonicalize(&dir).unwrap())
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.path(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.0));
    }
}

fn run(program: &str, args: &[&str], cwd: &Path) -> ProcOutput {
    let spec = ProcSpec::command(program, args.iter().copied(), Duration::from_mins(1)).cwd(cwd);
    let out = real_proc().run(&spec).unwrap();
    assert!(out.exit.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    out
}

fn git(cwd: &Path, args: &[&str]) -> String {
    String::from_utf8(run("git", args, cwd).stdout).unwrap()
}

/// A repository with one commit holding `files`, and the scratch that holds it.
fn repo(name: &str, files: &[(&str, &str)]) -> (Scratch, PathBuf) {
    let scratch = Scratch::new(name);
    let repo = scratch.path("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    for (rel, body) in files {
        let path = repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    git(&repo, &["add", "-A", "-f"]);
    git(&repo, &["commit", "-q", "-m", "c", "--allow-empty"]);
    (scratch, repo)
}

fn sandbox() -> RealSandbox {
    RealSandbox::new(
        Arc::new(real_proc()),
        Arc::new(RealPipe::new(PATHS, None)),
        Runtime::Podman,
        Vec::new(),
    )
    .unwrap()
}

fn extract(sandbox: &RealSandbox, scratch: &Scratch, repo: &Path) -> mx_sandbox::ExtractedTree {
    let work = sandbox.scratch_dir(&scratch.path("work-parent")).unwrap();
    sandbox.extract_tree(repo, "HEAD", &work).unwrap()
}

fn contained(tree: mx_sandbox::ExtractedTree) -> ContainedTree {
    ContainedTree::check(tree, Vec::new()).unwrap()
}

fn globs(list: &[&str]) -> NonEmpty<MutateGlob> {
    NonEmpty::from_vec(list.iter().map(|g| MutateGlob::parse(g).unwrap()).collect()).unwrap()
}

fn sed(text: &str) -> SedExpression {
    SedExpression::parse(text).unwrap()
}

// ---- extract_tree ---------------------------------------------------------------------

#[test]
fn a_tree_is_extracted_with_every_file_and_every_link_counted() {
    let (scratch, repo) = repo("extract", &[("a.rs", "x"), ("d/b.rs", "y"), ("d/e/c.sh", "z")]);
    symlink("a.rs", repo.join("l")).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "link"]);
    let sandbox = sandbox();
    let tree = extract(&sandbox, &scratch, &repo);
    assert_eq!(std::fs::read_to_string(tree.root().join("d/e/c.sh")).unwrap(), "z");
    assert!(std::fs::symlink_metadata(tree.root().join("l")).unwrap().file_type().is_symlink());
}

#[test]
fn a_submodule_is_not_counted_as_a_file() {
    let (scratch, repo) = repo("submodule", &[("a.rs", "x")]);
    // A gitlink: mode 160000, no files on disk.
    git(
        &repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            "160000,1234567890123456789012345678901234567890,vendor/dep",
        ],
    );
    git(&repo, &["commit", "-q", "-m", "gitlink"]);
    let sandbox = sandbox();
    let tree = extract(&sandbox, &scratch, &repo);
    assert!(tree.root().join("vendor/dep").is_dir());
}

#[test]
fn a_revision_that_lists_no_file_is_refused() {
    let (scratch, repo) = repo("nofiles", &[]);
    let sandbox = sandbox();
    let work = sandbox.scratch_dir(&scratch.path("wp")).unwrap();
    assert_eq!(
        sandbox.extract_tree(&repo, "HEAD", &work),
        Err(SandboxError::NoFiles { rev: "HEAD".into() })
    );
}

#[test]
fn an_unknown_revision_is_refused_as_read_tree_failing() {
    let (scratch, repo) = repo("unknown", &[("a", "x")]);
    let sandbox = sandbox();
    let work = sandbox.scratch_dir(&scratch.path("wp")).unwrap();
    let refused = sandbox.extract_tree(&repo, "v-99", &work).unwrap_err();
    assert_eq!(refused, SandboxError::ReadTree { rev: "v-99".into() });
    assert_eq!(refused.to_string(), "read-tree v-99 failed");
}

#[test]
fn a_revision_that_looks_like_a_git_option_is_refused_before_git_runs() {
    let (scratch, repo) = repo("dash", &[("a", "x")]);
    let sandbox = sandbox();
    let work = sandbox.scratch_dir(&scratch.path("wp")).unwrap();
    // `git read-tree --empty` succeeds, so only the guard stands between the name and git.
    assert_eq!(
        sandbox.extract_tree(&repo, "--empty", &work),
        Err(SandboxError::ReadTree { rev: "--empty".into() })
    );
}

/// A `Proc` that runs everything for real except the calls `skip` names, which succeed with
/// no output and do nothing.
struct Skipping {
    real: RealProc,
    skip: &'static str,
}

impl Proc for Skipping {
    fn run(&self, spec: &ProcSpec) -> Result<ProcOutput, ProcError> {
        if spec.argv.as_slice().iter().any(|a| a == self.skip) {
            return Ok(ProcOutput {
                exit: mx_ports::ProcExit::Code(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            });
        }
        self.real.run(spec)
    }
}

#[test]
fn a_checkout_that_wrote_nothing_is_refused_with_both_counts() {
    // bash extract_tree: the tar that created no file under a seccomp filter, with status
    // thrown away. The count of files on disk is what catches it.
    let (scratch, repo) = repo("partial", &[("a", "1"), ("b", "2"), ("c/d", "3")]);
    let sandbox = RealSandbox::new(
        Arc::new(Skipping { real: real_proc(), skip: "checkout-index" }),
        Arc::new(RealPipe::new(PATHS, None)),
        Runtime::Podman,
        Vec::new(),
    )
    .unwrap();
    let work = sandbox.scratch_dir(&scratch.path("wp")).unwrap();
    let refused = sandbox.extract_tree(&repo, "HEAD", &work).unwrap_err();
    assert_eq!(refused, SandboxError::PartialExtract { rev: "HEAD".into(), want: 3, got: 0 });
    assert_eq!(refused.to_string(), "extracted 0 of the 3 files in HEAD");
}

#[test]
fn a_checkout_that_cannot_write_is_refused_as_checkout_index_failing() {
    let (scratch, repo) = repo("unwritable", &[("a", "1")]);
    let sandbox = sandbox();
    let work = sandbox.scratch_dir(&scratch.path("wp")).unwrap();
    std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o500)).unwrap();
    let refused = sandbox.extract_tree(&repo, "HEAD", &work).unwrap_err();
    std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(refused, SandboxError::CheckoutIndex { rev: "HEAD".into(), dir: work.clone() });
    assert_eq!(
        refused.to_string(),
        format!("checkout-index of HEAD into {} failed", work.display())
    );
}

#[test]
fn extraction_runs_no_hook_of_the_repository() {
    let (scratch, repo) = repo("hooks", &[("a", "1")]);
    let hooks = repo.join(".git/hooks");
    std::fs::write(hooks.join("post-checkout"), "#!/bin/sh\ntouch ../HOOK-RAN\n").unwrap();
    std::fs::set_permissions(hooks.join("post-checkout"), std::fs::Permissions::from_mode(0o755))
        .unwrap();
    let sandbox = sandbox();
    extract(&sandbox, &scratch, &repo);
    assert!(!repo.join("HOOK-RAN").exists());
}

#[test]
fn the_index_of_the_extraction_is_private_and_the_repository_index_is_untouched() {
    let (scratch, repo) = repo("index", &[("a", "1")]);
    let before = std::fs::read(repo.join(".git/index")).unwrap();
    let sandbox = sandbox();
    extract(&sandbox, &scratch, &repo);
    assert_eq!(std::fs::read(repo.join(".git/index")).unwrap(), before);
}

// ---- tree_escapes ----------------------------------------------------------------------

fn tree_with_links(name: &str, links: &[(&str, &str)]) -> (Scratch, mx_sandbox::ExtractedTree) {
    let scratch = Scratch::new(name);
    let root = scratch.path("tree");
    std::fs::create_dir_all(root.join("d/e")).unwrap();
    std::fs::write(root.join("f"), "x").unwrap();
    for (link, target) in links {
        let path = root.join(link);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        symlink(target, path).unwrap();
    }
    let tree = mx_sandbox::ExtractedTree::counted(root, "r", 1, 1).unwrap();
    (scratch, tree)
}

fn escapes(links: &[(&str, &str)]) -> Vec<String> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (_scratch, tree) = tree_with_links(&format!("escapes{n}"), links);
    sandbox().tree_escapes(&tree).unwrap().iter().map(ToString::to_string).collect()
}

#[test]
fn links_that_stay_inside_the_tree_are_not_reported() {
    assert_eq!(
        escapes(&[
            ("a", "f"),
            ("d/up", "../f"),
            ("d/e/deep", "../../f"),
            ("dir", "d"),
            ("dangling", "no-such-file"),
            ("dangling2", "d/no/such/../thing"),
            ("loop-a", "loop-b"),
            ("loop-b", "loop-a"),
        ]),
        Vec::<String>::new()
    );
}

#[test]
fn a_link_that_resolves_outside_the_tree_is_reported_as_link_arrow_target() {
    assert_eq!(
        escapes(&[("harness.sh", "/etc/passwd"), ("d/x", "../../outside"), ("ok", "f")]),
        ["d/x -> ../../outside", "harness.sh -> /etc/passwd"]
    );
}

#[test]
fn a_link_chain_that_ends_outside_is_reported_at_its_first_link() {
    assert_eq!(
        escapes(&[("one", "two"), ("two", "three"), ("three", "/tmp")]),
        ["one -> two", "three -> /tmp", "two -> three"]
    );
}

#[test]
fn a_link_that_leaves_and_comes_back_is_inside() {
    let (scratch, tree) = tree_with_links("returns", &[]);
    let name = tree.root().file_name().unwrap().to_str().unwrap().to_string();
    symlink(format!("../{name}/f"), tree.root().join("back")).unwrap();
    assert!(sandbox().tree_escapes(&tree).unwrap().is_empty());
    drop(scratch);
}

#[test]
fn a_link_through_a_linked_directory_is_followed_by_name_not_by_a_descent() {
    // A link to a directory is checked but never walked into, like os.walk without
    // followlinks, so a link inside the target directory is reported once, at its own path.
    let result = escapes(&[("d/out", "/etc"), ("alias", "d")]);
    assert_eq!(result, ["d/out -> /etc"]);
}

#[test]
fn an_escape_whose_name_is_not_utf8_is_still_reported() {
    // The bash prints through python and, in a UTF-8 locale, dies on such a name with its
    // status thrown away, so the tree passes. This port decides on bytes.
    use std::os::unix::ffi::OsStrExt;
    let (_scratch, tree) = tree_with_links("bytes", &[]);
    let name = std::ffi::OsStr::from_bytes(b"bad\xff");
    symlink("/etc/passwd", tree.root().join(name)).unwrap();
    let found = sandbox().tree_escapes(&tree).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].target, "/etc/passwd");
    assert!(found[0].link.starts_with("bad"));
}

#[test]
fn an_unreadable_directory_is_an_error_and_not_a_tree_with_nothing_in_it() {
    let (_scratch, tree) = tree_with_links("unreadable", &[("d/e/x", "/etc/passwd")]);
    let locked = tree.root().join("d/e");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = sandbox().tree_escapes(&tree);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(result, Err(SandboxError::Run { .. })), "{result:?}");
}

#[test]
fn the_escapes_come_back_sorted_as_python_sorts_the_printed_strings() {
    let names = escapes(&[("b", "/x"), ("a b", "/x"), ("a", "/x"), ("a.b", "/x"), ("A", "/x")]);
    assert_eq!(names, ["A -> /x", "a -> /x", "a b -> /x", "a.b -> /x", "b -> /x"]);
}

// ---- apply_mutation --------------------------------------------------------------------

fn mutable(name: &str, files: &[(&str, &str)]) -> (Scratch, ContainedTree) {
    let scratch = Scratch::new(name);
    let root = scratch.path("tree");
    std::fs::create_dir_all(&root).unwrap();
    for (rel, body) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    let tree = contained(mx_sandbox::ExtractedTree::counted(root, "r", 1, 1).unwrap());
    (scratch, tree)
}

#[test]
fn the_mutation_changes_the_files_the_globs_name_and_lists_them_without_dot_slash() {
    let (_s, tree) = mutable(
        "mutate",
        &[
            ("src/a.rs", "if ok {}"),
            ("src/b.rs", "if ok {}"),
            ("README.md", "if ok"),
            ("Makefile", "if ok"),
        ],
    );
    let changed = sandbox()
        .apply_mutation(&tree, &globs(&["*.rs", "Makefile"]), &sed("s/if ok/if !ok/"))
        .unwrap();
    let mut paths = changed.paths().to_vec();
    paths.sort();
    assert_eq!(paths, ["Makefile", "src/a.rs", "src/b.rs"]);
    assert_eq!(std::fs::read_to_string(tree.root().join("src/a.rs")).unwrap(), "if !ok {}");
    assert_eq!(std::fs::read_to_string(tree.root().join("README.md")).unwrap(), "if ok");
}

#[test]
fn a_mutation_that_changes_nothing_is_the_bash_refusal_with_the_globs() {
    let (_s, tree) = mutable("nothing", &[("a.rs", "x")]);
    let refused =
        sandbox().apply_mutation(&tree, &globs(&["*.rs", "*.py"]), &sed("s/zzz/y/")).unwrap_err();
    assert_eq!(refused, SandboxError::NothingChanged { globs: vec!["*.rs".into(), "*.py".into()] });
}

#[test]
fn a_substitution_that_rewrites_a_line_to_itself_changed_nothing() {
    let (_s, tree) = mutable("same", &[("a.rs", "x")]);
    assert!(matches!(
        sandbox().apply_mutation(&tree, &globs(&["*.rs"]), &sed("s/x/x/")),
        Err(SandboxError::NothingChanged { .. })
    ));
}

#[test]
fn the_mutation_never_enters_the_git_directory_or_the_container_target() {
    let (_s, tree) = mutable(
        "skips",
        &[
            ("a.rs", "x"),
            (".git/hooks/h.rs", "x"),
            (".container-target/dep/z.rs", "x"),
            ("deep/.container-target/q.rs", "x"),
        ],
    );
    let changed = sandbox().apply_mutation(&tree, &globs(&["*.rs"]), &sed("s/x/y/")).unwrap();
    let mut paths = changed.paths().to_vec();
    paths.sort();
    assert_eq!(
        paths,
        ["a.rs", "deep/.container-target/q.rs"],
        "only the top-level ones are skipped"
    );
    assert_eq!(std::fs::read_to_string(tree.root().join(".git/hooks/h.rs")).unwrap(), "x");
    assert_eq!(
        std::fs::read_to_string(tree.root().join(".container-target/dep/z.rs")).unwrap(),
        "x"
    );
}

#[test]
fn a_symlink_is_never_rewritten_and_its_target_is_never_written_through() {
    let scratch = Scratch::new("link");
    let outside = scratch.path("outside.txt");
    std::fs::write(&outside, "secret x").unwrap();
    let (_s, tree) = mutable("link-tree", &[("a.rs", "x")]);
    symlink(&outside, tree.root().join("harness.rs")).unwrap();
    let changed = sandbox().apply_mutation(&tree, &globs(&["*.rs"]), &sed("s/x/y/")).unwrap();
    assert_eq!(changed.paths(), ["a.rs"]);
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret x");
    assert!(
        std::fs::symlink_metadata(tree.root().join("harness.rs")).unwrap().file_type().is_symlink()
    );
}

#[test]
fn the_sandboxed_sed_refuses_the_commands_that_run_or_write() {
    for hostile in ["s/x/id/e", "w /tmp/mx-sandbox-should-not-exist", "r /etc/passwd"] {
        let (_s, tree) = mutable("hostile", &[("a.rs", "x")]);
        let result = sandbox().apply_mutation(&tree, &globs(&["*.rs"]), &sed(hostile));
        assert_eq!(result, Err(SandboxError::MutationFailed), "{hostile}");
        assert_eq!(std::fs::read_to_string(tree.root().join("a.rs")).unwrap(), "x");
    }
    assert!(!Path::new("/tmp/mx-sandbox-should-not-exist").exists());
}

#[test]
fn a_file_name_with_spaces_is_reported_whole() {
    // The bash reads the changed names through awk's $2 and cuts at the first blank.
    let (_s, tree) = mutable("spaces", &[("my file.rs", "x"), ("my other.rs", "x")]);
    let changed = sandbox().apply_mutation(&tree, &globs(&["*.rs"]), &sed("s/x/y/")).unwrap();
    let mut paths = changed.paths().to_vec();
    paths.sort();
    assert_eq!(paths, ["my file.rs", "my other.rs"]);
}

#[test]
fn a_file_name_that_starts_with_a_dash_is_a_file_and_not_an_option() {
    let (_s, tree) = mutable("dashname", &[("-n.rs", "x"), ("--version.rs", "x")]);
    let changed = sandbox().apply_mutation(&tree, &globs(&["*.rs"]), &sed("s/x/y/")).unwrap();
    assert_eq!(changed.paths().len(), 2);
    assert_eq!(std::fs::read_to_string(tree.root().join("-n.rs")).unwrap(), "y");
}

#[test]
fn a_tree_of_many_long_names_is_mutated_in_several_sed_calls_and_all_of_them_change() {
    // More file names than one argument list can hold (about 2 MB), so a single sed call fails.
    let long = "d".repeat(60);
    let names: Vec<String> = (0..36_000).map(|i| format!("{long}/f{i:05}.rs")).collect();
    let files: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "x")).collect();
    let (_s, tree) = mutable("many", &files);
    let changed = sandbox().apply_mutation(&tree, &globs(&["*.rs"]), &sed("s/x/y/")).unwrap();
    assert_eq!(changed.paths().len(), 36_000);
    for name in names.iter().step_by(997) {
        assert_eq!(std::fs::read_to_string(tree.root().join(name)).unwrap(), "y");
    }
}

#[test]
fn the_line_addresses_of_the_expression_restart_in_every_file() {
    let (_s, tree) = mutable("lines", &[("a.rs", "one\ntwo\n"), ("b.rs", "one\ntwo\n")]);
    sandbox().apply_mutation(&tree, &globs(&["*.rs"]), &sed("1d")).unwrap();
    assert_eq!(std::fs::read_to_string(tree.root().join("a.rs")).unwrap(), "two\n");
    assert_eq!(std::fs::read_to_string(tree.root().join("b.rs")).unwrap(), "two\n");
}

// ---- scratch_dir -----------------------------------------------------------------------

#[test]
fn a_scratch_directory_is_private_made_under_the_parent_and_the_parent_is_locked_down() {
    let scratch = Scratch::new("scratch");
    let parent = scratch.path("verify-work");
    let sandbox = sandbox();
    let a = sandbox.scratch_dir(&parent).unwrap();
    let b = sandbox.scratch_dir(&parent).unwrap();
    assert_ne!(a, b);
    for dir in [&a, &b] {
        assert_eq!(dir.parent().unwrap(), parent);
        assert_eq!(dir.file_name().unwrap().len(), 8);
        assert_eq!(std::fs::metadata(dir).unwrap().permissions().mode() & 0o777, 0o700);
    }
    assert_eq!(std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777, 0o700);
}

#[test]
fn a_parent_that_exists_with_loose_permissions_is_locked_down_as_chmod_700_does() {
    let scratch = Scratch::new("loose");
    let parent = scratch.path("vw");
    std::fs::create_dir(&parent).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
    sandbox().scratch_dir(&parent).unwrap();
    assert_eq!(std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777, 0o700);
}

#[test]
fn a_parent_that_cannot_be_made_is_the_first_bash_sentence() {
    let scratch = Scratch::new("blocked");
    scratch.write("file", "in the way");
    let parent = scratch.path("file/verify-work");
    let refused = sandbox().scratch_dir(&parent).unwrap_err();
    assert_eq!(refused, SandboxError::ScratchParent { dir: parent.clone() });
    assert_eq!(refused.to_string(), format!("cannot create {}", parent.display()));
}

#[test]
fn every_scratch_directory_is_removed_when_the_sandbox_is_dropped() {
    let scratch = Scratch::new("drop");
    let parent = scratch.path("vw");
    let (a, b) = {
        let sandbox = sandbox();
        let a = sandbox.scratch_dir(&parent).unwrap();
        let b = sandbox.scratch_dir(&parent).unwrap();
        std::fs::write(a.join("inner"), "x").unwrap();
        (a, b)
    };
    assert!(!a.exists() && !b.exists());
    assert!(parent.exists(), "the parent is not ours to remove");
}

#[test]
fn scratch_directories_are_removed_when_the_run_ends_by_a_panic() {
    let scratch = Scratch::new("panic");
    let parent = scratch.path("vw");
    let made = std::sync::Mutex::new(PathBuf::new());
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sandbox = sandbox();
        *made.lock().unwrap() = sandbox.scratch_dir(&parent).unwrap();
        panic!("the gate died");
    }));
    assert!(caught.is_err());
    assert!(!made.lock().unwrap().exists());
}

#[test]
fn remove_scratch_can_be_called_before_the_sandbox_is_dropped_and_twice() {
    let scratch = Scratch::new("explicit");
    let sandbox = sandbox();
    let dir = sandbox.scratch_dir(&scratch.path("vw")).unwrap();
    sandbox.remove_scratch();
    sandbox.remove_scratch();
    assert!(!dir.exists());
}

// ---- seed_index ------------------------------------------------------------------------

#[test]
fn the_seeded_index_lists_exactly_what_was_extracted_even_under_a_gitignore() {
    let (scratch, repo) = repo(
        "seed",
        &[("a.rs", "x"), ("d/b.rs", "y"), (".gitignore", "ignored.txt\n"), ("ignored.txt", "z")],
    );
    let sandbox = sandbox();
    let tree = contained(extract(&sandbox, &scratch, &repo));
    sandbox.seed_index(&tree).unwrap();
    let listed = git(tree.root(), &["ls-files"]);
    let mut files: Vec<&str> = listed.lines().collect();
    files.sort_unstable();
    assert_eq!(files, [".gitignore", "a.rs", "d/b.rs", "ignored.txt"]);
    assert!(tree.root().join(".container-target").is_dir());
    assert!(!listed.contains(".container-target"));
}

#[test]
fn a_hook_or_template_of_the_host_does_not_reach_the_extracted_tree() {
    let (scratch, repo) = repo("seedhooks", &[("a", "1")]);
    let sandbox = sandbox();
    let tree = contained(extract(&sandbox, &scratch, &repo));
    sandbox.seed_index(&tree).unwrap();
    let hooks: Vec<_> = std::fs::read_dir(tree.root().join(".git/hooks"))
        .map(|d| d.map(|e| e.unwrap().file_name()).collect())
        .unwrap_or_default();
    assert!(hooks.is_empty(), "{hooks:?}: init.templateDir= leaves no sample hooks");
}

#[test]
fn a_tree_that_git_cannot_index_is_a_note_and_the_target_directory_is_still_made() {
    let scratch = Scratch::new("noindex");
    let root = scratch.path("tree");
    std::fs::create_dir_all(&root).unwrap();
    // A file where `.git` must go: `git init` fails, and the bash goes on.
    std::fs::write(root.join(".git"), "not a directory").unwrap();
    let tree = contained(mx_sandbox::ExtractedTree::counted(root.clone(), "r", 1, 1).unwrap());
    assert_eq!(sandbox().seed_index(&tree), Err(SandboxError::IndexNotSeeded));
    assert!(root.join(".container-target").is_dir());
}

// ---- run_container ---------------------------------------------------------------------

fn limits() -> Limits {
    Limits {
        tmpfs: Limit::parse(LimitKey::Tmpfs, "64m").unwrap(),
        pids: Limit::parse(LimitKey::Pids, "64").unwrap(),
        memory: Limit::parse(LimitKey::Memory, "1g").unwrap(),
        cpus: Limit::parse(LimitKey::Cpus, "1").unwrap(),
    }
}

fn spec(work: &Path, prefetch: bool) -> ContainerSpec {
    let inputs = ContainerInputs {
        runtime: Runtime::Podman,
        work: work.to_path_buf(),
        image: ImageRef::parse("img:1").unwrap(),
        command: "echo hi".into(),
        extra_args: ExtraArgs::split("-e\nA=1"),
        timeout: Timeout::from_secs(2).unwrap(),
        limits: limits(),
    };
    if prefetch {
        ContainerSpec::prefetch(inputs).unwrap()
    } else {
        ContainerSpec::verify(inputs).unwrap()
    }
}

/// A directory with a `podman` that logs its argv (NUL-separated) and its environment names,
/// prints a line, and exits with the status in `$STUB_EXIT`... which it takes from its own
/// first container argument instead, because a stage starts with an empty environment.
fn stub_runtime(scratch: &Scratch, body: &str) -> String {
    let bin = scratch.path("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("podman");
    std::fs::write(&script, format!("#!/bin/bash\n{body}\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    format!("{}:{PATHS}", bin.display())
}

/// `run_container`, retried while the kernel says `Text file busy`: another test thread wrote
/// its own stub script a moment ago, and a child forked meanwhile still holds the file open for
/// writing until it execs. The stub being executed is never the one being written.
fn run_container(
    sandbox: &RealSandbox,
    spec: &ContainerSpec,
    log: &Path,
) -> Result<ContainerExit, SandboxError> {
    for _ in 0..50 {
        match sandbox.run_container(spec, log) {
            Err(SandboxError::Run { reason }) if reason.contains("Text file busy") => {
                std::thread::sleep(Duration::from_millis(20));
            }
            other => return other,
        }
    }
    sandbox.run_container(spec, log)
}

fn sandbox_with(path: &str, env: Vec<(String, String)>, grace: Duration) -> RealSandbox {
    RealSandbox::new(
        Arc::new(real_proc()),
        Arc::new(RealPipe::new(path, None)),
        Runtime::Podman,
        env,
    )
    .unwrap()
    .with_runtime_grace(grace)
}

#[test]
fn the_runtime_gets_exactly_the_argument_vector_of_the_spec_and_its_output_lands_in_the_log() {
    let scratch = Scratch::new("run");
    let argv_log = scratch.path("argv");
    let path = stub_runtime(
        &scratch,
        &format!(
            "printf '%s\\0' \"$@\" > {}\necho out-line; echo err-line >&2; exit 3",
            argv_log.display()
        ),
    );
    let sandbox = sandbox_with(&path, Vec::new(), Duration::from_secs(30));
    let work = scratch.path("work");
    std::fs::create_dir_all(&work).unwrap();
    let spec = spec(&work, false);
    let log = work.join("clean.log");
    assert_eq!(run_container(&sandbox, &spec, &log), Ok(ContainerExit(3)));
    let seen: Vec<String> = std::fs::read_to_string(&argv_log)
        .unwrap()
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    assert_eq!(seen, spec.argv().into_vec()[1..], "everything after the program name");
    let written = std::fs::read_to_string(&log).unwrap();
    assert!(written.contains("out-line") && written.contains("err-line"), "{written}");
}

#[test]
fn the_verify_log_is_appended_to_and_the_prefetch_log_starts_over() {
    let scratch = Scratch::new("modes");
    let path = stub_runtime(&scratch, "echo ran");
    let sandbox = sandbox_with(&path, Vec::new(), Duration::from_secs(30));
    let work = scratch.path("work");
    std::fs::create_dir_all(&work).unwrap();
    let log = work.join("run.log");
    std::fs::write(&log, "older\n").unwrap();
    run_container(&sandbox, &spec(&work, false), &log).unwrap();
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "older\nran\n");
    run_container(&sandbox, &spec(&work, true), &log).unwrap();
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "ran\n");
}

#[test]
fn the_runtime_sees_only_the_environment_it_was_given() {
    let scratch = Scratch::new("env");
    let seen = scratch.path("env-names");
    let path = stub_runtime(&scratch, &format!("env | sed 's/=.*//' | sort > {}", seen.display()));
    let env = pick_env(&["HOME", "XDG_RUNTIME_DIR", "NOT_THERE"], |n| match n {
        "HOME" => Some("/h".into()),
        "XDG_RUNTIME_DIR" => Some("/run/user/1".into()),
        _ => None,
    });
    let sandbox = sandbox_with(&path, env, Duration::from_secs(30));
    let work = scratch.path("work");
    std::fs::create_dir_all(&work).unwrap();
    run_container(&sandbox, &spec(&work, false), &work.join("l")).unwrap();
    let names = std::fs::read_to_string(&seen).unwrap();
    let names: Vec<&str> =
        names.lines().filter(|n| !["_", "PWD", "SHLVL", "OLDPWD"].contains(n)).collect();
    assert_eq!(names, ["HOME", "LC_ALL", "PATH", "XDG_RUNTIME_DIR"]);
}

#[test]
fn a_runtime_that_outlives_its_deadline_is_killed_and_reads_as_a_timeout() {
    let scratch = Scratch::new("hang");
    let path = stub_runtime(&scratch, "sleep 30");
    let sandbox = sandbox_with(&path, Vec::new(), Duration::from_millis(100));
    let work = scratch.path("work");
    std::fs::create_dir_all(&work).unwrap();
    // --timeout 2 plus 100 ms of grace: the stub is killed at about 2.1 s.
    let started = std::time::Instant::now();
    assert_eq!(
        run_container(&sandbox, &spec(&work, false), &work.join("l")),
        Ok(ContainerExit(124))
    );
    assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
}

#[test]
fn the_client_may_outlive_the_container_timeout_by_the_grace() {
    let scratch = Scratch::new("grace");
    let path = stub_runtime(&scratch, "sleep 3; echo done");
    // --timeout 2 and a grace of 30 s: the stub's 3 s are inside the deadline.
    let sandbox = sandbox_with(&path, Vec::new(), Duration::from_secs(30));
    let work = scratch.path("work");
    std::fs::create_dir_all(&work).unwrap();
    assert_eq!(run_container(&sandbox, &spec(&work, false), &work.join("l")), Ok(ContainerExit(0)));
    assert_eq!(std::fs::read_to_string(work.join("l")).unwrap(), "done\n");
}

#[test]
fn a_runtime_that_is_not_installed_is_an_error_and_not_an_exit_code() {
    let scratch = Scratch::new("absent");
    let sandbox =
        sandbox_with(scratch.path("empty").to_str().unwrap(), Vec::new(), Duration::from_secs(1));
    let work = scratch.path("work");
    std::fs::create_dir_all(&work).unwrap();
    let refused = run_container(&sandbox, &spec(&work, false), &work.join("l")).unwrap_err();
    assert!(matches!(refused, SandboxError::Run { .. }), "{refused:?}");
}

#[test]
fn a_death_by_signal_reads_as_128_plus_the_signal() {
    let scratch = Scratch::new("signal");
    let path = stub_runtime(&scratch, "kill -9 $$");
    let sandbox = sandbox_with(&path, Vec::new(), Duration::from_secs(30));
    let work = scratch.path("work");
    std::fs::create_dir_all(&work).unwrap();
    assert_eq!(
        run_container(&sandbox, &spec(&work, false), &work.join("l")),
        Ok(ContainerExit(137))
    );
}

#[test]
fn docker_is_refused_when_the_sandbox_is_built() {
    let refused = RealSandbox::new(
        Arc::new(real_proc()),
        Arc::new(RealPipe::new(PATHS, None)),
        Runtime::Docker,
        Vec::new(),
    )
    .unwrap_err();
    assert_eq!(refused, SandboxError::RuntimeHasNoRunTimeout);
}

// ---- stage_executable and the runtime lookup -------------------------------------------

#[test]
fn an_executable_staged_into_the_tree_keeps_a_path_every_copy_of_the_tree_keeps() {
    let scratch = Scratch::new("stage");
    let bin = scratch.path("gate");
    std::fs::write(&bin, "#!/bin/sh\necho gate\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (_s, tree) = mutable("stage-tree", &[("a.rs", "x")]);
    let sandbox = sandbox();
    let relative = sandbox.stage_executable(&tree, &bin, "maintainer-merge").unwrap();
    assert_eq!(relative, ".container-target/stage/maintainer-merge");
    let staged = tree.root().join(&relative);
    assert_eq!(std::fs::metadata(&staged).unwrap().permissions().mode() & 0o777, 0o755);
    assert_eq!(std::fs::read_to_string(&staged).unwrap(), "#!/bin/sh\necho gate\n");
    // A cp -r of the tree, as the characterization suite makes, carries it along.
    let copy = scratch.path("copy");
    run("cp", &["-r", tree.root().to_str().unwrap(), copy.to_str().unwrap()], &scratch.0);
    assert!(copy.join(&relative).is_file());
    // The mutation leaves it alone even for a glob that matches its name.
    let changed = sandbox
        .apply_mutation(&tree, &globs(&["maintainer*", "*.rs"]), &sed("s/x/y/;s/gate/mutated/"))
        .unwrap();
    assert_eq!(changed.paths(), ["a.rs"]);
    assert_eq!(std::fs::read_to_string(&staged).unwrap(), "#!/bin/sh\necho gate\n");
}

#[test]
fn a_staged_name_that_is_not_one_plain_file_name_is_refused() {
    let scratch = Scratch::new("stagebad");
    let bin = scratch.path("gate");
    std::fs::write(&bin, "x").unwrap();
    let (_s, tree) = mutable("stagebad-tree", &[("a", "x")]);
    for bad in ["", "../x", "a/b", ".hidden", "-n", "a b", "a;b", &"x".repeat(65)] {
        assert_eq!(
            sandbox().stage_executable(&tree, &bin, bad),
            Err(SandboxError::BadStageName { name: bad.to_string() }),
            "{bad:?}"
        );
    }
    assert!(!tree.root().join(".container-target").exists());
}

#[test]
fn a_staged_source_that_is_a_directory_or_missing_is_refused() {
    let scratch = Scratch::new("stagesrc");
    let (_s, tree) = mutable("stagesrc-tree", &[("a", "x")]);
    for src in [scratch.0.clone(), scratch.path("missing")] {
        let refused = sandbox().stage_executable(&tree, &src, "ok").unwrap_err();
        assert!(matches!(refused, SandboxError::Stage { .. }), "{refused:?}");
    }
}

#[test]
fn staging_over_a_link_replaces_the_link_and_writes_nothing_through_it() {
    let scratch = Scratch::new("stagelink");
    let victim = scratch.path("victim");
    std::fs::write(&victim, "keep").unwrap();
    let bin = scratch.path("gate");
    std::fs::write(&bin, "new").unwrap();
    let (_s, tree) = mutable("stagelink-tree", &[("a", "x")]);
    std::fs::create_dir_all(tree.root().join(".container-target/stage")).unwrap();
    symlink(&victim, tree.root().join(".container-target/stage/gate")).unwrap();
    sandbox().stage_executable(&tree, &bin, "gate").unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
    assert_eq!(
        std::fs::read_to_string(tree.root().join(".container-target/stage/gate")).unwrap(),
        "new"
    );
}

#[test]
fn podman_is_preferred_to_docker_and_neither_is_none() {
    let scratch = Scratch::new("rt");
    let bin = scratch.path("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let make = |name: &str, mode: u32| {
        let path = bin.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    let search = std::ffi::OsString::from(bin.to_str().unwrap());
    assert_eq!(container_runtime(&search), None);
    make("docker", 0o755);
    assert_eq!(container_runtime(&search), Some(Runtime::Docker));
    make("podman", 0o644);
    assert_eq!(
        container_runtime(&search),
        Some(Runtime::Docker),
        "a file that is not executable is not found"
    );
    make("podman", 0o755);
    assert_eq!(container_runtime(&search), Some(Runtime::Podman));
}

#[test]
fn a_link_planted_at_the_log_path_is_refused_and_nothing_is_written_through_it() {
    // Today's bash `>>"$log"` follows such a link; the tree is the container's to write to.
    let scratch = Scratch::new("loglink");
    let path = stub_runtime(&scratch, "echo ran");
    let victim = scratch.path("victim");
    std::fs::write(&victim, "keep\n").unwrap();
    let work = scratch.path("work");
    std::fs::create_dir_all(&work).unwrap();
    symlink(&victim, work.join("mutated.log")).unwrap();
    let sandbox = sandbox_with(&path, Vec::new(), Duration::from_secs(30));
    let refused =
        run_container(&sandbox, &spec(&work, false), &work.join("mutated.log")).unwrap_err();
    assert!(
        matches!(&refused, SandboxError::Run { reason } if reason.contains("not a regular file")),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep\n");
    // a directory at the path is refused too
    std::fs::create_dir(work.join("d.log")).unwrap();
    assert!(run_container(&sandbox, &spec(&work, false), &work.join("d.log")).is_err());
}

#[test]
fn a_link_at_the_prefetch_log_is_replaced_and_not_followed() {
    let scratch = Scratch::new("prelink");
    let path = stub_runtime(&scratch, "echo ran");
    let victim = scratch.path("victim");
    std::fs::write(&victim, "keep\n").unwrap();
    let work = scratch.path("work");
    std::fs::create_dir_all(&work).unwrap();
    symlink(&victim, work.join("prefetch.log")).unwrap();
    let sandbox = sandbox_with(&path, Vec::new(), Duration::from_secs(30));
    run_container(&sandbox, &spec(&work, true), &work.join("prefetch.log")).unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep\n");
    assert_eq!(std::fs::read_to_string(work.join("prefetch.log")).unwrap(), "ran\n");
}

#[test]
fn staging_refuses_a_linked_container_target_or_stage_directory() {
    let scratch = Scratch::new("stagedir");
    let bin = scratch.path("gate");
    std::fs::write(&bin, "new").unwrap();
    let outside = scratch.path("outside");
    std::fs::create_dir_all(outside.join("stage")).unwrap();
    // a linked .container-target
    let (_s, tree) = mutable("stagedir-a", &[("a", "x")]);
    symlink(&outside, tree.root().join(".container-target")).unwrap();
    let refused = sandbox().stage_executable(&tree, &bin, "gate").unwrap_err();
    assert!(
        matches!(&refused, SandboxError::Stage { reason, .. } if reason.contains("not a plain directory")),
        "{refused:?}"
    );
    assert!(!outside.join("stage/gate").exists());
    // a linked stage directory under a real .container-target
    let (_s2, tree2) = mutable("stagedir-b", &[("a", "x")]);
    std::fs::create_dir_all(tree2.root().join(".container-target")).unwrap();
    symlink(outside.join("stage"), tree2.root().join(".container-target/stage")).unwrap();
    assert!(matches!(
        sandbox().stage_executable(&tree2, &bin, "gate"),
        Err(SandboxError::Stage { .. })
    ));
    assert!(!outside.join("stage/gate").exists());
    // a file standing where the directory must be
    let (_s3, tree3) = mutable("stagedir-c", &[("a", "x"), (".container-target", "file")]);
    assert!(matches!(
        sandbox().stage_executable(&tree3, &bin, "gate"),
        Err(SandboxError::Stage { .. })
    ));
}

#[test]
fn a_staged_gate_binary_runs_from_a_copy_of_the_tree_at_its_relative_path() {
    // The container mounts the tree at /repo and a suite may `cp -r /repo /tmp/w`. The host path
    // of the gate does not exist there, so the binary travels inside the tree and the command
    // names it relative to the tree root. Here the copy is a different directory and the staged
    // file is run from it by that relative path.
    let scratch = Scratch::new("stagerun");
    let bin = scratch.path("gate-src");
    std::fs::write(&bin, "#!/bin/sh\necho gate-ran-from \"$(pwd)\"\n").unwrap();
    let (_s, tree) = mutable("stagerun-tree", &[("a.rs", "x")]);
    let relative = sandbox().stage_executable(&tree, &bin, "maintainer-merge").unwrap();
    let copy = scratch.path("copy");
    run("cp", &["-r", tree.root().to_str().unwrap(), copy.to_str().unwrap()], &scratch.0);
    // Remove the original: nothing of the host path of the first tree may be needed.
    std::fs::remove_dir_all(tree.root()).unwrap();
    let staged = copy.join(&relative);
    let out = run(staged.to_str().unwrap(), &[], &copy);
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("gate-ran-from {}\n", copy.display())
    );
}

#[test]
fn the_changed_paths_come_back_sorted_whatever_order_find_lists_them_in() {
    let scratch = Scratch::new("findorder");
    let stub = scratch.path("stubbin");
    std::fs::create_dir_all(&stub).unwrap();
    let script = stub.join("find");
    std::fs::write(
        &script,
        "#!/bin/bash\n/usr/bin/find \"$@\" | python3 -c 'import sys; d=[x for x in sys.stdin.buffer.read().split(b\"\\0\") if x]; sys.stdout.buffer.write(b\"\".join(x+b\"\\0\" for x in reversed(d)))'\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (_s, tree) = mutable(
        "findorder-tree",
        &[("b.rs", "x"), ("a.rs", "x"), ("c/z.rs", "x"), ("c/a.rs", "x")],
    );
    let path = format!("{}:{PATHS}", stub.display());
    let sandbox = RealSandbox::new(
        Arc::new(RealProc::new(path.as_str(), git_env())),
        Arc::new(RealPipe::new(PATHS, None)),
        Runtime::Podman,
        Vec::new(),
    )
    .unwrap();
    let mut changed = None;
    for _ in 0..50 {
        match sandbox.apply_mutation(&tree, &globs(&["*.rs"]), &sed("s/x/y/")) {
            Err(SandboxError::Run { reason }) if reason.contains("Text file busy") => {
                std::thread::sleep(Duration::from_millis(20));
            }
            other => {
                changed = Some(other.unwrap());
                break;
            }
        }
    }
    assert_eq!(changed.unwrap().paths(), ["a.rs", "b.rs", "c/a.rs", "c/z.rs"]);
}

#[test]
fn the_debug_output_of_a_sandbox_names_the_runtime_variables_and_never_their_values() {
    let env = pick_env(&["HOME", "XDG_RUNTIME_DIR"], |n| Some(format!("secret-value-of-{n}")));
    let sandbox = RealSandbox::new(
        Arc::new(real_proc()),
        Arc::new(RealPipe::new(PATHS, None)),
        Runtime::Podman,
        env,
    )
    .unwrap();
    let shown = format!("{sandbox:?}");
    assert!(shown.contains("HOME") && shown.contains("XDG_RUNTIME_DIR"), "{shown}");
    assert!(!shown.contains("secret-value"), "{shown}");
}

fn staged_fixture(name: &str) -> (Scratch, Scratch, ContainedTree, RealSandbox, String, PathBuf) {
    let scratch = Scratch::new(name);
    let bin = scratch.path("gate");
    std::fs::write(&bin, "#!/bin/sh\necho gate\n").unwrap();
    let (tree_dir, tree) = mutable(&format!("{name}-tree"), &[("a.rs", "x")]);
    let path = stub_runtime(&scratch, "echo ran");
    let sandbox = sandbox_with(&path, Vec::new(), Duration::from_secs(30));
    let relative = sandbox.stage_executable(&tree, &bin, "gate").unwrap();
    (scratch, tree_dir, tree, sandbox, relative, bin)
}

#[test]
fn a_staged_binary_overwritten_by_an_earlier_container_blocks_the_next_run_until_it_is_staged_again()
 {
    let (scratch, _keep, tree, sandbox, relative, bin) = staged_fixture("stagehash");
    let work = tree.root().to_path_buf();
    let log = scratch.path("log");
    assert_eq!(run_container(&sandbox, &spec(&work, false), &log), Ok(ContainerExit(0)));
    // the first container replaces the staged gate with its own program
    std::fs::write(tree.root().join(&relative), "#!/bin/sh\necho planted\n").unwrap();
    let refused = run_container(&sandbox, &spec(&work, false), &log).unwrap_err();
    assert_eq!(refused, SandboxError::StagedChanged { path: tree.root().join(&relative) });
    // staging again restores the file and records the new hash
    sandbox.stage_executable(&tree, &bin, "gate").unwrap();
    assert_eq!(
        std::fs::read_to_string(tree.root().join(&relative)).unwrap(),
        "#!/bin/sh\necho gate\n"
    );
    assert_eq!(run_container(&sandbox, &spec(&work, false), &log), Ok(ContainerExit(0)));
}

#[test]
fn a_staged_binary_replaced_by_a_link_or_removed_blocks_the_run() {
    let (scratch, _keep, tree, sandbox, relative, _bin) = staged_fixture("stagelink2");
    let work = tree.root().to_path_buf();
    let staged = tree.root().join(&relative);
    let log = scratch.path("log");
    let copy = scratch.path("copy");
    std::fs::copy(&staged, &copy).unwrap();
    std::fs::remove_file(&staged).unwrap();
    symlink(&copy, &staged).unwrap();
    assert!(matches!(
        run_container(&sandbox, &spec(&work, false), &log),
        Err(SandboxError::StagedChanged { .. })
    ));
    std::fs::remove_file(&staged).unwrap();
    assert!(matches!(
        run_container(&sandbox, &spec(&work, false), &log),
        Err(SandboxError::StagedChanged { .. })
    ));
}

#[test]
fn staging_a_changed_binary_again_replaces_the_recorded_hash() {
    let (scratch, _keep, tree, sandbox, relative, bin) = staged_fixture("restage");
    let log = scratch.path("log");
    std::fs::write(&bin, "#!/bin/sh\necho gate-v2\n").unwrap();
    sandbox.stage_executable(&tree, &bin, "gate").unwrap();
    assert_eq!(
        std::fs::read_to_string(tree.root().join(&relative)).unwrap(),
        "#!/bin/sh\necho gate-v2\n"
    );
    // Only the second hash is recorded: a leftover record of the first would refuse this run.
    assert_eq!(run_container(&sandbox, &spec(tree.root(), false), &log), Ok(ContainerExit(0)));
}

#[test]
fn a_changed_staged_file_is_refused_with_a_sentence_that_says_what_to_do() {
    let refused =
        SandboxError::StagedChanged { path: PathBuf::from("/w/.container-target/stage/gate") };
    assert_eq!(
        refused.to_string(),
        "the staged file /w/.container-target/stage/gate is not the one that was staged; stage it again before this run"
    );
}
