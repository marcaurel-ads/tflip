//! The types of SQL values, and the column types of results.
//!
//! The rules for the result types of operators and functions are learnt
//! from the original engine (`fixtures/sql/statements.json`):
//!
//! - Exact numbers have a precision and a scale. An integer literal's
//!   precision is its digit count; SMALLINT columns have 5, INTEGER 10 and
//!   BIGINT 19; a NUMERIC(len, dec) column has `len` minus its sign and point.
//! - `+` and `-` keep the larger scale and add a digit to the larger
//!   integer part; `*` adds precisions and scales; `/` has precision
//!   p1 + p2 + s1 and scale s1 + p2 - s2; `%` keeps the larger scale and the
//!   divisor's integer digits.
//! - Integers combine to the wider integer type (two SMALLINTs give an
//!   INTEGER); MONEY absorbs exact numbers; DOUBLE absorbs everything but
//!   MONEY, which with DOUBLE gives CURDOUBLE.
//! - A NUMERIC result is `precision + 1` long, `+ 2` with decimals.

use tflip_storage::field::{Field, FieldType};

/// The width of an integer type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IntWidth {
    Short,
    Int,
    Long,
}

/// The type of an SQL value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Bool,
    /// An integer: its width, and its precision for mixing with NUMERIC.
    Int {
        width: IntWidth,
        digits: u16,
    },
    Num {
        precision: u16,
        scale: u16,
    },
    Double,
    CurDouble,
    Money,
    Date,
    Time,
    TimeStamp,
    /// Text: Character, CICharacter, VarChar, Memo, or their Unicode kin;
    /// `length` in characters.
    Str {
        field: FieldType,
        length: u16,
    },
    /// Bytes: Raw, VarBinary, Binary or Image.
    Bin {
        field: FieldType,
        length: u16,
    },
    Guid,
    /// NULL, or a parameter, before anything gives it a type.
    Unknown,
}

impl Ty {
    pub const INTEGER: Ty = Ty::Int {
        width: IntWidth::Int,
        digits: 10,
    };
    pub const SHORT: Ty = Ty::Int {
        width: IntWidth::Short,
        digits: 5,
    };
    pub const LONG: Ty = Ty::Int {
        width: IntWidth::Long,
        digits: 19,
    };

    /// Text of `length` characters, VarChar.
    pub fn varchar(length: u16) -> Ty {
        Ty::Str {
            field: FieldType::VarChar,
            length,
        }
    }

    /// The type of a column of `field`'s type.
    pub fn of_field(field: &Field) -> Ty {
        let length = field.length();
        match field.field_type {
            FieldType::Logical => Ty::Bool,
            // An ADT Numeric keeps a place for the sign; a DBF one's digits
            // take it too, so its precision is one more
            // (`fixtures/sql/dbf.json`).
            FieldType::Numeric => {
                let decimals = field.decimals();
                let point = u16::from(decimals > 0);
                let sign = u16::from(!matches!(field.layout, tflip_storage::field::Layout::Dbf(_)));
                Ty::Num { precision: length.saturating_sub(sign + point), scale: decimals }
            }
            FieldType::ShortInt => Ty::SHORT,
            FieldType::Integer | FieldType::AutoInc => Ty::INTEGER,
            FieldType::LongInt | FieldType::RowVersion => Ty::LONG,
            FieldType::Double => Ty::Double,
            FieldType::CurDouble => Ty::CurDouble,
            FieldType::Money => Ty::Money,
            FieldType::Date | FieldType::CompactDate => Ty::Date,
            FieldType::Time => Ty::Time,
            FieldType::TimeStamp | FieldType::ModTime => Ty::TimeStamp,
            FieldType::Character
            | FieldType::CiCharacter
            | FieldType::VarChar
            // The Advantage VarChar of DBF tables.
            | FieldType::LegacyVarChar
            | FieldType::Memo
            | FieldType::NChar
            | FieldType::NVarChar
            | FieldType::NMemo => Ty::Str { field: field.field_type, length },
            FieldType::Raw
            | FieldType::VarBinary
            | FieldType::Binary
            | FieldType::Image
            | FieldType::Other(_) => Ty::Bin { field: field.field_type, length },
            FieldType::Guid => Ty::Guid,
        }
    }

    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            Ty::Int { .. } | Ty::Num { .. } | Ty::Double | Ty::CurDouble | Ty::Money
        )
    }

    pub fn is_text(self) -> bool {
        matches!(self, Ty::Str { .. })
    }

    /// Whether this is Unicode text (NChar, NVarChar, NMemo).
    pub fn is_national(self) -> bool {
        matches!(
            self,
            Ty::Str {
                field: FieldType::NChar | FieldType::NVarChar | FieldType::NMemo,
                ..
            }
        )
    }

    /// Whether this is a date, a time or a timestamp.
    pub fn is_temporal(self) -> bool {
        matches!(self, Ty::Date | Ty::Time | Ty::TimeStamp)
    }

    /// Whether text of this type compares regardless of case.
    pub fn case_insensitive(self) -> bool {
        matches!(
            self,
            Ty::Str {
                field: FieldType::CiCharacter,
                ..
            }
        )
    }

    /// Whether this is a memo type: Memo, NMemo, Binary or Image.
    pub fn is_memo(self) -> bool {
        matches!(
            self,
            Ty::Str {
                field: FieldType::Memo | FieldType::NMemo,
                ..
            } | Ty::Bin {
                field: FieldType::Binary | FieldType::Image,
                ..
            }
        )
    }

    /// The text length of a text type, 0 for others.
    pub fn length(self) -> u16 {
        match self {
            Ty::Str { length, .. } | Ty::Bin { length, .. } => length,
            _ => 0,
        }
    }

    /// The precision and scale of an exact number.
    pub fn exact(self) -> Option<(u16, u16)> {
        match self {
            // A LONGINT counts a digit more than its 19 in arithmetic with
            // numbers.
            Ty::Int {
                width: IntWidth::Long,
                ..
            } => Some((20, 0)),
            Ty::Int { digits, .. } => Some((digits, 0)),
            Ty::Num { precision, scale } => Some((precision, scale)),
            _ => None,
        }
    }

    /// The column a result of this type is: its field type, declared length
    /// and decimals.
    pub fn column(self) -> (FieldType, u16, u16) {
        match self {
            Ty::Bool | Ty::Unknown => (FieldType::Logical, 1, 0),
            Ty::Int {
                width: IntWidth::Short,
                ..
            } => (FieldType::ShortInt, 2, 0),
            Ty::Int {
                width: IntWidth::Int,
                ..
            } => (FieldType::Integer, 4, 0),
            Ty::Int {
                width: IntWidth::Long,
                ..
            } => (FieldType::LongInt, 8, 0),
            // A result column is 32 long at most.
            Ty::Num { precision, scale } => (
                FieldType::Numeric,
                numeric_length(precision, scale).min(32),
                scale,
            ),
            Ty::Double => (FieldType::Double, 8, 15),
            Ty::CurDouble => (FieldType::CurDouble, 8, 2),
            Ty::Money => (FieldType::Money, 8, 4),
            Ty::Date => (FieldType::Date, 4, 0),
            Ty::Time => (FieldType::Time, 4, 0),
            Ty::TimeStamp => (FieldType::TimeStamp, 8, 0),
            Ty::Str { field, length } | Ty::Bin { field, length } => match field {
                FieldType::Memo | FieldType::NMemo | FieldType::Binary | FieldType::Image => {
                    (field, 9, 0)
                }
                _ => (field, length.max(1), 0),
            },
            Ty::Guid => (FieldType::Guid, 16, 0),
        }
    }
}

/// The length of a NUMERIC column of `precision` digits, `scale` of them
/// decimals: a sign, and a point if there are decimals.
pub fn numeric_length(precision: u16, scale: u16) -> u16 {
    precision + 1 + u16::from(scale > 0)
}

/// The result type of an arithmetic operator on numbers; `None` if the
/// operator does not take them.
pub fn arithmetic(op: crate::ast::BinOp, a: Ty, b: Ty) -> Option<Ty> {
    use crate::ast::BinOp;
    if !a.is_numeric() || !b.is_numeric() {
        return None;
    }
    if a == Ty::Double || b == Ty::Double {
        return Some(if a == Ty::Money || b == Ty::Money {
            Ty::CurDouble
        } else {
            Ty::Double
        });
    }
    if a == Ty::CurDouble || b == Ty::CurDouble {
        return Some(Ty::CurDouble);
    }
    if a == Ty::Money || b == Ty::Money {
        return Some(Ty::Money);
    }
    if let (Ty::Int { width: wa, .. }, Ty::Int { width: wb, .. }) = (a, b) {
        // A quotient or remainder of two SHORTINTs stays one.
        if matches!(op, BinOp::Div | BinOp::Mod) && wa == IntWidth::Short && wb == IntWidth::Short {
            return Some(Ty::SHORT);
        }
        let width = wa.max(wb).max(IntWidth::Int);
        let digits = match width {
            IntWidth::Long => 19,
            _ => 10,
        };
        return Some(Ty::Int { width, digits });
    }
    let (p1, s1) = a.exact()?;
    let (p2, s2) = b.exact()?;
    let (precision, scale) = match op {
        BinOp::Add | BinOp::Sub => {
            let scale = s1.max(s2);
            ((p1 - s1).max(p2 - s2) + 1 + scale, scale)
        }
        BinOp::Mul => (p1 + p2, s1 + s2),
        BinOp::Div => (p1 + p2 + s1, s1 + p2 - s2),
        BinOp::Mod => {
            let scale = s1.max(s2);
            (scale + (p2 - s2), scale)
        }
        _ => return None,
    };
    // A quotient of more than 32 digits keeps 20 decimals at most, and a
    // result is 32 characters long at most.
    let scale = match op {
        BinOp::Div if precision > 32 => scale.min(20),
        _ => scale,
    };
    let precision = precision.min(31 - u16::from(scale > 0));
    Some(Ty::Num { precision, scale })
}

/// The type that values of `a` and `b` both convert to, for the branches of
/// `CASE`, `IIF` and `COALESCE`; `None` if they do not mix.
pub fn merge(a: Ty, b: Ty) -> Option<Ty> {
    match (a, b) {
        (Ty::Unknown, t) | (t, Ty::Unknown) => Some(t),
        (a, b) if a == b => Some(a),
        (
            Ty::Str {
                field: fa,
                length: la,
            },
            Ty::Str {
                field: fb,
                length: lb,
            },
        ) => {
            let memo = |f| matches!(f, FieldType::Memo | FieldType::NMemo);
            let field = if memo(fa) || memo(fb) {
                FieldType::Memo
            } else if fa == fb {
                fa
            // A CHAR with an NCHAR stays fixed width and Unicode
            // (`fixtures/sql/claims.json`).
            } else if matches!(
                (fa, fb),
                (FieldType::Character, FieldType::NChar) | (FieldType::NChar, FieldType::Character)
            ) {
                FieldType::NChar
            } else {
                FieldType::VarChar
            };
            Some(Ty::Str {
                field,
                length: la.max(lb),
            })
        }
        // Bytes of two kinds are VarBinary, which a RAW joined to one is not
        // padded to its width (`fixtures/sql/gram_cast.json`).
        (
            Ty::Bin {
                field: fa,
                length: la,
            },
            Ty::Bin {
                field: fb,
                length: lb,
            },
        ) => Some(Ty::Bin {
            field: match fa == fb || fa.is_memo() {
                true => fa,
                false => FieldType::VarBinary,
            },
            length: la.max(lb),
        }),
        // A GUID with a RAW is bytes as wide as the GUID's 16
        // (`fixtures/sql/claims.json`).
        (
            Ty::Guid,
            Ty::Bin {
                field: FieldType::Raw,
                length,
            },
        )
        | (
            Ty::Bin {
                field: FieldType::Raw,
                length,
            },
            Ty::Guid,
        ) => Some(Ty::Bin {
            field: FieldType::Raw,
            length: length.max(16),
        }),
        // MONEY and a double that counts currency make a plain double.
        (Ty::Money, Ty::CurDouble) | (Ty::CurDouble, Ty::Money) => Some(Ty::Double),
        (a, b) if a.is_numeric() && b.is_numeric() => {
            if let (Some((p1, s1)), Some((p2, s2))) = (a.exact(), b.exact())
                && (matches!(a, Ty::Num { .. }) || matches!(b, Ty::Num { .. }))
            {
                let scale = s1.max(s2);
                return Some(Ty::Num {
                    precision: (p1 - s1).max(p2 - s2) + scale,
                    scale,
                });
            }
            arithmetic(crate::ast::BinOp::Add, a, b)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::BinOp;

    fn num(precision: u16, scale: u16) -> Ty {
        Ty::Num { precision, scale }
    }

    fn int(digits: u16) -> Ty {
        Ty::Int {
            width: IntWidth::Int,
            digits,
        }
    }

    #[test]
    fn exact_arithmetic() {
        // From the original engine: (operands) → column length, decimals.
        let column = |op, a, b| {
            let (_, length, decimals) = arithmetic(op, a, b).unwrap().column();
            (length, decimals)
        };
        assert_eq!(column(BinOp::Add, int(1), num(2, 1)), (5, 1));
        assert_eq!(column(BinOp::Add, int(2), num(4, 3)), (8, 3));
        assert_eq!(column(BinOp::Add, num(4, 2), num(4, 1)), (8, 2));
        assert_eq!(column(BinOp::Add, int(1), num(10, 2)), (13, 2));
        assert_eq!(column(BinOp::Add, int(1), num(5, 0)), (7, 0));
        assert_eq!(column(BinOp::Mul, num(4, 2), num(4, 2)), (10, 4));
        assert_eq!(column(BinOp::Mul, num(10, 2), num(10, 2)), (22, 4));
        assert_eq!(column(BinOp::Div, int(1), num(2, 1)), (5, 1));
        assert_eq!(column(BinOp::Div, num(4, 2), int(2)), (10, 4));
        assert_eq!(column(BinOp::Div, num(4, 3), num(4, 1)), (13, 6));
        assert_eq!(column(BinOp::Mod, int(1), num(4, 2)), (6, 2));
        assert_eq!(column(BinOp::Mod, num(4, 3), num(4, 1)), (8, 3));
        assert_eq!(
            arithmetic(BinOp::Add, Ty::Money, Ty::Double),
            Some(Ty::CurDouble)
        );
        assert_eq!(
            arithmetic(BinOp::Add, Ty::SHORT, Ty::SHORT),
            Some(Ty::INTEGER)
        );
        assert_eq!(arithmetic(BinOp::Add, Ty::SHORT, Ty::LONG), Some(Ty::LONG));
    }
}
