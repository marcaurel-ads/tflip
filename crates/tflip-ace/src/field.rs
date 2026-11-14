//! Fields of tables: their names, numbers, types, lengths, decimals and
//! offsets, whether they take NULL, and column permissions. Their values are
//! read in `getters.rs` and set in `setters.rs`.

use std::ffi::CStr;

use tflip_engine::Error;
use tflip_engine::getters::FieldId;
use tflip_engine::handle::HandleKind;
use tflip_engine::table::{FieldRef, TableInfo, find_field};
use tflip_storage::field::{Field, FieldType};

use crate::backend::{Backend, api};
use crate::last_error::{Call, fail};
use crate::{
    ADS_BYTE_LENGTH, ADS_BYTE_LENGTH_IN_BUFFER, ADS_CODEUNIT_LENGTH, ADS_FALSE, ADS_TRUE,
    AE_INVALID_OPTION, AE_INVALID_POINTER, AE_SUCCESS, AdsHandle, Unsigned8, Unsigned16,
    Unsigned32, ansi, error_code, write_string,
};

/// A field argument: a field name, or a 1-based field number that the client
/// passed in its place with `ADSFIELD(n)`, which casts the number to a
/// pointer.
#[derive(Clone)]
pub(crate) enum FieldArg {
    Number(usize),
    Name(String),
}

impl FieldArg {
    /// Reads a field argument. Windows never maps the first 64 KB of the
    /// address space, so a "pointer" below 0x10000 is a field number (null is
    /// field number 0, which does not exist).
    ///
    /// # Safety
    ///
    /// `field` must be a field number below 0x10000 or point to a
    /// NUL-terminated string.
    pub(crate) unsafe fn new(field: *const Unsigned8) -> FieldArg {
        let value = field as usize;
        if value < 0x10000 {
            FieldArg::Number(value)
        } else {
            // SAFETY: not a number, so a string, as the caller promises.
            FieldArg::Name(ansi::data_in(
                unsafe { CStr::from_ptr(field.cast()) }.to_bytes(),
            ))
        }
    }

    /// The field as the engine takes it.
    pub(crate) fn id(self) -> FieldId {
        match self {
            FieldArg::Number(n) => FieldId::Number(n),
            FieldArg::Name(name) => FieldId::Name(name),
        }
    }

    pub(crate) fn as_ref(&self) -> FieldRef<'_> {
        match self {
            FieldArg::Number(n) => FieldRef::Number(*n),
            FieldArg::Name(name) => FieldRef::Name(name),
        }
    }

    /// The 1-based number of the field of `table` this names. A number is
    /// passed on as it is, for the engine to check.
    pub(crate) fn number(&self, backend: &Backend, table: AdsHandle) -> Result<usize, Error> {
        match self {
            FieldArg::Number(n) => Ok(*n),
            FieldArg::Name(name) => {
                let info = backend.table_info(table)?;
                find_field(fields(&info), FieldRef::Name(name)).map(|(number, _)| number)
            }
        }
    }
}

/// The fields of a table.
pub(crate) fn fields(info: &TableInfo) -> &[Field] {
    &info.structure.fields
}

/// Calls `f` on the field `field` of `table`, or returns the ACE error code
/// if either does not exist.
pub(crate) fn with_field<R>(
    table: AdsHandle,
    field: FieldArg,
    f: impl FnOnce(&Field) -> R,
) -> Result<R, Unsigned32> {
    let backend = api(table);
    let info = backend.table_info(table).map_err(error_code)?;
    find_field(fields(&info), field.as_ref())
        .map(|(_, field)| f(field))
        .map_err(|error| {
            // A cursor's fields are its columns (`fixtures/errors/param_n`).
            let cursor = backend.handle_kind(table) == Ok(Some(HandleKind::Cursor));
            error_code(match (cursor, error) {
                (true, tflip_engine::Error::InvalidFieldName) => tflip_engine::Error::Constraint {
                    code: 5113,
                    text: "The column name specified was invalid for the cursor.".to_owned(),
                },
                (true, tflip_engine::Error::InvalidFieldNumber) => {
                    tflip_engine::Error::Constraint {
                        code: 5112,
                        text: "The column number specified was invalid for the cursor.".to_owned(),
                    }
                }
                (_, error) => error,
            })
        })
}

/// Writes `value` to `out`, or fails with `AE_INVALID_POINTER` if `out` is
/// null; `value` is the result of [`with_field`].
///
/// # Safety
///
/// `out` must be null or valid for writes.
pub(crate) unsafe fn write<T>(value: Result<T, Unsigned32>, out: *mut T) -> Unsigned32 {
    match value {
        Err(code) => code,
        Ok(_) if out.is_null() => fail(AE_INVALID_POINTER),
        Ok(value) => {
            // SAFETY: not null, so writable, as the caller promises.
            unsafe { *out = value };
            AE_SUCCESS
        }
    }
}

/// [`write`] with `out` the call's argument number `param`: a NULL one
/// fails naming its number (`fixtures/errors/param_n.json`).
///
/// # Safety
///
/// `out` must be null or valid for writes.
pub(crate) unsafe fn write_at<T>(
    value: Result<T, Unsigned32>,
    out: *mut T,
    param: u32,
) -> Unsigned32 {
    match value {
        Ok(_) if out.is_null() => crate::last_error::null_param(param),
        // SAFETY: as the caller promises.
        value => unsafe { write(value, out) },
    }
}

/// `AdsGetFieldName`: the name of field number `number` (1-based) of a table.
///
/// # Safety
///
/// `name` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetFieldName(
    table: AdsHandle,
    number: Unsigned16,
    name: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetFieldName");
    let info = match api(table).table_info(table) {
        Ok(info) => info,
        Err(error) => return error_code(error),
    };
    // The pointers are checked before the field number.
    if let Some(code) = crate::ffi::null_buffer(name, len, 3) {
        return code;
    }
    match find_field(fields(&info), FieldRef::Number(usize::from(number))) {
        // The name goes out as its bytes, as table data does.
        Ok((_, field)) => {
            let bytes = ansi::data_out(&field.name);
            // SAFETY: `name` and `len` are as # Safety says.
            unsafe { write_string(&bytes, name, len) }
        }
        Err(error) => error_code(error),
    }
}

/// `AdsGetFieldType`: the type of a field, as one of the `ADS_*` field type
/// constants (the codes ADT tables store).
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string; `field_type` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetFieldType(
    table: AdsHandle,
    field: *mut Unsigned8,
    field_type: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetFieldType");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    let value = with_field(table, field, |f| f.field_type.code());
    // SAFETY: `field_type` is null or writable, as # Safety says.
    unsafe { write_at(value, field_type, 3) }
}

/// `AdsGetFieldLength`: the declared length of a field. For the
/// variable-length and Unicode types this is less than the bytes the field
/// takes in a record (VarChar(n) takes n + 2, NChar(n) 2n).
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string; `length` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetFieldLength(
    table: AdsHandle,
    field: *mut Unsigned8,
    length: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetFieldLength");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    let value = with_field(table, field, |f| Unsigned32::from(f.length()));
    // SAFETY: `length` is null or writable, as # Safety says.
    unsafe { write_at(value, length, 3) }
}

/// `AdsGetFieldLength100`: the declared length of a field, counted as
/// `options` says, as in the original engine: in characters
/// (`ADS_CODEUNIT_LENGTH`, as `AdsGetFieldLength` counts), in bytes
/// (`ADS_BYTE_LENGTH`: an NChar(5) is 10, an NVarChar(10) 20), or in the
/// bytes it takes in a record (`ADS_BYTE_LENGTH_IN_BUFFER`: a VarChar(20) is
/// 22). Other options fail with `AE_INVALID_OPTION`.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string; `length` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetFieldLength100(
    table: AdsHandle,
    field: *mut Unsigned8,
    options: Unsigned32,
    length: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetFieldLength100");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    let lengths = with_field(table, field, |f| (f.field_type, f.length(), f.size));
    let length_of = lengths.and_then(|(field_type, characters, size)| match options {
        ADS_CODEUNIT_LENGTH => Ok(Unsigned32::from(characters)),
        ADS_BYTE_LENGTH => Ok(Unsigned32::from(match field_type {
            FieldType::NChar => size,
            FieldType::NVarChar => size.saturating_sub(2),
            _ => characters,
        })),
        ADS_BYTE_LENGTH_IN_BUFFER => Ok(Unsigned32::from(size)),
        _ => Err(fail(AE_INVALID_OPTION)),
    });
    // SAFETY: `length` is null or writable, as # Safety says.
    unsafe { write(length_of, length) }
}

/// `AdsGetFieldDecimals`: the number of decimals of a field (0 for the types
/// without decimals, and for compressed memo fields).
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string; `decimals` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetFieldDecimals(
    table: AdsHandle,
    field: *mut Unsigned8,
    decimals: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetFieldDecimals");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    let value = with_field(table, field, |f| f.decimals());
    // SAFETY: `decimals` is null or writable, as # Safety says.
    unsafe { write_at(value, decimals, 3) }
}

/// `AdsGetFieldOffset`: where a field starts in a record, in bytes from the
/// start of the record. ADT records start with a 5-byte prefix, so the first
/// field is at 5, matching `AdsGetRecordLength`, which counts the prefix too
/// (as the original engine does; in DBF tables it is at 1, after the deleted
/// flag).
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string; `offset` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetFieldOffset(
    table: AdsHandle,
    field: *mut Unsigned8,
    offset: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetFieldOffset");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    let value = with_field(table, field, |f| Unsigned32::from(f.offset));
    // SAFETY: `offset` is null or writable, as # Safety says.
    unsafe { write_at(value, offset, 3) }
}

/// The column permissions of a field outside a data dictionary: every
/// `ADS_DD_COLUMN_PERMISSION_*` flag (from the unpublished ACE declarations)
/// set, as the original engine reports for free tables.
const COLUMN_PERMISSIONS_FREE: Unsigned8 = 0xff;

/// `AdsGetColumnPermissions`: what the user may do with field number `number`
/// (1-based) of a table, as `ADS_DD_COLUMN_PERMISSION_*` flags in one byte.
///
/// Unpublished: not declared in the SDK's `ace.h`; the signature is
/// `UNSIGNED32 AdsGetColumnPermissions( ADSHANDLE hTbl, UNSIGNED16 usFieldNum,
/// UNSIGNED8 *pucPermissions )`.
///
/// Dictionary rights are not read yet, so every field reports 0xFF, all
/// flags set, as the original engine does for free tables. Field numbers 0
/// and past the last field fail with `AE_INVALID_FIELDNUMBER`. Unlike the
/// original engine, a null `permissions` fails with `AE_INVALID_POINTER`
/// instead of being written through.
///
/// # Safety
///
/// `permissions` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetColumnPermissions(
    table: AdsHandle,
    number: Unsigned16,
    permissions: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetColumnPermissions");
    let info = match api(table).table_info(table) {
        Ok(info) => info,
        Err(error) => return error_code(error),
    };
    let flags = find_field(fields(&info), FieldRef::Number(usize::from(number)))
        .map(|_| COLUMN_PERMISSIONS_FREE)
        .map_err(error_code);
    // SAFETY: `permissions` is null or writable, as # Safety says.
    unsafe { write(flags, permissions) }
}

/// `AdsIsNullable`: whether a field can hold NULL (`ADS_TRUE`) or not.
///
/// Every ADT field type can hold NULL except AutoInc and RowVersion. `NOT
/// NULL` constraints of dictionary-bound tables are not read yet.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a NUL-terminated
/// string; `nullable` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsNullable(
    table: AdsHandle,
    field: *mut Unsigned8,
    nullable: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsNullable");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    // A DBF field can only with a NULL bit, which only a VFP field defined
    // with NULL has (`fixtures/fields/nullable.json`).
    let value = with_field(table, field, |f| f.field_type.nullable() && f.has_null())
        .map(|n| if n { ADS_TRUE } else { ADS_FALSE });
    // SAFETY: `nullable` is null or writable, as # Safety says.
    unsafe { write(value, nullable) }
}

/// `AdsGetFieldNum`: the 1-based number of a field, by name or number.
///
/// # Safety
///
/// `field` must be a field number below 0x10000 or point to a
/// NUL-terminated string; `number` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetFieldNum(
    table: AdsHandle,
    field: *mut Unsigned8,
    number: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetFieldNum");
    // SAFETY: `field` is a field number or a string, as # Safety says.
    let field = unsafe { FieldArg::new(field) };
    let found = api(table)
        .table_info(table)
        .map_err(error_code)
        .and_then(|info| {
            let (n, _) = find_field(fields(&info), field.as_ref()).map_err(error_code)?;
            Ok(n as Unsigned16)
        });
    // As in the original engine, a field that is not found is number 0.
    if let Err(code) = found
        && code != crate::AE_INVALID_TABLE_HANDLE
        && !number.is_null()
    {
        // SAFETY: not null, so writable, as # Safety says.
        unsafe { *number = 0 };
    }
    // SAFETY: `number` is null or writable, as # Safety says.
    unsafe { write(found, number) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AE_INSUFFICIENT_BUFFER, AE_INVALID_FIELDNAME, AE_INVALID_FIELDNUMBER,
        AE_INVALID_TABLE_HANDLE, fixtures,
    };

    /// `ADSFIELD(n)`: field number `n` passed as a field name.
    fn adsfield(n: usize) -> *mut Unsigned8 {
        n as *mut Unsigned8
    }

    #[test]
    fn field_types() {
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let mut value = 0;
        assert_eq!(
            unsafe { AdsGetFieldType(table, adsfield(1), &mut value) },
            AE_SUCCESS
        );
        assert_eq!(value, 4); // ADS_STRING (Character)
        let name = c"C_MONEY".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { AdsGetFieldType(table, name, &mut value) },
            AE_SUCCESS
        );
        assert_eq!(value, 18); // ADS_MONEY

        let rc = unsafe { AdsGetFieldType(table, adsfield(25), &mut value) };
        assert_eq!(rc, AE_INVALID_FIELDNUMBER);
        let rc = unsafe { AdsGetFieldType(table, std::ptr::null_mut(), &mut value) };
        assert_eq!(rc, AE_INVALID_FIELDNUMBER);
        let nope = c"nope".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { AdsGetFieldType(table, nope, &mut value) },
            AE_INVALID_FIELDNAME
        );
        let rc = unsafe { AdsGetFieldType(table, adsfield(1), std::ptr::null_mut()) };
        assert_eq!(rc, AE_INVALID_POINTER);
        let rc = unsafe { AdsGetFieldType(connect, adsfield(1), &mut value) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn field_names() {
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let mut name = [0xffu8; 16];
        let mut len = name.len() as u16;
        assert_eq!(
            unsafe { AdsGetFieldName(table, 1, name.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert_eq!(&name[..usize::from(len) + 1], b"c_char\0");
        let mut len = name.len() as u16;
        assert_eq!(
            unsafe { AdsGetFieldName(table, 24, name.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert_eq!(&name[..usize::from(len)], b"c_rowversion");

        let mut small = [0u8; 4];
        let mut len = small.len() as u16;
        let rc = unsafe { AdsGetFieldName(table, 1, small.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INSUFFICIENT_BUFFER);
        let mut len = name.len() as u16;
        let rc = unsafe { AdsGetFieldName(table, 0, name.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INVALID_FIELDNUMBER);
        let rc = unsafe { AdsGetFieldName(connect, 1, name.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    #[test]
    fn column_permissions() {
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let mut flags = 0;
        assert_eq!(
            unsafe { AdsGetColumnPermissions(table, 1, &mut flags) },
            AE_SUCCESS
        );
        assert_eq!(flags, 0xff);
        assert_eq!(
            unsafe { AdsGetColumnPermissions(table, 24, &mut flags) },
            AE_SUCCESS
        );
        let rc = unsafe { AdsGetColumnPermissions(table, 0, &mut flags) };
        assert_eq!(rc, AE_INVALID_FIELDNUMBER);
        let rc = unsafe { AdsGetColumnPermissions(table, 25, &mut flags) };
        assert_eq!(rc, AE_INVALID_FIELDNUMBER);
        let rc = unsafe { AdsGetColumnPermissions(connect, 1, &mut flags) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);
        let rc = unsafe { AdsGetColumnPermissions(table, 1, std::ptr::null_mut()) };
        assert_eq!(rc, AE_INVALID_POINTER);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    /// Every field's name, type, length and decimals, as the JSON dump of
    /// the fixture records them from the original engine.
    #[test]
    fn fields_match_the_engine() {
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let expected = [
            ("c_char", 4, 10, 0),
            ("c_cichar", 20, 10, 0),
            ("c_varchar", 23, 20, 0),
            ("c_nchar", 26, 5, 0),
            ("c_nvarchar", 27, 10, 0),
            ("c_memo", 5, 9, 0),
            ("c_nmemo", 28, 9, 0),
            ("c_blob", 6, 9, 0),
            ("c_raw", 16, 4, 0),
            ("c_varbinary", 24, 8, 0),
            ("c_guid", 29, 16, 0),
            ("c_logical", 1, 1, 0),
            ("c_numeric", 2, 10, 2),
            ("c_short", 12, 2, 0),
            ("c_integer", 11, 4, 0),
            ("c_longint", 19, 8, 0),
            ("c_double", 10, 8, 15),
            ("c_curdouble", 17, 8, 2),
            ("c_money", 18, 8, 4),
            ("c_date", 3, 4, 0),
            ("c_time", 13, 4, 0),
            ("c_timestamp", 14, 8, 0),
            ("c_autoinc", 15, 4, 0),
            ("c_rowversion", 21, 8, 0),
        ];
        let mut offsets = Vec::new();
        for (n, &(name, field_type, length, decimals)) in expected.iter().enumerate() {
            let field = adsfield(n + 1);
            let mut buf = [0u8; 32];
            let mut len = buf.len() as u16;
            let number = n as u16 + 1;
            assert_eq!(
                unsafe { AdsGetFieldName(table, number, buf.as_mut_ptr(), &mut len) },
                0
            );
            assert_eq!(&buf[..usize::from(len)], name.as_bytes());
            let (mut t, mut l, mut d) = (0, 0, 0);
            assert_eq!(unsafe { AdsGetFieldType(table, field, &mut t) }, AE_SUCCESS);
            assert_eq!(
                unsafe { AdsGetFieldLength(table, field, &mut l) },
                AE_SUCCESS
            );
            assert_eq!(
                unsafe { AdsGetFieldDecimals(table, field, &mut d) },
                AE_SUCCESS
            );
            assert_eq!((t, l, d), (field_type, length, decimals), "{name}");
            let mut offset = 0;
            assert_eq!(
                unsafe { AdsGetFieldOffset(table, field, &mut offset) },
                AE_SUCCESS
            );
            offsets.push(offset);
            let mut nullable = 0;
            assert_eq!(
                unsafe { AdsIsNullable(table, field, &mut nullable) },
                AE_SUCCESS
            );
            let never_null = field_type == 15 || field_type == 21; // AutoInc, RowVersion
            assert_eq!(
                nullable,
                if never_null { ADS_FALSE } else { ADS_TRUE },
                "{name}"
            );
        }
        // The fields follow each other from 5, after the record prefix.
        assert_eq!(offsets[0], 5);
        assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    /// Field lengths in characters, bytes and record bytes, as the original
    /// engine counts them.
    #[test]
    fn field_lengths_by_unit() {
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let length = |name: &CStr, options| {
            let mut length = 0;
            let name = name.as_ptr().cast_mut().cast();
            let rc = unsafe { AdsGetFieldLength100(table, name, options, &mut length) };
            (rc, length)
        };
        for (name, expected) in [
            (c"c_char", [10, 10, 10]),
            (c"c_varchar", [20, 20, 22]),
            (c"c_nchar", [5, 10, 10]),
            (c"c_nvarchar", [10, 20, 22]),
            (c"c_memo", [9, 9, 9]),
            (c"c_integer", [4, 4, 4]),
            (c"c_varbinary", [8, 8, 10]),
        ] {
            let lengths = [0, 1, 2].map(|options| length(name, options).1);
            assert_eq!(lengths, expected, "{name:?}");
        }
        assert_eq!(length(c"c_char", 3).0, AE_INVALID_OPTION);
        assert_eq!(length(c"nope", 0).0, AE_INVALID_FIELDNAME);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    /// Which fields can hold NULL, and what `AdsSetNull` does to them, as in
    /// the original engine (`fixtures/fields/nullable.json`, from
    /// `tools/ace/fields/nullable.c`, whose steps this repeats).
    #[test]
    fn nullable_fields() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/fields/nullable.json");
        let want: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, _dir) = fixtures::scratch_from("nullable_fields", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let values: std::collections::HashMap<&str, &str> = [
            ("Character", "abcd"),
            ("C", "abcd"),
            ("Numeric", "12.5"),
            ("Memo", "abc"),
            ("Integer", "7"),
            ("Double", "1.5"),
            ("Currency", "3"),
            ("VarChar", "ab"),
        ]
        .into_iter()
        .collect();
        let mut got = Vec::new();
        for row in &want {
            let (kind, def) = (row["type"].as_str().unwrap(), row["def"].as_str().unwrap());
            let table_type = match kind {
                "ADT" => crate::ADS_ADT,
                "NTX" => crate::ADS_NTX,
                "CDX" => crate::ADS_CDX,
                _ => crate::ADS_VFP,
            };
            let n = got
                .iter()
                .filter(|r: &&serde_json::Value| r["type"] == kind)
                .count();
            let name = std::ffi::CString::new(format!("n{table_type}_{n}")).unwrap();
            let fields = std::ffi::CString::new(format!("{def};X,Character,2,0")).unwrap();
            let mut table = 0;
            let create = unsafe {
                crate::AdsCreateTable(
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
            let (mut nullable, mut is_null, mut null_after, mut empty_after) = (9u16, 9, 9, 9);
            let mut set_null = 9;
            if create == AE_SUCCESS {
                let f = c"F".as_ptr().cast_mut().cast();
                crate::AdsAppendRecord(table);
                let field_type = def.split(',').nth(1).unwrap();
                if let Some(value) = values.get(field_type) {
                    unsafe {
                        crate::AdsSetField(table, f, value.as_ptr().cast_mut(), value.len() as u32)
                    };
                }
                unsafe {
                    AdsIsNullable(table, f, &mut nullable);
                    crate::AdsIsNull(table, f, &mut is_null);
                    set_null = crate::AdsSetNull(table, f);
                    crate::AdsIsNull(table, f, &mut null_after);
                    crate::AdsIsEmpty(table, f, &mut empty_after);
                }
                crate::AdsWriteRecord(table);
                crate::AdsCloseTable(table);
            }
            got.push(
                serde_json::json!({"type": kind, "def": def, "create": create,
                "nullable": nullable, "is_null": is_null, "set_null": set_null,
                "null_after": null_after, "empty_after": empty_after}),
            );
        }
        crate::AdsDisconnect(connect);
        let problems: Vec<String> = want
            .iter()
            .zip(&got)
            .filter(|(want, got)| want != got)
            .map(|(want, got)| format!("want {want}\n got {got}"))
            .collect();
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// Which values after a field's type `AdsCreateTable` takes, as in the
    /// original engine: a Clipper or FoxPro field has no more than four
    /// (`fixtures/errors/field_attributes.json`, from
    /// `tools/ace/errors/field_attributes.c`, whose steps this repeats).
    #[test]
    fn field_attributes() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/errors/field_attributes.json");
        let want: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, _dir) = fixtures::scratch_from("field_attributes", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let mut got = Vec::new();
        for row in &want {
            let (kind, def) = (row["type"].as_str().unwrap(), row["def"].as_str().unwrap());
            let table_type = match kind {
                "ADT" => crate::ADS_ADT,
                "NTX" => crate::ADS_NTX,
                "CDX" => crate::ADS_CDX,
                _ => crate::ADS_VFP,
            };
            let n = got
                .iter()
                .filter(|r: &&serde_json::Value| r["type"] == kind)
                .count();
            let name = std::ffi::CString::new(format!("a{table_type}_{n}")).unwrap();
            let fields = std::ffi::CString::new(def).unwrap();
            let mut table = 0;
            let rc = unsafe {
                crate::AdsCreateTable(
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
            let mut row = serde_json::json!({"type": kind, "def": def, "rc": rc});
            if rc == AE_SUCCESS {
                crate::AdsCloseTable(table);
            } else {
                let (mut code, mut buf, mut len) = (0, [0u8; 1200], 1200);
                unsafe { crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
                row["error"] = String::from_utf8_lossy(&buf[..usize::from(len)]).into();
            }
            got.push(row);
        }
        crate::AdsDisconnect(connect);
        let problems: Vec<String> = want
            .iter()
            .zip(&got)
            .filter(|(want, got)| want != got)
            .map(|(want, got)| format!("want {want}\n got {got}"))
            .collect();
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    crate::fixtures::remote_twins!(
        column_permissions,
        field_attributes,
        nullable_fields,
        field_lengths_by_unit,
        field_names,
        fields_match_the_engine,
        field_types
    );
}
