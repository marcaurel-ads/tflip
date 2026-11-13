//! How many requests common exports send to a server: the budget the
//! layering keeps (see the README's architecture section). Each export is
//! called once to warm the client's caches, then counted.

use crate::fixtures;
use crate::{
    ADS_IGNOREFILTERS, ADS_STRINGKEY, AE_SUCCESS, AdsGetIndexHandle, AdsGetRecordCount, AdsGotoTop,
    AdsHandle, AdsIsNull, AdsSkip,
};

/// A call of an export, returning its code.
type Call = Box<dyn Fn() -> u32>;

/// The requests each export sends, as `(export, requests)`.
fn counted(
    connection: AdsHandle,
    table: AdsHandle,
    order: AdsHandle,
) -> Vec<(&'static str, usize)> {
    let id = c"id".as_ptr().cast_mut().cast();
    let name = c"name".as_ptr().cast_mut().cast();
    let born = c"born".as_ptr().cast_mut().cast();
    // The table's full path, which AdsGetTableHandle takes.
    let path = crate::backend::api(table).table_info(table).unwrap().path;
    let path = std::ffi::CString::new(path).unwrap();
    let path: *mut u8 = path.into_raw().cast();
    let key = c"B".as_ptr().cast_mut().cast();
    let calls: Vec<(&'static str, Call)> = vec![
        (
            "AdsGetLong",
            Box::new(move || unsafe { crate::AdsGetLong(table, id, &mut 0i32) }),
        ),
        (
            "AdsGetString",
            Box::new(move || {
                let mut text = [0u8; 64];
                let mut len = text.len() as u32;
                unsafe { crate::AdsGetString(table, name, text.as_mut_ptr(), &mut len, 0) }
            }),
        ),
        (
            "AdsGetField",
            Box::new(move || {
                let mut text = [0u8; 64];
                let mut len = text.len() as u32;
                unsafe { crate::AdsGetField(table, born, text.as_mut_ptr(), &mut len, 0) }
            }),
        ),
        (
            "AdsGetDate",
            Box::new(move || {
                let mut text = [0u8; 64];
                let mut len = text.len() as u16;
                unsafe { crate::AdsGetDate(table, born, text.as_mut_ptr(), &mut len) }
            }),
        ),
        (
            "AdsIsNull",
            Box::new(move || unsafe { AdsIsNull(table, name, &mut 0u16) }),
        ),
        ("AdsSkip", Box::new(move || AdsSkip(table, 1))),
        ("AdsGotoTop", Box::new(move || AdsGotoTop(table))),
        (
            "AdsGetRecordCount",
            Box::new(move || unsafe { AdsGetRecordCount(table, ADS_IGNOREFILTERS, &mut 0u32) }),
        ),
        (
            "AdsSeek",
            Box::new(move || unsafe { crate::AdsSeek(order, key, 1, ADS_STRINGKEY, 1, &mut 0u16) }),
        ),
        (
            "AdsGetRelKeyPos",
            Box::new(move || unsafe { crate::AdsGetRelKeyPos(order, &mut 0f64) }),
        ),
        (
            "AdsGetTableHandle",
            Box::new(move || unsafe { crate::AdsGetTableHandle(path, &mut 0) }),
        ),
        (
            "AdsGetNumOpenTables",
            Box::new(move || unsafe { crate::AdsGetNumOpenTables(&mut 0u16) }),
        ),
    ];
    calls
        .into_iter()
        .map(|(export, call)| {
            assert_eq!(call(), AE_SUCCESS, "{export}");
            fixtures::forget_requests(connection);
            assert_eq!(call(), AE_SUCCESS, "{export}");
            (export, fixtures::requests(connection).len())
        })
        .collect()
}

/// The requests common exports send to a server once the client's caches
/// are warm.
#[test]
fn requests_per_export() {
    let _counting = fixtures::counting();
    fixtures::remotely(|| {
        let (connection, table) = fixtures::open("indexes", c"indexed.adt");
        let mut order = 0;
        let rc =
            unsafe { AdsGetIndexHandle(table, c"BY_NAME".as_ptr().cast_mut().cast(), &mut order) };
        assert_eq!(rc, AE_SUCCESS);
        let counted = counted(connection, table, order);
        let budget = [
            ("AdsGetLong", 1),
            ("AdsGetString", 1),
            ("AdsGetField", 1),
            ("AdsGetDate", 1),
            ("AdsIsNull", 1),
            ("AdsSkip", 1),
            ("AdsGotoTop", 1),
            ("AdsGetRecordCount", 1),
            ("AdsSeek", 1),
            ("AdsGetRelKeyPos", 1),
            ("AdsGetTableHandle", 1),
            ("AdsGetNumOpenTables", 1),
        ];
        crate::AdsDisconnect(connection);
        let over: Vec<String> = counted
            .iter()
            .zip(budget)
            .filter(|((_, got), (_, allowed))| got > allowed)
            .map(|((export, got), (_, allowed))| format!("{export}: {got} > {allowed}"))
            .collect();
        assert!(
            over.is_empty(),
            "over budget: {over:?}\ncounted: {counted:?}"
        );
    });
}

/// The requests the exports that write send to a server: one each.
#[test]
fn write_requests_per_export() {
    let _counting = fixtures::counting();
    fixtures::remotely(|| {
        let source = fixtures::scratch("write_requests", "freelist");
        let connection = fixtures::connect_to(&source);
        let mut table = 0;
        let rc = unsafe {
            crate::AdsOpenTable(
                connection,
                c"fl_chain.adt".as_ptr().cast_mut().cast(),
                std::ptr::null_mut(),
                crate::ADS_ADT,
                crate::ADS_ANSI,
                crate::ADS_PROPRIETARY_LOCKING,
                1,
                crate::ADS_SHARED,
                &mut table,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(AdsGotoTop(table), AE_SUCCESS);
        let name = c"c_name".as_ptr().cast_mut().cast();
        let id = c"c_id".as_ptr().cast_mut().cast();
        let calls: Vec<(&'static str, Call)> = vec![
            (
                "AdsSetString",
                Box::new(move || {
                    let mut text = *b"abc";
                    unsafe { crate::AdsSetString(table, name, text.as_mut_ptr(), 3) }
                }),
            ),
            (
                "AdsSetLong",
                Box::new(move || unsafe { crate::AdsSetLong(table, id, 7) }),
            ),
            (
                "AdsWriteRecord",
                Box::new(move || crate::AdsWriteRecord(table)),
            ),
            (
                "AdsAppendRecord",
                Box::new(move || crate::AdsAppendRecord(table)),
            ),
            (
                "AdsLockRecord",
                Box::new(move || crate::AdsLockRecord(table, 0)),
            ),
        ];
        let mut over = Vec::new();
        for (export, call) in calls {
            assert_eq!(call(), AE_SUCCESS, "{export}");
            fixtures::forget_requests(connection);
            assert_eq!(call(), AE_SUCCESS, "{export}");
            let sent = fixtures::requests(connection);
            if sent.len() > 1 {
                over.push(format!("{export}: {sent:?}"));
            }
        }
        crate::AdsDisconnect(connection);
        assert!(over.is_empty(), "over budget: {over:?}");
    });
}

/// A field read as text, which succeeds for every field type.
fn field_text(table: AdsHandle, name: &std::ffi::CStr) -> String {
    let mut text = [0u8; 64];
    let mut len = text.len() as u32;
    let rc = unsafe {
        crate::AdsGetField(
            table,
            name.as_ptr().cast_mut().cast(),
            text.as_mut_ptr(),
            &mut len,
            0,
        )
    };
    assert_eq!(rc, AE_SUCCESS);
    String::from_utf8_lossy(&text[..len as usize]).into_owned()
}

/// Reading more than one field of a record fetches the whole row in one
/// request and answers the rest from the cache; a move throws the cache
/// away, so the next read sees the record it moved to, not the cached one.
#[test]
fn a_row_is_read_once_and_dropped_on_a_move() {
    let _counting = fixtures::counting();
    fixtures::remotely(|| {
        let (connection, table) = fixtures::open("indexes", c"indexed.adt");
        assert_eq!(AdsGotoTop(table), AE_SUCCESS);
        let top = field_text(table, c"id");

        // A move drops the cache: the skipped-to record is read, not the
        // one cached at the top.
        assert_eq!(AdsSkip(table, 1), AE_SUCCESS);
        assert_ne!(field_text(table, c"id"), top, "the skipped-to record");

        // Back at the top, two fields of the record are one request for them
        // both, and the first field reads as it did before.
        assert_eq!(AdsGotoTop(table), AE_SUCCESS);
        fixtures::forget_requests(connection);
        assert_eq!(field_text(table, c"id"), top, "the first record again");
        let _ = field_text(table, c"name");
        assert_eq!(
            fixtures::requests(connection).len(),
            1,
            "one read for the whole row"
        );

        crate::AdsDisconnect(connection);
    });
}

/// An edit of the current record is seen through the cache: changing a field
/// drops the cached row, so the next read gives the new value, not the one
/// read before the change.
#[test]
fn an_edit_is_seen_through_the_cache() {
    let _counting = fixtures::counting();
    fixtures::remotely(|| {
        let source = fixtures::scratch("row_cache_edit", "freelist");
        let connection = fixtures::connect_to(&source);
        let mut table = 0;
        let rc = unsafe {
            crate::AdsOpenTable(
                connection,
                c"fl_chain.adt".as_ptr().cast_mut().cast(),
                std::ptr::null_mut(),
                crate::ADS_ADT,
                crate::ADS_ANSI,
                crate::ADS_PROPRIETARY_LOCKING,
                1,
                crate::ADS_SHARED,
                &mut table,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(AdsGotoTop(table), AE_SUCCESS);
        let name = c"c_name".as_ptr().cast_mut().cast();

        // Read the field (caching it), then change it, then read it again.
        let before = field_text(table, c"c_name");
        let mut text = *b"ZZZ";
        assert_eq!(
            unsafe { crate::AdsSetString(table, name, text.as_mut_ptr(), 3) },
            AE_SUCCESS
        );
        let after = field_text(table, c"c_name");
        assert_eq!(after.trim_end(), "ZZZ", "the edit, not the cached value");
        assert_ne!(after, before);

        crate::AdsDisconnect(connection);
    });
}
