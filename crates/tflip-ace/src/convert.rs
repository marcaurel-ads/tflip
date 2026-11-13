//! The unpublished conversions between dates, times and their texts
//! (`AdsConvertJulianToString`, `AdsConvertStringToJulian`,
//! `AdsConvertDateToJulian`, `AdsConvertMillisecondsToString`,
//! `AdsConvertStringToMilliseconds`), as `aceunpub.pas` declares them and the
//! original engine answers them (probed).

use tflip_storage::value::{Ymd, julian_to_ymd, ymd_to_julian};

use crate::ffi::{c_str, slice};
use crate::last_error::{Call, fail, fail_text};
use crate::{
    AE_INSUFFICIENT_BUFFER, AE_INVALID_DATE, AE_INVALID_POINTER, AE_INVALID_VALUE, AE_SUCCESS,
    AdsHandle, LOCAL, Unsigned8, Unsigned16, Unsigned32, error_code, write_cut,
};

/// Milliseconds in a day.
const DAY_MS: Unsigned32 = 86_400_000;

/// The `CCYYMMDD` text of the Julian day number `julian`, as the original
/// engine writes it: 0 is no date (eight spaces), a fraction is cut off, and
/// a year that does not fit in four characters is `****`. (Some days before
/// the year 1 come out a day earlier in the original engine.)
fn julian_text(julian: f64) -> String {
    if julian == 0.0 {
        return " ".repeat(8);
    }
    let day = julian.trunc() as i64;
    let Ymd { year, month, day } =
        julian_to_ymd(day.clamp(i32::MIN.into(), i32::MAX.into()) as i32);
    let year = match year {
        -999..=9999 => format!("{year:04}"),
        _ => "****".to_owned(),
    };
    format!("{year}{month:02}{day:02}")
}

/// `AdsConvertJulianToString`: the date of the Julian day number `julian` as
/// `CCYYMMDD` (see `julian_text`) in `buf` of `*len` bytes. As in the
/// original engine, a buffer of 8 bytes or less gets what fits and
/// `AE_INSUFFICIENT_BUFFER`, with `*len` set to 9.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsConvertJulianToString(
    julian: f64,
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsConvertJulianToString");
    // SAFETY: `buf` and `len` are as # Safety says.
    unsafe { write_cut(julian_text(julian).as_bytes(), buf, len) }
}

/// `atoi` of `bytes`: blanks, a sign and the digits after them.
fn atoi(bytes: &[u8]) -> i32 {
    let bytes = bytes.trim_ascii_start();
    let (negative, digits) = match bytes.first() {
        Some(b'-') => (true, &bytes[1..]),
        Some(b'+') => (false, &bytes[1..]),
        _ => (false, bytes),
    };
    let value = digits
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .fold(0i32, |n, d| {
            n.wrapping_mul(10).wrapping_add(i32::from(d - b'0'))
        });
    if negative { -value } else { value }
}

/// The Julian day number of the date `text` gives as `CCYYMMDD`, as the
/// original engine reads it: each part with `atoi`, the parts all zero being
/// no date (0) unless the text is empty. `Err` holds the detail of `AE_INVALID_DATE`, and whether the
/// result is set to 0.
fn text_julian(text: &[u8]) -> Result<f64, (&'static str, bool)> {
    let part = |from: usize, to: usize| atoi(&text[from.min(text.len())..to.min(text.len())]);
    let (year, month, day) = (part(0, 4), part(4, 6), part(6, 8));
    if (year, month, day) == (0, 0, 0) && !text.is_empty() {
        return Ok(0.0);
    }
    if year == 0 || month == 0 || day == 0 {
        return Err(("Date won't convert to julian.", false));
    }
    let date = (year >= 1 && month >= 1 && day >= 1)
        .then(|| {
            ymd_to_julian(Ymd {
                year,
                month: month as u32,
                day: day as u32,
            })
        })
        .flatten();
    date.map(f64::from)
        .ok_or(("Converted to negative julian date.", true))
}

/// `AdsConvertStringToJulian`: the Julian day number of the date `text`
/// gives as `CCYYMMDD` (see `text_julian`). As in the original engine,
/// `text` is read up to its NUL, whatever `len` says.
///
/// # Safety
///
/// `text` must be null or point to a NUL-terminated string; `julian` must be
/// null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsConvertStringToJulian(
    text: *mut Unsigned8,
    _len: Unsigned16,
    julian: *mut f64,
) -> Unsigned32 {
    let _call = Call::begin("AdsConvertStringToJulian");
    // SAFETY: `text` is null or NUL-terminated and `julian` null or
    // writable, as # Safety says.
    let (text, julian) = unsafe { (c_str(text), julian.as_mut()) };
    let (Some(text), Some(julian)) = (text, julian) else {
        return fail(AE_INVALID_POINTER);
    };
    match text_julian(text) {
        Ok(value) => {
            *julian = value;
            AE_SUCCESS
        }
        Err((detail, zero)) => {
            if zero {
                *julian = 0.0;
            }
            invalid_date(detail)
        }
    }
}

/// Fails with `AE_INVALID_DATE` and the original engine's `detail`.
fn invalid_date(detail: &str) -> Unsigned32 {
    let text = crate::last_error::error_text(AE_INVALID_DATE).unwrap_or_default();
    fail_text(
        AE_INVALID_DATE,
        format!("Error {AE_INVALID_DATE}:  {text}  {detail}"),
    )
}

/// `AdsConvertDateToJulian`: the Julian day number of the date `text` gives
/// in the date format (`AdsSetDateFormat`) and epoch, as `AdsSetDate` reads
/// it; text that is no date is 0. `connect` must be a connection.
///
/// # Safety
///
/// `text` must be null or hold `len` bytes; `julian` must be null or valid
/// for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsConvertDateToJulian(
    connect: AdsHandle,
    text: *mut Unsigned8,
    len: Unsigned16,
    julian: *mut f64,
) -> Unsigned32 {
    let _call = Call::begin("AdsConvertDateToJulian");
    if let Err(code) = crate::connection::checked(connect) {
        return code;
    }
    // SAFETY: `text` is null or holds `len` bytes and `julian` is null or
    // writable, as # Safety says.
    let (bytes, julian) = unsafe { (slice(text, usize::from(len)), julian.as_mut()) };
    let (Some(bytes), Some(julian)) = (bytes, julian) else {
        return fail(AE_INVALID_POINTER);
    };
    let text = crate::ansi::to_string(bytes);
    let format = tflip_engine::settings::date_format(&LOCAL);
    let dates = tflip_engine::setters::Dates {
        format: &format,
        epoch: tflip_engine::settings::epoch(&LOCAL),
    };
    match tflip_engine::setters::date(&text, dates) {
        Ok(day) => {
            *julian = day.map_or(0.0, f64::from);
            AE_SUCCESS
        }
        Err(error) => error_code(error),
    }
}

/// `AdsConvertMillisecondsToString`: the time of day `milliseconds` after
/// midnight as `HH:MM:SS.mmm` in `buf` of `*len` bytes, or as `HH:MM:SS` in
/// a buffer of 9 to 12 bytes. As in the original engine, `*len` is set to
/// the length with the NUL, a smaller buffer fails with
/// `AE_INSUFFICIENT_BUFFER` (and is left alone), and a day or more with
/// `AE_INVALID_VALUE`.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsConvertMillisecondsToString(
    milliseconds: Unsigned32,
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsConvertMillisecondsToString");
    if milliseconds >= DAY_MS {
        return fail(AE_INVALID_VALUE);
    }
    if buf.is_null() || len.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    let seconds = milliseconds / 1000;
    let mut text = format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    // SAFETY: not null, so readable, as # Safety says.
    let size = unsafe { *len };
    match size {
        0..=8 => return fail(AE_INSUFFICIENT_BUFFER),
        9..=12 => {}
        _ => text += &format!(".{:03}", milliseconds % 1000),
    }
    let mut bytes = text.into_bytes();
    bytes.push(0);
    // SAFETY: neither is null, and the text with its NUL is at most 13
    // bytes, which fit the `*len` bytes of `buf` checked above.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
        *len = bytes.len() as Unsigned16;
    }
    AE_SUCCESS
}

/// The milliseconds after midnight of the time `text` gives, as the
/// original engine reads it: the first four runs of digits are the hours,
/// minutes, seconds and milliseconds, each read into 16 bits and not
/// checked; `PM` anywhere adds 12 hours to an hour under 12, and `AM` makes
/// 12 o'clock 0.
fn text_milliseconds(text: &[u8]) -> Unsigned32 {
    let mut parts = [0u16; 4];
    let mut runs = text
        .split(|b| !b.is_ascii_digit())
        .filter(|run| !run.is_empty());
    for (part, run) in parts.iter_mut().zip(&mut runs) {
        *part = run.iter().fold(0u16, |n, d| {
            n.wrapping_mul(10).wrapping_add(u16::from(d - b'0'))
        });
    }
    let upper = text.to_ascii_uppercase();
    let has = |word: &[u8]| upper.windows(2).any(|w| w == word);
    let [mut hours, minutes, seconds, milliseconds] = parts.map(Unsigned32::from);
    if has(b"PM") && hours < 12 {
        hours += 12;
    } else if has(b"AM") && hours == 12 {
        hours = 0;
    }
    ((hours * 60 + minutes) * 60 + seconds)
        .wrapping_mul(1000)
        .wrapping_add(milliseconds)
}

/// `AdsConvertStringToMilliseconds`: the milliseconds after midnight of the
/// time in the `len` bytes of `text` (see `text_milliseconds`); text that
/// is no time is 0.
///
/// # Safety
///
/// `text` must be null or hold `len` bytes; `milliseconds` must be null or
/// valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsConvertStringToMilliseconds(
    text: *mut Unsigned8,
    len: Unsigned16,
    milliseconds: *mut Unsigned32,
) -> Unsigned32 {
    let _call = Call::begin("AdsConvertStringToMilliseconds");
    // SAFETY: `text` is null or holds `len` bytes and `milliseconds` is null
    // or writable, as # Safety says.
    let (text, milliseconds) = unsafe { (slice(text, usize::from(len)), milliseconds.as_mut()) };
    let (Some(text), Some(milliseconds)) = (text, milliseconds) else {
        return fail(AE_INVALID_POINTER);
    };
    *milliseconds = text_milliseconds(text);
    AE_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    fn julian_string(julian: f64, size: u16) -> (Unsigned32, u16, String) {
        let (mut buf, mut len) = ([b'#'; 16], size);
        let rc = unsafe { AdsConvertJulianToString(julian, buf.as_mut_ptr(), &mut len) };
        let text = &buf[..usize::from(len.min(size).min(16))];
        (rc, len, String::from_utf8_lossy(text).into_owned())
    }

    /// As the original engine answers (probed).
    #[test]
    fn julian_days_to_text() {
        let ok = |julian: f64| match julian_string(julian, 64) {
            (AE_SUCCESS, 8, text) => text,
            other => panic!("{julian}: {other:?}"),
        };
        assert_eq!(ok(2451545.0), "20000101");
        assert_eq!(ok(2451545.9), "20000101");
        assert_eq!(ok(0.0), "        ");
        assert_eq!(ok(2440588.0), "19700101");
        assert_eq!(ok(2459000.5), "20200530");
        assert_eq!(ok(1721426.0), "00010101");
        assert_eq!(ok(5373484.0), "99991231");
        assert_eq!(ok(2299160.0), "15821014");
        // Before the year 1 (where the original engine gives some days a
        // day earlier: 1721425 is 00001230 there).
        assert_eq!(ok(1720000.0), "-0030205");
        assert_eq!(ok(1000000.0), "****1021");
        assert_eq!(ok(5373485.0), "****0101");
        assert_eq!(ok(9999999.0), "****1219");
        assert_eq!(
            julian_string(2451545.0, 8),
            (AE_INSUFFICIENT_BUFFER, 9, "20000101".into())
        );
        assert_eq!(
            julian_string(2451545.0, 5),
            (AE_INSUFFICIENT_BUFFER, 9, "20000".into())
        );
        assert_eq!(
            julian_string(2451545.0, 9),
            (AE_SUCCESS, 8, "20000101".into())
        );
        let rc =
            unsafe { AdsConvertJulianToString(0.0, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(rc, AE_INVALID_POINTER);
    }

    fn last_text() -> String {
        let (mut code, mut buf, mut len) = (0, [0u8; 300], 300);
        unsafe { crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned()
    }

    /// As the original engine answers (probed).
    #[test]
    fn texts_to_julian_days() {
        let convert = |text: &CStr, len: u16| {
            let mut julian = -1.0;
            let rc = unsafe {
                AdsConvertStringToJulian(text.as_ptr().cast_mut().cast(), len, &mut julian)
            };
            (rc, julian)
        };
        let ok = |text: &CStr| convert(text, text.count_bytes() as u16);
        let wont = (AE_INVALID_DATE, -1.0);
        let negative = (AE_INVALID_DATE, 0.0);
        assert_eq!(ok(c"20000101"), (AE_SUCCESS, 2451545.0));
        assert_eq!(ok(c"20000101 "), (AE_SUCCESS, 2451545.0));
        // The length is not looked at.
        assert_eq!(convert(c"20000102", 4), (AE_SUCCESS, 2451546.0));
        assert_eq!(ok(c"2000013"), (AE_SUCCESS, 2451547.0));
        assert_eq!(ok(c"00010101"), (AE_SUCCESS, 1721426.0));
        assert_eq!(ok(c"99991231"), (AE_SUCCESS, 5373484.0));
        assert_eq!(ok(c"20000229"), (AE_SUCCESS, 2451604.0));
        // Each part read with atoi.
        assert_eq!(ok(c"01/01/2000"), (AE_SUCCESS, 1721445.0));
        assert_eq!(ok(c"12/31/1999"), (AE_SUCCESS, 1725461.0));
        assert_eq!(ok(c"13/01/2000"), (AE_SUCCESS, 1725828.0));
        assert_eq!(ok(c"+2000101"), (AE_SUCCESS, 1794109.0));
        assert_eq!(ok(c" 2000101"), (AE_SUCCESS, 1794109.0));
        assert_eq!(ok(c"20 00101"), (AE_SUCCESS, 1728365.0));
        assert_eq!(ok(c"00000000"), (AE_SUCCESS, 0.0));
        assert_eq!(ok(c"        "), (AE_SUCCESS, 0.0));
        assert_eq!(ok(c"  /  /    "), (AE_SUCCESS, 0.0));
        assert_eq!(ok(c""), wont);
        assert_eq!(ok(c"2000"), wont);
        assert_eq!(ok(c"200001"), wont);
        assert_eq!(ok(c"1/2/2000"), wont);
        assert_eq!(ok(c"00000101"), wont);
        assert_eq!(ok(c"20000100"), wont);
        assert_eq!(ok(c"00000001"), wont);
        assert_eq!(
            last_text(),
            "Error 5095:  The date specified is not valid.  Date won't convert to julian."
        );
        assert_eq!(ok(c"20001301"), negative);
        assert_eq!(ok(c"20000132"), negative);
        assert_eq!(ok(c"19000229"), negative);
        assert_eq!(ok(c"-0010101"), negative);
        assert_eq!(
            last_text(),
            "Error 5095:  The date specified is not valid.  Converted to negative julian date."
        );
        let rc = unsafe {
            AdsConvertStringToJulian(c"x".as_ptr().cast_mut().cast(), 1, std::ptr::null_mut())
        };
        assert_eq!(rc, AE_INVALID_POINTER);
    }

    /// As the original engine answers (probed).
    #[test]
    fn dates_to_julian_days() {
        let _settings = crate::fixtures::settings_lock();
        let (connect, table) = crate::fixtures::open("all_types", c"all_types.adt");
        let convert = |handle: AdsHandle, text: &str| {
            let mut julian = -1.0;
            let rc = unsafe {
                AdsConvertDateToJulian(
                    handle,
                    text.as_ptr().cast_mut(),
                    text.len() as u16,
                    &mut julian,
                )
            };
            (rc, julian)
        };
        assert_eq!(convert(connect, "01/01/2000"), (AE_SUCCESS, 2451545.0));
        assert_eq!(convert(connect, "12/31/1999"), (AE_SUCCESS, 2451544.0));
        assert_eq!(convert(connect, "1/2/2000"), (AE_SUCCESS, 2451546.0));
        assert_eq!(convert(connect, ""), (AE_SUCCESS, 0.0));
        assert_eq!(convert(connect, "  /  /    "), (AE_SUCCESS, 0.0));
        assert_eq!(convert(connect, "abc"), (AE_SUCCESS, 0.0));
        assert_eq!(convert(connect, "01/01/00"), (AE_SUCCESS, 2415021.0));
        assert_eq!(convert(connect, "01/01"), (AE_SUCCESS, 2415021.0));
        assert_eq!(convert(connect, "01/01/0001"), (AE_SUCCESS, 1721426.0));
        assert_eq!(convert(connect, "01/01/2000xyz"), (AE_SUCCESS, 2451545.0));
        assert_eq!(
            convert(connect, "13/01/2000"),
            (crate::AE_INVALID_MONTH, -1.0)
        );
        assert_eq!(
            convert(connect, "20000101"),
            (crate::AE_INVALID_MONTH, -1.0)
        );
        assert_eq!(
            convert(connect, "02/30/2000"),
            (crate::AE_INVALID_DAY, -1.0)
        );
        assert_eq!(
            convert(connect, "02/29/1900"),
            (crate::AE_INVALID_DAY, -1.0)
        );
        assert_eq!(
            convert(table, "01/01/2000"),
            (crate::AE_INVALID_CONNECTION_HANDLE, -1.0)
        );
        assert_eq!(
            convert(0, "01/01/2000"),
            (crate::AE_INVALID_CONNECTION_HANDLE, -1.0)
        );
        assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
    }

    fn time_string(milliseconds: Unsigned32, size: u16) -> (Unsigned32, u16, String) {
        let (mut buf, mut len) = ([b'#'; 16], size);
        let rc =
            unsafe { AdsConvertMillisecondsToString(milliseconds, buf.as_mut_ptr(), &mut len) };
        let text = buf
            .iter()
            .take_while(|&&b| b != 0 && b != b'#')
            .copied()
            .collect::<Vec<_>>();
        (rc, len, String::from_utf8(text).unwrap())
    }

    /// As the original engine answers (probed).
    #[test]
    fn milliseconds_to_text() {
        assert_eq!(time_string(0, 64), (AE_SUCCESS, 13, "00:00:00.000".into()));
        assert_eq!(
            time_string(999, 64),
            (AE_SUCCESS, 13, "00:00:00.999".into())
        );
        assert_eq!(
            time_string(45296789, 13),
            (AE_SUCCESS, 13, "12:34:56.789".into())
        );
        assert_eq!(
            time_string(86399999, 64),
            (AE_SUCCESS, 13, "23:59:59.999".into())
        );
        assert_eq!(
            time_string(45296789, 12),
            (AE_SUCCESS, 9, "12:34:56".into())
        );
        assert_eq!(time_string(45296789, 9), (AE_SUCCESS, 9, "12:34:56".into()));
        assert_eq!(
            time_string(45296789, 8),
            (AE_INSUFFICIENT_BUFFER, 8, String::new())
        );
        assert_eq!(
            time_string(86400000, 64),
            (AE_INVALID_VALUE, 64, String::new())
        );
        assert_eq!(
            time_string(u32::MAX, 64),
            (AE_INVALID_VALUE, 64, String::new())
        );
        let rc = unsafe {
            AdsConvertMillisecondsToString(5, std::ptr::null_mut(), std::ptr::null_mut())
        };
        assert_eq!(rc, AE_INVALID_POINTER);
    }

    /// As the original engine answers (probed).
    #[test]
    fn texts_to_milliseconds() {
        let convert = |text: &str| {
            let mut milliseconds = 7;
            let rc = unsafe {
                AdsConvertStringToMilliseconds(
                    text.as_ptr().cast_mut(),
                    text.len() as u16,
                    &mut milliseconds,
                )
            };
            assert_eq!(rc, AE_SUCCESS);
            milliseconds
        };
        for (text, expected) in [
            ("12:34:56", 45296000),
            ("23:59:59", 86399000),
            ("12:34:56.789", 45296789),
            ("12:34:56.7", 45296007),
            ("12:34:56.1234", 45297234),
            ("12:34", 45240000),
            ("1:2:3", 3723000),
            ("  12 : 34", 45240000),
            ("12.34", 45240000),
            ("-1:00:00", 3600000),
            ("12:-5:00", 43500000),
            ("1:2:3:4:5", 3723004),
            ("1 2 3", 3723000),
            ("1:00 7 PM", 46807000),
            ("25:00:00", 90000000),
            ("100:00:00", 360000000),
            ("12:60:00", 46800000),
            ("65536:00", 0),
            ("1:65536", 3600000),
            ("12:34:56 PM", 45296000),
            ("12:34:56 AM", 2096000),
            ("01:02:03 pm", 46923000),
            ("13:00:00 PM", 46800000),
            ("00:00:00 PM", 43200000),
            ("12:00 AM", 0),
            ("1:00PM", 46800000),
            ("1:00 xx PM", 46800000),
            ("x1:00 PM", 46800000),
            ("1:00P", 3600000),
            ("12:34:56.789 PM", 45296789),
            ("abc", 0),
            ("", 0),
        ] {
            assert_eq!(convert(text), expected, "{text}");
        }
        // Only `len` bytes are read.
        let mut milliseconds = 7;
        let text = "1:00 PM";
        unsafe { AdsConvertStringToMilliseconds(text.as_ptr().cast_mut(), 4, &mut milliseconds) };
        assert_eq!(milliseconds, 3600000);
    }
}
