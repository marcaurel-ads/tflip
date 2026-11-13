//! Memo files: where Memo, Binary, Image and NMemo values are.
//!
//! An ADT memo file (`.adm`) is a heap of blocks. A value starts at its
//! first block × the block size and takes its stored length in consecutive
//! blocks; a compressed value is a zlib stream. Reading a value needs only
//! its reference from the record ([`MemoRef`]) and the block size from the
//! file header; the rest of the header is the write side's allocator.
//!
//! DBF tables keep their values in a Clipper `.dbt` or a FoxPro `.fpt`
//! (see [`crate::dbf`]): there the record holds only the first block, and
//! the value's length is in the memo file.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::field::MemoRef;
use crate::source::Source;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use crate::table::{Error, RecordCipher};

/// The kind of a memo file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoKind {
    /// ADT (`.adm`).
    Adm,
    /// Clipper (`.dbt`): 512-byte blocks, a text value ends at its first
    /// 0x1A.
    Dbt,
    /// FoxPro (`.fpt`): each value starts with its type and length.
    Fpt,
}

/// An open memo file.
#[derive(Debug)]
pub struct MemoFile {
    file: Source,
    kind: MemoKind,
    block_size: u32,
    /// Where the file is, if it was opened from a path.
    path: Option<PathBuf>,
    /// Whether an `.adm` file's allocations go uncounted (see
    /// [`MemoFile::set_counting`]).
    uncounted: bool,
    /// Whether the next values stored are put in new blocks, not over the
    /// old ones (see [`MemoFile::set_moving`]).
    moving: bool,
    /// How an encrypted table's memo values are deciphered on read; `None`
    /// leaves them as stored.
    cipher: RecordCipher,
    /// The item (the referencing record's number) the next read deciphers at,
    /// set by [`MemoFile::decipher_record`] before reading a record's value.
    item: u32,
    /// An AES table's write IV counter, shared with the table's records so a
    /// memo value and a record never reuse an IV (see [`MemoFile::aes_write`]).
    aes_iv: Arc<AtomicU64>,
    /// Whether each value is stored behind an 8-byte IV even when it is not
    /// enciphered, as an AES dictionary that keeps its records in the clear
    /// stores its memo values (`fixtures/aes/dictwrite`).
    iv_prefix: bool,
    /// The key and nonce of the values and records a table that was
    /// decrypted still has enciphered (see [`crate::adm::SlabRecords`]).
    enciphered_before: Option<(Vec<u8>, [u8; 4])>,
}

/// What [`MemoFile::read_dbf`] says of a block that starts past the end of
/// the file.
pub const BLOCK_PAST_END: &str = "memo block past the end of the file";

impl MemoFile {
    /// Opens the memo file at `path`.
    pub fn open(path: &Path) -> Result<MemoFile, Error> {
        MemoFile::from_source(File::open(path).map_err(Error::Io)?.into())
    }

    /// Opens the ADT memo file at `path`, for writing if `write`.
    pub fn open_adm(path: &Path, write: bool) -> Result<MemoFile, Error> {
        let file = OpenOptions::new()
            .read(true)
            .write(write)
            .open(path)
            .map_err(Error::Io)?;
        MemoFile::from_source(file.into())
    }

    /// The allocator of an ADT memo file, to write values with.
    pub(crate) fn allocator(&mut self) -> Result<crate::adm::Allocator<'_>, Error> {
        if self.kind != MemoKind::Adm {
            return Err(Error::NotWritable);
        }
        let records = match (&self.cipher, self.iv_prefix) {
            (RecordCipher::Aes { key, nonce }, _) => crate::adm::SlabRecords::Enciphered {
                key: key.clone(),
                nonce: *nonce,
                counter: Arc::clone(&self.aes_iv),
            },
            (_, true) => crate::adm::SlabRecords::Clear(self.enciphered_before.clone()),
            _ => crate::adm::SlabRecords::Plain,
        };
        crate::adm::Allocator::new(&mut self.file, self.block_size, !self.uncounted, records)
    }

    /// Gives back the blocks of an `.fpt` value at `block`, `stored` bytes
    /// long (its block header not counted), if they are the last before the
    /// next free block: the next free block moves back to it, as the
    /// original's server does when a transaction that appended it rolls
    /// back (`fixtures/write/tx_dbf`). Other memo files keep their blocks.
    pub fn release_at_end(&mut self, block: u32, stored: u32) -> Result<(), Error> {
        if self.kind != MemoKind::Fpt || block == 0 {
            return Ok(());
        }
        let blocks = (stored + 8 + self.value_overhead()).div_ceil(self.block_size);
        let mut next = [0; 4];
        self.file.read_at(0, &mut next)?;
        if u32::from_be_bytes(next) == block + blocks {
            self.file.write_at(0, &block.to_be_bytes())?;
        }
        Ok(())
    }

    /// Makes an `.adm` file's allocations count in its header, or not: the
    /// original's server does not count those of a transaction
    /// (`fixtures/write/tx_basic`).
    pub fn set_counting(&mut self, counting: bool) {
        self.uncounted = !counting;
    }

    /// Makes the values a DBF table's memo file stores next go to new blocks
    /// instead of over the old ones, as the original's server writes a value
    /// while another handle stands on its record.
    pub fn set_moving(&mut self, moving: bool) {
        self.moving = moving;
    }

    /// Whether stored values are put in new blocks (see
    /// [`MemoFile::set_moving`]).
    pub(crate) fn moving(&self) -> bool {
        self.moving
    }

    /// The memo file in `file`, a file or the bytes of one.
    pub fn from_source(file: Source) -> Result<MemoFile, Error> {
        MemoFile::of_kind(file, MemoKind::Adm)
    }

    /// The memo file of kind `kind` at `path`, to be written if `write`.
    pub fn open_kind(path: &Path, kind: MemoKind, write: bool) -> Result<MemoFile, Error> {
        let file = OpenOptions::new()
            .read(true)
            .write(write)
            .open(path)
            .map_err(Error::Io)?;
        let mut memo = MemoFile::of_kind(file.into(), kind)?;
        memo.path = Some(path.to_path_buf());
        Ok(memo)
    }

    /// The memo file of kind `kind` in `file`.
    pub fn of_kind(mut file: Source, kind: MemoKind) -> Result<MemoFile, Error> {
        if kind == MemoKind::Dbt {
            return Ok(MemoFile {
                file,
                kind,
                block_size: 512,
                path: None,
                uncounted: false,
                moving: false,
                cipher: RecordCipher::None,
                item: 0,
                aes_iv: Arc::new(AtomicU64::new(0)),
                iv_prefix: false,
                enciphered_before: None,
            });
        }
        let mut header = [0; 8];
        file.read_exact(&mut header)
            .map_err(|_| Error::Corrupt("memo file ends inside its header"))?;
        // A big-endian u16 at 0x06, as in FoxPro's FPT files.
        let block_size = u32::from(u16::from_be_bytes([header[6], header[7]]));
        if block_size == 0 {
            return Err(Error::Corrupt("memo block size 0"));
        }
        Ok(MemoFile {
            file,
            kind,
            block_size,
            path: None,
            uncounted: false,
            moving: false,
            cipher: RecordCipher::None,
            item: 0,
            aes_iv: Arc::new(AtomicU64::new(0)),
            iv_prefix: false,
            enciphered_before: None,
        })
    }

    /// The memo file belonging to the table at `table`: the same name with
    /// `.adm` (`.ADM` if the table's extension is upper case).
    pub fn path_for(table: &Path) -> PathBuf {
        let upper = table
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e == "ADT");
        table.with_extension(if upper { "ADM" } else { "adm" })
    }

    /// The file, to write to.
    pub(crate) fn source(&mut self) -> &mut Source {
        &mut self.file
    }

    /// Where the file is, if it was opened from a path.
    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The kind of memo file.
    pub fn kind(&self) -> MemoKind {
        self.kind
    }

    /// The block size in bytes.
    pub fn block_size(&self) -> u32 {
        self.block_size
    }

    /// Makes the file's blocks `block_size` bytes, as when it is written
    /// anew.
    pub(crate) fn set_block_size(&mut self, block_size: u32) {
        self.block_size = block_size;
    }

    /// Give the memo file its cipher, so reads decipher an encrypted table's
    /// memo values (see [`crate::adt::open_with_password`]).
    pub(crate) fn set_cipher(&mut self, cipher: RecordCipher) {
        self.cipher = cipher;
    }

    /// Remembers the key and nonce the file's records were enciphered with,
    /// when the table is decrypted.
    pub(crate) fn set_enciphered_before(&mut self, key: Vec<u8>, nonce: [u8; 4]) {
        self.enciphered_before = Some((key, nonce));
    }

    /// The bytes before each value: an IV, 8 bytes, in an AES table's file,
    /// enciphered or not.
    pub(crate) fn value_overhead(&self) -> u32 {
        match self.iv_prefix || matches!(self.cipher, RecordCipher::Aes { .. }) {
            true => 8,
            false => 0,
        }
    }

    /// Reads each value from behind an 8-byte IV, enciphered or not (see
    /// [`MemoFile::iv_prefix`]).
    pub(crate) fn set_iv_prefix(&mut self, iv_prefix: bool) {
        self.iv_prefix = iv_prefix;
    }

    /// Takes the memo file's cipher away, leaving its values read and
    /// written as they are stored (see [`crate::table::TableFile::seal`]).
    pub(crate) fn take_cipher(&mut self) -> RecordCipher {
        std::mem::replace(&mut self.cipher, RecordCipher::None)
    }

    /// Set the item the next reads decipher at: the number of the record whose
    /// memo values are about to be read (every memo of a record is that one
    /// item). No effect on an unencrypted table.
    pub fn decipher_record(&mut self, record: u32) {
        self.item = record;
    }

    /// Reads the value `memo` refers to, decompressed; NULL reads as empty.
    pub fn read(&mut self, memo: MemoRef) -> Result<Vec<u8>, Error> {
        if memo.block == 0 {
            return Ok(Vec::new());
        }
        let offset = u64::from(memo.block) * u64::from(self.block_size);
        self.file.seek(SeekFrom::Start(offset))?;
        // An encrypted table's memo value is one cipher stream over its stored
        // (possibly compressed) bytes (see [`crate::crypto`]): RC4 at the item
        // of the record that references it, AES-CTR under an 8-byte IV the value
        // is stored with.
        let mut iv = [0u8; 8];
        if self.iv_prefix || matches!(self.cipher, RecordCipher::Aes { .. }) {
            self.file
                .read_exact(&mut iv)
                .map_err(|_| Error::Corrupt("memo file ends inside a value"))?;
        }
        let mut stored = vec![0; memo.stored as usize];
        self.file
            .read_exact(&mut stored)
            .map_err(|_| Error::Corrupt("memo file ends inside a value"))?;
        match &self.cipher {
            RecordCipher::None => {}
            RecordCipher::Rc4(password) => {
                crate::crypto::rc4_crypt(password, self.item, &mut stored);
            }
            // A value written before the table was armed is still in the
            // clear behind a zero IV, until `AdsEncryptTable` enciphers it.
            RecordCipher::Aes { key, nonce } if iv != [0; 8] => {
                crate::crypto::aes_ctr_xor(key, &iv, nonce, &mut stored);
            }
            RecordCipher::Aes { .. } => {}
        }
        if memo.stored >= memo.length {
            stored.truncate(memo.length as usize);
            return Ok(stored);
        }
        let value = miniz_oxide::inflate::decompress_to_vec_zlib(&stored)
            .map_err(|_| Error::Corrupt("compressed memo value does not inflate"))?;
        if value.len() != memo.length as usize {
            return Err(Error::Corrupt("compressed memo value has the wrong length"));
        }
        Ok(value)
    }

    /// The RC4 key and item (the record number set by [`MemoFile::decipher_record`])
    /// to encipher a memo value with on write, if the table is RC4-encrypted.
    /// `None` leaves the value in the clear (an unencrypted or record-level
    /// table).
    pub(crate) fn rc4_write(&self) -> Option<(Vec<u8>, u32)> {
        match &self.cipher {
            RecordCipher::Rc4(key) => Some((key.clone(), self.item)),
            _ => None,
        }
    }

    /// Share the table's AES write IV counter, so memo values and records draw
    /// IVs from the one counter (see [`crate::table::Records`]).
    pub(crate) fn set_aes_counter(&mut self, counter: Arc<AtomicU64>) {
        self.aes_iv = counter;
    }

    /// The AES key and nonce to encipher a memo value with on write, if the
    /// table is AES-encrypted. An AES memo value is stored as `[8-byte IV][AES-CTR
    /// ciphertext]`, the IV drawn from the shared counter (see
    /// [`MemoFile::aes_counter`]) so it never collides with a record's. `None`
    /// leaves the value clear.
    pub(crate) fn aes_cipher(&self) -> Option<(Vec<u8>, [u8; 4])> {
        match &self.cipher {
            RecordCipher::Aes { key, nonce } => Some((key.clone(), *nonce)),
            _ => None,
        }
    }

    /// A clone of the shared AES write IV counter, taken before the allocator
    /// borrows the file so a memo write can draw and advance it.
    pub(crate) fn aes_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.aes_iv)
    }

    /// Writes `data` at the start of block `block`, as it is stored (the
    /// inverse region of [`MemoFile::read_raw`]): used to re-cipher a memo
    /// value in place when a table is encrypted or decrypted.
    pub(crate) fn write_raw(&mut self, block: u32, data: &[u8]) -> Result<(), Error> {
        let at = u64::from(block) * u64::from(self.block_size);
        self.file.write_at(at, data)?;
        Ok(())
    }

    /// Reads `length` bytes at the start of block `block`, as they are stored.
    pub fn read_raw(&mut self, block: u32, length: usize) -> Result<Vec<u8>, Error> {
        let mut bytes = vec![0; length];
        self.file.seek(SeekFrom::Start(
            u64::from(block) * u64::from(self.block_size),
        ))?;
        self.file
            .read_exact(&mut bytes)
            .map_err(|_| Error::Corrupt("memo file ends inside a value"))?;
        Ok(bytes)
    }

    /// Reads the DBF memo value that starts at block `block`: in an `.fpt`
    /// the length its block header gives; in a `.dbt` the bytes up to the
    /// first 0x1A, or for a `binary` value its SIx tag and the length the
    /// tag gives (see [`crate::dbf`]).
    pub fn read_dbf(&mut self, block: u32, binary: bool) -> Result<Vec<u8>, Error> {
        let start = u64::from(block) * u64::from(self.block_size);
        self.file.seek(SeekFrom::Start(start))?;
        let rc4 = match &self.cipher {
            RecordCipher::Rc4(key) => Some(key.clone()),
            _ => None,
        };
        if self.kind == MemoKind::Fpt {
            let mut header = [0; 8];
            // A reference to a block past the end of the file (an enciphered
            // record's, read as it is stored) is a failed read.
            if start >= self.file.len()? {
                return Err(Error::Corrupt(BLOCK_PAST_END));
            }
            self.file
                .read_exact(&mut header)
                .map_err(|_| Error::Corrupt("memo file ends inside a block header"))?;
            let length = u32::from_be_bytes(header[4..].try_into().unwrap());
            // An AES table's value is behind an IV, zero when it was not
            // enciphered.
            let mut iv = [0; 8];
            if self.value_overhead() != 0 {
                self.file
                    .read_exact(&mut iv)
                    .map_err(|_| Error::Corrupt("memo file ends inside a value"))?;
            }
            let mut value = Vec::new();
            (&mut self.file)
                .take(u64::from(length))
                .read_to_end(&mut value)?;
            if value.len() != length as usize {
                return Err(Error::Corrupt("memo file ends inside a value"));
            }
            // The block header stays clear; the value is one stream at the
            // number of the record that refers to it, but for the SIx tag of
            // a Binary or Image value, which is clear too
            // (`fixtures/aes/rc4tp/y13`).
            if let Some(key) = rc4 {
                let tag = if binary { 16.min(value.len()) } else { 0 };
                crate::crypto::rc4_crypt(&key, self.item, &mut value[tag..]);
            }
            if let (RecordCipher::Aes { key, nonce }, true) = (&self.cipher, iv != [0; 8]) {
                crate::crypto::aes_ctr_xor(key, &iv, nonce, &mut value);
            }
            return Ok(value);
        }
        if binary {
            let mut tag = [0; 16];
            self.file
                .read_exact(&mut tag)
                .map_err(|_| Error::Corrupt("memo file ends inside a tag"))?;
            let length = u32::from_le_bytes(tag[12..].try_into().unwrap());
            let mut data = Vec::new();
            (&mut self.file)
                .take(u64::from(length))
                .read_to_end(&mut data)?;
            if data.len() != length as usize {
                return Err(Error::Corrupt("memo file ends inside a value"));
            }
            // The tag and the bytes after the data are clear, the data one
            // stream at the record's number (`fixtures/aes/rc4tp/y13`).
            if let Some(key) = &rc4 {
                crate::crypto::rc4_crypt(key, self.item, &mut data);
            }
            let mut value = tag.to_vec();
            value.extend_from_slice(&data);
            return Ok(value);
        }
        if let Some(key) = rc4 {
            // The value and its first 0x1A are one stream: decipher what has
            // been read until the 0x1A shows.
            let mut raw = Vec::new();
            let mut chunk = [0; 512];
            loop {
                let read = self.file.read(&mut chunk)?;
                raw.extend_from_slice(&chunk[..read]);
                let mut clear = raw.clone();
                crate::crypto::rc4_crypt(&key, self.item, &mut clear);
                if let Some(end) = clear.iter().position(|&b| b == 0x1A) {
                    clear.truncate(end);
                    return Ok(clear);
                }
                if read == 0 {
                    return Ok(clear);
                }
            }
        }
        let mut value = Vec::new();
        let mut chunk = [0; 512];
        loop {
            let read = self.file.read(&mut chunk)?;
            if let Some(end) = chunk[..read].iter().position(|&b| b == 0x1A) {
                value.extend_from_slice(&chunk[..end]);
                return Ok(value);
            }
            if read == 0 {
                return Ok(value);
            }
            value.extend_from_slice(&chunk[..read]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memo_paths() {
        assert_eq!(
            MemoFile::path_for(Path::new("/d/a.adt")),
            PathBuf::from("/d/a.adm")
        );
        assert_eq!(
            MemoFile::path_for(Path::new("/d/A.ADT")),
            PathBuf::from("/d/A.ADM")
        );
    }
}
