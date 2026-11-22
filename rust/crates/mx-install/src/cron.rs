//! The text `platform/posix/install-cron.sh` writes: the crontab block, the strip of an
//! earlier block, and the count it reports.

use crate::util::{cat, raw};
use std::ffi::OsStr;

/// `# maintainer-agent (<profile>)`: the line that opens a block. The closing line is
/// this text and ` end`.
#[must_use]
pub fn marker(profile: &OsStr) -> Vec<u8> {
    cat(&[b"# maintainer-agent (", raw(profile), b")"])
}

/// What cron's job environment gets for `PATH`. Cron gives a job almost nothing, and a
/// scheduled agent reports that `cargo` does not exist without this.
#[must_use]
pub fn cron_path(home: &OsStr) -> Vec<u8> {
    cat(&[raw(home), b"/.local/bin:", raw(home), b"/.cargo/bin:/usr/local/bin:/usr/bin:/bin"])
}

/// The block for one profile: every task fires daily from 09:17, one hour apart, and
/// appends what it prints to `<state>/logs/cron.out`. The real cadence is
/// `MIN_HOURS_<task>`, which `run.sh` enforces.
#[must_use]
pub fn generate(
    home: &OsStr,
    share: &OsStr,
    profile: &OsStr,
    tasks: &[&[u8]],
    state_dir: &[u8],
) -> Vec<u8> {
    let marker = marker(profile);
    let mut out = marker.clone();
    out.push(b'\n');
    out.extend(cat(&[b"PATH=", &cron_path(home), b"\n"]));
    let log = cat(&[state_dir, b"/logs/cron.out"]);
    for (hour, task) in (9_u32..).zip(tasks) {
        out.extend(cat(&[
            format!("17 {hour} * * *  ").as_bytes(),
            raw(share),
            b"/run.sh ",
            raw(profile),
            b" ",
            task,
            b" >>\"",
            &log,
            b"\" 2>&1\n",
        ]));
    }
    out.extend(cat(&[&marker, b" end\n"]));
    out
}

/// What the awk program of the bash keeps of the crontab `current`: every line outside
/// the block that opens with `marker` and ends with `marker end`, as
/// `$(printf '%s\n' "$current" | awk ...)` returns it (without the trailing newlines).
///
/// The awk drops the opening line, everything up to the closing line, and the closing
/// line itself; an opening line with no closing line drops the rest of the crontab; a
/// closing line with no opening line stays.
#[must_use]
pub fn strip_block(current: &[u8], marker: &[u8]) -> Vec<u8> {
    let end = cat(&[marker, b" end"]);
    let mut skip = false;
    let mut kept: Vec<&[u8]> = Vec::new();
    for line in current.split(|byte| *byte == b'\n') {
        if line == marker {
            skip = true;
        }
        if !skip {
            kept.push(line);
        }
        if line == end.as_slice() {
            skip = false;
        }
    }
    let mut out = kept.join(&b'\n');
    out.push(b'\n');
    crate::util::strip_trailing_newlines(out)
}

/// `grep -c run.sh`: how many lines hold `run`, any one character and `sh` (the dot is
/// not escaped in the bash, so a profile named `x_run_sh` counts as well).
#[must_use]
pub fn count_run_sh(text: &[u8]) -> usize {
    text.split(|byte| *byte == b'\n').filter(|line| has_run_any_sh(line)).count()
}

/// A character is one UTF-8 character when the line is UTF-8 and one byte when it is not.
fn has_run_any_sh(line: &[u8]) -> bool {
    match std::str::from_utf8(line) {
        Ok(text) => {
            let chars: Vec<char> = text.chars().collect();
            chars
                .windows(6)
                .any(|w| w[0] == 'r' && w[1] == 'u' && w[2] == 'n' && w[4] == 's' && w[5] == 'h')
        }
        Err(_) => line.windows(6).any(|w| &w[..3] == b"run" && &w[4..] == b"sh"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(tasks: &[&[u8]]) -> Vec<u8> {
        generate(
            OsStr::new("/h"),
            OsStr::new("/h/.local/share/maintainer"),
            OsStr::new("sk"),
            tasks,
            b"/h/.local/state/sk-maint",
        )
    }

    #[test]
    fn each_task_fires_daily_one_hour_after_the_previous_from_09_17() {
        let text = String::from_utf8(block(&[b"review", b"issues"])).unwrap();
        assert_eq!(
            text,
            "# maintainer-agent (sk)\nPATH=/h/.local/bin:/h/.cargo/bin:/usr/local/bin:/usr/bin:/bin\n\
             17 9 * * *  /h/.local/share/maintainer/run.sh sk review >>\"/h/.local/state/sk-maint/logs/cron.out\" 2>&1\n\
             17 10 * * *  /h/.local/share/maintainer/run.sh sk issues >>\"/h/.local/state/sk-maint/logs/cron.out\" 2>&1\n\
             # maintainer-agent (sk) end\n"
        );
    }

    #[test]
    fn a_profile_with_no_task_still_gets_its_two_marker_lines() {
        assert_eq!(count_run_sh(&block(&[])), 0);
        assert_eq!(String::from_utf8(block(&[])).unwrap().lines().count(), 3);
    }

    #[test]
    fn strip_drops_the_block_and_keeps_every_other_line() {
        let marker = marker(OsStr::new("sk"));
        let current = b"0 1 * * * /bin/backup\n# maintainer-agent (sk)\nPATH=x\n17 9 * * * y\n# maintainer-agent (sk) end\n5 5 * * * /bin/other";
        assert_eq!(strip_block(current, &marker), b"0 1 * * * /bin/backup\n5 5 * * * /bin/other");
    }

    #[test]
    fn strip_keeps_the_block_of_another_profile() {
        let current = b"# maintainer-agent (other)\nx\n# maintainer-agent (other) end";
        assert_eq!(strip_block(current, &marker(OsStr::new("sk"))), current);
    }

    #[test]
    fn an_opening_line_with_no_closing_line_drops_the_rest_of_the_crontab() {
        let current = b"a\n# maintainer-agent (sk)\nb\nc";
        assert_eq!(strip_block(current, &marker(OsStr::new("sk"))), b"a");
    }

    #[test]
    fn a_closing_line_with_no_opening_line_stays() {
        let current = b"a\n# maintainer-agent (sk) end\nb";
        assert_eq!(strip_block(current, &marker(OsStr::new("sk"))), current);
    }

    #[test]
    fn an_empty_crontab_strips_to_nothing() {
        assert_eq!(strip_block(b"", &marker(OsStr::new("sk"))), b"");
    }

    #[test]
    fn the_count_matches_run_any_character_sh_and_so_counts_a_profile_called_x_run_sh() {
        let text = block(&[b"review", b"issues"]);
        assert_eq!(count_run_sh(&text), 2);
        let odd =
            generate(OsStr::new("/h"), OsStr::new("/s"), OsStr::new("x_run_sh"), &[b"t"], b"/st");
        // The marker line, the task line and the closing line all hold `run_sh`.
        assert_eq!(count_run_sh(&odd), 3);
    }
}
