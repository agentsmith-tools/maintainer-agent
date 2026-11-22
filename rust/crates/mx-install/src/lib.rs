//! `install.sh`, `new-profile.sh` and the platform scripts of magent, in Rust.
//!
//! # Shape
//!
//! [`plan`] holds the decisions. Probes read the machine into facts, a pure function turns
//! the facts into a [`plan::Plan`] or a [`refusal::Refusal`], and [`apply`] performs the
//! plan through a sink: a live one that changes the machine, or a dry one that prints
//! `would: ...`. A refusal comes before the first step, so a run that refuses has written
//! nothing.
//!
//! The renderers are pure functions of their input: [`cron`] (the crontab block),
//! [`launchd`] (the property list), [`alert`] (the alert line, the notification and the
//! instance split) and [`newprofile`] (the timer units and the text of `new-profile`).
//! [`sedsub`] reproduces what GNU sed does with the text it is handed, because the bash
//! builds sed scripts out of user input and the port must write the same bytes.
//!
//! # Differences from the bash
//!
//! Each is a numbered decision (`P4A-n`) in the pull request that added this crate.

pub mod alert;
pub mod args;
pub mod console;
pub mod cron;
pub mod launchd;
pub mod newprofile;
pub mod plan;
pub mod refusal;
pub mod sedsub;
pub mod util;
