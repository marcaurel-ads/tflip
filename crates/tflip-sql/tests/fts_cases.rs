//! Replays the full-text `CONTAINS` cases of `fixtures/sql/fts.json` that run on
//! a default full-text index (docs.body), on a field with no index (docs.title,
//! nofts.c), or over all indexed fields (`*`), against the query engine in
//! `tflip_sql::fts`. The documents are the ones `tools/sql/fts_check.py` inserts.
//! Cases that need per-index parsing options (opts, fixed), SCORE, SHOW PLAN, or
//! a non-literal argument are left to later work.

use std::collections::BTreeSet;

use serde_json::Value;
use tflip_sql::fts::{self, Rules};

/// docs: (id, title, body). body has a default FTS index; title has none.
const DOCS: &[(i64, &str, Option<&str>)] = &[
    (
        1,
        "Medical doctors",
        Some(
            "Both medical doctors and doctors of veterinary medicine require extensive schooling.",
        ),
    ),
    (
        2,
        "Alpine",
        Some("We went alpine skiing in the mountains; skiing alpine is fun."),
    ),
    (
        3,
        "Locks",
        Some("The locker had an antilock blocking mechanism. O'Malley said 1,423.99 dollars."),
    ),
    (4, "Null body", None),
    (
        5,
        "Birthday",
        Some("Happy birthday card for a birthday party, card enclosed."),
    ),
    (
        6,
        "Alphabet",
        Some("alpha x y z of the bravo charlie delta echo foxtrot golf hotel india juliet"),
    ),
    (7, "Repeats", Some("alpha bravo alpha")),
];

/// nofts: (id, c). No FTS index at all.
const NOFTS: &[(i64, &str)] = &[(1, "hello world"), (2, "alpha x y z of the bravo")];

/// Extracts (table, field, condition) from a `SELECT id FROM t WHERE
/// CONTAINS(field, 'cond')[ ORDER BY id]`, or `None` if the key is not that shape
/// (SCORE, a non-literal argument, several predicates, …).
fn parse_case(sql: &str) -> Option<(String, String, String)> {
    let rest = sql.strip_prefix("SELECT id FROM ")?;
    let (table, rest) = rest.split_once(" WHERE CONTAINS(")?;
    let (field, rest) = rest.split_once(", ")?;
    // The condition must be a single quoted literal, then `)`.
    let rest = rest.strip_prefix('\'')?;
    let bytes: Vec<char> = rest.chars().collect();
    let mut cond = String::new();
    let mut i = 0;
    loop {
        let c = *bytes.get(i)?;
        if c == '\'' {
            if bytes.get(i + 1) == Some(&'\'') {
                cond.push('\'');
                i += 2;
                continue;
            }
            break; // closing quote
        }
        cond.push(c);
        i += 1;
    }
    // After the closing quote: `)` then nothing or ` ORDER BY id`.
    let tail: String = bytes[i + 1..].iter().collect();
    let tail = tail.strip_prefix(')')?;
    if !(tail.is_empty() || tail == " ORDER BY id") {
        return None;
    }
    Some((table.to_owned(), field.to_owned(), cond))
}

/// Whether a query is "fully optimized" by the index: no NEAR and no phrase.
fn fully_optimized(q: &fts::Query) -> bool {
    use fts::Query::*;
    match q {
        Word { .. } => true,
        Phrase(_) | Near(..) => false,
        Not(a) => fully_optimized(a),
        And(a, b) | Or(a, b) => fully_optimized(a) && fully_optimized(b),
    }
}

/// The ids the condition selects over our documents, or the native error number.
fn evaluate(table: &str, field: &str, cond: &str) -> Result<Vec<i64>, u32> {
    let indexed = Rules::indexed_default();
    let non_indexed = Rules::non_indexed();
    // Parse once with the rules the target field uses (* and body are indexed).
    let rules = match (table, field) {
        ("docs", "body") | ("docs", "*") => &indexed,
        ("docs", "title") | ("nofts", "c") => &non_indexed,
        _ => return Err(0), // unsupported target for this test
    };
    let query = fts::parse(cond, rules).map_err(|e| e.0)?;
    let mut ids = Vec::new();
    // The FTS index enumerates every row, so a fully-optimized NOT over the index
    // matches rows with no words (a NULL body); a condition with NEAR or a phrase
    // is "not fully optimized" and drops them.
    let optimized = fully_optimized(&query);
    match table {
        "docs" => {
            for &(id, title, body) in DOCS {
                let text = match field {
                    "body" | "*" => body,
                    "title" => Some(title),
                    _ => None,
                };
                let words = match text {
                    Some(text) => fts::tokenize(text, rules),
                    None if field == "title" => Vec::new(), // title is never NULL
                    None if optimized => Vec::new(),
                    None => continue, // NULL body, not fully optimized: dropped
                };
                if fts::matches(&query, &words) {
                    ids.push(id);
                }
            }
        }
        "nofts" => {
            for &(id, c) in NOFTS {
                let words = fts::tokenize(c, rules);
                if fts::matches(&query, &words) {
                    ids.push(id);
                }
            }
        }
        _ => return Err(0),
    }
    Ok(ids)
}

#[test]
fn contains_cases_match_the_engine() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sql/fts.json"),
    )
    .unwrap();
    let fixture: serde_json::Map<String, Value> = serde_json::from_str(&text).unwrap();

    let mut checked = 0;
    let mut skipped = 0;
    for (sql, expected) in &fixture {
        let Some((table, field, cond)) = parse_case(sql) else {
            skipped += 1;
            continue;
        };
        // Only the targets this test knows (default index / no index).
        if !matches!(
            (table.as_str(), field.as_str()),
            ("docs", "body") | ("docs", "title") | ("docs", "*") | ("nofts", "c")
        ) {
            skipped += 1;
            continue;
        }

        let got = evaluate(&table, &field, &cond);
        if let Some(err) = expected.get("error").and_then(Value::as_u64) {
            assert_eq!(got, Err(err as u32), "error for {sql}");
        } else if let Some(rows) = expected.get("rows").and_then(Value::as_array) {
            let want: BTreeSet<i64> = rows
                .iter()
                .map(|r| r[0].as_str().unwrap().parse::<i64>().unwrap())
                .collect();
            let got = got.unwrap_or_else(|e| panic!("{sql}: engine wanted rows, we errored {e}"));
            let got: BTreeSet<i64> = got.into_iter().collect();
            assert_eq!(got, want, "rows for {sql}");
        } else {
            continue;
        }
        checked += 1;
    }
    eprintln!("fts: {checked} CONTAINS cases checked, {skipped} skipped");
    assert!(checked > 50, "expected to check many cases, only {checked}");
}
