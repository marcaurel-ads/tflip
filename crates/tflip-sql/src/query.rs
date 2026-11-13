//! `SELECT`s bound to their tables: names resolved, expressions typed and
//! checked, as the original engine checks them, into a [`Query`]; then
//! lowered into a plan ([`crate::ops`]), which [`Prepared::run`] runs.
//!
//! What the original engine does is learnt from `fixtures/sql/select.json`:
//!
//! - Joins nest loops, the left table outermost; a `RIGHT` or `FULL` join
//!   adds the right rows that matched nothing after the others.
//! - Groups come in the order of their keys, NULL first. `COUNT` gives an
//!   INTEGER; `SUM` keeps its argument's type, a NUMERIC with 10 more
//!   digits; `AVG` keeps the type too, an integer average rounded to even,
//!   MONEY and NUMERIC ones cut off.
//! - `UNION`s bind to the right: `a UNION ALL b UNION c` is
//!   `a UNION ALL (b UNION c)`. `UNION` sorts its rows and drops the
//!   repeated ones; the first `SELECT` names the columns.
//! - `ORDER BY` columns of the results leave the rows as they are and
//!   show them sorted (a temporary index in the original); other keys,
//!   `TOP` and `DISTINCT` store them sorted.

use std::borrow::Cow;

use tflip_storage::field::{Field, FieldType};

use crate::ast::{self, ExprKind, From, Item, JoinKind, OrderKey, Select, Top};
use crate::compile::{AggContext, AggSpec, Compiler, Kind, Node, Scope, SubInfo, collated_column};
use crate::error::SqlError;
use crate::eval::{Env, Row};
use crate::exec::Exec;
use crate::ops::{self, Op, SubPlan};
use crate::plan::{Column, SortKey};
use crate::types::Ty;
use crate::value::V;

/// The tables a statement reads, opened by the engine: by
/// [`ast::TableRef::id`].
pub trait Tables {
    /// The table's fields; none for `system.iota`.
    fn fields(&self, id: usize) -> &[Field];

    /// The user-defined functions the statement can call.
    fn functions(&self) -> Vec<crate::compile::FunctionSig> {
        Vec::new()
    }

    /// The connection's user, for `USER` / `USER()`: the login user, or the
    /// OS user of a free-table connection. `None` leaves `USER` a column.
    fn user(&self) -> Option<String> {
        None
    }

    /// Calls `f` on each record of the table (once for `system.iota`).
    fn scan(
        &self,
        id: usize,
        f: &mut dyn FnMut(&mut dyn Row) -> Result<(), SqlError>,
    ) -> Result<(), SqlError>;

    /// The index orders the table's records can be looked up in: those
    /// whose key is one field's value, of every record.
    fn indexes(&self, _id: usize) -> Vec<TableIndex> {
        Vec::new()
    }

    /// How many records the table holds (deleted ones not counted), where
    /// that is known.
    fn record_count(&self, _id: usize) -> Option<usize> {
        None
    }

    /// What expressions over the table's records need to know of it.
    fn info(&self, _id: usize) -> crate::compile::TableInfo {
        crate::compile::TableInfo::default()
    }

    /// The full-text rules of each of the table's fields, by its place: `Some`
    /// for a field with a full-text index, `None` otherwise. Empty for a table
    /// with no full-text index at all.
    fn fts(&self, _id: usize) -> Vec<Option<crate::fts::Rules>> {
        Vec::new()
    }

    /// What the table's full-text order on the field at place `column` (any
    /// full-text order for `None`, a `CONTAINS(*, …)`) finds for `query`, as a
    /// plan reports it; `None` for a field with no full-text order.
    fn fts_lookup(
        &self,
        _id: usize,
        _column: Option<usize>,
        _query: &crate::fts::Query,
    ) -> Result<Option<FtsLookup>, SqlError> {
        Ok(None)
    }

    /// The numbers of the records whose field in the order `index` (by its
    /// place in [`Tables::indexes`]) is from `low` to `high`, each included,
    /// as SQL compares them; without a bound, from the smallest or to the
    /// largest. In the order's order; there may be more (a bound the field
    /// cannot hold is left out).
    fn lookup(
        &self,
        _id: usize,
        _index: usize,
        _low: Option<&V>,
        _high: Option<&V>,
    ) -> Result<Vec<u32>, SqlError> {
        Ok(Vec::new())
    }

    /// Calls `f` on each of the `records` (numbers [`Tables::lookup`] gave)
    /// the table has, in that order, with its number.
    fn fetch(
        &self,
        _id: usize,
        _records: &[u32],
        _f: &mut dyn FnMut(u32, &mut dyn Row) -> Result<(), SqlError>,
    ) -> Result<(), SqlError> {
        Ok(())
    }
}

/// What a full-text order finds for a `CONTAINS` condition: its name, how far
/// it answers the condition, and the records its keys hold it for (deleted
/// records' included; a superset of those it holds for where the order only
/// narrows them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtsLookup {
    pub order: String,
    pub level: crate::fts::IndexLevel,
    pub records: Vec<u32>,
}

/// An index order a table's records can be looked up in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableIndex {
    /// The order's name.
    pub name: String,
    /// Its key expression, with the field's name.
    pub expression: String,
    /// The field whose value is the key (the first of several), by its place
    /// in the table.
    pub field: usize,
    /// Every field of its key, by its place in the table: those of an order
    /// on several fields, the first of which a restriction looks records up
    /// by.
    pub fields: Vec<usize>,
    /// Whether each of them is CICHARACTER text.
    pub ci: Vec<bool>,
    /// Whether its text keys sort by a collation, not as SQL compares text
    /// (by their bytes).
    pub collated: bool,
}

/// A planned statement: its result's columns and its plan.
#[derive(Debug)]
pub struct Prepared {
    pub columns: Vec<Column>,
    /// The temporary index the static cursor has if it has two rows or more
    /// (see [`Query::temp_index`]).
    pub order_index: Option<crate::order_index::TempIndex>,
    pub(crate) op: Op,
    /// The subqueries' plans, by number.
    pub(crate) subqueries: Vec<SubPlan>,
    /// Each parameter's type.
    pub params: Vec<Ty>,
    /// Whether it reads the rows another statement changes.
    pub(crate) changing: bool,
}

/// A planned `SELECT`.
#[derive(Debug)]
pub struct Query {
    pub(crate) from: Source,
    pub(crate) filter: Option<Node>,
    /// Whether the rows are grouped (by `GROUP BY`, or into one group by
    /// aggregates alone).
    pub(crate) grouped: bool,
    pub(crate) group: Vec<Node>,
    pub(crate) having: Option<Node>,
    pub(crate) aggregates: Vec<AggSpec>,
    pub columns: Vec<Column>,
    pub(crate) distinct: bool,
    pub(crate) top: Option<Top>,
    /// `UNION`ed `SELECT`s, and whether with `ALL`.
    pub(crate) unions: Vec<(bool, Query)>,
    /// `ORDER BY`: over the row, or over the result's columns with
    /// `UNION`s.
    pub(crate) order: Vec<SortKey>,
    /// Whether the rows can stay as they are and be shown sorted.
    pub(crate) shown_sorted: bool,
    /// The column the rows are read ordered by (see [`crate::like_order`]).
    pub(crate) like_order: Option<Node>,
    /// Whether a join of two tables may read the smaller one first (set for
    /// a statement's own `SELECT`).
    pub(crate) swap_joins: bool,
    /// How many levels out the query reads rows: 0 if it can be run once.
    pub(crate) depth: usize,
    /// Each column of the row as plans show it.
    pub(crate) labels: Vec<String>,
}

impl Source {
    /// Whether a table of the source has an order on the one field that is
    /// column `column` of the row.
    fn has_order_on(&self, column: usize) -> bool {
        match self {
            Source::Table { start, indexes, .. } => indexes
                .iter()
                .any(|index| start + index.field == column && !index.expression.contains(';')),
            Source::Join { left, right, .. } => {
                left.has_order_on(column) || right.has_order_on(column)
            }
            Source::Derived { .. } => false,
        }
    }
}

/// Where a `SELECT`'s rows come from; each has its columns at a place in
/// the row.
#[derive(Debug)]
pub(crate) enum Source {
    /// A table, with its name and alias as plans show them.
    Table {
        id: usize,
        start: usize,
        width: usize,
        name: String,
        alias: String,
        /// The orders its records can be looked up in.
        indexes: Vec<TableIndex>,
        /// How many records it holds, if that is known.
        records: Option<usize>,
        /// Whether it is one of the dictionary's `system.` tables, or the
        /// rows of a procedure, whose records keep their order under a
        /// `LIKE` (see [`crate::like_order`]).
        unordered: bool,
    },
    Derived {
        query: Box<Query>,
        start: usize,
        width: usize,
        alias: String,
    },
    Join {
        kind: JoinKind,
        left: Box<Source>,
        right: Box<Source>,
        on: Option<Node>,
    },
}

impl Source {
    pub(crate) fn range(&self) -> (usize, usize) {
        match self {
            Source::Table { start, width, .. } | Source::Derived { start, width, .. } => {
                (*start, start + width)
            }
            Source::Join { left, right, .. } => (left.range().0, right.range().1),
        }
    }
}

/// Plans `select`, whose statement has `params` parameters, over the
/// tables `tables` opened.
pub fn prepare(select: &Select, tables: &dyn Tables, params: usize) -> Result<Prepared, SqlError> {
    prepare_as(select, tables, params, false)
}

/// [`prepare`] for the `SELECT` that reads the rows an `UPDATE`, a `DELETE`
/// or a `MERGE` changes, which the original engine plans a little otherwise
/// (tables stay in the order the statement has them).
pub fn prepare_changing(
    select: &Select,
    tables: &dyn Tables,
    params: usize,
) -> Result<Prepared, SqlError> {
    prepare_as(select, tables, params, true)
}

fn prepare_as(
    select: &Select,
    tables: &dyn Tables,
    params: usize,
    changing: bool,
) -> Result<Prepared, SqlError> {
    let mut planner = Planner {
        tables,
        subqueries: Vec::new(),
        params: vec![Ty::Unknown; params],
    };
    let mut query = planner.select(select, &[])?;
    query.swap_joins = !changing;
    let subqueries: Vec<Query> = planner
        .subqueries
        .into_iter()
        .map(|q| q.expect("planned"))
        .collect();
    let mut plans: Vec<Option<SubPlan>> = subqueries.iter().map(|_| None).collect();
    let op = ops::lower(&query, None, &subqueries, &mut plans);
    let subqueries = plans.into_iter().map(|p| p.expect("lowered")).collect();
    Ok(Prepared {
        order_index: query.temp_index(),
        columns: query.columns,
        op,
        subqueries,
        params: planner.params,
        changing,
    })
}

pub(crate) struct Planner<'t> {
    pub(crate) tables: &'t dyn Tables,
    pub(crate) subqueries: Vec<Option<Query>>,
    pub(crate) params: Vec<Ty>,
}

/// A table's name as plans show it: without a path, an extension or
/// `system.`.
pub fn plan_name(name: &str) -> String {
    match name.get(..7) {
        Some(prefix) if prefix.eq_ignore_ascii_case("system.") => name[7..].to_owned(),
        _ => short_name(name).to_owned(),
    }
}

/// A table's name as expressions qualify columns with it: without a path
/// or extension.
pub(crate) fn short_name(name: &str) -> &str {
    let name = name.rsplit(['\\', '/']).next().unwrap_or(name);
    if name.eq_ignore_ascii_case("system.iota") {
        return name;
    }
    name.split('.').next().unwrap_or(name)
}

/// Structural equality of nodes, regardless of where they are written.
pub(crate) fn same(a: &Node, b: &Node) -> bool {
    format!("{:?}", strip(a)) == format!("{:?}", strip(b))
}

fn strip(node: &Node) -> String {
    without_positions(&format!("{node:?}"))
}

/// A node's `Debug` text without the positions in it.
pub(crate) fn without_positions(text: &str) -> String {
    // Positions are the only numbers after `pos: `.
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("pos: ") {
        out.push_str(&rest[..i + 5]);
        rest = rest[i + 5..].trim_start_matches(|c: char| c.is_ascii_digit());
    }
    out.push_str(rest);
    out
}

impl<'t> Planner<'t> {
    pub(crate) fn compiler<'a>(
        &self,
        tables: Vec<Scope<'a>>,
        outer: &[Vec<Scope<'a>>],
    ) -> Compiler<'a> {
        let mut compiler = Compiler::over(tables, self.params.len());
        compiler.params.clone_from(&self.params);
        compiler.outer = outer.to_vec();
        compiler.functions = self.tables.functions();
        compiler.user = self.tables.user();
        compiler
    }

    pub(crate) fn take_params(&mut self, compiler: &Compiler<'_>) {
        for (mine, theirs) in self.params.iter_mut().zip(&compiler.params) {
            if *mine == Ty::Unknown {
                *mine = *theirs;
            }
        }
    }

    /// Plans the subqueries in `expr` for `compiler`, whose row is the
    /// outer one of theirs.
    pub(crate) fn subqueries<'a>(
        &mut self,
        expr: &'a ast::Expr,
        compiler: &mut Compiler<'a>,
    ) -> Result<(), SqlError>
    where
        't: 'a,
    {
        let mut selects = Vec::new();
        expr.walk(&mut |e| match &e.kind {
            ExprKind::Subquery(s) | ExprKind::Exists(s) => selects.push(&**s),
            ExprKind::InSelect { select, .. } | ExprKind::Quantified { select, .. } => {
                selects.push(&**select)
            }
            _ => {}
        });
        for select in selects {
            let mut outer = compiler.outer.clone();
            outer.push(compiler.tables().to_vec());
            self.take_params(compiler);
            let query = self.select(select, &outer)?;
            let columns = query.columns.iter().map(|c| c.ty).collect();
            let id = self.subqueries.len();
            self.subqueries.push(Some(query));
            compiler
                .subqueries
                .insert(select as *const Select, SubInfo { id, columns });
            compiler.params.clone_from(&self.params);
        }
        Ok(())
    }

    /// Compiles `expr` in `context` after planning its subqueries.
    pub(crate) fn compile<'a>(
        &mut self,
        compiler: &mut Compiler<'a>,
        expr: &'a ast::Expr,
        context: AggContext,
        condition: bool,
    ) -> Result<Node, SqlError>
    where
        't: 'a,
    {
        self.subqueries(expr, compiler)?;
        compiler.agg_context = context;
        let node = if condition {
            compiler.condition(expr)
        } else {
            compiler.compile(expr)
        };
        self.take_params(compiler);
        node
    }

    /// The tables of `from`, added to `scopes` with their columns after
    /// those before.
    fn from<'a>(
        &mut self,
        from: &'a From,
        scopes: &mut Vec<Scope<'a>>,
        outer: &[Vec<Scope<'a>>],
    ) -> Result<Source, SqlError>
    where
        't: 'a,
    {
        let start: usize = scopes.iter().map(|s| s.fields.len()).sum();
        let add = |scopes: &mut Vec<Scope<'a>>, scope: Scope<'a>, pos| {
            let name = scope.alias.unwrap_or(scope.name);
            if !name.is_empty() && Compiler::table_index(scopes, name).is_some() {
                return Err(SqlError::alias_twice(name, pos));
            }
            scopes.push(scope);
            Ok(())
        };
        Ok(match from {
            From::Table(table) => {
                let fields = self.tables.fields(table.id);
                let width = fields.len();
                let scope = Scope {
                    name: short_name(&table.name),
                    alias: table.alias.as_deref(),
                    fields: Cow::Borrowed(fields),
                    fts: std::rc::Rc::new(self.tables.fts(table.id)),
                    info: self.tables.info(table.id),
                };
                // Where the alias is, or would be: what follows the name
                // (probed, `fixtures/sql/links.json`).
                add(scopes, scope, table.alias_pos)?;
                let name = plan_name(&table.name);
                let alias = table.alias.clone().unwrap_or_else(|| name.clone());
                let indexes = self.tables.indexes(table.id);
                let unordered = table
                    .name
                    .get(..7)
                    .is_some_and(|p| p.eq_ignore_ascii_case("system."));
                Source::Table {
                    id: table.id,
                    start,
                    width,
                    name,
                    alias,
                    indexes,
                    records: self.tables.record_count(table.id),
                    unordered,
                }
            }
            // The engine ran the procedure and holds its rows as a table.
            From::Procedure {
                execute,
                id,
                alias,
                pos,
            } => {
                let fields = self.tables.fields(*id);
                let width = fields.len();
                let scope = Scope {
                    name: "",
                    alias: alias.as_deref(),
                    fields: Cow::Borrowed(fields),
                    fts: std::rc::Rc::new(Vec::new()),
                    info: crate::compile::TableInfo::default(),
                };
                add(scopes, scope, *pos)?;
                Source::Table {
                    id: *id,
                    start,
                    width,
                    name: execute.name.clone(),
                    alias: alias.clone().unwrap_or_default(),
                    indexes: Vec::new(),
                    records: None,
                    unordered: true,
                }
            }
            From::Derived { select, alias, pos } => {
                let query = self.select(select, outer)?;
                let fields = result_fields(&query.columns);
                let width = fields.len();
                let scope = Scope {
                    name: "",
                    alias: alias.as_deref(),
                    fields: Cow::Owned(fields),
                    fts: std::rc::Rc::new(Vec::new()),
                    info: crate::compile::TableInfo::default(),
                };
                add(scopes, scope, *pos)?;
                let alias = alias.clone().unwrap_or_default();
                Source::Derived {
                    query: Box::new(query),
                    start,
                    width,
                    alias,
                }
            }
            From::Join {
                kind,
                left,
                right,
                on,
            } => {
                // What the original engine refuses on the right of a `RIGHT`
                // or `FULL JOIN`, whatever the tables hold
                // (`tools/ace/sql/literals.txt`).
                if matches!(kind, JoinKind::Right | JoinKind::Full) {
                    match &**right {
                        From::Derived { .. } => return Err(SqlError::subquery_in_outer_join()),
                        From::Table(table)
                            if table.name.to_ascii_lowercase().starts_with("system.") =>
                        {
                            return Err(SqlError::not_supported());
                        }
                        _ => {}
                    }
                }
                let left = self.from(left, scopes, outer)?;
                let right = self.from(right, scopes, outer)?;
                let on = match on {
                    Some(on) => {
                        let mut compiler = self.compiler(scopes.clone(), outer);
                        Some(self.compile(&mut compiler, on, AggContext::Where, true)?)
                    }
                    None => None,
                };
                Source::Join {
                    kind: *kind,
                    left: Box::new(left),
                    right: Box::new(right),
                    on,
                }
            }
        })
    }

    pub(crate) fn select<'a>(
        &mut self,
        select: &'a Select,
        outer: &[Vec<Scope<'a>>],
    ) -> Result<Query, SqlError>
    where
        't: 'a,
    {
        let mut scopes = Vec::new();
        let from = self.from(&select.from, &mut scopes, outer)?;
        // Each column of the row as plans show it: `[alias].[column]`.
        let labels = scopes
            .iter()
            .flat_map(|scope| {
                let table = scope.alias.unwrap_or(scope.name);
                scope
                    .fields
                    .iter()
                    .map(move |f| format!("[{table}].[{}]", f.name))
            })
            .collect();
        let mut compiler = self.compiler(scopes, outer);

        // The `WHERE`'s `CONTAINS` predicates, in order, that `SCORE(n)` in the
        // select list or `ORDER BY` numbers.
        if let Some(filter) = &select.filter {
            filter.walk(&mut |e| {
                if matches!(e.kind, ExprKind::Contains { .. }) {
                    compiler.where_contains.push(e);
                }
            });
        }

        // The select list.
        let mut columns = Vec::new();
        let mut aliases = Vec::new();
        let mut expressions = 0;
        for item in &select.items {
            match item {
                Item::Star { table, pos } => {
                    let tables: Vec<usize> = match table {
                        Some(name) => vec![
                            Compiler::table_index(compiler.tables(), name)
                                .ok_or_else(|| SqlError::table_not_found(name, *pos))?,
                        ],
                        None => (0..compiler.tables().len()).collect(),
                    };
                    for t in tables {
                        let offset = compiler.offset(t);
                        // `system.iota` has one column, a NULL logical.
                        if compiler.tables()[t]
                            .name
                            .eq_ignore_ascii_case("system.iota")
                        {
                            columns.push(Column {
                                name: "iota".to_owned(),
                                node: Node::new(Kind::Const(V::Null), Ty::Bool, *pos),
                                ty: Ty::Bool,
                                field: (FieldType::Logical, 1, 0),
                                alias: None,
                            });
                        }
                        for (i, field) in compiler.tables()[t].fields.iter().enumerate() {
                            let ty = Ty::of_field(field);
                            columns.push(Column {
                                name: field.name.clone(),
                                node: Node::new(Kind::Column(offset + i), ty, *pos),
                                ty,
                                field: result_field(field),
                                alias: None,
                            });
                        }
                    }
                }
                Item::Expr { expr, alias } => {
                    let node = self.compile(&mut compiler, expr, AggContext::Allowed, false)?;
                    let ty = node.ty;
                    let (name, field) = match (&expr.kind, &node.kind) {
                        // Named as the statement writes it.
                        (ExprKind::Column { name, .. }, Kind::Column(i)) => {
                            let field = field_at(compiler.tables(), *i);
                            (name.clone(), result_field(field))
                        }
                        (ExprKind::Column { name, .. }, Kind::RowId(..)) => {
                            (name.clone(), ty.column())
                        }
                        // A column given a collation keeps its name.
                        (ExprKind::Collate { expr, .. }, _) if collated_column(expr).is_some() => {
                            (collated_column(expr).unwrap_or_default(), ty.column())
                        }
                        _ => {
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
                    if let Some(alias) = alias {
                        aliases.push((alias.clone(), columns.len()));
                    }
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
        unique_names(&mut columns)?;
        // The cursor's field for a cast to more decimals than digits cannot
        // be made.
        if let Some(column) = columns.iter().find(|c| {
            matches!(c.node.kind, Kind::Cast(..))
                && matches!(c.ty, Ty::Num { precision, scale } if scale > precision)
        }) {
            return Err(SqlError::ace(
                5012,
                "The field definition specified for the table was not valid.",
                &format!(
                    "Numeric length must be at least 2 greater than number of decimals. Field: \"{}\"",
                    column.name
                ),
            ));
        }

        let filter = match &select.filter {
            Some(expr) => {
                let mut node = self.compile(&mut compiler, expr, AggContext::Where, true)?;
                crate::compile::mark_restrictions(&mut node);
                Some(node)
            }
            None => None,
        };

        let mut group = Vec::new();
        for key in &select.group {
            let node = match key {
                OrderKey::Ordinal(0, _) => return Err(SqlError::ordinal_invalid(0)),
                OrderKey::Ordinal(n, _) => {
                    let index = usize::try_from(*n).ok().filter(|&n| n <= columns.len());
                    let node = columns[index.ok_or_else(SqlError::ordinal_too_large)? - 1]
                        .node
                        .clone();
                    let mut aggregate = false;
                    visit(&node, &mut |n| aggregate |= matches!(n.kind, Kind::Agg(_)));
                    if aggregate {
                        return Err(SqlError::aggregate_not_allowed("GROUP BY"));
                    }
                    node
                }
                OrderKey::Expr(expr) => {
                    let node = match alias_of(&aliases, &columns, expr) {
                        Some(node) => node,
                        None => self.compile(&mut compiler, expr, AggContext::GroupBy, false)?,
                    };
                    if is_constant(&node) {
                        return Err(SqlError::constant_not_allowed("GROUP BY"));
                    }
                    node
                }
            };
            if node.ty.is_memo() {
                return Err(SqlError::cannot_order(&node_name(&node, compiler.tables())));
            }
            group.push(node);
        }

        // `HAVING` may name the select list's expressions by their aliases.
        let having = match &select.having {
            Some(expr) => {
                let aliased = with_aliases(expr, select, &compiler);
                let expr = match &aliased {
                    Some(e) => e,
                    None => expr,
                };
                // The aliased expression lives as long as this call; plan
                // its subqueries from the original.
                self.subqueries(select.having.as_ref().expect("HAVING"), &mut compiler)?;
                compiler.agg_context = AggContext::Allowed;
                let node = compiler.condition(expr);
                self.take_params(&compiler);
                Some(node?)
            }
            None => None,
        };

        // `ORDER BY` of the `UNION` is planned after it.
        let mut order = Vec::new();
        if select.unions.is_empty() {
            for item in &select.order {
                if order_key_ignored(&item.key)? {
                    continue;
                }
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
                        let named = match &expr.kind {
                            ExprKind::Column { table: None, name } => columns
                                .iter()
                                .find(|c| c.name.eq_ignore_ascii_case(name) && c.is_named()),
                            _ => None,
                        };
                        match named {
                            Some(column) => column.node.clone(),
                            None => crate::compile::ordering(|| {
                                self.compile(&mut compiler, expr, AggContext::Allowed, false)
                            })?,
                        }
                    }
                };
                order.push(SortKey {
                    node,
                    descending: item.descending,
                    alias: None,
                });
            }
        }

        for key in &order {
            if key.node.ty.is_memo() {
                return Err(SqlError::cannot_order(&node_name(
                    &key.node,
                    compiler.tables(),
                )));
            }
            let aggregate = key.node.any(&|n| matches!(n.kind, Kind::Agg(_)));
            if aggregate && !columns.iter().any(|c| same(&c.node, &key.node)) {
                return Err(SqlError::cannot_order(
                    "There are aggregate functions in the ORDER BY clause but some sort columns \
                     are not in the result set.",
                ));
            }
        }
        let grouped = !group.is_empty() || !compiler.aggregates.is_empty();
        // A `HAVING` makes the columns need grouping too, without a `GROUP BY`
        // (`fixtures/sql/gram_syntax.json`).
        if grouped || having.is_some() {
            // A subquery that reads the rows around it is no column the
            // groups have.
            for node in columns.iter().map(|c| &c.node) {
                let mut correlated = false;
                visit(node, &mut |n| {
                    if let Kind::Subquery(id) = n.kind
                        && let Some(Some(sub)) = self.subqueries.get(id)
                    {
                        correlated |= sub.depth > 0;
                    }
                });
                if correlated {
                    return Err(SqlError::not_grouped("Subquery"));
                }
            }
            let checked = columns
                .iter()
                .map(|c| &c.node)
                .chain(&having)
                .chain(order.iter().map(|k| &k.node));
            for node in checked {
                check_grouped(node, &group, compiler.tables())?;
            }
        }

        let mut depth = 0;
        let mut nodes: Vec<&Node> = columns.iter().map(|c| &c.node).collect();
        nodes.extend(filter.iter().chain(&having).chain(&group));
        nodes.extend(order.iter().map(|k| &k.node));
        nodes.extend(compiler.aggregates.iter().filter_map(|a| a.arg.as_ref()));
        let mut subqueries = Vec::new();
        for node in &nodes {
            visit(node, &mut |n| match n.kind {
                Kind::Outer(d, _) => depth = depth.max(d),
                Kind::Subquery(id)
                | Kind::Exists(id)
                | Kind::InSelect(_, id, _)
                | Kind::Quantified(_, _, id, _) => subqueries.push(id),
                _ => {}
            });
        }
        source_nodes(&from, &mut |n| {
            visit(n, &mut |n| match n.kind {
                Kind::Outer(d, _) => depth = depth.max(d),
                Kind::Subquery(id)
                | Kind::Exists(id)
                | Kind::InSelect(_, id, _)
                | Kind::Quantified(_, _, id, _) => subqueries.push(id),
                _ => {}
            })
        });
        for id in subqueries {
            if let Some(Some(sub)) = self.subqueries.get(id) {
                depth = depth.max(sub.depth.saturating_sub(1));
            }
        }

        let aggregates = std::mem::take(&mut compiler.aggregates);
        self.take_params(&compiler);
        // The `ORDER BY` names of a `UNION` that are columns of this
        // `SELECT`'s tables, though not of its select list.
        let table_columns: Vec<String> = select
            .order
            .iter()
            .filter_map(|item| match &item.key {
                OrderKey::Expr(ast::Expr {
                    kind: ExprKind::Column { table: None, name },
                    ..
                }) if compiler.column(None, name, 0).is_ok() => Some(name.to_lowercase()),
                _ => None,
            })
            .collect();
        drop(compiler);

        let mut unions = Vec::new();
        for (all, part) in &select.unions {
            let query = self.select(part, outer)?;
            depth = depth.max(query.depth);
            unions.push((*all, query));
        }
        if !unions.is_empty() {
            merge_union(&mut columns, &unions, select)?;
            for item in &select.order {
                if order_key_ignored(&item.key)? {
                    continue;
                }
                let node = match &item.key {
                    OrderKey::Ordinal(0, _) => return Err(SqlError::ordinal_invalid(0)),
                    OrderKey::Ordinal(n, _) => {
                        let index = usize::try_from(*n)
                            .ok()
                            .filter(|&n| n >= 1 && n <= columns.len());
                        let i = index.ok_or_else(SqlError::ordinal_too_large)? - 1;
                        Node::new(Kind::Column(i), columns[i].ty, 0)
                    }
                    // The names are the first `SELECT`'s: its aliases, its
                    // columns as named and its tables
                    // (`tools/ace/sql/literals.txt`).
                    OrderKey::Expr(expr) => match &expr.kind {
                        ExprKind::Column { table, name } => {
                            if let Some(table) = table
                                && !from_names(&select.from)
                                    .iter()
                                    .any(|n| n.eq_ignore_ascii_case(table))
                            {
                                // Reported at the column after the dot.
                                let at = expr.pos + table.chars().count() + 1;
                                return Err(SqlError::table_not_found(table, at));
                            }
                            let underlying = || {
                                select.items.iter().position(|item| {
                                    matches!(item, Item::Expr { expr: ast::Expr {
                                        kind: ExprKind::Column { name: n, .. }, ..
                                    }, .. } if n.eq_ignore_ascii_case(name))
                                })
                            };
                            let star = select.items.iter().any(|i| matches!(i, Item::Star { .. }));
                            let i = columns
                                .iter()
                                .position(|c| c.name.eq_ignore_ascii_case(name) && c.is_named())
                                .or_else(|| if star { None } else { underlying() })
                                .ok_or_else(|| {
                                    match table_columns.contains(&name.to_lowercase()) {
                                        true => SqlError::cannot_order(UNION_ORDER),
                                        false => SqlError::column_not_found(name, expr.pos),
                                    }
                                })?;
                            Node::new(Kind::Column(i), columns[i].ty, expr.pos)
                        }
                        _ => {
                            return Err(SqlError::cannot_order(UNION_ORDER));
                        }
                    },
                };
                order.push(SortKey {
                    node,
                    descending: item.descending,
                    alias: None,
                });
            }
        }

        // The rows stay as they are, shown sorted, if they are sorted on
        // their columns, unless they are grouped in that order anyway.
        let on_columns = order
            .iter()
            .all(|k| !select.unions.is_empty() || columns.iter().any(|c| same(&c.node, &k.node)));
        // Grouped rows sorted on group keys only are stored so.
        let group_order = grouped
            && !order.is_empty()
            && order.iter().all(|k| group.iter().any(|g| same(&k.node, g)));
        let mut shown_sorted = on_columns && !group_order;

        // A `LIKE` alone gives its rows ordered by its column. An `ORDER BY`
        // on that column stores the rows so, with no index; one on other
        // columns of the select list shows these rows in its order; on what
        // is not a column it sorts the rows as the table has them.
        let mut like_order = None;
        // A table read through an order of the `ORDER BY`'s one column has
        // its records sorted, and the rows are stored so, with no index
        // (`fixtures/sql/staticorder.json`).
        let read_in_order = match &order[..] {
            [key] => {
                matches!(key.node.kind, Kind::Column(column) if from.has_order_on(column))
                    && !grouped
                    && select.unions.is_empty()
            }
            _ => false,
        };
        let ordered_source = match &from {
            Source::Table { unordered, .. } => !unordered,
            Source::Derived { .. } => true,
            Source::Join { .. } => false,
        };
        if !grouped && ordered_source {
            like_order = filter.as_ref().and_then(crate::like_order::column);
            if let Some(node) = &like_order {
                if order.is_empty() {
                } else if !unions.is_empty() {
                    like_order = None;
                } else if same(&order[0].node, node) {
                    shown_sorted = false;
                    like_order = None;
                } else if !shown_sorted {
                    like_order = None;
                }
            }
        }
        if read_in_order && like_order.is_none() {
            shown_sorted = false;
        }

        if select.distinct
            && let Some(column) = columns.iter().find(|c| c.ty.is_memo())
        {
            return Err(match column.node.kind {
                Kind::Column(_) => SqlError::cannot_distinct(&column.name),
                _ => SqlError::cannot_distinct_expression(column.node.pos),
            });
        }

        Ok(Query {
            from,
            filter,
            grouped,
            group,
            having,
            aggregates,
            columns,
            distinct: select.distinct,
            top: select.top,
            unions,
            order,
            shown_sorted,
            like_order,
            swap_joins: false,
            depth,
            labels,
        })
    }
}

impl Query {
    /// The temporary index the static cursor of the query has, when its
    /// rows were sorted on its columns (see [`crate::order_index`]).
    fn temp_index(&self) -> Option<crate::order_index::TempIndex> {
        use crate::order_index::{Key, build, field_key};
        // The `TOP` of a `UNION`'s part cuts the part, not the result.
        let cut = self.top.is_some() && self.unions.is_empty();
        if !self.shown_sorted || self.order.is_empty() || cut {
            return None;
        }
        let keys: Option<Vec<Key>> = self
            .order
            .iter()
            .map(|key| {
                // The keys of a `UNION` number its result's columns.
                let column = match (&key.node.kind, self.unions.is_empty()) {
                    (Kind::Column(i), false) => self.columns.get(*i)?,
                    _ => self.columns.iter().find(|c| same(&c.node, &key.node))?,
                };
                let (length, kind) = field_key(column.field.0, column.field.1);
                Some(Key {
                    text: column.name.clone(),
                    descending: key.descending,
                    plain: true,
                    length,
                    kind,
                })
            })
            .collect();
        keys.map(|keys| build(&keys, "DESCEND"))
    }
}

/// The column type, length and decimals of a result column that is
/// `field`: its own, but an AutoInc's values are plain integers.
fn result_field(field: &Field) -> (FieldType, u16, u16) {
    match field.field_type {
        FieldType::AutoInc => (FieldType::Integer, 4, 0),
        // A static cursor is an ADT table: DBF fields take the ADT types,
        // an ADS VarChar a memo, as in the original engine
        // (`fixtures/sql/dbf.json`).
        FieldType::LegacyVarChar => (FieldType::Memo, 9, 0),
        t @ (FieldType::Memo | FieldType::NMemo | FieldType::Binary | FieldType::Image) => {
            (t, 9, 0)
        }
        FieldType::CompactDate => (FieldType::Date, 4, 0),
        // A DBF Numeric's digits take the sign's place; an ADT one keeps a
        // place for it (`fixtures/sql/dbfdict.json`).
        FieldType::Numeric if matches!(field.layout, tflip_storage::field::Layout::Dbf(_)) => {
            (FieldType::Numeric, field.length() + 1, field.decimals())
        }
        // A double shows all 15 decimals, whatever its field's (as in
        // `fixtures/sql/spview.json`).
        FieldType::Double => (FieldType::Double, 8, 15),
        // A DBF numeric takes the ADT numeric of its precision, which keeps a
        // place for the sign (`fixtures/sql/dbf.json`).
        FieldType::Numeric if matches!(field.layout, tflip_storage::field::Layout::Dbf(_)) => {
            Ty::of_field(field).column()
        }
        t => (t, field.length(), field.decimals()),
    }
}

/// The field at `index` of the row of `tables`.
fn field_at<'s>(tables: &'s [Scope<'_>], mut index: usize) -> &'s Field {
    for scope in tables {
        if index < scope.fields.len() {
            return &scope.fields[index];
        }
        index -= scope.fields.len();
    }
    panic!("a column of the row")
}

/// A node's name in errors: its column's.
fn node_name(node: &Node, tables: &[Scope<'_>]) -> String {
    match node.kind {
        Kind::Column(i) => field_at(tables, i).name.clone(),
        _ => "<expression>".to_owned(),
    }
}

/// The names a `FROM` gives its tables: their aliases, or their names.
fn from_names(from: &ast::From) -> Vec<String> {
    match from {
        ast::From::Table(table) => vec![table.alias.clone().unwrap_or_else(|| table.name.clone())],
        ast::From::Derived { alias, .. } | ast::From::Procedure { alias, .. } => {
            alias.iter().cloned().collect()
        }
        ast::From::Join { left, right, .. } => {
            let mut names = from_names(left);
            names.extend(from_names(right));
            names
        }
    }
}

/// The select list's expression an unqualified name in `GROUP BY` names by
/// its alias: `aliases` holds each alias's column.
fn alias_of(aliases: &[(String, usize)], columns: &[Column], expr: &ast::Expr) -> Option<Node> {
    let ExprKind::Column { table: None, name } = &expr.kind else {
        return None;
    };
    let (_, i) = aliases
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))?;
    Some(columns[*i].node.clone())
}

/// `expr` with names of the select list's aliases that are no columns
/// replaced by the aliased expressions; `None` if there are none.
fn with_aliases(expr: &ast::Expr, select: &Select, compiler: &Compiler<'_>) -> Option<ast::Expr> {
    let aliases: Vec<(&String, &ast::Expr)> = select
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Expr {
                expr,
                alias: Some(alias),
            } => Some((alias, expr)),
            _ => None,
        })
        .collect();
    let lookup = |name: &str| {
        aliases
            .iter()
            .find(|(a, _)| a.eq_ignore_ascii_case(name))
            .map(|(_, e)| (*e).clone())
    };
    let mut changed = false;
    let replaced = replace_columns(expr, &mut |table, name| {
        if table.is_some() || compiler.column(None, name, 0).is_ok() {
            return None;
        }
        let found = lookup(name);
        changed |= found.is_some();
        found
    });
    changed.then_some(replaced)
}

/// A copy of `expr` with the columns `f` gives a replacement for replaced.
fn replace_columns(
    expr: &ast::Expr,
    f: &mut dyn FnMut(Option<&str>, &str) -> Option<ast::Expr>,
) -> ast::Expr {
    let mut go = |e: &ast::Expr| Box::new(replace_columns(e, f));
    let kind = match &expr.kind {
        ExprKind::Column { table, name } => match f(table.as_deref(), name) {
            Some(e) => return e,
            None => expr.kind.clone(),
        },
        ExprKind::Neg(a) => ExprKind::Neg(go(a)),
        ExprKind::Plus(a) => ExprKind::Plus(go(a)),
        ExprKind::Not(a) => ExprKind::Not(go(a)),
        ExprKind::Binary(op, a, b) => {
            let a = go(a);
            ExprKind::Binary(*op, a, go(b))
        }
        ExprKind::IsNull { expr, negated } => ExprKind::IsNull {
            expr: go(expr),
            negated: *negated,
        },
        ExprKind::Between {
            expr,
            low,
            high,
            negated,
        } => {
            let (e, l) = (go(expr), go(low));
            ExprKind::Between {
                expr: e,
                low: l,
                high: go(high),
                negated: *negated,
            }
        }
        ExprKind::In {
            expr,
            list,
            negated,
        } => {
            let e = go(expr);
            let list = list.iter().map(|item| *go(item)).collect();
            ExprKind::In {
                expr: e,
                list,
                negated: *negated,
            }
        }
        ExprKind::Func { name, args } => ExprKind::Func {
            name: name.clone(),
            args: args.iter().map(|a| *go(a)).collect(),
        },
        other => other.clone(),
    };
    ast::Expr {
        kind,
        pos: expr.pos,
    }
}

const UNION_ORDER: &str = "The UNION statement cannot be sorted because the ORDER BY columns are \
                           too large or are not in the SELECT list.";

/// Gives repeated column names a suffix: `id`, `id_1`, ...
///
/// A name of more than 128 characters, or one that cannot be made unique in
/// 128, is a field definition the original engine refuses (5012).
fn unique_names(columns: &mut [Column]) -> Result<(), SqlError> {
    let invalid = |detail: &str| {
        SqlError::ace(
            5012,
            "The field definition specified for the table was not valid.",
            detail,
        )
    };
    if let Some(column) = columns.iter().find(|c| c.name.chars().count() > 128) {
        return Err(invalid(&format!("Field name too long: {}", column.name)));
    }
    for i in 1..columns.len() {
        let base = columns[i].name.clone();
        let taken = |name: &str, columns: &[Column]| {
            columns[..i]
                .iter()
                .any(|c| c.name.eq_ignore_ascii_case(name))
        };
        if !taken(&base, columns) {
            continue;
        }
        let mut n = 1;
        while taken(&format!("{base}_{n}"), columns) {
            n += 1;
        }
        let name = format!("{base}_{n}");
        if name.chars().count() > 128 {
            return Err(invalid("Field name too long to generate a unique name."));
        }
        columns[i].name = name;
    }
    Ok(())
}

/// Calls `f` on `node` and every node in it.
fn visit(node: &Node, f: &mut dyn FnMut(&Node)) {
    f(node);
    for child in children_of(node) {
        visit(child, f);
    }
}

/// A node's direct children.
pub(crate) fn children_of(node: &Node) -> Vec<&Node> {
    match &node.kind {
        Kind::Const(_)
        | Kind::Column(_)
        | Kind::RecNo(_)
        | Kind::Deleted(_)
        | Kind::RowId(..)
        | Kind::Param(_)
        | Kind::Outer(..)
        | Kind::Agg(_)
        | Kind::Subquery(_)
        | Kind::Exists(_) => vec![],
        Kind::Neg(a)
        | Kind::BitNot(a)
        | Kind::Not(a)
        | Kind::IsNull(a, _)
        | Kind::Cast(a, _)
        | Kind::InSelect(a, ..)
        | Kind::Quantified(_, a, ..) => vec![a],
        Kind::Binary(_, a, b) => vec![a, b],
        Kind::Between(a, b, c, _) => vec![a, b, c],
        Kind::In(a, list, _) => std::iter::once(&**a).chain(list).collect(),
        Kind::Like(a, b, c, _) => [Some(&**a), Some(&**b), c.as_deref()]
            .into_iter()
            .flatten()
            .collect(),
        Kind::Case(a, whens, c) => {
            let mut v: Vec<&Node> = a.as_deref().into_iter().collect();
            for (w, t) in whens {
                v.push(w);
                v.push(t);
            }
            v.extend(c.as_deref());
            v
        }
        Kind::Func(_, args) | Kind::UserFunc(_, args) => args.iter().collect(),
        Kind::Trim(_, a, b) => a
            .as_deref()
            .into_iter()
            .chain(std::iter::once(&**b))
            .collect(),
        Kind::Contains(s) | Kind::Score(s, _) => {
            let mut v: Vec<&Node> = Vec::new();
            match &s.source {
                crate::compile::FtsSource::Expr(n) => v.push(n),
                crate::compile::FtsSource::Fields(fields) => {
                    v.extend(fields.iter().map(|(n, _)| n));
                }
            }
            if let crate::compile::FtsCondition::Dynamic(n) = &s.condition {
                v.push(n);
            }
            v
        }
    }
}

/// Calls `f` on the join conditions of `source`.
fn source_nodes(source: &Source, f: &mut dyn FnMut(&Node)) {
    match source {
        Source::Join {
            left, right, on, ..
        } => {
            source_nodes(left, f);
            source_nodes(right, f);
            if let Some(on) = on {
                f(on);
            }
        }
        Source::Table { .. } | Source::Derived { .. } => {}
    }
}

/// Whether a node's value is the same in every row.
fn is_constant(node: &Node) -> bool {
    match &node.kind {
        Kind::Const(_) => true,
        Kind::Func(_, args) if args.is_empty() => false,
        Kind::Column(_)
        | Kind::RecNo(_)
        | Kind::Deleted(_)
        | Kind::RowId(..)
        | Kind::Param(_)
        | Kind::Outer(..)
        | Kind::Agg(_)
        | Kind::Subquery(_)
        | Kind::Exists(_) => false,
        _ => children_of(node).into_iter().all(is_constant),
    }
}

/// Whether an `ORDER BY` key is a constant the original leaves out of the
/// sort (a number with decimals, text); a negative whole number is a bad
/// ordinal, as 0 is.
pub(crate) fn order_key_ignored(key: &OrderKey) -> Result<bool, SqlError> {
    let OrderKey::Expr(expr) = key else {
        return Ok(false);
    };
    Ok(match &expr.kind {
        ExprKind::Number(n) => {
            if n.bytes().all(|b| b.is_ascii_digit()) {
                return Ok(false);
            }
            if let Some(digits) = n.strip_prefix('-')
                && !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(SqlError::ordinal_invalid(n));
            }
            true
        }
        ExprKind::Str(_) => true,
        ExprKind::Neg(inner) => match &inner.kind {
            ExprKind::Number(n) if n.bytes().all(|b| b.is_ascii_digit()) => {
                return Err(SqlError::ordinal_invalid(format!("-{n}")));
            }
            _ => false,
        },
        _ => false,
    })
}

/// 2196 for a column outside aggregates that is not a `GROUP BY` key (or
/// in one).
fn check_grouped(node: &Node, group: &[Node], tables: &[Scope<'_>]) -> Result<(), SqlError> {
    // A key's collation does not make it another column.
    let uncollated = |g: &Node| match &g.kind {
        Kind::Cast(inner, "COLLATE") => Some((**inner).clone()),
        _ => None,
    };
    if group
        .iter()
        .any(|g| same(g, node) || uncollated(g).is_some_and(|u| same(&u, node)))
    {
        return Ok(());
    }
    match &node.kind {
        Kind::Agg(_) => Ok(()),
        Kind::Column(i) => Err(SqlError::not_grouped(&field_at(tables, *i).name)),
        _ => children_of(node)
            .into_iter()
            .try_for_each(|c| check_grouped(c, group, tables)),
    }
}

/// The fields of a result's columns, for a `SELECT` in `FROM`.
pub(crate) fn result_fields(columns: &[Column]) -> Vec<Field> {
    let mut offset = 5u16;
    columns
        .iter()
        .map(|c| {
            let (field_type, length, decimals) = c.field;
            let field = tflip_storage::adt::field(&c.name, field_type, offset, length, decimals);
            offset = offset.saturating_add(field.size);
            field
        })
        .collect()
}

/// The types of a `UNION`'s columns: the first `SELECT`'s, widened to
/// hold the others' values.
fn merge_union(
    columns: &mut [Column],
    unions: &[(bool, Query)],
    select: &Select,
) -> Result<(), SqlError> {
    for (i, (all, part)) in unions.iter().enumerate() {
        if part.columns.len() != columns.len() {
            return Err(SqlError::union_columns());
        }
        let items = &select.unions[i].1.items;
        for (j, (column, other)) in columns.iter_mut().zip(&part.columns).enumerate() {
            let pos = match items.get(j) {
                Some(Item::Expr { expr, .. }) => expr.pos,
                Some(Item::Star { pos, .. }) => *pos,
                None => 0,
            };
            let merged =
                union_type(column.ty, other.ty).ok_or_else(|| SqlError::union_type(pos))?;
            if merged != column.ty {
                column.ty = merged;
                column.field = merged.column();
            }
            if !all && column.ty.is_memo() {
                return Err(SqlError::cannot_distinct(&column.name));
            }
        }
    }
    if unions.iter().any(|(all, _)| !all)
        && let Some(column) = columns.iter().find(|c| c.ty.is_memo())
    {
        return Err(SqlError::cannot_distinct(&column.name));
    }
    Ok(())
}

/// The type of a `UNION`'s column of type `a` and `b`.
fn union_type(a: Ty, b: Ty) -> Option<Ty> {
    let text = |f: FieldType| {
        matches!(
            f,
            FieldType::Character | FieldType::CiCharacter | FieldType::VarChar
        )
    };
    Some(match (a, b) {
        (a, b) if a == b => a,
        (Ty::Unknown, t) | (t, Ty::Unknown) => t,
        (
            Ty::Str {
                field: fa,
                length: la,
            },
            Ty::Str {
                field: fb,
                length: lb,
            },
        ) => {
            let memo = |f| matches!(f, FieldType::Memo | FieldType::NMemo);
            let field = if memo(fa) || memo(fb) {
                FieldType::Memo
            } else if fa == FieldType::VarChar || fb == FieldType::VarChar {
                FieldType::VarChar
            } else if text(fa) {
                fa
            } else {
                fb
            };
            Ty::Str {
                field,
                length: la.max(lb),
            }
        }
        // A logical takes an integer's place (`fixtures/sql/claims.json`).
        (Ty::Bool, t @ Ty::Int { .. }) | (t @ Ty::Int { .. }, Ty::Bool) => t,
        (Ty::Double, b) | (b, Ty::Double) if b.is_numeric() => Ty::Double,
        (Ty::Money, Ty::CurDouble) | (Ty::CurDouble, Ty::Money) => Ty::Double,
        (Ty::Money, b) | (b, Ty::Money) if b.is_numeric() => Ty::Money,
        (Ty::CurDouble, b) | (b, Ty::CurDouble) if b.is_numeric() => Ty::CurDouble,
        (
            Ty::Int {
                width: wa,
                digits: da,
            },
            Ty::Int {
                width: wb,
                digits: db,
            },
        ) => Ty::Int {
            width: wa.max(wb),
            digits: da.max(db),
        },
        // After an integer column, the integer's digits; else as many
        // as both need.
        (Ty::Int { digits, .. }, b) if b.is_numeric() => {
            let (precision, scale) = b.exact()?;
            Ty::Num {
                precision: digits.max(precision),
                scale,
            }
        }
        (a, b) if a.is_numeric() && b.is_numeric() => {
            let (pa, sa) = a.exact()?;
            let (pb, sb) = b.exact()?;
            let scale = sa.max(sb);
            Ty::Num {
                precision: (pa - sa).max(pb - sb) + scale,
                scale,
            }
        }
        (Ty::Bin { field, length: la }, Ty::Bin { length: lb, .. }) => Ty::Bin {
            field,
            length: la.max(lb),
        },
        _ => return None,
    })
}

/// A statement's results: rows of values, in the columns' types, and the
/// order to show them in.
#[derive(Debug, Default)]
pub struct Output {
    pub rows: Vec<Vec<V>>,
    /// Each shown row's 1-based number in `rows`, if they are shown sorted.
    pub sequence: Option<Vec<u32>>,
}

impl Prepared {
    /// Runs the statement.
    pub fn run(&self, tables: &dyn Tables, env: &Env) -> Result<Output, SqlError> {
        Exec::new(tables, env, &self.subqueries).output(&self.op)
    }
}

#[cfg(test)]
mod tests {
    use tflip_storage::adt;
    use tflip_storage::value::Value;

    use super::*;
    use crate::ast::Statement;

    /// Tables in memory: `a (id INTEGER, name CHAR(8))` and `b (a INTEGER,
    /// n INTEGER)`, maybe with an order on `b.a`.
    struct Memory {
        fields: Vec<Vec<Field>>,
        rows: Vec<Vec<Vec<Value>>>,
        /// Each table reference's table.
        refs: Vec<usize>,
        indexed: bool,
        /// What `USER` / `USER()` folds to.
        user: Option<String>,
    }

    struct MemoryRow<'a>(&'a [Value]);

    impl Row for MemoryRow<'_> {
        fn value(&mut self, index: usize) -> Result<Value, SqlError> {
            Ok(self.0[index].clone())
        }
    }

    impl Tables for Memory {
        fn fields(&self, id: usize) -> &[Field] {
            &self.fields[self.refs[id]]
        }

        fn user(&self) -> Option<String> {
            self.user.clone()
        }

        fn scan(
            &self,
            id: usize,
            f: &mut dyn FnMut(&mut dyn Row) -> Result<(), SqlError>,
        ) -> Result<(), SqlError> {
            for row in &self.rows[self.refs[id]] {
                f(&mut MemoryRow(row))?;
            }
            Ok(())
        }

        fn indexes(&self, id: usize) -> Vec<TableIndex> {
            match self.indexed && self.refs[id] == 1 {
                true => vec![TableIndex {
                    fields: vec![0],
                    ci: vec![false],
                    name: "B_A".to_owned(),
                    expression: "a".to_owned(),
                    field: 0,
                    collated: false,
                }],
                false => Vec::new(),
            }
        }

        fn lookup(
            &self,
            id: usize,
            _index: usize,
            low: Option<&V>,
            high: Option<&V>,
        ) -> Result<Vec<u32>, SqlError> {
            let int = |v: Option<&V>| v.map(|v| v.as_f64().unwrap());
            let (low, high) = (int(low), int(high));
            let mut keys: Vec<(i128, u32)> = Vec::new();
            for (i, row) in self.rows[self.refs[id]].iter().enumerate() {
                let Value::Integer(a) = row[0] else { continue };
                let x = a as f64;
                if low.is_none_or(|low| x >= low) && high.is_none_or(|high| x <= high) {
                    keys.push((a, i as u32 + 1));
                }
            }
            keys.sort_unstable();
            Ok(keys.into_iter().map(|(_, record)| record).collect())
        }

        fn fetch(
            &self,
            id: usize,
            records: &[u32],
            f: &mut dyn FnMut(u32, &mut dyn Row) -> Result<(), SqlError>,
        ) -> Result<(), SqlError> {
            for &record in records {
                f(
                    record,
                    &mut MemoryRow(&self.rows[self.refs[id]][record as usize - 1]),
                )?;
            }
            Ok(())
        }
    }

    fn memory(select: &Select, a: Vec<Vec<Value>>, b: Vec<Vec<Value>>) -> Memory {
        let mut refs = Vec::new();
        select.for_each_table(&mut |t| {
            if refs.len() <= t.id {
                refs.resize(t.id + 1, 0);
            }
            refs[t.id] = usize::from(t.name.eq_ignore_ascii_case("b"));
        });
        let field = |name: &str, t, offset, length| adt::field(name, t, offset, length, 0);
        Memory {
            fields: vec![
                vec![
                    field("id", FieldType::Integer, 5, 4),
                    field("name", FieldType::Character, 9, 8),
                ],
                vec![
                    field("a", FieldType::Integer, 5, 4),
                    field("n", FieldType::Integer, 9, 4),
                ],
            ],
            rows: vec![a, b],
            refs,
            indexed: false,
            user: None,
        }
    }

    fn run(sql: &str, a: Vec<Vec<Value>>, b: Vec<Vec<Value>>) -> Vec<Vec<V>> {
        run_on(sql, a, b, false)
    }

    /// `USER` and `USER()` fold to the connection user, a `VARCHAR(len)`, as
    /// the original engine prepares them; a real column named `USER` wins.
    #[test]
    fn user_folds_to_the_connection_user() {
        use tflip_storage::field::FieldType;
        let parse = |sql: &str| {
            let Statement::Select(select) = crate::parse(sql).unwrap().statement else {
                panic!()
            };
            select
        };
        let one = || vec![vec![Value::Integer(1), Value::Text("x       ".to_owned())]];
        for sql in ["SELECT USER() FROM a", "SELECT USER FROM a"] {
            let select = parse(sql);
            let tables = Memory {
                user: Some("alice".to_owned()),
                ..memory(&select, one(), vec![])
            };
            let prepared = prepare(&select, &tables, 0).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
            assert_eq!(
                prepared.columns[0].field,
                (FieldType::VarChar, 5, 0),
                "{sql}"
            );
            let rows = prepared.run(&tables, &Env::default()).unwrap().rows;
            assert_eq!(rows, vec![vec![V::Str("alice".to_owned())]], "{sql}");
        }
        // With no user, bare `USER` is an unknown column.
        let select = parse("SELECT USER FROM a");
        let tables = Memory {
            user: None,
            ..memory(&select, one(), vec![])
        };
        assert!(prepare(&select, &tables, 0).is_err());
    }

    /// SQL `NEWIDSTRING` takes its format as an unquoted word (`NEWIDSTRING(M)`),
    /// each a VARCHAR(38). `MIME` works here (unlike the expression engine); a
    /// quoted string, a number or an unknown word is an invalid argument.
    #[test]
    fn newidstring_sql_formats() {
        use tflip_storage::field::FieldType;
        let parse = |sql: &str| {
            let Statement::Select(select) = crate::parse(sql).unwrap().statement else {
                panic!()
            };
            select
        };
        let one = || vec![vec![Value::Integer(1), Value::Text("x       ".to_owned())]];
        let run1 = |sql: &str| {
            let select = parse(sql);
            let tables = memory(&select, one(), vec![]);
            let prepared = prepare(&select, &tables, 0).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
            assert_eq!(
                prepared.columns[0].field,
                (FieldType::VarChar, 38, 0),
                "{sql} type"
            );
            match &prepared.run(&tables, &Env::default()).unwrap().rows[0][0] {
                V::Str(s) => s.clone(),
                other => panic!("{sql}: {other:?}"),
            }
        };
        // The niladic form and D are the 36-char delimited layout.
        for sql in [
            "SELECT NEWIDSTRING() FROM a",
            "SELECT NEWIDSTRING(D) FROM a",
        ] {
            assert_eq!(run1(sql).len(), 36, "{sql}");
        }
        assert_eq!(run1("SELECT NEWIDSTRING(N) FROM a").len(), 32);
        let b = run1("SELECT NEWIDSTRING(BRACKETED) FROM a");
        assert_eq!((b.len(), &b[..1], &b[37..]), (38, "[", "]"));
        // M and its long name MIME are both the 24-byte MIME base64 here.
        for sql in [
            "SELECT NEWIDSTRING(M) FROM a",
            "SELECT NEWIDSTRING(MIME) FROM a",
        ] {
            assert_eq!(run1(sql).len(), 24, "{sql}");
        }
        assert_eq!(run1("SELECT NEWIDSTRING(F) FROM a").len(), 22);
        // A quoted string, a number, an unknown word and REGISTRY are rejected.
        for sql in [
            "SELECT NEWIDSTRING('M') FROM a",
            "SELECT NEWIDSTRING(0) FROM a",
            "SELECT NEWIDSTRING(X) FROM a",
            "SELECT NEWIDSTRING(REGISTRY) FROM a",
        ] {
            let select = parse(sql);
            let tables = memory(&select, one(), vec![]);
            assert!(prepare(&select, &tables, 0).is_err(), "{sql} should fail");
        }
    }

    /// The rows of `sql`, with an order on `b.a` if `indexed`.
    fn run_on(sql: &str, a: Vec<Vec<Value>>, b: Vec<Vec<Value>>, indexed: bool) -> Vec<Vec<V>> {
        let parsed = crate::parse(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
        let Statement::Select(select) = parsed.statement else {
            panic!()
        };
        let tables = Memory {
            indexed,
            ..memory(&select, a, b)
        };
        let prepared = prepare(&select, &tables, 0).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
        let output = prepared.run(&tables, &Env::default()).unwrap();
        match output.sequence {
            Some(sequence) => sequence
                .iter()
                .map(|&i| output.rows[i as usize - 1].clone())
                .collect(),
            None => output.rows,
        }
    }

    fn ints(rows: &[(i64, i64)]) -> Vec<Vec<Value>> {
        rows.iter()
            .map(|&(x, y)| vec![Value::Integer(i128::from(x)), Value::Integer(i128::from(y))])
            .collect()
    }

    fn named(rows: &[(i64, &str)]) -> Vec<Vec<Value>> {
        rows.iter()
            .map(|&(x, s)| {
                vec![
                    Value::Integer(i128::from(x)),
                    Value::Text(format!("{s:<8}")),
                ]
            })
            .collect()
    }

    /// Joins on equal columns look rows up, and give what comparing every
    /// pair gives, in the same order.
    #[test]
    fn keyed_joins_match_nested_loops() {
        let a: Vec<(i64, String)> = (0..600).map(|i| (i % 90, format!("n{i}"))).collect();
        let a: Vec<(i64, &str)> = a.iter().map(|(i, s)| (*i, s.as_str())).collect();
        let b: Vec<(i64, i64)> = (0..600).map(|i| ((i * 7) % 97, i)).collect();
        for (keyed, plain) in [
            (
                "SELECT x.name, y.n FROM a x JOIN b y ON x.id = y.a",
                "SELECT x.name, y.n FROM a x JOIN b y ON x.id + 0 = y.a",
            ),
            (
                "SELECT x.name, y.n FROM a x LEFT JOIN b y ON y.a = x.id AND y.n > 300",
                "SELECT x.name, y.n FROM a x LEFT JOIN b y ON y.a = x.id + 0 AND y.n > 300",
            ),
            (
                "SELECT x.name, y.n FROM a x, b y WHERE x.id = y.a AND y.n < 50",
                "SELECT x.name, y.n FROM a x, b y WHERE x.id + 0 = y.a AND y.n < 50",
            ),
            (
                "SELECT COUNT(*) FROM a x FULL JOIN b y ON x.id = y.a",
                "SELECT COUNT(*) FROM a x FULL JOIN b y ON x.id = y.a + 0",
            ),
        ] {
            let start = std::time::Instant::now();
            let fast = run(keyed, named(&a), ints(&b));
            let fast_time = start.elapsed();
            let slow = run(plain, named(&a), ints(&b));
            assert_eq!(fast, slow, "{keyed}");
            assert!(!fast.is_empty());
            assert!(
                fast_time < std::time::Duration::from_secs(2),
                "{keyed}: {fast_time:?}"
            );
        }
    }

    /// The original engine's averages of integers: rounded half to even,
    /// and the sum for a negative one.
    #[test]
    fn integer_averages() {
        let avg = |values: &[i64]| {
            let b: Vec<(i64, i64)> = values.iter().map(|&v| (v, 0)).collect();
            run("SELECT AVG(a) FROM b", vec![], ints(&b))[0][0].clone()
        };
        assert_eq!(avg(&[1, 2]), V::Int(2));
        assert_eq!(avg(&[1, 2, 3, 4]), V::Int(2));
        assert_eq!(avg(&[2, 7]), V::Int(4));
        assert_eq!(avg(&[3, 1, -2]), V::Int(1));
        assert_eq!(avg(&[-1, -2, -4]), V::Int(-7));
        assert_eq!(avg(&[]), V::Null);
    }

    /// Restrictions and joins an order serves give what reading every
    /// record gives, in the same order (but for a scope, see below).
    #[test]
    fn index_lookups_match_full_scans() {
        let a: Vec<(i64, String)> = (0..200).map(|i| (i % 40, format!("n{i}"))).collect();
        let a: Vec<(i64, &str)> = a.iter().map(|(i, s)| (*i, s.as_str())).collect();
        let b: Vec<(i64, i64)> = (0..300).map(|i| ((i * 7) % 53, i)).collect();
        for sql in [
            "SELECT x.name, y.n FROM a x JOIN b y ON x.id = y.a",
            "SELECT x.name, y.n FROM a x LEFT JOIN b y ON y.a = x.id AND y.n > 150",
            "SELECT x.name, y.n FROM a x, b y WHERE y.a = x.id AND y.n < 40",
            "SELECT x.name, y.n FROM a x LEFT JOIN b y ON y.n = x.id WHERE y.a > 20",
            "SELECT n FROM b WHERE a > 5 AND n < 100",
            "SELECT n FROM b WHERE a BETWEEN 3 AND 9",
            "SELECT n FROM b WHERE a IN (3, 5, 3)",
            "SELECT n FROM b WHERE a <> 4",
            "SELECT n FROM b WHERE a = 2.5",
            "SELECT n FROM b WHERE a > 2.5 AND a < 4.5",
            "SELECT name FROM a x WHERE EXISTS (SELECT * FROM b y WHERE y.a = x.id)",
            "SELECT name, (SELECT COUNT(*) FROM b y WHERE y.a <= x.id) FROM a x",
        ] {
            let full = run_on(sql, named(&a), ints(&b), false);
            assert_eq!(run_on(sql, named(&a), ints(&b), true), full, "{sql}");
        }
    }

    /// A table whose only restriction is one range of an order (`=`, `>=`,
    /// `<=`) is read in the order's order, as the original engine reads it.
    #[test]
    fn scopes_read_in_index_order() {
        let b = ints(&[(5, 1), (2, 2), (5, 3), (9, 4), (2, 5)]);
        let indexed = |sql: &str| run_on(sql, vec![], b.clone(), true);
        let column = |rows: Vec<Vec<V>>| rows.into_iter().map(|r| r[0].clone()).collect::<Vec<_>>();
        let n = |values: &[i64]| values.iter().map(|&v| V::Int(v)).collect::<Vec<_>>();
        assert_eq!(
            column(indexed("SELECT n FROM b WHERE a >= 2")),
            n(&[2, 5, 1, 3, 4])
        );
        assert_eq!(
            column(indexed("SELECT n FROM b WHERE 5 >= a")),
            n(&[2, 5, 1, 3])
        );
        // Other restrictions keep the records' order.
        assert_eq!(
            column(indexed("SELECT n FROM b WHERE a > 1")),
            n(&[1, 2, 3, 4, 5])
        );
        assert_eq!(
            column(indexed("SELECT n FROM b WHERE a >= 2 AND n > 0")),
            n(&[1, 2, 3, 4, 5])
        );
    }

    /// Plans show which order finds a table's records.
    #[test]
    fn plans_show_index_use() {
        let indexed = |sql: &str| {
            let Statement::Select(select) = crate::parse(sql).unwrap().statement else {
                panic!()
            };
            let tables = Memory {
                indexed: true,
                ..memory(&select, vec![], vec![])
            };
            let prepared = prepare(&select, &tables, 0).unwrap();
            let rows = crate::explain::query(sql, &prepared, None);
            rows.into_iter()
                .skip(1)
                .map(|r| {
                    (
                        r.operator.unwrap_or_default().trim().to_owned(),
                        r.text,
                        r.arguments,
                    )
                })
                .collect::<Vec<_>>()
        };
        let plan = indexed("SELECT n + 0 FROM b WHERE a = 3");
        assert_eq!(plan[1].0, "INDEX SCAN");
        assert_eq!(
            plan[1].1.as_deref(),
            Some("Table Name : b\nTable Alias : b\nTag : B_A\n")
        );
        assert_eq!(
            plan[1].2.as_deref(),
            Some("\nScope Optimization for restriction: <a = 3>")
        );
        let plan = indexed("SELECT n + 0 FROM b WHERE a > 3 AND n = 1");
        assert_eq!(plan[1].0, "AOF SCAN");
        assert_eq!(
            plan[1].2.as_deref(),
            Some(
                "\nAOF<a > 3 AND n = 1> \n====================\nAOF Optimization Details: \n \
                 Index: <none>, Expr: n=1, Estimate: NA, Actual: NA\n Index: B_A, Expr: a>3, \
                 Estimate: NA, Actual: NA\n====================\n"
            )
        );
        let plan = indexed("SELECT x.name FROM a x JOIN b y ON y.a = x.id");
        assert_eq!(plan[2].0, "INDEX SCAN");
        assert_eq!(
            plan[2].1.as_deref(),
            Some("Table Name : b\nTable Alias : y\nIndex Tag : B_A\nIndex Expression : a\n")
        );
    }

    /// The plan of `sql` as `(depth, operator, arguments)`, the statement's
    /// row left out.
    fn plan(sql: &str) -> Vec<(usize, String, String)> {
        let Statement::Select(select) = crate::parse(sql).unwrap().statement else {
            panic!()
        };
        let tables = memory(&select, vec![], vec![]);
        let prepared = prepare(&select, &tables, 0).unwrap();
        let rows = crate::explain::query(sql, &prepared, None);
        let mut depth = std::collections::HashMap::new();
        depth.insert(1, 0);
        rows.iter()
            .skip(1)
            .map(|r| {
                let d = depth[&r.parent] + 1;
                depth.insert(r.node, d);
                let operator = r.operator.clone().unwrap_or_default().trim().to_owned();
                (d, operator, r.arguments.clone().unwrap_or_default())
            })
            .collect()
    }

    fn operators(plan: &[(usize, String, String)]) -> Vec<(usize, &str)> {
        plan.iter().map(|(d, o, _)| (*d, o.as_str())).collect()
    }

    /// The plan shows how the statement runs: a `UNION` after a `UNION ALL`
    /// under it, as they bind to the right.
    #[test]
    fn plans_nest_unions_as_they_run() {
        let plan = plan("SELECT id FROM a UNION ALL SELECT a FROM b UNION SELECT n FROM b");
        assert_eq!(
            operators(&plan),
            [
                (1, "UNION ALL"),
                (2, "TABLE SCAN"),
                (2, "DISTINCT"),
                (3, "UNION ALL"),
                (4, "TABLE SCAN"),
                (4, "TABLE SCAN"),
            ]
        );
    }

    /// Joins on equal columns look rows up through a hash, others compare
    /// every pair; `TOP` is no step (as in the original engine), the select
    /// list's subqueries are, a correlated one running for each row.
    #[test]
    fn plans_show_joins_top_and_subqueries() {
        let hashed = plan("SELECT TOP 1 x.name FROM a x JOIN b y ON x.id = y.a");
        assert_eq!(
            operators(&hashed),
            [(1, "JOIN"), (2, "TABLE SCAN"), (2, "TABLE SCAN")]
        );
        assert_eq!(hashed[0].2, "\nHash Join<[y].[a]=[x].[id]>");
        assert_eq!(hashed[2].2, "\nJoin Condition<[y].[a]=[x].[id]>");

        let looped = plan("SELECT x.name FROM a x JOIN b y ON x.id < y.a");
        assert_eq!(looped[0].2, "");

        let sql = "SELECT name, (SELECT COUNT(*) FROM b WHERE b.a = x.id) FROM a x";
        let rows = {
            let Statement::Select(select) = crate::parse(sql).unwrap().statement else {
                panic!()
            };
            let tables = memory(&select, vec![], vec![]);
            crate::explain::query(sql, &prepare(&select, &tables, 0).unwrap(), None)
        };
        let operators: Vec<&str> = rows
            .iter()
            .skip(1)
            .map(|r| r.operator.as_deref().unwrap_or("").trim())
            .collect();
        // The `COUNT(*)` alone reads no table of its own in a plan.
        assert_eq!(operators, ["TABLE SCAN", "GROUP BY", "EVALUATE"]);
        assert!(
            rows[2]
                .warning
                .as_deref()
                .unwrap_or("")
                .contains("for each row")
        );
    }

    /// `UNION`s bind to the right: `UNION ALL` keeps what a `UNION` after
    /// it made distinct.
    #[test]
    fn unions_bind_to_the_right() {
        let a = named(&[(1, "x"), (2, "y")]);
        let b = ints(&[(2, 0), (3, 0), (3, 0)]);
        let rows = run(
            "SELECT id FROM a UNION ALL SELECT a FROM b UNION SELECT a FROM b",
            a,
            b,
        );
        let ids: Vec<V> = rows.into_iter().map(|r| r[0].clone()).collect();
        assert_eq!(ids, [1, 2, 2, 3].map(V::Int));
    }
}
