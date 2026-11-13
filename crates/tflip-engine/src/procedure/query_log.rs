//! The query logging procedures (`sp_EnableQueryLogging`,
//! `sp_DisableQueryLogging`, `sp_GetQueryLoggingResults`,
//! `sp_ViewQueryLogging`): what they refuse and give is as the original's
//! (`fixtures/sql/spql.json`, `spddql.json`, ...); the logging itself is in
//! [`crate::query_log`].

use tflip_sql::V;
use tflip_sql::ast::Execute;
use tflip_storage::build::Column;
use tflip_storage::field::FieldType;
use tflip_storage::value::Value;

use super::Results;
use super::args::{Kind, Spec, check_arguments, column};
use crate::handle::Handle;
use crate::query_log::{self, Settings};
use crate::{Error, Session, ace_error, statement};

const SPECS: [Spec; 4] = [
    Spec {
        name: "SP_ENABLEQUERYLOGGING",
        params: &[
            ("TableName", Kind::Text),
            ("TruncateExistingData", Kind::Bool),
            ("LogOnlyUnoptimizedQueries", Kind::Bool),
            ("MinimumTimeBeforeLogging", Kind::Integer),
            ("EncryptionPassword", Kind::Text),
            ("Options", Kind::Integer),
        ],
        required: 5,
        results: false,
    },
    Spec {
        name: "SP_DISABLEQUERYLOGGING",
        params: &[],
        required: 0,
        results: false,
    },
    Spec {
        name: "SP_GETQUERYLOGGINGRESULTS",
        params: &[("EncryptionPassword", Kind::Text)],
        required: 1,
        results: true,
    },
    Spec {
        name: "SP_VIEWQUERYLOGGING",
        params: &[],
        required: 0,
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

/// Whether the procedure `name` has a result set (true for any that is not
/// one of these).
pub(crate) fn has_results(name: &str) -> bool {
    find(name).is_none_or(|s| s.results)
}

/// The parameters of the procedure `name`, as fields.
pub(crate) fn inputs(name: &str) -> Vec<tflip_storage::field::Field> {
    find(name).map_or_else(Vec::new, super::args::inputs)
}

fn error(detail: &str) -> Error {
    ace_error::sql(5226, detail)
}

/// Runs the procedure `execute` calls with `args`.
pub(crate) fn run(
    session: &Session,
    statement: Handle,
    connection: Handle,
    execute: &Execute,
    args: &[V],
) -> Result<Option<Results>, Error> {
    let spec = find(&execute.name).ok_or(Error::NotImplemented)?;
    let args = check_arguments(spec, execute, args)?;
    if spec.name == "SP_VIEWQUERYLOGGING" {
        let columns = vec![
            column("Database", FieldType::Character, 255),
            column("Table", FieldType::Character, 255),
        ];
        let rows = match query_log::logging(session, connection) {
            Some((database, table)) => {
                let dictionary = crate::connection::get(session, connection)?.dictionary;
                match dictionary {
                    true => vec![vec![Value::Text(database), Value::Text(table)]],
                    false => {
                        let table = std::path::Path::new(&database).join(table);
                        vec![vec![
                            Value::Text("Free Table Logging".to_owned()),
                            Value::Text(table.display().to_string()),
                        ]]
                    }
                }
            }
            None => Vec::new(),
        };
        return Ok(Some(Results::new(columns, rows)));
    }
    // Only a dictionary's administrator may turn logging on and off and read
    // it, after the arguments are looked at.
    if super::is_ordinary_user(session, connection) {
        return Err(ace_error::sql_bare(5161));
    }
    match spec.name {
        "SP_ENABLEQUERYLOGGING" => {
            let options = args.integer(5).unwrap_or(0);
            if options & 3 == 3 {
                return Err(ace_error::sql(
                    5019,
                    "Options cannot be error only and no error.",
                ));
            }
            if options & !3 != 0 {
                return Err(ace_error::sql(5019, "Unrecognized query logging option."));
            }
            if query_log::logging(session, connection).is_some() {
                return Err(error("Query Logging is already enabled on this database."));
            }
            let settings = Settings {
                table: args.text(0).unwrap_or_default(),
                only_unoptimized: args.integer(2).unwrap_or(0) != 0,
                // A negative number is read as an unsigned one: a very long time.
                minimum: i64::from(args.integer(3).unwrap_or(0) as i32 as u32),
                options,
                password: args.text(4).unwrap_or_default(),
            };
            let locked = query_log::prepare(
                session,
                connection,
                &settings.table,
                args.integer(1).unwrap_or(0) != 0,
                &settings.password,
            )?;
            query_log::enable(session, connection, settings, locked)?;
            if let Some(sql) = statement::sql_of(session, statement)
                && !locked
            {
                query_log::began(session, connection, &sql);
            }
            Ok(None)
        }
        "SP_DISABLEQUERYLOGGING" => {
            query_log::disable(session, connection);
            Ok(None)
        }
        _ => {
            let Some((_, table)) = query_log::logging(session, connection) else {
                return Err(error("Query Logging is not enabled on this database."));
            };
            let columns: Vec<Column> = query_log::SCHEMA
                .iter()
                .map(|&(name, field_type, length, decimals)| Column {
                    name: name.to_owned(),
                    field_type,
                    length,
                    decimals,
                })
                .collect();
            let rows = query_log::contents(
                session,
                connection,
                &table,
                &args.text(0).unwrap_or_default(),
            )?;
            Ok(Some(Results::new(columns, rows)))
        }
    }
}
