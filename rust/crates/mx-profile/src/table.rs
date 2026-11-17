//! The failure-shapes table, read the way `maintainer_classify_failure` reads it.
//!
//! The bash hands the table to a `python3` child that prints one
//! `id<TAB>class<TAB>pattern` line per shape. It then reads those lines back with
//! `IFS=<TAB> read -r id cls pat`. Both halves have quirks that the table's
//! readers depend on without knowing it, and this module keeps each one:
//!
//! - The python child exits non-zero for a table that is not JSON, whose top
//!   level is not an object, whose `shapes` is neither a list nor an empty
//!   string or object, or one of whose entries lacks `id`, `class` or `pattern`
//!   as a string. The bash treats every one of those as "could not be parsed".
//!   One bad entry discards the whole table, even after good ones.
//! - A `shapes` that is missing, `[]`, `{}` or `""` prints nothing, and the bash
//!   reports "lists no shapes", a different message.
//! - `read` treats a tab as IFS whitespace. Runs of tabs collapse, leading and
//!   trailing tabs vanish, and the last variable takes the rest of the line. A
//!   field that holds a tab, an empty class or a newline therefore moves the
//!   other fields. [`read_rows`] reproduces that split byte for byte.
//! - Command substitution drops NUL bytes and trailing newlines.
//!
//! The table is parsed with `serde_json` and a typed [`Shape`]. Python's `json`
//! accepts a few things `serde_json` refuses: the bare literals `NaN`,
//! `Infinity` and `-Infinity`, a lone surrogate escape such as `"\ud800"` in a
//! field nobody prints, and nesting deeper than 128 levels in an unused field. A
//! table holding one of those reads here as "could not be parsed" (return code
//! 2, the classifier could not ask), where the bash reads it. That is decision
//! P1F-4 in the pull request: the failure direction is closed, and no shipped
//! table has any of them.

use serde::Deserialize;
use serde_json::Value;

/// Why a table gave the classifier nothing to match against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableFault {
    /// The python child exited non-zero. The bash message is "could not be parsed".
    NotParsable,
    /// The table parsed and holds no shapes. The bash message is "lists no shapes".
    NoShapes,
}

/// One entry of `shapes`. Fields other than these three are ignored, as the
/// python child ignores them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Shape {
    pub id: String,
    pub class: String,
    pub pattern: String,
}

/// What the bash loop holds after `IFS=<TAB> read -r id cls pat` for one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub class: String,
    pub pattern: String,
}

/// The shapes of a table, in file order.
///
/// `Ok` holds at least one shape. A table that parses and lists none is
/// [`TableFault::NoShapes`], and every way the python child would exit non-zero
/// is [`TableFault::NotParsable`].
pub fn parse_shapes(bytes: &[u8]) -> Result<Vec<Shape>, TableFault> {
    // Python opens the file as UTF-8 text, so a bad byte anywhere fails the read.
    let text = std::str::from_utf8(bytes).map_err(|_| TableFault::NotParsable)?;
    // Parse into `Value` first: a duplicate key keeps the last value, as it does
    // in python, where a struct deserializer would refuse the duplicate.
    let Value::Object(top) =
        serde_json::from_str::<Value>(text).map_err(|_| TableFault::NotParsable)?
    else {
        return Err(TableFault::NotParsable);
    };
    let shapes = match top.get("shapes") {
        None => return Err(TableFault::NoShapes),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                // A serde struct also reads a three-element array. Python does not.
                Value::Object(_) => Shape::deserialize(item).map_err(|_| TableFault::NotParsable),
                _ => Err(TableFault::NotParsable),
            })
            .collect::<Result<Vec<Shape>, TableFault>>()?,
        // Iterating these gives nothing, or gives strings that `sh["id"]` rejects.
        Some(Value::Object(map)) if map.is_empty() => Vec::new(),
        Some(Value::String(text)) if text.is_empty() => Vec::new(),
        Some(_) => return Err(TableFault::NotParsable),
    };
    if shapes.is_empty() { Err(TableFault::NoShapes) } else { Ok(shapes) }
}

/// The rows the bash `while read` loop sees for these shapes, in order.
///
/// The python child prints each shape as `id<TAB>class<TAB>pattern`. Command
/// substitution drops NUL bytes and trailing newlines, the here-string gives the
/// text one newline back, and `read` splits each line on tabs.
#[must_use]
pub fn read_rows(shapes: &[Shape]) -> Vec<Row> {
    let mut printed = String::new();
    for shape in shapes {
        printed.push_str(&shape.id);
        printed.push('\t');
        printed.push_str(&shape.class);
        printed.push('\t');
        printed.push_str(&shape.pattern);
        printed.push('\n');
    }
    let printed = printed.replace('\0', "");
    let printed = printed.trim_end_matches('\n');
    if printed.is_empty() {
        return Vec::new();
    }
    printed
        .split('\n')
        .map(|line| {
            let (id, class, pattern) = split_line(line);
            Row { id: id.to_string(), class: class.to_string(), pattern: pattern.to_string() }
        })
        .collect()
}

/// What `IFS=<TAB> read -r a b c` assigns for one line.
///
/// A tab is IFS whitespace: leading tabs are skipped, a run of tabs is one
/// separator, and the last variable takes the rest of the line with its
/// trailing tabs removed. Tabs only, so a space is an ordinary character.
#[must_use]
pub fn split_line(line: &str) -> (&str, &str, &str) {
    let (id, rest) = take_field(line.trim_start_matches('\t'));
    let (class, rest) = take_field(rest.trim_start_matches('\t'));
    let pattern = rest.trim_start_matches('\t').trim_end_matches('\t');
    (id, class, pattern)
}

/// The text up to the first tab, and the text from that tab on.
fn take_field(text: &str) -> (&str, &str) {
    match text.find('\t') {
        Some(at) => text.split_at(at),
        None => (text, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(id: &str, class: &str, pattern: &str) -> Shape {
        Shape { id: id.into(), class: class.into(), pattern: pattern.into() }
    }

    fn ok_table(entries: &str) -> Vec<u8> {
        format!(r#"{{"shapes": [{entries}]}}"#).into_bytes()
    }

    const ONE: &str = r#"{"id": "a", "class": "transient", "pattern": "boom"}"#;

    #[test]
    fn the_shipped_table_parses_to_its_21_shapes_in_file_order() {
        let path = format!("{}/../../../lib/failure-shapes.json", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(&path).unwrap();
        let shapes = parse_shapes(&bytes).unwrap();
        assert_eq!(shapes.len(), 21, "the shipped table holds 21 shapes");
        assert_eq!(shapes[0].id, "session-limit");
        assert_eq!(shapes[20].id, "died-without-saying");
        assert_eq!(shapes[20].pattern, "(?!x)x");
    }

    #[test]
    fn a_table_with_one_shape_gives_that_shape() {
        assert_eq!(parse_shapes(&ok_table(ONE)), Ok(vec![shape("a", "transient", "boom")]),);
    }

    // Plan decision F8: the bash reads `d.get("shapes", [])`, so a table with
    // no `shapes` key prints nothing and reads as "lists no shapes".
    #[test]
    fn a_table_with_no_shapes_key_lists_no_shapes() {
        assert_eq!(parse_shapes(b"{}"), Err(TableFault::NoShapes));
        assert_eq!(parse_shapes(br#"{"version": 1}"#), Err(TableFault::NoShapes));
    }

    // Python iterates an empty list, an empty dict and an empty string without
    // a step, so all three print nothing. Plan decision F8.
    #[test]
    fn an_empty_list_dict_or_string_of_shapes_lists_no_shapes() {
        for table in [r#"{"shapes": []}"#, r#"{"shapes": {}}"#, r#"{"shapes": ""}"#] {
            assert_eq!(parse_shapes(table.as_bytes()), Err(TableFault::NoShapes), "{table}");
        }
    }

    // A non-empty dict iterates its keys, which are strings, and `sh["id"]` on a
    // string raises TypeError. A string iterates characters, with the same
    // result. None, a number and a bool are not iterable. Plan decision F8.
    #[test]
    fn shapes_that_is_not_a_list_cannot_be_parsed() {
        for table in [
            r#"{"shapes": null}"#,
            r#"{"shapes": 7}"#,
            r#"{"shapes": true}"#,
            r#"{"shapes": "abc"}"#,
            r#"{"shapes": {"id": "a", "class": "b", "pattern": "c"}}"#,
        ] {
            assert_eq!(parse_shapes(table.as_bytes()), Err(TableFault::NotParsable), "{table}");
        }
    }

    // Plan decision F8: python raises KeyError, the child exits 1, the bash says
    // "could not be parsed".
    #[test]
    fn an_entry_without_id_class_or_pattern_cannot_be_parsed() {
        for entry in [
            r#"{"class": "transient", "pattern": "boom"}"#,
            r#"{"id": "a", "pattern": "boom"}"#,
            r#"{"id": "a", "class": "transient"}"#,
            "{}",
        ] {
            assert_eq!(parse_shapes(&ok_table(entry)), Err(TableFault::NotParsable), "{entry}");
        }
    }

    // `"\t".join(...)` raises TypeError for a field that is not a string.
    #[test]
    fn an_entry_with_a_field_that_is_not_a_string_cannot_be_parsed() {
        for entry in [
            r#"{"id": 1, "class": "t", "pattern": "p"}"#,
            r#"{"id": "a", "class": null, "pattern": "p"}"#,
            r#"{"id": "a", "class": "t", "pattern": ["p"]}"#,
        ] {
            assert_eq!(parse_shapes(&ok_table(entry)), Err(TableFault::NotParsable), "{entry}");
        }
    }

    // Python indexes `sh["id"]` and fails on a list, a string and null, while a
    // serde struct would accept a three-element array. The port must not.
    #[test]
    fn an_entry_that_is_not_an_object_cannot_be_parsed() {
        for entry in [r#"["a", "t", "p"]"#, r#""abc""#, "null", "5"] {
            assert_eq!(parse_shapes(&ok_table(entry)), Err(TableFault::NotParsable), "{entry}");
        }
    }

    // The python child prints as it goes, but the bash keeps nothing from a child
    // that exited non-zero. Plan decision F8.
    #[test]
    fn one_bad_entry_after_good_ones_discards_the_whole_table() {
        let table = ok_table(&format!(r#"{ONE}, {ONE}, {{"id": "z"}}"#));
        assert_eq!(parse_shapes(&table), Err(TableFault::NotParsable));
    }

    #[test]
    fn text_that_is_not_a_json_object_cannot_be_parsed() {
        let cases: [&[u8]; 9] = [
            b"this is not json",
            b"",
            b"   \n",
            b"[]",
            b"[{\"id\":\"a\",\"class\":\"t\",\"pattern\":\"p\"}]",
            b"\"shapes\"",
            b"null",
            br#"{"shapes": [],}"#,
            b"{\"shapes\": [",
        ];
        for case in cases {
            assert_eq!(parse_shapes(case), Err(TableFault::NotParsable), "{case:?}");
        }
    }

    // Python reads the file as UTF-8 text, so a BOM and invalid bytes anywhere,
    // even in a key nobody uses, fail the whole read.
    #[test]
    fn a_bom_or_invalid_utf8_cannot_be_parsed() {
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(&ok_table(ONE));
        assert_eq!(parse_shapes(&bom), Err(TableFault::NotParsable));
        let mut bad = br#"{"x": ""#.to_vec();
        bad.extend_from_slice(&[0xFF, b'"', b',']);
        bad.extend_from_slice(br#""shapes": []}"#);
        assert_eq!(parse_shapes(&bad), Err(TableFault::NotParsable));
    }

    #[test]
    fn fields_other_than_id_class_and_pattern_are_ignored() {
        let table = br#"{"version": 1, "classes": {"x": [1, 2]}, "shapes": [
            {"id": "a", "class": "t", "pattern": "p", "means": "m", "fix": null}]}"#;
        assert_eq!(parse_shapes(table), Ok(vec![shape("a", "t", "p")]));
    }

    // json.load keeps the last of two equal keys. A serde struct would refuse the
    // duplicate, which is why the port goes through `Value` first.
    #[test]
    fn the_last_of_two_equal_keys_wins_as_it_does_in_python() {
        let table = br#"{"shapes": [], "shapes": [{"id": "a", "class": "t", "pattern": "p"}]}"#;
        assert_eq!(parse_shapes(table), Ok(vec![shape("a", "t", "p")]));
        let entry = br#"{"shapes": [{"id": "a", "id": "b", "class": "t", "pattern": "p"}]}"#;
        assert_eq!(parse_shapes(entry), Ok(vec![shape("b", "t", "p")]));
    }

    // Decision P1F-4: python's json accepts NaN, Infinity and a lone surrogate
    // escape in an unused field. serde_json refuses them, so the classifier says
    // it could not read the table. The failure is closed and no shipped table
    // holds any of them.
    #[test]
    fn json_that_python_accepts_and_serde_refuses_reads_as_unparsable() {
        for table in [
            r#"{"v": NaN, "shapes": [{"id": "a", "class": "t", "pattern": "p"}]}"#,
            r#"{"v": Infinity, "shapes": [{"id": "a", "class": "t", "pattern": "p"}]}"#,
            r#"{"v": "\ud800", "shapes": [{"id": "a", "class": "t", "pattern": "p"}]}"#,
        ] {
            assert_eq!(parse_shapes(table.as_bytes()), Err(TableFault::NotParsable), "{table}");
        }
    }

    // Python reads 1e999 as infinity and keeps going. serde_json with
    // arbitrary_precision keeps the digits, so the table still parses.
    #[test]
    fn a_number_too_large_for_a_float_in_an_unused_field_is_accepted() {
        let table = br#"{"v": 1e999, "w": 123456789012345678901234567890, "shapes": [
            {"id": "a", "class": "t", "pattern": "p"}]}"#;
        assert_eq!(parse_shapes(table), Ok(vec![shape("a", "t", "p")]));
    }

    // `IFS=<TAB> read -r id cls pat`. Every row below was read by bash 5.3.
    #[test]
    fn split_line_matches_what_bash_read_assigns() {
        let cases: [(&str, (&str, &str, &str)); 11] = [
            ("a\tb\tc", ("a", "b", "c")),
            ("\tb\tc", ("b", "c", "")),
            ("a\t\tc", ("a", "c", "")),
            ("a\tb\t", ("a", "b", "")),
            ("a\tb\tc\td", ("a", "b", "c\td")),
            ("e f\tg h\ti j", ("e f", "g h", "i j")),
            ("a\tb\t  c  \t\t", ("a", "b", "  c  ")),
            ("a\t\t\t", ("a", "", "")),
            ("", ("", "", "")),
            ("\t\t\t", ("", "", "")),
            ("a\tb\tc\t\td\t", ("a", "b", "c\t\td")),
        ];
        for (line, want) in cases {
            assert_eq!(split_line(line), want, "{line:?}");
        }
    }

    // A tab inside the id shifts every field one place to the right.
    #[test]
    fn a_tab_inside_the_id_moves_the_class_into_the_id_slot() {
        let rows = read_rows(&[shape("x\ty", "needs_human", "boom")]);
        assert_eq!(
            rows,
            vec![Row { id: "x".into(), class: "y".into(), pattern: "needs_human\tboom".into() }]
        );
    }

    // An empty class collapses two tabs into one separator, so the pattern lands
    // in the class slot and the pattern slot is empty. grep -o on an empty
    // pattern prints nothing, so the row never matches.
    #[test]
    fn an_empty_class_puts_the_pattern_in_the_class_slot() {
        let rows = read_rows(&[shape("a", "", "error")]);
        assert_eq!(
            rows,
            vec![Row { id: "a".into(), class: "error".into(), pattern: String::new() }]
        );
    }

    // A newline in a pattern ends the bash line there. The rest becomes a second
    // line whose first field is the id.
    #[test]
    fn a_newline_inside_a_pattern_starts_a_second_row_with_the_tail_as_its_id() {
        let rows = read_rows(&[shape("a", "t", "one\ntwo")]);
        assert_eq!(
            rows,
            vec![
                Row { id: "a".into(), class: "t".into(), pattern: "one".into() },
                Row { id: "two".into(), class: String::new(), pattern: String::new() },
            ]
        );
    }

    // Command substitution drops NUL bytes, with a warning on stderr.
    #[test]
    fn nul_bytes_in_a_pattern_are_dropped_the_way_command_substitution_drops_them() {
        let rows = read_rows(&[shape("a", "t", "x\0y")]);
        assert_eq!(rows, vec![Row { id: "a".into(), class: "t".into(), pattern: "xy".into() }]);
    }

    #[test]
    fn rows_keep_the_order_of_the_shapes() {
        let rows = read_rows(&[shape("a", "t", "1"), shape("b", "u", "2"), shape("c", "v", "3")]);
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }
}
