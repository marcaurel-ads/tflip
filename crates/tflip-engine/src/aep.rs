//! Running external procedures (an `.aep`: a Windows DLL or a Linux shared
//! object), as the Advantage 12 help describes its version 2 interface:
//! the library's `Startup` the first time a connection calls into it, then
//! the procedure's function with the connection's id, its handle and where
//! to put the rows it affected; `Shutdown` when the connection closes.
//!
//! The library calls back through the ACE API with the handle it was given,
//! on the thread that called it, inside the statement that runs the
//! procedure. It reads its arguments from `__input` (the pseudo-table a
//! script procedure has too) and writes its results to `__output` and an
//! error to `__error`, which [`collects`] and [`insert`] keep for the
//! procedure that runs on the thread.
//!
//! Loading and calling the library is unsafe code, which the engine has
//! none of: the host that embeds the engine registers [`Libraries`] for it.
//! A trigger whose container is a library calls a function of the version-1
//! kind the help gives for triggers: the connection's id and handle, the
//! trigger's and its table's names, the event and trigger types and the
//! record number ([`run_trigger`]); the library reads `__new` and `__old`
//! through the handle, and writes its error to `__error`. The library need
//! not be an AEP of version 2, and its `Startup` is not called.
//!
//! What the original's server does was probed (`fixtures/sql/aep.json`,
//! `tools/ace/aep/`): the error texts, that a non-zero return fails the
//! procedure (reporting 5154, whatever was returned), that only an
//! `__error` row with a number other than 0 does (the help says a message
//! alone does too), and that the rows affected are what the library says
//! (`fixtures/sql/aeptrig.json`, `tools/ace/aep/aeptrig.c`, for the triggers).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tflip_sql::ast::{Insert, InsertSource};
use tflip_sql::{Compiler, NoRow, SqlError, V, eval};
use tflip_storage::build::Column;
use tflip_storage::create::{self, FieldSpec};
use tflip_storage::field::FieldType;
use tflip_storage::memo::MemoKind;
use tflip_storage::value::Value;

use crate::handle::Handle;
use crate::table::{self, FieldRef, TableSettings, TableType};
use crate::{Error, Session, connection, statement};

/// What loads and calls the libraries of external procedures.
pub trait Libraries: Send + Sync {
    /// Calls `function` of the library at `path` for `connection` of
    /// `session`, with `rows` for the rows it affected, and returns what it
    /// returned; the library's `Startup` runs first the first time the
    /// connection calls into it.
    fn call(
        &self,
        session: &Session,
        path: &Path,
        function: &str,
        connection: Handle,
        rows: &mut u32,
    ) -> Result<u32, Failure>;

    /// Calls the trigger function `function` of the library at `path` for
    /// `connection` of `session`, and returns what it returned; no `Startup`
    /// runs, and the library need not be a version 2 one.
    fn trigger(
        &self,
        session: &Session,
        path: &Path,
        function: &str,
        connection: Handle,
        call: &TriggerCall<'_>,
    ) -> Result<u32, Failure>;

    /// The connection closes: the `Shutdown` of each library it called into.
    fn closing(&self, session: &Session, connection: Handle);
}

/// What a trigger function is told of the change that fires it.
#[derive(Debug, Clone, Copy)]
pub struct TriggerCall<'a> {
    pub name: &'a str,
    pub table: &'a str,
    /// `ADS_TRIGEVENT_*` and `ADS_TRIGTYPE_*`.
    pub event: u32,
    pub kind: u32,
    pub recno: u32,
}

/// Why a library's function could not be called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The library is not there, or cannot be loaded.
    NotLoaded,
    /// The library has no such function.
    NoFunction,
    /// Something tflip does not call, as this says.
    Refused(String),
}

static LIBRARIES: OnceLock<Box<dyn Libraries>> = OnceLock::new();

/// Registers what loads the libraries; the first registration stays.
pub fn set_libraries(libraries: Box<dyn Libraries>) {
    let _ = LIBRARIES.set(libraries);
}

/// The number and message of an `__error` row.
type ErrorRow = (Option<i64>, Option<String>);

/// What the procedure that runs on the thread has written.
struct Frame {
    connection: Handle,
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
    /// The `__input` table, if the procedure has input parameters.
    input: Option<Built>,
    /// The `__error` rows written through SQL.
    errors: Vec<ErrorRow>,
    /// How many `errors` there were when the table API opened `__error`:
    /// what it wrote there goes after those.
    errors_at: Option<usize>,
    /// The temporary tables the table API made of `__input` and `__error`,
    /// and the handles it opened on them.
    made: Vec<&'static str>,
    opened: Vec<Handle>,
}

/// A table in memory: its columns and rows.
pub(crate) type Built = (Vec<Column>, Vec<Vec<Value>>);

thread_local! {
    /// The external procedures running on this thread, the innermost last.
    static FRAMES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
}

/// Whether an `INSERT` into `table` is one of the running external
/// procedure's results.
pub(crate) fn collects(table: &str) -> bool {
    (table.eq_ignore_ascii_case("__output") || table.eq_ignore_ascii_case("__error"))
        && FRAMES.with(|frames| !frames.borrow().is_empty())
}

/// Runs the `INSERT` into `__output` or `__error` of the running external
/// procedure, and returns the rows it added.
pub(crate) fn insert(
    session: &Session,
    connection: Handle,
    settings: &statement::Settings,
    insert: &Insert,
) -> Result<u32, Error> {
    let error = insert.table.name.eq_ignore_ascii_case("__error");
    let columns = match error {
        true => vec!["errno".to_owned(), "message".to_owned()],
        false => FRAMES.with(|frames| {
            frames.borrow().last().map_or_else(Vec::new, |frame| {
                frame.columns.iter().map(|c| c.name.clone()).collect()
            })
        }),
    };
    // Where each value goes: the columns named, or all of them in order.
    let slots = match &insert.columns {
        Some(named) => named
            .iter()
            .map(|c| {
                columns
                    .iter()
                    .position(|name| name.eq_ignore_ascii_case(&c.name))
                    .ok_or_else(|| Error::Sql(SqlError::column_not_found(&c.name, c.pos)))
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => (0..columns.len()).collect(),
    };
    let rows: Vec<Vec<V>> = match &insert.source {
        InsertSource::Values(rows) => {
            let env = statement::connection_env(session, connection)?;
            rows.iter()
                .map(|row| {
                    row.iter()
                        .map(|expr| match expr.kind {
                            // A NULL is of the column's type.
                            tflip_sql::ast::ExprKind::Null => Ok(V::Null),
                            _ => {
                                let node = Compiler::new(None, 0).compile(expr)?;
                                eval(&node, &mut NoRow, &env)
                            }
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .collect::<Result<_, _>>()
                .map_err(Error::Sql)?
        }
        InsertSource::Select(select) => {
            let (columns, rows) = statement::select_rows(session, connection, settings, select)?;
            rows.into_iter()
                .map(|row| {
                    row.into_iter()
                        .zip(&columns)
                        .map(|(value, column)| V::from_field(value, column_ty(column)))
                        .collect()
                })
                .collect()
        }
    };
    let added = rows.len() as u32;
    FRAMES.with(|frames| {
        let mut frames = frames.borrow_mut();
        let Some(frame) = frames.last_mut() else {
            return Ok(());
        };
        for row in rows {
            let mut values = vec![V::Null; columns.len()];
            for (slot, value) in slots.iter().zip(row) {
                values[*slot] = value;
            }
            if error {
                let number = match &values[0] {
                    V::Null => None,
                    v => v.as_f64().map(|n| n as i64),
                };
                let message = match &values[1] {
                    V::Str(text) => Some(text.clone()),
                    _ => None,
                };
                frame.errors.push((number, message));
                continue;
            }
            let row = values
                .into_iter()
                .zip(&frame.columns)
                .map(|(value, column)| value.to_field(column_ty(column), &column.name))
                .collect::<Result<Vec<_>, _>>()
                .map_err(Error::Sql)?;
            frame.rows.push(row);
        }
        Ok(())
    })?;
    Ok(added)
}

/// Which of the running procedure's tables a client opens as `name` with
/// `settings`: as the original's servers take `#__input` and `#__error`, and
/// the same without the `#` with `ADS_TEMP_TABLE` (not with both); a plain
/// name is a file that is not there (7041), and `__output` is none of
/// them by any name (7112), before the procedure writes to it or after
/// (`fixtures/sql/aeptablesremote.json`).
fn table_key(name: &str, settings: &TableSettings) -> Option<&'static str> {
    let temp = settings.options & crate::table::options::TEMP_TABLE != 0;
    let bare = match (name.strip_prefix('#'), temp) {
        (Some(bare), false) => bare,
        (None, true) => name,
        _ => return None,
    };
    ["__input", "__error"]
        .into_iter()
        .find(|key| bare.eq_ignore_ascii_case(key))
}

/// Whether `name` is the temporary table [`table_name`] makes of a
/// procedure's table, which the original gives no alias.
pub(crate) fn is_procedure_table(name: &str) -> bool {
    ["#__input", "#__error"]
        .into_iter()
        .any(|key| name.eq_ignore_ascii_case(key))
}

/// A procedure's table opened by its plain name, which is a file the
/// original looks for beside the dictionary and does not find (7041,
/// `fixtures/sql/aeptablesremote.json`).
fn not_a_file(
    connection: Handle,
    name: &str,
    settings: &TableSettings,
) -> Result<Option<String>, Error> {
    let temp = settings.options & crate::table::options::TEMP_TABLE != 0;
    let named = ["__input", "__output", "__error"]
        .into_iter()
        .any(|key| name.eq_ignore_ascii_case(key));
    let running = FRAMES.with(|frames| {
        frames
            .borrow()
            .last()
            .is_some_and(|frame| frame.connection == connection)
    });
    match named && !temp && running {
        true => Err(Error::Constraint {
            code: 7041,
            text: format!(
                "File not found.  Verify the specified path and file name is correct.  \\\\{name}.adt"
            ),
        }),
        false => Ok(None),
    }
}

/// A client opens the table `name` on `connection`: if it is the `__input`
/// or `__error` of an external procedure running there, makes it a
/// temporary table of the connection (an ADT table whatever the type asked
/// for, which the original does not look at) and returns the name to open,
/// with `settings` to open it by.
pub(crate) fn table_name(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: &mut TableSettings,
) -> Result<Option<String>, Error> {
    let Some(key) = table_key(name, settings) else {
        return not_a_file(connection, name, settings);
    };
    let wanted = FRAMES.with(|frames| {
        let frames = frames.borrow();
        let frame = frames
            .last()
            .filter(|frame| frame.connection == connection)?;
        match key {
            "__input" => frame.input.clone(),
            _ => Some((error_columns(), Vec::new())),
        }
    });
    let Some((columns, rows)) = wanted else {
        return Ok(None);
    };
    let temp = format!("#{key}");
    let owner = connection::get(session, connection)?;
    if owner.temps.find(&temp).is_none() {
        let path = owner
            .temps
            .create(&temp, TableType::Adt)
            .ok_or(Error::FileNotFound)?;
        let specs: Vec<FieldSpec> = columns
            .iter()
            .map(|c| FieldSpec {
                name: c.name.clone(),
                field_type: c.field_type,
                length: c.length,
                decimals: c.decimals,
                compressed: false,
            })
            .collect();
        let made = create::adt_table(&path, &specs, crate::ddl::BLOCK_SIZE)
            .map_err(|_| Error::CorruptTable)
            .and_then(|()| fill(session, connection, &temp, &columns, rows));
        if let Err(error) = made {
            remove_temp(session, connection, &temp);
            return Err(error);
        }
        FRAMES.with(|frames| {
            if let Some(frame) = frames.borrow_mut().last_mut() {
                frame.made.push(key);
                if key == "__error" {
                    frame.errors_at = Some(frame.errors.len());
                }
            }
        });
    }
    settings.options &= !crate::table::options::TEMP_TABLE;
    settings.table_type = TableType::Adt;
    Ok(Some(temp))
}

/// The client opened `handle` on a table [`table_name`] made.
pub(crate) fn opened(connection: Handle, handle: Handle) {
    FRAMES.with(|frames| {
        if let Some(frame) = frames
            .borrow_mut()
            .last_mut()
            .filter(|frame| frame.connection == connection)
        {
            frame.opened.push(handle);
        }
    });
}

/// `__error`: the number and the message of the error a library makes.
fn error_columns() -> Vec<Column> {
    let column = |name: &str, field_type, length| Column {
        name: name.to_owned(),
        field_type,
        length,
        decimals: 0,
    };
    vec![
        column("errno", FieldType::Integer, 4),
        column("message", FieldType::Memo, 9),
    ]
}

/// Appends `rows` to the temporary table `name`.
fn fill(
    session: &Session,
    connection: Handle,
    name: &str,
    columns: &[Column],
    rows: Vec<Vec<Value>>,
) -> Result<(), Error> {
    let handle = table::open(session, connection, name, TableSettings::default())?;
    let written = table::get(session, handle).and_then(|target| {
        for row in rows {
            target.append()?;
            for (i, (value, column)) in row.into_iter().zip(columns).enumerate() {
                let value = crate::dml::set_value_in(column.field_type, value);
                target.set_field(FieldRef::Number(i + 1), &value)?;
            }
            target.write_record()?;
        }
        Ok(())
    });
    written.and(table::close(session, handle))
}

/// Forgets the temporary table `name` and deletes its files.
fn remove_temp(session: &Session, connection: Handle, name: &str) {
    let Ok(owner) = connection::get(session, connection) else {
        return;
    };
    if let Some(path) = owner.temps.remove(name) {
        let _ = std::fs::remove_file(&path);
        for kind in [MemoKind::Adm, MemoKind::Dbt, MemoKind::Fpt] {
            let _ = std::fs::remove_file(tflip_storage::dbf::memo_path(&path, kind));
        }
    }
}

/// What the table API wrote to `__error`, and where in the rows SQL wrote
/// it goes.
#[derive(Default)]
struct Written {
    errors_at: Option<usize>,
    errors: Vec<ErrorRow>,
}

/// The procedure's call is over: reads the `__error` rows the table API
/// wrote (those it posted, not a record appended and left),
/// closes the handles the library left open and drops the tables.
fn tables_end(session: &Session, connection: Handle) -> Written {
    let (errors_at, made, opened) = FRAMES.with(|frames| {
        frames
            .borrow_mut()
            .last_mut()
            .map_or_else(Default::default, |frame| {
                (
                    frame.errors_at,
                    std::mem::take(&mut frame.made),
                    std::mem::take(&mut frame.opened),
                )
            })
    });
    // A record appended and not written is none (the original raises no
    // error for an `__error` record left so).
    for handle in &opened {
        if let Ok(left) = table::get(session, *handle) {
            let _ = left.cancel_update();
        }
    }
    let mut written = Written {
        errors_at,
        ..Written::default()
    };
    if made.contains(&"__error") {
        let rows = table_rows(session, connection, "#__error").unwrap_or_default();
        written.errors = rows
            .into_iter()
            .map(|row| {
                let number = row.first().and_then(V::as_f64).map(|n| n as i64);
                let message = match row.get(1) {
                    Some(V::Str(text)) => Some(text.clone()),
                    _ => None,
                };
                (number, message)
            })
            .collect();
    }
    for handle in opened {
        let _ = table::close(session, handle);
    }
    for key in made {
        remove_temp(session, connection, &format!("#{key}"));
    }
    written
}

/// The rows of the temporary table `name`, as a fresh handle sees them.
fn table_rows(session: &Session, connection: Handle, name: &str) -> Result<Vec<Vec<V>>, Error> {
    let handle = table::open(session, connection, name, TableSettings::default())?;
    let read = table::get(session, handle).and_then(|source| {
        let mut rows = Vec::new();
        for record in 1..=source.record_count()? {
            source.goto_record(record)?;
            if source.is_deleted()? {
                continue;
            }
            if let Some((_, values)) = source.stored_row()? {
                rows.push(values);
            }
        }
        Ok(rows)
    });
    let _ = table::close(session, handle);
    read
}

fn column_ty(column: &Column) -> tflip_sql::Ty {
    tflip_sql::Ty::of_field(&statement::field_of(column))
}

/// Pops the frame [`run`] pushed, however the call ends.
struct Pushed;

impl Drop for Pushed {
    fn drop(&mut self) {
        FRAMES.with(|frames| frames.borrow_mut().pop());
    }
}

/// What an external procedure returned: the rows it wrote to `__output`
/// and the rows it says it affected.
pub(crate) struct Ran {
    pub(crate) rows: Vec<Vec<Value>>,
    pub(crate) affected: u32,
}

/// Runs the external procedure `name`, the function `function` of the
/// library `library` (a path relative to the connection's dictionary), whose
/// results have `columns`; its `__input` is registered by the caller.
pub(crate) fn run(
    session: &Session,
    connection: Handle,
    name: &str,
    library: &str,
    function: &str,
    columns: Vec<Column>,
    input: Option<Built>,
) -> Result<Ran, Error> {
    let fail = |text: &str| {
        Error::Sql(SqlError::ace(
            5154,
            &format!(
                "Execution of the stored procedure failed.  {text} Error executing stored \
                 procedure: {name}"
            ),
            "",
        ))
    };
    let path = library_path(&connection::get(session, connection)?.folder(), library);
    let shown = path.display();
    // As the original words them, with its Windows error codes.
    let failure = |failure| match failure {
        Failure::NotLoaded => fail(&format!(
            "The stored procedure named \"{name}\" requires the AEP named \"{shown}\".  The AEP \
             failed to load with an error code 126. Module not found.\r\n"
        )),
        Failure::NoFunction => fail(&format!(
            "The stored procedure named \"{name}\" called a function named \"{function}\".  The \
             function could not be found within the AEP named \"{shown}\".  The error code is 127 \
             Procedure not found.\r\n"
        )),
        Failure::Refused(text) => fail(&text),
    };
    let libraries = LIBRARIES.get().ok_or_else(|| {
        failure(Failure::Refused(
            "External procedures cannot be loaded here.".to_owned(),
        ))
    })?;
    FRAMES.with(|frames| {
        frames.borrow_mut().push(Frame {
            connection,
            columns,
            rows: Vec::new(),
            input,
            errors: Vec::new(),
            errors_at: None,
            made: Vec::new(),
            opened: Vec::new(),
        })
    });
    let _pushed = Pushed;
    let mut affected = 0;
    let called = libraries.call(session, &path, function, connection, &mut affected);
    let written = tables_end(session, connection);
    let returned = called.map_err(failure)?;
    let (rows, mut errors) = FRAMES.with(|frames| {
        frames
            .borrow_mut()
            .last_mut()
            .map(|frame| {
                (
                    std::mem::take(&mut frame.rows),
                    std::mem::take(&mut frame.errors),
                )
            })
            .unwrap_or_default()
    });
    let at = written.errors_at.unwrap_or(errors.len()).min(errors.len());
    errors.splice(at..at, written.errors);
    // The first row is the error, and one without a number, or numbered 0,
    // is none (`fixtures/sql/aeptables.json`).
    let error = errors.into_iter().next();
    if let Some((Some(number), message)) = error.filter(|(n, _)| n.is_some_and(|n| n != 0)) {
        let message = message.unwrap_or_default();
        return Err(fail(&format!("ProcError {number}:   {message}")));
    }
    if returned != 0 {
        return Err(fail(&format!(
            "The stored procedure named \"{name}\" called a function named \"{function}\" in the \
             AEP named \"{shown}\".  The function returned with the error code: 5154"
        )));
    }
    Ok(Ran { rows, affected })
}

/// What a trigger library wrote to `__error`: the number and the message.
pub(crate) type TriggerError = (Option<i64>, Option<String>);

/// Runs the trigger function `function` of the library `library` (a path
/// relative to the connection's dictionary) as `call` says, and returns the
/// row it wrote to `__error`. A library that cannot be called is the text of
/// the error the trigger fails with.
pub(crate) fn run_trigger(
    session: &Session,
    connection: Handle,
    library: &str,
    function: &str,
    call: &TriggerCall<'_>,
) -> Result<Option<TriggerError>, String> {
    let path = library_path(
        &connection::get(session, connection)
            .map_err(|_| "The connection is gone.".to_owned())?
            .folder(),
        library,
    );
    let shown = path.display();
    let libraries = LIBRARIES
        .get()
        .ok_or("External triggers cannot be loaded here.")?;
    FRAMES.with(|frames| {
        frames.borrow_mut().push(Frame {
            connection,
            columns: Vec::new(),
            rows: Vec::new(),
            input: None,
            errors: Vec::new(),
            errors_at: None,
            made: Vec::new(),
            opened: Vec::new(),
        })
    });
    let _pushed = Pushed;
    // The return value is reserved, as the help says; what the original
    // does with it was probed: nothing.
    let called = libraries
        .trigger(session, &path, function, connection, call)
        .map_err(|failure| match failure {
            Failure::NotLoaded => {
                format!("The container named \"{shown}\" failed to load with an error code 126.")
            }
            Failure::NoFunction => format!(
                "The function named \"{function}\" could not be found within the container \
                 named \"{shown}\".  The error code is 127"
            ),
            Failure::Refused(text) => text,
        });
    let written = tables_end(session, connection);
    called?;
    let mut errors = FRAMES.with(|frames| {
        frames
            .borrow_mut()
            .last_mut()
            .map(|frame| std::mem::take(&mut frame.errors))
            .unwrap_or_default()
    });
    let at = written.errors_at.unwrap_or(errors.len()).min(errors.len());
    errors.splice(at..at, written.errors);
    // The first row is the error, as a procedure's is.
    Ok(errors.into_iter().next())
}

/// The connection closes: the libraries it called into shut down for it.
pub(crate) fn connection_closing(session: &Session, connection: Handle) {
    if let Some(libraries) = LIBRARIES.get() {
        libraries.closing(session, connection);
    }
}

/// Where the library a procedure names is: relative to the dictionary's
/// folder, with the backslashes of a Windows path as this system's
/// separators, and no `.` in it (the folder of a dictionary opened as
/// `folder\.\name.add` has one).
fn library_path(folder: &Path, library: &str) -> PathBuf {
    let relative: PathBuf = library
        .split(['\\', '/'])
        .filter(|part| !part.is_empty())
        .collect();
    folder
        .join(relative)
        .components()
        .filter(|part| *part != std::path::Component::CurDir)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_library_is_found_next_to_its_dictionary() {
        let folder = Path::new("data");
        assert_eq!(
            library_path(folder, r"procs\sp.aep"),
            folder.join("procs").join("sp.aep")
        );
        assert_eq!(library_path(folder, "sp.aep"), folder.join("sp.aep"));
    }
}
