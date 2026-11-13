//! Evaluating typed expressions on a row.

use std::cmp::Ordering;
use std::rc::Rc;
use std::sync::Arc;

use tflip_storage::cp1252;
use tflip_storage::field::FieldType;
use tflip_storage::value::{Value, Ymd};

use crate::ast::BinOp;
use crate::compile::{
    FtsCondition, FtsSearch, FtsSource, Kind, Node, date_text, time_text, timestamp_text,
};
use crate::error::SqlError;
use crate::fts;
use crate::func::{self, Func};
use crate::types::{IntWidth, Ty};
use crate::value::{Collation, V, compare, parse_exact, pow10};

/// A row expressions read: the table's record, or none (`system.iota`).
pub trait Row {
    /// The value of field `index`.
    fn value(&mut self, index: usize) -> Result<Value, SqlError>;

    /// The value of column `index`, of type `ty`.
    fn column(&mut self, index: usize, ty: Ty) -> Result<V, SqlError> {
        Ok(V::from_field(self.value(index)?, ty))
    }

    /// The value of column `index` of the row `depth` levels out, in a
    /// subquery.
    fn outer(&mut self, _depth: usize, _index: usize) -> Result<V, SqlError> {
        Ok(V::Null)
    }

    /// The value of the group's aggregate `index`.
    fn aggregate(&mut self, _index: usize) -> Result<V, SqlError> {
        Ok(V::Null)
    }

    /// The rows of subquery `id`, run for this row.
    fn subquery(&mut self, _id: usize) -> Result<Rc<Vec<Vec<V>>>, SqlError> {
        Err(SqlError::unsupported("subqueries here", 0))
    }

    /// The number of the record the value of column `index` is read from, if
    /// the row knows it: a FIXED full-text order answers by it.
    fn record(&mut self, _index: usize) -> Option<u32> {
        None
    }

    /// Whether the record the value of column `index` is read from is marked
    /// deleted: a DBF table's records are, an ADT table's never are read.
    fn deleted(&mut self, _index: usize) -> bool {
        false
    }

    /// `ROWNUM()`: how many rows the result has when this one is added.
    fn rownum(&mut self) -> u32 {
        1
    }
}

/// No row: expressions over `system.iota`, which has no fields to read.
pub struct NoRow;

impl Row for NoRow {
    fn value(&mut self, _index: usize) -> Result<Value, SqlError> {
        Ok(Value::Null)
    }
}

/// The user-defined functions expressions can call, run by the engine over
/// the connection's dictionary.
pub trait UserFunctions: std::fmt::Debug + Send + Sync {
    /// Runs the function `name` (a call name, `sq` or `util.pad`) with
    /// `args`, and returns its result.
    fn call(&self, name: &str, args: &[V]) -> Result<V, SqlError>;
}

/// How `UPPER` and `LOWER` fold case under a static collation: each byte's
/// upper- and lower-case byte, from the collation's own case tables (the
/// display character, not weighed — unlike [`tflip_expr::CaseFold`], which
/// the expression evaluator weighs for comparisons). `None` on [`Env`] folds
/// ASCII only, under the default collation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseFold {
    /// `UPPER`: each byte in upper case.
    pub upper: [u8; 256],
    /// `LOWER`: each byte in lower case.
    pub lower: [u8; 256],
}

/// What expressions are evaluated with.
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// The connection's date format, for `DTOC`, `CTOD` and dates given as
    /// text.
    pub date_format: String,
    /// The epoch of years of two digits (`AdsSetEpoch`); 0 for the default,
    /// 1900.
    pub epoch: u16,
    /// The local date and time now: a Julian day and milliseconds.
    pub now: (i32, u32),
    pub utc_now: (i32, u32),
    /// The parameters' values, converted to their types.
    pub params: Vec<V>,
    /// The user-defined functions the dictionary offers, for `Kind::UserFunc`.
    pub functions: Option<std::sync::Arc<dyn UserFunctions>>,
    /// How `UPPER` and `LOWER` fold case under [`Env::collation`]; `None`
    /// under the default collation (ASCII folding).
    pub case_fold: Option<CaseFold>,
    /// The collation SQL compares text by: the statement's dynamic one, else
    /// the connection's static ANSI one if it is not the default (identity)
    /// one; `None` for byte-wise comparisons.
    pub collation: Option<Arc<Collation>>,
    /// The connection's database path, for `DATABASE()`: the folder of a
    /// free-table connection, or the `.add` of a dictionary one.
    pub database: String,
    /// The connection's random-number generator, for `RAND()`.
    pub rand: std::sync::Arc<std::sync::Mutex<crate::Rand>>,
    /// Whether `EMPTY` takes tabs and line ends for spaces, as a live
    /// cursor's `WHERE` does (the original engine's expression engine
    /// evaluates it).
    pub xbase_blanks: bool,
    /// Whether the expression engine that filters a table's records
    /// evaluates, as a live cursor's `WHERE` does: `AND` and `OR`, `IIF` and
    /// `COALESCE` evaluate every operand, so a division by zero in one that
    /// is not needed still fails (`fixtures/sql/wherefilter.json`).
    pub engine: bool,
    /// The connection's application ID, for `APPLICATIONID()`.
    pub application_id: String,
    /// The AutoInc value the connection last generated, for
    /// `LASTAUTOINC(CONNECTION)`.
    pub connection_autoinc: std::sync::Arc<std::sync::Mutex<Option<i32>>>,
    /// What the statement last inserted or wrote, for `LASTAUTOINC(STATEMENT)`,
    /// `LASTROWID()` and `LASTROWVERSION()`.
    pub statement_last: std::sync::Arc<std::sync::Mutex<Last>>,
}

/// What a statement last wrote ([`Env::statement_last`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Last {
    /// The AutoInc value of the last record it inserted.
    pub autoinc: Option<i32>,
    /// The `ROWID` of the last record it inserted.
    pub rowid: Option<String>,
    /// The ROWVERSION of the last record it wrote.
    pub rowversion: Option<i64>,
}

impl Env {
    /// The epoch of years of two digits.
    pub fn epoch(&self) -> u16 {
        if self.epoch == 0 {
            tflip_expr::text::DEFAULT_EPOCH
        } else {
            self.epoch
        }
    }
}

/// Whether the value `v` of a column of type `ty` is empty: NULL in an ADT
/// table; in a DBF one also its type's empty value, as xBase has it. A memo
/// is empty with no text at all, text with nothing but spaces, and a blank
/// date of a FoxPro table is day 0 of its calendar.
fn empty(v: &V, ty: Ty, dbf: bool, tabs: bool) -> bool {
    const DAY_ZERO: i32 = 2_415_019;
    match v {
        V::Null => true,
        V::Blank(blank) => empty(&blank.value, ty, true, tabs),
        _ if !dbf => false,
        V::Int(n) => *n == 0,
        V::Num(n, _) => *n == 0,
        V::Money(n) => *n == 0,
        V::Double(d) => *d == 0.0,
        V::Bool(b) => !b,
        V::Date(d) => *d == 0 || *d == DAY_ZERO,
        V::TimeStamp(d, t) => (*d == 0 || *d == DAY_ZERO) && *t == 0,
        V::Str(s) if ty.is_memo() => s.is_empty(),
        // The expression engine a `WHERE` runs on takes tabs and line ends
        // for spaces too.
        V::Str(s) if tabs => s.trim_matches([' ', '\t', '\r', '\n']).is_empty(),
        V::Str(s) => s.trim_matches(' ').is_empty(),
        _ => false,
    }
}

/// Evaluates `node` on `row`, but a column read straight from a record as
/// it is there: a blank field of a Clipper or FoxPro table is [`V::Blank`],
/// NULL to `IS NULL`, `IFNULL` and `COALESCE` (`fixtures/sql/dbf.json`).
pub(crate) fn read(node: &Node, row: &mut dyn Row, env: &Env) -> Result<V, SqlError> {
    match &node.kind {
        Kind::Column(i) => row.column(*i, node.ty),
        Kind::Outer(depth, i) => row.outer(*depth, *i),
        _ => eval(node, row, env),
    }
}

/// Evaluates `node` on `row` as the original's expression engine does for a
/// table's records, which `env` has with [`Env::engine`]: a division by zero
/// is its error 4120.
pub fn eval_filter(node: &Node, row: &mut dyn Row, env: &Env) -> Result<V, SqlError> {
    eval(node, row, env).map_err(|e| match e.native {
        2153 => SqlError::filter_division(),
        _ => e,
    })
}

/// Evaluates `node` on `row`.
pub fn eval(node: &Node, row: &mut dyn Row, env: &Env) -> Result<V, SqlError> {
    Ok(match &node.kind {
        Kind::Const(v) => v.clone(),
        // A blank field computes as its blank value.
        Kind::Column(i) => row.column(*i, node.ty)?.unblank(),
        Kind::Outer(depth, i) => row.outer(*depth, *i)?.unblank(),
        Kind::Agg(i) => row.aggregate(*i)?,
        Kind::Subquery(id) => {
            let rows = row.subquery(*id)?;
            match rows.as_slice() {
                [] => V::Null,
                [only] => only.first().cloned().unwrap_or(V::Null).unblank(),
                _ => return Err(SqlError::subquery_rows()),
            }
        }
        Kind::Exists(id) => V::Bool(!row.subquery(*id)?.is_empty()),
        Kind::InSelect(e, id, negated) => {
            let v = eval(e, row, env)?;
            if v.is_null() {
                return Ok(V::Null);
            }
            let rows = row.subquery(*id)?;
            let ci = e.ty.case_insensitive();
            let mut null = false;
            for x in rows.iter().filter_map(|r| r.first()) {
                if x.is_null() {
                    null = true;
                } else if compared(&v, e.ty, x, e.ty, ci, env) == Some(Ordering::Equal) {
                    return Ok(V::Bool(!*negated));
                }
            }
            if null { V::Null } else { V::Bool(*negated) }
        }
        Kind::Quantified(op, e, id, all) => {
            let v = eval(e, row, env)?;
            let rows = row.subquery(*id)?;
            let ci = e.ty.case_insensitive();
            let mut null = false;
            for x in rows.iter().filter_map(|r| r.first()) {
                match compared(&v, e.ty, x, e.ty, ci, env) {
                    None => null = true,
                    Some(o) => {
                        let holds = match op {
                            BinOp::Eq => o == Ordering::Equal,
                            BinOp::Ne => o != Ordering::Equal,
                            BinOp::Lt => o == Ordering::Less,
                            BinOp::Le => o != Ordering::Greater,
                            BinOp::Gt => o == Ordering::Greater,
                            _ => o != Ordering::Less,
                        };
                        if holds != *all {
                            return Ok(V::Bool(holds));
                        }
                    }
                }
            }
            if null { V::Null } else { V::Bool(*all) }
        }
        Kind::Param(i) => param(
            env.params.get(*i).cloned().unwrap_or(V::Null),
            node.ty,
            node.pos,
            env,
        )?,
        Kind::Neg(e) => match eval(e, row, env)? {
            V::Null => V::Null,
            // A number that no longer fits its 16 or 32 bits is out of range
            // where it is computed, not where it is stored.
            V::Int(n) => match node.ty {
                Ty::Int {
                    width: IntWidth::Short,
                    ..
                } if i16::try_from(-n).is_err() => return Err(SqlError::out_of_range()),
                Ty::Int {
                    width: IntWidth::Int,
                    ..
                } if i32::try_from(-n).is_err() => return Err(SqlError::out_of_range()),
                _ => V::Int(n.wrapping_neg()),
            },
            V::Num(n, s) => V::Num(-n, s),
            V::Money(m) => V::Money(-m),
            V::Double(d) => V::Double(-d),
            _ => return Err(SqlError::invalid_operand("-", node.pos)),
        },
        Kind::BitNot(e) => match eval(e, row, env)? {
            V::Null => V::Null,
            v => V::Int(!(v.as_i128().unwrap_or(0) as i64)),
        },
        Kind::Not(e) => match eval(e, row, env)? {
            V::Bool(b) => V::Bool(!b),
            _ => V::Null,
        },
        Kind::Binary(op, a, b) => binary(*op, a, b, node, row, env)?,
        Kind::IsNull(e, negated) => V::Bool(read(e, row, env)?.is_null() != *negated),
        Kind::Between(e, low, high, negated) => {
            let v = eval(e, row, env)?;
            let low = eval(low, row, env)?;
            let high = eval(high, row, env)?;
            let ci = e.ty.case_insensitive();
            for bound in [&low, &high] {
                if let Some(what) = unreadable_text(bound, low_ty(node), e.ty, env)
                    .or_else(|| unreadable_text(&v, e.ty, low_ty(node), env))
                {
                    return Err(unreadable_error("BETWEEN", what, node.pos));
                }
            }
            match (
                compared(&v, e.ty, &low, low_ty(node), ci, env),
                compared(&v, e.ty, &high, low_ty(node), ci, env),
            ) {
                (Some(l), Some(h)) => {
                    V::Bool((l != Ordering::Less && h != Ordering::Greater) != *negated)
                }
                _ => V::Null,
            }
        }
        Kind::In(e, list, negated) => {
            let v = eval(e, row, env)?;
            if v.is_null() {
                return Ok(V::Null);
            }
            let ci = e.ty.case_insensitive();
            let mut found = false;
            for item in list {
                let x = eval(item, row, env)?;
                if let Some(what) = unreadable_text(&x, item.ty, e.ty, env)
                    .or_else(|| unreadable_text(&v, e.ty, item.ty, env))
                {
                    return Err(unreadable_error("IN", what, node.pos));
                }
                if compared(&v, e.ty, &x, item.ty, ci || item.ty.case_insensitive(), env)
                    == Some(Ordering::Equal)
                {
                    found = true;
                    break;
                }
            }
            V::Bool(found != *negated)
        }
        Kind::Like(e, pattern, escape, negated) => {
            let v = eval(e, row, env)?;
            let p = eval(pattern, row, env)?;
            let escape = match escape {
                Some(escape) => match eval(escape, row, env)? {
                    V::Str(s) => s.chars().next(),
                    _ => None,
                },
                None => None,
            };
            match (v, p) {
                (V::Str(v), V::Str(p)) => {
                    let ci = e.ty.case_insensitive();
                    // A dynamic collation that ignores case does so here too
                    // (probed, `fixtures/sql/collation_dynamic.json`).
                    let dynamic = env.collation.as_deref().and_then(Collation::dynamic);
                    let folded = match dynamic {
                        Some(dynamic) if dynamic.ignores_case() => {
                            let fold = |s: &str| {
                                let bytes = cp1252::encode(s);
                                let upper: Vec<u8> = bytes
                                    .iter()
                                    .map(|&b| dynamic.upper[usize::from(b)])
                                    .collect();
                                cp1252::decode(&upper)
                            };
                            Some((fold(&v), fold(&p)))
                        }
                        _ => None,
                    };
                    let (v, p) = folded.unwrap_or((v, p));
                    V::Bool(like(&v, &p, escape, ci) != *negated)
                }
                _ => V::Null,
            }
        }
        Kind::Case(operand, whens, otherwise) => {
            let operand = match operand {
                Some(o) => Some((eval(o, row, env)?, o.ty)),
                None => None,
            };
            for (when, then) in whens {
                let w = eval(when, row, env)?;
                let hit = match &operand {
                    Some((v, ty)) => {
                        compared(v, *ty, &w, when.ty, ty.case_insensitive(), env)
                            == Some(Ordering::Equal)
                    }
                    None => w == V::Bool(true),
                };
                if hit {
                    return convert_to(eval(then, row, env)?, then.ty, node.ty, node.pos);
                }
            }
            match otherwise {
                Some(e) => convert_to(eval(e, row, env)?, e.ty, node.ty, node.pos)?,
                None => V::Null,
            }
        }
        Kind::RecNo(i) => V::Int(row.record(*i).map_or(0, i64::from)),
        Kind::Deleted(i) => V::Bool(row.deleted(*i)),
        Kind::RowId(i, prefix) => row
            .record(*i)
            .map_or(V::Null, |n| V::Str(crate::rowid::encode(prefix, n))),
        Kind::Func(Func::RowNum, _) => V::Int(i64::from(row.rownum())),
        // Only a column read from a record is empty, as its table's type
        // says; an expression never is.
        Kind::Func(f @ (Func::Empty | Func::EmptyDbf), args) => {
            let arg = &args[0];
            V::Bool(
                matches!(arg.kind, Kind::Column(_))
                    && empty(
                        &read(arg, row, env)?,
                        arg.ty,
                        *f == Func::EmptyDbf,
                        env.xbase_blanks,
                    ),
            )
        }
        Kind::Func(Func::Iif, args) => {
            let holds = eval(&args[0], row, env)? == V::Bool(true);
            let branch = if holds { &args[1] } else { &args[2] };
            let value = eval(branch, row, env)?;
            if env.engine {
                eval(&args[if holds { 2 } else { 1 }], row, env)?;
            }
            convert_to(value, branch.ty, node.ty, node.pos)?
        }
        Kind::Func(f @ (Func::Coalesce | Func::IfNull), args) => {
            // Unlike `COALESCE`, the original's `IFNULL` evaluates every
            // argument: a division by 0 in the second fails it though the
            // first is not NULL (tools/ace/sql/iif.txt).
            if *f == Func::IfNull || env.engine {
                for arg in args {
                    read(arg, row, env)?;
                }
            }
            for arg in args {
                let v = read(arg, row, env)?;
                if !v.is_null() {
                    return convert_to(v, arg.ty, node.ty, node.pos);
                }
            }
            V::Null
        }
        Kind::Func(f, args) => {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                values.push(eval(arg, row, env)?);
            }
            let value = func::apply(*f, args, &values, node.ty, env, node.pos)?;
            // A parameter's text beyond its width is cut where a function
            // makes text of it (`fixtures/sql/params.json`).
            match (f, value, node.ty) {
                (Func::Upper | Func::Lower, V::Str(text), Ty::Str { length, .. })
                    if text.chars().count() > usize::from(length) =>
                {
                    V::Str(text.chars().take(usize::from(length)).collect())
                }
                (_, value, _) => value,
            }
        }
        Kind::UserFunc(name, args) => {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                values.push(eval(arg, row, env)?);
            }
            match &env.functions {
                Some(functions) => functions.call(name, &values)?,
                None => {
                    return Err(SqlError::unsupported(
                        &format!("the function {name}"),
                        node.pos,
                    ));
                }
            }
        }
        Kind::Cast(e, "TRUTH") => truth(eval(e, row, env)?, e.ty),
        Kind::Cast(e, function) => {
            let v = eval(e, row, env)?;
            let cast = cast(
                v,
                e.ty,
                node.ty,
                function,
                node.pos,
                matches!(e.kind, Kind::Param(_)).then_some(e.pos),
                env,
            )?;
            // A constant that does not fit fails when the statement runs; a
            // column's value, when its row is read.
            match (node.ty, &e.kind) {
                (Ty::Int { width, .. }, Kind::Const(_)) => in_width(cast, width)?,
                _ => cast,
            }
        }
        Kind::Trim(side, chars, e) => {
            let v = eval(e, row, env)?;
            let chars = match chars {
                Some(c) => match eval(c, row, env)? {
                    V::Str(s) => s,
                    V::Null => return Ok(V::Null),
                    other => other.display(),
                },
                None => " ".to_owned(),
            };
            match v {
                V::Str(s) => V::Str(trim(&s, &chars, *side)),
                _ => V::Null,
            }
        }
        Kind::Contains(search) => fts_contains(search, row, env)?,
        Kind::Score(search, distinct) => fts_score(search, *distinct, row, env)?,
    })
}

/// The parsed `CONTAINS`/`SCORE` condition for a row: `None` means the result is
/// NULL (a NULL or absent condition).
fn fts_condition<'a>(
    search: &'a FtsSearch,
    row: &mut dyn Row,
    env: &Env,
) -> Result<Option<std::borrow::Cow<'a, fts::Query>>, SqlError> {
    use std::borrow::Cow;
    Ok(match &search.condition {
        FtsCondition::Null => None,
        FtsCondition::Invalid(code) => return Err(SqlError::fts(*code)),
        FtsCondition::Const(query) => Some(Cow::Borrowed(query.as_ref())),
        FtsCondition::Dynamic(node) => match eval(node, row, env)? {
            V::Null => None,
            V::Str(text) => match fts::parse(&text, &search.rules) {
                Ok(query) => Some(Cow::Owned(query)),
                Err(error) => return Err(SqlError::fts(error.0)),
            },
            _ => None,
        },
    })
}

/// The text of an expression source for a row: `None` when the field is NULL.
fn fts_text(node: &Node, row: &mut dyn Row, env: &Env) -> Result<Option<String>, SqlError> {
    Ok(match eval(node, row, env)? {
        V::Str(text) => Some(text),
        _ => None,
    })
}

/// `CONTAINS`: whether the condition matches the field(s), with the engine's
/// NULL rules (a NULL field is NULL, except a fully-optimized condition on an
/// indexed field, which the index answers over a word-less row). A part of a
/// `WHERE` the original engine answers through a FIXED order is answered from
/// its keys (see [`fixed_contains`]).
fn fts_contains(search: &FtsSearch, row: &mut dyn Row, env: &Env) -> Result<V, SqlError> {
    let Some(query) = fts_condition(search, row, env)? else {
        return Ok(V::Null);
    };
    // An indexed WHERE predicate is answered over the index, which keeps rows
    // with no words (a NULL field), when the condition is fully optimized; as a
    // projected scalar, a NULL field is just NULL.
    let on_empty = |query: &fts::Query| -> V {
        if search.filter && search.indexed && fts::fully_optimized(query) {
            V::Bool(fts::matches(query, &[]))
        } else {
            V::Null
        }
    };
    Ok(match &search.source {
        FtsSource::Expr(node) => {
            if search.answered
                && search.indexed
                && let Some(answer) = indexed_contains(node, &search.rules, &query, row, env)?
            {
                return Ok(V::Bool(answer));
            }
            match fts_text(node, row, env)? {
                Some(text) => V::Bool(fts::matches(&query, &fts::tokenize(&text, &search.rules))),
                None => on_empty(&query),
            }
        }
        FtsSource::Fields(fields) => {
            let mut any_text = false;
            let mut matched = false;
            for (node, rules) in fields {
                if search.answered
                    && let Some(answer) = indexed_contains(node, rules, &query, row, env)?
                {
                    any_text = true;
                    matched |= answer;
                    continue;
                }
                if let V::Str(text) = eval(node, row, env)? {
                    any_text = true;
                    matched |= fts::matches(&query, &fts::tokenize(&text, rules));
                }
            }
            if any_text {
                V::Bool(matched)
            } else {
                on_empty(&query)
            }
        }
    })
}

/// `CONTAINS` of `query` over the indexed field `node` answered as the original
/// engine answers it through the field's full-text order: from the order's keys
/// for the record, and — where the keys only narrow the records (a phrase,
/// `NEAR`, a word as long as the keys) — from the record's text too. A
/// condition the order cannot answer at all is the text's alone. The keys of a
/// FIXED order are `rules.postings`, by the row's record; an order kept current
/// holds the text's words, cut to its longest word. `None` for a FIXED order
/// whose record the row does not know, and where the field is NULL (see
/// [`fts_contains`]).
fn indexed_contains(
    node: &Node,
    rules: &fts::Rules,
    query: &fts::Query,
    row: &mut dyn Row,
    env: &Env,
) -> Result<Option<bool>, SqlError> {
    let level = fts::index_level(query, rules.max_word);
    let words = fts_text(node, row, env)?.map(|text| fts::tokenize(&text, rules));
    let text = |words: &Option<Vec<String>>| words.as_ref().is_some_and(|w| fts::matches(query, w));
    let keys_hold = match (&rules.postings, &node.kind) {
        (Some(postings), Kind::Column(column)) => {
            let Some(record) = row.record(*column) else {
                return Ok(None);
            };
            postings.holds(record, query)
        }
        _ => match &words {
            Some(words) => {
                fts::keys_hold(&fts::keys_of(words, rules.max_word), rules.max_word, query)
            }
            None => return Ok(None),
        },
    };
    Ok(Some(match level {
        fts::IndexLevel::Full => keys_hold,
        fts::IndexLevel::Partial => keys_hold && text(&words),
        fts::IndexLevel::None => text(&words),
    }))
}

/// `SCORE`/`SCOREDISTINCT`: the score of the condition over the field(s); NULL
/// for a NULL field (but `*`) or condition. `SCOREDISTINCT` over a field with a FIXED
/// order counts the words the order holds for the record (see
/// [`fts::Postings::score_distinct`]).
fn fts_score(
    search: &FtsSearch,
    distinct: bool,
    row: &mut dyn Row,
    env: &Env,
) -> Result<V, SqlError> {
    let Some(query) = fts_condition(search, row, env)? else {
        return Ok(V::Null);
    };
    let one =
        |node: &Node, rules: &fts::Rules, row: &mut dyn Row| -> Result<Option<u64>, SqlError> {
            let Some(text) = fts_text(node, row, env)? else {
                return Ok(None);
            };
            let words = fts::tokenize(&text, rules);
            if distinct
                && let (Some(postings), Kind::Column(column)) = (&rules.postings, &node.kind)
                && let Some(record) = row.record(*column)
            {
                return Ok(Some(postings.score_distinct(record, &query, &words)));
            }
            Ok(Some(fts::score(&query, &words, distinct)))
        };
    Ok(match &search.source {
        FtsSource::Expr(node) => match one(node, &search.rules, row)? {
            Some(n) => V::Int(n as i64),
            None => V::Null,
        },
        // Over every full-text field, NULL ones score nothing: the score is
        // never NULL (`fixtures/sql/fts_plans.json`).
        FtsSource::Fields(fields) => {
            let mut total = 0u64;
            for (node, rules) in fields {
                total += one(node, rules, row)?.unwrap_or(0);
            }
            V::Int(total as i64)
        }
    })
}

/// The type of a `BETWEEN`'s bounds.
fn low_ty(node: &Node) -> Ty {
    match &node.kind {
        Kind::Between(_, low, _, _) => low.ty,
        _ => Ty::Unknown,
    }
}

/// Compares two values of types `ta` and `tb`, text given for a date
/// read as one; `None` if either is NULL or they do not compare.
fn compared(a: &V, ta: Ty, b: &V, tb: Ty, ci: bool, env: &Env) -> Option<Ordering> {
    if a.is_null() || b.is_null() {
        return None;
    }
    let as_date = |v: &V, ty: Ty, other: Ty| -> V {
        match (v, other) {
            (V::Str(s), Ty::Date) if ty.is_text() => text_date(s, env).map_or(V::Null, V::Date),
            (V::Str(s), Ty::Time) if ty.is_text() => time_text(s).map_or(V::Null, V::Time),
            (V::Str(s), Ty::Guid) if ty.is_text() => guid_bytes(s).map_or(V::Null, V::Guid),
            (V::Str(s), Ty::TimeStamp) if ty.is_text() => {
                timestamp_text(s, &env.date_format, env.epoch())
                    .map_or(V::Null, |(d, t)| V::TimeStamp(d, t))
            }
            _ => v.clone(),
        }
    };
    let a = as_date(a, ta, tb);
    let b = as_date(b, tb, ta);
    compare(
        &a,
        &b,
        ci || ta.case_insensitive() || tb.case_insensitive(),
        env.collation.as_deref(),
    )
}

/// What `v`, text compared with a value of type `other`, is not: a date,
/// time, timestamp (named) or GUID (the empty name).
pub(crate) fn unreadable_text(v: &V, ty: Ty, other: Ty, env: &Env) -> Option<&'static str> {
    let V::Str(s) = v else {
        return None;
    };
    if !ty.is_text() {
        return None;
    }
    match other {
        Ty::Date if text_date(s, env).is_none() => Some("DATE"),
        Ty::Time if time_text(s).is_none() => Some("TIME"),
        Ty::TimeStamp if timestamp_text(s, &env.date_format, env.epoch()).is_none() => {
            Some("TIMESTAMP")
        }
        Ty::Guid if guid_bytes(s).is_none() => Some(""),
        _ => None,
    }
}

/// The error of an operator whose text operand is not what `what` names.
fn unreadable_error(operator: &str, what: &str, pos: crate::error::Pos) -> SqlError {
    match what {
        "" => SqlError::invalid_operand(operator, pos),
        _ => SqlError::invalid_operand_value(operator, what, pos),
    }
}

/// The error the first comparison in `node` with a text constant that is no
/// date, time, timestamp or GUID gives, which the original engine raises
/// when it opens a cursor on the statement, before any record is read.
pub fn check_text_constants(node: &Node, env: &Env) -> Result<(), SqlError> {
    let found = std::cell::RefCell::new(None);
    node.any(&|n| {
        let error = match &n.kind {
            Kind::Binary(op, a, b) if op.is_comparison() => [(a, b), (b, a)]
                .into_iter()
                .find_map(|(text, other)| constant_text(text, other, env))
                .map(|what| unreadable_error(op.symbol(), what, n.pos)),
            Kind::Between(e, low, high, _) => [low, high]
                .into_iter()
                .find_map(|bound| constant_text(bound, e, env))
                .map(|what| unreadable_error("BETWEEN", what, n.pos)),
            Kind::In(e, list, _) => list
                .iter()
                .find_map(|item| constant_text(item, e, env))
                .map(|what| unreadable_error("IN", what, n.pos)),
            _ => None,
        };
        let stop = error.is_some();
        if stop {
            *found.borrow_mut() = error;
        }
        stop
    });
    found.into_inner().map_or(Ok(()), Err)
}

fn constant_text(text: &Node, other: &Node, env: &Env) -> Option<&'static str> {
    match &text.kind {
        Kind::Const(v) => unreadable_text(v, text.ty, other.ty, env),
        _ => None,
    }
}

/// A date written as text: `YYYY-MM-DD`, or in the date format.
pub(crate) fn text_date(text: &str, env: &Env) -> Option<i32> {
    date_text(text, &env.date_format, env.epoch())
}

fn binary(
    op: BinOp,
    a: &Node,
    b: &Node,
    node: &Node,
    row: &mut dyn Row,
    env: &Env,
) -> Result<V, SqlError> {
    if matches!(op, BinOp::And | BinOp::Or) {
        let x = eval(a, row, env)?;
        let short = if op == BinOp::And {
            V::Bool(false)
        } else {
            V::Bool(true)
        };
        if x == short && !env.engine {
            return Ok(short);
        }
        let y = eval(b, row, env)?;
        return Ok(match (x, y) {
            (x, y) if x == short || y == short => short,
            (V::Bool(_), V::Bool(y)) => V::Bool(y),
            _ => V::Null,
        });
    }
    let x = eval(a, row, env)?;
    let y = eval(b, row, env)?;
    if op.is_comparison() {
        let unreadable =
            unreadable_text(&x, a.ty, b.ty, env).or_else(|| unreadable_text(&y, b.ty, a.ty, env));
        if let Some(what) = unreadable {
            return Err(unreadable_error(op.symbol(), what, node.pos));
        }
        let ci = a.ty.case_insensitive() || b.ty.case_insensitive();
        return Ok(match compared(&x, a.ty, &y, b.ty, ci, env) {
            None => V::Null,
            Some(order) => V::Bool(match op {
                BinOp::Eq => order == Ordering::Equal,
                BinOp::Ne => order != Ordering::Equal,
                BinOp::Lt => order == Ordering::Less,
                BinOp::Le => order != Ordering::Greater,
                BinOp::Gt => order == Ordering::Greater,
                _ => order != Ordering::Less,
            }),
        });
    }
    if x.is_null() || y.is_null() {
        return Ok(V::Null);
    }
    if matches!(op, BinOp::Shl | BinOp::Shr) {
        return Ok(shift(op, &x, &y, node.ty));
    }
    // A parameter's text beyond its width cannot be joined to anything
    // (`fixtures/sql/params.json`).
    if op == BinOp::Add && node.ty.is_text() {
        for (operand, value) in [(a, &x), (b, &y)] {
            if let (Kind::Param(_), V::Str(text)) = (&operand.kind, value)
                && text.chars().count() > usize::from(operand.ty.length())
            {
                return Err(SqlError::concat_overflow(node.pos));
            }
        }
    }
    let result = arithmetic(op, &x, a.ty, &y, b.ty, node.ty)?;
    // A result too wide for the capped type fails where it is computed.
    if let (V::Num(n, _), Ty::Num { precision, .. }) = (&result, node.ty)
        && n.unsigned_abs() >= pow10(precision) as u128
    {
        return Err(SqlError::numeric_overflow().located_at(node.pos));
    }
    Ok(result)
}

/// `x << y` and `x >> y`, in the width of the type `ty` of the left operand
/// (`fixtures/sql/gram_shift.json`): a count of that many bits or more
/// leaves 0, or -1 for a negative number shifted right; a negative count
/// shifts the other way; the lowest number of the width is no value, which a
/// column of 16 or 32 bits refuses where it is stored (`Value::to_field`)
/// and a BIGINT reads as NULL.
fn shift(op: BinOp, x: &V, y: &V, ty: Ty) -> V {
    let count = |v: &V| match v {
        V::Bool(b) => i128::from(*b),
        v => v.as_i128().unwrap_or(0),
    };
    let (n, by) = (count(x), count(y));
    let bits = match ty {
        Ty::Int {
            width: IntWidth::Short,
            ..
        } => 16,
        Ty::Int {
            width: IntWidth::Long,
            ..
        } => 64,
        _ => 32,
    };
    // Left means a shift towards the high bits.
    let left = (op == BinOp::Shl) == (by >= 0);
    let by = by.unsigned_abs();
    let shifted = match (left, by >= bits) {
        (true, true) => 0,
        (true, false) => {
            // Wrapped into the width, as the bits that leave it are lost.
            let wide = (n << by) & ((1i128 << bits) - 1);
            match wide >= 1i128 << (bits - 1) {
                true => wide - (1i128 << bits),
                false => wide,
            }
        }
        (false, true) => n.signum().min(0),
        (false, false) => n >> by,
    };
    match ty {
        Ty::Bool => V::Bool(shifted == 1),
        Ty::Int {
            width: IntWidth::Long,
            ..
        } if shifted == i128::from(i64::MIN) => V::Null,
        _ => V::Int(shifted as i64),
    }
}

/// `x op y` with the result type `ty`.
fn arithmetic(op: BinOp, x: &V, tx: Ty, y: &V, ty_y: Ty, ty: Ty) -> Result<V, SqlError> {
    let zero = || SqlError::division_by_zero();
    Ok(match (ty, x, y) {
        (Ty::Str { .. }, V::Str(a), V::Str(b)) => V::Str(format!("{a}{b}")),
        (Ty::Bool, V::Bool(a), V::Bool(b)) => V::Bool(match op {
            BinOp::BitOr => a | b,
            BinOp::BitXor => a ^ b,
            _ => a & b,
        }),
        (Ty::Date, V::Date(d), n) | (Ty::Date, n, V::Date(d)) => {
            let days = n.as_i128().unwrap_or(0) as i64;
            let day = i64::from(*d) + if op == BinOp::Sub { -days } else { days };
            // The calendar is years 1 to 9999.
            let first = tflip_storage::value::ymd_to_julian(Ymd {
                year: 1,
                month: 1,
                day: 1,
            });
            let last = tflip_storage::value::ymd_to_julian(Ymd {
                year: 9999,
                month: 12,
                day: 31,
            });
            if first.is_some_and(|f| day < i64::from(f)) || last.is_some_and(|l| day > i64::from(l))
            {
                return Err(SqlError::date_to_julian());
            }
            V::Date(day as i32)
        }
        (_, V::Date(a), V::Date(b)) => V::Int(i64::from(a - b)),
        (Ty::Int { .. }, _, _) => {
            // Bytes are the integer of their first four, a missing one 0.
            let whole = |v: &V| match v {
                V::Bin(bytes) => {
                    let mut head = [0u8; 4];
                    let n = bytes.len().min(4);
                    head[..n].copy_from_slice(&bytes[..n]);
                    i128::from(i32::from_be_bytes(head))
                }
                other => other.as_i128().unwrap_or(0),
            };
            let (a, b) = (whole(x), whole(y));
            let n = match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a.saturating_mul(b),
                BinOp::Div if b == 0 => return Err(zero()),
                BinOp::Div => a / b,
                BinOp::Mod if b == 0 => return Err(zero()),
                BinOp::Mod => a % b,
                BinOp::BitOr => a | b,
                BinOp::BitAnd => a & b,
                BinOp::BitXor => a ^ b,
                _ => 0,
            };
            // A BIGINT wraps around; the lowest number is the one that is no
            // value (`Value::to_field`).
            V::Int(match ty {
                Ty::Int {
                    width: IntWidth::Long,
                    ..
                } => n as i64,
                _ => n.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64,
            })
        }
        (Ty::Num { scale, .. }, _, _) => V::Num(
            exact(
                op,
                x,
                scale_of_ty(tx, x),
                y,
                scale_of_ty(ty_y, y),
                scale,
                false,
            )?,
            scale,
        ),
        // MONEY rounds what it keeps; a result beyond its range is kept as
        // the number it is, which the field refuses (`Value::to_field`).
        (Ty::Money, _, _) => {
            let n = exact(op, x, scale_of_ty(tx, x), y, scale_of_ty(ty_y, y), 4, true)?;
            match i64::try_from(n) {
                Ok(m) => V::Money(m),
                Err(_) => V::Num(n, 4),
            }
        }
        _ => {
            let (a, b) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            V::Double(match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div if b == 0.0 => return Err(zero()),
                BinOp::Div => a / b,
                BinOp::Mod if b == 0.0 => return Err(zero()),
                _ => a % b,
            })
        }
    })
}

/// The scale an exact operand is computed at.
fn scale_of_ty(ty: Ty, v: &V) -> u16 {
    match (ty, v) {
        (Ty::Num { scale, .. }, _) => scale,
        (_, V::Num(_, scale)) => *scale,
        (Ty::Money, _) | (_, V::Money(_)) => 4,
        _ => 0,
    }
}

/// `x op y` on exact numbers at scales `sx` and `sy`, the result × 10^`s`;
/// truncated, or rounded half away from zero where `round`.
fn exact(op: BinOp, x: &V, sx: u16, y: &V, sy: u16, s: u16, round: bool) -> Result<i128, SqlError> {
    let a = x.as_scaled(sx).unwrap_or(0);
    let b = y.as_scaled(sy).unwrap_or(0);
    let quotient = |n: i128, d: i128| -> i128 {
        let q = n / d;
        match round && (n % d).abs() * 2 >= d.abs() {
            true => q + if (n < 0) != (d < 0) { -1 } else { 1 },
            false => q,
        }
    };
    let rescale = |n: i128, from: i32| -> i128 {
        let shift = i32::from(s) - from;
        if shift >= 0 {
            n.saturating_mul(pow10(shift as u16))
        } else {
            quotient(n, pow10((-shift) as u16))
        }
    };
    Ok(match op {
        BinOp::Add => rescale(a, i32::from(sx)) + rescale(b, i32::from(sy)),
        BinOp::Sub => rescale(a, i32::from(sx)) - rescale(b, i32::from(sy)),
        BinOp::Mul => rescale(a.saturating_mul(b), i32::from(sx) + i32::from(sy)),
        BinOp::Div | BinOp::Mod if b == 0 => return Err(SqlError::division_by_zero()),
        BinOp::Div => {
            // a × 10^(s - sx + sy) / b.
            let shift = i32::from(s) - i32::from(sx) + i32::from(sy);
            if shift >= 0 {
                // A dividend too wide for the shift gives its zeros to the
                // divisor first.
                let (mut b, mut shift) = (b, shift as u16);
                while a.checked_mul(pow10(shift)).is_none() && shift > 0 && b % 10 == 0 {
                    b /= 10;
                    shift -= 1;
                }
                quotient(a.saturating_mul(pow10(shift)), b)
            } else {
                quotient(a / pow10((-shift) as u16), b)
            }
        }
        BinOp::Mod => {
            let common = sx.max(sy);
            let (a, b) = (rescale_to(a, sx, common), rescale_to(b, sy, common));
            rescale(a % b, i32::from(common))
        }
        _ => 0,
    })
}

fn rescale_to(n: i128, from: u16, to: u16) -> i128 {
    if to >= from {
        n.saturating_mul(pow10(to - from))
    } else {
        n / pow10(from - to)
    }
}

/// A branch's value in the merged type of `CASE`, `IIF` and `COALESCE`.
fn convert_to(v: V, from: Ty, to: Ty, pos: crate::error::Pos) -> Result<V, SqlError> {
    Ok(match (v, to) {
        (V::Null, _) => V::Null,
        (v, Ty::Num { precision, scale }) if from != to && from.is_numeric() => {
            let n = v.as_scaled(scale).unwrap_or(0);
            // The merged type may be narrower than a branch's value.
            if n.unsigned_abs() >= pow10(precision) as u128 {
                return Err(SqlError::numeric_overflow().located_at(pos));
            }
            V::Num(n, scale)
        }
        (v, Ty::Double | Ty::CurDouble) if from.is_numeric() => {
            V::Double(v.as_f64().unwrap_or(0.0))
        }
        (v, Ty::Money) if from.is_numeric() => V::Money(v.as_scaled(4).unwrap_or(0) as i64),
        (V::Date(d), Ty::TimeStamp) => V::TimeStamp(d, 0),
        (v, _) => v,
    })
}

/// `CAST` and `CONVERT`: `v` as type `to`.
/// The value `v` of the parameter at `pos` where the type `ty` is wanted,
/// which the original engine converts when it runs the statement, with
/// errors of its own for what does not convert
/// (`fixtures/sql/params3.json`).
pub(crate) fn param(v: V, ty: Ty, pos: crate::error::Pos, env: &Env) -> Result<V, SqlError> {
    match (&v, ty) {
        // More decimals than a number it meets holds: the value is cut and
        // the statement fails with it.
        (V::Str(_) | V::Double(_), Ty::Num { scale, .. })
            if crate::value::cut_decimals(&v, scale).is_some() =>
        {
            Err(SqlError::data_truncated())
        }
        // A double too large for the digits of a NUMERIC.
        (V::Double(d), Ty::Num { precision, scale })
            if d.is_finite() && d.abs() * pow10(scale) as f64 >= pow10(precision) as f64 =>
        {
            Err(SqlError::numeric_overflow().located_at(pos))
        }
        // No text is no date, and no match.
        (V::Str(s), Ty::Date | Ty::TimeStamp) if s.trim().is_empty() => Ok(V::Null),
        (V::Str(s), Ty::TimeStamp) => {
            let (d, t) = crate::compile::timestamp_text(s, &env.date_format, env.epoch())
                .ok_or_else(SqlError::bad_literal)?;
            Ok(V::TimeStamp(d, t))
        }
        // Text where a date is, in the connection's format or ISO.
        (V::Str(s), Ty::Date) => Ok(V::Date(
            text_date(s, env).ok_or_else(SqlError::bad_literal)?,
        )),
        (V::Str(s), Ty::Int { .. } | Ty::Bool | Ty::Num { .. } | Ty::Double | Ty::Money) => {
            param_number(s, ty, pos)
        }
        (V::Int(_) | V::Double(_) | V::Bool(_), Ty::Date | Ty::Time | Ty::TimeStamp) => {
            Err(SqlError::parameter_conversion(pos))
        }
        (V::Money(_), Ty::Date | Ty::Time | Ty::TimeStamp) => Err(SqlError::bad_literal()),
        _ => Ok(v),
    }
}

/// The text `s` of a parameter at `pos` as the number of the type `to`.
fn param_number(s: &str, to: Ty, pos: crate::error::Pos) -> Result<V, SqlError> {
    let text = s.trim();
    match to {
        Ty::Int { .. } | Ty::Bool => {
            let n = match text {
                "" => 0,
                _ => text.parse::<i64>().map_err(|_| {
                    // A number with decimals is one too large for a whole
                    // number's place.
                    match text.parse::<f64>().is_ok()
                        && text.contains('.')
                        && !text.contains(['e', 'E'])
                    {
                        true => SqlError::parameter_not_whole(pos),
                        false => SqlError::parameter_not_integer(pos),
                    }
                })?,
            };
            match to {
                Ty::Bool => Ok(V::Bool(n == 1)),
                Ty::Int { width, .. } => {
                    let fits = match width {
                        IntWidth::Short => i16::try_from(n).is_ok(),
                        IntWidth::Int => i32::try_from(n).is_ok(),
                        IntWidth::Long => true,
                    };
                    match fits {
                        true => Ok(V::Int(n)),
                        false => Err(SqlError::out_of_range()),
                    }
                }
                _ => unreachable!("an integer or a logical"),
            }
        }
        Ty::Num { precision, scale } => match text {
            "" => Ok(V::Num(0, scale)),
            _ => {
                let n = parse_exact(text, scale).ok_or_else(|| SqlError::parameter_malformed(s))?;
                match n.unsigned_abs() >= pow10(precision) as u128 {
                    true => Err(SqlError::numeric_overflow().located_at(pos)),
                    false => Ok(V::Num(n, scale)),
                }
            }
        },
        Ty::Money => match text {
            "" => Ok(V::Money(0)),
            _ => parse_exact(text, 4)
                .map(|m| V::Money(m as i64))
                .ok_or_else(|| SqlError::parameter_malformed(s)),
        },
        _ => match text {
            "" => Ok(V::Double(0.0)),
            _ => text
                .parse()
                .map(V::Double)
                .map_err(|_| SqlError::assignment_error()),
        },
    }
}

/// `v`, of the type `ty`, as the logical an `AND`, `OR` or `NOT` makes of
/// it (`fixtures/sql/gram_cast2.json`): an integer or a double is true if it
/// is not 0, text if it is not empty (a CHARACTER column's is padded, so
/// never; an empty memo is NULL), and everything else is true.
fn truth(v: V, ty: Ty) -> V {
    V::Bool(match v {
        V::Null => return V::Null,
        V::Bool(b) => b,
        V::Int(n) => n != 0,
        V::Double(d) => d != 0.0,
        V::Str(s) => match ty {
            Ty::Str {
                field: FieldType::Character | FieldType::CiCharacter,
                ..
            } => true,
            Ty::Str {
                field: FieldType::Memo | FieldType::NMemo,
                ..
            } if s.is_empty() => return V::Null,
            _ => !s.is_empty(),
        },
        _ => true,
    })
}

fn cast(
    v: V,
    from: Ty,
    to: Ty,
    function: &str,
    pos: crate::error::Pos,
    param_at: Option<crate::error::Pos>,
    env: &Env,
) -> Result<V, SqlError> {
    if v.is_null() {
        return Ok(V::Null);
    }
    let from_param = param_at.is_some();
    // What a parameter's value converts to is its own business
    // (`fixtures/sql/params3.json`).
    if let Some(at) = param_at {
        match (&v, to) {
            (
                V::Str(s),
                Ty::Int { .. } | Ty::Bool | Ty::Num { .. } | Ty::Double | Ty::CurDouble | Ty::Money,
            ) => {
                let value = param_number(s, to, at)?;
                // A BIT is 0 or 1, though a condition takes any number.
                let wide = to == Ty::Bool && !matches!(s.trim(), "" | "0" | "1");
                return match wide {
                    true => Err(SqlError::out_of_range()),
                    false => Ok(value),
                };
            }
            (V::Int(_) | V::Double(_) | V::Bool(_), Ty::Date | Ty::Time | Ty::TimeStamp) => {
                return Err(SqlError::parameter_conversion(at));
            }
            (V::Money(_), Ty::Date | Ty::Time | Ty::TimeStamp) => {
                return Err(SqlError::bad_literal());
            }
            _ => {}
        }
    }
    let conversion = |what: &str| SqlError::conversion(what, function, pos);
    let cast_failed = || SqlError::cast_failed(function, pos);
    // A parameter that is no date, time or timestamp is a bad literal
    // (`fixtures/sql/params2.json`).
    let date_failed = || match from_param {
        true => SqlError::bad_literal(),
        false => cast_failed(),
    };
    // A date or a time is no number.
    if matches!(
        to,
        Ty::Int { .. } | Ty::Double | Ty::CurDouble | Ty::Num { .. } | Ty::Money
    ) && matches!(v, V::Date(_) | V::Time(_) | V::TimeStamp(..))
    {
        return Err(cast_failed());
    }
    Ok(match to {
        Ty::Int { width, .. } => match &v {
            V::Str(s) => {
                let s = s.trim();
                V::Int(match s.parse::<i64>() {
                    Ok(n) => n,
                    Err(_) if s.is_empty() => 0,
                    // Digits that do not fit, or decimals, are a value
                    // out of the type's range.
                    Err(_) if is_decimal_text(s) => return Err(SqlError::out_of_range()),
                    Err(_) => return Err(conversion("character string to integer")),
                })
            }
            // The first bytes, most significant first
            // (`fixtures/sql/gram_cast.json`).
            V::Bin(b) => {
                let size = match width {
                    IntWidth::Short => 2,
                    IntWidth::Int => 4,
                    IntWidth::Long => 8,
                };
                let Some(head) = b.get(..size) else {
                    return Err(SqlError::not_enough_binary(function, pos));
                };
                V::Int(match size {
                    2 => i64::from(i16::from_be_bytes([head[0], head[1]])),
                    4 => i64::from(i32::from_be_bytes([head[0], head[1], head[2], head[3]])),
                    _ => i64::from_be_bytes(head.try_into().expect("eight bytes")),
                })
            }
            V::Num(n, scale) if n % pow10(*scale) != 0 => return Err(SqlError::out_of_range()),
            V::Money(m) if m % 10_000 != 0 => return Err(SqlError::out_of_range()),
            V::Double(d) => V::Int(d.trunc() as i64),
            other => V::Int(other.as_i128().unwrap_or(0) as i64),
        },
        Ty::Num { .. } | Ty::Money if matches!(v, V::Bin(_)) => return Err(cast_failed()),
        Ty::Num { scale, .. } => match &v {
            V::Str(s) if s.trim().is_empty() => V::Num(0, scale),
            V::Str(s) => V::Num(parse_exact(s.trim(), scale).ok_or_else(cast_failed)?, scale),
            // An exact number loses the decimals the type has no room for.
            V::Num(..) | V::Money(_) | V::Int(_) => V::Num(v.as_scaled(scale).unwrap_or(0), scale),
            other => V::Num(rounded_scaled(other, scale), scale),
        },
        Ty::Double | Ty::CurDouble => match &v {
            V::Str(s) if s.trim().is_empty() => V::Double(0.0),
            V::Str(s) => V::Double(match is_float_text(s.trim()) {
                true => s.trim().parse().map_err(|_| cast_failed())?,
                false => return Err(cast_failed()),
            }),
            V::Bin(b) => match b.first_chunk::<8>() {
                Some(bytes) => V::Double(f64::from_be_bytes(*bytes)),
                None => return Err(cast_failed()),
            },
            other => V::Double(other.as_f64().unwrap_or(0.0)),
        },
        Ty::Money => match &v {
            V::Str(s) if s.trim().is_empty() => V::Money(0),
            V::Str(s) => V::Money(parse_exact(s.trim(), 4).ok_or_else(cast_failed)? as i64),
            V::Num(..) | V::Money(_) | V::Int(_) => V::Money(v.as_scaled(4).unwrap_or(0) as i64),
            other => V::Money(rounded_scaled(other, 4) as i64),
        },
        // Only 0 and 1 are a BIT's values, and text is read as a number
        // first (`fixtures/sql/gram_cast3.json`).
        Ty::Bool => match &v {
            V::Bool(b) => V::Bool(*b),
            V::Str(s) => match s.trim().parse::<i64>() {
                Ok(0) => V::Bool(false),
                Ok(1) => V::Bool(true),
                Ok(_) => return Err(SqlError::out_of_range()),
                Err(_) => return Err(conversion("character string to integer")),
            },
            other => match other.as_f64().unwrap_or(0.0) {
                0.0 => V::Bool(false),
                1.0 => V::Bool(true),
                _ => return Err(SqlError::out_of_range()),
            },
        },
        Ty::Date => match &v {
            V::Str(s) => V::Date(text_date(s, env).ok_or_else(date_failed)?),
            V::TimeStamp(d, _) => V::Date(*d),
            V::Date(_) => v.clone(),
            _ => return Err(cast_failed()),
        },
        Ty::Time => match &v {
            V::Str(s) => V::Time(time_text(s).ok_or_else(date_failed)?),
            V::TimeStamp(_, t) => V::Time(*t),
            V::Time(_) => v.clone(),
            _ => return Err(cast_failed()),
        },
        Ty::TimeStamp => match &v {
            V::Str(s) => {
                let (d, t) =
                    timestamp_text(s, &env.date_format, env.epoch()).ok_or_else(date_failed)?;
                V::TimeStamp(d, t)
            }
            V::Date(d) => V::TimeStamp(*d, 0),
            V::TimeStamp(..) => v.clone(),
            _ => return Err(cast_failed()),
        },
        Ty::Str { field, length } => {
            let date = |d: i32| {
                let ymd = tflip_storage::value::julian_to_ymd(d);
                format!("{:04}-{:02}-{:02}", ymd.year, ymd.month, ymd.day)
            };
            let time = |t: u32| {
                let seconds = t / 1000;
                format!(
                    "{:02}:{:02}:{:02}",
                    seconds / 3600,
                    seconds / 60 % 60,
                    seconds % 60
                )
            };
            let text = match &v {
                V::Str(s) => s.clone(),
                V::Date(d) => date(*d),
                V::Time(t) => time(*t),
                V::TimeStamp(d, t) => format!("{} {}.{:03}", date(*d), time(*t), t % 1000),
                V::Double(d) => format!("{d}"),
                V::Bin(b) => cp1252::decode(b),
                V::Bool(b) => String::from(if *b { "1" } else { "0" }),
                // A number below one has no zero before its point.
                V::Num(..) | V::Money(_) => {
                    let text = v.display();
                    match (text.strip_prefix("-0."), text.strip_prefix("0.")) {
                        (Some(rest), _) => format!("-.{rest}"),
                        (_, Some(rest)) => format!(".{rest}"),
                        _ => text,
                    }
                }
                other => other.display(),
            };
            let mut text: String = match to.is_memo() {
                // A memo has no length to cut to.
                true => text,
                false => text.chars().take(usize::from(length)).collect(),
            };
            if matches!(
                field,
                FieldType::Character | FieldType::CiCharacter | FieldType::NChar
            ) {
                let pad = usize::from(length).saturating_sub(text.chars().count());
                text.extend(std::iter::repeat_n(' ', pad));
            }
            V::Str(text)
        }
        Ty::Bin { field, length } => {
            let mut bytes = match v {
                V::Str(s) => cp1252::encode(&s),
                V::Bin(b) => b,
                // A number's bytes, most significant first, as wide as its
                // type.
                V::Int(n) => {
                    let size = match from {
                        Ty::Int {
                            width: IntWidth::Short,
                            ..
                        } => 2,
                        Ty::Int {
                            width: IntWidth::Long,
                            ..
                        } => 8,
                        _ => 4,
                    };
                    n.to_be_bytes()[8 - size..].to_vec()
                }
                V::Double(d) => d.to_be_bytes().to_vec(),
                V::Guid(g) => g.to_vec(),
                other => return Ok(other),
            };
            if field != FieldType::Binary {
                bytes.truncate(usize::from(length));
                if field == FieldType::Raw {
                    bytes.resize(usize::from(length), 0);
                }
            }
            V::Bin(bytes)
        }
        Ty::Guid => match &v {
            V::Str(s) => V::Guid(guid_bytes(s).ok_or_else(cast_failed)?),
            V::Guid(_) => v,
            _ => return Err(cast_failed()),
        },
        Ty::Unknown => v,
    })
}

/// A GUID's stored bytes from its text: `8-4-4-4-12` or 32 digits, with
/// braces round it or without; the first three groups are little-endian.
pub(crate) fn guid_bytes(text: &str) -> Option<[u8; 16]> {
    let bare = match (text.strip_prefix('{'), text.strip_suffix('}')) {
        (Some(_), Some(_)) => text.get(1..text.len() - 1)?,
        (None, None) => text,
        _ => return None,
    };
    let groups: Vec<&str> = bare.split('-').collect();
    let dashed = groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n);
    if !dashed && !(groups.len() == 1 && bare.len() == 32) {
        return None;
    }
    let hex: String = groups.concat();
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    bytes[0..4].reverse();
    bytes[4..6].reverse();
    bytes[6..8].reverse();
    Some(bytes)
}

/// Whether `text` is a number written with digits, a sign, a point and an
/// exponent only (`Inf` and `NaN` are no numbers to the engine).
fn is_float_text(text: &str) -> bool {
    text.bytes().any(|b| b.is_ascii_digit())
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'))
}

fn is_decimal_text(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().any(|b| b.is_ascii_digit())
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.'))
}

/// A cast integer that fits `width`, or the error of one that does not.
fn in_width(v: V, width: IntWidth) -> Result<V, SqlError> {
    let V::Int(n) = v else { return Ok(v) };
    let fits = match width {
        IntWidth::Short => i16::try_from(n).is_ok(),
        IntWidth::Int => i32::try_from(n).is_ok(),
        IntWidth::Long => true,
    };
    match fits {
        true => Ok(v),
        false => Err(SqlError::out_of_range()),
    }
}

/// A number × 10^scale, rounded half away from zero.
fn rounded_scaled(v: &V, scale: u16) -> i128 {
    match v {
        V::Double(d) => (d * pow10(scale) as f64).round() as i128,
        other => {
            // One more digit, then round on it.
            let more = other.as_scaled(scale + 1).unwrap_or(0);
            let (q, r) = (more / 10, more % 10);
            if r.abs() >= 5 { q + r.signum() } else { q }
        }
    }
}

/// `TRIM`: `chars` taken off one or both ends of `s`.
fn trim(s: &str, chars: &str, side: crate::ast::TrimSide) -> String {
    use crate::ast::TrimSide;
    if chars.is_empty() {
        return s.to_owned();
    }
    let mut s = s;
    if matches!(side, TrimSide::Leading | TrimSide::Both) {
        while let Some(rest) = s.strip_prefix(chars) {
            s = rest;
        }
    }
    if matches!(side, TrimSide::Trailing | TrimSide::Both) {
        while let Some(rest) = s.strip_suffix(chars) {
            s = rest;
        }
    }
    s.to_owned()
}

/// `value LIKE pattern`: `%` for any characters, `_` for one, `escape`
/// before either for itself. Trailing spaces of both are ignored.
pub fn like(value: &str, pattern: &str, escape: Option<char>, ci: bool) -> bool {
    let norm = |s: &str| -> Vec<char> {
        s.trim_end_matches(' ')
            .chars()
            .map(|c| if ci { c.to_ascii_uppercase() } else { c })
            .collect()
    };
    let value = norm(value);
    // The pattern as (char, literal) pairs.
    let mut items = Vec::new();
    let mut chars = norm(pattern).into_iter();
    while let Some(c) = chars.next() {
        if Some(c) == escape {
            if let Some(next) = chars.next() {
                items.push((next, true));
            }
        } else {
            items.push((c, false));
        }
    }
    matches_from(&value, &items)
}

fn matches_from(value: &[char], pattern: &[(char, bool)]) -> bool {
    match pattern.first() {
        None => value.is_empty(),
        Some(('%', false)) => (0..=value.len()).any(|i| matches_from(&value[i..], &pattern[1..])),
        Some(('_', false)) => !value.is_empty() && matches_from(&value[1..], &pattern[1..]),
        Some((c, _)) => value.first() == Some(c) && matches_from(&value[1..], &pattern[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_patterns() {
        assert!(like("abc", "a%", None, false));
        assert!(like("abc", "a_c", None, false));
        assert!(!like("abc", "ABC", None, false));
        assert!(like("abc", "ABC", None, true));
        assert!(like("abc ", "abc", None, false));
        assert!(like("abc", "abc ", None, false));
        assert!(like("", "%", None, false));
        assert!(like("a_c", "a!_c", Some('!'), false));
        assert!(!like("abc", "a!_c", Some('!'), false));
    }
}
