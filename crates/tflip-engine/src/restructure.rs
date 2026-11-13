//! Changing a free ADT table's fields (`AdsRestructureTable`, `ALTER
//! TABLE`), as the original engine does it (`fixtures/write/restructure`):
//! the table is written anew with the new fields, its records that are not
//! deleted copied over one by one with their values converted, and its
//! structural index file rebuilt with the orders it had, their expressions
//! following the fields; the old files stay as `<file>.BAK`.
//!
//! A value goes to a field of another type as the original engine converts
//! it (probed for every pair of the common types):
//!
//! - NULL stays NULL;
//! - a number (ShortInt, Integer, Double, Numeric, Money, and a Logical as 1
//!   or 0) to a numeric field as `AdsGetLong`, `AdsGetShort` (cut to 16
//!   bits) or `AdsGetDouble` read it, a value too large failing with
//!   `AE_VALUE_OVERFLOW`;
//! - text (Character, CICharacter, VarChar, Memo, NMemo) to any field, and
//!   numbers and Logicals to the fields that are not numeric, as its text
//!   (`AdsGetField`'s) is set with `AdsSetField`, failing as that fails
//!   (`AE_INVALID_DATA_TYPE` for text that is no number for a Numeric or
//!   Money field or no logical, `AE_INVALID_DATE`, `AE_INVALID_VALUE` for a
//!   time of a day or more);
//! - a Date, TimeStamp or Time to one of the others but Date and Time, a
//!   Time to a TimeStamp being NULL;
//! - anything else (a Double or a date to text, ...) fails with
//!   `AE_INVALID_FIELD_TYPE`.
//!
//! An AutoInc keeps a whole number's value, and gives a record without one
//! its place among the records; the next value follows the largest. A
//! RowVersion counts the records; a new ModTime is NULL.
//!
//! A conversion that fails leaves the table as it was.

use std::path::{Path, PathBuf};

use tflip_sql::ast::FieldRules;
use tflip_storage::create::{self, FieldSpec, OrderSpec};
use tflip_storage::field::{Field, FieldType};
use tflip_storage::order::{IndexFile, Tag};
use tflip_storage::table::{Format, TableFile};
use tflip_storage::value::Value;

use crate::format::{FieldText, Layout};
use crate::handle::Handle;
use crate::setters::{self, Dates, SetValue};
use crate::table::{self, TableSettings, TableType, options};
use crate::{Error, Session, index};

/// What changes: fields to add, delete and change, and the memo block size
/// (0 keeps the table's).
pub struct Changes {
    pub add: Vec<Added>,
    pub delete: Vec<String>,
    pub change: Vec<Changed>,
    pub block_size: u32,
    /// The table's new transaction-free mark (`ALTER TABLE … IGNORE |
    /// RESPECT TRANSACTIONS`); the table keeps its own if `None`.
    pub transaction_free: Option<bool>,
    /// Whether a restructure that changes nothing fails, as `ALTER TABLE`'s
    /// does.
    pub require_change: bool,
    /// The dictionary object id and file the restructured table is bound to,
    /// for a table of a dictionary.
    pub bind: Option<(u32, String)>,
    /// The index files of a dictionary's table besides its structural one
    /// (`IN FILE`), whose orders follow the fields too.
    pub indexes: Vec<PathBuf>,
    /// The key expression and condition the dictionary has for each order,
    /// by its index file: a Clipper table's own files may say the names the
    /// fields had before they were renamed where they were.
    pub expressions: Vec<(PathBuf, String, String)>,
    /// The Key objects of a dictionary's table, which a restructure that
    /// touches the fields the orders use makes them all from again
    /// (`fixtures/write/restructure_fts_dict_after`).
    pub keys: Vec<KeyObject>,
}

/// What the dictionary says of one order: its Key object, with the file's
/// path in the folder.
#[derive(Debug, Clone)]
pub struct KeyObject {
    pub id: u32,
    pub name: String,
    pub file: PathBuf,
    pub expression: String,
    pub condition: String,
    /// The `ADS_*` index options, a full-text order's among them.
    pub options: u32,
    pub key_length: u16,
    pub key_type: u16,
    pub collation: String,
    pub min_word: u16,
    /// A full-text order's delimiters, noise words, drop and conditional
    /// characters, as the dictionary has them.
    pub lists: [Vec<u8>; 4],
}

/// What a restructure of an ADT table did to its fields.
pub struct Outcome {
    /// For each field the table has now, the number of the old field it came
    /// from; none for one that was added.
    pub sources: Vec<Option<usize>>,
    /// Whether the fields were only named again, which changes the table's
    /// header alone.
    pub renamed: bool,
}

/// A field to add: its definition, the position it asks for (1-based), and
/// its definition as the client wrote it, for errors.
#[derive(Clone)]
pub struct Added {
    pub spec: FieldSpec,
    pub position: Option<usize>,
    pub text: String,
    /// A DBF field's own definition (VFP's `NULL` and `NOCPTRANS`).
    pub dbf: Option<create::DbfFieldSpec>,
    /// The constraints the field is given, for a table of a dictionary.
    pub rules: FieldRules,
}

/// A field to change: its name now, its new definition and the position it
/// asks for.
#[derive(Clone)]
pub struct Changed {
    pub from: String,
    pub spec: FieldSpec,
    pub position: Option<usize>,
    /// A DBF field's own definition.
    pub dbf: Option<create::DbfFieldSpec>,
    /// The definition as the client wrote it, for errors.
    pub text: String,
    /// The constraints the field is given, for a table of a dictionary.
    pub rules: FieldRules,
}

impl Changes {
    /// `AdsRestructureTable`'s lists: definitions as `AdsCreateTable` takes
    /// them (`change`'s each after the field's name), maybe ending in
    /// `,(n)`, and names, all separated by `;`.
    pub fn parse(
        add: &str,
        delete: &str,
        change: &str,
        block_size: u32,
        table_type: TableType,
    ) -> Result<Changes, Error> {
        use tflip_storage::dbf::Flavour;
        let flavour = match table_type {
            TableType::Ntx => Some(Flavour::Clipper),
            TableType::Cdx => Some(Flavour::FoxPro),
            TableType::Vfp => Some(Flavour::Visual),
            _ => None,
        };
        let spec = |definition: &str| -> Result<(FieldSpec, Option<create::DbfFieldSpec>), Error> {
            if let Some(flavour) = flavour {
                let mut specs = crate::ddl::dbf_field_specs(definition, flavour)?;
                let dbf = specs
                    .pop()
                    .ok_or(Error::InvalidFieldDefinition(String::new()))?;
                return Ok((dbf.spec.clone(), Some(dbf)));
            }
            let (mut specs, _) = crate::ddl::field_specs(definition)?;
            Ok((
                specs
                    .pop()
                    .ok_or(Error::InvalidFieldDefinition(String::new()))?,
                None,
            ))
        };
        let mut changes = Changes {
            add: Vec::new(),
            delete: Vec::new(),
            change: Vec::new(),
            block_size,
            transaction_free: None,
            require_change: false,
            bind: None,
            indexes: Vec::new(),
            expressions: Vec::new(),
            keys: Vec::new(),
        };
        for definition in definitions(change) {
            let (definition, position) = with_position(definition);
            let (from, rest) = definition.split_once(',').unwrap_or((&definition, ""));
            let (spec, dbf) = spec(rest)?;
            changes.change.push(Changed {
                from: from.trim().to_owned(),
                spec,
                position,
                dbf,
                text: definition.trim().to_owned(),
                rules: FieldRules::default(),
            });
        }
        for definition in definitions(add) {
            let (definition, position) = with_position(definition);
            // The original engine shows an ADT table's definition to add
            // with the `;` that ends it (`fixtures/write/restructure`).
            let (spec, dbf) = spec(&definition).map_err(|error| match error {
                Error::InvalidFieldDefinition(why) if flavour.is_none() => {
                    let shown = format!("\"{definition}\"");
                    let ended = format!("\"{definition};\"");
                    Error::InvalidFieldDefinition(why.replace(&shown, &ended))
                }
                error => error,
            })?;
            let text = definition.trim().to_owned();
            changes.add.push(Added {
                spec,
                position,
                text,
                dbf,
                rules: FieldRules::default(),
            });
        }
        changes.delete = definitions(delete)
            .map(|name| name.trim().to_owned())
            .collect();
        Ok(changes)
    }
}

impl Changes {
    /// The transaction-free mark the restructured table gets, given the
    /// `current` one; fails if `changes` ask for a restructure that changes
    /// nothing (`ALTER TABLE`).
    fn transaction_free(&self, current: bool, block_changes: bool) -> Result<bool, Error> {
        let free = self.transaction_free.unwrap_or(current);
        let idle = self.add.is_empty()
            && self.delete.is_empty()
            && self.change.is_empty()
            && !block_changes
            && free == current;
        match self.require_change && idle {
            true => Err(Error::RestructureFailed("Nothing to be done.".to_owned())),
            false => Ok(free),
        }
    }
}

/// Whether the table `opened` is transaction-free now.
fn is_transaction_free(opened: &table::Table) -> bool {
    opened.info().structure.header.flags & tflip_storage::table::flags::TRANSACTION_FREE != 0
}

/// 5011 with `why`, a field to delete or change that the table does not
/// have.
fn no_such_field(why: String) -> Error {
    Error::NoSuchField(why)
}

/// A field definition and the position it asks for (`,(n)` at its end).
fn with_position(definition: &str) -> (String, Option<usize>) {
    let trimmed = definition.trim();
    if let Some((rest, last)) = trimmed.rsplit_once(',') {
        let last = last.trim();
        if let Some(number) = last.strip_prefix('(').and_then(|l| l.strip_suffix(')')) {
            return (rest.to_owned(), number.trim().parse().ok());
        }
    }
    (trimmed.to_owned(), None)
}

/// The definitions of a list, the empty ones left out.
fn definitions(list: &str) -> impl Iterator<Item = &str> {
    list.split(';').filter(|d| !d.trim().is_empty())
}

/// The new number of each old field that stays.
fn renumbering(fields: &[(FieldSpec, Option<usize>)]) -> impl Fn(usize) -> Option<usize> + '_ {
    |number| {
        fields
            .iter()
            .position(|(_, source)| *source == Some(number))
            .map(|i| i + 1)
    }
}

/// The name a table's new file has until it is complete.
fn temporary(path: &Path, extension: &str) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{stem}_tflip_restructure.{extension}"))
}

/// Stores `cell` in `field` of `record`, which is the record `slot` of the
/// old table.
fn apply_cell(
    cell: Cell,
    field: &Field,
    record: &mut [u8],
    dates: Dates,
    file: &mut TableFile,
    slot: u32,
) -> Result<(), Error> {
    match cell {
        Cell::Null => {}
        Cell::Value(value) => field
            .encode(&value, record)
            .map_err(|_| failed_on(Error::InvalidFieldType, field, slot))?,
        Cell::Set(value) => match setters::apply(field, &value, dates, record) {
            Ok(()) | Err(Error::DataTruncated) => {}
            Err(error) => return Err(failed_on(error, field, slot)),
        },
        Cell::Memo(bytes) => {
            let memo = file.memo().ok_or(Error::CorruptTable)?;
            field
                .store_memo(Some(&bytes), record, memo)
                .map_err(|_| Error::CorruptMemo)?;
        }
    }
    Ok(())
}

/// Removes the new table's files if writing it failed, and passes the
/// result on.
fn discard_on_failure(
    written: Result<(), Error>,
    fresh: &Path,
    fresh_memo: &Path,
) -> Result<(), Error> {
    if written.is_err() {
        let _ = std::fs::remove_file(fresh);
        let _ = std::fs::remove_file(fresh_memo);
    }
    written
}

/// The old files `aside` go to their backups, the new table and its memo
/// file in their place.
fn swap_in(
    aside: &[&Path],
    fresh: &Path,
    path: &Path,
    fresh_memo: &Path,
    memo_path: &Path,
) -> Result<(), Error> {
    for file in aside {
        if file.exists() {
            let _ = std::fs::remove_file(backup(file));
            std::fs::rename(file, backup(file)).map_err(|_| Error::FileInUse)?;
        }
    }
    std::fs::rename(fresh, path).map_err(|_| Error::FileCreate)?;
    if fresh_memo.exists() {
        std::fs::rename(fresh_memo, memo_path).map_err(|_| Error::FileCreate)?;
    }
    Ok(())
}

/// Undoes [`swap_in`] when the orders cannot be built on the new fields: the
/// new table goes and the old files come back from their backups. A new memo
/// file, where the old table had none, stays but for a `free` table's
/// (`fixtures/sql/ddalterfree`, `ddalterdbf`).
fn swap_back(aside: &[&Path], path: &Path, memo_path: &Path, free: bool) {
    let had_memo = backup(memo_path).exists();
    let _ = std::fs::remove_file(path);
    for file in aside {
        let from = backup(file);
        if from.exists() {
            let _ = std::fs::remove_file(file);
            let _ = std::fs::rename(&from, file);
        }
    }
    if free && !had_memo {
        let _ = std::fs::remove_file(memo_path);
    }
}

/// Opens the table again by `reopen`, reindexes it and closes it.
fn reindex_reopened(
    session: &Session,
    connection: Handle,
    reopen: &str,
    settings: &TableSettings,
) -> Result<(), Error> {
    let reopened = table::open_rebuilding(session, connection, reopen, settings.clone())?;
    let rebuilt = index::reindex(session, reopened);
    table::close(session, reopened)?;
    rebuilt
}

/// The new fields, each with the number of the field its values come from
/// (none for a new field), and whether it came from a change.
fn new_fields(old: &[Field], changes: &Changes) -> Result<Vec<(FieldSpec, Option<usize>)>, Error> {
    let mut fields: Vec<(FieldSpec, Option<usize>)> = old
        .iter()
        .enumerate()
        .map(|(i, f)| (FieldSpec::from(f), Some(i + 1)))
        .collect();
    let find = |fields: &[(FieldSpec, Option<usize>)], name: &str| {
        fields
            .iter()
            .position(|(spec, _)| spec.name.eq_ignore_ascii_case(name.trim()))
    };
    for name in &changes.delete {
        let at = find(&fields, name).ok_or_else(|| {
            no_such_field(format!(
                "The field to be deleted named \"{name}\" does not exist in the table."
            ))
        })?;
        fields.remove(at);
    }
    for Changed {
        from,
        spec,
        position,
        text,
        ..
    } in &changes.change
    {
        // A field is named as the table had it, not as an earlier change
        // of the statement renamed it.
        let not_there = || {
            no_such_field(format!(
                "The field to be changed named \"{from}\" does not exist in the table."
            ))
        };
        let number = old
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(from.trim()))
            .ok_or_else(not_there)?
            + 1;
        let at = fields
            .iter()
            .position(|(_, source)| *source == Some(number))
            .ok_or_else(not_there)?;
        if let Some(taken) = find(&fields, &spec.name)
            && taken != at
        {
            return Err(Error::InvalidFieldDefinition(format!(
                "The new field name in the field changing specification \"{text}\" already \
                 exists in the table."
            )));
        }
        let source = fields[at].1;
        fields.remove(at);
        let to = position
            .map_or(at, |p| p.saturating_sub(1))
            .min(fields.len());
        fields.insert(to, (spec.clone(), source));
    }
    for Added {
        spec,
        position,
        text,
        ..
    } in &changes.add
    {
        if find(&fields, &spec.name).is_some() {
            return Err(Error::InvalidFieldDefinition(format!(
                "The field to be added named \"{text}\" already exists in the table."
            )));
        }
        let to = position
            .map_or(fields.len(), |p| p.saturating_sub(1))
            .min(fields.len());
        fields.insert(to, (spec.clone(), None));
    }
    Ok(fields)
}

/// What a new field of a record gets.
enum Cell {
    Null,
    /// The value, as the field stores it.
    Value(Value),
    /// A value set as a client would set it.
    Set(SetValue),
    /// A memo value's bytes.
    Memo(Vec<u8>),
}

fn is_text(t: FieldType) -> bool {
    use FieldType::*;
    matches!(
        t,
        Character | CiCharacter | VarChar | LegacyVarChar | NChar | NVarChar | Memo | NMemo
    )
}

fn is_number(t: FieldType) -> bool {
    use FieldType::*;
    matches!(
        t,
        ShortInt | Integer | LongInt | AutoInc | RowVersion | Double | CurDouble | Numeric | Money
    )
}

fn is_moment(t: FieldType) -> bool {
    matches!(
        t,
        FieldType::Date | FieldType::TimeStamp | FieldType::ModTime | FieldType::Time
    )
}

/// The number `value`, a value of a number or Logical field, is.
fn number(value: &Value) -> f64 {
    match value {
        Value::Integer(n) => *n as f64,
        Value::Double(d) => *d,
        Value::Money(m) => *m as f64 / 10_000.0,
        Value::Numeric(text) => text.trim().parse().unwrap_or(0.0),
        Value::Logical(b) => f64::from(u8::from(*b)),
        _ => 0.0,
    }
}

/// What the value `value` of the field `from` becomes in the field `to` (see
/// the module's documentation).
fn convert(
    from: &Field,
    to: &FieldSpec,
    value: Value,
    date_format: &str,
    epoch: u16,
) -> Result<Cell, Error> {
    use FieldType::*;
    if matches!(value, Value::Null) {
        return Ok(Cell::Null);
    }
    let (s, t) = (from.field_type, to.field_type);
    let text = || match crate::format::field_text(&value, &Layout::from(from), date_format) {
        FieldText::Text(text) => text,
        FieldText::Bytes(bytes) => tflip_storage::cp1252::decode(&bytes),
    };
    let memo = |text: String| Cell::Memo(tflip_storage::cp1252::encode(&text));
    if s == t {
        return Ok(match (&value, t) {
            (Value::Text(text), Memo | NMemo) => {
                Cell::Memo(tflip_storage::field::memo_text_bytes(to.field_type, text))
            }
            (Value::Binary(bytes), Binary | Image) => Cell::Memo(bytes.clone()),
            _ if from.length() == to.length && from.decimals() == to.decimals => Cell::Value(value),
            (Value::Text(text), _) => Cell::Set(SetValue::String(text.clone())),
            _ => Cell::Set(SetValue::Field(text())),
        });
    }
    if is_number(t) && (is_number(s) || s == Logical) {
        let n = number(&value);
        let whole = || -> Result<i32, Error> {
            let whole = n.trunc();
            match (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&whole) {
                true => Ok(whole as i32),
                false => Err(Error::ValueOverflow),
            }
        };
        return Ok(Cell::Set(match t {
            Integer | LongInt => SetValue::Long(whole()?),
            ShortInt => SetValue::Short(whole()? as i16),
            _ => SetValue::Double(n),
        }));
    }
    if is_moment(s) && is_moment(t) {
        let (day, ms) = match value {
            Value::Date(day) => (Some(day), None),
            Value::TimeStamp(day, ms) => (Some(day), Some(ms)),
            Value::Time(ms) => (None, Some(ms)),
            _ => (None, None),
        };
        return match (t, day, ms) {
            (Date, Some(day), _) if s != Time => Ok(Cell::Value(Value::Date(day))),
            (TimeStamp | ModTime, Some(day), ms) => {
                Ok(Cell::Value(Value::TimeStamp(day, ms.unwrap_or(0))))
            }
            // A time of day alone is a TimeStamp of day 0, which reads as NULL.
            (TimeStamp | ModTime, None, Some(ms)) => Ok(Cell::Value(Value::TimeStamp(0, ms))),
            (Time, _, Some(ms)) if s != Date => Ok(Cell::Value(Value::Time(ms))),
            _ => Err(Error::InvalidFieldType),
        };
    }
    let textual =
        is_text(s) || matches!(s, ShortInt | Integer | LongInt | Numeric | Money | Logical);
    if !textual || matches!(t, Binary | Image | Raw | VarBinary | Guid) {
        return Err(Error::InvalidFieldType);
    }
    Ok(match t {
        Memo | NMemo => memo(text()),
        // Dates are read as CTOD reads them: what is no date is NULL, or
        // for a TimeStamp, text with digits that is no date fails.
        Date => match lenient_date(&text(), date_format, epoch) {
            Ok(Some(day)) => Cell::Value(Value::Date(day)),
            _ => Cell::Null,
        },
        TimeStamp => match lenient_date(&text(), date_format, epoch) {
            Ok(Some(day)) => Cell::Value(Value::TimeStamp(day, 0)),
            Ok(None) => Cell::Null,
            Err(()) => return Err(Error::InvalidDate),
        },
        _ => Cell::Set(SetValue::Field(text())),
    })
}

/// The date `text` holds, read leniently in `date_format` as the original
/// engine reads it in a restructure: the runs of digits are the month, day
/// and year in the format's order, a year of up to two digits (or none)
/// in the hundred of `epoch`. `Ok(None)` for text without digits; `Err` for
/// one that is no date.
fn lenient_date(text: &str, date_format: &str, epoch: u16) -> Result<Option<i32>, ()> {
    let runs: Vec<&str> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|run| !run.is_empty())
        .collect();
    if runs.is_empty() {
        return Ok(None);
    }
    let format = date_format.to_ascii_uppercase();
    let mut order: Vec<(usize, char)> = ['M', 'D', 'Y']
        .iter()
        .filter_map(|&c| format.find(c).map(|at| (at, c)))
        .collect();
    order.sort();
    let (mut month, mut day, mut year) = (0u32, 0u32, None);
    for ((_, part), run) in order.iter().zip(&runs) {
        let value: u32 = run.parse().map_err(|_| ())?;
        match part {
            'M' => month = value,
            'D' => day = value,
            _ => year = Some((value, run.len())),
        }
    }
    let year = match year {
        Some((year, digits)) if digits > 2 => year as i32,
        Some((year, _)) => tflip_expr::text::century_year(year, epoch) as i32,
        None => tflip_expr::text::century_year(0, epoch) as i32,
    };
    let ymd = tflip_storage::value::Ymd { year, month, day };
    tflip_storage::value::ymd_to_julian(ymd).map(Some).ok_or(())
}

/// `<file>.BAK` beside `path`.
/// `error`, the value of the field `field` the record `record` could not
/// take, with what the original engine adds to its text: why, as for a
/// setter (see `AdsSet*`), the field, and the record.
fn failed_on(error: Error, field: &Field, record: u32) -> Error {
    let reason = match &error {
        Error::InvalidDataType => match field.field_type {
            FieldType::Logical => "Not a valid logical value. ",
            FieldType::Money => "The specified value is not a valid numeric expression. ",
            _ => "Expected a numeric value. ",
        },
        Error::InvalidFieldType | Error::InvalidValue | Error::ValueOverflow => "",
        Error::JulianConversion { negative } => {
            let why = crate::julian_conversion_detail(*negative);
            return error.detailed(format!("{why}  Restructure failed on record {record}."));
        }
        // Text with digits that is no date, for a TimeStamp
        // (`fixtures/write/restructure`).
        Error::InvalidDate => {
            let why = crate::julian_conversion_detail(true);
            return error.detailed(format!("{why}  Restructure failed on record {record}."));
        }
        _ => return error,
    };
    let name = &field.name;
    error.detailed(format!(
        "{reason}Field: \"{name}\"  Restructure failed on record {record}."
    ))
}

/// Refuses a restructure that deletes a field an order of `tags` (each its
/// name, expression and condition, with fields as `F<n>`) uses: as the
/// original engine names them, the first order whose key uses one, else the
/// first whose condition does (`fixtures/write/dbf_restructure`), and the
/// field as `delete` names it. `renumber` gives the fields that stay their
/// new numbers.
fn check_in_use(
    old: &[Field],
    tags: &[(String, String, String)],
    renumber: impl Fn(usize) -> Option<usize>,
    delete: &[String],
) -> Result<(), Error> {
    let gone = |text: &str| {
        index::field_numbers(text)
            .into_iter()
            .find(|&number| renumber(number).is_none())
    };
    let used = tags
        .iter()
        .find_map(|(tag, expression, _)| gone(expression).map(|number| (tag, number)))
        .or_else(|| {
            tags.iter()
                .find_map(|(tag, _, condition)| gone(condition).map(|number| (tag, number)))
        });
    let Some((tag, number)) = used else {
        return Ok(());
    };
    let name = old.get(number - 1).map_or("", |f| &f.name[..]);
    let field = delete
        .iter()
        .find(|d| d.eq_ignore_ascii_case(name))
        .map_or(name, |d| &d[..]);
    Err(Error::RestructureFailed(format!(
        "The restructure may not proceed because the field to be deleted named \"{field}\" is \
         used by an index named \"{tag}\"."
    )))
}

/// Whether a restructure of the dictionary's table `opened` makes its orders
/// from the Key objects: a field an order uses goes, moves (a field before
/// it goes) or is changed. A restructure that adds fields or changes or
/// drops others leaves the index files alone, whatever they hold
/// (`fixtures/write/restructure_fts_dict_after`).
fn rebuilds_from_keys(
    opened: &table::Table,
    changes: &Changes,
    renumber: impl Fn(usize) -> Option<usize>,
) -> bool {
    let used: Vec<usize> = changes
        .keys
        .iter()
        .flat_map(|key| [&key.expression, &key.condition])
        .flat_map(|text| index::field_numbers(&opened.with_field_numbers(text)))
        .collect();
    used.iter().any(|&number| renumber(number) != Some(number))
        || changes.change.iter().any(|change| {
            opened
                .fields()
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(change.from.trim()))
                .is_some_and(|at| used.contains(&(at + 1)))
        })
}

/// The `ADS_FTS_*` options the dictionary keeps a full-text order's flags in.
const FTS_OPTIONS: u32 = 0x3C0;

/// The order the Key object `key` describes, as its index file would have it
/// (the keys' length and pad as `CREATE INDEX` gives them to the table's
/// fields now), with the options block of a full-text order. An order on an
/// NChar or NVarChar field is not made so.
fn tag_of_key(
    opened: &table::Table,
    key: &KeyObject,
) -> Result<(Tag, Option<create::FtsBlock>), Error> {
    use tflip_storage::order::{flags, options};
    const ADS_UNIQUE: u32 = 0x01;
    const ADS_CUSTOM: u32 = 0x04;
    const ADS_DESCENDING: u32 = 0x08;
    const ADS_CANDIDATE: u32 = 0x800;
    const ADS_BINARY: u32 = 0x1000;
    const ADS_FULL_TEXT: u32 = 0x20;
    const ADS_STRING: u16 = 4;
    let expression = opened.with_field_numbers(&key.expression);
    let condition = opened.with_field_numbers(&key.condition);
    if opened.names_unicode(&expression) || opened.names_unicode(&condition) {
        return Err(Error::NotImplemented);
    }
    let mut tag = Tag {
        name: key.name.to_ascii_uppercase(),
        header_page: 0,
        root: 0,
        key_length: key.key_length,
        options: 0,
        pad: 0,
        flags: 0,
        descending: key.options & ADS_DESCENDING != 0,
        expression,
        condition,
        collation: key.collation.clone(),
        locale: String::new(),
        uchar_width: 0,
        decimals: 0,
        fts_block: 0,
        keeps_score: false,
    };
    if key.options & ADS_FULL_TEXT != 0 {
        tag.options = options::FLAGS;
        tag.flags = flags::FULL_TEXT;
        tag.pad = b' ';
        // The words of the lists the Key object keeps, the noise ones
        // sorted and cut by the shortest word as the index keeps them.
        let [delimiters, noise, drop, conditional] = key.lists.clone();
        let mut words: Vec<&[u8]> = noise
            .split(|b| b.is_ascii_whitespace())
            .filter(|word| word.len() >= usize::from(key.min_word))
            .collect();
        words.sort_unstable();
        let block = create::FtsBlock {
            options: key.options & FTS_OPTIONS,
            min_word: u32::from(key.min_word),
            max_word: u32::from(key.key_length),
            delimiters,
            noise: words.join(&b' '),
            drop,
            conditional,
        };
        return Ok((tag, Some(block)));
    }
    if key.options & (ADS_UNIQUE | ADS_CANDIDATE) != 0 {
        tag.options |= options::UNIQUE;
    }
    if key.options & ADS_CUSTOM != 0 {
        tag.options |= options::CUSTOM;
    }
    if !tag.condition.is_empty() {
        tag.options |= options::CONDITION;
    }
    tag.pad = match (key.options & ADS_BINARY != 0, key.key_type == ADS_STRING) {
        (true, _) => {
            tag.options |= options::FLAGS;
            tag.flags |= flags::BINARY;
            b'F'
        }
        (false, true) => b' ',
        (false, false) => 0,
    };
    Ok((tag, None))
}

/// Refuses a restructure of a table with a custom order, whose keys the
/// rebuilt table could not keep (`fixtures/write/cdx_custom_reindex`): the
/// first one's name, as the original engine gives it.
fn check_custom<'a>(mut custom: impl Iterator<Item = &'a Tag>) -> Result<(), Error> {
    match custom.next() {
        Some(tag) => Err(Error::RestructureFailed(format!(
            "The restructure may not proceed because custom or user defined indexes exist in an \
             index named \"{}\".",
            tag.name
        ))),
        None => Ok(()),
    }
}

/// An error of compiling an order's key on the new fields, as the key's.
fn key_expression(error: Error) -> Error {
    match error {
        Error::Expression(kind) => Error::KeyExpression(kind),
        error => error,
    }
}

fn backup(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".BAK");
    path.with_file_name(name)
}

/// Changes the fields of the free ADT table `name` of `connection`
/// (`AdsRestructureTable120`; see the module's documentation). The table
/// must not be open anywhere else.
///
/// What it returns is what became of the fields of an ADT table; a DBF
/// table's are not followed.
pub fn restructure(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: TableSettings,
    changes: &Changes,
) -> Result<Outcome, Error> {
    restructure_with(session, connection, name, settings, changes, None)
}

/// How far an online restructure of an ADT table has come (see
/// [`restructure_with`]).
#[derive(Default)]
struct Online {
    /// Whether the new table is to be built and no more, beside the old one
    /// while it is open to other connections.
    prebuild: bool,
    /// What the process had changed of the old table when the new one was
    /// built: the new one is current while it still is.
    stamp: Option<u64>,
}

/// [`restructure`] as the maintenance `gate` runs it, if it does: an online
/// restructure of an ADT table builds the new table while the old one is
/// open to the other connections, and in the final stage builds it again if
/// they changed the old one meanwhile (see [`crate::maintenance`]).
pub(crate) fn restructure_with(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: TableSettings,
    changes: &Changes,
    gate: Option<&crate::maintenance::Guard>,
) -> Result<Outcome, Error> {
    if !matches!(
        settings.table_type,
        TableType::Adt | TableType::Ntx | TableType::Cdx | TableType::Vfp
    ) {
        return Err(Error::NotImplemented);
    }
    // ADT blocks are 8 to 32768 bytes; a FoxPro or Visual FoxPro size is a
    // byte count from 33, or 1 to 32 blocks of 512 bytes (`ALTER TABLE …
    // MEMOBLOCKSIZE`, probed).
    let block_size = match (settings.table_type, changes.block_size) {
        (_, 0) => None,
        (TableType::Adt, 8..=32768) => Some(changes.block_size as u16),
        (TableType::Adt, _) => return Err(Error::InvalidMemoBlockSize),
        (_, 1..=1024) => Some(changes.block_size as u16),
        _ => return Err(Error::InvalidMemoBlockSize),
    };
    let exclusive = TableSettings {
        options: options::EXCLUSIVE,
        ..settings.clone()
    };
    let mut online = Online::default();
    if let Some(gate) = gate.filter(|gate| gate.online())
        && settings.table_type == TableType::Adt
    {
        let handle = table::open(session, connection, name, settings.clone())?;
        online.prebuild = true;
        let built = restructure_open(
            session,
            connection,
            name,
            handle,
            &settings,
            changes,
            block_size,
            &mut online,
        );
        let _ = table::close(session, handle);
        built?;
        online.prebuild = false;
        gate.final_stage();
    }
    let open = || {
        table::open(session, connection, name, exclusive.clone()).map_err(|error| match error {
            Error::LockFailed | Error::FileInUse => table::with_file(
                session,
                connection,
                name,
                exclusive.clone(),
                Error::FileInUse,
            ),
            error => error,
        })
    };
    let handle = match gate {
        Some(gate) => gate.until_free(|| {
            table::open(session, connection, name, exclusive.clone()).map_err(|error| match error {
                // Whatever the others had open, they have let go of it.
                Error::LockFailed => Error::FileInUse,
                error => error,
            })
        }),
        None => open(),
    }?;
    let result = match settings.table_type {
        TableType::Adt => restructure_open(
            session,
            connection,
            name,
            handle,
            &exclusive,
            changes,
            block_size,
            &mut online,
        ),
        _ => restructure_dbf(session, connection, name, handle, &exclusive, changes),
    };
    // The table is closed by now, unless it failed before.
    let _ = table::close(session, handle);
    result
}

/// `AdsRestructureTable`: [`restructure`] of a free table, or
/// [`restructure_dictionary`] of one of the dictionary `connection` is to
/// (its objects follow).
pub fn restructure_table(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: TableSettings,
    changes: &Changes,
) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    let listed = crate::connection::get(session, connection)
        .ok()
        .and_then(|owner| {
            owner.with_database_opt(|db| db.dictionary().named(Kind::Table, name).is_some())
        })
        .unwrap_or(false);
    match listed {
        true => restructure_dictionary(session, connection, name, changes),
        false => restructure(session, connection, name, settings, changes).map(|_| ()),
    }
}

/// The restructure of the table `name` of the dictionary `connection` is to:
/// `ALTER TABLE` of a table of the dictionary (`fixtures/sql/ddoptsdict`,
/// `ddalter`, `ddalterdbf`). The old files go aside as `.BAK` (an ADT
/// table's unbound, its index file and a CDX file of the dictionary kept
/// out of them), the new ADT table is bound to the dictionary, and the
/// dictionary's objects follow: the Field objects are made anew (or renamed
/// where the table's fields were only named again), the Table object has
/// its field count and the memo block size and transaction-free mark it was
/// given, and the Key objects the names, key lengths and key types of the
/// orders that were rebuilt. A Clipper table's orders, each in a file of its
/// own, are rebuilt too. Where an order cannot be built on the new fields
/// the old table comes back, but the index files are gone.
pub fn restructure_dictionary(
    session: &Session,
    connection: Handle,
    name: &str,
    changes: &Changes,
) -> Result<(), Error> {
    restructure_dictionary_with(session, connection, name, changes, None)
}

/// [`restructure_dictionary`] as the maintenance `gate` runs it, if it does
/// (see [`restructure_with`]).
pub(crate) fn restructure_dictionary_with(
    session: &Session,
    connection: Handle,
    name: &str,
    changes: &Changes,
    gate: Option<&crate::maintenance::Guard>,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let owner = crate::connection::get(session, connection)?;
    let found = owner
        .with_database_opt(|db| {
            let table = db.dictionary().named(Kind::Table, name)?;
            let files = db
                .dictionary()
                .children(table.id, Kind::IndexFile)
                .into_iter()
                .filter_map(|file| file.text(dd::index_file::RELATIVE_PATH))
                .collect::<Vec<_>>();
            let list = |key: &dd::Object, slot| {
                let bytes = key.slot(slot).unwrap_or_default();
                bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())].to_vec()
            };
            let keys = db
                .dictionary()
                .children(table.id, Kind::Index)
                .into_iter()
                .filter_map(|key| {
                    let file = db.dictionary().object(key.info1? as u32)?;
                    Some(KeyObject {
                        id: key.id,
                        name: key.name.clone(),
                        file: PathBuf::from(file.text(dd::index_file::RELATIVE_PATH)?),
                        expression: key.text(dd::index::EXPRESSION)?,
                        condition: key.text(dd::index::CONDITION).unwrap_or_default(),
                        options: key.u32(dd::index::OPTIONS).unwrap_or(0),
                        key_length: key.u16(dd::index::KEY_LENGTH).unwrap_or(0),
                        key_type: key.u16(dd::index::KEY_TYPE).unwrap_or(0),
                        collation: key.text(dd::index::COLLATION).unwrap_or_default(),
                        min_word: key.u16(dd::index::FTS_MIN_LENGTH).unwrap_or(0),
                        lists: [
                            list(key, dd::index::FTS_DELIMITERS),
                            list(key, dd::index::FTS_NOISE),
                            list(key, dd::index::FTS_DROP_CHARS),
                            list(key, dd::index::FTS_CONDITIONAL_CHARS),
                        ],
                    })
                })
                .collect::<Vec<_>>();
            Some((
                table.id,
                crate::dictionary::table_type(table),
                table.text(dd::table::RELATIVE_PATH),
                (files, keys),
            ))
        })
        .flatten();
    let Some((id, table_type, Some(relative), (files, keys))) = found else {
        return Err(Error::NotImplemented);
    };
    if table_type == TableType::Database {
        return Err(Error::NotImplemented);
    }
    let folder = owner.path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let dictionary = owner
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let in_folder = |relative: &str| folder.join(relative.strip_prefix(".\\").unwrap_or(relative));
    let path = in_folder(&relative);
    let moved = Changes {
        add: changes.add.clone(),
        delete: changes.delete.clone(),
        change: changes.change.clone(),
        block_size: changes.block_size,
        transaction_free: changes.transaction_free,
        require_change: changes.require_change,
        // A DBF file has nowhere to say it belongs to a dictionary.
        bind: (table_type == TableType::Adt).then_some((id, dictionary)),
        indexes: files.iter().map(|file| in_folder(file)).collect(),
        expressions: keys
            .iter()
            .map(|key| {
                let file = key.file.to_string_lossy();
                (
                    in_folder(&file),
                    key.expression.clone(),
                    key.condition.clone(),
                )
            })
            .collect(),
        keys: keys
            .into_iter()
            .map(|key| KeyObject {
                file: in_folder(&key.file.to_string_lossy()),
                ..key
            })
            .collect(),
    };
    let settings = TableSettings {
        table_type,
        ..TableSettings::default()
    };
    // The original puts an encrypted table of an AES dictionary in the clear
    // for the restructure (the old files go aside so) and enciphers the new
    // one after, its index files in the clear whatever the dictionary says
    // (`fixtures/aes/dictwrite`, `44_restructure_one`).
    // A Clipper or FoxPro table is not put in the clear: its old files go aside
    // enciphered (`fixtures/aes/rc4tp`, `y8`).
    let encrypted = match table_type {
        TableType::Adt => {
            crate::dictionary_write::table_file_encryption(session, connection, name, false)?
        }
        _ => crate::dictionary_write::table_marked_encrypted(session, connection, name)?,
    };
    let outcome = match restructure_with(session, connection, name, settings, &moved, gate) {
        // A full-text order's field of a type that cannot have one is
        // refused plain, where the free table's refusal has its detail
        // (`fixtures/write/restructure_fts_dict`); the records' own
        // conversions fail with the field and record named.
        Err(error)
            if matches!(error, Error::KeyExpression(_))
                || (matches!(
                    error,
                    Error::InvalidFieldType | Error::Ace { code: 5032, .. }
                ) && table_type == TableType::Adt) =>
        {
            crate::dictionary_write::failed_restructure(session, connection, name)?;
            return Err(error);
        }
        Err(error) => return Err(error),
        outcome => outcome?,
    };
    if table_type == TableType::Adt && !outcome.renamed {
        // The old table is a free one now, and its index file is the
        // dictionary's to keep as it is.
        let _ = dd::create::free_table(&backup(&path));
        let _ = dd::create::keep_aes_mark(&backup(&path));
        let _ = std::fs::remove_file(backup(&path.with_extension("adi")));
        crate::cache::evict_path(&path);
        crate::cache::evict_kind(crate::cache::Kind::Tables);
    } else if matches!(table_type, TableType::Cdx | TableType::Vfp) {
        let _ = std::fs::remove_file(backup(&path.with_extension("cdx")));
    }
    crate::dictionary_write::restructured_table(
        session, connection, name, &path, table_type, changes, &outcome,
    )?;
    if encrypted {
        crate::dictionary_write::table_file_encryption(session, connection, name, true)?;
    }
    // The dictionary keeps the memo block size the table was given.
    if changes.block_size != 0 {
        crate::dictionary_write::set_table_property_api(
            session,
            connection,
            name,
            215,
            &(changes.block_size as u16).to_le_bytes(),
        )?;
    }
    match changes.transaction_free {
        Some(free) => crate::dictionary_write::set_table_property_api(
            session,
            connection,
            name,
            220,
            &[u8::from(free), 0],
        ),
        None => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
fn restructure_open(
    session: &Session,
    connection: Handle,
    name: &str,
    handle: Handle,
    settings: &TableSettings,
    changes: &Changes,
    block_size: Option<u16>,
    online: &mut Online,
) -> Result<Outcome, Error> {
    let opened = table::get(session, handle)?;
    let path = opened.path().to_path_buf();
    let old: Vec<Field> = opened.fields().to_vec();
    // A table with no memo field has no block size to change.
    let block_changes = block_size.is_some_and(|b| {
        opened
            .memo_block_size()
            .is_some_and(|now| now != u32::from(b))
    });
    let free = changes.transaction_free(is_transaction_free(&opened), block_changes)?;
    let fields = new_fields(&old, changes)?;
    let sources: Vec<Option<usize>> = fields.iter().map(|(_, source)| *source).collect();
    // Fields that are only named again keep their table as it is, but for
    // their names in its header (`fixtures/write/restructure_rename`).
    if let Some(names) = renamed_only(
        &old,
        &fields,
        changes,
        is_transaction_free(&opened),
        free,
        block_changes,
        false,
    ) && !opened.info().structure.header.encrypted_descriptors()
    {
        drop(opened);
        table::close(session, handle)?;
        // Nothing to build beforehand: the final stage renames them.
        if online.prebuild {
            return Ok(Outcome {
                sources,
                renamed: true,
            });
        }
        let names: Vec<(usize, &str)> = names.iter().map(|(i, name)| (*i, name.as_str())).collect();
        create::rename_adt_fields(&path, &names).map_err(|_| Error::FileCreate)?;
        crate::cache::evict_path(&path);
        crate::cache::evict_kind(crate::cache::Kind::Tables);
        return Ok(Outcome {
            sources,
            renamed: true,
        });
    }
    let renumber = renumbering(&fields);
    // The orders of the index files (the structural one, and those of a
    // dictionary's table in files of their own), which must not use a field
    // that goes.
    let index_path = path.with_extension("adi");
    let mut files = vec![index_path.clone()];
    for file in &changes.indexes {
        if !files.contains(file) {
            files.push(file.clone());
        }
    }
    // A dictionary's table whose fields the orders use (or whose field
    // numbers move) has them all made from the Key objects, which a failed
    // restructure left more of than the index file holds; any other
    // restructure leaves the orders as the file has them.
    let rekey = changes.bind.is_some() && rebuilds_from_keys(&opened, changes, &renumber);
    let mut kept = Vec::new();
    for file in files {
        let Ok(mut opened_index) = IndexFile::open(&file) else {
            continue;
        };
        let page_size = opened_index.page_size();
        let mut tags = opened_index.tags("");
        // The orders the Key objects have and the file has not: made from
        // the Key objects' properties.
        let mut made = std::collections::HashMap::new();
        if rekey {
            let mut ordered = Vec::with_capacity(tags.len());
            let mut here: Vec<&KeyObject> = changes
                .keys
                .iter()
                .filter(|key| {
                    key.file
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&file.to_string_lossy())
                })
                .collect();
            here.sort_by_key(|key| key.id);
            for key in here {
                match tags
                    .iter()
                    .position(|t| t.name.eq_ignore_ascii_case(&key.name))
                {
                    Some(at) => ordered.push(tags.remove(at)),
                    None => {
                        let (tag, block) = tag_of_key(&opened, key)?;
                        made.insert(tag.name.clone(), block);
                        ordered.push(tag);
                    }
                }
            }
            ordered.append(&mut tags);
            tags = ordered;
        }
        check_custom(tags.iter().filter(|tag| tag.custom()))?;
        let numbered: Vec<_> = tags
            .iter()
            .filter(|t| !made.contains_key(&t.name))
            .map(|t| (t.name.clone(), t.expression.clone(), t.condition.clone()))
            .collect();
        check_in_use(&old, &numbered, &renumber, &changes.delete)?;
        let mut renumbered = Vec::with_capacity(tags.len());
        for tag in tags {
            let expression = index::renumber_fields(&tag.expression, &renumber);
            let condition = index::renumber_fields(&tag.condition, &renumber);
            let (Some(expression), Some(condition)) = (expression, condition) else {
                // An order made from its Key object that uses a field that
                // went fails when its turn comes, after those before it
                // were built (`fixtures/write/restructure_fts_dict_after`).
                if let Some(block) = made.get(&tag.name) {
                    let error = match block {
                        Some(_) => Error::Ace {
                            code: 5032,
                            detail: String::new(),
                        },
                        None => Error::KeyExpression(tflip_expr::ErrorKind::UnknownIdentifier),
                    };
                    renumbered.push((tag, String::new(), String::new(), None, Some(error)));
                    continue;
                }
                return Err(Error::CorruptIndex);
            };
            // Read before the old files go aside: a full-text order is made
            // again with its options.
            let block = match made.remove(&tag.name) {
                Some(block) => block,
                None => opened_index
                    .fts_block(&tag)
                    .map_err(|_| Error::CorruptIndex)?,
            };
            // A free table's is refused before anything is built, whatever
            // the records hold (`fixtures/write/restructure_fts_types`); a
            // dictionary's order fails when it is built, as any other does
            // (`fixtures/write/restructure_fts_dict`).
            if block.is_some()
                && changes.bind.is_none()
                && index::field_numbers(&expression).into_iter().any(|m| {
                    fields
                        .get(m - 1)
                        .is_some_and(|(spec, _)| !crate::ddl::can_have_full_text(spec.field_type))
                })
            {
                return Err(Error::InvalidFieldType
                    .detailed("FTS indexes can only be created on character and BLOB fields."));
            }
            renumbered.push((tag, expression, condition, block, None));
        }
        let header = std::fs::read(&file).map_err(|_| Error::CorruptIndex)?;
        kept.push((file, page_size, header, renumbered));
    }
    let specs: Vec<FieldSpec> = fields.iter().map(|(spec, _)| spec.clone()).collect();
    let block_size = block_size
        .or_else(|| opened.memo_block_size().map(|b| b as u16))
        .unwrap_or(256);
    let date_format = opened.date_format();
    let dates = Dates {
        format: &date_format,
        epoch: opened.session_settings().epoch(),
    };

    // The new table, under a name of its own until it is complete.
    let fresh = temporary(&path, "adt");
    let fresh_memo = fresh.with_extension("adm");
    let stamp = opened.file_changes();
    // The table built beforehand is the one, if nothing has changed since.
    let current = online.stamp == Some(stamp) && fresh.exists();
    if !current {
        let _ = std::fs::remove_file(&fresh);
        let _ = std::fs::remove_file(&fresh_memo);
    }
    let written = (|| {
        if current {
            return Ok(());
        }
        // A dictionary's table laid out for AES gets a new one laid out so,
        // which the dictionary arms after (`fixtures/aes/dictwrite`,
        // `44_restructure_one`).
        let aes_layout = changes.bind.is_some()
            && opened.info().structure.header.extension as usize
                == tflip_storage::create::AES_EXTENSION;
        match aes_layout {
            true => create::aes_adt_table(&fresh, &specs, block_size),
            false => create::adt_table(&fresh, &specs, block_size),
        }
        .map_err(|_| Error::FileCreate)?;
        let mut file =
            TableFile::open_for_writing(&fresh, Format::Adt).map_err(|_| Error::FileCreate)?;
        if free {
            file.set_transaction_free(true)
                .map_err(|_| Error::FileCreate)?;
        }
        let new: Vec<Field> = file.structure.fields.clone();
        // An AutoInc's largest value, which the next one follows.
        let mut largest: Option<u32> = None;
        for (ordinal, slot) in opened.active_slots()?.into_iter().enumerate() {
            let values = opened.slot_values(slot)?;
            let mut record = file.structure.null_record();
            for (i, (spec, source)) in fields.iter().enumerate() {
                let field = &new[i];
                let cell = match (field.field_type, source) {
                    // An AutoInc keeps a whole number's value; a record
                    // without one gets its place, as in the original engine.
                    (FieldType::AutoInc, Some(n)) => {
                        let kept = match (&values[n - 1], old[n - 1].field_type) {
                            (
                                Value::Integer(v),
                                FieldType::ShortInt
                                | FieldType::Integer
                                | FieldType::LongInt
                                | FieldType::AutoInc,
                            ) => u32::try_from(*v).ok(),
                            _ => None,
                        };
                        let value = kept.unwrap_or(ordinal as u32 + 1);
                        largest = largest.max(Some(value));
                        Cell::Value(Value::Integer(i128::from(value)))
                    }
                    // A RowVersion counts the records.
                    (FieldType::RowVersion, _) => {
                        let next = file.take_row_version().map_err(|_| Error::CorruptTable)?;
                        Cell::Value(Value::Integer(i128::from(next)))
                    }
                    (_, Some(n)) => {
                        let value = values[n - 1].clone();
                        convert(&old[n - 1], spec, value, &date_format, dates.epoch)
                            .map_err(|error| failed_on(error, field, slot))?
                    }
                    (FieldType::AutoInc, None) => {
                        let next = file.take_autoinc(field).map_err(|_| Error::CorruptTable)?;
                        Cell::Value(Value::Integer(i128::from(next)))
                    }
                    (_, None) => Cell::Null,
                };
                apply_cell(cell, field, &mut record, dates, &mut file, slot)?;
            }
            file.insert(&record).map_err(|_| Error::CorruptTable)?;
        }
        if let Some(largest) = largest {
            file.set_next_autoinc(largest.wrapping_add(1))
                .map_err(|_| Error::CorruptTable)?;
        }
        Ok(())
    })();
    discard_on_failure(written, &fresh, &fresh_memo)?;
    if online.prebuild {
        online.stamp = Some(stamp);
        drop(opened);
        table::close(session, handle)?;
        return Ok(Outcome {
            sources,
            renamed: false,
        });
    }
    if let Some((id, dictionary)) = &changes.bind {
        tflip_storage::dd::create::bind_table(&fresh, *id, dictionary)
            .map_err(|_| Error::FileCreate)?;
    }
    drop(opened);
    table::close(session, handle)?;

    // The old files go aside, the new ones in their place.
    let aside = ["adt", "adm", "adi"].map(|extension| path.with_extension(extension));
    let aside: Vec<&Path> = aside.iter().map(PathBuf::as_path).collect();
    swap_in(
        &aside,
        &fresh,
        &path,
        &fresh_memo,
        &path.with_extension("adm"),
    )?;

    // The orders, on the new fields, built as a reindex builds them. The
    // table opens by its name again: a server's sessions reach files in
    // their shares only.
    if kept.is_empty() {
        return Ok(Outcome {
            sources,
            renamed: false,
        });
    }
    let full = path.to_string_lossy();
    let reopen = if connection == 0 { &*full } else { name };
    let reopened = table::open_rebuilding(session, connection, reopen, settings.clone())?;
    let table = table::get(session, reopened)?;
    type Built = (PathBuf, u32, u32, Vec<u8>, Vec<OrderSpec>);
    let mut rebuilt: Vec<Built> = Vec::with_capacity(kept.len());
    let files: Vec<PathBuf> = kept.iter().map(|(file, ..)| file.clone()).collect();
    let shaped = (|| {
        for (file, page_size, header, tags) in kept {
            let collation = u32::from_le_bytes(header[0x14..0x18].try_into().unwrap());
            let mut bytes = create::adi_bytes(page_size as u16, collation);
            bytes[create::VERSION] = header[create::VERSION];
            // Pushed as each is shaped: the ones before a failure are kept.
            rebuilt.push((file, page_size, collation, bytes, Vec::new()));
            for (tag, expression, condition, block, deferred) in tags {
                if let Some(error) = deferred {
                    return Err(error);
                }
                if block.is_some() {
                    let unfit = index::field_numbers(&expression).into_iter().any(|m| {
                        fields.get(m - 1).is_some_and(|(spec, _)| {
                            !crate::ddl::can_have_full_text(spec.field_type)
                        })
                    });
                    if unfit {
                        return Err(Error::InvalidFieldType);
                    }
                    let order = OrderSpec {
                        expression,
                        condition,
                        fts: block,
                        ..OrderSpec::from_tag(&tag)
                    };
                    if let Some((.., orders)) = rebuilt.last_mut() {
                        orders.push(order);
                    }
                    continue;
                }
                let long = table.with_field_names(&expression)?;
                let key = table.compile(&long).map_err(key_expression)?;
                let (key_length, text) = crate::ddl::key_shape(&table, &long, &key, 0);
                // `ALTER TABLE` gives an order whose key has a field that
                // changes its type the new keys' own pad byte; a restructure
                // call keeps the old one (`fixtures/sql/ddalterdbf`,
                // `fixtures/write/restructure`).
                let retyped = index::field_numbers(&expression).into_iter().any(|m| {
                    fields.get(m - 1).is_some_and(|(spec, source)| {
                        source.is_some_and(|n| old[n - 1].field_type != spec.field_type)
                    })
                });
                let order = OrderSpec {
                    expression,
                    condition,
                    key_length: key_length as u16,
                    pad: match (retyped && changes.require_change, text) {
                        (false, _) => tag.pad,
                        (true, true) => b' ',
                        (true, false) => 0,
                    },
                    ..OrderSpec::from_tag(&tag)
                };
                if let Some((.., orders)) = rebuilt.last_mut() {
                    orders.push(order);
                }
            }
        }
        Ok(())
    })();
    drop(table);
    table::close(session, reopened)?;
    if let Err(error) = shaped {
        // The old table comes back; of a dictionary's table the index files
        // are gone (`fixtures/sql/ddalterdbf`).
        swap_back(
            &aside,
            &path,
            &path.with_extension("adm"),
            changes.indexes.is_empty(),
        );
        if !changes.indexes.is_empty() {
            // The files after the one that failed stay as they were
            // (`fixtures/sql/spddalterfail3`).
            for file in &files[..rebuilt.len()] {
                let _ = std::fs::remove_file(file);
            }
            // A file keeps the orders built before the one that failed
            // (`fixtures/sql/spddalterfail`).
            let mut kept = false;
            for (file, page_size, collation, _, orders) in &rebuilt {
                if orders.is_empty() {
                    continue;
                }
                // The version of the orders it keeps, not of the file it was
                // (`fixtures/write/restructure_fts_dict_after`).
                let bytes = create::adi_bytes(*page_size as u16, *collation);
                std::fs::write(file, bytes).map_err(|_| Error::FileCreate)?;
                for order in orders {
                    create::add_order(file, order, *page_size as u16, *collation)
                        .map_err(|_| Error::CorruptIndex)?;
                }
                kept = true;
            }
            if kept {
                let _ = reindex_reopened(session, connection, reopen, settings);
            }
        }
        return Err(error);
    }
    build_orders(session, connection, reopen, settings, &rebuilt)?;
    Ok(Outcome {
        sources,
        renamed: false,
    })
}

/// Makes each order of `rebuilt`'s files and builds its keys before the next
/// order is made, as the original does (`fixtures/write/restructure_fts_layout`:
/// the pages of a large order come out as `CREATE INDEX` lays them, not as a
/// reindex does).
fn build_orders(
    session: &Session,
    connection: Handle,
    reopen: &str,
    settings: &TableSettings,
    rebuilt: &[(PathBuf, u32, u32, Vec<u8>, Vec<OrderSpec>)],
) -> Result<(), Error> {
    let reopened = table::open_rebuilding(session, connection, reopen, settings.clone())?;
    let built = (|| {
        let table = table::get(session, reopened)?;
        for (file, page_size, collation, bytes, orders) in rebuilt {
            std::fs::write(file, bytes).map_err(|_| Error::FileCreate)?;
            for order in orders {
                create::add_order(file, order, *page_size as u16, *collation)
                    .map_err(|_| Error::CorruptIndex)?;
                match order.fts {
                    Some(_) => index::build_full_text(&table, file, &order.name),
                    None => index::build_keys(&table, file, &order.name),
                }?;
            }
        }
        Ok(())
    })();
    table::close(session, reopened)?;
    built
}

/// Whether `fields`, the table's new ones, are `old` with other names and
/// nothing else changed, in the order of `old`: each change names its field
/// otherwise (a change that gives the name it has is a restructure), no
/// field is added or deleted, and the table's block size and
/// transaction-free mark stay; then the new names, by field number.
///
/// A Clipper or FoxPro table's names are upper case (`upper`), so a change
/// that only gives the other case gives the name it has
/// (`fixtures/sql/ddalterfree`).
fn renamed_only(
    old: &[Field],
    fields: &[(FieldSpec, Option<usize>)],
    changes: &Changes,
    was_free: bool,
    free: bool,
    block_changes: bool,
    upper: bool,
) -> Option<Vec<(usize, String)>> {
    let same_name = |a: &str, b: &str| match upper {
        true => a.eq_ignore_ascii_case(b),
        false => a == b,
    };
    let idle = changes.add.is_empty()
        && changes.delete.is_empty()
        && changes.transaction_free.is_none()
        && !block_changes
        && free == was_free
        && fields.len() == old.len();
    if !idle
        || changes.change.is_empty()
        || changes
            .change
            .iter()
            .any(|c| same_name(c.from.trim(), &c.spec.name))
    {
        return None;
    }
    let mut names = Vec::new();
    for (i, ((spec, source), field)) in fields.iter().zip(old).enumerate() {
        let same = FieldSpec {
            name: field.name.clone(),
            ..spec.clone()
        } == FieldSpec::from(field);
        if *source != Some(i + 1) || !same {
            return None;
        }
        if !same_name(&spec.name, &field.name) {
            names.push((i, spec.name.clone()));
        }
    }
    Some(names)
}

/// The DBF definition of a new field: its own if it was given one, else
/// one the old field `old` had (VFP's NULL and binary flags kept).
fn dbf_spec(
    spec: &FieldSpec,
    old: Option<&Field>,
    given: Option<&create::DbfFieldSpec>,
) -> create::DbfFieldSpec {
    if let Some(given) = given {
        return given.clone();
    }
    let base = old.map_or_else(
        || create::DbfFieldSpec {
            spec: spec.clone(),
            nullable: false,
            binary: false,
            autoinc: None,
        },
        create::DbfFieldSpec::from,
    );
    create::DbfFieldSpec {
        spec: spec.clone(),
        ..base
    }
}

/// [`restructure`] of a DBF table, as the original engine does it
/// (`fixtures/write/dbf_restructure`): the table written anew with every
/// record, deleted ones too (flagged still), the values converted as for
/// ADT tables, the memo values written anew to an empty memo file of the
/// same block size, and the structural `.cdx` file rebuilt with its tags,
/// their expressions following the fields' names; the old files stay as
/// `<file>.BAK`. NTX files are left alone.
fn restructure_dbf(
    session: &Session,
    connection: Handle,
    name: &str,
    handle: Handle,
    settings: &TableSettings,
    changes: &Changes,
) -> Result<Outcome, Error> {
    use tflip_storage::dbf::Flavour;
    let flavour = match settings.table_type {
        TableType::Ntx => Flavour::Clipper,
        TableType::Cdx => Flavour::FoxPro,
        _ => Flavour::Visual,
    };
    let opened = table::get(session, handle)?;
    // A table with a password the handle has not given is not restructured
    // (`fixtures/aes/rc4tp`, `v15`).
    if opened.password_missing() {
        return Err(Error::EncryptionNotEnabled
            .detailed("Password is required because the table may contain encrypted records."));
    }
    let path = opened.path().to_path_buf();
    let old: Vec<Field> = opened.fields().to_vec();
    // A size up to 32 counts blocks of 512 bytes.
    let asked = match changes.block_size {
        0 => None,
        n @ 1..=32 => Some(n * 512),
        n => Some(n),
    }
    .filter(|_| flavour != Flavour::Clipper);
    // A Clipper table has no block size of its own to compare: any asked for is a
    // restructure (`fixtures/sql/ddalterdbf`).
    let block_changes = match flavour {
        Flavour::Clipper => changes.block_size != 0,
        _ => asked.is_some_and(|b| opened.memo_block_size().is_some_and(|now| now != b)),
    };
    let free = changes.transaction_free(is_transaction_free(&opened), block_changes)?;
    let fields = new_fields(&old, changes)?;
    let sources: Vec<Option<usize>> = fields.iter().map(|(_, source)| *source).collect();
    // Fields that are only named again keep their table as it is, but for
    // their names in its header and in the expressions of its structural
    // orders (`fixtures/sql/ddalterfree`).
    let upper = flavour != Flavour::Visual;
    if let Some(names) = renamed_only(
        &old,
        &fields,
        changes,
        is_transaction_free(&opened),
        free,
        block_changes,
        upper,
    ) {
        let index_path = path.with_extension(match path.extension().is_some_and(|e| e == "DBF") {
            true => "CDX",
            false => "cdx",
        });
        let renames: Vec<(String, String)> = changes
            .change
            .iter()
            .filter(|c| !c.from.trim().eq_ignore_ascii_case(&c.spec.name))
            .map(|c| match upper {
                true => (c.from.trim().to_owned(), c.spec.name.to_ascii_uppercase()),
                false => (c.from.trim().to_owned(), c.spec.name.clone()),
            })
            .collect();
        let renames: Vec<(&str, &str)> = renames
            .iter()
            .map(|(old, new)| (old.as_str(), new.as_str()))
            .collect();
        let mut edits = Vec::new();
        if flavour != Flavour::Clipper && index_path.exists() {
            let file = IndexFile::open(&index_path).map_err(|_| Error::CorruptIndex)?;
            for tag in file.tags("") {
                let expression = rename_words(&tag.expression, &renames);
                let condition = rename_words(&tag.condition, &renames);
                if expression != tag.expression || condition != tag.condition {
                    edits.push((tag.name.clone(), expression, condition));
                }
            }
        }
        drop(opened);
        table::close(session, handle)?;
        let names: Vec<(usize, &str)> = names.iter().map(|(i, name)| (*i, name.as_str())).collect();
        create::rename_dbf_fields(&path, &names, upper).map_err(|_| Error::FileCreate)?;
        let edits: Vec<(&str, &str, &str)> = edits
            .iter()
            .map(|(n, e, c)| (n.as_str(), e.as_str(), c.as_str()))
            .collect();
        if !edits.is_empty() {
            create::cdx_rename_expressions(&index_path, &edits).map_err(|_| Error::CorruptIndex)?;
        }
        crate::cache::evict_path(&path);
        crate::cache::evict_kind(crate::cache::Kind::Tables);
        return Ok(Outcome {
            sources,
            renamed: true,
        });
    }
    // The DBF definitions of the new fields.
    let given = |name: &str| {
        let changed = changes
            .change
            .iter()
            .find(|c| c.spec.name.eq_ignore_ascii_case(name));
        let added = changes
            .add
            .iter()
            .find(|a| a.spec.name.eq_ignore_ascii_case(name));
        changed
            .and_then(|c| c.dbf.as_ref())
            .or_else(|| added.and_then(|a| a.dbf.as_ref()))
    };
    let mut specs: Vec<create::DbfFieldSpec> = fields
        .iter()
        .map(|(spec, source)| dbf_spec(spec, source.map(|n| &old[n - 1]), given(&spec.name)))
        .collect();
    // An AutoInc field kept goes on from its next value and step, as in the
    // original engine (`fixtures/write/sql_dbf`).
    for (dbf, (spec, source)) in specs.iter_mut().zip(&fields) {
        if let Some(n) = source
            && spec.field_type == FieldType::AutoInc
            && old[n - 1].field_type == FieldType::AutoInc
        {
            dbf.autoinc = opened.autoinc_state(&old[n - 1])?;
        }
    }
    // The tags of the structural index file, on the fields by their new
    // names.
    let upper = path.extension().is_some_and(|e| e == "DBF");
    let index_path = path.with_extension(if upper { "CDX" } else { "cdx" });
    let renumber = renumbering(&fields);
    let mut tags = Vec::new();
    if flavour != Flavour::Clipper && index_path.exists() {
        let mut file = IndexFile::open(&index_path).map_err(|_| Error::CorruptIndex)?;
        // Option 0x04 of a FoxPro tag is a custom order's; of a VFP one, a
        // candidate order's.
        let candidate = tflip_storage::order::flags::CANDIDATE;
        check_custom(
            file.tags("")
                .iter()
                .filter(|tag| flavour != Flavour::Visual && tag.flags & candidate != 0),
        )?;
        let all: Vec<_> = file
            .tags("")
            .into_iter()
            .map(|tag| {
                let expression = opened.with_field_numbers(&tag.expression);
                let condition = opened.with_field_numbers(&tag.condition);
                (tag.name.clone(), expression, condition)
            })
            .collect();
        check_in_use(&old, &all, &renumber, &changes.delete)?;
        for tag in file.tags("") {
            let numbered = (
                opened.with_field_numbers(&tag.expression),
                opened.with_field_numbers(&tag.condition),
            );
            let expression = index::renumber_fields(&numbered.0, &renumber);
            let condition = index::renumber_fields(&numbered.1, &renumber);
            let (Some(expression), Some(condition)) = (expression, condition) else {
                return Err(Error::CorruptIndex);
            };
            // A full-text order is made again with its options.
            let block = file.fts_block(&tag).map_err(|_| Error::CorruptIndex)?;
            tags.push((tag, expression, condition, block));
        }
    }
    // The orders of a dictionary's Clipper table, each in a file of its own
    // (a free one's are left as they are).
    let mut ntx = Vec::new();
    if flavour == Flavour::Clipper {
        for file in changes
            .indexes
            .iter()
            .filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("ntx")))
        {
            let stem = file.file_stem().unwrap_or_default().to_string_lossy();
            let Ok(index) = IndexFile::open(file) else {
                continue;
            };
            for mut tag in index.tags(&stem) {
                if let Some((_, expression, condition)) =
                    changes.expressions.iter().find(|(f, ..)| f == file)
                {
                    tag.expression.clone_from(expression);
                    tag.condition.clone_from(condition);
                }
                ntx.push((file.clone(), tag));
            }
        }
        let all: Vec<_> = ntx
            .iter()
            .map(|(_, tag)| {
                let expression = opened.with_field_numbers(&tag.expression);
                let condition = opened.with_field_numbers(&tag.condition);
                (tag.name.clone(), expression, condition)
            })
            .collect();
        check_in_use(&old, &all, &renumber, &changes.delete)?;
    }
    // The expressions as written, but for the fields renamed.
    // (A Clipper or FoxPro table declares its names in upper case.)
    let renames: Vec<(String, String)> = changes
        .change
        .iter()
        .filter(|c| !c.from.eq_ignore_ascii_case(&c.spec.name))
        .map(|c| match flavour == Flavour::Visual {
            true => (c.from.clone(), c.spec.name.clone()),
            false => (c.from.clone(), c.spec.name.to_ascii_uppercase()),
        })
        .collect();
    let renames: Vec<(&str, &str)> = renames
        .iter()
        .map(|(old, new)| (old.as_str(), new.as_str()))
        .collect();
    let memo_block = asked.or_else(|| opened.memo_block_size());
    let date_format = opened.date_format();
    let dates = Dates {
        format: &date_format,
        epoch: opened.session_settings().epoch(),
    };
    let fresh = temporary(&path, "dbf");
    let today = tflip_storage::value::julian_to_ymd(crate::clock::now().local.0);
    // A Visual FoxPro table laid out for AES stays so, unarmed: a dictionary
    // arms the new one after (`fixtures/aes/rc4tp`, `v7`).
    let aes = flavour == Flavour::Visual && opened.dbf_aes_layout();
    let (mut bytes, has_memo) = create::dbf_bytes_layout(flavour, &specs, today, false, aes);
    if free {
        bytes[0x1E] |= 0x02;
    }
    let memo_kind = match flavour {
        Flavour::Clipper => tflip_storage::memo::MemoKind::Dbt,
        _ => tflip_storage::memo::MemoKind::Fpt,
    };
    let fresh_memo = tflip_storage::dbf::memo_path(&fresh, memo_kind);
    let written = (|| {
        std::fs::write(&fresh, &bytes).map_err(|_| Error::FileCreate)?;
        if has_memo {
            let memo = match flavour {
                Flavour::Clipper => create::dbt_bytes(),
                _ => create::fpt_bytes(memo_block.unwrap_or(64) as u16),
            };
            std::fs::write(&fresh_memo, memo).map_err(|_| Error::FileCreate)?;
        }
        let mut file = TableFile::open_for_writing(&fresh, Format::Dbf(flavour))
            .map_err(|_| Error::FileCreate)?;
        let new: Vec<Field> = file.structure.fields.clone();
        for slot in 1..=opened.slot_count() {
            let values = opened.slot_values(slot)?;
            let deleted = opened.slot_deleted(slot)?;
            let mut record = file.structure.null_record();
            for (i, (spec, source)) in fields.iter().enumerate() {
                let field = &new[i];
                let cell = match source {
                    Some(n) => convert(
                        &old[n - 1],
                        spec,
                        values[n - 1].clone(),
                        &date_format,
                        dates.epoch,
                    )
                    .map_err(|error| failed_on(error, field, slot))?,
                    None => Cell::Null,
                };
                match cell {
                    // As in the original engine, a Clipper or FoxPro table's
                    // NULL LongInt and NVarChar values come over as 0 and
                    // empty (`fixtures/write/sql_dbf`).
                    Cell::Null if source.is_some() && flavour != Flavour::Visual => {
                        let at = usize::from(field.offset);
                        let size = usize::from(field.size);
                        match field.field_type {
                            FieldType::LongInt => record[at..at + size].fill(0),
                            FieldType::NVarChar => record[at + size - 2..at + size].fill(0),
                            _ => {}
                        }
                    }
                    // A NULL stays one where the new field can hold it.
                    Cell::Null
                        if source.is_some()
                            && matches!(&field.layout,
                                tflip_storage::field::Layout::Dbf(l) if l.null_bit.is_some()) =>
                    {
                        field
                            .encode(&Value::Null, &mut record)
                            .map_err(|_| failed_on(Error::InvalidFieldType, field, slot))?;
                    }
                    // An ADS VarChar's text past its inline part goes to
                    // the memo file, as when it is written.
                    Cell::Value(Value::Text(text))
                        if field.field_type == FieldType::LegacyVarChar
                            && text.len() > usize::from(field.size) - 2 =>
                    {
                        let bytes = tflip_storage::cp1252::encode(&text);
                        field
                            .store_var_char(&bytes, &mut record, file.memo())
                            .map_err(|_| Error::InvalidFieldType)?;
                    }
                    cell => apply_cell(cell, field, &mut record, dates, &mut file, slot)?,
                }
            }
            if deleted {
                record[0] = b'*';
            }
            file.insert(&record).map_err(|_| Error::CorruptTable)?;
        }
        if !tags.is_empty() {
            file.mark_structural_index()
                .map_err(|_| Error::CorruptTable)?;
        }
        Ok(())
    })();
    discard_on_failure(written, &fresh, &fresh_memo)?;
    // Checked after the records are converted, unlike an ADT table's
    // (`fixtures/write/restructure_fts_cdx`).
    let unfit = tags.iter().any(|(tag, _, _, block)| {
        let name = rename_words(&tag.expression, &renames);
        block.is_some()
            && fields
                .iter()
                .find(|(spec, _)| spec.name.eq_ignore_ascii_case(name.trim()))
                .is_some_and(|(spec, _)| !crate::ddl::can_have_full_text(spec.field_type))
    });
    if unfit {
        let _ = std::fs::remove_file(&fresh);
        let _ = std::fs::remove_file(&fresh_memo);
        return Err(Error::InvalidFieldType
            .detailed("FTS indexes can only be created on character and BLOB fields."));
    }
    drop(opened);
    table::close(session, handle)?;
    let memo_path = tflip_storage::dbf::memo_path(&path, memo_kind);
    let mut aside = vec![path.as_path(), memo_path.as_path()];
    if flavour != Flavour::Clipper {
        aside.push(&index_path);
    }
    swap_in(&aside, &fresh, &path, &fresh_memo, &memo_path)?;
    if tags.is_empty() && ntx.is_empty() {
        return Ok(Outcome {
            sources,
            renamed: false,
        });
    }
    // The tags, on the new fields, built as a reindex builds them.
    let full = path.to_string_lossy();
    let reopen = if connection == 0 { &*full } else { name };
    let reopened = table::open_rebuilding(session, connection, reopen, settings.clone())?;
    let table = table::get(session, reopened)?;
    let visual = flavour == Flavour::Visual;
    let mut cdx_specs = Vec::with_capacity(tags.len());
    let mut ntx_specs = Vec::with_capacity(ntx.len());
    // The order an expression of which no longer compiles, if one.
    let mut failed: Option<PathBuf> = None;
    let shaped = (|| {
        for (tag, _, _, block) in &tags {
            let expression = rename_words(&tag.expression, &renames);
            let condition = rename_words(&tag.condition, &renames);
            if block.is_some() {
                cdx_specs.push(create::CdxSpec {
                    fts: block.clone(),
                    ..create::CdxSpec::from_tag(tag, expression, condition, visual)
                });
                continue;
            }
            let key = table.compile(&expression).map_err(key_expression)?;
            let (key_length, text) = index::cdx_key_shape(&table, &expression, &key)?;
            cdx_specs.push(create::CdxSpec {
                key_length,
                pad: if text { b' ' } else { 0 },
                ..create::CdxSpec::from_tag(tag, expression, condition, visual)
            });
        }
        for (file, tag) in &ntx {
            failed = Some(file.clone());
            let expression = rename_words(&tag.expression, &renames);
            let key = table.compile(&expression).map_err(key_expression)?;
            let (key_length, decimals) = index::ntx_key_shape(&table, &expression, &key)?;
            failed = None;
            ntx_specs.push((
                file.clone(),
                create::NtxSpec {
                    key_length,
                    decimals,
                    expression,
                    condition: rename_words(&tag.condition, &renames),
                    unique: tag.unique(),
                    descending: tag.descending,
                },
            ));
        }
        Ok(())
    })();
    drop(table);
    table::close(session, reopened)?;
    if let Err(error) = shaped {
        // The old table comes back; of a dictionary's orders the files that
        // were being built are gone, and a new memo file stays
        // (`fixtures/sql/ddalterfree`, `ddalterdbf`).
        swap_back(&aside, &path, &memo_path, changes.indexes.is_empty());
        if !changes.indexes.is_empty() {
            let _ = std::fs::remove_file(&index_path);
            if let Some(file) = failed {
                let _ = std::fs::remove_file(file);
            }
            // The tags built before the one that failed stay in the new
            // file (`fixtures/sql/spddalterfail`).
            for spec in &cdx_specs {
                create::cdx_add_tag(&index_path, spec).map_err(|_| Error::CorruptIndex)?;
            }
            for (file, spec) in &ntx_specs {
                std::fs::write(file, create::ntx_bytes(spec)).map_err(|_| Error::FileCreate)?;
            }
            if !cdx_specs.is_empty() || !ntx_specs.is_empty() {
                let _ = reindex_reopened(session, connection, reopen, settings);
            }
        }
        return Err(error);
    }
    for (file, spec) in &ntx_specs {
        std::fs::write(file, create::ntx_bytes(spec)).map_err(|_| Error::FileCreate)?;
    }
    if cdx_specs.is_empty() {
        reindex_reopened(session, connection, reopen, settings)?;
    } else {
        // Each tag is made and built before the next, as `CREATE INDEX`
        // does, not all at once as a reindex does
        // (`fixtures/write/restructure_fts_cdx`).
        let reopened = table::open_rebuilding(session, connection, reopen, settings.clone())?;
        let built = (|| {
            let table = table::get(session, reopened)?;
            for spec in &cdx_specs {
                let header =
                    create::cdx_add_tag(&index_path, spec).map_err(|_| Error::CorruptIndex)?;
                index::build_cdx_tag(&table, &index_path, header, spec)?;
            }
            Ok(())
        })();
        table::close(session, reopened)?;
        built?;
    }
    Ok(Outcome {
        sources,
        renamed: false,
    })
}

/// `text` (an expression) with each word that is one of `renames`' old
/// names, in any case, replaced by its new name; words in quotes and
/// those a `(` or `.` follows (functions, aliases) stay.
pub(crate) fn rename_words(text: &str, renames: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut quote = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            out.push(c);
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
            out.push(c);
            i += 1;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let after = chars[i..].iter().find(|c| !c.is_whitespace());
            let renamed = renames
                .iter()
                .find(|(old, _)| old.eq_ignore_ascii_case(&word));
            match (renamed, after) {
                (Some((_, new)), next) if !matches!(next, Some('(' | '.')) => out.push_str(new),
                _ => out.push_str(&word),
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}
