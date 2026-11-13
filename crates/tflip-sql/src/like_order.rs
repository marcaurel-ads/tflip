//! The order of the rows of a `LIKE`.
//!
//! The original engine gives the rows of `WHERE name LIKE 'res%'` ordered by
//! the column, not as the table holds them (`fixtures/sql/likeorder.json`,
//! `fixtures/sql/oem.json`; tables with an index and without alike), but only
//! where the `LIKE` is the one condition the engine has to find the rows by.
//! A condition beside it that its AOF holds (a comparison, `IN`, `BETWEEN`,
//! `IS NULL`, another `LIKE`) finds them in the table's order again; one it
//! cannot (`NOT`, `NOT LIKE`, a constant, `EXISTS`, a comparison of two
//! columns) leaves the `LIKE` to. An `OR`, a pattern that begins with a
//! wildcard, an `ORDER BY` and a join are not ordered by it.

use crate::ast::BinOp;
use crate::compile::{Kind, Node};
use crate::query::children_of;
use crate::value::V;

/// The column the rows of a `WHERE` `filter` come ordered by, if one.
pub(crate) fn column(filter: &Node) -> Option<Node> {
    let mut parts = Vec::new();
    conjuncts(filter, &mut parts);
    let likes: Vec<&Node> = parts
        .iter()
        .copied()
        .filter(|p| matches!(p.kind, Kind::Like(.., false)))
        .collect();
    let [like] = likes[..] else {
        return None;
    };
    let Kind::Like(column, pattern, _, _) = &like.kind else {
        return None;
    };
    if !matches!(column.kind, Kind::Column(_)) {
        return None;
    }
    // A text that begins with a character that matches itself.
    // A parameter is taken to be one that does (probed with such a value
    // only; its value is not known when the statement is planned).
    if !matches!(pattern.kind, Kind::Param(_)) {
        let pattern = constant_text(pattern)?;
        if pattern.starts_with(['%', '_', '[']) || pattern.is_empty() {
            return None;
        }
    }
    let others_find = parts
        .iter()
        .any(|p| !std::ptr::eq(*p, like) && !matches!(p.kind, Kind::Like(.., true)) && held(p));
    (!others_find).then(|| (**column).clone())
}

fn conjuncts<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
    match &node.kind {
        Kind::Binary(BinOp::And, a, b) => {
            conjuncts(a, out);
            conjuncts(b, out);
        }
        _ => out.push(node),
    }
}

/// Whether the AOF of the original engine holds the condition `node`.
fn held(node: &Node) -> bool {
    match &node.kind {
        Kind::Not(_) | Kind::Exists(_) | Kind::Like(.., true) => false,
        Kind::Binary(BinOp::And | BinOp::Or, a, b) => held(a) && held(b),
        Kind::Binary(
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge,
            a,
            b,
        ) => has_column(a) != has_column(b),
        _ => has_column(node),
    }
}

fn has_column(node: &Node) -> bool {
    matches!(node.kind, Kind::Column(_)) || children_of(node).into_iter().any(has_column)
}

/// The text of a constant: a literal, or literals added.
fn constant_text(node: &Node) -> Option<String> {
    match &node.kind {
        Kind::Const(V::Str(text)) => Some(text.clone()),
        Kind::Binary(BinOp::Add, a, b) => Some(constant_text(a)? + &constant_text(b)?),
        _ => None,
    }
}
