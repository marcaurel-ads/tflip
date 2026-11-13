//! Replays what the original engine made of the expressions in
//! `fixtures/expr/expressions.json` (see `tools/ace/expr/`) on the same
//! records, as the `AdsEval*Expr` functions report it.

use std::path::{Path, PathBuf};

use serde_json::Value as Json;
use tflip_expr::{Env, Expr, Failure, Field, Row, Schema, Type, Val, number};
use tflip_storage::table::{self, Format, TableFile};
use tflip_storage::value::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

struct Table {
    alias: String,
    adt: TableFile,
}

impl Schema for Table {
    fn alias(&self) -> &str {
        &self.alias
    }

    fn field(&self, name: &str) -> Option<Field> {
        let fields = &self.adt.structure.fields;
        let index = fields
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(name))?;
        let f = &fields[index];
        Some(Field {
            index,
            field_type: f.field_type,
            length: f.length(),
            decimals: f.decimals(),
        })
    }
}

struct Record<'t> {
    table: &'t mut TableFile,
    bytes: Vec<u8>,
    number: u32,
}

impl Row for Record<'_> {
    type Error = table::Error;

    fn value(&mut self, index: usize) -> Result<Value, table::Error> {
        let field = self.table.structure.fields[index].clone();
        field.value(&self.bytes, self.table.memo())
    }

    fn record_number(&self) -> u32 {
        self.number
    }

    fn deleted(&self) -> bool {
        false
    }
}

/// A double as C's `%.17g` writes it.
fn g17(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() { "-0" } else { "0" }.into();
    }
    let scientific = format!("{value:.16e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap();
    let exponent: i32 = exponent.parse().unwrap();
    let trim = |s: String| {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    };
    if !(-4..17).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", trim(mantissa.to_string()), exponent.abs())
    } else {
        trim(format!("{value:.*}", (16 - exponent) as usize))
    }
}

/// What the three `AdsEval*Expr` functions return for `expr` on the
/// record `row`: `[logical, numeric, string]`.
fn results(expr: &Expr, row: &mut Record, env: &Env) -> [String; 3] {
    let ty = expr.ty();
    let wrong = || "E5011".to_string();
    let value = expr.eval(row, env);
    let value = match value {
        Ok(value) => value,
        Err(Failure::Runtime(e)) => {
            let code = format!("E{}", e.code());
            let pick = |ok: bool| if ok { code.clone() } else { wrong() };
            return [
                pick(ty == Type::Logical),
                pick(matches!(ty, Type::Number(_) | Type::Date)),
                pick(!matches!(ty, Type::Number(_) | Type::Logical)),
            ];
        }
        Err(Failure::Row(e)) => panic!("reading the record: {e:?}"),
    };
    let logical = match (ty, &value) {
        (Type::Logical, Val::Logical(true)) => "T".into(),
        (Type::Logical, _) => "F".into(),
        _ => wrong(),
    };
    let numeric = match (ty, &value) {
        (Type::Number(f), Val::Number(n)) if f.fixed && !number::fits(*n, f.length, f.decimals) => {
            "E4504".into()
        }
        (Type::Number(_), Val::Number(n)) => g17(*n),
        (Type::Date, Val::Date(j)) => j.to_string(),
        (Type::Number(_) | Type::Date, _) => "0".into(),
        _ => wrong(),
    };
    let string = match value.bytes(ty) {
        Some(bytes) if !matches!(ty, Type::Number(_) | Type::Logical) => {
            bytes.iter().map(|b| format!("{b:02x}")).collect()
        }
        _ => wrong(),
    };
    [logical, numeric, string]
}

/// Whether `actual` matches what the original engine `expected` on a
/// result that is `null` or not. For a NULL result the original engine
/// returns whatever number it left in memory, and zero bytes of a length
/// that does not always follow from the expression's type.
fn matches(key: &str, expected: &str, actual: &str, null: bool) -> bool {
    let zeros = |s: &str| s.bytes().all(|c| c == b'0');
    expected == actual
        || null && key == "N" && !expected.starts_with('E')
        || null && key == "S" && zeros(expected) && zeros(actual)
}

/// Whether the original engine's text result for `expr` is a collation
/// key, which tflip does not make: the Unicode fields (NChar, NVarChar)
/// come back as ICU sort keys.
fn unicode(expr: &str) -> bool {
    let expr = expr.to_ascii_lowercase();
    expr.contains("nchar") || expr.contains("nvarchar")
}

/// The expression text as the engine got it: the fixture writes its
/// Windows-1252 bytes as U+0000–U+00FF.
fn ansi(text: &str) -> String {
    let bytes: Vec<u8> = text.chars().map(|c| c as u32 as u8).collect();
    tflip_storage::cp1252::decode(&bytes)
}

#[test]
fn expressions_match_the_original_engine() {
    let cases: Vec<Json> =
        serde_json::from_slice(&std::fs::read(fixtures().join("expr/expressions.json")).unwrap())
            .unwrap();
    let mut failures = Vec::new();
    for case in &cases {
        let file = case["table"].as_str().unwrap();
        let path = fixtures().join("adt").join(file);
        let alias = path.file_stem().unwrap().to_string_lossy().into_owned();
        let mut table = Table {
            alias,
            adt: TableFile::open(&path, Format::Adt).unwrap(),
        };
        let text = ansi(case["expr"].as_str().unwrap());
        let env = Env {
            exact: case["exact"].as_bool().unwrap(),
            date_format: case["date_format"].as_str().unwrap().into(),
            epoch: tflip_expr::text::DEFAULT_EPOCH,
            now: (2461327, 0),
            utc: (2461327, 0),
            case_fold: None,
            weights: None,
            dynamic: None,
            blank_empty: false,
        };
        let mut fail = |what: String| failures.push(format!("{text:40} {what}"));
        let expr = Expr::compile(&text, &table);
        let (valid, valid_error, ty) = match &expr {
            Ok(expr) => (
                "T",
                0,
                expr.ty()
                    .ace_type()
                    .map_or("E5011".into(), |t| t.to_string()),
            ),
            Err(kind) => ("F", kind.code(3400), format!("E{}", kind.code(3500))),
        };
        let expected_type = match &case["type"] {
            Json::String(s) => s.clone(),
            n => n.to_string(),
        };
        if valid != case["valid"]
            || u64::from(valid_error) != case["valid_error"]
            || ty != expected_type
        {
            fail(format!(
                "valid {valid}/{valid_error} type {ty}, expected {}/{} type {expected_type}",
                case["valid"], case["valid_error"]
            ));
            continue;
        }
        let Ok(expr) = expr else { continue };
        for record in case["records"].as_array().unwrap() {
            if record["eof"].as_bool().unwrap() {
                continue;
            }
            let number = record["recno"].as_u64().unwrap() as u32;
            let length = table.adt.structure.header.record_length as usize;
            let mut bytes = vec![0; length];
            table.adt.read(number, &mut bytes).unwrap();
            let mut row = Record {
                table: &mut table.adt,
                bytes,
                number,
            };
            let actual = results(&expr, &mut row, &env);
            let null = expr.eval(&mut row, &env).is_ok_and(|v| v == Val::Null);
            for (key, actual) in ["L", "N", "S"].iter().zip(&actual) {
                let expected = record[key].as_str().unwrap();
                if *key == "S" && unicode(&text) {
                    continue;
                }
                if !matches(key, expected, actual, null) {
                    fail(format!(
                        "record {number} {key}: {actual}, expected {expected}"
                    ));
                }
            }
        }
    }
    for failure in &failures {
        eprintln!("{failure}");
    }
    assert!(failures.is_empty(), "{} mismatches", failures.len());
}
