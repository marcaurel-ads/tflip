//! What a statement may do with the columns of a dictionary table that its
//! user may not use (`fixtures/sql/ddcolrights`).
//!
//! A column the user may not read reads as NULL, in a select list and
//! through the table API alike (see [`crate::table::Table`]). What a
//! statement does with such a column beyond that depends on the table's
//! permission level, which this checks before the statement runs, as the
//! original engine refuses it, with 5171:
//!
//! - at level 1 a `WHERE`, `GROUP BY`, `HAVING`, `ORDER BY` or `ON` may name
//!   it, and works on what it holds;
//! - at level 2 (the default) it may not;
//! - at level 3, where only SQL reaches the table, the select list may not
//!   name it either, and `*` stands for it.
//!
//! An `UPDATE` may not set a column the user may not update, at any level.

use crate::system::right;
use tflip_sql::SqlError;
use tflip_sql::ast::{Expr, ExprKind, From, Item, OrderKey, Select, Statement as Sql, TableRef};

use crate::Error;
use crate::dictionary::{ColumnAccess, Database};
use crate::{Handle, Session, connection};

/// Refuses `statement`, if the connection's user may not do with the columns
/// of the tables it names what it says.
pub(crate) fn check(session: &Session, connection: Handle, statement: &Sql) -> Result<(), Error> {
    let connection = connection::get(session, connection)?;
    // A user without the right to read a table may change it as far as it
    // holds the right to, but not by a condition, which reads it
    // (`fixtures/sql/ddinsertonly`).
    let changed = match statement {
        Sql::Update(update) => Some((
            &update.table,
            update
                .filter
                .as_ref()
                .is_some_and(|f| reads_target(f, &update.table)),
            right::UPDATE,
        )),
        Sql::Delete(delete) => Some((
            &delete.table,
            delete
                .filter
                .as_ref()
                .is_some_and(|f| reads_target(f, &delete.table)),
            right::DELETE,
        )),
        _ => None,
    };
    if let Some((table, true, needs)) = changed
        && !connection.privileged()
    {
        let access = connection.with_database_opt(|database| database.access_mask(&table.name));
        if access.is_some_and(|access| access & right::READ == 0 && access & needs != 0) {
            return Err(Error::Sql(SqlError::ace(
                5054,
                &format!(
                    "The command cannot be completed with the current user permissions. \
                     Read permission not granted to table: {}",
                    table.name
                ),
                "",
            )));
        }
    }
    connection
        .with_database_opt(|database| statement_check(database, statement))
        .unwrap_or(Ok(()))
        .map_err(Error::Sql)
}

/// Whether a condition reads the table the statement changes: a column of
/// its own, or a correlated one in a subquery. A condition that reads
/// nothing of it (`WHERE 1 = 0`, a subquery over other tables) opens it for
/// no read (`fixtures/sql/ddinsertonly3`).
fn reads_target(filter: &Expr, target: &TableRef) -> bool {
    let qualifier = target.alias.as_deref().unwrap_or(&target.name);
    let mut found = false;
    filter.walk(&mut |e| match &e.kind {
        ExprKind::Column { table, .. } => {
            found |= table
                .as_deref()
                .is_none_or(|t| t.eq_ignore_ascii_case(qualifier));
        }
        ExprKind::Subquery(select) | ExprKind::Exists(select) => {
            found |= correlated(select, qualifier);
        }
        ExprKind::InSelect { select, .. } | ExprKind::Quantified { select, .. } => {
            found |= correlated(select, qualifier);
        }
        _ => {}
    });
    found
}

/// Whether `select` names a column by `qualifier`, a table of an outer
/// statement, unless its own `FROM` has a table of that name.
fn correlated(select: &Select, qualifier: &str) -> bool {
    let mut own = Vec::new();
    add_from(&mut own, &select.from);
    if own.iter().any(|name| name.eq_ignore_ascii_case(qualifier)) {
        return false;
    }
    let mut found = false;
    let mut look = |e: &Expr| {
        e.walk(&mut |e| match &e.kind {
            ExprKind::Column {
                table: Some(table), ..
            } => found |= table.eq_ignore_ascii_case(qualifier),
            ExprKind::Subquery(s) | ExprKind::Exists(s) => found |= correlated(s, qualifier),
            ExprKind::InSelect { select: s, .. } | ExprKind::Quantified { select: s, .. } => {
                found |= correlated(s, qualifier)
            }
            _ => {}
        })
    };
    for item in &select.items {
        if let Item::Expr { expr, .. } = item {
            look(expr);
        }
    }
    for expr in select
        .filter
        .iter()
        .chain(&select.having)
        .chain(select.group.iter().filter_map(key_expr))
        .chain(select.order.iter().filter_map(|o| key_expr(&o.key)))
    {
        look(expr);
    }
    fn add_from(names: &mut Vec<String>, from: &From) {
        match from {
            From::Table(t) => names.push(t.alias.clone().unwrap_or_else(|| t.name.clone())),
            From::Join { left, right, .. } => {
                add_from(names, left);
                add_from(names, right);
            }
            From::Derived { alias: Some(a), .. } | From::Procedure { alias: Some(a), .. } => {
                names.push(a.clone())
            }
            _ => {}
        }
    }
    found
}

/// What a column is named in: a condition, or a select list.
#[derive(Clone, Copy, PartialEq)]
enum Place {
    Condition,
    Select,
}

/// A table a statement reads, under the name its columns are qualified by.
struct Source {
    name: String,
    access: ColumnAccess,
}

fn refused(detail: &str) -> SqlError {
    SqlError::ace(
        5171,
        "The requested access to a table column is not allowed for the current user.",
        detail,
    )
}

fn statement_check(database: &Database, statement: &Sql) -> Result<(), SqlError> {
    match statement {
        Sql::Select(select) => select_check(database, select, &[], Place::Select),
        Sql::Update(update) => {
            let mut sources = Vec::new();
            add_table(database, &update.table, &mut sources);
            if let Some(from) = &update.from {
                add_from(database, from, &mut sources);
            }
            for set in &update.sets {
                let denied = sources
                    .iter()
                    .filter(|s| !s.access.lenient && s.access.columns.iter().any(|c| c.update))
                    .filter(|s| {
                        let table = update.table.alias.as_deref().unwrap_or(&update.table.name);
                        s.name.eq_ignore_ascii_case(table)
                    })
                    .filter_map(|s| s.access.column(&set.column.name))
                    .find(|c| !c.update);
                if let Some(column) = denied {
                    return Err(refused(&format!(
                        " Update permission not granted to column: {}",
                        column.name
                    )));
                }
            }
            if let Some(filter) = &update.filter {
                expr_check(database, filter, &sources, &[], Place::Condition)?;
            }
            if let Some(from) = &update.from {
                from_check(database, from, &sources, &[])?;
            }
            Ok(())
        }
        Sql::Delete(delete) => {
            let mut sources = Vec::new();
            add_table(database, &delete.table, &mut sources);
            match &delete.filter {
                Some(filter) => expr_check(database, filter, &sources, &[], Place::Condition),
                None => Ok(()),
            }
        }
        Sql::Insert(insert) => match &insert.source {
            tflip_sql::ast::InsertSource::Select(select) => {
                select_check(database, select, &[], Place::Select)
            }
            _ => Ok(()),
        },
        _ => Ok(()),
    }
}

fn add_table(database: &Database, table: &TableRef, sources: &mut Vec<Source>) {
    if table.link.is_some() || table.name.to_ascii_lowercase().starts_with("system.") {
        return;
    }
    if let Some(access) = database.column_access(&table.name) {
        sources.push(Source {
            name: table.alias.clone().unwrap_or_else(|| table.name.clone()),
            access,
        });
    }
}

fn add_from(database: &Database, from: &From, sources: &mut Vec<Source>) {
    match from {
        From::Table(table) => add_table(database, table, sources),
        From::Join { left, right, .. } => {
            add_from(database, left, sources);
            add_from(database, right, sources);
        }
        _ => {}
    }
}

/// The conditions of the joins in `from`, and the `SELECT`s it holds.
fn from_check(
    database: &Database,
    from: &From,
    sources: &[Source],
    outer: &[Source],
) -> Result<(), SqlError> {
    match from {
        From::Join {
            left, right, on, ..
        } => {
            from_check(database, left, sources, outer)?;
            from_check(database, right, sources, outer)?;
            match on {
                Some(on) => expr_check(database, on, sources, outer, Place::Condition),
                None => Ok(()),
            }
        }
        From::Derived { select, .. } => select_check(database, select, &[], Place::Select),
        _ => Ok(()),
    }
}

/// `items` is what the select list's columns are named in: a select list of
/// its own, or a condition when the `SELECT` is a subquery of one, which
/// reads what it names (`fixtures/sql/ddinsertonly3`).
fn select_check(
    database: &Database,
    select: &Select,
    outer: &[Source],
    items: Place,
) -> Result<(), SqlError> {
    let mut sources = Vec::new();
    add_from(database, &select.from, &mut sources);
    for item in &select.items {
        match item {
            Item::Star { table, .. } => {
                for source in &sources {
                    let wanted = table
                        .as_deref()
                        .is_none_or(|t| t.eq_ignore_ascii_case(&source.name));
                    if wanted
                        && source.access.level == 3
                        && let Some(hidden) = source.access.hidden().first()
                    {
                        return Err(refused(&format!(
                            "{hidden} in table: {}",
                            source.access.table
                        )));
                    }
                }
            }
            Item::Expr { expr, .. } => expr_check(database, expr, &sources, outer, items)?,
        }
    }
    from_check(database, &select.from, &sources, outer)?;
    let conditions = select
        .filter
        .iter()
        .chain(select.having.iter())
        .chain(select.group.iter().filter_map(key_expr))
        .chain(select.order.iter().filter_map(|o| key_expr(&o.key)));
    for condition in conditions {
        expr_check(database, condition, &sources, outer, Place::Condition)?;
    }
    for (_, union) in &select.unions {
        select_check(database, union, outer, items)?;
    }
    Ok(())
}

fn key_expr(key: &OrderKey) -> Option<&Expr> {
    match key {
        OrderKey::Expr(expr) => Some(expr),
        OrderKey::Ordinal(..) => None,
    }
}

/// Checks the columns `expr` names, and the subqueries in it, which see the
/// tables of the `SELECT` they are in too.
fn expr_check(
    database: &Database,
    expr: &Expr,
    sources: &[Source],
    outer: &[Source],
    place: Place,
) -> Result<(), SqlError> {
    let mut found: Option<Result<(), SqlError>> = None;
    expr.walk(&mut |e| {
        if found.is_some() {
            return;
        }
        let result = match &e.kind {
            ExprKind::Column { table, name } => {
                column_check(table.as_deref(), name, sources, outer, place)
            }
            ExprKind::Subquery(select) | ExprKind::Exists(select) => {
                subquery_check(database, select, sources, outer, place)
            }
            ExprKind::InSelect { select, .. } | ExprKind::Quantified { select, .. } => {
                subquery_check(database, select, sources, outer, place)
            }
            _ => Ok(()),
        };
        if result.is_err() {
            found = Some(result);
        }
    });
    found.unwrap_or(Ok(()))
}

fn subquery_check(
    database: &Database,
    select: &Select,
    sources: &[Source],
    outer: &[Source],
    place: Place,
) -> Result<(), SqlError> {
    let seen: Vec<Source> = sources
        .iter()
        .chain(outer)
        .map(|s| Source {
            name: s.name.clone(),
            access: s.access.clone(),
        })
        .collect();
    select_check(database, select, &seen, place)
}

fn column_check(
    qualifier: Option<&str>,
    name: &str,
    sources: &[Source],
    outer: &[Source],
    place: Place,
) -> Result<(), SqlError> {
    let owner = sources
        .iter()
        .chain(outer)
        .filter(|s| qualifier.is_none_or(|q| q.eq_ignore_ascii_case(&s.name)))
        .find_map(|s| s.access.column(name).map(|c| (s, c)));
    let Some((source, column)) = owner else {
        return Ok(());
    };
    let refuses = match place {
        Place::Condition => source.access.level >= 2,
        Place::Select => source.access.level == 3 && !source.access.lenient,
    };
    match !column.read && refuses {
        true => Err(refused(&format!(
            "{} in table: {}",
            column.name, source.access.table
        ))),
        false => Ok(()),
    }
}
