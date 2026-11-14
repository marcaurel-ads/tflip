//! Creating tables and index orders, and keeping tables in shape:
//! `AdsCreateTable`, `AdsCreateIndex`, `AdsDeleteIndex`, `AdsReindex`,
//! `AdsPackTable`, `AdsZapTable`, `AdsRecallAllRecords`, `AdsDeleteTable`,
//! restructuring (`AdsRestructureTable`), copying (`AdsCopyTable`, ...) and
//! converting tables (`AdsConvertTable`), as the original engine does them
//! (`fixtures/write/`; see [`tflip_engine::ddl`]), for ADT and DBF tables.

use tflip_engine::table::{TableSettings, TableType};

use crate::backend::api;
use crate::ffi::text;
use crate::last_error::fail;
use crate::table::{check_options, engine_table_type};
use crate::{
    AE_INVALID_CHARSETTYPE, AE_INVALID_POINTER, AE_INVALID_TABLETYPE, AE_SUCCESS, AdsHandle,
    Unsigned8, Unsigned16, Unsigned32, ansi, code, error_code,
};

/// Creates a table and opens it (see [`AdsCreateTable90`]); `None` for a
/// null pointer.
///
/// `ADS_DEFAULT` makes an ADT table. Any other type but the four fails
/// with `AE_INVALID_TABLETYPE` and leaves the last error alone, as in the
/// original engine; the other options are checked as
/// [`check_options`] says (`fixtures/errors/table_options.json`).
#[allow(clippy::too_many_arguments)]
fn create_table(
    connect: AdsHandle,
    name: Option<String>,
    table_type: Unsigned16,
    char_type: Unsigned16,
    lock_type: Unsigned16,
    check_rights: Unsigned16,
    memo_size: Unsigned16,
    fields: Option<String>,
    collation: Option<String>,
    table: Option<&mut AdsHandle>,
) -> Unsigned32 {
    let (Some(name), Some(fields), Some(table)) = (name, fields, table) else {
        return fail(AE_INVALID_POINTER);
    };
    let table_type = match table_type {
        crate::ADS_DEFAULT => TableType::Adt,
        other => match engine_table_type(other) {
            Some(table_type) => table_type,
            None => {
                crate::last_error::restore_pending();
                return AE_INVALID_TABLETYPE;
            }
        },
    };
    if let Some(code) = check_options(true, table_type, Some(char_type), lock_type, check_rights) {
        return code;
    }
    // The table comes back open exclusively, as in the original engine, so
    // that it can be encrypted at once (`fixtures/aes/api/00_probe`).
    // A VFP table's collation says whether it is OEM, as its character type
    // does (`fixtures/dbf/vfp_oem.json`); others are not kept yet.
    let collation = match table_type {
        TableType::Vfp => collation.unwrap_or_default(),
        _ => String::new(),
    };
    let settings = TableSettings {
        table_type,
        char_type,
        collation,
        rights: check_rights,
        options: tflip_engine::table::options::EXCLUSIVE
            | tflip_engine::table::options::CREATED
            | crate::table::locking_option(table_type, lock_type),
        ..Default::default()
    };
    let name = crate::remote::relative_name(connect, &name);
    match api(connect).create_table(connect, name, fields, memo_size, settings) {
        Ok(handle) => {
            *table = handle;
            AE_SUCCESS
        }
        Err(error) => error_code(error),
    }
}

/// `AdsCreateTable`: [`AdsCreateTable90`] with an alias, which is not kept.
///
/// # Safety
///
/// As for [`AdsCreateTable90`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCreateTable(
    connect: AdsHandle,
    name: *mut Unsigned8,
    _alias: *mut Unsigned8,
    table_type: Unsigned16,
    char_type: Unsigned16,
    lock_type: Unsigned16,
    check_rights: Unsigned16,
    memo_size: Unsigned16,
    fields: *mut Unsigned8,
    table: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCreateTable");
    // SAFETY: the pointers are as # Safety says.
    let (name, fields, table) = unsafe { (text(name), text(fields), table.as_mut()) };
    create_table(
        connect,
        name,
        table_type,
        char_type,
        lock_type,
        check_rights,
        memo_size,
        fields,
        None,
        table,
    )
}

/// `AdsCreateTable71`: [`AdsCreateTable90`] without a collation.
///
/// # Safety
///
/// As for [`AdsCreateTable90`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCreateTable71(
    connect: AdsHandle,
    name: *mut Unsigned8,
    _object_name: *mut Unsigned8,
    table_type: Unsigned16,
    char_type: Unsigned16,
    lock_type: Unsigned16,
    check_rights: Unsigned16,
    memo_size: Unsigned16,
    fields: *mut Unsigned8,
    _options: Unsigned32,
    table: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCreateTable71");
    // SAFETY: the pointers are as # Safety says.
    let (name, fields, table) = unsafe { (text(name), text(fields), table.as_mut()) };
    create_table(
        connect,
        name,
        table_type,
        char_type,
        lock_type,
        check_rights,
        memo_size,
        fields,
        None,
        table,
    )
}

/// `AdsCreateTable90`: creates the table `name` (an ADT table; others are
/// `AE_NOT_IMPLEMENTED`) with the fields `fields` describes
/// (`name,type,length,decimals;…`, the type's name as short as is unique)
/// and memo blocks of `memo_size` bytes (0: 256), and opens it. A definition
/// the original engine refuses is `AE_INVALID_FIELDDEF`, a file there
/// already 7040. The object name and options are not used, nor the
/// collation, but that a VFP table's makes it OEM as its character type
/// would.
///
/// # Safety
///
/// `name` and `fields` must point to NUL-terminated strings; the others
/// must be null or point to them; `table` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCreateTable90(
    connect: AdsHandle,
    name: *mut Unsigned8,
    _object_name: *mut Unsigned8,
    table_type: Unsigned16,
    char_type: Unsigned16,
    lock_type: Unsigned16,
    check_rights: Unsigned16,
    memo_size: Unsigned16,
    fields: *mut Unsigned8,
    _options: Unsigned32,
    collation: *mut Unsigned8,
    table: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCreateTable90");
    // SAFETY: the pointers are as # Safety says.
    let (name, fields, collation, table) =
        unsafe { (text(name), text(fields), text(collation), table.as_mut()) };
    create_table(
        connect,
        name,
        table_type,
        char_type,
        lock_type,
        check_rights,
        memo_size,
        fields,
        collation,
        table,
    )
}

/// Adds an existing table to a data dictionary (see [`AdsDDAddTable`]);
/// `None` for a null pointer.
fn add_table(
    dictionary: AdsHandle,
    name: Option<String>,
    path: Option<String>,
    table_type: Unsigned16,
    char_type: Unsigned16,
    index_files: Option<String>,
    comments: Option<String>,
) -> Unsigned32 {
    let (Some(name), Some(path)) = (name, path) else {
        return fail(AE_INVALID_POINTER);
    };
    // `ADS_DEFAULT` is no type to add, nor is a character type of none or
    // beyond the collations (1 to 118), and a Clipper or FoxPro table takes
    // ADS_ANSI or ADS_OEM only (`fixtures/dictionary/dbfcorners`).
    let Some(table_type) = engine_table_type(table_type).filter(|t| *t != TableType::Database)
    else {
        return crate::last_error::fail_detail(AE_INVALID_TABLETYPE, "Parameter 4");
    };
    if !(1..=118).contains(&char_type) {
        return crate::last_error::fail_detail(AE_INVALID_CHARSETTYPE, "Parameter 5");
    }
    if matches!(table_type, TableType::Ntx | TableType::Cdx) && char_type > 2 {
        return crate::last_error::fail_detail(
            AE_INVALID_TABLETYPE,
            "Collation languages can only be specified with ADS_ADT and ADS_VFP table types.",
        );
    }
    let index_files = index_files.unwrap_or_default();
    let comments = comments.unwrap_or_default();
    match api(dictionary).add_table(
        dictionary,
        name,
        path,
        table_type,
        char_type,
        index_files,
        comments,
    ) {
        Ok(()) => AE_SUCCESS,
        // A file that is not there is 7041, as a table's name found by
        // `AdsOpenTable101` is, the table's with its path.
        Err(error) if *error.plain() == tflip_engine::Error::FileNotFound => match &error {
            tflip_engine::Error::Detailed(_, detail) => {
                crate::last_error::fail_detail(crate::TABLE_NOT_FOUND, detail)
            }
            _ => fail(crate::TABLE_NOT_FOUND),
        },
        Err(error) => direct_error_code(error),
    }
}

/// `AdsDDAddTable`: adds the existing table at `path` (relative to the
/// dictionary's folder) to the data dictionary `dictionary`, as `name`, with a
/// Table object, a Field object per column and, for each file in `index_files`,
/// an Index object with a Key object per order; an ADT table's own file is
/// bound to the dictionary. ADT, NTX, CDX and VFP tables are added, as the
/// original engine adds them; `ADS_DEFAULT` is `AE_INVALID_TABLETYPE`. A DBF
/// table keeps `ADS_OEM` as its character type. The comments are not used yet.
///
/// # Safety
///
/// `name` and `path` must point to NUL-terminated strings; `index_files` and
/// `comments` must be null or point to them.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDAddTable(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
    path: *mut Unsigned8,
    table_type: Unsigned16,
    char_type: Unsigned16,
    index_files: *mut Unsigned8,
    comments: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDAddTable");
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let (name, path, index_files, comments) =
        unsafe { (text(name), text(path), text(index_files), text(comments)) };
    add_table(
        dictionary,
        name,
        path,
        table_type,
        char_type,
        index_files,
        comments,
    )
}

/// `AdsDDAddTable90`: [`AdsDDAddTable`] with a collation, which is not used.
///
/// # Safety
///
/// As for [`AdsDDAddTable`]; `collation` must be null or a NUL-terminated
/// string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDAddTable90(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
    path: *mut Unsigned8,
    table_type: Unsigned16,
    char_type: Unsigned16,
    index_files: *mut Unsigned8,
    comments: *mut Unsigned8,
    _collation: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDAddTable90");
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let (name, path, index_files, comments) =
        unsafe { (text(name), text(path), text(index_files), text(comments)) };
    add_table(
        dictionary,
        name,
        path,
        table_type,
        char_type,
        index_files,
        comments,
    )
}

/// Adds a view to a data dictionary (see [`AdsDDAddView`]); `None` for a
/// null pointer. `sql` is the statement, already read from its ANSI or wide
/// form.
fn add_view(
    dictionary: AdsHandle,
    name: Option<String>,
    comments: Option<String>,
    sql: String,
) -> Unsigned32 {
    let Some(name) = name else {
        return fail(AE_INVALID_POINTER);
    };
    let comments = comments.unwrap_or_default();
    match api(dictionary).add_view(dictionary, name, sql, comments) {
        Ok(()) => AE_SUCCESS,
        // The engine's refusals of a name or a table are its own codes
        // (`fixtures/sql/ddviews.json`).
        Err(tflip_engine::Error::Sql(sql)) if (5000..6000).contains(&sql.native) => {
            direct_error_code(tflip_engine::Error::Sql(sql))
        }
        // The SQL engine's own code is what this returns, not 7200 as for a
        // statement (`fixtures/sql/views.json`).
        Err(tflip_engine::Error::Sql(sql)) => {
            let native = sql.native;
            error_code(tflip_engine::Error::Sql(sql));
            native
        }
        Err(error) => error_code(error),
    }
}

/// `AdsDDAddView`: adds a view named `name`, with the statement `sql` and the
/// comment `comments`, to the data dictionary `dictionary`. The statement is
/// stored as the original engine stores it.
///
/// # Safety
///
/// `name` and `sql` must point to NUL-terminated strings; `comments` must be
/// null or point to one.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDAddView(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
    comments: *mut Unsigned8,
    sql: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDAddView");
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let (name, comments, sql) = unsafe { (text(name), text(comments), text(sql)) };
    let Some(sql) = sql else {
        return fail(AE_INVALID_POINTER);
    };
    add_view(dictionary, name, comments, sql)
}

/// `AdsDDAddView100`: [`AdsDDAddView`] with the statement as wide characters.
///
/// # Safety
///
/// `name` must point to a NUL-terminated string, `sql` to a NUL-terminated
/// wide string; `comments` must be null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDAddView100(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
    comments: *mut Unsigned8,
    sql: *mut crate::Wchar,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDAddView100");
    if sql.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are NUL-terminated, or null where # Safety allows.
    let (name, comments, sql) = unsafe { (text(name), text(comments), crate::wide_string(sql)) };
    add_view(dictionary, name, comments, sql)
}

/// Adds a stored procedure to a data dictionary (see [`AdsDDAddProcedure`]);
/// `None` for a null pointer. `container` is already read from its ANSI or
/// wide form.
#[allow(clippy::too_many_arguments)]
fn add_procedure(
    dictionary: AdsHandle,
    name: Option<String>,
    container: String,
    proc_name: Option<String>,
    invoke_option: Unsigned32,
    in_params: Option<String>,
    out_params: Option<String>,
    comments: Option<String>,
) -> Unsigned32 {
    let Some(name) = name else {
        return fail(AE_INVALID_POINTER);
    };
    code_direct(api(dictionary).add_procedure(
        dictionary,
        name,
        container,
        proc_name.unwrap_or_default(),
        invoke_option,
        in_params.unwrap_or_default(),
        out_params.unwrap_or_default(),
        comments.unwrap_or_default(),
    ))
}

/// `AdsDDAddProcedure`: adds a stored procedure named `name` to the data
/// dictionary. `container` is the SQL script (when `invoke_option` has the
/// script bit) or the external library name; `proc_name` the function in that
/// library; `in_params`/`out_params` the parameter lists; `comments` the
/// comment. Stored as the original engine stores it.
///
/// # Safety
///
/// `name` and `container` must point to NUL-terminated strings; the rest must be
/// null or point to one.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn AdsDDAddProcedure(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
    container: *mut Unsigned8,
    proc_name: *mut Unsigned8,
    invoke_option: Unsigned32,
    in_params: *mut Unsigned8,
    out_params: *mut Unsigned8,
    comments: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDAddProcedure");
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let (container, name, proc_name, in_params, out_params, comments) = unsafe {
        (
            text(container),
            text(name),
            text(proc_name),
            text(in_params),
            text(out_params),
            text(comments),
        )
    };
    let Some(container) = container else {
        return fail(AE_INVALID_POINTER);
    };

    add_procedure(
        dictionary,
        name,
        container,
        proc_name,
        invoke_option,
        in_params,
        out_params,
        comments,
    )
}

/// `AdsDDAddProcedure100`: [`AdsDDAddProcedure`] with the container as wide
/// characters.
///
/// # Safety
///
/// `name` must point to a NUL-terminated string, `container` to a NUL-terminated
/// wide string; the rest must be null or NUL-terminated strings.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn AdsDDAddProcedure100(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
    container: *mut crate::Wchar,
    proc_name: *mut Unsigned8,
    invoke_option: Unsigned32,
    in_params: *mut Unsigned8,
    out_params: *mut Unsigned8,
    comments: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDAddProcedure100");
    if container.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are NUL-terminated, or null where # Safety allows.
    let (container, name, proc_name, in_params, out_params, comments) = unsafe {
        (
            crate::wide_string(container),
            text(name),
            text(proc_name),
            text(in_params),
            text(out_params),
            text(comments),
        )
    };

    add_procedure(
        dictionary,
        name,
        container,
        proc_name,
        invoke_option,
        in_params,
        out_params,
        comments,
    )
}

/// `AdsDDAddIndexFile`: adds the existing index file at `index_file_path`
/// (relative to the dictionary's folder) to the table `table_name` in the data
/// dictionary, with an Index object and a Key object per order.
///
/// # Safety
///
/// `table_name` and `index_file_path` must point to NUL-terminated strings;
/// `comment` must be null or point to one.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDAddIndexFile(
    dictionary: AdsHandle,
    table_name: *mut Unsigned8,
    index_file_path: *mut Unsigned8,
    comment: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDAddIndexFile");
    if table_name.is_null() || index_file_path.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [table, path, comment] =
        unsafe { [text(table_name), text(index_file_path), text(comment)] }
            .map(Option::unwrap_or_default);
    code_direct(api(dictionary).add_index_file(dictionary, table, path, comment))
}

/// The `property_len` bytes at `property`, or empty if null.
///
/// # Safety
///
/// `property` must be null or point to `property_len` readable bytes.
unsafe fn property_bytes(property: *const std::ffi::c_void, property_len: Unsigned16) -> Vec<u8> {
    // SAFETY: as the caller promises.
    let bytes = unsafe { crate::ffi::slice(property.cast::<u8>(), usize::from(property_len)) };
    bytes.map_or_else(Vec::new, <[u8]>::to_vec)
}

/// `AdsDDSetTableProperty`: sets a property of the table `table_name` in the
/// data dictionary. Only the comment (`ADS_DD_COMMENT`) is handled; other
/// properties are `AE_NOT_IMPLEMENTED`. The validation arguments are unused.
///
/// # Safety
///
/// `table_name` must point to a NUL-terminated string; `property` must be null
/// or point to `property_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDSetTableProperty(
    dictionary: AdsHandle,
    table_name: *mut Unsigned8,
    property_id: Unsigned16,
    property: *mut std::ffi::c_void,
    property_len: Unsigned16,
    _validate_option: Unsigned16,
    _fail_table: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDSetTableProperty");
    if table_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `table_name` is null or NUL-terminated, as # Safety says.
    let table = unsafe { text(table_name) }.unwrap_or_default();
    // The wide validation expression (ADS_DD_TABLE_VALIDATION_EXPR_W) takes a
    // UTF-16 value; the engine stores it as UTF-8 with a byte-order mark in the
    // same slot as the narrow validation expression.
    if property_id == 221 {
        if property.is_null() {
            return fail(AE_INVALID_POINTER);
        }
        // SAFETY: not null, so a NUL-terminated wide string, as # Safety says.
        let wide = unsafe { crate::wide_string(property.cast::<crate::Wchar>()) };
        let value = utf8_bom(&wide);
        return code_direct(api(dictionary).set_table_property(dictionary, table, 200, value));
    }
    // SAFETY: `property` is null or holds `property_len` bytes, as # Safety says.
    let value = unsafe { property_bytes(property, property_len) };
    code_direct(api(dictionary).set_table_property(dictionary, table, property_id, value))
}

/// `text` as the engine stores a wide dictionary property that keeps Unicode: a
/// UTF-8 byte-order mark, the UTF-8 bytes, and a NUL terminator.
fn utf8_bom(text: &str) -> Vec<u8> {
    let mut value = vec![0xEF, 0xBB, 0xBF];
    value.extend_from_slice(text.as_bytes());
    value.push(0);
    value
}

/// `AdsDDSetViewProperty`: sets a property of the view `view_name` in the data
/// dictionary. Only the comment (`ADS_DD_COMMENT`) is handled; other properties
/// are `AE_NOT_IMPLEMENTED`.
///
/// # Safety
///
/// `view_name` must point to a NUL-terminated string; `property` must be null
/// or point to `property_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDSetViewProperty(
    dictionary: AdsHandle,
    view_name: *mut Unsigned8,
    property_id: Unsigned16,
    property: *mut std::ffi::c_void,
    property_len: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDSetViewProperty");
    if view_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `view_name` is null or NUL-terminated, as # Safety says.
    let view = unsafe { text(view_name) }.unwrap_or_default();
    // SAFETY: `property` is null or holds `property_len` bytes, as # Safety says.
    let value = unsafe { property_bytes(property, property_len) };
    code_direct(api(dictionary).set_view_property(dictionary, view, property_id, value))
}

/// `AdsDDSetFieldProperty`: sets a property of the field `field_name` of the
/// table `table_name` in the data dictionary. The comment and the string-valued
/// field properties are handled; other properties are `AE_NOT_IMPLEMENTED`. The
/// validation arguments are unused.
///
/// # Safety
///
/// `table_name` and `field_name` must point to NUL-terminated strings;
/// `property` must be null or point to `property_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDSetFieldProperty(
    dictionary: AdsHandle,
    table_name: *mut Unsigned8,
    field_name: *mut Unsigned8,
    property_id: Unsigned16,
    property: *mut std::ffi::c_void,
    property_len: Unsigned16,
    _validate_option: Unsigned16,
    _fail_table: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDSetFieldProperty");
    if table_name.is_null() || field_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [table, field] =
        unsafe { [text(table_name), text(field_name)] }.map(Option::unwrap_or_default);
    // The wide field properties (default, minimum, maximum) take a UTF-16
    // value; the engine stores it in the connection's ANSI code page, as the
    // matching narrow property does, and fails with AE_UNICODE_CONVERSION on a
    // character the code page lacks.
    if let Some(narrow_id) = wide_field_property(property_id) {
        if property.is_null() {
            return fail(AE_INVALID_POINTER);
        }
        // SAFETY: not null, so a NUL-terminated wide string, as # Safety says.
        let wide = unsafe { crate::wide_string(property.cast::<crate::Wchar>()) };
        let Some(mut value) = ansi::from_os(std::ffi::OsStr::new(&wide)) else {
            return fail(crate::AE_UNICODE_CONVERSION);
        };
        value.push(0);
        return code(
            api(dictionary).set_field_property(dictionary, table, field, narrow_id, value),
        );
    }
    // SAFETY: `property` is null or holds `property_len` bytes, as # Safety says.
    let value = unsafe { property_bytes(property, property_len) };
    code_direct(api(dictionary).set_field_property(dictionary, table, field, property_id, value))
}

/// The narrow field property a wide one (`…_W`) stores as: `ADS_DD_FIELD_*_W`
/// (311–313) map to their `ADS_DD_FIELD_*` counterparts (300, 302, 303).
fn wide_field_property(property_id: Unsigned16) -> Option<Unsigned16> {
    match property_id {
        311 => Some(300), // ADS_DD_FIELD_DEFAULT_VALUE_W -> ADS_DD_FIELD_DEFAULT_VALUE
        312 => Some(302), // ADS_DD_FIELD_MIN_VALUE_W     -> ADS_DD_FIELD_MIN_VALUE
        313 => Some(303), // ADS_DD_FIELD_MAX_VALUE_W     -> ADS_DD_FIELD_MAX_VALUE
        _ => None,
    }
}

/// `AdsDDSetIndexProperty`: sets a property of the index order `index_name` of
/// the table `table_name` in the data dictionary. Only the comment
/// (`ADS_DD_COMMENT`) is handled; other properties are `AE_NOT_IMPLEMENTED`.
///
/// # Safety
///
/// `table_name` and `index_name` must point to NUL-terminated strings;
/// `property` must be null or point to `property_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDSetIndexProperty(
    connection: AdsHandle,
    table_name: *mut Unsigned8,
    index_name: *mut Unsigned8,
    property_id: Unsigned16,
    property: *mut std::ffi::c_void,
    property_len: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDSetIndexProperty");
    if table_name.is_null() || index_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [table, index] =
        unsafe { [text(table_name), text(index_name)] }.map(Option::unwrap_or_default);
    // SAFETY: `property` is null or holds `property_len` bytes, as # Safety says.
    let value = unsafe { property_bytes(property, property_len) };
    code_direct(api(connection).set_index_property(connection, table, index, property_id, value))
}

/// `AdsDDSetDatabaseProperty`: sets a property of the data dictionary's database
/// object. The comment and the customer version numbers are handled; other
/// properties are `AE_NOT_IMPLEMENTED`.
///
/// # Safety
///
/// `property` must be null or point to `property_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDSetDatabaseProperty(
    dictionary: AdsHandle,
    property_id: Unsigned16,
    property: *mut std::ffi::c_void,
    property_len: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDSetDatabaseProperty");
    // SAFETY: `property` is null or holds `property_len` bytes, as # Safety says.
    let value = unsafe { property_bytes(property, property_len) };
    code_direct(api(dictionary).set_database_property(dictionary, property_id, value))
}

/// `AdsDDSetUserProperty`: sets a property of the user `user_name`. The comment,
/// the user-defined property and the require-old-password flag are handled; other
/// properties are `AE_NOT_IMPLEMENTED`.
///
/// # Safety
///
/// `user_name` must point to a NUL-terminated string; `property` must be null or
/// point to `property_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDSetUserProperty(
    dictionary: AdsHandle,
    user_name: *mut Unsigned8,
    property_id: Unsigned16,
    property: *mut std::ffi::c_void,
    property_len: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDSetUserProperty");
    if user_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `user_name` is null or NUL-terminated, as # Safety says.
    let user = unsafe { text(user_name) }.unwrap_or_default();
    // SAFETY: `property` is null or holds `property_len` bytes, as # Safety says.
    let value = unsafe { property_bytes(property, property_len) };
    code_direct(api(dictionary).set_user_property(dictionary, user, property_id, value))
}

/// `AdsDDSetUserGroupProperty`: sets a property of the user group
/// `user_group_name`. The comment and the user-defined property are handled;
/// other properties are `AE_NOT_IMPLEMENTED`.
///
/// # Safety
///
/// `user_group_name` must point to a NUL-terminated string; `property` must be
/// null or point to `property_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDSetUserGroupProperty(
    dictionary: AdsHandle,
    user_group_name: *mut Unsigned8,
    property_id: Unsigned16,
    property: *mut std::ffi::c_void,
    property_len: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDSetUserGroupProperty");
    if user_group_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `user_group_name` is null or NUL-terminated, as # Safety says.
    let group = unsafe { text(user_group_name) }.unwrap_or_default();
    // SAFETY: `property` is null or holds `property_len` bytes, as # Safety says.
    let value = unsafe { property_bytes(property, property_len) };
    code_direct(api(dictionary).set_user_group_property(dictionary, group, property_id, value))
}

/// `AdsDDSetProcedureProperty`: sets a property of the stored procedure
/// `procedure_name`. The comment, the parameter lists, the DLL name and function,
/// the invoke option and the script (narrow and wide) are handled; other
/// properties are `AE_NOT_IMPLEMENTED`.
///
/// # Safety
///
/// `procedure_name` must point to a NUL-terminated string; `property` must be
/// null or point to `property_len` bytes (a UTF-16 string for the wide script).
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDSetProcedureProperty(
    dictionary: AdsHandle,
    procedure_name: *mut Unsigned8,
    property_id: Unsigned16,
    property: *mut std::ffi::c_void,
    property_len: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDSetProcedureProperty");
    if procedure_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `procedure_name` is null or NUL-terminated, as # Safety says.
    let procedure = unsafe { text(procedure_name) }.unwrap_or_default();
    // The wide script (ADS_DD_PROC_SCRIPT_W) takes a UTF-16 value; the engine
    // stores it as UTF-8 with a byte-order mark in the same slot as the narrow
    // script, as it does a view's or table's wide text.
    if property_id == 806 {
        if property.is_null() {
            return fail(AE_INVALID_POINTER);
        }
        // SAFETY: not null, so a NUL-terminated wide string, as # Safety says.
        let wide = unsafe { crate::wide_string(property.cast::<crate::Wchar>()) };
        let value = utf8_bom(&wide);
        return code_direct(
            api(dictionary).set_procedure_property(dictionary, procedure, 806, value),
        );
    }
    // SAFETY: `property` is null or holds `property_len` bytes, as # Safety says.
    let value = unsafe { property_bytes(property, property_len) };
    code_direct(api(dictionary).set_procedure_property(dictionary, procedure, property_id, value))
}

/// `AdsDDFreeTable`: frees the table at `table_name` from its data dictionary,
/// clearing the binding in the table's own file so it becomes a free table.
/// `password` is for an encrypted table (not used yet).
///
/// # Safety
///
/// `table_name` must point to a NUL-terminated string; `password` must be null
/// or point to one.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDFreeTable(
    table_name: *mut Unsigned8,
    password: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDFreeTable");
    if table_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [path, password] =
        unsafe { [text(table_name), text(password)] }.map(Option::unwrap_or_default);
    code(tflip_protocol::Api::free_table(
        &*crate::LOCAL,
        path,
        password,
    ))
}

/// `AdsDDRemoveView`: removes the view `name` from the data dictionary.
///
/// # Safety
///
/// `name` must point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDRemoveView(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDRemoveView");
    if name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `name` is null or NUL-terminated, as # Safety says.
    let name = unsafe { text(name) }.unwrap_or_default();
    code_direct(api(dictionary).remove_view(dictionary, name))
}

/// `AdsDDRemoveProcedure`: removes the stored procedure `name` from the data
/// dictionary.
///
/// # Safety
///
/// `name` must point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDRemoveProcedure(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDRemoveProcedure");
    if name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `name` is null or NUL-terminated, as # Safety says.
    let name = unsafe { text(name) }.unwrap_or_default();
    code_direct(api(dictionary).remove_procedure(dictionary, name))
}

/// Adds a trigger to a data dictionary (see [`AdsDDCreateTrigger`]); `None`
/// for a null pointer. `container` is already read from its ANSI or wide
/// form.
#[allow(clippy::too_many_arguments)]
fn create_trigger(
    dictionary: AdsHandle,
    name: Option<String>,
    table_name: Option<String>,
    trigger_type: Unsigned32,
    event_types: Unsigned32,
    container_type: Unsigned32,
    container: String,
    function_name: Option<String>,
    priority: Unsigned32,
    comments: Option<String>,
    options: Unsigned32,
) -> Unsigned32 {
    let (Some(name), Some(table)) = (name, table_name) else {
        return fail(AE_INVALID_POINTER);
    };
    code(api(dictionary).add_trigger(
        dictionary,
        name,
        table,
        trigger_type,
        event_types,
        container_type,
        container,
        function_name.unwrap_or_default(),
        priority,
        comments.unwrap_or_default(),
        options,
    ))
}

/// `AdsDDCreateTrigger`: adds a trigger named `name` on the table `table_name` to
/// the data dictionary. `trigger_type` and `event_types` say when it fires,
/// `container_type` whether `container` is a SQL script or an external library,
/// `function_name` the function in that library, `priority`, `comments` and
/// `options` the rest. Stored as the original engine stores it.
///
/// # Safety
///
/// `name`, `table_name` and `container` must point to NUL-terminated strings; the
/// rest must be null or point to one.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn AdsDDCreateTrigger(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
    table_name: *mut Unsigned8,
    trigger_type: Unsigned32,
    event_types: Unsigned32,
    container_type: Unsigned32,
    container: *mut Unsigned8,
    function_name: *mut Unsigned8,
    priority: Unsigned32,
    comments: *mut Unsigned8,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDCreateTrigger");
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [container, name, table_name, function_name, comments] = unsafe {
        [
            text(container),
            text(name),
            text(table_name),
            text(function_name),
            text(comments),
        ]
    };
    let Some(container) = container else {
        return fail(AE_INVALID_POINTER);
    };
    create_trigger(
        dictionary,
        name,
        table_name,
        trigger_type,
        event_types,
        container_type,
        container,
        function_name,
        priority,
        comments,
        options,
    )
}

/// `AdsDDCreateTrigger100`: [`AdsDDCreateTrigger`] with the container as wide
/// characters.
///
/// # Safety
///
/// `name` and `table_name` must point to NUL-terminated strings, `container` to a
/// NUL-terminated wide string; the rest must be null or NUL-terminated strings.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn AdsDDCreateTrigger100(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
    table_name: *mut Unsigned8,
    trigger_type: Unsigned32,
    event_types: Unsigned32,
    container_type: Unsigned32,
    container: *mut crate::Wchar,
    function_name: *mut Unsigned8,
    priority: Unsigned32,
    comments: *mut Unsigned8,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDCreateTrigger100");
    if container.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are NUL-terminated, or null where # Safety allows.
    let (container, [name, table_name, function_name, comments]) = unsafe {
        (
            crate::wide_string(container),
            [
                text(name),
                text(table_name),
                text(function_name),
                text(comments),
            ],
        )
    };
    create_trigger(
        dictionary,
        name,
        table_name,
        trigger_type,
        event_types,
        container_type,
        container,
        function_name,
        priority,
        comments,
        options,
    )
}

/// `AdsDDRemoveTrigger`: removes the trigger `name` from the data dictionary,
/// recomputing its table's aggregate trigger slots. The original engine's own
/// version fails with 5132 for a trigger it wrote; tflip removes it correctly.
///
/// # Safety
///
/// `name` must point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDRemoveTrigger(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDRemoveTrigger");
    if name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `name` is null or NUL-terminated, as # Safety says.
    let name = unsafe { text(name) }.unwrap_or_default();
    code_direct(api(dictionary).remove_trigger(dictionary, name))
}

/// Adds a referential-integrity rule to a data dictionary (see
/// [`AdsDDCreateRefIntegrity`]); `None` for a null pointer. `no_primary_error`
/// and `cascade_error` are the error messages (none for the non-62 form). The
/// failed-rows table is not kept.
///
/// `names` are the rule's name, the parent table and tag, and the child table
/// and tag.
fn create_refintegrity(
    dictionary: AdsHandle,
    names: [Option<String>; 5],
    update_rule: Unsigned16,
    delete_rule: Unsigned16,
    no_primary_error: Option<String>,
    cascade_error: Option<String>,
) -> Unsigned32 {
    let [
        Some(name),
        Some(parent_table),
        Some(parent_tag),
        Some(child_table),
        Some(child_tag),
    ] = names
    else {
        return fail(AE_INVALID_POINTER);
    };
    code(api(dictionary).add_refintegrity(
        dictionary,
        name,
        parent_table,
        parent_tag,
        child_table,
        child_tag,
        update_rule as u32,
        delete_rule as u32,
        no_primary_error.unwrap_or_default(),
        cascade_error.unwrap_or_default(),
    ))
}

/// `AdsDDCreateRefIntegrity`: adds a referential-integrity rule named `ri_name`
/// between the parent table's tag and the child table's tag, with the update and
/// delete rules. Stored as the original engine stores it.
///
/// # Safety
///
/// `ri_name`, the table names and the tag names must point to NUL-terminated
/// strings; `fail_table` must be null or point to one.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn AdsDDCreateRefIntegrity(
    dictionary: AdsHandle,
    ri_name: *mut Unsigned8,
    _fail_table: *mut Unsigned8,
    parent_table: *mut Unsigned8,
    parent_tag: *mut Unsigned8,
    child_table: *mut Unsigned8,
    child_tag: *mut Unsigned8,
    update_rule: Unsigned16,
    delete_rule: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDCreateRefIntegrity");
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let names = unsafe {
        [
            text(ri_name),
            text(parent_table),
            text(parent_tag),
            text(child_table),
            text(child_tag),
        ]
    };
    create_refintegrity(dictionary, names, update_rule, delete_rule, None, None)
}

/// `AdsDDCreateRefIntegrity62`: [`AdsDDCreateRefIntegrity`] with the no-primary
/// and cascade error messages.
///
/// # Safety
///
/// As for [`AdsDDCreateRefIntegrity`]; `no_primary_error` and `cascade_error`
/// must be null or point to NUL-terminated strings.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn AdsDDCreateRefIntegrity62(
    dictionary: AdsHandle,
    ri_name: *mut Unsigned8,
    _fail_table: *mut Unsigned8,
    parent_table: *mut Unsigned8,
    parent_tag: *mut Unsigned8,
    child_table: *mut Unsigned8,
    child_tag: *mut Unsigned8,
    update_rule: Unsigned16,
    delete_rule: Unsigned16,
    no_primary_error: *mut Unsigned8,
    cascade_error: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDCreateRefIntegrity62");
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let (names, no_primary_error, cascade_error) = unsafe {
        (
            [
                text(ri_name),
                text(parent_table),
                text(parent_tag),
                text(child_table),
                text(child_tag),
            ],
            text(no_primary_error),
            text(cascade_error),
        )
    };
    create_refintegrity(
        dictionary,
        names,
        update_rule,
        delete_rule,
        no_primary_error,
        cascade_error,
    )
}

/// `AdsDDRemoveRefIntegrity`: removes the referential-integrity rule `ri_name`
/// from the data dictionary.
///
/// # Safety
///
/// `ri_name` must point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDRemoveRefIntegrity(
    dictionary: AdsHandle,
    ri_name: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDRemoveRefIntegrity");
    if ri_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `ri_name` is null or NUL-terminated, as # Safety says.
    let name = unsafe { text(ri_name) }.unwrap_or_default();
    code_direct(api(dictionary).remove_refintegrity(dictionary, name))
}

/// `AdsDDCreateLink`: adds a link named `alias` to the data dictionary at
/// `linked_path`, with the `user`/`password` credentials and the `options`.
/// Stored as the original engine stores it.
///
/// # Safety
///
/// `alias` and `linked_path` must point to NUL-terminated strings; `user` and
/// `password` must be null or point to one.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDCreateLink(
    connection: AdsHandle,
    alias: *mut Unsigned8,
    linked_path: *mut Unsigned8,
    user: *mut Unsigned8,
    password: *mut Unsigned8,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDCreateLink");
    if alias.is_null() || linked_path.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [alias, path, user, password] =
        unsafe { [text(alias), text(linked_path), text(user), text(password)] }
            .map(Option::unwrap_or_default);
    match api(connection).add_link(connection, alias, path, user, password, options) {
        Ok(()) => AE_SUCCESS,
        // The engine's refusals are its own codes here, not SQL's 7200
        // (probed, `fixtures/sql/links.json`).
        Err(tflip_engine::Error::Sql(sql)) if sql.state == "HY000" && sql.native >= 5000 => {
            let text = sql.message.strip_prefix("[ASA] ").unwrap_or(&sql.message);
            crate::last_error::fail_text(sql.native, text.to_owned())
        }
        Err(error) => error_code(error),
    }
}

/// `AdsDDDropLink`: removes the link `linked_dd` (its name or path) from the data
/// dictionary. `drop_global` (removing the reverse link) is not acted on.
///
/// # Safety
///
/// `linked_dd` must point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDDropLink(
    connection: AdsHandle,
    linked_dd: *mut Unsigned8,
    _drop_global: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDDropLink");
    if linked_dd.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `linked_dd` is null or NUL-terminated, as # Safety says.
    let alias = unsafe { text(linked_dd) }.unwrap_or_default();
    code(api(connection).drop_link(connection, alias))
}

/// `AdsDDModifyLink`: rewrites the link `alias`'s linked-dictionary path,
/// credentials and options. The original engine's own version fails with 5132
/// for a link it wrote; tflip performs the modification correctly.
///
/// # Safety
///
/// `alias` and `linked_path` must point to NUL-terminated strings; `user` and
/// `password` must be null or point to one.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDModifyLink(
    connection: AdsHandle,
    alias: *mut Unsigned8,
    linked_path: *mut Unsigned8,
    user: *mut Unsigned8,
    password: *mut Unsigned8,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDModifyLink");
    if alias.is_null() || linked_path.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [alias, path, user, password] =
        unsafe { [text(alias), text(linked_path), text(user), text(password)] }
            .map(Option::unwrap_or_default);
    code(api(connection).modify_link(connection, alias, path, user, password, options))
}

/// `AdsDDMoveObjectFile`: moves a table's files (and the index files named in
/// `index_files`, `;`-separated), or a single index file (`object_type` 3), to
/// the directory `new_path`, updating the dictionary — as the original engine
/// does.
///
/// # Safety
///
/// `object_name` and `new_path` must point to NUL-terminated strings;
/// `index_files` and `parent` must be null or point to one.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn AdsDDMoveObjectFile(
    connection: AdsHandle,
    object_type: Unsigned16,
    object_name: *mut Unsigned8,
    new_path: *mut Unsigned8,
    index_files: *mut Unsigned8,
    parent: *mut Unsigned8,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDMoveObjectFile");
    if object_name.is_null() || new_path.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [object_name, new_path, index_files, parent] = unsafe {
        [
            text(object_name),
            text(new_path),
            text(index_files),
            text(parent),
        ]
    }
    .map(Option::unwrap_or_default);
    code(api(connection).move_object_file(
        connection,
        object_type as u32,
        object_name,
        new_path,
        index_files,
        parent,
        options,
    ))
}

/// `AdsDDCreatePublication`: creates the replication publication `name` in the
/// data dictionary, with the comment `comments` (null for none) and the create
/// options `options` (0), as the original engine does.
///
/// # Safety
///
/// `name` must point to a NUL-terminated string; `comments` must be null or
/// point to one.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDCreatePublication(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
    comments: *mut Unsigned8,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDCreatePublication");
    if name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [name, comments] = unsafe { [text(name), text(comments)] }.map(Option::unwrap_or_default);
    code_direct(api(dictionary).create_publication(dictionary, name, comments, options))
}

/// `AdsDDDeletePublication`: removes the replication publication `name` from the
/// data dictionary, with every article under it.
///
/// # Safety
///
/// `name` must point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDDeletePublication(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDDeletePublication");
    if name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `name` is null or NUL-terminated, as # Safety says.
    let name = unsafe { text(name) }.unwrap_or_default();
    code_direct(api(dictionary).delete_publication(dictionary, name))
}

/// `AdsDDSetPublicationProperty`: sets the property `property_id` of the
/// replication publication `name` to the `property_len` bytes at `property`.
///
/// # Safety
///
/// `name` must point to a NUL-terminated string; `property` must be null or hold
/// `property_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDSetPublicationProperty(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
    property_id: Unsigned16,
    property: *mut std::ffi::c_void,
    property_len: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDSetPublicationProperty");
    if name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `name` is null or NUL-terminated, as # Safety says.
    let name = unsafe { text(name) }.unwrap_or_default();
    // SAFETY: `property` is null or holds `property_len` bytes, as # Safety says.
    let value = unsafe { property_bytes(property, property_len) };
    code_direct(api(dictionary).set_publication_property(dictionary, name, property_id, value))
}

/// `AdsDDCreateArticle`: creates a replication article publishing `object_name`
/// in `publication_name`, with the row-identifying columns `row_ident_columns`
/// (null to use `options`), the row filter `filter` (null for none) and the
/// identify `options`, as the original engine does.
///
/// # Safety
///
/// `publication_name` and `object_name` must point to NUL-terminated strings;
/// `row_ident_columns` and `filter` must be null or point to ones.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDCreateArticle(
    dictionary: AdsHandle,
    publication_name: *mut Unsigned8,
    object_name: *mut Unsigned8,
    row_ident_columns: *mut Unsigned8,
    filter: *mut Unsigned8,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDCreateArticle");
    if publication_name.is_null() || object_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [publication, object, row_ident, filter] = unsafe {
        [
            text(publication_name),
            text(object_name),
            text(row_ident_columns),
            text(filter),
        ]
    }
    .map(Option::unwrap_or_default);
    code_direct(api(dictionary).create_article(
        dictionary,
        publication,
        object,
        row_ident,
        filter,
        options,
        false,
    ))
}

/// `AdsDDCreateArticle100`: as [`AdsDDCreateArticle`], but the filter is a wide
/// (UTF-16) string.
///
/// # Safety
///
/// `publication_name` and `object_name` must point to NUL-terminated strings;
/// `row_ident_columns` must be null or point to one; `filter` must be null or
/// point to a NUL-terminated wide string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDCreateArticle100(
    dictionary: AdsHandle,
    publication_name: *mut Unsigned8,
    object_name: *mut Unsigned8,
    row_ident_columns: *mut Unsigned8,
    filter: *mut crate::Wchar,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDCreateArticle100");
    if publication_name.is_null() || object_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [publication, object, row_ident] = unsafe {
        [
            text(publication_name),
            text(object_name),
            text(row_ident_columns),
        ]
    }
    .map(Option::unwrap_or_default);
    let filter = if filter.is_null() {
        String::new()
    } else {
        // SAFETY: not null, so a NUL-terminated wide string, as # Safety says.
        unsafe { crate::wide_string(filter) }
    };
    code_direct(api(dictionary).create_article(
        dictionary,
        publication,
        object,
        row_ident,
        filter,
        options,
        true,
    ))
}

/// `AdsDDDeleteArticle`: removes the article publishing `object_name` from
/// `publication_name`.
///
/// # Safety
///
/// `publication_name` and `object_name` must point to NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDDeleteArticle(
    dictionary: AdsHandle,
    publication_name: *mut Unsigned8,
    object_name: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDDeleteArticle");
    if publication_name.is_null() || object_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [publication, object] =
        unsafe { [text(publication_name), text(object_name)] }.map(Option::unwrap_or_default);
    code_direct(api(dictionary).delete_article(dictionary, publication, object))
}

/// `AdsDDSetArticleProperty`: sets the property `property_id` of the article
/// publishing `object_name` in `publication_name` to the `property_len` bytes at
/// `property`.
///
/// # Safety
///
/// `publication_name` and `object_name` must point to NUL-terminated strings;
/// `property` must be null or hold `property_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDSetArticleProperty(
    dictionary: AdsHandle,
    publication_name: *mut Unsigned8,
    object_name: *mut Unsigned8,
    property_id: Unsigned16,
    property: *mut std::ffi::c_void,
    property_len: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDSetArticleProperty");
    if publication_name.is_null() || object_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [publication, object] =
        unsafe { [text(publication_name), text(object_name)] }.map(Option::unwrap_or_default);
    // SAFETY: `property` is null or holds `property_len` bytes, as # Safety says.
    let value = unsafe { property_bytes(property, property_len) };
    code_direct(api(dictionary).set_article_property(
        dictionary,
        publication,
        object,
        property_id,
        value,
    ))
}

/// `AdsDDCreateSubscription`: creates the replication subscription `name` for
/// `publication_name`, replicating to `target` as `user`/`password` through the
/// replication queue `replication_queue`, with `forward`, the comment `comments`
/// and `options`. Synthesizes the replication queue table, as the engine does.
///
/// # Safety
///
/// `name`, `publication_name`, `target` and `replication_queue` must point to
/// NUL-terminated strings; `user`, `password` and `comments` must be null or
/// point to ones.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDCreateSubscription(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
    publication_name: *mut Unsigned8,
    target: *mut Unsigned8,
    user: *mut Unsigned8,
    password: *mut Unsigned8,
    replication_queue: *mut Unsigned8,
    forward: Unsigned16,
    comments: *mut Unsigned8,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDCreateSubscription");
    if name.is_null()
        || publication_name.is_null()
        || target.is_null()
        || replication_queue.is_null()
    {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [name, publication, target, user, password, queue, comments] = unsafe {
        [
            text(name),
            text(publication_name),
            text(target),
            text(user),
            text(password),
            text(replication_queue),
            text(comments),
        ]
    }
    .map(Option::unwrap_or_default);
    code_direct(api(dictionary).create_subscription(
        dictionary,
        name,
        publication,
        target,
        user,
        password,
        queue,
        forward,
        comments,
        options,
    ))
}

/// `AdsDDDeleteSubscription`: removes the replication subscription `name` and its
/// replication queue table.
///
/// # Safety
///
/// `name` must point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDDeleteSubscription(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDDeleteSubscription");
    if name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `name` is null or NUL-terminated, as # Safety says.
    let name = unsafe { text(name) }.unwrap_or_default();
    code_direct(api(dictionary).delete_subscription(dictionary, name))
}

/// `AdsDDSetSubscriptionProperty`: sets the property `property_id` of the
/// replication subscription `name` to the `property_len` bytes at `property`.
///
/// # Safety
///
/// `name` must point to a NUL-terminated string; `property` must be null or hold
/// `property_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDSetSubscriptionProperty(
    dictionary: AdsHandle,
    name: *mut Unsigned8,
    property_id: Unsigned16,
    property: *mut std::ffi::c_void,
    property_len: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDSetSubscriptionProperty");
    if name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `name` is null or NUL-terminated, as # Safety says.
    let name = unsafe { text(name) }.unwrap_or_default();
    // SAFETY: `property` is null or holds `property_len` bytes, as # Safety says.
    let value = unsafe { property_bytes(property, property_len) };
    code_direct(api(dictionary).set_subscription_property(dictionary, name, property_id, value))
}

/// `AdsDDCreateUserGroup`: creates a user group named `group_name` in the data
/// dictionary `dictionary`, with the description `description` (null for none),
/// as the original engine does.
///
/// # Safety
///
/// `group_name` must point to a NUL-terminated string; `description` must be
/// null or point to one.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDCreateUserGroup(
    dictionary: AdsHandle,
    group_name: *mut Unsigned8,
    description: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDCreateUserGroup");
    if group_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [name, description] =
        unsafe { [text(group_name), text(description)] }.map(Option::unwrap_or_default);
    code_direct(api(dictionary).create_group(dictionary, name, description))
}

/// `AdsDDDeleteUserGroup`: deletes the user group `group_name` from the data
/// dictionary `dictionary`, as the original engine does for a group with no
/// members.
///
/// # Safety
///
/// `group_name` must point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDDeleteUserGroup(
    dictionary: AdsHandle,
    group_name: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDDeleteUserGroup");
    if group_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `group_name` is null or NUL-terminated, as # Safety says.
    let name = unsafe { text(group_name) }.unwrap_or_default();
    code_direct(api(dictionary).delete_group(dictionary, name))
}

/// `AdsDDCreateUser`: creates a user named `user_name` in the data dictionary
/// `dictionary`, with the password `password`, the description `description`
/// (null for none), and adds it to the group `group_name` (null or empty for
/// the built-in `DB:Public` only), as the original engine does.
///
/// # Safety
///
/// `user_name` must point to a NUL-terminated string; `group_name`, `password`
/// and `description` must be null or point to one.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDCreateUser(
    dictionary: AdsHandle,
    group_name: *mut Unsigned8,
    user_name: *mut Unsigned8,
    password: *mut Unsigned8,
    description: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDCreateUser");
    if user_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [group, name, password, description] = unsafe {
        [
            text(group_name),
            text(user_name),
            text(password),
            text(description),
        ]
    }
    .map(Option::unwrap_or_default);
    code_direct(api(dictionary).create_user(dictionary, group, name, password, description))
}

/// `AdsDDDeleteUser`: deletes the user `user_name` from the data dictionary
/// `dictionary`, as the original engine does.
///
/// # Safety
///
/// `user_name` must point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDDeleteUser(
    dictionary: AdsHandle,
    user_name: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDDeleteUser");
    if user_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `user_name` is null or NUL-terminated, as # Safety says.
    let name = unsafe { text(user_name) }.unwrap_or_default();
    code_direct(api(dictionary).delete_user(dictionary, name))
}

/// `AdsDDAddUserToGroup`: adds the user `user_name` to the group `group_name` in
/// the data dictionary `dictionary`, as the original engine does.
///
/// # Safety
///
/// `group_name` and `user_name` must point to NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDAddUserToGroup(
    dictionary: AdsHandle,
    group_name: *mut Unsigned8,
    user_name: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDAddUserToGroup");
    if group_name.is_null() || user_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [group, user] =
        unsafe { [text(group_name), text(user_name)] }.map(Option::unwrap_or_default);
    code_direct(api(dictionary).add_user_to_group(dictionary, group, user))
}

/// `AdsDDRemoveUserFromGroup`: removes the user `user_name` from the group
/// `group_name` in the data dictionary `dictionary`, as the original engine does.
///
/// # Safety
///
/// `group_name` and `user_name` must point to NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDRemoveUserFromGroup(
    dictionary: AdsHandle,
    group_name: *mut Unsigned8,
    user_name: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDRemoveUserFromGroup");
    if group_name.is_null() || user_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [group, user] =
        unsafe { [text(group_name), text(user_name)] }.map(Option::unwrap_or_default);
    code_direct(api(dictionary).remove_user_from_group(dictionary, group, user))
}

/// `AdsDDGrantPermission`: grants `permissions` on the object `object_name` (of
/// `object_type`, under `parent_name`) to `grantee` in the data dictionary
/// `admin_conn`, as the original engine does (every object type but 0).
///
/// # Safety
///
/// `grantee` must point to a NUL-terminated string; `object_name` and
/// `parent_name` must each be null or point to one.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDGrantPermission(
    admin_conn: AdsHandle,
    object_type: Unsigned16,
    object_name: *mut Unsigned8,
    parent_name: *mut Unsigned8,
    grantee: *mut Unsigned8,
    permissions: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDGrantPermission");
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [object_name, parent_name, grantee] =
        unsafe { [text(object_name), text(parent_name), text(grantee)] };
    change_permission(
        admin_conn,
        object_type,
        object_name,
        parent_name,
        grantee,
        permissions,
        false,
    )
}

/// `AdsDDRevokePermission`: revokes `permissions` on `object_name` from
/// `grantee` in the data dictionary `admin_conn`.
///
/// # Safety
///
/// `grantee` must point to a NUL-terminated string; `object_name` and
/// `parent_name` must each be null or point to one.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDRevokePermission(
    admin_conn: AdsHandle,
    object_type: Unsigned16,
    object_name: *mut Unsigned8,
    parent_name: *mut Unsigned8,
    grantee: *mut Unsigned8,
    permissions: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDRevokePermission");
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [object_name, parent_name, grantee] =
        unsafe { [text(object_name), text(parent_name), text(grantee)] };
    change_permission(
        admin_conn,
        object_type,
        object_name,
        parent_name,
        grantee,
        permissions,
        true,
    )
}

/// The shared body of [`AdsDDGrantPermission`] and [`AdsDDRevokePermission`];
/// `None` for a null pointer.
fn change_permission(
    admin_conn: AdsHandle,
    object_type: Unsigned16,
    object_name: Option<String>,
    parent_name: Option<String>,
    grantee: Option<String>,
    permissions: Unsigned32,
    revoke: bool,
) -> Unsigned32 {
    let Some(grantee) = grantee else {
        return fail(AE_INVALID_POINTER);
    };
    // A null object name where one is needed is refused before the dictionary is
    // touched (probed, `tools/ace/dd/grants.c`): not for the database (which has
    // none), the right to create (which takes none) or `ALL` (which looks the
    // empty name up, and finds nothing).
    let base = permissions & !0x8000_0000;
    let all = permissions == 0xFFFF_FFFF || permissions == 0x8FFF_FFFF;
    let named = matches!(
        object_type,
        0 | 1 | 4 | 6 | 8 | 9 | 10 | 12 | 15 | 17 | 18 | 19
    );
    if object_name.is_none() && named && base != 0 && base & 0x80 == 0 && !all {
        return crate::last_error::fail_text(
            5132,
            "Error 5132:  The specified database object name is not valid.  The object name \
             cannot be empty."
                .to_owned(),
        );
    }
    let object = object_name.unwrap_or_default();
    let parent = parent_name.unwrap_or_default();
    let result = if revoke {
        api(admin_conn).revoke_permission(
            admin_conn,
            object_type,
            object,
            parent,
            grantee,
            permissions,
        )
    } else {
        api(admin_conn).grant_permission(
            admin_conn,
            object_type,
            object,
            parent,
            grantee,
            permissions,
        )
    };
    match result {
        Ok(()) => AE_SUCCESS,
        Err(error) => direct_error_code(error),
    }
}

/// [`code`] where an engine refusal numbered 5xxx is the call's own return code and
/// text, as the original's calls that add an object refuse a user without the
/// right (`fixtures/sql/ddbuiltin`).
fn code_direct(result: Result<(), tflip_engine::Error>) -> Unsigned32 {
    match result {
        Ok(()) => AE_SUCCESS,
        Err(error) => direct_error_code(error),
    }
}

/// The code of a dictionary error the original engine returns as itself, not
/// wrapped in SQL error 7200: `AdsDDGrantPermission`/`AdsDDRevokePermission`
/// answer 5132, 5136 and 5158 directly (probed, `tools/ace/dd/pubperm.c`), as
/// do the calls of `fixtures/sql/ddapirights`.
pub(crate) fn direct_error_code(error: tflip_engine::Error) -> Unsigned32 {
    match error {
        // 7041 is `AdsDDAddIndexFile`'s for an index file that is not there.
        tflip_engine::Error::Sql(sql)
            if (5000..6000).contains(&sql.native) || sql.native == 7041 =>
        {
            let text = sql.message.strip_prefix("[ASA] ").unwrap_or(&sql.message);
            crate::last_error::fail_text(sql.native, text.to_owned())
        }
        error => error_code(error),
    }
}

/// `AdsDDSetObjectAccessRights`: sets the accessor `accessor_name`'s rights on the
/// object `object_name` to exactly those the access string `allowed_access`
/// names (`R` read, `W` write, `X` execute, `I` inherit; other letters ignored),
/// replacing whatever was there. The object's type is inferred from its name.
///
/// # Safety
///
/// `object_name`, `accessor_name` and `allowed_access` must point to
/// NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDSetObjectAccessRights(
    dictionary: AdsHandle,
    object_name: *mut Unsigned8,
    accessor_name: *mut Unsigned8,
    allowed_access: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDSetObjectAccessRights");
    if object_name.is_null() || accessor_name.is_null() || allowed_access.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [object, accessor, access] =
        unsafe { [text(object_name), text(accessor_name), text(allowed_access)] }
            .map(Option::unwrap_or_default);
    code(api(dictionary).set_object_access_rights(dictionary, object, accessor, access))
}

/// `AdsDDGetPermissions`: writes to `permissions` the mask `grantee` holds on the
/// object `object_name` (of `object_type`, under `parent_name` for a column) in
/// the data dictionary `dbconn`. A non-zero `get_inherited` adds the grants of the
/// groups a user belongs to. The mask carries `ADS_PERMISSION_INHERIT` for a user
/// that holds it.
///
/// # Safety
///
/// `grantee` must point to a NUL-terminated string; `object_name` and
/// `parent_name` must be null or point to one; `permissions` must be null or
/// valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDGetPermissions(
    dbconn: AdsHandle,
    grantee: *mut Unsigned8,
    object_type: Unsigned16,
    object_name: *mut Unsigned8,
    parent_name: *mut Unsigned8,
    get_inherited: Unsigned16,
    permissions: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDGetPermissions");
    if grantee.is_null() {
        return crate::last_error::fail_detail(
            AE_INVALID_POINTER,
            "Parameter 2 is not a valid string.",
        );
    }
    if permissions.is_null() {
        return crate::last_error::null_param(7);
    }
    // A null object name finds nothing (5132), as the original answers.
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [grantee, object, parent] =
        unsafe { [text(grantee), text(object_name), text(parent_name)] }
            .map(Option::unwrap_or_default);
    match api(dbconn).get_permissions(
        dbconn,
        grantee.clone(),
        object_type,
        object,
        parent,
        get_inherited != 0,
    ) {
        // A connection with no user may not read what a group holds
        // (`fixtures/errors/param_n`).
        Ok(_) if crate::dictionary::anonymous(dbconn) && is_group(dbconn, &grantee) => {
            fail(crate::AE_PERMISSION_DENIED)
        }
        Ok(mask) => {
            // SAFETY: not null, so writable, as # Safety says.
            unsafe { *permissions = mask };
            AE_SUCCESS
        }
        Err(error) => error_code(error),
    }
}

/// Whether `name` is a group the dictionary of `connection` has, not one of
/// the built-in ones.
fn is_group(connection: AdsHandle, name: &str) -> bool {
    let group = tflip_engine::dictionary::ObjectRef::Group(name.to_owned());
    // A group with no comment is still there.
    !name.starts_with("DB:")
        && !matches!(
            api(connection).dd_property(connection, group, 1),
            Err(tflip_engine::Error::InvalidObjectName)
        )
}

/// `AdsDDRemoveTable`: removes the table `table_name` and all that belongs to it
/// from the data dictionary; a non-zero `delete_files` removes its files too.
///
/// # Safety
///
/// `table_name` must point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDRemoveTable(
    object: AdsHandle,
    table_name: *mut Unsigned8,
    delete_files: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDRemoveTable");
    if table_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `table_name` is null or NUL-terminated, as # Safety says.
    let name = unsafe { text(table_name) }.unwrap_or_default();
    code_direct(api(object).remove_table(object, name, delete_files))
}

/// `AdsDDRemoveIndexFile`: removes the index file `index_file_name` of the table
/// `table_name` from the data dictionary; a non-zero `delete_file` removes the
/// file too.
///
/// # Safety
///
/// `table_name` and `index_file_name` must point to NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDRemoveIndexFile(
    dictionary: AdsHandle,
    table_name: *mut Unsigned8,
    index_file_name: *mut Unsigned8,
    delete_file: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDRemoveIndexFile");
    if table_name.is_null() || index_file_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [table, index_file] =
        unsafe { [text(table_name), text(index_file_name)] }.map(Option::unwrap_or_default);
    code_direct(api(dictionary).remove_index_file(dictionary, table, index_file, delete_file))
}

/// `AdsDDDeleteIndex`: deletes the index order `index_name` of the table
/// `table_name` from the data dictionary.
///
/// # Safety
///
/// `table_name` and `index_name` must point to NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDDeleteIndex(
    dictionary: AdsHandle,
    table_name: *mut Unsigned8,
    index_name: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDDeleteIndex");
    if table_name.is_null() || index_name.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [table, index] =
        unsafe { [text(table_name), text(index_name)] }.map(Option::unwrap_or_default);
    code_direct(api(dictionary).dd_delete_index(dictionary, table, index))
}

/// `AdsDDRenameObject`: renames the dictionary object `object_name` of
/// `object_type` to `new_object_name`. A table takes its files with it unless
/// `options` is `ADS_KEEP_TABLE_FILE_NAME`; fields and indexes are not
/// renamed by the original either (`AE_INVALID_OBJECT_TYPE`).
///
/// # Safety
///
/// `object_name` and `new_object_name` must point to NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDRenameObject(
    dictionary: AdsHandle,
    object_name: *mut Unsigned8,
    new_object_name: *mut Unsigned8,
    object_type: Unsigned16,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDRenameObject");
    if object_name.is_null() {
        return crate::last_error::fail_detail(
            AE_INVALID_POINTER,
            "Parameter 2 is not a valid string.",
        );
    }
    if new_object_name.is_null() {
        return crate::last_error::fail_detail(
            AE_INVALID_POINTER,
            "Parameter 3 is not a valid string.",
        );
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [name, new_name] =
        unsafe { [text(object_name), text(new_object_name)] }.map(Option::unwrap_or_default);
    code(api(dictionary).rename_object(dictionary, name, new_name, object_type, options))
}

/// Creates an index order (see [`AdsCreateIndex90`]).
///
/// `strings` are the file, the tag, the expression and the condition, `None`
/// where null.
fn create_index(
    table: AdsHandle,
    strings: [Option<String>; 4],
    options: Unsigned32,
    page_size: Unsigned32,
    index: Option<&mut AdsHandle>,
) -> Unsigned32 {
    let [file, Some(tag), Some(expression), condition] = strings else {
        return fail(AE_INVALID_POINTER);
    };
    let Some(index) = index else {
        return fail(AE_INVALID_POINTER);
    };
    let file = file.filter(|f| !f.is_empty());
    let condition = condition.unwrap_or_default();
    // An OEM table's index holds its literals in the OEM code page, as the
    // original engine writes them.
    let (expression, condition) = match crate::filter::is_oem(table) {
        true => (
            ansi::os_text_to_oem(&expression),
            ansi::os_text_to_oem(&condition),
        ),
        false => (expression, condition),
    };
    match api(table).create_index(table, file, tag, expression, condition, options, page_size) {
        Ok(handle) => {
            *index = handle;
            AE_SUCCESS
        }
        Err(error) => error_code(error),
    }
}

/// `AdsCreateIndex`: [`AdsCreateIndex90`] with 512-byte pages.
///
/// # Safety
///
/// As for [`AdsCreateIndex90`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCreateIndex(
    table: AdsHandle,
    file: *mut Unsigned8,
    tag: *mut Unsigned8,
    expression: *mut Unsigned8,
    condition: *mut Unsigned8,
    _while: *mut Unsigned8,
    options: Unsigned32,
    index: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCreateIndex");
    // SAFETY: the pointers are as # Safety says.
    let (strings, index) = unsafe {
        let strings = [text(file), text(tag), text(expression), text(condition)];
        (strings, index.as_mut())
    };
    create_index(table, strings, options, 512, index)
}

/// `AdsCreateIndex61`: [`AdsCreateIndex90`] without a collation.
///
/// # Safety
///
/// As for [`AdsCreateIndex90`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCreateIndex61(
    table: AdsHandle,
    file: *mut Unsigned8,
    tag: *mut Unsigned8,
    expression: *mut Unsigned8,
    condition: *mut Unsigned8,
    _while: *mut Unsigned8,
    options: Unsigned32,
    page_size: Unsigned32,
    index: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCreateIndex61");
    // SAFETY: the pointers are as # Safety says.
    let (strings, index) = unsafe {
        let strings = [text(file), text(tag), text(expression), text(condition)];
        (strings, index.as_mut())
    };
    create_index(table, strings, options, page_size, index)
}

/// `AdsCreateIndex90`: creates the order `tag` on the table `table`, in the
/// index file `file` (its structural one if null or empty), with the key
/// `expression`, the `FOR` condition `condition`, `options` (`ADS_UNIQUE`,
/// `ADS_CUSTOM`, `ADS_DESCENDING`, `ADS_BINARY_INDEX`, ...) and pages of
/// `page_size` bytes; builds it from the table's records and opens it,
/// replacing an order of that name. A key that does not compile is its
/// error (3000 and up). The `WHILE` condition and the collation are not
/// used.
///
/// # Safety
///
/// `tag` and `expression` must point to NUL-terminated strings, the others
/// be null or point to them; `index` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCreateIndex90(
    table: AdsHandle,
    file: *mut Unsigned8,
    tag: *mut Unsigned8,
    expression: *mut Unsigned8,
    condition: *mut Unsigned8,
    _while: *mut Unsigned8,
    options: Unsigned32,
    page_size: Unsigned32,
    _collation: *mut Unsigned8,
    index: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCreateIndex90");
    // SAFETY: the pointers are as # Safety says.
    let (strings, index) = unsafe {
        let strings = [text(file), text(tag), text(expression), text(condition)];
        (strings, index.as_mut())
    };
    create_index(table, strings, options, page_size, index)
}

/// `AdsDeleteIndex`: deletes an index order from its file, and closes its
/// handle.
#[unsafe(no_mangle)]
pub extern "system" fn AdsDeleteIndex(index: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDeleteIndex");
    code(api(index).delete_index(index))
}

/// `AdsReindex`: rebuilds the index files open on a table, or the one of an
/// order, as the original engine does; the table must be open exclusively
/// (`AE_TABLE_NOT_EXCLUSIVE`).
#[unsafe(no_mangle)]
pub extern "system" fn AdsReindex(handle: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsReindex");
    if let Err(code) = crate::handle::reindexed(handle) {
        return code;
    }
    code(api(handle).reindex(handle))
}

/// `AdsReindex61`: [`AdsReindex`] with ADI files of `page_size` bytes, 512
/// to 8,192 or 0 for the files' own, checked before the table's open mode
/// (`fixtures/write/reindex_pages`).
#[unsafe(no_mangle)]
pub extern "system" fn AdsReindex61(handle: AdsHandle, page_size: Unsigned32) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsReindex61");
    if let Err(code) = crate::handle::reindexed(handle) {
        return code;
    }
    if page_size != 0 && !(512..=8192).contains(&page_size) {
        return crate::last_error::fail_detail(
            crate::AE_INVALID_OPTION,
            "Parameter 2.  Page size is outside valid range.",
        );
    }
    code(api(handle).reindex_pages(handle, page_size))
}

/// `AdsCreateFTSIndex`: creates the full-text order `tag` on the field `field`
/// of the table `table`, in the index file `file` (its structural one if null
/// or empty), with pages of `page_size` bytes for a new file (1,024 for 0),
/// words of `min_word_len` to `max_word_len` letters, and the delimiters,
/// noise words, drop and conditional characters given, each with the
/// defaults or instead of them; builds it from the table's records and opens
/// it, replacing an order of that name (`fixtures/write/fts_rebuild`). The
/// lists are ANSI text, or UTF-8 or UTF-16 as `options` say
/// (`ADS_FTS_ENCODE_UTF8`, `_UTF16`); the collation is not used.
///
/// # Safety
///
/// `tag` and `field` must point to NUL-terminated strings, `file` and the
/// lists be null or point to them.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn AdsCreateFTSIndex(
    table: AdsHandle,
    file: *mut Unsigned8,
    tag: *mut Unsigned8,
    field: *mut Unsigned8,
    page_size: Unsigned32,
    min_word_len: Unsigned32,
    max_word_len: Unsigned32,
    use_default_delim: Unsigned16,
    delimiters: *mut std::ffi::c_void,
    use_default_noise: Unsigned16,
    noise_words: *mut std::ffi::c_void,
    use_default_drop: Unsigned16,
    drop_chars: *mut std::ffi::c_void,
    use_default_conditionals: Unsigned16,
    conditional_chars: *mut std::ffi::c_void,
    _collation: *mut Unsigned8,
    _reserved1: *mut Unsigned8,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCreateFTSIndex");
    const ENCODE_UTF8: Unsigned32 = 0x2000;
    const ENCODE_UTF16: Unsigned32 = 0x4000;
    let list = |text: *mut std::ffi::c_void| -> Option<String> {
        if text.is_null() {
            return None;
        }
        // SAFETY: a list is null or a NUL-terminated string, as # Safety says.
        unsafe {
            Some(match options {
                o if o & ENCODE_UTF16 != 0 => crate::ffi::wide_string(text.cast()),
                o if o & ENCODE_UTF8 != 0 => {
                    String::from_utf8_lossy(crate::ffi::c_str(text.cast()).unwrap_or_default())
                        .into_owned()
                }
                _ => crate::ffi::text(text.cast()).unwrap_or_default(),
            })
        }
    };
    let lists = vec![
        (use_default_delim != 0, list(delimiters)),
        (use_default_noise != 0, list(noise_words)),
        (use_default_drop != 0, list(drop_chars)),
        (use_default_conditionals != 0, list(conditional_chars)),
    ];
    // SAFETY: the pointers are as # Safety says.
    let (file, tag, field) = unsafe { (text(file), text(tag), text(field)) };
    let (Some(tag), Some(field)) = (tag, field) else {
        return fail(AE_INVALID_POINTER);
    };
    let file = file.filter(|f| !f.is_empty());
    code(api(table).create_fts_index(
        table,
        file,
        tag,
        field,
        page_size,
        min_word_len,
        max_word_len,
        lists,
        options,
    ))
}

/// `AdsReindexFTS`: rebuilds the index files of the table `table` that hold
/// only full-text orders, with pages of `page_size` bytes (their own for 0);
/// a file with other orders is left as it is.
#[unsafe(no_mangle)]
pub extern "system" fn AdsReindexFTS(table: AdsHandle, page_size: Unsigned32) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsReindexFTS");
    if let Err(code) = crate::handle::reindexed(table) {
        return code;
    }
    code(api(table).reindex_fts(table, page_size))
}

/// `AdsPackTable`: removes a table's deleted records and the memo values
/// only they hold, and rebuilds its open index files, as the original engine
/// does; the table must be open exclusively (`AE_TABLE_NOT_EXCLUSIVE`).
#[unsafe(no_mangle)]
pub extern "system" fn AdsPackTable(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsPackTable");
    if let Err(code) = crate::handle::known(table, crate::AE_INVALID_TABLE_HANDLE) {
        return code;
    }
    code(api(table).pack_table(table, false, 0))
}

/// `AdsZapTable`: removes all of a table's records, and rebuilds its open
/// index files; the table must be open exclusively.
#[unsafe(no_mangle)]
pub extern "system" fn AdsZapTable(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsZapTable");
    code(api(table).pack_table(table, true, 0))
}

/// `AdsPackTable120`: [`AdsPackTable`], with memo blocks of
/// `memo_block_size` bytes (0 keeps the table's; else 8 to 1,024, or
/// `AE_INVALID_MEMO_BLOCK_SIZE`). `options` must be 0 (`AE_INVALID_OPTION`).
#[unsafe(no_mangle)]
pub extern "system" fn AdsPackTable120(
    table: AdsHandle,
    memo_block_size: Unsigned32,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsPackTable120");
    if options != 0 {
        return crate::last_error::fail_detail(crate::AE_INVALID_OPTION, "Parameter 3");
    }
    match api(table).pack_table(table, false, memo_block_size) {
        Ok(()) => AE_SUCCESS,
        // The original engine names the call the size was given to.
        Err(tflip_engine::Error::InvalidMemoBlockSize) => {
            crate::last_error::fail_detail(crate::AE_INVALID_MEMO_BLOCK_SIZE, "AdsPackTable")
        }
        Err(error) => error_code(error),
    }
}

/// `AdsRecallAllRecords`: recalls every deleted record still on an ADT
/// table's free list, as the original engine does; the table must be open
/// exclusively (`AE_TABLE_NOT_EXCLUSIVE`).
#[unsafe(no_mangle)]
pub extern "system" fn AdsRecallAllRecords(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsRecallAllRecords");
    if let Err(code) = crate::handle::known(table, crate::AE_INVALID_TABLE_HANDLE) {
        return code;
    }
    code(api(table).recall_all(table))
}

/// `AdsDeleteTable` (unpublished): closes a table and deletes its file, its
/// memo file and its structural index file. As in the original engine, a
/// table open shared is deleted too.
#[unsafe(no_mangle)]
pub extern "system" fn AdsDeleteTable(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDeleteTable");
    if let Err(code) = crate::filter::table_of(table) {
        return code;
    }
    code(api(table).delete_table(table))
}

/// `AdsSetTableTransactionFree`: sets or clears the transaction-free flag
/// in an ADT table's header; the table must be open exclusively.
#[unsafe(no_mangle)]
pub extern "system" fn AdsSetTableTransactionFree(
    table: AdsHandle,
    free: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetTableTransactionFree");
    if let Err(code) = crate::filter::table_of(table) {
        return code;
    }
    code(api(table).set_transaction_free(table, free != 0))
}

/// Changes the fields of a table (see [`AdsRestructureTable120`]); `None`
/// for a null pointer. `lists` are the fields to add, delete and change.
fn restructure(
    connect: AdsHandle,
    name: Option<String>,
    table_type: Unsigned16,
    char_type: Unsigned16,
    check_rights: Unsigned16,
    lists: [Option<String>; 3],
    memo_block_size: Unsigned32,
) -> Unsigned32 {
    let Some(name) = name else {
        return fail(AE_INVALID_POINTER);
    };
    let Some(table_type) = engine_table_type(table_type) else {
        return fail(AE_INVALID_TABLETYPE);
    };
    let settings = TableSettings {
        table_type,
        char_type,
        rights: check_rights,
        ..Default::default()
    };
    let name = crate::remote::relative_name(connect, &name);
    let [add, delete, change] = lists.map(Option::unwrap_or_default);
    code(api(connect).restructure_table(
        connect,
        name,
        settings,
        add,
        delete,
        change,
        memo_block_size,
    ))
}

/// `AdsRestructureTable`: [`AdsRestructureTable120`] keeping the memo block
/// size.
///
/// # Safety
///
/// As for [`AdsRestructureTable120`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsRestructureTable(
    connect: AdsHandle,
    name: *mut Unsigned8,
    _password: *mut Unsigned8,
    table_type: Unsigned16,
    char_type: Unsigned16,
    _lock_type: Unsigned16,
    check_rights: Unsigned16,
    add: *mut Unsigned8,
    delete: *mut Unsigned8,
    change: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsRestructureTable");
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let (name, lists) = unsafe { (text(name), [text(add), text(delete), text(change)]) };
    restructure(connect, name, table_type, char_type, check_rights, lists, 0)
}

/// `AdsRestructureTable90`: [`AdsRestructureTable120`] keeping the memo
/// block size; the collation is not used.
///
/// # Safety
///
/// As for [`AdsRestructureTable120`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsRestructureTable90(
    connect: AdsHandle,
    name: *mut Unsigned8,
    _password: *mut Unsigned8,
    table_type: Unsigned16,
    char_type: Unsigned16,
    _lock_type: Unsigned16,
    check_rights: Unsigned16,
    add: *mut Unsigned8,
    delete: *mut Unsigned8,
    change: *mut Unsigned8,
    _collation: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsRestructureTable90");
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let (name, lists) = unsafe { (text(name), [text(add), text(delete), text(change)]) };
    restructure(connect, name, table_type, char_type, check_rights, lists, 0)
}

/// `AdsRestructureTable120`: changes the fields of the free ADT table `name`
/// (others are `AE_NOT_IMPLEMENTED`) as the original engine does (see
/// [`tflip_engine::restructure`]): `add` and `change` are field definitions
/// as `AdsCreateTable` takes them (`change`'s each after the old name, and
/// both maybe ending in `,(n)`, the field's position), `delete` field names,
/// all separated by `;`. The memo file takes blocks of `memo_block_size`
/// bytes (0 keeps them); `options` must be 0. The old files stay as
/// `<file>.BAK`. The password and collation are not used.
///
/// # Safety
///
/// `name`, `add`, `delete` and `change` must be null or point to
/// NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsRestructureTable120(
    connect: AdsHandle,
    name: *mut Unsigned8,
    _password: *mut Unsigned8,
    table_type: Unsigned16,
    char_type: Unsigned16,
    _lock_type: Unsigned16,
    check_rights: Unsigned16,
    add: *mut Unsigned8,
    delete: *mut Unsigned8,
    change: *mut Unsigned8,
    _collation: *mut Unsigned8,
    memo_block_size: Unsigned32,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsRestructureTable120");
    if options != 0 {
        return fail(crate::AE_INVALID_OPTION);
    }
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let (name, lists) = unsafe { (text(name), [text(add), text(delete), text(change)]) };
    restructure(
        connect,
        name,
        table_type,
        char_type,
        check_rights,
        lists,
        memo_block_size,
    )
}

/// The connection of the table, cursor or index order `handle`.
fn connection_of(handle: AdsHandle) -> AdsHandle {
    let backend = api(handle);
    let table = backend
        .index_info(handle)
        .map_or(handle, |order| order.table);
    backend.table_info(table).map_or(0, |info| info.connection)
}

/// `ADS_EXCLUSIVE`, an open option.
const ADS_EXCLUSIVE: u32 = 1;

/// After a copy that returned 0: a table opened exclusively cannot be copied
/// by the server, which leaves `AE_INFO_COPY_MADE_BY_CLIENT` as the last
/// error of the call, with the call's success. It says the source table
/// was opened exclusively, or the index order a copy goes in
/// (`fixtures/write/copy_made_by_client`). Not for a structure alone.
fn made_by_client(handle: AdsHandle, rc: Unsigned32) -> Unsigned32 {
    if rc != AE_SUCCESS {
        return rc;
    }
    let backend = api(handle);
    let order = backend.index_info(handle).ok();
    let table = order.as_ref().map_or(handle, |order| order.table);
    let exclusive = backend
        .table_info(table)
        .is_ok_and(|info| info.settings.options & ADS_EXCLUSIVE != 0);
    if exclusive {
        let what = match order {
            Some(_) => "The index order used for copying was opened exclusively.",
            None => "The source table was opened exclusively.",
        };
        crate::last_error::fail_detail(crate::AE_INFO_COPY_MADE_BY_CLIENT, what);
    }
    rc
}

/// Copies a table to `file` (see [`AdsCopyTable`]); `None` for a null
/// pointer.
fn copy(
    handle: AdsHandle,
    filter: Unsigned16,
    file: Option<String>,
    structure_only: bool,
) -> Unsigned32 {
    if !(1..=3).contains(&filter) && !structure_only {
        return fail(crate::AE_INVALID_OPTION);
    }
    let Some(file) = file else {
        return fail(AE_INVALID_POINTER);
    };
    let name = crate::remote::relative_name(connection_of(handle), &file);
    let rc = code(api(handle).copy_table(handle, filter, name, structure_only));
    if structure_only {
        rc
    } else {
        made_by_client(handle, rc)
    }
}

/// `AdsCopyTable`: copies a table (an index order's, in its order; a
/// cursor) to the new table `file`, with the records `filter` takes
/// (`ADS_IGNOREFILTERS`, `ADS_RESPECTSCOPES`, `ADS_RESPECTFILTERS`; a DBF
/// table's deleted ones while they are shown), byte for byte, as the
/// original engine does (see [`tflip_engine::copy`]). Index files are not
/// copied.
///
/// # Safety
///
/// `file` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCopyTable(
    handle: AdsHandle,
    filter: Unsigned16,
    file: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCopyTable");
    // SAFETY: `file` is null or NUL-terminated, as # Safety says.
    let file = unsafe { text(file) };
    copy(handle, filter, file, false)
}

/// `AdsCopyTableStructure`: creates the table `file` with the fields of a
/// table, and no records.
///
/// # Safety
///
/// `file` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCopyTableStructure(
    table: AdsHandle,
    file: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCopyTableStructure");
    // SAFETY: `file` is null or NUL-terminated, as # Safety says.
    let file = unsafe { text(file) };
    copy(table, 0, file, true)
}

/// `AdsCopyTableContents`: appends the records of a table (an index
/// order's, in its order; a cursor) that `filter` takes to the open table
/// `to`, each field set from the source field of the same name, as a client
/// would set it (AutoInc values are new).
#[unsafe(no_mangle)]
pub extern "system" fn AdsCopyTableContents(
    from: AdsHandle,
    to: AdsHandle,
    filter: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCopyTableContents");
    if !(1..=3).contains(&filter) {
        return fail(crate::AE_INVALID_OPTION);
    }
    made_by_client(from, code(api(from).copy_contents(from, to, filter)))
}

/// `AdsConvertTable`: copies a table (an index order's, in its order; a
/// cursor) to the new table `file` of the type `table_type`, with the
/// records `filter` takes, its fields converted as the original engine
/// converts them (see [`tflip_engine::copy::convert_table`]): the common
/// field types; others are `AE_NOT_IMPLEMENTED`. Index files are not made.
///
/// # Safety
///
/// `file` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsConvertTable(
    handle: AdsHandle,
    filter: Unsigned16,
    file: *mut Unsigned8,
    table_type: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsConvertTable");
    if !(1..=3).contains(&filter) {
        return fail(crate::AE_INVALID_OPTION);
    }
    let Some(table_type) = engine_table_type(table_type) else {
        return fail(AE_INVALID_TABLETYPE);
    };
    // SAFETY: `file` is null or NUL-terminated, as # Safety says.
    let Some(file) = (unsafe { text(file) }) else {
        return fail(AE_INVALID_POINTER);
    };
    let name = crate::remote::relative_name(connection_of(handle), &file);
    let rc = code(api(handle).convert_table(handle, filter, name, table_type));
    // Only a copy to a table of the same type is one.
    let backend = api(handle);
    let table = backend
        .index_info(handle)
        .map_or(handle, |order| order.table);
    let same = backend
        .table_info(table)
        .is_ok_and(|info| info.settings.table_type == table_type);
    if same { made_by_client(handle, rc) } else { rc }
}

/// `AdsDDDeployDatabase`: deploys the data dictionary `source` to the folder
/// `destination`, a database of free tables, which then are the dictionary's
/// (see [`tflip_engine::dictionary_write::deploy_database_api`]). It runs in
/// this process, as `AdsDDFreeTable` does, whatever the server type: the
/// original's remote server would run it on the server's side of the same
/// files. The destination's password is ignored, as it is in the original.
///
/// # Safety
///
/// The pointers must be null or point to NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDDeployDatabase(
    destination: *mut Unsigned8,
    _destination_password: *mut Unsigned8,
    source: *mut Unsigned8,
    source_password: *mut Unsigned8,
    server_types: Unsigned16,
    validate_option: Unsigned16,
    backup_files: Unsigned16,
    _options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDDDeployDatabase");
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let [destination, source, password] =
        unsafe { [text(destination), text(source), text(source_password)] };
    let (Some(destination), Some(source)) = (destination, source) else {
        return fail(AE_INVALID_POINTER);
    };
    match tflip_engine::dictionary_write::deploy_database_api(
        &crate::LOCAL,
        &destination,
        &source,
        &password.unwrap_or_default(),
        server_types,
        validate_option,
        backup_files,
    ) {
        // The engine's own codes, as the original answers this call
        // (`fixtures/dictionary/deploy`).
        Err(tflip_engine::Error::Sql(sql)) if (5000..6000).contains(&sql.native) => {
            direct_error_code(tflip_engine::Error::Sql(sql))
        }
        result => code(result),
    }
}

#[cfg(test)]
mod tests {
    use super::{utf8_bom, wide_field_property};

    #[test]
    fn utf8_bom_wraps_text_as_the_engine_stores_a_wide_property() {
        assert_eq!(utf8_bom("id > 0"), b"\xEF\xBB\xBFid > 0\0");
        assert_eq!(utf8_bom("caf\u{e9}"), b"\xEF\xBB\xBFcaf\xC3\xA9\0");
        assert_eq!(utf8_bom(""), b"\xEF\xBB\xBF\0");
    }

    #[test]
    fn wide_field_properties_map_to_their_narrow_ids() {
        assert_eq!(wide_field_property(311), Some(300)); // default value
        assert_eq!(wide_field_property(312), Some(302)); // minimum
        assert_eq!(wide_field_property(313), Some(303)); // maximum
        // The comment and narrow properties are not wide.
        assert_eq!(wide_field_property(300), None);
        assert_eq!(wide_field_property(1), None);
    }
}

/// What `AdsDDAddTable` does with the corners of DBF tables the first
/// dictionaries leave (`fixtures/dictionary/dbfcorners`, made by
/// `tools/ace/dd/ddwrite.c`): OEM tables, a Visual FoxPro table with NULL
/// fields, `ADS_DEFAULT` as the type to add, and the character types each
/// type takes; the error codes and texts, the dictionary and the files, and
/// how the tables open.
#[cfg(test)]
mod dbf_corners {
    use std::ffi::CString;
    use std::path::Path;

    use crate::fixtures;
    use crate::{
        ADS_ADT, ADS_CDX, ADS_DATABASE_TABLE, ADS_NTX, ADS_VFP, AdsCloseTable, AdsDDAddTable,
        AdsDDCreate, AdsDDGetTableProperty, AdsDisconnect, AdsGetField, AdsGetLastError,
        AdsGotoTop, AdsHandle, AdsOpenTable, Unsigned8,
    };

    const ADS_ANSI: u16 = 1;
    const ADS_OEM: u16 = 2;
    const CORRUPT_CDX: &str = "Corrupt .ADI, .CDX, or .IDX index.";
    const NO_ADI_VERSION: &str = "The ADI version number is not recognized.";
    const INVALID_TABLE_TYPE: &str = "Error 5025:  An invalid table type was specified.  Valid \
         choices are ADS_ADT, ADS_VFP, ADS_CDX, and ADS_NTX.  The table type must support the \
         memo file type to be opened, if applicable.";

    fn c(text: &str) -> CString {
        CString::new(text).unwrap()
    }

    fn pointer(text: &CString) -> *mut Unsigned8 {
        text.as_ptr().cast_mut().cast()
    }

    /// The last error's text.
    fn last_error() -> String {
        let (mut code, mut buf, mut len) = (0, [0u8; 600], 600);
        unsafe { AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned()
    }

    /// Adds the table at `file` as `name`; its return code, and the text of
    /// the error.
    fn add(
        dictionary: AdsHandle,
        name: &str,
        file: &str,
        table_type: u16,
        chars: u16,
        indexes: &str,
    ) -> (u32, String) {
        let (name, file, indexes, empty) = (c(name), c(file), c(indexes), c(""));
        let rc = unsafe {
            AdsDDAddTable(
                dictionary,
                pointer(&name),
                pointer(&file),
                table_type,
                chars,
                pointer(&indexes),
                pointer(&empty),
            )
        };
        let text = if rc == 0 { String::new() } else { last_error() };
        (rc, text)
    }

    /// The character type the dictionary keeps for `name`.
    fn char_type(dictionary: AdsHandle, name: &str) -> u16 {
        let name = c(name);
        let (mut value, mut len) = (0u16, 2u16);
        let rc = unsafe {
            AdsDDGetTableProperty(
                dictionary,
                pointer(&name),
                212,
                (&mut value as *mut u16).cast(),
                &mut len,
            )
        };
        assert_eq!(rc, 0, "property of {name:?}");
        value
    }

    /// The first record's `name` of the table `name`, opened by it.
    fn first_name(dictionary: AdsHandle, name: &str, chars: u16) -> Vec<u8> {
        let table_name = c(name);
        let mut table = 0;
        let null = std::ptr::null_mut();
        let rc = unsafe {
            AdsOpenTable(
                dictionary,
                pointer(&table_name),
                null,
                ADS_DATABASE_TABLE,
                chars,
                0,
                0,
                0,
                &mut table,
            )
        };
        assert_eq!(rc, 0, "opening {name}");
        AdsGotoTop(table);
        let (field, mut buf, mut len) = (c("name"), [0u8; 64], 64u32);
        let rc = unsafe { AdsGetField(table, pointer(&field), buf.as_mut_ptr(), &mut len, 0) };
        assert_eq!(rc, 0);
        AdsCloseTable(table);
        buf[..len as usize].to_vec()
    }

    #[test]
    fn dbf_corners_match_the_original_engine() {
        let golden = fixtures::dir("../dictionary/dbfcorners");
        let (_, dir) = fixtures::scratch_from("dd-dbfcorners", &golden.join("free"));
        let add_path = c(&dir.join("db.add").display().to_string());
        let mut dictionary = 0;
        let rc =
            unsafe { AdsDDCreate(pointer(&add_path), 0, std::ptr::null_mut(), &mut dictionary) };
        assert_eq!(rc, 0);

        let ok = (0, String::new());
        // OEM tables, made with either character type and added as either.
        assert_eq!(
            add(dictionary, "oc", "oc.dbf", ADS_CDX, ADS_OEM, "oc.cdx"),
            ok
        );
        assert_eq!(
            add(dictionary, "on1", "on.dbf", ADS_NTX, ADS_OEM, "on_id.ntx"),
            ok
        );
        assert_eq!(
            add(dictionary, "ov", "ov.dbf", ADS_VFP, ADS_OEM, "ov.cdx"),
            ok
        );
        assert_eq!(add(dictionary, "oco", "oco.dbf", ADS_CDX, ADS_ANSI, ""), ok);
        assert_eq!(add(dictionary, "oca", "oca.dbf", ADS_CDX, ADS_OEM, ""), ok);
        // No character type, and one a FoxPro table cannot have.
        assert_eq!(
            add(dictionary, "oc0", "oco.dbf", ADS_CDX, 0, ""),
            (
                5027,
                "Error 5027:  An invalid character set was specified.  Parameter 5".to_owned()
            )
        );
        assert_eq!(
            add(dictionary, "ocx", "oca.dbf", ADS_CDX, 9, ""),
            (
                5025,
                format!(
                    "{INVALID_TABLE_TYPE}  Collation languages can only be specified with \
                     ADS_ADT and ADS_VFP table types."
                )
            )
        );
        for (name, kept) in [("oc", 2), ("on1", 2), ("ov", 2), ("oco", 1), ("oca", 2)] {
            assert_eq!(char_type(dictionary, name), kept, "{name}");
        }
        // The dictionary's character type, not the one a table is opened
        // with, says how its text reads: an OEM table's é is stored as 0x82
        // and an ANSI table's as 0xE9 (read as OEM, that is a theta, which
        // comes out as a T).
        let blanks = |first: &[u8]| [first, &[b' '; 7]].concat();
        for chars in [ADS_ANSI, ADS_OEM] {
            for name in ["oc", "on1", "ov"] {
                assert_eq!(
                    first_name(dictionary, name, chars),
                    blanks(b"\xe9t\xe9"),
                    "{name}"
                );
            }
            assert_eq!(first_name(dictionary, "oco", chars), blanks(b"\x82t\x82"));
            assert_eq!(first_name(dictionary, "oca", chars), blanks(b"TtT"));
        }

        // A Visual FoxPro table with NULL-able fields.
        assert_eq!(
            add(dictionary, "vn", "vn.dbf", ADS_VFP, ADS_ANSI, "vn.cdx"),
            ok
        );
        assert_eq!(
            add(dictionary, "vn2", "vn2.dbf", ADS_VFP, ADS_ANSI, "vn2.cdx"),
            ok
        );

        // ADS_DEFAULT is no type to add, whatever the file, before the file
        // is looked for; nor is another number.
        for (name, file, table_type, indexes) in [
            ("dd", "d.adt", ADS_DATABASE_TABLE, "d.adi"),
            ("d1", "d1.dbf", ADS_DATABASE_TABLE, "d1.cdx"),
            ("d2", "d2.dbf", ADS_DATABASE_TABLE, "d2_id.ntx"),
            ("d3", "d3.dbf", ADS_DATABASE_TABLE, ""),
            ("dmiss", "missing.adt", ADS_DATABASE_TABLE, ""),
            ("dmiss2", "missing", ADS_DATABASE_TABLE, ""),
            ("dbad", "d1.dbf", 9, ""),
        ] {
            assert_eq!(
                add(dictionary, name, file, table_type, ADS_ANSI, indexes),
                (5025, format!("{INVALID_TABLE_TYPE}  Parameter 4")),
                "{name}"
            );
        }
        // The files and index files that are not there or are not the type.
        let missing = |file: &str| format!("{}\\{file}", dir.display());
        let (rc, text) = add(dictionary, "dmiss3", "missing.adt", ADS_ADT, ADS_ANSI, "");
        assert_eq!(rc, 7041);
        assert!(
            text.starts_with(
                "Error 7041:  File not found.  Verify the specified path and file name is \
                 correct."
            ),
            "{text} ({})",
            missing("missing.adt")
        );
        assert_eq!(
            add(dictionary, "dmiss4", "missing.dbf", ADS_CDX, ADS_ANSI, "").0,
            7041
        );
        assert_eq!(
            add(
                dictionary, "dnoidx", "d1.dbf", ADS_CDX, ADS_ANSI, "nope.cdx"
            ),
            (
                7041,
                "Error 7041:  File not found.  Verify the specified path and file name is \
                 correct."
                    .to_owned()
            )
        );
        // An index file of another type than its table's: refused for each
        // type its way.
        for (name, file, table_type, indexes, code, text) in [
            (
                "dwrongidx",
                "d2.dbf",
                ADS_NTX,
                "d1.cdx",
                7015,
                "Corrupt NTX index.",
            ),
            (
                "dwrongidx2",
                "d1.dbf",
                ADS_CDX,
                "d2_id.ntx",
                7017,
                CORRUPT_CDX,
            ),
            (
                "dwrongidx3",
                "d3.dbf",
                ADS_VFP,
                "d2_id.ntx",
                7017,
                CORRUPT_CDX,
            ),
            (
                "dwrongidx4",
                "d.adt",
                ADS_ADT,
                "d1.cdx",
                7108,
                NO_ADI_VERSION,
            ),
            (
                "dwrongidx5",
                "d2.dbf",
                ADS_NTX,
                "d.adi",
                7015,
                "Corrupt NTX index.",
            ),
            (
                "dwrongidx6",
                "d.adt",
                ADS_ADT,
                "d2_id.ntx",
                7108,
                NO_ADI_VERSION,
            ),
        ] {
            assert_eq!(
                add(dictionary, name, file, table_type, ADS_ANSI, indexes),
                (code, format!("Error {code}:  {text}")),
                "{name}"
            );
        }

        // The character types each table type takes: 1 to 118 for ADT and
        // VFP tables (ADS_OEM kept by a VFP table alone), ADS_ANSI and
        // ADS_OEM for the others; none else.
        let bad_set = (
            5027,
            "Error 5027:  An invalid character set was specified.  Parameter 5".to_owned(),
        );
        let collation = (
            5025,
            format!(
                "{INVALID_TABLE_TYPE}  Collation languages can only be specified with ADS_ADT \
                 and ADS_VFP table types."
            ),
        );
        type Refusal = (u32, String);
        let tries: [(u16, u16, Option<Refusal>, u16); 18] = [
            (ADS_ADT, 0, Some(bad_set.clone()), 0),
            (ADS_ADT, 1, None, 1),
            (ADS_ADT, 2, None, 1),
            (ADS_ADT, 3, None, 1),
            (ADS_ADT, 9, None, 1),
            (ADS_ADT, 118, None, 1),
            (ADS_ADT, 119, Some(bad_set.clone()), 0),
            (ADS_ADT, 65535, Some(bad_set.clone()), 0),
            (ADS_VFP, 0, Some(bad_set.clone()), 0),
            (ADS_VFP, 3, None, 1),
            (ADS_VFP, 9, None, 1),
            (ADS_VFP, 119, Some(bad_set.clone()), 0),
            (ADS_CDX, 3, Some(collation.clone()), 0),
            (ADS_CDX, 118, Some(collation.clone()), 0),
            (ADS_CDX, 119, Some(bad_set.clone()), 0),
            (ADS_CDX, 65535, Some(bad_set.clone()), 0),
            (ADS_NTX, 9, Some(collation), 0),
            (ADS_NTX, 0, Some(bad_set), 0),
        ];
        for (k, (table_type, chars, error, kept)) in tries.into_iter().enumerate() {
            let file = format!("t{k}.{}", if table_type == ADS_ADT { "adt" } else { "dbf" });
            let result = add(dictionary, &format!("t{k}"), &file, table_type, chars, "");
            match error {
                Some(error) => assert_eq!(result, error, "t{k}"),
                None => {
                    assert_eq!(result, ok, "t{k}");
                    assert_eq!(char_type(dictionary, &format!("t{k}")), kept, "t{k}");
                }
            }
        }
        AdsDisconnect(dictionary);

        // The dictionary's records and the files, as they were.
        let ours = std::fs::read(dir.join("db.add")).unwrap();
        let want = std::fs::read(golden.join("db.add")).unwrap();
        assert_eq!(ours.len(), want.len(), "add length");
        let data = u32::from_le_bytes(ours[0x20..0x24].try_into().unwrap()) as usize;
        let record_length = 524;
        for record in 1..(ours.len() - data) / record_length {
            let at = data + record * record_length;
            assert_eq!(
                &ours[at..at + record_length],
                &want[at..at + record_length],
                "record {}",
                record + 1
            );
        }
        assert_eq!(ours[..0x60], want[..0x60], "add header");
        assert_same_files(&dir, &golden);
    }

    /// Every file of `golden` but the dictionary's own `.add` is `dir`'s.
    fn assert_same_files(dir: &Path, golden: &Path) {
        for entry in std::fs::read_dir(golden).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap();
            if path.is_file() && path.extension().is_none_or(|e| e != "add") {
                let ours = std::fs::read(dir.join(name)).unwrap();
                assert!(ours == std::fs::read(&path).unwrap(), "{}", name.display());
            }
        }
    }
}

/// `AdsDDDeployDatabase` through the exported function: the codes and texts
/// of the refusals as the original gives them, and a deployment of the
/// folder of `fixtures/dictionary/deploy/same_backup` (the engine's own
/// replay of every scenario is in `tflip_engine::dictionary_write`).
#[cfg(test)]
mod deploy {
    use std::ffi::CString;
    use std::path::Path;

    use crate::{AdsDDDeployDatabase, AdsGetLastError, Unsigned8};

    fn c(path: &Path) -> CString {
        CString::new(path.to_string_lossy().into_owned()).unwrap()
    }

    fn pointer(text: &CString) -> *mut Unsigned8 {
        text.as_ptr().cast_mut().cast()
    }

    fn last_error() -> String {
        let (mut code, mut buf, mut len) = (0, [0u8; 600], 600);
        unsafe { AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned()
    }

    fn copy_all(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let path = entry.unwrap().path();
            let target = to.join(path.file_name().unwrap());
            match path.is_dir() {
                true => copy_all(&path, &target),
                false => drop(std::fs::copy(&path, &target).unwrap()),
            }
        }
    }

    #[test]
    fn deploys_and_refuses_as_the_engine_does() {
        let work = tflip_storage::testing::temp_root()
            .join(format!("tflip-ace-deploy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        let before = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/dictionary/deploy/same_backup/before");
        copy_all(&before, &work);
        let (dest, add) = (c(&work.join("D")), c(&work.join("S/newdb.add")));
        let missing = c(&work.join("missing"));
        let deploy = |dest: &CString, add: &CString, options: (u16, u16, u16)| unsafe {
            AdsDDDeployDatabase(
                pointer(dest),
                std::ptr::null_mut(),
                pointer(add),
                std::ptr::null_mut(),
                options.0,
                options.1,
                options.2,
                0,
            )
        };

        assert_eq!(deploy(&dest, &add, (3, 0, 0)), 5019);
        assert_eq!(
            last_error(),
            "Error 5019:  An invalid option was specified.  Parameter 5"
        );
        assert_eq!(deploy(&dest, &add, (1, 1, 0)), 5019);
        assert_eq!(
            last_error(),
            "Error 5019:  An invalid option was specified.  Parameter 6"
        );
        assert_eq!(deploy(&missing, &add, (1, 0, 0)), 5020);
        assert_eq!(
            last_error(),
            "Error 5020:  The path given was not valid.  Could not find dictionary file"
        );
        assert_eq!(
            unsafe {
                AdsDDDeployDatabase(
                    pointer(&dest),
                    std::ptr::null_mut(),
                    pointer(&add),
                    pointer(&c(Path::new("secret"))),
                    1,
                    0,
                    0,
                    0,
                )
            },
            7078
        );

        assert_eq!(deploy(&dest, &add, (2, 0, 1)), 0);
        assert!(work.join("D/newdb.add").exists());
        assert!(work.join("D/adsdeploy.log").exists());
        assert!(work.join("D/backup/t.adt").exists());
        assert!(work.join("S/adsbackup.add").exists());
        assert!(!work.join("S/newdb.add").exists());
        // The folder has a dictionary now, as it has no room for another.
        assert_eq!(
            deploy(&dest, &c(&work.join("D/newdb.add")), (1, 0, 0)),
            5159
        );
        assert_eq!(
            last_error(),
            "Error 5159:  Error encountered when trying to open a database table.  The table is \
             a database table. It cannot be opened as a free table."
        );
        let _ = std::fs::remove_dir_all(&work);
    }
}
