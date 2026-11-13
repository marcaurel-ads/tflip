//! Connections.
//!
//! How connecting behaves follows what clients see from the original engine's
//! local server: the folder is not checked, a drive that does not exist is
//! an error, a path ending in `.add` names a data dictionary that must exist
//! and that the user may connect to (see [`crate::dictionary`]), and
//! disconnecting handle 0 closes every connection.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use serde::{Deserialize, Serialize};

use crate::dictionary::Database;
use crate::handle::{Handle, Object};
use crate::settings::{self, Settings};
use crate::{Error, Session, lock};

/// The `ADS_LOCAL_SERVER` bit of a set of server types.
pub const LOCAL: u16 = 1;
/// The `ADS_REMOTE_SERVER` bit.
pub const REMOTE: u16 = 2;
/// The `ADS_AIS_SERVER` bit (Advantage Internet Server).
pub const AIS: u16 = 4;

/// The kind of server behind a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerType {
    /// tflip's own engine, working on the files directly.
    Local,
}

/// An open connection.
#[derive(Debug)]
pub struct Connection {
    /// The `Data Source`: a folder of free tables, or a data dictionary.
    pub path: PathBuf,
    pub server_type: ServerType,
    /// Whether `path` names a data dictionary (`.add`).
    pub dictionary: bool,
    /// The data dictionary, for a connection to one. Behind a lock because
    /// writing to the dictionary (e.g. `AdsDDAddTable`) reloads it, so the
    /// connection sees its own changes.
    database: RwLock<Option<Database>>,
    /// What the dictionary's files were when `database` was read, so that
    /// another connection's change to it is read again (see [`stamp`]).
    stamp: Mutex<Stamp>,
    /// Where the connection's copy of the dictionary has each trigger, which
    /// is the order the triggers of a table fire in (see [`TriggerOrder`]).
    trigger_order: Mutex<crate::trigger::order::TriggerOrder>,
    /// The connection's own date format (`AdsSetDateFormat60`), if it has
    /// one; else it uses its session's.
    date_format: Mutex<Option<String>>,
    /// The settings of the session the connection belongs to.
    pub(crate) settings: Arc<Settings>,
    /// The connection's "exact" setting (see [`settings::exact`]), which
    /// tables opened on it start with.
    pub(crate) exact: AtomicBool,
    /// The application ID (`sp_SetApplicationID`).
    pub(crate) application_id: Mutex<String>,
    /// The number of SQL statements the connection may have open
    /// (`sp_SetStatementLimit`), 50 by default; 0 or less for no limit.
    statement_limit: std::sync::atomic::AtomicI32,
    /// The AutoInc value an `INSERT` on the connection last generated
    /// (`LASTAUTOINC(CONNECTION)`).
    pub(crate) last_autoinc: Arc<Mutex<Option<i32>>>,
    /// Whether the connection is an SQL debugger (`DEBUG BEGIN`).
    pub(crate) debugging: AtomicBool,
    /// The number of the last plan `SHOW PLAN FOR` made on the connection.
    pub(crate) plans: std::sync::atomic::AtomicU32,
    /// The collation tables opened on the connection without one take
    /// (`AdsSetCollation`); empty for none.
    collation: Mutex<String>,
    /// The user the connection was opened as, if one was given.
    user: Option<String>,
    /// Its transaction.
    pub(crate) transaction: Mutex<crate::transaction::State>,
    /// SQL's autocommit mode on it (`SET TRANSACTION`).
    pub(crate) autocommit: Mutex<crate::transaction::Autocommit>,
    /// The stored procedures running on it, innermost last (see
    /// [`crate::transaction::call_begins`]).
    pub(crate) calls: Mutex<Vec<crate::transaction::Call>>,
    /// Whether the connection is running in a privileged context, in which
    /// access rights are not enforced: set while a trigger fires, whose body
    /// runs with the table owner's rights, not the connected user's.
    privileged: AtomicBool,
    /// The data-encryption type for tables created on the connection, from the
    /// connect string's `EncryptionType`: 0 for none (RC4 is the default a table
    /// then gets from `AdsEnableEncryption`), or AES-128/AES-256
    /// ([`tflip_storage::adt::ENCRYPTION_AES128`]/`ENCRYPTION_AES256`), in which
    /// case a created table is laid out for AES and enabling encryption on it
    /// uses AES.
    encryption_type: AtomicU8,
    /// The connect string's `TableType`: what the free-table backup opens a
    /// file as when its call maps no type to the file's extension.
    table_type: Mutex<crate::table::TableType>,
    /// Whether unique keys are enforced on the connection's writes
    /// (`AdsDisableUniqueEnforcement`/`AdsEnableUniqueEnforcement`). On by
    /// default; off lets a write add a key an ADI-unique or CDX-candidate order
    /// already has, instead of refusing it (`AE_KEY_NOT_UNIQUE`).
    unique_enforcement: AtomicBool,
    /// Whether AutoInc fields are filled by the engine on the connection's
    /// writes (`AdsDisableAutoIncEnforcement`/`AdsEnableAutoIncEnforcement`). On
    /// by default; off lets a client set an AutoInc field to its own value,
    /// which is stored instead of the engine's next counter value.
    autoinc_enforcement: AtomicBool,
    /// Whether the connection may not open free tables, unless its user is
    /// an administrator (`CheckFreeTableAccess`, the connect option
    /// `ADS_CHECK_FREE_TABLE_ACCESS`).
    check_free_tables: AtomicBool,
    /// The server's `Check_Free_Table_Access`, as it was when the connection
    /// was made.
    server_checks_free_tables: bool,
    /// SQL `RAND()`'s generator: each connection has its own, as in the
    /// original engine, which a new statement on it goes on with.
    pub(crate) rand: Arc<Mutex<tflip_sql::Rand>>,
    /// The links the connection made for itself (`AdsDDCreateLink` without
    /// `ADS_LINK_GLOBAL`), which no other connection sees.
    pub(crate) links: Mutex<Vec<crate::link::Local>>,
    /// The connections to linked dictionaries opened for the connection's
    /// statements, by link name (or dictionary path) in upper case; they
    /// close with it.
    pub(crate) linked: Mutex<Vec<(String, Handle)>>,
    /// How many tables the connection keeps open after closing them
    /// (`AdsCacheOpenTables`; 0, none, by default) and of how many SQL
    /// statements it keeps the tables open (`AdsCacheOpenCursors`; 25 by
    /// default on a server, none in a client's own process unless its
    /// caching is on, see [`settings::set_local_caching`]): see
    /// [`crate::cache`].
    cached_tables: std::sync::atomic::AtomicU16,
    cached_cursors: std::sync::atomic::AtomicU16,
    /// The links the connection has active, newest first
    /// (`AdsGetNumActiveLinks`; see `crate::link`).
    pub(crate) active_links: Mutex<Vec<crate::link::Active>>,
    /// Whether the connection has disabled the dictionary's triggers for
    /// itself (`sp_DisableTriggers` for no other user).
    pub(crate) triggers_disabled: AtomicBool,
    /// The temporary tables (`#name`) it made, which go when it closes.
    pub(crate) temps: crate::temp::Temps,
    /// Its number among the connections of the process, which
    /// `::conn.Name` shows (the first is 2).
    pub(crate) number: u32,
    /// How many statements it has run (`::conn.OperationCount`).
    pub(crate) operations: std::sync::atomic::AtomicU32,
    /// Its number as a user, which a database trigger's `__info` shows
    /// (`UserID`): the lowest of the connections open that is free.
    pub(crate) user_id: u32,
    /// How many statements it has run, those of triggers and procedures too
    /// (`StmtCount`).
    pub(crate) statements: std::sync::atomic::AtomicU32,
    /// The tables a client opened on it, as its database triggers saw them
    /// open (see [`crate::trigger::attach`]), by table handle.
    pub(crate) table_opens: Mutex<Vec<(Handle, crate::trigger::Opened)>>,
    /// The tables the triggers of a table opened for their statements, by
    /// the table's name in lower case (see [`crate::trigger::in_trigger`]).
    pub(crate) trigger_opens: Mutex<Vec<(String, crate::trigger::Opened)>>,
}

/// The lowest user number no connection of `session` has: the first is 2,
/// and a number that closed is the next connection's (probed,
/// `tools/ace/sql/trigdblog.txt`).
fn free_user_id(session: &Session) -> u32 {
    let taken: Vec<u32> = all(session).iter().map(|c| c.user_id).collect();
    (2..).find(|id| !taken.contains(id)).unwrap_or(2)
}

/// The counter connections are numbered after.
static CONNECTIONS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

impl Drop for Connection {
    /// Closes the files the connection kept open.
    fn drop(&mut self) {
        crate::cache::close(self, None);
    }
}

/// The data-encryption type a connect string's `EncryptionType` asks for: an
/// AES type for `AES128`/`AES256`, else 0 (none; RC4 remains the default
/// `AdsEnableEncryption` applies). Unknown values are ignored (0), as the engine
/// treats an absent one.
/// The name of the operating system's user running the process, as the
/// original names the user of a connection in its lists and in
/// `::conn.OSUserLoginName`.
pub(crate) fn os_user() -> String {
    ["USERNAME", "USER", "LOGNAME"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
        .unwrap_or_default()
}

pub fn encryption_type(value: Option<&str>) -> u8 {
    match value.map(str::trim) {
        Some(v) if v.eq_ignore_ascii_case("AES128") => tflip_storage::adt::ENCRYPTION_AES128,
        Some(v) if v.eq_ignore_ascii_case("AES256") => tflip_storage::adt::ENCRYPTION_AES256,
        _ => 0,
    }
}

impl Connection {
    /// The data-encryption type of the tables created on the connection (see
    /// the field).
    pub(crate) fn encryption_type(&self) -> u8 {
        self.encryption_type.load(Ordering::Relaxed)
    }

    /// The table type the connection's connect string named (ADT without one).
    pub(crate) fn table_type(&self) -> crate::table::TableType {
        *lock(&self.table_type)
    }

    /// Whether the connection is to a data dictionary.
    pub(crate) fn has_database(&self) -> bool {
        self.database
            .read()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    /// Whether the connection runs privileged (access rights not enforced).
    pub(crate) fn privileged(&self) -> bool {
        self.privileged.load(Ordering::Relaxed)
    }

    /// Whether the connection's user is refused free tables: the connection
    /// asked for that, and the user is neither the administrator nor of
    /// `DB:Admin` (`fixtures/dictionary/freetables.json`).
    pub(crate) fn refuses_free_tables(&self) -> bool {
        self.check_free_tables.load(Ordering::Relaxed)
            && !self
                .with_database_opt(|database| database.is_admin() || database.is_admin_member())
                .unwrap_or(true)
    }

    /// Whether opening a free table is refused on the connection: as
    /// [`Connection::refuses_free_tables`] has it, or the server's
    /// `Check_Free_Table_Access` is on, which binds every user of a
    /// dictionary connection (`fixtures/events/freeconf_check.json`). Making
    /// and dropping free tables are not opening.
    pub(crate) fn refuses_free_table_open(&self) -> bool {
        self.refuses_free_tables() || (self.server_checks_free_tables && self.has_database())
    }

    /// Whether unique keys are enforced on the connection's writes.
    pub(crate) fn unique_enforcement(&self) -> bool {
        self.unique_enforcement.load(Ordering::Relaxed)
    }

    /// Whether the engine fills AutoInc fields on the connection's writes.
    pub(crate) fn autoinc_enforcement(&self) -> bool {
        self.autoinc_enforcement.load(Ordering::Relaxed)
    }

    /// Sets whether unique keys are enforced (`AdsDisableUniqueEnforcement`
    /// disables, `AdsEnableUniqueEnforcement` enables).
    pub(crate) fn set_unique_enforcement(&self, on: bool) {
        self.unique_enforcement.store(on, Ordering::Relaxed);
    }

    /// Sets whether the engine fills AutoInc fields
    /// (`AdsDisableAutoIncEnforcement` disables, `AdsEnableAutoIncEnforcement`
    /// enables).
    pub(crate) fn set_autoinc_enforcement(&self, on: bool) {
        self.autoinc_enforcement.store(on, Ordering::Relaxed);
    }

    /// How many tables (or statements' tables) the connection keeps open
    /// after closing them (see [`crate::cache`]).
    pub(crate) fn cache_size(&self, kind: crate::cache::Kind) -> u16 {
        match kind {
            crate::cache::Kind::Tables => self.cached_tables.load(Ordering::Relaxed),
            crate::cache::Kind::Cursors => self.cached_cursors.load(Ordering::Relaxed),
        }
    }

    /// Sets how many tables (`cursors` false) or SQL statements' tables the
    /// connection keeps open after closing them. The tables it keeps past a
    /// new table size close at once. The statements' tables past a new size
    /// close at once in a client's own process (`local`), as with the
    /// original's local server; on a server they stay until the next
    /// statement closes, as on the original's, which keeps them open even
    /// when the size drops to 0.
    pub(crate) fn set_cache_size(&self, cursors: bool, size: u16, local: bool) {
        let (count, kind) = match cursors {
            false => (&self.cached_tables, crate::cache::Kind::Tables),
            true => (&self.cached_cursors, crate::cache::Kind::Cursors),
        };
        count.store(size, Ordering::Relaxed);
        if !cursors || local {
            crate::cache::trim(self, kind, size);
        }
    }

    /// Runs `f` with the connection privileged, so access rights are not
    /// enforced for the tables it opens (a trigger body runs with the table
    /// owner's rights). Restores the previous state after, so firings nest.
    pub(crate) fn as_owner<R>(&self, f: impl FnOnce() -> R) -> R {
        let previous = self.privileged.swap(true, Ordering::Relaxed);
        let result = f();
        self.privileged.store(previous, Ordering::Relaxed);
        result
    }

    /// Runs `f` on the connection's data dictionary;
    /// [`Error::InvalidConnection`] for a connection to a folder.
    pub fn with_database<R>(
        &self,
        f: impl FnOnce(&Database) -> Result<R, Error>,
    ) -> Result<R, Error> {
        self.refresh_database();
        let guard = self.database.read().unwrap_or_else(|e| e.into_inner());
        f(guard.as_ref().ok_or(Error::InvalidConnection)?)
    }

    /// Runs `f` on the connection's data dictionary, or `None` for a folder.
    pub(crate) fn with_database_opt<R>(&self, f: impl FnOnce(&Database) -> R) -> Option<R> {
        self.refresh_database();
        let guard = self.database.read().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map(f)
    }

    /// Reads the dictionary again if another connection (or program) changed
    /// it since: the original's server has one dictionary for all of them.
    fn refresh_database(&self) {
        if self.dictionary && stamp(&self.path) != *lock(&self.stamp) {
            // A dictionary caught in the middle of a write is read at the
            // next call. What another connection changed is read as a whole,
            // its triggers in the order of its records.
            lock(&self.trigger_order).forget();
            let _ = self.reload_database();
        }
    }

    /// The ids of the dictionary's triggers in the order its copy of them
    /// fires them in, newest first (see [`crate::trigger::order`]); `dict`
    /// is the dictionary as it is now.
    pub(crate) fn trigger_sequence(&self, dict: &tflip_storage::dd::Dictionary) -> Vec<u32> {
        lock(&self.trigger_order).sequence(dict)
    }

    /// Takes what changed in the dictionary's triggers into the order, before
    /// the connection changes them: the order's first look at a dictionary
    /// is the one its triggers are loaded as.
    pub(crate) fn trigger_order_sync(&self) {
        self.refresh_database();
        let guard = self.database.read().unwrap_or_else(|e| e.into_inner());
        if let Some(database) = guard.as_ref() {
            lock(&self.trigger_order).sequence(database.dictionary());
        }
    }

    /// Notes that triggers of the dictionary fired.
    pub(crate) fn trigger_order_fired(&self) {
        lock(&self.trigger_order).fired();
    }

    /// Notes that the trigger `id` was altered in the dictionary.
    pub(crate) fn trigger_order_altered(&self, id: u32) {
        let guard = self.database.read().unwrap_or_else(|e| e.into_inner());
        if let Some(database) = guard.as_ref() {
            let mut order = lock(&self.trigger_order);
            order.sequence(database.dictionary());
            order.altered(id);
        }
    }

    /// Reloads the connection's data dictionary from disk (after a write to
    /// it), logging in as the connection's user again.
    pub(crate) fn reload_database(&self) -> Result<(), Error> {
        let read = stamp(&self.path);
        // Carry over the passwords the connection opened the dictionary and
        // logged in with, so the reload reads it and re-authenticates as the
        // same user, recovering the dictionary key.
        let (password, dd_password, opened_type) = {
            let guard = self.database.read().unwrap_or_else(|e| e.into_inner());
            guard
                .as_ref()
                .map(|db| {
                    (
                        db.password().to_vec(),
                        db.dd_password().map(<[u8]>::to_vec),
                        db.opened_type(),
                    )
                })
                .unwrap_or_default()
        };
        let mut database = Database::open_with_password(&self.path, dd_password.as_deref())?;
        database.keep_opened_type(opened_type.or(database.opened_type()));
        let key = database.login(self.user.as_deref(), &password)?;
        database.set_identity(self.user.as_deref(), key);
        database.set_password(&password);
        *self.database.write().unwrap_or_else(|e| e.into_inner()) = Some(database);
        *lock(&self.stamp) = read;
        Ok(())
    }

    /// Records the password the connection's user changed to, which a reload
    /// logs in with.
    pub(crate) fn set_database_password(&self, password: &[u8]) {
        let mut guard = self.database.write().unwrap_or_else(|e| e.into_inner());
        if let Some(database) = guard.as_mut() {
            database.set_password(password);
        }
    }

    /// The folder the connection's tables are in: its path, or the folder
    /// of its data dictionary.
    pub fn folder(&self) -> PathBuf {
        match self.dictionary {
            true => self
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            false => self.path.clone(),
        }
    }

    /// The collation tables opened on the connection without one take;
    /// empty for none.
    pub fn collation(&self) -> String {
        lock(&self.collation).clone()
    }

    /// The user the connection logged in as, if one was given (a dictionary
    /// login); `None` for a free-table connection with no user.
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Sets the collation tables opened on the connection without one take.
    pub fn set_collation(&self, collation: &str) {
        *lock(&self.collation) = collation.to_owned();
    }

    /// The epoch of years of two digits: its session's.
    pub fn epoch(&self) -> u16 {
        self.settings.epoch()
    }

    /// The connection's date format: its own, or its session's.
    pub fn date_format(&self) -> String {
        lock(&self.date_format)
            .clone()
            .unwrap_or_else(|| self.settings.date_format())
    }
}

/// What does not change about an open connection, for a client to keep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionInfo {
    /// The `Data Source`, as the server names it.
    pub path: String,
    pub server_type: ServerType,
    /// Whether `path` names a data dictionary.
    pub dictionary: bool,
    /// The user the connection was opened as, if one was given.
    pub user: Option<String>,
}

impl Connection {
    /// What does not change about the connection while it is open.
    pub fn info(&self) -> ConnectionInfo {
        ConnectionInfo {
            path: self.path.to_string_lossy().into_owned(),
            server_type: self.server_type,
            dictionary: self.dictionary,
            user: self.user.clone(),
        }
    }
}

/// The server types a connect string's `ServerType` allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ServerTypes {
    local: bool,
    remote: bool,
    ais: bool,
}

/// The value of `key` in a connect string such as
/// `Data Source=C:\data\;ServerType=LOCAL;`. Keys are case-insensitive.
pub fn connect_string_value<'a>(connect_string: &'a str, key: &str) -> Option<&'a str> {
    connect_string.split(';').find_map(|item| {
        let (k, v) = item.split_once('=')?;
        k.trim().eq_ignore_ascii_case(key).then(|| v.trim())
    })
}

/// The `ADS_*_SERVER` bits of the server types a connect string's
/// `ServerType` allows: names (`LOCAL`, `REMOTE`, `AIS`) or their numbers,
/// joined by `|`. Unknown parts are ignored.
pub fn server_type_bits(value: &str) -> u16 {
    value
        .split('|')
        .map(str::trim)
        .map(|part| match part.parse::<u16>() {
            Ok(bits) => bits,
            Err(_) if part.eq_ignore_ascii_case("local") => LOCAL,
            Err(_) if part.eq_ignore_ascii_case("remote") => REMOTE,
            Err(_) if part.eq_ignore_ascii_case("ais") => AIS,
            Err(_) => 0,
        })
        .fold(0, |bits, part| bits | part)
}

/// The server types `ServerType` allows.
fn server_types(value: &str) -> ServerTypes {
    let bits = server_type_bits(value);
    ServerTypes {
        local: bits & LOCAL != 0,
        remote: bits & REMOTE != 0,
        ais: bits & AIS != 0,
    }
}

/// The server to connect to. Without a `ServerType`, the original engine
/// tries a remote server first and falls back to the local one; tflip has no
/// remote server yet, so that ends up local too. Only remote or AIS servers
/// fail.
fn choose_server(server_type: Option<&str>) -> Result<ServerType, Error> {
    match server_type.filter(|s| !s.is_empty()).map(server_types) {
        None => Ok(ServerType::Local),
        Some(types) if types.local => Ok(ServerType::Local),
        Some(_) => Err(Error::RemoteUnavailable),
    }
}

/// Fails if `path` names a drive that does not exist (`Q:\data`). Only
/// Windows has drive letters.
fn check_drive(path: &Path) -> Result<(), Error> {
    let s = path.to_string_lossy();
    let mut chars = s.chars();
    if let (Some(letter), Some(':')) = (chars.next(), chars.next())
        && letter.is_ascii_alphabetic()
        && cfg!(windows)
        && !Path::new(&format!("{letter}:\\")).exists()
    {
        return Err(Error::NoDriveConnection);
    }
    Ok(())
}

/// Opens a connection described by a connect string, as the user its
/// `User ID` names.
pub fn connect(session: &Session, connect_string: &str) -> Result<Handle, Error> {
    let source = connect_string_value(connect_string, "data source");
    let path = PathBuf::from(source.unwrap_or_default());
    let server_type = choose_server(connect_string_value(connect_string, "servertype"))?;
    let user = connect_string_value(connect_string, "user id");
    let password = connect_string_value(connect_string, "password");
    let encryption = encryption_type(connect_string_value(connect_string, "encryptiontype"));
    let dd_password = connect_string_value(connect_string, "ddpassword");
    open(
        session,
        path,
        server_type,
        user,
        password,
        dd_password,
        encryption,
    )
}

/// Opens a connection to `path` on one of the servers `server_types` allows
/// (`ADS_*_SERVER` bits; 0 for those [`settings::server_types`] allows), as
/// `user` if given, as `AdsConnect26` and `AdsConnect60` do. tflip has only
/// the local server; without it, as in the original engine, a set with a
/// remote server fails with [`Error::RemoteNotFound`], an Internet server
/// alone with [`Error::NoDriveConnection`].
pub fn connect_path(
    session: &Session,
    path: &str,
    server_types: u16,
    user: Option<&str>,
    password: Option<&str>,
) -> Result<Handle, Error> {
    connect_path_with(session, path, server_types, user, password, None)
}

/// [`connect_path`] with the dictionary password `dd_password` that opens a
/// dictionary encrypted with AES (`DDPassword`), which a client of a server
/// sends along, as it does the login's password.
pub fn connect_path_with(
    session: &Session,
    path: &str,
    server_types: u16,
    user: Option<&str>,
    password: Option<&str>,
    dd_password: Option<&str>,
) -> Result<Handle, Error> {
    let server_types = if server_types == 0 {
        settings::server_types(session)
    } else {
        server_types
    };
    match server_types {
        types if types & LOCAL != 0 => open(
            session,
            PathBuf::from(path),
            ServerType::Local,
            user,
            password,
            dd_password,
            0,
        ),
        types if types & REMOTE != 0 => Err(Error::RemoteNotFound),
        _ => Err(Error::NoDriveConnection),
    }
}

/// Opens a connection to `path` on `server_type` as `user`, with the
/// administrator `password` that recovers the dictionary key (see
/// [`Database::key`]) and the dictionary password `dd_password` that opens a
/// dictionary encrypted with AES. A client's `path` is kept as the original
/// names it (see [`tflip_storage::universal_path`]); a server's names a share.
pub(crate) fn open(
    session: &Session,
    path: PathBuf,
    server_type: ServerType,
    user: Option<&str>,
    password: Option<&str>,
    dd_password: Option<&str>,
    encryption_type: u8,
) -> Result<Handle, Error> {
    let path = match &session.shares {
        Some(shares) => shares.connection_path(&path.to_string_lossy())?,
        None => PathBuf::from(tflip_storage::universal_path(&path.to_string_lossy())),
    };
    check_drive(&path)?;
    let dictionary = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("add"));
    // `Disable_Free_Connections` binds ADSSYS too
    // (`fixtures/events/freeconf_disable.json`).
    if !dictionary
        && session
            .shares
            .as_ref()
            .is_some_and(|shares| shares.disable_free_connections())
    {
        return Err(Error::Ace {
            code: 7083,
            detail: "axServerConnect".to_owned(),
        });
    }
    if dictionary && !path.is_file() {
        return Err(Error::FileNotFound);
    }
    let database = match dictionary {
        true => {
            let mut database = Database::open_with_password(&path, dd_password.map(str::as_bytes))?;
            let secret = password.unwrap_or("").as_bytes();
            let key = match session.shares {
                Some(_) => crate::login_lockout::login(session, &path, &database, user, secret)?,
                // The local server counts no failed logins either way
                // (`fixtures/events/lockout_local.json`), but its refusal names
                // the call for every user it knows, the administrator and no
                // user at all, and not for a name it does not.
                None => database.login(user, secret).map_err(|error| match error {
                    Error::LoginFailed
                        if user
                            .filter(|user| !user.is_empty())
                            .is_none_or(|user| database.has_user(user)) =>
                    {
                        Error::Ace {
                            code: 7078,
                            detail: "axServerConnect".to_owned(),
                        }
                    }
                    other => other,
                })?,
            };
            database.check_logins_enabled(user)?;
            database.set_identity(user, key);
            if let Some(password) = password {
                database.set_password(password.as_bytes());
            }
            Some(database)
        }
        false => None,
    };
    let connection = new(session, path, server_type, database, user, encryption_type);
    let handle = session.insert(Object::Connection(Arc::new(connection)), None);
    // A dictionary's `CONNECT` triggers may refuse the connection.
    if dictionary && let Err(error) = crate::trigger::fire_connect(session, handle) {
        session.remove(handle);
        return Err(error);
    }
    Ok(handle)
}

/// Opens a connection to the dictionary at `path`, which `database` is,
/// logged in as `user`, for `parent`'s statements to reach the tables of a
/// link: it closes with `parent`.
pub(crate) fn open_linked(
    session: &Session,
    parent: Handle,
    path: PathBuf,
    database: Database,
    user: Option<&str>,
) -> Handle {
    let connection = new(session, path, ServerType::Local, Some(database), user, 0);
    session.insert(Object::Connection(Arc::new(connection)), Some(parent))
}

/// A connection to `path`, its dictionary `database` if it is to one.
fn new(
    session: &Session,
    path: PathBuf,
    server_type: ServerType,
    database: Option<Database>,
    user: Option<&str>,
    encryption_type: u8,
) -> Connection {
    let dictionary = database.is_some();
    let date_format = Mutex::new(None);
    let exact = AtomicBool::new(settings::exact(session));
    let settings = Arc::clone(&session.settings);
    let application_id = Mutex::new(application_name());
    // A server's connections keep the statements' tables open as the
    // original server's do; a client's own process's keep none, unless its
    // caching is on (see `crate::cache`).
    let [cached_tables, cached_cursors] = match session.shares {
        Some(_) => [0, DEFAULT_CACHED_CURSORS],
        None => settings::local_cache_sizes(session).unwrap_or([0, 0]),
    };
    Connection {
        stamp: Mutex::new(stamp(&path)),
        trigger_order: Mutex::default(),
        path,
        server_type,
        dictionary,
        database: RwLock::new(database),
        date_format,
        settings,
        exact,
        application_id,
        statement_limit: std::sync::atomic::AtomicI32::new(50),
        last_autoinc: Arc::default(),
        debugging: AtomicBool::new(false),
        plans: std::sync::atomic::AtomicU32::new(0),
        collation: Mutex::new(String::new()),
        user: user.filter(|u| !u.is_empty()).map(str::to_owned),
        transaction: Mutex::default(),
        autocommit: Mutex::default(),
        calls: Mutex::default(),
        privileged: AtomicBool::new(false),
        encryption_type: AtomicU8::new(encryption_type),
        table_type: Mutex::new(crate::table::TableType::Adt),
        unique_enforcement: AtomicBool::new(true),
        autoinc_enforcement: AtomicBool::new(true),
        check_free_tables: AtomicBool::new(false),
        server_checks_free_tables: session
            .shares
            .as_ref()
            .is_some_and(|shares| shares.check_free_table_access()),
        rand: Arc::default(),
        links: Mutex::default(),
        linked: Mutex::default(),
        cached_tables: std::sync::atomic::AtomicU16::new(cached_tables),
        cached_cursors: std::sync::atomic::AtomicU16::new(cached_cursors),
        active_links: Mutex::default(),
        triggers_disabled: AtomicBool::new(false),
        temps: crate::temp::Temps::default(),
        number: CONNECTIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1,
        operations: std::sync::atomic::AtomicU32::new(0),
        user_id: free_user_id(session),
        statements: std::sync::atomic::AtomicU32::new(0),
        table_opens: Mutex::default(),
        trigger_opens: Mutex::default(),
    }
}

/// When the files of the dictionary at `path` (the table and its memo file)
/// last changed, and how long they were.
type Stamp = [Option<(std::time::SystemTime, u64)>; 2];

fn stamp(path: &Path) -> Stamp {
    [path.to_owned(), path.with_extension("am")].map(|file| {
        let meta = std::fs::metadata(file).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    })
}

/// What a new connection's application ID is: the file name of the
/// program, as the original client engine starts it. (On a server this is
/// the server's.)
fn application_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default()
}

/// The statement limit of `connection` (`sp_mgGetConfigInfo`).
pub(crate) fn statement_limit(session: &Session, connection: Handle) -> Result<i32, Error> {
    Ok(get(session, connection)?
        .statement_limit
        .load(std::sync::atomic::Ordering::Relaxed))
}

/// Sets the statement limit of `connection` (`sp_SetStatementLimit`).
pub(crate) fn set_statement_limit(
    session: &Session,
    connection: Handle,
    limit: i32,
) -> Result<(), Error> {
    get(session, connection)?
        .statement_limit
        .store(limit, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

/// The application ID of `connection` (`sp_GetApplicationID`).
pub fn application_id(session: &Session, connection: Handle) -> Result<String, Error> {
    Ok(lock(&get(session, connection)?.application_id).clone())
}

/// Sets the application ID of `connection` (`sp_SetApplicationID`).
pub fn set_application_id(session: &Session, connection: Handle, id: &str) -> Result<(), Error> {
    *lock(&get(session, connection)?.application_id) = id.to_owned();
    Ok(())
}

/// Sets whether unique keys are enforced on `connection`'s writes
/// (`AdsDisableUniqueEnforcement`/`AdsEnableUniqueEnforcement`).
pub fn set_unique_enforcement(
    session: &Session,
    connection: Handle,
    on: bool,
) -> Result<(), Error> {
    get(session, connection)?.set_unique_enforcement(on);
    Ok(())
}

/// How many statements' tables a connection on a server (or in a client's
/// own process with its caching on) keeps open after closing them unless
/// told otherwise (`AdsCacheOpenCursors`), as in the original engine.
pub const DEFAULT_CACHED_CURSORS: u16 = 25;

/// Sets how many tables (`cursors` false: `AdsCacheOpenTables`) or SQL
/// statements' tables (`AdsCacheOpenCursors`) `connection` keeps open after
/// closing them (see `crate::cache` and [`Connection::set_cache_size`]).
pub fn set_cache_size(
    session: &Session,
    connection: Handle,
    cursors: bool,
    size: u16,
) -> Result<(), Error> {
    get(session, connection)?.set_cache_size(cursors, size, session.shares.is_none());
    Ok(())
}

/// Closes the tables `connection` keeps open after closing them
/// (`AdsCloseCachedTables`); its SQL statements' tables stay open, as in
/// the original engine.
pub fn close_cached_tables(session: &Session, connection: Handle) -> Result<(), Error> {
    let connection = get(session, connection)?;
    crate::cache::close(&connection, Some(crate::cache::Kind::Tables));
    Ok(())
}

/// Sets the data-encryption type of the tables created on `connection`, as
/// the `EncryptionType` of its connect string does: a client of a server
/// sends it after connecting.
pub fn set_encryption_type(
    session: &Session,
    connection: Handle,
    encryption_type: u8,
) -> Result<(), Error> {
    get(session, connection)?
        .encryption_type
        .store(encryption_type, Ordering::Relaxed);
    Ok(())
}

/// Sets the table type of `connection`'s connect string, which a client of a
/// server sends after connecting.
pub fn set_table_type(
    session: &Session,
    connection: Handle,
    table_type: crate::table::TableType,
) -> Result<(), Error> {
    *lock(&get(session, connection)?.table_type) = table_type;
    Ok(())
}

/// Makes `connection` refuse free tables to the users that are no
/// administrators, as `CheckFreeTableAccess` does.
pub fn set_check_free_tables(session: &Session, connection: Handle, on: bool) -> Result<(), Error> {
    get(session, connection)?
        .check_free_tables
        .store(on, Ordering::Relaxed);
    Ok(())
}

/// Sets whether the engine fills AutoInc fields on `connection`'s writes
/// (`AdsDisableAutoIncEnforcement`/`AdsEnableAutoIncEnforcement`).
pub fn set_autoinc_enforcement(
    session: &Session,
    connection: Handle,
    on: bool,
) -> Result<(), Error> {
    get(session, connection)?.set_autoinc_enforcement(on);
    Ok(())
}

/// The date format of the connection `connection`: its own, or the
/// session's ([`settings::date_format`]); connection 0 is the session's. A
/// handle that is not a connection is [`Error::InvalidHandle`].
pub fn date_format(session: &Session, connection: Handle) -> Result<String, Error> {
    if connection == 0 {
        return Ok(settings::date_format(session));
    }
    Ok(get(session, connection)
        .map_err(|_| Error::InvalidHandle)?
        .date_format())
}

/// Sets the date format of the connection `connection`, checked as
/// [`settings::set_date_format`] checks it (`AdsSetDateFormat60`).
/// Connection 0 sets the session's, as `AdsSetDateFormat` does. A handle
/// that is not a connection is [`Error::InvalidHandle`].
pub fn set_date_format(session: &Session, connection: Handle, format: &str) -> Result<(), Error> {
    if connection == 0 {
        return settings::set_date_format(session, format);
    }
    let connection = get(session, connection).map_err(|_| Error::InvalidHandle)?;
    *lock(&connection.date_format) = Some(settings::parse_date_format(format)?);
    Ok(())
}

/// Drops every connection's own date format, as setting the session's one
/// does.
pub(crate) fn forget_date_formats(session: &Session) {
    session.for_each(|object| {
        if let Object::Connection(connection) = object {
            *lock(&connection.date_format) = None;
        }
    });
}

/// The connection behind `handle`.
pub fn get(session: &Session, handle: Handle) -> Result<Arc<Connection>, Error> {
    session
        .with(handle, |object| match object {
            Object::Connection(connection) => Some(Arc::clone(connection)),
            _ => None,
        })
        .flatten()
        .ok_or(Error::InvalidConnection)
}

/// Closes everything `session` has open (`AdsApplicationExit`): its
/// connections and what they own, and the tables opened without one. As in
/// the original engine, the tables write their pending changes as they
/// close.
pub fn close_all(session: &Session) {
    let connections = all(session);
    session.remove_all(|_| false);
    for connection in connections {
        let _ = crate::transaction::abandon(&connection);
    }
}

/// The connections of `session`.
pub(crate) fn all(session: &Session) -> Vec<Arc<Connection>> {
    let mut connections = Vec::new();
    session.for_each(|object| {
        if let Object::Connection(connection) = object {
            connections.push(Arc::clone(connection));
        }
    });
    connections
}

/// Closes the connection behind `handle` and everything opened on it; handle
/// 0 closes every connection of the session.
pub fn disconnect(session: &Session, handle: Handle) -> Result<(), Error> {
    // A transaction still running is rolled back, after the tables closed
    // (writing what was pending).
    if handle == 0 {
        let connections = all(session);
        session.remove_all(|object| !matches!(object, Object::Connection(_)));
        for connection in connections {
            crate::transaction::abandon(&connection)?;
        }
        return Ok(());
    }
    let connection = get(session, handle)?;
    if connection.dictionary {
        // What it has open closes first.
        crate::trigger::connection_closing(session, handle);
        crate::trigger::fire_disconnect(session, handle);
        crate::aep::connection_closing(session, handle);
    }
    session.remove(handle);
    crate::transaction::abandon(&connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Connections to the AES dictionaries the original made, as it answered
    /// them (`fixtures/aes/dict/aesdict3.json`): e3 enciphers its records and
    /// opens only with its dictionary password; a3 keeps them in the clear.
    #[test]
    fn connects_to_aes_dictionaries() {
        use crate::script_host::tests::query;
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/aes/dict");
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-aes-dict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for name in ["a3", "e3"] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            for entry in std::fs::read_dir(src.join(name)).unwrap() {
                let entry = entry.unwrap();
                std::fs::copy(entry.path(), dir.join(name).join(entry.file_name())).unwrap();
            }
        }
        let session = Session::new();
        let cs = |name: &str, rest: &str| {
            let add = dir.join(name).join(format!("{name}.add"));
            format!("Data Source={};ServerType=LOCAL;{rest}", add.display())
        };
        for rest in [
            "User ID=ADSSYS;Password=admpw;",
            "User ID=ADSSYS;Password=wrongpw;",
            "User ID=u1;Password=userpw;",
            "User ID=ADSSYS;Password=admpw;EncryptionType=AES128;",
        ] {
            let refused = connect(&session, &cs("e3", rest));
            assert_eq!(refused.err(), Some(Error::DictionaryEncrypted), "{rest}");
        }
        let wrong = "User ID=ADSSYS;Password=admpw;EncryptionType=AES128;DDPassword=wrong;";
        assert_eq!(
            connect(&session, &cs("e3", wrong)).err(),
            Some(Error::InvalidPassword)
        );

        let a3 = connect(&session, &cs("a3", "User ID=ADSSYS;Password=admpw;")).unwrap();
        let stmt = crate::statement::create(&session, a3).unwrap();
        let (_, users) = query(&session, stmt, "SELECT Name FROM system.users");
        assert_eq!(users.len(), 2, "u1 and u2: {users:?}");
        // Its table t, encrypted after, needs the dictionary password.
        let t = crate::statement::execute_direct(&session, stmt, "SELECT * FROM t", true);
        match t {
            Err(Error::Sql(error)) => {
                let recorded = std::fs::read_to_string(src.join("aesdict3.json")).unwrap();
                let recorded: serde_json::Value = serde_json::from_str(&recorded).unwrap();
                assert_eq!(format!("Error 7200:  {error}"), recorded["008_text"]);
            }
            other => panic!("{other:?}"),
        }
        disconnect(&session, a3).unwrap();
        let u1 = connect(&session, &cs("a3", "User ID=u1;Password=userpw;")).unwrap();
        disconnect(&session, u1).unwrap();

        let dd = "EncryptionType=AES128;DDPassword=ddpw;";
        let admin = connect(
            &session,
            &cs("e3", &format!("User ID=ADSSYS;Password=admpw;{dd}")),
        )
        .unwrap();
        let stmt = crate::statement::create(&session, admin).unwrap();
        let (_, rows) = query(&session, stmt, "SELECT * FROM t");
        assert_eq!(rows.len(), 1);
        let (_, rows) = query(&session, stmt, "SELECT * FROM t2");
        assert_eq!(rows.len(), 1);
        disconnect(&session, admin).unwrap();
        let refused = connect(
            &session,
            &cs("e3", &format!("User ID=u1;Password=wrongpw;{dd}")),
        );
        assert_eq!(
            refused.err(),
            Some(Error::Ace {
                code: 7078,
                detail: "axServerConnect".to_owned()
            })
        );
        let u1 = connect(
            &session,
            &cs("e3", &format!("User ID=u1;Password=userpw;{dd}")),
        )
        .unwrap();
        let stmt = crate::statement::create(&session, u1).unwrap();
        let (_, rows) = query(&session, stmt, "SELECT * FROM t");
        assert_eq!(rows.len(), 1);
        disconnect(&session, u1).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// As `AdsApplicationExit` does in the original engine (probed):
    /// pending changes are written, and no handle is left.
    #[test]
    fn closing_everything_writes_what_is_pending() {
        use crate::setters::SetValue;
        use crate::table::{self, FieldRef, TableSettings, TableType};
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-close-all-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fixtures =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt/indexes");
        for file in ["indexed.adt", "indexed.adi"] {
            std::fs::copy(fixtures.join(file), dir.join(file)).unwrap();
        }
        let source = format!("Data Source={};ServerType=LOCAL;", dir.display());
        let settings = TableSettings {
            table_type: TableType::Adt,
            ..TableSettings::default()
        };
        let name = FieldRef::Name("name");
        let session = Session::new();
        let connection = connect(&session, &source).unwrap();
        let first = table::open(&session, connection, "indexed.adt", settings.clone()).unwrap();
        let second = table::open(&session, connection, "indexed.adt", settings.clone()).unwrap();
        let count = table::get(&session, first).unwrap().record_count().unwrap();
        let appending = table::get(&session, first).unwrap();
        appending.append().unwrap();
        appending
            .set_field(name, &SetValue::String("Appended".to_owned()))
            .unwrap();
        let appended = appending.record_number();
        let changing = table::get(&session, second).unwrap();
        changing.goto_record(1).unwrap();
        changing
            .set_field(name, &SetValue::String("Changed".to_owned()))
            .unwrap();
        drop((appending, changing));
        close_all(&session);
        assert!(table::get(&session, first).is_err());
        assert!(get(&session, connection).is_err());

        let session = Session::new();
        let connection = connect(&session, &source).unwrap();
        let table = table::open(&session, connection, "indexed.adt", settings).unwrap();
        let table = table::get(&session, table).unwrap();
        assert_eq!(table.record_count().unwrap(), count + 1);
        let text = |record| {
            table.goto_record(record).unwrap();
            match table.value(name).unwrap() {
                tflip_storage::value::Value::Text(text) => text.trim_end().to_owned(),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(
            (text(1), text(appended)),
            ("Changed".to_owned(), "Appended".to_owned())
        );
        drop(table);
        drop(session);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn connect_string_values() {
        let s = r"Data Source=C:\Program Files (x86)\ADS_DATA\;FIPS=False;ServerType=LOCAL;";
        assert_eq!(
            connect_string_value(s, "data source"),
            Some(r"C:\Program Files (x86)\ADS_DATA\")
        );
        assert_eq!(connect_string_value(s, "SERVERTYPE"), Some("LOCAL"));
        assert_eq!(
            connect_string_value("ServerType=LOCAL; data source = x ;", "Data Source"),
            Some("x")
        );
        assert_eq!(
            connect_string_value("ServerType=LOCAL;", "data source"),
            None
        );
    }

    #[test]
    fn server_type_choice() {
        for local in ["LOCAL", "local", "1", "REMOTE|LOCAL", "3", "7"] {
            assert_eq!(choose_server(Some(local)), Ok(ServerType::Local), "{local}");
        }
        assert_eq!(choose_server(None), Ok(ServerType::Local));
        assert_eq!(choose_server(Some("")), Ok(ServerType::Local));
        for remote in ["REMOTE", "AIS", "2", "remote|ais", "nonsense"] {
            assert_eq!(
                choose_server(Some(remote)),
                Err(Error::RemoteUnavailable),
                "{remote}"
            );
        }
    }

    /// A connection's user number is the lowest not taken, from 2 (probed,
    /// `tools/ace/sql/trigdblog3.txt`).
    #[test]
    fn user_numbers_are_the_lowest_free() {
        let session = Session::new();
        let open = || connect(&session, "Data Source=elsewhere;ServerType=LOCAL;").unwrap();
        let id = |handle| get(&session, handle).unwrap().user_id;
        let (a, b, c) = (open(), open(), open());
        assert_eq!([id(a), id(b), id(c)], [2, 3, 4]);
        disconnect(&session, b).unwrap();
        let d = open();
        assert_eq!(id(d), 3);
        assert_eq!(id(open()), 5);
    }

    #[test]
    fn connect_and_disconnect() {
        let session = Session::new();
        // The folder is not checked.
        let a = connect(
            &session,
            r"Data Source=C:\no\such\folder\;ServerType=LOCAL;",
        )
        .unwrap();
        let b = connect(&session, "Data Source=elsewhere;ServerType=LOCAL;").unwrap();
        assert_ne!(a, b);
        assert_eq!(
            get(&session, a).unwrap().path,
            PathBuf::from(r"C:\no\such\folder\")
        );
        assert_eq!(get(&session, a).unwrap().server_type, ServerType::Local);
        assert!(!get(&session, a).unwrap().dictionary);

        assert_eq!(disconnect(&session, a), Ok(()));
        assert_eq!(get(&session, a).unwrap_err(), Error::InvalidConnection);
        assert_eq!(disconnect(&session, a), Err(Error::InvalidConnection));
        assert_eq!(disconnect(&session, 0x12345), Err(Error::InvalidConnection));
        assert!(get(&session, b).is_ok());

        assert_eq!(disconnect(&session, 0), Ok(()));
        assert!(get(&session, b).is_err());
        assert_eq!(disconnect(&session, 0), Ok(()));
    }

    #[test]
    fn dictionary_must_exist() {
        let session = Session::new();
        let missing = std::env::temp_dir().join("tflip-no-such-dictionary.add");
        let s = format!("Data Source={};ServerType=LOCAL;", missing.display());
        assert_eq!(connect(&session, &s), Err(Error::FileNotFound));
    }
}
