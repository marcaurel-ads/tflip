//! Running a plan ([`crate::ops`]): each operator makes its rows from its
//! inputs', in memory. The executor decides nothing; the plan says how.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use tflip_storage::field::FieldType;
use tflip_storage::value::Value;

use crate::ast::{Aggregate, JoinKind, Top};
use crate::compile::{AggSpec, Node};
use crate::error::SqlError;
use crate::eval::{Env, Row, eval, read};
use crate::ops::{
    Equality, HashKey, IndexUse, JoinMethod, Op, Purpose, Restriction, SortBy, SortSpec, SubPlan,
};
use crate::query::{Output, Tables};
use crate::types::Ty;
use crate::value::{Collation, V, order, order_compared, order_indexed};

/// A row between operators: its values and, above a group, the group's
/// aggregates; below one, the records its values were read from.
#[derive(Debug, Clone)]
struct Tuple {
    values: Vec<V>,
    aggregates: Rc<[V]>,
    records: Vec<Placed>,
}

/// A record whose values a row holds: the places it fills, and its number.
#[derive(Debug, Clone, Copy)]
struct Placed {
    start: usize,
    end: usize,
    number: u32,
    /// Whether the record is marked deleted.
    deleted: bool,
}

impl Tuple {
    fn new(values: Vec<V>) -> Tuple {
        Tuple {
            values,
            aggregates: Rc::from([]),
            records: Vec::new(),
        }
    }

    /// The row of a table's record `(number, values)` placed at `start` of a
    /// row `width` values wide.
    fn placed((number, values, deleted): &Record, start: usize, width: usize) -> Tuple {
        let records = number
            .map(|number| Placed {
                start,
                end: start + values.len(),
                number,
                deleted: *deleted,
            })
            .into_iter()
            .collect();
        Tuple {
            records,
            ..Tuple::new(place(values, start, width))
        }
    }
}

/// A table's record: its number, if known, whether it is marked deleted, and
/// a value per field.
type Record = (Option<u32>, Values, bool);

/// A record's values, a value per field.
type Values = Rc<Vec<V>>;

/// Runs plans on the tables of a statement.
pub(crate) struct Exec<'a> {
    pub tables: &'a dyn Tables,
    pub env: &'a Env,
    /// `env` as the expression engine that filters a table's records has it.
    engine: Env,
    pub subqueries: &'a [SubPlan],
    /// Each table's records, read once.
    records: RefCell<HashMap<usize, Rc<Vec<Record>>>>,
    /// The records looked up in indexes, by table and number, read once.
    fetched: RefCell<HashMap<(usize, u32), (Values, bool)>>,
    /// What `ROWNUM()` is now: how many rows the `SELECT` running has added,
    /// plus one.
    rownum: Cell<u32>,
    /// The rows of subqueries that read no rows around them.
    cache: RefCell<HashMap<usize, Rc<Vec<Vec<V>>>>>,
}

/// A row expressions are evaluated on: its values, its group's aggregates,
/// and the rows of the `SELECT`s around.
struct Frame<'f> {
    values: &'f [V],
    aggregates: &'f [V],
    records: &'f [Placed],
    parent: Option<&'f Frame<'f>>,
    exec: &'f Exec<'f>,
}

impl Row for Frame<'_> {
    fn value(&mut self, _index: usize) -> Result<Value, SqlError> {
        Ok(Value::Null)
    }

    fn record(&mut self, index: usize) -> Option<u32> {
        let placed = self
            .records
            .iter()
            .find(|p| (p.start..p.end).contains(&index))?;
        Some(placed.number)
    }

    fn deleted(&mut self, index: usize) -> bool {
        self.records
            .iter()
            .find(|p| (p.start..p.end).contains(&index))
            .is_some_and(|placed| placed.deleted)
    }

    fn rownum(&mut self) -> u32 {
        self.exec.rownum.get()
    }

    fn column(&mut self, index: usize, _ty: Ty) -> Result<V, SqlError> {
        Ok(self.values.get(index).cloned().unwrap_or(V::Null))
    }

    fn outer(&mut self, depth: usize, index: usize) -> Result<V, SqlError> {
        let mut frame: &Frame<'_> = self;
        for _ in 0..depth {
            match frame.parent {
                Some(parent) => frame = parent,
                None => return Ok(V::Null),
            }
        }
        Ok(frame.values.get(index).cloned().unwrap_or(V::Null))
    }

    fn aggregate(&mut self, index: usize) -> Result<V, SqlError> {
        Ok(self.aggregates.get(index).cloned().unwrap_or(V::Null))
    }

    fn subquery(&mut self, id: usize) -> Result<Rc<Vec<Vec<V>>>, SqlError> {
        let exec = self.exec;
        let plan = &exec.subqueries[id];
        if !plan.correlated
            && let Some(rows) = exec.cache.borrow().get(&id)
        {
            return Ok(Rc::clone(rows));
        }
        let rows: Vec<Vec<V>> = exec
            .run(&plan.op, Some(self))?
            .into_iter()
            .map(|t| t.values)
            .collect();
        let rows = Rc::new(rows);
        if !plan.correlated {
            exec.cache.borrow_mut().insert(id, Rc::clone(&rows));
        }
        Ok(rows)
    }
}

impl<'a> Exec<'a> {
    pub(crate) fn new(tables: &'a dyn Tables, env: &'a Env, subqueries: &'a [SubPlan]) -> Exec<'a> {
        Exec {
            tables,
            env,
            engine: Env {
                engine: true,
                ..env.clone()
            },
            subqueries,
            records: RefCell::new(HashMap::new()),
            fetched: RefCell::new(HashMap::new()),
            rownum: Cell::new(1),
            cache: RefCell::new(HashMap::new()),
        }
    }

    /// Forgets the records read, so that tables are read again.
    pub(crate) fn forget_records(&self) {
        self.records.borrow_mut().clear();
        self.fetched.borrow_mut().clear();
    }

    fn eval(&self, node: &Node, tuple: &Tuple, parent: Option<&Frame<'_>>) -> Result<V, SqlError> {
        let mut frame = Frame {
            values: &tuple.values,
            aggregates: &tuple.aggregates,
            records: &tuple.records,
            parent,
            exec: self,
        };
        eval(node, &mut frame, self.env)
    }

    /// As [`Exec::eval`], run as the original's expression engine runs a
    /// filter on a table's records: see [`Env::engine`].
    fn eval_engine(
        &self,
        node: &Node,
        tuple: &Tuple,
        parent: Option<&Frame<'_>>,
    ) -> Result<V, SqlError> {
        let mut frame = Frame {
            values: &tuple.values,
            aggregates: &tuple.aggregates,
            records: &tuple.records,
            parent,
            exec: self,
        };
        crate::eval::eval_filter(node, &mut frame, &self.engine)
    }

    /// As [`Exec::eval`], but a column as it is in the record: a blank
    /// field stays [`V::Blank`] — shown as a result's column, and a group of
    /// its own.
    fn read(&self, node: &Node, tuple: &Tuple, parent: Option<&Frame<'_>>) -> Result<V, SqlError> {
        let mut frame = Frame {
            values: &tuple.values,
            aggregates: &tuple.aggregates,
            records: &tuple.records,
            parent,
            exec: self,
        };
        read(node, &mut frame, self.env)
    }

    /// As [`Exec::eval_values`], run as the original's expression engine
    /// runs a filter (see [`Exec::eval_engine`]).
    pub(crate) fn eval_engine_values(
        &self,
        node: &Node,
        values: &[V],
        record: Option<u32>,
    ) -> Result<V, SqlError> {
        let records: Vec<Placed> = record
            .map(|number| Placed {
                start: 0,
                end: values.len(),
                number,
                deleted: false,
            })
            .into_iter()
            .collect();
        let mut frame = Frame {
            values,
            aggregates: &[],
            records: &records,
            parent: None,
            exec: self,
        };
        crate::eval::eval_filter(node, &mut frame, &self.engine)
    }

    /// The value of `node` on a row of `values` (with no row around it),
    /// the values of the record `record` of a table when it has one.
    pub(crate) fn eval_values(
        &self,
        node: &Node,
        values: &[V],
        record: Option<u32>,
    ) -> Result<V, SqlError> {
        let records: Vec<Placed> = record
            .map(|number| Placed {
                start: 0,
                end: values.len(),
                number,
                deleted: false,
            })
            .into_iter()
            .collect();
        let mut frame = Frame {
            values,
            aggregates: &[],
            records: &records,
            parent: None,
            exec: self,
        };
        eval(node, &mut frame, self.env)
    }

    /// Runs the plan of a statement: its rows, and the order to show them in
    /// if they stay as they are.
    pub(crate) fn output(&self, op: &Op) -> Result<Output, SqlError> {
        self.output_with(op, None)
    }

    /// As [`Exec::output`] for a plan that reads the row `outer` of a
    /// statement around it.
    pub(crate) fn output_in(&self, op: &Op, outer: &[V]) -> Result<Output, SqlError> {
        let frame = Frame {
            values: outer,
            aggregates: &[],
            records: &[],
            parent: None,
            exec: self,
        };
        self.output_with(op, Some(&frame))
    }

    fn output_with(&self, op: &Op, parent: Option<&Frame<'_>>) -> Result<Output, SqlError> {
        if let Op::Final { input, columns } = op
            && let Op::Sort {
                input: sorted,
                keys,
                purpose: Purpose::Shown,
                ..
            } = &**input
        {
            let tuples = self.run(sorted, parent)?;
            let indexes = sorted_indexes(
                &tuples,
                keys,
                true,
                |t, k| self.key(t, k, parent),
                self.env.collation.as_deref(),
            )?;
            let rows = tuples
                .into_iter()
                .map(|t| cut_to(t.values, *columns))
                .collect();
            let sequence = indexes.iter().map(|&i| i as u32 + 1).collect();
            return Ok(Output {
                rows,
                sequence: Some(sequence),
            });
        }
        let rows = self
            .run(op, parent)?
            .into_iter()
            .map(|t| t.values)
            .collect();
        Ok(Output {
            rows,
            sequence: None,
        })
    }

    /// The rows of `op`, run for the row `parent` of the `SELECT` around.
    fn run(&self, op: &Op, parent: Option<&Frame<'_>>) -> Result<Vec<Tuple>, SqlError> {
        Ok(match op {
            Op::Scan {
                table,
                start,
                width,
                restrictions,
                engine,
                ..
            } => {
                let tuples: Vec<Tuple> = match self.found(*table, restrictions, parent)? {
                    Some(records) => self
                        .fetch(*table, &records)?
                        .iter()
                        .map(|record| Tuple::placed(record, *start, *width))
                        .collect(),
                    None => self
                        .records(*table)?
                        .iter()
                        .map(|record| Tuple::placed(record, *start, *width))
                        .collect(),
                };
                let mut kept = Vec::with_capacity(tuples.len());
                for tuple in tuples {
                    for part in engine {
                        self.eval_engine(part, &tuple, parent)?;
                    }
                    if self.holds(restrictions, &tuple, parent)? {
                        kept.push(tuple);
                    }
                }
                // A lone `>=` or `<=` gives its rows ordered by its column
                // with no order to read them in too, equal values in record
                // order (`fixtures/sql/indexsel2.json`); by the fields of an
                // order that starts with it (`indexsel.json`).
                if let [restriction, ..] = &restrictions[..]
                    && restrictions.iter().all(|r| r.sole)
                    && (restrictions.len() > 1 || restriction.index.is_none() || !restriction.finds)
                {
                    let collation = self.env.collation.as_deref();
                    kept.sort_by(|a, b| {
                        restriction
                            .sort
                            .iter()
                            .map(|&(column, ci)| {
                                order_indexed(&a.values[column], &b.values[column], ci, collation)
                            })
                            .find(|o| *o != Ordering::Equal)
                            .unwrap_or(Ordering::Equal)
                    });
                }
                kept
            }
            Op::Derived {
                input,
                start,
                width,
                ..
            } => self
                .run(input, None)?
                .into_iter()
                .enumerate()
                .map(|(i, t)| {
                    // The rows of a derived table are numbered as records.
                    let mut tuple = Tuple::new(place(&t.values, *start, *width));
                    tuple.records.push(Placed {
                        start: *start,
                        end: start + width,
                        number: i as u32 + 1,
                        deleted: false,
                    });
                    tuple
                })
                .collect(),
            Op::Join {
                kind,
                method: JoinMethod::Index { index, key },
                left,
                right,
                right_range,
                condition,
                ..
            } => {
                let left = self.run(left, parent)?;
                let join = IndexJoin {
                    index,
                    key,
                    range: *right_range,
                    condition: condition.as_ref(),
                };
                self.index_join(*kind, &join, left, right, parent)?
            }
            Op::Join {
                kind,
                method,
                left,
                right,
                right_range,
                condition,
                ..
            } => {
                let left = self.run(left, parent)?;
                let right = self.run(right, parent)?;
                self.join(
                    *kind,
                    method,
                    left,
                    right,
                    *right_range,
                    condition.as_ref(),
                    parent,
                )?
            }
            Op::Filter {
                input, condition, ..
            } => {
                let mut kept = Vec::new();
                for tuple in self.run(input, parent)? {
                    self.rownum.set(kept.len() as u32 + 1);
                    if self.eval(condition, &tuple, parent)? == V::Bool(true) {
                        kept.push(tuple);
                    }
                }
                kept
            }
            Op::Sort {
                input,
                keys,
                purpose,
                ..
            } => {
                let tuples = self.run(input, parent)?;
                let indexed = matches!(purpose, Purpose::Indexed | Purpose::Shown);
                let indexes = sorted_indexes(
                    &tuples,
                    keys,
                    indexed,
                    |t, k| self.key(t, k, parent),
                    self.env.collation.as_deref(),
                )?;
                let mut taken: Vec<Option<Tuple>> = tuples.into_iter().map(Some).collect();
                indexes.iter().filter_map(|&i| taken[i].take()).collect()
            }
            Op::Group {
                input,
                keys,
                aggregates,
                width,
                ..
            } => {
                let tuples = self.run(input, parent)?;
                self.group(tuples, keys, aggregates, *width, parent)?
            }
            Op::Project {
                input,
                columns,
                keys,
                ..
            } => {
                let mut out = Vec::new();
                for tuple in self.run(input, parent)? {
                    self.rownum.set(out.len() as u32 + 1);
                    let mut values = Vec::with_capacity(columns.len() + keys.len());
                    for node in columns {
                        values.push(self.read(node, &tuple, parent)?);
                    }
                    // A sort key sees no row added: they all compare equal.
                    self.rownum.set(0);
                    for node in keys {
                        values.push(self.eval(node, &tuple, parent)?);
                    }
                    out.push(Tuple::new(values));
                }
                out
            }
            Op::Distinct {
                input,
                columns,
                keep_last,
                ..
            } => {
                let ci: Vec<bool> = columns.iter().map(|c| c.ty.case_insensitive()).collect();
                let collation = self.env.collation.as_deref();
                // Under a dynamic collation, where different text can be
                // equal, the first of equal rows stays.
                let keep_last = *keep_last && collation.and_then(Collation::dynamic).is_none();
                distinct(self.run(input, parent)?, &ci, keep_last, collation)
            }
            Op::Top { input, top } => match self.top_of_projection(input, *top, parent)? {
                Some(tuples) => tuples,
                None => cut(self.run(input, parent)?, *top),
            },
            Op::Concat { inputs } => {
                let mut out = Vec::new();
                for input in inputs {
                    out.extend(self.run(input, parent)?);
                }
                out
            }
            Op::Fit { input, types, trim } => {
                let mut tuples = self.run(input, parent)?;
                for tuple in &mut tuples {
                    for (v, ty) in tuple.values.iter_mut().zip(types) {
                        // A `UNION` shows a blank field's blank value.
                        *v = std::mem::replace(v, V::Null).unblank();
                        fit(v, *ty, *trim)?;
                    }
                }
                tuples
            }
            Op::Final { input, columns } => self
                .run(input, parent)?
                .into_iter()
                .map(|t| Tuple::new(cut_to(t.values, *columns)))
                .collect(),
        })
    }

    /// `TOP n` (not a percentage) of a projection, maybe sorted: as in the
    /// original engine, the select list is worked out only for the rows
    /// kept, after the sort keys of every row (a key that is a column of the
    /// select list works that column out for every row, once). It shows with
    /// `RAND()` (`fixtures/sql/funcs.json`). `None` for any other `TOP`.
    fn top_of_projection(
        &self,
        op: &Op,
        top: Top,
        parent: Option<&Frame<'_>>,
    ) -> Result<Option<Vec<Tuple>>, SqlError> {
        let (project, sort) = match op {
            Op::Sort {
                input,
                keys,
                purpose,
                ..
            } => (&**input, Some((keys, *purpose))),
            other => (other, None),
        };
        let Op::Project {
            input,
            columns,
            keys,
            ..
        } = project
        else {
            return Ok(None);
        };
        if top.percent {
            return Ok(None);
        }
        let n = columns.len();
        // The columns the keys need: those a key is.
        let eager: Vec<bool> = columns
            .iter()
            .map(|c| keys.iter().any(|k| crate::query::same(c, k)))
            .collect();
        let mut rows: Vec<(Tuple, Tuple)> = Vec::new();
        for tuple in self.run(input, parent)? {
            let mut values = vec![V::Null; n + keys.len()];
            for (i, node) in columns.iter().enumerate() {
                if eager[i] {
                    values[i] = self.read(node, &tuple, parent)?;
                }
            }
            for (i, node) in keys.iter().enumerate() {
                values[n + i] = match columns.iter().position(|c| crate::query::same(c, node)) {
                    Some(j) => values[j].clone().unblank(),
                    None => self.eval(node, &tuple, parent)?,
                };
            }
            rows.push((Tuple::new(values), tuple));
        }
        if let Some((keys, purpose)) = sort {
            let projected: Vec<Tuple> = rows.iter().map(|(p, _)| p.clone()).collect();
            let indexed = matches!(purpose, Purpose::Indexed | Purpose::Shown);
            let indexes = sorted_indexes(
                &projected,
                keys,
                indexed,
                |t, k| self.key(t, k, parent),
                self.env.collation.as_deref(),
            )?;
            let mut taken: Vec<Option<(Tuple, Tuple)>> = rows.into_iter().map(Some).collect();
            rows = indexes.iter().filter_map(|&i| taken[i].take()).collect();
        }
        let mut out = Vec::new();
        for (mut projected, tuple) in cut(rows, top) {
            self.rownum.set(out.len() as u32 + 1);
            for (i, node) in columns.iter().enumerate() {
                if !eager[i] {
                    projected.values[i] = self.read(node, &tuple, parent)?;
                }
            }
            out.push(projected);
        }
        Ok(Some(out))
    }

    /// A sort key's value for `tuple`.
    fn key(
        &self,
        tuple: &Tuple,
        spec: &SortSpec,
        parent: Option<&Frame<'_>>,
    ) -> Result<V, SqlError> {
        match &spec.by {
            SortBy::Node(node) => self.read(node, tuple, parent),
            SortBy::Place(i) => Ok(tuple.values.get(*i).cloned().unwrap_or(V::Null)),
        }
    }

    /// The records of table `id`, each a value per field.
    fn records(&self, id: usize) -> Result<Rc<Vec<Record>>, SqlError> {
        if let Some(rows) = self.records.borrow().get(&id) {
            return Ok(Rc::clone(rows));
        }
        let types: Vec<Ty> = self.tables.fields(id).iter().map(Ty::of_field).collect();
        let mut rows = Vec::new();
        self.tables.scan(id, &mut |row| {
            let mut values = Vec::with_capacity(types.len());
            for (i, ty) in types.iter().enumerate() {
                values.push(row.column(i, *ty)?);
            }
            rows.push((row.record(0), Rc::new(values), row.deleted(0)));
            Ok(())
        })?;
        let rows = Rc::new(rows);
        self.records.borrow_mut().insert(id, Rc::clone(&rows));
        Ok(rows)
    }

    /// Joins `left` and `right` rows: each left row with the right ones
    /// `method` picks and `condition` holds for, in the order of the right
    /// rows; then, for `RIGHT` and `FULL`, the right rows nothing matched.
    #[allow(clippy::too_many_arguments)]
    fn join(
        &self,
        kind: JoinKind,
        method: &JoinMethod,
        left: Vec<Tuple>,
        right: Vec<Tuple>,
        (start, end): (usize, usize),
        condition: Option<&Node>,
        parent: Option<&Frame<'_>>,
    ) -> Result<Vec<Tuple>, SqlError> {
        let collation = self.env.collation.as_deref();
        let index: Option<HashMap<Vec<HashKey>, Vec<usize>>> = match method {
            JoinMethod::Loop | JoinMethod::Index { .. } => None,
            JoinMethod::Hash(keys) => {
                let mut index: HashMap<Vec<HashKey>, Vec<usize>> = HashMap::new();
                for (j, r) in right.iter().enumerate() {
                    let key: Option<Vec<HashKey>> = keys
                        .iter()
                        .map(|k| k.key(&r.values[k.right], collation))
                        .collect();
                    if let Some(key) = key {
                        index.entry(key).or_default().push(j);
                    }
                }
                Some(index)
            }
        };
        let all: Vec<usize> = (0..right.len()).collect();
        let mut matched = vec![false; right.len()];
        let mut rows = Vec::new();
        for l in &left {
            let candidates: &[usize] = match (&index, method) {
                (Some(index), JoinMethod::Hash(keys)) => {
                    let key: Option<Vec<HashKey>> = keys
                        .iter()
                        .map(|k| k.key(&l.values[k.left], collation))
                        .collect();
                    key.and_then(|key| index.get(&key))
                        .map_or(&[], Vec::as_slice)
                }
                _ => &all,
            };
            let mut any = false;
            for &j in candidates {
                let mut values = l.values.clone();
                values[start..end].clone_from_slice(&right[j].values[start..end]);
                let records = [l.records.as_slice(), &right[j].records].concat();
                let row = Tuple {
                    records,
                    ..Tuple::new(values)
                };
                let keep = match condition {
                    Some(condition) => self.eval(condition, &row, parent)? == V::Bool(true),
                    None => true,
                };
                if keep {
                    any = true;
                    matched[j] = true;
                    rows.push(row);
                }
            }
            if !any && matches!(kind, JoinKind::Left | JoinKind::Full) {
                rows.push(l.clone());
            }
        }
        if matches!(kind, JoinKind::Right | JoinKind::Full) {
            for (r, matched) in right.into_iter().zip(matched) {
                if !matched {
                    rows.push(r);
                }
            }
        }
        Ok(rows)
    }

    /// Joins `left` rows with the records of the scan `right` that `join`
    /// looks up for each, in record order; then, for a `LEFT` join, the left
    /// rows nothing matched.
    fn index_join(
        &self,
        kind: JoinKind,
        join: &IndexJoin<'_>,
        left: Vec<Tuple>,
        right: &Op,
        parent: Option<&Frame<'_>>,
    ) -> Result<Vec<Tuple>, SqlError> {
        let Op::Scan {
            table,
            start,
            width,
            restrictions,
            ..
        } = right
        else {
            unreachable!("an index join's right side is a scan")
        };
        let (from, to) = join.range;
        let collation = self.env.collation.as_deref();
        let mut rows = Vec::new();
        for l in left {
            let mut any = false;
            let value = &l.values[join.key.left];
            if let Some(wanted) = join.key.key(value, collation) {
                let mut records =
                    self.tables
                        .lookup(*table, join.index.number, Some(value), Some(value))?;
                records.sort_unstable();
                records.dedup();
                for record in self.fetch(*table, &records)? {
                    let r = Tuple::placed(&record, *start, *width);
                    if join.key.key(&r.values[join.key.right], collation).as_ref() != Some(&wanted)
                        || !self.holds(restrictions, &r, parent)?
                    {
                        continue;
                    }
                    let mut values = l.values.clone();
                    values[from..to].clone_from_slice(&r.values[from..to]);
                    let records = [l.records.as_slice(), &r.records].concat();
                    let row = Tuple {
                        records,
                        ..Tuple::new(values)
                    };
                    let keep = match join.condition {
                        Some(condition) => self.eval(condition, &row, parent)? == V::Bool(true),
                        None => true,
                    };
                    if keep {
                        any = true;
                        rows.push(row);
                    }
                }
            }
            if !any && kind == JoinKind::Left {
                rows.push(l);
            }
        }
        Ok(rows)
    }

    /// The records of `table` that the `restrictions` with an index can hold
    /// for: those in their ranges in each one's index. In record order, but
    /// in the order's for a scope that is the only restriction. `None` if
    /// none has an index.
    fn found(
        &self,
        table: usize,
        restrictions: &[Restriction],
        parent: Option<&Frame<'_>>,
    ) -> Result<Option<Vec<u32>>, SqlError> {
        let mut found: Option<Vec<u32>> = None;
        for restriction in restrictions {
            let Some(index) = &restriction.index else {
                continue;
            };
            if !restriction.finds {
                continue;
            }
            let mut records = Vec::new();
            'ranges: for (low, high) in &restriction.ranges {
                let mut bounds = [None, None];
                for (bound, node) in bounds.iter_mut().zip([low, high]) {
                    if let Some(node) = node {
                        // Nothing compares with NULL.
                        match self.eval(node, &Tuple::new(Vec::new()), parent)? {
                            V::Null => continue 'ranges,
                            v => *bound = Some(v),
                        }
                    }
                }
                let [low, high] = &bounds;
                records.extend(self.tables.lookup(
                    table,
                    index.number,
                    low.as_ref(),
                    high.as_ref(),
                )?);
            }
            if restriction.scope && restriction.ordered && restrictions.len() == 1 {
                return Ok(Some(records));
            }
            records.sort_unstable();
            records.dedup();
            found = Some(match found {
                Some(before) => before
                    .into_iter()
                    .filter(|r| records.binary_search(r).is_ok())
                    .collect(),
                None => records,
            });
        }
        Ok(found)
    }

    /// How many rows `op` makes, run on its own (not for a row around it).
    pub(crate) fn count(&self, op: &Op) -> Result<usize, SqlError> {
        Ok(self.run(op, None)?.len())
    }

    /// The records of `table` (placed in rows `width` values wide from
    /// `start` on) `restriction` holds for, if it has an index to find them
    /// with, as the original engine's plans count them.
    pub(crate) fn matching(
        &self,
        table: usize,
        (start, width): (usize, usize),
        restriction: &Restriction,
    ) -> Result<Option<Vec<u32>>, SqlError> {
        let Some(records) = self.found(table, std::slice::from_ref(restriction), None)? else {
            return Ok(None);
        };
        let mut held = Vec::new();
        for record in self.fetch(table, &records)? {
            let tuple = Tuple::placed(&record, start, width);
            if self.holds(std::slice::from_ref(restriction), &tuple, None)?
                && let Some(number) = record.0
            {
                held.push(number);
            }
        }
        Ok(Some(held))
    }

    /// Whether every one of `restrictions` holds for `tuple`.
    fn holds(
        &self,
        restrictions: &[Restriction],
        tuple: &Tuple,
        parent: Option<&Frame<'_>>,
    ) -> Result<bool, SqlError> {
        for restriction in restrictions {
            if self.eval(&restriction.condition, tuple, parent)? != V::Bool(true) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The `records` of table `id` that it has, each a value per field, in
    /// that order.
    fn fetch(&self, id: usize, records: &[u32]) -> Result<Vec<Record>, SqlError> {
        let missing: Vec<u32> = {
            let fetched = self.fetched.borrow();
            records
                .iter()
                .copied()
                .filter(|r| !fetched.contains_key(&(id, *r)))
                .collect()
        };
        if !missing.is_empty() {
            let types: Vec<Ty> = self.tables.fields(id).iter().map(Ty::of_field).collect();
            let mut read = Vec::with_capacity(missing.len());
            self.tables.fetch(id, &missing, &mut |number, row| {
                let mut values = Vec::with_capacity(types.len());
                for (i, ty) in types.iter().enumerate() {
                    values.push(row.column(i, *ty)?);
                }
                read.push((number, Rc::new(values), row.deleted(0)));
                Ok(())
            })?;
            self.fetched
                .borrow_mut()
                .extend(read.into_iter().map(|(n, v, d)| ((id, n), (v, d))));
        }
        let fetched = self.fetched.borrow();
        Ok(records
            .iter()
            .filter_map(|r| {
                let (values, deleted) = fetched.get(&(id, *r))?.clone();
                Some((Some(*r), values, deleted))
            })
            .collect())
    }

    /// A row of each run of rows with equal `keys`: its first row (NULLs
    /// for the one group of no rows) and `aggregates` over the run.
    fn group(
        &self,
        tuples: Vec<Tuple>,
        keys: &[Node],
        aggregates: &[AggSpec],
        width: usize,
        parent: Option<&Frame<'_>>,
    ) -> Result<Vec<Tuple>, SqlError> {
        // An aggregate of `ROWNUM()` sees no row added yet
        // (`fixtures/sql/rowfuncs.json`).
        self.rownum.set(0);
        let mut runs: Vec<(usize, usize)> = Vec::new();
        if keys.is_empty() {
            runs.push((0, tuples.len()));
        } else {
            let ci: Vec<bool> = keys.iter().map(|k| k.ty.case_insensitive()).collect();
            let collation = self.env.collation.as_deref();
            let mut previous: Option<Vec<V>> = None;
            for (i, tuple) in tuples.iter().enumerate() {
                let mut key = Vec::with_capacity(keys.len());
                for node in keys {
                    key.push(self.read(node, tuple, parent)?);
                }
                match &previous {
                    Some(p) if compare_values(p, &key, &ci, collation) == Ordering::Equal => {
                        runs.last_mut().expect("a run").1 = i + 1;
                    }
                    _ => {
                        runs.push((i, i + 1));
                        previous = Some(key);
                    }
                }
            }
        }
        let mut out = Vec::with_capacity(runs.len());
        for (from, to) in runs {
            let members = &tuples[from..to];
            let values = members
                .first()
                .map_or_else(|| vec![V::Null; width], |t| t.values.clone());
            let computed = self.aggregates(aggregates, members, parent)?;
            out.push(Tuple {
                aggregates: Rc::from(computed),
                ..Tuple::new(values)
            });
        }
        Ok(out)
    }

    /// The `aggregates` of the rows `members`.
    fn aggregates(
        &self,
        aggregates: &[AggSpec],
        members: &[Tuple],
        parent: Option<&Frame<'_>>,
    ) -> Result<Vec<V>, SqlError> {
        let mut out = Vec::with_capacity(aggregates.len());
        for spec in aggregates {
            let Some(arg) = &spec.arg else {
                out.push(V::Int(members.len() as i64));
                continue;
            };
            let mut values = Vec::with_capacity(members.len());
            for member in members {
                let v = self.eval(arg, member, parent)?;
                if !v.is_null() {
                    values.push(v);
                }
            }
            let ci = arg.ty.case_insensitive();
            let collation = self.env.collation.as_deref();
            out.push(match spec.func {
                Aggregate::Count if spec.distinct => {
                    values.sort_by(|a, b| order(a, b, ci, collation));
                    values.dedup_by(|a, b| order(a, b, ci, collation) == Ordering::Equal);
                    V::Int(values.len() as i64)
                }
                Aggregate::Count => V::Int(values.len() as i64),
                Aggregate::GroupConcat => self.group_concat(spec, members, values, parent)?,
                Aggregate::Sum => sum(&values, spec.ty, false)?,
                Aggregate::Avg => sum(&values, spec.ty, true)?,
                Aggregate::Min | Aggregate::Max => {
                    // Text compares with its case, even CICHARACTER.
                    let keep = if spec.func == Aggregate::Min {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    };
                    // Of equal values, the last counts; under a dynamic
                    // collation, where different text can be equal, the
                    // first.
                    let first = collation.and_then(Collation::dynamic).is_some();
                    let mut best: Option<V> = None;
                    for v in values {
                        if best.as_ref().is_none_or(|b| {
                            let o = order_compared(&v, b, false, collation);
                            o != keep && !(first && o == Ordering::Equal)
                        }) {
                            best = Some(v);
                        }
                    }
                    best.unwrap_or(V::Null)
                }
            });
        }
        Ok(out)
    }

    /// `GROUP_CONCAT` of the group's `members`: its non-NULL text `values`
    /// (already in record order) joined by the separator, after applying the
    /// inner `ORDER BY` and `DISTINCT`. An empty group is NULL; a result past
    /// the 1024-byte column raises "Data truncated", as the engine does.
    fn group_concat(
        &self,
        spec: &AggSpec,
        members: &[Tuple],
        values: Vec<V>,
        parent: Option<&Frame<'_>>,
    ) -> Result<V, SqlError> {
        let ci = spec.arg.as_ref().is_some_and(|a| a.ty.case_insensitive());
        let collation = self.env.collation.as_deref();
        // The values in the order the engine concatenates them: record order,
        // or the inner ORDER BY (a stable sort keeps record order among ties).
        let mut items = if spec.order.is_empty() {
            values
        } else {
            let arg = spec.arg.as_ref().expect("GROUP_CONCAT has an argument");
            let mut rows: Vec<(Vec<V>, V)> = Vec::new();
            for member in members {
                let value = self.eval(arg, member, parent)?;
                if value.is_null() {
                    continue;
                }
                let mut keys = Vec::with_capacity(spec.order.len());
                for (node, _) in &spec.order {
                    keys.push(self.eval(node, member, parent)?);
                }
                rows.push((keys, value));
            }
            // Case-insensitive keys compare regardless of case; equal keys
            // keep record order, reversed when the last key descends (as the
            // engine's temporary index gives them).
            let last_descending = spec.order.last().is_some_and(|(_, d)| *d);
            let mut indexes: Vec<usize> = (0..rows.len()).collect();
            indexes.sort_by(|&a, &b| {
                for (i, (node, descending)) in spec.order.iter().enumerate() {
                    let ci = node.ty.case_insensitive();
                    let mut ordering = order(&rows[a].0[i], &rows[b].0[i], ci, collation);
                    if *descending {
                        ordering = ordering.reverse();
                    }
                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                }
                if last_descending {
                    b.cmp(&a)
                } else {
                    a.cmp(&b)
                }
            });
            indexes.into_iter().map(|i| rows[i].1.clone()).collect()
        };
        if spec.distinct {
            // Keep the first occurrence of each value (after ordering).
            let mut seen: Vec<V> = Vec::new();
            items.retain(|value| {
                if seen
                    .iter()
                    .any(|s| order(s, value, ci, collation) == Ordering::Equal)
                {
                    false
                } else {
                    seen.push(value.clone());
                    true
                }
            });
        }
        if items.is_empty() {
            return Ok(V::Null);
        }
        let separator = spec.separator.as_deref().unwrap_or(",");
        let mut joined = String::new();
        for (i, value) in items.iter().enumerate() {
            if i > 0 {
                joined.push_str(separator);
            }
            joined.push_str(&value.display());
        }
        if tflip_storage::cp1252::encode(&joined).len() > 1024 {
            return Err(SqlError::data_truncated());
        }
        Ok(V::Str(joined))
    }
}

/// The order of `tuples` sorted on `keys` (`key` gives a key's value). As
/// a temporary index sorts (`indexed`), by [`order_indexed`], walked
/// backwards if every key is descending (the others are `Descend()`ed): NULL
/// text first in it whichever way its own key goes, and equal keys in the
/// order of the rows; else equal keys keep it.
fn sorted_indexes(
    tuples: &[Tuple],
    keys: &[SortSpec],
    indexed: bool,
    key: impl Fn(&Tuple, &SortSpec) -> Result<V, SqlError>,
    collation: Option<&Collation>,
) -> Result<Vec<usize>, SqlError> {
    let mut values = Vec::with_capacity(tuples.len());
    for tuple in tuples {
        let mut row = Vec::with_capacity(keys.len());
        for spec in keys {
            row.push(key(tuple, spec)?);
        }
        values.push(row);
    }
    let last_descending = indexed && keys.iter().all(|k| k.descending);
    let mut indexes: Vec<usize> = (0..tuples.len()).collect();
    indexes.sort_by(|&a, &b| {
        keys.iter()
            .enumerate()
            .map(|(i, k)| {
                let (x, y) = (&values[a][i], &values[b][i]);
                let o = match indexed {
                    true => order_indexed(x, y, k.ci, collation),
                    false => order(x, y, k.ci, collation),
                };
                let null_text = indexed && k.text && (x.is_null() || y.is_null());
                let reversed = if null_text {
                    last_descending
                } else {
                    k.descending
                };
                if reversed { o.reverse() } else { o }
            })
            .find(|o| *o != Ordering::Equal)
            .unwrap_or_else(|| {
                if last_descending {
                    b.cmp(&a)
                } else {
                    a.cmp(&b)
                }
            })
    });
    Ok(indexes)
}

/// How an index join finds the right rows: in `index`, by `key`; the
/// right side's columns are `range` of the row, and `condition` is the
/// join's.
struct IndexJoin<'a> {
    index: &'a IndexUse,
    key: &'a Equality,
    range: (usize, usize),
    condition: Option<&'a Node>,
}

/// `values` cut to its first `n`.
fn cut_to(mut values: Vec<V>, n: usize) -> Vec<V> {
    values.truncate(n);
    values
}

/// Cuts `rows` to `TOP`. A percentage is of the rows, rounded up (exactly:
/// 7% of 100 rows is 7).
fn cut<T>(rows: Vec<T>, top: Top) -> Vec<T> {
    let skip = usize::try_from(top.start.saturating_sub(1)).unwrap_or(usize::MAX);
    let take = match top.percent {
        true => (rows.len() as u128 * u128::from(top.count)).div_ceil(100),
        false => u128::from(top.count),
    };
    let take = usize::try_from(take).unwrap_or(usize::MAX);
    rows.into_iter().skip(skip).take(take).collect()
}

/// Fits a `UNION`'s value to its column's type `ty`.
fn fit(v: &mut V, ty: Ty, trim: bool) -> Result<(), SqlError> {
    // A logical that a `UNION` made an integer column's value.
    if let (V::Bool(b), Ty::Int { .. }) = (&*v, ty) {
        *v = V::Int(i64::from(*b));
    }
    if let Ty::Num { precision, scale } = ty
        && v.as_scaled(scale)
            .is_some_and(|n| n.unsigned_abs() >= 10u128.pow(u32::from(precision)))
    {
        return Err(SqlError::numeric_overflow());
    }
    if trim
        && let (
            V::Str(s),
            Ty::Str {
                field: FieldType::VarChar,
                ..
            },
        ) = (&mut *v, ty)
    {
        s.truncate(s.trim_end_matches(' ').len());
    }
    Ok(())
}

/// The `SUM` (or with `average`, the `AVG`) of non-NULL `values`, of type
/// `ty`; NULL if there are none.
fn sum(values: &[V], ty: Ty, average: bool) -> Result<V, SqlError> {
    if values.is_empty() {
        return Ok(V::Null);
    }
    let n = values.len() as i128;
    let exact = |scale: u16| -> Result<i128, SqlError> {
        values.iter().try_fold(0i128, |total, v| {
            v.as_scaled(scale)
                .and_then(|x| total.checked_add(x))
                .ok_or_else(SqlError::out_of_range)
        })
    };
    Ok(match ty {
        Ty::Int { .. } => {
            let total = exact(0)?;
            // As the original engine does it: an average rounded half to
            // even, but the sum for a negative one.
            let value = match average {
                true if total >= 0 => (total as f64 / n as f64).round_ties_even() as i128,
                _ => total,
            };
            V::Int(i64::try_from(value).map_err(|_| SqlError::out_of_range())?)
        }
        Ty::Num { scale, .. } => {
            let total = exact(scale)?;
            V::Num(if average { total / n } else { total }, scale)
        }
        Ty::Money => {
            let total = exact(4)?;
            let value = if average { total / n } else { total };
            V::Money(i64::try_from(value).map_err(|_| SqlError::out_of_range())?)
        }
        _ => {
            let total: f64 = values.iter().filter_map(V::as_f64).sum();
            V::Double(if average { total / n as f64 } else { total })
        }
    })
}

/// Puts `record` at `start` in a row of `width` values.
fn place(record: &[V], start: usize, width: usize) -> Vec<V> {
    let mut row = vec![V::Null; width.max(start + record.len())];
    row[start..start + record.len()].clone_from_slice(record);
    row
}

/// Compares rows value by value, NULL first.
fn compare_values(a: &[V], b: &[V], ci: &[bool], collation: Option<&Collation>) -> Ordering {
    a.iter()
        .zip(b)
        .zip(ci)
        .map(|((x, y), &ci)| order(x, y, ci, collation))
        .find(|o| *o != Ordering::Equal)
        .unwrap_or(Ordering::Equal)
}

/// `rows` sorted on their first `ci.len()` values and without repeats: of
/// equal rows, the last (`keep_last`) or the first stays.
fn distinct(
    rows: Vec<Tuple>,
    ci: &[bool],
    keep_last: bool,
    collation: Option<&Collation>,
) -> Vec<Tuple> {
    let n = ci.len();
    let head = |t: &Tuple| t.values[..n.min(t.values.len())].to_vec();
    let mut rows = rows;
    rows.sort_by(|a, b| compare_values(&head(a), &head(b), ci, collation));
    let mut out: Vec<Tuple> = Vec::with_capacity(rows.len());
    for row in rows {
        match out.last_mut() {
            Some(prev)
                if compare_values(&head(prev), &head(&row), ci, collation) == Ordering::Equal =>
            {
                if keep_last {
                    *prev = row;
                }
            }
            _ => out.push(row),
        }
    }
    out
}
