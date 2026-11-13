//! The wire format against copies of it kept with the protocol version
//! (`tests/wire/`): every operation's signature, and the bytes of sample
//! requests and replies carrying the engine's types. A change to either
//! fails here until `PROTOCOL_VERSION` is raised and the copies renewed
//! with `TFLIP_BLESS=1`, so that clients and servers of different builds
//! never misread each other.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use tflip_engine::Error;
use tflip_engine::dictionary::Property;
use tflip_engine::getters::{Charset, Scalar};
use tflip_engine::table::{TableSettings, TableType};
use tflip_protocol::{Api, PROTOCOL_VERSION, Reply, Request, SIGNATURES, Session};
use tflip_storage::value::Value;

fn wire() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wire")
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt")
}

/// Sample messages as `name: hex` lines.
fn samples() -> String {
    let session = Session::new();
    let source = |name: &str| fixtures().join(name).display().to_string();
    let connection = session
        .connect_path(source("indexes"), 1, None, None, None)
        .unwrap();
    let settings = TableSettings {
        table_type: TableType::Adt,
        ..TableSettings::default()
    };
    let table = session
        .open_table(connection, "indexed.adt".into(), settings.clone())
        .unwrap();
    let mut table_info = session.table_info(table).unwrap();
    table_info.path = "indexed.adt".into();
    let order = session.indexes(table).unwrap()[0];
    let mut index_info = session.index_info(order).unwrap();
    index_info.path = "indexed.adi".into();

    let values = [
        Value::Null,
        Value::Text("a".into()),
        Value::Binary(vec![1]),
        Value::Logical(true),
        Value::Numeric("1.5".into()),
        Value::Integer(-1),
        Value::Double(0.5),
        Value::Money(10_000),
        Value::Date(2_458_066),
        Value::Time(1),
        Value::TimeStamp(2_458_066, 1),
        Value::Guid([7; 16]),
        Value::Blank,
    ];
    let errors = [
        Error::InvalidConnection,
        Error::FileNotFound,
        Error::NoCurrentRecord,
        Error::NotImplemented,
        Error::ConnectionLost,
        Error::NoCursorHandle,
        Error::InvalidPropertyId,
        Error::TempTableRefused,
    ];
    let mut messages: Vec<(String, Vec<u8>)> = Vec::new();
    let mut add = |name: String, bytes: Vec<u8>| messages.push((name, bytes));
    let request = |request: Request| postcard::to_stdvec(&request).unwrap();
    let reply = |reply: Reply| postcard::to_stdvec(&reply).unwrap();
    add(
        "open_table request".into(),
        request(Request::open_table {
            connection: 1,
            name: "t".into(),
            settings,
        }),
    );
    add(
        "table_info reply".into(),
        reply(Reply::table_info(Ok(table_info))),
    );
    add(
        "index_info reply".into(),
        reply(Reply::index_info(Ok(index_info))),
    );
    for value in values {
        add(
            format!("value reply {value:?}"),
            reply(Reply::value(Ok(value))),
        );
    }
    for error in errors {
        add(
            format!("error reply {error:?}"),
            reply(Reply::goto_top(Err(error))),
        );
    }
    let scalars = [
        Scalar::Long(1),
        Scalar::Money(1),
        Scalar::Text("x".into()),
        Scalar::FieldText {
            text: tflip_engine::format::FieldText::Text("x".into()),
            charset: Charset::Oem,
        },
    ];
    for scalar in scalars {
        add(
            format!("field_as reply {scalar:?}"),
            reply(Reply::field_as(Ok(scalar))),
        );
    }
    for property in [Property::Text("x".into()), Property::Unset(2)] {
        add(
            format!("dd_property reply {property:?}"),
            reply(Reply::dd_property(Ok(property))),
        );
    }
    let mut text = String::new();
    for (name, bytes) in messages {
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        writeln!(text, "{name}: {hex}").unwrap();
    }
    text
}

/// Compares `text` with the copy `file` keeps, or renews it with
/// `TFLIP_BLESS=1`.
fn check(file: &str, text: &str) {
    let path = wire().join(file);
    let text = format!("protocol {PROTOCOL_VERSION}\n{text}");
    if std::env::var_os("TFLIP_BLESS").is_some() {
        std::fs::create_dir_all(wire()).unwrap();
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let kept = std::fs::read_to_string(&path).unwrap_or_default();
    let kept_version = kept.lines().next().unwrap_or_default();
    assert!(
        kept == text,
        "{file} differs from the wire format of {kept_version}: raise PROTOCOL_VERSION if \
         it changed, then renew the copy with TFLIP_BLESS=1"
    );
}

/// Every operation's signature, one per line, its spacing normalized (the
/// compiler's may change).
fn signatures() -> String {
    let words: Vec<&str> = SIGNATURES.split_whitespace().collect();
    words.join(" ").replace(" fn ", "\nfn ") + "\n"
}

#[test]
fn operations_are_as_kept() {
    check("signatures.txt", &signatures());
}

#[test]
fn messages_are_as_kept() {
    check("samples.txt", &samples());
}
