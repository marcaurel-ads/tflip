//! What a `DB:Public` rights slot that does not check does to a statement on
//! `system.permissions` or `system.effectivepermissions`, as the original
//! engine does it (`fixtures/sql/ddpublicbad3`, `ddpublicbad4`,
//! `ddpublicbad5`).
//!
//! The grid is built row by row, and a row whose rights cells read such a slot
//! is lost to the statement. The first of them fails the statement if no row
//! has been delivered yet, and any of them does if the statement holds the
//! rows back (`ORDER BY`, grouping, aggregates). Otherwise the row is a hole:
//! the first hole is only closed up, each later one is paid for with a copy of
//! the first row of the ten-row block of what was delivered that the hole
//! comes in, so that the statement always shows one row fewer than the grid has.

use tflip_sql::SqlError;
use tflip_sql::ast::{BinOp, Expr, ExprKind, Item, Select};
use tflip_storage::build::Column;
use tflip_storage::value::Value;

/// The first of the rights columns (`Select`); the four before it name the
/// row.
const FIRST_RIGHT: usize = 4;

/// What a statement reads of the rows.
pub(super) struct Usage {
    filter: Option<Expr>,
    /// The column names, lower case.
    names: Vec<String>,
    /// Whether the `WHERE` reads a rights column.
    filter_reads_rights: bool,
    /// Whether the select list reads a rights column.
    projects_rights: bool,
    /// Whether the statement reads every row before it shows one.
    buffered: bool,
}

impl Usage {
    /// What `select` reads of a table of `columns`; `None` for a statement
    /// whose reads this does not follow (a subquery, a `UNION`, a condition
    /// it does not evaluate).
    pub(super) fn of(select: &Select, columns: &[Column]) -> Option<Usage> {
        if !select.unions.is_empty() || select.into.is_some() {
            return None;
        }
        let names: Vec<String> = columns
            .iter()
            .map(|c| c.name.to_ascii_lowercase())
            .collect();
        let is_right = |name: &str| {
            names
                .iter()
                .position(|n| n.eq_ignore_ascii_case(name))
                .is_some_and(|at| at >= FIRST_RIGHT)
        };
        let mut supported = true;
        let mut reads_rights = |expr: &Expr| {
            let mut reads = false;
            expr.walk(&mut |e| match &e.kind {
                ExprKind::Column { name, .. } => reads |= is_right(name),
                ExprKind::Subquery(_) | ExprKind::Exists(_) | ExprKind::InSelect { .. } => {
                    supported = false
                }
                _ => {}
            });
            reads
        };
        let filter_reads_rights = select.filter.as_ref().is_some_and(&mut reads_rights);
        let mut projects_rights = false;
        for item in &select.items {
            match item {
                Item::Star { .. } => projects_rights = true,
                Item::Expr { expr, .. } => projects_rights |= reads_rights(expr),
            }
        }
        for key in &select.order {
            if let tflip_sql::ast::OrderKey::Expr(expr) = &key.key {
                projects_rights |= reads_rights(expr);
            }
        }
        let buffered = !select.is_simple() || select.distinct || !select.order.is_empty();
        supported.then_some(Usage {
            filter: select.filter.clone(),
            names,
            filter_reads_rights,
            projects_rights,
            buffered,
        })
    }

    /// Whether the `WHERE` lets `row` through, `None` where it is a condition
    /// this does not evaluate.
    fn passes(&self, row: &[Value]) -> Option<bool> {
        match &self.filter {
            None => Some(true),
            Some(filter) => self.eval(filter, row),
        }
    }

    fn eval(&self, expr: &Expr, row: &[Value]) -> Option<bool> {
        use std::cmp::Ordering;
        match &expr.kind {
            ExprKind::Bool(value) => Some(*value),
            ExprKind::Not(inner) => self.eval(inner, row).map(|value| !value),
            ExprKind::Binary(BinOp::And, a, b) => Some(self.eval(a, row)? && self.eval(b, row)?),
            ExprKind::Binary(BinOp::Or, a, b) => Some(self.eval(a, row)? || self.eval(b, row)?),
            ExprKind::Binary(op, a, b) => {
                let (cell, literal, flipped) = match (&a.kind, &b.kind) {
                    (ExprKind::Column { name, .. }, _) => (self.cell(name, row)?, b, false),
                    (_, ExprKind::Column { name, .. }) => (self.cell(name, row)?, a, true),
                    _ => return None,
                };
                let ordering = compare(cell, literal)?;
                let Some(ordering) = ordering else {
                    return Some(false);
                };
                let ordering = if flipped {
                    ordering.reverse()
                } else {
                    ordering
                };
                Some(match op {
                    BinOp::Eq => ordering == Ordering::Equal,
                    BinOp::Ne => ordering != Ordering::Equal,
                    BinOp::Lt => ordering == Ordering::Less,
                    BinOp::Le => ordering != Ordering::Greater,
                    BinOp::Gt => ordering == Ordering::Greater,
                    BinOp::Ge => ordering != Ordering::Less,
                    _ => return None,
                })
            }
            ExprKind::In {
                expr: inner,
                list,
                negated,
            } => {
                let ExprKind::Column { name, .. } = &inner.kind else {
                    return None;
                };
                let cell = self.cell(name, row)?;
                let mut found = false;
                for item in list {
                    found |= compare(cell, item)? == Some(Ordering::Equal);
                }
                Some(found != *negated)
            }
            ExprKind::IsNull {
                expr: inner,
                negated,
            } => {
                let ExprKind::Column { name, .. } = &inner.kind else {
                    return None;
                };
                Some(matches!(self.cell(name, row)?, Value::Null) != *negated)
            }
            _ => None,
        }
    }

    fn cell<'a>(&self, name: &str, row: &'a [Value]) -> Option<&'a Value> {
        let at = self
            .names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))?;
        row.get(at)
    }
}

/// How `cell` compares with the literal `literal`: `Some(None)` where either
/// is `NULL`, `None` where this does not compare them.
fn compare(cell: &Value, literal: &Expr) -> Option<Option<std::cmp::Ordering>> {
    match (cell, &literal.kind) {
        (Value::Null, _) | (_, ExprKind::Null) => Some(None),
        (Value::Text(text), ExprKind::Str(other)) => Some(Some(
            text.trim_end()
                .to_ascii_lowercase()
                .cmp(&other.trim_end().to_ascii_lowercase()),
        )),
        (Value::Integer(value), ExprKind::Number(other)) => {
            Some(Some(value.cmp(&other.parse::<i128>().ok()?)))
        }
        _ => None,
    }
}

/// The rows of the grid (`rows`, of which `bad` flags the ones that read a
/// slot that does not check) as the statement `usage` leaves them: the rows
/// the original loses taken out, and the copies it pays for them put in. Fails
/// with 5127 where the original does.
pub(super) fn apply(
    rows: Vec<Vec<Value>>,
    bad: &[bool],
    usage: &Usage,
) -> Result<Vec<Vec<Value>>, SqlError> {
    // The rows the statement shows, found by the condition the grid's own
    // cells meet.
    let passed: Vec<Option<bool>> = rows.iter().map(|row| usage.passes(row)).collect();
    // A condition this does not evaluate leaves the grid as it is.
    if passed.contains(&None) {
        return Ok(rows);
    }
    let mut kept: Vec<(Vec<Value>, bool)> = Vec::with_capacity(rows.len());
    // How many rows had been shown at each hole.
    let mut holes: Vec<usize> = Vec::new();
    let mut shown = 0;
    for ((row, bad), passes) in rows.into_iter().zip(bad).zip(passed) {
        let reads =
            *bad && (usage.filter_reads_rights || (passes != Some(false) && usage.projects_rights));
        if reads {
            if usage.buffered || (holes.is_empty() && shown == 0) {
                return Err(SqlError::ace(
                    5127,
                    crate::ace_error::error_text(5127).unwrap_or_default(),
                    "Inconsistency detected while reading stored property.",
                ));
            }
            holes.push(shown);
            continue;
        }
        let shows = passes == Some(true);
        shown += usize::from(shows);
        kept.push((row, shows));
    }
    // The first hole is only closed up; each later one is paid for with a copy
    // of the first row of its block of ten shown rows.
    let mut copies = std::collections::BTreeMap::<usize, usize>::new();
    for &at in holes.iter().skip(1) {
        if at > 0 && at % 10 == 0 {
            continue;
        }
        *copies.entry(at.saturating_sub(1) / 10 * 10).or_default() += 1;
    }
    let mut out = Vec::with_capacity(kept.len() + holes.len());
    let mut shown = 0;
    for (row, shows) in kept {
        out.push(row.clone());
        if shows {
            for _ in 0..copies.get(&shown).copied().unwrap_or(0) {
                out.push(row.clone());
            }
            shown += 1;
        }
    }
    Ok(out)
}
