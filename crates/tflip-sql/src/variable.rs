//! What assigning a value to a script's variable does, as the original
//! engine does it (`fixtures/sql/declare.json`): which kinds of value each
//! type takes, and what becomes of one that does not fit, which is not what a
//! table's column does with it.

use tflip_storage::cp1252;
use tflip_storage::field::{Field, FieldType};
use tflip_storage::value::Value;

use crate::ast::ExprKind;
use crate::error::{Pos, SqlError};
use crate::eval::Env;
use crate::types::{IntWidth, Ty};
use crate::value::{V, pow10};

/// A type's name in the messages of a failed assignment to a variable of it.
fn target_name(ty: Ty) -> &'static str {
    match ty {
        // The logical and the integers are named alike.
        Ty::Bool | Ty::Int { .. } => "Integer",
        Ty::Num { .. } | Ty::Money => "Numeric",
        Ty::Double | Ty::CurDouble => "Double",
        Ty::Date => "Date",
        Ty::Time => "Time",
        Ty::TimeStamp => "Timestamp",
        Ty::Str { .. } | Ty::Unknown => "Character",
        Ty::Bin { .. } | Ty::Guid => "Binary",
    }
}

/// A value's type's name, as it is in them.
fn source_name(ty: Ty) -> &'static str {
    match ty {
        Ty::Bool | Ty::Int { .. } => "Integer",
        Ty::Num { .. } | Ty::Money => "Numeric",
        Ty::Double | Ty::CurDouble => "Double",
        other => target_name(other),
    }
}

/// Whether a value of type `source` can be assigned to a variable of type
/// `target`, or why not, `name` the variable and `value` the expression of
/// the value at `pos`, `runtime` whether it reads variables, as a constant is
/// checked before the script runs, and its text read as a date, time or GUID
/// by [`convert`], where the text of a variable is no date.
pub(crate) fn check(
    target: Ty,
    source: Ty,
    value: &ExprKind,
    runtime: bool,
    name: &str,
    pos: Pos,
) -> Result<(), SqlError> {
    check_kinds(target, source, value, runtime, name, pos).map_err(|error| match runtime {
        // A failing script statement is worded with the script's location
        // after it.
        true => {
            let mut error = error;
            error.message = error.message.replace(&crate::error::at(pos), "");
            script(error, pos)
        }
        false => error,
    })
}

fn check_kinds(
    target: Ty,
    source: Ty,
    value: &ExprKind,
    runtime: bool,
    name: &str,
    pos: Pos,
) -> Result<(), SqlError> {
    // A date written in ISO form is a date to the numbers and the logical.
    let source_name = match value {
        ExprKind::Str(text) if (target.is_numeric() || target == Ty::Bool) && iso_date(text) => {
            "Date"
        }
        _ => source_name(source),
    };
    let refused = || SqlError::invalid_assignment(target_name(target), source_name, name, pos);
    let boolean = || SqlError::bool_assignment(target_name(target), source_name, name, pos);
    match (target, source) {
        (_, Ty::Unknown) => Ok(()),
        (Ty::Bool, Ty::Bool) => Ok(()),
        // Of the numbers, the literals 0 and 1 are logicals too.
        (Ty::Bool, _) if matches!(value, ExprKind::Number(n) if n == "0" || n == "1") => Ok(()),
        // A logical is no number, and a number no logical.
        (Ty::Bool, s) if s.is_numeric() => Err(boolean()),
        (t, Ty::Bool) if t.is_numeric() => Err(boolean()),
        (t, s) if t.is_numeric() => s.is_numeric().then_some(()).ok_or_else(refused),
        (Ty::Str { .. }, Ty::Str { .. }) => Ok(()),
        (Ty::Bin { .. }, Ty::Bin { .. }) => Ok(()),
        (Ty::Guid, Ty::Guid | Ty::Bin { .. }) => Ok(()),
        (Ty::Date, Ty::Date) | (Ty::Time, Ty::Time) | (Ty::TimeStamp, Ty::TimeStamp) => Ok(()),
        (Ty::Guid | Ty::Date | Ty::Time | Ty::TimeStamp, Ty::Str { .. }) if !runtime => Ok(()),
        _ => Err(refused()),
    }
}

/// Whether `text` is a date as `YYYY-MM-DD`.
fn iso_date(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    parts.len() == 3
        && parts
            .iter()
            .zip([4, 2, 2])
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_digit()))
}

/// `value`, of a type [`check`] let through, as the value of the variable
/// `field`: text read as a date, time or GUID, text cut to its length, and
/// the errors of a number that does not fit.
pub(crate) fn convert(
    value: V,
    field: &Field,
    name: &str,
    pos: Pos,
    env: &Env,
) -> Result<V, SqlError> {
    let ty = Ty::of_field(field);
    let invalid = |what: &str, source: &str| {
        SqlError::invalid_assignment_value(what, pos, target_name(ty), source, name)
    };
    Ok(match (ty, value) {
        (_, V::Null) => V::Null,
        (Ty::Bool, V::Int(n)) => V::Bool(n != 0),
        (Ty::Int { width, .. }, v) => {
            if !v.is_whole() {
                return Err(script(SqlError::too_large_bare(), pos));
            }
            let n = v.as_i128().unwrap_or(0);
            let fits = match width {
                IntWidth::Short => i16::try_from(n).is_ok(),
                IntWidth::Int => i32::try_from(n).is_ok(),
                IntWidth::Long => i64::try_from(n).is_ok(),
            };
            if !fits {
                return Err(script(SqlError::variable_out_of_range(name), pos));
            }
            V::Int(n as i64)
        }
        (Ty::Num { scale, .. }, V::Num(n, from)) if from > scale => {
            if n % pow10(from - scale) != 0 {
                return Err(script(SqlError::variable_truncated(name), pos));
            }
            V::Num(n / pow10(from - scale), scale)
        }
        (Ty::Money, V::Num(n, from)) if from > 4 && n % pow10(from - 4) != 0 => {
            return Err(script(SqlError::variable_truncated(name), pos));
        }
        (Ty::Str { field, length }, V::Str(s)) => match field {
            FieldType::Memo | FieldType::NMemo => V::Str(s),
            // A fixed length of text is padded with spaces.
            FieldType::Character | FieldType::CiCharacter | FieldType::NChar => {
                let mut text: String = s.chars().take(usize::from(length)).collect();
                let missing = usize::from(length).saturating_sub(text.chars().count());
                text.extend(std::iter::repeat_n(' ', missing));
                V::Str(text)
            }
            _ => V::Str(s.chars().take(usize::from(length)).collect()),
        },
        // A date is digits with its separators, which a time or a number
        // with a colon is not.
        (Ty::Date, V::Str(s)) => V::Date(
            Some(&s)
                .filter(|s| {
                    s.bytes()
                        .all(|b| b.is_ascii_digit() || b"-/. ".contains(&b))
                })
                .and_then(|s| crate::eval::text_date(s, env))
                .ok_or_else(|| invalid("DATE", "Character"))?,
        ),
        (Ty::Time, V::Str(s)) => {
            V::Time(crate::compile::time_text(&s).ok_or_else(|| invalid("TIME", "Character"))?)
        }
        (Ty::TimeStamp, V::Str(s)) => {
            let (d, t) = crate::compile::timestamp_text(&s, &env.date_format, env.epoch())
                .ok_or_else(|| invalid("TIMESTAMP", "Character"))?;
            V::TimeStamp(d, t)
        }
        (Ty::Guid, V::Str(s)) => match guid_text(&s) {
            true => V::Str(s),
            false => {
                return Err(SqlError::invalid_assignment(
                    "Binary",
                    "Character",
                    name,
                    pos,
                ));
            }
        },
        (Ty::Guid, V::Bin(b)) => {
            let mut bytes = [0u8; 16];
            for (to, from) in bytes.iter_mut().zip(&b) {
                *to = *from;
            }
            V::Str(Value::guid_text(&bytes))
        }
        (Ty::Bin { length, field }, V::Bin(mut b)) => {
            if !matches!(field, FieldType::Binary | FieldType::Image) {
                b.truncate(usize::from(length));
            }
            V::Bin(b)
        }
        (Ty::Str { .. }, V::Bin(b)) => V::Str(cp1252::decode(&b)),
        (_, v) => v,
    })
}

/// `value`, if a text, as the date, time or timestamp of the type `ty`, if it
/// reads as one: what a procedure's argument is, or a value written to its
/// `__output`.
pub(crate) fn from_text(value: V, ty: Ty, env: &Env) -> V {
    match (&value, ty) {
        (V::Str(s), Ty::Date) => crate::eval::text_date(s, env).map_or(value, V::Date),
        (V::Str(s), Ty::Time) => crate::compile::time_text(s).map_or(value, V::Time),
        (V::Str(s), Ty::TimeStamp) => {
            crate::compile::timestamp_text(s, &env.date_format, env.epoch())
                .map_or(value, |(day, ms)| V::TimeStamp(day, ms))
        }
        _ => value,
    }
}

/// `value`, an argument at `pos` of a procedure whose input parameter is
/// `field`: a text read as a date, time or timestamp, and refused if it is no
/// such or longer than the parameter.
pub(crate) fn input(value: V, field: &Field, pos: Pos, env: &Env) -> Result<V, SqlError> {
    let ty = Ty::of_field(field);
    match (&value, ty) {
        (V::Str(_), Ty::Date | Ty::Time | Ty::TimeStamp) => {
            let read = from_text(value, ty, env);
            if let V::Str(_) = read {
                let what = match ty {
                    Ty::Date => "DATE",
                    Ty::Time => "TIME",
                    _ => "TIMESTAMP",
                };
                // Its location is after the parameter's name.
                let mut error = SqlError::invalid_assignment_value(
                    what,
                    pos,
                    target_name(ty),
                    "Character",
                    &field.name,
                );
                error.message = error.message.replace(&crate::error::at(pos), "");
                error.message.push_str(&crate::error::at(pos));
                return Err(error);
            }
            Ok(read)
        }
        (
            V::Str(s),
            Ty::Str {
                field: kind,
                length,
            },
        ) if !matches!(kind, FieldType::Memo | FieldType::NMemo)
            && s.chars().count() > usize::from(length) =>
        {
            Err(SqlError::input_too_large())
        }
        _ => Ok(value),
    }
}

/// The error `error` of an assignment, located in the script at `pos`.
fn script(mut error: SqlError, pos: Pos) -> SqlError {
    error.message.push_str(&crate::error::script_at(pos));
    error
}

/// Whether `text` is a GUID: `8-4-4-4-12` hexadecimal digits.
fn guid_text(text: &str) -> bool {
    crate::eval::guid_bytes(text).is_some()
}
