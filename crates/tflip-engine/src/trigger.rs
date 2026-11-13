//! Table triggers: the SQL that a dictionary runs before, after or instead of
//! an `INSERT`, `UPDATE` or `DELETE` on a table.
//!
//! When a statement — or a table-API write or delete ([`write_record_api`],
//! [`delete_record_api`]) — changes a table of a dictionary, its triggers fire
//! ([`load`] finds them, [`fire`] runs them): a `BEFORE` trigger before each
//! affected record's change, an `AFTER` trigger after it, and an `INSTEAD OF`
//! trigger in its place (the change itself is skipped). A trigger body is
//! SQL/PSM like a stored procedure's; it reads the affected record through the
//! pseudo-tables `__new` (the row after an insert or update) and `__old` (the
//! row before an update or delete) — as a `FROM` table, or as the columns
//! `__new.col`/`__old.col` — and the record number as `::stmt.TrigRecNo`.
//!
//! The pseudo-tables are held in a per-thread registry while a trigger runs, so
//! the embedded SQL that names them in `FROM` resolves them ([`lookup`], which
//! [`crate::statement`] consults when it opens a table).
//!
//! As the original engine does (`fixtures/sql/triggers.json`): a trigger made
//! `NO MEMOS` reads its memo and BLOB columns as NULL; with real transactions,
//! each record's change (through SQL or the table API) runs with the
//! triggers it fires in a transaction of its own (two levels deeper in the
//! client's), which a failing trigger takes back, unless one trigger of the
//! event is `NO TRANSACTION`; and a failing trigger's error is 5177's, with the
//! error number and text the trigger wrote to its `__error` table (as `TrigError
//! 12345:   text`, the number left out for none) after it.
//! `CONFLICT` triggers are made and kept, but only replication fires them,
//! which tflip does not do: never a table's own changes.

use std::cell::RefCell;
use std::collections::HashMap;

use tflip_sql::ast::{Delete, Expr, ExprKind, Insert, InsertSource, Statement as Sql, Update};
use tflip_sql::script::{self, Vars};
use tflip_sql::{Compiler, Env, NoRow, SqlError, Ty, V, eval};
use tflip_storage::build::Column;
use tflip_storage::dd::{Kind, trigger};
use tflip_storage::field::Field;
use tflip_storage::value::Value;

/// A table built in memory: its columns and rows (a pseudo-table `__new`/`__old`).
type Built = (Vec<Column>, Vec<Vec<Value>>);

use tflip_sql::dml::DmlKind;

use crate::dictionary_write::rights;
use crate::handle::Handle;
use crate::script_host::{ScriptCtx, SettingsFrom};
use crate::statement::Settings;
use crate::table::Row;
use crate::{Error, Session, connection, statement};

mod opens;
pub(crate) mod order;
pub(crate) use opens::{
    Opened, attach, close_all, collecting, collects, connection_closing, created, dropping,
    fire_open, table_closed, unseen,
};

/// When a trigger runs relative to the change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timing {
    Before,
    InsteadOf,
    After,
}

thread_local! {
    /// The table, event and timing of the triggers that run, innermost last.
    static RUNNING: RefCell<Vec<(String, u32, u32)>> = const { RefCell::new(Vec::new()) };
}

/// Notes the trigger that runs, for as long as it does: writes to its table,
/// of the event it fires on, fire no trigger of the table while an `AFTER` or
/// `INSTEAD OF` trigger of them is the innermost of those that run (a `BEFORE`
/// trigger's own writes fire, and it does not hide the trigger it runs in), as
/// the original does (`fixtures/sql/trigself.json`,
/// `fixtures/sql/trigproc2.json`).
struct Running;

impl Running {
    fn enter(table: &str, event: u32, timing: u32) -> Running {
        let now = (table.to_ascii_lowercase(), event, timing);
        RUNNING.with(|running| running.borrow_mut().push(now));
        Running
    }

    /// Whether a trigger that runs makes writes to `table` of `event` fire
    /// none.
    fn silences(table: &str, event: u32) -> bool {
        RUNNING.with(|running| {
            let running = running.borrow();
            let innermost = running
                .iter()
                .rev()
                .find(|(_, _, timing)| *timing != BEFORE);
            innermost.is_some_and(|(t, e, _)| *e == event && t.eq_ignore_ascii_case(table))
        })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.with(|running| running.borrow_mut().pop());
    }
}

/// A trigger to run: its body, its priority (triggers of the same timing
/// run in ascending priority) and its options (`ADS_TRIGOPTIONS_*`).
#[derive(Debug, Clone)]
struct Trigger {
    name: String,
    body: String,
    priority: u32,
    options: u32,
    /// Whether the dictionary keeps its script where the original cannot read
    /// it back (one of 65,535 bytes), and fails the statement that reaches
    /// the trigger (`fixtures/sql/trigbig.json`).
    unreadable: bool,
    /// The function of a library it runs, if its container is one.
    function: Option<Function>,
}

/// The function of a library a trigger runs (`ADS_TRIGGER_WIN32DLL`).
#[derive(Debug, Clone)]
struct Function {
    library: String,
    name: String,
    /// Whether the library is a file: a COM or .NET container is not, and
    /// the original's server cannot load one either.
    file: bool,
}

impl Trigger {
    /// The trigger the dictionary object `object` is, if it has a container
    /// to run.
    fn of(object: &tflip_storage::dd::Object) -> Option<Trigger> {
        let container = object.u32(trigger::CONTAINER_TYPE);
        let unreadable = container == Some(SQL_SCRIPT)
            && object
                .slot(trigger::CONTAINER)
                .is_some_and(<[u8]>::is_empty);
        let body = object
            .text(trigger::CONTAINER)
            .or(unreadable.then(String::new))?;
        let function = match container {
            Some(SQL_SCRIPT) => None,
            container => Some(Function {
                library: body.clone(),
                name: object.text(trigger::FUNCTION_NAME).unwrap_or_default(),
                file: container == Some(WIN32_DLL),
            }),
        };
        Some(Trigger {
            name: object.name.clone(),
            body: if function.is_some() {
                String::new()
            } else {
                body
            },
            priority: object.u32(trigger::PRIORITY).unwrap_or(0),
            options: object.u32(trigger::OPTIONS).unwrap_or(3),
            unreadable,
            function,
        })
    }
}

/// `ADS_TRIGOPTIONS_WANT_VALUES`: `__new` and `__old` at all (else neither
/// is there, `NO VALUES`).
const WANT_VALUES: u32 = 1;
/// `ADS_TRIGOPTIONS_WANT_MEMOS_AND_BLOBS`: the memo and BLOB values in
/// `__new` and `__old` (else NULL, `NO MEMOS`).
const WANT_MEMOS: u32 = 2;
/// `ADS_TRIGOPTIONS_NO_TRANSACTION`.
const NO_TRANSACTION: u32 = 4;
/// `ADS_TRIGGER_SQL_SCRIPT`: the container of a trigger that is a script.
const SQL_SCRIPT: u32 = 3;
/// `ADS_TRIGGER_WIN32DLL`: a Windows DLL or a Linux shared object.
const WIN32_DLL: u32 = 1;
/// `ADS_TRIGTYPE_BEFORE` and `ADS_TRIGTYPE_AFTER`.
const BEFORE: u32 = 1;
const AFTER: u32 = 4;
/// The `ADS_DD_DATABASE_OBJECT` type, for the right to alter the database.
const DATABASE: u16 = 11;
/// The first of the events of a database trigger, `ADS_TRIGEVENT_OPEN_TABLE`
/// (then `CLOSE_TABLE`, `CONNECT` and `DISCONNECT`).
const DATABASE_EVENT: u32 = 4;
/// `ADS_TRIGEVENT_OPEN_TABLE` and `ADS_TRIGEVENT_CLOSE_TABLE`.
const OPEN_EVENT: u32 = 4;
const CLOSE_EVENT: u32 = 5;
/// `ADS_TRIGEVENT_CONNECT` and `ADS_TRIGEVENT_DISCONNECT`.
const CONNECT_EVENT: u32 = 6;
const DISCONNECT_EVENT: u32 = 7;

/// The triggers of one event on a table, by timing.
#[derive(Debug, Default)]
pub struct Set {
    before: Vec<Trigger>,
    instead_of: Vec<Trigger>,
    after: Vec<Trigger>,
    /// The table the triggers are on, the event they fire on (`ADS_TRIGEVENT_*`)
    /// and the start of its records' `ROWID`s, which a trigger tells of.
    table: String,
    event: u32,
    rowid_prefix: Option<std::sync::Arc<str>>,
}

impl Set {
    /// Whether the table has no triggers for the event.
    pub fn is_empty(&self) -> bool {
        self.before.is_empty() && self.instead_of.is_empty() && self.after.is_empty()
    }

    /// Whether an `INSTEAD OF` trigger replaces the change.
    pub fn has_instead_of(&self) -> bool {
        !self.instead_of.is_empty()
    }

    /// Whether a record's change runs with its triggers in a transaction:
    /// unless one of the event's triggers is `NO TRANSACTION`, which the
    /// others follow (`fixtures/sql/txtrig.json`).
    pub fn transactional(&self) -> bool {
        let triggers = || {
            [&self.before, &self.instead_of, &self.after]
                .into_iter()
                .flatten()
        };
        triggers().next().is_some() && triggers().all(|t| t.options & NO_TRANSACTION == 0)
    }

    /// The set with the start of the `ROWID`s of its table's records.
    pub fn with_rowid_prefix(mut self, prefix: Option<std::sync::Arc<str>>) -> Set {
        self.rowid_prefix = prefix;
        self
    }

    /// Fires this set's triggers of `timing` on the affected record `at`.
    pub fn fire(
        &self,
        session: &Session,
        connection: Handle,
        settings: &Settings,
        timing: Timing,
        at: &Affected<'_>,
    ) -> Result<(), Error> {
        let (triggers, kind) = match timing {
            Timing::Before => (&self.before, 1),
            Timing::InsteadOf => (&self.instead_of, 2),
            Timing::After => (&self.after, 4),
        };
        let rowid = match &self.rowid_prefix {
            Some(prefix) => tflip_sql::rowid::encode(prefix, at.recno),
            None => String::new(),
        };
        let on = Event {
            table: &self.table,
            event: self.event,
            kind,
            rowid,
            info: None,
        };
        // A trigger that fails is the ACE error 5177 with what failed after
        // it (probed, `tools/ace/dd/constraints.c`), which SQL wraps again.
        fire(session, connection, settings, triggers, at, &on).map_err(Self::error)
    }

    /// The error of a trigger that stopped the change.
    fn error(fired: Fired) -> Error {
        match fired {
            Fired::Raised(error) => error,
            Fired::Failed(error) => Self::failed(error),
        }
    }

    /// The error of a trigger whose script failed with `error`.
    fn failed(error: Error) -> Error {
        match error {
            Error::Sql(sql) => Error::Constraint {
                code: crate::constraints::TRIGGER_FAILED,
                text: format!("Trigger execution failed.  Error 7200:  {sql}"),
            },
            // A trigger that fires another and fails in it is a statement of
            // its own that failed, as SQL words it.
            Error::Constraint { code, text } if code == crate::constraints::TRIGGER_FAILED => {
                let sql = SqlError::ace(code, &text, "");
                Error::Constraint {
                    code,
                    text: format!("Trigger execution failed.  Error 7200:  {sql}"),
                }
            }
            Error::Constraint { code, text } => Error::Constraint {
                code: crate::constraints::TRIGGER_FAILED,
                text: format!("Trigger execution failed.  Error {code}:  {text}"),
            },
            error => error,
        }
    }
}

/// The triggers of `connection`'s dictionary on the table `table_name` for the
/// event `kind`. Empty for a folder connection or a table with none.
pub fn load(session: &Session, connection: Handle, table_name: &str, kind: DmlKind) -> Set {
    let event = match kind {
        DmlKind::Insert => 1,
        DmlKind::Update | DmlKind::Merge => 2,
        DmlKind::Delete => 3,
    };
    let short = table_name.rsplit(['\\', '/']).next().unwrap_or(table_name);
    let short = short.split('.').next().unwrap_or(short);
    if Running::silences(short, event) {
        return Set::default();
    }
    let Ok(owner) = connection::get(session, connection) else {
        return Set::default();
    };
    let silenced = owner
        .triggers_disabled
        .load(std::sync::atomic::Ordering::Relaxed);
    owner
        .with_database_opt(|db| {
            let dict = db.dictionary();
            let Some(table) = dict.named(Kind::Table, short) else {
                return Set::default();
            };
            // Disabled for the connection, the dictionary or the table.
            let database_off = dict
                .database()
                .is_some_and(|d| d.has(tflip_storage::dd::database::TRIGGERS_DISABLED));
            if silenced || database_off || table.has(tflip_storage::dd::table::TRIGGERS_DISABLED) {
                return Set::default();
            }
            let mut set = Set {
                table: table.name.clone(),
                event,
                ..Set::default()
            };
            // The newest first (`order.rs`); a stable sort by priority puts
            // those of one priority in that order.
            let sequence = owner.trigger_sequence(dict);
            let mut objects = dict.all(Kind::Trigger);
            objects.sort_by_key(|o| sequence.iter().position(|&id| id == o.id));
            for object in objects {
                if object.parent != table.id
                    || object.u32(trigger::EVENT_TYPE) != Some(event)
                    || object.has(trigger::TRIGGERS_DISABLED)
                {
                    continue;
                }
                let Some(trigger) = Trigger::of(object) else {
                    continue;
                };
                match object.u32(trigger::TRIGGER_TYPE) {
                    Some(1) => set.before.push(trigger),
                    Some(2) => set.instead_of.push(trigger),
                    Some(4) => set.after.push(trigger),
                    // CONFLICT (8) and anything else do not fire here.
                    _ => {}
                }
            }
            for list in [&mut set.before, &mut set.instead_of, &mut set.after] {
                list.sort_by_key(|t| t.priority);
            }
            if !set.is_empty() {
                owner.trigger_order_fired();
            }
            set
        })
        .unwrap_or_default()
}

/// The database triggers of `connection`'s dictionary for `event` (`OPEN_TABLE`,
/// `CLOSE_TABLE`, `CONNECT` or `DISCONNECT`) of the timing `timing`, in the order
/// they run.
fn database_triggers(
    session: &Session,
    connection: Handle,
    event: u32,
    timing: u32,
) -> Vec<Trigger> {
    let Ok(owner) = connection::get(session, connection) else {
        return Vec::new();
    };
    owner
        .with_database_opt(|db| {
            let dict = db.dictionary();
            let Some(database) = dict.database() else {
                return Vec::new();
            };
            if database.has(tflip_storage::dd::database::TRIGGERS_DISABLED) {
                return Vec::new();
            }
            let mut found: Vec<Trigger> = dict
                .all(Kind::Trigger)
                .into_iter()
                .filter(|t| {
                    t.parent == database.id
                        && t.u32(trigger::EVENT_TYPE) == Some(event)
                        && t.u32(trigger::TRIGGER_TYPE) == Some(timing)
                        && !t.has(trigger::TRIGGERS_DISABLED)
                })
                .filter_map(Trigger::of)
                .collect();
            // Triggers of one priority run the newest first (probed,
            // `tools/ace/sql/trigdblog.txt`).
            found.reverse();
            found.sort_by_key(|t| t.priority);
            found
        })
        .unwrap_or_default()
}

/// Fires the database triggers for `event` of `timing` on `connection`, which
/// read `info` as `__info`.
fn fire_database(
    session: &Session,
    connection: Handle,
    event: u32,
    timing: u32,
    info: Built,
) -> Result<(), Error> {
    // What a database trigger itself opens fires no trigger of its kind.
    if opens::silenced(event, timing) {
        return Ok(());
    }
    let triggers = database_triggers(session, connection, event, timing);
    if triggers.is_empty() {
        return Ok(());
    }
    let _firing = opens::firing(event, timing);
    let at = Affected {
        new: None,
        old: None,
        recno: 0,
        transaction: false,
    };
    let on = Event {
        table: "",
        event,
        kind: timing,
        rowid: String::new(),
        info: Some(info),
    };
    fire(
        session,
        connection,
        &Settings::default(),
        &triggers,
        &at,
        &on,
    )
    .map_err(Set::error)
}

/// What `__info` tells a trigger of the connection `connection`'s own events:
/// who connected, and how (`CONNECT` and `DISCONNECT`).
fn connection_info(session: &Session, connection: Handle, timing: u32, event: &str) -> Built {
    use tflip_storage::field::FieldType::{CiCharacter, Integer};
    let Ok(owner) = connection::get(session, connection) else {
        return (Vec::new(), Vec::new());
    };
    let encryption = owner
        .with_database_opt(|db| {
            db.dictionary()
                .database()
                .and_then(|d| d.u32(tflip_storage::dd::database::DATA_ENCRYPTION_TYPE))
        })
        .flatten()
        .unwrap_or(0);
    let administrator = owner.with_database_opt(|db| db.is_admin()).unwrap_or(false);
    let text = |s: &str, width: usize| Value::Text(format!("{s:<width$}"));
    let columns = [
        ("UserID", Integer, 4),
        ("TrigType", CiCharacter, 10),
        ("TrigEvent", CiCharacter, 15),
        ("UserName", CiCharacter, 50),
        ("ClientName", CiCharacter, 50),
        ("LoginName", CiCharacter, 50),
        ("ConnPath", CiCharacter, 256),
        ("Address", CiCharacter, 30),
        ("AppID", CiCharacter, 70),
        ("OpCount", Integer, 4),
        ("StmtCount", Integer, 4),
        ("CommType", CiCharacter, 10),
        ("EncrType", CiCharacter, 10),
        ("TLSCipher", CiCharacter, 20),
        ("TLSVersion", CiCharacter, 20),
    ]
    .map(|(name, field_type, length)| Column {
        name: name.to_owned(),
        field_type,
        length,
        decimals: 0,
    });
    let row = vec![
        Value::Integer(owner.user_id.into()),
        text(if timing == BEFORE { "BEFORE" } else { "AFTER" }, 10),
        text(event, 15),
        text(owner.user().unwrap_or("ADSSYS"), 50),
        text(&connection::os_user(), 50),
        text("", 50),
        text(&owner.path.display().to_string(), 256),
        Value::Null,
        text(
            &connection::application_id(session, connection).unwrap_or_default(),
            70,
        ),
        // The original counts the requests it served: 27 for the connection
        // (28 for a user other than the administrator), which is all tflip
        // can tell; what a statement costs depends on its plan.
        Value::Integer(
            27 + i128::from(!administrator)
                + i128::from(owner.operations.load(std::sync::atomic::Ordering::Relaxed)),
        ),
        Value::Integer(
            owner
                .statements
                .load(std::sync::atomic::Ordering::Relaxed)
                .into(),
        ),
        text("LOCAL", 10),
        text(crate::procedure::args::encryption_name(encryption), 10),
        Value::Null,
        Value::Null,
    ];
    (columns.to_vec(), vec![row])
}

/// The `CONNECT` triggers of a connection just made, which refuse it with
/// 7109 if one fails, unless it is the administrator's (whose connection they
/// run for, and whose failure is ignored).
pub(crate) fn fire_connect(session: &Session, connection: Handle) -> Result<(), Error> {
    let info = connection_info(session, connection, AFTER, "CONNECT");
    let fired = fire_database(session, connection, CONNECT_EVENT, AFTER, info);
    let administrator = connection::get(session, connection)?
        .with_database_opt(|db| db.is_admin())
        .unwrap_or(false);
    match fired {
        Err(error) if !administrator => {
            let (code, text) = match error {
                Error::Constraint { code, text } => (code, text),
                other => return Err(other),
            };
            Err(Error::Constraint {
                code: 7109,
                text: format!(
                    "Database logins are currently disabled or are prevented by a connection \
                     trigger.  Error {code}:  {text}"
                ),
            })
        }
        _ => Ok(()),
    }
}

/// The `DISCONNECT` triggers of a connection about to close, whose failure the
/// disconnect ignores.
pub(crate) fn fire_disconnect(session: &Session, connection: Handle) {
    let info = connection_info(session, connection, BEFORE, "DISCONNECT");
    let _ = fire_database(session, connection, DISCONNECT_EVENT, BEFORE, info);
}

/// One affected record, as a trigger reads it (`__new` or `__old`): the
/// table's fields and the record's values.
pub struct Record {
    fields: Vec<Field>,
    values: Vec<V>,
}

impl Record {
    pub fn new(fields: Vec<Field>, values: Vec<V>) -> Record {
        Record { fields, values }
    }

    /// The record's fields and values, for referential integrity.
    pub(crate) fn fields(&self) -> &[Field] {
        &self.fields
    }

    pub(crate) fn values(&self) -> &[V] {
        &self.values
    }

    /// The value of the field `name` as an expression of the field's type, so
    /// that a NULL keeps it.
    fn typed(&self, name: &str) -> Option<ExprKind> {
        let i = self
            .fields
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(name))?;
        Some(script::typed_value(self.values.get(i)?, &self.fields[i]))
    }

    /// The record as a `NO MEMOS` trigger reads it: its memo and BLOB values
    /// NULL.
    fn without_memos(&self) -> Record {
        let values = self
            .fields
            .iter()
            .zip(&self.values)
            .map(|(f, v)| if f.is_memo() { V::Null } else { v.clone() })
            .collect();
        Record {
            fields: self.fields.clone(),
            values,
        }
    }

    /// The record as a one-row table (its columns and that row).
    fn table(&self) -> Built {
        let columns: Vec<Column> = self
            .fields
            .iter()
            .map(|f| Column {
                name: f.name.clone(),
                field_type: f.field_type,
                length: f.length(),
                decimals: f.decimals(),
            })
            .collect();
        let row = self
            .fields
            .iter()
            .zip(&self.values)
            .map(|(f, v)| v.to_field(Ty::of_field(f), &f.name).unwrap_or(Value::Null))
            .collect();
        (columns, vec![row])
    }
}

/// An affected record a trigger reads: `new` its row after an insert or update,
/// `old` its row before an update or delete, `recno` its record number;
/// `transaction` whether the change runs in a transaction of its own (see
/// [`Set::transactional`]).
pub struct Affected<'a> {
    pub new: Option<&'a Record>,
    pub old: Option<&'a Record>,
    pub recno: u32,
    pub transaction: bool,
}

/// What a trigger tells of the change that fires it.
struct Event<'a> {
    table: &'a str,
    event: u32,
    kind: u32,
    rowid: String,
    /// What a database trigger tells of its event: `__info`.
    info: Option<Built>,
}

/// Why a trigger stopped the change.
enum Fired {
    /// The trigger wrote its error to `__error`: the error as it is.
    Raised(Error),
    /// Its script failed.
    Failed(Error),
}

impl From<Error> for Fired {
    fn from(error: Error) -> Fired {
        Fired::Failed(error)
    }
}

/// Runs the triggers `triggers` of one timing on the affected record `at`.
fn fire(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    triggers: &[Trigger],
    at: &Affected<'_>,
    on: &Event<'_>,
) -> Result<(), Fired> {
    if triggers.is_empty() {
        return Ok(());
    }
    // A table's triggers run their statements on tables of their own, which
    // the last firing's close first.
    let _opening = (!on.table.is_empty()).then(|| {
        opens::release_triggered(session, connection, on.table);
        opens::in_trigger(on.table)
    });
    let env = statement::connection_env(session, connection)?;
    let mut vars = Vars::new();
    // A trigger body runs with the table owner's rights, not the connected
    // user's, so its own DML is not refused by access-rights enforcement
    // (the original engine's triggers are not rights-checked as the invoker).
    let owner = connection::get(session, connection).ok();
    let mut run_all = || {
        for trigger in triggers {
            if trigger.unreadable {
                return Err(Fired::Raised(crate::ace_error::sql(
                    5005,
                    ".\\cconnect.cpp, 12678",
                )));
            }
            // A `NO MEMOS` trigger reads the memo and BLOB values as NULL.
            let (new, old);
            let (new, old) = match trigger.options & WANT_MEMOS {
                0 => {
                    new = at.new.map(Record::without_memos);
                    old = at.old.map(Record::without_memos);
                    (new.as_ref(), old.as_ref())
                }
                _ => (at.new, at.old),
            };
            // `__new`/`__old` as FROM tables, for the span of this trigger
            // (saved and restored so nested firings — an INSTEAD OF
            // trigger's own DML — nest).
            let mut entries = Vec::new();
            let values = trigger.options & WANT_VALUES != 0;
            if trigger.function.is_none() {
                entries.push(("__error".to_owned(), ErrorTable::default().table()));
            }
            if let Some(info) = &on.info {
                entries.push(("__info".to_owned(), info.clone()));
            }
            // `NO VALUES` leaves neither there to read.
            if let Some(new) = new.filter(|_| values) {
                entries.push(("__new".to_owned(), new.table()));
            }
            if let Some(old) = old.filter(|_| values) {
                entries.push(("__old".to_owned(), old.table()));
            }
            let _scope = enter(entries);
            let _running = Running::enter(on.table, on.event, on.kind);
            let _nesting =
                crate::psm::Nesting::enter("trigger", &trigger.name).map_err(Error::Sql)?;
            if let Some(function) = &trigger.function {
                run_function(session, connection, trigger, function, at, on)?;
                continue;
            }
            let body = tflip_sql::parse::parse_script(&trigger.body).map_err(Error::Sql)?;
            let transaction = at.transaction;
            let mut host = Host {
                ctx: ScriptCtx::new(session, connection, SettingsFrom::Given(settings))
                    .ending(crate::script_host::Ends::Never),
                env: &env,
                new,
                old,
                info: on.info.as_ref(),
                transaction,
                trigger: crate::sysvars::Trigger {
                    name: trigger.name.clone(),
                    table: on.table.to_owned(),
                    event: on.event,
                    kind: on.kind,
                    rowid: on.rowid.clone(),
                    recno: at.recno,
                },
                updated: 0,
                errors: ErrorTable::default(),
            };
            let result = script::run(&body, &mut vars, &mut host, &env);
            if let Some(error) = host.ctx.failure {
                return Err(error.into());
            }
            result.map_err(Error::Sql)?;
            // What the trigger wrote to `__error` stops the change once the
            // trigger is through, not where it is written.
            if let Some(error) = host.errors.raised() {
                return Err(Fired::Raised(error));
            }
        }
        Ok(())
    };
    match &owner {
        Some(owner) => owner.as_owner(run_all),
        None => run_all(),
    }
}

/// Whether an `UPDATE` of `table` is a write to the `__new` of the trigger
/// that runs.
pub(crate) fn writes_new(table: &str) -> bool {
    table.eq_ignore_ascii_case("__new") && lookup("__new").is_some()
}

/// Runs the function of a library that `trigger` is: it fails the change as a
/// script does, with what it wrote to `__error` (`fixtures/sql/aeptrig.json`);
/// its return value is reserved and ignored. A database trigger is told its
/// table is `Database`.
fn run_function(
    session: &Session,
    connection: Handle,
    trigger: &Trigger,
    function: &Function,
    at: &Affected<'_>,
    on: &Event<'_>,
) -> Result<(), Fired> {
    let raised = |text: String| {
        Fired::Raised(Error::Constraint {
            code: crate::constraints::TRIGGER_FAILED,
            text: format!("Trigger execution failed.  {text}"),
        })
    };
    if !function.file {
        return Err(raised(format!(
            "The container named \"{}\" failed to load with an error code 0.",
            function.library
        )));
    }
    let call = crate::aep::TriggerCall {
        name: &trigger.name,
        table: if on.table.is_empty() {
            "Database"
        } else {
            on.table
        },
        event: on.event,
        kind: on.kind,
        recno: at.recno,
    };
    let written = crate::aep::run_trigger(
        session,
        connection,
        &function.library,
        &function.name,
        &call,
    )
    .map_err(raised)?;
    let Some((number, message)) = written else {
        return Ok(());
    };
    let errors = ErrorTable {
        rows: vec![(number.map(|n| n as i32), message)],
        written: true,
    };
    match errors.raised() {
        Some(error) => Err(Fired::Raised(error)),
        None => Ok(()),
    }
}

/// The host that runs a trigger body's SQL statements, substituting the
/// record it reads as `__new.col`/`__old.col` and the record number.
struct Host<'a> {
    ctx: ScriptCtx<'a>,
    env: &'a Env,
    new: Option<&'a Record>,
    old: Option<&'a Record>,
    /// What a database trigger tells of its event (`__info`).
    info: Option<&'a Built>,
    /// Whether the trigger runs in a transaction of its own.
    transaction: bool,
    /// What the trigger tells of itself, and the rows it changed so far.
    trigger: crate::sysvars::Trigger,
    updated: u32,
    /// What the trigger wrote to `__error`.
    errors: ErrorTable,
}

/// The pseudo-table `__error`: the error number and text a trigger makes its
/// change fail with (probed, `fixtures/sql/trigerr.json`).
#[derive(Default)]
struct ErrorTable {
    /// Its rows, the error number and the text, in the order written.
    rows: Vec<(Option<i32>, Option<String>)>,
    /// Whether a row was ever written: a table that was written and is empty
    /// again has no record to read the error from.
    written: bool,
}

/// The name of the table a trigger writes its error to.
const ERROR_TABLE: &str = "__error";

impl ErrorTable {
    const NAMES: [&'static str; 2] = ["errno", "message"];

    /// The type of the column `slot`, if it is one.
    fn ty(slot: usize) -> Option<Ty> {
        let (columns, _) = ErrorTable::default().table();
        let column = columns.get(slot)?;
        Some(Ty::of_field(&statement::field_of(column)))
    }

    /// The position of the column `name`: 0 for `errno`, 1 for `message`.
    fn slot(name: &str) -> Option<usize> {
        Self::NAMES
            .iter()
            .position(|column| column.eq_ignore_ascii_case(name))
    }

    /// `value` as an `errno`, which is an `INTEGER` (NULL for 0 or none, as
    /// the original reads it).
    fn errno(value: &V) -> Result<Option<i32>, SqlError> {
        let (columns, _) = ErrorTable::default().table();
        let ty = Ty::of_field(&statement::field_of(&columns[0]));
        Ok(match value.to_field(ty, "errno")? {
            Value::Integer(n) => i32::try_from(n).ok(),
            _ => None,
        })
    }

    /// `value` as a `message`.
    fn message(value: &V) -> Result<Option<String>, SqlError> {
        let (columns, _) = ErrorTable::default().table();
        let ty = Ty::of_field(&statement::field_of(&columns[1]));
        Ok(match value.to_field(ty, "message")? {
            Value::Text(text) => Some(text),
            _ => None,
        })
    }

    /// The value of the column `kind` names in row `i`, as a literal, if it
    /// names one of `__error`'s.
    fn literal(&self, i: usize, kind: &ExprKind) -> Option<ExprKind> {
        let ExprKind::Column { table, name } = kind else {
            return None;
        };
        if table
            .as_deref()
            .is_some_and(|t| !t.eq_ignore_ascii_case(ERROR_TABLE))
        {
            return None;
        }
        let (errno, message) = &self.rows[i];
        let value = match Self::slot(name)? {
            0 => errno.map_or(V::Null, |n| V::Int(n.into())),
            _ => message.clone().map_or(V::Null, V::Str),
        };
        Some(script::literal_of(&value))
    }

    /// The table as a trigger reads it: `errno` and `message`.
    fn table(&self) -> Built {
        let columns = vec![
            Column {
                name: "errno".to_owned(),
                field_type: tflip_storage::field::FieldType::Integer,
                length: 4,
                decimals: 0,
            },
            Column {
                name: "message".to_owned(),
                field_type: tflip_storage::field::FieldType::Memo,
                length: 9,
                decimals: 0,
            },
        ];
        let rows = self
            .rows
            .iter()
            .map(|(errno, message)| {
                vec![
                    errno.map_or(Value::Null, |n| Value::Integer(n.into())),
                    message.clone().map_or(Value::Null, Value::Text),
                ]
            })
            .collect();
        (columns, rows)
    }

    /// The error the first row makes the change fail with, if it holds one:
    /// a number other than 0, or a text. The trigger failure's code is 5177
    /// whatever the number; it is in the text, and left out if there is none.
    fn raised(&self) -> Option<Error> {
        if !self.written {
            return None;
        }
        let Some((errno, message)) = self.rows.first() else {
            return Some(Error::Constraint {
                code: 5068,
                text: "There is no current record.  .\\cconnect.cpp, 11390".to_owned(),
            });
        };
        let errno = errno.filter(|n| *n != 0);
        let message = message.as_deref().filter(|m| !m.is_empty());
        if errno.is_none() && message.is_none() {
            return None;
        }
        let text = match errno {
            Some(n) => format!("TrigError {n}:   {}", message.unwrap_or("")),
            None => message.unwrap_or("").to_owned(),
        };
        Some(Error::Constraint {
            code: crate::constraints::TRIGGER_FAILED,
            text: format!("Trigger execution failed.  {text}"),
        })
    }
}

impl Host<'_> {
    /// The value of `expr`, which reads no table, over the script's variables.
    fn value(&self, expr: &Expr, vars: &Vars) -> Result<V, SqlError> {
        let expr = script::Host::rewrite_expr(self, expr, vars)?.unwrap_or_else(|| expr.clone());
        let node = Compiler::new(None, 0).compile(&expr)?;
        eval(&node, &mut NoRow, self.env)
    }

    /// The value of `expr` for the column `slot` of `__error`, or the error of
    /// an operand the column does not take.
    fn column_value(&self, expr: &Expr, slot: usize, vars: &Vars) -> Result<V, SqlError> {
        if let Some(ty) = ErrorTable::ty(slot) {
            script::check_column_value(expr, ty, ErrorTable::NAMES[slot])?;
        }
        self.constant(expr, vars)
    }

    /// [`Host::value`], a NULL one with no type of its own.
    fn constant(&self, expr: &Expr, vars: &Vars) -> Result<V, SqlError> {
        match expr.kind {
            ExprKind::Null => Ok(V::Null),
            _ => self.value(expr, vars),
        }
    }

    /// Shows the trigger what `__error` holds now.
    fn publish_errors(&self) {
        replace(ERROR_TABLE, self.errors.table());
    }

    /// `INSERT INTO __error`: the error number and text, by position or by
    /// the columns named.
    fn insert_error(&mut self, insert: &Insert, vars: &Vars) -> Result<(), SqlError> {
        let slots = match &insert.columns {
            Some(columns) => columns
                .iter()
                .map(|c| {
                    ErrorTable::slot(&c.name)
                        .ok_or_else(|| SqlError::column_not_found(&c.name, c.pos))
                })
                .collect::<Result<Vec<_>, _>>()?,
            None => vec![0, 1],
        };
        let rows = match &insert.source {
            InsertSource::Values(rows) => rows
                .iter()
                .map(|row| {
                    let slots = slots.iter().chain(std::iter::repeat(&2));
                    row.iter()
                        .zip(slots)
                        .map(|(e, slot)| self.column_value(e, *slot, vars))
                        .collect()
                })
                .collect::<Result<Vec<Vec<V>>, _>>()?,
            InsertSource::Select(select) => {
                let settings = self.ctx.settings_or_fail()?;
                statement::select_values(self.ctx.session, self.ctx.connection, &settings, select)
                    .map_err(|e| self.ctx.fail(e))?
                    .1
            }
        };
        for row in rows {
            if row.len() != slots.len() {
                // Worded with a space more here than elsewhere.
                let mut error = SqlError::insert_count();
                error.message.push(' ');
                return Err(error);
            }
            let mut entry = (None, None);
            for (slot, value) in slots.iter().zip(&row) {
                match slot {
                    0 => entry.0 = ErrorTable::errno(value)?,
                    _ => entry.1 = ErrorTable::message(value)?,
                }
            }
            self.errors.rows.push(entry);
            self.updated = self.updated.saturating_add(1);
        }
        self.errors.written = true;
        self.publish_errors();
        Ok(())
    }

    /// The rows of `__error` that `filter` holds for, by their indexes, and
    /// each one's columns as literals for an expression to read.
    fn error_rows(&self, filter: Option<&Expr>, vars: &Vars) -> Result<Vec<usize>, SqlError> {
        let mut found = Vec::new();
        for i in 0..self.errors.rows.len() {
            let held = match filter {
                Some(filter) => {
                    let filter = script::rewrite_expr(filter, &|kind| self.errors.literal(i, kind));
                    matches!(self.value(&filter, vars)?, V::Bool(true))
                }
                None => true,
            };
            if held {
                found.push(i);
            }
        }
        Ok(found)
    }

    /// `UPDATE __error`.
    fn update_error(&mut self, update: &Update, vars: &Vars) -> Result<(), SqlError> {
        for set in &update.sets {
            let name = &set.column;
            if ErrorTable::slot(&name.name).is_none() {
                return Err(SqlError::column_not_found(&name.name, name.pos));
            }
        }
        for i in self.error_rows(update.filter.as_ref(), vars)? {
            let mut entry = self.errors.rows[i].clone();
            for set in &update.sets {
                let expr = script::rewrite_expr(&set.value, &|kind| self.errors.literal(i, kind));
                let value = self.constant(&expr, vars)?;
                match ErrorTable::slot(&set.column.name) {
                    Some(0) => entry.0 = ErrorTable::errno(&value)?,
                    _ => entry.1 = ErrorTable::message(&value)?,
                }
            }
            self.errors.rows[i] = entry;
            self.updated = self.updated.saturating_add(1);
        }
        self.publish_errors();
        Ok(())
    }

    /// `DELETE FROM __error`.
    fn delete_error(&mut self, delete: &Delete, vars: &Vars) -> Result<(), SqlError> {
        let found = self.error_rows(delete.filter.as_ref(), vars)?;
        let mut index = 0;
        self.errors.rows.retain(|_| {
            index += 1;
            !found.contains(&(index - 1))
        });
        self.updated = self.updated.saturating_add(found.len() as u32);
        self.publish_errors();
        Ok(())
    }

    /// What the trigger reads of itself, its statement and connection
    /// (`::stmt.TrigName`).
    fn scope(&self) -> crate::sysvars::Scope<'_> {
        self.ctx.scope(None, self.updated, Some(&self.trigger))
    }

    /// The value of `__info.col`.
    fn info_value(&self, name: &str) -> Option<ExprKind> {
        let (columns, rows) = self.info?;
        let i = columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))?;
        let ty = Ty::of_field(&statement::field_of(&columns[i]));
        let value = V::from_field(rows.first()?[i].clone(), ty);
        Some(script::literal_of(&value))
    }

    /// The affected record's value for `__new.col` / `__old.col`, and a
    /// system variable.
    fn pseudo_column(&self, kind: &ExprKind) -> Option<ExprKind> {
        match kind {
            ExprKind::Column { table: None, .. } if self.scope().literal(kind).is_some() => {
                self.scope().literal(kind)
            }
            ExprKind::Column {
                table: Some(table),
                name,
            } if table.eq_ignore_ascii_case("__new") => self.new.and_then(|r| r.typed(name)),
            ExprKind::Column {
                table: Some(table),
                name,
            } if table.eq_ignore_ascii_case("__old") => self.old.and_then(|r| r.typed(name)),
            ExprKind::Column {
                table: Some(table),
                name,
            } if table.eq_ignore_ascii_case("__info") => self.info_value(name),
            _ => None,
        }
    }
}

impl script::Host for Host<'_> {
    // A trigger's `SELECT` is never run, so none is a row.
    fn statement_begins(&mut self, text: &str, statement: &Sql, _last: bool) {
        self.ctx.log_begins(text, statement, false);
    }

    fn statement_ends(&mut self, result: Result<(), &SqlError>) {
        let affected = self.updated;
        self.ctx.log_ends(result, affected);
    }

    fn cursor_opens(&mut self, declared: bool) {
        self.ctx.log_opens(declared);
    }

    fn script_ends(&mut self, last: bool) {
        self.ctx.log_script_ends(last);
    }

    fn fetch(&mut self, cursor: usize) -> Result<Option<Vec<V>>, SqlError> {
        self.ctx.fetch(cursor)
    }

    fn close(&mut self, cursor: usize) {
        self.ctx.close(cursor);
    }

    fn functions(&self) -> Vec<tflip_sql::FunctionSig> {
        self.ctx.functions()
    }

    fn rewrite_expr(
        &self,
        expr: &tflip_sql::ast::Expr,
        vars: &Vars,
    ) -> Result<Option<tflip_sql::ast::Expr>, SqlError> {
        let expr = script::rewrite_expr(expr, &|kind| self.pseudo_column(kind));
        self.ctx.rewrite_expr(&self.scope(), &expr, vars)
    }

    fn query(&mut self, def: &Sql, vars: &Vars) -> Result<script::Query, SqlError> {
        let def = script::rewrite(def, &|kind| match kind {
            ExprKind::Column { table: None, name } if vars.get(name).is_some() => vars.expr(name),
            kind => self.pseudo_column(kind),
        });
        match self.scope().statement(&def) {
            Sql::Select(select) => self.ctx.query(&select),
            Sql::Execute(execute) => self.ctx.procedure_query(&execute),
            _ => Err(SqlError::unsupported("this cursor", 0)),
        }
    }

    fn run(&mut self, statement: &Sql, vars: &Vars) -> Result<(), SqlError> {
        let statement = script::rewrite(statement, &|kind| match kind {
            // The record number and other script variables.
            ExprKind::Column { table: None, name } if vars.get(name).is_some() => vars.expr(name),
            // `__new.col` / `__old.col`: the affected record's value.
            kind => self.pseudo_column(kind),
        });
        let statement = self.scope().statement(&statement);
        match &statement {
            // The trigger's error table is in memory, not a table to write.
            Sql::Insert(insert) if insert.table.name.eq_ignore_ascii_case(ERROR_TABLE) => {
                self.insert_error(insert, vars)?;
            }
            Sql::Update(update) if update.table.name.eq_ignore_ascii_case(ERROR_TABLE) => {
                self.update_error(update, vars)?;
            }
            Sql::Delete(delete) if delete.table.name.eq_ignore_ascii_case(ERROR_TABLE) => {
                self.delete_error(delete, vars)?;
            }
            // Accepted, and the record is written as it was given: a trigger
            // cannot fill a field (`fixtures/sql/trignn.json`).
            Sql::Update(update) if update.table.name.eq_ignore_ascii_case("__new") => {}
            Sql::Insert(_) | Sql::Update(_) | Sql::Delete(_) | Sql::Merge(_) => {
                let rows = self
                    .ctx
                    .dml(&statement)
                    .map_err(|e| self.ctx.nested_failure(e))?;
                self.updated = self.updated.saturating_add(rows);
            }
            // A trigger body may run a `SELECT` with no effect; ignore its rows.
            Sql::Select(_) => {}
            Sql::Execute(execute) if !crate::procedure::is_builtin(&execute.name) => {
                // The original counts the arguments where it is called, as
                // a statement does, before anything of the procedure runs.
                let signature =
                    crate::psm::signature(self.ctx.session, self.ctx.connection, &execute.name)
                        .map_err(|e| self.ctx.fail(e))?
                        .ok_or_else(|| SqlError::no_procedure(&execute.name, execute.pos))?;
                crate::statement::check_call_arity(signature.inputs.len(), execute).map_err(
                    |e| match e {
                        // Said in the script with a space more than a
                        // statement's own.
                        Error::Sql(mut sql) if sql.native == 2129 => {
                            sql.message.push(' ');
                            self.ctx.fail(Error::Sql(sql))
                        }
                        e => self.ctx.fail(e),
                    },
                )?;
                // A subquery in an argument (`__new` is one) runs first.
                let mut call = execute.clone();
                for arg in &mut call.args {
                    if let Some(resolved) = self.rewrite_expr(arg, vars)? {
                        *arg = resolved;
                    }
                }
                let settings = self.ctx.settings_or_fail()?;
                let args = crate::statement::procedure_arguments(
                    self.ctx.session,
                    self.ctx.connection,
                    &settings,
                    &call,
                    &signature.inputs,
                    crate::statement::Procedures::Run {
                        statement: None,
                        params: &[],
                    },
                )
                .map_err(|e| self.ctx.fail(e))?;
                let written = crate::psm::call_from_trigger(
                    self.ctx.session,
                    self.ctx.connection,
                    &settings,
                    &execute.name,
                    execute.pos,
                    &args,
                )
                .map_err(|e| self.ctx.fail(e))?;
                // What it wrote counts as the trigger's.
                self.updated = self.updated.saturating_add(written);
            }
            // The results of a system procedure go nowhere, and a trigger has
            // no statement to build them for.
            Sql::Execute(execute)
                if crate::procedure::has_results(&execute.name)
                    && !crate::procedure::is_event(&execute.name) => {}
            Sql::Execute(execute) => {
                let args = crate::statement::arguments(
                    self.ctx.session,
                    self.ctx.connection,
                    execute,
                    &[],
                )
                .map_err(|e| self.ctx.fail(e))?;
                crate::procedure::run_from(
                    self.ctx.session,
                    self.ctx.connection,
                    self.ctx.connection,
                    execute,
                    &args,
                    crate::events::Context::Trigger,
                )
                .map_err(|e| self.ctx.fail(e))?;
            }
            Sql::SetVariable(set) => self.ctx.set_variable(set)?,
            // In a trigger's own transaction, a `ROLLBACK` is out of sequence
            // (with or without a transaction the trigger began); with `NO
            // TRANSACTION`, transactions work as anywhere.
            Sql::Transaction(tflip_sql::ast::Transaction::Rollback(None)) if self.transaction => {
                let error = crate::transaction::sql_error(Error::NotInTransaction);
                return Err(self.ctx.fail(error));
            }
            Sql::Transaction(transaction) => {
                crate::transaction::sql(self.ctx.session, self.ctx.connection, transaction)
                    .map_err(|e| self.ctx.fail(e))?;
            }
            _ => {
                let error = self.ctx.fail(Error::NotImplemented);
                return Err(error);
            }
        }
        Ok(())
    }
}

/// The triggers of `target`'s dictionary for `kind`; empty for a free table.
fn table_set(session: &Session, target: &crate::table::Table, kind: DmlKind) -> Set {
    match target.dictionary_name() {
        Some(name) => load(session, target.connection, &name, kind)
            .with_rowid_prefix(target.sql_info().rowid_prefix),
        None => Set::default(),
    }
}

/// The fields and values of a table-API row as a trigger reads it.
fn api_record(row: Option<Row>) -> Option<Record> {
    row.map(|(fields, values)| Record::new(fields, values))
}

/// Runs `change`, a table-API record's change and the triggers it fires,
/// as [`crate::dml`] runs a statement's: with real transactions, in a
/// transaction of its own that a failure takes back, unless the event has
/// a `NO TRANSACTION` trigger (as the original's server, probed,
/// `fixtures/dictionary/constraints.json`). `change` is told whether it runs
/// in one, and answers its result and whether it went through (a refused
/// delete returns success but takes its triggers' writes back).
fn api_record_change(
    session: &Session,
    connection: Handle,
    set: &Set,
    change: impl FnOnce(bool) -> (Result<(), Error>, bool),
) -> Result<(), Error> {
    let nested = match set.transactional() {
        true => crate::transaction::trigger_begins(session, connection)?,
        false => None,
    };
    let Some(nested) = nested else {
        // A `NO TRANSACTION` trigger runs in the caller's transaction as a
        // call (see `dml::Fire::record`).
        crate::transaction::call_begins(session, connection)?;
        let result = change(false).0;
        let ended = crate::transaction::call_ends(session, connection, result.is_ok());
        return result.and(ended);
    };
    let (result, ok) = change(true);
    let ended = crate::transaction::trigger_ends(session, nested, ok && result.is_ok());
    result.and(ended)
}

/// Writes `target`'s pending record through the table API (`AdsWriteRecord`)
/// as the original engine writes a dictionary's table (probed,
/// `tools/ace/dd/constraints.c`, `tools/ace/dd/rules.ops`): the dictionary's
/// constraints are checked, the BEFORE triggers fire, then its referential
/// integrity is weighed (7076 for a foreign key with no parent, 5141 for a
/// changed key a RESTRICT rule's children hold), the record is written and
/// the other rules re-point the children (a failure puts the record back),
/// and the AFTER triggers fire; or an INSTEAD OF trigger runs in its place and
/// the change is dropped. All in a transaction of the record's own (see
/// [`api_record_change`]).
pub fn write_record_api(session: &Session, target: &crate::table::Table) -> Result<(), Error> {
    target.check_pending()?;
    let kind = match target.pending_append() {
        Some(true) => DmlKind::Insert,
        Some(false) => DmlKind::Update,
        None => return target.write_record(),
    };
    let set = table_set(session, target, kind);
    let rules = target.ri();
    if set.is_empty() && rules.is_empty() {
        return target.write_record();
    }
    let pending = target.ri_pending_row()?;
    let stored = match kind {
        DmlKind::Update => target.stored_row()?,
        _ => None,
    };
    let new = api_record(pending.clone());
    let old = api_record(stored.clone());
    let settings = Settings::default();
    let change = |transaction: bool| {
        let at = Affected {
            new: new.as_ref(),
            old: old.as_ref(),
            recno: target.record_number(),
            transaction,
        };
        let result = (|| {
            if set.has_instead_of() {
                set.fire(
                    session,
                    target.connection,
                    &settings,
                    Timing::InsteadOf,
                    &at,
                )?;
                return target.cancel_update();
            }
            set.fire(session, target.connection, &settings, Timing::Before, &at)?;
            let Some((fields, values)) = &pending else {
                return target.write_record();
            };
            crate::ri::check_child(session, target.connection, rules, fields, values)?;
            let Some((_, old)) = &stored else {
                target.write_record()?;
                crate::transaction::insert_fires(
                    session,
                    target.connection,
                    target.shared(),
                    target.record_number(),
                );
                return set.fire(session, target.connection, &settings, Timing::After, &at);
            };
            let record = target.record_number();
            crate::ri::before_update(
                session,
                target.connection,
                rules,
                fields,
                old,
                values,
                record,
            )?;
            let slot = target.record_number();
            let bytes = target.stored_bytes()?;
            target.write_record()?;
            if let Err(error) =
                crate::ri::after_update(session, target.connection, rules, fields, old, values)
            {
                if let Some(bytes) = bytes {
                    let _ = crate::ri::restore(target, slot, bytes);
                }
                return Err(error);
            }
            set.fire(session, target.connection, &settings, Timing::After, &at)
        })();
        (result, true)
    };
    match set.is_empty() {
        true => change(false).0,
        false => api_record_change(session, target.connection, &set, change),
    }
}

/// Deletes `target`'s current record through the table API
/// (`AdsDeleteRecord`) as the original engine deletes a dictionary's record:
/// its referential integrity applies, and the triggers fire around the delete,
/// in a transaction of the record's own (see [`api_record_change`]). A BEFORE
/// trigger that fails leaves the record and returns success, as a refusing
/// rule does, its writes taken back (probed, `tools/ace/dd/constraints.c`);
/// an INSTEAD OF trigger runs in place of the delete.
pub fn delete_record_api(session: &Session, target: &crate::table::Table) -> Result<(), Error> {
    let set = table_set(session, target, DmlKind::Delete);
    let triggered = !set.is_empty();
    // The old key is read, then the row is marked deleted — which first
    // checks the delete right, so access rights are refused before RI is
    // weighed (as the original orders them).
    let row = match !target.ri().is_empty() || triggered {
        true => target.ri_current_row()?,
        false => None,
    };
    let old = api_record(row.clone());
    let settings = Settings::default();
    let change = |transaction: bool| -> (Result<(), Error>, bool) {
        let at = Affected {
            new: None,
            old: old.as_ref(),
            recno: target.record_number(),
            transaction,
        };
        if let Err(error) = target.delete_record() {
            return (Err(error), false);
        }
        if triggered {
            if set.has_instead_of() {
                if let Err(error) = target.recall_record() {
                    return (Err(error), false);
                }
                let fired = set.fire(
                    session,
                    target.connection,
                    &settings,
                    Timing::InsteadOf,
                    &at,
                );
                return (fired, true);
            }
            if set
                .fire(session, target.connection, &settings, Timing::Before, &at)
                .is_err()
            {
                return (target.recall_record(), false);
            }
        }
        // A rule then applies to the parent row's children. A refusal — a
        // RESTRICT rule's, or a child's constraint a SET NULL or SET DEFAULT
        // breaks — undoes the mark and returns success: the original leaves
        // the row and returns success through the table API (where SQL
        // surfaces the error; `tools/ace/dd/rules.ops`).
        if let Some((fields, values)) = row.clone().filter(|_| !target.ri().is_empty())
            && crate::ri::on_delete(
                session,
                target.connection,
                target.ri(),
                &fields,
                &values,
                target.record_number(),
            )
            .is_err()
        {
            return (target.recall_record(), false);
        }
        if triggered && !set.after.is_empty() {
            let result = target
                .write_record()
                .and_then(|()| set.fire(session, target.connection, &settings, Timing::After, &at));
            return (result, true);
        }
        (Ok(()), true)
    };
    match triggered {
        true => api_record_change(session, target.connection, &set, change),
        false => change(false).0,
    }
}

/// `CREATE TRIGGER` on `connection`'s dictionary, as the original engine
/// checks it (`fixtures/sql/triggers.json`): a `CONFLICT` trigger only on
/// `UPDATE` or `DELETE` (2215), one of each a table, and a `BEFORE` and an
/// `INSTEAD OF` trigger not on the same event (5181).
pub fn create(
    session: &Session,
    connection: Handle,
    create: &tflip_sql::ast::CreateTrigger,
) -> Result<(), Error> {
    if create.timing == 8 && create.event == 1 {
        return Err(Error::Sql(SqlError {
            state: "HY000".to_owned(),
            native: 2215,
            message: format!(
                "Invalid create options:  trigger {} -- CONFLICT ON INSERT trigger is not \
                 suppported.",
                create.name.name
            ),
        }));
    }
    let owner = connection::get(session, connection)?;
    if !owner.dictionary {
        return Err(Error::Sql(SqlError::no_dictionary()));
    }
    let (container_type, container, function) = containers(create);
    // A database trigger: on the database, not a table.
    if create.event >= DATABASE_EVENT {
        let (name, timings) = match create.event {
            4 => ("OPEN_TABLE", BEFORE | AFTER),
            5 => ("CLOSE_TABLE", BEFORE | AFTER),
            6 => ("CONNECT", AFTER),
            _ => ("DISCONNECT", BEFORE),
        };
        if create.timing & timings == 0 {
            return Err(Error::Sql(SqlError::ace(
                5019,
                "An invalid option was specified.",
                &format!("Unsupported trigger type for {name} event."),
            )));
        }
        if !rights::may(&owner, DATABASE, "", rights::ALTER) {
            return Err(rights::refuse(""));
        }
        let name_taken = owner
            .with_database_opt(|db| {
                let dict = db.dictionary();
                let database = dict.database()?;
                dict.child(database.id, Kind::Trigger, &create.name.name)
                    .map(|_| ())
            })
            .flatten()
            .is_some();
        if create.alter {
            return match name_taken {
                true => alter(session, connection, create, ""),
                false => Err(missing_trigger()),
            };
        }
        if name_taken {
            return Err(Error::Sql(SqlError::ace(
                5132,
                "The specified database object name is not valid.",
                "A trigger with the same name already exists in the dictionary.",
            )));
        }
        return crate::dictionary_write::add_trigger_api(
            session,
            connection,
            &create.name.name,
            "",
            create.timing,
            create.event,
            container_type,
            container,
            function,
            create.priority,
            "",
            create.options,
        );
    }
    let known = owner
        .with_database_opt(|db| {
            db.dictionary()
                .named(Kind::Table, &create.table.name)
                .is_some()
        })
        .unwrap_or(false);
    if !known {
        return Err(Error::Sql(SqlError::ace(
            5132,
            "The specified database object name is not valid.",
            "The table or view name is not in the data dictionary.",
        )));
    }
    rights::require_table(session, connection, &create.table.name, rights::ALTER, "")?;
    if create.alter {
        let there = owner
            .with_database_opt(|db| {
                let dict = db.dictionary();
                let table = dict.named(Kind::Table, &create.table.name)?;
                dict.child(table.id, Kind::Trigger, &create.name.name)
                    .map(|_| ())
            })
            .flatten()
            .is_some();
        return match there {
            true => alter(session, connection, create, &create.table.name),
            false => Err(missing_trigger()),
        };
    }
    let clash = owner
        .with_database_opt(|db| {
            let dict = db.dictionary();
            let table = dict.named(Kind::Table, &create.table.name)?;
            dict.all(Kind::Trigger)
                .into_iter()
                .filter(|t| t.parent == table.id)
                .filter(|t| t.u32(trigger::EVENT_TYPE) == Some(create.event))
                .filter_map(|t| t.u32(trigger::TRIGGER_TYPE))
                .find_map(|existing| match (existing, create.timing) {
                    (8, 8) => Some("A CONFLICT ON trigger of this type already exists."),
                    (1, 2) | (2, 1) => Some(
                        "Either a BEFORE trigger already exists, and you are attempting to \
                         create an INSTEAD OF trigger, or vice versa.",
                    ),
                    _ => None,
                })
        })
        .flatten();
    if let Some(why) = clash {
        return Err(Error::Sql(SqlError::ace(
            5181,
            "Trigger creation failed.",
            why,
        )));
    }
    crate::dictionary_write::add_trigger_api(
        session,
        connection,
        &create.name.name,
        &create.table.name,
        create.timing,
        create.event,
        container_type,
        container,
        function,
        create.priority,
        "",
        create.options,
    )
}

/// What the dictionary keeps of the container of a trigger: its type
/// (`ADS_TRIGGER_WIN32DLL`, `COM` for an assembly, or the script), the
/// script or the library, and the function.
fn containers(create: &tflip_sql::ast::CreateTrigger) -> (u32, &str, &str) {
    match &create.library {
        Some(library) => (
            if library.assembly { 2 } else { WIN32_DLL },
            &library.library,
            &library.function,
        ),
        None => (SQL_SCRIPT, &create.body, ""),
    }
}

/// `ALTER TRIGGER` of the trigger `create` describes, on `table` (the database
/// if empty).
fn alter(
    session: &Session,
    connection: Handle,
    create: &tflip_sql::ast::CreateTrigger,
    table: &str,
) -> Result<(), Error> {
    let (container_type, container, function) = containers(create);
    crate::dictionary_write::alter_trigger(
        session,
        connection,
        &create.name.name,
        table,
        create.timing,
        create.event,
        container_type,
        container,
        function,
        create.priority,
        create.options,
    )
}

/// The refusal of an `ALTER TRIGGER` of a trigger that is not there.
fn missing_trigger() -> Error {
    Error::Sql(SqlError::ace(
        5132,
        "The specified database object name is not valid.",
        "The trigger does not exist.",
    ))
}

/// `DROP TRIGGER table.name`: the trigger of that name on that table. One
/// not named with its table is not found, as in the original engine.
pub fn drop(
    session: &Session,
    connection: Handle,
    table: Option<&tflip_sql::ast::Named>,
    name: &tflip_sql::ast::Named,
) -> Result<(), Error> {
    let owner = connection::get(session, connection)?;
    if !owner.dictionary {
        return Err(Error::Sql(SqlError::no_dictionary()));
    }
    // A name with no table is a database trigger's.
    let found = owner
        .with_database_opt(|db| {
            let dict = db.dictionary();
            let parent = match table {
                Some(table) => dict.named(Kind::Table, &table.name)?.id,
                None => dict.database()?.id,
            };
            dict.child(parent, Kind::Trigger, &name.name)?;
            Some(parent)
        })
        .flatten();
    let Some(parent) = found else {
        return Err(Error::Sql(SqlError::ace(
            5132,
            "The specified database object name is not valid.",
            "The trigger is not part of the database.",
        )));
    };
    if let Some(table) = table {
        rights::require_table(session, connection, &table.name, rights::ALTER, "")?;
    }
    crate::dictionary_write::remove_trigger(session, connection, Some(parent), &name.name)
}

// The per-thread registry of pseudo-tables (`__new`, `__old`) a trigger body
// names in `FROM`.
thread_local! {
    static PSEUDO: RefCell<HashMap<String, Built>> =
        RefCell::new(HashMap::new());
}

/// The pseudo-table `name` (`__new`, `__old`), if one is registered for the
/// running trigger.
pub(crate) fn lookup(name: &str) -> Option<Built> {
    PSEUDO.with(|m| m.borrow().get(&name.to_ascii_lowercase()).cloned())
}

/// Registers `entries` as pseudo-tables, restoring what they shadowed when the
/// returned guard drops (so nested trigger firings nest).
pub(crate) fn enter(entries: Vec<(String, Built)>) -> Scope {
    let saved = PSEUDO.with(|m| {
        let mut map = m.borrow_mut();
        entries
            .into_iter()
            .map(|(name, table)| {
                let key = name.to_ascii_lowercase();
                let previous = map.insert(key.clone(), table);
                (key, previous)
            })
            .collect()
    });
    Scope { saved }
}

/// Replaces the pseudo-table `name`, which the running trigger registered.
fn replace(name: &str, table: Built) {
    PSEUDO.with(|m| m.borrow_mut().insert(name.to_ascii_lowercase(), table));
}

pub(crate) struct Scope {
    saved: Vec<(String, Option<Built>)>,
}

impl Drop for Scope {
    fn drop(&mut self) {
        PSEUDO.with(|m| {
            let mut map = m.borrow_mut();
            for (key, previous) in self.saved.drain(..) {
                match previous {
                    Some(table) => map.insert(key, table),
                    None => map.remove(&key),
                };
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use tflip_storage::value::Value;

    use crate::handle::Handle;
    use crate::script_host::tests::{open as open_psm, query};
    use crate::{Session, statement};

    /// A fresh copy of the `psm` fixture, with one connection and statement.
    fn open(name: &str) -> (Session, Handle) {
        open_psm("trig", name)
    }

    fn exec(session: &Session, stmt: Handle, sql: &str) {
        statement::execute_direct(session, stmt, sql, false).ok();
    }

    /// The first text column of every row of `sql` (a live cursor scanned in
    /// record order).
    fn column(session: &Session, stmt: Handle, sql: &str) -> Vec<String> {
        let (_, rows) = query(session, stmt, sql);
        rows.into_iter()
            .map(|row| match &row[0] {
                Value::Text(s) => s.trim_end().to_owned(),
                Value::Integer(n) => n.to_string(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn insert_fires_before_and_after_triggers() {
        // Matches `adsdump`: BEFORE INSERT writes "BI <name> rec <recno>",
        // AFTER INSERT writes "AI <name> <padded id>" (via the util.pad UDF).
        let (session, stmt) = open("insert");
        exec(
            &session,
            stmt,
            "INSERT INTO items (id, name, qty) VALUES (10, 'fig', 3)",
        );
        let audit = column(&session, stmt, "SELECT msg FROM audit");
        assert_eq!(&audit[6..], ["BI fig rec 3", "AI fig 0010"]);
        assert_eq!(
            column(&session, stmt, "SELECT id FROM items"),
            ["1", "2", "10"]
        );
    }

    #[test]
    fn update_fires_the_after_trigger() {
        let (session, stmt) = open("update");
        exec(
            &session,
            stmt,
            "INSERT INTO items (id, name, qty) VALUES (10, 'fig', 3)",
        );
        exec(
            &session,
            stmt,
            "UPDATE items SET qty = qty + 1 WHERE id = 10",
        );
        let audit = column(&session, stmt, "SELECT msg FROM audit");
        assert_eq!(audit.last().unwrap(), "AU 3->4");
    }

    #[test]
    fn delete_runs_the_instead_of_trigger() {
        // INSTEAD OF DELETE updates qty to 0 (firing AFTER UPDATE) and keeps
        // the row, as `adsdump` shows.
        let (session, stmt) = open("delete");
        exec(
            &session,
            stmt,
            "INSERT INTO items (id, name, qty) VALUES (10, 'fig', 3)",
        );
        exec(
            &session,
            stmt,
            "UPDATE items SET qty = qty + 1 WHERE id = 10",
        );
        exec(&session, stmt, "DELETE FROM items WHERE id = 10");
        let audit = column(&session, stmt, "SELECT msg FROM audit");
        assert_eq!(audit.last().unwrap(), "AU 4->0");
        // The row is still there, with qty 0 (the delete was replaced).
        assert_eq!(
            column(&session, stmt, "SELECT id FROM items"),
            ["1", "2", "10"]
        );
    }

    /// Without real transactions (the original local server's mode), as the
    /// original local server does (probed with `tools/ace/sql/triggers.txt`
    /// locally): a failing trigger keeps what it wrote, with or without `NO
    /// TRANSACTION`; the record a failing AFTER INSERT trigger fired on goes
    /// all the same; a failing AFTER UPDATE keeps the change; a `ROLLBACK`
    /// in a trigger is accepted; and the count a trigger reads is 0.
    #[test]
    fn without_transactions_a_failing_trigger_keeps_its_writes() {
        let (session, stmt) = open("local");
        crate::settings::set_real_transactions(&session, false);
        for name in [
            "items_bi",
            "items_ai",
            "items_au",
            "items_iod",
            "items_conflict",
        ] {
            exec(&session, stmt, &format!("DROP TRIGGER items.{name}"));
        }
        exec(&session, stmt, "DELETE FROM audit");
        exec(
            &session,
            stmt,
            "CREATE TRIGGER t1 ON items AFTER INSERT BEGIN INSERT INTO audit (msg) VALUES \
             ('t1 ' + CAST(::conn.TransactionCount AS SQL_CHAR)); IF __new.id = 10 THEN \
             RAISE boom(1, 'x'); END IF; END",
        );
        assert!(
            statement::execute_direct(
                &session,
                stmt,
                "INSERT INTO items VALUES (10, 'fig', 3, 'm')",
                false
            )
            .is_err()
        );
        exec(
            &session,
            stmt,
            "INSERT INTO items VALUES (11, 'kiwi', 3, 'm')",
        );
        let sorted = |mut v: Vec<String>| {
            v.sort();
            v
        };
        assert_eq!(
            sorted(column(&session, stmt, "SELECT id FROM items")),
            ["1", "11", "2"]
        );
        assert_eq!(
            sorted(column(&session, stmt, "SELECT msg FROM audit")),
            ["t1 0", "t1 0"]
        );
        exec(&session, stmt, "DROP TRIGGER items.t1");
        exec(
            &session,
            stmt,
            "CREATE TRIGGER t2 ON items AFTER UPDATE BEGIN INSERT INTO audit (msg) VALUES \
             ('t2'); ROLLBACK WORK; RAISE boom(2, 'y'); END",
        );
        assert!(
            statement::execute_direct(
                &session,
                stmt,
                "UPDATE items SET qty = 7 WHERE id = 11",
                false
            )
            .is_err()
        );
        // (The live cursor scans every record: apple 6, pear 0, kiwi now 7.)
        assert_eq!(
            sorted(column(&session, stmt, "SELECT qty FROM items")),
            ["0", "6", "7"]
        );
        assert_eq!(
            sorted(column(&session, stmt, "SELECT msg FROM audit")),
            ["t1 0", "t1 0", "t2"]
        );
    }
}
