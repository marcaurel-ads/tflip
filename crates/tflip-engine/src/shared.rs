//! Table files shared by every handle on them in the process.
//!
//! Each table handle has its own position and record buffer, but all the
//! handles on one file, in all sessions of the process (the client
//! library's, and every client's of `tflipd`), read and write it through one
//! [`TableFile`], so that what one writes the others see at once: its
//! header (the record slots, the free list, the AutoInc and RowVersion
//! counters) and its records. The record and table locks of the file are
//! kept here too, by the handle that holds them.
//!
//! Tables built in memory (static SQL cursors) are their own, and never
//! shared.
//!
//! A connection may keep a file open after its handles on it closed (see
//! [`crate::cache`]): the file then counts as open, but as kept, which an
//! exclusive open makes let go, and a reopen reuses only while the file on
//! disk is unchanged.
//!
//! Locks are taken on the file too, so that other processes see them: the
//! record and table locks, and whether the file is open exclusively (see
//! `tflip_storage::lock`). The file's open counts once for the process; its
//! record locks are held for the process by whichever handle holds them, and
//! step aside while the process holds the table lock, whose range covers
//! them.

use std::collections::{BTreeMap, HashMap};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

use tflip_storage::adi::IndexFile as AdiFile;
use tflip_storage::cdx::CdxFile;
use tflip_storage::ntx::NtxFile;
use tflip_storage::order::Kind;
use tflip_storage::order::{Error as OrderError, IndexFile as OrderFile};
use tflip_storage::table::{Error as StorageError, Format, TableFile};

use crate::{Error, lock};

/// Who holds a lock: a table handle's number (see [`next_owner`]).
pub(crate) type Owner = u64;

/// A new owner number, for a new table handle.
pub(crate) fn next_owner() -> Owner {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The locks on one file.
#[derive(Debug, Default)]
struct Locks {
    /// The locked records and who holds each.
    records: BTreeMap<u32, Owner>,
    /// Who holds the table lock, if anyone.
    table: Option<Owner>,
}

/// Who has the file open.
#[derive(Debug, Default)]
struct Opens {
    /// How many table handles.
    count: usize,
    /// Whether one of them opened it exclusively.
    exclusive: bool,
    /// How many connections keep it open for reuse, with no handle on it
    /// (see [`crate::cache`]).
    cached: usize,
    /// The file as it was on disk when its last handle closed while
    /// connections kept it open: a reopen reuses the open file only if the
    /// file is still so (see [`Shared::stale`]).
    stamp: Option<Stamp>,
}

/// What a table file is like on disk: which file it is, its length and
/// when it was last written. Another program that replaces or writes a file
/// that only connections' caches keep open changes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    id: (u64, u64),
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl Stamp {
    /// The file at `path` as it is now; `None` if it cannot be read.
    fn of(path: &Path) -> Option<Stamp> {
        let metadata = std::fs::metadata(path).ok()?;
        #[cfg(unix)]
        let id = {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        };
        #[cfg(windows)]
        let id = {
            use std::os::windows::fs::MetadataExt;
            (metadata.creation_time(), 0)
        };
        #[cfg(not(any(unix, windows)))]
        let id = (0, 0);
        Some(Stamp {
            id,
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

/// A record a transaction not ended yet wrote (see
/// [`crate::transaction`]): the file has it marked, or as a blank record
/// if the transaction appended it, and holds what it was; the transaction
/// reads it as it has it.
#[derive(Debug, Clone)]
pub(crate) struct TxRecord {
    /// The transaction's number.
    pub(crate) tx: u64,
    /// The record before the transaction; `None` for one it appended.
    pub(crate) original: Option<Vec<u8>>,
    /// The record as the transaction has it.
    pub(crate) image: Vec<u8>,
    /// Whether the transaction deleted it (an ADT record; a DBF record's
    /// deletion is in its image).
    pub(crate) deleted: bool,
    /// The orders its keys are kept in: those of the handle that wrote it.
    pub(crate) orders: Vec<Arc<crate::index::Index>>,
    /// The handle that holds its lock until the transaction ends.
    pub(crate) owner: Owner,
    /// A DBF record's memo values the transaction set, by field offset,
    /// which the commit writes (an appended record's are written at once).
    pub(crate) memos: Vec<(u16, Option<Vec<u8>>)>,
    /// Whether it is a record the transaction appended and has not written
    /// yet: the orders have no keys of it.
    pub(crate) unwritten: bool,
    /// Whether a referential integrity rule wrote it (a cascade), as the
    /// original's rules hold their own locks (see [`Shared::lock_record`]).
    pub(crate) by_ri: bool,
    /// Whether a handle that has not given the table's password wrote it:
    /// it is written as it is, not enciphered (see [`Sealing`]).
    pub(crate) sealed: bool,
}

/// A record's state in a transaction before a write of it, which a rollback
/// to a savepoint goes back to (see [`TxRecord`]).
#[derive(Debug, Clone)]
pub(crate) struct Earlier {
    pub(crate) image: Vec<u8>,
    pub(crate) deleted: bool,
    pub(crate) unwritten: bool,
    pub(crate) memos: Vec<(u16, Option<Vec<u8>>)>,
}

impl Earlier {
    /// The state `record` is in.
    pub(crate) fn of(record: &TxRecord) -> Earlier {
        Earlier {
            image: record.image.clone(),
            deleted: record.deleted,
            unwritten: record.unwritten,
            memos: record.memos.clone(),
        }
    }
}

/// What a reader sees of a record a transaction wrote (see
/// [`Shared::seen`]).
pub(crate) enum Seen {
    /// The transaction's own record, and whether it deleted it.
    Mine(Vec<u8>, bool),
    /// Another transaction's: the record as it was.
    Before(Vec<u8>),
    /// A record another transaction appended, not there yet.
    Absent,
}

/// An open table file and its locks.
#[derive(Debug)]
pub(crate) struct Shared {
    /// The file's path, for a file of the process's ([`open`]); `None` for
    /// a table built in memory.
    path: Option<PathBuf>,
    file: Mutex<TableFile>,
    locks: Mutex<Locks>,
    opens: Mutex<Opens>,
    /// The record each handle stands on, by handle, and whether the record
    /// image it read is still the file's: a DBF memo value written while
    /// another handle holds its record's image goes to a new block, once
    /// (`fixtures/write/memo_other_handle`).
    standing: Mutex<Vec<(Owner, u32, bool)>>,
    /// The records of transactions not ended yet, by slot.
    transactions: Mutex<BTreeMap<u32, TxRecord>>,
}

impl Shared {
    /// A file of its own, not shared with anyone (a table built in memory).
    pub(crate) fn private(file: TableFile) -> Arc<Shared> {
        Arc::new(Shared {
            path: None,
            file: Mutex::new(file),
            locks: Mutex::default(),
            opens: Mutex::default(),
            standing: Mutex::default(),
            transactions: Mutex::default(),
        })
    }

    /// Counts a table handle as having the file open, exclusively if
    /// `exclusive`. [`Error::FileInUse`] if another handle has it open
    /// exclusively, or if it is to be opened exclusively while open (or
    /// while a transaction holds records of it).
    ///
    /// Connections that keep the file open for reuse with no handle on it
    /// ([`crate::cache`]) let go of it for an exclusive open, as in the
    /// original engine.
    pub(crate) fn attach(&self, exclusive: bool) -> Result<(), Error> {
        if exclusive && self.only_cached() {
            crate::cache::evict_file(self);
        }
        let mut opens = lock(&self.opens);
        let held = !self.transactions().is_empty();
        let others = opens.count + opens.cached;
        if opens.exclusive || exclusive && (others > 0 || held) {
            return Err(Error::FileInUse);
        }
        if others == 0 && !self.file().lock_open(exclusive).map_err(file_error)? {
            return Err(Error::FileInUse);
        }
        opens.count += 1;
        opens.exclusive = exclusive;
        opens.stamp = None;
        Ok(())
    }

    /// Notes that handle `owner` stands on record `record` (0 for none).
    pub(crate) fn stand(&self, owner: Owner, record: u32) {
        let mut standing = lock(&self.standing);
        standing.retain(|(handle, ..)| *handle != owner);
        if record != 0 {
            standing.push((owner, record, true));
        }
    }

    /// Whether a handle other than `owner` holds the current image of record
    /// `record`; what it holds is out of date once `owner` writes the record.
    pub(crate) fn outdate_others(&self, owner: Owner, record: u32) -> bool {
        let mut moved = false;
        for (handle, at, current) in lock(&self.standing).iter_mut() {
            if *handle != owner && *at == record && *current {
                *current = false;
                moved = true;
            }
        }
        moved
    }

    /// Whether the file is open exclusively.
    pub(crate) fn opened_exclusively(&self) -> bool {
        lock(&self.opens).exclusive
    }

    /// How many table handles have the file open.
    pub(crate) fn handles(&self) -> usize {
        lock(&self.opens).count
    }

    /// Whether only connections' caches keep the file open: no handle has
    /// it open.
    fn only_cached(&self) -> bool {
        let opens = lock(&self.opens);
        opens.count == 0 && opens.cached > 0
    }

    /// Counts a connection as keeping the file open for reuse
    /// ([`crate::cache`]), while a handle on it is still open.
    pub(crate) fn park(&self) {
        lock(&self.opens).cached += 1;
    }

    /// Counts a connection as no longer keeping the file open; with no
    /// handle on it either, it closes (see [`Shared::settle`]).
    pub(crate) fn unpark(&self) {
        let mut opens = lock(&self.opens);
        opens.cached = opens.cached.saturating_sub(1);
        if opens.count == 0 && opens.cached == 0 {
            opens.stamp = None;
            drop(opens);
            self.settle();
        }
    }

    /// Whether the file is open for connections' caches only and is not
    /// what it was when its last handle closed: another program changed or
    /// replaced it, so a reopen must not reuse what was read of it.
    fn stale(&self) -> bool {
        let opens = lock(&self.opens);
        if opens.count > 0 || opens.cached == 0 || !self.transactions().is_empty() {
            return false;
        }
        opens.stamp.is_none() || opens.stamp != self.path.as_deref().and_then(Stamp::of)
    }

    /// The file's path, for a file of the process's.
    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Counts a table handle as having closed the file.
    pub(crate) fn detach(&self) {
        let mut opens = lock(&self.opens);
        opens.count = opens.count.saturating_sub(1);
        if opens.count == 0 {
            opens.exclusive = false;
            drop(opens);
            self.settle();
        }
    }

    /// Once no handle has the file open and no transaction holds a record
    /// of it (a table closed in a transaction stays open until it ends):
    /// its end-of-file byte written, and its open lock released unless a
    /// connection keeps the file open for reuse ([`crate::cache`]), which
    /// then notes what the file is like (see [`Shared::stale`]).
    pub(crate) fn settle(&self) {
        let mut opens = lock(&self.opens);
        if opens.count > 0 || !self.transactions().is_empty() {
            return;
        }
        let mut file = self.file();
        let _ = file.finish_end_of_file();
        if opens.cached == 0 {
            let _ = file.unlock_open();
        } else {
            drop(file);
            opens.stamp = self.path.as_deref().and_then(Stamp::of);
        }
    }

    /// The file, for one read or write: sealed ([`TableFile::seal`]) while
    /// the thread works for a table handle that has not given the table's
    /// password (see [`Sealing`]).
    pub(crate) fn file(&self) -> FileGuard<'_> {
        let mut guard = lock(&self.file);
        let sealed = sealed();
        if sealed {
            guard.seal();
        }
        FileGuard { guard, sealed }
    }

    /// The records of transactions not ended yet.
    pub(crate) fn transactions(&self) -> MutexGuard<'_, BTreeMap<u32, TxRecord>> {
        lock(&self.transactions)
    }

    /// What the transaction `reader` (none outside one) sees of slot `slot`,
    /// if a transaction wrote it; `None` if none did.
    pub(crate) fn seen(&self, slot: u32, reader: Option<u64>) -> Option<Seen> {
        let records = self.transactions();
        let record = records.get(&slot)?;
        Some(match (Some(record.tx) == reader, &record.original) {
            (true, _) => Seen::Mine(record.image.clone(), record.deleted),
            (false, Some(original)) => Seen::Before(original.clone()),
            (false, None) => Seen::Absent,
        })
    }

    /// Locks record `record` for `owner`, which is in the transaction `tx`
    /// if it is in one; a lock it holds already is kept, and so is the lock
    /// another handle of the same transaction holds on a record the
    /// transaction wrote (the transaction holds it, not the handle) when an
    /// SQL statement locks it, but not when a client's handle does
    /// (`fixtures/write/tx_handles`); and between a referential integrity
    /// rule and the statements it runs for: the original's rules cannot lock
    /// what the transaction wrote before, nor the statements what a rule did
    /// (`fixtures/sql/txtrig.json`).
    /// [`Error::LockFailed`] if another handle holds that record or the
    /// table.
    pub(crate) fn lock_record(
        &self,
        owner: Owner,
        record: u32,
        tx: Option<u64>,
    ) -> Result<(), Error> {
        let statement = crate::statement::running_name().is_some();
        let theirs = statement
            && tx.is_some_and(|tx| {
                self.transactions()
                    .get(&record)
                    .is_some_and(|held| held.tx == tx && held.by_ri == crate::ri::active())
            });
        let mut locks = lock(&self.locks);
        if locks.table.is_some_and(|holder| holder != owner) {
            return Err(Error::LockFailed);
        }
        match locks.records.get(&record) {
            Some(&holder) if holder != owner && !theirs => Err(Error::LockFailed),
            Some(_) => Ok(()),
            None => {
                // Under the process's table lock the record is locked for it.
                let file = self.file();
                if locks.table.is_none() && !file.lock_record(record).map_err(file_error)? {
                    return Err(Error::LockFailed);
                }
                locks.records.insert(record, owner);
                Ok(())
            }
        }
    }

    /// Releases `owner`'s lock on record `record`; whether it held one.
    pub(crate) fn unlock_record(&self, owner: Owner, record: u32) -> bool {
        let mut locks = lock(&self.locks);
        match locks.records.get(&record) {
            Some(&holder) if holder == owner => {
                locks.records.remove(&record);
                if locks.table.is_none() {
                    let _ = self.file().unlock_record(record);
                }
                true
            }
            _ => false,
        }
    }

    /// Locks the table for `owner`. [`Error::LockFailed`] if another handle
    /// holds the table or one of its records.
    pub(crate) fn lock_table(&self, owner: Owner) -> Result<(), Error> {
        let mut locks = lock(&self.locks);
        let other_records = locks.records.values().any(|&holder| holder != owner);
        if locks.table.is_some_and(|holder| holder != owner) || other_records {
            return Err(Error::LockFailed);
        }
        if locks.table.is_none() {
            // The record locks step aside for the table's range, and come
            // back if it is not granted.
            let file = self.file();
            for &record in locks.records.keys() {
                file.unlock_record(record).map_err(file_error)?;
            }
            if !file.lock_table().map_err(file_error)? {
                for &record in locks.records.keys() {
                    let _ = file.lock_record(record);
                }
                return Err(Error::LockFailed);
            }
        }
        locks.table = Some(owner);
        Ok(())
    }

    /// Releases `owner`'s table lock; whether it held it.
    pub(crate) fn unlock_table(&self, owner: Owner) -> bool {
        let mut locks = lock(&self.locks);
        match locks.table == Some(owner) {
            true => {
                locks.table = None;
                self.unlock_table_file(&locks);
                true
            }
            false => false,
        }
    }

    /// Releases the process's table lock on the file, taking back the record
    /// locks it covered.
    fn unlock_table_file(&self, locks: &Locks) {
        let file = self.file();
        let _ = file.unlock_table();
        for &record in locks.records.keys() {
            let _ = file.lock_record(record);
        }
    }

    /// Whether `owner` holds a lock on record `record`.
    pub(crate) fn holds_record(&self, owner: Owner, record: u32) -> bool {
        lock(&self.locks).records.get(&record) == Some(&owner)
    }

    /// Whether `owner` holds the table lock.
    pub(crate) fn holds_table(&self, owner: Owner) -> bool {
        lock(&self.locks).table == Some(owner)
    }

    /// The records `owner` holds locks on, in record order.
    pub(crate) fn records_locked_by(&self, owner: Owner) -> Vec<u32> {
        let locks = lock(&self.locks);
        locks
            .records
            .iter()
            .filter(|(_, holder)| **holder == owner)
            .map(|(r, _)| *r)
            .collect()
    }

    /// Releases every handle's lock on record `record` (it was deleted).
    pub(crate) fn release_record(&self, record: u32) {
        let mut locks = lock(&self.locks);
        if locks.records.remove(&record).is_some() && locks.table.is_none() {
            let _ = self.file().unlock_record(record);
        }
    }

    /// Releases every lock `owner` holds (its handle closes), but those of
    /// records a transaction holds, which stay until it ends.
    pub(crate) fn release(&self, owner: Owner) {
        let held: Vec<u32> = self
            .transactions()
            .iter()
            .filter(|(_, record)| record.owner == owner)
            .map(|(slot, _)| *slot)
            .collect();
        let mut locks = lock(&self.locks);
        let kept = |record: &u32, holder: &Owner| *holder != owner || held.contains(record);
        let released: Vec<u32> = locks
            .records
            .iter()
            .filter(|(r, h)| !kept(r, h))
            .map(|(r, _)| *r)
            .collect();
        locks.records.retain(|record, holder| kept(record, holder));
        if locks.table == Some(owner) {
            locks.table = None;
            self.unlock_table_file(&locks);
        } else if locks.table.is_none() {
            let file = self.file();
            for record in released {
                let _ = file.unlock_record(record);
            }
        }
    }
}

/// A table file taken for one read or write ([`Shared::file`]).
pub(crate) struct FileGuard<'f> {
    guard: MutexGuard<'f, TableFile>,
    /// Whether it is sealed, to unseal when it is let go.
    sealed: bool,
}

impl FileGuard<'_> {
    /// Seals the file until it is let go, for a record a handle without the
    /// table's password wrote (see [`TxRecord::sealed`]).
    pub(crate) fn seal(&mut self) {
        if !self.sealed {
            self.guard.seal();
            self.sealed = true;
        }
    }
}

impl std::ops::Deref for FileGuard<'_> {
    type Target = TableFile;

    fn deref(&self) -> &TableFile {
        &self.guard
    }
}

impl std::ops::DerefMut for FileGuard<'_> {
    fn deref_mut(&mut self) -> &mut TableFile {
        &mut self.guard
    }
}

impl Drop for FileGuard<'_> {
    fn drop(&mut self) {
        if self.sealed {
            self.guard.unseal();
        }
    }
}

thread_local! {
    /// The table handles the thread works for, innermost last, and whether
    /// each has not given its table's password (see [`Sealing`]).
    static SEALINGS: std::cell::RefCell<Vec<(u64, bool)>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

/// While it lives, the thread works for a table handle, which has given its
/// table's encryption password or not. In the original engine the password
/// is the handle's, not the file's: while the thread works for a handle that
/// has not given it, the files it reads and writes are sealed (see
/// [`TableFile::seal`]), whatever other handles on them gave. A table's state
/// guard holds one (`crate::table::StateGuard`).
pub(crate) struct Sealing(u64);

impl Sealing {
    /// The thread works for a handle that has given its table's password, or
    /// not (`sealed`), until the sealing is dropped.
    pub(crate) fn new(sealed: bool) -> Sealing {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        SEALINGS.with(|sealings| sealings.borrow_mut().push((id, sealed)));
        Sealing(id)
    }
}

impl Drop for Sealing {
    fn drop(&mut self) {
        // Guards may be let go in any order: each takes out its own.
        SEALINGS.with(|sealings| {
            let mut sealings = sealings.borrow_mut();
            if let Some(at) = sealings.iter().rposition(|(id, _)| *id == self.0) {
                sealings.remove(at);
            }
        });
    }
}

/// Whether the thread works for a table handle that has not given its
/// table's password (see [`Sealing`]).
pub(crate) fn sealed() -> bool {
    SEALINGS.with(|sealings| sealings.borrow().last().is_some_and(|(_, sealed)| *sealed))
}

/// What a failure to lock a file is for the client.
fn file_error(_: StorageError) -> Error {
    Error::LockFailed
}

/// The table files open in the process, by path.
fn files() -> MutexGuard<'static, HashMap<PathBuf, Weak<Shared>>> {
    static FILES: OnceLock<Mutex<HashMap<PathBuf, Weak<Shared>>>> = OnceLock::new();
    lock(FILES.get_or_init(Mutex::default))
}

/// Whether a table handle in the process, or a statement holding it (see
/// [`crate::cache::Hold`]), has the table file at `path` open; not one only
/// connections' caches keep open.
pub(crate) fn in_use(path: &Path) -> bool {
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let shared = files().get(&key).and_then(Weak::upgrade);
    shared.is_some_and(|shared| lock(&shared.opens).count > 0)
}

/// The records of the table file at `path` that transactions not ended yet
/// have written, as they were before them, by slot (`None` for one a
/// transaction appended): what makes the committed table of a file that
/// holds their changes already.
pub(crate) fn committed_view(path: &Path) -> BTreeMap<u32, Option<Vec<u8>>> {
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let shared = files().get(&key).and_then(Weak::upgrade);
    shared.map_or_else(BTreeMap::new, |shared| {
        let transactions = shared.transactions();
        transactions
            .iter()
            .map(|(slot, record)| (*slot, record.original.clone()))
            .collect()
    })
}

/// Whether a table handle in the process has the table file at `path` open
/// exclusively.
pub(crate) fn held_exclusively(path: &Path) -> bool {
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let shared = files().get(&key).and_then(Weak::upgrade);
    shared.is_some_and(|shared| shared.opened_exclusively() && lock(&shared.opens).count > 0)
}

/// Whether a transaction not ended yet holds records of the table file at
/// `path` open in the process.
pub(crate) fn transactions_held(path: &Path) -> bool {
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let shared = files().get(&key).and_then(Weak::upgrade);
    shared.is_some_and(|shared| !shared.transactions().is_empty())
}

/// How many table handles and statements in the process have the table file
/// at `path` open (see [`in_use`]).
pub(crate) fn open_count(path: &Path) -> usize {
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let shared = files().get(&key).and_then(Weak::upgrade);
    shared.map_or(0, |shared| lock(&shared.opens).count)
}

/// The file of the table of format `format` at `path`, as open in the
/// process, or opened now: for writing if the file system allows it, else
/// to read.
pub(crate) fn open(
    path: &Path,
    format: Format,
    password: Option<&[u8]>,
) -> Result<Arc<Shared>, StorageError> {
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let mut files = files();
    if let Some(shared) = files.get(&key).and_then(Weak::upgrade) {
        // Reuse the open file only when it is of the kind asked for: a DBF file
        // opened as any of its flavours (CDX/NTX/VFP) shares one open, but an
        // ADT file opened as a DBF (or the reverse) is a different, failing
        // open, not a share of this one.
        let cached = shared.file().structure.header.format;
        if std::mem::discriminant(&cached) == std::mem::discriminant(&format) {
            // A file only connections' caches keep open is reused as it was
            // opened, and while unchanged since; else they let go of it and
            // it opens afresh, checked as any open is.
            let other_flavour = cached != format && shared.only_cached();
            if !other_flavour && !shared.stale() {
                return Ok(shared);
            }
            drop(files);
            crate::cache::evict_file(&shared);
            files = self::files();
        }
    }
    let file = match TableFile::open_deciphering(path, format, true, password) {
        Err(StorageError::Io(error)) if error.kind() == ErrorKind::PermissionDenied => {
            TableFile::open_deciphering(path, format, false, password)?
        }
        result => result?,
    };
    let shared = Arc::new(Shared {
        path: Some(key.clone()),
        file: Mutex::new(file),
        locks: Mutex::default(),
        opens: Mutex::default(),
        standing: Mutex::default(),
        transactions: Mutex::default(),
    });
    files.retain(|_, weak| weak.strong_count() > 0);
    files.insert(key, Arc::downgrade(&shared));
    Ok(shared)
}

/// An ADI index file shared by every order handle on it in the process,
/// which writes change; the number of changes, for cursors to see that the
/// pages they read may have changed.
#[derive(Debug)]
pub(crate) struct SharedIndex {
    file: Mutex<OrderFile>,
    changes: AtomicU64,
}

impl SharedIndex {
    /// The file, for one read or change.
    pub(crate) fn file(&self) -> MutexGuard<'_, OrderFile> {
        lock(&self.file)
    }

    /// How many times the file was changed.
    pub(crate) fn changes(&self) -> u64 {
        self.changes.load(Ordering::Acquire)
    }

    /// Counts a change of the file.
    pub(crate) fn changed(&self) {
        self.changes.fetch_add(1, Ordering::AcqRel);
    }
}

/// Whether opening a file failed for want of the right to write it.
fn denied<T>(result: &Result<T, OrderError>) -> bool {
    matches!(result, Err(OrderError::Io(e)) if e.kind() == ErrorKind::PermissionDenied)
}

/// The index files open in the process, by path.
fn index_files() -> MutexGuard<'static, HashMap<PathBuf, Weak<SharedIndex>>> {
    static FILES: OnceLock<Mutex<HashMap<PathBuf, Weak<SharedIndex>>>> = OnceLock::new();
    lock(FILES.get_or_init(Mutex::default))
}

/// The index file at `path`, as open in the process, or opened
/// now: for writing if the file system allows it, else to read.
/// Reads the ADI file `path` again where the process has it open, after it
/// was written anew (see [`tflip_storage::adi::IndexFile::reopened`]).
pub(crate) fn reopen_index(path: &Path) -> Result<(), OrderError> {
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let shared = index_files().get(&key).and_then(Weak::upgrade);
    if let Some(shared) = shared
        && let OrderFile::Adi(file) = &mut *shared.file()
    {
        file.reopened(path)?;
    }
    Ok(())
}

/// The shared file of the index at `path`; of an AES table's, with the
/// table's `cipher` and the IV counter its pages draw from.
pub(crate) fn open_index(
    path: &Path,
    cipher: Option<(&tflip_storage::table::RecordCipher, Arc<AtomicU64>)>,
) -> Result<Arc<SharedIndex>, OrderError> {
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let mut files = index_files();
    if let Some(shared) = files.get(&key).and_then(Weak::upgrade) {
        return Ok(shared);
    }
    let file = match Kind::of(path) {
        Kind::Adi => match cipher {
            Some((cipher, counter)) => {
                let file = AdiFile::open_encrypted_for_writing(path, cipher);
                let mut file = match denied(&file) {
                    true => AdiFile::open_encrypted(path, cipher)?,
                    false => file?,
                };
                file.set_aes_counter(counter);
                OrderFile::Adi(file)
            }
            None => {
                let file = AdiFile::open_for_writing(path);
                OrderFile::Adi(if denied(&file) {
                    AdiFile::open(path)?
                } else {
                    file?
                })
            }
        },
        Kind::Ntx => {
            let file = NtxFile::open_for_writing(path);
            OrderFile::Ntx(if denied(&file) {
                NtxFile::open(path)?
            } else {
                file?
            })
        }
        Kind::Cdx => {
            let file = CdxFile::open_for_writing(path);
            OrderFile::Cdx(if denied(&file) {
                CdxFile::open(path)?
            } else {
                file?
            })
        }
    };
    let shared = Arc::new(SharedIndex {
        file: Mutex::new(file),
        changes: AtomicU64::new(0),
    });
    files.retain(|_, weak| weak.strong_count() > 0);
    files.insert(key, Arc::downgrade(&shared));
    Ok(shared)
}
