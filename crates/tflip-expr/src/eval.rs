//! Evaluating a checked expression on a record.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use tflip_storage::cp1252;
use tflip_storage::field::FieldType;
use tflip_storage::key;
use tflip_storage::value::Value;

use crate::check::{Kind, Node};
use crate::func::{self, Func};
use crate::lex::Op;
use crate::{Env, Failure, Field, Row, Runtime, Type};

/// A value in an expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Val {
    Null,
    Logical(bool),
    Number(f64),
    /// A Julian day; 0 is the empty date.
    Date(i32),
    /// Windows-1252 text.
    Str(Vec<u8>),
    Binary(Vec<u8>),
    /// Milliseconds since midnight.
    Time(u32),
    /// A Julian day and milliseconds since midnight.
    TimeStamp(i32, u32),
}

impl Val {
    /// The value's bytes as the original engine returns them from
    /// `AdsEvalStringExpr` and joins them with `;`: text and binary data as
    /// they are, other types as their index keys ([`tflip_storage::key`]),
    /// and NULL as `width` zero bytes. `None` for numbers and logical values
    /// outside a join.
    pub fn bytes(&self, ty: Type) -> Option<Vec<u8>> {
        Some(match self {
            Val::Null => vec![0; usize::from(ty.width())],
            Val::Str(bytes) | Val::Binary(bytes) => bytes.clone(),
            Val::Date(julian) => key::double(f64::from(*julian)).to_vec(),
            Val::Time(ms) => key::time(*ms).to_vec(),
            Val::TimeStamp(julian, ms) => key::timestamp(*julian, *ms).to_vec(),
            Val::Number(_) | Val::Logical(_) => return None,
        })
    }

    /// The value's key in a `;` join: as [`Val::bytes`], with numbers as
    /// their double keys, logical values as `T` or `F`, and text padded to
    /// `ty`'s width.
    fn join_key(&self, ty: Type) -> Vec<u8> {
        match self {
            Val::Number(n) => key::double(*n).to_vec(),
            Val::Logical(b) => vec![if *b { b'T' } else { b'F' }],
            Val::Str(bytes) => {
                let mut bytes = bytes.clone();
                bytes.resize(usize::from(ty.width()).max(bytes.len()), b' ');
                bytes
            }
            _ => self.bytes(ty).unwrap_or_default(),
        }
    }
}

/// The value of a field as expressions see it: CICharacter text in lower
/// case (ASCII letters only), VarBinary data after a byte 0x80, and a blank
/// VFP date, timestamp or logical as an empty one.
fn field_value(field: &Field, value: Value) -> Val {
    match value {
        Value::Null => Val::Null,
        Value::Text(text) if field.field_type == FieldType::CiCharacter => {
            Val::Str(cp1252::encode(&text).to_ascii_lowercase())
        }
        Value::Text(text) => Val::Str(cp1252::encode(&text)),
        Value::Binary(bytes) if field.field_type == FieldType::VarBinary => {
            Val::Binary([&[0x80], bytes.as_slice()].concat())
        }
        Value::Binary(bytes) => Val::Binary(bytes),
        Value::Logical(b) => Val::Logical(b),
        Value::Numeric(text) => Val::Number(text.trim().parse().unwrap_or(0.0)),
        Value::Integer(n) => Val::Number(n as f64),
        Value::Double(n) => Val::Number(n),
        Value::Money(n) => Val::Number(n as f64 / 10_000.0),
        Value::Date(julian) => Val::Date(julian),
        Value::Time(ms) => Val::Time(ms),
        Value::TimeStamp(julian, ms) => Val::TimeStamp(julian, ms),
        Value::Guid(bytes) => Val::Binary(bytes.to_vec()),
        // A blank logical is false, a blank date the empty date; a blank
        // timestamp is empty.
        Value::Blank if field.field_type == FieldType::Logical => Val::Logical(false),
        Value::Blank if field.field_type == FieldType::Date => Val::Date(0),
        Value::Blank => Val::Null,
    }
}

pub(crate) fn eval<R: Row + ?Sized>(
    node: &Node,
    row: &mut R,
    env: &Env,
) -> Result<Val, Failure<R::Error>> {
    Ok(match &node.kind {
        Kind::Const(value) => value.clone(),
        Kind::Field(field) => field_value(field, row.value(field.index).map_err(Failure::Row)?),
        Kind::Binary(op, left, right) => {
            let l = eval(left, row, env)?;
            let r = eval(right, row, env)?;
            // `UPPER()` and `LOWER()` under a static collation give its
            // weights, which comparisons take as they are.
            let weighed = |node: &Node| {
                env.case_fold.is_some()
                    && matches!(node.kind, Kind::Call(Func::Upper | Func::Lower, _))
            };
            binary(
                *op,
                l,
                r,
                left.ty,
                right.ty,
                (weighed(left), weighed(right)),
                env,
            )?
        }
        Kind::And(left, right) => {
            let l = eval(left, row, env)?;
            let r = eval(right, row, env)?;
            match (l, r) {
                (Val::Logical(false), _) | (_, Val::Logical(false)) => Val::Logical(false),
                (Val::Logical(true), Val::Logical(true)) => Val::Logical(true),
                _ => Val::Null,
            }
        }
        Kind::Or(left, right) => {
            let l = eval(left, row, env)?;
            let r = eval(right, row, env)?;
            match (l, r) {
                (Val::Logical(true), _) | (_, Val::Logical(true)) => Val::Logical(true),
                (Val::Logical(false), Val::Logical(false)) => Val::Logical(false),
                _ => Val::Null,
            }
        }
        Kind::Not(operand) => match eval(operand, row, env)? {
            Val::Logical(b) => Val::Logical(!b),
            _ => Val::Null,
        },
        Kind::Join(left, right) => {
            let mut bytes = eval(left, row, env)?.join_key(left.ty);
            bytes.extend(eval(right, row, env)?.join_key(right.ty));
            Val::Binary(bytes)
        }
        Kind::Call(func, args) => call(*func, args, node.ty, row, env)?,
    })
}

/// A function call. The functions that take NULL are evaluated here; the
/// others are NULL if any argument is.
fn call<R: Row + ?Sized>(
    func: Func,
    args: &[Node],
    ty: Type,
    row: &mut R,
    env: &Env,
) -> Result<Val, Failure<R::Error>> {
    match func {
        Func::IsNull => return Ok(Val::Logical(eval(&args[0], row, env)? == Val::Null)),
        Func::Empty => {
            // NULL is empty in an ADT table, not in a Visual FoxPro one
            // (`fixtures/write/aofs_rest`).
            let empty = match eval(&args[0], row, env)? {
                Val::Null => !env.blank_empty,
                _ if !env.blank_empty => false,
                Val::Str(text) => text.iter().all(|&b| b == b' '),
                Val::Number(n) => n == 0.0,
                Val::Logical(b) => !b,
                Val::Date(day) => day == 0,
                Val::Binary(bytes) => bytes.is_empty(),
                Val::Time(_) | Val::TimeStamp(..) => false,
            };
            return Ok(Val::Logical(empty));
        }
        Func::Iif => {
            // All three arguments are evaluated, as the original does: a
            // division by 0 in the branch not taken still fails.
            let condition = eval(&args[0], row, env)?;
            let then = eval(&args[1], row, env)?;
            let otherwise = eval(&args[2], row, env)?;
            return Ok(match condition {
                Val::Logical(true) => then,
                Val::Logical(false) => otherwise,
                _ => Val::Null,
            });
        }
        Func::Coalesce => {
            // Every argument is evaluated, as in `IIF`.
            let mut first = Val::Null;
            for arg in args {
                let value = eval(arg, row, env)?;
                if first == Val::Null {
                    first = value;
                }
            }
            return Ok(first);
        }
        // A NULL among the candidates is skipped, not the end of the list
        // (`fixtures/expr/expressions.json`).
        Func::InList => {
            let first = eval(&args[0], row, env)?;
            if first == Val::Null {
                return Ok(Val::Null);
            }
            for arg in &args[1..] {
                let value = eval(arg, row, env)?;
                if value != Val::Null
                    && equal(&first, &value, args[0].ty, arg.ty, env) == Some(true)
                {
                    return Ok(Val::Logical(true));
                }
            }
            return Ok(Val::Logical(false));
        }
        // Answered by the full-text search the environment has, if any
        // (see [`crate::Contains`]); NULL for a NULL argument.
        Func::Contains if env.contains.is_some() => {
            let field = match &args[0].kind {
                crate::check::Kind::Field(field) => Some(field.index),
                _ => None,
            };
            let text = eval(&args[0], row, env)?;
            let condition = eval(&args[1], row, env)?;
            let found = match (text, condition, &env.contains) {
                (Val::Str(text) | Val::Binary(text), Val::Str(condition), Some(contains)) => {
                    (contains.0)(field, &text, &condition)
                }
                (Val::Null, ..) | (_, Val::Null, _) => return Ok(Val::Null),
                _ => Ok(true),
            };
            return match found {
                Ok(found) => Ok(Val::Logical(found)),
                Err(code) => Err(Failure::Runtime(Runtime::FullText(code))),
            };
        }
        Func::RecNo => return Ok(Val::Number(f64::from(row.record_number()))),
        Func::Deleted => return Ok(Val::Logical(row.deleted())),
        _ => {}
    }
    let mut values = Vec::with_capacity(args.len());
    for arg in args {
        match eval(arg, row, env)? {
            Val::Null => return Ok(Val::Null),
            value => values.push(value),
        }
    }
    let types: Vec<Type> = args.iter().map(|arg| arg.ty).collect();
    Ok(func::apply(func, &values, &types, ty, env)?)
}

/// `l op r`; `weighed` says which operands are a static collation's
/// weights already (see [`compare_text`]).
pub(crate) fn binary(
    op: Op,
    l: Val,
    r: Val,
    lt: Type,
    rt: Type,
    weighed: (bool, bool),
    env: &Env,
) -> Result<Val, Runtime> {
    if l == Val::Null || r == Val::Null {
        return Ok(Val::Null);
    }
    if crate::check::is_comparison(op) {
        let ci = matches!(lt, Type::String { ci: true, .. })
            || matches!(rt, Type::String { ci: true, .. });
        let compared = match (&l, &r) {
            (Val::Str(a), Val::Str(b)) => Some(compare_text(op, a, b, ci, weighed, env)),
            _ => compare(op, &l, &r, env, ci),
        };
        return Ok(compared.map_or(Val::Null, Val::Logical));
    }
    Ok(match (op, l, r) {
        (Op::Add, Val::Number(a), Val::Number(b)) => Val::Number(a + b),
        (Op::Sub, Val::Number(a), Val::Number(b)) => Val::Number(a - b),
        (Op::Mul, Val::Number(a), Val::Number(b)) => Val::Number(a * b),
        (Op::Div | Op::Mod, Val::Number(_), Val::Number(0.0)) => {
            return Err(Runtime::DivideByZero);
        }
        (Op::Div, Val::Number(a), Val::Number(b)) => Val::Number(a / b),
        (Op::Mod, Val::Number(a), Val::Number(b)) => Val::Number(a % b),
        (Op::Pow, Val::Number(a), Val::Number(b)) => Val::Number(a.powf(b)),
        (Op::Add, Val::Str(mut a), Val::Str(b)) => {
            a.extend(b);
            Val::Str(a)
        }
        // The left string's trailing spaces move to the end.
        (Op::Sub, Val::Str(a), Val::Str(b)) => {
            let kept = a.iter().rposition(|&c| c != b' ').map_or(0, |i| i + 1);
            let mut joined = a[..kept].to_vec();
            joined.extend(b);
            joined.resize(joined.len() + a.len() - kept, b' ');
            Val::Str(joined)
        }
        (Op::Add, Val::Date(d), Val::Number(n)) | (Op::Add, Val::Number(n), Val::Date(d)) => {
            Val::Date((f64::from(d) + n).floor() as i32)
        }
        (Op::Add, Val::Date(a), Val::Date(b)) => Val::Date(a.wrapping_add(b)),
        (Op::Sub, Val::Date(d), Val::Number(n)) => Val::Date((f64::from(d) - n).floor() as i32),
        (Op::Sub, Val::Date(a), Val::Date(b)) => Val::Number(f64::from(a) - f64::from(b)),
        _ => Val::Null,
    })
}

/// Whether `a` equals `b` as `=` compares them (`INLIST`); `None` for
/// types that do not compare.
pub(crate) fn equal(a: &Val, b: &Val, ta: Type, tb: Type, env: &Env) -> Option<bool> {
    let ci =
        matches!(ta, Type::String { ci: true, .. }) || matches!(tb, Type::String { ci: true, .. });
    compare(Op::Eq, a, b, env, ci)
}

/// `l op r` for a comparison; `None` for types that do not compare.
fn compare(op: Op, l: &Val, r: &Val, env: &Env, ci: bool) -> Option<bool> {
    let ordering = match (l, r) {
        (Val::Str(a), Val::Str(b)) => return Some(compare_text(op, a, b, ci, (false, false), env)),
        (Val::Number(a), Val::Number(b)) => a.partial_cmp(b)?,
        (Val::Date(a), Val::Date(b)) => a.cmp(b),
        (Val::Time(a), Val::Time(b)) => a.cmp(b),
        (Val::TimeStamp(a, x), Val::TimeStamp(b, y)) => (a, x).cmp(&(b, y)),
        (Val::Binary(a), Val::Binary(b)) => a.cmp(b),
        (Val::Logical(a), Val::Logical(b)) => a.cmp(b),
        (Val::Logical(a), Val::Number(n)) => a.cmp(&(*n != 0.0)),
        (Val::Number(n), Val::Logical(b)) => (*n != 0.0).cmp(b),
        _ => return None,
    };
    Some(holds(op, ordering))
}

fn holds(op: Op, ordering: Ordering) -> bool {
    match op {
        Op::Eq | Op::Exactly => ordering.is_eq(),
        Op::Ne => ordering.is_ne(),
        Op::Lt => ordering.is_lt(),
        Op::Gt => ordering.is_gt(),
        Op::Le => ordering.is_le(),
        Op::Ge => ordering.is_ge(),
        _ => false,
    }
}

/// Compares text as the original engine does. `==` compares the bytes
/// exactly, and `$` looks for `a` in `b` (both case-folded for a CICharacter
/// operand, but never collation-weighed). The ordering comparisons (`=`, `<>`,
/// `<`, `>`, `<=`, `>=`) weigh each byte by the static collation
/// ([`Env::weights`], a CICharacter operand folding case through the collation
/// first — [`CaseFold::lower`]) and follow `exact` (`AdsSetExact`): with it the
/// shorter text is taken as padded with spaces (weighed too); without it `a` is
/// cut to the length of `b`, so `'abc' = 'ab'` but not `'ab' = 'abc'`. Under the
/// default collation the weights are the bytes themselves and case folds in
/// ASCII, so this is a plain byte comparison. An operand `weighed` says is
/// the static collation's weights already (`UPPER()` or `LOWER()` of text,
/// which give them) is compared as it is, as the original engine compares
/// it (`UPPER(name) = 'ÄPFEL'` under German, `fixtures/write/aofs_static`).
fn compare_text(op: Op, a: &[u8], b: &[u8], ci: bool, weighed: (bool, bool), env: &Env) -> bool {
    if matches!(op, Op::Exactly | Op::In) {
        // CICharacter text folds by the static collation's case, as `=`
        // folds it (`ci == 'Äpfel'` holds for `äpfel` under German).
        let fold = |s: &[u8]| -> Vec<u8> {
            match (ci, &env.case_fold) {
                (true, Some(fold)) => s.iter().map(|&b| fold.lower[usize::from(b)]).collect(),
                (true, None) => s.to_ascii_lowercase(),
                (false, _) => s.to_vec(),
            }
        };
        let (a, b) = (fold(a), fold(b));
        return match op {
            Op::Exactly => a == b,
            _ => a.is_empty() || b.windows(a.len()).any(|w| w == a.as_slice()),
        };
    }
    // Under a dynamic collation, by its weights (trailing blanks aside),
    // padded with zeros as its keys are; without `exact`, `a`'s weights are
    // cut to the length of `b`'s, so that text compares as the keys of the
    // orders find it.
    if let Some(dynamic) = &env.dynamic {
        let fold = |s: &[u8]| -> Vec<u8> {
            if ci {
                s.to_ascii_lowercase()
            } else {
                s.to_vec()
            }
        };
        let (mut a, mut b) = (dynamic.weights(&fold(a)), dynamic.weights(&fold(b)));
        if env.exact {
            let width = a.len().max(b.len());
            a.resize(width, 0);
            b.resize(width, 0);
        } else {
            a.resize(b.len(), 0);
        }
        return holds(op, a.cmp(&b));
    }
    let weigh = |s: &[u8], weighed: bool| -> Vec<u8> {
        if weighed {
            return s.to_vec();
        }
        s.iter()
            .map(|&byte| {
                let i = usize::from(byte);
                match (ci, &env.case_fold, &env.weights) {
                    (true, Some(fold), _) => fold.lower[i],
                    (true, None, _) => byte.to_ascii_lowercase(),
                    (false, _, Some(weights)) => weights[i],
                    (false, _, None) => byte,
                }
            })
            .collect()
    };
    let (a, b) = (weigh(a, weighed.0), weigh(b, weighed.1));
    if env.exact {
        let width = a.len().max(b.len());
        let pad = env
            .weights
            .map_or(b' ', |weights| weights[usize::from(b' ')]);
        let (mut a, mut b) = (a, b);
        a.resize(width, pad);
        b.resize(width, pad);
        holds(op, a.cmp(&b))
    } else {
        holds(op, a[..a.len().min(b.len())].cmp(&b))
    }
}
