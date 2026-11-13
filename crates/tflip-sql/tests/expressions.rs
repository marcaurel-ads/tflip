//! The result column types and errors of the single expressions the
//! original engine was asked for (`expr` lines of
//! `fixtures/sql/statements.json`): `SELECT <expr> FROM system.iota`.
//! Values are compared where the engine formats them, in tflip-ace.

use std::path::Path;

use serde_json::Value as Json;
use tflip_sql::ast::Statement;
use tflip_sql::{Env, NoRow, SqlError, eval, parse, plan};

fn run(sql: &str) -> Result<(u16, u32, u16), SqlError> {
    let parsed = parse(sql)?;
    let Statement::Select(select) = parsed.statement else {
        panic!("{sql}")
    };
    let plan = plan(&select, None, parsed.params.len())?;
    let column = &plan.columns[0];
    let value = eval(&column.node, &mut NoRow, &Env::default())?;
    value.to_field(column.ty, &column.name)?;
    let (field_type, length, decimals) = column.field;
    Ok((field_type.code(), u32::from(length), decimals))
}

#[test]
fn expression_types_match_the_original_engine() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sql/statements.json");
    let cases: Vec<Json> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut failures = Vec::new();
    let mut checked = 0;
    // Syntax errors the original engine words or places otherwise.
    let known = [
        "DATE '2026-09-29'",
        "1 != 1",
        "SUBSTRING('hello' FROM 2 FOR 3)",
        "1 +",
    ];
    for case in &cases {
        let cmd = case["cmd"].as_str().unwrap();
        let Some(expr) = cmd.strip_prefix("expr ") else {
            continue;
        };
        if known.contains(&expr) {
            continue;
        }
        checked += 1;
        let sql = format!("SELECT {expr} FROM system.iota");
        let got = run(&sql);
        let rc = case["rc"].as_u64().unwrap();
        match (rc, got) {
            (0, Ok(got)) => {
                let field = &case["cursor"]["fields"][0];
                let want = (
                    field[1].as_u64().unwrap() as u16,
                    field[2].as_u64().unwrap() as u32,
                    field[3].as_u64().unwrap() as u16,
                );
                if got != want {
                    failures.push(format!("{expr}: type {got:?}, want {want:?}"));
                }
            }
            (0, Err(e)) => failures.push(format!("{expr}: fails {e}, want success")),
            (7200, Err(e)) => {
                let error = case["error"].as_str().unwrap();
                let want = format!("Error 7200:  {e}");
                if !error.contains(&format!("NativeError = {};", e.native)) {
                    failures.push(format!("{expr}: native {}, want {error}", e.native));
                } else if want != error {
                    failures.push(format!("{expr}: message\n  {want}\n  {error}"));
                }
            }
            (_, got) => failures.push(format!("{expr}: {got:?}, want rc {rc}")),
        }
    }
    assert!(checked > 150);
    assert!(
        failures.is_empty(),
        "{} of {checked} differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// `fixtures/sql/functions.json`: the type of each function's result, by
/// its name, or the error.
#[test]
fn function_types_match_the_original_engine() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sql/functions.json");
    let cases: serde_json::Map<String, Json> =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let code = |name: &str| match name {
        "Logical" => 1,
        "Numeric" => 2,
        "Date" => 3,
        "Character" => 4,
        "Memo" => 5,
        "Double" => 10,
        "Integer" => 11,
        "ShortInt" => 12,
        "Time" => 13,
        "TimeStamp" => 14,
        "Money" => 18,
        "LongInt" => 19,
        "VarCharFox" => 23,
        "VarBinaryFox" => 24,
        "GUID" => 29,
        other => panic!("type {other}"),
    };
    // Full-text search, aggregates and what needs a connection are not
    // tflip's SQL yet.
    let skipped = |expr: &str| {
        expr.starts_with("__agg")
            || [
                "CONTAINS",
                "SCORE",
                "NEWID",
                "DATABASE",
                "USER",
                "RAND",
                "OINDEXOF",
                "OSUBSTRING",
                "DESCEND",
                "ISNULL('a'",
                "Date'",
            ]
            .iter()
            .any(|s| expr.contains(s))
    };
    let mut failures = Vec::new();
    for (expr, want) in &cases {
        if skipped(expr) {
            continue;
        }
        let got = run(&format!("SELECT {expr} FROM system.iota"));
        match (
            want.get("type").and_then(Json::as_str),
            want.get("error"),
            got,
        ) {
            (Some(name), _, Ok((field_type, ..))) if field_type == code(name) => {}
            (Some(name), _, got) => failures.push(format!("{expr}: {got:?}, want {name}")),
            (None, Some(error), Err(e)) if error.as_u64() == Some(u64::from(e.native)) => {}
            (None, error, got) => failures.push(format!("{expr}: {got:?}, want error {error:?}")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
