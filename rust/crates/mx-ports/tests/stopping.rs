//! A program that is ending on a signal starts no new child. Own test binary: the refusal is
//! process-wide, so it must not share a process with the tests that start children.

use mx_ports::{Proc, ProcError, ProcSpec, RealProc, refuse_new_children};
use std::time::Duration;

#[test]
fn after_refuse_new_children_a_spawn_is_refused_with_a_named_error() {
    let proc = RealProc::new("/usr/bin:/bin", Vec::new());
    let spec = ProcSpec::command("true", Vec::<String>::new(), Duration::from_secs(5));
    assert!(proc.run(&spec).is_ok(), "the control call must run before the refusal");
    refuse_new_children(Duration::from_millis(400));
    let started = std::time::Instant::now();
    let error = proc.run(&spec).expect_err("a call after the refusal must not start a child");
    assert!(
        matches!(&error, ProcError::Spawn { reason, .. } if reason.contains("ending on a signal")),
        "got {error:?}"
    );
    assert!(started.elapsed() >= Duration::from_millis(400), "the refused call did not hold");
}
