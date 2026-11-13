//! Subqueries in a script's own expressions (`n = (SELECT COUNT(*) FROM t)`,
//! `IF EXISTS (SELECT …) THEN`), which the interpreter has no tables to run:
//! the host runs each as the interpreter asks for the expression, and gives
//! back the expression with its value in its place
//! (`fixtures/sql/subq.json`).

use std::cell::RefCell;

use tflip_sql::ast::{BinOp, Expr, ExprKind, Select, Statement as Sql};
use tflip_sql::script::{self, Vars};
use tflip_sql::{SqlError, Ty, V};
use tflip_storage::build::Column;
use tflip_storage::value::Value;

use crate::{Error, statement};

/// How a host runs a `SELECT`: its columns and rows.
pub(crate) type Run<'a> = &'a dyn Fn(&Select) -> Result<(Vec<Column>, Vec<Vec<Value>>), Error>;

/// `expr` with each subquery in it run, and replaced by what it is worth: a
/// value, whether there is a row, a list of values for `IN`, or the
/// comparisons of `ANY` and `ALL`. The variables `vars` are values in them.
/// `None` if `expr` has none. What fails is located in the script if `located`.
pub(crate) fn resolve(
    expr: &Expr,
    vars: &Vars,
    located: bool,
    run: Run<'_>,
) -> Result<Option<Expr>, SqlError> {
    resolve_where(expr, vars, located, run, &|_| true)
}

/// [`resolve`] for the subqueries `wanted` says to run: the others stay.
pub(crate) fn resolve_where(
    expr: &Expr,
    vars: &Vars,
    located: bool,
    run: Run<'_>,
    wanted: &dyn Fn(&Select) -> bool,
) -> Result<Option<Expr>, SqlError> {
    if !script::has_subquery(expr) {
        return Ok(None);
    }
    let failure: RefCell<Option<SqlError>> = RefCell::new(None);
    let resolved = script::rewrite_expr(expr, &|kind| {
        let at = script::replacing_at();
        let select = match kind {
            ExprKind::Subquery(select)
            | ExprKind::Exists(select)
            | ExprKind::InSelect { select, .. }
            | ExprKind::Quantified { select, .. } => select,
            _ => return None,
        };
        if !wanted(select) {
            return None;
        }
        match one(kind, select, vars, (located, at), run) {
            Ok(replacement) => Some(replacement),
            Err(error) => {
                failure.borrow_mut().get_or_insert(error);
                None
            }
        }
    });
    match failure.into_inner() {
        Some(error) => Err(error),
        None => Ok(Some(resolved)),
    }
}

/// Whether `select` reads a column of one of the tables `outer` names (by
/// their name or alias), which makes it depend on the row its `SELECT` is
/// run for.
pub(crate) fn reads_outer(select: &Select, outer: &[&str]) -> bool {
    use tflip_sql::ast::{Item, OrderKey};
    let found = std::cell::Cell::new(false);
    let look = |expr: &Expr| {
        expr.walk(&mut |e| match &e.kind {
            ExprKind::Column {
                table: Some(table), ..
            } if outer.iter().any(|name| name.eq_ignore_ascii_case(table)) => found.set(true),
            ExprKind::Subquery(s)
            | ExprKind::Exists(s)
            | ExprKind::InSelect { select: s, .. }
            | ExprKind::Quantified { select: s, .. }
                if reads_outer(s, outer) =>
            {
                found.set(true);
            }
            _ => {}
        });
    };
    for item in &select.items {
        if let Item::Expr { expr, .. } = item {
            look(expr);
        }
    }
    select.filter.iter().for_each(&look);
    select.having.iter().for_each(&look);
    for key in select
        .group
        .iter()
        .chain(select.order.iter().map(|o| &o.key))
    {
        if let OrderKey::Expr(expr) = key {
            look(expr);
        }
    }
    found.get()
}

/// The expression the subquery `kind`, of the `SELECT` `select`, is worth.
fn one(
    kind: &ExprKind,
    select: &Select,
    vars: &Vars,
    (located, at): (bool, usize),
    run: Run<'_>,
) -> Result<ExprKind, SqlError> {
    let Sql::Select(select) = script::substitute(&Sql::Select(Box::new(select.clone())), vars)
    else {
        unreachable!("a SELECT stays one");
    };
    let (columns, rows) = run(&select).map_err(|error| {
        script::in_script_at(
            match error {
                Error::Sql(error) => error,
                _ => statement::read_failed(),
            },
            at,
        )
    })?;
    if let ExprKind::Exists(_) = kind {
        return Ok(ExprKind::Bool(!rows.is_empty()));
    }
    let [column] = &columns[..] else {
        return Err(own(SqlError::subquery_columns(), located, at));
    };
    let field = statement::field_of(column);
    let ty = Ty::of_field(&field);
    let values: Vec<V> = rows
        .into_iter()
        .map(|row| V::from_field(row.into_iter().next().unwrap_or(Value::Null), ty))
        .collect();
    let literal = |value: &V| Expr {
        kind: script::typed_value(value, &field),
        pos: 0,
    };
    Ok(match kind {
        ExprKind::Subquery(_) => match &values[..] {
            [] => script::typed_value(&V::Null, &field),
            [value] => script::typed_value(value, &field),
            _ => return Err(own(SqlError::subquery_rows(), located, at)),
        },
        ExprKind::InSelect { expr, negated, .. } => ExprKind::In {
            expr: expr.clone(),
            list: values.iter().map(literal).collect(),
            negated: *negated,
        },
        ExprKind::Quantified { op, expr, all, .. } => {
            let join = if *all { BinOp::And } else { BinOp::Or };
            let compare = |value: &V| Expr {
                kind: ExprKind::Binary(*op, expr.clone(), Box::new(literal(value))),
                pos: expr.pos,
            };
            let mut comparisons = values.iter().map(compare);
            match comparisons.next() {
                // `ALL` of nothing holds, and `ANY` of nothing does not.
                None => ExprKind::Bool(*all),
                Some(first) => {
                    comparisons
                        .fold(first, |left, right| Expr {
                            kind: ExprKind::Binary(join, Box::new(left), Box::new(right)),
                            pos: expr.pos,
                        })
                        .kind
                }
            }
        }
        _ => unreachable!("a subquery"),
    })
}

/// `error` of the subquery at `at`, as a script words it (with a space after
/// its text and where it is) if `located`; a function leaves it as it is.
fn own(mut error: SqlError, located: bool, at: usize) -> SqlError {
    if located {
        error.message.push(' ');
        return script::in_script_at(error, at);
    }
    error
}
