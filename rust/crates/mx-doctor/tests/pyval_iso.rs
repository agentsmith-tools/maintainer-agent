//! `iso_epoch` against Python 3.14 `datetime.fromisoformat`: every row was produced by
//! running Python on the text, so a row is what Python answered, not what this crate expects.

#![allow(clippy::unreadable_literal)]

use mx_doctor::pyval::iso_epoch;

const TABLE: &[(&str, Option<f64>)] = &[
    ("2026-01-05T12:00:00Z", Some(1767614400.0)),
    ("2026-02-30T00:00:00Z", None),
    ("0000-01-01T00:00:00Z", None),
    ("2026-01-05T24:00:00Z", Some(1767657600.0)),
    ("2026-01-05T00Z", Some(1767571200.0)),
    ("2026-W02-1T00:00:00Z", Some(1767571200.0)),
    ("2026W021T00:00:00Z", Some(1767571200.0)),
    ("2026-01-05T12:00:00+01:00:30", Some(1767610770.0)),
    ("2026-01-05T12:00:00,5Z", Some(1767614400.5)),
    ("20260105T120000Z", Some(1767614400.0)),
    ("2026-01-05 12:00:00+0000", Some(1767614400.0)),
    ("2026-01-05T12:00Z", Some(1767614400.0)),
    ("2026-01-05T12Z", Some(1767614400.0)),
    ("2026-01-05T24:00:01Z", None),
    ("2026-W53-1T00:00:00Z", Some(1798416000.0)),
    ("2020-W53-1T00:00:00Z", Some(1609113600.0)),
    ("2026-W01T00:00:00Z", Some(1766966400.0)),
    ("2024-02-29T00:00:00Z", Some(1709164800.0)),
    ("2023-02-29T00:00:00Z", None),
    ("2026-01-05T12:00:00.123456789Z", Some(1767614400.123456)),
    ("2026-01-05", None),
    ("2026-01-05T12:00:00", None),
    ("2026-01-05T25:00:00Z", None),
    ("2026-01-05T12:61:00Z", None),
    ("2026-01-05T12:00:00+24:00", None),
    ("2026-01-05T12:00:00-05:30", Some(1767634200.0)),
    ("2026-01-05T1200Z", Some(1767614400.0)),
    ("2026-01-05T120000Z", Some(1767614400.0)),
    ("2026-01-05T12:00:00+01", Some(1767610800.0)),
    ("2026-01-05T12:00:60Z", None),
    ("2026-W00-1T00:00:00Z", None),
    ("2026-W54-1T00:00:00Z", None),
    ("2026-W10-0T00:00:00Z", None),
    ("2026-W10-8T00:00:00Z", None),
    ("2000-02-29T00:00:00Z", Some(951782400.0)),
    ("1900-02-29T00:00:00Z", None),
    ("2100-02-29T00:00:00Z", None),
    ("2400-02-29T00:00:00Z", Some(13574563200.0)),
];

#[test]
fn every_text_reads_as_python_reads_it() {
    assert!(TABLE.len() >= 30);
    for (text, want) in TABLE {
        let got = iso_epoch(text);
        match (got, want) {
            (Some(a), Some(b)) => assert!((a - b).abs() < 1e-5, "{text}: {a} vs {b}"),
            (None, None) => {}
            _ => panic!("{text}: got {got:?}, Python gave {want:?}"),
        }
    }
}
