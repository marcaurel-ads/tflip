//! `fixtures/sql/rowid_paths.json`: the `ROWID`s the original engine gave
//! for tables of databases at known paths (`tools/ace/sql/rowid*.txt`).

use std::path::Path;

use serde_json::Value as Json;
use tflip_sql::rowid::{Table, prefix};

#[test]
fn rowids_name_the_database_and_the_table_as_the_original_engine_does() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sql/rowid_paths.json");
    let cases: Vec<Json> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(cases.len() > 15);
    for case in cases {
        let database = case["database"].as_str().unwrap();
        let rowid = case["rowid"].as_str().unwrap();
        let table = match (case["file"].as_str(), case["object"].as_u64()) {
            (Some(name), _) => {
                // A free table is in the database's own folder.
                Table::File {
                    folder: database,
                    name,
                }
            }
            (None, Some(id)) => Table::Object(id as u32),
            _ => panic!("{case}"),
        };
        let got = tflip_sql::rowid::encode(&prefix(database, table), 1);
        assert_eq!(got, rowid, "{case}");
    }
}
