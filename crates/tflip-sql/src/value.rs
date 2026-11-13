//! SQL values while a statement runs, from and to field values.

use std::cmp::Ordering;
use std::sync::Arc;

use tflip_storage::collation::{Dynamic, Static};
use tflip_storage::cp1252;
use tflip_storage::value::Value;

use crate::error::SqlError;
use crate::types::{IntWidth, Ty};

/// A collation SQL compares text by, other than the default one (whose
/// weights are the bytes themselves).
#[derive(Debug, Clone)]
pub enum Collation {
    /// The connection's static ANSI collation.
    Static(Box<Static>),
    /// The dynamic collation of the tables a statement opens
    /// (`AdsStmtSetTableCollation`, else the connection's `AdsSetCollation`):
    /// the original engine compares all of the statement's text by it, a
    /// literal's too.
    Dynamic(Arc<Dynamic>),
    /// The same as a live cursor's `WHERE` compares text by it (in the
    /// original engine an AOF on the table): as `GROUP BY` weighs it, `ß` as
    /// `ss` (probed, `fixtures/sql/collation_dynamic.json`).
    DynamicFilter(Arc<Dynamic>),
}

impl Collation {
    /// How `UPPER` and `LOWER` fold case under it: by its own case tables.
    pub fn case_fold(&self) -> crate::CaseFold {
        let (upper, lower) = match self {
            Collation::Static(c) => (c.upper, c.lower),
            Collation::Dynamic(c) | Collation::DynamicFilter(c) => (c.upper, c.lower),
        };
        crate::CaseFold { upper, lower }
    }

    /// The dynamic collation, if it is one.
    pub fn dynamic(&self) -> Option<&Arc<Dynamic>> {
        match self {
            Collation::Static(_) => None,
            Collation::Dynamic(c) | Collation::DynamicFilter(c) => Some(c),
        }
    }

    /// The same collation as a live cursor's `WHERE` compares by.
    pub fn for_filter(self: &Arc<Collation>) -> Arc<Collation> {
        match &**self {
            Collation::Dynamic(c) => Arc::new(Collation::DynamicFilter(Arc::clone(c))),
            _ => Arc::clone(self),
        }
    }
}

/// A value.
#[derive(Debug, Clone, PartialEq)]
pub enum V {
    Null,
    Bool(bool),
    Int(i64),
    /// An exact number: the value × 10^scale, and the scale.
    Num(i128, u16),
    /// DOUBLE and CURDOUBLE.
    Double(f64),
    /// MONEY, × 10,000.
    Money(i64),
    /// A Julian day.
    Date(i32),
    /// Milliseconds since midnight.
    Time(u32),
    TimeStamp(i32, u32),
    Str(String),
    Bin(Vec<u8>),
    Guid([u8; 16]),
    /// An empty field of a Clipper or FoxPro table, which has no NULL: blank,
    /// or 0, false or spaces. It is NULL to `IS NULL`, `IFNULL` and
    /// `COALESCE`; grouped, made `DISTINCT` and shown as [`Blank::shown`]; and
    /// its value (a blank one's 0, empty date, false or spaces) to everything
    /// else — comparisons, arithmetic, aggregates. Only a column read
    /// straight from a record is one; [`eval`](crate::eval) gives its value.
    Blank(Box<Blank>),
}

/// A blank field's values ([`V::Blank`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Blank {
    /// What expressions compute with.
    pub value: V,
    /// What a result shows: for a blank field NULL, but the value of text, a
    /// double or an integer; else the value (`fixtures/sql/dbf.json`).
    pub shown: V,
}

pub(crate) fn pow10(n: u16) -> i128 {
    10i128.saturating_pow(u32::from(n))
}

impl V {
    pub fn is_null(&self) -> bool {
        matches!(self, V::Null | V::Blank(_))
    }

    /// Whether the value has no decimals, as an integer needs it; anything
    /// that is not a number has none.
    pub fn is_whole(&self) -> bool {
        match self {
            V::Num(n, scale) => n % pow10(*scale) == 0,
            V::Double(d) => d.fract() == 0.0,
            V::Money(m) => m % 10_000 == 0,
            _ => true,
        }
    }

    /// The value's type as the original engine's messages name it.
    pub fn type_name(&self) -> &'static str {
        match self {
            V::Null => "Unknown",
            V::Bool(_) => "Logical",
            V::Int(_) => "Integer",
            V::Num(..) => "Numeric",
            V::Double(_) => "Double",
            V::Money(_) => "Money",
            V::Date(_) => "Date",
            V::Time(_) => "Time",
            V::TimeStamp(..) => "Timestamp",
            V::Str(_) => "Character",
            V::Bin(_) => "Binary",
            V::Guid(_) => "GUID",
            V::Blank(blank) => blank.shown.type_name(),
        }
    }

    /// What a result shows of the value, grouped and ordered by.
    pub fn shown(&self) -> &V {
        match self {
            V::Blank(blank) => &blank.shown,
            v => v,
        }
    }

    /// Whether this is an empty value, as xBase's `EMPTY()` has it: 0, false,
    /// the empty date or text of spaces.
    pub fn is_empty(&self) -> bool {
        match self {
            V::Int(n) => *n == 0,
            V::Num(n, _) => *n == 0,
            V::Double(d) => *d == 0.0,
            V::Money(m) => *m == 0,
            V::Bool(b) => !b,
            V::Date(d) => *d == 0,
            V::Str(s) => s.trim_matches(' ').is_empty(),
            _ => false,
        }
    }

    /// The value expressions compute with: an empty field's value.
    pub fn unblank(self) -> V {
        match self {
            V::Blank(blank) => blank.value,
            v => v,
        }
    }

    /// The value of a field of type `ty`, as read from a record.
    pub fn from_field(value: Value, ty: Ty) -> V {
        match value {
            Value::Null => V::Null,
            Value::Text(s) => V::Str(s),
            Value::Binary(b) => V::Bin(b),
            Value::Logical(b) => V::Bool(b),
            Value::Numeric(text) => {
                let scale = match ty {
                    Ty::Num { scale, .. } => scale,
                    _ => 0,
                };
                // Blank, but not NULL (in a VFP field that takes no NULL),
                // is 0, as the original engine computes with it.
                match text.trim() {
                    "" => V::Num(0, scale),
                    text => parse_exact(text, scale).map_or(V::Null, |n| V::Num(n, scale)),
                }
            }
            Value::Integer(n) => V::Int(n as i64),
            Value::Double(d) => V::Double(d),
            Value::Money(m) => V::Money(m),
            Value::Date(d) => V::Date(d),
            Value::Time(t) => V::Time(t),
            Value::TimeStamp(d, t) => V::TimeStamp(d, t),
            Value::Guid(g) => V::Guid(g),
            // A blank VFP logical is false; a blank date or timestamp empty,
            // which is not NULL in a field that takes no NULL
            // (`fixtures/sql/dbf.json`).
            Value::Blank if ty == Ty::Bool => V::Bool(false),
            Value::Blank if ty == Ty::Date => V::Date(0),
            Value::Blank if ty == Ty::TimeStamp => V::TimeStamp(0, 0),
            Value::Blank => V::Null,
        }
    }

    /// The field value of this value in a column of type `ty`, for a
    /// result set; `column` names the column in errors.
    pub fn to_field(&self, ty: Ty, column: &str) -> Result<Value, SqlError> {
        let too_large = || SqlError::too_large(column);
        Ok(match (self, ty) {
            (V::Blank(blank), _) => return blank.shown.to_field(ty, column),
            (V::Null, _) => Value::Null,
            (V::Bool(b), _) => Value::Logical(*b),
            (_, Ty::Int { width, .. }) => {
                let n = self.as_i128().ok_or_else(too_large)?;
                let fits = match width {
                    // The lowest number of the width is the one that is no
                    // value (`fixtures/sql/gram_shift.json`).
                    IntWidth::Short => i16::try_from(n).is_ok_and(|n| n != i16::MIN),
                    IntWidth::Int => i32::try_from(n).is_ok_and(|n| n != i32::MIN),
                    IntWidth::Long => i64::try_from(n).is_ok(),
                };
                if !fits {
                    return Err(too_large());
                }
                if width == IntWidth::Long && n == i128::from(i64::MIN) {
                    return Ok(Value::Null);
                }
                Value::Integer(n)
            }
            (_, Ty::Num { precision, scale }) => {
                let n = self.as_scaled_rounded(scale).ok_or_else(too_large)?;
                let mut text = exact_text(n, scale);
                let length = usize::from(crate::types::numeric_length(precision, scale));
                // A place is kept for the sign; without room for it, a zero
                // before the point goes.
                let room = length - usize::from(n >= 0);
                if text.len() > room && (text.starts_with("0.") || text.starts_with("-0.")) {
                    text = text.replacen("0.", ".", 1);
                }
                if text.len() > room {
                    return Err(too_large());
                }
                Value::Numeric(text)
            }
            (_, Ty::Double | Ty::CurDouble) => {
                let d = self.as_f64().ok_or_else(too_large)?;
                if !d.is_finite() {
                    let what = if d.is_nan() { "NAN" } else { "INF" };
                    return Err(SqlError::invalid_float(what, column));
                }
                Value::Double(d)
            }
            (_, Ty::Money) => Value::Money(
                i64::try_from(self.as_scaled(4).ok_or_else(too_large)?)
                    .map_err(|_| SqlError::money_too_large(column))?,
            ),
            (V::Date(d), Ty::TimeStamp) => Value::TimeStamp(*d, 0),
            (V::Date(d), _) => Value::Date(*d),
            (V::Time(t), _) => Value::Time(*t),
            (V::TimeStamp(d, _), Ty::Date) => Value::Date(*d),
            (V::TimeStamp(d, t), _) => Value::TimeStamp(*d, *t),
            (V::Str(s), Ty::Bin { .. }) => Value::Binary(cp1252::encode(s)),
            (V::Str(s), _) => Value::Text(s.clone()),
            (V::Bin(b), Ty::Str { .. }) => Value::Text(cp1252::decode(b)),
            (V::Bin(b), _) => Value::Binary(b.clone()),
            (V::Guid(g), _) => Value::Guid(*g),
            (V::Int(_) | V::Num(..) | V::Double(_) | V::Money(_), _) => Value::Text(self.display()),
        })
    }

    /// An exact number as an integer, truncated; `None` for others.
    pub fn as_i128(&self) -> Option<i128> {
        Some(match self {
            V::Int(n) => i128::from(*n),
            V::Num(n, scale) => n / pow10(*scale),
            V::Money(m) => i128::from(*m) / 10_000,
            V::Double(d) if d.is_finite() => d.trunc() as i128,
            V::Bool(b) => i128::from(*b),
            _ => return None,
        })
    }

    /// The number × 10^scale, truncated.
    pub fn as_scaled(&self, scale: u16) -> Option<i128> {
        let rescale = |n: i128, from: u16| {
            if from <= scale {
                n.checked_mul(pow10(scale - from))
            } else {
                Some(n / pow10(from - scale))
            }
        };
        match self {
            V::Int(n) => rescale(i128::from(*n), 0),
            V::Num(n, from) => rescale(*n, *from),
            V::Money(m) => rescale(i128::from(*m), 4),
            V::Bool(b) => rescale(i128::from(*b), 0),
            V::Double(d) if d.is_finite() => {
                let scaled = (d * pow10(scale) as f64).round();
                (scaled.abs() < 1e38).then_some(scaled as i128)
            }
            _ => None,
        }
    }

    /// The number × 10^scale, rounded half away from zero where an exact
    /// number has more decimals than `scale`, as a column takes it.
    pub fn as_scaled_rounded(&self, scale: u16) -> Option<i128> {
        match self {
            V::Num(n, from) if *from > scale => {
                let unit = pow10(from - scale);
                let (whole, rest) = (n / unit, n % unit);
                Some(match rest.abs() * 2 >= unit {
                    true => whole + n.signum(),
                    false => whole,
                })
            }
            V::Money(m) => V::Num(i128::from(*m), 4).as_scaled_rounded(scale),
            _ => self.as_scaled(scale),
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        Some(match self {
            V::Int(n) => *n as f64,
            V::Num(n, scale) => *n as f64 / pow10(*scale) as f64,
            V::Money(m) => *m as f64 / 10_000.0,
            V::Double(d) => *d,
            V::Bool(b) => f64::from(u8::from(*b)),
            _ => return None,
        })
    }

    /// The value as text, for concatenation with numbers and messages.
    pub fn display(&self) -> String {
        match self {
            V::Null => String::new(),
            V::Blank(blank) => blank.value.display(),
            V::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
            V::Int(n) => n.to_string(),
            V::Num(n, scale) => exact_text(*n, *scale),
            V::Money(m) => exact_text(i128::from(*m), 4),
            V::Double(d) => d.to_string(),
            V::Str(s) => s.clone(),
            V::Bin(b) => cp1252::decode(b),
            V::Guid(g) => guid_text(g),
            V::Date(_) | V::Time(_) | V::TimeStamp(..) => format!("{self:?}"),
        }
    }
}

/// A GUID's stored bytes as lower-case `8-4-4-4-12` text (the first three
/// groups are little-endian), as the original engine casts one to text.
pub(crate) fn guid_text(g: &[u8; 16]) -> String {
    let hex = |bytes: &mut dyn Iterator<Item = &u8>| -> String {
        bytes.map(|b| format!("{b:02x}")).collect()
    };
    format!(
        "{}-{}-{}-{}-{}",
        hex(&mut g[0..4].iter().rev()),
        hex(&mut g[4..6].iter().rev()),
        hex(&mut g[6..8].iter().rev()),
        hex(&mut g[8..10].iter()),
        hex(&mut g[10..16].iter())
    )
}

/// Text to an exact number × 10^scale: digits with an optional sign and
/// point; `None` if it is not one. Extra decimals are cut off.
pub fn parse_exact(text: &str, scale: u16) -> Option<i128> {
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if whole.is_empty() && fraction.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let mut n: i128 = 0;
    for b in whole.bytes() {
        n = n.checked_mul(10)?.checked_add(i128::from(b - b'0'))?;
    }
    let mut fraction = fraction.bytes();
    for _ in 0..scale {
        let digit = fraction.next().map_or(0, |b| b - b'0');
        n = n.checked_mul(10)?.checked_add(i128::from(digit))?;
    }
    Some(if negative { -n } else { n })
}

/// The number a parameter given as text or a double is for a NUMERIC column
/// of `scale` decimals, cut there, if it has more decimals than that: the
/// original sets it so and reports 2102 (`fixtures/sql/gram_assign.json`).
pub fn cut_decimals(given: &V, scale: u16) -> Option<V> {
    let text = match given {
        V::Str(s) => s.trim().to_owned(),
        V::Double(d) if d.is_finite() => format!("{d}"),
        _ => return None,
    };
    let (_, fraction) = text.split_once('.')?;
    if fraction.bytes().skip(usize::from(scale)).all(|b| b == b'0') {
        return None;
    }
    parse_exact(&text, scale).map(|n| V::Num(n, scale))
}

/// An exact number as NUMERIC fields hold it: `-`, the integer digits (at
/// least one), and the decimals.
pub(crate) fn exact_text(n: i128, scale: u16) -> String {
    let magnitude = n.unsigned_abs();
    let unit = pow10(scale) as u128;
    let mut text = String::new();
    if n < 0 {
        text.push('-');
    }
    text.push_str(&(magnitude / unit).to_string());
    if scale > 0 {
        text.push('.');
        text.push_str(&format!(
            "{:0width$}",
            magnitude % unit,
            width = usize::from(scale)
        ));
    }
    text
}

/// Compares two non-NULL values of comparable types; text compares as
/// [`compare_text`] does, by `collation`'s weights and regardless of case
/// if `ci`. `None` if they do not compare.
pub(crate) fn compare(a: &V, b: &V, ci: bool, collation: Option<&Collation>) -> Option<Ordering> {
    Some(match (a, b) {
        (V::Str(x), V::Str(y)) => compare_text(x, y, ci, collation),
        (V::Bin(x), V::Bin(y)) => x.cmp(y),
        (V::Bool(x), V::Bool(y)) => x.cmp(y),
        (V::Date(x), V::Date(y)) => x.cmp(y),
        (V::Time(x), V::Time(y)) => x.cmp(y),
        (V::TimeStamp(d1, t1), V::TimeStamp(d2, t2)) => (d1, t1).cmp(&(d2, t2)),
        (V::Guid(x), V::Guid(y)) => x.cmp(y),
        (V::Int(x), V::Int(y)) => x.cmp(y),
        (x, y) if is_number(x) && is_number(y) => {
            if matches!(x, V::Double(_)) || matches!(y, V::Double(_)) {
                x.as_f64()?.partial_cmp(&y.as_f64()?)?
            } else {
                let scale = scale_of(x).max(scale_of(y));
                x.as_scaled(scale)?.cmp(&y.as_scaled(scale)?)
            }
        }
        _ => return None,
    })
}

/// How `GROUP BY`, `DISTINCT` and a sort not in a temporary index order two
/// values of a column: NULL first, then as comparisons compare (text by
/// `collation`'s weights, regardless of case if `ci`), but text under a
/// dynamic collation as [`Weighing::Grouped`].
pub fn order(a: &V, b: &V, ci: bool, collation: Option<&Collation>) -> Ordering {
    ordered(a, b, ci, collation, Weighing::Grouped)
}

/// How a temporary index orders two values of a column (an `ORDER BY` the
/// original engine sorts in one): as [`order`], but text under a dynamic
/// collation as [`Weighing::Indexed`].
pub fn order_indexed(a: &V, b: &V, ci: bool, collation: Option<&Collation>) -> Ordering {
    ordered(a, b, ci, collation, Weighing::Indexed)
}

/// How `MIN` and `MAX` order two values: as [`order`], but text under a
/// dynamic collation as comparisons compare it ([`Weighing::Compared`]).
pub(crate) fn order_compared(a: &V, b: &V, ci: bool, collation: Option<&Collation>) -> Ordering {
    ordered(a, b, ci, collation, Weighing::Compared)
}

fn ordered(a: &V, b: &V, ci: bool, collation: Option<&Collation>, weighing: Weighing) -> Ordering {
    let (a, b) = (a.shown(), b.shown());
    match (a.is_null(), b.is_null()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        (false, false) => {}
    }
    match (a, b, collation.and_then(Collation::dynamic)) {
        (V::Str(x), V::Str(y), Some(dynamic)) => compare_dynamic(x, y, ci, dynamic, weighing),
        _ => compare(a, b, ci, collation).unwrap_or(Ordering::Equal),
    }
}

/// The three ways the original engine's SQL weighs text under a dynamic
/// collation (probed, `fixtures/sql/collation_dynamic.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Weighing {
    /// Comparisons and `MIN`/`MAX`: [`Dynamic::compare`] (`ß` after `ss`).
    Compared,
    /// `GROUP BY`, `DISTINCT`, `UNION` and a live cursor's `WHERE`:
    /// [`Dynamic::grouping_weights`] (`ß` as `ss`, a blank before a
    /// character that weighs least).
    Grouped,
    /// A temporary index: the keys, [`Dynamic::weights`] (a character that
    /// weighs least before a blank).
    Indexed,
}

fn is_number(v: &V) -> bool {
    matches!(
        v,
        V::Int(_) | V::Num(..) | V::Double(_) | V::Money(_) | V::Bool(_)
    )
}

fn scale_of(v: &V) -> u16 {
    match v {
        V::Num(_, scale) => *scale,
        V::Money(_) => 4,
        _ => 0,
    }
}

/// Compares text as the original engine does: Windows-1252 bytes, the
/// shorter padded with spaces. Under the default collation these are the
/// bytes themselves, in lower case if `ci` (`[` sorts before `a`); under a
/// static `collation` each byte's weight, of its lower case if `ci`; under
/// a dynamic one as [`Dynamic::compare`] compares the text (a live cursor's
/// filter, as [`Dynamic::grouping_weights`] weigh it), in lower case if
/// `ci`.
pub(crate) fn compare_text(a: &str, b: &str, ci: bool, collation: Option<&Collation>) -> Ordering {
    let collation = match collation {
        Some(Collation::Dynamic(dynamic)) => {
            return compare_dynamic(a, b, ci, dynamic, Weighing::Compared);
        }
        Some(Collation::DynamicFilter(dynamic)) => {
            return compare_dynamic(a, b, ci, dynamic, Weighing::Grouped);
        }
        Some(Collation::Static(collation)) => Some(collation),
        None => None,
    };
    let weigh = |b: u8| match collation {
        Some(collation) => {
            let b = if ci {
                collation.lower[usize::from(b)]
            } else {
                b
            };
            collation.weights[usize::from(b)]
        }
        None if ci => b.to_ascii_lowercase(),
        None => b,
    };
    let (a, b) = (cp1252::encode(a), cp1252::encode(b));
    let pad = weigh(b' ');
    (0..a.len().max(b.len()))
        .map(|i| {
            let x = a.get(i).map_or(pad, |&x| weigh(x));
            let y = b.get(i).map_or(pad, |&y| weigh(y));
            x.cmp(&y)
        })
        .find(|o| *o != Ordering::Equal)
        .unwrap_or(Ordering::Equal)
}

/// Compares text under the dynamic collation `dynamic`, weighed as
/// `weighing` says, in lower case if `ci`.
fn compare_dynamic(a: &str, b: &str, ci: bool, dynamic: &Dynamic, weighing: Weighing) -> Ordering {
    let bytes = |s: &str| {
        let mut bytes = cp1252::encode(s);
        if ci {
            for byte in &mut bytes {
                *byte = dynamic.lower[usize::from(*byte)];
            }
        }
        bytes
    };
    let (a, b) = (bytes(a), bytes(b));
    // The weights end in non-zero ones: as ever shorter ones padded with
    // zeros, as the keys are, they compare as they are.
    match weighing {
        Weighing::Compared => dynamic.compare(&a, &b),
        Weighing::Grouped => dynamic
            .grouping_weights(&a)
            .cmp(&dynamic.grouping_weights(&b)),
        Weighing::Indexed => dynamic.weights(&a).cmp(&dynamic.weights(&b)),
    }
}

/// What text that compares equal under `dynamic` (in lower case if `ci`)
/// is equal by, for a join's hash.
pub(crate) fn comparison_key(text: &str, ci: bool, dynamic: &Dynamic) -> Vec<u8> {
    let mut bytes = cp1252::encode(text);
    if ci {
        for byte in &mut bytes {
            *byte = dynamic.lower[usize::from(*byte)];
        }
    }
    dynamic.comparison_key(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_numbers() {
        assert_eq!(parse_exact("12.25", 2), Some(1225));
        assert_eq!(parse_exact("-0.5", 3), Some(-500));
        assert_eq!(parse_exact(".5", 1), Some(5));
        assert_eq!(parse_exact("1.999", 1), Some(19));
        assert_eq!(parse_exact("x", 0), None);
        assert_eq!(exact_text(1225, 2), "12.25");
        assert_eq!(exact_text(-5, 1), "-0.5");
        assert_eq!(exact_text(7, 0), "7");
    }

    #[test]
    fn text_compares_as_bytes() {
        assert_eq!(compare_text("a", "a  ", false, None), Ordering::Equal);
        assert_eq!(compare_text("B", "a", false, None), Ordering::Less);
        assert_eq!(compare_text("abc", "ABC", true, None), Ordering::Equal);
        // Regardless of case is in lower case: `[` sorts before `a`, `A` or not.
        assert_eq!(compare_text("[", "A", true, None), Ordering::Less);
        assert_eq!(compare_text("[", "A", false, None), Ordering::Greater);
    }
}
