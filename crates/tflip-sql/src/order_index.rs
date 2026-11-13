//! The temporary index a cursor gets for an `ORDER BY` no order serves.
//!
//! The original engine adds it to the cursor, an index of the expression
//! engine named `TIM_<n>` that `AdsGetNumIndexes` counts
//! (`fixtures/sql/orderby.json`). Its key is the `ORDER BY`'s keys as that
//! engine writes them: `name;Descend(id)`, `UPPER(name)`, `(0-id)`, with the
//! table's names for fields, constant keys left out, and every key
//! `Descend()`ed but when all are descending, which turns the index round
//! instead. An `ORDER BY` the engine cannot write that way (`CAST`, `IIF`,
//! `TRIM`, `ROUND`, `MOD`, `CASE`, text of two columns added) is sorted
//! without an index, in a static cursor.
//!
//! A static cursor has one too when its rows were sorted on its columns
//! (`TIM_<n>` with the columns' names and `DESCEND`), unless there are
//! fewer than two rows, a `TOP`, or its rows came out of `GROUP BY` in the
//! order of the group keys (`fixtures/sql/orderby.json`, `select.json`).

use tflip_storage::field::{Field, FieldType};

use crate::ast::BinOp;
use crate::compile::{Kind, Node};
use crate::func::Func;
use crate::plan::SortKey;
use crate::types::Ty;
use crate::value::V;

/// What kind of key an index has (`AdsGetKeyType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Logical,
    Numeric,
    Date,
    String,
    Raw,
}

/// A temporary index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TempIndex {
    /// The key expression.
    pub expression: String,
    /// Whether the index is walked backwards: every key is `DESC`.
    pub descending: bool,
    /// The index's options as the original engine reports them
    /// (`AdsGetIndexFlags`).
    pub flags: u32,
    pub key_length: u16,
    pub key_kind: KeyKind,
}

/// A key of a temporary index.
pub struct Key {
    /// The key as the index expression has it.
    pub text: String,
    pub descending: bool,
    /// Whether the key is a field, not an expression.
    pub plain: bool,
    pub length: u16,
    pub kind: KeyKind,
}

/// The key length and kind of a field of `field_type` and `length`.
pub fn field_key(field_type: FieldType, length: u16) -> (u16, KeyKind) {
    match field_type {
        FieldType::Logical => (1, KeyKind::Logical),
        FieldType::Date => (8, KeyKind::Date),
        FieldType::Numeric
        | FieldType::ShortInt
        | FieldType::Integer
        | FieldType::LongInt
        | FieldType::AutoInc
        | FieldType::RowVersion
        | FieldType::Double
        | FieldType::CurDouble
        | FieldType::Money => (8, KeyKind::Numeric),
        FieldType::Raw | FieldType::Guid | FieldType::Time | FieldType::TimeStamp => {
            (8, KeyKind::Raw)
        }
        FieldType::Binary | FieldType::Memo => (length, KeyKind::String),
        _ => (length, KeyKind::String),
    }
}

/// The index `keys` make, written with `descend` for a key that is not
/// walked the way the others are.
pub fn build(keys: &[Key], descend: &str) -> TempIndex {
    // Every key descending turns the index round, as it has no `Descend`.
    let descending = keys.iter().all(|key| key.descending);
    let mixed = !descending && keys.iter().any(|key| key.descending);
    let plain = keys.iter().all(|key| key.plain);
    let expression = keys
        .iter()
        .map(|key| match key.descending && !descending {
            true => format!("{descend}({})", key.text),
            false => key.text.clone(),
        })
        .collect::<Vec<_>>()
        .join(";");
    // Compound, then `ADS_DESCENDING`, then the kind of key: 0xC000 for
    // an expression, 0x8000 for a `Descend()`ed field.
    let mut flags = 0x2;
    if descending {
        flags |= 0x8;
    }
    flags |= match (plain, mixed) {
        (false, _) => 0xC000,
        (true, true) => 0x8000,
        (true, false) => 0,
    };
    TempIndex {
        expression,
        descending,
        flags,
        key_length: keys.iter().map(|key| key.length).sum(),
        key_kind: match keys {
            [key] => key.kind,
            _ => KeyKind::Raw,
        },
    }
}

/// What an `ORDER BY` of a live cursor makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Order {
    /// An index.
    Index(TempIndex),
    /// Nothing, for there is no key to sort by (only constants).
    Nothing,
    /// It cannot be an index.
    Unindexable,
}

/// The index an `ORDER BY` of a live cursor makes. The fields are those of
/// the table the columns of `order` number.
pub fn temp_index(order: &[SortKey], fields: &[Field]) -> Order {
    let mut keys = Vec::new();
    for key in order {
        if matches!(key.node.kind, Kind::Const(_)) {
            continue;
        }
        let text = match &key.alias {
            Some(alias) => alias.clone(),
            None => match text(&key.node, fields, 0) {
                Some(text) => text,
                None => return Order::Unindexable,
            },
        };
        keys.push(Key {
            text,
            descending: key.descending,
            plain: column(&key.node).is_some(),
            length: length(&key.node, fields),
            kind: kind(&key.node, fields),
        });
    }
    // The original engine sorts 21 keys or more in a static cursor
    // (`fixtures/sql/limits.json`).
    if keys.len() > 20 {
        return Order::Unindexable;
    }
    match keys.is_empty() {
        true => Order::Nothing,
        false => Order::Index(build(&keys, "Descend")),
    }
}

fn column(node: &Node) -> Option<usize> {
    match node.kind {
        Kind::Column(i) => Some(i),
        _ => None,
    }
}

/// How long a key of `node` is in the index: a text field's length, 8 for
/// numbers and dates, 1 for a logical.
fn length(node: &Node, fields: &[Field]) -> u16 {
    if let Some(field) = column(node).and_then(|i| fields.get(i)) {
        return field_key(field.field_type, field.length()).0;
    }
    match node.ty {
        Ty::Str { length, .. } => length,
        Ty::Bool => 1,
        _ => 8,
    }
}

fn kind(node: &Node, fields: &[Field]) -> KeyKind {
    if let Some(field) = column(node).and_then(|i| fields.get(i)) {
        return field_key(field.field_type, field.length()).1;
    }
    match node.ty {
        Ty::Bool => KeyKind::Logical,
        Ty::Date => KeyKind::Date,
        Ty::Str { .. } => KeyKind::String,
        Ty::Time | Ty::TimeStamp | Ty::Bin { .. } | Ty::Guid => KeyKind::Raw,
        _ => KeyKind::Numeric,
    }
}

/// How tightly an operator binds.
fn precedence(op: BinOp) -> u8 {
    match op {
        BinOp::Add | BinOp::Sub => 1,
        _ => 2,
    }
}

/// `node` as the expression engine writes it, inside an operator that
/// binds as tight as `outer` (0 for none) or tighter; `None` for what it
/// cannot write.
fn text(node: &Node, fields: &[Field], outer: u8) -> Option<String> {
    Some(match &node.kind {
        Kind::Column(i) => fields.get(*i)?.name.clone(),
        Kind::Const(V::Null) => return None,
        Kind::Const(v) => crate::xbase::constant(v)?,
        Kind::Neg(inner) => format!("(0-{})", text(inner, fields, 2)?),
        Kind::Binary(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div), a, b) => {
            let symbol = match op {
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                _ => "/",
            };
            // Text of two columns cannot be added, a constant can be.
            let is_text = |n: &Node| matches!(n.ty, Ty::Str { .. });
            let constant = |n: &Node| matches!(n.kind, Kind::Const(_));
            if *op == BinOp::Add && is_text(a) && is_text(b) && !constant(a) && !constant(b) {
                return None;
            }
            let own = precedence(*op);
            let right = match op {
                BinOp::Sub | BinOp::Div => own + 1,
                _ => own,
            };
            let body = format!(
                "{}{symbol}{}",
                text(a, fields, own)?,
                text(b, fields, right)?
            );
            match own < outer {
                true => format!("({body})"),
                false => body,
            }
        }
        Kind::Func(func, args) => {
            let name = match func {
                Func::Upper => "UPPER",
                Func::Lower => "LOWER",
                Func::Left => "LEFT",
                Func::Right => "RIGHT",
                Func::Substring => "SUBSTR",
                Func::Ltrim => "LTRIM",
                Func::Rtrim => "RTRIM",
                Func::Length => "LENGTH",
                Func::Len | Func::CharLength => "LEN",
                Func::Abs => "ABS",
                Func::Year => "YEAR",
                Func::Coalesce => "COALESCE",
                Func::CurDate if args.is_empty() => return Some("DATE()".to_owned()),
                Func::IfNull => {
                    let [value, other] = args.as_slice() else {
                        return None;
                    };
                    let (value, other) = (text(value, fields, 0)?, text(other, fields, 0)?);
                    return Some(format!("IIF(ISNULL({value}),{other},{value})"));
                }
                _ => return None,
            };
            let args: Option<Vec<String>> = args.iter().map(|a| text(a, fields, 0)).collect();
            format!("{name}({})", args?.join(","))
        }
        _ => return None,
    })
}
