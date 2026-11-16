//! What the doctor prints: headings, findings with an optional fix line, and the
//! summary. Rendering is the only place that knows the escape sequences.

/// One line (or two) of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A section heading, preceded by a blank line.
    Head(String),
    /// `ok`.
    Ok(String),
    /// `warn`, with a fix line when the fix is not empty.
    Warn(String, String),
    /// `FAIL`, with a fix line when the fix is not empty.
    Fail(String, String),
}

impl Item {
    #[must_use]
    pub fn head(text: impl Into<String>) -> Self {
        Self::Head(text.into())
    }
    #[must_use]
    pub fn ok(text: impl Into<String>) -> Self {
        Self::Ok(text.into())
    }
    #[must_use]
    pub fn warn(text: impl Into<String>, fix: impl Into<String>) -> Self {
        Self::Warn(text.into(), fix.into())
    }
    #[must_use]
    pub fn fail(text: impl Into<String>, fix: impl Into<String>) -> Self {
        Self::Fail(text.into(), fix.into())
    }
}

/// The findings so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    items: Vec<Item>,
}

impl Report {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, item: Item) {
        self.items.push(item);
    }

    pub fn extend(&mut self, items: impl IntoIterator<Item = Item>) {
        self.items.extend(items);
    }

    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// `(ok, warn, fail)`.
    #[must_use]
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut counts = (0, 0, 0);
        for item in &self.items {
            match item {
                Item::Ok(_) => counts.0 += 1,
                Item::Warn(..) => counts.1 += 1,
                Item::Fail(..) => counts.2 += 1,
                Item::Head(_) => {}
            }
        }
        counts
    }

    /// The exit code: 0 when nothing failed, 1 otherwise.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        i32::from(self.counts().2 != 0)
    }

    /// The text of one item, as the bash functions print it.
    #[must_use]
    pub fn render_item(item: &Item) -> String {
        let finding = |color: &str, label: &str, text: &str, fix: &str| {
            let mut out = format!("  \x1b[{color}m{label}\x1b[0m  {text}\n");
            if !fix.is_empty() {
                out.push_str("        fix: ");
                out.push_str(fix);
                out.push('\n');
            }
            out
        };
        match item {
            Item::Head(text) => format!("\n\x1b[1m{text}\x1b[0m\n"),
            // `green` takes one argument and prints four spaces after `ok`.
            Item::Ok(text) => format!("  \x1b[32mok\x1b[0m    {text}\n"),
            Item::Warn(text, fix) => finding("33", "warn", text, fix),
            Item::Fail(text, fix) => finding("31", "FAIL", text, fix),
        }
    }

    /// The closing count line.
    #[must_use]
    pub fn render_summary(&self) -> String {
        let (ok, warn, fail) = self.counts();
        format!("\n  \x1b[1m{ok} ok, {warn} warning(s), {fail} failure(s)\x1b[0m\n")
    }
}
