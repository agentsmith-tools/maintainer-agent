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
//! The work runs once, and every caller returns only after it has finished
//! (decision `P2A-trap`). The slot holds a phase, `NotRun`, `Running` or `Done`,
//! and a `Condvar` that every change of phase signals. The first caller to find
//! `NotRun` takes the work and runs it; a caller that finds `Running` waits for
//! `Done`; a caller that finds `Done` returns. So a second thread never returns,
//! and the process never ends, while the first thread is still scrubbing. Two
//! paths led there when the slot was only taken: a signal that arrives as the pass
//! finishes (the signal thread starts the scrub, the main thread returns from
//! `main`, and the process ends mid-scrub) and a [`checkpoint`] that gave up while
//! the signal thread was scrubbing (it found the slot empty and called `exit`).
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
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

/// The signals the bash's EXIT trap runs for when they kill the script and that
/// a caller can send.
const FATAL: [i32; 6] = [SIGHUP, SIGINT, SIGTERM, SIGUSR1, SIGUSR2, SIGALRM];

/// How long [`checkpoint`] waits for the signal thread to end the process before
/// it gives up and ends it itself.
const CHECKPOINT_PATIENCE: Duration = Duration::from_mins(1);

type Work = Box<dyn FnOnce() + Send>;

/// Where the armed work stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Nothing has started it. There may be nothing armed either.
    NotRun,
    /// A thread is running it now.
    Running,
    /// It has finished.
    Done,
}

struct Slot {
    work: Option<Work>,
    phase: Phase,
}

static SLOT: Mutex<Slot> = Mutex::new(Slot { work: None, phase: Phase::NotRun });
/// Signalled when the phase changes.
static CHANGED: Condvar = Condvar::new();
static SIGNALLED: OnceLock<Arc<AtomicBool>> = OnceLock::new();

fn slot() -> MutexGuard<'static, Slot> {
    SLOT.lock().unwrap_or_else(PoisonError::into_inner)
}

fn signalled() -> &'static Arc<AtomicBool> {
    SIGNALLED.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

/// `trap work EXIT`: replaces whatever was armed.
pub fn arm(work: impl FnOnce() + Send + 'static) {
    let mut state = slot();
    state.work = Some(Box::new(work));
    if state.phase == Phase::Done {
        state.phase = Phase::NotRun;
    }
}

/// `trap - EXIT`.
pub fn disarm() {
    slot().work = None;
}

/// Whether work is armed and has not started.
#[must_use]
pub fn is_armed() -> bool {
    slot().work.is_some()
}

/// Marks the work finished, also when it panics, and wakes everyone who waits.
struct Finish;

impl Drop for Finish {
    fn drop(&mut self) {
        let mut state = slot();
        // Work armed while this ran is still to run.
        state.phase = if state.work.is_some() { Phase::NotRun } else { Phase::Done };
        drop(state);
        CHANGED.notify_all();
    }
}

/// Runs the armed work once and returns after it has finished, whichever thread
/// started it: a caller that finds the work running waits for it.
pub fn run_now() {
    let mut state = slot();
    loop {
        match state.phase {
            Phase::Done => return,
            Phase::Running => {
                state = CHANGED.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
            Phase::NotRun => {
                let Some(work) = state.work.take() else {
                    return;
                };
                state.phase = Phase::Running;
                drop(state);
                let _finish = Finish;
                work();
                return;
            }
        }
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
    checkpoint_with(CHECKPOINT_PATIENCE, || std::process::exit(1));
}

/// [`checkpoint`] with its patience and its way out as parameters.
///
/// When the patience runs out the signal thread has not ended the process. Then
/// [`run_now`] runs the work if nobody has (`NotRun`) and waits for the thread that
/// is running it (`Running`), and only after that does `give_up` leave. The way out
/// never comes while the work is running.
fn checkpoint_with(patience: Duration, give_up: impl FnOnce()) {
    if !has_signalled() {
        return;
    }
    let mut waited = Duration::ZERO;
    let step = Duration::from_millis(20).min(patience);
    while waited < patience {
        std::thread::sleep(step);
        waited += step;
    }
    run_now();
    give_up();
}

/// Runs the armed work when it drops, including during a panic.
#[derive(Debug)]
pub struct ExitGuard;

impl Drop for ExitGuard {
    fn drop(&mut self) {
        run_now();
        // A signal that arrived while this ran ends the process by the signal.
        checkpoint();
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
            // Returns after the work has finished, also when the main thread started it.
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
    use std::sync::mpsc;

    /// The slot is global, so the tests that arm it take turns.
    static TURN: Mutex<()> = Mutex::new(());

    fn turn() -> MutexGuard<'static, ()> {
        TURN.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Work that says it started, then blocks until released, then sets `finished`.
    struct Blocking {
        started: mpsc::Receiver<()>,
        release: mpsc::Sender<()>,
        finished: Arc<AtomicBool>,
    }

    fn arm_blocking() -> Blocking {
        let finished = Arc::new(AtomicBool::new(false));
        let (started_tx, started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<()>();
        let flag = Arc::clone(&finished);
        arm(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            flag.store(true, Ordering::SeqCst);
        });
        Blocking { started, release, finished }
    }

    /// Sets the signalled flag for a test and clears it when the test ends.
    struct Signalled;

    impl Signalled {
        fn raise() -> Self {
            signalled().store(true, Ordering::SeqCst);
            Self
        }
    }

    impl Drop for Signalled {
        fn drop(&mut self) {
            signalled().store(false, Ordering::SeqCst);
        }
    }

    /// Runs `second` on a thread while the work blocks, waits a bounded time for it
    /// to return, releases the work and reports whether the work had finished when
    /// `second` returned. A `second` that does not wait returns first and reports false.
    fn finished_when_second_returns(second: fn()) -> bool {
        let blocking = arm_blocking();
        let first = std::thread::spawn(run_now);
        blocking.started.recv().unwrap();
        let saw_finished = Arc::new(AtomicBool::new(false));
        let returned = Arc::new(AtomicBool::new(false));
        let (saw, done, flag) =
            (Arc::clone(&saw_finished), Arc::clone(&returned), Arc::clone(&blocking.finished));
        let other = std::thread::spawn(move || {
            second();
            saw.store(flag.load(Ordering::SeqCst), Ordering::SeqCst);
            done.store(true, Ordering::SeqCst);
        });
        let give_up = std::time::Instant::now() + Duration::from_millis(500);
        while !returned.load(Ordering::SeqCst) && std::time::Instant::now() < give_up {
            std::thread::sleep(Duration::from_millis(5));
        }
        blocking.release.send(()).unwrap();
        first.join().unwrap();
        other.join().unwrap();
        saw_finished.load(Ordering::SeqCst)
    }

    #[test]
    fn a_second_caller_of_run_now_returns_only_after_the_work_the_first_started_has_finished() {
        let _turn = turn();
        assert!(
            finished_when_second_returns(run_now),
            "run_now returned while the work was running"
        );
    }

    #[test]
    fn a_dropped_exit_guard_returns_only_after_the_work_another_thread_started_has_finished() {
        let _turn = turn();
        assert!(
            finished_when_second_returns(|| drop(ExitGuard)),
            "the guard returned while the work was running"
        );
    }

    #[test]
    fn a_checkpoint_after_a_signal_does_not_leave_while_the_work_is_running() {
        let _turn = turn();
        let blocking = arm_blocking();
        let first = std::thread::spawn(run_now);
        blocking.started.recv().unwrap();
        let _signalled = Signalled::raise();
        let left = Arc::new(AtomicBool::new(false));
        let finished_at_exit = Arc::new(AtomicBool::new(false));
        let (left_flag, seen, work_done) =
            (Arc::clone(&left), Arc::clone(&finished_at_exit), Arc::clone(&blocking.finished));
        let waiter = std::thread::spawn(move || {
            checkpoint_with(Duration::from_millis(40), move || {
                seen.store(work_done.load(Ordering::SeqCst), Ordering::SeqCst);
                left_flag.store(true, Ordering::SeqCst);
            });
        });
        let give_up = std::time::Instant::now() + Duration::from_millis(500);
        while !left.load(Ordering::SeqCst) && std::time::Instant::now() < give_up {
            std::thread::sleep(Duration::from_millis(5));
        }
        let left_while_running = left.load(Ordering::SeqCst);
        blocking.release.send(()).unwrap();
        first.join().unwrap();
        waiter.join().unwrap();
        assert!(!left_while_running, "the checkpoint left while the work was running");
        assert!(left.load(Ordering::SeqCst), "the checkpoint never left after the work finished");
        assert!(finished_at_exit.load(Ordering::SeqCst), "it left before the work finished");
    }

    #[test]
    fn a_checkpoint_that_gives_up_with_the_work_not_started_runs_it_first() {
        let _turn = turn();
        let (seen, count) = counter();
        arm(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        let _signalled = Signalled::raise();
        let left = Arc::new(AtomicUsize::new(0));
        let at_exit = Arc::clone(&left);
        let ran = Arc::clone(&count);
        checkpoint_with(Duration::from_millis(20), move || {
            at_exit.store(ran.load(Ordering::SeqCst) + 10, Ordering::SeqCst);
        });
        assert_eq!(left.load(Ordering::SeqCst), 11, "the way out came before the work ran");
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn work_armed_while_the_earlier_work_runs_still_runs() {
        let _turn = turn();
        let blocking = arm_blocking();
        let first = std::thread::spawn(run_now);
        blocking.started.recv().unwrap();
        let (seen, count) = counter();
        arm(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        blocking.release.send(()).unwrap();
        first.join().unwrap();
        run_now();
        assert_eq!(count.load(Ordering::SeqCst), 1);
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
        // A test that raises the signalled flag would make this checkpoint wait a minute.
        let _turn = turn();
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
