//! The two refusals of the sandbox that the bash prints in `cmd_verify`, compared with the
//! text of `bin/maintainer-merge` itself, so a change to either side turns a test red.

use mx_sandbox::SandboxError;
use std::path::Path;

fn bash_source() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../bin/maintainer-merge");
    std::fs::read_to_string(path).unwrap()
}

/// The literal between `start` and the next `"` that ends a bash string.
fn literal(source: &str, start: &str) -> String {
    let from = source.find(start).unwrap_or_else(|| panic!("no {start:?} in bin/maintainer-merge"));
    let body = &source[from + start.len()..];
    body[..body.find("\"\n").unwrap()].to_string()
}

#[test]
fn an_unchanged_tree_refuses_with_the_die_text_of_cmd_verify_byte_for_byte() {
    let text = literal(&bash_source(), "die \"the mutation changed nothing");
    let want = format!("the mutation changed nothing{text}")
        .replace("$(suite_mutate_glob | tr '\\n' ' ')", "*.sh *.py")
        .replace("$pr", "7");
    let got =
        SandboxError::NothingChanged { globs: vec!["*.sh".into(), "*.py".into()] }.message("7");
    assert_eq!(got, want);
}

#[test]
fn a_tree_git_cannot_index_prints_the_note_of_cmd_verify_byte_for_byte() {
    let text = literal(&bash_source(), "say \"note: ");
    // `say` adds two spaces before the first line.
    assert_eq!(SandboxError::IndexNotSeeded.to_string(), format!("note: {text}"));
}

#[test]
fn every_other_error_prints_the_same_with_or_without_a_pull_request() {
    let e = SandboxError::ReadTree { rev: "v-7".into() };
    assert_eq!(e.message("7"), e.to_string());
}
