//! `new-profile` against real files, with the golden files of the characterization suite
//! as the byte-for-byte oracle.

mod common;

use common::{Sandbox, capture};
use mx_install::machine::RealMachine;
use mx_install::scaffold::new_profile;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

fn run(sb: &Sandbox, args: &[&str]) -> common::Run {
    let env = sb.env("new-profile.sh", args);
    capture(|con| new_profile(&env, &RealMachine, con))
}

fn golden(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/characterization/golden/install")
        .join(name);
    fs::read_to_string(path).unwrap()
}

/// The characterization fixture: a template with the five placeholders and a file with none.
fn fixture(sb: &Sandbox) {
    let template = sb.co.join("profiles/_template");
    fs::remove_dir_all(&template).unwrap();
    common::write(
        &template.join("profile.env"),
        "PROFILE_NAME=\"__PROFILE__\"\nREPO_PATH=\"${MAINTAINER_REPO_PATH:-__REPO_PATH__}\"\nREPO_SLUG=\"__SLUG__\"\nMAINTAINER_NAME=\"__MAINTAINER_NAME__\"\nGH_ACCOUNT=\"__GH_ACCOUNT__\"\nSTATE_DIR=\"${MAINTAINER_STATE_DIR:-$HOME/.local/state/__PROFILE__-maint}\"\nTASKS=\"review issues\"\nMIN_HOURS_review=6\nMIN_HOURS_issues=36\n",
        0o644,
    );
    common::write(
        &template.join("deny.json"),
        "{\"spelled_everywhere\": [\"gh pr merge\", \"__PROFILE__ publish\"], \"credential_paths\": [\"~/.ssh/**\"]}\n",
        0o644,
    );
    common::write(
        &template.join("prompts/review.md"),
        "Review __SLUG__ as __MAINTAINER_NAME__ (__GH_ACCOUNT__), twice: __SLUG__.\n",
        0o644,
    );
    common::write(&template.join("notes.txt"), "nothing to substitute here\n", 0o644);
}

fn norm(sb: &Sandbox, text: &str) -> String {
    text.replace(&sb.co.to_string_lossy().into_owned(), "@TREE@")
        .replace(&sb.home.to_string_lossy().into_owned(), "@HOME@")
}

#[test]
fn the_output_and_the_generated_files_are_the_golden_files_of_the_bash() {
    let sb = Sandbox::new("alpha");
    fixture(&sb);
    fs::create_dir_all(sb.home.join("code/widgets")).unwrap();
    let out = run(
        &sb,
        &[
            "alpha",
            "acme/widgets",
            &sb.home.join("code/widgets").to_string_lossy(),
            "alice",
            "Alice Example",
        ],
    );
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!(out.err, "");
    assert_eq!(norm(&sb, &out.out), golden("new-profile-alpha-stdout.txt"));
    let mut dump = String::new();
    for rel in ["deny.json", "notes.txt", "profile.env", "prompts/review.md"] {
        write!(
            dump,
            "=== ./{rel} ===\n{}",
            fs::read_to_string(sb.co.join("profiles/alpha").join(rel)).unwrap()
        )
        .unwrap();
    }
    assert_eq!(norm(&sb, &dump), golden("new-profile-alpha-profile.txt"));
    let mut units = String::new();
    for task in ["review", "issues"] {
        let name = format!("maintainer@alpha-{task}.timer");
        write!(
            units,
            "=== {name} ===\n{}",
            fs::read_to_string(sb.co.join("platform/linux").join(&name)).unwrap()
        )
        .unwrap();
    }
    assert_eq!(norm(&sb, &units), golden("new-profile-alpha-timers.txt"));
}

#[test]
fn a_hyphenated_profile_and_a_missing_name_give_the_second_golden() {
    let sb = Sandbox::new("beta");
    fixture(&sb);
    let checkout = sb.dir.join("checkout");
    fs::create_dir_all(checkout.join(".git")).unwrap();
    let out = run(&sb, &["beta-2", "org/proj-x", &checkout.to_string_lossy(), "bob"]);
    assert_eq!(out.code, 0, "{}", out.err);
    let text = out.out.replace(&sb.co.to_string_lossy().into_owned(), "@TREE@");
    assert_eq!(text, golden("new-profile-beta-stdout.txt"));
    assert!(
        fs::read_to_string(sb.co.join("profiles/beta-2/profile.env"))
            .unwrap()
            .contains("MAINTAINER_NAME=\"bob\"")
    );
}

#[test]
fn a_checkout_under_home_is_written_with_a_literal_home() {
    let sb = Sandbox::new("homepath");
    fixture(&sb);
    let path = sb.home.join("code/w");
    run(&sb, &["alpha", "a/w", &path.to_string_lossy(), "al"]);
    let text = fs::read_to_string(sb.co.join("profiles/alpha/profile.env")).unwrap();
    assert!(text.contains("REPO_PATH=\"${MAINTAINER_REPO_PATH:-$HOME/code/w}\""), "{text}");
    assert!(!text.contains(&sb.home.to_string_lossy().into_owned()));
}

#[test]
fn the_refusals_exit_with_their_status_say_their_words_and_write_nothing() {
    let cases: Vec<(Vec<&str>, i32, &str)> = vec![
        (
            vec!["_x", "a/b", "/srv/x", "acct"],
            64,
            "new-profile: '_x' is not a usable profile name\n",
        ),
        (
            vec!["a/b", "a/b", "/srv/x", "acct"],
            64,
            "new-profile: 'a/b' is not a usable profile name\n",
        ),
        (vec!["", "a/b", "/srv/x", "acct"], 64, "new-profile: '' is not a usable profile name\n"),
        (
            vec!["alpha", "widgets", "/srv/x", "acct"],
            64,
            "new-profile: 'widgets' is not owner/repo\n",
        ),
        (
            vec!["al|pha", "a/b", "/srv/x", "acct"],
            64,
            "new-profile: '|' and '&' are not allowed in an argument\n",
        ),
        (
            vec!["alpha", "a&b/c", "/srv/x", "acct"],
            64,
            "new-profile: '|' and '&' are not allowed in an argument\n",
        ),
        (
            vec!["alpha", "a/b", "/srv/x", "ac|ct"],
            64,
            "new-profile: '|' and '&' are not allowed in an argument\n",
        ),
        (
            vec!["alpha", "a/b", "/srv/x", "acct", "na&me"],
            64,
            "new-profile: '|' and '&' are not allowed in an argument\n",
        ),
        (vec!["sk", "a/b", "/srv/x", "acct"], 1, "new-profile: profiles/sk already exists\n"),
    ];
    for (args, code, want) in cases {
        let sb = Sandbox::new("refuse");
        let before = sb.listing(&sb.co);
        let out = run(&sb, &args);
        assert_eq!(out.code, code, "{args:?}");
        assert_eq!(out.err, want, "{args:?}");
        assert!(!out.out.contains("ready"));
        assert_eq!(sb.listing(&sb.co), before, "{args:?}");
    }
}

#[test]
fn fewer_than_four_arguments_print_the_eight_header_lines_and_exit_64() {
    let sb = Sandbox::new("usage");
    for args in [vec![], vec!["a", "b/c", "/x"]] {
        let out = run(&sb, &args);
        assert_eq!(out.code, 64);
        assert_eq!(out.out, "");
        assert_eq!(out.err.lines().count(), 8);
        assert!(out.err.starts_with("# Scaffold a profile for another repository.\n#\n#   ./new-profile.sh <profile> <owner/repo>"));
    }
}

#[test]
fn the_not_a_checkout_warning_comes_before_an_argument_is_refused() {
    let sb = Sandbox::new("warnfirst");
    let out = run(&sb, &["alpha", "a/b", "/nonexistent/x", "acct", "bad|name"]);
    assert_eq!(out.code, 64);
    assert_eq!(out.out, "  warning: /nonexistent/x is not a git checkout yet\n");
}

#[test]
fn an_argument_goes_to_sed_as_a_replacement_so_backslashes_mean_what_sed_says() {
    // Pins current behaviour (plan decision: preserve): \n is a newline, \\ is one backslash,
    // a $(...) stays text.
    let sb = Sandbox::new("sedtext");
    fixture(&sb);
    let out = run(&sb, &["alpha", "a/b", "/srv/w", "al", "Al\\nice $(echo hi) O'Brien\\\\x"]);
    assert_eq!(out.code, 0, "{}", out.err);
    let text = fs::read_to_string(sb.co.join("profiles/alpha/profile.env")).unwrap();
    assert!(text.contains("MAINTAINER_NAME=\"Al\nice $(echo hi) O'Brien\\x\"\n"), "{text:?}");
}

#[test]
fn an_argument_sed_refuses_ends_the_run_with_1_after_the_profile_was_copied() {
    // Pins current behaviour: `\1` is an invalid reference, and the half-made profile stays.
    let sb = Sandbox::new("sedfail");
    fixture(&sb);
    let out = run(&sb, &["alpha", "a/b", "/srv/w", "al", "x\\1y"]);
    assert_eq!(out.code, 1);
    assert_eq!(
        out.err,
        "sed: -e expression #5, char 28: invalid reference \\1 on `s' command's RHS\n"
    );
    assert!(sb.co.join("profiles/alpha/profile.env").is_file());
    assert!(
        fs::read_to_string(sb.co.join("profiles/alpha/profile.env"))
            .unwrap()
            .contains("__PROFILE__"),
        "no file was rewritten"
    );
    assert!(!sb.co.join("platform/linux/maintainer@alpha-review.timer").exists());
}

#[test]
fn a_placeholder_that_survives_leaves_the_profile_and_writes_no_timer() {
    let sb = Sandbox::new("survive");
    fixture(&sb);
    fs::write(
        sb.co.join("profiles/_template/prompts/review.md"),
        "a __SLUG__\nHello __EXTRA__ and __SLUG__\n",
    )
    .unwrap();
    let out = run(&sb, &["alpha", "acme/widgets", "/srv/w", "alice"]);
    assert_eq!(out.code, 1);
    assert_eq!(
        out.err,
        format!(
            "new-profile: a placeholder survived substitution:\n{}/profiles/alpha/prompts/review.md:2:Hello __EXTRA__ and acme/widgets\n",
            sb.co.display()
        )
    );
    assert!(sb.co.join("profiles/alpha/profile.env").is_file());
    assert!(!sb.co.join("platform/linux/maintainer@alpha-review.timer").exists());
    assert!(!out.out.contains("ready"));
}

#[test]
fn timers_are_written_one_hour_apart_from_nine_and_the_sixteenth_is_24_17() {
    let sb = Sandbox::new("hours");
    fixture(&sb);
    let words: Vec<String> = (b'a'..=b'p').map(|c| (c as char).to_string()).collect();
    let env = format!("TASKS=\"{}\"\n", words.join(" "));
    fs::write(sb.co.join("profiles/_template/profile.env"), env).unwrap();
    let out = run(&sb, &["alpha", "a/b", "/srv/w", "al"]);
    assert_eq!(out.code, 0, "{}", out.err);
    for (at, word) in words.iter().enumerate() {
        let text =
            fs::read_to_string(sb.co.join(format!("platform/linux/maintainer@alpha-{word}.timer")))
                .unwrap();
        assert!(
            text.contains(&format!("OnCalendar=*-*-* {:02}:17:00\n", 9 + at)),
            "{word}: {text}"
        );
    }
    assert!(
        out.out.contains("  wrote maintainer@alpha-a.timer\n  wrote maintainer@alpha-b.timer\n")
    );
}

#[test]
fn a_template_with_no_tasks_line_writes_no_timer_and_succeeds() {
    let sb = Sandbox::new("notasks");
    fixture(&sb);
    fs::write(sb.co.join("profiles/_template/profile.env"), "PROFILE_NAME=\"__PROFILE__\"\n")
        .unwrap();
    let out = run(&sb, &["alpha", "a/b", "/srv/w", "al"]);
    assert_eq!(out.code, 0);
    assert!(!out.out.contains("wrote "));
    assert!(out.out.contains("profiles/alpha/ is ready."));
}

#[test]
fn an_existing_timer_of_the_same_name_is_overwritten_without_a_word() {
    let sb = Sandbox::new("overwrite");
    fixture(&sb);
    fs::write(sb.co.join("platform/linux/maintainer@alpha-review.timer"), "old content\n").unwrap();
    run(&sb, &["alpha", "a/b", "/srv/w", "al"]);
    assert!(
        fs::read_to_string(sb.co.join("platform/linux/maintainer@alpha-review.timer"))
            .unwrap()
            .starts_with("[Unit]\nDescription=maintainer: alpha review (timer)\n")
    );
}

#[test]
fn the_template_is_not_changed_and_a_file_without_placeholders_is_copied_as_it_is() {
    let sb = Sandbox::new("template");
    fixture(&sb);
    let before = sb.listing(&sb.co.join("profiles/_template"));
    run(&sb, &["alpha", "a/b", "/srv/w", "al"]);
    assert_eq!(sb.listing(&sb.co.join("profiles/_template")), before);
    assert_eq!(
        fs::read(sb.co.join("profiles/alpha/notes.txt")).unwrap(),
        b"nothing to substitute here\n"
    );
}

#[test]
fn a_new_file_takes_the_mode_of_its_template_file() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new("modes");
    fixture(&sb);
    fs::set_permissions(
        sb.co.join("profiles/_template/notes.txt"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::set_permissions(
        sb.co.join("profiles/_template/profile.env"),
        fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    run(&sb, &["alpha", "a/b", "/srv/w", "al"]);
    let mode = |name: &str| {
        fs::metadata(sb.co.join("profiles/alpha").join(name)).unwrap().permissions().mode() & 0o7777
    };
    assert_eq!(mode("notes.txt"), 0o755 & !common::umask());
    assert_eq!(
        mode("profile.env"),
        0o640 & !common::umask(),
        "sed -i keeps the mode of the file it rewrites"
    );
}
