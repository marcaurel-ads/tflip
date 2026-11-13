//! The keys of the AES dictionaries this process has opened.
//!
//! A dictionary that enciphers its records is written with the key its
//! dictionary password makes, which only a connection that gave the password
//! knows; once one has, every write to the dictionary in the process can use
//! it, as the original's do (a connection without the password opens a
//! dictionary another connection of the process opened with it:
//! `tools/ace/crypto/FINDINGS.md`, "AES dictionaries"). The key is derived
//! once, as PBKDF2 is slow on purpose.
//!
//! The process also keeps one IV counter per dictionary, which its records,
//! memo values and index pages all draw from, so no two of them share an IV
//! whichever file they are written through. When the process first writes
//! to the dictionary, the counter in its header (0x198) is moved on by a
//! block of [`RESERVED`] IVs that the process may then use, as the original
//! moves it (`fixtures/aes/dictwrite`), so another process starts past them;
//! another block is reserved when one runs out.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use crate::table::RecordCipher;

/// How many IVs an open to write reserves in the header.
pub(crate) const RESERVED: u64 = 1000;

/// Where an AES table keeps its IV counter.
const COUNTER: u64 = crate::adt::AES_IV_COUNTER;

struct Entry {
    password: Vec<u8>,
    cipher: RecordCipher,
    counter: Arc<AtomicU64>,
    /// Where the block the process reserved in the header ends (0 before
    /// it reserved one).
    reserved: u64,
}

static KEYS: LazyLock<Mutex<HashMap<PathBuf, Entry>>> = LazyLock::new(Default::default);

fn key_of(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The record cipher of the dictionary at `path` for `password`, if the
/// process already derived it.
pub(crate) fn cipher_for(path: &Path, password: &[u8]) -> Option<RecordCipher> {
    let keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    keys.get(&key_of(path))
        .filter(|entry| entry.password == password)
        .map(|entry| entry.cipher.clone())
}

/// The record cipher of the dictionary at `path`, if a connection of the
/// process gave its password.
pub(crate) fn cipher(path: &Path) -> Option<RecordCipher> {
    let keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    keys.get(&key_of(path)).map(|entry| entry.cipher.clone())
}

/// Keeps `cipher`, which `password` makes, for the dictionary at `path`.
pub(crate) fn remember(path: &Path, password: &[u8], cipher: RecordCipher) {
    let mut keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    let entry = keys.entry(key_of(path)).or_insert_with(|| Entry {
        password: Vec::new(),
        cipher: RecordCipher::None,
        counter: Arc::new(AtomicU64::new(0)),
        reserved: 0,
    });
    entry.password = password.to_vec();
    entry.cipher = cipher;
}

/// The cipher of the dictionary at `path` and its IV counter, with a block
/// reserved in the header if the process holds none there (see the module's
/// documentation); `None` if no connection of the process gave its password.
pub(crate) fn reserve(path: &Path) -> io::Result<Option<(RecordCipher, Arc<AtomicU64>)>> {
    let mut keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = keys.get_mut(&key_of(path)) else {
        return Ok(None);
    };
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let stored = read_counter(&mut file)?;
    // Another process moved the counter past the block, or there is none.
    if stored != entry.reserved {
        let next = entry
            .counter
            .fetch_max(stored, Ordering::Relaxed)
            .max(stored);
        entry.reserved = next + RESERVED;
        write_counter(&mut file, entry.reserved)?;
    }
    Ok(Some((entry.cipher.clone(), Arc::clone(&entry.counter))))
}

/// Reserves another block in the header of the dictionary at `path` if the
/// process used up the one it holds.
pub(crate) fn persist(path: &Path, counter: &AtomicU64) -> io::Result<()> {
    let mut keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    let used = counter.load(Ordering::Relaxed);
    let Some(entry) = keys.get_mut(&key_of(path)) else {
        return Ok(());
    };
    if used >= entry.reserved {
        entry.reserved = used + RESERVED;
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        write_counter(&mut file, entry.reserved)?;
    }
    Ok(())
}

/// Forgets the key of the dictionary at `path`, whose file was keyed anew
/// (a conversion gave it another salt), so the next password given derives
/// the new one.
pub(crate) fn forget(path: &Path) {
    let mut keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    keys.remove(&key_of(path));
}

fn read_counter(file: &mut std::fs::File) -> io::Result<u64> {
    let mut stored = [0u8; 8];
    file.seek(SeekFrom::Start(COUNTER))?;
    file.read_exact(&mut stored)?;
    Ok(u64::from_le_bytes(stored))
}

fn write_counter(file: &mut std::fs::File, value: u64) -> io::Result<()> {
    file.seek(SeekFrom::Start(COUNTER))?;
    file.write_all(&value.to_le_bytes())
}
