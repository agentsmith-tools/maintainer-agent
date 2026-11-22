//! The pure half of `new-profile.sh`: the arguments it accepts, the substitutions it
//! makes, the timer units it writes and the text it prints.

use crate::sedsub::{self, Replacement, SedFailure};
use crate::util::{cat, raw, words};
use std::ffi::{OsStr, OsString};

/// Lines 2 to 9 of the script, which `usage` prints on standard error.
pub const USAGE: [&str; 8] = [
    "# Scaffold a profile for another repository.",
    "#",
    "#   ./new-profile.sh <profile> <owner/repo> <path-to-checkout> <gh-account> \"<your name>\"",
    "#",
    "# Writes profiles/<profile>/ and the systemd units for it, then tells you the",
    "# three things to do before the first run. Nothing is enabled and nothing posts:",
    "# a new profile starts at POST=off, so the first week produces reports and",
    "# drafts and reaches nobody.",
];

/// The five placeholders of a template, in the order the bash substitutes them.
pub const PLACEHOLDERS: [&str; 5] =
    ["__PROFILE__", "__SLUG__", "__REPO_PATH__", "__GH_ACCOUNT__", "__MAINTAINER_NAME__"];

/// Why `new-profile` refuses before it writes anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Fewer than four arguments. Exit 64.
    Usage,
    /// A name that is empty, starts with an underscore or holds a slash. Exit 64.
    BadProfile(OsString),
    /// A slug with no slash. Exit 64.
    BadSlug(OsString),
    /// A `|` or `&` in an argument. Exit 64.
    BadCharacter,
    /// `profiles/<name>` is there. Exit 1.
    Exists(OsString),
}

impl Refusal {
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage | Self::BadProfile(_) | Self::BadSlug(_) | Self::BadCharacter => 64,
            Self::Exists(_) => 1,
        }
    }

    /// What goes to standard error, newline included.
    #[must_use]
    pub fn stderr(&self) -> Vec<u8> {
        let line = |parts: &[&[u8]]| {
            let mut text = cat(parts);
            text.push(b'\n');
            text
        };
        match self {
            Self::Usage => USAGE.iter().flat_map(|l| line(&[l.as_bytes()])).collect(),
            Self::BadProfile(name) => {
                line(&[b"new-profile: '", raw(name), b"' is not a usable profile name"])
            }
            Self::BadSlug(slug) => line(&[b"new-profile: '", raw(slug), b"' is not owner/repo"]),
            Self::BadCharacter => {
                line(&[b"new-profile: '|' and '&' are not allowed in an argument"])
            }
            Self::Exists(name) => line(&[b"new-profile: profiles/", raw(name), b" already exists"]),
        }
    }
}

/// A checkout under `$HOME` is written as `$HOME/...`, never expanded: profile.env is
/// meant to be committed, and an expanded path publishes the adopter's username.
#[must_use]
pub fn path_written(path: &OsStr, home: &OsStr) -> Vec<u8> {
    let path = raw(path);
    let mut prefix = raw(home).to_vec();
    prefix.push(b'/');
    match path.strip_prefix(prefix.as_slice()) {
        Some(rest) => cat(&[b"$HOME/", rest]),
        None => path.to_vec(),
    }
}

/// The name and slug checks, in the order the bash makes them.
pub fn check_names(profile: &OsStr, slug: &OsStr) -> Result<(), Refusal> {
    let name = raw(profile);
    if name.is_empty() || name.starts_with(b"_") || name.contains(&b'/') {
        return Err(Refusal::BadProfile(profile.to_os_string()));
    }
    if !raw(slug).contains(&b'/') {
        return Err(Refusal::BadSlug(slug.to_os_string()));
    }
    Ok(())
}

/// A `|` is sed's delimiter and a `&` is the matched text, so the bash refuses them.
#[must_use]
pub fn has_bad_character(value: &OsStr) -> bool {
    raw(value).iter().any(|byte| matches!(byte, b'|' | b'&'))
}

/// The five `-e` expressions of the bash: `s|__PROFILE__|$profile|g` and four more.
#[derive(Debug, Clone)]
pub struct Substitutions {
    parsed: Vec<(&'static str, Replacement)>,
}

impl Substitutions {
    /// Reads the five values as sed reads them. The first value sed refuses is the
    /// error, as sed parses the expressions in order before it opens a file.
    pub fn parse(values: [&[u8]; 5]) -> Result<Self, SedFailure> {
        let mut parsed = Vec::new();
        for (at, (placeholder, value)) in PLACEHOLDERS.iter().zip(values).enumerate() {
            let prefix = format!("s|{placeholder}|").len();
            parsed.push((*placeholder, sedsub::parse_replacement(value, at + 1, prefix, b"|g")?));
        }
        Ok(Self { parsed })
    }

    /// One file's text after the five substitutions, each over the output of the last.
    #[must_use]
    pub fn apply(&self, text: &[u8]) -> Vec<u8> {
        let mut out = text.to_vec();
        for (placeholder, replacement) in &self.parsed {
            out = sedsub::replace_all(&out, placeholder.as_bytes(), replacement);
        }
        out
    }
}

/// `sed -n 's/^TASKS="\(.*\)"/\1/p'`, then the words of what it printed. The pattern is
/// not anchored at its end, so text after the last quote of a matching line is kept:
/// `TASKS="a b" # c` yields the words `a`, `b`, `#` and `c`.
#[must_use]
pub fn tasks_of(profile_env: &[u8]) -> Vec<Vec<u8>> {
    let mut printed: Vec<u8> = Vec::new();
    for line in profile_env.split(|byte| *byte == b'\n') {
        let Some(rest) = line.strip_prefix(b"TASKS=\"") else { continue };
        let Some(close) = rest.iter().rposition(|byte| *byte == b'"') else { continue };
        printed.extend_from_slice(&rest[..close]);
        printed.extend_from_slice(&rest[close + 1..]);
        printed.push(b'\n');
    }
    words(&printed).into_iter().map(<[u8]>::to_vec).collect()
}

/// The timer unit for one task: daily, at `hour`:17. The cadence lives in
/// `MIN_HOURS_<task>`, which every platform reads.
#[must_use]
pub fn timer_unit(profile: &OsStr, task: &[u8], hour: u32) -> Vec<u8> {
    let (p, t) = (raw(profile), task);
    cat(&[
        b"[Unit]\nDescription=maintainer: ",
        p,
        b" ",
        t,
        b" (timer)\n\n[Timer]\n",
        b"# Fires daily. The real cadence is MIN_HOURS_",
        t,
        b" in profiles/",
        p,
        b"/profile.env,\n# which run.sh enforces on every platform.\n",
        format!("OnCalendar=*-*-* {hour:02}:17:00\n").as_bytes(),
        b"Persistent=true\nRandomizedDelaySec=300\nUnit=maintainer@",
        p,
        b"-",
        t,
        b".service\n\n[Install]\nWantedBy=timers.target\n",
    ])
}

/// `__[A-Z_]*__` found in one line: two underscores, any letters and underscores, two
/// underscores. A match lies inside one run of capital letters and underscores and needs
/// two pairs of underscores that do not overlap.
#[must_use]
pub fn has_placeholder(line: &[u8]) -> bool {
    let is_name = |byte: &u8| byte.is_ascii_uppercase() || *byte == b'_';
    line.split(|byte| !is_name(byte)).any(|run| {
        let Some(first) = run.windows(2).position(|pair| pair == b"__") else { return false };
        run[first + 2..].windows(2).any(|pair| pair == b"__")
    })
}

const NEXT: &str = "
  profiles/$profile/ is ready. Three things before the first run:

  1. Read profiles/$profile/profile.env end to end. It decides what an
     unattended agent does in your name.
  2. Rewrite profiles/$profile/prompts/*.md for this repository. The generic
     ones are a starting point, not a description of your project. Read
     profiles/sysknife/prompts/ for a worked set.
  3. Install, then read the prompt before anything runs:

       ./install.sh
       export MAINTAINER_PROFILE=$profile
       maintainer-doctor
       maintainer run --show-prompt review | less

  Then one rehearsal run by hand. POST=off, so it posts nothing:

       maintainer run review
       maintainer status

  Every command above reads MAINTAINER_PROFILE, so export it once per shell or
  the tools answer for whichever profile is first.

  Turn posting on in profile.env only after you have read a week of reports,
  then enable the timers with ./install.sh --timers.
";

/// The closing text, which names the profile.
#[must_use]
pub fn next_steps(profile: &OsStr) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = NEXT.as_bytes();
    while let Some(at) = rest.windows(8).position(|w| w == b"$profile") {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(raw(profile));
        rest = &rest[at + 8..];
    }
    out.extend_from_slice(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checkout_under_home_is_written_with_a_literal_home_and_other_paths_stay() {
        assert_eq!(path_written(OsStr::new("/h/code/w"), OsStr::new("/h")), b"$HOME/code/w");
        assert_eq!(path_written(OsStr::new("/srv/w"), OsStr::new("/h")), b"/srv/w");
        // Pins current behaviour: HOME itself is not a path below HOME.
        assert_eq!(path_written(OsStr::new("/h"), OsStr::new("/h")), b"/h");
        assert_eq!(path_written(OsStr::new("/h/"), OsStr::new("/h")), b"$HOME/");
    }

    #[test]
    fn an_empty_home_makes_every_absolute_path_a_path_below_home() {
        // Pins current behaviour: with HOME="" the pattern "$HOME"/* matches any /path.
        assert_eq!(path_written(OsStr::new("/srv/w"), OsStr::new("")), b"$HOME/srv/w");
    }

    #[test]
    fn profile_names_that_start_with_an_underscore_hold_a_slash_or_are_empty_are_refused() {
        for bad in ["_x", "_template", "a/b", ""] {
            assert_eq!(
                check_names(OsStr::new(bad), OsStr::new("o/r")),
                Err(Refusal::BadProfile(OsString::from(bad)))
            );
        }
        assert_eq!(
            check_names(OsStr::new("ok"), OsStr::new("widgets")),
            Err(Refusal::BadSlug("widgets".into()))
        );
        assert_eq!(check_names(OsStr::new("ok"), OsStr::new("o/r")), Ok(()));
    }

    #[test]
    fn a_pipe_and_an_ampersand_are_refused_and_nothing_else_is() {
        assert!(has_bad_character(OsStr::new("a|b")));
        assert!(has_bad_character(OsStr::new("a&b")));
        assert!(!has_bad_character(OsStr::new("a\\b $(x) 'y' /z")));
    }

    #[test]
    fn each_value_replaces_its_placeholder_in_order_and_later_ones_see_earlier_output() {
        let subs = Substitutions::parse([b"__SLUG__", b"o/r", b"/p", b"acct", b"Ann"]).unwrap();
        assert_eq!(
            subs.apply(b"__PROFILE__ __SLUG__ __REPO_PATH__ __GH_ACCOUNT__ __MAINTAINER_NAME__"),
            b"o/r o/r /p acct Ann"
        );
    }

    #[test]
    fn the_first_value_sed_refuses_is_the_error_and_names_its_expression() {
        let failure = Substitutions::parse([b"a", b"b", b"c\\1", b"d\\2", b"e"]).unwrap_err();
        assert_eq!(failure.expression, 3);
        assert_eq!(
            failure.message(),
            "sed: -e expression #3, char 21: invalid reference \\1 on `s' command's RHS"
        );
    }

    #[test]
    fn the_tasks_line_keeps_the_text_after_its_last_quote() {
        assert_eq!(
            tasks_of(b"TASKS=\"review issues\"\n"),
            vec![b"review".to_vec(), b"issues".to_vec()]
        );
        assert_eq!(
            tasks_of(b"TASKS=\"a b\" # c\n"),
            vec![b"a".to_vec(), b"b".to_vec(), b"#".to_vec(), b"c".to_vec()]
        );
        assert!(tasks_of(b"TASKS=\"unclosed\nX=1\n").is_empty());
        assert!(tasks_of(b"  TASKS=\"a\"\n").is_empty());
        assert_eq!(tasks_of(b"TASKS=\"a\"\nTASKS=\"b\"\n"), vec![b"a".to_vec(), b"b".to_vec()]);
    }

    #[test]
    fn the_timer_unit_pads_the_hour_and_names_the_service() {
        let text = String::from_utf8(timer_unit(OsStr::new("alpha"), b"review", 9)).unwrap();
        assert!(text.contains("OnCalendar=*-*-* 09:17:00\n"));
        assert!(text.contains("Unit=maintainer@alpha-review.service\n"));
        let late = String::from_utf8(timer_unit(OsStr::new("alpha"), b"p", 24)).unwrap();
        assert!(
            late.contains("OnCalendar=*-*-* 24:17:00\n"),
            "pins current behaviour: 24:17 is not a valid time"
        );
    }

    #[test]
    fn a_placeholder_is_two_underscores_capitals_and_two_underscores() {
        assert!(has_placeholder(b"Hello __EXTRA__ and x"));
        assert!(has_placeholder(b"Owner: __MY_OWNER__"));
        assert!(has_placeholder(b"____"));
        assert!(!has_placeholder(b"___"));
        assert!(!has_placeholder(b"__a__"));
        assert!(!has_placeholder(b"__A_ _B__"));
        assert!(has_placeholder(b"x __ABC__ y"));
        assert!(has_placeholder(b"__A_B__"));
    }

    #[test]
    fn the_closing_text_names_the_profile_five_times() {
        let text = String::from_utf8(next_steps(OsStr::new("alpha"))).unwrap();
        assert_eq!(text.matches("alpha").count(), 4);
        assert!(text.starts_with("\n  profiles/alpha/ is ready."));
        assert!(text.ends_with("then enable the timers with ./install.sh --timers.\n"));
    }
}
