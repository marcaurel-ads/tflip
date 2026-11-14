//! Index orders: opening and closing index files, finding orders by name,
//! number or expression, and what they are (expression, condition, key
//! type and length, unique, descending, custom, ...). Their keys are in
//! `keys.rs`, positions in them in `scope.rs`.

use std::ffi::CStr;

use tflip_engine::Error;
use tflip_engine::handle::HandleKind;
use tflip_engine::index::{IndexInfo, KeyType};
use tflip_engine::table::{FieldRef, find_field};

use crate::backend::api;
use crate::ffi::c_str;
use crate::field::write;
use crate::last_error::fail;
use crate::{
    ADS_DATE, ADS_FALSE, ADS_LOGICAL, ADS_NUMERIC, ADS_RAW, ADS_STRING, ADS_TRUE,
    AE_INVALID_OPTION, AE_INVALID_POINTER, AE_SUCCESS, AdsHandle, Unsigned8, Unsigned16,
    Unsigned32, Wchar, ansi, code, error_code, write_at, write_name, write_string, write_string_at,
};

/// A property of the index order behind `index`, as `ADS_TRUE` or
/// `ADS_FALSE`, for [`write`]. A handle that is not an index order fails
/// with `AE_INVALID_INDEX_HANDLE`.
fn flag(index: AdsHandle, f: impl FnOnce(&IndexInfo) -> bool) -> Result<Unsigned16, Unsigned32> {
    order_value(index, |order| if f(order) { ADS_TRUE } else { ADS_FALSE })
}

/// `value` of the order behind `index`, for [`write`]. A handle that is not
/// an index order fails with `AE_INVALID_INDEX_HANDLE`.
fn order_value<T>(index: AdsHandle, value: impl FnOnce(&IndexInfo) -> T) -> Result<T, Unsigned32> {
    api(index)
        .index_info(index)
        .map(|order| value(&order))
        .map_err(error_code)
}

/// Copies `handles` to `buf` of `*len` handles: what fits, with `*len` set
/// to their number, and `AE_INSUFFICIENT_BUFFER` if they did not all fit.
///
/// # Safety
///
/// `len` must be valid for reads and writes; `buf` must hold `*len` handles,
/// or may be null if `handles` is empty.
pub(crate) unsafe fn write_handles(
    handles: &[AdsHandle],
    buf: *mut AdsHandle,
    len: *mut Unsigned16,
) -> Unsigned32 {
    // SAFETY: as the caller promises.
    let fits = usize::from(unsafe { *len }).min(handles.len());
    if fits > 0 {
        // SAFETY: `buf` holds `*len` handles, at least `fits`.
        unsafe { std::ptr::copy_nonoverlapping(handles.as_ptr(), buf, fits) };
    }
    // SAFETY: as the caller promises.
    unsafe { *len = handles.len() as Unsigned16 };
    if fits < handles.len() {
        return fail(crate::AE_INSUFFICIENT_BUFFER);
    }
    AE_SUCCESS
}

/// `AdsIsIndexDescending`: whether an index order is descending.
///
/// # Safety
///
/// `descending` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsIndexDescending(
    index: AdsHandle,
    descending: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsIndexDescending");
    let value = flag(index, |order| order.descending);
    // SAFETY: `descending` is null or writable, as # Safety says.
    unsafe { write_at(value, descending, 2) }
}

/// `AdsIsIndexUnique`: whether an index order has unique keys. Orders
/// created `UNIQUE` or `CANDIDATE` are both unique (the index file does not
/// tell them apart for ADT tables).
///
/// # Safety
///
/// `unique` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsIndexUnique(
    index: AdsHandle,
    unique: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsIndexUnique");
    let value = flag(index, |order| order.unique);
    // SAFETY: `unique` is null or writable, as # Safety says.
    unsafe { write_at(value, unique, 2) }
}

/// What the local server fails `AdsOpenIndex` with for a file that does not
/// exist (not in `ace.h`).
const INDEX_FILE_NOT_FOUND: Unsigned32 = 7041;

/// `AdsOpenIndex`: opens the index file `name` of a table (in the table's
/// folder, `.adi` if it has no extension, or a full path) and writes the
/// handles of its orders to `indexes` (an array of `*len` handles), `*len`
/// set to their number. As in the original engine: a file that is open
/// already (the structural one among them) fails with
/// `AE_INDEX_ALREADY_OPEN` after writing its handles; one that does not
/// exist with 7041, writing nothing; an array that is too small with
/// `AE_INSUFFICIENT_BUFFER` and `*len` set to the number needed. Only ADI
/// files are read.
///
/// # Safety
///
/// `name` must be null or point to a NUL-terminated string; `indexes` must
/// be null or hold `*len` handles; `len` must be null or valid for reads and
/// writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsOpenIndex(
    table: AdsHandle,
    name: *mut Unsigned8,
    indexes: *mut AdsHandle,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsOpenIndex");
    let backend = match crate::filter::table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    if name.is_null() || indexes.is_null() || len.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: not null, so a NUL-terminated string, as # Safety says.
    let name = ansi::data_in(unsafe { CStr::from_ptr(name.cast()) }.to_bytes());
    let (orders, already) = match backend.open_index(table, name) {
        Ok(opened) => opened,
        Err(Error::FileNotFound) => return fail(INDEX_FILE_NOT_FOUND),
        Err(error) => return error_code(error),
    };
    // SAFETY: neither is null, so `len` is readable and writable and
    // `indexes` holds `*len` handles, as # Safety says.
    let rc = unsafe { write_handles(&orders, indexes, len) };
    if rc != AE_SUCCESS {
        return rc;
    }
    if already {
        return fail(crate::AE_INDEX_ALREADY_OPEN);
    }
    AE_SUCCESS
}

/// `AdsCloseIndex`: closes the index file holding an order, all its orders.
/// As in the original engine, the structural index fails with
/// `AE_AUTOOPEN_INDEX` (an IDX file named as a FoxPro table's `.cdx` is not
/// one), and a value that is no handle with `AE_INVALID_INDEX_HANDLE`,
/// recording `AE_INVALID_HANDLE` unless an error was left before
/// (`fixtures/errors/index_files.json`,
/// `fixtures/errors/moves_after_errors.json`).
#[unsafe(no_mangle)]
pub extern "system" fn AdsCloseIndex(index: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCloseIndex");
    // A value that is no handle records `AE_INVALID_HANDLE` as a move does,
    // but the call returns `AE_INVALID_INDEX_HANDLE`
    // (`fixtures/errors/moves_after_errors.json`).
    if matches!(api(index).handle_kind(index), Ok(None)) {
        crate::handle::no_handle(index);
        return crate::AE_INVALID_INDEX_HANDLE;
    }
    code(api(index).close_index(index))
}

/// `AdsCloseAllIndexes`: closes a table's index files but its structural
/// one, as in the original engine.
#[unsafe(no_mangle)]
pub extern "system" fn AdsCloseAllIndexes(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCloseAllIndexes");
    let backend = match crate::filter::table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    code(backend.close_all_indexes(table))
}

/// `AdsIsIndexCandidate`: whether an index order is a candidate key; as in
/// the original engine, every unique order of an ADT table is.
///
/// # Safety
///
/// `candidate` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsIndexCandidate(
    index: AdsHandle,
    candidate: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsIndexCandidate");
    let value = flag(index, |order| order.candidate);
    // SAFETY: `candidate` is null or writable, as # Safety says.
    unsafe { write(value, candidate) }
}

/// `AdsIsIndexFTS`: whether an index is a full-text (FTS) index.
///
/// # Safety
///
/// `fts` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsIndexFTS(index: AdsHandle, fts: *mut Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsIndexFTS");
    let value = flag(index, |order| order.flags & ADS_FTS_INDEX != 0);
    // SAFETY: `fts` is null or writable, as # Safety says.
    unsafe { write(value, fts) }
}

/// `AdsIsIndexUserDefined`: whether an index order is user-defined;
/// `ADS_FALSE` for every order of an ADT table, custom ones included, as
/// the original engine reports.
///
/// # Safety
///
/// `user_defined` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsIndexUserDefined(
    index: AdsHandle,
    user_defined: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsIndexUserDefined");
    let value = flag(index, |_| false);
    // SAFETY: `user_defined` is null or writable, as # Safety says.
    unsafe { write(value, user_defined) }
}

/// `AdsIsIndexNullable`: as in the original engine, the orders of ADT
/// tables fail with `AE_INVALID_INDEX_TYPE` (other handles with
/// `AE_INVALID_INDEX_HANDLE`).
///
/// # Safety
///
/// `nullable` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsIndexNullable(
    index: AdsHandle,
    nullable: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsIndexNullable");
    let _ = nullable;
    match api(index).index_info(index) {
        Ok(_) => fail(crate::AE_INVALID_INDEX_TYPE),
        Err(error) => error_code(error),
    }
}

/// The FTS flag of [`IndexInfo::flags`].
pub(crate) const ADS_FTS_INDEX: Unsigned32 = 0x20;

/// A table's index orders as the original engine numbers them (see
/// [`tflip_engine::index::numbered`]).
fn numbered_orders(table: AdsHandle) -> Result<Vec<AdsHandle>, Unsigned32> {
    let backend = crate::filter::table_of(table)?;
    backend.numbered_indexes(table).map_err(error_code)
}

/// `AdsGetIndexHandleByOrder`: a table's index order number `order`
/// (1-based), counting in the order they were opened, full-text indexes
/// (which `AdsGetNumIndexes` does not count) among them, as in the original
/// engine; others fail with `AE_INVALID_INDEX_ORDER`.
///
/// # Safety
///
/// `index` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetIndexHandleByOrder(
    table: AdsHandle,
    order: Unsigned16,
    index: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetIndexHandleByOrder");
    let orders = match numbered_orders(table) {
        Ok(orders) => orders,
        Err(code) => return code,
    };
    // The pointer is checked before the order number.
    if index.is_null() {
        return crate::last_error::null_param(3);
    }
    let Some(&handle) = usize::from(order)
        .checked_sub(1)
        .and_then(|i| orders.get(i))
    else {
        return fail(crate::AE_INVALID_INDEX_ORDER);
    };
    // SAFETY: `index` is not null, so writable, as # Safety says.
    unsafe { write(Ok(handle), index) }
}

/// `AdsGetIndexOrderByHandle`: the number of an index order among its
/// table's (see `AdsGetIndexHandleByOrder`). As in the original engine, a
/// handle that is not an index order fails with `AE_INVALID_TABLE_HANDLE`.
///
/// # Safety
///
/// `order` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetIndexOrderByHandle(
    index: AdsHandle,
    order: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetIndexOrderByHandle");
    let number = match api(index).index_info(index) {
        Ok(info) => numbered_orders(info.table)
            .map(|orders| orders.iter().position(|&h| h == index).map_or(0, |i| i + 1)),
        Err(Error::InvalidIndex) => Err(fail(crate::AE_INVALID_TABLE_HANDLE)),
        Err(error) => Err(error_code(error)),
    };
    // SAFETY: `order` is null or writable, as # Safety says.
    unsafe { write(number.map(|n| n as Unsigned16), order) }
}

/// `AdsGetIndexHandleByExpr`: the index order of a table whose key
/// expression is `expression`, compared regardless of case and of spaces
/// around it; the last of several. As in the original engine, only
/// ascending orders without a condition count (`descending` other than 0
/// fails with `AE_INVALID_OPTION`), none found fails with
/// `AE_INDEX_EXPR_NOT_FOUND` and sets `*index` to 0, and an empty
/// expression succeeds with 0.
///
/// # Safety
///
/// `expression` must be null or point to a NUL-terminated string; `index`
/// must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetIndexHandleByExpr(
    table: AdsHandle,
    expression: *mut Unsigned8,
    descending: Unsigned32,
    index: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetIndexHandleByExpr");
    // SAFETY: `expression` is null or NUL-terminated, `index` null or
    // writable, as # Safety says.
    let (expression, index) = unsafe { (c_str(expression), index.as_mut()) };
    let backend = match crate::filter::table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    if descending != 0 {
        return fail(AE_INVALID_OPTION);
    }
    let (Some(expression), Some(index)) = (expression, index) else {
        return fail(AE_INVALID_POINTER);
    };
    let wanted = match crate::filter::is_oem(table) {
        true => ansi::ansi_to_oem(&ansi::data_in(expression)),
        false => ansi::data_in(expression),
    };
    let wanted = wanted.trim();
    if wanted.is_empty() {
        *index = 0;
        return AE_SUCCESS;
    }
    let found = match backend.index_by_expression(table, wanted.to_owned()) {
        Ok(found) => found,
        Err(error) => return error_code(error),
    };
    *index = found.unwrap_or(0);
    match found {
        Some(_) => AE_SUCCESS,
        None => fail(crate::AE_INDEX_EXPR_NOT_FOUND),
    }
}

/// `AdsGetFTSIndexes`: the handles of a table's full-text indexes in
/// `indexes` (an array of `*len` handles), as `AdsGetAllIndexes` writes
/// its orders: if they do not all fit, the first `*len` and
/// `AE_INSUFFICIENT_BUFFER`, with `*len` set to the number needed.
///
/// # Safety
///
/// `indexes` must be null or hold `*len` handles; `len` must be null or
/// valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetFTSIndexes(
    table: AdsHandle,
    indexes: *mut AdsHandle,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetFTSIndexes");
    let backend = match crate::filter::table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    let found = match backend.full_text_indexes(table) {
        Ok(found) => found,
        Err(error) => return error_code(error),
    };
    if len.is_null() || (indexes.is_null() && !found.is_empty()) {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `len` is not null, so readable and writable, and `indexes`
    // holds `*len` handles or there are none to write, as # Safety says.
    unsafe { write_handles(&found, indexes, len) }
}

/// `AdsGetIndexFlags`: an index order's options, as the original engine
/// reports them (see [`tflip_engine::index::Index::flags`]); Delphi's
/// `TAdsTable` checks `ADS_UNIQUE` in them. Not declared in the SDK's
/// `ace.h`; the signature is `UNSIGNED32 AdsGetIndexFlags( ADSHANDLE hIndex,
/// UNSIGNED32 *pulFlags )` (`aceunpub.pas`). As in the original engine, a
/// handle that is not an index order fails with `AE_INVALID_INDEX_HANDLE`.
///
/// # Safety
///
/// `flags` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetIndexFlags(
    index: AdsHandle,
    flags: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetIndexFlags");
    let value = order_value(index, |order| order.flags);
    // SAFETY: `flags` is null or writable, as # Safety says.
    unsafe { write(value, flags) }
}

/// `AdsGetIndexFilename`: the name of an index order's file: without its
/// extension (`ADS_BASENAME`), with it (`ADS_BASENAMEANDEXT`), or its whole
/// path (`ADS_FULLPATHNAME`), in `name` of `*len` bytes. As in the original
/// engine, a name that does not fit reports the size it needs with the NUL,
/// and a handle that is not an index order fails with
/// `AE_INVALID_INDEX_HANDLE`. On a server the path is the server's.
///
/// # Safety
///
/// `name` must be null or hold `*len` bytes; `len` must be null or valid
/// for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetIndexFilename(
    index: AdsHandle,
    option: Unsigned16,
    name: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetIndexFilename");
    let order = match api(index).index_info(index) {
        Ok(order) => order,
        Err(error) => return error_code(error),
    };
    // The option is checked before the pointers.
    let Some(text) = crate::table::file_name(&order.path, option) else {
        return crate::last_error::fail_detail(crate::AE_INVALID_OPTION, "Parameter 2");
    };
    if let Some(code) = crate::ffi::null_buffer(name, len, 3) {
        return code;
    }
    // SAFETY: `name` and `len` are as # Safety says.
    unsafe { crate::write_cut(&ansi::data_out(text), name, len) }
}

/// `AdsGetIndexCollation`: the name of an index order's dynamic collation
/// (such as `GERMAN_VFP_CI_AS_1252`) in `collation` of `*len` bytes; empty
/// for an order with the table's static collation. As in the original
/// engine, a name that does not fit is cut, reporting the size it needs with
/// the NUL, and a handle that is not an index order fails with
/// `AE_INVALID_INDEX_HANDLE`.
///
/// # Safety
///
/// `collation` must be null or hold `*len` bytes; `len` must be null or
/// valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetIndexCollation(
    index: AdsHandle,
    collation: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetIndexCollation");
    let order = match api(index).index_info(index) {
        Ok(order) => order,
        Err(error) => return error_code(error),
    };
    // SAFETY: `collation` and `len` are as # Safety says.
    unsafe { crate::write_cut_at(&ansi::from_str_lossy(&order.collation), collation, len, 2) }
}

/// `AdsIsIndexCompound`: whether an index order is compound; every ADI order
/// is, as in the original engine.
///
/// # Safety
///
/// `compound` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsIndexCompound(
    index: AdsHandle,
    compound: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsIndexCompound");
    let value = flag(index, |order| order.flags & 0x2 != 0);
    // SAFETY: `compound` is null or writable, as # Safety says.
    unsafe { write_at(value, compound, 2) }
}

/// `AdsIsIndexCustom`: whether an index order is custom: its keys are added
/// explicitly (`AdsAddCustomKey`).
///
/// # Safety
///
/// `custom` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsIndexCustom(
    index: AdsHandle,
    custom: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsIndexCustom");
    let value = flag(index, |order| order.flags & 0x4 != 0);
    // SAFETY: `custom` is null or writable, as # Safety says.
    unsafe { write_at(value, custom, 2) }
}

/// `AdsIsIndexPrimaryKey`: whether an index order is its table's primary
/// key: a property of a table in a data dictionary, so the orders of free
/// tables are not.
///
/// # Safety
///
/// `primary_key` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsIndexPrimaryKey(
    index: AdsHandle,
    primary_key: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsIndexPrimaryKey");
    let value = flag(index, |info| info.primary_key);
    // SAFETY: `primary_key` is null or writable, as # Safety says.
    unsafe { write(value, primary_key) }
}

/// `AdsGetIndexHandle`: the handle of a table's open index order named
/// `order`, regardless of case. A table without such an order fails with
/// `AE_INVALID_INDEX_ORDER_NAME` and `*index` set to 0, an empty (or null)
/// name with the same code and `*index` left alone, as in the original
/// engine; on an SQL cursor an empty name is the natural order, handle 0. A
/// handle that is not a table fails with `AE_INVALID_TABLE_HANDLE`.
///
/// # Safety
///
/// `order` must be null or point to a NUL-terminated string; `index` must be
/// null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetIndexHandle(
    table: AdsHandle,
    order: *mut Unsigned8,
    index: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetIndexHandle");
    // SAFETY: `order` is null or NUL-terminated, `index` null or writable, as
    // # Safety says.
    let (order, index) = unsafe { (c_str(order), index.as_mut()) };
    let Some(index) = index else {
        return crate::last_error::null_param(3);
    };
    let name = order.map(ansi::data_in).unwrap_or_default();
    let backend = api(table);
    if name.is_empty() && backend.handle_kind(table) == Ok(Some(HandleKind::Cursor)) {
        *index = backend.index_by_name(table, String::new()).unwrap_or(0);
        return AE_SUCCESS;
    }
    match backend.index_by_name(table, name.clone()) {
        Ok(handle) => {
            *index = handle;
            AE_SUCCESS
        }
        Err(error @ tflip_engine::Error::InvalidIndexOrderName) if !name.is_empty() => {
            *index = 0;
            error_code(error)
        }
        Err(error) => error_code(error),
    }
}

/// `AdsGetKeyLength`: the length in bytes of an index order's keys.
///
/// # Safety
///
/// `length` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetKeyLength(
    index: AdsHandle,
    length: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetKeyLength");
    let value = order_value(index, |order| order.key_length);
    // SAFETY: `length` is null or writable, as # Safety says.
    unsafe { write_at(value, length, 2) }
}

/// `AdsGetKeyType`: the type of an index order's keys: `ADS_STRING`,
/// `ADS_NUMERIC`, `ADS_DATE`, `ADS_LOGICAL` or `ADS_RAW` (Raw, GUID, Time and
/// TimeStamp fields, and keys of several segments). An order on an expression
/// reports the type of its result (a numeric expression `ADS_NUMERIC`, a date
/// one `ADS_DATE`); see [`tflip_engine::index::Index::key_type`].
///
/// # Safety
///
/// `key_type` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetKeyType(
    index: AdsHandle,
    key_type: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetKeyType");
    let value = order_value(index, |order| match order.key_type {
        KeyType::Logical => ADS_LOGICAL,
        KeyType::Numeric => ADS_NUMERIC,
        KeyType::Date => ADS_DATE,
        KeyType::String => ADS_STRING,
        KeyType::Raw => ADS_RAW,
    });
    // SAFETY: `key_type` is null or writable, as # Safety says.
    unsafe { write_at(value, key_type, 2) }
}

/// Returns an expression of the order behind `index` through `buf` of `*len`
/// bytes, like [`write_string`].
///
/// # Safety
///
/// As for [`write_string`].
unsafe fn write_expression(
    index: AdsHandle,
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
    f: impl FnOnce(IndexInfo) -> String,
) -> Unsigned32 {
    match api(index).index_info(index) {
        Ok(_) if buf.is_null() || len.is_null() => {
            crate::last_error::null_param(if buf.is_null() { 2 } else { 3 })
        }
        Ok(info) => {
            // An OEM table's index holds its literals in the OEM code page.
            let oem = crate::filter::is_oem(info.table);
            let text = f(info);
            let bytes = match oem {
                true => ansi::oem_to_ansi(&text),
                false => ansi::data_out(&text),
            };
            // SAFETY: as the caller promises.
            unsafe { write_string(&bytes, buf, len) }
        }
        Err(error) => error_code(error),
    }
}

/// `AdsGetIndexExpr`: the key expression of an index order, with fields by
/// the names the table defines (the index file stores field numbers), in
/// `expr` of `*len` bytes (NUL-terminated); `*len` is set to its length.
///
/// # Safety
///
/// `expr` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetIndexExpr(
    index: AdsHandle,
    expr: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetIndexExpr");
    // SAFETY: `expr` and `len` are as # Safety says.
    unsafe { write_expression(index, expr, len, |order| order.expression) }
}

/// `AdsGetIndexCondition`: the `FOR` condition of an index order, with
/// fields by the names the table defines, in `expr` of `*len` bytes
/// (NUL-terminated); `*len` is set to its length, 0 if the order has none.
///
/// # Safety
///
/// `expr` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetIndexCondition(
    index: AdsHandle,
    expr: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetIndexCondition");
    // SAFETY: `expr` and `len` are as # Safety says.
    unsafe { write_expression(index, expr, len, |order| order.condition) }
}

/// `AdsGetIndexName`: the name of an index order (its tag name, in upper
/// case), in `name` of `*len` bytes (NUL-terminated); `*len` is set to its
/// length. A buffer that is too small fails with `AE_INSUFFICIENT_BUFFER`
/// and `*len` set to the length needed. A handle that is not an index order
/// fails with `AE_INVALID_INDEX_HANDLE`.
///
/// # Safety
///
/// `name` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetIndexName(
    index: AdsHandle,
    name: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetIndexName");
    match api(index).index_info(index) {
        // SAFETY: `name` and `len` are as # Safety says.
        Ok(order) => unsafe { write_string_at(&ansi::data_out(&order.name), name, len, 2) },
        Err(error) => error_code(error),
    }
}

/// `AdsGetNumSegments`: the number of segments of an index order: the
/// distinct fields its key expression references (`id;UPPER(name);born` has
/// three, `RECNO()` none), as in the original engine.
///
/// Unpublished: not declared in the SDK's `ace.h`; the signature is
/// `UNSIGNED32 AdsGetNumSegments( ADSHANDLE hTag, UNSIGNED16 *pusSegments )`.
/// A handle that is not an index order fails with `AE_INVALID_INDEX_HANDLE`.
///
/// # Safety
///
/// `segments` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetNumSegments(
    tag: AdsHandle,
    segments: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetNumSegments");
    let value = order_value(tag, |order| order.segments.len() as Unsigned16);
    // SAFETY: `segments` is null or writable, as # Safety says.
    unsafe { write(value, segments) }
}

/// `AdsGetSegmentFieldname`: the name of the field of segment `segment`
/// (0-based) of an index order, as the table defines it, in `name` of `*len`
/// bytes (NUL-terminated); `*len` is set to its length. A segment is a
/// distinct field the key expression references (see `AdsGetNumSegments`):
/// segment 0 of `UPPER(name)` is `name`.
///
/// Unpublished: not declared in the SDK's `ace.h`; the signature is
/// `UNSIGNED32 AdsGetSegmentFieldname( ADSHANDLE hTag, UNSIGNED16
/// usSegmentNum, UNSIGNED8 *pucFieldname, UNSIGNED16 *pusFldnameLen )`.
///
/// As in the original engine: a segment past the last fails with
/// `AE_INVALID_FIELDNUMBER` and leaves `*len` alone; a buffer that is too
/// small fails with `AE_INSUFFICIENT_BUFFER` and `*len` set to the length
/// needed including the NUL; a handle that is not an index order fails with
/// `AE_INVALID_INDEX_HANDLE`.
///
/// # Safety
///
/// `name` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetSegmentFieldname(
    tag: AdsHandle,
    segment: Unsigned16,
    name: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetSegmentFieldname");
    let backend = api(tag);
    let field = backend.index_info(tag).and_then(|order| {
        let number = *order
            .segments
            .get(usize::from(segment))
            .ok_or(Error::InvalidFieldNumber)?;
        let table = backend.table_info(order.table)?;
        Ok(
            find_field(&table.structure.fields, FieldRef::Number(number))?
                .1
                .name
                .clone(),
        )
    });
    match field {
        // SAFETY: `name` and `len` are as # Safety says.
        Ok(field) => unsafe { write_name(&ansi::data_out(&field), name, len) },
        Err(error) => error_code(error),
    }
}

/// `AdsGetIndexPageSize`: the page size of an index order's file, in bytes
/// (512 to 8,192).
///
/// Not declared in the SDK's `ace.h`; the signature is
/// `UNSIGNED32 AdsGetIndexPageSize( ADSHANDLE hIndex, UNSIGNED32 *pulPageSize )`.
/// A handle that is not an index order fails with `AE_INVALID_INDEX_HANDLE`.
///
/// # Safety
///
/// `page_size` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetIndexPageSize(
    index: AdsHandle,
    page_size: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetIndexPageSize");
    let value = order_value(index, |order| order.page_size);
    // SAFETY: `page_size` is null or writable, as # Safety says.
    unsafe { write_at(value, page_size, 2) }
}

/// `AdsRegisterUDF`: registers a user-defined function of type
/// `ADS_INDEX_UDF` (1) for a table, cursor or index order. tflip keeps no
/// such functions and never calls one. As in the original engine: handle 0
/// succeeds; another kind of handle fails with `AE_INVALID_HANDLE`; another
/// type fails with `AE_INVALID_OPTION` and a null function with
/// `AE_INVALID_POINTER`, neither of them recorded as the last error (nor
/// does success clear it).
#[unsafe(no_mangle)]
pub extern "system" fn AdsRegisterUDF(
    obj: AdsHandle,
    udf_type: Unsigned16,
    udf: Option<unsafe extern "system" fn() -> Unsigned32>,
) -> Unsigned32 {
    // Not a call that clears the last error: the original keeps it, but
    // for a handle it refuses.
    if obj == 0 {
        return AE_SUCCESS;
    }
    let kind = crate::backend::api(obj).handle_kind(obj);
    let usable = matches!(
        kind,
        Ok(Some(
            HandleKind::Table | HandleKind::Cursor | HandleKind::Index
        ))
    );
    match () {
        _ if !usable => crate::last_error::fail_text(
            crate::AE_INVALID_HANDLE,
            "Error 5018:  The handle given was not recognized by Advantage. Verify specified \
             handle is open/active.  Must be a table, cursor or index order handle."
                .to_owned(),
        ),
        _ if udf_type != ADS_INDEX_UDF => AE_INVALID_OPTION,
        _ if udf.is_none() => AE_INVALID_POINTER,
        _ => AE_SUCCESS,
    }
}

/// `ADS_INDEX_UDF`, the one type of user-defined function.
const ADS_INDEX_UDF: Unsigned16 = 1;

/// The five strings `AdsGetFTSIndexInfo` reports, in the order they are packed
/// into the caller's buffer: the field name, then the delimiter, noise, drop and
/// conditional character sets.
fn fts_segments(fts: &tflip_engine::index::FtsInfo) -> [&str; 5] {
    [
        &fts.field,
        &fts.delimiters,
        &fts.noise,
        &fts.drop,
        &fts.conditional,
    ]
}

/// Resolves `index` to its full-text parsing options, failing as the original
/// engine does: `AE_INVALID_INDEX_HANDLE` for a handle that is not an index
/// order, `AE_INVALID_INDEX_TYPE` for one that is not full-text, and
/// `AE_INVALID_POINTER` when `output` or `buf_len` is null.
fn fts_info(
    index: AdsHandle,
    output_null: bool,
    buf_len_null: bool,
) -> Result<tflip_engine::index::FtsInfo, Unsigned32> {
    let info = api(index).index_info(index).map_err(error_code)?;
    let fts = info.fts.ok_or(crate::AE_INVALID_INDEX_TYPE)?;
    if output_null || buf_len_null {
        return Err(AE_INVALID_POINTER);
    }
    Ok(fts)
}

/// The pointers `AdsGetFTSIndexInfo` and `AdsGetFTSIndexInfoW` set besides
/// their buffer: into it, for the strings of [`fts_segments`], null for the
/// reserved ones, and the numbers.
struct FtsOut<T> {
    strings: [*mut *mut T; 5],
    reserved: [*mut *mut T; 2],
    min_word_len: *mut Unsigned32,
    max_word_len: *mut Unsigned32,
    options: *mut Unsigned32,
}

/// Packs `units`, the strings of [`fts_segments`] starting at `offsets`, into
/// `output` of `*buf_len` units, `*buf_len` set to the units needed, and sets
/// the pointers of `out`; `AE_INSUFFICIENT_BUFFER`, setting nothing else, if
/// they do not fit.
///
/// # Safety
///
/// `buf_len` must be valid for reads and writes, `output` must hold
/// `*buf_len` units, and each pointer of `out` must be null or valid for
/// writes.
unsafe fn write_fts_info<T: Copy>(
    units: &[T],
    offsets: [usize; 5],
    fts: &tflip_engine::index::FtsInfo,
    output: *mut T,
    buf_len: *mut Unsigned32,
    out: FtsOut<T>,
) -> Unsigned32 {
    let needed = units.len() as Unsigned32;
    // SAFETY: as the caller promises.
    let capacity = unsafe { std::mem::replace(&mut *buf_len, needed) };
    if needed > capacity {
        return fail(crate::AE_INSUFFICIENT_BUFFER);
    }
    // SAFETY: `output` holds `capacity` units, at least `needed`.
    unsafe { std::ptr::copy_nonoverlapping(units.as_ptr(), output, units.len()) };
    for (p, offset) in out.strings.into_iter().zip(offsets) {
        // SAFETY: null or writable, as the caller promises.
        if let Some(p) = unsafe { p.as_mut() } {
            *p = output.wrapping_add(offset);
        }
    }
    for reserved in out.reserved {
        // SAFETY: null or writable, as the caller promises.
        if let Some(reserved) = unsafe { reserved.as_mut() } {
            *reserved = std::ptr::null_mut();
        }
    }
    for (p, value) in [
        (out.min_word_len, fts.min_word),
        (out.max_word_len, fts.max_word),
        (out.options, fts.options),
    ] {
        // SAFETY: null or writable, as the caller promises.
        if let Some(p) = unsafe { p.as_mut() } {
            *p = value;
        }
    }
    AE_SUCCESS
}

/// `AdsGetFTSIndexInfo`: the parsing options of a full-text index order. Packs
/// the field name and the delimiter, noise, drop and conditional character sets
/// into `output` (each NUL-terminated, in that order) and points `field`,
/// `delimiters`, `noise_words`, `drop_chars` and `conditional_chars` into it;
/// sets `min_word_len`, `max_word_len` and `options`, and the reserved pointers
/// to null. `buf_len` is the buffer's size in bytes going in, and the bytes
/// needed coming out; a buffer too small is `AE_INSUFFICIENT_BUFFER`. (Black-box
/// `tools/ace/fts/ftsinfo.c`.)
///
/// # Safety
///
/// `output` must hold `*buf_len` bytes; the pointer out-parameters must be null
/// or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetFTSIndexInfo(
    index: AdsHandle,
    output: *mut Unsigned8,
    buf_len: *mut Unsigned32,
    field: *mut *mut Unsigned8,
    min_word_len: *mut Unsigned32,
    max_word_len: *mut Unsigned32,
    delimiters: *mut *mut Unsigned8,
    noise_words: *mut *mut Unsigned8,
    drop_chars: *mut *mut Unsigned8,
    conditional_chars: *mut *mut Unsigned8,
    reserved1: *mut *mut Unsigned8,
    reserved2: *mut *mut Unsigned8,
    options: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetFTSIndexInfo");
    let fts = match fts_info(index, output.is_null(), buf_len.is_null()) {
        Ok(fts) => fts,
        Err(code) => return fail(code),
    };
    // Each segment, encoded in the ANSI code page and NUL-terminated, in order.
    let mut bytes = Vec::new();
    let mut offsets = [0usize; 5];
    for (i, segment) in fts_segments(&fts).iter().enumerate() {
        offsets[i] = bytes.len();
        bytes.extend(ansi::from_str_lossy(segment));
        bytes.push(0);
    }
    let out = FtsOut {
        strings: [
            field,
            delimiters,
            noise_words,
            drop_chars,
            conditional_chars,
        ],
        reserved: [reserved1, reserved2],
        min_word_len,
        max_word_len,
        options,
    };
    // SAFETY: neither `output` nor `buf_len` is null (`fts_info`), so they
    // and the other pointers are as # Safety says.
    unsafe { write_fts_info(&bytes, offsets, &fts, output, buf_len, out) }
}

/// `AdsGetFTSIndexInfoW`: [`AdsGetFTSIndexInfo`] for a UTF-16 client. The strings
/// are packed as wide, NUL-terminated runs, and `buf_len` counts characters.
///
/// # Safety
///
/// `output` must hold `*buf_len` characters; the pointer out-parameters must be
/// null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetFTSIndexInfoW(
    index: AdsHandle,
    output: *mut Wchar,
    buf_len: *mut Unsigned32,
    field: *mut *mut Wchar,
    min_word_len: *mut Unsigned32,
    max_word_len: *mut Unsigned32,
    delimiters: *mut *mut Wchar,
    noise_words: *mut *mut Wchar,
    drop_chars: *mut *mut Wchar,
    conditional_chars: *mut *mut Wchar,
    reserved1: *mut *mut Wchar,
    reserved2: *mut *mut Wchar,
    options: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetFTSIndexInfoW");
    let fts = match fts_info(index, output.is_null(), buf_len.is_null()) {
        Ok(fts) => fts,
        Err(code) => return fail(code),
    };
    // Each segment as UTF-16 with a wide NUL, in order; offsets in characters.
    let mut units: Vec<Wchar> = Vec::new();
    let mut offsets = [0usize; 5];
    for (i, segment) in fts_segments(&fts).iter().enumerate() {
        offsets[i] = units.len();
        units.extend(segment.encode_utf16());
        units.push(0);
    }
    let out = FtsOut {
        strings: [
            field,
            delimiters,
            noise_words,
            drop_chars,
            conditional_chars,
        ],
        reserved: [reserved1, reserved2],
        min_word_len,
        max_word_len,
        options,
    };
    // SAFETY: neither `output` nor `buf_len` is null (`fts_info`), so they
    // and the other pointers are as # Safety says.
    unsafe { write_fts_info(&units, offsets, &fts, output, buf_len, out) }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value as Json, json};

    use super::*;
    use crate::{
        ADS_ADT, ADS_INDEX_ORDER, AE_INSUFFICIENT_BUFFER, AE_INVALID_FIELDNUMBER,
        AE_INVALID_INDEX_HANDLE, AE_INVALID_INDEX_ORDER_NAME, AE_INVALID_INDEX_TYPE,
        AE_INVALID_TABLE_HANDLE, AdsDisconnect, AdsGetAllIndexes, AdsGetHandleType,
        AdsGetNumIndexes, AdsOpenTable90, fixtures,
    };
    use crate::{AdsExtractKey, AdsGetKeyCount, AdsGetKeyNum, AdsSeek};

    /// A text getter such as `AdsGetIndexExpr`.
    type Getter =
        unsafe extern "system" fn(AdsHandle, *mut Unsigned8, *mut Unsigned16) -> Unsigned32;

    /// Opens a table in a scratch copy of the full-text fixtures
    /// (`fixtures/sql/fts`), so the committed fixtures are not touched, and
    /// returns `(connection, table)`.
    fn open_fts(scratch: &str, table: &std::ffi::CStr) -> (AdsHandle, AdsHandle) {
        let (source, _dir) = fixtures::scratch_from(scratch, &fixtures::dir("../sql/fts"));
        let connect = fixtures::connect_to(&source);
        let mut handle = 0;
        let null = std::ptr::null_mut();
        let rc = unsafe {
            AdsOpenTable90(
                connect,
                table.as_ptr().cast_mut().cast(),
                null,
                ADS_ADT,
                1,
                1,
                2,
                0,
                c"ansi".as_ptr().cast_mut().cast(),
                &mut handle,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        (connect, handle)
    }

    /// A NUL-terminated string at `p`.
    unsafe fn cstr(p: *mut Unsigned8) -> String {
        unsafe {
            std::ffi::CStr::from_ptr(p.cast())
                .to_string_lossy()
                .into_owned()
        }
    }

    #[test]
    fn fts_index_info_matches_the_engine() {
        let (connect, table) = open_fts("ace-fts-info", c"opts.adt");
        let mut index = 0;
        // Order 1 is C_FTS, on the character field `c`: case-sensitive,
        // keep-score, a 2..5 word window (`tools/ace/fts/ftsinfo.c`).
        assert_eq!(
            unsafe { AdsGetIndexHandleByOrder(table, 1, &mut index) },
            AE_SUCCESS
        );

        let mut out = [0u8; 4096];
        let mut len = out.len() as Unsigned32;
        let (mut field, mut del, mut noise, mut drop, mut cond, mut r1, mut r2) = (
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0xAAu8 as *mut Unsigned8,
            0xAAu8 as *mut Unsigned8,
        );
        let (mut min, mut max, mut options) = (0, 0, 0);
        let rc = unsafe {
            AdsGetFTSIndexInfo(
                index,
                out.as_mut_ptr(),
                &mut len,
                &mut field,
                &mut min,
                &mut max,
                &mut del,
                &mut noise,
                &mut drop,
                &mut cond,
                &mut r1,
                &mut r2,
                &mut options,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!((min, max, options), (2, 5, 0x1a0));
        assert_eq!(unsafe { cstr(field) }, "c");
        assert_eq!(unsafe { cstr(del) }, "\x08\t\n\x0b\x0c\r ");
        assert_eq!(unsafe { cstr(drop) }, "'\"`");
        assert_eq!(unsafe { cstr(cond) }, ",.?!;:@#$%^&()-_");
        let noise = unsafe { cstr(noise) };
        assert!(noise.starts_with("about after all"), "{noise}");
        assert!(noise.split(' ').any(|w| w == "an"), "a bigger noise list");
        // The field is first, then the character sets; the reserved pointers are
        // cleared, and buf_len is the bytes used.
        assert_eq!(field, out.as_mut_ptr());
        assert!(r1.is_null() && r2.is_null());
        assert_eq!(len, needed_bytes(index));

        // A buffer too small reports the size needed and nothing more.
        let (mut small, mut slen) = ([0u8; 8], 8);
        assert_eq!(
            info_rc(index, small.as_mut_ptr(), &mut slen),
            AE_INSUFFICIENT_BUFFER
        );
        assert_eq!(slen, len);

        AdsDisconnect(connect);
    }

    /// Calls `AdsGetFTSIndexInfo` on `index` with a `len`-byte `out`, discarding
    /// the out-pointers, and returns the result code.
    fn info_rc(index: AdsHandle, out: *mut Unsigned8, len: *mut Unsigned32) -> Unsigned32 {
        let (mut f, mut d, mut n, mut dr, mut c, mut r1, mut r2) = (
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        let (mut mn, mut mx, mut op) = (0, 0, 0);
        unsafe {
            AdsGetFTSIndexInfo(
                index, out, len, &mut f, &mut mn, &mut mx, &mut d, &mut n, &mut dr, &mut c,
                &mut r1, &mut r2, &mut op,
            )
        }
    }

    /// The bytes `AdsGetFTSIndexInfo` needs for `index`, read with a large buffer.
    fn needed_bytes(index: AdsHandle) -> Unsigned32 {
        let mut out = [0u8; 4096];
        let mut len = out.len() as Unsigned32;
        assert_eq!(info_rc(index, out.as_mut_ptr(), &mut len), AE_SUCCESS);
        len
    }

    #[test]
    fn fts_index_info_errors() {
        // A handle that is not a full-text order is AE_INVALID_INDEX_TYPE.
        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        let mut index = 0;
        assert_eq!(
            unsafe { AdsGetIndexHandleByOrder(table, 1, &mut index) },
            AE_SUCCESS
        );
        let mut out = [0u8; 256];
        let mut len = out.len() as Unsigned32;
        assert_eq!(
            info_rc(index, out.as_mut_ptr(), &mut len),
            AE_INVALID_INDEX_TYPE
        );
        // A table handle is not an index order.
        assert_eq!(
            info_rc(table, out.as_mut_ptr(), &mut len),
            AE_INVALID_INDEX_HANDLE
        );
        AdsDisconnect(connect);

        // A null output or length on a valid full-text order is AE_INVALID_POINTER.
        let (connect, table) = open_fts("ace-fts-err", c"opts.adt");
        assert_eq!(
            unsafe { AdsGetIndexHandleByOrder(table, 1, &mut index) },
            AE_SUCCESS
        );
        assert_eq!(
            info_rc(index, std::ptr::null_mut(), &mut len),
            AE_INVALID_POINTER
        );
        assert_eq!(
            info_rc(index, out.as_mut_ptr(), std::ptr::null_mut()),
            AE_INVALID_POINTER
        );
        AdsDisconnect(connect);
    }

    #[test]
    fn fts_index_info_w_matches_the_engine() {
        let (connect, table) = open_fts("ace-fts-w", c"opts.adt");
        let mut index = 0;
        assert_eq!(
            unsafe { AdsGetIndexHandleByOrder(table, 1, &mut index) },
            AE_SUCCESS
        );
        let mut out = [0u16; 4096];
        let mut len = out.len() as Unsigned32;
        let (mut field, mut del, mut noise, mut drop, mut cond, mut r1, mut r2) = (
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        let (mut min, mut max, mut options) = (0, 0, 0);
        let rc = unsafe {
            AdsGetFTSIndexInfoW(
                index,
                out.as_mut_ptr(),
                &mut len,
                &mut field,
                &mut min,
                &mut max,
                &mut del,
                &mut noise,
                &mut drop,
                &mut cond,
                &mut r1,
                &mut r2,
                &mut options,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!((min, max, options), (2, 5, 0x1a0));
        let wide = |p: *mut Wchar| -> String {
            let mut s = String::new();
            let mut i = 0;
            unsafe {
                while *p.add(i) != 0 {
                    s.push(char::from_u32(u32::from(*p.add(i))).unwrap());
                    i += 1;
                }
            }
            s
        };
        assert_eq!(wide(field), "c");
        assert_eq!(wide(drop), "'\"`");
        assert_eq!(wide(cond), ",.?!;:@#$%^&()-_");
        // buf_len counts characters.
        assert_eq!(field, out.as_mut_ptr());
        AdsDisconnect(connect);
    }

    /// The handles of the index orders of `table`.
    fn all_indexes(table: AdsHandle) -> Vec<AdsHandle> {
        let mut indexes = [0; 8];
        let mut len = indexes.len() as u16;
        let rc = unsafe { AdsGetAllIndexes(table, indexes.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_SUCCESS);
        indexes[..usize::from(len)].to_vec()
    }

    #[test]
    fn orders_of_a_table() {
        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        let mut count = 0;
        assert_eq!(unsafe { AdsGetNumIndexes(table, &mut count) }, AE_SUCCESS);
        assert_eq!(count, 5);
        let indexes = all_indexes(table);
        assert_eq!(indexes.len(), 5);
        let mut kind = 0;
        assert_eq!(
            unsafe { AdsGetHandleType(indexes[0], &mut kind) },
            AE_SUCCESS
        );
        assert_eq!(kind, ADS_INDEX_ORDER);

        // A short array gets the handles that fit.
        let mut short = [0; 2];
        let mut len = short.len() as u16;
        let rc = unsafe { AdsGetAllIndexes(table, short.as_mut_ptr(), &mut len) };
        assert_eq!(
            (rc, len, short),
            (AE_INSUFFICIENT_BUFFER, 5, [indexes[0], indexes[1]])
        );
        let mut len = 0;
        let rc = unsafe { AdsGetAllIndexes(table, short.as_mut_ptr(), &mut len) };
        assert_eq!((rc, len), (AE_INSUFFICIENT_BUFFER, 5));

        let names: Vec<String> = indexes
            .iter()
            .map(|&index| {
                let mut name = [0u8; 16];
                let mut len = name.len() as u16;
                let rc = unsafe { AdsGetIndexName(index, name.as_mut_ptr(), &mut len) };
                assert_eq!(rc, AE_SUCCESS);
                String::from_utf8(name[..usize::from(len)].to_vec()).unwrap()
            })
            .collect();
        assert_eq!(
            names,
            [
                "PK_INDEXED",
                "BY_NAME",
                "BY_AMOUNT_DESC",
                "BY_BORN_NAME",
                "BY_CINAME"
            ]
        );
        let mut name = [0u8; 4];
        let mut len = name.len() as u16;
        let rc = unsafe { AdsGetIndexName(indexes[0], name.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INSUFFICIENT_BUFFER);
        let rc = unsafe { AdsGetIndexName(table, name.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INVALID_INDEX_HANDLE);

        let segments = |index| {
            let mut count = 0;
            assert_eq!(unsafe { AdsGetNumSegments(index, &mut count) }, AE_SUCCESS);
            count
        };
        assert_eq!(segments(indexes[0]), 1, "id");
        assert_eq!(segments(indexes[3]), 2, "born;name");
        let rc = unsafe { AdsGetNumSegments(table, &mut count) };
        assert_eq!(rc, AE_INVALID_INDEX_HANDLE);
        let rc = unsafe { AdsGetNumSegments(indexes[0], std::ptr::null_mut()) };
        assert_eq!(rc, AE_INVALID_POINTER);

        // BY_BORN_NAME is `born;name`.
        let field = |segment, size: u16| {
            let mut name = [0u8; 16];
            let mut len = size;
            let rc =
                unsafe { AdsGetSegmentFieldname(indexes[3], segment, name.as_mut_ptr(), &mut len) };
            (
                rc,
                len,
                String::from_utf8_lossy(&name[..usize::from(len).min(16)]).into_owned(),
            )
        };
        assert_eq!(field(0, 16), (AE_SUCCESS, 4, "born".to_owned()));
        assert_eq!(field(1, 16), (AE_SUCCESS, 4, "name".to_owned()));
        assert_eq!(field(2, 16).0, AE_INVALID_FIELDNUMBER);
        assert_eq!(field(2, 16).1, 16, "the length is left alone");
        assert_eq!(field(0, 4).0, AE_INSUFFICIENT_BUFFER, "the NUL counts");
        assert_eq!(field(0, 4).1, 5);
        let mut name = [0u8; 16];
        let mut len = name.len() as u16;
        let rc = unsafe { AdsGetSegmentFieldname(table, 0, name.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INVALID_INDEX_HANDLE);

        let mut flag = 0xffff;
        assert_eq!(
            unsafe { AdsIsIndexDescending(indexes[2], &mut flag) },
            AE_SUCCESS
        );
        assert_eq!(flag, ADS_TRUE, "BY_AMOUNT_DESC");
        assert_eq!(
            unsafe { AdsIsIndexDescending(indexes[1], &mut flag) },
            AE_SUCCESS
        );
        assert_eq!(flag, ADS_FALSE, "BY_NAME");
        assert_eq!(
            unsafe { AdsIsIndexDescending(table, &mut flag) },
            AE_INVALID_INDEX_HANDLE
        );
        let rc = unsafe { AdsIsIndexDescending(indexes[2], std::ptr::null_mut()) };
        assert_eq!(rc, AE_INVALID_POINTER);

        assert_eq!(
            unsafe { AdsIsIndexUnique(indexes[4], &mut flag) },
            AE_SUCCESS
        );
        assert_eq!(flag, ADS_TRUE, "BY_CINAME");
        assert_eq!(
            unsafe { AdsIsIndexUnique(indexes[1], &mut flag) },
            AE_SUCCESS
        );
        assert_eq!(flag, ADS_FALSE, "BY_NAME");
        assert_eq!(
            unsafe { AdsIsIndexUnique(table, &mut flag) },
            AE_INVALID_INDEX_HANDLE
        );

        assert_eq!(
            unsafe { AdsIsIndexPrimaryKey(indexes[0], &mut flag) },
            AE_SUCCESS
        );
        assert_eq!(flag, ADS_FALSE, "a free table has no primary key");
        assert_eq!(
            unsafe { AdsIsIndexPrimaryKey(table, &mut flag) },
            AE_INVALID_INDEX_HANDLE
        );

        let mut handle = 0;
        let by_name = c"by_name".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { AdsGetIndexHandle(table, by_name, &mut handle) },
            AE_SUCCESS
        );
        assert_eq!(handle, indexes[1]);
        let nope = c"nope".as_ptr().cast_mut().cast();
        let rc = unsafe { AdsGetIndexHandle(table, nope, &mut handle) };
        assert_eq!((rc, handle), (AE_INVALID_INDEX_ORDER_NAME, 0));
        let mut handle = 0xdead;
        let empty = c"".as_ptr().cast_mut().cast();
        let rc = unsafe { AdsGetIndexHandle(table, empty, &mut handle) };
        assert_eq!(
            (rc, handle),
            (AE_INVALID_INDEX_ORDER_NAME, 0xdead),
            "left alone"
        );
        let rc = unsafe { AdsGetIndexHandle(connect, by_name, &mut handle) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);

        // Moves with an order's handle go in key order; with the table's in
        // record order. BY_AMOUNT_DESC holds records 5, 1, 2, 3.
        let record_num = || {
            let mut record = 0;
            let option = crate::ADS_IGNOREFILTERS;
            assert_eq!(
                unsafe { crate::AdsGetRecordNum(table, option, &mut record) },
                AE_SUCCESS
            );
            record
        };
        assert_eq!(crate::AdsGotoTop(indexes[2]), AE_SUCCESS);
        assert_eq!(record_num(), 5);
        assert_eq!(crate::AdsSkip(indexes[2], 1), AE_SUCCESS);
        assert_eq!(record_num(), 1);
        assert_eq!(crate::AdsGotoBottom(indexes[2]), AE_SUCCESS);
        assert_eq!(record_num(), 3);
        let key_num = |index| {
            let mut key = 0xffff_ffff;
            assert_eq!(unsafe { AdsGetKeyNum(index, 1, &mut key) }, AE_SUCCESS);
            key
        };
        assert_eq!(key_num(indexes[2]), 4);
        assert_eq!(crate::AdsGotoTop(table), AE_SUCCESS);
        assert_eq!(record_num(), 1);
        assert_eq!(
            key_num(indexes[2]),
            2,
            "record 1 is second in BY_AMOUNT_DESC"
        );
        assert_eq!(crate::AdsSkip(table, -1), AE_SUCCESS);
        assert_eq!(key_num(indexes[2]), 0, "BOF");
        assert_eq!(crate::AdsGotoBottom(table), AE_SUCCESS);
        assert_eq!(crate::AdsSkip(table, 1), AE_SUCCESS);
        assert_eq!(key_num(indexes[2]), 5, "EOF: one past the last key");
        let mut key = 0;
        assert_eq!(
            unsafe { AdsGetKeyNum(table, 1, &mut key) },
            AE_INVALID_INDEX_HANDLE
        );
        let mut count = 0;
        assert_eq!(
            unsafe { AdsGetKeyCount(indexes[2], 1, &mut count) },
            AE_SUCCESS
        );
        assert_eq!(count, 4);
        assert_eq!(
            unsafe { AdsGetKeyCount(table, 1, &mut count) },
            AE_INVALID_INDEX_HANDLE
        );

        let text = |f: Getter, index| {
            let mut buf = [0u8; 32];
            let mut len = buf.len() as u16;
            assert_eq!(unsafe { f(index, buf.as_mut_ptr(), &mut len) }, AE_SUCCESS);
            String::from_utf8(buf[..usize::from(len)].to_vec()).unwrap()
        };
        assert_eq!(text(AdsGetIndexExpr, indexes[3]), "born;name");
        assert_eq!(text(AdsGetIndexCondition, indexes[3]), "");
        let mut buf = [0u8; 4];
        let mut len = buf.len() as u16;
        let rc = unsafe { AdsGetIndexExpr(table, buf.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INVALID_INDEX_HANDLE);

        let mut size = 0;
        assert_eq!(
            unsafe { AdsGetIndexPageSize(indexes[0], &mut size) },
            AE_SUCCESS
        );
        assert_eq!(size, 512);
        let rc = unsafe { AdsGetIndexPageSize(table, &mut size) };
        assert_eq!(rc, AE_INVALID_INDEX_HANDLE);
        let rc = unsafe { AdsGetIndexPageSize(indexes[0], std::ptr::null_mut()) };
        assert_eq!(rc, AE_INVALID_POINTER);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);

        let (connect, table) = fixtures::open("indexes", c"pages8k.adt");
        let indexes = all_indexes(table);
        assert_eq!(
            unsafe { AdsGetIndexPageSize(indexes[0], &mut size) },
            AE_SUCCESS
        );
        assert_eq!(size, 8192);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn key_lengths_and_types() {
        use crate::{ADS_NUMERIC, ADS_RAW, ADS_STRING};
        let (connect, table) = fixtures::open("adi_keys", c"keys.adt");
        let order = |name: &CStr| {
            let mut handle = 0;
            let name = name.as_ptr().cast_mut().cast();
            assert_eq!(
                unsafe { AdsGetIndexHandle(table, name, &mut handle) },
                AE_SUCCESS
            );
            handle
        };
        for (name, length, key_type) in [
            (c"I_CHAR", 6, ADS_STRING),
            (c"I_MULTI", 22, ADS_RAW),
            (c"I_INTEGER", 8, ADS_NUMERIC),
            (c"X_STR", 11, ADS_STRING),
        ] {
            let (mut got_length, mut got_type) = (0, 0);
            assert_eq!(
                unsafe { AdsGetKeyLength(order(name), &mut got_length) },
                AE_SUCCESS
            );
            assert_eq!(
                unsafe { AdsGetKeyType(order(name), &mut got_type) },
                AE_SUCCESS
            );
            assert_eq!((got_length, got_type), (length, key_type), "{name:?}");
        }
        let mut value = 0;
        assert_eq!(
            unsafe { AdsGetKeyLength(table, &mut value) },
            AE_INVALID_INDEX_HANDLE
        );
        assert_eq!(
            unsafe { AdsGetKeyType(connect, &mut value) },
            AE_INVALID_INDEX_HANDLE
        );
        let null = std::ptr::null_mut();
        assert_eq!(
            unsafe { AdsGetKeyType(order(c"I_CHAR"), null) },
            AE_INVALID_POINTER
        );
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    /// As the original engine reports them (probed under Wine).
    #[test]
    fn index_flags() {
        /// A fixture folder, a table in it, and its orders' flags.
        type Case = (&'static str, &'static CStr, &'static [(&'static CStr, u32)]);
        let cases: [Case; 3] = [
            (
                "indexes",
                c"indexed.adt",
                &[
                    (c"PK_INDEXED", 0x803),
                    (c"BY_NAME", 0x2),
                    (c"BY_AMOUNT_DESC", 0xa),
                    (c"BY_CINAME", 0x803),
                ],
            ),
            (
                "adi_options",
                c"opts.adt",
                &[(c"COND", 0xc002), (c"CUST", 0x6)],
            ),
            (
                "adi_keys",
                c"keys.adt",
                &[
                    (c"X_UPPER", 0xc002),
                    (c"X_CONCAT", 0xc002),
                    (c"X_STR", 0x2),
                    (c"X_DTOS", 0x2),
                    (c"I_MULTI", 0x2),
                ],
            ),
        ];
        for (dir, table, orders) in cases {
            let (connect, table) = fixtures::open(dir, table);
            for &(name, want) in orders {
                let mut index = 0;
                let rc = unsafe {
                    AdsGetIndexHandle(table, name.as_ptr().cast_mut().cast(), &mut index)
                };
                assert_eq!(rc, AE_SUCCESS);
                let (mut flags, mut custom, mut compound) = (0, 0, 0);
                assert_eq!(unsafe { AdsGetIndexFlags(index, &mut flags) }, AE_SUCCESS);
                assert_eq!(flags, want, "{name:?}");
                assert_eq!(unsafe { AdsIsIndexCustom(index, &mut custom) }, AE_SUCCESS);
                assert_eq!(
                    unsafe { AdsIsIndexCompound(index, &mut compound) },
                    AE_SUCCESS
                );
                assert_eq!((custom, compound), (u16::from(want & 4 != 0), 1));
            }
            let mut flags = 0;
            assert_eq!(
                unsafe { AdsGetIndexFlags(table, &mut flags) },
                crate::AE_INVALID_INDEX_HANDLE
            );
            crate::AdsDisconnect(connect);
        }
    }

    /// As the original engine names them (probed under Wine).
    #[test]
    fn index_file_names() {
        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        let mut index = 0;
        let rc =
            unsafe { AdsGetIndexHandle(table, c"BY_NAME".as_ptr().cast_mut().cast(), &mut index) };
        assert_eq!(rc, AE_SUCCESS);
        let name = |option: u16, size: u16| {
            let (mut buf, mut len) = ([b'X'; 300], size);
            let rc = unsafe { AdsGetIndexFilename(index, option, buf.as_mut_ptr(), &mut len) };
            (rc, len, buf)
        };
        let (rc, len, buf) = name(1, 300);
        assert_eq!(
            (rc, &buf[..usize::from(len) + 1]),
            (AE_SUCCESS, &b"indexed\0"[..])
        );
        let (rc, len, buf) = name(2, 300);
        assert_eq!(
            (rc, &buf[..usize::from(len)]),
            (AE_SUCCESS, &b"indexed.adi"[..])
        );
        let (rc, len, buf) = name(3, 300);
        assert_eq!(rc, AE_SUCCESS);
        assert!(buf[..usize::from(len)].ends_with(b"indexed.adi"));
        // What does not fit is cut, with the size needed with the NUL.
        let (rc, len, buf) = name(2, 5);
        assert_eq!(
            (rc, len, &buf[..6]),
            (crate::AE_INSUFFICIENT_BUFFER, 12, &b"indexX"[..])
        );
        assert_eq!(name(0, 300).0, crate::AE_INVALID_OPTION);
        let mut len = 300;
        let mut buf = [0u8; 300];
        let rc = unsafe { AdsGetIndexFilename(table, 2, buf.as_mut_ptr(), &mut len) };
        assert_eq!(rc, crate::AE_INVALID_INDEX_HANDLE);
        crate::AdsDisconnect(connect);
    }

    /// As the original engine reports them (probed under Wine).
    #[test]
    fn index_collations() {
        let german = c"GERMAN_VFP_CI_AS_1252";
        let collation = |dir: &str, table: &CStr, order: &CStr, size: u16| {
            let opened = if dir == "adi_options" {
                german
            } else {
                c"ansi"
            };
            let (connect, table) = fixtures::open_with(dir, table, opened);
            let mut index = 0;
            let rc =
                unsafe { AdsGetIndexHandle(table, order.as_ptr().cast_mut().cast(), &mut index) };
            assert_eq!(rc, AE_SUCCESS);
            let (mut buf, mut len) = ([b'X'; 64], size);
            let rc = unsafe { AdsGetIndexCollation(index, buf.as_mut_ptr(), &mut len) };
            crate::AdsDisconnect(connect);
            (rc, len, buf)
        };
        let (rc, len, buf) = collation("adi_options", c"coll.adt", c"GERMAN", 64);
        assert_eq!(
            (rc, &buf[..usize::from(len)]),
            (AE_SUCCESS, &b"GERMAN_VFP_CI_AS_1252"[..])
        );
        let (rc, len, buf) = collation("adi_options", c"coll.adt", c"GERMAN", 5);
        assert_eq!(
            (rc, len, &buf[..6]),
            (crate::AE_INSUFFICIENT_BUFFER, 22, &b"GERMAX"[..])
        );
        let (rc, len, buf) = collation("indexes", c"indexed.adt", c"BY_NAME", 64);
        assert_eq!((rc, len, buf[0]), (AE_SUCCESS, 0, 0));
        let (rc, len, _) = collation("indexes", c"indexed.adt", c"BY_NAME", 0);
        assert_eq!((rc, len), (crate::AE_INSUFFICIENT_BUFFER, 1));

        // As in the original engine, the table needs the collation asked
        // for.
        let connect = fixtures::connect("adi_options");
        let (mut table, null) = (0, std::ptr::null_mut());
        let name = c"coll.adt".as_ptr().cast_mut().cast();
        let ansi = c"ansi".as_ptr().cast_mut().cast();
        let rc = unsafe {
            crate::AdsOpenTable90(
                connect,
                name,
                null,
                crate::ADS_ADT,
                1,
                1,
                2,
                0,
                ansi,
                &mut table,
            )
        };
        assert_eq!(rc, crate::AE_COLLATIONS_DO_NOT_MATCH);
        crate::AdsDisconnect(connect);
    }

    /// As in the original engine (`indexes` and `adi_options` fixtures).
    #[test]
    fn orders_by_number_and_expression() {
        let (connect, table) = fixtures::open("adi_options", c"opts.adt");
        let mut handles = Vec::new();
        for number in 0..=6 {
            let mut handle = 0x77;
            let rc = unsafe { AdsGetIndexHandleByOrder(table, number, &mut handle) };
            if rc == AE_SUCCESS {
                handles.push(handle);
                let mut back = 0;
                assert_eq!(
                    unsafe { AdsGetIndexOrderByHandle(handle, &mut back) },
                    AE_SUCCESS
                );
                assert_eq!(back, number);
            } else {
                assert_eq!(rc, crate::AE_INVALID_INDEX_ORDER, "{number}");
            }
        }
        // The full-text index is the fifth, though AdsGetNumIndexes says 4.
        assert_eq!(handles.len(), 5);
        let flag = |f: unsafe extern "system" fn(AdsHandle, *mut Unsigned16) -> Unsigned32,
                    handle| {
            let mut value = 0x77;
            let rc = unsafe { f(handle, &mut value) };
            (rc, value)
        };
        let candidates: Vec<_> = handles
            .iter()
            .map(|&h| flag(AdsIsIndexCandidate, h).1)
            .collect();
        assert_eq!(candidates, [0, 1, 1, 0, 0]);
        let fts: Vec<_> = handles.iter().map(|&h| flag(AdsIsIndexFTS, h).1).collect();
        assert_eq!(fts, [0, 0, 0, 0, 1]);
        assert_eq!(flag(AdsIsIndexUserDefined, handles[3]), (AE_SUCCESS, 0));
        assert_eq!(
            flag(AdsIsIndexNullable, handles[0]),
            (crate::AE_INVALID_INDEX_TYPE, 0x77)
        );
        assert_eq!(flag(AdsIsIndexNullable, table).0, AE_INVALID_INDEX_HANDLE);
        let mut number = 0;
        let rc = unsafe { AdsGetIndexOrderByHandle(table, &mut number) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);
        let (mut fts, mut len) = ([0; 4], 4);
        let rc = unsafe { AdsGetFTSIndexes(table, fts.as_mut_ptr(), &mut len) };
        assert_eq!((rc, len, fts[0]), (AE_SUCCESS, 1, handles[4]));
        let mut len = 0;
        let rc = unsafe { AdsGetFTSIndexes(table, fts.as_mut_ptr(), &mut len) };
        assert_eq!((rc, len), (AE_INSUFFICIENT_BUFFER, 1));

        let by_expr = |expression: &CStr, descending| {
            let mut handle = 0x77;
            let expression = expression.as_ptr().cast_mut().cast();
            let rc = unsafe { AdsGetIndexHandleByExpr(table, expression, descending, &mut handle) };
            (rc, handle)
        };
        // UNIQ and CAND both have `id`; the last wins. COND has a condition.
        assert_eq!(by_expr(c" ID ", 0), (AE_SUCCESS, handles[2]));
        assert_eq!(by_expr(c"id", 1), (AE_INVALID_OPTION, 0x77));
        assert_eq!(by_expr(c"doc", 0), (crate::AE_INDEX_EXPR_NOT_FOUND, 0));
        assert_eq!(by_expr(c"", 0), (AE_SUCCESS, 0));
        crate::AdsDisconnect(connect);

        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        let mut handle = 0x77;
        let amount = c"amount".as_ptr().cast_mut().cast();
        let rc = unsafe { AdsGetIndexHandleByExpr(table, amount, 0, &mut handle) };
        assert_eq!(
            (rc, handle),
            (crate::AE_INDEX_EXPR_NOT_FOUND, 0),
            "descending orders"
        );
        crate::AdsDisconnect(connect);
    }

    /// As in the original engine: the keys, walks and seeks of orders on
    /// numeric expressions of ADT, NTX, CDX and VFP tables, and on ADT tables
    /// of other numeric field types (`fixtures/expr/numkeys.json`, from
    /// `tools/ace/expr/numkeys.c`, whose steps this repeats).
    #[test]
    fn numeric_expression_keys() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/expr/numkeys.json");
        let want: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, _dir) =
            fixtures::scratch_from("numeric_expression_keys", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let text = |s: &str| std::ffi::CString::new(s).unwrap();
        let plain = "ID,N,6,0;D,N,8,2;C,C,4,0";
        let typed = "ID,N,6,0;D,N,8,2;C,C,4,0;I,Integer;F,Double,2";
        let exprs = [
            "ID",
            "ID + 1",
            "ID * 2",
            "ID - 1000",
            "D",
            "D + 1",
            "ID + D",
            "ID / 4",
            "-ID",
        ];
        let type_exprs = ["ID + 1", "1 + ID", "I + 1", "F + 1", "I * 2", "D + ID", "I"];
        /// A table type: its name in the fixture, its table, its index files'
        /// extension, its fields, the expressions of its orders and its type.
        type Kind<'a> = (&'a str, &'a str, &'a str, &'a str, &'a [&'a str], u16);
        let types: [Kind; 5] = [
            ("ADT", "nadt", "adi", plain, &exprs, ADS_ADT),
            ("NTX", "nntx", "ntx", plain, &exprs, crate::ADS_NTX),
            ("CDX", "ncdx", "cdx", plain, &exprs, crate::ADS_CDX),
            ("VFP", "nvfp", "cdx", plain, &exprs, crate::ADS_VFP),
            ("ADT types", "ntypes", "adi", typed, &type_exprs, ADS_ADT),
        ];
        let rows = [
            ("1001", "2.50", "a"),
            ("1002", "-1.25", "b"),
            ("1003", "10.00", "c"),
            ("999", "0.00", "d"),
        ];
        let set = |table, field: &str, value: &str| {
            let (field, value) = (text(field), text(value));
            unsafe {
                crate::AdsSetField(
                    table,
                    field.as_ptr().cast_mut().cast(),
                    value.as_ptr().cast_mut().cast(),
                    value.as_bytes().len() as Unsigned32,
                )
            };
        };
        let at = |table| -> Json {
            let (mut eof, mut recno) = (0, 0);
            unsafe { crate::AdsAtEOF(table, &mut eof) };
            unsafe { crate::AdsGetRecordNum(table, 2, &mut recno) };
            if eof != 0 { json!("E") } else { json!(recno) }
        };
        let mut got = Vec::new();
        for (t, (name, table_name, ext, fields, exprs, table_type)) in types.into_iter().enumerate()
        {
            let (table_name, fields) = (text(table_name), text(fields));
            let mut table = 0;
            let rc = unsafe {
                crate::AdsCreateTable(
                    connect,
                    table_name.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    table_type,
                    1,
                    0,
                    1,
                    0,
                    fields.as_ptr().cast_mut().cast(),
                    &mut table,
                )
            };
            assert_eq!(rc, AE_SUCCESS);
            for (id, d, c) in rows {
                crate::AdsAppendRecord(table);
                set(table, "ID", id);
                set(table, "D", d);
                set(table, "C", c);
                if name == "ADT types" {
                    set(table, "I", id);
                    set(table, "F", id);
                }
                crate::AdsWriteRecord(table);
            }
            for (e, expr) in exprs.iter().enumerate() {
                let (bag, tag, key, empty) = (
                    text(&format!("k{t}_{e}.{ext}")),
                    text("K"),
                    text(expr),
                    text(""),
                );
                let mut order = 0;
                let options = if table_type == crate::ADS_NTX { 0 } else { 2 };
                let rc = unsafe {
                    crate::AdsCreateIndex61(
                        table,
                        bag.as_ptr().cast_mut().cast(),
                        tag.as_ptr().cast_mut().cast(),
                        key.as_ptr().cast_mut().cast(),
                        empty.as_ptr().cast_mut().cast(),
                        empty.as_ptr().cast_mut().cast(),
                        options,
                        512,
                        &mut order,
                    )
                };
                let mut row = json!({"type": name, "expr": expr, "rc": rc});
                if rc != 0 {
                    got.push(row);
                    continue;
                }
                let (mut key_length, mut key_type) = (0, 0);
                unsafe { crate::AdsGetKeyLength(order, &mut key_length) };
                unsafe { AdsGetKeyType(order, &mut key_type) };
                let mut keys = Vec::new();
                for record in 1..=4 {
                    crate::AdsGotoRecord(table, record);
                    let (mut key, mut len) = ([0u8; 300], 300u16);
                    let rc = unsafe { AdsExtractKey(order, key.as_mut_ptr(), &mut len) };
                    keys.push(match rc {
                        0 => key[..usize::from(len)]
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect(),
                        rc => format!("E{rc}"),
                    });
                }
                let mut walk = Vec::new();
                crate::AdsGotoTop(order);
                for _ in 0..10 {
                    let mut eof = 0;
                    unsafe { crate::AdsAtEOF(table, &mut eof) };
                    if eof != 0 {
                        break;
                    }
                    let mut recno = 0;
                    unsafe { crate::AdsGetRecordNum(table, 2, &mut recno) };
                    walk.push(recno);
                    crate::AdsSkip(order, 1);
                }
                let mut seeks = Vec::new();
                for (value, how) in [(1003.0f64, 2u16), (1002.5, 2), (-1.0, 2), (1002.5, 1)] {
                    let mut found = 7;
                    crate::AdsGotoRecord(table, 1);
                    let rc = unsafe {
                        AdsSeek(
                            order,
                            &value as *const f64 as *mut u8,
                            8,
                            4,
                            how,
                            &mut found,
                        )
                    };
                    let key = if value.fract() == 0.0 {
                        json!(value as i64)
                    } else {
                        json!(value)
                    };
                    let how = if how == 1 { "soft" } else { "hard" };
                    seeks.push(json!({"key": key, "how": how, "rc": rc, "found": found,
                                      "at": at(table)}));
                }
                let mut found = 7;
                crate::AdsGotoRecord(table, 1);
                let rc = unsafe {
                    AdsSeek(
                        order,
                        c"1003".as_ptr().cast_mut().cast(),
                        4,
                        2,
                        2,
                        &mut found,
                    )
                };
                seeks.push(
                    json!({"key": "1003", "how": "string", "rc": rc, "found": found,
                                  "at": at(table)}),
                );
                row["key_length"] = json!(key_length);
                row["key_type"] = json!(key_type);
                row["keys"] = json!(keys);
                row["walk"] = json!(walk);
                row["seeks"] = json!(seeks);
                got.push(row);
            }
            crate::AdsCloseTable(table);
        }
        AdsDisconnect(connect);
        let problems: Vec<String> = want
            .iter()
            .zip(&got)
            .filter(|(want, got)| want != got)
            .map(|(want, got)| format!("want {want}\n got {got}"))
            .collect();
        assert_eq!(want.len(), got.len());
        assert!(
            problems.is_empty(),
            "{} differ:\n{}",
            problems.len(),
            problems.join("\n")
        );
    }

    /// As in the original engine: the names of orders by how they were made
    /// and found, for each kind of order and each way of naming its file
    /// (`fixtures/dbf/index_names.json`, from `tools/ace/dbf/index_names.c`,
    /// whose steps this repeats). Remotely, files named by a path of the
    /// server's own are left out, as a server refuses them.
    #[test]
    fn order_names() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/dbf/index_names.json");
        let remote = fixtures::is_remote();
        let want: Vec<Json> =
            serde_json::from_str::<Vec<Json>>(&std::fs::read_to_string(path).unwrap())
                .unwrap()
                .into_iter()
                .filter(|row| !remote || row["bag"] != "full")
                .collect();
        let (source, _dir) = fixtures::scratch_from("order_names", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let separator = if remote { r"\" } else { "/" };
        let text = |s: &str| std::ffi::CString::new(s).unwrap();
        let name_of = |order: AdsHandle| -> String {
            let (mut name, mut len) = ([0u8; 260], 260u16);
            let rc = unsafe { AdsGetIndexName(order, name.as_mut_ptr(), &mut len) };
            match rc {
                0 => String::from_utf8_lossy(&name[..usize::from(len)]).into_owned(),
                rc => format!("E{rc}"),
            }
        };
        let names = |table: AdsHandle| -> Vec<String> {
            let mut n = 0;
            unsafe { AdsGetNumIndexes(table, &mut n) };
            (1..=n.min(16))
                .map(|i| {
                    let mut order = 0;
                    unsafe { AdsGetIndexHandleByOrder(table, i, &mut order) };
                    name_of(order)
                })
                .collect()
        };
        let open_index = |table: AdsHandle, file: &str| -> (Unsigned32, Vec<String>) {
            let file = text(file);
            let (mut handles, mut len) = ([0 as AdsHandle; 16], 16u16);
            let rc = unsafe {
                AdsOpenIndex(
                    table,
                    file.as_ptr().cast_mut().cast(),
                    handles.as_mut_ptr(),
                    &mut len,
                )
            };
            let opened = match rc {
                0 | crate::AE_INDEX_ALREADY_OPEN => handles[..usize::from(len)]
                    .iter()
                    .map(|&h| name_of(h))
                    .collect(),
                _ => Vec::new(),
            };
            (rc, opened)
        };
        let kinds = [
            ("ADT", "adi", ADS_ADT, 0),
            ("NTX", "ntx", crate::ADS_NTX, 0),
            ("CDX idx", "cdx", crate::ADS_CDX, 0),
            ("VFP idx", "cdx", crate::ADS_VFP, 0),
            ("CDX compound", "cdx", crate::ADS_CDX, 2),
            ("VFP compound", "cdx", crate::ADS_VFP, 2),
        ];
        let mut got = Vec::new();
        for (k, (kind, ext, table_type, options)) in kinds.into_iter().enumerate() {
            let ntx = table_type == crate::ADS_NTX;
            let letter = char::from(b'a' + k as u8);
            for (b, bag) in ["lower", "mixed", "upper", "bare", "full"]
                .into_iter()
                .enumerate()
            {
                if remote && bag == "full" {
                    continue;
                }
                let ext = match bag {
                    "upper" => ext.to_ascii_uppercase(),
                    _ => ext.to_owned(),
                };
                let base = match bag {
                    "mixed" => "Bag_",
                    "upper" => "BAG_",
                    _ => "bag_",
                };
                let files: Vec<String> = ["n", "c"]
                    .iter()
                    .map(|suffix| {
                        let stem = format!("{base}{letter}{b}{}", if ntx { *suffix } else { "" });
                        match bag {
                            "bare" => stem,
                            "full" => format!("{source}{separator}{stem}.{ext}"),
                            _ => format!("{stem}.{ext}"),
                        }
                    })
                    .collect();
                let table_name = text(&format!("t{letter}{b}"));
                let mut table = 0;
                let rc = unsafe {
                    crate::AdsCreateTable(
                        connect,
                        table_name.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        table_type,
                        1,
                        0,
                        1,
                        0,
                        c"K,C,4,0".as_ptr().cast_mut().cast(),
                        &mut table,
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
                crate::AdsAppendRecord(table);
                unsafe {
                    crate::AdsSetString(
                        table,
                        c"K".as_ptr().cast_mut().cast(),
                        c"A".as_ptr().cast_mut().cast(),
                        1,
                    )
                };
                crate::AdsWriteRecord(table);
                let mut created = [0 as AdsHandle; 2];
                let mut rcs = 0;
                for (o, tag) in ["tg_n", "TG_C"].into_iter().enumerate() {
                    let (file, tag, key) =
                        (text(&files[if ntx { o } else { 0 }]), text(tag), text("K"));
                    let rc = unsafe {
                        crate::AdsCreateIndex61(
                            table,
                            file.as_ptr().cast_mut().cast(),
                            tag.as_ptr().cast_mut().cast(),
                            key.as_ptr().cast_mut().cast(),
                            c"".as_ptr().cast_mut().cast(),
                            c"".as_ptr().cast_mut().cast(),
                            options,
                            512,
                            &mut created[o],
                        )
                    };
                    if rc != 0 {
                        rcs = rc;
                    }
                }
                got.push(json!({"kind": kind, "bag": bag, "step": "create", "rc": rcs,
                                "names": names(table),
                                "created": created.iter().map(|&h| name_of(h)).collect::<Vec<_>>()}));
                AdsCloseAllIndexes(table);
                let (rc, opened) = open_index(table, &files[0]);
                if ntx {
                    open_index(table, &files[1]);
                }
                got.push(
                    json!({"kind": kind, "bag": bag, "step": "closed and opened",
                                "rc": rc, "names": names(table), "opened": opened}),
                );
                crate::AdsCloseTable(table);
                let rc = unsafe {
                    crate::AdsOpenTable(
                        connect,
                        table_name.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        table_type,
                        1,
                        0,
                        1,
                        1,
                        &mut table,
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
                let mut n = 0;
                unsafe { AdsGetNumIndexes(table, &mut n) };
                let (rc, opened) = match n {
                    0 => open_index(table, &files[0]),
                    _ => (0, Vec::new()),
                };
                got.push(
                    json!({"kind": kind, "bag": bag, "step": "reopened", "rc": rc,
                                "names": names(table), "opened": opened}),
                );
                crate::AdsCloseTable(table);
            }
        }
        AdsDisconnect(connect);
        let problems: Vec<String> = want
            .iter()
            .zip(&got)
            .filter(|(want, got)| want != got)
            .map(|(want, got)| format!("want {want}\n got {got}"))
            .collect();
        assert_eq!(want.len(), got.len());
        assert!(
            problems.is_empty(),
            "{} differ:\n{}",
            problems.len(),
            problems.join("\n")
        );
    }

    /// As in the original engine: which index file is the table's auto-open
    /// one for AdsOpenIndex, AdsCloseIndex and AdsCloseAllIndexes, on new and
    /// reopened ADT, CDX and VFP tables, with compound orders and without, and
    /// index handles across AdsPackTable (`fixtures/errors/index_files.json`,
    /// from `tools/ace/errors/index_files.c`, whose steps this repeats).
    #[test]
    fn auto_open_index_files() {
        use std::cell::RefCell;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/errors/index_files.json");
        // A server refuses index files named by a path of its own
        // (`AE_FILE_NOT_ON_SERVER`), so remotely only the files named
        // without a folder are replayed.
        let remote = fixtures::is_remote();
        let want: Vec<Json> =
            serde_json::from_str::<Vec<Json>>(&std::fs::read_to_string(path).unwrap())
                .unwrap()
                .into_iter()
                .filter(|row| !remote || row["path"] == "name")
                .collect();
        let (source, folder) =
            fixtures::scratch_from("auto_open_index_files", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let separator = if fixtures::is_remote() { r"\" } else { "/" };
        let text = |s: &str| std::ffi::CString::new(s).unwrap();
        let got: RefCell<Vec<Json>> = RefCell::new(Vec::new());
        let current = std::cell::Cell::new(0);
        let (kind, path_kind, scenario) = (RefCell::new(""), RefCell::new(""), RefCell::new(""));
        let row = |step: &str, rc: Unsigned32, table: AdsHandle| -> Json {
            let mut error = None;
            if rc != 0 {
                let (mut code, mut buf, mut len) = (0, [0u8; 600], 600u16);
                unsafe { crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
                error = Some(String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned());
            }
            let mut n = 777;
            unsafe { AdsGetNumIndexes(table, &mut n) };
            let mut row = json!({"type": *kind.borrow(), "path": *path_kind.borrow(),
                                 "scenario": *scenario.borrow(), "step": step, "rc": rc,
                                 "n": n});
            if let Some(error) = error {
                row["error"] = json!(error);
            }
            row
        };
        let push = |row: Json| got.borrow_mut().push(row);
        let provoke = || {
            let mut count = 0;
            unsafe { crate::AdsGetRecordCount(current.get(), 0, &mut count) };
        };
        let create_index_with = |table, bag: &str, tag: &str, options, order: &mut AdsHandle| {
            let (bag, tag, key, empty) = (text(bag), text(tag), text("K"), text(""));
            unsafe {
                crate::AdsCreateIndex61(
                    table,
                    bag.as_ptr().cast_mut().cast(),
                    tag.as_ptr().cast_mut().cast(),
                    key.as_ptr().cast_mut().cast(),
                    empty.as_ptr().cast_mut().cast(),
                    empty.as_ptr().cast_mut().cast(),
                    options,
                    512,
                    order,
                )
            }
        };
        let create_index = |table, bag: &str, tag: &str, order: &mut AdsHandle| {
            create_index_with(table, bag, tag, 0, order)
        };
        let close_index = |order| {
            provoke();
            AdsCloseIndex(order)
        };
        let open_index = |table, step: &str, bag: &str| -> AdsHandle {
            let mut handles = [0 as AdsHandle; 16];
            let mut len = 16u16;
            provoke();
            let bag = text(bag);
            let rc = unsafe {
                AdsOpenIndex(
                    table,
                    bag.as_ptr().cast_mut().cast(),
                    handles.as_mut_ptr(),
                    &mut len,
                )
            };
            let mut r = row(step, rc, table);
            let mut names = Vec::new();
            if rc == 0 || rc == crate::AE_INDEX_ALREADY_OPEN {
                for &handle in handles.iter().take(usize::from(len).min(16)) {
                    let (mut name, mut name_len) = ([0u8; 64], 64u16);
                    unsafe { AdsGetIndexName(handle, name.as_mut_ptr(), &mut name_len) };
                    let end = name.iter().position(|&b| b == 0).unwrap_or(0);
                    names.push(String::from_utf8_lossy(&name[..end]).into_owned());
                }
            }
            r["len"] = json!(len);
            r["names"] = json!(names);
            push(r);
            handles[0]
        };
        let handle_of = |table, tag: &str| {
            let tag = text(tag);
            let mut order = 0;
            unsafe { AdsGetIndexHandle(table, tag.as_ptr().cast_mut().cast(), &mut order) };
            order
        };
        let fill = |table| {
            let field = text("K");
            for value in ["A", "B", "C"] {
                let value = text(value);
                crate::AdsAppendRecord(table);
                unsafe {
                    crate::AdsSetString(
                        table,
                        field.as_ptr().cast_mut().cast(),
                        value.as_ptr().cast_mut().cast(),
                        1,
                    )
                };
                crate::AdsWriteRecord(table);
            }
        };
        let create_table = |name: &str, table_type| {
            let (name, fields) = (text(name), text("K,C,4,0"));
            let mut table = 0;
            let rc = unsafe {
                crate::AdsCreateTable(
                    connect,
                    name.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    table_type,
                    1,
                    0,
                    1,
                    0,
                    fields.as_ptr().cast_mut().cast(),
                    &mut table,
                )
            };
            assert_eq!(rc, AE_SUCCESS);
            current.set(table);
            table
        };
        let open_table = |name: &str, table_type| {
            let name = text(name);
            let mut table = 0;
            let rc = unsafe {
                crate::AdsOpenTable(
                    connect,
                    name.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    table_type,
                    1,
                    0,
                    1,
                    1,
                    &mut table,
                )
            };
            current.set(table);
            (rc, table)
        };
        // The probe tries to open the index file without sharing, which no
        // engine holding it would allow; here it is enough that the file is
        // there and can be read.
        let after_close = |stem: &str, table_ext: &str, ext: &str| {
            let held = std::fs::File::open(folder.join(format!("{stem}.{ext}"))).is_err();
            let header = std::fs::read(folder.join(format!("{stem}.{table_ext}"))).unwrap();
            let flags = if table_ext == "dbf" {
                i64::from(header[28])
            } else {
                -1
            };
            push(json!({"type": *kind.borrow(), "path": *path_kind.borrow(),
                        "scenario": *scenario.borrow(), "step": "closed",
                        "held": i32::from(held), "flags": flags}));
        };
        let types = [
            ("ADT", "adi", "adt", ADS_ADT),
            ("CDX", "cdx", "dbf", crate::ADS_CDX),
            ("VFP", "cdx", "dbf", crate::ADS_VFP),
        ];
        let mut n = 0;
        for (type_name, ext, table_ext, table_type) in types {
            *kind.borrow_mut() = type_name;
            for full in [false, true] {
                if full && remote {
                    n += 1;
                    continue;
                }
                *path_kind.borrow_mut() = if full { "full" } else { "name" };
                let bag = |stem: &str| match full {
                    true => format!("{source}{separator}{stem}.{ext}"),
                    false => format!("{stem}.{ext}"),
                };
                let (stem, other, pstem, rstem, lstem, cstem) = (
                    format!("t{n}"),
                    format!("o{n}"),
                    format!("p{n}"),
                    format!("r{n}"),
                    format!("l{n}"),
                    format!("c{n}"),
                );
                let (mut a, mut b, mut o) = (0, 0, 0);

                *scenario.borrow_mut() = "created";
                let table = create_table(&stem, table_type);
                fill(table);
                let rc = create_index(table, &bag(&stem), "A", &mut a);
                push(row("create A", rc, table));
                let rc = create_index(table, &bag(&stem), "B", &mut b);
                push(row("create B", rc, table));
                let rc = close_index(b);
                push(row("close B", rc, table));
                let rc = close_index(a);
                push(row("close A", rc, table));
                open_index(table, "open", &bag(&stem));
                open_index(table, "open again", &bag(&stem));
                push(row("close all", AdsCloseAllIndexes(table), table));
                crate::AdsCloseTable(table);
                after_close(&stem, table_ext, ext);

                *scenario.borrow_mut() = "created reopened";
                let (rc, table) = open_table(&stem, table_type);
                push(row("opened", rc, table));
                open_index(table, "open", &bag(&stem));
                crate::AdsCloseTable(table);

                *scenario.borrow_mut() = "reopened";
                let table = create_table(&rstem, table_type);
                fill(table);
                create_index(table, &bag(&rstem), "A", &mut a);
                create_index(table, &bag(&rstem), "B", &mut b);
                crate::AdsCloseTable(table);
                after_close(&rstem, table_ext, ext);
                let (rc, table) = open_table(&rstem, table_type);
                assert_eq!(rc, AE_SUCCESS);
                push(row("opened", 0, table));
                open_index(table, "open", &bag(&rstem));
                open_index(table, "open again", &bag(&rstem));
                let rc = close_index(handle_of(table, "A"));
                push(row("close A", rc, table));
                push(row("close all", AdsCloseAllIndexes(table), table));

                *scenario.borrow_mut() = "other";
                let rc = create_index(table, &bag(&other), "O", &mut o);
                push(row("create O", rc, table));
                let rc = close_index(o);
                push(row("close O", rc, table));
                o = open_index(table, "open", &bag(&other));
                open_index(table, "open again", &bag(&other));
                let rc = close_index(o);
                push(row("close O", rc, table));
                let rc = close_index(o);
                push(row("close O again", rc, table));

                *scenario.borrow_mut() = "pack";
                o = open_index(table, "open", &bag(&other));
                a = handle_of(table, "A");
                crate::AdsGotoRecord(table, 2);
                crate::AdsDeleteRecord(table);
                crate::AdsWriteRecord(table);
                push(row("pack", crate::AdsPackTable(table), table));
                push(row("top A", crate::AdsGotoTop(a), table));
                push(row("top O", crate::AdsGotoTop(o), table));
                let rc = close_index(a);
                push(row("close A", rc, table));
                let rc = close_index(o);
                push(row("close O", rc, table));
                let tag = text("A");
                let mut again = 0;
                let rc =
                    unsafe { AdsGetIndexHandle(table, tag.as_ptr().cast_mut().cast(), &mut again) };
                let mut r = row("handle A", rc, table);
                r["same"] = json!(i32::from(again == a));
                push(r);
                crate::AdsCloseTable(table);

                *scenario.borrow_mut() = "later";
                let table = create_table(&lstem, table_type);
                fill(table);
                crate::AdsCloseTable(table);
                let (rc, table) = open_table(&lstem, table_type);
                assert_eq!(rc, AE_SUCCESS);
                let rc = create_index(table, &bag(&lstem), "A", &mut a);
                push(row("create A", rc, table));
                let rc = create_index(table, &bag(&lstem), "B", &mut b);
                push(row("create B", rc, table));
                let rc = close_index(b);
                push(row("close B", rc, table));
                open_index(table, "open same", &bag(&lstem));
                crate::AdsCloseTable(table);
                after_close(&lstem, table_ext, ext);
                let (rc, table) = open_table(&lstem, table_type);
                push(row("opened", rc, table));
                open_index(table, "open", &bag(&lstem));
                let rc = close_index(handle_of(table, "A"));
                push(row("close A", rc, table));
                crate::AdsCloseTable(table);

                *scenario.borrow_mut() = "compound";
                let table = create_table(&cstem, table_type);
                fill(table);
                let rc = create_index_with(table, &bag(&cstem), "A", 2, &mut a);
                push(row("create A", rc, table));
                let rc = create_index_with(table, &bag(&cstem), "B", 2, &mut b);
                push(row("create B", rc, table));
                let rc = close_index(b);
                push(row("close B", rc, table));
                open_index(table, "open", &bag(&cstem));
                crate::AdsCloseTable(table);
                after_close(&cstem, table_ext, ext);
                let (rc, table) = open_table(&cstem, table_type);
                push(row("opened", rc, table));
                open_index(table, "open", &bag(&cstem));
                crate::AdsCloseTable(table);

                *scenario.borrow_mut() = "created pack";
                let table = create_table(&pstem, table_type);
                fill(table);
                let (file, tag, key) = (text(&bag(&pstem)), text("A"), text("K"));
                let rc = unsafe {
                    crate::AdsCreateIndex(
                        table,
                        file.as_ptr().cast_mut().cast(),
                        tag.as_ptr().cast_mut().cast(),
                        key.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        0,
                        &mut a,
                    )
                };
                push(row("create A", rc, table));
                crate::AdsGotoRecord(table, 2);
                crate::AdsDeleteRecord(table);
                crate::AdsWriteRecord(table);
                push(row("pack", crate::AdsPackTable(table), table));
                push(row("top A", crate::AdsGotoTop(a), table));
                let rc = close_index(a);
                push(row("close A", rc, table));
                crate::AdsCloseTable(table);
                n += 1;
            }
        }
        AdsDisconnect(connect);
        let got = got.into_inner();
        let problems: Vec<String> = want
            .iter()
            .zip(&got)
            .filter(|(want, got)| want != got)
            .map(|(want, got)| format!("want {want}\n got {got}"))
            .collect();
        assert_eq!(want.len(), got.len());
        assert!(
            problems.is_empty(),
            "{} of {} differ:\n{}",
            problems.len(),
            want.len(),
            problems.join("\n")
        );
    }

    /// As in the original engine (`adi_options`: `bitmap.adi` beside the
    /// structural `opts.adi`).
    #[test]
    fn index_files_open_and_close() {
        let (connect, table) = fixtures::open("adi_options", c"opts.adt");
        let count = || {
            let mut count = 0;
            unsafe { AdsGetNumIndexes(table, &mut count) };
            count
        };
        let open = |name: &CStr, size: u16| {
            let (mut handles, mut len) = ([0x77; 8], size);
            let name = name.as_ptr().cast_mut().cast();
            let rc = unsafe { AdsOpenIndex(table, name, handles.as_mut_ptr(), &mut len) };
            (rc, len, handles[0])
        };
        assert_eq!(count(), 4);
        let (rc, len, bitmap) = open(c"bitmap.adi", 8);
        assert_eq!((rc, len), (AE_SUCCESS, 1));
        assert_eq!(count(), 5);
        let mut number = 0;
        assert_eq!(
            unsafe { AdsGetIndexOrderByHandle(bitmap, &mut number) },
            AE_SUCCESS
        );
        assert_eq!(number, 6, "after the structural file's full-text index");
        let mut records = 0;
        assert_eq!(
            unsafe { crate::AdsGetRecordCount(bitmap, 2, &mut records) },
            AE_SUCCESS
        );
        assert_eq!(records, 41, "a binary order's keys");
        assert_eq!(
            open(c"BITMAP", 8),
            (crate::AE_INDEX_ALREADY_OPEN, 1, bitmap)
        );
        assert_eq!(open(c"opts.adi", 8).0, crate::AE_INDEX_ALREADY_OPEN);
        assert_eq!(open(c"nosuch.adi", 8), (INDEX_FILE_NOT_FOUND, 8, 0x77));
        assert_eq!(open(c"bitmap.adi", 0), (AE_INSUFFICIENT_BUFFER, 1, 0x77));

        let mut uniq = 0;
        unsafe { AdsGetIndexHandle(table, c"UNIQ".as_ptr().cast_mut().cast(), &mut uniq) };
        assert_eq!(AdsCloseIndex(uniq), crate::AE_AUTOOPEN_INDEX);
        assert_eq!(AdsCloseIndex(bitmap), AE_SUCCESS);
        assert_eq!(AdsCloseIndex(bitmap), AE_INVALID_INDEX_HANDLE);
        assert_eq!(count(), 4);
        open(c"bitmap.adi", 8);
        assert_eq!(count(), 5);
        assert_eq!(AdsCloseAllIndexes(table), AE_SUCCESS);
        assert_eq!(count(), 4, "but the structural index");
        assert_eq!(AdsCloseAllIndexes(connect), AE_INVALID_TABLE_HANDLE);
        crate::AdsDisconnect(connect);
    }

    /// Where the table is, as `tools/ace/dbf/index.c` records it after a
    /// move in `order`.
    fn probed_position(table: AdsHandle, order: AdsHandle, step: &Json, rc: u32) -> Json {
        let (mut eof, mut bof, mut found, mut record) = (0, 0, 0, 0);
        let mut key_num = 0x5A5A_5A5A;
        let mut key = [0u8; 300];
        let mut len = key.len() as u16;
        let (key_num_rc, key_rc) = unsafe {
            crate::AdsAtEOF(table, &mut eof);
            crate::AdsAtBOF(table, &mut bof);
            crate::AdsIsFound(table, &mut found);
            crate::AdsGetRecordNum(table, 2, &mut record);
            (
                AdsGetKeyNum(order, 2, &mut key_num),
                AdsExtractKey(order, key.as_mut_ptr(), &mut len),
            )
        };
        let key = if key_rc == 0 {
            &key[..usize::from(len)]
        } else {
            &[][..]
        };
        json!({
            "table": step["table"], "index": step["index"], "case": step["case"],
            "fn": step["fn"], "rc": rc, "record": record, "eof": eof, "bof": bof,
            "found": found, "key_num_rc": key_num_rc, "key_num": key_num, "key_rc": key_rc,
            "key": key.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        })
    }

    /// Where the original engine and tflip are known to differ in
    /// `fixtures/dbf/indexes.json` (see KNOWN_ISSUES): the key `AdsExtractKey`
    /// gives at EOF after a seek, text that is not a date in the date format
    /// seeking a date order (the original reads it leniently), and numbers
    /// too wide for their key.
    fn known_difference(want: &Json, got: &Json) -> bool {
        let case = want["case"].as_str().unwrap_or("");
        let index = want["index"].as_str().unwrap_or("");
        if want["fn"] == "AdsSeek" && want["eof"] == 1 {
            let mut want = want.clone();
            let mut got = got.clone();
            for value in [&mut want, &mut got] {
                value["key"] = Json::Null;
                value["key_rc"] = Json::Null;
            }
            if want == got {
                return true;
            }
        }
        let date_order = matches!(index, "i_date" | "I_DATE" | "BY_BORN" | "I_TS");
        date_order && (case.starts_with("text") || case.starts_with("key"))
            || index == "i_num" && (case.contains("2.45807e+06") || case.contains("20171108"))
    }

    /// NTX, CDX and IDX orders behave as in the original engine: their
    /// properties, walks with key numbers and keys, and seeks
    /// (`fixtures/dbf/indexes.json`, probed with `tools/ace/dbf/index.c`).
    #[test]
    fn dbf_orders_match_the_original_engine() {
        // Other tests hide deleted records for a while (`AdsShowDeleted`).
        let _settings = fixtures::settings_lock();
        let path = fixtures::dir("../dbf/indexes.json");
        let steps: Vec<Json> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let mut failures = Vec::new();
        let (mut connect, mut table) = (0, 0);
        let mut orders: std::collections::HashMap<String, AdsHandle> = Default::default();
        // The distinct keys of the current order's walk, which its seeks use.
        let mut keys: Vec<Vec<u8>> = Vec::new();
        let hex = |s: &Json| -> Vec<u8> {
            let s = s.as_str().unwrap_or("");
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        };
        for step in &steps {
            let name = step["fn"].as_str().unwrap();
            let case = step["case"].as_str().unwrap();
            let index = step["index"].as_str().unwrap();
            if name == "AdsOpenTable" {
                if connect != 0 {
                    crate::AdsDisconnect(connect);
                }
                orders.clear();
                let file = step["table"].as_str().unwrap();
                let (kind, base) = file.trim_end_matches(".dbf").split_once('/').unwrap();
                let folder = match (kind, base) {
                    ("ntx", "opts") => "ntx_options",
                    (_, "opts") => "cdx_options",
                    (_, "ca") => "charset_default",
                    (_, base) => base,
                };
                let table_type = match kind {
                    "ntx" => crate::ADS_NTX,
                    "cdx" => crate::ADS_CDX,
                    _ => crate::ADS_VFP,
                };
                connect = fixtures::connect(&format!("{kind}/{folder}"));
                let file = std::ffi::CString::new(file.rsplit('/').next().unwrap()).unwrap();
                let null = std::ptr::null_mut();
                let rc = unsafe {
                    crate::AdsOpenTable(
                        connect,
                        file.as_ptr().cast_mut().cast(),
                        null,
                        table_type,
                        crate::ADS_ANSI,
                        1,
                        1,
                        0,
                        &mut table,
                    )
                };
                assert_eq!(rc, 0, "{file:?}");
                continue;
            }
            // The orders open with the table, and those of index files, by
            // name.
            let mut name_orders = |handles: &[AdsHandle]| {
                for &handle in handles {
                    let mut buf = [0u8; 64];
                    let mut len = buf.len() as u16;
                    unsafe { AdsGetIndexName(handle, buf.as_mut_ptr(), &mut len) };
                    let name = String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned();
                    orders.insert(name, handle);
                }
            };
            if name == "AdsGetNumIndexes" {
                let mut n = 0;
                unsafe { AdsGetNumIndexes(table, &mut n) };
                assert_eq!(step["orders"], n, "{}", step["table"]);
                let handles: Vec<AdsHandle> = (1..=n)
                    .map(|i| {
                        let mut handle = 0;
                        unsafe { AdsGetIndexHandleByOrder(table, i, &mut handle) };
                        handle
                    })
                    .collect();
                name_orders(&handles);
                continue;
            }
            if name == "AdsOpenIndex" {
                let file = std::ffi::CString::new(index).unwrap();
                let mut handles = [0; 50];
                let mut n = handles.len() as u16;
                let rc = unsafe {
                    AdsOpenIndex(
                        table,
                        file.as_ptr().cast_mut().cast(),
                        handles.as_mut_ptr(),
                        &mut n,
                    )
                };
                assert_eq!(step["rc"], rc, "{index}");
                if rc == 0 {
                    assert_eq!(step["orders"], n, "{index}");
                    name_orders(&handles[..usize::from(n)]);
                }
                continue;
            }
            if case == "info" && name == "AdsGetIndexName" {
                keys.clear();
            }
            let order = orders[index];
            let got = match (name, case) {
                (_, "info") if step.get("len").is_some() => {
                    let getter: Getter = match name {
                        "AdsGetIndexName" => AdsGetIndexName,
                        "AdsGetIndexExpr" => AdsGetIndexExpr,
                        _ => AdsGetIndexCondition,
                    };
                    let mut buf = [0u8; 600];
                    let mut len = 500;
                    let rc = unsafe { getter(order, buf.as_mut_ptr(), &mut len) };
                    let value = if rc == 0 {
                        &buf[..usize::from(len)]
                    } else {
                        &[][..]
                    };
                    let value: String = value.iter().map(|b| format!("{b:02x}")).collect();
                    let mut got = step.clone();
                    got["rc"] = json!(rc);
                    got["len"] = json!(len);
                    got["value"] = json!(value);
                    got
                }
                (_, "info") => {
                    let mut value = 0x5A5A;
                    let rc = unsafe {
                        match name {
                            "AdsGetKeyLength" => AdsGetKeyLength(order, &mut value),
                            "AdsGetKeyType" => AdsGetKeyType(order, &mut value),
                            "AdsIsIndexUnique" => AdsIsIndexUnique(order, &mut value),
                            "AdsIsIndexDescending" => AdsIsIndexDescending(order, &mut value),
                            _ => AdsIsIndexCompound(order, &mut value),
                        }
                    };
                    let mut got = step.clone();
                    got["rc"] = json!(rc);
                    got["value"] = json!(value);
                    got
                }
                ("AdsGetKeyCount", _) => {
                    let mut value = 0x5A5A_5A5A;
                    let option = if case == "respect filters" { 1 } else { 2 };
                    let rc = unsafe { AdsGetKeyCount(order, option, &mut value) };
                    let mut got = step.clone();
                    got["rc"] = json!(rc);
                    got["value"] = json!(value);
                    got
                }
                ("AdsGotoTop" | "AdsGotoBottom" | "AdsSkip", _) => {
                    if (case == "top" || case == "forward") && step["eof"] == 0 {
                        let key = hex(&step["key"]);
                        if keys.len() < 40 && keys.last() != Some(&key) {
                            keys.push(key);
                        }
                    }
                    let rc = match case {
                        "top" => crate::AdsGotoTop(order),
                        "bottom" => crate::AdsGotoBottom(order),
                        "forward" => crate::AdsSkip(order, 1),
                        _ => crate::AdsSkip(order, -1),
                    };
                    probed_position(table, order, step, rc)
                }
                _ => {
                    // "<what> type <t> how <h>"
                    let words: Vec<&str> = case.split(' ').collect();
                    let how: u16 = words[words.len() - 1].parse().unwrap();
                    let data_type: u16 = words[words.len() - 3].parse().unwrap();
                    let what = words[..words.len() - 4].join(" ");
                    let mut bytes: Vec<u8> = if let Some(text) = what.strip_prefix("text ") {
                        text.as_bytes().to_vec()
                    } else if let Some(number) = what.strip_prefix("double ") {
                        let value: f64 = match number {
                            "2.45807e+06" => 2458066.0,
                            number => number.parse().unwrap(),
                        };
                        value.to_le_bytes().to_vec()
                    } else if what == "past" {
                        vec![0xFF; 3]
                    } else {
                        let n: usize = words[1].parse().unwrap();
                        let key = keys[n].clone();
                        if what.ends_with("first") {
                            key[..1].to_vec()
                        } else {
                            key
                        }
                    };
                    let mut found = 0x5A5A;
                    let rc = unsafe {
                        AdsSeek(
                            order,
                            bytes.as_mut_ptr(),
                            bytes.len() as u16,
                            data_type,
                            how,
                            &mut found,
                        )
                    };
                    probed_position(table, order, step, rc)
                }
            };
            if &got != step && !known_difference(step, &got) {
                failures.push(format!("want {step}\n got {got}"));
            }
        }
        crate::AdsDisconnect(connect);
        assert!(
            failures.is_empty(),
            "{} differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// An index order built on an expression reports the key type and length of
    /// its result, and a numeric expression order is sought by a double key, as
    /// the original engine does (`fixtures/expr/keytype.json`, probed with
    /// `tools/ace/expr/keytype.c` on the `expr_keys` fixture): `n + 1` and
    /// `d * 2` are `ADS_NUMERIC`, `dt + 1` is `ADS_DATE`, `n > 0` is
    /// `ADS_LOGICAL`, `UPPER(c)` is `ADS_STRING`; seeking `X_NUM` for 11.0 lands
    /// on the row whose `n + 1` is 11.
    #[test]
    fn expression_key_types_and_seeks_match_the_engine() {
        use crate::{AdsGetField, AdsGetKeyLength, AdsSeek};
        let cases: Vec<Json> =
            serde_json::from_slice(&std::fs::read(fixtures::dir("../expr/keytype.json")).unwrap())
                .unwrap();
        let (connect, table) = fixtures::open("expr_keys", c"keys.adt");
        let order = |name: &str| {
            let name = std::ffi::CString::new(name).unwrap();
            let mut handle = 0;
            assert_eq!(
                unsafe { AdsGetIndexHandle(table, name.as_ptr().cast_mut().cast(), &mut handle) },
                AE_SUCCESS,
                "order {name:?}"
            );
            handle
        };
        let mut failures = Vec::new();
        for case in &cases {
            let op = case["op"].as_str().unwrap();
            if op == "keytype" {
                let name = case["order"].as_str().unwrap();
                let (mut len, mut kt) = (0u16, 0u16);
                unsafe { AdsGetKeyLength(order(name), &mut len) };
                unsafe { AdsGetKeyType(order(name), &mut kt) };
                let want = (
                    case["key_type"].as_u64().unwrap() as u16,
                    case["key_length"].as_u64().unwrap() as u16,
                );
                if (kt, len) != want {
                    failures.push(format!("{name} key: want {want:?} got {:?}", (kt, len)));
                }
            } else if let Some(rest) = op.strip_prefix("seek ") {
                let (name, value) = rest.split_once(' ').unwrap();
                let key: f64 = value.parse().unwrap();
                let mut found = 0u16;
                // ADS_DOUBLEKEY = 4, ADS_HARDSEEK = 2.
                unsafe {
                    AdsSeek(
                        order(name),
                        &key as *const f64 as *mut u8,
                        8,
                        4,
                        2,
                        &mut found,
                    )
                };
                let got_found = u64::from(found != 0);
                let mut buf = [0u8; 32];
                let mut buf_len = buf.len() as u32 - 1;
                let id = std::ffi::CString::new("id").unwrap();
                let got_id = if found != 0
                    && unsafe {
                        AdsGetField(
                            table,
                            id.as_ptr().cast_mut().cast(),
                            buf.as_mut_ptr(),
                            &mut buf_len,
                            0,
                        )
                    } == AE_SUCCESS
                {
                    Json::String(String::from_utf8_lossy(&buf[..buf_len as usize]).into_owned())
                } else {
                    Json::Null
                };
                if got_found != case["found"].as_u64().unwrap() || got_id != case["id"] {
                    failures.push(format!(
                        "{op:?}: want found {} id {}, got found {got_found} id {got_id}",
                        case["found"], case["id"]
                    ));
                }
            }
        }
        assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
        assert!(
            failures.is_empty(),
            "{} differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// As in the original engine: which `qualifier->field` names index keys,
    /// AdsIsExprValid, AdsEvalNumericExpr, filters and AOFs take, with the
    /// table open under its own alias and another (`fixtures/expr/alias.json`,
    /// from `tools/ace/expr/alias.c`, whose steps this repeats).
    #[test]
    fn qualified_field_names() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/expr/alias.json");
        let want: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let exprs: Vec<String> = want
            .iter()
            .filter(|row| row["type"] == "ADT" && row["alias"] == "acct" && row["op"] == "index")
            .map(|row| row["expr"].as_str().unwrap().to_owned())
            .collect();
        let (source, folder) =
            fixtures::scratch_from("qualified_field_names", &fixtures::empty_dir());
        let separator = if fixtures::is_remote() { r"\" } else { "/" };
        let connect = fixtures::connect_to(&source);
        let text = |s: &str| std::ffi::CString::new(s).unwrap();
        let mut got: Vec<Json> = Vec::new();
        let types = [
            ("ADT", "adt", "adi", ADS_ADT),
            ("NTX", "ntx", "ntx", crate::ADS_NTX),
            ("CDX", "cdx", "cdx", crate::ADS_CDX),
            ("VFP", "vfp", "cdx", crate::ADS_VFP),
        ];
        for (t, (type_name, dir, ext, table_type)) in types.into_iter().enumerate() {
            std::fs::create_dir(folder.join(dir)).unwrap();
            let name = text(&format!("{dir}{separator}acct"));
            let fields = text("ID,N,6,0;C,C,4,0");
            let mut table = 0;
            let rc = unsafe {
                crate::AdsCreateTable(
                    connect,
                    name.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
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
            let (id, c) = (text("ID"), text("C"));
            for (i, value) in ["a", "b", "c"].into_iter().enumerate() {
                let value = text(value);
                crate::AdsAppendRecord(table);
                unsafe {
                    crate::AdsSetDouble(table, id.as_ptr().cast_mut().cast(), 1001.0 + i as f64)
                };
                unsafe {
                    crate::AdsSetString(
                        table,
                        c.as_ptr().cast_mut().cast(),
                        value.as_ptr().cast_mut().cast(),
                        1,
                    )
                };
                crate::AdsWriteRecord(table);
            }
            crate::AdsCloseTable(table);
            for (a, alias) in [None, Some("XX")].into_iter().enumerate() {
                let alias_name = alias.unwrap_or("acct");
                let alias = alias.map(text);
                let rc = unsafe {
                    crate::AdsOpenTable(
                        connect,
                        name.as_ptr().cast_mut().cast(),
                        alias.as_ref().map_or(std::ptr::null_mut(), |alias| {
                            alias.as_ptr().cast_mut().cast()
                        }),
                        table_type,
                        1,
                        1,
                        1,
                        1,
                        &mut table,
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
                for (e, expr) in exprs.iter().enumerate() {
                    let row = |op: &str, rc: Unsigned32| {
                        let mut row = json!({"type": type_name, "alias": alias_name,
                                             "expr": expr, "op": op, "rc": rc});
                        if rc != 0 {
                            let (mut code, mut buf, mut len) = (0, [0u8; 600], 600u16);
                            unsafe {
                                crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len)
                            };
                            row["error"] = json!(String::from_utf8_lossy(&buf[..usize::from(len)]));
                        }
                        row
                    };
                    let expr_c = text(expr);
                    let expr_p = expr_c.as_ptr().cast_mut().cast();
                    let (bag, tag, empty) =
                        (text(&format!("b{t}{a}_{e}.{ext}")), text("T"), text(""));
                    let mut order = 0;
                    crate::AdsGotoRecord(table, 2);
                    let rc = unsafe {
                        crate::AdsCreateIndex61(
                            table,
                            bag.as_ptr().cast_mut().cast(),
                            tag.as_ptr().cast_mut().cast(),
                            expr_p,
                            empty.as_ptr().cast_mut().cast(),
                            empty.as_ptr().cast_mut().cast(),
                            0,
                            512,
                            &mut order,
                        )
                    };
                    let mut index = row("index", rc);
                    if rc == 0 {
                        let (mut back, mut len) = ([0u8; 300], 300u16);
                        unsafe { AdsGetIndexExpr(order, back.as_mut_ptr(), &mut len) };
                        let (mut key_length, mut key_type, mut found) = (0, 0, 7);
                        unsafe { crate::AdsGetKeyLength(order, &mut key_length) };
                        unsafe { AdsGetKeyType(order, &mut key_type) };
                        let key = 1003.0f64;
                        let seek_rc = unsafe {
                            AdsSeek(order, &key as *const f64 as *mut u8, 8, 4, 2, &mut found)
                        };
                        let (mut eof, mut recno) = (0, 0);
                        unsafe { crate::AdsAtEOF(table, &mut eof) };
                        unsafe { crate::AdsGetRecordNum(table, 2, &mut recno) };
                        index["key_expr"] =
                            json!(String::from_utf8_lossy(&back[..usize::from(len)]));
                        index["key_length"] = json!(key_length);
                        index["key_type"] = json!(key_type);
                        index["seek_rc"] = json!(seek_rc);
                        index["found"] = json!(found);
                        index["at"] = if eof != 0 { json!("E") } else { json!(recno) };
                        crate::AdsCloseIndex(order);
                    }
                    got.push(index);

                    let mut valid = 7;
                    crate::AdsGotoRecord(table, 2);
                    let rc = unsafe { crate::AdsIsExprValid(table, expr_p, &mut valid) };
                    let mut r = row("valid", rc);
                    r["valid"] = json!(valid);
                    got.push(r);

                    let mut value = 777.0;
                    let rc = unsafe { crate::AdsEvalNumericExpr(table, expr_p, &mut value) };
                    let mut r = row("eval", rc);
                    r["value"] = if value.fract() == 0.0 {
                        json!(value as i64)
                    } else {
                        json!(value)
                    };
                    got.push(r);

                    let cond = text(&format!("({expr}) > 1001"));
                    let mut count = 777;
                    let rc = unsafe { crate::AdsSetFilter(table, cond.as_ptr().cast_mut().cast()) };
                    let mut r = row("filter", rc);
                    if rc == 0 {
                        unsafe { crate::AdsGetRecordCount(table, 1, &mut count) };
                    }
                    r["count"] = json!(count);
                    got.push(r);
                    crate::AdsClearFilter(table);

                    let mut count = 777;
                    let rc = unsafe { crate::AdsSetAOF(table, cond.as_ptr().cast_mut().cast(), 2) };
                    let mut r = row("aof", rc);
                    if rc == 0 {
                        unsafe { crate::AdsGetRecordCount(table, 1, &mut count) };
                    }
                    r["count"] = json!(count);
                    got.push(r);
                    crate::AdsClearAOF(table);
                }
                crate::AdsCloseTable(table);
            }
        }
        AdsDisconnect(connect);
        let problems: Vec<String> = want
            .iter()
            .zip(&got)
            .filter(|(want, got)| want != got)
            .map(|(want, got)| format!("want {want}\n got {got}"))
            .collect();
        assert_eq!(want.len(), got.len());
        assert!(
            problems.is_empty(),
            "{} of {} differ:\n{}",
            problems.len(),
            want.len(),
            problems.join("\n")
        );
    }

    /// As in the original engine: fields named `AND`, `OR`, `NOT`, `TRUE`,
    /// `FALSE` and `NULL` in filters and index keys, where ADT and DBF
    /// tables part ways (`fixtures/expr/keywords.json`, from
    /// `tools/ace/expr/keywords.c`, whose steps this repeats).
    #[test]
    fn keyword_field_names() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/expr/keywords.json");
        let want: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let exprs = |table: &str, op: &str, type_name: &str| -> Vec<String> {
            want.iter()
                .filter(|row| row["table"] == table && row["op"] == op && row["type"] == type_name)
                .map(|row| row["expr"].as_str().unwrap().to_owned())
                .collect()
        };
        let (source, _folder) =
            fixtures::scratch_from("keyword_field_names", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let text = |s: &str| std::ffi::CString::new(s).unwrap();
        let mut got: Vec<Json> = Vec::new();
        let types = [
            ("ADT", "adi", ADS_ADT, 2),
            ("NTX", "ntx", crate::ADS_NTX, 0),
            ("CDX", "cdx", crate::ADS_CDX, 2),
        ];
        let kw_chars = ["AND", "OR", "TRUE", "FALSE", "NULL"];
        let kw_values = [
            ["a", "b", "a", "b", "a"],
            ["b", "a", "b", "a", "b"],
            ["a", "a", "a", "a", "a"],
        ];
        for (t, (type_name, ext, table_type, options)) in types.into_iter().enumerate() {
            for (k, table_name) in ["kw", "plain"].into_iter().enumerate() {
                let name = text(&format!("{table_name}{t}"));
                let fields = text(if k == 1 {
                    "C,C,2,0;X,Logical"
                } else {
                    "AND,C,2,0;OR,C,2,0;TRUE,C,2,0;FALSE,C,2,0;NULL,C,2,0;NOT,Logical;X,Logical"
                });
                let mut table = 0;
                let rc = unsafe {
                    crate::AdsCreateTable(
                        connect,
                        name.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        table_type,
                        1,
                        0,
                        1,
                        0,
                        fields.as_ptr().cast_mut().cast(),
                        &mut table,
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
                for r in 0..3 {
                    crate::AdsAppendRecord(table);
                    let set = |field: &str, value: &str| {
                        let (field, value) = (text(field), text(value));
                        unsafe {
                            crate::AdsSetString(
                                table,
                                field.as_ptr().cast_mut().cast(),
                                value.as_ptr().cast_mut().cast(),
                                1,
                            )
                        };
                    };
                    let logical = |field: &str, value: bool| {
                        let field = text(field);
                        unsafe {
                            crate::AdsSetLogical(
                                table,
                                field.as_ptr().cast_mut().cast(),
                                u16::from(value),
                            )
                        };
                    };
                    if k == 1 {
                        set("C", ["a", "b", "a"][r]);
                    } else {
                        for (field, value) in kw_chars.iter().zip(kw_values[r]) {
                            set(field, value);
                        }
                        logical("NOT", r == 0);
                    }
                    logical("X", r < 2);
                    crate::AdsWriteRecord(table);
                }
                for expr in exprs(table_name, "filter", type_name) {
                    let cond = text(&expr);
                    let (mut count, mut count_rc) = (777, 0);
                    let rc = unsafe { crate::AdsSetFilter(table, cond.as_ptr().cast_mut().cast()) };
                    if rc == 0 {
                        count_rc = unsafe { crate::AdsGetRecordCount(table, 1, &mut count) };
                        crate::AdsClearFilter(table);
                    }
                    got.push(
                        json!({"type": type_name, "table": table_name, "op": "filter",
                                    "expr": expr, "rc": rc, "count_rc": count_rc,
                                    "count": count}),
                    );
                }
                let prefix = &table_name[..1];
                for (e, expr) in exprs(table_name, "index", type_name)
                    .into_iter()
                    .enumerate()
                {
                    let (bag, tag, empty, key) = (
                        text(&format!("{prefix}{e}.{ext}")),
                        text("T"),
                        text(""),
                        text(&expr),
                    );
                    let mut order = 0;
                    let rc = unsafe {
                        crate::AdsCreateIndex61(
                            table,
                            bag.as_ptr().cast_mut().cast(),
                            tag.as_ptr().cast_mut().cast(),
                            key.as_ptr().cast_mut().cast(),
                            empty.as_ptr().cast_mut().cast(),
                            empty.as_ptr().cast_mut().cast(),
                            options,
                            512,
                            &mut order,
                        )
                    };
                    let mut walk = Vec::new();
                    if rc == 0 {
                        crate::AdsGotoTop(order);
                        for _ in 0..5 {
                            let (mut eof, mut recno) = (0, 0);
                            unsafe { crate::AdsAtEOF(table, &mut eof) };
                            if eof != 0 {
                                break;
                            }
                            unsafe { crate::AdsGetRecordNum(table, 2, &mut recno) };
                            walk.push(recno);
                            crate::AdsSkip(order, 1);
                        }
                        crate::AdsCloseIndex(order);
                    }
                    got.push(
                        json!({"type": type_name, "table": table_name, "op": "index",
                                    "expr": expr, "rc": rc, "walk": walk}),
                    );
                }
                crate::AdsCloseTable(table);
            }
        }
        AdsDisconnect(connect);
        assert_eq!(want.len(), got.len());
        // Set aside: `NULL` alone where no field has the name (the original
        // takes it as a filter, then fails the record count with 4109, and
        // refuses an order on it with 4009, or 5011 on DBF tables), and the
        // codes of two expressions both refuse on DBF tables (`NOT = .T.`
        // and `NOT .AND. X` with a field NOT: 3111 and 3101 there, 3106 here).
        let set_aside = |row: &Json| {
            (row["table"] == "plain" && row["expr"] == "NULL")
                || (row["type"] != "ADT"
                    && (row["expr"] == "NOT = .T." || row["expr"] == "NOT .AND. X"))
        };
        let mut aside = 0;
        let mut problems = Vec::new();
        for (want, got) in want.iter().zip(&got) {
            if want == got {
                continue;
            }
            if set_aside(want) {
                aside += 1;
            } else {
                problems.push(format!("want {want}\n got {got}"));
            }
        }
        assert!(
            problems.is_empty(),
            "{} of {} differ:\n{}",
            problems.len(),
            want.len(),
            problems.join("\n")
        );
        assert_eq!(aside, 10, "rows set aside");
    }

    /// As in the original engine: `AdsOpenIndex` looks for an index file
    /// named alone in the table's folder only, and `AdsCreateIndex61` makes
    /// it there; one named with a folder is relative to the current
    /// directory, which is the connection's
    /// folder here (`fixtures/errors/index_paths.json`, from
    /// `tools/ace/errors/index_paths.c`; its "same" layout, with the current
    /// directory the connection's folder).
    #[test]
    fn relative_index_file_names() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/errors/index_paths.json");
        let want: Vec<Json> =
            serde_json::from_str::<Vec<Json>>(&std::fs::read_to_string(path).unwrap())
                .unwrap()
                .into_iter()
                .filter(|row| row["layout"] == "same")
                .collect();
        let (source, folder) =
            fixtures::scratch_from("relative_index_file_names", &fixtures::empty_dir());
        for sub in ["tables/data", "data"] {
            std::fs::create_dir_all(folder.join(sub)).unwrap();
        }
        let separator = if fixtures::is_remote() { r"\" } else { "/" };
        let connect = fixtures::connect_to(&source);
        let text = |s: &str| std::ffi::CString::new(s).unwrap();
        let types = [
            ("ADT", "adi", ADS_ADT, 2),
            ("NTX", "ntx", crate::ADS_NTX, 0),
            ("CDX", "cdx", crate::ADS_CDX, 2),
        ];
        let mut got = Vec::new();
        for (type_name, ext, table_type, options) in types {
            let in_source = |relative: &str| {
                let relative = relative.replace('/', separator);
                format!("{source}{separator}{relative}")
            };
            let name = text(&in_source(&format!("tables/t{table_type}")));
            let mut table = 0;
            let rc = unsafe {
                crate::AdsCreateTable(
                    connect,
                    name.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    table_type,
                    1,
                    0,
                    1,
                    0,
                    c"ID,N,4,0".as_ptr().cast_mut().cast(),
                    &mut table,
                )
            };
            assert_eq!(rc, AE_SUCCESS);
            crate::AdsAppendRecord(table);
            crate::AdsWriteRecord(table);
            // x1 to x6 in the table's folder, the connection's (twice, as the
            // current directory is), its data folder (twice) and the table's.
            // Each is made beside the table, by its bare name (tflipd refuses
            // a full path to itself), and then moved to its folder.
            let folders = ["tables", "", "", "tables/data", "data", "data"];
            for x in 0..folders.len() {
                let bare = format!("x{}_{table_type}.{ext}", x + 1);
                let file = text(&bare);
                let mut order = 0;
                let rc = unsafe {
                    crate::AdsCreateIndex61(
                        table,
                        file.as_ptr().cast_mut().cast(),
                        c"K".as_ptr().cast_mut().cast(),
                        c"ID".as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        options,
                        512,
                        &mut order,
                    )
                };
                assert_eq!(rc, AE_SUCCESS, "{file:?}");
            }
            // Orders made by a bare name and by one with a folder: where
            // their files land, under the connection's folder (C).
            for row in want
                .iter()
                .filter(|row| row["type"] == type_name && row.get("create").is_some())
            {
                let name = row["create"].as_str().unwrap();
                let name_c = text(&name.replace(['\\', '/'], separator));
                let mut order = 0;
                let rc = unsafe {
                    crate::AdsCreateIndex61(
                        table,
                        name_c.as_ptr().cast_mut().cast(),
                        c"K".as_ptr().cast_mut().cast(),
                        c"ID".as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        options,
                        512,
                        &mut order,
                    )
                };
                let bare = name.rsplit(['\\', '/']).next().unwrap();
                let landed = ["tables", "data", ""]
                    .into_iter()
                    .find(|sub| folder.join(sub).join(bare).exists())
                    .map(|sub| match sub {
                        "" => format!("C\\{bare}"),
                        sub => format!("C\\{sub}\\{bare}"),
                    })
                    .unwrap_or_default();
                got.push(json!({"layout": "same", "type": type_name, "create": name,
                                "rc": rc, "file": landed}));
            }
            crate::AdsCloseAllIndexes(table);
            for (x, sub) in folders.iter().enumerate() {
                let bare = format!("x{}_{table_type}.{ext}", x + 1);
                let from = folder.join("tables").join(&bare);
                std::fs::rename(&from, folder.join(sub).join(&bare)).unwrap();
            }
            for row in want
                .iter()
                .filter(|row| row["type"] == type_name && row.get("name").is_some())
            {
                let name = row["name"].as_str().unwrap();
                // The probe's names, with this side's separator.
                let name_c = text(&name.replace(['\\', '/'], separator));
                let (mut orders, mut count) = ([0 as AdsHandle; 8], 8u16);
                let rc = unsafe {
                    crate::AdsOpenIndex(
                        table,
                        name_c.as_ptr().cast_mut().cast(),
                        orders.as_mut_ptr(),
                        &mut count,
                    )
                };
                if rc != 0 {
                    count = 0;
                }
                for &order in &orders[..usize::from(count)] {
                    crate::AdsCloseIndex(order);
                }
                got.push(json!({"layout": "same", "type": type_name, "name": name,
                                "rc": rc, "orders": count}));
            }
            crate::AdsCloseTable(table);
        }
        AdsDisconnect(connect);
        let problems: Vec<String> = want
            .iter()
            .zip(&got)
            .filter(|(want, got)| want != got)
            .map(|(want, got)| format!("want {want}\n got {got}"))
            .collect();
        assert_eq!(want.len(), got.len());
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// As in the original engine: `AdsAddCustomKey` and `AdsDeleteCustomKey`
    /// refuse an order that is not custom with 5017, and a Visual FoxPro
    /// table has no custom orders (5019) (`fixtures/errors/custom_kinds.json`,
    /// from `tools/ace/errors/custom_kinds.c`, whose steps this repeats).
    #[test]
    fn custom_key_order_kinds() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/errors/custom_kinds.json");
        let want: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, _dir) =
            fixtures::scratch_from("custom_key_order_kinds", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let kinds = [
            ("ADT", "adi", ADS_ADT, 2),
            ("NTX", "ntx", crate::ADS_NTX, 0),
            ("CDX", "cdx", crate::ADS_CDX, 2),
            ("CDX single", "idx", crate::ADS_CDX, 0),
            ("VFP", "cdx", crate::ADS_VFP, 2),
            ("VFP single", "idx", crate::ADS_VFP, 0),
        ];
        let mut got = Vec::new();
        for (k, (kind, ext, table_type, options)) in kinds.into_iter().enumerate() {
            for custom in 0..2u32 {
                let name = std::ffi::CString::new(format!("c{k}{custom}")).unwrap();
                let bag = std::ffi::CString::new(format!("c{k}{custom}.{ext}")).unwrap();
                let mut table = 0;
                let rc = unsafe {
                    crate::AdsCreateTable(
                        connect,
                        name.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        table_type,
                        1,
                        0,
                        1,
                        0,
                        c"ID,N,4,0".as_ptr().cast_mut().cast(),
                        &mut table,
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
                for _ in 0..2 {
                    crate::AdsAppendRecord(table);
                    crate::AdsWriteRecord(table);
                }
                let mut order = 0;
                let created = unsafe {
                    crate::AdsCreateIndex61(
                        table,
                        bag.as_ptr().cast_mut().cast(),
                        c"K".as_ptr().cast_mut().cast(),
                        c"ID".as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        options | if custom == 1 { 4 } else { 0 },
                        512,
                        &mut order,
                    )
                };
                crate::AdsGotoRecord(table, 2);
                let add = crate::AdsAddCustomKey(order);
                let delete = crate::AdsDeleteCustomKey(order);
                let on_table = crate::AdsAddCustomKey(table);
                got.push(json!({"kind": kind, "custom": custom, "create": created,
                                "add": add, "delete": delete, "table": on_table}));
                crate::AdsCloseTable(table);
            }
        }
        AdsDisconnect(connect);
        assert_eq!(want, got);
    }

    /// As in the original engine: a single-order file named as a FoxPro
    /// table's structural `.cdx` opens with `AdsOpenIndex` on a shared or
    /// read-only handle, and only an exclusive one refuses it with 7008
    /// (`fixtures/errors/stray_index_modes.json`, from
    /// `tools/ace/errors/stray_index_modes.c`, whose steps this repeats).
    #[test]
    fn stray_index_open_modes() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/errors/stray_index_modes.json");
        let want: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, _dir) =
            fixtures::scratch_from("stray_index_open_modes", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let mut got = Vec::new();
        for (kind, table_type) in [("CDX", crate::ADS_CDX), ("VFP", crate::ADS_VFP)] {
            for options in 0..3u32 {
                let name = std::ffi::CString::new(format!("s{table_type}_{options}")).unwrap();
                let bag = std::ffi::CString::new(format!("s{table_type}_{options}.cdx")).unwrap();
                let mut table = 0;
                let rc = unsafe {
                    crate::AdsCreateTable(
                        connect,
                        name.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        table_type,
                        1,
                        0,
                        1,
                        0,
                        c"ID,N,4,0".as_ptr().cast_mut().cast(),
                        &mut table,
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
                for id in 1..=2 {
                    crate::AdsAppendRecord(table);
                    unsafe {
                        crate::AdsSetDouble(table, c"ID".as_ptr().cast_mut().cast(), id as f64)
                    };
                    crate::AdsWriteRecord(table);
                }
                let mut order = 0;
                let rc = unsafe {
                    crate::AdsCreateIndex61(
                        table,
                        bag.as_ptr().cast_mut().cast(),
                        c"K".as_ptr().cast_mut().cast(),
                        c"ID".as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        0,
                        512,
                        &mut order,
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
                crate::AdsCloseTable(table);
                let open = unsafe {
                    crate::AdsOpenTable(
                        connect,
                        name.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        table_type,
                        1,
                        1,
                        1,
                        options,
                        &mut table,
                    )
                };
                let (mut n, mut after, mut count, mut again_count) = (0u16, 0u16, 8u16, 8u16);
                let (mut orders, mut again_orders) = ([0; 8], [0; 8]);
                let (mut index, mut again) = (0, 0);
                if open == AE_SUCCESS {
                    unsafe {
                        AdsGetNumIndexes(table, &mut n);
                        index = AdsOpenIndex(
                            table,
                            bag.as_ptr().cast_mut().cast(),
                            orders.as_mut_ptr(),
                            &mut count,
                        );
                        again = AdsOpenIndex(
                            table,
                            bag.as_ptr().cast_mut().cast(),
                            again_orders.as_mut_ptr(),
                            &mut again_count,
                        );
                        AdsGetNumIndexes(table, &mut after);
                    }
                    crate::AdsCloseTable(table);
                }
                got.push(
                    json!({"type": kind, "options": options, "open": open, "n": n,
                                "index": index, "orders": if index == 0 { count } else { 0 },
                                "again": again, "after": after}),
                );
            }
        }
        AdsDisconnect(connect);
        assert_eq!(want, got);
    }

    /// `s` as a C string.
    fn text(s: &str) -> std::ffi::CString {
        std::ffi::CString::new(s).unwrap()
    }

    /// Makes the table `name` of `fields`, with the arguments the probes of
    /// `tools/ace/errors` make their tables with.
    fn probe_table(
        connect: AdsHandle,
        name: &str,
        table_type: u16,
        fields: &std::ffi::CStr,
    ) -> AdsHandle {
        let name = std::ffi::CString::new(name).unwrap();
        let mut table = 0;
        let rc = unsafe {
            crate::AdsCreateTable(
                connect,
                name.as_ptr().cast_mut().cast(),
                std::ptr::null_mut(),
                table_type,
                1,
                0,
                1,
                0,
                fields.as_ptr().cast_mut().cast(),
                &mut table,
            )
        };
        assert_eq!(rc, AE_SUCCESS, "{name:?}");
        table
    }

    /// `AdsCreateIndex61` of ID into `bag` as the probes call it.
    fn probe_index(table: AdsHandle, bag: &str, tag: &str, options: u32) -> u32 {
        probe_index_handle(table, bag, tag, options).0
    }

    /// [`probe_index`] and the order's handle it gave.
    fn probe_index_handle(
        table: AdsHandle,
        bag: &str,
        tag: &str,
        options: u32,
    ) -> (u32, AdsHandle) {
        let (bag, tag) = (text(bag), text(tag));
        let mut order = 0;
        let rc = unsafe {
            crate::AdsCreateIndex61(
                table,
                bag.as_ptr().cast_mut().cast(),
                tag.as_ptr().cast_mut().cast(),
                c"ID".as_ptr().cast_mut().cast(),
                c"".as_ptr().cast_mut().cast(),
                c"".as_ptr().cast_mut().cast(),
                options,
                512,
                &mut order,
            )
        };
        (rc, order)
    }

    /// `AdsOpenTable` of `name` as the probes call it.
    fn probe_open(connect: AdsHandle, name: &str, table_type: u16, options: u32) -> AdsHandle {
        let name = text(name);
        let mut table = 0;
        unsafe {
            crate::AdsOpenTable(
                connect,
                name.as_ptr().cast_mut().cast(),
                std::ptr::null_mut(),
                table_type,
                1,
                0,
                1,
                options,
                &mut table,
            )
        };
        table
    }

    /// `AdsCreateFTSIndex` of NOTES into `file` with the default lists, as
    /// the probes call it.
    fn probe_fts(table: AdsHandle, file: &str) -> u32 {
        let file = text(file);
        let mut empty = [0u8; 2];
        let empty = empty.as_mut_ptr().cast();
        unsafe {
            crate::AdsCreateFTSIndex(
                table,
                file.as_ptr().cast_mut().cast(),
                c"NOTESFTS".as_ptr().cast_mut().cast(),
                c"NOTES".as_ptr().cast_mut().cast(),
                0,
                3,
                30,
                1,
                empty,
                1,
                empty,
                1,
                empty,
                1,
                empty,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        }
    }

    /// Index files named with an extension that is not their format's
    /// (`.fts`, `.xyz`) are made and opened again as in the original engine,
    /// a Clipper table refuses a full-text order with 5017
    /// (`fixtures/errors/index_extensions.json`, from
    /// `tools/ace/errors/index_extensions.c`, whose steps this repeats).
    #[test]
    fn index_file_extensions() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/errors/index_extensions.json");
        let want: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, _dir) =
            fixtures::scratch_from("index_file_extensions", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let notes = [
            (3.0, "Quick brown fox jumps over the lazy dog"),
            (1.0, "The lazy dog sleeps in the sun"),
            (2.0, "Quick fox returns to the den at dusk"),
        ];
        let mut got = Vec::new();
        for (kind, table_type) in [
            ("NTX", crate::ADS_NTX),
            ("CDX", crate::ADS_CDX),
            ("ADT", ADS_ADT),
            ("VFP", crate::ADS_VFP),
        ] {
            for (o, order) in ["fts", "index", "compound"].into_iter().enumerate() {
                if table_type == crate::ADS_NTX && o == 2 {
                    continue;
                }
                let name = format!("x{table_type}_{o}");
                let table = probe_table(connect, &name, table_type, c"ID,N,4,0;NOTES,C,40,0");
                for (id, note) in notes {
                    crate::AdsAppendRecord(table);
                    let note = text(note);
                    unsafe {
                        crate::AdsSetDouble(table, c"ID".as_ptr().cast_mut().cast(), id);
                        crate::AdsSetString(
                            table,
                            c"NOTES".as_ptr().cast_mut().cast(),
                            note.as_ptr().cast_mut().cast(),
                            note.as_bytes().len() as u32,
                        );
                    }
                    crate::AdsWriteRecord(table);
                }
                let file = format!("{name}.{}", if o == 0 { "fts" } else { "xyz" });
                let made = match o {
                    0 => probe_fts(table, &file),
                    _ => probe_index(table, &file, "K", if o == 2 { 2 } else { 0 }),
                };
                crate::AdsCloseTable(table);
                let table = probe_open(connect, &name, table_type, 1);
                let (mut opened, mut count) = ([0; 8], 8u16);
                let file_c = text(&file);
                let open = unsafe {
                    AdsOpenIndex(
                        table,
                        file_c.as_ptr().cast_mut().cast(),
                        opened.as_mut_ptr(),
                        &mut count,
                    )
                };
                if open != AE_SUCCESS && open != 5065 {
                    count = 0;
                }
                let found = match o {
                    0 => {
                        let mut found = 9;
                        let filter = c"CONTAINS(NOTES, 'quick')";
                        if unsafe { crate::AdsSetFilter(table, filter.as_ptr().cast_mut().cast()) }
                            == AE_SUCCESS
                        {
                            unsafe { crate::AdsGetRecordCount(table, 1, &mut found) };
                        }
                        json!(found)
                    }
                    _ => {
                        let mut records = Vec::new();
                        if count > 0 {
                            crate::AdsGotoTop(opened[0]);
                            for _ in 0..5 {
                                let (mut eof, mut record) = (0, 0);
                                unsafe {
                                    crate::AdsAtEOF(table, &mut eof);
                                }
                                if eof != 0 {
                                    break;
                                }
                                unsafe { crate::AdsGetRecordNum(table, 2, &mut record) };
                                records.push(record);
                                crate::AdsSkip(opened[0], 1);
                            }
                        }
                        json!(records)
                    }
                };
                got.push(
                    json!({"type": kind, "order": order, "create": made, "open": open,
                                "orders": count, "found": found}),
                );
                crate::AdsCloseTable(table);
            }
        }
        AdsDisconnect(connect);
        assert_eq!(want, got);
    }

    /// The records from `AdsGotoTop` and `AdsSkip` of `order` on, as the
    /// probes read them (none where there is no order).
    fn probe_records(table: AdsHandle, order: Option<AdsHandle>) -> Vec<u32> {
        let mut records = Vec::new();
        let Some(order) = order else {
            return records;
        };
        crate::AdsGotoTop(order);
        for _ in 0..5 {
            let (mut eof, mut record) = (1, 0);
            if unsafe { crate::AdsAtEOF(table, &mut eof) } != AE_SUCCESS || eof != 0 {
                break;
            }
            unsafe { crate::AdsGetRecordNum(table, 2, &mut record) };
            records.push(record);
            crate::AdsSkip(order, 1);
        }
        records
    }

    /// Index files named with the extension of another table type's format
    /// (`.ntx`, `.cdx`, `.idx`, `.adi`) are made of the table's format and
    /// read back by their header, as in the original engine
    /// (`fixtures/errors/foreign_extensions.json`, from
    /// `tools/ace/errors/foreign_extensions.c`, whose steps this repeats).
    #[test]
    fn foreign_index_file_extensions() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/errors/foreign_extensions.json");
        let want: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, _dir) =
            fixtures::scratch_from("foreign_index_file_extensions", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let mut got = Vec::new();
        for (kind, table_type) in [
            ("NTX", crate::ADS_NTX),
            ("CDX", crate::ADS_CDX),
            ("ADT", ADS_ADT),
            ("VFP", crate::ADS_VFP),
        ] {
            for (e, extension) in ["ntx", "cdx", "idx", "adi"].into_iter().enumerate() {
                for compound in 0..2u32 {
                    let name = format!("f{table_type}_{}", e as u32 * 2 + compound);
                    let file = format!("{name}.{extension}");
                    let table = probe_table(connect, &name, table_type, c"ID,N,4,0");
                    for id in [3.0, 1.0, 2.0] {
                        crate::AdsAppendRecord(table);
                        unsafe { crate::AdsSetDouble(table, c"ID".as_ptr().cast_mut().cast(), id) };
                        crate::AdsWriteRecord(table);
                    }
                    let (made, order) = probe_index_handle(table, &file, "K", compound * 2);
                    let records = probe_records(table, (made == AE_SUCCESS).then_some(order));
                    crate::AdsCloseTable(table);
                    let table = probe_open(connect, &name, table_type, 1);
                    let (mut opened, mut count) = ([0; 8], 8u16);
                    let file_c = text(&file);
                    let open = unsafe {
                        AdsOpenIndex(
                            table,
                            file_c.as_ptr().cast_mut().cast(),
                            opened.as_mut_ptr(),
                            &mut count,
                        )
                    };
                    if open != AE_SUCCESS && open != 5065 {
                        count = 0;
                    }
                    let reopened = probe_records(table, (count > 0).then_some(opened[0]));
                    got.push(
                        json!({"type": kind, "extension": extension, "compound": compound,
                                    "create": made, "records": records, "open": open,
                                    "orders": count, "reopened": reopened}),
                    );
                    crate::AdsCloseTable(table);
                }
            }
        }
        AdsDisconnect(connect);
        assert_eq!(want, got);
    }

    /// A compound and a single order made into the same file of a FoxPro or
    /// VFP table: the file there decides what the second one is, as in the
    /// original engine (`fixtures/errors/order_files.json`, from
    /// `tools/ace/errors/order_files.c`, whose steps this repeats).
    #[test]
    fn compound_and_single_orders_in_one_file() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/errors/order_files.json");
        let want: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, _dir) = fixtures::scratch_from(
            "compound_and_single_orders_in_one_file",
            &fixtures::empty_dir(),
        );
        let connect = fixtures::connect_to(&source);
        let mut got = Vec::new();
        let mut n = 0;
        for (kind, table_type) in [("CDX", crate::ADS_CDX), ("VFP", crate::ADS_VFP)] {
            for extension in ["cdx", "xyz"] {
                for way in 0..2 {
                    let name = format!("m{table_type}_{n}");
                    let bag = format!("p{table_type}_{n}.{extension}");
                    n += 1;
                    let table = probe_table(connect, &name, table_type, c"ID,N,4,0");
                    for id in [3.0, 2.0, 1.0] {
                        crate::AdsAppendRecord(table);
                        unsafe { crate::AdsSetDouble(table, c"ID".as_ptr().cast_mut().cast(), id) };
                        crate::AdsWriteRecord(table);
                    }
                    let a = probe_index(table, &bag, "K1", if way == 1 { 2 } else { 0 });
                    let b = probe_index(table, &bag, "K2", if way == 1 { 0 } else { 2 });
                    let mut orders = 0;
                    unsafe { AdsGetNumIndexes(table, &mut orders) };
                    crate::AdsCloseTable(table);
                    let table = probe_open(connect, &name, table_type, 0);
                    let (mut opened, mut count) = ([0; 8], 8u16);
                    let bag_c = text(&bag);
                    let reopen = unsafe {
                        AdsOpenIndex(
                            table,
                            bag_c.as_ptr().cast_mut().cast(),
                            opened.as_mut_ptr(),
                            &mut count,
                        )
                    };
                    if reopen != AE_SUCCESS && reopen != 5065 {
                        count = 0;
                    }
                    let names: Vec<String> = opened[..usize::from(count)]
                        .iter()
                        .map(|&order| {
                            let (mut buf, mut len) = ([0u8; 32], 32u16);
                            unsafe { AdsGetIndexName(order, buf.as_mut_ptr(), &mut len) };
                            String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned()
                        })
                        .collect();
                    let (first, second) = match way {
                        1 => ("compound", "single"),
                        _ => ("single", "compound"),
                    };
                    got.push(json!({"type": kind, "extension": extension, "first": first,
                                    "second": second, "rc": [a, b], "orders": orders,
                                    "reopen": reopen, "names": names}));
                    crate::AdsCloseTable(table);
                }
            }
        }
        AdsDisconnect(connect);
        assert_eq!(want, got);
    }

    /// What `AdsGetRecordCount` counts on a table's handle and on an
    /// order's: through an order with a condition, only the records it
    /// holds, as in the original engine (`fixtures/errors/order_counts.json`,
    /// from `tools/ace/errors/order_counts.c`, whose steps this repeats).
    #[test]
    fn record_counts_through_orders() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/errors/order_counts.json");
        let want: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, _dir) =
            fixtures::scratch_from("record_counts_through_orders", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let records = [
            ("PERA", "Y"),
            ("MANGO", "N"),
            ("UVA", "Y"),
            ("KIWI", "N"),
            ("FRESA", "Y"),
        ];
        let cases = ["plain", "for", "scope", "for scope", "filter", "moved"];
        let mut got = Vec::new();
        for (kind, ext, table_type, options) in [
            ("NTX", "ntx", crate::ADS_NTX, 0),
            ("CDX", "cdx", crate::ADS_CDX, 2),
            ("ADT", "adi", ADS_ADT, 2),
            ("VFP", "idx", crate::ADS_VFP, 0),
        ] {
            for (c, case) in cases.into_iter().enumerate() {
                let name = format!("n{table_type}_{c}");
                let table = probe_table(
                    connect,
                    &name,
                    table_type,
                    c"NAME,C,6,0;VAL,N,4,0;INC,C,1,0",
                );
                for (r, (record, inc)) in records.into_iter().enumerate() {
                    crate::AdsAppendRecord(table);
                    let (record, inc) = (text(record), text(inc));
                    unsafe {
                        crate::AdsSetString(
                            table,
                            c"NAME".as_ptr().cast_mut().cast(),
                            record.as_ptr().cast_mut().cast(),
                            record.as_bytes().len() as u32,
                        );
                        crate::AdsSetDouble(
                            table,
                            c"VAL".as_ptr().cast_mut().cast(),
                            10.0 * (r + 1) as f64,
                        );
                        crate::AdsSetString(
                            table,
                            c"INC".as_ptr().cast_mut().cast(),
                            inc.as_ptr().cast_mut().cast(),
                            1,
                        );
                    }
                    crate::AdsWriteRecord(table);
                }
                let bag = text(&format!("{name}_k.{ext}"));
                let condition = if c == 0 || c == 2 { c"" } else { c"VAL >= 20" };
                let mut order = 0;
                let rc = unsafe {
                    crate::AdsCreateIndex61(
                        table,
                        bag.as_ptr().cast_mut().cast(),
                        c"K".as_ptr().cast_mut().cast(),
                        c"STR(VAL,8)".as_ptr().cast_mut().cast(),
                        condition.as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        options,
                        512,
                        &mut order,
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
                if c == 2 || c == 3 {
                    unsafe {
                        crate::AdsSetScope(order, 1, c"      20".as_ptr().cast_mut().cast(), 8, 2);
                        crate::AdsSetScope(order, 2, c"      40".as_ptr().cast_mut().cast(), 8, 2);
                    }
                }
                if c == 4 {
                    unsafe { crate::AdsSetFilter(table, c"INC = 'Y'".as_ptr().cast_mut().cast()) };
                }
                if c == 5 {
                    crate::AdsGotoTop(order);
                }
                for (handle, which) in [(table, "table"), (order, "order")] {
                    for option in 1..=3u16 {
                        let mut count = 777;
                        let rc = unsafe { crate::AdsGetRecordCount(handle, option, &mut count) };
                        got.push(json!({"type": kind, "case": case, "handle": which,
                                        "option": option, "rc": rc, "count": count}));
                    }
                }
                crate::AdsCloseTable(table);
            }
        }
        AdsDisconnect(connect);
        assert_eq!(want, got);
    }

    crate::fixtures::remote_twins!(
        numeric_expression_keys,
        record_counts_through_orders,
        index_file_extensions,
        foreign_index_file_extensions,
        compound_and_single_orders_in_one_file,
        custom_key_order_kinds,
        stray_index_open_modes,
        relative_index_file_names,
        keyword_field_names,
        order_names,
        auto_open_index_files,
        qualified_field_names,
        dbf_orders_match_the_original_engine,
        index_collations,
        index_file_names,
        index_flags,
        key_lengths_and_types,
        orders_of_a_table,
        orders_by_number_and_expression,
        index_files_open_and_close,
        expression_key_types_and_seeks_match_the_engine
    );
}
