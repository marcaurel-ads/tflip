//! Transactions, per connection, as the original engine keeps them
//! (`docs/TRANSACTIONS.md`): a nesting count, which the outermost begin and
//! commit start and end, and savepoints by name.
//!
//! The session's mode decides what they are. With real transactions (the
//! default, as the original's remote server has them), a rollback to an
//! unknown savepoint fails, SQL's transaction statements count as the API's
//! calls do, and the commands that cannot run in one are refused. In the
//! mode of the original's local server, which has no transactions, only the
//! count and its sequence are kept, and SQL's statements change nothing.
//! With real transactions, what a transaction writes is held back until it
//! ends (see the table module's `tx`): the records are marked in the files,
//! their new images kept in the overlay of the table files the process
//! shares, and the transaction's log here says, write by write, what each
//! record was before, so that a rollback, to a savepoint too, can take it
//! back. Its log on disk (see [`crate::txlog`]) says what each record is,
//! so that a crash leaves nothing that cannot be finished or taken back.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::connection::{self, Connection};
use crate::handle::{Handle, Object};
use crate::shared::Shared;
use crate::shared::{Earlier, TxRecord};
use crate::{Error, Session, lock, settings, txlog};

/// A connection's transaction.
#[derive(Debug, Default)]
pub(crate) struct State {
    /// The nesting level: 0 outside a transaction.
    count: u32,
    /// The transaction's number, while it runs.
    id: u64,
    /// When it began (local time: the day and the milliseconds since midnight).
    started: (i32, u32),
    /// The savepoints, oldest first, by the names given, each with the
    /// length the log had when it was set.
    savepoints: Vec<(String, usize)>,
    /// What the transaction wrote, write by write.
    log: Vec<Logged>,
    /// Its log on disk, from its first write.
    disk: Option<txlog::Log>,
    /// A library of an external procedure made a call the original's remote
    /// server refuses with 5047 inside the caller's transaction: its commits
    /// and rollbacks happen, but fail with 7207 until it ends, and the
    /// library's `AdsInTransaction` sees only the levels it began.
    tainted: bool,
    /// The slots of records the transaction appended through a handle that a
    /// rollback to a savepoint took back: it let their locks go, and the
    /// commit's own release of them fails (7138,
    /// `fixtures/write/tx_savepoint_lock`). An append into the slot again
    /// locks it anew.
    lost: Vec<(Arc<Shared>, u32)>,
}

impl State {
    /// Whether a transaction is running.
    pub(crate) fn in_transaction(&self) -> bool {
        self.count > 0
    }
}

/// A write of a transaction: the record's slot in the table file `shared`,
/// and its state in the transaction before the write; `None` if the
/// transaction had not written it before.
#[derive(Debug)]
struct Logged {
    shared: Arc<Shared>,
    slot: u32,
    before: Option<Earlier>,
    /// Whether it is the append of a record by a handle, which locks the
    /// record until the transaction ends; SQL's appends do not (see
    /// [`State::lost`]).
    locked: bool,
    /// Whether it is the append of a record, by a handle or SQL: a rollback
    /// frees its slot, and a handle standing on it keeps what it had.
    appended: bool,
    /// Whether a rollback to a savepoint of a trigger took the record back as
    /// the one its `AFTER INSERT` triggers fired on (see [`take_back_insert`]):
    /// neither a commit nor a later rollback does anything for it.
    taken: bool,
}

/// A transaction that writes: its number and its connection.
pub(crate) struct Writer {
    pub(crate) id: u64,
    pub(crate) connection: Arc<Connection>,
}

/// The transaction `connection` writes in, if it has a real one.
pub(crate) fn writer(connection: &Arc<Connection>) -> Option<Writer> {
    if !connection.settings.real_transactions_on() {
        return None;
    }
    let state = lock(&connection.transaction);
    (state.count > 0).then(|| Writer {
        id: state.id,
        connection: Arc::clone(connection),
    })
}

/// The number of the transaction `connection` reads in, if it has one.
pub(crate) fn reader(connection: &Connection) -> Option<u64> {
    let state = lock(&connection.transaction);
    (state.count > 0).then_some(state.id)
}

/// Logs a write of the transaction of `connection` (see [`Logged`]),
/// which leaves the record `record` in slot `slot` of the table file
/// `shared` (opened as `source`): on disk first, and synced.
/// [`Error::TransactionLog`] if the log cannot be written.
pub(crate) fn logged(
    connection: &Connection,
    shared: &Arc<Shared>,
    source: &txlog::Source,
    slot: u32,
    before: Option<Earlier>,
    record: &TxRecord,
    adt: bool,
) -> Result<(), Error> {
    let mut state = lock(&connection.transaction);
    // The transaction the write began in ended meanwhile (on another
    // thread): the write is in none.
    if state.count == 0 || state.id != record.tx {
        return Err(Error::NotInTransaction);
    }
    if state.disk.is_none() {
        state.disk = Some(txlog::Log::create().map_err(|_| Error::TransactionLog)?);
    }
    let disk = state.disk.as_mut().ok_or(Error::TransactionLog)?;
    disk.record(shared, source, slot, record)
        .map_err(|_| Error::TransactionLog)?;
    let appended = before.is_none() && record.original.is_none();
    // A table open exclusively takes no locks; a DBF table's appends are not
    // locked this way (`fixtures/write/tx_cursor_after`).
    let locked = appended
        && adt
        && !shared.opened_exclusively()
        && crate::statement::running_name().is_none();
    if locked {
        state
            .lost
            .retain(|(lost, at)| !(Arc::ptr_eq(lost, shared) && *at == slot));
    }
    state.log.push(Logged {
        shared: Arc::clone(shared),
        slot,
        before,
        locked,
        appended,
        taken: false,
    });
    Ok(())
}

/// The number of the next transaction.
fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// What a commit puts in the full-text orders of an ADT table, write by write
/// in the order of the log, as the original's server does
/// (`fixtures/write/fts_tx_big_commit`, `fts_tx_memo_commit`): the words each
/// write took away and brought, whose order decides how the orders' pages are
/// laid out. The images are those the transaction held before and after the
/// write; a record not in the orders (appended and not written, or deleted)
/// has none. The orders are taken those on memo fields first, each
/// group by the names of its tags.
fn replay_words(log: &[Logged]) -> Result<(), Error> {
    let state = |earlier: &Earlier| match earlier.deleted || earlier.unwritten {
        true => None,
        false => Some(earlier.image.clone()),
    };
    let on_memo = |order: &Arc<crate::index::Index>| {
        let number: usize = order
            .tag
            .expression
            .trim()
            .strip_prefix('F')
            .and_then(|digits| digits.parse().ok())
            .unwrap_or(0);
        order
            .owner()
            .fields()
            .get(number.wrapping_sub(1))
            .is_some_and(|field| field.is_memo())
    };
    for (i, entry) in log.iter().enumerate().filter(|(_, entry)| !entry.taken) {
        let Some(record) = entry.shared.transactions().get(&entry.slot).cloned() else {
            continue;
        };
        if !record.orders.iter().any(|order| order.tag.full_text()) {
            continue;
        }
        // The record's state after this write: the next write of it says what
        // it was before, the transaction's record what it is now.
        let next = log[i + 1..].iter().find(|other| {
            !other.taken && Arc::ptr_eq(&other.shared, &entry.shared) && other.slot == entry.slot
        });
        let before = match &entry.before {
            Some(earlier) => state(earlier),
            None => record.original.clone(),
        };
        let after = match next.and_then(|next| next.before.as_ref()) {
            Some(earlier) => state(earlier),
            None => state(&Earlier::of(&record)),
        };
        if before == after {
            continue;
        }
        let mut orders = record.orders;
        orders.sort_by_key(|order| (!on_memo(order), order.tag.name.clone()));
        crate::table::commit_words(
            &entry.shared,
            entry.slot,
            &orders,
            before.as_deref(),
            after.as_deref(),
        )?;
    }
    Ok(())
}

/// Commits what `log` wrote: each record once, in the order the
/// transaction last wrote them (which frees deleted records' slots in the
/// order the original's server does, `fixtures/write/fts_tx_big_commit`).
fn apply(log: Vec<Logged>) -> Result<(), Error> {
    let mut result = replay_words(&log);
    let last = |entry: &Logged| {
        log.iter().rposition(|other| {
            Arc::ptr_eq(&other.shared, &entry.shared) && other.slot == entry.slot && !other.taken
        })
    };
    let mut records: Vec<(Option<usize>, &Logged)> = log
        .iter()
        .filter(|entry| entry.before.is_none() && !entry.taken)
        .map(|entry| (last(entry), entry))
        .collect();
    records.sort_by_key(|(last, _)| *last);
    for (n, (_, entry)) in records.into_iter().enumerate() {
        crash_point(n);
        // The images it held between, whose memo values the commit frees.
        let between: Vec<Vec<u8>> = log
            .iter()
            .filter(|other| {
                Arc::ptr_eq(&other.shared, &entry.shared)
                    && other.slot == entry.slot
                    && !other.taken
            })
            .filter_map(|other| other.before.as_ref())
            .filter(|earlier| !earlier.unwritten)
            .map(|earlier| earlier.image.clone())
            .collect();
        let done = crate::table::commit_record(&entry.shared, entry.slot, &between);
        if result.is_ok() {
            result = done;
        }
    }
    result
}

/// For the tests of recovery, in debug builds: the process ends before the
/// commit writes its `n`th record if `TFLIP_TEST_CRASH_IN_COMMIT` says `n`.
fn crash_point(n: usize) {
    #[cfg(debug_assertions)]
    if std::env::var("TFLIP_TEST_CRASH_IN_COMMIT").is_ok_and(|at| at == n.to_string()) {
        std::process::exit(3);
    }
    #[cfg(not(debug_assertions))]
    let _ = n;
}

/// Ends the transaction `state` has had: its log on disk removed, unless
/// its commit or rollback failed on the way (`done` is not `Ok`); then the
/// log stays, for recovery to finish it.
fn finish(disk: Option<txlog::Log>, done: &Result<(), Error>) {
    if let Some(disk) = disk
        && done.is_ok()
    {
        disk.remove();
    }
}

/// Commits the transaction whose `state` was taken: its commit logged, then
/// applied; if the commit cannot be logged, it is rolled back.
///
/// Whether the commit left locks it fails to release (`Ok(true)`: the
/// transaction is committed, and the client told 7138).
fn commit_state(state: State) -> Result<bool, Error> {
    let State {
        log,
        mut disk,
        lost,
        ..
    } = state;
    if let Some(log_file) = disk.as_mut()
        && log_file.commit().is_err()
    {
        let undone = undo(log);
        finish(disk, &undone);
        return Err(Error::TransactionLog);
    }
    let result = apply(log);
    finish(disk, &result);
    result.map(|()| !lost.is_empty())
}

/// Rolls back the transaction whose `state` was taken.
fn roll_back_state(state: State) -> Result<(), Error> {
    let State { log, disk, .. } = state;
    let result = undo(log);
    finish(disk, &result);
    result
}

/// Takes back what `log` wrote: record by record in the order the
/// transaction first wrote them, as the original's server frees appended
/// records (`fixtures/write/tx_sql`), each record's writes the last first.
fn undo(log: Vec<Logged>) -> Result<(), Error> {
    let mut records: Vec<Vec<Logged>> = Vec::new();
    for entry in log.into_iter().filter(|entry| !entry.taken) {
        let same = |other: &Vec<Logged>| {
            Arc::ptr_eq(&other[0].shared, &entry.shared) && other[0].slot == entry.slot
        };
        match records.iter_mut().find(|record| same(record)) {
            Some(record) => record.push(entry),
            None => records.push(vec![entry]),
        }
    }
    // A DBF table's memo blocks go back to its end as the last ones are given
    // up, so its records are taken back newest first: both of two appended
    // ones' blocks go (`fixtures/write/fts_tx_vfp_rollback`).
    let dbf = |record: &Vec<Logged>| {
        record[0].shared.file().structure.header.format != tflip_storage::table::Format::Adt
    };
    let (mut dbf_records, adt_records): (Vec<_>, Vec<_>) = records.into_iter().partition(dbf);
    dbf_records.reverse();
    let mut result = Ok(());
    for entry in adt_records
        .into_iter()
        .chain(dbf_records)
        .flat_map(|record| record.into_iter().rev())
    {
        let done = crate::table::roll_back_record(&entry.shared, entry.slot, entry.before);
        if result.is_ok() {
            result = done;
        }
    }
    result
}

/// [`undo`] for a rollback to a savepoint, which takes the writes back
/// newest first, so that the appended records' slots go onto the free list
/// in that order (`fixtures/write/tx_savepoint_lock`).
fn undo_back(log: Vec<Logged>) -> Result<(), Error> {
    let mut result = Ok(());
    for entry in log.into_iter().rev().filter(|entry| !entry.taken) {
        let done = crate::table::roll_back_record(&entry.shared, entry.slot, entry.before);
        if result.is_ok() {
            result = done;
        }
    }
    result
}

/// Rolls back the transaction of `connection`, if it has one, as when it
/// closes.
pub(crate) fn abandon(connection: &Connection) -> Result<(), Error> {
    // Its events and the signals it held go with it.
    crate::events::drop_all(connection);
    let state = std::mem::take(&mut *lock(&connection.transaction));
    roll_back_state(state)
}

/// A command a transaction does not allow, with the reason the original
/// engine gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Illegal {
    Reindex,
    Pack,
    Zap,
    /// Anything else (`AdsSetTableTransactionFree`, SQL's `SET
    /// TRANSACTION`).
    Other,
    /// `ALTER TABLE` that restructures a table, online or not.
    Restructure,
    /// `DROP INDEX` and `AdsDeleteIndex`.
    DeleteIndex,
}

impl Illegal {
    /// The original engine's words for it, after the code's text.
    pub fn reason(self) -> &'static str {
        match self {
            Illegal::Reindex => "Cannot Reindex.",
            Illegal::Pack => "Can't Pack in a transaction.",
            Illegal::Zap => "Can't Zap in a transaction.",
            Illegal::Restructure => "Cannot restructure a table from within a transaction.",
            Illegal::DeleteIndex => "Can't delete index orders.",
            Illegal::Other => "",
        }
    }
}

/// Whether the session has real transactions (else the count alone).
fn real(session: &Session) -> bool {
    settings::real_transactions(session)
}

/// The connections of `session`, or the one behind `handle` if it is not 0.
fn connections(session: &Session, handle: Handle) -> Result<Vec<Arc<Connection>>, Error> {
    if handle != 0 {
        return Ok(vec![connection::get(session, handle)?]);
    }
    let mut all = Vec::new();
    session.for_each(|object| {
        if let Object::Connection(connection) = object {
            all.push(Arc::clone(connection));
        }
    });
    Ok(all)
}

/// The tables open on `owner`.
fn tables_of(session: &Session, owner: &Arc<Connection>) -> Vec<Arc<crate::table::Table>> {
    let mut tables = Vec::new();
    session.for_each(|object| {
        if let Object::Table(table) = object
            && table
                .owner()
                .is_some_and(|connection| Arc::ptr_eq(connection, owner))
        {
            tables.push(Arc::clone(table));
        }
    });
    tables
}

/// Writes, in the transaction of `owner` ending now, the records the
/// tables on it have changed and not written yet: as the original's server
/// does, the commit writes them, and a rollback takes them back. One that
/// cannot be written is the error; with `discard`, its change is dropped
/// instead.
fn write_pending(session: &Session, owner: &Arc<Connection>, discard: bool) -> Result<(), Error> {
    for table in tables_of(session, owner) {
        // A rollback writes nothing of them, memo values included
        // (`fixtures/write/fts_tx_rollback`).
        if discard {
            drop(table.cancel_update());
            continue;
        }
        if let Err(error) = table.flush_pending() {
            match discard {
                true => drop(table.cancel_update()),
                false => return Err(error),
            }
        }
    }
    Ok(())
}

/// Begins a transaction on `connection`, or nests one in the transaction it
/// has (`AdsBeginTransaction`); on every connection for 0. The connection's
/// autocommit mode (`SET TRANSACTION`) becomes `EXPLICIT`, as the help says
/// (`master_set_transaction`) and the original's server does (`tx_sql`).
pub fn begin(session: &Session, connection: Handle) -> Result<(), Error> {
    for owner in connections(session, connection)? {
        *lock(&owner.autocommit) = Autocommit::Explicit;
        let mut state = lock(&owner.transaction);
        if state.count == 0 {
            state.id = next_id();
            state.started = crate::clock::now().local;
        }
        state.count += 1;
    }
    Ok(())
}

/// Commits `connection`'s transaction, or ends one level of it
/// (`AdsCommitTransaction`); for 0, the transactions of every connection
/// that has one. [`Error::NotInTransaction`] without one.
pub fn commit(session: &Session, connection: Handle) -> Result<(), Error> {
    let mut ended = false;
    let mut result = Ok(());
    for owner in connections(session, connection)? {
        let (count, tainted) = {
            let state = lock(&owner.transaction);
            (state.count, state.tainted)
        };
        if count == 0 {
            continue;
        }
        ended = true;
        if tainted {
            result = Err(Error::AgentFailed);
        }
        if count == 1
            && real(session)
            && let Err(error) = write_pending(session, &owner, false)
        {
            if result.is_ok() {
                result = Err(error);
            }
            continue;
        }
        let mut state = lock(&owner.transaction);
        // Another thread may have ended it meanwhile.
        if state.count == 0 {
            continue;
        }
        state.count -= 1;
        if state.count > 0 {
            continue;
        }
        let taken = std::mem::take(&mut *state);
        drop(state);
        let (done, lost) = match commit_state(taken) {
            Ok(lost) => (Ok(()), lost),
            Err(error) => (Err(error), false),
        };
        match done {
            Ok(()) => crate::events::committed(&owner),
            Err(_) => crate::events::rolled_back(&owner),
        }
        let done = match lost {
            true => Err(Error::LockLost),
            false => done,
        };
        if result.is_ok() {
            result = done;
        }
    }
    match ended {
        true => result,
        false => Err(Error::NotInTransaction),
    }
}

/// Rolls `connection`'s transaction back, every level of it
/// (`AdsRollbackTransaction`), or to the savepoint `to`, which stays, as do
/// the transaction and its count (`AdsRollbackTransaction80`). For 0, the
/// transactions of every connection that has one. [`Error::NotInTransaction`]
/// without one; [`Error::SavepointNotFound`] for a savepoint it does not have
/// (in the original's local server's mode, any savepoint is rolled back
/// to).
pub fn rollback(session: &Session, connection: Handle, to: Option<&str>) -> Result<(), Error> {
    let mut ended = false;
    let mut result = Ok(());
    for owner in connections(session, connection)? {
        if to.is_none() && real(session) && lock(&owner.transaction).count > 0 {
            write_pending(session, &owner, true)?;
        }
        let mut state = lock(&owner.transaction);
        if state.count == 0 {
            continue;
        }
        ended = true;
        if state.tainted && result.is_ok() {
            result = Err(Error::AgentFailed);
        }
        let done = match to {
            None => {
                let taken = std::mem::take(&mut *state);
                drop(state);
                crate::events::rolled_back(&owner);
                let freed = appended_slots(&taken.log);
                let done = roll_back_state(taken);
                for table in tables_of(session, &owner) {
                    table.freed_by_rollback(&freed);
                }
                done
            }
            Some(name) => {
                let at = state
                    .savepoints
                    .iter()
                    .position(|(s, _)| s.eq_ignore_ascii_case(name));
                let mut freed = Vec::new();
                let log = match at {
                    Some(at) => {
                        let length = state.savepoints[at].1;
                        state.savepoints.truncate(at + 1);
                        let undone = length < state.log.len();
                        let logged = match &mut state.disk {
                            Some(disk) if undone => disk.rolled_back_to(length),
                            _ => Ok(()),
                        };
                        logged.map_err(|_| Error::TransactionLog)?;
                        let undone = state.log.split_off(length);
                        freed = appended_slots(&undone);
                        let taken_back = undone.iter().filter(|entry| entry.locked && !entry.taken);
                        let lost: Vec<_> = taken_back
                            .map(|entry| (Arc::clone(&entry.shared), entry.slot))
                            .collect();
                        state.lost.extend(lost);
                        undone
                    }
                    None if real(session) => return Err(Error::SavepointNotFound),
                    None => Vec::new(),
                };
                drop(state);
                // Before the writes are taken back: the handles read the
                // images they stand on through the transaction.
                for table in tables_of(session, &owner) {
                    table.keep_freed(&freed);
                }
                undo_back(log)
            }
        };
        if result.is_ok() {
            result = done;
        }
    }
    match ended {
        true => result,
        false => Err(Error::NotInTransaction),
    }
}

/// The slots `log` appended.
fn appended_slots(log: &[Logged]) -> Vec<(Arc<Shared>, u32)> {
    log.iter()
        .filter(|entry| entry.appended && !entry.taken)
        .map(|entry| (Arc::clone(&entry.shared), entry.slot))
        .collect()
}

/// The call of an external procedure's library that `connection` is in,
/// with real transactions, and the connection.
fn library_call(session: &Session, connection: Handle) -> Option<(Arc<Connection>, Call)> {
    if connection == 0 || !real(session) {
        return None;
    }
    let owner = connection::get(session, connection).ok()?;
    let call = lock(&owner.calls)
        .last()
        .cloned()
        .filter(|call| call.library)?;
    Some((owner, call))
}

/// A library's call that the remote server refuses with 5047: in the
/// caller's transaction it spoils it (see [`State::tainted`]).
fn refused(owner: &Connection) -> Error {
    let mut state = lock(&owner.transaction);
    state.tainted = state.count > 0;
    Error::NotInTransaction
}

/// `AdsCommitTransaction`, as a library of an external procedure that calls
/// it on the handle it is given gets it from the original's remote server
/// (`fixtures/sql/aeprulesremote.json`): it commits only the levels it began
/// itself (5047 otherwise, even in the caller's transaction).
pub fn api_commit(session: &Session, connection: Handle) -> Result<(), Error> {
    if let Some((owner, call)) = library_call(session, connection)
        && lock(&owner.transaction).count <= call.entry
    {
        return Err(refused(&owner));
    }
    commit(session, connection)
}

/// `AdsRollbackTransaction` as [`api_commit`] has it: a library may roll
/// back only a transaction it began, not one nested in the caller's (5047,
/// the levels it began ending without undoing anything).
pub fn api_rollback(session: &Session, connection: Handle, to: Option<&str>) -> Result<(), Error> {
    if to.is_none()
        && let Some((owner, call)) = library_call(session, connection)
    {
        let mut state = lock(&owner.transaction);
        if state.count <= call.entry || call.entry > 0 {
            if state.count > call.entry {
                state.count -= 1;
            }
            drop(state);
            return Err(refused(&owner));
        }
    }
    rollback(session, connection, to)
}

/// `AdsInTransaction`: a library whose call was refused sees only the levels
/// it began.
pub fn api_active(session: &Session, connection: Handle) -> Result<bool, Error> {
    if let Some((owner, call)) = library_call(session, connection) {
        let state = lock(&owner.transaction);
        if state.tainted {
            return Ok(state.count > call.entry);
        }
    }
    active(session, connection)
}

/// Sets the savepoint `name` in `connection`'s transaction
/// (`AdsCreateSavepoint`); one of that name goes first.
/// [`Error::NotInTransaction`] without a transaction.
pub fn savepoint(session: &Session, connection: Handle, name: &str) -> Result<(), Error> {
    let owner = connection::get(session, connection)?;
    let mut state = lock(&owner.transaction);
    if state.count == 0 {
        return Err(Error::NotInTransaction);
    }
    state
        .savepoints
        .retain(|(s, _)| !s.eq_ignore_ascii_case(name));
    let length = state.log.len();
    state.savepoints.push((name.to_owned(), length));
    Ok(())
}

/// `connection`'s nesting level (`AdsGetTransactionCount`): 0 outside a
/// transaction.
pub fn count(session: &Session, connection: Handle) -> Result<u32, Error> {
    Ok(lock(&connection::get(session, connection)?.transaction).count)
}

/// When the oldest transaction running on a connection of the session began
/// (`::server.OldestActiveTxn`).
pub(crate) fn oldest_active(session: &Session) -> Option<(i32, u32)> {
    let mut oldest = None;
    session.for_each(|object| {
        if let Object::Connection(connection) = object {
            let state = lock(&connection.transaction);
            if state.count > 0 && oldest.is_none_or(|o| state.started < o) {
                oldest = Some(state.started);
            }
        }
    });
    oldest
}

/// Whether `connection` is in a transaction (`AdsInTransaction`); for 0,
/// whether any connection is.
pub fn active(session: &Session, connection: Handle) -> Result<bool, Error> {
    let all = connections(session, connection)?;
    Ok(all.iter().any(|owner| lock(&owner.transaction).count > 0))
}

/// [`Error::IllegalInTransaction`] for `command` if `connection` is in a
/// real transaction.
pub fn check_legal(session: &Session, connection: Handle, command: Illegal) -> Result<(), Error> {
    if connection == 0 || !real(session) {
        return Ok(());
    }
    match count(session, connection)? {
        0 => Ok(()),
        _ => Err(Error::IllegalInTransaction(command)),
    }
}

/// A stored procedure running on a connection: the nesting level it was
/// called at, and the savepoints then set. What a procedure does with the
/// transaction is its own levels' (see [`call_begins`]).
#[derive(Debug, Clone)]
pub(crate) struct Call {
    entry: u32,
    savepoints: usize,
    /// The record an `INSERT` wrote, while the `AFTER INSERT` triggers fire
    /// on it: a trigger's rollback to its own savepoint takes it back (see
    /// [`insert_fires`]).
    insert: Option<(Arc<Shared>, u32)>,
    /// An external procedure's library, whose calls on the handle it is given
    /// answer as the remote server's do ([`api_commit`]).
    library: bool,
}

/// Begins a stored procedure's call on `connection`, with real
/// transactions: the procedure may end the levels it begins (`COMMIT`, and
/// `ROLLBACK` if the caller has no transaction), but none of the caller's, and
/// roll back to its own savepoints only (the original's server answers 5047
/// and 7129). [`call_ends`] ends it.
pub(crate) fn call_begins(session: &Session, connection: Handle) -> Result<(), Error> {
    call_begins_as(session, connection, false)
}

/// [`call_begins`] for a procedure that is a library's function (`library`)
/// or not.
pub(crate) fn call_begins_as(
    session: &Session,
    connection: Handle,
    library: bool,
) -> Result<(), Error> {
    if connection == 0 || !real(session) {
        return Ok(());
    }
    let owner = connection::get(session, connection)?;
    let state = lock(&owner.transaction);
    let call = Call {
        entry: state.count,
        savepoints: state.savepoints.len(),
        insert: None,
        library,
    };
    drop(state);
    lock(&owner.calls).push(call);
    Ok(())
}

/// Ends the call [`call_begins`] began, when the procedure ended (`ok`) or
/// failed: its savepoints go, and the levels it left open end: committed (with
/// the caller's own transaction, if it had one, going on), or, when it
/// failed, rolled back if the caller had no transaction; in the caller's,
/// what it wrote stays (`fixtures/sql/txprocs.json`).
pub(crate) fn call_ends(session: &Session, connection: Handle, ok: bool) -> Result<(), Error> {
    if connection == 0 || !real(session) {
        return Ok(());
    }
    let owner = connection::get(session, connection)?;
    let Some(call) = lock(&owner.calls).pop() else {
        return Ok(());
    };
    {
        let mut state = lock(&owner.transaction);
        state.savepoints.truncate(call.savepoints);
        if state.count <= call.entry {
            return Ok(());
        }
        if !ok && call.entry > 0 {
            state.count = call.entry;
            return Ok(());
        }
    }
    if !ok {
        return match rollback(session, connection, None) {
            Err(Error::NotInTransaction) => Ok(()),
            other => other,
        };
    }
    while lock(&owner.transaction).count > call.entry {
        match commit(session, connection) {
            Ok(()) | Err(Error::AgentFailed) => {}
            error => return error,
        }
    }
    Ok(())
}

/// Tells the call `connection` is in (the triggers of a record's change) that
/// its `AFTER INSERT` triggers fire on the record in slot `slot` of the table
/// file `shared`, which the `INSERT` has written. The original's `INSERT`
/// logs it after its `AFTER` triggers ran, so that a trigger's rollback to a
/// savepoint it set takes the record back, with what the trigger wrote after
/// the savepoint (`fixtures/sql/txtrig2.json`); what the trigger wrote
/// before it stays. A change that writes the record first and fires
/// afterwards logs it first, so this takes it back by hand
/// ([`take_back_insert`]).
pub(crate) fn insert_fires(session: &Session, connection: Handle, shared: Arc<Shared>, slot: u32) {
    if connection == 0 || !real(session) {
        return;
    }
    if let Ok(owner) = connection::get(session, connection)
        && let Some(call) = lock(&owner.calls).last_mut()
    {
        call.insert = Some((shared, slot));
    }
}

/// Takes back the record the call's `AFTER INSERT` triggers fire on, if a
/// rollback to a savepoint in one of them did (see [`insert_fires`]): what
/// the transaction wrote of it is undone, and its commit and any later
/// rollback leave it alone.
fn take_back_insert(session: &Session, connection: Handle) -> Result<(), Error> {
    let Ok(owner) = connection::get(session, connection) else {
        return Ok(());
    };
    let Some((shared, slot)) = lock(&owner.calls)
        .last_mut()
        .and_then(|call| call.insert.take())
    else {
        return Ok(());
    };
    let mut state = lock(&owner.transaction);
    let mut written = Vec::new();
    for entry in state.log.iter_mut() {
        if Arc::ptr_eq(&entry.shared, &shared) && entry.slot == slot && !entry.taken {
            entry.taken = true;
            written.push(Logged {
                shared: Arc::clone(&entry.shared),
                slot,
                before: entry.before.clone(),
                locked: false,
                appended: false,
                taken: false,
            });
        }
    }
    drop(state);
    undo(written)
}

/// A transaction statement of SQL on `connection`: as the API's calls with
/// real transactions; in the original's local server's mode, accepted in any
/// order without changing anything.
pub(crate) fn sql(
    session: &Session,
    connection: Handle,
    statement: &tflip_sql::ast::Transaction,
) -> Result<(), Error> {
    use tflip_sql::ast::Transaction as T;
    if !real(session) {
        return Ok(());
    }
    let owner = connection::get(session, connection).ok();
    let call = owner
        .as_ref()
        .and_then(|owner| lock(&owner.calls).last().cloned());
    if let (Some(call), Some(owner)) = (call, &owner) {
        let state = lock(&owner.transaction);
        // A procedure ends its own levels only, and rolls back what the
        // caller began never.
        let refused = match statement {
            T::Commit => state.count <= call.entry,
            T::Rollback(None) => state.count <= call.entry || call.entry > 0,
            _ => false,
        };
        if refused {
            return Err(sql_error(Error::NotInTransaction));
        }
        if let T::Rollback(Some(name)) = statement {
            let own = state
                .savepoints
                .iter()
                .skip(call.savepoints)
                .any(|(s, _)| s.eq_ignore_ascii_case(name));
            if state.count > 0 && !own {
                return Err(sql_error(Error::SavepointNotFound));
            }
        }
    }
    // A library outside the caller's transaction that ends the one it began
    // with SQL ends it, and is told 5047 (`fixtures/sql/aeprulesremote.json`).
    let own_end = matches!(statement, T::Commit | T::Rollback(None))
        && library_call(session, connection)
            .is_some_and(|(owner, call)| call.entry == 0 && lock(&owner.transaction).count > 0);
    if own_end {
        match statement {
            T::Commit => commit(session, connection),
            _ => rollback(session, connection, None),
        }
        .map_err(sql_error)?;
        return Err(Error::NotInTransaction);
    }
    match statement {
        T::Begin => begin(session, connection),
        T::Commit => commit(session, connection),
        T::Rollback(to) => rollback(session, connection, to.as_deref()).and_then(|()| {
            // A trigger that rolls back to a savepoint it set takes back
            // the record it fires on, if an INSERT wrote it.
            match to {
                Some(_) => take_back_insert(session, connection),
                None => Ok(()),
            }
        }),
        T::Savepoint(name) => savepoint(session, connection, name),
        T::Set(mode) => check_legal(session, connection, Illegal::Other).and_then(|()| {
            let mode = match mode.as_str() {
                "AUTOCOMMIT_ON" => Autocommit::On,
                "AUTOCOMMIT_OFF" => Autocommit::Off,
                _ => Autocommit::Explicit,
            };
            *lock(&connection::get(session, connection)?.autocommit) = mode;
            Ok(())
        }),
    }
    .map_err(sql_error)
}

/// SQL's autocommit mode on a connection (`SET TRANSACTION`, see
/// `master_set_transaction`), with real transactions: whether an `INSERT`,
/// `UPDATE` or `DELETE` run outside a transaction is put in one of its own.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Autocommit {
    /// Only the client's transactions (the mode a connection starts in,
    /// and the one a transaction begun by the client leaves).
    #[default]
    Explicit,
    /// Each statement in a transaction committed after it, or rolled back
    /// if it fails.
    On,
    /// Each statement begins a transaction, left for the client to end; one
    /// that fails takes it back.
    Off,
}

/// A transaction a statement began for its connection's autocommit mode
/// (see [`statement_begins`]).
pub(crate) struct Implicit {
    connection: Handle,
    commit: bool,
}

/// Begins the transaction `connection`'s autocommit mode puts an `INSERT`,
/// `UPDATE` or `DELETE` in, if it is not in one already: then the statement
/// is to end with [`statement_ends`].
pub(crate) fn statement_begins(
    session: &Session,
    connection: Handle,
) -> Result<Option<Implicit>, Error> {
    if connection == 0 || !real(session) {
        return Ok(None);
    }
    let owner = connection::get(session, connection)?;
    let commit = match *lock(&owner.autocommit) {
        Autocommit::Explicit => return Ok(None),
        Autocommit::On => true,
        Autocommit::Off => false,
    };
    let mut state = lock(&owner.transaction);
    if state.count > 0 {
        return Ok(None);
    }
    state.id = next_id();
    state.started = crate::clock::now().local;
    state.count = 1;
    Ok(Some(Implicit { connection, commit }))
}

/// Ends the transaction a statement began (see [`statement_begins`]) as the
/// statement ended (`ok`): committed with `AUTOCOMMIT_ON`, kept with
/// `AUTOCOMMIT_OFF`, rolled back if it failed (`fixtures/write/tx_sql`).
pub(crate) fn statement_ends(session: &Session, implicit: Implicit, ok: bool) -> Result<(), Error> {
    match (ok, implicit.commit) {
        (true, true) => commit(session, implicit.connection),
        (true, false) => Ok(()),
        (false, _) => rollback(session, implicit.connection, None),
    }
}

/// The transaction a record's change and the triggers it fires run in (see
/// [`trigger_begins`]): what the connection's transaction was before it.
pub(crate) struct Nested {
    connection: Handle,
    /// The nesting level before, and the lengths of the log and of the
    /// savepoints.
    count: u32,
    log: usize,
    savepoints: usize,
}

/// Begins the transaction a record's change runs in with the triggers it
/// fires, with real transactions: two levels deeper in the connection's
/// own, or a new one (a trigger reads `::conn.TransactionCount` 2 outside
/// a transaction, as the original's server has it, `fixtures/sql/triggers.json`).
/// [`trigger_ends`] ends it.
pub(crate) fn trigger_begins(
    session: &Session,
    connection: Handle,
) -> Result<Option<Nested>, Error> {
    if connection == 0 || !real(session) {
        return Ok(None);
    }
    let owner = connection::get(session, connection)?;
    let mut state = lock(&owner.transaction);
    let nested = Nested {
        connection,
        count: state.count,
        log: state.log.len(),
        savepoints: state.savepoints.len(),
    };
    if state.count == 0 {
        state.id = next_id();
        state.started = crate::clock::now().local;
    }
    state.count += 2;
    // The triggers are a call as a procedure is: they end their own levels
    // only, and roll back to their own savepoints only.
    let call = Call {
        entry: state.count,
        savepoints: state.savepoints.len(),
        insert: None,
        library: false,
    };
    drop(state);
    lock(&owner.calls).push(call);
    Ok(Some(nested))
}

/// Ends the transaction [`trigger_begins`] began: if the change and its
/// triggers went through (`ok`), its two levels are committed (the whole
/// transaction, if it was a new one); if not, what they wrote is taken back
/// and the connection's transaction, if it had one, goes on as it was
/// before (a failing trigger does not end the client's transaction).
pub(crate) fn trigger_ends(session: &Session, nested: Nested, ok: bool) -> Result<(), Error> {
    let connection = nested.connection;
    if let Ok(owner) = connection::get(session, connection)
        && let Some(call) = lock(&owner.calls).pop()
    {
        lock(&owner.transaction)
            .savepoints
            .truncate(call.savepoints);
        // What the triggers began and did not end ends with them.
        while ok && lock(&owner.transaction).count > call.entry {
            commit(session, connection)?;
        }
    }
    if ok {
        commit(session, connection)?;
        return commit(session, connection);
    }
    if nested.count == 0 {
        return match rollback(session, connection, None) {
            Err(Error::NotInTransaction) => Ok(()),
            other => other,
        };
    }
    let owner = connection::get(session, connection)?;
    let mut state = lock(&owner.transaction);
    if state.count == 0 {
        return Ok(());
    }
    let length = nested.log.min(state.log.len());
    if length < state.log.len()
        && let Some(disk) = &mut state.disk
    {
        disk.rolled_back_to(length)
            .map_err(|_| Error::TransactionLog)?;
    }
    let log = state.log.split_off(length);
    state.savepoints.truncate(nested.savepoints);
    state.count = nested.count;
    drop(state);
    undo(log)
}

/// Rolls back what a script that failed left open on `connection`, if it
/// began a transaction (the client had none when it started).
pub(crate) fn script_failed(session: &Session, connection: Handle) {
    if real(session) {
        let _ = rollback(session, connection, None);
    }
}

/// The SQL error for a transaction's error, as the original engine words it.
pub(crate) fn sql_error(error: Error) -> Error {
    use tflip_sql::SqlError;
    match error {
        Error::NotInTransaction => Error::Sql(SqlError::ace(
            5047,
            "The transaction command was not in valid sequence.",
            "",
        )),
        Error::SavepointNotFound => {
            Error::Sql(SqlError::ace_bare(7129, "The savepoint was not found."))
        }
        Error::AgentFailed => Error::Sql(SqlError::ace_bare(
            7207,
            "Unexpected error from Advantage Server Agent.  Check Advantage error log for details.",
        )),
        Error::IllegalInTransaction(command) => Error::Sql(SqlError::ace(
            5063,
            "The command specified is illegal within a transaction.",
            command.reason(),
        )),
        error => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connect(session: &Session) -> Handle {
        connection::connect(session, "Data Source=x;ServerType=LOCAL;").unwrap()
    }

    /// Connection 0 begins on every connection, and commits or rolls back
    /// those that have a transaction; as on the original's remote server.
    #[test]
    fn connection_zero_is_every_connection() {
        let session = Session::new();
        let (a, b) = (connect(&session), connect(&session));
        assert_eq!(commit(&session, 0), Err(Error::NotInTransaction));
        assert!(!active(&session, 0).unwrap());
        begin(&session, a).unwrap();
        begin(&session, 0).unwrap();
        assert_eq!((count(&session, a), count(&session, b)), (Ok(2), Ok(1)));
        assert!(active(&session, 0).unwrap());
        commit(&session, 0).unwrap();
        assert_eq!((count(&session, a), count(&session, b)), (Ok(1), Ok(0)));
        rollback(&session, 0, None).unwrap();
        assert_eq!(count(&session, a), Ok(0));
        assert_eq!(rollback(&session, 0, None), Err(Error::NotInTransaction));
    }

    /// Savepoints: by name regardless of case, a new one of a name replacing
    /// the old, a rollback to one keeping it and the transaction.
    #[test]
    fn savepoints() {
        let session = Session::new();
        let c = connect(&session);
        assert_eq!(savepoint(&session, c, "a"), Err(Error::NotInTransaction));
        begin(&session, c).unwrap();
        savepoint(&session, c, "a").unwrap();
        savepoint(&session, c, "b").unwrap();
        rollback(&session, c, Some("A")).unwrap();
        assert_eq!(
            rollback(&session, c, Some("b")),
            Err(Error::SavepointNotFound)
        );
        rollback(&session, c, Some("a")).unwrap();
        assert_eq!(count(&session, c), Ok(1));
        commit(&session, c).unwrap();
        assert_eq!(
            rollback(&session, c, Some("a")),
            Err(Error::NotInTransaction)
        );
    }

    /// In the original local server's mode, an unknown savepoint is rolled
    /// back to, SQL's statements change nothing, and no command is illegal.
    #[test]
    fn counting_only() {
        use tflip_sql::ast::Transaction as T;
        let session = Session::new();
        settings::set_real_transactions(&session, false);
        let c = connect(&session);
        sql(&session, c, &T::Commit).unwrap();
        sql(&session, c, &T::Begin).unwrap();
        assert_eq!(count(&session, c), Ok(0));
        begin(&session, c).unwrap();
        rollback(&session, c, Some("nosuch")).unwrap();
        assert_eq!(count(&session, c), Ok(1));
        check_legal(&session, c, Illegal::Pack).unwrap();
        settings::set_real_transactions(&session, true);
        assert_eq!(
            check_legal(&session, c, Illegal::Pack),
            Err(Error::IllegalInTransaction(Illegal::Pack))
        );
    }
}
