//! The exit trap: work that must run on every way out.
//!
//! The bash sets `trap scrub_pinned_token EXIT` and relies on bash running it
//! on `exit`, on a failed `set -u` expansion, and when SIGHUP, SIGINT or SIGTERM
//! kills the script. Setting a new `trap ... EXIT` replaces the old one. This
//! module keeps one slot, [`arm`], with the same replace rule, and runs it from
//! three places:
//!
//! - [`ExitGuard`], whose `Drop` runs it when the run returns and when a panic
//!   unwinds through it;
//! - [`run_now`], for a caller that is about to leave the process by another way;
//! - the signal thread of [`install`], which runs it when a fatal signal
//!   arrives, then dies of that signal so the caller sees `128 + n`.
//!
//! The slot is taken, not copied: whichever caller gets there first runs the work
//! and the others find it empty, so the scrub never runs twice.
//!
//! # The race a signal opens
//!
//! The thread that handles a signal runs beside the thread that is running the
//! pass. The bash defers the trap until the foreground command ends; here the
//! agent is a child that ends in its own time, and the thread that waits for it
//! would go on to `finish` and the digest after the signal. [`checkpoint`] is
//! called after every external call: once a signal has arrived (the flag is set
//! by the signal handler itself, before any thread wakes) it never returns, and
//! the signal thread ends the process.
//!
//! The thread also kills every process group a pipeline started before it
//! scrubs, so the agent cannot write the token again after the scrub. A shell's
//! terminal Ctrl-C reaches the agent because the shell leaves it in its own
//! process group; each stage here leads a group of its own.
//!
//! A signal that the caller left ignored when the program started stays ignored,
//! because bash does not trap what it inherited as ignored.

use signal_hook::consts::signal::{SIGALRM, SIGHUP, SIGINT, SIGTERM, SIGUSR1, SIGUSR2};
use signal_hook::flag;
use signal_hook::iterator::Signals;
use signal_hook::low_level::emulate_default_handler;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

/// The signals the bash's EXIT trap runs for when they kill the script and that
/// a caller can send.
const FATAL: [i32; 6] = [SIGHUP, SIGINT, SIGTERM, SIGUSR1, SIGUSR2, SIGALRM];

/// How long [`checkpoint`] waits for the signal thread to end the process before
/// it gives up and ends it itself.
const CHECKPOINT_PATIENCE: Duration = Duration::from_mins(1);

type Work = Box<dyn FnOnce() + Send>;

static SLOT: Mutex<Option<Work>> = Mutex::new(None);
static SIGNALLED: OnceLock<Arc<AtomicBool>> = OnceLock::new();

fn slot() -> MutexGuard<'static, Option<Work>> {
    SLOT.lock().unwrap_or_else(PoisonError::into_inner)
}

fn signalled() -> &'static Arc<AtomicBool> {
    SIGNALLED.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

/// `trap work EXIT`: replaces whatever was armed.
pub fn arm(work: impl FnOnce() + Send + 'static) {
    *slot() = Some(Box::new(work));
}

/// `trap - EXIT`.
pub fn disarm() {
    *slot() = None;
}

/// Whether work is armed.
#[must_use]
pub fn is_armed() -> bool {
    slot().is_some()
}

/// Runs the armed work, once. Later calls find the slot empty.
pub fn run_now() {
    let work = slot().take();
    if let Some(work) = work {
        work();
    }
}

/// Whether a fatal signal has arrived.
#[must_use]
pub fn has_signalled() -> bool {
    signalled().load(Ordering::Acquire)
}

/// Returns at once while no fatal signal has arrived. After one it waits for the
/// signal thread to end the process.
pub fn checkpoint() {
    if !has_signalled() {
        return;
    }
    let mut waited = Duration::ZERO;
    let step = Duration::from_millis(20);
    while waited < CHECKPOINT_PATIENCE {
        std::thread::sleep(step);
        waited += step;
    }
    run_now();
    std::process::exit(1);
}

/// Runs the armed work when it drops, including during a panic.
#[derive(Debug)]
pub struct ExitGuard;

impl Drop for ExitGuard {
    fn drop(&mut self) {
        run_now();
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

/// Starts the thread that handles fatal signals. Call it once, at the start of
/// `main`.
pub fn install() {
    let ignored = ignored_at_start();
    let wanted: Vec<i32> = FATAL
        .iter()
        .copied()
        .filter(|signal| {
            u32::try_from(*signal - 1).ok().is_none_or(|bit| ignored & (1_u64 << bit) == 0)
        })
        .collect();
    for signal in &wanted {
        // The flag is set inside the handler, before any thread wakes.
        drop(flag::register(*signal, Arc::clone(signalled())));
    }
    let Ok(mut signals) = Signals::new(wanted) else {
        return;
    };
    drop(std::thread::spawn(move || {
        if let Some(signal) = signals.forever().next() {
            mx_ports::kill_live_pipelines();
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
    use std::sync::atomic::AtomicUsize;

    /// The slot is global, so the tests that arm it take turns.
    static TURN: Mutex<()> = Mutex::new(());

    fn turn() -> MutexGuard<'static, ()> {
        TURN.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn counter() -> (Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        (Arc::clone(&count), count)
    }

    #[test]
    fn the_armed_work_runs_once_however_many_times_it_is_asked() {
        let _turn = turn();
        let (seen, count) = counter();
        arm(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        run_now();
        run_now();
        drop(ExitGuard);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(!is_armed());
    }

    #[test]
    fn arming_again_replaces_the_earlier_work_as_a_second_trap_does() {
        let _turn = turn();
        let (first, first_count) = counter();
        let (second, second_count) = counter();
        arm(move || {
            first.fetch_add(1, Ordering::SeqCst);
        });
        arm(move || {
            second.fetch_add(1, Ordering::SeqCst);
        });
        run_now();
        assert_eq!(first_count.load(Ordering::SeqCst), 0);
        assert_eq!(second_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_guard_runs_the_work_when_a_panic_unwinds_through_it() {
        let _turn = turn();
        let (seen, count) = counter();
        arm(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        let outcome = std::panic::catch_unwind(|| {
            let _guard = ExitGuard;
            panic!("the pass blew up");
        });
        assert!(outcome.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn disarm_removes_the_work() {
        let _turn = turn();
        let (seen, count) = counter();
        arm(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        disarm();
        run_now();
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_checkpoint_returns_at_once_while_no_signal_has_arrived() {
        let started = std::time::Instant::now();
        checkpoint();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!has_signalled());
    }

    #[test]
    fn the_ignored_signal_mask_is_read_from_the_process_status_file() {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let line = status.lines().find(|line| line.starts_with("SigIgn:")).unwrap();
        let want = u64::from_str_radix(line["SigIgn:".len()..].trim(), 16).unwrap();
        assert_eq!(ignored_at_start(), want);
    }
}
