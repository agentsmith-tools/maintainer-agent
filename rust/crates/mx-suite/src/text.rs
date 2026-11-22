//! Text helpers the three gate crates share.

/// What `printf '%s' "$text" | sed 's/^/    /'` leaves in a message: every line
/// indented by four spaces, with no trailing newline. The empty text stays empty,
/// because `sed` prints nothing for an empty input.
#[must_use]
pub fn indent4(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    text.split('\n').map(|line| format!("    {line}")).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_gets_four_spaces() {
        assert_eq!(indent4("a\nb"), "    a\n    b");
    }

    #[test]
    fn an_empty_line_in_the_middle_is_indented_too() {
        assert_eq!(indent4("a\n\nb"), "    a\n    \n    b");
    }

    #[test]
    fn empty_text_stays_empty() {
        assert_eq!(indent4(""), "");
    }

    #[test]
    fn one_line_is_indented() {
        assert_eq!(indent4("x"), "    x");
    }
}
