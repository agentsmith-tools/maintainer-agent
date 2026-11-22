//! A forwarded signal reaches the process group of a live child. Own test binary: the forward goes
//! to every live child of the process, so it must not share a process with other tests that run one.

use mx_ports::{Proc, ProcExit, ProcSpec, RealProc, forward_signal_to_children, live_children};
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

fn is_alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().is_some_and(|stat| {
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.trim_start().chars().next())
            .is_some_and(|state| state != 'Z' && state != 'X')
    })
}

#[test]
fn a_forwarded_signal_ends_the_child_and_its_group_and_the_call_returns_the_signal_exit() {
    let proc = RealProc::new("/usr/bin:/bin", Vec::new());
    let spec = ProcSpec::command("sh", ["-c", "sleep 30 & echo $!; wait"], Duration::from_secs(25));
    let (sender, receiver) = mpsc::channel();
    drop(thread::spawn(move || drop(sender.send(proc.run(&spec)))));
    assert!(
        wait_for(Duration::from_secs(5), || live_children() == 1),
        "the child never registered"
    );
    thread::sleep(Duration::from_millis(300));
    assert_eq!(forward_signal_to_children(15), 1, "the forward reached no group");
    let out = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("the call did not return after the forward")
        .expect("the call failed");
    assert_eq!(out.exit, ProcExit::Code(143), "the child did not die of the signal");
    let grandchild: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().expect("a pid");
    assert!(wait_for(Duration::from_secs(3), || !is_alive(grandchild)), "the grandchild survived");
    assert!(
        wait_for(Duration::from_secs(2), || live_children() == 0),
        "the child stayed registered"
    );
}
