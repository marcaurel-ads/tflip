//! Running a parsed statement: queries, data changes, plans and the session's variables.

use super::*;

/// Prepares `sql` on the statement `handle` and runs it (see [`execute`]).
pub fn execute_direct(
    session: &Session,
    handle: Handle,
    sql: &str,
    want_cursor: bool,
) -> Result<Option<Handle>, Error> {
    prepare(session, handle, sql)?;
    execute(session, handle, want_cursor)
}

/// Opens the view whose `SELECT` is `sql` on `connection` as a table: the
/// cursor of a statement of its own, called `alias`, or as its file if
/// that is empty.
pub(crate) fn open_view(
    session: &Session,
    connection: Handle,
    sql: &str,
    alias: &str,
) -> Result<Handle, Error> {
    let statement = create(session, connection)?;
    let cursor = execute_direct(session, statement, sql, true)?.ok_or(Error::NoCursorHandle)?;
    let table = table::get(session, cursor)?;
    let alias = match alias.is_empty() {
        true => table::stem(&table.path.to_string_lossy()),
        false => alias.to_owned(),
    };
    let _ = table.alias.set(alias);
    table.opened.store(true, atomic::Ordering::Relaxed);
    Ok(cursor)
}

/// Runs the SQL prepared on the statement `handle`, and returns the cursor
/// of a `SELECT` or of a procedure with results. Without `want_cursor` (no
/// place for one), a `SELECT` is [`Error::NoCursorHandle`] and its cursor is
/// not kept. The outermost statement is logged, if its connection's queries
/// are (see [`crate::query_log`]).
pub fn execute(
    session: &Session,
    handle: Handle,
    want_cursor: bool,
) -> Result<Option<Handle>, Error> {
    let level = crate::query_log::Level::enter();
    let mut logged = Logged::default();
    if !level.is_outermost() {
        return run_statement(session, handle, want_cursor, &mut logged);
    }
    let started = std::time::Instant::now();
    let result = run_statement(session, handle, want_cursor, &mut logged);
    if let (Some(sql), Ok(statement)) = (logged.sql, get(session, handle)) {
        // A `SELECT` without a place for its cursor ran all the same.
        let outcome = match &result {
            Err(Error::NoCursorHandle) => Ok(false),
            other => other.as_ref().map(|cursor| cursor.is_some()),
        };
        let affected = statement.state().affected;
        let end = crate::query_log::finish(
            session,
            statement.connection,
            logged.entry,
            &sql,
            crate::query_log::Outcome {
                result: outcome,
                affected,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            },
        );
        statement.state().log_end = end;
    }
    result
}

/// What the query log learns of a statement as it runs.
#[derive(Default)]
struct Logged {
    /// Its text, once it is known to run.
    sql: Option<String>,
    /// Its row in the log, once it is known to parse and plan.
    entry: Option<crate::query_log::Entry>,
}

fn run_statement(
    session: &Session,
    handle: Handle,
    want_cursor: bool,
    logged: &mut Logged,
) -> Result<Option<Handle>, Error> {
    let statement = get(session, handle)?;
    let _running = running(&statement);
    statement.check_cursor(session)?;
    // The cursor of its last run is closed: the log has the end of that.
    cursor_closed(session, handle, 0);
    // The tables its last run held close once this run opened its own, so
    // that those it reads again stay open.
    let _previous = std::mem::take(&mut statement.state().held);
    let (sql, params, again) = {
        let mut state = statement.state();
        let sql = state.sql.clone().ok_or(Error::NotPrepared)?;
        if state.executed.is_none() {
            drop(state);
            check_statement_limit(session, &statement)?;
            state = statement.state();
        }
        let again = state
            .executed
            .as_ref()
            .is_some_and(|(last, _)| *last == sql);
        state.executed = Some((sql.clone(), crate::clock::now().local.1));
        (sql, state.params.clone(), again)
    };
    let located = |error| match error {
        Error::Sql(error) => Error::Sql(error.with_lines(&sql)),
        other => other,
    };
    logged.sql = Some(sql.clone());
    let parsed = tflip_sql::parse(&sql).map_err(|e| located(Error::Sql(e)))?;
    connection::get(session, statement.connection)?
        .operations
        .fetch_add(1, atomic::Ordering::Relaxed);
    let parsed = match sql.contains("::") {
        true => bound(session, handle, parsed, 0),
        false => parsed,
    };
    crate::column_rights::check(session, statement.connection, &parsed.statement)
        .map_err(located)?;
    let cursor = matches!(parsed.statement, Sql::Select(_) | Sql::Execute(_));
    // A script's statements are rows of their own, not the script.
    if matches!(parsed.statement, Sql::Script(_)) {
        logged.sql = None;
    } else {
        logged.entry =
            crate::query_log::begin(session, statement.connection, &sql, cursor, || {
                optimized(
                    session,
                    handle,
                    statement.connection,
                    &parsed,
                    &sql,
                    &params,
                )
            })?;
    }
    // Only a `SELECT` reads again what the last one read.
    let select = matches!(parsed.statement, Sql::Select(_));
    let before = std::mem::take(&mut statement.state().temps_read);
    // The tables its last run opened close now, for the database triggers,
    // unless this runs the same SQL, which keeps them. A statement a
    // procedure or a trigger runs leaves its tables to the one it runs in.
    let nested = crate::trigger::collects();
    let (run, read) = match nested {
        true => crate::temp::reading(if select { before } else { Vec::new() }, || {
            run_parsed(session, handle, &parsed, &params, want_cursor)
        }),
        false => {
            let previous = std::mem::take(&mut statement.state().opened);
            let reuse = match again {
                true => previous,
                false => {
                    crate::trigger::close_all(session, previous);
                    Vec::new()
                }
            };
            let ((run, read), mut opened, unused) = crate::trigger::collecting(reuse, || {
                crate::temp::reading(if select { before } else { Vec::new() }, || {
                    run_parsed(session, handle, &parsed, &params, want_cursor)
                })
            });
            if closes_backwards(&parsed.statement) {
                opened.reverse();
            }
            statement.state().opened = opened;
            crate::trigger::close_all(session, unused);
            (run, read)
        }
    };
    statement.state().temps_read = if select { read } else { Vec::new() };
    let (cursor, affected) = run.map_err(located)?;
    let mut state = statement.state();
    state.affected = affected;
    state.cursor = cursor;
    if matches!(parsed.statement, Sql::Select(_) | Sql::ShowPlan(..)) && !want_cursor {
        return Err(Error::NoCursorHandle);
    }
    Ok(cursor)
}

/// Whether `statement` closes the tables it opened last first, for the
/// database triggers: a `SELECT` of several tables with no condition does
/// (probed, `tools/ace/sql/trigdblog.txt`), the others in the order they were
/// opened.
fn closes_backwards(statement: &Sql) -> bool {
    use tflip_sql::ast::{From, Select};
    fn conditioned(from: &From) -> bool {
        match from {
            From::Join {
                left, right, on, ..
            } => on.is_some() || conditioned(left) || conditioned(right),
            From::Derived { select, .. } => select_conditioned(select),
            _ => false,
        }
    }
    fn select_conditioned(select: &Select) -> bool {
        select.filter.is_some()
            || conditioned(&select.from)
            || select
                .unions
                .iter()
                .any(|(_, union)| select_conditioned(union))
    }
    matches!(statement, Sql::Select(select) if !select_conditioned(select))
}

/// A server refuses a connection's statement that runs for the first time
/// while the connection has as many that ran and are not closed as its limit
/// (`sp_SetStatementLimit`, 50 to begin with, 0 or less for no limit): 7216
/// (`fixtures/sql/txlimit.json`). The original's local server enforces none.
fn check_statement_limit(session: &Session, statement: &Statement) -> Result<(), Error> {
    if session.shares.is_none() {
        return Ok(());
    }
    let limit = connection::statement_limit(session, statement.connection)?;
    if limit <= 0 {
        return Ok(());
    }
    let mut running = 0;
    session.for_each(|object| {
        if let Object::Statement(other) = object
            && other.connection == statement.connection
            && other.state().executed.is_some()
        {
            running += 1;
        }
    });
    match running >= limit {
        true => Err(Error::StatementLimit),
        false => Ok(()),
    }
}

/// `parsed` with the system variables in it (`::conn.Name`) read as the
/// statement `handle` reads them, `updated` rows changed so far by the script
/// it is part of.
pub(crate) fn bound(
    session: &Session,
    handle: Handle,
    parsed: tflip_sql::Parsed,
    updated: u32,
) -> tflip_sql::Parsed {
    let Ok(statement) = get(session, handle) else {
        return parsed;
    };
    let scope = crate::sysvars::Scope {
        session,
        connection: statement.connection,
        statement: Some(handle),
        updated,
        trigger: None,
    };
    tflip_sql::Parsed {
        statement: scope.statement(&parsed.statement),
        params: parsed.params,
    }
}

/// The cursor on the result of a system procedure a script ran.
pub(crate) fn results_cursor(
    session: &Session,
    handle: Handle,
    connection: Handle,
    result: procedure::Results,
) -> Result<Handle, Error> {
    static_cursor(
        session,
        handle,
        connection,
        result.columns,
        result.rows,
        result.shown,
        None,
        false,
        false,
    )
}

/// Runs `parsed`, a statement of the statement `handle` with `params`, and
/// returns its cursor and the rows it affected (`u32::MAX` for none).
pub(crate) fn run_parsed(
    session: &Session,
    handle: Handle,
    parsed: &tflip_sql::Parsed,
    params: &[Param],
    want_cursor: bool,
) -> Result<(Option<Handle>, u32), Error> {
    let statement = get(session, handle)?;
    let connection = statement.connection;
    let settings = statement.state().settings.clone();
    let settings = &settings;
    if let Ok(owner) = connection::get(session, connection) {
        owner.statements.fetch_add(1, atomic::Ordering::Relaxed);
    }
    let (cursor, affected) = match &parsed.statement {
        Sql::Select(select) => {
            // A `SELECT` takes a statement number, as a plan does: the next
            // plan's number is one more (`fixtures/sql/fts_plans.json`).
            next_plan(session, connection)?;
            // The statement holds the tables it reads until it closes or
            // runs again (probed, `tools/ace/cache`).
            let (cursor, held) =
                holding(|| select_cursor(session, handle, connection, select, params, want_cursor));
            statement.state().held = held;
            (cursor?, u32::MAX)
        }
        Sql::Execute(execute) => {
            if procedure::is_builtin(&execute.name) {
                procedure::refuse_user(session, connection, &execute.name)?;
                let inputs = procedure::inputs(&execute.name);
                let args = match inputs.is_empty() {
                    true => arguments(session, connection, execute, params)?,
                    false => procedure_arguments(
                        session,
                        connection,
                        settings,
                        execute,
                        &inputs,
                        Procedures::Run {
                            statement: Some(handle),
                            params,
                        },
                    )?,
                };
                let cursor = match procedure::run(session, handle, connection, execute, &args)? {
                    Some(result) if want_cursor => Some(static_cursor(
                        session,
                        handle,
                        connection,
                        result.columns,
                        result.rows,
                        result.shown,
                        None,
                        false,
                        false,
                    )?),
                    _ => None,
                };
                (cursor, 0)
            } else {
                refuse_procedure(session, connection, execute)?;
                // The arguments are the procedure's inputs' kind.
                let signature = crate::psm::signature(session, connection, &execute.name)?
                    .ok_or_else(|| {
                        Error::Sql(SqlError::no_procedure(&execute.name, execute.pos))
                    })?;
                check_call_arity(signature.inputs.len(), execute)?;
                let args = procedure_arguments(
                    session,
                    connection,
                    settings,
                    execute,
                    &signature.inputs,
                    Procedures::Run {
                        statement: Some(handle),
                        params,
                    },
                )?;
                // What the procedure's statements opened closes with it.
                let (ran, opened, _) = crate::trigger::collecting(Vec::new(), || {
                    crate::psm::run(
                        session,
                        handle,
                        connection,
                        &execute.name,
                        execute.pos,
                        &args,
                        want_cursor,
                    )
                });
                crate::trigger::close_all(session, opened);
                ran?
            }
        }
        &Sql::Debug(command, pos) => {
            debug(session, connection, command, pos)?;
            (None, u32::MAX)
        }
        Sql::CachePrepare(_) => (None, u32::MAX),
        Sql::ShowPlan(inner, text) => {
            let cursor = show_plan(
                session,
                handle,
                connection,
                inner,
                text,
                params,
                want_cursor,
            )?;
            (cursor, u32::MAX)
        }
        Sql::Insert(insert) if crate::aep::collects(&insert.table.name) => {
            let rows = crate::aep::insert(session, connection, settings, insert)?;
            statement.state().affected = rows;
            (None, rows)
        }
        // A library trigger's write to `__new`, which changes nothing, as a
        // script trigger's does not (`fixtures/sql/aeptrig.json`).
        Sql::Update(update) if crate::trigger::writes_new(&update.table.name) => {
            statement.state().affected = 1;
            (None, 1)
        }
        Sql::Insert(_) | Sql::Update(_) | Sql::Delete(_) | Sql::Merge(_) => {
            // The rows changed count even when the statement fails.
            let implicit = crate::transaction::statement_begins(session, connection)?;
            let mut done = dml_statement(session, connection, settings, &parsed.statement, params);
            if let Some(implicit) = implicit {
                let ended =
                    crate::transaction::statement_ends(session, implicit, done.result.is_ok());
                done.result = done.result.and(ended);
            }
            statement.state().affected = done.rows;
            done.result.map_err(rights_sql_error)?;
            (None, done.rows)
        }
        Sql::CreateTable(create) => {
            crate::ddl::create_table(session, connection, settings, create)?;
            (None, u32::MAX)
        }
        Sql::CreateIndex(create) => {
            crate::ddl::create_index(session, connection, settings, create)?;
            (None, u32::MAX)
        }
        Sql::DropTable(drop) => {
            crate::ddl::drop_table(session, connection, settings, drop)?;
            (None, u32::MAX)
        }
        Sql::SetVariable(set) => {
            set_variable(session, connection, set, params)?;
            (None, u32::MAX)
        }
        Sql::DropIndex(table, name) => {
            crate::ddl::drop_index(session, connection, settings, table, name)?;
            (None, u32::MAX)
        }
        Sql::AlterTable(alter) => {
            crate::ddl::alter_table(session, connection, settings, alter)?;
            (None, u32::MAX)
        }
        Sql::Transaction(transaction) => {
            crate::transaction::sql(session, connection, transaction)?;
            (None, u32::MAX)
        }
        Sql::Grant(grant) => {
            crate::dictionary_write::sql_grant(session, connection, grant)?;
            (None, u32::MAX)
        }
        Sql::CreateTrigger(create) => {
            crate::trigger::create(session, connection, create)?;
            (None, u32::MAX)
        }
        Sql::DropTrigger(table, name) => {
            crate::trigger::drop(session, connection, table.as_ref(), name)?;
            (None, u32::MAX)
        }
        Sql::Script(body) => crate::sent::run(session, handle, body, params, want_cursor)?,
        Sql::CreateView(create) => {
            crate::dictionary_write::create_view(session, connection, create, params.len())?;
            (None, u32::MAX)
        }
        Sql::DropView(name) => {
            crate::dictionary_write::drop_view(session, connection, name)?;
            (None, u32::MAX)
        }
        Sql::CreateDatabase(create) => {
            crate::dictionary_write::create_database(session, connection, create)?;
            (None, u32::MAX)
        }
        Sql::CreateProcedure(create) => {
            crate::dictionary_write::create_procedure(session, connection, create)?;
            (None, u32::MAX)
        }
        Sql::DropProcedure(name) => {
            crate::dictionary_write::drop_procedure(session, connection, name)?;
            (None, u32::MAX)
        }
        Sql::FunctionDdl(ddl) => {
            crate::dictionary_write::function_ddl(session, connection, ddl)?;
            (None, u32::MAX)
        }
    };
    Ok((cursor, affected))
}

/// A user with no rights at all on the table an `INSERT`, `UPDATE` or `DELETE`
/// changes is told what it lacks for the operation (7087), not that it may
/// not read (7079) (`fixtures/sql/aeprulesremote.json`).
fn no_rights_to_change(statement: &Sql, error: Error) -> Error {
    let table = match statement {
        Sql::Insert(insert) => &insert.table,
        Sql::Update(update) => &update.table,
        Sql::Delete(delete) => &delete.table,
        _ => return error,
    };
    match &error {
        Error::Sql(sql)
            if sql.native == 7079
                && sql
                    .to_string()
                    .ends_with(&format!("Table name: {}", table.name)) =>
        {
            Error::Sql(SqlError::ace(
                7087,
                &format!(
                    "Cannot open table or view for this operation because the requested \
                     permissions have not been granted to the user. Table name: {}",
                    table.name
                ),
                "",
            ))
        }
        _ => error,
    }
}

/// 5072: a `system.*` table, which every cursor of is static, is not written
/// by `INSERT`, `UPDATE`, `DELETE` or `MERGE`.
pub(super) fn writable(statement: &Sql) -> Result<(), Error> {
    let table = match statement {
        Sql::Insert(insert) => &insert.table,
        Sql::Update(update) => &update.table,
        Sql::Delete(delete) => &delete.table,
        Sql::Merge(merge) => &merge.table,
        _ => return Ok(()),
    };
    let name = tflip_sql::query::plan_name(&table.name);
    let system = (table.name.get(..7)).is_some_and(|p| p.eq_ignore_ascii_case("system."));
    match system && (name.eq_ignore_ascii_case("iota") || system_name(&table.name).is_some()) {
        true => Err(Error::Sql(SqlError::ace(
            5072,
            &format!("Action requires read-write access to the table. View is static: {name}"),
            "",
        ))),
        false => Ok(()),
    }
}

/// Runs `statement`, an `INSERT`, `UPDATE`, `DELETE` or `MERGE` with `params`, on
/// `connection`.
pub(super) fn dml_statement(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    statement: &Sql,
    params: &[Param],
) -> crate::dml::Done {
    use crate::system::right;
    let failed = |error| crate::dml::Done {
        rows: u32::MAX,
        result: Err(error),
    };
    if let Err(error) = writable(statement) {
        return failed(error);
    }
    // A user may insert, update and delete what it holds the right to,
    // without the right to read the table. A condition reads it, and is
    // refused (`fixtures/sql/ddinsertonly`).
    let (table, needs) = match statement {
        Sql::Insert(insert) => (&insert.table, right::INSERT),
        Sql::Update(update) => (&update.table, right::UPDATE),
        Sql::Delete(delete) => (&delete.table, right::DELETE),
        _ => return dml_statement_open(session, connection, settings, statement, params, true),
    };
    // A statement that reads the table it changes reads it as any statement
    // does: the right to change does not open it, and a user holding that
    // right is told it may not read (7079), not that it lacks the right
    // (`fixtures/sql/ddinsertonly2`).
    let mut named = 0;
    statement.for_each_table(&mut |named_table| {
        named += (named_table.link.is_none() && named_table.name.eq_ignore_ascii_case(&table.name))
            as usize;
    });
    let reads_itself = match statement {
        Sql::Insert(_) => named > 1,
        Sql::Update(update) => update.from.is_none() && named > 1,
        Sql::Delete(_) => named > 1,
        _ => false,
    };
    if reads_itself {
        let holds = crate::connection::get(session, connection).is_ok_and(|owner| {
            owner.privileged()
                || owner
                    .with_database_opt(|database| database.access_mask(&table.name))
                    .is_some_and(|mask| mask & needs != 0)
        });
        return dml_statement_open(session, connection, settings, statement, params, !holds);
    }
    crate::table::changing(&table.name, needs, || {
        dml_statement_open(session, connection, settings, statement, params, true)
    })
}

/// [`dml_statement`] once the table's rights are settled; `told_what_lacks` is
/// whether a user with no right on the table is told which it lacks (7087)
/// where it may not read it (7079).
fn dml_statement_open(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    statement: &Sql,
    params: &[Param],
    told_what_lacks: bool,
) -> crate::dml::Done {
    let failed = |error| crate::dml::Done {
        rows: u32::MAX,
        result: Err(error),
    };
    let tables = match Opened::open_statement(
        session,
        connection,
        settings,
        statement,
        Procedures::Run {
            statement: None,
            params,
        },
    ) {
        Ok(tables) => tables,
        Err(error) if told_what_lacks => return failed(no_rights_to_change(statement, error)),
        Err(error) => return failed(error),
    };
    let prepared = match tflip_sql::dml::prepare(statement, &tables, params.len()) {
        Ok(prepared) => prepared,
        Err(error) => return failed(tables.error(error)),
    };
    let mut env = match env(
        session,
        connection,
        Some(settings),
        params,
        &prepared.params,
    ) {
        Ok(env) => env,
        Err(error) => return failed(error),
    };
    oem_params(&mut env, tables.oem());
    let target = match statement {
        Sql::Insert(insert) => &insert.table,
        Sql::Update(update) => &update.table,
        Sql::Delete(delete) => &delete.table,
        Sql::Merge(merge) => &merge.table,
        _ => return failed(Error::NotImplemented),
    };
    // A linked dictionary's table is written on the connection to it.
    let connection = match &target.link {
        Some(link) => match crate::link::open(session, connection, link, &target.name) {
            Ok(linked) => linked,
            Err(error) => return failed(error),
        },
        None => connection,
    };
    let run = prepared.runner(&tables, &env);
    let name = &target.name;
    let mut done = crate::dml::run(session, connection, settings, name, &prepared, &run);
    // A statement that fails before changing a row counts none.
    if done.result.is_err() && done.rows == 0 {
        done.rows = u32::MAX;
    }
    done
}

/// The plan of the `SELECT` `select`, whose statement's text is `text`: a
/// live cursor's, a `system.*` table's or one that runs through its
/// operators (always, if the rows are those a statement `changing` them
/// reads), with the number the plan has.
fn select_plan(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    select: &tflip_sql::ast::Select,
    text: &str,
    params: &[Param],
    changing: bool,
) -> Result<(u32, Vec<tflip_sql::explain::PlanRow>), Error> {
    use tflip_sql::explain;
    // The rows an `UPDATE` or `DELETE` changes are read through the
    // operators, not as a live cursor.
    let from = select.table().filter(|_| select.is_simple() && !changing);
    // The original engine plans a `system.*` table without reading
    // it, even on a connection without a dictionary.
    if let Some(from) = from
        && system_name(&from.name).is_some()
    {
        let name = tflip_sql::query::plan_name(&from.name);
        let alias = from.alias.clone().unwrap_or_else(|| name.clone());
        Ok((
            next_plan(session, connection)?,
            explain::scan(text, &name, &alias),
        ))
    } else if !changing
        && let Some((plan, source, table_name)) =
            live_plan(session, connection, settings, select, params)?
    {
        // A live cursor is the table itself, filtered and sorted as
        // it is read.
        let number = next_plan(session, connection)?;
        env(session, connection, Some(settings), params, &plan.params)?;
        // A derived table's alias is the table's alias in the plan.
        let (default, own) = match &select.from {
            tflip_sql::ast::From::Derived { alias, .. } => {
                (alias.clone().unwrap_or_default(), alias.clone())
            }
            tflip_sql::ast::From::Table(from) => {
                (tflip_sql::query::plan_name(&from.name), from.alias.clone())
            }
            _ => return Err(Error::NotImplemented),
        };
        let name = table_name.map_or_else(
            || default.clone(),
            |name| tflip_sql::query::plan_name(&name),
        );
        let alias = own.unwrap_or(default);
        let Some(table) = source.table else {
            return Err(Error::NotImplemented);
        };
        let table = Arc::new(table);
        let used = live_use(&table, &plan, &[], true)?;
        let rows = explain::live(text, &plan, &name, &alias, table.fields(), &used.indexes);
        Ok((number, rows))
    } else {
        let tables = Opened::open(
            session,
            connection,
            settings,
            select,
            Procedures::Describe(params.len()),
        )?;
        let prepared = match changing {
            true => tflip_sql::query::prepare_changing(select, &tables, params.len()),
            false => tflip_sql::query::prepare(select, &tables, params.len()),
        }
        .map_err(|e| tables.error(e))?;
        let number = next_plan(session, connection)?;
        let env = env(
            session,
            connection,
            Some(settings),
            params,
            &prepared.params,
        )?;
        Ok((
            number,
            explain::query(text, &prepared, Some((&tables, &env))),
        ))
    }
}

/// Whether the query log counts the statement `parsed` as optimized: a
/// `SELECT` is if its plan has nothing unoptimized, any other statement is.
/// `None` for one that does not plan (it fails before it runs).
fn optimized(
    session: &Session,
    handle: Handle,
    connection: Handle,
    parsed: &tflip_sql::Parsed,
    text: &str,
    params: &[Param],
) -> Option<bool> {
    let Sql::Select(select) = &parsed.statement else {
        return Some(true);
    };
    let settings = settings_of(session, handle).ok()?;
    plan_optimized(session, connection, &settings, select, text, params)
}

/// [`optimized`] of the `SELECT` `select` with the table `settings`.
pub(crate) fn plan_optimized(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    select: &tflip_sql::ast::Select,
    text: &str,
    params: &[Param],
) -> Option<bool> {
    // Planning is not showing a plan: it takes no number of its own.
    let owner = connection::get(session, connection).ok()?;
    let numbered = owner.plans.load(atomic::Ordering::Relaxed);
    let plan = select_plan(session, connection, settings, select, text, params, false);
    owner.plans.store(numbered, atomic::Ordering::Relaxed);
    match plan {
        Ok((_, rows)) => Some(!rows.iter().any(|row| {
            [&row.arguments, &row.warning]
                .into_iter()
                .flatten()
                .any(|text| {
                    text.contains("not optimized")
                        || text.contains("not fully optimized")
                        || text.contains("No index matching the ORDER BY")
                        || text.contains("no existing index to optimize the join")
                })
        })),
        Err(Error::NotImplemented) => Some(true),
        Err(_) => None,
    }
}

/// `SHOW PLAN FOR statement`: the plan of `statement`, whose text is
/// `text`, as a cursor of the original engine's columns (see
/// [`tflip_sql::explain`]). The statement is checked as running it would
/// check it, but not run.
fn show_plan(
    session: &Session,
    handle: Handle,
    connection: Handle,
    statement: &Sql,
    text: &str,
    params: &[Param],
    want_cursor: bool,
) -> Result<Option<Handle>, Error> {
    use tflip_sql::explain;
    let settings = &settings_of(session, handle)?;
    let rows = match statement {
        Sql::Select(select) => {
            select_plan(session, connection, settings, select, text, params, false)?
        }
        Sql::Execute(execute) => {
            check_execute(session, connection, execute)?;
            (
                next_plan(session, connection)?,
                explain::statement("EXECUTE", text),
            )
        }
        Sql::Debug(..) | Sql::ShowPlan(..) => (
            next_plan(session, connection)?,
            explain::statement("DEBUG", text),
        ),
        Sql::Merge(_) => {
            let tables = Opened::open_statement(
                session,
                connection,
                settings,
                statement,
                Procedures::Describe(params.len()),
            )?;
            let bound = tflip_sql::dml::prepare(statement, &tables, params.len())
                .map_err(|e| tables.error(e))?;
            let number = next_plan(session, connection)?;
            let env = env(session, connection, Some(settings), params, &bound.params)?;
            let rows = tflip_sql::dml::merge_plan(statement, text, &tables, &env, params.len())
                .map_err(|e| tables.error(e))?;
            (number, rows)
        }
        Sql::Insert(_) | Sql::Update(_) | Sql::Delete(_) => {
            check(session, connection, settings, statement, params.len())?;
            let tables = Opened::open_statement(
                session,
                connection,
                settings,
                statement,
                Procedures::Describe(params.len()),
            )?;
            let bound = tflip_sql::dml::prepare(statement, &tables, params.len())
                .map_err(|e| tables.error(e))?;
            let plan = bound.plan.as_ref().ok_or(Error::NotImplemented)?;
            let (number, mut rows) = match &plan.select {
                Some(select) => {
                    select_plan(session, connection, settings, select, text, params, true)?
                }
                None => (
                    next_plan(session, connection)?,
                    explain::statement("INSERT", text),
                ),
            };
            let row_type = match statement {
                Sql::Insert(_) => "INSERT",
                Sql::Update(_) => "UPDATE",
                _ => "DELETE",
            };
            explain::dml(
                &mut rows,
                row_type,
                plan.operator,
                &plan.table,
                plan.arguments.as_deref(),
            );
            (number, rows)
        }
        Sql::CreateTable(_)
        | Sql::CreateIndex(_)
        | Sql::DropTable(_)
        | Sql::DropIndex(..)
        | Sql::AlterTable(_)
        | Sql::CreateTrigger(_)
        | Sql::DropTrigger(..)
        | Sql::CreateView(_)
        | Sql::DropView(_)
        | Sql::CreateDatabase(_)
        | Sql::CreateProcedure(_)
        | Sql::DropProcedure(_)
        | Sql::FunctionDdl(_)
        | Sql::CachePrepare(_)
        | Sql::Transaction(_)
        | Sql::SetVariable(_)
        | Sql::Grant(_)
        | Sql::Script(_) => {
            return Err(Error::Sql(SqlError::unsupported_plan()));
        }
    };
    if !want_cursor {
        return Ok(None);
    }
    let (number, rows) = rows;
    // The original engine shows the nodes by the node they are part of.
    let mut sequence: Vec<u32> = (1..=rows.len() as u32).collect();
    sequence.sort_by_key(|&i| rows[i as usize - 1].parent);
    let sequence = (sequence.iter().zip(1..).any(|(&a, b)| a != b)).then_some(sequence);
    let columns = explain::COLUMNS
        .iter()
        .map(|&(name, field_type, length, decimals)| Column {
            name: name.to_owned(),
            field_type,
            length,
            decimals,
        })
        .collect();
    let text = |s: &Option<String>| s.clone().map_or(Value::Null, Value::Text);
    let rows = rows
        .iter()
        .map(|row| {
            vec![
                Value::Integer(row.node.into()),
                Value::Integer(row.parent.into()),
                Value::Text(row.row_type.clone()),
                text(&row.text),
                Value::Integer(number.into()),
                text(&row.operator),
                text(&row.arguments),
                row.estimate
                    .map_or(Value::Null, |n| Value::Integer(n.into())),
                text(&row.warning),
            ]
        })
        .collect();
    let n = PLAN_CURSORS.fetch_add(1, atomic::Ordering::Relaxed);
    let name = format!("timace_{n:x}.tmp");
    named_cursor(session, handle, connection, &name, columns, rows, sequence).map(Some)
}

/// The plan of `select`, which reads one table (or a view or derived table
/// of it) alone, and the table (and its name, where it is read through one),
/// if it is a live cursor; checking it as running it would.
fn live_plan(
    session: &Session,
    connection: Handle,
    settings: &Settings,
    select: &Select,
    params: &[Param],
) -> Result<Option<(Plan, Source, Option<String>)>, Error> {
    let Some(planned) = planned_select(session, connection, settings, select, params.len())? else {
        return Ok(None);
    };
    Ok(planned
        .is_live()
        .then_some((planned.plan, planned.source, planned.table_name)))
}

/// The number of the next plan made on `connection`.
fn next_plan(session: &Session, connection: Handle) -> Result<u32, Error> {
    let owner = connection::get(session, connection)?;
    Ok(owner.plans.fetch_add(1, atomic::Ordering::Relaxed) + 1)
}

/// Runs a command of the SQL debugger at `pos` on `connection`. As in the
/// original engine, `DEBUG BEGIN` makes the connection a debugger and
/// `DEBUG END` ends that; the other commands need a debugger, and tflip does
/// not run them yet.
fn debug(session: &Session, connection: Handle, command: Debug, pos: usize) -> Result<(), Error> {
    let debugging = &connection::get(session, connection)?.debugging;
    let was = debugging.load(atomic::Ordering::Relaxed);
    let error = match (command, was) {
        (Debug::Begin, false) | (Debug::End, true) => {
            debugging.store(!was, atomic::Ordering::Relaxed);
            return Ok(());
        }
        (Debug::Begin, true) => SqlError::already_debugging(),
        (_, false) => SqlError::not_debugging(pos),
        (Debug::Wait, true) => SqlError::debug_unsupported("WAIT", pos),
        (Debug::Database, true) => SqlError::debug_unsupported("DATABASE", pos),
    };
    Err(Error::Sql(error))
}

/// `SET ::conn.Collation = value`, `SET ::stmt.Collation = value` and `SET
/// ::stmt.unicode_sort_key_size = n`. A
/// client's statement sees no effect of either (`AdsSetCollation` and
/// `AdsStmtSetTableCollation` set what its tables open with,
/// `fixtures/sql/setcollation.json`), but the name is checked as the original
/// engine checks it; so is the size of the key a Unicode sort makes of a
/// character, an integer from 1 to 8, which sorts here as it always does
/// (`fixtures/sql/gram_sysvar.json`).
pub(crate) fn set_variable(
    session: &Session,
    connection: Handle,
    set: &tflip_sql::ast::SetVariable,
    params: &[Param],
) -> Result<(), Error> {
    if !matches!(set.scope.as_str(), "conn" | "stmt") {
        return Err(Error::Sql(SqlError::invalid_set_value()));
    }
    let key_size = set.scope == "stmt" && set.member.eq_ignore_ascii_case("unicode_sort_key_size");
    if !key_size && !set.member.eq_ignore_ascii_case("collation") {
        return Err(Error::Sql(SqlError::unknown_system_variable(
            &set.member,
            set.member_pos,
        )));
    }
    // A parameter is the text the variable takes.
    let text = Ty::Str {
        field: tflip_storage::field::FieldType::Character,
        length: 255,
    };
    let env = env(session, connection, None, params, &vec![text; params.len()])?;
    let mut compiler = tflip_sql::Compiler::new(None, params.len());
    compiler.params = vec![text; params.len()];
    compiler.functions = crate::udf::catalog(session, connection);
    compiler.user = environment::sql_user(session, connection);
    let value = match set.value.kind {
        tflip_sql::ast::ExprKind::Null => V::Null,
        _ => {
            let node = compiler.compile(&set.value).map_err(Error::Sql)?;
            tflip_sql::eval(&node, &mut NoRow, &env).map_err(Error::Sql)?
        }
    };
    if key_size {
        let at = set.value.pos;
        return match value {
            V::Int(1..=8) => Ok(()),
            V::Null => Err(Error::Sql(SqlError::sort_key_size_range(None))),
            V::Int(_) | V::Num(..) | V::Double(_) | V::Money(_) => {
                Err(Error::Sql(SqlError::sort_key_size_range(Some(at))))
            }
            V::Bool(_) => Err(Error::Sql(SqlError::bool_set_value(set.member_pos))),
            _ => Err(Error::Sql(SqlError::invalid_set_value())),
        };
    }
    let V::Str(name) = value else {
        return Err(Error::Sql(SqlError::invalid_set_value()));
    };
    check_collation(session, &name)
}
