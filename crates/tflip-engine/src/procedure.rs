//! System procedures (`EXECUTE PROCEDURE sp_…`), as far as clients such as
//! Advantage Data Architect need them: the application ID, and the tables
//! and columns of a free-table connection or of a dictionary (its views and
//! links among them). Their results are as the original engine gives them
//! (`fixtures/sql/statements.json`, `spdict.json`, `spview.json`).

use std::fs;
use std::path::PathBuf;

use tflip_sql::ast::Execute;
use tflip_sql::{SqlError, V};
use tflip_storage::build::Column;
use tflip_storage::dd::{self, Kind};
use tflip_storage::field::{Field, FieldType};
use tflip_storage::table::{Format, Structure};
use tflip_storage::value::Value;

use crate::ace_error;
use crate::dictionary::Database;
use crate::handle::Handle;
use crate::view_columns::Described;
use crate::{Error, Session, connection, shares};
use args::{column, text_of};

pub(crate) mod args;
mod backup;
mod catalog;
mod dictionary;
pub(crate) use dictionary::rename_dd_object;
mod events;
mod query_log;
mod server;
mod tables;

/// A procedure's result set.
pub struct Results {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
    /// The rows the cursor shows, by their 1-based numbers, if not all: the
    /// others are filtered out, the shown ones keep their numbers.
    pub shown: Option<Vec<u32>>,
}

impl Results {
    /// A result set that shows all its rows.
    pub fn new(columns: Vec<Column>, rows: Vec<Vec<Value>>) -> Results {
        Results {
            columns,
            rows,
            shown: None,
        }
    }
}

/// The procedures tflip has, by name in upper case.
const PROCEDURES: [&str; 9] = [
    "SP_CREATEINDEX",
    "SP_CREATEINDEX90",
    "SP_SETAPPLICATIONID",
    "SP_GETAPPLICATIONID",
    "SP_GETCOLUMNS",
    "SP_GETTABLES",
    "SP_MGGETINSTALLINFO",
    "SP_GETSQLSTATEMENTS",
    "SP_GETSQLKEYWORDS",
];

/// The system procedures `system.systemprocedures` lists.
pub(crate) use catalog::system_procedures;

/// Whether `name` is one of the built-in system procedures (`sp_…`).
pub fn is_builtin(name: &str) -> bool {
    PROCEDURES.contains(&name.to_ascii_uppercase().as_str())
        || tables::handles(name)
        || server::handles(name)
        || events::handles(name)
        || dictionary::handles(name)
        || catalog::handles(name)
        || query_log::handles(name)
        || backup::handles(name)
}

/// Whether `name` is a procedure of event notifications, which a script, a
/// stored procedure or a trigger may call.
pub(crate) fn is_event(name: &str) -> bool {
    events::handles(name)
}

/// Whether the built-in procedure `name` has a result set to select from (the
/// others do their work and return nothing).
pub fn has_results(name: &str) -> bool {
    if dictionary::handles(name) {
        return dictionary::has_results(name);
    }
    tables::has_results(name)
        && server::has_results(name)
        && events::has_results(name)
        && query_log::has_results(name)
        && !matches!(
            name.to_ascii_uppercase().as_str(),
            "SP_SETAPPLICATIONID" | "SP_CREATEINDEX" | "SP_CREATEINDEX90"
        )
}

/// How many arguments the built-in procedure `name` takes, where the
/// original engine insists on it (probed for the two that list tables and
/// columns).
pub fn arguments(name: &str) -> Option<usize> {
    match name.to_ascii_uppercase().as_str() {
        "SP_GETTABLES" | "SP_GETCOLUMNS" => Some(4),
        _ => None,
    }
}

/// The parameters of the built-in procedure `name` that has them declared, as
/// fields: what types a call's parameters (`?`) get.
pub(crate) fn inputs(name: &str) -> Vec<tflip_storage::field::Field> {
    let mut fields = tables::inputs(name);
    if fields.is_empty() {
        fields = server::inputs(name);
    }
    if fields.is_empty() {
        fields = events::inputs(name);
    }
    if fields.is_empty() {
        fields = dictionary::inputs(name);
    }
    if fields.is_empty() {
        fields = catalog::inputs(name);
    }
    if fields.is_empty() {
        fields = query_log::inputs(name);
    }
    if fields.is_empty() {
        fields = backup::inputs(name);
    }
    fields
}

/// Checks that the built-in procedure `execute` calls exists.
pub fn check(execute: &Execute) -> Result<(), Error> {
    if is_builtin(&execute.name) {
        Ok(())
    } else {
        Err(Error::Sql(SqlError::no_procedure(
            &execute.name,
            execute.pos,
        )))
    }
}

/// Runs the procedure `execute` calls with `args` on `connection` for the
/// statement `statement`; returns its result set, if it has one.
pub fn run(
    session: &Session,
    statement: Handle,
    connection: Handle,
    execute: &Execute,
    args: &[V],
) -> Result<Option<Results>, Error> {
    run_from(
        session,
        statement,
        connection,
        execute,
        args,
        crate::events::Context::Direct,
    )
}

/// [`run`] for a call from `context`: the procedures of events act by it.
pub(crate) fn run_from(
    session: &Session,
    statement: Handle,
    connection: Handle,
    execute: &Execute,
    args: &[V],
    context: crate::events::Context,
) -> Result<Option<Results>, Error> {
    check(execute)?;
    if events::handles(&execute.name) {
        return events::run(session, statement, connection, execute, args, context);
    }
    if dictionary::handles(&execute.name) {
        return dictionary::run(session, connection, execute, args);
    }
    if catalog::handles(&execute.name) {
        return catalog::run(session, connection, execute, args);
    }
    if query_log::handles(&execute.name) {
        return query_log::run(session, statement, connection, execute, args);
    }
    if backup::handles(&execute.name) {
        return backup::run(session, connection, execute, args);
    }
    let text = |i: usize| text_of(args.get(i));
    match execute.name.to_ascii_uppercase().as_str() {
        "SP_SETAPPLICATIONID" => {
            connection::set_application_id(session, connection, &text(0).unwrap_or_default())?;
            Ok(None)
        }
        // (table, file, tag, expression, condition, options, page size[,
        // collation]): an order in the index file `file`.
        "SP_CREATEINDEX" | "SP_CREATEINDEX90" => {
            let number = |i: usize| match args.get(i) {
                Some(V::Int(n)) => *n as u32,
                _ => 0,
            };
            let (table, file, tag, expression) = (text(0), text(1), text(2), text(3));
            let (Some(table), Some(tag), Some(expression)) = (table, tag, expression) else {
                return Err(Error::InvalidExpression);
            };
            let condition = text(4).unwrap_or_default();
            let page_size = number(6) as u16;
            let order = crate::ddl::Order {
                table: &table,
                file: file.as_deref(),
                tag: &tag,
                expression: &expression,
                condition: &condition,
                options: number(5),
                page_size,
            };
            let settings = crate::statement::settings_of(session, statement)?;
            crate::ddl::create_order(session, connection, &settings, &order)?;
            // A dictionary's table has the order's Key object, as `CREATE
            // INDEX` makes it.
            if crate::ddl::in_dictionary(session, connection, &table) {
                crate::dictionary_write::register_created_index(
                    session,
                    connection,
                    &table,
                    file.as_deref(),
                    &tag,
                    None,
                )?;
            }
            Ok(None)
        }
        "SP_GETAPPLICATIONID" => {
            let id = connection::application_id(session, connection)?;
            Ok(Some(Results::new(
                vec![column("ApplicationID", FieldType::Memo, 9)],
                vec![vec![Value::Text(id)]],
            )))
        }
        "SP_GETCOLUMNS" => {
            // The original engine fails like this without a table pattern.
            let Some(table_pattern) = text(2) else {
                return Err(unreadable_handle());
            };
            let catalog = text(0);
            get_columns(
                session,
                connection,
                catalog.as_deref(),
                &table_pattern,
                text(3).as_deref(),
            )
        }
        // (catalog, schema, table pattern, table type).
        "SP_GETTABLES" => {
            let (catalog, pattern, kind) = (text(0), text(2), text(3));
            get_tables(
                session,
                connection,
                catalog.as_deref(),
                pattern.as_deref(),
                kind.as_deref(),
            )
        }
        "SP_MGGETINSTALLINFO" => Ok(Some(install_info())),
        "SP_GETSQLKEYWORDS" => Ok(Some(sql_keywords())),
        name if tables::handles(name) => tables::run(session, statement, connection, execute, args),
        name if server::handles(name) => server::run(session, connection, execute, args),
        _ => sql_statements(session, statement),
    }
}

/// The procedures a user of a dictionary who is not its administrator, nor in
/// `DB:Admin`, may not run (`system.systemprocedures` has them under
/// `Required_Permissions`, though not all of what it names is gated: the
/// original's local server lets such a user run the other `sp_mg…`
/// procedures). They are refused before the arguments are looked at.
const ADMINISTRATORS_ONLY: [&str; 4] = [
    "SP_MGKILLUSER",
    "SP_MGKILLUSER90",
    "SP_GETSQLSTATEMENTS",
    "SP_CANCELQUERY",
];

/// Whether `connection` is to a dictionary whose user may not change it.
fn is_ordinary_user(session: &Session, connection: Handle) -> bool {
    connection::get(session, connection)
        .ok()
        .and_then(|owner| owner.with_database_opt(|db| !crate::link::may_create(db)))
        .unwrap_or(false)
}

/// Refuses the procedure `name` (5054) to a user who may not run it. The
/// statement asks before it looks at the arguments.
pub fn refuse_user(session: &Session, connection: Handle, name: &str) -> Result<(), Error> {
    let upper = name.to_ascii_uppercase();
    match ADMINISTRATORS_ONLY.contains(&upper.as_str()) && is_ordinary_user(session, connection) {
        true => Err(ace_error::sql_bare(5054)),
        false => Ok(()),
    }
}

/// `sp_mgGetInstallInfo`: the local server's install information, as the
/// original engine's reports it (as `AdsMgGetInstallInfo` does).
fn install_info() -> Results {
    let text = |name: &str, length| column(name, FieldType::CiCharacter, length);
    let columns = vec![
        text("Owner", 36),
        text("SerialNumber", 16),
        column("UserOption", FieldType::Integer, 4),
        text("Version", 16),
        text("InstallDate", 16),
        text("EvalExpireDate", 16),
        text("ANSI", 16),
        text("OEM", 16),
        column("ReplicationEnabled", FieldType::Logical, 1),
        column("MaxStatefulUsers", FieldType::Integer, 4),
        column("MaxStatelessUsers", FieldType::Integer, 4),
    ];
    let s = |text: &str| Value::Text(text.to_owned());
    let rows = vec![vec![
        s(""),
        s(""),
        Value::Integer(5),
        s("12.0.0.0"),
        s(""),
        s(""),
        s("ENGL(AMER)"),
        s("USA"),
        Value::Logical(false),
        Value::Integer(5),
        Value::Integer(0),
    ]];
    Results::new(columns, rows)
}

/// `sp_GetSQLKeywords`: the reserved SQL words, one `Keyword` column, in the
/// original engine's order (not quite alphabetical — `FUNCTION` before `FULL`,
/// `REVOKE` before `RETURN`). ARC reads it to colour its SQL editor.
fn sql_keywords() -> Results {
    let columns = vec![column("Keyword", FieldType::CiCharacter, 100)];
    let rows = tflip_sql::parse::KEYWORDS
        .iter()
        .map(|&k| vec![Value::Text(k.to_owned())])
        .collect();
    Results::new(columns, rows)
}

/// `sp_GetSQLStatements`: the session's statements with SQL prepared but
/// the one asking, as the original engine lists them for a free
/// connection; a server's session lists those of every client's connection to
/// the same dictionary or folder.
fn sql_statements(session: &Session, statement: Handle) -> Result<Option<Results>, Error> {
    let columns = vec![
        column("Query Number", FieldType::Integer, 4),
        column("Active", FieldType::Logical, 1),
        Column {
            decimals: 2,
            ..column("Percent Complete", FieldType::Double, 8)
        },
        column("Connection Name", FieldType::Character, 100),
        column("Database User", FieldType::Character, 100),
        column("Database", FieldType::Character, 255),
        column("Query", FieldType::Memo, 9),
        column("Is Script", FieldType::Logical, 1),
        column("Full Script", FieldType::Memo, 9),
        column("Start Time", FieldType::Time, 4),
        column("Seconds Until Finished", FieldType::Integer, 4),
        column("ApplicationID", FieldType::CiCharacter, 70),
    ];
    // The original engine names the user running the client here.
    let user = connection::os_user();
    // A server lists what every client runs on the dictionary; a local
    // engine, its own process's.
    let listed: Vec<(Session, crate::statement::Listed)> = match session.shares {
        Some(_) => crate::statement::listed_on_server(session, statement),
        None => crate::statement::listed(session, statement)
            .into_iter()
            .map(|listed| (session.clone(), listed))
            .collect(),
    };
    let mut rows = Vec::new();
    for (owning, listed) in listed {
        let Ok(owner) = connection::get(&owning, listed.connection) else {
            continue;
        };
        let application_id = connection::application_id(&owning, listed.connection)?;
        let database_user = match owning.shares {
            Some(_) => owner.user().map(|user| Value::Text(user.to_owned())),
            None => None,
        };
        rows.push(vec![
            Value::Integer(i128::from(listed.number)),
            Value::Logical(crate::statement::is_active(listed.number)),
            Value::Null,
            Value::Text(user.clone()),
            database_user.unwrap_or(Value::Null),
            Value::Text(owner.path.to_string_lossy().into_owned()),
            Value::Text(listed.sql),
            Value::Logical(false),
            Value::Null,
            Value::Time(listed.started),
            Value::Null,
            Value::Text(application_id),
        ]);
    }
    Ok(Some(Results::new(columns, rows)))
}

/// The tables of the connection's folder whose names match `pattern` (a
/// `LIKE` pattern, regardless of case, with or without the extension), in
/// name order: their paths and their names as the result shows them,
/// without the extension unless the pattern has it.
fn tables(
    session: &Session,
    connection: Handle,
    pattern: Option<&str>,
    with_extension: bool,
) -> Result<Vec<(PathBuf, String)>, Error> {
    let owner = connection::get(session, connection)?;
    let folder = match session.shares {
        Some(_) => shares::inside(&owner.path, ".")?,
        None => owner.path.clone(),
    };
    let mut names: Vec<String> = fs::read_dir(&folder)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|name| name.to_ascii_lowercase().ends_with(".adt"))
                .collect()
        })
        .unwrap_or_default();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    let pattern_has_extension = pattern.is_some_and(|p| p.to_ascii_lowercase().ends_with(".adt"));
    Ok(names
        .into_iter()
        .filter(|name| {
            let stem = &name[..name.len() - 4];
            pattern.is_none_or(|p| {
                tflip_sql::eval::like(name, p, None, true)
                    || tflip_sql::eval::like(stem, p, None, true)
            })
        })
        .map(|name| {
            let shown = match with_extension || pattern_has_extension {
                true => name.clone(),
                false => name[..name.len() - 4].to_owned(),
            };
            (folder.join(&name), shown)
        })
        .collect())
}

/// `sp_GetTables`: the tables matching `pattern`, in the catalogs `catalog`
/// names, of the type `kind` (any for none).
fn get_tables(
    session: &Session,
    connection: Handle,
    catalog: Option<&str>,
    pattern: Option<&str>,
    kind: Option<&str>,
) -> Result<Option<Results>, Error> {
    let connect = connection::get(session, connection)?;
    let path = connect.path.to_string_lossy().into_owned();
    let columns = vec![
        column("TABLE_CAT", FieldType::CiCharacter, 200),
        column("TABLE_SCHEM", FieldType::CiCharacter, 200),
        column("TABLE_NAME", FieldType::CiCharacter, 255),
        column("TABLE_TYPE", FieldType::CiCharacter, 17),
        column("REMARKS", FieldType::Memo, 9),
    ];
    // On a dictionary connection the engine lists the dictionary's own tables
    // and views, its links' tables, then the system tables; on a free
    // connection, the folder's tables.
    let filter = Filter {
        catalog,
        pattern,
        kind,
    };
    // A user held to its rights is refused the listing where a table or view
    // has a rights slot for DB:Public that does not check
    // (`fixtures/sql/ddpublicbad`).
    if connect
        .with_database_opt(|db| {
            db.lists_by_rights() && db.any_public_slot_bad(&[Kind::Table, Kind::View, Kind::Link])
        })
        .unwrap_or(false)
    {
        // A table says what is wrong, a view or a link does not.
        let table = connect
            .with_database_opt(|db| db.any_public_slot_bad(&[Kind::Table]))
            .unwrap_or(false);
        return Err(match table {
            true => crate::ace_error::sql(
                5127,
                "Inconsistency detected while reading stored property.",
            ),
            false => Error::Sql(tflip_sql::SqlError::ace_bare(
                5127,
                "The supplied property is not valid.",
            )),
        });
    }
    if let Some((rows, shown)) = connect.with_database_opt(|db| dict_table_rows(db, &path, &filter))
    {
        return Ok(Some(Results {
            columns,
            rows,
            shown,
        }));
    }
    let rows = tables(session, connection, pattern, true)?
        .into_iter()
        .map(|(_, name)| {
            vec![
                Value::Text(path.clone()),
                Value::Null,
                Value::Text(name),
                Value::Text("TABLE".to_owned()),
                Value::Null,
            ]
        })
        .collect();
    Ok(Some(Results::new(columns, rows)))
}

/// Whether `name` matches the `LIKE` `pattern` (any name for none), regardless
/// of case.
fn name_matches(name: &str, pattern: Option<&str>) -> bool {
    pattern.is_none_or(|p| tflip_sql::eval::like(name, p, None, true))
}

/// What the dictionary procedures list: the first argument names the
/// catalogs (all for none; the dictionary's own for an empty one; else those
/// whose name — a link's, `system`, the dictionary's path — it matches as a
/// `LIKE` pattern, regardless of case), the table pattern the tables, and
/// `sp_GetTables`' last argument a table type (any for none or blanks; else
/// one type, matched regardless of case and surrounding blanks).
struct Filter<'a> {
    catalog: Option<&'a str>,
    pattern: Option<&'a str>,
    kind: Option<&'a str>,
}

impl Filter<'_> {
    /// Whether the dictionary's own objects are listed: its path is `path`.
    fn own(&self, path: &str) -> bool {
        self.catalog
            .is_none_or(|c| c.is_empty() || tflip_sql::eval::like(path, c, None, true))
    }

    /// Whether the catalog `name` (a link's, or `system`) is listed.
    fn catalog(&self, name: &str) -> bool {
        self.catalog
            .is_none_or(|c| !c.is_empty() && tflip_sql::eval::like(name, c, None, true))
    }

    fn name(&self, name: &str) -> bool {
        name_matches(name, self.pattern)
    }
}

/// The dictionary the link `link` of `db` is to, if it can be read.
fn linked(db: &Database, link: &dd::Object) -> Option<Database> {
    let relative = link.text(dd::link::RELATIVE_PATH)?;
    Database::open(&db.path(&relative)).ok()
}

/// The `sp_GetTables` rows of a dictionary connection, and those shown if not
/// all: the dictionary's tables and views in object order (`TABLE`/`VIEW`,
/// with the object's comment) and, where a link is, the linked dictionary's
/// tables (not its views or links, under the link's name); then the system
/// tables (catalog `system`, type `SYSTEM TABLE`). As in the original engine,
/// a table type of `SYSTEM TABLE` lists the system tables alone, and another
/// shows those of the dictionary's and links' rows that are of the type, with
/// the numbers they have among them all.
fn dict_table_rows(
    db: &Database,
    path: &str,
    filter: &Filter,
) -> (Vec<Vec<Value>>, Option<Vec<u32>>) {
    let row = |catalog: &str, object: &dd::Object, kind: &str| {
        vec![
            Value::Text(catalog.to_owned()),
            Value::Null,
            Value::Text(object.name.clone()),
            Value::Text(kind.to_owned()),
            object.comment.clone().map_or(Value::Null, Value::Text),
        ]
    };
    let kind = filter.kind.map(str::trim).filter(|k| !k.is_empty());
    let system_only = kind.is_some_and(|k| k.eq_ignore_ascii_case("SYSTEM TABLE"));
    let mut rows = Vec::new();
    for object in db
        .dictionary()
        .objects()
        .iter()
        .filter(|o| !system_only && db.shows(o))
    {
        match object.kind {
            Kind::Table | Kind::View => {
                let kind = if object.kind == Kind::Table {
                    "TABLE"
                } else {
                    "VIEW"
                };
                if filter.own(path) && filter.name(&object.name) {
                    rows.push(row(path, object, kind));
                }
            }
            Kind::Link if filter.catalog(&object.name) => {
                let Some(linked) = linked(db, object) else {
                    continue;
                };
                for table in linked.dictionary().all(Kind::Table) {
                    if filter.name(&table.name) {
                        rows.push(row(&object.name, table, "TABLE"));
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(kind) = kind.filter(|_| !system_only) {
        let shown: Vec<u32> = (1..)
            .zip(&rows)
            .filter(|(_, row)| matches!(&row[3], Value::Text(k) if k.eq_ignore_ascii_case(kind)))
            .map(|(number, _)| number)
            .collect();
        return match shown.is_empty() {
            true => (Vec::new(), None),
            false => (rows, Some(shown)),
        };
    }
    if filter.catalog("system") {
        for &(name, ..) in crate::system::SYSTEM_TABLE_CATALOG {
            if filter.name(name) {
                rows.push(vec![
                    Value::Text("system".to_owned()),
                    Value::Text(String::new()),
                    Value::Text(name.to_owned()),
                    Value::Text("SYSTEM TABLE".to_owned()),
                    Value::Null,
                ]);
            }
        }
    }
    (rows, None)
}

/// The original engine's error when it cannot read what it was asked for:
/// `sp_GetColumns` without a table pattern, or with one that takes in a
/// system table.
fn unreadable_handle() -> Error {
    ace_error::sql(5018, "The given handle is not readable.")
}

/// `sp_GetColumns`: the columns matching `column_pattern` of the tables
/// matching `table_pattern`, in the catalogs `catalog` names.
fn get_columns(
    session: &Session,
    connection: Handle,
    catalog: Option<&str>,
    table_pattern: &str,
    column_pattern: Option<&str>,
) -> Result<Option<Results>, Error> {
    let connect = connection::get(session, connection)?;
    let path = connect.path.to_string_lossy().into_owned();
    let columns = vec![
        column("TABLE_CAT", FieldType::CiCharacter, 200),
        column("TABLE_SCHEM", FieldType::CiCharacter, 200),
        column("TABLE_NAME", FieldType::CiCharacter, 255),
        column("COLUMN_NAME", FieldType::CiCharacter, 200),
        column("DATA_TYPE", FieldType::ShortInt, 2),
        column("TYPE_NAME", FieldType::CiCharacter, 20),
        column("COLUMN_SIZE", FieldType::Integer, 4),
        column("BUFFER_LENGTH", FieldType::Integer, 4),
        column("DECIMAL_DIGITS", FieldType::ShortInt, 2),
        column("NUM_PREC_RADIX", FieldType::Integer, 4),
        column("NULLABLE", FieldType::ShortInt, 2),
        column("REMARKS", FieldType::Memo, 9),
        column("COLUMN_DEF", FieldType::Memo, 9),
        column("SQL_DATA_TYPE", FieldType::Integer, 4),
        column("SQL_DATETIME_SUB", FieldType::Integer, 4),
        column("CHAR_OCTET_LENGTH", FieldType::Integer, 4),
        column("ORDINAL_POSITION", FieldType::Integer, 4),
        column("IS_NULLABLE", FieldType::Character, 4),
        column("NOCPTRANS", FieldType::ShortInt, 2),
        column("FIELD_OPTIONS", FieldType::Integer, 4),
        column("LOCAL_TYPE_NAME", FieldType::CiCharacter, 25),
    ];
    // A dictionary connection lists the columns of its tables and views and
    // its links' tables, in object order; a free connection, the columns of
    // the folder's tables.
    let filter = Filter {
        catalog,
        pattern: Some(table_pattern),
        kind: None,
    };
    if let Some(listed) = connect.with_database_opt(|db| dict_columns(db, &path, &filter)) {
        let mut rows = Vec::new();
        for (catalog, table, described) in listed {
            // A view is described now, the dictionary no longer held.
            let described = match described {
                Some(described) => described,
                None => crate::view_columns::view_columns(session, connection, &table)?,
            };
            column_rows(&mut rows, &catalog, &table, &described, column_pattern);
        }
        // The original engine cannot describe its system tables.
        let system = crate::system::SYSTEM_TABLE_CATALOG
            .iter()
            .any(|(name, ..)| filter.name(name));
        if filter.catalog("system") && system {
            return Err(unreadable_handle());
        }
        return Ok(Some(Results::new(columns, rows)));
    }
    let mut rows = Vec::new();
    for (table, name) in tables(session, connection, Some(table_pattern), false)? {
        let Ok(structure) = Structure::read(&table, Format::Adt) else {
            continue;
        };
        let described: Vec<Described> = structure
            .fields
            .into_iter()
            .map(|field| Described {
                nullable: field.field_type.nullable(),
                field,
                meta: None,
            })
            .collect();
        column_rows(&mut rows, &path, &name, &described, column_pattern);
    }
    Ok(Some(Results::new(columns, rows)))
}

/// What `sp_GetColumns` lists on a dictionary connection, in order: each
/// table's catalog, name and columns — `None` for a view, which is described
/// once the dictionary is no longer held (see [`dict_table_rows`] for the
/// order).
type Listed = Vec<(String, String, Option<Vec<Described>>)>;

fn dict_columns(db: &Database, path: &str, filter: &Filter) -> Listed {
    use crate::view_columns::table_columns;
    let mut listed = Vec::new();
    for object in db.dictionary().objects().iter().filter(|o| db.shows(o)) {
        match object.kind {
            // The columns are read from the table, which a user without the right to read it cannot open.
            Kind::Table
                if filter.own(path)
                    && filter.name(&object.name)
                    && !db.public_slot_bad(object)
                    && db.access_mask(&object.name) & crate::system::right::READ != 0 =>
            {
                let Some(columns) = table_columns(db, object) else {
                    continue;
                };
                listed.push((path.to_owned(), object.name.clone(), Some(columns)));
            }
            // A view whose rights slot does not check describes no columns,
            // as a table does, to the administrator too (`fixtures/sql/ddpublicbad7`,
            // `ddpublicbad8`).
            Kind::View
                if filter.own(path) && filter.name(&object.name) && !db.public_slot_bad(object) =>
            {
                listed.push((path.to_owned(), object.name.clone(), None));
            }
            Kind::Link if filter.catalog(&object.name) => {
                let Some(linked) = linked(db, object) else {
                    continue;
                };
                for table in linked.dictionary().all(Kind::Table) {
                    if !filter.name(&table.name) {
                        continue;
                    }
                    let Some(columns) = table_columns(&linked, table) else {
                        continue;
                    };
                    listed.push((object.name.clone(), table.name.clone(), Some(columns)));
                }
            }
            _ => {}
        }
    }
    listed
}

/// Adds the `sp_GetColumns` rows of the columns `described` of the table
/// `table` in `catalog` whose names match `column_pattern` to `rows`: the
/// column as ODBC describes it, with its dictionary comment (`REMARKS`),
/// default (`COLUMN_DEF`) and options, if it has them.
fn column_rows(
    rows: &mut Vec<Vec<Value>>,
    catalog: &str,
    table: &str,
    described: &[Described],
    column_pattern: Option<&str>,
) {
    for (i, column) in described.iter().enumerate() {
        let field = &column.field;
        if column_pattern.is_some_and(|p| !tflip_sql::eval::like(&field.name, p, None, true)) {
            continue;
        }
        let info = describe(field);
        let int = |n: i64| Value::Integer(i128::from(n));
        let optional = |n: Option<i64>| n.map_or(Value::Null, int);
        let meta = column.meta.clone().unwrap_or_default();
        let nullable = column.nullable;
        rows.push(vec![
            Value::Text(catalog.to_owned()),
            Value::Text(String::new()),
            Value::Text(table.to_owned()),
            Value::Text(field.name.clone()),
            int(info.data_type),
            Value::Text(info.type_name.to_owned()),
            int(info.size),
            int(info.buffer),
            int(info.decimals),
            optional(info.radix),
            int(i64::from(nullable)),
            meta.comment.map_or(Value::Null, Value::Text),
            Value::Text(meta.default.unwrap_or_else(|| "NULL".to_owned())),
            int(info.sql_data_type),
            optional(info.datetime_sub),
            optional(info.octets),
            int(i as i64 + 1),
            Value::Text(if nullable { "YES" } else { "NO" }.to_owned()),
            int(0),
            int(i64::from(meta.options)),
            Value::Text(info.local_name.to_owned()),
        ]);
    }
}

/// How `sp_GetColumns` describes a field: as ODBC does, and in Advantage's
/// own names.
struct Description {
    data_type: i64,
    type_name: &'static str,
    size: i64,
    buffer: i64,
    decimals: i64,
    radix: Option<i64>,
    sql_data_type: i64,
    datetime_sub: Option<i64>,
    octets: Option<i64>,
    local_name: &'static str,
}

fn describe(field: &Field) -> Description {
    let length = i64::from(field.length());
    let decimals = i64::from(field.decimals());
    let text = |data_type, type_name, size: i64, buffer: i64, local_name| Description {
        data_type,
        type_name,
        size,
        buffer,
        decimals: 0,
        radix: None,
        sql_data_type: data_type,
        datetime_sub: None,
        octets: Some(buffer),
        local_name,
    };
    let number = |data_type, type_name, size, buffer, decimals, local_name| Description {
        data_type,
        type_name,
        size,
        buffer,
        decimals,
        radix: Some(10),
        sql_data_type: data_type,
        datetime_sub: None,
        octets: None,
        local_name,
    };
    let datetime = |data_type, type_name, size, sub, local_name| Description {
        data_type,
        type_name,
        size,
        buffer: if sub == 3 { 16 } else { 6 },
        decimals: 0,
        radix: None,
        sql_data_type: 9,
        datetime_sub: Some(sub),
        octets: None,
        local_name,
    };
    const MEMO: i64 = 2_147_483_640;
    const BLOB: i64 = 2_147_483_647;
    match field.field_type {
        FieldType::Character => text(1, "CHAR", length, length, "CHAR"),
        FieldType::CiCharacter => text(1, "CICHAR", length, length, "CICHAR"),
        FieldType::VarChar => text(12, "VARCHAR", length, length, "VARCHARFOX"),
        FieldType::NChar => text(-8, "NCHAR", length, 2 * length, "NCHAR"),
        FieldType::NVarChar => text(-9, "NVARCHAR", length, 2 * length, "NVARCHAR"),
        FieldType::Memo | FieldType::LegacyVarChar => text(-1, "MEMO", MEMO, MEMO, "MEMO"),
        FieldType::NMemo => text(-10, "NMEMO", MEMO / 2, MEMO / 2, "NMEMO"),
        FieldType::Binary => text(-4, "BLOB", BLOB, BLOB, "BINARY"),
        FieldType::Image => text(-4, "IMAGE", BLOB, BLOB, "IMAGE"),
        FieldType::Raw => text(-2, "RAW", length, length, "RAW"),
        FieldType::VarBinary => text(-3, "VARBINARY", length, length, "VARBINARYFOX"),
        FieldType::Guid => Description {
            octets: Some(16),
            ..text(-11, "GUID", 36, 36, "GUID")
        },
        FieldType::Logical => Description {
            octets: None,
            ..text(-7, "LOGICAL", 1, 1, "LOGICAL")
        },
        FieldType::Numeric => {
            let precision = length - 1 - i64::from(decimals > 0);
            number(2, "NUMERIC", precision, length, decimals, "NUMERIC")
        }
        FieldType::ShortInt => number(5, "SHORT", 5, 2, 0, "SHORTINT"),
        FieldType::Integer => number(4, "INTEGER", 10, 4, 0, "INTEGER"),
        FieldType::AutoInc => number(4, "AUTOINC", 10, 4, 0, "AUTOINC"),
        FieldType::LongInt => number(-5, "LONGINT", 19, 20, 0, "LONGINTEGER"),
        FieldType::RowVersion => number(2, "ROWVERSION", 19, 21, 0, "ROWVERSION"),
        FieldType::Double => number(8, "DOUBLE", 15, 8, decimals, "DOUBLE"),
        FieldType::CurDouble => number(8, "CURDOUBLE", 15, 8, decimals, "CURDOUBLE"),
        FieldType::Money => number(2, "MONEY", 22, 24, 4, "MONEY"),
        FieldType::Date | FieldType::CompactDate => datetime(91, "DATE", 10, 1, "DATE"),
        FieldType::Time => datetime(92, "TIME", 8, 2, "TIME"),
        FieldType::TimeStamp => datetime(93, "TIMESTAMP", 23, 3, "TIMESTAMP"),
        FieldType::ModTime => datetime(93, "MODTIME", 23, 3, "MODTIME"),
        FieldType::Other(_) => text(-2, "RAW", length, length, "RAW"),
    }
}
