//! The resources a verify container gets, and where each figure comes from.

use crate::SandboxError;
use mx_suite::LimitKey;

/// Digits only, and not all zeros.
fn is_positive_integer(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) && text.bytes().any(|b| b != b'0')
}

/// Digits, then at most one unit letter.
fn is_size(text: &str) -> bool {
    let number = match text.as_bytes().last() {
        Some(b'b' | b'k' | b'm' | b'g' | b'B' | b'K' | b'M' | b'G') => &text[..text.len() - 1],
        _ => text,
    };
    is_positive_integer(number)
}

/// Digits, optionally a point and more digits, and not all zeros.
fn is_positive_decimal(text: &str) -> bool {
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    digits(whole)
        && fraction.is_none_or(digits)
        && text.bytes().any(|b| b.is_ascii_digit() && b != b'0')
}

/// One resource figure, checked.
///
/// The text goes to the runtime as one argument (`--memory 6g`, `--cpus 2`,
/// `--pids-limit 512`, `--tmpfs /tmp:rw,nosuid,size=256m`). Podman reads `0` as
/// no limit, so a `0` in `MAINTAINER_VERIFY_PIDS` removes the wall a fork bomb
/// hits and `0` in `MAINTAINER_VERIFY_MEMORY` removes the cap the OOM kill
/// relies on. The bash passes the value through. This type refuses it
/// (decision P30-3), together with anything that is not a plain amount:
///
/// - memory and tmpfs: digits, then at most one of `b k m g` in either case;
/// - pids: digits;
/// - cpus: digits, optionally a `.` and digits;
/// - never zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limit(String);

impl Limit {
    /// Checks `text` as the figure for `key`.
    pub fn parse(key: LimitKey, text: &str) -> Result<Self, SandboxError> {
        let well_formed = match key {
            LimitKey::Memory | LimitKey::Tmpfs => is_size(text),
            LimitKey::Pids => is_positive_integer(text),
            LimitKey::Cpus => is_positive_decimal(text),
        };
        if well_formed {
            return Ok(Self(text.to_string()));
        }
        let what = match key {
            LimitKey::Memory | LimitKey::Tmpfs => "size such as 6g",
            LimitKey::Pids => "whole number",
            LimitKey::Cpus => "number",
        };
        Err(SandboxError::InvalidLimit { name: key.env_name(), value: text.to_string(), what })
    }

    /// The figure as one argument.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How long a container may run before the runtime kills it, in whole seconds.
///
/// `podman run --timeout` is enforced by conmon; GNU `timeout` around the client
/// is not (it kills the client and leaves the container spinning, measured in
/// the comment above `run_in_container`). Zero is refused (decision P30-4):
/// podman reads `--timeout 0` as no timeout, which is the unbounded run the flag
/// exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeout(u64);

impl Timeout {
    /// A timeout of `secs` seconds.
    pub fn from_secs(secs: u64) -> Result<Self, SandboxError> {
        if secs == 0 {
            return Err(SandboxError::BadTimeout { value: secs.to_string() });
        }
        Ok(Self(secs))
    }

    /// Reads `VERIFY_TIMEOUT` or `PREFETCH_TIMEOUT` as `lib/defaults.env` writes
    /// it: digits only.
    pub fn parse(text: &str) -> Result<Self, SandboxError> {
        let bad = || SandboxError::BadTimeout { value: text.to_string() };
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        let secs: u64 = text.parse().map_err(|_| bad())?;
        Self::from_secs(secs).map_err(|_| bad())
    }

    /// The number of seconds, above 0.
    #[must_use]
    pub fn secs(self) -> u64 {
        self.0
    }
}

/// The three places a figure can come from, as the bash `verify_limit` reads them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LimitSource {
    /// `MAINTAINER_<KEY>` in the environment.
    pub env: Option<String>,
    /// What the suite's own function printed, when it defines one.
    pub suite: Option<String>,
    /// `<KEY>` from `profile.env`, else `lib/defaults.env`.
    pub profile: Option<String>,
}

/// The four figures of a verify container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub tmpfs: Limit,
    pub pids: Limit,
    pub memory: Limit,
    pub cpus: Limit,
}

impl Limits {
    /// Picks each figure the way `verify_limit` does and checks it.
    ///
    /// Precedence for one key: the environment variable, then the suite's
    /// function, then the profile, then `lib/defaults.env`. An EMPTY value counts
    /// as unset at every step (`${!ev:-}`, `${v:-...}`). The suite sits above
    /// the profile on purpose: it knows what its own build needs. When every
    /// source is empty the run refuses (`LimitUnset`). The figure that WINS is
    /// the one checked: an env value of `0` is refused, not skipped in favour of
    /// the suite's.
    pub fn resolve(source: impl Fn(LimitKey) -> LimitSource) -> Result<Self, SandboxError> {
        let figure = |key: LimitKey| -> Result<Limit, SandboxError> {
            let LimitSource { env, suite, profile } = source(key);
            let chosen = [env, suite, profile]
                .into_iter()
                .flatten()
                .find(|value| !value.is_empty())
                .ok_or(SandboxError::LimitUnset { key: key.defaults_key() })?;
            Limit::parse(key, &chosen)
        };
        let tmpfs = figure(LimitKey::Tmpfs)?;
        let pids = figure(LimitKey::Pids)?;
        let memory = figure(LimitKey::Memory)?;
        let cpus = figure(LimitKey::Cpus)?;
        Ok(Self { tmpfs, pids, memory, cpus })
    }
}

/// The words of `suite_podman_args`, as the bash splits them: unquoted, on
/// blanks, tabs and newlines.
///
/// `$(suite_extra_args "$rt" "$scratch")` sits unquoted on the `podman run` line,
/// so a path with a space in it (`$HOME` is one) becomes two arguments, and a
/// word with `*` could also be expanded against the working directory. This port
/// splits and does not expand. Both are listed under "Found, not fixed".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtraArgs(Vec<String>);

impl ExtraArgs {
    /// Splits what the suite printed.
    #[must_use]
    pub fn split(printed: &str) -> Self {
        Self(
            printed
                .split([' ', '\t', '\n'])
                .filter(|word| !word.is_empty())
                .map(str::to_string)
                .collect(),
        )
    }

    /// The words, in order.
    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(key: LimitKey, text: &str) -> Result<String, SandboxError> {
        Limit::parse(key, text).map(|l| l.as_str().to_string())
    }

    #[test]
    fn the_figures_of_lib_defaults_env_are_accepted() {
        assert_eq!(limit(LimitKey::Memory, "2g").unwrap(), "2g");
        assert_eq!(limit(LimitKey::Cpus, "2").unwrap(), "2");
        assert_eq!(limit(LimitKey::Pids, "512").unwrap(), "512");
        assert_eq!(limit(LimitKey::Tmpfs, "256m").unwrap(), "256m");
        assert_eq!(limit(LimitKey::Memory, "6g").unwrap(), "6g");
    }

    #[test]
    fn units_in_either_case_and_a_plain_byte_count_are_accepted_for_memory_and_tmpfs() {
        for text in ["1024", "512M", "1G", "64k", "100b", "7B"] {
            assert!(Limit::parse(LimitKey::Memory, text).is_ok(), "{text}");
            assert!(Limit::parse(LimitKey::Tmpfs, text).is_ok(), "{text}");
        }
    }

    #[test]
    fn a_fraction_is_a_cpu_figure_and_nothing_else() {
        assert_eq!(limit(LimitKey::Cpus, "1.5").unwrap(), "1.5");
        for malformed in ["1.", ".5", "1.5.5", "."] {
            assert!(Limit::parse(LimitKey::Cpus, malformed).is_err(), "{malformed:?}");
        }
        assert!(Limit::parse(LimitKey::Pids, "1.5").is_err());
        assert!(Limit::parse(LimitKey::Memory, "1.5g").is_err());
    }

    #[test]
    fn zero_is_refused_for_every_key() {
        // P30-3: the runtime reads 0 as no limit.
        for key in LimitKey::ALL {
            for text in ["0", "00", "0g", "0.0"] {
                let refused = Limit::parse(key, text);
                assert!(
                    matches!(refused, Err(SandboxError::InvalidLimit { .. })),
                    "{key:?} {text}: {refused:?}"
                );
            }
        }
    }

    #[test]
    fn a_negative_or_unlimited_figure_is_refused() {
        for key in LimitKey::ALL {
            for text in ["-1", "-512", "max", "unlimited", ""] {
                assert!(Limit::parse(key, text).is_err(), "{key:?} {text:?}");
            }
        }
    }

    #[test]
    fn a_comma_a_colon_a_space_or_a_second_unit_is_refused() {
        // The tmpfs figure ends up inside `--tmpfs /tmp:rw,nosuid,size=<figure>`,
        // so a comma would add a mount option.
        for text in ["256m,exec", "256m:rw", "2 g", "2gg", "g2", "2g\n", "1e3"] {
            assert!(Limit::parse(LimitKey::Tmpfs, text).is_err(), "{text:?}");
            assert!(Limit::parse(LimitKey::Memory, text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn a_refused_figure_names_the_variable_and_what_it_should_be() {
        let refused = Limit::parse(LimitKey::Pids, "0").unwrap_err();
        assert_eq!(
            refused.to_string(),
            "MAINTAINER_VERIFY_PIDS is '0', which is not a positive whole number"
        );
        let refused = Limit::parse(LimitKey::Cpus, "x").unwrap_err();
        assert_eq!(
            refused.to_string(),
            "MAINTAINER_VERIFY_CPUS is 'x', which is not a positive number"
        );
        let refused = Limit::parse(LimitKey::Memory, "0").unwrap_err();
        assert_eq!(
            refused.to_string(),
            "MAINTAINER_VERIFY_MEMORY is '0', which is not a positive size such as 6g"
        );
    }

    #[test]
    fn a_timeout_is_a_whole_number_of_seconds_above_zero() {
        assert_eq!(Timeout::parse("1800").unwrap().secs(), 1800);
        assert_eq!(Timeout::parse("900").unwrap().secs(), 900);
        assert_eq!(Timeout::from_secs(1).unwrap().secs(), 1);
    }

    #[test]
    fn a_zero_or_malformed_timeout_is_refused() {
        // P30-4.
        for text in
            ["0", "00", "", "-5", "+5", "1.5", "10s", " 5", "5\n", "99999999999999999999999"]
        {
            assert!(
                matches!(Timeout::parse(text), Err(SandboxError::BadTimeout { .. })),
                "{text:?}"
            );
        }
        assert_eq!(Timeout::from_secs(0), Err(SandboxError::BadTimeout { value: "0".into() }));
        assert_eq!(
            Timeout::from_secs(0).unwrap_err().to_string(),
            "the container timeout is '0', which is not a whole number of seconds above 0"
        );
    }

    fn only_default(key: LimitKey) -> LimitSource {
        let value = match key {
            LimitKey::Memory => "2g",
            LimitKey::Cpus => "2",
            LimitKey::Pids => "512",
            LimitKey::Tmpfs => "256m",
        };
        LimitSource { env: None, suite: None, profile: Some(value.to_string()) }
    }

    #[test]
    fn the_profile_figures_apply_when_nothing_overrides_them() {
        let limits = Limits::resolve(only_default).unwrap();
        assert_eq!(
            (
                limits.tmpfs.as_str(),
                limits.pids.as_str(),
                limits.memory.as_str(),
                limits.cpus.as_str()
            ),
            ("256m", "512", "2g", "2")
        );
    }

    #[test]
    fn the_suite_overrides_the_profile_and_the_environment_overrides_the_suite() {
        let limits = Limits::resolve(|key| {
            let mut source = only_default(key);
            if key == LimitKey::Memory {
                source.suite = Some("6g".into());
            }
            if key == LimitKey::Pids {
                source.suite = Some("1024".into());
                source.env = Some("2048".into());
            }
            source
        })
        .unwrap();
        assert_eq!(limits.memory.as_str(), "6g", "the suite beats the profile");
        assert_eq!(limits.pids.as_str(), "2048", "the environment beats the suite");
        assert_eq!(limits.tmpfs.as_str(), "256m");
    }

    #[test]
    fn an_empty_value_counts_as_unset_at_every_step() {
        let limits = Limits::resolve(|key| {
            let mut source = only_default(key);
            source.env = Some(String::new());
            source.suite = Some(String::new());
            source
        })
        .unwrap();
        assert_eq!(limits.memory.as_str(), "2g");
    }

    #[test]
    fn a_figure_that_no_source_gives_refuses_and_names_the_key() {
        let refused = Limits::resolve(|key| {
            if key == LimitKey::Cpus {
                LimitSource { env: None, suite: Some(String::new()), profile: Some(String::new()) }
            } else {
                only_default(key)
            }
        })
        .unwrap_err();
        assert_eq!(refused, SandboxError::LimitUnset { key: "VERIFY_CPUS" });
        assert_eq!(refused.to_string(), "VERIFY_CPUS: lib/defaults.env was not loaded");
    }

    #[test]
    fn the_figure_that_wins_is_the_one_checked_and_a_bad_winner_is_not_skipped() {
        let refused = Limits::resolve(|key| {
            let mut source = only_default(key);
            if key == LimitKey::Pids {
                source.env = Some("0".into());
                source.suite = Some("512".into());
            }
            source
        })
        .unwrap_err();
        assert!(
            matches!(refused, SandboxError::InvalidLimit { name: "MAINTAINER_VERIFY_PIDS", .. }),
            "{refused:?}"
        );
    }

    #[test]
    fn extra_args_split_on_blanks_tabs_and_newlines() {
        let printed =
            "-v\n/home/u/.cargo:/cargo:O,upperdir=/s/.cargo-upper\n-e CARGO_HOME=/cargo\t-e\nX=1\n";
        assert_eq!(
            ExtraArgs::split(printed).as_slice(),
            [
                "-v",
                "/home/u/.cargo:/cargo:O,upperdir=/s/.cargo-upper",
                "-e",
                "CARGO_HOME=/cargo",
                "-e",
                "X=1"
            ]
        );
    }

    #[test]
    fn a_word_with_a_space_in_it_becomes_two_words_as_in_the_bash() {
        // Found, not fixed (P30-10): $HOME with a space splits the overlay argument.
        assert_eq!(
            ExtraArgs::split("-v\n/home/a b/.cargo:/cargo:O").as_slice(),
            ["-v", "/home/a", "b/.cargo:/cargo:O"]
        );
    }

    #[test]
    fn a_glob_character_is_kept_as_it_is_and_never_expanded() {
        assert_eq!(ExtraArgs::split("--x=*.rs").as_slice(), ["--x=*.rs"]);
    }

    #[test]
    fn nothing_printed_gives_no_words() {
        assert!(ExtraArgs::split("").as_slice().is_empty());
        assert!(ExtraArgs::split(" \n\t \n").as_slice().is_empty());
    }
}
