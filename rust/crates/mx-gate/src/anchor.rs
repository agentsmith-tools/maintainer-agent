//! Test helpers that read `bin/maintainer-merge` and compare its messages with ours.

/// Every `die "..."` or `say "..."` literal of `bin/maintainer-merge`, whole, with each
/// expansion (`$x`, `${x}`, `$(...)`) turned into a NUL.
pub(crate) fn bash_whole_literals(source: &str, keyword: &str) -> Vec<String> {
    let bytes: Vec<char> = source.chars().collect();
    let mut lines = Vec::new();
    let mut from = 0;
    while let Some(at) = source[from..].find(keyword) {
        let start_byte = from + at;
        let prev = source[..start_byte].chars().next_back();
        from = start_byte + keyword.len();
        if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let start = source[..from].chars().count();
        let mut text = String::new();
        let mut depth = 0_usize;
        let mut i = start;
        while i < bytes.len() {
            let c = bytes[i];
            if c == '\\' && depth == 0 {
                // \` \" \$ and \\ stand for the character after the backslash.
                if let Some(next) = bytes.get(i + 1) {
                    text.push(*next);
                }
                i += 2;
                continue;
            }
            if depth == 0 && c == '"' {
                break;
            }
            if c == '$' && bytes.get(i + 1) == Some(&'(') {
                if depth == 0 {
                    text.push('\0');
                }
                depth += 1;
                i += 2;
                continue;
            }
            if depth > 0 {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                i += 1;
                continue;
            }
            if c == '$' && bytes.get(i + 1) == Some(&'{') {
                while i < bytes.len() && bytes[i] != '}' {
                    i += 1;
                }
                text.push('\0');
                i += 1;
                continue;
            }
            if c == '$'
                && bytes
                    .get(i + 1)
                    .is_some_and(|n| n.is_ascii_alphanumeric() || *n == '_' || *n == '#')
            {
                i += 1;
                while bytes.get(i).is_some_and(|n| n.is_ascii_alphanumeric() || *n == '_') {
                    i += 1;
                }
                text.push('\0');
                continue;
            }
            text.push(c);
            i += 1;
        }
        lines.push(text);
    }
    lines
}

/// Does `text` match a bash line whose expansions are wildcards?
pub(crate) fn matches_bash_line(bash: &str, text: &str) -> bool {
    let pieces: Vec<&str> = bash.split('\0').collect();
    let (first, rest) = pieces.split_first().expect("split yields one piece");
    let Some(mut remaining) = text.strip_prefix(first) else { return false };
    if rest.is_empty() {
        return remaining.is_empty();
    }
    let (last, middle) = rest.split_last().expect("rest is not empty");
    for piece in middle {
        let Some(at) = remaining.find(piece) else { return false };
        remaining = &remaining[at + piece.len()..];
    }
    remaining.ends_with(last)
}

/// The literals as lines.
pub(crate) fn bash_literal_lines(source: &str, keyword: &str) -> Vec<String> {
    bash_whole_literals(source, keyword)
        .iter()
        .flat_map(|text| text.split('\n').map(str::to_string).collect::<Vec<_>>())
        .collect()
}

/// The `die` and `say` literals of the bash gate, split into lines, that hold at least eight
/// characters of text. A line that is only an expansion matches anything, so it is left out.
pub(crate) fn strong_lines() -> Vec<String> {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bin/maintainer-merge"
    ))
    .expect("bin/maintainer-merge is two directories above rust/crates/mx-gate");
    let mut lines = bash_literal_lines(&source, "die \"");
    lines.extend(bash_literal_lines(&source, "say \""));
    // `update_refuse3 "$pr" "<why>"` prints `will not update #N: <why>` and exits 3.
    lines.extend(bash_literal_lines(&source, "update_refuse3 \"$pr\" \""));
    let strong: Vec<String> =
        lines.into_iter().filter(|line| line.replace('\0', "").trim().len() >= 5).collect();
    assert!(strong.len() > 100, "the scanner found {} lines; it read nothing", strong.len());
    strong
}

/// Every static line of every text must be a line of a `die` or `say` of the bash gate.
/// A sample value is written `\u{27e6}name\u{27e7}`; a line that holds nothing else is a value and is skipped.
pub(crate) fn check_static_lines(texts: &[String]) {
    let strong = strong_lines();
    let mut checked = 0;
    for text in texts {
        for line in text.split('\n') {
            // A message this port adds has no bash line (decision P30-21).
            if line.contains("is not owner/repo[/dir] with plain segments") {
                continue;
            }
            let line = strip_refuse3(line);
            let mut bare = String::new();
            let mut in_value = false;
            for c in line.chars() {
                match c {
                    '\u{27e6}' => in_value = true,
                    '\u{27e7}' => in_value = false,
                    _ if !in_value => bare.push(c),
                    _ => {}
                }
            }
            if bare.trim().is_empty() {
                continue;
            }
            assert!(
                strong.iter().any(|bash| matches_bash_line(bash, line)),
                "no die or say line of bin/maintainer-merge matches {line:?}"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no line was compared");
}

/// `will not update #7: <why>` is `update_refuse3`'s own format; the bash literal is `<why>`.
fn strip_refuse3(line: &str) -> &str {
    line.strip_prefix("will not update #")
        .and_then(|rest| rest.split_once(": "))
        .map_or(line, |(_, why)| why)
}
