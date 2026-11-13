//! Reading field values: as text (`AdsGetField`, `AdsGetFieldW`), as
//! binary data (`AdsGetBinary`, `AdsBinaryToFile`), whether they are NULL
//! or empty, and through the typed getters, which give a field's value in
//! the current record as a number, a logical, a date or time, or text of
//! its own (`AdsGetLong`, `AdsGetString`, `AdsGetDate`, ...), as the
//! original engine converts it (`fixtures/fields/getters.json`, probed with
//! `tools/ace/fields/`).
//!
//! Each getter takes some field types and fails with
//! `AE_INVALID_FIELD_TYPE` on the others, before it looks at the record
//! (at BOF or EOF, `AE_NO_CURRENT_RECORD`). NULL is 0, false, or the text
//! `AdsGetField` writes for it.

use std::ffi::OsStr;

use tflip_engine::Error;
use tflip_engine::format::FieldText;
use tflip_engine::getters::{Charset, Null, Scalar, Want};
use tflip_engine::table::{LengthUnit, find_field};
use tflip_storage::field::FieldType;

use crate::backend::api;
use crate::ffi::{path_arg, wide_path_arg, write_units};
use crate::field::{FieldArg, fields, with_field, write};
use crate::last_error::null_param;
use crate::last_error::{Call, fail};
use crate::{
    ADS_BYTE_LENGTH, ADS_BYTE_LENGTH_IN_BUFFER, ADS_CODEUNIT_LENGTH, ADS_DONT_CHECK_CONV_ERR,
    ADS_FALSE, ADS_GET_FORMAT_ANSI, ADS_GET_GUID_FILE, ADS_GET_GUID_MIME, ADS_GET_GUID_NUMBERS,
    ADS_GET_GUID_REGISTRY, ADS_GET_UTF8, ADS_LTRIM, ADS_RTRIM, ADS_TRIM, ADS_TRUE,
    AE_BINARY_FILE_ERROR, AE_INSUFFICIENT_BUFFER, AE_INVALID_FIELD_TYPE, AE_INVALID_OPTION,
    AE_SUCCESS, AE_UNICODE_CONVERSION, AdsHandle, Unsigned8, Unsigned16, Unsigned32, Wchar, ansi,
    error_code, write_cut,
};

/// Reads `field` of `table` as `want` and writes what `pick` takes of it
/// to `out` (`None`: a null pointer).
fn get<T>(
    table: AdsHandle,
    field: FieldArg,
    want: Want,
    out: Option<&mut T>,
    param: u32,
    pick: impl FnOnce(Scalar) -> Option<T>,
) -> Unsigned32 {
    let value = field_as(table, field, want, out.is_none().then_some(param))
        .and_then(|scalar| pick(scalar).ok_or_else(|| fail(AE_INVALID_FIELD_TYPE)));
    put(value, out, param)
}

/// Writes `value` to `out`, or fails naming the call's argument number
/// `param` if `out` is `None` (a null pointer), as [`write_at`] does.
fn put<T>(value: Result<T, Unsigned32>, out: Option<&mut T>, param: u32) -> Unsigned32 {
    match (value, out) {
        (Err(code), _) => code,
        (Ok(_), None) => null_param(param),
        (Ok(value), Some(out)) => {
            *out = value;
            AE_SUCCESS
        }
    }
}

/// Whether a getter has nowhere to write its text: no `len` (`size`, the
/// value it points to), or no buffer of a size other than 0, which only
/// asks how long the text is.
/// The number of the argument that is missing then: the buffer's, `param`,
/// or the length's, the next.
fn no_output<T>(buf: *const T, size: Option<Unsigned32>, param: u32) -> Option<u32> {
    if buf.is_null() && size != Some(0) {
        Some(param)
    } else if size.is_none() {
        Some(param + 1)
    } else {
        None
    }
}

/// `AdsGetLong`: a numeric field's value as a whole number (doubles
/// truncated toward zero, Money rounded half away from zero); one beyond
/// 32 bits fails with `AE_VALUE_OVERFLOW`. Numeric, the integer types,
/// Double, CurDouble and Money only. See [`tflip_engine::getters`].
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a
/// NUL-terminated string; `value` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetLong(
    table: AdsHandle,
    field: *mut Unsigned8,
    value: *mut i32,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetLong");
    // SAFETY: `field` is a field number or a string and `value` null or
    // writable, as # Safety says.
    let (field, value) = unsafe { (FieldArg::new(field), value.as_mut()) };
    get(table, field, Want::Long, value, 3, |s| match s {
        Scalar::Long(n) => Some(n),
        _ => None,
    })
}

/// `AdsGetLongLong`: as `AdsGetLong`, in 64 bits; a Double beyond them is
/// `i64::MIN`, without an error, as in the original engine.
///
/// # Safety
///
/// As for `AdsGetLong`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetLongLong(
    table: AdsHandle,
    field: *mut Unsigned8,
    value: *mut i64,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetLongLong");
    // SAFETY: `field` is a field number or a string and `value` null or
    // writable, as # Safety says.
    let (field, value) = unsafe { (FieldArg::new(field), value.as_mut()) };
    get(table, field, Want::LongLong, value, 3, |s| match s {
        Scalar::LongLong(n) => Some(n),
        _ => None,
    })
}

/// `AdsGetShort`: as `AdsGetLong`, then cut to its low 16 bits, as the
/// original engine does (123456789 is -13035).
///
/// # Safety
///
/// As for `AdsGetLong`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetShort(
    table: AdsHandle,
    field: *mut Unsigned8,
    value: *mut i16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetShort");
    // SAFETY: `field` is a field number or a string and `value` null or
    // writable, as # Safety says.
    let (field, value) = unsafe { (FieldArg::new(field), value.as_mut()) };
    get(table, field, Want::Short, value, 3, |s| match s {
        Scalar::Short(n) => Some(n),
        _ => None,
    })
}

/// `AdsGetDouble`: a numeric field's value as a double.
///
/// # Safety
///
/// As for `AdsGetLong`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetDouble(
    table: AdsHandle,
    field: *mut Unsigned8,
    value: *mut f64,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetDouble");
    // SAFETY: `field` is a field number or a string and `value` null or
    // writable, as # Safety says.
    let (field, value) = unsafe { (FieldArg::new(field), value.as_mut()) };
    get(table, field, Want::Double, value, 3, |s| match s {
        Scalar::Double(d) => Some(d),
        _ => None,
    })
}

/// `AdsGetMoney`: a numeric field's value × 10,000, rounded; one beyond 64
/// bits fails with `AE_VALUE_OVERFLOW`.
///
/// # Safety
///
/// As for `AdsGetLong`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetMoney(
    table: AdsHandle,
    field: *mut Unsigned8,
    value: *mut i64,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetMoney");
    // SAFETY: `field` is a field number or a string and `value` null or
    // writable, as # Safety says.
    let (field, value) = unsafe { (FieldArg::new(field), value.as_mut()) };
    get(table, field, Want::Money, value, 3, |s| match s {
        Scalar::Money(m) => Some(m),
        _ => None,
    })
}

/// `AdsGetLogical`: a Logical field's value, `ADS_TRUE` or `ADS_FALSE`
/// (NULL is false).
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a
/// NUL-terminated string; `value` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetLogical(
    table: AdsHandle,
    field: *mut Unsigned8,
    value: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetLogical");
    // SAFETY: `field` is a field number or a string and `value` null or
    // writable, as # Safety says.
    let (field, value) = unsafe { (FieldArg::new(field), value.as_mut()) };
    get(table, field, Want::Logical, value, 3, |s| match s {
        Scalar::Logical(true) => Some(ADS_TRUE),
        Scalar::Logical(false) => Some(ADS_FALSE),
        _ => None,
    })
}

/// `AdsGetJulian`: the Julian day of a Date, TimeStamp or ModTime field's
/// value (2458066 is 2017-11-08); 0 for NULL.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a
/// NUL-terminated string; `value` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetJulian(
    table: AdsHandle,
    field: *mut Unsigned8,
    value: *mut i32,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetJulian");
    // SAFETY: `field` is a field number or a string and `value` null or
    // writable, as # Safety says.
    let (field, value) = unsafe { (FieldArg::new(field), value.as_mut()) };
    get(table, field, Want::Julian, value, 3, |s| match s {
        Scalar::Julian(julian) => Some(julian),
        _ => None,
    })
}

/// `AdsGetMilliseconds`: the milliseconds since midnight of a Time,
/// TimeStamp or ModTime field's value; 0 for NULL.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a
/// NUL-terminated string; `value` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetMilliseconds(
    table: AdsHandle,
    field: *mut Unsigned8,
    value: *mut i32,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetMilliseconds");
    // SAFETY: `field` is a field number or a string and `value` null or
    // writable, as # Safety says.
    let (field, value) = unsafe { (FieldArg::new(field), value.as_mut()) };
    get(table, field, Want::Milliseconds, value, 3, |s| match s {
        Scalar::Milliseconds(ms) => Some(ms as i32),
        _ => None,
    })
}

/// `AdsGetMemoLength`: the length of a Memo, NMemo, Binary or Image field's
/// value: bytes, and characters for NMemo; 0 for NULL.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a
/// NUL-terminated string; `length` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetMemoLength(
    table: AdsHandle,
    field: *mut Unsigned8,
    length: *mut Unsigned32,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetMemoLength");
    // SAFETY: `field` is a field number or a string and `length` null or
    // writable, as # Safety says.
    let (field, length) = unsafe { (FieldArg::new(field), length.as_mut()) };
    get(table, field, Want::MemoLength, length, 3, |s| match s {
        Scalar::Length(n) => Some(n),
        _ => None,
    })
}

/// `AdsGetString`: a field's value as text, written as `AdsGetField`
/// writes it (trimming, `ADS_GET_UTF8`, buffers that are too small,
/// characters the ANSI code page lacks), for the character, memo, binary,
/// Logical, Numeric and Date types: dates as `CCYYMMDD` whatever the date
/// format, and binary values in base64 (`AP8Q`). With `ADS_GET_UTF8`,
/// Numeric, Logical and Date fail with `AE_INVALID_FIELD_TYPE`, as in the
/// original engine.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a
/// NUL-terminated string; `buf` must be null or hold `*len` bytes; `len`
/// must be null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetString(
    table: AdsHandle,
    field: *mut Unsigned8,
    buf: *mut Unsigned8,
    len: *mut Unsigned32,
    option: Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetString");
    // A connection that trims trailing spaces does for a caller who did not
    // say how to trim (`fixtures/config/connect101.json`).
    let option = match option & ADS_TRIM == 0
        && api(table)
            .table_info(table)
            .is_ok_and(|info| crate::connect_options::trims(info.connection))
    {
        true => option | ADS_RTRIM,
        false => option,
    };
    // SAFETY: `field` is a field number or a string and `len` null or
    // readable, as # Safety says.
    let (field, size) = unsafe { (FieldArg::new(field), len.as_ref().copied()) };
    let no_output = no_output(buf, size, 3);
    let want = Want::String {
        wide: false,
        utf8: option & ADS_GET_UTF8 != 0,
    };
    match field_as(table, field, want, no_output) {
        // SAFETY: `buf` and `len` are as # Safety says.
        Ok(Scalar::FieldText { text, charset }) => unsafe {
            write_text(text, charset, option, buf, len, 3)
        },
        Ok(_) => fail(AE_INVALID_FIELD_TYPE),
        Err(code) => code,
    }
}

/// `AdsGetStringW`: as `AdsGetString`, in UTF-16 code units, written as
/// `AdsGetFieldW` writes them; VarBinary fields fail with
/// `AE_INVALID_FIELD_TYPE` and `ADS_GET_UTF8` with `AE_INVALID_OPTION`, as
/// in the original engine.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a
/// NUL-terminated string; `buf` must be null or hold `*len` code units;
/// `len` must be null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetStringW(
    table: AdsHandle,
    field: *mut Unsigned8,
    buf: *mut Wchar,
    len: *mut Unsigned32,
    option: Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetStringW");
    if option & ADS_GET_UTF8 != 0 {
        return fail(AE_INVALID_OPTION);
    }
    // SAFETY: `field` is a field number or a string and `len` null or
    // readable, as # Safety says.
    let (field, size) = unsafe { (FieldArg::new(field), len.as_ref().copied()) };
    let no_output = no_output(buf, size, 3);
    let want = Want::String {
        wide: true,
        utf8: false,
    };
    match field_as(table, field, want, no_output) {
        // SAFETY: `buf` and `len` are as # Safety says.
        Ok(Scalar::FieldText { text, charset }) => unsafe {
            write_wide_text(text, charset, option, buf, len, 3)
        },
        Ok(_) => fail(AE_INVALID_FIELD_TYPE),
        Err(code) => code,
    }
}

/// Reads `field` of `table` as `want` (a date or a time) and writes its
/// text to `buf` of `*len` bytes, for `AdsGetDate` and `AdsGetTime`.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a
/// NUL-terminated string; `buf` must be null or hold `*len` bytes; `len`
/// must be null or valid for reads and writes.
unsafe fn write_short_text(
    table: AdsHandle,
    field: *const Unsigned8,
    want: Want,
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    // SAFETY: `field` is a field number or a string and `len` null or
    // readable, as the caller promises.
    let (field, size) = unsafe { (FieldArg::new(field), len.as_ref().copied()) };
    let no_output = no_output(buf, size.map(Unsigned32::from), 3);
    match field_as(table, field, want, no_output) {
        // SAFETY: `buf` and `len` are as the caller promises.
        Ok(Scalar::Text(text)) => unsafe { write_cut(text.as_bytes(), buf, len) },
        Ok(_) => fail(AE_INVALID_FIELD_TYPE),
        Err(code) => code,
    }
}

/// `AdsGetDate`: the date of a Date, TimeStamp or ModTime field's value in
/// the date format (`AdsSetDateFormat`); NULL as the format's separators
/// with spaces (`  /  /    `).
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a
/// NUL-terminated string; `buf` must be null or hold `*len` bytes; `len`
/// must be null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetDate(
    table: AdsHandle,
    field: *mut Unsigned8,
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetDate");
    // SAFETY: the pointers are as # Safety says.
    unsafe { write_short_text(table, field, Want::Date, buf, len) }
}

/// `AdsGetTime`: the time of a Time, TimeStamp or ModTime field's value,
/// 12-hour with AM or PM (`01:45:30 PM`); NULL as midnight.
///
/// # Safety
///
/// As for `AdsGetDate`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetTime(
    table: AdsHandle,
    field: *mut Unsigned8,
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetTime");
    // SAFETY: the pointers are as # Safety says.
    unsafe { write_short_text(table, field, Want::Time, buf, len) }
}

/// `AdsIsFieldBinary`: `ADS_FALSE` for every field of an ADT, Clipper or
/// FoxPro table, as the original engine reports, binary types included; in
/// a VFP table whether the field is flagged binary.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a
/// NUL-terminated string; `binary` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsFieldBinary(
    table: AdsHandle,
    field: *mut Unsigned8,
    binary: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsIsFieldBinary");
    // SAFETY: `field` is a field number or a string and `binary` null or
    // writable, as # Safety says.
    let (field, binary) = unsafe { (FieldArg::new(field), binary.as_mut()) };
    let found = api(table)
        .table_info(table)
        .map_err(error_code)
        .and_then(|info| {
            find_field(fields(&info), field.as_ref())
                .map(|(_, field)| if field.binary() { ADS_TRUE } else { ADS_FALSE })
                .map_err(error_code)
        });
    put(found, binary, 3)
}

/// Writes the value of the Binary or Image field `field` of `table` to the
/// file `path`, made anew; NULL makes it empty. Other types fail with
/// `AE_INVALID_FIELD_TYPE` and a file that cannot be made with
/// `AE_BINARY_FILE_ERROR`, which the original engine checks first. The file is the client's: a server's tables'
/// values are written where the client runs.
fn binary_to_file(
    table: AdsHandle,
    field: FieldArg,
    path: Option<std::path::PathBuf>,
) -> Unsigned32 {
    let backend = api(table);
    let found = backend
        .table_info(table)
        .map_err(error_code)
        .and_then(|info| {
            let (number, found) = find_field(fields(&info), field.as_ref()).map_err(error_code)?;
            Ok((number, found.field_type, found.name.clone()))
        });
    let (number, field_type, name) = match found {
        Ok(found) => found,
        Err(code) => return code,
    };
    let Some(path) = path else {
        return crate::last_error::null_param(3);
    };
    // As in the original engine, the file is made before the field's type
    // is checked, and removed again if it is not one.
    let Ok(mut file) = std::fs::File::create(&path) else {
        return fail(AE_BINARY_FILE_ERROR);
    };
    if !matches!(field_type, FieldType::Binary | FieldType::Image) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return crate::last_error::fail_detail(
            AE_INVALID_FIELD_TYPE,
            &format!("Field: \"{name}\""),
        );
    }
    let value = match backend.read_memo(table, number) {
        Ok(value) => value,
        Err(error) => return error_code(error),
    };
    match std::io::Write::write_all(&mut file, &value) {
        Ok(()) => AE_SUCCESS,
        Err(_) => fail(AE_BINARY_FILE_ERROR),
    }
}

/// `AdsBinaryToFile`: writes a Binary or Image field's value in the current
/// record to the file `file` (ANSI), made anew; see `binary_to_file`.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a
/// NUL-terminated string; `file` must be null or point to a NUL-terminated
/// string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsBinaryToFile(
    table: AdsHandle,
    field: *mut Unsigned8,
    file: *mut Unsigned8,
) -> Unsigned32 {
    let _call = Call::begin("AdsBinaryToFile");
    // SAFETY: `field` is a field number or a string and `file` null or a
    // string, as # Safety says.
    let (field, path) = unsafe { (FieldArg::new(field), path_arg(file)) };
    binary_to_file(table, field, path)
}

/// `AdsBinaryToFileW`: as `AdsBinaryToFile`, with a UTF-16 file name.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a
/// NUL-terminated string; `file` must be null or point to a NUL-terminated
/// UTF-16 string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsBinaryToFileW(
    table: AdsHandle,
    field: *mut Unsigned8,
    file: *mut Wchar,
) -> Unsigned32 {
    let _call = Call::begin("AdsBinaryToFileW");
    // SAFETY: `field` is a field number or a string and `file` null or a
    // UTF-16 string, as # Safety says.
    let (field, path) = unsafe { (FieldArg::new(field), wide_path_arg(file)) };
    binary_to_file(table, field, path)
}

/// Writes whether `field` holds NULL in the current record of `table` to
/// `out` (`None`: a null pointer), as `ADS_TRUE` or `ADS_FALSE`.
fn write_null(
    table: AdsHandle,
    field: FieldArg,
    mut out: Option<&mut Unsigned16>,
    empty: bool,
    param: u32,
) -> Unsigned32 {
    let flag = match api(table).null(table, field.id(), empty) {
        Ok(Null::Is(null)) => Ok(if null { ADS_TRUE } else { ADS_FALSE }),
        // The original engine writes `ADS_FALSE` before failing off a
        // record, for most fields (see `tflip_engine::getters::null`).
        Ok(Null::OffRecord { writes_false }) => {
            if writes_false && let Some(out) = out.as_deref_mut() {
                *out = ADS_FALSE;
            }
            Err(error_code(Error::NoCurrentRecord))
        }
        Err(error) => Err(error_code(error)),
    };
    put(flag, out, param)
}

/// `AdsIsNull`: whether a field holds NULL in the current record. At BOF or
/// EOF it fails with `AE_NO_CURRENT_RECORD`, having written `ADS_FALSE` as
/// the original engine does (see `tflip_engine::getters::null`).
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string; `null` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsNull(
    table: AdsHandle,
    field: *mut Unsigned8,
    null: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsNull");
    // SAFETY: `field` is a field number or a string and `null` null or
    // writable, as # Safety says.
    let (field, null) = unsafe { (FieldArg::new(field), null.as_mut()) };
    write_null(table, field, null, false, 3)
}

/// `AdsIsEmpty`: whether a field is empty in the current record.
///
/// In ADT tables empty means NULL, as `AdsIsNull`: zeros, blank strings and
/// `F` are not empty (as the original engine reports). In DBF tables it
/// means blank (see [`tflip_storage::field::Field::is_empty`]). Off a record it writes
/// `ADS_FALSE` and fails with `AE_NO_CURRENT_RECORD`, as `AdsIsNull` does
/// in most tables.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string; `empty` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsEmpty(
    table: AdsHandle,
    field: *mut Unsigned8,
    empty: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsEmpty");
    // SAFETY: `field` is a field number or a string and `empty` null or
    // writable, as # Safety says.
    let (field, empty) = unsafe { (FieldArg::new(field), empty.as_mut()) };
    write_null(table, field, empty, true, 3)
}

/// `AdsGetBinaryLength`: the length in bytes of a Binary, Image or memo
/// field's value in the current record (UTF-16LE bytes for NMemo); 0 for
/// NULL. Other field types fail with `AE_INVALID_FIELD_TYPE`, BOF and EOF
/// with `AE_NO_CURRENT_RECORD`.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string; `length` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetBinaryLength(
    table: AdsHandle,
    field: *mut Unsigned8,
    length: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetBinaryLength");
    // SAFETY: `field` is a field number or a string and `length` null or
    // writable, as # Safety says.
    let (field, length) = unsafe { (FieldArg::new(field), length.as_mut()) };
    let backend = api(table);
    // The pointer is checked before the field's type.
    let number = match field.number(&backend, table) {
        Ok(number) => number,
        Err(error) => return error_code(error),
    };
    if length.is_none() {
        return null_param(3);
    }
    let value = backend.memo_length(table, number);
    put(value.map_err(error_code), length, 3)
}

/// `AdsGetBinary`: reads a Binary, Image or memo field's value in the
/// current record from byte `offset` on, as much as fits in `buf` of `*len`
/// bytes; `*len` is set to the bytes copied. Clients read a long value in
/// pieces by advancing `offset`; a buffer of no bytes fails with
/// `AE_INSUFFICIENT_BUFFER` and an offset at or past the end of a value that
/// is not empty with `AE_INVALID_BINARY_OFFSET`, as the original engine does.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string; `buf` must be null or hold `*len` bytes; `len` must be null or
/// valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetBinary(
    table: AdsHandle,
    field: *mut Unsigned8,
    offset: Unsigned32,
    buf: *mut Unsigned8,
    len: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetBinary");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    let backend = api(table);
    // The table and the field are checked first, then the pointers, then
    // the field's type (`fixtures/errors/param_n.json`).
    let number = match field.number(&backend, table) {
        Ok(number) => number,
        Err(error) => return error_code(error),
    };
    if let Some(code) = crate::ffi::null_buffer(buf, len, 4) {
        return code;
    }
    let value = match backend.read_memo(table, number) {
        Ok(value) => value,
        Err(error) => return error_code(error),
    };
    // SAFETY: not null, so readable, as # Safety says.
    let room = unsafe { *len } as usize;
    // An empty value reads as no bytes; otherwise the original engine
    // fails a buffer of no bytes (5005) and an offset past the value
    // (5082), leaving `*len` as it was.
    if !value.is_empty() {
        if room == 0 {
            return fail(crate::AE_INSUFFICIENT_BUFFER);
        }
        if offset as usize >= value.len() {
            return fail(crate::AE_INVALID_BINARY_OFFSET);
        }
    }
    let rest = value.get(offset as usize..).unwrap_or_default();
    let count = rest.len().min(room);
    // SAFETY: neither is null, so `buf` holds the `*len` bytes the count is
    // cut to, and `len` is writable, as # Safety says.
    unsafe {
        std::ptr::copy_nonoverlapping(rest.as_ptr(), buf, count);
        *len = count as Unsigned32;
    }
    AE_SUCCESS
}

/// `AdsGetMemoDataType`: what kind of data a memo field holds, as an `ADS_*`
/// field type constant.
///
/// ADT memo files store values without a type of their own (unlike FoxPro
/// memo files, which mark text and pictures), so this is the field's type:
/// `ADS_MEMO`, `ADS_BINARY`, `ADS_IMAGE` or `ADS_NMEMO`. Other field types
/// fail with `AE_INVALID_FIELD_TYPE`. (Assumed to be what the original engine
/// reports for ADT tables.)
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string; `data_type` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetMemoDataType(
    table: AdsHandle,
    field: *mut Unsigned8,
    data_type: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetMemoDataType");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    let value = with_field(table, field, |f| {
        (f.is_memo().then(|| f.field_type.code()), f.name.clone())
    });
    // The pointer is checked before the field's type, which is named when
    // it is wrong.
    let value = value.and_then(|(code, name)| match (code, data_type.is_null()) {
        (_, true) => Err(null_param(3)),
        (Some(code), false) => Ok(code),
        (None, false) => Err(crate::last_error::fail_detail(
            AE_INVALID_FIELD_TYPE,
            &format!("Field: \"{name}\""),
        )),
    });
    // SAFETY: `data_type` is null or writable, as # Safety says.
    unsafe { write(value, data_type) }
}

/// `AdsGetDataLength`: the length of the data a field holds in the current
/// record. `options` is `ADS_CODEUNIT_LENGTH` (characters; UTF-16 code units
/// in the Unicode types), `ADS_BYTE_LENGTH` (bytes) or
/// `ADS_BYTE_LENGTH_IN_BUFFER` (the field's bytes in the record buffer);
/// other options fail with `AE_INVALID_OPTION`.
///
/// A memo field reports its value's length and a VarChar, VarBinary or
/// NVarChar field its stored length (0 for NULL); other fields report their
/// declared length or storage size, even when NULL, as the original engine
/// does. See [`tflip_engine::table::Table::data_length`].
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string; `length` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetDataLength(
    table: AdsHandle,
    field: *mut Unsigned8,
    options: Unsigned32,
    length: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetDataLength");
    let unit = match options {
        ADS_CODEUNIT_LENGTH => LengthUnit::CodeUnits,
        ADS_BYTE_LENGTH => LengthUnit::Bytes,
        ADS_BYTE_LENGTH_IN_BUFFER => LengthUnit::RecordBytes,
        _ => return AE_INVALID_OPTION,
    };
    // SAFETY: `field` is a field number or a string and `length` null or
    // writable, as # Safety says.
    let (field, length) = unsafe { (FieldArg::new(field), length.as_mut()) };
    let backend = api(table);
    let value = field
        .number(&backend, table)
        .and_then(|field| backend.data_length(table, field, unit));
    put(value.map_err(error_code), length, 3)
}

/// Reads `field` of `table` as `want`, in one call of the engine (see
/// [`tflip_engine::getters::field_as`]). As in the original engine, the
/// table and the field are checked before the output (`no_output`: the
/// getter has nowhere to write, `AE_INVALID_POINTER`), the field's type and
/// the record after it.
pub(crate) fn field_as(
    table: AdsHandle,
    field: FieldArg,
    want: Want,
    no_output: Option<u32>,
) -> Result<Scalar, Unsigned32> {
    let result = api(table).field_as(table, field.id(), want);
    let plain = result.as_ref().map_err(Error::plain);
    match result {
        _ if no_output.is_some()
            && matches!(
                plain,
                Ok(_)
                    | Err(Error::InvalidFieldType | Error::NoCurrentRecord | Error::ValueOverflow)
            ) =>
        {
            Err(null_param(no_output.unwrap_or_default()))
        }
        Ok(scalar) => Ok(scalar),
        Err(error) => Err(error_code(error)),
    }
}

/// The value of a field in the current record as `AdsGetField` writes it,
/// before trimming and conversion; see [`tflip_engine::format::field_text`].
fn field_text(
    table: AdsHandle,
    field: FieldArg,
    option: Unsigned16,
) -> Result<(FieldText, Charset), Unsigned32> {
    let utf8 = option & ADS_GET_UTF8 != 0;
    let ansi_dates = option & ADS_GET_FORMAT_ANSI != 0;
    match field_as(table, field, Want::Field { utf8, ansi_dates }, None)? {
        Scalar::FieldText { text, charset } => Ok((text, charset)),
        _ => Err(fail(AE_INVALID_FIELD_TYPE)),
    }
}

/// `text` with the spaces `option` (`ADS_LTRIM`, `ADS_RTRIM`, `ADS_TRIM`)
/// trims removed.
pub(crate) fn trimmed(text: &str, option: Unsigned16) -> &str {
    match option & ADS_TRIM {
        ADS_LTRIM => text.trim_start_matches(' '),
        ADS_RTRIM => text.trim_end_matches(' '),
        ADS_TRIM => text.trim_matches(' '),
        _ => text,
    }
}

/// `AdsGetField`: the value of a field in the current record as text, in
/// `buf` of `*len` bytes (NUL-terminated); `*len` is set to its length
/// without the NUL. As in the original engine, a buffer that is too small
/// fails with `AE_INSUFFICIENT_BUFFER` after the bytes that fit are copied
/// (without a NUL), and `*len` set to the length needed with the NUL (11 for
/// a Character(10) field).
///
/// `option` may trim spaces (`ADS_LTRIM`, `ADS_RTRIM`, `ADS_TRIM`) and ask for
/// UTF-8 instead of the ANSI code page (`ADS_GET_UTF8`), and write a GUID
/// field in one of its forms (see [`guid_format`]); the other options are
/// ignored. Text is converted to the ANSI code page; as in the original
/// engine, text with a character it lacks (in the Unicode types) fails with
/// `AE_UNICODE_CONVERSION`, after the text up to that character is copied as
/// far as it fits (without a NUL) and `*len` set to its length. Binary
/// values are returned as their bytes. Dates follow the
/// date format of the table's connection (`AdsSetDateFormat60`), else the
/// process-wide one (`AdsSetDateFormat`). See
/// [`tflip_engine::format::field_text`] for how each type is written.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string; `buf` must be null or hold `*len` bytes; `len` must be null or
/// valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetField(
    table: AdsHandle,
    field: *mut Unsigned8,
    buf: *mut Unsigned8,
    len: *mut Unsigned32,
    option: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetField");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    match field_text(table, field.clone(), option) {
        Ok((text, charset)) => {
            let text = match guid_format(table, field, text, option) {
                Ok(text) => text,
                Err(code) => return code,
            };
            // SAFETY: `buf` and `len` are as # Safety says.
            unsafe { write_text(text, charset, option, buf, len, 3) }
        }
        Err(code) => code,
    }
}

/// The text of a GUID field in the form `option` asks for: the bits
/// `ADS_GET_GUID_MIME` (base64 of the 16 bytes as stored), `_FILE` (the same
/// without the `=` padding), `_NUMBERS` (the 32 hex digits) and `_REGISTRY`
/// (the default, 36 characters with dashes, no braces). More than one of
/// them fails with `AE_INVALID_OPTION`, after the record is checked. Other
/// fields keep their text.
fn guid_format(
    table: AdsHandle,
    field: FieldArg,
    text: FieldText,
    option: Unsigned16,
) -> Result<FieldText, Unsigned32> {
    let forms = option
        & (ADS_GET_GUID_MIME | ADS_GET_GUID_FILE | ADS_GET_GUID_NUMBERS | ADS_GET_GUID_REGISTRY);
    let guid = with_field(table, field, |f| f.field_type == FieldType::Guid)?;
    if !guid || forms == 0 || forms == ADS_GET_GUID_REGISTRY {
        return Ok(text);
    }
    if !forms.is_power_of_two() {
        return Err(crate::last_error::fail_detail(
            AE_INVALID_OPTION,
            "Unknown GUID encoding option",
        ));
    }
    let FieldText::Text(dashed) = &text else {
        return Ok(text);
    };
    let digits: String = dashed.chars().filter(|c| *c != '-').collect();
    if forms == ADS_GET_GUID_NUMBERS {
        return Ok(FieldText::Text(digits));
    }
    // The text writes the first three groups from little-endian numbers;
    // the stored bytes are what the base64 encodes.
    let mut bytes: Vec<u8> = (0..16)
        .map(|i| u8::from_str_radix(&digits[2 * i..2 * i + 2], 16).unwrap_or(0))
        .collect();
    bytes[0..4].reverse();
    bytes[4..6].reverse();
    bytes[6..8].reverse();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for k in 0..=chunk.len() {
            out.push(BASE64[(n >> (18 - 6 * k) & 63) as usize] as char);
        }
    }
    if forms == ADS_GET_GUID_MIME {
        out.push_str("==");
    }
    Ok(FieldText::Text(out))
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Writes `text` to `buf` of `*len` bytes as `AdsGetField` does (see
/// there), and `AdsGetString`: trimmed as `option` says; table data as its
/// bytes (in UTF-8 with `ADS_GET_UTF8`, read in the ANSI code page), OEM
/// table data converted to the ANSI code page (or UTF-8), and Unicode text
/// (of the Unicode field types) in UTF-8 with `ADS_GET_UTF8`, else in the
/// ANSI code page, where with `ADS_DONT_CHECK_CONV_ERR` a character the
/// code page lacks is written as 0x1A instead of failing.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
pub(crate) unsafe fn write_text(
    text: FieldText,
    charset: Charset,
    option: Unsigned16,
    buf: *mut Unsigned8,
    len: *mut Unsigned32,
    param: u32,
) -> Unsigned32 {
    // The bytes, and whether they stop short at a character the ANSI code
    // page lacks.
    let binary = matches!(text, FieldText::Bytes(_));
    let (text, lacking) = match text {
        FieldText::Bytes(bytes) => (bytes, false),
        FieldText::Text(text) if option & ADS_GET_UTF8 != 0 => (
            unicode_text(trimmed(&text, option), charset).into_bytes(),
            false,
        ),
        FieldText::Text(text) if charset == Charset::Ansi => {
            (ansi::data_out(trimmed(&text, option)), false)
        }
        FieldText::Text(text) if charset == Charset::Oem => {
            (ansi::oem_to_ansi(trimmed(&text, option)), false)
        }
        FieldText::Text(text) => {
            let text = trimmed(&text, option);
            match ansi::from_os(OsStr::new(text)) {
                Some(bytes) => (bytes, false),
                None if option & ADS_DONT_CHECK_CONV_ERR != 0 => {
                    (ansi::from_str_substituted(text), false)
                }
                None => (ansi::from_str_prefix(text), true),
            }
        }
    };
    // SAFETY: `len` is null or readable, as the caller promises.
    let size = unsafe { len.as_ref().copied() };
    // A null buffer of no bytes is only asked how long the text is.
    if let Some(param) = no_output(buf, size, param) {
        return null_param(param);
    }
    let Some(size) = size else {
        return null_param(param + 1);
    };
    let length = Unsigned32::try_from(text.len()).unwrap_or(Unsigned32::MAX);
    // As in the original engine, a failure copies what fits, without a
    // NUL; binary values have no NUL at all.
    let fits = text.len().min(size as usize);
    let (count, nul, written, rc) = if binary && length <= size {
        (text.len(), false, length, AE_SUCCESS)
    } else if lacking {
        (fits, false, length, fail(AE_UNICODE_CONVERSION))
    } else if length >= size {
        (
            fits,
            false,
            length.saturating_add(1),
            fail(AE_INSUFFICIENT_BUFFER),
        )
    } else {
        (text.len(), true, length, AE_SUCCESS)
    };
    // SAFETY: `len` is writable, and `buf` holds its `size` bytes, which the
    // copy and the NUL stay inside (none for a null `buf` of size 0), as the
    // caller promises.
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), buf, count);
        if nul {
            *buf.add(count) = 0;
        }
        *len = written;
    }
    rc
}

/// `text`, of `charset`, as Unicode.
fn unicode_text(text: &str, charset: Charset) -> String {
    match charset {
        Charset::Unicode => text.to_owned(),
        Charset::Ansi => ansi::data_unicode(text),
        Charset::Oem => ansi::oem_to_unicode(text),
    }
}

/// `AdsGetFieldW`: the value of a field in the current record as UTF-16
/// text, in `buf` of `*len` code units (NUL-terminated); `*len` is set to
/// its length in code units without the NUL. The text is that of
/// `AdsGetField`, so Unicode fields keep every character. As in the
/// original engine:
///
/// - A buffer that is too small fails with `AE_INSUFFICIENT_BUFFER`, after
///   the units that fit are copied (without a NUL), and `*len` set to the
///   length needed with the NUL.
/// - Binary values (Binary, Image, Raw, VarBinary) are copied as their
///   bytes, with no NUL; `*len` is set to half their length (3 bytes are 1
///   unit). If they do not fit, the bytes that fit are copied and `*len` is
///   set to the units needed.
/// - The trim options work as for `AdsGetField`; `ADS_GET_UTF8` fails with
///   `AE_INVALID_OPTION`.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string; `buf` must be null or hold `*len` code units; `len` must be null
/// or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetFieldW(
    table: AdsHandle,
    field: *mut Unsigned8,
    buf: *mut Wchar,
    len: *mut Unsigned32,
    option: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetFieldW");
    if option & ADS_GET_UTF8 != 0 {
        return fail(AE_INVALID_OPTION);
    }
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    match field_text(table, field, option) {
        // SAFETY: `buf` and `len` are as # Safety says.
        Ok((text, charset)) => unsafe { write_wide_text(text, charset, option, buf, len, 3) },
        Err(code) => code,
    }
}

/// Writes `text` to `buf` of `*len` UTF-16 code units as `AdsGetFieldW`
/// does (see there), and `AdsGetStringW`.
///
/// # Safety
///
/// `buf` must be null or hold `*len` code units; `len` must be null or
/// valid for reads and writes.
pub(crate) unsafe fn write_wide_text(
    text: FieldText,
    charset: Charset,
    option: Unsigned16,
    buf: *mut Wchar,
    len: *mut Unsigned32,
    param: u32,
) -> Unsigned32 {
    if let Some(code) = crate::ffi::null_buffer(buf, len, param) {
        return code;
    }
    // SAFETY: not null, so readable, as the caller promises.
    let size = unsafe { *len } as usize;
    match text {
        FieldText::Text(text) => {
            let text = unicode_text(trimmed(&text, option), charset);
            let units: Vec<Wchar> = text.encode_utf16().collect();
            // SAFETY: `buf` holds `size` units and `len` is writable, as the
            // caller promises.
            return unsafe { write_units(&units, buf, len) };
        }
        FieldText::Bytes(bytes) => {
            let room = size.saturating_mul(2);
            let fits = bytes.len() <= room;
            // As the original engine counts them: whole units when they fit,
            // and enough units for every byte when they do not.
            let units = if fits {
                bytes.len() / 2
            } else {
                bytes.len().div_ceil(2)
            };
            let units = Unsigned32::try_from(units).unwrap_or(Unsigned32::MAX);
            // SAFETY: `buf` holds `size` units, the `room` bytes the copy is
            // cut to, and `len` is writable, as the caller promises.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast(), bytes.len().min(room));
                *len = units;
            }
            if !fits {
                return fail(AE_INSUFFICIENT_BUFFER);
            }
        }
    }
    AE_SUCCESS
}

/// `AdsImageToClipboard`: copies the BMP in an Image field of the current
/// record onto the Windows clipboard (as `CF_DIB`; see `crate::clipboard`).
/// As in the original engine, a field of another type fails with
/// `AE_INVALID_FIELD_TYPE`, an unknown field with `AE_INVALID_FIELDNAME`, and
/// data that is not a valid BMP with 5091/5001. Built for a non-Windows host it
/// reports `AE_NOT_IMPLEMENTED` (there is no clipboard to copy to).
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsImageToClipboard(
    table: AdsHandle,
    field: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsImageToClipboard");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    let backend = api(table);
    let found = backend
        .table_info(table)
        .map_err(error_code)
        .and_then(|info| {
            let (number, found) = find_field(fields(&info), field.as_ref()).map_err(error_code)?;
            Ok((number, found.field_type, found.name.clone()))
        });
    let (number, field_type, name) = match found {
        Ok(found) => found,
        Err(code) => return code,
    };
    if field_type != FieldType::Image {
        // The original names the field, or for a memo the type it wants.
        let detail = match field_type {
            FieldType::Memo | FieldType::NMemo | FieldType::Binary => {
                "Type must be ADS_IMAGE.".to_owned()
            }
            _ => format!("Field: \"{name}\""),
        };
        let text = "The requested operation is not legal for the given field type.";
        return crate::last_error::fail_text(
            AE_INVALID_FIELD_TYPE,
            format!("Error {AE_INVALID_FIELD_TYPE}:  {text}  {detail}"),
        );
    }
    match backend.read_memo(table, number) {
        Ok(image) => crate::clipboard::copy_image(&image),
        Err(error) => error_code(error),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, CString};

    /// `ADSFIELD(n)`: field number `n` passed as a field name.
    fn adsfield(n: usize) -> *mut Unsigned8 {
        n as *mut Unsigned8
    }

    use serde_json::{Value as Json, json};

    use super::*;

    use crate::fixtures::{self, remote_twins};
    use crate::{ADS_NONE, AE_INVALID_FIELDNUMBER, AdsSetDateFormat};
    use crate::{AdsGotoBottom, AdsGotoTop, AdsSkip};

    const SENTINEL: u8 = 0x5A;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The bytes of `buf` up to the last that is not the sentinel, as
    /// `fields.c` records them.
    fn written(buf: &[u8]) -> &[u8] {
        let end = buf
            .iter()
            .rposition(|&b| b != SENTINEL)
            .map_or(0, |i| i + 1);
        &buf[..end]
    }

    /// A field argument as `fields.c` passes it: a name, or a number.
    fn field_arg(name: &str, keep: &mut Vec<CString>) -> *mut Unsigned8 {
        match name.parse::<usize>() {
            Ok(number) => std::ptr::without_provenance_mut(number),
            Err(_) => {
                keep.push(CString::new(name).unwrap());
                keep.last().unwrap().as_ptr().cast_mut().cast()
            }
        }
    }

    /// Calls the getter `step` names as `fields.c` did, and records the
    /// result as it does.
    fn call(step: &Json, table: AdsHandle, field: *mut Unsigned8) -> Json {
        let case = step["case"].as_str().unwrap();
        let name = step["fn"].as_str().unwrap();
        let mut got = json!({
            "case": case, "fn": name, "field": step["field"], "rc": 0,
        });
        let size = step["size"].as_u64().unwrap_or(0) as u32;
        let option = step["option"].as_u64().unwrap_or(0) as u16;
        let rc = match (case, name) {
            ("null output", _) => unsafe {
                match name {
                    "AdsGetLong" => AdsGetLong(table, field, std::ptr::null_mut()),
                    "AdsGetLongLong" => AdsGetLongLong(table, field, std::ptr::null_mut()),
                    "AdsGetShort" => AdsGetShort(table, field, std::ptr::null_mut()),
                    "AdsGetDouble" => AdsGetDouble(table, field, std::ptr::null_mut()),
                    "AdsGetLogical" => AdsGetLogical(table, field, std::ptr::null_mut()),
                    "AdsGetJulian" => AdsGetJulian(table, field, std::ptr::null_mut()),
                    "AdsGetMilliseconds" => AdsGetMilliseconds(table, field, std::ptr::null_mut()),
                    "AdsGetMoney" => AdsGetMoney(table, field, std::ptr::null_mut()),
                    "AdsGetMemoLength" => AdsGetMemoLength(table, field, std::ptr::null_mut()),
                    "AdsGetFieldNum" => crate::AdsGetFieldNum(table, field, std::ptr::null_mut()),
                    "AdsIsFieldBinary" => AdsIsFieldBinary(table, field, std::ptr::null_mut()),
                    _ => unreachable!("{name}"),
                }
            },
            ("null buffer", "AdsGetString") => unsafe {
                AdsGetString(table, field, std::ptr::null_mut(), std::ptr::null_mut(), 0)
            },
            ("null buffer", "AdsGetDate") => {
                let mut len = 60;
                unsafe { AdsGetDate(table, field, std::ptr::null_mut(), &mut len) }
            }
            ("null buffer, length 0", _) => {
                let mut len = 0;
                let rc = unsafe { AdsGetString(table, field, std::ptr::null_mut(), &mut len, 0) };
                got["len"] = json!(len);
                rc
            }
            (_, "AdsGetString" | "AdsGetField" | "AdsGetStringW" | "AdsGetFieldW") => {
                let mut buf = vec![SENTINEL; size as usize * 2 + 8];
                let mut len = size;
                let rc = unsafe {
                    match name {
                        "AdsGetString" => {
                            AdsGetString(table, field, buf.as_mut_ptr(), &mut len, option)
                        }
                        "AdsGetField" => {
                            AdsGetField(table, field, buf.as_mut_ptr(), &mut len, option)
                        }
                        "AdsGetStringW" => {
                            AdsGetStringW(table, field, buf.as_mut_ptr().cast(), &mut len, option)
                        }
                        _ => AdsGetFieldW(table, field, buf.as_mut_ptr().cast(), &mut len, option),
                    }
                };
                let unit = if name.ends_with('W') { 2 } else { 1 };
                let end = (size as usize * unit + 4).min(buf.len());
                got["size"] = json!(size);
                got["option"] = json!(option);
                got["len"] = json!(len);
                got["value"] = json!(hex(written(&buf[..end])));
                rc
            }
            (_, "AdsGetDate" | "AdsGetTime") => {
                let mut buf = [SENTINEL; 200];
                let mut len = size as u16;
                let rc = unsafe {
                    let get = if name == "AdsGetDate" {
                        AdsGetDate
                    } else {
                        AdsGetTime
                    };
                    get(table, field, buf.as_mut_ptr(), &mut len)
                };
                got["size"] = json!(size);
                got["len"] = json!(len);
                got["value"] = json!(hex(written(&buf[..size as usize + 4])));
                rc
            }
            _ => {
                let mut out = [SENTINEL; 8];
                let ptr = out.as_mut_ptr();
                let (rc, bytes) = unsafe {
                    match name {
                        "AdsGetLong" => (AdsGetLong(table, field, ptr.cast()), 4),
                        "AdsGetLongLong" => (AdsGetLongLong(table, field, ptr.cast()), 8),
                        "AdsGetShort" => (AdsGetShort(table, field, ptr.cast()), 2),
                        "AdsGetDouble" => (AdsGetDouble(table, field, ptr.cast()), 8),
                        "AdsGetLogical" => (AdsGetLogical(table, field, ptr.cast()), 2),
                        "AdsGetJulian" => (AdsGetJulian(table, field, ptr.cast()), 4),
                        "AdsGetMilliseconds" => (AdsGetMilliseconds(table, field, ptr.cast()), 4),
                        "AdsGetMoney" => (AdsGetMoney(table, field, ptr.cast()), 8),
                        "AdsGetMemoLength" => (AdsGetMemoLength(table, field, ptr.cast()), 4),
                        "AdsGetFieldNum" => (crate::AdsGetFieldNum(table, field, ptr.cast()), 2),
                        "AdsIsFieldBinary" => (AdsIsFieldBinary(table, field, ptr.cast()), 2),
                        "AdsIsNull" => (crate::AdsIsNull(table, field, ptr.cast()), 2),
                        "AdsIsEmpty" => (crate::AdsIsEmpty(table, field, ptr.cast()), 2),
                        "AdsGetFieldType" => (crate::AdsGetFieldType(table, field, ptr.cast()), 2),
                        "AdsGetFieldDecimals" => {
                            (crate::AdsGetFieldDecimals(table, field, ptr.cast()), 2)
                        }
                        "AdsGetFieldLength" => {
                            (crate::AdsGetFieldLength(table, field, ptr.cast()), 4)
                        }
                        _ => unreachable!("{name}"),
                    }
                };
                if name == "AdsGetDouble" {
                    got["value"] = json!(format!("{:.17e}", f64::from_le_bytes(out)));
                }
                got["raw"] = json!(hex(&out[..bytes]));
                rc
            }
        };
        got["rc"] = json!(rc);
        got
    }

    /// Moves `table` to where the case `case` reads: a record, EOF or BOF.
    fn position(table: AdsHandle, case: &str) {
        let case = case.trim_start_matches("keys ");
        if let Some(n) = case.strip_prefix("record ") {
            AdsGotoTop(table);
            AdsSkip(table, n.parse::<i32>().unwrap() - 1);
        } else if case == "eof" {
            AdsGotoBottom(table);
            AdsSkip(table, 1);
        } else if case == "bof" {
            AdsGotoTop(table);
            AdsSkip(table, -1);
        } else {
            AdsGotoTop(table);
        }
    }

    /// The getters give what the original engine gives on the `all_types`
    /// and `adi_keys` fixtures (`fixtures/fields/getters.json`).
    #[test]
    fn getters_match_the_original_engine() {
        let _settings = fixtures::settings_lock();
        let path = fixtures::dir("../fields/getters.json");
        let steps: Vec<Json> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let (connect, all_types) = fixtures::open("all_types", c"all_types.adt");
        let (keys_connect, keys) = fixtures::open("adi_keys", c"keys.adt");
        let set_format =
            |format: &CStr| unsafe { AdsSetDateFormat(format.as_ptr().cast_mut().cast()) };
        set_format(c"MM/DD/CCYY");
        let mut failures = Vec::new();
        let mut keep = Vec::new();
        // As `fields.c` sets them: each once, and back to the default after.
        let mut format = String::from("MM/DD/CCYY");
        for step in &steps {
            let case = step["case"].as_str().unwrap();
            let wanted = case.strip_prefix("date format ").unwrap_or("MM/DD/CCYY");
            if wanted != format {
                set_format(&CString::new(wanted).unwrap());
                format = wanted.to_owned();
            }
            let table = match case {
                "connection" => connect,
                case if case.starts_with("keys ") => keys,
                _ => all_types,
            };
            position(table, case);
            let field = field_arg(step["field"].as_str().unwrap(), &mut keep);
            let mut got = call(step, table, field);
            let mut want = step.clone();
            // `%.17g` and Rust's formatting differ; compare doubles by value.
            for result in [&mut got, &mut want] {
                if let Some(value) = result.get("value").and_then(Json::as_str)
                    && result["fn"] == "AdsGetDouble"
                {
                    let value: f64 = value.parse().unwrap();
                    result["value"] = json!(value.to_bits());
                }
            }
            if got != want {
                failures.push(format!("want {want}\n got {got}"));
            }
        }
        set_format(c"MM/DD/CCYY");
        crate::AdsDisconnect(connect);
        crate::AdsDisconnect(keys_connect);
        assert!(
            failures.is_empty(),
            "{} differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// Binary values go to a file of the client's, as in the original
    /// engine: a NULL one makes it empty, other field types make none.
    #[test]
    fn binary_values_to_files() {
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-binary-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = |name: &str| dir.join(name);
        let c_path = |name: &str| CString::new(path(name).to_str().unwrap()).unwrap();
        let blob = c"c_blob".as_ptr().cast_mut().cast();
        let memo = c"c_memo".as_ptr().cast_mut().cast();
        AdsGotoTop(table);
        let file = c_path("one.bin");
        assert_eq!(
            unsafe { AdsBinaryToFile(table, blob, file.as_ptr().cast_mut().cast()) },
            0
        );
        assert_eq!(std::fs::read(path("one.bin")).unwrap(), [0x00, 0xff, 0x10]);
        let wide: Vec<u16> = path("wide.bin")
            .to_str()
            .unwrap()
            .encode_utf16()
            .chain([0])
            .collect();
        assert_eq!(
            unsafe { AdsBinaryToFileW(table, blob, wide.as_ptr().cast_mut()) },
            0
        );
        assert_eq!(std::fs::read(path("wide.bin")).unwrap(), [0x00, 0xff, 0x10]);
        let file = c_path("memo.bin");
        let rc = unsafe { AdsBinaryToFile(table, memo, file.as_ptr().cast_mut().cast()) };
        assert_eq!(rc, AE_INVALID_FIELD_TYPE);
        assert!(!path("memo.bin").exists());
        let rc = unsafe { AdsBinaryToFile(table, memo, std::ptr::null_mut()) };
        assert_eq!(rc, crate::AE_INVALID_POINTER);
        let file = c_path("no/such/dir.bin");
        let rc = unsafe { AdsBinaryToFile(table, blob, file.as_ptr().cast_mut().cast()) };
        assert_eq!(rc, AE_BINARY_FILE_ERROR);
        AdsSkip(table, 1);
        let file = c_path("null.bin");
        assert_eq!(
            unsafe { AdsBinaryToFile(table, blob, file.as_ptr().cast_mut().cast()) },
            0
        );
        assert_eq!(std::fs::read(path("null.bin")).unwrap(), b"");
        std::fs::remove_dir_all(&dir).unwrap();
        crate::AdsDisconnect(connect);
    }

    /// The fixture folder and file of a table `tools/ace/dbf/run.sh` names
    /// (`ntx/dbf_api.dbf`), with the table type and character type it opens
    /// it as.
    fn dbf_table(name: &str) -> (String, CString, u16, u16) {
        let (kind, file) = name.split_once('/').unwrap();
        let folder = match file {
            "dbf_api.dbf" => "dbf_ext",
            "co.dbf" => "charset_default",
            file => file.trim_end_matches(".dbf"),
        };
        let table_type = match kind {
            "ntx" => crate::ADS_NTX,
            "cdx" => crate::ADS_CDX,
            _ => crate::ADS_VFP,
        };
        let char_type = if file == "co.dbf" {
            crate::ADS_OEM
        } else {
            crate::ADS_ANSI
        };
        (
            format!("{kind}/{folder}"),
            CString::new(file).unwrap(),
            table_type,
            char_type,
        )
    }

    /// Calls the table-level function `step` names as `dbf.c` did; `None`
    /// for a field getter.
    fn dbf_table_call(step: &Json, table: AdsHandle) -> Option<Json> {
        let case = step["case"].as_str().unwrap();
        let name = step["fn"].as_str().unwrap();
        let mut got = json!({
            "table": step["table"], "case": case, "fn": name, "field": "", "rc": 0,
        });
        let mut out = [SENTINEL; 24];
        let ptr = out.as_mut_ptr();
        let (rc, bytes) = unsafe {
            match name {
                "AdsIsRecordDeleted" => (crate::AdsIsRecordDeleted(table, ptr.cast()), 2),
                "AdsGetRecordCount" => {
                    let option = case.rsplit(' ').next().unwrap().parse().unwrap();
                    (crate::AdsGetRecordCount(table, option, ptr.cast()), 4)
                }
                "AdsGetMemoBlockSize" => (crate::AdsGetMemoBlockSize(table, ptr.cast()), 2),
                "AdsGetRecordLength" => (crate::AdsGetRecordLength(table, ptr.cast()), 4),
                "AdsGetTableCharType" => (crate::AdsGetTableCharType(table, ptr.cast()), 2),
                "AdsGetLastTableUpdate" => {
                    let mut len = 20;
                    let rc = crate::AdsGetLastTableUpdate(table, ptr, &mut len);
                    got["len"] = json!(len);
                    got["value"] = json!(hex(written(&out)));
                    got["rc"] = json!(rc);
                    return Some(got);
                }
                "walk" => {
                    let rc = AdsGotoTop(table);
                    let mut records = Vec::new();
                    loop {
                        let mut eof = 1;
                        crate::AdsAtEOF(table, &mut eof);
                        if eof != 0 {
                            break;
                        }
                        let mut record = 0;
                        crate::AdsGetRecordNum(table, 1, &mut record);
                        records.push(record);
                        AdsSkip(table, 1);
                    }
                    got["rc"] = json!(rc);
                    got["records"] = json!(records);
                    return Some(got);
                }
                _ => return None,
            }
        };
        got["rc"] = json!(rc);
        got["raw"] = json!(hex(&out[..bytes]));
        Some(got)
    }

    /// What DBF tables report, field by field and record by record, is what
    /// the original engine reports (`fixtures/dbf/getters.json`, probed with
    /// `tools/ace/dbf/`): the getters, NULL and empty fields, deleted
    /// records, counts, OEM text, and deleted records hidden.
    #[test]
    fn dbf_tables_match_the_original_engine() {
        let _settings = fixtures::settings_lock();
        let path = fixtures::dir("../dbf/getters.json");
        let steps: Vec<Json> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let mut failures = Vec::new();
        let mut keep = Vec::new();
        let mut open: Option<(String, AdsHandle, AdsHandle)> = None;
        for step in &steps {
            let name = step["table"].as_str().unwrap();
            if open.as_ref().is_none_or(|(open, ..)| open != name) {
                if let Some((_, connect, _)) = open.take() {
                    crate::AdsDisconnect(connect);
                }
                let (folder, file, table_type, char_type) = dbf_table(name);
                let connect = fixtures::connect(&folder);
                let mut table = 0;
                let null = std::ptr::null_mut();
                let file = file.as_ptr().cast_mut().cast();
                let rc = unsafe {
                    crate::AdsOpenTable(
                        connect, file, null, table_type, char_type, 1, 1, 0, &mut table,
                    )
                };
                assert_eq!(rc, 0, "{name}");
                open = Some((name.to_owned(), connect, table));
            }
            let (_, _, table) = *open.as_ref().unwrap();
            let case = step["case"].as_str().unwrap();
            if case == "open" {
                continue;
            }
            let hidden = case.starts_with("hidden deleted");
            crate::AdsShowDeleted(if hidden { ADS_FALSE } else { ADS_TRUE });
            if !hidden {
                position(table, case);
            }
            let got = match dbf_table_call(step, table) {
                Some(got) => got,
                None => {
                    let field = field_arg(step["field"].as_str().unwrap(), &mut keep);
                    let mut got = call(step, table, field);
                    got["table"] = step["table"].clone();
                    got
                }
            };
            let mut want = step.clone();
            for result in [&mut want] {
                if let Some(value) = result.get("value").and_then(Json::as_str)
                    && result["fn"] == "AdsGetDouble"
                {
                    let value: f64 = value.parse().unwrap();
                    result["value"] = json!(format!("{value:.17e}"));
                }
            }
            if got != want {
                failures.push(format!("want {want}\n got {got}"));
            }
        }
        crate::AdsShowDeleted(ADS_TRUE);
        if let Some((_, connect, _)) = open {
            crate::AdsDisconnect(connect);
        }
        assert!(
            failures.is_empty(),
            "{} differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn nulls() {
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let null = |field| {
            let mut value = 0xffff;
            assert_eq!(unsafe { AdsIsNull(table, field, &mut value) }, AE_SUCCESS);
            let mut empty = 0xffff;
            assert_eq!(unsafe { AdsIsEmpty(table, field, &mut empty) }, AE_SUCCESS);
            assert_eq!(value, empty);
            value == ADS_TRUE
        };
        // Record 1 has no NULLs; record 3 is NULL in every field but the
        // AutoInc and RowVersion (fields 23 and 24).
        assert_eq!(crate::AdsGotoRecord(table, 1), AE_SUCCESS);
        assert!((1..=24).all(|n| !null(adsfield(n))));
        assert_eq!(crate::AdsGotoRecord(table, 3), AE_SUCCESS);
        assert!((1..=22).all(|n| null(adsfield(n))));
        assert!(!null(adsfield(23)) && !null(adsfield(24)));

        let mut value = 0;
        let rc = unsafe { AdsIsNull(table, adsfield(99), &mut value) };
        assert_eq!(rc, AE_INVALID_FIELDNUMBER);
        assert_eq!(crate::AdsGotoBOF(table), AE_SUCCESS);
        let rc = unsafe { AdsIsEmpty(table, adsfield(1), &mut value) };
        assert_eq!(rc, crate::AE_NO_CURRENT_RECORD);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn binary_lengths() {
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let blob = c"c_blob".as_ptr().cast_mut().cast();
        let mut length = 0xffff_ffff;
        assert_eq!(crate::AdsGotoRecord(table, 3), AE_SUCCESS);
        assert_eq!(
            unsafe { AdsGetBinaryLength(table, blob, &mut length) },
            AE_SUCCESS
        );
        assert_eq!(length, 0, "NULL");
        assert_eq!(crate::AdsGotoRecord(table, 1), AE_SUCCESS);
        assert_eq!(
            unsafe { AdsGetBinaryLength(table, blob, &mut length) },
            AE_SUCCESS
        );
        assert!(length > 0);
        let rc = unsafe { AdsGetBinaryLength(table, adsfield(15), &mut length) };
        assert_eq!(rc, AE_INVALID_FIELD_TYPE, "an Integer field");
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn binary_values_in_pieces() {
        let (connect, table) = fixtures::open("memo_compressed", c"compressed.adt");
        assert_eq!(crate::AdsGotoRecord(table, 2), AE_SUCCESS);
        let m = c"m".as_ptr().cast_mut().cast();
        let mut length = 0;
        assert_eq!(
            unsafe { AdsGetBinaryLength(table, m, &mut length) },
            AE_SUCCESS
        );

        // Read in pieces of 7 bytes and put them together.
        let mut value = Vec::new();
        while (value.len() as u32) < length {
            let mut piece = [0u8; 7];
            let mut len = piece.len() as u32;
            let offset = value.len() as u32;
            let rc = unsafe { AdsGetBinary(table, m, offset, piece.as_mut_ptr(), &mut len) };
            assert_eq!(rc, AE_SUCCESS);
            value.extend_from_slice(&piece[..len as usize]);
        }
        assert_eq!(value.len() as u32, length);
        assert!(value.starts_with(b"compressible text"));

        let mut len = 7;
        let mut piece = [0u8; 7];
        let rc = unsafe { AdsGetBinary(table, m, length + 5, piece.as_mut_ptr(), &mut len) };
        assert_eq!(
            (rc, len),
            (crate::AE_INVALID_BINARY_OFFSET, 7),
            "past the end"
        );
        let mut none = 0;
        let rc = unsafe { AdsGetBinary(table, m, 0, piece.as_mut_ptr(), &mut none) };
        assert_eq!(rc, crate::AE_INSUFFICIENT_BUFFER, "no room");
        let id = c"id".as_ptr().cast_mut().cast();
        let rc = unsafe { AdsGetBinary(table, id, 0, piece.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INVALID_FIELD_TYPE);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn memo_data_types() {
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let data_type = |n| {
            let mut value = 0xffff;
            let rc = unsafe { AdsGetMemoDataType(table, adsfield(n), &mut value) };
            (rc, value)
        };
        assert_eq!(data_type(6), (AE_SUCCESS, 5), "c_memo: ADS_MEMO");
        assert_eq!(data_type(7), (AE_SUCCESS, 28), "c_nmemo: ADS_NMEMO");
        assert_eq!(data_type(8), (AE_SUCCESS, 6), "c_blob: ADS_BINARY");
        assert_eq!(data_type(1).0, AE_INVALID_FIELD_TYPE, "c_char");
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn data_lengths() {
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        assert_eq!(crate::AdsGotoRecord(table, 1), AE_SUCCESS);
        let length = |n, options| {
            let mut value = 0xffff_ffff;
            let rc = unsafe { AdsGetDataLength(table, adsfield(n), options, &mut value) };
            (rc, value)
        };
        // c_nchar (field 4): NChar(5).
        assert_eq!(length(4, ADS_CODEUNIT_LENGTH), (AE_SUCCESS, 5));
        assert_eq!(length(4, ADS_BYTE_LENGTH), (AE_SUCCESS, 10));
        assert_eq!(length(4, ADS_BYTE_LENGTH_IN_BUFFER), (AE_SUCCESS, 10));
        // c_memo (field 6): its value's length, or the 9-byte reference.
        let mut memo = 0;
        assert_eq!(
            unsafe { AdsGetBinaryLength(table, adsfield(6), &mut memo) },
            AE_SUCCESS
        );
        assert_eq!(length(6, ADS_CODEUNIT_LENGTH), (AE_SUCCESS, memo));
        assert_eq!(length(6, ADS_BYTE_LENGTH_IN_BUFFER), (AE_SUCCESS, 9));
        assert_eq!(length(4, 7).0, AE_INVALID_OPTION);
        assert_eq!(length(99, ADS_CODEUNIT_LENGTH).0, AE_INVALID_FIELDNUMBER);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn field_texts() {
        let _settings = fixtures::settings_lock();
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let text = |name: &CStr, option| {
            let mut buf = [0u8; 64];
            let mut len = buf.len() as u32;
            let field = name.as_ptr().cast_mut().cast();
            let rc = unsafe { AdsGetField(table, field, buf.as_mut_ptr(), &mut len, option) };
            assert_eq!(rc, AE_SUCCESS, "{name:?}");
            String::from_utf8(buf[..len as usize].to_vec()).unwrap()
        };
        assert_eq!(crate::AdsGotoRecord(table, 1), AE_SUCCESS);
        assert_eq!(text(c"c_char", ADS_NONE), "abc       ");
        assert_eq!(text(c"c_char", ADS_RTRIM), "abc");
        assert_eq!(text(c"c_numeric", ADS_NONE), "  12345.67");
        assert_eq!(text(c"c_numeric", ADS_LTRIM), "12345.67");
        assert_eq!(text(c"c_integer", ADS_NONE), "123456789");
        assert_eq!(text(c"c_money", ADS_NONE), "12.3456");
        assert_eq!(text(c"c_logical", ADS_NONE), "T");
        assert_eq!(text(c"c_memo", ADS_NONE), "memo text");
        assert_eq!(
            text(c"c_guid", ADS_NONE),
            "01234567-89ab-cdef-0123-456789abcdef"
        );
        assert_eq!(text(c"c_nvarchar", ADS_GET_UTF8), "Ω✓");

        // Dates follow AdsSetDateFormat (ACE's default first).
        assert_eq!(text(c"c_date", ADS_NONE), "11/08/2017");
        let format = c"dd.mm.yyyy".as_ptr().cast_mut().cast();
        assert_eq!(unsafe { crate::AdsSetDateFormat(format) }, AE_SUCCESS);
        assert_eq!(text(c"c_date", ADS_NONE), "08.11.2017");
        assert_eq!(text(c"c_timestamp", ADS_NONE), "08.11.2017 01:45:30 PM");
        // Formats the original engine refuses leave the format alone: ARC's
        // Windows-style one, and one longer than 11 characters.
        let format = c"M/d/yyyy".as_ptr().cast_mut().cast();
        let rc = unsafe { crate::AdsSetDateFormat(format) };
        assert_eq!(rc, crate::AE_INVALID_DATEFORMAT);
        let format = c"DD. MM. YYYY".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { crate::AdsSetDateFormat(format) },
            crate::AE_DATA_TOO_LONG
        );
        assert_eq!(text(c"c_date", ADS_NONE), "08.11.2017");
        assert_eq!(
            unsafe { crate::AdsSetDateFormat(c"MM/DD/CCYY".as_ptr().cast_mut().cast()) },
            crate::AE_SUCCESS
        );

        assert_eq!(crate::AdsGotoRecord(table, 3), AE_SUCCESS);
        assert_eq!(text(c"c_integer", ADS_NONE), "0", "NULL");
        assert_eq!(text(c"c_char", ADS_NONE), " ".repeat(10), "NULL");
        assert_eq!(text(c"c_date", ADS_NONE), "  /  /    ", "NULL");

        let mut small = [0u8; 4];
        let mut len = small.len() as u32;
        let field = c"c_char".as_ptr().cast_mut().cast();
        assert_eq!(crate::AdsGotoRecord(table, 1), AE_SUCCESS);
        let rc = unsafe { AdsGetField(table, field, small.as_mut_ptr(), &mut len, ADS_NONE) };
        assert_eq!(
            (rc, len),
            (AE_INSUFFICIENT_BUFFER, 11),
            "the length with the NUL"
        );
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    /// `AdsGetFieldW` with a buffer of `size` units, from a buffer full of
    /// 0xEEEE: the result, `*len`, and the units written up to `size + 1`.
    fn field_w(table: AdsHandle, name: &CStr, size: u32, option: u16) -> (u32, u32, Vec<u16>) {
        let mut buf = [0xEEEEu16; 64];
        let mut len = size;
        let field = name.as_ptr().cast_mut().cast();
        let rc = unsafe { AdsGetFieldW(table, field, buf.as_mut_ptr(), &mut len, option) };
        (rc, len, buf[..size as usize + 1].to_vec())
    }

    /// As the original engine answers.
    #[test]
    fn wide_field_texts() {
        let _settings = fixtures::settings_lock();
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let units = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
        let with = |text: &str, rest: &[u16]| [units(text), rest.to_vec()].concat();
        assert_eq!(crate::AdsGotoRecord(table, 1), AE_SUCCESS);
        // Text, with a NUL after it; Unicode fields keep every character.
        let (rc, len, buf) = field_w(table, c"c_char", 11, ADS_NONE);
        assert_eq!(
            (rc, len, buf),
            (AE_SUCCESS, 10, with("abc       ", &[0, 0xEEEE]))
        );
        let (rc, len, buf) = field_w(table, c"c_nvarchar", 3, ADS_NONE);
        assert_eq!(
            (rc, len, buf),
            (AE_SUCCESS, 2, vec![0x03A9, 0x2713, 0, 0xEEEE])
        );
        let (rc, len, buf) = field_w(table, c"c_nmemo", 20, ADS_NONE);
        assert_eq!((rc, len), (AE_SUCCESS, 15));
        assert_eq!(buf[..16], with("nmemo ünïcødé ✓", &[0]));
        // Too small: what fits, without a NUL, and the length with the NUL.
        let (rc, len, buf) = field_w(table, c"c_char", 3, ADS_NONE);
        assert_eq!(
            (rc, len, buf),
            (AE_INSUFFICIENT_BUFFER, 11, with("abc", &[0xEEEE]))
        );
        let (rc, len, buf) = field_w(table, c"c_char", 10, ADS_NONE);
        assert_eq!(
            (rc, len, buf),
            (AE_INSUFFICIENT_BUFFER, 11, with("abc       ", &[0xEEEE]))
        );
        let (rc, len, buf) = field_w(table, c"c_nchar", 3, ADS_NONE);
        assert_eq!(
            (rc, len, buf),
            (AE_INSUFFICIENT_BUFFER, 6, vec![0xE4, 0x20AC, 0x20, 0xEEEE])
        );
        // Options.
        let (rc, len, buf) = field_w(table, c"c_char", 4, ADS_RTRIM);
        assert_eq!((rc, len, buf), (AE_SUCCESS, 3, with("abc", &[0, 0xEEEE])));
        assert_eq!(
            field_w(table, c"c_char", 20, ADS_GET_UTF8).0,
            AE_INVALID_OPTION
        );
        // Binary values are their bytes, in half as many units, with no NUL.
        let (rc, len, buf) = field_w(table, c"c_raw", 3, ADS_NONE);
        assert_eq!(
            (rc, len, buf),
            (AE_SUCCESS, 2, vec![0xADDE, 0xEFBE, 0xEEEE, 0xEEEE])
        );
        let (rc, len, buf) = field_w(table, c"c_raw", 1, ADS_NONE);
        assert_eq!(
            (rc, len, buf),
            (AE_INSUFFICIENT_BUFFER, 2, vec![0xADDE, 0xEEEE])
        );
        let (rc, len, buf) = field_w(table, c"c_blob", 3, ADS_NONE);
        assert_eq!((rc, len), (AE_SUCCESS, 1), "3 bytes");
        assert_eq!(buf[..2], [0xFF00, 0xEE10]);
        // NULL binary values write nothing.
        assert_eq!(crate::AdsGotoRecord(table, 2), AE_SUCCESS);
        let (rc, len, buf) = field_w(table, c"c_blob", 3, ADS_NONE);
        assert_eq!((rc, len, buf), (AE_SUCCESS, 0, vec![0xEEEE; 4]));
        // Errors.
        assert_eq!(
            field_w(table, c"nope", 20, ADS_NONE).0,
            crate::AE_INVALID_FIELDNAME
        );
        let null = std::ptr::null_mut();
        let mut len = 20;
        let field = c"c_char".as_ptr().cast_mut().cast();
        let rc = unsafe { AdsGetFieldW(table, field, null, &mut len, ADS_NONE) };
        assert_eq!((rc, len), (crate::AE_INVALID_POINTER, 20));
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    remote_twins!(
        wide_field_texts,
        binary_lengths,
        binary_values_in_pieces,
        data_lengths,
        field_texts,
        memo_data_types,
        nulls,
        getters_match_the_original_engine,
        binary_values_to_files,
        dbf_tables_match_the_original_engine
    );
}
