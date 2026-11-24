//! The real sandbox: git, find, sed and the container runtime, started through `mx_ports`.
//!
//! Each method is one bash function of `bin/maintainer-merge`, and keeps its order of steps:
//!
//! | method | bash | process |
//! |---|---|---|
//! | `extract_tree` | `extract_tree` | `git read-tree`, `git checkout-index`, `git ls-tree` |
//! | `tree_escapes` | `tree_escapes` | none: the tree is walked here, with Python's `realpath` rules |
//! | `apply_mutation` | `apply_mutation` | `find`, then `sed --sandbox` |
//! | `scratch_dir` | `scratch_dir` | none |
//! | `seed_index` | the `git init` block and `mkdir -p .container-target` | `git init`, `git add` |
//! | `run_container` | `run_in_container` and the prefetch `podman run` | the runtime, through `Pipe` |
//!
//! # The container runs a copy of the tree
//!
//! The container mounts the extracted tree at `/repo`, and a suite may copy it again
//! (`cp -r /repo /tmp/w`). A command that names the Rust gate's binary by its absolute host
//! path finds nothing inside. [`RealSandbox::stage_executable`] solves that: it copies the
//! file into the tree at `.container-target/stage/<name>`, a path relative to the tree root
//! that every copy of the tree keeps. That directory is outside the mutation (`find` skips
//! `./.container-target/*`) and outside `git add -A` (it is created after the index is
//! seeded), and the suite's command names the file as `/repo/.container-target/stage/<name>`
//! (or `.container-target/stage/<name>` from the root of a copy).
//!
//! # Staged files
//!
//! A container of an earlier run has the tree mounted writable and could overwrite a staged
//! binary. `stage_executable` records the hash of the copy, and `run_container` checks every
//! recorded file before it starts a container (`StagedChanged`). The caller must stage the
//! binary again immediately before EACH container run (clean, mutated, and the prefetch): the
//! call copies afresh and records the new hash, so the check is the net for a run that forgot.
//!
//! # Cleanup
//!
//! Scratch directories made by [`Sandbox::scratch_dir`] are removed by [`Drop`] and by
//! [`RealSandbox::remove_scratch`]. A binary whose `main` ends in `std::process::exit` skips
//! `Drop`, so it calls `remove_scratch` first.

use crate::{
    ContainedTree, ContainerExit, ContainerSpec, Escape, ExtractedTree, LogMode, Mutated, Sandbox,
    SandboxError, SedExpression, realpath,
};
use mx_core::NonEmpty;
use mx_ports::{
    Pipe, PipelineSpec, Proc, ProcError, ProcExit, ProcOutput, ProcSpec, Sink, Source, StageSpec,
};
use mx_suite::{MutateGlob, Runtime};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// What the runtime's own client may take beyond the container's `--timeout` before the
/// adapter kills it: podman needs a moment to stop and remove the container.
const RUNTIME_GRACE: Duration = Duration::from_mins(5);
/// A git call over a large repository. The bash has no bound.
const GIT_TIMEOUT: Duration = Duration::from_mins(15);
/// A `find` or `sed` over the extracted tree.
const TREE_TIMEOUT: Duration = Duration::from_mins(10);
/// The most bytes of file names one `sed` call receives (the bash lets `xargs` choose).
const SED_ARGV_BYTES: usize = 60_000;
/// What a timed-out client reads as, the `timeout(1)` convention `looks_killed` knows.
const TIMED_OUT: i32 = 124;

/// The variables a container runtime needs to find its storage, its rootless socket and its
/// configuration. The caller picks them from its own environment ([`pick_env`]) and passes
/// them to [`RealSandbox::new`], because a pipeline stage starts with an empty environment.
pub const RUNTIME_ENV_NAMES: [&str; 10] = [
    "HOME",
    "USER",
    "LOGNAME",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "DBUS_SESSION_BUS_ADDRESS",
    "CONTAINERS_CONF",
    "CONTAINERS_STORAGE_CONF",
    "TMPDIR",
];

/// The `(name, value)` pairs of `names` that `lookup` finds. A name it does not find is left out.
pub fn pick_env(names: &[&str], lookup: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    names.iter().filter_map(|n| lookup(n).map(|v| ((*n).to_string(), v))).collect()
}

/// `container_runtime`: podman if it is on the search path, docker otherwise, nothing if neither.
/// `command -v` finds an executable regular file in a directory of the path, which is what is
/// checked here, so no process is started.
#[must_use]
pub fn container_runtime(search_path: &OsStr) -> Option<Runtime> {
    let found = |program: &str| {
        std::env::split_paths(search_path).any(|dir| {
            std::fs::metadata(dir.join(program))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    };
    if found("podman") {
        Some(Runtime::Podman)
    } else if found("docker") {
        Some(Runtime::Docker)
    } else {
        None
    }
}

/// The sandbox, for real.
pub struct RealSandbox {
    proc: Arc<dyn Proc + Send + Sync>,
    pipe: Arc<dyn Pipe + Send + Sync>,
    runtime: Runtime,
    runtime_env: Vec<(String, String)>,
    grace: Duration,
    scratch: Mutex<Vec<PathBuf>>,
    /// Each staged file and the hash of what was copied.
    staged: Mutex<Vec<(PathBuf, [u8; 32])>>,
}

impl std::fmt::Debug for RealSandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.runtime_env.iter().map(|(n, _)| n.as_str()).collect();
        f.debug_struct("RealSandbox")
            .field("runtime", &self.runtime)
            .field("runtime_env_names", &names)
            .finish_non_exhaustive()
    }
}

impl RealSandbox {
    /// A sandbox that starts git through `proc` and `runtime` through `pipe`, giving the
    /// runtime `runtime_env` and nothing else.
    ///
    /// Docker is refused here (`RuntimeHasNoRunTimeout`): `docker run` has no `--timeout`, so
    /// the verify container could not be bounded, and the failure at run time would read as
    /// "the test does not pass unmutated" (rv264 O3).
    pub fn new(
        proc: Arc<dyn Proc + Send + Sync>,
        pipe: Arc<dyn Pipe + Send + Sync>,
        runtime: Runtime,
        runtime_env: Vec<(String, String)>,
    ) -> Result<Self, SandboxError> {
        if runtime == Runtime::Docker {
            return Err(SandboxError::RuntimeHasNoRunTimeout);
        }
        Ok(Self {
            proc,
            pipe,
            runtime,
            runtime_env,
            grace: RUNTIME_GRACE,
            scratch: Mutex::new(Vec::new()),
            staged: Mutex::new(Vec::new()),
        })
    }

    /// Changes how long the runtime's client may outlive the container's `--timeout` before the
    /// adapter kills it. The default is five minutes; a test shortens it.
    #[must_use]
    pub fn with_runtime_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    /// Removes every scratch directory this sandbox made. Called by `Drop` as well; calling it
    /// twice is harmless.
    pub fn remove_scratch(&self) {
        let dirs: Vec<PathBuf> =
            std::mem::take(&mut *self.scratch.lock().unwrap_or_else(PoisonError::into_inner));
        for dir in dirs {
            drop(std::fs::remove_dir_all(dir));
        }
    }

    /// Copies the executable `src` into the tree as `.container-target/stage/<name>` and returns
    /// that path relative to the tree root. See the module documentation for why.
    ///
    /// `name` is one file name of ASCII letters, digits, `.`, `_` and `-`, not starting with
    /// `.` or `-`. `src` must be a regular file. The copy is mode 755.
    pub fn stage_executable(
        &self,
        tree: &ContainedTree,
        src: &Path,
        name: &str,
    ) -> Result<String, SandboxError> {
        let ok_name = !name.is_empty()
            && name.len() <= 64
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            && !name.starts_with(['.', '-']);
        if !ok_name {
            return Err(SandboxError::BadStageName { name: name.to_string() });
        }
        let meta = std::fs::metadata(src)
            .map_err(|e| SandboxError::Stage { path: src.to_path_buf(), reason: e.to_string() })?;
        if !meta.is_file() {
            return Err(SandboxError::Stage {
                path: src.to_path_buf(),
                reason: "not a regular file".into(),
            });
        }
        let stage_err = |path: &Path, e: std::io::Error| SandboxError::Stage {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };
        // Both directories must be plain directories: a link there (planted by an earlier
        // container run, which has the tree mounted writable) would carry the copy elsewhere.
        let target = tree.root().join(".container-target");
        let dir = target.join("stage");
        ensure_plain_dir(&target)?;
        ensure_plain_dir(&dir)?;
        let dest = dir.join(name);
        // A leftover link is removed, never written through; create_new refuses to follow one.
        match std::fs::symlink_metadata(&dest) {
            Ok(_) => std::fs::remove_file(&dest).map_err(|e| stage_err(&dest, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(stage_err(&dest, e)),
        }
        let mut from = std::fs::File::open(src).map_err(|e| stage_err(src, e))?;
        let mut to = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest)
            .map_err(|e| stage_err(&dest, e))?;
        std::io::copy(&mut from, &mut to).map_err(|e| stage_err(&dest, e))?;
        to.set_permissions(std::fs::Permissions::from_mode(0o755))
            .map_err(|e| stage_err(&dest, e))?;
        drop(to);
        // The hash of what is on disk now, checked again before every container run.
        let hash = sha256_of(&dest).map_err(|e| stage_err(&dest, e))?;
        let mut staged = self.staged.lock().unwrap_or_else(PoisonError::into_inner);
        staged.retain(|(path, _)| path != &dest);
        staged.push((dest, hash));
        Ok(format!(".container-target/stage/{name}"))
    }

    fn run(
        &self,
        argv: Vec<String>,
        cwd: Option<&Path>,
        env: Vec<(String, String)>,
        timeout: Duration,
    ) -> Result<ProcOutput, SandboxError> {
        let first = argv[0].clone();
        let mut spec = ProcSpec::command(&first, argv.into_iter().skip(1), timeout);
        spec.env = env;
        spec.cwd = cwd.map(Path::to_path_buf);
        let out = self
            .proc
            .run(&spec)
            .map_err(|e: ProcError| SandboxError::Run { reason: e.to_string() })?;
        if out.exit == ProcExit::TimedOut {
            return Err(SandboxError::Run {
                reason: format!("{first} ran past {} s", timeout.as_secs()),
            });
        }
        Ok(out)
    }

    fn git(&self, args: &[&str], env: Vec<(String, String)>) -> Result<ProcOutput, SandboxError> {
        let mut words = vec!["git".to_string()];
        words.extend(args.iter().map(|a| (*a).to_string()));
        self.run(words, None, env, GIT_TIMEOUT)
    }
}

impl Drop for RealSandbox {
    fn drop(&mut self) {
        self.remove_scratch();
    }
}

/// `path` is a directory that is not a link, made when it is missing. `create_dir_all` and
/// `metadata` follow links, so neither is used.
fn ensure_plain_dir(path: &Path) -> Result<(), SandboxError> {
    let err = |reason: String| SandboxError::Stage { path: path.to_path_buf(), reason };
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(err("is not a plain directory (a link or a file stands there)".into())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(path).map_err(|e| err(e.to_string()))
        }
        Err(e) => Err(err(e.to_string())),
    }
}

fn text(path: &Path) -> Result<&str, SandboxError> {
    path.to_str().ok_or_else(|| SandboxError::Run {
        reason: format!("{} is not valid UTF-8", path.display()),
    })
}

fn succeeded(out: &ProcOutput) -> bool {
    out.exit == ProcExit::Code(0)
}

/// `grep -zvc '^160000 '` over `git ls-tree -r -z`: the records that are not submodules.
fn count_listed(listing: &[u8]) -> u64 {
    listing
        .split(|b| *b == 0)
        .filter(|record| !record.is_empty() && !record.starts_with(b"160000 "))
        .count() as u64
}

/// `find "$dir" \( -type f -o -type l \)`, counted. Links are not followed.
fn count_files(dir: &Path) -> std::io::Result<u64> {
    let mut count = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            count += count_files(&entry.path())?;
        } else if kind.is_file() || kind.is_symlink() {
            count += 1;
        }
    }
    Ok(count)
}

/// Every symlink under `dir` whose resolution leaves `root`.
fn walk_escapes(root: &Path, dir: &Path, out: &mut Vec<Escape>) -> Result<(), SandboxError> {
    let unreadable = |path: &Path, e: std::io::Error| SandboxError::Run {
        reason: format!("cannot read {}: {e}", path.display()),
    };
    for entry in std::fs::read_dir(dir).map_err(|e| unreadable(dir, e))? {
        let entry = entry.map_err(|e| unreadable(dir, e))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| unreadable(&path, e))?;
        if kind.is_symlink() {
            let target = realpath::realpath(&path);
            let inside = target == root || target.starts_with(root);
            if !inside {
                let link = path.strip_prefix(root).unwrap_or(&path);
                let written = std::fs::read_link(&path).map_err(|e| unreadable(&path, e))?;
                out.push(Escape {
                    link: String::from_utf8_lossy(link.as_os_str().as_bytes()).into_owned(),
                    target: String::from_utf8_lossy(written.as_os_str().as_bytes()).into_owned(),
                });
            }
        } else if kind.is_dir() {
            walk_escapes(root, &path, out)?;
        }
    }
    Ok(())
}

fn sha256_of(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().into())
}

fn random_name() -> std::io::Result<String> {
    const ALPHABET: &[u8; 62] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut bytes = [0_u8; 8];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| char::from(ALPHABET[usize::from(*b) % ALPHABET.len()])).collect())
}

impl Sandbox for RealSandbox {
    fn extract_tree(
        &self,
        repo: &Path,
        rev: &str,
        dest: &Path,
    ) -> Result<ExtractedTree, SandboxError> {
        let parent =
            dest.parent().ok_or_else(|| SandboxError::ScratchDir { dir: dest.to_path_buf() })?;
        let tmp = self.scratch_dir(parent)?;
        let index = tmp.join("index");
        let env = vec![("GIT_INDEX_FILE".to_string(), text(&index)?.to_string())];
        let repo_text = text(repo)?;
        let dest_text = text(dest)?;
        // A revision that starts with `-` would be one of git's own options.
        if rev.starts_with('-') {
            return Err(SandboxError::ReadTree { rev: rev.to_string() });
        }
        let read_tree = self.git(
            &["-C", repo_text, "-c", "core.hooksPath=/dev/null", "read-tree", rev],
            env.clone(),
        )?;
        if !succeeded(&read_tree) {
            return Err(SandboxError::ReadTree { rev: rev.to_string() });
        }
        let prefix = format!("--prefix={dest_text}/");
        let checkout = self.git(
            &["-C", repo_text, "-c", "core.hooksPath=/dev/null", "checkout-index", "-a", &prefix],
            env,
        )?;
        if !succeeded(&checkout) {
            return Err(SandboxError::CheckoutIndex {
                rev: rev.to_string(),
                dir: dest.to_path_buf(),
            });
        }
        let listing = self.git(&["-C", repo_text, "ls-tree", "-r", "-z", rev], Vec::new())?;
        if !succeeded(&listing) {
            return Err(SandboxError::LsTree { rev: rev.to_string() });
        }
        let listed = count_listed(&listing.stdout);
        let found = count_files(dest).map_err(|e| SandboxError::Run {
            reason: format!("cannot count the files of {}: {e}", dest.display()),
        })?;
        ExtractedTree::counted(dest.to_path_buf(), rev, listed, found)
    }

    fn tree_escapes(&self, tree: &ExtractedTree) -> Result<Vec<Escape>, SandboxError> {
        let root = realpath::realpath(tree.root());
        let mut found = Vec::new();
        walk_escapes(&root, &root, &mut found)?;
        // Python sorts the printed `link -> target` strings.
        found.sort_by_key(ToString::to_string);
        Ok(found)
    }

    fn apply_mutation(
        &self,
        tree: &ContainedTree,
        globs: &NonEmpty<MutateGlob>,
        expression: &SedExpression,
    ) -> Result<Mutated, SandboxError> {
        let root = tree.root();
        let mut find: Vec<String> =
            ["find", ".", "-type", "f", "("].iter().map(ToString::to_string).collect();
        for (i, glob) in globs.iter().enumerate() {
            if i > 0 {
                find.push("-o".into());
            }
            find.extend(["-name".to_string(), glob.as_str().to_string()]);
        }
        find.push(")".into());
        find.extend(
            ["-not", "-path", "./.container-target/*", "-not", "-path", "./.git/*", "-print0"]
                .map(String::from),
        );
        let listed = self.run(find, Some(root), Vec::new(), TREE_TIMEOUT)?;
        if !succeeded(&listed) {
            return Err(SandboxError::MutationFailed);
        }
        let files: Vec<&[u8]> =
            listed.stdout.split(|b| *b == 0).filter(|name| !name.is_empty()).collect();
        let mut names = Vec::with_capacity(files.len());
        for raw in &files {
            let name = std::str::from_utf8(raw).map_err(|_| SandboxError::MutationFailed)?;
            names.push(name.to_string());
        }
        let hash_all = |names: &[String]| -> Result<HashMap<String, [u8; 32]>, SandboxError> {
            names
                .iter()
                .map(|n| {
                    sha256_of(&root.join(n))
                        .map(|h| (n.clone(), h))
                        .map_err(|_| SandboxError::MutationFailed)
                })
                .collect()
        };
        let before = hash_all(&names)?;
        let mut batch: Vec<&String> = Vec::new();
        let mut bytes = 0;
        let mut batches: Vec<Vec<&String>> = Vec::new();
        for name in &names {
            if !batch.is_empty() && bytes + name.len() > SED_ARGV_BYTES {
                batches.push(std::mem::take(&mut batch));
                bytes = 0;
            }
            bytes += name.len() + 1;
            batch.push(name);
        }
        if !batch.is_empty() {
            batches.push(batch);
        }
        for batch in batches {
            let mut sed: Vec<String> = ["sed", "--sandbox", "-i", "-e"].map(String::from).to_vec();
            sed.push(expression.as_str().to_string());
            sed.push("--".into());
            sed.extend(batch.into_iter().cloned());
            let done = self.run(sed, Some(root), Vec::new(), TREE_TIMEOUT)?;
            if !succeeded(&done) {
                return Err(SandboxError::MutationFailed);
            }
        }
        let after = hash_all(&names)?;
        let changed: Vec<String> = names
            .iter()
            .filter(|n| before.get(*n) != after.get(*n))
            .map(|n| n.strip_prefix("./").unwrap_or(n).to_string())
            .collect();
        // `find` lists a tree in directory order, which differs between two checkouts of the
        // same commit; the changed paths are sorted so a refusal that prints them is stable
        // (P3A-17). The bash prints them in `find` order.
        let mut changed = changed;
        changed.sort();
        let globs: Vec<MutateGlob> = globs.iter().cloned().collect();
        Mutated::new(changed, &globs)
            .map_err(|nothing| SandboxError::NothingChanged { globs: nothing.globs })
    }

    fn scratch_dir(&self, parent: &Path) -> Result<PathBuf, SandboxError> {
        let parent_err = || SandboxError::ScratchParent { dir: parent.to_path_buf() };
        std::fs::create_dir_all(parent).map_err(|_| parent_err())?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| parent_err())?;
        for _ in 0..100 {
            let name = random_name()
                .map_err(|_| SandboxError::ScratchDir { dir: parent.to_path_buf() })?;
            let dir = parent.join(name);
            match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {
                    self.scratch.lock().unwrap_or_else(PoisonError::into_inner).push(dir.clone());
                    return Ok(dir);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => break,
            }
        }
        Err(SandboxError::ScratchDir { dir: parent.to_path_buf() })
    }

    fn seed_index(&self, tree: &ContainedTree) -> Result<(), SandboxError> {
        let root = text(tree.root())?;
        let init = self.git(
            &[
                "-c",
                "init.defaultBranch=main",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "init.templateDir=",
                "init",
                "-q",
                root,
            ],
            Vec::new(),
        );
        let seeded = match init {
            Ok(out) if succeeded(&out) => self
                .git(&["-C", root, "-c", "core.hooksPath=/dev/null", "add", "-A", "-f"], Vec::new())
                .is_ok_and(|out| succeeded(&out)),
            _ => false,
        };
        // `mkdir -p "$work/.container-target"` follows in the bash whether or not the index
        // was seeded, and after `git add`, so the directory is never in the index.
        std::fs::create_dir_all(tree.root().join(".container-target")).map_err(|e| {
            SandboxError::Run { reason: format!("cannot create .container-target: {e}") }
        })?;
        if seeded { Ok(()) } else { Err(SandboxError::IndexNotSeeded) }
    }

    fn run_container(
        &self,
        spec: &ContainerSpec,
        log: &Path,
    ) -> Result<ContainerExit, SandboxError> {
        // `>"$work/prefetch.log"` starts the file over; the pipeline only appends.
        if spec.log_mode() == LogMode::Truncate {
            match std::fs::remove_file(log) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(SandboxError::Run {
                        reason: format!("cannot start {} over: {e}", log.display()),
                    });
                }
            }
        }
        // A staged binary an earlier container overwrote is not the gate that was staged.
        let staged = self.staged.lock().unwrap_or_else(PoisonError::into_inner).clone();
        for (path, hash) in staged {
            let plain = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file());
            if !plain || sha256_of(&path).ok() != Some(hash) {
                return Err(SandboxError::StagedChanged { path });
            }
        }
        // The tree is mounted writable, so an earlier container could have left a link at the
        // log path; the host's append would write through it. Nothing runs in the tree between
        // this check and the open, because the container of this run has not started yet.
        match std::fs::symlink_metadata(log) {
            Ok(meta) if !meta.is_file() => {
                return Err(SandboxError::Run {
                    reason: format!(
                        "{} is not a regular file, so it is not appended to",
                        log.display()
                    ),
                });
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(SandboxError::Run {
                    reason: format!("cannot check {}: {e}", log.display()),
                });
            }
        }
        let mut stage = StageSpec::new(spec.argv());
        stage.env.clone_from(&self.runtime_env);
        stage.stdin = Source::Empty;
        stage.stdout = Sink::AppendFile(log.to_path_buf());
        stage.stderr = Sink::SameAsStdout;
        let pipeline = PipelineSpec {
            stages: NonEmpty::new(stage, []),
            timeout: Duration::from_secs(spec.timeout().secs()) + self.grace,
        };
        let outcome = self
            .pipe
            .run_pipeline(&pipeline)
            .map_err(|e| SandboxError::Run { reason: e.to_string() })?;
        match outcome.pipefail() {
            ProcExit::Code(code) => Ok(ContainerExit(code)),
            ProcExit::TimedOut => Ok(ContainerExit(TIMED_OUT)),
        }
    }
}
