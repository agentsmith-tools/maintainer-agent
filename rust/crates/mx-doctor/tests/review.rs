//! The fixes the independent review of the first version asked for. Each test names
//! the behaviour of the bash it holds the port to.

mod common;

use common::*;
use mx_core::Fact;
use mx_doctor::audit::{self, LeaseFile, forgotten_leases};
use mx_doctor::ctx::{Settings, Stop, Stream};
use mx_doctor::github;
use mx_doctor::host::{FakeHost, HostFs, RealHost, SCAN_CHUNK, stream_contains};
use mx_doctor::hostchecks;
use mx_doctor::install;
use mx_doctor::pyval;
use mx_doctor::report::{Item, Report};
use mx_doctor::text;
use std::collections::BTreeMap;

const NOWI: i64 = 1_767_614_400;

fn without(rig: &mut Rig, name: &str, extra: &[(&str, &str)]) {
    let mut vars: BTreeMap<String, String> = [
        ("DENY_WALL_MIN_RULES", "100"),
        ("RED_GRACE_HOURS", "6"),
        ("DOCTOR_RUN_LIMIT", "40"),
        ("DOCTOR_LEASE_HOURS", "12"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
    .collect();
    for (k, v) in extra {
        vars.insert((*k).to_string(), (*v).to_string());
    }
    vars.remove(name);
    rig.settings = Settings::new(vars);
}

// --- B1: an unbound variable ends a script file with status 1 -------------------------

#[test]
fn an_unset_floor_grace_or_lease_limit_ends_the_run_with_status_1_and_the_bash_line_numbers() {
    for (name, line) in
        [("DENY_WALL_MIN_RULES", 92), ("RED_GRACE_HOURS", 187), ("DOCTOR_LEASE_HOURS", 463)]
    {
        let mut rig = Rig::new(&[]);
        without(
            &mut rig,
            name,
            &[("REPO_SLUG", "o/r"), ("STATE_DIR", "/state"), ("REPO_PATH", "")],
        );
        rig.host.dir("/state");
        rig.proc.have("gh", "/bin/gh");
        rig.proc.on(&["gh", "auth", "token", "--user", ""], 1, "");
        let mut o = out();
        let stop = match name {
            "DENY_WALL_MIN_RULES" => {
                let p = rig.pdir();
                rig.host.file(&format!("{p}/profile.env"), b"", false, 0);
                rig.host.file(&format!("{p}/deny.json"), b"{}", false, 0);
                rig.host.file(
                    &format!("{p}/settings.json"),
                    b"{\"permissions\":{\"deny\":[1]}}",
                    false,
                    0,
                );
                install::profile_and_containment(&rig.ctx(), &mut o).unwrap_err()
            }
            "RED_GRACE_HOURS" => github::repository(&rig.ctx(), &mut o).unwrap_err(),
            _ => audit::audit_trail(&rig.ctx(), &mut o).unwrap_err(),
        };
        assert_eq!(stop, Stop::Exit(1), "{name}");
        assert_eq!(
            String::from_utf8_lossy(&o.stderr),
            format!("maintainer-doctor: line {line}: {name}: unbound variable\n")
        );
    }
}

// --- B2: an unset run limit kills only the substitution --------------------------------

#[test]
fn an_unset_run_limit_warns_on_line_189_and_goes_on_without_asking_gh_for_runs() {
    let mut rig = Rig::new(&[("REPO_SLUG", "o/r"), ("REPO_PATH", "")]);
    without(&mut rig, "DOCTOR_RUN_LIMIT", &[("REPO_SLUG", "o/r"), ("REPO_PATH", "")]);
    rig.proc.have("gh", "/bin/gh");
    let mut o = out();
    assert_eq!(github::repository(&rig.ctx(), &mut o), Ok(()));
    assert_eq!(
        String::from_utf8_lossy(&o.stderr),
        "maintainer-doctor: line 189: DOCTOR_RUN_LIMIT: unbound variable\n"
    );
    assert!(has_warn(
        &o.report,
        "could not read main's workflow runs on o/r, so whether main is red is unknown",
        "gh run list --repo o/r --branch main"
    ));
    assert!(rig.proc.tool_calls().is_empty(), "{:?}", rig.proc.tool_calls());
}

// --- B3: the scan reads in pieces and finds a token across a boundary -----------------------

#[test]
fn a_token_straddling_a_read_boundary_is_found_at_every_offset_and_chunk_size() {
    let token = b"ghp_abcdef";
    for chunk in 1..=12 {
        for offset in 0..=30 {
            let mut data = vec![b'x'; offset];
            data.extend_from_slice(token);
            data.extend_from_slice(b"yyyy");
            assert!(
                stream_contains(data.as_slice(), &[token.to_vec()], chunk).unwrap(),
                "chunk {chunk} offset {offset}"
            );
            let mut cut = vec![b'x'; offset];
            cut.extend_from_slice(&token[..token.len() - 1]);
            cut.extend_from_slice(b"yyyy");
            assert!(
                !stream_contains(cut.as_slice(), &[token.to_vec()], chunk).unwrap(),
                "chunk {chunk} offset {offset} (token one byte short)"
            );
        }
    }
}

#[test]
fn the_scan_holds_one_chunk_and_a_pattern_tail_never_the_whole_file() {
    struct Counting {
        left: usize,
        biggest_read: usize,
    }
    impl std::io::Read for Counting {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.left);
            self.biggest_read = self.biggest_read.max(buf.len());
            buf[..n].fill(b'a');
            self.left -= n;
            Ok(n)
        }
    }
    let mut source = Counting { left: 64 * 1024 * 1024, biggest_read: 0 };
    assert!(!stream_contains(&mut source, &[b"ghp_nothere".to_vec()], SCAN_CHUNK).unwrap());
    assert_eq!(source.left, 0, "the whole input was read");
    assert_eq!(source.biggest_read, 64 * 1024, "and never more than one chunk at a time");
}

#[test]
fn an_empty_pattern_matches_a_non_empty_stream_only() {
    assert!(stream_contains(&b"x"[..], &[Vec::new()], 4).unwrap());
    assert!(!stream_contains(&b""[..], &[Vec::new()], 4).unwrap());
}

// --- B4: a host without python3 fails the checks the snippets were ---------------------------

#[test]
fn without_python3_the_wall_reads_as_invalid_json_and_the_rehearsal_count_is_empty() {
    let rig = Rig::new(&[]);
    rig.proc.lack("python3");
    let p = rig.pdir();
    rig.host.file(&format!("{p}/profile.env"), b"", false, 0);
    rig.host.file(&format!("{p}/deny.json"), b"{}", false, 0);
    let wall = b"{\"permissions\":{\"deny\":[\"Read(~/x)\"]}}";
    rig.host.file(&format!("{p}/settings.json"), wall, false, 0);
    rig.host.file(&format!("{p}/settings-rehearsal.json"), wall, false, 0);
    let mut o = out();
    install::profile_and_containment(&rig.ctx(), &mut o).unwrap();
    assert!(has_fail(&o.report, "deny wall is not valid JSON", "reinstall"));
    assert!(
        has_ok(&o.report, "rehearsal wall present,  rules (POST=off posts nothing)"),
        "{}",
        dump(&o.report)
    );
}

#[test]
fn without_python3_the_run_list_is_unknown_and_the_leases_cannot_be_read() {
    let rig = Rig::new(&[
        ("REPO_SLUG", "o/r"),
        ("REPO_PATH", ""),
        ("STATE_DIR", "/state"),
        ("GH_ACCOUNT", "a"),
    ]);
    rig.proc.lack("python3");
    rig.proc.have("gh", "/bin/gh");
    rig.proc.on(
        &[
            "gh", "run", "list", "--repo", "o/r", "--branch", "main", "--event", "push", "--limit", "40",
            "--json", "workflowName,status,conclusion,createdAt,url",
        ],
        0,
        "[{\"workflowName\":\"CI\",\"status\":\"completed\",\"conclusion\":\"success\",\"createdAt\":\"2026-01-05T00:00:00Z\"}]",
    );
    let mut o = out();
    github::repository(&rig.ctx(), &mut o).unwrap();
    assert!(has_warn(
        &o.report,
        "could not read main's workflow runs on o/r, so whether main is red is unknown",
        "gh run list --repo o/r --branch main"
    ));
    rig.host.dir("/state");
    rig.host.file("/state/leases/pr-1.json", b"{\"renewed\": 1}", false, 0);
    rig.proc.on(&["gh", "auth", "token", "--user", "a"], 1, "");
    let mut o = out();
    audit::audit_trail(&rig.ctx(), &mut o).unwrap();
    assert!(has_warn(
        &o.report,
        "could not read the leases under /state/leases, so forgotten ones are unknown",
        "python3 and a readable state directory"
    ));
}

#[test]
fn without_python3_a_failed_task_has_no_reason_recorded() {
    let rig = Rig::new(&[("STATE_DIR", "/state"), ("GH_ACCOUNT", "a")]);
    rig.proc.lack("python3");
    rig.host.file("/state/state/failed-review.json", b"{\"reason\": \"x\"}", false, 1_000);
    rig.proc.on(&["gh", "auth", "token", "--user", "a"], 1, "");
    let mut o = out();
    audit::audit_trail(&rig.ctx(), &mut o).unwrap();
    assert!(o.report.items().iter().any(
        |i| matches!(i, Item::Fail(t, _) if t.ends_with("no reason recorded in failed-review.json"))
    ));
}

#[test]
fn python3_is_asked_once_and_only_by_a_check_that_needs_it() {
    let rig = Rig::new(&[("STATE_DIR", "/nope")]);
    let mut o = out();
    audit::audit_trail(&rig.ctx(), &mut o).unwrap();
    assert!(rig.proc.argvs().is_empty(), "no snippet was reached");
    let rig = Rig::new(&[("STATE_DIR", "/state"), ("GH_ACCOUNT", "a")]);
    rig.host.file("/state/leases/pr-1.json", b"{\"renewed\": 1}", false, 0);
    rig.proc.on(&["gh", "auth", "token", "--user", "a"], 1, "");
    let mut o = out();
    audit::audit_trail(&rig.ctx(), &mut o).unwrap();
    let asks = rig.proc.argvs().iter().filter(|a| a.last().is_some_and(|l| l == "python3")).count();
    assert_eq!(asks, 1);
}

// --- B6: output is made in order, as it happens ------------------------------------------------

fn record(events: &mut Vec<(Stream, String)>) -> impl FnMut(Stream, &[u8]) -> bool + '_ {
    move |stream, bytes| {
        events.push((stream, String::from_utf8_lossy(bytes).into_owned()));
        true
    }
}

#[test]
fn a_diagnostic_lands_between_the_findings_that_surround_it_not_after_the_summary() {
    let rig = Rig::new(&[("DENY_WALL_MIN_RULES", "abc")]);
    let p = rig.pdir();
    rig.host.file(&format!("{p}/profile.env"), b"", false, 0);
    rig.host.file(&format!("{p}/deny.json"), b"{}", false, 0);
    rig.host.file(
        &format!("{p}/settings.json"),
        b"{\"permissions\":{\"deny\":[\"Read(~/x)\"]}}",
        false,
        0,
    );
    let mut events = Vec::new();
    {
        let mut emit = record(&mut events);
        let mut o = mx_doctor::ctx::Out::live(&mut emit);
        install::profile_and_containment(&rig.ctx(), &mut o).unwrap();
    }
    let at = |needle: &str| events.iter().position(|(_, t)| t.contains(needle)).unwrap();
    let present = at("profile 'magent' present");
    let note = at("integer expected");
    let wall = at("deny wall generated");
    assert!(present < note && note < wall, "{events:?}");
    assert_eq!(events[note].0, Stream::Stderr);
    assert_eq!(events[wall].0, Stream::Stdout);
}

#[test]
fn a_reader_that_has_gone_ends_the_run_with_status_141() {
    let proc = ScriptProc::default();
    let log = mx_ports::EffectLog::new();
    let clock = mx_ports::FakeClock::at(NOW, &log);
    let input = mx_doctor::RunInput {
        args: vec![],
        vars: vec![("HOME".into(), "/home/u".into())],
        tree: std::path::PathBuf::from("/tree"),
    };
    let mut gone = |_: Stream, _: &[u8]| false;
    assert_eq!(mx_doctor::run_live(&proc, &clock, &FakeHost::new(), &input, &mut gone), 141);
    let mut open = |_: Stream, _: &[u8]| true;
    assert_eq!(mx_doctor::run_live(&proc, &clock, &FakeHost::new(), &input, &mut open), 64);
}

// --- P1I-7: names that are not UTF-8 print as the bytes they have -------------------------------

#[test]
fn bytes_that_are_not_utf8_survive_a_round_trip_through_text() {
    let raw: &[u8] = b"a\xffb\xc3\xa9\xfe\xfe";
    let shown = text::lossless(raw);
    assert!(!shown.contains('\u{FFFD}'));
    assert_eq!(text::to_bytes(&shown), raw);
}

#[test]
fn the_token_leak_line_prints_the_raw_bytes_of_a_file_name_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;
    let exe = std::env::current_exe().unwrap();
    let dir = exe
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .join(format!("mx-doctor-bytes-{}", std::process::id()));
    drop(std::fs::remove_dir_all(&dir));
    std::fs::create_dir_all(&dir).unwrap();
    let name = std::ffi::OsStr::from_bytes(b"bad\xff.log");
    std::fs::write(dir.join(name), b"ghp_FAKEFAKEFAKEFAKEFAKE0000").unwrap();
    let host = RealHost::new();
    let found =
        host.scan_for(&[dir.display().to_string()], &[b"ghp_FAKEFAKEFAKEFAKEFAKE0000".to_vec()]);
    assert_eq!(found.len(), 1);
    let bytes = Report::render_bytes(&Item::fail(format!("starting {}", found[0]), ""));
    assert!(bytes.windows(8).any(|w| w == b"bad\xff.log"), "{bytes:?}");
    assert!(!bytes.windows(3).any(|w| w == [0xEF, 0xBF, 0xBD]));
    drop(std::fs::remove_dir_all(&dir));
}

// --- P1I-11: the diagnostics the bash does not silence --------------------------------------------

fn sandbox_rig() -> Rig {
    let rig = Rig::new(&[]);
    rig.proc.on(&["uname", "-s"], 0, "Linux\n");
    rig.host.file(
        &format!("{HOME}/.config/systemd/user/maintainer@.service"),
        b"[Service]\nNoNewPrivileges=yes\n",
        false,
        0,
    );
    rig.proc.have("systemd-run", "/bin/systemd-run");
    rig.proc.have("maintainer-merge", "/bin/maintainer-merge");
    rig.proc.on(&["systemd-run", "--user", "--wait", "--pipe", "--quiet", "true"], 0, "");
    rig
}

#[test]
fn a_failed_mktemp_a_failed_git_and_two_failed_mkdirs_each_print_their_diagnostic() {
    let rig = sandbox_rig();
    rig.host.fail_temp_dir();
    rig.host.fail_make_dirs("Permission denied");
    rig.proc.on_err(
        &["git", "init", "-q", "-b", "main", "/r"],
        128,
        "",
        "fatal: cannot mkdir /r: Permission denied\n",
    );
    let mut o = out();
    hostchecks::sandbox(&rig.ctx(), &mut o);
    assert_eq!(
        String::from_utf8_lossy(&o.stderr),
        "mktemp: failed to create directory via template '/tmp-case/tmp.XXXXXXXXXX': cannot create temp dir\nfatal: cannot mkdir /r: Permission denied\nmkdir: Permission denied\nmkdir: Permission denied\n"
    );
}

// --- P1I-12: no sha256sum means an empty recipe, as in the bash -------------------------------------

#[test]
fn without_sha256sum_the_recipe_is_empty_and_an_unstamped_image_reads_as_current() {
    let mut rig = Rig::new(&[]);
    rig.proc.have("podman", "/bin/podman");
    rig.proc.on(&["podman", "system", "migrate"], 0, "");
    rig.proc.on(
        &["podman", "run", "--rm", "--network=none", "docker.io/library/busybox:latest", "true"],
        0,
        "",
    );
    rig.proc.on(&["uname", "-s"], 0, "Linux\n");
    rig.proc.lack("sha256sum");
    let dir = format!("{}/verify.d", rig.pdir());
    rig.host.file(&format!("{dir}/s.sh"), b"x", false, 0);
    rig.host.file(&format!("{dir}/s.Containerfile"), b"FROM scratch\n", false, 0);
    rig.suite_env = vec![];
    let file = format!("{dir}/s.sh");
    let script = |what: &str| {
        format!(
            "set -uo pipefail; . \"$1\" >/dev/null 2>&1; declare -F suite_{what} >/dev/null && suite_{what}"
        )
    };
    rig.proc.on(&["bash", "-c", &script("image"), "_", &file], 0, "localhost/i:1");
    rig.proc.on(&["bash", "-c", &script("needs"), "_", &file], 0, "bash");
    rig.proc.on(&["podman", "image", "exists", "localhost/i:1"], 0, "");
    rig.proc.on(
        &[
            "podman",
            "image",
            "inspect",
            "--format",
            "{{index .Config.Labels \"maintainer.recipe\"}}",
            "localhost/i:1",
        ],
        0,
        "\n",
    );
    rig.proc.on(
        &[
            "podman",
            "run",
            "--rm",
            "--network=none",
            "localhost/i:1",
            "sh",
            "-c",
            "for b in \"$@\"; do command -v \"$b\" >/dev/null 2>&1 || printf \"%s \" \"$b\"; done",
            "sh",
            "bash",
        ],
        0,
        "",
    );
    rig.host.file(
        &format!("{HOME}/.config/systemd/user/podman-userns-warmup.service"),
        b"",
        false,
        0,
    );
    let mut o = out();
    hostchecks::container_verification(&rig.ctx(), &mut o);
    assert!(
        has_ok(&o.report, "verify suite 's': localhost/i:1 provides bash"),
        "{}",
        dump(&o.report)
    );
    assert!(
        !o.report.items().iter().any(|i| matches!(i, Item::Warn(..) | Item::Fail(..))),
        "an empty recipe equals an empty stamp, so nothing is said: {}",
        dump(&o.report)
    );
}

// --- Python's JSON words and createdAt kinds ----------------------------------------------------------

#[test]
fn nan_and_infinity_are_read_as_python_reads_them() {
    let nan = pyval::parse(b"{\"renewed\": NaN}").unwrap();
    assert_eq!(nan["renewed"], serde_json::json!(pyval::NAN_MARK));
    let file = |body: &str| LeaseFile {
        name: "pr-1.json".into(),
        contents: Fact::Known(body.as_bytes().to_vec()),
    };
    assert_eq!(
        forgotten_leases("12", &[file("{\"renewed\": NaN}")], NOWI),
        Ok(vec!["#1 (unreadable)".to_string()])
    );
    assert_eq!(
        forgotten_leases("12", &[file("{\"renewed\": Infinity}")], NOWI),
        Err(mx_doctor::pyval::Crash)
    );
    assert_eq!(
        forgotten_leases("12", &[file("{\"renewed\": -Infinity}")], NOWI),
        Err(mx_doctor::pyval::Crash)
    );
    assert!(pyval::parse(b"{\"a\": \"NaN inside text\"}").is_ok());
}

#[test]
fn a_nan_in_a_field_nobody_reads_does_not_make_the_run_list_unreadable() {
    let runs = b"[{\"workflowName\":\"CI\",\"status\":\"completed\",\"conclusion\":\"success\",\"createdAt\":\"2026-01-05T00:00:00Z\",\"x\":NaN}]";
    assert_eq!(github::verdicts(runs, "6", NOWI), Ok(String::new()));
}

#[test]
fn two_numeric_creation_times_compare_and_a_text_with_a_number_does_not() {
    let two = b"[{\"workflowName\":\"CI\",\"status\":\"completed\",\"conclusion\":\"success\",\"createdAt\":1},{\"workflowName\":\"CI\",\"status\":\"completed\",\"conclusion\":\"success\",\"createdAt\":2}]";
    assert_eq!(github::verdicts(two, "6", NOWI), Ok(String::new()));
    let mixed = b"[{\"workflowName\":\"CI\",\"createdAt\":1},{\"workflowName\":\"CI\",\"createdAt\":\"2026-01-05T00:00:00Z\"}]";
    assert!(github::verdicts(mixed, "6", NOWI).is_err());
}

// --- survivors of the review's mutants -------------------------------------------------------------------

#[test]
fn a_lease_file_is_read_only_when_its_name_starts_with_pr_dash() {
    let rig = Rig::new(&[("STATE_DIR", "/state"), ("GH_ACCOUNT", "a")]);
    rig.host.file("/state/leases/pronto.json", b"not json", false, 0);
    rig.proc.on(&["gh", "auth", "token", "--user", "a"], 1, "");
    let mut o = out();
    audit::audit_trail(&rig.ctx(), &mut o).unwrap();
    assert!(has_ok(&o.report, "no lease has gone 12h without a renewal"));
}

#[test]
fn only_the_exact_answer_yes_means_lingering() {
    for (answer, on) in [("yes\n", true), ("y\n", false), ("yesterday\n", false), ("no\n", false)] {
        let rig = Rig::new(&[]);
        rig.proc.on(&["uname", "-s"], 0, "Linux\n");
        rig.proc.have("systemctl", "/bin/systemctl");
        rig.proc.on(&["systemctl", "--user", "show-environment"], 0, "");
        rig.proc.on(
            &["systemctl", "--user", "list-timers", "maintainer@magent-*", "--all", "--no-pager"],
            0,
            "",
        );
        rig.proc.on(
            &["systemctl", "--user", "list-unit-files", "maintainer@magent-*.timer", "--no-pager"],
            0,
            "",
        );
        rig.proc.on(&["loginctl", "show-user", "fixture", "-p", "Linger", "--value"], 0, answer);
        let mut o = out();
        hostchecks::scheduler(&rig.ctx(), &mut o);
        assert_eq!(
            has_ok(&o.report, "lingering on: timers run without a login session"),
            on,
            "{answer:?}"
        );
    }
}

#[test]
fn an_unset_lease_limit_with_a_leases_directory_prints_line_454_then_line_463() {
    let mut rig = Rig::new(&[("STATE_DIR", "/state"), ("GH_ACCOUNT", "a")]);
    without(&mut rig, "DOCTOR_LEASE_HOURS", &[("STATE_DIR", "/state"), ("GH_ACCOUNT", "a")]);
    rig.host.dir("/state/leases");
    rig.proc.on(&["gh", "auth", "token", "--user", "a"], 1, "");
    let mut o = out();
    let stop = audit::audit_trail(&rig.ctx(), &mut o).unwrap_err();
    assert_eq!(stop, Stop::Exit(1));
    assert_eq!(
        String::from_utf8_lossy(&o.stderr),
        "maintainer-doctor: line 454: DOCTOR_LEASE_HOURS: unbound variable\nmaintainer-doctor: line 463: DOCTOR_LEASE_HOURS: unbound variable\n"
    );
}

#[test]
fn a_reader_that_is_interrupted_once_is_read_again_and_the_token_is_still_found() {
    struct Flaky {
        data: Vec<u8>,
        at: usize,
        interrupted: bool,
    }
    impl std::io::Read for Flaky {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let n = buf.len().min(self.data.len() - self.at).min(3);
            buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }
    let flaky = Flaky { data: b"xxghp_tokenyy".to_vec(), at: 0, interrupted: false };
    assert!(stream_contains(flaky, &[b"ghp_token".to_vec()], 3).unwrap());
}
