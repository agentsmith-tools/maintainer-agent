//! Removing the temporary files when a signal ends the run (decision `P4B-12`).
//!
//! The scripts set `trap 'rm -rf "$work"' EXIT` (the probe) or `trap 'rm -rf "$gen_dir"' EXIT`
//! (`check-claims`). Bash runs that trap when the script dies of SIGHUP, SIGINT or SIGTERM too, and
//! then ends by the same signal, so a caller sees the death a shell reports as 129, 130 or 143. Rust
//! runs no destructor on a fatal signal, so the directory would stay in `TMPDIR`. [`install`] starts a
//! thread that waits for those signals, removes every armed path, and re-raises the signal with its
//! default action, which gives the same status the bash gives.
//!
//! The normal path removes its files through the [`Host`](crate::host::Host) port and then drops its
//! [`Armed`], which only disarms. A signal that arrives meanwhile removes the files from the real file
//! system, because a signal handler has no port.
//!
//! A signal that the caller left ignored when the program started stays ignored, because bash does not
//! trap what it inherited as ignored. A child process keeps running when the signal names this process
//! alone: `RealProc` starts a child in its own process group, so a terminal's Ctrl-C does not reach it,
//! where it reaches the bash's children. The five programs run short children except the suite that
//! `check-claims` runs, which finishes on its own.

use signal_hook::consts::signal::{SIGALRM, SIGHUP, SIGINT, SIGTERM, SIGUSR1, SIGUSR2};
use signal_hook::iterator::Signals;
use signal_hook::low_level::emulate_default_handler;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The signals the bash's `EXIT` trap runs for when they kill the script and that a caller can send.
const FATAL: [i32; 6] = [SIGHUP, SIGINT, SIGTERM, SIGUSR1, SIGUSR2, SIGALRM];

static ARMED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn armed() -> std::sync::MutexGuard<'static, Vec<PathBuf>> {
    ARMED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A path that a fatal signal removes. Dropping the guard disarms it and removes nothing.
#[derive(Debug)]
pub struct Armed {
    path: PathBuf,
}

impl Armed {
    /// Puts `path` under the guard: the trap line.
    #[must_use]
    pub fn new(path: &Path) -> Self {
        armed().push(path.to_path_buf());
        Self { path: path.to_path_buf() }
    }
}

impl Drop for Armed {
    fn drop(&mut self) {
        armed().retain(|candidate| *candidate != self.path);
    }
}

/// Removes every armed path now, a directory with what is in it.
pub fn run_now() {
    let paths: Vec<PathBuf> = armed().clone();
    for path in paths {
        if path.is_dir() {
            drop(std::fs::remove_dir_all(&path));
        } else {
            drop(std::fs::remove_file(&path));
        }
    }
}

/// The signals this process had ignored when it started, from `SigIgn` in `/proc/self/status`.
fn ignored_at_start() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("SigIgn:"))
                .and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok())
        })
        .unwrap_or(0)
}

/// Starts the thread that removes the armed paths on a fatal signal and then dies of the signal. Call
/// it once, at the start of `main`.
pub fn install() {
    let ignored = ignored_at_start();
    let wanted: Vec<i32> = FATAL
        .iter()
        .copied()
        .filter(|signal| {
            u32::try_from(*signal - 1).ok().is_none_or(|bit| ignored & (1_u64 << bit) == 0)
        })
        .collect();
    let Ok(mut signals) = Signals::new(wanted) else {
        return;
    };
    drop(std::thread::spawn(move || {
        // The first signal ends the program, so this waits for one value.
        if let Some(signal) = signals.forever().next() {
            run_now();
            // The default action of the signal: the process ends by it, as the bash ends after its trap.
            drop(emulate_default_handler(signal));
            std::process::exit(128 + signal);
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mx-checks-cleanup-{tag}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(dir.join("r")).expect("the directory can be made");
        std::fs::write(dir.join("r/file"), "x").expect("the file can be written");
        dir
    }

    #[test]
    fn run_now_removes_an_armed_directory_and_an_armed_file() {
        let dir = scratch("armed");
        let file = dir.join("r/file");
        let guard_dir = Armed::new(&dir.join("r"));
        let guard_file = Armed::new(&file);
        run_now();
        assert!(!dir.join("r").exists(), "the directory was not removed");
        drop((guard_dir, guard_file));
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn dropping_the_guard_disarms_and_removes_nothing() {
        let dir = scratch("disarm");
        drop(Armed::new(&dir));
        assert!(dir.exists(), "the drop removed the directory");
        run_now();
        assert!(dir.exists(), "a disarmed directory was removed by a signal");
        drop(std::fs::remove_dir_all(&dir));
    }
}
