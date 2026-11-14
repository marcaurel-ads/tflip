//! Setting field values: `AdsSetString`, `AdsSetLong`, `AdsSetField`, ...
//! On a table, or a live SQL cursor, they change the current record, which
//! is written with `AdsWriteRecord` or when the table moves (see
//! `record.rs`); on a static cursor they are refused; on a statement
//! handle they set the parameters of its SQL, by number (`ADSFIELD(n)`) or
//! name (without the `:`). As in the original engine, a null buffer with a
//! length of 0 is an empty value (Delphi passes an empty string as a null
//! pointer).

use tflip_engine::getters::{Charset, FieldId, charset_in};
use tflip_engine::handle::HandleKind;
use tflip_engine::setters::SetValue;
use tflip_engine::statement::{ParamRef, ParamValue};

use crate::backend::{Backend, api};
use crate::ffi::{path_arg, wide_path_arg};
use crate::field::FieldArg;
use crate::last_error::{add_detail, fail, null_param};
use crate::pieces;
use crate::{
    ADS_BINARY, ADS_IMAGE, AE_DATA_TOO_LONG, AE_DATA_TRUNCATED, AE_INVALID_COLUMN_NAME,
    AE_INVALID_COLUMN_NUMBER, AE_INVALID_DATA, AE_INVALID_DATA_TYPE, AE_INVALID_DAY,
    AE_INVALID_FIELD_TYPE, AE_INVALID_HANDLE, AE_INVALID_MONTH, AE_INVALID_SQL_PARAM_NAME,
    AE_INVALID_TABLE_HANDLE, AE_INVALID_TABLETYPE, AE_INVALID_VALUE, AE_NOT_VFP_NULLABLE_FIELD,
    AE_SUCCESS, AE_UNICODE_CONVERSION, AE_VALUE_OVERFLOW, AdsHandle, Unsigned8, Unsigned16,
    Unsigned32, Wchar, ansi, code, error_code,
};

/// A value set on a statement, as its parameter takes it.
fn param_value(value: SetValue) -> ParamValue {
    match value {
        SetValue::String(text) | SetValue::Field(text) => ParamValue::Text(text),
        SetValue::Time(text) | SetValue::TimeStamp(text) => ParamValue::Text(text),
        SetValue::Short(n) => ParamValue::Integer(i64::from(n)),
        SetValue::Long(n) => ParamValue::Integer(i64::from(n)),
        SetValue::LongLong(n) => ParamValue::Integer(n),
        SetValue::Milliseconds(ms) => ParamValue::Integer(i64::from(ms)),
        SetValue::Double(d) => ParamValue::Double(d),
        SetValue::Logical(b) => ParamValue::Logical(b),
        SetValue::Date(text) => ParamValue::DateText(text),
        SetValue::Julian(day) => ParamValue::Date(day),
        SetValue::TimeStampRaw(day, ms) => ParamValue::TimeStamp(day, ms),
        SetValue::Money(m) => ParamValue::Money(m),
        SetValue::Binary(bytes) | SetValue::Image(bytes) | SetValue::Raw(bytes) => {
            ParamValue::Binary(bytes)
        }
        SetValue::Null => ParamValue::Null,
        SetValue::Empty => ParamValue::Empty,
    }
}

/// Sets `field` of the current record of the table `handle`, or the
/// parameter `field` of the statement `handle`, to `value`; `unicode` is
/// the text of a `…W` setter as the client gave it.
pub(crate) fn set(
    name: &str,
    handle: AdsHandle,
    field: FieldArg,
    value: SetValue,
    unicode: Option<&str>,
) -> Unsigned32 {
    let backend = api(handle);
    match backend.handle_kind(handle) {
        Ok(Some(HandleKind::Statement)) => {
            // A parameter has no field to set by its bytes or as its type
            // (`fixtures/errors/param_n`).
            if matches!(name, "AdsSetField" | "AdsSetFieldW" | "AdsSetFieldRaw") {
                if name == "AdsSetFieldRaw" {
                    // Fails without an error to tell of it.
                    fail(AE_SUCCESS);
                    return AE_INVALID_HANDLE;
                }
                return on_connection(name);
            }
        }
        Ok(Some(HandleKind::Table | HandleKind::Cursor)) => {
            let id = field.clone().id();
            let kind = SetKind::of(&value);
            if let SetValue::Raw(bytes) = &value
                && let Some(rc) = raw_refused(handle, &field, bytes.len())
            {
                return rc;
            }
            let value = match oem_value(&backend, handle, &id, value, unicode) {
                Ok(value) => value,
                Err(code) => return set_failure(handle, &field, kind, fail(code)),
            };
            return match backend.set_field(handle, id, value) {
                Ok(()) => AE_SUCCESS,
                // A cursor names its fields columns.
                Err(
                    error @ tflip_engine::Error::Constraint {
                        code: AE_INVALID_COLUMN_NAME | AE_INVALID_COLUMN_NUMBER,
                        ..
                    },
                ) => error_code(error),
                Err(error) => {
                    let kind = match error {
                        tflip_engine::Error::InvalidDay(days) => SetKind::Day(days),
                        _ => kind,
                    };
                    let code = error_code(error);
                    set_failure(handle, &field, kind, code)
                }
            };
        }
        Ok(Some(HandleKind::Index)) => {
            // The record an order's raw key is built from (`AdsInitRawKey`).
            return code(backend.set_key_field(handle, field.id(), value));
        }
        Ok(Some(HandleKind::Connection)) => return on_connection(name),
        _ => return error_code(tflip_engine::Error::InvalidStatement),
    }
    let value = param_value(value);
    let param = match field {
        FieldArg::Number(n) => ParamRef::Number(n),
        FieldArg::Name(name) => {
            if name.starts_with(':') {
                return fail(AE_INVALID_SQL_PARAM_NAME);
            }
            ParamRef::Name(name)
        }
    };
    let name = match &param {
        ParamRef::Name(name) => Some(name.clone()),
        ParamRef::Number(_) => None,
    };
    let rc = code(backend.set_param(handle, param, value));
    // The original names the parameter it has none of
    // (`fixtures/sql/params3.json`).
    match name {
        Some(name) if rc == AE_INVALID_SQL_PARAM_NAME => add_detail(rc, &name),
        _ => rc,
    }
}

/// Whether `handle` is a statement, whose parameters the setters set.
fn is_statement(handle: AdsHandle) -> bool {
    matches!(
        api(handle).handle_kind(handle),
        Ok(Some(HandleKind::Statement))
    )
}

/// The failure of the setter `name` on a connection's handle, which differs
/// by setter in the original engine (`fixtures/write/database_type`).
fn on_connection(name: &str) -> Unsigned32 {
    use crate::last_error::fail_detail;
    match name {
        "AdsSetBinary" | "AdsFileToBinary" => {
            fail_detail(AE_INVALID_TABLE_HANDLE, crate::last_error::NOT_A_TABLE)
        }
        "AdsSetField" | "AdsSetFieldW" | "AdsSetFieldRaw" | "AdsSetTimeStamp"
        | "AdsSetTimeStampRaw" | "AdsSetNull" | "AdsSetEmpty" => fail_detail(
            AE_INVALID_HANDLE,
            "Must be a table, cursor or index order handle.",
        ),
        _ => fail(AE_INVALID_HANDLE),
    }
}

/// What a setter set, as far as the details of its failure tell it apart.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SetKind {
    /// `AdsSetString(W)`.
    String,
    /// `AdsSetField(W)`.
    Field,
    Date,
    Time,
    Binary,
    Image,
    /// `AdsSetFieldRaw` and `AdsSetTimeStampRaw` on a table.
    Raw,
    /// A date whose day its month (of this many days) does not have.
    Day(u32),
    Other,
}

impl SetKind {
    fn of(value: &SetValue) -> SetKind {
        match value {
            SetValue::String(_) => SetKind::String,
            SetValue::Field(_) => SetKind::Field,
            SetValue::Date(_) => SetKind::Date,
            SetValue::Time(_) => SetKind::Time,
            SetValue::Binary(_) => SetKind::Binary,
            SetValue::Image(_) => SetKind::Image,
            SetValue::Raw(_) => SetKind::Raw,
            _ => SetKind::Other,
        }
    }
}

/// The result of a raw setter on `field` of the table `handle` that the
/// field's type decides before any byte is set: `Some` when it is refused
/// or the engine ignores it (`fixtures/errors/param_n`).
fn raw_refused(handle: AdsHandle, field: &FieldArg, len: usize) -> Option<Unsigned32> {
    use tflip_storage::field::FieldType::*;
    let (name, ty) =
        crate::field::with_field(handle, field.clone(), |f| (f.name.clone(), f.field_type)).ok()?;
    let reason = match ty {
        // The engine fills these in itself.
        AutoInc | RowVersion => return Some(AE_SUCCESS),
        _ if ty.is_memo() => {
            return Some(add_detail(
                fail(AE_INVALID_FIELD_TYPE),
                &format!("Data type not supported by this function. Field: \"{name}\""),
            ));
        }
        Guid if len != 16 => "Data length is not a valid length of GUID. ",
        _ => return None,
    };
    Some(add_detail(
        fail(AE_INVALID_DATA_TYPE),
        &format!("{reason}Field: \"{name}\""),
    ))
}

/// `code`, the failure of a setter of `kind` on `field` of the table
/// `handle`, with the details the original engine adds to its text
/// (`fixtures/write/*setters*`, `dbf_memos`, ...): mostly the field's name
/// (`Field: "C"`, as the table has it), after a reason for some codes.
fn set_failure(handle: AdsHandle, field: &FieldArg, kind: SetKind, code: Unsigned32) -> Unsigned32 {
    use tflip_storage::field::FieldType::*;
    let Ok((name, ty)) =
        crate::field::with_field(handle, field.clone(), |f| (f.name.clone(), f.field_type))
    else {
        return code;
    };
    let reason = match code {
        AE_INVALID_FIELD_TYPE => match kind {
            SetKind::Date => "Expected date field. ",
            SetKind::Time => "Expected time field. ",
            SetKind::Binary => "Expected binary field type. ",
            SetKind::Image if ty != Binary => "Expected binary field type. ",
            _ => "",
        },
        AE_DATA_TRUNCATED => match ty {
            _ if kind == SetKind::Raw => "",
            Raw => "",
            VarBinary | Binary | Image if kind != SetKind::String => "",
            _ => "Character value too long. ",
        },
        AE_INVALID_DATA_TYPE => match ty {
            Logical => "Not a valid logical value. ",
            Money => "The specified value is not a valid numeric expression. ",
            _ => "Expected a numeric value. ",
        },
        AE_INVALID_DAY => {
            let SetKind::Day(days) = kind else {
                return code;
            };
            return add_detail(
                code,
                &format!("Must be from 1 to {days}. Field: \"{name}\""),
            );
        }
        AE_UNICODE_CONVERSION => {
            return add_detail(
                code,
                "Some Unicode characters cannot be converted to code page characters.",
            );
        }
        AE_INVALID_TABLETYPE => {
            return add_detail(code, "ADS_MONEY is available only with ADT tables.");
        }
        AE_VALUE_OVERFLOW if ty == LongInt && kind == SetKind::Field => {
            "The value is out of the range of the field type. "
        }
        AE_VALUE_OVERFLOW
        | AE_INVALID_MONTH
        | AE_INVALID_VALUE
        | AE_NOT_VFP_NULLABLE_FIELD
        | AE_INVALID_DATA => "",
        _ => return code,
    };
    add_detail(code, &format!("{reason}Field: \"{name}\""))
}

/// `value` as a field of the table `handle` takes it: text for the 8-bit
/// text of an OEM table (a Clipper or FoxPro table opened with `ADS_OEM`)
/// in the client's OEM code page, as the original engine stores it (ANSI
/// text best fit; Unicode text, `unicode`, exactly, or
/// `AE_UNICODE_CONVERSION`); anything else as it is.
fn oem_value(
    backend: &Backend,
    handle: AdsHandle,
    field: &FieldId,
    value: SetValue,
    unicode: Option<&str>,
) -> Result<SetValue, Unsigned32> {
    use tflip_storage::field::FieldType::*;
    let (SetValue::String(text) | SetValue::Field(text) | SetValue::TimeStamp(text)) = &value
    else {
        return Ok(value);
    };
    let Ok(info) = backend.table_info(handle) else {
        return Ok(value);
    };
    let fields = &info.structure.fields[..];
    let Ok((_, found)) = tflip_engine::table::find_field(fields, field.as_ref()) else {
        return Ok(value);
    };
    // A Unicode field keeps the text as given, not as the client's code page
    // has it.
    if let (Some(unicode), NChar | NVarChar | NMemo) = (unicode, found.field_type) {
        return Ok(match value {
            SetValue::String(_) => SetValue::String(unicode.to_owned()),
            SetValue::Field(_) => SetValue::Field(unicode.to_owned()),
            _ => SetValue::TimeStamp(unicode.to_owned()),
        });
    }
    let text_field = matches!(
        found.field_type,
        Character | CiCharacter | Memo | LegacyVarChar
    );
    if !text_field || charset_in(&info.settings, &info.structure, found) != Charset::Oem {
        return Ok(value);
    }
    let converted = match unicode {
        Some(unicode) => ansi::unicode_to_oem(unicode).ok_or(AE_UNICODE_CONVERSION)?,
        None => ansi::ansi_to_oem(text),
    };
    Ok(match value {
        SetValue::String(_) => SetValue::String(converted),
        SetValue::Field(_) => SetValue::Field(converted),
        _ => SetValue::TimeStamp(converted),
    })
}

/// The `len` items at `buf`: none for a null `buf` of length 0, `None` for
/// a null `buf` of any other length.
///
/// # Safety
///
/// `buf` must be null or hold `len` items.
unsafe fn slice<'a, T>(buf: *const T, len: usize) -> Option<&'a [T]> {
    match len {
        0 if buf.is_null() => Some(&[]),
        // SAFETY: as the caller promises.
        _ => unsafe { crate::ffi::slice(buf, len) },
    }
}

/// `items` up to the first 0, if any.
fn until_nul<T: Copy + Default + PartialEq>(items: &[T]) -> &[T] {
    let nul = T::default();
    &items[..items.iter().position(|&b| b == nul).unwrap_or(items.len())]
}

/// The text of the `len` UTF-16 units at `buf` up to a NUL; `None` for null.
///
/// # Safety
///
/// `buf` must be null or hold `len` units.
unsafe fn wide_arg(buf: *const Wchar, len: Unsigned32) -> Option<String> {
    // SAFETY: as the caller promises.
    unsafe { slice(buf, len as usize) }.map(|units| String::from_utf16_lossy(until_nul(units)))
}

/// The text of `bytes` up to a NUL, as the client's ANSI bytes.
fn text_arg(bytes: &[u8]) -> String {
    ansi::data_in(until_nul(bytes))
}

/// The text of all of `bytes`, NULs and what follows them too, as the
/// client's ANSI bytes: what `AdsSetString` and `AdsSetField` store, as
/// the original does (`fixtures/fields/nul_strings.json`).
fn data_arg(bytes: &[u8]) -> String {
    ansi::data_in(bytes)
}

/// `AdsSetString`: a field's or parameter's value as text, `len` bytes of `buf`.
///
/// # Safety
///
/// `field` must be a number below 0x10000 or point to a NUL-terminated
/// string; `buf` must be null or hold `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetString(
    obj: AdsHandle,
    field: *mut Unsigned8,
    buf: *mut Unsigned8,
    len: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetString");
    // SAFETY: `field` and `buf` are as # Safety says.
    let (field, bytes) = unsafe { (FieldArg::new(field), slice(buf, len as usize)) };
    let Some(bytes) = bytes else {
        return null_param(3);
    };
    set(
        "AdsSetString",
        obj,
        field,
        SetValue::String(data_arg(bytes)),
        None,
    )
}

/// `AdsSetStringW`: [`AdsSetString`] with `len` UTF-16 units.
///
/// # Safety
///
/// As for [`AdsSetString`], with `buf` holding `len` units.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetStringW(
    obj: AdsHandle,
    field: *mut Unsigned8,
    buf: *mut Wchar,
    len: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetStringW");
    // SAFETY: `field` and `buf` are as # Safety says.
    let (field, unicode) = unsafe { (FieldArg::new(field), wide_arg(buf, len)) };
    let Some(unicode) = unicode else {
        return null_param(3);
    };
    let value = SetValue::String(crate::ansi::data_from_unicode(&unicode));
    set("AdsSetStringW", obj, field, value, Some(&unicode))
}

/// `AdsSetLong`: a field's or parameter's value as an integer.
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetLong(
    obj: AdsHandle,
    field: *mut Unsigned8,
    value: i32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetLong");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    set("AdsSetLong", obj, field, SetValue::Long(value), None)
}

/// `AdsSetShort`: a field's or parameter's value as an integer.
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetShort(
    obj: AdsHandle,
    field: *mut Unsigned8,
    value: i16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetShort");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    set("AdsSetShort", obj, field, SetValue::Short(value), None)
}

/// `AdsSetDouble`: a field's or parameter's value as a double.
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetDouble(
    obj: AdsHandle,
    field: *mut Unsigned8,
    value: f64,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetDouble");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    set("AdsSetDouble", obj, field, SetValue::Double(value), None)
}

/// `AdsSetLogical`: a field's or parameter's value as a logical.
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetLogical(
    obj: AdsHandle,
    field: *mut Unsigned8,
    value: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetLogical");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    set(
        "AdsSetLogical",
        obj,
        field,
        SetValue::Logical(value != 0),
        None,
    )
}

/// `AdsSetDate`: a field's or parameter's value as a date, `len` bytes of text in the
/// date format.
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetDate(
    obj: AdsHandle,
    field: *mut Unsigned8,
    value: *mut Unsigned8,
    len: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetDate");
    // SAFETY: `field` and `value` are as # Safety says.
    let (field, bytes) = unsafe { (FieldArg::new(field), slice(value, usize::from(len))) };
    let bytes = match bytes {
        Some(bytes) => bytes,
        // A parameter takes no text as an empty one.
        None if is_statement(obj) => &[],
        None => return null_param(3),
    };
    set(
        "AdsSetDate",
        obj,
        field,
        SetValue::Date(text_arg(bytes)),
        None,
    )
}

/// `AdsSetMoney`: a field's or parameter's value as money, × 10,000.
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetMoney(
    obj: AdsHandle,
    field: *mut Unsigned8,
    value: i64,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetMoney");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    set("AdsSetMoney", obj, field, SetValue::Money(value), None)
}

/// `AdsSetNull`: a field's or parameter's value as NULL.
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetNull(obj: AdsHandle, field: *mut Unsigned8) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetNull");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    set("AdsSetNull", obj, field, SetValue::Null, None)
}

/// `AdsSetEmpty`: a field's or parameter's value as empty, which SQL takes as NULL.
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetEmpty(obj: AdsHandle, field: *mut Unsigned8) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetEmpty");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    set("AdsSetEmpty", obj, field, SetValue::Empty, None)
}

/// `AdsSetBinary`: a field's or parameter's value as bytes: `len` bytes of `buf` at
/// `offset` of a value of `total` bytes. A Binary, Image or VarBinary field
/// takes a long value in pieces; see [`pieces`].
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetBinary(
    obj: AdsHandle,
    field: *mut Unsigned8,
    binary_type: Unsigned16,
    total: Unsigned32,
    offset: Unsigned32,
    buf: *mut Unsigned8,
    len: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetBinary");
    // SAFETY: `field` and `buf` are as # Safety says.
    let (field, bytes) = unsafe { (FieldArg::new(field), slice(buf, len as usize)) };
    let Some(bytes) = bytes else {
        return null_param(6);
    };
    if is_statement(obj) && !matches!(binary_type, ADS_BINARY | ADS_IMAGE) {
        return crate::last_error::fail_detail(AE_INVALID_DATA_TYPE, "Parameter 3");
    }
    let bytes = bytes.to_vec();
    let piece = pieces::Piece {
        binary_type,
        total,
        offset,
    };
    if let Some(rc) = pieces::set_binary(obj, &field, piece, &bytes) {
        return rc;
    }
    let value = match binary_type {
        ADS_IMAGE => SetValue::Image(bytes),
        _ => SetValue::Binary(bytes),
    };
    set("AdsSetBinary", obj, field, value, None)
}

/// `AdsSetTimeStampRaw`: a parameter's value as a timestamp, 8 bytes: a
/// Julian day and milliseconds; fewer bytes fail with `AE_DATA_TOO_LONG`, as
/// in the original engine. A table's field takes the bytes as
/// [`AdsSetFieldRaw`] does. Not declared in the SDK's `ace.h`; the
/// signature is `UNSIGNED32 AdsSetTimeStampRaw( ADSHANDLE hObj, UNSIGNED8
/// *pucFldName, UNSIGNED8 *pucBuf, UNSIGNED32 ulLen )` (`aceunpub.pas`).
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetTimeStampRaw(
    obj: AdsHandle,
    field: *mut Unsigned8,
    buf: *mut Unsigned8,
    len: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetTimeStampRaw");
    if buf.is_null() && len != 0 {
        return null_param(3);
    }
    if !is_statement(obj) {
        // A table takes the bytes as `AdsSetFieldRaw` does, whatever the
        // field's type and the length (`fixtures/errors/param_n`).
        // SAFETY: `field` is as # Safety says, and `buf` is null or holds
        // `len` bytes.
        let (field, bytes) = unsafe { (FieldArg::new(field), slice(buf, len as usize)) };
        let Some(bytes) = bytes else {
            return null_param(3);
        };
        return set(
            "AdsSetTimeStampRaw",
            obj,
            field,
            SetValue::Raw(bytes.to_vec()),
            None,
        );
    }
    if len < 8 {
        // A parameter names the length it was given
        // (`fixtures/errors/param_n`).
        return match is_statement(obj) {
            true => crate::last_error::fail_detail(AE_DATA_TOO_LONG, "Parameter 4"),
            false => fail(AE_DATA_TOO_LONG),
        };
    }
    // SAFETY: `field` is as # Safety says, and `buf` is null or holds `len`
    // bytes, at least 8.
    let (field, bytes) = unsafe { (FieldArg::new(field), crate::ffi::slice(buf, 8)) };
    let Some(bytes) = bytes else {
        return null_param(3);
    };
    let day = i32::from_le_bytes(bytes[..4].try_into().unwrap());
    let ms = u32::from_le_bytes(bytes[4..].try_into().unwrap());
    set(
        "AdsSetTimeStampRaw",
        obj,
        field,
        SetValue::TimeStampRaw(day, ms),
        None,
    )
}

/// `AdsSetLongLong`: a field's or parameter's value as a 64-bit integer.
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetLongLong(
    obj: AdsHandle,
    field: *mut Unsigned8,
    value: i64,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetLongLong");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    set(
        "AdsSetLongLong",
        obj,
        field,
        SetValue::LongLong(value),
        None,
    )
}

/// `AdsSetJulian`: a field's or parameter's value as a Julian day.
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetJulian(
    obj: AdsHandle,
    field: *mut Unsigned8,
    value: i32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetJulian");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    set("AdsSetJulian", obj, field, SetValue::Julian(value), None)
}

/// `AdsSetMilliseconds`: a field's or parameter's value as a time of day,
/// in milliseconds.
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetMilliseconds(
    obj: AdsHandle,
    field: *mut Unsigned8,
    value: i32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetMilliseconds");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    set(
        "AdsSetMilliseconds",
        obj,
        field,
        SetValue::Milliseconds(value),
        None,
    )
}

/// `AdsSetTime`: a field's or parameter's value as a time of day, `len`
/// bytes of text.
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetTime(
    obj: AdsHandle,
    field: *mut Unsigned8,
    value: *mut Unsigned8,
    len: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetTime");
    // SAFETY: `field` and `value` are as # Safety says.
    let (field, bytes) = unsafe { (FieldArg::new(field), slice(value, usize::from(len))) };
    let Some(bytes) = bytes else {
        return null_param(3);
    };
    set(
        "AdsSetTime",
        obj,
        field,
        SetValue::Time(text_arg(bytes)),
        None,
    )
}

/// `AdsSetTimeStamp`: a field's or parameter's value as a date in the date
/// format and a time of day, `len` bytes of text.
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetTimeStamp(
    obj: AdsHandle,
    field: *mut Unsigned8,
    value: *mut Unsigned8,
    len: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetTimeStamp");
    // SAFETY: `field` and `value` are as # Safety says.
    let (field, bytes) = unsafe { (FieldArg::new(field), slice(value, len as usize)) };
    let Some(bytes) = bytes else {
        return null_param(3);
    };
    set(
        "AdsSetTimeStamp",
        obj,
        field,
        SetValue::TimeStamp(text_arg(bytes)),
        None,
    )
}

/// `AdsSetField`: a field's or parameter's value as text, read as the
/// field's type is written (numbers, dates in the date format, ...).
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetField(
    obj: AdsHandle,
    field: *mut Unsigned8,
    buf: *mut Unsigned8,
    len: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetField");
    // SAFETY: `field` and `buf` are as # Safety says.
    let (field, bytes) = unsafe { (FieldArg::new(field), slice(buf, len as usize)) };
    let Some(bytes) = bytes else {
        return null_param(3);
    };
    set(
        "AdsSetField",
        obj,
        field,
        SetValue::Field(data_arg(bytes)),
        None,
    )
}

/// `AdsSetFieldRaw` (unpublished): a field's bytes as the record holds them,
/// the `len` bytes at `buf`, padded to the field (text with blanks; an ADT
/// field of another type keeps its old bytes after them), or cut to it, which
/// fails with `AE_DATA_TRUNCATED` after the cut value is set. Memo and
/// binary fields are refused, a GUID takes 16 bytes, and an AutoInc or
/// RowVersion field ignores them.
///
/// # Safety
///
/// As for [`AdsSetString`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetFieldRaw(
    obj: AdsHandle,
    field: *mut Unsigned8,
    buf: *mut Unsigned8,
    len: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetFieldRaw");
    // SAFETY: `field` and `buf` are as # Safety says.
    let (field, bytes) = unsafe { (FieldArg::new(field), slice(buf, len as usize)) };
    let Some(bytes) = bytes else {
        return null_param(3);
    };
    set(
        "AdsSetFieldRaw",
        obj,
        field,
        SetValue::Raw(bytes.to_vec()),
        None,
    )
}

/// The type of `field` of the table `handle`; `Err` with the code for a
/// handle that is no table or a field it does not have.
fn field_type(
    handle: AdsHandle,
    field: &FieldArg,
) -> Result<(tflip_storage::field::FieldType, String), Unsigned32> {
    let backend = crate::filter::table_of(handle)?;
    let info = backend.table_info(handle).map_err(error_code)?;
    let fields = &info.structure.fields[..];
    match tflip_engine::table::find_field(fields, field.as_ref()) {
        Ok((_, found)) => Ok((found.field_type, found.name.clone())),
        Err(error) => Err(error_code(error)),
    }
}

/// `AE_INVALID_FIELD_TYPE` for the field `name`, named as the original
/// engine names it.
fn wrong_field_type(name: &str) -> Unsigned32 {
    crate::last_error::fail_detail(crate::AE_INVALID_FIELD_TYPE, &format!("Field: \"{name}\""))
}

/// `AdsFileToBinary`: sets a Binary or Image field of the current record to
/// the bytes of the file `file` (ANSI), as `AdsSetBinary` with
/// `binary_type` does. As in the original engine, other fields fail with
/// `AE_INVALID_FIELD_TYPE`, and a file that cannot be read with
/// `AE_BINARY_FILE_ERROR`.
///
/// # Safety
///
/// `field` must be a number below 0x10000 or point to a NUL-terminated
/// string; `file` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsFileToBinary(
    table: AdsHandle,
    field: *mut Unsigned8,
    binary_type: Unsigned16,
    file: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsFileToBinary");
    // SAFETY: `field` and `file` are as # Safety says.
    let (field, path) = unsafe { (FieldArg::new(field), path_arg(file)) };
    file_to_binary(table, field, binary_type, path)
}

/// `AdsFileToBinaryW`: as [`AdsFileToBinary`], with a UTF-16 file name.
///
/// # Safety
///
/// As for [`AdsFileToBinary`], with `file` UTF-16.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsFileToBinaryW(
    table: AdsHandle,
    field: *mut Unsigned8,
    binary_type: Unsigned16,
    file: *mut Wchar,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsFileToBinaryW");
    // SAFETY: `field` is a field number or a string and `file` null or a
    // UTF-16 string, as # Safety says.
    let (field, path) = unsafe { (FieldArg::new(field), wide_path_arg(file)) };
    file_to_binary(table, field, binary_type, path)
}

/// See [`AdsFileToBinary`]; `path` is `None` for a null file name.
fn file_to_binary(
    table: AdsHandle,
    field: FieldArg,
    binary_type: Unsigned16,
    path: Option<std::path::PathBuf>,
) -> Unsigned32 {
    use tflip_storage::field::FieldType::{Binary, Image};
    // Unlike the check of the field's type, this replaces an error left
    // before.
    if api(table).handle_kind(table) == Ok(Some(HandleKind::Connection)) {
        return on_connection("AdsFileToBinary");
    }
    match field_type(table, &field) {
        Ok(_) if binary_type != crate::ADS_BINARY && binary_type != ADS_IMAGE => {
            return crate::last_error::fail_detail(AE_INVALID_DATA_TYPE, "Parameter 3");
        }
        Ok((Binary | Image, _)) => {}
        Ok((_, name)) => return wrong_field_type(&name),
        Err(code) => return code,
    }
    let Some(path) = path else {
        return null_param(4);
    };
    let Ok(bytes) = std::fs::read(path) else {
        return crate::last_error::fail_detail(
            crate::AE_BINARY_FILE_ERROR,
            "Unable to open the file for reading.",
        );
    };
    let value = match binary_type {
        ADS_IMAGE => SetValue::Image(bytes),
        _ => SetValue::Binary(bytes),
    };
    set("AdsFileToBinary", table, field, value, None)
}

/// `AdsSetStringFromCodePage`: a text field's value as `len` bytes of text
/// in the code page `code_page`, converted for the table, as `AdsSetString`
/// sets it. tflip knows the single-byte Windows and OEM code pages the
/// original engine does (see [`crate::codepages`]), converting them byte for
/// byte as it does, and the four double-byte pages (932/936/949/950) through
/// the engine's lead/trail tables (see [`crate::dbcs`]); a code page it has no
/// converter for fails with `AE_NO_CONVERTER` (1565), and 0 with
/// `AE_UNICODE_CONVERSION`, as in the original. A field that is not text fails
/// with `AE_INVALID_FIELD_TYPE`.
///
/// # Safety
///
/// `field` must be a number below 0x10000 or point to a NUL-terminated
/// string; `buf` must be null or hold `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetStringFromCodePage(
    obj: AdsHandle,
    code_page: Unsigned32,
    field: *mut Unsigned8,
    buf: *mut Unsigned8,
    len: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetStringFromCodePage");
    use tflip_storage::field::FieldType::*;
    // SAFETY: `field` and `buf` are as # Safety says.
    let (field, bytes) = unsafe { (FieldArg::new(field), slice(buf, len as usize)) };
    if !is_statement(obj) {
        match field_type(obj, &field) {
            Ok((
                Character | CiCharacter | VarChar | LegacyVarChar | NChar | NVarChar | Memo | NMemo,
                _,
            )) => {}
            Ok((_, name)) => return wrong_field_type(&name),
            Err(code) => return code,
        }
    }
    let Some(bytes) = bytes else {
        // Neither the code the original returns nor the error it leaves is
        // `AE_INVALID_POINTER` (`fixtures/errors/param_n`).
        crate::last_error::restore_pending();
        return 1502;
    };
    let unicode = match code_page {
        0 => {
            return crate::last_error::fail_detail(
                AE_UNICODE_CONVERSION,
                "Unknown code page. Ensure that the correct adscollate.adt is installed.",
            );
        }
        // The double-byte code pages the engine converts through its bundled
        // ICU (see [`crate::dbcs`]).
        932 | 936 | 949 | 950 => {
            let units = crate::dbcs::decode(code_page, bytes).expect("a double-byte page");
            String::from_utf16_lossy(&units)
        }
        _ => match crate::codepages::table(code_page) {
            Some(table) => {
                let units: Vec<u16> = bytes.iter().map(|&b| table[usize::from(b)]).collect();
                String::from_utf16_lossy(&units)
            }
            None => return fail(crate::AE_NO_CONVERTER),
        },
    };
    let text = ansi::data_from_unicode(&unicode);
    set(
        "AdsSetStringFromCodePage",
        obj,
        field,
        SetValue::String(text),
        Some(&unicode),
    )
}

/// `AdsSetFieldW`: [`AdsSetField`] with `len` UTF-16 units.
///
/// # Safety
///
/// As for [`AdsSetString`], with `buf` holding `len` units.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetFieldW(
    obj: AdsHandle,
    field: *mut Unsigned8,
    buf: *mut Wchar,
    len: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetFieldW");
    // SAFETY: `field` and `buf` are as # Safety says.
    let (field, unicode) = unsafe { (FieldArg::new(field), wide_arg(buf, len)) };
    let Some(unicode) = unicode else {
        return null_param(3);
    };
    let value = SetValue::Field(ansi::data_from_unicode(&unicode));
    set("AdsSetFieldW", obj, field, value, Some(&unicode))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{self, remote_twins};
    use crate::{AdsCloseSQLStatement, AdsCreateSQLStatement, AdsDisconnect, AdsPrepareSQL};

    /// As in the original engine: a null buffer is an empty value if its
    /// length is 0, else `AE_INVALID_POINTER`.
    #[test]
    fn null_buffers_of_length_0_are_empty() {
        let connect = fixtures::connect_sql();
        let mut stmt = 0;
        // `ADSFIELD(1)`.
        let one = std::ptr::without_provenance_mut::<Unsigned8>(1);
        let null = std::ptr::null_mut();
        unsafe {
            assert_eq!(AdsCreateSQLStatement(connect, &mut stmt), AE_SUCCESS);
            let sql = c"SELECT * FROM indexed WHERE name = :n"
                .as_ptr()
                .cast_mut()
                .cast();
            assert_eq!(AdsPrepareSQL(stmt, sql), AE_SUCCESS);
            assert_eq!(AdsSetString(stmt, one, null, 0), AE_SUCCESS);
            assert_eq!(AdsSetString(stmt, one, null, 5), crate::AE_INVALID_POINTER);
            assert_eq!(
                AdsSetStringW(stmt, one, std::ptr::null_mut(), 0),
                AE_SUCCESS
            );
            assert_eq!(AdsSetDate(stmt, one, null, 0), AE_SUCCESS);
            assert_eq!(AdsSetBinary(stmt, one, 6, 0, 0, null, 0), AE_SUCCESS);
            assert_eq!(AdsSetTimeStampRaw(stmt, one, null, 0), AE_DATA_TOO_LONG);
        }
        AdsCloseSQLStatement(stmt);
        AdsDisconnect(connect);
    }

    /// A setter on a SQL cursor: a live one (one table's columns) is the
    /// table, changed and written as a table is; a static one refuses with
    /// `AE_TABLE_READONLY` once the column is found, and leaves the table as
    /// it was (`fixtures/errors/cursor_set.json`, from
    /// `tools/ace/errors/cursor_set.c`).
    #[test]
    fn setters_on_cursors_match_the_original_engine() {
        use crate::{
            AdsExecuteSQLDirect, AdsGetField, AdsGetLastError, AdsGotoBottom, AdsGotoTop,
            AdsStmtSetTableReadOnly, AdsWriteRecord,
        };
        // Values read back as text follow the process-wide date format.
        let _settings = fixtures::settings_shared();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/errors/cursor_set.json");
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let sql = |cursor: &str| match cursor {
            "live_star" | "live_read_only" => "SELECT * FROM all_types",
            "live_columns" => {
                "SELECT c_char, c_integer, c_double, c_logical, c_date, c_blob, c_money, \
                 c_timestamp FROM all_types"
            }
            "static_join" => {
                "SELECT a.c_char, a.c_integer, a.c_double, a.c_logical, a.c_date, a.c_blob, \
                 a.c_money, a.c_timestamp FROM all_types a JOIN all_types b ON \
                 a.c_integer = b.c_integer"
            }
            _ => {
                "SELECT c_char, c_integer, COUNT(*) AS n FROM all_types GROUP BY c_char, c_integer"
            }
        };
        let original = fixtures::dir("all_types");
        let mut problems = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let text = |key: &str| row[key].as_str().unwrap();
            let (cursor, name, field, how) =
                (text("cursor"), text("fn"), text("field"), text("how"));
            // A folder of its own for each row: in the full parallel run, a
            // table an earlier row closed was seen to stay open in the
            // process (what held it was not found), and the fresh copy at its
            // path then opened as that stale file, which this row's writes
            // went to instead.
            let (source, dir) =
                fixtures::scratch_from(&format!("setters_on_cursors_{index}"), &original);
            let connect = fixtures::connect_to(&source);
            let mut stmt = 0;
            assert_eq!(
                unsafe { AdsCreateSQLStatement(connect, &mut stmt) },
                AE_SUCCESS
            );
            if cursor == "live_read_only" {
                assert_eq!(AdsStmtSetTableReadOnly(stmt, 1), AE_SUCCESS);
            }
            let mut sql_text = format!("{}\0", sql(cursor)).into_bytes();
            let mut handle = 0;
            let rc = unsafe { AdsExecuteSQLDirect(stmt, sql_text.as_mut_ptr(), &mut handle) };
            assert_eq!(rc, AE_SUCCESS, "{cursor}");
            assert_eq!(AdsGotoTop(handle), AE_SUCCESS);
            let mut field_name = format!("{field}\0").into_bytes();
            // `#n` is the field number n, given instead of a name.
            let f = match field.strip_prefix('#') {
                Some(n) => std::ptr::without_provenance_mut(n.parse().unwrap()),
                None => field_name.as_mut_ptr(),
            };
            let mut changed = *b"changed";
            let mut wide: Vec<Wchar> = "changed".encode_utf16().collect();
            let mut date = *b"01/02/2020";
            let mut stamp = *b"01/02/2020 03:04:05.250";
            let mut abc = *b"abc";
            let mut long = *b"a text longer than the field";
            let rc = unsafe {
                match name {
                    "AdsSetString" => AdsSetString(handle, f, changed.as_mut_ptr(), 7),
                    "AdsSetString long" => AdsSetString(handle, f, long.as_mut_ptr(), 28),
                    "AdsSetString bad" => AdsSetString(handle, f, abc.as_mut_ptr(), 3),
                    "AdsSetStringW" => AdsSetStringW(handle, f, wide.as_mut_ptr(), 7),
                    "AdsSetField" => AdsSetField(handle, f, changed.as_mut_ptr(), 7),
                    "AdsSetLong" => AdsSetLong(handle, f, 4242),
                    "AdsSetShort" => AdsSetShort(handle, f, 42),
                    "AdsSetDouble" => AdsSetDouble(handle, f, 42.5),
                    "AdsSetLogical" => AdsSetLogical(handle, f, 1),
                    "AdsSetDate" => AdsSetDate(handle, f, date.as_mut_ptr(), 10),
                    "AdsSetMoney" => AdsSetMoney(handle, f, 12345),
                    "AdsSetNull" => AdsSetNull(handle, f),
                    "AdsSetEmpty" => AdsSetEmpty(handle, f),
                    "AdsSetBinary" => AdsSetBinary(handle, f, 6, 3, 0, abc.as_mut_ptr(), 3),
                    "AdsSetJulian" => AdsSetJulian(handle, f, 2458850),
                    "AdsSetTimeStamp" => AdsSetTimeStamp(handle, f, stamp.as_mut_ptr(), 23),
                    "AdsSetLongLong" => AdsSetLongLong(handle, f, 4242),
                    other => panic!("{other}"),
                }
            };
            let last = || {
                let (mut code, mut len, mut buf) = (0, 600u16, [0u8; 600]);
                unsafe { AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
                (
                    u64::from(code),
                    String::from_utf8_lossy(until_nul(&buf[..usize::from(len)])).into_owned(),
                )
            };
            let set_last = last();
            let write = match how {
                "write" => AdsWriteRecord(handle),
                "move" => AdsGotoBottom(handle),
                _ => 0,
            };
            if how == "move" {
                AdsGotoTop(handle);
            }
            let (mut value, mut len) = ([0u8; 600], 600u32);
            let got_value =
                unsafe { AdsGetField(handle, f, value.as_mut_ptr(), &mut len, 0) } == AE_SUCCESS;
            let value =
                got_value.then(|| String::from_utf8_lossy(&value[..len as usize]).into_owned());
            AdsCloseSQLStatement(stmt);
            AdsDisconnect(connect);
            let table_changed = ["adt", "adm"].iter().any(|ext| {
                let file = format!("all_types.{ext}");
                std::fs::read(dir.join(&file)).unwrap()
                    != std::fs::read(original.join(&file)).unwrap()
            });
            // `AdsGetField` of a binary field of a cursor the original does not
            // write answers an empty text, where tflip gives the bytes (see
            // KNOWN_ISSUES.md).
            let value = value.filter(|_| !(field == "c_blob" && rc != AE_SUCCESS));
            let want_value = row["value"]
                .as_str()
                .filter(|_| !(field == "c_blob" && rc != AE_SUCCESS))
                .map(str::to_owned);
            let want = (
                row["rc"].as_u64().unwrap(),
                row["code"].as_u64().unwrap(),
                text("text").to_owned(),
                row["write"].as_u64().unwrap(),
                want_value,
                row["table_changed"].as_bool().unwrap(),
            );
            let got = (
                u64::from(rc),
                set_last.0,
                set_last.1,
                u64::from(write),
                value,
                table_changed,
            );
            if got != want {
                problems.push(format!(
                    "{cursor} {name} {field} {how}: got {got:?}, want {want:?}"
                ));
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// NUL bytes in the text `AdsSetString` and `AdsSetField` are given are
    /// stored as in the original engine (`fixtures/fields/nul_strings.json`,
    /// from `tools/ace/fields/nul_strings.c`, whose steps this repeats).
    #[test]
    fn nul_bytes_in_set_text() {
        use crate::{
            AdsAppendRecord, AdsCloseTable, AdsCreateIndex61, AdsCreateTable, AdsGetField,
            AdsGetRecordNum, AdsGotoRecord, AdsSeek, AdsWriteRecord,
        };
        let want: Vec<serde_json::Value> = serde_json::from_slice(
            &std::fs::read(fixtures::dir("../fields/nul_strings.json")).unwrap(),
        )
        .unwrap();
        let (source, _dir) =
            fixtures::scratch_from("nul_bytes_in_set_text", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let shown = |bytes: &[u8]| -> String {
            bytes
                .iter()
                .map(|&b| match b {
                    0 => "\\0".to_owned(),
                    b => char::from(b).to_string(),
                })
                .collect()
        };
        let c = |text: &str| std::ffi::CString::new(text).unwrap();
        let texts: [&[u8]; 4] = [b"ab\0cd", b"abcd\0", b"\0abc", b"ab\0\0\0\0\0\0"];
        let mut got = Vec::new();
        for (kind, table_type, fields, extension, options) in [
            ("NTX", crate::ADS_NTX, "C,C,8,0;M,M,10,0", "ntx", 0),
            ("CDX", crate::ADS_CDX, "C,C,8,0;M,M,10,0", "cdx", 2),
            (
                "ADT",
                crate::ADS_ADT,
                "C,Char,8;M,Memo;V,VarChar,8;Z,CIChar,8",
                "adi",
                2,
            ),
            ("VFP", crate::ADS_VFP, "C,C,8,0;M,M,10,0", "cdx", 2),
        ] {
            let names: &[&str] = match table_type {
                crate::ADS_ADT => &["C", "M", "V", "Z"],
                _ => &["C", "M"],
            };
            let (name, fields) = (c(&format!("n{table_type}")), c(fields));
            let mut table = 0;
            let rc = unsafe {
                AdsCreateTable(
                    connect,
                    name.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    table_type,
                    1,
                    0,
                    1,
                    0,
                    fields.as_ptr().cast_mut().cast(),
                    &mut table,
                )
            };
            assert_eq!(rc, AE_SUCCESS);
            let mut orders = vec![0; names.len()];
            for (f, field) in names.iter().enumerate().filter(|&(f, _)| f != 1) {
                let own = if table_type == crate::ADS_NTX {
                    *field
                } else {
                    ""
                };
                let bag = c(&format!("n{table_type}{own}.{extension}"));
                let field = c(field);
                let rc = unsafe {
                    AdsCreateIndex61(
                        table,
                        bag.as_ptr().cast_mut().cast(),
                        field.as_ptr().cast_mut().cast(),
                        field.as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        options,
                        512,
                        &mut orders[f],
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
            }
            for setter in ["string", "field"] {
                for text in texts {
                    AdsAppendRecord(table);
                    let set: Vec<u32> = names
                        .iter()
                        .map(|field| {
                            let field = c(field);
                            let field = field.as_ptr().cast_mut().cast();
                            let mut bytes = text.to_vec();
                            let (buf, len) = (bytes.as_mut_ptr(), text.len() as u32);
                            unsafe {
                                match setter {
                                    "string" => AdsSetString(table, field, buf, len),
                                    _ => AdsSetField(table, field, buf, len),
                                }
                            }
                        })
                        .collect();
                    AdsWriteRecord(table);
                    let mut record = 0;
                    unsafe { AdsGetRecordNum(table, 2, &mut record) };
                    for (f, field) in names.iter().enumerate() {
                        let (mut bytes, mut len) = ([0u8; 64], 64u32);
                        AdsGotoRecord(table, record);
                        let name = c(field);
                        let read = unsafe {
                            AdsGetField(
                                table,
                                name.as_ptr().cast_mut().cast(),
                                bytes.as_mut_ptr(),
                                &mut len,
                                0,
                            )
                        };
                        let read = (read == AE_SUCCESS).then(|| shown(&bytes[..len as usize]));
                        let found = (orders[f] != 0).then(|| {
                            let (mut found, mut at) = (9, 0);
                            let mut key = text.to_vec();
                            unsafe {
                                AdsSeek(
                                    orders[f],
                                    key.as_mut_ptr(),
                                    text.len() as u16,
                                    2,
                                    2,
                                    &mut found,
                                );
                                AdsGetRecordNum(table, 2, &mut at);
                            }
                            found != 0 && at == record
                        });
                        got.push(serde_json::json!({"type": kind, "setter": setter,
                            "text": shown(text), "field": field, "rc": set[f],
                            "got": read, "found": found}));
                    }
                }
            }
            AdsCloseTable(table);
        }
        AdsDisconnect(connect);
        // tflip's legacy VarChar orders do not find a value with a NUL
        // inside as the original's do (docs/KNOWN_ISSUES.md).
        let seeks_differ = |row: &serde_json::Value| {
            row["field"] == "V"
                && row["setter"] == "string"
                && ["ab\\0cd", "\\0abc"].contains(&row["text"].as_str().unwrap())
        };
        assert_eq!(want.len(), got.len());
        for (want, mut got) in want.iter().zip(got) {
            if seeks_differ(want) {
                got["found"] = want["found"].clone();
            }
            assert_eq!(*want, got);
        }
    }

    remote_twins!(
        null_buffers_of_length_0_are_empty,
        setters_on_cursors_match_the_original_engine,
        nul_bytes_in_set_text
    );
}
