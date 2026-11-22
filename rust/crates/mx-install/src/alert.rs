//! The pure parts of `platform/linux/alert.sh` and `platform/linux/run-instance.sh`.

use crate::util::{before_last_hyphen, cat};

/// What every alert says.
pub const MESSAGE: &str =
    "the run unit failed before it could report; systemd killed or refused it";

/// `${instance%-*}`: the profile an instance name belongs to. systemd passes
/// `<profile>-<task>` and the profile is everything before the last hyphen.
#[must_use]
pub fn profile_of(instance: &[u8]) -> &[u8] {
    before_last_hyphen(instance)
}

/// The line appended to `alerts.log`.
#[must_use]
pub fn log_line(date: &[u8], instance: &[u8]) -> Vec<u8> {
    cat(&[date, b"  ALERT ", instance, b": ", MESSAGE.as_bytes(), b"\n"])
}

/// The title and body of the desktop notification.
#[must_use]
pub fn notification(instance: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let title = cat(&["maintainer \u{b7} ".as_bytes(), instance, " \u{b7} FAILED".as_bytes()]);
    let body = cat(&[
        MESSAGE.as_bytes(),
        b"\nfix    maintainer-doctor\nthen   journalctl --user -u maintainer@",
        instance,
        b".service -n 40",
    ]);
    (title, body)
}

/// `run-instance.sh` splits `<profile>-<task>` at the last hyphen: a hyphenated
/// repository makes a hyphenated profile, and a task name cannot hold a hyphen because
/// `run.sh` builds shell variable names from it. A name with no hyphen is both.
#[must_use]
pub fn split_instance(instance: &[u8]) -> (&[u8], &[u8]) {
    let profile = before_last_hyphen(instance);
    let task = match instance.iter().rposition(|byte| *byte == b'-') {
        Some(at) => &instance[at + 1..],
        None => instance,
    };
    (profile, task)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_is_everything_before_the_last_hyphen() {
        assert_eq!(profile_of(b"my-repo-review"), b"my-repo");
        assert_eq!(profile_of(b"solo"), b"solo");
    }

    #[test]
    fn plan_decision_f7_the_unit_name_systemd_passes_names_a_profile_that_has_no_directory() {
        // F7: the alert unit passes %n, so the "profile" is `maintainer@sysknife`.
        assert_eq!(profile_of(b"maintainer@sysknife-ci.service"), b"maintainer@sysknife");
    }

    #[test]
    fn the_instance_splits_at_the_last_hyphen_and_a_bare_name_is_profile_and_task() {
        assert_eq!(split_instance(b"my-repo-review"), (&b"my-repo"[..], &b"review"[..]));
        assert_eq!(split_instance(b"solo"), (&b"solo"[..], &b"solo"[..]));
    }

    #[test]
    fn the_notification_names_the_failing_unit_and_the_journal_command() {
        let (title, body) = notification(b"sk-ci");
        assert_eq!(title, "maintainer \u{b7} sk-ci \u{b7} FAILED".as_bytes());
        assert!(body.ends_with(b"journalctl --user -u maintainer@sk-ci.service -n 40"));
    }
}
