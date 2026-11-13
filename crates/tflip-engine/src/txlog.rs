//! The transactions' logs on disk (`docs/TRANSACTIONS.md`), tflip's own
//! format: what a transaction wrote, record by record, so that one a crash
//! interrupted is finished or taken back afterwards, as the original's
//! server does when it starts. A transaction that reached its commit is
//! committed, any other rolled back.
//!
//! A transaction's log is a file of its own in the log folder, made when it
//! first writes and deleted when it ends. Each entry goes to the disk
//! (synced) before the table files are changed for it: the record's state
//! (its bytes before the transaction, its bytes as the transaction has
//! them, whether it deleted it, a DBF record's memo values that wait for
//! the commit), a rollback to a savepoint, and the commit. The transaction
//! holds a lock on its log while it runs; a log nobody holds is a crashed
//! transaction's.
//!
//! Recovery runs when a table is opened, and for
//! `AdsFailedTransactionRecovery`: each table of a crashed transaction is
//! opened exclusively, its records that are still marked (see
//! `tflip_storage::table::TableFile::marked`) committed or rolled back as a
//! running transaction's would be, and its index files rebuilt, since the
//! crash may have left their keys half moved. A table another handle has
//! open waits for the next recovery.
//!
//! File: `TFLIPTX1`, then entries, each its length (u32), the CRC-32 of its
//! bytes (u32) and its bytes, all little-endian; an entry cut short or
//! damaged, and anything after it, was never synced and is ignored. The
//! entries (a byte for the kind, then its fields):
//!
//! - 1, a table: its number (u32), type (u8: ADT 0, NTX 1, CDX 2, VFP 3), character
//!   type (u16), collation, path and its index files (a u16 count, then the
//!   paths); again with more index files when another handle writes it;
//! - 2, a record's state: the table's number (u32), the slot (u32), whether
//!   deleted (u8), its bytes before (u8 0, or 1 and the bytes), its bytes
//!   now, and the memo values that wait (a u16 count, then each field's
//!   offset (u16) and value, as the bytes before);
//! - 3, a rollback to a savepoint: how many record states stay (u32);
//! - 4, the commit.
//!
//! Text and byte strings are a u32 length and the bytes (text in UTF-8).

use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::shared::{Shared, TxRecord};
use crate::table::{self, TableSettings, TableType};
use crate::{Error, Session, index, lock};

const MAGIC: &[u8; 8] = b"TFLIPTX1";

const TABLE: u8 = 1;
const RECORD: u8 = 2;
const ROLLBACK_TO: u8 = 3;
const COMMIT: u8 = 4;

/// The folder set for the logs (see [`folder`]).
static FOLDER: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Puts the logs in `folder` from now on (`tflip.ini`'s `[transactions]
/// log`, `tflipd`'s state folder).
pub fn set_folder(folder: PathBuf) {
    *lock(&FOLDER) = Some(folder);
}

/// The folder of the logs: the one set, else `TFLIP_TX_LOG`, else
/// `tflip/log` in the user's state folder (`$XDG_STATE_HOME`,
/// `~/.local/state`, or `%LOCALAPPDATA%` on Windows), else in the temporary
/// folder.
pub fn folder() -> PathBuf {
    if let Some(folder) = lock(&FOLDER).clone() {
        return folder;
    }
    if let Some(folder) = std::env::var_os("TFLIP_TX_LOG") {
        return PathBuf::from(folder);
    }
    let state = match cfg!(windows) {
        true => std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        false => std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            }),
    };
    state
        .unwrap_or_else(std::env::temp_dir)
        .join("tflip")
        .join("log")
}

/// What a table a transaction wrote is reopened with to recover it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Source {
    pub(crate) path: PathBuf,
    pub(crate) table_type: TableType,
    pub(crate) char_type: u16,
    pub(crate) collation: String,
}

/// The number of a table type in the log.
fn type_code(table_type: TableType) -> u8 {
    match table_type {
        TableType::Adt | TableType::Database => 0,
        TableType::Ntx => 1,
        TableType::Cdx => 2,
        TableType::Vfp => 3,
    }
}

fn type_of(code: u8) -> TableType {
    match code {
        1 => TableType::Ntx,
        2 => TableType::Cdx,
        3 => TableType::Vfp,
        _ => TableType::Adt,
    }
}

/// The logs of this process's running transactions, which a recovery in
/// the process leaves alone whatever the file locks say (on Unix but Linux
/// they are the process's, which another open of the file would drop).
static LIVE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// A running transaction's log.
#[derive(Debug)]
pub(crate) struct Log {
    file: File,
    path: PathBuf,
    /// The tables logged, by number: the file (by address, which stays
    /// while the transaction holds it) and the index files logged for it.
    tables: Vec<(usize, Vec<PathBuf>)>,
}

impl Drop for Log {
    /// The log is no longer this process's running transaction's: removed,
    /// or left for recovery.
    fn drop(&mut self) {
        lock(&LIVE).retain(|path| *path != self.path);
    }
}

impl Log {
    /// A new log in the log folder, locked.
    pub(crate) fn create() -> io::Result<Log> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let folder = folder();
        fs::create_dir_all(&folder)?;
        // A recovery that looks at the new file before it is locked locks it
        // itself (and removes it, as no log): then another is made.
        let (path, mut file) = loop {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = folder.join(format!("tx-{}-{n}.log", std::process::id()));
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)?;
            if tflip_storage::try_lock_exclusive(&file)? {
                break (path, file);
            }
        };
        lock(&LIVE).push(path.clone());
        file.write_all(MAGIC)?;
        file.sync_data()?;
        sync_folder(&folder);
        Ok(Log {
            file,
            path,
            tables: Vec::new(),
        })
    }

    /// Logs the state `record` of slot `slot` of the table file `shared`
    /// (opened as `source`).
    pub(crate) fn record(
        &mut self,
        shared: &Arc<Shared>,
        source: &Source,
        slot: u32,
        record: &TxRecord,
    ) -> io::Result<()> {
        let address = Arc::as_ptr(shared) as usize;
        let indexes: Vec<PathBuf> = {
            let mut paths: Vec<PathBuf> = Vec::new();
            for order in &record.orders {
                if !paths.contains(&order.path) {
                    paths.push(order.path.clone());
                }
            }
            paths
        };
        let at = self.tables.iter().position(|(a, _)| *a == address);
        let number = match at {
            Some(at) if indexes.iter().all(|path| self.tables[at].1.contains(path)) => at,
            _ => {
                let number = at.unwrap_or(self.tables.len());
                if at.is_none() {
                    self.tables.push((address, Vec::new()));
                }
                let known = &mut self.tables[number].1;
                for path in indexes {
                    if !known.contains(&path) {
                        known.push(path);
                    }
                }
                let mut entry = vec![TABLE];
                put_u32(&mut entry, number as u32);
                entry.push(type_code(source.table_type));
                entry.extend_from_slice(&source.char_type.to_le_bytes());
                put_text(&mut entry, &source.collation);
                put_text(&mut entry, &source.path.to_string_lossy());
                entry.extend_from_slice(&(known.len() as u16).to_le_bytes());
                for path in known.iter() {
                    put_text(&mut entry, &path.to_string_lossy());
                }
                self.append_frame(&entry, false)?;
                number
            }
        };
        let mut entry = vec![RECORD];
        put_u32(&mut entry, number as u32);
        put_u32(&mut entry, slot);
        entry.push(u8::from(record.deleted));
        put_optional(&mut entry, record.original.as_deref());
        put_bytes(&mut entry, &record.image);
        entry.extend_from_slice(&(record.memos.len() as u16).to_le_bytes());
        for (offset, value) in &record.memos {
            entry.extend_from_slice(&offset.to_le_bytes());
            put_optional(&mut entry, value.as_deref());
        }
        self.append_frame(&entry, true)
    }

    /// Logs a rollback to a savepoint that keeps the first `kept` record
    /// states.
    pub(crate) fn rolled_back_to(&mut self, kept: usize) -> io::Result<()> {
        let mut entry = vec![ROLLBACK_TO];
        put_u32(&mut entry, kept as u32);
        self.append_frame(&entry, true)
    }

    /// Logs the commit.
    pub(crate) fn commit(&mut self) -> io::Result<()> {
        self.append_frame(&[COMMIT], true)
    }

    /// Ends the log: the transaction is over.
    pub(crate) fn remove(self) {
        // Emptied while still locked: a recovery in another process that
        // takes the lock between the close and the removal finds no
        // transaction, rather than reopen its tables to redo it.
        let _ = self.file.set_len(0);
        // Closed first: Windows deletes no open file.
        let path = self.path.clone();
        drop(self);
        let _ = fs::remove_file(&path);
    }

    fn append_frame(&mut self, entry: &[u8], sync: bool) -> io::Result<()> {
        let mut frame = Vec::with_capacity(entry.len() + 8);
        put_u32(&mut frame, entry.len() as u32);
        put_u32(&mut frame, crc32(entry));
        frame.extend_from_slice(entry);
        self.file.write_all(&frame)?;
        if sync {
            self.file.sync_data()?;
        }
        Ok(())
    }
}

/// Makes a new file's name in `folder` last (on Unix; Windows needs no
/// more).
fn sync_folder(folder: &Path) {
    #[cfg(unix)]
    if let Ok(folder) = File::open(folder) {
        let _ = folder.sync_all();
    }
    #[cfg(not(unix))]
    let _ = folder;
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

fn put_text(out: &mut Vec<u8>, text: &str) {
    put_bytes(out, text.as_bytes());
}

fn put_optional(out: &mut Vec<u8>, bytes: Option<&[u8]>) {
    match bytes {
        Some(bytes) => {
            out.push(1);
            put_bytes(out, bytes);
        }
        None => out.push(0),
    }
}

/// The CRC-32 (IEEE) of `bytes`.
fn crc32(bytes: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *entry = c;
        }
        table
    });
    !bytes.iter().fold(!0u32, |c, &b| {
        table[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8)
    })
}

/// Reads an entry's fields.
struct Reader<'b> {
    bytes: &'b [u8],
}

impl<'b> Reader<'b> {
    fn take(&mut self, n: usize) -> Option<&'b [u8]> {
        let (head, rest) = self.bytes.split_at_checked(n)?;
        self.bytes = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = self.u32()? as usize;
        Some(self.take(n)?.to_vec())
    }

    fn text(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?).ok()
    }

    fn optional(&mut self) -> Option<Option<Vec<u8>>> {
        match self.u8()? {
            0 => Some(None),
            _ => Some(Some(self.bytes()?)),
        }
    }
}

/// A record's state as logged.
#[derive(Debug, Clone, PartialEq)]
struct Logged {
    table: u32,
    slot: u32,
    deleted: bool,
    original: Option<Vec<u8>>,
    image: Vec<u8>,
    memos: Vec<(u16, Option<Vec<u8>>)>,
}

/// What a log says.
#[derive(Debug, Default, PartialEq)]
struct Contents {
    /// The tables by number, with their index files.
    tables: Vec<Option<(Source, Vec<PathBuf>)>>,
    /// The record states that stand, in order.
    records: Vec<Logged>,
    /// Every record state logged, also those a rollback to a savepoint took
    /// back.
    all: Vec<Logged>,
    committed: bool,
}

/// Reads a log's bytes, up to the first entry that is cut short or
/// damaged; `None` if it is no log.
fn parse(bytes: &[u8]) -> Option<Contents> {
    let mut rest = bytes.strip_prefix(MAGIC.as_slice())?;
    let mut contents = Contents::default();
    while rest.len() >= 8 {
        let length = u32::from_le_bytes(rest[..4].try_into().ok()?) as usize;
        let crc = u32::from_le_bytes(rest[4..8].try_into().ok()?);
        let Some(entry) = rest[8..].get(..length) else {
            break;
        };
        if crc32(entry) != crc {
            break;
        }
        rest = &rest[8 + length..];
        if parse_entry(entry, &mut contents).is_none() {
            break;
        }
    }
    Some(contents)
}

fn parse_entry(entry: &[u8], contents: &mut Contents) -> Option<()> {
    let mut r = Reader { bytes: entry };
    match r.u8()? {
        TABLE => {
            let number = r.u32()? as usize;
            let table_type = type_of(r.u8()?);
            let char_type = r.u16()?;
            let collation = r.text()?;
            let path = PathBuf::from(r.text()?);
            let count = r.u16()?;
            let indexes = (0..count)
                .map(|_| r.text().map(PathBuf::from))
                .collect::<Option<Vec<_>>>()?;
            if contents.tables.len() <= number {
                contents.tables.resize(number + 1, None);
            }
            let source = Source {
                path,
                table_type,
                char_type,
                collation,
            };
            contents.tables[number] = Some((source, indexes));
        }
        RECORD => {
            let table = r.u32()?;
            let slot = r.u32()?;
            let deleted = r.u8()? != 0;
            let original = r.optional()?;
            let image = r.bytes()?;
            let count = r.u16()?;
            let mut memos = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                memos.push((r.u16()?, r.optional()?));
            }
            let logged = Logged {
                table,
                slot,
                deleted,
                original,
                image,
                memos,
            };
            contents.all.push(logged.clone());
            contents.records.push(logged);
        }
        ROLLBACK_TO => {
            let kept = r.u32()? as usize;
            contents.records.truncate(kept);
        }
        COMMIT => contents.committed = true,
        _ => return None,
    }
    Some(())
}

/// For one table of a log, the records to finish or take back, each once,
/// in the order the transaction first wrote them: the state that stands
/// (or, for one a rollback to a savepoint took back, the last logged, in
/// case the crash came before that rollback reached it), and its bytes
/// before the transaction (from its first state).
fn records_of(contents: &Contents, table: u32) -> Vec<(u32, Logged)> {
    let mut slots: Vec<u32> = Vec::new();
    for logged in contents.all.iter().filter(|l| l.table == table) {
        if !slots.contains(&logged.slot) {
            slots.push(logged.slot);
        }
    }
    let last = |list: &[Logged], slot: u32| {
        list.iter()
            .rev()
            .find(|l| l.table == table && l.slot == slot)
            .cloned()
    };
    let mut records = Vec::new();
    for slot in slots {
        let standing = last(&contents.records, slot);
        // A record a rollback to a savepoint took back entirely is not
        // committed.
        if contents.committed && standing.is_none() {
            continue;
        }
        let Some(mut state) = standing.or_else(|| last(&contents.all, slot)) else {
            continue;
        };
        let first = contents
            .all
            .iter()
            .find(|l| l.table == table && l.slot == slot);
        state.original = first.and_then(|first| first.original.clone());
        records.push((slot, state));
    }
    records
}

thread_local! {
    /// Whether this thread is recovering, so that the tables it opens do not
    /// start another recovery.
    static RECOVERING: Cell<bool> = const { Cell::new(false) };
}

/// Recovers the transactions in the log folder that crashed (see the
/// module's documentation), with the collations of `session`: how many.
/// [`Error::RecoveryFailed`] if one could not be, which stays for the next
/// recovery.
pub fn recover(session: &Session) -> Result<usize, Error> {
    recover_in(session, &folder())
}

/// [`recover`] for the logs in `folder`.
pub fn recover_in(session: &Session, folder: &Path) -> Result<usize, Error> {
    if RECOVERING.get() {
        return Ok(0);
    }
    // One recovery at a time in the process: a table another thread opens
    // meanwhile waits for it here, rather than be written before it.
    static RECOVERY: Mutex<()> = Mutex::new(());
    let _one = lock(&RECOVERY);
    RECOVERING.set(true);
    let result = recover_folder(session, folder);
    RECOVERING.set(false);
    result
}

fn recover_folder(session: &Session, folder: &Path) -> Result<usize, Error> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Ok(0);
    };
    let mut logs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "log"))
        .collect();
    logs.sort();
    let mut recovered = 0;
    let mut failed = false;
    for path in logs {
        match recover_log(session, &path) {
            Ok(true) => recovered += 1,
            Ok(false) => {}
            Err(_) => failed = true,
        }
    }
    match failed {
        true => Err(Error::RecoveryFailed),
        false => Ok(recovered),
    }
}

/// Recovers the transaction of the log at `path` if it crashed: whether it
/// did.
fn recover_log(session: &Session, path: &Path) -> Result<bool, Error> {
    let failed = |_| Error::RecoveryFailed;
    if lock(&LIVE).iter().any(|live| live == path) {
        return Ok(false);
    }
    let mut file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(failed(error)),
    };
    // Its transaction runs, or another recovers it.
    if !tflip_storage::try_lock_exclusive(&file).map_err(failed)? {
        return Ok(false);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(failed)?;
    if let Some(contents) = parse(&bytes) {
        for (number, table) in contents.tables.iter().enumerate() {
            let Some((source, indexes)) = table else {
                continue;
            };
            let records = records_of(&contents, number as u32);
            recover_table(session, source, indexes, records, contents.committed)?;
        }
    }
    drop(file);
    fs::remove_file(path).map_err(failed)?;
    Ok(true)
}

/// Commits or rolls back `records` of the table `source`, and rebuilds its
/// index files if any was still marked.
fn recover_table(
    session: &Session,
    source: &Source,
    indexes: &[PathBuf],
    records: Vec<(u32, Logged)>,
    committed: bool,
) -> Result<(), Error> {
    if records.is_empty() {
        return Ok(());
    }
    let own = Session::new();
    crate::settings::share_collations(&own, session);
    let settings = TableSettings {
        table_type: source.table_type,
        char_type: source.char_type,
        collation: source.collation.clone(),
        options: crate::table::options::EXCLUSIVE,
        ..TableSettings::default()
    };
    let path = source.path.to_string_lossy();
    let handle = table::open(&own, 0, &path, settings).map_err(|_| Error::RecoveryFailed)?;
    for index in indexes {
        let _ = index::open_file(&own, handle, &index.to_string_lossy());
    }
    let shared = table::get(&own, handle)?.shared();
    let mut touched = false;
    let ordered: Vec<(u32, Logged)> = match committed {
        true => records,
        false => records.into_iter().rev().collect(),
    };
    for (slot, state) in ordered {
        let marked = {
            let mut file = shared.file();
            file.record(slot).is_ok_and(|bytes| file.marked(&bytes))
        };
        if !marked {
            continue;
        }
        let record = TxRecord {
            tx: 0,
            original: state.original,
            image: state.image,
            deleted: state.deleted,
            orders: Vec::new(),
            owner: 0,
            memos: state.memos,
            unwritten: false,
            by_ri: false,
            sealed: false,
        };
        shared.transactions().insert(slot, record);
        match committed {
            true => crate::table::commit_record(&shared, slot, &[])?,
            false => crate::table::roll_back_record(&shared, slot, None)?,
        }
        touched = true;
    }
    // A DBF table's end-of-file byte, which a transaction that appended
    // leaves for the close (`defer_end_of_file`), is written as it closes.
    if touched {
        shared
            .file()
            .defer_end_of_file()
            .map_err(|_| Error::RecoveryFailed)?;
    }
    // The orders are rebuilt even when no record was still marked: a crash
    // in a commit (or in an earlier recovery) between a record's write and
    // its keys leaves the keys behind.
    index::reindex(&own, handle)?;
    table::close(&own, handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logged(slot: u32, image: u8, original: Option<u8>) -> Logged {
        Logged {
            table: 0,
            slot,
            deleted: false,
            original: original.map(|b| vec![b; 4]),
            image: vec![image; 4],
            memos: vec![(10, None), (20, Some(b"memo".to_vec()))],
        }
    }

    fn entry(logged: &Logged) -> Vec<u8> {
        let mut entry = vec![RECORD];
        put_u32(&mut entry, logged.table);
        put_u32(&mut entry, logged.slot);
        entry.push(u8::from(logged.deleted));
        put_optional(&mut entry, logged.original.as_deref());
        put_bytes(&mut entry, &logged.image);
        entry.extend_from_slice(&(logged.memos.len() as u16).to_le_bytes());
        for (offset, value) in &logged.memos {
            entry.extend_from_slice(&offset.to_le_bytes());
            put_optional(&mut entry, value.as_deref());
        }
        entry
    }

    fn frame(entry: &[u8]) -> Vec<u8> {
        let mut frame = Vec::new();
        put_u32(&mut frame, entry.len() as u32);
        put_u32(&mut frame, crc32(entry));
        frame.extend_from_slice(entry);
        frame
    }

    #[test]
    fn crc32_is_ieee() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    /// Entries are read up to one cut short or damaged; a rollback to a
    /// savepoint takes back the states after it, which stay known.
    #[test]
    fn parses_up_to_damage() {
        let (a, b, c) = (
            logged(1, 0xA, Some(1)),
            logged(2, 0xB, None),
            logged(1, 0xC, Some(1)),
        );
        let mut bytes = MAGIC.to_vec();
        bytes.extend(frame(&entry(&a)));
        bytes.extend(frame(&entry(&b)));
        bytes.extend(frame(&[ROLLBACK_TO, 1, 0, 0, 0]));
        bytes.extend(frame(&entry(&c)));
        let whole = bytes.clone();
        bytes.extend(frame(&[COMMIT]));
        let contents = parse(&bytes).unwrap();
        assert_eq!(contents.records, [a.clone(), c.clone()]);
        assert_eq!(contents.all, [a.clone(), b.clone(), c.clone()]);
        assert!(contents.committed);
        // The commit cut short, or damaged: not committed.
        let cut = &bytes[..bytes.len() - 1];
        assert!(!parse(cut).unwrap().committed);
        let mut damaged = bytes.clone();
        *damaged.last_mut().unwrap() ^= 1;
        assert_eq!(parse(&damaged).unwrap(), parse(&whole).unwrap());
        assert!(parse(b"nothing").is_none());
    }

    /// What recovery does with each record: committed, the states that
    /// stand; rolled back, every record ever written, from its first bytes.
    #[test]
    fn records_to_recover() {
        let (a, b, c) = (
            logged(1, 0xA, Some(1)),
            logged(2, 0xB, None),
            logged(1, 0xC, Some(1)),
        );
        let mut contents = Contents {
            tables: Vec::new(),
            records: vec![a.clone(), c.clone()],
            all: vec![a.clone(), b.clone(), c.clone()],
            committed: true,
        };
        let slots = |records: Vec<(u32, Logged)>| -> Vec<(u32, u8)> {
            records
                .into_iter()
                .map(|(slot, l)| (slot, l.image[0]))
                .collect()
        };
        assert_eq!(slots(records_of(&contents, 0)), [(1, 0xC)]);
        contents.committed = false;
        assert_eq!(slots(records_of(&contents, 0)), [(1, 0xC), (2, 0xB)]);
        assert!(records_of(&contents, 1).is_empty());
    }
}
