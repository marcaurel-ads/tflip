//! The physical plan of a `SELECT`: a tree of operators, which the executor
//! runs (`exec`) and `SHOW PLAN` prints ([`crate::explain`]).
//!
//! A bound [`Query`] says what a `SELECT` asks for; `lower` decides how it
//! runs: which conditions each table's scan checks and which index finds
//! its records, which joins look rows up through an index or a hash, how
//! `UNION`s nest, in which order the rows are sorted and whether a subquery
//! runs once or for each row around it. Everything the executor does is an
//! operator here, so the plan shown is the plan run.
//!
//! Rows flow up the tree. Below a [`Op::Project`] a row is the `SELECT`'s
//! row: the columns of its tables side by side (and, above a
//! [`Op::Group`], its group's aggregates); above it, the result's columns
//! followed by the `ORDER BY` keys, which [`Op::Final`] drops.

use std::rc::Rc;

use tflip_storage::field::FieldType;

use crate::ast::{BinOp, JoinKind, Top};
use crate::compile::{AggSpec, FtsCondition, FtsSource, Kind, Node};
use crate::query::{Query, Source, TableIndex, children_of};
use crate::types::Ty;
use crate::value::{Collation, V};

/// What the expressions of a `SELECT` read, for showing them: each column
/// of its row as `[table].[column]`, its aggregates and the `SELECT`
/// around it.
#[derive(Debug)]
pub struct Context {
    pub labels: Vec<String>,
    pub aggregates: Vec<AggSpec>,
    pub outer: Option<Rc<Context>>,
}

/// An operator of a plan.
#[derive(Debug)]
pub enum Op {
    /// Reads the records of table `table` into the row's columns from
    /// `start` on, rows `width` values wide, and keeps those the
    /// `restrictions` hold for: every record, or those the restrictions'
    /// indexes find.
    Scan {
        table: usize,
        start: usize,
        width: usize,
        name: String,
        alias: String,
        restrictions: Vec<Restriction>,
        /// The parts of the `WHERE` the original runs as a filter on the
        /// table's records, evaluating each whole, so that a division by zero
        /// in one fails though another already decided the record.
        engine: Vec<Node>,
        ctx: Rc<Context>,
    },
    /// Runs a `SELECT` in `FROM` and puts its rows' values from `start` on.
    Derived {
        input: Box<Op>,
        start: usize,
        width: usize,
        alias: String,
    },
    /// Joins the rows of `left` with those of `right`, whose columns are
    /// `right_range` of the row, where `condition` holds.
    Join {
        kind: JoinKind,
        method: JoinMethod,
        left: Box<Op>,
        right: Box<Op>,
        right_range: (usize, usize),
        condition: Option<Node>,
        ctx: Rc<Context>,
    },
    /// Keeps the rows `condition` holds for.
    Filter {
        input: Box<Op>,
        condition: Node,
        clause: Clause,
        ctx: Rc<Context>,
    },
    /// Sorts the rows.
    Sort {
        input: Box<Op>,
        keys: Vec<SortSpec>,
        purpose: Purpose,
        ctx: Rc<Context>,
    },
    /// Makes a row of each run of rows with equal `keys` (the input is
    /// sorted on them; all rows are one group without keys, even none): its
    /// first row (or NULLs, `width` of them) and `aggregates` over the run.
    Group {
        input: Box<Op>,
        keys: Vec<Node>,
        aggregates: Vec<AggSpec>,
        width: usize,
        ctx: Rc<Context>,
    },
    /// Computes the result's `columns` and the `ORDER BY` `keys` of each row.
    Project {
        input: Box<Op>,
        columns: Vec<Node>,
        keys: Vec<Node>,
        ctx: Rc<Context>,
    },
    /// Sorts the rows on their first `columns.len()` values and keeps one
    /// of each run of equal ones: the first, or the last with `keep_last`.
    Distinct {
        input: Box<Op>,
        columns: Vec<Node>,
        keep_last: bool,
        ctx: Rc<Context>,
    },
    /// Skips to `TOP`'s first row and keeps its count of rows.
    Top { input: Box<Op>, top: Top },
    /// The rows of each input, one after the other (`UNION ALL`).
    Concat { inputs: Vec<Op> },
    /// Fits a `UNION`'s values to its columns' `types`: VARCHAR text loses
    /// its trailing blanks if `trim` (a `UNION` that is distinct somewhere:
    /// `UNION ALL` keeps what a `CHAR` column had), a number too large for a
    /// NUMERIC is an error.
    Fit {
        input: Box<Op>,
        types: Vec<Ty>,
        trim: bool,
    },
    /// The result's first `columns` values of each row; the rows of a
    /// [`Purpose::Shown`] sort below stay as they are, shown sorted.
    Final { input: Box<Op>, columns: usize },
}

/// How a join finds the right rows for a left one.
#[derive(Debug)]
pub enum JoinMethod {
    /// Every right row, the condition deciding.
    Loop,
    /// The right rows with equal values in the [`Equality`]s' columns,
    /// looked up in a hash of the right rows; the condition decides.
    Hash(Vec<Equality>),
    /// The records of the right side, a [`Op::Scan`], whose column equals
    /// the left row's in `key`, looked up in `index` for each left row; the
    /// scan's restrictions and the condition decide.
    Index { index: IndexUse, key: Equality },
}

/// An index order a scan or join looks records up in.
#[derive(Debug, Clone)]
pub struct IndexUse {
    /// Its place among the table's orders ([`crate::query::Tables::indexes`]).
    pub number: usize,
    pub name: String,
    pub expression: String,
}

/// A condition on one column of a table that its scan checks on each
/// record: a part of `WHERE` (or of a join's condition) that compares the
/// column with values from outside the row (constants, parameters, columns
/// of the `SELECT` around), so it never holds for NULL.
#[derive(Debug)]
pub struct Restriction {
    /// The condition, over the row.
    pub condition: Node,
    /// The column, by its place in the row.
    pub column: usize,
    /// The ranges the column's value is in where the condition holds, each
    /// bound included; none for no bound. The bounds are over no row.
    pub ranges: Vec<(Option<Node>, Option<Node>)>,
    /// The order whose keys are the column's values, which finds the
    /// records in the ranges, if one can.
    pub index: Option<IndexUse>,
    /// Whether the condition is one range of the order from a value on
    /// (`=`, `>=`, `<=`): the original engine reads a table whose only
    /// restriction that is through the order, in its order, where it reads
    /// those of others in record order.
    pub scope: bool,
    /// Whether the index finds the records of the ranges. It does not for a
    /// range of text sorted by a collation: SQL compares that text by its
    /// bytes, and tflip does not know which keys the weights put in a range.
    pub finds: bool,
    /// The columns (by their place in the row, and whether they are
    /// CICHARACTER text) the original engine orders the rows of a scope by,
    /// with an order of them to read the table in or without: the fields of
    /// an order that starts with the column, else the column.
    pub sort: Vec<(usize, bool)>,
    /// Whether the table is the whole of the `FROM` and this scope the only
    /// restriction (or all of them the scope of one order): the original
    /// engine reads the table in the order's order then.
    pub ordered: bool,
    /// Whether it is also the whole of the `WHERE`: the rows are sorted by
    /// `sort` then even with no order to read them in.
    pub sole: bool,
    /// Whether it is a `CONTAINS` (with a constant condition), which the
    /// field's full-text order serves rather than `index`.
    pub fts: bool,
}

/// Which condition a filter is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clause {
    Where,
    Having,
}

/// What a sort is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Grouping: equal keys stay in the order of the rows.
    Group,
    /// `ORDER BY`, the rows stored in that order.
    Stored,
    /// `ORDER BY` as a temporary index orders the rows: NULL text first
    /// either way, equal keys in the order of the rows or, when the last
    /// key descends, the reverse.
    Indexed,
    /// As [`Purpose::Indexed`], but the rows stay as they are and are shown
    /// in that order.
    Shown,
}

/// A sort key.
#[derive(Debug)]
pub struct SortSpec {
    pub by: SortBy,
    pub descending: bool,
    /// Text compared regardless of case.
    pub ci: bool,
    pub text: bool,
    /// The key as the statement wrote it, for showing it.
    pub shown: Node,
}

/// What a sort key is.
#[derive(Debug)]
pub enum SortBy {
    /// An expression over the row.
    Node(Node),
    /// The row's value at this place.
    Place(usize),
}

/// An equality between a column of the left rows and one of the right, by
/// their places in the row.
#[derive(Debug, Clone)]
pub struct Equality {
    pub left: usize,
    pub right: usize,
    /// Text compared regardless of case.
    pub ci: bool,
    /// The right column's type.
    pub ty: Ty,
}

/// A subquery's plan, and whether it reads rows around it (else it runs
/// once and its rows are kept).
#[derive(Debug)]
pub struct SubPlan {
    pub op: Op,
    pub correlated: bool,
}

/// Lowers `query`, whose subqueries are `subqueries`, into its plan; the
/// subqueries' plans go to `plans` by number. `outer` is the context of the
/// `SELECT` around it.
pub(crate) fn lower(
    query: &Query,
    outer: Option<Rc<Context>>,
    subqueries: &[Query],
    plans: &mut Vec<Option<SubPlan>>,
) -> Op {
    let ctx = context(query, outer.clone());
    lower_subqueries(query, &ctx, subqueries, plans);
    let n = query.columns.len();
    if query.unions.is_empty() {
        let mut op = pipeline(query, &ctx, true, subqueries, plans);
        if query.distinct {
            op = distinct(op, query, &ctx, false);
        }
        if !query.order.is_empty() {
            // `DISTINCT` stores its rows sorted on the columns: a sort on
            // them shows them so.
            let purpose = match (query.top.is_some(), query.distinct, query.shown_sorted) {
                (true, _, _) | (false, true, false) => Purpose::Indexed,
                (false, _, true) => Purpose::Shown,
                (false, false, false) => Purpose::Stored,
            };
            let keys = query
                .order
                .iter()
                .enumerate()
                .map(|(i, k)| SortSpec {
                    by: SortBy::Place(n + i),
                    descending: k.descending,
                    ci: k.node.ty.case_insensitive(),
                    text: k.node.ty.is_text(),
                    shown: k.node.clone(),
                })
                .collect();
            op = Op::Sort {
                input: Box::new(op),
                keys,
                purpose,
                ctx: Rc::clone(&ctx),
            };
        }
        if let Some(top) = query.top {
            op = Op::Top {
                input: Box::new(op),
                top,
            };
        }
        return Op::Final {
            input: Box::new(op),
            columns: n,
        };
    }

    // Each `SELECT` of the `UNION`, made distinct and cut to its `TOP`;
    // the `UNION`s bind to the right.
    let first = trimmed(pipeline(query, &ctx, false, subqueries, plans), query, &ctx);
    let mut parts = vec![first];
    for (_, part) in &query.unions {
        let part_ctx = context(part, outer.clone());
        lower_subqueries(part, &part_ctx, subqueries, plans);
        let op = pipeline(part, &part_ctx, false, subqueries, plans);
        parts.push(trimmed(op, part, &part_ctx));
    }
    let mut op = parts.pop().expect("a UNION has parts");
    for (i, part) in parts.into_iter().enumerate().rev() {
        op = Op::Concat {
            inputs: vec![part, op],
        };
        if !query.unions[i].0 {
            op = Op::Distinct {
                input: Box::new(op),
                columns: query.columns.iter().map(|c| c.node.clone()).collect(),
                keep_last: true,
                ctx: Rc::clone(&ctx),
            };
        }
    }
    op = Op::Fit {
        input: Box::new(op),
        types: query.columns.iter().map(|c| c.ty).collect(),
        trim: query.unions.iter().any(|(all, _)| !all),
    };
    if !query.order.is_empty() {
        // The keys are the result's columns.
        let mut columns = Vec::with_capacity(n);
        for (i, c) in query.columns.iter().enumerate() {
            columns.push(Node::new(Kind::Column(i), c.ty, 0));
        }
        let keys: Vec<Node> = query.order.iter().map(|k| k.node.clone()).collect();
        let shown = |k: &Node| match k.kind {
            Kind::Column(i) => query.columns[i].node.clone(),
            _ => k.clone(),
        };
        let specs = query
            .order
            .iter()
            .enumerate()
            .map(|(i, k)| SortSpec {
                by: SortBy::Place(n + i),
                descending: k.descending,
                ci: k.node.ty.case_insensitive(),
                text: k.node.ty.is_text(),
                shown: shown(&k.node),
            })
            .collect();
        op = Op::Project {
            input: Box::new(op),
            columns,
            keys,
            ctx: Rc::clone(&ctx),
        };
        let purpose = if query.shown_sorted {
            Purpose::Shown
        } else {
            Purpose::Stored
        };
        op = Op::Sort {
            input: Box::new(op),
            keys: specs,
            purpose,
            ctx: Rc::clone(&ctx),
        };
    }
    Op::Final {
        input: Box::new(op),
        columns: n,
    }
}

pub(crate) fn context(query: &Query, outer: Option<Rc<Context>>) -> Rc<Context> {
    Rc::new(Context {
        labels: query.labels.clone(),
        aggregates: query.aggregates.clone(),
        outer,
    })
}

/// Lowers the subqueries `ids`, whose rows around them are `ctx`'s.
pub(crate) fn lower_ids(
    ids: &[usize],
    ctx: &Rc<Context>,
    subqueries: &[Query],
    plans: &mut Vec<Option<SubPlan>>,
) {
    for &id in ids {
        if plans[id].is_some() {
            continue;
        }
        let sub = &subqueries[id];
        let op = lower(sub, Some(Rc::clone(ctx)), subqueries, plans);
        plans[id] = Some(SubPlan {
            op,
            correlated: sub.depth > 0,
        });
    }
}

/// Lowers the subqueries `query`'s expressions run, whose rows around
/// them are `ctx`'s.
fn lower_subqueries(
    query: &Query,
    ctx: &Rc<Context>,
    subqueries: &[Query],
    plans: &mut Vec<Option<SubPlan>>,
) {
    let mut ids = Vec::new();
    for node in query_nodes(query) {
        subquery_ids(node, &mut ids);
    }
    lower_ids(&ids, ctx, subqueries, plans);
}

/// The expressions of `query` (not of its `UNION`s or `SELECT`s in
/// `FROM`).
fn query_nodes(query: &Query) -> Vec<&Node> {
    let mut nodes: Vec<&Node> = query.columns.iter().map(|c| &c.node).collect();
    nodes.extend(query.filter.iter().chain(&query.having).chain(&query.group));
    nodes.extend(query.order.iter().map(|k| &k.node));
    nodes.extend(query.aggregates.iter().filter_map(|a| a.arg.as_ref()));
    fn joins<'a>(source: &'a Source, out: &mut Vec<&'a Node>) {
        if let Source::Join {
            left, right, on, ..
        } = source
        {
            joins(left, out);
            joins(right, out);
            out.extend(on);
        }
    }
    joins(&query.from, &mut nodes);
    nodes
}

/// The subqueries `node` runs, in order.
pub(crate) fn subquery_ids(node: &Node, out: &mut Vec<usize>) {
    if let Kind::Subquery(id)
    | Kind::Exists(id)
    | Kind::InSelect(_, id, _)
    | Kind::Quantified(_, _, id, _) = node.kind
    {
        out.push(id);
    }
    for child in children_of(node) {
        subquery_ids(child, out);
    }
}

/// A `SELECT`'s rows up to its result's columns (and, with `keys`, its
/// `ORDER BY` keys).
fn pipeline(
    query: &Query,
    ctx: &Rc<Context>,
    keys: bool,
    subqueries: &[Query],
    plans: &mut Vec<Option<SubPlan>>,
) -> Op {
    let width = query.from.range().1;
    let mut pushed = Vec::new();
    if let Some(filter) = &query.filter {
        conjuncts(filter, &mut pushed);
    }
    let from = Lowering {
        width,
        ctx,
        filter: query.filter.as_ref(),
        engine: true,
        swap: query.swap_joins,
        alone: matches!(query.from, Source::Table { .. }),
    };
    let mut op = from.source(&query.from, &pushed, subqueries, plans);
    if let Some(filter) = &query.filter {
        op = Op::Filter {
            input: Box::new(op),
            condition: filter.clone(),
            clause: Clause::Where,
            ctx: Rc::clone(ctx),
        };
    }
    if let Some(key) = &query.like_order {
        op = Op::Sort {
            input: Box::new(op),
            keys: vec![SortSpec {
                by: SortBy::Node(key.clone()),
                descending: false,
                ci: key.ty.case_insensitive(),
                text: key.ty.is_text(),
                shown: key.clone(),
            }],
            purpose: Purpose::Group,
            ctx: Rc::clone(ctx),
        };
    }
    if query.grouped {
        if !query.group.is_empty() {
            // Grouped in the order of `ORDER BY` when the rows are stored
            // sorted on the group keys anyway.
            let keys = query
                .group
                .iter()
                .enumerate()
                .map(|(i, g)| SortSpec {
                    by: SortBy::Node(g.clone()),
                    descending: !query.shown_sorted
                        && query.order.get(i).is_some_and(|k| k.descending),
                    ci: g.ty.case_insensitive(),
                    text: g.ty.is_text(),
                    shown: g.clone(),
                })
                .collect();
            op = Op::Sort {
                input: Box::new(op),
                keys,
                purpose: Purpose::Group,
                ctx: Rc::clone(ctx),
            };
        }
        op = Op::Group {
            input: Box::new(op),
            keys: query.group.clone(),
            aggregates: query.aggregates.clone(),
            width,
            ctx: Rc::clone(ctx),
        };
        if let Some(having) = &query.having {
            op = Op::Filter {
                input: Box::new(op),
                condition: having.clone(),
                clause: Clause::Having,
                ctx: Rc::clone(ctx),
            };
        }
    }
    let keys = match keys {
        true => query.order.iter().map(|k| k.node.clone()).collect(),
        false => Vec::new(),
    };
    Op::Project {
        input: Box::new(op),
        columns: query.columns.iter().map(|c| c.node.clone()).collect(),
        keys,
        ctx: Rc::clone(ctx),
    }
}

/// A `SELECT` of a `UNION` made distinct and cut to its `TOP`.
fn trimmed(op: Op, query: &Query, ctx: &Rc<Context>) -> Op {
    let mut op = op;
    if query.distinct {
        op = distinct(op, query, ctx, false);
    }
    if let Some(top) = query.top {
        op = Op::Top {
            input: Box::new(op),
            top,
        };
    }
    op
}

fn distinct(op: Op, query: &Query, ctx: &Rc<Context>, keep_last: bool) -> Op {
    Op::Distinct {
        input: Box::new(op),
        columns: query.columns.iter().map(|c| c.node.clone()).collect(),
        keep_last,
        ctx: Rc::clone(ctx),
    }
}

/// How the `FROM` of a `SELECT` is lowered: its rows are `width` values
/// wide, and `filter` is the `WHERE` they will be filtered with, whose
/// equalities serve joins without a condition of their own.
struct Lowering<'a> {
    width: usize,
    ctx: &'a Rc<Context>,
    filter: Option<&'a Node>,
    /// Whether the tables run the `WHERE`'s parts their expression engine has
    /// as filters (not the `WHERE` of a join's condition).
    engine: bool,
    /// Whether an inner join of two tables reads the one with fewer records
    /// first.
    swap: bool,
    /// Whether the `FROM` is one table.
    alone: bool,
}

impl Lowering<'_> {
    /// The rows of `source`, whose tables check the conditions of `pushed`
    /// that are restrictions on them. Each of those must hold for a row to
    /// be kept, and fails on NULL, so a restriction can go to any table: the
    /// rows of the other side of an outer join that lose their partners by
    /// it are NULL-extended and then fail it.
    fn source(
        &self,
        source: &Source,
        pushed: &[&Node],
        subqueries: &[Query],
        plans: &mut Vec<Option<SubPlan>>,
    ) -> Op {
        let ctx = self.ctx;
        match source {
            Source::Table {
                id,
                start,
                width,
                name,
                alias,
                indexes,
                ..
            } => {
                let range = (*start, start + width);
                let mut restrictions: Vec<Restriction> = pushed
                    .iter()
                    .filter_map(|node| restriction(node, range, indexes))
                    .collect();
                // A scope orders the rows of a table that is the whole of the
                // `FROM` and whose restrictions are the whole of the `WHERE`.
                if self.alone {
                    scope_together(&mut restrictions, pushed.len(), indexes, range);
                }
                let engine = match self.filter {
                    Some(filter) if self.engine => crate::plan::engine_conjuncts(filter, range)
                        .into_iter()
                        .cloned()
                        .collect(),
                    _ => Vec::new(),
                };
                Op::Scan {
                    table: *id,
                    start: *start,
                    width: self.width,
                    name: name.clone(),
                    alias: alias.clone(),
                    restrictions,
                    engine,
                    ctx: Rc::clone(ctx),
                }
            }
            Source::Derived {
                query,
                start,
                alias,
                ..
            } => Op::Derived {
                input: Box::new(lower(query, ctx.outer.clone(), subqueries, plans)),
                start: *start,
                width: self.width,
                alias: alias.clone(),
            },
            Source::Join {
                kind,
                left,
                right,
                on,
            } => {
                // The original engine reads the smaller of two tables of an
                // inner join first, the right one only if it is the smaller
                // by its records (those its restrictions hold for halved
                // for each); equal, the left one stays first
                // (`fixtures/sql/joinorder.json`, `joinsize.json`).
                let (left, right) = match (&**left, &**right) {
                    (
                        Source::Table {
                            records: Some(a), ..
                        },
                        Source::Table {
                            records: Some(b), ..
                        },
                    ) if self.swap && *kind == JoinKind::Inner => {
                        let mut nodes = pushed.to_vec();
                        nodes.extend(
                            on.as_ref()
                                .map(|on| {
                                    let mut own = Vec::new();
                                    conjuncts(on, &mut own);
                                    own
                                })
                                .unwrap_or_default(),
                        );
                        let size = |source: &Source, records: usize| {
                            let range = source.range();
                            let own = nodes.iter().filter(|n| only_within(n, range)).count();
                            records as f64 / f64::powi(2.0, own as i32)
                        };
                        match size(right, *b) < size(left, *a) {
                            true => (right, left),
                            false => (left, right),
                        }
                    }
                    _ => (left, right),
                };
                let keys = match (on, self.filter) {
                    (Some(on), _) => equalities(on, left.range(), right.range()),
                    (None, Some(filter)) => equalities(filter, left.range(), right.range()),
                    (None, None) => Vec::new(),
                };
                // The join's condition restricts the sides an outer join does
                // not keep every row of.
                let mut own = Vec::new();
                if let Some(on) = on {
                    conjuncts(on, &mut own);
                }
                let with_own = |takes: bool| {
                    let mut nodes = pushed.to_vec();
                    if takes {
                        nodes.extend(&own);
                    }
                    nodes
                };
                let (left_takes, right_takes) = match kind {
                    JoinKind::Inner => (true, true),
                    JoinKind::Left => (false, true),
                    JoinKind::Right => (true, false),
                    JoinKind::Full => (false, false),
                };
                let left_op = self.source(left, &with_own(left_takes), subqueries, plans);
                let mut right_op = self.source(right, &with_own(right_takes), subqueries, plans);
                // A restriction on the right table that one of its orders
                // serves is read as an AOF, not through the join's index.
                let served = matches!(&right_op, Op::Scan { restrictions, .. }
                    if restrictions.iter().any(|r| r.index.is_some() && !reads_outer(&r.condition)));
                let method = match index_join(*kind, right, &keys).filter(|_| !served) {
                    Some(method) => {
                        // The join finds the right side's records.
                        if let Op::Scan { restrictions, .. } = &mut right_op {
                            for restriction in restrictions {
                                restriction.index = None;
                            }
                        }
                        method
                    }
                    None if keys.is_empty() => JoinMethod::Loop,
                    None => JoinMethod::Hash(keys),
                };
                Op::Join {
                    kind: *kind,
                    method,
                    left: Box::new(left_op),
                    right: Box::new(right_op),
                    right_range: right.range(),
                    condition: on.clone(),
                    ctx: Rc::clone(ctx),
                }
            }
        }
    }
}

/// Marks `restrictions`, all of the `WHERE`, as one scope the rows are read
/// in the order of: one `=`, `>=` or `<=`, or several `=` on the leading
/// fields of one order (`c = 1 AND d = 2` of an order on `c, d`), which the
/// original engine finds in that order's keys.
fn scope_together(
    restrictions: &mut [Restriction],
    conditions: usize,
    indexes: &[TableIndex],
    range: (usize, usize),
) {
    let eq = |r: &Restriction| matches!(r.condition.kind, Kind::Binary(BinOp::Eq, ..));
    match restrictions {
        [] => {}
        [only] => {
            only.ordered = only.scope && !only.fts;
            only.sole = only.ordered && conditions == 1;
        }
        all if all.len() == conditions && all.iter().all(|r| r.scope && !r.fts && eq(r)) => {
            let columns: Vec<usize> = all.iter().map(|r| r.column).collect();
            // The order the restriction on its first field finds the
            // records in, if the restrictions are on its leading fields.
            let found = all.iter().find_map(|r| {
                let number = r.index.as_ref()?.number;
                let fields: Vec<usize> =
                    indexes[number].fields.iter().map(|f| range.0 + f).collect();
                let leading = fields.first() == Some(&r.column);
                let distinct: std::collections::BTreeSet<usize> = columns.iter().copied().collect();
                let prefix: std::collections::BTreeSet<usize> =
                    fields.iter().copied().take(distinct.len()).collect();
                (leading && prefix == distinct).then_some((number, fields))
            });
            let Some((number, fields)) = found else {
                return;
            };
            let index = IndexUse {
                number,
                name: indexes[number].name.clone(),
                expression: indexes[number].expression.clone(),
            };
            let sort: Vec<(usize, bool)> = fields
                .iter()
                .zip(&indexes[number].ci)
                .map(|(&field, &ci)| (field, ci))
                .collect();
            let first = fields[0];
            for r in all {
                r.finds = r.finds && r.column == first;
                r.index = Some(IndexUse {
                    number,
                    name: index.name.clone(),
                    expression: index.expression.clone(),
                });
                r.sort = sort.clone();
                r.ordered = true;
                r.sole = true;
            }
        }
        _ => {}
    }
}

/// Whether `node` reads a column of a `SELECT` around.
fn reads_outer(node: &Node) -> bool {
    matches!(node.kind, Kind::Outer(..)) || children_of(node).into_iter().any(reads_outer)
}

/// Whether `node` reads columns of the row's `range` and no others.
fn only_within(node: &Node, range: (usize, usize)) -> bool {
    fn walk(node: &Node, range: (usize, usize), found: &mut bool) -> bool {
        match &node.kind {
            Kind::Column(i) => {
                *found = true;
                (range.0..range.1).contains(i)
            }
            Kind::Outer(..) => false,
            _ => children_of(node).into_iter().all(|c| walk(c, range, found)),
        }
    }
    let mut found = false;
    walk(node, range, &mut found) && found
}

/// The parts of `node` joined by `AND`, in order.
fn conjuncts<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
    match &node.kind {
        Kind::Binary(BinOp::And, a, b) => {
            conjuncts(a, out);
            conjuncts(b, out);
        }
        _ => out.push(node),
    }
}

/// How a join of `kind` with `right` on equalities `keys` looks the right
/// rows up through an index: for an inner or left join of a table with an
/// order on a column of an equality. A right or full join needs all the
/// right rows.
fn index_join(kind: JoinKind, right: &Source, keys: &[Equality]) -> Option<JoinMethod> {
    let Source::Table { start, indexes, .. } = right else {
        return None;
    };
    if !matches!(kind, JoinKind::Inner | JoinKind::Left) {
        return None;
    }
    keys.iter().find_map(|key| {
        if !indexable(key.ty, key.ci, false, true) {
            return None;
        }
        let number = (0..indexes.len())
            .filter(|&number| start + indexes[number].field == key.right)
            .min_by_key(|&number| indexes[number].fields.len() > 1)?;
        let index = &indexes[number];
        Some(JoinMethod::Index {
            index: IndexUse {
                number,
                name: index.name.clone(),
                expression: index.expression.clone(),
            },
            key: key.clone(),
        })
    })
}

/// Whether an order on a column of type `ty` serves the values that compare
/// with it (regardless of case with `ci`) as equal, or with `ordered` in a
/// range; `collated` if its text keys sort by a collation rather than as
/// SQL compares text. A CHARACTER order tells case apart.
fn indexable(ty: Ty, ci: bool, ordered: bool, collated: bool) -> bool {
    match ty {
        Ty::Str {
            field: FieldType::Character,
            ..
        } => !ci && !(ordered && collated),
        // Its keys are in lower case, as SQL compares it.
        Ty::Str {
            field: FieldType::CiCharacter,
            ..
        } => !(ordered && collated),
        Ty::Int { .. }
        | Ty::Num { .. }
        | Ty::Double
        | Ty::CurDouble
        | Ty::Money
        | Ty::Date
        | Ty::Time
        | Ty::TimeStamp
        | Ty::Bool => true,
        _ => false,
    }
}

/// Whether `node` is a value from outside the row: a constant, a
/// parameter or a column of a `SELECT` around.
fn outside(node: &Node) -> bool {
    match &node.kind {
        Kind::Const(_) | Kind::Param(_) | Kind::Outer(..) => true,
        Kind::Neg(a) => outside(a),
        _ => false,
    }
}

/// `node` as a restriction on the table whose columns are `range` of the
/// row, with `indexes`, if it is one.
fn restriction(node: &Node, range: (usize, usize), indexes: &[TableIndex]) -> Option<Restriction> {
    let own = |n: &Node| match n.kind {
        Kind::Column(i) if (range.0..range.1).contains(&i) => Some(i),
        _ => None,
    };
    let bound = |n: &Node| Some(n.clone());
    // A full-text search of the table's own fields with a constant condition.
    if let Kind::Contains(search) = &node.kind {
        if !matches!(search.condition, FtsCondition::Const(_)) {
            return None;
        }
        let column = match &search.source {
            FtsSource::Expr(field) => own(field)?,
            FtsSource::Fields(fields) => {
                let columns: Option<Vec<usize>> = fields.iter().map(|(f, _)| own(f)).collect();
                *columns?.first()?
            }
        };
        let (ranges, index) = (Vec::new(), None);
        return Some(Restriction {
            condition: node.clone(),
            column,
            ranges,
            index,
            scope: false,
            finds: true,
            sort: Vec::new(),
            ordered: false,
            sole: false,
            fts: true,
        });
    }
    type Parts<'n> = (
        usize,
        Vec<(Option<Node>, Option<Node>)>,
        bool,
        bool,
        Vec<&'n Node>,
    );
    let (column, ranges, ordered, scope, keys): Parts<'_> = match &node.kind {
        Kind::Column(i) if own(node).is_some() && node.ty == Ty::Bool => {
            let yes = Node::new(Kind::Const(V::Bool(true)), Ty::Bool, 0);
            (
                *i,
                vec![(Some(yes.clone()), Some(yes))],
                false,
                true,
                Vec::new(),
            )
        }
        Kind::Binary(
            op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge),
            a,
            b,
        ) => {
            let (column, key, op) = match (own(a), own(b)) {
                (Some(c), _) if outside(b) => (c, &**b, *op),
                (_, Some(c)) if outside(a) => (c, &**a, flipped(*op)),
                _ => return None,
            };
            let ranges = match op {
                BinOp::Eq => vec![(bound(key), bound(key))],
                BinOp::Ne => vec![(None, bound(key)), (bound(key), None)],
                BinOp::Lt | BinOp::Le => vec![(None, bound(key))],
                _ => vec![(bound(key), None)],
            };
            let scope = matches!(op, BinOp::Eq | BinOp::Le | BinOp::Ge);
            (column, ranges, op != BinOp::Eq, scope, vec![key])
        }
        Kind::Between(a, low, high, false) if outside(low) && outside(high) => (
            own(a)?,
            vec![(bound(low), bound(high))],
            true,
            false,
            vec![&**low, &**high],
        ),
        Kind::In(a, list, false) if list.iter().all(outside) => {
            let ranges = list.iter().map(|k| (bound(k), bound(k))).collect();
            (own(a)?, ranges, false, false, list.iter().collect())
        }
        _ => return None,
    };
    let ty = match &node.kind {
        Kind::Column(_) => node.ty,
        Kind::Binary(_, a, b) => {
            if own(a).is_some() {
                a.ty
            } else {
                b.ty
            }
        }
        Kind::Between(a, ..) | Kind::In(a, ..) => a.ty,
        _ => return None,
    };
    let ci = keys.iter().any(|k| k.ty.case_insensitive());
    // The order on the column alone, else the first that starts with it.
    let leading = |number: &usize| range.0 + indexes[*number].field == column;
    let number = (0..indexes.len())
        .filter(leading)
        .min_by_key(|&number| indexes[number].fields.len() > 1);
    let index = number
        .filter(|_| indexable(ty, ci, false, false))
        .map(|number| IndexUse {
            number,
            name: indexes[number].name.clone(),
            expression: indexes[number].expression.clone(),
        });
    // The original engine scans the table for an `IN` list, which it takes
    // for `OR`ed equalities (`fixtures/sql/indexsel.json`).
    let finds = !matches!(node.kind, Kind::In(..))
        && number.is_none_or(|number| indexable(ty, ci, ordered, indexes[number].collated));
    let sort = match number {
        Some(number) => indexes[number]
            .fields
            .iter()
            .zip(&indexes[number].ci)
            .map(|(&field, &ci)| (range.0 + field, ci))
            .collect(),
        None => vec![(column, ty.case_insensitive())],
    };
    Some(Restriction {
        condition: node.clone(),
        column,
        ranges,
        index,
        scope,
        finds,
        sort,
        ordered: false,
        sole: false,
        fts: false,
    })
}

/// The comparison `a op b` written as `b op' a`.
fn flipped(op: BinOp) -> BinOp {
    match op {
        BinOp::Lt => BinOp::Gt,
        BinOp::Le => BinOp::Ge,
        BinOp::Gt => BinOp::Lt,
        BinOp::Ge => BinOp::Le,
        op => op,
    }
}

/// The equalities in the `AND`s of `condition` between a column in `left`
/// and one in `right` (ranges of the row) of types whose values a hash
/// tells apart as comparisons do: integers, dates, logicals and text.
pub(crate) fn equalities(
    condition: &Node,
    left: (usize, usize),
    right: (usize, usize),
) -> Vec<Equality> {
    let mut out = Vec::new();
    let mut stack = vec![condition];
    while let Some(node) = stack.pop() {
        match &node.kind {
            Kind::Binary(BinOp::And, a, b) => {
                stack.push(b);
                stack.push(a);
            }
            Kind::Binary(BinOp::Eq, a, b) => {
                let (Kind::Column(x), Kind::Column(y)) = (&a.kind, &b.kind) else {
                    continue;
                };
                let within = |i: &usize, (from, to): (usize, usize)| (from..to).contains(i);
                let (l, r) = if within(x, left) && within(y, right) {
                    (*x, *y)
                } else if within(y, left) && within(x, right) {
                    (*y, *x)
                } else {
                    continue;
                };
                let plain = |f| {
                    matches!(
                        f,
                        FieldType::Character | FieldType::CiCharacter | FieldType::VarChar
                    )
                };
                let keyed = match (a.ty, b.ty) {
                    (Ty::Int { .. }, Ty::Int { .. })
                    | (Ty::Date, Ty::Date)
                    | (Ty::Bool, Ty::Bool) => true,
                    (Ty::Str { field: fa, .. }, Ty::Str { field: fb, .. }) => {
                        plain(fa) && plain(fb)
                    }
                    _ => false,
                };
                if keyed {
                    let ci = a.ty.case_insensitive() || b.ty.case_insensitive();
                    let ty = if l == *x { b.ty } else { a.ty };
                    out.push(Equality {
                        left: l,
                        right: r,
                        ci,
                        ty,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// The value a join's hash looks `v` up by: equal for values that compare
/// equal; none for NULL, which equals nothing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum HashKey {
    Int(i64),
    Text(Vec<u8>),
    Date(i32),
    Bool(bool),
}

impl Equality {
    /// The key of `v`, text by the weights `collation` compares it by.
    pub(crate) fn key(&self, v: &V, collation: Option<&Collation>) -> Option<HashKey> {
        Some(match v {
            // A blank field joins as its blank value, as `=` compares it.
            V::Blank(blank) => return self.key(&blank.value, collation),
            V::Int(n) => HashKey::Int(*n),
            V::Str(s) => {
                let s = s.trim_end_matches(' ');
                if let Some(dynamic) = collation.and_then(Collation::dynamic) {
                    return Some(HashKey::Text(crate::value::comparison_key(
                        s, self.ci, dynamic,
                    )));
                }
                let mut bytes = tflip_storage::cp1252::encode(s);
                match collation {
                    Some(Collation::Static(collation)) => {
                        for byte in &mut bytes {
                            let b = match self.ci {
                                true => collation.lower[usize::from(*byte)],
                                false => *byte,
                            };
                            *byte = collation.weights[usize::from(b)];
                        }
                    }
                    _ if self.ci => bytes.make_ascii_uppercase(),
                    _ => {}
                }
                HashKey::Text(bytes)
            }
            V::Date(d) => HashKey::Date(*d),
            V::Bool(b) => HashKey::Bool(*b),
            _ => return None,
        })
    }
}
