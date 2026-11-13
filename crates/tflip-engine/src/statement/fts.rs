//! The full-text search functions of a query.

use super::*;

/// The `CONTAINS` terms of the `WHERE` `filter` of a live cursor on `table`,
/// as its AOF has them (see [`tflip_sql::xbase::contains_terms`]): how far the
/// field's full-text order (among `orders`) answers each, the records the
/// order finds for it, and the order's name. A term on a field with no order,
/// or with a condition that is not a constant, is not optimized.
pub(crate) fn fts_terms(
    table: &table::Table,
    filter: &tflip_sql::Node,
    orders: &[Arc<crate::index::Index>],
) -> Result<Vec<(tflip_expr::optimize::FtsTerm, Option<String>)>, Error> {
    use tflip_expr::optimize::{FtsTerm, Level};
    use tflip_sql::compile::{FtsCondition, FtsSource, Kind};
    use tflip_sql::fts::IndexLevel;
    let mut out = Vec::new();
    for (text, search) in tflip_sql::xbase::contains_terms(filter, table.fields()) {
        let column = match &search.source {
            FtsSource::Fields(_) => None,
            FtsSource::Expr(node) => match node.kind {
                Kind::Column(i) => Some(i),
                _ => continue,
            },
        };
        let found = match &search.condition {
            FtsCondition::Const(query) => fts_found(table, orders, column, query)?,
            _ => None,
        };
        let Some(found) = found else {
            out.push((
                FtsTerm {
                    text,
                    level: Level::None,
                    records: Vec::new(),
                },
                None,
            ));
            continue;
        };
        let level = match found.level {
            IndexLevel::Full => Level::Full,
            IndexLevel::Partial => Level::Partial,
            IndexLevel::None => Level::None,
        };
        let term = FtsTerm {
            text,
            level,
            records: found.records,
        };
        out.push((term, Some(found.order)));
    }
    Ok(out)
}

/// What `table`'s full-text order (among `orders`) on the field at place
/// `column` finds for `query`: how far it answers it and the records its keys
/// hold it for, deleted records' included (see
/// [`tflip_sql::fts::Postings::found`]). For `column` `None` (`CONTAINS(*,
/// …)`), every full-text order's, named after the first. `None` for a field
/// with no full-text order.
pub(super) fn fts_found(
    table: &table::Table,
    orders: &[Arc<crate::index::Index>],
    column: Option<usize>,
    query: &tflip_sql::fts::Query,
) -> Result<Option<tflip_sql::query::FtsLookup>, Error> {
    let found: Vec<&Arc<crate::index::Index>> = match column {
        None => orders
            .iter()
            .filter(|order| order.tag.full_text())
            .collect(),
        Some(column) => fts_order(table, orders, Some(column)).into_iter().collect(),
    };
    let Some(first) = found.first() else {
        return Ok(None);
    };
    let mut level = tflip_sql::fts::IndexLevel::Full;
    let mut records: Vec<u32> = Vec::new();
    for order in &found {
        let key_length = usize::from(order.tag.key_length);
        level = level.max(tflip_sql::fts::index_level(query, key_length));
        records.extend(order.fts_postings()?.found(query, table.slot_count()));
    }
    records.sort_unstable();
    records.dedup();
    Ok(Some(tflip_sql::query::FtsLookup {
        order: first.tag.name.clone(),
        level,
        records,
    }))
}

/// How many records the original engine counts for a live cursor's `WHERE`
/// `filter` of `CONTAINS` terms (`terms`, by their AOF text) its full-text
/// orders answer, joined by `AND`, `OR` and `NOT` with parts `other` finds the
/// records of: the records the orders find, of the table's `slots` (deleted
/// records' included, as a FIXED order keeps their keys and a `NOT` takes every
/// slot); `None` for a `WHERE` with no full-text term, or a part neither finds.
pub(super) fn fts_count(
    filter: &tflip_sql::Node,
    fields: &[tflip_storage::field::Field],
    terms: &[tflip_expr::optimize::FtsTerm],
    slots: u32,
    other: &mut dyn FnMut(&tflip_sql::Node) -> Option<std::collections::BTreeSet<u32>>,
) -> Option<u32> {
    use tflip_sql::ast::BinOp;
    use tflip_sql::compile::Kind;
    if !filter.any(&|n| matches!(n.kind, Kind::Contains(_))) {
        return None;
    }
    fn found(
        node: &tflip_sql::Node,
        fields: &[tflip_storage::field::Field],
        terms: &[tflip_expr::optimize::FtsTerm],
        slots: u32,
        other: &mut dyn FnMut(&tflip_sql::Node) -> Option<std::collections::BTreeSet<u32>>,
    ) -> Option<std::collections::BTreeSet<u32>> {
        Some(match &node.kind {
            Kind::Contains(search) => {
                let text = tflip_sql::xbase::contains(search, fields, "=.T.")?;
                let term = terms.iter().find(|term| term.text == text)?;
                if term.level != tflip_expr::optimize::Level::Full {
                    return None;
                }
                term.records.iter().copied().collect()
            }
            Kind::Binary(BinOp::And, a, b) => {
                let a = found(a, fields, terms, slots, other)?;
                let b = found(b, fields, terms, slots, other)?;
                a.intersection(&b).copied().collect()
            }
            Kind::Binary(BinOp::Or, a, b) => {
                let a = found(a, fields, terms, slots, other)?;
                let b = found(b, fields, terms, slots, other)?;
                a.union(&b).copied().collect()
            }
            Kind::Not(inner) => {
                let inner = found(inner, fields, terms, slots, other)?;
                (1..=slots).filter(|r| !inner.contains(r)).collect()
            }
            _ => other(node)?,
        })
    }
    found(filter, fields, terms, slots, other).map(|records| records.len() as u32)
}

/// The native error of the first `CONTAINS` of `filter` whose condition does
/// not parse, if one does not.
pub(super) fn invalid_fts(filter: &tflip_sql::Node) -> Option<u32> {
    use tflip_sql::compile::{FtsCondition, Kind};
    let code = std::cell::Cell::new(None);
    filter.any(&|node| match &node.kind {
        Kind::Contains(search) if let FtsCondition::Invalid(c) = search.condition => {
            code.set(Some(c));
            true
        }
        _ => false,
    });
    code.get()
}

/// The full-text order on the column at place `column` (any full-text order for
/// `*`, with `column` `None`), among `orders`.
fn fts_order<'a>(
    table: &table::Table,
    orders: &'a [Arc<crate::index::Index>],
    column: Option<usize>,
) -> Option<&'a Arc<crate::index::Index>> {
    orders.iter().find(|order| {
        if !order.tag.full_text() {
            return false;
        }
        let Some(column) = column else { return true };
        let crate::seek::KeySource::Field(field) = order.key_source(table) else {
            return false;
        };
        table.fields().iter().position(|f| std::ptr::eq(f, field)) == Some(column)
    })
}
