//! Dates and times as the original engine writes and reads them, in date
//! formats such as `MM/DD/CCYY` (`AdsSetDateFormat`).

use tflip_storage::value::{Ymd, julian_to_ymd, ymd_to_julian};

/// A date that does not exist (`02/30/2003`), or text that is not a date in
/// the format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidDate;

/// Julian day `julian` in `format`, where runs of `D` (day), `M` (month) and
/// `Y` or `C` (year; `CC` is the century) are replaced, in either case: two
/// letters zero-padded (`MM`, `DD`), and the year as two digits for a run of
/// two (`YY`), else four (`YYYY`, `CCYY`). Other characters are kept. (A
/// single letter is written without padding.)
pub fn format_date(julian: i32, format: &str) -> String {
    let date = julian_to_ymd(julian);
    let mut text = String::new();
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let kind = chars[i].to_ascii_uppercase();
        let same = |c: char| match kind {
            'Y' | 'C' => matches!(c.to_ascii_uppercase(), 'Y' | 'C'),
            _ => c.to_ascii_uppercase() == kind,
        };
        let run = chars[i..].iter().take_while(|&&c| same(c)).count();
        match kind {
            'D' => text += &pad(date.day, run),
            'M' => text += &pad(date.month, run),
            'Y' | 'C' if run == 2 => text += &format!("{:02}", date.year.rem_euclid(100)),
            'Y' | 'C' => text += &format!("{:04}", date.year),
            _ => text.push(chars[i]),
        }
        i += if matches!(kind, 'D' | 'M' | 'Y' | 'C') {
            run
        } else {
            1
        };
    }
    text
}

fn pad(n: u32, run: usize) -> String {
    if run == 1 {
        n.to_string()
    } else {
        format!("{n:02}")
    }
}

/// The date `text` starts with, read in the order of `format`'s fields
/// (`MM/DD/CCYY`), as the original engine reads Date values, seek keys and
/// `CTOD`'s argument: its Julian day and where the rest of `text` starts.
/// `None` for the empty date: text without digits, or `00/00/0000`.
///
/// Spaces are left out wherever they are (`11 8 2017` is `1182017`). Then
/// each field is up to two digits (four for the year), and exactly one
/// character after each but the last is skipped, whatever it is: a separator
/// (`11/8/2017`, `11-08-2017` and `11.08.2017` are all the same date) or a
/// digit (`1108/2017` is 2017-11-08, `01011900` 1900-01-11). A missing field
/// is 0. A year of up to two digits is in the 1900s (see [`parse_date_with`]
/// for another epoch), one of three or four digits is that year (`11/082017`
/// is 0017-11-08). Anything after the year is ignored. A date that does not
/// exist, or is in the year 0, is [`InvalidDate`] ([`date_fields`] tells
/// which part is wrong).
///
/// A format without separators (`CCYYMMDD`, `DDMMYY`) is read the same way
/// with nothing skipped: each field is as many digits as it has, up to its
/// width (four for the year, `YY` too), spaces left out (`1182017` is
/// 1182-01-07, `171108` in `YYMMDD` the year 1711 without a day, `081117
/// 12:34` in `DDMMYY` 1712-11-08).
pub fn parse_date(text: &str, format: &str) -> Result<Option<(i32, usize)>, InvalidDate> {
    parse_date_with(text, format, DEFAULT_EPOCH)
}

/// The epoch until a client sets another (`AdsSetEpoch`).
pub const DEFAULT_EPOCH: u16 = 1900;

/// The year a year of up to two digits is, under `epoch` (`AdsSetEpoch`):
/// the one in the hundred years from `epoch` on (49 is 2049 under 1950, 50
/// is 1950).
pub fn century_year(year: u32, epoch: u16) -> u32 {
    let epoch = u32::from(epoch);
    let year = epoch / 100 * 100 + year;
    if year < epoch { year + 100 } else { year }
}

/// As [`parse_date`], with years of up to two digits under `epoch` (see
/// [`century_year`]).
pub fn parse_date_with(
    text: &str,
    format: &str,
    epoch: u16,
) -> Result<Option<(i32, usize)>, InvalidDate> {
    if !text.bytes().any(|b| b.is_ascii_digit()) {
        return Ok(None);
    }
    let fields = date_fields(text, format, epoch);
    if (fields.year, fields.month, fields.day) == (0, 0, 0) {
        return Ok(None);
    }
    if fields.year == 0 {
        return Err(InvalidDate);
    }
    let date = Ymd {
        year: fields.year as i32,
        month: fields.month,
        day: fields.day,
    };
    ymd_to_julian(date)
        .map(|julian| Some((julian, fields.end)))
        .ok_or(InvalidDate)
}

/// Whether `format` has no separators (`CCYYMMDD`).
fn is_packed(format: &str) -> bool {
    format
        .bytes()
        .all(|b| matches!(b, b'C' | b'Y' | b'M' | b'D'))
}

/// The fields of a date as [`parse_date_with`] reads them, before they are
/// checked: the year (one of up to two digits
/// already in its century), month and day, each 0 when missing, and where
/// the rest of the text starts. A setter tells a wrong month (not 1 to 12)
/// from a wrong day by them, as the original engine does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateFields {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub end: usize,
}

/// `text`'s date fields in `format` (see [`DateFields`]).
pub fn date_fields(text: &str, format: &str, epoch: u16) -> DateFields {
    let mut fields: Vec<(usize, u8)> = ["MM", "DD", "YY"]
        .iter()
        .filter_map(|f| format.find(f).map(|at| (at, f.as_bytes()[0])))
        .collect();
    fields.sort();
    let separated = !is_packed(format);
    // The text without its spaces, each byte with where it is in `text`.
    let bytes: Vec<(usize, u8)> = text
        .bytes()
        .enumerate()
        .filter(|&(_, b)| b != b' ')
        .collect();
    let (mut year, mut month, mut day, mut year_digits) = (0, 0, 0, 0);
    let (mut at, mut end, mut read) = (0, 0, 0);
    for (i, &(_, field)) in fields.iter().enumerate() {
        let max = if field == b'Y' { 4 } else { 2 };
        let digits = bytes[at..]
            .iter()
            .take(max)
            .take_while(|(_, b)| b.is_ascii_digit());
        let (mut value, mut count) = (0, 0);
        for &(position, digit) in digits {
            value = value * 10 + u32::from(digit - b'0');
            count += 1;
            end = position + 1;
        }
        at += count;
        read += count;
        match field {
            b'Y' => (year, year_digits) = (value, count),
            b'M' => month = value,
            _ => day = value,
        }
        if separated
            && i < fields.len() - 1
            && let Some(&(skipped, _)) = bytes.get(at)
        {
            at += 1;
            end = skipped + 1;
        }
    }
    // With no digit read there is no date, not one in the epoch's century
    // (`x20171108` in `CCYYMMDD`, `x/y1`).
    if year_digits <= 2 && read > 0 {
        year = century_year(year, epoch);
    }
    DateFields {
        year,
        month,
        day,
        end,
    }
}

/// Where the date part of a timestamp's `text` ends and its time starts, as
/// the original engine splits a TimeStamp value's text, a TimeStamp seek
/// key and `CTOTS`'s argument (`fixtures/fields/dates.tsv`, `tstexts.tsv`):
/// at the third separator — a run of characters that are not digits, after
/// a digit — wherever the date's own fields end, in any format
/// (`11/08/2017 13:00` at the space, `1108/2017 13:00` at the colon, so its
/// time is `00`; `20171108 12:34` in `CCYYMMDD` not at all). With fewer, the
/// whole text is the date and there is no time.
pub fn timestamp_split(text: &str) -> (usize, usize) {
    let mut wanted = 3;
    let bytes = text.as_bytes();
    let mut seen_digit = false;
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at].is_ascii_digit() {
            seen_digit = true;
            at += 1;
            continue;
        }
        let start = at;
        while at < bytes.len() && !bytes[at].is_ascii_digit() {
            at += 1;
        }
        if seen_digit {
            wanted -= 1;
            if wanted == 0 {
                return (start, at);
            }
        }
    }
    (text.len(), text.len())
}

/// A timestamp's `text` as the original engine reads it: its date part
/// (see [`timestamp_split`]) read as a Date value is ([`parse_date_with`]),
/// and the time after it ([`timestamp_time`], not checked). `None` for no
/// date.
pub fn parse_timestamp_with(
    text: &str,
    format: &str,
    epoch: u16,
) -> Result<Option<(i32, u32)>, InvalidDate> {
    let (end, time) = timestamp_split(text);
    let date = parse_date_with(&text[..end], format, epoch)?;
    Ok(date.map(|(day, _)| (day, timestamp_time(&text[time..]))))
}

/// The time of day in a timestamp's text after its date, in milliseconds,
/// as the original engine reads it: up to four numbers (hours, minutes,
/// seconds, milliseconds), each after one character that is not a digit;
/// anything before the first digit is skipped, and `AM` (taking 12 hours
/// from 12 or more) or `PM` (adding 12 to less) may follow. Nothing is checked (`1:90` is 2:30, `25:00` more than a day); each part
/// is read into 16 bits, and the milliseconds they add up to into 32.
pub fn timestamp_time(text: &str) -> u32 {
    let bytes = text.as_bytes();
    let Some(mut at) = bytes.iter().position(u8::is_ascii_digit) else {
        return 0;
    };
    let number = |at: &mut usize| {
        let count = bytes[*at..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        let value = bytes[*at..*at + count].iter().fold(0u16, |n, d| {
            n.wrapping_mul(10).wrapping_add(u16::from(d - b'0'))
        });
        *at += count;
        u32::from(value)
    };
    let mut parts = [number(&mut at), 0, 0, 0];
    for part in &mut parts[1..] {
        let separated = bytes.get(at).is_some_and(|b| !b.is_ascii_digit());
        if !separated || !bytes.get(at + 1).is_some_and(u8::is_ascii_digit) {
            break;
        }
        at += 1;
        *part = number(&mut at);
    }
    let [mut hours, minutes, seconds, milliseconds] = parts;
    match text[at..].trim_start().as_bytes().first() {
        Some(b'P' | b'p') if hours < 12 => hours += 12,
        Some(b'A' | b'a') if hours >= 12 => hours -= 12,
        _ => {}
    }
    hours
        .wrapping_mul(3_600_000)
        .wrapping_add(minutes.wrapping_mul(60_000))
        .wrapping_add(seconds.wrapping_mul(1000))
        .wrapping_add(milliseconds)
}

/// The time of day `text` holds, in milliseconds, as the original engine
/// reads seek keys and `CTOT`'s argument: anything before the first digit
/// is skipped; hours, minutes and seconds are separated by `:` or `.`, and
/// digits after the seconds are milliseconds (`.5` is 5 ms, `.500` 500 ms);
/// `AM` or `PM` may follow. Nothing is checked: `12:60` is 13:00.
pub fn parse_time(text: &str) -> u32 {
    let bytes = text.as_bytes();
    let Some(mut at) = bytes.iter().position(u8::is_ascii_digit) else {
        return 0;
    };
    let number = |at: &mut usize| {
        let count = bytes[*at..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        let value = text[*at..*at + count].bytes().fold(0u64, |n, d| {
            n.wrapping_mul(10).wrapping_add(u64::from(d - b'0'))
        });
        *at += count;
        value
    };
    let mut parts = [number(&mut at), 0, 0, 0];
    for part in &mut parts[1..] {
        let separated = matches!(bytes.get(at), Some(b':' | b'.'));
        if !separated || !bytes.get(at + 1).is_some_and(u8::is_ascii_digit) {
            break;
        }
        at += 1;
        *part = number(&mut at);
    }
    let [mut hours, minutes, seconds, milliseconds] = parts;
    while bytes.get(at) == Some(&b' ') {
        at += 1;
    }
    match bytes.get(at) {
        Some(b'P' | b'p') if hours < 12 => hours += 12,
        Some(b'A' | b'a') if hours == 12 => hours = 0,
        _ => {}
    }
    let total = ((hours * 60 + minutes) * 60 + seconds) * 1000 + milliseconds;
    total as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOV_8_2017: i32 = 2458066;
    const MDY: &str = "MM/DD/CCYY";

    fn julian(year: i32, month: u32, day: u32) -> i32 {
        ymd_to_julian(Ymd { year, month, day }).unwrap()
    }

    #[test]
    fn dates() {
        assert_eq!(format_date(NOV_8_2017, "MM/DD/CCYY"), "11/08/2017");
        assert_eq!(format_date(NOV_8_2017, "M/d/yyyy"), "11/8/2017");
        assert_eq!(format_date(NOV_8_2017, "DD.MM.YYYY"), "08.11.2017");
        assert_eq!(format_date(NOV_8_2017, "yy-mm-dd"), "17-11-08");
    }

    /// As the original engine reads them (see the module documentation).
    #[test]
    fn dates_as_the_engine_reads_them() {
        let date = |text| parse_date(text, MDY).map(|d| d.map(|(julian, _)| julian));
        for (text, expected) in [
            ("11/08/2017", julian(2017, 11, 8)),
            ("11/8/2017", julian(2017, 11, 8)),
            (" 1-1-1900", julian(1900, 1, 1)),
            ("01.01.1900x", julian(1900, 1, 1)),
            ("01/01/ 1900", julian(1900, 1, 1)),
            ("01/01/19000", julian(1900, 1, 1)),
            ("11/08/17", julian(1917, 11, 8)),
            ("01/01/0", julian(1900, 1, 1)),
            ("01/01", julian(1900, 1, 1)),
            ("01/01/", julian(1900, 1, 1)),
            ("11/08/201", julian(201, 11, 8)),
            ("11/08/2017 12:00", julian(2017, 11, 8)),
            // One character is skipped after a field, digit or not, and
            // spaces are left out.
            ("1108/2017", julian(2017, 11, 8)),
            ("01011900", julian(1900, 1, 11)),
            ("1182017", julian(1907, 11, 20)),
            ("11 8 2017", julian(1907, 11, 20)),
            ("11/082017", julian(17, 11, 8)),
            ("11/08/17 12:34", julian(1712, 11, 8)),
            ("1108", julian(1900, 11, 8)),
        ] {
            assert_eq!(date(text), Ok(Some(expected)), "{text}");
        }
        for empty in ["", "garbage", "  /  /    ", "ab/cd/efgh", "00/00/0000"] {
            assert_eq!(date(empty), Ok(None), "{empty}");
        }
        for invalid in [
            "2017-11-08",
            "13/08/2017",
            "02/30/2017",
            "02/29/1900",
            "00/00/00",
            "x01/01/1900",
            "/01/1900",
            "11 08 2017",
            "011/08/2017",
            "01//1900",
            "1",
            "01/",
            "11082017",
            "110817",
            "1 1 2000",
            "11/08/0000",
        ] {
            assert_eq!(date(invalid), Err(InvalidDate), "{invalid}");
        }
        let date = |text, format| parse_date(text, format).unwrap().unwrap().0;
        assert_eq!(date("8.11.2017", "DD.MM.CCYY"), julian(2017, 11, 8));
        assert_eq!(date("2017/11/08", "YY/MM/DD"), julian(2017, 11, 8));
        assert_eq!(date("00/01/01", "YY/MM/DD"), julian(1900, 1, 1));
        assert_eq!(
            parse_date("11/08/2017T12", MDY),
            Ok(Some((julian(2017, 11, 8), 10)))
        );
        // Formats without separators: nothing skipped, as the original engine
        // reads them.
        assert_eq!(date("20171108", "CCYYMMDD"), julian(2017, 11, 8));
        assert_eq!(
            date("1182017", "CCYYMMDD"),
            julian(1182, 1, 7),
            "a short last field"
        );
        assert_eq!(date("081117", "DDMMYY"), julian(1917, 11, 8));
        assert_eq!(
            date("081117 12:34", "DDMMYY"),
            julian(1712, 11, 8),
            "a four-digit year"
        );
        assert_eq!(
            date(" 2017 11 08", "CCYYMMDD"),
            julian(2017, 11, 8),
            "spaces left out"
        );
        for refused in [
            "11082017",
            "110817",
            "1108",
            "01011",
            "1/1/1900",
            "11 08 2017",
        ] {
            assert_eq!(
                parse_date(refused, "CCYYMMDD"),
                Err(InvalidDate),
                "{refused}"
            );
        }
    }

    #[test]
    fn times_as_the_engine_reads_them() {
        let ms = |h: u32, m: u32, s: u32, ms: u32| ((h * 60 + m) * 60 + s) * 1000 + ms;
        for (text, expected) in [
            ("12:34:56", ms(12, 34, 56, 0)),
            ("12:34:56.789", ms(12, 34, 56, 789)),
            ("12:34:56.7", ms(12, 34, 56, 7)),
            ("00:00:00.0010", 10),
            ("00:00:00:001", 1),
            ("12:34:55.1789", ms(12, 34, 56, 789)),
            ("12.34.56", ms(12, 34, 56, 0)),
            ("12:34", ms(12, 34, 0, 0)),
            ("12", ms(12, 0, 0, 0)),
            ("12:60", ms(13, 0, 0, 0)),
            ("12:34:56 PM", ms(12, 34, 56, 0)),
            ("1:00pm", ms(13, 0, 0, 0)),
            ("00:00 PM", ms(12, 0, 0, 0)),
            ("12:00 AM", 0),
            ("12:34:56 AM", ms(0, 34, 56, 0)),
            ("-1:00", ms(1, 0, 0, 0)),
            ("a12:00x", ms(12, 0, 0, 0)),
            ("x", 0),
            ("", 0),
        ] {
            assert_eq!(parse_time(text), expected, "{text}");
        }
    }
}
