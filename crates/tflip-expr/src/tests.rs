//! Expressions on a small table of their own; the original engine's
//! results on the fixtures are checked by the engine's tests.

use tflip_storage::field::FieldType;
use tflip_storage::value::Value;

use super::*;

struct Table;

const FIELDS: [(&str, FieldType, u16, u16); 7] = [
    ("name", FieldType::Character, 6, 0),
    ("ci", FieldType::CiCharacter, 6, 0),
    ("n", FieldType::Integer, 4, 0),
    ("born", FieldType::Date, 4, 0),
    ("ok", FieldType::Logical, 1, 0),
    ("Query Number", FieldType::Integer, 4, 0),
    ("Seconds Until Finished", FieldType::Integer, 4, 0),
];

impl Schema for Table {
    fn alias(&self) -> &str {
        "people"
    }

    fn field(&self, name: &str) -> Option<Field> {
        let index = FIELDS.iter().position(|f| f.0.eq_ignore_ascii_case(name))?;
        let (_, field_type, length, decimals) = FIELDS[index];
        Some(Field {
            index,
            field_type,
            length,
            decimals,
        })
    }
}

struct Record(Vec<Value>);

impl Row for Record {
    type Error = ();

    fn value(&mut self, index: usize) -> Result<Value, ()> {
        Ok(self.0[index].clone())
    }

    fn record_number(&self) -> u32 {
        7
    }

    fn deleted(&self) -> bool {
        false
    }
}

fn record() -> Record {
    Record(vec![
        Value::Text("abc   ".into()),
        Value::Text("MiXed ".into()),
        Value::Integer(-1),
        Value::Date(2458066),
        Value::Null,
        Value::Integer(2),
        Value::Integer(30),
    ])
}

fn env(exact: bool) -> Env {
    Env {
        exact,
        date_format: "MM/DD/CCYY".into(),
        epoch: crate::text::DEFAULT_EPOCH,
        now: (2458066, 0),
        utc: (2458066, 0),
        case_fold: None,
        weights: None,
        dynamic: None,
        blank_empty: false,
        contains: None,
    }
}

fn eval_with(text: &str, exact: bool) -> Result<Val, ErrorKind> {
    let expr = Expr::compile(text, &Table)?;
    Ok(expr.eval(&mut record(), &env(exact)).unwrap())
}

fn eval(text: &str) -> Val {
    eval_with(text, false).unwrap()
}

fn text(text: &str) -> Val {
    Val::Str(text.as_bytes().to_vec())
}

#[test]
fn newidstring_variants() {
    let bytes = |v: Val| match v {
        Val::Str(b) => b,
        other => panic!("not a string: {other:?}"),
    };
    // The result is a VARCHAR(38) whatever the format.
    let expr = Expr::compile("NEWIDSTRING('D')", &Table).unwrap();
    assert_eq!(
        expr.ty(),
        Type::String {
            width: 38,
            ci: false
        }
    );
    // D: 36-char delimited hex.
    let d = bytes(eval("NEWIDSTRING('D')"));
    assert_eq!(d.len(), 36);
    assert_eq!((d[8], d[13], d[18], d[23]), (b'-', b'-', b'-', b'-'));
    assert!(d.iter().all(|&c| c == b'-' || c.is_ascii_hexdigit()));
    // N / NUMBERS: 32 hex digits, no separators; case-insensitive long name.
    for f in ["N", "NUMBERS", "numbers"] {
        let n = bytes(eval(&format!("NEWIDSTRING('{f}')")));
        assert_eq!(n.len(), 32, "{f}");
        assert!(n.iter().all(u8::is_ascii_hexdigit), "{f}");
    }
    // B / P: the delimited form wrapped, 38 characters.
    let b = bytes(eval("NEWIDSTRING('B')"));
    assert_eq!((b.len(), b[0], b[37]), (38, b'[', b']'));
    let p = bytes(eval("NEWIDSTRING('PARENTHESIS')"));
    assert_eq!((p.len(), p[0], p[37]), (38, b'(', b')'));
    // M / F: Base64, 24 (padded) and 22 (url-safe, no padding) bytes.
    assert_eq!(bytes(eval("NEWIDSTRING('M')")).len(), 24);
    let f = bytes(eval("NEWIDSTRING('F')"));
    assert_eq!(f.len(), 22);
    assert!(!f.contains(&b'=') && !f.contains(&b'+') && !f.contains(&b'/'));
    // No argument is a parameter-count error (3402); a non-string argument a
    // type mismatch (3409); an unknown format fails at eval with 4407.
    assert_eq!(
        Expr::compile("NEWIDSTRING()", &Table),
        Err(ErrorKind::ParameterCount)
    );
    assert_eq!(
        Expr::compile("NEWIDSTRING(5)", &Table),
        Err(ErrorKind::TypeMismatch)
    );
    let x = Expr::compile("NEWIDSTRING('MIME')", &Table).unwrap();
    assert_eq!(
        x.eval(&mut record(), &env(false)),
        Err(Failure::Runtime(Runtime::GuidFormat))
    );
}

#[test]
fn arithmetic_and_precedence() {
    assert_eq!(eval("2+3*4"), Val::Number(14.0));
    assert_eq!(eval("2^3^2"), Val::Number(64.0));
    assert_eq!(eval("-2^2"), Val::Number(4.0));
    assert_eq!(eval("--n"), Val::Number(1.0));
    assert_eq!(eval("-"), Val::Number(0.0));
    assert_eq!(eval("7.5%2"), Val::Number(1.5));
    assert_eq!(
        Expr::compile("-n", &Table),
        Err(ErrorKind::UnsupportedOperator)
    );
    let expr = Expr::compile("1/0", &Table).unwrap();
    assert_eq!(
        expr.eval(&mut record(), &env(false)),
        Err(Failure::Runtime(Runtime::DivideByZero))
    );
}

#[test]
fn transform_pictures_with_literals_lose_a_digit_as_the_original_does() {
    // Answers of the original engine (tools/ace/expr/iif.c).
    let rows = [
        ("5", "999%", "   %"),
        ("50", "999%", "  5%"),
        ("100", "999%", " 10%"),
        ("999", "999%", " 99%"),
        ("12.5", "999%", "  1%"),
        ("-5", "999%", "  -%"),
        ("100", "@R 99%", "10%"),
        ("50", "@B 999%", "5%"),
        ("5", "@BR 999%", "%"),
        ("100", "@BR 999%", "10%"),
        ("1234", "@BR 99%", "**%"),
        ("1234", "@B 999%", "123%"),
        ("50", "999", " 50"),
        ("12.5", "@B 999", "13"),
        ("5", "@B 999", "5"),
        ("1234", "999", "***"),
        ("50", "999-999", "   -  5"),
        ("1234", "999-999", "   -123"),
        ("100", "%999", "% 10"),
        ("1234", "9%99", "1%23"),
        ("50", "999.99%", " 50.00%"),
        ("12.5", "999.99%", " 12.50%"),
        ("1234", "999.99%", "***.**%"),
        ("50", "99.9%", "50.0%"),
        ("100", "99.9%", "**.*%"),
    ];
    for (value, picture, want) in rows {
        let got = eval(&format!("TRANSFORM({value}, \"{picture}\")"));
        assert_eq!(got, text(want), "{value} in {picture}");
    }
}

#[test]
fn iif_and_coalesce_evaluate_every_argument() {
    // The original fails on a division by 0 in a branch it does not take
    // (tools/ace/expr/iif.c).
    for text in [
        "IIF(.T.,1,1/0)",
        "IIF(.F.,1/0,1)",
        "IF(n>5,1/0,2)",
        "COALESCE(n,1/0)",
    ] {
        let expr = Expr::compile(text, &Table).unwrap();
        assert_eq!(
            expr.eval(&mut record(), &env(false)),
            Err(Failure::Runtime(Runtime::DivideByZero)),
            "{text}"
        );
    }
    assert_eq!(eval("IIF(n<0,5/(0-n),0)"), Val::Number(5.0));
}

#[test]
fn upper_and_lower_use_the_collation_case_fold() {
    // Under the default collation (no case fold), ASCII folding.
    assert_eq!(eval("UPPER(name)"), text("ABC   "));
    assert_eq!(eval("LOWER(name)"), text("abc   "));
    // A static collation folds each byte through its tables: here UPPER maps
    // every byte to 0xAA and LOWER to 0xBB.
    let fold = CaseFold {
        upper: [0xAA; 256],
        lower: [0xBB; 256],
    };
    let env = Env {
        case_fold: Some(fold),
        ..env(false)
    };
    let upper = Expr::compile("UPPER(name)", &Table).unwrap();
    assert_eq!(
        upper.eval(&mut record(), &env).unwrap(),
        Val::Str(vec![0xAA; 6])
    );
    let lower = Expr::compile("LOWER(name)", &Table).unwrap();
    assert_eq!(
        lower.eval(&mut record(), &env).unwrap(),
        Val::Str(vec![0xBB; 6])
    );
}

#[test]
fn strings_compare_as_the_exact_setting_says() {
    for (text, loose, exact) in [
        ("'abc' = 'ab'", true, false),
        ("'ab' = 'abc'", false, false),
        ("'abc ' = 'abc'", true, true),
        ("'abc' = 'abc '", false, true),
        ("'abc' = ''", true, false),
        ("'abc' == 'abc '", false, false),
        ("'abc' > 'ab'", false, true),
        ("'ab' < 'abc'", true, true),
        ("'b' $ 'abc'", true, true),
        ("ci = 'mixed'", true, true),
        ("name = 'ab'", true, false),
    ] {
        assert_eq!(eval_with(text, false), Ok(Val::Logical(loose)), "{text}");
        assert_eq!(
            eval_with(text, true),
            Ok(Val::Logical(exact)),
            "{text} exactly"
        );
    }
}

#[test]
fn null_propagates() {
    assert_eq!(eval("ok"), Val::Null);
    assert_eq!(eval(".NOT. ok"), Val::Null);
    assert_eq!(eval("ok .OR. .T."), Val::Logical(true));
    assert_eq!(eval("ok .AND. .F."), Val::Logical(false));
    assert_eq!(eval("ISNULL(ok)"), Val::Logical(true));
    assert_eq!(eval("IIF(ok, 1, 2)"), Val::Null);
    assert_eq!(eval("COALESCE(ok, .T.)"), Val::Logical(true));
    let expr = Expr::compile("ok = .T.", &Table).unwrap();
    assert!(!expr.holds(&mut record(), &env(false)).unwrap());
}

#[test]
fn strings_and_dates() {
    assert_eq!(eval("name - 'x'"), text("abcx   "));
    assert_eq!(eval("UPPER(name) + '|'"), text("ABC   |"));
    assert_eq!(eval("SUBSTR('abcdef', -1, 2)"), text("f"));
    assert_eq!(eval("STR(n, 5)"), text("   -1"));
    assert_eq!(eval("DTOS(born + 1.5)"), text("20171109"));
    assert_eq!(eval("DTOC(born)"), text("11/08/2017"));
    assert_eq!(eval("born - CTOD('11/01/2017')"), Val::Number(7.0));
    assert_eq!(eval("RECNO()"), Val::Number(7.0));
    assert_eq!(eval("people->n"), Val::Number(-1.0));
}

#[test]
fn types_and_widths() {
    let ty = |text: &str| Expr::compile(text, &Table).map(|e| e.ty());
    assert_eq!(
        ty("name + 'x'"),
        Ok(Type::String {
            width: 7,
            ci: false
        })
    );
    assert_eq!(ty("born"), Ok(Type::Date));
    assert_eq!(ty("name;n"), Ok(Type::Binary { width: 14 }));
    assert_eq!(
        ty("STR(n)"),
        Ok(Type::String {
            width: 11,
            ci: false
        })
    );
    assert_eq!(ty("other->n"), Err(ErrorKind::UnknownAlias));
    // The table's alias, FIELD and _FIELD qualify its fields, in any case,
    // with the arrow right after the qualifier (`fixtures/expr/alias.json`).
    for qualified in [
        "people->n",
        "PEOPLE->n",
        "FIELD->n",
        "field-> n",
        "_Field->n",
    ] {
        assert_eq!(ty(qualified), ty("n"), "{qualified}");
    }
    for unknown in ["FIELD ->n", "people -> n", "FIELD->FIELD->n", "FIELD->nope"] {
        assert_eq!(ty(unknown), Err(ErrorKind::UnknownIdentifier), "{unknown}");
    }
    for other in ["FIELDS->n", "M->n", "MEMVAR->n"] {
        assert_eq!(ty(other), Err(ErrorKind::UnknownAlias), "{other}");
    }
    assert_eq!(ty("nope"), Err(ErrorKind::UnknownIdentifier));
    assert_eq!(ty("FOO(1)"), Err(ErrorKind::UnsupportedFunction));
    assert_eq!(ty("UPPER()"), Err(ErrorKind::ParameterCount));
    assert_eq!(ty("'a' + 1"), Err(ErrorKind::TypeMismatch));
    assert_eq!(ty(".NOT. n > 0"), Err(ErrorKind::TypeMismatch));
}

#[test]
fn names_with_spaces_name_fields_that_have_them() {
    assert_eq!(eval("(Query Number=2)"), Val::Logical(true));
    assert_eq!(
        eval("query number + Seconds Until Finished"),
        Val::Number(32.0)
    );
    for (text, error) in [
        ("Query  Number = 2", ErrorKind::UnknownIdentifier),
        ("Query Numberx = 2", ErrorKind::UnknownIdentifier),
        ("n Number = 2", ErrorKind::UnsupportedOperator),
        ("Seconds Until = 2", ErrorKind::UnknownIdentifier),
    ] {
        assert_eq!(Expr::compile(text, &Table), Err(error), "{text}");
    }
}

#[test]
fn the_first_error_from_the_left_is_reported() {
    for (text, error) in [
        ("foo bar = 1", ErrorKind::UnknownIdentifier),
        ("n bar = 1", ErrorKind::UnsupportedOperator),
        ("foo 1", ErrorKind::UnknownIdentifier),
        ("foo(1) bar", ErrorKind::UnsupportedFunction),
        ("foo(", ErrorKind::UnsupportedFunction),
        ("(foo", ErrorKind::UnknownIdentifier),
        ("1 2 (", ErrorKind::UnsupportedOperator),
        ("n + (1", ErrorKind::Parentheses),
        ("UPPER(", ErrorKind::Parentheses),
        ("foo + 'abc", ErrorKind::UnknownIdentifier),
        ("1 2 + 'abc", ErrorKind::UnsupportedOperator),
        ("n + 'abc", ErrorKind::MissingDelimiter),
        ("UPPER(name, foo)", ErrorKind::UnknownIdentifier),
        ("n , 1", ErrorKind::Incomplete),
        ("x->n = 1", ErrorKind::UnknownAlias),
    ] {
        assert_eq!(Expr::compile(text, &Table), Err(error), "{text}");
    }
}

/// `CTOTS` reads its argument as the original engine does
/// (`fixtures/fields/tstexts.tsv`, probed with `tools/ace/fields/tstexts.c`:
/// the format, the epoch, the text, then `DTOS` of the result and its time
/// in milliseconds): as a TimeStamp value's text, but with the time not
/// checked, and kept when there is no date or it is wrong.
#[test]
fn ctots_matches_the_original_engine() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/fields/tstexts.tsv"
    );
    let mut failures = Vec::new();
    for line in std::fs::read_to_string(path).unwrap().lines() {
        let columns: Vec<&str> = line.split('\t').collect();
        let (format, epoch, text) = (columns[0], columns[1].parse().unwrap(), columns[2]);
        let env = Env {
            date_format: format.into(),
            epoch,
            ..env(false)
        };
        let expr = Expr::compile(&format!("CTOTS('{text}')"), &Table).unwrap();
        let got = match expr.eval(&mut record(), &env).unwrap() {
            Val::TimeStamp(0, ms) => format!("[        ] {ms}"),
            Val::TimeStamp(day, ms) => {
                let ymd = tflip_storage::value::julian_to_ymd(day);
                format!("[{:04}{:02}{:02}] {ms}", ymd.year, ymd.month, ymd.day)
            }
            other => format!("{other:?}"),
        };
        let want = format!("{} {}", columns[3], columns[4]);
        if got != want {
            failures.push(format!(
                "{format} {epoch} {text:?}: {got}, the original {want}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
