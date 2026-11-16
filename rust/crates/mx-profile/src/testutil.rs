//! Helpers for the unit tests. Compiled only for tests.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// A directory under the cargo target directory, removed on drop. The target
/// directory is on disk, where `/tmp` is a small RAM filesystem on this host.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(tag: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let exe = std::env::current_exe().expect("the test binary has a path");
        let base = exe
            .parent()
            .and_then(Path::parent)
            .map_or_else(std::env::temp_dir, |target| target.join("mx-profile-scratch"));
        let dir = base.join(format!("{tag}-{}-{unique}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).expect("the scratch directory can be made");
        Self(dir)
    }

    /// Writes `contents` to `name` inside the directory and returns the path.
    pub fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, contents).expect("the scratch file can be written");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.0));
    }
}
