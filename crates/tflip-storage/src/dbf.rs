//! DBF tables (`.dbf`): the xBase tables of the Clipper (NTX), FoxPro (CDX)
//! and Visual FoxPro (VFP) table types, with their memo files (`.dbt`,
//! `.fpt`).
//!
//! A DBF file is a 32-byte header, one 32-byte descriptor per field ended by
//! 0x0D (and in a VFP table a 263-byte backlink), the records, and 0x1A. A
//! record starts with its deletion flag: `*` for a deleted record, a space
//! for an active one. Integers are little-endian unless noted.
//!
//! The structure is read into the same [`Structure`] as an ADT table's, with
//! each field's [`Layout`] saying how its bytes are stored. Besides the
//! dBase types, the original engine stores its own (Double, Integer,
//! LongInt, GUID, VarChar, the Unicode types, binary and image memos) in
//! Clipper and FoxPro tables. Those have no NULL: a blank field (spaces, or
//! zeros for the binary types) reads as NULL. VFP tables have real NULLs,
//! marked in the hidden system field `_NullFlags`, and their blank values
//! read as themselves ([`Value::Blank`] for dates, timestamps and logicals).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cp1252;
use crate::field::{Field, FieldType, MemoRef, utf16};
use crate::memo::{MemoFile, MemoKind};
use crate::table::{Error, Format, Header, RecordStatus, Records, Structure, TableFile, flags};
use crate::value::{Value, Ymd, ymd_to_julian};

/// The flavour of a DBF table: the ACE table type it is opened as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Flavour {
    /// Clipper, with `.ntx` indexes and a `.dbt` memo file (`ADS_NTX`).
    Clipper,
    /// FoxPro, with `.cdx` indexes and an `.fpt` memo file (`ADS_CDX`).
    FoxPro,
    /// Visual FoxPro (`ADS_VFP`).
    Visual,
}

/// The size of the header and of a field descriptor.
const DESCRIPTOR_SIZE: usize = 32;

/// VFP field flags (descriptor byte 18).
mod field_flags {
    /// A system field (`_NullFlags`).
    pub const SYSTEM: u8 = 0x01;
    /// The field can hold NULL.
    pub const NULLABLE: u8 = 0x02;
    /// Binary: no code page translation.
    pub const BINARY: u8 = 0x04;
    /// AutoInc (both bits).
    pub const AUTOINC: u8 = 0x0C;
}

/// The SIx tags a Clipper or FoxPro Binary or Image value starts with: 8
/// bytes of signature, 00 1A 0B 0C, and the data's length.
const BINARY_TAGS: [&[u8; 8]; 2] = [b"@R_BLOB@", b"@SIXPIC@"];

/// One bit of a VFP record's `_NullFlags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bit {
    /// The byte in the record.
    pub offset: u16,
    pub mask: u8,
}

impl Bit {
    pub fn is_set(self, record: &[u8]) -> bool {
        record
            .get(usize::from(self.offset))
            .is_some_and(|&b| b & self.mask != 0)
    }

    pub fn set(self, record: &mut [u8], on: bool) {
        if let Some(byte) = record.get_mut(usize::from(self.offset)) {
            match on {
                true => *byte |= self.mask,
                false => *byte &= !self.mask,
            }
        }
    }
}

/// How a DBF field is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    /// The type letter (or code, for the Advantage types) of the descriptor.
    pub letter: u8,
    pub flavour: Flavour,
    /// In a VFP table, the `_NullFlags` bit set when the field is NULL.
    pub null_bit: Option<Bit>,
    /// In a VFP table, the `_NullFlags` bit set when a VarChar or VarBinary
    /// value is shorter than the field (its length is then in the last byte).
    pub length_bit: Option<Bit>,
    /// The VFP field flags (descriptor byte 18); 0 in other tables.
    pub flags: u8,
}

impl Layout {
    fn vfp(&self) -> bool {
        self.flavour == Flavour::Visual
    }

    /// Whether the field is a VFP table's.
    pub(crate) fn vfp_table(&self) -> bool {
        self.vfp()
    }

    /// Whether a VFP field is flagged binary: its text is not translated
    /// between code pages (the binary types, and `NOCPTRANS` fields).
    pub fn binary(&self) -> bool {
        self.vfp() && self.flags & field_flags::BINARY != 0
    }

    /// Whether the field is empty in `record`, as the original engine's
    /// `AdsIsEmpty` says: its bytes are blank (spaces in the text types,
    /// zeros in the binary types, no memo, a length of 0), whatever its
    /// type's NULL is, but a VFP field that is NULL is not empty.
    pub(crate) fn is_empty(&self, field: &Field, record: &[u8]) -> bool {
        if self.vfp() && self.is_null(field, record) {
            return false;
        }
        let bytes = field.bytes(record);
        let spaces = || bytes.iter().all(|&b| b == b' ');
        let zero = || bytes.iter().all(|&b| b == 0);
        match field.field_type {
            FieldType::Character | FieldType::Numeric | FieldType::Date | FieldType::Logical => {
                spaces()
            }
            FieldType::Memo | FieldType::Binary | FieldType::Image | FieldType::NMemo => {
                self.memo_block(field, record).is_none()
            }
            FieldType::NChar => {
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .all(|unit| *unit == [0x20, 0])
                    || bytes.starts_with(&[0, 0])
            }
            FieldType::NVarChar | FieldType::LegacyVarChar => {
                matches!(self.var_length(field, record), Some(0 | 0xFFFF))
            }
            FieldType::VarChar | FieldType::VarBinary => self.var_length(field, record) == Some(0),
            _ => zero(),
        }
    }

    /// The declared length: characters for NChar and NVarChar, else the
    /// bytes the field takes.
    pub(crate) fn length(&self, field: &Field) -> u16 {
        match field.field_type {
            FieldType::NChar => field.size / 2,
            FieldType::NVarChar => field.size.saturating_sub(2) / 2,
            _ => field.size,
        }
    }

    /// The first block of a memo field's value, if it has one.
    fn memo_block(&self, field: &Field, record: &[u8]) -> Option<u32> {
        let bytes = field.bytes(record);
        let block = match self.vfp() {
            true => u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?),
            false => {
                let digits = std::str::from_utf8(bytes).ok()?;
                digits.trim_matches([' ', '\0']).parse().ok()?
            }
        };
        (block != 0).then_some(block)
    }

    /// A memo field's reference: its first block (0 for none). The length is
    /// in the memo file, not the record, so it is 0 here.
    pub(crate) fn memo_ref(&self, field: &Field, record: &[u8]) -> MemoRef {
        MemoRef {
            block: self.memo_block(field, record).unwrap_or(0),
            stored: 0,
            length: 0,
        }
    }

    /// A memo field's value as stored (UTF-16LE for NMemo), a Binary or
    /// Image value without its SIx tag; empty if it has none.
    pub(crate) fn memo_data(
        &self,
        field: &Field,
        record: &[u8],
        memo: Option<&mut MemoFile>,
    ) -> Result<Vec<u8>, Error> {
        let Some(block) = self.memo_block(field, record) else {
            return Ok(Vec::new());
        };
        let memo = memo.ok_or(Error::Corrupt("memo field without a memo file"))?;
        let tagged =
            !self.vfp() && matches!(field.field_type, FieldType::Binary | FieldType::Image);
        let mut data = memo.read_dbf(block, tagged)?;
        if tagged && data.len() >= 16 && BINARY_TAGS.iter().any(|tag| data[..8] == tag[..]) {
            data.drain(..16);
        }
        Ok(data)
    }

    /// The value of a General or Picture field (an OLE object or a picture
    /// in the memo file, which the getters do not read) as the memo file
    /// keeps it; `None` if it has none.
    pub(crate) fn object_data(
        &self,
        field: &Field,
        record: &[u8],
        memo: Option<&mut MemoFile>,
    ) -> Result<Option<Vec<u8>>, Error> {
        let Some(block) = self.memo_block(field, record) else {
            return Ok(None);
        };
        let memo = memo.ok_or(Error::Corrupt("memo field without a memo file"))?;
        memo.read_dbf(block, false).map(Some)
    }

    /// The stored length of a VarChar or VarBinary (bytes) or NVarChar (code
    /// units) value; `None` for other fields.
    pub(crate) fn var_length(&self, field: &Field, record: &[u8]) -> Option<u16> {
        let bytes = field.bytes(record);
        let tail = || u16::from_le_bytes([bytes[bytes.len() - 2], bytes[bytes.len() - 1]]);
        match field.field_type {
            FieldType::LegacyVarChar if bytes.iter().all(|&b| b == b' ') => Some(0),
            FieldType::LegacyVarChar => Some(tail()),
            FieldType::NVarChar if self.vfp() && tail() == 0xFFFF => Some(0),
            FieldType::NVarChar => Some(tail()),
            FieldType::VarChar | FieldType::VarBinary => Some(
                match self.length_bit.is_some_and(|bit| bit.is_set(record)) {
                    true => u16::from(bytes[bytes.len() - 1]),
                    false => field.size,
                },
            ),
            _ => None,
        }
    }

    /// Whether the field holds NULL in `record`: in a VFP table, whether its
    /// NULL bit is set; else whether it is blank.
    pub(crate) fn is_null(&self, field: &Field, record: &[u8]) -> bool {
        if self.vfp() {
            return self.null_bit.is_some_and(|bit| bit.is_set(record));
        }
        let bytes = field.bytes(record);
        let spaces = || bytes.iter().all(|&b| b == b' ');
        let zero = || bytes.iter().all(|&b| b == 0);
        match field.field_type {
            FieldType::Character
            | FieldType::Numeric
            | FieldType::Date
            | FieldType::Logical
            | FieldType::Memo
            | FieldType::Binary
            | FieldType::Image => spaces() || self.memo_like_zero(field, bytes),
            FieldType::LegacyVarChar => self.var_length(field, record) == Some(0),
            FieldType::LongInt => bytes == i64::MIN.to_le_bytes(),
            FieldType::NVarChar => self.var_length(field, record) == Some(0xFFFF),
            FieldType::NChar => bytes.starts_with(&[0, 0]),
            FieldType::NMemo => false,
            _ => zero(),
        }
    }

    /// Whether a memo reference is all zero bytes (as NULL leaves an NMemo's;
    /// read as no reference for the other memo fields too).
    fn memo_like_zero(&self, field: &Field, bytes: &[u8]) -> bool {
        field.is_memo() && bytes.iter().all(|&b| b == 0)
    }

    /// The value the field holds in `record`; memo values are read from
    /// `memo`.
    pub(crate) fn value(
        &self,
        field: &Field,
        record: &[u8],
        memo: Option<&mut MemoFile>,
    ) -> Result<Value, Error> {
        // A VFP memo field set to NULL keeps its reference, and the original
        // engine's getters still read the value it refers to.
        let kept = self.vfp() && field.is_memo() && self.memo_block(field, record).is_some();
        if self.is_null(field, record) && !kept {
            return Ok(Value::Null);
        }
        self.decode(field, record, memo)
    }

    /// The value the field holds in `record` as expressions read it: in a
    /// Clipper or FoxPro table, a blank field is its blank value (spaces,
    /// 0, the empty date, false, an empty memo), not NULL; else as
    /// [`Layout::value`].
    pub(crate) fn expression_value(
        &self,
        field: &Field,
        record: &[u8],
        memo: Option<&mut MemoFile>,
    ) -> Result<Value, Error> {
        if self.vfp() && self.is_null(field, record) {
            return Ok(Value::Null);
        }
        if self.vfp() || !self.is_null(field, record) {
            return self.value(field, record, memo);
        }
        Ok(match field.field_type {
            FieldType::Character | FieldType::Numeric | FieldType::Date | FieldType::Logical => {
                self.decode(field, record, memo)?
            }
            FieldType::Memo | FieldType::NMemo | FieldType::LegacyVarChar => {
                Value::Text(String::new())
            }
            FieldType::Double => Value::Double(0.0),
            FieldType::Integer | FieldType::AutoInc => Value::Integer(0),
            _ => Value::Null,
        })
    }

    /// The value of the field's bytes in `record`, NULL or not.
    fn decode(
        &self,
        field: &Field,
        record: &[u8],
        memo: Option<&mut MemoFile>,
    ) -> Result<Value, Error> {
        let bytes = field.bytes(record);
        let i32_at = |offset| i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let i64_at = |offset| i64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let blank = bytes.iter().all(|&b| b == b' ');
        Ok(match field.field_type {
            FieldType::Character => Value::Text(cp1252::decode(bytes)),
            FieldType::Numeric => Value::Numeric(bytes.iter().map(|&b| char::from(b)).collect()),
            FieldType::Date if blank => Value::Blank,
            FieldType::Date => Value::Date(text_date(bytes).unwrap_or(0)),
            FieldType::Logical if blank => Value::Blank,
            FieldType::Logical => Value::Logical(matches!(bytes[0], b'T' | b't' | b'Y' | b'y')),
            FieldType::Memo | FieldType::NMemo | FieldType::Binary | FieldType::Image => {
                let data = self.memo_data(field, record, memo)?;
                match field.field_type {
                    FieldType::Memo => Value::Text(cp1252::decode(&data)),
                    FieldType::NMemo => Value::Text(utf16(&data)),
                    _ => Value::Binary(data),
                }
            }
            FieldType::Double => Value::Double(f64::from_bits(i64_at(0) as u64)),
            FieldType::Integer | FieldType::AutoInc => Value::Integer(i32_at(0).into()),
            FieldType::LongInt => Value::Integer(i64_at(0).into()),
            FieldType::Money => Value::Money(i64_at(0)),
            FieldType::TimeStamp if bytes.iter().all(|&b| b == 0) => Value::Blank,
            // VFP timestamps have whole seconds: the engine reads stored
            // milliseconds rounded to the nearest second (500 reads as 1000).
            FieldType::TimeStamp => Value::TimeStamp(
                i32_at(0),
                (i32_at(4) as u32).saturating_add(500) / 1000 * 1000,
            ),
            FieldType::CompactDate => {
                let packed = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
                let date = Ymd {
                    year: (packed >> 9) as i32,
                    month: (packed >> 5) & 0x0F,
                    day: packed & 0x1F,
                };
                Value::Date(ymd_to_julian(date).unwrap_or(0))
            }
            FieldType::Guid => Value::Guid(bytes.try_into().unwrap()),
            FieldType::NChar if bytes.starts_with(&[0, 0]) => Value::Text(String::new()),
            FieldType::NChar => Value::Text(utf16(bytes)),
            FieldType::NVarChar => {
                let units = usize::from(self.var_length(field, record).unwrap());
                Value::Text(utf16(&bytes[..(2 * units).min(bytes.len() - 2)]))
            }
            FieldType::VarChar | FieldType::VarBinary => {
                let length = usize::from(self.var_length(field, record).unwrap()).min(bytes.len());
                match field.field_type {
                    FieldType::VarChar => Value::Text(cp1252::decode(&bytes[..length])),
                    _ => Value::Binary(bytes[..length].to_vec()),
                }
            }
            FieldType::LegacyVarChar => {
                Value::Text(cp1252::decode(&self.legacy_var_char(field, record, memo)?))
            }
            _ => Value::Binary(bytes.to_vec()),
        })
    }

    /// The bytes of an Advantage VarChar in a Clipper or FoxPro table: a
    /// value of up to the field's size − 2 bytes is inline, followed by its
    /// u16 length; a longer one keeps its first size − 6 bytes inline, then
    /// the u32 memo block that holds the rest and the u16 total length.
    fn legacy_var_char(
        &self,
        field: &Field,
        record: &[u8],
        memo: Option<&mut MemoFile>,
    ) -> Result<Vec<u8>, Error> {
        let bytes = field.bytes(record);
        let size = bytes.len();
        let length = usize::from(self.var_length(field, record).unwrap_or(0));
        if length <= size - 2 {
            return Ok(bytes[..length].to_vec());
        }
        let inline = size - 6;
        let block = u32::from_le_bytes(bytes[inline..inline + 4].try_into().unwrap());
        let memo = memo.ok_or(Error::Corrupt("VarChar overflow without a memo file"))?;
        let mut value = bytes[..inline].to_vec();
        value.extend(memo.read_raw(block, length - inline)?);
        Ok(value)
    }

    /// Whether the field can hold NULL: in a VFP table, whether it has a
    /// NULL bit; the other tables store NULL as a blank value.
    pub fn nullable(&self) -> bool {
        !self.vfp() || self.null_bit.is_some()
    }

    /// Writes `value` into the field's bytes in `record` as the original
    /// engine stores it (see [`crate::adt::encode`] for the types stored
    /// alike): dates as `YYYYMMDD`, a VFP VarChar or VarBinary padded with
    /// spaces and its length in the last byte when it is shorter than the
    /// field. A value clears a VFP field's NULL bit; NULL is
    /// [`Layout::encode_null`]'s. An ADS VarChar's memo overflow is the
    /// caller's: only a value that fits inline is taken here.
    pub(crate) fn encode(
        &self,
        field: &Field,
        value: &Value,
        record: &mut [u8],
    ) -> Result<(), Error> {
        if *value == Value::Null {
            return self.encode_null(field, record);
        }
        let start = usize::from(field.offset);
        let slot = &mut record[start..start + usize::from(field.size)];
        let mismatch = || Error::Corrupt("value does not match the field type");
        match (field.field_type, value) {
            (FieldType::Date, Value::Date(0) | Value::Blank) => slot.fill(b' '),
            (FieldType::Date, Value::Date(day)) => {
                slot.copy_from_slice(&crate::value::date_text(*day));
            }
            (FieldType::CompactDate, Value::Date(day)) => {
                let date = crate::value::julian_to_ymd(*day);
                let packed = (date.year as u32) << 9 | date.month << 5 | date.day;
                slot.copy_from_slice(&packed.to_be_bytes()[1..]);
            }
            (FieldType::Logical, Value::Blank) => slot[0] = b' ',
            // An empty VFP TimeStamp: day and time 0.
            (FieldType::TimeStamp, Value::Blank) => slot.fill(0),
            // The data, padded (with spaces, zeros in a VarBinary) as far as
            // the old value went, and its length in the last byte if it is
            // shorter than the field.
            (FieldType::VarChar | FieldType::VarBinary, Value::Text(_) | Value::Binary(_)) => {
                let (data, pad) = match value {
                    Value::Text(text) => (cp1252::encode(text), b' '),
                    Value::Binary(data) => (data.clone(), 0),
                    _ => unreachable!(),
                };
                let size = slot.len();
                let old = match self.length_bit.is_some_and(|bit| bit.is_set(record)) {
                    _ if self.is_null(field, record) => 0,
                    true => usize::from(record[start + size - 1]).min(size),
                    false => size,
                };
                let slot = &mut record[start..start + size];
                let length = data.len().min(size);
                slot[..length].copy_from_slice(&data[..length]);
                if old > length {
                    slot[length..old].fill(pad);
                }
                if length < size {
                    slot[size - 1] = length as u8;
                }
                if let Some(bit) = self.length_bit {
                    bit.set(record, length < size);
                }
            }
            (FieldType::LegacyVarChar, Value::Text(text)) => {
                let data = cp1252::encode(text);
                let size = slot.len();
                if data.len() > size - 2 {
                    return Err(mismatch());
                }
                slot[..data.len()].copy_from_slice(&data);
                slot[data.len()..size - 2].fill(b' ');
                slot[size - 2..].copy_from_slice(&(data.len() as u16).to_le_bytes());
            }
            (FieldType::Memo | FieldType::NMemo | FieldType::Binary | FieldType::Image, _) => {
                return Err(mismatch());
            }
            _ => crate::adt::encode(field, value, slot)?,
        }
        if let Some(bit) = self.null_bit {
            bit.set(record, false);
        }
        Ok(())
    }

    /// Writes NULL into the field in `record` (`AdsSetNull`): in a Clipper or
    /// FoxPro table its blank bytes (spaces, zeros in the binary types, no
    /// memo reference); in a VFP table its NULL bit, the bytes kept. Either
    /// way a LongInt holds its minimum, a GUID zeros, an NChar a first code
    /// unit 0 (the rest kept), an NVarChar the length 0xFFFF (the text kept)
    /// and an NMemo an all-zero reference. A VFP field without a NULL bit
    /// cannot be NULL.
    pub(crate) fn encode_null(&self, field: &Field, record: &mut [u8]) -> Result<(), Error> {
        if !self.nullable() {
            return Err(Error::Corrupt("the field cannot be NULL"));
        }
        let start = usize::from(field.offset);
        let size = usize::from(field.size);
        let slot = &mut record[start..start + size];
        match field.field_type {
            FieldType::LongInt => slot.copy_from_slice(&i64::MIN.to_le_bytes()),
            FieldType::Guid | FieldType::NMemo => slot.fill(0),
            FieldType::NChar => slot[..2.min(size)].fill(0),
            FieldType::NVarChar => slot[size - 2..].fill(0xFF),
            _ if self.vfp() => {}
            _ => slot.copy_from_slice(&self.blank_bytes(field)),
        }
        if let Some(bit) = self.null_bit {
            bit.set(record, true);
        }
        Ok(())
    }

    /// Writes the bytes `AdsGetRecord` shows for the field into `record`: a
    /// VFP field that is NULL shows its blank bytes (an NChar with a first
    /// code unit 0), whatever the file keeps; but the types whose NULL is in
    /// their bytes (LongInt, GUID, NVarChar) show them.
    pub(crate) fn show(&self, field: &Field, record: &mut [u8]) {
        use FieldType::*;
        if !self.vfp() || !self.is_null(field, record) {
            return;
        }
        if matches!(field.field_type, LongInt | Guid | NVarChar) {
            return;
        }
        let start = usize::from(field.offset);
        let mut blank = self.blank_bytes(field);
        if field.field_type == FieldType::NChar {
            let first = 2.min(blank.len());
            blank[..first].fill(0);
        }
        record[start..start + blank.len()].copy_from_slice(&blank);
    }

    /// Clears the field's NULL bit in `record`, if it has one.
    pub(crate) fn set_not_null(&self, record: &mut [u8]) {
        if let Some(bit) = self.null_bit {
            bit.set(record, false);
        }
    }

    /// Writes the field's blank bytes into `record` (`AdsSetEmpty`): what an
    /// insert starts from, not NULL.
    pub(crate) fn encode_blank(&self, field: &Field, record: &mut [u8]) {
        let start = usize::from(field.offset);
        // An empty LongInt is 0, where an insert starts from the minimum; an
        // empty NVarChar keeps its text, with the length 0.
        let blank = match field.field_type {
            FieldType::LongInt => vec![0; usize::from(field.size)],
            FieldType::NVarChar => {
                let size = usize::from(field.size);
                let mut bytes = record[start..start + size].to_vec();
                bytes[size - 2..].fill(0);
                bytes
            }
            _ => self.blank_bytes(field),
        };
        record[start..start + blank.len()].copy_from_slice(&blank);
        if let Some(bit) = self.length_bit {
            bit.set(record, true);
        }
        if let Some(bit) = self.null_bit {
            bit.set(record, false);
        }
    }

    /// The bytes of the field in the record an insert starts from: spaces,
    /// zeros for the binary types, the minimum in a Clipper or FoxPro
    /// LongInt, and U+0020 in NChar and NVarChar (with the length 0xFFFF).
    /// A VFP VarChar or VarBinary also has its length bit set (see
    /// [`blank_record`]).
    pub(crate) fn blank_bytes(&self, field: &Field) -> Vec<u8> {
        let size = usize::from(field.size);
        let vfp = self.vfp();
        match field.field_type {
            FieldType::Double
            | FieldType::Integer
            | FieldType::AutoInc
            | FieldType::CompactDate
            | FieldType::Guid
            | FieldType::Money
            | FieldType::TimeStamp => vec![0; size],
            FieldType::LongInt if vfp => vec![0; size],
            FieldType::LongInt => i64::MIN.to_le_bytes().to_vec(),
            // A VFP memo reference, a General's and a Picture's too.
            FieldType::Memo
            | FieldType::Binary
            | FieldType::Image
            | FieldType::NMemo
            | FieldType::Other(_)
                if vfp =>
            {
                vec![0; size]
            }
            FieldType::NChar | FieldType::NVarChar => {
                let mut bytes: Vec<u8> = [0x20, 0].repeat(size / 2);
                if field.field_type == FieldType::NVarChar {
                    bytes[size - 2..].fill(0xFF);
                }
                bytes
            }
            FieldType::VarChar | FieldType::VarBinary => {
                let mut bytes = vec![b' '; size];
                bytes[size - 1] = 0;
                bytes
            }
            _ => vec![b' '; size],
        }
    }
}

/// Fills in the parts of a blank record that are not a field's own bytes:
/// the deletion flag, and in a VFP table the length bits of VarChar and
/// VarBinary fields.
pub(crate) fn blank_record(structure: &Structure, record: &mut [u8]) {
    record[0] = b' ';
    for field in &structure.fields {
        if let crate::field::Layout::Dbf(Layout {
            length_bit: Some(bit),
            ..
        }) = field.layout
        {
            record[usize::from(bit.offset)] |= bit.mask;
        }
    }
}

/// The Julian day of a date stored as `YYYYMMDD`.
fn text_date(bytes: &[u8]) -> Option<i32> {
    let text = std::str::from_utf8(bytes).ok()?;
    let number = |range: std::ops::Range<usize>| text.get(range)?.trim().parse::<u32>().ok();
    let year = number(0..4)? as i32;
    ymd_to_julian(Ymd {
        year,
        month: number(4..6)?,
        day: number(6..8)?,
    })
}

/// What the header of a DBF table says besides its structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// The version byte.
    pub version: u8,
    /// The date of the last update: years since 1900, month and day.
    pub updated: [u8; 3],
    /// The table flags (byte 0x1C): 0x01 a structural `.cdx` exists, 0x02
    /// (VFP) the table has a memo file.
    pub table_flags: u8,
    /// The code page byte (0x1D): 0x03 for Windows ANSI, 0x00 for an OEM
    /// Clipper or FoxPro table.
    pub code_page: u8,
    /// The kind of the table's memo file, if it has one.
    pub memo: Option<MemoKind>,
    /// Bytes 0x0C to 0x15 of the header, which an RC4 table keeps its password
    /// check in (see [`crate::crypto::dbf_password_check`]) and are zero in
    /// any other.
    pub password_check: [u8; 10],
    /// Whether the table is laid out for AES (a VFP table of an AES
    /// dictionary or made on an AES connection): each record ends with an 8-byte
    /// IV inside its `_NullFlags` field, and the memo values are stored behind
    /// one (`fixtures/aes/rc4tp`, `v1` to `v11`).
    pub aes_layout: bool,
}

/// The version byte of a VFP table laid out for AES that is not encrypted
/// (one that is has [`VFP_ENCRYPTED`]).
pub(crate) const AES_LAYOUT: u8 = 0xF9;

/// The version byte of an encrypted VFP table, RC4 or AES.
pub(crate) const VFP_ENCRYPTED: u8 = 0xF8;

/// The size of the back link a VFP header ends with, which an AES table keeps
/// its crypto block in.
pub(crate) const BACKLINK: usize = 263;

/// The encryption types of an AES table (header byte 0x0C): the same numbers
/// an ADT table's header gives.
pub(crate) fn aes_type(check: u8) -> bool {
    matches!(check, 5 | 6)
}

/// Reads the structure of a DBF table from the start of `reader`, as the
/// table type `flavour`.
pub fn read_structure(
    reader: &mut impl Read,
    flavour: Flavour,
) -> Result<(Structure, Info), Error> {
    let mut head = [0; DESCRIPTOR_SIZE];
    reader.read_exact(&mut head)?;
    let version = head[0];
    let record_count = u32::from_le_bytes(head[4..8].try_into().unwrap());
    let header_length = u16::from_le_bytes([head[8], head[9]]);
    let record_length = u16::from_le_bytes([head[10], head[11]]);
    let encrypted = matches!(version, 0x07 | 0x87 | 0xF7 | 0xF8);
    let vfp_file = matches!(version, 0x30..=0x32 | AES_LAYOUT | 0xF8);
    let memo = match version {
        0x83 | 0x87 => Some(MemoKind::Dbt),
        0xF5 | 0xF7 => Some(MemoKind::Fpt),
        _ if vfp_file && head[0x1C] & 0x02 != 0 => Some(MemoKind::Fpt),
        _ => None,
    };
    if !matches!(
        version,
        0x03 | 0x07 | 0x83 | 0x87 | 0xF5 | 0xF7 | 0x30..=0x32 | AES_LAYOUT | 0xF8
    ) {
        return Err(Error::Corrupt("not a DBF table"));
    }
    let reads = match flavour {
        Flavour::Clipper => MemoKind::Dbt,
        Flavour::FoxPro | Flavour::Visual => MemoKind::Fpt,
    };
    if let Some(kind) = memo.filter(|&kind| kind != reads) {
        return Err(Error::WrongMemoFile(match kind {
            MemoKind::Dbt => Flavour::Clipper,
            _ if vfp_file => Flavour::Visual,
            _ => Flavour::FoxPro,
        }));
    }
    if usize::from(header_length) < DESCRIPTOR_SIZE + 1 || record_length == 0 {
        return Err(Error::Corrupt("DBF header too short"));
    }
    let mut descriptors = vec![0; usize::from(header_length) - DESCRIPTOR_SIZE];
    reader
        .read_exact(&mut descriptors)
        .map_err(|_| Error::Corrupt("file ends inside the field descriptors"))?;

    let mut raw = Vec::new();
    for descriptor in descriptors.as_chunks::<DESCRIPTOR_SIZE>().0 {
        if descriptor[0] == 0x0D {
            break;
        }
        raw.push(descriptor);
    }
    if descriptors.get(raw.len() * DESCRIPTOR_SIZE) != Some(&0x0D) {
        return Err(Error::Corrupt("field descriptors not terminated"));
    }

    // The _NullFlags system field, which VFP tables end with.
    let mut offset = 1u16;
    let mut null_flags = None;
    let mut sized = Vec::with_capacity(raw.len());
    for descriptor in &raw {
        let letter = descriptor[11];
        let wide = matches!(letter, b'C' | b'V' | b'Q' | 0x1A | 0x1B);
        let size = match wide {
            true => u16::from_le_bytes([descriptor[16], descriptor[17]]),
            false => u16::from(descriptor[16]),
        };
        if flavour == Flavour::Visual && letter == b'0' && descriptor[18] & field_flags::SYSTEM != 0
        {
            null_flags = Some(offset);
        }
        sized.push((descriptor, offset, size));
        offset = offset
            .checked_add(size)
            .ok_or(Error::Corrupt("fields too long"))?;
    }
    if offset > record_length {
        return Err(Error::Corrupt("fields extend past the record length"));
    }

    let mut fields = Vec::with_capacity(sized.len());
    let mut next_bit = 0u16;
    let mut take_bit = || {
        let bit = next_bit;
        next_bit += 1;
        null_flags.map(|start| Bit {
            offset: start + bit / 8,
            mask: 1 << (bit % 8),
        })
    };
    for (descriptor, offset, size) in sized {
        let letter = descriptor[11];
        let field_flags = descriptor[18];
        if flavour == Flavour::Visual && letter == b'0' && field_flags & field_flags::SYSTEM != 0 {
            continue;
        }
        let vfp = flavour == Flavour::Visual;
        let field_type = match letter {
            b'C' => FieldType::Character,
            b'N' | b'F' => FieldType::Numeric,
            b'D' => FieldType::Date,
            b'L' => FieldType::Logical,
            b'M' if !vfp && descriptor[19] == b'B' => FieldType::Binary,
            b'M' if !vfp && descriptor[19] == b'I' => FieldType::Image,
            b'M' => FieldType::Memo,
            b'B' => FieldType::Double,
            b'I' if vfp && field_flags & field_flags::AUTOINC == field_flags::AUTOINC => {
                FieldType::AutoInc
            }
            b'I' => FieldType::Integer,
            b'V' if vfp => FieldType::VarChar,
            b'V' if size == 3 => FieldType::CompactDate,
            b'V' => FieldType::LegacyVarChar,
            b'Q' => FieldType::VarBinary,
            b'W' => FieldType::Binary,
            b'Y' => FieldType::Money,
            b'T' => FieldType::TimeStamp,
            code @ (0x13 | 0x1A..=0x1D) => FieldType::from_code(code.into()),
            other => FieldType::Other(other.into()),
        };
        let length_bit = match field_type {
            FieldType::VarChar | FieldType::VarBinary if vfp => take_bit(),
            _ => None,
        };
        let null_bit = match vfp && field_flags & field_flags::NULLABLE != 0 {
            true => take_bit(),
            false => None,
        };
        let decimals = match letter {
            b'N' | b'F' | b'B' => u16::from(descriptor[17]),
            _ => 0,
        };
        let name = &descriptor[..11];
        let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
        fields.push(Field {
            name: cp1252::decode(name),
            field_type,
            offset,
            size,
            decimals_slot: decimals,
            layout: crate::field::Layout::Dbf(Layout {
                letter,
                flavour,
                null_bit,
                length_bit,
                flags: if vfp { field_flags } else { 0 },
            }),
        });
    }

    let header = Header {
        format: Format::Dbf(flavour),
        record_slots: record_count,
        data_offset: u32::from(header_length),
        record_length: u32::from(record_length),
        free_head: 0,
        free_count: 0,
        encryption_type: 0,
        flags: if memo.is_some() { flags::MEMO } else { 0 }
            | if encrypted { flags::ENCRYPTED } else { 0 }
            | if head[0x1E] & 0x02 != 0 {
                flags::TRANSACTION_FREE
            } else {
                0
            },
        field_count: fields.len() as u16,
        extension: 0,
        dictionary: false,
    };
    let info = Info {
        version,
        updated: [head[1], head[2], head[3]],
        table_flags: head[0x1C],
        code_page: head[0x1D],
        memo,
        password_check: head[0x0C..0x16].try_into().unwrap(),
        aes_layout: version == AES_LAYOUT || (version == VFP_ENCRYPTED && aes_type(head[0x0C])),
    };
    Ok((Structure { header, fields }, info))
}

/// The memo file belonging to the table at `table`: the same name with the
/// extension of `kind` (in upper case if the table's is).
pub fn memo_path(table: &Path, kind: MemoKind) -> PathBuf {
    let upper = table
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e == "DBF");
    let extension = match kind {
        MemoKind::Dbt => "dbt",
        _ => "fpt",
    };
    table.with_extension(if upper {
        extension.to_ascii_uppercase()
    } else {
        extension.to_owned()
    })
}

/// Where the header keeps the number of records.
pub(crate) const RECORD_COUNT: u64 = 4;

/// The byte after the last record.
pub(crate) const END_OF_FILE: u8 = 0x1A;

/// The deletion flag of a deleted record.
pub(crate) const DELETED: u8 = b'*';

/// The first byte of an enciphered record that is live and of one that is
/// deleted (`fixtures/aes/rc4tp`, `y2`, `y6`).
pub(crate) const ENCRYPTED_ACTIVE: u8 = b'E';
pub(crate) const ENCRYPTED_DELETED: u8 = b'D';

/// The version byte of the table once it is encrypted (or the one it had when
/// it was, going the other way).
pub(crate) fn version_encrypted(version: u8, on: bool) -> u8 {
    match (version, on) {
        (0x03 | 0x83, true) => version + 4,
        (0xF5, true) => 0xF7,
        (0x07 | 0x87, false) => version - 4,
        (0xF7, false) => 0xF5,
        (0x30..=0x32, true) => 0xF8,
        (0xF8, false) => 0x30,
        _ => version,
    }
}

/// Where the descriptor of the field `index` (0-based) starts.
pub(crate) fn descriptor(index: usize) -> u64 {
    (DESCRIPTOR_SIZE * (index + 1)) as u64
}

/// The status stored in a record's first byte: `*` (or `D` when encrypted)
/// marks a deleted record and anything else an active one. A record a
/// transaction changed has bit 7 of the byte set ([`crate::adt::TX_MARK`])
/// and is what it was; one it appended (`0`, [`TX_APPENDED`]) is not there
/// for anyone else yet.
pub(crate) fn record_status(byte: u8) -> RecordStatus {
    match byte & !crate::adt::TX_MARK {
        b'*' | b'D' => RecordStatus::Deleted,
        _ if byte == TX_APPENDED => RecordStatus::Deleted,
        _ => RecordStatus::Active,
    }
}

/// The deletion byte of a blank record a transaction appended, as the
/// original's server writes it.
pub const TX_APPENDED: u8 = b'0';

/// Opens the DBF table at `path` as the table type `flavour` (see
/// [`TableFile::open`]), to be written if `write` and the file can be.
pub(crate) fn open(path: &Path, flavour: Flavour, write: bool) -> Result<TableFile, Error> {
    let opened = match write {
        true => File::options().read(true).write(true).open(path).ok(),
        false => None,
    };
    let write = opened.is_some();
    let mut file = match opened {
        Some(file) => file,
        None => File::open(path).map_err(Error::Io)?,
    };
    let (structure, info) = read_structure(&mut file, flavour)?;
    file.seek(SeekFrom::Start(0))?;
    let records = Records::new(file, &structure.header);
    let memo = match info.memo {
        Some(kind) => {
            let mut memo = MemoFile::open_kind(&memo_path(path, kind), kind, write)?;
            memo.set_iv_prefix(info.aes_layout);
            Some(memo)
        }
        None => None,
    };
    Ok(TableFile::new(structure, records, memo, Some(info)).writable(write))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::Value as Json;

    use super::*;
    use crate::adt::tests::assert_value;

    /// Every `.dbf` fixture with its JSON dump and flavour.
    fn tables() -> Vec<(PathBuf, Json, Flavour)> {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let mut tables = Vec::new();
        for (kind, flavour) in [
            ("ntx", Flavour::Clipper),
            ("cdx", Flavour::FoxPro),
            ("vfp", Flavour::Visual),
        ] {
            for dir in std::fs::read_dir(fixtures.join(kind)).unwrap() {
                for entry in std::fs::read_dir(dir.unwrap().path()).unwrap() {
                    let path = entry.unwrap().path();
                    let json = path.with_extension("json");
                    if path.extension().is_some_and(|e| e == "dbf") && json.exists() {
                        let json = std::fs::read_to_string(json).unwrap();
                        tables.push((path, serde_json::from_str(&json).unwrap(), flavour));
                    }
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
        for (path, dump, flavour) in &tables {
            let name = path.display();
            let table = open(path, *flavour, false).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            let info = table.dbf().unwrap();
            let structure = &table.structure;
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
            let block_size = table.memo_block_size().unwrap_or(0);
            assert_eq!(u64::from(block_size), dump["memo_block_size"], "{name}");
            if *flavour != Flavour::Visual {
                assert_eq!(info.code_page == 0, dump["char_type"] == "oem", "{name}");
            }
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
            // The file ends with 0x1A right after the last record.
            let size = std::fs::metadata(path).unwrap().len();
            let records = u64::from(header.record_slots) * u64::from(header.record_length);
            assert_eq!(size, u64::from(header.data_offset) + records + 1, "{name}");
        }
    }

    /// Every record's raw bytes and deleted flag, and every value, as the
    /// engine dumped them.
    #[test]
    fn records_match_the_engine() {
        let (mut records_checked, mut values, mut nulls) = (0, 0, 0);
        for (path, dump, flavour) in &tables() {
            let name = path.display();
            let mut table = open(path, *flavour, false).unwrap();
            let structure = table.structure.clone();
            if structure.header.encrypted() {
                continue;
            }
            // The engine reads an OEM table's 8-bit text in the client's OEM
            // code page and hands it out converted to ANSI; the values here
            // are the bytes as stored, which the raw records already check.
            let oem = dump["char_type"] == "oem";
            let mut buf = vec![0; structure.header.record_length as usize];
            for record in dump["records"].as_array().unwrap() {
                let slot = record["recno"].as_u64().unwrap() as u32;
                table.read(slot, &mut buf).unwrap();
                let raw: String = buf.iter().map(|b| format!("{b:02x}")).collect();
                assert_eq!(
                    raw,
                    record["raw"].as_str().unwrap().to_lowercase(),
                    "{name} #{slot}"
                );
                let deleted = table.status(slot).unwrap() == RecordStatus::Deleted;
                assert_eq!(deleted, record["deleted"] == true, "{name} #{slot}");
                records_checked += 1;
                for field in &structure.fields {
                    let expected = &record["values"][&field.name];
                    let what = format!("{name} #{slot} {}: {expected}", field.name);
                    assert_eq!(field.is_null(&buf), expected["null"] == true, "{what}");
                    nulls += usize::from(expected["null"] == true);
                    let value = field
                        .value(&buf, table.memo())
                        .unwrap_or_else(|e| panic!("{what}: {e:?}"));
                    let ansi_text = matches!(
                        field.field_type,
                        FieldType::Character | FieldType::Memo | FieldType::LegacyVarChar
                    );
                    if !(oem && ansi_text) {
                        assert_value(field, &value, expected, &what);
                    }
                    values += 1;
                }
            }
        }
        assert!(
            records_checked > 100,
            "checked only {records_checked} records"
        );
        assert!(values > 1000, "checked only {values} values");
        assert!(nulls > 20, "saw only {nulls} NULLs");
    }

    #[test]
    fn blank_records() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let table = open(
            &fixtures.join("vfp/vfp_more/vfp_more.dbf"),
            Flavour::Visual,
            false,
        )
        .unwrap();
        let record = table.structure.null_record();
        assert_eq!(record[0], b' ');
        for field in &table.structure.fields {
            assert!(!field.is_null(&record), "{}", field.name);
            if matches!(field.field_type, FieldType::VarChar | FieldType::VarBinary) {
                assert_eq!(field.var_length(&record), Some(0), "{}", field.name);
            }
        }
        let table = open(
            &fixtures.join("cdx/dbf_ext/dbf_ext.dbf"),
            Flavour::FoxPro,
            false,
        )
        .unwrap();
        let record = table.structure.null_record();
        for field in &table.structure.fields {
            let never_null = matches!(field.field_type, FieldType::NChar | FieldType::NMemo);
            assert_eq!(field.is_null(&record), !never_null, "{}", field.name);
        }
    }

    #[test]
    fn rejects_other_files() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let adt = fixtures.join("adt/all_types/all_types.adt");
        assert!(matches!(
            read_structure(&mut File::open(adt).unwrap(), Flavour::Clipper),
            Err(Error::Corrupt(_))
        ));
        let mut short = &[0x03u8, 0, 0][..];
        assert!(matches!(
            read_structure(&mut short, Flavour::Clipper),
            Err(Error::Corrupt(_))
        ));
    }

    #[test]
    fn memo_paths() {
        assert_eq!(
            memo_path(Path::new("/d/a.dbf"), MemoKind::Dbt),
            PathBuf::from("/d/a.dbt")
        );
        assert_eq!(
            memo_path(Path::new("/d/A.DBF"), MemoKind::Fpt),
            PathBuf::from("/d/A.FPT")
        );
    }
}
