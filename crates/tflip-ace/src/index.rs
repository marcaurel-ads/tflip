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
/// `AE_AUTOOPEN_INDEX`, and a handle that is not an open order with
/// `AE_INVALID_INDEX_HANDLE`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsCloseIndex(index: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCloseIndex");
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

    crate::fixtures::remote_twins!(
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
