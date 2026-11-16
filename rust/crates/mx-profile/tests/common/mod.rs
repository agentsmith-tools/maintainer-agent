//! Shared fixtures for the integration tests.
#![allow(dead_code)]

use mx_ports::RealProc;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// A directory under the cargo target directory, removed on drop. The target
/// directory is on disk; `/tmp` is a small RAM filesystem on this host.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(tag: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mx-profile");
        let dir = base.join(format!("{tag}-{}-{unique}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).expect("the scratch directory can be made");
        Self(dir.canonicalize().expect("the scratch directory resolves"))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Writes `contents` to `rel` under the directory, making parents, and returns the path.
    pub fn write(&self, rel: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().expect("a file has a parent")).unwrap();
        std::fs::write(&path, contents).expect("the scratch file can be written");
        path
    }

    pub fn read(&self, rel: &str) -> Vec<u8> {
        std::fs::read(self.0.join(rel)).expect("the scratch file can be read")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.0));
    }
}

/// The real process adapter, searching the system directories only.
pub fn real_proc() -> RealProc {
    RealProc::new("/usr/bin:/bin", Vec::new())
}

/// The repository's `lib/` directory.
pub fn repo_lib() -> PathBuf {
    PathBuf::from(format!("{}/../../../lib", env!("CARGO_MANIFEST_DIR")))
        .canonicalize()
        .expect("the repository lib directory exists")
}

/// A `Proc` that runs the real adapter and records every call it is given.
pub struct RecordingProc {
    inner: RealProc,
    pub calls: std::sync::Mutex<Vec<mx_ports::ProcSpec>>,
}

impl RecordingProc {
    pub fn new(inner: RealProc) -> Self {
        Self { inner, calls: std::sync::Mutex::new(Vec::new()) }
    }

    pub fn specs(&self) -> Vec<mx_ports::ProcSpec> {
        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

impl mx_ports::Proc for RecordingProc {
    fn run(&self, spec: &mx_ports::ProcSpec) -> Result<mx_ports::ProcOutput, mx_ports::ProcError> {
        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(spec.clone());
        self.inner.run(spec)
    }
}
