//! A live cursor's `WHERE` as the original engine rewrites it: an AOF, an
//! expression of the expression engine (`tflip_expr`), which its plan shows
//! (`AOF<id=2 .AND. name="Bravo">`, `fixtures/sql/plans.json`) and whose
//! orders find its records.
//!
//! Comparisons keep the column on the left (`2 = id` is `id=2`); text is
//! written in double quotes, dates as `STOD("CCYYMMDD")` and timestamps as
//! `STOTS("CCYYMMDD HH:MM:SS.SSS")`, a logical column alone as `col=.T.`.
//! `IN` becomes `.OR.`ed comparisons, `BETWEEN` two `.AND.`ed ones, `IS
//! NULL` `ISNULL()`, and `NOT` `.NOT.(…)`.

use tflip_storage::field::Field;

use crate::ast::BinOp;
use crate::compile::{FtsCondition, FtsSearch, FtsSource, Kind, Node};
use crate::func::Func;
use crate::types::Ty;
use crate::value::V;

/// The AOF the original engine makes of the condition `node` over a table
/// with `fields`, its parameters' values `params`; `None` for a condition
/// it cannot be (or tflip does not know how the original writes).
pub fn aof(node: &Node, fields: &[Field], params: &[V]) -> Option<String> {
    Writer { fields, params }.logical(node)
}

struct Writer<'a> {
    fields: &'a [Field],
    params: &'a [V],
}

impl Writer<'_> {
    /// A condition.
    fn logical(&self, node: &Node) -> Option<String> {
        Some(match &node.kind {
            Kind::Binary(BinOp::And, a, b) => {
                format!(
                    "{} .AND. {}",
                    self.operand(a, BinOp::And)?,
                    self.operand(b, BinOp::And)?
                )
            }
            Kind::Binary(BinOp::Or, a, b) => {
                format!(
                    "{} .OR. {}",
                    self.operand(a, BinOp::Or)?,
                    self.operand(b, BinOp::Or)?
                )
            }
            Kind::Not(inner) => format!(".NOT.({})", self.logical(inner)?),
            Kind::Binary(op, a, b) => self.comparison(*op, a, b)?,
            Kind::IsNull(inner, false) => format!("ISNULL({})", self.value(inner)?),
            Kind::IsNull(inner, true) => format!("(.NOT.ISNULL({}))", self.value(inner)?),
            Kind::Between(x, low, high, false) => format!(
                "{} .AND. {}",
                self.comparison(BinOp::Ge, x, low)?,
                self.comparison(BinOp::Le, x, high)?
            ),
            Kind::In(x, list, false) if !list.is_empty() => {
                let parts: Option<Vec<String>> = list
                    .iter()
                    .map(|item| self.comparison(BinOp::Eq, x, item))
                    .collect();
                parts?.join(" .OR. ")
            }
            Kind::Column(_) if node.ty == Ty::Bool => format!("{}=.T.", self.value(node)?),
            Kind::Contains(search) => contains(search, self.fields, "=.T.")?,
            _ => return None,
        })
    }

    /// An operand of `.AND.` or `.OR.` (`within`): an `.OR.` (or what
    /// becomes one) under an `.AND.` is parenthesized.
    fn operand(&self, node: &Node, within: BinOp) -> Option<String> {
        let text = self.logical(node)?;
        let or = matches!(node.kind, Kind::Binary(BinOp::Or, ..))
            || matches!(&node.kind, Kind::In(_, list, false) if list.len() > 1);
        Some(if within == BinOp::And && or {
            format!("({text})")
        } else {
            text
        })
    }

    fn comparison(&self, op: BinOp, a: &Node, b: &Node) -> Option<String> {
        let symbol = |op| match op {
            BinOp::Eq => Some("="),
            BinOp::Ne => Some("<>"),
            BinOp::Lt => Some("<"),
            BinOp::Le => Some("<="),
            BinOp::Gt => Some(">"),
            BinOp::Ge => Some(">="),
            _ => None,
        };
        // The column goes on the left.
        let (op, a, b) = match (column(a), column(b)) {
            (false, true) => {
                let flipped = match op {
                    BinOp::Lt => BinOp::Gt,
                    BinOp::Le => BinOp::Ge,
                    BinOp::Gt => BinOp::Lt,
                    BinOp::Ge => BinOp::Le,
                    op => op,
                };
                (flipped, b, a)
            }
            _ => (op, a, b),
        };
        // Text compared with a date or a timestamp is one.
        let converted = match (&a.ty, &b.kind) {
            (Ty::Date, Kind::Const(V::Str(s))) => {
                Some(V::Date(crate::compile::strict_iso_date(s)?))
            }
            (Ty::TimeStamp, Kind::Const(V::Str(s))) => {
                let (day, ms) = crate::compile::timestamp_text(
                    s,
                    "MM/DD/CCYY",
                    tflip_expr::text::DEFAULT_EPOCH,
                )?;
                Some(V::TimeStamp(day, ms))
            }
            _ => None,
        };
        let value = match converted {
            Some(v) => constant(&v)?,
            None => self.value(b)?,
        };
        Some(format!("{}{}{value}", self.value(a)?, symbol(op)?))
    }

    /// A value: a column, a constant, arithmetic or a function the
    /// expression engine has too.
    fn value(&self, node: &Node) -> Option<String> {
        Some(match &node.kind {
            Kind::Column(i) => self.fields.get(*i)?.name.clone(),
            Kind::Const(v) => constant(v)?,
            Kind::Param(i) => constant(self.params.get(*i)?)?,
            Kind::Neg(inner) => format!("-{}", self.value(inner)?),
            Kind::Binary(op, a, b) => {
                let symbol = match op {
                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                    BinOp::Mul => "*",
                    BinOp::Div => "/",
                    _ => return None,
                };
                format!("{}{symbol}{}", self.value(a)?, self.value(b)?)
            }
            Kind::Func(Func::CurDate, args) if args.is_empty() => "DATE()".to_owned(),
            Kind::Func(func @ (Func::Upper | Func::Lower), args) if args.len() == 1 => {
                let name = if *func == Func::Upper {
                    "UPPER"
                } else {
                    "LOWER"
                };
                format!("{name}({})", self.value(&args[0])?)
            }
            _ => return None,
        })
    }
}

fn column(node: &Node) -> bool {
    matches!(node.kind, Kind::Column(_))
}

/// A `CONTAINS` as the original engine writes it in an AOF, compared as
/// `compared` says (`=.T.` in a live cursor's, `=1` in a static one's):
/// `CONTAINS(field,"condition")`, `*` for every full-text field, the condition
/// in single quotes if it has a double quote (in brackets if it has both), or
/// a column's name. `None` for one tflip does not know how it writes.
pub fn contains(search: &FtsSearch, fields: &[Field], compared: &str) -> Option<String> {
    let field = match &search.source {
        FtsSource::Fields(_) => "*".to_owned(),
        FtsSource::Expr(node) => match node.kind {
            Kind::Column(i) => fields.get(i)?.name.clone(),
            _ => return None,
        },
    };
    let condition = match &search.condition {
        FtsCondition::Const(_) | FtsCondition::Invalid(_) => {
            let text = &search.condition_src;
            match (text.contains('\''), text.contains('"')) {
                (true, true) => format!("[{text}]"),
                (false, true) => format!("'{text}'"),
                (_, false) => format!("\"{text}\""),
            }
        }
        FtsCondition::Dynamic(node) => match node.kind {
            Kind::Column(i) => fields.get(i)?.name.clone(),
            _ => return None,
        },
        FtsCondition::Null => return None,
    };
    Some(format!("CONTAINS({field},{condition}){compared}"))
}

/// Whether the original engine cannot set the AOF of the condition `node`: a
/// `CONTAINS` of it has a condition that does not parse, or has a single quote
/// (which its expression engine does not take inside the condition).
pub fn unsettable(node: &Node) -> bool {
    node.any(&|n| match &n.kind {
        Kind::Contains(search) => {
            matches!(search.condition, FtsCondition::Invalid(_))
                || matches!(search.condition, FtsCondition::Const(_))
                    && search.condition_src.contains('\'')
        }
        _ => false,
    })
}

/// The `CONTAINS` terms of the condition `node` that [`aof`] writes, each with
/// its text there.
pub fn contains_terms<'n>(node: &'n Node, fields: &[Field]) -> Vec<(String, &'n FtsSearch)> {
    let mut out = Vec::new();
    fn walk<'n>(node: &'n Node, fields: &[Field], out: &mut Vec<(String, &'n FtsSearch)>) {
        match &node.kind {
            Kind::Binary(BinOp::And | BinOp::Or, a, b) => {
                walk(a, fields, out);
                walk(b, fields, out);
            }
            Kind::Not(inner) => walk(inner, fields, out),
            Kind::Contains(search) => {
                if let Some(text) = contains(search, fields, "=.T.") {
                    out.push((text, search));
                }
            }
            _ => {}
        }
    }
    walk(node, fields, &mut out);
    out
}

/// A constant as the original engine writes it in an AOF.
pub(crate) fn constant(v: &V) -> Option<String> {
    use tflip_storage::value::julian_to_ymd;
    let date = |day: i32| {
        let ymd = julian_to_ymd(day);
        format!("{:04}{:02}{:02}", ymd.year, ymd.month, ymd.day)
    };
    Some(match v {
        V::Bool(true) => ".T.".to_owned(),
        V::Bool(false) => ".F.".to_owned(),
        V::Int(n) => n.to_string(),
        V::Num(n, scale) => decimal(*n, *scale),
        V::Money(n) => decimal(i128::from(*n), 4),
        V::Double(d) if d.is_finite() => d.to_string(),
        V::Str(s) if !s.contains('"') => format!("\"{s}\""),
        V::Date(day) => format!("STOD(\"{}\")", date(*day)),
        V::TimeStamp(day, ms) => {
            let (h, m, s, ms) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
            format!("STOTS(\"{} {h:02}:{m:02}:{s:02}.{ms:03}\")", date(*day))
        }
        _ => return None,
    })
}

/// `n` × 10^-`scale`, with `scale` decimals.
fn decimal(n: i128, scale: u16) -> String {
    if scale == 0 {
        return n.to_string();
    }
    let digits = format!(
        "{:0>width$}",
        n.unsigned_abs(),
        width = usize::from(scale) + 1
    );
    let (whole, fraction) = digits.split_at(digits.len() - usize::from(scale));
    format!("{}{whole}.{fraction}", if n < 0 { "-" } else { "" })
}
