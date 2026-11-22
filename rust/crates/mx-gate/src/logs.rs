//! What the gate reads out of a container's log: killed, environmental, the first failure.

/// bash `looks_killed`: 137 is `SIGKILL` (the memory cap or the OOM killer), 143 is `SIGTERM` (the
/// `--timeout`), 124 is GNU `timeout`. A segfault (139) is absent on purpose: it can be the code
/// under test. A killed container is not a verdict and not a proof.
#[must_use]
pub fn looks_killed(code: i32) -> bool {
    matches!(code, 137 | 143 | 124)
}

/// One shape of `looks_environmental`, matched on one line, ignoring case (`grep -aiE`).
fn shape_matches(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    let contains = |s: &str| l.contains(s);
    if contains("command not found")
        || l.ends_with(": not found")
        || contains("permission denied")
        || contains("cannot execute")
        || contains("exec format error")
        || contains("could not find system library")
        || contains("failed to run custom build command")
        || contains("error while loading shared libraries")
        || contains("[errno 1] operation not permitted")
        || contains("required tool is missing")
    {
        return true;
    }
    // pkg-config[^ ]* (not found|exited|failed)
    let mut from = 0;
    while let Some(at) = l[from..].find("pkg-config") {
        let rest = &l[from + at + "pkg-config".len()..];
        let word_end = rest.find(' ').unwrap_or(rest.len());
        let after = &rest[word_end..];
        if [" not found", " exited", " failed"].iter().any(|s| after.starts_with(s)) {
            return true;
        }
        from += at + 1;
    }
    // linker .?cc.? not found
    let mut from = 0;
    while let Some(at) = l[from..].find("linker ") {
        let rest = &l[from + at + "linker ".len()..];
        for skip in 0..=rest.chars().next().map_or(0, char::len_utf8).min(4) {
            if !rest.is_char_boundary(skip) {
                continue;
            }
            if let Some(tail) = rest[skip..].strip_prefix("cc") {
                for skip2 in 0..=tail.chars().next().map_or(0, char::len_utf8).min(4) {
                    if tail.is_char_boundary(skip2) && tail[skip2..].starts_with(" not found") {
                        return true;
                    }
                }
            }
        }
        from += at + 1;
    }
    false
}

/// bash `looks_environmental`: the last line of the log that matches one of the shapes of a gate
/// that could not run (a missing binary, an unexecutable path, a toolchain that cannot link, an
/// `EPERM` under dropped capabilities, a repository preflight naming a missing tool), or `None`.
/// The shapes are short and specific on purpose: a classifier that calls everything environmental
/// would excuse a real failure.
#[must_use]
pub fn looks_environmental(log: &str) -> Option<String> {
    log.lines().rfind(|line| shape_matches(line)).map(str::to_string)
}

/// A line of runtime logfmt: `time="..." level=(debug|info|warning|error)`.
fn is_runtime_noise(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("time=\"") else { return false };
    let Some(end) = rest.find('"') else { return false };
    let after = &rest[end + 1..];
    ["debug", "info", "warning", "error"]
        .iter()
        .any(|level| after.starts_with(&format!(" level={level}")))
}

/// bash `first_failure_line`: the line of a failed run that goes into the receipt, at most 200
/// characters. The runtime's own logfmt lines are dropped first. The first line holding
/// `panicked at`, `assertion`, `FAILED`, `FAIL `, `Error` or `error` wins; with none, the last
/// non-empty line does.
#[must_use]
pub fn first_failure_line(log: &str) -> String {
    let clean: Vec<&str> = log.lines().filter(|line| !is_runtime_noise(line)).collect();
    let hit = clean
        .iter()
        .find(|line| {
            ["panicked at", "assertion", "FAILED", "FAIL ", "Error", "error"]
                .iter()
                .any(|p| line.contains(p))
        })
        .or_else(|| clean.iter().rev().find(|line| !line.is_empty()));
    hit.map_or_else(String::new, |line| line.chars().take(200).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn killed_is_137_143_and_124_and_a_segfault_is_not() {
        for code in [137, 143, 124] {
            assert!(looks_killed(code), "{code}");
        }
        for code in [0, 1, 101, 139, 125, 126, 127] {
            assert!(!looks_killed(code), "{code}");
        }
    }

    #[test]
    fn each_environmental_shape_is_found_and_the_last_matching_line_wins() {
        for line in [
            "sh: 1: python3: not found",
            "bash: git: command not found",
            "/tmp/x: Permission denied",
            "cannot execute binary file",
            "exec format error",
            "The system library `glib-2.0` required by crate `glib-sys` was not found.\nerror: could not find system library",
            "error: failed to run custom build command for `glib-sys v0.18.1`",
            "pkg-config exited with status code 1",
            "pkg-config-rs not found",
            "pkg-config failed",
            "linker `cc` not found",
            "linker cc not found",
            "error while loading shared libraries: libz.so",
            "[Errno 1] Operation not permitted: '/x'",
            "ERROR: required tool is missing: cargo",
        ] {
            assert!(looks_environmental(line).is_some(), "{line:?}");
        }
        let log = "a: command not found\nfine\nb: Permission denied\nmore";
        assert_eq!(looks_environmental(log), Some("b: Permission denied".into()));
    }

    #[test]
    fn a_dependency_named_pkg_config_is_not_an_environmental_failure() {
        // The bare token once matched `Compiling pkg-config v0.3.33` and blamed the container.
        assert_eq!(looks_environmental("   Compiling pkg-config v0.3.33"), None);
        assert_eq!(looks_environmental("test result: FAILED. 1 passed; 1 failed"), None);
        assert_eq!(looks_environmental(""), None);
    }

    #[test]
    fn a_trailing_not_found_matches_only_at_the_end_of_the_line() {
        assert!(looks_environmental("x: not found").is_some());
        assert_eq!(looks_environmental("x: not found here"), None);
    }

    #[test]
    fn matching_ignores_case() {
        assert!(looks_environmental("PERMISSION DENIED").is_some());
    }

    #[test]
    fn the_first_failure_line_prefers_a_rust_shaped_line() {
        let log =
            "running 2 tests\nthread 'x' panicked at src/lib.rs:3:5:\nassertion failed\nafter";
        assert_eq!(first_failure_line(log), "thread 'x' panicked at src/lib.rs:3:5:");
    }

    #[test]
    fn runtime_logfmt_is_never_the_observed_failure() {
        let log = "time=\"2026-11-22T10:00:00Z\" level=warning msg=\"Error validating CNI config file\"\nall quiet\nlast line";
        assert_eq!(first_failure_line(log), "last line");
    }

    #[test]
    fn with_no_shaped_line_the_last_non_empty_line_is_used() {
        assert_eq!(first_failure_line("ok\nmismatch of the figure\n\n"), "mismatch of the figure");
        assert_eq!(first_failure_line(""), "");
    }

    #[test]
    fn the_line_is_cut_at_200_characters() {
        let long = format!("Error: {}", "x".repeat(400));
        assert_eq!(first_failure_line(&long).chars().count(), 200);
    }
}
