//! What the signal thread does to a live child (decision `P4B-12`). Own test binary: the stop is
//! process-wide, so it must not share a process with other tests that start children.

use mx_checks::cleanup::stop_children;
use mx_ports::{Proc, ProcExit, ProcSpec, RealProc, live_children};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

fn wait_for(limit: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if ready() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    ready()
}

#[test]
fn a_signal_the_bash_children_do_not_get_leaves_the_child_and_sigterm_ends_it() {
    let proc = RealProc::new("/usr/bin:/bin", Vec::new());
    let spec = ProcSpec::command("sleep", ["30"], Duration::from_secs(25));
    let (sender, receiver) = mpsc::channel();
    drop(thread::spawn(move || drop(sender.send(proc.run(&spec)))));
    assert!(
        wait_for(Duration::from_secs(5), || live_children() == 1),
        "the child never registered"
    );
    thread::sleep(Duration::from_millis(200));

    stop_children(10); // SIGUSR1: not forwarded
    assert_eq!(live_children(), 1, "SIGUSR1 reached the child");
    assert!(receiver.try_recv().is_err(), "the call returned after a signal that is not forwarded");

    let started = Instant::now();
    stop_children(15);
    assert_eq!(live_children(), 0, "stop_children returned with the child still live");
    assert!(started.elapsed() < Duration::from_secs(4), "it waited out the grace");
    let late = RealProc::new("/usr/bin:/bin", Vec::new()).run(&ProcSpec::command(
        "true",
        Vec::<String>::new(),
        Duration::from_secs(5),
    ));
    assert!(late.is_err(), "a child started after the stop");
    let out = receiver
        .recv_timeout(Duration::from_secs(40))
        .expect("the call never answered")
        .expect("failed");
    assert_eq!(out.exit, ProcExit::Code(143), "the child did not die of SIGTERM");
}
