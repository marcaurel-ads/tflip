//! The values of a table's fields in its current record: as read, NULL
//! or empty, memo values and their lengths, including values written but
//! not yet flushed.

use super::*;

impl Table {
    /// Whether the connection's user may not read `field`, which then reads
    /// as NULL (`fixtures/sql/ddcolrights`).
    fn hides(&self, field: &Field) -> bool {
        !self.hidden.is_empty()
            && self
                .hidden
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&field.name))
    }

    /// Whether `field` is a memo field of an encrypted Clipper or FoxPro table
    /// that this handle has not given the password of: its values cannot be
    /// read, though the record can (`fixtures/write/dbf_encryption_cdx`).
    fn memo_locked(&self, field: &Field) -> bool {
        field.is_memo()
            && matches!(field.layout, Layout::Dbf(_))
            && !self.keyed()
            && self.state().file().structure.header.encrypted()
            // An AES table's records read blank and its memo values empty.
            && !self.state().file().is_aes()
    }

    /// Whether `field` is a memo field of an enciphered record of a table
    /// that is not encrypted as a whole, which this handle has not given the
    /// password of: it reads as empty, though the record's image is clear
    /// (`fixtures/write/dbf_encryption_cached_cdx`).
    fn memo_of_enciphered_record(&self, field: &Field) -> bool {
        if !field.is_memo() || self.keyed() {
            return false;
        }
        let state = self.state();
        let Some(slot) = state.position.record else {
            return false;
        };
        // The record kept after the password went has a valid memo
        // reference to refuse; an ADT table opened exclusively reads empty
        // for the others, where a shared one reads NULL
        // (`fixtures/write/dbf_encryption_cached_adt`, `share.json`).
        let kept = state
            .image
            .as_ref()
            .is_some_and(|(image, _)| *image == slot);
        if !kept && !(self.format() == Format::Adt && self.exclusive()) {
            return false;
        }
        let mut file = state.file();
        // An AES record does not count as enciphered, and its memo reads NULL.
        !file.is_aes()
            && file
                .status_byte(slot)
                .is_ok_and(|status| file.enciphered(&[status & !tflip_storage::adt::TX_MARK]))
    }

    /// An empty record of the table: every field NULL, as the original
    /// engine fills it in (`AdsGetNullRecord`, `AdsClearRecordBuffer`); for a
    /// live SQL cursor, every field of the table it reads.
    pub fn null_record(&self) -> Vec<u8> {
        self.null_record.clone()
    }

    /// Whether `field` holds NULL in the current record.
    pub fn is_null(&self, field: FieldRef) -> Result<bool, Error> {
        let field = self.field(field)?;
        if self.hides(field) {
            return Ok(true);
        }
        if self.memo_locked(field) || self.memo_of_enciphered_record(field) {
            return Ok(false);
        }
        if let Some(value) = self.pending_memo(field) {
            return Ok(value.is_none());
        }
        Ok(field.is_null(&self.read_record()?))
    }

    /// The value memo field `field` of the current record is set to and not
    /// written yet, if it is: `Some(None)` for NULL.
    pub(super) fn pending_memo(&self, field: &Field) -> Option<Option<Vec<u8>>> {
        let state = self.state();
        let record = state.position.record?;
        let set = state.pending.as_ref().filter(|p| p.slot == record);
        if set.is_some_and(|pending| pending.nulled.contains(&field.offset)) {
            return Some(None);
        }
        let value = set.and_then(|pending| {
            pending
                .memos
                .iter()
                .find(|(offset, _)| *offset == field.offset)
        });
        if let Some((_, value)) = value {
            return Some(value.clone());
        }
        // A value the transaction holds back until its commit.
        let reader = state
            .connection
            .as_deref()
            .and_then(crate::transaction::reader)?;
        let transactions = state.file.transactions();
        let held = transactions.get(&record).filter(|held| held.tx == reader)?;
        held.memos
            .iter()
            .find(|(offset, _)| *offset == field.offset)
            .map(|(_, v)| v.clone())
    }

    /// Whether the text getters refuse `field` of the current record: a text
    /// Memo of a Clipper or FoxPro table given a binary value in the change
    /// to it.
    pub fn binary_memo(&self, field: &Field) -> bool {
        let state = self.state();
        let Some(record) = state.position.record else {
            return false;
        };
        state
            .pending
            .as_ref()
            .filter(|p| p.slot == record)
            .is_some_and(|p| p.binary_memos.contains(&field.offset))
    }

    /// Whether `field` is empty in the current record (see
    /// [`Field::is_empty`]).
    pub fn is_empty(&self, field: FieldRef) -> Result<bool, Error> {
        let field = self.field(field)?;
        if self.hides(field) {
            return Ok(true);
        }
        let record = self.read_record()?;
        // A VFP field whose NULL bit is set is not empty, a memo among them
        // (`fixtures/fields/nullable.json`).
        if matches!(&field.layout, Layout::Dbf(dbf) if dbf.null_bit.is_some())
            && field.is_null(&record)
        {
            return Ok(false);
        }
        if let Some(value) = self.pending_memo(field) {
            return Ok(value.is_none());
        }
        Ok(field.is_empty(&record))
    }

    /// The length in bytes of a memo field's value in the current record
    /// (UTF-16LE bytes for NMemo); 0 for NULL. Fields that are not Memo,
    /// Binary, Image or NMemo are [`Error::InvalidFieldType`].
    pub fn memo_length(&self, field: FieldRef) -> Result<u32, Error> {
        let field = self.field(field)?;
        if self.hides(field) && field.is_memo() {
            return Ok(0);
        }
        if let Some(value) = self.pending_memo(field) {
            return Ok(value.map_or(0, |v| v.len() as u32));
        }
        let record = self.read_record()?;
        if let Layout::Dbf(_) = field.layout {
            // A DBF VarChar's length is in the record (its long values are
            // in the memo file); a memo value's in the memo file.
            if field.field_type == FieldType::LegacyVarChar {
                return Ok(u32::from(field.var_length(&record).unwrap_or(0)));
            }
            return self
                .read_memo_of(field, &record)
                .map(|data| data.len() as u32);
        }
        field
            .memo_ref(&record)
            .map(|memo| memo.length)
            .ok_or_else(|| not_memo(field))
    }

    /// The value of a memo field in the current record, decompressed (UTF-16LE
    /// bytes for NMemo); empty for NULL. Fields that are not Memo, Binary,
    /// Image or NMemo are [`Error::InvalidFieldType`].
    pub fn read_memo(&self, field: FieldRef) -> Result<Vec<u8>, Error> {
        let field = self.field(field)?;
        if self.hides(field) && field.is_memo() {
            return Ok(Vec::new());
        }
        if self.memo_locked(field) {
            return Err(Error::EncryptionNotEnabled);
        }
        if let Some(value) = self.pending_memo(field) {
            return Ok(value.unwrap_or_default());
        }
        let record = self.read_record()?;
        self.read_memo_of(field, &record)
    }

    /// The value of the memo field `field` in `record` (see
    /// [`Table::read_memo`]).
    pub(super) fn read_memo_of(&self, field: &Field, record: &[u8]) -> Result<Vec<u8>, Error> {
        if field.is_null(record) {
            return Ok(Vec::new());
        }
        let record_number = self.record_number();
        let state = self.state();
        state
            .with_memo(|mut memo| {
                if let Some(m) = memo.as_mut() {
                    m.decipher_record(record_number);
                }
                field.memo_data(record, memo)
            })
            .map_err(storage_error)?
            .ok_or_else(|| not_memo(field))
    }

    /// The length of the data `field` holds in the current record, counted
    /// in `unit`: a memo field's value length and a VarChar, VarBinary or
    /// NVarChar field's stored length, 0 for NULL; an NChar field's
    /// characters; else the declared length, which for the binary types is
    /// their storage size (Integer 4, Double 8, ...). Fixed-length fields
    /// report their length even when NULL, as in the original engine.
    /// [`LengthUnit::RecordBytes`] is the field's bytes in the record,
    /// whatever it holds.
    pub fn data_length(&self, field: FieldRef, unit: LengthUnit) -> Result<u32, Error> {
        let field = self.field(field)?;
        if unit == LengthUnit::RecordBytes {
            return Ok(u32::from(field.size));
        }
        if let Some(value) = self.pending_memo(field) {
            let bytes = value.map_or(0, |v| v.len() as u32);
            let units = unit == LengthUnit::CodeUnits && field.field_type == FieldType::NMemo;
            return Ok(if units { bytes / 2 } else { bytes });
        }
        let record = self.read_record()?;
        let variable = field.is_memo() || field.var_length(&record).is_some();
        if variable && field.is_null(&record) {
            return Ok(0);
        }
        let unicode = matches!(
            field.field_type,
            FieldType::NChar | FieldType::NVarChar | FieldType::NMemo
        );
        let bytes = match (field.memo_ref(&record), field.var_length(&record)) {
            (Some(_), _) if matches!(field.layout, Layout::Dbf(_)) => {
                self.read_memo_of(field, &record)?.len() as u32
            }
            (Some(memo), _) => memo.length,
            (_, Some(length)) if field.field_type == FieldType::NVarChar => 2 * u32::from(length),
            (_, Some(length)) => u32::from(length),
            _ if field.field_type == FieldType::NChar => u32::from(field.size),
            _ => u32::from(field.length()),
        };
        Ok(match unit {
            LengthUnit::CodeUnits if unicode => bytes / 2,
            _ => bytes,
        })
    }

    /// The value `field` holds in the current record.
    pub fn value(&self, field: FieldRef) -> Result<Value, Error> {
        let field = self.field(field)?;
        if self.hides(field) {
            return Ok(Value::Null);
        }
        if self.memo_locked(field) {
            return Err(Error::EncryptionNotEnabled);
        }
        if self.memo_of_enciphered_record(field) {
            return Ok(match field.field_type {
                FieldType::Memo | FieldType::NMemo => Value::Text(String::new()),
                _ => Value::Binary(Vec::new()),
            });
        }
        if let Some(value) = self.pending_memo(field) {
            let Some(bytes) = value else {
                return Ok(Value::Null);
            };
            return Ok(match field.field_type {
                FieldType::NMemo => Value::Text(tflip_storage::field::utf16(&bytes)),
                FieldType::Memo => Value::Text(tflip_storage::cp1252::decode(&bytes)),
                _ => Value::Binary(bytes),
            });
        }
        let record = self.read_record()?;
        let record_number = self.record_number();
        let state = self.state();
        state
            .with_memo(|mut memo| {
                if let Some(m) = memo.as_mut() {
                    m.decipher_record(record_number);
                }
                field.value(&record, memo)
            })
            .map_err(storage_error)
    }
}

/// The failure of a memo call on a field that is no memo field, which names
/// the field (`fixtures/errors/param_n.json`).
fn not_memo(field: &Field) -> Error {
    Error::InvalidFieldType.detailed(format!("Field: \"{}\"", field.name))
}
