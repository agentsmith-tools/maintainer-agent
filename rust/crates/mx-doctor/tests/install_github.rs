//! Deployment, Profile and containment, Backend, Commands, GitHub identity,
//! Posting, Repository and the workflow runs on main.

mod common;

use common::*;
use mx_core::Fact;
use mx_doctor::github::{self, RepoFacts};
use mx_doctor::host::HostFs;
use mx_doctor::install::{self, rule_count};
use mx_doctor::report::Item;

fn settings_json(n: usize) -> String {
    let mut rules: Vec<String> = (0..n - 1).map(|i| format!("Bash(rule{i}:*)")).collect();
    rules.push("Read(~/.ssh/**)".to_string());
    format!("{{\"permissions\":{{\"deny\":{rules:?}}}}}")
}

fn healthy_profile(rig: &Rig) {
    let p = rig.pdir();
    rig.host.file(&format!("{p}/profile.env"), b"", false, 0);
    rig.host.file(&format!("{p}/deny.json"), b"{}", false, 0);
    rig.host.file(&format!("{p}/settings.json"), settings_json(120).as_bytes(), false, 0);
    rig.host.file(&format!("{p}/settings-rehearsal.json"), settings_json(120).as_bytes(), false, 0);
}

// --- Deployment ---------------------------------------------------------------

#[test]
fn a_missing_share_directory_is_not_installed_and_run_scripts_are_missing() {
    let rig = Rig::new(&[]);
    rig.proc.on(&["uname", "-s"], 0, "Linux\n");
    let mut o = out();
    install::deployment(&rig.ctx(), &mut o);
    assert!(has_fail(&o.report, "not installed", "run ./install.sh from the repository"));
    assert!(has_fail(&o.report, "run.sh missing", "./install.sh"));
    assert!(has_fail(&o.report, "run-instance.sh missing", "./install.sh"));
    assert!(has_fail(
        &o.report,
        "/home/u/.local/bin/maintainer missing or not executable",
        "./install.sh"
    ));
}

#[test]
fn a_missing_run_instance_script_is_fine_on_any_system_but_linux_and_names_it() {
    let rig = Rig::new(&[]);
    rig.host.file(&format!("{SHARE}/run.sh"), b"", true, 0);
    rig.proc.on(&["uname", "-s"], 0, "FreeBSD\n");
    let mut o = out();
    install::deployment(&rig.ctx(), &mut o);
    assert!(has_ok(&o.report, "run-instance.sh not needed on FreeBSD"));
}

#[test]
fn run_sh_without_the_execute_bit_fails_with_the_chmod_fix() {
    let rig = Rig::new(&[]);
    rig.host.file(&format!("{SHARE}/run.sh"), b"", false, 0);
    rig.host.file(&format!("{SHARE}/run-instance.sh"), b"", true, 0);
    rig.proc.on(&["uname", "-s"], 0, "Linux\n");
    let mut o = out();
    install::deployment(&rig.ctx(), &mut o);
    assert!(has_fail(
        &o.report,
        "run.sh is NOT executable",
        &format!("chmod +x {SHARE}/run.sh, then fix install.sh")
    ));
    assert!(has_ok(&o.report, "run-instance.sh is executable"));
}

#[test]
fn more_than_two_profiles_or_backends_directories_read_as_a_nested_reinstall() {
    let rig = Rig::new(&[]);
    for dir in ["profiles", "backends", "profiles/x/profiles"] {
        rig.host.dir(&format!("{SHARE}/{dir}"));
    }
    rig.proc.on(&["uname", "-s"], 0, "Linux\n");
    let mut o = out();
    install::deployment(&rig.ctx(), &mut o);
    assert!(has_fail(
        &o.report,
        "3 profiles/backends dirs; a reinstall nested them",
        &format!("rm -rf {SHARE} && ./install.sh")
    ));
}

#[test]
fn two_profiles_and_backends_directories_are_the_healthy_count() {
    let rig = Rig::new(&[]);
    rig.host.dir(&format!("{SHARE}/profiles"));
    rig.host.dir(&format!("{SHARE}/backends"));
    rig.proc.on(&["uname", "-s"], 0, "Linux\n");
    let mut o = out();
    install::deployment(&rig.ctx(), &mut o);
    assert!(has_ok(&o.report, "no nested profiles/ or backends/"));
}

#[test]
fn uname_is_asked_only_where_a_missing_run_instance_script_needs_it() {
    let rig = Rig::new(&[]);
    rig.host.file(&format!("{SHARE}/run.sh"), b"", true, 0);
    rig.host.file(&format!("{SHARE}/run-instance.sh"), b"", true, 0);
    let mut o = out();
    install::deployment(&rig.ctx(), &mut o);
    assert!(rig.proc.tool_calls().is_empty(), "{:?}", rig.proc.tool_calls());
}

// --- Profile and containment ----------------------------------------------------

fn wall_lines(rig: &Rig) -> (mx_doctor::ctx::Out, Result<(), mx_doctor::ctx::Stop>) {
    let mut o = out();
    let r = install::profile_and_containment(&rig.ctx(), &mut o);
    (o, r)
}

#[test]
fn a_wall_of_exactly_the_floor_passes_and_one_rule_less_fails() {
    for (n, passes) in [(100, true), (99, false), (101, true)] {
        let rig = Rig::new(&[]);
        healthy_profile(&rig);
        rig.host.file(
            &format!("{}/settings.json", rig.pdir()),
            settings_json(n).as_bytes(),
            false,
            0,
        );
        let (o, r) = wall_lines(&rig);
        assert!(r.is_ok());
        if passes {
            assert!(has_ok(&o.report, &format!("deny wall generated, {n} rules")), "n={n}");
        } else {
            assert!(
                has_fail(
                    &o.report,
                    &format!("deny wall has only {n} rules, under the floor of 100; it is stale"),
                    &format!("rm -rf {SHARE} && ./install.sh")
                ),
                "n={n}"
            );
        }
    }
}

#[test]
fn a_floor_that_is_not_a_number_prints_the_test_error_and_lets_the_wall_pass() {
    let rig = Rig::new(&[("DENY_WALL_MIN_RULES", "abc")]);
    healthy_profile(&rig);
    rig.host.file(&format!("{}/settings.json", rig.pdir()), settings_json(3).as_bytes(), false, 0);
    let (o, _) = wall_lines(&rig);
    assert!(has_ok(&o.report, "deny wall generated, 3 rules"));
    assert_eq!(
        String::from_utf8_lossy(&o.stderr),
        "maintainer-doctor: line 92: [: abc: integer expected\n"
    );
}

#[test]
fn an_unset_floor_ends_the_run_with_the_unbound_variable_status() {
    let mut rig = Rig::new(&[]);
    rig.settings = mx_doctor::ctx::Settings::new(std::collections::BTreeMap::new());
    healthy_profile(&rig);
    let (o, r) = wall_lines(&rig);
    assert_eq!(r, Err(mx_doctor::ctx::Stop::Exit(127)));
    assert_eq!(
        String::from_utf8_lossy(&o.stderr),
        "maintainer-doctor: line 92: DENY_WALL_MIN_RULES: unbound variable\n"
    );
}

#[test]
fn an_unparseable_wall_fails_twice_as_invalid_json_and_as_lacking_the_home_rule() {
    let rig = Rig::new(&[]);
    healthy_profile(&rig);
    rig.host.file(&format!("{}/settings.json", rig.pdir()), b"not json\n", false, 0);
    let (o, _) = wall_lines(&rig);
    assert!(has_fail(&o.report, "deny wall is not valid JSON", "reinstall"));
    assert!(has_fail(
        &o.report,
        "no Read(~/...) rule; credential paths are not protected",
        "check credential_paths in deny.json and re-run ./install.sh"
    ));
}

#[test]
fn an_unrendered_home_placeholder_fails_even_when_the_rule_count_is_fine() {
    let rig = Rig::new(&[]);
    healthy_profile(&rig);
    let text = settings_json(120).replace("Bash(rule0:*)", "Read(__HOME__/.ssh/**)");
    rig.host.file(&format!("{}/settings.json", rig.pdir()), text.as_bytes(), false, 0);
    let (o, _) = wall_lines(&rig);
    assert!(has_fail(&o.report, "unrendered __HOME__ left in the deny wall", "reinstall"));
    assert!(has_ok(&o.report, "deny wall generated, 120 rules"));
}

#[test]
fn a_wall_with_only_an_absolute_read_rule_denies_nothing_and_fails() {
    let rig = Rig::new(&[]);
    healthy_profile(&rig);
    rig.host.file(
        &format!("{}/settings.json", rig.pdir()),
        b"{\"permissions\":{\"deny\":[\"Read(/home/u/.ssh/**)\"]}}",
        false,
        0,
    );
    let (o, _) = wall_lines(&rig);
    assert!(has_fail(
        &o.report,
        "no Read(~/...) rule; credential paths are not protected",
        "check credential_paths in deny.json and re-run ./install.sh"
    ));
}

#[test]
fn an_old_template_a_missing_deny_json_and_a_missing_wall_each_end_the_wall_checks() {
    let rig = Rig::new(&[]);
    healthy_profile(&rig);
    rig.host.file(&format!("{}/settings.json.template", rig.pdir()), b"{}", false, 0);
    let (o, _) = wall_lines(&rig);
    assert!(has_fail(
        &o.report,
        "an old settings.json.template is still here",
        "rm it and re-run ./install.sh; walls are generated from deny.json now"
    ));
    assert!(!has_ok(&o.report, "deny wall generated, 120 rules"));

    let rig = Rig::new(&[]);
    rig.host.file(&format!("{}/profile.env", rig.pdir()), b"", false, 0);
    let (o, _) = wall_lines(&rig);
    assert!(has_fail(
        &o.report,
        "no deny.json for 'magent'",
        "every profile needs one; copy profiles/_template/deny.json"
    ));

    let rig = Rig::new(&[]);
    rig.host.file(&format!("{}/profile.env", rig.pdir()), b"", false, 0);
    rig.host.file(&format!("{}/deny.json", rig.pdir()), b"{}", false, 0);
    let (o, _) = wall_lines(&rig);
    assert!(has_fail(&o.report, "no deny wall at all", "./install.sh"));
}

#[test]
fn an_unparseable_rehearsal_wall_still_prints_ok_with_an_empty_count() {
    let rig = Rig::new(&[]);
    healthy_profile(&rig);
    rig.host.file(&format!("{}/settings-rehearsal.json", rig.pdir()), b"nope", false, 0);
    let (o, _) = wall_lines(&rig);
    assert!(has_ok(&o.report, "rehearsal wall present,  rules (POST=off posts nothing)"));
}

#[test]
fn a_missing_rehearsal_wall_and_a_deployed_template_are_warnings() {
    let rig = Rig::new(&[]);
    healthy_profile(&rig);
    rig.host.dir(&format!("{SHARE}/profiles/_template"));
    rig.host.remove_tree(&format!("{}/settings-rehearsal.json", rig.pdir()));
    let (o, _) = wall_lines(&rig);
    assert!(has_warn(
        &o.report,
        "no rehearsal wall; POST=off would refuse to start",
        "./install.sh generates it"
    ));
    assert!(has_warn(
        &o.report,
        "the scaffolding template was deployed",
        "harmless, but ./install.sh should have skipped it"
    ));
}

#[test]
fn the_rule_count_reads_a_list_a_dict_or_a_string_and_nothing_else() {
    let known = |text: &str| rule_count(&Fact::Known(text.as_bytes().to_vec()));
    assert_eq!(known("{\"permissions\":{\"deny\":[1,2,3]}}"), Some(3));
    assert_eq!(known("{\"permissions\":{\"deny\":{\"a\":1}}}"), Some(1));
    assert_eq!(known("{\"permissions\":{\"deny\":\"abcd\"}}"), Some(4));
    assert_eq!(known("{\"permissions\":{\"deny\":5}}"), None);
    assert_eq!(known("{\"permissions\":[1]}"), None);
    assert_eq!(known("[]"), None);
    assert_eq!(rule_count(&Fact::Unreadable("denied".into())), None);
    assert_eq!(rule_count(&Fact::Absent), None);
}

// --- Backend and commands ---------------------------------------------------------

#[test]
fn claude_is_run_once_with_version_and_only_its_first_line_is_shown() {
    let rig = Rig::new(&[]);
    rig.host.file(&format!("{SHARE}/backends/claude.sh"), b"", false, 0);
    rig.proc.have("claude", "/bin/claude");
    rig.proc.on(&["claude", "--version"], 0, "2.0.0 (Claude Code)\nsecond line\n");
    let mut o = out();
    install::backend(&rig.ctx(), &mut o);
    assert!(has_ok(&o.report, "backend 'claude' installed"));
    assert!(has_ok(&o.report, "claude on PATH (2.0.0 (Claude Code))"));
    assert_eq!(rig.proc.tool_calls(), ["claude --version"]);
}

#[test]
fn each_backend_checks_its_own_binary_and_names_its_own_fix() {
    for (backend, binary, fix) in [
        ("codex", "codex", "npm i -g @openai/codex"),
        ("cursor", "cursor-agent", "install the Cursor CLI"),
        ("opencode", "opencode", "curl -fsSL https://opencode.ai/install | bash"),
    ] {
        let rig = Rig::new(&[("BACKEND", backend)]);
        rig.proc.lack(binary);
        let mut o = out();
        install::backend(&rig.ctx(), &mut o);
        assert!(has_fail(&o.report, &format!("backend '{backend}' not installed"), "./install.sh"));
        assert!(has_fail(&o.report, &format!("{binary} not on PATH"), fix), "{backend}");
        assert!(rig.proc.tool_calls().is_empty());
    }
}

#[test]
fn an_unknown_backend_with_a_script_is_ok_and_gets_no_path_check() {
    let rig = Rig::new(&[("BACKEND", "weird")]);
    rig.host.file(&format!("{SHARE}/backends/weird.sh"), b"", false, 0);
    let mut o = out();
    install::backend(&rig.ctx(), &mut o);
    assert_eq!(o.report.counts(), (1, 0, 0));
    assert!(rig.proc.argvs().is_empty());
}

#[test]
fn an_empty_required_command_list_falls_back_to_the_four_defaults_in_order() {
    let rig = Rig::new(&[("REQUIRED_COMMANDS", "")]);
    for name in ["maintainer", "maintainer-merge", "gh", "git"] {
        rig.proc.have(name, "/bin/x");
    }
    let mut o = out();
    install::commands(&rig.ctx(), &mut o);
    let names: Vec<String> = rig.proc.argvs().iter().map(|argv| argv[4].clone()).collect();
    assert_eq!(names, ["maintainer", "maintainer-merge", "gh", "git"]);
    assert_eq!(o.report.counts(), (4, 0, 0));
}

#[test]
fn a_command_that_does_not_resolve_fails_and_the_others_still_print() {
    let rig = Rig::new(&[("REQUIRED_COMMANDS", "gh  frobnicate\tgit")]);
    rig.proc.have("gh", "/bin/gh");
    rig.proc.lack("frobnicate");
    rig.proc.have("git", "/bin/git");
    let mut o = out();
    install::commands(&rig.ctx(), &mut o);
    assert_eq!(
        o.report.items()[1..],
        [
            Item::ok("gh resolves"),
            Item::fail(
                "frobnicate is named in the prompts and is NOT on PATH",
                "install it, or remove it from the prompts and REQUIRED_COMMANDS"
            ),
            Item::ok("git resolves"),
        ]
    );
}

// --- GitHub identity and posting ---------------------------------------------------

fn identity(who: Option<(i32, &str)>, want: &str) -> mx_doctor::ctx::Out {
    let rig = Rig::new(&[("GH_ACCOUNT", want)]);
    rig.proc.have("gh", "/bin/gh");
    if let Some((code, text)) = who {
        rig.proc.on(&["gh", "api", "user", "--jq", ".login"], code, text);
    }
    let mut o = out();
    github::identity(&rig.ctx(), &mut o);
    o
}

#[test]
fn the_account_match_ignores_case_and_prints_the_name_gh_reports() {
    let o = identity(Some((0, "TestUser\n")), "testuser");
    assert!(has_ok(&o.report, "gh is TestUser"));
}

#[test]
fn a_rejected_token_is_named_as_such_and_never_as_the_account() {
    let o = identity(Some((1, "{\"message\":\"Bad credentials\"}\n")), "testuser");
    assert!(has_fail(
        &o.report,
        "GitHub rejects the stored token for testuser (HTTP 401)",
        "gh auth login -h github.com, signed in as testuser"
    ));
    assert_eq!(o.report.counts().2, 1);
}

#[test]
fn a_failing_call_and_an_empty_login_both_read_as_not_authenticated() {
    for answer in [(1, "boom\n"), (0, "")] {
        let o = identity(Some(answer), "testuser");
        assert!(has_fail(&o.report, "gh is not authenticated", "gh auth login"), "{answer:?}");
    }
}

#[test]
fn a_different_active_account_is_a_warning_and_a_missing_account_name_is_a_failure() {
    let o = identity(Some((0, "someone\n")), "testuser");
    assert!(has_warn(
        &o.report,
        "gh is 'someone', not 'testuser'",
        "run.sh switches automatically, but check ~/.config/gh/hosts.yml"
    ));
    let o = identity(Some((0, "someone\n")), "");
    assert!(has_fail(
        &o.report,
        "the profile names no GH_ACCOUNT",
        "set it in profile.env; a run refuses to start without one"
    ));
}

#[test]
fn without_gh_the_identity_section_fails_and_makes_no_call() {
    let rig = Rig::new(&[]);
    rig.proc.lack("gh");
    let mut o = out();
    github::identity(&rig.ctx(), &mut o);
    assert!(has_fail(&o.report, "gh not installed", "install the GitHub CLI"));
    assert!(rig.proc.tool_calls().is_empty());
}

#[test]
fn any_post_value_holding_off_in_lower_case_reads_as_off() {
    for (value, off) in [("off", true), ("xoffx", true), ("OFF", false), ("on", false)] {
        let rig = Rig::new(&[("POST", value), ("GH_ACCOUNT", "me")]);
        let mut o = out();
        github::posting(&rig.ctx(), &mut o);
        assert_eq!(
            has_ok(
                &o.report,
                "POST=off: this profile writes reports and drafts, and posts nothing"
            ),
            off,
            "{value}"
        );
    }
}

#[test]
fn an_empty_post_value_means_on_and_names_a_question_mark_without_an_account() {
    let rig = Rig::new(&[("POST", "")]);
    let mut o = out();
    github::posting(&rig.ctx(), &mut o);
    assert!(has_warn(
        &o.report,
        "POST=on: this profile writes to GitHub as ?",
        "set POST=off in profile.env for a rehearsal week"
    ));
}

// --- Repository ------------------------------------------------------------------------

#[test]
fn the_repository_decisions_follow_the_dirty_and_ahead_counts() {
    let items = github::decide_repo(&RepoFacts { dirty: 3, ahead: "2".into() });
    assert_eq!(
        items,
        [
            Item::warn("3 uncommitted change(s); a run will refuse to start", "commit or stash"),
            Item::warn("2 commit(s) ahead of origin/main", "push or reset"),
        ]
    );
    let items = github::decide_repo(&RepoFacts { dirty: 0, ahead: "0".into() });
    assert_eq!(items, [Item::ok("working tree clean"), Item::ok("not ahead of origin/main")]);
}

#[test]
fn a_profile_with_no_repo_path_fails_and_a_missing_directory_fails_differently() {
    let rig = Rig::new(&[("REPO_PATH", "")]);
    let mut o = out();
    github::repository(&rig.ctx(), &mut o).unwrap();
    assert!(has_fail(
        &o.report,
        "the profile names no REPO_PATH",
        "set it in profiles/magent/profile.env"
    ));

    let rig = Rig::new(&[("REPO_PATH", "/nowhere")]);
    let mut o = out();
    github::repository(&rig.ctx(), &mut o).unwrap();
    assert!(has_fail(
        &o.report,
        "no repository at /nowhere",
        "clone it or set REPO_PATH in profile.env"
    ));
}

#[test]
fn a_repository_reads_its_status_and_its_distance_from_origin_main_through_git() {
    let rig = Rig::new(&[("REPO_PATH", "/r")]);
    rig.host.dir("/r/.git");
    rig.proc.on(&["git", "-C", "/r", "status", "--porcelain"], 0, "?? a\n?? b\n");
    rig.proc.on(&["git", "-C", "/r", "rev-list", "--count", "origin/main..HEAD"], 0, "4\n");
    let mut o = out();
    github::repository(&rig.ctx(), &mut o).unwrap();
    assert!(has_ok(&o.report, "repo at /r"));
    assert!(has_warn(
        &o.report,
        "2 uncommitted change(s); a run will refuse to start",
        "commit or stash"
    ));
    assert!(has_warn(&o.report, "4 commit(s) ahead of origin/main", "push or reset"));
}

#[test]
fn a_failing_rev_list_reads_as_not_ahead() {
    let rig = Rig::new(&[("REPO_PATH", "/r")]);
    rig.host.dir("/r/.git");
    rig.proc.on(&["git", "-C", "/r", "status", "--porcelain"], 0, "");
    rig.proc.on(&["git", "-C", "/r", "rev-list", "--count", "origin/main..HEAD"], 128, "");
    let mut o = out();
    github::repository(&rig.ctx(), &mut o).unwrap();
    assert!(has_ok(&o.report, "not ahead of origin/main"));
}

// --- Workflow runs on main ---------------------------------------------------------------

const NOW_I: i64 = 1_767_614_400;

fn run_row(
    name: &str,
    status: &str,
    conclusion: &str,
    hours_ago_x100: i64,
    url: Option<&str>,
) -> serde_json::Value {
    let at = NOW_I - hours_ago_x100 * 36;
    let ts = format_iso(at);
    let mut row = serde_json::json!({"workflowName": name, "status": status, "conclusion": conclusion, "createdAt": ts});
    if let Some(url) = url {
        row["url"] = serde_json::json!(url);
    }
    row
}

fn format_iso(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    // 2026-01-05 is day 20458.
    let day_of_january = days - 20_454 + 1;
    format!(
        "2026-01-{day_of_january:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

fn verdicts(rows: &[serde_json::Value], grace: &str) -> Result<String, ()> {
    github::verdicts(serde_json::Value::Array(rows.to_vec()).to_string().as_bytes(), grace, NOW_I)
        .map_err(|_| ())
}

#[test]
fn a_failure_exactly_at_the_grace_is_red_and_one_minute_inside_it_is_not() {
    let at = [run_row("CI", "completed", "failure", 900, Some("u"))];
    assert_eq!(verdicts(&at, "9").unwrap(), "red\tCI\t9\tu");
    let inside = [run_row("CI", "completed", "failure", 898, Some("u"))];
    assert_eq!(verdicts(&inside, "9").unwrap(), "");
}

#[test]
fn only_the_newest_run_of_each_workflow_counts_and_workflows_print_in_name_order() {
    let rows = [
        run_row("Zeta", "completed", "failure", 1000, Some("z")),
        run_row("Alpha", "completed", "failure", 1100, Some("a")),
        run_row("Mid", "completed", "failure", 1200, Some("m")),
        run_row("Mid", "completed", "success", 300, None),
    ];
    assert_eq!(verdicts(&rows, "6").unwrap(), "red\tAlpha\t11\ta\nred\tZeta\t10\tz");
}

#[test]
fn a_run_that_is_not_completed_or_not_a_failure_is_never_red() {
    for (status, conclusion) in
        [("in_progress", ""), ("queued", "failure"), ("completed", "cancelled")]
    {
        let rows = [run_row("CI", status, conclusion, 3000, None)];
        assert_eq!(verdicts(&rows, "6").unwrap(), "", "{status} {conclusion}");
    }
}

#[test]
fn an_empty_list_an_object_and_garbage_are_unknown_and_never_not_red() {
    assert_eq!(github::verdicts(b"[]", "6", NOW_I), Ok("unreadable".to_string()));
    assert_eq!(github::verdicts(b"{}", "6", NOW_I), Ok("unreadable".to_string()));
    assert!(github::verdicts(b"garbage", "6", NOW_I).is_err());
}

#[test]
fn the_snippet_crashes_on_a_naive_time_a_bad_grace_and_a_run_that_is_not_an_object() {
    let mut naive = run_row("CI", "completed", "failure", 1000, None);
    naive["createdAt"] = serde_json::json!("2026-01-04T00:00:00");
    assert!(verdicts(&[naive], "6").is_err());
    let rows = [run_row("CI", "completed", "failure", 1000, None)];
    assert!(verdicts(&rows, "abc").is_err());
    assert!(github::verdicts(b"[1]", "6", NOW_I).is_err());
}

#[test]
fn a_bad_grace_is_not_read_until_a_failed_newest_run_needs_it() {
    let rows = [run_row("CI", "completed", "success", 1000, None)];
    assert_eq!(verdicts(&rows, "abc").unwrap(), "");
}

#[test]
fn a_run_with_no_workflow_name_prints_as_a_question_mark_and_no_url_leaves_the_fix_text_bare() {
    let mut row = run_row("x", "completed", "failure", 1000, None);
    row["workflowName"] = serde_json::json!(null);
    assert_eq!(verdicts(&[row], "6").unwrap(), "red\t?\t10\t");
}

#[test]
fn the_comparison_of_creation_times_runs_only_between_runs_of_one_workflow() {
    let mut first = run_row("A", "completed", "success", 1000, None);
    first.as_object_mut().unwrap().remove("createdAt");
    assert_eq!(verdicts(&[first.clone()], "6").unwrap(), "", "a single run needs no comparison");
    let second = run_row("A", "completed", "success", 500, None);
    assert!(
        verdicts(&[first, second], "6").is_err(),
        "two runs of one workflow compare their times"
    );
}

#[test]
fn the_runs_section_asks_gh_for_main_push_runs_with_the_configured_limit_and_prints_red_ones() {
    let rig = Rig::new(&[("REPO_SLUG", "o/r"), ("DOCTOR_RUN_LIMIT", "7"), ("REPO_PATH", "")]);
    rig.proc.have("gh", "/bin/gh");
    let rows = serde_json::Value::Array(vec![run_row(
        "CI",
        "completed",
        "failure",
        1000,
        Some("https://x/1"),
    )]);
    rig.proc.on(
        &[
            "gh",
            "run",
            "list",
            "--repo",
            "o/r",
            "--branch",
            "main",
            "--event",
            "push",
            "--limit",
            "7",
            "--json",
            "workflowName,status,conclusion,createdAt,url",
        ],
        0,
        &rows.to_string(),
    );
    let mut o = out();
    github::repository(&rig.ctx(), &mut o).unwrap();
    assert!(has_fail(
        &o.report,
        "main's latest 'CI' run failed 10h ago and main has been red since",
        "read the failing job: https://x/1"
    ));
}

#[test]
fn a_failing_gh_run_list_is_a_warning_that_main_s_state_is_unknown() {
    let rig = Rig::new(&[("REPO_SLUG", "o/r"), ("REPO_PATH", "")]);
    rig.proc.have("gh", "/bin/gh");
    rig.proc.on(
        &[
            "gh",
            "run",
            "list",
            "--repo",
            "o/r",
            "--branch",
            "main",
            "--event",
            "push",
            "--limit",
            "40",
            "--json",
            "workflowName,status,conclusion,createdAt,url",
        ],
        1,
        "",
    );
    let mut o = out();
    github::repository(&rig.ctx(), &mut o).unwrap();
    assert!(has_warn(
        &o.report,
        "could not read main's workflow runs on o/r, so whether main is red is unknown",
        "gh run list --repo o/r --branch main"
    ));
}

#[test]
fn a_profile_with_no_slug_skips_the_runs_check_without_asking_gh() {
    let rig = Rig::new(&[("REPO_PATH", "")]);
    let mut o = out();
    github::repository(&rig.ctx(), &mut o).unwrap();
    assert!(rig.proc.argvs().is_empty());
}

#[test]
fn a_workflow_name_may_be_a_number_but_a_text_and_a_number_together_crash_the_snippet() {
    let mut numeric = run_row("x", "completed", "failure", 1000, Some("u"));
    numeric["workflowName"] = serde_json::json!(7);
    assert_eq!(verdicts(&[numeric.clone()], "6").unwrap(), "red\t7\t10\tu");
    let text = run_row("CI", "completed", "success", 1000, None);
    assert!(verdicts(&[numeric, text], "6").is_err());
    let mut listed = run_row("x", "completed", "failure", 1000, None);
    listed["workflowName"] = serde_json::json!(["a"]);
    assert!(verdicts(&[listed], "6").is_err(), "a list cannot key a dict");
}
