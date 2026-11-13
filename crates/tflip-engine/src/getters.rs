//! A field's value in the current record as the typed getters give it
//! (`AdsGetLong`, `AdsGetString`, `AdsGetField`, ...), converted as the
//! original engine converts it (`fixtures/fields/getters.json`).
//!
//! Each getter takes some field types and fails with
//! [`Error::InvalidFieldType`] on the others, before it looks at the record
//! (at BOF or EOF, [`Error::NoCurrentRecord`]). NULL is 0, false, or the
//! text `AdsGetField` gives for it.

use serde::{Deserialize, Serialize};
use tflip_storage::dbf::Flavour;
use tflip_storage::field::{self, Field, FieldType};
use tflip_storage::table::Format;
use tflip_storage::value::Value;

use crate::Error;
use crate::format::{self, FieldText, Layout};
use crate::table::{FieldRef, Table};

/// A field, as a client names it: by its 1-based number or by its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldId {
    Number(usize),
    Name(String),
}

impl FieldId {
    pub fn as_ref(&self) -> FieldRef<'_> {
        match self {
            FieldId::Number(n) => FieldRef::Number(*n),
            FieldId::Name(name) => FieldRef::Name(name),
        }
    }
}

/// What a getter reads a field as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Want {
    /// `AdsGetLong`: a whole number of 32 bits.
    Long,
    /// `AdsGetLongLong`.
    LongLong,
    /// `AdsGetShort`: `AdsGetLong`'s cut to its low 16 bits.
    Short,
    /// `AdsGetDouble`.
    Double,
    /// `AdsGetMoney`: × 10,000.
    Money,
    /// `AdsGetLogical`.
    Logical,
    /// `AdsGetJulian`.
    Julian,
    /// `AdsGetMilliseconds`.
    Milliseconds,
    /// `AdsGetMemoLength`: bytes, characters for NMemo.
    MemoLength,
    /// `AdsGetDate`: the date in the date format.
    Date,
    /// `AdsGetTime`: the time, 12-hour.
    Time,
    /// `AdsGetString` (`wide`: `AdsGetStringW`), with `ADS_GET_UTF8` or not.
    String { wide: bool, utf8: bool },
    /// `AdsGetField`, with `ADS_GET_UTF8` or not, and dates in the date
    /// format or as ANSI (`ADS_GET_FORMAT_ANSI`).
    Field { utf8: bool, ansi_dates: bool },
}

/// The character set of a field's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Charset {
    /// Bytes in the client's ANSI code page.
    Ansi,
    /// Unicode: the NChar, NVarChar and NMemo types.
    Unicode,
    /// Bytes in the client's OEM code page: the 8-bit text of a Clipper or
    /// FoxPro table opened with `ADS_OEM`.
    Oem,
}

/// A field's value as a getter gives it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Scalar {
    Long(i32),
    LongLong(i64),
    Short(i16),
    Double(f64),
    Money(i64),
    Logical(bool),
    Julian(i32),
    Milliseconds(u32),
    Length(u32),
    /// The text of a date or a time.
    Text(String),
    /// The text of a field and its character set, untrimmed.
    FieldText {
        text: FieldText,
        charset: Charset,
    },
}

/// The `char_type` of a table opened with `ADS_OEM`.
const OEM: u16 = 2;

impl Want {
    /// Whether a getter reading fields as `self` takes fields of type `t`.
    fn takes(self, t: FieldType) -> bool {
        use FieldType::*;
        match self {
            Want::Long | Want::LongLong | Want::Short | Want::Double | Want::Money => matches!(
                t,
                Numeric
                    | ShortInt
                    | Integer
                    | LongInt
                    | AutoInc
                    | RowVersion
                    | Double
                    | CurDouble
                    | Money
            ),
            Want::Logical => t == Logical,
            Want::Julian | Want::Date => matches!(t, Date | CompactDate | TimeStamp | ModTime),
            Want::Milliseconds | Want::Time => matches!(t, Time | TimeStamp | ModTime),
            Want::MemoLength => matches!(t, Memo | NMemo | Binary | Image),
            Want::String { wide: false, .. } => matches!(
                t,
                Character
                    | CiCharacter
                    | VarChar
                    | LegacyVarChar
                    | NChar
                    | NVarChar
                    | Memo
                    | NMemo
                    | Binary
                    | Image
                    | Raw
                    | VarBinary
                    | Logical
                    | Numeric
                    | Date
            ),
            // `AdsGetStringW` takes the types `AdsGetString` does but
            // VarBinary.
            Want::String { wide: true, utf8 } => {
                t != VarBinary && Want::String { wide: false, utf8 }.takes(t)
            }
            Want::Field { .. } => true,
        }
    }

    fn is_number(self) -> bool {
        matches!(
            self,
            Want::Long | Want::LongLong | Want::Short | Want::Double | Want::Money
        )
    }
}

/// Whether `AdsGetField` reads a field of type `t` with `ADS_GET_UTF8`: as
/// in the original engine, all but Numeric and Logical.
fn takes_utf8(t: FieldType) -> bool {
    !matches!(t, FieldType::Numeric | FieldType::Logical)
}

/// The value of `field` in the current record of `table` as a getter
/// reading it as `want` gives it.
///
/// As in the original engine, the field, its type and the current record
/// are checked in that order (`AdsGetField` with `ADS_GET_UTF8` refuses
/// Numeric and Logical before the record, `AdsGetString` those and Date
/// after it).
pub fn field_as(table: &Table, field: &FieldId, want: Want) -> Result<Scalar, Error> {
    let (number, found) = table.find_field(field.as_ref())?;
    let found = found.clone();
    let dbf = matches!(found.layout, field::Layout::Dbf(_));
    // A DBF table's VarChar keeps its long values in the memo file.
    let var_memo = dbf && want == Want::MemoLength && found.field_type == FieldType::LegacyVarChar;
    if !want.takes(found.field_type) && !var_memo {
        // These name the field (`fixtures/errors/param_n.json`).
        return Err(match want {
            Want::Time | Want::Milliseconds | Want::MemoLength => {
                Error::InvalidFieldType.detailed(format!("Field: \"{}\"", found.name))
            }
            _ => Error::InvalidFieldType,
        });
    }
    if let Want::Field { utf8: true, .. } = want
        && !takes_utf8(found.field_type)
    {
        return Err(Error::InvalidFieldType);
    }
    if matches!(want, Want::Field { .. }) {
        table.reveal(&found);
    }
    if matches!(want, Want::Field { .. } | Want::String { .. }) && table.binary_memo(&found) {
        let why = format!("Memo field contains binary data. Field: \"{}\"", found.name);
        return Err(Error::InvalidFieldType.detailed(why));
    }
    let value = match table.value(FieldRef::Number(number))? {
        // A Clipper or FoxPro LongInt is NULL as its minimum, which the
        // numeric getters read as the number it is.
        Value::Null if dbf && want.is_number() && found.field_type == FieldType::LongInt => {
            Value::Integer(i64::MIN.into())
        }
        value => value,
    };
    let charset = || charset(table, &found);
    let numeric = || to_number(&value);
    Ok(match want {
        Want::Long => Scalar::Long(long(&numeric())?),
        Want::LongLong => Scalar::LongLong(long_long(&numeric())),
        // Cut to the low 16 bits, as the original engine does (123456789
        // is -13035).
        Want::Short => Scalar::Short(long(&numeric())? as i16),
        Want::Double => Scalar::Double(double(&numeric())),
        Want::Money => Scalar::Money(money(&numeric())?),
        Want::Logical => Scalar::Logical(value == Value::Logical(true)),
        Want::Julian => Scalar::Julian(julian(&value)),
        Want::Milliseconds => Scalar::Milliseconds(milliseconds(&value)),
        Want::MemoLength => {
            let bytes = table.memo_length(FieldRef::Number(number))?;
            Scalar::Length(if found.field_type == FieldType::NMemo {
                bytes / 2
            } else {
                bytes
            })
        }
        Want::Date => {
            // A blank VFP date reads as NULL does.
            let date = match value {
                Value::Null | Value::Blank => Value::Null,
                value => Value::Date(julian(&value)),
            };
            Scalar::Text(short_text(table, &date, FieldType::Date))
        }
        Want::Time => {
            let time = Value::Time(milliseconds(&value));
            Scalar::Text(short_text(table, &time, FieldType::Time))
        }
        Want::String { utf8, .. } => {
            // `AdsGetField` takes a Date with `ADS_GET_UTF8`, `AdsGetString`
            // does not.
            if utf8 && (!takes_utf8(found.field_type) || found.field_type == FieldType::Date) {
                return Err(Error::InvalidFieldType);
            }
            Scalar::FieldText {
                text: string_text(&found, &value),
                charset: charset(),
            }
        }
        Want::Field { ansi_dates, .. } => {
            let layout = Layout::from(&found);
            let text = match ansi_dates {
                true => format::ansi_field_text(&value, &layout),
                false => format::field_text(&value, &layout, &table.date_format()),
            };
            Scalar::FieldText {
                text,
                charset: charset(),
            }
        }
    })
}

/// Whether a field's value can be read from a whole-record snapshot without
/// touching the memo file, so that a client may cache it from one read of the
/// current record's fields. Memo-stored fields and a DBF `LegacyVarChar`
/// (whose long values live in the memo file) are read on demand instead, one
/// call each.
pub fn row_cacheable(field: &Field) -> bool {
    !field.is_memo() && field.field_type != FieldType::LegacyVarChar
}

/// The character set of the text of `field`, a field of `table`. As in the
/// original engine, only DBF tables can be OEM, and a Visual FoxPro one only
/// if its dictionary says so (see [`oem_table`]); ADT tables are ANSI
/// whatever they are opened as.
pub fn charset(table: &Table, field: &Field) -> Charset {
    charset_in(&table.settings, &table.structure, field)
}

/// [`charset`] for a field of the table opened with `settings` whose
/// structure is `structure` (as a client knows them from
/// [`crate::table::TableInfo`]).
pub fn charset_in(
    settings: &crate::table::TableSettings,
    structure: &tflip_storage::table::Structure,
    field: &Field,
) -> Charset {
    if matches!(
        field.field_type,
        FieldType::NChar | FieldType::NVarChar | FieldType::NMemo
    ) {
        return Charset::Unicode;
    }
    if oem_table(settings, structure) {
        Charset::Oem
    } else {
        Charset::Ansi
    }
}

/// Whether a table opened with `settings`, of `structure`, keeps its 8-bit
/// text in the client's OEM code page: a DBF table opened with `ADS_OEM`, or
/// that its dictionary says is OEM. A free Visual FoxPro table is ANSI
/// whatever it is opened as ([`crate::table::open`] sees to that), but a
/// dictionary's OEM one converts its text as the others do
/// (`fixtures/dictionary/dbfcorners`).
pub fn oem_table(
    settings: &crate::table::TableSettings,
    structure: &tflip_storage::table::Structure,
) -> bool {
    settings.char_type == OEM
        && (matches!(structure.header.format, Format::Dbf(_)) || settings.oem_text)
}

/// The text of `value` laid out as a field of type `field_type`, in the
/// date format of `table`: `AdsGetDate`'s and `AdsGetTime`'s.
fn short_text(table: &Table, value: &Value, field_type: FieldType) -> String {
    let layout = Layout {
        field_type,
        length: 0,
        decimals: 0,
    };
    match format::field_text(value, &layout, &table.date_format()) {
        FieldText::Text(text) => text,
        FieldText::Bytes(_) => String::new(),
    }
}

/// The text `AdsGetString` and `AdsGetStringW` give for a value of `field`:
/// `AdsGetField`'s, but dates as `CCYYMMDD` whatever the date format, and
/// binary values in base64.
fn string_text(field: &Field, value: &Value) -> FieldText {
    match format::field_text(value, &Layout::from(field), "CCYYMMDD") {
        FieldText::Bytes(bytes) => FieldText::Text(
            String::from_utf8(tflip_expr::base64_encode(&bytes)).unwrap_or_default(),
        ),
        text => text,
    }
}

/// A number as the numeric getters read it.
enum Number {
    Integer(i128),
    Double(f64),
    /// × 10,000.
    Money(i64),
}

/// The number `value` holds: NULL and a blank Numeric are 0.
fn to_number(value: &Value) -> Number {
    match value {
        Value::Integer(n) => Number::Integer(*n),
        Value::Double(d) => Number::Double(*d),
        Value::Money(m) => Number::Money(*m),
        Value::Numeric(text) => match text.trim() {
            "" => Number::Integer(0),
            text => Number::Double(text.parse().unwrap_or(0.0)),
        },
        _ => Number::Integer(0),
    }
}

/// Money (× 10,000) as a whole number, halves rounded away from zero, as
/// the original engine converts it (where it truncates doubles).
fn whole_money(m: i64) -> i64 {
    let m = i128::from(m);
    let rounded = (m + m.signum() * 5_000) / 10_000;
    rounded as i64
}

/// The number as a whole number, if it fits 32 bits; else
/// [`Error::ValueOverflow`]. Doubles are truncated toward zero, Money
/// rounded (see [`whole_money`]).
fn long(number: &Number) -> Result<i32, Error> {
    match *number {
        Number::Integer(n) => i32::try_from(n).map_err(|_| Error::ValueOverflow),
        Number::Money(m) => i32::try_from(whole_money(m)).map_err(|_| Error::ValueOverflow),
        Number::Double(d) => {
            let d = d.trunc();
            match d >= f64::from(i32::MIN) && d <= f64::from(i32::MAX) {
                true => Ok(d as i32),
                false => Err(Error::ValueOverflow),
            }
        }
    }
}

/// The number as a whole number, as [`long`] takes it; a double beyond 64
/// bits is `i64::MIN`, as the original engine's conversion leaves it.
fn long_long(number: &Number) -> i64 {
    match *number {
        Number::Integer(n) => i64::try_from(n).unwrap_or(i64::MIN),
        Number::Money(m) => whole_money(m),
        Number::Double(d) => {
            let d = d.trunc();
            if d >= -(2f64.powi(63)) && d < 2f64.powi(63) {
                d as i64
            } else {
                i64::MIN
            }
        }
    }
}

fn double(number: &Number) -> f64 {
    match *number {
        Number::Integer(n) => n as f64,
        Number::Money(m) => m as f64 / 10_000.0,
        Number::Double(d) => d,
    }
}

/// The number × 10,000, rounded, if it fits 64 bits; else
/// [`Error::ValueOverflow`].
fn money(number: &Number) -> Result<i64, Error> {
    match *number {
        Number::Integer(n) => n
            .checked_mul(10_000)
            .and_then(|m| i64::try_from(m).ok())
            .ok_or(Error::ValueOverflow),
        Number::Money(m) => Ok(m),
        Number::Double(d) => {
            let m = (d * 10_000.0).round();
            match m >= -(2f64.powi(63)) && m < 2f64.powi(63) {
                true => Ok(m as i64),
                false => Err(Error::ValueOverflow),
            }
        }
    }
}

/// The Julian day of a Date, TimeStamp or ModTime value; 0 for NULL.
fn julian(value: &Value) -> i32 {
    match *value {
        Value::Date(julian) | Value::TimeStamp(julian, _) => julian,
        _ => 0,
    }
}

/// The milliseconds since midnight of a Time, TimeStamp or ModTime value;
/// 0 for NULL.
fn milliseconds(value: &Value) -> u32 {
    match *value {
        Value::Time(ms) | Value::TimeStamp(_, ms) => ms,
        _ => 0,
    }
}

/// Whether a field is NULL (`AdsIsNull`) or empty (`AdsIsEmpty`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Null {
    /// On a record: whether it is.
    Is(bool),
    /// Off a record ([`Error::NoCurrentRecord`]), and whether the original
    /// engine writes `false` before failing.
    OffRecord { writes_false: bool },
}

/// Whether `field` is NULL in the current record of `table`, or with
/// `empty` empty (see [`Table::is_null`], [`Table::is_empty`]).
///
/// Off a record the original engine writes `false` before it fails:
/// always, but for `AdsIsNull` in a VFP table, and in a Clipper or FoxPro
/// table on the Advantage types that have a NULL of their own (LongInt,
/// GUID and the Unicode types).
pub fn null(table: &Table, field: &FieldId, empty: bool) -> Result<Null, Error> {
    let (number, found) = table.find_field(field.as_ref())?;
    let field = FieldRef::Number(number);
    let result = if empty {
        table.is_empty(field)
    } else {
        table.is_null(field)
    };
    match result {
        Ok(is) => Ok(Null::Is(is)),
        Err(Error::NoCurrentRecord) => {
            let writes_false = empty
                || match &found.layout {
                    field::Layout::Adt => true,
                    field::Layout::Dbf(layout) => {
                        use FieldType::*;
                        layout.flavour != Flavour::Visual
                            && !matches!(
                                found.field_type,
                                LongInt | Guid | NChar | NVarChar | NMemo
                            )
                    }
                };
            Ok(Null::OffRecord { writes_false })
        }
        Err(error) => Err(error),
    }
}
