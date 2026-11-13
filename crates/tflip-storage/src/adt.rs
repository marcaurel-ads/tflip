//! ADT tables (`.adt`): the header, the field descriptors, and how values
//! are stored in records.
//!
//! An ADT file is a 400-byte header, an optional header extension, one
//! 200-byte descriptor per field, and then the records. All integers are
//! little-endian. A record starts with a 5-byte prefix, its first byte its
//! status.

use crate::bytes::{u16_at, u32_at};
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::path::Path;

use crate::cp1252;
use crate::field::{Field, FieldType, Layout, MemoRef, utf16};
use crate::memo::MemoFile;
use crate::table::{
    Error, Format, Header, RecordCipher, RecordStatus, Records, Structure, TableFile, flags,
};
use crate::value::Value;

/// The size of the table header.
pub const HEADER_SIZE: usize = 400;

/// The size of a field descriptor.
pub const DESCRIPTOR_SIZE: usize = 200;

/// The signature the header starts with.
const SIGNATURE: &[u8; 16] = b"Advantage Table\0";

/// Header offsets that writes change: the record slots, the head and
/// length of the deleted-record free list, the next AutoInc value and the
/// last RowVersion value given out.
pub(crate) mod offsets {
    pub const RECORD_SLOTS: u64 = 0x18;
    pub const FREE_HEAD: u64 = 0x28;
    pub const FREE_COUNT: u64 = 0x30;
    /// The 20-byte RC4 password check (`b"ADVANTAGE"` enciphered at item 0x45).
    pub const PASSWORD_CHECK: u64 = 0x38;
    /// The encryption type: 0 none, 2 the 40-bit RC4 variant, 3 the default RC4.
    pub const ENCRYPTION_TYPE: u64 = 0x4C;
    pub const NEXT_AUTOINC: u64 = 0x50;
    pub const FLAGS: u64 = 0x58;
    pub const ROW_VERSION: u64 = 0x168;
    /// The differential backup stamp: Unix seconds, set by `PrepareDiff`.
    pub const BACKUP_STAMP: u64 = 0x174;
}

/// The header flag a differential backup's `PrepareDiff` clears; a created
/// table has it (flags 0x11).
pub const UNPREPARED: u32 = 0x10;

/// The clear prefix every record begins with (its first byte the status);
/// in an encrypted table only the fixed-length data area after it is enciphered.
pub const RECORD_PREFIX: usize = 5;

/// The status byte of an active record (unencrypted).
pub const ACTIVE: u8 = 0x04;

/// The status byte of a deleted record (unencrypted).
pub const DELETED: u8 = 0x05;

/// The status byte of an active record in an encrypted table.
pub(crate) const ENCRYPTED_ACTIVE: u8 = 0x06;

/// The status byte of a deleted record in an encrypted table.
pub(crate) const ENCRYPTED_DELETED: u8 = 0x07;

/// The default RC4 encryption type (header 0x4C), keyed by the first 20
/// password bytes (`ADS_ENCRYPTION_RC4`).
pub const ENCRYPTION_RC4: u8 = 3;

/// The 40-bit RC4 encryption type, keyed by the first 5 password bytes.
pub const ENCRYPTION_RC4_40: u8 = 2;

/// The AES-128 encryption type (header 0x4C): a PBKDF2-derived key over a stored
/// salt, AES-CTR, the key material self-contained in the table.
pub const ENCRYPTION_AES128: u8 = 5;

/// The AES-256 encryption type (header 0x4C).
pub const ENCRYPTION_AES256: u8 = 6;

/// An AES record carries a clear 8-byte IV right after the 5-byte prefix, so its
/// field data begins 8 bytes further in than an unencrypted record's.
pub const AES_IV_SIZE: usize = 8;

/// Where the header extension keeps an AES table's IV counter (the next 64-bit
/// IV to hand out), a little-endian `u64`.
pub const AES_IV_COUNTER: u64 = 0x198;

/// Encipher a clear AES record in place: write the 8-byte `iv` into the slot
/// after the 5-byte prefix, flip the status to the enciphered form (0x04 → 0x06,
/// 0x05 → 0x07), and AES-CTR the field data (from offset 13) under `key` and
/// `nonce`. The inverse of the reader's AES decipher (see
/// [`crate::table::RecordCipher`]); the caller passes a record laid out with the
/// IV slot zeroed and the field data clear.
pub fn aes_encipher_record(key: &[u8], nonce: &[u8; 4], iv: &[u8; 8], record: &mut [u8]) {
    let data = RECORD_PREFIX + AES_IV_SIZE;
    if record.len() <= data {
        return;
    }
    record[RECORD_PREFIX..data].copy_from_slice(iv);
    record[0] = match record[0] {
        ACTIVE => ENCRYPTED_ACTIVE,
        DELETED => ENCRYPTED_DELETED,
        other => other,
    };
    crate::crypto::aes_ctr_xor(key, iv, nonce, &mut record[data..]);
}

/// Reads a header.
pub fn parse_header(bytes: &[u8; HEADER_SIZE]) -> Result<Header, Error> {
    parse_header_signed(bytes, SIGNATURE)
}

/// Reads the header of a file in the ADT format that starts with
/// `signature`, such as a data dictionary (see [`crate::dd`]).
pub(crate) fn parse_header_signed(
    bytes: &[u8; HEADER_SIZE],
    signature: &[u8],
) -> Result<Header, Error> {
    if !bytes.starts_with(signature) {
        return Err(Error::Corrupt("not an ADT table"));
    }
    let header = Header {
        format: Format::Adt,
        record_slots: u32_at(bytes, 0x18),
        data_offset: u32_at(bytes, 0x20),
        record_length: u32_at(bytes, 0x24),
        free_head: u32_at(bytes, 0x28),
        free_count: u32_at(bytes, 0x30),
        encryption_type: bytes[0x4C],
        flags: u32_at(bytes, 0x58),
        field_count: u16_at(bytes, 0x166),
        extension: u32_at(bytes, 0x170),
        dictionary: bytes[0x5C] != 0,
    };
    let descriptors_offset = HEADER_SIZE as u64 + u64::from(header.extension);
    let descriptors_size = DESCRIPTOR_SIZE as u64 * u64::from(header.field_count);
    if u64::from(header.data_offset) != descriptors_offset + descriptors_size {
        return Err(Error::Corrupt(
            "data offset does not follow the field descriptors",
        ));
    }
    if header.free_count > header.record_slots {
        return Err(Error::Corrupt("more free records than record slots"));
    }
    Ok(header)
}

/// The header as stored, the reverse of [`parse_header`]: the fields this
/// crate knows, and zeros elsewhere.
pub fn header_bytes(header: &Header) -> [u8; HEADER_SIZE] {
    let mut bytes = [0; HEADER_SIZE];
    bytes[..16].copy_from_slice(SIGNATURE);
    bytes[0x18..0x1C].copy_from_slice(&header.record_slots.to_le_bytes());
    bytes[0x20..0x24].copy_from_slice(&header.data_offset.to_le_bytes());
    bytes[0x24..0x28].copy_from_slice(&header.record_length.to_le_bytes());
    bytes[0x28..0x2C].copy_from_slice(&header.free_head.to_le_bytes());
    bytes[0x30..0x34].copy_from_slice(&header.free_count.to_le_bytes());
    bytes[0x58..0x5C].copy_from_slice(&header.flags.to_le_bytes());
    bytes[0x166..0x168].copy_from_slice(&header.field_count.to_le_bytes());
    bytes[0x170..0x174].copy_from_slice(&header.extension.to_le_bytes());
    bytes
}

/// Reads a field descriptor.
pub fn parse_field(bytes: &[u8; DESCRIPTOR_SIZE]) -> Field {
    let name = &bytes[..128];
    let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
    Field {
        name: cp1252::decode(name),
        field_type: FieldType::from_code(u16_at(bytes, 129)),
        offset: u16_at(bytes, 131),
        size: u16_at(bytes, 135),
        decimals_slot: u16_at(bytes, 139),
        layout: Layout::Adt,
    }
}

/// The descriptor as stored, the reverse of [`parse_field`].
pub fn field_bytes(field: &Field) -> [u8; DESCRIPTOR_SIZE] {
    let mut bytes = [0; DESCRIPTOR_SIZE];
    let name = cp1252::encode(&field.name);
    let name = &name[..name.len().min(128)];
    bytes[..name.len()].copy_from_slice(name);
    bytes[129..131].copy_from_slice(&field.field_type.code().to_le_bytes());
    bytes[131..133].copy_from_slice(&field.offset.to_le_bytes());
    bytes[135..137].copy_from_slice(&field.size.to_le_bytes());
    bytes[139..141].copy_from_slice(&field.decimals_slot.to_le_bytes());
    bytes
}

/// A field of an ADT table, of type `field_type` at `offset` in a record,
/// `length` long as declared (characters for the Unicode types) with
/// `decimals`.
pub fn field(name: &str, field_type: FieldType, offset: u16, length: u16, decimals: u16) -> Field {
    Field {
        name: name.to_owned(),
        field_type,
        offset,
        size: field_size(field_type, length),
        decimals_slot: decimals,
        layout: Layout::Adt,
    }
}

/// How many bytes a field of type `field_type`, `length` long as declared,
/// takes in a record.
pub fn field_size(field_type: FieldType, length: u16) -> u16 {
    match field_type {
        FieldType::Logical => 1,
        FieldType::ShortInt => 2,
        FieldType::Date | FieldType::Time | FieldType::Integer | FieldType::AutoInc => 4,
        FieldType::Double
        | FieldType::CurDouble
        | FieldType::Money
        | FieldType::LongInt
        | FieldType::TimeStamp
        | FieldType::ModTime
        | FieldType::RowVersion => 8,
        FieldType::Memo | FieldType::Binary | FieldType::Image | FieldType::NMemo => 9,
        FieldType::Guid => 16,
        FieldType::VarChar | FieldType::VarBinary => length + 2,
        FieldType::NChar => 2 * length,
        FieldType::NVarChar => 2 * length + 2,
        _ => length,
    }
}

/// The declared length of `field`: the stored size, except for the
/// variable-length and Unicode types, which store more.
pub(crate) fn length(field: &Field) -> u16 {
    match field.field_type {
        FieldType::VarChar | FieldType::VarBinary => field.size.saturating_sub(2),
        FieldType::NChar => field.size / 2,
        FieldType::NVarChar => field.size.saturating_sub(2) / 2,
        _ => field.size,
    }
}

/// The memo reference memo field `field` holds in `record`. A reference is
/// 9 bytes (first block, length), or 16 in a compressed field (first block,
/// stored length, 0, uncompressed length); block 0 is NULL.
pub(crate) fn memo_ref(field: &Field, record: &[u8]) -> MemoRef {
    let bytes = field.bytes(record);
    let block = u32_at(bytes, 0);
    let (stored, length) = match bytes.len() {
        16 => (u32_at(bytes, 4), u32_at(bytes, 12)),
        _ => (u32_at(bytes, 4), u32_at(bytes, 4)),
    };
    match block {
        0 => MemoRef {
            block: 0,
            stored: 0,
            length: 0,
        },
        _ => MemoRef {
            block,
            stored,
            length,
        },
    }
}

/// The value memo field `field` holds in `record`, as stored (decompressed;
/// UTF-16LE bytes for NMemo), read from `memo`; empty for NULL.
pub(crate) fn memo_data(
    field: &Field,
    record: &[u8],
    memo: Option<&mut MemoFile>,
) -> Result<Vec<u8>, Error> {
    let memo_ref = memo_ref(field, record);
    if memo_ref.block == 0 {
        return Ok(Vec::new());
    }
    let memo = memo.ok_or(Error::Corrupt("memo field without a memo file"))?;
    memo.read(memo_ref)
}

/// The value `field` holds in `record`. Memo values are read from `memo`,
/// the table's memo file; a memo field without one is an error.
pub(crate) fn value(
    field: &Field,
    record: &[u8],
    memo: Option<&mut MemoFile>,
) -> Result<Value, Error> {
    // A RowVersion set to NULL reads as its number (`i64::MIN`), though it
    // is NULL, as in the original engine.
    if is_null(field, record) && field.field_type != FieldType::RowVersion {
        return Ok(Value::Null);
    }
    let bytes = field.bytes(record);
    let i32_at = |offset| i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let i64_at = |offset| i64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    Ok(match field.field_type {
        FieldType::Character | FieldType::CiCharacter => Value::Text(cp1252::decode(bytes)),
        FieldType::VarChar => {
            let length = usize::from(var_length(field, record).unwrap());
            Value::Text(cp1252::decode(&bytes[..length.min(bytes.len() - 2)]))
        }
        FieldType::NChar => Value::Text(utf16(bytes)),
        FieldType::NVarChar => {
            let units = usize::from(var_length(field, record).unwrap());
            Value::Text(utf16(&bytes[..(2 * units).min(bytes.len() - 2)]))
        }
        FieldType::Memo | FieldType::NMemo | FieldType::Binary | FieldType::Image => {
            let memo_file = memo.ok_or(Error::Corrupt("memo field without a memo file"))?;
            let data = memo_file.read(memo_ref(field, record))?;
            match field.field_type {
                FieldType::Memo => Value::Text(cp1252::decode(&data)),
                FieldType::NMemo => Value::Text(utf16(&data)),
                _ => Value::Binary(data),
            }
        }
        FieldType::LegacyVarChar => {
            Value::Text(cp1252::decode(&legacy_var_char(field, record, memo)?))
        }
        FieldType::Raw | FieldType::CompactDate | FieldType::Other(_) => {
            Value::Binary(bytes.to_vec())
        }
        FieldType::VarBinary => {
            let length = usize::from(var_length(field, record).unwrap());
            Value::Binary(bytes[..length.min(bytes.len() - 2)].to_vec())
        }
        FieldType::Logical => Value::Logical(matches!(bytes[0], b'T' | b't' | b'Y' | b'y')),
        FieldType::Numeric => Value::Numeric(bytes.iter().map(|&b| char::from(b)).collect()),
        FieldType::ShortInt => Value::Integer(i16::from_le_bytes([bytes[0], bytes[1]]).into()),
        FieldType::Integer => Value::Integer(i32_at(0).into()),
        FieldType::AutoInc => Value::Integer((i32_at(0) as u32).into()),
        FieldType::LongInt => Value::Integer(i64_at(0).into()),
        FieldType::RowVersion => Value::Integer(i64_at(0).into()),
        FieldType::Double | FieldType::CurDouble => Value::Double(f64::from_bits(i64_at(0) as u64)),
        FieldType::Money => Value::Money(i64_at(0)),
        FieldType::Date => Value::Date(i32_at(0)),
        FieldType::Time => Value::Time(i32_at(0) as u32),
        FieldType::TimeStamp | FieldType::ModTime => Value::TimeStamp(i32_at(0), i32_at(4) as u32),
        FieldType::Guid => Value::Guid(bytes.try_into().unwrap()),
    })
}

/// The stored length of a VarChar or VarBinary (bytes) or NVarChar (UTF-16
/// code units) field in `record`: the u16 after the data, 0xFFFF for NULL.
/// `None` for other fields.
pub(crate) fn var_length(field: &Field, record: &[u8]) -> Option<u16> {
    match field.field_type {
        FieldType::VarChar | FieldType::VarBinary | FieldType::NVarChar => {
            let bytes = field.bytes(record);
            Some(u16_at(bytes, bytes.len() - 2))
        }
        _ => None,
    }
}

/// Whether `field` holds NULL in `record`. ADT tables have no NULL bitmap;
/// each type has its own NULL value.
pub(crate) fn is_null(field: &Field, record: &[u8]) -> bool {
    let bytes = field.bytes(record);
    let zero = || bytes.iter().all(|&b| b == 0);
    let tail_length = || bytes.len() >= 2 && bytes[bytes.len() - 2..] == [0xFF, 0xFF];
    match field.field_type {
        FieldType::Logical => bytes == b" ",
        FieldType::Double | FieldType::CurDouble => bytes == 0x8000_0000_0000_0020u64.to_le_bytes(),
        FieldType::Integer => bytes == i32::MIN.to_le_bytes(),
        FieldType::ShortInt => bytes == i16::MIN.to_le_bytes(),
        FieldType::Money | FieldType::LongInt | FieldType::RowVersion => {
            bytes == i64::MIN.to_le_bytes()
        }
        FieldType::Time => bytes == [0xFF; 4],
        FieldType::AutoInc => false,
        // A TimeStamp without a date is NULL, whatever its time.
        FieldType::TimeStamp | FieldType::ModTime => bytes[..4] == [0; 4],
        FieldType::VarChar | FieldType::VarBinary | FieldType::NVarChar => tail_length(),
        FieldType::NChar => bytes.len() >= 2 && bytes[..2] == [0, 0],
        // Spaces are the blank record an AES table gives a handle without
        // its password (`fixtures/aes/api/01_types`).
        FieldType::LegacyVarChar => zero() || bytes.iter().all(|&b| b == b' '),
        // Numeric, Date, Character, CICharacter, the memo types, Raw and
        // GUID: all zero bytes.
        _ => zero(),
    }
}

/// The bytes of `field` in an empty record: its NULL value, as the original
/// engine fills them in. AutoInc and ModTime are zeros and RowVersion is the
/// NULL value of LongInt, though these three are never NULL in a stored
/// record.
pub(crate) fn null_bytes(field: &Field) -> Vec<u8> {
    let mut bytes = vec![0; usize::from(field.size)];
    let value: &[u8] = match field.field_type {
        FieldType::Logical => b" ",
        FieldType::Double | FieldType::CurDouble => &0x8000_0000_0000_0020u64.to_le_bytes(),
        FieldType::Integer => &i32::MIN.to_le_bytes(),
        FieldType::ShortInt => &i16::MIN.to_le_bytes(),
        FieldType::Money | FieldType::LongInt | FieldType::RowVersion => &i64::MIN.to_le_bytes(),
        FieldType::Time => &[0xFF; 4],
        _ => &[],
    };
    bytes[..value.len()].copy_from_slice(value);
    // The VarChar types end in their stored length, 0xFFFF for NULL.
    if matches!(
        field.field_type,
        FieldType::VarChar | FieldType::VarBinary | FieldType::NVarChar
    ) {
        let end = bytes.len();
        bytes[end - 2..].fill(0xFF);
    }
    bytes
}

/// The status stored in a record's first byte: bit 0 is the deleted flag,
/// bit 1 the enciphered flag (0x06 and 0x07 in an encrypted table) and bit 2
/// ([`CHANGED`]) is set by every write. A differential backup's `PrepareDiff`
/// clears it, so 0x00/0x01 (0x02/0x03 enciphered) are the active and deleted
/// records nothing has written since (`fixtures/backup/s50_diff`). Tables
/// written by older versions (as in the sample data shipped with ADS 12)
/// mark active records 0x00 too. A record a transaction changed or deleted
/// has bit 7 set ([`TX_MARK`]) and is what it was; one it appended
/// ([`TX_APPENDED`]) is not there for anyone else yet.
pub(crate) fn record_status(byte: u8) -> Option<RecordStatus> {
    match byte {
        TX_APPENDED => Some(RecordStatus::Deleted),
        _ if byte & TX_MARK != 0 => record_status(byte & !TX_MARK),
        0x00..=0x07 if byte & 1 == 0 => Some(RecordStatus::Active),
        0x00..=0x07 => Some(RecordStatus::Deleted),
        _ => None,
    }
}

/// The bit of a record's status byte that every write of the record sets and
/// a differential backup's `PrepareDiff` clears.
pub const CHANGED: u8 = 0x04;

/// The bit of a record's status byte that marks its data enciphered.
const ENCIPHERED: u8 = 0x02;

/// Whether the status byte `byte` of a record in the file says it was
/// written since the changed bits were last cleared. A record a transaction
/// marked or appended is not counted: it is not written yet.
pub fn is_changed(byte: u8) -> bool {
    byte <= 0x07 && byte & CHANGED != 0
}

/// Whether the status byte `byte` marks a record enciphered, a transaction
/// mark aside.
pub(crate) fn is_enciphered(byte: u8) -> bool {
    let byte = byte & !TX_MARK;
    byte <= 0x07 && byte & ENCIPHERED != 0
}

/// The enciphered form of an unenciphered record's status byte (its changed
/// bit as it is), `None` for any other byte.
pub(crate) fn enciphered_status(byte: u8) -> Option<u8> {
    (byte <= 0x07 && byte & ENCIPHERED == 0).then_some(byte | ENCIPHERED)
}

/// The clear form of an enciphered record's status byte, `None` for any
/// other byte.
pub(crate) fn clear_status(byte: u8) -> Option<u8> {
    (byte <= 0x07 && byte & ENCIPHERED != 0).then_some(byte & !ENCIPHERED)
}

/// Whether the status byte `byte` marks an active record whose data is in
/// the clear.
pub fn is_active_clear(byte: u8) -> bool {
    byte <= 0x07 && byte & (ENCIPHERED | 1) == 0
}

/// Whether the status byte `byte` marks a deleted record whose data is in
/// the clear.
pub(crate) fn is_deleted_clear(byte: u8) -> bool {
    byte <= 0x07 && byte & (ENCIPHERED | 1) == 1
}

/// The bit of a record's status byte that marks it as written in a
/// transaction not ended yet, as the original's server marks it (the next
/// byte holds the transaction's number); its data is what it was before.
pub const TX_MARK: u8 = 0x80;

/// The status of a blank record a transaction appended, not there for
/// anyone else until the commit (the next byte holds the transaction's
/// number).
pub const TX_APPENDED: u8 = 0x11;

/// Reads the structure of an ADT table from the start of `reader`.
pub fn read_structure(reader: &mut impl Read) -> Result<Structure, Error> {
    read_structure_signed(reader, SIGNATURE)
}

/// Reads the structure of a file in the ADT format that starts with
/// `signature` (see [`parse_header_signed`]).
pub(crate) fn read_structure_signed(
    reader: &mut impl Read,
    signature: &[u8],
) -> Result<Structure, Error> {
    let mut bytes = [0; HEADER_SIZE];
    reader.read_exact(&mut bytes)?;
    let header = parse_header_signed(&bytes, signature)?;
    if header.flags & flags::ENCRYPTED_DESCRIPTORS != 0 {
        return Err(Error::EncryptedDescriptors);
    }
    io::copy(
        &mut reader.take(u64::from(header.extension)),
        &mut io::sink(),
    )?;

    let mut fields = Vec::with_capacity(usize::from(header.field_count));
    let mut next = 5;
    for _ in 0..header.field_count {
        let mut bytes = [0; DESCRIPTOR_SIZE];
        reader
            .read_exact(&mut bytes)
            .map_err(|_| Error::Corrupt("file ends inside the field descriptors"))?;
        let field = parse_field(&bytes);
        if u32::from(field.offset) < next {
            return Err(Error::Corrupt("fields overlap"));
        }
        next = u32::from(field.offset) + u32::from(field.size);
        fields.push(field);
    }
    if next > header.record_length {
        return Err(Error::Corrupt("fields extend past the record length"));
    }
    Ok(Structure { header, fields })
}

/// Opens the ADT table at `path` (see [`TableFile::open`]), for writing if
/// `write`.
pub(crate) fn open(path: &Path, write: bool) -> Result<TableFile, Error> {
    open_signed(path, write, SIGNATURE, true)
}

/// [`open`] for a file in the ADT format that starts with `signature`, such
/// as a data dictionary read as a table (to build its index). The structure
/// comes out as an ordinary ADT one ([`Format::Adt`]); only reading is meant
/// for now, as writing would rewrite the header with the table signature. The
/// memo file is left closed (`with_memo` false) when the caller does not read
/// memo fields — a dictionary's memo is `.am`, not the `.adm` a table's is.
pub(crate) fn open_signed(
    path: &Path,
    write: bool,
    signature: &[u8],
    with_memo: bool,
) -> Result<TableFile, Error> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(write)
        .open(path)
        .map_err(Error::Io)?;
    let structure = read_structure_signed(&mut file, signature)?;
    let records = Records::new(file, &structure.header);
    let memo = match with_memo && structure.header.flags & flags::MEMO != 0 {
        false => None,
        true => {
            let mut memo = MemoFile::open_adm(&MemoFile::path_for(path), write)?;
            // A table laid out for AES keeps its memo values and free runs
            // behind IVs even in the clear (`fixtures/aes/dictwrite`).
            memo.set_iv_prefix(structure.header.extension as usize == crate::create::AES_EXTENSION);
            Some(memo)
        }
    };
    Ok(TableFile::new(structure, records, memo, None).writable(write))
}

/// Opens the ADT table at `path`, deciphering it with `password` when it is
/// encrypted. Without a password, or for an encryption not handled here yet
/// (RC4), it behaves like [`open`]. AES tables (self-contained: a PBKDF2 key
/// over a stored salt, stored IVs) open fully, their field descriptors and
/// records deciphered, when the password is right.
pub(crate) fn open_with_password(
    path: &Path,
    write: bool,
    password: Option<&[u8]>,
) -> Result<TableFile, Error> {
    let Some(password) = password else {
        return open(path, write);
    };
    let mut file = OpenOptions::new()
        .read(true)
        .write(write)
        .open(path)
        .map_err(Error::Io)?;
    let mut head = [0; HEADER_SIZE];
    file.read_exact(&mut head)?;
    let header = parse_header(&head)?;
    // A whole-table encryption sets the ENCRYPTED flag; record-level RC4
    // encryption (`AdsEncryptRecord`, or a table merely enabled) leaves the flag
    // clear but marks the header with the RC4 type and a password check, and
    // enciphers only individual records (0x06/0x07) — not the memo file. Open
    // either; anything else (type 0) is a plain table.
    let table_level = header.encrypted();
    let record_level_rc4 = !table_level && matches!(head[0x4C], 2 | 3);
    if !table_level && !record_level_rc4 {
        return open(path, write);
    }

    // The bytes between the header and the first record: the header extension
    // (which for AES holds the crypto fields) then the field descriptors.
    let extension = header.extension as usize;
    let mut region = vec![0u8; header.data_offset as usize - HEADER_SIZE];
    file.read_exact(&mut region)
        .map_err(|_| Error::Corrupt("file ends before the first record"))?;
    let encrypted_descriptors = header.flags & flags::ENCRYPTED_DESCRIPTORS != 0;

    // Decipher the descriptors (if encrypted) and settle the record cipher.
    // The encryption type at header 0x4C tells the scheme apart: 2/3 are the RC4
    // family (40-bit and default), 5/6 are AES-128/256.
    // An AES table carries its write IV counter in the header (0x198); it is
    // shared with the memo file and persisted as writes advance it.
    let mut aes_counter: Option<u64> = None;
    let cipher = if matches!(head[0x4C], 5 | 6) {
        let at = |offset: usize| offset - HEADER_SIZE;
        let key_len = if head[0x4C] == 5 { 16 } else { 32 };
        let nonce: [u8; 4] = region[at(0x194)..at(0x194) + 4].try_into().unwrap();
        let salt = &region[at(0x1A0)..at(0x1A0) + 32];
        let descriptor_iv: [u8; 8] = region[at(0x1C0)..at(0x1C0) + 8].try_into().unwrap();
        aes_counter = Some(u64::from_le_bytes(
            region[at(0x198)..at(0x198) + 8].try_into().unwrap(),
        ));
        let derived = crate::crypto::aes_derive(password, salt, key_len);
        let key = derived[..key_len].to_vec();
        if derived[key_len..key_len + 20] != head[0x38..0x4C] {
            return Err(Error::Encrypted); // the password's verifier does not match
        }
        if encrypted_descriptors {
            crate::crypto::aes_ctr_xor(&key, &descriptor_iv, &nonce, &mut region[extension..]);
        }
        RecordCipher::Aes { key, nonce }
    } else {
        // RC4: the 40-bit variant (type 2) keys on the first five password
        // bytes, the default (type 3) on the first twenty.
        let key_bytes = match head[0x4C] {
            2 => &password[..password.len().min(5)],
            _ => &password[..password.len().min(crate::crypto::KEY_MAX)],
        };
        if !crate::crypto::rc4_password_ok(key_bytes, head[0x38..0x4C].try_into().unwrap()) {
            return Err(Error::Encrypted); // the password check does not match
        }
        if encrypted_descriptors {
            // The whole header-to-data region is one stream at item 0x45·fields.
            let item = 0x45 * u32::from(header.field_count);
            crate::crypto::rc4_crypt(key_bytes, item, &mut region);
        }
        RecordCipher::Rc4(key_bytes.to_vec())
    };

    let descriptors = &region[extension..];
    let mut fields = Vec::with_capacity(usize::from(header.field_count));
    let mut next = 5;
    let (chunks, _) = descriptors.as_chunks::<DESCRIPTOR_SIZE>();
    for chunk in chunks.iter().take(usize::from(header.field_count)) {
        let field = parse_field(chunk);
        if u32::from(field.offset) < next {
            return Err(Error::Corrupt("fields overlap"));
        }
        next = u32::from(field.offset) + u32::from(field.size);
        fields.push(field);
    }
    if next > header.record_length {
        return Err(Error::Corrupt("fields extend past the record length"));
    }
    let structure = Structure { header, fields };

    let mut records = Records::new(file, &structure.header);
    // Seed the shared AES write IV counter from the header and give records and
    // the memo file the one counter.
    let counter = aes_counter.map(|v| std::sync::Arc::new(std::sync::atomic::AtomicU64::new(v)));
    if let Some(counter) = &counter {
        records.set_aes_counter(std::sync::Arc::clone(counter));
    }
    let memo = match structure.header.flags & flags::MEMO {
        0 => None,
        _ => {
            let mut memo = MemoFile::open_adm(&MemoFile::path_for(path), write)?;
            memo.set_iv_prefix(structure.header.extension as usize == crate::create::AES_EXTENSION);
            // Record-level RC4 encryption leaves memo values in the clear, so
            // only a whole-table encryption gives the memo file a cipher.
            if table_level {
                memo.set_cipher(cipher.clone());
                if let Some(counter) = &counter {
                    memo.set_aes_counter(std::sync::Arc::clone(counter));
                }
            }
            Some(memo)
        }
    };
    records.set_cipher(cipher);
    Ok(TableFile::new(structure, records, memo, None).writable(write))
}

/// Writes `value` into `slot`, the bytes of `field` in a record, as an ADT
/// table stores it: NULL as the type's NULL value (in an NChar field only
/// its first character, as the original engine does when it updates a
/// field to NULL), and text and binary values cut to the field's size.
/// Memo fields, whose values are in the memo file, are not written here.
pub fn encode(field: &Field, value: &Value, slot: &mut [u8]) -> Result<(), Error> {
    let mismatch = || Error::Corrupt("value does not match the field type");
    if *value == Value::Null {
        match field.field_type {
            FieldType::NChar => {
                let first = slot.len().min(2);
                slot[..first].fill(0);
            }
            _ => slot.copy_from_slice(&null_bytes(field)),
        }
        return Ok(());
    }
    match (field.field_type, value) {
        (FieldType::Character | FieldType::CiCharacter, Value::Text(text)) => {
            padded(slot, &cp1252::encode(text), b' ');
        }
        (FieldType::VarChar, Value::Text(text)) => variable(slot, &cp1252::encode(text), 1, 0),
        (FieldType::VarBinary, Value::Binary(data)) => variable(slot, data, 1, 0),
        (FieldType::NChar, Value::Text(text)) => {
            let units: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
            padded(slot, &units, 0);
            // The rest is spaces.
            let used = units.len().min(slot.len()) & !1;
            for unit in slot[used..].as_chunks_mut::<2>().0 {
                *unit = 0x20u16.to_le_bytes();
            }
        }
        (FieldType::NVarChar, Value::Text(text)) => {
            let units: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
            variable(slot, &units, 2, 0x20);
        }
        (FieldType::Raw, Value::Binary(data)) => padded(slot, data, 0),
        (FieldType::Logical, Value::Logical(b)) => slot[0] = if *b { b'T' } else { b'F' },
        (FieldType::Numeric, Value::Numeric(text)) => {
            let text = text.as_bytes();
            if text.len() > slot.len() {
                return Err(Error::Corrupt("numeric value too long for its field"));
            }
            slot.fill(b' ');
            let start = slot.len() - text.len();
            slot[start..].copy_from_slice(text);
        }
        (FieldType::ShortInt, Value::Integer(n)) => {
            let n = i16::try_from(*n).map_err(|_| mismatch())?;
            slot.copy_from_slice(&n.to_le_bytes());
        }
        (FieldType::Integer, Value::Integer(n)) => {
            let n = i32::try_from(*n).map_err(|_| mismatch())?;
            slot.copy_from_slice(&n.to_le_bytes());
        }
        (FieldType::AutoInc, Value::Integer(n)) => {
            let n = u32::try_from(*n).map_err(|_| mismatch())?;
            slot.copy_from_slice(&n.to_le_bytes());
        }
        (FieldType::LongInt, Value::Integer(n)) => {
            let n = i64::try_from(*n).map_err(|_| mismatch())?;
            slot.copy_from_slice(&n.to_le_bytes());
        }
        (FieldType::RowVersion, Value::Integer(n)) => {
            let n = u64::try_from(*n).map_err(|_| mismatch())?;
            slot.copy_from_slice(&n.to_le_bytes());
        }
        (FieldType::Double | FieldType::CurDouble, Value::Double(d)) => {
            slot.copy_from_slice(&d.to_le_bytes());
        }
        (FieldType::Money, Value::Money(m)) => slot.copy_from_slice(&m.to_le_bytes()),
        (FieldType::Date, Value::Date(day)) => slot.copy_from_slice(&day.to_le_bytes()),
        (FieldType::Time, Value::Time(ms)) => slot.copy_from_slice(&ms.to_le_bytes()),
        (FieldType::TimeStamp | FieldType::ModTime, Value::TimeStamp(day, ms)) => {
            slot[..4].copy_from_slice(&day.to_le_bytes());
            slot[4..].copy_from_slice(&ms.to_le_bytes());
        }
        (FieldType::Guid, Value::Guid(guid)) => slot.copy_from_slice(guid),
        _ => return Err(mismatch()),
    }
    Ok(())
}

/// The shortest value a COMPRESSED memo field stores compressed.
const COMPRESS_MIN: usize = 128;

/// Stores the value of memo field `field` (as the memo file keeps it: the
/// text's bytes, UTF-16LE for NMemo; `None` for NULL) in `memo`, and its
/// reference in `slot`, the field's bytes in the record, which hold the
/// reference to the old value. As the original engine does it: a value that
/// takes as many blocks as the old one is written over it; else the old one
/// is freed and the new one allocated. An empty value is NULL, and NULL
/// frees nothing (the old value's blocks stay taken). A COMPRESSED field
/// keeps a value of 128 bytes or more as a zlib stream (level 1) if that
/// takes fewer blocks, counting one byte more for both.
pub fn store_memo(
    field: &Field,
    value: Option<&[u8]>,
    slot: &mut [u8],
    memo: &mut MemoFile,
) -> Result<(), Error> {
    let full = value.unwrap_or_default();
    let block_size = memo.block_size();
    // On an encrypted table the stored bytes are enciphered: RC4 at the record
    // number (the same item reads decipher at), or AES-CTR under an 8-byte IV
    // stored before the value (see [`crate::memo`]). Both the cipher and the
    // shared IV counter are taken before the allocator borrows the file.
    let cipher = memo.rc4_write();
    let aes = memo.aes_cipher();
    let aes_counter = memo.aes_counter();
    // An AES value stores an 8-byte IV before its ciphertext (zero in the
    // clear), so it takes 8 more bytes on disk than it records as its stored
    // length.
    let overhead = memo.value_overhead();
    let mut allocator = memo.allocator()?;
    let compressed;
    let data = match field.compressed() && full.len() >= COMPRESS_MIN {
        true => {
            compressed = zlib(full)?;
            let blocks = |n: usize| (n as u32).div_ceil(block_size);
            match blocks(compressed.len() + 1) < blocks(full.len() + 1) {
                true => &compressed[..],
                false => full,
            }
        }
        false => full,
    };
    let (block, length) = (u32_at(slot, 0), u32_at(slot, 4));
    let length_of = |n: usize| u32::try_from(n).map_err(|_| Error::Corrupt("memo value too long"));
    let stored = length_of(data.len())?;
    // Allocation and free work in on-disk bytes (the IV overhead included); the
    // reference records only the ciphertext length.
    let physical = stored + overhead;
    let old_physical = length + overhead;
    let new = match (block, data.is_empty()) {
        (_, true) => 0,
        (0, false) => allocator.alloc(physical)?,
        (old, false) if allocator.blocks(old_physical) == allocator.blocks(physical) => old,
        (old, false) => {
            allocator.free(old, old_physical)?;
            allocator.alloc(physical)?
        }
    };
    if new != 0 {
        match (&aes, &cipher) {
            (Some((key, nonce)), _) => {
                let blocks = u64::from(stored.div_ceil(16).max(1));
                let iv = aes_counter
                    .fetch_add(blocks, std::sync::atomic::Ordering::Relaxed)
                    .to_le_bytes();
                let mut buf = Vec::with_capacity(AES_IV_SIZE + data.len());
                buf.extend_from_slice(&iv);
                let mut ciphertext = data.to_vec();
                crate::crypto::aes_ctr_xor(key, &iv, nonce, &mut ciphertext);
                buf.extend_from_slice(&ciphertext);
                allocator.write(new, &buf)?;
            }
            (None, Some((key, item))) => {
                let mut enciphered = data.to_vec();
                crate::crypto::rc4_crypt(key, *item, &mut enciphered);
                allocator.write(new, &enciphered)?;
            }
            (None, None) if overhead > 0 => {
                let mut buf = vec![0; AES_IV_SIZE];
                buf.extend_from_slice(data);
                allocator.write(new, &buf)?;
            }
            (None, None) => allocator.write(new, data)?,
        }
    }
    slot.fill(0);
    if new != 0 {
        slot[..4].copy_from_slice(&new.to_le_bytes());
        slot[4..8].copy_from_slice(&stored.to_le_bytes());
        if slot.len() == 16 {
            slot[12..16].copy_from_slice(&length_of(full.len())?.to_le_bytes());
        }
    }
    Ok(())
}

/// The bytes of a legacy ADS VarChar (`ADS_VARCHAR`, code 8) in `record`,
/// laid out as in a Clipper or FoxPro table (see `dbf.rs`): a value of up to
/// the field's size − 2 bytes is inline, padded with spaces and followed by
/// its u16 length; a longer one keeps its first size − 6 bytes inline, then
/// the u32 memo block that holds the rest, raw, and the u16 total length.
fn legacy_var_char(
    field: &Field,
    record: &[u8],
    memo: Option<&mut MemoFile>,
) -> Result<Vec<u8>, Error> {
    let bytes = field.bytes(record);
    let size = bytes.len();
    // The blank record an AES table gives a handle without its password is
    // spaces throughout and reads as empty (`fixtures/aes/api/01_types`).
    if bytes.iter().all(|&b| b == b' ') {
        return Ok(Vec::new());
    }
    let length = usize::from(u16::from_le_bytes([bytes[size - 2], bytes[size - 1]]));
    if length <= size - 2 {
        return Ok(bytes[..length].to_vec());
    }
    let inline = size - 6;
    let block = u32_at(bytes, inline);
    let memo = memo.ok_or(Error::Corrupt("VarChar overflow without a memo file"))?;
    let mut value = bytes[..inline].to_vec();
    value.extend(memo.read_raw(block, length - inline)?);
    Ok(value)
}

/// Stores `text` in a legacy ADS VarChar field, `slot` its bytes in the
/// record (see [`legacy_var_char`]): the rest of a value too long for the
/// field is allocated anew in `memo`.
pub fn store_legacy_var_char(
    text: &[u8],
    slot: &mut [u8],
    memo: Option<&mut MemoFile>,
) -> Result<(), Error> {
    let size = slot.len();
    let length = u16::try_from(text.len()).map_err(|_| Error::Corrupt("VarChar too long"))?;
    if text.len() <= size - 2 {
        slot[..text.len()].copy_from_slice(text);
        slot[text.len()..size - 2].fill(b' ');
    } else {
        let inline = size - 6;
        let memo = memo.ok_or(Error::Corrupt("VarChar overflow without a memo file"))?;
        let mut allocator = memo.allocator()?;
        let rest = &text[inline..];
        let block = allocator.alloc(rest.len() as u32)?;
        allocator.write(block, rest)?;
        slot[..inline].copy_from_slice(&text[..inline]);
        slot[inline..inline + 4].copy_from_slice(&block.to_le_bytes());
    }
    slot[size - 2..].copy_from_slice(&length.to_le_bytes());
    Ok(())
}

/// Frees the value memo field `field` refers to in `slot` (its record is
/// deleted, and keeps the reference).
pub fn free_memo(field: &Field, slot: &[u8], memo: &mut MemoFile) -> Result<(), Error> {
    let (block, length) = (u32_at(slot, 0), u32_at(slot, 4));
    if block == 0 || !field.is_memo() {
        return Ok(());
    }
    let length = length + memo.value_overhead();
    memo.allocator()?.free(block, length)
}

/// Takes back the memo value a recalled record's `field` refers to (in
/// `slot`, its bytes) from the memo file's free space, where it still can
/// be (see `Allocator::reclaim` in `adm.rs`).
pub fn reclaim_memo(field: &Field, slot: &[u8], memo: &mut MemoFile) -> Result<(), Error> {
    let (block, length) = (u32_at(slot, 0), u32_at(slot, 4));
    if block == 0 || !field.is_memo() {
        return Ok(());
    }
    let length = length + memo.value_overhead();
    memo.allocator()?.reclaim(block, length)
}

/// `data` as a zlib stream at compression level 1, as the reference zlib
/// writes it (the original engine's streams are).
fn zlib(data: &[u8]) -> Result<Vec<u8>, Error> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(1));
    encoder.write_all(data)?;
    Ok(encoder.finish()?)
}

/// Copies `data` into `slot`, cut to its length or padded with `pad`.
fn padded(slot: &mut [u8], data: &[u8], pad: u8) {
    let n = data.len().min(slot.len());
    slot[..n].copy_from_slice(&data[..n]);
    slot[n..].fill(pad);
}

/// Copies `data` into a VarChar, VarBinary or NVarChar slot: the data, cut
/// to fit, padded with `pad` (zeros, or spaces in NVarChar), and its length
/// (in `unit`-byte units) in the last two bytes.
fn variable(slot: &mut [u8], data: &[u8], unit: usize, pad: u8) {
    let end = slot.len();
    let room = (end - 2) / unit * unit;
    let n = data.len().min(room) / unit * unit;
    slot[..n].copy_from_slice(&data[..n]);
    match unit {
        2 => {
            for chunk in slot[n..end - 2].as_chunks_mut::<2>().0 {
                *chunk = [pad, 0];
            }
        }
        _ => slot[n..end - 2].fill(pad),
    }
    slot[end - 2..].copy_from_slice(&((n / unit) as u16).to_le_bytes());
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use serde_json::Value as Json;

    use super::*;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt")
    }

    /// Every `.adt` fixture with its JSON dump.
    fn tables() -> Vec<(PathBuf, Json)> {
        let mut tables = Vec::new();
        for dir in std::fs::read_dir(fixtures()).unwrap() {
            for entry in std::fs::read_dir(dir.unwrap().path()).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_some_and(|e| e == "adt") {
                    let json = std::fs::read_to_string(path.with_extension("json")).unwrap();
                    tables.push((path, serde_json::from_str(&json).unwrap()));
                }
            }
        }
        tables.sort_by(|a, b| a.0.cmp(&b.0));
        tables
    }

    #[test]
    fn structures_match_the_engine() {
        let tables = tables();
        assert!(tables.len() >= 30, "found only {} fixtures", tables.len());
        let mut checked = 0;
        for (path, dump) in &tables {
            let name = path.display();
            let structure = match Structure::read(path, Format::Adt) {
                Err(Error::EncryptedDescriptors) => continue,
                result => result.unwrap_or_else(|e| panic!("{name}: {e:?}")),
            };
            let header = &structure.header;
            assert_eq!(
                u64::from(header.record_count()),
                dump["record_count"],
                "{name}"
            );
            assert_eq!(
                u64::from(header.record_length),
                dump["record_length"],
                "{name}"
            );
            let fields = dump["fields"].as_array().unwrap();
            assert_eq!(structure.fields.len(), fields.len(), "{name}");
            for (field, expected) in structure.fields.iter().zip(fields) {
                let what = format!("{name}: {}", field.name);
                assert_eq!(field.name, expected["name"], "{name}");
                assert_eq!(
                    u64::from(field.field_type.code()),
                    expected["type"],
                    "{what}"
                );
                assert_eq!(u64::from(field.length()), expected["length"], "{what}");
                assert_eq!(u64::from(field.decimals()), expected["decimals"], "{what}");
            }
            // The file ends right after the last record slot.
            let size = std::fs::metadata(path).unwrap().len();
            let records = u64::from(header.record_slots) * u64::from(header.record_length);
            assert_eq!(size, u64::from(header.data_offset) + records, "{name}");
            checked += 1;
        }
        assert!(checked >= 30, "checked only {checked} tables");
    }

    /// Every record's raw bytes and deleted flag, as the engine dumped them
    /// (the dumps list live records; deleted ones only where the engine
    /// shows them, which it does not for ADT tables).
    #[test]
    fn records_match_the_engine() {
        let mut checked = 0;
        for (path, dump) in &tables() {
            let name = path.display();
            let mut table = match open(path, false) {
                Err(Error::EncryptedDescriptors) => continue,
                result => result.unwrap_or_else(|e| panic!("{name}: {e:?}")),
            };
            let structure = table.structure.clone();
            if structure.header.encrypted() {
                continue;
            }
            let mut buf = vec![0; structure.header.record_length as usize];
            let mut active = 0;
            for slot in 1..=table.slots() {
                if table.status(slot).unwrap() == RecordStatus::Active {
                    active += 1;
                }
            }
            assert_eq!(active, structure.header.record_count(), "{name}");
            for record in dump["records"].as_array().unwrap() {
                let slot = record["recno"].as_u64().unwrap() as u32;
                table.read(slot, &mut buf).unwrap();
                let raw: String = buf.iter().map(|b| format!("{b:02x}")).collect();
                assert_eq!(
                    raw,
                    record["raw"].as_str().unwrap().to_lowercase(),
                    "{name} #{slot}"
                );
                assert_eq!(
                    table.status(slot).unwrap(),
                    RecordStatus::Active,
                    "{name} #{slot}"
                );
                checked += 1;
            }
        }
        assert!(checked > 100, "checked only {checked} records");
    }

    /// Every field of every record is NULL exactly where the engine said so.
    #[test]
    fn nulls_match_the_engine() {
        let mut nulls = 0;
        for (path, dump) in &tables() {
            let name = path.display();
            let Ok(mut table) = open(path, false) else {
                continue;
            };
            let structure = table.structure.clone();
            if structure.header.encrypted() {
                continue;
            }
            let mut buf = vec![0; structure.header.record_length as usize];
            for record in dump["records"].as_array().unwrap() {
                let slot = record["recno"].as_u64().unwrap() as u32;
                table.read(slot, &mut buf).unwrap();
                for field in &structure.fields {
                    let value = &record["values"][&field.name];
                    let expected = value["null"] == true;
                    let what = format!("{name} #{slot} {}", field.name);
                    assert_eq!(field.is_null(&buf), expected, "{what}: {value}");
                    nulls += usize::from(expected);
                }
            }
        }
        assert!(nulls > 50, "saw only {nulls} NULLs");
    }

    /// Every memo value's length, as the engine decoded the value.
    #[test]
    fn memo_lengths_match_the_engine() {
        let mut checked = 0;
        for (path, dump) in &tables() {
            let name = path.display();
            let Ok(mut table) = open(path, false) else {
                continue;
            };
            let structure = table.structure.clone();
            if structure.header.encrypted() {
                continue;
            }
            let mut buf = vec![0; structure.header.record_length as usize];
            for record in dump["records"].as_array().unwrap() {
                let slot = record["recno"].as_u64().unwrap() as u32;
                table.read(slot, &mut buf).unwrap();
                for field in structure.fields.iter().filter(|f| f.is_memo()) {
                    let value = &record["values"][&field.name];
                    let expected = if value["null"] == true {
                        0
                    } else if let Some(hex) = value["hex"].as_str() {
                        hex.len() / 2
                    } else {
                        let text = value["text"].as_str().unwrap();
                        match field.field_type {
                            FieldType::NMemo => text.encode_utf16().count() * 2,
                            _ => text.chars().count(),
                        }
                    };
                    let what = format!("{name} #{slot} {}", field.name);
                    let length = field.memo_ref(&buf).unwrap().length;
                    assert_eq!(length as usize, expected, "{what}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 50, "checked only {checked} memo values");
    }

    /// Every memo value, read from the memo file, as the engine decoded it.
    #[test]
    fn memo_values_match_the_engine() {
        let mut checked = 0;
        for (path, dump) in &tables() {
            let name = path.display();
            let Ok(mut table) = open(path, false) else {
                continue;
            };
            let structure = table.structure.clone();
            if table.memo().is_none() {
                continue;
            }
            if structure.header.encrypted() {
                continue;
            }
            let mut buf = vec![0; structure.header.record_length as usize];
            for record in dump["records"].as_array().unwrap() {
                let slot = record["recno"].as_u64().unwrap() as u32;
                table.read(slot, &mut buf).unwrap();
                for field in structure.fields.iter().filter(|f| f.is_memo()) {
                    let expected = &record["values"][&field.name];
                    let what = format!("{name} #{slot} {}", field.name);
                    let value = table
                        .memo()
                        .unwrap()
                        .read(field.memo_ref(&buf).unwrap())
                        .unwrap();
                    if expected["null"] == true {
                        assert!(value.is_empty(), "{what}");
                    } else if let Some(hex) = expected["hex"].as_str() {
                        let got: String = value.iter().map(|b| format!("{b:02x}")).collect();
                        assert_eq!(got, hex.to_lowercase(), "{what}");
                    } else {
                        let text = match field.field_type {
                            FieldType::NMemo => {
                                let units: Vec<u16> = value
                                    .chunks(2)
                                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                                    .collect();
                                String::from_utf16(&units).unwrap()
                            }
                            // Dumped as Latin-1 code points, one per byte.
                            _ => value.iter().map(|&b| char::from(b)).collect(),
                        };
                        assert_eq!(text, expected["text"].as_str().unwrap(), "{what}");
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 50, "checked only {checked} memo values");
    }

    /// The stored lengths of VarChar, VarBinary and NVarChar values, as the
    /// engine decoded the values.
    #[test]
    fn var_lengths_match_the_engine() {
        let mut checked = 0;
        for (path, dump) in &tables() {
            let Ok(mut table) = open(path, false) else {
                continue;
            };
            let structure = table.structure.clone();
            if structure.header.encrypted() {
                continue;
            }
            let mut buf = vec![0; structure.header.record_length as usize];
            for record in dump["records"].as_array().unwrap() {
                let slot = record["recno"].as_u64().unwrap() as u32;
                table.read(slot, &mut buf).unwrap();
                for field in &structure.fields {
                    let Some(length) = field.var_length(&buf) else {
                        continue;
                    };
                    let value = &record["values"][&field.name];
                    let what = format!("{} #{slot} {}", path.display(), field.name);
                    if value["null"] == true {
                        assert_eq!(length, 0xFFFF, "{what}");
                        continue;
                    }
                    let expected = match (field.field_type, &value["hex"]) {
                        (_, Json::String(hex)) => hex.len() / 2,
                        (FieldType::NVarChar, _) => {
                            value["text"].as_str().unwrap().encode_utf16().count()
                        }
                        _ => value["text"].as_str().unwrap().chars().count(),
                    };
                    assert_eq!(usize::from(length), expected, "{what}: {value}");
                    checked += 1;
                }
            }
        }
        assert!(checked >= 15, "checked only {checked} values");
    }

    /// Asserts that `value`, decoded from `field`, is the engine's `expected`
    /// dump value.
    pub(crate) fn assert_value(field: &Field, value: &Value, expected: &Json, what: &str) {
        let text = expected["text"].as_str();
        match value {
            Value::Null => assert_eq!(expected["null"], true, "{what}"),
            Value::Text(s) | Value::Numeric(s) => {
                // The dumps write 8-bit text as one code point per byte
                // (U+0000-U+00FF); decode those bytes as Windows-1252.
                let expected = match field.field_type {
                    FieldType::NChar | FieldType::NVarChar | FieldType::NMemo => {
                        text.unwrap().to_owned()
                    }
                    _ => {
                        let bytes: Vec<u8> = text.unwrap().chars().map(|c| c as u8).collect();
                        cp1252::decode(&bytes)
                    }
                };
                assert_eq!(s, &expected, "{what}");
            }
            Value::Binary(b) => {
                let hex: String = b.iter().map(|b| format!("{b:02x}")).collect();
                assert_eq!(
                    hex,
                    expected["hex"].as_str().unwrap().to_lowercase(),
                    "{what}"
                );
            }
            Value::Logical(t) => assert_eq!(text, Some(if *t { "T" } else { "F" }), "{what}"),
            Value::Integer(n) => assert_eq!(text, Some(n.to_string().as_str()), "{what}"),
            Value::Double(d) => assert_eq!(text.unwrap().parse::<f64>().unwrap(), *d, "{what}"),
            Value::Money(m) => {
                let text = text.unwrap();
                let (int, frac) = text.split_once('.').unwrap_or((text, ""));
                let frac: i64 = format!("{frac:0<4}").parse().unwrap();
                let int: i64 = int.parse().unwrap();
                let sign = if text.starts_with('-') { -1 } else { 1 };
                assert_eq!(*m, int * 10_000 + sign * frac, "{what}");
            }
            Value::Date(j) => assert_eq!(expected["julian"], *j, "{what}"),
            Value::Time(ms) => assert_eq!(expected["ms"], *ms, "{what}"),
            Value::TimeStamp(j, ms) => {
                assert_eq!(expected["julian"], *j, "{what}");
                assert_eq!(expected["ms"], *ms, "{what}");
            }
            Value::Guid(g) => assert_eq!(Some(Value::guid_text(g).as_str()), text, "{what}"),
            Value::Blank => match field.field_type {
                FieldType::Logical => assert_eq!(text, Some(" "), "{what}"),
                _ => assert_eq!(expected["julian"], 0, "{what}"),
            },
        }
    }

    /// Every value of every record, decoded, as the engine decoded it.
    #[test]
    fn values_match_the_engine() {
        let mut checked = 0;
        for (path, dump) in &tables() {
            let Ok(mut table) = open(path, false) else {
                continue;
            };
            let structure = table.structure.clone();
            if structure.header.encrypted() {
                continue;
            }
            let mut buf = vec![0; structure.header.record_length as usize];
            for record in dump["records"].as_array().unwrap() {
                let slot = record["recno"].as_u64().unwrap() as u32;
                table.read(slot, &mut buf).unwrap();
                for field in &structure.fields {
                    let expected = &record["values"][&field.name];
                    let what = format!("{} #{slot} {}: {expected}", path.display(), field.name);
                    let value = field.value(&buf, table.memo()).unwrap();
                    assert_value(field, &value, expected, &what);
                    checked += 1;
                }
            }
        }
        assert!(checked > 1000, "checked only {checked} values");
    }

    #[test]
    fn rejects_other_files() {
        let json = fixtures().join("all_types/all_types.json");
        assert!(matches!(
            Structure::read(&json, Format::Adt),
            Err(Error::Corrupt(_))
        ));
        assert!(matches!(
            Structure::read(&fixtures().join("nonexistent.adt"), Format::Adt),
            Err(Error::Io(_))
        ));
        let mut short = &b"Advantage Table\0"[..];
        assert!(matches!(read_structure(&mut short), Err(Error::Corrupt(_))));
    }

    #[test]
    fn field_types_round_trip() {
        for code in 0..40 {
            assert_eq!(FieldType::from_code(code).code(), code);
        }
    }

    #[test]
    fn record_status_bytes() {
        for active in [0x00, 0x02, 0x04, 0x06] {
            assert_eq!(record_status(active), Some(RecordStatus::Active));
        }
        for deleted in [0x01, 0x03, 0x05, 0x07] {
            assert_eq!(record_status(deleted), Some(RecordStatus::Deleted));
        }
        assert_eq!(record_status(0x08), None);
        assert_eq!(record_status(0x20), None);
    }

    #[test]
    fn nullable_types() {
        assert!(FieldType::Character.nullable());
        assert!(FieldType::Integer.nullable());
        assert!(FieldType::ModTime.nullable());
        assert!(!FieldType::AutoInc.nullable());
        assert!(!FieldType::RowVersion.nullable());
    }

    #[test]
    fn null_records_hold_nulls() {
        let path = fixtures().join("all_types/all_types.adt");
        let structure = Structure::read(&path, Format::Adt).unwrap();
        let record = structure.null_record();
        assert_eq!(record.len() as u32, structure.header.record_length);
        assert_eq!(record[..5], [0; 5]);
        for field in &structure.fields {
            // RowVersion's empty value is LongInt's NULL; AutoInc has none.
            let never_null = field.field_type == FieldType::AutoInc;
            assert_eq!(field.is_null(&record), !never_null, "{}", field.name);
        }
        let bytes = |name| {
            let field = structure.fields.iter().find(|f| f.name == name).unwrap();
            field.bytes(&record).to_vec()
        };
        // As the original engine fills them in.
        assert_eq!(bytes("c_integer"), [0, 0, 0, 0x80]);
        assert_eq!(bytes("c_double"), [0x20, 0, 0, 0, 0, 0, 0, 0x80]);
        assert_eq!(bytes("c_logical"), b" ");
        assert_eq!(bytes("c_time"), [0xFF; 4]);
        assert_eq!(bytes("c_rowversion"), [0, 0, 0, 0, 0, 0, 0, 0x80]);
        assert_eq!(bytes("c_char"), [0; 10]);
    }
}
