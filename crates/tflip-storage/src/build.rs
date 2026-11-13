//! ADT tables built in memory from values: the result sets of SQL
//! statements, which clients read like any other table.
//!
//! The table is laid out as an ADT file would be (header, descriptors,
//! records, and a memo file for memo fields), so it is read through the same
//! [`Records`] and [`MemoFile`] as tables on disk.

use crate::adt::{self, HEADER_SIZE};
use crate::field::{Field, FieldType};
use crate::memo::MemoFile;
use crate::source::Source;
use crate::table::{Error, Format, Header, Records, Structure, TableFile, flags};
use crate::value::Value;

/// The block size of memo files built in memory.
const MEMO_BLOCK: usize = 8;

/// A column of a table to build: its name, type, declared length and
/// decimals (as [`adt::field`] takes them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub field_type: FieldType,
    pub length: u16,
    pub decimals: u16,
}

/// Builds a table with `columns` whose records are `rows`, each a value per
/// column. A value the column's type cannot hold is an error.
pub fn build(columns: &[Column], rows: &[Vec<Value>]) -> Result<TableFile, Error> {
    let mut fields = Vec::with_capacity(columns.len());
    let mut offset = 5u32;
    for column in columns {
        let field = adt::field(
            &column.name,
            column.field_type,
            u16::try_from(offset).map_err(|_| Error::Corrupt("record too long"))?,
            column.length,
            column.decimals,
        );
        offset += u32::from(field.size);
        fields.push(field);
    }
    let field_count = u16::try_from(fields.len()).map_err(|_| Error::Corrupt("too many fields"))?;
    let memo_fields = fields.iter().any(Field::is_memo);
    let header = Header {
        format: Format::Adt,
        record_slots: u32::try_from(rows.len()).map_err(|_| Error::Corrupt("too many rows"))?,
        data_offset: (HEADER_SIZE + adt::DESCRIPTOR_SIZE * fields.len()) as u32,
        record_length: offset,
        free_head: 0,
        free_count: 0,
        encryption_type: 0,
        flags: if memo_fields { flags::MEMO } else { 0 },
        field_count,
        extension: 0,
        dictionary: false,
    };
    let structure = Structure { header, fields };

    let mut bytes = Vec::with_capacity(
        structure.header.data_offset as usize
            + rows.len() * structure.header.record_length as usize,
    );
    bytes.extend_from_slice(&adt::header_bytes(&structure.header));
    for field in &structure.fields {
        bytes.extend_from_slice(&adt::field_bytes(field));
    }
    let mut memo = Memo::new();
    for row in rows {
        if row.len() != structure.fields.len() {
            return Err(Error::Corrupt("a row has the wrong number of values"));
        }
        let mut record = structure.null_record();
        // Active.
        record[0] = 0x04;
        for (field, value) in structure.fields.iter().zip(row) {
            let start = usize::from(field.offset);
            let slot = &mut record[start..start + usize::from(field.size)];
            encode(field, value, slot, &mut memo)?;
        }
        bytes.extend_from_slice(&record);
    }

    let records = Records::new(bytes, &structure.header);
    let memo = match memo_fields {
        true => Some(MemoFile::from_source(Source::from(memo.bytes))?),
        false => None,
    };
    Ok(TableFile::new(structure, records, memo, None))
}

/// A memo file being built: a header block with the block size, then each
/// value in whole blocks.
struct Memo {
    bytes: Vec<u8>,
}

impl Memo {
    fn new() -> Memo {
        let mut bytes = vec![0; MEMO_BLOCK];
        bytes[6..8].copy_from_slice(&(MEMO_BLOCK as u16).to_be_bytes());
        Memo { bytes }
    }

    /// Stores `value` and returns its reference: first block and length.
    fn store(&mut self, value: &[u8]) -> Result<[u8; 9], Error> {
        let block = u32::try_from(self.bytes.len() / MEMO_BLOCK)
            .map_err(|_| Error::Corrupt("memo file too large"))?;
        let length = u32::try_from(value.len()).map_err(|_| Error::Corrupt("memo too long"))?;
        self.bytes.extend_from_slice(value);
        // An empty value still takes a block, so its reference is not NULL.
        let padded = value.len().max(1).div_ceil(MEMO_BLOCK) * MEMO_BLOCK;
        self.bytes
            .resize(self.bytes.len() + padded - value.len(), 0);
        let mut reference = [0; 9];
        reference[..4].copy_from_slice(&block.to_le_bytes());
        reference[4..8].copy_from_slice(&length.to_le_bytes());
        Ok(reference)
    }
}

/// Writes `value` into `slot`, the field's bytes in a record, which hold
/// its NULL value; memo values go to `memo`.
fn encode(field: &Field, value: &Value, slot: &mut [u8], memo: &mut Memo) -> Result<(), Error> {
    let stored = match (field.field_type, value) {
        (_, Value::Null) => return Ok(()),
        // An empty memo value is NULL, as the original engine's static
        // cursors hold it (`fixtures/sql/dbf.json`).
        (FieldType::Memo | FieldType::NMemo, Value::Text(text)) if text.is_empty() => return Ok(()),
        (FieldType::Binary | FieldType::Image, Value::Binary(data)) if data.is_empty() => {
            return Ok(());
        }
        (FieldType::Memo | FieldType::NMemo, Value::Text(text)) => {
            crate::field::memo_text_bytes(field.field_type, text)
        }
        (FieldType::Binary | FieldType::Image, Value::Binary(data)) => data.clone(),
        (FieldType::Memo | FieldType::NMemo | FieldType::Binary | FieldType::Image, _) => {
            return Err(Error::Corrupt("value does not match the field type"));
        }
        _ => return adt::encode(field, value, slot),
    };
    slot.copy_from_slice(&memo.store(&stored)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, field_type: FieldType, length: u16, decimals: u16) -> Column {
        Column {
            name: name.to_owned(),
            field_type,
            length,
            decimals,
        }
    }

    fn read_back(table: &mut TableFile) -> Vec<Vec<Value>> {
        let fields = table.structure.fields.clone();
        let mut rows = Vec::new();
        for slot in 1..=table.slots() {
            let record = table.record(slot).unwrap();
            let values = fields
                .iter()
                .map(|f| f.value(&record, table.memo()).unwrap())
                .collect();
            rows.push(values);
        }
        rows
    }

    #[test]
    fn values_read_back_as_built() {
        let columns = [
            column("c", FieldType::Character, 5, 0),
            column("ci", FieldType::CiCharacter, 3, 0),
            column("v", FieldType::VarChar, 6, 0),
            column("nc", FieldType::NChar, 3, 0),
            column("nv", FieldType::NVarChar, 4, 0),
            column("m", FieldType::Memo, 9, 0),
            column("nm", FieldType::NMemo, 9, 0),
            column("b", FieldType::Binary, 9, 0),
            column("r", FieldType::Raw, 3, 0),
            column("vb", FieldType::VarBinary, 4, 0),
            column("l", FieldType::Logical, 1, 0),
            column("n", FieldType::Numeric, 8, 2),
            column("s", FieldType::ShortInt, 2, 0),
            column("i", FieldType::Integer, 4, 0),
            column("li", FieldType::LongInt, 8, 0),
            column("d", FieldType::Double, 8, 15),
            column("mo", FieldType::Money, 8, 4),
            column("da", FieldType::Date, 4, 0),
            column("t", FieldType::Time, 4, 0),
            column("ts", FieldType::TimeStamp, 8, 0),
            column("g", FieldType::Guid, 16, 0),
        ];
        let full = vec![
            Value::Text("ab   ".into()),
            Value::Text("xyz".into()),
            Value::Text("var".into()),
            Value::Text("äb ".into()),
            Value::Text("€".into()),
            Value::Text("a memo longer than a block".into()),
            Value::Text("ünï".into()),
            Value::Binary(vec![0, 1, 2]),
            Value::Binary(vec![9, 8, 7]),
            Value::Binary(vec![1, 2]),
            Value::Logical(true),
            Value::Numeric("   -1.50".into()),
            Value::Integer(-2),
            Value::Integer(123456789),
            Value::Integer(-1234567890123),
            Value::Double(2.5),
            Value::Money(123456),
            Value::Date(2458066),
            Value::Time(49530000),
            Value::TimeStamp(2458066, 1),
            Value::Guid([7; 16]),
        ];
        let empty = vec![
            Value::Text("     ".into()),
            Value::Text("   ".into()),
            Value::Text(String::new()),
            Value::Text("   ".into()),
            Value::Text(String::new()),
            Value::Text(String::new()),
            Value::Text(String::new()),
            Value::Binary(Vec::new()),
            // All zeros would be NULL, as in tables on disk.
            Value::Binary(vec![0, 0, 1]),
            Value::Binary(Vec::new()),
            Value::Logical(false),
            Value::Numeric("    0.00".into()),
            Value::Integer(0),
            Value::Integer(0),
            Value::Integer(0),
            Value::Double(0.0),
            Value::Money(0),
            Value::Date(1),
            Value::Time(0),
            Value::TimeStamp(1, 0),
            Value::Guid([1; 16]),
        ];
        let null = vec![Value::Null; columns.len()];
        let rows = vec![full, null, empty];
        let mut table = build(&columns, &rows).unwrap();
        assert_eq!(table.structure.fields[3].length(), 3);
        assert_eq!(table.structure.fields[4].length(), 4);
        assert_eq!(table.structure.fields[11].decimals(), 2);
        // Empty memo values are NULL.
        let mut expected = rows.clone();
        expected[2][5..8].fill(Value::Null);
        assert_eq!(read_back(&mut table), expected);
    }

    #[test]
    fn values_are_cut_to_fit() {
        let columns = [
            column("c", FieldType::Character, 2, 0),
            column("v", FieldType::VarChar, 2, 0),
        ];
        let rows = vec![vec![Value::Text("abc".into()), Value::Text("abc".into())]];
        let mut table = build(&columns, &rows).unwrap();
        assert_eq!(
            read_back(&mut table),
            vec![vec![Value::Text("ab".into()), Value::Text("ab".into())]]
        );
    }

    #[test]
    fn mismatched_values_are_refused() {
        let columns = [column("i", FieldType::Integer, 4, 0)];
        assert!(build(&columns, &[vec![Value::Text("1".into())]]).is_err());
        assert!(build(&columns, &[vec![]]).is_err());
    }

    #[test]
    fn empty_tables_have_their_fields() {
        let mut table = build(&[column("i", FieldType::Integer, 4, 0)], &[]).unwrap();
        assert_eq!(table.slots(), 0);
        assert_eq!(table.structure.fields[0].name, "i");
        assert!(read_back(&mut table).is_empty());
    }
}
