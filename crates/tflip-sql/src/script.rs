//! Running SQL scripts: stored-procedure, function and trigger bodies.
//!
//! The parser turns a body into [`ScriptStmt`]s ([`crate::parse::parse_script`]);
//! the interpreter here runs them. It keeps the script's variables — the
//! routine's parameters (a procedure references them `_name`, a function by
//! their plain name) and its `DECLARE`d locals — evaluates the control flow
//! and the expressions of its conditions, assignments and `RETURN`, and
//! hands the ordinary SQL statements it runs to a [`Host`].
//!
//! The expressions of a script read its variables, not a table's columns, so
//! the interpreter compiles them against a [`Scope`] of the variables (their
//! names resolve like columns do) and evaluates them over a [`Row`] that
//! gives each variable's value. Everything that touches tables — the SQL
//! statements, and the pseudo-tables `__input`, `__output`, `__new` and
//! `__old` they name — is the [`Host`]'s, which the engine implements over
//! its tables; the host sees the variables in scope, so the SQL it runs can
//! read them too.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};

use tflip_storage::adt;
use tflip_storage::field::{Field, FieldType};
use tflip_storage::value::{Value, Ymd, julian_to_ymd};

use crate::ast::{
    CastType, Catch, Delete, Expr, ExprKind, From, Insert, InsertSource, Item, Merge, OrderKey,
    ScoreSpec, ScriptStmt, Select, Statement, TypedLiteral, Update,
};
use crate::compile::{Compiler, Scope};
use crate::error::SqlError;
use crate::eval::{Env, Row, eval};
use crate::parse::ERROR_VARIABLES;
use crate::types::Ty;
use crate::value::V;

/// A script's variable: its synthetic field (its name and type, for
/// compiling and typing references to it) and its current value.
struct Var {
    field: Field,
    value: V,
    /// Whether it is a `STRING`, which is as long as the text it holds.
    string: bool,
}

/// The variables a running script holds: its parameters and `DECLARE`d
/// locals. Names resolve without regard to case, as the engine resolves
/// them, and a later declaration of a name shadows an earlier one.
#[derive(Default)]
pub struct Vars {
    vars: Vec<Var>,
}

impl Vars {
    pub fn new() -> Vars {
        Vars::default()
    }

    /// Declares the variable `name` of the declared type `ty` (as written),
    /// with no value yet (NULL).
    pub fn declare(&mut self, name: &str, ty: &str) -> Result<(), SqlError> {
        let field = declared_field(name, ty)?;
        self.push(field, V::Null, is_string(ty));
        Ok(())
    }

    /// Declares the variable `name` of type `ty` set to `value` (a
    /// parameter, as a routine starts): the value is coerced to the type.
    pub fn define(&mut self, name: &str, ty: &str, value: V) -> Result<(), SqlError> {
        let field = declared_field(name, ty)?;
        let value = coerce(value, &field);
        self.push(field, value, is_string(ty));
        Ok(())
    }

    fn push(&mut self, field: Field, value: V, string: bool) {
        // A fresh declaration of a name replaces the old one.
        let var = Var {
            field,
            value,
            string,
        };
        if let Some(i) = self.index(&var.field.name) {
            self.vars[i] = var;
        } else {
            self.vars.push(var);
        }
    }

    /// The field of the variable `name`, if it is declared.
    fn field(&self, name: &str) -> Option<&Field> {
        self.index(name).map(|i| &self.vars[i].field)
    }

    /// Assigns `value` to the variable `name`, coerced to its type.
    pub fn assign(&mut self, name: &str, value: V) -> Result<(), SqlError> {
        let i = self
            .index(name)
            .ok_or_else(|| SqlError::column_not_found(name, 0))?;
        self.vars[i].value = coerce(value, &self.vars[i].field);
        Ok(())
    }

    /// The value of the variable `name`, if it is declared.
    pub fn get(&self, name: &str) -> Option<&V> {
        self.index(name).map(|i| &self.vars[i].value)
    }

    /// The type of the variable `name`, if it is declared.
    fn ty(&self, name: &str) -> Option<Ty> {
        self.index(name).map(|i| Ty::of_field(&self.vars[i].field))
    }

    /// The variable `name` as an expression a statement of the script can
    /// hold in its place: its value as a literal of the variable's type (a
    /// `CAST` where the literal's own type is another), and a NULL as a NULL
    /// of the type.
    pub fn expr(&self, name: &str) -> Option<ExprKind> {
        let i = self.index(name)?;
        let var = &self.vars[i];
        // A `STRING` is as long as its text; without any, one character.
        let to = match var.string {
            true => (var.value.is_null()).then_some(CastType::VarChar(Some(1))),
            false => cast_type(&var.field),
        };
        let typed = !matches!(
            var.field.field_type,
            FieldType::Integer
                | FieldType::Logical
                | FieldType::Date
                | FieldType::Time
                | FieldType::TimeStamp
        );
        Some(match (&var.value, to) {
            (V::Null, Some(to)) => cast(ExprKind::Null, to),
            (value, Some(to)) if typed => cast(literal_kind(value), to),
            (value, _) => literal_kind(value),
        })
    }

    fn index(&self, name: &str) -> Option<usize> {
        self.vars
            .iter()
            .position(|v| v.field.name.eq_ignore_ascii_case(name))
    }

    /// The variables as a [`Scope`], so expressions that read them compile
    /// their names to columns. The scope has no table name: references to
    /// the variables are unqualified.
    pub fn scope(&self) -> Scope<'_> {
        let fields: Vec<Field> = self.vars.iter().map(|v| v.field.clone()).collect();
        Scope {
            name: "",
            alias: None,
            fields: Cow::Owned(fields),
            fts: std::rc::Rc::new(Vec::new()),
            info: crate::compile::TableInfo::default(),
        }
    }

    /// A [`Row`] over the variables' values, for evaluating the expressions
    /// compiled against [`Vars::scope`].
    pub fn row(&self) -> VarRow<'_> {
        VarRow { vars: self }
    }
}

/// A [`Row`] of a script's variables: column `i` is the `i`th variable, in
/// the order [`Vars::scope`] lists them.
pub struct VarRow<'a> {
    vars: &'a Vars,
}

impl Row for VarRow<'_> {
    fn value(&mut self, index: usize) -> Result<Value, SqlError> {
        let var = &self.vars.vars[index];
        var.value
            .to_field(Ty::of_field(&var.field), &var.field.name)
    }

    fn column(&mut self, index: usize, _ty: Ty) -> Result<V, SqlError> {
        Ok(self
            .vars
            .vars
            .get(index)
            .map(|v| v.value.clone())
            .unwrap_or(V::Null))
    }
}

/// Where a script host runs the SQL statements a script contains, with its
/// variables in scope. The engine implements it over its tables; the
/// pseudo-tables the statement names (`__input`, `__output`, `__new`,
/// `__old`) are the host's to resolve.
pub trait Host {
    /// Runs `statement`, one of a script's SQL statements, with the script's
    /// variables `vars` available to the expressions in it.
    fn run(&mut self, statement: &Statement, vars: &Vars) -> Result<(), SqlError>;

    /// `expr`, one of the script's own expressions (a condition, an
    /// assignment), with what the host resolves in it replaced (a trigger's
    /// `__new.col`, the subqueries, which read the script's variables
    /// `vars`); `None` to leave it as it is.
    fn rewrite_expr(&self, _expr: &Expr, _vars: &Vars) -> Result<Option<Expr>, SqlError> {
        Ok(None)
    }

    /// The user-defined functions the script's own expressions may call.
    fn functions(&self) -> Vec<crate::compile::FunctionSig> {
        Vec::new()
    }

    /// Whether the errors of the script's own expressions say where they are
    /// (a function's evaluation errors are not located).
    fn locates_errors(&self) -> bool {
        true
    }

    /// The script goes on to another statement (or tests a loop's
    /// condition): what a script sent as a statement keeps as its result is
    /// the cursor of the last one it ran, so the host lets go of an earlier
    /// one.
    fn next_statement(&mut self) {}

    /// Runs the `SELECT` or `EXECUTE PROCEDURE` `def`, which defines a
    /// cursor, with the script's variables in scope, and gives its rows.
    fn query(&mut self, _def: &Statement, _vars: &Vars) -> Result<Query, SqlError> {
        Err(SqlError::unsupported("cursors in scripts", 0))
    }

    /// The script is about to run the SQL statement `statement`, written as
    /// `text`; `last` if it is the last of the script's body. The query log
    /// takes a row for it.
    fn statement_begins(&mut self, _text: &str, _statement: &Statement, _last: bool) {}

    /// The statement that began has ended, with the error that stopped it.
    fn statement_ends(&mut self, _result: Result<(), &SqlError>) {}

    /// The script opens a cursor: `declared` if its definition is the one
    /// `DECLARE` gave it.
    fn cursor_opens(&mut self, _declared: bool) {}

    /// The script has ended, with a statement the last thing it ran if `last`.
    fn script_ends(&mut self, _last: bool) {}

    /// The next row of the live cursor `cursor` (see [`Rows::Live`]), none
    /// at its end.
    fn fetch(&mut self, _cursor: usize) -> Result<Option<Vec<V>>, SqlError> {
        Ok(None)
    }

    /// The script closes the live cursor `cursor`.
    fn close(&mut self, _cursor: usize) {}
}

/// The rows a cursor of a script reads: its columns (names and types) and
/// the values of each row.
pub struct Query {
    pub columns: Vec<Field>,
    pub rows: Rows,
}

/// Where the rows of a cursor come from.
pub enum Rows {
    /// Taken at `OPEN`.
    Fixed(Vec<Vec<V>>),
    /// Read from the tables as each `FETCH` comes to them, by the host's
    /// cursor of this number, as the original's cursors are.
    Live(usize),
}

/// Why a script stopped running.
#[derive(Debug, Clone, PartialEq)]
pub enum Flow {
    /// It ran to the end of its body.
    Done,
    /// It hit `RETURN`, with the value given (a function's result), if any.
    Return(Option<V>),
    /// It hit `LEAVE`: the innermost loop ends.
    Leave,
    /// It hit `CONTINUE`: the innermost loop goes on with its next pass.
    Continue,
}

/// Runs the script `body` with the variables `vars` already holding its
/// parameters, the host `host` and the evaluation environment `env` (the
/// connection's date format and the time now). Returns how it stopped.
pub fn run(
    body: &[ScriptStmt],
    vars: &mut Vars,
    host: &mut dyn Host,
    env: &Env,
) -> Result<Flow, SqlError> {
    // No cursor's definition may have a parameter, which the script finds
    // before it runs anything.
    if let Some(at) = first_cursor_parameter(body) {
        return Err(SqlError::cursor_parameter(at));
    }
    for (name, ty) in ERROR_VARIABLES.iter().zip(["INTEGER", "STRING", "STRING"]) {
        vars.declare(name, ty)?;
    }
    let mut interp = Interp::new(vars, host, env, false, body.len() == 1);
    interp.last = body.last().map(|last| last as *const ScriptStmt);
    let flow = interp.block(body);
    let last = flow.is_ok() && interp.ran_sql;
    host.script_ends(last);
    flow
}

/// Whether the value of `expr`, which reads no table, can be assigned to the
/// column `name` of type `target` of a pseudo-table a script writes (a
/// trigger's `__error`), or the error of an operand it does not take.
pub fn check_column_value(expr: &Expr, target: Ty, name: &str) -> Result<(), SqlError> {
    // A NULL takes the column's type, and a subquery's is not known yet.
    if matches!(expr.kind, ExprKind::Null) || has_subquery(expr) {
        return Ok(());
    }
    let node = Compiler::new(None, 0).compile(expr)?;
    let reads = std::cell::Cell::new(false);
    rewrite_expr(expr, &|kind| {
        reads.set(reads.get() || matches!(kind, ExprKind::Column { .. }));
        None
    });
    crate::variable::check(target, node.ty, &expr.kind, reads.get(), name, expr.pos)
}

/// The suffix the original engine gives an error of a script statement:
/// where in the script it is (1-based).
fn located(pos: crate::error::Pos) -> String {
    crate::error::script_at(pos)
}

/// The error of the script statement at `pos`, located in the script unless
/// it says where it is already.
fn in_script(mut error: SqlError, pos: crate::error::Pos) -> SqlError {
    // A note with no place is how the original ends an error of the engine
    // that a procedure's statement had (`fixtures/sql/trigproc.json`).
    // The failure of a trigger that a statement fired is located in the
    // script it ran in too, whatever place its own words have.
    if (!error.message.contains(" -- Location of error") || error.native == 5177)
        && !error.message.ends_with(crate::error::SCRIPT_NOTE)
    {
        error.message.push_str(&located(pos));
    }
    error
}

struct Interp<'a> {
    vars: &'a mut Vars,
    host: &'a mut dyn Host,
    env: &'a Env,
    /// Whether the script is the text of an `EXECUTE IMMEDIATE`, which only
    /// checks the `SELECT`s it holds.
    immediate: bool,
    /// Whether the script is one statement, whose errors the original
    /// words without the script's own information.
    lone: bool,
    /// The last statement of the script's own body, which a `SELECT` is only
    /// run for the query log if it is.
    last: Option<*const ScriptStmt>,
    /// Whether the last thing the script ran is an SQL statement.
    ran_sql: bool,
    /// The cursors the script declared.
    cursors: Vec<Cursor>,
    /// The errors the `CATCH` blocks being run handle, innermost last.
    handling: Vec<SqlError>,
}

impl<'a> Interp<'a> {
    fn new(
        vars: &'a mut Vars,
        host: &'a mut dyn Host,
        env: &'a Env,
        immediate: bool,
        lone: bool,
    ) -> Self {
        Interp {
            vars,
            host,
            env,
            immediate,
            lone,
            last: None,
            ran_sql: false,
            cursors: Vec::new(),
            handling: Vec::new(),
        }
    }
}

/// A cursor of a script: its definition, if it has one yet, and its rows
/// while it is open.
struct Cursor {
    name: String,
    def: Option<Statement>,
    open: Option<Open>,
}

/// An open cursor, whose rows the script reads one at a time.
struct Open {
    columns: Vec<Field>,
    rows: Rows,
    /// The row the next `FETCH` of fixed rows reads.
    next: usize,
    /// The row `FETCH` read last, none before the first and after the last.
    current: Option<Vec<V>>,
}

impl Interp<'_> {
    fn block(&mut self, body: &[ScriptStmt]) -> Result<Flow, SqlError> {
        for stmt in body {
            match self.statement(stmt)? {
                Flow::Done => {}
                flow => return Ok(flow),
            }
        }
        Ok(Flow::Done)
    }

    fn statement(&mut self, stmt: &ScriptStmt) -> Result<Flow, SqlError> {
        if !matches!(stmt, ScriptStmt::Block(_)) {
            self.host.next_statement();
        }
        if !matches!(
            stmt,
            ScriptStmt::Block(_)
                | ScriptStmt::If { .. }
                | ScriptStmt::While { .. }
                | ScriptStmt::IfFetch { .. }
                | ScriptStmt::WhileFetch { .. }
                | ScriptStmt::Try { .. }
        ) {
            self.ran_sql = false;
        }
        match stmt {
            ScriptStmt::Declare { name, ty } => self.vars.declare(name, ty)?,
            ScriptStmt::Assign {
                name,
                value,
                pos,
                set,
            } => {
                let Some(target) = self.vars.ty(name) else {
                    return Err(SqlError::variable_not_found(name, *pos));
                };
                let value = self.assigned(value, target, name).map_err(|mut error| {
                    // A `SET` is a statement of its own, which words a
                    // value cut to fit without the variable's name.
                    if *set && error.native == 2102 {
                        let note = format!("{name}{}", crate::error::SCRIPT_NOTE);
                        error.message = error.message.replacen(&note, "", 1);
                    }
                    error
                })?;
                self.vars.assign(name, value)?;
            }
            ScriptStmt::If { cond, then, els } => {
                return match self.condition(cond)? {
                    true => self.block(then),
                    false => self.block(els),
                };
            }
            ScriptStmt::While { cond, body } => loop {
                self.host.next_statement();
                if !self.condition(cond)? {
                    break;
                }
                match self.block(body)? {
                    Flow::Done | Flow::Continue => {}
                    Flow::Leave => break,
                    Flow::Return(v) => return Ok(Flow::Return(v)),
                }
            },
            ScriptStmt::Block(body) => return self.block(body),
            ScriptStmt::Leave => return Ok(Flow::Leave),
            ScriptStmt::Continue => return Ok(Flow::Continue),
            ScriptStmt::Return(expr) => {
                let value = expr.as_ref().map(|e| self.eval(e)).transpose()?;
                return Ok(Flow::Return(value));
            }
            ScriptStmt::Raise {
                name,
                code,
                message,
                pos,
            } => {
                let (code, message) = (self.eval(code)?, self.eval(message)?);
                // The code is a number and the message text.
                let (V::Int(_), V::Str(_)) = (&code, &message) else {
                    return Err(SqlError::invalid_set_value());
                };
                return Err(SqlError::raised(
                    name,
                    &code.display(),
                    &message.display(),
                    *pos,
                ));
            }
            ScriptStmt::ExecuteImmediate { expr, pos } => self.execute_immediate(expr, *pos)?,
            ScriptStmt::Print { expr, pos } => {
                if !matches!(self.eval(expr)?, V::Str(_) | V::Null) {
                    return Err(SqlError::print_not_text(*pos));
                }
            }
            ScriptStmt::DeclareCursor { name, def, .. } => {
                self.cursors.push(Cursor {
                    name: name.to_lowercase(),
                    def: def.as_deref().cloned(),
                    open: None,
                });
            }
            ScriptStmt::OpenCursor { name, def, pos } => self.open_cursor(name, def, *pos)?,
            ScriptStmt::FetchCursor { name, pos } => {
                self.fetch(name, *pos)?;
            }
            ScriptStmt::CloseCursor { name, pos } => {
                let i = self.cursor(name);
                match self.cursors[i].open.take() {
                    Some(Open {
                        rows: Rows::Live(id),
                        ..
                    }) => self.host.close(id),
                    Some(_) => {}
                    None => return Err(SqlError::closed_cursor(name, Some(*pos))),
                }
            }
            ScriptStmt::IfFetch {
                name,
                negated,
                pos,
                then,
                els,
            } => {
                return match self.fetch(name, *pos)? != *negated {
                    true => self.block(then),
                    false => self.block(els),
                };
            }
            ScriptStmt::WhileFetch {
                name,
                negated,
                pos,
                body,
            } => loop {
                self.host.next_statement();
                if self.fetch(name, *pos)? == *negated {
                    break;
                }
                match self.block(body)? {
                    Flow::Done | Flow::Continue => {}
                    Flow::Leave => break,
                    Flow::Return(v) => return Ok(Flow::Return(v)),
                }
            },
            ScriptStmt::Reraise { pos } => {
                let Some(error) = self.handling.last() else {
                    return Err(SqlError::script_error(
                        "Raising a previously thrown exception is only allowed in the CATCH \
                         clause.",
                        *pos,
                    ));
                };
                let mut error = error.clone();
                // Passed on, an error the script located is at the `RAISE;`.
                if error.strip_script_location() {
                    error.message.push_str(&located(*pos));
                }
                return Err(error);
            }
            ScriptStmt::Try {
                body,
                catches,
                finally,
            } => return self.try_block(body, catches, finally.as_deref()),
            // The text of an `EXECUTE IMMEDIATE` is parsed, but its
            // `SELECT`s are not run (`fixtures/sql/scripts.json`).
            ScriptStmt::Sql(statement, ..)
                if self.immediate && matches!(**statement, Statement::Select(_)) => {}
            ScriptStmt::Sql(statement, pos, text) => {
                let resolved = self
                    .cursor_statement(statement, *pos)
                    .map_err(|e| in_script(e, *pos))?;
                let last = self.last.is_some_and(|last| std::ptr::eq(last, stmt));
                self.host.statement_begins(text, &resolved, last);
                let ran = self.host.run(&resolved, self.vars);
                self.host.statement_ends(ran.as_ref().map(|_| ()));
                self.ran_sql = true;
                ran.map_err(|e| in_script(e, *pos))?
            }
        }
        Ok(Flow::Done)
    }

    /// The index of the cursor `name`, which the parser has checked is there.
    fn cursor(&self, name: &str) -> usize {
        self.cursors
            .iter()
            .rposition(|c| c.name.eq_ignore_ascii_case(name))
            .expect("a declared cursor")
    }

    /// `OPEN name [AS def]`.
    fn open_cursor(
        &mut self,
        name: &str,
        def: &Option<Box<Statement>>,
        pos: crate::error::Pos,
    ) -> Result<(), SqlError> {
        let i = self.cursor(name);
        if self.cursors[i].open.is_some() {
            return Err(SqlError::cursor_open(name, pos));
        }
        let declared = def.is_none();
        if let Some(def) = def {
            self.cursors[i].def = Some((**def).clone());
        }
        let Some(def) = self.cursors[i].def.clone() else {
            return Err(SqlError::cursor_undefined(pos));
        };
        let def = self
            .cursor_statement(&def, pos)
            .map_err(|e| in_script(e, pos))?;
        self.host.cursor_opens(declared);
        let query = self
            .host
            .query(&def, self.vars)
            .map_err(|e| in_script(e, pos))?;
        self.cursors[i].open = Some(Open {
            columns: query.columns,
            rows: query.rows,
            next: 0,
            current: None,
        });
        Ok(())
    }

    /// `FETCH name`: whether there was a row to read.
    fn fetch(&mut self, name: &str, pos: crate::error::Pos) -> Result<bool, SqlError> {
        let i = self.cursor(name);
        let Some(open) = self.cursors[i].open.as_mut() else {
            return Err(SqlError::closed_cursor(name, Some(pos)));
        };
        let row = match &open.rows {
            Rows::Fixed(rows) => {
                let row = rows.get(open.next).cloned();
                open.next += usize::from(row.is_some());
                row
            }
            Rows::Live(id) => self.host.fetch(*id).map_err(|e| in_script(e, pos))?,
        };
        let found = row.is_some();
        open.current = row;
        Ok(found)
    }

    /// `input`, a statement or an expression, with the columns of cursors it
    /// reads (`cursor.column`) replaced by their values; `None` if it reads
    /// none. `rewrite` rewrites `input` with a replacement the way
    /// [`rewrite_at`] and [`rewrite_expr_at`] do, and `statement` is whether
    /// `input` is a statement, which words a missing column differently.
    fn resolve_cursors<T>(
        &self,
        input: &T,
        rewrite: impl Fn(&T, &dyn Fn(&Expr) -> Option<ExprKind>) -> T,
        statement: bool,
    ) -> Result<Option<T>, SqlError> {
        let is_cursor = |table: &str| {
            self.cursors
                .iter()
                .any(|c| c.name.eq_ignore_ascii_case(table))
        };
        let found = RefCell::new(Vec::new());
        rewrite(input, &|expr| {
            if let ExprKind::Column {
                table: Some(table),
                name,
            } = &expr.kind
                && is_cursor(table)
            {
                found
                    .borrow_mut()
                    .push((table.clone(), name.clone(), expr.pos));
            }
            None
        });
        let found = found.into_inner();
        if found.is_empty() {
            return Ok(None);
        }
        let mut values = Vec::new();
        for (table, column, pos) in &found {
            // Where the column's own name is, which the errors point at.
            let at = pos + table.chars().count() + 1;
            let rows = self.cursors[self.cursor(table)].open.as_ref();
            let Some(rows) = rows else {
                return Err(SqlError::closed_cursor(table, None));
            };
            let Some(i) = rows
                .columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(column))
            else {
                return Err(match statement {
                    true => SqlError::table_not_found(table, at),
                    false => SqlError::column_not_found(column, at),
                });
            };
            let Some(current) = &rows.current else {
                return Err(SqlError::cursor_no_row(table, at));
            };
            values.push(typed_value(&current[i], &rows.columns[i]));
        }
        let next = Cell::new(0);
        Ok(Some(rewrite(input, &|expr| match &expr.kind {
            ExprKind::Column {
                table: Some(table), ..
            } if is_cursor(table) => {
                let value = values[next.get()].clone();
                next.set(next.get() + 1);
                Some(value)
            }
            _ => None,
        })))
    }

    /// `statement` with the columns of cursors it reads replaced by their
    /// values.
    fn cursor_statement(
        &self,
        statement: &Statement,
        _pos: crate::error::Pos,
    ) -> Result<Statement, SqlError> {
        if self.cursors.is_empty() {
            return Ok(statement.clone());
        }
        Ok(self
            .resolve_cursors(statement, |s, f| rewrite_at(s, f), true)?
            .unwrap_or_else(|| statement.clone()))
    }

    /// `expr` with the columns of cursors it reads replaced by their values.
    fn cursor_expr<'e>(&self, expr: &'e Expr) -> Result<Cow<'e, Expr>, SqlError> {
        if self.cursors.is_empty() {
            return Ok(Cow::Borrowed(expr));
        }
        let resolved = self
            .resolve_cursors(expr, |e, f| rewrite_expr_at(e, f), false)
            .map_err(|e| in_script(e, expr.pos))?;
        Ok(match resolved {
            Some(resolved) => Cow::Owned(resolved),
            None => Cow::Borrowed(expr),
        })
    }

    /// `TRY body CATCH … FINALLY finally`.
    fn try_block(
        &mut self,
        body: &[ScriptStmt],
        catches: &[Catch],
        finally: Option<&[ScriptStmt]>,
    ) -> Result<Flow, SqlError> {
        let mut outcome = self.block(body);
        if let Err(error) = &outcome {
            let class = error_info(error).2;
            let handler = catches.iter().find(|c| {
                c.class
                    .as_ref()
                    .is_none_or(|name| name.eq_ignore_ascii_case(&class))
            });
            if let Some(handler) = handler {
                let error = error.clone();
                self.handling.push(error.clone());
                self.set_error(Some(&error));
                outcome = self.block(&handler.body);
                self.set_error(None);
                self.handling.pop();
            }
        }
        if let Some(finally) = finally {
            // An error nothing handled is still there for the `FINALLY`, and
            // stays: an error of the `FINALLY` itself, or its `RETURN`, does
            // not replace it.
            let pending = outcome.as_ref().err().cloned();
            self.set_error(pending.as_ref());
            let after = self.block(finally);
            self.set_error(None);
            if pending.is_none() {
                match after {
                    Err(error) => outcome = Err(error),
                    Ok(Flow::Done) => {}
                    Ok(flow) => outcome = Ok(flow),
                }
            }
        }
        outcome
    }

    /// Makes the error `error`, or none, what `__errcode`, `__errtext` and
    /// `__errclass` read.
    fn set_error(&mut self, error: Option<&SqlError>) {
        let (code, text, class) = match error.map(error_info) {
            Some((code, text, class)) => (V::Int(code), V::Str(text), V::Str(class)),
            None => (V::Null, V::Null, V::Null),
        };
        for (name, value) in ERROR_VARIABLES.iter().zip([code, text, class]) {
            let _ = self.vars.assign(name, value);
        }
    }

    /// `EXECUTE IMMEDIATE expr`: the text `expr` gives runs as a script of
    /// its own, which has none of this one's variables.
    fn execute_immediate(&mut self, expr: &Expr, pos: crate::error::Pos) -> Result<(), SqlError> {
        let text = match self.eval(expr)? {
            V::Str(text) => text,
            V::Null => {
                return Err(SqlError::script_error(
                    "The SQL statement expression is NULL.",
                    pos,
                ));
            }
            other => {
                let mut error =
                    SqlError::invalid_assignment("Character", other.type_name(), "<Unknown>", pos);
                // Among other statements, the error is the script's.
                if !self.lone {
                    error
                        .message
                        .truncate(error.message.rfind(" -- Location").unwrap_or(0));
                    error.message.push_str(&located(pos));
                }
                return Err(error);
            }
        };
        if text.trim().is_empty() {
            return Ok(());
        }
        let parsed = crate::parse::parse(&text).map_err(|e| e.in_immediate(pos))?;
        let body = match parsed.statement {
            Statement::Script(body) => body,
            single => vec![ScriptStmt::Sql(Box::new(single), 0, text.clone())],
        };
        Interp::new(&mut Vars::new(), &mut *self.host, self.env, true, false)
            .block(&body)
            .map_err(|e| e.in_immediate(pos))?;
        Ok(())
    }

    /// `expr` as the host rewrites it, with the columns of cursors it reads
    /// replaced by their values.
    fn prepared<'e>(&self, expr: &'e Expr) -> Result<Cow<'e, Expr>, SqlError> {
        let rewritten = self.host.rewrite_expr(expr, self.vars)?;
        let expr = rewritten.map_or(Cow::Borrowed(expr), Cow::Owned);
        let resolved = match self.cursor_expr(&expr)? {
            Cow::Owned(resolved) => Some(resolved),
            Cow::Borrowed(_) => None,
        };
        Ok(resolved.map_or(expr, Cow::Owned))
    }

    /// Evaluates `expr` over the script's variables.
    fn eval(&self, expr: &Expr) -> Result<V, SqlError> {
        let expr = &*self.prepared(expr)?;
        let scope = self.vars.scope();
        let run = || {
            let node = self.compiler(scope).compile(expr)?;
            eval(&node, &mut self.vars.row(), self.env)
        };
        run().map_err(|e| self.located(e, expr.pos))
    }

    /// A compiler of the script's expressions over `scope`.
    fn compiler<'s>(&self, scope: Scope<'s>) -> Compiler<'s> {
        let mut compiler = Compiler::new(Some(scope), self.env.params.len());
        compiler.functions = self.host.functions();
        compiler
    }

    /// `error` of the expression at `pos`, with where it is if the host says it.
    fn located(&self, error: SqlError, pos: crate::error::Pos) -> SqlError {
        match self.host.locates_errors() {
            true => at_expression(error, pos),
            false => error,
        }
    }

    /// Evaluates `expr` as the value of the variable `name` of type
    /// `target`: a NULL or a parameter takes its type, and a value of
    /// another kind is refused.
    fn assigned(&self, expr: &Expr, target: Ty, name: &str) -> Result<V, SqlError> {
        let subquery = has_subquery(expr);
        let expr = &*self.prepared(expr)?;
        let scope = self.vars.scope();
        let run = || {
            let node = self.compiler(scope).node_for(expr, target)?;
            let reads = std::cell::Cell::new(subquery);
            rewrite_expr(expr, &|kind| {
                reads.set(reads.get() || matches!(kind, ExprKind::Column { .. }));
                None
            });
            let at = match &expr.kind {
                ExprKind::Neg(inner) => inner.pos,
                ExprKind::Number(n) if n.starts_with('-') => expr.pos + 1,
                _ => expr.pos,
            };
            crate::variable::check(target, node.ty, &expr.kind, reads.get(), name, at)?;
            let value = eval(&node, &mut self.vars.row(), self.env)?;
            match self.vars.field(name) {
                Some(field) => crate::variable::convert(value, field, name, expr.pos, self.env),
                None => Ok(value),
            }
        };
        run().map_err(|e| self.located(e, expr.pos))
    }

    /// Evaluates the condition `expr`, which must be logical, as `IF` and
    /// `WHILE` do: a NULL condition is false.
    fn condition(&self, expr: &Expr) -> Result<bool, SqlError> {
        let subquery = has_subquery(expr);
        let expr = &*self.prepared(expr)?;
        let scope = self.vars.scope();
        let run = || {
            let node = self.compiler(scope).condition(expr)?;
            eval(&node, &mut self.vars.row(), self.env)
        };
        let value = run().map_err(|mut error| {
            // What a subquery gave is worked out as the script runs, and its
            // errors are worded with the script's location; so is a condition
            // that is none (`fixtures/sql/gram_syntax.json`).
            let none = error.native == 2124
                && !matches!(
                    expr.kind,
                    ExprKind::Number(_) | ExprKind::Str(_) | ExprKind::Bin(_)
                );
            if subquery || none {
                let inline = crate::error::at(expr.pos);
                if error.message.contains(&inline) {
                    error.message = error.message.replace(&inline, "");
                    error.message.push_str(&located(expr.pos));
                }
            }
            self.located(error, expr.pos)
        })?;
        Ok(matches!(value, V::Bool(true)))
    }
}

/// Where the first parameter in a cursor definition of `body` is, if there is
/// one.
fn first_cursor_parameter(body: &[ScriptStmt]) -> Option<crate::error::Pos> {
    body.iter().find_map(|stmt| match stmt {
        ScriptStmt::DeclareCursor { def, .. } | ScriptStmt::OpenCursor { def, .. } => {
            def.as_deref().and_then(first_parameter)
        }
        ScriptStmt::If { then, els, .. } | ScriptStmt::IfFetch { then, els, .. } => {
            first_cursor_parameter(then).or_else(|| first_cursor_parameter(els))
        }
        ScriptStmt::While { body, .. }
        | ScriptStmt::WhileFetch { body, .. }
        | ScriptStmt::Block(body) => first_cursor_parameter(body),
        ScriptStmt::Try {
            body,
            catches,
            finally,
        } => first_cursor_parameter(body)
            .or_else(|| catches.iter().find_map(|c| first_cursor_parameter(&c.body)))
            .or_else(|| finally.as_deref().and_then(first_cursor_parameter)),
        _ => None,
    })
}

/// Where the first parameter of the cursor definition `def` is, if it has
/// one.
fn first_parameter(def: &Statement) -> Option<crate::error::Pos> {
    let found = Cell::new(None);
    rewrite_at(def, &|expr| {
        if matches!(expr.kind, ExprKind::Param(_)) && found.get().is_none() {
            found.set(Some(expr.pos));
        }
        None
    });
    found.get()
}

/// Fails as the parameters of `statement`, of the types `types`, do when it
/// runs, for a statement whose rows are read later: the original reads each
/// parameter where it stands as the statement starts.
pub fn check_params(
    statement: &Statement,
    types: &[Ty],
    env: &crate::eval::Env,
) -> Result<(), SqlError> {
    let found = std::cell::RefCell::new(Vec::new());
    rewrite_at(statement, &|expr| {
        if let ExprKind::Param(i) = expr.kind {
            found.borrow_mut().push((i, expr.pos));
        }
        None
    });
    for (i, pos) in found.into_inner() {
        if let (Some(value), Some(ty)) = (env.params.get(i), types.get(i)) {
            crate::eval::param(value.clone(), *ty, pos, env)?;
        }
    }
    Ok(())
}

/// What `__errcode`, `__errtext` and `__errclass` give for `error`: a `RAISE`
/// gives its own code, message and exception name, and any other error its
/// native code, its states and message, in the class of the engine's errors.
fn error_info(error: &SqlError) -> (i64, String, String) {
    const RAISED: &str = "An exception is raised in the SQL script. {[";
    if let Some(rest) = error.message.strip_prefix(RAISED)
        && let Some(end) = rest.rfind("} ** Script error information:")
        && let Some((name, rest)) = rest[..end].split_once("] ")
        && let Some((code, text)) = rest.split_once(" : ")
    {
        return (code.parse().unwrap_or(0), text.to_owned(), name.to_owned());
    }
    let mut message = error.message.as_str();
    // What the script added to say where the statement was is not the
    // error's; nor is the location of an expression, set off by two spaces.
    for marker in [" ** Script error information:", "  -- Location of error"] {
        if let Some(i) = message.find(marker)
            && !message.contains(" - EXECUTE IMMEDIATE")
        {
            message = &message[..i];
        }
    }
    (
        i64::from(error.native),
        format!(
            "{} {} [SAP][Advantage SQL Engine]{message}",
            error.odbc2_state(),
            error.state
        ),
        "ADS_SCRIPT_EXCEPTION".to_owned(),
    )
}

/// Whether `expr` has a subquery in it, which a host runs as the script does.
pub fn has_subquery(expr: &Expr) -> bool {
    expr.any(&|e| {
        matches!(
            e.kind,
            ExprKind::Subquery(_)
                | ExprKind::Exists(_)
                | ExprKind::InSelect { .. }
                | ExprKind::Quantified { .. }
        )
    })
}

/// `error`, located at the expression at `pos` if it does not say where it is
/// already.
fn at_expression(mut error: SqlError, pos: crate::error::Pos) -> SqlError {
    if !error.message.contains(" -- Location of error") {
        error.message.push(' ');
        error.message.push_str(&crate::error::at(pos));
    }
    error
}

/// Coerces `value` to the type of `field`, as assigning it to that variable
/// does, through the field's storage representation (as the engine stores it
/// in the variable). A value that does not convert is left as it is.
fn coerce(value: V, field: &Field) -> V {
    if value.is_null() {
        return V::Null;
    }
    let ty = Ty::of_field(field);
    match value.to_field(ty, &field.name) {
        Ok(stored) => V::from_field(stored, ty),
        Err(_) => value,
    }
}

/// The synthetic field for the variable `name` of the declared type `ty` (as
/// written, e.g. `INTEGER`, `CHAR(10)`, `NUMERIC(10,2)`, `STRING`). Its type
/// decides how references to the variable are typed and how values assigned
/// to it are coerced, and the engine builds a procedure's output columns from
/// its output parameters' fields this way.
pub fn declared_field(name: &str, ty: &str) -> Result<Field, SqlError> {
    let (field_type, length, decimals) = declared_type(ty)?;
    Ok(adt::field(name, field_type, 0, length, decimals))
}

/// The field type, length and decimals of the declared type `ty`.
fn declared_type(ty: &str) -> Result<(FieldType, u16, u16), SqlError> {
    let ty = ty.trim();
    // The base name, and the numbers in parentheses after it.
    let (base, nums) = match ty.split_once('(') {
        Some((base, rest)) => {
            let inside = rest.strip_suffix(')').unwrap_or(rest);
            let nums: Vec<u16> = inside
                .split(',')
                .filter_map(|p| p.trim().parse().ok())
                .collect();
            (base.trim(), nums)
        }
        None => (ty, Vec::new()),
    };
    let base = base.trim().to_ascii_uppercase();
    let base = base.strip_prefix("SQL_").unwrap_or(&base);
    let length = nums.first().copied();
    let decimals = nums.get(1).copied().unwrap_or(0);
    Ok(match base {
        "INTEGER" | "INT" | "AUTOINC" => (FieldType::Integer, 4, 0),
        "SHORT" | "SMALLINT" | "TINYINT" | "SHORTINT" => (FieldType::ShortInt, 2, 0),
        "BIGINT" | "LONG" | "LONGINT" | "LONGINTEGER" => (FieldType::LongInt, 8, 0),
        // A double's decimals show as 15, and a currency double's as given.
        "DOUBLE" | "FLOAT" | "REAL" => (FieldType::Double, 8, length.unwrap_or(15)),
        "CURDOUBLE" => (FieldType::CurDouble, 8, length.unwrap_or(2)),
        "MONEY" | "CURRENCY" => (FieldType::Money, 8, 4),
        "LOGICAL" | "BIT" | "BOOLEAN" => (FieldType::Logical, 1, 0),
        "DATE" => (FieldType::Date, 4, 0),
        "TIME" => (FieldType::Time, 4, 0),
        // A variable of a modification time is a plain timestamp.
        "TIMESTAMP" | "DATETIME" | "MODTIME" => (FieldType::TimeStamp, 8, 0),
        "ROWVERSION" => (FieldType::RowVersion, 8, 0),
        "GUID" | "UNIQUEIDENTIFIER" => (FieldType::Guid, 16, 0),
        "NUMERIC" | "DECIMAL" => {
            let precision = length.unwrap_or(20);
            // An ADT Numeric keeps a place for the sign and the point, and
            // is no longer than 32.
            (
                FieldType::Numeric,
                (precision + 1 + u16::from(decimals > 0)).min(32),
                decimals,
            )
        }
        // No size at all is one character.
        "CHAR" | "CHARACTER" => (FieldType::Character, length.unwrap_or(1).max(1), 0),
        "CICHAR" | "CICHARACTER" => (FieldType::CiCharacter, length.unwrap_or(1).max(1), 0),
        "VARCHAR" | "VARCHARFOX" => (FieldType::VarChar, length.unwrap_or(1).max(1), 0),
        "NCHAR" => (FieldType::NChar, length.unwrap_or(1).max(1), 0),
        "NVARCHAR" => (FieldType::NVarChar, length.unwrap_or(1).max(1), 0),
        // STRING is a variable-length character variable, like a memo.
        "STRING" | "MEMO" | "LONGVARCHAR" => (FieldType::Memo, 9, 0),
        "NMEMO" => (FieldType::NMemo, 9, 0),
        // Of a dictionary's types, a BLOB is `BINARY` with no size.
        "BINARY" if length.is_none() => (FieldType::Binary, 9, 0),
        "BINARY" | "RAW" => (FieldType::Raw, length.unwrap_or(1), 0),
        "VARBINARY" | "VARBINARYFOX" => (FieldType::VarBinary, length.unwrap_or(1), 0),
        "BLOB" | "IMAGE" | "LONGVARBINARY" => (FieldType::Binary, 9, 0),
        _ => return Err(SqlError::unsupported(&format!("the variable type {ty}"), 0)),
    })
}

/// `statement` with every reference to one of the script's variables (an
/// unqualified name `vars` declares) replaced by that variable's current
/// value as a literal, so the engine can run it through its ordinary path,
/// without knowing the script's variables. A variable shadows a table column
/// of the same name; qualified names (`t.col`) and names `vars` does not
/// declare are left alone, as are the pseudo-tables (`__input`, `__output`,
/// `__new`, `__old`) the host resolves.
pub fn substitute(statement: &Statement, vars: &Vars) -> Statement {
    rewrite(statement, &|kind| match kind {
        ExprKind::Column { table: None, name } => vars.expr(name),
        _ => None,
    })
}

/// The type `CAST` gives a NULL of the variable `field`'s type.
fn cast_type(field: &Field) -> Option<CastType> {
    let length = Some(field.length());
    Some(match field.field_type {
        FieldType::Integer | FieldType::AutoInc => CastType::Integer,
        FieldType::ShortInt => CastType::SmallInt,
        FieldType::LongInt => CastType::BigInt,
        FieldType::Double => CastType::Double,
        FieldType::CurDouble => CastType::Double,
        FieldType::Money => CastType::Money,
        FieldType::Logical => CastType::Bit,
        FieldType::Date => CastType::Date,
        FieldType::Time => CastType::Time,
        FieldType::TimeStamp => CastType::TimeStamp,
        FieldType::Guid => CastType::Guid,
        FieldType::Numeric => match Ty::of_field(field) {
            Ty::Num { precision, scale } => CastType::Numeric(Some((precision, scale))),
            _ => return None,
        },
        FieldType::RowVersion => CastType::BigInt,
        FieldType::Character => CastType::Char(length),
        FieldType::CiCharacter => CastType::CiChar(length),
        FieldType::VarChar => CastType::VarChar(length),
        FieldType::Memo => CastType::LongVarChar,
        FieldType::NChar => CastType::WChar(length),
        FieldType::NVarChar => CastType::WVarChar(length),
        FieldType::NMemo => CastType::WLongVarChar,
        FieldType::Raw => CastType::Binary(length),
        FieldType::VarBinary => CastType::VarBinary(length),
        FieldType::Binary => CastType::LongVarBinary,
        _ => return None,
    })
}

/// `expr` as `CAST(expr AS to)`.
fn cast(expr: ExprKind, to: CastType) -> ExprKind {
    ExprKind::Cast {
        expr: Box::new(Expr { kind: expr, pos: 0 }),
        to,
        function: "CAST",
    }
}

/// `value`, a text, as the date, time or timestamp of the type `field` has, if it
/// reads as one.
pub fn text_value(value: V, field: &Field, env: &Env) -> V {
    from_text(value, Ty::of_field(field), env)
}

/// `value`, an argument at `pos` of a procedure whose input parameter is
/// `field`, as the parameter takes it, or the error of one it does not take.
pub fn checked_input(
    value: V,
    field: &Field,
    pos: crate::error::Pos,
    env: &Env,
) -> Result<V, SqlError> {
    crate::variable::input(value, field, pos, env)
}

/// Whether `value`, an argument of a procedure whose input parameter is
/// `field`, is whole, as an integer parameter needs it: the error of a number
/// with decimals.
pub fn whole_input(value: &V, field: &Field) -> Result<(), SqlError> {
    match !value.is_whole() && matches!(Ty::of_field(field), Ty::Int { .. }) {
        true => Err(SqlError::input_not_whole()),
        false => Ok(()),
    }
}

/// [`text_value`] for the type `ty`.
pub fn from_text(value: V, ty: Ty, env: &Env) -> V {
    crate::variable::from_text(value, ty, env)
}

/// `value` as the value of a variable of `field`'s type.
pub fn coerced(value: V, field: &Field) -> V {
    coerce(value, field)
}

/// `value` as a literal of the type `to` (a `NULL` of it for no value), as a
/// host puts a system variable in a statement.
pub fn typed(value: &V, to: CastType) -> ExprKind {
    cast(literal_kind(value), to)
}

/// Whether the declared type `ty` is a `STRING`.
fn is_string(ty: &str) -> bool {
    ty.trim()
        .split('(')
        .next()
        .is_some_and(|base| base.trim().eq_ignore_ascii_case("STRING"))
}

/// Checks the declared type `name` with the numbers in parentheses after it,
/// `sizes`, of the variable at `pos`, as the original does when it parses a
/// script (`fixtures/sql/declare.json`): the names a variable may have, how
/// many sizes each takes and how large they may be.
pub(crate) fn check_declared_type(
    name: &str,
    sizes: &[u32],
    pos: crate::error::Pos,
) -> Result<(), SqlError> {
    let upper = name.to_ascii_uppercase();
    let wrong_count = || SqlError::procedure_parameters(&upper, pos);
    let out_of_range = |ok: bool| match ok {
        true => Ok(()),
        false => Err(SqlError::out_of_range()),
    };
    match upper.as_str() {
        "AUTOINC" => Err(SqlError::declaration(
            "The autoincrement data type is not a valid variable data type.",
            pos,
        )),
        "INTEGER" | "SHORT" | "LONGINT" | "MONEY" | "LOGICAL" | "DATE" | "TIME" | "TIMESTAMP"
        | "GUID" | "MEMO" | "NMEMO" | "BLOB" | "ROWVERSION" | "MODTIME" => match sizes.is_empty() {
            true => Ok(()),
            false => Err(wrong_count()),
        },
        "DOUBLE" | "CURDOUBLE" | "STRING" => Ok(()),
        "CHAR" | "CICHAR" | "VARCHAR" | "RAW" | "VARBINARY" | "NCHAR" | "NVARCHAR" => {
            let [size] = sizes else {
                return Err(wrong_count());
            };
            let largest = match upper.starts_with('N') {
                true => 32500,
                false => 65530,
            };
            out_of_range(*size <= largest)
        }
        "NUMERIC" => {
            let [precision, scale] = sizes else {
                return Err(wrong_count());
            };
            out_of_range(*precision <= 32 && scale <= precision)
        }
        _ => Err(SqlError::no_such_type(name).located_at(pos)),
    }
}

/// `statement` with each expression `replace` gives a replacement for swapped
/// for it (and no deeper recursion into the one replaced). The general form
/// behind [`substitute`]: the engine uses it to substitute a trigger's `__new`
/// and `__old` column references too.
pub fn rewrite(
    statement: &Statement,
    replace: &dyn Fn(&ExprKind) -> Option<ExprKind>,
) -> Statement {
    rewrite_at(statement, &|expr| replace(&expr.kind))
}

/// [`rewrite`] where the replacement sees the whole expression, its position
/// too.
fn rewrite_at(statement: &Statement, replace: &dyn Fn(&Expr) -> Option<ExprKind>) -> Statement {
    let mut statement = statement.clone();
    visit_statement(&mut statement, replace);
    statement
}

thread_local! {
    /// Where the expression `replace` is asked about is.
    static AT: std::cell::Cell<crate::error::Pos> = const { std::cell::Cell::new(0) };
}

/// Where the expression is that the function given to [`rewrite`] or
/// [`rewrite_expr`] is asked about.
pub fn replacing_at() -> crate::error::Pos {
    AT.get()
}

/// `error` of the expression at `pos` of a script, which says where it is
/// as the original says it of a failing script statement, unless it does
/// already.
pub fn in_script_at(error: SqlError, pos: crate::error::Pos) -> SqlError {
    in_script(error, pos)
}

/// The expression for `value`, of the column `field`: a literal of its type.
pub fn typed_value(value: &V, field: &Field) -> ExprKind {
    match cast_type(field) {
        Some(to) => cast(literal_kind(value), to),
        None => literal_kind(value),
    }
}

/// [`rewrite`] for one expression.
pub fn rewrite_expr(expr: &Expr, replace: &dyn Fn(&ExprKind) -> Option<ExprKind>) -> Expr {
    rewrite_expr_at(expr, &|expr| replace(&expr.kind))
}

/// [`rewrite_expr`] where the replacement sees the whole expression.
fn rewrite_expr_at(expr: &Expr, replace: &dyn Fn(&Expr) -> Option<ExprKind>) -> Expr {
    let mut expr = expr.clone();
    visit_expr(&mut expr, replace);
    expr
}

/// The value `value` as a literal expression, for [`substitute`].
pub fn literal(value: &V) -> Expr {
    Expr {
        kind: literal_kind(value),
        pos: 0,
    }
}

/// The value `value` as a literal expression kind, for a `rewrite` closure.
pub fn literal_of(value: &V) -> ExprKind {
    literal_kind(value)
}

fn literal_kind(value: &V) -> ExprKind {
    match value {
        V::Null => ExprKind::Null,
        V::Blank(blank) => literal_kind(&blank.value),
        V::Bool(b) => ExprKind::Bool(*b),
        V::Str(s) => ExprKind::Str(s.clone()),
        V::Bin(b) => ExprKind::Bin(b.clone()),
        V::Int(n) => signed_number(i128::from(*n)),
        V::Num(..) | V::Money(_) | V::Double(_) => signed_text(value.display()),
        V::Date(d) => ExprKind::Typed(TypedLiteral::Date, iso_date(*d)),
        V::Time(t) => ExprKind::Typed(TypedLiteral::Time, iso_time(*t)),
        V::TimeStamp(d, t) => ExprKind::Typed(
            TypedLiteral::TimeStamp,
            format!("{} {}", iso_date(*d), iso_time(*t)),
        ),
        V::Guid(g) => ExprKind::Str(guid(g)),
    }
}

/// A number literal, negated if `n` is negative (a `Number` is unsigned).
fn signed_number(n: i128) -> ExprKind {
    signed_text(n.to_string())
}

fn signed_text(text: String) -> ExprKind {
    match text.strip_prefix('-') {
        Some(rest) => ExprKind::Neg(Box::new(Expr {
            kind: number(rest),
            pos: 0,
        })),
        None => number(&text),
    }
}

fn number(text: &str) -> ExprKind {
    ExprKind::Number(text.to_owned())
}

/// A Julian day as `YYYY-MM-DD`.
fn iso_date(julian: i32) -> String {
    let Ymd { year, month, day } = julian_to_ymd(julian);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Milliseconds since midnight as `HH:MM:SS.fff`.
fn iso_time(ms: u32) -> String {
    let (ms, seconds) = (ms % 1000, ms / 1000);
    let (seconds, minutes, hours) = (seconds % 60, seconds / 60 % 60, seconds / 3600);
    format!("{hours:02}:{minutes:02}:{seconds:02}.{ms:03}")
}

/// A GUID in the canonical `8-4-4-4-12` form.
fn guid(bytes: &[u8; 16]) -> String {
    let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn visit_statement(statement: &mut Statement, replace: &dyn Fn(&Expr) -> Option<ExprKind>) {
    match statement {
        Statement::Select(select) => visit_select(select, replace),
        Statement::Insert(insert) => visit_insert(insert, replace),
        Statement::Update(update) => visit_update(update, replace),
        Statement::Delete(delete) => visit_delete(delete, replace),
        Statement::Merge(merge) => visit_merge(merge, replace),
        Statement::ShowPlan(inner, _) => visit_statement(inner, replace),
        Statement::SetVariable(set) => visit_expr(&mut set.value, replace),
        Statement::Execute(execute) => {
            for arg in &mut execute.args {
                visit_expr(arg, replace);
            }
        }
        // These hold no expressions that read a script's variables.
        Statement::Debug(..)
        | Statement::CachePrepare(_)
        | Statement::CreateTable(_)
        | Statement::CreateIndex(_)
        | Statement::DropTable(_)
        | Statement::DropIndex(..)
        | Statement::AlterTable(_)
        | Statement::Transaction(_)
        | Statement::CreateTrigger(_)
        | Statement::DropTrigger(..)
        | Statement::CreateView(_)
        | Statement::DropView(_)
        | Statement::CreateDatabase(_)
        | Statement::CreateProcedure(_)
        | Statement::DropProcedure(_)
        | Statement::FunctionDdl(_)
        | Statement::Grant(_)
        | Statement::Script(_) => {}
    }
}

fn visit_insert(insert: &mut Insert, replace: &dyn Fn(&Expr) -> Option<ExprKind>) {
    match &mut insert.source {
        InsertSource::Values(rows) => {
            for row in rows {
                for expr in row {
                    visit_expr(expr, replace);
                }
            }
        }
        InsertSource::Select(select) => visit_select(select, replace),
    }
}

fn visit_update(update: &mut Update, replace: &dyn Fn(&Expr) -> Option<ExprKind>) {
    for set in &mut update.sets {
        visit_expr(&mut set.value, replace);
    }
    if let Some(from) = &mut update.from {
        visit_from(from, replace);
    }
    if let Some(filter) = &mut update.filter {
        visit_expr(filter, replace);
    }
}

fn visit_merge(merge: &mut Merge, replace: &dyn Fn(&Expr) -> Option<ExprKind>) {
    if let Some(source) = &mut merge.source {
        visit_from(source, replace);
    }
    visit_expr(&mut merge.on, replace);
    if let Some(update) = &mut merge.update {
        for set in &mut update.sets {
            visit_expr(&mut set.value, replace);
        }
    }
    if let Some(insert) = &mut merge.insert {
        match &mut insert.source {
            InsertSource::Values(rows) => {
                for expr in rows.iter_mut().flatten() {
                    visit_expr(expr, replace);
                }
            }
            InsertSource::Select(select) => visit_select(select, replace),
        }
    }
}

fn visit_delete(delete: &mut Delete, replace: &dyn Fn(&Expr) -> Option<ExprKind>) {
    if let Some(filter) = &mut delete.filter {
        visit_expr(filter, replace);
    }
}

fn visit_select(select: &mut Select, replace: &dyn Fn(&Expr) -> Option<ExprKind>) {
    for item in &mut select.items {
        if let Item::Expr { expr, .. } = item {
            visit_expr(expr, replace);
        }
    }
    visit_from(&mut select.from, replace);
    for expr in select.filter.iter_mut().chain(&mut select.having) {
        visit_expr(expr, replace);
    }
    let order_keys = select.order.iter_mut().map(|o| &mut o.key);
    for key in select.group.iter_mut().chain(order_keys) {
        if let OrderKey::Expr(expr) = key {
            visit_expr(expr, replace);
        }
    }
    for (_, union) in &mut select.unions {
        visit_select(union, replace);
    }
}

fn visit_from(from: &mut From, replace: &dyn Fn(&Expr) -> Option<ExprKind>) {
    match from {
        From::Table(_) => {}
        From::Procedure { execute, .. } => {
            for arg in &mut execute.args {
                visit_expr(arg, replace);
            }
        }
        From::Derived { select, .. } => visit_select(select, replace),
        From::Join {
            left, right, on, ..
        } => {
            visit_from(left, replace);
            visit_from(right, replace);
            if let Some(on) = on {
                visit_expr(on, replace);
            }
        }
    }
}

/// Replaces `expr` with a literal if it reads a script variable, else
/// recurses into it. A mutable mirror of [`Expr::walk`] that also descends
/// into subqueries.
fn visit_expr(expr: &mut Expr, replace: &dyn Fn(&Expr) -> Option<ExprKind>) {
    AT.set(expr.pos);
    if let Some(kind) = replace(expr) {
        expr.kind = kind;
        return;
    }
    match &mut expr.kind {
        ExprKind::Number(_)
        | ExprKind::Str(_)
        | ExprKind::Bin(_)
        | ExprKind::Bool(_)
        | ExprKind::Null
        | ExprKind::Default
        | ExprKind::Typed(..)
        | ExprKind::Column { .. }
        | ExprKind::Param(_) => {}
        ExprKind::Subquery(select) | ExprKind::Exists(select) => visit_select(select, replace),
        ExprKind::Neg(a) | ExprKind::Plus(a) | ExprKind::BitNot(a) | ExprKind::Not(a) => {
            visit_expr(a, replace)
        }
        ExprKind::Binary(_, a, b) | ExprKind::Position(a, b) => {
            visit_expr(a, replace);
            visit_expr(b, replace);
        }
        ExprKind::IsNull { expr, .. }
        | ExprKind::Collate { expr, .. }
        | ExprKind::Cast { expr, .. }
        | ExprKind::Extract { expr, .. } => visit_expr(expr, replace),
        ExprKind::InSelect { expr, select, .. } | ExprKind::Quantified { expr, select, .. } => {
            visit_expr(expr, replace);
            visit_select(select, replace);
        }
        ExprKind::Between {
            expr, low, high, ..
        } => {
            visit_expr(expr, replace);
            visit_expr(low, replace);
            visit_expr(high, replace);
        }
        ExprKind::In { expr, list, .. } => {
            visit_expr(expr, replace);
            list.iter_mut().for_each(|e| visit_expr(e, replace));
        }
        ExprKind::Like {
            expr,
            pattern,
            escape,
            ..
        } => {
            visit_expr(expr, replace);
            visit_expr(pattern, replace);
            if let Some(escape) = escape {
                visit_expr(escape, replace);
            }
        }
        ExprKind::Case {
            operand,
            whens,
            otherwise,
        } => {
            if let Some(operand) = operand {
                visit_expr(operand, replace);
            }
            for (when, then) in whens {
                visit_expr(when, replace);
                visit_expr(then, replace);
            }
            if let Some(otherwise) = otherwise {
                visit_expr(otherwise, replace);
            }
        }
        ExprKind::Func { args, .. } => args.iter_mut().for_each(|e| visit_expr(e, replace)),
        ExprKind::Contains {
            field, condition, ..
        } => {
            if let Some(field) = field {
                visit_expr(field, replace);
            }
            visit_expr(condition, replace);
        }
        ExprKind::Score { spec, .. } => {
            if let ScoreSpec::Field {
                field, condition, ..
            } = spec
            {
                if let Some(field) = field {
                    visit_expr(field, replace);
                }
                visit_expr(condition, replace);
            }
        }
        ExprKind::Trim { chars, expr, .. } => {
            if let Some(chars) = chars {
                visit_expr(chars, replace);
            }
            visit_expr(expr, replace);
        }
        ExprKind::Aggregate { arg, .. } => {
            if let Some(arg) = arg {
                visit_expr(arg, replace);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::InsertSource;
    use crate::parse::parse_script;

    /// A host that collects the values a script writes to `__output` (its
    /// `VALUES` rows) and the names of the tables its other statements name.
    #[derive(Default)]
    struct Mock {
        outputs: Vec<V>,
        tables: Vec<String>,
    }

    impl Host for Mock {
        fn run(&mut self, statement: &Statement, vars: &Vars) -> Result<(), SqlError> {
            if let Statement::Insert(insert) = statement {
                self.tables.push(insert.table.name.to_ascii_lowercase());
                if insert.table.name.eq_ignore_ascii_case("__output")
                    && let InsertSource::Values(rows) = &insert.source
                {
                    for row in rows {
                        for expr in row {
                            let node = Compiler::new(Some(vars.scope()), 0).compile(expr)?;
                            self.outputs
                                .push(eval(&node, &mut vars.row(), &Env::default())?);
                        }
                    }
                }
            }
            Ok(())
        }
    }

    fn body(src: &str) -> Vec<ScriptStmt> {
        parse_script(src).unwrap_or_else(|e| panic!("parse {src:?}: {e:?}"))
    }

    /// Runs `body` with `params` (name, type, value) already in scope.
    fn exec(src: &str, params: &[(&str, &str, V)]) -> (Flow, Mock) {
        let mut vars = Vars::new();
        for (name, ty, value) in params {
            vars.define(name, ty, value.clone()).unwrap();
        }
        let mut host = Mock::default();
        let flow = run(&body(src), &mut vars, &mut host, &Env::default()).unwrap();
        (flow, host)
    }

    #[test]
    fn a_while_loop_writes_each_number() {
        // numbers(3): i goes 1, 2, 3 into __output.
        let src = " DECLARE i INTEGER; i = 0; \
                    WHILE i < _n DO i = i + 1; INSERT INTO __output VALUES (i); END WHILE; ";
        let (flow, host) = exec(src, &[("_n", "INTEGER", V::Int(3))]);
        assert_eq!(flow, Flow::Done);
        assert_eq!(host.outputs, vec![V::Int(1), V::Int(2), V::Int(3)]);
    }

    #[test]
    fn a_zero_count_loop_writes_nothing() {
        let src = " DECLARE i INTEGER; i = 0; \
                    WHILE i < _n DO i = i + 1; INSERT INTO __output VALUES (i); END WHILE; ";
        let (_, host) = exec(src, &[("_n", "INTEGER", V::Int(0))]);
        assert!(host.outputs.is_empty());
    }

    #[test]
    fn a_function_returns_an_expression() {
        // sq(i): RETURN i * i.
        let (flow, _) = exec(" RETURN i * i; ", &[("i", "INTEGER", V::Int(3))]);
        assert_eq!(flow, Flow::Return(Some(V::Int(9))));
    }

    #[test]
    fn a_function_pads_with_control_flow() {
        // util.pad(i, len): left-pad the number with zeros to `len`.
        let src = " DECLARE s STRING; s = TRIM(CAST(i AS SQL_CHAR)); \
                    IF LENGTH(s) < len THEN s = REPEAT('0', len - LENGTH(s)) + s; END IF; \
                    RETURN s; ";
        let (flow, _) = exec(
            src,
            &[("i", "INTEGER", V::Int(2)), ("len", "INTEGER", V::Int(4))],
        );
        assert_eq!(flow, Flow::Return(Some(V::Str("0002".to_owned()))));
        // A number already long enough is returned unchanged.
        let (flow, _) = exec(
            src,
            &[
                ("i", "INTEGER", V::Int(12345)),
                ("len", "INTEGER", V::Int(4)),
            ],
        );
        assert_eq!(flow, Flow::Return(Some(V::Str("12345".to_owned()))));
    }

    #[test]
    fn if_without_else_falls_through() {
        let src = " DECLARE s STRING; s = 'x'; IF 1 = 2 THEN s = 'y'; END IF; RETURN s; ";
        let (flow, _) = exec(src, &[]);
        assert_eq!(flow, Flow::Return(Some(V::Str("x".to_owned()))));
    }

    #[test]
    fn a_procedure_runs_its_statements_in_order() {
        // add_item: INSERT INTO items, then INSERT INTO __output.
        let src = " INSERT INTO items (id, name, qty) VALUES (_id, _name, _qty); \
                    INSERT INTO __output VALUES (_qty); ";
        let (_, host) = exec(
            src,
            &[
                ("_id", "INTEGER", V::Int(1)),
                ("_name", "CHAR(10)", V::Str("apple".to_owned())),
                ("_qty", "INTEGER", V::Int(5)),
            ],
        );
        assert_eq!(host.tables, vec!["items", "__output"]);
        assert_eq!(host.outputs, vec![V::Int(5)]);
    }

    #[test]
    fn an_assignment_coerces_to_the_variable_type() {
        // An INTEGER variable truncates a fractional value assigned to it.
        let src = " DECLARE n INTEGER; n = 7 / 2; RETURN n; ";
        let (flow, _) = exec(src, &[]);
        assert_eq!(flow, Flow::Return(Some(V::Int(3))));
    }

    #[test]
    fn a_declared_variable_starts_null() {
        let src = " DECLARE n INTEGER; RETURN n; ";
        let (flow, _) = exec(src, &[]);
        assert_eq!(flow, Flow::Return(Some(V::Null)));
    }

    #[test]
    fn assigning_an_undeclared_variable_fails() {
        let mut vars = Vars::new();
        let mut host = Mock::default();
        let err = run(&body(" x = 1; "), &mut vars, &mut host, &Env::default()).unwrap_err();
        assert_eq!(err.native, 2218);
    }

    fn vars(params: &[(&str, &str, V)]) -> Vars {
        let mut vars = Vars::new();
        for (name, ty, value) in params {
            vars.define(name, ty, value.clone()).unwrap();
        }
        vars
    }

    /// The one statement `sql` parses into.
    fn stmt(sql: &str) -> Statement {
        crate::parse(sql)
            .unwrap_or_else(|e| panic!("parse {sql:?}: {e:?}"))
            .statement
    }

    #[test]
    fn substitutes_variables_in_insert_values() {
        let vars = vars(&[
            ("_id", "INTEGER", V::Int(1)),
            ("_name", "CHAR(10)", V::Str("apple".to_owned())),
            ("_qty", "INTEGER", V::Int(5)),
        ]);
        let out = substitute(
            &stmt("INSERT INTO items (id, name, qty) VALUES (_id, _name, _qty)"),
            &vars,
        );
        let Statement::Insert(insert) = out else {
            panic!("insert")
        };
        let InsertSource::Values(rows) = &insert.source else {
            panic!("values")
        };
        let kinds: Vec<&ExprKind> = rows[0].iter().map(|e| &e.kind).collect();
        assert!(matches!(kinds[0], ExprKind::Number(n) if n == "1"));
        // A text variable is a CHAR(10) of its type.
        assert!(matches!(
            kinds[1],
            ExprKind::Cast { expr, to: CastType::Char(Some(10)), .. }
                if matches!(&expr.kind, ExprKind::Str(s) if s == "apple")
        ));
        assert!(matches!(kinds[2], ExprKind::Number(n) if n == "5"));
    }

    #[test]
    fn substitutes_in_set_and_where_but_leaves_columns() {
        let vars = vars(&[("_q", "INTEGER", V::Int(7)), ("_id", "INTEGER", V::Int(3))]);
        let out = substitute(
            &stmt("UPDATE items SET qty = qty + _q WHERE id = _id"),
            &vars,
        );
        let Statement::Update(update) = out else {
            panic!("update")
        };
        // `qty + _q`: the column `qty` stays, `_q` becomes 7.
        let value = &update.sets[0].value;
        let ExprKind::Binary(_, left, right) = &value.kind else {
            panic!("binary")
        };
        assert!(matches!(&left.kind, ExprKind::Column { name, .. } if name == "qty"));
        assert!(matches!(&right.kind, ExprKind::Number(n) if n == "7"));
        // `id = _id`: the column stays, the variable becomes 3.
        let ExprKind::Binary(_, left, right) = &update.filter.unwrap().kind else {
            panic!()
        };
        assert!(matches!(&left.kind, ExprKind::Column { name, .. } if name == "id"));
        assert!(matches!(&right.kind, ExprKind::Number(n) if n == "3"));
    }

    #[test]
    fn leaves_qualified_names_and_pseudo_columns_alone() {
        // A qualified name is a real column even if a variable shares its name.
        let vars = vars(&[("id", "INTEGER", V::Int(9))]);
        let out = substitute(&stmt("SELECT t.id FROM items t WHERE t.id > 0"), &vars);
        let Statement::Select(select) = out else {
            panic!("select")
        };
        let Item::Expr { expr, .. } = &select.items[0] else {
            panic!("item")
        };
        assert!(matches!(&expr.kind, ExprKind::Column { table: Some(_), name } if name == "id"));
    }

    /// `RAISE` fails the script with error 2224 naming the exception, its code
    /// and message, located at the exception's name (as the original reports
    /// a trigger's, `fixtures/dictionary/constraints.json`).
    #[test]
    fn raise_fails_with_the_exception_at_its_name() {
        let src = " IF _n = 99 THEN RAISE nope(1, 'refused'); END IF;";
        let mut vars = Vars::new();
        vars.define("_n", "INTEGER", V::Int(99)).unwrap();
        let error = run(&body(src), &mut vars, &mut Mock::default(), &Env::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "AQE Error:  State = 42000;   NativeError = 2224;  [SAP][Advantage SQL Engine]An \
             exception is raised in the SQL script. {[nope] 1 : refused} ** Script error \
             information:  -- Location of error in the SQL statement is: 24"
        );
        vars.assign("_n", V::Int(1)).unwrap();
        let flow = run(&body(src), &mut vars, &mut Mock::default(), &Env::default()).unwrap();
        assert_eq!(flow, Flow::Done);
    }

    #[test]
    fn a_negative_number_substitutes_as_a_negation() {
        let out = substitute(
            &stmt("INSERT INTO t VALUES (n)"),
            &vars(&[("n", "INTEGER", V::Int(-5))]),
        );
        let Statement::Insert(insert) = out else {
            panic!("insert")
        };
        let InsertSource::Values(rows) = &insert.source else {
            panic!("values")
        };
        let ExprKind::Neg(inner) = &rows[0][0].kind else {
            panic!("neg")
        };
        assert!(matches!(&inner.kind, ExprKind::Number(n) if n == "5"));
    }
}
