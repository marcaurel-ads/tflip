//! Field values, decoded from records and memo files.

use serde::{Deserialize, Serialize};

/// A decoded field value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    /// Character, CICharacter, VarChar, NChar, NVarChar, Memo and NMemo,
    /// as stored (fixed-length values keep their padding).
    Text(String),
    /// Binary, Image, Raw and VarBinary.
    Binary(Vec<u8>),
    Logical(bool),
    /// Numeric, as its ASCII digits are stored (right-aligned, space-padded).
    Numeric(String),
    /// ShortInt, Integer, LongInt, AutoInc and RowVersion.
    Integer(i128),
    /// Double and CurDouble.
    Double(f64),
    /// Money, as the value × 10,000.
    Money(i64),
    /// Date, as a Julian day number.
    Date(i32),
    /// Time, as milliseconds since midnight.
    Time(u32),
    /// TimeStamp and ModTime: a Julian day and milliseconds since midnight.
    TimeStamp(i32, u32),
    /// GUID, in Windows `GUID` byte order.
    Guid([u8; 16]),
    /// A blank Date, TimeStamp or Logical in a VFP table, which is not NULL
    /// (spaces, or zeros for a TimeStamp).
    Blank,
}

/// A calendar date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ymd {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

/// The date `julian` as `YYYYMMDD`, or spaces for 0 (an empty date).
pub fn date_text(julian: i32) -> [u8; 8] {
    let mut text = [b' '; 8];
    if julian != 0 {
        let date = julian_to_ymd(julian);
        let digits = format!("{:04}{:02}{:02}", date.year, date.month, date.day);
        text.copy_from_slice(&digits.as_bytes()[..8]);
    }
    text
}

/// The calendar date of Julian day number `julian` (the proleptic Gregorian
/// calendar; day 2458066 is 2017-11-08).
pub fn julian_to_ymd(julian: i32) -> Ymd {
    // Fliegel and Van Flandern's algorithm.
    let l = i64::from(julian) + 68569;
    let n = 4 * l / 146097;
    let l = l - (146097 * n + 3) / 4;
    let i = 4000 * (l + 1) / 1461001;
    let l = l - 1461 * i / 4 + 31;
    let j = 80 * l / 2447;
    let day = l - 2447 * j / 80;
    let l = j / 11;
    let month = j + 2 - 12 * l;
    let year = 100 * (n - 49) + i + l;
    Ymd {
        year: year as i32,
        month: month as u32,
        day: day as u32,
    }
}

/// The Julian day number of a calendar date, the reverse of
/// [`julian_to_ymd`]; `None` for a month or day that does not exist.
pub fn ymd_to_julian(date: Ymd) -> Option<i32> {
    let Ymd { year, month, day } = date;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if day == 0 || day > days {
        return None;
    }
    // Fliegel and Van Flandern's algorithm.
    let (y, m, d) = (i64::from(year), i64::from(month), i64::from(day));
    let a = (m - 14) / 12;
    let julian = 1461 * (y + 4800 + a) / 4 + 367 * (m - 2 - 12 * a) / 12
        - 3 * ((y + 4900 + a) / 100) / 4
        + d
        - 32075;
    i32::try_from(julian).ok()
}

/// The decimal number `text` × 10^`decimals`, rounded half away from zero:
/// an optional sign, digits with an optional decimal point, and spaces
/// around them (`  -12.5`). `None` for anything else, or a number too large.
pub fn scaled_decimal(text: &str, decimals: u32) -> Option<i128> {
    let text = text.trim_matches(|c: char| c.is_ascii_whitespace());
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() && fraction.is_empty() || !all_digits(whole) || !all_digits(fraction) {
        return None;
    }
    let mut value: i128 = 0;
    for digit in whole
        .bytes()
        .chain(fraction.bytes().chain(std::iter::repeat(b'0')))
        .take(whole.len() + decimals as usize)
    {
        value = value
            .checked_mul(10)?
            .checked_add(i128::from(digit - b'0'))?;
    }
    if fraction
        .as_bytes()
        .get(decimals as usize)
        .is_some_and(|&b| b >= b'5')
    {
        value = value.checked_add(1)?;
    }
    Some(if negative { -value } else { value })
}

impl Value {
    /// The GUID in its usual text form, `01234567-89ab-cdef-0123-456789abcdef`
    /// (the first three groups are little-endian in the stored bytes).
    pub fn guid_text(bytes: &[u8; 16]) -> String {
        let d1 = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let d2 = u16::from_le_bytes([bytes[4], bytes[5]]);
        let d3 = u16::from_le_bytes([bytes[6], bytes[7]]);
        let rest: String = bytes[8..].iter().map(|b| format!("{b:02x}")).collect();
        format!("{d1:08x}-{d2:04x}-{d3:04x}-{}-{}", &rest[..4], &rest[4..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn julian_days() {
        assert_eq!(
            julian_to_ymd(2458066),
            Ymd {
                year: 2017,
                month: 11,
                day: 8
            }
        );
        assert_eq!(
            julian_to_ymd(1721426),
            Ymd {
                year: 1,
                month: 1,
                day: 1
            }
        );
        assert_eq!(
            julian_to_ymd(2451604),
            Ymd {
                year: 2000,
                month: 2,
                day: 29
            }
        );
        assert_eq!(
            julian_to_ymd(2451605),
            Ymd {
                year: 2000,
                month: 3,
                day: 1
            }
        );
        for julian in [0, 1721426, 2299161, 2440588, 2451604, 2458066, 5373484] {
            assert_eq!(ymd_to_julian(julian_to_ymd(julian)), Some(julian));
        }
        assert_eq!(
            ymd_to_julian(Ymd {
                year: 1900,
                month: 2,
                day: 29
            }),
            None
        );
        assert_eq!(
            ymd_to_julian(Ymd {
                year: 2000,
                month: 2,
                day: 29
            }),
            Some(2451604)
        );
        assert_eq!(
            ymd_to_julian(Ymd {
                year: 2017,
                month: 13,
                day: 1
            }),
            None
        );
        assert_eq!(
            ymd_to_julian(Ymd {
                year: 2017,
                month: 4,
                day: 31
            }),
            None
        );
    }

    #[test]
    fn decimals() {
        assert_eq!(scaled_decimal("1.5", 2), Some(150));
        assert_eq!(scaled_decimal("  -12.505 ", 2), Some(-1251));
        assert_eq!(scaled_decimal("1.495", 2), Some(150));
        assert_eq!(scaled_decimal("1.5049", 2), Some(150));
        assert_eq!(scaled_decimal("+.5", 0), Some(1));
        assert_eq!(scaled_decimal("1.", 4), Some(10000));
        assert_eq!(scaled_decimal("7", 0), Some(7));
        for bad in ["", "-", ".", "1e5", "1,5", "1 .5", "--1", "1.5.0", "1.5x"] {
            assert_eq!(scaled_decimal(bad, 2), None, "{bad}");
        }
    }

    #[test]
    fn guids() {
        let bytes = [
            0x67, 0x45, 0x23, 0x01, 0xab, 0x89, 0xef, 0xcd, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef,
        ];
        assert_eq!(
            Value::guid_text(&bytes),
            "01234567-89ab-cdef-0123-456789abcdef"
        );
    }
}
