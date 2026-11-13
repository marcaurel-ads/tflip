//! The catalog system procedures the ODBC and JDBC drivers call
//! (`sp_GetCatalogs`, `sp_GetTypeInfo`, `sp_GetProcedureColumns`,
//! `sp_SupportsConvert`, ...), the ones that describe the system procedures
//! themselves (`sp_RetrieveSystemProcProperty`), and the collations
//! (`sp_GetCollations`, `sp_GetCollationTable`). Their result sets are as the
//! original engine's (`fixtures/sql/spcat.json`, `spddcat.json`, `spcoll.json`).

use std::sync::OnceLock;

use tflip_sql::V;
use tflip_sql::ast::Execute;
use tflip_storage::build::Column;
use tflip_storage::dd::{Kind as Object, procedure};
use tflip_storage::field::{Field, FieldType};
use tflip_storage::value::Value;

use super::args::{Kind, Spec, check_arguments, column};
use super::{Results, name_matches};
use crate::ace_error;
use crate::handle::Handle;
use crate::{Error, Session, connection, psm};

const NONE: &[(&str, Kind)] = &[];

const SPECS: [Spec; 12] = [
    Spec {
        name: "SP_GETCATALOGS",
        params: NONE,
        required: 0,
        results: true,
    },
    Spec {
        name: "SP_GETSCHEMAS",
        params: NONE,
        required: 0,
        results: true,
    },
    Spec {
        name: "SP_GETTABLETYPES",
        params: NONE,
        required: 0,
        results: true,
    },
    Spec {
        name: "SP_GETTYPEINFO",
        params: NONE,
        required: 0,
        results: true,
    },
    Spec {
        name: "SP_SUPPORTSCONVERT",
        params: &[("fromType", Kind::Integer), ("toType", Kind::Integer)],
        required: 2,
        results: true,
    },
    Spec {
        name: "SP_SUPPORTSTXNISOLATIONLEVEL",
        params: &[("level", Kind::Integer)],
        required: 1,
        results: true,
    },
    Spec {
        name: "SP_RETRIEVESYSTEMPROCPROPERTY",
        params: &[("Name", Kind::Text), ("Property", Kind::Short)],
        required: 2,
        results: true,
    },
    Spec {
        name: "SP_GETPROCEDURECOLUMNS",
        params: &[
            ("catalog", Kind::Text),
            ("schemaPattern", Kind::Text),
            ("procedureNamePattern", Kind::Text),
            ("columnNamePattern", Kind::Text),
        ],
        required: 4,
        results: true,
    },
    Spec {
        name: "SP_GETPROCEDURES",
        params: &[
            ("catalog", Kind::Text),
            ("schema", Kind::Text),
            ("procedureNamePattern", Kind::Text),
        ],
        required: 3,
        results: true,
    },
    Spec {
        name: "SP_GETCOLLATIONS",
        params: &[("Pattern", Kind::Text)],
        required: 1,
        results: true,
    },
    Spec {
        name: "SP_GETUNICODECONVERTERS",
        params: NONE,
        required: 0,
        results: true,
    },
    Spec {
        name: "SP_GETCOLLATIONTABLE",
        params: &[
            ("Name", Kind::Text),
            ("ShortName", Kind::Text),
            ("CodePage", Kind::Integer),
        ],
        required: 3,
        results: true,
    },
];

fn find(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

/// Whether `name` is one of these procedures.
pub(crate) fn handles(name: &str) -> bool {
    find(name).is_some()
}

/// The parameters of the procedure `name`, as fields (none for one that is
/// not one of these).
pub(crate) fn inputs(name: &str) -> Vec<Field> {
    find(name).map_or_else(Vec::new, super::args::inputs)
}

fn ci(name: &str, length: u16) -> Column {
    column(name, FieldType::CiCharacter, length)
}

fn short(name: &str) -> Column {
    column(name, FieldType::ShortInt, 2)
}

fn integer(name: &str) -> Column {
    column(name, FieldType::Integer, 4)
}

fn text(s: &str) -> Value {
    Value::Text(s.to_owned())
}

fn int(n: impl Into<i128>) -> Value {
    Value::Integer(n.into())
}

/// Runs the procedure `execute` calls with `args` on `connection`.
pub(crate) fn run(
    session: &Session,
    connection: Handle,
    execute: &Execute,
    args: &[V],
) -> Result<Option<Results>, Error> {
    let spec = find(&execute.name).ok_or(Error::NotImplemented)?;
    let args = check_arguments(spec, execute, args)?;
    let result = match spec.name {
        "SP_GETCATALOGS" => catalogs(session, connection)?,
        "SP_GETSCHEMAS" => Results::new(vec![ci("TABLE_SCHEM", 200)], Vec::new()),
        "SP_GETTABLETYPES" => Results::new(
            vec![ci("TABLE_TYPE", 17)],
            ["TABLE", "VIEW", "SYSTEM TABLE", "LOCAL TEMPORARY"]
                .iter()
                .map(|t| vec![text(t)])
                .collect(),
        ),
        "SP_GETTYPEINFO" => type_info(
            connection::get(session, connection)?
                .with_database_opt(|_| ())
                .is_some(),
        ),
        "SP_SUPPORTSCONVERT" => {
            let supported = supports_convert(
                args.integer(0).unwrap_or(0) as i32,
                args.integer(1).unwrap_or(0) as i32,
            )?;
            Results::new(
                vec![column("IsTypeSupported", FieldType::Logical, 1)],
                vec![vec![Value::Logical(supported)]],
            )
        }
        // Read committed, which is also what no level asks for.
        "SP_SUPPORTSTXNISOLATIONLEVEL" => Results::new(
            vec![column("IsLevelSupported", FieldType::Logical, 1)],
            vec![vec![Value::Logical(matches!(
                args.integer(0).unwrap_or(0),
                0 | 2
            ))]],
        ),
        "SP_RETRIEVESYSTEMPROCPROPERTY" => {
            system_procedure_property(args.text(0).as_deref(), args.integer(1))?
        }
        "SP_GETPROCEDURECOLUMNS" => {
            procedure_columns(session, connection, args.text(2), args.text(3))?
        }
        "SP_GETPROCEDURES" => procedures(session, connection, args.text(0), args.text(2))?,
        "SP_GETCOLLATIONS" => collations(session, args.text(0).as_deref()),
        "SP_GETUNICODECONVERTERS" => unicode_converters(),
        _ => collation_table(session, args.text(0).as_deref())?,
    };
    Ok(Some(result))
}

/// `sp_GetCatalogs`: a free connection's folder; a dictionary's path, its
/// links and `system`.
fn catalogs(session: &Session, connection: Handle) -> Result<Results, Error> {
    let connect = connection::get(session, connection)?;
    let path = connect.path.to_string_lossy().into_owned();
    let mut rows = vec![vec![text(&path)]];
    if let Some(links) = connect.with_database_opt(|db| {
        db.dictionary()
            .all(Object::Link)
            .into_iter()
            .map(|link| link.name.clone())
            .collect::<Vec<_>>()
    }) {
        rows.extend(links.iter().map(|link| vec![text(link)]));
        rows.push(vec![text("system")]);
    }
    Ok(Results::new(vec![ci("TABLE_CAT", 200)], rows))
}

/// A row of a table that ships as text: a tab between the columns, `\N` for
/// NULL, the cell otherwise as it is.
fn cells(line: &str) -> impl Iterator<Item = Option<&str>> {
    line.split('\t').map(|cell| (cell != "\\N").then_some(cell))
}

/// `sp_GetTypeInfo`: the types, as the original engine lists them for ODBC
/// and JDBC.
fn type_info(dictionary: bool) -> Results {
    let columns = vec![
        ci("TYPE_NAME", 25),
        short("DATA_TYPE"),
        integer("PRECISION"),
        column("LITERAL_PREFIX", FieldType::Character, 5),
        column("LITERAL_SUFFIX", FieldType::Character, 5),
        column("CREATE_PARAMS", FieldType::Character, 200),
        short("NULLABLE"),
        column("CASE_SENSITIVE", FieldType::Logical, 1),
        short("SEARCHABLE"),
        column("UNSIGNED_ATTRIBUTE", FieldType::Logical, 1),
        column("FIXED_PREC_SCALE", FieldType::Logical, 1),
        column("AUTO_INCREMENT", FieldType::Logical, 1),
        column("LOCAL_TYPE_NAME", FieldType::Character, 25),
        short("MINIMUM_SCALE"),
        short("MAXIMUM_SCALE"),
        integer("SQL_DATA_TYPE"),
        integer("SQL_DATETIME_SUB"),
        integer("NUM_PREC_RADIX"),
    ];
    let mut lines: Vec<&str> = include_str!("type_info.tsv").lines().collect();
    // On a dictionary connection NUMERIC comes after VARCHAR, not after CHAR.
    if dictionary && let Some(at) = lines.iter().position(|l| l.starts_with("NUMERIC\t")) {
        let numeric = lines.remove(at);
        let money = lines
            .iter()
            .position(|l| l.starts_with("MONEY\t"))
            .unwrap_or(lines.len());
        lines.insert(money, numeric);
    }
    let rows = lines
        .into_iter()
        .map(|line| {
            cells(line)
                .zip(&columns)
                .map(|(cell, column)| match (cell, column.field_type) {
                    (None, _) => Value::Null,
                    (Some(cell), FieldType::Logical) => Value::Logical(cell == "T"),
                    (Some(cell), FieldType::ShortInt | FieldType::Integer) => {
                        int(cell.parse::<i64>().unwrap_or(0))
                    }
                    (Some(cell), _) => text(cell),
                })
                .collect()
        })
        .collect();
    Results::new(columns, rows)
}

/// The SQL types `sp_SupportsConvert` knows, and for each (in this order)
/// which it converts to.
const CONVERT_TYPES: [i32; 20] = [
    -11, -7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4, 5, 6, 7, 8, 12, 91, 92, 93,
];
const CONVERTS: [&str; 20] = [
    "10000110100000001000",
    "01110110111111111000",
    "01110110111111111000",
    "01110110111111111000",
    "00000000000000000000",
    "11110110111111111111",
    "11110110111111111111",
    "00000000000000000000",
    "11110110111111111111",
    "01110110111111111000",
    "01110110111111111000",
    "01110110111111111000",
    "01110110111111111000",
    "01110110111111111000",
    "01110110111111111000",
    "01110110111111111000",
    "11110110111111111111",
    "00000110100000001101",
    "00000110100000001011",
    "00000110100000001111",
];

/// `sp_SupportsConvert`: whether the ODBC type `from` converts to `to`. Type
/// 0 (what NULL is) converts to nothing; a type outside the list is refused,
/// unless it is the `from` that is 0.
fn supports_convert(from: i32, to: i32) -> Result<bool, Error> {
    if from == 0 {
        return Ok(false);
    }
    let at = |ty: i32| CONVERT_TYPES.iter().position(|&t| t == ty);
    let (Some(from), true) = (at(from), to == 0 || at(to).is_some()) else {
        return Err(ace_error::sql_bare(5066));
    };
    Ok(at(to).is_some_and(|to| CONVERTS[from].as_bytes()[to] == b'1'))
}

/// A system procedure of `system.systemprocedures`: its name, input and
/// output parameters, comment and required permissions.
pub(crate) struct SystemProcedure {
    pub name: &'static str,
    pub input: Option<&'static str>,
    pub output: Option<&'static str>,
    pub comment: Option<&'static str>,
    pub permissions: i32,
}

/// The rows of `system.systemprocedures`, in the original engine's order.
pub(crate) fn system_procedures() -> &'static [SystemProcedure] {
    static ROWS: OnceLock<Vec<SystemProcedure>> = OnceLock::new();
    ROWS.get_or_init(|| {
        include_str!("system_procedures.tsv")
            .lines()
            .map(|line| {
                let mut cells = cells(line);
                let mut next = || cells.next().flatten();
                SystemProcedure {
                    name: next().unwrap_or_default(),
                    input: next(),
                    output: next(),
                    comment: next(),
                    permissions: next().and_then(|n| n.parse().ok()).unwrap_or(0),
                }
            })
            .collect()
    })
}

/// `sp_RetrieveSystemProcProperty`: property 1, the comment, of the system
/// procedure `name` (regardless of case, and of blanks after it). Property
/// -1 is refused for a procedure that exists, any other property is not
/// valid for any, and that is found out before the procedure is looked up.
fn system_procedure_property(name: Option<&str>, property: Option<i64>) -> Result<Results, Error> {
    let property = property.unwrap_or(0);
    if property != 1 && property != -1 {
        return Err(ace_error::sql_bare(5126));
    }
    let found = name.and_then(|name| {
        system_procedures()
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name.trim_end()))
    });
    let Some(found) = found else {
        return Err(ace_error::sql(
            5132,
            "There is no such canned procedure in the data dictionary.",
        ));
    };
    if property == -1 {
        return Err(ace_error::sql(
            5133,
            "The specified property lenght was not the correct length, expected an 8 byte \
             integer",
        ));
    }
    // The memo's value ends with a NUL.
    let comment = found
        .comment
        .map_or(Value::Null, |c| text(&format!("{c}\0")));
    Ok(Results::new(
        vec![column("Value", FieldType::Memo, 9)],
        vec![vec![comment]],
    ))
}

/// The stored procedures of the connection's dictionary (none on a free
/// connection), in the dictionary's order, and its path.
fn dictionary_procedures(
    session: &Session,
    connection: Handle,
) -> Result<(String, Vec<tflip_storage::dd::Object>), Error> {
    let connect = connection::get(session, connection)?;
    let path = connect.path.to_string_lossy().into_owned();
    let procedures = connect
        .with_database_opt(|db| {
            db.dictionary()
                .all(Object::Procedure)
                .into_iter()
                .filter(|p| db.shows(p))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok((path, procedures))
}

/// The procedure name a pattern argument means: blanks after it do not
/// count.
fn pattern(arg: Option<&str>) -> Option<&str> {
    arg.map(str::trim_end)
}

/// `sp_GetProcedures`: the dictionary's stored procedures matching
/// `pattern` of the catalog `catalog` (all for none; the dictionary's path,
/// as a `LIKE` pattern: a link's procedures are not listed).
fn procedures(
    session: &Session,
    connection: Handle,
    catalog: Option<String>,
    name: Option<String>,
) -> Result<Results, Error> {
    let columns = vec![
        ci("PROCEDURE_CAT", 200),
        ci("PROCEDURE_SCHEM", 200),
        ci("PROCEDURE_NAME", 255),
        integer("RFFU1"),
        integer("RFFU2"),
        integer("RFFU3"),
        column("REMARKS", FieldType::Memo, 9),
        short("PROCEDURE_TYPE"),
    ];
    // A user other than the administrator reads every procedure's rights slot
    // and the listing fails at one that does not check, with no name to give
    // (`fixtures/sql/ddpublicbad6`).
    let refused = connection::get(session, connection)?
        .with_database_opt(|db| {
            db.is_ordinary_user() && db.any_public_slot_bad(&[tflip_storage::dd::Kind::Procedure])
        })
        .unwrap_or(false);
    if refused {
        return Err(crate::dictionary_write::not_valid(
            "There is no such named procedure in the data dictionary: ",
        ));
    }
    let (path, procedures) = dictionary_procedures(session, connection)?;
    let listed = pattern(catalog.as_deref()).is_none_or(|c| name_matches(&path, Some(c)));
    let rows = procedures
        .iter()
        .filter(|p| listed && name_matches(&p.name, pattern(name.as_deref())))
        .map(|p| {
            vec![
                text(&path),
                text(""),
                text(&p.name),
                int(0),
                int(0),
                int(0),
                p.comment.clone().map_or(Value::Null, Value::Text),
                int(2),
            ]
        })
        .collect();
    Ok(Results::new(columns, rows))
}

/// `sp_GetProcedureColumns`: the parameters of the dictionary's stored
/// procedures matching `procedure`, those matching `column`, the input ones
/// first and the output ones after (`COLUMN_TYPE` 1 and 3). The catalog and
/// schema arguments filter nothing.
fn procedure_columns(
    session: &Session,
    connection: Handle,
    procedure_name: Option<String>,
    column_name: Option<String>,
) -> Result<Results, Error> {
    let columns = vec![
        ci("PROCEDURE_CAT", 200),
        ci("PROCEDURE_SCHEM", 200),
        ci("PROCEDURE_NAME", 200),
        ci("COLUMN_NAME", 200),
        short("COLUMN_TYPE"),
        short("DATA_TYPE"),
        ci("TYPE_NAME", 20),
        integer("PRECISION"),
        integer("LENGTH"),
        short("SCALE"),
        integer("RADIX"),
        short("NULLABLE"),
        column("REMARKS", FieldType::Memo, 9),
    ];
    let (path, procedures) = dictionary_procedures(session, connection)?;
    let mut rows = Vec::new();
    for object in &procedures {
        if !name_matches(&object.name, pattern(procedure_name.as_deref())) {
            continue;
        }
        for (slot, kind) in [(procedure::INPUT, 1), (procedure::OUTPUT, 3)] {
            for (name, ty) in psm::params(object, slot) {
                let Ok(field) = tflip_sql::script::declared_field(&name, &ty) else {
                    continue;
                };
                if !name_matches(&name, pattern(column_name.as_deref())) {
                    continue;
                }
                let (data_type, type_name, precision, length, scale, radix) =
                    match ty.trim().eq_ignore_ascii_case("MODTIME") {
                        // A variable of that type is a plain timestamp.
                        true => (11, "MODTIME", 8, 8, None, None),
                        false => parameter_type(&field),
                    };
                rows.push(vec![
                    text(&path),
                    text(""),
                    text(&object.name),
                    text(&name),
                    int(kind),
                    int(data_type),
                    text(type_name),
                    int(precision),
                    int(length),
                    scale.map_or(Value::Null, int),
                    radix.map_or(Value::Null, int),
                    int(1),
                    Value::Null,
                ]);
            }
        }
    }
    Ok(Results::new(columns, rows))
}

/// How `sp_GetProcedureColumns` describes a parameter of `field`'s type: its
/// ODBC type, the name of the type, its precision and length, scale and
/// radix.
fn parameter_type(field: &Field) -> (i64, &'static str, i64, i64, Option<i64>, Option<i64>) {
    let length = i64::from(field.length());
    let decimals = i64::from(field.decimals());
    let sized = |data_type, type_name| (data_type, type_name, length, length, None, None);
    let fixed = |data_type, type_name, size| (data_type, type_name, size, size, None, None);
    let number = |data_type, type_name, size| (data_type, type_name, size, size, None, Some(10));
    match field.field_type {
        FieldType::Integer | FieldType::AutoInc => number(4, "Integer", 4),
        FieldType::ShortInt => number(5, "SHORTINT", 2),
        FieldType::LongInt => number(-5, "LONGINTEGER", 8),
        FieldType::Double => number(8, "DOUBLE", 8),
        FieldType::CurDouble => number(8, "CURDOUBLE", 8),
        FieldType::Money => number(2, "Money", 8),
        FieldType::RowVersion => number(2, "Rowversion", 8),
        FieldType::Numeric => {
            let precision = length - 1 - i64::from(decimals > 0);
            (2, "Numeric", precision, precision, Some(decimals), Some(10))
        }
        FieldType::Date | FieldType::CompactDate => fixed(9, "DATE", 4),
        FieldType::Time => fixed(10, "TIME", 4),
        FieldType::TimeStamp => fixed(11, "TIMESTAMP", 8),
        FieldType::ModTime => fixed(11, "MODTIME", 8),
        FieldType::Logical => fixed(-7, "LOGICAL", 1),
        FieldType::Guid => fixed(-11, "GUID", 16),
        FieldType::Character => sized(1, "CHAR"),
        FieldType::CiCharacter => sized(1, "CICHAR"),
        FieldType::VarChar => sized(12, "VARCHARFOX"),
        FieldType::NChar => sized(-8, "NCHAR"),
        FieldType::NVarChar => sized(-9, "NVARCHAR"),
        FieldType::Raw | FieldType::Other(_) => sized(-2, "RAW"),
        FieldType::VarBinary => sized(-3, "VARBINARYFOX"),
        FieldType::Memo | FieldType::LegacyVarChar => fixed(-1, "Memo", 2_147_483_640),
        FieldType::Binary | FieldType::Image => fixed(-4, "BINARY", 2_147_483_647),
        FieldType::NMemo => (-10, "NMEMO", 8, 9, None, None),
    }
}

/// The ICU locales `sp_GetCollations` lists after the repository's: name,
/// short name and description, in name order.
fn icu_collations() -> impl Iterator<Item = [&'static str; 3]> {
    include_str!("icu_collations.tsv").lines().map(|line| {
        let mut cells = line.split('\t');
        std::array::from_fn(|_| cells.next().unwrap_or_default())
    })
}

/// `sp_GetCollations`: the repository's collations (`adscollate.adt`, as the
/// engine reads it from its folder) and the ICU Unicode locales whose names
/// match `pattern`, regardless of case and of blanks after it (all for none or
/// for none that is not blank).
fn collations(session: &Session, pattern_arg: Option<&str>) -> Results {
    let columns = vec![
        ci("Name", 100),
        ci("ShortName", 8),
        column("Description", FieldType::Memo, 9),
        integer("Version"),
        integer("CodePage"),
        column("FoxCompat", FieldType::Logical, 1),
        column("UnicodeLocale", FieldType::Character, 16),
        column("AllowMultiple", FieldType::Logical, 1),
    ];
    let pattern = pattern(pattern_arg).filter(|p| !p.is_empty());
    let mut records = session.settings.collations().records();
    // A pattern that starts with a letter is looked up by an index on the name
    // (regardless of case); any other reads the file in its order.
    if pattern.is_some_and(|p| !p.starts_with(['%', '_'])) {
        records.sort_by_key(|record| record.name.to_ascii_lowercase());
    }
    let mut rows = Vec::new();
    for record in records {
        if name_matches(&record.name, pattern) {
            rows.push(vec![
                text(&record.name),
                text(&record.short_name),
                record.description.map_or(Value::Null, Value::Text),
                int(record.version),
                int(record.code_page),
                Value::Logical(record.fox_compat),
                record.unicode_locale.map_or(Value::Null, Value::Text),
                Value::Logical(record.allow_multiple),
            ]);
        }
    }
    for [name, short, description] in icu_collations() {
        if name_matches(name, pattern) {
            rows.push(vec![
                text(name),
                text(short),
                text(description),
                Value::Null,
                int(1202),
                Value::Logical(false),
                Value::Null,
                Value::Null,
            ]);
        }
    }
    Results::new(columns, rows)
}

/// `sp_GetUnicodeConverters`: the ICU converters the original engine ships,
/// numbered from 0, with the code page each stands for (`0` for those that
/// are none); the description is always NULL.
fn unicode_converters() -> Results {
    let columns = vec![
        integer("ID"),
        ci("Name", 200),
        column("DESC", FieldType::Memo, 9),
        column("Codepage", FieldType::Character, 20),
    ];
    let rows = include_str!("unicode_converters.tsv")
        .lines()
        .map(|line| {
            let mut cells = line.split('\t');
            let mut next = || cells.next().unwrap_or_default();
            vec![
                int(next().parse::<i32>().unwrap_or_default()),
                text(next()),
                Value::Null,
                text(next()),
            ]
        })
        .collect();
    Results::new(columns, rows)
}

/// `sp_GetCollationTable`: the repository's record of the collation `name`
/// (regardless of case, and of blanks after it; the other arguments play no
/// part), numbered by the order the engine loaded the collations in.
fn collation_table(session: &Session, name: Option<&str>) -> Result<Results, Error> {
    let collations = session.settings.collations();
    let name = name.map(str::trim_end).unwrap_or_default();
    let found = collations
        .records()
        .into_iter()
        .find(|r| r.name.eq_ignore_ascii_case(name));
    let Some(record) = found else {
        return Err(ace_error::sql_bare(7147));
    };
    let binary = |name: &str| column(name, FieldType::Binary, 9);
    let columns = vec![
        integer("CollationID"),
        ci("Name", 100),
        ci("ShortName", 8),
        integer("Version"),
        integer("CodePage"),
        binary("CE"),
        binary("Upper"),
        binary("Lower"),
        integer("NumContractions"),
        binary("Contractions"),
        column("AllowMultiple", FieldType::Logical, 1),
        column("UnicodeLocale", FieldType::Character, 16),
    ];
    let contractions = record.contractions.as_deref().map_or(0, <[u8]>::len) / 4;
    let row = vec![
        int(collations.number(&record.name)),
        text(&record.name),
        text(&record.short_name),
        int(record.version),
        int(record.code_page),
        Value::Binary(record.elements),
        Value::Binary(record.upper),
        Value::Binary(record.lower),
        int(contractions as i64),
        record.contractions.map_or(Value::Null, Value::Binary),
        Value::Logical(record.allow_multiple),
        record.unicode_locale.map_or(Value::Null, Value::Text),
    ];
    Ok(Results::new(columns, vec![row]))
}
