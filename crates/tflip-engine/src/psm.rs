//! Running a dictionary's SQL stored procedures (`EXECUTE PROCEDURE name`).
//!
//! A script stored procedure's body is SQL/PSM (`tflip_sql::script`): the
//! control flow and variables the interpreter runs, and the SQL statements it
//! hands back here to run on the connection's tables. The procedure's
//! parameters start as variables the body reads as `_name`; its results are
//! the rows the body writes to the pseudo-table `__output` (its columns are
//! the output parameters), or, for a `CURSOR VARYING` procedure, the result
//! set of the `SELECT` the body runs.
//!
//! The input parameters are the one row of the pseudo-table `__input` too
//! (`fixtures/sql/procvars.json`), which the statements of the body read as
//! they read a table. A failing body is the ACE error 5154, with what failed
//! in it. An external procedure (an `.aep`, invoke option without the
//! script bit) is its library's function, which [`crate::aep`] calls.

use tflip_sql::ast::{Insert, InsertSource, Statement as Sql};
use tflip_sql::script::{self, Vars};
use tflip_sql::{Compiler, Env, NoRow, SqlError, Ty, V, eval};
use tflip_storage::build::Column;
use tflip_storage::dd::{Kind, Object, procedure};
use tflip_storage::field::Field;
use tflip_storage::value::Value;

use crate::handle::Handle;
use crate::script_host::{Ends, ScriptCtx, SettingsFrom};
use crate::{Error, Session, connection, statement};

/// The invoke-option bit a script procedure has (as against an external one).
const SCRIPT: u32 = 0x4;
/// The invoke-option bit a `CURSOR VARYING` procedure has: its result is the
/// `SELECT` its body runs, not rows written to `__output`.
const CURSOR_VARYING: u32 = 0x1000;

/// Whether the procedure `name` is one of the connection's dictionary's.
pub fn exists(session: &Session, connection: Handle, name: &str) -> Result<bool, Error> {
    Ok(find(session, connection, name)?.is_some())
}

/// The connection's dictionary's procedure `name`, if there is one.
fn find(session: &Session, connection: Handle, name: &str) -> Result<Option<Object>, Error> {
    let owner = connection::get(session, connection)?;
    Ok(owner
        .with_database_opt(|db| db.dictionary().named(Kind::Procedure, name).cloned())
        .flatten())
}

/// The rows the statements of a procedure and of the procedures it calls
/// have written so far (`::stmt.UpdateCount`): `None` before one has.
#[derive(Default)]
pub(crate) struct Counts {
    updated: Option<u32>,
}

impl Counts {
    fn add(&mut self, rows: u32) {
        self.updated = Some(self.updated.unwrap_or(0).saturating_add(rows));
    }

    /// The rows the statement affected: none at all if nothing was written.
    fn affected(&self) -> u32 {
        self.updated.unwrap_or(u32::MAX)
    }
}

/// Runs the stored procedure `name` with `args` on `connection`, owned by the
/// statement `handle`, and returns its result cursor (none without
/// `want_cursor`) and the rows it wrote (`u32::MAX` if it wrote none).
pub fn run(
    session: &Session,
    handle: Handle,
    connection: Handle,
    name: &str,
    pos: usize,
    args: &[V],
    want_cursor: bool,
) -> Result<(Option<Handle>, u32), Error> {
    let settings = statement::settings_of(session, handle)?;
    let mut counts = Counts::default();
    let done = execute(
        session,
        handle,
        connection,
        &settings,
        name,
        pos,
        args,
        want_cursor,
        &mut counts,
        false,
    )?;
    let affected = counts.affected();
    if !want_cursor {
        return Ok((None, affected));
    }
    if done.varying {
        // A procedure with no output and no `CURSOR VARYING` runs its last
        // `SELECT` but has no cursor to answer (`fixtures/sql/spddql3.json`).
        if !done.answers
            && let Some(cursor) = done.cursor
        {
            let _ = crate::table::close(session, cursor);
            return Ok((None, affected));
        }
        return Ok((done.cursor, affected));
    }
    statement::build_cursor(session, handle, connection, done.columns, done.rows)
        .map(|cursor| (Some(cursor), affected))
}

/// Runs the procedure `name` that a trigger's body calls, whose output goes
/// nowhere, and returns the rows it wrote. What fails in it fails the trigger
/// as a statement of its own that failed (`fixtures/sql/trigproc.json`): a
/// `RAISE` and a failing trigger as they are, an error of the engine with
/// the procedure it was in after the words and where its statement was in
/// the procedure's script, any other (a table that is not there) with the
/// script's note and no place.
pub(crate) fn call_from_trigger(
    session: &Session,
    connection: Handle,
    settings: &statement::Settings,
    name: &str,
    pos: usize,
    args: &[V],
) -> Result<u32, Error> {
    let mut counts = Counts::default();
    FAILED_IN.with(|first| first.take());
    let result = execute(
        session,
        connection,
        connection,
        settings,
        name,
        pos,
        args,
        false,
        &mut counts,
        true,
    );
    let Err(Error::Sql(mut error)) = result else {
        return result.map(|_| counts.updated.unwrap_or(0));
    };
    let name = FAILED_IN
        .with(|failed| failed.take())
        .unwrap_or_else(|| name.to_owned());
    // A trigger words the procedure's name its own way below.
    if !matches!(error.native, 5154 | 5177)
        && let Some(cut) = error
            .message
            .strip_suffix(&format!(" Error in stored procedure: {name}"))
    {
        error.message = cut.to_owned();
    }
    match error.native {
        // A call that failed before the body ran has its words already.
        5154 | 5177 => {}
        2224 => {
            error.strip_script_location();
        }
        _ if error.message.starts_with("[ASA]") => {
            error.strip_script_location();
            error.message.push_str(&format!(
                " Error executing stored procedure: {name}{}",
                tflip_sql::error::SCRIPT_NOTE
            ));
        }
        _ => {
            // Where the statement was in the procedure's script stays, as the
            // script's note takes the place of its own words.
            error.message = error
                .message
                .replacen(" ** Script error information:", "", 1);
            error
                .message
                .push_str(&format!(" Error in stored procedure: {name}"));
        }
    }
    Err(Error::Sql(error))
}

/// The output columns and rows of the procedure `name`, run with `args` on
/// `connection` for a statement with `settings`: what `FROM` reads. The
/// procedure must not be a varying one (see [`signature`]).
pub fn rows(
    session: &Session,
    connection: Handle,
    settings: &statement::Settings,
    name: &str,
    pos: usize,
    args: &[V],
) -> Result<(Vec<Column>, Vec<Vec<Value>>), Error> {
    // A body that is not varying never runs a statement through its owner
    // handle, so the connection stands in for one.
    let done = execute(
        session,
        connection,
        connection,
        settings,
        name,
        pos,
        args,
        false,
        &mut Counts::default(),
        false,
    )?;
    Ok((done.columns, done.rows))
}

/// What a procedure returned.
struct Done {
    /// Whether its result is the cursor of the `SELECT` its body ran.
    varying: bool,
    /// Whether that cursor is its answer (`CURSOR VARYING`).
    answers: bool,
    cursor: Option<Handle>,
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
}

/// `error`, of a statement of the procedure `name`, as the original words it:
/// executing the procedure (`executing`, that of its result) or in it.
fn failed(name: &str, error: &SqlError, executing: bool) -> Error {
    let how = match executing {
        true => "executing",
        false => "in",
    };
    // An error of a procedure called by this one has its name already.
    if !executing && error.message.contains(" Error in stored procedure: ") {
        return Error::Sql(SqlError::ace(
            5154,
            &format!(
                "Execution of the stored procedure failed.   Procedure Name: {name}. Error 7200:  \
                 {error}"
            ),
            "",
        ));
    }
    Error::Sql(SqlError::ace(
        5154,
        &format!(
            "Execution of the stored procedure failed.   Procedure Name: {name}. Error 7200:  \
             {error} Error {how} stored procedure: {name}"
        ),
        "",
    ))
}

/// `error` of the procedure `name` that another calls, with the name of the
/// procedure it failed in.
fn named(mut error: SqlError, name: &str) -> SqlError {
    if !error.message.contains(" Error in stored procedure: ") {
        error
            .message
            .push_str(&format!(" Error in stored procedure: {name}"));
    }
    error
}

thread_local! {
    /// The procedure whose statement failed, while the failure goes up.
    static FAILED_IN: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// How many levels the procedures and triggers that run inside one
    /// another take, and how many of them are triggers.
    static NESTING: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// Whether a procedure or a trigger is running, whose statements the rights
/// of its owner cover, not the user's (`fixtures/sql/ddexecrights`).
pub(crate) fn running() -> bool {
    NESTING.with(|depth| depth.get().0 > 0)
}

/// A procedure or a trigger running inside others: 64 levels at most
/// (`fixtures/sql/limits_procs.json`). A procedure takes one level; a trigger
/// two, and four when no other runs: a `BEFORE UPDATE` trigger that updates
/// its own table runs 31 times, 30 of them when a procedure's statement
/// started it, and 20, 15 or 12 when it calls a procedure that updates it
/// through none, one or two others (`fixtures/sql/trigproc2.json`).
pub(crate) struct Nesting(usize, bool);

impl Nesting {
    /// The levels more, or the original engine's 7093 (in the 5154 or 5177 of
    /// what was running) if they are more than 64.
    pub(crate) fn enter(what: &str, name: &str) -> Result<Nesting, SqlError> {
        let trigger = what == "trigger";
        NESTING.with(|depth| {
            let (levels, triggers) = depth.get();
            let cost = match (trigger, triggers) {
                (false, _) => 1,
                (true, 0) => 4,
                (true, _) => 2,
            };
            match levels + cost > 64 {
            true => {
                let recursion = "The maximum number of recursion levels was exceeded. Check \
                                 triggers/stored procedures/etc for errors.  axExecuteSQL Error \
                                 executing";
                let located = "** Script error information:  -- Location of error in the SQL \
                               statement is: 1";
                Err(match what {
                    "trigger" => {
                        SqlError::ace(7093, &format!("{recursion} trigger: {name} {located}"), "")
                    }
                    _ => SqlError::ace(
                        5154,
                        &format!(
                            "Execution of the stored procedure failed.   Procedure Name: {name}. \
                             Error 7093:  {recursion} stored procedure: {name} {located}"
                        ),
                        "",
                    ),
                })
            }
                false => {
                    depth.set((levels + cost, triggers + usize::from(trigger)));
                    Ok(Nesting(cost, trigger))
                }
            }
        })
    }
}

impl Drop for Nesting {
    fn drop(&mut self) {
        NESTING.with(|depth| {
            let (levels, triggers) = depth.get();
            depth.set((levels - self.0, triggers - usize::from(self.1)));
        });
    }
}

/// Runs the procedure `name`; `counts` has the rows written by the procedures
/// that call it, which it goes on counting, and `nested` that it is called by
/// one, whose failure is that one's and not its own.
#[allow(clippy::too_many_arguments)]
fn execute(
    session: &Session,
    handle: Handle,
    connection: Handle,
    settings: &statement::Settings,
    name: &str,
    pos: usize,
    args: &[V],
    want_cursor: bool,
    counts: &mut Counts,
    nested: bool,
) -> Result<Done, Error> {
    let object = find(session, connection, name)?
        .ok_or_else(|| Error::Sql(SqlError::no_procedure(name, pos)))?;
    let _nesting = Nesting::enter("stored procedure", &object.name).map_err(Error::Sql)?;
    let invoke = object.u32(procedure::INVOKE_OPTION).unwrap_or(0);
    if invoke & SCRIPT == 0 {
        return external(session, connection, &object, args, counts);
    }
    let body = object.text(procedure::SCRIPT).unwrap_or_default();
    let script = tflip_sql::parse::parse_script(&body)
        .map_err(|error| failed(&object.name, &error, false))?;

    let inputs = params(&object, procedure::INPUT);
    let outputs = params(&object, procedure::OUTPUT);
    let varying = invoke & CURSOR_VARYING != 0 || outputs.is_empty();

    let env = statement::connection_env(session, connection)?;
    // The parameters start as variables the body reads as `_name`, a text
    // for a date read as one.
    let mut args = args.to_vec();
    let mut vars = Vars::new();
    for (i, (name, ty)) in inputs.iter().enumerate() {
        let field = script::declared_field(name, ty).map_err(Error::Sql)?;
        let value = args.get(i).cloned().unwrap_or(V::Null);
        let value = script::text_value(value, &field, &env);
        // What the parameter cannot hold is a failure of the procedure.
        script::whole_input(&value, &field).map_err(|error| failed(&object.name, &error, true))?;
        if let Some(arg) = args.get_mut(i) {
            *arg = value.clone();
        }
        vars.define(&format!("_{name}"), ty, value)
            .map_err(Error::Sql)?;
    }
    let args = &args[..];

    // The input parameters are the one row of `__input`, for as long as the
    // procedure runs (a procedure with none has no such table).
    let _input = (!inputs.is_empty()).then(|| {
        crate::trigger::enter(vec![(
            "__input".to_owned(),
            input_table(&inputs, args).unwrap_or_default(),
        )])
    });

    let output_columns = output_columns(&outputs)?;
    let name = object.name.as_str();
    let written = counts.updated;
    let mut ctx =
        ScriptCtx::new(session, connection, SettingsFrom::Given(settings)).ending(Ends::Last);
    // A body of one statement is logged as the statement alone.
    if let [tflip_sql::ast::ScriptStmt::Sql(..)] = script[..] {
        ctx = ctx.lone(body.to_string());
    }
    let mut host = Host {
        ctx,
        handle,
        env: &env,
        varying,
        want_cursor,
        output_columns: output_columns.clone(),
        output_rows: Vec::new(),
        cursor: None,
        counts,
        base: written,
        name,
        nested,
    };
    crate::transaction::call_begins(session, connection)?;
    let result = script::run(&script, &mut vars, &mut host, &env);
    let ended = crate::transaction::call_ends(
        session,
        connection,
        result.is_ok() && host.ctx.failure.is_none(),
    );
    // What a procedure that fails wrote does not count for the one that
    // called it (`fixtures/sql/txprocs.json`).
    if nested && (result.is_err() || host.ctx.failure.is_some()) {
        host.counts.updated = written;
    }
    if let Some(error) = host.ctx.failure {
        return Err(error);
    }
    result.map_err(|error| {
        // The first to fail is the procedure a trigger names in its error.
        FAILED_IN.with(|first| {
            first.borrow_mut().get_or_insert_with(|| name.to_owned());
        });
        match nested {
            // The procedure that fails names itself, whoever called it.
            true => Error::Sql(named(error, name)),
            false => failed(name, &error, false),
        }
    })?;
    ended?;
    Ok(Done {
        varying,
        answers: invoke & CURSOR_VARYING != 0,
        cursor: host.cursor,
        columns: output_columns,
        rows: host.output_rows,
    })
}

/// Runs the external procedure `object` with `args`: its library's function,
/// with the input parameters in `__input`, inside the statement's
/// transaction.
fn external(
    session: &Session,
    connection: Handle,
    object: &Object,
    args: &[V],
    counts: &mut Counts,
) -> Result<Done, Error> {
    let inputs = params(object, procedure::INPUT);
    let outputs = params(object, procedure::OUTPUT);
    let columns = output_columns(&outputs)?;
    let input = (!inputs.is_empty()).then(|| input_table(&inputs, args).unwrap_or_default());
    let _input = input
        .clone()
        .map(|table| crate::trigger::enter(vec![("__input".to_owned(), table)]));
    let library = object.text(procedure::DLL_NAME).unwrap_or_default();
    let function = object
        .text(procedure::DLL_FUNCTION_NAME)
        .unwrap_or_default();
    crate::transaction::call_begins_as(session, connection, true)?;
    // The help: data operations of a version 2 library check no rights, so
    // that it can hide tables from the users it serves.
    let run = || {
        crate::aep::run(
            session,
            connection,
            &object.name,
            &library,
            &function,
            columns.clone(),
            input,
        )
    };
    let result = match crate::connection::get(session, connection) {
        Ok(owner) => owner.as_owner(run),
        Err(_) => run(),
    };
    crate::transaction::call_ends(session, connection, result.is_ok())?;
    let ran = result?;
    // The rows affected are the ones the library says, 0 if it says none
    // (`fixtures/sql/aep.json`).
    counts.add(ran.affected);
    Ok(Done {
        varying: outputs.is_empty(),
        answers: false,
        cursor: None,
        columns,
        rows: ran.rows,
    })
}

/// The column of an input parameter: as an output's, but for a variable-length
/// text or binary, which is as long as given.
fn input_column(name: &str, field: &Field) -> Column {
    let mut column = column(name, field);
    column.length = match field.field_type {
        tflip_storage::field::FieldType::VarChar
        | tflip_storage::field::FieldType::NVarChar
        | tflip_storage::field::FieldType::VarBinary => field.length(),
        _ => column.length,
    };
    column
}

/// `__input`: the columns of the input parameters and their one row, `args`.
fn input_table(
    inputs: &[(String, String)],
    args: &[V],
) -> Result<(Vec<Column>, Vec<Vec<Value>>), SqlError> {
    let mut columns = Vec::with_capacity(inputs.len());
    let mut row = Vec::with_capacity(inputs.len());
    for (i, (name, ty)) in inputs.iter().enumerate() {
        let field = script::declared_field(name, ty)?;
        let value = args.get(i).cloned().unwrap_or(V::Null);
        let converted = script::coerced(value, &field);
        row.push(converted.to_field(Ty::of_field(&field), name)?);
        columns.push(input_column(name, &field));
    }
    Ok((columns, vec![row]))
}

/// What a `FROM` needs to know of a stored procedure before it runs.
pub struct Signature {
    /// Its input parameters, as the columns a value is checked against.
    pub inputs: Vec<Field>,
    /// Its output parameters, which are its result's columns.
    pub columns: Vec<Column>,
    /// Whether it has a result to select from: output parameters, and no
    /// varying cursor.
    pub selectable: bool,
    /// Whether its result is the cursor of the `SELECT` its body runs.
    pub varying: bool,
}

/// The signature of the connection's dictionary's procedure `name`, if there
/// is one.
pub fn signature(
    session: &Session,
    connection: Handle,
    name: &str,
) -> Result<Option<Signature>, Error> {
    let Some(object) = find(session, connection, name)? else {
        return Ok(None);
    };
    let invoke = object.u32(procedure::INVOKE_OPTION).unwrap_or(0);
    let outputs = params(&object, procedure::OUTPUT);
    let inputs = params(&object, procedure::INPUT)
        .iter()
        .map(|(name, ty)| script::declared_field(name, ty).map_err(Error::Sql))
        .collect::<Result<_, _>>()?;
    Ok(Some(Signature {
        inputs,
        columns: output_columns(&outputs)?,
        selectable: invoke & CURSOR_VARYING == 0 && !outputs.is_empty(),
        varying: invoke & CURSOR_VARYING != 0,
    }))
}

/// The host the interpreter runs the body's SQL statements through.
struct Host<'a> {
    ctx: ScriptCtx<'a>,
    handle: Handle,
    env: &'a Env,
    /// Whether the procedure returns the `SELECT` its body runs.
    varying: bool,
    want_cursor: bool,
    output_columns: Vec<Column>,
    /// The rows written to `__output`, for a procedure that is not varying.
    output_rows: Vec<Vec<Value>>,
    /// The cursor of a varying procedure's `SELECT`.
    cursor: Option<Handle>,
    /// The rows written so far, with the procedures that called this one.
    counts: &'a mut Counts,
    /// What `counts` held when the procedure began.
    base: Option<u32>,
    /// The procedure's name, and whether another procedure runs it.
    name: &'a str,
    nested: bool,
}

impl Host<'_> {
    /// What the body's statements read of themselves and the connection.
    fn scope(&self) -> crate::sysvars::Scope<'_> {
        self.ctx
            .scope(Some(self.handle), self.counts.updated.unwrap_or(0), None)
    }

    /// Appends the rows of the `__output` insert `insert` to the result.
    fn collect_output(&mut self, insert: &Insert) -> Result<(), SqlError> {
        match &insert.source {
            InsertSource::Values(rows) => {
                self.counts.add(0);
                for row in rows {
                    let mut values = Vec::with_capacity(row.len());
                    for (expr, column) in row.iter().zip(&self.output_columns) {
                        // A NULL is of the column's type.
                        let value = match expr.kind {
                            tflip_sql::ast::ExprKind::Null => V::Null,
                            _ => {
                                let node = Compiler::new(None, 0).compile(expr)?;
                                eval(&node, &mut NoRow, self.env)?
                            }
                        };
                        let ty = column_ty(column);
                        let value = script::from_text(value, ty, self.env);
                        values.push(value.to_field(ty, &column.name)?);
                    }
                    self.output_rows.push(values);
                    self.counts.add(1);
                }
            }
            InsertSource::Select(select) => {
                self.counts.add(0);
                let settings = self.ctx.settings_or_fail()?;
                let (_, rows) = statement::select_rows(
                    self.ctx.session,
                    self.ctx.connection,
                    &settings,
                    select,
                )
                .map_err(|e| self.ctx.fail(e))?;
                // The selected values take the output columns' types.
                for row in rows {
                    let mut values = Vec::with_capacity(row.len());
                    for (value, column) in row.into_iter().zip(&self.output_columns) {
                        let v = V::from_field(value, column_ty(column));
                        values.push(v.to_field(column_ty(column), &column.name)?);
                    }
                    self.output_rows.push(values);
                    self.counts.add(1);
                }
            }
        }
        Ok(())
    }
}

impl script::Host for Host<'_> {
    fn statement_begins(&mut self, text: &str, statement: &Sql, last: bool) {
        self.ctx.log_begins(text, statement, last);
    }

    fn statement_ends(&mut self, result: Result<(), &SqlError>) {
        // The rows are those this procedure wrote, not those of the one that
        // called it.
        let affected = match (self.counts.updated, self.base) {
            (Some(now), Some(base)) if now != base => now - base,
            (Some(now), None) => now,
            _ => u32::MAX,
        };
        self.ctx.log_ends(result, affected);
    }

    fn cursor_opens(&mut self, declared: bool) {
        self.ctx.log_opens(declared);
    }

    fn script_ends(&mut self, last: bool) {
        self.ctx.log_script_ends(last);
    }

    fn query(&mut self, def: &Sql, vars: &Vars) -> Result<script::Query, SqlError> {
        match script::substitute(def, vars) {
            Sql::Select(select) => self.ctx.query(&select),
            Sql::Execute(execute) => {
                let query = self.ctx.procedure_query(&execute)?;
                // The rows a procedure gives a cursor count as the ones it
                // writes (`::stmt.UpdateCount`), a system procedure's not.
                if let script::Rows::Fixed(rows) = &query.rows
                    && !crate::procedure::is_builtin(&execute.name)
                {
                    self.counts.add(rows.len() as u32);
                }
                Ok(query)
            }
            _ => Err(SqlError::unsupported("this cursor", 0)),
        }
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
        self.ctx.rewrite_expr(&self.scope(), expr, vars)
    }

    fn run(&mut self, statement: &Sql, vars: &Vars) -> Result<(), SqlError> {
        let statement = self.scope().statement(&script::substitute(statement, vars));
        match &statement {
            Sql::Insert(insert) if insert.table.name.eq_ignore_ascii_case("__output") => {
                self.collect_output(insert)?;
            }
            // (A trigger's call has no statement to run it for, and nowhere
            // to put the cursor: the `SELECT` is only checked.)
            Sql::Select(select)
                if self.varying
                    && (self.want_cursor
                        || statement::get(self.ctx.session, self.handle).is_ok()) =>
            {
                // The varying procedure's result is the last `SELECT` it runs,
                // which fails as the procedure does, not as its body.
                if let Some(old) = self.cursor.take() {
                    let _ = crate::table::close(self.ctx.session, old);
                }
                match statement::run_select(
                    self.ctx.session,
                    self.handle,
                    self.ctx.connection,
                    select,
                    self.want_cursor,
                ) {
                    Ok(cursor) => self.cursor = cursor,
                    Err(Error::Sql(error)) if !self.nested => {
                        self.ctx.failure = Some(failed(self.name, &error, true));
                        return Err(statement::read_failed());
                    }
                    Err(error) => return Err(self.ctx.fail(error)),
                }
            }
            Sql::Insert(_) | Sql::Update(_) | Sql::Delete(_) | Sql::Merge(_) => {
                let rows = self
                    .ctx
                    .dml(&statement)
                    .map_err(|e| self.ctx.nested_failure(e))?;
                self.counts.add(rows);
            }
            // A procedure called by the body, which writes its rows to no
            // result of its own.
            Sql::Execute(execute) if !crate::procedure::is_builtin(&execute.name) => {
                let args =
                    statement::arguments(self.ctx.session, self.ctx.connection, execute, &[])
                        .map_err(|e| self.ctx.fail(e))?;
                let settings = self.ctx.settings_or_fail()?;
                // Where it fails is where it is called.
                self::execute(
                    self.ctx.session,
                    self.handle,
                    self.ctx.connection,
                    &settings,
                    &execute.name,
                    execute.pos,
                    &args,
                    false,
                    self.counts,
                    true,
                )
                .map_err(|error| {
                    self.ctx.fail(match error {
                        Error::Sql(mut sql) => {
                            sql.strip_script_location();
                            Error::Sql(sql)
                        }
                        other => other,
                    })
                })?;
            }
            // The notifications of events act in a procedure; a wait there
            // does not wait, and its answer goes nowhere.
            Sql::Execute(execute) if crate::procedure::is_event(&execute.name) => {
                let args =
                    statement::arguments(self.ctx.session, self.ctx.connection, execute, &[])
                        .map_err(|e| self.ctx.fail(e))?;
                crate::procedure::run_from(
                    self.ctx.session,
                    self.handle,
                    self.ctx.connection,
                    execute,
                    &args,
                    crate::events::Context::Procedure,
                )
                .map_err(|e| self.ctx.fail(e))?;
            }
            // A bare `SELECT` in a non-varying procedure has no result to
            // return, but is checked; anything else a procedure body does not
            // do yet.
            Sql::Select(select) => self.ctx.check_select(select)?,
            Sql::SetVariable(set) => self.ctx.set_variable(set)?,
            Sql::Transaction(transaction) => {
                match crate::transaction::sql(self.ctx.session, self.ctx.connection, transaction) {
                    // The original words a savepoint it cannot find as
                    // the procedure's own failure, with no place in the
                    // script (`fixtures/sql/txprocs.json`).
                    Err(Error::Sql(error)) if error.native == 7129 && !self.nested => {
                        self.ctx.failure = Some(failed(self.name, &error, true));
                        return Err(statement::read_failed());
                    }
                    result => result.map_err(|e| self.ctx.fail(e))?,
                }
            }
            _ => {
                let error = self.ctx.fail(Error::NotImplemented);
                return Err(error);
            }
        }
        Ok(())
    }
}

/// The parameters in the procedure's `slot` property (`INPUT` or `OUTPUT`):
/// each a name and its type as [`script`] parses it, from the stored
/// `name,Type[,length];` form.
pub(crate) fn params(object: &Object, slot: usize) -> Vec<(String, String)> {
    let Some(text) = object.text(slot) else {
        return Vec::new();
    };
    text.split(';')
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            let mut fields = part.split(',');
            let name = fields.next().unwrap_or("").trim().to_owned();
            let type_name = fields.next().unwrap_or("").trim();
            let ty = match (fields.next(), fields.next()) {
                (Some(length), Some(decimals)) => {
                    format!("{type_name}({},{})", length.trim(), decimals.trim())
                }
                (Some(length), None) => format!("{type_name}({})", length.trim()),
                _ => type_name.to_owned(),
            };
            (name, ty)
        })
        .collect()
}

/// The result columns of a procedure whose output parameters are `outputs`.
fn output_columns(outputs: &[(String, String)]) -> Result<Vec<Column>, Error> {
    outputs
        .iter()
        .map(|(name, ty)| {
            let field = script::declared_field(name, ty).map_err(Error::Sql)?;
            Ok(column(name, &field))
        })
        .collect()
}

/// The column of a parameter of `field`'s type: a Numeric's length is its
/// precision, and a variable-length text or binary is a little longer than
/// given (`fixtures/sql/procvars.json`).
fn column(name: &str, field: &Field) -> Column {
    use tflip_storage::field::FieldType;
    let length = field.length();
    let length = match field.field_type {
        FieldType::Numeric => length - 1 - u16::from(field.decimals() > 0),
        FieldType::VarChar | FieldType::VarBinary => length + 2,
        FieldType::NVarChar => length + 1,
        _ => length,
    };
    Column {
        name: name.to_owned(),
        field_type: field.field_type,
        length,
        decimals: field.decimals(),
    }
}

fn column_ty(column: &Column) -> Ty {
    Ty::of_field(&statement::field_of(column))
}

#[cfg(test)]
mod tests {
    use tflip_storage::value::Value;

    use crate::script_host::tests::{connect, query as run, scratch};
    use crate::statement;

    fn ints(rows: &[Vec<Value>]) -> Vec<i128> {
        rows.iter()
            .map(|r| match r[0] {
                Value::Integer(n) => n,
                ref other => panic!("not an integer: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn runs_a_while_loop_procedure() {
        // numbers(n): the output column is the OUTPUT parameter `v`, one row
        // per loop pass (matches `adsdump` on the fixture).
        let dir = scratch("psm", "while");
        let (session, _connection, stmt) = connect(&dir);
        let (cols, rows) = run(&session, stmt, "EXECUTE PROCEDURE numbers(3)");
        assert_eq!(cols, ["v"]);
        assert_eq!(ints(&rows), [1, 2, 3]);
        let (_, rows) = run(&session, stmt, "EXECUTE PROCEDURE numbers(0)");
        assert!(rows.is_empty());
    }

    #[test]
    fn runs_a_cursor_varying_procedure() {
        // pick(which): a CURSOR VARYING procedure returns the SELECT it runs.
        let dir = scratch("psm", "varying");
        let (session, _connection, stmt) = connect(&dir);
        let (cols, rows) = run(&session, stmt, "EXECUTE PROCEDURE pick(1)");
        assert_eq!(cols, ["id", "name", "qty", "note"]);
        assert_eq!(rows.len(), 2);
        let (cols, rows) = run(&session, stmt, "EXECUTE PROCEDURE pick(2)");
        assert_eq!(cols, ["n"]);
        assert_eq!(ints(&rows), [6]);
    }

    #[test]
    fn runs_a_procedure_that_writes_a_table() {
        // add_item inserts a row and returns the new row count in `new_count`.
        let dir = scratch("psm", "writes");
        let (session, _connection, stmt) = connect(&dir);
        let (cols, rows) = run(&session, stmt, "EXECUTE PROCEDURE add_item(99, 'kiwi', 7)");
        assert_eq!(cols, ["new_count"]);
        assert_eq!(ints(&rows), [3]);
        // The row really went in.
        let (_, items) = run(&session, stmt, "SELECT id FROM items");
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn an_unknown_procedure_is_reported() {
        let dir = scratch("psm", "unknown");
        let (session, _connection, stmt) = connect(&dir);
        let error = statement::execute_direct(&session, stmt, "EXECUTE PROCEDURE nope(1)", true);
        assert!(error.is_err());
    }
}
