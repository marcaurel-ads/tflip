//! The tables a connection has open, as its database triggers on
//! `OPEN_TABLE` and `CLOSE_TABLE` tell of them.
//!
//! What the original does, probed (`tools/ace/sql/trigdblog.txt`): every table
//! a statement opens fires `OPEN_TABLE`, the statement keeps it open when it
//! is through, and `CLOSE_TABLE` fires when the statement runs other SQL, when
//! it closes or when its connection does. Running the same SQL again keeps
//! the tables (no event). A `SELECT` that is read live makes the client open
//! the table of its cursor once more, which closes with the cursor. The open
//! count `__info` tells is the number of opens of the file, of all
//! connections: after an open for `AFTER OPEN_TABLE` and before a close for
//! `BEFORE CLOSE_TABLE`, else NULL. A trigger that fails in its `AFTER`
//! `OPEN_TABLE` takes the open back unseen; one that fails in `BEFORE
//! CLOSE_TABLE` skips the `AFTER` ones; the table closes either way.
//!
//! tflip does not hold the tables: it notes what a statement opened
//! ([`Opened`]) and lets go of the notes at the same times, so that the
//! triggers see the same sequence. A statement closes its tables in the order
//! it opened them, but a `SELECT` of several tables with no condition, which
//! closes them the other way round (the original's plans, which tflip does not
//! have, decide the order of the others: `docs/DIFFERENCES.md`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, MutexGuard};

use tflip_storage::build::Column;
use tflip_storage::dd::{Kind, trigger};
use tflip_storage::field::FieldType::{CiCharacter, Integer, Logical};
use tflip_storage::value::Value;

use super::{AFTER, BEFORE, Built, CLOSE_EVENT, OPEN_EVENT, fire_database};
use crate::handle::Handle;
use crate::{Error, Session, connection, lock};

/// How many times each table is open (its file, in lower case, or its name for
/// a table with none), for every connection of the process.
fn counts() -> MutexGuard<'static, HashMap<String, u32>> {
    static COUNTS: LazyLock<Mutex<HashMap<String, u32>>> = LazyLock::new(Mutex::default);
    lock(&COUNTS)
}

/// A table open as the triggers saw it opened.
#[derive(Debug)]
pub(crate) struct Opened {
    connection: Handle,
    key: String,
    name: String,
    path: Option<PathBuf>,
    shared: bool,
    free: bool,
    table_type: i64,
    temporary: bool,
    /// Whether it was counted as closed (else dropping it does).
    closed: bool,
}

impl Drop for Opened {
    /// A table whose statement or connection goes without closing it still
    /// stops counting as open.
    fn drop(&mut self) {
        if !self.closed {
            uncount(&self.key);
        }
    }
}

fn uncount(key: &str) {
    let mut counts = counts();
    if let Some(count) = counts.get_mut(key) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            counts.remove(key);
        }
    }
}

/// What the opens of the statement running on the thread are kept in, and
/// those its last run left, which it is running again.
#[derive(Default)]
struct Collect {
    reuse: Vec<Opened>,
    opened: Vec<Opened>,
}

thread_local! {
    /// Whether the opens on the thread are ones the original does not make:
    /// they fire no trigger and count for nothing.
    /// The table whose trigger runs innermost.
    static TRIGGER_TABLE: RefCell<Option<String>> = const { RefCell::new(None) };
    static UNSEEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static COLLECT: RefCell<Option<Collect>> = const { RefCell::new(None) };
    /// The `(event, timing)` of the database triggers that run, innermost
    /// last.
    static FIRING: RefCell<Vec<(u32, u32)>> = const { RefCell::new(Vec::new()) };
}

/// Runs `f`, which opens the tables of a statement: what it opens is kept
/// for the statement to close later, but for the tables of `reuse` (those of
/// its last run, if that was the same SQL) that it opens again, which stay
/// as they are. Returns what `f` returned, what it opened, and what of
/// `reuse` it left.
pub(crate) fn collecting<R>(
    reuse: Vec<Opened>,
    f: impl FnOnce() -> R,
) -> (R, Vec<Opened>, Vec<Opened>) {
    let outer = COLLECT.replace(Some(Collect {
        reuse,
        opened: Vec::new(),
    }));
    let result = f();
    let done = COLLECT.replace(outer).unwrap_or_default();
    (result, done.opened, done.reuse)
}

/// Whether a statement is running on the thread, whose are the opens of any
/// other (a procedure's or a trigger's) that runs inside it.
pub(crate) fn collects() -> bool {
    COLLECT.with_borrow(Option::is_some)
}

/// Notes that the triggers of `table` run, until the result is dropped: the
/// tables their statements open are theirs, not those of the statement that
/// fired them.
pub(super) fn in_trigger(table: &str) -> InTrigger {
    InTrigger(TRIGGER_TABLE.replace(Some(table.to_ascii_lowercase())))
}

pub(super) struct InTrigger(Option<String>);

impl Drop for InTrigger {
    fn drop(&mut self) {
        TRIGGER_TABLE.replace(self.0.take());
    }
}

/// The tables the triggers of `table` opened close (probed, they do when the
/// triggers fire again and when a table of theirs closes, which the original
/// does for any table with triggers).
pub(super) fn release_triggered(session: &Session, connection: Handle, table: &str) {
    let Ok(owner) = connection::get(session, connection) else {
        return;
    };
    let table = table.to_ascii_lowercase();
    let released = {
        let mut kept = lock(&owner.trigger_opens);
        let (released, rest) = std::mem::take(&mut *kept)
            .into_iter()
            .partition::<Vec<_>, _>(|(owner, _)| *owner == table);
        *kept = rest;
        released
    };
    close_all(
        session,
        released.into_iter().map(|(_, opened)| opened).collect(),
    );
}

/// Runs `f`, whose opens are a second look at a table a statement has
/// already opened (the target of an `INSERT`, `UPDATE` or `DELETE`, which the
/// original opens once).
pub(crate) fn unseen<R>(f: impl FnOnce() -> R) -> R {
    let outer = UNSEEN.replace(true);
    let result = f();
    UNSEEN.set(outer);
    result
}

/// Whether a database trigger of `event` and `timing` runs, whose own opens
/// and closes fire no others of the kind (a `BEFORE OPEN_TABLE` trigger's
/// opens fire only `AFTER` ones, as the original's do).
pub(super) fn silenced(event: u32, timing: u32) -> bool {
    FIRING.with_borrow(|firing| firing.contains(&(event, timing)))
}

/// Notes that a database trigger of `event` and `timing` runs, until the
/// result is dropped.
pub(super) fn firing(event: u32, timing: u32) -> Firing {
    FIRING.with_borrow_mut(|firing| firing.push((event, timing)));
    Firing
}

pub(super) struct Firing;

impl Drop for Firing {
    fn drop(&mut self) {
        FIRING.with_borrow_mut(|firing| {
            firing.pop();
        });
    }
}

/// Whether `connection`'s dictionary has database triggers on `OPEN_TABLE`
/// or `CLOSE_TABLE`: no table is noted for one that has none.
fn watched(owner: &connection::Connection) -> bool {
    owner
        .with_database_opt(|db| {
            let dict = db.dictionary();
            let Some(database) = dict.database() else {
                return false;
            };
            dict.all(Kind::Trigger).into_iter().any(|t| {
                t.parent == database.id
                    && matches!(t.u32(trigger::EVENT_TYPE), Some(OPEN_EVENT | CLOSE_EVENT))
            })
        })
        .unwrap_or(false)
}

/// The `OPEN_TABLE` triggers on a table opened on `connection`, for the table
/// `name` opened with `settings`, `direct` if a client asked for it rather than
/// a statement: `BEFORE` the open is made or `AFTER` it was (`timing`). One that
/// fails cancels the open.
///
/// The table's open that `AFTER` makes is the statement's, if a statement
/// runs, which keeps it; a client's is returned for its handle to keep.
pub(crate) fn fire_open(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: &crate::table::TableSettings,
    direct: bool,
    timing: u32,
) -> Result<Option<Opened>, Error> {
    if connection == 0 || UNSEEN.get() {
        return Ok(None);
    }
    let Ok(owner) = connection::get(session, connection) else {
        return Ok(None);
    };
    if !watched(&owner) {
        return Ok(None);
    }
    let known = name.to_ascii_lowercase();
    let stem = |opened: &Opened| opened.name.to_ascii_lowercase() == known;
    // A table the statement's last run opened, which this run opens again.
    let reused = |take: bool| {
        COLLECT.with_borrow_mut(|collect| {
            let collect = collect.as_mut()?;
            let at = collect.reuse.iter().position(stem)?;
            if take {
                let opened = collect.reuse.remove(at);
                collect.opened.push(opened);
            }
            Some(())
        })
    };
    if !direct && timing == BEFORE && reused(false).is_some() {
        return Ok(None);
    }
    if !direct && timing == AFTER && reused(true).is_some() {
        return Ok(None);
    }
    let mut opened = describe(session, connection, name, settings);
    let count = match timing {
        BEFORE => None,
        _ => {
            let mut counts = counts();
            let count = counts.entry(opened.key.clone()).or_insert(0);
            *count += 1;
            Some(*count)
        }
    };
    let info = table_info(&owner, &opened, "OPEN_TABLE", timing, count, Some(direct));
    let fired = fire_database(session, connection, OPEN_EVENT, timing, info);
    if timing == BEFORE {
        // Nothing was opened yet.
        opened.closed = true;
        return fired.map(|()| None);
    }
    // The open an `AFTER` trigger cancelled is taken back, unseen (`opened`
    // does as it drops).
    fired?;
    if direct {
        return Ok(Some(opened));
    }
    if let Some(table) = TRIGGER_TABLE.with_borrow(Clone::clone) {
        lock(&owner.trigger_opens).push((table, opened));
        return Ok(None);
    }
    // The statements of a database trigger leave their tables open, and
    // counted, for good (probed, `tools/ace/sql/trigdblog3.txt`).
    if FIRING.with_borrow(|firing| !firing.is_empty()) {
        opened.closed = true;
        return Ok(None);
    }
    // An open no statement runs for is not kept.
    COLLECT.with_borrow_mut(|collect| {
        if let Some(collect) = collect {
            collect.opened.push(opened);
        }
    });
    Ok(None)
}

/// What a trigger tells of the table `name` opened with `settings`.
fn describe(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: &crate::table::TableSettings,
) -> Opened {
    use crate::table::TableType;
    let found = crate::table::open_info(session, connection, name, settings);
    let (path, free, table_type) = match found {
        Some((path, free, table_type)) => (Some(path), free, table_type),
        None => (None, true, settings.table_type),
    };
    let table_type = match table_type {
        TableType::Ntx => 1,
        TableType::Cdx => 2,
        TableType::Adt | TableType::Database => 3,
        TableType::Vfp => 4,
    };
    let stem = crate::table::stem(name).trim_start_matches('#').to_owned();
    let key = path
        .as_ref()
        .map_or_else(|| stem.clone(), |p| p.display().to_string())
        .to_ascii_lowercase();
    Opened {
        connection,
        key,
        name: stem,
        path,
        shared: settings.options & crate::table::options::EXCLUSIVE == 0,
        free,
        table_type,
        temporary: crate::temp::is_temp(name),
        closed: false,
    }
}

/// `__info` of an `OPEN_TABLE` or `CLOSE_TABLE` trigger on `opened`.
fn table_info(
    owner: &connection::Connection,
    opened: &Opened,
    event: &str,
    timing: u32,
    count: Option<u32>,
    direct: Option<bool>,
) -> Built {
    let columns = [
        ("UserID", Integer, 4),
        ("TrigType", CiCharacter, 10),
        ("TrigEvent", CiCharacter, 15),
        ("TableName", CiCharacter, 256),
        ("FullPath", CiCharacter, 256),
        ("UserName", CiCharacter, 50),
        ("ClientName", CiCharacter, 50),
        ("OpenCount", Integer, 4),
        ("IsShared", Logical, 1),
        ("IsFree", Logical, 1),
        ("LockType", Integer, 4),
        ("TableType", Integer, 4),
        ("IsDirect", Logical, 1),
        ("IsTemp", Logical, 1),
    ]
    .map(|(name, field_type, length)| Column {
        name: name.to_owned(),
        field_type,
        length,
        decimals: 0,
    });
    let text = |s: &str, width: usize| Value::Text(format!("{s:<width$}"));
    let row = vec![
        Value::Integer(owner.user_id.into()),
        text(if timing == BEFORE { "BEFORE" } else { "AFTER" }, 10),
        text(event, 15),
        text(&opened.name, 256),
        text(
            &opened
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            256,
        ),
        text(owner.user().unwrap_or("ADSSYS"), 50),
        text(&connection::os_user(), 50),
        count.map_or(Value::Null, |count| Value::Integer(count.into())),
        Value::Logical(opened.shared),
        Value::Logical(opened.free),
        Value::Integer(0),
        Value::Integer(opened.table_type.into()),
        direct.map_or(Value::Null, Value::Logical),
        Value::Logical(opened.temporary),
    ];
    (columns.to_vec(), vec![row])
}

/// The table `name` of `path`, as a statement that makes or drops it opens
/// it: exclusively, and a free table, the dictionary not having it (yet or
/// any more).
fn exclusive(connection: Handle, name: &str, path: &std::path::Path) -> Opened {
    Opened {
        connection,
        key: path.display().to_string().to_ascii_lowercase(),
        name: name.to_owned(),
        path: Some(path.to_owned()),
        shared: false,
        free: true,
        table_type: 3,
        temporary: false,
        closed: false,
    }
}

/// `CREATE TABLE` made the table `name` in `path`, which it opened once and
/// closed: only the close is seen.
pub(crate) fn created(session: &Session, connection: Handle, name: &str, path: &std::path::Path) {
    let opened = exclusive(connection, name, path);
    if connection::get(session, connection).is_ok_and(|owner| watched(&owner)) {
        *counts().entry(opened.key.clone()).or_insert(0) += 1;
        let _ = close(session, opened);
    }
}

/// `DROP TABLE` of the table `name` in `path`, which it opens and closes
/// before the files go. A trigger that fails in the open stops it.
pub(crate) fn dropping(
    session: &Session,
    connection: Handle,
    name: &str,
    path: &std::path::Path,
) -> Result<(), Error> {
    let Ok(owner) = connection::get(session, connection) else {
        return Ok(());
    };
    if !watched(&owner) {
        return Ok(());
    }
    let opened = exclusive(connection, name, path);
    let info = table_info(&owner, &opened, "OPEN_TABLE", BEFORE, None, Some(false));
    fire_database(session, connection, OPEN_EVENT, BEFORE, info)?;
    let count = {
        let mut counts = counts();
        let count = counts.entry(opened.key.clone()).or_insert(0);
        *count += 1;
        *count
    };
    let info = table_info(
        &owner,
        &opened,
        "OPEN_TABLE",
        AFTER,
        Some(count),
        Some(false),
    );
    // As an `AFTER` trigger that fails does for any open, it takes it back.
    fire_database(session, connection, OPEN_EVENT, AFTER, info)?;
    let _ = close(session, opened);
    Ok(())
}

/// The `CLOSE_TABLE` triggers on a table that closes, the open count they
/// tell before the close. A trigger that fails in the `BEFORE` ones skips the
/// `AFTER` ones; the table closes all the same, and the failure is the
/// result: a client's close reports it, the closes of statements and
/// connections ignore it.
pub(crate) fn close(session: &Session, mut opened: Opened) -> Result<(), Error> {
    opened.closed = true;
    let count = counts().get(&opened.key).copied().unwrap_or(1);
    let owner = connection::get(session, opened.connection).ok();
    let info = |timing, count| {
        owner
            .as_ref()
            .map(|owner| table_info(owner, &opened, "CLOSE_TABLE", timing, count, None))
    };
    let before = info(BEFORE, Some(count))
        .map(|info| fire_database(session, opened.connection, CLOSE_EVENT, BEFORE, info));
    release_triggered(session, opened.connection, &opened.name);
    uncount(&opened.key);
    match (before, info(AFTER, None)) {
        (Some(Err(error)), _) => Err(error),
        (_, Some(info)) => fire_database(session, opened.connection, CLOSE_EVENT, AFTER, info),
        _ => Ok(()),
    }
}

/// Closes `opened`, in the order they are given.
pub(crate) fn close_all(session: &Session, opened: Vec<Opened>) {
    for opened in opened {
        let _ = close(session, opened);
    }
}

/// Has the connection `connection` keep `opened`, a client's open of the
/// table `handle`, until the handle closes.
pub(crate) fn attach(session: &Session, connection: Handle, handle: Handle, opened: Opened) {
    if let Ok(owner) = connection::get(session, connection) {
        lock(&owner.table_opens).push((handle, opened));
    }
}

/// The table `handle` of `connection` closed: its `CLOSE_TABLE` triggers fire
/// if a client opened it.
pub(crate) fn table_closed(
    session: &Session,
    connection: Handle,
    handle: Handle,
) -> Result<(), Error> {
    let Ok(owner) = connection::get(session, connection) else {
        return Ok(());
    };
    let opened = {
        let mut kept = lock(&owner.table_opens);
        let at = kept.iter().position(|(h, _)| *h == handle);
        at.map(|at| kept.remove(at).1)
    };
    opened.map_or(Ok(()), |opened| close(session, opened))
}

/// The connection `connection` is closing: what it has open closes, the
/// newest handle first, a statement's cursor before the tables it holds.
pub(crate) fn connection_closing(session: &Session, connection: Handle) {
    use crate::handle::Object;
    let mut owned = session.owned(connection, |object| {
        matches!(object, Object::Table(_) | Object::Statement(_))
    });
    owned.sort_unstable_by(|a, b| b.cmp(a));
    for handle in owned {
        let statement = session
            .with(handle, |object| matches!(object, Object::Statement(_)))
            .unwrap_or(false);
        match statement {
            true => crate::statement::release(session, handle),
            false => {
                let _ = table_closed(session, connection, handle);
            }
        }
    }
}
