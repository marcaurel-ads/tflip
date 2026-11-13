//! `INSERT`, `UPDATE` and `DELETE`: bound to the table they change, their
//! values typed and checked as the original engine checks them
//! (`fixtures/sql/dml.json`). The engine writes the records; [`DmlRun`]
//! evaluates what it writes.
//!
//! What the original engine does:
//!
//! - A value is checked against its column's type: text cannot go into a
//!   number column (2124); text for a date or timestamp is read when the
//!   statement runs (`YYYY-MM-DD`, or the date format; a timestamp needs a
//!   time), and is an error there if it is no date.
//! - A value for an AutoInc column is ignored, in `INSERT` and in `UPDATE`.
//! - Text too long for its column is cut, the row written, and the
//!   statement then fails with 2102; a number too large fails it with 5179.
//! - Aggregates are not allowed.

use std::borrow::Cow;
use std::rc::Rc;

use tflip_storage::field::{Field, FieldType};
use tflip_storage::value::Value;

use crate::ast::{
    Expr, ExprKind, From, InsertSource, JoinKind, Mark, Merge, MergeSet, Named, Select, Statement,
    Update,
};
use crate::compile::{AggContext, Compiler, Node, Scope};
use crate::error::{Pos, SqlError};
use crate::eval::{Env, Row};
use crate::exec::Exec;
use crate::ops::{self, Context, Op, SubPlan, subquery_ids};
use crate::query::{Planner, Query, Tables, short_name};
use crate::types::Ty;
use crate::value::V;

/// A bound `INSERT`, `UPDATE`, `DELETE` or `MERGE`.
#[derive(Debug)]
pub struct Dml {
    pub kind: DmlKind,
    /// The table changed, by its number in the statement.
    pub table: usize,
    /// Its name, as the statement wrote it, without path or extension.
    pub table_name: String,
    /// What `UPDATE` marks the records it writes as.
    pub mark: Mark,
    /// `UPDATE … FROM`: the join that gives its rows.
    pub from: Option<FromParts>,
    /// Which rows change (`UPDATE` and `DELETE`); all without one.
    filter: Option<Node>,
    /// The parts of `filter` the original runs as a filter on the table's
    /// records, evaluating each whole (see [`crate::plan::engine_conjuncts`]).
    engine: Vec<Node>,
    /// The columns given values.
    pub assigns: Vec<Assign>,
    /// `UPDATE`'s new values, a node per assignment.
    sets: Vec<Node>,
    /// `INSERT … VALUES`' rows, a node per assignment each.
    rows: Vec<Vec<Node>>,
    /// `INSERT … SELECT`'s rows.
    source: Option<Op>,
    subqueries: Vec<SubPlan>,
    /// Each parameter's type.
    pub params: Vec<Ty>,
    /// What `MERGE` adds: its source, condition and `INSERT` (the columns
    /// and values of its `UPDATE` are `assigns` and `sets`).
    pub merge: Option<MergeParts>,
    /// Which of `INSERT … VALUES`' values are `DEFAULT`, by row and assignment.
    pub defaults: Vec<Vec<bool>>,
    /// What the plan of an `UPDATE` or `DELETE` shows.
    pub plan: Option<PlanSource>,
}

/// What the plan of an `INSERT`, `UPDATE` or `DELETE` is made of: the
/// `SELECT` that reads the rows it changes or inserts, if it has one, and
/// the step that changes the table `table`, with the `arguments` it shows.
#[derive(Debug)]
pub struct PlanSource {
    pub select: Option<Select>,
    pub table: String,
    pub operator: &'static str,
    pub arguments: Option<String>,
}

/// The `SELECT` of every column of `from`'s rows the condition `filter` holds
/// for.
fn rows_read(from: From, filter: Option<&Expr>) -> Select {
    Select {
        force_static: false,
        into: None,
        top: None,
        distinct: false,
        items: vec![crate::ast::Item::Star {
            table: None,
            pos: 0,
        }],
        from,
        filter: filter.cloned(),
        group: Vec::new(),
        having: None,
        unions: Vec::new(),
        order: Vec::new(),
    }
}

/// How the plan of an `UPDATE` shows what it sets: a line for each column,
/// the labels `labels` those of the expressions' row.
fn sets_shown(target: &str, assigns: &[Assign], sets: &[Node], labels: &[String]) -> String {
    let mut shown = String::from("SET:\n");
    for (assign, node) in assigns.iter().zip(sets) {
        let value = crate::explain::expression(labels, node);
        shown.push_str(&format!("[{target}].[{}] = {value}\n", assign.name));
    }
    shown
}

/// The join an `UPDATE … FROM` updates by.
#[derive(Debug)]
pub struct FromParts {
    /// Its rows: every table's columns in order, then the number of the
    /// record of the table changed.
    source: Op,
    /// Where the table changed has its columns in a row, and how many.
    pub start: usize,
    pub width: usize,
    /// Whether the join reads the table changed again, and so sees the rows
    /// the statement writes.
    pub live: bool,
}

/// A bound `MERGE`'s source and clauses.
#[derive(Debug)]
pub struct MergeParts {
    /// The condition, over the target's row followed by the source's.
    on: Node,
    /// The source's rows; none without `USING`, where the statement runs
    /// once.
    source: Option<Op>,
    /// Whether the source reads the target's table, and so sees the rows
    /// the statement writes.
    pub live_source: bool,
    /// `UPDATE`'s mark; its values are the `Dml`'s `sets`.
    pub update: Option<Mark>,
    pub insert: Option<MergeInsert>,
    /// How `SHOW PLAN` shows the clauses.
    steps: crate::explain::MergeSteps,
}

/// The `INSERT` of a `MERGE`.
#[derive(Debug)]
pub struct MergeInsert {
    /// Whether the record written is a deleted one.
    pub deleted: bool,
    pub assigns: Vec<Assign>,
    /// A row of values for each assignment, over the source's row.
    rows: Vec<Vec<Node>>,
    /// Which of the values are `DEFAULT`, by row and assignment.
    pub defaults: Vec<Vec<bool>>,
    /// The `SELECT`, which reads the source's row.
    select: Option<Op>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmlKind {
    Insert,
    Update,
    Delete,
    Merge,
}

/// A column given a value.
#[derive(Debug, Clone)]
pub struct Assign {
    /// The field, by index.
    pub field: usize,
    pub name: String,
    pub field_type: FieldType,
    ty: Ty,
    /// Where the statement names the column (or the value, when it does
    /// not), for errors reading the value.
    pos: Pos,
}

impl Assign {
    /// Whether the column takes the values given it: an AutoInc column
    /// ignores them.
    pub fn takes(&self) -> bool {
        self.field_type != FieldType::AutoInc
    }
}

/// Binds `statement`, an `INSERT`, `UPDATE`, `DELETE` or `MERGE` with
/// `params` parameters, to the tables `tables` opened.
pub fn prepare(statement: &Statement, tables: &dyn Tables, params: usize) -> Result<Dml, SqlError> {
    if let Statement::Merge(merge) = statement {
        return prepare_merge(merge, tables, params);
    }
    if let Statement::Update(update) = statement
        && update.from.is_some()
    {
        return prepare_update_from(update, tables, params);
    }
    let mut planner = Planner {
        tables,
        subqueries: Vec::new(),
        params: vec![Ty::Unknown; params],
    };
    let (kind, table) = match statement {
        Statement::Insert(insert) => (DmlKind::Insert, &insert.table),
        Statement::Update(update) => (DmlKind::Update, &update.table),
        Statement::Delete(delete) => (DmlKind::Delete, &delete.table),
        _ => return Err(SqlError::unsupported("this statement here", 0)),
    };
    let fields = tables.fields(table.id);
    let name = short_name(&table.name);
    let scope = Scope {
        name,
        alias: None,
        fields: Cow::Borrowed(fields),
        fts: std::rc::Rc::new(tables.fts(table.id)),
        info: tables.info(table.id),
    };
    let mut dml = Dml {
        kind,
        table: table.id,
        table_name: name.to_owned(),
        mark: Mark::Keep,
        from: None,
        filter: None,
        engine: Vec::new(),
        assigns: Vec::new(),
        sets: Vec::new(),
        rows: Vec::new(),
        source: None,
        subqueries: Vec::new(),
        params: Vec::new(),
        merge: None,
        defaults: Vec::new(),
        plan: None,
    };
    let mut nodes: Vec<Node> = Vec::new();
    match statement {
        Statement::Insert(insert) => {
            dml.assigns = insert_columns(insert.columns.as_deref(), &insert.source, fields)?;
            let columns: Vec<String> = dml
                .assigns
                .iter()
                .map(|a| format!("[{name}].[{}]", a.name))
                .collect();
            dml.plan = Some(PlanSource {
                select: match &insert.source {
                    InsertSource::Select(select) => Some((**select).clone()),
                    InsertSource::Values(_) => None,
                },
                table: crate::query::plan_name(&table.name),
                operator: "INSERT INTO",
                arguments: Some(format!("Columns<{}>", columns.join("::"))),
            });
            match &insert.source {
                InsertSource::Values(rows) => {
                    dml.rows = insert_rows(
                        &mut planner,
                        &[],
                        scope.info.oem,
                        insert.columns.is_some(),
                        rows,
                        &dml.assigns,
                    )?;
                    dml.defaults = default_flags(rows);
                    nodes.extend(dml.rows.iter().flatten().cloned());
                }
                InsertSource::Select(select) => {
                    let query = planner.select(select, &[])?;
                    if query.columns.len() != dml.assigns.len() {
                        return Err(SqlError::insert_count());
                    }
                    for (column, assign) in query.columns.iter().zip(&dml.assigns) {
                        check(assign, column.ty, column.node.pos)?;
                    }
                    let subqueries = finished(&mut planner);
                    let mut plans: Vec<Option<SubPlan>> = subqueries.iter().map(|_| None).collect();
                    dml.source = Some(ops::lower(&query, None, &subqueries, &mut plans));
                    dml.subqueries = plans.into_iter().map(|p| p.expect("lowered")).collect();
                    dml.params = planner.params;
                    return Ok(dml);
                }
            }
        }
        Statement::Update(update) => {
            dml.mark = update.mark;
            let mut compiler = planner.compiler(vec![scope.clone()], &[]);
            // The original engine reads the `WHERE` before the new values.
            dml.filter = condition(&mut planner, &mut compiler, update.filter.as_ref())?;
            for set in &update.sets {
                let field = set_column(&mut planner, &scope, fields, set)?;
                let assign = assign(fields, field, set.column.pos);
                planner.subqueries(&set.value, &mut compiler)?;
                compiler.agg_context = AggContext::Statement;
                let node = compiler.node_for(&set.value, assign.ty)?;
                planner.take_params(&compiler);
                check(&assign, node.ty, node.pos)?;
                nodes.push(node.clone());
                dml.sets.push(node);
                dml.assigns.push(assign);
            }
        }
        Statement::Delete(delete) => {
            let mut compiler = planner.compiler(vec![scope.clone()], &[]);
            dml.filter = condition(&mut planner, &mut compiler, delete.filter.as_ref())?;
        }
        _ => {}
    }
    if let Some(filter) = &dml.filter {
        dml.engine = crate::plan::engine_conjuncts(filter, (0, fields.len()))
            .into_iter()
            .cloned()
            .collect();
    }
    nodes.extend(dml.filter.iter().cloned());
    // The subqueries see the table's row around them.
    let ctx = Rc::new(Context {
        labels: fields
            .iter()
            .map(|f| format!("[{name}].[{}]", f.name))
            .collect(),
        aggregates: Vec::new(),
        outer: None,
    });
    let changed = match statement {
        Statement::Update(update) => Some((update.filter.as_ref(), true)),
        Statement::Delete(delete) => Some((delete.filter.as_ref(), false)),
        _ => None,
    };
    if let Some((filter, updating)) = changed {
        dml.plan = Some(PlanSource {
            select: Some(rows_read(From::Table(table.clone()), filter)),
            table: crate::query::plan_name(&table.name),
            operator: if updating { "UPDATE" } else { "DELETE" },
            arguments: updating.then(|| sets_shown(name, &dml.assigns, &dml.sets, &ctx.labels)),
        });
    }
    let mut ids = Vec::new();
    for node in &nodes {
        subquery_ids(node, &mut ids);
    }
    let subqueries = finished(&mut planner);
    let mut plans: Vec<Option<SubPlan>> = subqueries.iter().map(|_| None).collect();
    ops::lower_ids(&ids, &ctx, &subqueries, &mut plans);
    // Subqueries of subqueries are lowered with theirs; the rest were not
    // reached and stay unused.
    dml.subqueries = plans
        .into_iter()
        .map(|p| {
            p.unwrap_or(SubPlan {
                op: Op::Concat { inputs: Vec::new() },
                correlated: false,
            })
        })
        .collect();
    dml.params = planner.params;
    Ok(dml)
}

/// Binds an `UPDATE … FROM`: its values and condition over the rows of the
/// join of its `FROM`, which a `SELECT` of every table's columns and the
/// target's record number reads as the original engine reads it. A `FROM`
/// that does not have the target is joined to it, the target first.
///
/// The original engine reads a `RIGHT JOIN` as an inner one, and a `FULL
/// JOIN` as a left one (`fixtures/sql/updfrom.json`).
fn prepare_update_from<'a>(
    update: &'a Update,
    tables: &'a dyn Tables,
    params: usize,
) -> Result<Dml, SqlError> {
    fn outer_joins_dropped(from: &From) -> From {
        match from {
            From::Join {
                kind,
                left,
                right,
                on,
            } => From::Join {
                kind: match kind {
                    JoinKind::Right => JoinKind::Inner,
                    JoinKind::Full => JoinKind::Left,
                    kind => *kind,
                },
                left: Box::new(outer_joins_dropped(left)),
                right: Box::new(outer_joins_dropped(right)),
                on: on.clone(),
            },
            from => from.clone(),
        }
    }
    let mut planner = Planner {
        tables,
        subqueries: Vec::new(),
        params: vec![Ty::Unknown; params],
    };
    let table = &update.table;
    let fields = tables.fields(table.id);
    let name = short_name(&table.name);
    let from = update.from.as_ref().expect("a FROM");
    let mut reads = 0;
    let mut has_target = false;
    from.for_each_source(&mut |source| {
        if let crate::ast::SourceRef::Table(t) = source {
            has_target |= t.id == table.id;
            reads +=
                usize::from(t.link == table.link && short_name(&t.name).eq_ignore_ascii_case(name));
        }
    });
    let joined = match has_target {
        true => outer_joins_dropped(from),
        false => From::Join {
            kind: JoinKind::Inner,
            left: Box::new(From::Table(table.clone())),
            right: Box::new(outer_joins_dropped(from)),
            on: None,
        },
    };
    // The tables' scopes, in the order of the row, and the target's place.
    fn leaves<'a>(
        from: &'a From,
        planner: &mut Planner<'a>,
        tables: &'a dyn Tables,
        out: &mut Vec<Scope<'a>>,
    ) -> Result<(), SqlError> {
        match from {
            From::Table(t) => out.push(Scope {
                name: short_name(&t.name),
                alias: t.alias.as_deref(),
                fields: Cow::Borrowed(tables.fields(t.id)),
                fts: Rc::new(tables.fts(t.id)),
                info: tables.info(t.id),
            }),
            From::Derived { select, alias, .. } => {
                let query = planner.select(select, &[])?;
                out.push(Scope {
                    name: "",
                    alias: alias.as_deref(),
                    fields: Cow::Owned(crate::query::result_fields(&query.columns)),
                    fts: Rc::new(Vec::new()),
                    info: crate::compile::TableInfo::default(),
                });
            }
            From::Join { left, right, .. } => {
                leaves(left, planner, tables, out)?;
                leaves(right, planner, tables, out)?;
            }
            From::Procedure { .. } => return Err(SqlError::unsupported("this statement here", 0)),
        }
        Ok(())
    }
    let mut scopes = Vec::new();
    leaves(&joined, &mut planner, tables, &mut scopes)?;
    // Where the target's columns start: after those of the tables before it.
    let mut order = Vec::new();
    table_ids(&joined, &mut order);
    let place = order
        .iter()
        .position(|&id| id == Some(table.id))
        .expect("the target is joined");
    let start = scopes[..place].iter().map(|s| s.fields.len()).sum();
    let label = scopes[place].alias.unwrap_or(scopes[place].name).to_owned();
    // The qualifiers of `SET` are the name `UPDATE` was given.
    let named = Scope {
        name: short_name(&update.named),
        alias: None,
        fields: Cow::Borrowed(fields),
        fts: Rc::new(tables.fts(table.id)),
        info: tables.info(table.id),
    };

    // The rows: all columns, and the target's record number.
    let recno = Expr {
        kind: ExprKind::Func {
            name: "RECNO".to_owned(),
            args: vec![Expr {
                kind: ExprKind::Column {
                    table: None,
                    name: label,
                },
                pos: 0,
            }],
        },
        pos: 0,
    };
    let select = Select {
        force_static: false,
        into: None,
        top: None,
        distinct: false,
        items: vec![
            crate::ast::Item::Star {
                table: None,
                pos: 0,
            },
            crate::ast::Item::Expr {
                expr: recno,
                alias: None,
            },
        ],
        from: joined.clone(),
        filter: update.filter.clone(),
        group: Vec::new(),
        having: None,
        unions: Vec::new(),
        order: Vec::new(),
    };
    let query = planner.select(&select, &[])?;

    let mut dml = Dml {
        kind: DmlKind::Update,
        table: table.id,
        table_name: name.to_owned(),
        mark: update.mark,
        from: None,
        filter: None,
        engine: Vec::new(),
        assigns: Vec::new(),
        sets: Vec::new(),
        rows: Vec::new(),
        source: None,
        subqueries: Vec::new(),
        params: Vec::new(),
        merge: None,
        defaults: Vec::new(),
        plan: None,
    };
    let mut compiler = planner.compiler(scopes.clone(), &[]);
    let mut nodes = Vec::new();
    for set in &update.sets {
        let field = set_column(&mut planner, &named, fields, set)?;
        let assign = assign(fields, field, set.column.pos);
        planner.subqueries(&set.value, &mut compiler)?;
        compiler.agg_context = AggContext::Statement;
        let node = compiler.node_for(&set.value, assign.ty)?;
        planner.take_params(&compiler);
        check(&assign, node.ty, node.pos)?;
        nodes.push(node.clone());
        dml.sets.push(node);
        dml.assigns.push(assign);
    }
    let subqueries = finished(&mut planner);
    let mut plans: Vec<Option<SubPlan>> = subqueries.iter().map(|_| None).collect();
    let around = Rc::new(Context {
        labels: scopes
            .iter()
            .flat_map(|scope| {
                let table = scope.alias.unwrap_or(scope.name);
                scope
                    .fields
                    .iter()
                    .map(move |f| format!("[{table}].[{}]", f.name))
            })
            .collect(),
        aggregates: Vec::new(),
        outer: None,
    });
    let source = ops::lower(&query, None, &subqueries, &mut plans);
    let mut ids = Vec::new();
    for node in &nodes {
        subquery_ids(node, &mut ids);
    }
    ops::lower_ids(&ids, &around, &subqueries, &mut plans);
    dml.subqueries = plans
        .into_iter()
        .map(|p| {
            p.unwrap_or(SubPlan {
                op: Op::Concat { inputs: Vec::new() },
                correlated: false,
            })
        })
        .collect();
    dml.from = Some(FromParts {
        source,
        start,
        width: fields.len(),
        live: reads > usize::from(has_target),
    });
    dml.plan = Some(PlanSource {
        select: Some(rows_read(joined.clone(), update.filter.as_ref())),
        table: crate::query::plan_name(&table.name),
        operator: "UPDATE",
        arguments: Some(sets_shown(
            scopes[place].alias.unwrap_or(scopes[place].name),
            &dml.assigns,
            &dml.sets,
            &around.labels,
        )),
    });
    dml.params = planner.params;
    Ok(dml)
}

/// The numbers of the tables a `FROM` joins, in order; none for a derived
/// table.
fn table_ids(from: &From, out: &mut Vec<Option<usize>>) {
    match from {
        From::Table(t) => out.push(Some(t.id)),
        From::Derived { .. } | From::Procedure { .. } => out.push(None),
        From::Join { left, right, .. } => {
            table_ids(left, out);
            table_ids(right, out);
        }
    }
}

/// Binds a `MERGE`: its condition and `UPDATE` over the target's row
/// followed by the source's, its `INSERT` over the source's alone.
fn prepare_merge<'a>(
    merge: &'a Merge,
    tables: &'a dyn Tables,
    params: usize,
) -> Result<Dml, SqlError> {
    let mut planner = Planner {
        tables,
        subqueries: Vec::new(),
        params: vec![Ty::Unknown; params],
    };
    let table = &merge.table;
    let fields = tables.fields(table.id);
    let name = short_name(&table.name);
    let target = Scope {
        name,
        alias: table.alias.as_deref(),
        fields: Cow::Borrowed(fields),
        fts: Rc::new(tables.fts(table.id)),
        info: tables.info(table.id),
    };
    // The source is read by a `SELECT`: a table's by `SELECT *`.
    let star;
    let mut source = None;
    let mut source_scope = None;
    match &merge.source {
        None => {}
        Some(From::Table(t)) => {
            star = Select {
                force_static: false,
                into: None,
                top: None,
                distinct: false,
                items: vec![crate::ast::Item::Star {
                    table: None,
                    pos: t.pos,
                }],
                from: From::Table(t.clone()),
                filter: None,
                group: Vec::new(),
                having: None,
                unions: Vec::new(),
                order: Vec::new(),
            };
            source = Some(planner.select(&star, &[])?);
            source_scope = Some(Scope {
                name: short_name(&t.name),
                alias: t.alias.as_deref(),
                fields: Cow::Borrowed(tables.fields(t.id)),
                fts: Rc::new(tables.fts(t.id)),
                info: tables.info(t.id),
            });
        }
        Some(From::Derived { select, alias, .. }) => {
            let query = planner.select(select, &[])?;
            source_scope = Some(Scope {
                name: "",
                alias: alias.as_deref(),
                fields: Cow::Owned(crate::query::result_fields(&query.columns)),
                fts: Rc::new(Vec::new()),
                info: crate::compile::TableInfo::default(),
            });
            source = Some(query);
        }
        Some(From::Join { .. } | From::Procedure { .. }) => {
            return Err(SqlError::unsupported("this statement here", 0));
        }
    }
    let in_source: Vec<Scope> = source_scope.iter().cloned().collect();
    if let Some(scope) = &source_scope {
        let (a, b) = (
            target.alias.unwrap_or(name),
            scope.alias.unwrap_or(scope.name),
        );
        if a.eq_ignore_ascii_case(b) {
            return Err(SqlError::alias_twice(a, table.alias_pos));
        }
    }
    let mut both = vec![target.clone()];
    both.extend(in_source.iter().cloned());

    let mut compiler = planner.compiler(both.clone(), &[]);
    planner.subqueries(&merge.on, &mut compiler)?;
    compiler.agg_context = AggContext::Statement;
    let on = compiler.compile(&merge.on)?;
    planner.take_params(&compiler);
    if on.ty != Ty::Bool {
        // A qualified column is pointed at after its qualifier.
        let at = match &merge.on.kind {
            ExprKind::Column {
                table: Some(qualifier),
                ..
            } => merge.on.pos + qualifier.chars().count() + 1,
            _ => on.pos,
        };
        return Err(SqlError::not_boolean("=", at));
    }
    let mut nodes = vec![on.clone()];

    let mut dml = Dml {
        kind: DmlKind::Merge,
        table: table.id,
        table_name: name.to_owned(),
        mark: Mark::Keep,
        from: None,
        filter: None,
        engine: Vec::new(),
        assigns: Vec::new(),
        sets: Vec::new(),
        rows: Vec::new(),
        source: None,
        subqueries: Vec::new(),
        params: Vec::new(),
        merge: None,
        defaults: Vec::new(),
        plan: None,
    };
    if let Some(update) = &merge.update {
        for set in &update.sets {
            let named = &set.column;
            let field = set_column(&mut planner, &target, fields, set)?;
            let assign = assign(fields, field, named.pos);
            planner.subqueries(&set.value, &mut compiler)?;
            compiler.agg_context = AggContext::Statement;
            let node = compiler.node_for(&set.value, assign.ty)?;
            planner.take_params(&compiler);
            check(&assign, node.ty, node.pos)?;
            nodes.push(node.clone());
            dml.sets.push(node);
            dml.assigns.push(assign);
        }
    }
    let mut insert_nodes = Vec::new();
    let mut insert = None;
    let mut insert_select = None;
    if let Some(ins) = &merge.insert {
        let assigns = insert_columns(ins.columns.as_deref(), &ins.source, fields)?;
        let mut rows = Vec::new();
        match &ins.source {
            InsertSource::Values(values) => {
                rows = insert_rows(
                    &mut planner,
                    &in_source,
                    target.info.oem,
                    ins.columns.is_some(),
                    values,
                    &assigns,
                )?;
                insert_nodes.extend(rows.iter().flatten().cloned());
            }
            InsertSource::Select(select) => {
                let query = planner.select(select, std::slice::from_ref(&in_source))?;
                if query.columns.len() != assigns.len() {
                    return Err(SqlError::insert_count());
                }
                for (column, assign) in query.columns.iter().zip(&assigns) {
                    check(assign, column.ty, column.node.pos)?;
                }
                insert_select = Some(query);
            }
        }
        let defaults = match &ins.source {
            InsertSource::Values(values) => default_flags(values),
            InsertSource::Select(_) => Vec::new(),
        };
        insert = Some((ins.deleted, assigns, rows, defaults));
    }

    let subqueries = finished(&mut planner);
    let mut plans: Vec<Option<SubPlan>> = subqueries.iter().map(|_| None).collect();
    let labels = |scopes: &[Scope]| -> Vec<String> {
        scopes
            .iter()
            .flat_map(|scope| {
                let table = scope.alias.unwrap_or(scope.name);
                scope
                    .fields
                    .iter()
                    .map(move |f| format!("[{table}].[{}]", f.name))
            })
            .collect()
    };
    let around = |scopes: &[Scope]| {
        Rc::new(Context {
            labels: labels(scopes),
            aggregates: Vec::new(),
            outer: None,
        })
    };
    let (both_ctx, source_ctx) = (around(&both), around(&in_source));
    let source_op = source
        .as_ref()
        .map(|query| ops::lower(query, None, &subqueries, &mut plans));
    let select_op = insert_select
        .as_ref()
        .map(|query| ops::lower(query, Some(Rc::clone(&source_ctx)), &subqueries, &mut plans));
    let mut ids = Vec::new();
    for node in &nodes {
        subquery_ids(node, &mut ids);
    }
    ops::lower_ids(&ids, &both_ctx, &subqueries, &mut plans);
    let mut ids = Vec::new();
    for node in &insert_nodes {
        subquery_ids(node, &mut ids);
    }
    ops::lower_ids(&ids, &source_ctx, &subqueries, &mut plans);
    dml.subqueries = plans
        .into_iter()
        .map(|p| {
            p.unwrap_or(SubPlan {
                op: Op::Concat { inputs: Vec::new() },
                correlated: false,
            })
        })
        .collect();
    let same = |t: &crate::ast::TableRef| {
        t.link == table.link && short_name(&t.name).eq_ignore_ascii_case(name)
    };
    let live_source = match &merge.source {
        Some(From::Table(t)) => same(t),
        Some(From::Derived { select, .. }) => {
            let mut reads = false;
            select.for_each_table(&mut |t| reads |= same(t));
            reads
        }
        _ => false,
    };
    // The steps of the plan: the `UPDATE` under the source's name and the
    // `INSERT` under the target's.
    let target_label = target.alias.unwrap_or(name);
    let source_name = match &merge.source {
        Some(From::Table(t)) => crate::query::plan_name(&t.name),
        Some(From::Derived { alias, .. }) => alias.clone().unwrap_or_default(),
        _ => "iota".to_owned(),
    };
    let update_step = merge.update.as_ref().map(|_| {
        let mut sets = String::from("SET:\n");
        for (assign, node) in dml.assigns.iter().zip(&dml.sets) {
            let value = crate::explain::expression(&both_ctx.labels, node);
            sets.push_str(&format!("[{target_label}].[{}] = {value}\n", assign.name));
        }
        (source_name, sets)
    });
    let insert_step = insert.as_ref().map(|(_, assigns, _, _)| {
        let columns: Vec<String> = assigns
            .iter()
            .map(|a| format!("[{target_label}].[{}]", a.name))
            .collect();
        (
            crate::query::plan_name(&table.name),
            format!("Columns<{}>", columns.join("::")),
        )
    });
    dml.merge = Some(MergeParts {
        steps: crate::explain::MergeSteps {
            update: update_step,
            insert: insert_step,
        },
        on,
        source: source_op,
        live_source,
        update: merge.update.as_ref().map(|update| update.mark),
        insert: insert.map(|(deleted, assigns, rows, defaults)| MergeInsert {
            deleted,
            assigns,
            rows,
            defaults,
            select: select_op,
        }),
    });
    dml.params = planner.params;
    Ok(dml)
}

/// The plan of the `MERGE` `statement` (`text` as written), as `SHOW PLAN`
/// shows it: its source (the one row of `system.iota` without `USING`) joined
/// to its target on its condition, and a step for each clause.
pub fn merge_plan(
    statement: &Statement,
    text: &str,
    tables: &dyn Tables,
    env: &Env,
    params: usize,
) -> Result<Vec<crate::explain::PlanRow>, SqlError> {
    let Statement::Merge(merge) = statement else {
        return Err(SqlError::unsupported("this statement here", 0));
    };
    let dml = prepare_merge(merge, tables, params)?;
    let left = merge.source.clone().unwrap_or_else(|| {
        From::Table(crate::ast::TableRef {
            name: "system.iota".to_owned(),
            link: None,
            alias: None,
            pos: 0,
            alias_pos: 0,
            // No table has this number, which reads as the one empty row.
            id: usize::MAX,
        })
    });
    let join = Select {
        force_static: false,
        into: None,
        top: None,
        distinct: false,
        items: vec![crate::ast::Item::Star {
            table: None,
            pos: 0,
        }],
        // Without a source, the condition holds for the target alone, as a
        // `WHERE` does.
        from: From::Join {
            kind: crate::ast::JoinKind::Inner,
            left: Box::new(left),
            right: Box::new(From::Table(merge.table.clone())),
            on: merge.source.is_some().then(|| merge.on.clone()),
        },
        filter: merge.source.is_none().then(|| merge.on.clone()),
        group: Vec::new(),
        having: None,
        unions: Vec::new(),
        order: Vec::new(),
    };
    let prepared = crate::query::prepare_changing(&join, tables, params)?;
    let parts = dml.merge.as_ref().expect("a MERGE");
    Ok(crate::explain::merge(
        text,
        &prepared,
        Some((tables, env)),
        &parts.steps,
    ))
}

/// The planner's subqueries, all planned.
fn finished(planner: &mut Planner<'_>) -> Vec<Query> {
    std::mem::take(&mut planner.subqueries)
        .into_iter()
        .map(|q| q.expect("planned"))
        .collect()
}

/// A `WHERE` over the table's row.
fn condition<'a>(
    planner: &mut Planner<'a>,
    compiler: &mut Compiler<'a>,
    filter: Option<&'a crate::ast::Expr>,
) -> Result<Option<Node>, SqlError> {
    let Some(filter) = filter else {
        return Ok(None);
    };
    planner.subqueries(filter, compiler)?;
    compiler.agg_context = AggContext::Statement;
    let node = compiler.condition(filter)?;
    planner.take_params(compiler);
    Ok(Some(node))
}

/// The field of the target `target` that `set` gives a value, which may be
/// qualified with the target's name or alias, as the original engine
/// resolves any column's.
fn set_column(
    planner: &mut Planner<'_>,
    target: &Scope<'_>,
    fields: &[Field],
    set: &MergeSet,
) -> Result<usize, SqlError> {
    let named = &set.column;
    match &set.qualifier {
        Some(qualifier) => Ok(planner
            .compiler(vec![target.clone()], &[])
            .column(Some(qualifier), &named.name, named.pos)?
            .0),
        None => fields
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(&named.name))
            .ok_or_else(|| SqlError::column_not_found(&named.name, named.pos)),
    }
}

fn assign(fields: &[Field], field: usize, pos: Pos) -> Assign {
    let f = &fields[field];
    Assign {
        field,
        name: f.name.clone(),
        field_type: f.field_type,
        ty: Ty::of_field(f),
        pos,
    }
}

/// The columns an `INSERT` gives values: those it lists, or all.
fn insert_columns(
    columns: Option<&[Named]>,
    source: &InsertSource,
    fields: &[Field],
) -> Result<Vec<Assign>, SqlError> {
    let Some(columns) = columns else {
        // Without a list, a value's errors point at the value.
        let positions: Vec<Pos> = match source {
            InsertSource::Values(rows) => rows
                .first()
                .map(|r| r.iter().map(|e| e.pos).collect())
                .unwrap_or_default(),
            InsertSource::Select(_) => Vec::new(),
        };
        return Ok((0..fields.len())
            .map(|i| assign(fields, i, positions.get(i).copied().unwrap_or(0)))
            .collect());
    };
    let mut out: Vec<Assign> = Vec::with_capacity(columns.len());
    for named in columns {
        let field = fields
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(&named.name))
            .ok_or_else(|| SqlError::column_not_found(&named.name, named.pos))?;
        if out.iter().any(|a| a.field == field) {
            return Err(SqlError::column_twice(&named.name));
        }
        out.push(assign(fields, field, named.pos));
    }
    Ok(out)
}

/// Which of the values of `rows` are `DEFAULT`.
fn default_flags(rows: &[Vec<Expr>]) -> Vec<Vec<bool>> {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|e| e.kind == crate::ast::ExprKind::Default)
                .collect()
        })
        .collect()
}

/// `INSERT`'s `rows` of values for `assigns`, compiled over the rows
/// `scopes` (none for `INSERT`, the source's for `MERGE`); their text is
/// the OEM table's (`oem`) it goes into.
fn insert_rows<'a>(
    planner: &mut Planner<'a>,
    scopes: &[Scope<'a>],
    oem: bool,
    listed: bool,
    rows: &'a [Vec<Expr>],
    assigns: &[Assign],
) -> Result<Vec<Vec<Node>>, SqlError> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        // Without a column list, the values' types are checked before their
        // number.
        if row.len() != assigns.len() && listed {
            return Err(SqlError::insert_count());
        }
        let mut compiler = planner.compiler(scopes.to_vec(), &[]);
        compiler.oem |= oem;
        let mut values = Vec::with_capacity(row.len());
        for (expr, assign) in row.iter().zip(assigns) {
            planner.subqueries(expr, &mut compiler)?;
            compiler.agg_context = AggContext::Statement;
            let node = compiler.node_for(expr, assign.ty)?;
            planner.take_params(&compiler);
            check(assign, node.ty, node.pos)?;
            values.push(node);
        }
        if row.len() != assigns.len() {
            return Err(SqlError::insert_count());
        }
        out.push(values);
    }
    Ok(out)
}

/// 2124 if a value of type `source`, at `pos`, cannot go into the column.
fn check(assign: &Assign, source: Ty, pos: Pos) -> Result<(), SqlError> {
    let target = assign.ty;
    // The original names the logical's source type an integer, and the
    // column where it is given (`fixtures/sql/claims.json`).
    if target.is_numeric() && source == Ty::Bool {
        return Err(SqlError::bool_assignment(
            type_name(target),
            "Integer",
            &assign.name,
            assign.pos,
        ));
    }
    match assignable(target, source) {
        true => Ok(()),
        false => Err(SqlError::invalid_assignment(
            type_name(target),
            type_name(source),
            &assign.name,
            pos,
        )),
    }
}

/// Whether a value of type `source` can be assigned to a column or
/// variable of type `target`.
pub(crate) fn assignable(target: Ty, source: Ty) -> bool {
    match (target, source) {
        (_, Ty::Unknown) => true,
        (t, s) if t.is_numeric() => s.is_numeric(),
        (Ty::Str { .. }, _) => !matches!(source, Ty::Bin { .. }),
        (Ty::Bin { .. }, s) => matches!(s, Ty::Bin { .. } | Ty::Str { .. }),
        (Ty::Date | Ty::TimeStamp, s) => matches!(s, Ty::Date | Ty::TimeStamp | Ty::Str { .. }),
        (Ty::Time, s) => matches!(s, Ty::Time | Ty::TimeStamp | Ty::Str { .. }),
        (Ty::Bool, s) => s == Ty::Bool || s.is_numeric(),
        (Ty::Guid, s) => matches!(s, Ty::Guid | Ty::Str { .. }),
        _ => false,
    }
}

/// 2124 if a value of type `source`, at `pos`, cannot be the argument of the
/// procedure's parameter `param`, which takes it as a column does.
pub fn check_argument(param: &Field, source: Ty, pos: Pos) -> Result<(), SqlError> {
    check(&assign(std::slice::from_ref(param), 0, pos), source, pos)
}

/// [`check_argument`] for a stored procedure of the dictionary, whose text
/// parameter takes text only, as a system procedure's does
/// (`fixtures/sql/trigproc.json`).
pub fn check_user_argument(param: &Field, source: Ty, pos: Pos) -> Result<(), SqlError> {
    let target = Ty::of_field(param);
    match target {
        Ty::Str { .. } => check_strict_argument(&param.name, target, source, pos),
        _ => check_argument(param, source, pos),
    }
}

/// 2124 if a value of type `source`, at `pos`, cannot be the argument of the
/// system procedure parameter `name` of type `target`: unlike a column, a text
/// parameter takes text only.
pub fn check_strict_argument(name: &str, target: Ty, source: Ty, pos: Pos) -> Result<(), SqlError> {
    let fits = match target {
        Ty::Str { .. } => matches!(source, Ty::Str { .. } | Ty::Unknown),
        _ => assignable(target, source),
    };
    match fits {
        true => Ok(()),
        false => Err(SqlError::invalid_assignment(
            type_name(target),
            type_name(source),
            name,
            pos,
        )),
    }
}

/// A type's name in the original engine's messages.
pub(crate) fn type_name(ty: Ty) -> &'static str {
    match ty {
        Ty::Bool => "Logical",
        Ty::Int {
            width: crate::types::IntWidth::Short,
            ..
        } => "Short",
        Ty::Int {
            width: crate::types::IntWidth::Int,
            ..
        } => "Integer",
        Ty::Int {
            width: crate::types::IntWidth::Long,
            ..
        } => "LongInteger",
        Ty::Num { .. } => "Numeric",
        Ty::Double => "Double",
        Ty::CurDouble => "CurDouble",
        Ty::Money => "Money",
        Ty::Date => "Date",
        Ty::Time => "Time",
        Ty::TimeStamp => "Timestamp",
        Ty::Str { .. } => "Character",
        Ty::Bin { .. } => "Binary",
        Ty::Guid => "GUID",
        Ty::Unknown => "Unknown",
    }
}

/// Runs a [`Dml`]'s expressions on the tables `tables`.
pub struct DmlRun<'a> {
    dml: &'a Dml,
    exec: Exec<'a>,
    env: &'a Env,
}

impl Dml {
    /// What runs the statement's expressions.
    pub fn runner<'a>(&'a self, tables: &'a dyn Tables, env: &'a Env) -> DmlRun<'a> {
        DmlRun {
            dml: self,
            exec: Exec::new(tables, env, &self.subqueries),
            env,
        }
    }
}

impl<'a> DmlRun<'a> {
    /// Whether the `WHERE` holds for a row of `values`.
    /// Which rows change (`UPDATE` and `DELETE`); all without one.
    pub fn filter(&self) -> Option<&Node> {
        self.dml.filter.as_ref()
    }

    /// The values of the statement's parameters.
    pub fn params(&self) -> &[V] {
        &self.env.params
    }

    /// `values` are those of the record number `record`.
    pub fn matches(&self, record: u32, values: &[V]) -> Result<bool, SqlError> {
        for part in &self.dml.engine {
            self.exec.eval_engine_values(part, values, Some(record))?;
        }
        match &self.dml.filter {
            Some(filter) => {
                Ok(self.exec.eval_values(filter, values, Some(record))? == V::Bool(true))
            }
            None => Ok(true),
        }
    }

    /// `UPDATE`'s new values for a row of `values`, an assignment each, and
    /// where each is written in the statement.
    pub fn sets(&self, record: u32, values: &[V]) -> Result<Vec<(V, Pos)>, SqlError> {
        self.dml
            .sets
            .iter()
            .zip(&self.dml.assigns)
            .map(|(node, assign)| {
                let value = match self.cut_param(node, assign) {
                    Some(cut) => cut,
                    None => self.exec.eval_values(node, values, Some(record))?,
                };
                Ok((value, node.pos))
            })
            .collect()
    }

    /// The value of the parameter `node`, if it is one, cut to the decimals
    /// of the NUMERIC column of `assign` that it has more of.
    fn cut_param(&self, node: &Node, assign: &Assign) -> Option<V> {
        let (crate::compile::Kind::Param(i), Ty::Num { scale, .. }) = (&node.kind, assign.ty)
        else {
            return None;
        };
        crate::value::cut_decimals(self.env.params.get(*i)?, scale)
    }

    /// Whether the value written at `pos` is a parameter cut by
    /// [`DmlRun::sets`] or [`DmlRun::rows`]: written, and the statement
    /// fails with 2102 after.
    pub fn is_cut(&self, pos: Pos) -> bool {
        let rows = self
            .dml
            .rows
            .iter()
            .flat_map(|row| row.iter().zip(&self.dml.assigns));
        let sets = self.dml.sets.iter().zip(&self.dml.assigns);
        rows.chain(sets)
            .any(|(node, assign)| node.pos == pos && self.cut_param(node, assign).is_some())
    }

    /// `INSERT`'s rows: a value per assignment each, and where each is
    /// written in the statement.
    pub fn rows(&self) -> Result<Vec<Vec<(V, Pos)>>, SqlError> {
        if let Some(source) = &self.dml.source {
            let output = self.exec.output(source)?;
            let rows = match output.sequence {
                Some(sequence) => sequence
                    .iter()
                    .map(|&i| output.rows[i as usize - 1].clone())
                    .collect(),
                None => output.rows,
            };
            return Ok(rows
                .into_iter()
                .map(|row| row.into_iter().map(|v| (v, 0)).collect())
                .collect());
        }
        let mut out = Vec::with_capacity(self.dml.rows.len());
        for row in &self.dml.rows {
            let mut values = Vec::with_capacity(row.len());
            for (node, assign) in row.iter().zip(&self.dml.assigns) {
                let value = match self.cut_param(node, assign) {
                    Some(cut) => cut,
                    None => self.exec.eval_values(node, &[], None)?,
                };
                values.push((value, node.pos));
            }
            out.push(values);
        }
        Ok(out)
    }

    /// `UPDATE … FROM`'s rows of the join, each a value per column of every
    /// table, then the number of the record of the table changed (0 where
    /// the join has none).
    pub fn from_rows(&self) -> Result<Vec<Vec<V>>, SqlError> {
        let parts = self.dml.from.as_ref().expect("an UPDATE … FROM");
        let output = self.exec.output(&parts.source)?;
        Ok(match output.sequence {
            Some(sequence) => sequence
                .iter()
                .map(|&i| output.rows[i as usize - 1].clone())
                .collect(),
            None => output.rows,
        })
    }

    fn merge_parts(&self) -> &MergeParts {
        self.dml.merge.as_ref().expect("a MERGE")
    }

    /// Forgets the records read, so that the tables are read again for what
    /// the statement wrote. The rows of subqueries that read no row around
    /// them are kept, as the original engine does.
    pub fn refresh(&self) {
        self.exec.forget_records();
    }

    /// `MERGE`'s source rows, each a value per column; one row of none
    /// without `USING`.
    pub fn source_rows(&self) -> Result<Vec<Vec<V>>, SqlError> {
        let Some(op) = &self.merge_parts().source else {
            return Ok(vec![Vec::new()]);
        };
        let output = self.exec.output(op)?;
        Ok(match output.sequence {
            Some(sequence) => sequence
                .iter()
                .map(|&i| output.rows[i as usize - 1].clone())
                .collect(),
            None => output.rows,
        })
    }

    /// Whether `MERGE`'s condition holds for the target row `target` and the
    /// source row `source`.
    pub fn merge_matches(&self, record: u32, target: &[V], source: &[V]) -> Result<bool, SqlError> {
        let values = [target, source].concat();
        let on = &self.merge_parts().on;
        Ok(self.exec.eval_values(on, &values, Some(record))? == V::Bool(true))
    }

    /// `MERGE`'s `UPDATE` values for the target row `target` and the source
    /// row `source`, as [`DmlRun::sets`].
    pub fn merge_sets(
        &self,
        record: u32,
        target: &[V],
        source: &[V],
    ) -> Result<Vec<(V, Pos)>, SqlError> {
        self.sets(record, &[target, source].concat())
    }

    /// `MERGE`'s `INSERT` rows for the source row `source`, as
    /// [`DmlRun::rows`].
    pub fn merge_rows(&self, source: &[V]) -> Result<Vec<Vec<(V, Pos)>>, SqlError> {
        let insert = self.merge_parts().insert.as_ref().expect("an INSERT");
        if let Some(select) = &insert.select {
            let output = self.exec.output_in(select, source)?;
            return Ok(output
                .rows
                .into_iter()
                .map(|row| row.into_iter().map(|v| (v, 0)).collect())
                .collect());
        }
        let mut out = Vec::with_capacity(insert.rows.len());
        for row in &insert.rows {
            let mut values = Vec::with_capacity(row.len());
            for node in row {
                values.push((self.exec.eval_values(node, source, None)?, node.pos));
            }
            out.push(values);
        }
        Ok(out)
    }

    /// `v` as a value of the column of `assign`: dates, times and logicals
    /// read from what was given, which can fail before anything is written.
    ///
    /// A date given for a timestamp column or the reverse is refused at the
    /// value `at`, text that is none at the column.
    pub fn read(&self, assign: &Assign, v: V, at: Pos) -> Result<V, SqlError> {
        let invalid = |what: &str| {
            SqlError::invalid_assignment_value(
                what,
                assign.pos,
                type_name(assign.ty),
                "Character",
                &assign.name,
            )
        };
        let v = match (assign.ty, v) {
            (Ty::Date, V::Str(s)) => {
                V::Date(crate::eval::text_date(&s, self.env).ok_or_else(|| invalid("DATE"))?)
            }
            (Ty::Date, V::TimeStamp(..)) => {
                return Err(SqlError::invalid_assignment(
                    "Date",
                    "Timestamp",
                    &assign.name,
                    at,
                ));
            }
            (Ty::TimeStamp, V::Str(s)) => {
                let (d, t) =
                    crate::compile::timestamp_text(&s, &self.env.date_format, self.env.epoch())
                        .ok_or_else(|| invalid("TIMESTAMP"))?;
                V::TimeStamp(d, t)
            }
            (Ty::TimeStamp, V::Date(_)) => {
                return Err(SqlError::invalid_assignment(
                    "Timestamp",
                    "Date",
                    &assign.name,
                    at,
                ));
            }
            (Ty::Time, V::Str(s)) => {
                V::Time(crate::compile::time_text(&s).ok_or_else(|| invalid("TIME"))?)
            }
            (Ty::Time, V::TimeStamp(..)) => {
                return Err(SqlError::invalid_assignment(
                    "Time",
                    "Timestamp",
                    &assign.name,
                    at,
                ));
            }
            (Ty::Guid, V::Str(s)) => V::Guid(crate::eval::guid_bytes(&s).ok_or_else(|| {
                SqlError::invalid_assignment("Binary", "Character", &assign.name, assign.pos)
            })?),
            (Ty::Bool, v @ (V::Int(_) | V::Num(..) | V::Double(_) | V::Money(_))) => {
                V::Bool(v.as_f64().is_some_and(|x| x != 0.0))
            }
            (_, v) => v,
        };
        Ok(v)
    }

    /// The field value of `v`, read for the column of `assign`: a number
    /// too large for it is an error.
    pub fn field(&self, assign: &Assign, v: &V) -> Result<Value, SqlError> {
        v.to_field(assign.ty, &assign.name)
    }
}

/// A row of a table's fields' values, as expressions over it read them.
pub fn row_values(fields: &[Field], row: &mut dyn Row) -> Result<Vec<V>, SqlError> {
    let mut values = Vec::with_capacity(fields.len());
    for (i, field) in fields.iter().enumerate() {
        values.push(row.column(i, Ty::of_field(field))?);
    }
    Ok(values)
}
