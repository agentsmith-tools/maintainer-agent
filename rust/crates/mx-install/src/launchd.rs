//! The property list `platform/macos/install-launchd.sh` writes for one task.

use crate::util::{cat, raw};
use std::ffi::OsStr;

/// Everything one plist names.
pub struct Plist<'a> {
    pub label: &'a [u8],
    pub share: &'a OsStr,
    pub profile: &'a OsStr,
    pub task: &'a [u8],
    pub repo_path: &'a [u8],
    pub home: &'a OsStr,
    pub hour: u32,
    pub state_dir: &'a [u8],
}

impl Plist<'_> {
    /// The file, byte for byte. Nothing is XML-escaped, as in the bash: a profile or a
    /// path with a `&` or a `<` makes a plist launchd cannot read.
    #[must_use]
    pub fn render(&self) -> Vec<u8> {
        cat(&[
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            b"<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
            b"<plist version=\"1.0\">\n<dict>\n",
            b"  <key>Label</key><string>", self.label, b"</string>\n",
            b"  <key>ProgramArguments</key>\n  <array>\n    <string>/bin/bash</string>\n",
            b"    <string>", raw(self.share), b"/run.sh</string>\n",
            b"    <string>", raw(self.profile), b"</string>\n",
            b"    <string>", self.task, b"</string>\n  </array>\n",
            b"  <key>WorkingDirectory</key><string>", self.repo_path, b"</string>\n",
            b"  <key>EnvironmentVariables</key>\n  <dict>\n",
            b"    <key>PATH</key><string>", raw(self.home),
            b"/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>\n  </dict>\n",
            b"  <key>StartCalendarInterval</key>\n",
            format!("  <dict><key>Hour</key><integer>{}</integer><key>Minute</key><integer>17</integer></dict>\n", self.hour).as_bytes(),
            b"  <key>RunAtLoad</key><false/>\n",
            b"  <key>StandardOutPath</key><string>", self.state_dir, b"/logs/launchd-", self.task, b".out</string>\n",
            b"  <key>StandardErrorPath</key><string>", self.state_dir, b"/logs/launchd-", self.task, b".err</string>\n",
            b"</dict>\n</plist>\n",
        ])
    }
}

/// The line printed after a task is loaded.
#[must_use]
pub fn loaded_line(label: &[u8], hour: u32, min_hours: &[u8]) -> Vec<u8> {
    cat(&[
        b"  loaded ",
        label,
        format!(" (fires daily at {hour:02}:17; runs when ").as_bytes(),
        min_hours,
        b"h have passed)\n",
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plist_names_the_run_script_the_hour_and_the_two_logs() {
        let text = Plist {
            label: b"dev.maintainer.sk.review",
            share: OsStr::new("/h/.local/share/maintainer"),
            profile: OsStr::new("sk"),
            task: b"review",
            repo_path: b"/h/code",
            home: OsStr::new("/h"),
            hour: 9,
            state_dir: b"/h/.local/state/sk-maint",
        }
        .render();
        let text = String::from_utf8(text).unwrap();
        assert!(text.contains("<string>/h/.local/share/maintainer/run.sh</string>"));
        assert!(
            text.contains(
                "<key>Hour</key><integer>9</integer><key>Minute</key><integer>17</integer>"
            )
        );
        assert!(text.contains("<key>StandardErrorPath</key><string>/h/.local/state/sk-maint/logs/launchd-review.err</string>"));
        assert!(text.ends_with("</dict>\n</plist>\n"));
    }

    #[test]
    fn the_loaded_line_pads_the_hour_to_two_digits() {
        assert_eq!(
            loaded_line(b"dev.maintainer.sk.ci", 9, b"60"),
            b"  loaded dev.maintainer.sk.ci (fires daily at 09:17; runs when 60h have passed)\n"
        );
    }
}
