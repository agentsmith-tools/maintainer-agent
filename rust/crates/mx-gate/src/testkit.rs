//! The world every behaviour test of the gate runs in: fakes for each port, a receipt key, and a
//! ready pull request #7 at a known head that merges, so a test changes one fact and names what
//! the gate then does.

use crate::{
    Account, Dirs, Exit, FakeGateGit, FakeGh, FakeHelper, FakeIdentity, FakeLockPort, FakeSay,
    Gate, GateConfig, LeaseAnswer, Ports, Posting, PrNumber, RuntimeProbe, StampSource,
    VerifySettings, WhoAmI,
};
use mx_ports::{EffectLog, FakeClock, FakeFs, Script};
use mx_receipt::{DependencyFacts, FoldBase, Key, Part, Receipt, Signed, Stamp};
use mx_sandbox::FakeSandbox;
use mx_suite::{FakeSelector, FakeSuiteHost, ProdGlobs, Runtime};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
pub const NEW_HEAD: &str = "fedcba9876543210fedcba9876543210fedcba98";
pub const KEY_TEXT: &[u8] = b"test-key-material\n";

pub fn pr(n: &str) -> PrNumber {
    PrNumber::parse(n).unwrap()
}

pub struct FixedStamp;

impl StampSource for FixedStamp {
    fn stamp(&self) -> Stamp {
        Stamp::new("2026-11-22T10:00:00.000000+00:00", "tester")
    }
}

pub struct FixedRuntime(pub Option<Runtime>);

impl RuntimeProbe for FixedRuntime {
    fn runtime(&self) -> Option<Runtime> {
        self.0
    }
}

#[derive(Default)]
pub struct FakeDirs {
    pub made: Mutex<Vec<PathBuf>>,
    pub removed: Mutex<Vec<PathBuf>>,
    pub fail_on: Mutex<Vec<PathBuf>>,
}

impl Dirs for FakeDirs {
    fn make_dir(&self, path: &Path) -> Result<(), String> {
        self.made.lock().unwrap().push(path.to_path_buf());
        if self.fail_on.lock().unwrap().iter().any(|p| p == path) {
            return Err("read-only file system".to_string());
        }
        Ok(())
    }

    fn remove_tree(&self, path: &Path) {
        self.removed.lock().unwrap().push(path.to_path_buf());
    }
}

pub struct World {
    pub cfg: GateConfig,
    pub gh: FakeGh,
    pub git: FakeGateGit,
    pub helper: FakeHelper,
    pub identity: FakeIdentity,
    pub locks: FakeLockPort,
    pub fs: FakeFs,
    pub dirs: FakeDirs,
    pub say: FakeSay,
    pub clock: FakeClock,
    pub selector: FakeSelector,
    pub suites: FakeSuiteHost,
    pub sandbox: FakeSandbox,
    pub runtime: FixedRuntime,
    pub verify: VerifySettings,
}

pub fn key() -> Key {
    Key::from_file_bytes(KEY_TEXT).unwrap()
}

pub fn part(suite: &str) -> Part {
    Part {
        suite: suite.into(),
        units_run: "3".into(),
        filter: "guard".into(),
        mutation: "s/a/b/".into(),
        mutated_rc: 101,
        observed_failure: "assertion failed".into(),
    }
}

pub fn observed(pr: u64, head: &str, suite: &str, partial: bool) -> Receipt<Signed> {
    Receipt::observed(
        pr,
        head,
        FoldBase::Fresh { note: None },
        &part(suite),
        partial.then_some("1"),
        &FixedStamp.stamp(),
    )
    .sign(&key())
}

pub fn asserted(pr: u64, head: &str) -> Receipt<Signed> {
    Receipt::asserted(pr, head, "a human said so", &FixedStamp.stamp(), false).unwrap().sign(&key())
}

pub fn dependency(pr: u64, head: &str) -> Receipt<Signed> {
    let facts = DependencyFacts {
        files: "Cargo.lock".into(),
        changed_lines: 2,
        pins_verified: 0,
        images_verified: 0,
        pip_verified: 0,
    };
    Receipt::dependency(pr, head, &facts, &FixedStamp.stamp()).sign(&key())
}

impl World {
    pub fn new() -> Self {
        let log = EffectLog::new();
        let world = Self {
            cfg: GateConfig {
                slug: "o/r".into(),
                repo: "/repo".into(),
                state: "/s".into(),
                account: Account::new("testuser").unwrap(),
                posting: Posting::On,
                prod_globs: Some(ProdGlobs::from_profile_value("bin/* lib/*").unwrap()),
                claim_label: String::new(),
                receipt_key: "/k/receipt.key".into(),
                in_run: false,
                run_id: 1,
                has_suite_host: true,
                readback_reads: Some("3".into()),
                readback_wait: Some("0".into()),
            },
            gh: FakeGh::new(),
            git: FakeGateGit::new(),
            helper: FakeHelper::new(),
            identity: FakeIdentity::always(WhoAmI::Login("testuser".into())),
            locks: FakeLockPort::new(),
            fs: FakeFs::new(&log),
            clock: FakeClock::at(0, &log),
            dirs: FakeDirs::default(),
            say: FakeSay::new(),
            selector: FakeSelector::new(),
            suites: FakeSuiteHost::new("/verify.d", Vec::new()),
            sandbox: FakeSandbox::new(),
            runtime: FixedRuntime(Some(Runtime::Podman)),
            verify: VerifySettings {
                verify_timeout: "600".into(),
                prefetch_timeout: "300".into(),
                env: std::collections::BTreeMap::new(),
                profile: [
                    ("VERIFY_MEMORY", "6g"),
                    ("VERIFY_CPUS", "2"),
                    ("VERIFY_PIDS", "512"),
                    ("VERIFY_TMPFS", "1g"),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            },
        };
        world.fs.put("/k/receipt.key", KEY_TEXT, 0);
        world.helper.script_lease(Script::always(Ok(LeaseAnswer::Free)));
        world.helper.script_holders(Script::always(Ok(Vec::new())));
        world.gh.script_head(Script::always(Ok(HEAD.into())));
        world.gh.script_merge_state(Script::always(Ok("CLEAN".into())));
        world.gh.script_review_decision(Script::always(Ok("APPROVED".into())));
        world.gh.script_closing_issues(Script::always(Ok(Vec::new())));
        world.gh.script_checks(Script::always(Ok(r#"[{"name":"ci","bucket":"pass"}]"#.into())));
        world.gh.script_merge(Script::always(Ok(())));
        world.git.script_fetch_ref(Script::always(Ok(())));
        world.git.script_rev_parse(Script::always(Ok(HEAD.into())));
        world.receipt(&observed(7, HEAD, "rust", false));
        world
    }

    pub fn receipt(&self, receipt: &Receipt<Signed>) {
        let n = receipt.body().pr;
        self.fs.put(format!("/s/receipts/{n}.json"), receipt.to_bytes(), 0);
    }

    pub fn gate(&self) -> Gate<'_> {
        Gate::new(&self.cfg, self.ports())
    }

    pub fn ports(&self) -> Ports<'_> {
        {
            Ports {
                gh: &self.gh,
                git: &self.git,
                helper: &self.helper,
                identity: &self.identity,
                locks: &self.locks,
                fs: &self.fs,
                dirs: &self.dirs,
                say: &self.say,
                clock: &self.clock,
                stamps: &FixedStamp,
                selector: &self.selector,
                suites: &self.suites,
                sandbox: &self.sandbox,
                runtime: &self.runtime,
                verify: &self.verify,
            }
        }
    }

    pub fn merge(&self) -> Result<(), Exit> {
        self.gate().merge(&pr("7"))
    }

    pub fn update(&self) -> Result<(), Exit> {
        self.gate().update(&pr("7"))
    }

    pub fn stdout(&self) -> String {
        self.say.stdout()
    }

    pub fn receipt_on_disk(&self, n: u64) -> Option<Vec<u8>> {
        self.fs.peek(format!("/s/receipts/{n}.json"))
    }

    pub fn calls_of(&self, name: &str) -> usize {
        self.gh.calls().iter().filter(|c| format!("{c:?}").starts_with(name)).count()
    }
}
