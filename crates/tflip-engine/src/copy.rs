//! Copying tables (`AdsCopyTableStructure`, `AdsCopyTable`,
//! `AdsCopyTableContents`), as the original engine does it
//! (`fixtures/write/copies`): a copy is a new table of the same type and
//! fields, as `AdsCreateTable` creates it, holding the records the filter
//! option takes, in record order or an order's, byte for byte (an ADT
//! table's AutoInc values and next value kept), their memo values written
//! anew to the new memo file record by record (an ADT table's of 8-byte
//! blocks). Index files are not copied.
//!
//! Converting a table (`AdsConvertTable`) makes a table of another type (or
//! of its own) whose fields take the closest types it has, and sets each
//! record's values in it as a client would (see [`convert_table`]).

use std::path::PathBuf;
use std::sync::Arc;

use tflip_storage::create::{self, FieldSpec};
use tflip_storage::field::{Field, FieldType};
use tflip_storage::table::{Format, TableFile};
use tflip_storage::value::Value;

use crate::handle::Handle;
use crate::index::Index;
use crate::setters::SetValue;
use crate::table::{self, FieldRef, Table, TableType};
use crate::{Error, Session, index};

/// The table `handle` names (a table, a cursor, or an index order's table),
/// and the order if it is one.
fn source(session: &Session, handle: Handle) -> Result<(Arc<Table>, Option<Arc<Index>>), Error> {
    match index::get(session, handle) {
        Ok(order) => Ok((table::get(session, order.table)?, Some(order))),
        Err(_) => Ok((table::get(session, handle)?, None)),
    }
}

/// The file `name` names for a table of `connection`: in its folder if it
/// is relative.
fn file(session: &Session, connection: Handle, name: &str) -> Result<PathBuf, Error> {
    match connection {
        0 => Ok(PathBuf::from(name)),
        _ => crate::ddl::path_in(session, connection, name),
    }
}

/// Copies the table `handle` (or an order's, or a cursor) to the new file
/// `name` (`AdsCopyTable`): with `structure_only`, without records
/// (`AdsCopyTableStructure`); else with those `filter` takes (see
/// `Table::copy_slots`).
pub fn copy_table(
    session: &Session,
    handle: Handle,
    filter: u16,
    name: &str,
    structure_only: bool,
) -> Result<(), Error> {
    let (table, order) = source(session, handle)?;
    let path = file(session, table.connection, name)?;
    if path.exists() {
        return Err(Error::FileCreate);
    }
    let fields: Vec<Field> = table.fields().to_vec();
    let block_size = table.memo_block_size();
    let format = table.structure.header.format;
    match format {
        Format::Adt => {
            let specs: Vec<FieldSpec> = fields.iter().map(FieldSpec::from).collect();
            // As in the original engine, the copy's memo blocks are 8 bytes,
            // whatever the source's.
            // The copy of an AES table is laid out for AES, in the clear
            // (`fixtures/aes/rc4tp`, `v14`).
            match table.file_is_aes() {
                true => create::aes_adt_table(&path, &specs, 8),
                false => create::adt_table(&path, &specs, 8),
            }
            .map_err(|_| Error::FileCreate)?;
        }
        Format::Dbf(flavour) => {
            let specs: Vec<create::DbfFieldSpec> =
                fields.iter().map(create::DbfFieldSpec::from).collect();
            let today = tflip_storage::value::julian_to_ymd(crate::clock::now().local.0);
            // The copy of a VFP table laid out for AES is laid out so, in the
            // clear (`fixtures/aes/rc4tp`, `v13`, `v14`).
            let aes = table.dbf_aes_layout();
            let (bytes, memo) = create::dbf_bytes_layout(flavour, &specs, today, false, aes);
            std::fs::write(&path, &bytes).map_err(|_| Error::FileCreate)?;
            if memo {
                let (kind, bytes) = match flavour {
                    tflip_storage::dbf::Flavour::Clipper => {
                        (tflip_storage::memo::MemoKind::Dbt, create::dbt_bytes())
                    }
                    _ => (
                        tflip_storage::memo::MemoKind::Fpt,
                        create::fpt_bytes(block_size.unwrap_or(64) as u16),
                    ),
                };
                std::fs::write(tflip_storage::dbf::memo_path(&path, kind), bytes)
                    .map_err(|_| Error::FileCreate)?;
            }
        }
    }
    if structure_only {
        return Ok(());
    }
    let slots = table.copy_slots(order.as_deref(), filter)?;
    let mut dest = TableFile::open_for_writing(&path, format).map_err(|_| Error::FileCreate)?;
    let new: Vec<Field> = dest.structure.fields.clone();
    let memo_fields: Vec<&Field> = new.iter().filter(|f| f.is_memo()).collect();
    // A handle without the password of an AES table copies what it reads: blank
    // records, no memo values. Of an RC4 DBF table it copies the enciphered
    // bytes, and fails at the first memo value it cannot read with the record
    // appended but for its memo pointers (`fixtures/aes/rc4tp`, `v14`).
    let sealed = table.password_missing();
    for slot in slots {
        if sealed && !table.file_is_aes() && format != Format::Adt {
            let mut bytes = table.slot_record(slot)?;
            let source: Vec<&Field> = fields.iter().filter(|f| f.is_memo()).collect();
            let referred = source
                .iter()
                .any(|f| f.memo_ref(&bytes).is_some_and(|memo| memo.block != 0));
            for field in &memo_fields {
                field
                    .encode_blank(&mut bytes)
                    .map_err(|_| Error::CorruptTable)?;
            }
            if referred {
                bytes[0] = match bytes[0] {
                    b'D' => b'*',
                    _ => b' ',
                };
                dest.insert(&bytes).map_err(|_| Error::CorruptTable)?;
                return Err(Error::EncryptionNotEnabled);
            }
        }
        let (mut bytes, memos) = table.slot_bytes(slot)?;
        let memos = match sealed && table.file_is_aes() {
            true => vec![Vec::new(); memos.len()],
            false => memos,
        };
        for (field, value) in memo_fields.iter().zip(memos) {
            field
                .encode_blank(&mut bytes)
                .map_err(|_| Error::CorruptTable)?;
            if !value.is_empty() {
                let memo = dest.memo().ok_or(Error::CorruptTable)?;
                field
                    .store_memo(Some(&value), &mut bytes, memo)
                    .map_err(|_| Error::CorruptMemo)?;
            }
        }
        dest.insert(&bytes).map_err(|_| Error::CorruptTable)?;
    }
    // An ADT table's next AutoInc value goes along.
    if format == Format::Adt {
        let header = std::fs::read(table.path()).map_err(|_| Error::CorruptTable)?;
        let next = u32::from_le_bytes(header[0x50..0x54].try_into().unwrap());
        dest.set_next_autoinc(next)
            .map_err(|_| Error::CorruptTable)?;
    }
    Ok(())
}

/// A value read from a field, as a client would set it in another.
fn set_value(value: Value) -> SetValue {
    match value {
        Value::Null => SetValue::Null,
        Value::Blank => SetValue::Empty,
        Value::Text(text) => SetValue::String(text),
        Value::Binary(bytes) => SetValue::Binary(bytes),
        Value::Logical(b) => SetValue::Logical(b),
        Value::Numeric(text) => SetValue::Field(text.trim().to_owned()),
        Value::Integer(n) => SetValue::LongLong(n as i64),
        Value::Double(d) => SetValue::Double(d),
        Value::Money(m) => SetValue::Money(m),
        Value::Date(day) => SetValue::Julian(day),
        Value::Time(ms) => SetValue::Milliseconds(ms as i32),
        Value::TimeStamp(day, ms) => SetValue::TimeStampRaw(day, ms),
        Value::Guid(bytes) => SetValue::Binary(bytes.to_vec()),
    }
}

/// Appends the records of `from` (a table, an order's table or a cursor)
/// that `filter` takes to the table `to` (`AdsCopyTableContents`), each
/// field set from the source field of its name, as a client would append
/// and set them; a table cannot be copied onto itself.
pub fn copy_contents(
    session: &Session,
    from: Handle,
    to: Handle,
    filter: u16,
) -> Result<(), Error> {
    let (table, order) = source(session, from)?;
    let dest = table::get(session, to)?;
    if Arc::ptr_eq(&table, &dest) {
        return Err(Error::InvalidTable);
    }
    let names: Vec<String> = dest.fields().iter().map(|f| f.name.clone()).collect();
    let sources: Vec<Option<usize>> = names
        .iter()
        .map(|name| {
            table
                .fields()
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(name))
        })
        .collect();
    for slot in table.copy_slots(order.as_deref(), filter)? {
        let values = table.slot_values(slot)?;
        dest.append()?;
        for (name, source) in names.iter().zip(&sources) {
            let Some(i) = source else { continue };
            let value = set_value(values[*i].clone());
            if value == SetValue::Null {
                continue;
            }
            match dest.set_field(FieldRef::Name(name), &value) {
                Ok(()) | Err(Error::DataTruncated) => {}
                Err(error) => return Err(error),
            }
        }
        dest.write_record()?;
    }
    Ok(())
}

/// Which part of a source value a converted field takes.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Part {
    Whole,
    /// A TimeStamp's date, in a Date field.
    Date,
    /// A TimeStamp's time of day in milliseconds, in a Numeric field.
    Time,
}

/// A field of a converted table: its definition and where its values come
/// from.
struct Target {
    spec: create::DbfFieldSpec,
    source: usize,
    part: Part,
}

/// The fields a table of `fields` (of `from`, the source's format) converts
/// to in a table of `to`, as the original engine converts them
/// (`fixtures/write/converts`, `converts_ext`, `converts_dbf` and
/// `converts_values`).
///
/// A Clipper, FoxPro or VFP table converted to its own kind keeps its fields
/// as they are, VFP's flags and AutoInc state too. Else each field takes the
/// type closest to its own that the new table has: to a Clipper or FoxPro
/// table, numbers become Numeric fields (AutoInc N10, Integer N11, ShortInt
/// N6, LongInt N20, RowVersion N19, Money N23.4, Time N10 of milliseconds, a
/// Double or CurDouble 15 digits wider than its decimals, up to 30), a
/// TimeStamp or ModTime a Date and a Numeric of milliseconds, a GUID a
/// 36-character string, raw and variable-length values Character fields as
/// wide as they are stored, an ADS VarChar, an NVarChar and a Character wider
/// than 254 memos, a short date a Date; to a VFP table likewise, but for the
/// types VFP has (Integer, AutoInc, Double, Money, TimeStamp, VarChar,
/// VarBinary). To an ADT table a field keeps its type (a short date becomes
/// a Date); a DBF Numeric is a digit wider, and a variable-length field as
/// long as its source is stored (an NVarChar a character more). A General or
/// Picture field converts only to its own kind of table
/// ([`Error::InvalidFieldDefinition`], 5012).
///
/// In a Clipper or FoxPro table a name longer than ten characters becomes
/// `FIELD<n>`, n the field's place, as does the time part of a TimeStamp;
/// names are in upper case but in VFP tables. A VFP field takes NULL when its
/// source can hold it: any field of an ADT table but an AutoInc, RowVersion
/// or ModTime, and the LongInt, NChar, NVarChar, NMemo and GUID fields of a
/// Clipper or FoxPro table.
fn targets(
    table: &Table,
    fields: &[Field],
    from: Format,
    to: Format,
) -> Result<Vec<Target>, Error> {
    use FieldType::*;
    if from == to && from != Format::Adt {
        let mut out = Vec::with_capacity(fields.len());
        for (i, field) in fields.iter().enumerate() {
            let mut spec = create::DbfFieldSpec::from(field);
            if field.field_type == AutoInc {
                spec.autoinc = table.autoinc_state(field)?;
            }
            out.push(Target {
                spec,
                source: i,
                part: Part::Whole,
            });
        }
        return Ok(out);
    }
    let from_adt = from == Format::Adt;
    let (adt, visual) = match to {
        Format::Adt => (true, false),
        Format::Dbf(flavour) => (false, flavour == tflip_storage::dbf::Flavour::Visual),
    };
    // A Clipper or FoxPro table: neither ADT nor VFP.
    let plain = !adt && !visual;
    let next_autoinc = match from_adt {
        true => {
            let header = std::fs::read(table.path()).map_err(|_| Error::CorruptTable)?;
            u32::from_le_bytes(header[0x50..0x54].try_into().unwrap())
        }
        false => 1,
    };
    let mut out: Vec<Target> = Vec::new();
    for (i, field) in fields.iter().enumerate() {
        let place = out.len() + 1;
        let name = match to {
            Format::Adt => field.name.clone(),
            _ if field.name.chars().count() > 10 => format!("FIELD{place}"),
            _ if visual => field.name.clone(),
            _ => field.name.to_ascii_uppercase(),
        };
        let spec = |field_type, length: u16, decimals: u16| FieldSpec {
            name: name.clone(),
            field_type,
            length,
            decimals,
            compressed: false,
        };
        let (length, decimals, size) = (field.length(), field.decimals(), field.size);
        let mut split = false;
        let made = match field.field_type {
            CiCharacter if adt => spec(CiCharacter, length, 0),
            Character | CiCharacter if !adt && size > 254 => spec(Memo, 10, 0),
            Character | CiCharacter => spec(Character, length, 0),
            Numeric if adt && !from_adt => spec(Numeric, length + 1, decimals),
            Numeric => spec(Numeric, length, decimals),
            Date | CompactDate => spec(Date, 8, 0),
            Logical => spec(Logical, 1, 0),
            Memo | Binary | Image | NMemo => spec(field.field_type, 10, 0),
            LegacyVarChar if adt => spec(LegacyVarChar, size, 0),
            LegacyVarChar => spec(Memo, 10, 0),
            Double | CurDouble if adt => spec(field.field_type, 8, decimals),
            Double | CurDouble if visual => spec(Double, 8, decimals),
            Double | CurDouble => spec(Numeric, (15 + decimals).min(30), decimals),
            Integer if !plain => spec(Integer, 4, 0),
            Integer => spec(Numeric, 11, 0),
            AutoInc if !plain => spec(AutoInc, 4, 0),
            AutoInc => spec(Numeric, 10, 0),
            ShortInt if adt => spec(ShortInt, 2, 0),
            ShortInt => spec(Numeric, 6, 0),
            Money if !plain => spec(Money, 8, 4),
            Money => spec(Numeric, 23, 4),
            Time if adt => spec(Time, 4, 0),
            Time => spec(Numeric, 10, 0),
            TimeStamp | ModTime if adt => spec(field.field_type, 8, 0),
            TimeStamp | ModTime if visual => spec(TimeStamp, 8, 0),
            TimeStamp | ModTime => {
                split = true;
                spec(Date, 8, 0)
            }
            LongInt if adt => spec(LongInt, 8, 0),
            LongInt => spec(Numeric, 20, 0),
            RowVersion if adt => spec(RowVersion, 8, 0),
            RowVersion => spec(Numeric, 19, 0),
            Guid if adt => spec(Guid, 16, 0),
            Guid => spec(Character, 36, 0),
            Raw if adt => spec(Raw, size, 0),
            VarChar | VarBinary if !plain => spec(field.field_type, size, 0),
            Raw | VarChar | VarBinary => spec(Character, size, 0),
            NChar => spec(NChar, size / 2, 0),
            NVarChar if adt => spec(NVarChar, size / 2, 0),
            NVarChar => spec(NMemo, 10, 0),
            Other(_) => {
                return Err(Error::InvalidFieldDefinition(
                    "A General or Picture field converts to a table of its own kind only."
                        .to_owned(),
                ));
            }
        };
        let nullable = visual
            && match from_adt {
                true => !matches!(field.field_type, AutoInc | RowVersion | ModTime),
                false => matches!(field.field_type, LongInt | NChar | NVarChar | NMemo | Guid),
            };
        // Raw bytes in a VFP Character field are not translated.
        let binary = visual && field.field_type == Raw;
        let autoinc = (visual && made.field_type == AutoInc).then_some((next_autoinc, 1));
        let part = if split { Part::Date } else { Part::Whole };
        out.push(Target {
            spec: create::DbfFieldSpec {
                spec: made,
                nullable,
                binary,
                autoinc,
            },
            source: i,
            part,
        });
        if split {
            let name = format!("FIELD{}", out.len() + 1);
            let time = FieldSpec {
                name,
                field_type: Numeric,
                length: 10,
                decimals: 0,
                compressed: false,
            };
            out.push(Target {
                spec: create::DbfFieldSpec {
                    spec: time,
                    nullable,
                    binary: false,
                    autoinc: None,
                },
                source: i,
                part: Part::Time,
            });
        }
    }
    Ok(out)
}

/// What the converted field `to` takes of `value`, its source's value.
fn converted(to: &Field, part: Part, value: Value) -> Option<CellValue> {
    use FieldType::*;
    if value == Value::Null {
        return None;
    }
    Some(match (part, value) {
        // A blank Clipper or FoxPro value (a date, a logical, a VFP
        // TimeStamp) is the empty value.
        (_, Value::Blank) => CellValue::Set(SetValue::Empty),
        (_, Value::Text(text)) if to.field_type == LegacyVarChar => {
            CellValue::VarChar(tflip_storage::cp1252::encode(&text))
        }
        (Part::Date, Value::TimeStamp(day, _)) => CellValue::Value(Value::Date(day)),
        (Part::Time, Value::TimeStamp(_, ms)) => CellValue::Set(SetValue::Long(ms as i32)),
        // A Unicode memo keeps its text as UTF-16; an 8-bit memo as cp1252.
        (_, Value::Text(text)) if to.is_memo() => {
            CellValue::Memo(tflip_storage::field::memo_text_bytes(to.field_type, &text))
        }
        (_, Value::Binary(bytes)) if to.is_memo() => CellValue::Memo(bytes),
        // A GUID widened to a Character field becomes its 36-character text; raw
        // and variable-binary bytes become the character field's bytes.
        (_, Value::Guid(g)) if to.field_type == Character => {
            CellValue::Set(SetValue::Field(Value::guid_text(&g)))
        }
        (_, Value::Binary(bytes)) if to.field_type == Character => {
            CellValue::Set(SetValue::Raw(bytes))
        }
        (_, Value::Text(text)) => CellValue::Set(SetValue::String(text)),
        (_, Value::Numeric(text)) => CellValue::Set(SetValue::Field(text.trim().to_owned())),
        (_, Value::Time(ms)) if to.field_type == Numeric => {
            CellValue::Set(SetValue::Long(ms as i32))
        }
        (_, Value::Integer(n)) if to.field_type == Numeric => {
            CellValue::Set(SetValue::LongLong(n as i64))
        }
        (_, Value::Double(d)) if to.field_type == Numeric => CellValue::Set(SetValue::Double(d)),
        (_, Value::Money(m)) if to.field_type == Numeric => CellValue::Set(SetValue::Field(
            crate::setters::numeric_text(i128::from(m), 4),
        )),
        (_, value) => CellValue::Value(value),
    })
}

/// A converted value: as stored, as a client sets it, a memo value, or an
/// ADS VarChar's text.
enum CellValue {
    Value(Value),
    Set(SetValue),
    Memo(Vec<u8>),
    VarChar(Vec<u8>),
}

/// Converts the table `handle` (an order's, a cursor) to the table `name`
/// of `table_type` (`AdsConvertTable`), with the records `filter` takes, as
/// the original engine does (see [`targets`]). A file of that name is
/// overwritten; index files are not made.
///
/// Each record is appended and its fields set in turn, as a client would: a
/// value its field cannot hold (a number too wide for its Numeric field,
/// 5179) ends the conversion with that error, the record written as far as
/// it got. A deleted record stays deleted in a Clipper, FoxPro or VFP table,
/// and an ADT table leaves it out. A Clipper table's General or Picture value
/// cannot be read (`AE_CORRUPT_MEMO`, 7019), which ends the conversion
/// likewise.
pub fn convert_table(
    session: &Session,
    handle: Handle,
    filter: u16,
    name: &str,
    table_type: TableType,
) -> Result<(), Error> {
    use tflip_storage::dbf::Flavour;
    let (table, order) = source(session, handle)?;
    let path = file(session, table.connection, name)?;
    let fields: Vec<Field> = table.fields().to_vec();
    let source_format = table.structure.header.format;
    let format = match table_type {
        TableType::Adt => Format::Adt,
        TableType::Ntx => Format::Dbf(Flavour::Clipper),
        TableType::Cdx => Format::Dbf(Flavour::FoxPro),
        TableType::Vfp => Format::Dbf(Flavour::Visual),
        TableType::Database => return Err(Error::InvalidTableType),
    };
    // As the original engine words it: where in its source it refused.
    let targets = targets(&table, &fields, source_format, format).map_err(|error| match error {
        Error::InvalidFieldDefinition(_) => Error::InvalidFieldDefinition(
            match format {
                Format::Adt => ".\\ctbldata.cpp, 2725",
                Format::Dbf(_) => ".\\ctbldata.cpp, 3051",
            }
            .to_owned(),
        ),
        error => error,
    })?;
    match format {
        Format::Adt => {
            let specs: Vec<FieldSpec> = targets.iter().map(|t| t.spec.spec.clone()).collect();
            let _ = std::fs::remove_file(&path);
            create::adt_table(&path, &specs, 8).map_err(|_| Error::FileCreate)?;
        }
        Format::Dbf(flavour) => {
            let specs: Vec<create::DbfFieldSpec> = targets.iter().map(|t| t.spec.clone()).collect();
            let today = tflip_storage::value::julian_to_ymd(crate::clock::now().local.0);
            let (bytes, memo) = create::dbf_bytes(flavour, &specs, today);
            std::fs::write(&path, &bytes).map_err(|_| Error::FileCreate)?;
            if memo {
                let (kind, bytes) = match flavour {
                    Flavour::Clipper => (tflip_storage::memo::MemoKind::Dbt, create::dbt_bytes()),
                    _ => (tflip_storage::memo::MemoKind::Fpt, create::fpt_bytes(64)),
                };
                std::fs::write(tflip_storage::dbf::memo_path(&path, kind), bytes)
                    .map_err(|_| Error::FileCreate)?;
            }
        }
    }
    let date_format = table.date_format();
    let dates = crate::setters::Dates {
        format: &date_format,
        epoch: table.session_settings().epoch(),
    };
    let mut dest = TableFile::open_for_writing(&path, format).map_err(|_| Error::FileCreate)?;
    let new: Vec<Field> = dest.structure.fields.clone();
    let nulls = matches!(source_format, Format::Adt | Format::Dbf(Flavour::Visual));
    // A Clipper or FoxPro table: one without NULL.
    let plain = matches!(format, Format::Dbf(Flavour::Clipper | Flavour::FoxPro));
    for slot in table.copy_slots(order.as_deref(), filter)? {
        let deleted = table.slot_deleted(slot)?;
        if deleted && format == Format::Adt {
            continue;
        }
        let values = table.slot_values(slot)?;
        let bytes = table.slot_record(slot)?;
        let mut record = dest.structure.null_record();
        let mut failed = None;
        // Text memos go to the memo file after the record's other values, as
        // the original engine writes them when the record is.
        let mut texts: Vec<(&Field, Vec<u8>)> = Vec::new();
        for (target, field) in targets.iter().zip(&new) {
            let from = &fields[target.source];
            let cell = match from.field_type {
                // An ADT table gives each record a RowVersion value of its
                // own, as on any append; a DBF field takes the source's.
                FieldType::RowVersion if format == Format::Adt => {
                    let version = dest.take_row_version().map_err(|_| Error::CorruptTable)?;
                    Some(CellValue::Value(Value::Integer(i128::from(version))))
                }
                FieldType::Other(_) => {
                    // A Clipper table's General or Picture value cannot be
                    // read.
                    let object = table.slot_object(slot, target.source)?;
                    if object.is_some() && source_format == Format::Dbf(Flavour::Clipper) {
                        failed = Some(Error::CorruptMemo);
                        break;
                    }
                    object.map(CellValue::Memo)
                }
                // A NULL value is set NULL, and an empty one (a blank Clipper
                // or FoxPro field, a VFP field of zeros) empty (`AdsIsNull`,
                // `AdsIsEmpty`). A Clipper or FoxPro table has no NULL but
                // the blank values the getters report as NULL: a blank field
                // is empty, and only one blank but not empty (a LongInt) is
                // NULL.
                _ if nulls && from.is_null(&bytes) => None,
                _ if from.is_empty(&bytes) => Some(CellValue::Set(SetValue::Empty)),
                _ if from.is_null(&bytes) => None,
                _ => converted(field, target.part, values[target.source].clone()),
            };
            let Some(cell) = cell else {
                // NULL, which an ADT field holds and a VFP field that takes it
                // flags (a VFP one without NULL stays blank). A Clipper or
                // FoxPro field takes the blank value it reads as NULL (zeros
                // in an NChar or NMemo) from a VFP table, but stays blank
                // from an ADT one (as probed).
                if !(plain && source_format == Format::Adt) {
                    let _ = field.encode(&Value::Null, &mut record);
                }
                continue;
            };
            let set = match cell {
                CellValue::Value(value) => field
                    .encode(&value, &mut record)
                    .map_err(|_| Error::InvalidFieldType),
                CellValue::Set(value) => {
                    match crate::setters::apply(field, &value, dates, &mut record) {
                        Ok(()) | Err(Error::DataTruncated) => Ok(()),
                        Err(error) => Err(error),
                    }
                }
                CellValue::Memo(bytes)
                    if matches!(field.field_type, FieldType::Memo | FieldType::NMemo) =>
                {
                    texts.push((field, bytes));
                    Ok(())
                }
                CellValue::Memo(bytes) => {
                    let memo = dest.memo().ok_or(Error::CorruptTable)?;
                    field
                        .store_memo(Some(&bytes), &mut record, memo)
                        .map_err(|_| Error::CorruptMemo)
                }
                CellValue::VarChar(bytes) => field
                    .store_var_char(&bytes, &mut record, dest.memo())
                    .map_err(|_| Error::CorruptMemo),
            };
            if let Err(error) = set {
                // As the original engine names the field of a value too big for it.
                failed = Some(match error {
                    Error::ValueOverflow => error.detailed(format!("Field: \"{}\"", field.name)),
                    error => error,
                });
                break;
            }
        }
        for (field, bytes) in texts {
            let memo = dest.memo().ok_or(Error::CorruptTable)?;
            field
                .store_memo(Some(&bytes), &mut record, memo)
                .map_err(|_| Error::CorruptMemo)?;
        }
        if deleted {
            record[0] = b'*';
        }
        dest.insert(&record).map_err(|_| Error::CorruptTable)?;
        if let Some(error) = failed {
            return Err(error);
        }
    }
    // An ADT table's next AutoInc value goes along, as does a VFP table's.
    if format == Format::Adt {
        let next = match source_format {
            Format::Adt => {
                let header = std::fs::read(table.path()).map_err(|_| Error::CorruptTable)?;
                Some(u32::from_le_bytes(header[0x50..0x54].try_into().unwrap()))
            }
            _ => match fields.iter().find(|f| f.field_type == FieldType::AutoInc) {
                Some(field) => table.autoinc_state(field)?.map(|(next, _)| next),
                None => None,
            },
        };
        if let Some(next) = next {
            dest.set_next_autoinc(next)
                .map_err(|_| Error::CorruptTable)?;
        }
    }
    Ok(())
}
