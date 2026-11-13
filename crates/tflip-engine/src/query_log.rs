//! Query logging (`sp_EnableQueryLogging` and its three companions), as the
//! original's local server does it (`fixtures/sql/spql*.json`,
//! `spddql*.json`): every statement a connection runs is a row of the log
//! table, put in when it starts and filled in when it ends.
//!
//! The log is a table of the connection's own folder, or of its data
//! dictionary; what it records is written with SQL on a statement of the
//! connection's own, which is not logged itself.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tflip_sql::Row as _;
use tflip_storage::field::{Field, FieldType};
use tflip_storage::value::Value;

use crate::handle::Handle;
use crate::table::{TableSettings, TableType};
use crate::{Error, Session, ace_error, connection, lock, statement, table};

/// What `sp_EnableQueryLogging` was given.
#[derive(Clone, Debug)]
pub(crate) struct Settings {
    /// The table the log is kept in, as it was named.
    pub table: String,
    pub only_unoptimized: bool,
    /// The seconds a statement must run before it is logged (0 or less for
    /// all).
    pub minimum: i64,
    /// `QL_OPTION_ERROR_ONLY` (1) and `QL_OPTION_NO_ERROR` (2).
    pub options: i64,
    /// The encryption password of a free log table (a dictionary's is never
    /// encrypted, whatever it is given: `fixtures/aes/api/13_querylog_dict`).
    pub password: String,
}

impl Settings {
    fn error_only(&self) -> bool {
        self.options & 1 != 0
    }

    fn no_error(&self) -> bool {
        self.options & 2 != 0
    }

    /// Whether a statement's row goes in when it starts: the filters that
    /// need the outcome put it in when it ends.
    fn immediate(&self) -> bool {
        self.minimum <= 0 && !self.only_unoptimized && !self.error_only()
    }

    /// [`Settings::immediate`] for a statement of a script, which is
    /// unoptimized: the filter for those lets it through.
    fn immediate_inner(&self, optimized: bool) -> bool {
        self.minimum <= 0 && !(self.only_unoptimized && optimized) && !self.error_only()
    }
}

#[derive(Clone)]
struct Active {
    settings: Settings,
    /// Whether the log table is encrypted and the log was turned on without
    /// its password: every statement fails with the append it cannot make.
    locked: bool,
    /// Which enabling it is: a statement that ends after the log was
    /// disabled, or enabled again, finds its row is not its own to fill in.
    generation: u64,
}

/// The logs that are on, by the path of the connections that log: the
/// original's local server keeps it for its process.
static ACTIVE: Mutex<Vec<(PathBuf, Active)>> = Mutex::new(Vec::new());
static GENERATION: AtomicU64 = AtomicU64::new(1);

/// The columns of the log table, as `sp_GetQueryLoggingResults` lists them
/// and a table is made with them.
pub(crate) const COLUMNS: &str = "ID AUTOINC, \"Start Time\" TIMESTAMP, Optimized LOGICAL, \
     \"Return Code\" INTEGER, \"Rows Affected\" INTEGER, \"End Time\" TIMESTAMP, \
     \"Run Time\" DOUBLE(2), Database CHAR(255), \"User Name\" CHAR(100), \
     \"Connection Name\" CHAR(100), \"Application ID\" MEMO, Query MEMO, Description MEMO";

/// The log of `connection`, if it is on.
fn active(session: &Session, connection: Handle) -> Option<(PathBuf, Active)> {
    let path = connection::get(session, connection).ok()?.path.clone();
    lock(&ACTIVE)
        .iter()
        .find(|(key, _)| *key == path)
        .map(|(key, active)| (key.clone(), active.clone()))
}

/// The table `connection` logs in, with the data folder it is in.
pub(crate) fn logging(session: &Session, connection: Handle) -> Option<(String, String)> {
    let (_, active) = active(session, connection)?;
    let owner = connection::get(session, connection).ok()?;
    Some((owner.path.display().to_string(), active.settings.table))
}

/// Turns the log of `connection` on.
pub(crate) fn enable(
    session: &Session,
    connection: Handle,
    settings: Settings,
    locked: bool,
) -> Result<(), Error> {
    let key = connection::get(session, connection)?.path.clone();
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
    let mut on = lock(&ACTIVE);
    on.retain(|(path, _)| *path != key);
    on.push((
        key,
        Active {
            settings,
            locked,
            generation,
        },
    ));
    Ok(())
}

/// Turns the log of `connection` off (there is no error for one that is not
/// on).
pub(crate) fn disable(session: &Session, connection: Handle) {
    if let Ok(owner) = connection::get(session, connection) {
        lock(&ACTIVE).retain(|(path, _)| *path != owner.path);
    }
}

thread_local! {
    /// How many statements are running on this thread, which are the
    /// outermost one and what it runs inside itself.
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// A statement that is running: only the outermost is logged.
pub(crate) struct Level(bool);

impl Level {
    pub fn enter() -> Level {
        let top = DEPTH.get() == 0;
        DEPTH.set(DEPTH.get() + 1);
        Level(top)
    }

    pub fn is_outermost(&self) -> bool {
        self.0
    }
}

impl Drop for Level {
    fn drop(&mut self) {
        DEPTH.set(DEPTH.get() - 1);
    }
}

/// What a statement leaves of itself in the log while it runs.
pub(crate) struct Entry {
    key: PathBuf,
    active: Active,
    /// Whether it is optimized.
    optimized: bool,
    /// Whether it has a cursor, which an error in it still opens.
    cursor: bool,
    id: Option<i64>,
}

/// The row of a statement whose cursor is still open: its end time goes in
/// when the cursor closes.
#[derive(Clone, Debug)]
pub(crate) struct End {
    key: PathBuf,
    generation: u64,
    table: String,
    id: i64,
}

/// A statement of a script that is running: its row is put in the log
/// when a statement inside it is, or when it ends, as a statement that fails
/// before it runs (a table it names is not there) has a row of its own at its
/// end, with the error.
struct Inner {
    key: PathBuf,
    active: Active,
    sql: String,
    optimized: bool,
    started_at: (i32, u32),
    started: std::time::Instant,
    id: Option<i64>,
}

thread_local! {
    /// The statements of scripts that are running, innermost last.
    static INNER: RefCell<Vec<Inner>> = const { RefCell::new(Vec::new()) };
}

/// Puts the rows of the running statements that are not in the log yet in,
/// as a statement inside one of them (a trigger's) begins.
fn flush(session: &Session, connection: Handle) {
    let count = INNER.with_borrow(|stack| stack.len());
    for at in 0..count {
        let job = INNER.with_borrow(|stack| {
            let inner = &stack[at];
            (inner.id.is_none() && inner.active.settings.immediate_inner(inner.optimized)).then(
                || {
                    (
                        inner.active.settings.table.clone(),
                        Record {
                            started: inner.started_at,
                            optimized: Some(inner.optimized),
                            ..Record::new(session, connection, &inner.sql)
                        },
                    )
                },
            )
        });
        if let Some((table, row)) = job {
            let id = insert(session, connection, &table, &row);
            INNER.with_borrow_mut(|stack| stack[at].id = id);
        }
    }
}

/// A statement of a script begins, `sql` as it was written: whether its log
/// is on, and so whether [`inner_end`] is to be called. The statements of a
/// script are all unoptimized, but for a procedure's only one.
pub(crate) fn inner_begin(
    session: &Session,
    connection: Handle,
    sql: &str,
    optimized: bool,
) -> bool {
    let Some((key, active)) = active(session, connection) else {
        return false;
    };
    flush(session, connection);
    INNER.with_borrow_mut(|stack| {
        stack.push(Inner {
            key,
            active,
            sql: sql.to_owned(),
            optimized,
            started_at: crate::clock::now().local,
            started: std::time::Instant::now(),
            id: None,
        })
    });
    true
}

/// The statement of a script that began last has ended with `result`:
/// `affected` is the rows the script has changed so far (`u32::MAX` for
/// none), which a failure only has if it is of a procedure the script called.
/// Returns its row, which the end of the script may put an end time in.
pub(crate) fn inner_end(
    session: &Session,
    connection: Handle,
    result: Result<(), &Error>,
    affected: u32,
    calls: bool,
) -> Option<End> {
    let inner = INNER.with_borrow_mut(|stack| stack.pop())?;
    let (_, now) = active(session, connection)?;
    if now.generation != inner.active.generation {
        return None;
    }
    let settings = &inner.active.settings;
    let table = &settings.table;
    let elapsed = inner.started.elapsed().as_secs_f64() * 1000.0;
    let rows = match affected {
        u32::MAX => None,
        n => Some(n as i32),
    };
    let record = Record {
        started: inner.started_at,
        optimized: Some(inner.optimized),
        ..Record::new(session, connection, &inner.sql)
    };
    let id = match (result, inner.id) {
        (Ok(()), Some(id)) => {
            let rows = rows.unwrap_or(-1);
            update(
                session,
                connection,
                table,
                id,
                &format!(
                    "\"Return Code\" = 0, \"Rows Affected\" = {rows}, \"Run Time\" = {elapsed:.2}"
                ),
            );
            Some(id)
        }
        (Ok(()), None) => {
            if settings.error_only()
                || (settings.only_unoptimized && inner.optimized)
                || (!settings.only_unoptimized
                    && (elapsed as i64) < settings.minimum.saturating_mul(1000))
            {
                return None;
            }
            insert(
                session,
                connection,
                table,
                &Record {
                    code: Some(0),
                    rows: Some(rows.unwrap_or(-1)),
                    run_time: Some(elapsed),
                    ..record
                },
            )
        }
        (Err(error), id) => {
            let error = ace_error::sql_error(error.clone());
            let Error::Sql(sql_error) = &error else {
                return None;
            };
            let (code, description) = (sql_error.native, description(&error));
            match id {
                Some(id) => {
                    let rows = rows
                        .map_or_else(String::new, |rows| format!("\"Rows Affected\" = {rows}, "));
                    // A procedure the script called ran, as a statement does
                    // when it fails with its data.
                    let time = match calls {
                        true => format!("\"Run Time\" = {elapsed:.2}, "),
                        false => String::new(),
                    };
                    update(
                        session,
                        connection,
                        table,
                        id,
                        &format!(
                            "\"Return Code\" = {code}, {rows}{time}Description = {}",
                            quote(&description)
                        ),
                    );
                }
                None => {
                    if settings.no_error()
                        || settings.minimum > 0
                        || (settings.only_unoptimized && inner.optimized)
                    {
                        return None;
                    }
                    insert(
                        session,
                        connection,
                        table,
                        &Record {
                            code: Some(code),
                            rows,
                            run_time: calls.then_some(elapsed),
                            description: Some(description),
                            ..record
                        },
                    );
                }
            }
            return None;
        }
    };
    id.map(|id| End {
        key: inner.key,
        generation: inner.active.generation,
        table: table.clone(),
        id,
    })
}

/// Puts a row in for what the script does that is no statement of its own to
/// run (it opens a cursor), `text` as the original words it.
pub(crate) fn inner_note(session: &Session, connection: Handle, text: &str, optimized: bool) {
    let Some((_, active)) = active(session, connection) else {
        return;
    };
    let settings = &active.settings;
    if settings.error_only() || settings.minimum > 0 || (settings.only_unoptimized && optimized) {
        return;
    }
    flush(session, connection);
    let row = Record {
        optimized: Some(optimized),
        ..Record::new(session, connection, text)
    };
    insert(session, connection, &settings.table, &row);
}

/// What the statement `sql` runs as, for the log: the row it starts with, if
/// its log is on and its settings put it in at once.
pub(crate) fn begin(
    session: &Session,
    connection: Handle,
    sql: &str,
    cursor: bool,
    optimized: impl FnOnce() -> Option<bool>,
) -> Result<Option<Entry>, Error> {
    // Only the outermost statement is logged: its writes to the log are not.
    if DEPTH.get() != 1 {
        return Ok(None);
    }
    let Some((key, active)) = active(session, connection) else {
        return Ok(None);
    };
    // The append of an encrypted log whose password the handle lacks fails,
    // and the statement with it, but for the one that turns logging off
    // (`fixtures/aes/api/11_querylog`).
    if active.locked && !sql.to_ascii_lowercase().contains("sp_disablequerylogging") {
        return Err(Error::EncryptionNotEnabled
            .detailed("Cannot append to an encrypted table unless encryption is enabled."));
    }
    let Some(optimized) = optimized() else {
        return Ok(None);
    };
    let mut entry = Entry {
        key,
        active,
        optimized,
        cursor,
        id: None,
    };
    if entry.active.settings.immediate() {
        let row = Record {
            optimized: Some(optimized),
            ..Record::new(session, connection, sql)
        };
        entry.id = insert(session, connection, &entry.active.settings.table, &row);
    }
    Ok(Some(entry))
}

/// Puts the row of the statement `sql` that turned the log on in: it is the
/// first of the log it began.
pub(crate) fn began(session: &Session, connection: Handle, sql: &str) {
    let Some((_, active)) = active(session, connection) else {
        return;
    };
    let settings = &active.settings;
    if settings.error_only()
        || settings.only_unoptimized
        || !(0..=90_000).contains(&settings.minimum)
    {
        return;
    }
    let row = Record {
        optimized: Some(true),
        ..Record::new(session, connection, sql)
    };
    insert(session, connection, &settings.table, &row);
}

/// How a statement ended.
pub(crate) struct Outcome<'a> {
    pub result: Result<bool, &'a Error>,
    /// The rows it affected, `u32::MAX` for none.
    pub affected: u32,
    pub elapsed_ms: f64,
}

/// Fills in the row of the statement `sql` now it has ended, or puts it in
/// if it was left until the outcome was known; returns its row if its end
/// time is to go in when its cursor closes.
pub(crate) fn finish(
    session: &Session,
    connection: Handle,
    entry: Option<Entry>,
    sql: &str,
    outcome: Outcome,
) -> Option<End> {
    let Outcome {
        result,
        affected,
        elapsed_ms,
    } = outcome;
    let rows = match affected {
        u32::MAX => -1,
        n => n as i32,
    };
    let Some(entry) = entry else {
        // It failed before it ran.
        if let Err(error) = result {
            error_row(session, connection, sql, error);
        }
        return None;
    };
    // The log is not the one the statement began in.
    let (_, now) = active(session, connection)?;
    if now.generation != entry.active.generation {
        return None;
    }
    let settings = &entry.active.settings;
    let table = &settings.table;
    match result {
        Ok(cursor) => {
            let run = Record {
                optimized: Some(entry.optimized),
                code: Some(0),
                rows: Some(rows),
                run_time: Some(elapsed_ms),
                ..Record::new(session, connection, sql)
            };
            let id = match entry.id {
                Some(id) => {
                    update(
                        session,
                        connection,
                        table,
                        id,
                        &format!(
                            "\"Return Code\" = 0, \"Rows Affected\" = {rows}, \"Run Time\" = {elapsed_ms:.2}"
                        ),
                    );
                    Some(id)
                }
                None if settings.error_only() => None,
                // The ones that are optimized are not what it is asked for.
                None if settings.only_unoptimized && entry.optimized => None,
                None if settings.only_unoptimized => insert(
                    session,
                    connection,
                    table,
                    &Record {
                        optimized: Some(false),
                        ..Record::new(session, connection, sql)
                    },
                ),
                None if (elapsed_ms as i64) < settings.minimum.saturating_mul(1000) => None,
                None => insert(session, connection, table, &run),
            };
            match (cursor, id) {
                (true, Some(id)) => Some(End {
                    key: entry.key,
                    generation: entry.active.generation,
                    table: table.clone(),
                    id,
                }),
                _ => None,
            }
        }
        Err(error) => {
            let error = ace_error::sql_error(error.clone());
            let Error::Sql(sql_error) = &error else {
                return None;
            };
            // What the data does is a failure of the running; the rest is of
            // the statement before it ran.
            let running = sql_error.state.starts_with("22") || sql_error.native == 5154;
            let description = description(&error);
            let code = sql_error.native;
            // A procedure that fails has no rows to count.
            let rows = if code == 5154 { -1 } else { rows };
            match (entry.id, running) {
                (Some(id), true) => {
                    // The cursor it had is over with the failure.
                    let (day, ms) = crate::clock::now().local;
                    let end = match entry.cursor && sql_error.native != 5154 {
                        true => format!(", \"End Time\" = {}", timestamp(day, ms)),
                        false => String::new(),
                    };
                    update(
                        session,
                        connection,
                        table,
                        id,
                        &format!(
                            "\"Return Code\" = {code}, \"Rows Affected\" = {rows}, \
                             \"Run Time\" = {elapsed_ms:.2}, Description = {}{end}",
                            quote(&description)
                        ),
                    );
                    None
                }
                (Some(id), false) => {
                    update(
                        session,
                        connection,
                        table,
                        id,
                        &format!(
                            "Optimized = NULL, \"Return Code\" = {code}, Description = {}",
                            quote(&description)
                        ),
                    );
                    None
                }
                (None, true) => {
                    if settings.only_unoptimized && entry.optimized {
                        return None;
                    }
                    let row = Record {
                        optimized: Some(entry.optimized),
                        code: Some(code),
                        description: Some(description),
                        ..Record::new(session, connection, sql)
                    };
                    let row = match settings.error_only() || settings.only_unoptimized {
                        true => row,
                        false => Record {
                            rows: Some(rows),
                            run_time: Some(elapsed_ms),
                            ..row
                        },
                    };
                    insert(session, connection, table, &row);
                    None
                }
                (None, false) => {
                    error_row(session, connection, sql, &error);
                    None
                }
            }
        }
    }
}

/// What a failure puts in the log's `Description`: the error as the SQL
/// engine words it.
fn description(error: &Error) -> String {
    match error {
        Error::Sql(error) => error.to_string().replacen("AQE Error:  ", "", 1),
        other => format!("{other:?}"),
    }
}

/// The row of a statement that failed before it ran: it has nothing but its
/// error.
fn error_row(session: &Session, connection: Handle, sql: &str, error: &Error) {
    let Some((_, active)) = active(session, connection) else {
        return;
    };
    let settings = &active.settings;
    if settings.no_error() || settings.minimum > 0 {
        return;
    }
    let error = ace_error::sql_error(error.clone());
    let Error::Sql(sql_error) = &error else {
        return;
    };
    let row = Record {
        code: Some(sql_error.native),
        description: Some(description(&error)),
        ..Record::new(session, connection, sql)
    };
    insert(session, connection, &settings.table, &row);
}

/// Puts the end time of a statement whose cursor closed in.
pub(crate) fn ended(session: &Session, connection: Handle, end: End) {
    let Some((key, active)) = active(session, connection) else {
        return;
    };
    if key != end.key || active.generation != end.generation {
        return;
    }
    let (day, ms) = crate::clock::now().local;
    update(
        session,
        connection,
        &end.table,
        end.id,
        &format!("\"End Time\" = {}", timestamp(day, ms)),
    );
}

/// A row of the log, as far as it is known.
struct Record {
    started: (i32, u32),
    optimized: Option<bool>,
    code: Option<u32>,
    rows: Option<i32>,
    run_time: Option<f64>,
    description: Option<String>,
    database: String,
    user: Option<String>,
    connection: String,
    application: String,
    sql: String,
}

impl Record {
    /// The row of the statement `sql` on `connection`, as it starts.
    fn new(session: &Session, connection: Handle, sql: &str) -> Record {
        let owner = connection::get(session, connection).ok();
        let user = owner.as_ref().and_then(|owner| match owner.dictionary {
            true => Some(owner.user().unwrap_or("ADSSYS").to_owned()),
            false => None,
        });
        Record {
            started: crate::clock::now().local,
            optimized: None,
            code: None,
            rows: None,
            run_time: None,
            description: None,
            database: owner
                .map(|owner| owner.path.display().to_string())
                .unwrap_or_default(),
            user,
            connection: connection::os_user(),
            application: connection::application_id(session, connection).unwrap_or_default(),
            sql: sql.to_owned(),
        }
    }
}

/// `text` as an SQL literal.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

fn timestamp(day: i32, ms: u32) -> String {
    let date = tflip_storage::value::julian_to_ymd(day);
    format!(
        "{{ts '{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}'}}",
        date.year,
        date.month,
        date.day,
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// What a statement of the log's own gave.
struct Done {
    /// What it inserted (its last AutoInc value).
    inserted: Option<i32>,
    /// For a `SELECT`, its columns and its rows.
    fields: Vec<Field>,
    rows: Vec<Vec<Value>>,
}

/// Runs `sql` on a statement of `connection` that is not logged, with the
/// access rights of the log's owner.
fn run(session: &Session, connection: Handle, sql: &str) -> Result<Done, Error> {
    // The password of a free log table goes with every statement on it.
    let password = active(session, connection)
        .filter(|(_, active)| !active.settings.password.is_empty())
        .map(|(_, active)| (active.settings.table, active.settings.password))
        .filter(|_| connection::get(session, connection).is_ok_and(|owner| !owner.dictionary));
    run_with(session, connection, sql, password)
}

/// [`run`], with the password of the table `.0` as `.1` if given.
fn run_with(
    session: &Session,
    connection: Handle,
    sql: &str,
    password: Option<(String, String)>,
) -> Result<Done, Error> {
    let _level = Level::enter();
    let owner = connection::get(session, connection)?;
    // The log's own writes leave no trace in what the connection reports.
    let last = *lock(&owner.last_autoinc);
    let operations = owner.operations.load(Ordering::Relaxed);
    let result = owner.as_owner(|| {
        let handle = statement::create(session, connection)?;
        if let Some((table, password)) = password {
            statement::set(
                session,
                handle,
                statement::Setting::TablePassword(table, password),
            )?;
        }
        let done = (|| {
            let cursor = statement::execute_direct(session, handle, sql, true)?;
            let read = match cursor {
                Some(cursor) => {
                    let read = cursor_rows(session, cursor);
                    table::close(session, cursor)?;
                    read
                }
                None => (Vec::new(), Vec::new()),
            };
            Ok::<_, Error>(read)
        })();
        let _ = statement::close(session, handle);
        done
    });
    let inserted = std::mem::replace(&mut *lock(&owner.last_autoinc), last);
    owner.operations.store(operations, Ordering::Relaxed);
    let (fields, rows) = result?;
    Ok(Done {
        inserted,
        fields,
        rows,
    })
}

/// The columns and the rows of a cursor, in record order.
fn cursor_rows(session: &Session, cursor: Handle) -> (Vec<Field>, Vec<Vec<Value>>) {
    let Ok(table) = table::get(session, cursor) else {
        return (Vec::new(), Vec::new());
    };
    let fields = table.fields().to_vec();
    let mut rows = Vec::new();
    let _ = table.scan(|_, bytes, memo| {
        let mut row = crate::filter::SqlRow {
            fields: &fields,
            bytes,
            memo,
            record: None,
            hidden: &[],
        };
        rows.push(
            (0..fields.len())
                .map(|i| row.value(i).unwrap_or(Value::Null))
                .collect(),
        );
        Ok(())
    });
    (fields, rows)
}

/// `value` as an SQL literal: NULL for none.
fn literal<T: ToString>(value: Option<T>) -> String {
    value.map_or_else(|| "NULL".to_owned(), |value| value.to_string())
}

/// Puts `row` in the log table `table`; its ID.
fn insert(session: &Session, connection: Handle, table: &str, row: &Record) -> Option<i64> {
    let text = |value: &str| match value.is_empty() {
        true => "NULL".to_owned(),
        false => quote(value),
    };
    let sql = format!(
        "INSERT INTO {} (\"Start Time\", Optimized, \"Return Code\", \"Rows Affected\", \
         \"Run Time\", Database, \"User Name\", \"Connection Name\", \"Application ID\", Query, \
         Description) VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
        quote_name(table),
        timestamp(row.started.0, row.started.1),
        literal(row.optimized.map(|optimized| match optimized {
            true => "TRUE",
            false => "FALSE",
        })),
        literal(row.code),
        literal(row.rows),
        literal(row.run_time.map(|ms| format!("{ms:.2}"))),
        quote(&row.database),
        row.user.as_deref().map_or_else(|| "NULL".to_owned(), quote),
        quote(&row.connection),
        text(&row.application),
        quote(&row.sql),
        row.description
            .as_deref()
            .map_or_else(|| "NULL".to_owned(), quote),
    );
    let done = run(session, connection, &sql).ok()?;
    done.inserted.map(i64::from)
}

fn update(session: &Session, connection: Handle, table: &str, id: i64, set: &str) {
    let sql = format!("UPDATE {} SET {set} WHERE ID = {id}", quote_name(table));
    let _ = run(session, connection, &sql);
}

/// A table's name as SQL has it.
pub(crate) fn quote_name(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The log table's columns: name, type, length and decimals.
pub(crate) const SCHEMA: [(&str, FieldType, u16, u16); 13] = [
    ("ID", FieldType::AutoInc, 4, 0),
    ("Start Time", FieldType::TimeStamp, 8, 0),
    ("Optimized", FieldType::Logical, 1, 0),
    ("Return Code", FieldType::Integer, 4, 0),
    ("Rows Affected", FieldType::Integer, 4, 0),
    ("End Time", FieldType::TimeStamp, 8, 0),
    ("Run Time", FieldType::Double, 8, 2),
    ("Database", FieldType::Character, 255, 0),
    ("User Name", FieldType::Character, 100, 0),
    ("Connection Name", FieldType::Character, 100, 0),
    ("Application ID", FieldType::Memo, 9, 0),
    ("Query", FieldType::Memo, 9, 0),
    ("Description", FieldType::Memo, 9, 0),
];

/// Whether the free table `table` of `connection` exists and is encrypted;
/// `None` for one that does not exist or is a dictionary's.
fn free_log_encrypted(session: &Session, connection: Handle, table: &str) -> Option<bool> {
    use std::io::Read;
    if connection::get(session, connection).ok()?.dictionary {
        return None;
    }
    let mut path = crate::ddl::path_in(session, connection, table).ok()?;
    if path.extension().is_none() {
        path.set_extension("adt");
    }
    let mut bytes = [0; tflip_storage::adt::HEADER_SIZE];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut bytes))
        .ok()?;
    Some(tflip_storage::adt::parse_header(&bytes).ok()?.encrypted())
}

/// The error of a free log table that does not take `password`: 7060, as
/// the SQL engine words it.
fn wrong_password(
    session: &Session,
    connection: Handle,
    table: &str,
    password: &str,
) -> Result<(), Error> {
    let settings = TableSettings {
        table_type: TableType::Adt,
        char_type: 1,
        options: table::options::SHARED,
        password: password.to_owned(),
        by_statement: true,
        ..TableSettings::default()
    };
    match table::open(session, connection, table, settings) {
        Ok(handle) => table::close(session, handle),
        Err(Error::InvalidPassword) => Err(ace_error::sql(7060, "")),
        Err(error) => Err(ace_error::sql_error(error)),
    }
}

/// Makes the log table `table` if there is none, and empties it if it is to
/// be; a table that is not a log table is refused. A free table is made
/// plain and encrypted with `password` if it has one (a dictionary's is
/// never encrypted); one that is encrypted already takes only its password
/// (7060), or none, with which the log is locked (5104 if it is to be
/// emptied): the result is whether it is locked
/// (`fixtures/aes/api/11_querylog`).
pub(crate) fn prepare(
    session: &Session,
    connection: Handle,
    table: &str,
    truncate: bool,
    password: &str,
) -> Result<bool, Error> {
    let owner = connection::get(session, connection)?;
    // A dictionary's tables have names, not paths.
    if owner.dictionary && table.contains(['\\', '/']) {
        return Err(ace_error::sql(
            5132,
            &format!("\"{table}\" is not a table or view in the data dictionary."),
        ));
    }
    let name = quote_name(table);
    let free = !owner.dictionary;
    let password = match free {
        true => password,
        false => "",
    };
    let encrypted = free_log_encrypted(session, connection, table);
    let with_password = |sql: &str| {
        let password = (encrypted == Some(true) && !password.is_empty())
            .then(|| (table.to_owned(), password.to_owned()));
        run_with(session, connection, sql, password)
    };
    if encrypted == Some(true) {
        match password.is_empty() {
            true if truncate => {
                return Err(ace_error::sql(
                    5104,
                    "You must call AdsEnableEncryption first.",
                ));
            }
            true => return Ok(true),
            false => wrong_password(session, connection, table, password)?,
        }
    }
    match with_password(&format!("SELECT * FROM {name} WHERE 1 = 0")) {
        Ok(done) => {
            let fits =
                done.fields.len() == SCHEMA.len()
                    && done.fields.iter().zip(SCHEMA).all(
                        |(field, (name, kind, length, decimals))| {
                            field.name.eq_ignore_ascii_case(name)
                                && field.field_type == kind
                                && field.decimals() == decimals
                                && field.length() == length
                        },
                    );
            if !fits {
                return Err(ace_error::sql(
                    5226,
                    &format!("Schema is not compatible with query logging: {table}"),
                ));
            }
        }
        Err(_) => {
            let make = |free_table: bool| {
                let create = || {
                    run(
                        session,
                        connection,
                        &format!("CREATE TABLE {name} ({COLUMNS}) MEMOBLOCKSIZE 512"),
                    )?;
                    run(
                        session,
                        connection,
                        &format!("CREATE INDEX \"ID\" ON {name} (ID)"),
                    )?;
                    if free_table && !password.is_empty() {
                        let quoted = password.replace('\'', "''");
                        run(
                            session,
                            connection,
                            &format!(
                                "EXECUTE PROCEDURE sp_EncryptTable({}, '{quoted}')",
                                quote(table)
                            ),
                        )?;
                    }
                    Ok::<_, Error>(())
                };
                match free_table {
                    true => crate::ddl::with_plain_layout(create),
                    false => create(),
                }
            };
            make(free)?;
        }
    }
    if truncate {
        with_password(&format!("DELETE FROM {name}"))?;
    }
    Ok(false)
}

/// The rows of the log table `table`; those of a free table that is
/// encrypted need its `password`, which, wrong, is 7060, and, none, fails
/// reading a time (5101).
pub(crate) fn contents(
    session: &Session,
    connection: Handle,
    table: &str,
    password: &str,
) -> Result<Vec<Vec<Value>>, Error> {
    if free_log_encrypted(session, connection, table) == Some(true) {
        match password.is_empty() {
            true => return Err(ace_error::sql(5101, "Field: \"Start Time\"")),
            false => wrong_password(session, connection, table, password)?,
        }
    }
    Ok(run(
        session,
        connection,
        &format!("SELECT * FROM {}", quote_name(table)),
    )?
    .rows)
}
