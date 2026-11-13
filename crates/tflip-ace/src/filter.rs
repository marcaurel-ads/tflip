//! Filters: Advantage Optimized Filters (AOF) and record filters
//! (`AdsSetFilter`), which hide the records they fail from moves and
//! counts. See [`tflip_engine::filter`].
//!
//! An AOF is optimized as the original engine optimizes it, with the index
//! orders open on the table, which `AdsGetAOFOptLevel` reports
//! ([`tflip_engine::filter`]).

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{LazyLock, Mutex, PoisonError};

use tflip_engine::filter::AofOptions;
use tflip_engine::handle::HandleKind;
use tflip_engine::table::Customize;

use crate::backend::{Backend, api};
use crate::ffi::{c_str, slice, wide_string, write_units};
use crate::last_error::fail;
use crate::{
    ADS_FALSE, ADS_TRUE, AE_INVALID_FILTER_OPTION, AE_INVALID_OPTION, AE_INVALID_POINTER,
    AE_INVALID_TABLE_HANDLE, AE_NO_FILTER, AE_SUCCESS, AdsHandle, Unsigned8, Unsigned16,
    Unsigned32, ansi, code, error_code, filter_error_code, write_cut,
};

/// `ADS_RESOLVE_IMMEDIATE`, `ADS_RESOLVE_DYNAMIC`: one of them is required.
const RESOLVE: Unsigned32 = 0x0001 | 0x0002;
/// The other `AdsSetAOF` options: `ADS_KEYSET_AOF`, `ADS_FIXED_AOF`,
/// `ADS_KEEP_AOF_PLAN`.
const AOF_OPTIONS: Unsigned32 = 0x0004 | 0x0008 | 0x0010;
/// `ADS_ENCODE_UTF16`.
const UTF16: Unsigned32 = 0x2000;
/// `ADS_ENCODE_UTF8`.
const UTF8: Unsigned32 = 0x4000;

/// An expression a client gave: as the engine takes it, and as the client
/// wrote it (what `AdsGetFilter` and `AdsGetAOF` give back).
pub(crate) struct Expression {
    pub(crate) engine: String,
    pub(crate) client: String,
}

/// The text clients gave filters (`false`) and AOFs (`true`) that the engine
/// holds in another form, by table: an OEM table's filter reads back as it
/// was set.
static CLIENT_TEXT: LazyLock<Mutex<HashMap<(AdsHandle, bool), String>>> =
    LazyLock::new(Mutex::default);

/// Notes what a client wrote as the filter (or AOF) just set on `table`, if
/// the engine holds it differently.
fn remember(table: AdsHandle, aof: bool, expression: &Expression) {
    let mut texts = CLIENT_TEXT.lock().unwrap_or_else(PoisonError::into_inner);
    if expression.engine == expression.client {
        texts.remove(&(table, aof));
    } else {
        texts.insert((table, aof), expression.client.clone());
    }
}

/// The filter (or AOF) of `table` that the engine gave back, `held`, as the
/// client wrote it.
fn client_text(table: AdsHandle, aof: bool, held: String) -> String {
    let texts = CLIENT_TEXT.lock().unwrap_or_else(PoisonError::into_inner);
    texts.get(&(table, aof)).cloned().unwrap_or(held)
}

/// Whether `table` is an OEM table (see [`tflip_engine::getters::oem_table`]).
pub(crate) fn is_oem(table: AdsHandle) -> bool {
    let backend = api(table);
    backend
        .table_info(table)
        .is_ok_and(|info| tflip_engine::getters::oem_table(&info.settings, &info.structure))
}

/// How a `100` function's text is encoded (`ADS_ENCODE_*`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    Ansi,
    Utf8,
    Utf16,
}

impl Encoding {
    /// The encoding the `ADS_ENCODE_*` bits of `options` name;
    /// `AE_INVALID_OPTION` for both.
    fn of(options: Unsigned32) -> Result<Encoding, Unsigned32> {
        match options & (UTF8 | UTF16) {
            0 => Ok(Encoding::Ansi),
            UTF8 => Ok(Encoding::Utf8),
            UTF16 => Ok(Encoding::Utf16),
            _ => Err(AE_INVALID_OPTION),
        }
    }

    /// The NUL-terminated text at `text` in this encoding; `None` for null.
    ///
    /// # Safety
    ///
    /// `text` must be null or point to NUL-terminated text in this encoding.
    pub(crate) unsafe fn read(self, text: *const c_void) -> Option<String> {
        if text.is_null() {
            return None;
        }
        Some(match self {
            Encoding::Ansi => {
                // SAFETY: not null, so NUL-terminated text, as the caller
                // promises.
                ansi::data_in(unsafe { c_str(text.cast()) }.unwrap_or_default())
            }
            // SAFETY: not null, so NUL-terminated, as the caller promises.
            _ => ansi::data_from_unicode(&unsafe { self.unicode(text) }),
        })
    }

    /// [`read`](Self::read) for an expression a client gives about `table`:
    /// against an OEM table the text is converted to the OEM code page, as
    /// the original engine does for the literals it compares with the
    /// table's text (and `AdsEvalStringExpr` returns).
    ///
    /// # Safety
    ///
    /// `text` must be null or point to NUL-terminated text in this encoding.
    pub(crate) unsafe fn read_expression(
        self,
        text: *const c_void,
        table: AdsHandle,
    ) -> Option<Expression> {
        // SAFETY: as the caller promises.
        let client = unsafe { self.read(text) }?;
        let engine = if !is_oem(table) {
            client.clone()
        } else if self == Encoding::Ansi {
            ansi::ansi_to_oem(&client)
        } else {
            // SAFETY: not null, as `read` found.
            ansi::unicode_to_oem_lossy(&unsafe { self.unicode(text) })
        };
        Some(Expression { engine, client })
    }

    /// The UTF-8 or UTF-16 text at `text`.
    ///
    /// # Safety
    ///
    /// `text` must point to NUL-terminated text in this encoding.
    unsafe fn unicode(self, text: *const c_void) -> String {
        if self == Encoding::Utf16 {
            // SAFETY: as the caller promises.
            return unsafe { wide_string(text.cast()) };
        }
        // SAFETY: as the caller promises.
        let bytes = unsafe { c_str(text.cast()) }.unwrap_or_default();
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// Writes `text` in this encoding to `buf` of `*len` characters
    /// (NUL-terminated), setting `*len` to its length, as the original
    /// engine does: what does not fit is copied as far as it fits, and
    /// `AE_INSUFFICIENT_BUFFER` is returned with the length needed
    /// including the NUL.
    ///
    /// # Safety
    ///
    /// `buf` must be null or hold `*len` characters; `len` must be null or
    /// valid for reads and writes.
    unsafe fn write(self, text: &str, buf: *mut c_void, len: *mut Unsigned32) -> Unsigned32 {
        if buf.is_null() || len.is_null() {
            return fail(AE_INVALID_POINTER);
        }
        let bytes = match self {
            Encoding::Utf16 => {
                let units: Vec<u16> = ansi::data_unicode(text).encode_utf16().collect();
                // SAFETY: neither is null, so `buf` holds `*len` units and
                // `len` is readable and writable, as the caller promises.
                return unsafe { write_units(&units, buf.cast(), len) };
            }
            Encoding::Utf8 => ansi::data_unicode(text).into_bytes(),
            Encoding::Ansi => ansi::data_out(text),
        };
        // SAFETY: neither is null, so `buf` holds `*len` bytes and `len` is
        // readable and writable, as the caller promises.
        unsafe { write_units(&bytes, buf.cast(), len) }
    }
}

/// The table behind `table`. As in the original engine, any other handle
/// (an index order or a connection among them) is `AE_INVALID_TABLE_HANDLE`.
pub(crate) fn table_of(table: AdsHandle) -> Result<Backend, Unsigned32> {
    let backend = api(table);
    match backend.handle_kind(table) {
        Ok(Some(HandleKind::Table | HandleKind::Cursor)) => Ok(backend),
        Err(error) => Err(error_code(error)),
        // A value that is no handle: the last error says so.
        Ok(None) => {
            let detail = crate::last_error::NOT_A_HANDLE;
            crate::last_error::fail_handle_detail(crate::AE_INVALID_HANDLE, detail);
            Err(AE_INVALID_TABLE_HANDLE)
        }
        Ok(Some(_)) => Err(crate::last_error::fail_handle_detail(
            AE_INVALID_TABLE_HANDLE,
            crate::last_error::NOT_A_TABLE,
        )),
    }
}

/// Checks `AdsSetAOF` options: exactly one of `ADS_RESOLVE_IMMEDIATE` and
/// `ADS_RESOLVE_DYNAMIC`, and the other AOF options; `extra` are the other
/// bits allowed.
fn check_options(options: Unsigned32, extra: Unsigned32) -> Result<(), Unsigned32> {
    let resolve = options & RESOLVE;
    if resolve == 0 || resolve == RESOLVE || options & !(RESOLVE | AOF_OPTIONS | extra) != 0 {
        return Err(AE_INVALID_OPTION);
    }
    Ok(())
}

/// Sets the filter of `table` to `expression`; `None` for a null pointer.
fn set_aof(table: AdsHandle, expression: Option<Expression>, options: Unsigned32) -> Unsigned32 {
    let opened = match table_of(table) {
        Ok(opened) => opened,
        Err(code) => return code,
    };
    let Some(expression) = expression else {
        return crate::last_error::null_param(2);
    };
    let options = AofOptions {
        immediate: options & 0x0001 != 0,
        fixed: options & 0x0008 != 0,
    };
    match opened.set_aof(table, expression.engine.clone(), options) {
        Ok(()) => {
            remember(table, true, &expression);
            AE_SUCCESS
        }
        Err(error) => {
            crate::log::err(format_args!("filter {:?}", expression.client));
            filter_error_code(error)
        }
    }
}

/// `AdsSetAOF`: sets a table's Advantage Optimized Filter, the expression
/// `filter`; `options` must hold one of `ADS_RESOLVE_IMMEDIATE` and
/// `ADS_RESOLVE_DYNAMIC`, and may add `ADS_KEYSET_AOF` (as the default,
/// `ADS_DYNAMIC_AOF`, in a local server), `ADS_FIXED_AOF` and
/// `ADS_KEEP_AOF_PLAN` (else `AE_INVALID_OPTION`); see
/// [`tflip_engine::filter`] for what they change. An empty expression fails
/// with `AE_INVALID_EXPRESSION`, one that does not compile with the code of
/// its error (3101 to 3125), one that is not logical with
/// `AOF_NOT_LOGICAL`; a handle that is not a table with
/// `AE_INVALID_TABLE_HANDLE`. The table does not move.
///
/// # Safety
///
/// `filter` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetAOF(
    table: AdsHandle,
    filter: *mut Unsigned8,
    options: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetAOF");
    let options = Unsigned32::from(options);
    if filter.is_null() {
        return crate::last_error::null_param(2);
    }
    if let Err(code) = check_options(options, 0) {
        return crate::last_error::fail_detail(code, "Parameter 3");
    }
    // SAFETY: `filter` is null or a NUL-terminated string, as # Safety says.
    let filter = unsafe { Encoding::Ansi.read_expression(filter.cast(), table) };
    set_aof(table, filter, options)
}

/// `AdsSetAOF100`: [`AdsSetAOF`] with the filter in ANSI, UTF-8
/// (`ADS_ENCODE_UTF8`) or UTF-16 (`ADS_ENCODE_UTF16`).
///
/// # Safety
///
/// `filter` must be null or point to NUL-terminated text in the encoding
/// `options` names.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetAOF100(
    table: AdsHandle,
    filter: *mut c_void,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetAOF100");
    let encoding = match check_options(options, UTF8 | UTF16).and_then(|()| Encoding::of(options)) {
        Ok(encoding) => encoding,
        Err(code) => return crate::last_error::fail_detail(code, "Parameter 3"),
    };
    // SAFETY: `filter` is null or NUL-terminated text in `encoding`, as
    // # Safety says.
    let filter = unsafe { encoding.read_expression(filter, table) };
    set_aof(table, filter, options)
}

/// Reports the optimization level of a filter that is not set, with the
/// index orders open on the table; `None` for a null pointer.
fn eval_aof(
    table: AdsHandle,
    filter: Option<Expression>,
    level: Option<&mut Unsigned16>,
    level_position: u32,
) -> Unsigned32 {
    if let Err(code) = table_of(table) {
        return code;
    }
    let Some(expression) = filter else {
        return crate::last_error::null_param(2);
    };
    let Some(level) = level else {
        return crate::last_error::null_param(level_position);
    };
    match api(table).eval_aof(table, expression.engine) {
        Ok(optimized) => {
            *level = optimized;
            AE_SUCCESS
        }
        // As the original engine does, the level is written anyway.
        Err(error) => {
            *level = 3;
            filter_error_code(error)
        }
    }
}

/// `AdsEvalAOF`: the optimization level a filter would have, without
/// setting it (`ADS_OPTIMIZED_FULL`, `ADS_OPTIMIZED_PART` or
/// `ADS_OPTIMIZED_NONE`).
///
/// # Safety
///
/// `filter` must be null or point to a NUL-terminated string; `level` must
/// be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsEvalAOF(
    table: AdsHandle,
    filter: *mut Unsigned8,
    level: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsEvalAOF");
    // SAFETY: `filter` is null or a NUL-terminated string, `level` null or
    // writable, as # Safety says.
    let (filter, level) = unsafe {
        (
            Encoding::Ansi.read_expression(filter.cast(), table),
            level.as_mut(),
        )
    };
    eval_aof(table, filter, level, 3)
}

/// `AdsEvalAOF100`: [`AdsEvalAOF`] with the filter in ANSI, UTF-8 or UTF-16
/// (`ADS_ENCODE_*` in `options`).
///
/// # Safety
///
/// `filter` must be null or point to NUL-terminated text in the encoding
/// `options` names; `level` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsEvalAOF100(
    table: AdsHandle,
    filter: *mut c_void,
    options: Unsigned32,
    level: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsEvalAOF100");
    let encoding = match Encoding::of(options) {
        Ok(encoding) => encoding,
        Err(code) => return fail(code),
    };
    // SAFETY: `filter` is null or NUL-terminated text in `encoding`, `level`
    // null or writable, as # Safety says.
    let (filter, level) = unsafe { (encoding.read_expression(filter, table), level.as_mut()) };
    eval_aof(table, filter, level, 4)
}

/// `AdsClearAOF`: removes a table's Advantage Optimized Filter. A table
/// without one fails with `AE_NO_FILTER`, as in the original engine.
#[unsafe(no_mangle)]
pub extern "system" fn AdsClearAOF(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsClearAOF");
    match table_of(table).map(|opened| opened.clear_aof(table)) {
        Ok(Ok(())) => AE_SUCCESS,
        Ok(Err(error)) => error_code(error),
        Err(code) => code,
    }
}

/// `AdsRefreshAOF`: resolves a table's filter anew, as when it was set,
/// dropping its customization (`AdsCustomizeAOF`) and counting its records
/// again; a table without a filter fails with `AE_NO_FILTER`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsRefreshAOF(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsRefreshAOF");
    let refreshed = |opened: Backend| opened.aof(table).and_then(|_| opened.refresh_aof(table));
    match table_of(table).map(refreshed) {
        Ok(Ok(_)) => AE_SUCCESS,
        Ok(Err(error)) => error_code(error),
        Err(code) => code,
    }
}

/// `AdsGetAOF`: a table's filter, with the field names as the table defines
/// them, in `filter` of `*len` bytes (NUL-terminated); `*len` is set to its
/// length. As in the original engine, a buffer that is too small gets what
/// fits and `AE_INSUFFICIENT_BUFFER` with the length needed including the
/// NUL, and a table without a filter fails with `AE_NO_FILTER` and `*len` set
/// to 0.
///
/// # Safety
///
/// `filter` must be null or hold `*len` bytes; `len` must be null or valid
/// for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetAOF(
    table: AdsHandle,
    filter: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetAOF");
    if filter.is_null() {
        return crate::last_error::null_param(2);
    }
    if len.is_null() {
        // The original engine does nothing, and leaves the last error.
        crate::last_error::restore_pending();
        return AE_SUCCESS;
    }
    match table_of(table).map(|opened| opened.aof(table)) {
        // SAFETY: `filter` and `len` are as # Safety says.
        Ok(Ok(aof)) => unsafe {
            write_cut(&ansi::data_out(&client_text(table, true, aof)), filter, len)
        },
        Ok(Err(error)) => {
            if !len.is_null() {
                // SAFETY: not null, so writable, as # Safety says.
                unsafe { *len = 0 };
            }
            error_code(error)
        }
        Err(code) => code,
    }
}

/// `AdsGetAOF100`: [`AdsGetAOF`] in ANSI, UTF-8 or UTF-16 (`ADS_ENCODE_*` in
/// `options`); `*len` counts characters (UTF-16 code units for UTF-16).
///
/// # Safety
///
/// `filter` must be null or hold `*len` characters; `len` must be null or
/// valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetAOF100(
    table: AdsHandle,
    options: Unsigned32,
    filter: *mut c_void,
    len: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetAOF100");
    let encoding = match Encoding::of(options) {
        Ok(encoding) if options & !(UTF8 | UTF16) == 0 => encoding,
        _ => return fail(AE_INVALID_OPTION),
    };
    match table_of(table).map(|opened| opened.aof(table)) {
        // SAFETY: `filter` and `len` are as # Safety says.
        Ok(Ok(aof)) => unsafe { encoding.write(&client_text(table, true, aof), filter, len) },
        Ok(Err(error)) => error_code(error),
        Err(code) => code,
    }
}

/// `AdsGetAOFOptLevel`: how far a table's filter is optimized with indexes,
/// and its part that is not, as the original engine prints it, in `non_opt`
/// of `*len` bytes (which may be null for the level alone). A table without
/// a filter fails with `AE_NO_FILTER`.
///
/// # Safety
///
/// `level` must be null or valid for writes; `non_opt` must be null or hold
/// `*len` bytes; `len` must be null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetAOFOptLevel(
    table: AdsHandle,
    level: *mut Unsigned16,
    non_opt: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetAOFOptLevel");
    if level.is_null() {
        return crate::last_error::null_param(2);
    }
    if !non_opt.is_null() && len.is_null() {
        return crate::last_error::null_param(4);
    }
    let (optimized, residual) = match table_of(table).map(|opened| opened.aof_level(table)) {
        Ok(Ok(level)) => level,
        Ok(Err(error)) => return error_code(error),
        Err(code) => return code,
    };
    if level.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *level = optimized };
    if non_opt.is_null() {
        return AE_SUCCESS;
    }
    // SAFETY: `non_opt` and `len` are as # Safety says.
    unsafe { write_cut(&ansi::data_out(&residual), non_opt, len) }
}

/// `AdsGetAOFOptLevel100`: [`AdsGetAOFOptLevel`] in ANSI, UTF-8 or UTF-16
/// (`ADS_ENCODE_*` in `options`).
///
/// # Safety
///
/// `level` must be null or valid for writes; `non_opt` must be null or hold
/// `*len` characters; `len` must be null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetAOFOptLevel100(
    table: AdsHandle,
    level: *mut Unsigned16,
    non_opt: *mut c_void,
    len: *mut Unsigned32,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetAOFOptLevel100");
    let encoding = match Encoding::of(options) {
        Ok(encoding) => encoding,
        Err(code) => return fail(code),
    };
    let (optimized, residual) = match table_of(table).map(|opened| opened.aof_level(table)) {
        Ok(Ok(level)) => level,
        Ok(Err(error)) => return error_code(error),
        Err(code) => return code,
    };
    if level.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *level = optimized };
    if non_opt.is_null() {
        return AE_SUCCESS;
    }
    // SAFETY: `non_opt` and `len` are as # Safety says.
    unsafe { encoding.write(&residual, non_opt, len) }
}

/// `AdsIsRecordInAOF`: whether record `record` passes a table's filter,
/// its customization included; a record that does not exist or is deleted
/// does not. A table without a filter fails with `AE_NO_FILTER`.
///
/// # Safety
///
/// `is_in` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsRecordInAOF(
    table: AdsHandle,
    record: Unsigned32,
    is_in: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsRecordInAOF");
    if is_in.is_null() {
        return crate::last_error::null_param(3);
    }
    let passes = match table_of(table).map(|opened| opened.is_in_aof(table, record)) {
        Ok(Ok(passes)) => passes,
        Ok(Err(error)) => return error_code(error),
        Err(code) => return code,
    };
    let value = if passes { ADS_TRUE } else { ADS_FALSE };
    // SAFETY: `is_in` is null or writable, as # Safety says.
    unsafe { crate::field::write(Ok(value), is_in) }
}

/// `AdsCustomizeAOF`: adds records to a table's filter, removes them or
/// toggles them (`ADS_AOF_ADD_RECORD`, `ADS_AOF_REMOVE_RECORD`,
/// `ADS_AOF_TOGGLE_RECORD`; else `AE_INVALID_FILTER_OPTION`), whatever its
/// expression says of them, until the filter is cleared or refreshed. A
/// table without a filter fails with `AE_NO_FILTER`.
///
/// # Safety
///
/// `records` must be null or hold `count` record numbers.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCustomizeAOF(
    table: AdsHandle,
    count: Unsigned32,
    records: *mut Unsigned32,
    option: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCustomizeAOF");
    match table_of(table).map(|opened| opened.aof(table)) {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => return error_code(error),
        Err(code) => return code,
    }
    let how = match option {
        1 => Customize::Add,
        2 => Customize::Remove,
        3 => Customize::Toggle,
        _ => return fail(AE_INVALID_FILTER_OPTION),
    };
    let records = match count {
        0 => Vec::new(),
        // SAFETY: `records` is null or holds `count` numbers, as # Safety
        // says.
        _ => match unsafe { slice(records, count as usize) } {
            Some(records) => records.to_vec(),
            None => return crate::last_error::null_param(3),
        },
    };
    code(api(table).customize_aof(table, records, how))
}

/// Sets the record filter of `table` to `expression`; `None` for a null
/// pointer.
fn set_filter(table: AdsHandle, expression: Option<Expression>) -> Unsigned32 {
    let opened = match table_of(table) {
        Ok(opened) => opened,
        Err(code) => return code,
    };
    let Some(expression) = expression else {
        return crate::last_error::null_param(2);
    };
    match opened.set_filter(table, expression.engine.clone()) {
        Ok(()) => {
            remember(table, false, &expression);
            AE_SUCCESS
        }
        Err(error) => {
            crate::log::err(format_args!("filter {:?}", expression.client));
            filter_error_code(error)
        }
    }
}

/// `AdsSetFilter`: sets a table's record filter, replacing any it had: the
/// records it does not hold for are hidden from moves and counts. As in the
/// original engine, the expression need not be logical (a number holds
/// unless it is 0, other values unless they are NULL), and the table does
/// not move. An empty expression fails with `AE_INVALID_EXPRESSION`, one
/// that does not compile with the code of its error (3101 to 3125).
///
/// # Safety
///
/// `filter` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetFilter(table: AdsHandle, filter: *mut Unsigned8) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetFilter");
    // SAFETY: `filter` is null or a NUL-terminated string, as # Safety says.
    let filter = unsafe { Encoding::Ansi.read_expression(filter.cast(), table) };
    set_filter(table, filter)
}

/// `AdsSetFilter100`: [`AdsSetFilter`] with the filter in ANSI, UTF-8
/// (`ADS_ENCODE_UTF8`) or UTF-16 (`ADS_ENCODE_UTF16`).
///
/// # Safety
///
/// `filter` must be null or point to NUL-terminated text in the encoding
/// `options` names.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetFilter100(
    table: AdsHandle,
    filter: *mut c_void,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetFilter100");
    let encoding = match Encoding::of(options) {
        Ok(encoding) => encoding,
        Err(code) => return fail(code),
    };
    // SAFETY: `filter` is null or NUL-terminated text in `encoding`, as
    // # Safety says.
    let filter = unsafe { encoding.read_expression(filter, table) };
    set_filter(table, filter)
}

/// `AdsGetFilter`: a table's record filter, as it was set, in `filter` of
/// `*len` bytes (NUL-terminated); `*len` is set to its length. As
/// [`AdsGetAOF`]: a buffer that is too small gets what fits and
/// `AE_INSUFFICIENT_BUFFER`, and a table without a filter fails with
/// `AE_NO_FILTER`.
///
/// # Safety
///
/// `filter` must be null or hold `*len` bytes; `len` must be null or valid
/// for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetFilter(
    table: AdsHandle,
    filter: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetFilter");
    if let Some(code) = crate::ffi::null_buffer(filter, len, 2) {
        return code;
    }
    match table_of(table).map(|opened| opened.filter(table)) {
        // SAFETY: `filter` and `len` are as # Safety says.
        Ok(Ok(text)) => unsafe {
            write_cut(
                &ansi::data_out(&client_text(table, false, text)),
                filter,
                len,
            )
        },
        Ok(Err(error)) => {
            if !len.is_null() {
                // SAFETY: not null, so writable, as # Safety says.
                unsafe { *len = 0 };
            }
            match error {
                tflip_engine::Error::NoFilter => fail(AE_NO_FILTER),
                error => error_code(error),
            }
        }
        Err(code) => code,
    }
}

/// `AdsClearFilter`: removes a table's record filter. A table without one
/// fails with `AE_NO_FILTER`, as in the original engine.
#[unsafe(no_mangle)]
pub extern "system" fn AdsClearFilter(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsClearFilter");
    match table_of(table).map(|opened| opened.clear_filter(table)) {
        Ok(Ok(())) => AE_SUCCESS,
        Ok(Err(tflip_engine::Error::NoFilter)) => fail(AE_NO_FILTER),
        Ok(Err(error)) => error_code(error),
        Err(code) => code,
    }
}

/// `AdsDeactivateAOF` (unpublished): sets a table's AOF aside, so that it
/// hides no records, until `AdsActivateAOF`; `AdsIsRecordInAOF` still
/// answers. A table without an AOF fails with `AE_NO_FILTER`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsDeactivateAOF(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDeactivateAOF");
    set_aof_active(table, false)
}

/// `AdsActivateAOF` (unpublished): puts back the AOF `AdsDeactivateAOF`
/// set aside; an active one stays.
#[unsafe(no_mangle)]
pub extern "system" fn AdsActivateAOF(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsActivateAOF");
    set_aof_active(table, true)
}

fn set_aof_active(table: AdsHandle, active: bool) -> Unsigned32 {
    let backend = match table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    code(backend.set_aof_active(table, active))
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use super::*;
    use crate::{
        ADS_ADT, ADS_CDX, ADS_VFP, AE_INSUFFICIENT_BUFFER, AE_INVALID_EXPRESSION, AE_NO_FILTER,
        Unsigned16, fixtures,
    };

    /// The arguments the filter functions check and what they report, as in
    /// the original engine (but for applying the filter).
    #[test]
    fn filters_are_kept() {
        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        let set = |expression: &CStr, options| unsafe {
            AdsSetAOF(table, expression.as_ptr().cast_mut().cast(), options)
        };
        let get = |size| {
            let mut buf = [b'#'; 16];
            let mut len = size;
            let rc = unsafe { AdsGetAOF(table, buf.as_mut_ptr(), &mut len) };
            (rc, len, buf)
        };
        let (rc, len, _) = get(16);
        assert_eq!((rc, len), (AE_NO_FILTER, 0), "the length is set to 0");
        assert_eq!(AdsClearAOF(table), AE_NO_FILTER);
        assert_eq!(AdsRefreshAOF(table), AE_NO_FILTER);

        // One of the two resolve options is required.
        for options in [0, 3, 0x20, 0x101] {
            assert_eq!(set(c"id > 2", options), AE_INVALID_OPTION, "{options:#x}");
        }
        assert_eq!(set(c"", 1), AE_INVALID_EXPRESSION);
        assert_eq!(set(c"ID > 2", 0x11), AE_SUCCESS);
        let (rc, len, buf) = get(16);
        assert_eq!(
            (rc, len, &buf[..7]),
            (AE_SUCCESS, 6, &b"id > 2\0"[..]),
            "as the table names it"
        );
        let (rc, len, buf) = get(3);
        assert_eq!(
            (rc, len, &buf[..4]),
            (AE_INSUFFICIENT_BUFFER, 7, &b"id #"[..])
        );

        // The 100 variants, in UTF-16.
        let wide: Vec<u16> = "id > 1\0".encode_utf16().collect();
        let rc = unsafe { AdsSetAOF100(table, wide.as_ptr().cast_mut().cast(), UTF16 | 2) };
        assert_eq!(rc, AE_SUCCESS);
        let rc = unsafe { AdsSetAOF100(table, wide.as_ptr().cast_mut().cast(), UTF16 | UTF8 | 1) };
        assert_eq!(rc, AE_INVALID_OPTION);
        let mut buf = [0u16; 8];
        let mut len = buf.len() as u32;
        let rc = unsafe { AdsGetAOF100(table, UTF16, buf.as_mut_ptr().cast(), &mut len) };
        assert_eq!((rc, len, &buf[..7]), (AE_SUCCESS, 6, &wide[..]));
        let mut len = 4;
        let rc = unsafe { AdsGetAOF100(table, UTF16, buf.as_mut_ptr().cast(), &mut len) };
        assert_eq!((rc, len), (AE_INSUFFICIENT_BUFFER, 7));

        // `id` has an order (PK_INDEXED), `active` none.
        let (mut level, mut non_opt, mut len) = (0, [0u8; 16], 16);
        let rc = unsafe { AdsGetAOFOptLevel(table, &mut level, non_opt.as_mut_ptr(), &mut len) };
        assert_eq!(
            (rc, level, &non_opt[..len as usize]),
            (AE_SUCCESS, 1, &b""[..])
        );
        let eval = |expression: &CStr| {
            let mut level = 0;
            let rc =
                unsafe { AdsEvalAOF(table, expression.as_ptr().cast_mut().cast(), &mut level) };
            (rc, level)
        };
        assert_eq!(eval(c"active"), (AE_SUCCESS, 3));
        assert_eq!(eval(c"id = 1 .AND. active"), (AE_SUCCESS, 2));
        assert_eq!(eval(c"UPPER(name) = 'X'"), (AE_SUCCESS, 3));
        assert_eq!(
            eval(c"name = 'X' .OR. born = CTOD('01/01/2000')"),
            (AE_SUCCESS, 1)
        );
        assert_eq!(set(c"id > 1 .AND. active", 2), AE_SUCCESS);
        let mut len = 16;
        let rc = unsafe { AdsGetAOFOptLevel(table, &mut level, non_opt.as_mut_ptr(), &mut len) };
        assert_eq!(
            (rc, level, &non_opt[..len as usize]),
            (AE_SUCCESS, 2, &b"active"[..])
        );

        // Every record that exists and is not deleted passes (slot 4 is
        // deleted).
        let passes = |record| {
            let mut is_in = 9;
            unsafe { AdsIsRecordInAOF(table, record, &mut is_in) };
            is_in
        };
        assert_eq!([0, 1, 4, 5, 6].map(passes), [0, 1, 0, 1, 0]);
        let mut records = [1];
        let rc = unsafe { AdsCustomizeAOF(table, 1, records.as_mut_ptr(), 9) };
        assert_eq!(rc, AE_INVALID_FILTER_OPTION);
        assert_eq!(
            unsafe { AdsCustomizeAOF(table, 1, records.as_mut_ptr(), 1) },
            AE_SUCCESS
        );

        assert_eq!(AdsRefreshAOF(table), AE_SUCCESS);
        assert_eq!(AdsClearAOF(connect), AE_INVALID_TABLE_HANDLE);
        assert_eq!(AdsClearAOF(table), AE_SUCCESS);
        assert_eq!(AdsClearAOF(table), AE_NO_FILTER);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    /// An AOF shows what the same expression shows as a record filter,
    /// whether its records are found through the orders (of every key type,
    /// descending, on a key's first segment) or by evaluating it, resolved
    /// at once or as moves reach records, fixed or not, with the exact
    /// setting off or on; and counts them.
    #[test]
    fn aofs_find_what_filters_do() {
        use crate::AdsGetRecordCount;
        use crate::AdsSetExact22;
        use crate::{ADS_IGNOREFILTERS, ADS_RESPECTFILTERS};
        use crate::{AdsAtEOF, AdsGetRecordNum, AdsGotoTop, AdsSkip};

        let _settings = fixtures::settings_lock();
        let rows = |table| {
            let mut rows = Vec::new();
            assert_eq!(AdsGotoTop(table), AE_SUCCESS);
            loop {
                let (mut eof, mut recno) = (0, 0);
                unsafe { AdsAtEOF(table, &mut eof) };
                if eof != 0 {
                    return rows;
                }
                unsafe { AdsGetRecordNum(table, ADS_IGNOREFILTERS, &mut recno) };
                rows.push(recno);
                AdsSkip(table, 1);
            }
        };
        let keys = [
            "k_char = 'a'",
            "k_char = 'abc'",
            "k_char == 'abc'",
            "k_char > 'Z'",
            "k_char >= 'a'",
            "k_char < 'a'",
            "k_char <= 'Zz'",
            "k_char = ''",
            "k_char = ' '",
            "k_char = 'abcdefgh'",
            "k_integer = 1",
            "k_integer > 0",
            "k_integer >= -1",
            "k_integer < 0",
            "k_integer <= 1.5",
            "k_integer = 1.5",
            "k_integer <> 1",
            "k_integer > 2147483647",
            "k_short = -1",
            "k_longint > 1",
            "k_double = 1.5",
            "k_double > 1",
            "k_double < -1000000",
            "k_curdouble <= 0",
            "k_money = 1.5",
            "k_money > 1.4999",
            "k_numeric = 1.5",
            "k_numeric > 9999",
            "k_numeric >= -999.99",
            "k_date = STOD('20171108')",
            "k_date > STOD('19000101')",
            "k_date <= CTOD('10/15/1582')",
            "k_date < CTOD('')",
            "k_logical",
            "!k_logical",
            "k_logical = .F.",
            "EMPTY(k_char)",
            "ISNULL(k_integer)",
            "YEAR(k_date) = 2017",
            "UPPER(k_char) = 'ABC'",
            "DTOS(k_date) = '20171108'",
            "k_cichar = 'abc'",
            "k_integer = 1 .OR. k_char = 'a'",
            "k_integer > 0 .AND. k_double < 2",
            ".NOT. (k_integer = 1)",
            "RECNO() > 2 .AND. k_integer > -2",
            "k_integer > 0 .AND. RECNO() < 4",
            "k_integer = 1 .OR. RECNO() = 6",
            ".T.",
            ".F.",
            "k_integer = 3 .AND. k_char = 'a'",
            "(k_integer < 0 .OR. k_double > 1) .AND. k_logical",
        ];
        let indexed = [
            "amount > 20",
            "amount = 10.25",
            "amount < 0",
            "amount <= 30.5",
            "id = 3",
            "id >= 2 .AND. amount < 40",
            "born > CTOD('01/01/1980')",
            "YEAR(born) = 1990",
            "name = 'B'",
            "name > 'a'",
            "ciname = 'ALPHA'",
            "ciname > 'bravo'",
            "!(id = 1)",
        ];
        let expressions = [
            "UPPER(k_char) > 'A'",
            "UPPER(k_char) <= 'ABC'",
            "DTOS(k_date) > '2000'",
            "DTOS(k_date) = ''",
            "DTOS(k_date) < '18'",
            "k_char + k_varchar = 'ab'",
            "k_char + k_varchar >= 'Z'",
            "STR(k_integer, 11) > '          0'",
            "STR(k_integer, 11) = ' '",
            "k_cichar >= 'ABC'",
            "k_cichar < 'B'",
            "k_varchar = 'a'",
            "k_varchar > 'Z'",
        ];
        let dbf_keys = [
            "K_CHAR = 'a'",
            "K_CHAR == 'abc'",
            "K_CHAR > 'Z'",
            "K_CHAR < 'b'",
            "K_CHAR = ''",
            "K_NUM > 0",
            "K_NUM = 1.5",
            "K_NUM <= -1.5",
            "K_DATE > STOD('19000101')",
            "K_DATE = STOD('20171108')",
            "K_DATE < CTOD('')",
            "K_LOG",
            "!K_LOG",
            "K_INT > 0",
            "K_INT < 0",
            "K_DBL >= 1.5",
            "K_DBL < 0",
            "K_CHAR + DTOS(K_DATE) = 'abc'",
            "K_CHAR + DTOS(K_DATE) > 'Zz'",
            "K_INT = 1 .OR. K_CHAR = 'a'",
            "K_NUM > 0 .AND. K_CHAR >= 'a'",
        ];
        let vfp_keys = [
            "k_char = 'abc'",
            "k_char < 'b'",
            "k_char >= ' '",
            "k_vchar = 'Zz'",
            "k_vchar >= 'a'",
            "k_vchar = ''",
            "k_num > 0",
            "k_int < 0",
            "k_int >= 0",
            "k_dbl = 1.5",
            "k_money > 1",
            "k_money <= 0",
            "k_date > STOD('19000101')",
            "k_date = CTOD('')",
            "k_log",
            "k_auto > 2",
            "ISNULL(k_char)",
        ];
        let options = [
            "UPPER(name) = 'N1'",
            "UPPER(name) > 'N3'",
            "UPPER(name) < 'N2'",
            "UPPER(name) == 'N12'",
            "name + DTOS(born) + STR(id, 5) = 'n1'",
            "name + DTOS(born) + STR(id, 5) < 'n2 '",
            "id > 35",
            "id = 3",
            "id <= 2",
            "name = 'n2'",
            "flag",
            "DELETED()",
            "!DELETED() .AND. id < 5",
        ];
        let grown = [
            "ID > 2990",
            "ID = 17 .OR. WORD = 'a'",
            "WORD < 'b'",
            "GRP = 3 .AND. ID < 100",
        ];
        let tables: [(&str, &CStr, Unsigned16, &[&str]); 8] = [
            ("adi_keys", c"keys.adt", ADS_ADT, &keys),
            ("adi_keys", c"keys.adt", ADS_ADT, &expressions),
            ("indexes", c"indexed.adt", ADS_ADT, &indexed),
            ("cdx/dbf_keys", c"dbf_keys.dbf", ADS_CDX, &dbf_keys),
            ("vfp/vfp_keys", c"vfp_keys.dbf", ADS_VFP, &vfp_keys),
            ("cdx/cdx_options", c"opts.dbf", ADS_CDX, &options),
            ("vfp/cdx_options", c"opts.dbf", ADS_VFP, &options),
            ("cdx/cdx_options", c"grown.dbf", ADS_CDX, &grown),
        ];
        for (dir, file, table_type, cases) in tables {
            let connect = fixtures::connect(dir);
            let mut table = 0;
            let rc = unsafe {
                crate::AdsOpenTable90(
                    connect,
                    file.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    table_type,
                    1,
                    1,
                    2,
                    0,
                    c"ansi".as_ptr().cast_mut().cast(),
                    &mut table,
                )
            };
            assert_eq!(rc, AE_SUCCESS, "{dir}");
            for exact in [0, 1] {
                assert_eq!(AdsSetExact22(table, exact), AE_SUCCESS);
                for case in cases {
                    let text = std::ffi::CString::new(*case).unwrap();
                    let text = text.as_ptr().cast_mut().cast();
                    assert_eq!(unsafe { AdsSetFilter(table, text) }, AE_SUCCESS, "{case}");
                    let expected = rows(table);
                    assert_eq!(AdsClearFilter(table), AE_SUCCESS);
                    for options in [1, 2, 9, 10] {
                        assert_eq!(unsafe { AdsSetAOF(table, text, options) }, AE_SUCCESS);
                        let got = rows(table);
                        let mut count = 0;
                        unsafe { AdsGetRecordCount(table, ADS_RESPECTFILTERS, &mut count) };
                        assert_eq!(
                            (&got, count as usize),
                            (&expected, expected.len()),
                            "{file:?} {case} (options {options}, exact {exact})"
                        );
                        assert_eq!(AdsClearAOF(table), AE_SUCCESS);
                    }
                }
            }
            crate::AdsDisconnect(connect);
        }
    }

    /// Replays the commands of `fixtures/expr/filters.json` (see
    /// `tools/ace/expr/filters.c`) as the probe ran them on the original
    /// engine, and compares each result and the table's state after it.
    #[test]
    fn filters_match_the_original_engine() {
        use serde_json::{Map, Value as Json, json};

        // Dates are in the default date format, which other tests change.
        let _settings = fixtures::settings_lock();

        use crate::AdsGetRecordCount;
        use crate::AdsSetExact22;
        use crate::{ADS_IGNOREFILTERS, ADS_RESPECTFILTERS, AdsGetLastError, expr::AdsLocate};
        use crate::{
            AdsAtBOF, AdsAtEOF, AdsGetRecordNum, AdsGotoBottom, AdsGotoRecord, AdsGotoTop,
            AdsIsRecordVisible, AdsSkip,
        };
        use crate::{AdsGetIndexHandle, AdsGetKeyCount, AdsGetKeyNum, AdsSeek};

        let path = fixtures::dir("../expr/filters.json");
        let steps: Vec<Json> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let mut failures = Vec::new();
        let (mut connect, mut table, mut order) = (0, 0, 0);
        let mut with_number = true;
        let mut opened = String::new();
        for step in &steps {
            let name = step["table"].as_str().unwrap();
            if name != opened {
                if connect != 0 {
                    crate::AdsDisconnect(connect);
                }
                let (dir, file) = name.split_once('/').unwrap();
                let file = std::ffi::CString::new(file).unwrap();
                (connect, table) = fixtures::open(dir, &file);
                (order, opened) = (0, name.to_owned());
            }
            let command = step["command"].as_str().unwrap();
            let (verb, arg) = command.split_once(' ').unwrap_or((command, ""));
            let text = std::ffi::CString::new(arg).unwrap();
            let text = || text.as_ptr().cast_mut().cast();
            let moving = if order != 0 { order } else { table };
            let mut flag: Option<u16> = None;
            let mut got: Option<Vec<u8>> = None;
            let rc = unsafe {
                match verb {
                    "order" if arg == "-" => {
                        order = 0;
                        AE_SUCCESS
                    }
                    "order" => AdsGetIndexHandle(table, text(), &mut order),
                    "aof" => AdsSetAOF(table, text(), 1),
                    "filter" => AdsSetFilter(table, text()),
                    "clearaof" => AdsClearAOF(table),
                    "clearfilter" => AdsClearFilter(table),
                    "getaof" | "getfilter" => {
                        let (mut buf, mut len) = ([0u8; 256], 256);
                        let rc = match verb {
                            "getaof" => AdsGetAOF(table, buf.as_mut_ptr(), &mut len),
                            _ => AdsGetFilter(table, buf.as_mut_ptr(), &mut len),
                        };
                        got = Some(buf[..usize::from(len)].to_vec());
                        rc
                    }
                    "top" => AdsGotoTop(moving),
                    "bottom" => AdsGotoBottom(moving),
                    "skip" => AdsSkip(moving, arg.parse().unwrap()),
                    "goto" => AdsGotoRecord(table, arg.parse().unwrap()),
                    "seek" => {
                        let (how, key) = arg.split_once(' ').unwrap();
                        let how = match how {
                            "soft" => 1,
                            "hard" => 2,
                            _ => 4,
                        };
                        let mut key = key.as_bytes().to_vec();
                        let length = key.len() as u16;
                        let mut found = 0;
                        let rc = AdsSeek(moving, key.as_mut_ptr(), length, 2, how, &mut found);
                        flag = Some(found);
                        rc
                    }
                    "locate" => {
                        let (forward, expr) = arg.split_once(' ').unwrap();
                        let expr = std::ffi::CString::new(expr).unwrap();
                        let mut found = 0;
                        let forward = forward.parse().unwrap();
                        let rc =
                            AdsLocate(table, expr.as_ptr().cast_mut().cast(), forward, &mut found);
                        flag = Some(found);
                        rc
                    }
                    "custom" => {
                        let mut numbers = arg.split(' ').map(|n| n.parse::<u32>().unwrap());
                        let option = numbers.next().unwrap() as u16;
                        let mut records: Vec<u32> = numbers.collect();
                        let count = records.len() as u32;
                        AdsCustomizeAOF(table, count, records.as_mut_ptr(), option)
                    }
                    "inaof" => {
                        let mut is_in = 0;
                        let rc = AdsIsRecordInAOF(table, arg.parse().unwrap(), &mut is_in);
                        flag = Some(is_in);
                        rc
                    }
                    "!exact" => AdsSetExact22(table, arg.parse().unwrap()),
                    "!number" => {
                        with_number = arg == "1";
                        AE_SUCCESS
                    }
                    _ => panic!("unknown command {command}"),
                }
            };
            let mut actual = Map::new();
            actual.insert("rc".into(), json!(rc));
            if rc != AE_SUCCESS {
                let (mut code, mut buf, mut len) = (0, [0u8; 600], 600);
                unsafe { AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
                actual.insert("error".into(), json!(code));
            }
            if let (Some(flag), AE_SUCCESS) = (flag, rc) {
                actual.insert("flag".into(), json!(flag != 0));
            }
            if let (Some(got), AE_SUCCESS) = (got, rc) {
                actual.insert("text".into(), json!(String::from_utf8(got).unwrap()));
            }
            // The state, read as the probe reads it, side effects included.
            let result = |rc: u32, value: u32| {
                if rc == 0 {
                    json!(value)
                } else {
                    json!(format!("E{rc}"))
                }
            };
            unsafe {
                let mut value = 0;
                let mut flag = 0;
                AdsGetRecordNum(table, ADS_IGNOREFILTERS, &mut value);
                actual.insert("recno".into(), json!(value));
                AdsAtBOF(table, &mut flag);
                actual.insert("bof".into(), json!(flag != 0));
                AdsAtEOF(table, &mut flag);
                actual.insert("eof".into(), json!(flag != 0));
                let rc = AdsIsRecordVisible(table, &mut flag);
                actual.insert("visible".into(), result(rc, u32::from(flag)));
                let rc = AdsGetRecordCount(table, ADS_RESPECTFILTERS, &mut value);
                actual.insert("count".into(), result(rc, value));
                if with_number {
                    let rc = AdsGetRecordNum(table, ADS_RESPECTFILTERS, &mut value);
                    actual.insert("number".into(), result(rc, value));
                }
                if order != 0 {
                    let rc = AdsGetKeyNum(order, ADS_RESPECTFILTERS, &mut value);
                    actual.insert("key_number".into(), result(rc, value));
                    let rc = AdsGetKeyCount(order, ADS_RESPECTFILTERS, &mut value);
                    actual.insert("key_count".into(), result(rc, value));
                }
            }
            let mut expected = step.as_object().unwrap().clone();
            expected.remove("table");
            expected.remove("command");
            if Json::Object(expected.clone()) != Json::Object(actual.clone()) {
                failures.push(format!(
                    "{name} {command}:\n  got      {actual:?}\n  expected {expected:?}"
                ));
            }
        }
        crate::AdsDisconnect(connect);
        for failure in &failures {
            eprintln!("{failure}");
        }
        assert!(failures.is_empty(), "{} steps differ", failures.len());
    }

    crate::fixtures::remote_twins!(filters_are_kept, filters_match_the_original_engine);
}
