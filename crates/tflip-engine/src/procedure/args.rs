//! What the system procedures that take parameters share: a procedure's
//! parameters ([`Spec`]), how a call's arguments are checked against them
//! as the original engine checks them, and the errors those and the
//! procedures raise as it words them.

use tflip_sql::ast::{Execute, ExprKind};
use tflip_sql::types::Ty;
use tflip_sql::{SqlError, V};
use tflip_storage::build::Column;
use tflip_storage::field::{Field, FieldType};

use crate::Error;
use crate::ace_error;

/// What a parameter takes.
#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Text,
    /// Text a parameter of `n` characters holds: a longer one is cut, and the
    /// call complains of it once it has run (2208).
    Char(u16),
    Integer,
    /// A ShortInt, which a whole number outside 16 bits is refused for.
    Short,
    /// An Integer that also takes a logical value (`true`, `false`).
    Flag,
    /// An Integer that is 0 or 1 and nothing else: not a logical value, not
    /// another number.
    Bool,
    Memo,
    /// A Memo that the online maintenance procedures read their options
    /// from; a number given for it is taken for one argument too many
    /// (`fixtures/events/events_online_args.json`), not a type mismatch.
    Options,
}

/// A procedure: its name in upper case, its parameters, how many of them a
/// call must give, and whether it has a result set.
pub(crate) struct Spec {
    pub name: &'static str,
    pub params: &'static [(&'static str, Kind)],
    pub required: usize,
    pub results: bool,
}

/// A parameter's field, as the SQL engine types it.
fn field_of(name: &str, kind: Kind) -> Field {
    let (field_type, length) = match kind {
        // Not the parameter's own length: the check a call's arguments first
        // get would refuse a longer text before the procedure runs.
        Kind::Text | Kind::Char(_) => (FieldType::Character, 515),
        Kind::Integer | Kind::Bool => (FieldType::Integer, 4),
        // The original types it as an Integer, though it keeps 16 bits.
        Kind::Short => (FieldType::Integer, 4),
        // Typed so that a logical value passes the check a call's arguments
        // first get; [`check_arguments`] holds it to an Integer.
        Kind::Flag => (FieldType::Character, 515),
        Kind::Memo | Kind::Options => (FieldType::Memo, 9),
    };
    tflip_storage::adt::field(name, field_type, 0, length, 0)
}

/// The parameters of the procedure `spec`, as fields: what types a call's
/// parameters (`?`) get.
pub(crate) fn inputs(spec: &Spec) -> Vec<Field> {
    spec.params
        .iter()
        .map(|(name, kind)| field_of(name, *kind))
        .collect()
}

/// A call's arguments, checked against the procedure's parameters.
pub(crate) struct Arguments {
    pub values: Vec<V>,
    /// Whether a text was cut to its parameter's length.
    pub cut: bool,
}

impl Arguments {
    pub fn text(&self, i: usize) -> Option<String> {
        text_of(self.values.get(i))
    }

    /// An integer argument, cut to a whole number as the original engine
    /// does; a logical one is 0 or 1.
    pub fn integer(&self, i: usize) -> Option<i64> {
        whole(self.values.get(i))
    }
}

/// `value` as a text parameter reads it: NULL is none, another type its
/// display.
pub(crate) fn text_of(value: Option<&V>) -> Option<String> {
    match value {
        Some(V::Null) | None => None,
        Some(V::Str(s)) => Some(s.clone()),
        Some(other) => Some(other.display()),
    }
}

/// The name `sp_GetTableEncryptionType` and `sp_GetSecurityInfo` give an
/// encryption type that is on.
pub(crate) fn encryption_name(encryption_type: u32) -> &'static str {
    match encryption_type {
        5 => "AES128",
        6 => "AES256",
        _ => "RC4",
    }
}

/// `value` as an Integer parameter reads it: a number is cut to a whole one
/// (`6.9` is 6, `0.5` is 0).
pub(crate) fn whole(value: Option<&V>) -> Option<i64> {
    match value {
        Some(V::Int(n)) => Some(*n),
        Some(V::Num(n, scale)) => Some(*n as i64 / 10_i64.pow(u32::from(*scale))),
        Some(V::Double(d)) => Some(*d as i64),
        Some(V::Money(m)) => Some(*m / 10_000),
        Some(V::Bool(b)) => Some(i64::from(*b)),
        _ => None,
    }
}

/// How many arguments a call gives (none at all is a parameter missing), and
/// what each is: a value the parameter's type does not take is 2124, one
/// outside an Integer 5179.
pub(crate) fn check_arguments(
    spec: &Spec,
    execute: &Execute,
    args: &[V],
) -> Result<Arguments, Error> {
    match (args.len(), spec.params.len()) {
        (0, _) if spec.required > 0 => return Err(Error::Sql(SqlError::parameter_missing())),
        (given, _) if given < spec.required => {
            return Err(Error::Sql(SqlError::procedure_arguments()));
        }
        (given, wanted) if given > wanted => {
            // The procedures that take no argument, or only optional ones,
            // word it otherwise.
            return Err(Error::Sql(match spec.required {
                0 => SqlError::procedure_parameters(&execute.name, execute.pos),
                _ => SqlError::procedure_arguments(),
            }));
        }
        _ => {}
    }
    for (i, (value, (name, kind))) in args.iter().zip(spec.params).enumerate() {
        let target = match kind {
            Kind::Text | Kind::Char(_) => Ty::Str {
                field: FieldType::Character,
                length: 515,
            },
            Kind::Integer | Kind::Short | Kind::Flag | Kind::Bool => Ty::INTEGER,
            Kind::Memo | Kind::Options => Ty::Str {
                field: FieldType::Memo,
                length: 0,
            },
        };
        let source = match value {
            V::Null => continue,
            V::Bool(_) if matches!(kind, Kind::Flag) => continue,
            V::Bool(_) => Ty::Bool,
            V::Bin(_) => Ty::Bin {
                field: FieldType::Binary,
                length: 0,
            },
            V::Int(_) => Ty::INTEGER,
            V::Num(_, scale) => Ty::Num {
                precision: 18,
                scale: *scale,
            },
            V::Double(_) => Ty::Double,
            V::Money(_) => Ty::Money,
            V::Date(_) => Ty::Date,
            V::Time(_) => Ty::Time,
            V::TimeStamp(..) => Ty::TimeStamp,
            V::Str(s) => Ty::Str {
                field: FieldType::Character,
                length: s.chars().count().min(usize::from(u16::MAX)) as u16,
            },
            _ => Ty::Unknown,
        };
        // A negative number is at its digits.
        let pos = match (execute.args.get(i), value) {
            (None, _) => execute.pos,
            (Some(arg), _) => match &arg.kind {
                ExprKind::Neg(inner) => inner.pos,
                _ if matches!(value, V::Int(n) if *n < 0) => arg.pos + 1,
                _ => arg.pos,
            },
        };
        // A parameter's value is read as an Integer as it is bound, and any
        // number but 0 is on (`fixtures/sql/spddparam.json`).
        let bound =
            matches!(execute.args.get(i), Some(arg) if matches!(arg.kind, ExprKind::Param(_)));
        if matches!(kind, Kind::Bool)
            && !bound
            && let Some(source) = match value {
                V::Int(0 | 1) => None,
                V::Int(n) if i32::try_from(*n).is_ok() => Some("Integer"),
                V::Int(_) => Some("LongInteger"),
                V::Num(..) => Some("Numeric"),
                V::Double(_) => Some("Double"),
                V::Money(_) => Some("Money"),
                _ => None,
            }
        {
            return Err(Error::Sql(SqlError {
                state: "S0000".to_owned(),
                native: 2124,
                message: format!(
                    "Invalid operand for operator: <assignment> Boolean value is not assignment \
                     compatible with non-Boolean value. Target Data Type: <Integer> Source Data \
                     Type: <{source}> Target Name: {name} -- Location of error in the SQL \
                     statement is: {}",
                    pos + 1
                ),
            }));
        }
        if matches!((kind, value), (Kind::Options, V::Int(_))) {
            return Err(Error::Sql(SqlError::procedure_arguments()));
        }
        tflip_sql::dml::check_strict_argument(name, target, source, pos).map_err(Error::Sql)?;
        // A negative literal is the number after it, one past the largest, made
        // negative: -2147483648 (or -32768 for a ShortInt) is out of range,
        // though a parameter's value that is is not.
        if let (Kind::Integer | Kind::Short | Kind::Flag, V::Int(n)) = (kind, value) {
            let (min, max) = match kind {
                Kind::Short => (i64::from(i16::MIN), i64::from(i16::MAX)),
                _ => (i64::from(i32::MIN), i64::from(i32::MAX)),
            };
            if *n > max || *n < min || (!bound && *n == min) {
                return Err(ace_error::sql(5179, &format!("Field: \"{name}\"")));
            }
        }
    }
    let mut values = args.to_vec();
    let mut cut = false;
    for (value, (_, kind)) in values.iter_mut().zip(spec.params) {
        if let (V::Str(text), Kind::Char(length)) = (value, kind)
            && text.chars().count() > usize::from(*length)
        {
            *text = text.chars().take(usize::from(*length)).collect();
            cut = true;
        }
    }
    Ok(Arguments { values, cut })
}

/// A result column.
pub(crate) fn column(name: &str, field_type: FieldType, length: u16) -> Column {
    Column {
        name: name.to_owned(),
        field_type,
        length,
        decimals: 0,
    }
}
