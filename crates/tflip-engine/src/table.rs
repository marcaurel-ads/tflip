//! Tables.
//!
//! A table is an object behind a handle that knows its file and how it was
//! opened. ADT and DBF tables have their structure (header and fields) read
//! when they are opened, keep their file open for the records, and have a
//! position: a current record, or before the first or after the last one.
//! On a connection to a data dictionary, tables and views open by their
//! names in it (see [`crate::dictionary`]). A table of type
//! [`TableType::Database`] that is not in a dictionary is refused.
//!
//! This module has the table, its state and its moves, in record order and
//! in index orders. The rest is in its submodules: opening and closing
//! tables (`opening`), field values (`values`), writing records and locks
//! (`write`), keys of index orders (`keys`), AOFs, record filters and
//! `AdsLocate` (`filtering`), and packing, recalling and raw records
//! (`maintenance`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

use serde::{Deserialize, Serialize};
use tflip_expr::{Env, Expr, Type};
use tflip_storage::dbf;
use tflip_storage::field::{Field, FieldType, Layout};
use tflip_storage::memo::MemoFile;
use tflip_storage::order::{IndexFile as OrderFile, Kind};
use tflip_storage::table::{Error as StorageError, Format, RecordStatus, Structure, TableFile};
use tflip_storage::value::{Value, Ymd};

use crate::connection::Connection;
use crate::filter::{self, Aof, AofOptions, Bits, Evaluation, Filter, Filters, Members, Sieve};
use crate::handle::{Handle, Object};
use crate::index::{self, Index, Place, Scope};
use crate::seek::{self, SeekType, SeekValue};
use crate::setters::{self, SetValue};
use crate::settings::Settings;
use crate::shared::{self, Owner, Seen, Shared, SharedIndex};
use crate::{Error, Session, connection, format, lock, shares};

mod filtering;
mod keys;
pub(crate) use keys::cdx_segment_length;
mod maintenance;
mod opening;
mod tx;
mod values;
mod write;

pub(crate) use tx::{commit_record, commit_words, roll_back_record};

pub(crate) use opening::{
    DATABASE_TABLE, built, changing, dictionary, load, open_info, register, source, stem,
    transient, transient_deciphered, with_file,
};
pub(crate) use opening::{ID_MISMATCH, MISSING_INDEX_FILE, opening_free_itself};
pub use opening::{
    clone_table, close, delete, get, open, open_for_client, open_options, open_rebuilding,
    standalone, write_all,
};
use write::{
    OrderKey, encode_into, maintain, maintain_recalled, order_file, order_keys, order_keys_with,
    write_error, write_pending,
};

/// A record's fields and their values.
pub type Row = (Vec<Field>, Vec<tflip_sql::V>);

/// The file format of a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TableType {
    /// A table in a data dictionary, whatever its format.
    Database,
    /// DBF with Clipper (`.ntx`) indexes.
    Ntx,
    /// DBF with FoxPro (`.cdx`) indexes.
    Cdx,
    /// Advantage's own format (`.adt`).
    Adt,
    /// Visual FoxPro DBF.
    Vfp,
}

impl TableType {
    /// The name of the ACE constant for the type, as the original engine's
    /// error texts give it (`ADS_NTX`).
    pub fn ace_name(self) -> &'static str {
        match self {
            TableType::Database => "ADS_DATABASE_TABLE",
            TableType::Ntx => "ADS_NTX",
            TableType::Cdx => "ADS_CDX",
            TableType::Adt => "ADS_ADT",
            TableType::Vfp => "ADS_VFP",
        }
    }
}

impl From<dbf::Flavour> for TableType {
    fn from(flavour: dbf::Flavour) -> TableType {
        match flavour {
            dbf::Flavour::Clipper => TableType::Ntx,
            dbf::Flavour::FoxPro => TableType::Cdx,
            dbf::Flavour::Visual => TableType::Vfp,
        }
    }
}

/// How a table is opened, as the client asked for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSettings {
    /// The type to open the table as.
    pub table_type: TableType,
    /// The character set, as the client gave it (for ACE: `ADS_ANSI`,
    /// `ADS_OEM` or a VFP collation number). Not interpreted yet.
    pub char_type: u16,
    /// The collation, by name (such as `ansi`); empty if none was given. Not
    /// interpreted yet.
    pub collation: String,
    /// The open options, as the client gave them (for ACE: `ADS_EXCLUSIVE`,
    /// `ADS_READONLY`, ... flags). Not interpreted yet.
    pub options: u32,
    /// The alias the client gave the table; empty for none (see
    /// [`TableInfo::alias`]).
    pub alias: String,
    /// The rights checking asked for, as the client gave it (for ACE:
    /// `ADS_CHECKRIGHTS` or `ADS_IGNORERIGHTS`). Not interpreted yet.
    pub rights: u16,
    /// The encryption password to open an encrypted table with, if the client
    /// gave the connection one; empty for none. Kept by the client and used in
    /// process, never sent to a server (like the connection password it comes
    /// from), so it stays out of the wire format.
    #[serde(skip)]
    pub password: String,
    /// Whether the table is a static cursor whose text is in the client's
    /// OEM code page (what a `SELECT` of OEM tables only gives), which no
    /// DBF format says of it.
    #[serde(skip)]
    pub oem_text: bool,
    /// Whether a statement opens the table: one with an encryption password
    /// of its own that it was not given is refused where an `AdsOpenTable`
    /// is not (5098).
    #[serde(skip)]
    pub by_statement: bool,
}

impl Default for TableSettings {
    /// An ADT table, ANSI, with no collation, options or alias, checking
    /// rights.
    fn default() -> Self {
        TableSettings {
            table_type: TableType::Adt,
            char_type: 1,
            collation: String::new(),
            options: 0,
            alias: String::new(),
            rights: 1,
            password: String::new(),
            oem_text: false,
            by_statement: false,
        }
    }
}

/// An open table.
#[derive(Debug)]
pub struct Table {
    /// The connection the table was opened on, or 0 if it was opened on its
    /// own.
    pub connection: Handle,
    /// The table's file.
    pub path: PathBuf,
    /// How the table was opened.
    pub settings: TableSettings,
    /// The structure of the table.
    pub structure: Structure,
    /// The position and the records, which navigation changes and reads.
    state: Mutex<State>,
    /// The connection the table was opened on, if any.
    owner: Option<Arc<Connection>>,
    /// The settings of the session the table belongs to.
    session_settings: Arc<Settings>,
    /// The table's "exact" setting (see [`crate::settings::exact`]).
    pub(crate) exact: AtomicBool,
    /// The table's AOF and filter. Moves and counts take them as they are
    /// when the call starts (see [`Table::sieve`]).
    filters: Mutex<Arc<Filters>>,
    /// The SQL statement whose cursor the table is, if it is one.
    statement: Option<Handle>,
    /// The fields of the table a live SQL cursor shows some of, all of
    /// them: its orders' key expressions name them.
    all_fields: Option<Vec<Field>>,
    /// The order a live SQL cursor reads its records in, by name: the
    /// table's order its `ORDER BY` is, or the temporary index made for it.
    cursor_order: Option<String>,
    /// Whether the table was built in memory (a static SQL cursor).
    built: bool,
    /// Whether a static cursor left rows out, which the original engine then
    /// counts no records of.
    uncounted: bool,
    /// An empty record of the table: for a live SQL cursor, of the table it
    /// reads, all of whose fields its records hold.
    null_record: Vec<u8>,
    /// What the header of a DBF table says besides its structure.
    pub dbf: Option<dbf::Info>,
    /// For a table of a data dictionary, its index files, which open with
    /// it instead of the structural one.
    pub(crate) index_files: Option<Vec<PathBuf>>,
    /// For a table of a data dictionary, the name of its primary key's
    /// index.
    pub(crate) primary_key: Option<String>,
    /// For a table of a data dictionary, its object id there, which its
    /// records' `ROWID`s name it by.
    pub(crate) object_id: Option<u32>,
    /// The table's alias (see [`TableInfo::alias`]); unset for an SQL
    /// cursor, which is called as its file.
    pub(crate) alias: OnceLock<String>,
    /// Whether the table was opened by name ([`open`]), a view among them,
    /// rather than returned by an SQL statement.
    pub(crate) opened: AtomicBool,
    /// The AutoInc value of the last record this handle appended
    /// (`AdsGetLastAutoinc`); 0 before the first.
    last_autoinc: AtomicU32,
    /// The relations the table is the parent of (see [`crate::relation`]).
    pub(crate) relations: Mutex<Vec<Arc<crate::relation::Relation>>>,
    /// The relation the table, its child, is to follow at its next use: its
    /// parent moved.
    pub(crate) following: Mutex<Option<Arc<crate::relation::Relation>>>,
    /// Whether the table counts as having its shared file open (see
    /// [`Shared::attach`]).
    attached: bool,
    /// Which of its connection's caches may keep its file open after it
    /// closes (see [`crate::cache`]): 1 for a table a client opened, 2 for
    /// one a `SELECT` reads, 0 for none (one opened for the engine's own
    /// use).
    kept: std::sync::atomic::AtomicU8,
    /// The rights the connection's user holds on the table (a mask of
    /// `ADS_PERMISSION_*`): `u32::MAX` for a free table, the administrator, or a
    /// dictionary that does not verify access rights; else the user's effective
    /// rights, which gate writing (see [`Table::check_access`]).
    pub(crate) access: u32,
    /// The columns the connection's user may not read, which read as NULL
    /// (see [`Database::column_access`]); empty for most.
    pub(crate) hidden: Vec<String>,
    /// The columns the connection's user may not update, which a set on a
    /// record that was there refuses (5171, `fixtures/sql/ddinsertonly2`).
    pub(crate) not_updatable: Vec<String>,
    /// Whether SQL statements read the columns in `hidden` as NULL too: at
    /// permission levels 2 and 3, where a condition may not name them; at 1
    /// it may, and sees what they hold.
    pub(crate) hidden_in_sql: bool,
    /// Whether only SQL reaches the table, which has columns the user may
    /// not read: permission level 3. A client may not open it, and no
    /// statement shows it live.
    pub(crate) sql_only: bool,
    /// The referential integrity rules of the dictionary that bear on the table,
    /// loaded at open; empty for a free table or one in no rule (see
    /// [`crate::ri`]).
    pub(crate) ri: crate::ri::Rules,
    /// The dictionary's defaults, field constraints and record validation on
    /// the table, loaded at open (see [`crate::constraints`]); `None` for a
    /// free table or one without them.
    pub(crate) checks: Option<Arc<crate::constraints::Checks>>,
    /// Whether the handle has given the table's encryption password: opened
    /// with it, or by `AdsEnableEncryption`. As in the original engine, the
    /// password is the handle's: one that has not given it reads and writes
    /// the file sealed (see [`crate::shared::Sealing`]), whatever other
    /// handles on it gave.
    keyed: AtomicBool,
    /// The index files a user without `ALTER` made for the table
    /// (`AdsCreateIndex`): temporary ones, deleted when the table closes
    /// (`fixtures/dictionary/freetables.json`).
    pub(crate) temporary_indexes: Mutex<Vec<PathBuf>>,
}

/// What does not change about an open table, for a client to keep: how it
/// was opened and its structure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableInfo {
    /// The connection the table was opened on, or 0 if it was opened on its
    /// own.
    pub connection: Handle,
    /// The table's file, as the server names it.
    pub path: String,
    /// How the table was opened.
    pub settings: TableSettings,
    /// The structure of the table.
    pub structure: Structure,
    /// The block size of the table's memo file, if it has one.
    pub memo_block_size: Option<u32>,
    /// An empty record (see [`Table::null_record`]).
    pub null_record: Vec<u8>,
    /// The table's alias: the one the client gave it; else a data
    /// dictionary's table is called as it was named, a view as its
    /// cursor's file, and any other table as the name it was opened by, or
    /// the file of an SQL cursor, without folders and extension.
    pub alias: String,
}

/// Where a table is. Off a record, `bof` means before the first record and
/// `eof` after the last one; an empty table is at both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    /// The current record's slot (1-based record number), if any.
    pub record: Option<u32>,
    pub bof: bool,
    pub eof: bool,
}

/// A table's state, locked; when it is let go after the table moved, the
/// children of the table's relations are told (see [`crate::relation`]).
pub(crate) struct StateGuard<'t> {
    guard: MutexGuard<'t, State>,
    table: &'t Table,
    before: Position,
    /// The thread works for the handle while its state is locked: the files
    /// are sealed if it has not given the table's password.
    _sealing: crate::shared::Sealing,
}

impl std::ops::Deref for StateGuard<'_> {
    type Target = State;

    fn deref(&self) -> &State {
        &self.guard
    }
}

impl std::ops::DerefMut for StateGuard<'_> {
    fn deref_mut(&mut self) -> &mut State {
        &mut self.guard
    }
}

impl Drop for StateGuard<'_> {
    fn drop(&mut self) {
        if self.guard.position != self.before {
            let record = self.guard.position.record.unwrap_or(0);
            self.guard.file.stand(self.guard.owner, record);
            crate::relation::moved(self.table);
        }
    }
}

#[derive(Debug)]
pub(crate) struct State {
    position: Position,
    /// Whether the last move was a seek that found its key (`AdsIsFound`);
    /// every other move clears it.
    found: bool,
    /// Whether, off a record, the record numbers are those of EOF even at
    /// BOF: the original engine leaves its record there after counting an
    /// order's keys through a filter (see [`Table::key_count_in`]). Every
    /// move clears it.
    counted_past_end: bool,
    /// The table's file, shared with every handle on it: its records and
    /// memo file.
    file: Arc<Shared>,
    /// The record being changed and not written yet, if any.
    pending: Option<Pending>,
    /// Off a record after its append was cancelled: the record number the
    /// table reports there, as the original engine does (the cancelled
    /// record's). Every move clears it.
    stranded: Option<u32>,
    /// How many records fewer than its file's header counts the table reports:
    /// each time a deleted record the table was left on is written again,
    /// the original counts it as deleted once more though its file does not
    /// (`fixtures/write/deleted_cursor`). An append, which reads the header
    /// again, ends it.
    dropped: u32,
    /// The record the handle stands on, which a rollback to a savepoint took
    /// back (freed): the handle goes on with the image it had, not deleted,
    /// until it moves or writes (`fixtures/write/tx_cursor_after`).
    kept: Option<(u32, Vec<u8>)>,
    /// The record the handle stands on as it read it when it moved there,
    /// which an encrypted table's handle goes on with after
    /// `AdsDisableEncryption` (deciphered), or after the password is given
    /// again (as the file has it) until it moves to another record
    /// (`fixtures/write/dbf_encryption_cached_cdx`).
    image: Option<(u32, Vec<u8>)>,
    /// The record count an ADT handle keeps after such a rollback: it does
    /// not read the header again until it appends.
    stale_count: Option<u32>,
    /// Who holds this handle's locks.
    owner: Owner,
    /// The index orders open on the table, which its writes keep current.
    orders: Vec<Weak<Index>>,
    /// For a DBF table, the settings that say whether its deleted records
    /// are shown (`AdsShowDeleted`, which applies at once).
    deleted_shown: Option<Arc<Settings>>,
    /// The order of the records without an index order, if it is not that
    /// of their numbers: a live SQL cursor's `ORDER BY`.
    sequence: Option<Sequence>,
    /// How a live cursor puts its records in the order of `sequence` again
    /// when a write changes it.
    sorter: Option<Sorter>,
    /// The `WHERE` a constrained cursor's writes must meet.
    constrained: Option<Arc<filter::SqlFilter>>,
    /// The condition and direction of the last `AdsLocate`, which
    /// `AdsContinue` goes on with.
    located: Option<(String, bool)>,
    /// The connection the table was opened on, whose transaction its writes
    /// and reads are in.
    connection: Option<Arc<Connection>>,
    /// What a transaction's log says the table is (see [`crate::txlog`]).
    source: Arc<crate::txlog::Source>,
    /// The dictionary's constraints a record must pass to be written (see
    /// [`crate::constraints`]).
    checks: Option<Arc<crate::constraints::Checks>>,
}

/// A record a client is changing (`AdsAppendRecord`, `AdsSetString`, ...):
/// its bytes as they will be written, which the table reads in place of the
/// record's, until it is written (`AdsWriteRecord`, a move, closing the
/// table) or the changes are cancelled (`AdsCancelUpdate`).
#[derive(Debug)]
struct Pending {
    slot: u32,
    bytes: Vec<u8>,
    /// Whether the record was appended, so it has no RowVersion to renew.
    appended: bool,
    /// The RowVersion an appended record got, which it is written with
    /// whatever a client set.
    row_version: u64,
    /// Whether a field was set.
    changed: bool,
    /// The VFP Money fields set to NULL that a getter read since, by offset:
    /// `AdsGetRecord` shows their bytes, not blanks.
    revealed: Vec<u16>,
    /// The DBF memo fields set to NULL in this change, by offset, which read
    /// as empty until it is written (a VFP field keeps its reference).
    nulled: Vec<u16>,
    /// The text Memo fields of a Clipper or FoxPro table given a binary
    /// value in this change, by offset, which the text getters refuse until
    /// text is set again.
    binary_memos: Vec<u16>,
    /// Whether it is deleted when it is written (`AdsDeleteRecord`, undone
    /// by `AdsRecallRecord`).
    deleted: bool,
    /// Whether an ADT record deleted when it is written keeps the values
    /// given to it.
    kept: bool,
    /// Whether the record was deleted already, the table having been left on
    /// it by a failed `AdsGotoRecord`: it is deleted again when written.
    redeleted: bool,
    /// Whether the record lock was taken for the change, and goes with it.
    implicit_lock: bool,
    /// The values of Memo and NMemo fields (and of COMPRESSED binary ones) set
    /// and not written yet, by the field's offset in the record, in the order
    /// they were set; `None` for NULL. (Other binary values go to the memo
    /// file when they are set, as in the original engine.)
    memos: Vec<(u16, Option<Vec<u8>>)>,
    /// The unique order that refused the record's key, when writing it was
    /// [`Error::UniqueViolation`].
    refused_by: Option<String>,
    /// The orders' keys of the record as it was before a Binary or Image
    /// value went to the memo file at once, over the one the record held: a
    /// full-text order's words come from the old value, which is gone by
    /// the write.
    old_keys: Option<Vec<write::OrderKey>>,
}

/// What a [`Sorter`] reads the table's records by: it calls its argument on
/// each record's slot, bytes and memo file.
pub(crate) type Scanner<'a> = &'a mut dyn FnMut(
    &mut dyn FnMut(u32, &[u8], Option<&mut MemoFile>) -> Result<(), Error>,
) -> Result<(), Error>;

/// The slots of a live cursor's records in the order of its `ORDER BY`,
/// from the table's open orders and its records.
pub(crate) struct Sorter(Arc<SortFn>);

/// What makes a table a live cursor (see [`Table::make_cursor`]).
pub(crate) struct LiveCursor {
    /// The table's fields the cursor shows, maybe fewer and renamed.
    pub(crate) fields: Vec<Field>,
    /// The records it holds are those this holds for.
    pub(crate) sql: Option<filter::SqlFilter>,
    /// The order of its records, if given.
    pub(crate) sequence: Option<Vec<u32>>,
    pub(crate) sorter: Option<Sorter>,
    /// Whether writes through the cursor must keep holding for `sql`.
    pub(crate) constrain: bool,
    pub(crate) order: Option<String>,
}

type SortFn = dyn Fn(&[Arc<Index>], Scanner) -> Result<Vec<u32>, Error> + Send + Sync;

impl Sorter {
    pub(crate) fn new(
        f: impl Fn(&[Arc<Index>], Scanner) -> Result<Vec<u32>, Error> + Send + Sync + 'static,
    ) -> Sorter {
        Sorter(Arc::new(f))
    }

    pub(crate) fn sort(&self, orders: &[Arc<Index>], scan: Scanner) -> Result<Vec<u32>, Error> {
        (self.0)(orders, scan)
    }
}

impl std::fmt::Debug for Sorter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sorter")
    }
}

/// An order of record slots.
#[derive(Debug)]
struct Sequence {
    slots: Vec<u32>,
    /// Each slot's place in `slots`, 1-based; 0 for a slot not in it.
    places: Vec<u32>,
}

impl Sequence {
    fn new(slots: Vec<u32>, count: u32) -> Sequence {
        let mut places = vec![0; count as usize + 1];
        for (i, &slot) in slots.iter().enumerate() {
            places[slot as usize] = i as u32 + 1;
        }
        Sequence { slots, places }
    }
}

impl State {
    fn slots(&self) -> u32 {
        self.file.file().slots()
    }

    /// Puts a live cursor's records in the order of its `ORDER BY` again,
    /// after a write that may have changed it (an append, a changed key).
    fn resequence(&mut self) -> Result<(), Error> {
        let Some(sorter) = self.sorter.take() else {
            return Ok(());
        };
        let orders: Vec<Arc<Index>> = self.orders.iter().filter_map(Weak::upgrade).collect();
        let length = self.file().structure.header.record_length as usize;
        let sorted = sorter.sort(&orders, &mut |f| {
            for slot in 1..=self.slots() {
                if !self.active(slot)? {
                    continue;
                }
                let bytes = self.read(slot, length)?;
                self.with_memo(|memo| f(slot, &bytes, memo))?;
            }
            Ok(())
        });
        self.sorter = Some(sorter);
        let slots = self.slots();
        self.sequence = Some(Sequence::new(sorted?, slots));
        Ok(())
    }

    /// Runs `f` with the memo file of a table with memo fields.
    fn with_memo<R>(&self, f: impl FnOnce(Option<&mut MemoFile>) -> R) -> R {
        // Without a memo file the table's file stays unlocked while `f` runs,
        // which a user-defined function in it may open tables of its own,
        // this one too.
        if self.file.file().memo().is_none() {
            return f(None);
        }
        f(self.file.file().memo())
    }

    /// The number of records in the file's header: in an ADT table, deleted
    /// ones not counted.
    fn count(&self) -> Result<u32, Error> {
        if let Some(count) = self.stale_count {
            return Ok(count);
        }
        self.refresh()?;
        let count = self.file().structure.header.record_count();
        Ok(count.saturating_sub(self.dropped))
    }

    /// Reads again what other processes may have changed in the file's
    /// header (its record slots and free list).
    fn refresh(&self) -> Result<(), Error> {
        self.file.file().refresh().map_err(storage_error)
    }

    /// The table's file, for one read or write.
    fn file(&self) -> shared::FileGuard<'_> {
        self.file.file()
    }

    /// Writes the pending record, if any (see [`write_pending`]). The record
    /// lock taken for the change is released; a lock the client took stays,
    /// but for a deleted record, whose locks all go. A record whose key an
    /// order has already, an order that is unique, is not written: it stays
    /// pending ([`Error::UniqueViolation`]).
    fn flush(&mut self) -> Result<(), Error> {
        // A dictionary's constraints refuse the record, which stays changed.
        self.check_pending()?;
        self.check_constrained()?;
        let Some(mut pending) = self.pending.take() else {
            return Ok(());
        };
        // What the handle wrote is what it reads now.
        if self
            .image
            .as_ref()
            .is_some_and(|(slot, _)| *slot == pending.slot)
        {
            self.image = None;
        }
        let shared = Arc::clone(&self.file);
        // A handle without the password of an encrypted table writes nothing
        // to it (an AES table's record it changed, `share.json`).
        if shared::sealed() && shared.file().structure.header.encrypted() {
            // An AES record stays changed and locked: every later move
            // fails the same way until the change is cancelled
            // (`fixtures/aes/api/09_pending`).
            if shared.file().is_aes() {
                self.pending = Some(pending);
                return Err(Error::EncryptionNotEnabled.detailed(
                    "Cannot update a record in an encrypted table unless encryption is enabled.",
                ));
            }
            if pending.implicit_lock {
                shared.unlock_record(self.owner, pending.slot);
            }
            return Err(Error::EncryptionNotEnabled);
        }
        // A record deleted that is the head of the free list already goes on
        // it no more, and the original counts it as deleted again.
        let redone = pending.deleted && !pending.appended && {
            let file = shared.file();
            file.structure.header.format == Format::Adt
                && file.structure.header.free_head == pending.slot
        };
        let orders: Vec<Arc<Index>> = self.orders.iter().filter_map(Weak::upgrade).collect();
        let writer = self
            .connection
            .as_ref()
            .and_then(crate::transaction::writer);
        let writer = writer.as_ref().map(|writer| (writer, &*self.source));
        let enforce_unique = self
            .connection
            .as_ref()
            .is_none_or(|connection| connection.unique_enforcement());
        let written = match write_pending(
            &shared,
            &orders,
            &mut pending,
            writer,
            self.owner,
            enforce_unique,
        ) {
            // A record an order refuses stays changed, and is not written
            // (a NULL key is 5208, `Error::Constraint`).
            Err(error)
                if matches!(
                    error,
                    Error::UniqueViolation(_)
                        | Error::KeyOverflow
                        | Error::Evaluation(tflip_expr::Runtime::KeyBuffer)
                        | Error::Constraint { code: 5208, .. }
                ) =>
            {
                self.pending = Some(pending);
                Err(error)
            }
            // A transaction keeps the record locked until it ends, also when
            // a write of it failed.
            Ok(true) => Ok(()),
            result => {
                let held = matches!(self.seen(pending.slot), Some(Seen::Mine(..)));
                if pending.implicit_lock && !held {
                    shared.unlock_record(self.owner, pending.slot);
                }
                result.map(drop)
            }
        };
        if written.is_ok() {
            self.dropped += u32::from(redone);
            self.resequence()?;
        }
        written
    }

    /// Checks the pending record against the `WHERE` of a cursor whose
    /// updates are constrained (`AdsStmtConstrainUpdates`), which refuses
    /// what it changes or appends outside it; the record stays changed. A
    /// record being deleted is not checked.
    fn check_constrained(&self) -> Result<(), Error> {
        let (Some(sql), Some(pending)) = (&self.constrained, &self.pending) else {
            return Ok(());
        };
        if pending.deleted || !(pending.changed || pending.appended) {
            return Ok(());
        }
        match self.with_memo(|memo| sql.holds(&pending.bytes, pending.slot, memo))? {
            true => Ok(()),
            false => Err(Error::Constraint {
                code: 7065,
                text: "Updates to records must meet the conditions of the SQL SELECT statement \
                       because the Constrained option has been selected."
                    .to_owned(),
            }),
        }
    }

    /// The first constraint of the dictionary's on the table that the record
    /// `bytes`, stored in slot `number`, breaks (see [`crate::constraints`]).
    fn stored_broken(&self, bytes: Vec<u8>, number: u32) -> Option<crate::constraints::Broken> {
        let checks = self.checks.as_ref()?;
        let date_format = match &self.connection {
            Some(connection) => connection.date_format(),
            None => checks.date_format(),
        };
        let dates = setters::Dates {
            format: &date_format,
            epoch: checks.epoch(),
        };
        let stored = bytes.clone();
        checks.first_broken(
            &bytes,
            |field| field.is_null(&stored),
            dates,
            |expr, env| {
                self.with_memo(|memo| {
                    let mut row = filter::Record {
                        fields: checks.fields(),
                        bytes: stored.clone(),
                        memo,
                        number,
                    };
                    expr.holds(&mut row, env).unwrap_or(false)
                })
            },
        )
    }

    /// Checks the pending record against the dictionary's constraints (see
    /// [`crate::constraints`]): each field, then the record validation. A
    /// record being deleted, or not changed, is not checked.
    fn check_pending(&self) -> Result<(), Error> {
        let (Some(checks), Some(pending)) = (&self.checks, &self.pending) else {
            return Ok(());
        };
        if pending.deleted || !(pending.changed || pending.appended) {
            return Ok(());
        }
        let date_format = match &self.connection {
            Some(connection) => connection.date_format(),
            None => checks.date_format(),
        };
        let dates = setters::Dates {
            format: &date_format,
            epoch: checks.epoch(),
        };
        let null = |field: &Field| {
            if pending.nulled.contains(&field.offset) {
                return true;
            }
            match pending
                .memos
                .iter()
                .find(|(offset, _)| *offset == field.offset)
            {
                Some((_, value)) => value.is_none(),
                None => field.is_null(&pending.bytes),
            }
        };
        let bytes = pending.bytes.clone();
        let number = pending.slot;
        checks.check_record(&pending.bytes, null, dates, |expr, env| {
            self.with_memo(|memo| {
                let mut row = filter::Record {
                    fields: checks.fields(),
                    bytes,
                    memo,
                    number,
                };
                expr.holds(&mut row, env).unwrap_or(false)
            })
        })
    }

    /// The slot after `slot` (forward) or before it in the table's natural
    /// order, where 0 is before the first and one past the last slot after
    /// the last.
    fn step(&self, slot: u32, forward: bool) -> Option<u32> {
        let slots = self.slots();
        let Some(sequence) = &self.sequence else {
            return match forward {
                true if slot < slots => Some(slot + 1),
                false if slot > 1 => Some((slot - 1).min(slots)),
                _ => None,
            };
        };
        // Places are 1-based; 0 is before the first, len + 1 after the last.
        let place = match slot {
            0 => 0,
            s if s > slots => sequence.slots.len() + 1,
            // A record another handle appended is in no place of it.
            s => sequence.places.get(s as usize).map_or(0, |&p| p as usize),
        };
        let next = match forward {
            true => place + 1,
            false => place.checked_sub(1)?,
        };
        next.checked_sub(1)
            .and_then(|i| sequence.slots.get(i))
            .copied()
    }

    /// The slots in the table's natural order.
    fn natural(&self) -> Vec<u32> {
        match &self.sequence {
            Some(sequence) => sequence.slots.clone(),
            None => (1..=self.slots()).collect(),
        }
    }

    /// What this handle sees of slot `slot`, if a transaction wrote it (see
    /// [`Shared::seen`]): the connection's own transaction its images,
    /// others the records as they were.
    fn seen(&self, slot: u32) -> Option<Seen> {
        let reader = self
            .connection
            .as_deref()
            .and_then(crate::transaction::reader);
        self.file.seen(slot, reader)
    }

    /// The status of slot `slot` as this handle sees it: a transaction's
    /// record as it sees it, a record another transaction appended as none.
    fn status(&mut self, slot: u32) -> Result<Option<RecordStatus>, Error> {
        match self.seen(slot) {
            Some(Seen::Mine(_, true)) => Ok(Some(RecordStatus::Deleted)),
            Some(Seen::Mine(bytes, false) | Seen::Before(bytes)) => {
                Ok(Some(self.file().status_of(&bytes)))
            }
            Some(Seen::Absent) => Ok(None),
            None => Ok(Some(self.file().status(slot).map_err(storage_error)?)),
        }
    }

    /// Whether slot `slot` holds a record that is not hidden as deleted:
    /// an active one, or in a DBF table a deleted one while deleted records
    /// are shown.
    fn active(&mut self, slot: u32) -> Result<bool, Error> {
        if self
            .deleted_shown
            .as_ref()
            .is_some_and(|settings| settings.shows_deleted())
        {
            return self.exists(slot);
        }
        Ok(self.status(slot)? == Some(RecordStatus::Active))
    }

    /// Whether slot `slot` holds a record a move to its number reaches: an
    /// active one, or any record of a DBF table, deleted or not (the
    /// original's `AdsGotoRecord` ignores `AdsShowDeleted`), but one another
    /// transaction appended.
    fn exists(&mut self, slot: u32) -> Result<bool, Error> {
        let slots = self.file().slots();
        if self.deleted_shown.is_some() {
            if slot < 1 || slot > slots {
                return Ok(false);
            }
            return Ok(!self.appended_elsewhere(slot)?);
        }
        Ok(self.status(slot)? == Some(RecordStatus::Active))
    }

    /// Whether slot `slot` holds a record another transaction appended (in
    /// the file blank and marked, also by another process).
    fn appended_elsewhere(&mut self, slot: u32) -> Result<bool, Error> {
        Ok(match self.seen(slot) {
            Some(seen) => matches!(seen, Seen::Absent),
            None => self.file().marked_at(slot).map_err(storage_error)?.1,
        })
    }

    /// The changes the process made to the table's file (see
    /// [`TableFile::changes`]).
    fn changes(&mut self) -> Result<u64, Error> {
        Ok(self.file().changes())
    }

    /// Whether slot `slot` holds a deleted record.
    fn exists_deleted(&mut self, slot: u32) -> Result<bool, Error> {
        Ok(self.status(slot)? == Some(RecordStatus::Deleted))
    }

    /// Whether the table has any record that is not deleted.
    fn any_active(&mut self) -> Result<bool, Error> {
        for slot in 1..=self.slots() {
            if self.active(slot)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The bytes of slot `slot`, `length` of them.
    fn read(&mut self, slot: u32, length: usize) -> Result<Vec<u8>, Error> {
        if let Some(pending) = self.pending.as_ref().filter(|p| p.slot == slot) {
            return Ok(pending.bytes.clone());
        }
        if let Some((_, bytes)) = self.kept.as_ref().filter(|(kept, _)| *kept == slot) {
            return Ok(bytes.clone());
        }
        if let Some((_, bytes)) = self.image.as_ref().filter(|(image, _)| *image == slot) {
            return Ok(bytes.clone());
        }
        // What another transaction's record was is kept as it reads with the
        // password: a handle without it reads the file, which has it still.
        match self.seen(slot) {
            Some(Seen::Mine(bytes, _)) => return Ok(bytes),
            Some(Seen::Before(bytes)) if !shared::sealed() => return Ok(bytes),
            _ => {}
        }
        let mut file = self.file();
        let mut bytes = vec![0; length];
        file.read(slot, &mut bytes).map_err(storage_error)?;
        Ok(bytes)
    }

    /// Whether slot `slot` holds a record that is visible: active, and
    /// passing the filters of `sieve`. Deleted records are never visible:
    /// the original engine does not show them in ADT tables, not even with
    /// `AdsShowDeleted`.
    fn visible(&mut self, slot: u32, sieve: &Sieve) -> Result<bool, Error> {
        let kept = sieve.filters.aof.is_some()
            || sieve
                .filters
                .sql
                .as_ref()
                .is_some_and(|sql| sql.superset.is_some());
        if kept && sieve.excludes(slot, self.changes()?) {
            return Ok(false);
        }
        if !self.active(slot)? {
            return Ok(false);
        }
        if sieve.filters.is_empty() {
            return Ok(true);
        }
        let bytes = self.read(slot, sieve.record_length)?;
        self.with_memo(|memo| sieve.passes(bytes, slot, memo))
    }

    /// The first visible slot after `slot` (forward) or before it
    /// (backward).
    fn next_visible(
        &mut self,
        slot: u32,
        forward: bool,
        sieve: &Sieve,
    ) -> Result<Option<u32>, Error> {
        let mut slot = slot;
        loop {
            slot = match self.step(slot, forward) {
                Some(next) => next,
                None => return Ok(None),
            };
            if self.visible(slot, sieve)? {
                return Ok(Some(slot));
            }
        }
    }

    /// Where a skip starts. A table left on no record by a written delete
    /// or a cancelled append (ADT) moves on from the slot that record has:
    /// the original engine's cursor is still there, though the record is on
    /// the free list (`fixtures/write/skip_deleted`).
    fn skip_position(&self) -> Position {
        match (self.position, self.stranded) {
            (
                Position {
                    record: None,
                    bof: false,
                    eof: false,
                },
                Some(slot),
            ) => Position {
                record: Some(slot),
                bof: false,
                eof: false,
            },
            (position, _) => position,
        }
    }

    /// Puts the table on `record`, or off the end the move went past (at
    /// both ends if no record is visible).
    fn land(&mut self, record: Option<u32>, past_end: bool, sieve: &Sieve) -> Result<(), Error> {
        self.flush()?;
        let empty = record.is_none() && self.next_visible(0, true, sieve)?.is_none();
        self.land_in(record, past_end, empty)
    }

    /// Puts the table on `record`, or off the end the move went past, where
    /// `empty` says whether the order moved in has no records (then it is at
    /// both ends).
    fn land_in(&mut self, record: Option<u32>, past_end: bool, empty: bool) -> Result<(), Error> {
        self.flush()?;
        self.found = false;
        self.counted_past_end = false;
        self.stranded = None;
        self.kept = None;
        if self
            .image
            .as_ref()
            .is_some_and(|(image, _)| Some(*image) != record)
        {
            self.image = None;
        }
        self.position = match record {
            Some(record) => Position {
                record: Some(record),
                bof: false,
                eof: false,
            },
            None if past_end => Position {
                record: None,
                bof: empty,
                eof: true,
            },
            None => Position {
                record: None,
                bof: true,
                eof: empty,
            },
        };
        Ok(())
    }
}

fn storage_error(error: StorageError) -> Error {
    match error {
        StorageError::CorruptMemo => Error::CorruptMemo,
        StorageError::Corrupt(tflip_storage::memo::BLOCK_PAST_END) => Error::FileRead,
        StorageError::Io(_) | StorageError::Corrupt(_) => Error::CorruptTable,
        StorageError::EncryptedDescriptors | StorageError::Encrypted => Error::TableEncrypted,
        StorageError::ReadOnly => Error::TableReadOnly,
        StorageError::NotWritable => Error::NotImplemented,
        StorageError::WrongMemoFile(flavour) => Error::WrongTableType(flavour.into()),
    }
}

/// What a data length counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LengthUnit {
    /// Characters (UTF-16 code units in the Unicode types), or bytes for
    /// binary data.
    CodeUnits,
    /// Bytes.
    Bytes,
    /// The bytes the field takes in the record.
    RecordBytes,
}

/// How `AdsCustomizeAOF` changes an AOF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Customize {
    Add,
    Remove,
    /// Add the records not in it, remove those in it.
    Toggle,
}

/// How a client names a field: by its 1-based number or by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldRef<'a> {
    Number(usize),
    Name(&'a str),
}

impl Table {
    /// Whether a handle other than this one has the table's file open.
    pub(crate) fn opened_by_others(&self) -> bool {
        lock(&self.state).file.handles() > 1
    }

    /// The table's state, for one read or move: when the table is the child
    /// of a relation whose parent moved, it follows the parent first (see
    /// [`crate::relation`]).
    fn state(&self) -> StateGuard<'_> {
        crate::relation::follow(self);
        self.guard()
    }

    /// The table's state, as it is: its children are told when it moves.
    pub(crate) fn guard(&self) -> StateGuard<'_> {
        let guard = lock(&self.state);
        let before = guard.position;
        let _sealing = crate::shared::Sealing::new(!self.keyed());
        StateGuard {
            guard,
            table: self,
            before,
            _sealing,
        }
    }

    /// What does not change about the table while it is open.
    pub fn info(&self) -> TableInfo {
        // The header's flags as the file has them now (the transaction-free
        // one changes).
        let mut structure = self.structure.clone();
        structure.header.flags = self.state().file.file().structure.header.flags;
        TableInfo {
            connection: self.connection,
            path: self.path.to_string_lossy().into_owned(),
            settings: self.settings.clone(),
            structure,
            memo_block_size: self.state().file.file().memo_block_size(),
            null_record: self.null_record(),
            alias: self
                .alias
                .get()
                .cloned()
                .unwrap_or_else(|| stem(&self.path.to_string_lossy())),
        }
    }

    /// Whether the table was opened by name, rather than returned by an SQL
    /// statement (a view opened by name was).
    pub fn opened_by_name(&self) -> bool {
        self.opened.load(Ordering::Relaxed)
    }

    /// Where the table is.
    pub fn position(&self) -> Position {
        self.state().position
    }

    /// Whether a transaction not ended yet holds records of the table.
    pub(crate) fn has_transaction_records(&self) -> bool {
        !self.state().file.transactions().is_empty()
    }

    /// Whether the connection's transaction wrote record `slot` itself, not
    /// through a referential integrity rule.
    pub(crate) fn written_by_transaction(&self, slot: u32) -> bool {
        let state = self.state();
        let Some(reader) = state
            .connection
            .as_deref()
            .and_then(crate::transaction::reader)
        else {
            return false;
        };
        state
            .file
            .transactions()
            .get(&slot)
            .is_some_and(|held| held.tx == reader && !held.by_ri)
    }

    /// Whether the handle has not given the password of an AES table, which
    /// refuses it the work on the table's files that needs the key.
    pub(crate) fn lacks_aes_password(&self) -> bool {
        let state = self.state();
        let file = state.file();
        !self.keyed() && file.is_aes() && file.structure.header.encrypted()
    }

    /// Whether the handle reads the blank record of an encrypted table, as it
    /// has not given the table's password: its keys are not the index's.
    fn reads_blank(&self) -> bool {
        !self.keyed() && self.state().file().structure.header.encrypted()
    }

    /// [`Table::record_number`], after the pending change of a handle that
    /// has not given its table's password: it finds the change unwritable
    /// when it asks for the number, as when it moves
    /// (`fixtures/aes/api/09_pending`). A DBF table's handle is told the
    /// number (`fixtures/write/dbf_aes_nopassword`).
    pub fn record_number_checked(&self) -> Result<u32, Error> {
        if !self.keyed() {
            let mut state = self.state();
            let sealed_adt = {
                let header = &state.file().structure.header;
                header.encrypted() && header.format == Format::Adt
            };
            if sealed_adt {
                state.flush()?;
            }
        }
        Ok(self.record_number())
    }

    /// The record number of the position, as the original engine reports it:
    /// the current record's, 0 at BOF, and one past the last slot at EOF
    /// (also in an empty table, which is at both ends).
    pub fn record_number(&self) -> u32 {
        let state = self.state();
        if let Some(record) = state.stranded {
            return record;
        }
        match state.position {
            Position {
                record: Some(record),
                ..
            } => record,
            Position { eof: true, .. } => state.slots() + 1,
            Position { .. } if state.counted_past_end => state.slots() + 1,
            Position { .. } => 0,
        }
    }

    /// The table's filters as they are now, and what they are evaluated
    /// with, for a call to take before it locks the table's state.
    fn sieve(&self) -> Sieve<'_> {
        let filters = Arc::clone(&lock(&self.filters));
        let length = self.structure.header.record_length as usize;
        let env = match filters.is_empty() {
            true => Env {
                exact: false,
                date_format: String::new(),
                epoch: tflip_expr::text::DEFAULT_EPOCH,
                now: (0, 0),
                utc: (0, 0),
                case_fold: None,
                weights: None,
                dynamic: None,
                blank_empty: false,
            },
            false => self.env(),
        };
        Sieve {
            fields: self.fields(),
            record_length: length,
            filters,
            env,
        }
    }

    /// What expressions on the table are evaluated with: its "exact"
    /// setting, its date format and the time now.
    fn env(&self) -> Env {
        let now = crate::clock::now();
        Env {
            exact: self.exact.load(Ordering::Relaxed),
            date_format: self.date_format(),
            epoch: self.session_settings.epoch(),
            now: now.local,
            utc: now.utc,
            case_fold: self
                .session_settings()
                .collations()
                .case_fold(self.is_oem()),
            weights: self
                .session_settings()
                .collations()
                .table_collation(self.is_oem())
                .map(|collation| collation.weights),
            dynamic: self.dynamic_collation(),
            blank_empty: self.format() != Format::Adt,
        }
    }

    /// The dynamic collation an ADT table was opened with, by which its
    /// expressions compare text; `None` for none (or one the collation
    /// files do not have).
    pub(crate) fn dynamic_collation(&self) -> Option<Arc<tflip_storage::collation::Dynamic>> {
        let name = self.settings.collation.trim();
        if name.is_empty() || self.format() != Format::Adt {
            return None;
        }
        self.session_settings().collations().dynamic(name)
    }

    /// The record number of the position counting only the records that are
    /// visible, as the original engine reports it when filters are respected:
    /// the current record's place among the visible records, 0 at BOF, and
    /// one past the last visible record at EOF. On a record the filters hide
    /// it is [`Error::NotFound`], and, as in the original engine, the table
    /// goes to EOF.
    pub fn visible_record_number(&self) -> Result<u32, Error> {
        let sieve = self.sieve();
        let mut state = self.state();
        let end = match state.position {
            Position {
                record: Some(record),
                ..
            } => {
                if !state.visible(record, &sieve)? {
                    state.land(None, true, &sieve)?;
                    return Err(Error::NotFound);
                }
                Some(record)
            }
            Position { eof: true, .. } => None,
            Position { .. } if state.counted_past_end => None,
            Position { .. } => return Ok(0),
        };
        let mut count = 0;
        for slot in state.natural() {
            count += u32::from(state.visible(slot, &sieve)?);
            if Some(slot) == end {
                break;
            }
        }
        Ok(if state.position.record.is_some() {
            count
        } else {
            count + 1
        })
    }

    /// How many bytes a record starts with before its fields.
    pub(crate) fn record_prefix(&self) -> usize {
        match self.format() {
            Format::Adt => 5,
            Format::Dbf(_) => 1,
        }
    }

    /// The local date the table's file was last written, as a Julian day;
    /// today for a cursor in memory. (The original engine writes to a table
    /// it opens to change, so it reports the day it opened it then.) A DBF
    /// table's is the date in its header.
    pub fn last_update(&self) -> i32 {
        if let Some(info) = &self.dbf {
            let [year, month, day] = info.updated.map(u32::from);
            let date = Ymd {
                year: 1900 + year as i32,
                month,
                day,
            };
            if let Some(julian) = tflip_storage::value::ymd_to_julian(date) {
                return julian;
            }
        }
        match std::fs::metadata(&self.path).and_then(|m| m.modified()) {
            Ok(modified) => crate::clock::local_date(modified),
            Err(_) => crate::clock::now().local.0,
        }
    }

    /// The number of records, filters ignored (`AdsGetRecordCount` with
    /// `ADS_IGNOREFILTERS`): the table's record count, or, for a live SQL
    /// cursor with a `WHERE`, the records it holds for; as in the original
    /// engine, a `WHERE` that always holds counts every record slot, deleted
    /// ones included.
    pub fn record_count(&self) -> Result<u32, Error> {
        if self.uncounted {
            return Ok(0);
        }
        let Some(sql) = lock(&self.filters).sql.clone() else {
            // A static cursor that shows only some of its rows has those; a
            // live cursor with no `WHERE` counts the table's records, the
            // ones a transaction deleted too (`fixtures/sql/txscripts.json`).
            let shown = self
                .state()
                .sequence
                .as_ref()
                .filter(|_| self.built)
                .map(|s| s.slots.len() as u32);
            return shown.map_or_else(|| self.state().count(), Ok);
        };
        let length = self.structure.header.record_length as usize;
        let mut state = self.state();
        if let Some(constant) = sql.constant {
            return Ok(if constant { state.slots() } else { 0 });
        }
        // A `WHERE` its full-text orders answer counts the records they
        // found, while the table is as it was.
        if let Some((changes, count)) = sql.count
            && changes == state.changes().unwrap_or(0)
        {
            return Ok(count);
        }
        // Counting a `WHERE` that is evaluated for each record, or one in an
        // order, writes the record being changed first, which a constrained
        // cursor refuses when it leaves the `WHERE`.
        if sql.superset.is_none() || state.sequence.is_some() {
            state.flush()?;
        }
        let mut count = 0;
        for slot in 1..=state.slots() {
            if !state.active(slot)? {
                continue;
            }
            let bytes = state.read(slot, length)?;
            count += u32::from(state.with_memo(|memo| sql.holds(&bytes, slot, memo))?);
        }
        Ok(count)
    }

    /// The number of visible records (deleted ones are not); with no
    /// filters, the table's record count.
    pub fn visible_count(&self) -> Result<u32, Error> {
        if self.uncounted {
            return Ok(0);
        }
        let mut sieve = self.sieve();
        // As [`Table::record_count`] does, a cursor's `WHERE` counts the
        // record being changed written.
        let mut state = self.state();
        if sieve
            .filters
            .sql
            .as_ref()
            .is_some_and(|sql| sql.superset.is_none() || state.sequence.is_some())
        {
            state.flush()?;
        }
        drop(state);
        // An AOF alone counts as when it counted (see [`Members::count`]);
        // one left over from a failed `AdsSetAOF` does not count.
        if let (Some(aof), None, None) = (
            &sieve.filters.aof,
            &sieve.filters.filter,
            &sieve.filters.sql,
        ) && !aof.orphaned
            && let Some(count) = lock(&aof.members).count
        {
            return Ok(count);
        }
        if sieve.filters.aof.as_ref().is_some_and(|aof| aof.orphaned) {
            let Filters { filter, sql, .. } = (*sieve.filters).clone();
            sieve.filters = Arc::new(Filters {
                aof: None,
                filter,
                sql,
                parked: None,
            });
        }
        let hidden = self
            .state()
            .deleted_shown
            .as_ref()
            .is_some_and(|s| !s.shows_deleted());
        // A static cursor that shows only some of its rows counts those.
        let shown = self.state().sequence.as_ref().map(|s| s.slots.len() as u32);
        if sieve.filters.is_empty() && !hidden {
            return shown.map_or_else(|| self.state().count(), Ok);
        }
        let mut state = self.state();
        let mut count = 0;
        for slot in state.natural() {
            count += u32::from(state.visible(slot, &sieve)?);
        }
        Ok(count)
    }

    /// Whether the current record is deleted (`AdsIsRecordDeleted`); in an
    /// ADT table it never is, deleted records being out of reach.
    /// [`Error::NoCurrentRecord`] off a record.
    pub fn is_deleted(&self) -> Result<bool, Error> {
        let state = self.state();
        let record = state.position.record.ok_or(Error::NoCurrentRecord)?;
        if let Some(pending) = state.pending.as_ref().filter(|p| p.slot == record) {
            // A DBF record's deletion flag is in its bytes.
            let flagged = self.format() != Format::Adt && pending.bytes.first() == Some(&b'*');
            return Ok(pending.deleted || flagged);
        }
        if state.kept.as_ref().is_some_and(|(kept, _)| *kept == record) {
            return Ok(false);
        }
        let mut state = state;
        drop(state.file());
        let deleted = state.exists_deleted(record)?;
        // A `WHERE` evaluated for each record leaves a record deleted out of
        // the cursor's view.
        let outside = lock(&self.filters)
            .sql
            .as_ref()
            .is_some_and(|sql| sql.superset.is_none());
        match deleted && outside && self.format() == Format::Adt {
            true => Err(Error::NoCurrentRecord),
            false => Ok(deleted),
        }
    }

    /// Whether the current record is visible: active and passing the
    /// filters (`AdsIsRecordVisible`); [`Error::NoCurrentRecord`] off a
    /// record.
    pub fn is_visible(&self) -> Result<bool, Error> {
        let sieve = self.sieve();
        let mut state = self.state();
        let record = state.position.record.ok_or(Error::NoCurrentRecord)?;
        state.visible(record, &sieve)
    }

    /// Moves to the first visible record (or to BOF and EOF if there is
    /// none).
    pub fn goto_top(&self) -> Result<(), Error> {
        let sieve = self.sieve();
        let mut state = self.state();
        // What is pending is written first, so that the record it deletes or
        // changes is not the one the move finds.
        state.flush()?;
        state.refresh()?;
        let first = state.next_visible(0, true, &sieve)?;
        state.land(first, true, &sieve)
    }

    /// Moves to the last visible record (or to BOF and EOF if there is
    /// none).
    pub fn goto_bottom(&self) -> Result<(), Error> {
        let sieve = self.sieve();
        let mut state = self.state();
        state.flush()?;
        state.refresh()?;
        let after = state.slots() + 1;
        let last = state.next_visible(after, false, &sieve)?;
        state.land(last, false, &sieve)
    }

    /// Moves before the first record.
    pub fn goto_bof(&self) -> Result<(), Error> {
        let sieve = self.sieve();
        self.state().land(None, false, &sieve)
    }

    /// Moves after the last record.
    pub fn goto_eof(&self) -> Result<(), Error> {
        let sieve = self.sieve();
        self.state().land(None, true, &sieve)
    }

    /// Moves `records` visible records forward, or back if negative,
    /// stopping at EOF or BOF. From BOF, one record forward is the first
    /// record; from EOF, one record back is the last. From a record the
    /// filters hide, a move counts from where it is.
    pub fn skip(&self, records: i64) -> Result<(), Error> {
        let sieve = self.sieve();
        let mut state = self.state();
        state.found = false;
        if records == 0 {
            return Ok(());
        }
        let forward = records > 0;
        let position = state.skip_position();
        // At BOF (also with EOF, after going to record 0 or past the last),
        // forward starts before the first record, and back stays at BOF.
        let mut slot = match position.record {
            Some(record) => record,
            None if position.bof && forward => 0,
            None if position.bof => return state.land(None, false, &sieve),
            None if forward => return Ok(()),
            None => state.slots() + 1,
        };
        for _ in 0..records.unsigned_abs() {
            match state.next_visible(slot, forward, &sieve)? {
                Some(next) => slot = next,
                // Past the end the table is at EOF (or BOF) only, even if the
                // filters hide every record; at both if there is none.
                None => {
                    let empty = !state.any_active()?;
                    state.land_in(None, forward, empty)?;
                    return Ok(());
                }
            }
        }
        state.land(Some(slot), forward, &sieve)
    }

    /// The record of the first visible key from `record` on, moving
    /// `order`'s cursor forward (or back) from it past the keys of hidden
    /// records; `record` is the cursor's.
    fn visible_key(
        state: &mut State,
        order: &Index,
        mut record: Option<u32>,
        forward: bool,
        sieve: &Sieve,
    ) -> Result<Option<u32>, Error> {
        while let Some(r) = record {
            if state.visible(r, sieve)? && Table::key_seen(state, order, r)? {
                return Ok(Some(r));
            }
            record = order.step(forward)?;
        }
        Ok(None)
    }

    /// Whether the key `order`'s cursor is on is record `record`'s as this
    /// handle sees it: while a transaction runs, an order has a key of the
    /// record as it was and one of the record as the transaction has it,
    /// and each reader passes the other by. A record another process's
    /// transaction changed is marked in the file, which holds it as it was:
    /// its new key is passed by too.
    fn key_seen(state: &mut State, order: &Index, record: u32) -> Result<bool, Error> {
        if state.seen(record).is_none() && !state.file().marked_at(record).map_err(storage_error)?.0
        {
            return Ok(true);
        }
        let table = order.owner();
        let length = table.structure.header.record_length as usize;
        let bytes = state.read(record, length)?;
        let row = filter::Record {
            fields: table.fields(),
            bytes,
            memo: None,
            number: record,
        };
        let key = table.build_key(order, row, true)?;
        Ok(key == order.current_key()?)
    }

    /// The record of the first visible key of `order` in its scope, moving
    /// the order's cursor there.
    fn first_visible(
        state: &mut State,
        order: &Index,
        sieve: &Sieve,
    ) -> Result<Option<u32>, Error> {
        let first = order.first()?;
        Table::visible_key(state, order, first, true, sieve)
    }

    /// Puts `order`'s cursor on the key of `record`, if the order has one
    /// for it: by seeking the record's key (see [`Table::record_key`]), or
    /// for orders whose keys tflip cannot build (collation weights, binary
    /// orders) by going through the keys.
    pub(crate) fn locate_in(&self, order: &Index, record: u32) -> Result<bool, Error> {
        if order.tag.binary() {
            return order.locate(record);
        }
        match self.record_key(order, record) {
            Ok(Some(key)) => order.locate_key(record, &key, self.reads_blank()),
            Ok(None) => Ok(false),
            Err(Error::NotImplemented) => order.locate(record),
            Err(error) => Err(error),
        }
    }

    /// The current record's place in `order`, 1-based, as the original
    /// engine reports it: 0 at BOF (also in an order without keys, which is
    /// at both ends) and one past the last key at EOF. A record without a
    /// key in `order` (one a `FOR` condition leaves out) is
    /// [`Error::NotFound`].
    ///
    /// With `respect`, only visible records count, and a record the
    /// filters hide is [`Error::NotFound`]; and with a scope set, the place
    /// counts from the scope's first key, and is 0 at EOF and for a record
    /// outside the scope. As in the original engine, counting from a scope's
    /// top seeks it: the found flag says whether a key starts with it.
    pub fn key_number_in(&self, order: &Index, respect: bool) -> Result<u32, Error> {
        // A cursor's temporary index holds the records it shows, in the
        // order it shows them.
        if order.is_temporary() {
            return self.visible_record_number();
        }
        let sieve = self.sieve();
        let filtered = respect && !sieve.filters.is_empty();
        let scoped = respect && order.scope() != Scope::default();
        let position = self.state().position;
        let record = match position {
            Position {
                record: Some(record),
                ..
            } => record,
            Position { bof: true, .. } => return Ok(0),
            Position { .. } if scoped => return Ok(0),
            Position { .. } if filtered => return Ok(self.key_count_in(order, true)? + 1),
            Position { .. } => return Ok(order.tree_keys()? + 1),
        };
        if !self.locate_in(order, record)? {
            return Err(match self.reads_blank() {
                true => Error::NotFound.detailed("Key not in index."),
                false => Error::NotFound,
            });
        }
        if filtered {
            let mut state = self.state();
            if !state.visible(record, &sieve)? {
                return Err(Error::NotFound);
            }
            if scoped && order.place()? != Some(Place::Inside) {
                return Ok(0);
            }
            // Count the visible keys up to the record's.
            let mut number = 0;
            let mut key = order.first()?;
            while let Some(r) = key {
                number += u32::from(state.visible(r, &sieve)?);
                if r == record {
                    break;
                }
                key = order.step(true)?;
            }
            drop(state);
            if scoped && let Some(found) = order.seek_top()? {
                self.state().found = found;
            }
            order.locate(record)?;
            return Ok(number);
        }
        let number = order.key_number()?.ok_or(Error::NotFound)?;
        if !scoped {
            return Ok(number);
        }
        if order.place()? != Some(Place::Inside) {
            return Ok(0);
        }
        if let Some(found) = order.seek_top()? {
            self.state().found = found;
        }
        order.first()?;
        let first = order.key_number()?.unwrap_or(number);
        order.locate(record)?;
        Ok(number + 1 - first)
    }

    /// Where the current key is in `order`, scopes and filters respected,
    /// from 0 to 1 (`AdsGetRelKeyPos`): the middle of its share, (k - 0.5)
    /// / n for key k of n (0.125 for the first of four), as in the original
    /// engine; off a record [`Error::NoCurrentRecord`].
    pub fn relative_position_in(&self, order: &Index) -> Result<f64, Error> {
        let count = self.key_count_in(order, true)?;
        let number = self.key_number_in(order, true)?;
        if number == 0 || number > count {
            return Err(Error::NoCurrentRecord);
        }
        Ok((f64::from(number) - 0.5) / f64::from(count))
    }

    /// Moves to the key at `position` (0 to 1) of `order`
    /// (`AdsSetRelKeyPos`): key floor(position × n) of its n keys, scopes
    /// and filters respected, counted from 0 and from the top, but from the
    /// bottom in a descending order, as in the original engine (so 0.25 of
    /// four keys is the second ascending, the first descending).
    pub fn set_relative_position_in(&self, order: &Index, position: f64) -> Result<(), Error> {
        let count = self.key_count_in(order, true)?;
        let last = count.saturating_sub(1);
        let at = |share: f64| ((share * f64::from(count)).floor() as u32).min(last);
        let key = if order.tag.descending {
            last - at(1.0 - position)
        } else {
            at(position)
        };
        self.goto_top_in(order)?;
        self.skip_in(order, i64::from(key))
    }

    /// The number of keys in `order` (see [`Index::key_count`]); with
    /// `respect`, of those in its scope whose records are visible. As in the
    /// original engine, counting from a scope's top seeks it: the found flag
    /// says whether a key starts with it; and counting through a filter at
    /// BOF leaves the record numbers at EOF's.
    pub fn key_count_in(&self, order: &Index, respect: bool) -> Result<u32, Error> {
        if order.is_temporary() {
            return order.key_count(respect);
        }
        let sieve = self.sieve();
        let count = if respect && !sieve.filters.is_empty() && !order.tag.binary() {
            let mut state = self.state();
            if state.position.record.is_none() {
                state.counted_past_end = true;
            }
            let mut count = 0;
            let mut key = order.first()?;
            while let Some(record) = key {
                count += u32::from(state.visible(record, &sieve)?);
                key = order.step(true)?;
            }
            count
        } else {
            order.key_count(respect)?
        };
        if respect && let Some(found) = order.seek_top()? {
            self.state().found = found;
        }
        Ok(count)
    }

    /// Moves to the first visible record of `order` in its scope (to BOF
    /// and EOF if it has none).
    pub fn goto_top_in(&self, order: &Index) -> Result<(), Error> {
        let sieve = self.sieve();
        let mut state = self.state();
        // The record written first, as a delete not written yet would leave
        // its bit in a binary order (`fixtures/write/binary_changes`).
        state.flush()?;
        let first = Table::first_visible(&mut state, order, &sieve)?;
        state.land_in(first, true, first.is_none())?;
        Ok(())
    }

    /// Moves to the last visible record of `order` in its scope (to BOF and
    /// EOF if it has none).
    pub fn goto_bottom_in(&self, order: &Index) -> Result<(), Error> {
        let sieve = self.sieve();
        let mut state = self.state();
        state.flush()?;
        let last = order.last()?;
        let last = Table::visible_key(&mut state, order, last, false, &sieve)?;
        state.land_in(last, false, last.is_none())?;
        Ok(())
    }

    /// Moves `records` visible records forward in `order`, or back if
    /// negative, stopping at EOF or BOF, as [`Table::skip`] does in record
    /// order, and within the order's scope: from a record before the scope,
    /// a move forward starts as from BOF; from one after it, a move back
    /// starts as from EOF. From a record the filters hide, a move counts
    /// from its key.
    ///
    /// From a record that has no key in `order` (one a `FOR` condition
    /// leaves out, or a deleted ADT record gone to with `AdsGotoRecord`), a
    /// move starts where its entry would be, by key and then record number,
    /// as in the original engine (`fixtures/write/skip_unkeyed`): its first
    /// step goes to the key next to that place; each further step in a CDX
    /// order is one key, in an ADI order two (so a skip of n lands on the
    /// (2n - 1)th key, as the original engine does). In an NTX order a move
    /// goes to EOF forward and BOF back.
    pub fn skip_in(&self, order: &Index, records: i64) -> Result<(), Error> {
        let sieve = self.sieve();
        // Put the order's cursor on the current record first: finding it
        // reads the record.
        let current = {
            let mut state = self.state();
            // An ADT record deleted and not written yet has lost its key
            // when the original starts to move from it, as one written
            // has; a DBF record keeps its key.
            if self.format() == Format::Adt
                && state
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.deleted && !pending.appended)
            {
                state.flush()?;
            }
            state.skip_position().record
        };
        let located = match current {
            Some(record) if records != 0 => self.locate_in(order, record)?,
            _ => false,
        };
        // A record without a key in the order: the key next to where its
        // entry would be, in the direction of the move (`Some(None)` if
        // there is none), unless the order is an NTX one or tflip cannot
        // build the key.
        let beside = match current {
            Some(record) if records != 0 && !located && order.kind != Kind::Ntx => {
                match self.built_key(order, record) {
                    Ok(Some(key)) => Some(order.seek_beside(&key, record, records > 0)?),
                    Ok(None) | Err(Error::NotImplemented | Error::KeyOverflow) => None,
                    Err(error) => return Err(error),
                }
            }
            _ => None,
        };
        let mut state = self.state();
        state.found = false;
        let forward = records > 0;
        let position = state.skip_position();
        let mut steps = records.unsigned_abs();
        if steps == 0 {
            return Ok(());
        }
        /// Where a move starts.
        enum From {
            Record(u32),
            Start,
            End,
        }
        let from = match position.record {
            // Put the order's cursor on the current record; each step is
            // then one key.
            Some(record) if located && position.record == current => match order.place()? {
                Some(Place::Before) => From::Start,
                Some(Place::After) => From::End,
                _ => From::Record(record),
            },
            Some(_) if let Some(beside) = beside => {
                let place = match beside {
                    Some(_) => order.place()?,
                    None => None,
                };
                match (beside, place) {
                    // The first step is to the key beside; in an ADI order
                    // each further one is two keys.
                    (Some(landed), Some(Place::Inside)) => {
                        let further = steps - 1;
                        steps = if order.kind == Kind::Adi {
                            2 * further
                        } else {
                            further
                        };
                        match Table::visible_key(&mut state, order, Some(landed), forward, &sieve)?
                        {
                            Some(record) if steps == 0 => {
                                state.land_in(Some(record), forward, false)?;
                                return Ok(());
                            }
                            Some(record) => From::Record(record),
                            None => {
                                state.land_in(None, forward, false)?;
                                return Ok(());
                            }
                        }
                    }
                    (Some(_), Some(Place::Before)) if forward => From::Start,
                    (Some(_), Some(Place::After)) if !forward => From::End,
                    _ => {
                        let empty = Table::first_visible(&mut state, order, &sieve)?.is_none();
                        state.land_in(None, forward, empty)?;
                        return Ok(());
                    }
                }
            }
            Some(_) => {
                let empty = Table::first_visible(&mut state, order, &sieve)?.is_none();
                state.land_in(None, forward, empty)?;
                return Ok(());
            }
            // At both ends (an order without keys), a move leaves the end
            // it goes to, as in the original engine, which numbers BOF
            // there as EOF.
            None if position.bof && position.eof => {
                state.land_in(None, forward, false)?;
                state.counted_past_end = !forward;
                return Ok(());
            }
            None if forward && position.eof || !forward && position.bof => return Ok(()),
            None if position.bof => From::Start,
            None => From::End,
        };
        let mut record = match from {
            From::Record(record) => record,
            // From the start one record forward is the first; from the end
            // one back the last.
            From::Start | From::End if forward == matches!(from, From::Start) => {
                steps -= 1;
                let end = if forward {
                    order.first()?
                } else {
                    order.last()?
                };
                match Table::visible_key(&mut state, order, end, forward, &sieve)? {
                    Some(record) => record,
                    None => {
                        state.land_in(None, forward, true)?;
                        return Ok(());
                    }
                }
            }
            // Away from the scope, past its end.
            _ => {
                let empty = Table::first_visible(&mut state, order, &sieve)?.is_none();
                state.land_in(None, forward, empty)?;
                return Ok(());
            }
        };
        for _ in 0..steps {
            let next = order.step(forward)?;
            match Table::visible_key(&mut state, order, next, forward, &sieve)? {
                Some(next) => record = next,
                None => {
                    state.land_in(None, forward, false)?;
                    return Ok(());
                }
            }
        }
        state.land_in(Some(record), forward, false)?;
        Ok(())
    }

    /// Moves `records` keys forward in `order`, or back if negative, as
    /// `AdsSkipUnique` does: each move goes on past every record with the
    /// key of the one it starts from, forward to the first record of the
    /// next key, back to the last record of the previous one.
    pub fn skip_unique_in(&self, order: &Index, records: i64) -> Result<(), Error> {
        let step = records.signum();
        for _ in 0..records.unsigned_abs() {
            // (Bound first: locating locks the state.)
            let current = self.state().position.record;
            let key = match current {
                Some(record) if self.locate_in(order, record)? => order.current_key()?,
                _ => None,
            };
            loop {
                self.skip_in(order, step)?;
                let Some(record) = self.state().position.record else {
                    break;
                };
                if key.is_none() || !self.locate_in(order, record)? || order.current_key()? != key {
                    break;
                }
            }
        }
        // As in the original engine, BOF reached this way in an ascending
        // order is numbered as EOF.
        let mut state = self.state();
        if state.position.bof && !order.tag.descending {
            state.counted_past_end = true;
        }
        Ok(())
    }

    /// Seeks the last key of `order` that starts with `value`'s, as
    /// `AdsSeekLast` does, and returns whether it was found: on its last
    /// record if so, else at EOF, and at BOF too if no key comes before it
    /// (as in the original engine). Keys outside the scope or of records
    /// the filters hide are not found.
    pub fn seek_last_in(&self, order: &Index, value: SeekValue) -> Result<bool, Error> {
        if order.is_temporary() {
            return self.find_temporary(order, value, keys::Find::Last);
        }
        let key = self.sought_key(order, value)?;
        let sieve = self.sieve();
        let mut state = self.state();
        let (landed, before_all) = match order.seek_last(&key)? {
            Ok((record, Place::Inside)) if state.visible(record, &sieve)? => (Some(record), false),
            Ok(_) => (None, false),
            Err(any_before) => (None, !any_before),
        };
        state.land_in(landed, true, before_all)?;
        state.found = landed.is_some();
        Ok(state.found)
    }

    /// Seeks `value` in `order`, as `AdsSeek` does, and returns whether it
    /// was found; [`Table::found`] reports it until the next move. See
    /// [`crate::seek`] for the key that `value` seeks.
    ///
    /// A key the order has is found on its first record. A key is found when
    /// an order's key starts with it, so a shorter one finds the first key
    /// it begins (`abc` finds `abcdef`); an empty key finds the first key.
    /// Otherwise a soft seek lands on the first key after where the key
    /// would be, a hard one at EOF. [`SeekType::After`] lands on the first
    /// key after every key that starts with the sought one, and is found if
    /// there is one. Past the last key the table is at EOF only, even in an
    /// order without keys. A value that cannot be converted to a key fails
    /// and leaves the position (and the found flag) as they were.
    ///
    /// In a scope, a key before the scope is not found: a soft seek lands on
    /// the scope's first key (at BOF and EOF if the scope is empty), a hard
    /// one at EOF. A key after the scope is not found either, and goes to
    /// EOF.
    ///
    /// A key whose record the filters hide is not found either: a soft seek
    /// lands on the next visible record, a hard one at EOF.
    pub fn seek_in(&self, order: &Index, value: SeekValue, how: SeekType) -> Result<bool, Error> {
        if order.is_temporary() {
            return self.find_temporary(order, value, keys::Find::Seek(how));
        }
        // A CDX binary order (a bitmap) finds nothing: the original engine
        // goes to BOF and EOF.
        if order.kind == tflip_storage::order::Kind::Cdx && order.tag.binary() {
            let mut state = self.state();
            state.land_in(None, true, true)?;
            return Ok(false);
        }
        let key_length = usize::from(order.tag.key_length);
        let key = self.sought_key(order, value)?;
        let sieve = self.sieve();
        let mut state = self.state();
        let landed = match (how, key.is_empty()) {
            (SeekType::Soft | SeekType::Hard, true) => {
                let first = Table::first_visible(&mut state, order, &sieve)?;
                state.land_in(first, true, first.is_none())?;
                state.found = first.is_some();
                return Ok(state.found);
            }
            (SeekType::After, empty) => {
                // An empty key is the key of an empty value: all pad bytes.
                let key = if empty {
                    vec![order.tag.pad; key_length]
                } else {
                    key
                };
                match order.past(key, false) {
                    Some(key) => order
                        .seek(&key)?
                        .map(|(record, _, place)| (record, true, place)),
                    // Every key is at or below it: none is after it in an
                    // ascending order; in a descending one the first key is.
                    None if order.tag.descending => {
                        order.first()?.map(|record| (record, true, Place::Inside))
                    }
                    None => None,
                }
            }
            (_, false) => order.seek(&key)?,
        };
        let landed = match landed {
            Some((record, found, Place::Inside)) => Some((record, found)),
            Some((_, _, Place::Before)) if how != SeekType::Hard => match order.first()? {
                Some(first) => Some((first, how == SeekType::After)),
                None => {
                    state.land_in(None, true, true)?;
                    return Ok(false);
                }
            },
            _ => None,
        };
        // A hidden record, or a key a transaction's record has for others
        // than its own: on to the next visible one, not found (but a key
        // after the sought one is still one).
        let hidden = |state: &mut State, record| -> Result<bool, Error> {
            Ok(!state.visible(record, &sieve)? || !Table::key_seen(state, order, record)?)
        };
        let landed = match landed {
            Some((record, _)) if hidden(&mut state, record)? => {
                let next = order.step(true)?;
                let next = Table::visible_key(&mut state, order, next, true, &sieve)?;
                next.filter(|_| how != SeekType::Hard)
                    .map(|record| (record, how == SeekType::After))
            }
            landed => landed,
        };
        let landed = landed.filter(|&(_, found)| found || how == SeekType::Soft);
        state.land_in(landed.map(|(record, _)| record), true, false)?;
        state.found = landed.is_some_and(|(_, found)| found);
        Ok(state.found)
    }

    /// Sets the top of `order`'s scope (or its `bottom`) to the key `value`
    /// seeks, converted as [`Table::seek_in`] converts it, cut to the order's
    /// key length. The table does not move.
    pub fn set_scope(&self, order: &Index, bottom: bool, value: SeekValue) -> Result<(), Error> {
        let mut key = self.sought_key(order, value)?;
        // A Unicode key's text is read to its end, as ICU's sort keys are
        // written, with a 0 (`fixtures/write/nchar_expr`).
        if matches!(value, SeekValue::Text(_))
            && self.unicode_keys(order)
            && key.len() < usize::from(order.tag.key_length)
        {
            key.push(0);
        }
        order.set_scope(bottom, key);
        Ok(())
    }

    /// The connection the table was opened on, if any.
    /// The SQL statement whose cursor the table is, if it is one.
    pub fn statement(&self) -> Option<Handle> {
        self.statement
    }

    pub(crate) fn owner(&self) -> Option<&Arc<Connection>> {
        self.owner.as_ref()
    }

    /// A rollback to a savepoint is about to free the slots `freed`: a handle
    /// standing on one keeps its image, and an ADT handle its record count.
    pub(crate) fn keep_freed(&self, freed: &[(Arc<Shared>, u32)]) {
        let mut state = self.state();
        let Some(slot) = state.position.record else {
            return;
        };
        if !freed
            .iter()
            .any(|(shared, at)| Arc::ptr_eq(shared, &state.file) && *at == slot)
        {
            return;
        }
        let length = self.structure.header.record_length as usize;
        let (Ok(bytes), Ok(count)) = (state.read(slot, length), state.count()) else {
            return;
        };
        state.kept = Some((slot, bytes));
        if self.format() == Format::Adt {
            state.stale_count = Some(count);
        }
    }

    /// A rollback of the whole transaction freed the slots `freed` (and an
    /// earlier rollback to a savepoint the one the handle kept): an ADT handle
    /// standing on one is at BOF, its record number past the last slot
    /// (`fixtures/write/tx_cursor_after`).
    pub(crate) fn freed_by_rollback(&self, freed: &[(Arc<Shared>, u32)]) {
        let mut state = self.state();
        let kept = state.kept.take().map(|(slot, _)| slot);
        state.stale_count = None;
        let Some(slot) = state.position.record else {
            return;
        };
        let freed = kept == Some(slot)
            || freed
                .iter()
                .any(|(shared, at)| Arc::ptr_eq(shared, &state.file) && *at == slot);
        if freed && self.format() == Format::Adt && state.land_in(None, false, false).is_ok() {
            state.position = Position {
                record: None,
                bof: true,
                eof: false,
            };
            state.counted_past_end = true;
        }
    }

    /// The settings of the session the table belongs to.
    pub(crate) fn session_settings(&self) -> &Arc<Settings> {
        &self.session_settings
    }

    /// The date format of the table's connection (see
    /// [`connection::date_format`]), or of its session for a table opened
    /// on its own, for converting dates to and from text.
    pub fn date_format(&self) -> String {
        match &self.owner {
            Some(connection) => connection.date_format(),
            None => self.session_settings.date_format(),
        }
    }

    /// Checks `text`, about to be set as the dictionary's default, minimum or
    /// maximum of the table's field `name` (see
    /// [`crate::constraints::check_property`]); a field the table does not
    /// have is not checked.
    pub(crate) fn check_dictionary_value(
        &self,
        name: &str,
        property: crate::constraints::Property,
        text: &str,
    ) -> Result<(), Error> {
        let fields = self.fields();
        let Some(field) = fields.iter().find(|f| f.name.eq_ignore_ascii_case(name)) else {
            return Ok(());
        };
        let record = self.structure.null_record();
        crate::constraints::check_property(
            field,
            fields,
            property,
            text,
            |expression| self.compile(expression),
            &self.env(),
            &record,
        )
    }

    /// Compiles `expression` for the table; [`Error::Expression`] if it is
    /// not a valid expression, [`Error::InvalidExpression`] if it is empty.
    pub fn compile(&self, expression: &str) -> Result<Expr, Error> {
        if expression.is_empty() {
            return Err(Error::InvalidExpression);
        }
        let alias = self
            .path
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let schema = filter::Schema {
            alias: &alias,
            fields: self.fields(),
        };
        Expr::compile(expression, &schema).map_err(Error::Expression)
    }

    /// Evaluates `expression` on the current record (`AdsEval*Expr`): its
    /// type, and its value or why there is none (no current record, or a
    /// failure such as dividing by 0). The filters do not matter.
    pub fn eval(&self, expression: &str) -> Result<Evaluation, Error> {
        let expr = self.compile(expression)?;
        let env = self.env();
        let length = self.structure.header.record_length as usize;
        let mut state = self.state();
        let value = match state.position.record {
            None => Err(Error::NoCurrentRecord),
            Some(number) => {
                let bytes = state.read(number, length)?;
                state.with_memo(|memo| {
                    let mut row = filter::Record {
                        fields: self.fields(),
                        bytes,
                        memo,
                        number,
                    };
                    expr.eval(&mut row, &env).map_err(|failure| match failure {
                        tflip_expr::Failure::Runtime(runtime) => Error::Evaluation(runtime),
                        tflip_expr::Failure::Row(error) => error,
                    })
                })
            }
        };
        Ok(Evaluation {
            ty: expr.ty(),
            value,
        })
    }

    /// Goes past the last record, to EOF, keeping the found flag (a child
    /// whose relation's parent is off its records).
    pub(crate) fn go_past_end(&self) -> Result<(), Error> {
        let mut state = self.state();
        let found = state.found;
        state.land_in(None, true, false)?;
        state.found = found;
        Ok(())
    }

    /// Whether the last move was a seek that found its key.
    pub fn found(&self) -> bool {
        self.state().found
    }

    /// Moves to record number `record` (its slot). As in the original
    /// engine, 0 or a number past the last slot puts the table at BOF and
    /// EOF at once, from where a skip forward starts at the first record; a
    /// slot that holds a deleted record is [`Error::InvalidRecordNumber`].
    pub fn goto_record(&self, record: u32) -> Result<(), Error> {
        let sieve = self.sieve();
        let mut state = self.state();
        state.kept = None;
        // A handle without the password of an AES table, going to the record
        // whose change it cannot write, stays on it, the change kept
        // (`fixtures/aes/api/09_pending`).
        if !self.keyed()
            && state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.slot == record)
            && state.file().is_aes()
            && state.file().structure.header.encrypted()
        {
            return Ok(());
        }
        // A record whose delete is not written yet reads as deleted from the
        // handle's copy and is not written by going to it: a recall still
        // takes the delete back (`fixtures/write/binary_changes`).
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.deleted && !pending.appended && pending.slot == record)
        {
            return Err(Error::InvalidRecordNumber);
        }
        // A handle without the password of an AES table, which cannot write
        // the record it changed, stays on it going to it again
        // (`fixtures/write/dbf_aes_nopassword`).
        if shared::sealed() && state.pending.as_ref().is_some_and(|p| p.slot == record) && {
            let file = state.file();
            file.is_aes() && file.structure.header.encrypted()
        } {
            return Ok(());
        }
        state.flush()?;
        if record == 0 || record > state.slots() {
            state.found = false;
            state.counted_past_end = false;
            state.stranded = None;
            state.position = Position {
                record: None,
                bof: true,
                eof: true,
            };
            return Ok(());
        }
        // A record another transaction appended: the table is on no record,
        // its number kept, as the original's server leaves it
        // (`fixtures/write/tx_sql`).
        if state.appended_elsewhere(record)? {
            state.found = false;
            state.position = Position {
                record: None,
                bof: false,
                eof: false,
            };
            state.stranded = Some(record);
            return Ok(());
        }
        // A deleted record (an ADT table's, on the free list) is refused, but
        // the table is put on it, as in the original engine: it reads the
        // record's bytes and moves on from it (`fixtures/write/goto_deleted`).
        if !state.exists(record)? {
            state.land_in(Some(record), false, false)?;
            return Err(Error::InvalidRecordNumber);
        }
        state.land(Some(record), true, &sieve)
    }

    /// A checksum of the current record (`AdsGetRecordCRC`), for telling
    /// whether it changed: the CRC-32 (unfinished, see `crc32_unfinished`)
    /// of the record without its prefix (5 bytes in an ADT table, the
    /// deletion flag of a DBF one), followed by the values of
    /// its memo fields in field order. With `ignore_pages`, the memo fields'
    /// block numbers and lengths count as zeros, so moving a memo value does
    /// not change the checksum. All as the original engine computes it.
    pub fn record_crc(&self, ignore_pages: bool) -> Result<u32, Error> {
        // An encrypted table's record is the file's, and its memo values are
        // left out, without its password (`fixtures/sql/spt.json`).
        let unreadable = self.structure.header.encrypted() && !self.keyed();
        let mut record = match unreadable {
            true => {
                let state = self.state();
                let slot = state.position.record.ok_or(Error::NoCurrentRecord)?;
                let mut bytes = vec![0; self.structure.header.record_length as usize];
                state
                    .file()
                    .read_stored(slot, &mut bytes)
                    .map_err(storage_error)?;
                bytes
            }
            false => self.read_record()?,
        };
        let mut memos = Vec::new();
        for (i, field) in self
            .fields()
            .iter()
            .enumerate()
            .filter(|(_, f)| f.is_memo() && !unreadable)
        {
            if ignore_pages {
                // The block number and the length; the byte after them stays.
                let start = usize::from(field.offset);
                if let Some(pointer) = record.get_mut(start..start + 8) {
                    pointer.fill(0);
                }
            }
            memos.extend(self.read_memo(FieldRef::Number(i + 1))?);
        }
        let data: Vec<u8> = record
            .get(self.record_prefix()..)
            .unwrap_or_default()
            .iter()
            .copied()
            .chain(memos)
            .collect();
        Ok(crc32_unfinished(&data))
    }

    /// The local date the table's file was last written, as
    /// [`Table::last_update`], in the date format of the table's
    /// connection (`AdsGetLastTableUpdate`).
    pub fn last_update_text(&self) -> String {
        let layout = format::Layout {
            field_type: FieldType::Date,
            length: 0,
            decimals: 0,
        };
        match format::field_text(
            &Value::Date(self.last_update()),
            &layout,
            &self.date_format(),
        ) {
            format::FieldText::Text(text) => text,
            format::FieldText::Bytes(_) => String::new(),
        }
    }

    /// The raw bytes of the current record as `AdsGetRecord` gives them, its
    /// 5-byte prefix included: a VFP field that is NULL shows its blank
    /// bytes, whatever the file keeps.
    pub fn shown_record(&self) -> Result<Vec<u8>, Error> {
        let mut record = self.read_record()?;
        let revealed = self
            .state()
            .pending
            .as_ref()
            .map(|p| p.revealed.clone())
            .unwrap_or_default();
        for field in self
            .fields()
            .iter()
            .filter(|f| !revealed.contains(&f.offset))
        {
            field.show(&mut record);
        }
        Ok(record)
    }

    /// Notes that a getter read `field`: a VFP Money field that is NULL in
    /// the change of the current record shows its bytes in `AdsGetRecord`
    /// from now on, as in the original engine.
    pub fn reveal(&self, field: &Field) {
        if field.field_type != FieldType::Money {
            return;
        }
        let mut state = self.state();
        let Some(pending) = state.pending.as_mut() else {
            return;
        };
        if field.is_null(&pending.bytes) && !pending.revealed.contains(&field.offset) {
            pending.revealed.push(field.offset);
        }
    }

    /// The bytes of the current record, a change to it included.
    pub fn read_record(&self) -> Result<Vec<u8>, Error> {
        let mut state = self.state();
        let record = state.position.record.ok_or(Error::NoCurrentRecord)?;
        let length = self.structure.header.record_length;
        let mut bytes = state.read(record, length as usize)?;
        // The record a handle without the password deleted shows its
        // deletion, though it cannot write it (`fixtures/aes/api/05_delete`).
        if !self.keyed()
            && state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.slot == record && pending.deleted)
            && state.file().structure.header.encrypted()
            && self.format() == Format::Adt
            && let Some(flag) = bytes.first_mut()
        {
            *flag = tflip_storage::adt::DELETED | (*flag & tflip_storage::adt::TX_MARK);
        }
        Ok(bytes)
    }

    /// The format of the table's file.
    fn format(&self) -> Format {
        self.structure.header.format
    }

    /// What SQL expressions over the table's records need to know of it: the
    /// start of their `ROWID`s and whether it is a DBF table.
    pub(crate) fn sql_info(&self) -> tflip_sql::compile::TableInfo {
        let rowid_prefix = self.owner.as_ref().and_then(|owner| {
            let database = owner.path.display().to_string();
            let folder = self.path.parent()?.display().to_string();
            let name = self.path.file_name()?.to_str()?;
            let table = match self.object_id {
                Some(id) => tflip_sql::rowid::Table::Object(id),
                None => tflip_sql::rowid::Table::File {
                    folder: &folder,
                    name,
                },
            };
            Some(Arc::from(tflip_sql::rowid::prefix(&database, table)))
        });
        tflip_sql::compile::TableInfo {
            rowid_prefix,
            dbf: self.dbf_flavour().is_some(),
            oem: crate::getters::oem_table(&self.settings, &self.structure),
        }
    }

    /// The flavour of a DBF table; `None` for others.
    pub(crate) fn dbf_flavour(&self) -> Option<tflip_storage::dbf::Flavour> {
        match self.format() {
            Format::Dbf(flavour) => Some(flavour),
            _ => None,
        }
    }

    /// Whether the table is an OEM table: a Clipper or FoxPro table opened
    /// with `ADS_OEM`, whose character keys weigh by the OEM static collation
    /// (as in the original engine, ADT and VFP tables are ANSI whatever they
    /// are opened as).
    pub(crate) fn is_oem(&self) -> bool {
        self.settings.char_type == 2
            && matches!(
                self.dbf_flavour(),
                Some(tflip_storage::dbf::Flavour::Clipper | tflip_storage::dbf::Flavour::FoxPro)
            )
    }

    /// Whether the table was opened exclusively, so it takes no locks.
    pub(crate) fn exclusive(&self) -> bool {
        self.settings.options & options::EXCLUSIVE != 0
    }

    /// The AutoInc value of the last record this handle appended
    /// (`AdsGetLastAutoinc`).
    pub fn last_autoinc(&self) -> u32 {
        self.last_autoinc.load(Ordering::Relaxed)
    }

    /// Writes the pending changes and releases the handle's locks, as
    /// closing the table does. Changes that cannot be written (a key a unique
    /// order has already) are dropped, as the original engine drops them.
    pub(crate) fn finish(&self) {
        let flushed = self.state().flush();
        if let Err(error) = flushed {
            // A record a constrained cursor refused is taken back as an
            // append that was cancelled.
            let appended = self.state().pending.as_ref().is_some_and(|p| p.appended);
            let cancelled = matches!(
                error,
                Error::Constraint { code: 7065, .. }
                    | Error::Evaluation(tflip_expr::Runtime::KeyBuffer)
            );
            if appended && cancelled {
                let _ = self.cancel(false);
            }
            self.state().pending = None;
        }
        let state = self.state();
        state.file.release(state.owner);
    }

    /// The fields of the table.
    pub fn fields(&self) -> &[Field] {
        &self.structure.fields
    }

    /// The columns an SQL statement reads as NULL for the connection's user
    /// (see [`Table::hidden_in_sql`]).
    pub(crate) fn sql_hidden(&self) -> &[String] {
        match self.hidden_in_sql {
            true => &self.hidden,
            false => &[],
        }
    }

    /// The fields of the table itself: all of those a live SQL cursor shows
    /// some of.
    pub(crate) fn table_fields(&self) -> &[Field] {
        self.all_fields.as_deref().unwrap_or_else(|| self.fields())
    }

    /// The field of the table itself that an index's `F<number>` names: a
    /// live cursor that shows fewer fields, or others first, still keeps
    /// the orders of the whole table.
    pub(crate) fn key_field(&self, number: usize) -> Result<&Field, Error> {
        find_field(self.table_fields(), FieldRef::Number(number)).map(|(_, field)| field)
    }

    /// The field `field` refers to. Names match regardless of case.
    pub fn field(&self, field: FieldRef) -> Result<&Field, Error> {
        self.find_field(field).map(|(_, field)| field)
    }

    /// [`find_field`] in this table's fields, which a cursor's errors name
    /// columns of.
    pub(crate) fn find_field(&self, field: FieldRef) -> Result<(usize, &Field), Error> {
        find_field(self.fields(), field).map_err(|error| match (self.statement.is_some(), error) {
            (true, Error::InvalidFieldName) => Error::Constraint {
                code: 5113,
                text: "The column name specified was invalid for the cursor.".to_owned(),
            },
            (true, Error::InvalidFieldNumber) => Error::Constraint {
                code: 5112,
                text: "The column number specified was invalid for the cursor.".to_owned(),
            },
            (_, error) => error,
        })
    }

    /// The referential integrity rules that bear on the table (see
    /// [`crate::ri`]).
    pub fn ri(&self) -> &crate::ri::Rules {
        &self.ri
    }

    /// `expression` with each field name replaced by `F<n>` (*n* the 1-based
    /// field number), the form index files store, as the original engine's
    /// `AdsExpressionLongToShort90` does. Nothing is parsed or checked: a word
    /// (see `replace_words`) that names a field, in any case, becomes `F<n>`
    /// unless `(` (a function) or `.` (a table alias) follows it right away.
    pub fn with_field_numbers(&self, expression: &str) -> String {
        let replaced = replace_words(expression, |name, after| {
            if after.starts_with(['(', '.']) {
                return Ok(None);
            }
            let number = self
                .fields()
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(name));
            Ok(number.map(|i| format!("F{}", i + 1)))
        });
        replaced.unwrap_or_else(|never: std::convert::Infallible| match never {})
    }

    /// `expression` with each `F<n>` replaced by the name of field *n* as the
    /// table defines it, as the original engine's `AdsExpressionShortToLong90`
    /// does: the reverse of [`Table::with_field_numbers`]. A word (see
    /// `replace_words`) of an upper-case `F` and digits is a field number,
    /// whatever follows it; one that is not a field of the table is
    /// [`Error::InvalidFieldNumber`].
    pub fn with_field_names(&self, expression: &str) -> Result<String, Error> {
        replace_words(expression, |word, _| {
            let Some(digits) = word.strip_prefix('F') else {
                return Ok(None);
            };
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Ok(None);
            }
            let number = digits.parse().map_err(|_| Error::InvalidFieldNumber)?;
            Ok(Some(self.field(FieldRef::Number(number))?.name.clone()))
        })
    }
}

/// The field of `fields` that `field` refers to, with its 1-based number.
/// Names match regardless of case.
pub fn find_field<'f>(fields: &'f [Field], field: FieldRef) -> Result<(usize, &'f Field), Error> {
    match field {
        FieldRef::Number(n) => n
            .checked_sub(1)
            .and_then(|i| fields.get(i))
            .map(|field| (n, field))
            .ok_or(Error::InvalidFieldNumber),
        FieldRef::Name(name) => {
            let name = name.to_lowercase();
            fields
                .iter()
                .position(|f| f.name.to_lowercase() == name)
                .map(|i| (i + 1, &fields[i]))
                .ok_or(Error::InvalidFieldName)
        }
    }
}

/// `expression` with the words `replace` returns a replacement for
/// replaced. A word is a run of ASCII letters, digits and `_`; `replace` is
/// given it and the text after it. Closed quotes (`'...'`, `"..."`) and
/// brackets (`[...]`) are copied as they are, an unclosed one as any other
/// character, as the original engine does.
fn replace_words<E>(
    expression: &str,
    mut replace: impl FnMut(&str, &str) -> Result<Option<String>, E>,
) -> Result<String, E> {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut text = String::with_capacity(expression.len());
    let mut rest = expression;
    while let Some(c) = rest.chars().next() {
        let close = match c {
            '\'' | '"' => Some(c),
            '[' => Some(']'),
            _ => None,
        };
        let taken = if let Some(end) = close.and_then(|close| rest[1..].find(close)) {
            end + 2
        } else if word(c) {
            let end = rest.find(|c| !word(c)).unwrap_or(rest.len());
            if let Some(replacement) = replace(&rest[..end], &rest[end..])? {
                text += &replacement;
                rest = &rest[end..];
                continue;
            }
            end
        } else {
            c.len_utf8()
        };
        text += &rest[..taken];
        rest = &rest[taken..];
    }
    Ok(text)
}

/// The CRC-32 of `data` (the polynomial of zlib), without its final
/// inversion, as the original engine computes record CRCs.
fn crc32_unfinished(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                crc >> 1 ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc
}

/// The bits of open options (`ADS_EXCLUSIVE`, ...), as ACE numbers them.
pub mod options {
    pub const EXCLUSIVE: u32 = 0x0001;
    pub const READONLY: u32 = 0x0002;
    pub const SHARED: u32 = 0x0004;
    pub const PERM_READ: u32 = 0x0010;
    pub const PERM_UPDATE: u32 = 0x0020;
    pub const PERM_INSERT: u32 = 0x0040;
    pub const PERM_DELETE: u32 = 0x0080;
    /// `ADS_REINDEX_ON_COLLATION_MISMATCH`: an ADI file built under another
    /// static collation is rebuilt on opening.
    pub const REINDEX_ON_MISMATCH: u32 = 0x0100;
    /// `ADS_IGNORE_COLLATION_MISMATCH`: an index of another collation is
    /// opened as it is.
    pub const IGNORE_MISMATCH: u32 = 0x0200;
    pub const FREE_TABLE: u32 = 0x1000;
    pub const TEMP_TABLE: u32 = 0x2000;
    pub const DICTIONARY_BOUND_TABLE: u32 = 0x4000;
    /// A bit every opened table reports, which `ace.h` does not name.
    pub const OPENED: u32 = 0x1_0000;
    /// Not an ACE option: `AdsOpenTable101` opens the table with compatible
    /// locking, as its connection's `LockMode=COMPATIBLE` asks
    /// (`AdsGetTableLockType` reports it); never reported as an option.
    pub const COMPATIBLE_LOCKING: u32 = 0x2_0000;
    /// Not an ACE option: the handle `AdsCreateTable` returns, which the
    /// original reports without [`OPENED`] (`fixtures/aes/api/00_probe`).
    pub const CREATED: u32 = 0x4_0000;
    /// The bits of the options a table keeps for what they do to it, which
    /// `AdsGetTableOpenOptions` leaves out.
    pub const KEPT: u32 = REINDEX_ON_MISMATCH | IGNORE_MISMATCH | COMPATIBLE_LOCKING | CREATED;
}

impl Drop for Table {
    /// Writes what is pending and releases the locks, however the table is
    /// closed (`AdsCloseTable`, closing its connection).
    fn drop(&mut self) {
        self.finish();
        self.keep_open();
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.attached {
            state.file.stand(state.owner, 0);
            state.file.detach();
        }
        for file in self
            .temporary_indexes
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
        {
            let _ = std::fs::remove_file(file);
        }
    }
}

impl Table {
    /// As the table closes, has its connection keep its file open for reuse
    /// if it keeps tables (or statements' tables, for one an SQL statement
    /// opened) open (see [`crate::cache`]); not a table opened exclusively
    /// or an encrypted one.
    fn keep_open(&self) {
        let kind = match self.kept.load(Ordering::Relaxed) {
            1 => crate::cache::Kind::Tables,
            2 => crate::cache::Kind::Cursors,
            _ => return,
        };
        let Some(owner) = &self.owner else {
            return;
        };
        if self.cacheable() {
            let shared = self.shared();
            crate::cache::keep(owner, kind, &shared, &self.path, self.settings.table_type);
        }
    }

    /// Whether a connection may keep the table's file open after it closes:
    /// a table of a connection's, opened shared, not encrypted (the file
    /// would hold its password).
    fn cacheable(&self) -> bool {
        let (true, false, Some(_)) = (self.attached, self.exclusive(), &self.owner) else {
            return false;
        };
        let shared = self.shared();
        let mut file = shared.file();
        !(file.has_password() || file.unsealed(|file| file.encryption_cipher().is_some()))
    }

    /// Holds the table's file open for the SQL statement that opened it
    /// until it lets go of the [`crate::cache::Hold`], however long after
    /// the table closes; then its connection's statements' cache may keep
    /// it (see [`crate::cache`]). `None` for a table not opened from a file
    /// of a connection's, or opened exclusively.
    pub(crate) fn hold(&self) -> Option<crate::cache::Hold> {
        let (true, false, Some(owner)) = (self.attached, self.exclusive(), &self.owner) else {
            return None;
        };
        let shared = self.shared();
        shared.path()?;
        crate::cache::Hold::new(
            owner,
            shared,
            &self.path,
            self.settings.table_type,
            self.cacheable(),
        )
    }

    /// Lets the connection's `kind` cache keep the table's file open after
    /// it closes (see [`crate::cache`]).
    pub(crate) fn keep_open_as(&self, kind: crate::cache::Kind) {
        let kept = match kind {
            crate::cache::Kind::Tables => 1,
            crate::cache::Kind::Cursors => 2,
        };
        self.kept.store(kept, Ordering::Relaxed);
    }

    /// The table's file, shared with every handle on it.
    pub(crate) fn shared(&self) -> Arc<Shared> {
        Arc::clone(&lock(&self.state).file)
    }

    /// How the table is deciphered, if it is encrypted: its index files
    /// decipher the same way.
    pub(crate) fn encryption_cipher(&self) -> Option<tflip_storage::table::RecordCipher> {
        if !self.keyed() {
            return None;
        }
        self.shared()
            .file()
            .unsealed(|file| file.encryption_cipher())
    }

    /// Whether the table is a dictionary's and the dictionary encrypts the
    /// index files of its RC4 tables (`ENCRYPT_INDEXES`).
    pub(crate) fn dictionary_encrypts_indexes(&self) -> bool {
        self.object_id.is_some()
            && self
                .owner
                .as_ref()
                .is_some_and(|owner| crate::dictionary_write::rc4_tables::encrypts_indexes(owner))
    }

    /// The IV counter an AES table's records, memo values and index pages
    /// draw from, if it is one.
    pub(crate) fn aes_counter(&self) -> Option<Arc<std::sync::atomic::AtomicU64>> {
        if !self.keyed() {
            return None;
        }
        self.shared().file().unsealed(|file| file.aes_counter())
    }

    /// Whether the handle has given the table's encryption password (see
    /// [`Table::keyed`]).
    pub(crate) fn keyed(&self) -> bool {
        self.keyed.load(Ordering::Relaxed)
    }

    /// The number of record slots, deleted records' included.
    pub(crate) fn slot_count(&self) -> u32 {
        self.state().slots()
    }

    /// Leaves the handle on no record, neither at BOF nor at EOF, as the
    /// original engine leaves a table it zapped (its record number 0).
    pub(crate) fn unposition(&self) {
        let mut state = self.state();
        state.position = Position {
            record: None,
            bof: false,
            eof: false,
        };
        state.counted_past_end = false;
    }

    /// The table's file.
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The block size of the table's memo file, if it has one.
    pub(crate) fn memo_block_size(&self) -> Option<u32> {
        self.state().file.file().memo_block_size()
    }

    /// The changes the process made to the table's file (see
    /// [`TableFile::changes`]).
    pub(crate) fn file_changes(&self) -> u64 {
        self.state().changes().unwrap_or(0)
    }

    /// Makes the table a live cursor of `statement`, as `cursor` describes.
    pub(crate) fn make_cursor(&mut self, statement: Handle, cursor: LiveCursor) {
        let LiveCursor {
            fields,
            sql,
            sequence,
            sorter,
            constrain,
            order,
        } = cursor;
        self.statement = Some(statement);
        self.cursor_order = order;
        self.structure.header.field_count = fields.len() as u16;
        self.all_fields = Some(std::mem::replace(&mut self.structure.fields, fields));
        let sql = sql.map(Arc::new);
        let constrained = sql.clone().filter(|_| constrain);
        *self
            .filters
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(Filters {
            sql,
            ..Filters::default()
        });
        self.set_sequence(sequence);
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.sorter = sorter;
        state.constrained = constrained;
    }

    /// Makes the table count no records (see the field).
    pub(crate) fn set_uncounted(&mut self) {
        self.uncounted = true;
    }

    /// Orders the records without an index order as `sequence` lists their
    /// slots, if given.
    pub(crate) fn set_sequence(&mut self, sequence: Option<Vec<u32>>) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let slots = state.slots();
        state.sequence = sequence.map(|slots_in_order| Sequence::new(slots_in_order, slots));
    }

    /// Calls `f` on every record slot's bytes in slot order, deleted
    /// records included, with the memo file to read memo fields from.
    pub(crate) fn scan(
        &self,
        mut f: impl FnMut(u32, &[u8], Option<&mut MemoFile>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let length = self.structure.header.record_length as usize;
        let mut state = self.state();
        for slot in 1..=state.slots() {
            if !state.active(slot)? {
                continue;
            }
            let bytes = state.read(slot, length)?;
            state.with_memo(|memo| f(slot, &bytes, memo))?;
        }
        Ok(())
    }

    /// Calls `f` on the record `slot` as [`Table::scan`] would, if the table
    /// has it and it is active.
    pub(crate) fn read_active(
        &self,
        slot: u32,
        f: impl FnOnce(&[u8], Option<&mut MemoFile>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let length = self.structure.header.record_length as usize;
        let mut state = self.state();
        if slot == 0 || slot > state.slots() || !state.active(slot)? {
            return Ok(());
        }
        let bytes = state.read(slot, length)?;
        state.with_memo(|memo| f(&bytes, memo))
    }

    /// The fields and values of the current record as it is stored, without
    /// the changes pending to it (a trigger's `__old` of a table-API update);
    /// `None` if the table is on no record.
    pub fn stored_row(&self) -> Result<Option<Row>, Error> {
        let fields = self.fields().to_vec();
        let slot = self.record_number();
        if slot == 0 {
            return Ok(None);
        }
        let length = self.structure.header.record_length as usize;
        let mut state = self.state();
        let pending = state.pending.take();
        let bytes = state.read(slot, length);
        state.pending = pending;
        let bytes = bytes?;
        let values = state.with_memo(|memo| {
            let mut row = filter::SqlRow {
                fields: &fields,
                bytes: &bytes,
                memo,
                record: None,
                hidden: &[],
            };
            tflip_sql::dml::row_values(&fields, &mut row).map_err(Error::Sql)
        })?;
        Ok(Some((fields, values)))
    }

    /// The current record's bytes as it is stored, without the changes
    /// pending to it; `None` if the table is on no record.
    pub fn stored_bytes(&self) -> Result<Option<Vec<u8>>, Error> {
        let slot = self.record_number();
        if slot == 0 {
            return Ok(None);
        }
        let length = self.structure.header.record_length as usize;
        let mut state = self.state();
        let pending = state.pending.take();
        let bytes = state.read(slot, length);
        state.pending = pending;
        bytes.map(Some)
    }

    /// The fields and values of the current (committed) record, for referential
    /// integrity; `None` if the table is on no record.
    pub fn ri_current_row(&self) -> Result<Option<Row>, Error> {
        let fields = self.fields().to_vec();
        let slot = self.record_number();
        if slot == 0 {
            return Ok(None);
        }
        let mut out = None;
        self.read_active(slot, |bytes, memo| {
            let mut row = filter::SqlRow {
                fields: &fields,
                bytes,
                memo,
                record: None,
                hidden: &[],
            };
            out = Some(tflip_sql::dml::row_values(&fields, &mut row).map_err(Error::Sql)?);
            Ok(())
        })?;
        Ok(out.map(|values| (fields, values)))
    }

    /// The fields and values of the record about to be written — the pending
    /// change if there is one, else the current record — for the foreign-key
    /// check of a table-API insert or update.
    pub fn ri_pending_row(&self) -> Result<Option<Row>, Error> {
        let fields = self.fields().to_vec();
        let state = self.state();
        let bytes = match &state.pending {
            Some(pending) => pending.bytes.clone(),
            None => {
                drop(state);
                return self.ri_current_row();
            }
        };
        let values = state.with_memo(|memo| {
            let mut row = filter::SqlRow {
                fields: &fields,
                bytes: &bytes,
                memo,
                record: None,
                hidden: &[],
            };
            tflip_sql::dml::row_values(&fields, &mut row).map_err(Error::Sql)
        })?;
        Ok(Some((fields, values)))
    }

    /// Whether the table is not one built in memory, so it has index files
    /// to open.
    fn statement_is_live(&self) -> bool {
        !self.built
    }

    /// Sets the order a static SQL cursor shows its records in, by name
    /// (see [`Table::cursor_order`]).
    pub(crate) fn set_cursor_order(&mut self, order: Option<String>) {
        self.cursor_order = order;
    }

    /// Whether the table is an SQL statement's cursor.
    pub(crate) fn is_cursor(&self) -> bool {
        self.statement.is_some()
    }

    /// The name of the order an SQL cursor reads its records in, if it
    /// reads them in one.
    pub(crate) fn cursor_order(&self) -> Option<&str> {
        self.cursor_order.as_deref()
    }

    /// Whether the table was built in memory: a static SQL cursor.
    pub fn is_built(&self) -> bool {
        self.built
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::Session;

    fn settings(table_type: TableType, char_type: u16, options: u32) -> TableSettings {
        TableSettings {
            table_type,
            char_type,
            collation: "ansi".to_owned(),
            options,
            ..TableSettings::default()
        }
    }

    /// The ADT fixtures written by the original engine.
    /// The fixtures' folder, named as a connection names it.
    fn fixtures() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt");
        PathBuf::from(tflip_storage::universal_path(&dir.to_string_lossy()))
    }

    fn connect_to(session: &Session, dir: &Path) -> Handle {
        connection::connect(
            session,
            &format!("Data Source={};ServerType=LOCAL;", dir.display()),
        )
        .unwrap()
    }

    #[test]
    fn open_and_get() {
        let session = Session::new();
        let dir = fixtures().join("all_types");
        let connection = connect_to(&session, &dir);

        let table = open(
            &session,
            connection,
            "all_types.adt",
            settings(TableType::Adt, 1, 4),
        )
        .unwrap();
        let opened = get(&session, table).unwrap();
        assert_eq!(opened.connection, connection);
        assert_eq!(opened.path, dir.join("all_types.adt"));
        // The options as the original engine reports them (shared: all
        // permissions).
        assert_eq!(opened.settings, settings(TableType::Adt, 1, 0x1_00F0));
        let structure = &opened.structure;
        assert_eq!(structure.header.record_count(), 5);
        let info = opened.info();
        assert_eq!(
            (info.connection, &info.settings),
            (connection, &opened.settings)
        );
        assert_eq!(&info.structure, structure);
        assert_eq!(structure.fields.len(), 24);
        assert_eq!(structure.fields[0].name, "c_char");

        // Type Database is a dictionary's tables' only, whether the file is
        // there or not (`fixtures/write/database_type`).
        for name in ["all_types.adt", "/elsewhere/x.dbf"] {
            let rc = open(
                &session,
                connection,
                name,
                settings(TableType::Database, 1, 0),
            );
            assert_eq!(rc.unwrap_err().plain(), &Error::InvalidTableType);
        }

        assert_eq!(get(&session, connection).unwrap_err(), Error::InvalidTable);
        let rc = open(
            &session,
            0x12345,
            "all_types.adt",
            settings(TableType::Adt, 1, 0),
        );
        assert_eq!(rc, Err(Error::InvalidConnection));

        let path = dir.join("all_types.adt");
        let free = open(
            &session,
            0,
            path.to_str().unwrap(),
            settings(TableType::Adt, 1, 0),
        )
        .unwrap();
        assert_eq!(get(&session, free).unwrap().connection, 0);
        connection::disconnect(&session, connection).unwrap();
        assert_eq!(get(&session, table).unwrap_err(), Error::InvalidTable);
        assert!(get(&session, free).is_ok());
        assert_eq!(close(&session, free), Ok(()));
        assert_eq!(close(&session, free), Err(Error::InvalidTable));
    }

    /// Under a static collation, `UPPER` and `LOWER` in an expression fold case
    /// as the original engine does — the byte folded through the collation's
    /// case table and weighed — not by ASCII. Replays `fixtures/expr/case.json`
    /// (the `charset_german` `ca` table under `ANSI_CHAR_SET=German`): the
    /// values of `UPPER(c)`, `LOWER(c)`, `UPPER(w)` and `LOWER(w)` on every
    /// record must match `AdsEvalStringExpr`'s (see `tools/ace/expr/run_case.sh`).
    #[test]
    fn collation_folds_case_in_expressions() {
        let dir = std::env::var_os("TFLIP_COLLATION_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let home = PathBuf::from(std::env::var_os("HOME")?);
                Some(home.join("src/ace-sdk/drive_c/Program Files (x86)/Advantage 12.0/acesdk"))
            });
        let Some(dir) = dir.filter(|dir| dir.join("ansi.chr").exists()) else {
            return;
        };
        let session = Session::new();
        crate::settings::set_collations(
            &session,
            crate::collations::Collations::new(&dir, "German", ""),
        );
        let path = fixtures().join("charset_german/ca.adt");
        let table = open(
            &session,
            0,
            path.to_str().unwrap(),
            settings(TableType::Adt, 1, 0),
        )
        .unwrap();
        let opened = get(&session, table).unwrap();

        let golden: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(fixtures().join("../expr/case.json")).unwrap(),
        )
        .unwrap();
        let mut checked = 0;
        for expression in golden["expressions"].as_array().unwrap() {
            let expr = expression["expr"].as_str().unwrap();
            for record in expression["records"].as_array().unwrap() {
                let number = record["recno"].as_u64().unwrap() as u32;
                opened.goto_record(number).unwrap();
                let value = opened.eval(expr).unwrap().value.unwrap();
                // A NULL field is a NULL result; the original's AdsEvalStringExpr
                // renders that as a single 0x00 byte, a quirk of the call, not of
                // case folding, so those records are left out here.
                let tflip_expr::Val::Str(bytes) = value else {
                    continue;
                };
                let got: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                assert_eq!(got, record["S"].as_str().unwrap(), "{expr} record {number}");
                checked += 1;
            }
        }
        assert!(checked > 400, "checked {checked}");
        close(&session, table).unwrap();
    }

    /// SQL `UPPER` and `LOWER` fold case by the static collation, as the
    /// original engine does: on the `charset_german` `ca` table under
    /// `ANSI_CHAR_SET=German`, `UPPER(c)` and `LOWER(c)` of each byte must
    /// match the engine's (`fixtures/sql/collation_case.json`, probed with
    /// `tools/ace/sql/collation.txt` and `run_collation.sh`). Needs the
    /// collation files, as the German collation does.
    #[test]
    fn collation_folds_case_in_sql() {
        let dir = std::env::var_os("TFLIP_COLLATION_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let home = PathBuf::from(std::env::var_os("HOME")?);
                Some(home.join("src/ace-sdk/drive_c/Program Files (x86)/Advantage 12.0/acesdk"))
            });
        let Some(dir) = dir.filter(|dir| dir.join("ansi.chr").exists()) else {
            return;
        };
        let session = Session::new();
        crate::settings::set_collations(
            &session,
            crate::collations::Collations::new(&dir, "German", ""),
        );
        let folder = fixtures().join("charset_german");
        let connection =
            crate::connection::connect(&session, &format!("Data Source={};", folder.display()))
                .unwrap();
        let statement = crate::statement::create(&session, connection).unwrap();
        let cursor = crate::statement::execute_direct(
            &session,
            statement,
            "SELECT code, c, UPPER(c) AS uc, LOWER(c) AS lc FROM ca ORDER BY code",
            true,
        )
        .unwrap()
        .unwrap();
        let cursor = get(&session, cursor).unwrap();

        let golden: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(fixtures().join("../sql/collation_case.json")).unwrap(),
        )
        .unwrap();
        let byte = |t: &Table, field: usize| -> u8 {
            match t.value(FieldRef::Number(field)).unwrap() {
                Value::Text(s) => tflip_storage::cp1252::encode(&s)
                    .first()
                    .copied()
                    .unwrap_or(b' '),
                other => panic!("field {field} is not text: {other:?}"),
            }
        };
        cursor.goto_top().unwrap();
        let mut checked = 0;
        for record in golden["records"].as_array().unwrap() {
            let want = record["byte"].as_u64().unwrap() as u8;
            assert_eq!(byte(&cursor, 2), want, "c of byte {want}");
            assert_eq!(
                byte(&cursor, 3),
                record["upper"].as_u64().unwrap() as u8,
                "UPPER {want}"
            );
            assert_eq!(
                byte(&cursor, 4),
                record["lower"].as_u64().unwrap() as u8,
                "LOWER {want}"
            );
            cursor.skip(1).unwrap();
            checked += 1;
        }
        assert_eq!(checked, 200);
        crate::connection::disconnect(&session, connection).unwrap();
    }

    /// String comparisons weigh bytes by the static collation, as the original
    /// engine does: on the `charset_german` `ca` table under
    /// `ANSI_CHAR_SET=German`, each byte compared with reference characters must
    /// match `AdsEvalLogicalExpr` (`fixtures/expr/compare.json`, probed with
    /// `tools/ace/expr/compare.txt`). Needs the collation files, as the German
    /// collation does.
    #[test]
    fn collation_weighs_string_comparisons() {
        let dir = std::env::var_os("TFLIP_COLLATION_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let home = PathBuf::from(std::env::var_os("HOME")?);
                Some(home.join("src/ace-sdk/drive_c/Program Files (x86)/Advantage 12.0/acesdk"))
            });
        let Some(dir) = dir.filter(|dir| dir.join("ansi.chr").exists()) else {
            return;
        };
        let session = Session::new();
        crate::settings::set_collations(
            &session,
            crate::collations::Collations::new(&dir, "German", ""),
        );
        let path = fixtures().join("charset_german/ca.adt");
        let table = open(
            &session,
            0,
            path.to_str().unwrap(),
            settings(TableType::Adt, 1, 0),
        )
        .unwrap();
        let opened = get(&session, table).unwrap();

        let golden: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(fixtures().join("../expr/compare.json")).unwrap(),
        )
        .unwrap();
        let mut checked = 0;
        for expression in golden["expressions"].as_array().unwrap() {
            let expr = expression["expr"].as_str().unwrap();
            for record in expression["records"].as_array().unwrap() {
                let number = record["recno"].as_u64().unwrap() as u32;
                opened.goto_record(number).unwrap();
                // Records 225..238 hold a NULL `c` (the accented-word rows); a
                // NULL comparison is NULL here, which `AdsEvalLogicalExpr`
                // renders as F — a quirk of the call, not of the comparison, so
                // those rows are left out, as the case-folding test leaves out
                // its NULLs.
                let tflip_expr::Val::Logical(got) = opened.eval(expr).unwrap().value.unwrap()
                else {
                    continue;
                };
                let want = record["L"].as_str().unwrap() == "T";
                assert_eq!(got, want, "{expr} record {number}");
                checked += 1;
            }
        }
        assert!(checked > 2000, "checked {checked}");
        close(&session, table).unwrap();
    }

    #[test]
    fn fields_by_number_and_name() {
        let session = Session::new();
        let connection = connect_to(&session, &fixtures().join("all_types"));
        let table = open(
            &session,
            connection,
            "all_types.adt",
            settings(TableType::Adt, 1, 0),
        )
        .unwrap();
        let table = get(&session, table).unwrap();

        let first = table.field(FieldRef::Number(1)).unwrap();
        assert_eq!(first.name, "c_char");
        assert_eq!(first.field_type, FieldType::Character);
        assert_eq!(
            table.field(FieldRef::Number(24)).unwrap().name,
            "c_rowversion"
        );
        assert_eq!(
            table.field(FieldRef::Number(0)),
            Err(Error::InvalidFieldNumber)
        );
        assert_eq!(
            table.field(FieldRef::Number(25)),
            Err(Error::InvalidFieldNumber)
        );

        let money = table.field(FieldRef::Name("C_MONEY")).unwrap();
        assert_eq!(money.field_type, FieldType::Money);
        assert_eq!(
            table.field(FieldRef::Name("nope")),
            Err(Error::InvalidFieldName)
        );
        connection::disconnect(&session, connection).unwrap();
    }

    /// DBF tables: read as the table type they are opened as; deleted
    /// records shown or hidden as `AdsShowDeleted` says at the time, and
    /// reached by number either way.
    #[test]
    fn dbf_tables() {
        let session = Session::new();
        let dir = fixtures().join("../ntx/dbf_types");
        let dir = PathBuf::from(tflip_storage::universal_path(&dir.to_string_lossy()));
        let connection = connect_to(&session, &dir);
        let handle = open(
            &session,
            connection,
            "dbf_types",
            settings(TableType::Ntx, 1, 0),
        )
        .unwrap();
        let table = get(&session, handle).unwrap();
        assert_eq!(table.path, dir.join("dbf_types.dbf"));
        assert_eq!(table.fields().len(), 5);
        assert_eq!(table.dbf.as_ref().unwrap().version, 0x83);
        assert_eq!(table.record_count(), Ok(4));
        let text = |name| match table.value(FieldRef::Name(name)).unwrap() {
            Value::Text(text) => text,
            other => panic!("{other:?}"),
        };
        assert_eq!(text("C_CHAR"), "abc       ");
        assert_eq!(text("C_MEMO"), "memo text");
        assert_eq!(
            table.value(FieldRef::Name("C_DATE")),
            Ok(Value::Date(2458066))
        );

        // Record 4 is deleted.
        table.goto_bottom().unwrap();
        assert_eq!(table.record_number(), 4);
        crate::settings::set_show_deleted(&session, false);
        assert_eq!(table.visible_count(), Ok(3));
        table.goto_bottom().unwrap();
        assert_eq!(table.record_number(), 3);
        table.goto_record(4).unwrap();
        assert_eq!(table.record_number(), 4);
        crate::settings::set_show_deleted(&session, true);
        assert_eq!(table.visible_count(), Ok(4));

        assert_eq!(
            open(
                &session,
                connection,
                "nope.dbf",
                settings(TableType::Cdx, 1, 0)
            ),
            Err(Error::FileNotFound)
        );
        let adt = fixtures().join("all_types/all_types.adt");
        let adt = open(
            &session,
            0,
            adt.to_str().unwrap(),
            settings(TableType::Cdx, 1, 0),
        );
        assert_eq!(adt, Err(Error::CorruptTable));
        connection::disconnect(&session, connection).unwrap();
    }

    /// Opens the ADT fixture table `name` (such as `freelist/fl_chain.adt`)
    /// on its own.
    fn open_fixture(name: &str) -> Arc<Table> {
        let session = Session::new();
        let path = fixtures().join(name);
        // Some fixtures belong to a dictionary, whose tables a client opens
        // only through it.
        let table = opening_free_itself(|| {
            open(
                &session,
                0,
                path.to_str().unwrap(),
                settings(TableType::Adt, 1, 0),
            )
        })
        .unwrap();
        let opened = get(&session, table).unwrap();
        session.remove(table);
        opened
    }

    /// The record numbers from the current position on, moving forward or
    /// back one record at a time until EOF or BOF.
    fn walk(table: &Table, forward: bool) -> Vec<u32> {
        let mut records = Vec::new();
        while let Some(record) = table.position().record {
            records.push(record);
            table.skip(if forward { 1 } else { -1 }).unwrap();
        }
        records
    }

    #[test]
    fn navigation_matches_the_engine() {
        let session = Session::new();
        let mut checked = 0;
        for dir in std::fs::read_dir(fixtures()).unwrap() {
            for entry in std::fs::read_dir(dir.unwrap().path()).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_none_or(|e| e != "adt") {
                    continue;
                }
                let s = path.to_str().unwrap();
                // Opened with the dynamic collation its orders have, as the
                // original engine asks.
                let collation = index::dynamic_collations(&path).into_iter().next();
                let mut settings = settings(TableType::Adt, 1, 0);
                settings.collation = collation.unwrap_or_default();
                let Ok(table) = opening_free_itself(|| open(&session, 0, s, settings)) else {
                    continue;
                };
                let opened = get(&session, table).unwrap();
                let json = std::fs::read_to_string(path.with_extension("json")).unwrap();
                let dump: serde_json::Value = serde_json::from_str(&json).unwrap();
                let expected: Vec<u32> = dump["records"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r["recno"].as_u64().unwrap() as u32)
                    .collect();

                // Opened on the first record; forward to EOF, back to BOF.
                assert_eq!(opened.position().record, expected.first().copied(), "{s}");
                assert_eq!(walk(&opened, true), expected, "{s}");
                assert!(opened.position().eof, "{s}");
                let mut backward = walk(&opened, false);
                assert!(backward.is_empty(), "{s}: EOF is past the last record");
                opened.skip(-1).unwrap();
                backward = walk(&opened, false);
                backward.reverse();
                assert_eq!(backward, expected, "{s}");
                assert!(opened.position().bof, "{s}");

                // The first record's raw bytes.
                if let Some(first) = dump["records"].as_array().unwrap().first() {
                    opened.goto_top().unwrap();
                    let raw: String = opened
                        .read_record()
                        .unwrap()
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    assert_eq!(raw, first["raw"].as_str().unwrap().to_lowercase(), "{s}");
                }
                session.remove(table);
                checked += 1;
            }
        }
        assert!(checked >= 30, "checked only {checked} tables");
    }

    #[test]
    fn moves_and_ends() {
        // Slots 1, 3 and 6 hold deleted records.
        let table = open_fixture("freelist/fl_chain.adt");
        let on = |record| Position {
            record: Some(record),
            bof: false,
            eof: false,
        };
        assert_eq!(table.position(), on(2));
        table.goto_bottom().unwrap();
        assert_eq!(table.position(), on(8));
        assert_eq!(table.record_number(), 8);
        // Counting the visible records only: 2, 4, 5, 7 and 8.
        assert_eq!(table.visible_record_number(), Ok(5));
        table.skip(1).unwrap();
        assert_eq!(
            table.position(),
            Position {
                record: None,
                bof: false,
                eof: true
            }
        );
        assert_eq!(table.record_number(), 9);
        assert_eq!(table.visible_record_number(), Ok(6));
        table.skip(1).unwrap();
        assert!(table.position().eof);
        table.skip(-2).unwrap();
        assert_eq!(table.position(), on(7));
        table.skip(-10).unwrap();
        assert_eq!(
            table.position(),
            Position {
                record: None,
                bof: true,
                eof: false
            }
        );
        assert_eq!(table.record_number(), 0);
        assert_eq!(table.visible_record_number(), Ok(0));
        table.skip(3).unwrap();
        assert_eq!(table.position(), on(5));
        table.goto_bof().unwrap();
        table.skip(1).unwrap();
        assert_eq!(table.position(), on(2));
        table.goto_eof().unwrap();
        assert_eq!(
            table.position(),
            Position {
                record: None,
                bof: false,
                eof: true
            }
        );
        table.skip(-1).unwrap();
        assert_eq!(table.position(), on(8));
        table.goto_record(2).unwrap();
        table.skip(0).unwrap();
        assert_eq!(table.position(), on(2));

        table.goto_record(7).unwrap();
        assert_eq!(table.position(), on(7));
        assert_eq!(table.goto_record(3), Err(Error::InvalidRecordNumber));
        // As in the original engine, the table is on the deleted record.
        assert_eq!(table.position(), on(3));
        // 0 and past the last slot go to BOF and EOF at once.
        let both = Position {
            record: None,
            bof: true,
            eof: true,
        };
        for past in [0, 9] {
            table.goto_record(past).unwrap();
            assert_eq!((table.position(), table.record_number()), (both, 9));
            table.skip(1).unwrap();
            assert_eq!(table.position(), on(2), "forward from before the first");
            table.goto_record(past).unwrap();
            table.skip(-1).unwrap();
            assert_eq!(
                table.position(),
                Position {
                    record: None,
                    bof: true,
                    eof: false
                }
            );
        }
        table.goto_record(7).unwrap();

        table.goto_bof().unwrap();
        assert_eq!(table.read_record(), Err(Error::NoCurrentRecord));
    }

    #[test]
    fn nulls() {
        let table = open_fixture("all_types/all_types.adt");
        table.goto_record(1).unwrap();
        assert_eq!(table.is_null(FieldRef::Name("c_integer")), Ok(false));
        table.goto_record(3).unwrap();
        assert_eq!(table.is_null(FieldRef::Name("c_integer")), Ok(true));
        assert_eq!(table.is_null(FieldRef::Name("c_autoinc")), Ok(false));
        assert_eq!(
            table.is_null(FieldRef::Number(99)),
            Err(Error::InvalidFieldNumber)
        );
        table.goto_bof().unwrap();
        assert_eq!(
            table.is_null(FieldRef::Number(1)),
            Err(Error::NoCurrentRecord)
        );
    }

    #[test]
    fn memo_lengths() {
        let table = open_fixture("all_types/all_types.adt");
        table.goto_record(1).unwrap();
        assert!(table.memo_length(FieldRef::Name("c_memo")).unwrap() > 0);
        table.goto_record(3).unwrap();
        assert_eq!(table.memo_length(FieldRef::Name("c_blob")), Ok(0));
        let rc = table.memo_length(FieldRef::Name("c_integer"));
        assert_eq!(
            rc.as_ref().map_err(Error::plain),
            Err(&Error::InvalidFieldType)
        );
        assert_eq!(
            rc,
            Err(Error::InvalidFieldType.detailed("Field: \"c_integer\"".to_owned()))
        );
    }

    #[test]
    fn data_lengths() {
        let table = open_fixture("all_types/all_types.adt");
        table.goto_record(1).unwrap();
        let length = |name, unit| table.data_length(FieldRef::Name(name), unit).unwrap();
        // The variable lengths are the ones stored in the record itself.
        let record = table.read_record().unwrap();
        let varchar = table.field(FieldRef::Name("c_varchar")).unwrap();
        let stored = u32::from(varchar.var_length(&record).unwrap());
        assert_eq!(length("c_varchar", LengthUnit::CodeUnits), stored);
        assert_eq!(length("c_varchar", LengthUnit::RecordBytes), 22);
        let nvarchar = table.field(FieldRef::Name("c_nvarchar")).unwrap();
        let units = u32::from(nvarchar.var_length(&record).unwrap());
        assert_eq!(length("c_nvarchar", LengthUnit::CodeUnits), units);
        assert_eq!(length("c_nvarchar", LengthUnit::Bytes), 2 * units);
        assert_eq!(length("c_nchar", LengthUnit::CodeUnits), 5);
        assert_eq!(length("c_nchar", LengthUnit::Bytes), 10);
        assert_eq!(length("c_char", LengthUnit::CodeUnits), 10);
        assert_eq!(length("c_integer", LengthUnit::Bytes), 4);
        let memo = table.memo_length(FieldRef::Name("c_memo")).unwrap();
        assert_eq!(length("c_memo", LengthUnit::CodeUnits), memo);
        assert_eq!(length("c_memo", LengthUnit::RecordBytes), 9);

        table.goto_record(3).unwrap();
        assert_eq!(length("c_varchar", LengthUnit::CodeUnits), 0, "NULL");
        assert_eq!(length("c_memo", LengthUnit::CodeUnits), 0, "NULL");
        // Fixed-length fields report their length even when NULL.
        assert_eq!(length("c_integer", LengthUnit::CodeUnits), 4, "NULL");
        assert_eq!(length("c_char", LengthUnit::CodeUnits), 10, "NULL");
        assert_eq!(length("c_nchar", LengthUnit::CodeUnits), 5, "NULL");
        assert_eq!(length("c_date", LengthUnit::CodeUnits), 4, "NULL");
        assert_eq!(length("c_timestamp", LengthUnit::CodeUnits), 8, "NULL");
        assert_eq!(length("c_integer", LengthUnit::RecordBytes), 4);
    }

    #[test]
    fn values() {
        let table = open_fixture("all_types/all_types.adt");
        table.goto_record(1).unwrap();
        assert_eq!(
            table.value(FieldRef::Name("c_char")),
            Ok(Value::Text("abc       ".into()))
        );
        assert_eq!(
            table.value(FieldRef::Name("c_integer")),
            Ok(Value::Integer(123456789))
        );
        assert_eq!(
            table.value(FieldRef::Name("c_date")),
            Ok(Value::Date(2458066))
        );
        assert_eq!(
            table.value(FieldRef::Name("c_memo")),
            Ok(Value::Text("memo text".into()))
        );
        table.goto_record(3).unwrap();
        assert_eq!(table.value(FieldRef::Name("c_integer")), Ok(Value::Null));
    }

    #[test]
    fn memo_values() {
        let table = open_fixture("memo_compressed/compressed.adt");
        table.goto_record(2).unwrap();
        let text = table.read_memo(FieldRef::Name("m")).unwrap();
        assert!(text.starts_with(b"compressible text"));
        assert_eq!(
            text.len() as u32,
            table.memo_length(FieldRef::Name("m")).unwrap()
        );
        let rc = table.read_memo(FieldRef::Name("id"));
        assert_eq!(
            rc.as_ref().map_err(Error::plain),
            Err(&Error::InvalidFieldType)
        );
    }

    #[test]
    fn empty_tables_are_at_both_ends() {
        let both = Position {
            record: None,
            bof: true,
            eof: true,
        };
        let table = open_fixture("dictionary_props/extra.adt");
        assert_eq!(table.position(), both);
        assert_eq!(table.record_number(), 1);
        table.goto_bottom().unwrap();
        assert_eq!(table.position(), both);
        table.skip(1).unwrap();
        assert_eq!(table.position(), both);
    }

    #[test]
    fn open_failures() {
        let session = Session::new();
        let connection = connect_to(&session, &fixtures());
        let adt = || settings(TableType::Adt, 1, 0);
        assert_eq!(
            open(&session, connection, "missing.adt", adt()),
            Err(Error::FileNotFound)
        );
        assert_eq!(
            open(&session, connection, "all_types/all_types.json", adt()),
            Err(Error::CorruptTable)
        );
        // A dictionary's table whose structure is encrypted: the password is
        // wrong, naming the file (`fixtures/write/encryption/freeopen.json`).
        let encrypted = "dictionary_encrypted/rc4_secret.adt";
        let file = fixtures().join(encrypted).display().to_string();
        let expected = Err(Error::InvalidPassword.detailed(file));
        assert_eq!(open(&session, connection, encrypted, adt()), expected);
        connection::disconnect(&session, connection).unwrap();
    }

    #[test]
    fn encrypts_a_table_through_the_engine_like_the_engine() {
        // Copy the plaintext base table (with its memo) to a scratch dir, then
        // AdsEnableEncryption + AdsEncryptTable through the engine and check the
        // files byte-for-byte against the engine's own output.
        let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/write/encryption");
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-eng-enc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(fx.join("base.adt"), dir.join("t.adt")).unwrap();
        std::fs::copy(fx.join("base.adm"), dir.join("t.adm")).unwrap();

        let session = Session::new();
        let connection = connect_to(&session, &dir);
        let handle = open(
            &session,
            connection,
            "t.adt",
            settings(TableType::Adt, 1, options::EXCLUSIVE),
        )
        .unwrap();
        let table = get(&session, handle).unwrap();
        table.enable_encryption(b"secret").unwrap();
        assert!(table.encryption_enabled());
        table.encrypt_table().unwrap();
        // The records read back deciphered in the same session.
        table.goto_record(1).unwrap();
        assert_eq!(
            table.value(FieldRef::Name("D1")),
            Ok(Value::Text(format!("alpha{:15}", "")))
        );
        assert_eq!(
            table.value(FieldRef::Name("M1")),
            Ok(Value::Text("memo-alpha".into()))
        );
        connection::disconnect(&session, connection).unwrap();

        assert_eq!(
            std::fs::read(dir.join("t.adt")).unwrap(),
            std::fs::read(fx.join("enctable.adt")).unwrap(),
            ".adt"
        );
        assert_eq!(
            std::fs::read(dir.join("t.adm")).unwrap(),
            std::fs::read(fx.join("enctable.adm")).unwrap(),
            ".adm"
        );

        // Opening the encrypted result (a fresh copy, to avoid the in-process
        // open cache) without the password is refused; with it, records read.
        let dir2 = tflip_storage::testing::temp_root()
            .join(format!("tflip-eng-enc2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir2);
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::copy(dir.join("t.adt"), dir2.join("t.adt")).unwrap();
        std::fs::copy(dir.join("t.adm"), dir2.join("t.adm")).unwrap();
        // Reopen the encrypted result (a fresh copy, to avoid the in-process
        // open cache) with the password: its records read back deciphered.
        let session = Session::new();
        let connection = connect_to(&session, &dir2);
        let mut pw = settings(TableType::Adt, 1, 0);
        pw.password = "secret".to_owned();
        let handle = open(&session, connection, "t.adt", pw).unwrap();
        let table = get(&session, handle).unwrap();
        table.goto_record(2).unwrap();
        assert_eq!(
            table.value(FieldRef::Name("D1")),
            Ok(Value::Text(format!("bravo{:15}", "")))
        );
        assert_eq!(
            table.value(FieldRef::Name("M1")),
            Ok(Value::Text("memo-bravo-longer-value".into())),
        );
        connection::disconnect(&session, connection).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn appends_to_an_encrypted_table_like_the_engine() {
        // Appending a record to an already-encrypted table enciphers it (and its
        // memo) in place, matching the engine byte for byte (append = enctable +
        // one record "delta").
        use crate::setters::SetValue;
        let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/write/encryption");
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-eng-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(fx.join("enctable.adt"), dir.join("t.adt")).unwrap();
        std::fs::copy(fx.join("enctable.adm"), dir.join("t.adm")).unwrap();

        let session = Session::new();
        let connection = connect_to(&session, &dir);
        let mut pw = settings(TableType::Adt, 1, options::EXCLUSIVE);
        pw.password = "secret".to_owned();
        let handle = open(&session, connection, "t.adt", pw).unwrap();
        let table = get(&session, handle).unwrap();
        table.append().unwrap();
        table
            .set_field(FieldRef::Name("D1"), &SetValue::Field("delta".into()))
            .unwrap();
        table
            .set_field(FieldRef::Name("N1"), &SetValue::Field("444".into()))
            .unwrap();
        table
            .set_field(FieldRef::Name("M1"), &SetValue::Field("memo-delta".into()))
            .unwrap();
        table.write_record().unwrap();
        connection::disconnect(&session, connection).unwrap();

        assert_eq!(
            std::fs::read(dir.join("t.adt")).unwrap(),
            std::fs::read(fx.join("append.adt")).unwrap(),
            ".adt"
        );
        assert_eq!(
            std::fs::read(dir.join("t.adm")).unwrap(),
            std::fs::read(fx.join("append.adm")).unwrap(),
            ".adm"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_aes_table_opens_with_the_connection_password() {
        let session = Session::new();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/aes/aes128");
        let connection = connect_to(&session, &dir);
        // Without the password, the encrypted table is refused.
        assert_eq!(
            opening_free_itself(|| open(
                &session,
                connection,
                "s.adt",
                settings(TableType::Adt, 1, 0)
            )),
            Err(Error::TableEncrypted),
        );
        // With it, the table opens: its descriptors and records deciphered
        // (a successful open means the verifier matched and the field
        // descriptors decrypted).
        let mut with_password = settings(TableType::Adt, 1, 0);
        with_password.password = "another pw".to_owned();
        let handle =
            opening_free_itself(|| open(&session, connection, "s.adt", with_password)).unwrap();
        let table = get(&session, handle).unwrap();
        let fields: Vec<&str> = table
            .structure
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        assert_eq!(fields, ["id", "name", "note"]);
        // Its memo value (stored with its own 8-byte IV) deciphers too.
        table.goto_record(1).unwrap();
        assert_eq!(
            table.value(FieldRef::Name("note")),
            Ok(Value::Text("aes-128 memo".into()))
        );
        connection::disconnect(&session, connection).unwrap();
    }

    #[test]
    fn a_free_aes_table_created_through_the_engine_round_trips() {
        use crate::setters::SetValue;
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-eng-free-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let session = Session::new();
        let connection = connection::connect(
            &session,
            &format!(
                "Data Source={};ServerType=LOCAL;EncryptionType=AES128;",
                dir.display()
            ),
        )
        .unwrap();
        let handle = crate::ddl::create_table_api(
            &session,
            connection,
            "f.adt",
            "id,Integer;name,Character,12",
            0,
            settings(TableType::Adt, 1, options::EXCLUSIVE),
        )
        .unwrap();
        let table = get(&session, handle).unwrap();
        table.enable_encryption(b"mypass").unwrap();
        table.encrypt_table().unwrap();
        assert!(table.encryption_enabled());
        table.append().unwrap();
        table
            .set_field(FieldRef::Name("id"), &SetValue::Field("42".into()))
            .unwrap();
        table
            .set_field(FieldRef::Name("name"), &SetValue::Field("forty-two".into()))
            .unwrap();
        table.write_record().unwrap();
        drop(table);
        connection::disconnect(&session, connection).unwrap();

        // Reopen with the password and read the record back.
        let session = Session::new();
        let connection = connect_to(&session, &dir);
        let mut pw = settings(TableType::Adt, 1, 0);
        pw.password = "mypass".to_owned();
        let handle = open(&session, connection, "f.adt", pw).unwrap();
        let table = get(&session, handle).unwrap();
        table.goto_record(1).unwrap();
        assert_eq!(table.value(FieldRef::Name("id")), Ok(Value::Integer(42)));
        assert_eq!(
            table.value(FieldRef::Name("name")),
            Ok(Value::Text("forty-two   ".into()))
        );
        connection::disconnect(&session, connection).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The memo values a free AES table held before `AdsEnableEncryption`
    /// stay in the clear behind a zero IV until `AdsEncryptTable`, and read
    /// as they are in between; values written after arming are enciphered.
    #[test]
    fn a_free_aes_tables_memo_values_read_between_enabling_and_encrypting() {
        use crate::setters::SetValue;
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-eng-aesmemo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let session = Session::new();
        let connection = connection::connect(
            &session,
            &format!(
                "Data Source={};ServerType=LOCAL;EncryptionType=AES128;",
                dir.display()
            ),
        )
        .unwrap();
        let handle = crate::ddl::create_table_api(
            &session,
            connection,
            "m.adt",
            "id,Integer;note,Memo",
            0,
            settings(TableType::Adt, 1, options::EXCLUSIVE),
        )
        .unwrap();
        let table = get(&session, handle).unwrap();
        let append = |id: &str, note: &str| {
            table.append().unwrap();
            table
                .set_field(FieldRef::Name("id"), &SetValue::Field(id.into()))
                .unwrap();
            table
                .set_field(FieldRef::Name("note"), &SetValue::Field(note.into()))
                .unwrap();
            table.write_record().unwrap();
        };
        let notes = |table: &Table, count: u32| -> Vec<Value> {
            (1..=count)
                .map(|record| {
                    table.goto_record(record).unwrap();
                    assert_eq!(
                        table.value(FieldRef::Name("id")),
                        Ok(Value::Integer(i128::from(record)))
                    );
                    table.value(FieldRef::Name("note")).unwrap()
                })
                .collect()
        };
        let text = |s: &str| Value::Text(s.into());
        append("1", "written in the clear");
        table.enable_encryption(b"mypass").unwrap();
        assert_eq!(notes(&table, 1), [text("written in the clear")]);
        append("2", "written once armed");
        assert_eq!(
            notes(&table, 2),
            [text("written in the clear"), text("written once armed")]
        );
        table.encrypt_table().unwrap();
        assert_eq!(
            notes(&table, 2),
            [text("written in the clear"), text("written once armed")]
        );
        drop(table);
        connection::disconnect(&session, connection).unwrap();

        let session = Session::new();
        let connection = connect_to(&session, &dir);
        let mut pw = settings(TableType::Adt, 1, 0);
        pw.password = "mypass".to_owned();
        let handle = open(&session, connection, "m.adt", pw).unwrap();
        let table = get(&session, handle).unwrap();
        assert_eq!(
            notes(&table, 2),
            [text("written in the clear"), text("written once armed")]
        );
        connection::disconnect(&session, connection).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_aes_table_reads_in_key_order() {
        let session = Session::new();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/aes/aes256");
        let connection = connect_to(&session, &dir);
        let mut with_password = settings(TableType::Adt, 1, 0);
        with_password.password = "ddsecret".to_owned();
        let handle =
            opening_free_itself(|| open(&session, connection, "big.adt", with_password)).unwrap();
        // Its AES index (pages [data][8-byte IV]) opens and reads in key order:
        // BIG_ID ascending starts at record 1.
        let orders = crate::index::orders_of(&session, handle).unwrap();
        let order = orders
            .iter()
            .filter_map(|&handle| crate::index::order_of(&session, handle))
            .find(|order| order.tag.name == "BIG_ID")
            .unwrap();
        let table = get(&session, handle).unwrap();
        table.goto_top_in(&order).unwrap();
        assert_eq!(table.record_number(), 1);
        connection::disconnect(&session, connection).unwrap();
    }

    #[test]
    fn a_dict_bound_rc4_table_opens_via_its_dictionary() {
        let session = Session::new();
        let add = fixtures().join("dictionary_encrypted/rc4.add");
        let connection = connection::connect(
            &session,
            &format!(
                "Data Source={};ServerType=LOCAL;User ID=ADSSYS;Password=adssyspw;",
                add.display()
            ),
        )
        .unwrap();
        // The login password releases the dictionary's table password, which
        // the bound encrypted table rc4_secret opens with.
        let mut with_password = settings(TableType::Adt, 1, 0);
        with_password.password = "adssyspw".to_owned();
        let handle = open(&session, connection, "rc4_secret", with_password).unwrap();
        let table = get(&session, handle).unwrap();
        let fields: Vec<&str> = table
            .structure
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        assert_eq!(fields, ["id", "name", "note"]);
        connection::disconnect(&session, connection).unwrap();
    }

    #[test]
    fn an_encrypted_table_reads_in_key_order() {
        let session = Session::new();
        let add = fixtures().join("dictionary_encrypted/rc4.add");
        let connection = connection::connect(
            &session,
            &format!(
                "Data Source={};ServerType=LOCAL;User ID=ADSSYS;Password=adssyspw;",
                add.display()
            ),
        )
        .unwrap();
        let mut with_password = settings(TableType::Adt, 1, 0);
        with_password.password = "adssyspw".to_owned();
        let handle = open(&session, connection, "rc4_secret", with_password).unwrap();
        // The encrypted index opened, and reads in its key order: BY_NAME
        // ascending is "also hidden" (record 2) before "hidden" (record 1).
        let orders = crate::index::orders_of(&session, handle).unwrap();
        let order = crate::index::order_of(&session, orders[0]).unwrap();
        let table = get(&session, handle).unwrap();
        table.goto_top_in(&order).unwrap();
        assert_eq!(table.record_number(), 2);
        table.skip_in(&order, 1).unwrap();
        assert_eq!(table.record_number(), 1);
        connection::disconnect(&session, connection).unwrap();
    }

    #[test]
    fn a_dict_bound_table_reads_encrypted_memos() {
        let session = Session::new();
        let add = fixtures().join("dictionary_encrypted/rc4.add");
        let connection = connection::connect(
            &session,
            &format!(
                "Data Source={};ServerType=LOCAL;User ID=ADSSYS;Password=adssyspw;",
                add.display()
            ),
        )
        .unwrap();
        let mut with_password = settings(TableType::Adt, 1, 0);
        with_password.password = "adssyspw".to_owned();
        let handle = open(&session, connection, "rc4_memos", with_password).unwrap();
        let table = get(&session, handle).unwrap();
        // Each memo value is deciphered at its record's number, a short value
        // and a multi-block extent alike.
        table.goto_record(1).unwrap();
        assert_eq!(
            table.value(FieldRef::Name("m")),
            Ok(Value::Text("first memo".into()))
        );
        assert_eq!(
            table.value(FieldRef::Name("n")),
            Ok(Value::Text("G".repeat(9000)))
        );
        table.goto_record(2).unwrap();
        assert_eq!(
            table.value(FieldRef::Name("m")),
            Ok(Value::Text("third memo".into()))
        );
        connection::disconnect(&session, connection).unwrap();
    }

    /// A scratch folder `name` with copies of the files `files` of
    /// `fixtures/write/encryption`, each as `(from, to)`.
    fn encryption_scratch(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/write/encryption");
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (from, to) in files {
            std::fs::copy(fixture.join(from), dir.join(to)).unwrap();
        }
        dir
    }

    /// As in the original engine (`fixtures/write/encryption/share.json`):
    /// an AES table's password is the handle's. Another handle, of another
    /// session (another client of a server), opened without it reads the
    /// records blank and the memos empty, and writes nothing, though the
    /// first handle deciphers the same open file.
    #[test]
    fn an_aes_tables_password_is_the_handles() {
        use crate::setters::SetValue;
        let dir = encryption_scratch(
            "aes-share",
            &[("share_aes.adt", "t.adt"), ("share_aes.adm", "t.adm")],
        );
        let before = std::fs::read(dir.join("t.adt")).unwrap();
        let (one, two) = (Session::new(), Session::new());
        let (c1, c2) = (connect_to(&one, &dir), connect_to(&two, &dir));
        let mut keyed = settings(TableType::Adt, 1, 0);
        keyed.password = "secret".to_owned();
        let h1 = get(&one, open(&one, c1, "t.adt", keyed).unwrap()).unwrap();
        let h2 = get(
            &two,
            open(&two, c2, "t.adt", settings(TableType::Adt, 1, 0)).unwrap(),
        )
        .unwrap();
        let alpha = Value::Text(format!("alpha{:15}", ""));
        h1.goto_top().unwrap();
        assert!(h1.encryption_enabled());
        assert_eq!(h1.value(FieldRef::Name("D1")), Ok(alpha.clone()));
        assert_eq!(
            h1.value(FieldRef::Name("M1")),
            Ok(Value::Text("memo-alpha".into()))
        );

        h2.goto_top().unwrap();
        assert!(!h2.encryption_enabled());
        assert_eq!(h2.record_count(), Ok(2));
        assert_eq!(
            h2.value(FieldRef::Name("D1")),
            Ok(Value::Text(" ".repeat(20)))
        );
        assert_eq!(h2.value(FieldRef::Name("N1")), Ok(Value::Integer(0)));
        assert_eq!(h2.value(FieldRef::Name("M1")), Ok(Value::Null));
        assert_eq!(h2.record_encrypted(), Ok(false));
        // It may change a record, but not write it, and the change stays
        // until it is cancelled; nor append; a deletion is such a change
        // (`fixtures/aes/api/09_pending`).
        let refused = Error::EncryptionNotEnabled
            .detailed("Cannot update a record in an encrypted table unless encryption is enabled.");
        h2.set_field(FieldRef::Name("D1"), &SetValue::Field("zulu".into()))
            .unwrap();
        assert_eq!(h2.write_record(), Err(refused.clone()));
        assert_eq!(h2.write_record(), Err(refused.clone()));
        assert_eq!(h2.cancel_update(), Ok(()));
        assert_eq!(h2.cancel_update(), Err(Error::NoPendingUpdate));
        assert_eq!(
            h2.append(),
            Err(Error::EncryptionNotEnabled
                .detailed("Cannot append to an encrypted table unless encryption is enabled."))
        );
        h2.goto_top().unwrap();
        assert_eq!(h2.delete_record(), Ok(()));
        assert_eq!(h2.write_record(), Err(refused));
        assert_eq!(h2.cancel_update(), Ok(()));
        assert_eq!(h2.encrypt_record(), Err(Error::EncryptionNotEnabled));
        assert_eq!(h2.decrypt_record(), Err(Error::EncryptionNotEnabled));
        assert_eq!(h1.record_count(), Ok(2));
        h1.goto_top().unwrap();
        assert_eq!(h1.value(FieldRef::Name("D1")), Ok(alpha.clone()));
        // A wrong password is refused; the right one deciphers.
        assert_eq!(h2.enable_encryption(b"wrong"), Err(Error::InvalidPassword));
        assert!(!h2.encryption_enabled());
        h2.enable_encryption(b"secret").unwrap();
        h2.goto_top().unwrap();
        assert_eq!(h2.value(FieldRef::Name("D1")), Ok(alpha));
        connection::disconnect(&one, c1).unwrap();
        connection::disconnect(&two, c2).unwrap();
        assert_eq!(
            std::fs::read(dir.join("t.adt")).unwrap(),
            before,
            "the file is unchanged"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// As in the original engine (`share.json`): on a table only some of
    /// whose records are encrypted (RC4, `AdsEncryptRecord`), a handle
    /// without the password reads an encrypted record enciphered and its
    /// memos empty, and may not change it; the clear ones it reads and writes
    /// in the clear, though another handle deciphers the same open file.
    #[test]
    fn a_handle_without_the_password_writes_in_the_clear() {
        use crate::setters::SetValue;
        let dir = encryption_scratch(
            "rec-share",
            &[("encrec.adt", "t.adt"), ("base.adm", "t.adm")],
        );
        let session = Session::new();
        let (c1, c2) = (connect_to(&session, &dir), connect_to(&session, &dir));
        let h1 = get(
            &session,
            open(&session, c1, "t.adt", settings(TableType::Adt, 1, 0)).unwrap(),
        )
        .unwrap();
        h1.enable_encryption(b"secret").unwrap();
        let h2 = get(
            &session,
            open(&session, c2, "t.adt", settings(TableType::Adt, 1, 0)).unwrap(),
        )
        .unwrap();
        let alpha = Value::Text(format!("alpha{:15}", ""));
        h1.goto_record(1).unwrap();
        assert_eq!(h1.value(FieldRef::Name("D1")), Ok(alpha.clone()));
        h2.goto_record(1).unwrap();
        assert_ne!(h2.value(FieldRef::Name("D1")), Ok(alpha));
        assert_eq!(h2.value(FieldRef::Name("M1")), Ok(Value::Null));
        assert_eq!(h2.record_encrypted(), Ok(true));
        let zulu = SetValue::Field("zulu".into());
        assert_eq!(
            h2.set_field(FieldRef::Name("D1"), &zulu),
            Err(Error::RecordEncrypted.detailed("You must call AdsEnableEncryption first."))
        );
        assert_eq!(
            h2.delete_record(),
            Err(Error::RecordEncrypted.detailed("You must call AdsEnableEncryption first."))
        );
        h2.goto_record(2).unwrap();
        assert_eq!(
            h2.value(FieldRef::Name("D1")),
            Ok(Value::Text(format!("bravo{:15}", "")))
        );
        assert_eq!(h2.record_encrypted(), Ok(false));
        h2.set_field(FieldRef::Name("D1"), &zulu).unwrap();
        h2.write_record().unwrap();
        // Written in the clear, not with the other handle's password.
        h1.goto_record(2).unwrap();
        assert_eq!(h1.record_encrypted(), Ok(false));
        assert_eq!(
            h1.value(FieldRef::Name("D1")),
            Ok(Value::Text(format!("zulu{:16}", "")))
        );
        // A wrong password changes nothing on disk.
        let before = std::fs::read(dir.join("t.adt")).unwrap();
        assert_eq!(h2.enable_encryption(b"wrong"), Err(Error::InvalidPassword));
        assert_eq!(std::fs::read(dir.join("t.adt")).unwrap(), before);
        connection::disconnect(&session, c1).unwrap();
        connection::disconnect(&session, c2).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A table of a dictionary that encrypts its structure, open through the
    /// dictionary, does not open as a free table without the password: not
    /// from the open file the dictionary's handle deciphers. As the original
    /// engine says (`fixtures/write/encryption/freeopen.json`), it is a
    /// dictionary's table (5159) while open, and the password is wrong (7060)
    /// once closed.
    #[test]
    fn a_dictionarys_encrypted_table_open_does_not_open_free() {
        let session = Session::new();
        let folder = fixtures().join("dictionary_encrypted");
        let add = folder.join("rc4.add");
        let dictionary = connection::connect(
            &session,
            &format!(
                "Data Source={};ServerType=LOCAL;User ID=ADSSYS;Password=adssyspw;",
                add.display()
            ),
        )
        .unwrap();
        let mut with_password = settings(TableType::Adt, 1, 0);
        with_password.password = "adssyspw".to_owned();
        let bound = open(&session, dictionary, "rc4_secret", with_password).unwrap();
        assert!(get(&session, bound).unwrap().encryption_enabled());
        let other = Session::new();
        let free = connect_to(&other, &folder);
        let adt = || settings(TableType::Adt, 1, 0);
        let plain = |result: Result<Handle, Error>| result.map_err(|error| error.plain().clone());
        let opened = open(&other, free, "rc4_secret.adt", adt());
        assert_eq!(plain(opened), Err(Error::DatabaseTable));
        // Nor with a password that is not the table's.
        let mut wrong = adt();
        wrong.password = "adssyspw".to_owned();
        assert_eq!(
            plain(open(&other, free, "rc4_secret.adt", wrong)),
            Err(Error::DatabaseTable)
        );
        close(&session, bound).unwrap();
        connection::disconnect(&session, dictionary).unwrap();
        let opened = open(&other, free, "rc4_secret.adt", adt());
        assert_eq!(plain(opened), Err(Error::InvalidPassword));
        connection::disconnect(&other, free).unwrap();
    }

    #[test]
    fn close_only_closes_tables() {
        let session = Session::new();
        let connection = connect_to(&session, &fixtures().join("all_types"));
        assert_eq!(close(&session, connection), Err(Error::InvalidTable));
        assert!(connection::get(&session, connection).is_ok());
        let table = open(
            &session,
            connection,
            "all_types.adt",
            settings(TableType::Adt, 1, 0),
        )
        .unwrap();
        assert_eq!(close(&session, table), Ok(()));
        assert!(connection::get(&session, connection).is_ok());
        connection::disconnect(&session, connection).unwrap();
    }

    #[test]
    fn field_numbers_match_the_engine() {
        let session = Session::new();
        let connection = connect_to(&session, &fixtures().join("indexes"));
        let table = open(
            &session,
            connection,
            "indexed.adt",
            settings(TableType::Adt, 1, 0),
        )
        .unwrap();
        let table = get(&session, table).unwrap();
        // id name ciname amount born active; as the original engine returns them.
        for (long, short) in [
            ("id", "F1"),
            ("CiName", "F3"),
            ("  id  ", "  F1  "),
            ("id>30;born", "F1>30;F5"),
            ("upper(name)+'x'", "upper(F2)+'x'"),
            ("IIF(active, id, 0)", "IIF(F6, F1, 0)"),
            (
                "name = 'id' .AND. \"id\" $ name",
                "F2 = 'id' .AND. \"id\" $ F2",
            ),
            ("'a''id'", "'a''id'"),
            ("[name] + id", "[name] + F1"),
            ("'open id", "'open F1"),
            ("[id", "[F1"),
            ("[a'b] + 'a[b' + id", "[a'b] + 'a[b' + F1"),
            ("name(1) + name (1)", "name(1) + F2 (1)"),
            (
                "name.x + name .x + indexed->id + a.b.id",
                "name.x + F2 .x + indexed->F1 + a.b.F1",
            ),
            (
                "id_x + id2 + 2id + f1 + F2 + nosuch",
                "id_x + id2 + 2id + f1 + F2 + nosuch",
            ),
            ("äid + idä", "äF1 + F1ä"),
            ("", ""),
        ] {
            assert_eq!(table.with_field_numbers(long), short, "{long}");
        }
        for (short, long) in [
            ("F1", "id"),
            ("F01 + F2", "id + name"),
            ("f1 + F + F 1", "f1 + F + F 1"),
            ("F1>30;F5", "id>30;born"),
            ("UPPER(F2)+'x'", "UPPER(name)+'x'"),
            ("F2 = 'F1' + \"F1\"", "name = 'F1' + \"F1\""),
            ("[F1] + F1", "[F1] + id"),
            ("'open F1", "'open id"),
            ("F1(1) + F1.x + a.F1 + x->F1", "id(1) + id.x + a.id + x->id"),
            ("F1F2 + F1_x + xF1 + F1ä", "F1F2 + F1_x + xF1 + idä"),
            ("F1+F2+F3+F4+F5+F6", "id+name+ciname+amount+born+active"),
            ("", ""),
        ] {
            assert_eq!(
                table.with_field_names(short),
                Ok(long.to_owned()),
                "{short}"
            );
        }
        for unknown in ["F0", "F7", "F99", "id + F12"] {
            assert_eq!(
                table.with_field_names(unknown),
                Err(Error::InvalidFieldNumber),
                "{unknown}"
            );
        }
        connection::disconnect(&session, connection).unwrap();
    }
}
