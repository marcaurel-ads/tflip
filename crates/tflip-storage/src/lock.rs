//! Byte-range locks on files, which other processes see: the locks tflip's
//! clients in different processes keep each other from records with, and
//! the lock that keeps their writes to a table's files from overlapping.
//!
//! The ranges are tflip's own, far past the end of any file: they are not
//! the original engine's, so tflip and the original cannot share files
//! safely (see `docs/KNOWN_ISSUES.md`). On Linux they are open file
//! description locks (`F_OFD_SETLK`), which belong to the open file rather
//! than the process, so closing another descriptor of the same file does not
//! drop them, and two opens in one process exclude each other as two
//! processes do; on Windows they are `LockFileEx` locks, which behave so.

use std::fs::File;
use std::io;

/// Where tflip's ranges start: 2^62, past any file.
const BASE: u64 = 1 << 62;
/// The byte a table's openers lock: shared, or exclusive for an exclusive
/// open.
pub(crate) const OPEN: u64 = BASE;
/// The byte a write holds while it changes a table's files.
pub(crate) const WRITE: u64 = BASE + 1;
/// Where the bytes of the record locks start: one per record slot.
pub(crate) const RECORDS: u64 = BASE + 0x10;
/// The range a table lock takes: every record's byte.
pub(crate) const TABLE_LENGTH: u64 = 1 << 33;

/// The byte of record slot `slot`'s lock.
pub(crate) fn record(slot: u32) -> u64 {
    RECORDS + u64::from(slot)
}

/// Locks `length` bytes at `offset` of `file`: exclusively or shared, and
/// with `wait` until it is granted. Whether it was granted; without `wait`,
/// `false` if another holds a lock that conflicts.
pub(crate) fn lock(
    file: &File,
    offset: u64,
    length: u64,
    exclusive: bool,
    wait: bool,
) -> io::Result<bool> {
    crate::os::file_lock::lock(file, offset, length, exclusive, wait)
}

/// Locks `file` exclusively for its opener, if no other open of it holds
/// that lock (a transaction's log, which its transaction holds while it
/// runs): whether it was granted. It goes when the file closes.
pub fn try_lock_exclusive(file: &File) -> io::Result<bool> {
    lock(file, OPEN, 1, true, false)
}

/// Releases the lock on `length` bytes at `offset` of `file`.
pub(crate) fn unlock(file: &File, offset: u64, length: u64) -> io::Result<()> {
    crate::os::file_lock::unlock(file, offset, length)
}
