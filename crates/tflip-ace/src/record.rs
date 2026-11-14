//! Moving through a table's records, bookmarks, reading whole records, and
//! writing them: appending, deleting and locking.
//!
//! A table is on a record, or before its first record (BOF) or after its
//! last (EOF); an empty table is at both. Deleted records are never visible.
//! See [`tflip_engine::table`] for how moves behave at the ends.
//!
//! A record that is appended or changed (`AdsAppendRecord`, `AdsSetString`,
//! ...) is written by `AdsWriteRecord`, or when the table moves or closes,
//! as in the original engine; until then `AdsCancelUpdate` drops the
//! changes.

use tflip_engine::Error;

use crate::backend::{Backend, api};
use crate::ffi::slice;
use crate::handle::{flag, table_check};
use crate::last_error::{Call, fail, fail_handle};
use crate::{
    ADS_FALSE, ADS_IGNOREFILTERS, ADS_RESPECTFILTERS, AE_INSUFFICIENT_BUFFER,
    AE_INVALID_FILTER_OPTION, AE_INVALID_HANDLE, AE_INVALID_POINTER, AE_INVALID_TABLE_HANDLE,
    AE_SUCCESS, AdsHandle, Signed32, Unsigned8, Unsigned16, Unsigned32, code, error_code,
};

/// Writes a flag read from the engine to `out` as `ADS_TRUE` or
/// `ADS_FALSE`.
///
/// # Safety
///
/// `out` must be null or valid for writes.
unsafe fn write_flag(value: Result<bool, Error>, out: *mut Unsigned16, param: u32) -> Unsigned32 {
    // SAFETY: as the caller promises.
    unsafe { crate::field::write_at(value.map(flag).map_err(error_code), out, param) }
}

/// Writes `bytes` to `buf` of `*len` bytes and sets `*len` to their length;
/// if they do not fit, the buffer is left alone, `*len` is set to the
/// length needed and `AE_INSUFFICIENT_BUFFER` is returned.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
unsafe fn write_bytes(
    bytes: &[u8],
    buf: *mut Unsigned8,
    len: *mut Unsigned32,
    param: u32,
) -> Unsigned32 {
    if let Some(code) = crate::ffi::null_buffer(buf, len, param) {
        return code;
    }
    let length = bytes.len() as Unsigned32;
    // SAFETY: neither is null, so `len` is readable and writable and `buf`
    // holds `*len` bytes, at least as many as are copied.
    unsafe {
        if *len < length {
            *len = length;
            return fail(AE_INSUFFICIENT_BUFFER);
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
        *len = length;
    }
    AE_SUCCESS
}

/// `AdsGotoTop`: moves to the first record (to BOF and EOF if there is
/// none): of a table in record order, or of an index order in key order.
#[unsafe(no_mangle)]
pub extern "system" fn AdsGotoTop(obj: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGotoTop");
    if let Err(code) = crate::handle::table_or_order(obj) {
        return code;
    }
    code(api(obj).goto_top(obj))
}

/// `AdsGotoBottom`: moves to the last record (to BOF and EOF if there is
/// none): of a table in record order, or of an index order in key order.
#[unsafe(no_mangle)]
pub extern "system" fn AdsGotoBottom(obj: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGotoBottom");
    if let Err(code) = crate::handle::table_or_order(obj) {
        return code;
    }
    code(api(obj).goto_bottom(obj))
}

/// The error for a handle that is not a table: `AE_INVALID_HANDLE` if the
/// engine does not know it at all, else `AE_INVALID_TABLE_HANDLE`.
fn not_a_table(handle: AdsHandle) -> Unsigned32 {
    match api(handle).handle_kind(handle) {
        Ok(None) => fail_handle(AE_INVALID_HANDLE),
        Ok(Some(_)) => fail_handle(AE_INVALID_TABLE_HANDLE),
        Err(error) => error_code(error),
    }
}

/// The ACE code for a move of a table that is not one when the handle is
/// not a table (see [`not_a_table`]).
fn moved(table: AdsHandle, result: Result<(), Error>) -> Unsigned32 {
    match result {
        Err(error) if *error.plain() == Error::InvalidTable => not_a_table(table),
        result => code(result),
    }
}

/// `AdsGotoBOF`: moves before a table's first record, so that skipping one
/// record forward lands on it. A handle the engine does not know fails with
/// `AE_INVALID_HANDLE`.
///
/// Unpublished: not declared in the SDK's `ace.h`; the signature is
/// `UNSIGNED32 AdsGotoBOF( ADSHANDLE hTbl )`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsGotoBOF(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGotoBOF");
    moved(table, api(table).goto_bof(table))
}

/// `AdsGotoEOF`: moves after a table's last record, so that EOF is true and
/// there is no current record; skipping one record back lands on the last. A
/// handle the engine does not know fails with `AE_INVALID_HANDLE`.
///
/// Unpublished: not declared in the SDK's `ace.h`; the signature is
/// `UNSIGNED32 AdsGotoEOF( ADSHANDLE hTbl )`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsGotoEOF(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGotoEOF");
    moved(table, api(table).goto_eof(table))
}

/// `AdsSkip`: moves `records` records forward, or back if negative, stopping
/// at EOF or BOF: in record order for a table, in key order for an index
/// order.
#[unsafe(no_mangle)]
pub extern "system" fn AdsSkip(obj: AdsHandle, records: Signed32) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSkip");
    code(api(obj).skip(obj, i64::from(records)))
}

/// `AdsGotoRecord`: moves to record number `record`. As in the original
/// engine, 0 or a number past the last record goes to BOF and EOF at once
/// (a skip forward from there starts at the first record), and a deleted
/// record fails with `AE_INVALID_RECORD_NUMBER`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsGotoRecord(table: AdsHandle, record: Unsigned32) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGotoRecord");
    let backend = api(table);
    match backend.goto_record(table, record) {
        // An ADT table's refusal says why, whatever the number.
        Err(Error::InvalidRecordNumber) if is_adt(backend, table) => {
            crate::last_error::fail_detail(
                crate::AE_INVALID_RECORD_NUMBER,
                "You cannot go to deleted records in ADT tables.",
            )
        }
        result => code(result),
    }
}

/// Whether `table` is an ADT table.
fn is_adt(backend: Backend, table: AdsHandle) -> bool {
    let info = backend.table_info(table);
    info.is_ok_and(|info| info.structure.header.format == tflip_storage::table::Format::Adt)
}

/// `AdsAppendRecord`: appends an empty record, locked until it is written,
/// and goes to it. In an ADT table it takes the slot of the last deleted
/// record if there is one.
#[unsafe(no_mangle)]
pub extern "system" fn AdsAppendRecord(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsAppendRecord");
    if let Err(code) = crate::handle::known(table, crate::AE_INVALID_TABLE_HANDLE) {
        return code;
    }
    code(api(table).append_record(table))
}

/// `AdsWriteRecord`: writes the changes of the current record, releasing
/// the lock taken for them.
#[unsafe(no_mangle)]
pub extern "system" fn AdsWriteRecord(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsWriteRecord");
    // As in the original engine, an index order's handle is no table here.
    if let Err(code) = table_check(table) {
        return code;
    }
    code(api(table).write_record(table))
}

/// `AdsFlushFileBuffers`: writes the changes of the current record, as
/// `AdsWriteRecord` does; a table opened read-only fails with
/// `AE_TABLE_READONLY`. (tflip writes through to the files; nothing else is
/// held back.)
#[unsafe(no_mangle)]
pub extern "system" fn AdsFlushFileBuffers(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsFlushFileBuffers");
    if let Err(code) = crate::handle::known(table, crate::AE_INVALID_TABLE_HANDLE) {
        return code;
    }
    code(api(table).flush_table(table))
}

/// `AdsSetRecord`: replaces the current record's bytes by the `len` bytes
/// at `record`, but for the first (the record's status), as a change
/// written with the record; on an index order, the record its raw key is
/// built from (`AdsInitRawKey`). `len` must be the table's record length
/// (`AE_INVALID_RECORD_LENGTH`).
///
/// # Safety
///
/// `record` must be null or hold `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetRecord(
    handle: AdsHandle,
    record: *mut Unsigned8,
    len: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetRecord");
    // SAFETY: `record` is null or holds `len` bytes, as # Safety says.
    let Some(bytes) = (unsafe { slice(record, len as usize) }) else {
        return crate::last_error::null_param(2);
    };
    code(api(handle).set_record(handle, bytes.to_vec()))
}

/// `AdsWriteAllRecords`: writes the changes of every open table.
#[unsafe(no_mangle)]
pub extern "system" fn AdsWriteAllRecords() -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsWriteAllRecords");
    code(crate::backend::each(|api| api.write_all_records()))
}

/// `AdsDeleteRecord`: deletes the current record when it is written; in an
/// ADT table it then goes on the list of slots for appends to reuse.
#[unsafe(no_mangle)]
pub extern "system" fn AdsDeleteRecord(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDeleteRecord");
    if let Err(code) = crate::handle::known(table, crate::AE_INVALID_TABLE_HANDLE) {
        return code;
    }
    code(api(table).delete_record(table))
}

/// `AdsRecallRecord`: undoes a delete of the current record that was not
/// written yet.
#[unsafe(no_mangle)]
pub extern "system" fn AdsRecallRecord(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsRecallRecord");
    if let Err(code) = crate::handle::known(table, crate::AE_INVALID_TABLE_HANDLE) {
        return code;
    }
    code(api(table).recall_record(table))
}

/// `AdsCancelUpdate`: drops the changes of the current record. A record
/// appended to an ADT table is deleted, and the table is then on no record.
#[unsafe(no_mangle)]
pub extern "system" fn AdsCancelUpdate(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCancelUpdate");
    if let Err(code) = crate::handle::known(table, crate::AE_INVALID_TABLE_HANDLE) {
        return code;
    }
    code(api(table).cancel_update(table))
}

/// `AdsCancelUpdate90`: [`AdsCancelUpdate`]; the options are taken and not
/// used.
#[unsafe(no_mangle)]
pub extern "system" fn AdsCancelUpdate90(table: AdsHandle, options: Unsigned32) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCancelUpdate90");
    let _ = options;
    code(api(table).cancel_update(table))
}

/// `AdsLockRecord`: locks record `record`, 0 for the current one; a record
/// another table handle holds fails with `AE_LOCK_FAILED`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsLockRecord(table: AdsHandle, record: Unsigned32) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsLockRecord");
    code(api(table).lock_record(table, record))
}

/// `AdsUnlockRecord`: releases the lock on record `record`, 0 for the
/// current one, writing its changes first.
#[unsafe(no_mangle)]
pub extern "system" fn AdsUnlockRecord(table: AdsHandle, record: Unsigned32) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsUnlockRecord");
    code(api(table).unlock_record(table, record))
}

/// `AdsLockTable`: locks the whole table.
#[unsafe(no_mangle)]
pub extern "system" fn AdsLockTable(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsLockTable");
    if let Err(code) = crate::handle::known(table, crate::AE_INVALID_TABLE_HANDLE) {
        return code;
    }
    code(api(table).lock_table(table))
}

/// `AdsUnlockTable`: releases the table lock.
#[unsafe(no_mangle)]
pub extern "system" fn AdsUnlockTable(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsUnlockTable");
    if let Err(code) = crate::handle::known(table, crate::AE_INVALID_TABLE_HANDLE) {
        return code;
    }
    code(api(table).unlock_table(table))
}

/// `AdsAtBOF`: whether a table is before its first record (`ADS_TRUE`).
///
/// # Safety
///
/// `bof` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsAtBOF(table: AdsHandle, bof: *mut Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsAtBOF");
    let value = api(table).position(table).map(|position| position.bof);
    // SAFETY: `bof` is null or writable, as # Safety says.
    unsafe { write_flag(value, bof, 2) }
}

/// `AdsAtEOF`: whether a table is after its last record (`ADS_TRUE`).
///
/// # Safety
///
/// `eof` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsAtEOF(table: AdsHandle, eof: *mut Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsAtEOF");
    let value = api(table).position(table).map(|position| position.eof);
    // SAFETY: `eof` is null or writable, as # Safety says.
    unsafe { write_flag(value, eof, 2) }
}

/// `AdsIsFound`: whether the last move of a table (`obj` is the table or one
/// of its index orders) was an `AdsSeek` that found its key (`ADS_TRUE`).
/// Any other move clears it; a seek that fails leaves it.
///
/// As in the original engine, any handle that is not a table or an index
/// order fails with `AE_INVALID_HANDLE`.
///
/// # Safety
///
/// `found` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsFound(obj: AdsHandle, found: *mut Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsFound");
    let flag = match api(obj).found(obj) {
        Ok(flag) => flag,
        Err(Error::ConnectionLost) => return error_code(Error::ConnectionLost),
        Err(_) => return fail_handle(AE_INVALID_HANDLE),
    };
    // SAFETY: `found` is null or writable, as # Safety says.
    unsafe { write_flag(Ok(flag), found, 2) }
}

/// `AdsIsRecordDeleted`: whether the current record is deleted. Deleted
/// records are never visible in ADT tables, so there it is always
/// `ADS_FALSE`; a DBF table's can be. At BOF or EOF it fails with
/// `AE_NO_CURRENT_RECORD`, as in the original engine.
///
/// # Safety
///
/// `deleted` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsRecordDeleted(
    table: AdsHandle,
    deleted: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsRecordDeleted");
    let value = api(table).is_deleted(table);
    // SAFETY: `deleted` is null or writable, as # Safety says.
    unsafe { write_flag(value, deleted, 2) }
}

/// `AdsIsRecordVisible`: whether the current record passes the table's
/// filters (deleted records never do). Off a record it fails with
/// `AE_NO_CURRENT_RECORD`, setting `*visible` to `ADS_FALSE`, as in the
/// original engine.
///
/// # Safety
///
/// `visible` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsRecordVisible(
    table: AdsHandle,
    visible: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsRecordVisible");
    match api(table).is_visible(table) {
        Err(Error::NoCurrentRecord) if !visible.is_null() => {
            // SAFETY: not null, so writable, as # Safety says.
            unsafe { *visible = ADS_FALSE };
            error_code(Error::NoCurrentRecord)
        }
        // SAFETY: `visible` is null or writable, as # Safety says.
        result => unsafe { write_flag(result, visible, 2) },
    }
}

/// `AdsGetRecordNum`: the current record's number, 0 at BOF, as in the
/// original engine. With `ADS_IGNOREFILTERS` it is the physical record
/// number, one past the last slot at EOF; with `ADS_RESPECTFILTERS` it
/// counts the visible records only (deleted records are never visible),
/// one past the last of them at EOF (1 in an empty table). On a record the
/// filters hide, that fails with `AE_NOT_FOUND`, and the table goes to EOF,
/// as in the original engine. Other options fail with
/// `AE_INVALID_FILTER_OPTION`, leaving `*record` alone.
///
/// # Safety
///
/// `record` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetRecordNum(
    table: AdsHandle,
    filter_option: Unsigned16,
    record: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetRecordNum");
    let backend = api(table);
    if record.is_null() {
        return match backend.position(table) {
            Ok(_) => crate::last_error::null_param(3),
            Err(error) => error_code(error),
        };
    }
    let number = match filter_option {
        ADS_IGNOREFILTERS => backend.record_number(table, false),
        ADS_RESPECTFILTERS => backend.record_number(table, true),
        _ => match backend.position(table) {
            Ok(_) => {
                return crate::last_error::fail_detail(AE_INVALID_FILTER_OPTION, "Parameter 2");
            }
            Err(error) => Err(error),
        },
    };
    // SAFETY: `record` is null or writable, as # Safety says.
    unsafe { crate::field::write(number.map_err(error_code), record) }
}

/// `AdsRefreshRecord`: rereads the current record. Records are read from the
/// file whenever they are needed, so there is nothing to refresh.
#[unsafe(no_mangle)]
pub extern "system" fn AdsRefreshRecord(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsRefreshRecord");
    if let Err(code) = crate::handle::known(table, crate::AE_INVALID_TABLE_HANDLE) {
        return code;
    }
    code(api(table).position(table).map(|_| ()))
}

/// `AdsGetRecord`: the current record's raw bytes, its 5-byte prefix
/// included, in `buf` of `*len` bytes; `*len` is set to the record length. A
/// buffer that is too small fails with `AE_INSUFFICIENT_BUFFER` (with `*len`
/// set to the length needed), and BOF or EOF with `AE_NO_CURRENT_RECORD`.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetRecord(
    table: AdsHandle,
    buf: *mut Unsigned8,
    len: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetRecord");
    let record = match api(table).read_record(table) {
        Ok(record) => record,
        Err(error) => return error_code(error),
    };
    // SAFETY: `buf` and `len` are as # Safety says.
    unsafe { write_bytes(&record, buf, len, 2) }
}

/// `AdsGetRecordCRC` option: leave out where memo values are kept.
const ADS_CRC_IGNOREMEMOPAGES: Unsigned32 = 2;

/// What the original engine fails `AdsGetRecordCRC` with at BOF or EOF with
/// `ADS_CRC_IGNOREMEMOPAGES` (not in `ace.h`).
const CRC_NO_RECORD: Unsigned32 = 7038;

/// `AdsGetRecordCRC`: a checksum of the current record, for telling whether
/// it changed (see [`tflip_engine::table::Table::record_crc`]); with
/// `ADS_CRC_IGNOREMEMOPAGES` without the places of its memo values, and at
/// BOF or EOF failing with 7038 then; `ADS_CRC_LOCALLY` changes nothing.
/// All as the original engine computes it.
///
/// # Safety
///
/// `crc` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetRecordCRC(
    table: AdsHandle,
    crc: *mut Unsigned32,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetRecordCRC");
    let backend = match crate::filter::table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    if crc.is_null() {
        return crate::last_error::null_param(2);
    }
    let ignore_pages = options & ADS_CRC_IGNOREMEMOPAGES != 0;
    match backend.record_crc(table, ignore_pages) {
        Ok(value) => {
            // SAFETY: not null, so writable, as # Safety says.
            unsafe { *crc = value };
            AE_SUCCESS
        }
        Err(Error::NoCurrentRecord) if ignore_pages => fail(CRC_NO_RECORD),
        Err(error) => error_code(error),
    }
}

/// `AdsIsRecordLocked`: whether the table handle holds a lock on record
/// `record` (0 for the current one), or the table lock.
///
/// # Safety
///
/// `locked` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsRecordLocked(
    table: AdsHandle,
    record: Unsigned32,
    locked: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsIsRecordLocked");
    let value = table_check(table).and_then(|()| {
        api(table)
            .is_record_locked(table, record)
            .map_err(error_code)
    });
    // SAFETY: `locked` is null or writable, as # Safety says.
    unsafe { crate::field::write_at(value.map(flag), locked, 3) }
}

/// `AdsIsTableLocked`: whether the table handle holds the table lock.
///
/// # Safety
///
/// `locked` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsTableLocked(
    table: AdsHandle,
    locked: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsIsTableLocked");
    let value =
        table_check(table).and_then(|()| api(table).is_table_locked(table).map_err(error_code));
    // SAFETY: `locked` is null or writable, as # Safety says.
    unsafe { crate::field::write_at(value.map(flag), locked, 2) }
}

/// `AdsGetNumLocks`: how many records the table handle holds locks on.
///
/// # Safety
///
/// `count` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetNumLocks(
    table: AdsHandle,
    count: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetNumLocks");
    let value = table_check(table).and_then(|()| api(table).locks(table).map_err(error_code));
    // SAFETY: `count` is null or writable, as # Safety says.
    unsafe { crate::field::write_at(value.map(|locks| locks.len() as Unsigned16), count, 2) }
}

/// `AdsGetAllLocks`: the records the table handle holds locks on, in record
/// order, as many as `*len` says fit; `*len` is set to how many were
/// written. More than fit fail with `AE_INSUFFICIENT_BUFFER`.
///
/// # Safety
///
/// `len` must be null or valid for reads and writes; `locks` must be null or
/// hold `*len` items.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetAllLocks(
    table: AdsHandle,
    locks: *mut Unsigned32,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetAllLocks");
    if locks.is_null() {
        return crate::last_error::null_param(2);
    }
    if len.is_null() {
        return crate::last_error::null_param(3);
    }
    let held = match table_check(table).and_then(|()| api(table).locks(table).map_err(error_code)) {
        Ok(held) => held,
        Err(code) => return code,
    };
    // SAFETY: not null, so readable, as # Safety says.
    let room = usize::from(unsafe { *len });
    for (i, &record) in held.iter().take(room).enumerate() {
        // SAFETY: not null, so it holds `room` items, as # Safety says.
        unsafe { *locks.add(i) = record };
    }
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *len = held.len() as Unsigned16 };
    match held.len() > room {
        true => fail(AE_INSUFFICIENT_BUFFER),
        false => AE_SUCCESS,
    }
}

/// `AdsGetNullRecord`: fills `buf` (of `len` bytes) with an empty record of
/// a table, of `AdsGetRecordLength` bytes: a zero record prefix, then each
/// field's NULL value, as the original engine fills it in. A buffer shorter
/// than a record fails with `AE_INSUFFICIENT_BUFFER`; the bytes past the
/// record are left alone.
///
/// Not declared in the SDK's `ace.h`; the signature is
/// `UNSIGNED32 AdsGetNullRecord( ADSHANDLE hTbl, UNSIGNED8 *pucBuf, UNSIGNED32 ulLen )`.
///
/// # Safety
///
/// `buf` must be null or hold `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetNullRecord(
    table: AdsHandle,
    buf: *mut Unsigned8,
    len: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetNullRecord");
    let record = api(table).table_info(table).map(|info| info.null_record);
    // SAFETY: `buf` is null or holds `len` bytes, as # Safety says.
    unsafe { write_null_record(record, buf, len) }
}

/// `AdsClearRecordBuffer`: [`AdsGetNullRecord`] into a client's record
/// buffer, as Delphi's `TAdsDataSet` initializes a record to insert. As in
/// the original engine, a buffer shorter than a record is
/// `AE_INSUFFICIENT_BUFFER`, and a handle that is not a table or cursor
/// `AE_INVALID_TABLE_HANDLE`. Not declared in the SDK's
/// `ace.h`; the signature is `UNSIGNED32 AdsClearRecordBuffer( ADSHANDLE
/// hTbl, UNSIGNED8 *pucBuf, UNSIGNED32 ulLen )` (`aceunpub.pas`).
///
/// # Safety
///
/// `buf` must be null or hold `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsClearRecordBuffer(
    table: AdsHandle,
    buf: *mut Unsigned8,
    len: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsClearRecordBuffer");
    let backend = match crate::filter::table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    let record = backend.table_info(table).map(|info| info.null_record);
    // SAFETY: `buf` is null or holds `len` bytes, as # Safety says.
    unsafe { write_null_record(record, buf, len) }
}

/// Writes an empty record, `record`, to `buf` of `len` bytes; a short
/// buffer is `AE_INSUFFICIENT_BUFFER`, as in the original engine not a last
/// error.
///
/// # Safety
///
/// `buf` must be null or hold `len` bytes.
unsafe fn write_null_record(
    record: Result<Vec<u8>, Error>,
    buf: *mut Unsigned8,
    len: Unsigned32,
) -> Unsigned32 {
    let record = match record {
        Ok(record) => record,
        Err(error) => return error_code(error),
    };
    if buf.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    if (len as usize) < record.len() {
        return AE_INSUFFICIENT_BUFFER;
    }
    // SAFETY: not null, so `len` bytes, no fewer than the record's.
    unsafe { std::ptr::copy_nonoverlapping(record.as_ptr(), buf, record.len()) };
    AE_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ADS_TRUE, AE_INVALID_RECORD_NUMBER, AE_INVALID_TABLE_HANDLE, AE_NO_CURRENT_RECORD, fixtures,
    };

    fn record_num(table: AdsHandle) -> u32 {
        let mut record = 0xffff_ffff;
        assert_eq!(
            unsafe { AdsGetRecordNum(table, ADS_IGNOREFILTERS, &mut record) },
            AE_SUCCESS
        );
        record
    }

    /// A flag function such as `AdsAtEOF`.
    type FlagFn = unsafe extern "system" fn(AdsHandle, *mut Unsigned16) -> Unsigned32;

    fn flag(f: FlagFn, table: AdsHandle) -> bool {
        let mut value = 0xffff;
        assert_eq!(unsafe { f(table, &mut value) }, AE_SUCCESS);
        value == ADS_TRUE
    }

    #[test]
    fn walks_records() {
        // Slots 1, 3 and 6 hold deleted records.
        let (connect, table) = fixtures::open("freelist", c"fl_chain.adt");
        assert_eq!(record_num(table), 2);
        let mut seen = Vec::new();
        assert_eq!(AdsGotoBOF(table), AE_SUCCESS);
        assert!(flag(AdsAtBOF, table));
        loop {
            assert_eq!(AdsSkip(table, 1), AE_SUCCESS);
            if flag(AdsAtEOF, table) {
                break;
            }
            assert!(!flag(AdsIsRecordDeleted, table));
            assert!(flag(AdsIsRecordVisible, table));
            seen.push(record_num(table));
        }
        assert_eq!(seen, [2, 4, 5, 7, 8]);
        assert_eq!(record_num(table), 9, "one past the last record");
        // Respecting filters, only the visible records count.
        let visible_num = || {
            let mut record = 0xffff_ffff;
            let rc = unsafe { AdsGetRecordNum(table, ADS_RESPECTFILTERS, &mut record) };
            assert_eq!(rc, AE_SUCCESS);
            record
        };
        assert_eq!(visible_num(), 6, "one past the last visible record");
        assert_eq!(AdsGotoRecord(table, 7), AE_SUCCESS);
        assert_eq!(visible_num(), 4);
        let mut record = 0xffff_ffff;
        for option in [0, 3, 4] {
            let rc = unsafe { AdsGetRecordNum(table, option, &mut record) };
            assert_eq!(
                (rc, record),
                (AE_INVALID_FILTER_OPTION, 0xffff_ffff),
                "{option}"
            );
        }
        assert_eq!(AdsGotoEOF(table), AE_SUCCESS);
        let mut visible = 0xffff;
        let rc = unsafe { AdsIsRecordVisible(table, &mut visible) };
        assert_eq!(
            (rc, visible),
            (AE_NO_CURRENT_RECORD, ADS_FALSE),
            "nothing is visible at EOF"
        );

        assert_eq!(AdsGotoBottom(table), AE_SUCCESS);
        assert_eq!(record_num(table), 8);
        assert_eq!(AdsGotoTop(table), AE_SUCCESS);
        assert_eq!(record_num(table), 2);
        assert_eq!(AdsSkip(table, -1), AE_SUCCESS);
        assert!(flag(AdsAtBOF, table) && !flag(AdsAtEOF, table));
        assert_eq!(record_num(table), 0);

        assert_eq!(AdsGotoRecord(table, 5), AE_SUCCESS);
        assert_eq!(record_num(table), 5);
        assert_eq!(AdsGotoRecord(table, 3), AE_INVALID_RECORD_NUMBER, "deleted");
        assert_eq!(record_num(table), 3);
        // As in the original engine: 0 and past the last slot go to BOF and
        // EOF at once, and a skip forward from there starts at the first
        // record, a skip back stays at BOF.
        for past in [0, 99] {
            assert_eq!(AdsGotoRecord(table, past), AE_SUCCESS);
            assert!(flag(AdsAtBOF, table) && flag(AdsAtEOF, table), "{past}");
            assert_eq!(record_num(table), 9);
            let mut deleted = 0xffff;
            let rc = unsafe { AdsIsRecordDeleted(table, &mut deleted) };
            assert_eq!((rc, deleted), (AE_NO_CURRENT_RECORD, 0xffff));
            assert_eq!(AdsSkip(table, 2), AE_SUCCESS);
            assert_eq!(record_num(table), 4);
            assert_eq!(AdsGotoRecord(table, past), AE_SUCCESS);
            assert_eq!(AdsSkip(table, -1), AE_SUCCESS);
            assert!(flag(AdsAtBOF, table) && !flag(AdsAtEOF, table));
        }
        assert_eq!(AdsRefreshRecord(table), AE_SUCCESS);

        assert_eq!(AdsGotoEOF(table), AE_SUCCESS);
        assert!(flag(AdsAtEOF, table) && !flag(AdsAtBOF, table));
        assert_eq!(record_num(table), 9);
        assert_eq!(AdsSkip(table, -1), AE_SUCCESS);
        assert_eq!(record_num(table), 8);
        assert_eq!(AdsGotoEOF(0x1234_5678), AE_INVALID_HANDLE);
        assert_eq!(AdsGotoBOF(0x1234_5678), AE_INVALID_HANDLE);
        assert_eq!(AdsGotoEOF(connect), AE_INVALID_TABLE_HANDLE);

        // A move that takes a table or an order (`fixtures/errors/handles.json`).
        assert_eq!(AdsGotoTop(connect), AE_INVALID_HANDLE);
        assert_eq!(AdsSkip(connect, 1), AE_INVALID_TABLE_HANDLE);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn reads_whole_records() {
        let (connect, table) = fixtures::open("freelist", c"fl_chain.adt");
        let mut length = 0;
        assert_eq!(
            unsafe { crate::AdsGetRecordLength(table, &mut length) },
            AE_SUCCESS
        );
        let mut buf = vec![0u8; length as usize];
        let mut len = length;
        assert_eq!(
            unsafe { AdsGetRecord(table, buf.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert_eq!(len, length);
        assert_eq!(buf[0], 0x04, "an active record");

        let mut len = 3;
        let rc = unsafe { AdsGetRecord(table, buf.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INSUFFICIENT_BUFFER);
        assert_eq!(len, length);
        assert_eq!(AdsGotoBOF(table), AE_SUCCESS);
        let mut len = length;
        let rc = unsafe { AdsGetRecord(table, buf.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_NO_CURRENT_RECORD);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    /// The checksums the original engine gives for `all_types`'s records,
    /// with and without `ADS_CRC_IGNOREMEMOPAGES`; record 1 and 5 have
    /// memo values.
    #[test]
    fn record_checksums() {
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let want = [
            (0x9772_b2ed, 0x0bbc_9b6e),
            (0x9000_6eb7, 0x9000_6eb7),
            (0x3e15_00b3, 0x3e15_00b3),
            (0xac7b_b829, 0xac7b_b829),
            (0x5d2c_3bd6, 0x645a_c310),
        ];
        AdsGotoTop(table);
        for (n, (all, ignoring_pages)) in want.into_iter().enumerate() {
            for (option, crc) in [(0, all), (1, all), (2, ignoring_pages), (3, ignoring_pages)] {
                let mut got = 0;
                assert_eq!(
                    unsafe { AdsGetRecordCRC(table, &mut got, option) },
                    AE_SUCCESS
                );
                assert_eq!(got, crc, "record {} option {option}", n + 1);
            }
            AdsSkip(table, 1);
        }
        let mut crc = 0x77;
        assert_eq!(
            unsafe { AdsGetRecordCRC(table, &mut crc, 0) },
            AE_NO_CURRENT_RECORD
        );
        assert_eq!(
            unsafe { AdsGetRecordCRC(table, &mut crc, 2) },
            CRC_NO_RECORD
        );
        assert_eq!(crc, 0x77);
        let null = std::ptr::null_mut();
        assert_eq!(
            unsafe { AdsGetRecordCRC(table, null, 0) },
            AE_INVALID_POINTER
        );
        assert_eq!(
            unsafe { AdsGetRecordCRC(connect, &mut crc, 0) },
            AE_INVALID_TABLE_HANDLE
        );
        crate::AdsDisconnect(connect);
    }

    /// As in the original engine: the empty record, as `AdsGetNullRecord`
    /// fills it in; a short buffer, an index order or a null buffer fail.
    #[test]
    fn clear_record_buffers() {
        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        let mut length = 0;
        assert_eq!(
            unsafe { crate::AdsGetRecordLength(table, &mut length) },
            AE_SUCCESS
        );
        let (mut cleared, mut null) = ([0xAA; 80], [0xAA; 80]);
        assert_eq!(
            unsafe { AdsClearRecordBuffer(table, cleared.as_mut_ptr(), 80) },
            AE_SUCCESS
        );
        assert_eq!(
            unsafe { AdsGetNullRecord(table, null.as_mut_ptr(), 80) },
            AE_SUCCESS
        );
        assert_eq!(cleared, null);
        assert_eq!(
            cleared[usize::try_from(length).unwrap()],
            0xAA,
            "past the record"
        );
        assert_eq!(&cleared[5..9], &[0, 0, 0, 0x80], "an Integer's NULL");
        let rc = unsafe { AdsClearRecordBuffer(table, cleared.as_mut_ptr(), 10) };
        assert_eq!(rc, AE_INSUFFICIENT_BUFFER);
        let mut order = 0;
        let name = c"BY_NAME".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { crate::AdsGetIndexHandle(table, name, &mut order) },
            AE_SUCCESS
        );
        let rc = unsafe { AdsClearRecordBuffer(order, cleared.as_mut_ptr(), 80) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);
        let rc = unsafe { AdsClearRecordBuffer(table, std::ptr::null_mut(), 80) };
        assert_eq!(rc, crate::AE_INVALID_POINTER);
        crate::AdsDisconnect(connect);
    }

    /// An exclusive handle keeps the slots its filtered record numbers pass
    /// by; they are what a shared handle, which counts every time, gives,
    /// through deletes, recalls, appends, `AdsShowDeleted` and AOFs, for
    /// DBF and ADT tables.
    #[test]
    fn kept_hidden_slots_number_as_counting_does() {
        let _settings = fixtures::settings_lock();
        let (source, _dir) = fixtures::scratch_from("hidden_slots", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let text = |s: &str| std::ffi::CString::new(s).unwrap();
        for (table_type, a, b) in [
            (crate::ADS_CDX, "x.dbf", "y.dbf"),
            (crate::ADS_ADT, "x.adt", "y.adt"),
        ] {
            let null = std::ptr::null_mut();
            let fields = text("N,N,4,0");
            for name in [a, b] {
                let (name, mut table) = (text(name), 0);
                let rc = unsafe {
                    crate::AdsCreateTable(
                        connect,
                        name.as_ptr().cast_mut().cast(),
                        null,
                        table_type,
                        1,
                        1,
                        1,
                        0,
                        fields.as_ptr().cast_mut().cast(),
                        &mut table,
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
                for _ in 0..12 {
                    crate::AdsAppendRecord(table);
                    crate::AdsWriteRecord(table);
                }
                crate::AdsCloseTable(table);
            }
            let open = |name: &str, options| {
                let (name, mut table) = (text(name), 0);
                let rc = unsafe {
                    crate::AdsOpenTable(
                        connect,
                        name.as_ptr().cast_mut().cast(),
                        null,
                        table_type,
                        1,
                        1,
                        1,
                        options,
                        &mut table,
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
                table
            };
            let (kept, counted) = (open(a, crate::ADS_EXCLUSIVE), open(b, crate::ADS_SHARED));
            let numbers = |table| {
                (1..=13u32)
                    .map(|record| {
                        crate::AdsGotoRecord(table, record);
                        let mut n = 0xffff_ffff;
                        let rc = unsafe { AdsGetRecordNum(table, ADS_RESPECTFILTERS, &mut n) };
                        (rc, n)
                    })
                    .collect::<Vec<_>>()
            };
            // Each step's return codes must agree too, so that the tables
            // stay alike.
            let both = |step: &dyn Fn(AdsHandle) -> Vec<Unsigned32>| {
                assert_eq!(step(kept), step(counted), "type {table_type}");
                assert_eq!(numbers(kept), numbers(counted), "type {table_type}");
            };
            both(&|_| vec![]);
            for record in [3, 7, 8] {
                both(&|t| {
                    crate::AdsGotoRecord(t, record);
                    vec![crate::AdsDeleteRecord(t), crate::AdsWriteRecord(t)]
                });
            }
            for show in [0, 1, 0] {
                crate::AdsShowDeleted(show);
                both(&|_| vec![]);
            }
            // A deleted ADT record recalled on a shared handle is refused
            // (5035), so the tables would no longer be alike: DBF only.
            if table_type == crate::ADS_CDX {
                both(&|t| {
                    crate::AdsGotoRecord(t, 7);
                    crate::AdsLockRecord(t, 7);
                    let codes = vec![crate::AdsRecallRecord(t), crate::AdsWriteRecord(t)];
                    crate::AdsUnlockRecord(t, 7);
                    codes
                });
            }
            both(&|t| vec![crate::AdsAppendRecord(t), crate::AdsWriteRecord(t)]);
            let aof = text("RECNO() <> 5");
            both(&|t| vec![unsafe { crate::AdsSetAOF(t, aof.as_ptr().cast_mut().cast(), 2) }]);
            both(&|t| vec![crate::AdsClearAOF(t)]);
            crate::AdsShowDeleted(1);
            crate::AdsCloseTable(kept);
            crate::AdsCloseTable(counted);
        }
        crate::AdsDisconnect(connect);
    }

    crate::fixtures::remote_twins!(
        clear_record_buffers,
        reads_whole_records,
        walks_records,
        record_checksums
    );
}
