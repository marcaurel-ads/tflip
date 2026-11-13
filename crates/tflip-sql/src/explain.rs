//! `SHOW PLAN FOR`: a statement's plan as rows of the plan cursor ARC's SQL
//! utility draws as a tree.
//!
//! The cursor's columns, its row types, the names of its operators and the
//! wording of their arguments and warnings are the original engine's
//! (`fixtures/sql/plans.json`). The plan is the one tflip runs: a `SELECT`
//! of one table's records shown as a live cursor, else its operators
//! ([`crate::ops`]) node by node, with the indexes its scans and joins look
//! records up in as the original engine shows them. A live cursor's `WHERE`
//! is shown as the AOF the original engine makes of it ([`crate::xbase`]),
//! and its `ORDER BY` with the order whose key it is.

use std::rc::Rc;

use tflip_storage::field::Field;

use crate::ast::{BinOp, JoinKind};
use crate::compile::{AggSpec, FtsCondition, FtsSource, Kind, Node};
use crate::eval::Env;
use crate::exec::Exec;
use crate::ops::{
    Clause, Context, IndexUse, JoinMethod, Op, Purpose, Restriction, SubPlan, subquery_ids,
};
use crate::plan::Plan;
use crate::query::{FtsLookup, Prepared, Tables, children_of};
use crate::value::V;

/// A row of the plan cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRow {
    /// 1-based; the statement's row is 1.
    pub node: i32,
    /// The node this one is part of; 0 for the statement's row.
    pub parent: i32,
    /// `SELECT` or `EXECUTE` for the statement, `PLAN` for its nodes.
    pub row_type: String,
    /// The statement's text, or the table a scan reads.
    pub text: Option<String>,
    pub operator: Option<String>,
    pub arguments: Option<String>,
    /// How often the node runs; `None` for nodes that are not scans.
    pub estimate: Option<i32>,
    pub warning: Option<String>,
}

/// The plan cursor's columns: name, field type, length and decimals.
pub const COLUMNS: [(&str, tflip_storage::field::FieldType, u16, u16); 9] = {
    use tflip_storage::field::FieldType::*;
    [
        ("NodeID", AutoInc, 4, 0),
        ("ParentID", Integer, 4, 0),
        ("RowType", Character, 30, 0),
        ("StatementText", Memo, 9, 0),
        ("StatementID", Integer, 4, 0),
        ("Operator", Character, 30, 0),
        ("Arguments", Memo, 9, 0),
        ("EstimatedExecution", Integer, 4, 0),
        ("Warning", Memo, 9, 0),
    ]
};

/// The rows of a plan, built from the statement down.
struct Rows(Vec<PlanRow>);

impl Rows {
    fn new(row_type: &str, text: &str) -> Rows {
        Rows(vec![PlanRow {
            node: 1,
            parent: 0,
            row_type: row_type.to_owned(),
            text: Some(text.to_owned()),
            operator: None,
            arguments: None,
            estimate: Some(1),
            warning: None,
        }])
    }

    /// Adds a node under `parent` and returns its number.
    fn add(
        &mut self,
        parent: i32,
        operator: &str,
        text: Option<String>,
        arguments: Option<String>,
        estimate: Option<i32>,
        warning: Option<String>,
    ) -> i32 {
        let node = self.0.len() as i32 + 1;
        self.0.push(PlanRow {
            node,
            parent,
            row_type: "PLAN".to_owned(),
            text,
            operator: Some(operator.to_owned()),
            arguments,
            estimate,
            warning,
        });
        node
    }
}

/// The plan of a statement that is not a `SELECT` (`EXECUTE PROCEDURE`):
/// the statement alone.
pub fn statement(row_type: &str, text: &str) -> Vec<PlanRow> {
    Rows::new(row_type, text).0
}

/// `rows`, the plan of the `SELECT` that reads the rows an `INSERT`, an
/// `UPDATE` or a `DELETE` (`row_type`) works on, or the statement alone, as
/// the plan of that statement: the step `operator` that changes `table`
/// (with the `arguments` it shows) comes under the statement, and what reads
/// the rows under it (`fixtures/sql/planupd.json`).
pub fn dml(
    rows: &mut Vec<PlanRow>,
    row_type: &str,
    operator: &str,
    table: &str,
    arguments: Option<&str>,
) {
    for row in rows.iter_mut().skip(1) {
        row.node += 1;
        row.parent += 1;
    }
    rows[0].row_type = row_type.to_owned();
    rows.insert(
        1,
        PlanRow {
            node: 2,
            parent: 1,
            row_type: "PLAN".to_owned(),
            text: Some(table.to_owned()),
            operator: Some(operator.to_owned()),
            arguments: arguments.map(str::to_owned),
            estimate: None,
            warning: None,
        },
    );
}

/// The plan of a `SELECT` that reads the table `name` (`alias` in the
/// statement) as it is.
pub fn scan(text: &str, name: &str, alias: &str) -> Vec<PlanRow> {
    let mut rows = Rows::new("SELECT", text);
    rows.add(
        1,
        "TABLE SCAN",
        Some(scanned(name, alias)),
        None,
        Some(1),
        None,
    );
    rows.0
}

/// What a scan reads: a table's name and alias.
fn scanned(name: &str, alias: &str) -> String {
    format!("Table Name : {name}\nTable Alias : {alias}\n")
}

/// What a live cursor's plan shows of the indexes it uses: its `WHERE` as
/// the original engine's AOF, optimized with the table's orders, and the
/// order its `ORDER BY` reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveIndexes {
    /// `None` for a `WHERE` that is no AOF.
    pub aof: Option<LiveAof>,
    /// The order whose key the `ORDER BY` is, if one is.
    pub order: Option<String>,
}

/// A live cursor's `WHERE` as an AOF (see [`crate::xbase`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveAof {
    pub text: String,
    /// `ADS_OPTIMIZED_FULL`, `_PART` or `_NONE`; 0 for an AOF the original
    /// engine cannot set (see [`crate::xbase::unsettable`]).
    pub level: u16,
    /// The part not optimized.
    pub residual: String,
    pub details: Vec<LiveDetail>,
}

/// A line of an AOF's optimization details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveDetail {
    /// The order that finds the records; `None` for none.
    pub index: Option<String>,
    pub expr: String,
    /// How many records it finds; `None` for none (shown as `NA`).
    pub actual: Option<u32>,
}

/// The plan of a live cursor: `plan` over the table called `name`
/// (`alias` in the statement) with `fields`, using `indexes`. Its records
/// are filtered as they are read: through an AOF, and for an `ORDER BY` in
/// an order whose key it is, else in a temporary index.
pub fn live(
    text: &str,
    plan: &Plan,
    name: &str,
    alias: &str,
    fields: &[Field],
    indexes: &LiveIndexes,
) -> Vec<PlanRow> {
    let plain: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
    let render = Render {
        labels: vec![&plain],
        aggregates: &[],
        hidden: false,
        bare: false,
    };
    let mut rows = Rows::new("SELECT", text);
    let mut scan_text = scanned(name, alias);
    let mut arguments = String::new();
    let mut warning = String::new();
    // An `ORDER BY` of constants sorts nothing.
    let index = match crate::order_index::temp_index(&plan.order, fields) {
        crate::order_index::Order::Index(index) => Some(index),
        _ => None,
    };
    let ordered = index.is_some();
    let mut operator = "TABLE SCAN (LIVE)";
    let mut evaluator = None;
    if let Some(filter) = &plan.filter {
        match &indexes.aof {
            Some(aof) if aof.level == 0 => {
                arguments += &format!("\nAOF<{}>", aof.text);
                warning += "Cannot set AOF.";
            }
            Some(aof) => {
                arguments += &aof_details(aof, ordered);
                match aof.level {
                    1 => operator = "AOF SCAN (LIVE)",
                    2 => {
                        operator = "AOF SCAN (LIVE)";
                        warning += &format!(
                            "Restriction is not fully optimized.\nUnoptimized Part : {}\n",
                            aof.residual
                        );
                    }
                    _ => {
                        warning += &format!(
                            "Restriction is not optimized.\nUnoptimized Part : {}\n",
                            aof.residual
                        );
                    }
                }
                evaluator = Some(
                    aof.text
                        .replace(" .AND. ", ".AND.")
                        .replace(" .OR. ", ".OR."),
                );
            }
            None => {
                let expr = render.condition(filter);
                arguments += &aof(&expr);
                warning += &format!("Restriction is not optimized.\nUnoptimized Part : {expr}\n");
                evaluator = Some(expr);
            }
        }
    }
    if ordered {
        operator = "INDEX SCAN (LIVE)";
        let expression = index.map(|index| index.expression).unwrap_or_default();
        arguments += &format!("\nIndex Expression for ORDER BY<{expression}>");
        match &indexes.order {
            Some(tag) => scan_text.push_str(&format!("Index Tag : {tag}\n")),
            None => {
                scan_text.push_str("Index Tag : Temp\n");
                warning +=
                    "\nNo index matching the ORDER BY clause. Temporary Index will be built.";
            }
        }
    }
    let some = |s: String| (!s.is_empty()).then_some(s);
    match evaluator.filter(|_| ordered) {
        // With both, an evaluator applies the condition above the index.
        Some(condition) => {
            let parent = rows.add(1, "EVALUATOR (LIVE)", None, Some(condition), Some(1), None);
            rows.add(
                parent,
                operator,
                Some(scan_text),
                some(arguments),
                Some(1),
                some(warning),
            );
        }
        None => {
            rows.add(
                1,
                operator,
                Some(scan_text),
                some(arguments),
                Some(1),
                some(warning),
            );
        }
    }
    rows.0
}

/// An unoptimized restriction, as the original engine describes an AOF.
fn aof(expr: &str) -> String {
    format!(
        "\nAOF<{expr}> \n====================\nAOF Optimization Details: \n Index: <none>, Expr: \
         {expr}, Estimate: NA, Actual: NA\n====================\n"
    )
}

/// An AOF's text and the details of its optimization, as the original
/// engine describes them; before an `ORDER BY`'s index expression, the
/// last line ends otherwise.
fn aof_details(aof: &LiveAof, ordered: bool) -> String {
    let mut out = format!(
        "\nAOF<{}> \n====================\nAOF Optimization Details: \n",
        aof.text
    );
    for detail in &aof.details {
        let index = detail.index.as_deref().unwrap_or("<none>");
        let actual = detail.actual.map_or("NA".to_owned(), |n| n.to_string());
        out += &format!(
            " Index: {index}, Expr: {}, Estimate: NA, Actual: {actual}\n",
            detail.expr
        );
    }
    out += if ordered {
        "==================== "
    } else {
        "====================\n"
    };
    out
}

/// The plan of a `SELECT` that runs through its operators. With the
/// `tables` it reads (and the values of its parameters), it says how many
/// records each restriction an index serves holds for, as the original
/// engine does.
pub fn query(text: &str, prepared: &Prepared, tables: Option<(&dyn Tables, &Env)>) -> Vec<PlanRow> {
    planned(text, prepared, tables, false)
}

/// [`query`], for a `MERGE`'s join if `merge`.
fn planned(
    text: &str,
    prepared: &Prepared,
    tables: Option<(&dyn Tables, &Env)>,
    merge: bool,
) -> Vec<PlanRow> {
    let mut rows = Rows::new("SELECT", text);
    let exec = tables.map(|(tables, env)| Exec::new(tables, env, &prepared.subqueries));
    let mut plans = Plans {
        rows: &mut rows,
        subqueries: &prepared.subqueries,
        exec: exec.as_ref(),
        merge,
        counting: false,
        changing: prepared.changing,
    };
    plans.op(&prepared.op, 1, None);
    rows.0
}

/// What the plan of a `MERGE` shows besides the join of its source and
/// target: a step for each clause, with the name it is shown under and its
/// arguments.
#[derive(Debug)]
pub struct MergeSteps {
    pub update: Option<(String, String)>,
    pub insert: Option<(String, String)>,
}

/// The plan of a `MERGE`: `prepared` is the join of its source (left) and
/// target (right) on its condition. The `WHERE`-like step the join makes of
/// that condition is not shown, and the clauses follow the join
/// (`fixtures/sql/merge.json`).
pub fn merge(
    text: &str,
    prepared: &Prepared,
    tables: Option<(&dyn Tables, &Env)>,
    steps: &MergeSteps,
) -> Vec<PlanRow> {
    let mut rows = planned(text, prepared, tables, true);
    rows[0].row_type = "MERGE".to_owned();
    // The condition's own step goes, its children moving up.
    if let Some(at) = rows
        .iter()
        .position(|r| r.operator.as_deref() == Some("EVALUATE"))
    {
        let (gone, parent) = (rows[at].node, rows[at].parent);
        rows.remove(at);
        for row in &mut rows {
            if row.parent == gone {
                row.parent = parent;
            }
            if row.node > gone {
                row.node -= 1;
            }
            if row.parent > gone {
                row.parent -= 1;
            }
        }
    }
    // The source is looped over, a scan of the target for each of its rows,
    // whatever the keys, and its scan runs as often as the target's.
    if let Some(join) = rows
        .iter()
        .position(|r| r.operator.as_deref() == Some("JOIN"))
    {
        let node = rows[join].node;
        let sides: Vec<usize> = (0..rows.len())
            .filter(|&i| rows[i].parent == node)
            .collect();
        if let [left, right] = sides[..] {
            rows[left].estimate = rows[right].estimate;
            let hash = rows[join]
                .arguments
                .take_if(|a| a.starts_with("\nHash Join<"));
            let hash = hash.map(|a| a["\nHash Join<".len()..a.len() - 1].to_owned());
            let (arguments, warning) = merge_scan(
                rows[right].arguments.take(),
                rows[right].warning.take(),
                hash,
            );
            rows[right].arguments = arguments;
            rows[right].warning = warning;
        }
    }
    let update = steps.update.as_ref().map(|step| ("UPDATE", step));
    let insert = steps.insert.as_ref().map(|step| ("INSERT INTO", step));
    for (operator, (name, arguments)) in update.into_iter().chain(insert) {
        let node = rows.len() as i32 + 1;
        rows.push(PlanRow {
            node,
            parent: 1,
            row_type: "PLAN".to_owned(),
            text: Some(name.clone()),
            operator: Some(operator.to_owned()),
            arguments: Some(arguments.clone()),
            estimate: None,
            warning: None,
        });
    }
    rows
}

/// What a `MERGE`'s scan of the target shows, from what a join's right
/// side shows (`hash` the keys of a hash join): the join condition after
/// the restrictions, and the warning about a join no index finds the rows
/// of after theirs.
fn merge_scan(
    arguments: Option<String>,
    warning: Option<String>,
    hash: Option<String>,
) -> (Option<String>, Option<String>) {
    let hashed = hash.is_some();
    let arguments = arguments.unwrap_or_default();
    let (joined, restricted) = match arguments.find("\nAOF<") {
        Some(at) => arguments.split_at(at),
        None => (arguments.as_str(), ""),
    };
    let joined = match hash {
        Some(keys) => format!("\nJoin Condition<{keys}>"),
        None => joined.to_owned(),
    };
    let arguments = match (joined.is_empty(), restricted.is_empty()) {
        (false, false) => format!("{} {joined}", restricted.trim_end_matches('\n')),
        _ => joined.clone() + restricted,
    };
    let warning = warning.unwrap_or_default();
    let unindexed = hashed || warning.contains(NO_JOIN_INDEX);
    let restriction = warning.replace(NO_JOIN_INDEX, "");
    let warning = match (restriction.is_empty(), unindexed) {
        (true, true) => NO_JOIN_INDEX.to_owned(),
        (false, true) => format!("{restriction}\n{NO_JOIN_INDEX}"),
        _ => restriction,
    };
    (
        (!arguments.is_empty()).then_some(arguments),
        (!warning.is_empty()).then_some(warning),
    )
}

/// An expression as plans show it, over the columns `labels`.
pub(crate) fn expression(labels: &[String], node: &Node) -> String {
    let ctx = Context {
        labels: labels.to_vec(),
        aggregates: Vec::new(),
        outer: None,
    };
    render(&ctx, false).node(node)
}

/// The labels of a context and those around it, innermost first.
fn labels(ctx: &Context) -> Vec<&[String]> {
    let mut out = vec![ctx.labels.as_slice()];
    let mut outer = ctx.outer.as_deref();
    while let Some(o) = outer {
        out.push(o.labels.as_slice());
        outer = o.outer.as_deref();
    }
    out
}

fn render(ctx: &Context, hidden: bool) -> Render<'_> {
    Render {
        labels: labels(ctx),
        aggregates: &ctx.aggregates,
        hidden,
        bare: false,
    }
}

/// The join an operator is the right side of: its condition, how it finds
/// the right rows, the right side's columns, and how many left rows it
/// runs for, if known.
struct JoinSide<'a> {
    condition: Option<&'a Node>,
    method: &'a JoinMethod,
    range: (usize, usize),
    ctx: &'a Rc<Context>,
    runs: Option<usize>,
}

struct Plans<'a> {
    rows: &'a mut Rows,
    subqueries: &'a [SubPlan],
    /// What counts the records of restrictions.
    exec: Option<&'a Exec<'a>>,
    /// Whether the plan is a `MERGE`'s, whose restrictions an index serves
    /// are never shown as a scope.
    merge: bool,
    /// Whether the rows under the step are only counted, so that the
    /// original shows no step for the table they are read from.
    counting: bool,
    /// Whether the plan is of the rows another statement changes.
    changing: bool,
}

impl Plans<'_> {
    /// The nodes of `op` under `parent`; `join` if it is a join's right
    /// side.
    fn op(&mut self, op: &Op, parent: i32, join: Option<JoinSide<'_>>) {
        // A join's condition shows on its right side.
        let (join_arguments, join_warning) = match &join {
            Some(JoinSide {
                condition: Some(on),
                method,
                range,
                ctx,
                ..
            }) => (
                Some(format!(
                    "\nJoin Condition<{}>",
                    render(ctx, false).join_condition(on, *range)
                )),
                matches!(method, JoinMethod::Loop).then(|| NO_JOIN_INDEX.to_owned()),
            ),
            _ => (None, None),
        };
        match op {
            Op::Final { input, .. } | Op::Fit { input, .. } => self.op(input, parent, join),
            Op::Project {
                input,
                columns,
                keys,
                ..
            } => {
                // A `COUNT(*)` alone is answered from the table, whose scan
                // the plan leaves out, with the `WHERE` step above it
                // (`fixtures/sql/derived.json`).
                let counted = matches!(
                    &**input,
                    Op::Group { keys, aggregates, .. }
                        if keys.is_empty()
                            && aggregates.iter().all(|a| {
                                a.func == crate::ast::Aggregate::Count
                                    && a.arg.is_none()
                                    && !a.distinct
                            })
                ) && columns.iter().all(|c| matches!(c.kind, Kind::Agg(_)));
                let before = std::mem::replace(&mut self.counting, counted);
                self.op(input, parent, join);
                self.counting = before;
                // The select list's subqueries run for each row.
                let mut ids = Vec::new();
                for node in columns.iter().chain(keys) {
                    subquery_ids(node, &mut ids);
                }
                self.subqueries(&ids, parent);
            }
            // The original engine shows no step for `TOP`.
            Op::Top { input, .. } => self.op(input, parent, join),
            Op::Sort {
                input,
                keys,
                purpose,
                ctx,
            } => {
                let render = render(ctx, *purpose != Purpose::Group);
                let shown: Vec<String> = keys.iter().map(|k| render.node(&k.shown)).collect();
                // The `ORDER BY` of a table's records has its keys `::`-joined;
                // one on values the rows do not show reads them again by
                // bookmark, and says so rather than that it is the `ORDER BY`
                // (`fixtures/sql/fts_plans.json`).
                let bookmark = *purpose == Purpose::Stored && !regrouped(input);
                let arguments = match purpose {
                    _ if bookmark => format!("{} \n<Using Bookmark>", shown.join("::")),
                    Purpose::Group => shown.join(", "),
                    _ if regrouped(input) => shown.join(", "),
                    _ => shown.join("::"),
                };
                let text = (*purpose != Purpose::Group && !bookmark).then(|| "ORDER BY".to_owned());
                let node = self
                    .rows
                    .add(parent, "SORT", text, Some(arguments), None, None);
                self.op(input, node, None);
            }
            Op::Distinct {
                input,
                columns,
                ctx,
                ..
            } => {
                let render = render(ctx, false);
                let columns: Vec<String> = columns.iter().map(|c| render.node(c)).collect();
                let arguments = format!("Sort and remove duplicates:<{}>", columns.join(", "));
                let node = self
                    .rows
                    .add(parent, "DISTINCT", None, Some(arguments), None, None);
                self.op(input, node, None);
            }
            Op::Filter {
                input,
                condition,
                clause,
                ctx,
            } => {
                let arguments = match clause {
                    Clause::Where => {
                        format!(
                            "WHERE clause \n<{}>",
                            render(ctx, false).condition(condition)
                        )
                    }
                    Clause::Having => {
                        format!(
                            "HAVING clause \n<{}>",
                            render(ctx, true).condition(condition)
                        )
                    }
                };
                let node = self
                    .rows
                    .add(parent, "EVALUATE", None, Some(arguments), None, None);
                self.op(input, node, None);
                // The condition's subqueries run under it.
                let mut ids = Vec::new();
                subquery_ids(condition, &mut ids);
                self.subqueries(&ids, node);
            }
            Op::Group {
                input,
                aggregates,
                ctx,
                ..
            } => {
                let render = render(ctx, false);
                let shown: Vec<String> = aggregates.iter().map(|a| render.aggregate(a)).collect();
                let arguments = (!shown.is_empty())
                    .then(|| format!("Aggregate Calculation<{}>", shown.join("::")));
                let node = self
                    .rows
                    .add(parent, "GROUP BY", None, arguments, None, None);
                self.op(input, node, None);
            }
            Op::Join {
                kind,
                method,
                left,
                right,
                right_range,
                condition,
                ctx,
            } => {
                let operator = match kind {
                    JoinKind::Inner => "JOIN",
                    JoinKind::Left => "LEFT OUTER JOIN",
                    JoinKind::Right => "RIGHT OUTER JOIN",
                    JoinKind::Full => "FULL OUTER JOIN",
                };
                let mut arguments = join_arguments.unwrap_or_default();
                if let JoinMethod::Hash(keys) = method {
                    let labels = &ctx.labels;
                    let label = |i: usize| labels.get(i).cloned().unwrap_or_default();
                    let keys: Vec<String> = keys
                        .iter()
                        .map(|k| format!("{}={}", label(k.right), label(k.left)))
                        .collect();
                    arguments += &format!("\nHash Join<{}>", keys.join(" .AND. "));
                }
                let arguments = (!arguments.is_empty()).then_some(arguments);
                let node = self
                    .rows
                    .add(parent, operator, None, arguments, None, join_warning);
                let before = std::mem::replace(&mut self.counting, false);
                self.op(left, node, None);
                // The right side runs once for each left row.
                let runs = self.exec.and_then(|exec| exec.count(left).ok());
                let side = JoinSide {
                    condition: condition.as_ref(),
                    method,
                    range: *right_range,
                    ctx,
                    runs,
                };
                self.op(right, node, Some(side));
                self.counting = before;
            }
            Op::Scan {
                table,
                start,
                width,
                name,
                alias,
                restrictions,
                engine: _,
                ctx,
            } => {
                // `system.iota` is scanned still.
                if self.counting && name != "iota" {
                    return;
                }
                let index = match &join {
                    Some(JoinSide {
                        method: JoinMethod::Index { index, .. },
                        ..
                    }) => Some(index),
                    _ => None,
                };
                let mut scan = Scan {
                    operator: "TABLE SCAN",
                    text: scanned(name, alias),
                    arguments: join_arguments.unwrap_or_default(),
                    warning: join_warning.unwrap_or_default(),
                };
                if let Some(index) = index {
                    scan.joined_by(index);
                }
                let actual = |restriction: &Restriction| {
                    let exec = self.exec?;
                    exec.matching(*table, (*start, *width), restriction)
                        .ok()
                        .flatten()
                };
                // What a `CONTAINS`'s full-text order finds.
                let fts = |restriction: &Restriction| {
                    let exec = self.exec?;
                    let Kind::Contains(search) = &restriction.condition.kind else {
                        return None;
                    };
                    let FtsCondition::Const(query) = &search.condition else {
                        return None;
                    };
                    let column = match &search.source {
                        FtsSource::Fields(_) => None,
                        FtsSource::Expr(_) => Some(restriction.column - start),
                    };
                    exec.tables.fts_lookup(*table, column, query).ok().flatten()
                };
                scan.restrictions(restrictions, ctx, &actual, &fts, self.merge, self.changing);
                let some = |s: String| (!s.is_empty()).then_some(s);
                let Scan {
                    operator,
                    text,
                    arguments,
                    warning,
                } = scan;
                let runs = join.as_ref().and_then(|side| side.runs).unwrap_or(1);
                let estimate = Some(i32::try_from(runs).unwrap_or(i32::MAX));
                self.rows.add(
                    parent,
                    operator,
                    Some(text),
                    some(arguments),
                    estimate,
                    some(warning),
                );
            }
            // The original shows the derived table as a scan of its own and
            // nothing of the `SELECT` it runs (`fixtures/sql/derived.json`).
            Op::Derived { alias, .. } => {
                if self.counting {
                    return;
                }
                let text = Some(scanned(alias, alias));
                self.rows.add(
                    parent,
                    "TABLE SCAN",
                    text,
                    join_arguments,
                    Some(1),
                    join_warning,
                );
            }
            Op::Concat { inputs } => {
                let node = self
                    .rows
                    .add(parent, "UNION ALL", None, None, Some(1), None);
                for input in inputs {
                    self.op(input, node, None);
                }
            }
        }
    }

    /// The plans of subqueries `ids` under `parent`; one that reads the rows
    /// around it says it runs for each.
    fn subqueries(&mut self, ids: &[usize], parent: i32) {
        for &id in ids {
            let Some(sub) = self.subqueries.get(id) else {
                continue;
            };
            let first = self.rows.0.len();
            let before = std::mem::replace(&mut self.counting, false);
            self.op(&sub.op, parent, None);
            self.counting = before;
            let Some(row) = self.rows.0.get_mut(first) else {
                continue;
            };
            match sub.correlated {
                true => {
                    let warning = row.warning.get_or_insert_with(String::new);
                    warning.push_str("\nRuns once for each row of the SELECT around it.");
                }
                // One that does not runs once.
                false => {
                    row.estimate.get_or_insert(1);
                }
            }
        }
    }
}

/// The warning of a join's right side whose rows no index finds.
const NO_JOIN_INDEX: &str = "\nThere is no existing index to optimize the join condition.";

/// A scan's node, as it is built.
struct Scan {
    operator: &'static str,
    text: String,
    arguments: String,
    warning: String,
}

impl Scan {
    /// The scan's records are looked up in `index`, by a join's condition or
    /// a restriction on a column of the `SELECT` around.
    fn joined_by(&mut self, index: &IndexUse) {
        if self.operator == "TABLE SCAN" {
            self.operator = "INDEX SCAN";
            self.text += &format!(
                "Index Tag : {}\nIndex Expression : {}\n",
                index.name, index.expression
            );
        }
    }

    /// Shows `restrictions`, over the row of `ctx`: one on a column of the
    /// `SELECT` around as a join condition; the others as the original
    /// engine shows an AOF, or the range of one index (`Scope
    /// Optimization`) if they are one scope it finds.
    /// `actual` finds the records a restriction an index serves holds for,
    /// `fts` what a `CONTAINS`'s full-text order finds.
    fn restrictions(
        &mut self,
        restrictions: &[Restriction],
        ctx: &Context,
        actual: &dyn Fn(&Restriction) -> Option<Vec<u32>>,
        fts: &dyn Fn(&Restriction) -> Option<FtsLookup>,
        merge: bool,
        changing: bool,
    ) {
        let render = render(ctx, false);
        let (correlated, plain): (Vec<&Restriction>, Vec<&Restriction>) =
            restrictions.iter().partition(|r| outer(&r.condition));
        for restriction in correlated {
            let condition = render.join_condition(&restriction.condition, (0, usize::MAX));
            self.arguments += &format!("\nJoin Condition<{condition}>");
            match &restriction.index {
                Some(index) => self.joined_by(index),
                None => self.warning += NO_JOIN_INDEX,
            }
        }
        let bare = Render {
            bare: true,
            ..render
        };
        // An `IN` list is an AOF of its own, `OR`ed equalities no order finds:
        // alone it is all of the scan's arguments, beside others it follows
        // their details (`fixtures/sql/indexsel.json`).
        let (lists, plain): (Vec<&Restriction>, Vec<&Restriction>) =
            match plain.iter().any(|r| r.fts) {
                true => (Vec::new(), plain),
                false => plain
                    .into_iter()
                    .partition(|r| matches!(r.condition.kind, Kind::In(..))),
            };
        let lists: Vec<String> = lists
            .iter()
            .map(|r| bare.in_restriction(&r.condition))
            .collect();
        let scoped = match plain.as_slice() {
            all if !all.is_empty()
                && !merge
                && lists.is_empty()
                && all.iter().all(|r| r.ordered && r.index.is_some()) =>
            {
                all[0].index.as_ref()
            }
            _ => None,
        };
        if let (Some(index), "TABLE SCAN") = (scoped, self.operator) {
            self.operator = "INDEX SCAN";
            self.text += &format!("Tag : {}\n", index.name);
            // Written in the order of the order's fields.
            let mut written: Vec<&&Restriction> = plain.iter().collect();
            written.sort_by_key(|r| r.sort.iter().position(|(c, _)| *c == r.column));
            let texts: Vec<String> = written
                .iter()
                .map(|r| bare.restriction(&r.condition, true))
                .collect();
            let restriction = texts.join(" AND ");
            self.arguments += &format!("\nScope Optimization for restriction: <{restriction}>");
            return;
        }
        if plain.is_empty() {
            if !lists.is_empty() {
                self.arguments += &format!("\nAOF<{}>", lists.join(" AND "));
            }
            return;
        }
        if plain.iter().any(|r| r.fts) {
            self.fts_restrictions(&plain, &bare, actual, fts);
            return;
        }
        let spaced: Vec<String> = plain
            .iter()
            .map(|r| bare.restriction(&r.condition, true))
            .collect();
        // A lone `=`, `>=` or `<=` no order finds has no details.
        if let [only] = plain.as_slice()
            && only.scope
            && only.index.is_none()
            && lists.is_empty()
            && !merge
            && !changing
        {
            self.arguments += &format!("\nAOF<{}>", spaced[0]);
            self.warning += "Restriction is not optimized.";
            return;
        }
        self.arguments += &format!(
            "\nAOF<{}> \n====================\nAOF Optimization Details: \n",
            spaced.join(" AND ")
        );
        let mut unoptimized = Vec::new();
        // The details come last first.
        for restriction in plain.iter().rev() {
            let expr = bare.restriction(&restriction.condition, false);
            let index = restriction
                .index
                .as_ref()
                .map_or("<none>", |index| index.name.as_str());
            let actual = match restriction.index {
                Some(_) => actual(restriction).map_or("NA".to_owned(), |n| n.len().to_string()),
                None => "NA".to_owned(),
            };
            self.arguments +=
                &format!(" Index: {index}, Expr: {expr}, Estimate: NA, Actual: {actual}\n");
            if restriction.index.is_none() {
                unoptimized.insert(0, expr);
            }
        }
        match lists.is_empty() {
            true => self.arguments += "====================\n",
            false => {
                self.arguments += &format!("==================== \nAOF<{}>", lists.join(" AND "));
            }
        }
        if unoptimized.len() == plain.len() {
            self.warning += &format!(
                "Restriction is not optimized.\nUnoptimized Part : {}\n",
                unoptimized.join(" .AND. ")
            );
        } else if self.operator == "TABLE SCAN" {
            self.operator = "AOF SCAN";
        }
    }
}

impl Scan {
    /// Shows `restrictions` with a `CONTAINS` among them, as the original
    /// engine shows them then (`fixtures/sql/fts_plans.json`): an AOF of them
    /// all (a `CONTAINS` compared `= 1`), joined by ` AND ` in the reverse of
    /// the order its details list them in: the `CONTAINS` its full-text orders
    /// answer, the other comparisons that are not equalities, the `CONTAINS` no
    /// order answers, and the equalities, those without an index first — each
    /// group in the order the `WHERE` has them. An `IN` an index serves is
    /// shown as an AOF of its own after them; one no index serves, and a
    /// comparison of a memo, not at all. The details count the records each
    /// part finds as a live cursor's do, and the warning is doubled.
    fn fts_restrictions(
        &mut self,
        restrictions: &[&Restriction],
        render: &Render,
        actual: &dyn Fn(&Restriction) -> Option<Vec<u32>>,
        fts: &dyn Fn(&Restriction) -> Option<FtsLookup>,
    ) {
        use crate::fts::IndexLevel;
        /// A part of the AOF: its restriction, what an index or full-text
        /// order finds for it (`None` for none), its level and texts.
        struct Part<'r> {
            restriction: &'r Restriction,
            found: Option<Vec<u32>>,
            fts: bool,
            level: IndexLevel,
            spaced: String,
            text: String,
        }
        let mut parts: Vec<(u8, Part)> = Vec::new();
        let mut trailers = Vec::new();
        for &restriction in restrictions {
            let node = &restriction.condition;
            if let Kind::Contains(search) = &node.kind {
                let found = fts(restriction);
                let level = found.as_ref().map_or(IndexLevel::None, |found| found.level);
                let text = format!("CONTAINS({})", render.fts_args(search));
                let class = if level == IndexLevel::None { 2 } else { 0 };
                let found = found
                    .filter(|_| level != IndexLevel::None)
                    .map(|f| f.records);
                let spaced = format!("{text} = 1");
                let text = format!("{text}=1");
                let part = Part {
                    restriction,
                    found,
                    fts: true,
                    level,
                    spaced,
                    text,
                };
                parts.push((class, part));
                continue;
            }
            let memo = crate::query::children_of(node)
                .iter()
                .any(|child| matches!(child.kind, Kind::Column(_)) && child.ty.is_memo());
            if memo {
                continue;
            }
            if let Kind::In(..) = node.kind {
                if restriction.index.is_some() {
                    trailers.push(render.in_restriction(node));
                }
                continue;
            }
            let equality = matches!(
                node.kind,
                Kind::Binary(BinOp::Eq, ..) | Kind::IsNull(_, false) | Kind::Column(_)
            );
            let indexed = restriction.index.is_some();
            let class = match (equality, indexed) {
                (false, _) => 1,
                (true, false) => 3,
                (true, true) => 4,
            };
            let found = if indexed { actual(restriction) } else { None };
            let level = if indexed {
                IndexLevel::Full
            } else {
                IndexLevel::None
            };
            let spaced = render.restriction(node, true);
            let text = render.restriction(node, false);
            parts.push((
                class,
                Part {
                    restriction,
                    found,
                    fts: false,
                    level,
                    spaced,
                    text,
                },
            ));
        }
        parts.sort_by_key(|(class, _)| *class);
        let parts: Vec<Part> = parts.into_iter().map(|(_, part)| part).collect();
        let aof: Vec<&str> = parts
            .iter()
            .rev()
            .map(|part| part.spaced.as_str())
            .collect();
        self.arguments += &format!(
            "\nAOF<{}> \n====================\nAOF Optimization Details: \n",
            aof.join(" AND ")
        );
        // The detail lines: two bounds of one index's keys are one line, the
        // lower first.
        let side = |part: &Part| match &part.restriction.condition.kind {
            Kind::Binary(BinOp::Gt | BinOp::Ge, ..) => Some(false),
            Kind::Binary(BinOp::Lt | BinOp::Le, ..) => Some(true),
            _ => None,
        };
        let mut lines: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            let pair = lines.iter_mut().find(|(_, members)| {
                let first = &parts[members[0]];
                members.len() == 1
                    && first.restriction.index.is_some()
                    && first.restriction.index.as_ref().map(|i| i.number)
                        == part.restriction.index.as_ref().map(|i| i.number)
                    && side(first).is_some()
                    && side(part).is_some()
                    && side(first) != side(part)
            });
            match pair {
                Some((text, members)) if side(part) == Some(false) => {
                    *text = format!("{} AND {text}", part.text);
                    members.insert(0, i);
                }
                Some((text, members)) => {
                    *text = format!("{text} AND {}", part.text);
                    members.push(i);
                }
                None => lines.push((part.text.clone(), vec![i])),
            }
        }
        // Of an `AND` of lookups, the original engine looks up no further
        // once they leave no record, or once two leave one and what is next
        // is not a `CONTAINS`.
        let (mut lookups, mut left): (u32, Option<Vec<u32>>) = (0, None);
        let intersect = |left: Option<Vec<u32>>, found: &[u32]| match left {
            Some(left) => left.into_iter().filter(|r| found.contains(r)).collect(),
            None => found.to_vec(),
        };
        for (text, members) in &lines {
            let first = &parts[members[0]];
            let index = match first.fts {
                true if first.level != IndexLevel::None => {
                    fts(first.restriction).map(|found| found.order)
                }
                true => None,
                false => first
                    .restriction
                    .index
                    .as_ref()
                    .map(|index| index.name.clone()),
            };
            let left_count = left.as_ref().map(Vec::len);
            let done = lookups >= 1 && left_count == Some(0)
                || lookups >= 2 && left_count.is_some_and(|n| n <= 1) && !first.fts;
            let mut found: Option<Vec<u32>> = None;
            for &member in members {
                if let Some(records) = &parts[member].found {
                    found = Some(intersect(found, records));
                }
            }
            let actual = match (found, done) {
                (Some(found), false) => {
                    lookups += 1;
                    let count = found.len();
                    left = Some(intersect(left, &found));
                    // A `CONTAINS` that finds nothing says so; an index, NA.
                    (first.fts || count > 0).then(|| count.to_string())
                }
                _ => None,
            };
            let index = index.as_deref().unwrap_or("<none>");
            let actual = actual.as_deref().unwrap_or("NA");
            self.arguments +=
                &format!(" Index: {index}, Expr: {text}, Estimate: NA, Actual: {actual}\n");
        }
        self.arguments += match trailers.first() {
            Some(trailer) => format!("==================== \nAOF<{trailer}>"),
            None => "====================\n".to_owned(),
        }
        .as_str();
        let optimized = |part: &Part| part.level != IndexLevel::None;
        if parts.iter().all(|part| !optimized(part)) {
            self.operator = "TABLE SCAN";
            let texts: Vec<&str> = parts.iter().map(|part| part.text.as_str()).collect();
            self.warning += &format!(
                "Restriction is not optimized.\nRestriction is not optimized.\nUnoptimized Part : \
                 {}\n",
                texts.join(".AND.")
            );
            return;
        }
        if self.operator == "TABLE SCAN" {
            self.operator = "AOF SCAN";
        }
        if parts.iter().all(|part| part.level == IndexLevel::Full) {
            return;
        }
        // What is left to evaluate, in the AOF's order: the parts no index
        // finds, and the `CONTAINS` an order only narrows but finds records
        // for; nothing when the orders find no record at all.
        let mut bound: Option<Vec<u32>> = None;
        for part in &parts {
            if let Some(found) = &part.found {
                bound = Some(intersect(bound, found));
            }
        }
        let residual: Vec<&str> = match bound.is_some_and(|bound| bound.is_empty()) {
            true => Vec::new(),
            false => parts
                .iter()
                .rev()
                .filter(|part| match part.level {
                    IndexLevel::None => true,
                    IndexLevel::Partial => part.found.as_ref().is_some_and(|f| !f.is_empty()),
                    IndexLevel::Full => false,
                })
                .map(|part| part.text.as_str())
                .collect(),
        };
        self.warning += &format!(
            "Restriction is not fully optimized.\nRestriction is not fully optimized.\n\
             Unoptimized Part : {}\n",
            residual.join(".AND.")
        );
    }
}

/// Whether the rows `op` makes are grouped or `UNION`ed rather than a table's
/// records (or joined ones).
fn regrouped(op: &Op) -> bool {
    match op {
        Op::Group { .. } | Op::Concat { .. } => true,
        Op::Final { input, .. }
        | Op::Fit { input, .. }
        | Op::Project { input, .. }
        | Op::Top { input, .. }
        | Op::Sort { input, .. }
        | Op::Distinct { input, .. }
        | Op::Filter { input, .. } => regrouped(input),
        _ => false,
    }
}

/// Whether `node` reads a column of a `SELECT` around.
fn outer(node: &Node) -> bool {
    matches!(node.kind, Kind::Outer(..)) || children_of(node).into_iter().any(outer)
}

/// Expressions as the original engine's plans show them: columns as
/// `[table].[column]`, `.AND.`, `.OR.`, `.T.` and text in double quotes.
struct Render<'a> {
    /// The columns of the row, then of the rows around it.
    labels: Vec<&'a [String]>,
    aggregates: &'a [AggSpec],
    /// Whether aggregates show as `( ... ) `, as in `HAVING` and `ORDER BY`.
    hidden: bool,
    /// Whether the row's own columns show without their table, as in an
    /// AOF.
    bare: bool,
}

impl Render<'_> {
    /// A condition: a logical column alone shows as `column=.T.`.
    fn condition(&self, node: &Node) -> String {
        match &node.kind {
            Kind::Column(_) | Kind::Outer(..) => format!("{}=.T.", self.node(node)),
            Kind::Binary(op @ (BinOp::And | BinOp::Or), a, b) => {
                // An `OR` under an `AND` is parenthesized.
                let operand = |n: &Node| match (op, &n.kind) {
                    (BinOp::And, Kind::Binary(BinOp::Or, ..)) => format!("({})", self.condition(n)),
                    _ => self.condition(n),
                };
                let op = if *op == BinOp::And {
                    " .AND. "
                } else {
                    " .OR. "
                };
                format!("{}{op}{}", operand(a), operand(b))
            }
            // A `CONTAINS` compares with `.T.`, in parentheses after `.NOT.`.
            Kind::Not(a) if matches!(a.kind, Kind::Contains(_)) => {
                format!(".NOT.({})", self.condition(a))
            }
            Kind::Not(a) => format!(".NOT. {}", self.condition(a)),
            Kind::Contains(_) => format!("{}=.T.", self.node(node)),
            _ => self.node(node),
        }
    }

    /// A join condition: of an equality, the joined table's column (in the
    /// row's columns `right`) first.
    fn join_condition(&self, node: &Node, right: (usize, usize)) -> String {
        let joined =
            |n: &Node| matches!(n.kind, Kind::Column(i) if (right.0..right.1).contains(&i));
        match &node.kind {
            Kind::Binary(BinOp::And, a, b) => {
                format!(
                    "{} .AND. {}",
                    self.join_condition(a, right),
                    self.join_condition(b, right)
                )
            }
            Kind::Binary(BinOp::Eq, a, b) if joined(b) && !joined(a) => {
                format!("{}={}", self.node(b), self.node(a))
            }
            _ => self.condition(node),
        }
    }

    fn label(&self, depth: usize, index: usize) -> String {
        let label = self.labels.get(depth).and_then(|l| l.get(index));
        let Some(label) = label else {
            return "?".to_owned();
        };
        match (self.bare, depth) {
            // `[table].[column]` as `column`.
            (true, 0) => {
                let column = label.rsplit("].[").next().unwrap_or(label);
                column
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_owned()
            }
            _ => label.clone(),
        }
    }

    /// A restriction as an AOF has it: `column op value`, with blanks
    /// around the operator if `spaced`.
    fn restriction(&self, node: &Node, spaced: bool) -> String {
        let op = |op: &str| {
            if spaced {
                format!(" {op} ")
            } else {
                op.to_owned()
            }
        };
        match &node.kind {
            Kind::Column(_) => format!("{}{}.T.", self.node(node), op("=")),
            Kind::Binary(o, a, b) => format!("{}{}{}", self.node(a), op(o.symbol()), self.node(b)),
            Kind::Between(a, low, high, _) => {
                let a = self.node(a);
                let (low, high) = (self.node(low), self.node(high));
                format!("{a}{}{low} AND {a}{}{high}", op(">="), op("<="))
            }
            _ => self.node(node),
        }
    }

    fn aggregate(&self, spec: &AggSpec) -> String {
        let arg = spec.arg.as_ref().map_or_else(String::new, |a| self.node(a));
        let distinct = if spec.distinct { "DISTINCT " } else { "" };
        let name = spec.func.name();
        let mut tail = String::new();
        if !spec.order.is_empty() {
            let keys: Vec<String> = spec
                .order
                .iter()
                .map(|(node, descending)| {
                    format!(
                        "{}{}",
                        self.node(node),
                        if *descending { " DESC" } else { "" }
                    )
                })
                .collect();
            tail.push_str(&format!(" ORDER BY {}", keys.join(", ")));
        }
        if let Some(separator) = &spec.separator {
            tail.push_str(&format!(" SEPARATOR \"{separator}\""));
        }
        format!("{name}({distinct}{arg}{tail})")
    }

    fn node(&self, node: &Node) -> String {
        match &node.kind {
            Kind::Const(V::Str(s)) => format!("\"{s}\""),
            Kind::Const(V::Bool(b)) => if *b { ".T." } else { ".F." }.to_owned(),
            Kind::Const(V::Null) => "NULL".to_owned(),
            Kind::Const(v) => v.display(),
            Kind::Column(i) => self.label(0, *i),
            Kind::RecNo(_) => "RECNO()".to_owned(),
            Kind::Deleted(_) => "DELETED()".to_owned(),
            Kind::RowId(..) => "ROWID".to_owned(),
            Kind::Outer(depth, i) => self.label(*depth, *i),
            Kind::Param(_) => "?".to_owned(),
            Kind::Neg(a) => format!("-{}", self.node(a)),
            Kind::BitNot(a) => format!("~{}", self.node(a)),
            Kind::Not(a) => format!(".NOT. {}", self.node(a)),
            Kind::Binary(op, a, b) => {
                let op = match op {
                    BinOp::And => " .AND. ",
                    BinOp::Or => " .OR. ",
                    op => op.symbol(),
                };
                format!("{}{op}{}", self.node(a), self.node(b))
            }
            Kind::IsNull(a, negated) => {
                format!(
                    "{} IS {}NULL",
                    self.node(a),
                    if *negated { "NOT " } else { "" }
                )
            }
            Kind::Between(a, low, high, negated) => {
                let a = self.node(a);
                let range = format!("{a}>={} .AND. {a}<={}", self.node(low), self.node(high));
                if *negated {
                    format!(".NOT. ({range})")
                } else {
                    range
                }
            }
            Kind::In(a, list, negated) => {
                let list: Vec<String> = list.iter().map(|n| self.node(n)).collect();
                let not = if *negated { " NOT" } else { "" };
                format!("{}{not} IN ({})", self.node(a), list.join(", "))
            }
            Kind::Like(a, pattern, _, negated) => {
                let not = if *negated { " NOT" } else { "" };
                format!("{}{not} LIKE {}", self.node(a), self.node(pattern))
            }
            Kind::Func(func, args) => {
                let args: Vec<String> = args.iter().map(|n| self.node(n)).collect();
                format!("{}({})", format!("{func:?}").to_uppercase(), args.join(","))
            }
            Kind::UserFunc(name, args) => {
                let args: Vec<String> = args.iter().map(|n| self.node(n)).collect();
                format!("{}({})", name.to_uppercase(), args.join(","))
            }
            Kind::Cast(a, function) => format!("{function}({})", self.node(a)),
            Kind::Trim(_, _, a) => format!("TRIM({})", self.node(a)),
            Kind::Case(..) => "CASE ... END".to_owned(),
            Kind::Agg(i) => match self.aggregates.get(*i) {
                Some(a) if !self.hidden => self.aggregate(a),
                _ => "( ... ) ".to_owned(),
            },
            Kind::Subquery(_) => "( SELECT ... )".to_owned(),
            Kind::Exists(_) => " EXISTS ( SELECT ... )".to_owned(),
            Kind::InSelect(a, _, negated) => {
                let op = if *negated { "<> ALL" } else { "= ANY" };
                format!("{}{op} ( SELECT ... )", self.node(a))
            }
            Kind::Quantified(op, a, _, all) => {
                let quantifier = if *all { "ALL" } else { "ANY" };
                format!(
                    "{}{} {quantifier} ( SELECT ... )",
                    self.node(a),
                    op.symbol()
                )
            }
            Kind::Contains(search) => format!("CONTAINS({})", self.fts_args(search)),
            Kind::Score(search, distinct) => {
                let name = if *distinct { "SCOREDISTINCT" } else { "SCORE" };
                format!("{name}({})", self.fts_args(search))
            }
        }
    }

    /// A `CONTAINS`/`SCORE`'s field and condition for a plan: `field,"condition"`
    /// (the field `*` for an all-fields search; the condition in single quotes
    /// if it has a double quote).
    fn fts_args(&self, search: &crate::compile::FtsSearch) -> String {
        let field = match &search.source {
            FtsSource::Fields(_) => "*".to_owned(),
            FtsSource::Expr(node) => self.node(node),
        };
        let condition = &search.condition_src;
        match condition.contains('"') {
            true => format!("{field},'{condition}'"),
            false => format!("{field},\"{condition}\""),
        }
    }

    /// An `IN` restriction as an AOF of its own: `a = 1 OR a = 2`.
    fn in_restriction(&self, node: &Node) -> String {
        let Kind::In(a, list, _) = &node.kind else {
            return self.node(node);
        };
        let a = self.node(a);
        let parts: Vec<String> = list
            .iter()
            .map(|item| format!("{a} = {}", self.node(item)))
            .collect();
        parts.join(" OR ")
    }
}
