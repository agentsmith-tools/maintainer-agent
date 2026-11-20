//! The environment `run.sh` exports, as data.
//!
//! In the bash every child inherits the shell's whole environment, which `export`
//! and `unset` change along the way. Here that environment is an [`Env`]: the
//! caller's variables, changed by the same calls in the same order, and handed to
//! every child name by name. This is decision `P2A-env`:
//!
//! - A child gets every variable of the [`Env`] except `PATH` and `LC_ALL`, which
//!   the process adapters own (`PATH` is the search path they were built with,
//!   `LC_ALL` the locale of the caller, or `C`).
//! - A variable whose name or value is not UTF-8 is not passed on. The adapters
//!   take text.
//! - The profile is evaluated in a child bash, so a variable that `profile.env`
//!   exports itself does not reach the agent. The shipped profiles export nothing.

use std::collections::BTreeMap;

/// The exported environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    vars: BTreeMap<String, String>,
}

impl Env {
    /// The environment of the calling process, UTF-8 variables only.
    #[must_use]
    pub fn from_process() -> Self {
        Self::from_pairs(std::env::vars_os().filter_map(|(name, value)| {
            Some((name.into_string().ok()?, value.into_string().ok()?))
        }))
    }

    /// An environment made of the given pairs.
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        Self { vars: pairs.into_iter().collect() }
    }

    /// `${NAME-}`: the value, or `None` when unset.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }

    /// `[ -n "${NAME:-}" ]` and the value: set and not empty.
    #[must_use]
    pub fn non_empty(&self, name: &str) -> Option<&str> {
        self.get(name).filter(|value| !value.is_empty())
    }

    /// `export NAME=value`.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.vars.insert(name.into(), value.into());
    }

    /// `unset NAME`.
    pub fn unset(&mut self, name: &str) {
        self.vars.remove(name);
    }

    /// Every variable, sorted by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars.iter().map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// What a child receives: everything but `PATH` and `LC_ALL`.
    #[must_use]
    pub fn for_child(&self) -> Vec<(String, String)> {
        self.vars
            .iter()
            .filter(|(name, _)| name.as_str() != "PATH" && name.as_str() != "LC_ALL")
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env::from_pairs([
            ("A".to_string(), "1".to_string()),
            ("EMPTY".to_string(), String::new()),
            ("PATH".to_string(), "/bin".to_string()),
            ("LC_ALL".to_string(), "C".to_string()),
        ])
    }

    #[test]
    fn a_variable_that_is_set_and_empty_differs_from_one_that_is_unset() {
        let env = env();
        assert_eq!(env.get("EMPTY"), Some(""));
        assert_eq!(env.get("NOPE"), None);
        assert_eq!(env.non_empty("EMPTY"), None);
        assert_eq!(env.non_empty("A"), Some("1"));
    }

    #[test]
    fn export_and_unset_change_what_a_child_receives() {
        let mut env = env();
        env.set("GH_TOKEN", "t");
        env.unset("A");
        let names: Vec<String> = env.for_child().into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, vec!["EMPTY", "GH_TOKEN"]);
    }

    #[test]
    fn a_child_never_receives_path_or_lc_all_from_the_environment() {
        let names: Vec<String> = env().for_child().into_iter().map(|(name, _)| name).collect();
        assert!(!names.contains(&"PATH".to_string()));
        assert!(!names.contains(&"LC_ALL".to_string()));
    }
}
