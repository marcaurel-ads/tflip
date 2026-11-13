//! Scripts sent as a statement (`AdsExecuteSQLDirect` of a text of several
//! statements, or of one that starts with `DECLARE`): the interpreter of
//! [`tflip_sql::script`] runs the text on the statement's connection, the
//! statements it holds as the statement runs them one by one
//! ([`statement::run_parsed`]). What the script answers is the cursor of its
//! last statement, if that is a `SELECT`, and the rows its `INSERT`, `UPDATE`
//! and `DELETE` statements changed together
//! (`fixtures/sql/scripts.json`).

use tflip_sql::ast::{ScriptStmt, Statement as Sql};
use tflip_sql::script::{self, Vars};
use tflip_sql::{Parsed, SqlError};

use crate::handle::Handle;
use crate::script_host::{ScriptCtx, SettingsFrom};
use crate::statement::{self, Param};
use crate::{Error, Session, table};

/// Runs `body` as the statement `handle` with `params`, and returns its
/// cursor, if the last statement it ran made one, and the rows it affected
/// (`u32::MAX` if no statement wrote rows).
pub(crate) fn run(
    session: &Session,
    handle: Handle,
    body: &[ScriptStmt],
    params: &[Param],
    want_cursor: bool,
) -> Result<(Option<Handle>, u32), Error> {
    let connection = statement::get(session, handle)?.connection;
    let env = statement::script_env(session, connection, params)?;
    let mut host = Host {
        ctx: ScriptCtx::new(session, connection, SettingsFrom::Statement(handle)),
        handle,
        params,
        cursor: None,
        affected: u32::MAX,
    };
    let outside = crate::transaction::count(session, connection) == Ok(0);
    let result = script::run(body, &mut Vars::new(), &mut host, &env);
    let failed = host.ctx.failure.take().or(result.err().map(Error::Sql));
    if let Some(error) = failed {
        host.close_cursor();
        // A script that fails takes back the transaction it began, if the
        // client had none: the original ends it, with the writes it held
        // (`fixtures/sql/txscripts.json`).
        if outside {
            crate::transaction::script_failed(session, connection);
        }
        return Err(error);
    }
    // A statement with no place for a cursor cannot take a `SELECT`'s.
    if host.cursor.is_some() && !want_cursor {
        host.close_cursor();
        return Err(Error::NoCursorHandle);
    }
    Ok((host.cursor, host.affected))
}

/// The host the interpreter runs the script's SQL statements through.
struct Host<'a> {
    ctx: ScriptCtx<'a>,
    handle: Handle,
    params: &'a [Param],
    /// The cursor of the last statement, which the script answers.
    cursor: Option<Handle>,
    affected: u32,
}

impl Host<'_> {
    /// The error of a cursor on the procedure `execute`: the original takes
    /// its rows to `__output` as for a statement, if the procedure is there.
    fn procedure_output(&self, execute: &tflip_sql::ast::Execute) -> Error {
        let name = &execute.name;
        let found = crate::procedure::is_builtin(name)
            || statement::connection_and_settings(self.ctx.session, self.handle)
                .and_then(|(connection, _)| {
                    Ok(crate::psm::signature(self.ctx.session, connection, name)?.is_some())
                })
                .unwrap_or(false);
        match found {
            true => statement::missing_table(self.ctx.session, self.handle, "__output"),
            false => Error::Sql(SqlError::no_procedure(name, execute.pos)),
        }
    }

    /// What the script's statements read of themselves: the rows they
    /// changed so far.
    fn scope(&self) -> crate::sysvars::Scope<'_> {
        let updated = match self.affected {
            u32::MAX => 0,
            rows => rows,
        };
        self.ctx.scope(Some(self.handle), updated, None)
    }

    fn close_cursor(&mut self) {
        if let Some(cursor) = self.cursor.take() {
            let _ = table::close(self.ctx.session, cursor);
        }
    }
}

impl script::Host for Host<'_> {
    fn statement_begins(&mut self, text: &str, statement: &Sql, last: bool) {
        self.ctx.log_begins(text, statement, last);
    }

    fn statement_ends(&mut self, result: Result<(), &SqlError>) {
        let affected = self.affected;
        self.ctx.log_ends(result, affected);
    }

    fn cursor_opens(&mut self, declared: bool) {
        self.ctx.log_opens(declared);
    }

    fn script_ends(&mut self, last: bool) {
        self.ctx.log_script_ends(last);
    }

    fn fetch(&mut self, cursor: usize) -> Result<Option<Vec<tflip_sql::V>>, SqlError> {
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
        // The notifications of events answer a script as they answer a
        // statement: with the rows of the last statement, if it is one.
        if let Sql::Execute(execute) = &statement
            && crate::procedure::is_event(&execute.name)
        {
            self.close_cursor();
            let args =
                statement::arguments(self.ctx.session, self.ctx.connection, execute, self.params)
                    .map_err(|e| self.ctx.fail(e))?;
            let results = crate::procedure::run_from(
                self.ctx.session,
                self.handle,
                self.ctx.connection,
                execute,
                &args,
                crate::events::Context::Script,
            )
            .map_err(|e| self.ctx.fail(e))?;
            if let Some(results) = results {
                self.cursor = Some(
                    statement::results_cursor(
                        self.ctx.session,
                        self.handle,
                        self.ctx.connection,
                        results,
                    )
                    .map_err(|e| self.ctx.fail(e))?,
                );
            }
            return Ok(());
        }
        // The original takes a procedure's results to the script's
        // `__output`, a table a script sent as a statement does not have.
        if let Sql::Execute(execute) = &statement {
            let error = self.procedure_output(execute);
            return Err(self.ctx.fail(error));
        }
        let parsed = Parsed {
            statement,
            params: Vec::new(),
        };
        let (cursor, rows) =
            statement::run_parsed(self.ctx.session, self.handle, &parsed, self.params, true)
                .map_err(|e| self.ctx.fail(e))?;
        self.cursor = cursor;
        if rows != u32::MAX {
            self.affected = match self.affected {
                u32::MAX => rows,
                total => total.saturating_add(rows),
            };
        }
        Ok(())
    }

    fn next_statement(&mut self) {
        self.close_cursor();
    }

    fn query(&mut self, def: &Sql, vars: &Vars) -> Result<script::Query, SqlError> {
        let def = script::substitute(def, vars);
        let select = match def {
            Sql::Select(select) => select,
            // The procedure's rows go to `__output`, as for a statement.
            Sql::Execute(execute) => {
                let error = self.procedure_output(&execute);
                return Err(self.ctx.fail(error));
            }
            _ => return Err(SqlError::unsupported("this cursor", 0)),
        };
        self.ctx.query(&select)
    }
}
