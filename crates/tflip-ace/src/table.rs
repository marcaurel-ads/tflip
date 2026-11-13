//! Tables and cursors.

use std::path::Path;

use tflip_engine::handle::HandleKind;
use tflip_engine::table::{TableSettings, TableType};
use tflip_protocol::Api;

use crate::backend::{self, api};
use crate::ffi::{c_str, text};
use crate::field::write;
use crate::handle::{answer_at, table_check};
use crate::index::write_handles;
use crate::last_error::{Call, fail, fail_handle};
use crate::log::{Hex, stub};
use crate::{
    ADS_ADT, ADS_CDX, ADS_DATABASE_TABLE, ADS_FALSE, ADS_IGNOREFILTERS, ADS_LOCAL_SERVER, ADS_NTX,
    ADS_PROPRIETARY_LOCKING, ADS_REMOTE_SERVER, ADS_RESPECTFILTERS, ADS_RESPECTSCOPES, ADS_TRUE,
    ADS_VFP, AE_INVALID_CONNECTION_HANDLE, AE_INVALID_HANDLE, AE_INVALID_OPTION, AE_INVALID_PATH,
    AE_INVALID_POINTER, AE_INVALID_TABLE_HANDLE, AE_INVALID_TABLETYPE, AE_NO_MEMO_FILE,
    AE_NOT_FOUND, AE_SUCCESS, AdsHandle, LOCAL, TABLE_NOT_FOUND, Unsigned8, Unsigned16, Unsigned32,
    ansi, code, error_code, write_at, write_cut_at, write_string_at,
};

/// The engine's table type for an `ADS_*` table type.
pub(crate) fn engine_table_type(value: Unsigned16) -> Option<TableType> {
    match value {
        ADS_DATABASE_TABLE => Some(TableType::Database),
        ADS_NTX => Some(TableType::Ntx),
        ADS_CDX => Some(TableType::Cdx),
        ADS_ADT => Some(TableType::Adt),
        ADS_VFP => Some(TableType::Vfp),
        _ => None,
    }
}

/// `AdsOpenTable90`: opens the table `name` on a connection.
///
/// `table_type` is one of `ADS_NTX`, `ADS_CDX`, `ADS_ADT`, `ADS_VFP` or
/// `ADS_DATABASE_TABLE`; `char_type` is `ADS_ANSI` or `ADS_OEM`; `options`
/// are `ADS_EXCLUSIVE`, `ADS_READONLY`, `ADS_SHARED`, ... flags.
///
/// Reads the table's structure and hands out a handle that knows how it was
/// opened (type, character set, collation and options); the lock type is
/// ignored (the alias and rights checking are kept for `AdsGetTableAlias`
/// and `AdsGetTableRights`). See [`tflip_engine::table::open`].
///
/// Unlike the original engine, which resolves a relative `name` against the
/// process's current directory even on a connection, tflip resolves it
/// against the connection's path (as for `AdsFindFirstTable`).
///
/// # Safety
///
/// `name`, `alias` and `collation` must each be null or point to a
/// NUL-terminated string; `table` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsOpenTable90(
    connect: AdsHandle,
    name: *mut Unsigned8,
    alias: *mut Unsigned8,
    table_type: Unsigned16,
    char_type: Unsigned16,
    _lock_type: Unsigned16,
    check_rights: Unsigned16,
    options: Unsigned32,
    collation: *mut Unsigned8,
    table: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsOpenTable90");
    // SAFETY: the strings are null or NUL-terminated, `table` is null or
    // writable, as # Safety says.
    let (name, alias, collation, table) =
        unsafe { (text(name), text(alias), text(collation), table.as_mut()) };
    let (Some(name), Some(table)) = (name, table) else {
        return fail(AE_INVALID_POINTER);
    };
    let Some(table_type) = engine_table_type(table_type) else {
        return crate::last_error::fail_detail(AE_INVALID_TABLETYPE, "Parameter 4");
    };
    // With 0x10000 (which `AdsGetTableOpenOptions` reports for every open
    // table), the original engine opens nothing and hands out the handle 0,
    // once the options are valid (`fixtures/write/open_opened`).
    if options & tflip_engine::table::options::OPENED != 0 {
        if let Err(error) = tflip_engine::table::open_options(options) {
            return error_code(error);
        }
        *table = 0;
        return AE_SUCCESS;
    }
    // Without one, the engine takes the connection's (`AdsSetCollation`).
    let collation = collation.unwrap_or_default();
    // The engine checks the options and makes ADT tables ANSI.
    // A table on its own goes through the servers `AdsSetServerType`
    // allows, as connecting does.
    if connect == 0 {
        let types = tflip_engine::settings::server_types(&LOCAL);
        if types & ADS_LOCAL_SERVER == 0 {
            let error = match types & ADS_REMOTE_SERVER {
                0 => tflip_engine::Error::NoDriveConnection,
                _ => tflip_engine::Error::RemoteNotFound,
            };
            return error_code(error);
        }
    }
    let alias = alias.unwrap_or_default();
    let settings = TableSettings {
        table_type,
        char_type,
        collation,
        // The locking bit is `AdsOpenTable101`'s alone.
        options: options & !tflip_engine::table::options::COMPATIBLE_LOCKING,
        alias,
        rights: check_rights,
        // An encrypted table opens with the connection's password, if it gave one.
        password: crate::connection::password_for(connect).unwrap_or_default(),
        oem_text: false,
        by_statement: false,
    };
    let name = crate::remote::relative_name(connect, &name);
    match api(connect).open_table(connect, name, settings) {
        Ok(handle) => {
            *table = handle;
            AE_SUCCESS
        }
        Err(error) => error_code(error),
    }
}

/// `AdsOpenTable`: [`AdsOpenTable90`] without a collation.
///
/// # Safety
///
/// As for [`AdsOpenTable90`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsOpenTable(
    connect: AdsHandle,
    name: *mut Unsigned8,
    alias: *mut Unsigned8,
    table_type: Unsigned16,
    char_type: Unsigned16,
    lock_type: Unsigned16,
    check_rights: Unsigned16,
    options: Unsigned32,
    table: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsOpenTable");
    let no_collation = std::ptr::null_mut();
    // SAFETY: the same contract, and the collation may be null.
    unsafe {
        AdsOpenTable90(
            connect,
            name,
            alias,
            table_type,
            char_type,
            lock_type,
            check_rights,
            options,
            no_collation,
            table,
        )
    }
}

/// `AdsOpenTable101`: opens the table `name` on a connection with the
/// connection's settings. A relative name is resolved against the
/// connection's folder (unlike [`AdsOpenTable90`] in the original engine),
/// and `.adt` may be left out.
///
/// The table is opened as the connect string of the connection says
/// (`TableType`, `CharType`, `ReadOnly`, `Shared` and `LockMode`, see
/// [`crate::connect_options`]); a data dictionary's settings are not read.
/// Without them it is an ADT table with ANSI characters, as the original
/// engine opens it on a free connection. A handle that is not a connection (0 among them)
/// fails with `AE_INVALID_HANDLE`.
///
/// # Safety
///
/// `name` must be null or point to a NUL-terminated string; `table` must be
/// null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsOpenTable101(
    connect: AdsHandle,
    name: *mut Unsigned8,
    table: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsOpenTable101");
    // SAFETY: `name` is null or NUL-terminated, `table` null or writable, as
    // # Safety says.
    let (name, table) = unsafe { (text(name), table.as_mut()) };
    let backend = api(connect);
    if backend.handle_kind(connect) != Ok(Some(HandleKind::Connection)) {
        return fail_handle(AE_INVALID_HANDLE);
    }
    let (Some(name), Some(table)) = (name, table) else {
        return fail(AE_INVALID_POINTER);
    };
    // The tables are opened as the connect string says, ADT tables with ANSI
    // characters without it.
    let wanted = crate::connect_options::of(connect);
    let settings = TableSettings {
        table_type: wanted.table_type,
        char_type: wanted.char_type,
        collation: String::new(),
        options: wanted.open_options(),
        ..TableSettings::default()
    };
    let name = crate::remote::relative_name(connect, &name);
    match backend.open_table(connect, name, settings) {
        Ok(handle) => {
            *table = handle;
            AE_SUCCESS
        }
        // As in the original engine, unlike the other open calls.
        Err(error) if *error.plain() == tflip_engine::Error::FileNotFound => fail(TABLE_NOT_FOUND),
        Err(error) => error_code(error),
    }
}

/// `AdsGetTableType`: the type (`ADS_ADT`, `ADS_CDX`, ...) a table was
/// opened as.
///
/// # Safety
///
/// `table_type` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetTableType(
    table: AdsHandle,
    table_type: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetTableType");
    let value = api(table)
        .table_info(table)
        .map(|table| match table.settings.table_type {
            TableType::Database => ADS_DATABASE_TABLE,
            TableType::Ntx => ADS_NTX,
            TableType::Cdx => ADS_CDX,
            TableType::Adt => ADS_ADT,
            TableType::Vfp => ADS_VFP,
        })
        .map_err(error_code);
    // SAFETY: `table_type` is null or writable, as # Safety says.
    unsafe { write_at(value, table_type, 2) }
}

/// `AdsIsTableTransactionFree`: whether a table (or cursor) is
/// transaction-free, as its header says; static cursors are not. As in the original engine, a null
/// `trans_free` is accepted, and any handle that is not a table or cursor
/// (an index order among them) fails with `AE_INVALID_TABLE_HANDLE`.
///
/// # Safety
///
/// `trans_free` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsTableTransactionFree(
    table: AdsHandle,
    trans_free: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsTableTransactionFree");
    let backend = match crate::filter::table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    let info = match backend.table_info(table) {
        Ok(info) => info,
        Err(error) => return error_code(error),
    };
    let free = info.structure.header.flags & tflip_storage::table::flags::TRANSACTION_FREE != 0;
    // SAFETY: `trans_free` is null or writable, as # Safety says.
    if let Some(trans_free) = unsafe { trans_free.as_mut() } {
        *trans_free = Unsigned16::from(free);
    }
    AE_SUCCESS
}

/// `AdsAccessVfpSystemField`'s options (`aceunpub.pas`).
const RETRIEVE_NULL_FLAG: Unsigned32 = 1;
const UPDATE_NULL_FLAG: Unsigned32 = 2;
const UPDATE_FULL_FLAG: Unsigned32 = 3;
const RETRIEVE_FULL_FLAG: Unsigned32 = 4;

/// `AdsAccessVfpSystemField` (unpublished): a field's bit in the VFP
/// `_NullFlags` of the record at `record` (a client's copy, as
/// `AdsGetRecord` returns it). `ADS_RETRIEVE_NULL_FLAG` reads whether the
/// field is NULL into `*flag` (0 for a field that cannot be),
/// `ADS_RETRIEVE_FULL_FLAG` whether a VarChar or VarBinary value fills the
/// field (1 for any other field); `ADS_UPDATE_NULL_FLAG` and
/// `ADS_UPDATE_FULL_FLAG` write `*flag` into the record instead, as the
/// Delphi components use them. Other options fail with
/// `AE_INVALID_PROPERTY_ID`, as in the original engine.
///
/// # Safety
///
/// `field` must be a number below 0x10000 or point to a NUL-terminated
/// string; `record` must be null or hold the table's record length; `flag`
/// must be null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsAccessVfpSystemField(
    table: AdsHandle,
    field: *mut Unsigned8,
    record: *mut Unsigned8,
    options: Unsigned32,
    flag: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsAccessVfpSystemField");
    let backend = match crate::filter::table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    let info = match backend.table_info(table) {
        Ok(info) => info,
        Err(error) => return error_code(error),
    };
    let fields = &info.structure.fields[..];
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let id = unsafe { crate::field::FieldArg::new(field) }.id();
    let found = match tflip_engine::table::find_field(fields, id.as_ref()) {
        Ok((_, found)) => found,
        Err(error) => return error_code(error),
    };
    if !(RETRIEVE_NULL_FLAG..=RETRIEVE_FULL_FLAG).contains(&options) {
        return crate::last_error::fail_detail(
            crate::AE_INVALID_PROPERTY_ID,
            "The specified option is not valid for the requested operation.",
        );
    }
    if record.is_null() || flag.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    let length = info.structure.header.record_length as usize;
    // SAFETY: neither is null, so `record` holds the record length and
    // `flag` is readable and writable, as # Safety says.
    let (record, flag) = unsafe { (std::slice::from_raw_parts_mut(record, length), &mut *flag) };
    let (null_bit, length_bit) = match &found.layout {
        tflip_storage::field::Layout::Dbf(layout) => (layout.null_bit, layout.length_bit),
        _ => (None, None),
    };
    match options {
        RETRIEVE_NULL_FLAG => {
            *flag = Unsigned16::from(null_bit.is_some_and(|bit| bit.is_set(record)));
        }
        RETRIEVE_FULL_FLAG => {
            *flag = Unsigned16::from(!length_bit.is_some_and(|bit| bit.is_set(record)));
        }
        UPDATE_NULL_FLAG => {
            if let Some(bit) = null_bit {
                bit.set(record, *flag != 0);
            }
        }
        UPDATE_FULL_FLAG => {
            if let Some(bit) = length_bit {
                bit.set(record, *flag == 0);
            }
        }
        _ => unreachable!("options checked above"),
    }
    AE_SUCCESS
}

/// `ADS_BASENAME`, `ADS_BASENAMEANDEXT` and `ADS_FULLPATHNAME`.
const BASENAME: Unsigned16 = 1;
const BASENAME_AND_EXT: Unsigned16 = 2;
const FULL_PATH_NAME: Unsigned16 = 3;

/// `AdsGetTableFilename`: the name of a table's (or cursor's) file: without
/// its extension (`ADS_BASENAME`), with it (`ADS_BASENAMEANDEXT`), or its
/// whole path (`ADS_FULLPATHNAME`), in `name` of `*len` bytes. As in the
/// original engine, a name that does not fit is copied as far as it fits and
/// reports the size it needs with the NUL, and an index order is no table
/// (`AE_INVALID_TABLE_HANDLE`). A static SQL cursor is named `tim_<n>.tmp`
/// in its connection's folder. On a server the path is the server's.
///
/// # Safety
///
/// `name` must be null or hold `*len` bytes; `len` must be null or valid
/// for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetTableFilename(
    table: AdsHandle,
    option: Unsigned16,
    name: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetTableFilename");
    let backend = match crate::filter::table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    let info = match backend.table_info(table) {
        Ok(info) => info,
        Err(error) => return error_code(error),
    };
    if let Some(code) = crate::ffi::null_buffer(name, len, 3) {
        return code;
    }
    match file_name(&info.path, option) {
        // SAFETY: `name` and `len` are as # Safety says.
        Some(text) => unsafe { crate::write_cut(&ansi::from_str_lossy(text), name, len) },
        // The option of a database table's file (ADS_FULLPATHNAME with
        // ...) is another number; a free table has none.
        None if option == 4 => {
            crate::last_error::fail_detail(AE_INVALID_OPTION, "The table is not a database table.")
        }
        None => fail(AE_INVALID_OPTION),
    }
}

/// The part of `path` an `ADS_BASENAME`, `ADS_BASENAMEANDEXT` or
/// `ADS_FULLPATHNAME` option asks for; `None` for another option.
pub(crate) fn file_name(path: &str, option: Unsigned16) -> Option<&str> {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    Some(match option {
        BASENAME => file.rsplit_once('.').map_or(file, |(stem, _)| stem),
        BASENAME_AND_EXT => file,
        FULL_PATH_NAME => path,
        _ => return None,
    })
}

/// `AdsGetTableCharType`: the character set (`ADS_ANSI`, `ADS_OEM` or a VFP
/// collation number) a table was opened with, reported as given to
/// `AdsOpenTable90`.
///
/// # Safety
///
/// `char_type` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetTableCharType(
    table: AdsHandle,
    char_type: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetTableCharType");
    let value = api(table)
        .table_info(table)
        .map(|table| table.settings.char_type)
        .map_err(error_code);
    // SAFETY: `char_type` is null or writable, as # Safety says.
    unsafe { write_at(value, char_type, 2) }
}

/// `AdsGetTableCollation`: the collation a table was opened with, by name,
/// as given to `AdsOpenTable90` (empty if none was given).
///
/// # Safety
///
/// `collation` must be null or hold `*len` bytes; `len` must be null or valid
/// for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetTableCollation(
    table: AdsHandle,
    collation: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetTableCollation");
    let table = match api(table).table_info(table) {
        Ok(table) => table,
        Err(error) => return error_code(error),
    };
    // The name came in through the ANSI code page, so it goes back out.
    let name = ansi::from_os(table.settings.collation.as_ref()).unwrap_or_default();
    // SAFETY: `collation` and `len` are as # Safety says.
    unsafe { write_string_at(&name, collation, len, 2) }
}

/// `AdsGetTableConnection`: the connection a table was opened on, or 0 for a
/// table opened without one.
///
/// # Safety
///
/// `connect` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetTableConnection(
    table: AdsHandle,
    connect: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetTableConnection");
    let value = api(table)
        .table_info(table)
        .map(|table| table.connection)
        .map_err(error_code);
    // SAFETY: `connect` is null or writable, as # Safety says.
    unsafe { write(value, connect) }
}

/// `AdsGetTableOpenOptions`: the options (`ADS_EXCLUSIVE`, `ADS_READONLY`,
/// ... flags) a table was opened with, reported as given to `AdsOpenTable90`.
///
/// # Safety
///
/// `options` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetTableOpenOptions(
    table: AdsHandle,
    options: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetTableOpenOptions");
    let value = api(table)
        .table_info(table)
        // What to do with an index of another collation is not reported.
        .map(|table| {
            use tflip_engine::table::options::{CREATED, KEPT, OPENED};
            let options = table.settings.options;
            match options & CREATED != 0 {
                true => options & !(KEPT | OPENED),
                false => options & !KEPT,
            }
        })
        .map_err(error_code);
    // SAFETY: `options` is null or writable, as # Safety says.
    unsafe { write_at(value, options, 2) }
}

/// `AdsGetRecordCount`: the number of records in a table or cursor;
/// `filter_option` is one of `ADS_RESPECTFILTERS`, `ADS_IGNOREFILTERS` or
/// `ADS_RESPECTSCOPES`, optionally ORed with `ADS_REFRESHCOUNT`.
///
/// An ADT table reports its records, deleted ones not counted. Through an
/// index order with a scope, the keys in the scope count, unless the option
/// is `ADS_IGNOREFILTERS`, as in the original engine. With
/// `ADS_RESPECTFILTERS`, only the records the table's filters hold for
/// count. A live cursor counts the records its `WHERE` holds for. A
/// statement reports the rows its last execution affected, `0xFFFFFFFF`
/// after a `SELECT`. Other handles report 0 for now. An option without
/// one of the three, or with other bits, is `AE_INVALID_FILTER_OPTION`, as
/// in the original engine. A handle that is none of these (a connection,
/// a closed handle) is `AE_INVALID_HANDLE`, after the option and the
/// pointer are checked (`fixtures/errors/record_count.json`).
///
/// # Safety
///
/// `count` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetRecordCount(
    table: AdsHandle,
    filter_option: Unsigned16,
    count: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetRecordCount");
    let scopes = filter_option & ADS_RESPECTSCOPES != ADS_IGNOREFILTERS;
    let filters = filter_option & ADS_RESPECTSCOPES == ADS_RESPECTFILTERS;
    let backend = api(table);
    let kind = backend.handle_kind(table);
    if matches!(kind, Ok(None)) {
        // A handle of 0 is "not readable", any other value "not recognized".
        let detail = if table == 0 {
            "The given handle is not readable."
        } else {
            crate::last_error::NOT_A_HANDLE
        };
        return crate::last_error::fail_detail(crate::AE_INVALID_HANDLE, detail);
    }
    if filter_option & ADS_RESPECTSCOPES == 0
        || filter_option & !(ADS_RESPECTSCOPES | crate::ADS_REFRESHCOUNT) != 0
    {
        return crate::last_error::fail_detail(crate::AE_INVALID_FILTER_OPTION, "Parameter 2");
    }
    if count.is_null() {
        return crate::last_error::fail_detail(crate::AE_INVALID_POINTER, "Parameter 3");
    }
    if matches!(kind, Ok(Some(HandleKind::Connection))) {
        return crate::last_error::fail_detail(
            crate::AE_INVALID_HANDLE,
            "Must be statement, table, cursor or index order handle.",
        );
    }
    let records = match kind {
        Ok(Some(HandleKind::Statement)) => backend.affected(table),
        _ => backend.record_count(table, scopes, filters),
    };
    let records = match records {
        Ok(records) => records,
        Err(tflip_engine::Error::NotImplemented) => {
            stub!("AdsGetRecordCount", Hex(table), Hex(filter_option), count);
            0
        }
        Err(error) => return error_code(error),
    };
    // SAFETY: `count` is null or writable, as # Safety says.
    unsafe { write(Ok(records), count) }
}

/// `AdsGetLastTableUpdate`: the date the table's file was last written, in
/// the date format, in `buf` of `*len` bytes; `*len` is set to its length.
/// A buffer that is too small gets what fits and `AE_INSUFFICIENT_BUFFER`,
/// with `*len` set to the length needed with the NUL, as in the original
/// engine.
///
/// The original engine writes to a table it opens for changing, so for it
/// the date is the day such a table was opened; tflip writes nothing (see
/// docs/KNOWN_ISSUES.md).
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetLastTableUpdate(
    table: AdsHandle,
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    // The original engine neither clears the last error nor records a
    // NULL pointer here (`fixtures/errors/param_n.json`).
    let backend = match crate::filter::table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    if buf.is_null() || len.is_null() {
        return AE_INVALID_POINTER;
    }
    match backend.last_update(table) {
        // SAFETY: `buf` and `len` are as # Safety says.
        Ok(date) => unsafe { crate::write_cut(date.as_bytes(), buf, len) },
        Err(error) => error_code(error),
    }
}

/// `AdsGetTableMemoSize`: the memo size of a DBF table (`AdsCreateTable`'s);
/// ADT tables have none and fail with `AE_NOT_IMPLEMENTED`, as in the
/// original engine. DBF tables fail the same way, for want of a probe.
///
/// # Safety
///
/// `size` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetTableMemoSize(
    table: AdsHandle,
    size: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetTableMemoSize");
    let _ = size;
    match crate::filter::table_of(table) {
        Ok(_) => fail(crate::AE_NOT_IMPLEMENTED),
        Err(code) => code,
    }
}

/// `AdsGetRecordLength`: the length of a table's records in bytes, the
/// 5-byte record prefix of ADT tables included.
///
/// # Safety
///
/// `length` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetRecordLength(
    table: AdsHandle,
    length: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetRecordLength");
    let opened = match api(table).table_info(table) {
        Ok(opened) => opened,
        Err(error) => return error_code(error),
    };
    if length.is_null() {
        return crate::last_error::null_param(2);
    }
    let value = opened.structure.header.record_length;
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *length = value };
    AE_SUCCESS
}

/// `AdsGetNumFields`: the number of fields of a table.
///
/// # Safety
///
/// `count` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetNumFields(
    table: AdsHandle,
    count: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetNumFields");
    let opened = match api(table).table_info(table) {
        Ok(opened) => opened,
        Err(error) => return error_code(error),
    };
    if count.is_null() {
        return crate::last_error::null_param(2);
    }
    let value = opened.structure.header.field_count;
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *count = value };
    AE_SUCCESS
}

/// The handles of the open index orders of the table (or cursor) `table`;
/// any other handle, an index order among them, is
/// `AE_INVALID_TABLE_HANDLE`, as in the original engine.
fn orders(table: AdsHandle) -> Result<Vec<AdsHandle>, Unsigned32> {
    crate::filter::table_of(table)?
        .indexes(table)
        .map_err(error_code)
}

/// `AdsGetNumIndexes`: the number of a table's open index orders: those of
/// its structural index (0 without one, as in the original engine), but its
/// full-text indexes (see [`AdsGetNumFTSIndexes`]).
///
/// # Safety
///
/// `count` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetNumIndexes(
    table: AdsHandle,
    count: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetNumIndexes");
    let value = orders(table).map(|orders| orders.len() as Unsigned16);
    // SAFETY: `count` is null or writable, as # Safety says.
    unsafe { write_at(value, count, 2) }
}

/// `AdsGetNumFTSIndexes`: the number of a table's (or cursor's) full-text
/// (FTS) indexes, which `AdsGetNumIndexes` does not count. As in the
/// original engine, any other handle (an index order among them) fails with
/// `AE_INVALID_TABLE_HANDLE`.
///
/// # Safety
///
/// `count` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetNumFTSIndexes(
    table: AdsHandle,
    count: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetNumFTSIndexes");
    let backend = match crate::filter::table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    let value = backend
        .full_text_indexes(table)
        .map(|indexes| indexes.len() as Unsigned16)
        .map_err(error_code);
    // SAFETY: `count` is null or writable, as # Safety says.
    unsafe { write(value, count) }
}

/// `AdsGetAllIndexes`: the handles of a table's open index orders, in the
/// order they were opened, in `indexes` (an array of `*len` handles);
/// `*len` is set to how many there are. If they do not all fit, the first
/// `*len` are written and `AE_INSUFFICIENT_BUFFER` is returned with `*len`
/// set to the number needed, as in the original engine.
///
/// # Safety
///
/// `indexes` must be null or hold `*len` handles; `len` must be null or
/// valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetAllIndexes(
    table: AdsHandle,
    indexes: *mut AdsHandle,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetAllIndexes");
    let orders = match orders(table) {
        Ok(orders) => orders,
        Err(code) => return code,
    };
    if len.is_null() {
        return crate::last_error::null_param(3);
    }
    if indexes.is_null() && !orders.is_empty() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `len` is not null, so readable and writable, and `indexes`
    // holds `*len` handles or there are none to write, as # Safety says.
    unsafe { write_handles(&orders, indexes, len) }
}

/// `AdsGetMemoBlockSize`: the block size of a table's memo file, in bytes.
///
/// As in the original engine: a table without a memo file fails with
/// `AE_NO_MEMO_FILE`, and any handle that is not a table (an index order
/// among them) with `AE_INVALID_TABLE_HANDLE`, with `AE_INVALID_HANDLE` as
/// the last error for a value that is no handle. A DBF table's is its
/// `.dbt`'s (512) or `.fpt`'s.
///
/// # Safety
///
/// `block_size` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetMemoBlockSize(
    table: AdsHandle,
    block_size: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetMemoBlockSize");
    let backend = api(table);
    match backend.handle_kind(table) {
        Ok(Some(HandleKind::Table | HandleKind::Cursor)) => {}
        Ok(None) => {
            fail_handle(AE_INVALID_HANDLE);
            return AE_INVALID_TABLE_HANDLE;
        }
        Ok(Some(_)) => return fail_handle(AE_INVALID_TABLE_HANDLE),
        Err(error) => return error_code(error),
    }
    if block_size.is_null() {
        return crate::last_error::null_param(2);
    }
    let info = match backend.table_info(table) {
        Ok(info) => info,
        Err(error) => return error_code(error),
    };
    let Some(size) = info.memo_block_size else {
        return crate::last_error::fail_detail(AE_NO_MEMO_FILE, "This table has no memo file.");
    };
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *block_size = Unsigned16::try_from(size).unwrap_or(Unsigned16::MAX) };
    AE_SUCCESS
}

/// `AdsCloseTable`: closes a table opened by `AdsOpenTable90`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsCloseTable(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCloseTable");
    let backend = api(table);
    // A handle that is not open at all: the original engine returns
    // `AE_INVALID_TABLE_HANDLE`, but records `AE_INVALID_HANDLE`.
    if backend.handle_kind(table) == Ok(None) {
        crate::last_error::fail_detail(crate::AE_INVALID_HANDLE, crate::last_error::NOT_A_HANDLE);
        return AE_INVALID_TABLE_HANDLE;
    }
    // Its check replaces an error left before
    // (`fixtures/errors/handle_order.json`).
    if let Ok(Some(HandleKind::Connection | HandleKind::Statement | HandleKind::Index)) =
        backend.handle_kind(table)
    {
        let detail = crate::last_error::NOT_A_TABLE;
        return crate::last_error::fail_detail(AE_INVALID_TABLE_HANDLE, detail);
    }
    code(backend.close_table(table))
}

/// The tables the process has opened by name and still has open, its own
/// and those on servers, with their full paths: local ones first, each
/// session's in the order they were opened.
fn open_tables() -> Vec<(AdsHandle, String)> {
    let mut handles = LOCAL.tables(0).unwrap_or_default();
    for remote in backend::all_remotes() {
        handles.extend(remote.client.tables(0).unwrap_or_default());
    }
    handles
        .into_iter()
        .map(|handle| {
            let path = api(handle)
                .table_info(handle)
                .map(|i| i.path)
                .unwrap_or_default();
            (handle, path)
        })
        .collect()
}

/// `AdsCheckExistence`: whether the file `name` exists. As in the original
/// engine, names are not resolved against the connection: a relative one is
/// looked for in the current folder, and on connection 0 fails with
/// `AE_INVALID_PATH`.
///
/// # Safety
///
/// `name` must be null or point to a NUL-terminated string; `exists` must
/// be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCheckExistence(
    connect: AdsHandle,
    name: *mut Unsigned8,
    exists: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsCheckExistence");
    // SAFETY: `name` is null or NUL-terminated, `exists` null or writable, as
    // # Safety says.
    let (name, exists) = unsafe { (c_str(name), exists.as_mut()) };
    if connect != 0 && api(connect).handle_kind(connect) != Ok(Some(HandleKind::Connection)) {
        return fail(AE_INVALID_CONNECTION_HANDLE);
    }
    let (Some(name), Some(exists)) = (name, exists) else {
        return fail(AE_INVALID_POINTER);
    };
    let name = ansi::to_os(name);
    let path = Path::new(&name);
    if connect == 0 && !path.is_absolute() {
        *exists = ADS_FALSE;
        return fail(AE_INVALID_PATH);
    }
    let found = !name.is_empty() && path.is_file();
    *exists = if found { ADS_TRUE } else { ADS_FALSE };
    AE_SUCCESS
}

/// `AdsGetNumOpenTables`: the number of tables the process has open
/// through `AdsOpenTable` and its variants.
///
/// # Safety
///
/// `count` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetNumOpenTables(count: *mut Unsigned16) -> Unsigned32 {
    let _call = Call::begin("AdsGetNumOpenTables");
    let value = open_tables().len() as Unsigned16;
    // SAFETY: `count` is null or writable, as # Safety says.
    unsafe { answer_at(Ok(()), count, value, 1) }
}

/// `AdsGetAllTables`: the handles of the tables the process has open, in
/// the order they were opened, in `tables` (an array of `*len` handles);
/// `*len` is set to their number. If they do not all fit, or fill the array
/// exactly, the first `*len` are written and `AE_INSUFFICIENT_BUFFER` is
/// returned.
///
/// # Safety
///
/// `tables` must be null or hold `*len` handles; `len` must be null or
/// valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetAllTables(
    tables: *mut AdsHandle,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetAllTables");
    let open: Vec<AdsHandle> = open_tables()
        .into_iter()
        .map(|(handle, ..)| handle)
        .collect();
    if len.is_null() || (tables.is_null() && !open.is_empty()) {
        return fail(AE_INVALID_POINTER);
    }
    // The original engine wants room for one handle more than there are
    // tables, none included; it writes those that fit and sets `*len` to
    // their number all the same.
    // SAFETY: `len` is not null, so readable and writable, and `tables`
    // holds `*len` handles or there are none to write, as # Safety says.
    let crowded = usize::from(unsafe { *len }) == open.len();
    // SAFETY: as above.
    let rc = unsafe { write_handles(&open, tables, len) };
    match (rc, crowded) {
        (AE_SUCCESS, true) => fail(crate::AE_INSUFFICIENT_BUFFER),
        _ => rc,
    }
}

/// The open table whose file is `name` (a full path, regardless of case),
/// on `connect` if it is not 0.
fn table_named(connect: AdsHandle, name: &str) -> Option<AdsHandle> {
    open_tables().into_iter().find_map(|(handle, path)| {
        let on = connect == 0
            || api(handle)
                .table_info(handle)
                .is_ok_and(|info| info.connection == connect);
        (on && path.eq_ignore_ascii_case(name)).then_some(handle)
    })
}

/// Writes the table `found` to `table`: `AE_NOT_FOUND` and 0 if there is
/// none; `AE_INVALID_POINTER` for no `table`.
fn write_table(found: Option<AdsHandle>, table: Option<&mut AdsHandle>) -> Unsigned32 {
    let Some(table) = table else {
        return fail(AE_INVALID_POINTER);
    };
    *table = found.unwrap_or(0);
    match found {
        Some(_) => AE_SUCCESS,
        None => fail(AE_NOT_FOUND),
    }
}

/// `AdsGetTableHandle`: the open table whose file is `name`. As in the
/// original engine, only a full path finds it (not its alias or its name
/// alone); none found fails with `AE_NOT_FOUND` and 0.
///
/// # Safety
///
/// `name` must be null or point to a NUL-terminated string; `table` must be
/// null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetTableHandle(
    name: *mut Unsigned8,
    table: *mut AdsHandle,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetTableHandle");
    // SAFETY: `name` is null or NUL-terminated, `table` null or writable, as
    // # Safety says.
    let (name, table) = unsafe { (text(name), table.as_mut()) };
    let Some(name) = name else {
        return fail(AE_INVALID_POINTER);
    };
    write_table(table_named(0, &name), table)
}

/// `AdsGetTableHandle25`: as `AdsGetTableHandle`, among the tables of a
/// connection.
///
/// # Safety
///
/// As for `AdsGetTableHandle`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetTableHandle25(
    connect: AdsHandle,
    name: *mut Unsigned8,
    table: *mut AdsHandle,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetTableHandle25");
    // SAFETY: `name` is null or NUL-terminated, `table` null or writable, as
    // # Safety says.
    let (name, table) = unsafe { (text(name), table.as_mut()) };
    let Some(name) = name else {
        return fail(AE_INVALID_POINTER);
    };
    write_table(table_named(connect, &name), table)
}

/// `AdsGetTableAlias`: the alias a table was opened with, or its file's
/// name without the extension, in `buf` of `*len` bytes. A buffer that is
/// too small gets what fits and `AE_INSUFFICIENT_BUFFER`, with `*len` set
/// to the length needed with the NUL.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetTableAlias(
    table: AdsHandle,
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetTableAlias");
    let info = match api(table).table_info(table) {
        Ok(info) => info,
        Err(error) => return error_code(error),
    };
    // SAFETY: `buf` and `len` are as # Safety says.
    unsafe { write_cut_at(&ansi::from_str_lossy(&info.alias), buf, len, 2) }
}

/// `AdsCloseAllTables`: closes every table the process opened.
#[unsafe(no_mangle)]
pub extern "system" fn AdsCloseAllTables() -> Unsigned32 {
    let _call = Call::begin("AdsCloseAllTables");
    for (handle, _) in open_tables() {
        let _ = api(handle).close_table(handle);
    }
    // As in the original engine, the tables connections keep open for reuse
    // close too.
    for remote in backend::all_remotes() {
        let _ = remote.client.close_cached_tables(remote.connection);
    }
    AE_SUCCESS
}

/// `AdsGetTableLockType`: `ADS_PROPRIETARY_LOCKING`, as the original
/// engine reports for every ADT table.
///
/// # Safety
///
/// `lock_type` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetTableLockType(
    table: AdsHandle,
    lock_type: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetTableLockType");
    let check = table_check(table);
    // Compatible locking is what `AdsOpenTable101` opens a table with if its
    // connection's `LockMode` says so; any other table is proprietary.
    let compatible = check.is_ok()
        && api(table).table_info(table).is_ok_and(|info| {
            info.settings.options & tflip_engine::table::options::COMPATIBLE_LOCKING != 0
        });
    let locking = match compatible {
        true => 0,
        false => ADS_PROPRIETARY_LOCKING,
    };
    // SAFETY: `lock_type` is null or writable, as # Safety says.
    unsafe { answer_at(check, lock_type, locking, 2) }
}

/// `AdsGetTableRights`: the rights checking the table was opened with
/// (`ADS_CHECKRIGHTS` or `ADS_IGNORERIGHTS`).
///
/// # Safety
///
/// `rights` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetTableRights(
    table: AdsHandle,
    rights: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetTableRights");
    if let Err(code) = table_check(table) {
        return code;
    }
    match api(table).table_info(table) {
        // SAFETY: `rights` is null or writable, as # Safety says.
        Ok(info) => unsafe { answer_at(Ok(()), rights, info.settings.rights, 2) },
        Err(error) => error_code(error),
    }
}

/// `AdsCacheRecords`: how many records to read ahead; tflip reads one at a
/// time and takes the number only.
#[unsafe(no_mangle)]
pub extern "system" fn AdsCacheRecords(table: AdsHandle, records: Unsigned16) -> Unsigned32 {
    let _call = Call::begin("AdsCacheRecords");
    let _ = records;
    table_check(table).map_or_else(|code| code, |()| AE_SUCCESS)
}

/// `AdsGetLastAutoinc`: the AutoInc value of the last record the table
/// handle appended, 0 before the first; 0 for other handles, but
/// `AE_INVALID_HANDLE` for a value that is no handle.
///
/// # Safety
///
/// `value` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetLastAutoinc(
    handle: AdsHandle,
    value: *mut Unsigned32,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetLastAutoinc");
    let last = match table_check(handle) {
        Ok(()) => api(handle).last_autoinc(handle).map_err(error_code),
        // The original takes other handles too (a connection, a statement),
        // but not a value that is no handle, such as 0
        // (`fixtures/write/open_opened`).
        Err(_) => match api(handle).handle_kind(handle) {
            Ok(None) => Err(crate::last_error::fail_handle(crate::AE_INVALID_HANDLE)),
            _ => Ok(0),
        },
    };
    // A NULL pointer is not refused (`fixtures/errors/param_n.json`).
    match last {
        Ok(last) if !value.is_null() => {
            // SAFETY: not null, so writable, as # Safety says.
            unsafe { *value = last };
            AE_SUCCESS
        }
        Ok(_) => AE_SUCCESS,
        Err(code) => code,
    }
}

/// `AdsCloneTable`: opens a table again, with its settings (its structural
/// orders opened too), on the record it is on; the clone has locks of its
/// own. A handle that is no table fails with `AE_INVALID_TABLE_HANDLE`.
///
/// # Safety
///
/// `clone` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCloneTable(table: AdsHandle, clone: *mut AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCloneTable");
    // SAFETY: `clone` is null or writable, as # Safety says.
    let clone = unsafe { clone.as_mut() };
    let backend = match crate::filter::table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    let Some(clone) = clone else {
        return fail(AE_INVALID_POINTER);
    };
    match backend.clone_table(table) {
        Ok(handle) => {
            *clone = handle;
            AE_SUCCESS
        }
        Err(error) => error_code(error),
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::AE_INSUFFICIENT_BUFFER;
    use crate::{
        ADS_ANSI, ADS_EXCLUSIVE, ADS_OEM, ADS_READONLY, ADS_SHARED, AE_CORRUPT_TABLE,
        AE_FILE_NOT_FOUND, AE_INVALID_CONNECTION_HANDLE, AE_INVALID_TABLE_HANDLE,
        INVALID_ENCRYPTION_PASSWORD, fixtures,
    };
    use std::ffi::CStr;

    /// As in the original engine: one of the filter options, with or
    /// without `ADS_REFRESHCOUNT`, and nothing else; checked before the
    /// pointer.
    #[test]
    fn record_count_options() {
        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        for (option, rc) in [
            (1, AE_SUCCESS),
            (7, AE_SUCCESS),
            (0, 5013),
            (4, 5013),
            (8, 5013),
        ] {
            let mut count = 0x77;
            assert_eq!(
                unsafe { AdsGetRecordCount(table, option, &mut count) },
                rc,
                "{option}"
            );
            assert_eq!(count, if rc == AE_SUCCESS { 4 } else { 0x77 }, "{option}");
        }
        let rc = unsafe { AdsGetRecordCount(table, 0, std::ptr::null_mut()) };
        assert_eq!(rc, 5013);
        crate::AdsDisconnect(connect);
    }

    /// As in the original engine, for handles of every kind, with the
    /// options and the pointer (`fixtures/errors/record_count.json`, from
    /// `tools/ace/errors/record_count.c`): the handle is checked first, then
    /// the option, then the pointer, then the handle's kind.
    #[test]
    fn record_count_handles() {
        use serde_json::Value as Json;
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/errors/record_count.json");
        let rows: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (connect, table) = fixtures::open("adi_keys", c"keys.adt");
        let mut order = 0;
        assert_eq!(
            unsafe { crate::AdsGetIndexHandleByOrder(table, 1, &mut order) },
            AE_SUCCESS
        );
        let (mut stmt, mut select, mut cursor, mut closed) = (0, 0, 0, 0);
        for statement in [&mut stmt, &mut select, &mut closed] {
            assert_eq!(
                unsafe { crate::AdsCreateSQLStatement(connect, statement) },
                AE_SUCCESS
            );
        }
        let sql = c"SELECT * FROM keys".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { crate::AdsExecuteSQLDirect(select, sql, &mut cursor) },
            AE_SUCCESS
        );
        assert_eq!(crate::AdsCloseSQLStatement(closed), AE_SUCCESS);
        let handle = |name: &str| match name {
            "none" => 0x12345,
            "zero" => 0,
            "connection" => connect,
            "statement" => stmt,
            "statement_after_select" => select,
            "order" => order,
            // The probe's closed table and statement are both unknown.
            "closed" | "closed_statement" => closed,
            "table" => table,
            "cursor" => cursor,
            _ => unreachable!("{name}"),
        };
        let mut problems = Vec::new();
        for row in rows {
            let name = row["handle"].as_str().unwrap();
            let option = row["option"].as_u64().unwrap() as u16;
            let mut count = 777;
            let pointer = if row["null"] == 1 {
                std::ptr::null_mut()
            } else {
                &mut count
            };
            let rc = unsafe { AdsGetRecordCount(handle(name), option, pointer) };
            let mut last = 0;
            let (mut buf, mut len) = ([0u8; 600], 600u16);
            unsafe { crate::AdsGetLastError(&mut last, buf.as_mut_ptr(), &mut len) };
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            let text = String::from_utf8_lossy(&buf[..end]).into_owned();
            let want = (
                row["rc"].as_u64().unwrap(),
                row["count"].as_u64().unwrap(),
                row["last"].as_u64().unwrap(),
                row["text"].as_str().unwrap(),
            );
            let got = (u64::from(rc), u64::from(count), u64::from(last), &*text);
            if got != want {
                problems.push(format!("{name} {option}: got {got:?}, want {want:?}"));
            }
        }
        crate::AdsDisconnect(connect);
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// As in the original engine: a full-text index is no index order.
    #[test]
    fn full_text_indexes() {
        let (connect, table) = fixtures::open("adi_options", c"opts.adt");
        let (mut fts, mut orders) = (0x77, 0x77);
        assert_eq!(unsafe { AdsGetNumFTSIndexes(table, &mut fts) }, AE_SUCCESS);
        assert_eq!(unsafe { AdsGetNumIndexes(table, &mut orders) }, AE_SUCCESS);
        assert_eq!((fts, orders), (1, 4));
        let rc = unsafe { AdsGetNumFTSIndexes(table, std::ptr::null_mut()) };
        assert_eq!(rc, crate::AE_INVALID_POINTER);
        crate::AdsDisconnect(connect);
        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        assert_eq!(unsafe { AdsGetNumFTSIndexes(table, &mut fts) }, AE_SUCCESS);
        assert_eq!(fts, 0);
        assert_eq!(
            unsafe { AdsGetNumFTSIndexes(connect, &mut fts) },
            AE_INVALID_TABLE_HANDLE
        );
        crate::AdsDisconnect(connect);
    }

    /// As in the original engine: the header's flag, a null pointer
    /// accepted, and an index order not a table.
    #[test]
    fn transaction_free_tables() {
        let mut free = 0x77;
        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        assert_eq!(
            unsafe { AdsIsTableTransactionFree(table, &mut free) },
            AE_SUCCESS
        );
        assert_eq!(free, 0);
        assert_eq!(
            unsafe { AdsIsTableTransactionFree(table, std::ptr::null_mut()) },
            AE_SUCCESS
        );
        let mut order = 0;
        let name = c"by_name".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { crate::AdsGetIndexHandle(table, name, &mut order) },
            AE_SUCCESS
        );
        free = 0x77;
        let rc = unsafe { AdsIsTableTransactionFree(order, &mut free) };
        assert_eq!((rc, free), (AE_INVALID_TABLE_HANDLE, 0x77));
        crate::AdsDisconnect(connect);
        // A dictionary's table with the flag, which opens only through its
        // dictionary.
        let (connect, table) =
            fixtures::open_in_dictionary("dictionary_props", "props.add", c"child");
        assert_eq!(
            unsafe { AdsIsTableTransactionFree(table, &mut free) },
            AE_SUCCESS
        );
        assert_eq!(free, 1);
        crate::AdsDisconnect(connect);
    }

    /// Opens `name` the way ARC does, with the given table type, character set
    /// and options.
    fn open_as(
        connect: AdsHandle,
        name: &CStr,
        table_type: Unsigned16,
        char_type: Unsigned16,
        options: Unsigned32,
    ) -> (Unsigned32, AdsHandle) {
        let mut table = 0;
        let name = name.as_ptr().cast_mut().cast();
        let collation = c"ansi".as_ptr().cast_mut().cast();
        let null = std::ptr::null_mut();
        let rc = unsafe {
            AdsOpenTable90(
                connect, name, null, table_type, char_type, 1, 2, options, collation, &mut table,
            )
        };
        (rc, table)
    }

    fn open(connect: AdsHandle, name: &CStr, table_type: Unsigned16) -> (Unsigned32, AdsHandle) {
        open_as(connect, name, table_type, ADS_ANSI, 0)
    }

    #[test]
    fn table_type_round_trip() {
        let connect = fixtures::connect("all_types");
        let mut value = 0;
        let (rc, table) = open(connect, c"all_types.adt", ADS_ADT);
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(unsafe { AdsGetTableType(table, &mut value) }, AE_SUCCESS);
        assert_eq!(value, ADS_ADT);
        // Only a dictionary's tables (`fixtures/write/database_type`).
        assert_eq!(
            open(connect, c"all_types.adt", ADS_DATABASE_TABLE).0,
            AE_INVALID_TABLETYPE
        );
        let dbf = fixtures::connect("vfp/dbf_types");
        for table_type in [ADS_CDX, ADS_NTX, ADS_VFP] {
            let (rc, table) = open(dbf, c"dbf_types.dbf", table_type);
            assert_eq!(rc, AE_SUCCESS);
            assert_eq!(unsafe { AdsGetTableType(table, &mut value) }, AE_SUCCESS);
            assert_eq!(value, table_type);
        }
        assert_eq!(crate::AdsDisconnect(dbf), crate::AE_SUCCESS);

        assert_eq!(open(connect, c"all_types.adt", 99).0, AE_INVALID_TABLETYPE);
        assert_eq!(
            open(0x12345, c"all_types.adt", ADS_ADT).0,
            AE_INVALID_CONNECTION_HANDLE
        );
        assert_eq!(
            unsafe { AdsGetTableType(connect, &mut value) },
            AE_INVALID_TABLE_HANDLE
        );
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn char_type_round_trip() {
        let connect = fixtures::connect("all_types");
        let dbf = fixtures::connect("cdx/dbf_types");
        let mut value = 0;
        // As in the original engine: ADT tables are ANSI whatever they are
        // opened as; other types keep the character set given.
        for char_type in [ADS_ANSI, ADS_OEM, 3] {
            let (rc, table) = open_as(connect, c"all_types.adt", ADS_ADT, char_type, 0);
            assert_eq!(rc, AE_SUCCESS);
            assert_eq!(
                unsafe { AdsGetTableCharType(table, &mut value) },
                AE_SUCCESS
            );
            assert_eq!(value, ADS_ANSI);
            let (rc, table) = open_as(dbf, c"dbf_types", ADS_CDX, char_type, 0);
            assert_eq!(rc, AE_SUCCESS);
            assert_eq!(
                unsafe { AdsGetTableCharType(table, &mut value) },
                AE_SUCCESS
            );
            assert_eq!(value, char_type);
        }
        let rc = unsafe { AdsGetTableCharType(connect, &mut value) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);
        assert_eq!(crate::AdsDisconnect(dbf), crate::AE_SUCCESS);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn open_options_round_trip() {
        // A copy of its own, which no other test has open when it is opened
        // exclusively.
        let connect = fixtures::connect_to(&fixtures::scratch("open_options", "all_types"));
        let mut value = 0;
        // As the original engine reports them, or refuses them.
        let cases = [
            (0, Ok(0x100f0)),
            (
                ADS_SHARED | crate::ADS_CLIPPER_MEMOS | 0x2000_0000,
                Ok(0x100f0),
            ),
            (ADS_EXCLUSIVE, Ok(0x100f1)),
            (ADS_EXCLUSIVE | ADS_READONLY, Ok(0x10013)),
            (crate::ADS_TABLE_PERM_READ, Ok(0x10012)),
            (
                crate::ADS_TABLE_PERM_READ | crate::ADS_TABLE_PERM_UPDATE,
                Ok(0x10030),
            ),
            (ADS_EXCLUSIVE | ADS_SHARED, Err(crate::AE_INVALID_OPTION)),
            (crate::ADS_TABLE_PERM_UPDATE, Err(crate::AE_INVALID_OPTION)),
            (crate::ADS_FREE_TABLE, Err(crate::AE_INVALID_OPTION)),
            (0x0400, Err(crate::AE_INVALID_OPTION)),
            (crate::ADS_TEMP_TABLE, Err(crate::TEMP_TABLE_REFUSED)),
            (0xffff_ffff, Err(crate::AE_INVALID_OPTION)),
        ];
        for (options, expected) in cases {
            let (rc, table) = open_as(connect, c"all_types.adt", ADS_ADT, ADS_ANSI, options);
            let reported = match rc {
                AE_SUCCESS => {
                    assert_eq!(
                        unsafe { AdsGetTableOpenOptions(table, &mut value) },
                        AE_SUCCESS
                    );
                    assert_eq!(AdsCloseTable(table), AE_SUCCESS);
                    Ok(value)
                }
                rc => Err(rc),
            };
            assert_eq!(reported, expected, "{options:#x}");
        }
        let rc = unsafe { AdsGetTableOpenOptions(connect, &mut value) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn table_connection() {
        let connect = fixtures::connect("all_types");
        let (rc, table) = open(connect, c"all_types.adt", ADS_ADT);
        assert_eq!(rc, AE_SUCCESS);
        let mut owner = 0;
        assert_eq!(
            unsafe { AdsGetTableConnection(table, &mut owner) },
            AE_SUCCESS
        );
        assert_eq!(owner, connect);

        let path = std::ffi::CString::new(
            fixtures::dir("all_types")
                .join("all_types.adt")
                .to_str()
                .unwrap(),
        );
        let path = path.unwrap();
        // As in the original engine, a table on its own needs the local
        // server allowed.
        let _settings = fixtures::settings_lock();
        assert_eq!(crate::AdsSetServerType(0), AE_SUCCESS);
        assert_eq!(open(0, &path, ADS_ADT).0, crate::REMOTE_SERVER_NOT_FOUND);
        assert_eq!(crate::AdsSetServerType(crate::ADS_LOCAL_SERVER), AE_SUCCESS);
        let (rc, free) = open(0, &path, ADS_ADT);
        assert_eq!(crate::AdsSetServerType(0), AE_SUCCESS);
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(
            unsafe { AdsGetTableConnection(free, &mut owner) },
            AE_SUCCESS
        );
        assert_eq!(owner, 0);
        assert_eq!(AdsCloseTable(free), AE_SUCCESS);

        let rc = unsafe { AdsGetTableConnection(connect, &mut owner) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn open_errors() {
        let connect = fixtures::connect("all_types");
        assert_eq!(open(connect, c"missing.adt", ADS_ADT).0, AE_FILE_NOT_FOUND);
        assert_eq!(
            open(connect, c"all_types.json", ADS_ADT).0,
            AE_CORRUPT_TABLE
        );
        let encrypted = c"../dictionary_encrypted/rc4_secret.adt";
        // A server keeps tables inside the connection's folder; a
        // dictionary's table with an encrypted structure, opened as a free
        // table, has the wrong password (`fixtures/write/encryption/freeopen.json`).
        let expected = if fixtures::is_remote() {
            crate::AE_FILE_NOT_ON_SERVER
        } else {
            INVALID_ENCRYPTION_PASSWORD
        };
        assert_eq!(open(connect, encrypted, ADS_ADT).0, expected);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn lengths_indexes_and_close() {
        let connect = fixtures::connect("all_types");
        let (rc, table) = open(connect, c"all_types.adt", ADS_ADT);
        assert_eq!(rc, AE_SUCCESS);

        let mut length = 0xffff_ffff;
        assert_eq!(
            unsafe { AdsGetRecordLength(table, &mut length) },
            AE_SUCCESS
        );
        assert_eq!(length, 213);
        assert_eq!(
            unsafe { AdsGetRecordLength(connect, &mut length) },
            AE_INVALID_TABLE_HANDLE
        );

        let mut count = 0xffff;
        assert_eq!(unsafe { AdsGetNumFields(table, &mut count) }, AE_SUCCESS);
        assert_eq!(count, 24);
        assert_eq!(
            unsafe { AdsGetNumFields(connect, &mut count) },
            AE_INVALID_TABLE_HANDLE
        );

        let mut records = 0;
        assert_eq!(
            unsafe { AdsGetRecordCount(table, 2, &mut records) },
            AE_SUCCESS
        );
        assert_eq!(records, 5);
        assert_eq!(
            unsafe { AdsGetRecordCount(connect, 2, &mut records) },
            AE_INVALID_HANDLE
        );
        assert_eq!(records, 5);

        let mut length = 0;
        assert_eq!(
            unsafe { AdsGetRecordLength(table, &mut length) },
            AE_SUCCESS
        );
        let mut record = vec![0xeeu8; length as usize + 2];
        let rc =
            unsafe { crate::AdsGetNullRecord(table, record.as_mut_ptr(), record.len() as u32) };
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(record[..5], [0; 5], "the record prefix");
        assert_eq!(record[length as usize..], [0xee; 2], "past the record");
        let rc = unsafe { crate::AdsGetNullRecord(table, record.as_mut_ptr(), length - 1) };
        assert_eq!(rc, AE_INSUFFICIENT_BUFFER);
        let rc = unsafe { crate::AdsGetNullRecord(connect, record.as_mut_ptr(), length) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);

        let mut indexes = [0xffff; 4];
        let mut len = indexes.len() as u16;
        let rc = unsafe { AdsGetAllIndexes(table, indexes.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(len, 0);
        assert_eq!(indexes, [0xffff; 4]);
        let rc = unsafe { AdsGetAllIndexes(connect, indexes.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);
        let mut count = 0xffff;
        assert_eq!(unsafe { AdsGetNumIndexes(table, &mut count) }, AE_SUCCESS);
        assert_eq!(count, 0);
        assert_eq!(
            unsafe { AdsGetNumIndexes(connect, &mut count) },
            AE_INVALID_TABLE_HANDLE
        );

        let mut encrypted = 0xffff;
        assert_eq!(
            unsafe { crate::AdsIsTableEncrypted(table, &mut encrypted) },
            AE_SUCCESS
        );
        assert_eq!(encrypted, ADS_FALSE);
        let rc = unsafe { crate::AdsIsTableEncrypted(connect, &mut encrypted) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);

        let mut length = 0;
        assert_eq!(AdsCloseTable(table), AE_SUCCESS);
        assert_eq!(AdsCloseTable(table), AE_INVALID_TABLE_HANDLE);
        assert_eq!(
            unsafe { AdsGetRecordLength(table, &mut length) },
            AE_INVALID_TABLE_HANDLE
        );
        assert_eq!(AdsCloseTable(connect), AE_INVALID_TABLE_HANDLE);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn collation_round_trip() {
        let connect = fixtures::connect("all_types");
        let (rc, table) = open(connect, c"all_types.adt", ADS_ADT);
        assert_eq!(rc, AE_SUCCESS);

        let mut buf = [0xffu8; 8];
        let mut len = buf.len() as u16;
        assert_eq!(
            unsafe { AdsGetTableCollation(table, buf.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert_eq!(&buf[..5], b"ansi\0");
        assert_eq!(len, 4);

        let mut len = buf.len() as u16;
        let rc = unsafe { AdsGetTableCollation(connect, buf.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn opening_with_the_connections_settings() {
        let connect = fixtures::connect("adi_keys");
        let mut table = 0;
        for name in [c"keys.adt", c"keys"] {
            let name = name.as_ptr().cast_mut().cast();
            assert_eq!(
                unsafe { AdsOpenTable101(connect, name, &mut table) },
                AE_SUCCESS
            );
            let mut table_type = 0;
            assert_eq!(
                unsafe { AdsGetTableType(table, &mut table_type) },
                AE_SUCCESS
            );
            assert_eq!(table_type, ADS_ADT);
            assert_eq!(AdsCloseTable(table), AE_SUCCESS);
        }
        let name = c"keys.adt".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { AdsOpenTable101(0, name, &mut table) },
            AE_INVALID_HANDLE
        );
        let rc = unsafe { AdsOpenTable101(connect, name, std::ptr::null_mut()) };
        assert_eq!(rc, AE_INVALID_POINTER);
        let null = std::ptr::null_mut();
        let rc =
            unsafe { AdsOpenTable(connect, name, null, ADS_ADT, ADS_ANSI, 1, 2, 0, &mut table) };
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    /// As the original engine answers.
    #[test]
    fn memo_block_sizes() {
        let block_size = |table: AdsHandle| {
            let mut size = 0xEEEE;
            let rc = unsafe { AdsGetMemoBlockSize(table, &mut size) };
            (rc, size)
        };
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        assert_eq!(block_size(table), (AE_SUCCESS, 256));
        let rc = unsafe { AdsGetMemoBlockSize(table, std::ptr::null_mut()) };
        assert_eq!(rc, AE_INVALID_POINTER);
        assert_eq!(block_size(connect), (AE_INVALID_TABLE_HANDLE, 0xEEEE));
        assert_eq!(block_size(0x1234_5678), (AE_INVALID_TABLE_HANDLE, 0xEEEE));
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);

        // The memo file's block size: 512 in a DBT, from the header in an FPT.
        for (dir, table_type, size) in [
            ("ntx/dbf_types", ADS_NTX, 512),
            ("cdx/dbf_types", ADS_CDX, 64),
        ] {
            let connect = fixtures::connect(dir);
            let (rc, dbf) = open(connect, c"dbf_types.dbf", table_type);
            assert_eq!(rc, AE_SUCCESS);
            assert_eq!(block_size(dbf), (AE_SUCCESS, size), "{dir}");
            assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
        }

        let (connect, table) = fixtures::open("memo_blocks", c"memo1024.adt");
        assert_eq!(block_size(table), (AE_SUCCESS, 1024));
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);

        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        assert_eq!(block_size(table), (crate::AE_NO_MEMO_FILE, 0xEEEE));
        let mut order = 0;
        let name = c"BY_NAME".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { crate::AdsGetIndexHandle(table, name, &mut order) },
            AE_SUCCESS
        );
        assert_eq!(
            block_size(order),
            (AE_INVALID_TABLE_HANDLE, 0xEEEE),
            "an index order"
        );
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    /// The date the table's file was last written, in the date format; the
    /// memo size of DBF tables, which ADT tables lack.
    #[test]
    fn last_updates_and_memo_sizes() {
        let _settings = fixtures::settings_lock();
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let modified = std::fs::metadata(fixtures::dir("all_types/all_types.adt"))
            .and_then(|m| m.modified())
            .unwrap();
        let julian = tflip_engine::clock::local_date(modified);
        let ymd = tflip_storage::value::julian_to_ymd(julian);
        let want = format!("{:02}/{:02}/{:04}", ymd.month, ymd.day, ymd.year);
        let (mut buf, mut len) = ([0u8; 20], 20);
        let rc = unsafe { AdsGetLastTableUpdate(table, buf.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(&buf[..usize::from(len)], want.as_bytes());
        let mut len = 5;
        let rc = unsafe { AdsGetLastTableUpdate(table, buf.as_mut_ptr(), &mut len) };
        assert_eq!((rc, len), (AE_INSUFFICIENT_BUFFER, 11));
        let rc = unsafe { AdsGetLastTableUpdate(connect, buf.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);
        let mut size = 0x77;
        let rc = unsafe { AdsGetTableMemoSize(table, &mut size) };
        assert_eq!((rc, size), (crate::AE_NOT_IMPLEMENTED, 0x77));
        crate::AdsDisconnect(connect);
    }

    #[test]
    fn tables_handles_and_connections() {
        let _counting = fixtures::counting();
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        // Values on handles: none until set.
        let mut value = 0x77;
        assert_eq!(
            unsafe { crate::AdsGetHandleLong(table, &mut value) },
            AE_INVALID_HANDLE
        );
        assert_eq!(crate::AdsSetHandleLong(table, 42), AE_SUCCESS);
        assert_eq!(
            unsafe { crate::AdsGetHandleLong(table, &mut value) },
            AE_SUCCESS
        );
        assert_eq!(value, 42);
        assert_eq!(crate::AdsSetHandleLong(0, 1), AE_INVALID_HANDLE);

        // The tables open, by full path only.
        let (mut all, mut len) = ([0; 256], 256);
        assert_eq!(
            unsafe { AdsGetAllTables(all.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert!(all[..usize::from(len)].contains(&table));
        let mut path = [0u8; 600];
        let mut path_len = path.len() as u16;
        let rc = unsafe {
            crate::AdsGetTableFilename(
                table,
                3, // ADS_FULLPATHNAME
                path.as_mut_ptr(),
                &mut path_len,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        path[usize::from(path_len)] = 0;
        let mut found = 0x77;
        assert_eq!(
            unsafe { AdsGetTableHandle25(connect, path.as_mut_ptr(), &mut found) },
            AE_SUCCESS
        );
        assert_eq!(found, table);
        let name = c"all_types".as_ptr().cast_mut().cast();
        assert_eq!(unsafe { AdsGetTableHandle(name, &mut found) }, AE_NOT_FOUND);
        assert_eq!(found, 0);
        let (mut alias, mut len) = ([0u8; 32], 32);
        assert_eq!(
            unsafe { AdsGetTableAlias(table, alias.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert_eq!(&alias[..usize::from(len)], b"all_types");
        let (mut flag, mut number) = (0x77, 0x77);
        assert_eq!(unsafe { AdsGetTableLockType(table, &mut flag) }, AE_SUCCESS);
        assert_eq!(flag, ADS_PROPRIETARY_LOCKING);
        assert_eq!(
            unsafe { crate::AdsIsRecordLocked(table, 1, &mut flag) },
            AE_SUCCESS
        );
        assert_eq!(flag, ADS_FALSE);
        assert_eq!(
            unsafe { crate::AdsGetNumLocks(table, &mut number) },
            AE_SUCCESS
        );
        assert_eq!(number, 0);
        assert_eq!(
            unsafe { crate::AdsIsConnectionAlive(connect, &mut flag) },
            AE_SUCCESS
        );
        assert_eq!(flag, ADS_TRUE);
        let rc = unsafe { crate::AdsIsConnectionAlive(table, &mut flag) };
        assert_eq!(rc, AE_INVALID_CONNECTION_HANDLE);

        // Collations kept per connection.
        let (mut buf, mut len) = ([0u8; 32], 32);
        assert_eq!(
            unsafe { crate::AdsGetCollation(connect, buf.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert_eq!(len, 0);
        let german = c"GERMAN_VFP_CI_AS_1252".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { crate::AdsSetCollation(connect, german) },
            AE_SUCCESS
        );
        len = 32;
        assert_eq!(
            unsafe { crate::AdsGetCollation(connect, buf.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert_eq!(&buf[..usize::from(len)], b"GERMAN_VFP_CI_AS_1252");
        assert_eq!(
            crate::AdsNullTerminateStrings(1),
            crate::settings::NULL_TERMINATE_REFUSED
        );

        // Resetting the connection closes its tables.
        assert_eq!(crate::AdsResetConnection(connect), AE_SUCCESS);
        let mut length = 0;
        let rc = unsafe { crate::AdsGetRecordLength(table, &mut length) };
        assert_eq!(rc, crate::AE_INVALID_TABLE_HANDLE);
        crate::AdsDisconnect(connect);
    }

    crate::fixtures::remote_twins!(
        tables_handles_and_connections,
        memo_block_sizes,
        char_type_round_trip,
        collation_round_trip,
        lengths_indexes_and_close,
        open_errors,
        opening_with_the_connections_settings,
        open_options_round_trip,
        table_connection,
        table_type_round_trip,
        transaction_free_tables,
        full_text_indexes,
        record_count_options,
        last_updates_and_memo_sizes
    );
}
