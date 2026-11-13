//! A data dictionary's `.add`, open to write its object records.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use crate::adt::{AES_IV_SIZE, RECORD_PREFIX};
use crate::table::{Error, TableFile};

/// The dictionary's file, its records read and written in the layout of a
/// dictionary of the default encryption (see [`super::create`]), whatever
/// the file's. An AES dictionary keeps its fields [`AES_IV_SIZE`] bytes on,
/// after each record's IV; one that encrypts its records enciphers each it
/// writes under a fresh IV, as the original does (`fixtures/aes/dictwrite`); one
/// that encrypts them with RC4 enciphers each under the fixed key.
pub struct DictionaryFile {
    file: TableFile,
    path: PathBuf,
    /// Whether the file has an AES dictionary's layout.
    aes: bool,
    /// The IV counter of a dictionary that encrypts its records.
    counter: Option<Arc<AtomicU64>>,
    /// Whether the file encrypts its records with RC4.
    rc4: bool,
}

impl DictionaryFile {
    /// Opens the dictionary at `path`. One that encrypts its records with
    /// AES opens only once a connection of the process gave its dictionary
    /// password ([`Error::Encrypted`]).
    pub fn open(path: &Path) -> Result<DictionaryFile, Error> {
        let mut file = TableFile::open_dictionary_for_writing(path)?;
        let header = &file.structure.header;
        let (aes, encrypted) = (header.extension != 0, header.encrypted());
        let rc4 = !aes && encrypted && header.encryption_type == crate::adt::ENCRYPTION_RC4;
        let mut counter = None;
        // An RC4 dictionary enciphers its records under the fixed key.
        if rc4 {
            file.set_encryption_key(crate::crypto::OBFUSCATION_KEY.to_vec());
        }
        if aes && encrypted {
            let (cipher, shared) = super::keyring::reserve(path)?.ok_or(Error::Encrypted)?;
            file.arm(cipher, Arc::clone(&shared));
            counter = Some(shared);
        }
        Ok(DictionaryFile {
            file,
            path: path.to_path_buf(),
            aes,
            counter,
            rc4,
        })
    }

    /// `record` in the file's layout.
    fn widen(&self, record: &[u8]) -> Vec<u8> {
        match self.aes {
            false => record.to_vec(),
            true => {
                let mut wide = Vec::with_capacity(record.len() + AES_IV_SIZE);
                wide.extend_from_slice(&record[..RECORD_PREFIX]);
                wide.extend_from_slice(&[0; AES_IV_SIZE]);
                wide.extend_from_slice(&record[RECORD_PREFIX..]);
                wide
            }
        }
    }

    /// `record`, read in the file's layout, in a plain dictionary's.
    fn narrow(&self, mut record: Vec<u8>) -> Vec<u8> {
        if self.aes {
            record.drain(RECORD_PREFIX..RECORD_PREFIX + AES_IV_SIZE);
        }
        record
    }

    /// Stores `record` in a new slot (see [`TableFile::insert`]).
    pub fn insert(&mut self, record: &[u8]) -> Result<u32, Error> {
        let wide = self.widen(record);
        let slot = self.file.insert(&wide)?;
        self.encipher_values(slot, record)?;
        Ok(slot)
    }

    /// Enciphers the memo values `record`, just stored in `slot`, refers to
    /// and that were written in the clear (see [`super::memo`]) if the
    /// dictionary enciphers its records with RC4.
    fn encipher_values(&self, slot: u32, record: &[u8]) -> Result<(), Error> {
        match self.rc4 {
            true => super::memo::encipher_pending(&self.path, slot, record),
            false => Ok(()),
        }
    }

    /// Writes `record` to slot `slot` (see [`TableFile::write`]).
    pub fn write(&mut self, slot: u32, record: &[u8]) -> Result<(), Error> {
        let wide = self.widen(record);
        self.file.write(slot, &wide)?;
        self.encipher_values(slot, record)
    }

    /// Writes `bytes` at `offset` of the record in slot `slot`, leaving the
    /// rest of it. An enciphered record is written again whole, under a new
    /// IV.
    pub fn write_part(&mut self, slot: u32, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        let at = match self.aes && offset >= RECORD_PREFIX {
            true => offset + AES_IV_SIZE,
            false => offset,
        };
        if self.counter.is_none() && !self.rc4 {
            return self.file.write_part(slot, at, bytes);
        }
        if self.rc4 {
            let mut record = self.file.record(slot)?;
            record
                .get_mut(at..at + bytes.len())
                .ok_or(Error::Corrupt("no such record slot"))?
                .copy_from_slice(bytes);
            self.file.write(slot, &record)?;
            let record = self.record(slot)?;
            return self.encipher_values(slot, &record);
        }
        let mut record = self.file.record(slot)?;
        record
            .get_mut(at..at + bytes.len())
            .ok_or(Error::Corrupt("no such record slot"))?
            .copy_from_slice(bytes);
        self.file.write(slot, &record)
    }

    /// Reads slot `slot` into `buf`, which holds a record of a plain
    /// dictionary.
    pub fn read(&mut self, slot: u32, buf: &mut [u8]) -> Result<(), Error> {
        let record = self.record(slot)?;
        if record.len() != buf.len() {
            return Err(Error::Corrupt("record of the wrong length"));
        }
        buf.copy_from_slice(&record);
        Ok(())
    }

    /// The record in slot `slot`.
    pub fn record(&mut self, slot: u32) -> Result<Vec<u8>, Error> {
        let record = self.file.record(slot)?;
        Ok(self.narrow(record))
    }

    /// Deletes slot `slot` (see [`TableFile::delete`]), its record taking
    /// the bytes of `record` if given.
    pub fn delete(&mut self, slot: u32, record: Option<&[u8]>) -> Result<(), Error> {
        let wide = record.map(|record| self.widen(record));
        self.file.delete(slot, wide.as_deref())
    }

    /// The slot [`DictionaryFile::insert`] stores the next record in.
    pub fn next_slot(&mut self) -> u32 {
        self.file.next_slot()
    }

    /// Sets the next object id the header keeps.
    pub fn set_next_autoinc(&mut self, next: u32) -> Result<(), Error> {
        self.file.set_next_autoinc(next)
    }

    /// The number of record slots, deleted ones included.
    pub fn record_slots(&self) -> u32 {
        self.file.structure.header.record_slots
    }
}

impl Drop for DictionaryFile {
    fn drop(&mut self) {
        if let Some(counter) = &self.counter {
            let _ = super::keyring::persist(&self.path, counter);
        }
    }
}
