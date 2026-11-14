//! Resolving names and checking types: a syntax tree to a [`Node`] tree,
//! each node with the type of its result.

use tflip_storage::field::FieldType;

use crate::eval::Val;
use crate::func::{self, Func};
use crate::lex::Op;
use crate::parse::Ast;
use crate::{ErrorKind, Field, Format, Schema, Type};

/// A checked expression.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    pub ty: Type,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    Const(Val),
    Field(Field),
    Binary(Op, Box<Node>, Box<Node>),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Not(Box<Node>),
    Join(Box<Node>, Box<Node>),
    Call(Func, Vec<Node>),
}

/// Checks `tree` against the table `schema` describes.
pub(crate) fn check(tree: &Ast, schema: &dyn Schema) -> Result<Node, ErrorKind> {
    let node = |ty, kind| Ok(Node { ty, kind });
    match tree {
        &Ast::Number {
            value,
            digits,
            decimals,
        } => node(
            Type::Number(literal_format(digits, decimals)),
            Kind::Const(Val::Number(value)),
        ),
        Ast::Str(bytes) => {
            let width = bytes.len() as u16;
            node(
                Type::String { width, ci: false },
                Kind::Const(Val::Str(bytes.clone())),
            )
        }
        &Ast::Logical(value) => node(Type::Logical, Kind::Const(Val::Logical(value))),
        Ast::Null => node(Type::Null, Kind::Const(Val::Null)),
        Ast::Name { alias, name } => {
            if let Some(alias) = alias
                && !crate::names_table(alias, schema)
            {
                return Err(ErrorKind::UnknownAlias);
            }
            let field = schema.field(name).ok_or(ErrorKind::UnknownIdentifier)?;
            node(field_type(&field), Kind::Field(field))
        }
        Ast::Call { name, args } => {
            let func = Func::named(name).ok_or(ErrorKind::UnsupportedFunction)?;
            let (min, max) = func.arity();
            if args.len() < min || args.len() > max {
                return Err(ErrorKind::ParameterCount);
            }
            let args = args
                .iter()
                .map(|arg| check(arg, schema))
                .collect::<Result<Vec<_>, _>>()?;
            let ty = func::check(func, &args)?;
            node(ty, Kind::Call(func, args))
        }
        Ast::Binary { op, left, right } => {
            let left = check(left, schema)?;
            let right = check(right, schema)?;
            let ty = binary_type(*op, left.ty, right.ty)?;
            node(ty, Kind::Binary(*op, Box::new(left), Box::new(right)))
        }
        Ast::And(left, right) | Ast::Or(left, right) => {
            let left = logical(check(left, schema)?)?;
            let right = logical(check(right, schema)?)?;
            let (left, right) = (Box::new(left), Box::new(right));
            let kind = if matches!(tree, Ast::And(..)) {
                Kind::And(left, right)
            } else {
                Kind::Or(left, right)
            };
            node(Type::Logical, kind)
        }
        Ast::Not(operand) => node(
            Type::Logical,
            Kind::Not(Box::new(logical(check(operand, schema)?)?)),
        ),
        Ast::Join(left, right) => {
            let left = check(left, schema)?;
            let right = check(right, schema)?;
            for part in [&left, &right] {
                if matches!(part.ty, Type::Memo | Type::Null) {
                    return Err(ErrorKind::TypeMismatch);
                }
            }
            let width = left.ty.width() + right.ty.width();
            node(
                Type::Binary { width },
                Kind::Join(Box::new(left), Box::new(right)),
            )
        }
    }
}

fn logical(node: Node) -> Result<Node, ErrorKind> {
    match node.ty {
        Type::Logical | Type::Null => Ok(node),
        _ => Err(ErrorKind::TypeMismatch),
    }
}

/// The display format of a number written in an expression: at least ten
/// characters, or the integer digits and a sign; the decimals written, after
/// a point.
fn literal_format(digits: u16, decimals: u16) -> Format {
    let whole = (digits + 1).max(10);
    if decimals == 0 {
        Format::new(whole, 0)
    } else {
        Format::new(whole + 1 + decimals, decimals)
    }
}

/// The display format of a computed number with `decimals` decimals.
pub(crate) fn computed_format(decimals: u16) -> Format {
    literal_format(0, decimals.min(9))
}

/// The type of a field's values in expressions.
pub(crate) fn field_type(field: &Field) -> Type {
    let number = |length, decimals| {
        Type::Number(Format {
            length,
            decimals,
            fixed: true,
        })
    };
    let length = field.length;
    match field.field_type {
        FieldType::Logical => Type::Logical,
        FieldType::Character | FieldType::VarChar | FieldType::LegacyVarChar => Type::String {
            width: length,
            ci: false,
        },
        FieldType::CiCharacter => Type::String {
            width: length,
            ci: true,
        },
        FieldType::NChar | FieldType::NVarChar => Type::String {
            width: length,
            ci: false,
        },
        FieldType::Numeric => number(length, field.decimals),
        FieldType::ShortInt => number(6, 0),
        FieldType::Integer | FieldType::AutoInc => number(11, 0),
        FieldType::LongInt | FieldType::RowVersion => number(20, 0),
        FieldType::Double => number(17 + field.decimals, field.decimals),
        FieldType::CurDouble => number(21 + field.decimals, field.decimals),
        FieldType::Money => number(21, 4),
        FieldType::Date | FieldType::CompactDate => Type::Date,
        FieldType::Time => Type::Time,
        FieldType::TimeStamp | FieldType::ModTime => Type::TimeStamp,
        FieldType::Raw => Type::Binary { width: length },
        FieldType::VarBinary => Type::Binary { width: length + 1 },
        FieldType::Guid => Type::Binary { width: 16 },
        FieldType::Memo
        | FieldType::NMemo
        | FieldType::Binary
        | FieldType::Image
        | FieldType::Other(_) => Type::Memo,
    }
}

/// The type of `left op right`.
fn binary_type(op: Op, left: Type, right: Type) -> Result<Type, ErrorKind> {
    use Type::*;
    let mismatch = Err(ErrorKind::TypeMismatch);
    if left == Memo || right == Memo {
        return mismatch;
    }
    if left == Null || right == Null {
        return Ok(if is_comparison(op) { Logical } else { Null });
    }
    match op {
        _ if is_comparison(op) => match (left, right) {
            (String { .. }, String { .. }) => Ok(Logical),
            _ if op == Op::In => mismatch,
            (Number(_), Number(_)) | (Date, Date) | (Time, Time) | (TimeStamp, TimeStamp) => {
                Ok(Logical)
            }
            (Binary { .. }, Binary { .. }) => Ok(Logical),
            (Logical, Logical) | (Logical, Number(_)) | (Number(_), Logical)
                if matches!(op, Op::Eq | Op::Exactly | Op::Ne) =>
            {
                Ok(Logical)
            }
            _ => mismatch,
        },
        Op::Add => match (left, right) {
            (Number(l), Number(r)) => Ok(Number(computed_format(l.decimals.max(r.decimals)))),
            (String { width: l, .. }, String { width: r, .. }) => Ok(String {
                width: l.saturating_add(r),
                ci: false,
            }),
            (Date, Number(_)) | (Number(_), Date) | (Date, Date) => Ok(Date),
            _ => mismatch,
        },
        Op::Sub => match (left, right) {
            (Number(l), Number(r)) => Ok(Number(computed_format(l.decimals.max(r.decimals)))),
            (String { width: l, .. }, String { width: r, .. }) => Ok(String {
                width: l.saturating_add(r),
                ci: false,
            }),
            (Date, Number(_)) => Ok(Date),
            (Date, Date) => Ok(Number(computed_format(0))),
            _ => mismatch,
        },
        Op::Mul => match (left, right) {
            (Number(l), Number(r)) => Ok(Number(computed_format(l.decimals + r.decimals))),
            _ => mismatch,
        },
        Op::Div | Op::Mod | Op::Pow => match (left, right) {
            (Number(l), Number(r)) => {
                let decimals = if op == Op::Div {
                    2
                } else {
                    l.decimals.max(r.decimals)
                };
                Ok(Number(computed_format(decimals)))
            }
            _ => mismatch,
        },
        _ => unreachable!("comparisons are handled above"),
    }
}

pub(crate) fn is_comparison(op: Op) -> bool {
    matches!(
        op,
        Op::Eq | Op::Exactly | Op::Ne | Op::Lt | Op::Gt | Op::Le | Op::Ge | Op::In
    )
}
