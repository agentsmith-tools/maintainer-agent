//! The prompt of `lib/run.sh`, assembled byte for byte.
//!
//! `--show-prompt` and a real run build the prompt through [`assemble`], so the
//! preview is a preview of the agent that runs and not of a second assembly.
//!
//! The order is the bash's, and each part has a reason:
//!
//! 0. The provenance line, first, because the doctrine tells the agent to open
//!    its report with the build that wrote it and this is the only place that
//!    build appears.
//! 1. The profile's own preamble: who this profile is and what it may do.
//! 2. The doctrine, shared by every profile so it cannot drift between repos,
//!    with `__MAINTAINER__` and `__SLUG__` replaced.
//! 3. The prose discipline, unless the profile says `raw`: anything other than an
//!    explicit `raw` gets it, so a misspelled value still writes well.
//! 4. The rehearsal or the posting section, last before the task so it overrides
//!    anything above it.
//! 5. The task, then what changed since the task last ran.
//!
//! # The substitution
//!
//! The bash runs `sed -e "s|__MAINTAINER__|$name|g" -e "s|__SLUG__|$slug|g"` on
//! the doctrine. That call stays a call: [`sed_arguments`] builds the same
//! argument vector, and the lifecycle runs the real `sed` through the `Proc`
//! port, so every quirk of its replacement syntax is the bash's, not a copy.
//! The maintainer's name is escaped first ([`escape_for_sed`]); the slug is not,
//! as in the bash, so a slug holding `&`, `|` or a backslash corrupts the
//! doctrine (decision D16, preserved and pinned).

/// What goes into the prompt, already read from disk and from the helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptParts<'a> {
    /// `MAINTAINER_VERSION`.
    pub version: &'a [u8],
    /// `MAINTAINER_COMMIT`.
    pub commit: &'a [u8],
    /// The profile name.
    pub profile: &'a str,
    /// The task.
    pub task: &'a str,
    /// `backend_name`.
    pub backend: &'a str,
    /// `date -Is` at assembly time.
    pub started: &'a [u8],
    /// `profiles/<p>/prompts/common-preamble.md`.
    pub common: &'a [u8],
    /// The doctrine after the `sed` substitution.
    pub core: &'a [u8],
    /// `prose-style.md` when the profile does not say `raw` and the file exists.
    pub prose: Option<&'a [u8]>,
    /// `POST`.
    pub post: &'a str,
    /// `REPO_SLUG`.
    pub repo_slug: &'a str,
    /// `GH_ACCOUNT`.
    pub gh_account: &'a str,
    /// `profiles/<p>/prompts/<task>.md`.
    pub task_prompt: &'a [u8],
    /// What `maintainer start` printed, with trailing newlines removed.
    pub context: &'a [u8],
}

const REHEARSAL: &str = "## This run is a REHEARSAL. Post nothing.

Do the whole pass: read, verify, recount, form the conclusions you would
publish. Then write them into the run report and the drafts directory instead
of into GitHub. Every posting command is blocked by the settings file, so a
refusal here is the rehearsal working. Do not look for another way to send it.

In the report, list what you WOULD have posted and where, so the maintainer can
read it and decide whether to turn posting on.
";

/// The section for a run that posts. `POST` is quoted from the value in force,
/// because a prompt that says nothing about posting leaves whatever the profile's
/// own preamble claims as the only statement on it, and on 2026-09-07 that claim
/// was a stale `POST=off` in front of a run that posted five issues.
fn posting(repo_slug: &str, gh_account: &str, post: &str) -> String {
    format!(
        "## This run POSTS. Say it once, in public, under a real name.

Writes go to {repo_slug} as {gh_account}, immediately and for real. There is no
rehearsal in front of you: a comment is delivered as you send it, an edit keeps
its history, and a notification cannot be recalled. `MAINTAINER_POST` is
`{post}` in your environment, and that variable is the answer, not any
sentence you read in a prompt.

Verify before you write, and read back what you wrote.
"
    )
}

/// Only the exact value `off` is a rehearsal; any other value posts.
#[must_use]
pub fn is_rehearsal(post: &str) -> bool {
    post == "off"
}

/// Escapes the maintainer's name for the replacement half of a `sed` `s` command
/// whose delimiter is `|`: backslash, then `&`, then `|`, in that order.
#[must_use]
pub fn escape_for_sed(name: &str) -> String {
    name.replace('\\', "\\\\").replace('&', "\\&").replace('|', "\\|")
}

/// The arguments of the `sed` that fills in the doctrine. `name` is the
/// maintainer's name as the profile gives it, and is escaped here.
#[must_use]
pub fn sed_arguments(name: &str, repo_slug: &str, core_file: &str) -> Vec<String> {
    vec![
        "-e".to_string(),
        format!("s|__MAINTAINER__|{}|g", escape_for_sed(name)),
        "-e".to_string(),
        format!("s|__SLUG__|{repo_slug}|g"),
        core_file.to_string(),
    ]
}

/// The prompt, byte for byte.
#[must_use]
pub fn assemble(parts: &PromptParts<'_>) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"Provenance for this run: maintainer ");
    out.extend_from_slice(parts.version);
    out.extend_from_slice(b" (commit ");
    out.extend_from_slice(parts.commit);
    out.extend_from_slice(
        format!(
            "), profile {}, task {}, backend {}, started ",
            parts.profile, parts.task, parts.backend
        )
        .as_bytes(),
    );
    out.extend_from_slice(parts.started);
    out.extend_from_slice(b".\n");
    out.extend_from_slice(
        b"Open your run report with that version and that commit, copied from this line. Do not copy them from an earlier report; an earlier report was written by an earlier build.\n\n",
    );
    out.extend_from_slice(parts.common);
    out.extend_from_slice(b"\n\n");
    out.extend_from_slice(parts.core);
    out.extend_from_slice(b"\n\n");
    if let Some(prose) = parts.prose {
        out.extend_from_slice(prose);
        out.extend_from_slice(b"\n\n");
    }
    if is_rehearsal(parts.post) {
        out.extend_from_slice(REHEARSAL.as_bytes());
    } else {
        out.extend_from_slice(posting(parts.repo_slug, parts.gh_account, parts.post).as_bytes());
    }
    out.extend_from_slice(b"\n\n");
    out.extend_from_slice(parts.task_prompt);
    out.extend_from_slice(b"\n\n## Context for this run, computed just now\n\n```\n");
    out.extend_from_slice(parts.context);
    out.extend_from_slice(b"\n```\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts<'a>(post: &'a str, prose: Option<&'a [u8]>) -> PromptParts<'a> {
        PromptParts {
            version: b"9.9.9",
            commit: b"abc1234",
            profile: "fx",
            task: "review",
            backend: "claude",
            started: b"2026-01-01T00:00:00+00:00",
            common: b"COMMON\nsecond\n",
            core: b"CORE for Fixture Maintainer\n",
            prose,
            post,
            repo_slug: "fx/repo",
            gh_account: "fxacct",
            task_prompt: b"TASK review body\n",
            context: b"=== maintainer id ===\nskill        s",
        }
    }

    const PROVENANCE: &str = "Provenance for this run: maintainer 9.9.9 (commit abc1234), profile fx, task review, backend claude, started 2026-01-01T00:00:00+00:00.\nOpen your run report with that version and that commit, copied from this line. Do not copy them from an earlier report; an earlier report was written by an earlier build.\n\n";

    #[test]
    fn a_posting_prompt_is_the_bytes_the_bash_writes() {
        let want = format!(
            "{PROVENANCE}COMMON\nsecond\n\n\nCORE for Fixture Maintainer\n\n\nPROSE\n\n\n## This run POSTS. Say it once, in public, under a real name.\n\nWrites go to fx/repo as fxacct, immediately and for real. There is no\nrehearsal in front of you: a comment is delivered as you send it, an edit keeps\nits history, and a notification cannot be recalled. `MAINTAINER_POST` is\n`on` in your environment, and that variable is the answer, not any\nsentence you read in a prompt.\n\nVerify before you write, and read back what you wrote.\n\n\nTASK review body\n\n\n## Context for this run, computed just now\n\n```\n=== maintainer id ===\nskill        s\n```\n"
        );
        let got = assemble(&parts("on", Some(b"PROSE\n")));
        assert_eq!(String::from_utf8(got).unwrap(), want);
    }

    #[test]
    fn a_rehearsal_prompt_carries_the_rehearsal_section_and_no_posting_section() {
        let want = format!(
            "{PROVENANCE}COMMON\nsecond\n\n\nCORE for Fixture Maintainer\n\n\n## This run is a REHEARSAL. Post nothing.\n\nDo the whole pass: read, verify, recount, form the conclusions you would\npublish. Then write them into the run report and the drafts directory instead\nof into GitHub. Every posting command is blocked by the settings file, so a\nrefusal here is the rehearsal working. Do not look for another way to send it.\n\nIn the report, list what you WOULD have posted and where, so the maintainer can\nread it and decide whether to turn posting on.\n\n\nTASK review body\n\n\n## Context for this run, computed just now\n\n```\n=== maintainer id ===\nskill        s\n```\n"
        );
        let got = assemble(&parts("off", None));
        assert_eq!(String::from_utf8(got).unwrap(), want);
    }

    #[test]
    fn a_post_value_other_than_off_posts_and_the_prompt_quotes_it() {
        let got = String::from_utf8(assemble(&parts("maybe", None))).unwrap();
        assert!(got.contains("## This run POSTS."), "{got}");
        assert!(got.contains("`MAINTAINER_POST` is\n`maybe` in your environment"), "{got}");
        assert!(!got.contains("REHEARSAL"), "{got}");
    }

    #[test]
    fn the_sections_come_in_the_documented_order() {
        let got = String::from_utf8(assemble(&parts("on", Some(b"PROSE\n")))).unwrap();
        let at = |needle: &str| got.find(needle).unwrap_or_else(|| panic!("no {needle}"));
        let order = [
            at("Provenance for this run"),
            at("COMMON"),
            at("CORE for"),
            at("PROSE"),
            at("## This run POSTS"),
            at("TASK review body"),
            at("## Context for this run"),
        ];
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
    }

    #[test]
    fn a_prose_section_is_left_out_when_there_is_none() {
        let got = String::from_utf8(assemble(&parts("on", None))).unwrap();
        assert!(!got.contains("PROSE"), "{got}");
        assert!(got.contains("CORE for Fixture Maintainer\n\n\n## This run POSTS"), "{got}");
    }

    #[test]
    fn a_context_with_a_backtick_fence_is_carried_unchanged() {
        let mut p = parts("on", None);
        p.context = b"```\ninside\n```";
        let got = String::from_utf8(assemble(&p)).unwrap();
        assert!(got.ends_with("```\n```\ninside\n```\n```\n"), "{got}");
    }

    #[test]
    fn bytes_that_are_not_utf8_pass_through_every_input() {
        let mut p = parts("on", None);
        p.common = &[0xff, 0xfe, b'\n'];
        p.context = &[0x80];
        let got = assemble(&p);
        assert!(got.windows(3).any(|w| w == [0xff, 0xfe, b'\n']));
        assert!(got.windows(2).any(|w| w == [0x80, b'\n']));
    }

    #[test]
    fn the_name_is_escaped_for_a_sed_replacement_in_the_bash_order() {
        assert_eq!(escape_for_sed("A&B"), "A\\&B");
        assert_eq!(escape_for_sed("A|B"), "A\\|B");
        assert_eq!(escape_for_sed("A\\B"), "A\\\\B");
        assert_eq!(escape_for_sed("\\&|"), "\\\\\\&\\|");
        assert_eq!(escape_for_sed("plain name"), "plain name");
    }

    #[test]
    fn the_sed_arguments_escape_the_name_and_leave_the_slug_alone_decision_d16() {
        // D16 is preserved: the slug is not escaped, so `&` in it means the match.
        let args = sed_arguments("A&B", "fx/a&b", "/c/core.md");
        assert_eq!(
            args,
            vec!["-e", "s|__MAINTAINER__|A\\&B|g", "-e", "s|__SLUG__|fx/a&b|g", "/c/core.md"]
        );
    }

    #[test]
    fn only_the_exact_value_off_is_a_rehearsal() {
        assert!(is_rehearsal("off"));
        assert!(!is_rehearsal("OFF"));
        assert!(!is_rehearsal("off "));
        assert!(!is_rehearsal(""));
        assert!(!is_rehearsal("on"));
    }
}
