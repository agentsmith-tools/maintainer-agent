//! Removing the scratch clone when a signal ends the run (decision P1H-7).
//!
//! The bash sets `trap "rm -rf '$work'" EXIT` right after it makes the scratch
//! directory. Bash runs that trap when the script dies of SIGHUP, SIGINT or
//! SIGTERM too, and then ends by the same signal, so a caller sees the death a
//! shell reports as 129, 130 or 143. Rust runs no destructor on a fatal signal,
//! so the directory would stay in the state directory. [`install`] starts a
//! thread that waits for those signals, removes every armed directory, and
//! re-raises the signal with its default action, which gives the same status
//! the bash gives.
//!
//! A signal that the caller left ignored when the program started stays
//! ignored, because bash does not trap what it inherited as ignored.
//!
//! A child process keeps running when the signal names this process alone. The
//! bash behaves the same for `kill PID`; a terminal's Ctrl-C reaches the bash's
//! children as well and does not reach a child here, which `RealProc` starts in
//! its own process group.

use signal_hook::consts::signal::{SIGALRM, SIGHUP, SIGINT, SIGTERM, SIGUSR1, SIGUSR2};
use signal_hook::iterator::Signals;
use signal_hook::low_level::emulate_default_handler;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// The signals the bash's EXIT trap runs for when they kill the script and
/// that a caller can send: hang-up, interrupt, terminate, the two user signals
/// and the alarm.
const FATAL: [i32; 6] = [SIGHUP, SIGINT, SIGTERM, SIGUSR1, SIGUSR2, SIGALRM];

/// How often a removal is tried again when a child still writes into the tree.
const RETRIES: u32 = 10;
const RETRY_WAIT: Duration = Duration::from_millis(100);

static ARMED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn armed() -> std::sync::MutexGuard<'static, Vec<PathBuf>> {
    ARMED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A directory that is removed when this value drops and, until then, when a
/// fatal signal arrives.
#[derive(Debug)]
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Puts `path` under the guard. The bash's `trap` line.
    #[must_use]
    pub fn arm(path: &Path) -> Self {
        armed().push(path.to_path_buf());
        Self { path: path.to_path_buf() }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Disarm first, so a signal that arrives now does not race the removal.
        armed().retain(|armed_path| *armed_path != self.path);
        drop(std::fs::remove_dir_all(&self.path));
    }
}

/// Removes every armed directory now. A removal that fails because a child is
/// still writing is tried again, because the bash's `rm -rf` also runs while
/// its children may be alive.
pub fn run_now() {
    let paths: Vec<PathBuf> = armed().clone();
    for path in paths {
        for attempt in 0..RETRIES {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(_) if attempt + 1 < RETRIES => std::thread::sleep(RETRY_WAIT),
                Err(_) => {}
            }
        }
    }
}

/// The signals this process had ignored when it started, from `SigIgn` in
/// `/proc/self/status`.
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

/// Starts the thread that removes the armed directories on a fatal signal and
/// then dies of the signal. Call it once, at the start of `main`.
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
            // The default action of the signal: the process ends by it, as the
            // bash ends after its trap.
            drop(emulate_default_handler(signal));
            std::process::exit(128 + signal);
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mx-repo-cleanup-{tag}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(dir.join("r")).expect("the directory can be made");
        std::fs::write(dir.join("r/file"), "x").expect("the file can be written");
        dir
    }

    #[test]
    fn a_guard_removes_its_directory_when_it_drops() {
        let dir = tmp("drop");
        drop(Scratch::arm(&dir));
        assert!(!dir.exists(), "{} was not removed", dir.display());
    }

    #[test]
    fn run_now_removes_a_directory_that_is_still_armed() {
        let dir = tmp("now");
        let guard = Scratch::arm(&dir);
        run_now();
        assert!(!dir.exists(), "{} was not removed", dir.display());
        drop(guard);
    }

    #[test]
    fn run_now_leaves_a_directory_alone_once_its_guard_dropped() {
        let dir = tmp("dropped");
        drop(Scratch::arm(&dir));
        std::fs::create_dir_all(&dir).expect("the directory is made again");
        run_now();
        assert!(dir.exists(), "a disarmed directory was removed");
        drop(std::fs::remove_dir_all(&dir));
    }
}
