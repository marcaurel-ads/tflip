//! Field values a client sets (`AdsSetString`, `AdsSetLong`, `AdsSetField`,
//! ...), written into a record's bytes as the original engine writes them in
//! an ADT table (`fixtures/write/setters`, every setter on every type) and in
//! DBF tables (`fixtures/write/dbf_setters`).
//!
//! Each setter takes what it takes for each type: text for the character
//! types through `AdsSetString`, `AdsSetField` and `AdsSetTimeStamp` alike;
//! numbers, dates and times through their own setters, and through
//! `AdsSetField` as text, read leniently as C reads numbers (`atol`, `strtod`)
//! for the integer and floating-point types and strictly for Numeric, Money
//! and LongInt. A setter a type does not take is [`Error::InvalidFieldType`],
//! and changes nothing. Some writes depend on what the field held: a VarChar
//! is padded with spaces only as far as its old value went, base64 text sets
//! only the bytes it decodes to in a Raw field, and a time sets only the time
//! of a TimeStamp. AutoInc, RowVersion and ModTime keep their values: the
//! engine fills them in.
//!
//! DBF tables take values as ADT tables do, with these differences: a
//! Numeric needs no room for a sign; a Clipper or FoxPro table refuses
//! `AdsSetMoney` ([`Error::InvalidTableType`]); NULL is stored as the
//! field's blank value, or in a VFP table as its NULL bit (which only
//! fields declared NULL have); `AdsSetEmpty` stores the blank value, and so
//! does an empty date; a VFP VarChar or VarBinary is padded with spaces.

use serde::{Deserialize, Serialize};
use tflip_storage::dbf::Flavour;
use tflip_storage::field::{Field, FieldType, Layout};
use tflip_storage::value::{Value, Ymd, ymd_to_julian};

use crate::Error;

/// A value as a client sets it, by the setter it calls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SetValue {
    /// `AdsSetString(W)`: text for the character fields, base64 for Raw.
    String(String),
    /// `AdsSetField(W)`: text for any field, read as its type is written.
    Field(String),
    /// `AdsSetShort`.
    Short(i16),
    /// `AdsSetLong`.
    Long(i32),
    /// `AdsSetLongLong`.
    LongLong(i64),
    /// `AdsSetDouble`.
    Double(f64),
    /// `AdsSetLogical`.
    Logical(bool),
    /// `AdsSetDate`: a date in the date format.
    Date(String),
    /// `AdsSetJulian`.
    Julian(i32),
    /// `AdsSetTime`: a time of day.
    Time(String),
    /// `AdsSetMilliseconds`.
    Milliseconds(i32),
    /// `AdsSetTimeStamp`: a date in the date format and a time of day.
    TimeStamp(String),
    /// `AdsSetTimeStampRaw`: a Julian day and milliseconds.
    TimeStampRaw(i32, u32),
    /// `AdsSetMoney`: ten-thousandths.
    Money(i64),
    /// `AdsSetBinary` with `ADS_BINARY`.
    Binary(Vec<u8>),
    /// `AdsSetBinary` with `ADS_IMAGE`, which only Image fields take.
    Image(Vec<u8>),
    /// `AdsSetNull`.
    Null,
    /// `AdsSetEmpty`.
    Empty,
    /// `AdsSetFieldRaw` (unpublished): the field's bytes as the record holds
    /// them.
    Raw(Vec<u8>),
}

/// How dates are read: the date format and the epoch of two-digit years.
#[derive(Debug, Clone, Copy)]
pub struct Dates<'a> {
    pub format: &'a str,
    pub epoch: u16,
}

/// Milliseconds in a day.
const DAY_MS: u32 = 86_400_000;

/// Writes `value` into `field` in `record` (a field outside the memo file).
/// A value cut to fit is written, and then [`Error::DataTruncated`]; any
/// other error leaves `record` as it was.
pub fn apply(
    field: &Field,
    value: &SetValue,
    dates: Dates,
    record: &mut [u8],
) -> Result<(), Error> {
    let start = usize::from(field.offset);
    let range = start..start + usize::from(field.size);
    // Raw bytes go in as they are, cut to the field (then `DataTruncated`,
    // as the original writes the cut value) or padded: text with blanks, an
    // ADT field of another type keeps its old bytes after them
    // (`fixtures/errors/param_n`).
    if let SetValue::Raw(bytes) = value {
        let slot = record.get_mut(range).ok_or(Error::CorruptTable)?;
        // A Numeric field of an ADT table reads them as the text of a
        // number, up to a NUL, and sets it as `AdsSetField` does: bytes that
        // are no number fail (5070), as does a number too large for it
        // (5179).
        if field.field_type == FieldType::Numeric
            && matches!(field.layout, Layout::Adt)
            && bytes.len() <= slot.len()
        {
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            let mut text = tflip_storage::cp1252::decode(&bytes[..end]);
            // A sign or a point alone, or nothing, is zero.
            let digits = text.trim();
            let digits = digits.strip_prefix(['+', '-']).unwrap_or(digits);
            if digits.strip_prefix('.').unwrap_or(digits).is_empty() {
                text = "0".to_owned();
            }
            return apply_slot(field, &SetValue::Field(text), dates, true, slot);
        }
        // The variable-length types store more than they take, and the
        // Unicode ones two bytes a character.
        let limit = match field.field_type {
            FieldType::NChar | FieldType::NVarChar => 2 * usize::from(field.length()),
            FieldType::VarChar | FieldType::VarBinary => usize::from(field.length()),
            _ => slot.len(),
        }
        .min(slot.len());
        // A VarChar value longer than the field is taken as 2 bytes shorter
        // than it is, and the two bytes after the text that tell its
        // length say so (`fixtures/errors/param_n`).
        let too_long = bytes.len() > limit;
        let (bytes, stored) = match (field.field_type, too_long) {
            (FieldType::VarChar, true) => {
                let shorter = &bytes[..bytes.len().saturating_sub(2)];
                (shorter, Some(shorter.len().min(slot.len())))
            }
            (FieldType::VarChar, false) => (&bytes[..], Some(bytes.len())),
            _ => (&bytes[..], None),
        };
        let cut = bytes.len().min(limit);
        slot[..cut].copy_from_slice(&bytes[..cut]);
        match field.field_type {
            FieldType::Character | FieldType::CiCharacter | FieldType::VarChar => {
                slot[cut..limit].fill(b' ')
            }
            FieldType::NChar => {
                for (i, byte) in slot.iter_mut().enumerate().skip(cut) {
                    *byte = if i % 2 == 0 { b' ' } else { 0 };
                }
            }
            // A DBF number or date is blank where the bytes end.
            FieldType::Numeric | FieldType::Date if matches!(field.layout, Layout::Dbf(_)) => {
                slot[cut..].fill(b' ')
            }
            _ if matches!(field.layout, Layout::Adt) => {}
            _ => slot[cut..].fill(0),
        }
        if let Some(stored) = stored {
            let length = u16::try_from(stored).unwrap_or(u16::MAX).to_le_bytes();
            slot[limit..limit + 2].copy_from_slice(&length);
        }
        return if too_long {
            Err(Error::DataTruncated)
        } else {
            Ok(())
        };
    }
    let Layout::Dbf(layout) = &field.layout else {
        let slot = record.get_mut(range).ok_or(Error::CorruptTable)?;
        return apply_slot(field, value, dates, true, slot);
    };
    use FieldType::*;
    use SetValue as V;
    let vfp = layout.flavour == Flavour::Visual;
    let encode = |record: &mut [u8], value: &Value| {
        field
            .encode(value, record)
            .map_err(|_| Error::InvalidFieldType)
    };
    match (field.field_type, value) {
        (_, V::Money(_)) if !vfp => Err(Error::InvalidTableType),
        // The engine fills an AutoInc in.
        (AutoInc, V::Null | V::Empty) => Ok(()),
        (_, V::Null) if !field.nullable() => Err(Error::NotVfpNullable),
        (_, V::Null) => encode(record, &Value::Null),
        (_, V::Empty) => field
            .encode_blank(record)
            .map_err(|_| Error::InvalidFieldType),
        (Date | CompactDate, V::Field(t) | V::TimeStamp(t) | V::Date(t)) => {
            let day = date(t, dates)?;
            encode(record, &day.map_or(Value::Blank, Value::Date))
        }
        (Date | CompactDate, V::Julian(day)) => encode(record, &Value::Date(*day)),
        (VarChar, V::String(t) | V::Field(t) | V::TimeStamp(t)) => {
            let bytes = tflip_storage::cp1252::encode(t);
            let cut = &bytes[..bytes.len().min(usize::from(field.size))];
            encode(record, &Value::Text(tflip_storage::cp1252::decode(cut)))?;
            if cut.len() < bytes.len() {
                Err(Error::DataTruncated)
            } else {
                Ok(())
            }
        }
        (VarBinary, V::String(_) | V::Field(_) | V::TimeStamp(_) | V::Binary(_)) => {
            let bytes = match value {
                V::Binary(bytes) => bytes.clone(),
                _ => tflip_storage::cp1252::encode(t_of(value)),
            };
            let cut = &bytes[..bytes.len().min(usize::from(field.size))];
            encode(record, &Value::Binary(cut.to_vec()))?;
            if cut.len() < bytes.len() {
                Err(Error::DataTruncated)
            } else {
                Ok(())
            }
        }
        // An ADS VarChar: text that fits inline (longer text continues in
        // the memo file, which the table writes).
        (LegacyVarChar, V::String(t) | V::Field(t) | V::TimeStamp(t)) => {
            let bytes = tflip_storage::cp1252::encode(t);
            if bytes.len() > usize::from(field.size) - 2 {
                return Err(Error::NotImplemented);
            }
            encode(record, &Value::Text(t.clone()))
        }
        (Date | CompactDate | VarChar | VarBinary | LegacyVarChar, _) => {
            Err(Error::InvalidFieldType)
        }
        _ => {
            // Stored as in an ADT table.
            let mut slot = record
                .get(range.clone())
                .ok_or(Error::CorruptTable)?
                .to_vec();
            let result = apply_slot(field, value, dates, false, &mut slot);
            if matches!(result, Ok(()) | Err(Error::DataTruncated)) {
                record[range].copy_from_slice(&slot);
                field.set_not_null(record);
            }
            result
        }
    }
}

/// The bytes a text setter sets an ADS VarChar to; `None` for other
/// setters.
pub fn var_char_text(value: &SetValue) -> Option<Vec<u8>> {
    match value {
        SetValue::String(t) | SetValue::Field(t) | SetValue::TimeStamp(t) => {
            Some(tflip_storage::cp1252::encode(t))
        }
        _ => None,
    }
}

/// The text of a text setter's value.
fn t_of(value: &SetValue) -> &str {
    match value {
        SetValue::String(t) | SetValue::Field(t) | SetValue::TimeStamp(t) => t,
        _ => "",
    }
}

/// [`apply`] on `slot`, the bytes of `field` in a record, as an ADT table
/// stores them; a Numeric keeps room for a sign if `sign_room`.
fn apply_slot(
    field: &Field,
    value: &SetValue,
    dates: Dates,
    sign_room: bool,
    slot: &mut [u8],
) -> Result<(), Error> {
    use FieldType::*;
    use SetValue as V;
    match (field.field_type, value) {
        (_, V::Null | V::Empty) => null_into(field, slot),
        (Character | CiCharacter | VarChar | NChar | NVarChar, V::String(t) | V::Field(t))
        | (Character | CiCharacter | VarChar | NChar | NVarChar, V::TimeStamp(t)) => {
            text_into(field, t, slot)
        }
        // An ADS VarChar's text that fits inline (the table writes longer
        // text, whose rest goes to the memo file).
        (LegacyVarChar, V::String(t) | V::Field(t) | V::TimeStamp(t)) => {
            let bytes = tflip_storage::cp1252::encode(t);
            if bytes.len() > slot.len().saturating_sub(2) {
                return Err(Error::NotImplemented);
            }
            tflip_storage::adt::store_legacy_var_char(&bytes, slot, None)
                .map_err(|_| Error::CorruptTable)
        }
        (Raw, V::String(t)) => base64_into(t, slot),
        (Raw, V::Field(t) | V::TimeStamp(t)) => {
            bytes_into(field, &tflip_storage::cp1252::encode(t), slot)
        }
        (VarBinary, V::String(t) | V::Field(t) | V::TimeStamp(t)) => {
            bytes_into(field, &tflip_storage::cp1252::encode(t), slot)
        }
        (VarBinary, V::Binary(bytes)) => bytes_into(field, bytes, slot),
        (Guid, V::Field(t) | V::TimeStamp(t)) => store(field, &Value::Guid(guid(t)?), slot),
        (Logical, V::String(t)) => {
            store(field, &Value::Logical(logical(t)?), slot)?;
            if t.chars().count() > 1 {
                Err(Error::DataTruncated)
            } else {
                Ok(())
            }
        }
        (Logical, V::Field(t) | V::TimeStamp(t)) => {
            store(field, &Value::Logical(logical(t)?), slot)
        }
        (Logical, V::Logical(b)) => store(field, &Value::Logical(*b), slot),
        (Numeric, V::Field(t) | V::TimeStamp(t)) => {
            let text = t.trim();
            let n = match text.is_empty() {
                true => Decimal::ZERO,
                false => Decimal::parse(text).ok_or(Error::InvalidDataType)?,
            };
            numeric_into(field, n, sign_room, slot)
        }
        (Numeric, n) if number(n).is_some() => {
            numeric_into(field, number(n).unwrap(), sign_room, slot)
        }
        // A Double too large for any number of digits.
        (Numeric, V::Double(d)) if d.is_finite() => Err(Error::ValueOverflow),
        (ShortInt | Integer, V::Field(t) | V::TimeStamp(t)) => {
            // As C's `atol` reads it, into a 32-bit `long`, then cut to the
            // field's width.
            let n = atol(t) as i32;
            integer_into(field, i64::from(n), true, slot)
        }
        (ShortInt | Integer | LongInt, V::Short(n)) => {
            integer_into(field, i64::from(*n), true, slot)
        }
        (ShortInt | Integer | LongInt, V::Long(n)) => {
            integer_into(field, i64::from(*n), true, slot)
        }
        (ShortInt | Integer | LongInt, V::LongLong(n)) => integer_into(field, *n, false, slot),
        (ShortInt | Integer | LongInt, V::Double(d)) => {
            let whole = d.trunc();
            if !(-9.3e18..9.3e18).contains(&whole) {
                return Err(Error::ValueOverflow);
            }
            integer_into(field, whole as i64, false, slot)
        }
        (ShortInt | Integer | LongInt, V::Money(m)) => integer_into(field, m / 10_000, false, slot),
        (LongInt, V::Field(t) | V::TimeStamp(t)) => {
            let text = t.trim();
            let n = match text.is_empty() {
                true => 0,
                false => match text.parse::<i64>() {
                    Ok(n) => n,
                    Err(_) if Decimal::parse(text).is_some() => return Err(Error::ValueOverflow),
                    Err(_) => return Err(Error::InvalidData),
                },
            };
            integer_into(field, n, false, slot)
        }
        (Double | CurDouble, V::Field(t) | V::TimeStamp(t)) => {
            store(field, &Value::Double(strtod(t)), slot)
        }
        (Double | CurDouble, V::Short(n)) => store(field, &Value::Double(f64::from(*n)), slot),
        (Double | CurDouble, V::Long(n)) => store(field, &Value::Double(f64::from(*n)), slot),
        (Double | CurDouble, V::LongLong(n)) => store(field, &Value::Double(*n as f64), slot),
        (Double | CurDouble, V::Double(d)) => store(field, &Value::Double(*d), slot),
        (Double | CurDouble, V::Money(m)) => {
            store(field, &Value::Double(*m as f64 / 10_000.0), slot)
        }
        (Money, V::Field(t) | V::TimeStamp(t)) => {
            let n = Decimal::parse(t.trim()).ok_or(Error::InvalidDataType)?;
            money_into(field, n, slot)
        }
        (Money, V::Money(m)) => store(field, &Value::Money(*m), slot),
        (Money, n) if number(n).is_some() => money_into(field, number(n).unwrap(), slot),
        (Date, V::Field(t) | V::TimeStamp(t) | V::Date(t)) => match date(t, dates)? {
            Some(day) => store(field, &Value::Date(day), slot),
            None => null_into(field, slot),
        },
        (Date, V::Julian(day)) => store(field, &Value::Date(*day), slot),
        (Time, V::Field(t) | V::TimeStamp(t) | V::Time(t)) => {
            store(field, &Value::Time(time(t)?), slot)
        }
        (Time, V::Milliseconds(ms)) => store(field, &Value::Time(milliseconds(*ms)?), slot),
        (TimeStamp, V::Field(t) | V::TimeStamp(t)) => match timestamp(t, dates)? {
            Some((day, ms)) => store(field, &Value::TimeStamp(day, ms), slot),
            None => null_into(field, slot),
        },
        (TimeStamp, V::Date(t)) => match date(t, dates)? {
            Some(day) => store(field, &Value::TimeStamp(day, 0), slot),
            None => null_into(field, slot),
        },
        (TimeStamp, V::Julian(day)) => store(field, &Value::TimeStamp(*day, 0), slot),
        (TimeStamp, V::Time(t)) => time_of_day_into(time(t)?, slot),
        (TimeStamp, V::Milliseconds(ms)) => time_of_day_into(milliseconds(*ms)?, slot),
        (TimeStamp, V::TimeStampRaw(day, ms)) => store(field, &Value::TimeStamp(*day, *ms), slot),
        // The engine fills these in; what a client sets is taken and dropped,
        // a ModTime's after it is read.
        (AutoInc | RowVersion, V::String(_)) => Err(Error::InvalidFieldType),
        (AutoInc | RowVersion, V::Field(_) | V::TimeStamp(_) | V::Money(_) | V::Double(_))
        | (AutoInc | RowVersion, V::Short(_) | V::Long(_) | V::LongLong(_)) => Ok(()),
        (ModTime, V::Field(t) | V::TimeStamp(t)) => timestamp(t, dates).map(|_| ()),
        (ModTime, V::Milliseconds(ms)) => milliseconds(*ms).map(|_| ()),
        (ModTime, V::Date(_) | V::Julian(_) | V::Time(_)) => Ok(()),
        _ => Err(Error::InvalidFieldType),
    }
}

/// Whether a set that fails with `error` has started the record's update in the
/// original, so that `AdsCancelUpdate` after it succeeds (probed,
/// `fixtures/write/set_errors`): a value too long or out of range does, and so
/// does one a Numeric field or a binary field cannot read and a NULL for a
/// field that takes none; a field of the wrong type, a bad date, time, GUID,
/// logical or Money value does not, and neither does text out of range for a
/// LongInt.
pub fn begins_update(field: &Field, value: &SetValue, error: &Error) -> bool {
    match error {
        Error::DataTruncated | Error::CorruptMemo | Error::NotVfpNullable => true,
        Error::ValueOverflow => {
            !(field.field_type == FieldType::LongInt
                && matches!(value, SetValue::Field(_) | SetValue::TimeStamp(_)))
        }
        Error::InvalidDataType => field.field_type == FieldType::Numeric,
        Error::InvalidValue => matches!(
            field.field_type,
            FieldType::Raw | FieldType::VarBinary | FieldType::Binary | FieldType::Image
        ),
        _ => false,
    }
}

/// Writes `value` as the field stores it.
fn store(field: &Field, value: &Value, slot: &mut [u8]) -> Result<(), Error> {
    tflip_storage::adt::encode(field, value, slot).map_err(|_| Error::InvalidFieldType)
}

/// Stores an explicit AutoInc value a client set with enforcement off
/// (`AdsDisableAutoIncEnforcement`) into `field` in `record`, read as the
/// integer setters read their value. A value `field` cannot take is
/// [`Error::InvalidFieldType`]; one too large is [`Error::ValueOverflow`].
pub fn set_autoinc(field: &Field, value: &SetValue, record: &mut [u8]) -> Result<(), Error> {
    use SetValue as V;
    let n: i64 = match value {
        V::Short(n) => i64::from(*n),
        V::Long(n) => i64::from(*n),
        V::LongLong(n) => *n,
        V::Double(d) => {
            let whole = d.trunc();
            if !(-9.3e18..9.3e18).contains(&whole) {
                return Err(Error::ValueOverflow);
            }
            whole as i64
        }
        V::Money(m) => m / 10_000,
        V::Field(t) | V::TimeStamp(t) => atol(t),
        _ => return Err(Error::InvalidFieldType),
    };
    let start = usize::from(field.offset);
    let slot = record
        .get_mut(start..start + usize::from(field.size))
        .ok_or(Error::CorruptTable)?;
    integer_into(field, n, false, slot)
}

/// Sets a field to NULL. AutoInc and ModTime keep their values; a VarChar,
/// NVarChar or VarBinary keeps its data, with the length 0xFFFF that means
/// NULL.
fn null_into(field: &Field, slot: &mut [u8]) -> Result<(), Error> {
    match field.field_type {
        FieldType::AutoInc | FieldType::ModTime => Ok(()),
        FieldType::VarChar | FieldType::NVarChar | FieldType::VarBinary => {
            let end = slot.len();
            slot[end - 2..].fill(0xFF);
            Ok(())
        }
        _ => store(field, &Value::Null, slot),
    }
}

/// Text for a character field, cut to its length.
fn text_into(field: &Field, text: &str, slot: &mut [u8]) -> Result<(), Error> {
    let limit = usize::from(field.length());
    let (text, truncated) = match field.field_type {
        FieldType::NChar | FieldType::NVarChar => {
            let units: Vec<u16> = text.encode_utf16().collect();
            let cut = units.len() > limit;
            (
                String::from_utf16_lossy(&units[..units.len().min(limit)]),
                cut,
            )
        }
        _ => {
            let cut = text.chars().count() > limit;
            (text.chars().take(limit).collect(), cut)
        }
    };
    if field.field_type == FieldType::VarChar {
        // The data, spaces as far as the old value went, and the length.
        let bytes = tflip_storage::cp1252::encode(&text);
        let end = slot.len();
        let old = usize::from(u16::from_le_bytes([slot[end - 2], slot[end - 1]]));
        slot[..bytes.len()].copy_from_slice(&bytes);
        if old != 0xFFFF && old > bytes.len() {
            slot[bytes.len()..old.min(end - 2)].fill(b' ');
        }
        slot[end - 2..].copy_from_slice(&(bytes.len() as u16).to_le_bytes());
    } else {
        store(field, &Value::Text(text), slot)?;
    }
    if truncated {
        Err(Error::DataTruncated)
    } else {
        Ok(())
    }
}

/// Bytes for a Raw or VarBinary field, cut to its size and padded with
/// zeros.
fn bytes_into(field: &Field, bytes: &[u8], slot: &mut [u8]) -> Result<(), Error> {
    let limit = usize::from(field.length());
    let cut = &bytes[..bytes.len().min(limit)];
    store(field, &Value::Binary(cut.to_vec()), slot)?;
    if bytes.len() > limit {
        Err(Error::DataTruncated)
    } else {
        Ok(())
    }
}

/// The value a memo field (Memo, NMemo, Binary, Image) is set to, as the
/// memo file keeps it: the text's bytes (UTF-16LE in an NMemo), or the
/// bytes; `None` for NULL, which empty text also is. Text comes from
/// `AdsSetString`, `AdsSetField` and `AdsSetTimeStamp`, base64 for a binary
/// field through `AdsSetString`; bytes from `AdsSetBinary`.
pub fn memo_value(field: &Field, value: &SetValue) -> Result<Option<Vec<u8>>, Error> {
    use FieldType::*;
    use SetValue as V;
    let bytes = match (field.field_type, value) {
        (_, V::Null | V::Empty) => return Ok(None),
        (Memo | NMemo, V::String(t) | V::Field(t) | V::TimeStamp(t)) => {
            tflip_storage::field::memo_text_bytes(field.field_type, t)
        }
        (Binary | Image, V::String(t)) => base64(t, false)?,
        (Binary | Image, V::Field(t) | V::TimeStamp(t)) => tflip_storage::cp1252::encode(t),
        (Binary | Image, V::Binary(bytes)) | (Image, V::Image(bytes)) => bytes.clone(),
        // A VFP Binary field takes an image too.
        (Binary, V::Image(bytes)) if vfp(field) => bytes.clone(),
        _ => return Err(Error::InvalidFieldType),
    };
    Ok((!bytes.is_empty()).then_some(bytes))
}

/// Whether `field` is a VFP table's.
fn vfp(field: &Field) -> bool {
    matches!(&field.layout, Layout::Dbf(layout) if layout.flavour == Flavour::Visual)
}

/// The type a binary value set on a text Memo field of a Clipper or FoxPro
/// table is stored as, which those fields take: Binary (`AdsSetBinary`) or
/// Image (`AdsSetBinary` with `ADS_IMAGE`). `None` for other fields and
/// values.
pub fn binary_memo(field: &Field, value: &SetValue) -> Option<FieldType> {
    let xbase = matches!(&field.layout, Layout::Dbf(layout) if layout.flavour != Flavour::Visual);
    match value {
        _ if !xbase || field.field_type != FieldType::Memo => None,
        SetValue::Binary(_) => Some(FieldType::Binary),
        SetValue::Image(_) => Some(FieldType::Image),
        _ => None,
    }
}

/// Base64 text for a Raw field (`AdsSetString`): its bytes replace the
/// field's first bytes, the rest stays; a last group of fewer than four
/// characters writes the bytes its bits start. See [`base64`].
fn base64_into(text: &str, slot: &mut [u8]) -> Result<(), Error> {
    let bytes = base64(text, true)?;
    let n = bytes.len().min(slot.len());
    slot[..n].copy_from_slice(&bytes[..n]);
    if bytes.len() > slot.len() {
        Err(Error::DataTruncated)
    } else {
        Ok(())
    }
}

/// The bytes base64 text decodes to: of a last group of fewer than four
/// characters, the whole bytes its bits make, and with `partial` the byte
/// its last bits start too. A character that is not base64 is
/// [`Error::InvalidValue`].
fn base64(text: &str, partial: bool) -> Result<Vec<u8>, Error> {
    let sextet = |c: char| match c {
        'A'..='Z' => Some(c as u32 - 'A' as u32),
        'a'..='z' => Some(c as u32 - 'a' as u32 + 26),
        '0'..='9' => Some(c as u32 - '0' as u32 + 52),
        '+' => Some(62),
        '/' => Some(63),
        _ => None,
    };
    let text = text.trim_end_matches('=');
    let sextets: Vec<u32> = text
        .chars()
        .map(sextet)
        .collect::<Option<_>>()
        .ok_or(Error::InvalidValue)?;
    let mut bytes = Vec::new();
    for group in sextets.chunks(4) {
        let value = group.iter().fold(0u32, |acc, s| acc << 6 | s) << (6 * (4 - group.len()));
        let bits = group.len() * 6;
        let count = if partial { bits.div_ceil(8) } else { bits / 8 };
        bytes.extend_from_slice(&value.to_be_bytes()[1..1 + count]);
    }
    Ok(bytes)
}

/// A logical's text: its first character, `T`, `t`, `Y`, `y` or `1` for
/// true, `F`, `f`, `N`, `n`, `0` or a space for false; anything else is
/// [`Error::InvalidDataType`].
fn logical(text: &str) -> Result<bool, Error> {
    match text.chars().next() {
        Some('T' | 't' | 'Y' | 'y' | '1') => Ok(true),
        Some('F' | 'f' | 'N' | 'n' | '0' | ' ') => Ok(false),
        _ => Err(Error::InvalidDataType),
    }
}

/// An integer for a ShortInt, Integer or LongInt field. With `wrap`, a value
/// too large for the field is cut to its width (as `AdsSetLong` on a
/// ShortInt does); else it is [`Error::ValueOverflow`].
fn integer_into(field: &Field, n: i64, wrap: bool, slot: &mut [u8]) -> Result<(), Error> {
    let n = match field.field_type {
        FieldType::ShortInt if wrap => i64::from(n as i16),
        FieldType::ShortInt => i64::from(i16::try_from(n).map_err(|_| Error::ValueOverflow)?),
        FieldType::Integer => i64::from(i32::try_from(n).map_err(|_| Error::ValueOverflow)?),
        _ => n,
    };
    store(field, &Value::Integer(i128::from(n)), slot)
}

/// The number a numeric setter gives, as a decimal.
fn number(value: &SetValue) -> Option<Decimal> {
    match value {
        SetValue::Short(n) => Some(Decimal::from_int(i128::from(*n))),
        SetValue::Long(n) => Some(Decimal::from_int(i128::from(*n))),
        SetValue::LongLong(n) => Some(Decimal::from_int(i128::from(*n))),
        SetValue::Double(d) => Decimal::from_f64(*d),
        SetValue::Money(m) => Some(Decimal {
            digits: i128::from(*m),
            scale: 4,
        }),
        _ => None,
    }
}

/// A number for a Numeric field: rounded half away from zero to its
/// decimals, right-aligned; one too wide for the field is
/// [`Error::ValueOverflow`]. A positive value leaves the position of a sign
/// free (NUMERIC(5, 0) holds -9999 to 9999).
fn numeric_into(field: &Field, n: Decimal, sign_room: bool, slot: &mut [u8]) -> Result<(), Error> {
    let decimals = u32::from(field.decimals());
    let scaled = n.rounded(decimals).ok_or(Error::ValueOverflow)?;
    let text = numeric_text(scaled, decimals);
    let signless = scaled > 0 || !text.starts_with('-');
    let room = usize::from(field.length()) - usize::from(sign_room && signless);
    if text.len() > room {
        return Err(Error::ValueOverflow);
    }
    store(field, &Value::Numeric(text), slot)
}

/// A number for a Money field: in ten-thousandths, rounded half away from
/// zero.
fn money_into(field: &Field, n: Decimal, slot: &mut [u8]) -> Result<(), Error> {
    let m = n
        .rounded(4)
        .and_then(|m| i64::try_from(m).ok())
        .filter(|&m| m != i64::MIN)
        .ok_or(Error::ValueOverflow)?;
    store(field, &Value::Money(m), slot)
}

/// A decimal number: `digits` × 10^-`scale`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Decimal {
    digits: i128,
    scale: u32,
}

impl Decimal {
    const ZERO: Decimal = Decimal {
        digits: 0,
        scale: 0,
    };

    fn from_int(n: i128) -> Decimal {
        Decimal {
            digits: n,
            scale: 0,
        }
    }

    /// A double as its shortest decimal text says it.
    fn from_f64(d: f64) -> Option<Decimal> {
        if !d.is_finite() {
            return None;
        }
        Decimal::parse(&format!("{d}"))
    }

    /// Text that is a decimal number: an optional sign, digits, and an
    /// optional point with more digits.
    fn parse(text: &str) -> Option<Decimal> {
        let (negative, rest) = match text.as_bytes().first() {
            Some(b'-') => (true, &text[1..]),
            Some(b'+') => (false, &text[1..]),
            _ => (false, text),
        };
        let (whole, fraction) = rest.split_once('.').unwrap_or((rest, ""));
        let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        if whole.is_empty() && fraction.is_empty() || !digits(whole) || !digits(fraction) {
            return None;
        }
        let mut n: i128 = 0;
        for b in whole.bytes().chain(fraction.bytes()) {
            n = n.checked_mul(10)?.checked_add(i128::from(b - b'0'))?;
        }
        Some(Decimal {
            digits: if negative { -n } else { n },
            scale: fraction.len() as u32,
        })
    }

    /// The number with `decimals` decimals, rounded half away from zero.
    fn rounded(self, decimals: u32) -> Option<i128> {
        if self.scale <= decimals {
            return self
                .digits
                .checked_mul(10i128.checked_pow(decimals - self.scale)?);
        }
        let divisor = 10i128.checked_pow(self.scale - decimals)?;
        let half = divisor / 2;
        let n = self.digits;
        Some(if n >= 0 {
            (n + half) / divisor
        } else {
            (n - half) / divisor
        })
    }
}

/// A Numeric field's text of `scaled` with `decimals` decimals: a value that
/// rounds to zero has no sign.
pub(crate) fn numeric_text(scaled: i128, decimals: u32) -> String {
    let digits = scaled.unsigned_abs().to_string();
    let decimals = decimals as usize;
    let digits = format!("{digits:0>width$}", width = decimals + 1);
    let (whole, fraction) = digits.split_at(digits.len() - decimals);
    let sign = if scaled < 0 { "-" } else { "" };
    match decimals {
        0 => format!("{sign}{whole}"),
        _ => format!("{sign}{whole}.{fraction}"),
    }
}

/// The integer `text` starts with, as C's `atol` reads it: leading white
/// space, a sign, digits; 0 if there are none.
fn atol(text: &str) -> i64 {
    let text = text.trim_start();
    let (negative, rest) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let n = rest
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |n, d| {
            n.wrapping_mul(10).wrapping_add(i64::from(d - b'0'))
        });
    if negative { n.wrapping_neg() } else { n }
}

/// The number `text` starts with, as C's `strtod` reads it; 0 if there is
/// none.
fn strtod(text: &str) -> f64 {
    let text = text.trim_start();
    let bytes = text.as_bytes();
    let mut end = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        end = 1;
    }
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let whole = digits(end);
    end += whole;
    let mut fraction = 0;
    if bytes.get(end) == Some(&b'.') {
        fraction = digits(end + 1);
        end += 1 + fraction;
    }
    if whole + fraction == 0 {
        return 0.0;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(bytes.get(end + 1), Some(b'+' | b'-')));
        let exponent = digits(end + 1 + sign);
        if exponent > 0 {
            end += 1 + sign + exponent;
        }
    }
    text[..end].trim_end_matches('.').parse().unwrap_or(0.0)
}

/// The date `text` holds in the date format, as a Julian day number; `None`
/// for no date. A date that does not exist is [`Error::InvalidMonth`] or
/// [`Error::InvalidDay`] (or, in the year 0, [`Error::InvalidDate`]), as the
/// original engine tells them apart.
pub fn date(text: &str, dates: Dates) -> Result<Option<i32>, Error> {
    match tflip_expr::text::parse_date_with(text, dates.format, dates.epoch) {
        Ok(date) => Ok(date.map(|(day, _)| day)),
        Err(_) => Err(invalid_date_part(text, dates)),
    }
}

/// Why `text` is no date, as the date is read: its month, if that is not 1
/// to 12; its year, if that is 0 ([`Error::InvalidDate`]); else its day
/// (with the days its month has).
fn invalid_date_part(text: &str, dates: Dates) -> Error {
    let fields = tflip_expr::text::date_fields(text, dates.format, dates.epoch);
    match fields.month {
        1..=12 if fields.year == 0 => Error::InvalidDate,
        month @ 1..=12 => {
            let year = fields.year as i32;
            let days = (28..=31)
                .rev()
                .find(|&day| ymd_to_julian(Ymd { year, month, day }).is_some());
            Error::InvalidDay(days.unwrap_or(31))
        }
        _ => Error::InvalidMonth,
    }
}

/// The date and time `text` holds: a date in the date format, then a time of
/// day; `None` for no date. A date that does not exist is
/// [`Error::JulianConversion`]: one without a month or a day (as a Date
/// value is read) is not converted, any other converts to a negative day,
/// as the original engine tells them apart.
fn timestamp(text: &str, dates: Dates) -> Result<Option<(i32, u32)>, Error> {
    let Some((day, ms)) = tflip_expr::text::parse_timestamp_with(text, dates.format, dates.epoch)
        .map_err(|_| {
        let (end, _) = tflip_expr::text::timestamp_split(text);
        let fields = tflip_expr::text::date_fields(&text[..end], dates.format, dates.epoch);
        Error::JulianConversion {
            negative: fields.month != 0 && fields.day != 0,
        }
    })?
    else {
        return Ok(None);
    };
    Ok(Some((day, time_of_day(ms)?)))
}

/// The time of day `text` holds, in milliseconds, as the original engine
/// reads the time of a timestamp (see [`tflip_expr::text::timestamp_time`]).
/// A time of a day or more is [`Error::InvalidValue`].
fn time(text: &str) -> Result<u32, Error> {
    time_of_day(tflip_expr::text::timestamp_time(text))
}

/// `ms`, if it is less than a day; else [`Error::InvalidValue`].
fn time_of_day(ms: u32) -> Result<u32, Error> {
    if ms >= DAY_MS {
        Err(Error::InvalidValue)
    } else {
        Ok(ms)
    }
}

/// Milliseconds since midnight from `AdsSetMilliseconds`; less than a day.
fn milliseconds(ms: i32) -> Result<u32, Error> {
    u32::try_from(ms)
        .ok()
        .filter(|&ms| ms < DAY_MS)
        .ok_or(Error::InvalidValue)
}

/// Sets the time of a TimeStamp field, keeping its date.
fn time_of_day_into(ms: u32, slot: &mut [u8]) -> Result<(), Error> {
    slot[4..8].copy_from_slice(&ms.to_le_bytes());
    Ok(())
}

/// A GUID's 16 bytes from its text, `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`
/// with or without the braces, in the order Windows keeps a `GUID` (the first
/// three groups little-endian); other text is [`Error::InvalidData`].
pub(crate) fn guid(text: &str) -> Result<[u8; 16], Error> {
    let text = text.trim();
    let text = text
        .strip_prefix('{')
        .and_then(|t| t.strip_suffix('}'))
        .unwrap_or(text);
    let groups: Vec<&str> = text.split('-').collect();
    let lengths = [8, 4, 4, 4, 12];
    if groups.len() != 5 || groups.iter().zip(lengths).any(|(g, n)| g.len() != n) {
        return Err(Error::InvalidData);
    }
    let hex: String = groups.concat();
    let mut bytes = [0u8; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| Error::InvalidData)?;
    }
    bytes[..4].reverse();
    bytes[4..6].reverse();
    bytes[6..8].reverse();
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn julian(year: i32, month: u32, day: u32) -> i32 {
        ymd_to_julian(Ymd { year, month, day }).unwrap()
    }

    #[test]
    fn numeric_texts() {
        assert_eq!(numeric_text(1234567, 2), "12345.67");
        assert_eq!(numeric_text(-9999999, 2), "-99999.99");
        assert_eq!(numeric_text(0, 2), "0.00");
        assert_eq!(numeric_text(5, 2), "0.05");
        assert_eq!(numeric_text(-5, 0), "-5");
    }

    #[test]
    fn decimals() {
        assert_eq!(Decimal::parse("-1.005").unwrap().rounded(2), Some(-101));
        assert_eq!(Decimal::from_f64(2.5).unwrap().rounded(0), Some(3));
        assert_eq!(Decimal::from_f64(-0.125).unwrap().rounded(2), Some(-13));
        assert_eq!(Decimal::parse("1e3"), None);
        assert_eq!(Decimal::parse("."), None);
    }

    #[test]
    fn c_numbers() {
        assert_eq!(atol(" 42"), 42);
        assert_eq!(atol("-1.005"), -1);
        assert_eq!(atol("x1"), 0);
        assert_eq!(strtod("1e3"), 1000.0);
        assert_eq!(strtod("11/08/2017"), 11.0);
        assert_eq!(strtod("abc"), 0.0);
        assert_eq!(strtod("-1.005"), -1.005);
    }

    #[test]
    fn times() {
        assert_eq!(time("13:45:30.250"), Ok(49_530_250));
        assert_eq!(time("11/08/2017"), Ok(42_097_000));
        assert_eq!(time("abc"), Ok(0));
        assert_eq!(time(" 42"), Err(Error::InvalidValue));
        assert_eq!(time("25:00"), Err(Error::InvalidValue));
        assert_eq!(time("123456789.125"), Ok(0x01f0_8160));
    }

    #[test]
    fn dates() {
        let dates = Dates {
            format: "MM/DD/CCYY",
            epoch: 1900,
        };
        assert_eq!(date("11/08/2017", dates), Ok(Some(julian(2017, 11, 8))));
        assert_eq!(date("1e3", dates), Ok(Some(julian(1900, 1, 3))));
        assert_eq!(date("abc", dates), Ok(None));
        assert_eq!(date(" 42", dates), Err(Error::InvalidMonth));
        assert_eq!(date("2/30/2017", dates), Err(Error::InvalidDay(28)));
        assert_eq!(date("123456789.125", dates), Err(Error::InvalidDay(31)));
        assert_eq!(date("x1", dates), Err(Error::InvalidMonth));
    }

    /// Dates are read from text as the original engine reads them into a
    /// Date field (`fixtures/fields/dates.tsv`, probed with
    /// `tools/ace/fields/dates.c`): the format, the text, then AdsSetDate's
    /// and AdsSetField's return code and the Julian day after it (1000 when
    /// the set failed).
    #[test]
    fn dates_match_the_original_engine() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/fields/dates.tsv"
        );
        let mut failures = Vec::new();
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let columns: Vec<&str> = line.split('\t').collect();
            let (format, text, rc, day) = (columns[0], columns[1], columns[4], columns[5]);
            let got = match date(
                text,
                Dates {
                    format,
                    epoch: 1900,
                },
            ) {
                Ok(day) => format!("0 {}", day.unwrap_or(0)),
                Err(Error::InvalidDay(_)) => "5079 1000".to_owned(),
                Err(Error::InvalidMonth) => "5080 1000".to_owned(),
                Err(Error::InvalidDate) => "5095 1000".to_owned(),
                Err(error) => format!("{error:?}"),
            };
            if got != format!("{rc} {day}") {
                failures.push(format!("{format} {text:?}: {got}, the original {rc} {day}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// Timestamps are read from text as the original engine reads them into
    /// a TimeStamp field (the last two columns of `fixtures/fields/dates.tsv`:
    /// AdsSetField's return code and the field's text after it). Of a failed
    /// set only the code is compared: the original leaves the field as it was
    /// (01/01/1990 01:01:01), or sometimes part-written (see
    /// `docs/ADS_BUGS.md`), where tflip leaves it.
    #[test]
    fn timestamps_match_the_original_engine() {
        const DEC_25_2000: i32 = 2_451_904;
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/fields/dates.tsv"
        );
        let mut failures = Vec::new();
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let columns: Vec<&str> = line.split('\t').collect();
            let (format, text, rc, shown) = (columns[0], columns[1], columns[6], columns[7]);
            let want = match rc {
                "0" => format!("0 {shown}"),
                rc => rc.to_owned(),
            };
            let got = match timestamp(
                text,
                Dates {
                    format,
                    epoch: 1900,
                },
            ) {
                Ok(value) => {
                    let (day, ms) = value.unwrap_or((0, 0));
                    let date = match day {
                        0 => tflip_expr::text::format_date(DEC_25_2000, format)
                            .chars()
                            .map(|c| if c.is_ascii_digit() { ' ' } else { c })
                            .collect(),
                        day => tflip_expr::text::format_date(day, format),
                    };
                    let s = ms / 1000;
                    let half = if s < 43_200 { "AM" } else { "PM" };
                    let hour = match s / 3600 % 12 {
                        0 => 12,
                        hour => hour,
                    };
                    format!("0 {date} {hour:02}:{:02}:{:02} {half}", s / 60 % 60, s % 60)
                }
                Err(Error::JulianConversion { .. }) => "5095".to_owned(),
                Err(Error::InvalidValue) => "5101".to_owned(),
                Err(error) => format!("{error:?}"),
            };
            if got != want {
                failures.push(format!("{format} {text:?}: {got}, the original {want}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// TimeStamp values are read from the texts of
    /// `fixtures/fields/tstexts.tsv` as the original engine reads them: its
    /// columns 6 and 7 are AdsSetField's return code and the field's Julian
    /// day and milliseconds after it (unchanged, 2447893 3661000, when the
    /// set failed).
    #[test]
    fn timestamp_texts_match_the_original_engine() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/fields/tstexts.tsv"
        );
        let mut failures = Vec::new();
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let columns: Vec<&str> = line.split('\t').collect();
            let (format, epoch, text) = (columns[0], columns[1].parse().unwrap(), columns[2]);
            let got = match timestamp(text, Dates { format, epoch }) {
                Ok(value) => {
                    let (day, ms) = value.unwrap_or((0, 0));
                    format!("0 {day} {ms}")
                }
                Err(Error::JulianConversion { .. }) => "5095 2447893 3661000".to_owned(),
                Err(Error::InvalidValue) => "5101".to_owned(),
                Err(error) => format!("{error:?}"),
            };
            let want = match columns[5] {
                // A too-large time can leave the date written (see
                // `docs/ADS_BUGS.md`).
                "5101" => "5101".to_owned(),
                rc => format!("{rc} {}", columns[6]),
            };
            if got != want {
                failures.push(format!(
                    "{format} {epoch} {text:?}: {got}, the original {want}"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn base64() {
        let mut slot = [0u8; 4];
        base64_into("abc", &mut slot).unwrap();
        assert_eq!(slot, [0x69, 0xb7, 0, 0]);
        base64_into("T", &mut slot).unwrap();
        assert_eq!(slot, [0x4c, 0xb7, 0, 0]);
        assert_eq!(base64_into("abcdef", &mut slot), Err(Error::DataTruncated));
        assert_eq!(slot, [0x69, 0xb7, 0x1d, 0x79]);
        assert_eq!(base64_into("  12", &mut slot), Err(Error::InvalidValue));
    }

    #[test]
    fn guids() {
        let bytes = guid("{01234567-89AB-CDEF-0123-456789ABCDEF}").unwrap();
        assert_eq!(
            bytes,
            [
                0x67, 0x45, 0x23, 0x01, 0xab, 0x89, 0xef, 0xcd, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
                0xcd, 0xef
            ]
        );
        assert!(guid("{0123}").is_err());
    }
}
