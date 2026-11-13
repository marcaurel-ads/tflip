//! Field values as text, the way `AdsGetField` returns them.

pub use tflip_expr::text::format_date;
use tflip_storage::field::{Field, FieldType};
use tflip_storage::value::Value;

/// A field value as returned by text getters: text, or the raw bytes of a
/// binary value.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FieldText {
    Text(String),
    Bytes(Vec<u8>),
}

/// What the text of a field's values depends on, besides the values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub field_type: FieldType,
    /// The declared length (see [`Field::length`]).
    pub length: u16,
    pub decimals: u16,
}

impl From<&Field> for Layout {
    fn from(field: &Field) -> Layout {
        Layout {
            field_type: field.field_type,
            length: field.length(),
            decimals: field.decimals(),
        }
    }
}

/// The text of `value`, a value of a field laid out as `layout`, as the
/// original engine writes it. Dates follow `date_format` (see
/// [`format_date`]).
///
/// Doubles have the field's decimals (not `AdsSetDecimals`'): Double,4
/// holding 3.14159 is `3.1416`. Money has four decimals and no leading zero
/// below 1 (`.5000`, `-.5000`, `.0000`). Text, Numeric, Logical, integer and GUID values have one
/// obvious text. Times are 12-hour with AM or PM (`01:45:12 PM`), without
/// milliseconds. NULL has a text of its own for each type (see
/// [`null_text`]); a NULL Raw field is its length in zero bytes, and a NULL
/// Binary, Image or VarBinary field no bytes.
pub fn field_text(value: &Value, layout: &Layout, date_format: &str) -> FieldText {
    FieldText::Text(match value {
        Value::Null if layout.field_type == FieldType::Raw => {
            return FieldText::Bytes(vec![0; usize::from(layout.length)]);
        }
        Value::Null
            if matches!(
                layout.field_type,
                FieldType::Binary | FieldType::Image | FieldType::VarBinary
            ) =>
        {
            return FieldText::Bytes(Vec::new());
        }
        // A blank VFP date, timestamp or logical looks as a NULL one.
        Value::Null | Value::Blank => null_text(layout, date_format),
        Value::Text(text) | Value::Numeric(text) => text.clone(),
        Value::Binary(bytes) => return FieldText::Bytes(bytes.clone()),
        Value::Logical(true) => "T".to_owned(),
        Value::Logical(false) => "F".to_owned(),
        Value::Integer(n) => n.to_string(),
        Value::Double(d) => double_text(*d, usize::from(layout.decimals)),
        Value::Money(m) => {
            let sign = if *m < 0 { "-" } else { "" };
            let m = m.unsigned_abs();
            let units = match m / 10_000 {
                0 => String::new(),
                units => units.to_string(),
            };
            format!("{sign}{units}.{:04}", m % 10_000)
        }
        Value::Date(julian) => format_date(*julian, date_format),
        Value::Time(ms) => format_time(*ms),
        // A VFP timestamp with a time but no day has a blank date.
        Value::TimeStamp(0, ms) => format!("{} {}", blank_date(date_format), format_time(*ms)),
        Value::TimeStamp(julian, ms) => {
            format!("{} {}", format_date(*julian, date_format), format_time(*ms))
        }
        Value::Guid(bytes) => Value::guid_text(bytes),
    })
}

/// The text of a field's value as `AdsGetField` writes it with
/// `ADS_GET_FORMAT_ANSI`: as [`field_text`], but dates as `CCYY-MM-DD`
/// whatever the date format, and times 24-hour with milliseconds
/// (`13:45:30.250`).
pub fn ansi_field_text(value: &Value, layout: &Layout) -> FieldText {
    const ISO: &str = "CCYY-MM-DD";
    match *value {
        Value::Time(ms) => FieldText::Text(iso_time(ms)),
        Value::TimeStamp(julian, ms) => {
            FieldText::Text(format!("{} {}", format_date(julian, ISO), iso_time(ms)))
        }
        _ => field_text(value, layout, ISO),
    }
}

/// A time of `ms` milliseconds after midnight, 24-hour with milliseconds.
fn iso_time(ms: u32) -> String {
    let s = ms / 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        s / 3600,
        s / 60 % 60,
        s % 60,
        ms % 1000
    )
}

/// The widest text the original engine writes a double as.
const DOUBLE_WIDTH: usize = 32;

/// `value` with `decimals` decimals, as the original engine writes Double
/// and CurDouble values:
///
/// - The value is taken to 17 significant digits, then rounded to the
///   decimals, halves away from zero (2.5 with no decimals is `3`, 0.125
///   with two `0.13`). A value that rounds to zero has no sign.
/// - The text is at most 32 characters: decimals are dropped to fit
///   (Double,15 holding 1e16 has 14), with no point when none are left, and
///   longer integer parts are cut off at 32 characters.
///
/// Where the value's 18th digit is a 5 with nothing after it, the original
/// engine rounds the 17th its own way, up or down (see
/// docs/KNOWN_ISSUES.md); tflip rounds the exact value.
pub fn double_text(value: f64, decimals: usize) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    // 17 significant digits: value = digits × 10^(exponent - 16).
    let scientific = format!("{:.16e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits: Vec<u8> = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    let exponent: i64 = exponent.parse().unwrap_or(0);
    let negative = value < 0.0;
    let integer_digits = usize::try_from(exponent + 1).unwrap_or(0).max(1);
    let room = DOUBLE_WIDTH.saturating_sub(usize::from(negative) + integer_digits + 1);
    let decimals = decimals.min(room);
    // The value × 10^decimals, rounded half away from zero, as decimal digits.
    let shift = exponent - 16 + decimals as i64;
    let mut scaled: Vec<u8> = if shift >= 0 {
        let mut scaled = digits.clone();
        scaled.resize(digits.len() + shift as usize, 0);
        scaled
    } else {
        let drop = shift.unsigned_abs() as usize;
        let keep = digits.len().saturating_sub(drop);
        let mut scaled = digits[..keep].to_vec();
        let next = if drop <= digits.len() {
            digits.get(keep).copied().unwrap_or(0)
        } else {
            0
        };
        if next >= 5 {
            let mut i = scaled.len();
            loop {
                if i == 0 {
                    scaled.insert(0, 1);
                    break;
                }
                i -= 1;
                if scaled[i] == 9 {
                    scaled[i] = 0;
                } else {
                    scaled[i] += 1;
                    break;
                }
            }
        }
        scaled
    };
    let first = scaled.iter().position(|&d| d != 0).unwrap_or(scaled.len());
    scaled.drain(..first);
    let zero = scaled.is_empty();
    if scaled.len() <= decimals {
        scaled.splice(0..0, std::iter::repeat_n(0, decimals + 1 - scaled.len()));
    }
    let mut text = String::with_capacity(DOUBLE_WIDTH + 1);
    if negative && !zero {
        text.push('-');
    }
    let point = scaled.len() - decimals;
    for (i, digit) in scaled.iter().enumerate() {
        if i == point {
            text.push('.');
        }
        text.push(char::from(b'0' + digit));
    }
    text.truncate(DOUBLE_WIDTH);
    text
}

/// A day whose month and day both have two digits, for the shape of a date.
const DEC_25_2000: i32 = 2_451_904;

/// The text of NULL in a field laid out as `layout`, as the original engine
/// writes it: spaces across the full width for Character, CICharacter, NChar
/// and Numeric; empty for the variable-length and memo types; 0 for the
/// integer types, with the field's decimals for Double and CurDouble, and
/// `.0000` for Money; one space for Logical; a date's separators with spaces
/// for the digits, in any date format (`  /  /    `, `  .  .    `);
/// midnight for Time; both for TimeStamp and ModTime; and the all-zero GUID
/// for GUID. Binary, Image and VarBinary are empty. (Raw, whose NULL is its
/// length in zero bytes, is not text; see [`field_text`].)
pub fn null_text(layout: &Layout, date_format: &str) -> String {
    let blank_date = || blank_date(date_format);
    match layout.field_type {
        FieldType::Character | FieldType::CiCharacter | FieldType::NChar | FieldType::Numeric => {
            " ".repeat(usize::from(layout.length))
        }
        FieldType::Integer
        | FieldType::ShortInt
        | FieldType::LongInt
        | FieldType::AutoInc
        | FieldType::RowVersion => "0".to_owned(),
        FieldType::Double | FieldType::CurDouble => {
            format!("{:.*}", usize::from(layout.decimals), 0.0)
        }
        FieldType::Money => ".0000".to_owned(),
        FieldType::Logical => " ".to_owned(),
        FieldType::Date => blank_date(),
        FieldType::Time => format_time(0),
        FieldType::TimeStamp | FieldType::ModTime => format!("{} {}", blank_date(), format_time(0)),
        FieldType::Guid => Value::guid_text(&[0; 16]),
        _ => String::new(),
    }
}

/// A date in `date_format` with spaces for its digits (`  /  /    `).
fn blank_date(date_format: &str) -> String {
    let date = format_date(DEC_25_2000, date_format);
    date.chars()
        .map(|c| if c.is_ascii_digit() { ' ' } else { c })
        .collect()
}

/// A time of `ms` milliseconds after midnight, 12-hour with AM or PM, as the
/// original engine writes it with its default settings: `01:45:12 PM`.
/// Milliseconds are dropped.
fn format_time(ms: u32) -> String {
    let s = ms / 1000;
    let hour = s / 3600;
    let half = if hour < 12 { "AM" } else { "PM" };
    let hour = match hour % 12 {
        0 => 12,
        hour => hour,
    };
    format!("{hour:02}:{:02}:{:02} {half}", s / 60 % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use tflip_storage::value::julian_to_ymd;

    use super::*;

    const NOV_8_2017: i32 = 2458066;

    fn layout(field_type: FieldType, length: u16, decimals: u16) -> Layout {
        Layout {
            field_type,
            length,
            decimals,
        }
    }

    fn text(value: Value, layout: Layout) -> String {
        match field_text(&value, &layout, "MM/DD/CCYY") {
            FieldText::Text(t) => t,
            FieldText::Bytes(_) => panic!("bytes"),
        }
    }

    /// As the original engine writes them (Double,15, Double,2 and
    /// Double,0), in the range where its digits are exact. The values are
    /// the probe's, digits beyond a double's precision included.
    #[test]
    #[allow(clippy::excessive_precision)]
    fn doubles_match_the_engine() {
        let cases: &[(f64, &str, &str, &str)] = &[
            (3.5, "3.500000000000000", "3.50", "4"),
            (0.1, "0.100000000000000", "0.10", "0"),
            (-0.5, "-0.500000000000000", "-0.50", "-1"),
            (1e-5, "0.000010000000000", "0.00", "0"),
            (1e-20, "0.000000000000000", "0.00", "0"),
            (-1e-20, "0.000000000000000", "0.00", "0"),
            (
                123456789.123456789,
                "123456789.123456790000000",
                "123456789.12",
                "123456789",
            ),
            (
                1e15,
                "1000000000000000.000000000000000",
                "1000000000000000.00",
                "1000000000000000",
            ),
            (
                1e16,
                "10000000000000000.00000000000000",
                "10000000000000000.00",
                "10000000000000000",
            ),
            (
                1e20,
                "100000000000000000000.0000000000",
                "100000000000000000000.00",
                "100000000000000000000",
            ),
            (
                123456789012345678.0,
                "123456789012345680.0000000000000",
                "123456789012345680.00",
                "123456789012345680",
            ),
            (
                -123456789012345678.0,
                "-123456789012345680.000000000000",
                "-123456789012345680.00",
                "-123456789012345680",
            ),
            (
                -1e16,
                "-10000000000000000.0000000000000",
                "-10000000000000000.00",
                "-10000000000000000",
            ),
            (
                1e29,
                "99999999999999991000000000000.00",
                "99999999999999991000000000000.00",
                "99999999999999991000000000000",
            ),
            // Probed with tools/ace/fields/doubles.c.
            (
                1.5e30,
                "1499999999999999900000000000000",
                "1499999999999999900000000000000",
                "1499999999999999900000000000000",
            ),
            (
                9.9e29,
                "990000000000000020000000000000.0",
                "990000000000000020000000000000.0",
                "990000000000000020000000000000",
            ),
            (
                1e32,
                "10000000000000001000000000000000",
                "10000000000000001000000000000000",
                "10000000000000001000000000000000",
            ),
            (56.625, "56.625000000000000", "56.63", "57"),
            (4.9e-324, "0.000000000000000", "0.00", "0"),
            (0.123456789012345678, "0.123456789012346", "0.12", "0"),
            (2.5, "2.500000000000000", "2.50", "3"),
            (-2.5, "-2.500000000000000", "-2.50", "-3"),
            (0.125, "0.125000000000000", "0.13", "0"),
            (0.115, "0.115000000000000", "0.12", "0"),
            (-0.001, "-0.001000000000000", "0.00", "0"),
            (0.99999999999999999, "1.000000000000000", "1.00", "1"),
            (9.5, "9.500000000000000", "9.50", "10"),
            (1234.5678, "1234.567800000000000", "1234.57", "1235"),
            (0.000123456789012345678, "0.000123456789012", "0.00", "0"),
            (5e-16, "0.000000000000001", "0.00", "0"),
            (-5e-16, "-0.000000000000001", "0.00", "0"),
            (0.0, "0.000000000000000", "0.00", "0"),
        ];
        for &(value, d15, d2, d0) in cases {
            assert_eq!(double_text(value, 15), d15, "{value:e},15");
            assert_eq!(double_text(value, 2), d2, "{value:e},2");
            assert_eq!(double_text(value, 0), d0, "{value:e},0");
        }
        // Past 32 characters, the integer part is cut off.
        assert_eq!(double_text(1e100, 15).len(), 32);
        assert_eq!(double_text(-9.9999999999999981e307, 15).len(), 32);
    }

    #[test]
    fn texts() {
        let any = layout(FieldType::Character, 10, 0);
        assert_eq!(text(Value::Numeric("  12345.67".into()), any), "  12345.67");
        assert_eq!(text(Value::Integer(-2), any), "-2");
        assert_eq!(text(Value::Logical(true), any), "T");
        assert_eq!(
            field_text(&Value::Binary(vec![0, 1]), &any, ""),
            FieldText::Bytes(vec![0, 1])
        );
    }

    /// As the original engine writes them.
    #[test]
    fn nulls() {
        let null =
            |field_type, length, decimals| text(Value::Null, layout(field_type, length, decimals));
        assert_eq!(null(FieldType::Character, 10, 0), " ".repeat(10));
        assert_eq!(null(FieldType::CiCharacter, 10, 0), " ".repeat(10));
        assert_eq!(null(FieldType::NChar, 5, 0), " ".repeat(5));
        assert_eq!(null(FieldType::Numeric, 10, 2), " ".repeat(10));
        assert_eq!(null(FieldType::VarChar, 20, 0), "");
        assert_eq!(null(FieldType::Memo, 9, 0), "");
        assert_eq!(null(FieldType::Integer, 4, 0), "0");
        assert_eq!(null(FieldType::ShortInt, 2, 0), "0");
        assert_eq!(null(FieldType::LongInt, 8, 0), "0");
        assert_eq!(null(FieldType::Double, 8, 4), "0.0000");
        assert_eq!(null(FieldType::Double, 8, 0), "0");
        assert_eq!(null(FieldType::CurDouble, 8, 2), "0.00");
        assert_eq!(null(FieldType::Money, 8, 4), ".0000");
        assert_eq!(null(FieldType::Logical, 1, 0), " ");
        assert_eq!(null(FieldType::Date, 4, 0), "  /  /    ");
        assert_eq!(null(FieldType::Time, 4, 0), "12:00:00 AM");
        assert_eq!(null(FieldType::TimeStamp, 8, 0), "  /  /     12:00:00 AM");
        assert_eq!(julian_to_ymd(DEC_25_2000).day, 25);
        assert_eq!(
            null(FieldType::Guid, 16, 0),
            "00000000-0000-0000-0000-000000000000"
        );
        for binary in [FieldType::Binary, FieldType::Image, FieldType::VarBinary] {
            let empty = field_text(&Value::Null, &layout(binary, 9, 0), "");
            assert_eq!(empty, FieldText::Bytes(Vec::new()), "{binary:?}");
        }
        let raw = layout(FieldType::Raw, 4, 0);
        assert_eq!(
            field_text(&Value::Null, &raw, ""),
            FieldText::Bytes(vec![0; 4])
        );

        // Other date formats keep their separators.
        let date = layout(FieldType::Date, 4, 0);
        let null_date = |format| match field_text(&Value::Null, &date, format) {
            FieldText::Text(t) => t,
            FieldText::Bytes(_) => panic!("bytes"),
        };
        assert_eq!(null_date("DD.MM.YYYY"), "  .  .    ");
        assert_eq!(null_date("YYYY-MM-DD"), "    -  -  ");
        assert_eq!(null_date("MM/DD/YY"), "  /  /  ");
    }

    /// As the original engine writes them.
    #[test]
    fn times() {
        let any = layout(FieldType::Time, 4, 0);
        assert_eq!(text(Value::Time(49_512_250), any), "01:45:12 PM");
        assert_eq!(text(Value::Time(0), any), "12:00:00 AM");
        assert_eq!(text(Value::Time(43_200_000), any), "12:00:00 PM");
        assert_eq!(text(Value::Time(9_000_000), any), "02:30:00 AM");
        let stamp = Value::TimeStamp(NOV_8_2017, 49_512_250);
        assert_eq!(text(stamp, any), "11/08/2017 01:45:12 PM");
    }

    /// As the original engine writes them.
    #[test]
    fn numbers() {
        let double = |decimals| layout(FieldType::Double, 8, decimals);
        assert_eq!(text(Value::Double(1.23456), double(4)), "1.2346");
        assert_eq!(text(Value::Double(1.56789), double(0)), "2");
        assert_eq!(text(Value::Double(0.0), double(4)), "0.0000");
        let cur_double = layout(FieldType::CurDouble, 8, 2);
        assert_eq!(text(Value::Double(2.5), cur_double), "2.50");

        let money = layout(FieldType::Money, 8, 4);
        assert_eq!(text(Value::Money(12_345_678), money), "1234.5678");
        assert_eq!(text(Value::Money(0), money), ".0000");
        assert_eq!(text(Value::Money(-12_345_000), money), "-1234.5000");
        assert_eq!(text(Value::Money(5_000), money), ".5000");
        assert_eq!(text(Value::Money(-5_000), money), "-.5000");
        assert_eq!(text(Value::Money(1), money), ".0001");
    }
}
