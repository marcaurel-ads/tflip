//! Filters: a table's Advantage Optimized Filter (`AdsSetAOF`) and its
//! traditional record filter (`AdsSetFilter`), which hide the records that
//! fail them from moves and counts, and the expressions they are, evaluated
//! with [`tflip_expr`] on the table's records. A live SQL cursor also has
//! its `WHERE` condition, which clients do not see as a filter.
//!
//! A record filter is evaluated on each record when a move or count reaches
//! it. An AOF is optimized as the original engine optimizes it, with the
//! index orders open on the table ([`tflip_expr::optimize`]), and keeps what
//! the original engine keeps for it (`Members`): records it found to be
//! in, and candidates it has yet to evaluate. As the original engine's
//! (`fixtures/write/aofs`), an AOF follows the table's writes, every
//! handle's in the process, and so shows what evaluating it on each record
//! shows; but a fixed AOF (`ADS_FIXED_AOF`) or one a client customized
//! (`AdsCustomizeAOF`) keeps the records it had, and an AOF resolved at once
//! or fully optimized counts the records it had then.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tflip_expr::optimize::{Bound, Level};
use tflip_expr::{Env, Expr, Failure, Row, Runtime};
use tflip_sql::{Ty, V};
use tflip_storage::field::{Field, FieldType};
use tflip_storage::memo::MemoFile;
use tflip_storage::value::Value;

use crate::Error;

/// A table's filters.
#[derive(Debug, Clone, Default)]
pub(crate) struct Filters {
    pub aof: Option<Aof>,
    pub filter: Option<Filter>,
    /// A live cursor's `WHERE`.
    pub sql: Option<Arc<SqlFilter>>,
    /// An AOF set aside by `AdsDeactivateAOF`, which hides no records until
    /// `AdsActivateAOF` puts it back.
    pub parked: Option<Aof>,
}

impl Filters {
    pub fn is_empty(&self) -> bool {
        self.aof.is_none() && self.filter.is_none() && self.sql.is_none()
    }
}

/// A live cursor's `WHERE` condition, over the fields of the table the
/// cursor reads (the cursor may show fewer).
#[derive(Debug)]
pub(crate) struct SqlFilter {
    pub fields: Vec<Field>,
    pub condition: tflip_sql::Node,
    pub env: tflip_sql::Env,
    /// The condition's value if it does not depend on the record.
    pub constant: Option<bool>,
    /// Records outside which it holds for none, as long as the table file
    /// has the first number of changes (as an AOF's superset).
    pub superset: Option<(u64, Bits)>,
    /// The records the original engine counts for a `WHERE` of full-text
    /// searches its orders answer with nothing left to evaluate: those their
    /// keys find, deleted records' included (see [`crate::statement`]'s
    /// `fts_count`); `None` for any other `WHERE`, whose records are counted
    /// one by one.
    pub count: Option<(u64, u32)>,
}

impl SqlFilter {
    /// Whether the record in `bytes` (record number `number`) passes the
    /// condition. Dividing by zero fails, as in the original's expression
    /// engine, whichever operand of an `AND` does it.
    pub fn holds(
        &self,
        bytes: &[u8],
        number: u32,
        memo: Option<&mut MemoFile>,
    ) -> Result<bool, Error> {
        if let Some(constant) = self.constant {
            return Ok(constant);
        }
        let mut row = SqlRow {
            fields: &self.fields,
            bytes,
            memo,
            record: Some(number),
            hidden: &[],
        };
        match tflip_sql::eval_filter(&self.condition, &mut row, &self.env) {
            Ok(tflip_sql::V::Bool(b)) => Ok(b),
            Ok(_) => Ok(false),
            Err(e) if e.native == crate::statement::READ_FAILED => Err(Error::CorruptTable),
            Err(e) if e.native == 4120 => {
                Err(Error::Evaluation(tflip_expr::Runtime::DivideInFilter))
            }
            Err(_) => Ok(false),
        }
    }
}

/// 1899-12-30, the day a VFP table's blank date is in SQL: day 0 of the OLE
/// dates.
const DAY_ZERO: i32 = 2_415_019;

/// A record as SQL expressions read it, and its number if it has one.
pub(crate) struct SqlRow<'a> {
    pub fields: &'a [Field],
    pub bytes: &'a [u8],
    pub memo: Option<&'a mut MemoFile>,
    pub record: Option<u32>,
    /// The columns the user may not read, which read as NULL.
    pub hidden: &'a [String],
}

impl SqlRow<'_> {
    /// Points the memo file at this record's number, which an encrypted
    /// table's memo values are enciphered at.
    fn at_record(&mut self) {
        if let (Some(memo), Some(number)) = (self.memo.as_deref_mut(), self.record) {
            memo.decipher_record(number);
        }
    }

    fn hides(&self, field: &Field) -> bool {
        !self.hidden.is_empty()
            && self
                .hidden
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&field.name))
    }
}

impl tflip_sql::Row for SqlRow<'_> {
    fn value(&mut self, index: usize) -> Result<Value, tflip_sql::SqlError> {
        let field = &self.fields[index];
        if self.hides(field) {
            return Ok(Value::Null);
        }
        self.at_record();
        field
            .value(self.bytes, self.memo.as_deref_mut())
            .map_err(|_| crate::statement::read_failed())
    }

    /// An empty field of a Clipper or FoxPro table (blank, 0, false or
    /// spaces) is [`V::Blank`]: NULL to `IS NULL`, but its value to
    /// expressions; a blank one computes as its blank value and shows as
    /// NULL, but text, doubles and integers. A VFP table's blank date is
    /// 1899-12-30 (`fixtures/sql/dbf.json`).
    fn column(&mut self, index: usize, ty: Ty) -> Result<V, tflip_sql::SqlError> {
        let field = &self.fields[index];
        if self.hides(field) {
            return Ok(V::Null);
        }
        let read = |value: Result<Value, _>| value.map_err(|_| crate::statement::read_failed());
        self.at_record();
        let value = read(field.value(self.bytes, self.memo.as_deref_mut()))?;
        if !field.blank_is_null() {
            return Ok(match value {
                Value::Blank if ty == Ty::Date => V::Date(DAY_ZERO),
                value => V::from_field(value, ty),
            });
        }
        let blank = matches!(value, Value::Null);
        let value = match blank {
            true => match read(field.expression_value(self.bytes, self.memo.as_deref_mut()))? {
                Value::Blank if ty == Ty::Date => V::Date(0),
                value => V::from_field(value, ty),
            },
            false => V::from_field(value, ty),
        };
        if value.is_null() || !value.is_empty() {
            return Ok(value);
        }
        let shows_value = matches!(
            field.field_type,
            FieldType::Character | FieldType::Double | FieldType::Integer | FieldType::AutoInc
        );
        let shown = if blank && !shows_value {
            V::Null
        } else {
            value.clone()
        };
        Ok(V::Blank(Box::new(tflip_sql::Blank { value, shown })))
    }

    fn record(&mut self, _index: usize) -> Option<u32> {
        self.record
    }

    /// A deleted record of a DBF table starts with `*`.
    fn deleted(&mut self, _index: usize) -> bool {
        self.bytes.first() == Some(&b'*')
    }
}

/// A filter: its text, as the client sees it again, and its expression.
#[derive(Debug, Clone)]
pub(crate) struct Filter {
    pub text: String,
    pub expr: Arc<Expr>,
}

/// How a client asked for an AOF (`AdsSetAOF`'s options).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AofOptions {
    /// `ADS_RESOLVE_IMMEDIATE`: every record is evaluated when the AOF is
    /// set (else `ADS_RESOLVE_DYNAMIC`: as moves reach them).
    pub immediate: bool,
    /// `ADS_FIXED_AOF`: the records in the AOF do not change.
    pub fixed: bool,
}

/// An AOF: its expression, how it was asked for, how far it is optimized,
/// and its records.
#[derive(Debug, Clone)]
pub(crate) struct Aof {
    pub filter: Filter,
    pub options: AofOptions,
    pub level: Level,
    /// The part not optimized, as `AdsGetAOFOptLevel` reports it.
    pub residual: String,
    /// What the orders find of its records.
    pub bound: Arc<Bound>,
    pub members: Arc<Mutex<Members>>,
    /// Whether setting another AOF failed since: as in the original engine,
    /// moves still apply it, but the table has no AOF for anything else
    /// (its text, level, count or records).
    pub orphaned: bool,
}

impl Aof {
    /// Whether the AOF knows its records as soon as it is set: resolved at
    /// once, or found by the orders.
    pub fn exact(&self) -> bool {
        self.options.immediate || self.level == Level::Full
    }
}

/// What an AOF keeps of its records, as the original engine keeps it: the
/// records it has yet to evaluate (candidates the orders could not rule
/// out), and, once it no longer follows writes (fixed or customized), the
/// records in it. A record that is neither is out.
#[derive(Debug, Default)]
pub(crate) struct Members {
    /// Whether writes no longer change the records in the AOF.
    pub frozen: bool,
    /// The records in it, when it is frozen.
    pub inside: Bits,
    /// The candidates not evaluated yet.
    pub unresolved: Bits,
    /// The records deleted when the AOF was set, which are not in it (one
    /// deleted since stays in, as far as `AdsIsRecordInAOF` tells).
    pub gone: Bits,
    /// Records outside which none is in the AOF, as long as the table file's
    /// [`changes`](tflip_storage::table::TableFile::changes) are the first
    /// number: the records it found when it was set, for moves to pass the
    /// others by without reading them.
    pub superset: Option<(u64, Bits)>,
    /// The number of records the AOF counted when it was set, customized or
    /// refreshed, which `AdsGetRecordCount` reports, writes since
    /// notwithstanding; `None` for one that counts anew each time.
    pub count: Option<u32>,
}

/// A set of record numbers.
#[derive(Debug, Default, Clone)]
pub(crate) struct Bits(Vec<u64>);

impl Bits {
    /// The set of `records`.
    pub fn of(records: &[u32]) -> Bits {
        let mut bits = Bits::default();
        records.iter().for_each(|&record| bits.set(record, true));
        bits
    }

    /// The records in both.
    pub fn and(&self, other: &Bits) -> Bits {
        Bits(self.0.iter().zip(&other.0).map(|(a, b)| a & b).collect())
    }

    /// The records in either.
    pub fn or(&self, other: &Bits) -> Bits {
        let (long, short) = if self.0.len() >= other.0.len() {
            (self, other)
        } else {
            (other, self)
        };
        let mut out = long.clone();
        for (word, other) in out.0.iter_mut().zip(&short.0) {
            *word |= other;
        }
        out
    }

    /// How many records it holds.
    pub fn count(&self) -> u32 {
        self.0.iter().map(|word| word.count_ones()).sum()
    }

    /// Its records, in order.
    pub fn records(&self) -> impl Iterator<Item = u32> + '_ {
        self.0.iter().enumerate().flat_map(|(i, &word)| {
            (0..64)
                .filter(move |bit| word & (1 << bit) != 0)
                .map(move |bit| (i * 64 + bit) as u32)
        })
    }

    pub fn get(&self, n: u32) -> bool {
        let n = n as usize;
        self.0
            .get(n / 64)
            .is_some_and(|word| word & (1 << (n % 64)) != 0)
    }

    pub fn set(&mut self, n: u32, on: bool) {
        let n = n as usize;
        if self.0.len() <= n / 64 {
            if !on {
                return;
            }
            self.0.resize(n / 64 + 1, 0);
        }
        if on {
            self.0[n / 64] |= 1 << (n % 64);
        } else {
            self.0[n / 64] &= !(1 << (n % 64));
        }
    }
}

/// The types of an [`Evaluation`]: an expression's type and value, and
/// how numbers are checked against their width.
pub use tflip_expr::{Type, Val, number};

/// The result of evaluating an expression on the current record
/// (`AdsEval*Expr`): its type, known as soon as the expression compiles,
/// and its value, which needs a current record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evaluation {
    pub ty: Type,
    pub value: Result<Val, Error>,
}

/// What decides whether a record is visible: the table's filters as a call
/// finds them, and what they are evaluated with. Taken before the table's
/// state is locked.
pub(crate) struct Sieve<'t> {
    pub fields: &'t [Field],
    /// The table's record length, for reading records.
    pub record_length: usize,
    pub filters: Arc<Filters>,
    pub env: Env,
}

impl Sieve<'_> {
    /// Whether the record in `bytes` (record number `number`) passes the
    /// AOF and the filter.
    pub fn passes(
        &self,
        bytes: Vec<u8>,
        number: u32,
        memo: Option<&mut MemoFile>,
    ) -> Result<bool, Error> {
        if self.filters.is_empty() {
            return Ok(true);
        }
        let mut memo = memo;
        if let Some(sql) = &self.filters.sql
            && !sql.holds(&bytes, number, memo.as_deref_mut())?
        {
            return Ok(false);
        }
        let mut row = Record {
            fields: self.fields,
            bytes,
            memo,
            number,
        };
        if let Some(aof) = &self.filters.aof
            && !aof_holds(aof, &mut row, &self.env)?
        {
            return Ok(false);
        }
        match &self.filters.filter {
            Some(filter) => {
                filter_holds(&filter.expr, &mut row, &self.env, Runtime::DivideInFilter)
            }
            None => Ok(true),
        }
    }

    /// Whether the record is in the AOF (`AdsIsRecordInAOF`): as the
    /// original engine answers, a candidate not evaluated yet is.
    pub fn in_aof(
        &self,
        bytes: Vec<u8>,
        number: u32,
        memo: Option<&mut MemoFile>,
    ) -> Result<bool, Error> {
        let Some(aof) = &self.filters.aof else {
            return Err(Error::NoFilter);
        };
        {
            let members = aof.members.lock().unwrap_or_else(|e| e.into_inner());
            if members.unresolved.get(number) {
                return Ok(true);
            }
            if members.frozen {
                return Ok(members.inside.get(number));
            }
        }
        let mut row = Record {
            fields: self.fields,
            bytes,
            memo,
            number,
        };
        filter_holds(&aof.filter.expr, &mut row, &self.env, Runtime::DivideInKey)
    }

    /// Whether the AOF certainly leaves record `number` out, without reading
    /// it: a frozen AOF's records are known, and those of another are among
    /// what it found as long as the table file has its `changes`.
    pub fn excludes(&self, number: u32, changes: u64) -> bool {
        if let Some(sql) = &self.filters.sql
            && let Some((at, superset)) = &sql.superset
            && *at == changes
            && !superset.get(number)
        {
            return true;
        }
        let Some(aof) = &self.filters.aof else {
            return false;
        };
        let members = aof.members.lock().unwrap_or_else(|e| e.into_inner());
        if members.frozen {
            return !members.inside.get(number) && !members.unresolved.get(number);
        }
        match &members.superset {
            Some((at, superset)) => *at == changes && !superset.get(number),
            None => false,
        }
    }

    /// Whether the record passes the AOF's expression itself.
    pub fn holds(
        &self,
        expr: &Expr,
        bytes: Vec<u8>,
        number: u32,
        memo: Option<&mut MemoFile>,
    ) -> Result<bool, Error> {
        let mut row = Record {
            fields: self.fields,
            bytes,
            memo,
            number,
        };
        filter_holds(expr, &mut row, &self.env, Runtime::DivideInKey)
    }
}

/// Whether a record passes an AOF: as its expression says, unless it is
/// frozen, when as it said then (evaluating a candidate the first time).
fn aof_holds(aof: &Aof, row: &mut Record, env: &Env) -> Result<bool, Error> {
    let mut members = aof.members.lock().unwrap_or_else(|e| e.into_inner());
    if members.frozen && !members.unresolved.get(row.number) {
        return Ok(members.inside.get(row.number));
    }
    let holds = filter_holds(&aof.filter.expr, row, env, Runtime::DivideInKey)?;
    members.unresolved.set(row.number, false);
    if members.frozen {
        members.inside.set(row.number, holds);
    }
    Ok(holds)
}

/// Whether a filter holds on `row`. A traditional filter need not be
/// logical: as in the original engine, a number passes unless it is 0, and
/// anything else that is not NULL passes. Dividing by 0 fails the call with
/// `division` (the error's code depends on what the expression is for).
fn filter_holds(
    expr: &Expr,
    row: &mut Record,
    env: &Env,
    division: Runtime,
) -> Result<bool, Error> {
    Ok(match expr.eval(row, env) {
        Ok(Val::Logical(b)) => b,
        Ok(Val::Number(n)) => n != 0.0,
        Ok(Val::Null) => false,
        Ok(_) => true,
        Err(Failure::Row(error)) => return Err(error),
        Err(Failure::Runtime(Runtime::DivideByZero)) => return Err(Error::Evaluation(division)),
        Err(Failure::Runtime(_)) => false,
    })
}

/// A record of an ADT table, as expressions read it.
pub(crate) struct Record<'a> {
    pub fields: &'a [Field],
    pub bytes: Vec<u8>,
    pub memo: Option<&'a mut MemoFile>,
    pub number: u32,
}

impl Row for Record<'_> {
    type Error = Error;

    fn value(&mut self, index: usize) -> Result<Value, Error> {
        let field = &self.fields[index];
        if let Some(memo) = self.memo.as_deref_mut() {
            memo.decipher_record(self.number);
        }
        field
            .expression_value(&self.bytes, self.memo.as_deref_mut())
            .map_err(|_| Error::CorruptTable)
    }

    fn record_number(&self) -> u32 {
        self.number
    }

    /// Whether a DBF record is deleted (its flag `*`, or `D` encrypted). An
    /// ADT table's deleted records are never visible, so expressions only
    /// see active ones there.
    fn deleted(&self) -> bool {
        match self.fields.first().map(|field| &field.layout) {
            Some(tflip_storage::field::Layout::Dbf(_)) => self
                .bytes
                .first()
                .is_some_and(|&flag| matches!(flag, b'*' | b'D')),
            _ => false,
        }
    }
}

/// An ADT table's fields, as expressions name them.
pub(crate) struct Schema<'a> {
    pub alias: &'a str,
    pub fields: &'a [Field],
}

impl tflip_expr::Schema for Schema<'_> {
    fn alias(&self) -> &str {
        self.alias
    }

    /// As the original engine prints it: as an ADT table names it, in
    /// capitals in a DBF table (VFP tables keep them in lower case).
    fn field_name(&self, index: usize) -> Option<String> {
        let field = self.fields.get(index)?;
        Some(match field.layout {
            tflip_storage::field::Layout::Dbf(_) => field.name.to_ascii_uppercase(),
            _ => field.name.clone(),
        })
    }

    fn field(&self, name: &str) -> Option<tflip_expr::Field> {
        let index = self
            .fields
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(name))?;
        let field = &self.fields[index];
        Some(tflip_expr::Field {
            index,
            field_type: field.field_type,
            length: field.length(),
            decimals: field.decimals(),
        })
    }
}
