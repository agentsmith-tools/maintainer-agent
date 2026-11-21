//! `backend_by_name`: the four names and nothing else (plan decision D18).

use mx_backends::backend_by_name;

const NAMES: [&str; 4] = ["claude", "codex", "cursor", "opencode"];

#[test]
fn the_four_shipped_names_find_a_backend_that_reports_the_same_name() {
    let mut found = 0;
    for name in NAMES {
        let backend = backend_by_name(name).unwrap_or_else(|| panic!("no backend {name}"));
        assert_eq!(backend.name(), name);
        found += 1;
    }
    assert_eq!(found, 4);
}

#[test]
fn every_other_string_finds_no_backend() {
    let refused = [
        "",
        " ",
        "codex.sh",
        "claude.sh",
        "../x",
        "..",
        "../backends/codex",
        "claude/../codex",
        "claude/",
        "/claude",
        "./claude",
        "backends/claude",
        "claude\ncodex",
        "claude\0codex",
        "cursor-agent",
        "open",
        "opencode/..",
        "nosuch",
        "all",
        "*",
        "claude*",
        "c*",
        "[c]laude",
        "c",
        "co",
        "cod",
        "clau",
    ];
    let mut checked = 0;
    for name in refused {
        assert!(backend_by_name(name).is_none(), "{name:?} must not name a backend");
        checked += 1;
    }
    assert_eq!(checked, 27, "every refused name must be tried");
}

#[test]
fn a_shipped_name_with_anything_added_or_changed_finds_no_backend() {
    let mut checked = 0;
    for name in NAMES {
        let upper = name.to_uppercase();
        let mut title = name.to_string();
        title.get_mut(0..1).expect("a first letter").make_ascii_uppercase();
        let variants = [
            format!(" {name}"),
            format!("{name} "),
            format!("\t{name}"),
            format!("{name}\t"),
            format!("{name}\n"),
            format!("\n{name}"),
            format!("{name}\r"),
            format!("{name}\0"),
            format!("\0{name}"),
            format!("{name}x"),
            format!("x{name}"),
            format!("{name}.sh"),
            format!("{name}/"),
            format!("/{name}"),
            format!("{name}/.."),
            format!("../{name}"),
            format!("{name}{name}"),
            upper,
            title,
            name[..name.len() - 1].to_string(),
            name[1..].to_string(),
        ];
        for variant in variants {
            assert!(backend_by_name(&variant).is_none(), "{variant:?} must not name a backend");
            checked += 1;
        }
    }
    assert_eq!(checked, 4 * 21, "every variant of every name must be tried");
}

#[test]
fn a_name_that_starts_or_ends_like_a_backend_is_not_that_backend() {
    for name in [
        "claudex",
        "codexx",
        "cursors",
        "opencodes",
        "claudecode",
        "codex-cli",
        "xclaude",
        "mycodex",
        "-cursor",
        "reopencode",
    ] {
        assert!(backend_by_name(name).is_none(), "{name} matched by prefix or suffix");
    }
}
