//! Tables of a dictionary of RC4 tables with a table password, replayed from
//! what the original made (`fixtures/aes/rc4tp`, made by
//! `tools/ace/crypto/rc4tp/run.sh` on the AES-capable engine): each script
//! of `tools/ace/crypto/rc4tp` runs on a new dictionary, and the failed steps
//! and every file it leaves are compared with the original's. RC4 has no
//! random material, so the files compare as they are, but for the secrets of
//! the dictionary's Database object (which hold the table password wrapped
//! under a key of its own).

use std::path::{Path, PathBuf};

use super::aes_replay::first_difference;
use crate::dictionary::{ObjectRef, Property};
use crate::table::{TableSettings, TableType, options};
use crate::{Session, connection, statement, table};

pub(super) fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/aes/rc4tp")
}

pub(super) fn tools() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/ace/crypto/rc4tp")
}

/// The table type of the number the probe's `createt` and `stmttype` give.
fn table_type(number: u16) -> TableType {
    match number {
        1 => TableType::Ntx,
        2 => TableType::Cdx,
        4 => TableType::Vfp,
        _ => TableType::Adt,
    }
}

/// What a script's steps came to: the failed ones by their number with the
/// code, and the values the `dbget` and `tget` steps read.
#[derive(Default)]
pub(super) struct Outcome {
    pub(super) failed: Vec<(usize, u32)>,
    pub(super) values: Vec<(usize, u16)>,
    /// The rows the `query` steps read, as the probe writes them.
    pub(super) rows: Vec<(usize, String)>,
}

/// A cursor's rows as the probe's `query` step writes them (`AdsGotoTop`,
/// then `AdsSkip` through the cursor's filters, every field as
/// `AdsGetString` gives it): each field as `name=text;`, each row closed by
/// `|`. Binary values are in base64, as the original gives them.
fn rows_text(session: &Session, cursor: crate::handle::Handle) -> String {
    use tflip_storage::value::Value;
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let base64 = |bytes: &[u8]| -> String {
        bytes
            .chunks(3)
            .flat_map(|c| {
                let n = c.iter().fold(0u32, |n, &b| n << 8 | u32::from(b)) << (8 * (3 - c.len()));
                (0..4).map(move |k| match k <= c.len() {
                    true => ALPHABET[(n >> (18 - 6 * k) & 63) as usize] as char,
                    false => '=',
                })
            })
            .collect()
    };
    let table = table::get(session, cursor).unwrap();
    let names: Vec<String> = table
        .fields()
        .iter()
        .map(|f| f.name.to_lowercase())
        .collect();
    let mut out = String::new();
    table.goto_top().unwrap();
    while !table.position().eof {
        for (number, name) in names.iter().enumerate() {
            let character = matches!(
                table.fields()[number].field_type,
                tflip_storage::field::FieldType::Character
            );
            let value = table.value(table::FieldRef::Number(number + 1)).unwrap();
            let text = match value {
                Value::Text(text) if character => text.trim_end().to_owned(),
                Value::Text(text) => text,
                Value::Binary(bytes) => base64(&bytes),
                Value::Integer(n) => n.to_string(),
                other => panic!("{other:?}"),
            };
            out.push_str(&format!("{name}={text};"));
        }
        out.push('|');
        table.skip(1).unwrap();
    }
    out
}

/// Runs the script `text` on the dictionaries in `dir`: the steps of
/// `aes_replay::run` (`connect`, `ddcreate`, `sql`, `disconnect`) and those of
/// `tools/ace/crypto/rc4tp/api.c`.
pub(super) fn run(session: &Session, dir: &Path, text: &str) -> Outcome {
    let folder = dir.to_string_lossy();
    let (mut handle, mut outcome) = (None, Outcome::default());
    let mut statement_type = TableType::Adt;
    let lines = text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    for (step, line) in lines.enumerate() {
        let line = line.replace("%D", &folder).replace('\\', "/");
        let step = step + 1;
        let mut done = |error: crate::Error| {
            if std::env::var("AES_DEBUG").is_ok() {
                eprintln!("step {step}: {line}: {error:?}");
            }
            outcome.failed.push((step, error.to_ace().code))
        };
        let word = |n: usize| line.split(' ').nth(n).unwrap_or("");
        let number = |n: usize| word(n).parse::<u16>().unwrap_or(0);
        let object = |what: ObjectRef, id: u16, handle: Option<crate::handle::Handle>| {
            connection::get(session, handle.unwrap())
                .and_then(|c| c.with_database(|db| db.property(&what, id)))
        };
        if let Some(cs) = line.strip_prefix("connect ") {
            handle = connection::connect(session, cs).map_err(&mut done).ok();
        } else if let Some(rest) = line.strip_prefix("ddcreate ") {
            let (flag, path) = rest.split_once(' ').unwrap();
            handle = super::create_dictionary_api(session, path, flag.parse().unwrap())
                .map_err(&mut done)
                .ok();
        } else if let Some(sql) = line.strip_prefix("sql ") {
            let statement_handle = statement::create(session, handle.unwrap()).unwrap();
            statement::set(
                session,
                statement_handle,
                statement::Setting::TableType(statement_type),
            )
            .unwrap();
            statement::execute_direct(session, statement_handle, sql, true).map_or_else(done, drop);
            statement::close(session, statement_handle).ok();
        } else if let Some(sql) = line.strip_prefix("query ") {
            let statement_handle = statement::create(session, handle.unwrap()).unwrap();
            statement::set(
                session,
                statement_handle,
                statement::Setting::TableType(statement_type),
            )
            .unwrap();
            match statement::execute_direct(session, statement_handle, sql, true) {
                Ok(Some(cursor)) => {
                    outcome.rows.push((step, rows_text(session, cursor)));
                    table::close(session, cursor).ok();
                }
                Ok(None) => {}
                Err(error) => done(error),
            }
            statement::close(session, statement_handle).ok();
        } else if line == "disconnect"
            && let Some(handle) = handle.take()
        {
            connection::disconnect(session, handle).unwrap();
        } else if line.starts_with("dbset ") {
            super::set_database_property_api(
                session,
                handle.unwrap(),
                number(1),
                &number(2).to_le_bytes(),
            )
            .map_or_else(done, drop);
        } else if line.starts_with("tset ") {
            super::set_table_property_api(
                session,
                handle.unwrap(),
                word(1),
                number(2),
                &number(3).to_le_bytes(),
            )
            .map_or_else(done, drop);
        } else if line.starts_with("dbget ") || line.starts_with("tget ") {
            let (what, id) = match line.starts_with("dbget ") {
                true => (ObjectRef::Database, number(1)),
                false => (ObjectRef::Table(word(1).to_owned()), number(2)),
            };
            match object(what, id, handle) {
                Ok(Property::U16(value)) => outcome.values.push((step, value)),
                Ok(other) => panic!("step {step}: {other:?}"),
                Err(error) => done(error),
            }
        } else if line.starts_with("stmttype ") {
            statement_type = table_type(number(1));
        } else if line.starts_with("create ")
            || line.starts_with("createt ")
            || line.starts_with("createf ")
        {
            let (kind, name) = match line.starts_with("create ") {
                true => (TableType::Adt, word(1)),
                false => (table_type(number(1)), word(2)),
            };
            let fields = match line.starts_with("createf ") {
                true => line.splitn(4, ' ').nth(3).unwrap(),
                false => "id, Integer; note, Memo;",
            };
            let settings = TableSettings {
                table_type: kind,
                ..Default::default()
            };
            match crate::ddl::create_table_in_dictionary_api(
                session,
                handle.unwrap(),
                name,
                fields,
                0,
                settings,
            ) {
                Ok(table) => drop(table::close(session, table)),
                Err(error) => done(error),
            }
        } else if line.starts_with("addtable ") || line.starts_with("addtablet ") {
            let skip = usize::from(line.starts_with("addtablet "));
            let kind = match skip {
                1 => table_type(number(1)),
                _ => TableType::Adt,
            };
            super::add_table_api(
                session,
                handle.unwrap(),
                word(1 + skip),
                word(2 + skip),
                kind,
                1,
                word(3 + skip),
                "",
            )
            .map_or_else(done, drop);
        } else if matches!(line.as_str(), "begin" | "commit" | "rollback") {
            let handle = handle.unwrap();
            match line.as_str() {
                "begin" => crate::transaction::begin(session, handle),
                "commit" => crate::transaction::commit(session, handle),
                _ => crate::transaction::rollback(session, handle, None),
            }
            .map_or_else(done, drop);
        } else if line.starts_with("restr ") {
            let (_, fields) = line["restr ".len()..].split_once(' ').unwrap();
            let settings = TableSettings {
                table_type: statement_type,
                char_type: 1,
                ..Default::default()
            };
            let changes =
                crate::restructure::Changes::parse(fields, "", "", 0, settings.table_type);
            changes
                .and_then(|changes| {
                    crate::restructure::restructure_table(
                        session,
                        handle.unwrap(),
                        word(1),
                        settings,
                        &changes,
                    )
                })
                .map_or_else(done, drop);
        } else if line.starts_with("copy ") || line.starts_with("copyc ") {
            let contents = line.starts_with("copyc ");
            let open = |name: &str| {
                let settings = TableSettings {
                    table_type: if name.contains('/') {
                        statement_type
                    } else {
                        TableType::Adt
                    },
                    options: options::EXCLUSIVE,
                    ..Default::default()
                };
                table::open(session, handle.unwrap(), name, settings)
            };
            match open(word(1)) {
                Ok(from) => {
                    let result = match contents {
                        true => open(word(2)).and_then(|to| {
                            let done = crate::copy::copy_contents(session, from, to, 2);
                            let _ = table::close(session, to);
                            done
                        }),
                        false => {
                            // The extension the table type gives a new file.
                            let name = match statement_type {
                                TableType::Adt => format!("{}.adt", word(2)),
                                _ => format!("{}.dbf", word(2)),
                            };
                            crate::copy::copy_table(session, from, 2, &name, false)
                        }
                    };
                    let _ = table::close(session, from);
                    result.map_or_else(done, drop);
                }
                Err(error) => done(error),
            }
        } else if line.starts_with("zap ")
            || line.starts_with("pack ")
            || line.starts_with("encpw ")
        {
            let name = word(1);
            let settings = TableSettings {
                // A free table is opened by its path, as the table type of
                // the statements says.
                table_type: if name.contains('/') {
                    statement_type
                } else {
                    TableType::Adt
                },
                options: options::EXCLUSIVE,
                ..Default::default()
            };
            match table::open(session, handle.unwrap(), name, settings) {
                Ok(opened) => {
                    let result = table::get(session, opened).and_then(|t| match word(0) {
                        "zap" => t.pack_file(true, None),
                        "pack" => t.pack_file(false, None),
                        _ => t
                            .enable_encryption(word(2).as_bytes())
                            .and_then(|()| match word(3) {
                                "x" => Ok(()),
                                _ => t.encrypt_table(),
                            }),
                    });
                    let _ = table::close(session, opened);
                    result.map_or_else(done, drop);
                }
                Err(error) => done(error),
            }
        } else if line.starts_with("enc ") || line.starts_with("dec ") {
            let settings = TableSettings {
                options: options::EXCLUSIVE,
                ..Default::default()
            };
            match table::open(session, handle.unwrap(), word(1), settings) {
                Ok(opened) => {
                    let done_table =
                        table::get(session, opened).and_then(|t| match line.starts_with("enc ") {
                            true => t.encrypt_table(),
                            false => t.decrypt_table(),
                        });
                    let _ = table::close(session, opened);
                    done_table.map_or_else(done, drop);
                }
                Err(error) => done(error),
            }
        }
    }
    outcome
}

/// The failed steps and the values the original recorded for `script`.
pub(super) fn recorded(script: &str) -> Outcome {
    let json = std::fs::read_to_string(fixtures().join(format!("{script}.json"))).unwrap();
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    let mut outcome = Outcome::default();
    for (key, value) in json.as_object().unwrap() {
        let step = |suffix: &str| key.strip_suffix(suffix)?.parse::<usize>().ok();
        if let (Some(step), Some(code)) = (step("_rc"), value.as_u64())
            && code != 0
        {
            outcome.failed.push((step, code as u32));
        }
        if let (Some(step), Some(value)) = (step("_val"), value.as_u64()) {
            outcome.values.push((step, value as u16));
        }
        if let (Some(step), Some(text)) = (step("_rows"), value.as_str()) {
            outcome.rows.push((step, text.to_owned()));
        }
    }
    outcome.failed.sort();
    outcome.values.sort();
    outcome.rows.sort();
    outcome
}

/// What `bytes` of the file `name` is, for comparing: a dictionary's secrets
/// blanked.
fn normalized(name: &str, mut bytes: Vec<u8>) -> Vec<u8> {
    if name.ends_with(".dbt") {
        // The byte the original leaves after a Binary or Image value, before
        // the 0x1A, is whatever the file had there.
        for block in (512..bytes.len()).step_by(512) {
            let tag = &bytes[block..bytes.len().min(block + 16)];
            if tag.len() == 16 && (tag.starts_with(b"@R_BLOB@\0") || tag.starts_with(b"@SIXPIC@\0"))
            {
                let length = u32::from_le_bytes(tag[12..16].try_into().unwrap()) as usize;
                if let Some(at) = bytes.get_mut(block + 16 + length) {
                    *at = 0;
                }
            }
        }
    }
    match name.ends_with(".add") {
        true => super::test_support::mask_db_secret(bytes),
        false => bytes,
    }
}

/// The files under `root`, by their path from it (the free tables of a
/// script are in a folder of their own).
pub(super) fn files_under(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() {
            files.extend(
                files_under(&entry.path())
                    .into_iter()
                    .map(|inner| format!("{name}/{inner}")),
            );
        } else {
            files.push(name);
        }
    }
    files.sort();
    files
}

/// Replays `script`, returning what differs from the original's.
fn replay(script: &str) -> Vec<String> {
    let dir = tflip_storage::testing::temp_root()
        .join(format!("tflip-rc4tp-{script}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let text = std::fs::read_to_string(tools().join(format!("{script}.txt")))
        .unwrap()
        .replace("%N", script);
    // The free tables of a script are made in a folder of their own, which the
    // fixtures leave out when they hold no table.
    std::fs::create_dir_all(dir.join(script)).unwrap();
    if text.contains("\\free\\") {
        std::fs::create_dir_all(dir.join(script).join("free")).unwrap();
    }
    let session = Session::new();
    let ours = run(&session, &dir, &text);
    let mut ours = ours;
    ours.rows.sort();
    let theirs = recorded(script);
    let mut differences = Vec::new();
    if ours.rows != theirs.rows {
        differences.push(format!(
            "{script}: rows {:?}, the original {:?}",
            ours.rows, theirs.rows
        ));
    }
    if ours.failed != theirs.failed || ours.values != theirs.values {
        differences.push(format!(
            "{script}: failed {:?} read {:?}, the original {:?} {:?}",
            ours.failed, ours.values, theirs.failed, theirs.values
        ));
    }
    let originals = fixtures().join(script);
    // A script with AES dictionaries (random IVs in their files) is compared
    // by what its steps returned only.
    if !originals.exists() {
        if std::env::var("KEEP").is_err() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        return differences;
    }
    for name in files_under(&originals) {
        let mut expected = normalized(&name, std::fs::read(originals.join(&name)).unwrap());
        let Ok(ours) = std::fs::read(dir.join(script).join(&name)) else {
            differences.push(format!("{script}/{name}: not made"));
            continue;
        };
        // A DBF header carries the date it was last written on.
        crate::clock::adopt_todays_dbf_date(&name, &ours, &mut expected);
        if let Some(difference) = first_difference(&normalized(&name, ours), &expected) {
            differences.push(format!("{script}/{name}: {difference}"));
        }
    }
    for name in files_under(&dir.join(script)) {
        if !originals.join(&name).exists() {
            differences.push(format!("{script}/{name}: made, not the original's"));
        }
    }
    if std::env::var("KEEP").is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    differences
}

/// The scripts replayed.
const SCRIPTS: [&str; 38] = [
    "y7", "y8", "y9", "y10", "y11", "y12", "y13", "y14", "y1", "y2", "y3", "y6", "x1", "x2", "x3",
    "x5", "x6", "e12", "e13", "a1", "a3", "a4", "e1", "e2", "e3", "e4", "e5", "e6", "e7", "e8",
    "e9", "e10", "e11", "f1", "f2", "z1", "z2", "y5",
];

#[test]
fn makes_and_changes_tables_of_a_dictionary_with_a_table_password_as_the_original() {
    let only = std::env::var("ONLY").ok();
    let differences: Vec<String> = SCRIPTS
        .iter()
        .filter(|script| only.as_deref().is_none_or(|only| only == **script))
        .flat_map(|script| replay(script))
        .collect();
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
