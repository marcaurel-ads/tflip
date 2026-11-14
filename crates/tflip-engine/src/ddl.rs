//! `CREATE TABLE`, `CREATE INDEX`, `DROP TABLE`, `DROP INDEX` and `ALTER
//! TABLE` on free tables, and `sp_CreateIndex90`: the files the original
//! engine writes for them (`fixtures/write/ddl` for ADT tables,
//! `fixtures/write/sql_dbf` for DBF tables, which a statement makes with its
//! table type; see [`tflip_storage::create`]). On a dictionary connection
//! `CREATE TABLE` and `DROP TABLE` add the table to the dictionary and take
//! it out (`fixtures/sql/ddtables`).
//!
//! An order created on a table with records is built at once, as the
//! original engine builds it (see [`tflip_storage::create::build_order`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tflip_sql::SqlError;
use tflip_sql::ast::{
    ColumnRule, CreateIndex, CreateTable, DropTable, FieldRules, Named, TableRef,
};
use tflip_storage::create::{self, FieldSpec, OrderSpec};
use tflip_storage::field::FieldType;
use tflip_storage::order::{flags, options};

use tflip_sql::ast::ColumnDef;
use tflip_storage::dbf::Flavour;

use crate::dictionary_write::rights;
use crate::handle::Handle;
use crate::statement::Settings;
use crate::table::{self, TableSettings, TableType};
use crate::{Error, Session, connection, index};

/// The default block size of a new table's memo file.
pub(crate) const BLOCK_SIZE: u16 = 256;

/// The `ADS_DD_TABLE_OBJECT` type, for the rights on a table.
const TABLE: u16 = 1;

/// The `char_type` of OEM tables (`ADS_OEM`).
const OEM: u16 = 2;

/// The default page size of a new index file.
const PAGE_SIZE: u16 = 512;

/// `AdsCreateIndex` options (`ace.h`).
pub mod index_options {
    pub const UNIQUE: u32 = 0x0001;
    pub const COMPOUND: u32 = 0x0002;
    pub const CUSTOM: u32 = 0x0004;
    pub const DESCENDING: u32 = 0x0008;
    pub const BINARY: u32 = 0x1000;
    pub const CANDIDATE: u32 = 0x0800;
    /// `ADS_UCHAR_KEY_SHORT`, `_LONG` and `_XLONG`: how many bytes a
    /// character of an NChar or NVarChar field takes in the order's keys.
    pub const UCHAR_KEY: u32 = 0x6000_0000;
    /// The options an ADI order may be made with (`ADS_INDEX_OPTIONS_MASK`).
    pub const ALL: u32 = 0x7020_7FFF;
}

/// The file `name` of `connection`'s folder (or share), as it is.
pub(crate) fn path_in(session: &Session, connection: Handle, name: &str) -> Result<PathBuf, Error> {
    let owner = connection::get(session, connection)?;
    let folder = owner.folder();
    match &session.shares {
        Some(shares) => shares.inside(&folder, name),
        None => Ok(crate::settings::local_join(session, &folder, name)),
    }
}

/// The length of the keys of the key `expression` (fields by name, compiled
/// as `key`) on `opened`, and whether they are text (padded with blanks): a
/// key of fields is theirs one after the other, else the expression's.
pub(crate) fn key_shape(
    opened: &table::Table,
    expression: &str,
    key: &tflip_expr::Expr,
    width: u8,
) -> (usize, bool) {
    let fields = opened.fields();
    let segments: Option<Vec<&tflip_storage::field::Field>> =
        crate::index::unqualified(opened, expression)
            .split(';')
            .map(|part| {
                fields
                    .iter()
                    .find(|f| f.name.eq_ignore_ascii_case(part.trim()))
            })
            .collect();
    match &segments {
        // Keys of segments are padded with blanks when the last one is text
        // (`fixtures/sql/ddtables`, `fr1`).
        Some(segments) => (
            segments
                .iter()
                .map(|f| tflip_storage::key::field_key_length_wide(f, width))
                .sum::<usize>(),
            segments.last().is_some_and(|f| is_text(f.field_type)),
        ),
        None => {
            let ty = key.ty();
            match key.unicode() {
                // Three bytes (or what the width option asks for) a
                // character and six more, as a field's keys are
                // (`fixtures/write/nchar_expr`); no more than a key may be.
                tflip_expr::Unicode::Text { width: chars, .. } => (
                    (tflip_storage::key::unicode_width(width) as i32 * chars + 6)
                        .clamp(0, i32::from(create::ADI_MAX_KEY)) as usize,
                    true,
                ),
                // 8-bit text made of Unicode text.
                tflip_expr::Unicode::Narrow { width: bytes } => (
                    bytes.clamp(0, i32::from(create::ADI_MAX_KEY)) as usize,
                    true,
                ),
                _ => (
                    usize::from(ty.width()),
                    matches!(ty, tflip_expr::Type::String { .. }),
                ),
            }
        }
    }
}

/// The file `name` of `connection`'s folder (or share), with `extension`
/// if it has none.
fn file_path(
    session: &Session,
    connection: Handle,
    name: &str,
    extension: &str,
) -> Result<PathBuf, Error> {
    let owner = connection::get(session, connection)?;
    let folder = owner.folder();
    let mut path = match &session.shares {
        Some(shares) => shares.inside(&folder, name)?,
        None => crate::settings::local_join(session, &folder, name),
    };
    if path.extension().is_none() {
        path.set_extension(extension);
    }
    Ok(path)
}

/// The file of the new temporary table `name` (`#name`) of the type of
/// the statement's `settings`.
pub(crate) fn temp_path(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    name: &str,
) -> Result<PathBuf, Error> {
    if crate::temp::bare(name).is_empty() {
        return Err(Error::Sql(SqlError::ace(
            5019,
            "An invalid option was specified.",
            "The name of the temporary table must be specified.",
        )));
    }
    let owner = connection::get(session, connection)?;
    owner
        .temps
        .create(name, settings.table_type)
        .ok_or_else(|| {
            Error::Sql(SqlError::ace(
                5189,
                "A table with the same name already exists.",
                &format!("Table Name: {}", crate::temp::bare(name)),
            ))
        })
}

/// [`temp_path`] for `AdsCreateTable`, which words a name in use as the
/// error itself and not as an SQL error.
fn temp_api_path(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: &TableSettings,
) -> Result<PathBuf, Error> {
    let owner = connection::get(session, connection)?;
    owner
        .temps
        .create(name, settings.table_type)
        .ok_or_else(|| {
            Error::FullText(5189).detailed(format!("Table Name: {}", crate::temp::bare(name)))
        })
}

/// 2125, a table name longer than a file's (255 characters).
fn check_table_name(name: &str) -> Result<(), Error> {
    match name.len() > 255 {
        true => Err(Error::Sql(SqlError::invalid_table_name(name))),
        false => Ok(()),
    }
}

/// 2009, the path of a new table of more than 255 characters.
fn check_path(path: &Path) -> Result<(), Error> {
    match path.as_os_str().len() > 255 {
        true => Err(Error::Sql(SqlError::path_too_long())),
        false => Ok(()),
    }
}

/// 5012, a field definition `CREATE TABLE` refuses, with `why`.
pub(crate) fn invalid_field(why: &str) -> Error {
    Error::Sql(SqlError::ace(
        5012,
        "The field definition specified for the table was not valid.",
        why,
    ))
}

/// What the original engine refuses of a Numeric field of `length` and
/// `decimals` (`name` as it shows it): a DBF field's length is a byte, and
/// one of 1 is fine there but not in an ADT table.
fn numeric_problem(dbf: bool, length: u64, decimals: u64, name: &str) -> Option<String> {
    let length = if dbf { length & 0xFF } else { length };
    if dbf && length == 0 {
        return Some(format!("Numeric length cannot be zero. Field: \"{name}\""));
    }
    if !dbf && length < 2 {
        return Some(format!(
            "Numeric length must be greater than one. Field: \"{name}\""
        ));
    }
    if length > 32 {
        return Some(format!(
            "Numeric length ({length}) is too long. Field: \"{name}\""
        ));
    }
    if decimals > 0 && length < decimals.saturating_add(2) {
        return Some(format!(
            "Numeric length must be at least 2 greater than number of decimals. Field: \"{name}\""
        ));
    }
    None
}

/// What the original engine refuses of a Double field's `decimals`: more
/// than 30 (of a byte, which wraps).
fn decimals_problem(decimals: u64, name: &str) -> Option<String> {
    let decimals = decimals & 0xFF;
    (decimals > 30).then(|| format!("Decimals value ({decimals}) is too long. Field: \"{name}\""))
}

/// The field `column` of `CREATE TABLE` defines.
pub(crate) fn field_spec(column: &tflip_sql::ast::ColumnDef) -> Result<FieldSpec, Error> {
    field_spec_in(column, None)
}

/// [`field_spec`] of a column of a table of `flavour` (an ADT table's if
/// `None`): the limits are those of the original engine for its type
/// (`fixtures/sql/limits.json`), and the field's name and definition are shown
/// upper-cased in the errors of a Clipper or FoxPro table.
fn field_spec_in(
    column: &tflip_sql::ast::ColumnDef,
    flavour: Option<Flavour>,
) -> Result<FieldSpec, Error> {
    use FieldType::*;
    let upper = column.type_name.to_ascii_uppercase();
    let (field_type, takes) = match upper.as_str() {
        "CHAR" => (Character, 1..=1),
        "CICHAR" => (CiCharacter, 1..=1),
        // The names a dictionary spells them with are types too
        // (`fixtures/sql/ddprocs2.json`).
        "VARCHAR" | "VARCHARFOX" => (VarChar, 1..=1),
        "NCHAR" => (NChar, 1..=1),
        "NVARCHAR" => (NVarChar, 1..=1),
        "RAW" => (Raw, 1..=1),
        "VARBINARY" | "VARBINARYFOX" => (VarBinary, 1..=1),
        "NUMERIC" => (Numeric, 2..=2),
        "DOUBLE" => (Double, 0..=1),
        "GUID" => (Guid, 0..=0),
        "LOGICAL" => (Logical, 0..=0),
        "SHORT" | "SHORTINT" => (ShortInt, 0..=0),
        "INTEGER" => (Integer, 0..=0),
        "LONGINT" | "BIGINT" => (LongInt, 0..=0),
        "CURDOUBLE" => (CurDouble, 0..=0),
        "MONEY" => (Money, 0..=0),
        "DATE" => (Date, 0..=0),
        "TIME" => (Time, 0..=0),
        "TIMESTAMP" => (TimeStamp, 0..=0),
        "AUTOINC" => (AutoInc, 0..=0),
        "ROWVERSION" => (RowVersion, 0..=0),
        "MODTIME" => (ModTime, 0..=0),
        "MEMO" => (Memo, 0..=0),
        "BLOB" | "BINARY" => (Binary, 0..=0),
        "NMEMO" => (NMemo, 0..=0),
        _ => return Err(Error::Sql(SqlError::no_such_type(&column.type_name))),
    };
    if !takes.contains(&column.params.len()) {
        return Err(Error::Sql(SqlError::type_parameters(&upper)));
    }
    let param = |i: usize| column.params.get(i).copied().unwrap_or(0);
    let visual = flavour == Some(Flavour::Visual);
    // A Clipper or FoxPro table's VARCHAR is the legacy type.
    let legacy = matches!(flavour, Some(Flavour::Clipper | Flavour::FoxPro));
    // Only a Visual FoxPro table has the type, among the DBF ones.
    if legacy && field_type == VarBinary {
        return Err(Error::Sql(SqlError::no_such_type(&column.type_name)));
    }
    let shown_name = match legacy {
        true => column.name.to_ascii_uppercase(),
        false => column.name.clone(),
    };
    // A DBF field's name has 10 characters at most, any other 128.
    let longest = match flavour {
        Some(Flavour::Visual) | None => 128,
        Some(_) => 10,
    };
    if column.name.len() > longest {
        return Err(Error::Sql(SqlError::invalid_column_name(&column.name)));
    }
    let shown_type = match upper.as_str() {
        "VARCHAR" if !legacy => "VARCHARFOX",
        "VARBINARY" => "VARBINARYFOX",
        other => other,
    };
    let mut definition = format!("{},{shown_type},{};", column.name, param(0));
    if legacy {
        definition = definition.to_ascii_uppercase();
    }
    let (length, decimals) = match field_type {
        Character | CiCharacter | VarChar | NChar | NVarChar | Raw | VarBinary => {
            let length = param(0);
            if legacy && field_type == VarChar {
                // A legacy VarChar is 6 to 64000 long.
                if length > 64000 {
                    return Err(invalid_field(&format!(
                        "VarChar length must be at least 6. Field: \"{shown_name}\""
                    )));
                }
                return Ok(FieldSpec {
                    name: column.name.clone(),
                    field_type,
                    length: length.max(6) as u16,
                    decimals: 0,
                    compressed: column.compressed,
                });
            }
            if length == 0 {
                return Err(invalid_field(&format!(
                    "Field must be at least one character long.  The problem field definition \
                     is: \"{definition}\"."
                )));
            }
            if matches!(field_type, NChar | NVarChar) && length > 32500 {
                let which = if field_type == NChar {
                    "NChar"
                } else {
                    "NVarChar"
                };
                return Err(invalid_field(&format!(
                    "{which} fields are limited to 32500 Code Units. Field: \"{shown_name}\""
                )));
            }
            if matches!(field_type, VarChar | VarBinary)
                && length > if visual { 254 } else { 65000 }
            {
                return Err(invalid_field(&format!(
                    "Field too long.  The problem field definition is: \"{definition}\"."
                )));
            }
            if length > 65535 {
                return Err(invalid_field(&format!(
                    "Field too long.  The problem field definition is: \"{definition}\"."
                )));
            }
            (length as u16, 0)
        }
        Numeric => {
            let dbf = flavour.is_some();
            if let Some(why) = numeric_problem(dbf, param(0), param(1), &shown_name) {
                return Err(invalid_field(&why));
            }
            let length = if dbf { param(0) & 0xFF } else { param(0) };
            (length as u16, param(1) as u16)
        }
        Double => {
            let decimals = if column.params.is_empty() {
                15
            } else {
                param(0)
            };
            if let Some(why) = decimals_problem(decimals, &shown_name) {
                return Err(invalid_field(&why));
            }
            (8, (decimals & 0xFF) as u16)
        }
        CurDouble => (8, 2),
        Money => (8, 4),
        Guid => (16, 0),
        Logical => (1, 0),
        ShortInt => (2, 0),
        Integer | AutoInc | Date | Time => (4, 0),
        _ => (8, 0),
    };
    let spec = FieldSpec {
        name: column.name.clone(),
        field_type,
        length,
        decimals,
        compressed: column.compressed,
    };
    // A Visual FoxPro field's name has 10 characters, which the engine finds
    // out in its own layer, after it has taken the name of up to 128.
    if visual && column.name.len() > 10 {
        return Err(invalid_field(&format!(
            "Field name invalid.  The problem field definition is: \"{};\".",
            shown_definition(column, &spec, column.nullable == Some(true))
        )));
    }
    Ok(spec)
}

/// The DBF flavour of the tables a statement with `settings` makes and
/// opens; `None` for ADT tables.
pub(crate) fn flavour(settings: &Settings) -> Option<Flavour> {
    match settings.table_type {
        TableType::Ntx => Some(Flavour::Clipper),
        TableType::Cdx => Some(Flavour::FoxPro),
        TableType::Vfp => Some(Flavour::Visual),
        TableType::Adt | TableType::Database => None,
    }
}

/// The `AdsCreateTable` definition of the column `column` in a DBF table
/// of `flavour`, as the original engine maps SQL's types
/// (`fixtures/write/sql_dbf`): the types DBF tables cannot have are no
/// types there (2135), as AutoInc, Money, TimeStamp and VarBinary are in
/// all but VFP tables.
fn dbf_definition(column: &ColumnDef, flavour: Flavour) -> Result<String, Error> {
    // The checks of the type's parameters and lengths.
    let spec = field_spec_in(column, Some(flavour))?;
    let visual = flavour == Flavour::Visual;
    let (name, n, d) = (&column.name, spec.length, spec.decimals);
    let mut definition = match column.type_name.to_ascii_uppercase().as_str() {
        "CHAR" => format!("{name},Character,{n}"),
        "VARCHAR" => format!("{name},VarChar,{n}"),
        "NCHAR" => format!("{name},NChar,{n}"),
        "NVARCHAR" => format!("{name},NVarChar,{n}"),
        "NUMERIC" => format!("{name},Numeric,{n},{d}"),
        "DOUBLE" => format!("{name},Double,{d}"),
        "GUID" => format!("{name},GUID"),
        "LOGICAL" => format!("{name},Logical"),
        "INTEGER" => format!("{name},Integer"),
        "LONGINT" | "BIGINT" => format!("{name},LongInt"),
        "DATE" => format!("{name},Date"),
        "MEMO" => format!("{name},Memo"),
        "BLOB" | "BINARY" => format!("{name},Binary"),
        "NMEMO" => format!("{name},NMemo"),
        "AUTOINC" if visual => format!("{name},AutoInc"),
        "MONEY" if visual => format!("{name},Money"),
        "TIMESTAMP" if visual => format!("{name},TimeStamp"),
        "VARBINARY" if visual => format!("{name},VarBinary,{n}"),
        _ => return Err(Error::Sql(SqlError::no_such_type(&column.type_name))),
    };
    // Only a Visual FoxPro table takes NULL and NOCPTRANS, which
    // `create_dbf_sql` has checked.
    let nullable = visual && column.nullable == Some(true);
    if visual && column.nocptrans && !has_characters(&column.type_name) {
        return Err(invalid_field(&format!(
            "The NOCPTRANS attribute is only valid with fields containing character \
             data.The problem field definition is \"{};\".",
            shown_definition(column, &spec, nullable)
        )));
    }
    if nullable {
        definition.push_str(",NULL");
    }
    if visual && column.nocptrans {
        definition.push_str(",NOCPTRANS");
    }
    Ok(definition)
}

/// Whether a column of the SQL type `type_name` holds character data, which
/// `NOCPTRANS` is for.
fn has_characters(type_name: &str) -> bool {
    matches!(
        type_name.to_ascii_uppercase().as_str(),
        "CHAR" | "VARCHAR" | "NCHAR" | "NVARCHAR" | "MEMO" | "NMEMO"
    )
}

/// The definition of `column` as the original engine shows it in the errors
/// of its field options (`name,Type[,length[,decimals]][,NULL][,NOCPTRANS]`):
/// the type spelled as its SQL layer passes it on.
fn shown_definition(column: &ColumnDef, spec: &FieldSpec, nullable: bool) -> String {
    let (name, n, d) = (&column.name, spec.length, spec.decimals);
    let ty = match column.type_name.to_ascii_uppercase().as_str() {
        "BLOB" | "BINARY" => "BINARY".to_owned(),
        "VARBINARY" => format!("VARBINARYFOX,{n}"),
        "INTEGER" => "Integer".to_owned(),
        "BIGINT" | "LONGINT" => "LONGINTEGER".to_owned(),
        "NUMERIC" => format!("Numeric,{n},{d}"),
        "DOUBLE" => format!("DOUBLE,{d}"),
        "LOGICAL" => "LOGICAL".to_owned(),
        "DATE" => "DATE".to_owned(),
        "TIMESTAMP" => "TIMESTAMP".to_owned(),
        "AUTOINC" => "AutoInc".to_owned(),
        "MONEY" => "Money".to_owned(),
        "GUID" => "GUID".to_owned(),
        "CHAR" => format!("CHAR,{n}"),
        other => format!("{other},{n}"),
    };
    let mut text = format!("{name},{ty}");
    if nullable {
        text.push_str(",NULL");
    }
    if column.nocptrans {
        text.push_str(",NOCPTRANS");
    }
    text
}

/// The fields of a DBF table of `flavour` that SQL's `columns` define.
fn dbf_specs(columns: &[&ColumnDef], flavour: Flavour) -> Result<Vec<create::DbfFieldSpec>, Error> {
    let definitions = columns
        .iter()
        .map(|column| dbf_definition(column, flavour))
        .collect::<Result<Vec<_>, _>>()?;
    dbf_field_specs(&definitions.join(";"), flavour).map_err(|error| match error {
        Error::InvalidFieldDefinition(why) => invalid_field(&why),
        error => error,
    })
}

thread_local! {
    /// Whether the tables the thread makes and encrypts are plain ones whatever
    /// the connection's EncryptionType (the free query log's).
    static PLAIN_LAYOUT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `work` with the tables it makes laid out plain and encrypted with
/// RC4, though its connection allows AES: the free log table of query
/// logging is, as the original makes it (`fixtures/aes/api/12_querylog_aes`).
pub(crate) fn with_plain_layout<T>(work: impl FnOnce() -> T) -> T {
    let before = PLAIN_LAYOUT.replace(true);
    let result = work();
    PLAIN_LAYOUT.set(before);
    result
}

/// Whether [`with_plain_layout`] is in force.
pub(crate) fn plain_layout() -> bool {
    PLAIN_LAYOUT.get()
}

/// Whether a table made on `connection` is laid out for AES (data pushed
/// past the crypto extension, fields shifted) but not yet armed
/// (`AdsEnableEncryption` arms it): on a connection whose EncryptionType is
/// AES, and on one of a dictionary of AES tables.
pub(crate) fn lays_out_aes(session: &Session, connection: Handle) -> Result<bool, Error> {
    if plain_layout() {
        return Ok(false);
    }
    let owner = connection::get(session, connection)?;
    Ok(owner.encryption_type() != 0
        || owner
            .with_database_opt(|db| db.scheme() != tflip_storage::crypto::Scheme::Rc4)
            .unwrap_or(false))
}

/// `CREATE TABLE` on `connection`, of the type of the statement's
/// `settings`. On a dictionary connection an ADT table joins the
/// dictionary, with its field rules, memo block size and primary key,
/// unless it is `AS FREE TABLE` (`fixtures/sql/ddtables`).
pub(crate) fn create_table(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    create: &CreateTable,
) -> Result<(), Error> {
    if let Some(flavour) = flavour(settings) {
        return create_dbf_sql(session, connection, settings, create, flavour);
    }
    check_table_name(&create.table.name)?;
    let temporary = crate::temp::is_temp(&create.table.name);
    // A temporary table is a free one, whatever the connection.
    let dictionary = connection::get(session, connection)?.dictionary && !temporary;
    if let Some(pos) = create.in_database.filter(|_| !dictionary) {
        return Err(Error::Sql(SqlError::invalid_create(
            " The IN DATABASE option is not valid on free connection.",
            pos,
        )));
    }
    let path = match temporary {
        true => temp_path(session, connection, settings, &create.table.name)?,
        false => file_path(session, connection, &create.table.name, "adt")?,
    };
    check_path(&path)?;
    let mut specs: Vec<FieldSpec> = Vec::with_capacity(create.columns.len());
    let mut autoinc = false;
    for column in &create.columns {
        let mut spec = field_spec(column)?;
        // A name taken already gets `_1`, `_2`, ... as the original engine
        // renames it.
        let taken = |name: &str, specs: &[FieldSpec]| {
            specs
                .iter()
                .any(|s: &FieldSpec| s.name.eq_ignore_ascii_case(name))
        };
        if taken(&spec.name, &specs) {
            let base = spec.name.clone();
            let mut n = 1;
            while taken(&format!("{base}_{n}"), &specs) {
                n += 1;
            }
            spec.name = format!("{base}_{n}");
        }
        if spec.field_type == FieldType::AutoInc {
            if autoinc {
                return Err(invalid_field(&format!(
                    "Only one auto-increment field allowed per table. Field: \"{}\"",
                    column.name
                )));
            }
            autoinc = true;
        }
        specs.push(spec);
    }
    if create::adt_record_length(&specs) > 65535 {
        return Err(invalid_field(RECORD_TOO_LONG));
    }
    if !dictionary {
        refuse_free_rules(&create.columns)?;
    }
    let key_columns = key_columns(
        create,
        specs.iter().map(|s| (s.name.as_str(), s.field_type)),
    )?;
    if path.exists() {
        return Err(Error::Sql(SqlError::table_exists()));
    }
    let mut in_dictionary = dictionary && !create.free;
    let mut free = create.free;
    // A user who is not the administrator, a member of `DB:Admin` too, makes
    // a free table of every table not said to be IN DATABASE, the right to
    // create tables held or not (`fixtures/sql/ddrights`, `ddadminpk`).
    if in_dictionary
        && connection::get(session, connection)?
            .with_database_opt(|db| db.creator())
            .flatten()
            .is_some()
    {
        if create.in_database.is_none() {
            in_dictionary = false;
            free = true;
        } else if !rights::may_create(&*connection::get(session, connection)?, TABLE) {
            return Err(rights::refuse(
                "Cannot create table object in the data dictionary.",
            ));
        }
    }
    refuse_free_in_dictionary(create, dictionary, free)?;
    refuse_free_creation(session, connection, create)?;
    if in_dictionary {
        refuse_view_name(session, connection, &create.table.name)?;
    }
    let block_size = create.block_size.map_or(BLOCK_SIZE, |n| n as u16);
    match lays_out_aes(session, connection)? {
        true => create::aes_adt_table(&path, &specs, block_size),
        false => create::adt_table(&path, &specs, block_size),
    }
    .map_err(|_| Error::CorruptTable)?;
    if create.ignore_transactions {
        set_file_transaction_free(&path, true)?;
    }
    let table_name = table_stem(&path);
    let tag = create.primary_key.as_ref().map(|key| match &key.name {
        Some(name) => name.clone(),
        None => format!("PK_{}", table_name.to_ascii_uppercase()),
    });
    if let (Some(key), Some(tag)) = (&create.primary_key, &tag) {
        let mut options = index_options::UNIQUE | index_options::COMPOUND;
        if key
            .columns
            .first()
            .is_some_and(|(_, descending)| *descending)
        {
            options |= index_options::DESCENDING;
        }
        let expression = key_columns.join(";");
        let order = Order {
            table: &create.table.name,
            file: None,
            tag,
            expression: &expression,
            condition: "",
            options,
            page_size: 0,
        };
        match in_dictionary {
            // Not in the dictionary yet, the table is opened by its file.
            true => {
                let opened = Arc::new(table::transient(
                    session,
                    &path,
                    tflip_storage::table::Format::Adt,
                )?);
                create_order_on(session, connection, &opened, &order, false)?;
            }
            false => create_order(session, connection, settings, &order)?,
        }
    }
    if !in_dictionary {
        return Ok(());
    }
    let file = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let index_file = format!("{table_name}.adi");
    let created = crate::dictionary_write::Created {
        name: &create.table.name,
        table_type: TableType::Adt,
        char_type: 1,
        file: &file,
        rules: create.columns.iter().map(|c| &c.rules).collect(),
        block_size: create.block_size.map(|n| n as u16),
        primary_key: tag.is_some().then_some(index_file.as_str()),
        trans_free: create.ignore_transactions,
    };
    // The table was opened and closed to make it, before the dictionary
    // had it.
    crate::trigger::created(session, connection, &create.table.name, &path);
    if crate::dictionary_write::add_created(session, connection, &created)? {
        return Ok(());
    }
    rights::grant_creator(session, connection, TABLE, &create.table.name)
}

/// 5160, a rule a free table cannot have: a free connection's table takes
/// none, but the original engine lets a lone MINIMUM by
/// (`fixtures/sql/ddtables`, `fixtures/sql/dbfpk`).
fn refuse_free_rules(columns: &[ColumnDef]) -> Result<(), Error> {
    let refused = |rules: &FieldRules| {
        rules.not_null
            || rules.default.is_some()
            || rules.maximum.is_some()
            || rules.message.is_some()
    };
    match columns.iter().find(|c| refused(&c.rules)) {
        Some(column) => Err(Error::Sql(SqlError::ace(
            5160,
            "The field-level or record-level constraint is invalid.",
            &format!(
                "Field level constraints are not supported on free table. Column name: {}.",
                column.name
            ),
        ))),
        None => Ok(()),
    }
}

/// A table is refused the name of a view or a procedure of its dictionary,
/// before any file is made (`fixtures/sql/ddviews.json`, `ddprocs.json`).
fn refuse_view_name(session: &Session, connection: Handle, name: &str) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    let owner = connection::get(session, connection)?;
    let taken = owner
        .with_database_opt(|db| {
            let dict = db.dictionary();
            (
                dict.named(Kind::View, name).is_some(),
                dict.named(Kind::Procedure, name).is_some()
                    || crate::dictionary_write::function_or_package(dict, name),
            )
        })
        .unwrap_or_default();
    match taken {
        (true, _) => Err(crate::dictionary_write::not_valid(
            "View already exists in the dictionary with the same name.",
        )),
        (_, true) => Err(crate::dictionary_write::name_taken(&owner.path)),
        _ => Ok(()),
    }
}

/// 7087, a free table made on a connection that may not open them
/// (`fixtures/dictionary/freetables.json`).
fn refuse_free_creation(
    session: &Session,
    connection: Handle,
    create: &CreateTable,
) -> Result<(), Error> {
    if create.free && connection::get(session, connection)?.refuses_free_tables() {
        return Err(Error::Sql(SqlError::ace(
            7087,
            "Cannot open table or view for this operation because the requested permissions have not been \
             granted to the user.",
            "",
        )));
    }
    Ok(())
}

/// 5132, a primary key or rules for a table `AS FREE TABLE` on a dictionary
/// connection: the original engine looks for it in the dictionary to give it
/// them.
fn refuse_free_in_dictionary(
    create: &CreateTable,
    dictionary: bool,
    free: bool,
) -> Result<(), Error> {
    if dictionary
        && free
        && (create.primary_key.is_some() || create.columns.iter().any(|c| c.rules.any()))
    {
        return Err(crate::dictionary_write::not_valid(
            "There is no such named table in the data dictionary.",
        ));
    }
    Ok(())
}

/// The primary key's columns of `create`, by the names `fields` (name and
/// type) took; none if it has no key.
fn key_columns<'a>(
    create: &CreateTable,
    fields: impl Iterator<Item = (&'a str, FieldType)> + Clone,
) -> Result<Vec<&'a str>, Error> {
    let Some(key) = &create.primary_key else {
        return Ok(Vec::new());
    };
    key.columns
        .iter()
        .map(|(column, _)| {
            let (name, field_type) = fields
                .clone()
                .find(|(name, _)| name.eq_ignore_ascii_case(&column.name))
                .ok_or_else(|| Error::Sql(SqlError::primary_key_column(&column.name)))?;
            match field_type {
                FieldType::Memo | FieldType::Binary | FieldType::Image | FieldType::NMemo => {
                    Err(Error::Sql(SqlError::key_column_type(&column.name)))
                }
                _ => Ok(name),
            }
        })
        .collect()
}

/// [`create_table`] of a DBF table of `flavour`. A Visual FoxPro table takes
/// NULL columns and a primary key (a candidate order of the structural index
/// file, as `ADT` has its own), and on a dictionary connection the rules of
/// its fields; the others refuse them (`fixtures/sql/dbfpk`,
/// `fixtures/sql/ddbf`). On a dictionary connection the table joins it,
/// unless it is `AS FREE TABLE`.
fn create_dbf_sql(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    create: &CreateTable,
    flavour: Flavour,
) -> Result<(), Error> {
    let visual = flavour == Flavour::Visual;
    let temporary = crate::temp::is_temp(&create.table.name);
    let dictionary = connection::get(session, connection)?.dictionary && !temporary;
    if let Some(column) = create
        .columns
        .iter()
        .find(|c| (c.nullable.is_some() || c.nocptrans) && !visual)
    {
        return Err(Error::Sql(SqlError::invalid_create(
            " NOCPTRANS and nullable column type options are only valid with ADS_VFP table \
             type.",
            column.name_pos,
        )));
    }
    check_table_name(&create.table.name)?;
    let columns: Vec<&ColumnDef> = create.columns.iter().collect();
    let specs = dbf_specs(&columns, flavour)?;
    if create::dbf_record_length(flavour, &specs) > 65535 {
        return Err(invalid_field(RECORD_TOO_LONG));
    }
    if create.primary_key.is_some() && !visual {
        return Err(Error::Sql(SqlError::ace(
            5042,
            "The requested feature has not yet been implemented.",
            "Primary keys are only supported with ADT and VFP table types.",
        )));
    }
    if !dictionary {
        refuse_free_rules(&create.columns)?;
    }
    let mut in_dictionary = dictionary && !create.free;
    let mut free = create.free;
    // A user who is not the administrator makes a free table of every table
    // not said to be IN DATABASE, and needs the right to create tables for
    // one that is (`fixtures/sql/ddcreatedbf`).
    if in_dictionary
        && connection::get(session, connection)?
            .with_database_opt(|db| db.creator())
            .flatten()
            .is_some()
    {
        if create.in_database.is_none() {
            in_dictionary = false;
            free = true;
        } else if !rights::may_create(&*connection::get(session, connection)?, TABLE) {
            return Err(rights::refuse(
                "Cannot create table object in the data dictionary.",
            ));
        }
    }
    refuse_free_in_dictionary(create, dictionary, free)?;
    refuse_free_creation(session, connection, create)?;
    // A NOT NULL rule is the dictionary's to keep, and a Visual FoxPro field
    // alone has the NULL it takes away: the other tables are refused it once
    // they have joined the dictionary, and go again (the object ids stay
    // taken).
    let refused = in_dictionary && !visual && create.columns.iter().any(|c| c.rules.not_null);
    let key_columns = key_columns(
        create,
        specs
            .iter()
            .map(|s| (s.spec.name.as_str(), s.spec.field_type)),
    )?;
    let path = match temporary {
        true => temp_path(session, connection, settings, &create.table.name)?,
        false => file_path(session, connection, &create.table.name, "dbf")?,
    };
    check_path(&path)?;
    if path.exists() {
        return Err(Error::Sql(SqlError::table_exists()));
    }
    if in_dictionary {
        refuse_view_name(session, connection, &create.table.name)?;
        if crate::dictionary_write::rc4_tables::refuses_dbf_without_strong_encryption(
            &*connection::get(session, connection)?,
            settings.table_type,
        ) {
            return Err(Error::Sql(SqlError::ace(
                7168,
                "The table format does not support strong encryption.",
                "",
            )));
        }
    }
    let memo_size = create.block_size.map_or(0, |n| n as u16);
    let aes = !temporary && flavour == Flavour::Visual && aes_layout(session, connection);
    write_dbf(&path, flavour, &specs, memo_size, settings.char_type, aes)?;
    if create.ignore_transactions {
        set_file_transaction_free(&path, true)?;
    }
    let table_name = table_stem(&path);
    let index_file = format!("{table_name}.cdx");
    if let Some(key) = &create.primary_key {
        let tag = match &key.name {
            Some(name) => name.clone(),
            None => format!("PK_{}", table_name.to_ascii_uppercase()),
        };
        let mut options = index_options::CANDIDATE | index_options::COMPOUND;
        if key
            .columns
            .first()
            .is_some_and(|(_, descending)| *descending)
        {
            options |= index_options::DESCENDING;
        }
        let expression = key_columns.join(";");
        let order = Order {
            table: &table_name,
            file: None,
            tag: &tag,
            expression: &expression,
            condition: "!Deleted()",
            options,
            page_size: 0,
        };
        // The table is not in the dictionary yet, so it opens by its file.
        let order = Order {
            table: &format!("{table_name}.dbf"),
            ..order
        };
        if let Err(error) = create_order(session, connection, settings, &order) {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(path.with_extension("cdx"));
            let _ = std::fs::remove_file(tflip_storage::dbf::memo_path(
                &path,
                tflip_storage::memo::MemoKind::Fpt,
            ));
            return Err(error);
        }
    }
    if !in_dictionary {
        return Ok(());
    }
    let file = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let created = crate::dictionary_write::Created {
        name: &create.table.name,
        table_type: settings.table_type,
        char_type: settings.char_type,
        file: &file,
        rules: create.columns.iter().map(|c| &c.rules).collect(),
        block_size: create.block_size.map(|n| n as u16),
        primary_key: create.primary_key.is_some().then_some(index_file.as_str()),
        trans_free: create.ignore_transactions,
    };
    if !crate::dictionary_write::add_created(session, connection, &created)? {
        rights::grant_creator(session, connection, TABLE, &create.table.name)?;
    }
    if refused {
        crate::dictionary_write::remove_table_api(session, connection, &create.table.name, true)?;
        return Err(Error::Sql(SqlError::ace(
            5042,
            "The requested feature has not yet been implemented.",
            "Cannot set property for the current table type.",
        )));
    }
    Ok(())
}

/// Whether keys of a field of type `t` are text, padded with blanks.
fn is_text(t: FieldType) -> bool {
    matches!(
        t,
        FieldType::Character | FieldType::CiCharacter | FieldType::VarChar
    )
}

fn table_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Sets or clears the transaction-free mark of the table file `path`: an
/// ADT table's header flag, a DBF table's bit in its header.
pub(crate) fn set_file_transaction_free(path: &Path, free: bool) -> Result<(), Error> {
    use tflip_storage::table::{Format, TableFile};
    let adt = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("adt"));
    if adt {
        let mut file =
            TableFile::open_for_writing(path, Format::Adt).map_err(|_| Error::CorruptTable)?;
        return file
            .set_transaction_free(free)
            .map_err(|_| Error::CorruptTable);
    }
    let mut bytes = std::fs::read(path).map_err(|_| Error::CorruptTable)?;
    let mark = bytes.get_mut(0x1E).ok_or(Error::CorruptTable)?;
    *mark = match free {
        true => *mark | 0x02,
        false => *mark & !0x02,
    };
    std::fs::write(path, bytes).map_err(|_| Error::CorruptTable)
}

/// The DBF flavour of the dictionary's table `table`; `None` for an ADT
/// table and for what the dictionary has no table of. A statement that sets
/// no table type works on the type the dictionary keeps.
fn dictionary_flavour(session: &Session, connection: Handle, table: &str) -> Option<Flavour> {
    let owner = connection::get(session, connection).ok()?;
    let bound = owner
        .with_database_opt(|database| database.table(table))
        .flatten()?;
    match bound.table_type {
        TableType::Ntx => Some(Flavour::Clipper),
        TableType::Cdx => Some(Flavour::FoxPro),
        TableType::Vfp => Some(Flavour::Visual),
        TableType::Adt | TableType::Database => None,
    }
}

/// The DBF flavour of the table `table` a statement with `settings` works
/// on: a dictionary's table has the type the dictionary keeps, whatever type
/// the statement was given; any other the statement's.
fn table_flavour(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    table: &str,
) -> Option<Flavour> {
    match in_dictionary(session, connection, table) {
        true => dictionary_flavour(session, connection, table),
        false => flavour(settings),
    }
}

/// Whether `table` is one of the dictionary `connection` is to.
pub(crate) fn in_dictionary(session: &Session, connection: Handle, table: &str) -> bool {
    let Ok(owner) = connection::get(session, connection) else {
        return false;
    };
    owner.dictionary
        && owner
            .with_database_opt(|db| {
                db.dictionary()
                    .named(tflip_storage::dd::Kind::Table, table)
                    .is_some()
            })
            .unwrap_or(false)
}

/// The name of the index file `file` of `table` as the dictionary spells it
/// (without its extension), `file` itself if it has none such.
fn dictionary_file_spelling(
    session: &Session,
    connection: Handle,
    table: &str,
    file: &str,
) -> String {
    use tflip_storage::dd::Kind;
    let Ok(owner) = connection::get(session, connection) else {
        return file.to_owned();
    };
    owner
        .with_database_opt(|db| {
            let dictionary = db.dictionary();
            let table = dictionary.named(Kind::Table, table)?;
            dictionary
                .children(table.id, Kind::IndexFile)
                .iter()
                .find_map(|object| {
                    let stem = object
                        .name
                        .rsplit_once('.')
                        .map_or(&*object.name, |(s, _)| s);
                    stem.eq_ignore_ascii_case(file).then(|| stem.to_owned())
                })
        })
        .flatten()
        .unwrap_or_else(|| file.to_owned())
}

/// Whether a full-text index can be made on a field of type `field_type`.
pub(crate) fn can_have_full_text(field_type: FieldType) -> bool {
    use FieldType::*;
    matches!(
        field_type,
        Character
            | CiCharacter
            | LegacyVarChar
            | VarChar
            | NChar
            | NVarChar
            | Memo
            | NMemo
            | Binary
            | Image
            | VarBinary
    )
}

/// Whether the dictionary `connection` is to has an order `tag` for `table`.
fn dictionary_has_order(session: &Session, connection: Handle, table: &str, tag: &str) -> bool {
    use tflip_storage::dd::Kind;
    let Ok(owner) = connection::get(session, connection) else {
        return false;
    };
    owner
        .with_database_opt(|db| {
            let dictionary = db.dictionary();
            dictionary.named(Kind::Table, table).is_some_and(|table| {
                dictionary
                    .children(table.id, Kind::Index)
                    .iter()
                    .any(|key| key.name.eq_ignore_ascii_case(tag))
            })
        })
        .unwrap_or(false)
}

/// 5042, for what a local server does not do (online maintenance).
fn not_on_local(what: &str) -> Error {
    Error::Sql(SqlError::ace(
        5042,
        "The requested feature has not yet been implemented.",
        what,
    ))
}

/// The refusals of `CREATE INDEX`'s options, in the order the original
/// engine makes them: an index file for a table that is not in a
/// dictionary, a page size out of range, `ONLINE` on a local server, and
/// `CANDIDATE` on a table type that has no such order.
fn check_index_options(
    session: &Session,
    connection: Handle,
    create: &CreateIndex,
    flavour: Option<Flavour>,
) -> Result<(), Error> {
    if create.file.is_some() && !in_dictionary(session, connection, &create.table.name) {
        return Err(Error::Sql(SqlError::ace(
            5184,
            "This operation is only allowed on tables belonging to a data dictionary.",
            "The IN FILE option cannot be used with free tables.",
        )));
    }
    if create
        .page_size
        .is_some_and(|n| n != 0 && !(512..=8192).contains(&n))
    {
        return Err(Error::Sql(SqlError::ace(
            5019,
            "An invalid option was specified.",
            "Parameter 8.  Page size must be 512..8192",
        )));
    }
    // A server (tflipd) is what the original engine's remote server is.
    if create.online && session.shares.is_none() {
        return Err(not_on_local(
            "Online create index is not supported with local server.",
        ));
    }
    if create.candidate && matches!(flavour, Some(Flavour::Clipper | Flavour::FoxPro)) {
        return Err(Error::Sql(SqlError::ace(
            5019,
            "An invalid option was specified.",
            "ADS_CANDIDATE is not a valid option for this table type.",
        )));
    }
    Ok(())
}

/// The page size `CREATE INDEX` asks for, 0 for the default: the one the keys
/// need in a new ADI file (see [`fit_page_size`]).
fn index_page_size(create: &CreateIndex) -> u16 {
    create.page_size.unwrap_or(0) as u16
}

/// A key too long for the order's index file is the SQL layer's 2011.
fn key_too_long(error: Error) -> Error {
    match error {
        Error::Detailed(inner, _) if matches!(*inner, Error::InvalidKeyLength) => {
            Error::Sql(SqlError::index_expression())
        }
        error => error,
    }
}

/// `CREATE INDEX` on `connection`, on a table of the type of the
/// statement's `settings`. On a dictionary connection an index of a table of
/// the dictionary joins it (`fixtures/sql/ddoptsdict`).
pub(crate) fn create_index(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    create: &CreateIndex,
) -> Result<(), Error> {
    // An index of 15 columns at most, and its keys: the engine's own refusal
    // is its layer's.
    if create.columns.len() > 15 {
        return Err(Error::Sql(SqlError::too_many_key_columns()));
    }
    // The table is closed to other connections meanwhile, unless `ONLINE`.
    let kind = match create.online {
        true => crate::maintenance::Kind::Online,
        false => crate::maintenance::Kind::Offline,
    };
    let gate = maintain(
        session,
        connection,
        settings,
        &create.table.name,
        kind,
        "__OCIFinalStage_",
    )?;
    if let Some(flavour) = table_flavour(session, connection, settings, &create.table.name) {
        if create.content.is_some() {
            return create_dbf_full_text(session, connection, settings, create, flavour);
        }
        return create_dbf_index(session, connection, settings, create, flavour)
            .map_err(key_too_long);
    }
    rights::require_table(
        session,
        connection,
        &create.table.name,
        rights::ALTER,
        "Must have alter table permission to create indexes in the database.",
    )?;
    if create.name.name.len() > 128 {
        return Err(Error::Sql(SqlError::invalid_index_name(&create.name.name)));
    }
    let expression: Vec<&str> = create
        .columns
        .iter()
        .map(|(c, _)| c.name.as_str())
        .collect();
    let expression = expression.join(";");
    // A candidate order is a unique one in an ADT index file.
    let mut options = index_options::COMPOUND;
    if create.unique || create.candidate {
        options |= index_options::UNIQUE;
    }
    if create
        .columns
        .first()
        .is_some_and(|(_, descending)| *descending)
    {
        options |= index_options::DESCENDING;
    }
    // An index file the dictionary has for the table is the one named, in
    // whatever case.
    let file = create
        .file
        .as_deref()
        .map(|file| dictionary_file_spelling(session, connection, &create.table.name, file));
    let order = Order {
        table: &create.table.name,
        file: file.as_deref(),
        tag: &create.name.name,
        expression: &expression,
        condition: "",
        options,
        page_size: index_page_size(create),
    };
    with_table(session, connection, settings, order.table, |opened| {
        for (column, _) in &create.columns {
            let field = opened
                .fields()
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(&column.name))
                .ok_or_else(|| Error::Sql(SqlError::column_not_found(&column.name, column.pos)))?;
            if field.is_memo() && create.content.is_none() {
                return Err(Error::Sql(SqlError::index_expression()));
            }
            if create.content.is_some() && !can_have_full_text(field.field_type) {
                return Err(Error::Sql(SqlError::ace(
                    5066,
                    "The requested operation is not legal for the given field type.",
                    "FTS indexes can only be created on character and BLOB fields.",
                )));
            }
        }
        // The name is taken in the table's structural index file, in the
        // file asked for, or by an order of any of the dictionary's index
        // files for the table.
        let files = [
            Some(opened.path.with_extension("adi")),
            order_file(session, connection, opened, order.file).ok(),
        ];
        let taken_in_files = files.into_iter().flatten().any(|path| {
            tflip_storage::order::IndexFile::open(&path).is_ok_and(|file| {
                file.tags("")
                    .iter()
                    .any(|t| t.name.eq_ignore_ascii_case(order.tag))
            })
        });
        if taken_in_files || dictionary_has_order(session, connection, order.table, order.tag) {
            return Err(Error::Sql(SqlError::ace_bare(
                5076,
                "An index with the same name already exists.",
            )));
        }
        check_index_options(session, connection, create, None)
    })?;
    match create.content {
        Some(_) => {
            // SQL gives its noise words in lower case (`fixtures/write/fts_build`).
            let mut full_text = create.full_text.clone();
            if let Some((_, noise)) = &mut full_text.noise {
                *noise = noise.to_ascii_lowercase();
            }
            let block = tflip_sql::fts::index_block(&full_text).map_err(|bad| {
                Error::Sql(SqlError::ace(
                    5019,
                    "An invalid option was specified.",
                    &format!("Parameter {}.  {}", bad.parameter, bad.why),
                ))
            })?;
            let column = &create.columns[0].0.name;
            with_table(session, connection, settings, order.table, |opened| {
                let field = opened
                    .fields()
                    .iter()
                    .position(|f| f.name.eq_ignore_ascii_case(column))
                    .ok_or(Error::NoSuchField(column.clone()))?;
                match create_full_text_on(
                    session, connection, opened, &order, field, block, false, true,
                )? {
                    true => Ok(()),
                    false => Err(Error::Sql(SqlError::ace_bare(
                        5076,
                        "An index with the same name already exists.",
                    ))),
                }
            })?;
        }
        None => create_order_with(session, connection, settings, &order, gate.as_ref())
            .map_err(key_too_long)?,
    }
    if in_dictionary(session, connection, &create.table.name) {
        crate::dictionary_write::register_created_index(
            session,
            connection,
            &create.table.name,
            order.file,
            &create.name.name,
            create
                .content
                .map(|_| tflip_sql::fts::dictionary_noise(&create.full_text))
                .as_deref(),
        )?;
    }
    Ok(())
}

/// `CREATE INDEX … CONTENT` on a FoxPro or VFP table: a full-text order in
/// its structural `.cdx` (`fixtures/write/fts_cdx`). A Clipper table has
/// none, as the original engine says.
fn create_dbf_full_text(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    create: &CreateIndex,
    flavour: Flavour,
) -> Result<(), Error> {
    let tag = &create.name.name;
    if tag.len() > 10 {
        return Err(Error::Sql(SqlError::invalid_index_name(tag)));
    }
    if flavour == Flavour::Clipper {
        return Err(Error::Sql(SqlError::ace(
            5017,
            "The index type is not valid for the requested operation.",
            "FTS indexes cannot be created with NTX indexes.",
        )));
    }
    let mut full_text = create.full_text.clone();
    if let Some((_, noise)) = &mut full_text.noise {
        *noise = noise.to_ascii_lowercase();
    }
    let block = tflip_sql::fts::index_block(&full_text).map_err(|bad| {
        Error::Sql(SqlError::ace(
            5019,
            "An invalid option was specified.",
            &format!("Parameter {}.  {}", bad.parameter, bad.why),
        ))
    })?;
    let column = &create.columns[0].0;
    with_table(
        session,
        connection,
        settings,
        &create.table.name,
        |opened| {
            let field = opened
                .fields()
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(&column.name))
                .ok_or_else(|| Error::Sql(SqlError::column_not_found(&column.name, column.pos)))?;
            if !can_have_full_text(opened.fields()[field].field_type) {
                return Err(Error::Sql(SqlError::ace(
                    5066,
                    "The requested operation is not legal for the given field type.",
                    "FTS indexes can only be created on character and BLOB fields.",
                )));
            }
            let order = Order {
                table: &create.table.name,
                file: create.file.as_deref(),
                tag,
                expression: "",
                condition: "",
                options: 0,
                page_size: 0,
            };
            match create_full_text_on(
                session, connection, opened, &order, field, block, false, true,
            )? {
                true => Ok(()),
                false => Err(Error::Sql(SqlError::ace_bare(
                    5076,
                    "An index with the same name already exists.",
                ))),
            }
        },
    )?;
    if in_dictionary(session, connection, &create.table.name) {
        crate::dictionary_write::register_created_index(
            session,
            connection,
            &create.table.name,
            create.file.as_deref(),
            tag,
            Some(&tflip_sql::fts::dictionary_noise(&create.full_text)),
        )?;
    }
    Ok(())
}

/// [`create_index`] on a DBF table of `flavour`: an order of the table's
/// structural `.cdx`, or a Clipper table's `.ntx` named after it. The key
/// of several columns is their text one after the other, a date's as
/// `DtoS()`, as the original engine writes it.
fn create_dbf_index(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    create: &CreateIndex,
    flavour: Flavour,
) -> Result<(), Error> {
    let tag = &create.name.name;
    if tag.len() > 10 {
        return Err(Error::Sql(SqlError::invalid_index_name(tag)));
    }
    let expression = with_table(
        session,
        connection,
        settings,
        &create.table.name,
        |opened| {
            let mut fields = Vec::new();
            for (column, _) in &create.columns {
                let field = opened
                    .fields()
                    .iter()
                    .find(|f| f.name.eq_ignore_ascii_case(&column.name))
                    .ok_or_else(|| {
                        Error::Sql(SqlError::column_not_found(&column.name, column.pos))
                    })?;
                if field.is_memo() {
                    return Err(Error::Sql(SqlError::index_expression()));
                }
                fields.push(field);
            }
            check_index_options(session, connection, create, Some(flavour))?;
            if let [field] = fields.as_slice() {
                return Ok(field.name.clone());
            }
            let parts = fields.iter().map(|field| match field.field_type {
                FieldType::Character => Ok(field.name.clone()),
                FieldType::Date => Ok(format!("DtoS({})", field.name)),
                _ => Err(Error::Sql(SqlError::index_expression())),
            });
            Ok(parts.collect::<Result<Vec<_>, _>>()?.join("+"))
        },
    )?;
    let clipper = flavour == Flavour::Clipper;
    let mut options = if clipper { 0 } else { index_options::COMPOUND };
    if create.unique {
        options |= index_options::UNIQUE;
    }
    if create.candidate {
        options |= index_options::CANDIDATE;
    }
    if create
        .columns
        .first()
        .is_some_and(|(_, descending)| *descending)
    {
        options |= index_options::DESCENDING;
    }
    let file = match clipper {
        true => Some(tag.as_str()),
        false => create.file.as_deref(),
    };
    let order = Order {
        table: &create.table.name,
        file,
        tag,
        expression: &expression,
        // A Clipper table's index has no such condition to keep.
        condition: if create.not_deleted && !clipper {
            "!Deleted()"
        } else {
            ""
        },
        options,
        page_size: index_page_size(create),
    };
    create_order(session, connection, settings, &order)?;
    if in_dictionary(session, connection, &create.table.name) {
        crate::dictionary_write::register_created_index(
            session,
            connection,
            &create.table.name,
            order.file,
            tag,
            None,
        )?;
    }
    Ok(())
}

/// An order to create: on the table `table`, in the index file `file` (the
/// table's structural one if `None`), named `tag`, with the key
/// `expression` and the `FOR` condition `condition` (fields by their
/// names), `AdsCreateIndex` `options`, in pages of `page_size` bytes (0 for
/// the default, which a new ADI file takes as its keys need).
pub(crate) struct Order<'a> {
    pub table: &'a str,
    pub file: Option<&'a str>,
    pub tag: &'a str,
    pub expression: &'a str,
    pub condition: &'a str,
    pub options: u32,
    pub page_size: u16,
}

/// Opens the table `name` as SQL opens a table it names, with the
/// statement's `settings`.
fn open_named(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    name: &str,
) -> Result<Handle, Error> {
    // A free connection does not open a table of a dictionary.
    if flavour(settings).is_none() && !connection::get(session, connection)?.dictionary {
        let path = file_path(session, connection, name, settings.extension())?;
        if tflip_storage::dd::create::is_bound(&path).unwrap_or(false) {
            return Err(Error::Sql(SqlError::ace(
                5159,
                "Error encountered when trying to open a database table.",
                &format!("{} Table name: {name}", table::DATABASE_TABLE),
            )));
        }
    }
    match table::open(session, connection, name, settings.table_named(0, name))
        .map_err(|error| crate::statement::open_error(error, name))
    {
        Err(Error::FileNotFound) if crate::temp::is_temp(name) => {
            Err(Error::Sql(crate::temp::not_found(name, true)))
        }
        Err(Error::FileNotFound) => {
            let path = file_path(session, connection, name, settings.extension())?;
            Err(Error::Sql(SqlError::ace(
                5004,
                "Either ACE could not find the specified file, or you do not have \
                 sufficient rights to access the file.",
                &format!("{} Table name: {name}", path.display()),
            )))
        }
        opened => opened,
    }
}

/// Where the table `name` is, for the maintenance of it: a dictionary's
/// table is where the dictionary has it. `None` for a temporary table, or
/// one whose file cannot be named.
pub(crate) fn table_path(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    name: &str,
) -> Option<std::path::PathBuf> {
    if crate::temp::is_temp(name) {
        return None;
    }
    let bound = connection::get(session, connection)
        .ok()
        .and_then(|owner| owner.with_database_opt(|database| database.table(name)))
        .flatten();
    match bound {
        Some(bound) => Some(bound.path),
        None => file_path(session, connection, name, settings.extension()).ok(),
    }
}

/// Starts the maintenance of the table `name` on a server, which closes it
/// to the other sessions unless `online` (see [`crate::maintenance`]).
pub(crate) fn maintain(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    name: &str,
    kind: crate::maintenance::Kind,
    event: &str,
) -> Result<Option<crate::maintenance::Guard>, Error> {
    let Some(path) = table_path(session, connection, settings, name) else {
        return Ok(None);
    };
    crate::maintenance::begin(session, connection, &path, kind, format!("{event}{name}"))
}

/// Opens the table `name` for a moment (see [`open_named`]).
fn with_table<T>(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    name: &str,
    f: impl FnOnce(&Arc<table::Table>) -> Result<T, Error>,
) -> Result<T, Error> {
    let handle = open_named(session, connection, settings, name)?;
    let result = table::get(session, handle).and_then(|opened| f(&opened));
    let _ = table::close(session, handle);
    result
}

/// Creates `order` (see [`Order`]) and gives it the keys of the table's
/// records. On a DBF table, as `AdsCreateIndex` creates it: without a file,
/// a compound order goes in the structural `.cdx`, another in an `.idx`
/// named after the table, and a Clipper table's in an `.ntx` named after
/// it.
pub(crate) fn create_order(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    order: &Order,
) -> Result<(), Error> {
    create_order_with(session, connection, settings, order, None)
}

/// [`create_order`] as the maintenance `online` runs it, if it does (see
/// [`crate::maintenance`]).
pub(crate) fn create_order_with(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    order: &Order,
    online: Option<&crate::maintenance::Guard>,
) -> Result<(), Error> {
    if let Some(flavour) = table_flavour(session, connection, settings, order.table) {
        let handle = open_named(session, connection, settings, order.table)?;
        let created = table::get(session, handle).and_then(|opened| {
            let stem = table_stem(&opened.path);
            let file = match (flavour, order.file) {
                (Flavour::Clipper, None) => Some(stem.as_str()),
                (_, file) => file,
            };
            let page_size = u32::from(order.page_size);
            let (tag, expression, condition) = (order.tag, order.expression, order.condition);
            index::create(
                session,
                handle,
                file,
                tag,
                expression,
                condition,
                order.options,
                page_size,
            )
        });
        let _ = table::close(session, handle);
        return created.map(drop);
    }
    with_table(
        session,
        connection,
        settings,
        order.table,
        |opened| match create_order_on_with(session, connection, opened, order, false, online)? {
            true => Ok(()),
            false => Err(Error::Sql(SqlError::ace_bare(
                5076,
                "An index with the same name already exists.",
            ))),
        },
    )
}

/// A limit of the original engine, as the error `code` with its `text`.
pub(crate) fn limit(code: u32, text: &str) -> Error {
    Error::Constraint {
        code,
        text: text.to_owned(),
    }
}

/// What the original engine refuses of an order's names and expressions
/// (`fixtures/write/limits`): a tag name over 128 characters, and a key
/// expression and condition of more than 510 characters together.
fn check_order_names(order: &Order) -> Result<(), Error> {
    if order.tag.len() > 128 {
        return Err(limit(5050, "The name given was too long.  Parameter 3"));
    }
    if order.expression.len() + order.condition.len() > 510 {
        return Err(Error::NoSuchField("Index expressions too long.".into()));
    }
    Ok(())
}

/// Makes room for `order`'s keys of `key_length` bytes in its ADI file, and
/// gives the page size to create the file with: a file that is there with
/// pages too small for them grows to bigger pages (every order of it built
/// again), a new file takes the page size asked for, or refuses the keys
/// (5188), or with the default, the smallest its keys leave room in (see
/// [`create::grown_page_size`]). The file takes at most 256 orders.
fn fit_page_size(
    opened: &Arc<table::Table>,
    path: &Path,
    order: &Order,
    key_length: u16,
    replace: bool,
) -> Result<u16, Error> {
    let Ok(file) = tflip_storage::order::IndexFile::open(path) else {
        return match order.page_size {
            0 => Ok(create::grown_page_size(key_length, PAGE_SIZE)),
            size if create::key_fits(key_length, size) => Ok(size),
            _ => Err(limit(
                5188,
                "The index key size is too large for the index file page size.  Specify a \
                 larger page size or use the default.",
            )),
        };
    };
    let tags = file.tags("");
    let named = tags.iter().any(|t| t.name.eq_ignore_ascii_case(order.tag));
    if tags.len() >= 256 && !(replace && named) {
        return Err(limit(
            5046,
            "The maximum number of tags that Advantage supports per compound index is 256.",
        ));
    }
    let page = file.page_size() as u16;
    if !create::key_fits(key_length, page) {
        crate::index::rebuild_file(
            opened,
            path,
            Some(create::grown_page_size(key_length, page)),
            None,
        )?;
    }
    Ok(page)
}

/// Creates `order` on the open table `opened` (the table `order` names is
/// ignored) and builds it; `false` if its index file has an order of that
/// name already, unless it `replace`s it. The index file is found in
/// `connection`'s folder.
pub(crate) fn create_order_on(
    session: &Session,
    connection: Handle,
    opened: &Arc<table::Table>,
    order: &Order,
    replace: bool,
) -> Result<bool, Error> {
    create_order_on_with(session, connection, opened, order, replace, None)
}

/// [`create_order_on`] as the maintenance `online` runs it, if it does: the
/// keys are collected while the table is open to other connections, and
/// again in the final stage if they changed it meanwhile.
fn create_order_on_with(
    session: &Session,
    connection: Handle,
    opened: &Arc<table::Table>,
    order: &Order,
    replace: bool,
    online: Option<&crate::maintenance::Guard>,
) -> Result<bool, Error> {
    check_order_names(order)?;
    let key = opened
        .compile(order.expression)
        .map_err(|error| match error {
            Error::Expression(kind) => Error::KeyExpression(kind),
            error => error,
        })?;
    if !order.condition.is_empty() {
        opened.compile(order.condition)?;
    }
    let binary = order.options & index_options::BINARY != 0;
    // The width option of Unicode keys: 2, 4 or 5 bytes a character, 3 without
    // (`fixtures/write/uchar_keys`).
    let uchar_width = match order.options & index_options::UCHAR_KEY {
        0x2000_0000 => 2,
        0x4000_0000 => 4,
        0x6000_0000 => 5,
        _ => 0,
    };
    let (key_length, text) = key_shape(opened, order.expression, &key, uchar_width);
    if key_length > usize::from(create::ADI_MAX_KEY) {
        return Err(Error::InvalidKeyLength.detailed("Key too long."));
    }
    // Orders on NChar and NVarChar fields (alone or among several) weigh
    // their keys by a locale; the keys of an expression on one are not known.
    let unicode_list = order.expression.split(';').any(|part| {
        opened.fields().iter().any(|f| {
            f.name.eq_ignore_ascii_case(part.trim())
                && matches!(f.field_type, FieldType::NChar | FieldType::NVarChar)
        })
    });
    // So does an expression that reads one, whatever it makes of it; the ones
    // whose keys tflip knows are built from Unicode text.
    if !unicode_list && key.unicode() == tflip_expr::Unicode::Unsupported {
        return Err(Error::NotImplemented);
    }
    let unicode_fields =
        unicode_list || opened.names_unicode(&opened.with_field_numbers(order.expression));
    let mut tag_options = 0;
    let mut tag_flags = 0;
    if order.options & index_options::UNIQUE != 0 {
        tag_options |= options::UNIQUE;
    }
    if order.options & index_options::CUSTOM != 0 {
        tag_options |= options::CUSTOM;
    }
    if !order.condition.is_empty() {
        tag_options |= options::CONDITION;
    }
    if binary {
        tag_options |= options::FLAGS;
        tag_flags |= flags::BINARY;
    }
    let spec = OrderSpec {
        name: order.tag.to_owned(),
        expression: opened.with_field_numbers(order.expression),
        condition: opened.with_field_numbers(order.condition),
        key_length: key_length as u16,
        options: tag_options,
        flags: tag_flags,
        // A binary order's keys are its pad byte, `F`.
        pad: match (binary, text) {
            (true, _) => b'F',
            // A Unicode field among them leaves it 0 (`fixtures/write/uchar_keys`).
            (false, true) if !unicode_fields => b' ',
            (false, _) => 0,
        },
        descending: order.options & index_options::DESCENDING != 0,
        collation: String::new(),
        locale: if unicode_fields { "en_US" } else { "" }.to_owned(),
        uchar_width: if unicode_fields { uchar_width } else { 0 },
        fts: None,
    };
    let path = order_file(session, connection, opened, order.file)?;
    let weights = opened
        .session_settings()
        .collations()
        .ansi()
        .map(|c| c.weights);
    let collation = create::collation_crc(weights.as_ref());
    let prepared = match online.filter(|guard| guard.online()) {
        Some(guard) => {
            // The keys of an order in a file of its own, which the table's
            // index file does not know until the final stage.
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            let scratch = path.with_file_name(format!("{stem}.{}.online", order.tag));
            let size = match order.page_size {
                0 => create::grown_page_size(key_length as u16, PAGE_SIZE),
                size => size,
            };
            let _ = std::fs::remove_file(&scratch);
            let prepared = match create::add_order(&scratch, &spec, size, collation) {
                Ok(true) => index::prepare_keys(opened, &scratch, order.tag).map(Some),
                _ => Ok(None),
            };
            let _ = std::fs::remove_file(&scratch);
            let prepared = prepared?;
            if prepared.as_ref().is_some_and(index::Prepared::duplicates) {
                return Err(index::not_unique());
            }
            guard.final_stage();
            guard.drain(opened.path(), 1);
            prepared
        }
        None => None,
    };
    with_clear_index(opened, &path, || {
        let page_size = fit_page_size(opened, &path, order, key_length as u16, replace)?;
        let existed = path.exists();
        let added = match replace {
            true => create::replace_order(&path, &spec, page_size, collation).map(|()| true),
            false => create::add_order(&path, &spec, page_size, collation),
        };
        if !added.map_err(|_| Error::CorruptIndex)? {
            return Ok(false);
        }
        let built = match prepared {
            Some(prepared) if prepared.changes == opened.file_changes() => {
                index::finish_keys(&path, order.tag, prepared)
            }
            _ => index::build_keys(opened, &path, order.tag),
        };
        match built {
            // A division by 0 leaves no order behind, and no index file the
            // call made (tools/ace/expr/iif.c).
            Err(error @ Error::Evaluation(_)) => {
                if existed {
                    let _ = create::drop_order(&path, order.tag);
                } else {
                    let _ = std::fs::remove_file(&path);
                }
                Err(error)
            }
            built => built.map(|()| true),
        }
    })
}

/// Runs `work` on the index file at `path`, in the clear if the table
/// `opened` is AES-encrypted and its index enciphered (see
/// [`tflip_storage::adi::with_clear`]): the writers of orders read plain
/// pages.
pub(crate) fn with_clear_index<T>(
    opened: &Arc<table::Table>,
    path: &Path,
    work: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    // The index of an RC4 table of a dictionary that encrypts them is one
    // stream a page (`fixtures/aes/rc4tp`).
    if let Some(tflip_storage::table::RecordCipher::Rc4(key)) = opened.encryption_cipher()
        && opened.object_id.is_some()
    {
        return tflip_storage::adi::with_clear_rc4(
            path,
            &key,
            opened.dictionary_encrypts_indexes(),
            work,
            || Error::CorruptIndex,
        );
    }
    let (Some(tflip_storage::table::RecordCipher::Aes { key, nonce }), Some(counter)) =
        (opened.encryption_cipher(), opened.aes_counter())
    else {
        return work();
    };
    // Only a dictionary's table gets its first index enciphered: a free AES
    // table's is in the clear (`fixtures/aes/api/08_idxorder`).
    if opened.object_id.is_none() && !path.exists() {
        return work();
    }
    // The table's own check of its key, which its index files carry.
    let mut verifier = [0; 20];
    std::fs::File::open(&opened.path)
        .and_then(|mut file| {
            use std::io::{Read, Seek, SeekFrom};
            file.seek(SeekFrom::Start(0x38))?;
            file.read_exact(&mut verifier)
        })
        .map_err(|_| Error::CorruptIndex)?;
    tflip_storage::adi::with_clear(path, &key, &nonce, &counter, &verifier, work, || {
        Error::CorruptIndex
    })
}

/// Creates the full-text order `order` on the field at place `field` of the
/// open table `opened`, with the options `block`, and builds it from the
/// words of its records (see [`index::build_full_text`]); `false` if its index
/// file has an order of that name already, unless it `replace`s it. A new
/// index file takes pages of 1,024 bytes by default (`fixtures/write/fts_build`).
/// The keys of a Unicode field's order are ICU sort keys, which tflip does not
/// build for it ([`Error::NotImplemented`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn create_full_text_on(
    session: &Session,
    connection: Handle,
    opened: &Arc<table::Table>,
    order: &Order,
    field: usize,
    block: create::FtsBlock,
    replace: bool,
    compound: bool,
) -> Result<bool, Error> {
    let field_type = opened.fields()[field].field_type;
    if !can_have_full_text(field_type) {
        return Err(Error::InvalidFieldType
            .detailed("FTS indexes can only be created on character and BLOB fields."));
    }
    if opened.dbf_flavour().is_some() {
        return index::create_full_text_xbase(
            session, connection, opened, order, field, block, compound,
        );
    }
    check_order_names(order)?;
    if matches!(
        field_type,
        FieldType::NChar | FieldType::NVarChar | FieldType::NMemo
    ) {
        return Err(Error::NotImplemented);
    }
    let key_length = u16::try_from(block.max_word).unwrap_or(u16::MAX);
    if key_length > create::ADI_MAX_KEY {
        return Err(Error::InvalidKeyLength.detailed("Key too long."));
    }
    let spec = OrderSpec {
        name: order.tag.to_owned(),
        expression: format!("F{}", field + 1),
        condition: String::new(),
        key_length,
        options: options::FLAGS,
        flags: flags::FULL_TEXT,
        pad: b' ',
        descending: false,
        collation: String::new(),
        locale: String::new(),
        uchar_width: 0,
        fts: Some(block),
    };
    let path = order_file(session, connection, opened, order.file)?;
    let weights = opened
        .session_settings()
        .collations()
        .ansi()
        .map(|c| c.weights);
    let collation = create::collation_crc(weights.as_ref());
    // The pages of an encrypted table's index are enciphered after, as
    // those of any order are (`fixtures/aes/dictwrite`, `40_fts_create`).
    with_clear_index(opened, &path, || {
        let page_size = match (path.exists(), order.page_size) {
            (false, 0) => FULL_TEXT_PAGE_SIZE,
            _ => fit_page_size(opened, &path, order, key_length, replace)?,
        };
        let added = match replace {
            true => create::replace_order(&path, &spec, page_size, collation).map(|()| true),
            false => create::add_order(&path, &spec, page_size, collation),
        };
        if !added.map_err(|_| Error::CorruptIndex)? {
            return Ok(false);
        }
        index::build_full_text(opened, &path, order.tag)?;
        Ok(true)
    })
}

/// The page size of an index file a full-text order is the first order of.
const FULL_TEXT_PAGE_SIZE: u16 = 1024;

/// The index file `file` of the table `opened` names (in `connection`'s
/// folder), its structural one for `None`.
pub(crate) fn order_file(
    session: &Session,
    connection: Handle,
    opened: &table::Table,
    file: Option<&str>,
) -> Result<PathBuf, Error> {
    order_file_ext(session, connection, opened, file, "adi")
}

/// [`order_file`] for index files with the extension `extension`.
pub(crate) fn order_file_ext(
    session: &Session,
    connection: Handle,
    opened: &table::Table,
    file: Option<&str>,
    extension: &str,
) -> Result<PathBuf, Error> {
    match file {
        // A bare file name is in the table's folder. One with a folder in it
        // is relative to the current directory in the original
        // (`fixtures/errors/index_paths.json`), for which tflip takes the
        // connection's folder, as it does for tables.
        Some(file) if !file.is_empty() && !file.contains(['\\', '/']) => {
            let folder = opened
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let mut path = match &session.shares {
                Some(shares) => shares.inside(&folder, file)?,
                None => folder.join(file),
            };
            if path.extension().is_none() {
                path.set_extension(extension);
            }
            Ok(path)
        }
        Some(file) if connection != 0 => file_path(session, connection, file, extension),
        Some(file) => {
            let mut path = PathBuf::from(file);
            if path.extension().is_none() {
                path.set_extension(extension);
            }
            Ok(path)
        }
        None => Ok(opened.path.with_extension(extension)),
    }
}

/// The type names of `AdsCreateTable`'s field definitions, which a definition
/// may cut short (`ch` for `Character`).
const TYPE_NAMES: [(&str, FieldType); 34] = [
    ("Character", FieldType::Character),
    ("CICharacter", FieldType::CiCharacter),
    ("Char", FieldType::Character),
    ("Date", FieldType::Date),
    // `Short` is a short integer, which `ShortDate` would otherwise be for
    // a definition that cuts it short.
    ("ShortInteger", FieldType::ShortInt),
    ("ShortInt", FieldType::ShortInt),
    ("ShortDate", FieldType::Date),
    ("Logical", FieldType::Logical),
    ("Memo", FieldType::Memo),
    ("Binary", FieldType::Binary),
    ("Blob", FieldType::Binary),
    ("Image", FieldType::Image),
    ("Numeric", FieldType::Numeric),
    ("Double", FieldType::Double),
    ("Integer", FieldType::Integer),
    ("Time", FieldType::Time),
    ("TimeStamp", FieldType::TimeStamp),
    ("AutoIncrement", FieldType::AutoInc),
    ("AutoInc", FieldType::AutoInc),
    ("Raw", FieldType::Raw),
    ("CurDouble", FieldType::CurDouble),
    ("Money", FieldType::Money),
    ("LongInteger", FieldType::LongInt),
    ("LongInt", FieldType::LongInt),
    ("RowVersion", FieldType::RowVersion),
    ("ModTime", FieldType::ModTime),
    // As in the original engine, `VarChar` is the legacy type (code 8).
    ("VarChar", FieldType::LegacyVarChar),
    ("VarCharFox", FieldType::VarChar),
    ("VarBinary", FieldType::VarBinary),
    ("VarBinaryFox", FieldType::VarBinary),
    ("NChar", FieldType::NChar),
    ("NVarChar", FieldType::NVarChar),
    ("NMemo", FieldType::NMemo),
    ("GUID", FieldType::Guid),
];

/// The name `AdsCreateTable`'s definitions give a field type.
pub(crate) fn type_name(field_type: FieldType) -> Option<&'static str> {
    TYPE_NAMES
        .iter()
        .find(|(_, t)| *t == field_type)
        .map(|(name, _)| *name)
}

/// The fields `AdsCreateTable`'s definitions `text` ("name,type,length,
/// decimals;…") give, and an AutoInc field's first value: as the original
/// engine reads them (`fixtures/write/maintenance`).
pub(crate) fn field_specs(text: &str) -> Result<(Vec<FieldSpec>, Option<u32>), Error> {
    field_specs_in(text, None)
}

/// What `AdsCreateTable` says of a record over 65,535 bytes.
const RECORD_TOO_LONG: &str = "Maximum record length exceeded.";

/// [`field_specs`] of the definitions of a DBF table (`Some`, VFP's if the
/// flag is set) or an ADT one: the limits are the original engine's for the
/// table type, and the definitions it quotes are upper-cased but in ADT and
/// VFP tables.
fn field_specs_in(text: &str, dbf: Option<bool>) -> Result<(Vec<FieldSpec>, Option<u32>), Error> {
    use FieldType::*;
    let invalid = |why: String| Error::InvalidFieldDefinition(why);
    let mut specs = Vec::new();
    let mut first = None;
    let definitions: Vec<&str> = text.split(';').collect();
    let last = definitions.len() - 1;
    for (i, definition) in definitions.iter().enumerate() {
        if definition.trim().is_empty() {
            // A trailing `;` ends the definitions.
            if i == last && i > 0 {
                break;
            }
            return Err(invalid("There may be an empty field definition.".into()));
        }
        let upper = dbf == Some(false);
        let shown = match upper {
            true => definition.to_ascii_uppercase(),
            false => (*definition).to_owned(),
        };
        let problem = format!("The problem field definition is: \"{shown}\".");
        let parts: Vec<&str> = definition.split(',').map(str::trim).collect();
        let name = parts[0].to_owned();
        let shown_name = match upper {
            true => name.to_ascii_uppercase(),
            false => name.clone(),
        };
        // A DBF field's name has 10 characters at most, any other 128.
        if name.len() > 128 {
            return Err(invalid(format!("Field name too long: {shown_name}")));
        }
        if dbf.is_some() && name.len() > 10 {
            return Err(invalid(format!("Field name invalid.  {problem}")));
        }
        let wanted = parts.get(1).copied().unwrap_or_default();
        let names = TYPE_NAMES.iter();
        let field_type = names
            .clone()
            .find(|(n, _)| n.eq_ignore_ascii_case(wanted))
            .or_else(|| {
                let lower = wanted.to_ascii_lowercase();
                names
                    .clone()
                    .find(|(n, _)| !lower.is_empty() && n.to_ascii_lowercase().starts_with(&lower))
            })
            .map(|(_, t)| *t)
            .ok_or_else(|| invalid(format!("Field type was not recognized.  {problem}")))?;
        let number = |i: usize| -> Option<u32> {
            parts
                .get(i)
                .filter(|p| !p.is_empty())
                .map(|p| p.parse::<u32>().unwrap_or(u32::MAX))
        };
        let (length, decimals) = match field_type {
            Character | CiCharacter | VarChar | LegacyVarChar | NChar | NVarChar | Raw
            | VarBinary => match number(2) {
                None => return Err(invalid(format!("Field too long.  {problem}"))),
                Some(0) => {
                    return Err(invalid(format!(
                        "Field must be at least one character long.  {problem}"
                    )));
                }
                Some(n) if matches!(field_type, NChar | NVarChar) && n > 32500 => {
                    let which = if field_type == NChar {
                        "NChar"
                    } else {
                        "NVarChar"
                    };
                    return Err(invalid(format!(
                        "{which} fields are limited to 32500 Code Units. Field: \"{shown_name}\""
                    )));
                }
                // A legacy VarChar is 6 to 64000 long, and the original words
                // both limits as the lower one.
                Some(n) if field_type == LegacyVarChar && !(6..=64000).contains(&n) => {
                    return Err(invalid(format!(
                        "VarChar length must be at least 6. Field: \"{shown_name}\""
                    )));
                }
                Some(n) if field_type == VarChar && n > 254 => {
                    return Err(invalid(format!("Field too long.  {problem}")));
                }
                Some(n)
                    if field_type == VarBinary
                        && n > if dbf == Some(true) { 254 } else { 65000 } =>
                {
                    return Err(invalid(format!("Field too long.  {problem}")));
                }
                Some(n) if n > 65535 => {
                    return Err(invalid(format!("Field too long.  {problem}")));
                }
                Some(n) => (n as u16, 0),
            },
            Numeric => {
                let length = u64::from(number(2).unwrap_or(0));
                let decimals = u64::from(number(3).unwrap_or(0));
                if let Some(why) = numeric_problem(dbf.is_some(), length, decimals, &shown_name) {
                    return Err(invalid(why));
                }
                // A DBF field's length is a byte.
                let length = if dbf.is_some() { length & 0xFF } else { length };
                (length as u16, decimals as u16)
            }
            Double | CurDouble => match number(3).or(number(2)) {
                Some(decimals) => {
                    if let Some(why) = decimals_problem(u64::from(decimals), &shown_name) {
                        return Err(invalid(why));
                    }
                    (8, (decimals & 0xFF) as u16)
                }
                None => {
                    return Err(invalid(format!(
                        "Invalid field information for DOUBLE or CURDOUBLE field.  {problem}"
                    )));
                }
            },
            Money => (8, 4),
            AutoInc => {
                first = number(2).or(first);
                (4, 0)
            }
            Guid => (16, 0),
            Logical => (1, 0),
            ShortInt => (2, 0),
            Integer | Date | Time => (4, 0),
            Memo | Binary | Image | NMemo => (9, 0),
            _ => (8, 0),
        };
        specs.push(FieldSpec {
            name,
            field_type,
            length,
            decimals,
            compressed: false,
        });
    }
    Ok((specs, first))
}

/// The fields of a new DBF table of `flavour` that `AdsCreateTable`'s
/// definitions `text` give, as the original engine reads them
/// (`fixtures/write/dbf_create`): the ADT type names, `ShortDate` the
/// three-byte date, `VarChar` the Advantage VarChar (VFP's own in a VFP
/// table), `General` and `Picture`, and in VFP tables `NULL` and
/// `NOCPTRANS` after a field's type. Types a DBF table cannot have are
/// refused as the original refuses them.
pub(crate) fn dbf_field_specs(
    text: &str,
    flavour: tflip_storage::dbf::Flavour,
) -> Result<Vec<create::DbfFieldSpec>, Error> {
    use FieldType::*;
    use tflip_storage::dbf::Flavour;
    let visual = flavour == Flavour::Visual;
    let invalid = Error::InvalidFieldDefinition;
    let mut out = Vec::new();
    let definitions: Vec<&str> = text.split(';').collect();
    let last = definitions.len() - 1;
    for (i, definition) in definitions.iter().enumerate() {
        if definition.trim().is_empty() {
            if i == last && i > 0 {
                break;
            }
            return Err(invalid("There may be an empty field definition.".into()));
        }
        // The attributes, wherever they are after the type.
        let mut parts: Vec<&str> = definition.split(',').map(str::trim).collect();
        let mut nullable = false;
        let mut binary = false;
        let shown = match visual {
            true => definition.trim().to_owned(),
            false => definition.trim().to_ascii_uppercase(),
        };
        let mut k = 2;
        while k < parts.len() {
            let word = parts[k].to_ascii_uppercase();
            if word == "NULL" || word == "NOCPTRANS" {
                if !visual {
                    return Err(invalid(format!(
                        "The {word} attribute is valid for ADS_VFP tables only. The problem field \
                         definition is \"{shown}\"."
                    )));
                }
                nullable |= word == "NULL";
                binary |= word == "NOCPTRANS";
                parts.remove(k);
            } else {
                k += 1;
            }
        }
        // A Clipper or FoxPro field has at most a name, type, length and
        // decimals (`fixtures/errors/field_attributes.json`).
        // (a trailing empty value aside).
        if !visual && parts.len() - usize::from(parts.last() == Some(&"")) > 4 {
            return Err(invalid(format!(
                "Field definition contains too many values. Missing semicolon? The problem \
                 field definition is: \"{shown}\"."
            )));
        }
        let name = parts[0].to_owned();
        let wanted = parts
            .get(1)
            .copied()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let field = if visual {
            name.clone()
        } else {
            name.to_ascii_uppercase()
        };
        let unsupported = |what: &str| {
            invalid(format!(
                "{what} not supported with DBF files. Field: \"{field}\""
            ))
        };
        let extra = match wanted.as_str() {
            "general" => Some(Other(u16::from(b'G'))),
            "picture" => Some(Other(u16::from(b'P'))),
            "shortdate" if visual => {
                return Err(invalid(
                    "Compact date fields are not supported with ADS_VFP files.".into(),
                ));
            }
            "shortdate" => Some(CompactDate),
            "varchar" if visual => Some(VarChar),
            "blob" => {
                return Err(invalid(format!(
                    "Field type was not recognized.  The problem field definition is: \"{shown}\"."
                )));
            }
            _ => None,
        };
        let rest = parts.join(",");
        let mut spec = match extra {
            Some(field_type) => {
                let length = parts.get(2).and_then(|p| p.parse().ok()).unwrap_or(10);
                FieldSpec {
                    name,
                    field_type,
                    length,
                    decimals: 0,
                    compressed: false,
                }
            }
            None => {
                let (mut specs, _) = field_specs_in(&rest, Some(visual))?;
                specs.pop().ok_or_else(|| invalid(String::new()))?
            }
        };
        let only_vfp = |what: &str| {
            invalid(format!(
                "{what} Fields are only supported with ADS_VFP files. Field: \"{field}\""
            ))
        };
        match spec.field_type {
            ShortInt => return Err(unsupported("Short integer fields")),
            Time => return Err(unsupported("Time fields")),
            TimeStamp if !visual => return Err(unsupported("Timestamp fields")),
            AutoInc if !visual => return Err(unsupported("Auto-increment fields")),
            Raw => return Err(unsupported("Raw fields")),
            CurDouble => return Err(unsupported("Currency fields")),
            Money if !visual => return Err(unsupported("Money fields")),
            CiCharacter => return Err(unsupported("Case-insensitive character fields")),
            RowVersion => return Err(unsupported("RowVersion fields")),
            ModTime => return Err(unsupported("ModTime fields")),
            VarChar if !visual => return Err(only_vfp("VarCharFox")),
            VarBinary if !visual => return Err(only_vfp("VarBinaryFox")),
            _ => {}
        }
        // An AutoInc's first value, the third item (1 if none).
        let autoinc = (spec.field_type == AutoInc).then(|| {
            let first = parts.get(2).and_then(|p| p.parse().ok()).unwrap_or(1);
            (first, 1)
        });
        if spec.field_type == AutoInc {
            spec.length = 4;
        }
        out.push(create::DbfFieldSpec {
            spec,
            nullable,
            binary,
            autoinc,
        });
    }
    Ok(out)
}

/// Creates the table `name` of `connection`, of the type of `settings`,
/// with the fields of `AdsCreateTable`'s definitions `fields` and memo
/// blocks of `memo_size` bytes (0: 256 in an ADT table, 64 in an `.fpt`),
/// and opens it with `settings` (`AdsCreateTable`); a file there already is
/// [`Error::FileCreate`].
pub fn create_table_api(
    session: &Session,
    connection: Handle,
    name: &str,
    fields: &str,
    memo_size: u16,
    settings: TableSettings,
) -> Result<Handle, Error> {
    create_table_inner(
        session, connection, name, fields, memo_size, settings, false,
    )
}

/// [`create_table_api`] as the original's `AdsCreateTable` does it on a
/// dictionary connection: an ADT table joins the dictionary, which
/// encrypts it if it encrypts new tables (`fixtures/aes/rc4tp`, `x1`, `x3`,
/// `x6`).
pub fn create_table_in_dictionary_api(
    session: &Session,
    connection: Handle,
    name: &str,
    fields: &str,
    memo_size: u16,
    settings: TableSettings,
) -> Result<Handle, Error> {
    create_table_inner(session, connection, name, fields, memo_size, settings, true)
}

fn create_table_inner(
    session: &Session,
    connection: Handle,
    name: &str,
    fields: &str,
    memo_size: u16,
    mut settings: TableSettings,
    join: bool,
) -> Result<Handle, Error> {
    use tflip_storage::dbf::Flavour;
    // A `#name` is the connection's temporary table, as `CREATE TABLE #name`
    // makes it; the default table type makes an ADT one, on a free
    // connection too (`fixtures/sql/temptables_api.json`).
    if connection != 0 && crate::temp::is_temp(name) && settings.table_type == TableType::Database {
        settings.table_type = TableType::Adt;
    }
    let flavour = match settings.table_type {
        TableType::Adt => None,
        TableType::Ntx => Some(Flavour::Clipper),
        TableType::Cdx => Some(Flavour::FoxPro),
        TableType::Vfp => Some(Flavour::Visual),
        TableType::Database => return Err(Error::NotImplemented),
    };
    if let Some(flavour) = flavour {
        return create_dbf_table(
            session, connection, name, fields, memo_size, settings, flavour, join,
        );
    }
    let (specs, first) = field_specs(fields)?;
    if create::adt_record_length(&specs) > 65535 {
        return Err(Error::InvalidFieldDefinition(RECORD_TOO_LONG.into()));
    }
    let has_memo = specs.iter().any(|s| {
        matches!(
            s.field_type,
            FieldType::Memo | FieldType::Binary | FieldType::Image | FieldType::NMemo
        )
    });
    // An ADT table's memo blocks are 8 to 32,768 bytes (0 for the default).
    if has_memo && ((1..8).contains(&memo_size) || memo_size > 32768) {
        return Err(Error::InvalidMemoBlockSize.detailed("Parameter 8"));
    }
    let path = match connection {
        0 => {
            let mut path = PathBuf::from(name);
            if path.extension().is_none() {
                path.set_extension("adt");
            }
            path
        }
        _ if crate::temp::is_temp(name) => temp_api_path(session, connection, name, &settings)?,
        _ => file_path(session, connection, name, "adt")?,
    };
    let joins = joins_dictionary(session, connection, name, &path, join)?;
    replace_existing(&path, Some("adi"))?;
    let block_size = if memo_size == 0 {
        BLOCK_SIZE
    } else {
        memo_size
    };
    // An AES connection lays the table out for AES, not yet armed (see
    // [`create_table`]); a free-table (connection 0) create is never AES.
    let aes = connection != 0 && connection::get(session, connection)?.encryption_type() != 0;
    let mut bytes = match aes {
        true => create::aes_adt_unarmed_bytes(&specs),
        false => create::adt_bytes(&specs),
    }
    .map_err(|_| Error::FileCreate)?;
    if let Some(first) = first {
        bytes[0x50..0x54].copy_from_slice(&first.to_le_bytes());
    }
    let memo = specs.iter().any(|s| {
        matches!(
            s.field_type,
            FieldType::Memo
                | FieldType::Binary
                | FieldType::Image
                | FieldType::NMemo
                | FieldType::LegacyVarChar
        )
    });
    std::fs::write(&path, &bytes).map_err(|_| Error::FileCreate)?;
    if memo {
        let memo_path = path.with_extension("adm");
        std::fs::write(memo_path, create::adm_bytes(block_size)).map_err(|_| Error::FileCreate)?;
    }
    if joins {
        let table_name = table_stem(&path);
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rules = vec![FieldRules::default(); specs.len()];
        let created = crate::dictionary_write::Created {
            name: &table_name,
            table_type: TableType::Adt,
            char_type: 1,
            file: &file,
            rules: rules.iter().collect(),
            block_size: (memo_size != 0).then_some(memo_size),
            primary_key: None,
            trans_free: false,
        };
        if !crate::dictionary_write::add_created(session, connection, &created)? {
            rights::grant_creator(session, connection, TABLE, &table_name)?;
        }
    }
    // By its name: a server's sessions reach files in their shares only.
    match connection {
        0 => table::open(session, connection, &path.to_string_lossy(), settings),
        _ => table::open(session, connection, name, settings),
    }
}

/// [`crate::restructure::restructure`] of the table `name` as `ALTER TABLE`
/// runs it: a temporary table that is not there is 7112, and the original
/// rebuilds one as a new table, without the orders it had
/// (`fixtures/sql/temptables.json`).
fn restructure_table(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: TableSettings,
    changes: &crate::restructure::Changes,
    gate: Option<&crate::maintenance::Guard>,
) -> Result<(), Error> {
    let temporary = crate::temp::is_temp(name);
    let owner = connection::get(session, connection)?;
    if temporary && owner.temps.find(name).is_none() {
        return Err(Error::Sql(crate::temp::not_found(name, false)));
    }
    if temporary {
        temp_type(&owner, name, settings.table_type)?;
    }
    crate::restructure::restructure_with(session, connection, name, settings, changes, gate)?;
    if let (true, Some((path, _))) = (temporary, owner.temps.find(name)) {
        let _ = std::fs::remove_file(path.with_extension("adi"));
    }
    Ok(())
}

/// 5152, the restructure part of `ALTER TABLE` refused with `why`.
fn restructure_refused(why: &str) -> Error {
    Error::Sql(SqlError::ace(5152, "The table restructure failed.", why))
}

/// `ALTER TABLE` on `connection`: the free ADT table's fields change as
/// `AdsRestructureTable` changes them (see [`crate::restructure`]). As in
/// the original engine, column constraints are dropped (a free table has
/// none), `MEMOBLOCKSIZE` rewrites the memo file in blocks of that size, and a
/// restructure that fails is the SQL error wrapping its own. `IGNORE` and `RESPECT TRANSACTIONS` set
/// the table's transaction-free mark in the same restructure, which fails
/// where it would change nothing. `DROP PRIMARY KEY` and `ALTER COLUMN …
/// DROP` take away what only a dictionary keeps: a free table has no such
/// thing, so they pass, the first after opening the table
/// (`fixtures/sql/ddopts`). The tables of a dictionary are altered by
/// [`alter_dictionary_table`].
pub(crate) fn alter_table(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    alter: &tflip_sql::ast::AlterTable,
) -> Result<(), Error> {
    use crate::restructure::{Added, Changed, Changes};
    use tflip_sql::ast::AlterAction;
    rights::require_table(
        session,
        connection,
        &alter.table.name,
        rights::ALTER,
        "Permission to change this table is not granted.",
    )?;
    // A field's definition, and a DBF field's own.
    let spec = |column: &ColumnDef,
                added: bool|
     -> Result<(FieldSpec, Option<create::DbfFieldSpec>), Error> {
        match flavour(settings) {
            Some(flavour) => {
                // Only a Visual FoxPro table takes NULL and NOCPTRANS.
                if flavour != Flavour::Visual && (column.nullable == Some(true) || column.nocptrans)
                {
                    let word = if column.nullable == Some(true) {
                        "NULL"
                    } else {
                        "NOCPTRANS"
                    };
                    let spec = field_spec(column)?;
                    let nullable = column.nullable == Some(true);
                    let shown = shown_definition(column, &spec, nullable).to_ascii_uppercase();
                    let end = if added { ";" } else { "" };
                    return Err(invalid_field(&format!(
                        "The {word} attribute is valid for ADS_VFP tables only. The problem \
                         field definition is \"{shown}{end}\"."
                    )));
                }
                let dbf = dbf_specs(&[column], flavour)?
                    .pop()
                    .ok_or(Error::NotImplemented)?;
                Ok((dbf.spec.clone(), Some(dbf)))
            }
            None => Ok((field_spec(column)?, None)),
        }
    };
    let mut changes = Changes {
        add: Vec::new(),
        delete: Vec::new(),
        change: Vec::new(),
        block_size: 0,
        transaction_free: None,
        require_change: true,
        bind: None,
        indexes: Vec::new(),
        expressions: Vec::new(),
        keys: Vec::new(),
    };
    let position = |p: &Option<u64>| p.map(|p| p as usize);
    // A definition as the original engine shows it in an error, of any table
    // type (`name,Integer`, `name,CHAR,7`; `fixtures/sql/ddalterfree`).
    let shown = |column: &ColumnDef, spec: &FieldSpec| shown_definition(column, spec, false);
    let mut memo_block_size = false;
    let mut columns = 0;
    let mut drop_key = false;
    let mut rules = Vec::new();
    for action in &alter.actions {
        if matches!(
            action,
            AlterAction::Add(..) | AlterAction::Alter(..) | AlterAction::Drop(_)
        ) {
            columns += 1;
        }
        match action {
            AlterAction::Add(column, at) => {
                let (spec, dbf) = spec(column, true)?;
                let text = shown(column, &spec);
                changes.add.push(Added {
                    spec,
                    position: position(at),
                    text,
                    dbf,
                    rules: column.rules.clone(),
                });
            }
            AlterAction::Alter(from, column, at) => {
                let (spec, dbf) = spec(column, false)?;
                let text = format!("{from},{}", shown(column, &spec));
                let from = from.clone();
                changes.change.push(Changed {
                    from,
                    spec,
                    position: position(at),
                    dbf,
                    text,
                    rules: column.rules.clone(),
                });
            }
            AlterAction::Drop(name) => changes.delete.push(name.clone()),
            // Once two column actions have gone before it, the original
            // engine drops a `MEMOBLOCKSIZE` unread (even a size it would
            // refuse): `DROP COLUMN b ALTER COLUMN a aa INTEGER MEMOBLOCKSIZE
            // 128` keeps the block size, with `MEMOBLOCKSIZE` first it changes.
            AlterAction::MemoBlockSize(size) if columns < 2 => {
                memo_block_size = true;
                changes.block_size = u32::try_from(*size).unwrap_or(u32::MAX);
            }
            AlterAction::MemoBlockSize(_) => {}
            AlterAction::Transactions(ignore) => changes.transaction_free = Some(*ignore),
            AlterAction::DropPrimaryKey => drop_key = true,
            AlterAction::DropRule(column, pos, rule) => rules.push((column.as_str(), *pos, *rule)),
        }
    }
    let structural =
        !(changes.add.is_empty() && changes.delete.is_empty() && changes.change.is_empty());
    let restructures = structural || memo_block_size || changes.transaction_free.is_some();
    if restructures {
        crate::transaction::check_legal(
            session,
            connection,
            crate::transaction::Illegal::Restructure,
        )
        .map_err(crate::transaction::sql_error)?;
    }
    // A server (tflipd) is what the original engine's remote server is.
    if alter.online && restructures && session.shares.is_none() {
        return Err(restructure_refused(
            "Online restructure is only available through ALTER ONLINE TABLE on remote server \
             connections.",
        ));
    }
    let name = &alter.table.name;
    // A temporary table is the connection's own, whatever the connection.
    let temporary = crate::temp::is_temp(name);
    // The table is closed to other connections meanwhile, unless `ONLINE`
    // (`ONLINE_WITH_TRANSITION` waits for those that opened it).
    let gate = match restructures {
        false => None,
        true => {
            use crate::maintenance::Kind;
            let kind = match (alter.online, alter.transition) {
                (false, _) => Kind::Offline,
                (true, false) => Kind::Online,
                (true, true) => Kind::Transition,
            };
            maintain(session, connection, settings, name, kind, "__OAFinalStage_")?
        }
    };
    if !temporary && connection::get(session, connection)?.dictionary {
        return alter_dictionary_table(
            session,
            connection,
            name,
            drop_key,
            &rules,
            changes,
            restructures,
            gate.as_ref(),
        );
    }
    if (drop_key || restructures) && !temporary {
        check_free_table(session, connection, settings, name)?;
    }
    if !restructures {
        return Ok(());
    }
    let table = settings.table(0);
    restructure_table(session, connection, name, table, &changes, gate.as_ref())
        .map_err(restructure_error)
}

/// 5004 or 5159, the refusal of `ALTER TABLE` to open the free table `name`
/// that is not there, or is a dictionary's.
fn check_free_table(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    name: &str,
) -> Result<(), Error> {
    let path = file_path(session, connection, name, settings.extension())?;
    if !path.exists() {
        return Err(Error::Sql(SqlError::ace(
            5004,
            "Either ACE could not find the specified file, or you do not have sufficient \
             rights to access the file.",
            &path.display().to_string(),
        )));
    }
    if flavour(settings).is_none() && tflip_storage::dd::create::is_bound(&path).unwrap_or(false) {
        return Err(Error::Sql(SqlError::ace(
            5159,
            "Error encountered when trying to open a database table.",
            "The table is a database table. It cannot be opened as a free table.",
        )));
    }
    Ok(())
}

/// A restructure's error as `ALTER TABLE` words it: the SQL error wrapping
/// its own.
fn restructure_error(error: Error) -> Error {
    // The details a restructure call's text has are not the statement's.
    let error = match error {
        Error::Detailed(error, _) => *error,
        error => error,
    };
    let (code, text, why) = match error {
        Error::NoSuchField(why) => (5011, "The expression was invalid.", why),
        Error::RestructureFailed(why) => (5152, "The table restructure failed.", why),
        Error::InvalidFieldDefinition(why) => (
            5012,
            "The field definition specified for the table was not valid.",
            why,
        ),
        Error::InvalidFieldType => (
            5066,
            "The requested operation is not legal for the given field type.",
            String::new(),
        ),
        Error::InvalidDataType => (
            5070,
            "The given data type is not valid for the requested operation.",
            String::new(),
        ),
        Error::InvalidDate => (5095, "The date specified is not valid.", String::new()),
        Error::JulianConversion { negative } => (
            5095,
            "The date specified is not valid.",
            crate::julian_conversion_detail(negative).to_owned(),
        ),
        Error::InvalidValue => (
            5101,
            "A value was outside of the expected range.",
            String::new(),
        ),
        Error::ValueOverflow => (
            5179,
            "The result value exceeded the given buffer size.",
            String::new(),
        ),
        Error::FileInUse => (
            7008,
            "The specified table, memo file, or index file was unable to be opened.",
            String::new(),
        ),
        Error::InvalidMemoBlockSize => (
            5058,
            "The specified memo block size is invalid.",
            String::new(),
        ),
        Error::KeyExpression(kind) => return crate::ace_error::sql(kind.code(3000), ""),
        error => return error,
    };
    Error::Sql(SqlError::ace(code, text, &why))
}

/// [`restructure_error`] of a table of a dictionary, whose key expressions
/// the original words with their text alone (`fixtures/sql/ddalterdbf`).
fn dictionary_restructure_error(error: Error) -> Error {
    match error {
        Error::KeyExpression(kind) => crate::ace_error::sql_bare(kind.code(3000)),
        // A full-text order on a field that cannot have one
        // (`fixtures/write/restructure_fts_dict2`).
        Error::InvalidFieldType => crate::ace_error::sql_bare(5066),
        // An order made from its Key object on a field that went
        // (`fixtures/write/restructure_fts_dict_after`).
        Error::Ace { code: 5032, .. } => crate::ace_error::sql_bare(5032),
        error => restructure_error(error),
    }
}

/// `ALTER TABLE` of a table of the dictionary `connection` is to: `DROP
/// PRIMARY KEY` and `ALTER COLUMN … DROP` change the dictionary's records
/// (the key's index stays), and a restructure (fields, `IGNORE` and `RESPECT
/// TRANSACTIONS`, `MEMOBLOCKSIZE`) changes the table and the dictionary's
/// objects for it ([`crate::restructure::restructure_dictionary`],
/// `fixtures/sql/ddalter`). It is the right to alter the table that lets a
/// user restructure.
#[allow(clippy::too_many_arguments)]
fn alter_dictionary_table(
    session: &Session,
    connection: Handle,
    name: &str,
    drop_key: bool,
    rules: &[(&str, usize, ColumnRule)],
    changes: crate::restructure::Changes,
    restructures: bool,
    gate: Option<&crate::maintenance::Guard>,
) -> Result<(), Error> {
    if !in_dictionary(session, connection, name) {
        return Err(crate::dictionary_write::not_valid(
            "There is no such named table in the data dictionary.",
        ));
    }
    if drop_key {
        crate::dictionary_write::drop_primary_key(session, connection, name)?;
    }
    for &(column, _, rule) in rules {
        crate::dictionary_write::drop_field_rule(session, connection, name, column, rule)?;
    }
    if !restructures {
        return Ok(());
    }
    crate::restructure::restructure_dictionary_with(session, connection, name, &changes, gate)
        .map_err(dictionary_restructure_error)
}

/// [`create_table_api`] for a DBF table of `flavour`: the table and, if a
/// field needs one, its memo file (a `.dbt` of 512-byte blocks for Clipper,
/// an `.fpt` of `memo_size`-byte blocks for FoxPro and VFP: 64 if 0, ×512
/// if 1 to 32).
/// Whether the table `path` made as `name` joins the dictionary of
/// `connection`, which it may do only under a name the dictionary has not
/// taken (5132).
fn joins_dictionary(
    session: &Session,
    connection: Handle,
    name: &str,
    path: &Path,
    join: bool,
) -> Result<bool, Error> {
    let joins = join
        && connection != 0
        && !crate::temp::is_temp(name)
        && connection::get(session, connection)?.dictionary;
    if joins {
        refuse_view_name(session, connection, &table_stem(path))
            .map_err(crate::ace_error::plain)?;
        let owner = connection::get(session, connection)?;
        let taken = owner
            .with_database_opt(|db| {
                db.dictionary()
                    .named(tflip_storage::dd::Kind::Table, &table_stem(path))
                    .is_some()
            })
            .unwrap_or(false);
        if taken {
            return Err(crate::ace_error::plain(crate::dictionary_write::not_valid(
                "An object with the same name already exists in the data dictionary.",
            )));
        }
    }
    Ok(joins)
}

/// Whether the tables `connection` makes are laid out for AES: it has an AES
/// `EncryptionType`, or is to a dictionary of AES tables (`fixtures/aes/rc4tp`,
/// `v1`, `v8`, `v10`). A Visual FoxPro table is, a Clipper or FoxPro one not.
pub(crate) fn aes_layout(session: &Session, connection: Handle) -> bool {
    let Ok(owner) = connection::get(session, connection) else {
        return false;
    };
    owner.encryption_type() != 0
        || owner
            .with_database_opt(|db| db.scheme() != tflip_storage::crypto::Scheme::Rc4)
            .unwrap_or(false)
}

fn create_dbf_table(
    session: &Session,
    connection: Handle,
    name: &str,
    fields: &str,
    memo_size: u16,
    settings: TableSettings,
    flavour: Flavour,
    join: bool,
) -> Result<Handle, Error> {
    let specs = dbf_field_specs(fields, flavour)?;
    if create::dbf_record_length(flavour, &specs) > 65535 {
        return Err(Error::InvalidFieldDefinition(RECORD_TOO_LONG.into()));
    }
    // An `.fpt`'s blocks are at most 1,024 (units of 512 bytes up to 32).
    let has_memo = specs.iter().any(|s| {
        matches!(
            s.spec.field_type,
            FieldType::Memo | FieldType::Binary | FieldType::Image | FieldType::NMemo
        ) || matches!(s.spec.field_type, FieldType::Other(_))
    });
    if has_memo && flavour != Flavour::Clipper && memo_size > 1024 {
        return Err(Error::InvalidMemoBlockSize.detailed("Parameter 8"));
    }
    let path = match connection {
        0 => {
            let mut path = PathBuf::from(name);
            if path.extension().is_none() {
                path.set_extension("dbf");
            }
            path
        }
        _ if crate::temp::is_temp(name) => temp_api_path(session, connection, name, &settings)?,
        _ => file_path(session, connection, name, "dbf")?,
    };
    let joins = joins_dictionary(session, connection, name, &path, join)?;
    if joins
        && crate::dictionary_write::rc4_tables::refuses_dbf_without_strong_encryption(
            &*connection::get(session, connection)?,
            settings.table_type,
        )
    {
        return Err(Error::Ace {
            code: 7168,
            detail: String::new(),
        });
    }
    replace_existing(&path, (flavour != Flavour::Clipper).then_some("cdx"))?;
    let aes = flavour == Flavour::Visual
        && connection != 0
        && !crate::temp::is_temp(name)
        && aes_layout(session, connection);
    write_dbf(&path, flavour, &specs, memo_size, settings.char_type, aes)?;
    if joins {
        let table_name = table_stem(&path);
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rules = vec![FieldRules::default(); specs.len()];
        let created = crate::dictionary_write::Created {
            name: &table_name,
            table_type: settings.table_type,
            char_type: settings.char_type,
            file: &file,
            rules: rules.iter().collect(),
            block_size: (memo_size != 0).then_some(memo_size),
            primary_key: None,
            trans_free: false,
        };
        if !crate::dictionary_write::add_created(session, connection, &created)? {
            rights::grant_creator(session, connection, TABLE, &table_name)?;
        }
    }
    match connection {
        0 => table::open(session, connection, &path.to_string_lossy(), settings),
        _ => table::open(session, connection, name, settings),
    }
}

/// Writes the DBF table `path` of `flavour` with the fields `specs`, and its
/// memo file if a field needs one (a `.dbt` of 512-byte blocks for Clipper,
/// an `.fpt` of `memo_size`-byte blocks for FoxPro and VFP: 64 if 0, ×512
/// if 1 to 32). `char_type` is the table's: OEM for ADS_OEM (2).
pub(crate) fn write_dbf(
    path: &Path,
    flavour: Flavour,
    specs: &[create::DbfFieldSpec],
    memo_size: u16,
    char_type: u16,
    aes: bool,
) -> Result<(), Error> {
    let today = tflip_storage::value::julian_to_ymd(crate::clock::now().local.0);
    let (bytes, memo) = create::dbf_bytes_layout(flavour, specs, today, char_type == OEM, aes);
    std::fs::write(path, &bytes).map_err(|_| Error::FileCreate)?;
    if memo {
        let (kind, memo_bytes) = match flavour {
            Flavour::Clipper => (tflip_storage::memo::MemoKind::Dbt, create::dbt_bytes()),
            _ => {
                let block_size = match memo_size {
                    0 => 64,
                    1..=32 => memo_size * 512,
                    size => size,
                };
                (
                    tflip_storage::memo::MemoKind::Fpt,
                    create::fpt_bytes(block_size),
                )
            }
        };
        let memo_path = tflip_storage::dbf::memo_path(path, kind);
        std::fs::write(memo_path, memo_bytes).map_err(|_| Error::FileCreate)?;
    }
    Ok(())
}

/// `DROP TABLE` on `connection`: the table's files go, of the type of the
/// statement's `settings`: its memo file, and its structural index file
/// (but a Clipper table's `.ntx` files, which stay). `FROM DATABASE` wants
/// a table of the dictionary, and with `NO_DELETE` leaves its files, free
/// again, where they are (`fixtures/sql/ddoptsdict`).
pub(crate) fn drop_table(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    drop: &DropTable,
) -> Result<(), Error> {
    let table = &drop.table;
    let owner = connection::get(session, connection)?;
    if crate::temp::is_temp(&table.name) {
        temp_type(&owner, &table.name, settings.table_type)?;
        return drop_temp(&owner, &table.name);
    }
    // A dictionary's table leaves it, its files with it, as
    // `AdsDDRemoveTable` removes it (`fixtures/sql/ddtables`).
    let listed = in_dictionary(session, connection, &table.name);
    if drop.from_database {
        if !owner.dictionary {
            return Err(Error::Sql(SqlError::no_dictionary()));
        }
        if !listed {
            return Err(crate::dictionary_write::not_valid(
                "There is no such named table in the data dictionary.",
            ));
        }
    }
    if listed {
        rights::require_table(
            session,
            connection,
            &table.name,
            rights::DROP,
            "Table cannot be removed from the database.",
        )?;
        crate::cache::evict_kind(crate::cache::Kind::Tables);
        // The table is opened, and closed, as the dictionary lets it go.
        if let Some((path, ..)) = table::open_info(
            session,
            connection,
            &table.name,
            &settings.table_named(0, &table.name),
        ) {
            crate::trigger::dropping(session, connection, &table.name, &path)?;
        }
        return match drop.no_delete {
            true => crate::dictionary_write::remove_table_keeping_files(
                session,
                connection,
                &table.name,
            ),
            false => {
                crate::dictionary_write::remove_table_api(session, connection, &table.name, true)
            }
        };
    }
    let path = file_path(session, connection, &table.name, settings.extension())?;
    if owner.refuses_free_tables() {
        return Err(Error::Sql(SqlError::ace(
            7087,
            "Cannot open table or view for this operation because the requested permissions have not been \
             granted to the user.",
            &path.display().to_string(),
        )));
    }
    if !path.exists() {
        return Err(Error::Sql(SqlError::ace(
            5004,
            "Either ACE could not find the specified file, or you do not have sufficient \
             rights to access the file.",
            &path.display().to_string(),
        )));
    }
    if flavour(settings).is_none() && tflip_storage::dd::create::is_bound(&path).unwrap_or(false) {
        return Err(Error::Sql(SqlError::ace(
            5159,
            "Error encountered when trying to open a database table.",
            "The table is a database table. It cannot be opened as a free table.",
        )));
    }
    let others: &[&str] = match flavour(settings) {
        None => &["adm", "adi"],
        Some(Flavour::Clipper) => &["dbt"],
        Some(_) => &["fpt", "cdx"],
    };
    // Connections that keep the table open for reuse let go of it; and, as
    // in the original engine, of every table they keep (not their
    // statements' tables).
    crate::cache::evict_path(&path);
    crate::cache::evict_kind(crate::cache::Kind::Tables);
    // A table open in the process, as a statement holds the one its closed
    // cursor read, is not dropped (probed, `tools/ace/cache`).
    if crate::shared::in_use(&path) {
        return Err(Error::Sql(SqlError::ace(
            7008,
            "The specified table, memo file, or index file was unable to be opened.",
            &path.display().to_string(),
        )));
    }
    std::fs::remove_file(&path).map_err(|_| Error::CorruptTable)?;
    for extension in others {
        let _ = std::fs::remove_file(path.with_extension(extension));
    }
    Ok(())
}

/// Makes way for a new table at `path` where a file is already, as the
/// original engine does (`fixtures/errors/create_existing.json`): a table
/// open anywhere in the process is refused (7040), and any other file is
/// replaced, with the structural index file (`structural`, its extension)
/// of the table it was. A memo file stays until the new table writes one.
fn replace_existing(path: &Path, structural: Option<&str>) -> Result<(), Error> {
    if !path.exists() {
        return Ok(());
    }
    // Connections that keep the table open for reuse let go of it.
    crate::cache::evict_path(path);
    if crate::shared::in_use(path) {
        return Err(Error::FileCreate);
    }
    if let Some(extension) = structural {
        let index = crate::shares::actual_case(path.with_extension(extension));
        if crate::shared::in_use(&index) {
            return Err(Error::FileCreate);
        }
        let _ = std::fs::remove_file(index);
    }
    Ok(())
}

/// The refusal of a statement of the type `asked` to the temporary table
/// `name`, if it was made as another (`fixtures/sql/temptables_types.json`),
/// where the table is not in the message.
fn temp_type(owner: &connection::Connection, name: &str, asked: TableType) -> Result<(), Error> {
    if let Some((_, made)) = owner.temps.find(name)
        && asked != TableType::Database
    {
        if !crate::temp::read_before(name)
            && let Some(error) =
                crate::temp::type_mismatch(name, made, asked, owner.temps.pinned(name), false)
        {
            return Err(Error::Sql(error));
        }
        crate::temp::note_read(name);
        crate::temp::pin(&owner.temps, name, asked);
    }
    Ok(())
}

/// `DROP TABLE #name`: the connection forgets the table and its files go.
fn drop_temp(owner: &connection::Connection, name: &str) -> Result<(), Error> {
    let path = owner
        .temps
        .remove(name)
        .ok_or_else(|| Error::Sql(crate::temp::not_found(name, false)))?;
    crate::cache::evict_path(&path);
    crate::cache::evict_kind(crate::cache::Kind::Tables);
    for extension in ["tmp", "adm", "adi", "dbt", "fpt", "cdx", "idx", "ntx"] {
        let _ = std::fs::remove_file(path.with_extension(extension));
    }
    Ok(())
}

/// Takes the order `tag` out of the index file at `path` of the dictionary's
/// table `table`, and the file with it if it is the last: what
/// `AdsDDDeleteIndex` does to the file the dictionary names.
pub(crate) fn drop_order_from_file(
    session: &Session,
    connection: Handle,
    table: &str,
    path: &Path,
    tag: &str,
) -> Result<(), Error> {
    let settings = Settings::default();
    let empty = with_table(session, connection, &settings, table, |opened| {
        with_clear_index(opened, path, || {
            create::drop_order(path, tag).map_err(|_| Error::CorruptIndex)
        })?;
        Ok(index::open_deciphering(opened, path).is_ok_and(|file| file.tags("").is_empty()))
    })?;
    if empty {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

/// `DROP INDEX table.name` on `connection`: the order leaves the table's
/// structural index file, as `AdsDeleteIndex` deletes it (see
/// [`index::delete`]). A Clipper table has none, so its orders are not
/// found, as in the original engine.
pub(crate) fn drop_index(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    table: &TableRef,
    name: &Named,
) -> Result<(), Error> {
    let not_found = || {
        Error::Sql(SqlError::ace(
            5016,
            "Advantage could not find an index order with the specified name.",
            "",
        ))
    };
    crate::transaction::check_legal(
        session,
        connection,
        crate::transaction::Illegal::DeleteIndex,
    )
    .map_err(crate::transaction::sql_error)?;
    if let Some(flavour) = flavour(settings) {
        let handle = open_named(session, connection, settings, &table.name)?;
        let structural =
            table::get(session, handle).map(|opened| opened.path.with_extension("cdx"));
        let dropped = match (flavour, structural) {
            (_, Err(error)) => Err(error),
            (Flavour::Clipper, _) => Err(not_found()),
            (_, Ok(structural)) => index::of_table(session, handle)
                .into_iter()
                .find(|&order| {
                    index::get(session, order).is_ok_and(|index| {
                        index.path == structural && index.tag.name.eq_ignore_ascii_case(&name.name)
                    })
                })
                .ok_or_else(not_found)
                .and_then(|order| index::delete(session, order)),
        };
        let _ = table::close(session, handle);
        return dropped;
    }
    // An order of a dictionary's table is in the index file the dictionary
    // names for it, and leaves the dictionary too.
    if in_dictionary(session, connection, &table.name) {
        rights::require_table(
            session,
            connection,
            &table.name,
            rights::ALTER,
            "Permission to modify this table not granted.",
        )?;
        let path =
            crate::dictionary_write::index_file_of(session, connection, &table.name, &name.name)
                .ok_or_else(not_found)?;
        let (dropped, empty) = with_table(session, connection, settings, &table.name, |opened| {
            let dropped = with_clear_index(opened, &path, || {
                create::drop_order(&path, &name.name).map_err(|_| Error::CorruptIndex)
            })?;
            let empty =
                index::open_deciphering(opened, &path).is_ok_and(|file| file.tags("").is_empty());
            Ok((dropped, empty))
        })?;
        if !dropped {
            return Err(not_found());
        }
        crate::dictionary_write::delete_index(session, connection, &table.name, &name.name, false)?;
        // The file goes with its last order (`fixtures/sql/ddoptsdict`).
        if empty {
            let _ = std::fs::remove_file(&path);
        }
        return Ok(());
    }
    with_table(session, connection, settings, &table.name, |opened| {
        let path = opened.path.with_extension("adi");
        let dropped = with_clear_index(opened, &path, || {
            create::drop_order(&path, &name.name).map_err(|_| Error::CorruptIndex)
        })?;
        match dropped {
            true => Ok(()),
            false => Err(not_found()),
        }
    })
}
