//! SQL statements: statement handles, preparing and executing SQL, and the
//! cursors it returns, which are table handles (see
//! [`tflip_engine::statement`]), and the settings of the tables a statement
//! opens (`AdsStmtSet*`).

use std::ffi::CStr;

use tflip_engine::handle::HandleKind;
use tflip_engine::statement::Setting;

use crate::backend::api;
use crate::filter::Encoding;
use crate::last_error::{fail, fail_handle};
use crate::{
    AE_INVALID_CONNECTION_HANDLE, AE_INVALID_POINTER, AE_INVALID_SQLSTATEMENT_HANDLE, AE_SUCCESS,
    AdsHandle, Unsigned8, Unsigned16, Unsigned32, Wchar, code, error_code, write_string_at,
};

/// `AdsCreateSQLStatement`: creates an SQL statement on a connection.
///
/// # Safety
///
/// `statement` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCreateSQLStatement(
    connect: AdsHandle,
    statement: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCreateSQLStatement");
    // SAFETY: `statement` is null or writable, as # Safety says.
    create_statement(connect, unsafe { statement.as_mut() })
}

/// Creates an SQL statement on a connection (see [`AdsCreateSQLStatement`]);
/// `None` for a null pointer.
fn create_statement(connect: AdsHandle, statement: Option<&mut AdsHandle>) -> Unsigned32 {
    let backend = api(connect);
    if backend.handle_kind(connect) != Ok(Some(HandleKind::Connection)) {
        return fail_handle(AE_INVALID_CONNECTION_HANDLE);
    }
    let Some(statement) = statement else {
        return crate::last_error::null_param(2);
    };
    match backend.create_statement(connect) {
        Ok(handle) => {
            *statement = handle;
            AE_SUCCESS
        }
        Err(error) => error_code(error),
    }
}

/// `AdsCloseSQLStatement`: frees a statement, and closes its cursors.
#[unsafe(no_mangle)]
pub extern "system" fn AdsCloseSQLStatement(statement: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCloseSQLStatement");
    let backend = api(statement);
    // Its check replaces an error left before; a value that is no handle
    // is reported as one (`fixtures/errors/handle_order.json`).
    if backend.handle_kind(statement) == Ok(None) {
        let detail = crate::last_error::NOT_A_HANDLE;
        crate::last_error::fail_detail(crate::AE_INVALID_HANDLE, detail);
        return AE_INVALID_SQLSTATEMENT_HANDLE;
    }
    match backend.close_statement(statement) {
        Ok(()) => AE_SUCCESS,
        Err(tflip_engine::Error::InvalidStatement) => crate::last_error::fail_detail(
            AE_INVALID_SQLSTATEMENT_HANDLE,
            crate::last_error::NOT_A_STATEMENT,
        ),
        Err(error) => error_code(error),
    }
}

/// Runs `sql` on `statement` with `run`, logs `word` and the outcome on the
/// `sql` channel, and ends the call with `finish`; `None` for a null pointer.
fn on_sql<T: std::fmt::Debug>(
    word: &str,
    statement: AdsHandle,
    sql: Option<String>,
    run: impl FnOnce(String) -> Result<T, tflip_engine::Error>,
    finish: impl FnOnce(Result<T, tflip_engine::Error>) -> Unsigned32,
) -> Unsigned32 {
    let Some(sql) = sql else {
        // The original leaves the last error as it was when it executes
        // (`fixtures/errors/param_n`).
        return if word == "execute" {
            crate::last_error::restore_pending();
            AE_INVALID_POINTER
        } else {
            crate::last_error::null_param(2)
        };
    };
    let result = run(sql.clone());
    crate::log::sql(format_args!(
        "{word} {statement:#x} {sql:?}: {}",
        outcome(&result)
    ));
    finish(result)
}

/// Prepares `sql` on `statement`; `None` for a null pointer.
fn prepare(statement: AdsHandle, sql: Option<String>) -> Unsigned32 {
    let run = |sql| api(statement).prepare(statement, sql);
    on_sql("prepare", statement, sql, run, code)
}

/// `AdsPrepareSQL`: prepares SQL for execution. As in the original engine,
/// the text is only checked when it runs; empty text fails with
/// `AE_INVALID_VALUE`.
///
/// # Safety
///
/// `sql` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsPrepareSQL(
    statement: AdsHandle,
    sql: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsPrepareSQL");
    // SAFETY: `sql` is null or a NUL-terminated string, as # Safety says.
    prepare(statement, unsafe { Encoding::Ansi.read(sql.cast()) })
}

/// `AdsPrepareSQLW`: [`AdsPrepareSQL`] with UTF-16 text.
///
/// # Safety
///
/// `sql` must be null or point to a NUL-terminated UTF-16 string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsPrepareSQLW(statement: AdsHandle, sql: *mut Wchar) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsPrepareSQLW");
    // SAFETY: `sql` is null or NUL-terminated UTF-16, as # Safety says.
    prepare(statement, unsafe { Encoding::Utf16.read(sql.cast()) })
}

/// `AdsCachePrepareSQL`: a new statement of the connection with `sql`
/// prepared, from the original engine's cache of prepared statements;
/// tflip prepares it anew. As in the original engine, SQL that does not
/// prepare is not an error here.
///
/// # Safety
///
/// `sql` must be null or point to a NUL-terminated string; `statement` must
/// be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCachePrepareSQL(
    connect: AdsHandle,
    sql: *mut Unsigned8,
    statement: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCachePrepareSQL");
    // SAFETY: `sql` is null or a NUL-terminated string, `statement` null or
    // writable, as # Safety says.
    let (sql, statement) = unsafe { (Encoding::Ansi.read(sql.cast()), statement.as_mut()) };
    cache_prepare(connect, sql, statement)
}

/// `AdsCachePrepareSQLW`: [`AdsCachePrepareSQL`] with UTF-16 text.
///
/// # Safety
///
/// As for [`AdsCachePrepareSQL`], with `sql` UTF-16.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCachePrepareSQLW(
    connect: AdsHandle,
    sql: *mut Wchar,
    statement: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCachePrepareSQLW");
    // SAFETY: `sql` is null or NUL-terminated UTF-16, `statement` null or
    // writable, as # Safety says.
    let (sql, statement) = unsafe { (Encoding::Utf16.read(sql.cast()), statement.as_mut()) };
    cache_prepare(connect, sql, statement)
}

/// See [`AdsCachePrepareSQL`]; `None` for a null pointer.
fn cache_prepare(
    connect: AdsHandle,
    sql: Option<String>,
    statement: Option<&mut AdsHandle>,
) -> Unsigned32 {
    if sql.is_none() {
        return crate::last_error::null_param(2);
    }
    let Some(statement) = statement else {
        return crate::last_error::null_param(3);
    };
    let rc = create_statement(connect, Some(&mut *statement));
    if rc != AE_SUCCESS {
        return rc;
    }
    let _ = prepare(*statement, sql);
    crate::last_error::clear();
    AE_SUCCESS
}

/// Verifies `sql` on `statement`; `None` for a null pointer.
fn verify(statement: AdsHandle, sql: Option<String>) -> Unsigned32 {
    let run = |sql| api(statement).verify(statement, sql);
    on_sql("verify", statement, sql, run, code)
}

/// `AdsVerifySQL`: checks SQL without running it: its syntax, and the
/// tables and columns it names.
///
/// # Safety
///
/// `sql` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsVerifySQL(
    statement: AdsHandle,
    sql: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsVerifySQL");
    // SAFETY: `sql` is null or a NUL-terminated string, as # Safety says.
    verify(statement, unsafe { Encoding::Ansi.read(sql.cast()) })
}

/// `AdsVerifySQLW`: [`AdsVerifySQL`] with UTF-16 text.
///
/// # Safety
///
/// `sql` must be null or point to a NUL-terminated UTF-16 string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsVerifySQLW(statement: AdsHandle, sql: *mut Wchar) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsVerifySQLW");
    // SAFETY: `sql` is null or NUL-terminated UTF-16, as # Safety says.
    verify(statement, unsafe { Encoding::Utf16.read(sql.cast()) })
}

/// How a call on a statement ended, for the `sql` log channel.
fn outcome<T: std::fmt::Debug>(result: &Result<T, tflip_engine::Error>) -> String {
    match result {
        Ok(value) => format!("{value:?}"),
        Err(tflip_engine::Error::Sql(error)) => format!("error {error}"),
        Err(error) => format!("error {error:?}"),
    }
}

/// Writes the cursor an execution returned to `cursor` (`None` for a null
/// pointer), 0 for none.
fn executed(
    result: Result<Option<tflip_engine::handle::Handle>, tflip_engine::Error>,
    cursor: Option<&mut AdsHandle>,
) -> Unsigned32 {
    match result {
        Ok(handle) => {
            if let Some(cursor) = cursor {
                *cursor = handle.unwrap_or(0);
            }
            AE_SUCCESS
        }
        // A `SELECT` (or `SHOW PLAN`) without a place for its cursor, which
        // the original engine explains.
        Err(tflip_engine::Error::NoCursorHandle) if cursor.is_none() => {
            crate::last_error::fail_text(
                crate::AE_INVALID_CURSORHANDLE,
                "Error 5119:  The cursor handle provided was invalid.  The SQL statement executed \
             was a SELECT statement."
                    .to_owned(),
            )
        }
        Err(error) => error_code(error),
    }
}

/// `AdsExecuteSQL`: runs the prepared SQL; `*cursor` is the cursor of a
/// `SELECT` or of a procedure with results, else 0. The cursor of the last
/// execution must be closed first (`AE_CURSOR_NOT_CLOSED`); a `SELECT`
/// without a place for its cursor fails with `AE_INVALID_CURSORHANDLE`.
///
/// # Safety
///
/// `cursor` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsExecuteSQL(
    statement: AdsHandle,
    cursor: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsExecuteSQL");
    // SAFETY: `cursor` is null or writable, as # Safety says.
    let mut cursor = unsafe { cursor.as_mut() };
    let backend = api(statement);
    let result = backend.execute(statement, cursor.is_some());
    crate::log::sql(format_args!("execute {statement:#x}: {}", outcome(&result)));
    // As in the original engine, a cursor still open is handed back with
    // the error (`AdsExecuteSQLDirect` hands back none).
    if let (Err(tflip_engine::Error::CursorNotClosed), Some(cursor)) =
        (&result, cursor.as_deref_mut())
        && let Ok(Some(open)) = backend.open_cursor(statement)
    {
        *cursor = open;
    }
    executed(result, cursor)
}

/// Prepares and runs `sql`; `None` for a null pointer.
fn execute_direct(
    statement: AdsHandle,
    sql: Option<String>,
    cursor: Option<&mut AdsHandle>,
) -> Unsigned32 {
    let wants_cursor = cursor.is_some();
    let run = |sql| api(statement).execute_direct(statement, sql, wants_cursor);
    on_sql("execute", statement, sql, run, |result| {
        executed(result, cursor)
    })
}

/// `AdsExecuteSQLDirect`: prepares and runs SQL (see [`AdsExecuteSQL`]).
///
/// # Safety
///
/// `sql` must be null or point to a NUL-terminated string; `cursor` must be
/// null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsExecuteSQLDirect(
    statement: AdsHandle,
    sql: *mut Unsigned8,
    cursor: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsExecuteSQLDirect");
    // SAFETY: `sql` is null or a NUL-terminated string, `cursor` null or
    // writable, as # Safety says.
    let (sql, cursor) = unsafe { (Encoding::Ansi.read(sql.cast()), cursor.as_mut()) };
    execute_direct(statement, sql, cursor)
}

/// `AdsExecuteSQLDirectW`: [`AdsExecuteSQLDirect`] with UTF-16 text.
///
/// # Safety
///
/// `sql` must be null or point to a NUL-terminated UTF-16 string; `cursor`
/// must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsExecuteSQLDirectW(
    statement: AdsHandle,
    sql: *mut Wchar,
    cursor: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsExecuteSQLDirectW");
    // SAFETY: `sql` is null or NUL-terminated UTF-16, `cursor` null or
    // writable, as # Safety says.
    let (sql, cursor) = unsafe { (Encoding::Utf16.read(sql.cast()), cursor.as_mut()) };
    execute_direct(statement, sql, cursor)
}

/// `AdsClearSQLParams`: clears the values of a statement's parameters.
#[unsafe(no_mangle)]
pub extern "system" fn AdsClearSQLParams(statement: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsClearSQLParams");
    if let Err(code) = crate::handle::known(statement, AE_INVALID_SQLSTATEMENT_HANDLE) {
        return code;
    }
    code(api(statement).clear_params(statement))
}

/// `AdsGetSQLStatementHandle`: the statement a cursor belongs to; any other
/// handle fails with `AE_INVALID_CURSORHANDLE`.
///
/// # Safety
///
/// `statement` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetSQLStatementHandle(
    cursor: AdsHandle,
    statement: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetSQLStatementHandle");
    match api(cursor).cursor_statement(cursor) {
        // SAFETY: `statement` is null or writable, as # Safety says.
        Ok(handle) => unsafe { crate::field::write(Ok(handle), statement) },
        Err(error) => error_code(error),
    }
}

/// `AdsSqlPeekStatement`: whether a cursor is live (1), the table its
/// `SELECT` reads, or static (0), a table of the results; Delphi's
/// `TAdsQuery` makes a dataset on a static cursor read-only. Any other
/// handle fails with `AE_INVALID_HANDLE` and leaves `*is_live` as it is; a
/// null `is_live` is accepted. Not
/// declared in the SDK's `ace.h`; the signature is `UNSIGNED32
/// AdsSqlPeekStatement( ADSHANDLE hSQL, UNSIGNED8 *pucIsLive )`
/// (`aceunpub.pas`).
///
/// # Safety
///
/// `is_live` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSqlPeekStatement(
    cursor: AdsHandle,
    is_live: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSqlPeekStatement");
    match api(cursor).cursor_is_live(cursor) {
        // A null pointer is accepted, as in the original engine.
        Ok(live) => {
            // SAFETY: `is_live` is null or writable, as # Safety says.
            if let Some(is_live) = unsafe { is_live.as_mut() } {
                *is_live = Unsigned8::from(live);
            }
            AE_SUCCESS
        }
        Err(error) => error_code(error),
    }
}

/// `AdsGetSQLStatement`: the SQL prepared on a statement, in `sql` of
/// `*len` bytes (NUL-terminated); `*len` is set to its length. Without SQL
/// prepared, both are left as they are.
///
/// # Safety
///
/// `sql` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetSQLStatement(
    statement: AdsHandle,
    sql: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetSQLStatement");
    match api(statement).statement_text(statement) {
        // Without SQL, the original engine leaves the buffer as it is.
        Ok(text) if text.is_empty() => AE_SUCCESS,
        Ok(text) => {
            // SAFETY: `sql` and `len` are as # Safety says.
            unsafe { write_string_at(&crate::ansi::data_out(&text), sql, len, 2) }
        }
        Err(error) if *error.plain() == tflip_engine::Error::InvalidStatement => {
            fail(AE_INVALID_SQLSTATEMENT_HANDLE)
        }
        Err(error) => error_code(error),
    }
}

/// `AdsGetNumParams`: the number of parameters (`?`, `:name`, each marker
/// one) of the SQL prepared on a statement, or the most any SQL prepared on
/// it had, which `AdsClearSQLParams` does not lower; 0 without SQL.
///
/// # Safety
///
/// `count` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetNumParams(
    statement: AdsHandle,
    count: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetNumParams");
    let params = match api(statement).statement_params(statement) {
        Ok(params) => params,
        Err(error) if *error.plain() == tflip_engine::Error::InvalidStatement => {
            return fail(AE_INVALID_SQLSTATEMENT_HANDLE);
        }
        Err(error) => return error_code(error),
    };
    // SAFETY: `count` is null or writable, as # Safety says.
    unsafe { crate::field::write_at(Ok(params as Unsigned16), count, 2) }
}

/// `AdsGetSQLStmtParams`: rewrites the SQL text `statement` in place so
/// that only its parameter markers (`?`, `:name`) are left, every other
/// character a space, as the original engine does; Delphi's `TAdsQuery`
/// then finds the parameters in it. Not declared in the SDK's `ace.h`; the
/// signature is `UNSIGNED32 AdsGetSQLStmtParams( UNSIGNED8 *pucStatement )`
/// (`aceunpub.pas`).
///
/// # Safety
///
/// `statement` must be null or point to a NUL-terminated, writable string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetSQLStmtParams(statement: *mut Unsigned8) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetSQLStmtParams");
    if statement.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: not null, so a writable NUL-terminated string, as # Safety
    // says, of which the slice takes the bytes before the NUL.
    let text = unsafe {
        let len = CStr::from_ptr(statement.cast()).to_bytes().len();
        std::slice::from_raw_parts_mut(statement, len)
    };
    tflip_engine::statement::blank_all_but_params(text);
    AE_SUCCESS
}

/// Sets `setting` on `statement`; `invalid` (the code for a value it does
/// not take) instead if `setting` is `None`.
pub(crate) fn set_statement(
    statement: AdsHandle,
    setting: Option<Setting>,
    invalid: Unsigned32,
) -> Unsigned32 {
    let Some(setting) = setting else {
        return fail(invalid);
    };
    match api(statement).set_statement(statement, setting) {
        Ok(()) => AE_SUCCESS,
        Err(error) if *error.plain() == tflip_engine::Error::InvalidStatement => {
            fail_handle(AE_INVALID_SQLSTATEMENT_HANDLE)
        }
        Err(error) => error_code(error),
    }
}

/// `AdsStmtSetTableType`: the type (`ADS_NTX`, `ADS_CDX`, `ADS_ADT` or
/// `ADS_VFP`) of the free tables a statement opens and creates; a name
/// without an extension is a file of that type. Others fail with
/// `AE_INVALID_TABLETYPE`, `ADS_DATABASE_TABLE` too (probed on a free-table
/// connection), and with the last execution's cursor open,
/// `AE_CURSOR_NOT_CLOSED`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsStmtSetTableType(
    statement: AdsHandle,
    table_type: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsStmtSetTableType");
    let setting = match table_type {
        crate::ADS_DATABASE_TABLE => None,
        _ => crate::table::engine_table_type(table_type).map(Setting::TableType),
    };
    if setting.is_none() {
        return crate::last_error::fail_detail(crate::AE_INVALID_TABLETYPE, "Parameter 2");
    }
    set_statement(statement, setting, crate::AE_INVALID_TABLETYPE)
}

/// `AdsStmtSetTableCharType`: the character type (`ADS_ANSI`, `ADS_OEM`,
/// or a collation's number) of the free tables a statement opens; as in the
/// original engine, 1 to 118, else `AE_INVALID_CHARSETTYPE`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsStmtSetTableCharType(
    statement: AdsHandle,
    char_type: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsStmtSetTableCharType");
    let setting = (1..=118)
        .contains(&char_type)
        .then_some(Setting::CharType(char_type));
    set_statement(statement, setting, crate::AE_INVALID_CHARSETTYPE)
}

/// `AdsStmtSetTableLockType`: the locking (`ADS_COMPATIBLE_LOCKING` or
/// `ADS_PROPRIETARY_LOCKING`) of the tables a statement opens; kept, as
/// tflip locks its own way (see `KNOWN_ISSUES.md`).
#[unsafe(no_mangle)]
pub extern "system" fn AdsStmtSetTableLockType(
    statement: AdsHandle,
    lock_type: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsStmtSetTableLockType");
    let setting = (lock_type <= 1).then_some(Setting::LockType(lock_type));
    set_statement(statement, setting, crate::AE_INVALID_LOCKTYPE)
}

/// `AdsStmtSetTableCollation`: the collation of the tables a statement
/// opens; empty or null for the connection's. A name the engine does not
/// know is refused as [`crate::settings::check_collation`] says (7147, 1564).
///
/// # Safety
///
/// `collation` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsStmtSetTableCollation(
    statement: AdsHandle,
    collation: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsStmtSetTableCollation");
    // SAFETY: `collation` is null or a NUL-terminated string, as # Safety
    // says.
    let collation = unsafe { crate::ffi::text(collation) }.unwrap_or_default();
    let collation = match crate::settings::check_collation(collation, true) {
        Ok(name) => name,
        Err(code) => return code,
    };
    set_statement(statement, Some(Setting::Collation(collation)), AE_SUCCESS)
}

/// `AdsStmtSetTableRights`: whether the tables a statement opens are
/// checked for access rights (`ADS_CHECKRIGHTS` or `ADS_IGNORERIGHTS`),
/// else `AE_INVALID_RIGHTSOPTION`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsStmtSetTableRights(
    statement: AdsHandle,
    check_rights: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsStmtSetTableRights");
    let setting = matches!(check_rights, 1 | 2).then_some(Setting::Rights(check_rights));
    set_statement(statement, setting, crate::AE_INVALID_RIGHTSOPTION)
}

/// `AdsStmtSetTableReadOnly`: whether a statement's live cursors are
/// read-only (`ADS_CURSOR_READONLY`) or not (`ADS_CURSOR_READWRITE`), else
/// `AE_INVALID_READONLY_OPTION`; their open options say so. SQL's own
/// writes (`UPDATE`, ...) are not affected, as in the original engine.
#[unsafe(no_mangle)]
pub extern "system" fn AdsStmtSetTableReadOnly(
    statement: AdsHandle,
    read_only: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsStmtSetTableReadOnly");
    let setting = match read_only {
        crate::ADS_CURSOR_READONLY => Some(Setting::ReadOnly(true)),
        crate::ADS_CURSOR_READWRITE => Some(Setting::ReadOnly(false)),
        _ => None,
    };
    set_statement(statement, setting, crate::AE_INVALID_READONLY_OPTION)
}

/// [`set_statement`] for the table password and encryption functions: a
/// handle that is no statement is 5106, with the last error 5018
/// (`fixtures/write/encryption/stmtpw.json`).
fn set_encryption(statement: AdsHandle, setting: Setting) -> Unsigned32 {
    match api(statement).set_statement(statement, setting) {
        Ok(()) => AE_SUCCESS,
        Err(error) if *error.plain() == tflip_engine::Error::InvalidStatement => {
            crate::last_error::fail_text(
                crate::AE_INVALID_HANDLE,
                "Error 5018:  The handle given was not recognized by Advantage. Verify \
                 specified handle is open/active.  The given handle is not readable."
                    .to_owned(),
            );
            AE_INVALID_SQLSTATEMENT_HANDLE
        }
        Err(error) => error_code(error),
    }
}

/// `AdsStmtSetTablePassword`: the password of the encrypted free table
/// `table` that the statement opens, whichever way its SQL names it, but by
/// its name alone in any case (not with its extension or a path); a later
/// password for the table replaces the earlier. A table without a password
/// is refused with 5098 and one with another with 7060 when the SQL opens
/// it; a table that is not encrypted opens whatever the password
/// (`fixtures/write/encryption/stmtpw`). A null table is `AE_INVALID_POINTER`
/// (`Parameter 2`), a null password an empty one.
///
/// # Safety
///
/// `table` and `password` must be null or point to NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsStmtSetTablePassword(
    statement: AdsHandle,
    table: *mut Unsigned8,
    password: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsStmtSetTablePassword");
    // SAFETY: both are null or NUL-terminated strings, as # Safety says.
    let (table, password) = unsafe { (crate::ffi::text(table), crate::ffi::text(password)) };
    let Some(table) = table else {
        return crate::last_error::fail_detail(AE_INVALID_POINTER, "Parameter 2");
    };
    let setting = Setting::TablePassword(table.to_owned(), password.unwrap_or_default().to_owned());
    set_encryption(statement, setting)
}

/// `AdsStmtClearTablePasswords`: forgets the passwords
/// [`AdsStmtSetTablePassword`] gave.
#[unsafe(no_mangle)]
pub extern "system" fn AdsStmtClearTablePasswords(statement: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsStmtClearTablePasswords");
    set_encryption(statement, Setting::ClearTablePasswords)
}

/// `AdsStmtEnableEncryption`: kept, and used by nothing: the tables a
/// statement creates and writes are not encrypted by it, nor does it open an
/// encrypted table (`fixtures/write/encryption/stmtpw`). A null password is
/// `AE_INVALID_POINTER` (`Parameter 2`).
///
/// # Safety
///
/// `password` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsStmtEnableEncryption(
    statement: AdsHandle,
    password: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsStmtEnableEncryption");
    // SAFETY: `password` is null or a NUL-terminated string, as # Safety says.
    let Some(password) = (unsafe { crate::ffi::text(password) }) else {
        return crate::last_error::fail_detail(AE_INVALID_POINTER, "Parameter 2");
    };
    set_encryption(statement, Setting::Encryption(Some(password.to_owned())))
}

/// `AdsStmtDisableEncryption`: undoes [`AdsStmtEnableEncryption`].
#[unsafe(no_mangle)]
pub extern "system" fn AdsStmtDisableEncryption(statement: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsStmtDisableEncryption");
    set_encryption(statement, Setting::Encryption(None))
}

/// `AdsStmtConstrainUpdates`: whether what a write through a statement's
/// live cursor changes or appends must meet its `WHERE`
/// (`ADS_CONSTRAIN` or `ADS_NO_CONSTRAIN`), else
/// `AE_INVALID_CONSTRAIN_TYPE`. A write that leaves it fails with 7065
/// (`fixtures/write/cursor_constrain`), and the cursor of the previous
/// execution must be closed (`AE_CURSOR_NOT_CLOSED`).
#[unsafe(no_mangle)]
pub extern "system" fn AdsStmtConstrainUpdates(
    statement: AdsHandle,
    constrain: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsStmtConstrainUpdates");
    let setting = match constrain {
        crate::ADS_CONSTRAIN => Some(Setting::Constrain(true)),
        crate::ADS_NO_CONSTRAIN => Some(Setting::Constrain(false)),
        _ => None,
    };
    if setting.is_none() {
        return crate::last_error::fail_detail(crate::AE_INVALID_CONSTRAIN_TYPE, "Parameter 2");
    }
    set_statement(statement, setting, crate::AE_INVALID_CONSTRAIN_TYPE)
}

/// `AdsStmtReadAllColumns`: whether a statement reads all columns of its
/// tables or only the selected ones (`ADS_READ_ALL_COLUMNS` or
/// `ADS_READ_SELECT_COLUMNS`), else `AE_INVALID_OPTION`; the cursor of the
/// previous execution must be closed (`AE_CURSOR_NOT_CLOSED`).
///
/// The setting makes no difference to a local server, as the help says and
/// the original engine does (`fixtures/write/cursor_constrain`).
#[unsafe(no_mangle)]
pub extern "system" fn AdsStmtReadAllColumns(
    statement: AdsHandle,
    read_columns: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsStmtReadAllColumns");
    let setting = match read_columns {
        crate::ADS_READ_ALL_COLUMNS => Some(Setting::ReadAllColumns(true)),
        crate::ADS_READ_SELECT_COLUMNS => Some(Setting::ReadAllColumns(false)),
        _ => None,
    };
    if setting.is_none() {
        return crate::last_error::fail_detail(crate::AE_INVALID_OPTION, "Parameter 2");
    }
    set_statement(statement, setting, crate::AE_INVALID_OPTION)
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use serde_json::{Map, Value as Json, json};

    use super::*;
    use crate::fixtures::{self, remote_twins};
    use crate::{
        AdsAtEOF, AdsCloseTable, AdsDisconnect, AdsGetAOF, AdsGetField, AdsGetFieldDecimals,
        AdsGetFieldLength, AdsGetFieldName, AdsGetFieldType, AdsGetFilter, AdsGetHandleType,
        AdsGetLastError, AdsGetNumFields, AdsGetNumIndexes, AdsGetRecordCount, AdsGetRecordNum,
        AdsGetTableFilename, AdsGetTableOpenOptions, AdsGetTableType, AdsGotoTop, AdsIsNull,
        AdsSetDate, AdsSetDouble, AdsSetEmpty, AdsSetLogical, AdsSetLong, AdsSetMoney, AdsSetNull,
        AdsSetShort, AdsSetString, AdsSkip,
    };

    /// Bytes as the probe records them.
    fn raw(bytes: &[u8]) -> String {
        bytes.iter().map(|&b| char::from(b)).collect()
    }

    /// What `AdsGetLastError` says, for a failed call.
    fn last_error() -> String {
        let (mut code, mut buf, mut len) = (0, [0u8; 1200], 1200u16);
        unsafe { AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        raw(&buf[..usize::from(len).min(buf.len())])
    }

    fn result(object: &mut Map<String, Json>, rc: Unsigned32) {
        object.insert("rc".into(), json!(rc));
        if rc != AE_SUCCESS {
            object.insert("error".into(), json!(last_error()));
        }
    }

    /// What an order's text getter (`AdsGetIndexName`, ...) answers for
    /// `handle`: its code, and the text (empty on failure).
    fn index_text(
        get: unsafe extern "system" fn(AdsHandle, *mut Unsigned8, *mut Unsigned16) -> Unsigned32,
        handle: AdsHandle,
    ) -> (Unsigned32, String) {
        let (mut buf, mut len) = ([0u8; 1024], 1024u16);
        let rc = unsafe { get(handle, buf.as_mut_ptr(), &mut len) };
        (
            rc,
            if rc == 0 {
                raw(&buf[..len.into()])
            } else {
                String::new()
            },
        )
    }

    /// The code and name of `cursor`'s current order, as the probe asks
    /// for it: `AdsGetIndexHandle` with no name, then `AdsGetIndexName` of
    /// what it gave (a cursor without an order gives handle 0, whose name
    /// is an error).
    fn current_order(cursor: AdsHandle) -> (Unsigned32, String) {
        let mut handle = 0;
        let rc = unsafe { crate::AdsGetIndexHandle(cursor, std::ptr::null_mut(), &mut handle) };
        match rc {
            0 => index_text(crate::AdsGetIndexName, handle),
            rc => (rc, String::new()),
        }
    }

    /// A field argument `ADSFIELD(n)`.
    fn field(n: u16) -> *mut Unsigned8 {
        usize::from(n) as *mut Unsigned8
    }

    /// The cursor `c` as the probe records it.
    fn cursor(c: AdsHandle) -> Json {
        unsafe {
            let (mut buf, mut len) = (vec![0u8; 70_000], 1024u16);
            let rc = AdsGetTableFilename(c, 2, buf.as_mut_ptr(), &mut len);
            let file = match rc {
                0 => raw(&buf[..usize::from(len)]),
                rc => format!("E{rc}"),
            };
            let (mut options, mut table_type, mut indexes, mut kind, mut fields) = (0, 0, 0, 0, 0);
            AdsGetTableOpenOptions(c, &mut options);
            AdsGetTableType(c, &mut table_type);
            AdsGetNumIndexes(c, &mut indexes);
            AdsGetHandleType(c, &mut kind);
            let mut count = 0;
            if AdsGetRecordCount(c, 2, &mut count) != 0 {
                count = 0;
            }
            AdsGetNumFields(c, &mut fields);
            let mut columns = Vec::new();
            for f in 1..=fields {
                let (mut name, mut nlen) = ([0u8; 260], 260u16);
                AdsGetFieldName(c, f, name.as_mut_ptr(), &mut nlen);
                let (mut ftype, mut flen, mut dec) = (0, 0, 0);
                AdsGetFieldType(c, field(f), &mut ftype);
                AdsGetFieldLength(c, field(f), &mut flen);
                AdsGetFieldDecimals(c, field(f), &mut dec);
                columns.push(json!([
                    raw(&name[..usize::from(nlen).min(name.len())]),
                    ftype,
                    flen,
                    dec
                ]));
            }
            let mut rows = Vec::new();
            AdsGotoTop(c);
            for _ in 0..DUMP_CAP.get() {
                let mut eof = 0;
                AdsAtEOF(c, &mut eof);
                if eof != 0 {
                    break;
                }
                let mut recno = 0;
                AdsGetRecordNum(c, 2, &mut recno);
                let mut values = Vec::new();
                for f in 1..=fields {
                    let mut null = 0;
                    AdsIsNull(c, field(f), &mut null);
                    if null != 0 {
                        values.push(Json::Null);
                        continue;
                    }
                    let mut flen = buf.len() as u32;
                    let rc = AdsGetField(c, field(f), buf.as_mut_ptr(), &mut flen, 0);
                    let mut ftype = 0;
                    AdsGetFieldType(c, field(f), &mut ftype);
                    // Raw, VarBinary, Binary and Image are bytes.
                    let binary = matches!(ftype, 6 | 7 | 16 | 24);
                    values.push(match rc {
                        0 if binary => json!(raw(&buf[..flen as usize])),
                        0 => json!(raw(&buf[..flen as usize])),
                        rc => json!(format!("E{rc}")),
                    });
                }
                rows.push(json!([recno, values]));
                AdsSkip(c, 1);
            }
            json!({
                "file": file, "options": options, "table_type": table_type, "handle_type": kind,
                "indexes": indexes, "count": count, "fields": columns, "rows": rows,
            })
        }
    }

    /// Makes a result comparable: the data folder's path is `<data>`, as
    /// the probe writes it, without the padding after it; static cursors
    /// are `tim_#.tmp` (the original engine's numbering cannot be told);
    /// errors other than SQL errors are only their codes (tflip words them
    /// its own way); and whether a failed execution wrote a cursor handle
    /// does not count.
    /// Today's date as the fixtures' date format writes it, which they have
    /// as `<today>` (`CURDATE()`; see `tools/ace/sql/run.sh`).
    fn today() -> String {
        let today = tflip_storage::value::julian_to_ymd(tflip_engine::clock::now().local.0);
        format!("{:02}/{:02}/{:04}", today.month, today.day, today.year)
    }

    /// Whether `s` is a GUID in its text form, `8-4-4-4-12` hex digits, as
    /// `NEWID` and `NEWIDSTRING` produce (their value is random).
    fn is_guid(s: &str) -> bool {
        let groups = [8, 4, 4, 4, 12];
        let parts: Vec<&str> = s.split('-').collect();
        parts.len() == 5
            && parts
                .iter()
                .zip(groups)
                .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
    }

    /// The file name of this test's process, which is what `APPLICATIONID()`
    /// gives.
    fn test_exe() -> Option<String> {
        std::env::current_exe()
            .ok()?
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }

    /// Whether `s` is a `ROWID`: 18 base 64 digits.
    fn is_rowid(s: &str) -> bool {
        s.len() == 18
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
            && s.bytes().any(|b| b.is_ascii_uppercase())
            && s[12..].starts_with("AAA")
    }

    fn normalize(value: &mut Json, data: &str) {
        match value {
            Json::String(s) if *s == today() => *s = "<today>".to_owned(),
            // A path mentioning the data folder, whether on its own or inside an
            // error message: the folder is `<data>`, any padding after a path
            // value goes, and the separators are unified (the original engine
            // writes `\`, tflip the host's `/`) so only the path compares.
            Json::String(s) if s.contains(data) || s.contains("<data>") => {
                *s = s.replace(data, "<data>");
                if s.starts_with("<data>") {
                    *s = s.trim_end_matches(' ').to_owned();
                }
                *s = s.replace('\\', "/");
            }
            // The application ID is the client's file name, and a `ROWID`
            // names the database and the table by their paths: only the
            // record number of it is compared (`rowid_paths.json` has the
            // rest).
            Json::String(s) if s == "sql.exe" || Some(s.as_str()) == test_exe().as_deref() => {
                *s = "<app>".to_owned();
            }
            Json::String(s) if is_rowid(s) => *s = format!("<rowid>{}", &s[12..]),
            // `NEWID`/`NEWIDSTRING` are random: only their shape is compared.
            Json::String(s) if is_guid(s) => *s = "<guid>".to_owned(),
            Json::String(s) if s.starts_with("tim_") && s.ends_with(".tmp") => {
                *s = "tim_#.tmp".to_owned();
            }
            Json::String(s) if s.starts_with("timace_") && s.ends_with(".tmp") => {
                *s = "timace_#.tmp".to_owned();
            }
            // A temporary index's name and file.
            Json::String(s)
                if s.len() > 4
                    && s.starts_with("TIM_")
                    && s[4..].bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                *s = "TIM_#".to_owned();
            }
            Json::String(s)
                if s.len() > 4
                    && s.starts_with("tim_")
                    && s[4..].bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                *s = "tim_#".to_owned();
            }
            Json::Array(items) => items.iter_mut().for_each(|v| normalize(v, data)),
            Json::Object(map) => {
                // What a failed execution leaves in its cursor handle is not
                // compared, but for a cursor still open (5109).
                if map
                    .get("rc")
                    .and_then(Json::as_u64)
                    .is_some_and(|rc| rc != 0 && rc != 5109)
                {
                    map.remove("has_cursor");
                }
                if let Some(Json::String(error)) = map.get_mut("error")
                    && !error.starts_with("Error 7200")
                {
                    let digits: String = error
                        .trim_start_matches("Error ")
                        .chars()
                        .take_while(char::is_ascii_digit)
                        .collect();
                    *error = format!("E{digits}");
                }
                map.values_mut().for_each(|v| normalize(v, data));
            }
            _ => {}
        }
    }

    thread_local! {
        /// The connections the replay opened and has not closed, which it
        /// closes when it is over: a remote one left open would stay for the
        /// rest of the process, a session the exports asking all servers
        /// (`AdsShowDeleted`, ...) ask too.
        static OPENED: std::cell::RefCell<Vec<AdsHandle>> = const { std::cell::RefCell::new(Vec::new()) };
        /// The connection, statement and cursor `switch` has set aside.
        static OTHER: std::cell::RefCell<[AdsHandle; 3]> = const { std::cell::RefCell::new([0; 3]) };
        /// The folder `connect` takes its dictionary paths in, if not
        /// [`fixtures::sql_data`]: the replay's own.
        static CONNECT_DATA: std::cell::RefCell<Option<std::path::PathBuf>> =
            const { std::cell::RefCell::new(None) };
        /// The data folder as tflip writes it in a link's path
        /// (`AdsGetActiveLinkInfo`), whose length the probe counts as the
        /// 6 bytes of `<data>`.
        static LINK_DATA: std::cell::RefCell<Option<String>> =
            const { std::cell::RefCell::new(None) };
        /// The folder a replay that makes files made them in, to compare
        /// after it.
        /// Whether the script's text is the bytes of the client's code page
        /// as Latin-1 characters (`oem.json`, as the probe records them),
        /// not Windows-1252 text.
        static BYTE_TEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        static CREATED_DATA: std::cell::RefCell<Option<std::path::PathBuf>> =
            const { std::cell::RefCell::new(None) };
        /// How many rows of a cursor are recorded; the script's `cap`
        /// command changes it.
        static DUMP_CAP: std::cell::Cell<u32> = const { std::cell::Cell::new(200) };
    }

    /// The bytes of the script's text `text`.
    fn script_bytes(text: &str) -> Vec<u8> {
        match BYTE_TEXT.get() {
            true => text.chars().map(|c| c as u8).collect(),
            false => crate::ansi::from_str_lossy(text),
        }
    }

    /// Runs one command of the probe's script; its result as the probe
    /// records it.
    fn run(cmd: &str, connect: &mut AdsHandle, stmt: &mut AdsHandle, cur: &mut AdsHandle) -> Json {
        let mut object = Map::new();
        object.insert("cmd".into(), json!(cmd));
        let (verb, rest) = cmd.split_once(' ').unwrap_or((cmd, ""));
        let rest = rest
            .replace("\\r", "\r")
            .replace("\\n", "\n")
            .replace("\\t", "\t");
        let (verb, rest) = match verb {
            "expr" => ("query", format!("SELECT {rest} FROM system.iota")),
            _ => (verb, rest),
        };
        // The data folder in a statement (`<data>\table`) is the scratch folder.
        let rest = match CREATED_DATA.with_borrow(Clone::clone) {
            Some(dir) if rest.contains("<data>") => {
                let dir = dir.display().to_string();
                rest.replace("<data>\\", &format!("{dir}/"))
                    .replace("<data>", &dir)
            }
            _ => rest,
        };
        let text = CString::new(script_bytes(&rest)).unwrap();
        let text = || text.as_ptr().cast_mut().cast::<Unsigned8>();
        unsafe {
            match verb {
                "stmt" => result(&mut object, AdsCreateSQLStatement(*connect, stmt)),
                "direct" | "query" | "exec" => {
                    let mut fresh = 0;
                    let rc = match verb {
                        "exec" => AdsExecuteSQL(*stmt, &mut fresh),
                        _ => AdsExecuteSQLDirect(*stmt, text(), &mut fresh),
                    };
                    result(&mut object, rc);
                    object.insert("has_cursor".into(), json!(fresh != 0));
                    let mut affected = 0;
                    if AdsGetRecordCount(*stmt, 2, &mut affected) != 0 {
                        affected = 0;
                    }
                    object.insert("affected".into(), json!(affected));
                    if rc == 0 && fresh != 0 {
                        *cur = fresh;
                        object.insert("cursor".into(), cursor(fresh));
                        if verb == "query" {
                            AdsCloseTable(fresh);
                        }
                    }
                }
                "directnull" => {
                    let rc = AdsExecuteSQLDirect(*stmt, text(), std::ptr::null_mut());
                    result(&mut object, rc);
                }
                "prepare" => result(&mut object, AdsPrepareSQL(*stmt, text())),
                "setprop" => {
                    let mut words = rest.split(' ');
                    let target = match words.next().unwrap() {
                        "stmt" => *stmt,
                        "conn" => *connect,
                        number => number.parse().unwrap(),
                    };
                    let (id, value) = (words.next().unwrap(), words.next().unwrap());
                    let number = |text: &str| match text.strip_prefix("0x") {
                        Some(hex) => usize::from_str_radix(hex, 16).unwrap(),
                        None => text.parse().unwrap(),
                    };
                    let rc = crate::AdsSetProperty(target, number(id) as u32, number(value));
                    result(&mut object, rc);
                }
                "verify" => result(&mut object, AdsVerifySQL(*stmt, text())),
                "param" => {
                    let mut words = rest.splitn(3, ' ');
                    let (target, kind) = (words.next().unwrap(), words.next().unwrap());
                    let value = words.next().unwrap_or("");
                    let name = CString::new(target).unwrap();
                    let param = match target.parse::<usize>() {
                        Ok(n) => n as *mut Unsigned8,
                        Err(_) => name.as_ptr().cast_mut().cast(),
                    };
                    let v = CString::new(match BYTE_TEXT.get() {
                        true => script_bytes(value),
                        false => value.as_bytes().to_vec(),
                    })
                    .unwrap();
                    let v_ptr = v.as_ptr().cast_mut().cast();
                    let rc = match kind {
                        "string" => AdsSetString(*stmt, param, v_ptr, v.as_bytes().len() as u32),
                        "long" => AdsSetLong(*stmt, param, value.parse().unwrap()),
                        "short" => AdsSetShort(*stmt, param, value.parse().unwrap()),
                        "double" => AdsSetDouble(*stmt, param, value.parse().unwrap()),
                        "logical" => AdsSetLogical(*stmt, param, value.parse().unwrap()),
                        "date" => AdsSetDate(*stmt, param, v_ptr, value.len() as u16),
                        "money" => AdsSetMoney(*stmt, param, value.parse().unwrap()),
                        "null" => AdsSetNull(*stmt, param),
                        "empty" => AdsSetEmpty(*stmt, param),
                        other => panic!("param kind {other}"),
                    };
                    result(&mut object, rc);
                }
                "clearparams" => result(&mut object, AdsClearSQLParams(*stmt)),
                "numparams" => {
                    let mut n = 77;
                    result(&mut object, AdsGetNumParams(*stmt, &mut n));
                    object.insert("n".into(), json!(n));
                }
                "tabletype" | "chartype" | "locktype" | "readonly" | "rights" => {
                    let set = match verb {
                        "tabletype" => AdsStmtSetTableType,
                        "chartype" => AdsStmtSetTableCharType,
                        "locktype" => AdsStmtSetTableLockType,
                        "readonly" => AdsStmtSetTableReadOnly,
                        _ => AdsStmtSetTableRights,
                    };
                    result(&mut object, set(*stmt, rest.trim().parse().unwrap()));
                }
                "collation" => result(&mut object, AdsStmtSetTableCollation(*stmt, text())),
                "setcollation" => result(&mut object, crate::AdsSetCollation(*connect, text())),
                "count" => {
                    let mut count = 0;
                    result(&mut object, AdsGetRecordCount(*stmt, 2, &mut count));
                    object.insert("count".into(), json!(count));
                }
                "closestmt" => result(&mut object, AdsCloseSQLStatement(*stmt)),
                "closecursor" => result(&mut object, AdsCloseTable(*cur)),
                "info" => {
                    let (mut stype, mut ctype) = (0u16, 0u16);
                    let src = AdsGetHandleType(*stmt, &mut stype);
                    let crc = AdsGetHandleType(*cur, &mut ctype);
                    let kind = |rc: u32, kind: u16| {
                        if rc != 0 {
                            rc + 100_000
                        } else {
                            u32::from(kind)
                        }
                    };
                    object.insert("stmt_type".into(), json!(kind(src, stype)));
                    object.insert("cursor_type".into(), json!(kind(crc, ctype)));
                    let mut owner = 0;
                    let rc = AdsGetSQLStatementHandle(*cur, &mut owner);
                    object.insert("owner_rc".into(), json!(rc));
                    object.insert("owner_is_stmt".into(), json!(owner == *stmt));
                    let (mut buf, mut len) = (vec![0u8; 4096], 4096u16);
                    let rc = AdsGetSQLStatement(*stmt, buf.as_mut_ptr(), &mut len);
                    object.insert("sql_rc".into(), json!(rc));
                    let sql = if rc != 0 {
                        String::new()
                    } else {
                        raw(&buf[..usize::from(len)])
                    };
                    object.insert("sql".into(), json!(sql));
                    for (key, get) in [
                        (
                            "filter",
                            AdsGetFilter as unsafe extern "system" fn(_, _, _) -> _,
                        ),
                        ("aof", AdsGetAOF),
                    ] {
                        let (mut buf, mut len) = (vec![0u8; 4096], 4096u16);
                        let rc = get(*cur, buf.as_mut_ptr(), &mut len);
                        object.insert(format!("{key}_rc"), json!(rc));
                        let text = if rc != 0 {
                            String::new()
                        } else {
                            raw(&buf[..usize::from(len)])
                        };
                        object.insert(key.into(), json!(text));
                    }
                }
                "peek" => {
                    let target = match rest.as_str() {
                        "stmt" => *stmt,
                        "0" => 0,
                        "table" => {
                            let mut table = 0;
                            let null = std::ptr::null_mut();
                            let name = c"indexed.adt".as_ptr().cast_mut().cast();
                            crate::AdsOpenTable90(
                                *connect, name, null, 3, 1, 1, 1, 4, null, &mut table,
                            );
                            table
                        }
                        _ => *cur,
                    };
                    let mut live = 0xAA;
                    result(&mut object, AdsSqlPeekStatement(target, &mut live));
                    object.insert("live".into(), json!(live));
                    if rest == "table" {
                        AdsCloseTable(target);
                    }
                }
                // `indexes`: the last cursor's `AdsGetNumIndexes` and each
                // order's name, expression, condition and file.
                "indexes" => {
                    let mut n = 0xAAAA;
                    result(&mut object, AdsGetNumIndexes(*cur, &mut n));
                    object.insert("num".into(), json!(n));
                    let mut orders = Vec::new();
                    for i in 1..=n {
                        let mut handle = 0;
                        let rc = crate::AdsGetIndexHandleByOrder(*cur, i, &mut handle);
                        let mut order = Map::new();
                        order.insert("order".into(), json!(i));
                        order.insert("rc".into(), json!(rc));
                        if rc == 0 {
                            let name = index_text(crate::AdsGetIndexName, handle);
                            order.insert("name_rc".into(), json!(name.0));
                            order.insert("name".into(), json!(name.1));
                            let expr = index_text(crate::AdsGetIndexExpr, handle);
                            order.insert("expr_rc".into(), json!(expr.0));
                            order.insert("expr".into(), json!(expr.1));
                            let cond = index_text(crate::AdsGetIndexCondition, handle);
                            order.insert("cond_rc".into(), json!(cond.0));
                            order.insert("cond".into(), json!(cond.1));
                            let (mut buf, mut len) = ([0u8; 1024], 1024u16);
                            let frc =
                                crate::AdsGetIndexFilename(handle, 2, buf.as_mut_ptr(), &mut len);
                            order.insert("file_rc".into(), json!(frc));
                            order.insert(
                                "file".into(),
                                json!(if frc == 0 {
                                    raw(&buf[..len.into()])
                                } else {
                                    String::new()
                                }),
                            );
                            let mut back = 0xAAAA;
                            let brc = crate::AdsGetIndexOrderByHandle(handle, &mut back);
                            order.insert("back_rc".into(), json!(brc));
                            order.insert("back".into(), json!(back));
                            let mut by_name = 0;
                            let cname = CString::new(name.1.clone()).unwrap();
                            let nrc = crate::AdsGetIndexHandle(
                                *cur,
                                cname.as_ptr().cast_mut().cast(),
                                &mut by_name,
                            );
                            order.insert("byname_rc".into(), json!(nrc));
                            order.insert("byname_same".into(), json!(by_name == handle));
                            let mut kind = 0xAAAA;
                            AdsGetHandleType(handle, &mut kind);
                            order.insert("handle_type".into(), json!(kind));
                        }
                        orders.push(Json::Object(order));
                    }
                    object.insert("orders".into(), Json::Array(orders));
                    let mut current = 0;
                    let rc = crate::AdsGetIndexHandle(*cur, std::ptr::null_mut(), &mut current);
                    object.insert("current_rc".into(), json!(rc));
                    if rc == 0 {
                        let name = index_text(crate::AdsGetIndexName, current);
                        object.insert("current_name".into(), json!(name.1));
                    }
                }
                // `indexflags <order>`: the answers about one order's kind
                // and keys.
                "indexflags" => {
                    let mut handle = 0;
                    let rc = crate::AdsGetIndexHandleByOrder(
                        *cur,
                        rest.trim().parse().unwrap(),
                        &mut handle,
                    );
                    result(&mut object, rc);
                    if rc == 0 {
                        type Is = unsafe extern "system" fn(AdsHandle, *mut u16) -> u32;
                        let flags: [(&str, Is); 8] = [
                            ("Descending", crate::AdsIsIndexDescending),
                            ("Unique", crate::AdsIsIndexUnique),
                            ("Candidate", crate::AdsIsIndexCandidate),
                            ("Compound", crate::AdsIsIndexCompound),
                            ("Custom", crate::AdsIsIndexCustom),
                            ("PrimaryKey", crate::AdsIsIndexPrimaryKey),
                            ("FTS", crate::AdsIsIndexFTS),
                            ("Nullable", crate::AdsIsIndexNullable),
                        ];
                        for (name, is) in flags {
                            let mut flag = 0xAAAA;
                            let rc = is(handle, &mut flag);
                            object.insert(name.into(), json!([rc, flag]));
                        }
                        for option in 1..=2u16 {
                            let mut count = 0xAAAA;
                            let rc = crate::AdsGetKeyCount(handle, option, &mut count);
                            object.insert(format!("key_count_{option}"), json!([rc, count]));
                        }
                        let mut flags = 0xAAAA;
                        let rc = crate::AdsGetIndexFlags(handle, &mut flags);
                        object.insert("flags".into(), json!([rc, flags]));
                        let (mut length, mut kind) = (0xAAAA, 0xAAAA);
                        let rc = crate::AdsGetKeyLength(handle, &mut length);
                        object.insert("key_length".into(), json!([rc, length]));
                        let rc = crate::AdsGetKeyType(handle, &mut kind);
                        object.insert("key_type".into(), json!([rc, kind]));
                        let mut number = 0xAAAA;
                        let rc = crate::AdsGetKeyNum(handle, 1, &mut number);
                        object.insert("key_num".into(), json!([rc, number]));
                        let (mut buf, mut len) = ([0u8; 300], 300u16);
                        let rc = crate::AdsGetIndexCollation(handle, buf.as_mut_ptr(), &mut len);
                        object.insert(
                            "collation".into(),
                            json!([
                                rc,
                                if rc == 0 {
                                    raw(&buf[..len.into()])
                                } else {
                                    String::new()
                                }
                            ]),
                        );
                    }
                }
                // `files`: the names of the last cursor and of its last
                // order, with options 1 and 2.
                "files" => {
                    let mut n = 0;
                    AdsGetNumIndexes(*cur, &mut n);
                    let mut handle = 0;
                    if n > 0 {
                        crate::AdsGetIndexHandleByOrder(*cur, n, &mut handle);
                    }
                    for option in 1..=2u16 {
                        let (mut buf, mut len) = ([0u8; 600], 600u16);
                        let rc = AdsGetTableFilename(*cur, option, buf.as_mut_ptr(), &mut len);
                        object.insert(
                            format!("table_{option}"),
                            json!([
                                rc,
                                if rc == 0 {
                                    raw(&buf[..len.into()])
                                } else {
                                    String::new()
                                }
                            ]),
                        );
                        let (mut buf, mut len) = ([0u8; 600], 600u16);
                        let rc = if handle == 0 {
                            99999
                        } else {
                            crate::AdsGetIndexFilename(handle, option, buf.as_mut_ptr(), &mut len)
                        };
                        object.insert(
                            format!("index_{option}"),
                            json!([
                                rc,
                                if rc == 0 {
                                    raw(&buf[..len.into()])
                                } else {
                                    String::new()
                                }
                            ]),
                        );
                    }
                }
                // `openindex <name>`: `AdsOpenIndex`, and the current order.
                "openindex" => {
                    let mut handles = [0; 16];
                    let mut n = 16u16;
                    let rc = crate::AdsOpenIndex(*cur, text(), handles.as_mut_ptr(), &mut n);
                    result(&mut object, rc);
                    object.insert("n".into(), json!(if rc == 0 { n } else { 0 }));
                    let (rc, name) = current_order(*cur);
                    object.insert("order_rc".into(), json!(rc));
                    object.insert("order".into(), json!(name));
                }
                // `allindexes`: `AdsGetAllIndexes` and the names of its handles.
                "allindexes" => {
                    let mut handles = [0; 32];
                    let mut n = 32u16;
                    let rc = crate::AdsGetAllIndexes(*cur, handles.as_mut_ptr(), &mut n);
                    result(&mut object, rc);
                    object.insert("n".into(), json!(if rc == 0 { n } else { 0 }));
                    let names: Vec<String> = handles
                        .iter()
                        .take(if rc == 0 { n.into() } else { 0 })
                        .map(|&h| index_text(crate::AdsGetIndexName, h).1)
                        .collect();
                    object.insert("names".into(), json!(names));
                }
                // `byexpr <descending> <expression>`: `AdsGetIndexHandleByExpr`.
                "byexpr" => {
                    let (descending, expression) = rest.split_once(' ').unwrap_or((&rest, ""));
                    let expression = CString::new(expression).unwrap();
                    let mut handle = 0;
                    let rc = crate::AdsGetIndexHandleByExpr(
                        *cur,
                        expression.as_ptr().cast_mut().cast(),
                        descending.parse().unwrap(),
                        &mut handle,
                    );
                    result(&mut object, rc);
                    if rc == 0 {
                        object.insert(
                            "name".into(),
                            json!(index_text(crate::AdsGetIndexName, handle).1),
                        );
                    }
                }
                // Moves of the last cursor, or through one of its orders
                // (`idx…` with the order's number or name), and where it
                // is after them.
                "row" | "gotop" | "gobottom" | "skip" | "gorec" | "idxtop" | "idxbottom"
                | "idxskip" | "idxseek" | "idxseekx" | "idxseeklast" | "idxscope"
                | "idxclearscope" => {
                    let mut target = *cur;
                    let mut verb = verb;
                    let mut words = rest.splitn(2, ' ');
                    let mut rest = rest.as_str();
                    let mut failed = None;
                    if let Some(moved) = verb.strip_prefix("idx") {
                        let which = words.next().unwrap();
                        rest = words.next().unwrap_or("");
                        let mut handle = 0;
                        let rc = match which.parse::<u16>() {
                            Ok(n) => crate::AdsGetIndexHandleByOrder(*cur, n, &mut handle),
                            Err(_) => {
                                let name = CString::new(which).unwrap();
                                crate::AdsGetIndexHandle(
                                    *cur,
                                    name.as_ptr().cast_mut().cast(),
                                    &mut handle,
                                )
                            }
                        };
                        target = handle;
                        verb = moved;
                        if rc != 0 {
                            failed = Some(rc);
                        }
                    }
                    match failed {
                        Some(rc) => result(&mut object, rc),
                        None => {
                            let mut found = 0xAAAA;
                            let rc = match verb {
                                "top" | "gotop" => AdsGotoTop(target),
                                "bottom" | "gobottom" => crate::AdsGotoBottom(target),
                                "skip" => AdsSkip(target, rest.trim().parse().unwrap()),
                                "gorec" => crate::AdsGotoRecord(*cur, rest.trim().parse().unwrap()),
                                "seek" => {
                                    let key = CString::new(rest).unwrap();
                                    crate::AdsSeek(
                                        target,
                                        key.as_ptr().cast_mut().cast(),
                                        rest.len() as u16,
                                        2,
                                        2,
                                        &mut found,
                                    )
                                }
                                "seekx" | "seeklast" | "scope" => {
                                    let mut words =
                                        rest.splitn(if verb == "seeklast" { 2 } else { 3 }, ' ');
                                    let option = match verb {
                                        "seeklast" => "",
                                        _ => words.next().unwrap_or(""),
                                    };
                                    let kind = words.next().unwrap_or("");
                                    let text = words.next().unwrap_or("");
                                    let (mut key, kind) = match kind {
                                        "double" => {
                                            let number = text.trim().parse::<f64>().unwrap_or(0.0);
                                            (number.to_le_bytes().to_vec(), 4)
                                        }
                                        _ => (text.as_bytes().to_vec(), 2),
                                    };
                                    let len = key.len() as u16;
                                    key.push(0);
                                    let key = key.as_mut_ptr();
                                    match verb {
                                        "seeklast" => {
                                            crate::AdsSeekLast(target, key, len, kind, &mut found)
                                        }
                                        "scope" => crate::AdsSetScope(
                                            target,
                                            if option == "top" { 1 } else { 2 },
                                            key,
                                            len,
                                            kind,
                                        ),
                                        _ => crate::AdsSeek(
                                            target,
                                            key,
                                            len,
                                            kind,
                                            match option {
                                                "soft" => 1,
                                                "gt" => 4,
                                                _ => 2,
                                            },
                                            &mut found,
                                        ),
                                    }
                                }
                                "clearscope" => crate::AdsClearScope(
                                    target,
                                    if rest.trim() == "top" { 1 } else { 2 },
                                ),
                                _ => 0,
                            };
                            result(&mut object, rc);
                            if verb.starts_with("seek") {
                                object.insert("found".into(), json!(found));
                            }
                            let (mut eof, mut bof, mut recno) = (0xAAAA, 0xAAAA, 0xAAAA);
                            AdsAtEOF(*cur, &mut eof);
                            crate::AdsAtBOF(*cur, &mut bof);
                            AdsGetRecordNum(*cur, 2, &mut recno);
                            object.insert("eof".into(), json!(eof));
                            object.insert("bof".into(), json!(bof));
                            object.insert("recno".into(), json!(recno));
                            let (rc, name) = current_order(*cur);
                            object.insert("order_rc".into(), json!(rc));
                            object.insert("order".into(), json!(name));
                            let mut fields = 0;
                            AdsGetNumFields(*cur, &mut fields);
                            let mut values = Vec::new();
                            for f in 1..=fields {
                                let (mut buf, mut len) = ([0u8; 2000], 2000u32);
                                let rc = AdsGetField(*cur, field(f), buf.as_mut_ptr(), &mut len, 0);
                                values.push(match rc {
                                    0 => json!(raw(&buf[..len as usize])),
                                    rc => json!(format!("E{rc}")),
                                });
                            }
                            object.insert("fields".into(), Json::Array(values));
                        }
                    }
                }
                // `edit <field number> <text>`: `AdsSetString` on the last
                // cursor's current record, then `AdsWriteRecord`.
                "edit" => {
                    let (number, value) = rest.split_once(' ').unwrap_or((&rest, ""));
                    let value = CString::new(value).unwrap();
                    let rc = AdsSetString(
                        *cur,
                        field(number.parse().unwrap()),
                        value.as_ptr().cast_mut().cast(),
                        value.as_bytes().len() as u32,
                    );
                    let mut set = Map::new();
                    result(&mut set, rc);
                    object.insert("set".into(), Json::Object(set));
                    result(&mut object, crate::AdsWriteRecord(*cur));
                }
                // `keep <name>`: `AdsOpenTable`, left open as the cursor.
                "keep" => {
                    let mut table = 0;
                    let rc = crate::AdsOpenTable(
                        *connect,
                        text(),
                        std::ptr::null_mut(),
                        0,
                        1,
                        0,
                        1,
                        0,
                        &mut table,
                    );
                    result(&mut object, rc);
                    if rc == 0 {
                        *cur = table;
                    }
                }
                // `setstring | setfield <field number> <text>`: the setter on
                // the last cursor's current record, not written.
                "setstring" | "setfield" => {
                    let (number, value) = rest.split_once(' ').unwrap_or((&rest, ""));
                    let value = CString::new(value).unwrap();
                    let (field, value, len) = (
                        field(number.parse().unwrap()),
                        value.as_ptr().cast_mut().cast(),
                        value.as_bytes().len() as u32,
                    );
                    let rc = match cmd {
                        "setfield" => crate::AdsSetField(*cur, field, value, len),
                        _ => AdsSetString(*cur, field, value, len),
                    };
                    result(&mut object, rc);
                }
                "write" => result(&mut object, crate::AdsWriteRecord(*cur)),
                "append" => result(&mut object, crate::AdsAppendRecord(*cur)),
                "delrec" => result(&mut object, crate::AdsDeleteRecord(*cur)),
                "txcount" => {
                    let mut n = 0xAAAA;
                    result(
                        &mut object,
                        crate::transaction::AdsGetTransactionCount(*connect, &mut n),
                    );
                    object.insert("count".into(), json!(n));
                }
                "cap" => {
                    DUMP_CAP.set(rest.trim().parse().unwrap_or(200));
                    result(&mut object, 0);
                }
                "cachecursors" => {
                    let n = rest.trim().parse().unwrap_or(0);
                    result(&mut object, crate::settings::AdsCacheOpenCursors(n));
                }
                "connect" => {
                    let mut words = rest.split(' ');
                    let relative = words.next().unwrap_or("");
                    let (user, password) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
                    let source = if fixtures::is_remote() {
                        let share = format!(r"\\127.0.0.1:{}\sqldata", fixtures::server());
                        format!(r"{share}\{relative}")
                    } else {
                        let data = CONNECT_DATA.with_borrow(Clone::clone);
                        let mut path = data.unwrap_or_else(fixtures::sql_data);
                        for part in relative.split(['\\', '/']) {
                            path.push(part);
                        }
                        path.display().to_string()
                    };
                    let (rc, fresh) = fixtures::connect_to_as(&source, user, password);
                    result(&mut object, rc);
                    if rc == 0 {
                        *connect = fresh;
                        OPENED.with_borrow_mut(|opened| opened.push(fresh));
                    }
                }
                // `switch`: the connection, statement and cursor set aside (none
                // the first time) are the ones in use, for a second connection.
                "switch" => {
                    OTHER.with_borrow_mut(|other| {
                        let mine = [*connect, *stmt, *cur];
                        [*connect, *stmt, *cur] = std::mem::replace(other, mine);
                    });
                    result(&mut object, 0);
                }
                // `ddcreate <dd>`: `AdsDDCreate` of a dictionary in the data
                // folder, which the commands after it use.
                "ddcreate" => {
                    let data = CONNECT_DATA.with_borrow(Clone::clone);
                    let mut path = data.unwrap_or_else(fixtures::sql_data);
                    for part in rest.trim().split(['\\', '/']) {
                        path.push(part);
                    }
                    let mut path = format!("{}\0", path.display()).into_bytes();
                    let mut fresh = 0;
                    let rc =
                        crate::AdsDDCreate(path.as_mut_ptr(), 0, std::ptr::null_mut(), &mut fresh);
                    result(&mut object, rc);
                    if rc == 0 {
                        *connect = fresh;
                        OPENED.with_borrow_mut(|opened| opened.push(fresh));
                    }
                }
                "stmtparams" => {
                    // In Windows-1252, as the probe ran.
                    let mut bytes = tflip_storage::cp1252::encode(&rest);
                    let n = bytes.len();
                    bytes.push(0);
                    bytes.extend([0xAA; 15]);
                    let rc = AdsGetSQLStmtParams(bytes.as_mut_ptr());
                    result(&mut object, rc);
                    let text: String = bytes[..n + 16].iter().map(|&b| char::from(b)).collect();
                    object.insert("text".into(), json!(text));
                }
                // `ddview <name> <SELECT …>`: `AdsDDAddView`, without a comment.
                "ddview" => {
                    let (name, sql) = rest.split_once(' ').unwrap_or((&rest, ""));
                    let name = CString::new(name).unwrap();
                    let sql = CString::new(crate::ansi::from_str_lossy(sql)).unwrap();
                    let rc = crate::AdsDDAddView(
                        *connect,
                        name.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        sql.as_ptr().cast_mut().cast(),
                    );
                    result(&mut object, rc);
                }
                // `remove <path>`: deletes a file, to probe a rename with a
                // companion file missing.
                "remove" => {
                    let removed = std::fs::remove_file(rest.replace('\\', "/")).is_ok();
                    object.insert("rc".into(), json!(u32::from(!removed)));
                }
                // `ddrename <name> <new name> <type> <options>`: `AdsDDRenameObject`;
                // a name of `-` is null, `=` is empty and `~` stands for a space.
                "ddrename" => {
                    let words: Vec<&str> = rest.split(' ').collect();
                    let name = |i: usize| match words[i] {
                        "-" => None,
                        "=" => Some(CString::default()),
                        word => Some(CString::new(word.replace('~', " ")).unwrap()),
                    };
                    let pointer = |name: &Option<CString>| {
                        name.as_ref()
                            .map_or(std::ptr::null_mut(), |n| n.as_ptr().cast_mut().cast())
                    };
                    let (old, new) = (name(0), name(1));
                    let rc = crate::AdsDDRenameObject(
                        *connect,
                        pointer(&old),
                        pointer(&new),
                        words[2].parse().unwrap(),
                        words[3].parse().unwrap(),
                    );
                    result(&mut object, rc);
                }
                // `dduser <name> <password>`: `AdsDDCreateUser`, in no group.
                "dduser" => {
                    let (name, password) = rest.split_once(' ').unwrap_or((&rest, ""));
                    let name = CString::new(name).unwrap();
                    let password = CString::new(password).unwrap();
                    let rc = crate::AdsDDCreateUser(
                        *connect,
                        std::ptr::null_mut(),
                        name.as_ptr().cast_mut().cast(),
                        match password.as_bytes() {
                            [] => std::ptr::null_mut(),
                            _ => password.as_ptr().cast_mut().cast(),
                        },
                        std::ptr::null_mut(),
                    );
                    result(&mut object, rc);
                }
                // `dddbprop <id> <length> <text>`: `AdsDDSetDatabaseProperty`.
                "dddbprop" => {
                    let words: Vec<&str> = rest.splitn(3, ' ').collect();
                    let id = words[0].parse().unwrap();
                    let text = match words[2] {
                        "-" => "",
                        text => text,
                    };
                    let length = match words[1].parse::<i32>().unwrap() {
                        n if n < 0 => text.len() as i32 + 1,
                        n => n,
                    };
                    // The buffer is as long as the call says, whatever the text is.
                    let mut buffer = text.as_bytes().to_vec();
                    buffer.resize(buffer.len().max(length as usize), 0);
                    let rc = crate::AdsDDSetDatabaseProperty(
                        *connect,
                        id,
                        buffer.as_mut_ptr().cast(),
                        length as u16,
                    );
                    result(&mut object, rc);
                }
                // `ddlink <alias> <path> <user> <password> <options>`:
                // `AdsDDCreateLink`.
                "ddlink" => {
                    let words: Vec<CString> =
                        rest.split(' ').map(|w| CString::new(w).unwrap()).collect();
                    // A user or password of `-` is none (null).
                    let word = |i: usize| match words[i].as_bytes() {
                        b"-" => std::ptr::null_mut(),
                        _ => words[i].as_ptr().cast_mut().cast::<Unsigned8>(),
                    };
                    let options = words[4].to_str().unwrap().parse().unwrap();
                    let rc = crate::AdsDDCreateLink(
                        *connect,
                        word(0),
                        word(1),
                        word(2),
                        word(3),
                        options,
                    );
                    result(&mut object, rc);
                }
                // `ddpub`, `ddart`, `ddsub`, `ddpubcomment`, `ddsubcomment`, `dddelpub`
                // and `dddelsub` (`tools/ace/sql/sql.c` lists their words): the
                // replication objects. A word of `-` is null, `~` stands for a space.
                "ddpub" | "ddart" | "ddsub" | "ddpubcomment" | "ddsubcomment" | "dddelpub"
                | "dddelsub" => {
                    let words: Vec<CString> = rest
                        .split(' ')
                        .map(|w| CString::new(w.replace('~', " ")).unwrap())
                        .collect();
                    let word = |i: usize| words[i].as_ptr().cast_mut().cast::<Unsigned8>();
                    let optional = |i: usize| match words[i].as_bytes() {
                        b"-" => std::ptr::null_mut(),
                        _ => word(i),
                    };
                    let number = |i: usize| words[i].to_str().unwrap().parse::<u32>().unwrap();
                    let comment = |i: usize| (word(i).cast(), words[i].as_bytes().len() as u16);
                    let rc = match verb {
                        "ddpub" => {
                            crate::AdsDDCreatePublication(*connect, word(0), optional(1), number(2))
                        }
                        "ddart" => crate::AdsDDCreateArticle(
                            *connect,
                            word(0),
                            word(1),
                            optional(2),
                            std::ptr::null_mut(),
                            number(3),
                        ),
                        "ddsub" => crate::AdsDDCreateSubscription(
                            *connect,
                            word(0),
                            word(1),
                            word(2),
                            word(3),
                            word(4),
                            word(5),
                            number(6) as u16,
                            optional(7),
                            number(8),
                        ),
                        "ddpubcomment" => {
                            let (text, length) = comment(1);
                            crate::AdsDDSetPublicationProperty(*connect, word(0), 1, text, length)
                        }
                        "ddsubcomment" => {
                            let (text, length) = comment(1);
                            crate::AdsDDSetSubscriptionProperty(*connect, word(0), 1, text, length)
                        }
                        "dddelpub" => crate::AdsDDDeletePublication(*connect, word(0)),
                        _ => crate::AdsDDDeleteSubscription(*connect, word(0)),
                    };
                    result(&mut object, rc);
                }
                // `ddapi <function> <args>`: one of the `AdsDD` calls whose arguments
                // are strings and integers (`tools/ace/sql/sql.c` says how the
                // arguments are written: `-` null, `=` empty, `#n` a number, `#L`
                // the length of the string before it plus one, `~` a space).
                "ddapi" => {
                    let (function, args) = rest.split_once(' ').unwrap_or((&rest, ""));
                    let mut strings: Vec<Option<CString>> = Vec::new();
                    let mut numbers: Vec<u32> = Vec::new();
                    for word in args.split(' ').filter(|w| !w.is_empty()) {
                        let (string, number) = match word {
                            "-" => (None, 0),
                            "=" => (Some(CString::default()), 0),
                            "#L" => (
                                None,
                                strings
                                    .last()
                                    .and_then(Option::as_ref)
                                    .map_or(0, |s| s.as_bytes().len() as u32 + 1),
                            ),
                            n if n.starts_with('#') => (None, n[1..].parse().unwrap()),
                            text => (Some(CString::new(text.replace('~', " ")).unwrap()), 0),
                        };
                        strings.push(string);
                        numbers.push(number);
                    }
                    let p = |i: usize| -> *mut Unsigned8 {
                        strings[i]
                            .as_ref()
                            .map_or(std::ptr::null_mut(), |s| s.as_ptr().cast_mut().cast())
                    };
                    let v = |i: usize| p(i).cast::<std::ffi::c_void>();
                    let n = |i: usize| numbers[i];
                    let c = *connect;
                    let rc = match function {
                        "AdsDDRemoveTable" => crate::AdsDDRemoveTable(c, p(0), n(1) as u16),
                        "AdsDDRemoveView" => crate::AdsDDRemoveView(c, p(0)),
                        "AdsDDRemoveProcedure" => crate::AdsDDRemoveProcedure(c, p(0)),
                        "AdsDDRemoveTrigger" => crate::AdsDDRemoveTrigger(c, p(0)),
                        "AdsDDRemoveRefIntegrity" => crate::AdsDDRemoveRefIntegrity(c, p(0)),
                        "AdsDDDeleteUser" => crate::AdsDDDeleteUser(c, p(0)),
                        "AdsDDDeleteUserGroup" => crate::AdsDDDeleteUserGroup(c, p(0)),
                        "AdsDDDeletePublication" => crate::AdsDDDeletePublication(c, p(0)),
                        "AdsDDDeleteSubscription" => crate::AdsDDDeleteSubscription(c, p(0)),
                        "AdsDDDeleteArticle" => crate::AdsDDDeleteArticle(c, p(0), p(1)),
                        "AdsDDRemoveUserFromGroup" => {
                            crate::AdsDDRemoveUserFromGroup(c, p(0), p(1))
                        }
                        "AdsDDAddUserToGroup" => crate::AdsDDAddUserToGroup(c, p(0), p(1)),
                        "AdsDDCreateUserGroup" => crate::AdsDDCreateUserGroup(c, p(0), p(1)),
                        "AdsDDDeleteIndex" => crate::AdsDDDeleteIndex(c, p(0), p(1)),
                        "AdsDDRemoveIndexFile" => {
                            crate::AdsDDRemoveIndexFile(c, p(0), p(1), n(2) as u16)
                        }
                        "AdsDDAddIndexFile" => crate::AdsDDAddIndexFile(c, p(0), p(1), p(2)),
                        "AdsDDAddTable" => crate::AdsDDAddTable(
                            c,
                            p(0),
                            p(1),
                            n(2) as u16,
                            n(3) as u16,
                            p(4),
                            p(5),
                        ),
                        "AdsDDSetTableProperty" => crate::AdsDDSetTableProperty(
                            c,
                            p(0),
                            n(1) as u16,
                            v(2),
                            n(3) as u16,
                            n(4) as u16,
                            p(5),
                        ),
                        "AdsDDSetFieldProperty" => crate::AdsDDSetFieldProperty(
                            c,
                            p(0),
                            p(1),
                            n(2) as u16,
                            v(3),
                            n(4) as u16,
                            n(5) as u16,
                            p(6),
                        ),
                        "AdsDDSetViewProperty" => {
                            crate::AdsDDSetViewProperty(c, p(0), n(1) as u16, v(2), n(3) as u16)
                        }
                        "AdsDDSetUserProperty" => {
                            crate::AdsDDSetUserProperty(c, p(0), n(1) as u16, v(2), n(3) as u16)
                        }
                        "AdsDDSetUserGroupProperty" => crate::AdsDDSetUserGroupProperty(
                            c,
                            p(0),
                            n(1) as u16,
                            v(2),
                            n(3) as u16,
                        ),
                        "AdsDDSetProcedureProperty" => crate::AdsDDSetProcedureProperty(
                            c,
                            p(0),
                            n(1) as u16,
                            v(2),
                            n(3) as u16,
                        ),
                        "AdsDDSetPublicationProperty" => crate::AdsDDSetPublicationProperty(
                            c,
                            p(0),
                            n(1) as u16,
                            v(2),
                            n(3) as u16,
                        ),
                        "AdsDDSetSubscriptionProperty" => crate::AdsDDSetSubscriptionProperty(
                            c,
                            p(0),
                            n(1) as u16,
                            v(2),
                            n(3) as u16,
                        ),
                        "AdsDDSetIndexProperty" => crate::AdsDDSetIndexProperty(
                            c,
                            p(0),
                            p(1),
                            n(2) as u16,
                            v(3),
                            n(4) as u16,
                        ),
                        "AdsDDSetArticleProperty" => crate::AdsDDSetArticleProperty(
                            c,
                            p(0),
                            p(1),
                            n(2) as u16,
                            v(3),
                            n(4) as u16,
                        ),
                        "AdsDDGrantPermission" => {
                            crate::AdsDDGrantPermission(c, n(0) as u16, p(1), p(2), p(3), n(4))
                        }
                        "AdsDDCreateArticle" => {
                            crate::AdsDDCreateArticle(c, p(0), p(1), p(2), p(3), n(4))
                        }
                        other => panic!("ddapi: {other} is not known"),
                    };
                    result(&mut object, rc);
                }
                // `droplink <alias> <global>`: `AdsDDDropLink`.
                "droplink" => {
                    let (alias, global) = rest.split_once(' ').unwrap();
                    let alias = CString::new(alias).unwrap();
                    let global = global.trim().parse().unwrap();
                    let rc =
                        crate::AdsDDDropLink(*connect, alias.as_ptr().cast_mut().cast(), global);
                    result(&mut object, rc);
                }
                "numlinks" => {
                    let mut links = 0xAAAA;
                    result(
                        &mut object,
                        crate::AdsGetNumActiveLinks(*connect, &mut links),
                    );
                    object.insert("links".into(), json!(links));
                }
                // `linkinfo <number> <size>`: `AdsGetActiveLinkInfo`, the
                // length written back counting the data folder as `<data>`.
                "linkinfo" => {
                    let (number, size) = rest.split_once(' ').unwrap();
                    let mut len: u16 = size.trim().parse().unwrap();
                    let mut buf = [0xEEu8; 1100];
                    let number = number.parse().unwrap();
                    let rc =
                        crate::AdsGetActiveLinkInfo(*connect, number, buf.as_mut_ptr(), &mut len);
                    result(&mut object, rc);
                    let written = buf.iter().position(|&b| b == 0xEE).unwrap_or(buf.len());
                    let shown = match rc {
                        0 | crate::AE_INSUFFICIENT_BUFFER => {
                            let data = LINK_DATA.with_borrow(|d| d.clone()).unwrap_or_default();
                            usize::from(len) - data.len() + 6
                        }
                        _ => usize::from(len),
                    };
                    object.insert("len".into(), json!(shown));
                    object.insert("value".into(), json!(raw(&buf[..written])));
                }
                // `find62 [mask]`, `find [mask]`: a whole table listing.
                "find62" | "find" => {
                    let mask = CString::new(rest.as_str()).unwrap();
                    let mask = match rest.is_empty() {
                        true => std::ptr::null_mut(),
                        false => mask.as_ptr().cast_mut().cast(),
                    };
                    let mut names = Vec::new();
                    let mut find = 0;
                    let mut rc;
                    loop {
                        let (mut name, mut dd) = ([0u8; 300], [0x5Au8; 300]);
                        let (mut len, mut dd_len) = (250u16, 250u16);
                        let (n, d) = (name.as_mut_ptr(), dd.as_mut_ptr());
                        rc = match (names.is_empty(), verb == "find62") {
                            (true, true) => crate::AdsFindFirstTable62(
                                *connect,
                                mask,
                                d,
                                &mut dd_len,
                                n,
                                &mut len,
                                &mut find,
                            ),
                            (true, false) => {
                                crate::AdsFindFirstTable(*connect, mask, n, &mut len, &mut find)
                            }
                            (false, true) => crate::AdsFindNextTable62(
                                *connect,
                                find,
                                d,
                                &mut dd_len,
                                n,
                                &mut len,
                            ),
                            (false, false) => crate::AdsFindNextTable(*connect, find, n, &mut len),
                        };
                        if rc != 0 || names.len() >= 100 {
                            break;
                        }
                        let text = |bytes: &[u8], len: u16| {
                            let bytes = &bytes[..usize::from(len)];
                            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                            raw(&bytes[..end])
                        };
                        let mut entry = vec![json!(text(&name, len)), json!(len)];
                        if verb == "find62" {
                            entry.extend([json!(text(&dd, dd_len)), json!(dd_len)]);
                        }
                        names.push(Json::Array(entry));
                    }
                    object.insert("names".into(), Json::Array(names));
                    result(&mut object, rc);
                    if find != 0 {
                        crate::AdsFindClose(*connect, find);
                    }
                }
                // `objects <type> [parent]`: a whole `AdsDDFindFirstObject`
                // listing.
                "objects" => {
                    let (kind, parent) = rest.split_once(' ').unwrap_or((&rest, ""));
                    let kind = kind.parse().unwrap();
                    let parent_c = CString::new(parent).unwrap();
                    let parent = match parent.is_empty() {
                        true => std::ptr::null_mut(),
                        false => parent_c.as_ptr().cast_mut().cast(),
                    };
                    let mut names = Vec::new();
                    let mut find = 0;
                    let mut rc;
                    loop {
                        let (mut name, mut len) = ([0u8; 300], 250u16);
                        rc = match names.is_empty() {
                            true => crate::AdsDDFindFirstObject(
                                *connect,
                                kind,
                                parent,
                                name.as_mut_ptr(),
                                &mut len,
                                &mut find,
                            ),
                            false => crate::AdsDDFindNextObject(
                                *connect,
                                find,
                                name.as_mut_ptr(),
                                &mut len,
                            ),
                        };
                        if rc != 0 || names.len() >= 100 {
                            break;
                        }
                        let bytes = &name[..usize::from(len)];
                        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                        names.push(json!([raw(&bytes[..end]), len]));
                    }
                    object.insert("names".into(), Json::Array(names));
                    result(&mut object, rc);
                    if find != 0 {
                        crate::AdsDDFindClose(*connect, find);
                    }
                }
                // `open <name>`: `AdsOpenTable`, recorded as a cursor.
                "open" => {
                    let mut table = 0;
                    let rc = crate::AdsOpenTable(
                        *connect,
                        text(),
                        std::ptr::null_mut(),
                        0,
                        1,
                        0,
                        1,
                        0,
                        &mut table,
                    );
                    result(&mut object, rc);
                    if rc == 0 {
                        object.insert("cursor".into(), cursor(table));
                        AdsCloseTable(table);
                    }
                }
                // `otype <type> <name>`: `AdsOpenTable` of <name> as given, as
                // table type <type>, recorded as a cursor.
                "otype" => {
                    let (table_type, name) = rest.split_once(' ').unwrap();
                    let name = CString::new(name).unwrap();
                    let mut table = 0;
                    let rc = crate::AdsOpenTable(
                        *connect,
                        name.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        table_type.parse().unwrap(),
                        1,
                        0,
                        1,
                        0,
                        &mut table,
                    );
                    result(&mut object, rc);
                    if rc == 0 {
                        object.insert("cursor".into(), cursor(table));
                        AdsCloseTable(table);
                    }
                }
                // `ctable <type> <name> <fields>`: `AdsCreateTable`.
                "ctable" => {
                    let mut parts = rest.splitn(3, ' ');
                    let table_type: u16 = parts.next().unwrap().parse().unwrap();
                    let name = CString::new(parts.next().unwrap()).unwrap();
                    let fields = CString::new(parts.next().unwrap()).unwrap();
                    let mut table = 0;
                    let rc = crate::AdsCreateTable(
                        *connect,
                        name.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        table_type,
                        1,
                        0,
                        1,
                        0,
                        fields.as_ptr().cast_mut().cast(),
                        &mut table,
                    );
                    result(&mut object, rc);
                    if rc == 0 {
                        AdsCloseTable(table);
                    }
                }
                // `transfree <type> <name>`: `AdsOpenTable` as table type <type>
                // (0 on a dictionary connection, else the table in the data
                // folder), and its `AdsIsTableTransactionFree`.
                "transfree" => {
                    let (table_type, name) = rest.split_once(' ').unwrap();
                    let table_type: u16 = table_type.parse().unwrap();
                    let path = match table_type {
                        0 => name.to_owned(),
                        _ => {
                            let data = CREATED_DATA.with_borrow(Clone::clone).unwrap();
                            data.join(name).display().to_string()
                        }
                    };
                    let path = CString::new(path).unwrap();
                    let mut table = 0;
                    let rc = crate::AdsOpenTable(
                        *connect,
                        path.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        table_type,
                        1,
                        0,
                        1,
                        0,
                        &mut table,
                    );
                    result(&mut object, rc);
                    if rc == 0 {
                        let mut free = 0xAAAA;
                        let rc = crate::AdsIsTableTransactionFree(table, &mut free);
                        object.insert("free_rc".into(), json!(rc));
                        object.insert("trans_free".into(), json!(free));
                        AdsCloseTable(table);
                    }
                }
                // `getcollation`: `AdsGetCollation` of the connection.
                "getcollation" => {
                    let (mut buf, mut len) = ([0u8; 300], 300u16);
                    let rc = crate::AdsGetCollation(*connect, buf.as_mut_ptr(), &mut len);
                    result(&mut object, rc);
                    let text = if rc == 0 {
                        raw(&buf[..len.into()])
                    } else {
                        String::new()
                    };
                    object.insert("collation".into(), json!(text));
                }
                "disconnect" => {
                    OPENED.with_borrow_mut(|opened| opened.retain(|&open| open != *connect));
                    result(&mut object, AdsDisconnect(*connect))
                }
                other => panic!("command {other}"),
            }
        }
        Json::Object(object)
    }

    /// Declares a test for each script: it replays it, and compares what it
    /// made if the fixture says so, but for the files `unchecked`.
    macro_rules! replays {
        ($($(#[$doc:meta])* $name:ident($file:literal $(, [$($unchecked:literal),*])?);)*) => {$(
            $(#[$doc])*
            #[test]
            fn $name() {
                replay_compared($file, &[$($($unchecked),*)?]);
            }
        )*};
    }

    replays! {
    /// Replays `fixtures/sql/statements.json` (see `tools/ace/sql/`), the
    /// original engine's results of a script of statements, and compares
    /// each result.
    statements_match_the_original_engine("statements.json");
    /// Replays `fixtures/sql/select.json`: joins, grouping and aggregates,
    /// `UNION` and subqueries (`tools/ace/sql/select.txt`).
    selects_match_the_original_engine("select.json");
    /// Replays `fixtures/sql/plans.json`: `SHOW PLAN FOR` (`tools/ace/sql/plans.txt`).
    plans_match_the_original_engine("plans.json");
    /// Replays `fixtures/sql/dml.json`: `INSERT`, `UPDATE` and `DELETE`
    /// (`tools/ace/sql/dml.txt`).
    dml_matches_the_original_engine("dml.json");
    /// Replays `fixtures/sql/dbf.json`: SQL on the Clipper, FoxPro and VFP
    /// tables the statement's table type makes (`tools/ace/sql/dbf.txt`),
    /// in an empty folder.
    dbf_matches_the_original_engine("dbf.json");
    /// Replays `fixtures/sql/groupconcat.json`: `GROUP_CONCAT` with `DISTINCT`,
    /// an inner `ORDER BY` and a `SEPARATOR` (`tools/ace/sql/groupconcat.txt`),
    /// on tables it makes in an empty folder.
    group_concat_matches_the_original_engine("groupconcat.json");
    /// Replays `fixtures/sql/funcs.json`: the `DATABASE()`, `NEWID()`,
    /// `NEWIDSTRING()` and `RAND()` scalar functions
    /// (`tools/ace/sql/funcs.txt`).
    scalar_functions_match_the_original_engine("funcs.json");
    /// Replays `fixtures/sql/iif.json`: which arguments `IIF`, `CASE`,
    /// `COALESCE` and `IFNULL` evaluate (`tools/ace/sql/iif.txt`), on a
    /// table it makes in an empty folder.
    conditionals_match_the_original_engine("iif.json");
    /// Replays `fixtures/sql/wherefilter.json`: what a `WHERE` run as a
    /// filter evaluates, and where its errors surface
    /// (`tools/ace/sql/wherefilter.txt`), on a table it makes in an empty
    /// folder.
    where_filters_evaluate_as_the_original_does("wherefilter.json");
    /// Replays `fixtures/sql/wherefilter3.json`: which conjuncts of a `WHERE`
    /// are run as a filter (`tools/ace/sql/wherefilter3.txt`).
    where_conjuncts_run_as_filters_as_in_the_original("wherefilter3.json");
    /// Replays `fixtures/sql/wherefilter4.json`: which constructs of a
    /// `WHERE` make a one-table `SELECT` a static cursor
    /// (`tools/ace/sql/wherefilter4.txt`).
    where_constructs_make_static_cursors_as_in_the_original("wherefilter4.json");
    /// Replays `fixtures/sql/wherefilter5.json`: the same for scalar
    /// functions and operators (`tools/ace/sql/wherefilter5.txt`).
    where_functions_make_static_cursors_as_in_the_original("wherefilter5.json");
    /// Replays `fixtures/sql/wherefilter6.json`: constant conditions in a
    /// `WHERE` run as a filter (`tools/ace/sql/wherefilter6.txt`).
    where_constants_run_as_filters_as_in_the_original("wherefilter6.json");
    /// Replays `fixtures/sql/literals.json`: `TIME`, `DATE` and `TIMESTAMP`
    /// literals, text assigned to dates, GUID text, `CURRENT_TIMESTAMP(n)`,
    /// `AVG` of integers, a unary `+`, subqueries' columns, memos in
    /// `DISTINCT`, `GROUP BY` and `ORDER BY`, `ORDER BY` of a `UNION` and
    /// `RIGHT JOIN` (`tools/ace/sql/literals.txt`), on tables it makes in an
    /// empty folder.
    literals_match_the_original_engine("literals.json");
    /// Replays `fixtures/sql/gram_shift.json`: the shifts `<<` and `>>`
    /// (`tools/ace/sql/gram_shift.txt`), on tables it makes in an empty
    /// folder.
    shifts_match_the_original_engine("gram_shift.json");
    /// Replays `fixtures/sql/gram_bin.json`: binary literals (`x'2A 2B'`)
    /// (`tools/ace/sql/gram_bin.txt`), on a table it makes in an empty folder.
    binary_literals_match_the_original_engine("gram_bin.json");
    /// Replays `fixtures/sql/gram_cast.json`: constant numbers as logicals,
    /// `CAST` to type names, binary data converted, `ABS`, and the syntax
    /// errors of a missing operand (`tools/ace/sql/gram_cast.txt`), on a
    /// table it makes in an empty folder.
    cast_grammar_matches_the_original_engine("gram_cast.json");
    /// Replays `fixtures/sql/gram_syntax.json`: the wording and place of
    /// syntax errors, and small grammar differences
    /// (`tools/ace/sql/gram_syntax.txt`), on tables it makes in an empty
    /// folder.
    syntax_errors_match_the_original_engine("gram_syntax.json");
    /// Replays `fixtures/sql/claims.json`: grammar, typing, optimizer and
    /// execution edge cases, each on a fresh copy of the tables it needs
    /// (`tools/ace/sql/claims.txt`), in an empty folder.
    sql_edge_cases_match_the_original_engine("claims.json");
    /// Replays `fixtures/sql/claims_huge.json`: the plans of the claims on
    /// a table of 384 000 rows (`tools/ace/sql/claims_huge.txt`).
    #[ignore = "takes minutes without optimizations (cargo test --release -- --ignored)"]
    sql_edge_cases_on_a_huge_table_match_the_original_engine("claims_huge.json");
    /// Replays `fixtures/sql/gram_cast2.json`: numbers and text where a
    /// logical is wanted (`tools/ace/sql/gram_cast2.txt`), on a table it
    /// makes in an empty folder.
    numbers_as_logicals_match_the_original_engine("gram_cast2.json");
    /// Replays `fixtures/sql/gram_cast3.json`: the type names and sizes of
    /// `CAST` and `CONVERT` (`tools/ace/sql/gram_cast3.txt`).
    cast_types_match_the_original_engine("gram_cast3.json");
    /// Replays `fixtures/sql/gram_script.json`: the script forms `ELSEIF`,
    /// `ENDIF`, `ENDWHILE`, `ENDTRY`, `END TRY` and a bare `END`
    /// (`tools/ace/sql/gram_script.txt`).
    script_ends_match_the_original_engine("gram_script.json");
    /// Replays `fixtures/sql/gram_oj.json`: the ODBC escape `{OJ …}`
    /// (`tools/ace/sql/gram_oj.txt`), on tables it makes in an empty folder.
    odbc_outer_joins_match_the_original_engine("gram_oj.json");
    /// Replays `fixtures/sql/gram_cross.json`: where the original refuses the
    /// `CROSS JOIN` keywords (`tools/ace/sql/gram_cross.txt`), on tables it
    /// makes in an empty folder.
    cross_join_matches_the_original_engine("gram_cross.json");
    /// Replays `fixtures/sql/gram_charlen.json`: `CHAR_LENGTH(x USING unit)`
    /// (`tools/ace/sql/gram_charlen.txt`), on a table it makes in an empty
    /// folder.
    char_length_using_matches_the_original_engine("gram_charlen.json");
    /// Replays `fixtures/sql/gram_sysvar.json`:
    /// `::stmt.unicode_sort_key_size` and `::conn.TerminalClientAddress`
    /// (`tools/ace/sql/gram_sysvar.txt`).
    unicode_sort_key_size_matches_the_original_engine("gram_sysvar.json");
    /// Replays `fixtures/sql/modulo.json`: the precedence of `%`
    /// (`tools/ace/sql/modulo.txt`).
    modulo_matches_the_original_engine("modulo.json");
    /// Replays `fixtures/sql/rowfuncs.json`: `APPLICATIONID`, `LASTAUTOINC`,
    /// `LASTROWID`, `LASTROWVERSION`, `RECNO`, `DELETED`, `ROWNUM`, `EMPTY`,
    /// `ROWID`, `TTOC` and `COLLATE` (`tools/ace/sql/rowfuncs.txt`), on
    /// tables it makes in an empty folder.
    row_functions_match_the_original_engine("rowfuncs.json");
    /// Replays `fixtures/sql/percent.json`: `TOP n PERCENT`
    /// (`tools/ace/sql/percent.txt`), on tables it makes in an empty folder.
    top_percent_matches_the_original_engine("percent.json");
    /// Replays `fixtures/sql/scripts.json`: scripts sent as a statement
    /// and `EXECUTE IMMEDIATE` (`tools/ace/sql/scripts.txt`), on tables it
    /// makes in an empty folder.
    scripts_match_the_original_engine("scripts.json");
    /// Replays `fixtures/sql/props39.json`: the unpublished property 39 that
    /// makes the next `AdsVerifySQL` check the syntax alone
    /// (`tools/ace/sql/props39.txt`), in an empty folder.
    parse_only_matches_the_original_engine("props39.json");
    /// Replays `fixtures/sql/cursors.json`: cursors in scripts
    /// (`tools/ace/sql/cursors.txt`), on tables it makes in an empty folder.
    script_cursors_match_the_original_engine("cursors.json");
    /// Replays `fixtures/sql/cursorlive.json`: what a cursor of a script sees
    /// of the changes made to its tables after `OPEN`
    /// (`tools/ace/sql/cursorlive.txt`), on tables it makes in an empty
    /// folder.
    script_cursors_read_live_as_the_original_engine("cursorlive.json");
    /// Replays `fixtures/sql/cursormore2.json`: the same for a cursor that
    /// reads its table through the index of its `ORDER BY` column
    /// (`tools/ace/sql/cursormore2.txt`), on a table it makes in an empty
    /// folder.
    script_cursors_through_an_index_read_live_as_the_original_engine("cursormore2.json");
    /// Replays `fixtures/sql/cursormore3.json`: the same for a cursor on a
    /// join of two tables (`tools/ace/sql/cursormore3.txt`), on tables it
    /// makes in an empty folder.
    script_cursors_over_a_join_read_live_as_the_original_engine("cursormore3.json");
    /// Replays `fixtures/sql/cursormore4.json`: the same for a cursor on a
    /// `LEFT JOIN`, a join with an `ORDER BY` or a second condition, a join
    /// of three tables, a `UNION ALL` and a `SELECT` in `FROM`
    /// (`tools/ace/sql/cursormore4.txt`), on tables it makes in an empty
    /// folder.
    script_cursors_over_more_read_live_as_the_original_engine("cursormore4.json");
    /// Replays `fixtures/sql/cursormore5.json`: the same for the joins with
    /// no equality, a `RIGHT JOIN`, a `LEFT JOIN` with a condition beyond its
    /// equality, a joined or sorted `SELECT` in `FROM`, and a join of three
    /// tables with an index or a condition (`tools/ace/sql/cursormore5.txt`),
    /// on tables it makes in an empty folder.
    script_cursors_over_the_rest_read_live_as_the_original_engine("cursormore5.json");
    /// Replays `fixtures/sql/trycatch.json`: `TRY ... CATCH ... FINALLY` in
    /// scripts (`tools/ace/sql/trycatch.txt`), on tables it makes in an empty
    /// folder.
    try_catch_matches_the_original_engine("trycatch.json");
    /// Replays `fixtures/sql/txscripts.json`: transactions in scripts and
    /// around `TRY ... CATCH` (`tools/ace/sql/txscripts.txt`, recorded on the
    /// remote server), on tables it makes in an empty folder.
    script_transactions_match_the_original_engine("txscripts.json");
    /// Replays `fixtures/sql/txprocs.json`: transactions in stored
    /// procedures and functions (`tools/ace/sql/txprocs.txt`, recorded on the
    /// remote server), in a dictionary it makes.
    procedure_transactions_match_the_original_engine("txprocs.json");
    /// Replays `fixtures/sql/txtrig.json`: transactions in triggers and
    /// referential integrity rules in the caller's transaction
    /// (`tools/ace/sql/txtrig.txt`, recorded on the remote server), in a
    /// dictionary it makes.
    trigger_transactions_match_the_original_engine("txtrig.json");
    /// Replays `fixtures/sql/txtrig2.json`: a trigger's savepoints, which take
    /// back the insert an `AFTER INSERT` trigger rolls back for, and the call
    /// a `NO TRANSACTION` trigger is (`tools/ace/sql/txtrig2.txt`, recorded on
    /// the remote server), in a dictionary it makes.
    trigger_savepoints_match_the_original_engine("txtrig2.json");
    /// Replays `fixtures/sql/txtrig3.json`: a trigger created and dropped on a
    /// second connection while a statement of the first holds the table
    /// (`tools/ace/sql/txtrig3.txt`), in a dictionary it makes.
    triggers_on_two_connections_match_the_original_engine("txtrig3.json");
    /// Replays `fixtures/sql/txddl.json`: DDL in a transaction
    /// (`tools/ace/sql/txddl.txt`, recorded on the remote server), on free
    /// tables it makes.
    ddl_in_transactions_matches_the_original_engine("txddl.json");
    /// Replays `fixtures/sql/declare.json`: the types of a script's
    /// variables and the values assigned to them (`tools/ace/sql/declare.txt`),
    /// in an empty folder.
    declared_variables_match_the_original_engine("declare.json");
    /// Replays `fixtures/sql/trigvars.json`: the variables a trigger reads of
    /// the change that fires it (`tools/ace/sql/trigvars.txt`), on a copy of
    /// the psm dictionary.
    trigger_variables_match_the_original_engine("trigvars.json");
    /// Replays `fixtures/sql/trigtyped.json`: the variables of a trigger
    /// body, typed where its statements read them
    /// (`tools/ace/sql/trigtyped.txt`).
    trigger_body_variables_are_typed_as_the_original_types_them("trigtyped.json");
    /// Replays `fixtures/sql/castchar.json`: how wide `CAST(x AS SQL_CHAR)`
    /// is for each type of `x` (`tools/ace/sql/castchar.txt`).
    cast_to_char_is_as_wide_as_the_original_makes_it("castchar.json");
    /// Replays `fixtures/sql/castchar2.json`: the same for the types of
    /// table columns and of `CAST`s (`tools/ace/sql/castchar2.txt`), on a
    /// table it makes in an empty folder.
    cast_to_char_is_as_wide_for_more_types("castchar2.json");
    /// Replays `fixtures/sql/tsarith.json`: arithmetic of a `TIMESTAMP`,
    /// `DATE` and `TIME` with numbers and each other
    /// (`tools/ace/sql/tsarith.txt`), on a table it makes in an empty folder.
    date_arithmetic_matches_the_original_engine("tsarith.json");
    /// Replays `fixtures/sql/tsarith2.json`: dates, times and timestamps
    /// with text (`tools/ace/sql/tsarith2.txt`).
    date_arithmetic_with_text_matches_the_original_engine("tsarith2.json");
    /// Replays `fixtures/sql/staticorder.json`: which static cursors have an
    /// index of their own for their `ORDER BY` and which are read in the
    /// order of one of the table's (`tools/ace/sql/staticorder.txt`), on a
    /// table it makes in an empty folder.
    static_cursors_are_ordered_as_in_the_original("staticorder.json");
    /// Replays `fixtures/sql/params.json`: the types the original gives
    /// parameters nothing else types, its errors for the ones it cannot,
    /// and parameter values kept by a re-prepared statement
    /// (`tools/ace/sql/params.txt`).
    parameter_types_match_the_original_engine("params.json");
    /// Replays `fixtures/sql/params4.json`: which values a re-prepared
    /// statement keeps, by position and by name, and when a failed prepare
    /// or run changes that (`tools/ace/sql/params4.txt`).
    reprepared_statements_keep_parameter_values_as_the_original_does("params4.json");
    /// Replays `fixtures/sql/params5.json`: a name that comes twice in the
    /// SQL is two parameters, and `AdsGetNumParams` counts the markers of
    /// any text prepared (`tools/ace/sql/params5.txt`).
    repeated_names_and_parameter_counts_match_the_original_engine("params5.json");
    /// Replays `fixtures/sql/params2.json`: parameters in `CASE`, `IIF`,
    /// `IN`, `CAST` and after a sign or `NOT`, typed by their names or not
    /// (`tools/ace/sql/params2.txt`).
    parameters_in_conditionals_match_the_original_engine("params2.json");
    /// Replays `fixtures/sql/params3.json`: text for a number, and numbers and
    /// dates for a date, given to parameters, and a parameter of a statement
    /// that does not parse (`tools/ace/sql/params3.txt`).
    parameter_conversions_match_the_original_engine("params3.json");
    /// Replays `fixtures/sql/gram_assign.json`: numbers with more decimals than
    /// a column or variable holds, as literals, expressions and parameters,
    /// and parameters read as other types (`tools/ace/sql/gram_assign.txt`).
    assignments_that_round_match_the_original_engine("gram_assign.json");
    /// Replays `fixtures/sql/guid.json`: text parameters compared with and
    /// written to a GUID column, and text that is no GUID
    /// (`tools/ace/sql/guid.txt`).
    guid_parameters_match_the_original_engine("guid.json");
    /// Replays `fixtures/sql/planupd.json`: the plans of `UPDATE`, with and
    /// without `FROM`, and of `DELETE` and `INSERT` (`tools/ace/sql/planupd.txt`).
    update_plans_match_the_original_engine("planupd.json");
    /// Replays `fixtures/sql/spddnum.json`: the numbers the dictionary
    /// procedures take for their INTEGER and BOOL parameters
    /// (`tools/ace/sql/spddnum.txt`).
    numbers_for_procedure_parameters_match_the_original_engine("spddnum.json");
    /// Replays `fixtures/sql/spddparam.json`: the dictionary procedures
    /// called with `?` and `:name` parameters (`tools/ace/sql/spddparam.txt`).
    procedure_parameters_match_the_original_engine("spddparam.json");
    /// Replays `fixtures/sql/procvars.json`: what a stored procedure reads
    /// of its input (`__input`), its statement and the tables
    /// (`tools/ace/sql/procvars.txt`), in a dictionary it makes.
    procedure_variables_match_the_original_engine("procvars.json");
    /// Replays `fixtures/sql/cursorprocs.json`: cursors in the bodies of
    /// procedures (`tools/ace/sql/cursorprocs.txt`), in a dictionary it makes.
    cursors_in_procedures_match_the_original_engine("cursorprocs.json");
    /// Replays `fixtures/sql/cursortrig.json`: cursors in the bodies of
    /// triggers (`tools/ace/sql/cursortrig.txt`), in a dictionary it makes.
    cursors_in_triggers_match_the_original_engine("cursortrig.json");
    /// Replays `fixtures/sql/loops.json`: `LEAVE` and `CONTINUE` in `WHILE`
    /// loops of scripts, procedures and triggers (`tools/ace/sql/loops.txt`),
    /// in a dictionary it makes.
    loop_control_matches_the_original_engine("loops.json");
    /// Replays `fixtures/sql/subq.json`: subqueries in the expressions of a
    /// script (`tools/ace/sql/subq.txt`), in an empty folder.
    subqueries_in_scripts_match_the_original_engine("subq.json");
    /// Replays `fixtures/sql/limits.json`: the limits on field lengths, names
    /// and the counts of columns, tags and `ORDER BY` items on each table type
    /// (`tools/ace/sql/limits.txt`, made by `limits.py`), in an empty folder.
    limits_match_the_original_engine("limits.json");
    /// Replays `fixtures/sql/udf.json`: user-defined functions that run SQL
    /// (`tools/ace/sql/udf.txt`), on a copy of the dictionary the original
    /// made (`tools/ace/sql/udfdict.txt`, `fixtures/sql/udf/`).
    functions_that_run_sql_match_the_original_engine("udf.json");
    /// Replays `fixtures/sql/sysvars.json`: the system variables
    /// (`tools/ace/sql/sysvars.txt`), in an empty folder.
    system_variables_match_the_original_engine("sysvars.json");
    /// Replays `fixtures/sql/temptables.json`: temporary tables (`#name`)
    /// (`tools/ace/sql/temptables.txt`), in an empty folder.
    temporary_tables_match_the_original_engine("temptables.json");
    /// Replays `fixtures/sql/temptables_types.json`: statements that read a
    /// temporary table made as another table type than theirs
    /// (`tools/ace/sql/temptables_types.txt`), in an empty folder.
    temporary_tables_of_other_types_match_the_original_engine("temptables_types.json");
    /// Replays `fixtures/sql/temptables_api.json`: temporary tables made and
    /// opened by `AdsCreateTable` and `AdsOpenTable`, and the refusal of a
    /// table type other than the table was made as
    /// (`tools/ace/sql/temptables_api.txt`), in an empty folder.
    temporary_tables_through_the_api_match_the_original_engine("temptables_api.json");
    /// Replays `fixtures/sql/temptables_into.json`: `SELECT … INTO #name`, in
    /// each table type (`tools/ace/sql/temptables_into.txt`), in an empty
    /// folder.
    select_into_temporary_tables_matches_the_original_engine("temptables_into.json");
    /// Replays `fixtures/sql/temptables_into2.json`: the fields `SELECT … INTO
    /// #name` makes of each result column in the DBF table types
    /// (`tools/ace/sql/temptables_into2.txt`), in an empty folder.
    select_into_temporary_dbf_fields_match_the_original_engine("temptables_into2.json");
    /// Replays `fixtures/sql/temptables_into3.json`: the types a DBF table
    /// cannot hold, and NULL in a VFP one
    /// (`tools/ace/sql/temptables_into3.txt`), in an empty folder.
    select_into_temporary_dbf_refusals_match_the_original_engine("temptables_into3.json");
    /// Replays `fixtures/sql/temptables_into4.json`: the refusal of a name of
    /// more than ten characters (`tools/ace/sql/temptables_into4.txt`), in an
    /// empty folder.
    select_into_temporary_dbf_names_match_the_original_engine("temptables_into4.json");
    /// Replays `fixtures/sql/temptables_api2.json`: `AdsOpenTable` of a
    /// temporary table as the other of Clipper and FoxPro, before and after a
    /// statement has opened it (`tools/ace/sql/temptables_api2.txt`), in an
    /// empty folder.
    temporary_tables_open_as_either_dbf_type_until_a_statement_does("temptables_api2.json");
    /// Replays `fixtures/sql/temptables_api3.json`: the same for a table
    /// `SELECT … INTO #name` made (`tools/ace/sql/temptables_api3.txt`), in an
    /// empty folder.
    select_into_temporary_tables_are_fixed_by_their_statement("temptables_api3.json");
    /// Replays `fixtures/sql/system.json`: `sp_GetSQLKeywords`, the
    /// `system.systemgroups` built-in view and the `system.articles` alias of
    /// `system.publicationarticles` (`tools/ace/sql/system.txt`).
    system_tables_match_the_original_engine("system.json");
    /// Replays `fixtures/sql/spdict.json`: `sp_GetTables`/`sp_GetColumns` on a
    /// dictionary connection, which list the dictionary's own tables and views
    /// (and the system tables) rather than the folder's free tables
    /// (`tools/ace/sql/spdict.txt`).
    dictionary_procedures_match_the_original_engine("spdict.json");
    /// Replays `fixtures/sql/spview.json`: `sp_GetColumns` on a dictionary's
    /// views, and `sp_GetTables`/`sp_GetColumns` following its links, on a
    /// copy of the props dictionary the script adds views and a link to
    /// (`tools/ace/sql/spview.txt`).
    view_and_link_procedures_match_the_original_engine("spview.json");
    /// Replays `fixtures/sql/links.json`: tables reached through a
    /// dictionary's links (`link.table`) and dictionary paths, read and
    /// written, with the linked dictionary's logins, rights and rules, and
    /// what fails (`tools/ace/sql/links.txt`).
    links_match_the_original_engine("links.json");
    /// Replays `fixtures/sql/dbfdict.json`: DBF tables of a dictionary the
    /// original engine made (`fixtures/dictionary/dbftables`), opened by name
    /// and read and written through SQL (`tools/ace/sql/dbfdict.txt`).
    dbf_tables_in_a_dictionary_match_the_original_engine("dbfdict.json");
    /// Replays `fixtures/sql/linklist.json`: a dictionary's links outside
    /// SQL — the tables of linked dictionaries in the table listings, tables
    /// opened through a link by name, and the links a connection has active
    /// (`tools/ace/sql/linklist.txt`).
    link_listings_match_the_original_engine("linklist.json");
    /// Replays `fixtures/sql/fts_plans.json` (`tools/ace/sql/fts.txt`) on a
    /// copy of `fixtures/sql/fts`: `SHOW PLAN` of `CONTAINS` with other
    /// predicates, of static cursors and of indexes that cut long words, and
    /// queries over a FIXED full-text index.
    full_text_plans_match_the_original_engine("fts_plans.json");
    /// Replays `fixtures/sql/triggers.json`: CONFLICT triggers, which can be
    /// made but never fire on a table's own changes, the `NO MEMOS` and
    /// `NO TRANSACTION` options, and what a failing trigger takes back, on a
    /// copy of the psm dictionary (`tools/ace/sql/triggers.txt`, recorded on
    /// the remote server, whose transactions tflip has).
    triggers_match_the_original_engine("triggers.json");
    /// Replays `fixtures/sql/trigerr.json`: the error number and text a
    /// trigger writes to its `__error` table, and what fails with them, on a
    /// copy of the psm dictionary (`tools/ace/sql/trigerr.txt`).
    trigger_errors_match_the_original_engine("trigerr.json");
    /// Replays `fixtures/sql/trigproc.json`: a trigger calling a stored
    /// procedure, its arguments, cursors and failures, on a copy of the psm
    /// dictionary (`tools/ace/sql/trigproc.txt`).
    triggers_call_procedures_as_the_original_does("trigproc.json");
    /// Replays `fixtures/sql/trigbig.json`: triggers whose script is 65,535
    /// bytes, which the original keeps unreadable and fails every statement
    /// of the event with (5005), and compares the dictionary
    /// (`tools/ace/sql/trigbig.txt`, made by `trigbig.py`;
    /// `fixtures/sql/trigbig/`).
    triggers_of_65535_bytes_fail_as_the_original_fails_them("trigbig.json");
    /// Replays `fixtures/sql/trigorder.json`: the order the triggers of one
    /// timing and priority fire in, after creates, drops and re-creates
    /// (`tools/ace/sql/trigorder.txt`).
    triggers_fire_in_the_order_the_original_fires_them("trigorder.json");
    /// Replays `fixtures/sql/trigorder2.json`: the triggers made right after
    /// the dictionary's own were dropped, which stand in their places
    /// (`tools/ace/sql/trigorder2.txt`).
    triggers_made_in_dropped_places_fire_as_the_original_fires_them("trigorder2.json");
    /// Replays `fixtures/sql/trigproc2.json`: how deep a trigger's recursion
    /// goes, with procedures in it (`tools/ace/sql/trigproc2.txt`).
    trigger_recursion_is_as_deep_as_the_original_goes("trigproc2.json");
    /// Replays `fixtures/sql/collation_compare_default.json`: the same
    /// script under the default static collation, where text compares
    /// byte-wise.
    default_collation_comparisons_match_the_original_engine("collation_compare_default.json");
    /// Replays `fixtures/sql/views.json`: which views of a dictionary open
    /// a live cursor on their table (`tools/ace/sql/views.txt`).
    views_match_the_original_engine("views.json");
    /// Replays `fixtures/sql/orderby.json`: the temporary index a cursor
    /// gets for an `ORDER BY` no index serves (`tools/ace/sql/orderby.txt`).
    temporary_indexes_match_the_original_engine("orderby.json");
    /// Replays `fixtures/sql/ddtables.json`: `CREATE TABLE` and `DROP
    /// TABLE` on a dictionary connection (`tools/ace/sql/ddtables.txt`), in
    /// an empty folder, then compares the dictionary and the table files it
    /// leaves with the original engine's (`fixtures/sql/ddtables/`), the
    /// dictionary's login secret aside (it is salted anew on every write).
    dictionary_tables_match_the_original_engine("ddtables.json");
    /// Replays `fixtures/sql/ddviews.json`: `CREATE VIEW` and `DROP VIEW` on
    /// a dictionary connection (`tools/ace/sql/ddviews.txt`), in an empty
    /// folder, then compares the dictionary and the tables with the original
    /// engine's (`fixtures/sql/ddviews/`).
    dictionary_views_match_the_original_engine("ddviews.json");
    /// Replays `fixtures/sql/ddslots.json`: what each kind of object added
    /// to a dictionary takes as its slot when objects were deleted
    /// (`tools/ace/sql/ddslots.txt`), in an empty folder, then compares the
    /// dictionary and the tables with the original engine's
    /// (`fixtures/sql/ddslots/`).
    dictionary_slots_match_the_original_engine("ddslots.json");
    /// Replays `fixtures/sql/ddfieldlen.json`: the length and decimals a
    /// dictionary records for a field of each type
    /// (`tools/ace/sql/ddfieldlen.txt`), in an empty folder, then compares
    /// the dictionary and the tables with the original engine's
    /// (`fixtures/sql/ddfieldlen/`).
    dictionary_field_lengths_match_the_original_engine("ddfieldlen.json");
    /// Replays `fixtures/sql/ddrights.json`: what a user who is not the
    /// administrator may create, alter and drop in a dictionary, with the
    /// rights it is granted, through its groups and as a member of `DB:Admin`
    /// (`tools/ace/sql/ddrights.txt`), in an empty folder, then compares the
    /// dictionary and the tables with the original engine's
    /// (`fixtures/sql/ddrights/`).
    dictionary_creation_rights_match_the_original_engine("ddrights.json");
    /// Replays `fixtures/sql/ddgrantopt.json`: what a user who is not the
    /// administrator may grant and revoke, the rights it holds with the grant
    /// option and no others (`tools/ace/sql/ddgrantopt.txt`), in an empty
    /// folder, then compares the dictionary and the tables with the original
    /// engine's (`fixtures/sql/ddgrantopt/`).
    grants_need_the_grant_option_as_in_the_original_engine("ddgrantopt.json");
    /// Replays `fixtures/sql/ddgrantopt2.json`: the same on functions,
    /// packages, links, users and groups (`tools/ace/sql/ddgrantopt2.txt`), in
    /// an empty folder, then compares the dictionary and the tables with the
    /// original engine's (`fixtures/sql/ddgrantopt2/`).
    grants_on_other_objects_need_the_grant_option_as_in_the_original_engine("ddgrantopt2.json");
    /// Replays `fixtures/sql/ddfuncrights.json`: a function, a package and
    /// the other objects renamed by a user who is not the administrator, with
    /// `ALTER FUNCTION` and `AdsDDRenameObject`, and which of the right and
    /// the names is checked first (`tools/ace/sql/ddfuncrights.txt`), in an
    /// empty folder, then compares the dictionary and the tables with the
    /// original engine's (`fixtures/sql/ddfuncrights/`).
    renames_by_users_need_the_alter_right_as_in_the_original_engine("ddfuncrights.json");
    /// Replays `fixtures/sql/ddapirights.json`: the `AdsDD` calls that drop
    /// objects or set their properties, made by a user with no rights, with
    /// `ALTER`, with `DROP`, with both and in `DB:Admin`
    /// (`tools/ace/sql/ddapirights.txt`), in an empty folder, then compares
    /// the dictionary and the tables with the original engine's
    /// (`fixtures/sql/ddapirights/`).
    api_drops_and_setters_need_rights_as_in_the_original_engine("ddapirights.json", ["ar.ai"]);
    /// Replays `fixtures/sql/ddapirights2.json`: what the `AdsDD` calls left
    /// out of `ddapirights.json` do for the administrator and for users with
    /// no rights, `ALTER`, `DROP`, both and in `DB:Admin`
    /// (`tools/ace/sql/ddapirights2.txt`): a property id the engine does not
    /// know, `AdsDDRemoveTrigger`, `AdsDDAddIndexFile` of a file that is not
    /// there and the subscription calls, in an empty folder, then compares
    /// the dictionary and the tables with the original engine's
    /// (`fixtures/sql/ddapirights2/`).
    api_property_ids_triggers_and_subscriptions_are_the_original_engines("ddapirights2.json", ["sr.ai"]);
    /// Replays `fixtures/sql/ddrenamerights.json`: users, publications,
    /// triggers, integrity rules and tables renamed through
    /// `AdsDDRenameObject` by users with the right to alter the object, its
    /// table or none (`tools/ace/sql/ddrenamerights.txt`), in an empty folder,
    /// then compares the dictionary and the tables with the original
    /// engine's (`fixtures/sql/ddrenamerights/`).
    renames_need_alter_on_the_object_or_table_as_in_the_original_engine("ddrenamerights.json", ["ar.ai"]);
    /// Replays `fixtures/sql/ddfuncgroup.json`: functions and packages
    /// created, altered and dropped by users with the rights through their
    /// groups (`tools/ace/sql/ddfuncgroup.txt`), in an empty folder, then
    /// compares the dictionary with the original engine's
    /// (`fixtures/sql/ddfuncgroup/`).
    functions_and_packages_follow_the_rights_of_groups_as_in_the_original_engine(
        "ddfuncgroup.json"
    );
    /// Replays `fixtures/sql/ddadminpk.json`: the tables a user who is not the
    /// administrator creates with and without `IN DATABASE`, a primary key
    /// among them (`tools/ace/sql/ddadminpk.txt`), in an empty folder, then
    /// compares the dictionary and the tables with the original engine's
    /// (`fixtures/sql/ddadminpk/`).
    tables_with_keys_by_users_are_the_original_engines("ddadminpk.json");
    /// Replays `fixtures/sql/ddbuiltin.json`: users added to and taken from
    /// the built-in groups, what the members of `DB:Admin` may do and the
    /// members of the other groups may not (`tools/ace/sql/ddbuiltin.txt`), in
    /// an empty folder, then compares the dictionary and the tables with the
    /// original engine's (`fixtures/sql/ddbuiltin/`).
    built_in_group_members_are_those_of_the_original_engine("ddbuiltin.json");
    /// Replays `fixtures/sql/ddvisible.json`: what the system tables and the
    /// catalog procedures show the administrator, a user with a right on one
    /// table and a user with none (`tools/ace/sql/ddvisible.txt`), in an
    /// empty folder, then compares the dictionary with the original
    /// engine's (`fixtures/sql/ddvisible/`).
    system_tables_show_a_user_what_its_rights_allow("ddvisible.json");
    /// Replays `fixtures/sql/ddenc.json`: the encryption columns of
    /// `system.dictionary` in dictionaries that are encrypted, have a table
    /// encryption password, encrypt new tables and indexes
    /// (`tools/ace/sql/ddenc.txt`, on `fixtures/adt/ddenc/`).
    dictionary_encryption_columns_match_the_original_engine("ddenc.json");
    /// Replays `fixtures/sql/ddlogins.json`: the connections a dictionary
    /// whose logins are disabled, or a user whose own are, refuses
    /// (`tools/ace/sql/ddlogins.txt`), in an empty folder, then compares the
    /// dictionary with the original engine's (`fixtures/sql/ddlogins/`).
    disabled_logins_match_the_original_engine("ddlogins.json");
    /// Replays `fixtures/sql/ddtrigrights.json`: which rights let a user who
    /// is not the administrator run `sp_DisableTriggers` and
    /// `sp_EnableTriggers` (`tools/ace/sql/ddtrigrights.txt`), in an empty
    /// folder, then compares the dictionary with the original engine's
    /// (`fixtures/sql/ddtrigrights/`).
    disabling_triggers_needs_the_right_to_alter_the_table("ddtrigrights.json");
    /// Replays `fixtures/sql/ddcolrights.json`: what a user who may read and
    /// update only some columns of a table gets from queries, updates and
    /// the table API at each `TABLE_PERMISSION_LEVEL`
    /// (`tools/ace/sql/ddcolrights.txt`), in an empty folder, then compares
    /// the dictionary and the table with the original engine's
    /// (`fixtures/sql/ddcolrights/`).
    column_rights_match_the_original_engine("ddcolrights.json");
    /// Replays `fixtures/sql/ddinsertonly.json`: what a user who may insert,
    /// update or delete but not select gets from statements and from the
    /// table API at each `TABLE_PERMISSION_LEVEL`
    /// (`tools/ace/sql/ddinsertonly.txt`), in an empty folder, then compares
    /// the dictionary and the table with the original engine's
    /// (`fixtures/sql/ddinsertonly/`).
    users_without_the_right_to_select_match_the_original_engine("ddinsertonly.json");
    /// Replays `fixtures/sql/ddinsertonly2.json`: an `INSERT ... SELECT`
    /// that reads the table it inserts into, and what a user holding INSERT,
    /// UPDATE or SELECT sets through the table API on a record it appended
    /// and on one that was there (`tools/ace/sql/ddinsertonly2.txt`), in an
    /// empty folder, then compares the dictionary and the table with the
    /// original engine's (`fixtures/sql/ddinsertonly2/`).
    insert_select_and_api_setters_of_users_without_select_match_the_original_engine("ddinsertonly2.json");
    /// Replays `fixtures/sql/ddinsertonly3.json`: a `DELETE` whose `WHERE` or
    /// subquery reads the table it deletes from, or another one, for users
    /// holding `DELETE` alone, with `SELECT`, with `SELECT` on a column and
    /// with `SELECT` on the other table (`tools/ace/sql/ddinsertonly3.txt`),
    /// in an empty folder, then compares the dictionary and the table with
    /// the original engine's (`fixtures/sql/ddinsertonly3/`).
    deletes_of_users_without_the_right_to_select_match_the_original_engine("ddinsertonly3.json");
    /// Replays `fixtures/sql/ddeffperm.json`: the permissions and effective
    /// permissions of a dictionary of three tables filtered by `Object_Type`,
    /// where `= 4` lists the columns of the first table only
    /// (`tools/ace/sql/ddeffperm.txt`), in an empty folder.
    permissions_filtered_by_object_type_match_the_original_engine("ddeffperm.json");
    /// Replays `fixtures/sql/ddpublicgrant.json`: `GRANT ALL` and `REVOKE ALL`
    /// to `DB:Public` by a user holding some, a part or none of the rights
    /// with the grant option (`tools/ace/sql/ddpublicgrant.txt`), in an empty
    /// folder, then compares the dictionary with the original engine's
    /// (`fixtures/sql/ddpublicgrant/`).
    grants_to_public_by_users_with_little_to_give_match_the_original_engine("ddpublicgrant.json");
    /// Replays `fixtures/sql/ddpublicbad.json` and `ddpublicbad2.json`: what a
    /// table, and a view, whose rights slot for `DB:Public` does not check
    /// does to the administrator and to a user
    /// (`tools/ace/sql/ddpublicbad.txt`, `ddpublicbad2.txt`), on copies of
    /// the dictionaries in `fixtures/sql/ddpublicbad/` and `ddpublicbad2/`.
    a_rights_slot_for_public_that_does_not_check_matches_the_original_engine("ddpublicbad.json");
    a_view_with_a_rights_slot_for_public_that_does_not_check_matches_the_original_engine("ddpublicbad2.json");
    /// Replays `fixtures/sql/ddpublicbad3.json`, `ddpublicbad4.json` and
    /// `ddpublicbad5.json`: what a procedure, then a function, user, group,
    /// package, link, publication, subscription and the database together,
    /// then a table, whose rights slot for `DB:Public` does not check does
    /// to the permissions, the listings and the calls
    /// (`tools/ace/sql/ddpublicbad3.txt` to `ddpublicbad5.txt`), on copies
    /// of the dictionaries in `fixtures/sql/ddpublicbad3/` to `ddpublicbad5/`.
    a_procedure_with_a_rights_slot_for_public_that_does_not_check_matches_the_original_engine("ddpublicbad3.json");
    objects_of_every_kind_with_a_rights_slot_for_public_that_does_not_check_match_the_original_engine("ddpublicbad4.json");
    a_table_with_a_rights_slot_for_public_that_does_not_check_shows_odd_permissions_as_the_original_does("ddpublicbad5.json");
    /// Replays `fixtures/sql/ddpublicbad6.json` and `ddpublicbad7.json`: a
    /// member of `DB:Admin` is the administrator for the listings that fail
    /// for an ordinary user when a procedure's, and a view's, rights slot for
    /// `DB:Public` does not check, and `sp_GetColumns` over them
    /// (`tools/ace/sql/ddpublicbad6.txt`, `ddpublicbad7.txt`).
    a_member_of_the_admin_group_sees_a_bad_procedure_slot_as_the_administrator_does("ddpublicbad6.json");
    a_member_of_the_admin_group_sees_a_bad_view_slot_as_the_administrator_does("ddpublicbad7.json");
    /// Replays `fixtures/sql/ddpublicbad8.json`: the same for a table's slot
    /// (`tools/ace/sql/ddpublicbad8.txt`).
    a_member_of_the_admin_group_sees_a_bad_table_slot_as_the_administrator_does("ddpublicbad8.json");
    /// Replays `fixtures/sql/ddpublicbad9.json`: the effective permissions of
    /// the other rows, and `sp_GetColumns` of a good view, for the same table
    /// (`tools/ace/sql/ddpublicbad9.txt`).
    effective_permissions_and_columns_next_to_a_bad_table_slot_match_the_original_engine("ddpublicbad9.json");
    /// Replays `fixtures/sql/ddexecrights.json`: functions, packages and
    /// procedures a user executes only with the right to
    /// (`tools/ace/sql/ddexecrights.txt`), on a copy of the dictionary in
    /// `fixtures/sql/ddexecrights/`.
    executing_functions_and_procedures_needs_the_right_as_in_the_original_engine("ddexecrights.json");
    /// Replays `fixtures/sql/ddprocs.json`: `CREATE`, `ALTER` and `DROP
    /// PROCEDURE` on a dictionary connection (`tools/ace/sql/ddprocs.txt`),
    /// in an empty folder, then compares the dictionary and the table with
    /// the original engine's (`fixtures/sql/ddprocs/`).
    dictionary_sql_procedures_match_the_original_engine("ddprocs.json");
    /// Replays `fixtures/sql/ddprocs2.json`: the types of a procedure's
    /// parameters beyond those of `ddprocs.json`, the FoxPro names among them
    /// (`tools/ace/sql/ddprocs2.txt`), then compares the dictionary with the
    /// original engine's (`fixtures/sql/ddprocs2/`).
    dictionary_procedure_parameter_types_match_the_original_engine("ddprocs2.json");
    /// Replays `fixtures/sql/ddfuncs.json`: `CREATE`, `ALTER` and `DROP
    /// FUNCTION`, `CREATE` and `DROP PACKAGE` on a dictionary connection
    /// (`tools/ace/sql/ddfuncs.txt`), in an empty folder, then compares the
    /// dictionary and the table with the original engine's
    /// (`fixtures/sql/ddfuncs/`).
    dictionary_sql_functions_match_the_original_engine("ddfuncs.json");
    /// Replays `fixtures/sql/trigdb.json`: `CREATE` and `DROP TRIGGER ... ON
    /// DATABASE` (`tools/ace/sql/trigdb.txt`) in an empty folder, then
    /// compares the dictionary and the tables with the original engine's
    /// (`fixtures/sql/trigdb/`).
    database_triggers_match_the_original_engine("trigdb.json");
    /// Replays `fixtures/sql/trigdbfire.json`: database triggers on
    /// `CONNECT` and `DISCONNECT` (`tools/ace/sql/trigdbfire.txt`): their
    /// `__info`, a failing one refusing the connection with 7109, but for
    /// ADSSYS. The dictionary is compared, but not the audit table, which
    /// holds the folder's path.
    database_connect_triggers_match_the_original_engine(
        "trigdbfire.json",
        ["audit.adt"]
    );
    /// Replays `fixtures/sql/trigdbopen.json`: database triggers on
    /// `OPEN_TABLE` that fail, cancelling the open of a statement or a client
    /// (`tools/ace/sql/trigdbopen.txt`), and `CLOSE_TABLE` ones, whose
    /// failure is ignored.
    database_open_triggers_match_the_original_engine("trigdbopen.json");
    /// Replays `fixtures/sql/trigdblog.json`: what `OPEN_TABLE` and
    /// `CLOSE_TABLE` triggers that log see of the tables statements open and
    /// close (`tools/ace/sql/trigdblog.txt`): when each fires, and the open
    /// count, sharing, type and client's open `__info` tells.
    statements_open_and_close_tables_as_the_original_does("trigdblog.json", ["logt.adt", "dbt.am"]);
    /// Replays `fixtures/sql/trigdblog2.json`: the same for triggers that
    /// fail, procedures and the tables a table trigger opens
    /// (`tools/ace/sql/trigdblog2.txt`).
    triggers_fail_and_nest_as_the_original_does("trigdblog2.json", ["logt.adt", "dbt.am"]);
    /// Replays `fixtures/sql/trigdblog3.json`: the same across connections,
    /// and the `UserID` and `StmtCount` of `CONNECT` and `DISCONNECT`
    /// triggers (`tools/ace/sql/trigdblog3.txt`).
    connections_open_and_close_tables_as_the_original_does(
        "trigdblog3.json",
        ["logt.adt", "logc.adt", "dbt.am"]
    );
    /// Replays `fixtures/sql/trigself.json`: triggers writing their own table
    /// and transaction statements in a trigger (`tools/ace/sql/trigself.txt`).
    triggers_writing_their_own_table_match_the_original_engine("trigself.json");
    /// Replays `fixtures/sql/trignn.json`: a dictionary field's `NOT NULL`
    /// is checked before the `BEFORE` and `INSTEAD OF` triggers that could
    /// fill the field (`tools/ace/sql/trignn.txt`).
    triggers_after_a_fields_not_null_match_the_original_engine("trignn.json");
    /// Replays `fixtures/sql/ddproclong.json`: `CREATE`, `ALTER` and `DROP
    /// PROCEDURE` with parameter lists longer than the dictionary record's
    /// property field (`tools/ace/sql/ddproclong.txt`), then compares the
    /// dictionary with the original engine's (`fixtures/sql/ddproclong/`).
    dictionary_sql_long_procedures_match_the_original_engine("ddproclong.json");
    /// Replays `fixtures/sql/spdd*.json`: the system procedures that change
    /// a dictionary's objects (`tools/ace/sql/spdd*.txt`), in an empty folder
    /// with a folder `de`, then compares the dictionary and the tables with
    /// the original engine's (`fixtures/sql/spdd*/`).
    user_procedures_match_the_original_engine("spddusers.json");
    /// Replays `fixtures/sql/spddpwchange.json`: who may change a user's
    /// password through `sp_ModifyUserProperty` and `AdsDDSetUserProperty`,
    /// and whether the old one is asked for (`tools/ace/sql/spddpwchange.txt`),
    /// then compares the dictionary with the original engine's
    /// (`fixtures/sql/spddpwchange/`).
    users_changing_passwords_match_the_original_engine("spddpwchange.json");
    /// Replays `fixtures/sql/spddadminpw.json`: setting the administrator's
    /// password, and the users, groups, rights and links written with the
    /// dictionary key under it, by the administrator and by users
    /// (`tools/ace/sql/spddadminpw.txt`), then compares the dictionary with
    /// the original engine's (`fixtures/sql/spddadminpw/`).
    administrator_password_matches_the_original_engine("spddadminpw.json");
    referential_integrity_procedures_match_the_original_engine("spddri.json");
    trigger_procedures_match_the_original_engine("spddtrig.json");
    dictionary_table_procedures_match_the_original_engine("spddtables.json");
    rename_procedure_matches_the_original_engine("spddrename.json");
    /// Replays `fixtures/sql/spddrenameapi.json`: `AdsDDRenameObject`
    /// (`tools/ace/sql/spddrenameapi.txt`), which renames what
    /// `sp_RenameDDObject` does, and compares the dictionary and the tables.
    rename_api_matches_the_original_engine("spddrenameapi.json");
    /// Replays `fixtures/sql/spddrenamerepl.json`: `AdsDDRenameObject` and
    /// `sp_RenameDDObject` of publications and subscriptions
    /// (`tools/ace/sql/spddrenamerepl.txt`): the articles and subscriptions
    /// that name a publication follow it, and which names are taken.
    replication_rename_matches_the_original_engine("spddrenamerepl.json");
    /// Replays `fixtures/sql/spddrenamerepl2.json`: the names every kind of
    /// object is refused or given among those of publications and
    /// subscriptions (`tools/ace/sql/spddrenamerepl2.txt`).
    replication_names_match_the_original_engine("spddrenamerepl2.json", []);
    /// Replays `fixtures/sql/ddrepl.json`: what is left of the replication
    /// objects (`tools/ace/sql/ddrepl.txt`): their listings and system
    /// tables, what deleting a publication that subscriptions use and
    /// creating an article or subscription of a missing object are told, the
    /// queue name without an extension, and the memo blocks of their
    /// comments. The dictionary's index is not compared: the original
    /// interleaves the deletes and inserts of its keys record by record, where
    /// tflip brings it up to date once per call.
    replication_objects_match_the_original_engine("ddrepl.json", ["de.ai"]);
    /// Replays `fixtures/sql/ddrepl2.json`: the memo blocks of the comments
    /// of publications and subscriptions as they are made, set and freed, in
    /// a dictionary small enough to compare its index too
    /// (`tools/ace/sql/ddrepl2.txt`).
    replication_comments_match_the_original_engine("ddrepl2.json");
    /// Replays `fixtures/sql/spddrenamedbf.json`: `AdsDDRenameObject` of
    /// DBF tables (`tools/ace/sql/spddrenamedbf.txt`): their memo, index and
    /// structural index files, and what stops a rename part way.
    dbf_rename_matches_the_original_engine("spddrenamedbf.json", []);
    encryption_procedure_matches_the_original_engine("spddenc.json");
    security_procedure_matches_the_original_engine("spddsec.json");
    link_procedures_match_the_original_engine("spddlinks.json");
    referential_integrity_messages_match_the_original_engine("spddrilong.json");
    /// Replays `fixtures/sql/spddlong.json`: functions, procedures, views and
    /// triggers whose scripts run past the record's property field
    /// (`tools/ace/sql/spddlong.txt`), the memo file's allocation counts and
    /// stale bytes included.
    long_scripts_match_the_original_engine("spddlong.json");
    /// Replays `fixtures/sql/spddlongalter.json`: `ALTER TABLE` of a
    /// dictionary's table with a default, limits and messages that run past
    /// the record's property field (`tools/ace/sql/spddlongalter.txt`).
    long_column_properties_match_the_original_engine("spddlongalter.json");
    /// Replays `fixtures/sql/spddlongalter2.json`: the same with
    /// continuations of more than 32 blocks
    /// (`tools/ace/sql/spddlongalter2.txt`).
    very_long_column_properties_match_the_original_engine("spddlongalter2.json");
    /// Replays `fixtures/sql/spddlongalter3.json`: `ADD COLUMN` with parts
    /// of a long field's properties (`tools/ace/sql/spddlongalter3.txt`).
    long_column_property_parts_match_the_original_engine("spddlongalter3.json", []);
    /// Replays `fixtures/sql/spddlongidx.json`: orders whose expression and
    /// condition run past the record's property field, added and dropped
    /// (`tools/ace/sql/spddlongidx.txt`).
    long_order_properties_match_the_original_engine("spddlongidx.json");
    /// Replays `fixtures/sql/spddidxcomment.json`: the comments of orders,
    /// short and long, changed and dropped with their order or table
    /// (`tools/ace/sql/spddidxcomment.txt`).
    order_comments_match_the_original_engine("spddidxcomment.json");
    /// Replays `fixtures/sql/spddidxcomment2.json`: the same with long
    /// expressions, and through `ALTER TABLE`
    /// (`tools/ace/sql/spddidxcomment2.txt`).
    order_comments_with_long_properties_match_the_original_engine("spddidxcomment2.json");
    /// Replays `fixtures/sql/spddri2.json`: the refusal of a `RESTRICT` rule
    /// with no message, and a rule over rows that break it
    /// (`tools/ace/sql/spddri2.txt`).
    restrict_refusals_match_the_original_engine("spddri2.json");
    /// Replays `fixtures/sql/dddatabase.json`: `CREATE DATABASE`
    /// (`tools/ace/sql/dddatabase.txt`), in an empty folder with a folder
    /// `dc`, then compares the dictionaries it made with the original
    /// engine's (`fixtures/sql/dddatabase/`).
    created_dictionaries_match_the_original_engine("dddatabase.json");
    /// Replays `fixtures/sql/execproc.json`: `EXECUTE PROCEDURE` as a table in
    /// `FROM`, on a copy of the psm dictionary (`tools/ace/sql/execproc.txt`).
    procedures_in_from_match_the_original_engine("execproc.json");
    /// Replays `fixtures/sql/refuse.json`: the statements tflip refuses with
    /// 2200 and the syntax errors of those the original has no statement of
    /// (`tools/ace/sql/refuse.txt`), and `CACHE PREPARE`.
    refused_statements_match_the_original_engine("refuse.json");
    /// Replays `fixtures/sql/spt.json`: the table system procedures
    /// (`sp_PackTable`, `sp_Reindex`, `sp_GetTableSizeInfo`, ...) on ADT
    /// tables (`tools/ace/sql/spt.txt`), then compares the files they leave
    /// with the original engine's (`fixtures/sql/spt/`).
    table_procedures_match_the_original_engine("spt.json");
    /// Replays `fixtures/sql/sptdbf.json`: the same on DBF tables
    /// (`tools/ace/sql/sptdbf.txt`), then compares the files
    /// (`fixtures/sql/sptdbf/`).
    table_procedures_match_the_original_engine_on_dbf_tables("sptdbf.json");
    /// Replays `fixtures/sql/sptdd.json`: the same on the tables of a data
    /// dictionary (`tools/ace/sql/sptdd.txt`), then compares the dictionary
    /// and the files (`fixtures/sql/sptdd/`).
    table_procedures_match_the_original_engine_in_a_dictionary("sptdd.json");
    /// Replays `fixtures/sql/spm1.json`: `sp_SetStatementLimit`,
    /// `sp_SetRequestPriority`, `sp_CancelQuery` and
    /// `sp_IgnoreTableTransactions` on free tables (`tools/ace/sql/spm1.txt`),
    /// then compares the tables (`fixtures/sql/spm1/`).
    connection_procedures_match_the_original_engine("spm1.json");
    /// Replays `fixtures/sql/spm1dd.json`: `sp_IgnoreTableTransactions` on the
    /// tables of a dictionary (`tools/ace/sql/spm1dd.txt`), then compares the
    /// dictionary and the tables (`fixtures/sql/spm1dd/`).
    ignoring_transactions_in_a_dictionary_matches_the_original_engine("spm1dd.json");
    /// Replays `fixtures/sql/spm.json`: the management procedures
    /// (`sp_mgGet…`) on a free connection (`tools/ace/sql/spm.txt`). What they
    /// list of the server depends on the machine and on what else the process
    /// has open: only the columns of those are compared.
    management_procedures_match_the_original_engine("spm.json");
    /// Replays `fixtures/sql/ddopts.json`: the options of `CREATE INDEX`,
    /// `CREATE TABLE` and `ALTER TABLE`, `DROP TABLE ... FROM DATABASE` and
    /// the VFP column options on a free connection
    /// (`tools/ace/sql/ddopts.txt`), then compares the tables and index
    /// files with the original engine's (`fixtures/sql/ddopts/`).
    ddl_options_match_the_original_engine("ddopts.json");
    /// Replays `fixtures/sql/dbfpk.json`: `PRIMARY KEY`, field constraints
    /// and NULL columns in `CREATE TABLE` of DBF tables on a free connection
    /// (`tools/ace/sql/dbfpk.txt`), then compares the tables and index files
    /// with the original engine's (`fixtures/sql/dbfpk/`).
    dbf_primary_keys_and_constraints_match_the_original_engine("dbfpk.json");
    /// Replays `fixtures/sql/merge.json`: `MERGE` on ADT, Clipper, FoxPro and
    /// Visual FoxPro tables (`tools/ace/sql/merge.txt`), then compares the
    /// tables and index files with the original engine's
    /// (`fixtures/sql/merge/`).
    merge_matches_the_original_engine("merge.json");
    /// Replays `fixtures/sql/updfrom.json`: `UPDATE ... FROM` with joins and
    /// `UPDATE ... WITH DELETE | WITH RECALL` on ADT, Clipper, FoxPro and
    /// Visual FoxPro tables (`tools/ace/sql/updfrom.txt`), then compares the
    /// tables and index files with the original engine's
    /// (`fixtures/sql/updfrom/`).
    update_from_matches_the_original_engine("updfrom.json");
    /// Replays `fixtures/sql/ddbf.json`: `CREATE TABLE` of DBF tables on a
    /// dictionary connection (`tools/ace/sql/ddbf.txt`), which join the
    /// dictionary, with what their type allows of primary keys and field
    /// constraints, then compares the dictionary and the files with the
    /// original engine's (`fixtures/sql/ddbf/`).
    dictionary_dbf_tables_match_the_original_engine("ddbf.json", []);
    /// Replays `fixtures/sql/ddcreatedbf.json`: `CREATE TABLE` of DBF tables,
    /// in the database or free, by a user with no right to create tables, one
    /// with it and a member of `DB:Admin` (`tools/ace/sql/ddcreatedbf.txt`),
    /// then compares the dictionary and the files with the original engine's
    /// (`fixtures/sql/ddcreatedbf/`).
    dbf_tables_of_users_match_the_original_engine("ddcreatedbf.json", []);
    /// Replays `fixtures/sql/ddoptsdict.json`: the same on a dictionary
    /// connection (`tools/ace/sql/ddoptsdict.txt`), then compares the
    /// dictionary and the files with the original engine's
    /// (`fixtures/sql/ddoptsdict/`).
    dictionary_ddl_options_match_the_original_engine("ddoptsdict.json", []);
    /// Replays `fixtures/sql/ddalter.json`: `ALTER TABLE` of a dictionary's
    /// tables that adds, changes and drops columns and their constraints
    /// (`tools/ace/sql/ddalter.txt`), then compares the dictionary and the
    /// files with the original engine's (`fixtures/sql/ddalter/`).
    dictionary_alter_table_matches_the_original_engine("ddalter.json", []);
    /// Replays `fixtures/sql/ddalterfree.json`: `ALTER TABLE` of free tables
    /// with an order the new types cannot build, and `MEMOBLOCKSIZE`
    /// (`tools/ace/sql/ddalterfree.txt`), then compares the files with the
    /// original engine's (`fixtures/sql/ddalterfree/`).
    free_alter_table_matches_the_original_engine("ddalterfree.json");
    /// Replays `fixtures/sql/ddalterdbf.json`: `ALTER TABLE` of a dictionary's
    /// Clipper, FoxPro, Visual FoxPro and ADT tables that changes fields, with
    /// the Key objects of their orders (`tools/ace/sql/ddalterdbf.txt`), then
    /// compares the dictionary and the files with the original engine's
    /// (`fixtures/sql/ddalterdbf/`).
    dictionary_alter_table_of_dbf_matches_the_original_engine("ddalterdbf.json", []);
    /// Replays `fixtures/sql/ddalterfail.json`: `ALTER TABLE` of a
    /// dictionary's tables whose first order the new types cannot build
    /// (`tools/ace/sql/ddalterfail.txt`), then compares the files they leave
    /// with the original engine's (`fixtures/sql/ddalterfail/`).
    dictionary_alter_table_that_fails_matches_the_original_engine(
        "ddalterfail.json",
        []
    );
    /// Replays `fixtures/sql/spddalterfail.json`: the same in shapes of
    /// fields and orders, with records, where the slots the objects go back to
    /// and the orders built before the one that failed show
    /// (`tools/ace/sql/spddalterfail.txt`).
    dictionary_alter_table_that_fails_in_shapes_matches_the_original_engine(
        "spddalterfail.json",
        []
    );
    /// Replays `fixtures/sql/spddalterfail2.json`: the same for tables with
    /// more orders than fields, in one index file
    /// (`tools/ace/sql/spddalterfail2.txt`).
    dictionary_alter_table_that_fails_with_more_orders_than_fields_matches_the_original_engine(
        "spddalterfail2.json",
        []
    );
    /// Replays `fixtures/sql/spddalterfail3.json`: the same for tables whose
    /// orders are in several index files
    /// (`tools/ace/sql/spddalterfail3.txt`).
    dictionary_alter_table_that_fails_in_several_index_files_matches_the_original_engine(
        "spddalterfail3.json",
        ["de.ai"]
    );
    /// Replays `fixtures/sql/mergedd.json`: `MERGE` on a dictionary
    /// connection, with defaults, constraints, a primary key and triggers
    /// (`tools/ace/sql/mergedd.txt`), then compares the dictionary and the
    /// files with the original engine's (`fixtures/sql/mergedd/`).
    ///
    /// The dictionary's trigger slots of a table with several triggers and
    /// the deleted record an INSTEAD OF insert leaves in the original are not
    /// tflip's (KNOWN_ISSUES.md).
    merge_on_a_dictionary_matches_the_original_engine("mergedd.json", ["ct.add", "inst.adt"]);
    }

    /// Replays `fixtures/sql/collation_compare.json`: WHERE, ORDER BY,
    /// DISTINCT, GROUP BY, MIN/MAX and UNION on text of the charset_german
    /// `ca` table, and of a CICHAR copy of it, under `ANSI_CHAR_SET=German`
    /// (`tools/ace/sql/collation_compare.txt`). Needs the original engine's
    /// collation files (see [`crate::config::collation_dir`]), skipped
    /// without them.
    #[test]
    fn collation_comparisons_match_the_original_engine() {
        if crate::config::collation_dir().is_none_or(|dir| !dir.join("ansi.chr").exists()) {
            return;
        }
        replay("collation_compare.json");
    }

    #[test]
    fn property_procedures_match_the_original_engine() {
        replay_compared("spddgprops.json", &[]);
        replay_compared("spdduprops.json", &[]);
        replay_compared("spddvprops.json", &[]);
        replay_compared("spddpprops.json", &[]);
        replay_compared("spddiprops.json", &[]);
        replay_compared("spddtgprops.json", &[]);
        replay_compared("spdddprops.json", &[]);
        replay_compared("spddtprops.json", &[]);
    }

    #[test]
    fn move_procedure_matches_the_original_engine() {
        replay_compared("spddmove.json", &[]);
        compare_created("spddmove/sub", "de/sub", &[]);
        compare_created("spddmove/sub2", "de/sub2", &[]);
    }

    /// Replays `fixtures/sql/selinto.json`: `SELECT … INTO table` on a free
    /// and on a dictionary connection (`tools/ace/sql/selinto.txt`), then
    /// compares the tables it made, and the dictionary, with the original
    /// engine's (`fixtures/sql/selinto/`).
    #[test]
    fn select_into_matches_the_original_engine() {
        replay_compared("selinto.json", &[]);
        compare_created("selinto/dd", "dd", &[]);
    }

    /// Replays `fixtures/sql/setcollation.json`: `SET ::conn.Collation` and
    /// `SET ::stmt.Collation` (`tools/ace/sql/setcollation.txt`). Needs the
    /// original engine's collation files, skipped without them.
    #[test]
    fn setting_collations_matches_the_original_engine() {
        if crate::config::collation_dir().is_none_or(|dir| !dir.join("adscollate.adt").exists()) {
            return;
        }
        replay("setcollation.json");
    }

    /// Replays `fixtures/sql/spcat.json`: the catalog procedures the ODBC and
    /// JDBC drivers call (`sp_GetCatalogs`, `sp_GetTypeInfo`,
    /// `sp_SupportsConvert`, `sp_RetrieveSystemProcProperty`, ...) and
    /// `system.systemprocedures`, on a free connection
    /// (`tools/ace/sql/spcat.txt`).
    #[test]
    fn catalog_procedures_match_the_original_engine() {
        replay("spcat.json");
    }

    /// Replays `fixtures/sql/spddcat.json`: the same on a dictionary
    /// connection (`tools/ace/sql/spddcat.txt`), the stored procedures'
    /// parameters among them, and what a user who is not the administrator
    /// may run; then compares the dictionary with the original engine's
    /// (`fixtures/sql/spddcat/`).
    #[test]
    fn catalog_procedures_match_the_original_engine_in_a_dictionary() {
        replay_compared("spddcat.json", &[]);
    }

    /// Replays `fixtures/sql/spql.json` and `spddql.json`: query logging
    /// (`tools/ace/sql/spql*.txt`, `spddql*.txt`), what the log table holds
    /// of each statement and what a user who is not the administrator may
    /// do, with the times masked.
    #[test]
    fn query_logging_matches_the_original_engine() {
        replay("spql.json");
        replay("spql2.json");
        replay("spql3.json");
        replay_compared("spql6.json", &[]);
        replay("spddql.json");
        replay("spddql2.json");
        replay("spddql3.json");
        replay("spddql4.json");
    }

    /// Replays `fixtures/sql/spmg.json` and `spddmg.json`: the cache, memory
    /// and kill-user management procedures and `sp_GetUnicodeConverters`
    /// (`tools/ace/sql/spmg.txt`, `spddmg.txt`), and what a user who is not
    /// the administrator may run.
    #[test]
    fn cache_memory_and_converter_procedures_match_the_original_engine() {
        replay("spmg.json");
        replay("spddmg.json");
    }

    /// Replays `fixtures/sql/spcoll.json` and `spcoll2.json`:
    /// `sp_GetCollations` and `sp_GetCollationTable`
    /// (`tools/ace/sql/spcoll.txt`, `spcoll2.txt`). Needs the original
    /// engine's collation files, skipped without them.
    #[test]
    fn collation_procedures_match_the_original_engine() {
        if crate::config::collation_dir().is_none_or(|dir| !dir.join("adscollate.adt").exists()) {
            return;
        }
        replay("spcoll.json");
        replay("spcoll2.json");
    }

    /// Compares the files the replay left in `folder` of its data folder with
    /// the original engine's in `fixtures/sql/<golden>`, the dictionary's
    /// login secret and the tables' date of the last update aside (the
    /// first is salted anew on every write, the second is today's); the
    /// contents of the files `unchecked` are not compared, only that they
    /// are there.
    fn compare_created(golden: &str, folder: &str, unchecked: &[&str]) {
        let golden = fixtures::dir(&format!("../sql/{golden}"));
        let ours = CREATED_DATA.with_borrow(Clone::clone).unwrap().join(folder);
        // The data folder of a free connection holds other files too.
        let names = |dir: &std::path::Path| {
            let mut names: Vec<String> = std::fs::read_dir(dir)
                .unwrap()
                .filter(|e| e.as_ref().unwrap().file_type().unwrap().is_file())
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|name| {
                    !folder.is_empty()
                        || [
                            ".dbf", ".cdx", ".fpt", ".ntx", ".dbt", ".adt", ".adi", ".adm", ".BAK",
                        ]
                        .iter()
                        .any(|e| name.ends_with(e))
                })
                .collect();
            names.sort();
            names
        };
        assert_eq!(names(&ours), names(&golden), "the files");
        let mut differ = Vec::new();
        for name in names(&golden)
            .into_iter()
            .filter(|name| !unchecked.contains(&name.as_str()))
        {
            let mut want = std::fs::read(golden.join(&name)).unwrap();
            let mut got = std::fs::read(ours.join(&name)).unwrap();
            if name.ends_with(".add") {
                for bytes in [&mut want, &mut got] {
                    let data = u32::from_le_bytes(bytes[0x20..0x24].try_into().unwrap()) as usize;
                    let length = u32::from_le_bytes(bytes[0x24..0x28].try_into().unwrap()) as usize;
                    bytes[data + 223..data + 498].fill(0);
                    // A user's password, the create rights of a user or group,
                    // the rights of a permission and a link's password are
                    // enciphered with a key from the dictionary's secret.
                    for record in (data..bytes.len()).step_by(length) {
                        let kind = bytes[record + 13..record + 23].to_vec();
                        if kind.starts_with(b"User ")
                            || kind.starts_with(b"Group")
                            || kind.starts_with(b"Permission")
                        {
                            bytes[record + 223..record + 498].fill(0);
                        }
                        // The rights granted to DB:Public are kept in a slot of
                        // the object itself, enciphered with a random word the
                        // engine draws on every write (what they hold is
                        // compared through `system.permissions`).
                        let public_slot = [
                            (&b"Table"[..], 12),
                            (b"View", 5),
                            (b"StoredProc", 6),
                            (b"Function", 0),
                            (b"Package", 2),
                            (b"LinkedDD", 2),
                            (b"Publicatn", 1),
                            (b"Subscriptn", 7),
                        ]
                        .iter()
                        .find(|(name, _)| kind.starts_with(name))
                        .map(|&(_, slot)| slot);
                        if let Some(public_slot) = public_slot {
                            // The slots of a long procedure or view run past
                            // the record's 275 bytes; the walk stops there.
                            let end = record + 498;
                            let mut at = record + 223;
                            for slot in 0..=public_slot {
                                if at + 2 > end {
                                    break;
                                }
                                let length =
                                    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
                                at += 2;
                                if length == 0xFFFF {
                                    continue;
                                }
                                if at + usize::from(length) > end {
                                    break;
                                }
                                if slot == public_slot && length == 32 {
                                    bytes[at..at + 32].fill(0);
                                }
                                at += usize::from(length);
                            }
                        }
                        if kind.starts_with(b"Subscriptn") {
                            // The target password is salted anew on every
                            // write: the third slot, after the target and the
                            // user.
                            let mut at = record + 223;
                            for slot in 0..3 {
                                let length =
                                    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
                                at += 2;
                                if length == 0xFFFF {
                                    continue;
                                }
                                if slot == 2 {
                                    bytes[at..at + usize::from(length)].fill(0);
                                }
                                at += usize::from(length);
                            }
                        }
                        if kind.starts_with(b"LinkedDD") {
                            // The password is the second slot, a length and its
                            // bytes: its length shows that it is there. A path
                            // that is a full one is the folder's of the run.
                            let mut at = record + 223;
                            let mut password = None;
                            let mut full_path = false;
                            for slot in 0..5 {
                                let length =
                                    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
                                at += 2;
                                if length == 0xFFFF {
                                    continue;
                                }
                                if slot == 1 {
                                    password = Some(at..at + usize::from(length));
                                }
                                if slot == 4 {
                                    full_path = matches!(bytes[at], b'Z' | b'/');
                                }
                                at += usize::from(length);
                            }
                            if full_path {
                                bytes[record + 223..record + 498].fill(0);
                            } else if let Some(password) = password {
                                bytes[password].fill(0);
                            }
                        }
                    }
                }
            }
            if name.ends_with(".dbf") || name.ends_with(".dbf.BAK") {
                want[1..4].fill(0);
                got[1..4].fill(0);
            }
            if got != want {
                let at = got.iter().zip(&want).position(|(g, w)| g != w);
                differ.push(format!(
                    "{name}: {} vs {} bytes, first at {at:?}",
                    got.len(),
                    want.len()
                ));
            }
        }
        assert!(differ.is_empty(), "{}", differ.join("\n"));
    }

    /// Replays `fixtures/sql/collation_dynamic.json`: WHERE, ORDER BY,
    /// DISTINCT, GROUP BY, MIN/MAX and UNION on text of tables a statement
    /// opens with a dynamic collation (`AdsStmtSetTableCollation`), and the
    /// 5209 of a table whose orders have another
    /// (`tools/ace/sql/collation_dynamic.txt`). Needs the original engine's
    /// collation files, skipped without them.
    #[test]
    fn dynamic_collation_comparisons_match_the_original_engine() {
        if crate::config::collation_dir().is_none_or(|dir| !dir.join("adscollate.adt").exists()) {
            return;
        }
        replay("collation_dynamic.json");
    }

    /// Replays `fixtures/sql/oem.json`: text of SQL statements (literals and
    /// parameters) against an OEM table (`tools/ace/sql/oem.txt`).
    #[test]
    fn text_in_sql_meets_an_oem_table_as_in_the_original_engine() {
        replay("oem.json");
    }

    /// Replays `fixtures/sql/oemrows.json`: text read out of an OEM table
    /// into a script's variables and rows, subqueries and derived tables
    /// (`tools/ace/sql/oemrows.txt`).
    #[test]
    fn text_read_out_of_an_oem_table_is_ansi_as_in_the_original_engine() {
        replay("oemrows.json");
    }

    /// Replays `fixtures/sql/likeorder.json`: the order of the rows of a
    /// `LIKE`, alone and beside other conditions (`tools/ace/sql/likeorder.txt`).
    #[test]
    fn the_rows_of_a_like_come_in_the_original_engines_order() {
        replay("likeorder.json");
    }

    /// Replays `fixtures/sql/indexsel.json`: the orders a planned `SELECT`
    /// looks its rows up in, by the rows' order and the plan
    /// (`tools/ace/sql/indexsel.txt`).
    #[test]
    fn selects_use_orders_as_in_the_original_engine() {
        replay("indexsel.json");
    }

    /// Replays `fixtures/sql/indexsel2.json`: what a lone `>=` or `<=` sorts
    /// the rows by, with or without an order, CICHARACTER text and text under
    /// a dynamic collation (`tools/ace/sql/indexsel2.txt`). Needs the original
    /// engine's collation files, skipped without them.
    #[test]
    fn lone_ranges_order_rows_as_in_the_original_engine() {
        if crate::config::collation_dir().is_none_or(|dir| !dir.join("adscollate.adt").exists()) {
            return;
        }
        replay("indexsel2.json");
    }

    /// Replays `fixtures/sql/indexsel3.json`: the rows and plans of ranges on
    /// text of orders under a dynamic collation (`tools/ace/sql/indexsel3.txt`).
    /// Needs the original engine's collation files, skipped without them.
    #[test]
    fn ranges_of_collated_text_read_as_in_the_original_engine() {
        if crate::config::collation_dir().is_none_or(|dir| !dir.join("adscollate.adt").exists()) {
            return;
        }
        replay("indexsel3.json");
    }

    /// Replays `fixtures/sql/joinorder.json` and `joinsize.json`: which table of
    /// an inner join is read first (`tools/ace/sql/joinorder.txt`, `joinsize.txt`).
    #[test]
    fn inner_joins_read_the_smaller_table_first_as_in_the_original_engine() {
        replay("joinorder.json");
        replay("joinsize.json");
    }

    /// Replays `fixtures/sql/statics.json`: which `WHERE` and `ORDER BY` of
    /// scalar functions make a cursor static, by the table it is of and by
    /// whether it takes a write (`tools/ace/sql/statics.txt`).
    #[test]
    fn the_functions_that_make_a_cursor_static_are_the_original_engines() {
        replay("statics.json");
    }

    /// Replays `fixtures/sql/derived.json`: which `SELECT`s of one table in
    /// `FROM` are live cursors on it, their plans and counts, and a derived
    /// table's `WHERE` as a filter (`tools/ace/sql/derived.txt`).
    #[test]
    fn a_select_of_one_table_in_from_is_a_live_cursor_as_in_the_original_engine() {
        replay("derived.json");
    }

    /// Replays `fixtures/sql/likedict.json`: the order of the rows of a
    /// `LIKE` on a dictionary connection, in a table of the dictionary and in
    /// its `system.` tables (`tools/ace/sql/likedict.txt`).
    #[test]
    fn the_rows_of_a_like_on_a_dictionary_come_in_the_original_engines_order() {
        replay("likedict.json");
    }

    /// How a fixture's data folder is made.
    enum Source {
        /// [`fixtures::sql_data`], the tables the probes ran on, as they are.
        Shared,
        /// [`fixtures::sql_dml_data`], for the script that changes them.
        Dml,
        /// A folder of the replay's own, empty.
        Empty,
        /// A copy of the fixture folder (as [`fixtures::dir`] names it).
        Copy(&'static str),
        /// A copy of [`fixtures::sql_data`].
        CopyShared,
        /// The fixture files, side by side.
        Files(&'static [&'static str]),
        /// Copies of fixture folders, each in a subfolder: `(folder,
        /// subfolder)`.
        Folders(&'static [(&'static str, &'static str)]),
    }

    /// What a probe's script runs on, and how its replay is compared.
    struct Fixture {
        /// The file in `fixtures/sql/`.
        file: &'static str,
        source: Source,
        /// The name of the folder of its own, for all but `Shared` and `Dml`.
        scratch: &'static str,
        /// Whether `connect` and `ddcreate` take their paths in that folder.
        connect_here: bool,
        /// Whether a link's path is written out in it (`AdsGetActiveLinkInfo`).
        links: bool,
        /// The folder of the data folder the files it made are compared in
        /// with the golden `fixtures/sql/<file without .json>`, `""` for the
        /// data folder itself (see [`compare_created`]).
        created: Option<&'static str>,
        /// Whether it only tells the commands where the files are, as the
        /// script's text names them, and there is no golden to compare.
        uncompared: bool,
        /// More folders to make in it.
        folders: &'static [&'static str],
        /// Whether the script ran under the German static collation
        /// (`run_collation_compare.sh`).
        german: bool,
        /// Whether the script's text is bytes of the table's code page
        /// (see [`script_bytes`]).
        byte_text: bool,
        /// Whether the script ran on the local server, whose transactions
        /// are only counted: what a failing statement wrote stays.
        counted: bool,
        /// Whether what follows a write through a cursor is compared too,
        /// as the script writes where the writes of tflip's cursors match.
        writes: bool,
        /// Adjusts the result of a command, tflip's and the original's, where
        /// they differ for a reason that is not what is tested.
        normalize: Option<fn(&str, &mut Json)>,
    }

    const BASE: Fixture = Fixture {
        file: "",
        source: Source::Shared,
        scratch: "",
        connect_here: false,
        links: false,
        created: None,
        uncompared: false,
        folders: &[],
        german: false,
        byte_text: false,
        counted: false,
        writes: false,
        normalize: None,
    };

    /// A statement on permissions that lost rows to a rights slot that does
    /// not check reports 0 records in the original, whatever it shows, and
    /// numbers the rows it shows by where they were in the grid (a lost row's
    /// number is skipped, a row paid for it has its original's).
    fn drop_count_of_permissions(cmd: &str, result: &mut Json) {
        let lower = cmd.to_ascii_lowercase();
        if (lower.contains("system.permissions") || lower.contains("system.effectivepermissions"))
            && let Some(cursor) = result.get_mut("cursor").and_then(Json::as_object_mut)
        {
            cursor.remove("count");
            if let Some(Json::Array(rows)) = cursor.get_mut("rows") {
                for row in rows {
                    if let Some(number) = row.get_mut(0) {
                        *number = Json::from(0);
                    }
                }
            }
        }
    }

    /// A connection's user number (`UserID` of a database trigger's
    /// `__info`) is the lowest the process has not given a connection open
    /// at the time, which other tests' connections change.
    fn mask_user_ids(_cmd: &str, result: &mut Json) {
        // What follows the number at the start of `text`, a cast to text of
        // ten characters (its padding goes with it).
        fn after_number(text: &str) -> &str {
            text.trim_start_matches(|c: char| c.is_ascii_digit())
                .trim_start()
        }

        let Some(Json::Array(rows)) = result
            .get_mut("cursor")
            .and_then(|cursor| cursor.get_mut("rows"))
        else {
            return;
        };
        for row in rows {
            let Some(Json::Array(columns)) = row.get_mut(1) else {
                continue;
            };
            for column in columns {
                let Some(text) = column.as_str() else {
                    continue;
                };
                let masked = match text.find("uid=") {
                    // `CONNECT uid=4 user=...`
                    Some(at) => {
                        let (head, tail) = text.split_at(at + 4);
                        format!("{head}<id> {}", after_number(tail))
                    }
                    // `B-OP t1 3   N 1 ...`: the name, then the number.
                    None => match text.splitn(3, ' ').collect::<Vec<_>>()[..] {
                        [event, table, rest]
                            if matches!(event.len(), 4) && event.starts_with(['A', 'B']) =>
                        {
                            format!("{event} {table} <id> {}", after_number(rest))
                        }
                        _ => continue,
                    },
                };
                *column = json!(masked);
            }
        }
    }

    /// The numbers `sp_GetCollationTable` gives the collations are the order
    /// the process loaded them in, which other tests change.
    fn mask_collation_numbers(_cmd: &str, result: &mut Json) {
        let Some(cursor) = result.get_mut("cursor") else {
            return;
        };
        let Some(at) = cursor["fields"]
            .as_array()
            .and_then(|fields| fields.iter().position(|f| f[0] == "CollationID"))
        else {
            return;
        };
        if let Some(Json::Array(rows)) = cursor.get_mut("rows") {
            for row in rows {
                row[1][at] = json!("<number>");
            }
        }
    }

    /// Whether `cmd` shows a plan, `SHOW ALL INTERNAL PLAN` included.
    fn shows_plan(cmd: &str) -> bool {
        cmd.contains("SHOW PLAN")
            || cmd.contains("show plan")
            || cmd.contains("SHOW ALL INTERNAL PLAN")
    }

    /// The original engine numbers the statements that made the views and
    /// tables too, in the plans it shows.
    fn number_made_statements(cmd: &str, result: &mut Json) {
        if !shows_plan(cmd) {
            return;
        }
        if let Some(Json::Array(rows)) = result
            .get_mut("cursor")
            .and_then(|cursor| cursor.get_mut("rows"))
        {
            for row in rows {
                row[1][4] = json!("0");
            }
        }
    }

    /// What the claims cannot give twice: the plans' statement numbers (the
    /// original counts the statements a script runs inside its loops, as
    /// tflip does not), the connection's name, which has the data folder in
    /// it, and the clock.
    fn claims_volatile(cmd: &str, result: &mut Json) {
        number_made_statements(cmd, result);
        if !(cmd.contains("::conn.Name") || cmd.contains("CURRENT_TIME(2)")) {
            return;
        }
        if let Some(Json::Array(rows)) = result
            .get_mut("cursor")
            .and_then(|cursor| cursor.get_mut("rows"))
        {
            for row in rows {
                for value in row[1].as_array_mut().into_iter().flatten() {
                    *value = json!("<volatile>");
                }
            }
        }
    }

    /// The commands of a claims script whose answers differ from the
    /// original's, listed in `fixtures/sql/<file>_known.txt` (see
    /// `docs/KNOWN_ISSUES.md`): one per line, the case's name in a comment
    /// above. An `exec` is known where the `prepare` of its statement is.
    fn claims_known(file: &str) -> std::collections::HashSet<String> {
        let name = file.replace(".json", "_known.txt");
        let text =
            std::fs::read_to_string(fixtures::dir(&format!("../sql/{name}"))).unwrap_or_default();
        text.split('\n')
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// The plans of the index scripts are compared by their steps: each one's
    /// operator, table and order, not the AOF's text, details and counts, the
    /// `EVALUATE` of a join's condition or the numbers (see KNOWN_ISSUES.md).
    fn index_plan_steps(cmd: &str, result: &mut Json) {
        if !(cmd.contains("SHOW PLAN") || cmd.contains("show plan")) {
            return;
        }
        let Some(cursor) = result.get_mut("cursor").and_then(Json::as_object_mut) else {
            return;
        };
        let Some(Json::Array(rows)) = cursor.get("rows").cloned() else {
            return;
        };
        let steps: Vec<Json> = rows
            .iter()
            .filter_map(|row| {
                let values = row[1].as_array()?;
                let operator = values[5].as_str()?.trim();
                if operator == "EVALUATE" {
                    return None;
                }
                let lines: Vec<&str> = values[3]
                    .as_str()
                    .unwrap_or_default()
                    .lines()
                    .filter(|line| {
                        ["Table Name", "Table Alias", "Index Tag", "Tag :"]
                            .iter()
                            .any(|key| line.starts_with(key))
                    })
                    .collect();
                Some(json!([operator, lines]))
            })
            .collect();
        cursor.insert("count".to_owned(), json!(steps.len()));
        let rows: Vec<Json> = steps
            .into_iter()
            .enumerate()
            .map(|(i, step)| json!([i + 1, step]))
            .collect();
        cursor.insert("rows".to_owned(), Json::Array(rows));
    }

    /// The clock's own reading is not the test: the type and the length of
    /// `CURRENT_TIMESTAMP(n)` are.
    fn mask_clock(cmd: &str, result: &mut Json) {
        if !cmd.contains("CURRENT_TIME") || cmd.contains("LENGTH(") || cmd.contains(" = ") {
            return;
        }
        if let Some(Json::Array(rows)) = result
            .get_mut("cursor")
            .and_then(|cursor| cursor.get_mut("rows"))
        {
            for row in rows {
                row[1][0] = json!("<now>");
            }
        }
    }

    /// A plan's statement number counts the statements the connection ran
    /// before, which the original counts as it does DDL and `INSERT`.
    fn mask_plan_numbers(cmd: &str, result: &mut Json) {
        if !cmd.contains("SHOW PLAN") {
            return;
        }
        if let Some(Json::Array(rows)) = result
            .get_mut("cursor")
            .and_then(|cursor| cursor.get_mut("rows"))
        {
            for row in rows {
                row[1][4] = json!("<n>");
            }
        }
    }

    /// The query log's clock and who logs: the times a statement started and
    /// ended and how long it ran are the run's own, so only whether they
    /// are set is compared; the connection's name is the host's user.
    fn mask_query_log(cmd: &str, result: &mut Json) {
        mask_server_lists(cmd, result);
        let Some(cursor) = result.get_mut("cursor") else {
            return;
        };
        let names: Vec<String> = cursor["fields"]
            .as_array()
            .map(|fields| {
                fields
                    .iter()
                    .map(|f| f[0].as_str().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        let Some(Json::Array(rows)) = cursor.get_mut("rows") else {
            return;
        };
        for row in rows {
            for (at, name) in names.iter().enumerate() {
                let masked = match name.as_str() {
                    "Start Time" | "End Time" => "<time>",
                    "Run Time" => "<ms>",
                    "Connection Name" => "<user>",
                    _ => continue,
                };
                if !row[1][at].is_null() {
                    row[1][at] = json!(masked);
                }
            }
        }
    }

    /// The record count of a cursor on a table whose record a trigger took back
    /// is the original's own (a number, 0 or -1 as the cursor was made), not
    /// the test: the rows are.
    fn without_counts(cmd: &str, result: &mut Json) {
        if cmd.starts_with("query")
            && let Some(cursor) = result.get_mut("cursor").and_then(Json::as_object_mut)
        {
            cursor.remove("count");
        }
    }

    /// What the management procedures list of the server: the users, tables
    /// and indexes open, what is in use and how long the server has run
    /// depend on the machine and on what else the process has open (and on
    /// the program's own state, which tflip's differs in: see
    /// KNOWN_ISSUES.md); the Windows version is the host's.
    fn mask_server_lists(cmd: &str, result: &mut Json) {
        let Some(cursor) = result.get_mut("cursor").and_then(Json::as_object_mut) else {
            return;
        };
        if [
            "sp_mgGetActivityInfo",
            "sp_mgGetUsageInfo",
            "sp_mgGetAllTables",
            "sp_mgGetAllIndexes",
            "sp_mgGetConnectedUsers",
            "sp_mgGetTableIndexes",
            "sp_mgGetTableUsers",
            "sp_mgGetCacheInfo",
            "sp_mgGetTableCacheInfo",
        ]
        .iter()
        .any(|name| cmd.contains(name))
        {
            cursor.remove("rows");
            cursor.remove("count");
        }
        if cmd.contains("sp_mgGetServerType()")
            && !cmd.contains("SELECT")
            && let Some(Json::Array(rows)) = cursor.get_mut("rows")
        {
            for row in rows {
                row[1][2] = json!("<os>");
                row[1][3] = json!("<os>");
            }
        }
    }

    /// The scripts that change tables run on folders of their own, the ones
    /// that make their own tables on empty ones; any other runs on
    /// [`BASE`].
    const SPDD_FOLDERS: &[&str] = &["de/sub", "de/sub2", "de/sub/deep"];

    static FIXTURES: &[Fixture] = &[
        Fixture {
            file: "dml.json",
            source: Source::Dml,
            ..BASE
        },
        Fixture {
            file: "dbf.json",
            source: Source::Empty,
            scratch: "sql-dbf",
            ..BASE
        },
        Fixture {
            file: "groupconcat.json",
            source: Source::Empty,
            scratch: "sql-groupconcat",
            ..BASE
        },
        Fixture {
            file: "funcs.json",
            source: Source::Empty,
            scratch: "sql-funcs",
            ..BASE
        },
        Fixture {
            file: "guid.json",
            source: Source::Empty,
            scratch: "sql-guid",
            ..BASE
        },
        Fixture {
            file: "iif.json",
            source: Source::Empty,
            scratch: "sql-iif",
            ..BASE
        },
        Fixture {
            file: "gram_shift.json",
            source: Source::Empty,
            scratch: "sql-gram-shift",
            ..BASE
        },
        Fixture {
            file: "gram_bin.json",
            source: Source::Empty,
            scratch: "sql-gram-bin",
            ..BASE
        },
        Fixture {
            file: "gram_cast.json",
            source: Source::Empty,
            scratch: "sql-gram-cast",
            ..BASE
        },
        Fixture {
            file: "gram_cast2.json",
            source: Source::Empty,
            scratch: "sql-gram-cast2",
            ..BASE
        },
        Fixture {
            file: "gram_cast3.json",
            source: Source::Empty,
            scratch: "sql-gram-cast3",
            ..BASE
        },
        Fixture {
            file: "gram_syntax.json",
            source: Source::Empty,
            scratch: "sql-gram-syntax",
            ..BASE
        },
        Fixture {
            file: "claims.json",
            source: Source::Empty,
            scratch: "sql-claims",
            normalize: Some(claims_volatile),
            ..BASE
        },
        Fixture {
            file: "claims_huge.json",
            source: Source::Empty,
            scratch: "sql-claims-huge",
            normalize: Some(claims_volatile),
            ..BASE
        },
        Fixture {
            file: "gram_script.json",
            source: Source::Empty,
            scratch: "sql-gram-script",
            connect_here: true,
            folders: &["gs"],
            ..BASE
        },
        Fixture {
            file: "gram_oj.json",
            source: Source::Empty,
            scratch: "sql-gram-oj",
            ..BASE
        },
        Fixture {
            file: "gram_cross.json",
            source: Source::Empty,
            scratch: "sql-gram-cross",
            ..BASE
        },
        Fixture {
            file: "gram_charlen.json",
            source: Source::Empty,
            scratch: "sql-gram-charlen",
            ..BASE
        },
        Fixture {
            file: "gram_sysvar.json",
            source: Source::Empty,
            scratch: "sql-gram-sysvar",
            ..BASE
        },
        Fixture {
            file: "wherefilter.json",
            source: Source::Empty,
            scratch: "sql-wherefilter",
            ..BASE
        },
        Fixture {
            file: "wherefilter3.json",
            source: Source::Empty,
            scratch: "sql-wherefilter3",
            ..BASE
        },
        Fixture {
            file: "wherefilter4.json",
            source: Source::Empty,
            scratch: "sql-wherefilter4",
            ..BASE
        },
        Fixture {
            file: "wherefilter5.json",
            source: Source::Empty,
            scratch: "sql-wherefilter5",
            ..BASE
        },
        Fixture {
            file: "castchar2.json",
            source: Source::Empty,
            scratch: "sql-castchar2",
            ..BASE
        },
        Fixture {
            file: "tsarith.json",
            source: Source::Empty,
            scratch: "sql-tsarith",
            ..BASE
        },
        Fixture {
            file: "tsarith2.json",
            source: Source::Empty,
            scratch: "sql-tsarith2",
            ..BASE
        },
        Fixture {
            file: "staticorder.json",
            source: Source::Empty,
            scratch: "sql-staticorder",
            ..BASE
        },
        Fixture {
            file: "wherefilter6.json",
            source: Source::Empty,
            scratch: "sql-wherefilter6",
            ..BASE
        },
        Fixture {
            file: "modulo.json",
            source: Source::Empty,
            scratch: "sql-modulo",
            ..BASE
        },
        Fixture {
            file: "percent.json",
            source: Source::Empty,
            scratch: "sql-percent",
            ..BASE
        },
        Fixture {
            file: "scripts.json",
            source: Source::Empty,
            scratch: "sql-scripts",
            ..BASE
        },
        Fixture {
            file: "temptables.json",
            source: Source::Empty,
            scratch: "sql-temptables",
            ..BASE
        },
        Fixture {
            file: "literals.json",
            source: Source::Empty,
            scratch: "sql-literals",
            connect_here: true,
            normalize: Some(mask_clock),
            ..BASE
        },
        Fixture {
            file: "temptables_types.json",
            source: Source::Empty,
            scratch: "sql-temptables-types",
            ..BASE
        },
        Fixture {
            file: "temptables_api.json",
            source: Source::Empty,
            scratch: "sql-temptables-api",
            ..BASE
        },
        Fixture {
            file: "temptables_into.json",
            source: Source::Empty,
            scratch: "sql-temptables-into",
            ..BASE
        },
        Fixture {
            file: "temptables_into2.json",
            source: Source::Empty,
            scratch: "sql-temptables-into2",
            ..BASE
        },
        Fixture {
            file: "temptables_into3.json",
            source: Source::Empty,
            scratch: "sql-temptables-into3",
            ..BASE
        },
        Fixture {
            file: "temptables_into4.json",
            source: Source::Empty,
            scratch: "sql-temptables-into4",
            ..BASE
        },
        Fixture {
            file: "temptables_api2.json",
            source: Source::Empty,
            scratch: "sql-temptables-api2",
            ..BASE
        },
        Fixture {
            file: "temptables_api3.json",
            source: Source::Empty,
            scratch: "sql-temptables-api3",
            ..BASE
        },
        Fixture {
            file: "rowfuncs.json",
            source: Source::Empty,
            scratch: "sql-rowfuncs",
            ..BASE
        },
        Fixture {
            file: "dbfdict.json",
            source: Source::Folders(&[("../dictionary/dbftables", "dbftables")]),
            scratch: "sql-dbfdict",
            connect_here: true,
            ..BASE
        },
        // Empty but for copies of the props dictionary in `props/` and of
        // the dict one in `dict/`, with all their files (as
        // `tools/ace/sql/run.sh`), which the script changes.
        Fixture {
            file: "links.json",
            source: Source::Folders(&[("dictionary_props", "props"), ("dictionary", "dict")]),
            scratch: "sql-links",
            connect_here: true,
            links: true,
            ..BASE
        },
        Fixture {
            file: "linklist.json",
            source: Source::Folders(&[("dictionary_props", "props"), ("dictionary", "dict")]),
            scratch: "sql-linklist",
            connect_here: true,
            links: true,
            ..BASE
        },
        Fixture {
            file: "dbfpk.json",
            source: Source::Empty,
            scratch: "sql-dbfpk",
            created: Some(""),
            ..BASE
        },
        Fixture {
            file: "merge.json",
            source: Source::Empty,
            scratch: "sql-merge",
            created: Some(""),
            normalize: Some(number_made_statements),
            ..BASE
        },
        Fixture {
            file: "planupd.json",
            source: Source::Empty,
            scratch: "sql-planupd",
            normalize: Some(number_made_statements),
            ..BASE
        },
        Fixture {
            file: "updfrom.json",
            source: Source::Empty,
            scratch: "sql-updfrom",
            created: Some(""),
            ..BASE
        },
        // The free tables are made in an empty folder too, and compared
        // with the original's.
        Fixture {
            file: "ddopts.json",
            source: Source::Empty,
            scratch: "sql-ddopts",
            created: Some(""),
            ..BASE
        },
        Fixture {
            file: "ddtables.json",
            source: Source::Empty,
            scratch: "sql-ddtables",
            connect_here: true,
            created: Some("ct"),
            ..BASE
        },
        Fixture {
            file: "ddbf.json",
            source: Source::Empty,
            scratch: "sql-ddbf",
            connect_here: true,
            created: Some("cb"),
            ..BASE
        },
        Fixture {
            file: "selinto.json",
            source: Source::Empty,
            scratch: "sql-selinto",
            connect_here: true,
            created: Some(""),
            folders: &["dd"],
            ..BASE
        },
        Fixture {
            file: "ddoptsdict.json",
            source: Source::Empty,
            scratch: "sql-ddoptsdict",
            connect_here: true,
            created: Some("ct"),
            ..BASE
        },
        Fixture {
            file: "ddalter.json",
            source: Source::Empty,
            scratch: "sql-ddalter",
            connect_here: true,
            created: Some("ct"),
            ..BASE
        },
        Fixture {
            file: "ddalterfree.json",
            source: Source::Empty,
            scratch: "sql-ddalterfree",
            created: Some(""),
            ..BASE
        },
        Fixture {
            file: "ddalterfail.json",
            source: Source::Empty,
            scratch: "sql-ddalterfail",
            connect_here: true,
            created: Some("cf"),
            ..BASE
        },
        Fixture {
            file: "ddalterdbf.json",
            source: Source::Empty,
            scratch: "sql-ddalterdbf",
            connect_here: true,
            created: Some("cd"),
            ..BASE
        },
        Fixture {
            file: "mergedd.json",
            source: Source::Empty,
            scratch: "sql-mergedd",
            connect_here: true,
            created: Some("ct"),
            ..BASE
        },
        Fixture {
            file: "ddviews.json",
            source: Source::Empty,
            scratch: "sql-ddviews",
            connect_here: true,
            created: Some("dv"),
            ..BASE
        },
        Fixture {
            file: "ddslots.json",
            source: Source::Empty,
            scratch: "sql-ddslots",
            connect_here: true,
            created: Some("ds"),
            folders: &["ds"],
            ..BASE
        },
        Fixture {
            file: "ddfieldlen.json",
            source: Source::Empty,
            scratch: "sql-ddfieldlen",
            connect_here: true,
            created: Some("fl"),
            folders: &["fl"],
            ..BASE
        },
        Fixture {
            file: "ddtrigrights.json",
            source: Source::Empty,
            scratch: "sql-ddtrigrights",
            connect_here: true,
            created: Some("gq"),
            folders: &["gq"],
            ..BASE
        },
        Fixture {
            file: "ddcolrights.json",
            source: Source::Empty,
            scratch: "sql-ddcolrights",
            connect_here: true,
            created: Some("cq"),
            folders: &["cq"],
            ..BASE
        },
        Fixture {
            file: "ddinsertonly.json",
            source: Source::Empty,
            scratch: "sql-ddinsertonly",
            connect_here: true,
            created: Some("io"),
            folders: &["io"],
            ..BASE
        },
        Fixture {
            file: "ddinsertonly2.json",
            source: Source::Empty,
            scratch: "sql-ddinsertonly2",
            connect_here: true,
            created: Some("i2"),
            folders: &["i2"],
            writes: true,
            ..BASE
        },
        Fixture {
            file: "ddinsertonly3.json",
            source: Source::Empty,
            scratch: "sql-ddinsertonly3",
            connect_here: true,
            created: Some("i3"),
            folders: &["i3"],
            writes: true,
            ..BASE
        },
        Fixture {
            file: "ddeffperm.json",
            source: Source::Empty,
            scratch: "sql-ddeffperm",
            connect_here: true,
            uncompared: true,
            ..BASE
        },
        Fixture {
            file: "ddpublicgrant.json",
            source: Source::Empty,
            scratch: "sql-ddpublicgrant",
            connect_here: true,
            created: Some("pg"),
            folders: &["pg"],
            ..BASE
        },
        Fixture {
            file: "ddpublicbad.json",
            source: Source::Copy("../sql/ddpublicbad"),
            scratch: "sql-ddpublicbad",
            connect_here: true,
            ..BASE
        },
        Fixture {
            file: "ddpublicbad2.json",
            source: Source::Copy("../sql/ddpublicbad2"),
            scratch: "sql-ddpublicbad2",
            connect_here: true,
            ..BASE
        },
        Fixture {
            file: "ddpublicbad3.json",
            source: Source::Copy("../sql/ddpublicbad3"),
            scratch: "sql-ddpublicbad3",
            connect_here: true,
            normalize: Some(drop_count_of_permissions),
            ..BASE
        },
        Fixture {
            file: "ddpublicbad4.json",
            source: Source::Copy("../sql/ddpublicbad4"),
            scratch: "sql-ddpublicbad4",
            connect_here: true,
            normalize: Some(drop_count_of_permissions),
            ..BASE
        },
        Fixture {
            file: "ddpublicbad6.json",
            source: Source::Copy("../sql/ddpublicbad6"),
            scratch: "sql-ddpublicbad6",
            connect_here: true,
            normalize: Some(drop_count_of_permissions),
            ..BASE
        },
        Fixture {
            file: "ddpublicbad7.json",
            source: Source::Copy("../sql/ddpublicbad7"),
            scratch: "sql-ddpublicbad7",
            connect_here: true,
            normalize: Some(drop_count_of_permissions),
            ..BASE
        },
        Fixture {
            file: "ddpublicbad8.json",
            source: Source::Copy("../sql/ddpublicbad8"),
            scratch: "sql-ddpublicbad8",
            connect_here: true,
            normalize: Some(drop_count_of_permissions),
            ..BASE
        },
        Fixture {
            file: "ddpublicbad9.json",
            source: Source::Copy("../sql/ddpublicbad9"),
            scratch: "sql-ddpublicbad9",
            connect_here: true,
            normalize: Some(drop_count_of_permissions),
            ..BASE
        },
        Fixture {
            file: "ddexecrights.json",
            source: Source::Copy("../sql/ddexecrights"),
            scratch: "sql-ddexecrights",
            connect_here: true,
            ..BASE
        },
        Fixture {
            file: "ddpublicbad5.json",
            source: Source::Copy("../sql/ddpublicbad5"),
            scratch: "sql-ddpublicbad5",
            connect_here: true,
            normalize: Some(drop_count_of_permissions),
            ..BASE
        },
        Fixture {
            file: "ddlogins.json",
            source: Source::Empty,
            scratch: "sql-ddlogins",
            connect_here: true,
            created: Some("lg"),
            folders: &["lg"],
            ..BASE
        },
        Fixture {
            file: "ddenc.json",
            source: Source::Copy("ddenc"),
            scratch: "sql-ddenc",
            connect_here: true,
            ..BASE
        },
        Fixture {
            file: "ddvisible.json",
            source: Source::Empty,
            scratch: "sql-ddvisible",
            connect_here: true,
            created: Some("vx"),
            folders: &["vx"],
            ..BASE
        },
        Fixture {
            file: "ddrights.json",
            source: Source::Empty,
            scratch: "sql-ddrights",
            connect_here: true,
            created: Some("dr"),
            folders: &["dr"],
            ..BASE
        },
        Fixture {
            file: "ddbuiltin.json",
            source: Source::Empty,
            scratch: "sql-ddbuiltin",
            connect_here: true,
            created: Some("bg"),
            folders: &["bg"],
            ..BASE
        },
        Fixture {
            file: "ddgrantopt.json",
            source: Source::Empty,
            scratch: "sql-ddgrantopt",
            connect_here: true,
            created: Some("go"),
            folders: &["go"],
            ..BASE
        },
        Fixture {
            file: "ddgrantopt2.json",
            source: Source::Empty,
            scratch: "sql-ddgrantopt2",
            connect_here: true,
            created: Some("g2"),
            folders: &["g2"],
            ..BASE
        },
        Fixture {
            file: "ddapirights.json",
            source: Source::Empty,
            scratch: "sql-ddapirights",
            connect_here: true,
            created: Some("ar"),
            folders: &["ar"],
            ..BASE
        },
        Fixture {
            file: "ddapirights2.json",
            source: Source::Empty,
            scratch: "sql-ddapirights2",
            connect_here: true,
            created: Some("sr"),
            folders: &["sr"],
            ..BASE
        },
        Fixture {
            file: "ddrenamerights.json",
            source: Source::Empty,
            scratch: "sql-ddrenamerights",
            connect_here: true,
            created: Some("ar"),
            folders: &["ar"],
            ..BASE
        },
        Fixture {
            file: "ddcreatedbf.json",
            source: Source::Empty,
            scratch: "sql-ddcreatedbf",
            connect_here: true,
            created: Some("cd2"),
            folders: &["cd2"],
            ..BASE
        },
        Fixture {
            file: "ddfuncrights.json",
            source: Source::Empty,
            scratch: "sql-ddfuncrights",
            connect_here: true,
            created: Some("fr"),
            folders: &["fr"],
            ..BASE
        },
        Fixture {
            file: "ddfuncgroup.json",
            source: Source::Empty,
            scratch: "sql-ddfuncgroup",
            connect_here: true,
            created: Some("fg"),
            folders: &["fg"],
            ..BASE
        },
        Fixture {
            file: "ddadminpk.json",
            source: Source::Empty,
            scratch: "sql-ddadminpk",
            connect_here: true,
            created: Some("ap"),
            folders: &["ap"],
            ..BASE
        },
        Fixture {
            file: "trigself.json",
            source: Source::Empty,
            scratch: "sql-trigself",
            connect_here: true,
            created: Some("dbt"),
            counted: true,
            ..BASE
        },
        Fixture {
            file: "trignn.json",
            source: Source::Empty,
            scratch: "sql-trignn",
            connect_here: true,
            uncompared: true,
            folders: &["zq"],
            ..BASE
        },
        Fixture {
            file: "trigdbopen.json",
            source: Source::Empty,
            scratch: "sql-trigdbopen",
            connect_here: true,
            created: Some("dbt"),
            counted: true,
            ..BASE
        },
        Fixture {
            file: "trigdblog.json",
            source: Source::Empty,
            scratch: "sql-trigdblog",
            connect_here: true,
            created: Some("dbt"),
            counted: true,
            normalize: Some(mask_user_ids),
            ..BASE
        },
        Fixture {
            file: "trigdblog2.json",
            source: Source::Empty,
            scratch: "sql-trigdblog2",
            connect_here: true,
            created: Some("dbt"),
            counted: true,
            normalize: Some(mask_user_ids),
            ..BASE
        },
        Fixture {
            file: "trigdblog3.json",
            source: Source::Empty,
            scratch: "sql-trigdblog3",
            connect_here: true,
            created: Some("dbt"),
            counted: true,
            normalize: Some(mask_user_ids),
            ..BASE
        },
        Fixture {
            file: "trigdbfire.json",
            source: Source::Empty,
            scratch: "sql-trigdbfire",
            connect_here: true,
            created: Some("dbt"),
            counted: true,
            ..BASE
        },
        Fixture {
            file: "trigdb.json",
            source: Source::Empty,
            scratch: "sql-trigdb",
            connect_here: true,
            created: Some("dbt"),
            counted: true,
            ..BASE
        },
        Fixture {
            file: "ddprocs.json",
            source: Source::Empty,
            scratch: "sql-ddprocs",
            connect_here: true,
            created: Some("dp"),
            ..BASE
        },
        Fixture {
            file: "ddprocs2.json",
            source: Source::Empty,
            scratch: "sql-ddprocs2",
            connect_here: true,
            created: Some("dpf"),
            ..BASE
        },
        Fixture {
            file: "ddfuncs.json",
            source: Source::Empty,
            scratch: "sql-ddfuncs",
            connect_here: true,
            created: Some("df"),
            ..BASE
        },
        Fixture {
            file: "ddproclong.json",
            source: Source::Empty,
            scratch: "sql-ddproclong",
            connect_here: true,
            created: Some("dpl"),
            ..BASE
        },
        Fixture {
            file: "dddatabase.json",
            source: Source::Empty,
            scratch: "sql-dddatabase",
            connect_here: true,
            created: Some("dc"),
            ..BASE
        },
        Fixture {
            file: "refuse.json",
            source: Source::Empty,
            scratch: "sql-refuse",
            ..BASE
        },
        Fixture {
            file: "props39.json",
            source: Source::Empty,
            scratch: "sql-props39",
            ..BASE
        },
        Fixture {
            file: "likeorder.json",
            source: Source::Empty,
            scratch: "sql-likeorder",
            ..BASE
        },
        Fixture {
            file: "indexsel.json",
            source: Source::Empty,
            scratch: "sql-indexsel",
            normalize: Some(index_plan_steps),
            ..BASE
        },
        Fixture {
            file: "indexsel2.json",
            source: Source::Empty,
            scratch: "sql-indexsel2",
            normalize: Some(index_plan_steps),
            ..BASE
        },
        Fixture {
            file: "indexsel3.json",
            source: Source::Files(&[
                "charset_german/ca.adt",
                "collation_dynamic/ger.adt",
                "collation_dynamic/ger.adi",
                "collation_dynamic/gen.adt",
                "collation_dynamic/gen.adi",
            ]),
            scratch: "collation-indexsel3",
            normalize: Some(index_plan_steps),
            ..BASE
        },
        Fixture {
            file: "joinorder.json",
            source: Source::Empty,
            scratch: "sql-joinorder",
            normalize: Some(index_plan_steps),
            ..BASE
        },
        Fixture {
            file: "joinsize.json",
            source: Source::Empty,
            scratch: "sql-joinsize",
            normalize: Some(index_plan_steps),
            ..BASE
        },
        Fixture {
            file: "statics.json",
            source: Source::Empty,
            scratch: "sql-statics",
            ..BASE
        },
        Fixture {
            file: "derived.json",
            source: Source::Empty,
            scratch: "sql-derived",
            writes: true,
            normalize: Some(mask_plan_numbers),
            ..BASE
        },
        Fixture {
            file: "likedict.json",
            source: Source::Empty,
            scratch: "sql-likedict",
            connect_here: true,
            created: Some("ld"),
            ..BASE
        },
        Fixture {
            file: "cursors.json",
            source: Source::Empty,
            scratch: "sql-cursors",
            ..BASE
        },
        Fixture {
            file: "cursorlive.json",
            source: Source::Empty,
            scratch: "sql-cursorlive",
            ..BASE
        },
        Fixture {
            file: "cursormore2.json",
            source: Source::Empty,
            scratch: "sql-cursormore2",
            ..BASE
        },
        Fixture {
            file: "cursormore3.json",
            source: Source::Empty,
            scratch: "sql-cursormore3",
            ..BASE
        },
        Fixture {
            file: "cursormore4.json",
            source: Source::Empty,
            scratch: "sql-cursormore4",
            ..BASE
        },
        Fixture {
            file: "cursormore5.json",
            source: Source::Empty,
            scratch: "sql-cursormore5",
            ..BASE
        },
        Fixture {
            file: "trycatch.json",
            source: Source::Empty,
            scratch: "sql-trycatch",
            ..BASE
        },
        Fixture {
            file: "txprocs.json",
            source: Source::Empty,
            scratch: "sql-txprocs",
            connect_here: true,
            created: Some("px"),
            uncompared: true,
            ..BASE
        },
        Fixture {
            file: "txtrig.json",
            source: Source::Empty,
            scratch: "sql-txtrig",
            connect_here: true,
            created: Some("py"),
            uncompared: true,
            ..BASE
        },
        Fixture {
            file: "txtrig3.json",
            source: Source::Empty,
            scratch: "sql-txtrig3",
            connect_here: true,
            uncompared: true,
            folders: &["zr"],
            ..BASE
        },
        Fixture {
            file: "txtrig2.json",
            source: Source::Empty,
            scratch: "sql-txtrig2",
            connect_here: true,
            created: Some("pq"),
            uncompared: true,
            normalize: Some(without_counts),
            ..BASE
        },
        Fixture {
            file: "txddl.json",
            source: Source::Empty,
            scratch: "sql-txddl",
            ..BASE
        },
        Fixture {
            file: "txscripts.json",
            source: Source::Empty,
            scratch: "sql-txscripts",
            ..BASE
        },
        Fixture {
            file: "declare.json",
            source: Source::Empty,
            scratch: "sql-declare",
            ..BASE
        },
        Fixture {
            file: "sysvars.json",
            source: Source::Empty,
            scratch: "sql-sysvars",
            ..BASE
        },
        Fixture {
            file: "limits.json",
            source: Source::Empty,
            scratch: "sql-limits",
            ..BASE
        },
        Fixture {
            file: "limits_procs.json",
            source: Source::Empty,
            scratch: "sql-limits-procs",
            connect_here: true,
            uncompared: true,
            counted: true,
            folders: &["lp"],
            ..BASE
        },
        Fixture {
            file: "subq.json",
            source: Source::Empty,
            scratch: "sql-subq",
            ..BASE
        },
        Fixture {
            file: "spt.json",
            source: Source::Empty,
            scratch: "sql-spt",
            created: Some(""),
            ..BASE
        },
        Fixture {
            file: "sptdbf.json",
            source: Source::Empty,
            scratch: "sql-sptdbf",
            created: Some(""),
            ..BASE
        },
        Fixture {
            file: "spm1.json",
            source: Source::Empty,
            scratch: "sql-spm1",
            created: Some(""),
            ..BASE
        },
        Fixture {
            file: "spm.json",
            source: Source::Empty,
            scratch: "sql-spm",
            created: Some(""),
            uncompared: true,
            normalize: Some(mask_server_lists),
            ..BASE
        },
        Fixture {
            file: "spcat.json",
            source: Source::Empty,
            scratch: "sql-spcat",
            created: Some(""),
            uncompared: true,
            ..BASE
        },
        Fixture {
            file: "spcoll.json",
            source: Source::Empty,
            scratch: "sql-spcoll",
            normalize: Some(mask_collation_numbers),
            ..BASE
        },
        Fixture {
            file: "spcoll2.json",
            source: Source::Empty,
            scratch: "sql-spcoll2",
            created: Some(""),
            uncompared: true,
            normalize: Some(mask_collation_numbers),
            ..BASE
        },
        Fixture {
            file: "spddcat.json",
            source: Source::Empty,
            scratch: "sql-spddcat",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            normalize: Some(mask_server_lists),
            ..BASE
        },
        Fixture {
            file: "spql.json",
            source: Source::Empty,
            scratch: "sql-spql",
            created: Some(""),
            uncompared: true,
            normalize: Some(mask_query_log),
            ..BASE
        },
        Fixture {
            file: "spql2.json",
            source: Source::Empty,
            scratch: "sql-spql2",
            created: Some(""),
            uncompared: true,
            normalize: Some(mask_query_log),
            ..BASE
        },
        Fixture {
            file: "spql3.json",
            source: Source::Empty,
            scratch: "sql-spql3",
            created: Some(""),
            uncompared: true,
            normalize: Some(mask_query_log),
            ..BASE
        },
        Fixture {
            file: "spql6.json",
            source: Source::Empty,
            scratch: "sql-spql6",
            created: Some(""),
            ..BASE
        },
        Fixture {
            file: "spddql.json",
            source: Source::Empty,
            scratch: "sql-spddql",
            connect_here: true,
            created: Some("de"),
            uncompared: true,
            normalize: Some(mask_query_log),
            ..BASE
        },
        Fixture {
            file: "spddql2.json",
            source: Source::Empty,
            scratch: "sql-spddql2",
            connect_here: true,
            created: Some("de"),
            uncompared: true,
            normalize: Some(mask_query_log),
            ..BASE
        },
        Fixture {
            file: "spddql3.json",
            source: Source::Empty,
            scratch: "sql-spddql3",
            connect_here: true,
            created: Some("de"),
            uncompared: true,
            normalize: Some(mask_query_log),
            ..BASE
        },
        Fixture {
            file: "spddql4.json",
            source: Source::Empty,
            scratch: "sql-spddql4",
            connect_here: true,
            created: Some("de"),
            uncompared: true,
            normalize: Some(mask_query_log),
            ..BASE
        },
        Fixture {
            file: "spmg.json",
            source: Source::Empty,
            scratch: "sql-spmg",
            created: Some(""),
            uncompared: true,
            normalize: Some(mask_server_lists),
            ..BASE
        },
        Fixture {
            file: "spddmg.json",
            source: Source::Empty,
            scratch: "sql-spddmg",
            connect_here: true,
            created: Some("de"),
            uncompared: true,
            normalize: Some(mask_server_lists),
            ..BASE
        },
        Fixture {
            file: "sptdd.json",
            source: Source::Empty,
            scratch: "sql-sptdd",
            connect_here: true,
            created: Some("dt"),
            ..BASE
        },
        Fixture {
            file: "spm1dd.json",
            source: Source::Empty,
            scratch: "sql-spm1dd",
            connect_here: true,
            created: Some("dm"),
            ..BASE
        },
        Fixture {
            file: "procvars.json",
            source: Source::Empty,
            scratch: "sql-procvars",
            connect_here: true,
            created: Some("pv"),
            uncompared: true,
            ..BASE
        },
        Fixture {
            file: "cursorprocs.json",
            source: Source::Empty,
            scratch: "sql-cursorprocs",
            connect_here: true,
            created: Some("cp"),
            uncompared: true,
            ..BASE
        },
        Fixture {
            file: "loops.json",
            source: Source::Empty,
            scratch: "sql-loops",
            connect_here: true,
            created: Some("cp"),
            uncompared: true,
            ..BASE
        },
        Fixture {
            file: "cursortrig.json",
            source: Source::Empty,
            scratch: "sql-cursortrig",
            connect_here: true,
            created: Some("cp"),
            uncompared: true,
            ..BASE
        },
        // The dictionary procedures (`tools/ace/sql/spdd*.txt`) make a
        // dictionary of their own, in the folder `de` of an empty one.
        Fixture {
            file: "spddusers.json",
            source: Source::Empty,
            scratch: "sql-spddusers",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddpwchange.json",
            source: Source::Empty,
            scratch: "sql-spddpwchange",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddadminpw.json",
            source: Source::Empty,
            scratch: "sql-spddadminpw",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddri.json",
            source: Source::Empty,
            scratch: "sql-spddri",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddgprops.json",
            source: Source::Empty,
            scratch: "sql-spddgprops",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spdduprops.json",
            source: Source::Empty,
            scratch: "sql-spdduprops",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddvprops.json",
            source: Source::Empty,
            scratch: "sql-spddvprops",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddpprops.json",
            source: Source::Empty,
            scratch: "sql-spddpprops",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddiprops.json",
            source: Source::Empty,
            scratch: "sql-spddiprops",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddtgprops.json",
            source: Source::Empty,
            scratch: "sql-spddtgprops",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spdddprops.json",
            source: Source::Empty,
            scratch: "sql-spdddprops",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddtprops.json",
            source: Source::Empty,
            scratch: "sql-spddtprops",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddtrig.json",
            source: Source::Empty,
            scratch: "sql-spddtrig",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddtables.json",
            source: Source::Empty,
            scratch: "sql-spddtables",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddrename.json",
            source: Source::Empty,
            scratch: "sql-spddrename",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddrenameapi.json",
            source: Source::Empty,
            scratch: "sql-spddrenameapi",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddrenamerepl.json",
            source: Source::Empty,
            scratch: "sql-spddrenamerepl",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "ddrepl2.json",
            source: Source::Empty,
            scratch: "sql-ddrepl2",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "ddrepl.json",
            source: Source::Empty,
            scratch: "sql-ddrepl",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddrenamerepl2.json",
            source: Source::Empty,
            scratch: "sql-spddrenamerepl2",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddrenamedbf.json",
            source: Source::Empty,
            scratch: "sql-spddrenamedbf",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddmove.json",
            source: Source::Empty,
            scratch: "sql-spddmove",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddnum.json",
            source: Source::Empty,
            scratch: "sql-spddnum",
            connect_here: true,
            created: Some("de"),
            uncompared: true,
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddparam.json",
            source: Source::Empty,
            scratch: "sql-spddparam",
            connect_here: true,
            created: Some("de"),
            uncompared: true,
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddenc.json",
            source: Source::Empty,
            scratch: "sql-spddenc",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddsec.json",
            source: Source::Empty,
            scratch: "sql-spddsec",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddlinks.json",
            source: Source::Empty,
            scratch: "sql-spddlinks",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddri2.json",
            source: Source::Empty,
            scratch: "sql-spddri2",
            connect_here: true,
            created: Some("de"),
            uncompared: true,
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddrilong.json",
            source: Source::Empty,
            scratch: "sql-spddrilong",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddalterfail2.json",
            source: Source::Empty,
            scratch: "sql-spddalterfail2",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddalterfail3.json",
            source: Source::Empty,
            scratch: "sql-spddalterfail3",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddalterfail.json",
            source: Source::Empty,
            scratch: "sql-spddalterfail",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddidxcomment.json",
            source: Source::Empty,
            scratch: "sql-spddidxcomment",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddidxcomment2.json",
            source: Source::Empty,
            scratch: "sql-spddidxcomment2",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddlongalter2.json",
            source: Source::Empty,
            scratch: "sql-spddlongalter2",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddlongalter3.json",
            source: Source::Empty,
            scratch: "sql-spddlongalter3",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddlongidx.json",
            source: Source::Empty,
            scratch: "sql-spddlongidx",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddlongalter.json",
            source: Source::Empty,
            scratch: "sql-spddlongalter",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        Fixture {
            file: "spddlong.json",
            source: Source::Empty,
            scratch: "sql-spddlong",
            connect_here: true,
            created: Some("de"),
            folders: SPDD_FOLDERS,
            ..BASE
        },
        // The views are made in a dictionary of the folder, and the scripts
        // edit its tables.
        Fixture {
            file: "views.json",
            source: Source::CopyShared,
            scratch: "sql-views",
            connect_here: true,
            normalize: Some(number_made_statements),
            ..BASE
        },
        Fixture {
            file: "orderby.json",
            source: Source::CopyShared,
            scratch: "sql-orderby",
            connect_here: true,
            ..BASE
        },
        // It makes tables `t1` and `t3`, which the shared folder's
        // `sp_GetTables` in `statements.json` would list.
        Fixture {
            file: "gram_assign.json",
            source: Source::CopyShared,
            scratch: "sql-gram-assign",
            connect_here: true,
            ..BASE
        },
        Fixture {
            file: "trigvars.json",
            source: Source::Copy("psm"),
            scratch: "sql-trigvars",
            connect_here: true,
            ..BASE
        },
        Fixture {
            file: "trigtyped.json",
            source: Source::Copy("psm"),
            scratch: "sql-trigtyped",
            connect_here: true,
            ..BASE
        },
        Fixture {
            file: "triggers.json",
            source: Source::Copy("psm"),
            scratch: "sql-triggers",
            connect_here: true,
            ..BASE
        },
        Fixture {
            file: "trigerr.json",
            source: Source::Copy("psm"),
            scratch: "sql-trigerr",
            connect_here: true,
            counted: true,
            ..BASE
        },
        Fixture {
            file: "trigproc.json",
            source: Source::Copy("psm"),
            scratch: "sql-trigproc",
            connect_here: true,
            counted: true,
            ..BASE
        },
        Fixture {
            file: "trigbig.json",
            source: Source::Empty,
            scratch: "sql-trigbig",
            connect_here: true,
            created: Some("bgd"),
            folders: &["bgd"],
            counted: true,
            ..BASE
        },
        Fixture {
            file: "trigorder.json",
            source: Source::Copy("psm"),
            scratch: "sql-trigorder",
            connect_here: true,
            counted: true,
            ..BASE
        },
        Fixture {
            file: "trigorder2.json",
            source: Source::Copy("psm"),
            scratch: "sql-trigorder2",
            connect_here: true,
            counted: true,
            ..BASE
        },
        Fixture {
            file: "trigproc2.json",
            source: Source::Copy("psm"),
            scratch: "sql-trigproc2",
            connect_here: true,
            counted: true,
            ..BASE
        },
        Fixture {
            file: "execproc.json",
            source: Source::Copy("psm"),
            scratch: "sql-execproc",
            connect_here: true,
            ..BASE
        },
        // The `ca` table without its index, which was built under German
        // (as `run_collation_compare.sh` copies it).
        Fixture {
            file: "collation_compare.json",
            source: Source::Files(&["charset_german/ca.adt"]),
            scratch: "collation-compare",
            german: true,
            ..BASE
        },
        Fixture {
            file: "collation_compare_default.json",
            source: Source::Files(&["charset_german/ca.adt"]),
            scratch: "collation-compare-default",
            ..BASE
        },
        // The `ca` table without its index, and the `ger` and `gen` tables
        // with theirs (as `run_collation_dynamic.sh` copies them).
        Fixture {
            file: "collation_dynamic.json",
            source: Source::Files(&[
                "charset_german/ca.adt",
                "collation_dynamic/ger.adt",
                "collation_dynamic/ger.adi",
                "collation_dynamic/gen.adt",
                "collation_dynamic/gen.adi",
            ]),
            scratch: "collation-dynamic",
            ..BASE
        },
        Fixture {
            file: "setcollation.json",
            source: Source::Files(&[
                "charset_german/ca.adt",
                "collation_dynamic/ger.adt",
                "collation_dynamic/ger.adi",
                "collation_dynamic/gen.adt",
                "collation_dynamic/gen.adi",
            ]),
            scratch: "collation-setcollation",
            ..BASE
        },
        // The `co` table of `charset_default` alone (as `oem.txt` says).
        Fixture {
            file: "oem.json",
            source: Source::Files(&["ntx/charset_default/co.dbf"]),
            scratch: "sql-oem",
            byte_text: true,
            ..BASE
        },
        Fixture {
            file: "oemrows.json",
            source: Source::Files(&["ntx/charset_default/co.dbf"]),
            scratch: "sql-oemrows",
            byte_text: true,
            ..BASE
        },
        // The dictionary of the original's functions, in a copy.
        Fixture {
            file: "udf.json",
            source: Source::Copy("../sql/udf"),
            scratch: "sql-udf",
            connect_here: true,
            ..BASE
        },
        // The full-text tables alone, in a copy (as `run.sh` reads them).
        Fixture {
            file: "fts_plans.json",
            source: Source::Copy("../sql/fts"),
            scratch: "sql-fts-plans",
            ..BASE
        },
    ];

    impl Fixture {
        /// The fixture of the script `file`.
        fn of(file: &str) -> &'static Fixture {
            FIXTURES
                .iter()
                .find(|fixture| fixture.file == file)
                .unwrap_or(&BASE)
        }

        /// Makes the data folder and connects to it, and tells the commands
        /// where it is.
        fn open(&self) -> (std::path::PathBuf, AdsHandle) {
            let from = match self.source {
                Source::Shared => return (fixtures::sql_data(), fixtures::connect_sql()),
                Source::Dml => {
                    let data = fixtures::sql_dml_data(fixtures::is_remote());
                    return (data, fixtures::connect_sql_dml());
                }
                Source::Empty | Source::Files(_) | Source::Folders(_) => fixtures::empty_dir(),
                Source::Copy(folder) => fixtures::dir(folder),
                Source::CopyShared => fixtures::sql_data(),
            };
            let (source, dir) = fixtures::scratch_from(self.scratch, &from);
            match self.source {
                Source::Files(files) => {
                    for file in files {
                        let name = file.rsplit('/').next().unwrap();
                        std::fs::copy(fixtures::dir(file), dir.join(name)).unwrap();
                    }
                }
                Source::Folders(folders) => {
                    for (folder, into) in folders {
                        fixtures::copy_files(&fixtures::dir(folder), &dir.join(into));
                    }
                }
                _ => {}
            }
            for folder in self.created.iter().chain(self.folders) {
                std::fs::create_dir_all(dir.join(folder)).unwrap();
            }
            if self.connect_here {
                CONNECT_DATA.set(Some(dir.clone()));
            }
            if self.links {
                LINK_DATA.set(Some(dir.display().to_string()));
            }
            if self.created.is_some() {
                CREATED_DATA.set(Some(dir.clone()));
            }
            (dir, fixtures::connect_to(&source))
        }
    }

    /// Replays `file` as [`replay`] does, then compares the files it made
    /// with the original engine's if its fixture names a folder for them
    /// (see [`compare_created`]).
    /// Replays `fixtures/sql/limits_procs.json`: how deep procedures and
    /// triggers nest, and how long a procedure's script is
    /// (`tools/ace/sql/limits_procs.txt`, made by `limits_procs.py`), in a
    /// dictionary it makes in an empty folder.
    #[test]
    fn nesting_and_script_limits_match_the_original_engine() {
        // 64 procedures running inside one another take more stack than a
        // test's thread has in a build without optimisation.
        std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn(|| replay_compared("limits_procs.json", &[]))
            .unwrap()
            .join()
            .unwrap();
    }

    fn replay_compared(file: &str, unchecked: &[&str]) {
        replay(file);
        let fixture = Fixture::of(file);
        if let (Some(folder), false) = (fixture.created, fixture.uncompared) {
            compare_created(file.trim_end_matches(".json"), folder, unchecked);
        }
    }

    fn replay(file: &str) {
        // The dates are in the default date format, which other tests change.
        let _settings = fixtures::settings_lock();
        // Nothing of an earlier replay of the test is left: a fixture sets
        // what it needs, and `compare_created` reads what the last one did.
        CONNECT_DATA.set(None);
        OTHER.set([0; 3]);
        LINK_DATA.set(None);
        CREATED_DATA.set(None);
        DUMP_CAP.set(200);
        let fixture = Fixture::of(file);
        let german = fixture.german;
        BYTE_TEXT.set(fixture.byte_text);
        /// Puts the transactions back to real ones when the replay is over.
        struct Counted;
        impl Drop for Counted {
            fn drop(&mut self) {
                tflip_engine::settings::set_real_transactions(&crate::LOCAL, true);
            }
        }
        let _counted = fixture.counted.then(|| {
            tflip_engine::settings::set_real_transactions(&crate::LOCAL, false);
            Counted
        });
        if german && let Some(dir) = crate::config::collation_dir() {
            let collations = tflip_engine::collations::Collations::new(&dir, "German", "");
            tflip_engine::settings::set_collations(&crate::LOCAL, collations);
        }
        let path = fixtures::dir(&format!("../sql/{file}"));
        let steps: Vec<Json> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let (data, mut connect) = fixture.open();
        OPENED.set(vec![connect]);
        let data = data.display().to_string();
        let (mut stmt, mut cur) = (0, 0);
        let mut failures = Vec::new();
        // `TFLIP_SQL_DUMP=<file>` writes what tflip answers there, in the
        // fixture's format.
        let mut dump = Vec::new();
        let mut failed = false;
        let mut wrote = false;
        // Whether the statement before is one that differs as known, so
        // that what asks about its cursor may too.
        let mut following = false;
        let mut prepared = String::new();
        let claims = if fixture.file.starts_with("claims") {
            claims_known(fixture.file)
        } else {
            Default::default()
        };
        for (index, step) in steps.iter().enumerate() {
            let cmd = step["cmd"].as_str().unwrap();
            if cmd.starts_with("prepare ") {
                prepared = cmd.to_owned();
            }
            if matches!(cmd.split(' ').next(), Some("direct" | "query" | "exec")) {
                following = false;
            }
            let mut got = run(cmd, &mut connect, &mut stmt, &mut cur);
            dump.push(got.clone());
            normalize(&mut got, &data);
            let mut want = step.clone();
            normalize(&mut want, &data);
            // The rows a statement that failed affected: the original
            // engine reports them erratically (see KNOWN_ISSUES.md).
            if cmd == "count" && failed {
                for result in [&mut got, &mut want] {
                    if let Some(object) = result.as_object_mut() {
                        object.remove("count");
                    }
                }
            }
            // The `affected` count a failed `direct`/`query` reports is erratic
            // the same way (the engine leaves it 0xFFFFFFFF).
            if step
                .get("rc")
                .and_then(Json::as_u64)
                .is_some_and(|rc| rc != 0)
            {
                for result in [&mut got, &mut want] {
                    if let Some(object) = result.as_object_mut() {
                        object.remove("affected");
                    }
                }
            }
            failed = step
                .get("rc")
                .and_then(Json::as_u64)
                .is_some_and(|rc| rc != 0)
                && cmd != "count";
            // The original engine's plan cursors have an index of their
            // own; where tflip plans otherwise, only the statement's row is
            // compared.
            if shows_plan(cmd) {
                for result in [&mut got, &mut want] {
                    if let Some(cursor) = result.get_mut("cursor").and_then(Json::as_object_mut) {
                        cursor.remove("indexes");
                        if planned_otherwise(cmd) {
                            cursor.remove("count");
                            if let Some(Json::Array(rows)) = cursor.get_mut("rows") {
                                rows.truncate(1);
                            }
                        }
                    }
                }
            }
            if let Some(adjust) = fixture.normalize {
                for result in [&mut got, &mut want] {
                    adjust(cmd, result);
                }
            }
            // The open statements, their numbers and start times depend on
            // the run and the host: only their columns are compared.
            if cmd.contains("sp_GetSQLStatements") {
                for result in [&mut got, &mut want] {
                    if let Some(cursor) = result.get_mut("cursor").and_then(Json::as_object_mut) {
                        cursor.remove("rows");
                        cursor.remove("count");
                    }
                }
            }
            // Without SQL, both leave the buffer as it was: whatever was in
            // the probe's memory.
            let untouched = |v: &Json| {
                v.get("sql")
                    .and_then(Json::as_str)
                    .map(|s| s.chars().count())
            };
            if untouched(&got) == Some(4096) && untouched(&want) == Some(4096) {
                got["sql"] = want["sql"].clone();
            }
            // A closed handle the original engine still caches.
            if let Some(Json::String(error)) = want.get_mut("error")
                && error == "E5094"
            {
                *error = "E5018".to_owned();
            }
            // The original engine's live cursors take edits, which tflip's
            // do not (see KNOWN_ISSUES): what the tables hold after them is
            // not compared.
            if !fixture.writes
                && matches!(cmd.split(' ').next(), Some("edit" | "append" | "delrec"))
            {
                wrote = true;
            }
            let differs_known =
                known(cmd) || claims.contains(cmd) || (cmd == "exec" && claims.contains(&prepared));
            if got != want && !wrote && !following && !differs_known {
                failures.push(format!(
                    "{cmd}{}\n  step: {index}",
                    differences(&got, &want)
                ));
            } else if differs_known {
                following = true;
            }
        }
        // Only those still open: a closed one's handle may be another
        // test's by now.
        // (0 would close every connection of the process.)
        for open in OPENED.take().into_iter().filter(|&open| open != 0) {
            AdsDisconnect(open);
        }
        CONNECT_DATA.set(None);
        if german {
            tflip_engine::settings::set_collations(&crate::LOCAL, crate::local_collations());
        }
        if let Ok(path) = std::env::var("TFLIP_SQL_DUMP") {
            std::fs::write(path, serde_json::to_vec_pretty(&dump).unwrap()).unwrap();
        }
        assert!(
            failures.is_empty(),
            "{} differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// The statements whose plan in tflip differs from the original
    /// engine's, as tflip runs them otherwise: a live cursor uses no index
    /// (`AOF SCAN (LIVE)`, `ORDER BY` an index's key); it joins on equal
    /// columns through a hash where no index finds the right rows, rather
    /// than scanning the right table per row, and never swaps a join's
    /// tables; it looks `IN` lists up in an index, scans the table for
    /// `COUNT(*)`, keeps a join's condition in the join, passes no
    /// restriction on to a `SELECT` in `FROM`, and shows that `SELECT`'s
    /// plan, a `TOP`, the subqueries of the select list and which ones run
    /// for each row around them.
    fn planned_otherwise(cmd: &str) -> bool {
        let Some(statement) = cmd.strip_prefix("query SHOW PLAN FOR ") else {
            return false;
        };
        [
            "SELECT c.name, s.item FROM cust c JOIN sales s ON c.id = s.cust",
            "SELECT * FROM cust WHERE name LIKE 'A%'",
            // A view's `WHERE` and the statement's, which the original
            // engine writes `(a).AND.(b)`, the view's first.
            "SELECT * FROM v_where WHERE id < 4 ORDER BY name",
            "SELECT * FROM v_of_view_where",
            "SELECT cust, SUM(qty) FROM sales WHERE paid GROUP BY cust HAVING SUM(qty) > 5 \
             ORDER BY 2 DESC",
            "SELECT c.name, s.item FROM cust c LEFT JOIN sales s ON c.id = s.cust",
            "SELECT c.name, s.item FROM cust c RIGHT JOIN sales s ON c.id = s.cust",
            "SELECT c.name, s.item FROM cust c FULL JOIN sales s ON c.id = s.cust",
            "SELECT c.name, s.item FROM cust c, sales s WHERE c.id = s.cust AND s.qty > 1",
            "SELECT c.name, s.item, i.name FROM cust c JOIN sales s ON c.id = s.cust \
             JOIN indexed i ON i.id = c.id",
            "SELECT i.name, s.item FROM sales s JOIN indexed i ON i.id = s.cust",
            "SELECT name FROM cust c WHERE EXISTS (SELECT * FROM sales s WHERE s.cust = c.id)",
            "SELECT * FROM (SELECT cust, COUNT(*) AS n FROM sales GROUP BY cust) t WHERE n > 1",
            "SELECT TOP 2 * FROM cust",
            // The original engine makes an AOF of what a `LIKE` begins
            // with, besides the restriction.
            "SELECT * FROM indexed WHERE name LIKE 'B%' AND id = 2",
            "SELECT name, (SELECT COUNT(*) FROM sales s WHERE s.cust = c.id) FROM cust c",
            "SELECT TOP 5 * FROM indexed WHERE id > 1",
            "SELECT TOP 5 * FROM indexed WHERE id > 1 AND name = 'Bravo'",
            "SELECT TOP 5 * FROM indexed WHERE amount > 1 AND name LIKE 'B%'",
            "SELECT c.name, i.name FROM cust c JOIN indexed i ON i.id = c.id",
            "SELECT c.name, i.name FROM cust c, indexed i WHERE i.id = c.id AND i.amount > 2",
            "SELECT name FROM cust c WHERE EXISTS (SELECT * FROM indexed i WHERE i.id = c.id)",
            "SELECT k_autoinc + 0 FROM keys WHERE k_integer IN (1, -1)",
            // Static cursors (`fts_plans.json`) whose plans are not the
            // full-text ones: an index's scope with another restriction, an
            // unindexed restriction alone, `TOP`, a join, `COUNT(*)`, and an
            // `ORDER BY` an index serves or none does (see KNOWN_ISSUES.md);
            // and restrictions on an expression or `IS NULL` beside a
            // `CONTAINS`, which tflip does not show.
            "SELECT id, SCORE(body, 'red') FROM px WHERE CONTAINS(body, 'red') ORDER BY n",
            "SELECT id, SCORE(body, 'red') FROM px WHERE n = 10 AND id = 1",
            "SELECT id, SCORE(body, 'red') FROM px WHERE id = 1 AND n = 10",
            "SELECT {static} id FROM px WHERE n = 10 AND id = 1",
            "SELECT {static} id FROM px WHERE id = 1 AND n = 10",
            "SELECT {static} id FROM px WHERE id = 1",
            "SELECT TOP 1 id, SCORE(body, 'red') FROM px WHERE CONTAINS(body, 'red')",
            "SELECT {static} id FROM px WHERE CONTAINS(body, 'red') AND id + 1 = 2",
            "SELECT {static} id FROM px WHERE CONTAINS(body, 'red') AND id IS NULL",
            "SELECT {static} p.id FROM px p JOIN px q ON p.id = q.id WHERE CONTAINS(p.body, 'red')",
            "SELECT COUNT(*) FROM px WHERE CONTAINS(body, 'red')",
            "SELECT {static} id FROM px ORDER BY n",
            "SELECT {static} id FROM px ORDER BY id",
            "SELECT {static} id, n FROM px ORDER BY n",
            "SELECT id, SCORE(body, 'red') FROM px ORDER BY id",
            // `fixtures/sql/derived.json`: the `WHERE`s of a derived table
            // and of the `SELECT` around it as the view's are, a restriction
            // of the outer `WHERE` the original passes on to a derived table
            // it scans, the tables of a join it swaps or reads by an index
            // on the table a derived table is of, and a `COUNT(*)` it
            // lists once for each time it is written.
            "SELECT * FROM (SELECT id, name FROM t WHERE id > 1) x WHERE id < 4",
            "SELECT * FROM (SELECT * FROM (SELECT id, name FROM t WHERE id > 1) a WHERE id < 4) b",
            "SELECT * FROM (SELECT id + 1 AS k, name FROM t) x WHERE k > 1",
            "SELECT COUNT(*), COUNT(*) FROM t",
            "SELECT COUNT(*) FROM t HAVING COUNT(*) > 1",
            "SELECT COUNT(*) FROM t, t u",
            "SELECT * FROM (SELECT id, name FROM t) x, t u WHERE x.id = u.id",
            "SELECT * FROM t u, (SELECT id, name FROM t) x WHERE x.id = u.id",
            "SELECT * FROM (SELECT id, name FROM t WHERE id > 1) x JOIN (SELECT id, n FROM t) y \
             ON x.id = y.id",
            "SELECT x.id, y.id FROM (SELECT id FROM t) x, (SELECT id FROM t) y",
            // An index serves the `WHERE` of an `UPDATE` or `DELETE` as an
            // AOF scan in the original, as an index scan in tflip.
            "UPDATE b SET val = 1 WHERE id = 1",
            "DELETE FROM b WHERE id = 1",
        ]
        .contains(&statement)
            // The original's plan of an `UPDATE … FROM` joins the target
            // first, with the conditions of its `JOIN`s and its `WHERE` in
            // one step (KNOWN_ISSUES.md).
            || statement.starts_with("UPDATE ") && statement.contains(" FROM ")
    }

    /// The keys of two results that differ, each value shortened.
    fn differences(got: &Json, want: &Json) -> String {
        let short = |v: Option<&Json>| {
            let text = v.map_or("-".to_owned(), Json::to_string);
            let n = text.chars().count();
            let head: String = text.chars().take(300).collect();
            if n > 300 {
                format!("{head}… ({n} characters)")
            } else {
                head
            }
        };
        let (Json::Object(g), Json::Object(w)) = (got, want) else {
            return String::new();
        };
        let mut keys: Vec<&String> = g.keys().chain(w.keys()).collect();
        keys.sort();
        keys.dedup();
        let mut out = String::new();
        for key in keys {
            if g.get(key) != w.get(key) {
                if let (Some(Json::Object(gc)), Some(Json::Object(wc))) = (g.get(key), w.get(key)) {
                    for (k, wv) in wc {
                        if gc.get(k) != Some(wv) {
                            out += &format!(
                                "\n  {key}.{k}: got {} want {}",
                                short(gc.get(k)),
                                short(Some(wv))
                            );
                        }
                    }
                    continue;
                }
                out += &format!(
                    "\n  {key}: got {} want {}",
                    short(g.get(key)),
                    short(w.get(key))
                );
            }
        }
        out
    }

    /// Where tflip differs from the original engine on purpose or for now
    /// (see `docs/KNOWN_ISSUES.md`).
    fn known(cmd: &str) -> bool {
        [
            // Orders on an expression (`UPPER(a)`, `c + d`) are not looked up: the
            // original's plan has an AOF scan of them (KNOWN_ISSUES.md).
            "query SHOW PLAN FOR SELECT id + 0 AS r, a, b, c, d FROM xs WHERE UPPER(a) = 'APPLE'",
            "query SHOW PLAN FOR SELECT id + 0 AS r, a, b, c, d FROM xs WHERE UPPER(a) >= 'F'",
            "query SHOW PLAN FOR SELECT id + 0 AS r, a, b, c, d FROM xs WHERE c + d = 4",
            "query SHOW PLAN FOR SELECT id + 0 AS r, a, b, c, d FROM xs WHERE c + d >= 4",
            // A join on an expression that an order on the other table finds, and
            // three tables joined (KNOWN_ISSUES.md).
            "query SHOW PLAN FOR SELECT ys.n, xs.id + 0 AS r, xs.a FROM ys JOIN xs ON xs.id = ys.n / 10",
            "query SHOW PLAN FOR SELECT ys.n, xs.id + 0 AS r, xs.a FROM ys LEFT JOIN xs ON xs.id = ys.n / 10",
            "query SELECT xs.id + 0 AS xid, xt.m, ys.n + 0 AS yn FROM ys JOIN xs ON xs.id = ys.id JOIN xt ON xt.id = xs.id",
            "query SHOW PLAN FOR SELECT xs.id + 0 AS xid, xt.m, ys.n + 0 AS yn FROM ys JOIN xs ON xs.id = ys.id JOIN xt ON xt.id = xs.id",
            "query SELECT xs.id + 0 AS xid, xt.m, ys.n + 0 AS yn FROM xs JOIN ys ON xs.id = ys.id JOIN xt ON xt.id = ys.id",
            "query SHOW PLAN FOR SELECT xs.id + 0 AS xid, xt.m, ys.n + 0 AS yn FROM xs JOIN ys ON xs.id = ys.id JOIN xt ON xt.id = ys.id",
            // A join the original keeps in the order written though the right table
            // is smaller (p4 and p6, not understood), the equality of a `WHERE` on a
            // join's column it passes on to the other table too, and three tables
            // joined, which it reads in an order of its own (KNOWN_ISSUES.md).
            "query SELECT p4.k + 0 AS ak, p6.v + 0 AS bv FROM p4 JOIN p6 ON p4.k = p6.k",
            "query SHOW PLAN FOR SELECT p4.k + 0 AS ak, p6.v + 0 AS bv FROM p4 JOIN p6 ON p4.k = p6.k",
            "query SHOW PLAN FOR SELECT p2.k + 0 AS ak, p6.v + 0 AS bv FROM p2 JOIN p6 ON p2.k = p6.k WHERE p6.k = 8",
            "query SHOW PLAN FOR SELECT p2.k + 0 AS ak, p1.v + 0 AS bv, p3.v + 0 AS cv FROM p2 JOIN p1 ON p2.k = p1.k JOIN p3 ON p3.k = p2.k",
            "query SELECT p2.k + 0 AS ak, p1.v + 0 AS bv, p6.v + 0 AS cv FROM p6 JOIN p2 ON p2.k = p6.k JOIN p1 ON p1.k = p6.k",
            "query SHOW PLAN FOR SELECT p2.k + 0 AS ak, p1.v + 0 AS bv, p6.v + 0 AS cv FROM p6 JOIN p2 ON p2.k = p6.k JOIN p1 ON p1.k = p6.k",
            // A script that commits an inner level of its own nested begins
            // leaves a transaction whose `COMMIT` or `ROLLBACK` the original
            // carries out but answers with 5047 (`docs/ADS_BUGS.md`).
            "direct COMMIT /* sequence a */",
            "direct ROLLBACK /* sequence b */",
            "direct COMMIT /* sequence c */",
            // At permission level 1 an expression over a column the user may
            // not read is NULL in a select list, though a condition on it
            // sees what it holds; tflip reads such a column as NULL in a
            // select list only through a live cursor (KNOWN_ISSUES.md).
            "query SELECT UPPER(secret) FROM t1",
            // `FUNCTION` and `PACKAGE` statements are 2200 (the original's
            // free connection has no dictionary to put them in: 5125).
            "direct CREATE FUNCTION f() RETURNS INTEGER BEGIN RETURN 1; END",
            "direct CREATE PACKAGE p",
            "direct DROP FUNCTION f",
            // A trigger made on a second connection while the first holds the
            // table is never seen by the original, which keeps the table as
            // it opened it; tflip sees it once the statement closes
            // (`fixtures/sql/txtrig3.json`, KNOWN_ISSUES.md).
            "query SELECT msg FROM audit /* created */",
            "query SELECT msg FROM audit /* held */",
            "query SELECT msg FROM audit /* dropped */",
            // The original's `CAST` of GUID text to a GUID fails with 5070
            // where it is a column (and compares unequal to the same text
            // in a `WHERE`), and a `GUID'…'` literal fails to parse at a
            // place of its own (`docs/ADS_BUGS.md`, `KNOWN_ISSUES.md`).
            "expr CAST('{6F9619FF-8B86-D011-B42D-00C04FC964FF}' AS SQL_GUID)",
            "expr CAST('6F9619FF-8B86-D011-B42D-00C04FC964FF' AS SQL_GUID)",
            "expr CAST('6F9619FF8B86D011B42D00C04FC964FF' AS SQL_GUID)",
            "expr GUID'6F9619FF-8B86-D011-B42D-00C04FC964FF'",
            "query SELECT id FROM g WHERE g = CAST('6F9619FF-8B86-D011-B42D-00C04FC964FF' AS \
             SQL_GUID)",
            "query SELECT id FROM g WHERE g = CAST('xyz' AS SQL_GUID)",
            "directnull INSERT INTO g VALUES (20, CAST('6F9619FF-8B86-D011-B42D-00C04FC964FF' AS \
             SQL_GUID))",
            // The row 20 that insert wrote here.
            "query SELECT id FROM g WHERE CAST(g AS SQL_CHAR) = \
             '6f9619ff-8b86-d011-b42d-00c04fc964ff'",
            "direct DROP PACKAGE p",
            "direct ALTER FUNCTION f",
            // `IFNULL` takes a VFP field's empty value (0, false, spaces) for
            // NULL (KNOWN_ISSUES.md).
            "query SELECT id, IFNULL(n, 9), IFNULL(c, 'z'), IFNULL(l, TRUE), COALESCE(n, 7), \
             COALESCE(l, TRUE) FROM vfp_b",
            // Of values a CICHARACTER group holds that differ in case, the
            // original engine shows one tflip cannot tell (here the last;
            // after a join, the first).
            "query SELECT city, COUNT(*) FROM cust GROUP BY city",
            "query SELECT DISTINCT c FROM cit ORDER BY c",
            "query SELECT c, COUNT(*) AS n FROM cit GROUP BY c ORDER BY c",
            // So of different text a dynamic collation equates
            // (`collation_dynamic.json`; first of few, often last of many).
            "query SELECT DISTINCT c FROM ca g ORDER BY c",
            "query SELECT DISTINCT w FROM ca g ORDER BY w",
            "query SELECT c, COUNT(*) AS n FROM ca g GROUP BY c ORDER BY c",
            "query SELECT w, COUNT(*) AS n FROM ca g GROUP BY w ORDER BY w",
            "query SELECT c FROM ca g WHERE code < 120 UNION SELECT c FROM ca WHERE code >= 100 \
             ORDER BY 1",
            "query SELECT w FROM ca g UNION SELECT UPPER(w) FROM ca ORDER BY 1",
            // `DISTINCT` on a column a dynamic-collation order serves sorts
            // by the order's keys in the original engine.
            "query SELECT DISTINCT c FROM ger ORDER BY c",
            // A ROWVERSION variable shows as a BIGINT; a CICHAR one compares
            // regardless of case where the original's compares exactly; a
            // NULL STRING added to text is as long as the text (the
            // original's is one) (KNOWN_ISSUES.md).
            "query DECLARE x ROWVERSION; SELECT x AS v FROM system.iota",
            "query DECLARE x ROWVERSION; x = 5; SELECT x AS v FROM system.iota",
            "query DECLARE x CICHAR(3); x = 'aBc'; SELECT x AS v, x = 'ABC' AS e FROM system.iota",
            "query DECLARE x STRING; SELECT x + 'b' AS v FROM system.iota",
            // Two literals compare under a dynamic collation character by
            // character in the original engine, unlike columns.
            "query SELECT IIF('resumé' > 'resumes', 1, 0) AS a, IIF('äa' > 'aeb', 1, 0) AS b, \
             IIF('ä' > 'az', 1, 0) AS c, IIF('aé' > 'áe', 1, 0) AS d FROM system.iota",
            // An `ORDER BY` of the record number, or of a column given a
            // collation, is made an index of in the original engine, which
            // gives a live cursor (KNOWN_ISSUES.md).
            "query SELECT n FROM t ORDER BY RECNO() DESC",
            "query SELECT n FROM t ORDER BY s COLLATE ads_default_ci",
            // A `SELECT` of one table's columns in `FROM` is a live cursor
            // on that table in the original engine.
            "query SELECT t.cust FROM (SELECT cust FROM sales) t",
            "query SELECT * FROM (SELECT id, name FROM cust WHERE id > 2) AS t ORDER BY name DESC",
            // So is a view's of one table's columns (here a linked one's).
            "query SELECT * FROM v_link2",
            // A view of a derived table with an `ORDER BY` is refused (2115)
            // by the original; `CREATE VIEW` is not done (KNOWN_ISSUES.md).
            "ddview v_ord4 SELECT * FROM (SELECT id FROM indexed ORDER BY id) t",
            "direct CREATE VIEW v_sql_ord AS SELECT id, name FROM indexed ORDER BY name",
            "direct CREATE VIEW v_sql_ord AS SELECT TOP 2 id, name FROM indexed ORDER BY name",
            "direct CREATE VIEW v_sql_plain AS SELECT id, name FROM indexed",
            "direct SELECT * FROM v_sql_ord",
            // The text of an expression of a CICHARACTER column sorts
            // regardless of case in the original's index, not in tflip's;
            // and the original's `ORDER BY MOD()` sorts equal keys in
            // another order.
            "direct SELECT * FROM cust ORDER BY IFNULL(city, 'x')",
            "direct SELECT * FROM cust ORDER BY COALESCE(city, 'x')",
            "direct SELECT * FROM cust ORDER BY MOD(id, 2)",
            "direct SELECT DISTINCT city FROM cust ORDER BY city",
            "direct SELECT * FROM (SELECT id, name FROM cust) t ORDER BY name",
            // Past the keys of a reversed order, `gt` is the first key at or
            // below the next one's, not after (KNOWN_ISSUES.md).
            "idxseekx 1 gt double 3",
            // The scope of a temporary index (KNOWN_ISSUES.md).
            "idxscope 1 top string Bob",
            "idxscope 1 bottom string Dee",
            "idxscope 1 top double 2",
            "idxscope 1 bottom double 4",
            "direct SELECT * FROM v_from",
            "direct SELECT * FROM (SELECT id, name FROM v_sub) t",
            "direct SELECT * FROM (SELECT id, name FROM indexed) t",
            // The original engine's optimizer picks which table of a join
            // to loop over first, and so the order of the rows.
            "query SELECT c.name, s.item FROM cust c JOIN sales s ON c.id = s.cust \
             WHERE s.item = 'apple' ORDER BY c.name DESC",
            "query SELECT x.id, y.id FROM (SELECT id FROM cust WHERE id < 3) x, \
             (SELECT id FROM cust WHERE id > 3) y",
            // Rows with equal keys sorted in another order.
            "query SELECT c.name FROM cust c \
             ORDER BY (SELECT COUNT(*) FROM sales s WHERE s.cust = c.id) DESC",
            // Syntax errors worded or placed otherwise.
            "query SELECT COUNT(qty, price) FROM sales",
            "query SELECT c.name, s.item FROM {static} cust c JOIN sales s ON c.id = s.cust",
            "query SELECT name FROM cust WHERE id = SOME (SELECT cust FROM sales)",
            // The original engine silently leaves out rows with values too
            // large for their column, and then counts none.
            "query SELECT c_integer + 1, c_short * 2, c_longint - 1, c_numeric + 1, \
             c_double / 2, c_money * 2, c_curdouble + 0 FROM all_types",
            "query SELECT c_integer + c_numeric, c_integer + c_double, c_numeric * c_numeric, \
             c_money + c_integer FROM all_types",
            // tflip has no system tables to match.
            "query EXECUTE PROCEDURE sp_GetColumns( NULL, NULL, 'inde%', 'n%' )",
            // Group memberships and permission grants are encrypted; tflip
            // reads neither, so it lists only each user's DB:Public
            // membership and no grants (see KNOWN_ISSUES).
            "query SELECT * FROM system.usergroupmembers",
            "query SELECT * FROM system.permissions",
            "query SELECT * FROM system.effectivepermissions",
            // A missing system table: the original words 5004 without the
            // path and strips `system.`, as for any missing table.
            "query SELECT * FROM system.nosuch",
            // A non-administrator sees NULL for columns it may not read;
            // tflip does not enforce permissions and returns them.
            "query SELECT Table_Relative_Path FROM system.tables",
            "query SELECT Comment FROM system.users",
            // Syntax errors the original engine words or places otherwise.
            "expr DATE '2026-09-29'",
            "expr 1 != 1",
            "expr SUBSTRING('hello' FROM 2 FOR 3)",
            // A parenthesised column where a condition stands is pointed at
            // before its parenthesis, a subquery as one is not located, an
            // aggregate-less `HAVING` is accepted, and a variable that is no
            // condition in a script gets the script's information
            // (`fixtures/sql/gram_cast2.json`, KNOWN_ISSUES.md).
            "expr (SELECT 1 FROM system.iota) AND 1",
            "expr 1 AND (SELECT 0 FROM system.iota)",
            "query SELECT id FROM lg WHERE (n) AND b",
            "query SELECT id FROM lg WHERE (SELECT 1 FROM system.iota)",
            "query SELECT id FROM lg HAVING 1",
            "query DECLARE i INTEGER; i = 2; IF i THEN i = 10; ENDIF; SELECT i AS i FROM \
             system.iota",
            "query DECLARE i INTEGER; i = 2; IF i AND 1 THEN i = 10; ENDIF; SELECT i AS i FROM \
             system.iota",
            "query DECLARE i INTEGER; i = 2; IF i = 1 THEN i = 10; ELSEIF i THEN i = 20; ENDIF; \
             SELECT i AS i FROM system.iota",
            // Eight bytes cast to a BIGINT are an internal error in the
            // original (`docs/ADS_BUGS.md`), a ROWVERSION cast to bytes is
            // its text, and the CAST sizes that make a NUMERIC no field
            // (`fixtures/sql/gram_cast.json`, `gram_cast3.json`).
            "expr 1 << (0 + ABS(-2))",
            "expr CAST(x'4142434445464748' AS SQL_BIGINT)",
            "query SELECT CAST(rv AS SQL_BINARY) FROM bn",
            "expr CAST(1 AS SQL_CHAR(70000))",
            "expr CAST(1 AS SQL_NUMERIC(5,6))",
            "expr CAST(1 AS SQL_NUMERIC(0,0))",
            "expr CAST(1 AS CHARACTER VARYING)",
            // A `MERGE` plan names a column as the statement spells it, tflip
            // by the field's name (here a DBF table's, in capitals); and the
            // plan of a `SELECT` source with a `WHERE` is one scan of its
            // table, tflip's two (KNOWN_ISSUES.md).
            "query SHOW PLAN FOR MERGE ctgt t USING csrc s ON t.id = s.id WHEN MATCHED THEN \
             UPDATE SET val = s.val",
            "query SHOW PLAN FOR MERGE tgt t USING (SELECT id, val FROM src WHERE id > 1) s ON \
             t.id = s.id WHEN MATCHED THEN UPDATE SET val = s.val",
        ]
        .contains(&cmd)
    }

    /// Replays the `CONTAINS`/`SCORE` cases of `fixtures/sql/fts.json` through
    /// real SQL on the original engine's fixture tables (`fixtures/sql/fts`):
    /// the default index (docs), per-index parsing options (opts), a FIXED
    /// index (fixed), a field with no index (nofts), `*`, and their plans.
    #[test]
    fn fts_matches_the_original_engine() {
        let _settings = fixtures::settings_lock();
        let path = fixtures::dir("../sql/fts.json");
        let cases: Map<String, Json> =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        // A scratch copy, so the probe never touches the committed fixtures.
        let (source, _dir) = fixtures::scratch_from("sql-fts", &fixtures::dir("../sql/fts"));
        let mut connect = fixtures::connect_to(&source);
        let (mut stmt, mut cur) = (0, 0);
        unsafe { AdsCreateSQLStatement(connect, &mut stmt) };
        // The result rows as value-only tuples, sorted, so order does not matter.
        let tuples = |rows: &[Json]| -> Vec<Vec<Json>> {
            let mut out: Vec<Vec<Json>> = rows
                .iter()
                .map(|row| match row {
                    // The golden: a row is its column values.
                    Json::Array(values) => values.iter().map(clean).collect(),
                    other => vec![clean(other)],
                })
                .collect();
            out.sort_by_key(|r| r.iter().map(Json::to_string).collect::<Vec<_>>());
            out
        };
        // The plan rows as ordered value tuples (a plan's order matters).
        let plan_rows = |rows: &[Json]| -> Vec<Vec<Json>> {
            rows.iter()
                .map(|row| match row {
                    Json::Array(values) => values.iter().map(clean).collect(),
                    other => vec![clean(other)],
                })
                .collect()
        };
        let mut failures = Vec::new();
        for (sql, expected) in &cases {
            if sql.starts_with("SHOW PLAN") || sql.starts_with("show plan") {
                let got = run(&format!("query {sql}"), &mut connect, &mut stmt, &mut cur);
                let want = expected
                    .get("rows")
                    .and_then(Json::as_array)
                    .map(|r| plan_rows(r));
                let got_rows = got
                    .get("cursor")
                    .and_then(|c| c.get("rows"))
                    .and_then(Json::as_array)
                    .map(|rows| {
                        let values: Vec<Json> = rows
                            .iter()
                            .map(|r| r.get(1).cloned().unwrap_or(Json::Null))
                            .collect();
                        plan_rows(&values)
                    });
                // Column 4 is the plan number, which climbs per connection; the
                // goldens were each captured fresh, so compare it as the same.
                let blank_plan_number = |rows: &mut Option<Vec<Vec<Json>>>| {
                    for row in rows.iter_mut().flatten() {
                        if let Some(cell) = row.get_mut(4) {
                            *cell = json!("#");
                        }
                    }
                };
                let (mut want, mut got_rows) = (want, got_rows);
                blank_plan_number(&mut want);
                blank_plan_number(&mut got_rows);
                if want != got_rows {
                    failures.push(format!("{sql}:\n  want {want:?}\n  got  {got_rows:?}"));
                }
                continue;
            }
            let got = run(&format!("query {sql}"), &mut connect, &mut stmt, &mut cur);
            if let Some(code) = expected.get("error").and_then(Json::as_u64) {
                let got_error = got.get("error").and_then(Json::as_str).unwrap_or("");
                // The last error carries the native code: `NativeError = <code>`
                // as an SQL error, or the code itself where the original engine
                // raises it (setting a live cursor's AOF).
                if !got_error.contains(&format!("NativeError = {code};"))
                    && !got_error.starts_with(&format!("Error {code}:"))
                {
                    failures.push(format!("{sql}: want error {code}, got {got_error:?}"));
                }
                continue;
            }
            let want = expected
                .get("rows")
                .and_then(Json::as_array)
                .map(|r| tuples(r));
            let got_rows = got
                .get("cursor")
                .and_then(|c| c.get("rows"))
                .and_then(Json::as_array)
                // The cursor's rows are `[recno, [values]]`: take the values.
                .map(|rows| {
                    let values: Vec<Json> = rows
                        .iter()
                        .map(|r| r.get(1).cloned().unwrap_or(Json::Null))
                        .collect();
                    tuples(&values)
                });
            if want != got_rows {
                failures.push(format!(
                    "{sql}: want {:?}, got {:?} (error {:?})",
                    want,
                    got_rows,
                    got.get("error")
                ));
            }
        }
        AdsDisconnect(connect);
        assert!(
            failures.is_empty(),
            "{} FTS cases differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// A result value for comparison: a string without its trailing blanks, a
    /// null kept as null.
    fn clean(value: &Json) -> Json {
        match value {
            Json::String(s) => json!(s.trim_end()),
            other => other.clone(),
        }
    }

    remote_twins!(
        statements_match_the_original_engine,
        selects_match_the_original_engine,
        plans_match_the_original_engine,
        dml_matches_the_original_engine,
        dbf_matches_the_original_engine,
        full_text_plans_match_the_original_engine
    );
}

#[cfg(test)]
mod cursor_tests {
    use super::*;
    use crate::fixtures::{self, remote_twins};
    use crate::{AdsDisconnect, AdsGetIndexHandle, AdsOpenTable90};

    /// As in the original engine: an empty (or null) order name is a
    /// cursor's natural order, handle 0, but no order of a table.
    #[test]
    fn empty_order_names_on_cursors() {
        let connect = fixtures::connect_sql();
        let (mut stmt, mut cursor, mut table, mut index) = (0, 0, 0, 0x77);
        let null = std::ptr::null_mut();
        unsafe {
            assert_eq!(AdsCreateSQLStatement(connect, &mut stmt), AE_SUCCESS);
            let sql = c"SELECT UPPER(name) FROM indexed"
                .as_ptr()
                .cast_mut()
                .cast();
            assert_eq!(AdsExecuteSQLDirect(stmt, sql, &mut cursor), AE_SUCCESS);
            assert_eq!(AdsGetIndexHandle(cursor, null, &mut index), AE_SUCCESS);
            assert_eq!(index, 0);
            let name = c"indexed.adt".as_ptr().cast_mut().cast();
            let rc = AdsOpenTable90(connect, name, null, 3, 1, 1, 1, 4, null, &mut table);
            assert_eq!(rc, AE_SUCCESS);
            index = 0x77;
            assert_eq!(AdsGetIndexHandle(table, null, &mut index), 5016);
            assert_eq!(index, 0x77);
        }
        AdsCloseSQLStatement(stmt);
        AdsDisconnect(connect);
    }

    /// ARC's filter on its list of active queries, whose field names have
    /// spaces: the original engine takes them as one name.
    #[test]
    fn filters_on_procedure_results() {
        let connect = fixtures::connect_sql();
        let (mut stmt, mut cursor, mut count) = (0, 0, 0x77);
        unsafe {
            assert_eq!(AdsCreateSQLStatement(connect, &mut stmt), AE_SUCCESS);
            let sql = c"EXECUTE PROCEDURE sp_GetSQLStatements()"
                .as_ptr()
                .cast_mut()
                .cast();
            assert_eq!(AdsExecuteSQLDirect(stmt, sql, &mut cursor), AE_SUCCESS);
            let filter = c"(Query Number=0)".as_ptr().cast_mut().cast();
            assert_eq!(crate::AdsSetAOF(cursor, filter, 1), AE_SUCCESS);
            assert_eq!(crate::AdsGetRecordCount(cursor, 1, &mut count), AE_SUCCESS);
            assert_eq!(count, 0);
            let filter = c"Query  Number = 0".as_ptr().cast_mut().cast();
            assert_eq!(crate::AdsSetFilter(cursor, filter), 3106);
        }
        AdsCloseSQLStatement(stmt);
        AdsDisconnect(connect);
    }

    /// A server refuses a connection's 51st statement that ran and is not
    /// closed (7216, `fixtures/sql/txlimit.json`); until one is closed, or
    /// `sp_SetStatementLimit` raises the limit. The original's local server
    /// refuses none.
    #[test]
    fn statement_limit_on_a_server() {
        unsafe {
            let connect = fixtures::connect_sql();
            let remote = fixtures::is_remote();
            let sql = c"SELECT name FROM indexed".as_ptr().cast_mut().cast();
            let mut statements = Vec::new();
            let run = |stmt| {
                let mut cursor = 0;
                let rc = AdsExecuteSQLDirect(stmt, sql, &mut cursor);
                if rc == AE_SUCCESS {
                    crate::AdsCloseTable(cursor);
                }
                rc
            };
            for _ in 0..50 {
                let mut stmt = 0;
                assert_eq!(AdsCreateSQLStatement(connect, &mut stmt), AE_SUCCESS);
                assert_eq!(run(stmt), AE_SUCCESS);
                statements.push(stmt);
            }
            let mut stmt = 0;
            assert_eq!(AdsCreateSQLStatement(connect, &mut stmt), AE_SUCCESS);
            assert_eq!(run(stmt), if remote { 7216 } else { AE_SUCCESS });
            if remote {
                // One closed gives its place, and a statement that ran runs again.
                assert_eq!(AdsCloseSQLStatement(statements.remove(0)), AE_SUCCESS);
                assert_eq!(run(stmt), AE_SUCCESS);
                assert_eq!(run(statements[0]), AE_SUCCESS);
                let mut other = 0;
                assert_eq!(AdsCreateSQLStatement(connect, &mut other), AE_SUCCESS);
                assert_eq!(run(other), 7216);
                let limit = c"EXECUTE PROCEDURE sp_SetStatementLimit(0)";
                let mut limiter = 0;
                assert_eq!(AdsCreateSQLStatement(connect, &mut limiter), AE_SUCCESS);
                AdsCloseSQLStatement(statements.remove(0));
                let rc = AdsExecuteSQLDirect(
                    limiter,
                    limit.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                );
                assert_eq!(rc, AE_SUCCESS);
                assert_eq!(run(other), AE_SUCCESS);
            }
            AdsDisconnect(connect);
        }
    }

    remote_twins!(
        empty_order_names_on_cursors,
        filters_on_procedure_results,
        statement_limit_on_a_server
    );
}
