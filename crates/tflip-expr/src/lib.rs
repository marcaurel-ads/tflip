//! The Advantage expression engine: the expressions of filters (`AdsSetAOF`,
//! `AdsSetFilter`), index orders and `AdsEval*Expr`, compiled against a
//! table's fields and evaluated on its records.
//!
//! An expression is compiled once ([`Expr::compile`]): parsed, its names
//! resolved against a [`Schema`] and its type checked, which also fixes the
//! type and width of its result. It is then evaluated on records through a
//! [`Row`]. Nothing here knows the ADT format; the engine adapts its tables
//! to the two traits.
//!
//! The language and its quirks are learnt from the documentation and from
//! calling the original engine (`fixtures/expr/`):
//!
//! - Strings are Windows-1252 bytes and compare as bytes; `UPPER` and
//!   `LOWER` change ASCII letters only. (Text of NChar and NVarChar fields is
//!   Windows-1252 here too, which loses what it holds; an index's key
//!   expression is evaluated by [`Expr::eval_unicode`] where it keeps it.)
//! - Numbers are doubles. Each numeric expression also has a display format
//!   (a length and decimals) that `STR` uses by default.
//! - NULL propagates through operators and functions but `ISNULL`, `EMPTY`,
//!   `COALESCE` and `IIF`'s branches; a comparison with NULL is NULL, which a
//!   filter takes as false.
//! - Unlike in the documentation, `NOT` binds more tightly than comparisons,
//!   and `+` or `-` where an operand is expected starts a number (a sign
//!   alone is 0): `--k` is `0 - k`, and `-k` is two operands in a row.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use tflip_storage::field::FieldType;
use tflip_storage::value::Value;

mod check;
mod eval;
mod func;
mod key;
mod lex;
pub mod number;
pub mod optimize;
mod parse;
pub mod text;
mod wide;

pub use eval::Val;
/// Helpers the SQL engine's functions share with the expression engine's.
pub use func::{
    base64_decode, base64_encode, day_of_year, guid_string, iso_week, new_id, soundex, weekday,
};
pub use key::{KeyKind, key_kind};
pub use wide::{UVal, Unicode};

/// Why an expression cannot be compiled. The original engine reports each
/// with a code of its own per use; [`ErrorKind::code`] gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorKind {
    /// Text after a complete expression (`1)`).
    Incomplete,
    /// A function called with the wrong number of arguments.
    ParameterCount,
    /// A string without its closing quote.
    MissingDelimiter,
    /// An operator where an operand is expected, or two operands in a row.
    UnsupportedOperator,
    /// A function the engine does not have.
    UnsupportedFunction,
    /// Operands or arguments of the wrong type.
    TypeMismatch,
    /// A field qualified by another table's alias.
    UnknownAlias,
    /// A name that is neither a field nor a keyword, or a missing operand.
    UnknownIdentifier,
    /// A memo, binary or image field where only `EMPTY`, `ISNULL` and
    /// `CONTAINS` take one.
    InvalidFieldType,
    /// More opening than closing parentheses.
    Parentheses,
    /// A collation the engine does not have (`COLLATE`).
    Collation,
}

impl ErrorKind {
    /// The error code for this failure where `base` is the code of the
    /// use's first error: 3100 for record filters, 3400 for
    /// `AdsIsExprValid`, 3500 for expressions evaluated otherwise.
    pub fn code(self, base: u32) -> u32 {
        base + match self {
            ErrorKind::Incomplete => 1,
            ErrorKind::ParameterCount => 2,
            ErrorKind::MissingDelimiter => 3,
            ErrorKind::UnsupportedOperator => 6,
            ErrorKind::UnsupportedFunction => 7,
            ErrorKind::TypeMismatch => 9,
            ErrorKind::UnknownAlias => 10,
            ErrorKind::UnknownIdentifier => 11,
            ErrorKind::InvalidFieldType => 15,
            ErrorKind::Parentheses => 18,
            ErrorKind::Collation => 24,
        }
    }
}

/// Why evaluating a compiled expression failed on a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Runtime {
    /// Division (or `%`) by zero: 4420.
    DivideByZero,
    /// Text that is not hexadecimal digits (`CHAR2HEX`): 4409.
    InvalidHex,
    /// A `NEWIDSTRING` format its repertoire does not have: 4407.
    GuidFormat,
    /// Division by 0 in an index's key expression, or in an AOF: 4020.
    DivideInKey,
    /// Division by 0 in a filter: 4120.
    DivideInFilter,
    /// Division by 0 in an index's condition: 4220.
    DivideInCondition,
    /// A value that outgrows its key (`BASE64ENCODE` of text with
    /// supplementary characters, which counts for two): 5005.
    KeyBuffer,
}

impl Runtime {
    /// The original engine's error code.
    pub fn code(self) -> u32 {
        match self {
            Runtime::DivideByZero => 4420,
            Runtime::InvalidHex => 4409,
            Runtime::GuidFormat => 4407,
            Runtime::DivideInKey => 4020,
            Runtime::DivideInFilter => 4120,
            Runtime::DivideInCondition => 4220,
            Runtime::KeyBuffer => 5005,
        }
    }
}

/// Why evaluating an expression on a record failed: the expression, or
/// reading the record.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure<E> {
    Runtime(Runtime),
    Row(E),
}

impl<E> From<Runtime> for Failure<E> {
    fn from(runtime: Runtime) -> Self {
        Failure::Runtime(runtime)
    }
}

/// How a number is displayed: its length and decimals, as `STR` writes it
/// by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Format {
    pub length: u16,
    pub decimals: u16,
    /// Whether the value must fit the format: a field's value, as is or
    /// through `ABS`. The original engine refuses to return such a value
    /// that does not (`AdsEvalNumericExpr` fails with 4504).
    pub fixed: bool,
}

impl Format {
    /// A format that need not fit.
    pub const fn new(length: u16, decimals: u16) -> Format {
        Format {
            length,
            decimals,
            fixed: false,
        }
    }
}

/// The type of an expression's result, with its width where results of
/// the type differ in size. A NULL result is as many zero bytes as the
/// width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Type {
    Logical,
    Number(Format),
    /// A date, as a Julian day; day 0 is the empty date.
    Date,
    /// Text of at most `width` bytes; `ci` for a CICharacter field, which
    /// compares regardless of case.
    String {
        width: u16,
        ci: bool,
    },
    Time,
    TimeStamp,
    /// Raw bytes: Raw, VarBinary and GUID fields, `;` joins, `CHAR2HEX`.
    Binary {
        width: u16,
    },
    /// A memo, binary or image field, which only a few functions take.
    Memo,
    /// The `NULL` literal, which has no type of its own.
    Null,
}

impl Type {
    /// The ACE type constant `AdsEvalTestExpr` reports: `ADS_LOGICAL` (1),
    /// `ADS_NUMERIC` (2, also for dates), `ADS_STRING` (4) or `ADS_RAW` (16);
    /// `None` for the `NULL` literal.
    pub fn ace_type(self) -> Option<u16> {
        match self {
            Type::Logical => Some(1),
            Type::Number(_) | Type::Date => Some(2),
            Type::String { .. } => Some(4),
            Type::Time | Type::TimeStamp | Type::Binary { .. } => Some(16),
            Type::Memo | Type::Null => None,
        }
    }

    /// The width of the type's key, and of its NULL result in
    /// `AdsEvalStringExpr`.
    pub fn width(self) -> u16 {
        match self {
            Type::Logical => 1,
            Type::Number(_) | Type::Date | Type::TimeStamp => 8,
            Type::Time => 4,
            Type::String { width, .. } | Type::Binary { width } => width,
            Type::Memo | Type::Null => 0,
        }
    }
}

/// A field as an expression sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    /// The field's position, as [`Row::value`] takes it.
    pub index: usize,
    pub field_type: FieldType,
    pub length: u16,
    pub decimals: u16,
}

/// The table an expression is compiled for.
pub trait Schema {
    /// The table's alias, which may qualify field names (`alias->field`).
    fn alias(&self) -> &str;
    /// The field named `name`, regardless of case.
    fn field(&self, name: &str) -> Option<Field>;
    /// The name the table gives field `index` (see [`Field::index`]), for
    /// printing expressions as the original engine does; `None` to print
    /// names as written.
    fn field_name(&self, index: usize) -> Option<String> {
        let _ = index;
        None
    }
}

/// The record an expression is evaluated on.
pub trait Row {
    type Error;
    /// The value of field `index` (see [`Field::index`]).
    fn value(&mut self, index: usize) -> Result<Value, Self::Error>;
    /// The record's number (`RECNO()`).
    fn record_number(&self) -> u32;
    /// Whether the record is deleted (`DELETED()`).
    fn deleted(&self) -> bool;
}

/// How `UPPER` and `LOWER` fold case under a static collation. The original
/// engine's `UPPER(b)` is `weights[upper[b]]` and `LOWER(b)` is
/// `weights[lower[b]]` — the byte folded through the collation's case table and
/// then weighed — so each table here already holds the weight of the folded
/// byte. Under the default collation (identity weights, an ASCII-only case
/// table) this is plain ASCII folding, which [`Env`] leaves as `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseFold {
    /// `UPPER`: each byte folded to upper case and weighed.
    pub upper: [u8; 256],
    /// `LOWER`: each byte folded to lower case and weighed.
    pub lower: [u8; 256],
}

/// What evaluation depends on besides the record.
#[derive(Debug, Clone, PartialEq)]
pub struct Env {
    /// The `AdsSetExact` setting: whether `=` compares whole strings.
    pub exact: bool,
    /// The date format of `CTOD` and `DTOC` (`MM/DD/CCYY`).
    pub date_format: String,
    /// The epoch of years of two digits (`AdsSetEpoch`): 1900 by default.
    pub epoch: u16,
    /// The local date and time, for `DATE()`, `NOW()` and `TIME()`: a Julian
    /// day and milliseconds since midnight.
    pub now: (i32, u32),
    /// The same in UTC, for `NOW_UTC()`.
    pub utc: (i32, u32),
    /// How `UPPER` and `LOWER` fold case, if a non-default static collation is
    /// in force; `None` for plain ASCII folding under the default collation.
    pub case_fold: Option<CaseFold>,
    /// The static collation's byte-to-weight table, by which string
    /// comparisons (`=`, `<`, ranges) order and equate text; `None` under the
    /// default collation, whose weights are the bytes themselves. A
    /// case-insensitive comparison folds through [`CaseFold::lower`] instead,
    /// which already weighs the folded byte.
    pub weights: Option<[u8; 256]>,
    /// The table's dynamic collation (an ADT table opened with one), by
    /// whose weights ([`tflip_storage::collation::Dynamic::weights`]) the ordering comparisons
    /// compare text instead, as the original engine's do.
    pub dynamic: Option<std::sync::Arc<tflip_storage::collation::Dynamic>>,
    /// Whether `EMPTY()` holds for a blank value (blank text, zero, `.F.`,
    /// the empty date) and not for NULL, as on a DBF table's records; on an
    /// ADT table's it holds for NULL alone.
    pub blank_empty: bool,
}

/// A compiled expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    node: check::Node,
}

impl Expr {
    /// Compiles `text` for the table `schema` describes.
    pub fn compile(text: &str, schema: &dyn Schema) -> Result<Expr, ErrorKind> {
        let tokens = lex::lex(text, Some(schema));
        let tree = parse::parse(&tokens, Some(schema))?;
        let node = check::check(&tree, schema)?;
        if node.ty == Type::Memo {
            return Err(ErrorKind::InvalidFieldType);
        }
        Ok(Expr { node })
    }

    /// The type of the expression's result.
    pub fn ty(&self) -> Type {
        self.node.ty
    }

    /// The expression's value on `row`.
    pub fn eval<R: Row + ?Sized>(&self, row: &mut R, env: &Env) -> Result<Val, Failure<R::Error>> {
        eval::eval(&self.node, row, env)
    }

    /// How the expression takes Unicode text (NChar and NVarChar fields),
    /// which [`Expr::eval`] does not keep: its values are Windows-1252.
    pub fn unicode(&self) -> Unicode {
        wide::unicode(&self.node)
    }

    /// The expression's value on `row`, with Unicode text kept as it is;
    /// `None` if [`Expr::unicode`] is [`Unicode::Unsupported`].
    pub fn eval_unicode<R: Row + ?Sized>(
        &self,
        row: &mut R,
        env: &Env,
    ) -> Result<Option<UVal>, Failure<R::Error>> {
        wide::eval(&self.node, row, env)
    }

    /// Whether the expression holds on `row`: true if its value is true;
    /// false if it is false, NULL or not logical.
    pub fn holds<R: Row + ?Sized>(
        &self,
        row: &mut R,
        env: &Env,
    ) -> Result<bool, Failure<R::Error>> {
        Ok(matches!(self.eval(row, env)?, Val::Logical(true)))
    }
}

/// The value of `expr`, which does not read the record (a constant);
/// `None` if evaluating it fails.
pub fn eval_constant(expr: &Expr, env: &Env) -> Option<Val> {
    struct NoRow;
    impl Row for NoRow {
        type Error = ();
        fn value(&mut self, _: usize) -> Result<Value, ()> {
            Err(())
        }
        fn record_number(&self) -> u32 {
            0
        }
        fn deleted(&self) -> bool {
            false
        }
    }
    expr.eval(&mut NoRow, env).ok()
}

#[cfg(test)]
mod tests;
