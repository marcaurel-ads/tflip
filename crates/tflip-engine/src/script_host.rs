//! What the hosts of a script ([`sent`](crate::sent), [`psm`](crate::psm),
//! [`trigger`](crate::trigger) and [`udf`](crate::udf)) have in common: the
//! session and connection their statements run on, how an engine error
//! becomes the SQL error the interpreter carries, and the parts of a
//! statement the interpreter hands back that every host runs the same way.
//! [`tflip_sql::script::Host`] cannot have them as default methods, as the
//! SQL crate knows nothing of a [`Session`].

use std::borrow::Cow;

use tflip_sql::ast::{Execute, Expr, From, Select, SetVariable, Statement as Sql};
use tflip_sql::script::{self, Vars};
use tflip_sql::{FunctionSig, SqlError};

use crate::handle::Handle;
use crate::statement::{self, Settings};
use crate::sysvars::{Scope, Trigger};
use crate::{Error, Session};

/// Where a host's statements take their table settings from.
pub(crate) enum SettingsFrom<'a> {
    /// The settings the host was given.
    Given(&'a Settings),
    /// Those of the statement, read again for each statement the script
    /// runs, as it may change them as it goes.
    Statement(Handle),
}

/// The part of a script host that is the same in all of them.
pub(crate) struct ScriptCtx<'a> {
    pub session: &'a Session,
    pub connection: Handle,
    settings: SettingsFrom<'a>,
    /// Whether what fails in an expression says where it is in the script.
    located: bool,
    /// Whether an error is the one the SQL of a client would get (a refusal
    /// of rights is the AQE error 7200), and one that is no SQL error is not
    /// kept, as for a function.
    sql_errors: bool,
    /// An engine error underneath a statement, which is not an SQL error.
    pub failure: Option<Error>,
    /// The cursors of the script that read their tables live, by the number
    /// [`script::Rows::Live`] gives; a closed one is gone.
    live: Vec<Option<statement::LiveCursor>>,
    /// What the query log has of the statements the script runs.
    log: Log,
}

/// Which row of the script gets an end time when it ends (probed,
/// `fixtures/sql/spddql4.json`).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Ends {
    /// None: a trigger's or a free script's last statement does not, apart
    /// from a `SELECT`, whose cursor closes.
    Select,
    /// The last statement a procedure ran.
    Last,
    /// None.
    Never,
}

/// The query log's view of a script host's statements.
struct Log {
    ends: Ends,
    /// The text of a procedure whose body is one statement, which is logged
    /// as it is written (spaces and `;` included) and as optimized as the
    /// statement alone.
    lone: Option<String>,
    /// Whether the statement that began has a row to end.
    begun: bool,
    /// Whether it calls a procedure, whose failure has the rows written so far.
    calls: bool,
    /// Whether it is a `SELECT`.
    select: bool,
    /// The row of the statement that ended last.
    last: Option<crate::query_log::End>,
}

impl<'a> ScriptCtx<'a> {
    pub fn new(session: &'a Session, connection: Handle, settings: SettingsFrom<'a>) -> Self {
        ScriptCtx {
            session,
            connection,
            settings,
            located: true,
            sql_errors: false,
            failure: None,
            live: Vec::new(),
            log: Log {
                ends: Ends::Select,
                lone: None,
                begun: false,
                calls: false,
                select: false,
                last: None,
            },
        }
    }

    /// The context of a script that puts an end time in the row of `ends`.
    pub fn ending(mut self, ends: Ends) -> Self {
        self.log.ends = ends;
        self
    }

    /// The context of a procedure whose body, `text`, is one statement.
    pub fn lone(mut self, text: String) -> Self {
        self.log.lone = Some(text);
        self
    }

    /// The statement `statement`, written as `text`, begins: it is a row of
    /// the query log. A `SELECT` that is not the last statement is only
    /// checked, not run.
    pub fn log_begins(&mut self, text: &str, statement: &Sql, last: bool) {
        self.log.begun = false;
        self.log.last = None;
        self.log.select = matches!(statement, Sql::Select(_));
        self.log.calls = matches!(statement, Sql::Execute(_));
        if self.log.select && !last {
            return;
        }
        let (text, optimized) = match (&self.log.lone, statement) {
            (None, _) => (text, false),
            (Some(body), Sql::Select(select)) => {
                let optimized = self.settings().ok().and_then(|settings| {
                    statement::plan_optimized(
                        self.session,
                        self.connection,
                        &settings,
                        select,
                        body,
                        &[],
                    )
                });
                (body.as_str(), optimized.unwrap_or(true))
            }
            (Some(body), _) => (body.as_str(), true),
        };
        self.log.begun =
            crate::query_log::inner_begin(self.session, self.connection, text, optimized);
    }

    /// The statement that began has ended with `result`, the script having
    /// changed `affected` rows so far (`u32::MAX` for none).
    pub fn log_ends(&mut self, result: Result<(), &SqlError>, affected: u32) {
        if !std::mem::take(&mut self.log.begun) {
            return;
        }
        let error = result.err().map(|error| {
            let kept = self.failure.clone();
            kept.unwrap_or_else(|| Error::Sql(error.clone()))
        });
        let affected = match (&error, self.log.calls) {
            (Some(_), false) => u32::MAX,
            _ => affected,
        };
        let end = crate::query_log::inner_end(
            self.session,
            self.connection,
            error.as_ref().map_or(Ok(()), Err),
            affected,
            self.log.calls,
        );
        let ends = match self.log.ends {
            Ends::Last => true,
            Ends::Select => self.log.select,
            Ends::Never => false,
        };
        self.log.last = end.filter(|_| ends);
    }

    /// The script opens a cursor.
    pub fn log_opens(&mut self, declared: bool) {
        self.log.last = None;
        crate::query_log::inner_note(
            self.session,
            self.connection,
            "Executing cursor OPEN statment in SQL script.",
            declared,
        );
    }

    /// The script has ended; `last` if the last thing it ran was a statement.
    pub fn log_script_ends(&mut self, last: bool) {
        if let Some(end) = self.log.last.take()
            && last
        {
            crate::query_log::ended(self.session, self.connection, end);
        }
    }

    /// The context of a user-defined function, whose errors are not located
    /// and are those of SQL.
    pub fn function(mut self) -> Self {
        self.located = false;
        self.sql_errors = true;
        self
    }

    /// The settings the statements run with.
    pub fn settings(&self) -> Result<Cow<'a, Settings>, Error> {
        match &self.settings {
            SettingsFrom::Given(settings) => Ok(Cow::Borrowed(settings)),
            SettingsFrom::Statement(handle) => {
                statement::settings_of(self.session, *handle).map(Cow::Owned)
            }
        }
    }

    /// [`ScriptCtx::settings`], its failure the SQL error the interpreter
    /// carries.
    pub fn settings_or_fail(&mut self) -> Result<Cow<'a, Settings>, SqlError> {
        self.settings().map_err(|e| self.fail(e))
    }

    /// Records `error` and returns the SQL error the interpreter carries in
    /// its place (the real one is taken from [`ScriptCtx::failure`]
    /// afterwards).
    pub fn fail(&mut self, error: Error) -> SqlError {
        let error = match self.sql_errors {
            true => statement::rights_sql_error(error),
            false => error,
        };
        match error {
            Error::Sql(e) => e,
            other => {
                if !self.sql_errors {
                    self.failure = Some(other);
                }
                statement::read_failed()
            }
        }
    }

    /// The user-defined functions the connection's dictionary has.
    pub fn functions(&self) -> Vec<FunctionSig> {
        crate::udf::catalog(self.session, self.connection)
    }

    /// What the statements of the script read of themselves: the rows they
    /// changed so far (`updated`), and the `trigger` they run in.
    pub fn scope<'s>(
        &'s self,
        statement: Option<Handle>,
        updated: u32,
        trigger: Option<&'s Trigger>,
    ) -> Scope<'s> {
        Scope {
            session: self.session,
            connection: self.connection,
            statement,
            updated,
            trigger,
        }
    }

    /// `expr` with what `scope` knows put in it and its subqueries run.
    pub fn rewrite_expr(
        &self,
        scope: &Scope<'_>,
        expr: &Expr,
        vars: &Vars,
    ) -> Result<Option<Expr>, SqlError> {
        let expr = scope.expr(expr);
        let settings = self.settings().map_err(|e| match e {
            Error::Sql(e) => e,
            _ => statement::read_failed(),
        })?;
        let run = |select: &Select| {
            statement::select_rows(self.session, self.connection, &settings, select)
        };
        Ok(Some(
            crate::subquery::resolve(&expr, vars, self.located, &run)?.unwrap_or(expr),
        ))
    }

    /// Runs the `INSERT`, `UPDATE`, `DELETE` or `MERGE` `statement`, and
    /// returns how many rows it changed.
    pub fn dml(&mut self, statement: &Sql) -> Result<u32, SqlError> {
        let settings = self.settings_or_fail()?;
        self.count_statement();
        // The tables the statement opened close as it ends, not with the
        // procedure (probed, `tools/ace/sql/trigdblog.txt`).
        let (ran, opened, _) = crate::trigger::collecting(Vec::new(), || {
            statement::run_dml(self.session, self.connection, &settings, statement)
        });
        crate::trigger::close_all(self.session, opened);
        ran.map_err(|e| self.fail(e))
    }

    /// The error of a statement that fired a trigger which failed: as that
    /// trigger's failure, it is the statement's own error, which the script
    /// locates.
    pub fn nested_failure(&mut self, error: SqlError) -> SqlError {
        match self.failure.take() {
            Some(Error::Constraint { code, text })
                if code == crate::constraints::TRIGGER_FAILED =>
            {
                SqlError::ace(code, &text, "")
            }
            failure => {
                self.failure = failure;
                error
            }
        }
    }

    /// Runs `SET ::scope.name = value`.
    pub fn set_variable(&mut self, set: &SetVariable) -> Result<(), SqlError> {
        statement::set_variable(self.session, self.connection, set, &[]).map_err(|e| self.fail(e))
    }

    /// Checks a `SELECT` that has no result to return.
    pub fn check_select(&mut self, select: &Select) -> Result<(), SqlError> {
        statement::check_select(self.session, self.connection, select, 0).map_err(|e| self.fail(e))
    }

    /// `select` with the subqueries in it that read nothing of its table's
    /// record run now, as the original runs them when it opens a cursor.
    fn resolve_subqueries(&mut self, select: &Select) -> Result<Select, SqlError> {
        let mut resolved = select.clone();
        let tflip_sql::ast::From::Table(table) = &select.from else {
            return Ok(resolved);
        };
        let outer = [table.name.as_str(), table.alias.as_deref().unwrap_or("")];
        let settings = self.settings_or_fail()?;
        let run = |select: &Select| {
            statement::select_rows(self.session, self.connection, &settings, select)
        };
        let wanted = |select: &Select| !crate::subquery::reads_outer(select, &outer);
        let (located, vars) = (self.located, Vars::new());
        if let Some(filter) = &select.filter
            && let Some(filter) =
                crate::subquery::resolve_where(filter, &vars, located, &run, &wanted)?
        {
            resolved.filter = Some(filter);
        }
        Ok(resolved)
    }

    /// The statement the script runs counts for its connection
    /// (`StmtCount` of a database trigger's `__info`).
    fn count_statement(&self) {
        if let Ok(owner) = crate::connection::get(self.session, self.connection) {
            owner
                .statements
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// The rows of the cursor on `select`, which the script reads.
    pub fn query(&mut self, select: &Select) -> Result<script::Query, SqlError> {
        let settings = self.settings_or_fail()?;
        if !select.unions.is_empty() && !select.order.is_empty() {
            return Err(SqlError::union_order_in_cursor());
        }
        self.count_statement();
        let resolved = self.resolve_subqueries(select)?;
        let live = statement::LiveCursor::open(self.session, self.connection, &settings, &resolved)
            .map_err(|e| self.fail(e))?;
        if let Some(cursor) = live {
            let columns = cursor.columns();
            self.live.push(Some(cursor));
            return Ok(script::Query {
                columns,
                rows: script::Rows::Live(self.live.len() - 1),
            });
        }
        let (columns, rows) =
            statement::select_values(self.session, self.connection, &settings, select)
                .map_err(|e| self.fail(e))?;
        Ok(script::Query {
            columns,
            rows: script::Rows::Fixed(rows),
        })
    }

    /// The next row of the live cursor `cursor`.
    pub fn fetch(&mut self, cursor: usize) -> Result<Option<Vec<tflip_sql::V>>, SqlError> {
        let Some(Some(live)) = self.live.get_mut(cursor) else {
            return Ok(None);
        };
        live.fetch().map_err(|e| self.fail(e))
    }

    /// The script closes the live cursor `cursor`.
    pub fn close(&mut self, cursor: usize) {
        if let Some(live) = self.live.get_mut(cursor) {
            *live = None;
        }
    }

    /// The rows of the cursor on the procedure `execute`, which the script
    /// reads. The original refuses a procedure with no output or a varying
    /// one at the `EXECUTE`, before it runs anything.
    pub fn procedure_query(&mut self, execute: &Execute) -> Result<script::Query, SqlError> {
        let name = &execute.name;
        let reason = match crate::procedure::is_builtin(name) {
            true => (!crate::procedure::has_results(name)).then_some("with no output"),
            false => {
                let signature = crate::psm::signature(self.session, self.connection, name)
                    .map_err(|e| self.fail(e))?
                    .ok_or_else(|| SqlError::no_procedure(name, execute.pos))?;
                match (signature.selectable, signature.varying) {
                    (true, _) => {
                        // Its arguments are counted as the cursor opens.
                        match (signature.inputs.len(), execute.args.len()) {
                            (1.., 0) => {
                                return Err(SqlError::cursor_parameter_missing(execute.pos));
                            }
                            (wanted, given) if wanted != given => {
                                return Err(SqlError::cursor_procedure_arguments());
                            }
                            _ => None,
                        }
                    }
                    (false, true) => Some("with varying output"),
                    (false, false) => Some("with no output"),
                }
            }
        };
        if let Some(reason) = reason {
            return Err(SqlError::declaration(
                &format!("Cannot declare cursor using procedure {reason}: {name}"),
                execute.keyword,
            ));
        }
        let Ok(parsed) = tflip_sql::parse::parse("SELECT * FROM (EXECUTE PROCEDURE p()) c") else {
            unreachable!("a fixed SELECT");
        };
        let Sql::Select(mut select) = parsed.statement else {
            unreachable!("a fixed SELECT");
        };
        if let From::Procedure { execute: call, .. } = &mut select.from {
            *call = execute.clone();
        }
        self.query(&select)
    }
}

/// What the tests of the hosts share: a copy of the `psm` fixture to run
/// SQL on, and the rows of a cursor.
#[cfg(test)]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};

    use tflip_storage::value::Value;

    use crate::filter::SqlRow;
    use crate::handle::Handle;
    use crate::{Session, connection, statement, table};

    /// A fresh copy of the `psm` dictionary and its tables in a directory of
    /// its own (`prefix` and `name` keep concurrent tests apart), so that
    /// running what writes never touches the fixture.
    pub fn scratch(prefix: &str, name: &str) -> PathBuf {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt/psm");
        let dir = std::env::temp_dir().join(format!("tflip-{prefix}-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for entry in std::fs::read_dir(&src).unwrap() {
            let entry = entry.unwrap();
            if entry.path().extension().is_some_and(|e| e != "json") {
                std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
            }
        }
        dir
    }

    /// A session with a connection to the `psm` dictionary in `dir`, and a
    /// statement on it.
    pub fn connect(dir: &Path) -> (Session, Handle, Handle) {
        let session = Session::new();
        let cs = format!(
            "Data Source={};User ID=ADSSYS;Password=adssyspw;",
            dir.join("psm.add").display()
        );
        let connection = connection::connect(&session, &cs).unwrap();
        let statement = statement::create(&session, connection).unwrap();
        (session, connection, statement)
    }

    /// [`scratch`] and [`connect`]: a session and a statement.
    pub fn open(prefix: &str, name: &str) -> (Session, Handle) {
        let (session, _, statement) = connect(&scratch(prefix, name));
        (session, statement)
    }

    /// The column names and the rows (each field as a [`Value`]) of a cursor,
    /// scanned in record order.
    pub fn cursor_rows(session: &Session, cursor: Handle) -> (Vec<String>, Vec<Vec<Value>>) {
        use tflip_sql::Row;
        let table = table::get(session, cursor).unwrap();
        let fields = table.fields().to_vec();
        let names = fields.iter().map(|f| f.name.clone()).collect();
        let mut rows = Vec::new();
        table
            .scan(|_, bytes, memo| {
                let mut row = SqlRow {
                    fields: &fields,
                    bytes,
                    memo,
                    record: None,
                    hidden: &[],
                };
                let values = (0..fields.len()).map(|i| row.value(i).unwrap()).collect();
                rows.push(values);
                Ok(())
            })
            .unwrap();
        (names, rows)
    }

    /// Runs `sql` and returns its columns and rows, closing the cursor after.
    pub fn query(session: &Session, stmt: Handle, sql: &str) -> (Vec<String>, Vec<Vec<Value>>) {
        let cursor = statement::execute_direct(session, stmt, sql, true)
            .unwrap()
            .unwrap();
        let out = cursor_rows(session, cursor);
        table::close(session, cursor).unwrap();
        out
    }
}
