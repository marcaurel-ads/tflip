//! Handles: their kinds and the values clients keep on them, and the
//! checks of handles and out-pointers the other modules share.

use tflip_engine::handle::HandleKind;

use crate::backend::api;
use crate::index::ADS_FTS_INDEX;
use crate::last_error::{Call, fail};
use crate::{
    ADS_CONNECTION, ADS_CURSOR, ADS_DATABASE_CONNECTION, ADS_FALSE, ADS_FTS_INDEX_ORDER,
    ADS_INDEX_ORDER, ADS_STATEMENT, ADS_TABLE, ADS_TRUE, AE_INVALID_CONNECTION_HANDLE,
    AE_INVALID_HANDLE, AE_INVALID_VALUE, AE_SUCCESS, AdsHandle, Unsigned16, Unsigned32, error_code,
};

/// `AdsGetHandleType`: what kind of object a handle refers to, as one of the
/// `ADS_CONNECTION`, `ADS_TABLE`, ... constants. A connection to a data
/// dictionary is `ADS_DATABASE_CONNECTION` and a full-text index
/// `ADS_FTS_INDEX_ORDER` (probed with `tools/ace/write`'s `handletype`). A
/// management handle (`AdsMgConnect`) is a connection to the original
/// (`fixtures/errors/param_n_mg.json`), so it is `ADS_CONNECTION`. A value
/// that is no handle fails with `AE_INVALID_HANDLE` and leaves the last
/// error as it was.
///
/// # Safety
///
/// `handle_type` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetHandleType(
    obj: AdsHandle,
    handle_type: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetHandleType");
    let kind = match api(obj).handle_kind(obj) {
        // A connection to a data dictionary has a kind of its own.
        Ok(Some(HandleKind::Connection)) => match api(obj).connection_info(obj) {
            Ok(info) if info.dictionary => ADS_DATABASE_CONNECTION,
            _ => ADS_CONNECTION,
        },
        Ok(Some(HandleKind::Statement)) => ADS_STATEMENT,
        Ok(Some(HandleKind::Table)) => ADS_TABLE,
        Ok(Some(HandleKind::Cursor)) => ADS_CURSOR,
        // A full-text index is an order of a kind of its own.
        Ok(Some(HandleKind::Index)) => match api(obj).index_info(obj) {
            Ok(order) if order.flags & ADS_FTS_INDEX != 0 => ADS_FTS_INDEX_ORDER,
            _ => ADS_INDEX_ORDER,
        },
        Ok(None) | Err(_) => return crate::last_error::fail_handle(AE_INVALID_HANDLE),
    };
    // A NULL pointer is not refused (`fixtures/errors/param_n.json`).
    if !handle_type.is_null() {
        // SAFETY: not null, so writable, as # Safety says.
        unsafe { *handle_type = kind };
    }
    AE_SUCCESS
}

/// `AdsGetIntProperty`: `ADS_CODE_PAGE` (1), the code page of a table's,
/// cursor's or statement's text, which is always Windows-1252 in tflip. As
/// in the original engine: another property fails with `AE_INVALID_VALUE`
/// before the handle is looked at, another kind of handle with
/// `AE_INVALID_HANDLE`, and a null `property` is not written.
///
/// # Safety
///
/// `property` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetIntProperty(
    obj: AdsHandle,
    property_id: Unsigned32,
    property: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetIntProperty");
    if property_id != ADS_CODE_PAGE {
        let text = "A value was outside of the expected range.  Parameter 2";
        return crate::last_error::fail_text(
            AE_INVALID_VALUE,
            format!("Error {AE_INVALID_VALUE}:  {text}"),
        );
    }
    match api(obj).handle_kind(obj) {
        Ok(Some(HandleKind::Table | HandleKind::Cursor | HandleKind::Statement)) => {
            if !property.is_null() {
                // SAFETY: not null, so writable, as # Safety says.
                unsafe { *property = 1252 };
            }
            AE_SUCCESS
        }
        _ => fail(AE_INVALID_HANDLE),
    }
}

/// `ADS_CODE_PAGE`, the property `AdsGetIntProperty` reports.
const ADS_CODE_PAGE: Unsigned32 = 1;

/// A flag as ACE gives it.
pub(crate) fn flag(value: bool) -> Unsigned16 {
    if value { ADS_TRUE } else { ADS_FALSE }
}

/// Checks that `table` is a table (or cursor), as the original engine does
/// first.
pub(crate) fn table_check(table: AdsHandle) -> Result<(), Unsigned32> {
    crate::filter::table_of(table).map(|_| ())
}

/// Writes `value` to `out` after `check`, or fails with
/// `AE_INVALID_POINTER` if `out` is null.
///
/// # Safety
///
/// `out` must be null or valid for writes.
pub(crate) unsafe fn answer<T>(check: Result<(), Unsigned32>, out: *mut T, value: T) -> Unsigned32 {
    // SAFETY: as the caller promises.
    unsafe { crate::field::write(check.map(|()| value), out) }
}

/// [`answer`] with `out` the call's argument number `param`: a NULL one
/// fails naming its number.
///
/// # Safety
///
/// `out` must be null or valid for writes.
pub(crate) unsafe fn answer_at<T>(
    check: Result<(), Unsigned32>,
    out: *mut T,
    value: T,
    param: u32,
) -> Unsigned32 {
    match check {
        Ok(()) if out.is_null() => crate::last_error::null_param(param),
        // SAFETY: as the caller promises.
        check => unsafe { answer(check, out, value) },
    }
}

/// `AdsGetHandleLong`: the value a client set on a handle with
/// `AdsSetHandleLong`; as in the original engine, a handle without one
/// fails with `AE_INVALID_HANDLE`.
///
/// # Safety
///
/// `value` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetHandleLong(
    handle: AdsHandle,
    value: *mut Unsigned32,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetHandleLong");
    match api(handle).handle_long(handle) {
        // SAFETY: `value` is null or writable, as # Safety says.
        Ok(kept) => unsafe { answer(Ok(()), value, kept) },
        Err(_) => fail(AE_INVALID_HANDLE),
    }
}

/// `AdsSetHandleLong`: keeps a value on an open handle for the client;
/// any other fails with `AE_INVALID_HANDLE`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsSetHandleLong(handle: AdsHandle, value: Unsigned32) -> Unsigned32 {
    let _call = Call::begin("AdsSetHandleLong");
    match api(handle).set_handle_long(handle, value) {
        Ok(()) => AE_SUCCESS,
        Err(_) => fail(AE_INVALID_HANDLE),
    }
}

/// Checks that `connect` is a connection.
pub(crate) fn connection_check(connect: AdsHandle) -> Result<(), Unsigned32> {
    match api(connect).handle_kind(connect) {
        Ok(Some(HandleKind::Connection)) => Ok(()),
        Err(error) => Err(error_code(error)),
        // A value that is no handle: the last error says so.
        Ok(None) => {
            crate::last_error::fail_handle_detail(
                AE_INVALID_HANDLE,
                crate::last_error::NOT_A_HANDLE,
            );
            Err(AE_INVALID_CONNECTION_HANDLE)
        }
        // Kept behind an error left before, as the original engine's
        // transaction calls do (`fixtures/errors/handle_order.json`).
        Ok(_) => Err(crate::last_error::fail_handle_detail(
            AE_INVALID_CONNECTION_HANDLE,
            crate::last_error::NOT_A_CONNECTION,
        )),
    }
}

/// Checks a handle that may be a table or an index order, as the original
/// engine checks it for the calls that move (`AdsGotoTop`): anything else
/// is `AE_INVALID_HANDLE`, which replaces an error left before
/// (`fixtures/errors/handles.json`, `fixtures/errors/handle_order.json`).
pub(crate) fn table_or_order(handle: AdsHandle) -> Result<(), Unsigned32> {
    match api(handle).handle_kind(handle) {
        Ok(Some(HandleKind::Table | HandleKind::Cursor | HandleKind::Index)) => Ok(()),
        Err(error) => Err(error_code(error)),
        Ok(_) => Err(crate::last_error::fail_detail(
            AE_INVALID_HANDLE,
            "Must be a table, cursor or index order handle.",
        )),
    }
}

/// Checks that `handle` is one at all, as the original engine does first
/// for some calls: a value that is no handle fails with `code` (the
/// call's code for a handle of the wrong kind), and its last error, unless
/// one was left before, is `AE_INVALID_HANDLE` (`fixtures/errors/handles.json`).
pub(crate) fn known(handle: AdsHandle, code: Unsigned32) -> Result<(), Unsigned32> {
    match api(handle).handle_kind(handle) {
        Ok(None) => {
            crate::last_error::fail_handle_detail(
                AE_INVALID_HANDLE,
                crate::last_error::NOT_A_HANDLE,
            );
            Err(code)
        }
        _ => Ok(()),
    }
}

/// Checks a handle as [`table_or_order`] for `AdsReindex`: anything else
/// is `AE_INVALID_HANDLE`, but the last error is that of a table call's
/// check, which keeps an error left before.
pub(crate) fn reindexed(handle: AdsHandle) -> Result<(), Unsigned32> {
    match api(handle).handle_kind(handle) {
        Ok(Some(HandleKind::Table | HandleKind::Cursor | HandleKind::Index)) => Ok(()),
        Err(error) => Err(error_code(error)),
        Ok(None) => {
            crate::last_error::fail_handle_detail(
                AE_INVALID_HANDLE,
                crate::last_error::NOT_A_HANDLE,
            );
            Err(AE_INVALID_HANDLE)
        }
        Ok(Some(_)) => {
            let detail = crate::last_error::NOT_A_TABLE;
            crate::last_error::fail_handle_detail(crate::AE_INVALID_TABLE_HANDLE, detail);
            Err(AE_INVALID_HANDLE)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LOCAL;
    use tflip_engine::table::{self, TableSettings, TableType};
    use tflip_engine::{connection, statement};

    #[test]
    fn handle_types() {
        let mut kind = 0;
        let connect = connection::connect(&LOCAL, "Data Source=x;ServerType=LOCAL;").unwrap();
        assert_eq!(unsafe { AdsGetHandleType(connect, &mut kind) }, AE_SUCCESS);
        assert_eq!(kind, ADS_CONNECTION);
        let stmt = statement::create(&LOCAL, connect).unwrap();
        assert_eq!(unsafe { AdsGetHandleType(stmt, &mut kind) }, AE_SUCCESS);
        assert_eq!(kind, ADS_STATEMENT);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
        assert_eq!(
            unsafe { AdsGetHandleType(connect, &mut kind) },
            AE_INVALID_HANDLE
        );
        assert_eq!(
            unsafe { AdsGetHandleType(stmt, &mut kind) },
            AE_INVALID_HANDLE
        );
        let settings = TableSettings {
            table_type: TableType::Cdx,
            char_type: 1,
            collation: String::new(),
            options: 0,
            ..TableSettings::default()
        };
        let path = crate::fixtures::dir("cdx/dbf_types").join("dbf_types.dbf");
        let table = table::open(&LOCAL, 0, path.to_str().unwrap(), settings).unwrap();
        assert_eq!(unsafe { AdsGetHandleType(table, &mut kind) }, AE_SUCCESS);
        assert_eq!(kind, ADS_TABLE);
        assert_eq!(unsafe { AdsGetHandleType(0, &mut kind) }, AE_INVALID_HANDLE);
    }

    /// Calls that take one handle, given a value that is no handle, 0, and
    /// handles of the wrong kinds, return what the original engine returns
    /// (`fixtures/errors/handles.json`, from `tools/ace/errors/handles.c`).
    /// No error is left before each call, as in the probe, so the last
    /// errors are compared too (`last_errors_after_wrong_handles` has calls
    /// after errors).
    #[test]
    fn wrong_handles() {
        use crate::fixtures;
        use serde_json::Value as Json;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/errors/handles.json");
        let rows: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let (mut order, mut n) = (0, 1u16);
        assert_eq!(
            unsafe { crate::AdsGetAllIndexes(table, &mut order, &mut n) },
            AE_SUCCESS
        );
        let mut stmt = 0;
        assert_eq!(
            unsafe { crate::AdsCreateSQLStatement(connect, &mut stmt) },
            AE_SUCCESS
        );
        type Call = extern "system" fn(AdsHandle) -> Unsigned32;
        let call = |name: &str| -> Call {
            match name {
                "AdsGotoTop" => crate::AdsGotoTop,
                "AdsGotoBottom" => crate::AdsGotoBottom,
                "AdsWriteRecord" => crate::AdsWriteRecord,
                "AdsRefreshRecord" => crate::AdsRefreshRecord,
                "AdsClearFilter" => crate::AdsClearFilter,
                "AdsClearAOF" => crate::AdsClearAOF,
                "AdsClearRelation" => crate::AdsClearRelation,
                "AdsLockTable" => crate::AdsLockTable,
                "AdsUnlockTable" => crate::AdsUnlockTable,
                "AdsAppendRecord" => crate::AdsAppendRecord,
                "AdsDeleteRecord" => crate::AdsDeleteRecord,
                "AdsRecallRecord" => crate::AdsRecallRecord,
                "AdsCancelUpdate" => crate::AdsCancelUpdate,
                "AdsReindex" => crate::AdsReindex,
                "AdsCloseTable" => crate::AdsCloseTable,
                "AdsPackTable" => crate::AdsPackTable,
                "AdsBeginTransaction" => crate::AdsBeginTransaction,
                "AdsCommitTransaction" => crate::AdsCommitTransaction,
                "AdsRollbackTransaction" => crate::AdsRollbackTransaction,
                "AdsCloseSQLStatement" => crate::AdsCloseSQLStatement,
                "AdsClearSQLParams" => crate::AdsClearSQLParams,
                "AdsRefreshAOF" => crate::AdsRefreshAOF,
                "AdsFlushFileBuffers" => crate::AdsFlushFileBuffers,
                "AdsRecallAllRecords" => crate::AdsRecallAllRecords,
                other => panic!("{other}"),
            }
        };
        let mut problems = Vec::new();
        for row in &rows {
            let (name, kind) = (row["fn"].as_str().unwrap(), row["handle"].as_str().unwrap());
            let handle = match kind {
                "none" => 0x12345,
                "zero" => 0,
                "connection" => connect,
                "table" => table,
                "order" => order,
                _ => stmt,
            };
            // A known difference: the original takes an index order's
            // connection for a transaction.
            let known = name.contains("Transaction") && kind == "order";
            if known {
                continue;
            }
            // No error before the call, as in the probe.
            assert_eq!(crate::AdsGotoTop(table), AE_SUCCESS);
            let rc = u64::from(call(name)(handle));
            let mut last = 0;
            let (mut buf, mut len) = ([0u8; 600], 600u16);
            unsafe { crate::AdsGetLastError(&mut last, buf.as_mut_ptr(), &mut len) };
            let got = (rc, u64::from(last));
            let want = (row["rc"].as_u64().unwrap(), row["last"].as_u64().unwrap());
            if got != want {
                problems.push(format!("{name}({kind}): got {got:?}, want {want:?}"));
            }
        }
        crate::AdsDisconnect(connect);
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// The last error calls given wrong handles leave one after another,
    /// with its text, which depends on the error the call before left
    /// (`fixtures/errors/handle_order.json`, from
    /// `tools/ace/errors/handle_order.c`).
    #[test]
    fn last_errors_after_wrong_handles() {
        use crate::fixtures;
        use serde_json::Value as Json;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let rows: Vec<Json> = serde_json::from_str(
            &std::fs::read_to_string(root.join("errors/handle_order.json")).unwrap(),
        )
        .unwrap();
        let folder = root.join("write/goto_deleted/before");
        let connect = fixtures::connect_to(folder.to_str().unwrap());
        let open = || {
            let mut table = 0;
            let rc = unsafe {
                crate::AdsOpenTable(
                    connect,
                    c"p.adt".as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    crate::ADS_ADT,
                    1,
                    1,
                    1,
                    2,
                    &mut table,
                )
            };
            assert_eq!(rc, AE_SUCCESS);
            table
        };
        let table = open();
        let closed = open();
        assert_eq!(crate::AdsCloseTable(closed), AE_SUCCESS);
        let (mut order, mut n) = (0, 1u16);
        assert_eq!(
            unsafe { crate::AdsGetAllIndexes(table, &mut order, &mut n) },
            AE_SUCCESS
        );
        let mut stmt = 0;
        assert_eq!(
            unsafe { crate::AdsCreateSQLStatement(connect, &mut stmt) },
            AE_SUCCESS
        );
        type Call = extern "system" fn(AdsHandle) -> Unsigned32;
        let call = |name: &str| -> Call {
            match name {
                "AdsGotoTop" => crate::AdsGotoTop,
                "AdsWriteRecord" => crate::AdsWriteRecord,
                "AdsClearAOF" => crate::AdsClearAOF,
                "AdsLockTable" => crate::AdsLockTable,
                "AdsReindex" => crate::AdsReindex,
                "AdsCloseTable" => crate::AdsCloseTable,
                "AdsRollbackTransaction" => crate::AdsRollbackTransaction,
                "AdsCloseSQLStatement" => crate::AdsCloseSQLStatement,
                "AdsClearSQLParams" => crate::AdsClearSQLParams,
                other => panic!("{other}"),
            }
        };
        let mut problems = Vec::new();
        for row in &rows {
            let step = row["step"].as_str().unwrap();
            let (name, kind) = step.split_once(' ').unwrap();
            let handle = match kind {
                "conn" => connect,
                "table" => table,
                "order" => order,
                "stmt" => stmt,
                "none" => 0x12345,
                // The original engine gave the statement opened after the
                // table closed the table's handle.
                "closed" => stmt,
                _ => 0,
            };
            let rc = call(name)(handle);
            let (mut code, mut buf, mut len) = (0, [0u8; 600], 600u16);
            unsafe { crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
            let text = match code {
                0 => String::new(),
                _ => crate::ansi::to_string(&buf[..usize::from(len)]),
            };
            let got = (u64::from(rc), u64::from(code), text);
            let want = (
                row["rc"].as_u64().unwrap(),
                row["last"].as_u64().unwrap(),
                row["text"].as_str().unwrap().to_owned(),
            );
            if got != want {
                problems.push(format!("{step}: got {got:?}\n  want {want:?}"));
            }
        }
        crate::AdsDisconnect(connect);
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }
}
