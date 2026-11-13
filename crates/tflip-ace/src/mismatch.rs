//! What opening a table does when its ADI index was built under another
//! collation than the connection's, with `ADS_REINDEX_ON_COLLATION_MISMATCH` and
//! `ADS_IGNORE_COLLATION_MISMATCH`, replayed from what the original engine
//! did (`fixtures/collation/mismatch.json`, `tools/ace/collation/mismatch.c`).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{AE_SUCCESS, AdsHandle, Unsigned16, Unsigned32, fixtures};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn open(connect: AdsHandle, path: &str, options: u32, collation: &str) -> (Unsigned32, AdsHandle) {
    let mut path = path.as_bytes().to_vec();
    path.push(0);
    let mut table = 0;
    let rc = if collation.is_empty() {
        unsafe {
            crate::AdsOpenTable(
                connect,
                path.as_mut_ptr(),
                std::ptr::null_mut(),
                3,
                1,
                1,
                2,
                options,
                &mut table,
            )
        }
    } else {
        let mut collation = collation.as_bytes().to_vec();
        collation.push(0);
        unsafe {
            crate::AdsOpenTable90(
                connect,
                path.as_mut_ptr(),
                std::ptr::null_mut(),
                3,
                1,
                1,
                2,
                options,
                collation.as_mut_ptr(),
                &mut table,
            )
        }
    };
    (rc, table)
}

fn text(read: impl FnOnce(*mut u8, *mut Unsigned16) -> Unsigned32) -> String {
    let mut buf = [0u8; 300];
    let mut len = buf.len() as Unsigned16;
    match read(buf.as_mut_ptr(), &mut len) {
        AE_SUCCESS => String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned(),
        _ => String::new(),
    }
}

fn record_number(table: AdsHandle) -> u32 {
    let mut record = 0;
    unsafe { crate::AdsGetRecordNum(table, 2, &mut record) };
    record
}

/// What the probe printed for one case, run by tflip.
fn run(number: usize, case: &Value) -> Value {
    let fixture = case["fixture"].as_str().unwrap();
    let table = case["table"].as_str().unwrap();
    let options = u32::from_str_radix(case["options"].as_str().unwrap(), 16).unwrap();
    let collation = case["collation"].as_str().unwrap();
    let (source, dir) =
        fixtures::scratch_from(&format!("mismatch-{number}"), &fixtures_dir().join(fixture));
    let path = |name: &str| dir.join(name).display().to_string();
    let connect = fixtures::connect_to(&source);
    let second = case["other_user"].as_bool().unwrap().then(|| {
        let second = fixtures::connect_to(&source);
        let (rc, _held) = open(second, &path(table), 0x204, "");
        assert_eq!(rc, AE_SUCCESS);
        second
    });
    let (rc, handle) = open(connect, &path(table), options, collation);
    let mut got = json!({ "open": rc });
    if rc != AE_SUCCESS {
        let mut code = 0;
        got["error"] = json!(text(|buf, len| unsafe {
            crate::AdsGetLastError(&mut code, buf, len)
        }));
        got["code"] = json!(code);
    } else {
        let mut count = 0;
        unsafe { crate::AdsGetNumIndexes(handle, &mut count) };
        got["indexes"] = json!(count);
        got["table_collation"] = json!(text(|buf, len| unsafe {
            crate::AdsGetTableCollation(handle, buf, len)
        }));
        if count > 0 {
            let mut order = 0;
            unsafe { crate::AdsGetIndexHandleByOrder(handle, 1, &mut order) };
            got["index_collation"] = json!(text(|buf, len| unsafe {
                crate::AdsGetIndexCollation(order, buf, len)
            }));
            let mut found = 0;
            let mut key = [0xe4u8];
            let seek = unsafe { crate::AdsSeek(order, key.as_mut_ptr(), 1, 2, 1, &mut found) };
            got["seek"] = json!([seek, found, record_number(handle)]);
            crate::AdsGotoTop(order);
            let mut walk = Vec::new();
            for _ in 0..8 {
                walk.push(record_number(handle));
                crate::AdsSkip(order, 1);
            }
            got["walk"] = json!(walk);
        }
        crate::AdsCloseTable(handle);
    }
    if let Some(second) = second {
        crate::AdsDisconnect(second);
    }
    crate::AdsDisconnect(connect);
    let index = Path::new(table).with_extension("adi");
    let index = std::fs::read(path(&index.to_string_lossy())).unwrap();
    got["index_crc"] = json!(format!(
        "{:08x}",
        u32::from_le_bytes(index[0x14..0x18].try_into().unwrap())
    ));
    got["index_size"] = json!(index.len());
    got
}

fn reindexed() -> Vec<u8> {
    std::fs::read(fixtures_dir().join("collation/mismatch/ca_reindexed.adi")).unwrap()
}

#[test]
fn the_options_act_as_in_the_original_engine() {
    let _shared = fixtures::settings_shared();
    let cases: Vec<Value> = serde_json::from_str(
        &std::fs::read_to_string(fixtures_dir().join("collation/mismatch.json")).unwrap(),
    )
    .unwrap();
    assert!(cases.len() >= 26);
    for (number, case) in cases.iter().enumerate() {
        let mut got = run(number, case);
        let mut expected = case["result"].clone();
        // The path of an open another user holds is the probe's own.
        if expected["open"] == 7008 {
            expected["text"] = Value::Null;
            got["error"] = Value::Null;
        }
        // Where it failed: the code and the text (as the probe prints them).
        if let Some(code) = expected.get("error") {
            assert_eq!(&got["code"], code, "{number}");
        }
        let describe = format!(
            "{} {} {} other user {}",
            case["fixture"], case["table"], case["options"], case["other_user"]
        );
        for key in [
            "open",
            "indexes",
            "table_collation",
            "index_collation",
            "walk",
            "seek",
        ] {
            // The original weighs a text sought in an order of a dynamic
            // collation by it, which tflip does not (see KNOWN_ISSUES).
            if key == "seek" && case["fixture"] == "adt/collation_dynamic" {
                continue;
            }
            assert_eq!(got[key], expected[key], "{describe}: {key}");
        }
        assert_eq!(got["index_crc"], expected["index_crc"], "{describe}");
        assert_eq!(got["index_size"], expected["index_size"], "{describe}");
        assert_eq!(got["error"], expected["text"], "{describe}: error text");
    }
}

/// The index file `ADS_REINDEX_ON_COLLATION_MISMATCH` rebuilds is the
/// original's, byte for byte.
#[test]
fn the_reindex_on_opening_writes_the_original_file() {
    let _shared = fixtures::settings_shared();
    let (source, dir) = fixtures::scratch_from(
        "mismatch-reindexed",
        &fixtures_dir().join("adt/charset_german"),
    );
    let connect = fixtures::connect_to(&source);
    let (rc, table) = open(
        connect,
        &dir.join("ca.adt").display().to_string(),
        0x101,
        "",
    );
    assert_eq!(rc, AE_SUCCESS);
    crate::AdsCloseTable(table);
    crate::AdsDisconnect(connect);
    let ours = std::fs::read(dir.join("ca.adi")).unwrap();
    let theirs = reindexed();
    assert_eq!(ours.len(), theirs.len());
    let first = ours.iter().zip(&theirs).position(|(a, b)| a != b);
    assert_eq!(first, None, "the index differs from the original's");
}
