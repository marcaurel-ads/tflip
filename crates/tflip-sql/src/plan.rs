//! Whether a `SELECT` of one table's records is a live cursor, and the plan
//! of one that is.
//!
//! As in the original engine, a `SELECT` of columns of one table, with any
//! `WHERE` and `ORDER BY`, gives a live cursor: the table itself, its records
//! filtered and ordered. `{static}`, `DISTINCT`, `TOP`, expressions in the
//! select list, `LIKE` and memo fields in `WHERE`, and `system.iota` give a
//! static one, a new table of the results, which runs through its plan
//! ([`crate::query`]) like any other `SELECT`.

use tflip_storage::field::{Field, FieldType};

use crate::ast::{BinOp, ExprKind, Item, OrderKey, Select, Top};
use crate::compile::{Compiler, Kind, Node, Scope, collated_column};
use crate::error::SqlError;
use crate::types::Ty;

/// A planned `SELECT`.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub live: bool,
    pub columns: Vec<Column>,
    /// The `WHERE` condition.
    pub filter: Option<Node>,
    pub order: Vec<SortKey>,
    pub distinct: bool,
    pub top: Option<Top>,
    /// Each parameter's type.
    pub params: Vec<Ty>,
}

/// A column of the result.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub name: String,
    pub node: Node,
    pub ty: Ty,
    /// The result column's field type, length and decimals: the field's own
    /// for a column of the table, else those of the expression's type.
    pub field: (FieldType, u16, u16),
    /// The name the select list gives it with `AS`.
    pub alias: Option<String>,
}

impl Column {
    /// Whether `ORDER BY` can name it: the select list gave it its name (an
    /// expression's `EXPR` is none).
    pub fn is_named(&self) -> bool {
        self.alias.is_some()
            || matches!(
                self.node.kind,
                Kind::Column(_) | Kind::RowId(..) | Kind::Cast(_, "COLLATE")
            )
    }

    /// The table field this column is, if it is one.
    pub fn field_index(&self) -> Option<usize> {
        match self.node.kind {
            Kind::Column(i) => Some(i),
            _ => None,
        }
    }
}

/// An `ORDER BY` key.
#[derive(Debug, Clone, PartialEq)]
pub struct SortKey {
    pub node: Node,
    pub descending: bool,
    /// The select list's `AS` name the `ORDER BY` gave, which the original
    /// engine writes in the key of the index it makes (see
    /// [`crate::order_index`]).
    pub alias: Option<String>,
}

/// The fields of a view whose live plan is `view`, over a table with the
/// fields `base`: its columns, named as the view names them.
pub fn view_fields(view: &Plan, base: &[Field]) -> Vec<Field> {
    view.columns
        .iter()
        .filter_map(|column| {
            let mut field = base.get(column.field_index()?)?.clone();
            field.name.clone_from(&column.name);
            Some(field)
        })
        .collect()
}

/// `plan`, a live plan of a `SELECT` over the fields [`view_fields`] gives,
/// as one over the view's table: its columns are the table's, and its
/// condition is the view's as well as its own. The original engine opens the
/// table itself for such a `SELECT` (`fixtures/sql/views.json`).
pub fn through_view(mut plan: Plan, view: &Plan) -> Plan {
    let table_field = |i: usize| view.columns[i].field_index().unwrap_or(i);
    for column in &mut plan.columns {
        column.node.map_columns(&table_field);
    }
    for key in &mut plan.order {
        key.node.map_columns(&table_field);
    }
    if let Some(filter) = &mut plan.filter {
        filter.map_columns(&table_field);
    }
    // Its own condition comes first, as in the original's plans.
    plan.filter = match (plan.filter.take(), &view.filter) {
        (Some(own), Some(views)) => Some(Node {
            pos: own.pos,
            kind: Kind::Binary(BinOp::And, Box::new(own), Box::new(views.clone())),
            ty: Ty::Bool,
        }),
        (own, views) => own.or_else(|| views.clone()),
    };
    plan
}

/// Plans `select` over `scope` (`None` for `system.iota`), whose statement
/// has `params` parameters.
/// Whether a condition forces a static cursor: a `LIKE`, a memo column used
/// directly (an AOF serves neither), the `TRIM(… FROM …)` form (not the
/// `LTRIM`/`RTRIM` functions; `fixtures/sql/groupconcat.json`), `RAND` (a new number for each record,
/// as in the original engine), a call of a user-defined function
/// (`fixtures/sql/udf.json`), a shift (`fixtures/sql/gram_shift.json`), a binary
/// literal (`fixtures/sql/gram_bin.json`) or a `SCORE` (`fixtures/sql/fts_plans.json`).
/// A `CONTAINS` does not — the full-text index serves its field — so the walk
/// does not descend into one.
///
/// In a `WHERE` (`filter`), what the expression engine that filters the
/// table does not have does too: `CASE`, `CAST` and `CONVERT`, the bit
/// operators, and `INSERT`, `REPLACE`, `MOD`, `SQRT`, `ROUND`, `TRUNCATE`, the
/// exponential, logarithmic and trigonometric functions, `TIMESTAMPADD` and
/// `TIMESTAMPDIFF` and the three predicate functions `STARTSWITH`,
/// `ENDSWITH` and `SUBSTRINGOF` (`fixtures/sql/wherefilter4.json`,
/// `wherefilter5.json`).
fn forces_static(node: &Node, filter: bool) -> bool {
    match &node.kind {
        Kind::Like(..) | Kind::Score(..) | Kind::Trim(..) | Kind::UserFunc(..) => true,
        Kind::Case(..) | Kind::BitNot(_) if filter => true,
        // A `CAST` of a constant is a script variable's value, which the
        // filter takes as it is.
        Kind::Cast(inner, function)
            if filter && *function != "COLLATE" && !matches!(inner.kind, Kind::Const(_)) =>
        {
            true
        }
        Kind::Binary(BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor, ..) if filter => true,
        Kind::Func(f, _) if filter && outside_filter_engine(*f) => true,
        Kind::Column(_) if node.ty.is_memo() => true,
        Kind::Func(crate::func::Func::Rand | crate::func::Func::RowNum, _) | Kind::RowId(..) => {
            true
        }
        Kind::Contains(_) => false,
        Kind::Binary(BinOp::Shl | BinOp::Shr, ..) | Kind::Const(crate::value::V::Bin(_)) => true,
        _ => crate::query::children_of(node)
            .iter()
            .any(|c| forces_static(c, filter)),
    }
}

/// Whether `node`, a comparison of literals, or its negation, is false: the
/// original folds it, and a `WHERE` with such a conjunct reads no record, so
/// nothing in it fails (`fixtures/sql/wherefilter6.json`). Arithmetic is not
/// folded (`1 + 1 = 3` is evaluated for each record).
pub fn folds_false(node: &Node) -> bool {
    fn folded(node: &Node) -> Option<bool> {
        match &node.kind {
            Kind::Not(inner) => folded(inner).map(|b| !b),
            Kind::Binary(op, a, b)
                if op.is_comparison()
                    && matches!(a.kind, Kind::Const(_))
                    && matches!(b.kind, Kind::Const(_)) =>
            {
                match crate::eval::eval(node, &mut crate::NoRow, &crate::Env::default()) {
                    Ok(crate::V::Bool(b)) => Some(b),
                    _ => None,
                }
            }
            _ => None,
        }
    }
    let mut parts = Vec::new();
    conjuncts(node, &mut parts);
    parts.into_iter().any(|part| folded(part) == Some(false))
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

/// The parts of the `WHERE` `node`, joined by `AND`, that the original runs
/// as a filter on the records of the table whose columns are `range`: those
/// its expression engine has everything of, and that read that table's
/// columns alone. The engine evaluates each whole
/// (`fixtures/sql/wherefilter3.json`).
pub fn engine_conjuncts(node: &Node, range: (usize, usize)) -> Vec<&Node> {
    if folds_false(node) {
        return Vec::new();
    }
    let mut parts = Vec::new();
    conjuncts(node, &mut parts);
    parts.retain(|part| {
        !forces_static(part, true)
            && !part.any(&|n| match &n.kind {
                Kind::Column(i) | Kind::RecNo(i) | Kind::Deleted(i) | Kind::RowId(i, _) => {
                    !(range.0..range.1).contains(i)
                }
                Kind::Outer(..)
                | Kind::Agg(_)
                | Kind::Subquery(_)
                | Kind::Exists(_)
                | Kind::InSelect(..)
                | Kind::Quantified(..)
                | Kind::Contains(_)
                | Kind::Score(..) => true,
                _ => false,
            })
    });
    parts
}

/// Whether the expression engine that filters a table lacks the function.
fn outside_filter_engine(func: crate::func::Func) -> bool {
    use crate::func::Func::*;
    matches!(
        func,
        Insert
            | Replace
            | Mod
            | Sqrt
            | Round
            | Truncate
            | Exp
            | Log
            | Log10
            | Degrees
            | Radians
            | Sin
            | Cos
            | Tan
            | Asin
            | Acos
            | Atan
            | Atan2
            | Cot
            | TimestampAdd
            | TimestampDiff
            | StartsWith
            | EndsWith
            | SubstringOf
    )
}

pub fn plan(select: &Select, scope: Option<Scope<'_>>, params: usize) -> Result<Plan, SqlError> {
    plan_with(select, scope, params, Vec::new(), None)
}

/// [`plan`], told the user-defined functions the live cursor's expressions
/// may call and the connection user `USER` folds to.
pub fn plan_with<'a>(
    select: &'a Select,
    scope: Option<Scope<'a>>,
    params: usize,
    functions: Vec<crate::compile::FunctionSig>,
    user: Option<String>,
) -> Result<Plan, SqlError> {
    let has_scope = scope.is_some();
    let mut compiler = Compiler::new(scope, params);
    compiler.functions = functions;
    compiler.user = user;
    // The `WHERE`'s `CONTAINS` predicates, in order, that `SCORE(n)` numbers.
    if let Some(filter) = &select.filter {
        filter.walk(&mut |e| {
            if matches!(e.kind, ExprKind::Contains { .. }) {
                compiler.where_contains.push(e);
            }
        });
    }
    let mut columns = Vec::new();
    let mut expressions = 0;
    let mut all_columns = true;
    for item in &select.items {
        match item {
            Item::Star { table, pos } => {
                if let Some(table) = table {
                    compiler.column(Some(table), "", *pos).or_else(|e| {
                        if e.native == 2122 {
                            Err(e)
                        } else {
                            Ok((0, Ty::Unknown))
                        }
                    })?;
                }
                match compiler.scope() {
                    Some(scope) => {
                        for (i, field) in scope.fields.iter().enumerate() {
                            let ty = Ty::of_field(field);
                            columns.push(Column {
                                name: field.name.clone(),
                                node: Node {
                                    kind: Kind::Column(i),
                                    ty,
                                    pos: *pos,
                                },
                                ty,
                                field: (field.field_type, field.length(), field.decimals()),
                                alias: None,
                            });
                        }
                    }
                    // `system.iota`'s one field.
                    None => {
                        all_columns = false;
                        columns.push(Column {
                            name: "iota".to_owned(),
                            node: Node {
                                kind: Kind::Const(crate::value::V::Null),
                                ty: Ty::Bool,
                                pos: *pos,
                            },
                            ty: Ty::Bool,
                            field: (FieldType::Logical, 1, 0),
                            alias: None,
                        });
                    }
                }
            }
            Item::Expr { expr, alias } => {
                let node = compiler.compile(expr)?;
                let ty = node.ty;
                let (name, field) = match (&expr.kind, node.kind.clone(), compiler.scope()) {
                    // A column is named as the statement writes it, as in
                    // the original engine (`id` for the field `ID`).
                    (ExprKind::Column { name, .. }, Kind::Column(i), Some(scope)) => {
                        let f = &scope.fields[i];
                        (name.clone(), (f.field_type, f.length(), f.decimals()))
                    }
                    (ExprKind::Column { name, .. }, Kind::RowId(..), _) => {
                        all_columns = false;
                        (name.clone(), ty.column())
                    }
                    // A column given a collation keeps its name.
                    (ExprKind::Collate { expr, .. }, ..) if collated_column(expr).is_some() => {
                        all_columns = false;
                        (collated_column(expr).unwrap_or_default(), ty.column())
                    }
                    _ => {
                        all_columns = false;
                        // Only unnamed expressions count.
                        let name = match (alias, expressions) {
                            (Some(alias), _) => alias.clone(),
                            (None, 0) => "EXPR".to_owned(),
                            (None, n) => format!("EXPR_{n}"),
                        };
                        if alias.is_none() {
                            expressions += 1;
                        }
                        (name, ty.column())
                    }
                };
                columns.push(Column {
                    name: alias.clone().unwrap_or(name),
                    node,
                    ty,
                    field,
                    alias: alias.clone(),
                });
            }
        }
    }
    let filter = match &select.filter {
        Some(expr) => Some(compiler.condition(expr)?),
        None => None,
    };
    let mut order = Vec::new();
    for item in &select.order {
        if crate::query::order_key_ignored(&item.key)? {
            continue;
        }
        let mut by_alias = None;
        let node = match &item.key {
            OrderKey::Ordinal(0, _) => return Err(SqlError::ordinal_invalid(0)),
            OrderKey::Ordinal(n, _) => {
                let index = usize::try_from(*n)
                    .ok()
                    .filter(|&n| n >= 1 && n <= columns.len());
                columns[index.ok_or_else(SqlError::ordinal_too_large)? - 1]
                    .node
                    .clone()
            }
            OrderKey::Expr(expr) => {
                let alias = match &expr.kind {
                    ExprKind::Column { table: None, name } => {
                        columns.iter().find(|c| c.name.eq_ignore_ascii_case(name))
                    }
                    _ => None,
                };
                match alias {
                    Some(column) => {
                        by_alias.clone_from(&column.alias);
                        column.node.clone()
                    }
                    None => crate::compile::ordering(|| compiler.compile(expr))?,
                }
            }
        };
        order.push(SortKey {
            node,
            descending: item.descending,
            alias: by_alias,
        });
    }
    // A `LIKE`, or a memo column used directly, keeps the cursor static (an AOF
    // cannot serve them); but a memo field inside `CONTAINS`/`SCORE` is fine —
    // the full-text index serves it — so those do not descend.
    let static_filter = filter.as_ref().is_some_and(|f| forces_static(f, true))
        || order.iter().any(|key| forces_static(&key.node, false));
    // An `ORDER BY` a `SCORE` does too.
    let scored = order
        .iter()
        .any(|key| matches!(key.node.kind, Kind::Score(..)));
    // An `ORDER BY` that cannot be an index is sorted in a static cursor.
    let sortable = compiler.scope().is_none_or(|scope| {
        !matches!(
            crate::order_index::temp_index(&order, &scope.fields),
            crate::order_index::Order::Unindexable
        )
    });
    let live = !select.force_static
        && !select.distinct
        && select.top.is_none()
        && all_columns
        && has_scope
        && !static_filter
        && !scored
        && sortable;
    Ok(Plan {
        live,
        columns,
        filter,
        order,
        distinct: select.distinct,
        top: select.top,
        params: compiler.params,
    })
}
