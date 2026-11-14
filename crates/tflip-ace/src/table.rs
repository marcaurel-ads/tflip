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
    ADS_ADT, ADS_ANSI, ADS_CDX, ADS_CHECKRIGHTS, ADS_COMPATIBLE_LOCKING, ADS_DATABASE_TABLE,
    ADS_FALSE, ADS_IGNOREFILTERS, ADS_IGNORERIGHTS, ADS_LOCAL_SERVER, ADS_NTX, ADS_OEM,
    ADS_PROPRIETARY_LOCKING, ADS_REMOTE_SERVER, ADS_RESPECTFILTERS, ADS_RESPECTSCOPES, ADS_TRUE,
    ADS_VFP, AE_INVALID_CHARSETTYPE, AE_INVALID_CONNECTION_HANDLE, AE_INVALID_HANDLE,
    AE_INVALID_LOCKTYPE, AE_INVALID_OPTION, AE_INVALID_PATH, AE_INVALID_POINTER,
    AE_INVALID_RIGHTSOPTION, AE_INVALID_TABLE_HANDLE, AE_INVALID_TABLETYPE, AE_NO_MEMO_FILE,
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

/// The highest character type: `ADS_ANSI` and `ADS_OEM`, then `ace.h`'s
/// VFP collations, 3 to `ADS_MAX_CHAR_SETS`.
const MAX_CHAR_TYPE: Unsigned16 = 118;

/// The open option a table created or opened with `lock_type` reports as its
/// locking (`AdsGetTableLockType`): a Clipper or FoxPro table's is the lock
/// type it was given, and an ADT table's always proprietary
/// (`fixtures/errors/lock_types.json`).
pub(crate) fn locking_option(table_type: TableType, lock_type: Unsigned16) -> u32 {
    match lock_type == ADS_COMPATIBLE_LOCKING && table_type != TableType::Adt {
        true => tflip_engine::table::options::COMPATIBLE_LOCKING,
        false => 0,
    }
}

/// Checks the character type, lock type and rights option of
/// `AdsOpenTable…` (`create` false) or `AdsCreateTable…` (true), after the
/// table type, as the original engine does
/// (`fixtures/errors/table_options.json`). `None` if they are good, or the
/// call's return code.
///
/// The rights option is 1 or 2, the lock type 0 or 1, the character type 1
/// to 118. A VFP collation (3 to 118) on an NTX or CDX table is
/// `AE_INVALID_TABLETYPE`. `char_type` is `None` when `AdsOpenTable90` is
/// given a collation, which stands for the character type, unchecked. A
/// dictionary's table (`ADS_DATABASE_TABLE`) has its lock type checked
/// only: the dictionary has its character type and rights. An open checks rights, lock type, character type
/// in that order. A create checks the lock type last, and leaves the last
/// error alone when the rights option is wrong or the character type is
/// beyond 118, as the original engine does.
pub(crate) fn check_options(
    create: bool,
    table_type: TableType,
    char_type: Option<Unsigned16>,
    lock_type: Unsigned16,
    check_rights: Unsigned16,
) -> Option<Unsigned32> {
    use crate::last_error::{fail_detail, restore_pending};
    let without_trace = |code| {
        restore_pending();
        code
    };
    let lock = || {
        (!matches!(lock_type, ADS_COMPATIBLE_LOCKING | ADS_PROPRIETARY_LOCKING))
            .then(|| fail_detail(AE_INVALID_LOCKTYPE, "Parameter 6"))
    };
    if table_type == TableType::Database {
        return lock();
    }
    if !matches!(check_rights, ADS_CHECKRIGHTS | ADS_IGNORERIGHTS) {
        return Some(match create {
            true => without_trace(AE_INVALID_RIGHTSOPTION),
            false => fail_detail(AE_INVALID_RIGHTSOPTION, "Parameter 7"),
        });
    }
    if !create && let Some(code) = lock() {
        return Some(code);
    }
    // No character type to check (a collation stands for it): all is well.
    let char_type = char_type?;
    if char_type > MAX_CHAR_TYPE && create {
        return Some(without_trace(AE_INVALID_CHARSETTYPE));
    }
    if char_type == 0 || char_type > MAX_CHAR_TYPE {
        return Some(fail_detail(AE_INVALID_CHARSETTYPE, "Parameter 5"));
    }
    if char_type > ADS_OEM && matches!(table_type, TableType::Ntx | TableType::Cdx) {
        return Some(fail_detail(
            AE_INVALID_TABLETYPE,
            "Collation languages can only be specified with ADS_ADT and ADS_VFP table types.",
        ));
    }
    if create { lock() } else { None }
}

/// The character type and collation `AdsOpenTable90` opens a table with,
/// given a collation, as the original engine takes them
/// (`fixtures/errors/table_options.json`): `ANSI` or `OEM` (regardless of
/// case) makes the table's character type that of an NTX or CDX table; any
/// other collation there is `AE_INVALID_TABLETYPE`. An ADT or VFP table
/// takes a collation of the repository, or `ANSI` or `OEM`; others are
/// refused with 7147 ([`crate::settings::check_collation`]).
fn open_collation(
    table_type: TableType,
    char_type: Unsigned16,
    collation: Option<String>,
) -> Result<(Unsigned16, Option<String>), Unsigned32> {
    let Some(name) = collation.filter(|name| !name.is_empty()) else {
        return Ok((char_type, None));
    };
    let ansi_or_oem = match name {
        _ if name.eq_ignore_ascii_case("ANSI") => Some(ADS_ANSI),
        _ if name.eq_ignore_ascii_case("OEM") => Some(ADS_OEM),
        _ => None,
    };
    match table_type {
        TableType::Ntx | TableType::Cdx => match ansi_or_oem {
            Some(char_type) => Ok((char_type, Some(name))),
            None => Err(crate::last_error::fail_detail(
                AE_INVALID_TABLETYPE,
                "Collation languages can only be specified with ADS_ADT and ADS_VFP table types.",
            )),
        },
        TableType::Adt | TableType::Vfp if ansi_or_oem.is_none() => {
            let name = crate::settings::check_collation(name, false)?;
            Ok((char_type, Some(name)))
        }
        _ => Ok((char_type, Some(name))),
    }
}

/// `AdsOpenTable90`: opens the table `name` on a connection.
///
/// `table_type` is one of `ADS_NTX`, `ADS_CDX`, `ADS_ADT`, `ADS_VFP` or
/// `ADS_DATABASE_TABLE`; `char_type` is `ADS_ANSI`, `ADS_OEM` or a VFP
/// collation (ADT and VFP tables only); `options` are `ADS_EXCLUSIVE`,
/// `ADS_READONLY`, `ADS_SHARED`, ... flags. Options outside these fail as
/// in the original engine ([`check_options`]).
///
/// Reads the table's structure and hands out a handle that knows how it was
/// opened (type, character set, collation and options); the lock type is
/// checked, then ignored (the alias and rights checking are kept for
/// `AdsGetTableAlias` and `AdsGetTableRights`). See
/// [`tflip_engine::table::open`].
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
    lock_type: Unsigned16,
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
    // A collation stands for the character type, which is not checked then.
    let checked_char_type = match &collation {
        Some(collation) if !collation.is_empty() => None,
        _ => Some(char_type),
    };
    if let Some(code) = check_options(
        false,
        table_type,
        checked_char_type,
        lock_type,
        check_rights,
    ) {
        return code;
    }
    let (char_type, collation) = match open_collation(table_type, char_type, collation) {
        Ok(pair) => pair,
        Err(code) => return code,
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
        // The locking bit comes from the lock type, not the options.
        options: (options & !tflip_engine::table::options::COMPATIBLE_LOCKING)
            | locking_option(table_type, lock_type),
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

/// `AdsGetTableLockType`: the locking a table was opened with
/// (`ADS_COMPATIBLE_LOCKING` or `ADS_PROPRIETARY_LOCKING`), always
/// proprietary for an ADT table, as in the original engine
/// (`fixtures/errors/lock_types.json`).
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
    // Compatible locking is what a Clipper or FoxPro table is opened or
    // created with by lock type, or by `AdsOpenTable101` with its
    // connection's `LockMode`; an ADT table is always proprietary.
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

    /// `tflipd --map`: a path of the original server's under a mapped one
    /// is the same path in its share, for a table's name and a connection's
    /// path; any other absolute path is still refused.
    #[test]
    fn mapped_server_paths() {
        fixtures::remotely_with(|| {
            let connect = fixtures::connect("indexes");
            let open = |connect, name: &CStr| {
                let mut table = 0;
                let rc = unsafe {
                    AdsOpenTable(
                        connect,
                        name.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        crate::ADS_ADT,
                        1,
                        0,
                        1,
                        ADS_SHARED,
                        &mut table,
                    )
                };
                if rc == AE_SUCCESS {
                    AdsCloseTable(table);
                }
                rc
            };
            assert_eq!(
                open(connect, c"X:\\TFLIP-FIXTURES\\indexes\\indexed.adt"),
                AE_SUCCESS
            );
            assert_eq!(
                open(connect, c"x:/tflip-fixtures/INDEXES/indexed.adt"),
                AE_SUCCESS
            );
            for outside in [
                c"X:\\other\\indexed.adt",
                c"X:\\tflip-fixtures-2\\indexes\\indexed.adt",
                c"X:\\tflip-fixtures\\..\\indexed.adt",
            ] {
                assert_eq!(
                    open(connect, outside),
                    crate::AE_FILE_NOT_ON_SERVER,
                    "{outside:?}"
                );
            }
            crate::AdsDisconnect(connect);
            let path = format!(
                r"\\127.0.0.1:{}\X:\tflip-fixtures\indexes",
                fixtures::server()
            );
            let connect = fixtures::connect_to(&path);
            assert_eq!(open(connect, c"indexed.adt"), AE_SUCCESS);
            crate::AdsDisconnect(connect);
        });
    }

    /// `[paths] relative = cwd`: a bare table name is looked for in the
    /// current directory, not the connection's folder, and an absolute one
    /// is found either way.
    #[test]
    fn relative_names_follow_the_setting() {
        let _settings = fixtures::settings_lock();
        let (source, _dir) = fixtures::scratch_from(
            "relative_names_follow_the_setting",
            &fixtures::dir("indexes"),
        );
        let connect = fixtures::connect_to(&source);
        let absolute = std::ffi::CString::new(
            std::path::Path::new(&source)
                .join("indexed.adt")
                .to_string_lossy()
                .into_owned(),
        )
        .unwrap();
        let open = |name: &CStr| {
            let mut table = 0;
            let rc = unsafe {
                AdsOpenTable(
                    connect,
                    name.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    crate::ADS_ADT,
                    1,
                    0,
                    1,
                    ADS_SHARED,
                    &mut table,
                )
            };
            if rc == AE_SUCCESS {
                AdsCloseTable(table);
            }
            rc
        };
        let session = &*crate::LOCAL;
        assert_eq!(open(c"indexed.adt"), AE_SUCCESS);
        tflip_engine::settings::set_cwd_relative(session, true);
        // The tests run in the crate's folder, which has no such table.
        let in_cwd = open(c"indexed.adt");
        let by_path = open(&absolute);
        tflip_engine::settings::set_cwd_relative(session, false);
        assert_eq!((in_cwd, by_path), (AE_FILE_NOT_FOUND, AE_SUCCESS));
        assert_eq!(open(c"indexed.adt"), AE_SUCCESS);
        crate::AdsDisconnect(connect);
    }

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

    /// As in the original engine: the table, character, lock and rights
    /// options of `AdsCreateTable`, `AdsCreateTable90`, `AdsOpenTable` and
    /// `AdsOpenTable90`, the order they are checked in, which failures
    /// leave the last error alone, the collations of `AdsOpenTable90` with
    /// the character types they make, and a dictionary connection's opens
    /// (`fixtures/errors/table_options.json`, from
    /// `tools/ace/errors/table_options.c`, whose steps this repeats).
    ///
    /// Without a collation repository, an unknown
    /// collation cannot be told from a known one, so the 7147 rows are left
    /// out then.
    #[test]
    fn table_options() {
        use serde_json::Value as Json;
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/errors/table_options.json");
        let rows: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, dir) = fixtures::scratch_from("table_options", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let fields = c"ID,N,4,0".as_ptr().cast_mut().cast();
        let create = |name: &str, table_type, char_type, lock_type, rights, v90: bool| {
            let name = std::ffi::CString::new(name).unwrap();
            let name = name.as_ptr().cast_mut().cast();
            let mut table = 0;
            let null = std::ptr::null_mut();
            let rc = unsafe {
                match v90 {
                    true => crate::AdsCreateTable90(
                        connect, name, null, table_type, char_type, lock_type, rights, 0, fields,
                        0, null, &mut table,
                    ),
                    false => crate::AdsCreateTable(
                        connect, name, null, table_type, char_type, lock_type, rights, 0, fields,
                        &mut table,
                    ),
                }
            };
            (rc, table)
        };
        let base_name = |table_type| match table_type {
            1 => "b1.dbf",
            3 => "b3.adt",
            4 => "b4.dbf",
            _ => "b2.dbf",
        };
        for table_type in 1..=4 {
            let (rc, table) = create(base_name(table_type), table_type, 1, 1, 1, false);
            assert_eq!(rc, AE_SUCCESS);
            AdsCloseTable(table);
        }
        let (rc, keep) = create("keep.adt", 3, 1, 1, 1, false);
        assert_eq!(rc, AE_SUCCESS);
        let provoke = || {
            let mut n = 0;
            unsafe { AdsGetRecordCount(keep, 0, &mut n) };
        };
        let last_error = || {
            let mut last = 0;
            let (mut buf, mut len) = ([0u8; 600], 600u16);
            unsafe { crate::AdsGetLastError(&mut last, buf.as_mut_ptr(), &mut len) };
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            (
                u64::from(last),
                String::from_utf8_lossy(&buf[..end]).into_owned(),
            )
        };
        let mut problems = Vec::new();
        let mut made = 0;
        let mut dictionary = 0;
        provoke();
        let (last, text) = last_error();
        assert_eq!(
            (last, &*text),
            (
                rows[0]["last"].as_u64().unwrap(),
                rows[0]["text"].as_str().unwrap()
            )
        );
        for row in &rows[1..] {
            let call = row["call"].as_str().unwrap();
            let option = |key: &str| row[key].as_u64().unwrap() as Unsigned16;
            let (table_type, char_type, lock_type, rights) = (
                option("type"),
                option("char"),
                option("lock"),
                option("rights"),
            );
            let repository = crate::config::collation_dir()
                .is_some_and(|dir| dir.join("adscollate.adt").exists());
            if row["rc"] == 7147 && !repository {
                continue;
            }
            let in_dictionary = call == "AdsOpenTable (dictionary)";
            if in_dictionary && dictionary == 0 {
                let path = std::ffi::CString::new(dir.join("test.add").to_str().unwrap()).unwrap();
                let null = std::ptr::null_mut();
                let rc = unsafe {
                    crate::AdsDDCreate(path.as_ptr().cast_mut().cast(), 0, null, &mut dictionary)
                };
                assert_eq!(rc, AE_SUCCESS);
                let (rc, table) = create("dt.adt", 3, 1, 1, 1, false);
                assert_eq!(rc, AE_SUCCESS);
                AdsCloseTable(table);
                let rc = unsafe {
                    crate::AdsDDAddTable(
                        dictionary,
                        c"t".as_ptr().cast_mut().cast(),
                        c"dt.adt".as_ptr().cast_mut().cast(),
                        3,
                        1,
                        null,
                        null,
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
            }
            provoke();
            let (rc, table) = match call {
                "AdsCreateTable" | "AdsCreateTable90" => {
                    made += 1;
                    let name = format!("c{}", made - 1);
                    create(
                        &name,
                        table_type,
                        char_type,
                        lock_type,
                        rights,
                        call.ends_with("90"),
                    )
                }
                _ => {
                    let name = match in_dictionary && table_type == 0 {
                        true => "t",
                        false => base_name(table_type),
                    };
                    let name = std::ffi::CString::new(name).unwrap();
                    let connect = if in_dictionary { dictionary } else { connect };
                    let name = name.as_ptr().cast_mut().cast();
                    let collation = row["collation"]
                        .as_str()
                        .map(|text| std::ffi::CString::new(text).unwrap());
                    let collation = collation
                        .as_ref()
                        .map_or(std::ptr::null_mut(), |text| text.as_ptr().cast_mut().cast());
                    let (null, mut table) = (std::ptr::null_mut(), 0);
                    let options = ADS_SHARED | ADS_READONLY;
                    let rc = unsafe {
                        match call {
                            "AdsOpenTable90" => AdsOpenTable90(
                                connect, name, null, table_type, char_type, lock_type, rights,
                                options, collation, &mut table,
                            ),
                            _ => AdsOpenTable(
                                connect, name, null, table_type, char_type, lock_type, rights,
                                options, &mut table,
                            ),
                        }
                    };
                    (rc, table)
                }
            };
            let (last, text) = last_error();
            let (mut got_type, mut got_char_type) = (0, 0);
            if table != 0 {
                unsafe { crate::AdsGetTableType(table, &mut got_type) };
                unsafe { AdsGetTableCharType(table, &mut got_char_type) };
                AdsCloseTable(table);
            }
            let want = (
                row["rc"].as_u64().unwrap(),
                row["handle"].as_u64().unwrap() != 0,
                row["last"].as_u64().unwrap(),
                row["text"].as_str().unwrap(),
                row["table_type"].as_u64().unwrap(),
                row["char_type"].as_u64().unwrap(),
            );
            let got = (
                u64::from(rc),
                table != 0,
                last,
                &*text,
                u64::from(got_type),
                u64::from(got_char_type),
            );
            if got != want {
                problems.push(format!(
                    "{call} type {table_type} char {char_type} lock {lock_type} rights \
                     {rights} collation {}: got {got:?}, want {want:?}",
                    row["collation"]
                ));
            }
        }
        AdsCloseTable(keep);
        crate::AdsDisconnect(dictionary);
        crate::AdsDisconnect(connect);
        assert!(
            problems.is_empty(),
            "{} problems:\n{}",
            problems.len(),
            problems.join("\n")
        );
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
        // As in the original engine (`fixtures/errors/table_options.json`):
        // ADT tables are ANSI whatever they are opened as. A CDX table keeps
        // ANSI or OEM as given, refuses a VFP collation (3), and is ANSI with
        // the collation `ansi`, which `open_as` gives.
        let name = c"dbf_types".as_ptr().cast_mut().cast();
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
            assert_eq!(value, ADS_ANSI);
            let mut table = 0;
            let null = std::ptr::null_mut();
            let rc =
                unsafe { AdsOpenTable(dbf, name, null, ADS_CDX, char_type, 1, 2, 0, &mut table) };
            if char_type == 3 {
                assert_eq!(rc, AE_INVALID_TABLETYPE);
                continue;
            }
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

    /// As in the original engine: a Visual FoxPro table opened or made with
    /// `ADS_OEM`, the collation `OEM` or a collation of an OEM code page is an
    /// OEM table, whose text is kept in the OEM code page and keyed by its
    /// bytes, as a FoxPro table's (`fixtures/dbf/vfp_oem.json`, from
    /// `tools/ace/oem/vfp_oem.c`, whose steps this repeats). The orders of a
    /// table with a collation (char types 3 to 118, or its name) are not
    /// collated yet: there only the text and the character type are checked.
    #[test]
    fn vfp_oem_tables() {
        use serde_json::{Value as Json, json};
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/dbf/vfp_oem.json");
        let want: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, dir) = fixtures::scratch_from("vfp_oem_tables", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let text = |s: &[u8]| std::ffi::CString::new(s).unwrap();
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let values: [&[u8]; 8] = [
            b"A", b"\xC4", b"\xE4", b"a", b"z", b"\xE9", b"\xDF", b"\x8E",
        ];
        let cases: [(&str, u16, u16, &str); 11] = [
            ("CDX oem", crate::ADS_CDX, 2, ""),
            ("VFP ansi", crate::ADS_VFP, 1, ""),
            ("VFP oem", crate::ADS_VFP, 2, ""),
            ("VFP 117", crate::ADS_VFP, 117, ""),
            ("VFP 118", crate::ADS_VFP, 118, ""),
            ("VFP 26", crate::ADS_VFP, 26, ""),
            ("VFP 12", crate::ADS_VFP, 12, ""),
            ("VFP collation oem", crate::ADS_VFP, 1, "oem"),
            (
                "VFP collation 12",
                crate::ADS_VFP,
                1,
                "GENERAL_VFP_CI_AS_1252",
            ),
            (
                "VFP collation 26",
                crate::ADS_VFP,
                1,
                "GENERAL_VFP_CI_AS_437",
            ),
            (
                "VFP collation 117",
                crate::ADS_VFP,
                1,
                "TURKISH_ADS_CS_AS_857",
            ),
        ];
        let field = text(b"C");
        let column = || field.as_ptr().cast_mut().cast::<u8>();
        let read = |table: AdsHandle, order: AdsHandle, row: &mut Json| {
            let mut char_type = 0;
            unsafe { AdsGetTableCharType(table, &mut char_type) };
            let (mut strings, mut wide, mut keys) = (vec![], vec![], vec![]);
            for record in 1..=8 {
                crate::AdsGotoRecord(table, record);
                let (mut buf, mut len) = ([0u8; 64], 64u32);
                let rc =
                    unsafe { crate::AdsGetString(table, column(), buf.as_mut_ptr(), &mut len, 2) };
                strings.push(match rc {
                    0 => hex(&buf[..len as usize]),
                    rc => format!("E{rc}"),
                });
                let (mut buf, mut len) = ([0u16; 64], 64u32);
                let rc =
                    unsafe { crate::AdsGetStringW(table, column(), buf.as_mut_ptr(), &mut len, 2) };
                wide.push(match rc {
                    0 => buf[..len as usize]
                        .iter()
                        .map(|u| format!("{u:04x}"))
                        .collect(),
                    rc => format!("E{rc}"),
                });
                let (mut key, mut len) = ([0u8; 64], 64u16);
                let rc = unsafe { crate::AdsExtractKey(order, key.as_mut_ptr(), &mut len) };
                keys.push(match rc {
                    0 => hex(&key[..usize::from(len)]),
                    rc => format!("E{rc}"),
                });
            }
            let mut walk = vec![];
            crate::AdsGotoTop(order);
            for _ in 0..10 {
                let (mut eof, mut recno) = (0, 0);
                unsafe { crate::AdsAtEOF(table, &mut eof) };
                if eof != 0 {
                    break;
                }
                unsafe { crate::AdsGetRecordNum(table, 2, &mut recno) };
                walk.push(recno);
                crate::AdsSkip(order, 1);
            }
            let mut found = 7;
            crate::AdsGotoRecord(table, 1);
            let rc =
                unsafe { crate::AdsSeek(order, b"\xE4".as_ptr().cast_mut(), 1, 1, 2, &mut found) };
            let (mut eof, mut recno) = (0, 0);
            unsafe { crate::AdsAtEOF(table, &mut eof) };
            unsafe { crate::AdsGetRecordNum(table, 2, &mut recno) };
            let at = if eof != 0 { json!("E") } else { json!(recno) };
            let filter = text(b"C = '\xE4'");
            let rc_filter =
                unsafe { crate::AdsSetFilter(table, filter.as_ptr().cast_mut().cast()) };
            assert_eq!(rc_filter, 0);
            let mut count = 0;
            crate::AdsGotoTop(table);
            for _ in 0..10 {
                let mut eof = 0;
                unsafe { crate::AdsAtEOF(table, &mut eof) };
                if eof != 0 {
                    break;
                }
                count += 1;
                crate::AdsSkip(table, 1);
            }
            crate::AdsClearFilter(table);
            row["char_type"] = json!(char_type);
            row["strings"] = json!(strings);
            row["wide"] = json!(wide);
            row["keys"] = json!(keys);
            row["walk"] = json!(walk);
            row["seek"] = json!([rc, found, at]);
            row["filter"] = json!(count);
        };
        // The field's bytes in the closed table's file.
        let raw = |name: &str, row: &mut Json| {
            let bytes = std::fs::read(dir.join(format!("{name}.dbf"))).unwrap();
            let start = usize::from(u16::from_le_bytes([bytes[8], bytes[9]]));
            let size = usize::from(u16::from_le_bytes([bytes[10], bytes[11]]));
            let fields: Vec<String> = (0..8)
                .map(|r| hex(&bytes[start + r * size + 1..start + r * size + 7]))
                .collect();
            row["raw"] = json!(fields);
        };
        let mut got = Vec::new();
        for (c, (case, table_type, char_type, collation)) in cases.into_iter().enumerate() {
            let name = format!("v{c}");
            let (table_name, fields, collation_text) = (
                text(name.as_bytes()),
                text(b"C,C,6,0"),
                text(collation.as_bytes()),
            );
            let collation_ptr = match collation {
                "" => std::ptr::null_mut(),
                _ => collation_text.as_ptr().cast_mut().cast(),
            };
            let mut table = 0;
            let rc = unsafe {
                crate::AdsCreateTable90(
                    connect,
                    table_name.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    table_type,
                    char_type,
                    1,
                    1,
                    0,
                    fields.as_ptr().cast_mut().cast(),
                    0,
                    collation_ptr,
                    &mut table,
                )
            };
            assert_eq!(rc, AE_SUCCESS, "{case}");
            for value in values {
                crate::AdsAppendRecord(table);
                unsafe {
                    crate::AdsSetString(
                        table,
                        column(),
                        value.as_ptr().cast_mut(),
                        value.len() as Unsigned32,
                    )
                };
                crate::AdsWriteRecord(table);
            }
            let (tag, empty) = (text(b"K"), text(b""));
            let mut order = 0;
            let rc = unsafe {
                crate::AdsCreateIndex61(
                    table,
                    table_name.as_ptr().cast_mut().cast(),
                    tag.as_ptr().cast_mut().cast(),
                    column(),
                    empty.as_ptr().cast_mut().cast(),
                    empty.as_ptr().cast_mut().cast(),
                    2,
                    512,
                    &mut order,
                )
            };
            assert_eq!(rc, AE_SUCCESS, "{case}");
            let mut row = json!({"case": case, "step": "create", "rc": 0});
            read(table, order, &mut row);
            AdsCloseTable(table);
            raw(&name, &mut row);
            got.push(row);
            for step in ["ansi", "oem", "own"] {
                let mut table = 0;
                let rc = match step {
                    "own" => unsafe {
                        AdsOpenTable90(
                            connect,
                            table_name.as_ptr().cast_mut().cast(),
                            std::ptr::null_mut(),
                            table_type,
                            char_type,
                            1,
                            1,
                            0,
                            collation_ptr,
                            &mut table,
                        )
                    },
                    _ => unsafe {
                        AdsOpenTable(
                            connect,
                            table_name.as_ptr().cast_mut().cast(),
                            std::ptr::null_mut(),
                            table_type,
                            if step == "ansi" { ADS_ANSI } else { ADS_OEM },
                            1,
                            1,
                            0,
                            &mut table,
                        )
                    },
                };
                let mut row = json!({"case": case, "step": step, "rc": rc});
                if rc == 0 {
                    let mut order = 0;
                    unsafe {
                        crate::AdsGetIndexHandle(table, tag.as_ptr().cast_mut().cast(), &mut order)
                    };
                    read(table, order, &mut row);
                    AdsCloseTable(table);
                    raw(&name, &mut row);
                }
                got.push(row);
            }
        }
        crate::AdsDisconnect(connect);
        assert_eq!(want.len(), got.len());
        // A collation's orders and comparisons, and the refusal to open its
        // table under another, are not the original's yet.
        let collated = |case: &str| {
            !matches!(
                case,
                "CDX oem" | "VFP ansi" | "VFP oem" | "VFP collation oem"
            )
        };
        let (mut set_aside, mut problems) = (0, vec![]);
        for (want, got) in want.iter().zip(&got) {
            let case = want["case"].as_str().unwrap();
            let same = match collated(case) {
                false => want == got,
                true if want["rc"] != 0 => {
                    set_aside += 1;
                    continue;
                }
                true => ["rc", "char_type", "strings", "wide", "raw"]
                    .iter()
                    .all(|field| want[field] == got[field]),
            };
            if !same {
                problems.push(format!("want {want}\n got {got}"));
            }
        }
        // 7 collated cases, opened as ANSI and as OEM. When this changes,
        // compare those rows too.
        assert_eq!(set_aside, 14, "rows of collated tables set aside");
        assert!(
            problems.is_empty(),
            "{} differ:\n{}",
            problems.len(),
            problems.join("\n")
        );
    }

    /// As in the original engine: a DBF table whose header dates its last
    /// update with a month outside 1 to 12 or a day outside 1 to 31 does not
    /// open (7016, with the table's path), whatever the year and whether or
    /// not the day exists in the month (`fixtures/dbf/header_dates.json`,
    /// from `tools/ace/dbf/header_dates.c`, whose files this writes again).
    #[test]
    fn dbf_header_dates() {
        use serde_json::{Value as Json, json};
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/dbf/header_dates.json");
        let want: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, dir) = fixtures::scratch_from("dbf_header_dates", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let mut got = Vec::new();
        for (n, row) in want.iter().enumerate() {
            let (table_type, vfp) = match row["type"].as_str().unwrap() {
                "NTX" => (crate::ADS_NTX, false),
                "CDX" => (crate::ADS_CDX, false),
                _ => (crate::ADS_VFP, true),
            };
            let date: Vec<u8> = row["date"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            let header = 32 + 32 + 1 + if vfp { 263 } else { 0 };
            let mut file = vec![0u8; 32];
            file[0] = if vfp { 0x30 } else { 0x03 };
            file[1..4].copy_from_slice(&date);
            file[4] = 1;
            file[8..10].copy_from_slice(&u16::to_le_bytes(header));
            file[10] = 5;
            let mut descriptor = [0u8; 32];
            descriptor[0] = b'C';
            descriptor[11] = b'C';
            descriptor[16] = 4;
            file.extend_from_slice(&descriptor);
            file.push(0x0D);
            file.resize(usize::from(header), 0);
            file.extend_from_slice(b" abcd\x1A");
            // Numbered within each type, as the probe numbers them.
            let n = n % (want.len() / 3);
            let name = format!("h{table_type}_{n}");
            std::fs::write(dir.join(format!("{name}.dbf")), &file).unwrap();
            let name_c = std::ffi::CString::new(name.clone()).unwrap();
            let mut table = 0;
            let rc = unsafe {
                crate::AdsOpenTable(
                    connect,
                    name_c.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    table_type,
                    1,
                    1,
                    1,
                    0,
                    &mut table,
                )
            };
            let mut out = json!({"type": row["type"], "date": row["date"], "rc": rc});
            if rc != 0 {
                let (mut code, mut buf, mut len) = (0, [0u8; 600], 600u16);
                unsafe { crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
                let error = String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned();
                // The path is the folder's here, the probe's there: only the
                // table's file name is compared.
                let file_name = format!("\\{name}.dbf");
                out["error"] = json!(match error.rsplit_once("  ") {
                    Some((text, path))
                        if path.ends_with(&file_name)
                            || path.ends_with(&format!("/{name}.dbf")) =>
                    {
                        format!("{text}  C:\\probe{file_name}")
                    }
                    _ => error,
                });
            } else {
                let (field, mut buf, mut len) = (c"C", [0u8; 16], 16u32);
                crate::AdsGotoTop(table);
                unsafe {
                    crate::AdsGetString(
                        table,
                        field.as_ptr().cast_mut().cast(),
                        buf.as_mut_ptr(),
                        &mut len,
                        2,
                    )
                };
                out["value"] = json!(String::from_utf8_lossy(&buf[..len as usize]));
                crate::AdsCloseTable(table);
            }
            got.push(out);
        }
        crate::AdsDisconnect(connect);
        let problems: Vec<String> = want
            .iter()
            .zip(&got)
            .filter(|(want, got)| want != got)
            .map(|(want, got)| format!("want {want}\n got {got}"))
            .collect();
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// The locking a table reports after it is created or opened with each
    /// lock type, as in the original engine (`fixtures/errors/lock_types.json`,
    /// from `tools/ace/errors/lock_types.c`, whose steps this repeats).
    #[test]
    fn lock_types() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/errors/lock_types.json");
        let want: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, _dir) = fixtures::scratch_from("lock_types", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let mut got = Vec::new();
        let mut row = |kind: &str, step: &str, lock: u16, rc: u32, table| {
            let mut lock_type = 9;
            if rc == AE_SUCCESS {
                unsafe { AdsGetTableLockType(table, &mut lock_type) };
                crate::AdsCloseTable(table);
            }
            got.push(
                serde_json::json!({"type": kind, "step": step, "lock": lock, "rc": rc,
                                        "lock_type": lock_type}),
            );
        };
        for (kind, table_type) in [
            ("NTX", crate::ADS_NTX),
            ("CDX", crate::ADS_CDX),
            ("ADT", crate::ADS_ADT),
            ("VFP", crate::ADS_VFP),
        ] {
            for lock in 0..2u16 {
                let name = std::ffi::CString::new(format!("l{table_type}_{lock}")).unwrap();
                let mut table = 0;
                let rc = unsafe {
                    crate::AdsCreateTable(
                        connect,
                        name.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        table_type,
                        1,
                        lock,
                        1,
                        0,
                        c"ID,N,4,0".as_ptr().cast_mut().cast(),
                        &mut table,
                    )
                };
                row(kind, "create", lock, rc, table);
                for open in 0..2u16 {
                    let mut table = 0;
                    let rc = unsafe {
                        AdsOpenTable(
                            connect,
                            name.as_ptr().cast_mut().cast(),
                            std::ptr::null_mut(),
                            table_type,
                            1,
                            open,
                            1,
                            0,
                            &mut table,
                        )
                    };
                    row(kind, "open", open, rc, table);
                }
            }
        }
        crate::AdsDisconnect(connect);
        assert_eq!(want, got);
    }

    /// What `AdsCreateTable` does where the table's file exists already, as
    /// in the original engine: refused (7040) while a handle has the table
    /// open, else replaced, the structural index file deleted
    /// (`fixtures/errors/create_existing.json`, from
    /// `tools/ace/write/create_existing.c`, whose steps this repeats).
    #[test]
    fn create_over_existing_tables() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/errors/create_existing.json");
        let want: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, dir) =
            fixtures::scratch_from("create_over_existing_tables", &fixtures::empty_dir());
        let folder = dir;
        let (one, two) = (fixtures::connect_to(&source), fixtures::connect_to(&source));
        let create = |connect, name: &std::ffi::CStr, table_type, fields: &std::ffi::CStr| {
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
            (rc, table)
        };
        let open = |connect, name: &std::ffi::CStr, table_type, options| {
            let mut table = 0;
            unsafe {
                AdsOpenTable(
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
        };
        let types = [
            ("NTX", ["dbf", "dbt", "ntx"], crate::ADS_NTX, 0),
            ("CDX", ["dbf", "fpt", "cdx"], crate::ADS_CDX, 2),
            ("ADT", ["adt", "adm", "adi"], crate::ADS_ADT, 2),
            ("VFP", ["dbf", "fpt", "cdx"], crate::ADS_VFP, 2),
        ];
        let scenarios = [
            "closed",
            "open shared",
            "open exclusive",
            "open here",
            "junk",
        ];
        let mut got = Vec::new();
        for (kind, exts, table_type, index_options) in types {
            for (s, scenario) in scenarios.into_iter().enumerate() {
                let stem = format!("e{table_type}_{s}");
                let name = std::ffi::CString::new(stem.clone()).unwrap();
                let (rc, table) = create(one, &name, table_type, c"NAME,Character,20,0;M,Memo");
                assert_eq!(rc, AE_SUCCESS);
                for _ in 0..2 {
                    crate::AdsAppendRecord(table);
                    unsafe {
                        crate::AdsSetString(
                            table,
                            c"NAME".as_ptr().cast_mut().cast(),
                            c"abc".as_ptr().cast_mut().cast(),
                            3,
                        );
                        crate::AdsSetString(
                            table,
                            c"M".as_ptr().cast_mut().cast(),
                            c"memo".as_ptr().cast_mut().cast(),
                            4,
                        );
                    }
                    crate::AdsWriteRecord(table);
                }
                let bag = std::ffi::CString::new(format!("{stem}.{}", exts[2])).unwrap();
                let mut order = 0;
                let rc = unsafe {
                    crate::AdsCreateIndex61(
                        table,
                        bag.as_ptr().cast_mut().cast(),
                        c"N".as_ptr().cast_mut().cast(),
                        c"NAME".as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        index_options,
                        512,
                        &mut order,
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
                crate::AdsCloseTable(table);
                let kept = match s {
                    1 => open(two, &name, table_type, 0),
                    2 => open(two, &name, table_type, 1),
                    3 => open(one, &name, table_type, 0),
                    _ => 0,
                };
                if s == 4 {
                    for ext in exts {
                        let _ = std::fs::remove_file(folder.join(format!("{stem}.{ext}")));
                    }
                    std::fs::write(folder.join(format!("{stem}.{}", exts[0])), b"hello").unwrap();
                }
                let (rc, table) = create(
                    one,
                    &name,
                    table_type,
                    c"NAME,Character,20,0;AGE,Numeric,4,0",
                );
                let (mut count, mut other, mut fields, mut orders) = (9u32, 9u32, 9u16, 9u16);
                if rc == AE_SUCCESS {
                    unsafe {
                        crate::AdsGetRecordCount(table, 1, &mut count);
                        crate::AdsGetNumFields(table, &mut fields);
                        crate::AdsGetNumIndexes(table, &mut orders);
                    }
                    crate::AdsCloseTable(table);
                }
                if kept != 0 {
                    if unsafe { crate::AdsGetRecordCount(kept, 1, &mut other) } != AE_SUCCESS {
                        other = 9;
                    }
                    crate::AdsCloseTable(kept);
                }
                let files: Vec<&str> = exts
                    .into_iter()
                    .filter(|ext| folder.join(format!("{stem}.{ext}")).exists())
                    .collect();
                got.push(
                    serde_json::json!({"type": kind, "scenario": scenario, "rc": rc,
                    "count": count, "fields": fields, "orders": orders, "files": files,
                    "other_count": other}),
                );
            }
        }
        crate::AdsDisconnect(two);
        crate::AdsDisconnect(one);
        let problems: Vec<String> = want
            .iter()
            .zip(&got)
            .filter(|(want, got)| want != got)
            .map(|(want, got)| format!("want {want}\n got {got}"))
            .collect();
        assert_eq!(want.len(), got.len());
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    crate::fixtures::remote_twins!(
        create_over_existing_tables,
        lock_types,
        vfp_oem_tables,
        dbf_header_dates,
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
