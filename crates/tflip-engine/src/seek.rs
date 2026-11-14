//! Seeking: the key a client's value seeks in an order, as the original
//! engine builds it for `AdsSeek`.
//!
//! The rules come from probing the original engine. A string is converted
//! the way the order's field is typed; for an order on an expression or on
//! several fields it is taken as the key's bytes. Some of the original's
//! conversions are odd, and are kept: a string or double seeks a LongInt
//! order with a Double's key, which LongInt keys never equal, and a double
//! seeks a character order with its own little-endian bytes.

use serde::{Deserialize, Serialize};
use tflip_expr::number::atof;
use tflip_expr::text::{InvalidDate, parse_time};
use tflip_storage::field::{Field, FieldType, Layout};
use tflip_storage::value::scaled_decimal;
use tflip_storage::{cp1252, key};

use crate::Error;

/// The date `text` starts with, as the original engine reads seek keys
/// (see [`tflip_expr::text::parse_date`]).
fn parse_date(text: &str, format: &str, epoch: u16) -> Result<Option<(i32, usize)>, Error> {
    tflip_expr::text::parse_date_with(text, format, epoch).map_err(|InvalidDate| Error::InvalidDate)
}

/// The date and time of a timestamp seek key, as a TimeStamp value's text
/// is read but with the time not checked (see
/// [`tflip_expr::text::parse_timestamp_with`]); the day 0 for no date.
fn parse_timestamp(text: &str, format: &str, epoch: u16) -> Result<(i32, u32), Error> {
    let timestamp = tflip_expr::text::parse_timestamp_with(text, format, epoch)
        .map_err(|InvalidDate| Error::InvalidDate)?;
    Ok(timestamp.unwrap_or((0, 0)))
}

/// What a client seeks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SeekValue<'a> {
    /// The key's bytes, or the start of them (`ADS_RAWKEY`).
    Raw(&'a [u8]),
    /// Text, converted as the order's field is typed (`ADS_STRINGKEY`,
    /// `ADS_WSTRINGKEY`).
    Text(&'a str),
    /// A number: a Julian day for a date, milliseconds for a time
    /// (`ADS_DOUBLEKEY`).
    Double(f64),
}

/// How a seek treats a key the order does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SeekType {
    /// Lands on the first key after it (`ADS_SOFTSEEK`).
    Soft,
    /// Goes to EOF (`ADS_HARDSEEK`).
    Hard,
    /// Lands on the first key after every key that starts with it
    /// (`ADS_SEEKGT`).
    After,
}

/// What an order's keys are built from, as far as seeking needs to know.
#[derive(Debug, Clone, Copy)]
pub(crate) enum KeySource<'a> {
    /// One field.
    Field(&'a Field),
    /// Several fields (`a;b`), the first of them given.
    Fields(&'a Field),
    /// An expression.
    Expression,
}

/// What seeking a CDX order on an expression needs to know of it.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CdxExpression {
    /// Whether its value is a timestamp.
    pub stamp: bool,
    /// Whether its keys start with a byte saying whether they are NULL (it
    /// reads a VFP field that takes NULL).
    pub nullable: bool,
    /// Whether its value is a number: a double is sought as one, as a
    /// Numeric field's (`fixtures/expr/numkeys.json`).
    pub numeric: bool,
}

/// The key that `value` seeks in an order whose keys are `key_length`
/// bytes and come from `source`, with dates in `date_format` (as
/// `AdsSetDateFormat` sets it). An empty key seeks the first key. A key
/// longer than the order's is cut to it.
pub(crate) fn seek_key(
    value: SeekValue,
    source: KeySource,
    key_length: usize,
    date_format: &str,
    epoch: u16,
) -> Result<Vec<u8>, Error> {
    let mut key = match (value, source) {
        (SeekValue::Raw(bytes), _) => bytes.to_vec(),
        (SeekValue::Text(text), KeySource::Field(field)) => {
            text_key(text, field, date_format, epoch)?
        }
        // The first of several fields is sought by its own key (an NChar's
        // by the sort key of the text), whole (`fixtures/write/nchar_expr`).
        (SeekValue::Text(text), KeySource::Fields(field))
            if matches!(field.field_type, FieldType::NChar | FieldType::NVarChar) =>
        {
            text_key(text, field, date_format, epoch)?
        }
        (SeekValue::Text(text), _) => cp1252::encode(text),
        (SeekValue::Double(value), KeySource::Field(field) | KeySource::Fields(field)) => {
            double_key(value, field)?
        }
        // A character expression order takes a double's raw bytes (a numeric or
        // date expression order is keyed as a double earlier, in `sought_key`).
        (SeekValue::Double(value), KeySource::Expression) => value.to_le_bytes().to_vec(),
    };
    key.truncate(key_length);
    Ok(key)
}

/// The key that `value` seeks in an NTX order whose keys are `key_length`
/// characters (numbers with `decimals` decimals) and come from `source`.
/// NTX keys are text: text seeks a character order or an expression as its
/// bytes; a number given as text (read as `atof` reads it) or a double
/// seeks a numeric order as its key (see [`tflip_storage::ntx::number_key`]);
/// a date given as text (in `date_format`, `00000000` if it is none) or a
/// Julian day seeks a date order as `YYYYMMDD`; a logical order takes text
/// as an ADT order does. Other seeks are [`Error::NotImplemented`].
pub(crate) fn ntx_seek_key(
    value: SeekValue,
    source: KeySource,
    key_length: usize,
    decimals: u16,
    date_format: &str,
    epoch: u16,
) -> Result<Vec<u8>, Error> {
    use FieldType::*;
    use tflip_storage::ntx;
    let field = match source {
        KeySource::Field(field) | KeySource::Fields(field) => Some(field.field_type),
        KeySource::Expression => None,
    };
    let number = |value: f64| ntx::number_key(value, key_length, usize::from(decimals));
    let mut key = match (value, field) {
        (SeekValue::Raw(bytes), _) => bytes.to_vec(),
        (SeekValue::Text(text), None | Some(Character | VarChar | LegacyVarChar)) => {
            cp1252::encode(text)
        }
        (
            SeekValue::Text(text),
            Some(Numeric | ShortInt | Integer | AutoInc | Double | CurDouble | LongInt | Money),
        ) => number(atof(text)),
        (
            SeekValue::Double(value),
            Some(Numeric | ShortInt | Integer | AutoInc | Double | CurDouble | LongInt | Money),
        ) => number(value),
        (SeekValue::Text(text), Some(Date | CompactDate)) => {
            match parse_date(text, date_format, epoch)? {
                Some((julian, _)) => ntx::date_key(julian),
                None => b"00000000".to_vec(),
            }
        }
        (SeekValue::Double(value), Some(Date | CompactDate)) => ntx::date_key(value as i32),
        (SeekValue::Text(text), Some(Logical)) => match text_logical(text)? {
            true => b"T".to_vec(),
            false => b"F".to_vec(),
        },
        (SeekValue::Double(_), Some(Logical)) => return Err(Error::InvalidExpression),
        // A double seeks a character order as its bytes, finding nothing
        // (`fixtures/expr/alias.json`).
        (SeekValue::Double(value), Some(Character | VarChar | LegacyVarChar)) => {
            value.to_le_bytes().to_vec()
        }
        _ => return Err(Error::NotImplemented),
    };
    key.truncate(key_length);
    Ok(key)
}

/// The key that `value` seeks in a CDX order whose keys are `key_length`
/// bytes and come from `source`: as in an ADT order (see [`seek_key`]),
/// but unweighed (the order's cursor weighs keys), a Numeric field's key
/// a double's, a VFP Integer's 4 bytes (the number cut to a whole one),
/// text that is no date seeking a date order as the empty date, and a key
/// of a VFP field that can be NULL led by 0x80. An order on an expression
/// whose value is a timestamp is sought as one on a VFP TimeStamp field
/// (`fixtures/write/cdx_timestamps`).
pub(crate) fn cdx_seek_key(
    value: SeekValue,
    source: KeySource,
    key_length: usize,
    expression: CdxExpression,
    date_format: &str,
    epoch: u16,
) -> Result<Vec<u8>, Error> {
    let field = match source {
        KeySource::Field(field) => Some(field),
        _ => None,
    };
    let nullable = match field {
        Some(f) => matches!(&f.layout, Layout::Dbf(l) if l.null_bit.is_some()),
        None => expression.nullable,
    };
    let field_type = match field {
        None if expression.stamp => Some(FieldType::TimeStamp),
        None if expression.numeric => Some(FieldType::Numeric),
        field => field.map(|f| f.field_type),
    };
    if matches!(value, SeekValue::Raw(_)) {
        return seek_key(value, source, key_length, date_format, epoch);
    }
    let length = key_length - usize::from(nullable);
    let int32 = |value: f64| key::sortable(i64::from(value as i64 as i32), 4);
    let mut key = match (value, field_type) {
        (SeekValue::Text(text), Some(FieldType::Numeric)) => key::double(atof(text)).to_vec(),
        (SeekValue::Double(value), Some(FieldType::Numeric)) => key::double(value).to_vec(),
        (SeekValue::Text(text), Some(FieldType::Integer | FieldType::AutoInc)) if length == 4 => {
            int32(atof(text))
        }
        (SeekValue::Double(value), Some(FieldType::Integer | FieldType::AutoInc))
            if length == 4 =>
        {
            int32(value)
        }
        (SeekValue::Text(text), Some(FieldType::Date)) => {
            let julian = parse_date(text, date_format, epoch).ok().flatten();
            key::double(f64::from(julian.map_or(0, |(julian, _)| julian))).to_vec()
        }
        // A VFP timestamp's key is a double of the day and its fraction, as
        // a record's is (see `key::vfp_timestamp`); the original leaves out
        // the milliseconds of the text (`fixtures/write/vfp_timestamps`).
        (SeekValue::Text(text), Some(FieldType::TimeStamp)) => {
            let day = match parse_timestamp(text, date_format, epoch) {
                Ok((julian, ms)) if julian != 0 => key::vfp_timestamp(julian, ms - ms % 1000),
                _ => 0.0,
            };
            key::double(day).to_vec()
        }
        (SeekValue::Double(value), Some(FieldType::TimeStamp)) => key::double(value).to_vec(),
        _ => seek_key(value, source, length, date_format, epoch)?,
    };
    if nullable {
        key.insert(0, 0x80);
    }
    key.truncate(key_length);
    Ok(key)
}

/// The key of `text` seeking an order on `field`.
fn text_key(text: &str, field: &Field, date_format: &str, epoch: u16) -> Result<Vec<u8>, Error> {
    use FieldType::*;
    Ok(match field.field_type {
        Character | VarChar | Raw | Guid => cp1252::encode(text),
        CiCharacter => cp1252::encode(text).to_ascii_lowercase(),
        // An ICU sort key, whole: `ab` does not find `abc`.
        NChar | NVarChar => {
            let text = tflip_storage::icu::nchar_text(text);
            tflip_storage::icu::sort_key(text).ok_or(Error::NotImplemented)?
        }
        Numeric => {
            let decimals = field.decimals();
            let scaled = match text.trim_matches(|c: char| c.is_ascii_whitespace()) {
                "" | "-" | "+" => 0,
                _ => scaled_decimal(text, u32::from(decimals)).ok_or(Error::InvalidDataType)?,
            };
            key::numeric(scaled, field.length(), decimals).ok_or(Error::ValueOverflow)?
        }
        Money => {
            let scaled = scaled_decimal(text, 4).ok_or(Error::InvalidDataType)?;
            key::int64(i64::try_from(scaled).map_err(|_| Error::ValueOverflow)?).to_vec()
        }
        RowVersion => match scaled_decimal(text, 4) {
            Some(scaled) if scaled % 10_000 == 0 => {
                let value = i64::try_from(scaled / 10_000).map_err(|_| Error::ValueOverflow)?;
                key::int64(value).to_vec()
            }
            Some(_) => return Err(Error::ValueOverflow),
            None => return Err(Error::InvalidData),
        },
        ShortInt | Integer | AutoInc | Double | CurDouble | LongInt => {
            key::double(atof(text)).to_vec()
        }
        Date => {
            let julian = parse_date(text, date_format, epoch)?.map_or(0, |(julian, _)| julian);
            key::double(f64::from(julian)).to_vec()
        }
        Time => key::time(parse_time(text)).to_vec(),
        TimeStamp | ModTime => {
            let (julian, ms) = parse_timestamp(text, date_format, epoch)?;
            key::timestamp(julian, ms).to_vec()
        }
        Logical => key::logical(text_logical(text)?).to_vec(),
        _ => return Err(Error::InvalidDataType),
    })
}

/// The Logical value text seeks: it starts with T, Y or 1 for true, F, N or 0
/// for false.
fn text_logical(text: &str) -> Result<bool, Error> {
    match text.bytes().next() {
        Some(b'T' | b't' | b'Y' | b'y' | b'1') => Ok(true),
        Some(b'F' | b'f' | b'N' | b'n' | b'0') => Ok(false),
        _ => Err(Error::InvalidExpression),
    }
}

/// The key of the number `value` seeking an order on `field`.
fn double_key(value: f64, field: &Field) -> Result<Vec<u8>, Error> {
    use FieldType::*;
    Ok(match field.field_type {
        Character | CiCharacter | VarChar => value.to_le_bytes().to_vec(),
        Logical => key::logical(value != 0.0).to_vec(),
        Numeric => {
            let decimals = field.decimals();
            let scaled = scaled_double(value, decimals).ok_or(Error::ValueOverflow)?;
            key::numeric(scaled, field.length(), decimals).ok_or(Error::ValueOverflow)?
        }
        Money => {
            let scaled = scaled_double(value, 4).ok_or(Error::ValueOverflow)?;
            key::int64(i64::try_from(scaled).map_err(|_| Error::ValueOverflow)?).to_vec()
        }
        RowVersion => key::int64(value as i64).to_vec(),
        Time => key::time(value as i64 as u32).to_vec(),
        TimeStamp | ModTime => return Err(Error::InvalidDataType),
        NChar | NVarChar => return Err(Error::NotImplemented),
        _ => key::double(value).to_vec(),
    })
}

/// The number text that starts as one gives (`1003`, ` -2.5`), as `atof`
/// reads it; `None` for other text, which seeks a numeric expression order
/// as text, finding nothing (`fixtures/write/nchar_expr_more`,
/// `fixtures/expr/numkeys.json`).
pub(crate) fn number_text(text: &str) -> Option<f64> {
    let rest = text.trim_start().trim_start_matches(['+', '-']);
    let rest = rest.strip_prefix('.').unwrap_or(rest);
    rest.starts_with(|c: char| c.is_ascii_digit())
        .then(|| atof(text))
}

/// `value` × 10^`decimals`, rounded half away from zero from its exact
/// value (1.505 is 1.50499…, so 150 for two decimals; 1002.5 is 1003 for
/// none, `fixtures/expr/numkeys.json`); `None` if it is not finite.
pub(crate) fn scaled_double(value: f64, decimals: u16) -> Option<i128> {
    if !value.is_finite() {
        return None;
    }
    let decimals = usize::from(decimals);
    // The exact digits, far enough past the last kept one that rounding
    // there cannot reach it.
    let precision = decimals + 30;
    let text = format!("{:.precision$}", value.abs());
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let mut scaled = scaled_decimal(
        &format!("{whole}.{}", &fraction[..decimals]),
        decimals as u32,
    )?;
    if fraction
        .as_bytes()
        .get(decimals)
        .is_some_and(|&d| d >= b'5')
    {
        scaled = scaled.checked_add(1)?;
    }
    Some(if value < 0.0 { -scaled } else { scaled })
}

/// `key` made the largest key of its length before it: its last byte that
/// is not 0 decremented, and the bytes after it 0xFF. `None` if every byte
/// is 0.
pub(crate) fn before(mut key: Vec<u8>) -> Option<Vec<u8>> {
    let last = key.iter().rposition(|&b| b != 0)?;
    key[last] -= 1;
    key[last + 1..].fill(0xFF);
    Some(key)
}

/// `key` made the smallest key after every key that starts with it, for
/// [`SeekType::After`]: its last byte that is not 0xFF incremented, and the
/// bytes after it 0. `None` if every byte is 0xFF.
pub(crate) fn after(mut key: Vec<u8>) -> Option<Vec<u8>> {
    let last = key.iter().rposition(|&b| b != 0xFF)?;
    key[last] += 1;
    key[last + 1..].fill(0);
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_after() {
        assert_eq!(after(b"abc".to_vec()), Some(b"abd".to_vec()));
        assert_eq!(after(vec![0x61, 0xFF]), Some(vec![0x62, 0x00]));
        assert_eq!(after(vec![0xFF, 0xFF]), None);
        assert_eq!(after(vec![]), None);
        assert_eq!(before(b"abc".to_vec()), Some(b"abb".to_vec()));
        assert_eq!(before(vec![0x61, 0x00]), Some(vec![0x60, 0xFF]));
        assert_eq!(before(vec![0, 0]), None);
    }

    /// A TimeStamp seek key's text is read as the original engine reads it
    /// (`fixtures/fields/tstexts.tsv`, probed with
    /// `tools/ace/fields/tstexts.c`: the format, the epoch, the text, then in
    /// the last two columns AdsSeek's return code and the key's Julian day
    /// and milliseconds). A key the probe could only place before a day's
    /// midnight (`~`) is the day before with a time of a day or more, or,
    /// before the first day, the day 0.
    #[test]
    fn timestamp_keys_match_the_original_engine() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/fields/tstexts.tsv"
        );
        let mut failures = Vec::new();
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let columns: Vec<&str> = line.split('\t').collect();
            let (format, epoch, text) = (columns[0], columns[1].parse().unwrap(), columns[2]);
            let got = match parse_timestamp(text, format, epoch) {
                Ok((0, _)) => "0 ~1 0".to_owned(),
                Ok((day, ms)) if ms >= 86_400_000 => format!("0 ~{} 0", day + 1),
                Ok((day, ms)) => format!("0 {day} {ms}"),
                Err(Error::InvalidDate) => "5095 ".to_owned(),
                Err(error) => format!("{error:?}"),
            };
            let want = format!("{} {}", columns[7], columns[8]);
            if got != want {
                failures.push(format!(
                    "{format} {epoch} {text:?}: {got}, the original {want}"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

/// Seeks in `adi_keys/keys.adt`, which has an order per key type, checked
/// against what the original engine does.
#[cfg(test)]
mod table_tests {
    use std::path::Path;

    use super::*;
    use crate::table::{self, Position, Table, TableSettings, TableType};
    use crate::{Session, index};

    /// Where `table` is after seeking `value` in `order` from record 3, as
    /// the probe of the original engine writes it: the record, or `E` at
    /// EOF, with `*` if the key was found.
    fn seek(
        session: &Session,
        table: &Table,
        handle: crate::handle::Handle,
        order: &str,
        value: SeekValue,
    ) -> String {
        let order = index::get(session, index::by_name(session, handle, order).unwrap()).unwrap();
        [SeekType::Soft, SeekType::Hard, SeekType::After]
            .map(|how| {
                table.goto_record(3).unwrap();
                match table.seek_in(&order, value, how) {
                    Err(error) => format!("{error:?}"),
                    Ok(found) => {
                        assert_eq!(found, table.found());
                        let at = match table.position() {
                            Position {
                                record: Some(record),
                                ..
                            } => record.to_string(),
                            Position {
                                bof: true,
                                eof: true,
                                ..
                            } => "BE".to_owned(),
                            Position { eof: true, .. } => "E".to_owned(),
                            Position { .. } => "B".to_owned(),
                        };
                        if found { at + "*" } else { at }
                    }
                }
            })
            .join(" ")
    }

    #[test]
    fn seeks_match_the_engine() {
        let session = Session::new();
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt/adi_keys/keys.adt");
        let settings = TableSettings {
            table_type: TableType::Adt,
            char_type: 1,
            collation: String::new(),
            options: 0,
            ..TableSettings::default()
        };
        let handle = table::open(&session, 0, path.to_str().unwrap(), settings).unwrap();
        let table = table::get(&session, handle).unwrap();
        let text = SeekValue::Text;
        let raw = SeekValue::Raw;
        let double = SeekValue::Double;
        // Soft, hard and after, as the original engine does them.
        for (order, value, expected) in [
            ("I_CHAR", text("abc"), "1* 1* 4*"),
            ("I_CHAR", text("a"), "5* 5* 4*"),
            ("I_CHAR", text("abd"), "4 E 4*"),
            ("I_CHAR", text("zz"), "4 E 4*"),
            ("I_CHAR", text(""), "6* 6* 2*"),
            ("I_CHAR", text(" "), "3* 3* 2*"),
            ("I_CHAR", text("abc    x"), "1* 1* 4*"),
            ("I_CHAR", text("a b"), "1 E 1*"),
            ("I_CHAR", text("Ä"), "4* 4* E"),
            ("I_CHAR", text("ÿ"), "E E E"),
            ("I_CHAR", raw(b""), "6* 6* 2*"),
            ("I_CHAR", raw(&[0x61, 0xFF]), "4 E 4*"),
            ("I_CHAR", raw(&[0xFF; 6]), "E E E"),
            ("I_CHAR", double(1.0), "6* 6* 3*"),
            ("I_CICHAR", text("ABC"), "1* 1* 2*"),
            ("I_CICHAR", text("ä"), "4* 4* E"),
            ("I_DESC", text("abc"), "1* 1* 1*"),
            ("I_DESC", text("b"), "1 E 1*"),
            ("I_DESC", text(""), "4* 4* 3*"),
            ("I_DESC", text("\u{1}"), "6 E 6*"),
            ("I_DESC", text("ÿ"), "4 E 4*"),
            ("X_UPPER", text("abc"), "4 E 4*"),
            ("X_STR", text("1"), "E E E"),
            ("X_DTOS", text("2017"), "1* 1* 4*"),
            ("X_DTOS", double(1.0), "3 E 3*"),
            ("I_INTEGER", text("1"), "1* 1* 4*"),
            ("I_INTEGER", text("1.5"), "4 E 4*"),
            ("I_INTEGER", text("abc"), "3* 3* 1*"),
            ("I_INTEGER", text("1e400"), "E E E"),
            ("I_INTEGER", raw(b""), "6* 6* 5*"),
            ("I_INTEGER", double(-3e9), "5 E 5*"),
            ("I_LONGINT", text("1"), "4 E 4*"),
            ("I_LONGINT", text("-9223372036854775807"), "2 E 2*"),
            ("I_ROWVERSION", text("3"), "3* 3* 4*"),
            (
                "I_ROWVERSION",
                text("3.7"),
                "ValueOverflow ValueOverflow ValueOverflow",
            ),
            (
                "I_ROWVERSION",
                text("x"),
                "InvalidData InvalidData InvalidData",
            ),
            ("I_DOUBLE", text("-1.5"), "2* 2* 3*"),
            ("I_MONEY", text("1.50005"), "4 E 4*"),
            ("I_MONEY", text("-922337203685477.5807"), "5* 5* 2*"),
            (
                "I_MONEY",
                text(""),
                "InvalidDataType InvalidDataType InvalidDataType",
            ),
            ("I_NUMERIC", text("1.505"), "4 E 4*"),
            ("I_NUMERIC", text("-1.495"), "2* 2* 3*"),
            ("I_NUMERIC", text("-"), "3* 3* 1*"),
            (
                "I_NUMERIC",
                text("10000"),
                "ValueOverflow ValueOverflow ValueOverflow",
            ),
            (
                "I_NUMERIC",
                text("1.5x"),
                "InvalidDataType InvalidDataType InvalidDataType",
            ),
            ("I_NUMERIC", double(1.505), "1* 1* 4*"),
            ("I_DATE", text("11/08/2017"), "1* 1* 4*"),
            ("I_DATE", text("11/08/17"), "1 E 1*"),
            ("I_DATE", text("garbage"), "3 E 3*"),
            (
                "I_DATE",
                text("02/30/2017"),
                "InvalidDate InvalidDate InvalidDate",
            ),
            ("I_DATE", double(2458066.5), "4 E 4*"),
            ("I_TIME", text("12:34:56 PM"), "1* 1* 3*"),
            ("I_TIME", text("00:00:00.1"), "4* 4* 5*"),
            ("I_TIME", text("x"), "2* 2* 4*"),
            ("I_TIME", raw(b""), "6* 6* 2*"),
            ("I_TIME", double(-5.0), "E E E"),
            ("I_TIMESTAMP", text("11/08/2017 12:34:56.789"), "1* 1* 3*"),
            ("I_TIMESTAMP", text("11/08/2017 12:34:56.0789"), "1* 1* 3*"),
            ("I_TIMESTAMP", text(""), "4 E 4*"),
            (
                "I_TIMESTAMP",
                text("x 12:00"),
                "InvalidDate InvalidDate InvalidDate",
            ),
            (
                "I_TIMESTAMP",
                double(1.0),
                "InvalidDataType InvalidDataType InvalidDataType",
            ),
            ("I_LOGICAL", text("yes"), "1* 1* E"),
            ("I_LOGICAL", text("0"), "2* 2* 1*"),
            (
                "I_LOGICAL",
                text(" T"),
                "InvalidExpression InvalidExpression InvalidExpression",
            ),
            ("I_LOGICAL", double(-2.0), "1* 1* E"),
            ("I_GUID", text("00112233"), "1 E 1*"),
            ("I_GUID", double(-1.0), "4 E 4*"),
            ("I_RAW", double(1.0), "2 E 2*"),
            ("I_MULTI", text("1"), "5 E 5*"),
            ("I_MULTI", double(-1.0), "2* 2* 3*"),
            ("I_NCHAR", text("abc"), "1* 1* 2*"),
            (
                "I_NVARCHAR",
                double(1.0),
                "NotImplemented NotImplemented NotImplemented",
            ),
        ] {
            assert_eq!(
                seek(&session, &table, handle, order, value),
                expected,
                "{order} {value:?}"
            );
        }
        // A seek that fails leaves the found flag; any move clears it.
        let order = index::get(
            &session,
            index::by_name(&session, handle, "I_CHAR").unwrap(),
        )
        .unwrap();
        assert!(table.seek_in(&order, text("abc"), SeekType::Soft).unwrap());
        let date = index::get(
            &session,
            index::by_name(&session, handle, "I_DATE").unwrap(),
        )
        .unwrap();
        assert!(
            table
                .seek_in(&date, text("13/01/2000"), SeekType::Soft)
                .is_err()
        );
        assert!(table.found());
        table.skip_in(&order, 0).unwrap();
        assert!(!table.found());
        table::close(&session, handle).unwrap();
    }

    /// Where `table` is: the record, `B`, `E` or `BE`.
    fn at(table: &Table) -> String {
        match table.position() {
            Position {
                record: Some(record),
                ..
            } => record.to_string(),
            Position {
                bof: true,
                eof: true,
                ..
            } => "BE".to_owned(),
            Position { eof: true, .. } => "E".to_owned(),
            Position { .. } => "B".to_owned(),
        }
    }

    /// The records of `order` forward from the top and back from the
    /// bottom, ending with where the walk ran off.
    fn walk(table: &Table, order: &index::Index) -> String {
        let mut forward = Vec::new();
        table.goto_top_in(order).unwrap();
        loop {
            forward.push(at(table));
            if table.position().eof {
                break;
            }
            table.skip_in(order, 1).unwrap();
        }
        let mut back = Vec::new();
        table.goto_bottom_in(order).unwrap();
        loop {
            back.push(at(table));
            if table.position().bof {
                break;
            }
            table.skip_in(order, -1).unwrap();
        }
        format!("{} / {}", forward.join(" "), back.join(" "))
    }

    /// Scopes in `adi_keys/keys.adt`, checked against what the original
    /// engine does.
    #[test]
    fn scopes_match_the_engine() {
        let session = Session::new();
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt/adi_keys/keys.adt");
        let settings = TableSettings {
            table_type: TableType::Adt,
            char_type: 1,
            collation: String::new(),
            options: 0,
            ..TableSettings::default()
        };
        let handle = table::open(&session, 0, path.to_str().unwrap(), settings).unwrap();
        let table = table::get(&session, handle).unwrap();
        let order =
            |name| index::get(&session, index::by_name(&session, handle, name).unwrap()).unwrap();
        let (chars, descending, integers) = (order("I_CHAR"), order("I_DESC"), order("I_INTEGER"));
        let text = SeekValue::Text;
        // I_CHAR: 6 null, 3 spaces, 2 Zz, 5 a, 1 abc, 4 Ä.
        assert_eq!(walk(&table, &chars), "6 3 2 5 1 4 E / 4 1 5 2 3 6 B");
        table.goto_record(6).unwrap();
        table.set_scope(&chars, false, text("Z")).unwrap();
        assert_eq!(at(&table), "6", "setting a scope does not move");
        assert_eq!(chars.scope_key(false), Ok(b"Z".to_vec()));
        assert_eq!(chars.scope_key(true), Err(Error::NoScope));
        assert_eq!(walk(&table, &chars), "2 5 1 4 E / 4 1 5 2 B");
        table.set_scope(&chars, true, text("a")).unwrap();
        assert_eq!(walk(&table, &chars), "2 5 1 E / 1 5 2 B");
        assert_eq!(chars.key_count(true), Ok(3));
        assert_eq!(chars.key_count(false), Ok(6));
        table.goto_top_in(&chars).unwrap();
        table.skip_in(&chars, 1).unwrap();
        assert_eq!(table.key_number_in(&chars, true), Ok(2));
        assert!(table.found(), "counting from the top seeks it");
        assert_eq!(table.key_number_in(&chars, false), Ok(4));
        // Seeks: inside, before and after the scope.
        for (value, expected) in [
            ("abc", "1* 1* E"),
            (" ", "2 E 2*"),
            ("Ä", "E E E"),
            ("", "2* 2* 2*"),
            ("b", "E E E"),
        ] {
            assert_eq!(
                seek(&session, &table, handle, "I_CHAR", text(value)),
                expected,
                "{value}"
            );
        }
        // Moves from records outside the scope.
        for (from, records, expected) in [(4, 1, "E"), (4, -1, "1"), (6, 1, "2"), (6, -1, "B")] {
            table.goto_record(from).unwrap();
            table.skip_in(&chars, records).unwrap();
            assert_eq!(at(&table), expected, "from {from} skip {records}");
        }
        // Record order ignores scopes; other orders have their own.
        table.goto_top().unwrap();
        assert_eq!(at(&table), "1");
        assert_eq!(walk(&table, &descending), "4 1 5 2 3 6 E / 6 3 2 5 1 4 B");
        chars.clear_scope(false).unwrap();
        assert_eq!(walk(&table, &chars), "6 3 2 5 1 E / 1 5 2 3 6 B");
        assert_eq!(chars.clear_scope(false), Err(Error::NoScope));
        // An empty scope.
        table.set_scope(&chars, false, text("b")).unwrap();
        assert_eq!(walk(&table, &chars), "BE / BE");
        assert_eq!(chars.key_count(true), Ok(0));
        assert_eq!(
            seek(&session, &table, handle, "I_CHAR", text("abc")),
            "BE E E"
        );
        index::clear_all_scopes(&session, handle).unwrap();
        assert_eq!(chars.scope(), Default::default());

        // I_DESC: 4 Ä, 1 abc, 5 a, 2 Zz, 3 spaces, 6 null; the top is the
        // largest key.
        table.set_scope(&descending, false, text("a")).unwrap();
        assert_eq!(walk(&table, &descending), "1 5 2 3 6 E / 6 3 2 5 1 B");
        table.set_scope(&descending, true, text("Z")).unwrap();
        assert_eq!(walk(&table, &descending), "1 5 2 E / 2 5 1 B");
        descending.clear_scope(false).unwrap();
        assert_eq!(walk(&table, &descending), "4 1 5 2 E / 2 5 1 4 B");

        // I_INTEGER: 6 null, 5 -2147483647, 2 -1, 3 0, 1 1, 4 2147483647.
        table.set_scope(&integers, false, text("-1")).unwrap();
        table.set_scope(&integers, true, text("1")).unwrap();
        assert_eq!(integers.scope_key(false), Ok(key::double(-1.0).to_vec()));
        assert_eq!(walk(&table, &integers), "2 3 1 E / 1 3 2 B");
        table
            .set_scope(&integers, false, SeekValue::Double(-5.0))
            .unwrap();
        table
            .set_scope(&integers, true, SeekValue::Double(0.5))
            .unwrap();
        assert_eq!(walk(&table, &integers), "2 3 E / 3 2 B");
        table
            .set_scope(&integers, false, SeekValue::Raw(b""))
            .unwrap();
        assert_eq!(walk(&table, &integers), "6 5 2 3 E / 3 2 5 6 B");
        let date = order("I_DATE");
        assert_eq!(
            table.set_scope(&date, false, text("13/01/2000")),
            Err(Error::InvalidDate)
        );
        assert_eq!(date.scope(), Default::default());
        table::close(&session, handle).unwrap();
    }
}

/// Seeks, seek-lasts and scopes replayed from the goldens of
/// `tools/ace/seek` (`fixtures/seek/*.json`): where the original engine
/// landed for each command.
#[cfg(test)]
mod golden_tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::collations::Collations;
    use crate::table::{self, Position, TableSettings, TableType};
    use crate::{Session, index};

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
    }

    /// The engine's collation files, where this machine has them.
    fn collation_dir() -> Option<PathBuf> {
        std::env::var_os("TFLIP_COLLATION_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let home = PathBuf::from(std::env::var_os("HOME")?);
                Some(home.join("src/ace-sdk/drive_c/Program Files (x86)/Advantage 12.0/acesdk"))
            })
            .filter(|dir| dir.join("ansi.chr").exists())
    }

    fn unhex(text: &str) -> Vec<u8> {
        if text == "-" {
            return Vec::new();
        }
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    /// Code page 437's characters 0x80 to 0xFF, a US client's OEM code page.
    const CP437: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐\
                         └┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";

    /// The text of a command's hex argument: UTF-16LE for the `w` commands,
    /// Windows-1252 for the others; for an `oem` table, as the client layer
    /// hands it to the engine, in code page 437 (see `tflip-ace`'s
    /// `read_key`).
    fn text_of(command: &str, hex: &str, oem: bool) -> String {
        let bytes = unhex(hex);
        let text = if command.starts_with('w') {
            let units: Vec<u16> = bytes
                .as_chunks()
                .0
                .iter()
                .map(|&c| u16::from_le_bytes(c))
                .collect();
            String::from_utf16(&units).unwrap()
        } else {
            cp1252::decode(&bytes)
        };
        if !oem {
            return text;
        }
        let oem: Vec<u8> = text
            .chars()
            .map(|c| match CP437.chars().position(|o| o == c) {
                Some(at) => 0x80 + at as u8,
                None => c as u8,
            })
            .collect();
        cp1252::decode(&oem)
    }

    fn at(table: &table::Table) -> String {
        match table.position() {
            Position {
                record: Some(record),
                ..
            } => record.to_string(),
            Position {
                bof: true,
                eof: true,
                ..
            } => "BE".to_owned(),
            Position { eof: true, .. } => "E".to_owned(),
            Position { .. } => "B".to_owned(),
        }
    }

    /// Replays `fixtures/seek/<golden>` on the fixtures of `folder`
    /// (`charset_german`): the commands whose results differ from the
    /// original engine's, as `case: tflip / engine`.
    fn replay(session: &Session, golden: &str, folder: &str) -> Vec<String> {
        let json = std::fs::read_to_string(root().join("seek").join(golden)).unwrap();
        let cases: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        let mut differences = Vec::new();
        let mut handle = 0;
        let mut oem = false;
        for case in &cases {
            let line = case["case"].as_str().unwrap();
            let expected = case["out"].as_str().unwrap();
            let words: Vec<&str> = line.split(' ').collect();
            if words[0] == "open" {
                if handle != 0 {
                    table::close(session, handle).unwrap();
                }
                let (kind, name) = words[3].split_once('/').unwrap();
                let path = root().join(kind).join(folder).join(name);
                oem = words[2] == "2";
                let table_type = match words[1] {
                    "1" => TableType::Ntx,
                    "2" => TableType::Cdx,
                    _ => TableType::Adt,
                };
                let settings = TableSettings {
                    table_type,
                    char_type: words[2].parse().unwrap(),
                    collation: String::new(),
                    options: 0,
                    ..TableSettings::default()
                };
                handle = table::open(session, 0, path.to_str().unwrap(), settings).unwrap();
                for file in &words[4..] {
                    let file = file.split_once('/').unwrap().1;
                    index::open_file(session, handle, file).unwrap();
                }
                continue;
            }
            let table = table::get(session, handle).unwrap();
            let order =
                index::get(session, index::by_name(session, handle, words[1]).unwrap()).unwrap();
            let landed = |result: Result<bool, Error>| match result {
                Ok(found) => at(&table) + if found { "*" } else { "" },
                Err(error) => format!("{error:?}"),
            };
            let got = match words[0] {
                "seek" | "wseek" | "rseek" => {
                    let how = match words[2] {
                        "soft" => SeekType::Soft,
                        "hard" => SeekType::Hard,
                        _ => SeekType::After,
                    };
                    let text = text_of(words[0], words[3], oem);
                    let raw = unhex(words[3]);
                    let value = match words[0] {
                        "rseek" => SeekValue::Raw(&raw),
                        _ => SeekValue::Text(&text),
                    };
                    table.goto_record(3).unwrap();
                    landed(table.seek_in(&order, value, how))
                }
                "last" | "wlast" => {
                    let text = text_of(words[0], words[2], oem);
                    table.goto_record(3).unwrap();
                    landed(table.seek_last_in(&order, SeekValue::Text(&text)))
                }
                "scope" | "wscope" => {
                    let text = text_of(words[0], words[3], oem);
                    match table.set_scope(&order, words[2] == "bottom", SeekValue::Text(&text)) {
                        Ok(()) => "0".to_owned(),
                        Err(error) => format!("{error:?}"),
                    }
                }
                "clear" => match order.clear_scope(words[2] == "bottom") {
                    Ok(()) => "0".to_owned(),
                    Err(error) => format!("{error:?}"),
                },
                "walk" => {
                    let mut forward = Vec::new();
                    table.goto_top_in(&order).unwrap();
                    loop {
                        forward.push(at(&table));
                        if table.position().eof {
                            break;
                        }
                        table.skip_in(&order, 1).unwrap();
                    }
                    let mut back = Vec::new();
                    table.goto_bottom_in(&order).unwrap();
                    loop {
                        back.push(at(&table));
                        if table.position().bof {
                            break;
                        }
                        table.skip_in(&order, -1).unwrap();
                    }
                    format!("{} / {}", forward.join(" "), back.join(" "))
                }
                "key" => {
                    table.goto_record(words[2].parse().unwrap()).unwrap();
                    match table.current_key(&order) {
                        Ok(key) => key.iter().map(|b| format!("{b:02x}")).collect(),
                        Err(error) => format!("{error:?}"),
                    }
                }
                other => panic!("unknown command {other}"),
            };
            if got != expected {
                differences.push(format!("{line}: {got} / {expected}"));
            }
        }
        if handle != 0 {
            table::close(session, handle).unwrap();
        }
        differences
    }

    /// Under `ANSI_CHAR_SET=German`, seeks, seek-lasts and scopes in the
    /// `charset_german` `ca` tables (ADT, NTX and CDX) land where the
    /// original engine's do: the sought value and the scopes are weighed as
    /// the order's keys are.
    #[test]
    fn collation_seeks_match_the_engine() {
        let Some(dir) = collation_dir() else { return };
        let session = Session::new();
        crate::settings::set_collations(&session, Collations::new(&dir, "German", "GERMAN"));
        let differences = replay(&session, "collation.json", "charset_german");
        let count = differences.len();
        assert!(
            differences.is_empty(),
            "{count} differences:\n{}",
            differences.join("\n")
        );
    }

    /// Seeks, seek-lasts, scopes and keys in the NChar and NVarChar orders of
    /// `adi_keys` land where the original engine's do: text is sought as its
    /// ICU sort key, whole (`ab` does not find `abc`), without its trailing
    /// spaces unless it is all spaces.
    #[test]
    fn nchar_seeks_match_the_engine() {
        let differences = replay(&Session::new(), "nchar.json", "adi_keys");
        let count = differences.len();
        assert!(
            differences.is_empty(),
            "{count} differences:\n{}",
            differences.join("\n")
        );
    }

    /// Seeks in the OEM `co` tables of `charset_default` (NTX and CDX), with
    /// the client's text in its OEM code page, land where the original
    /// engine's do. Needs no collation files.
    #[test]
    fn oem_seeks_match_the_engine() {
        let differences = replay(&Session::new(), "oem.json", "charset_default");
        let count = differences.len();
        assert!(
            differences.is_empty(),
            "{count} differences:\n{}",
            differences.join("\n")
        );
    }
}
