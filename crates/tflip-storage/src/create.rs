//! Creating tables: the files of a new, empty ADT table (`.adt` and, for
//! memo fields, `.adm`), as the original engine writes them for `CREATE
//! TABLE` (`fixtures/write/ddl`).
//!
//! The header of a new table holds the record length, the descriptors'
//! end, the field count and flags: [`flags::MEMO`], [`flags::AUTOINC`],
//! [`flags::ROWVERSION`] and [`flags::MODTIME`] for the fields it has, and
//! 0x10 always (its meaning is not known). An AutoInc field's next value,
//! 1, is at 0x50; 0x14 holds 1 and 0x164 holds 4. Each field takes its
//! descriptor; a compressed memo field (16 bytes in the record, not 9) has
//! 0x10 at 140.
//!
//! A new memo file is its allocator's empty header (see `tflip_storage::adm`): the
//! end of the allocated space (the 4,096-byte header, in blocks) and the
//! block size, the smallest run of each size class, and a slab list for
//! each chunk size, none with a slab yet. It ends after the slab lists.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use crate::adt::{self, DESCRIPTOR_SIZE, HEADER_SIZE};
use crate::field::{Field, FieldType};
use crate::maintain::{Compact, Layout};
use crate::table::{Error, Format, Header, flags};

/// A field of a table to create: its name, type, declared length (for the
/// types that have one) and decimals, and whether a memo field is
/// compressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSpec {
    pub name: String,
    pub field_type: FieldType,
    pub length: u16,
    pub decimals: u16,
    pub compressed: bool,
}

impl From<&Field> for FieldSpec {
    /// A field of a table as `AdsCreateTable` would define it.
    fn from(field: &Field) -> FieldSpec {
        FieldSpec {
            name: field.name.clone(),
            field_type: field.field_type,
            length: field.length(),
            decimals: field.decimals(),
            compressed: field.compressed(),
        }
    }
}

/// Set in the header of every table the original engine creates.
const CREATED: u32 = 0x10;

/// Where a new table's header holds its AutoInc field's next value.
const AUTOINC_NEXT: usize = 0x50;

/// The record offset of an AES table's first field: the clear prefix (5)
/// and the record's own 8-byte IV.
const AES_FIRST: u32 = (adt::RECORD_PREFIX + adt::AES_IV_SIZE) as u32;

/// The descriptors of the fields of a new ADT table, whose first field is
/// at record offset `first`, with the record's length and `table_flags`
/// with the flags the fields raise.
fn adt_descriptors(
    fields: &[FieldSpec],
    first: u32,
    mut table_flags: u32,
) -> Result<(Vec<u8>, u32, u32), Error> {
    let mut offset = first;
    let mut descriptors = Vec::with_capacity(fields.len() * DESCRIPTOR_SIZE);
    for spec in fields {
        let at = u16::try_from(offset).map_err(|_| Error::Corrupt("record too long"))?;
        let mut field = adt::field(&spec.name, spec.field_type, at, spec.length, spec.decimals);
        if spec.compressed {
            field.size = 16;
        }
        offset += u32::from(field.size);
        let mut bytes = adt::field_bytes(&field);
        if spec.compressed {
            bytes[140] = 0x10;
        }
        descriptors.extend_from_slice(&bytes);
        table_flags |= match spec.field_type {
            // A legacy VarChar keeps what does not fit its record in the memo
            // file.
            FieldType::Memo
            | FieldType::Binary
            | FieldType::Image
            | FieldType::NMemo
            | FieldType::LegacyVarChar => flags::MEMO,
            FieldType::AutoInc => flags::AUTOINC,
            FieldType::RowVersion => flags::ROWVERSION,
            FieldType::ModTime => flags::MODTIME,
            _ => 0,
        };
    }
    Ok((descriptors, offset, table_flags))
}

/// The header of a new, empty ADT table of `field_count` fields, whose
/// `descriptors` follow its `extension` bytes; an AutoInc's next value is 1.
fn adt_header(
    field_count: usize,
    descriptors: &[u8],
    record_length: u32,
    table_flags: u32,
    extension: u32,
    encryption_type: u8,
    dictionary: bool,
) -> Result<Vec<u8>, Error> {
    let field_count = u16::try_from(field_count).map_err(|_| Error::Corrupt("too many fields"))?;
    let header = Header {
        format: Format::Adt,
        record_slots: 0,
        data_offset: HEADER_SIZE as u32 + extension + descriptors.len() as u32,
        record_length,
        free_head: 0,
        free_count: 0,
        encryption_type,
        flags: table_flags,
        field_count,
        extension,
        dictionary,
    };
    let mut bytes = adt::header_bytes(&header).to_vec();
    if table_flags & flags::AUTOINC != 0 {
        bytes[AUTOINC_NEXT..AUTOINC_NEXT + 4].copy_from_slice(&1u32.to_le_bytes());
    }
    Ok(bytes)
}

/// The length of a record of a new ADT table with `fields`.
pub fn adt_record_length(fields: &[FieldSpec]) -> u32 {
    let sizes = fields.iter().map(|spec| {
        let size = adt::field(&spec.name, spec.field_type, 0, spec.length, spec.decimals).size;
        match spec.compressed {
            true => 16,
            false => u32::from(size),
        }
    });
    adt::RECORD_PREFIX as u32 + sizes.sum::<u32>()
}

/// The length of a record of a new DBF table of `flavour` with `fields`:
/// the deletion mark, the fields and VFP's null flags.
pub fn dbf_record_length(flavour: crate::dbf::Flavour, fields: &[DbfFieldSpec]) -> u32 {
    let visual = flavour == crate::dbf::Flavour::Visual;
    let layouts: Vec<DbfLayout> = fields.iter().map(|f| dbf_layout(flavour, f)).collect();
    let bits: usize = fields
        .iter()
        .zip(&layouts)
        .map(|(f, l)| usize::from(l.length_bit) + usize::from(visual && f.nullable))
        .sum();
    let lengths: u32 = layouts.iter().map(|l| u32::from(l.length)).sum();
    1 + lengths + bits.div_ceil(8) as u32
}

/// The bytes of a new, empty ADT table with `fields`.
pub fn adt_bytes(fields: &[FieldSpec]) -> Result<Vec<u8>, Error> {
    let (descriptors, offset, table_flags) = adt_descriptors(fields, 5, CREATED)?;
    let mut bytes = adt_header(fields.len(), &descriptors, offset, table_flags, 0, 0, false)?;
    bytes[0x14] = 1;
    bytes[0x164] = 4;
    bytes.extend_from_slice(&descriptors);
    Ok(bytes)
}

/// Gives the fields of the ADT table at `path` the `names` (field number,
/// 0-based, and new name), as the original engine renames fields that change
/// nothing else: only their names in the header's descriptors are written,
/// each 128 bytes, NUL-padded (`fixtures/write/restructure_rename`).
pub fn rename_adt_fields(path: &Path, names: &[(usize, &str)]) -> Result<(), Error> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut header = [0u8; HEADER_SIZE];
    file.read_exact(&mut header)?;
    let header = adt::parse_header(&header)?;
    let first = HEADER_SIZE as u64 + u64::from(header.extension);
    for &(number, name) in names {
        let mut bytes = [0u8; 128];
        let name = crate::cp1252::encode(name);
        let name = &name[..name.len().min(128)];
        bytes[..name.len()].copy_from_slice(name);
        file.seek(SeekFrom::Start(first + (number * DESCRIPTOR_SIZE) as u64))?;
        file.write_all(&bytes)?;
    }
    Ok(())
}

/// Gives the fields of the DBF table at `path` the `names` (field number,
/// 0-based, and new name), as the original engine renames fields that change
/// nothing else: only their names in the header's descriptors are written
/// (11 bytes, NUL-padded), in upper case for a Clipper or FoxPro table
/// (`fixtures/sql/ddalterfree`).
pub fn rename_dbf_fields(path: &Path, names: &[(usize, &str)], upper: bool) -> Result<(), Error> {
    use std::io::{Seek, SeekFrom};
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    for &(number, name) in names {
        let mut bytes = [0u8; 11];
        let name = match upper {
            true => crate::cp1252::encode(&name.to_ascii_uppercase()),
            false => crate::cp1252::encode(name),
        };
        let name = &name[..name.len().min(10)];
        bytes[..name.len()].copy_from_slice(name);
        file.seek(SeekFrom::Start(32 + 32 * number as u64))?;
        file.write_all(&bytes)?;
    }
    Ok(())
}

/// Gives the orders of the compound CDX file at `path` named in `edits`
/// (name, key expression, `FOR` condition) their new expressions where they
/// are, as the original engine does for fields that were only renamed: the
/// tag's expression pool and the lengths in its header are written, and the
/// directory's change counter then counts the file's orders
/// (`fixtures/sql/ddalterfree`).
pub fn cdx_rename_expressions(
    path: &Path,
    edits: &[(&str, &str, &str)],
) -> Result<(), crate::order::Error> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let directory = Directory::cdx(&mut file)?;
    let mut changed = false;
    for &(name, expression, condition) in edits {
        let Some(key) = directory.find(name) else {
            continue;
        };
        let at = u64::from(key.record);
        let mut header = vec![0; 1024];
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(&mut header)?;
        let (expression, condition) = (
            crate::cp1252::encode(expression),
            crate::cp1252::encode(condition),
        );
        let (e, c) = (expression.len() + 1, condition.len() + 1);
        header[0x1F8..0x1FA].copy_from_slice(&(e as u16).to_le_bytes());
        header[0x1FA..0x1FC].copy_from_slice(&(c as u16).to_le_bytes());
        header[0x1FE..0x200].copy_from_slice(&(e as u16).to_le_bytes());
        header[0x200..].fill(0);
        header[0x200..0x200 + expression.len()].copy_from_slice(&expression);
        header[0x200 + e..0x200 + e + condition.len()].copy_from_slice(&condition);
        file.seek(SeekFrom::Start(at + 0x1F8))?;
        file.write_all(&header[0x1F8..])?;
        changed = true;
    }
    if changed {
        file.seek(SeekFrom::Start(0x0A))?;
        file.write_all(&(directory.names.len() as u16).to_be_bytes())?;
    }
    Ok(())
}

/// The random material an AES-encrypted table carries in its header, generated
/// fresh by the engine for each table (and injectable, so a byte-for-byte test
/// can reproduce a fixture): the scheme (5 = AES-128, 6 = AES-256), the PBKDF2
/// salt, the per-table nonce, the field-descriptor IV, and the IV counter the
/// header stores (the next value to hand out).
#[derive(Debug, Clone)]
pub struct AesMaterial {
    pub enc_type: u8,
    pub salt: [u8; 32],
    pub nonce: [u8; 4],
    pub descriptor_iv: [u8; 8],
    pub iv_counter: u64,
}

impl AesMaterial {
    /// The AES key length: 32 bytes for AES-256, 16 otherwise.
    pub fn key_len(&self) -> usize {
        if self.enc_type == adt::ENCRYPTION_AES256 {
            32
        } else {
            16
        }
    }

    /// Fresh random material for a new table of `enc_type` (5 = AES-128,
    /// 6 = AES-256), as the engine generates it when encryption is enabled: a
    /// random salt and nonce, and a random 64-bit base the engine uses as the
    /// descriptor IV, with the IV counter (the next IV to hand out) one past it.
    pub fn generate(enc_type: u8) -> Result<Self, Error> {
        let mut bytes = [0u8; 32 + 4 + 8];
        getrandom::getrandom(&mut bytes).map_err(|_| Error::Corrupt("no system randomness"))?;
        let salt: [u8; 32] = bytes[..32].try_into().unwrap();
        let nonce: [u8; 4] = bytes[32..36].try_into().unwrap();
        let descriptor_iv: [u8; 8] = bytes[36..44].try_into().unwrap();
        let iv_counter = u64::from_le_bytes(descriptor_iv).wrapping_add(1);
        Ok(AesMaterial {
            enc_type,
            salt,
            nonce,
            descriptor_iv,
            iv_counter,
        })
    }
}

/// The 600-byte header extension every AES table carries.
pub const AES_EXTENSION: usize = 600;

/// The bytes of a new, empty AES-encrypted ADT table: the header's AES markers
/// (0x14 = 3, 0x164 = 12) and crypto block (the 600-byte extension holding the
/// type, nonce, IV counter, salt and descriptor IV), the 20-byte verifier
/// derived from `password` and the salt, and the field descriptors. Fields begin
/// at record offset 13 (a clear 8-byte IV sits between the 5-byte prefix and the
/// data). `binding` is `Some((dict_path, dict_object))` for a dictionary-bound
/// table — the `.add` reference (header 0x5C) and its object id (0x160), with the
/// descriptors enciphered (`ENCRYPTED_DESCRIPTORS`) — or `None` for a free table
/// (`AdsEnableEncryption` + `AdsEncryptTable` on a connection with an AES
/// encryption type), whose descriptors stay clear and which binds no dictionary.
/// Records are appended enciphered afterwards. Verified byte-for-byte against
/// `fixtures/aes` (bound) and a probe's free table.
pub fn aes_adt_bytes(
    fields: &[FieldSpec],
    password: &[u8],
    material: &AesMaterial,
    binding: Option<(&str, u32)>,
) -> Result<Vec<u8>, Error> {
    let key_len = material.key_len();
    let derived = crate::crypto::aes_derive(password, &material.salt, key_len);
    let key = &derived[..key_len];
    let verifier = &derived[key_len..key_len + 20];

    // A dictionary-bound table enciphers its descriptors; a free one leaves them
    // clear (like record-level encryption).
    let mut table_flags = CREATED | flags::ENCRYPTED;
    if binding.is_some() {
        table_flags |= flags::ENCRYPTED_DESCRIPTORS;
    }
    // The clear prefix (5) plus the record's own 8-byte IV slot: the first field
    // sits at offset 13.
    let (mut descriptors, offset, table_flags) = adt_descriptors(fields, AES_FIRST, table_flags)?;
    if binding.is_some() {
        crate::crypto::aes_ctr_xor(
            key,
            &material.descriptor_iv,
            &material.nonce,
            &mut descriptors,
        );
    }

    let extension = AES_EXTENSION as u32;
    let mut bytes = adt_header(
        fields.len(),
        &descriptors,
        offset,
        table_flags,
        extension,
        material.enc_type,
        binding.is_some(),
    )?;
    // The AES/dictionary markers header_bytes does not write: 0x14 = 3 and
    // 0x164 = 12 mark the extended, AES-bearing format (a plain dict table is
    // 2 / 4); 0x160 is the object id and 0x5C the `.add` path of the binding.
    bytes[0x14] = 3;
    bytes[0x164] = 12;
    if let Some((dict_path, dict_object)) = binding {
        bytes[0x160..0x164].copy_from_slice(&dict_object.to_le_bytes());
        let path = crate::cp1252::encode(dict_path);
        let path = &path[..path.len().min(HEADER_SIZE - 0x5C - 1)];
        bytes[0x5C..0x5C + path.len()].copy_from_slice(path);
    }
    bytes[0x38..0x4C].copy_from_slice(verifier);
    bytes[adt::offsets::ENCRYPTION_TYPE as usize] = material.enc_type;
    // The 600-byte crypto block (the header extension): type word, nonce, IV
    // counter, salt, descriptor IV, then zero padding.
    let mut ext = vec![0u8; AES_EXTENSION];
    ext[0..4].copy_from_slice(&u32::from(material.enc_type).to_le_bytes());
    ext[4..8].copy_from_slice(&material.nonce);
    ext[8..16].copy_from_slice(&material.iv_counter.to_le_bytes());
    ext[16..48].copy_from_slice(&material.salt);
    ext[48..56].copy_from_slice(&material.descriptor_iv);
    bytes.extend_from_slice(&ext);
    bytes.extend_from_slice(&descriptors);
    Ok(bytes)
}

/// The bytes of a new, empty AES-structured ADT table that is not yet armed:
/// what the engine writes when a table is created on a connection whose
/// `EncryptionType` is AES, before `AdsEnableEncryption` sets the password.
/// It has the full AES layout — the 600-byte header extension (so the data
/// starts at 0x640 + descriptors), the `0x14` = 3 / `0x164` = 12 markers and
/// fields at record offset 13 — but its crypto is all zero: the encryption type
/// at 0x4C is 0, the verifier at 0x38 and the whole crypto block are zero, and
/// only the `CREATED` flag (plus `MEMO`/`AUTOINC`/... for the fields) is set, not
/// `ENCRYPTED`. The descriptors are in the clear. Being all-zero in the random
/// parts it is deterministic; [`enable_encryption_aes`](crate::table::TableFile::enable_encryption_aes)
/// fills in the crypto later. Verified byte-for-byte against
/// `fixtures/aes/free/unarmed.adt`.
pub fn aes_adt_unarmed_bytes(fields: &[FieldSpec]) -> Result<Vec<u8>, Error> {
    let (descriptors, offset, table_flags) = adt_descriptors(fields, AES_FIRST, CREATED)?;
    let mut bytes = adt_header(
        fields.len(),
        &descriptors,
        offset,
        table_flags,
        AES_EXTENSION as u32,
        0,
        false,
    )?;
    bytes[0x14] = 3;
    bytes[0x164] = 12;
    // The 600-byte crypto block is present but all zero until the table is armed.
    bytes.extend_from_slice(&[0u8; AES_EXTENSION]);
    bytes.extend_from_slice(&descriptors);
    Ok(bytes)
}

/// A field of a new DBF table: its definition, and in a VFP table whether
/// it takes NULL, whether its text is not translated (`NOCPTRANS`), and an
/// AutoInc's first value and step.
#[derive(Debug, Clone, PartialEq)]
pub struct DbfFieldSpec {
    pub spec: FieldSpec,
    pub nullable: bool,
    pub binary: bool,
    pub autoinc: Option<(u32, u8)>,
}

impl From<&Field> for DbfFieldSpec {
    /// A DBF field as `AdsCreateTable` would define it, VFP's flags kept.
    fn from(field: &Field) -> DbfFieldSpec {
        let (nullable, binary) = match &field.layout {
            crate::field::Layout::Dbf(layout) => (layout.null_bit.is_some(), layout.binary()),
            _ => (false, false),
        };
        DbfFieldSpec {
            spec: FieldSpec::from(field),
            nullable,
            binary,
            autoinc: None,
        }
    }
}

/// A DBF field's descriptor bytes: type letter, length, decimals and VFP
/// flags; whether it needs a memo file, a VFP length bit, and the version
/// byte it needs.
struct DbfLayout {
    letter: u8,
    length: u16,
    decimals: u8,
    flags: u8,
    memo: bool,
    length_bit: bool,
    version: u8,
}

/// How a field of `flavour` is described, as the original engine writes it
/// (`fixtures/write/dbf_create`).
fn dbf_layout(flavour: crate::dbf::Flavour, field: &DbfFieldSpec) -> DbfLayout {
    use FieldType::*;
    let visual = flavour == crate::dbf::Flavour::Visual;
    let spec = &field.spec;
    let memo_length = if visual { 4 } else { 10 };
    // VFP marks most binary types as not translated.
    let binary = 0x04;
    let (letter, length, decimals, mut flags, memo) = match spec.field_type {
        Character => (b'C', spec.length, 0, 0, false),
        Numeric => (b'N', spec.length, spec.decimals as u8, 0, false),
        Date => (b'D', 8, 0, 0, false),
        CompactDate => (b'V', 3, 0, 0, false),
        Logical => (b'L', 1, 0, 0, false),
        Memo => (b'M', memo_length, 0, 0, true),
        Binary | Image if visual => (b'W', 4, 0, binary, true),
        Binary | Image => (b'M', 10, 0, 0, true),
        Double => (b'B', 8, spec.decimals as u8, binary, false),
        Integer => (b'I', 4, 0, binary, false),
        AutoInc => (b'I', 4, 0, 0x0C, false),
        TimeStamp => (b'T', 8, 0, binary, false),
        Money => (b'Y', 8, 4, binary, false),
        LongInt => (0x13, 8, 0, binary, false),
        LegacyVarChar => (b'V', spec.length, 0, 0, true),
        VarChar => (b'V', spec.length, 0, 0, false),
        VarBinary => (b'Q', spec.length, 0, binary, false),
        NChar => (0x1A, spec.length * 2, 0, binary, false),
        NVarChar => (0x1B, spec.length * 2 + 2, 0, binary, false),
        NMemo => (0x1C, memo_length, 0, binary, true),
        Guid => (0x1D, 16, 0, binary, false),
        Other(code) => (code as u8, memo_length, 0, 0, true),
        _ => (b'C', spec.length, 0, 0, false),
    };
    if !visual {
        flags = 0;
    } else {
        if field.nullable {
            flags |= 0x02;
        }
        if field.binary {
            flags |= 0x04;
        }
    }
    let length_bit = visual && matches!(spec.field_type, VarChar | VarBinary);
    let version = match spec.field_type {
        VarChar | VarBinary | Binary | Image if visual => 0x32,
        AutoInc if visual => 0x31,
        _ => 0x30,
    };
    DbfLayout {
        letter,
        length,
        decimals,
        flags,
        memo,
        length_bit,
        version,
    }
}

/// The bytes of a new, empty DBF table of `flavour` with `fields`, as the
/// original engine creates it (`fixtures/write/dbf_create`): the version
/// byte (0x03, or with a memo file 0x83 for Clipper and 0xF5 for FoxPro;
/// 0x30 to 0x32 for VFP), the date of `today` (years since 1900), no
/// records, the field descriptors (names in upper case but in VFP, each
/// field's offset in the record), VFP's `_NullFlags` field for the fields
/// that take NULL and the VarChar and VarBinary ones, and the end of the
/// header (with VFP's 263-byte back link) and of the file. Also whether it
/// needs a memo file.
pub fn dbf_bytes(
    flavour: crate::dbf::Flavour,
    fields: &[DbfFieldSpec],
    today: crate::value::Ymd,
) -> (Vec<u8>, bool) {
    dbf_bytes_in(flavour, fields, today, false)
}

/// [`dbf_bytes`] of an `oem` table: a Clipper or FoxPro table of the OEM
/// character type has no language driver in its header (byte 29, 0x03 for an
/// ANSI one; `fixtures/sql/ddbf`).
pub fn dbf_bytes_in(
    flavour: crate::dbf::Flavour,
    fields: &[DbfFieldSpec],
    today: crate::value::Ymd,
    oem: bool,
) -> (Vec<u8>, bool) {
    dbf_bytes_layout(flavour, fields, today, oem, false)
}

/// [`dbf_bytes_in`] of a Visual FoxPro table laid out for AES when `aes` (an
/// AES dictionary's table, or one made on an AES connection;
/// `fixtures/aes/rc4tp`, `v1` to `v10`): `_NullFlags` is always there, 8 bytes
/// longer for the IV each record ends with, and the version byte is
/// [`crate::dbf::AES_LAYOUT`].
pub fn dbf_bytes_layout(
    flavour: crate::dbf::Flavour,
    fields: &[DbfFieldSpec],
    today: crate::value::Ymd,
    oem: bool,
    aes: bool,
) -> (Vec<u8>, bool) {
    let visual = flavour == crate::dbf::Flavour::Visual;
    let aes = aes && visual;
    let layouts: Vec<DbfLayout> = fields.iter().map(|f| dbf_layout(flavour, f)).collect();
    let memo = layouts.iter().any(|l| l.memo);
    let bits: usize = fields
        .iter()
        .zip(&layouts)
        .map(|(f, l)| usize::from(l.length_bit) + usize::from(visual && f.nullable))
        .sum();
    let descriptors = fields.len() + usize::from(bits > 0 || aes);
    let header_length = 32 + 32 * descriptors + if visual { 1 + 263 } else { 2 };
    let mut bytes = vec![0; header_length];
    let mut offset = 1u32;
    for (i, (field, layout)) in fields.iter().zip(&layouts).enumerate() {
        let d = &mut bytes[32 + 32 * i..64 + 32 * i];
        let name = match visual {
            true => crate::cp1252::encode(&field.spec.name),
            false => crate::cp1252::encode(&field.spec.name.to_ascii_uppercase()),
        };
        let n = name.len().min(10);
        d[..n].copy_from_slice(&name[..n]);
        d[11] = layout.letter;
        d[12..16].copy_from_slice(&offset.to_le_bytes());
        match layout.letter {
            b'C' | b'V' | b'Q' | 0x1A | 0x1B => {
                d[16..18].copy_from_slice(&layout.length.to_le_bytes())
            }
            _ => {
                d[16] = layout.length as u8;
                d[17] = layout.decimals;
            }
        }
        d[18] = layout.flags;
        if let (true, Some((first, step))) = (visual, field.autoinc) {
            d[19..23].copy_from_slice(&first.to_le_bytes());
            d[23] = step;
        }
        if !visual {
            match field.spec.field_type {
                FieldType::Binary => d[19] = b'B',
                FieldType::Image => d[19] = b'I',
                _ => {}
            }
        }
        offset += u32::from(layout.length);
    }
    if bits > 0 || aes {
        let length = bits.div_ceil(8) + if aes { 8 } else { 0 };
        let d = &mut bytes[32 + 32 * fields.len()..64 + 32 * fields.len()];
        d[..10].copy_from_slice(b"_NullFlags");
        d[11] = b'0';
        d[12..16].copy_from_slice(&offset.to_le_bytes());
        d[16] = length as u8;
        d[18] = 0x05;
        offset += length as u32;
    }
    bytes[32 + 32 * descriptors] = 0x0D;
    bytes[0] = match flavour {
        crate::dbf::Flavour::Visual if aes => crate::dbf::AES_LAYOUT,
        crate::dbf::Flavour::Visual => layouts.iter().map(|l| l.version).max().unwrap_or(0x30),
        crate::dbf::Flavour::Clipper if memo => 0x83,
        crate::dbf::Flavour::FoxPro if memo => 0xF5,
        _ => 0x03,
    };
    bytes[1] = (today.year - 1900).clamp(0, 255) as u8;
    bytes[2] = today.month as u8;
    bytes[3] = today.day as u8;
    bytes[8..10].copy_from_slice(&(header_length as u16).to_le_bytes());
    bytes[10..12].copy_from_slice(&(offset as u16).to_le_bytes());
    match visual {
        true => bytes[28] = if memo { 0x02 } else { 0 },
        false => bytes[29] = if oem { 0 } else { 0x03 },
    }
    bytes.push(crate::dbf::END_OF_FILE);
    (bytes, memo)
}

/// The bytes of a new, empty `.dbt` memo file: its next block, 1.
pub fn dbt_bytes() -> Vec<u8> {
    let mut bytes = vec![0; 512];
    bytes[0..4].copy_from_slice(&1u32.to_le_bytes());
    bytes
}

/// The bytes of a new, empty `.fpt` memo file of `block_size`-byte blocks:
/// the block after its 512-byte header, and the block size (big-endian).
pub fn fpt_bytes(block_size: u16) -> Vec<u8> {
    let mut bytes = vec![0; 512];
    let next = 512u32.div_ceil(u32::from(block_size.max(1)));
    bytes[0..4].copy_from_slice(&next.to_be_bytes());
    bytes[6..8].copy_from_slice(&block_size.to_be_bytes());
    bytes
}

/// What a new NTX order is.
#[derive(Debug, Clone, PartialEq)]
pub struct NtxSpec {
    pub key_length: u16,
    pub decimals: u16,
    pub expression: String,
    pub condition: String,
    pub unique: bool,
    pub descending: bool,
}

/// How many keys an NTX page of `key_length`-byte keys takes, as the
/// original engine has it: as many items (8 bytes and the key, with its
/// 2-byte offset) as fit a page besides the count and one more item, made
/// even.
fn ntx_max(key_length: u16) -> u16 {
    let item = usize::from(key_length) + 8;
    let most = (crate::ntx::PAGE_SIZE - 2) / (item + 2) - 1;
    (most - most % 2) as u16
}

/// The bytes of a new NTX file with no keys, as the original engine creates
/// it (`fixtures/write/ntx_create`): the header (signature 6, or 7 with a
/// condition or descending; its change counter 1; the root at 1,024; the
/// item and key sizes, the decimals, the keys a page takes and half of
/// them; the expression, the unique and descending flags and the
/// condition) and an empty root page.
pub fn ntx_bytes(spec: &NtxSpec) -> Vec<u8> {
    let ps = crate::ntx::PAGE_SIZE;
    let mut bytes = vec![0; 2 * ps];
    let max = ntx_max(spec.key_length);
    let extended = spec.descending || !spec.condition.is_empty();
    let put16 = |bytes: &mut Vec<u8>, at: usize, v: u16| {
        bytes[at..at + 2].copy_from_slice(&v.to_le_bytes())
    };
    put16(&mut bytes, 0, if extended { 7 } else { 6 });
    put16(&mut bytes, 2, 1);
    bytes[4..8].copy_from_slice(&(ps as u32).to_le_bytes());
    put16(&mut bytes, 0x0C, spec.key_length + 8);
    put16(&mut bytes, 0x0E, spec.key_length);
    put16(&mut bytes, 0x10, spec.decimals);
    put16(&mut bytes, 0x12, max);
    put16(&mut bytes, 0x14, max / 2);
    let text = |bytes: &mut Vec<u8>, at: usize, text: &str| {
        let text = crate::cp1252::encode(text);
        let n = text.len().min(255);
        bytes[at..at + n].copy_from_slice(&text[..n]);
    };
    text(&mut bytes, 0x16, &spec.expression);
    bytes[0x116] = u8::from(spec.unique);
    bytes[0x118] = u8::from(spec.descending);
    text(&mut bytes, 0x11A, &spec.condition);
    let page = ntx_page(spec.key_length, max, &[], &[]);
    bytes[ps..].copy_from_slice(&page);
    bytes
}

/// An NTX page with `keys` (in their slots, in order) and the child pages
/// (byte offsets) before each and after the last, if it is interior.
fn ntx_page(key_length: u16, max: u16, keys: &[&crate::order::Key], children: &[u32]) -> Vec<u8> {
    let ps = crate::ntx::PAGE_SIZE;
    let item = usize::from(key_length) + 8;
    let mut page = vec![0; ps];
    page[0..2].copy_from_slice(&(keys.len() as u16).to_le_bytes());
    let first = 2 + 2 * (usize::from(max) + 1);
    for i in 0..=usize::from(max) {
        let at = first + i * item;
        page[2 + 2 * i..4 + 2 * i].copy_from_slice(&(at as u16).to_le_bytes());
    }
    for i in 0..=keys.len() {
        let at = first + i * item;
        if let Some(&child) = children.get(i) {
            page[at..at + 4].copy_from_slice(&child.to_le_bytes());
        }
        if let Some(key) = keys.get(i) {
            page[at + 4..at + 8].copy_from_slice(&key.record.to_le_bytes());
            let n = key.bytes.len().min(usize::from(key_length));
            page[at + 8..at + 8 + n].copy_from_slice(&key.bytes[..n]);
        }
    }
    page
}

/// Builds the tree of the NTX file at `path`, just created (see
/// [`ntx_bytes`]), from `keys` in index order at once, as the original
/// engine builds an order on a table with records (`fixtures/write/
/// ntx_create`): the tree as low as its keys allow; a page's keys spread
/// over the fewest children that hold them, evenly (the first ones taking
/// one more), with a key between each two; pages written children first,
/// the root last (at 1,024 if it is the only page).
pub fn build_ntx(path: &Path, keys: &[crate::order::Key]) -> Result<(), crate::order::Error> {
    use std::io::{Read, Seek, SeekFrom};
    let ps = crate::ntx::PAGE_SIZE;
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut header = vec![0; ps];
    file.read_exact(&mut header)?;
    let key_length = u16::from_le_bytes([header[0x0E], header[0x0F]]);
    let max = u16::from_le_bytes([header[0x12], header[0x13]]);
    let m = u64::from(max);
    // The keys a tree of height `h` holds.
    let capacity = |h: u32| (m + 1).saturating_pow(h).saturating_sub(1);
    let mut height = 1;
    while capacity(height) < keys.len() as u64 {
        height += 1;
    }
    let refs: Vec<&crate::order::Key> = keys.iter().collect();
    let mut next = 1u32;
    let mut pages: Vec<(u32, Vec<u8>)> = Vec::new();
    fn build(
        keys: &[&crate::order::Key],
        height: u32,
        key_length: u16,
        max: u16,
        capacity: &dyn Fn(u32) -> u64,
        next: &mut u32,
        pages: &mut Vec<(u32, Vec<u8>)>,
    ) -> u32 {
        let ps = crate::ntx::PAGE_SIZE as u32;
        if height == 1 {
            let page = *next;
            *next += 1;
            pages.push((page, ntx_page(key_length, max, keys, &[])));
            return page * ps;
        }
        let n = keys.len() as u64;
        let below = capacity(height - 1);
        let mut c = 1u64;
        while c * below + (c - 1) < n {
            c += 1;
        }
        let spread = n - (c - 1);
        let (each, extra) = (spread / c, spread % c);
        let mut children = Vec::with_capacity(c as usize);
        let mut separators = Vec::with_capacity(c as usize - 1);
        let mut at = 0usize;
        for i in 0..c {
            let size = (each + u64::from(i < extra)) as usize;
            children.push(build(
                &keys[at..at + size],
                height - 1,
                key_length,
                max,
                capacity,
                next,
                pages,
            ));
            at += size;
            if i + 1 < c {
                separators.push(keys[at]);
                at += 1;
            }
        }
        let page = *next;
        *next += 1;
        pages.push((page, ntx_page(key_length, max, &separators, &children)));
        page * ps
    }
    let root = build(
        &refs, height, key_length, max, &capacity, &mut next, &mut pages,
    );
    for (page, bytes) in pages {
        file.seek(SeekFrom::Start(u64::from(page) * ps as u64))?;
        file.write_all(&bytes)?;
    }
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&root.to_le_bytes())?;
    Ok(())
}

/// What a new CDX order is.
#[derive(Debug, Clone, PartialEq)]
pub struct CdxSpec {
    /// The tag's name, in upper case.
    pub name: String,
    pub expression: String,
    pub condition: String,
    pub key_length: u16,
    pub unique: bool,
    pub descending: bool,
    /// The byte trailing compression leaves out (a space for text keys,
    /// else zero).
    pub pad: u8,
    /// Whether the table is a Visual FoxPro one, whose tag headers say so.
    pub visual: bool,
    /// A custom order, which takes keys only when asked.
    pub custom: bool,
    /// An Advantage binary order: a bitmap of the records whose key is `T`.
    pub binary: bool,
    /// A VFP order on a field that takes NULL: keys of a byte more.
    pub nullable: bool,
    /// A VFP candidate order (unique, refusing a key it has).
    pub candidate: bool,
    /// The options block of a full-text order, which the tag header names.
    pub fts: Option<FtsBlock>,
}

impl CdxSpec {
    /// Whether the order is a full-text one that keeps scores.
    fn keeps_score(&self) -> bool {
        self.fts
            .as_ref()
            .is_some_and(|block| block.options & crate::adi::FTS_KEEP_SCORE != 0)
    }

    /// Writes the tag header at `at` (`fixtures/write/fts_cdx`): the byte
    /// offset of a full-text order's options block is in it.
    fn header_bytes(&self, root: u32, compound: bool, block: u32) -> Vec<u8> {
        let (options, signature) = cdx_options(self, compound);
        let mut header = cdx_header(
            root,
            self.key_length,
            options,
            self.descending,
            &self.expression,
            &self.condition,
        );
        header[0x0F] = signature;
        if self.fts.is_some() {
            header[0x11] = 1;
            header[0x14..0x18].copy_from_slice(&block.to_le_bytes());
        }
        header
    }

    /// The order `tag` of a CDX file as it is, with the expression and
    /// condition as written in the file, to create again.
    pub fn from_tag(
        tag: &crate::order::Tag,
        expression: String,
        condition: String,
        visual: bool,
    ) -> CdxSpec {
        CdxSpec {
            name: tag.name.clone(),
            expression,
            condition,
            key_length: tag.key_length,
            unique: tag.unique(),
            descending: tag.descending,
            pad: tag.pad,
            visual,
            custom: tag.custom(),
            binary: tag.binary(),
            nullable: tag.flags & crate::order::flags::NULLABLE != 0,
            candidate: tag.flags & crate::order::flags::CANDIDATE != 0,
            fts: None,
        }
    }
}

/// A tag's options and signature in its header: compact (and compound in a
/// `.cdx`), unique, custom, `FOR`, full-text; signature 5 for a binary
/// order, 1 for VFP's others.
fn cdx_options(spec: &CdxSpec, compound: bool) -> (u8, u8) {
    let options = if compound { 0x60 } else { 0x20 }
        | if spec.fts.is_some() { 0x10 } else { 0 }
        | if spec.unique { 0x01 } else { 0 }
        | if spec.custom || spec.candidate {
            0x04
        } else {
            0
        }
        | if spec.nullable { 0x02 } else { 0 }
        | if spec.condition.is_empty() { 0 } else { 0x08 };
    let signature = match (spec.binary, spec.visual) {
        (true, _) => 5,
        (false, true) => 1,
        (false, false) => 0,
    };
    (options, signature)
}

/// The root of a new tag: an empty leaf, or for a binary order an empty
/// bitmap.
fn cdx_root(spec: &CdxSpec) -> Vec<u8> {
    let ps = crate::cdx::NODE_SIZE;
    if spec.binary {
        return bitmap_leaf(ps, 0, &[]);
    }
    let pad = spec.condition.is_empty().then_some(spec.pad);
    let leaf = Compact::empty_root();
    let layout = Layout::initial(usize::from(spec.key_length)).scored(spec.keeps_score());
    leaf.bytes(&[], layout, pad, ps).expect("it fits")
}

/// Creates the single-order `.idx` file at `path` for `spec`, as the
/// original engine does (`fixtures/write/cdx_create`): the tag's header
/// and its root after it. Its keys are then built with [`cdx_build_tag`]
/// (the header at 0).
pub fn idx_create(path: &Path, spec: &CdxSpec) -> Result<(), crate::order::Error> {
    let mut bytes = spec.header_bytes(1024, false, 0);
    bytes.extend(cdx_root(spec));
    // A full-text order's options block follows its root
    // (`fixtures/write/fts_cdx`).
    if let Some(block) = &spec.fts {
        let at = bytes.len() as u32;
        bytes[0x14..0x18].copy_from_slice(&at.to_le_bytes());
        let mut block = block.bytes();
        let ps = crate::cdx::NODE_SIZE;
        block.resize(block.len().div_ceil(ps) * ps, 0);
        bytes.extend(block);
    }
    std::fs::write(path, &bytes)?;
    Ok(())
}

/// A bitmap order's root leaf in a page of `ps` bytes: bits for record
/// slots 0 to `slots`, set for the records `set`.
fn bitmap_leaf(ps: usize, slots: u32, set: &[u32]) -> Vec<u8> {
    let mut leaf = vec![0; ps];
    leaf[0..2].copy_from_slice(&3u16.to_le_bytes());
    leaf[2..4].copy_from_slice(&(slots as u16 + 1).to_le_bytes());
    leaf[4..12].fill(0xFF);
    for &record in set {
        leaf[16 + record as usize / 8] |= 1 << (record % 8);
    }
    leaf
}

/// Writes the bitmap of the binary CDX tag whose header is at `header`:
/// bits for record slots 0 to `slots`, set for `records` (those whose key
/// is `T`), in its root; a bitmap of more than one node as the original
/// engine builds it (`fixtures/write/cdx_bitmaps`; see [`build_bitmap`]),
/// its leaves from `counter` if given (a node number, as a reindex takes
/// them), else from the file's end. A bitmap in its root takes no node.
pub fn cdx_build_bitmap(
    path: &Path,
    header: u32,
    slots: u32,
    records: &[u32],
    counter: Option<&mut u32>,
) -> Result<(), crate::order::Error> {
    use std::io::{Read, Seek, SeekFrom};
    let ps = crate::cdx::NODE_SIZE;
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut bytes = [0; 4];
    file.seek(SeekFrom::Start(u64::from(header)))?;
    file.read_exact(&mut bytes)?;
    let root = u32::from_le_bytes(bytes);
    if 16 + (slots as usize + 1).div_ceil(8) > ps {
        return build_bitmap(
            &mut file,
            ps,
            root / ps as u32,
            slots,
            records,
            ps as u32,
            counter,
        );
    }
    file.seek(SeekFrom::Start(u64::from(root)))?;
    file.write_all(&bitmap_leaf(ps, slots, records))?;
    Ok(())
}

/// The layout of a CDX tag directory's leaf, whose record numbers are the
/// offsets of the tags' headers.
const CDX_DIRECTORY_LAYOUT: Layout = Layout {
    r: 24,
    d: 4,
    t: 4,
    s: 4,
};

/// A CDX tag header (1,024 bytes): the root node, the key length, the
/// options (compact, compound, and unique or `FOR`), whether descending, and
/// the key expression and condition in its pool, each ending in a NUL.
fn cdx_header(
    root: u32,
    key_length: u16,
    options: u8,
    descending: bool,
    expression: &str,
    condition: &str,
) -> Vec<u8> {
    let mut header = vec![0; 1024];
    header[0..4].copy_from_slice(&root.to_le_bytes());
    header[0x0C..0x0E].copy_from_slice(&key_length.to_le_bytes());
    header[0x0E] = options;
    header[0x1F6..0x1F8].copy_from_slice(&u16::from(descending).to_le_bytes());
    let expression = crate::cp1252::encode(expression);
    let condition = crate::cp1252::encode(condition);
    let (e, c) = (expression.len() + 1, condition.len() + 1);
    header[0x1F8..0x1FA].copy_from_slice(&(e as u16).to_le_bytes());
    header[0x1FA..0x1FC].copy_from_slice(&(c as u16).to_le_bytes());
    header[0x1FE..0x200].copy_from_slice(&(e as u16).to_le_bytes());
    header[0x200..0x200 + expression.len()].copy_from_slice(&expression);
    header[0x200 + e..0x200 + e + condition.len()].copy_from_slice(&condition);
    header
}

/// Adds the order `spec` to the compound index file at `path`, creating
/// the file if there is none, and builds it from `keys` (in index order) at
/// once, as the original engine does (`fixtures/write/cdx_create`): a new
/// file is the tag directory's header and root leaf; a tag's header, its
/// root leaf, and then its tree are written at the file's end; the
/// directory leaf takes the tag's name, by name, and the directory's change
/// counter counts the tag.
pub fn cdx_add_tag(path: &Path, spec: &CdxSpec) -> Result<u32, crate::order::Error> {
    use crate::order::Key;
    use std::io::{Seek, SeekFrom};
    let ps = crate::cdx::NODE_SIZE;
    // A file that is not there, or empty, starts anew.
    if std::fs::metadata(path).map_or(true, |m| m.len() == 0) {
        let mut bytes = cdx_header(1024, 10, 0xE0, false, "", "");
        let leaf = Compact::empty_root();
        bytes.extend(
            leaf.bytes(&[], CDX_DIRECTORY_LAYOUT, Some(b' '), ps)
                .expect("it fits"),
        );
        std::fs::write(path, &bytes)?;
    }
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let at = file.seek(SeekFrom::End(0))?.div_ceil(ps as u64) * ps as u64;
    let empty = cdx_root(spec);
    // The tag's header goes at the end, then its name into the directory,
    // whose nodes split at the end too (`fixtures/write/tagdir`), then a
    // full-text order's options block and the tag's root leaf
    // (`fixtures/write/fts_cdx`).
    let header = spec.header_bytes(0, true, 0);
    file.seek(SeekFrom::Start(at))?;
    file.write_all(&header)?;
    let mut directory = Directory::cdx(&mut file)?;
    let name = directory.key(&spec.name);
    directory.insert(
        &mut file,
        Key {
            bytes: name,
            record: at as u32,
        },
    )?;
    if let Some(block) = &spec.fts {
        let end = file.seek(SeekFrom::End(0))?;
        let mut bytes = block.bytes();
        bytes.resize(bytes.len().div_ceil(ps) * ps, 0);
        file.seek(SeekFrom::Start(at + 0x14))?;
        file.write_all(&(end as u32).to_le_bytes())?;
        file.seek(SeekFrom::Start(end))?;
        file.write_all(&bytes)?;
    }
    let root = file.seek(SeekFrom::End(0))?;
    file.write_all(&empty)?;
    file.seek(SeekFrom::Start(at))?;
    file.write_all(&(root as u32).to_le_bytes())?;
    directory.count(&mut file)?;
    Ok(at as u32)
}

/// Removes the order `name` from the compound index file at `path`, as the
/// original engine does (`fixtures/write/cdx_drop`): its name leaves the tag
/// directory, whose change counter counts it, and its pages stay as they
/// are. Returns the number of orders left, `None` if the file has no such
/// order.
pub fn cdx_drop_tag(path: &Path, name: &str) -> Result<Option<usize>, crate::order::Error> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut directory = Directory::cdx(&mut file)?;
    let wanted = directory.key(name);
    let Some(at) = directory.names.iter().position(|key| key.bytes == wanted) else {
        return Ok(None);
    };
    directory.remove(&mut file, at)?;
    directory.count(&mut file)?;
    Ok(Some(directory.names.len()))
}

/// Builds the tree of the CDX tag whose header is at `header`, just added
/// (see [`cdx_add_tag`]), from `keys` in index order at once, as the
/// original engine builds an order on a table with records: leaves packed
/// full after the tag's root, which takes the top node (see
/// [`build_order`]). Trailing `pad` bytes are compressed (none for an order
/// with a condition). New nodes come from `counter` if given (a node
/// number, which then passes one node more, as a reindex takes them; see
/// [`rebuild_file`]), else from the file's end.
pub fn cdx_build_tag(
    path: &Path,
    header: u32,
    pad: Option<u8>,
    keys: &[crate::order::Key],
    counter: Option<&mut u32>,
) -> Result<(), crate::order::Error> {
    cdx_build_tag_scored(path, header, pad, keys, counter, None)
}

/// [`cdx_build_tag`] for a full-text order that keeps scores: `score` is the
/// word count of a key.
pub fn cdx_build_tag_scored(
    path: &Path,
    header: u32,
    pad: Option<u8>,
    keys: &[crate::order::Key],
    counter: Option<&mut u32>,
    score: Option<&dyn Fn(&crate::order::Key) -> u16>,
) -> Result<(), crate::order::Error> {
    use std::io::{Read, Seek, SeekFrom};
    if keys.is_empty() {
        return Ok(());
    }
    let ps = crate::cdx::NODE_SIZE;
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut bytes = [0; 16];
    file.seek(SeekFrom::Start(u64::from(header)))?;
    file.read_exact(&mut bytes)?;
    let root = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let key_length = usize::from(u16::from_le_bytes([bytes[0x0C], bytes[0x0D]]));
    build_tree(
        &mut file,
        ps,
        root / ps as u32,
        key_length,
        pad,
        keys,
        ps as u32,
        counter,
        score,
        0,
    )
}

/// The smallest run of each of a memo file's 128 size classes, in blocks,
/// whatever its block size.
const CLASSES: [u32; 128] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 33,
    34, 38, 44, 50, 58, 66, 76, 87, 99, 114, 130, 149, 171, 196, 224, 257, 294, 337, 386, 442, 506,
    579, 663, 760, 870, 996, 1141, 1306, 1495, 1712, 1961, 2245, 2571, 2943, 3370, 3859, 4418,
    5059, 5792, 6632, 7593, 8694, 9955, 11398, 13051, 14943, 17109, 19589, 22429, 25681, 29404,
    33667, 38548, 44136, 50535, 57861, 66249, 75853, 86850, 99441, 113857, 130364, 149263, 170902,
    195678, 224046, 256526, 293716, 336296, 385050, 440871, 504786, 577965, 661754, 757690, 867535,
    993303, 1137304, 1302182, 1490962, 1707110, 1954593, 2237955, 2562396, 2933873, 3359202,
    3846193, 4403784, 5042210, 5773190, 6610142, 7568429, 8665640, 9921917, 11360318, 13007248,
    14892937, 17051998, 19524063, 22354508, 25595289, 29305893,
];

/// The bytes of a new, empty memo file of `block_size`-byte blocks.
pub fn adm_bytes(block_size: u16) -> Vec<u8> {
    let mut bytes = vec![0; 0x900];
    // Rounded up: a block bigger than the header must not start inside it.
    let end = 4096_u32.div_ceil(u32::from(block_size.max(1)));
    bytes[0..4].copy_from_slice(&end.to_be_bytes());
    bytes[6..8].copy_from_slice(&block_size.to_be_bytes());
    bytes[0x0A] = 2;
    for (i, smallest) in CLASSES.iter().enumerate() {
        bytes[0x400 + 8 * i..0x404 + 8 * i].copy_from_slice(&smallest.to_le_bytes());
    }
    for chunk in 1..=32u32 {
        let at = 0x800 + 8 * (chunk as usize - 1);
        bytes[at..at + 4].copy_from_slice(&chunk.to_le_bytes());
    }
    bytes
}

/// Creates the ADT table at `path` with `fields`, and its memo file of
/// `block_size`-byte blocks if it has memo fields. A file that exists
/// already is [`Error::Io`] (`AlreadyExists`).
pub fn adt_table(path: &Path, fields: &[FieldSpec], block_size: u16) -> Result<(), Error> {
    write_adt_table(path, &adt_bytes(fields)?, fields, block_size)
}

/// Like [`adt_table`] but writes the AES-structured, not-yet-armed table (see
/// [`aes_adt_unarmed_bytes`]), for a `CREATE TABLE` on a connection whose
/// `EncryptionType` is AES. `AdsEnableEncryption` arms it afterwards.
pub fn aes_adt_table(path: &Path, fields: &[FieldSpec], block_size: u16) -> Result<(), Error> {
    write_adt_table(path, &aes_adt_unarmed_bytes(fields)?, fields, block_size)
}

/// Writes `table` to `path` and, if any field needs one, an empty memo file.
fn write_adt_table(
    path: &Path,
    table: &[u8],
    fields: &[FieldSpec],
    block_size: u16,
) -> Result<(), Error> {
    let memo = fields.iter().any(|f| f.field_type.is_memo());
    write_new(path, table)?;
    if memo {
        write_new(&path.with_extension("adm"), &adm_bytes(block_size))?;
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

/// An order to create in an ADI file: its name, key expression and `FOR`
/// condition (fields as `F<n>`, as the file keeps them), the length of its
/// keys, its options and flags (see [`crate::order`]), the byte its keys
/// are padded with, whether it is descending, its collation (empty for
/// none) and, for an NChar or NVarChar order, its locale (see
/// [`crate::order::locale`]; empty for any other).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderSpec {
    pub name: String,
    pub expression: String,
    pub condition: String,
    pub key_length: u16,
    pub options: u8,
    pub flags: u32,
    pub pad: u8,
    pub descending: bool,
    pub collation: String,
    pub locale: String,
    /// The key width option of a Unicode order (see
    /// [`crate::order::locale::WIDTH`]).
    pub uchar_width: u8,
    /// The options block of a full-text order.
    pub fts: Option<FtsBlock>,
}

/// The options block of a full-text order, as the original engine writes it
/// (`fixtures/write/fts_build`): a version (1), the block's length, the
/// `ADS_FTS_*` options (without `ADS_FTS_INDEX`), the shortest and longest
/// word, then the delimiters, noise words, drop and conditional characters,
/// each a length and its bytes with a NUL. It starts on a page of its own
/// after the tag header and fills its last page with zeros.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtsBlock {
    pub options: u32,
    pub min_word: u32,
    pub max_word: u32,
    pub delimiters: Vec<u8>,
    /// The noise words, separated by single spaces.
    pub noise: Vec<u8>,
    pub drop: Vec<u8>,
    pub conditional: Vec<u8>,
}

impl FtsBlock {
    pub fn bytes(&self) -> Vec<u8> {
        let mut bytes = vec![0; 8];
        bytes[0..4].copy_from_slice(&1u32.to_le_bytes());
        for value in [self.options, self.min_word, self.max_word] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for text in [&self.delimiters, &self.noise, &self.drop, &self.conditional] {
            bytes.extend_from_slice(&(text.len() as u32 + 1).to_le_bytes());
            bytes.extend_from_slice(text);
            bytes.push(0);
        }
        let length = bytes.len() as u32;
        bytes[4..8].copy_from_slice(&length.to_le_bytes());
        bytes
    }
}

impl OrderSpec {
    /// Whether the order is a full-text one that keeps scores.
    fn keeps_score(&self) -> bool {
        self.fts
            .as_ref()
            .is_some_and(|block| block.options & crate::adi::FTS_KEEP_SCORE != 0)
    }

    /// The order `tag` of an ADI file as it is, to create again.
    pub fn from_tag(tag: &crate::order::Tag) -> OrderSpec {
        OrderSpec {
            name: tag.name.clone(),
            expression: tag.expression.clone(),
            condition: tag.condition.clone(),
            key_length: tag.key_length,
            options: tag.options,
            flags: tag.flags,
            pad: tag.pad,
            descending: tag.descending,
            collation: tag.collation.clone(),
            locale: tag.locale.clone(),
            uchar_width: tag.uchar_width,
            fts: None,
        }
    }
}

/// The size of an ADI file header and of a tag header.
const ADI_HEADER: usize = 1024;

/// Set in the options of every header the original engine writes.
const ADI_OPTIONS: u8 = 0x60;

/// Where an ADI file's header has what looks like the version of the
/// format its orders need, the highest of: 1, 2 for pages larger than 512
/// bytes, 3 with a full-text order, 4 with keys over 240 bytes, 6 with a
/// collation, 7 with a binary order (learnt from the fixtures; dropping an
/// order leaves it).
pub const VERSION: usize = 0x08;

/// The version an order needs (see [`VERSION`]).
fn version(order: &OrderSpec) -> u8 {
    use crate::order::flags;
    if order.flags & flags::BINARY != 0 {
        7
    } else if !order.collation.is_empty() {
        6
    } else if order.key_length > 240 {
        4
    } else if order.flags & flags::FULL_TEXT != 0 {
        3
    } else {
        1
    }
}

/// The layout of the tag directory's leaf: tag names of 128 bytes, whose
/// records are their tag headers' pages.
const DIRECTORY: Layout = Layout {
    r: 32,
    d: 8,
    t: 8,
    s: 6,
};

/// The tag directory of an index file: the root leaf of the tags' names,
/// whose records are the places of their headers. The formats differ in the
/// length of a name, the leaf's layout and where the leaf is.
struct Directory {
    /// The file's first 1,024 bytes.
    header: Vec<u8>,
    /// The size of a page or node.
    ps: usize,
    /// The byte position of the leaf.
    at: u64,
    key_length: usize,
    layout: Layout,
    /// What a node number counts in bytes (see [`Directory::read`]).
    unit: u64,
    /// The root's node number.
    root: u32,
    /// Whether the root is an interior node: the file has more tags than a
    /// leaf takes (`fixtures/write/tagdir`).
    deep: bool,
    names: Vec<crate::order::Key>,
}

impl Directory {
    /// The directory of the ADI file `file`.
    fn adi(file: &mut std::fs::File) -> Result<Directory, crate::order::Error> {
        use std::io::{Read, Seek, SeekFrom};
        let mut header = vec![0; ADI_HEADER];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut header)?;
        let ps = usize::from(u16::from_le_bytes([header[0x12], header[0x13]]));
        let root = u32::from_le_bytes(header[0..4].try_into().unwrap());
        let at = u64::from(root) * ps as u64;
        Directory::read(file, header, root, at, ps, 128, DIRECTORY, ps as u64)
    }

    /// The directory of the compound CDX file `file`.
    fn cdx(file: &mut std::fs::File) -> Result<Directory, crate::order::Error> {
        use std::io::{Read, Seek, SeekFrom};
        let mut header = vec![0; 1024];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut header)?;
        let root = u32::from_le_bytes(header[0..4].try_into().unwrap());
        let ps = crate::cdx::NODE_SIZE;
        Directory::read(
            file,
            header,
            root,
            u64::from(root),
            ps,
            10,
            CDX_DIRECTORY_LAYOUT,
            1,
        )
    }

    /// Reads the directory whose leaf is at byte `at`, which the file's
    /// header calls `root`: a page number in an ADI file, the offset in a CDX
    /// file.
    #[allow(clippy::too_many_arguments)]
    fn read(
        file: &mut std::fs::File,
        header: Vec<u8>,
        root: u32,
        at: u64,
        ps: usize,
        key_length: usize,
        layout: Layout,
        unit: u64,
    ) -> Result<Directory, crate::order::Error> {
        use std::io::{Read, Seek, SeekFrom};
        let mut leaf = vec![0; ps];
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(&mut leaf)?;
        let tree = crate::adi::TreeRef {
            root,
            key_length: key_length as u16,
            pad: b' ',
            binary: false,
        };
        let mut names = Vec::new();
        // Bit 1 of the root's attributes: a leaf.
        let deep = leaf[0] & 2 == 0;
        if deep {
            names = crate::maintain::directory_keys(file, ps, unit, root, key_length)?;
        } else {
            crate::adi::decode_leaf(&leaf, &tree, &mut names)?;
        }
        Ok(Directory {
            header,
            ps,
            at,
            key_length,
            layout,
            unit,
            root,
            deep,
            names,
        })
    }

    /// Adds the entry `key` to the directory in `file`: a leaf that is full
    /// splits as an order's does.
    fn insert(
        &mut self,
        file: &mut std::fs::File,
        key: crate::order::Key,
    ) -> Result<(), crate::order::Error> {
        crate::maintain::directory_insert(
            file,
            self.ps,
            self.unit,
            self.root,
            self.key_length,
            key.clone(),
        )?;
        let at = self.names.partition_point(|k| k.bytes < key.bytes);
        self.names.insert(at, key);
        Ok(())
    }

    /// Removes the entry at `at` of the names from the directory in `file`.
    fn remove(&mut self, file: &mut std::fs::File, at: usize) -> Result<(), crate::order::Error> {
        let key = self.names.remove(at);
        if !self.deep {
            return self.write(file);
        }
        crate::maintain::directory_delete(
            file,
            self.ps,
            self.unit,
            self.root,
            self.key_length,
            &key,
        )
    }

    /// The directory's key of the order `name`: in upper case, padded.
    fn key(&self, name: &str) -> Vec<u8> {
        let mut key = crate::cp1252::encode(&name.to_ascii_uppercase());
        key.resize(self.key_length, b' ');
        key
    }

    /// The directory's entry of the order `name`.
    fn find(&self, name: &str) -> Option<&crate::order::Key> {
        let wanted = self.key(name);
        self.names.iter().find(|key| key.bytes == wanted)
    }

    /// Writes the leaf of `names` back.
    fn write(&self, file: &mut std::fs::File) -> Result<(), crate::order::Error> {
        use std::io::{Seek, SeekFrom};
        let leaf = Compact::empty_root()
            .bytes(&self.names, self.layout, Some(b' '), self.ps)
            .ok_or(crate::order::Error::Corrupt("tag directory full"))?;
        file.seek(SeekFrom::Start(self.at))?;
        Ok(file.write_all(&leaf)?)
    }

    /// Counts a change of the directory of a CDX file in its change counter
    /// (big-endian, at 0x0A).
    fn count(&self, file: &mut std::fs::File) -> Result<(), crate::order::Error> {
        use std::io::{Seek, SeekFrom};
        let count = u16::from_be_bytes([self.header[0x0A], self.header[0x0B]]).wrapping_add(1);
        file.seek(SeekFrom::Start(0x0A))?;
        Ok(file.write_all(&count.to_be_bytes())?)
    }
}

/// The bytes of a new ADI file of `page_size`-byte pages without orders,
/// for a collation whose byte-to-weight table has the CRC-32 `collation`
/// (that of the bytes 0 to 255 without one): its header and its empty tag
/// directory.
pub fn adi_bytes(page_size: u16, collation: u32) -> Vec<u8> {
    let ps = usize::from(page_size);
    let directory = ADI_HEADER.div_ceil(ps);
    let mut bytes = vec![0; directory * ps];
    bytes[0..4].copy_from_slice(&(directory as u32).to_le_bytes());
    bytes[VERSION] = if page_size > 512 { 2 } else { 1 };
    bytes[0x0C..0x0E].copy_from_slice(&128u16.to_le_bytes());
    bytes[0x0E] = ADI_OPTIONS;
    bytes[0x0F] = b' ';
    bytes[0x10..0x12].copy_from_slice(&0x0400u16.to_le_bytes());
    bytes[0x12..0x14].copy_from_slice(&page_size.to_le_bytes());
    bytes[0x14..0x18].copy_from_slice(&collation.to_le_bytes());
    bytes[0x1FA] = 1;
    bytes[0x1FE] = 1;
    let leaf = Compact::empty_root();
    bytes.extend(
        leaf.bytes(&[], DIRECTORY, Some(b' '), ps)
            .expect("an empty leaf fits"),
    );
    bytes
}

const NONE: u32 = 0xFFFF_FFFF;

/// The bytes of the tag header of `order`, whose tree's root is page
/// `root`.
fn tag_header(order: &OrderSpec, root: u32) -> Result<[u8; ADI_HEADER], crate::order::Error> {
    use crate::order::Error;
    let mut bytes = [0; ADI_HEADER];
    bytes[0..4].copy_from_slice(&root.to_le_bytes());
    bytes[0x0C..0x0E].copy_from_slice(&order.key_length.to_le_bytes());
    bytes[0x0E] = ADI_OPTIONS | order.options;
    bytes[0x0F] = order.pad;
    bytes[0x10..0x12].copy_from_slice(&0x0400u16.to_le_bytes());
    if order.options & crate::order::options::FLAGS != 0 {
        bytes[0x18..0x1C].copy_from_slice(&order.flags.to_le_bytes());
    }
    let collation = crate::cp1252::encode(&order.collation);
    if collation.len() > 0x1F6 - 0x34 {
        return Err(Error::Corrupt("collation name too long"));
    }
    bytes[0x34..0x34 + collation.len()].copy_from_slice(&collation);
    if !order.locale.is_empty() {
        use crate::order::locale;
        let name = crate::cp1252::encode(&order.locale);
        bytes[locale::OFFSET..locale::OFFSET + name.len()].copy_from_slice(&name);
        bytes[locale::END..locale::END + 4].copy_from_slice(&locale::TAIL);
        bytes[locale::WIDTH] = order.uchar_width;
    }
    if order.descending {
        bytes[0x1F6] = 1;
    }
    let expression = crate::cp1252::encode(&order.expression);
    let condition = crate::cp1252::encode(&order.condition);
    let pool = expression.len() + 1 + condition.len() + 1;
    if pool > ADI_HEADER - 0x200 {
        return Err(Error::Corrupt("expressions too long"));
    }
    bytes[0x1FA..0x1FC].copy_from_slice(&(condition.len() as u16 + 1).to_le_bytes());
    bytes[0x1FE..0x200].copy_from_slice(&(expression.len() as u16 + 1).to_le_bytes());
    bytes[0x200..0x200 + expression.len()].copy_from_slice(&expression);
    let at = 0x200 + expression.len() + 1;
    bytes[at..at + condition.len()].copy_from_slice(&condition);
    Ok(bytes)
}

/// The longest key of an order in an ADI file: what the original engine
/// allows in a page of 8,192 bytes.
pub const ADI_MAX_KEY: u16 = 4082;

/// Whether an order of `key_length`-byte keys fits an ADI file of
/// `page_size`-byte pages: a page holds two keys besides its 28 bytes of
/// header (`fixtures/write/limits`).
pub fn key_fits(key_length: u16, page_size: u16) -> bool {
    2 * u32::from(key_length) + 28 <= u32::from(page_size)
}

/// The page size an ADI file of `page_size`-byte pages grows to when an
/// order of `key_length`-byte keys that does not fit is added to it, and
/// that a new file takes by default: the double of the page size, again and
/// again, until the page holds three keys (and 36 bytes), or 8,192
/// (`fixtures/write/limits`).
pub fn grown_page_size(key_length: u16, page_size: u16) -> u16 {
    let mut size = u32::from(page_size);
    while size < 8192 && 3 * u32::from(key_length) + 36 > size {
        size *= 2;
    }
    size as u16
}

/// Adds `order` to the ADI file at `path`, created if it does not exist
/// (with `page_size`-byte pages and the collation CRC `collation`): its tag
/// header and an empty root leaf go at the file's end, and its name into
/// the tag directory. Returns `false` without adding it if the file has an
/// order of that name.
pub fn add_order(
    path: &Path,
    order: &OrderSpec,
    page_size: u16,
    collation: u32,
) -> Result<bool, crate::order::Error> {
    add_or_replace(path, order, page_size, collation, false)
}

/// [`add_order`], but an order of that name the file has is replaced, as
/// the original engine replaces it (`AdsCreateIndex`): its tag header, in
/// its place, describes `order`, whose empty root goes at the file's end
/// (the old tree's pages stay as they are).
pub fn replace_order(
    path: &Path,
    order: &OrderSpec,
    page_size: u16,
    collation: u32,
) -> Result<(), crate::order::Error> {
    add_or_replace(path, order, page_size, collation, true).map(|_| ())
}

fn add_or_replace(
    path: &Path,
    order: &OrderSpec,
    page_size: u16,
    collation: u32,
    replace: bool,
) -> Result<bool, crate::order::Error> {
    use crate::order::{Error as OrderError, Key};
    use std::io::{Seek, SeekFrom};
    let mut file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(path)?;
            file.write_all(&adi_bytes(page_size, collation))?;
            file
        }
        Err(e) => return Err(e.into()),
    };
    let mut directory = Directory::adi(&mut file)?;
    let ps = directory.ps;
    let name = directory.key(&order.name);
    if let Some(existing) = directory.names.iter().find(|key| key.bytes == name) {
        if !replace {
            return Ok(false);
        }
        let header_page = existing.record;
        let block = write_fts_block(&mut file, order, ps)?;
        let end = file.seek(SeekFrom::End(0))?;
        let root = end.div_ceil(ps as u64) as u32;
        let mut header = tag_header(order, root)?;
        header[0x1C..0x20].copy_from_slice(&block.to_le_bytes());
        file.seek(SeekFrom::Start(u64::from(header_page) * ps as u64))?;
        file.write_all(&header)?;
        let empty = Compact::empty_root();
        let layout = Layout::initial(usize::from(order.key_length)).scored(order.keeps_score());
        let leaf = empty
            .bytes(&[], layout, Some(order.pad), ps)
            .ok_or(OrderError::Corrupt("key too long"))?;
        file.seek(SeekFrom::Start(u64::from(root) * ps as u64))?;
        file.write_all(&leaf)?;
        return Ok(true);
    }
    // The tag header goes at the end, then the name into the directory, whose
    // nodes split at the end too (`fixtures/write/tagdir`), and then the
    // root leaf.
    let end = file.seek(SeekFrom::End(0))?;
    let header_page = end.div_ceil(ps as u64) as u32;
    let header_pages = ADI_HEADER.div_ceil(ps);
    let mut pages = vec![0; header_pages * ps];
    pages[..ADI_HEADER].copy_from_slice(&tag_header(order, 0)?);
    file.seek(SeekFrom::Start(u64::from(header_page) * ps as u64))?;
    file.write_all(&pages)?;
    directory.insert(
        &mut file,
        Key {
            bytes: name,
            record: header_page,
        },
    )?;
    // A full-text order's options block, then its root (`fixtures/write/fts_build`).
    let block = write_fts_block(&mut file, order, ps)?;
    if block != 0 {
        file.seek(SeekFrom::Start(u64::from(header_page) * ps as u64 + 0x1C))?;
        file.write_all(&block.to_le_bytes())?;
    }
    let root = (file.seek(SeekFrom::End(0))?.div_ceil(ps as u64)) as u32;
    let leaf = if order.flags & crate::order::flags::BINARY != 0 {
        // A bitmap with the bit of record 0.
        bitmap_leaf(ps, 0, &[])
    } else {
        let layout = Layout::initial(usize::from(order.key_length)).scored(order.keeps_score());
        Compact::empty_root()
            .bytes(&[], layout, Some(order.pad), ps)
            .ok_or(OrderError::Corrupt("key too long"))?
    };
    file.seek(SeekFrom::Start(u64::from(root) * ps as u64))?;
    file.write_all(&leaf)?;
    file.seek(SeekFrom::Start(u64::from(header_page) * ps as u64))?;
    file.write_all(&root.to_le_bytes())?;
    let needed = directory.header[VERSION].max(version(order));
    file.seek(SeekFrom::Start(VERSION as u64))?;
    file.write_all(&[needed])?;
    Ok(true)
}

/// Writes the options block of `order`, if it is a full-text order, at the
/// end of `file` (pages of `ps` bytes): its page, or 0 for any other order.
fn write_fts_block(
    file: &mut std::fs::File,
    order: &OrderSpec,
    ps: usize,
) -> Result<u32, crate::order::Error> {
    use std::io::{Seek, SeekFrom};
    let Some(block) = &order.fts else {
        return Ok(0);
    };
    let end = file.seek(SeekFrom::End(0))?;
    let page = end.div_ceil(ps as u64) as u32;
    let mut bytes = block.bytes();
    bytes.resize(bytes.len().div_ceil(ps) * ps, 0);
    file.seek(SeekFrom::Start(u64::from(page) * ps as u64))?;
    file.write_all(&bytes)?;
    Ok(page)
}

/// A node of a level being built that waits for its right sibling's page:
/// its page, entries and left sibling.
type Waiting = (u32, Vec<(crate::order::Key, u32)>, u32);

/// Writes an interior node: at a page, its entries, its left and right
/// siblings.
type WriteNode<'a> =
    dyn FnMut(u32, &[(crate::order::Key, u32)], u32, u32) -> std::io::Result<()> + 'a;

/// Builds the tree of the order `name` of the ADI file at `path`, just
/// added and empty, from `keys` in the order's order (by their bytes, then
/// record numbers), at once as the original engine builds an order on a
/// table with records (`fixtures/write/build`): leaves packed as full as
/// keys fit, in sequence; an interior node written as soon as it fills (or
/// at the end), each level's nodes linked to their siblings; the top node
/// in the root page the order was created with (a single leaf too). A
/// binary order's root is given the bits of record slots 0 to `slots`, set
/// for the `deleted` ones.
pub fn build_order(
    path: &Path,
    name: &str,
    keys: &[crate::order::Key],
    slots: u32,
    deleted: &[u32],
) -> Result<(), crate::order::Error> {
    build_order_at(path, name, keys, slots, deleted, 0, None, None, 0)
}

/// [`build_order`] keeping the last `reserve` bytes of each page free, as
/// the index of an AES-encrypted table keeps 8 for each page's IV
/// (`fixtures/aes/dictwrite`); the pages are enciphered after (see
/// [`crate::adi::encipher_tree_pages`]).
pub fn build_order_reserving(
    path: &Path,
    name: &str,
    keys: &[crate::order::Key],
    slots: u32,
    deleted: &[u32],
    reserve: usize,
) -> Result<(), crate::order::Error> {
    build_order_at(path, name, keys, slots, deleted, 0, None, None, reserve)
}

/// [`build_order`] for a full-text order that keeps scores: `scores` are the
/// word counts of `keys`, one for each.
pub fn build_scored_order(
    path: &Path,
    name: &str,
    keys: &[crate::order::Key],
    scores: &[u16],
) -> Result<(), crate::order::Error> {
    let scores: std::collections::HashMap<(u32, &[u8]), u16> = keys
        .iter()
        .zip(scores)
        .map(|(key, &score)| ((key.record, key.bytes.as_slice()), score))
        .collect();
    let score = |key: &crate::order::Key| {
        scores
            .get(&(key.record, key.bytes.as_slice()))
            .copied()
            .unwrap_or(0)
    };
    build_order_at(path, name, keys, 0, &[], 0, None, Some(&score), 0)
}

/// [`build_order`], taking new pages from `next` (else from the file's
/// end). As in the original engine, a build that takes its pages from
/// `next` takes one more it does not write, which leaves a page of zeros
/// before the next order's pages when it rebuilds an index file.
#[allow(clippy::too_many_arguments)]
fn build_order_at(
    path: &Path,
    name: &str,
    keys: &[crate::order::Key],
    slots: u32,
    deleted: &[u32],
    changes: u32,
    counter: Option<&mut u32>,
    score: Option<&dyn Fn(&crate::order::Key) -> u16>,
    reserve: usize,
) -> Result<(), crate::order::Error> {
    use crate::order::Error as OrderError;
    use std::io::{Read, Seek, SeekFrom};
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let directory = Directory::adi(&mut file)?;
    let ps = directory.ps;
    let tag = directory
        .find(name)
        .ok_or(OrderError::Corrupt("no such order"))?
        .record;
    let mut tag_header = [0; ADI_HEADER];
    file.seek(SeekFrom::Start(u64::from(tag) * ps as u64))?;
    file.read_exact(&mut tag_header)?;
    if changes != 0 {
        file.seek(SeekFrom::Start(u64::from(tag) * ps as u64 + 8))?;
        file.write_all(&changes.to_le_bytes())?;
    }
    let root = u32::from_le_bytes(tag_header[0..4].try_into().unwrap());
    let key_length = usize::from(u16::from_le_bytes([tag_header[0x0C], tag_header[0x0D]]));
    let pad = Some(tag_header[0x0F]);
    let binary = tag_header[0x0E] & crate::order::options::FLAGS != 0
        && u32::from_le_bytes(tag_header[0x18..0x1C].try_into().unwrap())
            & crate::order::flags::BINARY
            != 0;
    let put = |file: &mut std::fs::File, at: u32, bytes: &[u8]| -> std::io::Result<()> {
        file.seek(SeekFrom::Start(u64::from(at) * ps as u64))?;
        file.write_all(bytes)
    };
    if binary {
        // Bits for record slots 0 to `slots`, set for the deleted ones.
        let capacity = ((ps - 16) * 8) as u32;
        if slots + 1 > capacity {
            return build_bitmap(&mut file, ps, root, slots, deleted, 1, counter);
        }
        if let Some(next) = counter {
            *next += 1;
        }
        return Ok(put(&mut file, root, &bitmap_leaf(ps, slots, deleted))?);
    }
    build_tree(
        &mut file, ps, root, key_length, pad, keys, 1, counter, score, reserve,
    )
}

/// Builds a tree of compact leaves and interior nodes in `file` (nodes of
/// `ps` bytes) from `keys`, as the original engine builds an order at once
/// (see [`build_order`]): the root at node `root`, new nodes from `counter`
/// (else the file's end); nodes refer to each other by node number times
/// `unit` (1 in an ADI file, the node size in a CDX file, whose nodes are
/// byte offsets). The last `reserve` bytes of an interior node, and as many
/// of a leaf's free bytes, are kept free (for an AES index's IV, which takes
/// the page's last 8 bytes, a leaf's moved into its free space).
#[allow(clippy::too_many_arguments)]
fn build_tree(
    file: &mut std::fs::File,
    ps: usize,
    root: u32,
    key_length: usize,
    pad: Option<u8>,
    keys: &[crate::order::Key],
    unit: u32,
    counter: Option<&mut u32>,
    score: Option<&dyn Fn(&crate::order::Key) -> u16>,
    reserve: usize,
) -> Result<(), crate::order::Error> {
    use crate::order::Key;
    use std::io::{Seek, SeekFrom};
    let put = |file: &mut std::fs::File, at: u32, bytes: &[u8]| -> std::io::Result<()> {
        file.seek(SeekFrom::Start(u64::from(at) * ps as u64))?;
        file.write_all(bytes)
    };
    let addr = |page: u32| if page == NONE { NONE } else { page * unit };
    if keys.is_empty() {
        return Ok(());
    }
    let most = keys.iter().map(|k| k.record).max().unwrap_or(0);
    let mut layout = Layout::initial(key_length);
    if most > Layout::mask(layout.r) as u32 {
        layout = Layout::widened(key_length, most);
    }
    let layout = layout.scored(score.is_some());
    let score = score.unwrap_or(&|_| 0);
    // The leaves, packed.
    let mut leaves: Vec<Vec<Key>> = vec![Vec::new()];
    let fits = |keys: &[Key]| {
        let leaf = Compact {
            root: false,
            left: NONE,
            right: NONE,
        };
        leaf.bytes_scored(keys, layout, pad, ps - reserve, score)
            .is_some()
    };
    for key in keys {
        let last = leaves.last_mut().unwrap();
        last.push(key.clone());
        if !fits(last) {
            let key = last.pop().unwrap();
            leaves.push(vec![key]);
        }
    }
    if leaves.len() == 1 {
        let leaf = Compact::empty_root();
        let bytes = leaf
            .bytes_scored(&leaves[0], layout, pad, ps, score)
            .expect("it fits");
        if let Some(next) = counter {
            *next += 1;
        }
        return Ok(put(file, root, &bytes)?);
    }
    let mut next = match counter.as_deref() {
        Some(&next) => next,
        None => file.seek(SeekFrom::End(0))?.div_ceil(ps as u64) as u32,
    };
    let capacity = (ps - 12 - reserve) / (key_length + 8);
    // Per level, the entries of the node being filled, and the node before
    // it waiting for its right sibling's page.
    let mut levels: Vec<Vec<(Key, u32)>> = Vec::new();
    let mut waiting: Vec<Option<Waiting>> = Vec::new();
    let interior = |entries: &[(Key, u32)], root: bool, left: u32, right: u32| {
        let mut bytes = vec![0; ps];
        bytes[0..2].copy_from_slice(&u16::from(root).to_le_bytes());
        bytes[2..4].copy_from_slice(&(entries.len() as u16).to_le_bytes());
        bytes[4..8].copy_from_slice(&addr(left).to_le_bytes());
        bytes[8..12].copy_from_slice(&addr(right).to_le_bytes());
        for (i, (key, child)) in entries.iter().enumerate() {
            let at = 12 + i * (key_length + 8);
            bytes[at..at + key_length].copy_from_slice(&key.bytes);
            bytes[at + key_length..at + key_length + 4].copy_from_slice(&key.record.to_be_bytes());
            bytes[at + key_length + 4..at + key_length + 8]
                .copy_from_slice(&addr(*child).to_be_bytes());
        }
        bytes
    };
    // Writes the node `level` is filling, full, to a new page (its left
    // sibling, waiting, now that its right sibling's page is known), and
    // adds its entry to the level above.
    fn flush(
        level: usize,
        levels: &mut Vec<Vec<(Key, u32)>>,
        waiting: &mut Vec<Option<Waiting>>,
        next: &mut u32,
        capacity: usize,
        write: &mut WriteNode,
    ) -> std::io::Result<()> {
        let entries = std::mem::take(&mut levels[level]);
        let page = *next;
        *next += 1;
        let left = match waiting[level].take() {
            Some((before, entries, left)) => {
                write(before, &entries, left, page)?;
                before
            }
            None => NONE,
        };
        let last = entries.last().unwrap().0.clone();
        waiting[level] = Some((page, entries, left));
        add(
            level + 1,
            (last, page),
            levels,
            waiting,
            next,
            capacity,
            write,
        )
    }
    // Adds the entry of a node (its last key, its page) to `level`; a full
    // node there is written first.
    fn add(
        level: usize,
        entry: (Key, u32),
        levels: &mut Vec<Vec<(Key, u32)>>,
        waiting: &mut Vec<Option<Waiting>>,
        next: &mut u32,
        capacity: usize,
        write: &mut WriteNode,
    ) -> std::io::Result<()> {
        if levels.len() <= level {
            levels.push(Vec::new());
            waiting.push(None);
        }
        if levels[level].len() == capacity {
            flush(level, levels, waiting, next, capacity, write)?;
        }
        levels[level].push(entry);
        Ok(())
    }
    let mut write = |at: u32, entries: &[(Key, u32)], left: u32, right: u32| {
        let bytes = interior(entries, false, left, right);
        put(file, at, &bytes)
    };
    let mut writes: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut previous: Option<(u32, usize, u32)> = None;
    for (i, leaf_keys) in leaves.iter().enumerate() {
        // A full node above is written before the leaf takes its page.
        if levels
            .first()
            .is_some_and(|entries| entries.len() == capacity)
        {
            flush(
                0,
                &mut levels,
                &mut waiting,
                &mut next,
                capacity,
                &mut write,
            )?;
        }
        let at = next;
        next += 1;
        if let Some((before, index, left)) = previous.take() {
            let leaf = Compact {
                root: false,
                left: addr(left),
                right: addr(at),
            };
            writes.push((
                before,
                leaf.bytes_scored(&leaves[index], layout, pad, ps, score)
                    .expect("it fits"),
            ));
            previous = Some((at, i, before));
        } else {
            previous = Some((at, i, NONE));
        }
        let last = leaf_keys.last().unwrap().clone();
        add(
            0,
            (last, at),
            &mut levels,
            &mut waiting,
            &mut next,
            capacity,
            &mut write,
        )?;
    }
    if let Some((at, index, left)) = previous {
        let leaf = Compact {
            root: false,
            left: addr(left),
            right: NONE,
        };
        writes.push((
            at,
            leaf.bytes_scored(&leaves[index], layout, pad, ps, score)
                .expect("it fits"),
        ));
    }
    // The nodes being filled, bottom up; the top one is the root.
    let mut level = 0;
    while level < levels.len() {
        let top = levels[level + 1..].iter().all(Vec::is_empty)
            && waiting[level + 1..].iter().all(Option::is_none)
            && waiting[level].is_none();
        let entries = std::mem::take(&mut levels[level]);
        if top {
            if !entries.is_empty() {
                writes.push((root, interior(&entries, true, NONE, NONE)));
            }
            break;
        }
        let left = match waiting[level].take() {
            Some((before, before_entries, left)) => {
                if entries.is_empty() {
                    // The level ends with a full node.
                    write(before, &before_entries, left, NONE)?;
                    level += 1;
                    continue;
                }
                let page = next;
                write(before, &before_entries, left, page)?;
                before
            }
            None => NONE,
        };
        if entries.is_empty() {
            level += 1;
            continue;
        }
        let page = next;
        next += 1;
        write(page, &entries, left, NONE)?;
        let last = entries.last().unwrap().0.clone();
        // A full node above is written first, as for a leaf's entry.
        add(
            level + 1,
            (last, page),
            &mut levels,
            &mut waiting,
            &mut next,
            capacity,
            &mut write,
        )?;
        level += 1;
    }
    for (at, bytes) in writes {
        put(file, at, &bytes)?;
    }
    if let Some(counter) = counter {
        *counter = next + 1;
    }
    Ok(())
}

/// An order of an index file to rebuild: what it is, its keys, and for a
/// binary order the record slots and the deleted ones (see
/// [`build_order`]).
pub struct Rebuilt {
    pub spec: OrderSpec,
    pub keys: Vec<crate::order::Key>,
    pub slots: u32,
    pub deleted: Vec<u32>,
    /// The record writes that changed the order's keys while the file was
    /// open, which the original engine leaves in the tag header at 0x08 when
    /// it rebuilds (`fixtures/write/cdx_custom_reindex`).
    pub changes: u32,
    /// A full-text order's word counts, one for each key, if it keeps scores.
    pub scores: Vec<u16>,
}

/// Writes the ADI file at `path` anew with `orders` (in the order they were
/// created), each built from its keys, as the original engine rewrites an
/// index file to reindex or pack its table: the header and the tag
/// directory, every order's tag header and root, then each order's tree,
/// taking pages one after the other (see [`build_order`]). The page size is
/// `page_size` if given, else the file's; the collation is the file's, and
/// so is the version unless the page size changes.
pub fn rebuild_file(
    path: &Path,
    orders: &[Rebuilt],
    page_size: Option<u16>,
) -> Result<(), crate::order::Error> {
    rebuild_file_collated(path, orders, page_size, None, 0)
}

/// [`rebuild_file`], with the collation (the CRC of its weights, see
/// [`collation_crc`]) the file records set to `collation` if given: the one
/// its keys are weighed by now. Each page keeps its last `reserve` bytes
/// free (see [`build_order_reserving`]).
pub fn rebuild_file_collated(
    path: &Path,
    orders: &[Rebuilt],
    page_size: Option<u16>,
    collation: Option<u32>,
    reserve: usize,
) -> Result<(), crate::order::Error> {
    use std::io::Read;
    let mut header = [0; ADI_HEADER];
    OpenOptions::new()
        .read(true)
        .open(path)?
        .read_exact(&mut header)?;
    let changed = page_size.is_some();
    let page_size = page_size.unwrap_or_else(|| u16::from_le_bytes([header[0x12], header[0x13]]));
    let collation =
        collation.unwrap_or_else(|| u32::from_le_bytes(header[0x14..0x18].try_into().unwrap()));
    let mut bytes = adi_bytes(page_size, collation);
    // A new page size brings the version its size has (`fixtures/sql/spt`),
    // or the highest one its orders need, a binary order's 7 among them
    // (`fixtures/write/reindex_pages`, `fixtures/write/limits`).
    let version = if changed {
        orders
            .iter()
            .map(|order| version(&order.spec))
            .fold(bytes[VERSION], u8::max)
    } else {
        header[VERSION]
    };
    bytes[VERSION] = version;
    std::fs::write(path, &bytes)?;
    for order in orders {
        add_order(path, &order.spec, page_size, collation)?;
    }
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    file.write_all(&bytes[..VERSION])?;
    file.write_all(&[version])?;
    let mut next = (std::fs::metadata(path)?.len() / u64::from(page_size)) as u32;
    for order in orders {
        let scores: std::collections::HashMap<(u32, &[u8]), u16> = order
            .keys
            .iter()
            .zip(&order.scores)
            .map(|(key, &score)| ((key.record, key.bytes.as_slice()), score))
            .collect();
        let score = |key: &crate::order::Key| {
            scores
                .get(&(key.record, key.bytes.as_slice()))
                .copied()
                .unwrap_or(0)
        };
        build_order_at(
            path,
            &order.spec.name,
            &order.keys,
            order.slots,
            &order.deleted,
            order.changes,
            Some(&mut next),
            order.spec.keeps_score().then_some(&score),
            reserve,
        )?;
    }
    Ok(())
}

/// Builds a binary order's bitmap of more than one page, as the original
/// engine builds it on a table with records (`fixtures/write/bitmaps`,
/// `fixtures/write/cdx_bitmaps`): its leaves, each of `(page size - 16) × 8`
/// bits, on new pages one after the other, linked to their siblings, each
/// with its first record number (big-endian) and its bits for the slots
/// from it on, set for the `deleted` ones (in a CDX file, those whose key is
/// `T`); the root page an interior node with each leaf's last record number
/// (by its capacity) and page, big-endian, and no key. A bitmap of more
/// leaves than the root holds entries has an interior node on a new page
/// after each run of as many leaves (and after the last leaf), linked to its
/// siblings, with their entries, and the root has the last record number
/// and page of each of them
/// (`fixtures/write/bitmap_levels`, `fixtures/write/cdx_bitmap_levels`). New
/// pages come from `counter` (and one more is taken, as for other orders),
/// else from the file's end; pages refer to each other by page number times
/// `unit` (see [`build_tree`]).
fn build_bitmap(
    file: &mut std::fs::File,
    ps: usize,
    root: u32,
    slots: u32,
    deleted: &[u32],
    unit: u32,
    counter: Option<&mut u32>,
) -> Result<(), crate::order::Error> {
    use std::io::{Seek, SeekFrom};
    let capacity = ((ps - 16) * 8) as u32;
    let bits = slots + 1;
    let leaves = bits.div_ceil(capacity);
    let fanout = ((ps - 12) / 8) as u32;
    let groups = match leaves > fanout {
        true => leaves.div_ceil(fanout),
        false => 0,
    };
    if groups > fanout {
        return Err(crate::order::Error::Corrupt(
            "bitmap order needs a third interior level",
        ));
    }
    let pages = leaves + groups;
    let first = match counter {
        Some(next) => {
            let first = *next;
            *next += pages + 1;
            first
        }
        None => (file.seek(SeekFrom::End(0))? / ps as u64) as u32,
    };
    // Each leaf's page: an interior node follows every `fanout` leaves.
    let leaf_page = |i: u32| first + i + if groups == 0 { 0 } else { i / fanout };
    let interior_page = |g: u32| first + ((g + 1) * fanout).min(leaves) + g;
    let none = u32::MAX;
    let entry = |node: &mut [u8], n: u32, last: u32, page: u32| {
        let at = 12 + 8 * n as usize;
        node[at..at + 4].copy_from_slice(&last.to_be_bytes());
        node[at + 4..at + 8].copy_from_slice(&(page * unit).to_be_bytes());
    };
    let mut top = vec![0; ps];
    top[0..2].copy_from_slice(&1u16.to_le_bytes());
    top[4..12].fill(0xFF);
    if groups == 0 {
        top[2..4].copy_from_slice(&(leaves as u16).to_le_bytes());
    } else {
        top[2..4].copy_from_slice(&(groups as u16).to_le_bytes());
    }
    for g in 0..groups {
        let start = g * fanout;
        let end = (start + fanout).min(leaves);
        let mut node = vec![0; ps];
        node[2..4].copy_from_slice(&((end - start) as u16).to_le_bytes());
        let previous = if g == 0 {
            none
        } else {
            interior_page(g - 1) * unit
        };
        let next = if g + 1 == groups {
            none
        } else {
            interior_page(g + 1) * unit
        };
        node[4..8].copy_from_slice(&previous.to_le_bytes());
        node[8..12].copy_from_slice(&next.to_le_bytes());
        for i in start..end {
            entry(&mut node, i - start, (i + 1) * capacity - 1, leaf_page(i));
        }
        file.seek(SeekFrom::Start(u64::from(interior_page(g)) * ps as u64))?;
        file.write_all(&node)?;
        entry(&mut top, g, end * capacity - 1, interior_page(g));
    }
    for i in 0..leaves {
        let page = leaf_page(i);
        let start = i * capacity;
        let count = (bits - start).min(capacity);
        let mut leaf = vec![0; ps];
        leaf[0..2].copy_from_slice(&2u16.to_le_bytes());
        leaf[2..4].copy_from_slice(&(count as u16).to_le_bytes());
        let previous = if i == 0 {
            none
        } else {
            leaf_page(i - 1) * unit
        };
        let next = if i + 1 == leaves {
            none
        } else {
            leaf_page(i + 1) * unit
        };
        leaf[4..8].copy_from_slice(&previous.to_le_bytes());
        leaf[8..12].copy_from_slice(&next.to_le_bytes());
        leaf[12..16].copy_from_slice(&start.to_be_bytes());
        for &record in deleted.iter().filter(|&&r| r >= start && r < start + count) {
            let bit = (record - start) as usize;
            leaf[16 + bit / 8] |= 1 << (bit % 8);
        }
        file.seek(SeekFrom::Start(u64::from(page) * ps as u64))?;
        file.write_all(&leaf)?;
        if groups == 0 {
            entry(&mut top, i, (i + 1) * capacity - 1, page);
        }
    }
    file.seek(SeekFrom::Start(u64::from(root) * ps as u64))?;
    file.write_all(&top)?;
    Ok(())
}

/// Empties the tree of the order `name` of the ADI file at `path` as the
/// original engine leaves an order it failed to build (duplicate keys in a
/// unique order): its root an empty leaf without a layout.
pub fn clear_order(path: &Path, name: &str) -> Result<(), crate::order::Error> {
    use crate::order::Error as OrderError;
    use std::io::{Read, Seek, SeekFrom};
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let directory = Directory::adi(&mut file)?;
    let ps = directory.ps;
    let tag = directory
        .find(name)
        .ok_or(OrderError::Corrupt("no such order"))?
        .record;
    let mut root = [0; 4];
    file.seek(SeekFrom::Start(u64::from(tag) * ps as u64))?;
    file.read_exact(&mut root)?;
    let mut leaf = vec![0; ps];
    leaf[0..2].copy_from_slice(&3u16.to_le_bytes());
    leaf[4..12].fill(0xFF);
    leaf[12..14].copy_from_slice(&((ps - 24) as u16).to_le_bytes());
    file.seek(SeekFrom::Start(
        u64::from(u32::from_le_bytes(root)) * ps as u64,
    ))?;
    file.write_all(&leaf)?;
    Ok(())
}

/// The CRC-32 the original engine keeps in an ADI file's header for its
/// collation: of the collation's byte-to-weight table, or of the bytes 0
/// to 255 for none (found black-box: it is the standard CRC-32 of the
/// tables in `ansi.chr`).
pub fn collation_crc(weights: Option<&[u8; 256]>) -> u32 {
    let identity: [u8; 256] = std::array::from_fn(|i| i as u8);
    crc32(weights.unwrap_or(&identity))
}

/// The standard CRC-32 (IEEE 802.3) of `bytes`.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Removes the order `name` from the ADI file at `path`: from its tag
/// directory (its pages stay as they are, as the original engine leaves
/// them). Returns `false` if the file has no such order.
pub fn drop_order(path: &Path, name: &str) -> Result<bool, crate::order::Error> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut directory = Directory::adi(&mut file)?;
    let wanted = directory.key(name);
    let Some(at) = directory.names.iter().position(|key| key.bytes == wanted) else {
        return Ok(false);
    };
    directory.remove(&mut file, at)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, field_type: FieldType, length: u16, decimals: u16) -> FieldSpec {
        FieldSpec {
            name: name.into(),
            field_type,
            length,
            decimals,
            compressed: false,
        }
    }

    /// The tables of one field each that `fixtures/write/ddl` has the
    /// original engine create, and their memo files.
    #[test]
    fn new_tables_are_the_original_engines() {
        use FieldType::*;
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/write/ddl/after");
        let cases = [
            ("f_char", Character, 10, 0),
            ("f_cichar", CiCharacter, 10, 0),
            ("f_varchar", VarChar, 20, 0),
            ("f_nchar", NChar, 5, 0),
            ("f_nvarchar", NVarChar, 10, 0),
            ("f_raw", Raw, 4, 0),
            ("f_varbinary", VarBinary, 8, 0),
            ("f_guid", Guid, 16, 0),
            ("f_logical", Logical, 1, 0),
            ("f_numeric", Numeric, 10, 2),
            ("f_short", ShortInt, 2, 0),
            ("f_longint", LongInt, 8, 0),
            ("f_double", Double, 8, 15),
            ("f_curdouble", CurDouble, 8, 2),
            ("f_money", Money, 8, 4),
            ("f_date", Date, 4, 0),
            ("f_time", Time, 4, 0),
            ("f_timestamp", TimeStamp, 8, 0),
            ("f_autoinc", AutoInc, 4, 0),
            ("f_rowversion", RowVersion, 8, 0),
            ("f_modtime", ModTime, 8, 0),
            ("f_blob", Binary, 9, 0),
            ("f_nmemo", NMemo, 9, 0),
        ];
        for (name, field_type, length, decimals) in cases {
            let original = std::fs::read(dir.join(format!("{name}.adt"))).unwrap();
            let fields = [spec("a", field_type, length, decimals)];
            assert_eq!(adt_bytes(&fields).unwrap(), original, "{name}");
        }
        let compressed = [FieldSpec {
            compressed: true,
            ..spec("a", Memo, 9, 0)
        }];
        assert_eq!(
            adt_bytes(&compressed).unwrap(),
            std::fs::read(dir.join("f_memoc.adt")).unwrap()
        );
        for (name, block_size) in [
            ("f_memo8", 8),
            ("f_memo64", 64),
            ("f_memo1024", 1024),
            ("f_blob", 256),
        ] {
            let original = std::fs::read(dir.join(format!("{name}.adm"))).unwrap();
            assert_eq!(adm_bytes(block_size), original, "{name}");
        }
    }

    /// A new AES table's header, crypto block and enciphered descriptors are the
    /// original engine's, byte for byte: built from the field specs with the
    /// crypto material read from the fixture, they reproduce everything up to the
    /// records (which the fixture has and an empty table does not).
    #[test]
    fn aes_tables_are_the_original_engines() {
        use FieldType::*;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/aes");
        type Case<'a> = (&'a str, &'a str, &'a [u8], &'a [(&'a str, FieldType, u16)]);
        let cases: [Case; 2] = [
            (
                "aes128/s.adt",
                ".\\aes128.add",
                b"another pw",
                &[
                    ("id", Integer, 0),
                    ("name", Character, 12),
                    ("note", Memo, 0),
                ],
            ),
            (
                "aes256/big.adt",
                ".\\aes256.add",
                b"ddsecret",
                &[
                    ("id", Integer, 0),
                    ("word", Character, 10),
                    ("n", Integer, 0),
                ],
            ),
        ];
        for (file, dict_path, password, fields) in cases {
            let original = std::fs::read(dir.join(file)).unwrap();
            let material = AesMaterial {
                enc_type: original[0x4C],
                salt: original[0x1A0..0x1C0].try_into().unwrap(),
                nonce: original[0x194..0x198].try_into().unwrap(),
                descriptor_iv: original[0x1C0..0x1C8].try_into().unwrap(),
                iv_counter: u64::from_le_bytes(original[0x198..0x1A0].try_into().unwrap()),
            };
            let dict_object = u32::from_le_bytes(original[0x160..0x164].try_into().unwrap());
            let specs: Vec<FieldSpec> = fields.iter().map(|&(n, t, l)| spec(n, t, l, 0)).collect();
            let built =
                aes_adt_bytes(&specs, password, &material, Some((dict_path, dict_object))).unwrap();
            let data_offset = u32::from_le_bytes(original[0x20..0x24].try_into().unwrap()) as usize;
            let mut head = original[..data_offset].to_vec();
            head[0x18..0x1C].copy_from_slice(&[0, 0, 0, 0]); // the empty table has no records
            assert_eq!(built, head, "{file}");
        }
    }

    /// A free (non-dictionary) AES table, as the engine writes one via
    /// `AdsEnableEncryption` + `AdsEncryptTable` on a connection with an AES
    /// encryption type: the AES markers and crypto block, the verifier from the
    /// enable password, but descriptors left clear and no dictionary binding.
    /// Byte-for-byte against the engine's own free table
    /// (`fixtures/aes/free/free.adt`, password "mypass").
    #[test]
    fn a_free_aes_table_is_the_original_engines() {
        use FieldType::*;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/aes/free/free.adt");
        let original = std::fs::read(path).unwrap();
        let material = AesMaterial {
            enc_type: original[0x4C],
            salt: original[0x1A0..0x1C0].try_into().unwrap(),
            nonce: original[0x194..0x198].try_into().unwrap(),
            descriptor_iv: original[0x1C0..0x1C8].try_into().unwrap(),
            iv_counter: u64::from_le_bytes(original[0x198..0x1A0].try_into().unwrap()),
        };
        let specs = [spec("id", Integer, 0, 0), spec("name", Character, 12, 0)];
        let built = aes_adt_bytes(&specs, b"mypass", &material, None).unwrap();
        let data_offset = u32::from_le_bytes(original[0x20..0x24].try_into().unwrap()) as usize;
        let mut head = original[..data_offset].to_vec();
        head[0x18..0x1C].copy_from_slice(&[0, 0, 0, 0]);
        assert_eq!(built, head);
    }

    /// A table created on an AES connection but not yet armed, as the engine
    /// lays it out: the full AES structure (data past the 600-byte crypto
    /// extension, the 0x14/0x164 markers, fields at offset 13) with its crypto
    /// block, verifier and type all zero and only CREATED (plus MEMO) set.
    /// Deterministic, so byte-for-byte against `fixtures/aes/free/unarmed.adt`.
    #[test]
    fn an_unarmed_aes_table_is_the_original_engines() {
        use FieldType::*;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/aes/free/unarmed.adt");
        let original = std::fs::read(path).unwrap();
        let specs = [
            spec("id", Integer, 0, 0),
            spec("name", Character, 12, 0),
            spec("note", Memo, 0, 0),
        ];
        let built = aes_adt_unarmed_bytes(&specs).unwrap();
        assert_eq!(built, original);
    }

    /// Enciphering a record reproduces the engine's stored AES record: deciphered
    /// to its clear form and re-enciphered with the same IV, each record of the
    /// `aes128/s.adt` fixture comes back byte for byte.
    #[test]
    fn aes_records_encipher_as_the_original_engine() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/aes/aes128/s.adt");
        let d = std::fs::read(path).unwrap();
        let salt: [u8; 32] = d[0x1A0..0x1C0].try_into().unwrap();
        let nonce: [u8; 4] = d[0x194..0x198].try_into().unwrap();
        let derived = crate::crypto::aes_derive(b"another pw", &salt, 16);
        let key = &derived[..16];
        let data_offset = u32::from_le_bytes(d[0x20..0x24].try_into().unwrap()) as usize;
        let rec_len = u32::from_le_bytes(d[0x24..0x28].try_into().unwrap()) as usize;
        for i in 0..2 {
            let off = data_offset + i * rec_len;
            let stored = &d[off..off + rec_len];
            let iv: [u8; 8] = stored[5..13].try_into().unwrap();
            // The clear form the reader presents: status 0x04, IV slot zeroed,
            // field data deciphered.
            let mut clear = stored.to_vec();
            crate::crypto::aes_ctr_xor(key, &iv, &nonce, &mut clear[13..]);
            clear[0] = adt::ACTIVE;
            clear[5..13].fill(0);
            let mut built = clear;
            adt::aes_encipher_record(key, &nonce, &iv, &mut built);
            assert_eq!(built, stored, "record {i}");
        }
    }

    /// Index files of orders created on empty tables, as `fixtures/write/ddl`
    /// has the original engine create them.
    #[test]
    fn new_index_files_are_the_original_engines() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/write/ddl/after");
        let work = crate::testing::temp_root().join(format!("tflip-create-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        let order = |name: &str, expression: &str, key_length, pad, options| OrderSpec {
            name: name.into(),
            expression: expression.into(),
            condition: String::new(),
            key_length,
            options,
            flags: 0,
            pad,
            descending: false,
            collation: String::new(),
            locale: String::new(),
            uchar_width: 0,
            fts: None,
        };
        let crc = 0x2905_8c73;
        let cases: [(&str, u16, Vec<OrderSpec>); 4] = [
            (
                "ix8",
                8192,
                vec![
                    order("id8", "F1", 8, 0, 0),
                    order("name8", "F2", 100, b' ', 0),
                ],
            ),
            ("single", 512, vec![order("single", "F1", 8, 0, 0)]),
            (
                "other",
                1024,
                vec![
                    order("o_id", "F1", 8, 0, 0),
                    order("o_name", "F2", 12, b' ', 0),
                ],
            ),
            ("f_pk", 512, vec![order("PK_F_PK", "F1", 8, 0, 1)]),
        ];
        for (name, page_size, orders) in cases {
            let path = work.join(format!("{name}.adi"));
            let _ = std::fs::remove_file(&path);
            for order in &orders {
                assert!(add_order(&path, order, page_size, crc).unwrap());
            }
            let original = std::fs::read(dir.join(format!("{name}.adi"))).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), original, "{name}");
        }
        std::fs::remove_dir_all(&work).unwrap();
    }

    /// A tree whose second level fills up as the levels end (a key of 48 bytes
    /// in 512-byte pages holds 8 entries): `CREATE INDEX` on 131072 records
    /// of `fixtures/events/events_online.json` once panicked here.
    #[test]
    fn a_full_second_level_is_written_first() {
        let work = crate::testing::temp_root().join(format!("tflip-deep-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        let path = work.join("deep.adi");
        let _ = std::fs::remove_file(&path);
        let spec = OrderSpec {
            name: "DEEP".into(),
            expression: "F1".into(),
            condition: String::new(),
            key_length: 48,
            options: 0,
            flags: 0,
            pad: b' ',
            descending: false,
            collation: String::new(),
            locale: String::new(),
            uchar_width: 0,
            fts: None,
        };
        assert!(add_order(&path, &spec, 512, 0).unwrap());
        let keys: Vec<crate::order::Key> = (0..65536u32)
            .map(|record| {
                let mut bytes = vec![b' '; 48];
                bytes[..8].copy_from_slice(format!("{record:08}").as_bytes());
                crate::order::Key {
                    bytes,
                    record: record + 1,
                }
            })
            .collect();
        build_order(&path, "DEEP", &keys, 65536, &[]).unwrap();
        // Every key is there, in order, and the walk back ends where it began.
        let mut file = crate::order::IndexFile::open(&path).unwrap();
        let tag = file.tags("").pop().unwrap();
        let mut cursor = crate::order::Cursor::new(&tag, &file);
        let mut seen = 0;
        let mut next = cursor.first(&mut file).unwrap().map(|k| k.record);
        while let Some(record) = next {
            seen += 1;
            assert_eq!(record, seen);
            next = cursor.next(&mut file).unwrap().map(|k| k.record);
        }
        assert_eq!(seen, 65536);
        std::fs::remove_dir_all(&work).unwrap();
    }
}
