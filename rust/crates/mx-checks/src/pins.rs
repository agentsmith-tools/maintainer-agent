//! `verify-action-pins`: every `uses:` in the workflows, the local composite actions and the
//! examples must be pinned to a 40-character SHA with a version comment, and the comment must
//! be the tag that SHA is.
//!
//! The text of a line is read by pure functions ([`unparsed_lines`], [`pin_texts`], [`Pin::parse`]),
//! each the Rust spelling of one `grep`, `sed` or `sort` of the script. What GitHub says about
//! a tag comes through the [`TagRefs`] port, which makes the same three `gh api` calls the script
//! makes, in the same order. [`run`] chains them and prints what the script prints.
//!
//! # Decisions
//!
//! - `P4B-2`: the `Gh` port of `mx-ports` has no `api` verb, so this crate has its own narrow port
//!   over `Proc`, [`GhTagRefs`], and the unit tests fake it ([`FakeTagRefs`]).
//! - `P4B-6`: lines and names sort by bytes, the order the script gets under `LC_ALL=C`.
//! - `P4B-14`: a workflow file with a NUL byte in its first 32 KiB is binary to `grep` and contributes no
//!   line, as it does under GNU `grep` and under `ugrep`.
//! - `P4B-16` (a fix): the repository and the tag of a pin become the path of `gh api`, and the script
//!   sends them as they are, so `../../orgs/x@SHA # v1` asks for `repos/../../git/ref/tags/v1` and a tag
//!   of `v1?x=1` or `..%2f..%2fuser` changes the request. A pin whose repository is not `OWNER/NAME`
//!   (letters, digits, `_`, `.`, `-`, neither part `.` or `..`) or whose tag holds `..`, `?`, `#`, `%`, a
//!   control character or an empty path segment is refused without a call, with a named line and exit 1
//!   ([`lookup_refusal`]). The tag is percent-encoded where it is not a URL path character
//!   ([`encode_tag`]), and a tag object id that GitHub answers is asked about only when it is
//!   hexadecimal ([`is_object_id`]).
//! - A comment that is not UTF-8 reaches `gh` with U+FFFD in its place (the process port takes text).
//!
//! # Found, not fixed
//!
//! The script has these defects; each is preserved and pinned by `tests/characterization/checks-pins.sh`:
//! a comment glued to the SHA (`@SHA# v1`) parses and then never matches; a two-word comment is
//! reported as unparsed and is also verified on its first word; a missing `gh` reads as "not a tag"
//! for every pin; a failing type call is ignored; and a `uses:` that is a quoted string, a local
//! action (`./...`) or a `docker://` reference is reported as unparsed.

use crate::ctx::{Call, Ctx};
use crate::host::{EntryKind, Kind};
use crate::shell;
use mx_core::Fact;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// What GitHub says about a tag.
pub trait TagRefs {
    /// `gh api repos/REPO/git/ref/tags/TAG --jq .object.sha`: the object the tag's ref names, or
    /// `None` when the call failed (a non-zero exit, or no `gh`).
    fn ref_sha(&self, ctx: &Ctx<'_>, repo: &[u8], tag: &[u8]) -> Option<Vec<u8>>;

    /// `... --jq .object.type`: `commit` for a lightweight tag, `tag` for an annotated one. Empty
    /// when the call failed: the script reads its output and never its status.
    fn ref_type(&self, ctx: &Ctx<'_>, repo: &[u8], tag: &[u8]) -> Vec<u8>;

    /// `gh api repos/REPO/git/tags/OBJECT --jq .object.sha`: the commit an annotated tag object
    /// names. Its output is used whatever the status.
    fn tag_object_sha(&self, ctx: &Ctx<'_>, repo: &[u8], object: &[u8]) -> Vec<u8>;
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The real answers: three `gh api` calls through the process port, standard error discarded.
#[derive(Debug, Clone, Copy, Default)]
pub struct GhTagRefs;

impl GhTagRefs {
    fn jq(ctx: &Ctx<'_>, path: &str, filter: &str) -> crate::ctx::Done {
        ctx.exec(&Call::new(["gh", "api", path, "--jq", filter]))
    }

    /// The API path of a tag's ref, or `None` when the repository or the tag could steer the path
    /// (decision `P4B-16`). The caller has refused such a pin already; this is the second lock.
    fn ref_path(repo: &[u8], tag: &[u8]) -> Option<String> {
        if lookup_refusal(repo, tag).is_some() {
            return None;
        }
        Some(format!("repos/{}/git/ref/tags/{}", text(repo), encode_tag(tag)))
    }
}

impl TagRefs for GhTagRefs {
    fn ref_sha(&self, ctx: &Ctx<'_>, repo: &[u8], tag: &[u8]) -> Option<Vec<u8>> {
        let path = Self::ref_path(repo, tag)?;
        let done = Self::jq(ctx, &path, ".object.sha");
        done.ok().then(|| shell::command_substitution(&done.stdout))
    }

    fn ref_type(&self, ctx: &Ctx<'_>, repo: &[u8], tag: &[u8]) -> Vec<u8> {
        let Some(path) = Self::ref_path(repo, tag) else { return Vec::new() };
        let done = Self::jq(ctx, &path, ".object.type");
        shell::command_substitution(&done.stdout)
    }

    fn tag_object_sha(&self, ctx: &Ctx<'_>, repo: &[u8], object: &[u8]) -> Vec<u8> {
        // The object id comes from GitHub's answer, and it too becomes part of a path: only a
        // hexadecimal id is asked about.
        if lookup_refusal(repo, b"x").is_some() || !is_object_id(object) {
            return Vec::new();
        }
        let done = Self::jq(
            ctx,
            &format!("repos/{}/git/tags/{}", text(repo), text(object)),
            ".object.sha",
        );
        shell::command_substitution(&done.stdout)
    }
}

/// Why a pin cannot be looked up on GitHub, or `None` when it can (decision `P4B-16`).
///
/// The repository and the tag come from the text of a workflow file, and they become a path of
/// `gh api`. The script sends them as they are, so `uses: ../../orgs/x@SHA # v1` asks for
/// `repos/../../git/ref/tags/v1`, and a tag of `v1?x=1` or `..%2f..%2fuser` changes the request
/// that is made. Here the repository must be `OWNER/NAME` of letters, digits, `_`, `.` and `-`
/// with neither part `.` or `..`, and the tag must hold no `..`, `?`, `#`, `%`, control character or
/// empty path segment (no leading, trailing or doubled `/`).
#[must_use]
pub fn lookup_refusal(repo: &[u8], tag: &[u8]) -> Option<&'static str> {
    let name_byte = |byte: &u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-');
    let mut parts = repo.split(|byte| *byte == b'/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return Some("the repository is not OWNER/NAME");
    };
    for part in [owner, name] {
        if part.is_empty() || part == b"." || part == b".." || !part.iter().all(name_byte) {
            return Some("the repository is not OWNER/NAME of letters, digits, _, . and -");
        }
    }
    if tag.is_empty() {
        return Some("the tag is empty");
    }
    if shell::contains(tag, b"..") {
        return Some("the tag holds ..");
    }
    if tag.iter().any(|byte| matches!(byte, b'?' | b'#' | b'%')) {
        return Some("the tag holds ?, # or %");
    }
    if tag.iter().any(|byte| *byte < 0x20 || *byte == 0x7f) {
        return Some("the tag holds a control character");
    }
    if tag.starts_with(b"/") || tag.ends_with(b"/") || shell::contains(tag, b"//") {
        return Some("the tag holds an empty path segment");
    }
    None
}

/// The tag as it goes into the path of a request: every byte that is not a letter, a digit or one
/// of ``-._~!$&'()*+,;=:@/`` (the characters a URL path takes as they are) is percent-encoded.
#[must_use]
pub fn encode_tag(tag: &[u8]) -> String {
    let mut out = String::new();
    for byte in tag {
        if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(byte) {
            out.push(char::from(*byte));
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    out
}

/// A git object id as GitHub prints it: 40 to 64 lower-case hexadecimal digits.
#[must_use]
pub fn is_object_id(object: &[u8]) -> bool {
    (40..=64).contains(&object.len())
        && object.iter().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// What a [`FakeTagRefs`] was told about one tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeTag {
    /// The ref's object, or `None` when the first call fails.
    pub sha: Option<Vec<u8>>,
    pub kind: Vec<u8>,
}

/// Scripted answers by `(repo, tag)` and by tag object. A question nobody scripted panics.
#[derive(Debug, Default)]
pub struct FakeTagRefs {
    tags: BTreeMap<(Vec<u8>, Vec<u8>), FakeTag>,
    objects: BTreeMap<(Vec<u8>, Vec<u8>), Vec<u8>>,
    calls: RefCell<Vec<String>>,
}

impl FakeTagRefs {
    /// A fake that knows nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A lightweight tag: its ref names commit `sha`.
    #[must_use]
    pub fn lightweight(mut self, repo: &str, tag: &str, sha: &str) -> Self {
        self.tags.insert(
            (repo.as_bytes().to_vec(), tag.as_bytes().to_vec()),
            FakeTag { sha: Some(sha.as_bytes().to_vec()), kind: b"commit".to_vec() },
        );
        self
    }

    /// An annotated tag: its ref names tag object `object`, which names commit `sha`.
    #[must_use]
    pub fn annotated(mut self, repo: &str, tag: &str, object: &str, sha: &str) -> Self {
        self.tags.insert(
            (repo.as_bytes().to_vec(), tag.as_bytes().to_vec()),
            FakeTag { sha: Some(object.as_bytes().to_vec()), kind: b"tag".to_vec() },
        );
        self.objects.insert(
            (repo.as_bytes().to_vec(), object.as_bytes().to_vec()),
            sha.as_bytes().to_vec(),
        );
        self
    }

    /// A ref that does not exist: the first call fails.
    #[must_use]
    pub fn missing(mut self, repo: &str, tag: &str) -> Self {
        self.tags.insert(
            (repo.as_bytes().to_vec(), tag.as_bytes().to_vec()),
            FakeTag { sha: None, kind: Vec::new() },
        );
        self
    }

    /// A ref whose type call fails (it prints nothing) while the first call works.
    #[must_use]
    pub fn type_call_fails(mut self, repo: &str, tag: &str, sha: &str) -> Self {
        self.tags.insert(
            (repo.as_bytes().to_vec(), tag.as_bytes().to_vec()),
            FakeTag { sha: Some(sha.as_bytes().to_vec()), kind: Vec::new() },
        );
        self
    }

    /// The calls made so far, one line each, in order: `sha REPO TAG`, `type REPO TAG`, `object REPO OBJECT`.
    #[must_use]
    pub fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    fn tag(&self, repo: &[u8], tag: &[u8]) -> &FakeTag {
        self.tags
            .get(&(repo.to_vec(), tag.to_vec()))
            .unwrap_or_else(|| panic!("FakeTagRefs: nobody scripted {}/{}", text(repo), text(tag)))
    }
}

impl TagRefs for FakeTagRefs {
    fn ref_sha(&self, _ctx: &Ctx<'_>, repo: &[u8], tag: &[u8]) -> Option<Vec<u8>> {
        self.calls.borrow_mut().push(format!("sha {} {}", text(repo), text(tag)));
        self.tag(repo, tag).sha.clone()
    }

    fn ref_type(&self, _ctx: &Ctx<'_>, repo: &[u8], tag: &[u8]) -> Vec<u8> {
        self.calls.borrow_mut().push(format!("type {} {}", text(repo), text(tag)));
        self.tag(repo, tag).kind.clone()
    }

    fn tag_object_sha(&self, _ctx: &Ctx<'_>, repo: &[u8], object: &[u8]) -> Vec<u8> {
        self.calls.borrow_mut().push(format!("object {} {}", text(repo), text(object)));
        self.objects.get(&(repo.to_vec(), object.to_vec())).cloned().unwrap_or_default()
    }
}

// --- reading the text --------------------------------------------------------------------------

/// The text of every line that starts a `uses:` (`^[[:space:]]*(-[[:space:]]*)?uses:`), from the
/// word `uses:` to the end of the line.
fn uses_tail(line: &[u8]) -> Option<&[u8]> {
    let mut at = 0;
    while at < line.len() && shell::is_space(line[at]) {
        at += 1;
    }
    if line.get(at) == Some(&b'-') {
        at += 1;
        while at < line.len() && shell::is_space(line[at]) {
            at += 1;
        }
    }
    line[at..].starts_with(b"uses:").then(|| &line[at..])
}

/// How far a pin pattern matches at `start`, or `None`: `uses: [^@ ]+@[0-9a-f]{40} *# *\S+`.
/// The match is the longest one from that point, as `grep -o` takes it.
fn pin_end(text: &[u8], start: usize) -> Option<usize> {
    if !text[start..].starts_with(b"uses: ") {
        return None;
    }
    let mut at = start + 6;
    let name = at;
    while at < text.len() && text[at] != b'@' && text[at] != b' ' {
        at += 1;
    }
    if at == name || text.get(at) != Some(&b'@') {
        return None;
    }
    at += 1;
    let sha = text.get(at..at + 40)?;
    if !sha.iter().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    at += 40;
    while text.get(at) == Some(&b' ') {
        at += 1;
    }
    if text.get(at) != Some(&b'#') {
        return None;
    }
    at += 1;
    while text.get(at) == Some(&b' ') {
        at += 1;
    }
    let word = at;
    while at < text.len() && !shell::is_space(text[at]) {
        at += 1;
    }
    (at > word).then_some(at)
}

/// `grep -vE '^uses: [^@ ]+@[0-9a-f]{40} *# *\S+$'` keeps a line when this is false: the whole
/// line is one pin.
#[must_use]
pub fn is_strict_pin(line: &[u8]) -> bool {
    pin_end(line, 0) == Some(line.len())
}

/// The first half of the script: the `uses:` tails of every file's lines, trailing white space
/// removed, sorted without duplicates (`sort -u`), and kept when they are not a whole pin.
#[must_use]
pub fn unparsed_lines(files: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut tails: Vec<Vec<u8>> = Vec::new();
    for file in files {
        for line in shell::lines(file, true) {
            if let Some(tail) = uses_tail(line) {
                tails.push(shell::trim_trailing_space(tail).to_vec());
            }
        }
    }
    tails.sort();
    tails.dedup();
    tails.retain(|line| !is_strict_pin(line));
    tails
}

/// The second half: what `grep -hoE 'uses: [^@ ]+@[0-9a-f]{40} *# *\S+'` prints over every file,
/// without the leading `uses: `, sorted without duplicates (`sort -u`). A line can hold several
/// matches, and a match can end before the line does.
#[must_use]
pub fn pin_texts(files: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut found: Vec<Vec<u8>> = Vec::new();
    for file in files {
        for line in shell::lines(file, true) {
            let mut at = 0;
            while at < line.len() {
                match pin_end(line, at) {
                    Some(end) => {
                        found.push(line[at + 6..end].to_vec());
                        at = end;
                    }
                    None => at += 1,
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// One pin, cut up the way the script's parameter expansions cut it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// `${line%%@*}`: the action, subdirectory included.
    pub action: Vec<u8>,
    /// `${rest%% *}`: the text after the `@` up to its first space. It is the SHA when a space
    /// follows the SHA, and the SHA with the comment stuck to it when none does.
    pub sha: Vec<u8>,
    /// `sed 's/.*# *//'`: what follows the last `#`, spaces removed.
    pub claim: Vec<u8>,
    /// `cut -d/ -f1,2`: the repository the tag lives in.
    pub repo: Vec<u8>,
}

impl Pin {
    /// Cuts `action@sha # claim`.
    #[must_use]
    pub fn parse(line: &[u8]) -> Self {
        let at = line.iter().position(|byte| *byte == b'@').unwrap_or(line.len());
        let action = line[..at].to_vec();
        let rest = if at < line.len() { &line[at + 1..] } else { &[][..] };
        let sha_end = rest.iter().position(|byte| *byte == b' ').unwrap_or(rest.len());
        let claim = match rest.iter().rposition(|byte| *byte == b'#') {
            Some(hash) => {
                let mut start = hash + 1;
                while rest.get(start) == Some(&b' ') {
                    start += 1;
                }
                rest[start..].to_vec()
            }
            None => rest.to_vec(),
        };
        let repo = shell::first_two_fields(&action);
        Self { action, sha: rest[..sha_end].to_vec(), claim, repo }
    }
}

/// What one pin turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The comment names a branch, which moves by design.
    Branch,
    /// The comment is a tag that is not there (or GitHub could not be asked).
    NotATag,
    /// The comment is a tag and it is this SHA.
    Ok,
    /// The comment is a tag on another commit.
    Mismatch { real: Vec<u8> },
    /// The pin names a repository or a tag that could steer the API path, so GitHub was not asked.
    Refused(&'static str),
}

/// `deref`: the commit a tag names, as GitHub says it. Empty when it could not be had.
fn deref(refs: &dyn TagRefs, ctx: &Ctx<'_>, pin: &Pin) -> Vec<u8> {
    let Some(object) = refs.ref_sha(ctx, &pin.repo, &pin.claim) else { return Vec::new() };
    let kind = refs.ref_type(ctx, &pin.repo, &pin.claim);
    if kind == b"tag" { refs.tag_object_sha(ctx, &pin.repo, &object) } else { object }
}

/// Judges one pin, asking GitHub only when the comment is not a branch.
pub fn judge(refs: &dyn TagRefs, ctx: &Ctx<'_>, pin: &Pin) -> Verdict {
    if pin.claim == b"main" || pin.claim == b"master" {
        return Verdict::Branch;
    }
    if let Some(reason) = lookup_refusal(&pin.repo, &pin.claim) {
        return Verdict::Refused(reason);
    }
    let real = deref(refs, ctx, pin);
    if real.is_empty() {
        Verdict::NotATag
    } else if real == pin.sha {
        Verdict::Ok
    } else {
        Verdict::Mismatch { real }
    }
}

/// Text for a terminal: a control byte (a hostile workflow may hold an escape sequence) shows as `?`.
fn printable(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(|byte| if *byte < 0x20 || *byte == 0x7f { b'?' } else { *byte }).collect()
}

/// The line the script prints for a pin, without its newline, and whether it counts as a failure.
#[must_use]
pub fn render(pin: &Pin, verdict: &Verdict) -> (Vec<u8>, bool) {
    let mut line = Vec::new();
    let head8 = |bytes: &[u8]| bytes[..bytes.len().min(8)].to_vec();
    let action = shell::pad_right(&pin.action, 46);
    match verdict {
        Verdict::Branch => {
            line.extend_from_slice(b"  branch  ");
            line.extend_from_slice(&action);
            line.extend_from_slice(b" tracks ");
            line.extend_from_slice(&pin.claim);
            line.extend_from_slice(b" by design");
            (line, false)
        }
        Verdict::NotATag => {
            line.extend_from_slice(b"  FAIL    ");
            line.extend_from_slice(&action);
            line.extend_from_slice(b" claims ");
            line.extend_from_slice(&pin.claim);
            line.extend_from_slice(b", which is not a tag in ");
            line.extend_from_slice(&pin.repo);
            (line, true)
        }
        Verdict::Refused(reason) => {
            line.extend_from_slice(b"  FAIL    ");
            line.extend_from_slice(&shell::pad_right(&printable(&pin.action), 46));
            line.extend_from_slice(b" claims ");
            line.extend_from_slice(&printable(&pin.claim));
            line.extend_from_slice(b", which cannot be checked: ");
            line.extend_from_slice(reason.as_bytes());
            (line, true)
        }
        Verdict::Ok => {
            line.extend_from_slice(b"  ok      ");
            line.extend_from_slice(&action);
            line.push(b' ');
            line.extend_from_slice(&head8(&pin.sha));
            line.extend_from_slice(b" is ");
            line.extend_from_slice(&pin.claim);
            (line, false)
        }
        Verdict::Mismatch { real } => {
            line.extend_from_slice(b"  FAIL    ");
            line.extend_from_slice(&action);
            line.extend_from_slice(b" pinned ");
            line.extend_from_slice(&head8(&pin.sha));
            line.extend_from_slice(b" but ");
            line.extend_from_slice(&pin.claim);
            line.extend_from_slice(b" is ");
            line.extend_from_slice(&head8(real));
            (line, true)
        }
    }
}

/// A file `grep` takes for binary: a NUL byte in its first 32 KiB. Both greps that matter here (GNU
/// and ugrep) print no matching line for such a file, and the script silences the message the GNU one
/// writes, so the file contributes nothing (decision `P4B-14`).
#[must_use]
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(32768)].contains(&0)
}

// --- the files ---------------------------------------------------------------------------------

/// `find DIR -maxdepth N -type f ( -name A -o -name B ) | sort`: the regular files whose name is
/// one of `names` or, with a leading `*`, ends with the rest. Links are not files here.
fn find_files(
    ctx: &Ctx<'_>,
    dir: &Path,
    max_depth: Option<usize>,
    names: &[&[u8]],
) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = ctx
        .host()
        .walk(dir, max_depth)
        .into_iter()
        .filter(|entry| entry.kind == EntryKind::File)
        .filter(|entry| {
            let name = entry.path.file_name().map(OsStr::as_bytes).unwrap_or_default();
            names.iter().any(|pattern| match pattern.strip_prefix(b"*") {
                Some(suffix) => name.ends_with(suffix),
                None => name == *pattern,
            })
        })
        .map(|entry| entry.path)
        .collect();
    found.sort_by(|a, b| a.as_os_str().as_bytes().cmp(b.as_os_str().as_bytes()));
    found
}

/// `$root/NAME`, spelled as the shell spells it: the root text as given, a slash, the name.
fn under(root: &OsStr, tail: &str) -> PathBuf {
    let mut bytes = root.as_bytes().to_vec();
    bytes.push(b'/');
    bytes.extend_from_slice(tail.as_bytes());
    PathBuf::from(OsString::from_vec(bytes))
}

/// The script, from its command line (an optional repository root) to its exit code.
pub fn run(ctx: &mut Ctx<'_>, refs: &dyn TagRefs, args: &[OsString]) -> i32 {
    let root: OsString = match args.first() {
        Some(given) if !given.is_empty() => given.clone(),
        _ => ctx.tree_root().into_os_string(),
    };
    let message = |root: &OsStr, middle: &str, tail: &str| {
        let mut text = b"verify-action-pins: ".to_vec();
        text.extend_from_slice(middle.as_bytes());
        text.extend_from_slice(root.as_bytes());
        text.extend_from_slice(tail.as_bytes());
        text.push(b'\n');
        text
    };
    let workflows = under(&root, ".github/workflows");
    if ctx.host().kind(&workflows) != Kind::Dir {
        ctx.put_err(&message(&root, "no .github/workflows under ", ""));
        return 2;
    }
    let mut sources = find_files(ctx, &workflows, Some(1), &[b"*.yml", b"*.yaml"]);
    if sources.is_empty() {
        ctx.put_err(&message(
            &root,
            "no workflow files under ",
            "/.github/workflows, so nothing was scanned",
        ));
        return 2;
    }
    let actions = under(&root, ".github/actions");
    if ctx.host().kind(&actions) == Kind::Dir {
        let found = find_files(ctx, &actions, None, &[b"action.yml", b"action.yaml"]);
        if found.is_empty() {
            ctx.put_err(&message(&root, "", "/.github/actions exists and holds no action.yml, so the composite scan read nothing"));
            return 2;
        }
        sources.extend(found);
    }
    sources.extend(find_files(ctx, &under(&root, "examples"), Some(1), &[b"*.yml"]));

    // A file that cannot be read is skipped without a word (`2>/dev/null`).
    let files: Vec<Vec<u8>> = sources
        .iter()
        .filter_map(|path| match ctx.host().read(path) {
            Fact::Known(bytes) if !is_binary(&bytes) => Some(bytes),
            Fact::Known(_) | Fact::Absent | Fact::Unreadable(_) => None,
        })
        .collect();

    let mut fail = false;
    let unparsed = unparsed_lines(&files);
    if !unparsed.is_empty() {
        ctx.put_err(
            b"  FAIL    not SHA-pinned with a version comment, so nothing below checks them:\n",
        );
        for line in &unparsed {
            let mut shown = b"            ".to_vec();
            shown.extend_from_slice(line);
            shown.push(b'\n');
            ctx.put_err(&shown);
        }
        fail = true;
    }

    for text in pin_texts(&files) {
        let pin = Pin::parse(&text);
        let verdict = judge(refs, ctx, &pin);
        let (mut line, failed) = render(&pin, &verdict);
        line.push(b'\n');
        ctx.put_out(&line);
        fail |= failed;
    }

    if fail {
        ctx.put_err(b"  a pin does not match its comment\n");
    } else {
        ctx.put_out(b"  every pin is the tag it claims\n");
    }
    i32::from(fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn pin_line(tail: &str) -> Vec<u8> {
        format!("uses: {tail}").into_bytes()
    }

    #[test]
    fn a_file_with_a_nul_byte_near_its_start_is_binary() {
        assert!(is_binary(b"uses: x\0"));
        assert!(!is_binary(b"uses: x\n"));
        let mut late = vec![b'a'; 40000];
        late.push(0);
        assert!(!is_binary(&late), "a NUL past the first 32 KiB does not make a file binary");
    }

    #[test]
    fn a_whole_line_with_one_word_comment_is_a_strict_pin() {
        assert!(is_strict_pin(&pin_line(&format!("actions/checkout@{A} # v4"))));
        assert!(is_strict_pin(&pin_line(&format!("actions/checkout@{A}   #v4"))));
        assert!(is_strict_pin(&pin_line(&format!("actions/checkout@{A}# v4"))));
    }

    #[test]
    fn each_shape_the_strict_pattern_refuses() {
        let refused = [
            "actions/checkout@3d3c42e  # v7".to_string(),
            format!("acme/nocomment@{A}"),
            format!("acme/upper@{} # v1", A.to_uppercase()),
            format!("\"acme/quoted@{A}\" # v1"),
            format!("acme/twowords@{A} # v1 extra"),
            "./.github/actions/local".to_string(),
            "docker://alpine:3".to_string(),
            format!("acme/empty@{A} #"),
            format!("acme/long@{A}a # v1"),
            format!("acme/short@{} # v1", &A[..39]),
        ];
        for tail in refused {
            assert!(!is_strict_pin(&pin_line(&tail)), "{tail} must not be a strict pin");
        }
    }

    #[test]
    fn a_missing_space_after_uses_is_not_a_pin() {
        let line = format!("uses:acme/one@{A} # v1").into_bytes();
        assert!(!is_strict_pin(&line));
        assert!(pin_texts(&[line]).is_empty());
    }

    #[test]
    fn a_tab_in_the_name_is_allowed_by_the_pattern_and_a_tab_in_the_comment_is_not() {
        assert!(is_strict_pin(&pin_line(&format!("a\tb@{A} # v1"))));
        assert!(!is_strict_pin(&pin_line(&format!("a/b@{A} # v1\tv2"))));
    }

    #[test]
    fn unparsed_lines_sees_list_items_and_plain_keys_and_ignores_comments() {
        let file = "      - uses: actions/deploy-pages@v5\n        uses: acme/two@v2   \n      # - uses: acme/old@v1\n# uses: acme/older@v1\n      - run: echo uses: a/b@v1\n"
            .as_bytes()
            .to_vec();
        let got = unparsed_lines(&[file]);
        assert_eq!(
            got,
            vec![b"uses: acme/two@v2".to_vec(), b"uses: actions/deploy-pages@v5".to_vec()]
        );
    }

    #[test]
    fn unparsed_lines_are_unique_and_sorted_by_bytes_across_files() {
        let a = b"- uses: b/x@v1\n- uses: a/y@v1\n".to_vec();
        let b = b"  - uses: a/y@v1  \n- uses: B/z@v1\n".to_vec();
        let got: Vec<String> = unparsed_lines(&[a, b])
            .iter()
            .map(|l| String::from_utf8_lossy(l).into_owned())
            .collect();
        assert_eq!(got, ["uses: B/z@v1", "uses: a/y@v1", "uses: b/x@v1"]);
    }

    #[test]
    fn carriage_returns_are_trailing_space_and_do_not_make_a_pin_unparsed() {
        let file = format!("- uses: acme/one@{A} # v1\r\n").into_bytes();
        assert!(unparsed_lines(std::slice::from_ref(&file)).is_empty());
        assert_eq!(pin_texts(&[file]), vec![format!("acme/one@{A} # v1").into_bytes()]);
    }

    #[test]
    fn pin_texts_takes_the_first_word_of_a_longer_comment_as_grep_o_does() {
        let file = format!("- uses: acme/two@{A} # v1 extra\n").into_bytes();
        assert_eq!(pin_texts(&[file]), vec![format!("acme/two@{A} # v1").into_bytes()]);
    }

    #[test]
    fn pin_texts_finds_two_pins_on_one_line_and_none_in_a_commented_line_text() {
        let file = format!("x uses: a/b@{A} # v1 uses: c/d@{A} # v2\n").into_bytes();
        assert_eq!(
            pin_texts(&[file]),
            vec![format!("a/b@{A} # v1").into_bytes(), format!("c/d@{A} # v2").into_bytes()]
        );
    }

    #[test]
    fn pin_texts_takes_a_final_line_with_no_newline() {
        let file = format!("- uses: a/b@{A} # v1").into_bytes();
        assert_eq!(pin_texts(&[file]).len(), 1);
    }

    #[test]
    fn parse_cuts_a_normal_pin() {
        let pin = Pin::parse(format!("github/codeql-action/analyze@{A} # v3").as_bytes());
        assert_eq!(pin.action, b"github/codeql-action/analyze");
        assert_eq!(pin.sha, A.as_bytes());
        assert_eq!(pin.claim, b"v3");
        assert_eq!(pin.repo, b"github/codeql-action");
    }

    #[test]
    fn parse_keeps_the_comment_on_the_sha_when_no_space_separates_them() {
        let pin = Pin::parse(format!("acme/one@{A}# v1").as_bytes());
        assert_eq!(pin.sha, format!("{A}#").as_bytes());
        assert_eq!(pin.claim, b"v1");
        let glued = Pin::parse(format!("acme/one@{A}#v1").as_bytes());
        assert_eq!(glued.sha, format!("{A}#v1").as_bytes());
        assert_eq!(glued.claim, b"v1");
    }

    #[test]
    fn parse_takes_the_claim_after_the_last_hash() {
        let pin = Pin::parse(format!("acme/one@{A} # v1#2").as_bytes());
        assert_eq!(pin.claim, b"2");
        let spaced = Pin::parse(format!("acme/one@{A}  #   v9").as_bytes());
        assert_eq!(spaced.claim, b"v9");
        assert_eq!(spaced.sha, A.as_bytes());
    }

    fn ctx_for<'a>(
        proc: &'a mx_ports::FakeProc,
        host: &'a crate::host::FakeHost,
        out: &'a mut Vec<u8>,
        err: &'a mut Vec<u8>,
    ) -> Ctx<'a> {
        Ctx::new(
            proc,
            host,
            "/t/scripts/verify-action-pins.sh".into(),
            "/x".into(),
            BTreeMap::new(),
            "/w".into(),
            out,
            err,
        )
    }

    fn judged(refs: &FakeTagRefs, tail: &str) -> Verdict {
        let log = mx_ports::EffectLog::new();
        let proc = mx_ports::FakeProc::new(&log);
        let host = crate::host::FakeHost::new();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let ctx = ctx_for(&proc, &host, &mut out, &mut err);
        judge(refs, &ctx, &Pin::parse(tail.as_bytes()))
    }

    #[test]
    fn a_lightweight_tag_on_the_pinned_sha_is_ok_after_two_calls() {
        let refs = FakeTagRefs::new().lightweight("actions/checkout", "v4", A);
        assert_eq!(judged(&refs, &format!("actions/checkout@{A} # v4")), Verdict::Ok);
        assert_eq!(refs.calls(), ["sha actions/checkout v4", "type actions/checkout v4"]);
    }

    #[test]
    fn an_annotated_tag_is_followed_to_its_commit_through_a_third_call() {
        let refs = FakeTagRefs::new().annotated("actions/checkout", "v4", "OBJ", A);
        assert_eq!(judged(&refs, &format!("actions/checkout@{A} # v4")), Verdict::Ok);
        assert_eq!(
            refs.calls(),
            ["sha actions/checkout v4", "type actions/checkout v4", "object actions/checkout OBJ"]
        );
    }

    #[test]
    fn a_pin_on_the_tag_object_instead_of_the_commit_is_a_mismatch() {
        let refs = FakeTagRefs::new().annotated("a/b", "v4", "OBJ", A);
        let verdict = judged(&refs, "a/b@OBJ # v4");
        assert_eq!(verdict, Verdict::Mismatch { real: A.as_bytes().to_vec() });
    }

    #[test]
    fn a_failing_first_call_reads_as_not_a_tag_and_the_type_is_never_asked() {
        let refs = FakeTagRefs::new().missing("a/b", "v9");
        assert_eq!(judged(&refs, &format!("a/b@{A} # v9")), Verdict::NotATag);
        assert_eq!(refs.calls(), ["sha a/b v9"]);
    }

    #[test]
    fn a_failing_type_call_is_ignored_and_the_object_is_compared_as_it_is() {
        let refs = FakeTagRefs::new().type_call_fails("a/b", "v4", A);
        assert_eq!(judged(&refs, &format!("a/b@{A} # v4")), Verdict::Ok);
    }

    #[test]
    fn main_and_master_are_branches_and_ask_nothing() {
        let refs = FakeTagRefs::new();
        assert_eq!(judged(&refs, &format!("a/b@{A} # main")), Verdict::Branch);
        assert_eq!(judged(&refs, &format!("a/b@{A} # master")), Verdict::Branch);
        assert!(refs.calls().is_empty());
        let near = FakeTagRefs::new().missing("a/b", "mainline");
        assert_eq!(judged(&near, &format!("a/b@{A} # mainline")), Verdict::NotATag);
    }

    #[test]
    fn real_repositories_and_tags_are_not_refused() {
        for repo in [
            "actions/checkout",
            "a/b",
            "Alpha/b",
            "o/r",
            "owner/repo.name",
            "my-org/my_repo",
            "a/b..c",
            "github/codeql-action",
        ] {
            assert_eq!(lookup_refusal(repo.as_bytes(), b"v1"), None, "{repo}");
        }
        for tag in [
            "v1",
            "v1.2.3",
            "release/1",
            "v1+x",
            "v1.0+x;y",
            "mainline",
            "v1.0.0-rc.1",
            "caf\u{e9}",
        ] {
            assert_eq!(lookup_refusal(b"a/b", tag.as_bytes()), None, "{tag}");
        }
    }

    #[test]
    fn a_repository_that_is_not_owner_slash_name_is_refused() {
        for repo in [
            "",
            "a",
            "a/b/c",
            "../..",
            "a/..",
            "./b",
            "a/.",
            "/b",
            "a/",
            "a//b",
            "a%2fb/c",
            "a/b?x",
            "a/b#f",
            "a b/c",
            "a/b\u{1}",
            "a\\b/c",
            "caf\u{e9}/b",
        ] {
            assert!(lookup_refusal(repo.as_bytes(), b"v1").is_some(), "{repo:?}");
        }
    }

    #[test]
    fn a_tag_that_could_steer_the_path_is_refused() {
        for tag in [
            "", "..", "v1..2", "../x", "x/..", "..%2f", "v1?x=1", "v1#f", "v1%2e", "/v1", "v1/",
            "a//b", "v1\u{1}", "v1\u{7f}", "v1\n",
        ] {
            assert!(lookup_refusal(b"a/b", tag.as_bytes()).is_some(), "{tag:?}");
        }
    }

    #[test]
    fn each_refusal_names_its_reason() {
        assert_eq!(lookup_refusal(b"a", b"v1"), Some("the repository is not OWNER/NAME"));
        assert_eq!(
            lookup_refusal(b"a/..", b"v1"),
            Some("the repository is not OWNER/NAME of letters, digits, _, . and -")
        );
        assert_eq!(lookup_refusal(b"a/b", b""), Some("the tag is empty"));
        assert_eq!(lookup_refusal(b"a/b", b"v1..2"), Some("the tag holds .."));
        assert_eq!(lookup_refusal(b"a/b", b"v1?"), Some("the tag holds ?, # or %"));
        assert_eq!(lookup_refusal(b"a/b", b"v1\x01"), Some("the tag holds a control character"));
        assert_eq!(lookup_refusal(b"a/b", b"/v1"), Some("the tag holds an empty path segment"));
    }

    #[test]
    fn encode_tag_keeps_path_characters_and_encodes_the_rest() {
        assert_eq!(encode_tag(b"v1.0+x;y"), "v1.0+x;y");
        assert_eq!(encode_tag(b"release/1"), "release/1");
        assert_eq!(encode_tag(b"a:b@c~d"), "a:b@c~d");
        assert_eq!(encode_tag("caf\u{e9}".as_bytes()), "caf%C3%A9");
        assert_eq!(encode_tag(b"a\\b\"c<d>e f"), "a%5Cb%22c%3Cd%3Ee%20f");
        assert_eq!(encode_tag(b"a`b{c|d}e^f"), "a%60b%7Bc%7Cd%7De%5Ef");
    }

    #[test]
    fn only_a_hexadecimal_id_of_a_plausible_length_is_an_object_id() {
        assert!(is_object_id(&[b'a'; 40]));
        assert!(is_object_id(&[b'0'; 64]));
        assert!(!is_object_id(&[b'a'; 39]));
        assert!(!is_object_id(&[b'a'; 65]));
        assert!(!is_object_id(&[b'A'; 40]));
        assert!(!is_object_id(b"../../x"));
        assert!(!is_object_id(b""));
    }

    #[test]
    fn a_refused_pin_is_judged_without_asking_the_port() {
        let refs = FakeTagRefs::new();
        assert_eq!(
            judged(&refs, &format!("../../x@{A} # v1")),
            Verdict::Refused("the repository is not OWNER/NAME of letters, digits, _, . and -")
        );
        assert_eq!(
            judged(&refs, &format!("a/b@{A} # v1?x=1")),
            Verdict::Refused("the tag holds ?, # or %")
        );
        assert!(refs.calls().is_empty());
    }

    #[test]
    fn a_refused_pin_renders_a_failing_line_with_control_bytes_shown_as_question_marks() {
        let pin = Pin::parse(format!("a/b@{A} # v\u{1b}[1").as_bytes());
        let (line, failed) = render(&pin, &Verdict::Refused("the tag holds a control character"));
        assert!(failed);
        assert_eq!(line, format!("  FAIL    {:<46} claims v?[1, which cannot be checked: the tag holds a control character", "a/b").into_bytes());
    }

    #[test]
    fn render_pads_the_action_to_46_bytes_and_cuts_every_sha_to_eight() {
        let pin = Pin::parse(format!("actions/checkout@{A} # v4").as_bytes());
        let (ok, failed) = render(&pin, &Verdict::Ok);
        assert!(!failed);
        assert_eq!(ok, format!("  ok      {:<46} aaaaaaaa is v4", "actions/checkout").into_bytes());
        let (bad, failed) = render(
            &pin,
            &Verdict::Mismatch { real: b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_vec() },
        );
        assert!(failed);
        assert_eq!(
            bad,
            format!("  FAIL    {:<46} pinned aaaaaaaa but v4 is bbbbbbbb", "actions/checkout")
                .into_bytes()
        );
        let (none, failed) = render(&pin, &Verdict::NotATag);
        assert!(failed);
        assert_eq!(
            none,
            format!(
                "  FAIL    {:<46} claims v4, which is not a tag in actions/checkout",
                "actions/checkout"
            )
            .into_bytes()
        );
        let (branch, failed) =
            render(&Pin::parse(format!("acme/x@{A} # main").as_bytes()), &Verdict::Branch);
        assert!(!failed);
        assert_eq!(
            branch,
            format!("  branch  {:<46} tracks main by design", "acme/x").into_bytes()
        );
    }

    #[test]
    fn an_action_name_longer_than_the_column_is_printed_whole() {
        let long = "owner-with-a-long-name/repository-with-a-very-long-name/sub-action-path";
        let pin = Pin::parse(format!("{long}@{A} # v1").as_bytes());
        let (line, _) = render(&pin, &Verdict::Ok);
        assert!(line.starts_with(format!("  ok      {long} aaaaaaaa").as_bytes()));
    }
}
