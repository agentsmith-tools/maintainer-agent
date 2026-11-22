//! The command: `maintainer-merge verify-deps <pr> <sha>` from the environment to the exit code.
//!
//! `bin/maintainer-merge` reads `MAINTAINER_SLUG`, `MAINTAINER_STATE` and the profile's
//! `CLASSIFY_TIMEOUT`, takes the lease, runs the checks, and signs and writes the receipt. This
//! module does the same through ports, so the gate binary calls [`run`] for its `verify-deps`
//! arm and the standalone binary `mx-verify-deps` (the differential harness of this module) calls
//! it for the whole program.

use crate::deps::{
    ClassifyLimit, DepsConfig, DepsFailure, DepsPorts, DepsRefusal, DepsVerified, ScriptPaths,
    lease_gate, verify_deps,
};
use crate::{Exit, GateDecision, HeadArg, Helper, PrNumber, Say};
use mx_core::Fact;
use mx_ports::Fs;
use mx_receipt::{Key, Stamp, write};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Everything the command needs from the outside.
pub struct Wiring<'a> {
    pub ports: DepsPorts<'a>,
    pub helper: &'a dyn Helper,
    /// `recorded_at` and `recorded_by`, read when the receipt is written, after the checks.
    pub stamp: &'a dyn Fn() -> Stamp,
}

/// `: "${VAR:?}"`: a variable that is set and not empty.
fn required<'a>(env: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, Exit> {
    match env.get(name) {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(Exit {
            code: 1,
            stderr: format!("maintainer-merge: {name}: parameter null or not set\n"),
        }),
    }
}

fn settings(env: &BTreeMap<String, String>) -> Result<(DepsConfig, PathBuf, PathBuf), Exit> {
    let slug = required(env, "MAINTAINER_SLUG")?;
    let state = required(env, "MAINTAINER_STATE")?;
    let var = |name: &str| env.get(name).cloned();
    let limit = ["MAINTAINER_CLASSIFY_TIMEOUT", "CLASSIFY_TIMEOUT"]
        .iter()
        .find_map(|name| env.get(*name).filter(|value| !value.is_empty()))
        .ok_or_else(|| Exit::die(1, "CLASSIFY_TIMEOUT: lib/defaults.env was not loaded"))?;
    let key = env.get("MAINTAINER_RECEIPT_KEY").filter(|value| !value.is_empty()).map_or_else(
        || {
            PathBuf::from(env.get("HOME").map_or("", String::as_str))
                .join(".local/share/maintainer/receipt.key")
        },
        PathBuf::from,
    );
    let config = DepsConfig {
        repo_slug: slug.to_string(),
        scripts: ScriptPaths::from_env(var),
        limit: ClassifyLimit::parse(limit),
    };
    Ok((config, Path::new(state).join("receipts"), key))
}

/// Signs the receipt for what was verified and writes it to `<receipts>/<pr>.json`, replacing an
/// older one, then says the last line.
///
/// The key is read before anything is written (decision `P3C-7`): the bash writes the unsigned
/// receipt first and then finds no key, which leaves an unsigned file behind that `merge`
/// refuses anyway.
pub fn record(
    fs: &dyn Fs,
    key_path: &Path,
    receipts: &Path,
    stamp: &Stamp,
    verified: &DepsVerified,
    say: &dyn Say,
) -> Result<(), DepsFailure> {
    let key_bytes = match fs.read(key_path) {
        Fact::Known(bytes) => bytes,
        Fact::Absent | Fact::Unreadable(_) => {
            return Err(DepsFailure::Gate(Box::new(GateDecision::NoReceiptKey {
                path: key_path.to_path_buf(),
            })));
        }
    };
    let key = Key::from_file_bytes(&key_bytes).map_err(|_| DepsRefusal::CannotRecord)?;
    let receipt = verified.receipt(stamp).sign(&key);
    let file = receipts.join(format!("{}.json", verified.pr().as_arg()));
    write(fs, &file, &receipt).map_err(|_| DepsRefusal::CannotRecord)?;
    say.say(&verified.recorded_line());
    Ok(())
}

/// Runs `verify-deps` for `pr` at `sha`: the lease, the checks, the receipt. The exit code and
/// standard error are returned; the progress lines went to [`Say`] as they happened.
#[must_use]
pub fn run(
    env: &BTreeMap<String, String>,
    pr: &PrNumber,
    sha: &HeadArg,
    wiring: &Wiring<'_>,
) -> Exit {
    let (config, receipts, key) = match settings(env) {
        Ok(settings) => settings,
        Err(exit) => return exit,
    };
    let outcome = lease_gate(wiring.helper, pr)
        .and_then(|lease| {
            verify_deps(&config, &wiring.ports, sha, &lease).map_err(DepsFailure::from)
        })
        .and_then(|verified| {
            let stamp = (wiring.stamp)();
            record(wiring.ports.fs, &key, &receipts, &stamp, &verified, wiring.ports.say)
        });
    match outcome {
        Ok(()) => Exit { code: 0, stderr: String::new() },
        Err(failure) => Exit { code: failure.exit_code(), stderr: failure.stderr_line() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::{ClassifierExit, ClassifierRun, FakeClassifier, FakeNet, FakeWholeFile};
    use crate::{FakeGh, FakeHelper, FakeSay, LeaseAnswer};
    use mx_ports::{EffectLog, FakeFs, Script};
    use mx_receipt::Stored;

    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
    const LOCKFILE: &str = r#"{"verdict": "dependency-bump", "files": {"Cargo.lock": "cargo-lock"}, "changed_lines": 4, "pins": [], "images": [], "pip": []}"#;

    struct Rig {
        gh: FakeGh,
        net: FakeNet,
        classifier: FakeClassifier,
        whole: FakeWholeFile,
        fs: FakeFs,
        say: FakeSay,
        helper: FakeHelper,
    }

    fn env(extra: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut vars: BTreeMap<String, String> = [
            ("MAINTAINER_SLUG", "o/r"),
            ("MAINTAINER_STATE", "/state"),
            ("MAINTAINER_RECEIPT_KEY", "/k/receipt.key"),
            ("MAINTAINER_SCRIPTS", "/s"),
            ("CLASSIFY_TIMEOUT", "60"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
        for (key, value) in extra {
            vars.insert((*key).to_string(), (*value).to_string());
        }
        vars
    }

    impl Rig {
        fn new() -> Self {
            let fs = FakeFs::new(&EffectLog::new());
            fs.put("/s/classify_dependency_bump.py", "#", 1);
            fs.put("/k/receipt.key", "secret key\n", 1);
            let gh = FakeGh::new();
            gh.script_head(Script::always(Ok(HEAD.into())));
            gh.script_diff(Script::always(Ok("diff --git a/Cargo.lock b/Cargo.lock\n".into())));
            let classifier = FakeClassifier::new();
            classifier.script(Script::always(Ok(ClassifierRun {
                exit: ClassifierExit::Code(0),
                stdout: LOCKFILE.into(),
            })));
            let helper = FakeHelper::new();
            helper.script_lease(Script::always(Ok(LeaseAnswer::Free)));
            Self {
                gh,
                net: FakeNet::new(),
                classifier,
                whole: FakeWholeFile::new(),
                fs,
                say: FakeSay::new(),
                helper,
            }
        }

        fn run(&self, vars: &BTreeMap<String, String>) -> Exit {
            let stamp = || Stamp::new("2026-11-22T10:00:00+00:00", "gate");
            let wiring = Wiring {
                ports: DepsPorts {
                    gh: &self.gh,
                    net: &self.net,
                    classifier: &self.classifier,
                    whole_file: &self.whole,
                    fs: &self.fs,
                    say: &self.say,
                },
                helper: &self.helper,
                stamp: &stamp,
            };
            run(vars, &PrNumber::parse("7").unwrap(), &HeadArg::parse("0123456").unwrap(), &wiring)
        }
    }

    #[test]
    fn a_clean_bump_exits_zero_writes_a_signed_receipt_and_says_the_last_line() {
        let rig = Rig::new();
        let exit = rig.run(&env(&[]));
        assert_eq!(exit, Exit { code: 0, stderr: String::new() });
        let bytes = rig.fs.peek("/state/receipts/7.json").expect("the receipt was written");
        let stored = Stored::parse(&bytes).unwrap();
        stored.verify_signature(&Key::from_file_bytes(b"secret key\n").unwrap()).unwrap();
        assert_eq!(stored.field_text("kind", ""), "dependency");
        assert_eq!(
            stored.field_text("head", ""),
            HEAD,
            "the receipt names the head GitHub reported"
        );
        assert_eq!(stored.field_text("recorded_by", ""), "gate");
        assert_eq!(
            rig.say.stdout(),
            "  dependency receipt recorded for #7 at 01234567 (4 line(s), 0 pin(s), 0 image(s), 0 pip package(s) verified)\n"
        );
    }

    #[test]
    fn a_refusal_exits_one_prints_the_message_and_writes_no_receipt() {
        let rig = Rig::new();
        rig.gh.script_diff(Script::always(Ok(String::new())));
        let exit = rig.run(&env(&[]));
        assert_eq!(
            exit,
            Exit {
                code: 1,
                stderr: "maintainer-merge: the diff for #7 is empty, so nothing was classified\n"
                    .into()
            }
        );
        assert_eq!(rig.fs.peek("/state/receipts/7.json"), None);
        assert_eq!(rig.say.stdout(), "");
    }

    #[test]
    fn a_lease_held_by_another_actor_stops_the_command_before_gh_is_read() {
        let rig = Rig::new();
        rig.helper
            .script_lease(Script::always(Ok(LeaseAnswer::Held { output: "amy has it".into() })));
        let exit = rig.run(&env(&[]));
        assert_eq!(exit.code, 1);
        assert!(exit.stderr.starts_with(
            "maintainer-merge: will not verify #7: another actor has leased it.\n  amy has it\n"
        ));
        assert!(rig.gh.calls().is_empty());
    }

    #[test]
    fn a_required_variable_that_is_missing_or_empty_stops_the_command_first() {
        for name in ["MAINTAINER_SLUG", "MAINTAINER_STATE"] {
            for value in [None, Some("")] {
                let rig = Rig::new();
                let mut vars = env(&[]);
                match value {
                    None => drop(vars.remove(name)),
                    Some(text) => drop(vars.insert(name.into(), text.into())),
                }
                let exit = rig.run(&vars);
                assert_eq!(exit.code, 1, "{name} {value:?}");
                assert!(exit.stderr.contains(name), "{}", exit.stderr);
                assert!(rig.gh.calls().is_empty());
            }
        }
    }

    #[test]
    fn a_profile_without_a_classify_timeout_is_refused_and_the_specific_variable_wins() {
        let rig = Rig::new();
        let mut vars = env(&[]);
        vars.remove("CLASSIFY_TIMEOUT");
        assert_eq!(
            rig.run(&vars).stderr,
            "maintainer-merge: CLASSIFY_TIMEOUT: lib/defaults.env was not loaded\n"
        );
        let rig = Rig::new();
        let exit = rig.run(&env(&[("MAINTAINER_CLASSIFY_TIMEOUT", "7")]));
        assert_eq!(exit.code, 0);
        assert_eq!(rig.classifier.calls()[0].limit, std::time::Duration::from_secs(7));
    }

    #[test]
    fn the_key_is_found_under_home_when_no_override_is_set() {
        let rig = Rig::new();
        rig.fs.put("/home/u/.local/share/maintainer/receipt.key", "other\n", 1);
        let mut vars = env(&[("HOME", "/home/u")]);
        vars.remove("MAINTAINER_RECEIPT_KEY");
        assert_eq!(rig.run(&vars).code, 0);
        let stored = Stored::parse(&rig.fs.peek("/state/receipts/7.json").unwrap()).unwrap();
        stored.verify_signature(&Key::from_file_bytes(b"other\n").unwrap()).unwrap();
    }

    #[test]
    fn a_missing_key_refuses_and_leaves_no_unsigned_receipt_behind() {
        let rig = Rig::new();
        let mut vars = env(&[]);
        vars.insert("MAINTAINER_RECEIPT_KEY".into(), "/k/absent.key".into());
        let exit = rig.run(&vars);
        assert_eq!(
            exit.stderr,
            "maintainer-merge: no receipt key at /k/absent.key; run ./install.sh\n"
        );
        assert_eq!(rig.fs.peek("/state/receipts/7.json"), None);
        assert_eq!(rig.say.stdout(), "", "the receipt line is said only after the file is written");
    }

    #[test]
    fn a_receipt_that_cannot_be_written_refuses_and_does_not_say_it_was_recorded() {
        let rig = Rig::new();
        rig.fs.fail_writes_to("/state/receipts/7.json", "disk full");
        let exit = rig.run(&env(&[]));
        assert_eq!(exit.stderr, "maintainer-merge: could not record the dependency receipt\n");
        assert_eq!(rig.say.stdout(), "");
    }

    #[test]
    fn a_key_that_is_not_text_cannot_sign_and_refuses_as_unrecordable() {
        let rig = Rig::new();
        rig.fs.put("/k/receipt.key", vec![0xff, 0xfe, 0x00], 1);
        assert_eq!(
            rig.run(&env(&[])).stderr,
            "maintainer-merge: could not record the dependency receipt\n"
        );
    }

    #[test]
    fn an_older_receipt_for_the_same_pull_request_is_replaced() {
        let rig = Rig::new();
        rig.fs.put("/state/receipts/7.json", "{\"old\": true}", 1);
        assert_eq!(rig.run(&env(&[])).code, 0);
        let stored = Stored::parse(&rig.fs.peek("/state/receipts/7.json").unwrap()).unwrap();
        assert_eq!(stored.field_text("kind", ""), "dependency");
    }
}
