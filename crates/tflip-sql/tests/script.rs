//! Parses the stored-procedure and function script bodies from the `psm`
//! dictionary fixture (as `tools/ace` builds it) into the script AST.

use tflip_sql::ast::{ScriptStmt, Statement};
use tflip_sql::parse::parse_script;

fn parse(src: &str) -> Vec<ScriptStmt> {
    parse_script(src).unwrap_or_else(|e| panic!("parse {src:?}: {e:?}"))
}

#[test]
fn parses_procedure_bodies() {
    // add_item: two INSERTs.
    let s = parse(
        " INSERT INTO items (id, name, qty) VALUES (_id, _name, _qty); \
          INSERT INTO __output SELECT COUNT(*) FROM items; ",
    );
    assert_eq!(s.len(), 2);
    assert!(matches!(s[0], ScriptStmt::Sql(ref b, ..) if matches!(**b, Statement::Insert(_))));
    assert!(matches!(s[1], ScriptStmt::Sql(ref b, ..) if matches!(**b, Statement::Insert(_))));

    // numbers: DECLARE, assign, WHILE with a two-statement body.
    let s = parse(
        " DECLARE i INTEGER; i = 0; \
          WHILE i < _n DO i = i + 1; INSERT INTO __output VALUES (i); END WHILE; ",
    );
    assert_eq!(s.len(), 3);
    assert!(matches!(s[0], ScriptStmt::Declare { .. }));
    assert!(matches!(s[1], ScriptStmt::Assign { .. }));
    let ScriptStmt::While { body, .. } = &s[2] else {
        panic!("while")
    };
    assert_eq!(body.len(), 2);
    assert!(matches!(body[0], ScriptStmt::Assign { .. }));
    assert!(matches!(body[1], ScriptStmt::Sql(..)));

    // pick: IF/THEN/ELSE, each branch a SELECT.
    let s = parse(
        " IF _which = 1 THEN SELECT * FROM items; \
          ELSE SELECT COUNT(*) AS n FROM audit; END IF; ",
    );
    assert_eq!(s.len(), 1);
    let ScriptStmt::If { then, els, .. } = &s[0] else {
        panic!("if")
    };
    assert_eq!(then.len(), 1);
    assert_eq!(els.len(), 1);
}

#[test]
fn parses_function_bodies() {
    // sq: a single RETURN of an expression.
    let s = parse(" RETURN i * i; ");
    assert!(matches!(s.as_slice(), [ScriptStmt::Return(Some(_))]));

    // pad: DECLARE, assign, IF (no ELSE) with an assignment, then RETURN.
    let s = parse(
        " DECLARE s STRING; s = TRIM(CAST(i AS SQL_CHAR)); \
          IF LENGTH(s) < len THEN s = REPEAT('0', len - LENGTH(s)) + s; END IF; RETURN s; ",
    );
    assert_eq!(s.len(), 4);
    assert!(matches!(s[0], ScriptStmt::Declare { .. }));
    assert!(matches!(s[1], ScriptStmt::Assign { .. }));
    let ScriptStmt::If { then, els, .. } = &s[2] else {
        panic!("if")
    };
    assert_eq!(then.len(), 1);
    assert!(matches!(then[0], ScriptStmt::Assign { .. }));
    assert!(els.is_empty());
    assert!(matches!(s[3], ScriptStmt::Return(Some(_))));
}

#[test]
fn parses_blocks_and_rejects_garbage() {
    // A BEGIN…END block (not a transaction).
    let s = parse(" BEGIN DECLARE x INTEGER; x = 1; END; ");
    let [ScriptStmt::Block(body)] = s.as_slice() else {
        panic!("block")
    };
    assert_eq!(body.len(), 2);

    // BEGIN TRANSACTION is a transaction statement, not a block.
    let s = parse(" BEGIN TRANSACTION; COMMIT; ");
    assert!(matches!(s[0], ScriptStmt::Sql(ref b, ..) if matches!(**b, Statement::Transaction(_))));

    // A declare with a sized type.
    let s = parse(" DECLARE c CHAR(10); ");
    assert!(matches!(&s[0], ScriptStmt::Declare { ty, .. } if ty == "CHAR(10)"));

    // Trailing garbage is rejected.
    assert!(parse_script(" SELECT 1; )").is_err());
}

/// A text of several statements, or one that starts with the script
/// language's own, is a script (`fixtures/sql/scripts.json`).
#[test]
fn a_text_of_several_statements_is_a_script() {
    let script = |text: &str| match tflip_sql::parse(text).unwrap().statement {
        Statement::Script(body) => body,
        other => panic!("{text:?} is {other:?}"),
    };
    assert_eq!(script("SELECT 1 FROM t; SELECT 2 FROM t").len(), 2);
    assert_eq!(script("SELECT 1 FROM t; SELECT 2 FROM t;").len(), 2);
    assert_eq!(script("DECLARE i INTEGER").len(), 1);
    assert_eq!(script("DECLARE i INTEGER, j INTEGER;").len(), 1);
    assert_eq!(script("DECLARE i INTEGER; i = 1").len(), 2);
    assert_eq!(script("EXECUTE IMMEDIATE 'SELECT 1 FROM t'").len(), 1);
    // One statement, with or without its `;`, is not.
    for text in [
        "SELECT 1 FROM t",
        "SELECT 1 FROM t;",
        "EXECUTE PROCEDURE p()",
    ] {
        assert!(!matches!(
            tflip_sql::parse(text).unwrap().statement,
            Statement::Script(_)
        ));
    }
    // Its parameters are numbered across the statements.
    let parsed = tflip_sql::parse("INSERT INTO t VALUES (?); INSERT INTO t VALUES (:a)").unwrap();
    assert_eq!(parsed.params, [None, Some("a".to_owned())]);
}

/// Empty statements, declarations in the body and statements that cannot
/// run are refused where the original refuses them, at the same place.
#[test]
fn a_script_is_checked_as_a_whole() {
    let error = |text: &str| tflip_sql::parse(text).unwrap_err();
    for (text, native, position) in [
        ("SELECT 1 FROM t;; SELECT 2 FROM t", 2115, " is: "),
        ("SELECT 1 FROM t; DECLARE i INTEGER", 2217, "is: 26"),
        ("DECLARE i INTEGER; DECLARE I INTEGER", 2217, "is: 28"),
        ("DECLARE i INTEGER; RETURN; i = 1", 2219, "is: 26"),
        ("DECLARE i INTEGER; RAISE e(1, 'x'); i = 1", 2219, "is: 26"),
        ("DECLARE i INTEGER; RETURN i", 2117, "is: 27"),
        ("DECLARE i INTEGER; BEGIN i = 1; END", 2115, "is: 26"),
    ] {
        let error = error(text);
        assert_eq!(error.native, native, "{text}: {}", error.message);
        if native != 2115 || text.starts_with("DECLARE") {
            assert!(
                error.message.ends_with(position),
                "{text}: {}",
                error.message
            );
        }
    }
}

/// Cursors and `TRY` parse; what the language refuses is refused at parse
/// time (`fixtures/sql/cursors.json`, `trycatch.json`).
#[test]
fn cursors_and_try_parse() {
    for text in [
        "DECLARE c CURSOR AS SELECT 1 FROM t; OPEN c; CLOSE c;",
        "DECLARE c CURSOR; OPEN c AS SELECT 1 FROM t; FETCH c;",
        "DECLARE c CURSOR AS SELECT 1 FROM t; DECLARE i INTEGER; WHILE FETCH c DO i = c.n; END WHILE;",
        "TRY SELECT 1 FROM t; CATCH ALL RAISE; END TRY;",
        "TRY SELECT 1 FROM t; CATCH a SELECT 2 FROM t; CATCH ALL SELECT 3 FROM t; FINALLY SELECT 4 FROM t; END TRY;",
    ] {
        tflip_sql::parse(text).unwrap_or_else(|e| panic!("{text}: {}", e.message));
    }
    for (text, native) in [
        ("OPEN c;", 2218),
        ("DECLARE c CURSOR; DECLARE c CURSOR;", 2217),
        ("RAISE;", 2219),
        ("TRY SELECT 1 FROM t; END TRY;", 2219),
        (
            "TRY SELECT 1 FROM t; CATCH ALL SELECT 2 FROM t; CATCH a SELECT 3 FROM t; END TRY;",
            2219,
        ),
    ] {
        let error = tflip_sql::parse(text).unwrap_err();
        assert_eq!(error.native, native, "{text}: {}", error.message);
    }
}
