//! The TableflipDB engine: the state and logic behind the client library
//! (`tflip-ace`) and, later, the server (`tflipd`).
//!
//! Everything here is safe, plain Rust. Pointers and the ANSI code page
//! belong to the callers: `tflip-ace` translates between them and the types
//! used here. What ACE error code and text an [`Error`] is, which tflip-ace
//! and tflip-studio both report, is in [`ace_error`].
//!
//! Connections, table listings, tables, SQL statements and their cursors
//! are objects behind handles. See [`handle`]. Handles and settings belong
//! to a [`Session`]: the client library has one, the server one per client.

#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use handle::{FIRST_HANDLE, Handle, HandleTable};
use settings::Settings;
use shares::Shares;

pub mod ace_error;
pub mod aep;
mod backup;
#[cfg(feature = "test-hooks")]
pub use backup::active::pause_next_backup;
mod cache;
pub mod clock;
pub mod collations;
mod column_rights;
pub mod connection;
pub(crate) mod constraints;
pub mod copy;
pub mod ddl;
pub mod dictionary;
pub mod dictionary_write;
mod dml;
mod events;
pub mod filter;
pub mod find;
pub mod format;
pub mod getters;
pub mod handle;
pub mod index;
pub mod link;
mod login_lockout;
mod maintenance;
pub mod procedure;
pub mod psm;
mod query_log;
pub mod relation;
pub mod restructure;
pub mod ri;
mod script_host;
pub mod seek;
mod select_into;
mod sent;
pub mod setters;
pub mod settings;
mod shared;
pub mod shares;
pub mod statement;
mod subquery;
pub mod system;
pub mod system_collation;
pub mod sysvars;
pub mod table;
mod temp;
pub mod transaction;
pub mod trigger;
pub mod txlog;
pub mod udf;
mod view_columns;

/// Why an engine call failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Error {
    /// The handle is not a connection.
    InvalidConnection,
    /// The handle is unknown.
    InvalidHandle,
    /// The handle is not a table.
    InvalidTable,
    /// The handle is not an index order.
    InvalidIndex,
    /// The structural index, which opens with its table, cannot be closed.
    AutoOpenIndex,
    /// A table's orders have a dynamic collation other than the one it is
    /// opened with.
    CollationMismatch,
    /// A table has no index order of the given name.
    InvalidIndexOrderName,
    /// The drive a path names does not exist.
    NoDriveConnection,
    /// A file that must exist does not, such as a data dictionary to connect
    /// to.
    FileNotFound,
    /// Only remote servers were allowed, and tflip cannot connect to one yet.
    RemoteUnavailable,
    /// A table file is not a valid table of its type.
    CorruptTable,
    /// An index file could not be read.
    CorruptIndex,
    /// A table's structure is encrypted, and no password was given.
    TableEncrypted,
    /// The table handle has not given the encrypted table's password, which
    /// what it was asked to do needs (`AE_ENCRYPTION_NOT_ENABLED`).
    EncryptionNotEnabled,
    /// The record is encrypted, and the table handle has not given the
    /// password to change it (`AE_RECORD_ENCRYPTED`).
    RecordEncrypted,
    /// The password is not the encrypted table's (7060).
    InvalidPassword,
    /// A data dictionary encrypted with AES was opened without its
    /// dictionary password (7068).
    DictionaryEncrypted,
    /// A data dictionary's table opened as a free table (5159).
    DatabaseTable,
    /// A field number is not between 1 and the number of fields.
    InvalidFieldNumber,
    /// No field has the given name.
    InvalidFieldName,
    /// A record number is not a record of the table.
    InvalidRecordNumber,
    /// The table is not on a record (it is at BOF or EOF).
    NoCurrentRecord,
    /// The field's type does not support the operation.
    InvalidFieldType,
    /// A date format is not one the original engine accepts.
    InvalidDateFormat,
    /// A value is longer than its setting or field allows.
    DataTooLong,
    /// What was asked for does not exist, such as the key of a record an
    /// order has no key for.
    NotFound,
    /// A value is not one of the logical values the original engine knows
    /// (`T`, `F`, `Y`, `N`, ...), or an expression is empty.
    InvalidExpression,
    /// An expression cannot be compiled; the original engine reports why
    /// with a code that depends on the call (see
    /// [`tflip_expr::ErrorKind::code`]).
    Expression(tflip_expr::ErrorKind),
    /// Evaluating an expression failed on the record.
    Evaluation(tflip_expr::Runtime),
    /// An AOF's expression is not logical.
    AofNotLogical,
    /// A key is not as long as its type needs (a double is 8 bytes).
    InvalidKeyLength,
    /// A value cannot be converted to the type it is needed as, such as
    /// text that is not a number for a Numeric field.
    InvalidDataType,
    /// Text names a date that does not exist, or is not a date.
    InvalidDate,
    /// A number is too large for its type, or has more decimals than an
    /// integer type allows.
    ValueOverflow,
    /// Text is not a value of the type at all, such as letters for a
    /// RowVersion.
    InvalidData,
    /// The original engine can do it; tflip cannot yet.
    NotImplemented,
    /// An index order has no scope of the kind asked for.
    NoScope,
    /// A table has no filter.
    NoFilter,
    /// A filter option is not one the original engine knows.
    InvalidFilterOption,
    /// Connecting by path may only use a remote server, and none could be
    /// reached (the original engine finds none either).
    RemoteNotFound,
    /// The connection to the server broke; the client cannot use it any
    /// more.
    ConnectionLost,
    /// The server speaks another version of the protocol.
    ProtocolMismatch,
    /// A path leaves the server's shares, or names a share it does not have.
    FileNotOnServer,
    /// A path is empty or otherwise not one.
    InvalidPath,
    /// No connection has the file open.
    FileNotOpen,
    /// An SQL statement failed: ACE error 7200 with what the SQL engine
    /// says.
    Sql(tflip_sql::SqlError),
    /// A full-text search condition that does not parse, by its native error
    /// (7094–7103), which the original engine raises itself — not as an SQL
    /// error — when it sets a live cursor's `WHERE` as an AOF.
    FullText(u32),
    /// A read the server's file layer could not make (7010): the block a
    /// record's memo field names is past the end of the memo file.
    FileRead,
    /// The handle is not an SQL statement.
    InvalidStatement,
    /// The statement was not prepared before it was executed.
    NotPrepared,
    /// The cursor of the statement's last execution is still open.
    CursorNotClosed,
    /// SQL text is empty.
    EmptyStatement,
    /// A parameter number is not one of the statement's.
    InvalidParamNumber,
    /// A parameter name is not one of the statement's.
    InvalidParamName,
    /// A `SELECT` was executed without a place for its cursor.
    NoCursorHandle,
    /// A data dictionary could not be read.
    DictionaryUnreadable,
    /// The user may not connect to the data dictionary.
    LoginFailed,
    /// The user has no read right on a dictionary table, so it cannot be opened
    /// (7079); a dictionary that verifies access rights refuses.
    NoReadRights,
    /// The user lacks the right for the operation on a dictionary table (5054);
    /// a dictionary that verifies access rights refuses the insert/update/delete.
    PermissionDenied,
    /// A child row's foreign key has no matching parent key, so a referential
    /// integrity rule refuses the insert or update (7076 through the table API,
    /// wrapped as the AQE error 7200 through SQL). See [`crate::ri`].
    /// It carries the rule's own message for it (`ADS_DD_RI_NO_PKEY_ERROR`),
    /// which the error's text ends with.
    NoParentKey(String),
    /// A referential integrity rule set to RESTRICT refuses an update or delete
    /// of a parent row that still has children (5141 / 7200). See [`crate::ri`].
    /// It carries the rule's message for it (`ADS_DD_RI_CASCADE_ERROR`).
    RiRestrict(String),
    /// The data dictionary has no object of the name.
    InvalidObjectName,
    /// The data dictionary has no object of the kind asked for.
    NoObjectFound,
    /// A data dictionary object type is not one that can be asked for.
    InvalidObjectType,
    /// A data dictionary object does not have the property set.
    PropertyNotSet,
    /// A property is not one of the object's kind.
    InvalidPropertyId,
    /// An option, or a combination of options, the original engine does
    /// not take.
    InvalidOption,
    /// A temporary table was asked for without being created.
    TempTableRefused,
    /// Packing, zapping or reindexing a table not opened exclusively.
    NotExclusive,
    /// A field definition creating a table refuses, and why, as the
    /// original engine words it.
    InvalidFieldDefinition(String),
    /// A table file that cannot be created (it exists already).
    FileCreate,
    /// An index order's key expression that does not compile.
    KeyExpression(tflip_expr::ErrorKind),
    /// A record or table lock is held by another table handle.
    LockFailed,
    /// A record to unlock is not locked.
    RecordNotLocked,
    /// The table was opened read-only, or its file cannot be written.
    TableReadOnly,
    /// A value was too long for its field and was cut to fit; the shorter
    /// value was set.
    DataTruncated,
    /// A date's month does not exist.
    InvalidMonth,
    /// A date's day does not exist in its month, which has this many days.
    InvalidDay(u32),
    /// A value outside what the field takes (a time of a day or more, text
    /// that is not base64).
    InvalidValue,
    /// The table to unlock is not locked.
    TableNotLocked,
    /// A lock on a table opened exclusively.
    TableNotShared,
    /// The file is open exclusively, or open when it is to be opened
    /// exclusively.
    FileInUse,
    /// A record's key is one a unique index order has for another record:
    /// the order, as the original engine names it (`<index>:<TAG>`, the
    /// index an ADI file's name without its extension, a CDX or NTX file's
    /// with it).
    UniqueViolation(String),
    /// The operation is not one the table's type has (`AdsSetMoney` on a
    /// Clipper or FoxPro table).
    InvalidTableType,
    /// A DBF table opened as another type than its memo file needs: the
    /// type it needs.
    WrongTableType(table::TableType),
    /// Only a VFP field declared NULL can be set to NULL.
    NotVfpNullable,
    /// A transaction command outside a transaction (`AdsCommitTransaction`,
    /// ...).
    NotInTransaction,
    /// A rollback to a savepoint the transaction does not have.
    SavepointNotFound,
    /// A commit that released the lock of an appended record a rollback to a
    /// savepoint had released: it is done, and said to have failed (7138).
    LockLost,
    /// A command of a transaction that an external procedure's refused
    /// call spoiled (7207): it is done, and said to have failed
    /// ([`transaction::api_commit`]).
    AgentFailed,
    /// A command a transaction does not allow.
    IllegalInTransaction(transaction::Illegal),
    /// A transaction's log cannot be written (see [`txlog`]).
    TransactionLog,
    /// A crashed transaction could not be recovered (see [`txlog`]).
    RecoveryFailed,
    /// A relation would make a table its own descendant; `true` if the
    /// child's order is on the parent table itself.
    CyclicRelation(bool),
    /// A memo value cannot be read or rewritten.
    CorruptMemo,
    /// A key does not fit its order (a number too wide for an NTX key), so
    /// the record is not written.
    KeyOverflow,
    /// A memo block size a table cannot have.
    InvalidMemoBlockSize,
    /// A record given whole is not the table's length.
    InvalidRecordLength,
    /// The connection has no data dictionary.
    NoDictionary,
    /// `AdsContinue` without an `AdsLocate` before it.
    NoLocate,
    /// A key a custom order cannot take: one the record has in it already,
    /// or an order that is not custom.
    CustomKey,
    /// A field to delete or change that the table does not have, with why.
    NoSuchField(String),
    /// A restructure the table does not allow, with why: a field to delete
    /// that an order uses.
    RestructureFailed(String),
    /// An index file's keys were built under a different static collation than
    /// the connection uses (its collation-CRC in the file header differs). Kept
    /// last so adding it does not shift the wire discriminants of the others.
    IndexCollationMismatch,
    /// A timestamp's date that does not exist: one without a month or a day
    /// is not converted to a Julian day, any other to a negative one (as the
    /// original engine says, with `AE_INVALID_DATE`).
    JulianConversion { negative: bool },
    /// A data dictionary's constraint refuses a value or a record, or a
    /// trigger failed, or another refusal that has its own ACE error: the
    /// `code` (5147 NULL, 5148 minimum, 5149 maximum, 5150 record validation,
    /// 5177 trigger, 5208 a NULL key a candidate order refuses, 7015, 7017
    /// and 7108 an index file added to a table of another type) and the text
    /// the original engine gives it, which may be the dictionary's own
    /// message (see [`crate::constraints`]). Through SQL it is the AQE error
    /// 7200.
    Constraint { code: u32, text: String },
    /// An error with what the original engine adds to its text (the file an
    /// open did not find, ...), as calls give it to their clients (see
    /// [`Error::detailed`]); the engine's own callers see the error alone.
    Detailed(Box<Error>, String),
    /// There is no update in progress to cancel (5085).
    NoPendingUpdate,
    /// The connection has run as many statements as its limit allows, and
    /// has not closed them (7216, a server's).
    StatementLimit,
    /// A function that does not take a cursor handle was given one (5115).
    IsCursorHandle,
    /// An ACE error of this code, with the detail the original engine adds
    /// to its text: what a call of the dictionary API answers where the SQL
    /// procedure of the same name words it as 7200.
    Ace { code: u32, detail: String },
}

impl Error {
    /// This error with `detail`, what the original engine adds to its text.
    pub fn detailed(self, detail: impl Into<String>) -> Error {
        Error::Detailed(Box::new(self), detail.into())
    }

    /// The error without a detail [`Error::Detailed`] adds.
    pub fn plain(&self) -> &Error {
        match self {
            Error::Detailed(error, _) => error.plain(),
            error => error,
        }
    }
}

/// What the original engine adds to the text of [`Error::JulianConversion`].
pub fn julian_conversion_detail(negative: bool) -> &'static str {
    match negative {
        true => "Converted to negative julian date.",
        false => "Date won't convert to julian.",
    }
}

/// The objects a client has open and its settings. Handles are only valid
/// in the session that handed them out. A session is a handle on what it
/// holds: a clone is the same session, which a user-defined function keeps to
/// run its statements on, and what it has open closes with the last.
pub struct Session {
    data: Arc<SessionData>,
}

/// The sessions [`Session::serving`] made and that are still open.
static SERVED: Mutex<Vec<std::sync::Weak<SessionData>>> = Mutex::new(Vec::new());

/// What a [`Session`] holds.
pub struct SessionData {
    pub(crate) handles: Mutex<HandleTable>,
    pub(crate) settings: Arc<Settings>,
    /// The folders a server's session may reach; `None` for any path.
    pub(crate) shares: Option<Arc<Shares>>,
    /// The network address a server's client connected from, as the
    /// connection's `::conn.NetworkAddress` and its communication type say.
    pub(crate) client_address: Mutex<Option<String>>,
}

impl std::ops::Deref for Session {
    type Target = SessionData;

    fn deref(&self) -> &SessionData {
        &self.data
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

impl Clone for Session {
    fn clone(&self) -> Self {
        Session {
            data: Arc::clone(&self.data),
        }
    }
}

impl Drop for Session {
    /// Closes everything the session has open, as when a client goes away:
    /// what was pending is written, and transactions still running are
    /// rolled back.
    fn drop(&mut self) {
        if Arc::strong_count(&self.data) == 1 {
            connection::close_all(self);
        }
    }
}

impl Session {
    /// A session handing out handles from [`FIRST_HANDLE`] on.
    pub fn new() -> Self {
        Self::with_base(FIRST_HANDLE)
    }

    /// A session handing out handles from `base` on, so that the handles of
    /// several sessions one client uses do not collide.
    pub fn with_base(base: Handle) -> Self {
        Session::of(base, None)
    }

    fn of(base: Handle, shares: Option<Arc<Shares>>) -> Self {
        Session {
            data: Arc::new(SessionData {
                handles: Mutex::new(HandleTable::new(base)),
                settings: Arc::new(Settings::default()),
                shares,
                client_address: Mutex::new(None),
            }),
        }
    }

    /// A server's session for a client: handles from `base` on, and only
    /// `shares` reachable (see [`shares`]).
    pub fn serving(base: Handle, shares: Arc<Shares>) -> Self {
        let session = Session::of(base, Some(shares));
        let mut served = lock(&SERVED);
        served.retain(|other| other.strong_count() > 0);
        served.push(Arc::downgrade(&session.data));
        session
    }

    /// The sessions of the server's clients, which see one another's
    /// statements (`sp_GetSQLStatements`).
    pub(crate) fn served() -> Vec<Session> {
        let served = lock(&SERVED);
        let sessions = served.iter().filter_map(std::sync::Weak::upgrade);
        sessions.map(|data| Session { data }).collect()
    }

    /// What tells this session from the others while it lives.
    pub(crate) fn id(&self) -> usize {
        Arc::as_ptr(&self.data) as usize
    }

    /// Tells a server's session the address its client connected from.
    pub fn set_client_address(&self, address: String) {
        *lock(&self.client_address) = Some(address);
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// Locks `mutex`, carrying on even if another thread panicked while holding
/// it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
