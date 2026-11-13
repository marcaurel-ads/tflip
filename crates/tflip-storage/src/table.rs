//! Tables of any format: their structure, their records, and the open
//! table file that holds them with its memo file.
//!
//! Advantage's own tables (`.adt`, see [`crate::adt`]) and DBF tables
//! (`.dbf`, see [`crate::dbf`]) are read into the same [`Structure`], whose
//! [`Format`] and fields' [`Layout`](crate::field::Layout) say how their
//! bytes are stored.

mod dbf_aes;

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::adt::{self, offsets};
use crate::dbf;
use crate::field::{Field, FieldType};
use crate::lock;
use crate::memo::MemoFile;
use crate::source::Source;

/// Why a table could not be read.
#[derive(Debug)]
pub enum Error {
    /// Reading the file failed.
    Io(io::Error),
    /// The file is not a table of its format, or its header contradicts
    /// itself.
    Corrupt(&'static str),
    /// The field descriptors are encrypted.
    EncryptedDescriptors,
    /// An encrypted table could not be opened: the password is wrong, or its
    /// encryption is one not supported here yet (RC4 through this path).
    Encrypted,
    /// The table was opened read-only.
    ReadOnly,
    /// The table cannot be written yet: an encrypted table.
    NotWritable,
    /// A memo value cannot be rewritten: an ADS VarChar's remainder at the
    /// end of a `.dbt`, which the original engine finds no end of.
    CorruptMemo,
    /// A DBF table's memo file is not of the kind its table type reads (a
    /// `.dbt` for Clipper, an `.fpt` for FoxPro and VFP), as the original
    /// engine refuses it: the table type that reads it.
    WrongMemoFile(dbf::Flavour),
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::UnexpectedEof => Error::Corrupt("file ends inside the header"),
            _ => Error::Io(error),
        }
    }
}

/// Feature flags of the header, as an ADT table stores them (offset 0x58);
/// a DBF table's header is read into the same flags.
pub mod flags {
    /// The table has memo, binary or NMemo fields (a memo file exists).
    pub const MEMO: u32 = 0x01;
    /// The table has an AutoInc field.
    pub const AUTOINC: u32 = 0x02;
    /// The table is encrypted.
    pub const ENCRYPTED: u32 = 0x04;
    /// The field descriptors are encrypted too.
    pub const ENCRYPTED_DESCRIPTORS: u32 = 0x08;
    /// The table has a RowVersion field.
    pub const ROWVERSION: u32 = 0x20;
    /// The table has a ModTime field.
    pub const MODTIME: u32 = 0x40;
    /// The table is transaction-free.
    pub const TRANSACTION_FREE: u32 = 0x100;
}

/// The file format of a table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    /// Advantage's own format (`.adt`).
    #[default]
    Adt,
    /// A DBF table (`.dbf`) of the given flavour (see [`dbf`]).
    Dbf(dbf::Flavour),
}

/// The table header, with the offsets an ADT table stores its fields at. A
/// DBF table's header is read into the same fields (see [`dbf`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// The table's format.
    pub format: Format,
    /// Record slots, deleted ones included (0x18).
    pub record_slots: u32,
    /// Where the first record starts (0x20).
    pub data_offset: u32,
    /// The length of a record, its prefix included (0x24).
    pub record_length: u32,
    /// The first record on the deleted-record free list, 1-based; 0 if the
    /// list is empty (0x28).
    pub free_head: u32,
    /// The number of records on the free list (0x30).
    pub free_count: u32,
    /// The encryption type (0x4C): 0 unencrypted, 2 the 40-bit RC4 variant, 3
    /// the default RC4, 5 AES-128, 6 AES-256. Read from the file on the server
    /// that opens it, so it stays out of the wire format.
    #[serde(skip)]
    pub encryption_type: u8,
    /// Feature flags, see [`flags`] (0x58).
    pub flags: u32,
    /// The number of fields (0x166).
    pub field_count: u16,
    /// The size of the header extension after the 400 bytes (0x170).
    pub extension: u32,
    /// Whether the header names the data dictionary the table belongs to
    /// (the `.add` path at 0x5C): a dictionary's ADT table. Read from the
    /// file on the server that opens it, so it stays out of the wire format.
    #[serde(skip)]
    pub dictionary: bool,
}

impl Header {
    /// The number of records: in an ADT table, deleted ones not counted
    /// (their slots are free); in a DBF table, all of them.
    pub fn record_count(&self) -> u32 {
        self.record_slots - self.free_count
    }

    /// Whether the table is encrypted.
    pub fn encrypted(&self) -> bool {
        self.flags & flags::ENCRYPTED != 0
    }

    /// Whether the field descriptors are encrypted too (an ADT table of a
    /// dictionary that encrypts its tables).
    pub fn encrypted_descriptors(&self) -> bool {
        self.flags & flags::ENCRYPTED_DESCRIPTORS != 0
    }
}

/// The structure of a table: its header and fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Structure {
    pub header: Header,
    pub fields: Vec<Field>,
}

impl Structure {
    /// Reads the structure of the table of format `format` at `path`.
    pub fn read(path: &Path, format: Format) -> Result<Structure, Error> {
        let mut file = File::open(path).map_err(Error::Io)?;
        match format {
            Format::Adt => adt::read_structure(&mut file),
            Format::Dbf(flavour) => dbf::read_structure(&mut file, flavour).map(|(s, _)| s),
        }
    }

    /// An empty record: each field's empty bytes (see
    /// [`Field::null_bytes`]) in a record of the format's prefix; the
    /// record an insert starts from.
    pub fn null_record(&self) -> Vec<u8> {
        let mut record = vec![0; self.header.record_length as usize];
        for field in &self.fields {
            let start = usize::from(field.offset);
            record[start..start + usize::from(field.size)].copy_from_slice(&field.null_bytes());
        }
        if let Format::Dbf(_) = self.header.format {
            dbf::blank_record(self, &mut record);
        }
        record
    }
}

/// Whether a record slot holds a record or a deleted one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordStatus {
    Active,
    Deleted,
}

impl RecordStatus {
    /// The status stored in a record's first byte in a table of format
    /// `format` (see [`adt::record_status`] and [`dbf::record_status`]).
    fn from_byte(byte: u8, format: Format) -> Option<RecordStatus> {
        match format {
            Format::Adt => adt::record_status(byte),
            Format::Dbf(_) => Some(dbf::record_status(byte)),
        }
    }
}

/// The records of an open table, read from its file (or the bytes of one in
/// memory). Slots are numbered from 1; deleted records keep their slots.
#[derive(Debug)]
pub struct Records {
    file: Source,
    format: Format,
    data_offset: u64,
    record_length: u32,
    slots: u32,
    /// Whether the table's data area is encrypted (an ADT `ENCRYPTED` table).
    encrypted: bool,
    /// How reads decipher each record, once the password is known; `None`
    /// leaves them as the enciphered bytes the engine returns for an encrypted
    /// table opened without it.
    cipher: RecordCipher,
    /// An AES table's write IV counter: the next 64-bit IV to hand out, shared
    /// with the memo file so records and memo values never reuse an IV. It is
    /// seeded from the header (0x198) at open and persisted back as it advances.
    aes_iv: Arc<AtomicU64>,
}

/// How an encrypted table's records (and its memo and index files) are
/// deciphered on read.
#[derive(Debug, Clone)]
pub enum RecordCipher {
    /// Not deciphered (an unencrypted table, or one opened without its
    /// password).
    None,
    /// RC4, keyed by the password: each record's fixed-length data area after
    /// the 5-byte clear prefix (see [`crate::crypto::table_record_xor`]).
    Rc4(Vec<u8>),
    /// AES-CTR, with the table key and nonce: each record carries a clear
    /// 8-byte IV right after the 5-byte prefix, and the field data follows it;
    /// the engine returns the record with the IV zeroed and the status normal.
    Aes { key: Vec<u8>, nonce: [u8; 4] },
}

/// What a DBF table laid out for AES keeps in the back link: a 1, the nonce,
/// the IV counter and the salt.
const DBF_AES_BLOCK: usize = 48;

/// Where an AES record's clear IV and field data sit, after the 5-byte prefix.
const AES_RECORD_IV: usize = crate::adt::RECORD_PREFIX;
const AES_RECORD_DATA: usize = AES_RECORD_IV + 8;

/// Whether an ADT record's status `byte` marks it enciphered (0x06 active, 0x07
/// deleted; a transaction mark aside), so its data area is (de)ciphered.
fn record_is_enciphered(byte: u8) -> bool {
    crate::adt::is_enciphered(byte)
}

impl Records {
    /// The records of the table in `file`, whose header is `header`.
    pub fn new(file: impl Into<Source>, header: &Header) -> Records {
        Records {
            file: file.into(),
            format: header.format,
            data_offset: u64::from(header.data_offset),
            record_length: header.record_length,
            slots: header.record_slots,
            encrypted: header.encrypted(),
            cipher: RecordCipher::None,
            aes_iv: Arc::new(AtomicU64::new(0)),
        }
    }

    /// How the records are deciphered: an encrypted table's memo and index
    /// files decipher the same way.
    pub fn cipher(&self) -> RecordCipher {
        self.cipher.clone()
    }

    /// Share the AES write IV counter, seeded from the header's value (0x198):
    /// the memo file takes a clone so records and memo values draw IVs from the
    /// one counter and never reuse one.
    pub(crate) fn set_aes_counter(&mut self, counter: Arc<AtomicU64>) {
        self.aes_iv = counter;
    }

    /// The next AES IV (as 8 little-endian bytes), advancing the shared counter
    /// past the `blocks` 16-byte blocks this item will encipher so no two items
    /// share an IV.
    fn next_aes_iv(&self, blocks: u32) -> [u8; 8] {
        self.aes_iv
            .fetch_add(u64::from(blocks.max(1)), Ordering::Relaxed)
            .to_le_bytes()
    }

    /// Give the records the RC4 encryption password, so reads decrypt an
    /// encrypted ADT table's data area. Has no effect on a table that is not
    /// encrypted.
    pub fn set_key(&mut self, key: Vec<u8>) {
        if self.encrypted {
            self.cipher = RecordCipher::Rc4(key);
        }
    }

    /// Give the records their cipher, so reads decrypt the table (an encrypted
    /// table opened with its password; see [`crate::adt::open_with_password`]).
    pub(crate) fn set_cipher(&mut self, cipher: RecordCipher) {
        self.cipher = cipher;
    }

    /// The number of record slots, deleted records included.
    pub fn slots(&self) -> u32 {
        self.slots
    }

    /// Reads slot `slot` (1-based) into `buf`, which must hold the record
    /// length.
    pub fn read(&mut self, slot: u32, buf: &mut [u8]) -> Result<(), Error> {
        if slot == 0 || slot > self.slots || buf.len() != self.record_length as usize {
            return Err(Error::Corrupt("no such record slot"));
        }
        let offset = self.offset(slot);
        self.file.seek(SeekFrom::Start(offset))?;
        self.file
            .read_exact(buf)
            .map_err(|_| Error::Corrupt("file ends inside a record"))?;
        self.decipher(slot, buf);
        Ok(())
    }

    /// Decipher a just-read encrypted record in place, so the rest of the
    /// engine sees the plaintext the original engine returns (see
    /// [`crate::crypto`]). A table opened without its password keeps the
    /// enciphered bytes.
    fn decipher(&self, slot: u32, buf: &mut [u8]) {
        if self.format != Format::Adt {
            // A DBF record is `E` or `D` in place of its blank or `*`, then one
            // stream at the record's number.
            // A record a transaction marked keeps its mark.
            let mark = buf.first().copied().unwrap_or(0) & crate::adt::TX_MARK;
            let clear = match buf.first().map(|byte| byte & !crate::adt::TX_MARK) {
                Some(crate::dbf::ENCRYPTED_ACTIVE) => b' ',
                Some(crate::dbf::ENCRYPTED_DELETED) => crate::dbf::DELETED,
                _ => return,
            };
            match &self.cipher {
                RecordCipher::Rc4(key) => {
                    buf[0] = clear | mark;
                    crate::crypto::table_record_xor(key, slot, &mut buf[1..]);
                }
                // An AES record keeps its IV in the last 8 bytes; the engine
                // gives it back zeroed, as an ADT record's.
                RecordCipher::Aes { key, nonce } if buf.len() > 9 => {
                    buf[0] = clear | mark;
                    let end = buf.len() - 8;
                    let iv: [u8; 8] = buf[end..].try_into().unwrap();
                    crate::crypto::aes_ctr_xor(key, &iv, nonce, &mut buf[1..end]);
                    buf[end..].fill(0);
                }
                _ => {}
            }
            return;
        }
        // Only an enciphered record (status 0x06 active, 0x07 deleted, a
        // transaction mark aside) is deciphered: a record-level encrypted table
        // mixes clear 0x04/0x05 records with enciphered ones, and a fully
        // table-encrypted one has every record enciphered, so the per-record
        // status settles it either way (see [`crate::crypto`]).
        if !buf.first().is_some_and(|&s| record_is_enciphered(s)) {
            return;
        }
        match &self.cipher {
            RecordCipher::None => {}
            // RC4: the 5-byte prefix stays clear, the data area is deciphered.
            RecordCipher::Rc4(key) => {
                if buf.len() > crate::adt::RECORD_PREFIX {
                    crate::crypto::table_record_xor(
                        key,
                        slot,
                        &mut buf[crate::adt::RECORD_PREFIX..],
                    );
                }
            }
            // AES: a clear 8-byte IV sits between the prefix and the field data.
            // Decipher the field data, then present the record as the engine
            // does: the IV zeroed and the status byte back to its clear value.
            RecordCipher::Aes { key, nonce } => {
                if buf.len() > AES_RECORD_DATA {
                    let iv: [u8; 8] = buf[AES_RECORD_IV..AES_RECORD_DATA].try_into().unwrap();
                    crate::crypto::aes_ctr_xor(key, &iv, nonce, &mut buf[AES_RECORD_DATA..]);
                    buf[AES_RECORD_IV..AES_RECORD_DATA].fill(0);
                    buf[0] = crate::adt::clear_status(buf[0]).unwrap_or(buf[0]);
                }
            }
        }
    }

    /// Where slot `slot` (1-based) starts in the file.
    fn offset(&self, slot: u32) -> u64 {
        self.data_offset + u64::from(slot.saturating_sub(1)) * u64::from(self.record_length)
    }

    /// Writes `record` to slot `slot` (1-based).
    fn write(&mut self, slot: u32, record: &[u8]) -> Result<(), Error> {
        if slot == 0 || slot > self.slots || record.len() != self.record_length as usize {
            return Err(Error::Corrupt("no such record slot"));
        }
        Ok(self.file.write_at(self.offset(slot), record)?)
    }

    /// The status of slot `slot` (1-based).
    pub fn status(&mut self, slot: u32) -> Result<RecordStatus, Error> {
        let byte = self.status_byte(slot)?;
        RecordStatus::from_byte(byte, self.format).ok_or(Error::Corrupt("unknown record status"))
    }

    /// The first byte of slot `slot`: an ADT record's status, a DBF
    /// record's deletion flag.
    pub fn status_byte(&mut self, slot: u32) -> Result<u8, Error> {
        if slot == 0 || slot > self.slots {
            return Err(Error::Corrupt("no such record slot"));
        }
        let offset = self.data_offset + u64::from(slot - 1) * u64::from(self.record_length);
        let mut byte = [0];
        self.file.seek(SeekFrom::Start(offset))?;
        self.file
            .read_exact(&mut byte)
            .map_err(|_| Error::Corrupt("file ends inside a record"))?;
        Ok(byte[0])
    }
}

/// An open table of any format: its structure, its records, its memo file,
/// and for a DBF table what its header says besides its structure.
///
/// An ADT table opened for writing also takes records
/// ([`TableFile::insert`], [`TableFile::write`], [`TableFile::delete`]).
/// Deleted records form the free list the original engine keeps: a LIFO
/// stack whose head (1-based, 0 for none) and length are in the header,
/// linked through the four bytes after each deleted record's status byte;
/// an insert takes the head's slot, and adds a slot only when the list is
/// empty. Writes change only the header fields they concern, so the bytes
/// tflip does not interpret stay as they are.
#[derive(Debug)]
pub struct TableFile {
    pub structure: Structure,
    records: Records,
    /// The memo file, for tables with memo fields.
    memo: Option<MemoFile>,
    dbf: Option<dbf::Info>,
    /// Whether the file was opened for writing.
    write: bool,
    /// How many times a record was written, appended or deleted through
    /// this file (see [`TableFile::changes`]).
    changes: u64,
    /// Whether an insert goes after the last slot and leaves the free list
    /// as it is (see [`TableFile::append_only`]).
    append_only: bool,
    /// Whether a DBF table's end-of-file byte is to be written when it
    /// closes (see [`TableFile::defer_end_of_file`]).
    end_deferred: bool,
    /// The ciphers of the records and the memo file while the file is
    /// sealed ([`TableFile::seal`]), to put back when it is unsealed.
    sealed: Option<(RecordCipher, Option<RecordCipher>)>,
}

impl TableFile {
    /// Opens the table of format `format` at `path` to read: reads its
    /// structure and keeps the table file, and its memo file if it has one,
    /// open.
    pub fn open(path: &Path, format: Format) -> Result<TableFile, Error> {
        match format {
            Format::Adt => adt::open(path, false),
            Format::Dbf(flavour) => dbf::open(path, flavour, false),
        }
    }

    /// Opens the table, deciphering an encrypted one with `password` (see
    /// [`adt::open_with_password`]); for writing if `write`. DBF tables and
    /// unencrypted tables ignore the password.
    pub fn open_deciphering(
        path: &Path,
        format: Format,
        write: bool,
        password: Option<&[u8]>,
    ) -> Result<TableFile, Error> {
        match format {
            Format::Adt => adt::open_with_password(path, write, password),
            Format::Dbf(flavour) => {
                let mut file = dbf::open(path, flavour, write)?;
                if let Some(password) = password.filter(|_| file.has_password()) {
                    file.supply_password(password)?;
                }
                Ok(file)
            }
        }
    }

    /// Opens the data dictionary at `path` as an ADT table, to read its
    /// objects as records (to build the dictionary's own index). Its records
    /// and fields are an ordinary ADT table's; see [`crate::dd`] for reading
    /// it as a dictionary. An encrypted one reads deciphered: an RC4 one under
    /// the `AtGdQ` key, an AES one under the key a connection of the process
    /// opened it with ([`Error::Encrypted`] if none did).
    pub fn open_dictionary(path: &Path) -> Result<TableFile, Error> {
        let mut file = adt::open_signed(path, false, crate::dd::SIGNATURE, false)?;
        if file.structure.header.encrypted() {
            match file.structure.header.encryption_type {
                adt::ENCRYPTION_AES128 | adt::ENCRYPTION_AES256 => {
                    let (cipher, counter) =
                        crate::dd::keyring::reserve(path)?.ok_or(Error::Encrypted)?;
                    file.arm(cipher, counter);
                }
                _ => file
                    .records
                    .set_cipher(RecordCipher::Rc4(crate::crypto::OBFUSCATION_KEY.to_vec())),
            }
        }
        Ok(file)
    }

    /// Opens the data dictionary at `path` as an ADT table for writing, to
    /// delete or modify its object records in place (see [`TableFile::delete`]
    /// and [`TableFile::write`]). Its memo file (`.am`) is left closed, so a
    /// record's comment memo is not freed on delete; the caller handles the
    /// memo if it must. Its records are as they are in the file; see
    /// [`crate::dd::DictionaryFile`] for writing them as a dictionary's.
    pub fn open_dictionary_for_writing(path: &Path) -> Result<TableFile, Error> {
        adt::open_signed(path, true, crate::dd::SIGNATURE, false)
    }

    /// Opens the table of format `format` at `path` to read and write (a DBF
    /// table that cannot be written opens to read, and refuses writes).
    pub fn open_for_writing(path: &Path, format: Format) -> Result<TableFile, Error> {
        match format {
            Format::Adt => adt::open(path, true),
            Format::Dbf(flavour) => dbf::open(path, flavour, true),
        }
    }

    /// A table from its parts, to read.
    pub(crate) fn new(
        structure: Structure,
        records: Records,
        memo: Option<MemoFile>,
        dbf: Option<dbf::Info>,
    ) -> TableFile {
        TableFile {
            structure,
            records,
            memo,
            dbf,
            write: false,
            changes: 0,
            append_only: false,
            end_deferred: false,
            sealed: None,
        }
    }

    /// How many times a record was written, appended or deleted through this
    /// file: what the process changed of it (not what other processes did),
    /// for what is kept of its records to know when it is out of date.
    pub fn changes(&self) -> u64 {
        self.changes
    }

    /// The table, to be written if `write`.
    pub(crate) fn writable(mut self, write: bool) -> TableFile {
        self.write = write;
        self
    }

    /// Why the table cannot take writes, if it cannot.
    pub fn check_writable(&self) -> Result<(), Error> {
        match self.records.cipher() {
            // An RC4- or AES-encrypted table is writable once its password is
            // known — writes encipher (see [`TableFile::write`]).
            RecordCipher::None | RecordCipher::Rc4(_) | RecordCipher::Aes { .. } => {}
        }
        // An encrypted table without its password (no cipher) cannot be written.
        if self.structure.header.encrypted() && matches!(self.records.cipher(), RecordCipher::None)
        {
            return Err(Error::NotWritable);
        }
        if !self.write {
            return Err(Error::ReadOnly);
        }
        Ok(())
    }

    /// What the header of a DBF table says besides its structure.
    pub fn dbf(&self) -> Option<&dbf::Info> {
        self.dbf.as_ref()
    }

    /// The memo file's block size, if the table has one.
    pub fn memo_block_size(&self) -> Option<u32> {
        self.memo.as_ref().map(MemoFile::block_size)
    }

    /// The memo file, if the table has one, to read memo fields from (see
    /// [`Field::value`]).
    pub fn memo(&mut self) -> Option<&mut MemoFile> {
        self.memo.as_mut()
    }

    /// The number of record slots, deleted records included.
    pub fn slots(&self) -> u32 {
        self.records.slots()
    }

    /// The status of slot `slot` (1-based).
    pub fn status(&mut self, slot: u32) -> Result<RecordStatus, Error> {
        self.records.status(slot)
    }

    /// The raw first byte of slot `slot` (1-based): an ADT record's status
    /// (0x06/0x07 when enciphered), a DBF record's deletion flag.
    pub fn status_byte(&mut self, slot: u32) -> Result<u8, Error> {
        self.records.status_byte(slot)
    }

    /// Whether a transaction has marked the record in slot `slot` (see
    /// [`TableFile::marked`]), and whether it appended it.
    pub fn marked_at(&mut self, slot: u32) -> Result<(bool, bool), Error> {
        let byte = self.records.status_byte(slot)?;
        let appended = byte == self.appended_mark();
        Ok((appended || byte & crate::adt::TX_MARK != 0, appended))
    }

    /// Give the table its encryption password, so reads of an encrypted ADT
    /// table return the decrypted records (see [`Records::set_key`]).
    pub fn set_encryption_key(&mut self, key: Vec<u8>) {
        self.records.set_key(key);
    }

    /// How the table is deciphered, if it is encrypted (see [`Records::cipher`]);
    /// its memo and index files decipher the same way.
    pub fn encryption_cipher(&self) -> Option<RecordCipher> {
        match self.records.cipher() {
            RecordCipher::None => None,
            cipher => Some(cipher),
        }
    }

    /// The IV counter of an AES table, which its records, memo values and
    /// index pages draw from.
    pub fn aes_counter(&self) -> Option<Arc<AtomicU64>> {
        match self.records.cipher() {
            RecordCipher::Aes { .. } => Some(Arc::clone(&self.records.aes_iv)),
            _ => None,
        }
    }

    /// Reads slot `slot` (1-based) into `buf`, which must hold the record
    /// length.
    pub fn read(&mut self, slot: u32, buf: &mut [u8]) -> Result<(), Error> {
        self.records.read(slot, buf)?;
        self.seal_record(buf);
        Ok(())
    }

    /// Reads slot `slot` (1-based) as it is in the file, whatever a handle
    /// without the password is shown of it (a memo field's place zeroed, see
    /// [`TableFile::seal`]).
    pub fn read_stored(&mut self, slot: u32, buf: &mut [u8]) -> Result<(), Error> {
        self.records.read(slot, buf)
    }

    /// The bytes of slot `slot` (1-based).
    pub fn record(&mut self, slot: u32) -> Result<Vec<u8>, Error> {
        let mut bytes = vec![0; self.structure.header.record_length as usize];
        self.read(slot, &mut bytes)?;
        Ok(bytes)
    }

    /// Whether the table has an encryption password: its header holds the
    /// password check (an RC4 table `AdsEnableEncryption` was called on,
    /// encrypted or not yet) or is encrypted (AES).
    pub fn has_password(&self) -> bool {
        let header = &self.structure.header;
        if let Some(info) = &self.dbf {
            return info.password_check[0] != 0 || header.encrypted();
        }
        header.format == Format::Adt
            && (header.encrypted()
                || matches!(
                    header.encryption_type,
                    adt::ENCRYPTION_RC4 | adt::ENCRYPTION_RC4_40
                ))
    }

    /// Whether the table is AES-encrypted.
    pub fn is_aes(&self) -> bool {
        self.dbf_aes_armed()
            || matches!(
                self.structure.header.encryption_type,
                adt::ENCRYPTION_AES128 | adt::ENCRYPTION_AES256
            )
    }

    /// Whether the record `bytes` is enciphered (an ADT record of status
    /// 0x06 or 0x07), as it is in the file.
    pub fn enciphered(&self, bytes: &[u8]) -> bool {
        let status = bytes.first().copied();
        match self.structure.header.format {
            Format::Adt => status.is_some_and(record_is_enciphered),
            Format::Dbf(_) => matches!(
                status.map(|byte| byte & !adt::TX_MARK),
                Some(crate::dbf::ENCRYPTED_ACTIVE | crate::dbf::ENCRYPTED_DELETED)
            ),
        }
    }

    /// Supplies the table's encryption password, as opening it with the
    /// password or `AdsEnableEncryption` on it does: checked against the
    /// header's password check (RC4) or verifier (AES), and the records and
    /// the memo file then deciphered with it if they were not yet. Whether
    /// the table has a password to supply ([`TableFile::has_password`]); a
    /// wrong one is [`Error::Encrypted`].
    pub fn supply_password(&mut self, password: &[u8]) -> Result<bool, Error> {
        if !self.has_password() {
            return Ok(false);
        }
        if self.dbf_aes_armed() {
            return self.supply_dbf_aes_password(password).map(|()| true);
        }
        if self.is_aes() {
            // Checked (and armed, if it is not) from the crypto block on disk.
            if let RecordCipher::Aes { key, .. } = self.records.cipher() {
                let derived = self.aes_derived(password)?;
                if derived.0 != key {
                    return Err(Error::Encrypted);
                }
                return Ok(true);
            }
            self.supply_aes_password(password)?;
            return Ok(true);
        }
        if let Some(info) = &self.dbf {
            if dbf::aes_type(info.password_check[0]) {
                return self.supply_dbf_aes_password(password).map(|()| true);
            }
            let key = &password[..password.len().min(crate::crypto::KEY_MAX)];
            if !crate::crypto::dbf_password_ok(key, &info.password_check) {
                return Err(Error::Encrypted);
            }
            if matches!(self.records.cipher(), RecordCipher::None) {
                let cipher = RecordCipher::Rc4(key.to_vec());
                self.records.set_cipher(cipher.clone());
                if self.structure.header.encrypted()
                    && let Some(memo) = self.memo.as_mut()
                {
                    memo.set_cipher(cipher);
                }
            }
            return Ok(true);
        }
        let key = match self.structure.header.encryption_type {
            adt::ENCRYPTION_RC4_40 => &password[..password.len().min(5)],
            _ => &password[..password.len().min(crate::crypto::KEY_MAX)],
        };
        let mut check = [0u8; 20];
        self.records
            .file
            .read_at(offsets::PASSWORD_CHECK, &mut check)?;
        if !crate::crypto::rc4_password_ok(key, &check) {
            return Err(Error::Encrypted);
        }
        if matches!(self.records.cipher(), RecordCipher::None) {
            let cipher = RecordCipher::Rc4(key.to_vec());
            self.records.set_cipher(cipher.clone());
            // Record-level encryption leaves the memo values in the clear.
            if self.structure.header.encrypted()
                && let Some(memo) = self.memo.as_mut()
            {
                memo.set_cipher(cipher);
            }
        }
        Ok(true)
    }

    /// Supplies the password of a DBF table laid out and encrypted for AES:
    /// the key is PBKDF2 of it over the salt in the crypto block of the
    /// header's back link, and the header holds the first 9 bytes of the
    /// verifier after the type (`fixtures/aes/rc4tp`, `v1`).
    fn supply_dbf_aes_password(&mut self, password: &[u8]) -> Result<(), Error> {
        let check = self.dbf.as_ref().map(|info| info.password_check).unwrap();
        let key_len = if check[0] == 5 { 16 } else { 32 };
        let mut block = [0u8; DBF_AES_BLOCK];
        let at = u64::from(self.structure.header.data_offset) - dbf::BACKLINK as u64;
        self.records.file.read_at(at, &mut block)?;
        let derived = crate::crypto::aes_derive(password, &block[16..48], key_len);
        if derived[key_len..key_len + 9] != check[1..] {
            return Err(Error::Encrypted);
        }
        if let RecordCipher::Aes { key, .. } = self.records.cipher() {
            return match key == derived[..key_len] {
                true => Ok(()),
                false => Err(Error::Encrypted),
            };
        }
        let nonce: [u8; 4] = block[4..8].try_into().unwrap();
        let counter = Arc::new(AtomicU64::new(u64::from_le_bytes(
            block[8..16].try_into().unwrap(),
        )));
        let cipher = RecordCipher::Aes {
            key: derived[..key_len].to_vec(),
            nonce,
        };
        self.records.set_cipher(cipher.clone());
        self.records.set_aes_counter(Arc::clone(&counter));
        if let Some(memo) = self.memo.as_mut() {
            memo.set_cipher(cipher);
            memo.set_aes_counter(counter);
        }
        Ok(())
    }

    /// An AES table's key and verifier for `password`, from the salt on disk.
    fn aes_derived(&mut self, password: &[u8]) -> Result<(Vec<u8>, Vec<u8>), Error> {
        let key_len = match self.structure.header.encryption_type {
            adt::ENCRYPTION_AES256 => 32,
            _ => 16,
        };
        let mut salt = [0u8; 32];
        self.records.file.read_at(0x1A0, &mut salt)?;
        let derived = crate::crypto::aes_derive(password, &salt, key_len);
        Ok((
            derived[..key_len].to_vec(),
            derived[key_len..key_len + 20].to_vec(),
        ))
    }

    /// Seals the file for a handle that has not given the table's password:
    /// until [`TableFile::unseal`], records and memo values are read and
    /// written as they are in the file, not deciphered or enciphered, and an
    /// enciphered record reads as the original engine gives it to such a
    /// handle (see [`TableFile::read`]): an RC4 one as its enciphered bytes,
    /// an AES one blank, and the memo values of either empty. Nothing
    /// changes for a table that is not encrypted.
    pub fn seal(&mut self) {
        if self.sealed.is_none() {
            let records = std::mem::replace(&mut self.records.cipher, RecordCipher::None);
            let memo = self.memo.as_mut().map(MemoFile::take_cipher);
            self.sealed = Some((records, memo));
        }
    }

    /// Runs `f` on the file unsealed, for one that is sealed (see
    /// [`TableFile::seal`]): to give it its password, or to encrypt it.
    pub fn unsealed<R>(&mut self, f: impl FnOnce(&mut TableFile) -> R) -> R {
        let sealed = self.sealed.is_some();
        self.unseal();
        let result = f(self);
        if sealed {
            self.seal();
        }
        result
    }

    /// Ends what [`TableFile::seal`] began: the ciphers back.
    pub fn unseal(&mut self) {
        if let Some((records, memo)) = self.sealed.take() {
            self.records.cipher = records;
            if let (Some(file), Some(cipher)) = (self.memo.as_mut(), memo) {
                file.set_cipher(cipher);
            }
        }
    }

    /// A record just read, as a sealed file gives it ([`TableFile::seal`]).
    fn seal_record(&self, buf: &mut [u8]) {
        if self.sealed.is_none() || !self.enciphered(buf) {
            return;
        }
        if self.dbf_aes_armed() {
            // A VFP record reads as a blank one, active or deleted as it is
            // (`fixtures/write/dbf_aes_nopassword`).
            let status = match buf[0] & !crate::adt::TX_MARK {
                crate::dbf::ENCRYPTED_DELETED => crate::dbf::DELETED,
                _ => b' ',
            } | (buf[0] & crate::adt::TX_MARK);
            buf.copy_from_slice(&self.structure.null_record());
            buf[0] = status;
            return;
        }
        if self.is_aes() && buf.len() >= AES_RECORD_DATA {
            // As the original engine gives an AES record to a handle without
            // the password (`fixtures/write/encryption/share.json`): its text
            // blank, the rest zero, the record active (or deleted) and not
            // enciphered.
            buf[0] = match crate::adt::record_status(buf[0]) {
                Some(RecordStatus::Deleted) => crate::adt::DELETED,
                _ => crate::adt::ACTIVE,
            } | (buf[0] & crate::adt::TX_MARK);
            buf[AES_RECORD_IV..].fill(0);
            // The kinds of field that are blank in the original's record
            // (`fixtures/aes/api/01_types`): text, Money, Logical and
            // Numeric ones as spaces, and the length of a variable-length
            // value as -1.
            for field in &self.structure.fields {
                let start = usize::from(field.offset);
                let end = start + usize::from(field.size);
                match field.field_type {
                    FieldType::Character
                    | FieldType::CiCharacter
                    | FieldType::LegacyVarChar
                    | FieldType::Money
                    | FieldType::Logical
                    | FieldType::Numeric => buf[start..end].fill(b' '),
                    FieldType::VarChar | FieldType::VarBinary | FieldType::NVarChar => {
                        buf[end - 2..end].fill(0xFF)
                    }
                    _ => {}
                }
            }
        }
        // A memo value's reference is enciphered with the record: the value
        // reads as none. (A DBF table says why: the engine refuses it.)
        if self.structure.header.format != Format::Adt {
            return;
        }
        for field in self.structure.fields.iter().filter(|field| field.is_memo()) {
            let start = usize::from(field.offset);
            buf[start..start + usize::from(field.size)].fill(0);
        }
    }

    /// The file, for locks on it; `None` for a table in memory.
    fn os_file(&self) -> Option<&File> {
        match &self.records.file {
            Source::File(file) => Some(file),
            Source::Memory(_) => None,
        }
    }

    /// Takes a lock on this open file that other opens of it (in other
    /// processes, or other opens in this one) see; granted at once for a
    /// table in memory. See [`crate::lock`].
    fn os_lock(
        &self,
        offset: u64,
        length: u64,
        exclusive: bool,
        wait: bool,
    ) -> Result<bool, Error> {
        match self.os_file() {
            Some(file) => Ok(lock::lock(file, offset, length, exclusive, wait)?),
            None => Ok(true),
        }
    }

    fn os_unlock(&self, offset: u64, length: u64) -> Result<(), Error> {
        match self.os_file() {
            Some(file) => Ok(lock::unlock(file, offset, length)?),
            None => Ok(()),
        }
    }

    /// Locks record slot `slot` against other opens of the file; whether it
    /// was granted. A lock this open holds already is granted again on Unix
    /// but not on Windows, so callers keep track of their own.
    pub fn lock_record(&self, slot: u32) -> Result<bool, Error> {
        self.os_lock(lock::record(slot), 1, true, false)
    }

    /// Releases this open's lock on record slot `slot`.
    pub fn unlock_record(&self, slot: u32) -> Result<(), Error> {
        self.os_unlock(lock::record(slot), 1)
    }

    /// Locks every record against other opens of the file; whether it was
    /// granted. This open's own record locks must be released first.
    pub fn lock_table(&self) -> Result<bool, Error> {
        self.os_lock(lock::RECORDS, lock::TABLE_LENGTH, true, false)
    }

    /// Releases this open's table lock.
    pub fn unlock_table(&self) -> Result<(), Error> {
        self.os_unlock(lock::RECORDS, lock::TABLE_LENGTH)
    }

    /// Marks the file as open, exclusively or shared, against other opens;
    /// whether that was granted (no exclusive open is held, nor any open
    /// when it is to be exclusive).
    pub fn lock_open(&self, exclusive: bool) -> Result<bool, Error> {
        self.os_lock(lock::OPEN, 1, exclusive, false)
    }

    /// Releases the mark of [`TableFile::lock_open`].
    pub fn unlock_open(&self) -> Result<(), Error> {
        self.os_unlock(lock::OPEN, 1)
    }

    /// Waits until no other open of the file writes, keeps others from
    /// writing until [`TableFile::end_write`], and reads again what others
    /// may have changed in the header: the record slots and the free list.
    pub fn begin_write(&mut self) -> Result<(), Error> {
        self.check_writable()?;
        self.os_lock(lock::WRITE, 1, true, true)?;
        self.refresh()
    }

    /// Ends what [`TableFile::begin_write`] began.
    pub fn end_write(&mut self) -> Result<(), Error> {
        self.os_unlock(lock::WRITE, 1)
    }

    /// Reads again the header fields other opens of the file change: the
    /// record slots and the free list (in a DBF table, the record count).
    pub fn refresh(&mut self) -> Result<(), Error> {
        if self.os_file().is_none() {
            return Ok(());
        }
        if let Format::Dbf(_) = self.structure.header.format {
            let count = self.header_u32(dbf::RECORD_COUNT)?;
            self.structure.header.record_slots = count;
            self.records.slots = count;
            return Ok(());
        }
        let slots = self.header_u32(offsets::RECORD_SLOTS)?;
        let free_head = self.header_u32(offsets::FREE_HEAD)?;
        let free_count = self.header_u32(offsets::FREE_COUNT)?;
        let header = &mut self.structure.header;
        header.record_slots = slots;
        header.free_head = free_head;
        header.free_count = free_count;
        self.records.slots = slots;
        Ok(())
    }

    /// A VFP AutoInc field's next value and step, as its descriptor keeps
    /// them; `None` in other tables.
    pub fn autoinc_state(&mut self, field: &Field) -> Result<Option<(u32, u8)>, Error> {
        let Format::Dbf(dbf::Flavour::Visual) = self.structure.header.format else {
            return Ok(None);
        };
        let Some(index) = self
            .structure
            .fields
            .iter()
            .position(|f| f.offset == field.offset)
        else {
            return Ok(None);
        };
        let mut bytes = [0; 5];
        self.records
            .file
            .read_at(dbf::descriptor(index) + 19, &mut bytes)?;
        Ok(Some((
            u32::from_le_bytes(bytes[..4].try_into().unwrap()),
            bytes[4],
        )))
    }

    fn header_u32(&mut self, offset: u64) -> Result<u32, Error> {
        let mut bytes = [0; 4];
        self.records.file.read_at(offset, &mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn set_header_u32(&mut self, offset: u64, value: u32) -> Result<(), Error> {
        Ok(self.records.file.write_at(offset, &value.to_le_bytes())?)
    }

    /// Sets the header flag [`adt::UNPREPARED`] of an ADT table that
    /// `PrepareDiff` or a `Diff` cleared it in: the first write of any kind
    /// after that sets it again and a second changes nothing
    /// (`fixtures/backup/s56_diff_writes`). A write a transaction only marks
    /// does not count until it is committed (`s57_diff_tx`).
    fn note_written(&mut self) -> Result<(), Error> {
        let flags = self.structure.header.flags;
        if self.structure.header.format != Format::Adt || flags & adt::UNPREPARED != 0 {
            return Ok(());
        }
        self.structure.header.flags = flags | adt::UNPREPARED;
        self.set_header_u32(offsets::FLAGS, flags | adt::UNPREPARED)
    }

    /// A rewrite of the whole ADT table (pack, encrypt): it counts as
    /// written, and its stamp goes so that the next `Diff` writes the table to
    /// the image in full (`fixtures/backup/s58_diff_reinit_hdr`).
    fn note_rewritten(&mut self) -> Result<(), Error> {
        if self.structure.header.format != Format::Adt {
            return Ok(());
        }
        self.note_written()?;
        self.set_header_u32(offsets::BACKUP_STAMP, 0)
    }

    /// An ADT table's differential backup stamp (header 0x174): the Unix
    /// time `PrepareDiff` or the last `Diff` gave it, 0 for none.
    pub fn backup_stamp(&mut self) -> Result<u32, Error> {
        if self.structure.header.format != Format::Adt {
            return Err(Error::NotWritable);
        }
        let mut bytes = [0; 4];
        self.records
            .file
            .seek(SeekFrom::Start(offsets::BACKUP_STAMP))?;
        self.records
            .file
            .read_exact(&mut bytes)
            .map_err(|_| Error::Corrupt("file ends inside the header"))?;
        Ok(u32::from_le_bytes(bytes))
    }

    /// The slots of an ADT table's records, deleted ones too, written since
    /// the changed bits were last cleared (status bit 0x04: every write,
    /// delete and recall sets it), in slot order. This is what a
    /// differential backup copies.
    pub fn changed_slots(&mut self) -> Result<Vec<u32>, Error> {
        if self.structure.header.format != Format::Adt {
            return Err(Error::NotWritable);
        }
        let mut slots = Vec::new();
        for slot in 1..=self.records.slots {
            if adt::is_changed(self.records.status_byte(slot)?) {
                slots.push(slot);
            }
        }
        Ok(slots)
    }

    /// `PrepareDiff`'s change to the source table, and what each `Diff` does
    /// after it copied the changed records (`fixtures/backup/s50_diff`):
    /// clears the changed bit of every record's status byte (0x04/0x05
    /// become 0x00/0x01, 0x06/0x07 become 0x02/0x03), clears the header flag
    /// [`adt::UNPREPARED`] and stores `stamp` at header 0x174. A record a
    /// transaction marked keeps its byte.
    pub fn clear_changed(&mut self, stamp: u32) -> Result<(), Error> {
        self.check_writable()?;
        if self.structure.header.format != Format::Adt {
            return Err(Error::NotWritable);
        }
        for slot in 1..=self.records.slots {
            let byte = self.records.status_byte(slot)?;
            if adt::is_changed(byte) {
                self.records
                    .file
                    .write_at(self.records.offset(slot), &[byte & !adt::CHANGED])?;
            }
        }
        let flags = self.structure.header.flags & !adt::UNPREPARED;
        self.structure.header.flags = flags;
        self.set_header_u32(offsets::FLAGS, flags)?;
        self.set_header_u32(offsets::BACKUP_STAMP, stamp)
    }

    /// Gives an ADT table the backup stamp `stamp` and leaves its records and
    /// header flags as they are: what a `Diff` does to a table that nothing
    /// was written to since the last one.
    pub fn set_backup_stamp(&mut self, stamp: u32) -> Result<(), Error> {
        self.check_writable()?;
        if self.structure.header.format != Format::Adt {
            return Err(Error::NotWritable);
        }
        self.set_header_u32(offsets::BACKUP_STAMP, stamp)
    }

    /// Whether an ADT table was written since the changed bits were last
    /// cleared (header flag [`adt::UNPREPARED`]), which is what tells a
    /// `Diff` whether there is anything to copy.
    pub fn written_since_backup(&self) -> bool {
        self.structure.header.format == Format::Adt
            && self.structure.header.flags & adt::UNPREPARED != 0
    }

    /// The next AutoInc value of `field`, which the table then counts as
    /// given out: in an ADT table the header's, in a VFP table the field
    /// descriptor's, which then grows by the descriptor's step (an unsigned
    /// byte).
    pub fn take_autoinc(&mut self, field: &Field) -> Result<u32, Error> {
        self.check_writable()?;
        if let Format::Dbf(_) = self.structure.header.format {
            let index = self
                .structure
                .fields
                .iter()
                .position(|f| f.offset == field.offset)
                .ok_or(Error::Corrupt("no such field"))?;
            let at = dbf::descriptor(index) + 19;
            let mut bytes = [0; 5];
            self.records.file.read_at(at, &mut bytes)?;
            let next = u32::from_le_bytes(bytes[..4].try_into().unwrap());
            let step = u32::from(bytes[4]);
            self.records
                .file
                .write_at(at, &next.wrapping_add(step).to_le_bytes())?;
            return Ok(next);
        }
        let next = self.header_u32(offsets::NEXT_AUTOINC)?;
        self.set_header_u32(offsets::NEXT_AUTOINC, next.wrapping_add(1))?;
        Ok(next)
    }

    /// Marks a DBF table's header as having a structural `.cdx` file (bit
    /// 0x01 of byte 0x1C).
    pub fn mark_structural_index(&mut self) -> Result<(), Error> {
        self.set_table_flags(|flags| flags | 0x01)
    }

    /// Marks a DBF table's header as having no structural `.cdx` file.
    pub fn unmark_structural_index(&mut self) -> Result<(), Error> {
        self.set_table_flags(|flags| flags & !0x01)
    }

    /// Changes a DBF table's header flags (byte 0x1C) with `change`, in the
    /// file and in what [`TableFile::dbf`] reports, so a later open sharing
    /// this one sees them.
    fn set_table_flags(&mut self, change: impl Fn(u8) -> u8) -> Result<(), Error> {
        self.check_writable()?;
        let mut flags = [0];
        self.records.file.read_at(0x1C, &mut flags)?;
        let flags = change(flags[0]);
        self.records.file.write_at(0x1C, &[flags])?;
        if let Some(info) = &mut self.dbf {
            info.table_flags = flags;
        }
        Ok(())
    }

    /// Sets the next AutoInc value of an ADT table's header to `next`.
    pub fn set_next_autoinc(&mut self, next: u32) -> Result<(), Error> {
        self.check_writable()?;
        self.set_header_u32(offsets::NEXT_AUTOINC, next)
    }

    /// Raises an ADT table's next AutoInc value to at least `floor`, as the
    /// engine keeps the next value past one a client set itself with AutoInc
    /// enforcement off (`AdsDisableAutoIncEnforcement`). A DBF table, whose
    /// counter lives in the field descriptor, is left unchanged.
    pub fn raise_next_autoinc(&mut self, floor: u32) -> Result<(), Error> {
        if let Format::Dbf(_) = self.structure.header.format {
            return Ok(());
        }
        self.check_writable()?;
        if floor > self.header_u32(offsets::NEXT_AUTOINC)? {
            self.set_header_u32(offsets::NEXT_AUTOINC, floor)?;
        }
        Ok(())
    }

    /// The next RowVersion value: the header's last one plus 1, which it then
    /// keeps.
    pub fn take_row_version(&mut self) -> Result<u64, Error> {
        self.check_writable()?;
        let mut bytes = [0; 8];
        self.records
            .file
            .read_at(offsets::ROW_VERSION, &mut bytes)?;
        let next = u64::from_le_bytes(bytes).wrapping_add(1);
        self.records
            .file
            .write_at(offsets::ROW_VERSION, &next.to_le_bytes())?;
        Ok(next)
    }

    /// Stores `record` (a whole record; an ADT record's prefix is set here)
    /// in a new slot: the head of the free list, or one after the last; in
    /// a DBF table always one after the last, followed by the end-of-file
    /// mark, with the header counting it. Returns the slot.
    pub fn insert(&mut self, record: &[u8]) -> Result<u32, Error> {
        self.insert_as(record, false)
    }

    /// [`TableFile::insert`] with `record`'s bytes as they are, an ADT
    /// record's prefix too (a record a transaction appends, marked).
    pub fn insert_raw(&mut self, record: &[u8]) -> Result<u32, Error> {
        self.insert_as(record, true)
    }

    /// Makes the inserts that follow go after the last slot, whatever the
    /// free list holds, which they leave alone: what an online restructure
    /// does to the table meanwhile, which replays the connections' changes
    /// on the new table in the order they were made.
    pub fn append_only(&mut self, append_only: bool) {
        self.append_only = append_only;
    }

    /// The slot [`TableFile::insert`] stores the next record in.
    pub fn next_slot(&mut self) -> u32 {
        match self.usable_free_head() {
            0 => self.structure.header.record_slots + 1,
            head => head,
        }
    }

    /// The head of an ADT table's free list, or 0 if there is none or its
    /// record is not deleted: a record recalled one at a time stays in the
    /// list's links, and an append the original makes past the last slot
    /// where it finds one (`fixtures/write/append_reuse`).
    fn usable_free_head(&mut self) -> u32 {
        let header = &self.structure.header;
        let (format, head) = (header.format, header.free_head);
        if format != Format::Adt || head == 0 || head > self.records.slots || self.append_only {
            return 0;
        }
        match self.records.status_byte(head) {
            Ok(status) if adt::record_status(status) == Some(RecordStatus::Deleted) => head,
            _ => 0,
        }
    }

    fn insert_as(&mut self, record: &[u8], raw: bool) -> Result<u32, Error> {
        self.check_writable()?;
        let store = |file: &mut TableFile, slot: u32| match raw {
            true => file.write_raw(slot, record),
            false => file.write(slot, record),
        };
        if let Format::Dbf(_) = self.structure.header.format {
            let slot = self.structure.header.record_slots + 1;
            self.records.slots = slot;
            self.structure.header.record_slots = slot;
            store(self, slot)?;
            let end = self.records.offset(slot) + u64::from(self.records.record_length);
            self.records.file.write_at(end, &[dbf::END_OF_FILE])?;
            self.end_deferred = false;
            return self.set_header_u32(dbf::RECORD_COUNT, slot).map(|()| slot);
        }
        let slot = match self.usable_free_head() {
            0 => {
                // The list the stale head leaves is dropped, its count too.
                if self.structure.header.free_head != 0 && !self.append_only {
                    self.structure.header.free_head = 0;
                    self.structure.header.free_count = 0;
                    self.set_header_u32(offsets::FREE_HEAD, 0)?;
                    self.set_header_u32(offsets::FREE_COUNT, 0)?;
                }
                let slot = self.structure.header.record_slots + 1;
                self.set_slots(slot)?;
                slot
            }
            head => {
                let mut next = [0; 4];
                self.records
                    .file
                    .read_at(self.records.offset(head) + 1, &mut next)?;
                let header = &mut self.structure.header;
                header.free_head = u32::from_le_bytes(next);
                header.free_count = header.free_count.saturating_sub(1);
                let (next, count) = (header.free_head, header.free_count);
                self.set_header_u32(offsets::FREE_HEAD, next)?;
                self.set_header_u32(offsets::FREE_COUNT, count)?;
                head
            }
        };
        store(self, slot)?;
        Ok(slot)
    }

    /// Sets the date of the last update in a DBF table's header to `date`
    /// (years since 1900, in VFP tables too, as the original engine writes
    /// it: `fixtures/write/sql_dbf`); nothing in an ADT table.
    pub fn set_updated(&mut self, date: crate::value::Ymd) -> Result<(), Error> {
        self.check_writable()?;
        let Format::Dbf(_) = self.structure.header.format else {
            return Ok(());
        };
        let year = date.year - 1900;
        let bytes = [year as u8, date.month as u8, date.day as u8];
        Ok(self.records.file.write_at(1, &bytes)?)
    }

    /// Packs an ADT table, as the original engine does (`AdsPackTable`,
    /// `fixtures/write/maintenance`): its records that are not deleted move
    /// to the first slots in their order, the free list empties, the file
    /// ends after them, and their memo values are written anew, record by
    /// record, to an empty memo file of the same block size.
    pub fn pack(&mut self) -> Result<(), Error> {
        self.pack_to(None)
    }

    /// [`TableFile::pack`], with memo blocks of `block_size` bytes if given
    /// (`AdsPackTable120`).
    pub fn pack_to(&mut self, block_size: Option<u16>) -> Result<(), Error> {
        self.check_writable()?;
        if let Format::Dbf(_) = self.structure.header.format {
            return self.pack_dbf(block_size);
        }
        if self.structure.header.format != Format::Adt {
            return Err(Error::NotWritable);
        }
        let mut kept = Vec::new();
        let mut changed = Vec::new();
        for slot in 1..=self.structure.header.record_slots {
            if self.status(slot)? == RecordStatus::Active {
                changed.push(adt::is_changed(self.records.status_byte(slot)?));
                kept.push(self.record(slot)?);
            }
        }
        self.note_rewritten()?;
        let memo_fields: Vec<Field> = self
            .structure
            .fields
            .iter()
            .filter(|f| f.is_memo())
            .cloned()
            .collect();
        if let Some(memo) = self.memo.as_mut() {
            let mut values = Vec::with_capacity(kept.len());
            for record in &kept {
                let mut fields = Vec::with_capacity(memo_fields.len());
                for field in &memo_fields {
                    fields.push(adt::memo_data(field, record, Some(memo))?);
                }
                values.push(fields);
            }
            let block_size = block_size.unwrap_or(memo.block_size() as u16);
            memo.set_block_size(u32::from(block_size));
            let empty = crate::create::adm_bytes(block_size);
            memo.source().set_len(0)?;
            memo.source().write_at(0, &empty)?;
            for (record, fields) in kept.iter_mut().zip(values) {
                for (field, value) in memo_fields.iter().zip(fields) {
                    let at = usize::from(field.offset);
                    let slot = &mut record[at..at + usize::from(field.size)];
                    slot.fill(0);
                    adt::store_memo(field, (!value.is_empty()).then_some(&value[..]), slot, memo)?;
                }
            }
        }
        for (i, record) in kept.iter().enumerate() {
            let slot = i as u32 + 1;
            self.write(slot, record)?;
            // A survivor the last backup has keeps its status.
            if !changed[i] {
                let byte = self.records.status_byte(slot)?;
                self.records
                    .file
                    .write_at(self.records.offset(slot), &[byte & !adt::CHANGED])?;
            }
        }
        self.cut(kept.len() as u32)
    }

    /// Empties an ADT table, as the original engine does (`AdsZapTable`):
    /// no records, no free list, and a memo file of just the first 1,024
    /// bytes of a header (the end of its space after them, its block size).
    pub fn zap(&mut self) -> Result<(), Error> {
        // Nothing is enciphered, so it takes no password
        // (`fixtures/aes/api/07_zap`).
        if !self.write {
            return Err(Error::ReadOnly);
        }
        if let Format::Dbf(_) = self.structure.header.format {
            self.reset_dbf_memo(None)?;
            return self.end_dbf(0);
        }
        if self.structure.header.format != Format::Adt {
            return Err(Error::NotWritable);
        }
        // The header flag stays as it is: a zap is not a write of a record
        // (`fixtures/backup/s58_diff_reinit_hdr`).
        self.set_header_u32(offsets::BACKUP_STAMP, 0)?;
        if let Some(memo) = self.memo.as_mut() {
            let block_size = memo.block_size();
            let mut header = vec![0; 1024];
            header[0..4].copy_from_slice(&1024_u32.div_ceil(block_size.max(1)).to_be_bytes());
            header[6..8].copy_from_slice(&(block_size as u16).to_be_bytes());
            memo.source().set_len(0)?;
            memo.source().write_at(0, &header)?;
        }
        // A zapped table's AutoInc fields start over, where a delete of every
        // record or a pack leaves the counter (`fixtures/write/autoinc_reset`).
        if self
            .structure
            .fields
            .iter()
            .any(|f| f.field_type == FieldType::AutoInc)
        {
            self.set_header_u32(offsets::NEXT_AUTOINC, 1)?;
        }
        self.cut(0)
    }

    /// Recalls every deleted record of an ADT table, as the original engine
    /// does (`AdsRecallAllRecords`, `fixtures/write/table_ops`): each record
    /// on the free list gets the active status byte (its other bytes, the
    /// list's link among them, stay), its memo values are taken back from
    /// the memo file's free space where they still can be, and the free list
    /// empties. Returns the recalled slots, in the list's order.
    pub fn recall_all(&mut self) -> Result<Vec<u32>, Error> {
        self.check_writable()?;
        if self.structure.header.format != Format::Adt {
            return Err(Error::NotWritable);
        }
        self.changes += 1;
        self.note_written()?;
        let mut slots = Vec::new();
        let mut slot = self.structure.header.free_head;
        while slot != 0 && slots.len() < self.records.slots as usize && !slots.contains(&slot) {
            let bytes = self.record(slot)?;
            if !bytes
                .first()
                .is_some_and(|&byte| adt::is_deleted_clear(byte))
            {
                break;
            }
            if let Some(memo) = self.memo.as_mut() {
                for field in self.structure.fields.iter().filter(|f| f.is_memo()) {
                    adt::reclaim_memo(field, field.bytes(&bytes), memo)?;
                }
            }
            self.records
                .file
                .write_at(self.records.offset(slot), &[adt::ACTIVE])?;
            slots.push(slot);
            slot = u32::from_le_bytes(bytes[1..5].try_into().unwrap());
        }
        self.structure.header.free_head = 0;
        self.structure.header.free_count = 0;
        self.set_header_u32(offsets::FREE_HEAD, 0)?;
        self.set_header_u32(offsets::FREE_COUNT, 0)?;
        Ok(slots)
    }

    /// Recalls the deleted ADT record in `slot` alone, as `AdsRecallRecord`
    /// does on a deleted record gone to with `AdsGotoRecord`
    /// (`fixtures/write/binary_changes`): only its status byte turns active,
    /// where `AdsRecallAllRecords` also empties the free list. The list keeps
    /// the slot in its links and the header's head and count stay.
    pub fn recall(&mut self, slot: u32) -> Result<(), Error> {
        self.check_writable()?;
        if self.structure.header.format != Format::Adt {
            return Err(Error::NotWritable);
        }
        let bytes = self.record(slot)?;
        if !bytes
            .first()
            .is_some_and(|&byte| adt::is_deleted_clear(byte))
        {
            return Ok(());
        }
        self.changes += 1;
        self.note_written()?;
        if let Some(memo) = self.memo.as_mut() {
            for field in self.structure.fields.iter().filter(|f| f.is_memo()) {
                adt::reclaim_memo(field, field.bytes(&bytes), memo)?;
            }
        }
        self.records
            .file
            .write_at(self.records.offset(slot), &[adt::ACTIVE])?;
        Ok(())
    }

    /// Sets or clears an ADT table's transaction-free flag in its header
    /// (`AdsSetTableTransactionFree`).
    pub fn set_transaction_free(&mut self, free: bool) -> Result<(), Error> {
        self.check_writable()?;
        if self.structure.header.format != Format::Adt {
            return Err(Error::NotWritable);
        }
        let header = &mut self.structure.header;
        header.flags = match free {
            true => header.flags | flags::TRANSACTION_FREE,
            false => header.flags & !flags::TRANSACTION_FREE,
        };
        let flags = header.flags;
        self.set_header_u32(offsets::FLAGS, flags)
    }

    /// Packs a DBF table, as the original engine does (`AdsPackTable`,
    /// `fixtures/write/dbf_maintenance`): its records not marked deleted
    /// move to the first slots in their order, the file ends after them,
    /// and their memo values are written anew, record by record, to an
    /// empty memo file of the same block size.
    fn pack_dbf(&mut self, block_size: Option<u16>) -> Result<(), Error> {
        let mut kept = Vec::new();
        let mut old_slots = Vec::new();
        for slot in 1..=self.structure.header.record_slots {
            let record = self.record(slot)?;
            if record.first() != Some(&dbf::DELETED) {
                kept.push(record);
                old_slots.push(slot);
            }
        }
        let memo_fields: Vec<Field> = self
            .structure
            .fields
            .iter()
            .filter(|f| f.is_memo())
            .cloned()
            .collect();
        if self.memo.is_some() && !memo_fields.is_empty() {
            let mut values = Vec::with_capacity(kept.len());
            for (record, slot) in kept.iter().zip(&old_slots) {
                let mut fields = Vec::with_capacity(memo_fields.len());
                if let Some(memo) = self.memo.as_mut() {
                    memo.decipher_record(*slot);
                }
                for field in &memo_fields {
                    fields.push(
                        field
                            .memo_data(record, self.memo.as_mut())?
                            .unwrap_or_default(),
                    );
                }
                values.push(fields);
            }
            self.reset_dbf_memo(block_size)?;
            let memo = self.memo.as_mut().ok_or(Error::NotWritable)?;
            for (i, (record, fields)) in kept.iter_mut().zip(values).enumerate() {
                // The values are enciphered for the record's new number.
                memo.decipher_record(i as u32 + 1);
                for (field, value) in memo_fields.iter().zip(fields) {
                    field.encode_blank(record)?;
                    if !value.is_empty() {
                        field.store_memo(Some(&value), record, memo)?;
                    }
                }
            }
        }
        for (i, record) in kept.iter().enumerate() {
            let mut bytes = record.clone();
            self.encipher_dbf_record(i as u32 + 1, &mut bytes)?;
            self.records.write(i as u32 + 1, &bytes)?;
        }
        self.end_dbf(kept.len() as u32)
    }

    /// Empties a DBF table's memo file but for its header, which has blocks of
    /// `block_size` bytes if given (a FoxPro memo file's; a Clipper one's are
    /// always 512).
    fn reset_dbf_memo(&mut self, block_size: Option<u16>) -> Result<(), Error> {
        let Some(memo) = self.memo.as_mut() else {
            return Ok(());
        };
        let header = match memo.kind() {
            crate::memo::MemoKind::Dbt => crate::create::dbt_bytes(),
            _ => {
                let block_size = block_size.unwrap_or(memo.block_size() as u16);
                memo.set_block_size(u32::from(block_size));
                crate::create::fpt_bytes(block_size)
            }
        };
        memo.source().set_len(0)?;
        memo.source().write_at(0, &header)?;
        Ok(())
    }

    /// Ends a DBF table after `slots` records: the header counts them, and
    /// the end-of-file mark follows them.
    fn end_dbf(&mut self, slots: u32) -> Result<(), Error> {
        self.changes += 1;
        self.structure.header.record_slots = slots;
        self.records.slots = slots;
        self.set_header_u32(dbf::RECORD_COUNT, slots)?;
        let end = self.records.offset(slots + 1);
        self.records.file.write_at(end, &[dbf::END_OF_FILE])?;
        Ok(self.records.file.set_len(end + 1)?)
    }

    /// Ends the table after `slots` records, without a free list.
    fn cut(&mut self, slots: u32) -> Result<(), Error> {
        self.changes += 1;
        self.structure.header.free_head = 0;
        self.structure.header.free_count = 0;
        self.set_header_u32(offsets::FREE_HEAD, 0)?;
        self.set_header_u32(offsets::FREE_COUNT, 0)?;
        self.set_slots(slots)?;
        let end = u64::from(self.structure.header.data_offset)
            + u64::from(slots) * u64::from(self.structure.header.record_length);
        Ok(self.records.file.set_len(end)?)
    }

    fn set_slots(&mut self, slots: u32) -> Result<(), Error> {
        self.structure.header.record_slots = slots;
        self.records.slots = slots;
        self.set_header_u32(offsets::RECORD_SLOTS, slots)
    }

    /// Writes `record` (a whole record) to slot `slot`: in an ADT table as an
    /// active record (its prefix is set here), in a DBF table as it is, its
    /// deletion flag included.
    pub fn write(&mut self, slot: u32, record: &[u8]) -> Result<(), Error> {
        self.check_writable()?;
        self.changes += 1;
        self.note_written()?;
        let mut bytes = record.to_vec();
        if bytes.len() != self.structure.header.record_length as usize {
            return Err(Error::Corrupt("record of the wrong length"));
        }
        if let Format::Dbf(_) = self.structure.header.format {
            self.encipher_dbf_record(slot, &mut bytes)?;
            return self.records.write(slot, &bytes);
        }
        // An ADT record's prefix takes five bytes.
        if bytes.len() < 5 {
            return Err(Error::Corrupt("record of the wrong length"));
        }
        bytes[..5].copy_from_slice(&[adt::ACTIVE, 0, 0, 0, 0]);
        self.encipher_written(slot, &mut bytes)?;
        self.records.write(slot, &bytes)
    }

    /// Turns a clear DBF record into the form the file holds when the table
    /// has its password: `E` or `D` for the deletion flag, and the rest one
    /// RC4 stream at the record's number, or AES-CTR under an IV of its own
    /// kept in the record's last 8 bytes.
    fn encipher_dbf_record(&mut self, slot: u32, bytes: &mut [u8]) -> Result<(), Error> {
        let mark = bytes[0] & adt::TX_MARK;
        let status = |bytes: &[u8]| match bytes[0] & !adt::TX_MARK {
            b' ' => Some(dbf::ENCRYPTED_ACTIVE | mark),
            dbf::DELETED => Some(dbf::ENCRYPTED_DELETED | mark),
            // The blank record a transaction appends has no data yet.
            _ => None,
        };
        match self.records.cipher() {
            RecordCipher::Rc4(key) => {
                let Some(status) = status(bytes) else {
                    return Ok(());
                };
                bytes[0] = status;
                crate::crypto::table_record_xor(&key, slot, &mut bytes[1..]);
            }
            RecordCipher::Aes { key, nonce } if bytes.len() > 9 => {
                let Some(status) = status(bytes) else {
                    return Ok(());
                };
                bytes[0] = status;
                let end = bytes.len() - 8;
                let iv = self.records.next_aes_iv((end as u32 - 1).div_ceil(16));
                crate::crypto::aes_ctr_xor(&key, &iv, &nonce, &mut bytes[1..end]);
                bytes[end..].copy_from_slice(&iv);
                self.persist_aes_counter()?;
            }
            _ => {}
        }
        Ok(())
    }

    /// If the table's records are enciphered, turn a just-prepared clear ADT
    /// record `bytes` (its prefix set to a clear 0x04/0x05 status) into the
    /// enciphered form the file holds. RC4: the status becomes 0x06/0x07 and the
    /// data area after the 5-byte prefix is enciphered at `item = slot`. AES: the
    /// record draws the next IV from the shared counter, which is written clear
    /// after the prefix and persisted to the header (0x198); the field data (from
    /// offset 13) is AES-CTR enciphered. No effect on an unencrypted table (its
    /// cipher is `None`); the memo file enciphers its own values when it has one.
    fn encipher_written(&mut self, slot: u32, bytes: &mut [u8]) -> Result<(), Error> {
        match self.records.cipher() {
            RecordCipher::None => {}
            RecordCipher::Rc4(key) => {
                bytes[0] = adt::enciphered_status(bytes[0]).unwrap_or(bytes[0]);
                if bytes.len() > adt::RECORD_PREFIX {
                    crate::crypto::table_record_xor(&key, slot, &mut bytes[adt::RECORD_PREFIX..]);
                }
            }
            RecordCipher::Aes { key, nonce } => {
                let data = adt::RECORD_PREFIX + adt::AES_IV_SIZE;
                let blocks = (bytes.len().saturating_sub(data) as u32).div_ceil(16);
                let iv = self.records.next_aes_iv(blocks);
                adt::aes_encipher_record(&key, &nonce, &iv, bytes);
                self.persist_aes_counter()?;
            }
        }
        Ok(())
    }

    /// Where the table keeps its IV counter: 0x198 in an ADT table's header,
    /// in the crypto block of a DBF table's back link.
    fn aes_counter_offset(&self) -> u64 {
        match self.dbf {
            Some(_) => u64::from(self.structure.header.data_offset) - dbf::BACKLINK as u64 + 8,
            None => adt::AES_IV_COUNTER,
        }
    }

    /// Persist the AES IV counter to the header (0x198), so a later open resumes
    /// from where this one left off and never reissues a spent IV. A header
    /// already past it (IVs an AES dictionary's writer reserved there) stays.
    fn persist_aes_counter(&mut self) -> Result<(), Error> {
        let next = self.records.aes_iv.load(Ordering::Relaxed);
        let at = self.aes_counter_offset();
        let mut stored = [0u8; 8];
        self.records.file.read_at(at, &mut stored)?;
        if next > u64::from_le_bytes(stored) {
            self.records.file.write_at(at, &next.to_le_bytes())?;
        }
        Ok(())
    }

    /// Gives the records `cipher` and the IV counter `counter` (an AES
    /// dictionary's, kept by [`crate::dd`]), so reads decipher them and
    /// writes encipher them.
    pub(crate) fn arm(&mut self, cipher: RecordCipher, counter: Arc<AtomicU64>) {
        self.records.set_cipher(cipher);
        self.records.set_aes_counter(counter);
    }

    /// Takes a DBF table's end-of-file byte off, to write it when the table
    /// closes ([`TableFile::finish_end_of_file`]), as the original's server
    /// does after a transaction appended a record (`fixtures/write/tx_dbf`).
    pub fn defer_end_of_file(&mut self) -> Result<(), Error> {
        let Format::Dbf(_) = self.structure.header.format else {
            return Ok(());
        };
        let end = self.records.offset(self.records.slots + 1);
        self.records.file.set_len(end)?;
        self.end_deferred = true;
        Ok(())
    }

    /// Writes the end-of-file byte [`TableFile::defer_end_of_file`] took
    /// off, as the table closes.
    pub fn finish_end_of_file(&mut self) -> Result<(), Error> {
        if !std::mem::take(&mut self.end_deferred) {
            return Ok(());
        }
        let end = self.records.offset(self.records.slots + 1);
        Ok(self.records.file.write_at(end, &[dbf::END_OF_FILE])?)
    }

    /// Writes `record` to slot `slot` as it is, its first bytes too (a
    /// record a transaction marks: see [`crate::adt::TX_MARK`]).
    pub fn write_raw(&mut self, slot: u32, record: &[u8]) -> Result<(), Error> {
        self.check_writable()?;
        self.changes += 1;
        if record.len() != self.structure.header.record_length as usize {
            return Err(Error::Corrupt("record of the wrong length"));
        }
        let mut bytes = record.to_vec();
        if self.dbf.is_some() {
            self.encipher_dbf_record(slot, &mut bytes)?;
        }
        self.records.write(slot, &bytes)
    }

    /// [`TableFile::write_raw`] without the table's cipher: the blank record
    /// the rollback of an appended DBF record leaves is in the clear in an
    /// encrypted table too (`fixtures/write/dbf_encryption_tx`).
    pub fn write_clear(&mut self, slot: u32, record: &[u8]) -> Result<(), Error> {
        self.check_writable()?;
        self.changes += 1;
        if record.len() != self.structure.header.record_length as usize {
            return Err(Error::Corrupt("record of the wrong length"));
        }
        Ok(self.records.write(slot, record)?)
    }

    /// The status a record's bytes `record` give it (see
    /// [`TableFile::status`]).
    pub fn status_of(&self, record: &[u8]) -> RecordStatus {
        let byte = record.first().copied().unwrap_or(0);
        RecordStatus::from_byte(byte, self.structure.header.format).unwrap_or(RecordStatus::Deleted)
    }

    /// Whether a transaction has marked the record whose bytes are `record`
    /// (changed or deleted it, or appended it blank): see
    /// [`crate::adt::TX_MARK`].
    pub fn marked(&self, record: &[u8]) -> bool {
        let byte = record.first().copied().unwrap_or(0);
        byte == self.appended_mark() || byte & crate::adt::TX_MARK != 0
    }

    /// The first byte of a record a transaction appended.
    fn appended_mark(&self) -> u8 {
        match self.structure.header.format {
            Format::Adt => crate::adt::TX_APPENDED,
            Format::Dbf(_) => crate::dbf::TX_APPENDED,
        }
    }

    /// Whether the table is transaction-free (an ADT table's header flag,
    /// [`TableFile::set_transaction_free`]): its writes are never held back.
    pub fn transaction_free(&self) -> bool {
        self.structure.header.format == Format::Adt
            && self.structure.header.flags & flags::TRANSACTION_FREE != 0
    }

    /// Writes `bytes` at `offset` of the record in slot `slot`, leaving the
    /// rest of it (a binary field's new reference, which the original engine
    /// writes as soon as the value is set).
    pub fn write_part(&mut self, slot: u32, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        self.check_writable()?;
        if slot == 0
            || slot > self.records.slots
            || offset + bytes.len() > self.records.record_length as usize
        {
            return Err(Error::Corrupt("no such record slot"));
        }
        if self.dbf.is_some()
            && matches!(
                self.records.cipher(),
                RecordCipher::Rc4(_) | RecordCipher::Aes { .. }
            )
        {
            // The stream runs over the whole record: splice the part into it.
            let mut whole = vec![0; self.records.record_length as usize];
            self.records.read(slot, &mut whole)?;
            whole[offset..offset + bytes.len()].copy_from_slice(bytes);
            self.encipher_dbf_record(slot, &mut whole)?;
            return Ok(self.records.write(slot, &whole)?);
        }
        let at = self.records.offset(slot) + offset as u64;
        self.records.file.write_at(at, bytes)?;
        // Not shown by the fixtures: a part written alone is taken as a write
        // of the record, so a differential backup copies it.
        if self.structure.header.format == Format::Adt && offset >= adt::RECORD_PREFIX {
            let status = self.records.status_byte(slot)?;
            if status <= 0x07 {
                self.note_written()?;
            }
            if status <= 0x07 && !adt::is_changed(status) {
                self.records
                    .file
                    .write_at(self.records.offset(slot), &[status | adt::CHANGED])?;
            }
        }
        Ok(())
    }

    /// Deletes slot `slot`, putting it at the head of the free list. The
    /// record keeps its bytes, but its memo values are freed; or it takes
    /// the bytes of `record` if given (and frees nothing). A DBF record is
    /// only marked deleted (`*`), its memo values kept.
    pub fn delete(&mut self, slot: u32, record: Option<&[u8]>) -> Result<(), Error> {
        self.check_writable()?;
        self.note_written()?;
        self.delete_unnoted(slot, record)
    }

    /// [`TableFile::delete`] that leaves the header flag [`adt::UNPREPARED`]
    /// alone: the rollback of an appended record leaves it deleted and on the
    /// free list without the table counting as written
    /// (`fixtures/backup/s57_diff_tx`).
    pub fn delete_unnoted(&mut self, slot: u32, record: Option<&[u8]>) -> Result<(), Error> {
        self.check_writable()?;
        self.changes += 1;
        if let Format::Dbf(_) = self.structure.header.format {
            let mut bytes = match record {
                Some(record) => record.to_vec(),
                None => self.record(slot)?,
            };
            bytes[0] = dbf::DELETED;
            return self.write(slot, &bytes);
        }
        let mut bytes = match record {
            Some(record) => record.to_vec(),
            None => {
                let bytes = self.record(slot)?;
                if let Some(memo) = self.memo.as_mut() {
                    for field in self.structure.fields.iter().filter(|f| f.is_memo()) {
                        adt::free_memo(field, field.bytes(&bytes), memo)?;
                    }
                }
                bytes
            }
        };
        if bytes.len() != self.structure.header.record_length as usize || bytes.len() < 5 {
            return Err(Error::Corrupt("record of the wrong length"));
        }
        bytes[0] = adt::DELETED;
        // A record recalled where the table was left (an `AdsGotoRecord` that
        // failed on it) stays the head of the free list, which the original
        // does not push it onto again: the list and its count stay, and the
        // record keeps its link (`fixtures/write/deleted_cursor`).
        if slot == self.structure.header.free_head {
            let link = self.record(slot)?;
            bytes[1..5].copy_from_slice(&link[1..5]);
            self.encipher_written(slot, &mut bytes)?;
            return self.records.write(slot, &bytes);
        }
        bytes[1..5].copy_from_slice(&self.structure.header.free_head.to_le_bytes());
        // A deleted record on an encrypted table keeps its data enciphered
        // (status 0x07); the free-list link stays in the clear 5-byte prefix.
        self.encipher_written(slot, &mut bytes)?;
        let header = &mut self.structure.header;
        header.free_head = slot;
        header.free_count += 1;
        let count = header.free_count;
        self.records.write(slot, &bytes)?;
        self.set_header_u32(offsets::FREE_HEAD, slot)?;
        self.set_header_u32(offsets::FREE_COUNT, count)
    }

    // ---- RC4 encryption (AdsEnableEncryption / Encrypt / Decrypt) ----
    //
    // The on-disk lifecycle is derived black-box and verified byte-for-byte
    // against the engine (see the engine tests). The index (`.adi`) is never
    // touched — RC4 proprietary encryption leaves index keys in the clear.

    /// The RC4 key the records are enciphered with, if the table is set up for
    /// RC4 encryption (its password given, via [`TableFile::enable_encryption`]
    /// or [`adt::open_with_password`]).
    fn rc4_key(&self) -> Option<Vec<u8>> {
        match self.records.cipher() {
            RecordCipher::Rc4(key) => Some(key),
            _ => None,
        }
    }

    /// `AdsEnableEncryption`: persist the header's 20-byte password check (at
    /// 0x38) and encryption type (at 0x4C) and set the records' RC4 cipher, so
    /// later writes encipher and already-enciphered records read deciphered.
    /// Existing records and the table's ENCRYPTED flag are left as they are —
    /// this alone does not encrypt the table. `key` is the password truncated to
    /// the scheme's key length (5 bytes for the 40-bit variant, 20 by default).
    pub fn enable_encryption(&mut self, key: &[u8], enc_type: u8) -> Result<(), Error> {
        if !self.write {
            return Err(Error::ReadOnly);
        }
        if self.dbf.is_some() {
            return self.enable_encryption_dbf(key);
        }
        if self.structure.header.format != Format::Adt {
            return Err(Error::NotWritable);
        }
        let check = crate::crypto::rc4_password_check(key);
        self.records
            .file
            .write_at(offsets::PASSWORD_CHECK, &check)?;
        self.records
            .file
            .write_at(offsets::ENCRYPTION_TYPE, &[enc_type])?;
        self.structure.header.encryption_type = enc_type;
        self.changes += 1;
        // The records cipher goes on now (so writes encipher and 0x06 records
        // read deciphered). The memo cipher goes on only when the whole table is
        // encrypted ([`TableFile::encrypt_table`]): record-level encryption
        // leaves memo values in the clear, as the engine does.
        self.records.set_cipher(RecordCipher::Rc4(key.to_vec()));
        if self.structure.header.encrypted()
            && let Some(memo) = self.memo.as_mut()
        {
            memo.set_cipher(RecordCipher::Rc4(key.to_vec()));
        }
        Ok(())
    }

    /// [`TableFile::enable_encryption`] of a DBF table: the check in bytes 0x0C
    /// to 0x15 of the header, and the records' cipher.
    fn enable_encryption_dbf(&mut self, key: &[u8]) -> Result<(), Error> {
        self.write_dbf_check(key, false)?;
        self.changes += 1;
        self.records.set_cipher(RecordCipher::Rc4(key.to_vec()));
        if self.structure.header.encrypted()
            && let Some(memo) = self.memo.as_mut()
        {
            memo.set_cipher(RecordCipher::Rc4(key.to_vec()));
        }
        Ok(())
    }

    /// Writes the password check of a DBF table's header: the type that says
    /// records only (3) after `AdsEnableEncryption`, and with a memo file, 4
    /// once `AdsEncryptTable` enciphered its memo values too.
    fn write_dbf_check(&mut self, key: &[u8], table: bool) -> Result<(), Error> {
        let check = crate::crypto::dbf_password_check(key, table && self.memo.is_some());
        self.records.file.write_at(0x0C, &check)?;
        if let Some(info) = &mut self.dbf {
            info.password_check = check;
        }
        Ok(())
    }

    /// Writes the version byte of a DBF table as it is when the table is
    /// encrypted (`on`) or not, and the flag of its structure.
    fn set_dbf_encrypted(&mut self, on: bool) -> Result<(), Error> {
        let Some(info) = &mut self.dbf else {
            return Ok(());
        };
        info.version = dbf::version_encrypted(info.version, on);
        let version = info.version;
        self.records.file.write_at(0, &[version])?;
        let flags = match on {
            true => self.structure.header.flags | flags::ENCRYPTED,
            false => self.structure.header.flags & !flags::ENCRYPTED,
        };
        self.structure.header.flags = flags;
        self.records.encrypted = on;
        Ok(())
    }

    /// `AdsEnableEncryption` on an AES table (one created on a connection whose
    /// `EncryptionType` is AES, so its file already has the AES structure — see
    /// [`crate::create::aes_adt_unarmed_bytes`]). Unlike RC4, this is the step
    /// that encrypts the table: it fills the crypto block (type word, nonce, IV
    /// counter, salt, descriptor IV) from `material`, writes the PBKDF2 verifier
    /// at 0x38 and the encryption type at 0x4C, sets the ENCRYPTED flag, and arms
    /// the records' and memo file's AES cipher so later writes encipher. A free
    /// AES table leaves its descriptors in the clear, so none are enciphered here.
    /// (`AdsEncryptTable` is then a no-op: the table is already marked encrypted.)
    pub fn enable_encryption_aes(
        &mut self,
        material: &crate::create::AesMaterial,
        password: &[u8],
    ) -> Result<(), Error> {
        if self.structure.header.format != Format::Adt {
            return Err(Error::NotWritable);
        }
        if !self.write {
            return Err(Error::ReadOnly);
        }
        // The table must already carry the AES layout (its 600-byte header
        // extension), as a table created on an AES connection does; arming a
        // plain table would corrupt it.
        if self.structure.header.extension as usize != crate::create::AES_EXTENSION {
            return Err(Error::NotWritable);
        }
        let key_len = material.key_len();
        let derived = crate::crypto::aes_derive(password, &material.salt, key_len);
        let key = derived[..key_len].to_vec();
        let verifier = &derived[key_len..key_len + 20];

        // The 600-byte crypto block the header extension holds (the inverse of
        // what `open_with_password` reads): type word, nonce, IV counter, salt,
        // descriptor IV, then zero padding.
        let mut ext = vec![0u8; crate::create::AES_EXTENSION];
        ext[0..4].copy_from_slice(&u32::from(material.enc_type).to_le_bytes());
        ext[4..8].copy_from_slice(&material.nonce);
        ext[8..16].copy_from_slice(&material.iv_counter.to_le_bytes());
        ext[16..48].copy_from_slice(&material.salt);
        ext[48..56].copy_from_slice(&material.descriptor_iv);
        self.records.file.write_at(adt::HEADER_SIZE as u64, &ext)?;
        self.records
            .file
            .write_at(offsets::PASSWORD_CHECK, verifier)?;
        self.records
            .file
            .write_at(offsets::ENCRYPTION_TYPE, &[material.enc_type])?;
        let flags = self.structure.header.flags | flags::ENCRYPTED;
        self.set_header_u32(offsets::FLAGS, flags)?;
        self.structure.header.flags = flags;
        self.structure.header.encryption_type = material.enc_type;
        self.changes += 1;

        // Arm the records and the memo file with one shared IV counter, seeded to
        // the counter just written, so writes encipher and advance it in step.
        let nonce = material.nonce;
        let counter = Arc::new(AtomicU64::new(material.iv_counter));
        self.records.set_cipher(RecordCipher::Aes {
            key: key.clone(),
            nonce,
        });
        self.records.set_aes_counter(Arc::clone(&counter));
        self.records.encrypted = true;
        if let Some(memo) = self.memo.as_mut() {
            memo.set_cipher(RecordCipher::Aes { key, nonce });
            memo.set_aes_counter(counter);
        }
        Ok(())
    }

    /// Encrypts a table bound to the AES dictionary at `add_path`, which has
    /// the AES layout and is not encrypted (`sp_EncryptTable`, the
    /// `TABLE_ENCRYPTION` property; `fixtures/aes/dictwrite`, `08_encrypt`):
    /// the table is armed under the dictionary's key as
    /// [`crate::dd::create::arm_table`] arms a new one, then each record it
    /// has is enciphered in place under an IV of its own, with its memo
    /// values and the pages of its `indexes`. A connection of the process
    /// must have given the dictionary's password.
    pub fn encrypt_for_dictionary(
        &mut self,
        add_path: &Path,
        indexes: &[std::path::PathBuf],
    ) -> Result<(), Error> {
        use std::io::Read;
        if !self.write {
            return Err(Error::ReadOnly);
        }
        if self.dbf.is_some() {
            return self.encrypt_dbf_for_dictionary(add_path);
        }
        if self.structure.header.format != Format::Adt
            || self.structure.header.extension as usize != crate::create::AES_EXTENSION
        {
            return Err(Error::NotWritable);
        }
        let Some(RecordCipher::Aes { key, .. }) = crate::dd::keyring::cipher(add_path) else {
            return Err(Error::Encrypted);
        };
        let mut dictionary = [0u8; 0x1C0];
        std::fs::File::open(add_path)?.read_exact(&mut dictionary)?;
        let mut material = crate::create::AesMaterial::generate(dictionary[0x190])?;
        material.salt = dictionary[0x1A0..0x1C0].try_into().unwrap();
        let nonce = material.nonce;

        // The crypto block, with the counter a reserved block on, and the
        // records' IVs drawn from the block's start.
        let mut block = vec![0u8; crate::create::AES_EXTENSION];
        block[0..4].copy_from_slice(&u32::from(material.enc_type).to_le_bytes());
        block[4..8].copy_from_slice(&nonce);
        let counter = material.iv_counter + crate::dd::keyring::RESERVED;
        block[8..16].copy_from_slice(&counter.to_le_bytes());
        block[16..48].copy_from_slice(&material.salt);
        block[48..56].copy_from_slice(&material.descriptor_iv);
        // Read before the cipher goes on: a clear table's records are as
        // they are.
        let clear: Vec<Vec<u8>> = (1..=self.structure.header.record_slots)
            .map(|slot| self.record(slot))
            .collect::<Result<_, _>>()?;
        self.note_rewritten()?;
        let file = &mut self.records.file;
        let data = u64::from(self.structure.header.data_offset);
        let start = adt::HEADER_SIZE as u64 + crate::create::AES_EXTENSION as u64;
        let mut descriptors = vec![0u8; (data - start) as usize];
        file.read_at(start, &mut descriptors)?;
        crate::crypto::aes_ctr_xor(&key, &material.descriptor_iv, &nonce, &mut descriptors);
        file.write_at(adt::HEADER_SIZE as u64, &block)?;
        file.write_at(start, &descriptors)?;
        file.write_at(offsets::PASSWORD_CHECK, &dictionary[0x38..0x4C])?;
        file.write_at(offsets::ENCRYPTION_TYPE, &[material.enc_type])?;
        let flags = self.structure.header.flags | flags::ENCRYPTED | flags::ENCRYPTED_DESCRIPTORS;
        self.set_header_u32(offsets::FLAGS, flags)?;
        self.structure.header.flags = flags;
        self.structure.header.encryption_type = material.enc_type;
        self.changes += 1;

        let cipher = RecordCipher::Aes {
            key: key.clone(),
            nonce,
        };
        let counter = Arc::new(AtomicU64::new(material.iv_counter));
        self.records.set_cipher(cipher.clone());
        self.records.set_aes_counter(Arc::clone(&counter));
        self.records.encrypted = true;
        if let Some(memo) = self.memo.as_mut() {
            memo.set_cipher(cipher);
            memo.set_aes_counter(counter);
        }
        for (slot, mut record) in (1..).zip(clear) {
            self.aes_recipher_memos(&record, Some((&key, &nonce)))?;
            if record[0] <= 0x07 {
                record[0] |= adt::CHANGED;
            }
            self.encipher_written(slot, &mut record)?;
            self.records.write(slot, &record)?;
        }
        for index in indexes.iter().filter(|index| index.is_file()) {
            crate::adi::encipher_tree_pages(index, &key, &nonce, &self.records.aes_iv)
                .map_err(|_| Error::Corrupt("index"))?;
            crate::adi::make_aes(index, &dictionary[0x38..0x4C])?;
        }
        self.persist_aes_counter()?;
        Ok(())
    }

    /// Enciphers in place, under an IV of its own, each memo value the clear
    /// record `record` refers to (`fixtures/aes/dictwrite`, `10_memo`): a
    /// value is enciphered to the end of its blocks, as the original does it.
    fn aes_recipher_memos(
        &mut self,
        record: &[u8],
        cipher: Option<(&[u8], &[u8; 4])>,
    ) -> Result<(), Error> {
        let Some(memo) = self.memo.as_mut() else {
            return Ok(());
        };
        let block_size = memo.block_size() as usize;
        let refs: Vec<(u32, usize)> = self
            .structure
            .fields
            .iter()
            .filter(|f| f.is_memo())
            .map(|f| adt::memo_ref(f, record))
            .filter(|r| r.block != 0 && r.stored != 0)
            .map(|r| (r.block, r.stored as usize))
            .collect();
        for (block, stored) in refs {
            let room = (8 + stored).div_ceil(block_size) * block_size - 8;
            let mut raw = memo.read_raw(block, 8 + room)?;
            // A value written once the table was armed is enciphered under
            // an IV of its own already; only the zero IV marks a clear one.
            if cipher.is_some() && raw[..8] != [0; 8] {
                continue;
            }
            if let Some((key, nonce)) = cipher {
                let iv = self.records.next_aes_iv(room.div_ceil(16) as u32);
                crate::crypto::aes_ctr_xor(key, &iv, nonce, &mut raw[8..]);
                raw[..8].copy_from_slice(&iv);
            }
            memo.write_raw(block, &raw)?;
        }
        Ok(())
    }

    /// Puts each memo value the record `record` refers to in the clear, in a
    /// place of its own: it is deciphered with `key` and `nonce` and written
    /// anew behind a zero IV, and the blocks it had freed, as the original
    /// does when it decrypts a table (`fixtures/aes/dictwrite`,
    /// `15_decrypt_all`). `record` takes the new places.
    fn aes_decipher_memos(
        &mut self,
        record: &mut [u8],
        key: &[u8],
        nonce: &[u8; 4],
    ) -> Result<(), Error> {
        let Some(memo) = self.memo.as_mut() else {
            return Ok(());
        };
        for field in self.structure.fields.iter().filter(|f| f.is_memo()) {
            let reference = adt::memo_ref(field, record);
            if reference.block == 0 || reference.stored == 0 {
                continue;
            }
            let stored = reference.stored as usize;
            let raw = memo.read_raw(reference.block, 8 + stored)?;
            let iv: [u8; 8] = raw[..8].try_into().unwrap();
            let mut value = vec![0; 8];
            value.extend_from_slice(&raw[8..]);
            if iv != [0; 8] {
                crate::crypto::aes_ctr_xor(key, &iv, nonce, &mut value[8..]);
            }
            let length = 8 + reference.stored;
            let mut allocator = memo.allocator()?;
            let block = allocator.alloc(length)?;
            allocator.write(block, &value)?;
            allocator.free(reference.block, length)?;
            let at = usize::from(field.offset);
            record[at..at + 4].copy_from_slice(&block.to_le_bytes());
        }
        Ok(())
    }

    /// Decrypts a table bound to an AES dictionary (`sp_DecryptTable`, the
    /// `TABLE_ENCRYPTION` property; `fixtures/aes/dictwrite`, `09_decrypt`):
    /// its descriptors and records are put back in the clear, their IVs
    /// zeroed, and its flags and type cleared, with its memo values and
    /// `indexes`; the crypto block stays as it is, as does the dictionary's
    /// verifier.
    pub fn decrypt_for_dictionary(&mut self, indexes: &[std::path::PathBuf]) -> Result<(), Error> {
        if !self.write {
            return Err(Error::ReadOnly);
        }
        if self.dbf.is_some() {
            return self.decrypt_dbf_aes();
        }
        let RecordCipher::Aes { key, nonce } = self.records.cipher() else {
            return Err(Error::Encrypted);
        };
        let clear: Vec<Vec<u8>> = (1..=self.structure.header.record_slots)
            .map(|slot| self.record(slot))
            .collect::<Result<_, _>>()?;
        let file = &mut self.records.file;
        let data = u64::from(self.structure.header.data_offset);
        let start = adt::HEADER_SIZE as u64 + crate::create::AES_EXTENSION as u64;
        let mut descriptors = vec![0u8; (data - start) as usize];
        file.read_at(start, &mut descriptors)?;
        let mut iv = [0u8; 8];
        file.read_at(adt::HEADER_SIZE as u64 + 48, &mut iv)?;
        crate::crypto::aes_ctr_xor(&key, &iv, &nonce, &mut descriptors);
        file.write_at(start, &descriptors)?;
        file.write_at(offsets::ENCRYPTION_TYPE, &[0])?;
        let flags =
            self.structure.header.flags & !(flags::ENCRYPTED | flags::ENCRYPTED_DESCRIPTORS);
        self.set_header_u32(offsets::FLAGS, flags)?;
        self.structure.header.flags = flags;
        self.structure.header.encryption_type = 0;
        self.changes += 1;
        self.records.encrypted = false;
        self.disable_encryption();
        if let Some(memo) = self.memo.as_mut() {
            memo.set_enciphered_before(key.clone(), nonce);
            memo.allocator()?.forget_free_space()?;
        }
        for (slot, mut record) in (1..).zip(clear) {
            self.aes_decipher_memos(&mut record, &key, &nonce)?;
            self.records.write(slot, &record)?;
        }
        for index in indexes.iter().filter(|index| index.is_file()) {
            crate::adi::decipher_in_place(index, &key, &nonce)
                .map_err(|_| Error::Corrupt("index"))?;
        }
        Ok(())
    }

    /// Enciphers or deciphers in place the header-to-data region of a table
    /// of a dictionary of RC4 tables, its field descriptors, as the original
    /// keeps them: one stream at item `0x45 × fields` (the table must be
    /// open with its key), and sets or clears the flag that says so.
    fn rc4_descriptors(&mut self, on: bool) -> Result<(), Error> {
        let Some(key) = self.rc4_key() else {
            return Err(Error::Encrypted);
        };
        let start = adt::HEADER_SIZE as u64 + u64::from(self.structure.header.extension);
        let data = u64::from(self.structure.header.data_offset);
        let mut region = vec![0u8; (data - start) as usize];
        self.records.file.read_at(start, &mut region)?;
        let item = 0x45 * u32::from(self.structure.header.field_count);
        crate::crypto::rc4_crypt(&key, item, &mut region);
        self.records.file.write_at(start, &region)?;
        let flags = match on {
            true => self.structure.header.flags | flags::ENCRYPTED_DESCRIPTORS,
            false => self.structure.header.flags & !flags::ENCRYPTED_DESCRIPTORS,
        };
        self.set_header_u32(offsets::FLAGS, flags)?;
        self.structure.header.flags = flags;
        Ok(())
    }

    /// Encrypts a table bound to a dictionary of RC4 tables, whose table
    /// password is `key` (`sp_EncryptTable`, the `TABLE_ENCRYPTION`
    /// property; `fixtures/aes/rc4tp`): as `AdsEnableEncryption` and
    /// `AdsEncryptTable` do a free table, with its descriptors enciphered as
    /// well, and the pages of its `indexes` when the dictionary encrypts
    /// them.
    pub fn encrypt_for_dictionary_rc4(
        &mut self,
        key: &[u8],
        indexes: &[std::path::PathBuf],
    ) -> Result<(), Error> {
        let key = &key[..key.len().min(crate::crypto::KEY_MAX)];
        self.enable_encryption(key, 3)?;
        self.encrypt_table()?;
        if self.dbf.is_none() {
            self.rc4_descriptors(true)?;
        }
        // The index files of a Clipper or FoxPro table stay in the clear
        // (`fixtures/aes/rc4tp`, `y9`).
        for index in indexes
            .iter()
            .filter(|index| index.is_file() && self.dbf.is_none())
        {
            crate::adi::encipher_rc4(index, key).map_err(|_| Error::Corrupt("index"))?;
        }
        Ok(())
    }

    /// Decrypts a table bound to a dictionary of RC4 tables, which was open
    /// with its key: the reverse of [`TableFile::encrypt_for_dictionary_rc4`],
    /// with its `indexes` put back in the clear.
    pub fn decrypt_for_dictionary_rc4(
        &mut self,
        indexes: &[std::path::PathBuf],
    ) -> Result<(), Error> {
        let Some(key) = self.rc4_key() else {
            return Err(Error::Encrypted);
        };
        if self.dbf.is_none() {
            self.rc4_descriptors(false)?;
        }
        self.decrypt_table_keeping(true)?;
        for index in indexes
            .iter()
            .filter(|index| index.is_file() && self.dbf.is_none())
        {
            crate::adi::decipher_rc4(index, &key).map_err(|_| Error::Corrupt("index"))?;
        }
        Ok(())
    }

    /// Whether the table is already AES-encrypted (its header's ENCRYPTED flag is
    /// set and its type is AES-128/256), so `AdsEnableEncryption` on it supplies
    /// the password rather than encrypting it afresh.
    pub fn is_aes_encrypted(&self) -> bool {
        self.structure.header.encrypted()
            && matches!(
                self.structure.header.encryption_type,
                adt::ENCRYPTION_AES128 | adt::ENCRYPTION_AES256
            )
    }

    /// `AdsEnableEncryption` on a table that is already AES-encrypted: supply the
    /// password so the session can read and write it, without changing the file.
    /// It derives the key from the stored salt, checks it against the stored
    /// verifier (wrong password → [`Error::Encrypted`]) and arms the records' and
    /// memo file's cipher from the crypto block already on disk (as opening with
    /// the password does). This is the inverse of [`enable_encryption_aes`], which
    /// encrypts a fresh table; here nothing is generated or rewritten.
    pub fn supply_aes_password(&mut self, password: &[u8]) -> Result<(), Error> {
        let enc_type = self.structure.header.encryption_type;
        let key_len = if enc_type == adt::ENCRYPTION_AES256 {
            32
        } else {
            16
        };
        // The crypto block the header extension holds: nonce (0x194), IV counter
        // (0x198), salt (0x1A0); the verifier is at 0x38.
        let mut crypto = [0u8; 0x1C8 - 0x190];
        self.records
            .file
            .read_at(adt::HEADER_SIZE as u64, &mut crypto)?;
        let at = |offset: usize| offset - adt::HEADER_SIZE;
        let nonce: [u8; 4] = crypto[at(0x194)..at(0x194) + 4].try_into().unwrap();
        let counter = u64::from_le_bytes(crypto[at(0x198)..at(0x198) + 8].try_into().unwrap());
        let salt = &crypto[at(0x1A0)..at(0x1A0) + 32];
        let mut verifier = [0u8; 20];
        self.records
            .file
            .read_at(offsets::PASSWORD_CHECK, &mut verifier)?;
        let derived = crate::crypto::aes_derive(password, salt, key_len);
        if derived[key_len..key_len + 20] != verifier {
            return Err(Error::Encrypted);
        }
        let key = derived[..key_len].to_vec();
        let shared = Arc::new(AtomicU64::new(counter));
        self.records.set_cipher(RecordCipher::Aes {
            key: key.clone(),
            nonce,
        });
        self.records.set_aes_counter(Arc::clone(&shared));
        self.records.encrypted = true;
        if let Some(memo) = self.memo.as_mut() {
            memo.set_cipher(RecordCipher::Aes { key, nonce });
            memo.set_aes_counter(shared);
        }
        Ok(())
    }

    /// `AdsDisableEncryption`: drop the session's cipher so later writes are
    /// plaintext and reads stop deciphering. Nothing on disk changes — the file
    /// stays encrypted as it is.
    pub fn disable_encryption(&mut self) {
        self.records.set_cipher(RecordCipher::None);
        if let Some(memo) = self.memo.as_mut() {
            memo.set_cipher(RecordCipher::None);
        }
    }

    /// `AdsEncryptTable`: encipher every record slot in place (active 0x04→0x06,
    /// deleted 0x05→0x07, data area after the 5-byte prefix) and every memo
    /// value, then set the header's ENCRYPTED flag. The password check and type
    /// must be in place already ([`TableFile::enable_encryption`]).
    pub fn encrypt_table(&mut self) -> Result<(), Error> {
        if !self.write {
            return Err(Error::ReadOnly);
        }
        if self.dbf_aes_armed() {
            return self.encrypt_dbf_aes();
        }
        // An AES table is armed by `enable_encryption_aes` (it sets the
        // ENCRYPTED flag and the cipher), so new writes already encipher;
        // the records and memo values written before are in the clear, and
        // each takes an IV of its own now (`fixtures/aes/api/09_encrypt_full`).
        if let RecordCipher::Aes { key, nonce } = self.records.cipher() {
            self.note_rewritten()?;
            for slot in 1..=self.structure.header.record_slots {
                let mut record = self.record(slot)?;
                if self.enciphered(&record) {
                    continue;
                }
                self.aes_recipher_memos(&record, Some((&key, &nonce)))?;
                if record[0] <= 0x07 {
                    record[0] |= adt::CHANGED;
                }
                self.encipher_written(slot, &mut record)?;
                self.records.write(slot, &record)?;
            }
            self.persist_aes_counter()?;
            return Ok(());
        }
        let key = self.rc4_key().ok_or(Error::NotWritable)?;
        if self.structure.header.encrypted() {
            return Ok(()); // already table-encrypted; do not re-cipher
        }
        self.note_rewritten()?;
        if self.dbf.is_some() {
            for slot in 1..=self.structure.header.record_slots {
                let bytes = self.record(slot)?;
                self.recipher_memos(slot, &bytes, &key)?;
                self.encrypt_slot(slot, &bytes, &key)?;
            }
            self.set_dbf_encrypted(true)?;
            self.write_dbf_check(&key, true)?;
            if let Some(memo) = self.memo.as_mut() {
                memo.set_cipher(RecordCipher::Rc4(key));
            }
            self.changes += 1;
            return Ok(());
        }
        for slot in 1..=self.structure.header.record_slots {
            let bytes = self.record(slot)?; // clear data, or a 0x06 record deciphered
            // Every memo value is enciphered (it was in the clear: only a
            // whole-table encryption ciphers memos), then the record data.
            self.recipher_memos(slot, &bytes, &key)?;
            self.encrypt_slot(slot, &bytes, &key)?;
        }
        let flags = self.structure.header.flags | flags::ENCRYPTED;
        self.structure.header.flags = flags;
        self.records.encrypted = true;
        if let Some(memo) = self.memo.as_mut() {
            memo.set_cipher(RecordCipher::Rc4(key));
        }
        self.changes += 1;
        self.set_header_u32(offsets::FLAGS, flags)
    }

    /// `AdsDecryptTable`: decipher every record slot in place (0x06→0x04,
    /// 0x07→0x05) and every memo value, clear the ENCRYPTED flag and the
    /// encryption type (the password-check bytes at 0x38 are left as the engine
    /// leaves them), and disable the session's cipher.
    pub fn decrypt_table(&mut self) -> Result<(), Error> {
        self.decrypt_table_keeping(false)
    }

    /// [`TableFile::decrypt_table`], leaving the deleted records enciphered
    /// if `keep_deleted`, as the original does a dictionary's table
    /// (`fixtures/aes/rc4tp`, `f2`).
    fn decrypt_table_keeping(&mut self, keep_deleted: bool) -> Result<(), Error> {
        if !self.write {
            return Err(Error::ReadOnly);
        }
        if self.dbf_aes_armed() {
            return self.decrypt_dbf_aes();
        }
        let key = self.rc4_key().ok_or(Error::NotWritable)?;
        let table_level = self.structure.header.encrypted();
        if self.dbf.is_some() {
            for slot in 1..=self.structure.header.record_slots {
                let bytes = self.record(slot)?;
                if table_level {
                    self.recipher_memos(slot, &bytes, &key)?;
                }
                self.decrypt_slot(slot, &bytes)?;
            }
            self.set_dbf_encrypted(false)?;
            self.records.file.write_at(0x0C, &[0; 10])?;
            if let Some(info) = &mut self.dbf {
                info.password_check = [0; 10];
            }
            self.changes += 1;
            self.disable_encryption();
            return Ok(());
        }
        for slot in 1..=self.structure.header.record_slots {
            if keep_deleted && matches!(self.status_byte(slot)? & !adt::TX_MARK, 0x03 | 0x07) {
                continue;
            }
            let bytes = self.record(slot)?; // a 0x06 record reads back deciphered
            // Memo values were enciphered only by a whole-table encryption.
            if table_level {
                self.recipher_memos(slot, &bytes, &key)?;
            }
            self.decrypt_slot(slot, &bytes)?;
        }
        let flags = self.structure.header.flags & !flags::ENCRYPTED;
        self.structure.header.flags = flags;
        self.records.encrypted = false;
        self.structure.header.encryption_type = 0;
        self.changes += 1;
        self.set_header_u32(offsets::FLAGS, flags)?;
        self.records.file.write_at(offsets::ENCRYPTION_TYPE, &[0])?;
        self.disable_encryption();
        Ok(())
    }

    /// `AdsEncryptRecord`: encipher slot `slot`'s data area in place (0x04→0x06),
    /// without touching the table's ENCRYPTED flag or the record's memo values —
    /// record-level encryption leaves memos in the clear.
    pub fn encrypt_record(&mut self, slot: u32) -> Result<(), Error> {
        if !self.write {
            return Err(Error::ReadOnly);
        }
        let key = self.rc4_key().ok_or(Error::NotWritable)?;
        self.changes += 1;
        let bytes = self.record(slot)?;
        self.encrypt_slot(slot, &bytes, &key)
    }

    /// `AdsDecryptRecord`: decipher slot `slot`'s data area in place (0x06→0x04).
    pub fn decrypt_record(&mut self, slot: u32) -> Result<(), Error> {
        if !self.write {
            return Err(Error::ReadOnly);
        }
        self.rc4_key().ok_or(Error::NotWritable)?;
        self.changes += 1;
        let bytes = self.record(slot)?;
        self.decrypt_slot(slot, &bytes)
    }

    /// Encipher one slot's data area in place from its clear `bytes` (as
    /// [`TableFile::record`] returns them): 0x04→0x06, 0x05→0x07, data after the
    /// 5-byte prefix enciphered at `item = slot`. An already-enciphered record is
    /// left as it is. Memo values are handled by the caller.
    fn encrypt_slot(&mut self, slot: u32, bytes: &[u8], key: &[u8]) -> Result<(), Error> {
        let mut bytes = bytes.to_vec();
        if self.dbf.is_some() {
            // The record reads the same either way: its first byte in the file
            // says whether it is enciphered.
            let raw = self.records.status_byte(slot)?;
            if self.enciphered(&[raw]) {
                return Ok(());
            }
            bytes[0] = match bytes[0] {
                b' ' => dbf::ENCRYPTED_ACTIVE,
                dbf::DELETED => dbf::ENCRYPTED_DELETED,
                _ => return Ok(()),
            };
            crate::crypto::table_record_xor(key, slot, &mut bytes[1..]);
            return Ok(self.records.write(slot, &bytes)?);
        }
        // Already enciphered, or a status we leave alone.
        let Some(enciphered) = adt::enciphered_status(bytes[0]) else {
            return Ok(());
        };
        // Every record counts as written for a differential backup
        // (`fixtures/backup/s58_diff_reinit_hdr`).
        bytes[0] = enciphered | adt::CHANGED;
        crate::crypto::table_record_xor(key, slot, &mut bytes[adt::RECORD_PREFIX..]);
        self.records.write(slot, &bytes)
    }

    /// Decipher one slot's data area in place: the inverse of
    /// [`TableFile::encrypt_slot`]. `bytes` is the deciphered record (as
    /// [`TableFile::record`] returns it); only the status and the stored bytes
    /// change. Memo values are handled by the caller.
    fn decrypt_slot(&mut self, slot: u32, bytes: &[u8]) -> Result<(), Error> {
        let mut bytes = bytes.to_vec();
        if self.dbf.is_some() {
            let raw = self.records.status_byte(slot)?;
            if !self.enciphered(&[raw]) {
                return Ok(());
            }
            return Ok(self.records.write(slot, &bytes)?);
        }
        let Some(clear) = adt::clear_status(bytes[0]) else {
            return Ok(()); // not enciphered
        };
        bytes[0] = clear;
        self.records.write(slot, &bytes)
    }

    /// [`TableFile::recipher_memos`] of a DBF table: an `.fpt` value after its
    /// block header, a `.dbt` value with its first 0x1A.
    fn recipher_dbf_memos(&mut self, slot: u32, record: &[u8], key: &[u8]) -> Result<(), Error> {
        let fields: Vec<(u32, bool)> = self
            .structure
            .fields
            .iter()
            .filter_map(|f| {
                let block = f.memo_ref(record)?.block;
                let tagged = matches!(f.field_type, FieldType::Binary | FieldType::Image)
                    && matches!(&f.layout, crate::field::Layout::Dbf(layout) if !layout.vfp_table());
                (block != 0).then_some((block, tagged))
            })
            .collect();
        let memo = self.memo.as_mut().unwrap();
        let fpt = memo.kind() == crate::memo::MemoKind::Fpt;
        for (block, tagged) in fields {
            // A SIx tag stays clear, in either memo file.
            let tag = if tagged { 16 } else { 0 };
            let (from, length) = if fpt {
                let header = memo.read_raw(block, 8)?;
                (
                    8 + tag,
                    u32::from_be_bytes(header[4..].try_into().unwrap()) as usize - tag,
                )
            } else {
                // The memo file reads its value in the state it is in, which
                // gives the end of the value in either case.
                memo.decipher_record(slot);
                let value = memo.read_dbf(block, tagged)?.len();
                (tag, value - tag + usize::from(!tagged))
            };
            let mut data = memo.read_raw(block, from + length)?;
            crate::crypto::rc4_crypt(key, slot, &mut data[from..]);
            memo.write_raw(block, &data)?;
        }
        Ok(())
    }

    /// Encipher (or decipher — RC4 is symmetric) in place every memo value that
    /// the clear record `record` in slot `slot` references, at `item = slot`.
    fn recipher_memos(&mut self, slot: u32, record: &[u8], key: &[u8]) -> Result<(), Error> {
        if self.memo.is_none() {
            return Ok(());
        }
        if self.dbf.is_some() {
            return self.recipher_dbf_memos(slot, record, key);
        }
        let refs: Vec<(u32, u32)> = self
            .structure
            .fields
            .iter()
            .filter(|f| f.is_memo())
            .map(|f| {
                let r = adt::memo_ref(f, record);
                (r.block, r.stored)
            })
            .filter(|&(block, stored)| block != 0 && stored != 0)
            .collect();
        let memo = self.memo.as_mut().unwrap();
        for (block, stored) in refs {
            let mut data = memo.read_raw(block, stored as usize)?;
            crate::crypto::rc4_crypt(key, slot, &mut data);
            memo.write_raw(block, &data)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A copy of a fixture file in a fresh folder, removed when dropped.
    struct Copy(PathBuf);

    impl Copy {
        fn of(fixture: &str, name: &str) -> Copy {
            let dir = crate::testing::temp_root()
                .join(format!("tflip-table-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let from = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/adt")
                .join(fixture);
            std::fs::copy(from.join(name), dir.join(name)).unwrap();
            Copy(dir.join(name))
        }
    }

    impl Drop for Copy {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    #[test]
    fn inserts_reuse_the_free_list() {
        // fl_chain's free list is 1 -> 6 -> 3, its slots 1 to 8.
        let copy = Copy::of("freelist", "fl_chain.adt");
        let mut table = TableFile::open_for_writing(&copy.0, Format::Adt).unwrap();
        let record = table.structure.null_record();
        let slots: Vec<u32> = (0..4).map(|_| table.insert(&record).unwrap()).collect();
        assert_eq!(slots, [1, 6, 3, 9]);
        assert_eq!(
            (
                table.structure.header.free_head,
                table.structure.header.free_count
            ),
            (0, 0)
        );
        table.delete(6, None).unwrap();
        table.delete(1, None).unwrap();
        drop(table);
        let mut table = TableFile::open(&copy.0, Format::Adt).unwrap();
        let header = &table.structure.header;
        assert_eq!(
            (header.record_slots, header.free_head, header.free_count),
            (9, 1, 2)
        );
        assert_eq!(table.status(9).unwrap(), RecordStatus::Active);
        assert_eq!(table.record(1).unwrap()[..5], [5, 6, 0, 0, 0]);
        assert_eq!(table.record(6).unwrap()[..5], [5, 0, 0, 0, 0]);
        assert!(matches!(table.insert(&record), Err(Error::ReadOnly)));
    }

    /// The RC4 encryption-write lifecycle matches the engine byte for byte
    /// (`fixtures/write/encryption`, password `secret`).
    #[test]
    fn encrypts_and_decrypts_like_the_engine() {
        let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/write/encryption");
        let golden = |name: &str| std::fs::read(fx.join(name)).unwrap();

        // A fresh copy of the plaintext base table (with its memo file).
        let work = |name: &str| -> PathBuf {
            let dir = crate::testing::temp_root()
                .join(format!("tflip-enc-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::copy(fx.join("base.adt"), dir.join("t.adt")).unwrap();
            std::fs::copy(fx.join("base.adm"), dir.join("t.adm")).unwrap();
            dir.join("t.adt")
        };
        let adt = |path: &Path| std::fs::read(path).unwrap();
        let adm = |path: &Path| std::fs::read(path.with_extension("adm")).unwrap();

        // AdsEnableEncryption writes the password check and type; nothing else.
        let path = work("enable");
        let mut t = TableFile::open_for_writing(&path, Format::Adt).unwrap();
        t.enable_encryption(b"secret", adt::ENCRYPTION_RC4).unwrap();
        drop(t);
        assert_eq!(adt(&path), golden("enable.adt"), "enable .adt");
        assert_eq!(adm(&path), golden("base.adm"), "enable leaves the memo");

        // AdsEncryptRecord on record 1: record-level, memo untouched.
        let path = work("encrec");
        let mut t = TableFile::open_for_writing(&path, Format::Adt).unwrap();
        t.enable_encryption(b"secret", adt::ENCRYPTION_RC4).unwrap();
        t.encrypt_record(1).unwrap();
        drop(t);
        assert_eq!(adt(&path), golden("encrec.adt"), "encrec .adt");
        assert_eq!(adm(&path), golden("base.adm"), "encrec leaves the memo");

        // AdsEncryptTable: every record and memo enciphered, the flag set.
        let path = work("enctable");
        let mut t = TableFile::open_for_writing(&path, Format::Adt).unwrap();
        t.enable_encryption(b"secret", adt::ENCRYPTION_RC4).unwrap();
        t.encrypt_table().unwrap();
        drop(t);
        assert_eq!(adt(&path), golden("enctable.adt"), "enctable .adt");
        assert_eq!(adm(&path), golden("enctable.adm"), "enctable .adm");

        // AdsDecryptTable on the encrypted table restores the records (the 0x38
        // block stays, the type is cleared) and the memo values.
        let path = work("dectable");
        let mut t = TableFile::open_for_writing(&path, Format::Adt).unwrap();
        t.enable_encryption(b"secret", adt::ENCRYPTION_RC4).unwrap();
        t.encrypt_table().unwrap();
        t.decrypt_table().unwrap();
        drop(t);
        assert_eq!(adt(&path), golden("dectable.adt"), "dectable .adt");
        assert_eq!(adm(&path), golden("base.adm"), "dectable restores the memo");

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A record-level encrypted table (one record enciphered by
    /// `AdsEncryptRecord`, the table flag clear) reads back deciphered with its
    /// password: the enciphered record matches the plaintext base, the clear
    /// records are untouched, and the memo values stay in the clear.
    #[test]
    fn reads_a_record_level_encrypted_table() {
        let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/write/encryption");
        let dir = crate::testing::temp_root().join(format!("tflip-enc-rl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(fx.join("encrec.adt"), dir.join("t.adt")).unwrap();
        std::fs::copy(fx.join("base.adm"), dir.join("t.adm")).unwrap();

        // The header flag is clear (record-level), but the table is readable only
        // with the password: record 1 deciphers to the plaintext base record.
        let mut base = TableFile::open(&fx.join("base.adt"), Format::Adt).unwrap();
        let mut enc =
            crate::adt::open_with_password(&dir.join("t.adt"), false, Some(b"secret")).unwrap();
        assert!(!enc.structure.header.encrypted(), "flag stays clear");
        // Record 1 was enciphered: it keeps its 0x06 status (as the engine
        // returns for RC4) but its data deciphers to the plaintext base.
        assert_eq!(enc.record(1).unwrap()[0], crate::adt::ENCRYPTED_ACTIVE);
        assert_eq!(
            enc.record(1).unwrap()[crate::adt::RECORD_PREFIX..],
            base.record(1).unwrap()[crate::adt::RECORD_PREFIX..],
            "record 1 data",
        );
        // Records 2 to 4 were never enciphered: clear and identical.
        for slot in 2..=4 {
            assert_eq!(
                enc.record(slot).unwrap(),
                base.record(slot).unwrap(),
                "record {slot}"
            );
        }
        // The memo of the enciphered record 1 is in the clear.
        let memo_field = enc
            .structure
            .fields
            .iter()
            .find(|f| f.is_memo())
            .unwrap()
            .clone();
        let rec1 = enc.record(1).unwrap();
        let value = memo_field.memo_data(&rec1, enc.memo()).unwrap().unwrap();
        assert_eq!(value, b"memo-alpha");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An RC4 dictionary-bound table (encrypted descriptors) opens and reads
    /// deciphered with its table password, and is refused with a wrong one.
    #[test]
    fn opens_and_decrypts_an_rc4_table() {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt/dictionary_encrypted");
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("rc4_secret.json")).unwrap()).unwrap();

        // The wrong password is refused (the "ADVANTAGE" check does not match).
        assert!(matches!(
            crate::adt::open_with_password(&dir.join("rc4_secret.adt"), false, Some(b"wrong")),
            Err(Error::Encrypted)
        ));

        // The right password: descriptors and records come back deciphered.
        let mut table =
            crate::adt::open_with_password(&dir.join("rc4_secret.adt"), false, Some(b"tblpass"))
                .unwrap();
        let names: Vec<&str> = table
            .structure
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["id", "name", "note"],
            "field descriptors deciphered"
        );
        let mut buf = vec![0; table.structure.header.record_length as usize];
        for rec in json["records"].as_array().unwrap() {
            let slot = rec["recno"].as_u64().unwrap() as u32;
            table.read(slot, &mut buf).unwrap();
            assert_eq!(buf, bytes_of(rec), "record {slot}");
        }
    }

    /// An AES table opens and reads deciphered with the right password, and is
    /// refused with the wrong one.
    #[test]
    fn opens_and_decrypts_an_aes_table() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/aes/aes128");
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("s.json")).unwrap()).unwrap();

        // The wrong password is refused (its verifier does not match).
        assert!(matches!(
            crate::adt::open_with_password(&dir.join("s.adt"), false, Some(b"wrong")),
            Err(Error::Encrypted)
        ));

        // The right password: descriptors and records come back deciphered.
        let mut table =
            crate::adt::open_with_password(&dir.join("s.adt"), false, Some(b"another pw")).unwrap();
        let names: Vec<&str> = table
            .structure
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["id", "name", "note"],
            "field descriptors deciphered"
        );
        let mut buf = vec![0; table.structure.header.record_length as usize];
        for rec in json["records"].as_array().unwrap() {
            let slot = rec["recno"].as_u64().unwrap() as u32;
            table.read(slot, &mut buf).unwrap();
            assert_eq!(buf, bytes_of(rec), "record {slot}");
        }
    }

    fn bytes_of(record: &serde_json::Value) -> Vec<u8> {
        let raw = record["raw"].as_str().unwrap();
        (0..raw.len() / 2)
            .map(|i| u8::from_str_radix(&raw[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    /// A freshly created AES table is writable: records appended through the
    /// writer encipher, and reopened with the password they read back exactly
    /// (the engine cannot verify this — it refuses AES under Wine — so the
    /// round-trip goes through tflip's own reader, which is golden against the
    /// engine's fixtures).
    #[test]
    fn an_aes_table_round_trips_through_the_writer() {
        use crate::create::{AesMaterial, FieldSpec, aes_adt_bytes};
        use crate::field::FieldType;
        let dir =
            crate::testing::temp_root().join(format!("tflip-aeswrite-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("w.adt");
        let field = |name: &str, field_type, length| FieldSpec {
            name: name.into(),
            field_type,
            length,
            decimals: 0,
            compressed: false,
        };
        let fields = [
            field("id", FieldType::Integer, 0),
            field("word", FieldType::Character, 10),
            field("n", FieldType::Integer, 0),
        ];
        let material = AesMaterial {
            enc_type: crate::adt::ENCRYPTION_AES256,
            salt: [0x11; 32],
            nonce: [0x22; 4],
            descriptor_iv: [0x33; 8],
            iv_counter: 1000,
        };
        let bytes = aes_adt_bytes(&fields, b"secret", &material, Some((".\\w.add", 1))).unwrap();
        std::fs::write(&path, &bytes).unwrap();

        let mut expected = Vec::new();
        {
            let mut table = crate::adt::open_with_password(&path, true, Some(b"secret")).unwrap();
            let len = table.structure.header.record_length as usize;
            for (id, word, n) in [(1i32, &b"alpha     "[..], 25i32), (2, b"beta      ", 74)] {
                let mut rec = vec![0u8; len];
                rec[13..17].copy_from_slice(&id.to_le_bytes());
                rec[17..27].copy_from_slice(word);
                rec[27..31].copy_from_slice(&n.to_le_bytes());
                let slot = table.insert(&rec).unwrap();
                rec[0] = crate::adt::ACTIVE; // the clear form a read returns
                expected.push((slot, rec));
            }
        }
        // The IV counter advanced in the header (0x198) past its seed.
        let reread = std::fs::read(&path).unwrap();
        let counter = u64::from_le_bytes(reread[0x198..0x1A0].try_into().unwrap());
        assert!(counter > 1000, "the IV counter was persisted and advanced");

        let mut table = crate::adt::open_with_password(&path, false, Some(b"secret")).unwrap();
        let names: Vec<&str> = table
            .structure
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names, ["id", "word", "n"]);
        let mut buf = vec![0; table.structure.header.record_length as usize];
        for (slot, want) in &expected {
            table.read(*slot, &mut buf).unwrap();
            assert_eq!(&buf, want, "record {slot}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A free (non-dictionary) AES table opens with its enable password: its
    /// descriptors are clear, its data area AES, no dictionary needed.
    #[test]
    fn opens_a_free_aes_table() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/aes/free/free.adt");
        assert!(matches!(
            crate::adt::open_with_password(&path, false, Some(b"wrong")),
            Err(Error::Encrypted)
        ));
        let table = crate::adt::open_with_password(&path, false, Some(b"mypass")).unwrap();
        let names: Vec<&str> = table
            .structure
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names, ["id", "name"], "clear descriptors read");
        assert_eq!(table.structure.header.record_slots, 0);
    }

    /// The free-AES create trigger end to end in the storage layer: an unarmed
    /// AES table (as a create on an AES connection makes it) is armed by
    /// `enable_encryption_aes`, records appended through the writer encipher, and
    /// reopened with the enable password they read back exactly. A wrong password
    /// is refused. (The live engine round-trip is checked separately with the
    /// patched DLLs; here the round-trip is through tflip's own golden reader.)
    #[test]
    fn a_free_aes_table_arms_and_round_trips() {
        use crate::create::{AesMaterial, FieldSpec, aes_adt_unarmed_bytes};
        use crate::field::FieldType;
        let dir = crate::testing::temp_root().join(format!("tflip-aesarm-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("f.adt");
        let field = |name: &str, field_type, length| FieldSpec {
            name: name.into(),
            field_type,
            length,
            decimals: 0,
            compressed: false,
        };
        let fields = [
            field("id", FieldType::Integer, 0),
            field("name", FieldType::Character, 12),
        ];
        // Create the unarmed table, as an AES connection would.
        std::fs::write(&path, aes_adt_unarmed_bytes(&fields).unwrap()).unwrap();
        // Before arming it is a plain, unencrypted table (type 0, no cipher).
        {
            let plain = crate::adt::open_with_password(&path, false, Some(b"mypass")).unwrap();
            assert_eq!(plain.structure.header.encryption_type, 0);
            assert!(!plain.structure.header.encrypted());
        }
        let material = AesMaterial {
            enc_type: crate::adt::ENCRYPTION_AES128,
            salt: [0x44; 32],
            nonce: [0x55; 4],
            descriptor_iv: [0x66; 8],
            iv_counter: 500,
        };
        let mut expected = Vec::new();
        {
            let mut table = TableFile::open_for_writing(&path, Format::Adt).unwrap();
            table.enable_encryption_aes(&material, b"mypass").unwrap();
            assert!(
                table.structure.header.encrypted(),
                "arming sets the ENCRYPTED flag"
            );
            assert_eq!(
                table.structure.header.encryption_type,
                crate::adt::ENCRYPTION_AES128
            );
            let len = table.structure.header.record_length as usize;
            for (id, name) in [(7i32, &b"seven       "[..]), (8, b"eight       ")] {
                let mut rec = vec![0u8; len];
                rec[13..17].copy_from_slice(&id.to_le_bytes());
                rec[17..29].copy_from_slice(name);
                let slot = table.insert(&rec).unwrap();
                rec[0] = crate::adt::ACTIVE;
                expected.push((slot, rec));
            }
        }
        // The verifier and crypto block landed; a wrong password is refused.
        assert!(matches!(
            crate::adt::open_with_password(&path, false, Some(b"wrong")),
            Err(Error::Encrypted)
        ));
        let mut table = crate::adt::open_with_password(&path, false, Some(b"mypass")).unwrap();
        let mut buf = vec![0; table.structure.header.record_length as usize];
        for (slot, want) in &expected {
            table.read(*slot, &mut buf).unwrap();
            assert_eq!(&buf, want, "record {slot}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Mimics the engine's append path on an armed AES table: insert a blank
    /// record (enciphered once), then overwrite the slot with the filled clear
    /// record (enciphered again, new IV). The stored record must read back to the
    /// clear form — exercising the insert-then-write double pass that the engine
    /// does via AdsAppendRecord + AdsWriteRecord.
    #[test]
    fn aes_insert_then_overwrite_round_trips() {
        use crate::create::{AesMaterial, FieldSpec, aes_adt_unarmed_bytes};
        use crate::field::FieldType;
        let dir = crate::testing::temp_root().join(format!("tflip-aesseq-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("s.adt");
        let field = |name: &str, field_type, length| FieldSpec {
            name: name.into(),
            field_type,
            length,
            decimals: 0,
            compressed: false,
        };
        let fields = [
            field("id", FieldType::Integer, 0),
            field("name", FieldType::Character, 12),
        ];
        std::fs::write(&path, aes_adt_unarmed_bytes(&fields).unwrap()).unwrap();
        let material = AesMaterial {
            enc_type: crate::adt::ENCRYPTION_AES128,
            salt: [0x44; 32],
            nonce: [0x55; 4],
            descriptor_iv: [0x66; 8],
            iv_counter: 500,
        };
        let slot;
        let want;
        {
            let mut table = TableFile::open_for_writing(&path, Format::Adt).unwrap();
            table.enable_encryption_aes(&material, b"mypass").unwrap();
            let len = table.structure.header.record_length as usize;
            // Append: a blank record first (as insert_empty does).
            let blank = table.structure.null_record();
            slot = table.insert(&blank).unwrap();
            // Then overwrite that slot with the filled record (as write_record does).
            let mut rec = vec![0u8; len];
            rec[13..17].copy_from_slice(&42i32.to_le_bytes());
            rec[17..29].copy_from_slice(b"forty-two   ");
            table.write(slot, &rec).unwrap();
            rec[0] = crate::adt::ACTIVE;
            want = rec;
        }
        let mut table = crate::adt::open_with_password(&path, false, Some(b"mypass")).unwrap();
        let mut buf = vec![0; table.structure.header.record_length as usize];
        table.read(slot, &mut buf).unwrap();
        assert_eq!(buf, want);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two opens of a file lock as two processes do.
    #[test]
    fn opens_exclude_each_other() {
        let copy = Copy::of("freelist", "fl_reuse.adt");
        let mut a = TableFile::open_for_writing(&copy.0, Format::Adt).unwrap();
        let mut b = TableFile::open_for_writing(&copy.0, Format::Adt).unwrap();
        assert!(a.lock_record(3).unwrap());
        if cfg!(unix) {
            assert!(a.lock_record(3).unwrap(), "a lock this open holds already");
        }
        assert!(!b.lock_record(3).unwrap());
        assert!(b.lock_record(4).unwrap());
        assert!(!a.lock_table().unwrap(), "b holds a record");
        b.unlock_record(4).unwrap();
        a.unlock_record(3).unwrap();
        assert!(a.lock_table().unwrap());
        assert!(!b.lock_record(1).unwrap());
        a.unlock_table().unwrap();
        assert!(b.lock_record(1).unwrap());
        b.unlock_record(1).unwrap();
        // Shared opens go together; an exclusive one goes alone.
        assert!(a.lock_open(false).unwrap());
        assert!(b.lock_open(false).unwrap());
        b.unlock_open().unwrap();
        a.unlock_open().unwrap();
        assert!(a.lock_open(true).unwrap());
        assert!(!b.lock_open(false).unwrap());
        a.unlock_open().unwrap();
        // Appends from both: each write reads the header the other left.
        let record = a.structure.null_record();
        a.begin_write().unwrap();
        let first = a.insert(&record).unwrap();
        a.end_write().unwrap();
        b.begin_write().unwrap();
        let second = b.insert(&record).unwrap();
        b.end_write().unwrap();
        assert_ne!(first, second);
        a.begin_write().unwrap();
        let third = a.insert(&record).unwrap();
        a.end_write().unwrap();
        assert!(third != first && third != second);
        assert_eq!(a.slots(), b.slots().max(third));
    }
}

/// The records of a table after `PrepareDiff`, against the bytes the
/// original left (`fixtures/backup/s50_diff`, `s52_diff_reinit`).
#[cfg(test)]
mod differential_tests {
    use std::path::{Path, PathBuf};

    use super::*;

    fn fixture(scenario: &str, folder: &str, name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/backup")
            .join(scenario)
            .join("tree")
            .join(folder)
            .join(name)
    }

    /// A copy of a fixture table (and its memo file) in a fresh folder.
    fn copy(scenario: &str, folder: &str, name: &str, test: &str) -> PathBuf {
        let dir = crate::testing::temp_root().join(format!("tflip-diff-{test}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(fixture(scenario, folder, name), dir.join(name)).unwrap();
        let memo = Path::new(name).with_extension("adm");
        if let Ok(from) = std::fs::read(fixture(scenario, folder, memo.to_str().unwrap())) {
            std::fs::write(dir.join(memo), from).unwrap();
        }
        dir.join(name)
    }

    fn statuses(table: &mut TableFile) -> Vec<u8> {
        (1..=table.slots())
            .map(|slot| table.status_byte(slot).unwrap())
            .collect()
    }

    #[test]
    fn cleared_records_read_as_active_and_deleted() {
        let path = fixture("s50_diff", "srcB", "t_adt.adt");
        let mut table = TableFile::open(&path, Format::Adt).unwrap();
        assert_eq!(statuses(&mut table), [0, 0, 1, 0, 0, 1, 0, 0, 0, 0]);
        let deleted: Vec<u32> = (1..=table.slots())
            .filter(|&slot| table.status(slot).unwrap() == RecordStatus::Deleted)
            .collect();
        assert_eq!(deleted, [3, 6]);
        assert!(table.changed_slots().unwrap().is_empty());
        assert_eq!(table.backup_stamp().unwrap(), 0x6aede112);
        // The record's data reads as before: the id is the first field.
        let field = table.structure.fields[0].clone();
        let record = table.record(2).unwrap();
        assert_eq!(
            field.bytes(&record),
            [2, 0, 0, 0],
            "a cleared record keeps its data"
        );
    }

    #[test]
    fn a_cleared_dictionary_reads_as_before() {
        let before = crate::dd::Dictionary::open(&fixture("s50_diff", "srcA", "d.add")).unwrap();
        let after = crate::dd::Dictionary::open(&fixture("s50_diff", "srcB", "d.add")).unwrap();
        assert!(!after.objects().is_empty());
        assert_eq!(after.objects().len(), before.objects().len());
    }

    #[test]
    fn clearing_gives_the_bytes_of_prepare_diff() {
        for (name, stamp) in [("t_adt.adt", 0x6aede112), ("d.add", 0x6aede112)] {
            let path = copy("s50_diff", "srcA", name, &format!("clear-{name}"));
            let mut table = match name {
                "d.add" => adt::open_signed(&path, true, crate::dd::SIGNATURE, false),
                _ => TableFile::open_for_writing(&path, Format::Adt),
            }
            .unwrap();
            assert!(table.changed_slots().unwrap().len() as u32 == table.slots());
            table.clear_changed(stamp).unwrap();
            assert!(table.changed_slots().unwrap().is_empty());
            drop(table);
            assert_eq!(
                std::fs::read(&path).unwrap(),
                std::fs::read(fixture("s50_diff", "srcB", name)).unwrap(),
                "{name}"
            );
        }
    }

    #[test]
    fn writes_set_the_changed_bit_again() {
        let path = copy("s50_diff", "srcB", "t_adt.adt", "writes");
        let mut table = TableFile::open_for_writing(&path, Format::Adt).unwrap();
        let record = table.record(2).unwrap();
        table.write(2, &record).unwrap();
        table.delete(5, None).unwrap();
        // Slot 3 is deleted and on no free list here: recall by slot.
        table.recall(3).unwrap();
        assert_eq!(table.changed_slots().unwrap(), [2, 3, 5]);
        assert_eq!(table.status(3).unwrap(), RecordStatus::Active);
        assert_eq!(table.status(5).unwrap(), RecordStatus::Deleted);
        // Only the written records changed: the others keep 0x00/0x01.
        assert_eq!(table.status_byte(1).unwrap(), 0);
        assert_eq!(table.status_byte(6).unwrap(), 1);
        // The first write set the header flag again; the stamp stays until
        // the next clearing.
        assert_eq!(
            table.structure.header.flags & adt::UNPREPARED,
            adt::UNPREPARED
        );
        assert_eq!(table.backup_stamp().unwrap(), 0x6aede112);
        table.clear_changed(0x6aede113).unwrap();
        assert!(table.changed_slots().unwrap().is_empty());
        assert_eq!(table.backup_stamp().unwrap(), 0x6aede113);
    }

    #[test]
    fn enciphering_keeps_the_changed_bit() {
        assert_eq!(adt::enciphered_status(0x00), Some(0x02));
        assert_eq!(adt::enciphered_status(0x05), Some(0x07));
        assert_eq!(adt::enciphered_status(0x06), None);
        assert_eq!(adt::clear_status(0x03), Some(0x01));
        assert_eq!(adt::clear_status(0x07), Some(0x05));
        assert_eq!(adt::clear_status(0x04), None);
        assert!(adt::is_changed(0x05) && adt::is_changed(0x06));
        assert!(!adt::is_changed(0x01) && !adt::is_changed(adt::TX_APPENDED));
        assert!(!adt::is_changed(0x84));
    }
}
