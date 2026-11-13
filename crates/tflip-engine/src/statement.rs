//! SQL statements, and the cursors they return.
//!
//! A statement belongs to a connection and holds the SQL prepared on it,
//! the values of its parameters and the cursor of its last execution, which
//! must be closed before it runs again (as in the original engine). A
//! cursor is a table: a live one is the table the `SELECT` reads, showing
//! its columns, its `WHERE` a filter clients do not see and its `ORDER BY`
//! the order of its records; a static one is a table built in memory of the
//! results. Statements own their cursors: closing one closes them.
//!
//! What runs is [`tflip_sql`]'s subset of SQL; see there.

use std::cmp::Ordering;
use std::sync::atomic::{self, AtomicU32};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tflip_sql::ast::{Debug, Execute, Select, SourceRef, Statement as Sql};
use tflip_sql::{Env, NoRow, Node, Plan, SqlError, Ty, V};
use tflip_storage::build::{self, Column};
use tflip_storage::field::Field;
use tflip_storage::value::Value;

use crate::filter::{SqlFilter, SqlRow};
use crate::handle::{Handle, Object};
use crate::table::{self, TableSettings, TableType};
use crate::{Error, Session, connection, lock, procedure};

mod cursors;
mod environment;
mod fts;
mod handles;
mod procedures;
mod running;

pub(crate) use cursors::*;
pub(crate) use environment::*;
pub(crate) use fts::*;
pub use handles::*;
pub(crate) use procedures::*;
pub use running::*;

/// The open options a live cursor reports (`AdsGetTableOpenOptions`), as
/// the original engine's do.
const LIVE_OPTIONS: u32 = 0x1_00F0;

/// The open options of a static cursor.
const STATIC_OPTIONS: u32 = 0x1_0013;

/// The open options a live cursor of a read-only statement reports.
const READ_ONLY_OPTIONS: u32 = 0x1_0012;

/// The native code of an SQL error reading a record.
pub(crate) const READ_FAILED: u32 = 5081;

/// The SQL error for `error`, opening the table `name` for a statement:
/// a table of another type than the statement's (5025, its memo file of the
/// other kind) as the original engine words it; other errors as they are.
pub(crate) fn open_error(error: Error, name: &str) -> Error {
    match error {
        // A database trigger that cancelled the open (probed,
        // `fixtures/sql/trigdbopen.json`).
        Error::Constraint { code, text } if code == crate::constraints::TRIGGER_FAILED => {
            Error::Sql(SqlError::ace(
                code,
                &format!("{text} Table name: {name}"),
                "",
            ))
        }
        Error::Constraint { code, text }
            if code == table::MISSING_INDEX_FILE || code == table::ID_MISMATCH =>
        {
            Error::Sql(SqlError::ace(
                code,
                &format!("{text} Table name: {name}"),
                "",
            ))
        }
        Error::WrongTableType(needed) => Error::Sql(SqlError::ace(
            5025,
            "An invalid table type was specified.  Valid choices are ADS_ADT, ADS_VFP, \
             ADS_CDX, and ADS_NTX.  The table type must support the memo file type to be \
             opened, if applicable.",
            &format!("Should have been {}. Table name: {name}", needed.ace_name()),
        )),
        // The original engine names the table it may not read (probed,
        // `fixtures/sql/links.json`).
        Error::NoReadRights => Error::Sql(SqlError::ace(
            7079,
            &format!(
                "Cannot open table or view because the user does not have read rights to the \
                 table or view. Table name: {name}"
            ),
            "",
        )),
        // A table or view whose rights slot for DB:Public does not check
        // (`fixtures/sql/ddpublicbad`).
        Error::AgentFailed => Error::Sql(SqlError::ace(
            7207,
            &format!(
                "Unexpected error from Advantage Server Agent.  Check Advantage error log for \
                 details. Table name: {name}"
            ),
            "",
        )),
        // A free table the connection may not open
        // (`fixtures/dictionary/freetables.json`).
        Error::Ace { code: 7087, detail } => Error::Sql(SqlError::ace(
            7087,
            "Cannot open table or view for this operation because the requested permissions \
             have not been granted to the user.",
            &format!("{detail} Table name: {name}"),
        )),
        // An encrypted table of an AES dictionary connected without its
        // dictionary password (probed, `fixtures/aes/dict/aesdict3.json`).
        Error::DictionaryEncrypted => Error::Sql(SqlError::ace(
            7068,
            &format!(
                "The requested database operation is not allowed unless encryption was \
                 enabled. Table name: {name}"
            ),
            "",
        )),
        // An AES dictionary's table opened as a free table (probed,
        // `fixtures/write/encryption/freeopen.json`).
        Error::Detailed(error, file) if *error == Error::DictionaryEncrypted => {
            Error::Sql(SqlError::ace(
                7068,
                "The requested database operation is not allowed unless encryption was enabled.",
                &format!("{file} Table name: {name}"),
            ))
        }
        // A dictionary's table opened as a free table, as the original
        // engine words it (probed, `fixtures/write/encryption/freeopen.json`).
        Error::Detailed(error, file) if *error == Error::InvalidPassword => {
            Error::Sql(SqlError::ace(
                7060,
                "Invalid encryption password.  The specified password does not match the \
                 password being used in the table.",
                &format!("{file} Table name: {name}"),
            ))
        }
        // An encrypted free table opened with another password.
        Error::InvalidPassword => Error::Sql(SqlError::ace_bare(
            7060,
            &format!(
                "Invalid encryption password.  The specified password does not match the \
                 password being used in the table. Table name: {name}"
            ),
        )),
        // An encrypted free table opened without its password.
        Error::TableEncrypted => Error::Sql(SqlError::ace_bare(
            5098,
            &format!("The table is encrypted. Table name: {name}"),
        )),
        // A table the maintenance of another connection has closed (probed,
        // `fixtures/events/events_online.json`).
        // A dictionary's table is named without its file.
        Error::Detailed(error, file) if *error == Error::FileInUse => {
            let text = "The specified table, memo file, or index file was unable to be opened.";
            Error::Sql(match file.is_empty() {
                true => SqlError::ace(7008, &format!("{text} Table name: {name}"), ""),
                false => SqlError::ace(7008, text, &format!("{file} Table name: {name}")),
            })
        }
        error if *error.plain() == Error::DatabaseTable => Error::Sql(SqlError::ace(
            5159,
            "Error encountered when trying to open a database table.",
            &format!("{} Table name: {name}", table::DATABASE_TABLE),
        )),
        error => rights_sql_error(error),
    }
}

/// `table`, a table a statement reads, reporting as open options
/// (`AdsGetTableOpenOptions`) only the permissions its user holds on it, and
/// whether a dictionary's, as the original engine's live cursors do (probed,
/// `fixtures/sql/links.json`).
fn granted(mut table: table::Table) -> table::Table {
    use crate::system::right;
    use table::options;
    if table.index_files.is_some() {
        table.settings.options |= options::DICTIONARY_BOUND_TABLE;
    }
    for (right, permission) in [
        (right::UPDATE, options::PERM_UPDATE),
        (right::INSERT, options::PERM_INSERT),
        (right::DELETE, options::PERM_DELETE),
    ] {
        if table.access & right == 0 {
            table.settings.options &= !permission;
        }
    }
    table
}

/// Wraps an access-rights refusal so a statement reports it as an AQE error
/// (7200), the way the original engine does for SQL (probed,
/// `tools/ace/dd/rights.c`); a table-API call surfaces the raw 7079/5054. Other
/// errors pass through unchanged.
pub(crate) fn rights_sql_error(error: Error) -> Error {
    match error {
        Error::NoReadRights => Error::Sql(SqlError::ace(
            7079,
            "Cannot open table or view because the user does not have read rights to the \
             table or view.",
            "",
        )),
        Error::PermissionDenied => Error::Sql(SqlError::ace(
            5054,
            "The command cannot be completed with the current user permissions.",
            "",
        )),
        // A referential integrity refusal is the AQE error 7200 through SQL, as
        // it is for access rights (probed, `tools/ace/dd/ri.c`); the table API
        // surfaces the raw 7076 / 5141.
        Error::NoParentKey(message) => Error::Sql(SqlError::ace(
            7076,
            "A corresponding primary key value was not found for the foreign key value.",
            &message,
        )),
        Error::RiRestrict(message) => Error::Sql(SqlError::ace(
            5141,
            "The record update or delete failed because an existing referential integrity rule \
             is set to RESTRICT.",
            &message,
        )),
        // A dictionary's constraint or a failed trigger is the AQE error 7200
        // through SQL, its own code the native error (probed,
        // `tools/ace/dd/constraints.c`).
        Error::Constraint { code, text } => Error::Sql(SqlError::ace(code, &text, "")),
        error => error,
    }
}

/// The SQL error for a record that could not be read.
pub(crate) fn read_failed() -> SqlError {
    SqlError::ace(READ_FAILED, "Corrupt table", "")
}

/// A statement.
#[derive(Debug)]
pub struct Statement {
    /// The connection the statement runs on.
    pub connection: Handle,
    /// The statement's number in the process, as `sp_GetSQLStatements`
    /// reports it.
    pub number: u32,
    /// What `::stmt.Name` shows: its number, and a random identifier.
    pub(crate) name: String,
    state: Mutex<State>,
    /// What the statement last inserted or wrote (`LASTAUTOINC(STATEMENT)`,
    /// `LASTROWID()`, `LASTROWVERSION()`).
    last: Arc<Mutex<tflip_sql::Last>>,
}

#[derive(Default)]
struct State {
    /// The prepared SQL.
    sql: Option<String>,
    /// The prepared SQL's parameters.
    params: Vec<Param>,
    /// The values of the parameters beyond the SQL's own, up to the most any
    /// text prepared had: a later text with more parameters takes them up.
    spare: Vec<Option<ParamValue>>,
    /// The cursor of the last execution.
    cursor: Option<Handle>,
    /// What `AdsGetRecordCount` reports for the statement: the rows the last
    /// execution affected, or `u32::MAX` after a `SELECT`.
    affected: u32,
    /// The SQL that last ran, successfully or not, and when it started:
    /// milliseconds since midnight, local time.
    executed: Option<(String, u32)>,
    /// The settings of the tables it opens.
    settings: Settings,
    /// Property 39 (see [`Setting::ParseOnly`]) was set and no `AdsVerifySQL`
    /// has used it yet.
    parse_only_armed: bool,
    /// The text of the last `AdsVerifySQL` that checked its syntax alone:
    /// the same text again is not checked further either.
    parse_only_text: Option<String>,
    /// The files of the tables its last `SELECT` read, which it holds open
    /// until it closes or runs again (see [`crate::cache::Hold`]).
    held: Vec<crate::cache::Hold>,
    /// The tables its last run opened, as the database triggers saw them
    /// (see [`crate::trigger::Opened`]), which close when it runs other SQL
    /// or closes.
    opened: Vec<crate::trigger::Opened>,
    /// The temporary tables its last run read (see [`crate::temp::reading`]).
    temps_read: Vec<String>,
    /// The row of the query log whose end time goes in when its cursor
    /// closes.
    log_end: Option<crate::query_log::End>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("sql", &self.sql)
            .field("cursor", &self.cursor)
            .field("affected", &self.affected)
            .field("executed", &self.executed)
            .field("settings", &self.settings)
            .field("held", &self.held.len())
            .finish_non_exhaustive()
    }
}

thread_local! {
    /// What the statement running on the thread last wrote, which its
    /// expressions and writes share (see [`running`]).
    static LAST: std::cell::RefCell<Option<Arc<Mutex<tflip_sql::Last>>>> =
        const { std::cell::RefCell::new(None) };
}

thread_local! {
    /// The name of the statement running on the thread (`::stmt.Name`).
    static NAME: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The numbers of the statements running, which `sp_GetSQLStatements` calls
/// active. A statement that waits for an event is not one.
static ACTIVE: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Whether the statement numbered `number` is running.
pub(crate) fn is_active(number: u32) -> bool {
    lock(&ACTIVE).contains(&number)
}

/// Takes the statement numbered `number` out of those running, for as long
/// as it waits for an event (it is not active then).
pub(crate) fn idle(number: u32) {
    lock(&ACTIVE).retain(|&n| n != number);
}

/// Restores the statement that was running on the thread.
struct Running(Option<Arc<Mutex<tflip_sql::Last>>>, Option<String>, u32);

impl Drop for Running {
    fn drop(&mut self) {
        LAST.replace(self.0.take());
        NAME.replace(self.1.take());
        idle(self.2);
    }
}

/// Makes `statement` the one running on the thread, until the result is
/// dropped.
fn running(statement: &Statement) -> Running {
    lock(&ACTIVE).push(statement.number);
    Running(
        LAST.replace(Some(Arc::clone(&statement.last))),
        NAME.replace(Some(statement.name.clone())),
        statement.number,
    )
}

/// The name of the statement running on the thread, if one is.
pub(crate) fn running_name() -> Option<String> {
    NAME.with_borrow(Clone::clone)
}

/// What the statement running on the thread last wrote; a fresh one, which
/// nothing keeps, for expressions that run on no statement.
pub(crate) fn last() -> Arc<Mutex<tflip_sql::Last>> {
    LAST.with_borrow(|last| last.clone().unwrap_or_default())
}

thread_local! {
    /// The tables the `SELECT` running on the thread opens, collected for
    /// its statement to hold (see [`holding`]).
    static HOLDING: std::cell::RefCell<Option<Vec<crate::cache::Hold>>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs `f`, collecting the files of the tables it opens for a statement
/// to hold (see [`crate::cache::Hold`]).
fn holding<R>(f: impl FnOnce() -> R) -> (R, Vec<crate::cache::Hold>) {
    let outer = HOLDING.replace(Some(Vec::new()));
    let result = f();
    let held = HOLDING.replace(outer).unwrap_or_default();
    (result, held)
}

/// Has the statement running on the thread, if one collects them (see
/// [`holding`]), hold `table`'s file until it closes or runs again; else
/// lets its connection keep it open after it closes, if it keeps
/// statements' tables (see [`crate::cache`]).
fn hold(session: &Session, table: &table::Table) {
    let held = crate::cache::holds(session)
        && HOLDING.with_borrow_mut(|holding| match holding {
            Some(holding) => {
                holding.extend(table.hold());
                true
            }
            None => false,
        });
    if !held {
        table.keep_open_as(crate::cache::Kind::Cursors);
    }
}

/// The tables the statements of `session` hold open after their cursors
/// closed (see [`crate::cache::Hold`]).
pub(crate) fn held(session: &Session) -> Vec<(std::path::PathBuf, TableType)> {
    let mut tables = Vec::new();
    session.for_each(|object| {
        if let Object::Statement(statement) = object {
            for hold in &statement.state().held {
                let (path, table_type) = hold.table();
                tables.push((path.to_owned(), table_type));
            }
        }
    });
    tables
}

/// The statement `handle` is closing, or its connection: its cursor's table
/// closes, then the tables it opened, for the database triggers.
pub(crate) fn release(session: &Session, handle: Handle) {
    let Ok(statement) = get(session, handle) else {
        return;
    };
    for cursor in session.owned(handle, |object| matches!(object, Object::Table(_))) {
        let _ = crate::trigger::table_closed(session, statement.connection, cursor);
    }
    let opened = std::mem::take(&mut statement.state().opened);
    crate::trigger::close_all(session, opened);
}

/// The settings of the free tables a statement opens and creates, as the
/// `AdsStmtSet*` functions give them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The type its tables are opened and created as: a name without an
    /// extension is the type's file (`.adt`, `.dbf`).
    pub table_type: TableType,
    /// Their character type (`ADS_ANSI`, `ADS_OEM`).
    pub char_type: u16,
    /// Their locking (`ADS_PROPRIETARY_LOCKING`, `ADS_COMPATIBLE_LOCKING`);
    /// kept, not used.
    pub lock_type: u16,
    /// Whether its live cursors are read-only.
    pub read_only: bool,
    /// Whether what their writes change or append must meet the cursor's
    /// `WHERE` (`ADS_CONSTRAIN`).
    pub constrain: bool,
    /// Their collation; empty for the connection's.
    pub collation: String,
    /// Whether rights are checked (`ADS_CHECKRIGHTS`, `ADS_IGNORERIGHTS`).
    pub rights: u16,
    /// The passwords `AdsStmtSetTablePassword` gave, by table name in lower
    /// case.
    pub passwords: Vec<(String, String)>,
    /// What `AdsStmtEnableEncryption` last gave; kept and used by nothing, as
    /// the original's statements encrypt no table they create or write
    /// (`fixtures/write/encryption/stmtpw`).
    pub encryption: Option<String>,
}

impl Default for Settings {
    /// ADT tables, ANSI, proprietary locking, read and write, checking
    /// rights.
    fn default() -> Self {
        Settings {
            table_type: TableType::Adt,
            char_type: 1,
            lock_type: 1,
            read_only: false,
            constrain: false,
            collation: String::new(),
            rights: 1,
            passwords: Vec::new(),
            encryption: None,
        }
    }
}

impl Settings {
    /// How a table it opens with the open `options` is opened.
    pub(crate) fn table(&self, options: u32) -> TableSettings {
        TableSettings {
            table_type: self.table_type,
            char_type: self.char_type,
            collation: self.collation.clone(),
            options,
            rights: self.rights,
            ..TableSettings::default()
        }
    }

    /// How the SQL of the statement opens the table `name` with the open
    /// `options`: as [`Settings::table`], with the password the statement was
    /// given for it, and refused without one an encrypted table has, which
    /// the system procedures that open one are not.
    /// A name matches whatever its case, and only as it is written: with an
    /// extension or a path it does not.
    pub(crate) fn table_named(&self, options: u32, name: &str) -> TableSettings {
        let mut settings = TableSettings {
            by_statement: true,
            ..self.table(options)
        };
        if let Some((_, password)) = self
            .passwords
            .iter()
            .find(|(table, _)| table.eq_ignore_ascii_case(name))
        {
            settings.password.clone_from(password);
        }
        settings
    }

    /// The extension of a table it names without one.
    pub(crate) fn extension(&self) -> &'static str {
        match self.table_type {
            TableType::Adt | TableType::Database => "adt",
            TableType::Ntx | TableType::Cdx | TableType::Vfp => "dbf",
        }
    }
}

/// One of a statement's [`Settings`], as an `AdsStmtSet*` function sets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Setting {
    TableType(TableType),
    CharType(u16),
    LockType(u16),
    ReadOnly(bool),
    Constrain(bool),
    /// Whether live cursors read all columns: accepted, as it makes no
    /// difference to a local server.
    ReadAllColumns(bool),
    Collation(String),
    Rights(u16),
    /// The password of the table `.0`, which a later one for it replaces.
    TablePassword(String, String),
    /// Forget the passwords of all tables.
    ClearTablePasswords,
    /// The password `AdsStmtEnableEncryption` gave, or none after
    /// `AdsStmtDisableEncryption`.
    Encryption(Option<String>),
    /// The next `AdsVerifySQL` checks the syntax alone, whatever the value
    /// of the unpublished property 39 that asks for it.
    ParseOnly,
}

#[derive(Debug, Clone)]
pub(crate) struct Param {
    name: Option<String>,
    value: Option<ParamValue>,
}

/// A parameter's value, as a client sets it (`AdsSetString`, ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ParamValue {
    Text(String),
    Integer(i64),
    Double(f64),
    Logical(bool),
    /// A date as text in the connection's date format (`AdsSetDate`).
    DateText(String),
    /// A Julian day.
    Date(i32),
    /// A Julian day and milliseconds (`AdsSetTimeStampRaw`).
    TimeStamp(i32, u32),
    /// × 10,000.
    Money(i64),
    Binary(Vec<u8>),
    Null,
    /// `AdsSetEmpty`: NULL.
    Empty,
}

/// How a client names a parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamRef {
    /// 1-based.
    Number(usize),
    /// Without its `:`.
    Name(String),
}

impl Statement {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// [`Error::CursorNotClosed`] if the last execution's cursor is open.
    fn check_cursor(&self, session: &Session) -> Result<(), Error> {
        let cursor = self.state().cursor;
        match cursor {
            Some(cursor) if session.with(cursor, |_| ()).is_some() => Err(Error::CursorNotClosed),
            _ => Ok(()),
        }
    }
}
