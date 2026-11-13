//! Fields of tables of any format: their types, and how their values are
//! read from a record.
//!
//! A field's [`Layout`] says how its bytes are stored: as in ADT tables (see
//! [`crate::adt`]) or as in DBF tables (see [`dbf::Layout`]). The methods of
//! [`Field`] read a value by its layout's rules.

use serde::{Deserialize, Serialize};

use crate::adt;
use crate::dbf;
use crate::memo::MemoFile;
use crate::table::Error;
use crate::value::Value;

/// The type of a field, with the codes ADT files store (the same as the
/// `ADS_*` field type constants of ACE).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldType {
    Logical,
    Numeric,
    Date,
    Character,
    Memo,
    Binary,
    Image,
    /// The legacy VarChar (code 8), part inline, part in the memo file.
    LegacyVarChar,
    /// The 3-byte date of DBF tables (`ADS_COMPACTDATE`).
    CompactDate,
    Double,
    Integer,
    ShortInt,
    Time,
    TimeStamp,
    AutoInc,
    Raw,
    CurDouble,
    Money,
    LongInt,
    CiCharacter,
    RowVersion,
    ModTime,
    VarChar,
    VarBinary,
    NChar,
    NVarChar,
    NMemo,
    Guid,
    /// A code ADT tables do not use.
    Other(u16),
}

impl FieldType {
    /// Whether a field of the type keeps its values in the memo file (Memo,
    /// Binary, Image and NMemo).
    pub fn is_memo(self) -> bool {
        matches!(
            self,
            FieldType::Memo | FieldType::Binary | FieldType::Image | FieldType::NMemo
        )
    }

    /// The field type stored as `code`.
    pub fn from_code(code: u16) -> FieldType {
        match code {
            1 => FieldType::Logical,
            2 => FieldType::Numeric,
            3 => FieldType::Date,
            4 => FieldType::Character,
            5 => FieldType::Memo,
            6 => FieldType::Binary,
            7 => FieldType::Image,
            8 => FieldType::LegacyVarChar,
            9 => FieldType::CompactDate,
            10 => FieldType::Double,
            11 => FieldType::Integer,
            12 => FieldType::ShortInt,
            13 => FieldType::Time,
            14 => FieldType::TimeStamp,
            15 => FieldType::AutoInc,
            16 => FieldType::Raw,
            17 => FieldType::CurDouble,
            18 => FieldType::Money,
            19 => FieldType::LongInt,
            20 => FieldType::CiCharacter,
            21 => FieldType::RowVersion,
            22 => FieldType::ModTime,
            23 => FieldType::VarChar,
            24 => FieldType::VarBinary,
            26 => FieldType::NChar,
            27 => FieldType::NVarChar,
            28 => FieldType::NMemo,
            29 => FieldType::Guid,
            other => FieldType::Other(other),
        }
    }

    /// Whether a field of this type can hold NULL. Every type has a NULL
    /// value except AutoInc and RowVersion, which the engine always fills.
    pub fn nullable(self) -> bool {
        !matches!(self, FieldType::AutoInc | FieldType::RowVersion)
    }

    /// The code the field type is stored as.
    pub fn code(self) -> u16 {
        match self {
            FieldType::Logical => 1,
            FieldType::Numeric => 2,
            FieldType::Date => 3,
            FieldType::Character => 4,
            FieldType::Memo => 5,
            FieldType::Binary => 6,
            FieldType::Image => 7,
            FieldType::LegacyVarChar => 8,
            FieldType::CompactDate => 9,
            FieldType::Double => 10,
            FieldType::Integer => 11,
            FieldType::ShortInt => 12,
            FieldType::Time => 13,
            FieldType::TimeStamp => 14,
            FieldType::AutoInc => 15,
            FieldType::Raw => 16,
            FieldType::CurDouble => 17,
            FieldType::Money => 18,
            FieldType::LongInt => 19,
            FieldType::CiCharacter => 20,
            FieldType::RowVersion => 21,
            FieldType::ModTime => 22,
            FieldType::VarChar => 23,
            FieldType::VarBinary => 24,
            FieldType::NChar => 26,
            FieldType::NVarChar => 27,
            FieldType::NMemo => 28,
            FieldType::Guid => 29,
            FieldType::Other(code) => code,
        }
    }
}

/// The value in the decimals slot of a compressed Memo, Binary or NMemo
/// field; it is a marker, not decimals.
pub(crate) const COMPRESSED: u16 = 0x1000;

/// How a field's bytes are laid out in a record: as in ADT tables, or as in
/// DBF tables (see [`dbf::Layout`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layout {
    #[default]
    Adt,
    Dbf(dbf::Layout),
}

/// A field descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    /// The field name.
    pub name: String,
    pub field_type: FieldType,
    /// Where the field starts in a record (the first field is at 5).
    pub offset: u16,
    /// How many bytes the field takes in a record.
    pub size: u16,
    /// The raw decimals slot: the decimals, or [`COMPRESSED`] in a
    /// compressed memo field.
    pub(crate) decimals_slot: u16,
    /// How the field is laid out.
    pub layout: Layout,
}

impl Field {
    /// The declared length: the stored size, except for the variable-length
    /// and Unicode types, which store more.
    pub fn length(&self) -> u16 {
        match &self.layout {
            Layout::Adt => adt::length(self),
            Layout::Dbf(layout) => layout.length(self),
        }
    }

    /// The number of decimals.
    pub fn decimals(&self) -> u16 {
        if self.decimals_slot == COMPRESSED {
            0
        } else {
            self.decimals_slot
        }
    }

    /// Whether the field is a compressed memo field.
    pub fn compressed(&self) -> bool {
        self.decimals_slot == COMPRESSED
    }

    /// The field's bytes in `record` (a whole record, prefix included).
    pub fn bytes<'r>(&self, record: &'r [u8]) -> &'r [u8] {
        let start = usize::from(self.offset);
        &record[start..start + usize::from(self.size)]
    }

    /// Whether the field keeps its values in the memo file (Memo, Binary,
    /// Image and NMemo).
    pub fn is_memo(&self) -> bool {
        self.field_type.is_memo()
    }

    /// The memo reference a memo field holds in `record`, or `None` for other
    /// fields; block 0 is NULL.
    pub fn memo_ref(&self, record: &[u8]) -> Option<MemoRef> {
        if !self.is_memo() {
            return None;
        }
        Some(match &self.layout {
            Layout::Adt => adt::memo_ref(self, record),
            Layout::Dbf(layout) => layout.memo_ref(self, record),
        })
    }

    /// The value a memo field holds in `record`, as stored (decompressed;
    /// UTF-16LE bytes for NMemo; a DBF Binary or Image value without its
    /// SIx tag), read from `memo`; empty for NULL, and `None` for other
    /// fields.
    pub fn memo_data(
        &self,
        record: &[u8],
        memo: Option<&mut MemoFile>,
    ) -> Result<Option<Vec<u8>>, Error> {
        if !self.is_memo() {
            return Ok(None);
        }
        match &self.layout {
            Layout::Adt => adt::memo_data(self, record, memo).map(Some),
            Layout::Dbf(layout) => layout.memo_data(self, record, memo).map(Some),
        }
    }

    /// The value of a DBF table's General or Picture field (see
    /// [`FieldType::Other`]) as its memo file keeps it, read from `memo`;
    /// `None` if it has none, or for other fields.
    pub fn object_data(
        &self,
        record: &[u8],
        memo: Option<&mut MemoFile>,
    ) -> Result<Option<Vec<u8>>, Error> {
        match (&self.layout, self.field_type) {
            (Layout::Dbf(layout), FieldType::Other(_)) => layout.object_data(self, record, memo),
            _ => Ok(None),
        }
    }

    /// The value the field holds in `record`. Memo values are read from
    /// `memo`, the table's memo file; a memo field without one is an error.
    pub fn value(&self, record: &[u8], memo: Option<&mut MemoFile>) -> Result<Value, Error> {
        match &self.layout {
            Layout::Adt => adt::value(self, record, memo),
            Layout::Dbf(layout) => layout.value(self, record, memo),
        }
    }

    /// Writes `value` into the field in `record`, as the table's format
    /// stores it (see [`adt::encode`] and the DBF layout's rules: dates as
    /// text, NULL as blanks or a VFP NULL bit).
    pub fn encode(&self, value: &Value, record: &mut [u8]) -> Result<(), Error> {
        match &self.layout {
            Layout::Adt => {
                let start = usize::from(self.offset);
                let slot = record
                    .get_mut(start..start + usize::from(self.size))
                    .ok_or(Error::Corrupt("record of the wrong length"))?;
                adt::encode(self, value, slot)
            }
            Layout::Dbf(layout) => layout.encode(self, value, record),
        }
    }

    /// Writes the field's blank bytes into `record` (`AdsSetEmpty` on a DBF
    /// field), clearing a VFP field's NULL bit; an ADT field's NULL.
    pub fn encode_blank(&self, record: &mut [u8]) -> Result<(), Error> {
        match &self.layout {
            Layout::Adt => self.encode(&Value::Null, record),
            Layout::Dbf(layout) => {
                layout.encode_blank(self, record);
                Ok(())
            }
        }
    }

    /// Stores the value of the memo field (as the memo file keeps it: the
    /// text's bytes, UTF-16LE for NMemo, a binary value's bytes; `None` for
    /// none) in `memo`, and its reference in `record`, as the table's format
    /// does (see [`adt::store_memo`] and the DBF memo files' rules).
    pub fn store_memo(
        &self,
        value: Option<&[u8]>,
        record: &mut [u8],
        memo: &mut MemoFile,
    ) -> Result<(), Error> {
        match &self.layout {
            Layout::Adt => {
                let start = usize::from(self.offset);
                let slot = record
                    .get_mut(start..start + usize::from(self.size))
                    .ok_or(Error::Corrupt("record of the wrong length"))?;
                adt::store_memo(self, value, slot, memo)
            }
            Layout::Dbf(layout) => crate::xmemo::store(layout, self, value, record, memo),
        }
    }

    /// Stores `text` in an ADS VarChar field of a Clipper or FoxPro table:
    /// inline if it fits the field's size − 2 bytes, else its first size − 6
    /// bytes inline and the rest in `memo`, over the old value's rest where
    /// that can be. An ADT table's legacy VarChar is laid out alike, its rest
    /// allocated anew (see [`adt::store_legacy_var_char`]).
    pub fn store_var_char(
        &self,
        text: &[u8],
        record: &mut [u8],
        memo: Option<&mut MemoFile>,
    ) -> Result<(), Error> {
        let Layout::Dbf(layout) = &self.layout else {
            let start = usize::from(self.offset);
            let slot = record
                .get_mut(start..start + usize::from(self.size))
                .ok_or(Error::Corrupt("record of the wrong length"))?;
            return adt::store_legacy_var_char(text, slot, memo);
        };
        let size = usize::from(self.size);
        if text.len() <= size - 2 {
            return layout.encode(self, &Value::Text(crate::cp1252::decode(text)), record);
        }
        let memo = memo.ok_or(Error::Corrupt("VarChar overflow without a memo file"))?;
        let start = usize::from(self.offset);
        let inline = size - 6;
        let slot = &record[start..start + size];
        let old_length = usize::from(u16::from_le_bytes([slot[size - 2], slot[size - 1]]));
        let old = match slot.iter().all(|&b| b == b' ') || old_length <= size - 2 {
            true => 0,
            false => u32::from_le_bytes(slot[inline..inline + 4].try_into().unwrap()),
        };
        // The inline part is set even when the rest cannot be written.
        record[start..start + inline].copy_from_slice(&text[..inline]);
        let block = crate::xmemo::store_remainder(memo, old, &text[inline..])?;
        let slot = &mut record[start..start + size];
        slot[inline..inline + 4].copy_from_slice(&block.to_le_bytes());
        let length = u16::try_from(text.len()).map_err(|_| Error::Corrupt("VarChar too long"))?;
        slot[size - 2..].copy_from_slice(&length.to_le_bytes());
        Ok(())
    }

    /// Writes the bytes `AdsGetRecord` shows for the field into `record`, a
    /// copy of the record: a VFP field that is NULL shows blank.
    pub fn show(&self, record: &mut [u8]) {
        if let Layout::Dbf(layout) = &self.layout {
            layout.show(self, record);
        }
    }

    /// Clears a VFP field's NULL bit in `record`, after its bytes were set
    /// to a value; nothing for other fields.
    pub fn set_not_null(&self, record: &mut [u8]) {
        if let Layout::Dbf(layout) = &self.layout {
            layout.set_not_null(record);
        }
    }

    /// Whether the field can be set to NULL: not a VFP field without a NULL
    /// bit.
    pub fn nullable(&self) -> bool {
        match &self.layout {
            Layout::Adt => true,
            Layout::Dbf(layout) => layout.nullable(),
        }
    }

    /// Whether a NULL the field reads is a blank value: a Clipper or FoxPro
    /// table's field, which has no NULL (see [`Field::expression_value`]).
    pub fn blank_is_null(&self) -> bool {
        matches!(&self.layout, Layout::Dbf(layout) if !layout.vfp_table())
    }

    /// The value the field holds in `record` as expressions read it: as
    /// [`Field::value`], but a blank field of a Clipper or FoxPro table is
    /// its blank value (spaces, 0, the empty date), as those tables have no
    /// NULL.
    pub fn expression_value(
        &self,
        record: &[u8],
        memo: Option<&mut MemoFile>,
    ) -> Result<Value, Error> {
        match &self.layout {
            Layout::Adt => adt::value(self, record, memo),
            Layout::Dbf(layout) => layout.expression_value(self, record, memo),
        }
    }

    /// The stored length of a VarChar or VarBinary (bytes) or NVarChar
    /// (UTF-16 code units) field in `record`. `None` for other fields.
    pub fn var_length(&self, record: &[u8]) -> Option<u16> {
        match &self.layout {
            Layout::Adt => adt::var_length(self, record),
            Layout::Dbf(layout) => layout.var_length(self, record),
        }
    }

    /// Whether the field holds NULL in `record`.
    pub fn is_null(&self, record: &[u8]) -> bool {
        match &self.layout {
            Layout::Adt => adt::is_null(self, record),
            Layout::Dbf(layout) => layout.is_null(self, record),
        }
    }

    /// Whether the field is empty in `record` (`AdsIsEmpty`): in an ADT
    /// table, whether it is NULL; in a DBF table, whether it is blank (see
    /// `dbf::Layout::is_empty`).
    pub fn is_empty(&self, record: &[u8]) -> bool {
        match &self.layout {
            Layout::Adt => adt::is_null(self, record),
            Layout::Dbf(layout) => layout.is_empty(self, record),
        }
    }

    /// Whether the field is flagged binary (`AdsIsFieldBinary`): only a VFP
    /// table's fields can be (see [`dbf::Layout::binary`]).
    pub fn binary(&self) -> bool {
        matches!(&self.layout, Layout::Dbf(layout) if layout.binary())
    }

    /// The bytes of the field in an empty record, as the original engine
    /// fills them in: in an ADT table its NULL value, in a DBF table its
    /// blank one.
    pub fn null_bytes(&self) -> Vec<u8> {
        match &self.layout {
            Layout::Adt => adt::null_bytes(self),
            Layout::Dbf(layout) => layout.blank_bytes(self),
        }
    }
}

/// Where a memo value is in the memo file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoRef {
    /// The first block of the value; 0 for NULL.
    pub block: u32,
    /// The bytes stored in the memo file: the zlib stream of a compressed
    /// value, else the value itself.
    pub stored: u32,
    /// The value's length in bytes (UTF-16LE bytes for NMemo).
    pub length: u32,
}

/// The bytes a memo field of `field_type` stores for `text`: UTF-16LE for an
/// NMemo, else cp1252.
pub fn memo_text_bytes(field_type: FieldType, text: &str) -> Vec<u8> {
    match field_type {
        FieldType::NMemo => text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        _ => crate::cp1252::encode(text),
    }
}

/// Decodes UTF-16LE bytes; unpaired surrogates become U+FFFD.
pub fn utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .collect();
    String::from_utf16_lossy(&units)
}
