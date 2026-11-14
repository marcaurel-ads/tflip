//! The functions of the expression engine that filters and indexes may use
//! (the help's "Supported in Navigational"), as the original engine
//! evaluates them.
//!
//! Left out: `CONTAINS` beyond checking for NULL, where the environment
//! has no full-text search to answer it ([`crate::Contains`]), `COLLATE` (no collations), `TRANSFORM` beyond numbers in
//! pictures of `9`s, and `TTOC`, `SYS` and `NEWIDSTRING`, which are refused
//! as unsupported once their arguments are counted. The functions only SQL
//! has (`MOD`, `SQRT`, `POWER`, `PI`, ...) are unknown here, as in the
//! original engine.

use tflip_storage::value::{Ymd, julian_to_ymd, ymd_to_julian};

use crate::check::{Kind, Node, computed_format};
use crate::eval::Val;
use crate::number;
use crate::text::{format_date, parse_date_with, parse_time, timestamp_split, timestamp_time};
use crate::{Env, ErrorKind, Format, Runtime, Type};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Func {
    // Strings
    Upper,
    Lower,
    Len,
    Length,
    Trim,
    LTrim,
    AllTrim,
    Substr,
    Left,
    Right,
    At,
    Rat,
    Chr,
    Space,
    Repeat,
    Reverse,
    PadL,
    PadR,
    PadC,
    Str,
    StrZero,
    Value,
    Soundex,
    Difference,
    Char2Hex,
    Hex2Char,
    Base64Encode,
    Base64Decode,
    I2Bin,
    L2Bin,
    Descend,
    // Numbers
    Abs,
    Ceiling,
    Floor,
    Round,
    Sign,
    Max,
    Min,
    // Dates and times
    CToD,
    DToC,
    DToS,
    SToD,
    Year,
    Month,
    Day,
    DayOfWeek,
    DayOfYear,
    DayName,
    MonthName,
    Quarter,
    Week,
    IsoWeek,
    Hour,
    Minute,
    Second,
    FracSecond,
    TsToD,
    SToTs,
    CToT,
    CToTs,
    CreateTimestamp,
    Today,
    Now,
    NowUtc,
    Time,
    // Others
    Iif,
    Empty,
    IsNull,
    Coalesce,
    InList,
    Deleted,
    RecNo,
    NewId,
    Contains,
    Transform,
    Collate,
    // Known, but not done: unsupported once their arguments are counted.
    TToC,
    Sys,
    NewIdString,
}

impl Func {
    /// The function called `name`, regardless of case.
    pub(crate) fn named(name: &str) -> Option<Func> {
        use Func::*;
        Some(match name.to_ascii_uppercase().as_str() {
            "UPPER" => Upper,
            "LOWER" => Lower,
            "LEN" => Len,
            "LENGTH" => Length,
            "TRIM" | "RTRIM" => Trim,
            "LTRIM" => LTrim,
            "ALLTRIM" => AllTrim,
            "SUBSTR" => Substr,
            "LEFT" => Left,
            "RIGHT" => Right,
            "AT" => At,
            "RAT" => Rat,
            "CHR" => Chr,
            "SPACE" => Space,
            "REPEAT" => Repeat,
            "REVERSE" => Reverse,
            "PADL" => PadL,
            "PADR" | "PAD" => PadR,
            "PADC" => PadC,
            "STR" => Str,
            "STRZERO" => StrZero,
            "VAL" => Value,
            "SOUNDEX" => Soundex,
            "DIFFERENCE" => Difference,
            "CHAR2HEX" => Char2Hex,
            "HEX2CHAR" => Hex2Char,
            "BASE64ENCODE" => Base64Encode,
            "BASE64DECODE" => Base64Decode,
            "I2BIN" => I2Bin,
            "L2BIN" => L2Bin,
            "DESCEND" => Descend,
            "ABS" => Abs,
            "CEILING" => Ceiling,
            "FLOOR" => Floor,
            "ROUND" => Round,
            "SIGN" => Sign,
            "MAX" => Max,
            "MIN" => Min,
            "CTOD" => CToD,
            "DTOC" => DToC,
            "DTOS" => DToS,
            "STOD" => SToD,
            "YEAR" => Year,
            "MONTH" => Month,
            "DAY" => Day,
            "DAYOFWEEK" => DayOfWeek,
            "DAYOFYEAR" => DayOfYear,
            "DAYNAME" => DayName,
            "MONTHNAME" => MonthName,
            "QUARTER" => Quarter,
            "WEEK" => Week,
            "ISOWEEK" => IsoWeek,
            "HOUR" => Hour,
            "MINUTE" => Minute,
            "SECOND" => Second,
            "FRAC_SECOND" => FracSecond,
            "TSTOD" => TsToD,
            "STOTS" => SToTs,
            "CTOT" => CToT,
            "CTOTS" => CToTs,
            "CREATETIMESTAMP" => CreateTimestamp,
            "DATE" | "TODAY" => Today,
            "NOW" => Now,
            "NOW_UTC" => NowUtc,
            "TIME" => Time,
            "IIF" | "IF" => Iif,
            "EMPTY" => Empty,
            "ISNULL" => IsNull,
            "COALESCE" => Coalesce,
            "INLIST" => InList,
            "DELETED" => Deleted,
            "RECNO" => RecNo,
            "NEWID" => NewId,
            "CONTAINS" => Contains,
            "TRANSFORM" => Transform,
            "COLLATE" => Collate,
            "TTOC" => TToC,
            "SYS" => Sys,
            "NEWIDSTRING" => NewIdString,
            _ => return None,
        })
    }

    /// The least and the most arguments the function takes.
    pub(crate) fn arity(self) -> (usize, usize) {
        use Func::*;
        match self {
            Today | Now | NowUtc | Time | Deleted | RecNo | NewId => (0, 0),
            Upper | Lower | Len | Length | Trim | LTrim | AllTrim | Chr | Space | Reverse
            | Value | Soundex | Char2Hex | Hex2Char | Base64Encode | Base64Decode | I2Bin
            | L2Bin | Descend | Abs | Ceiling | Floor | Sign | CToD | DToC | DToS | SToD | Year
            | Month | Day | DayOfWeek | DayOfYear | DayName | MonthName | Quarter | Week
            | IsoWeek | Hour | Minute | Second | FracSecond | TsToD | SToTs | CToT | CToTs
            | Empty | IsNull => (1, 1),
            Left | Right | At | Rat | Repeat | Difference | Round | Max | Min | Contains
            | Transform | Collate | TToC | Sys => (2, 2),
            NewIdString => (1, 1),
            Substr => (2, 3),
            PadL | PadR | PadC => (2, 3),
            Str | StrZero => (1, 3),
            Iif => (3, 3),
            Coalesce | InList => (2, usize::MAX),
            CreateTimestamp => (7, 7),
        }
    }
}

/// The value of a number written in the expression, if `node` is one.
fn constant(node: &Node) -> Option<f64> {
    match node.kind {
        Kind::Const(Val::Number(n)) => Some(n),
        _ => None,
    }
}

/// A width given by a constant argument, if it is one.
fn constant_width(node: &Node) -> Option<u16> {
    constant(node).map(|n| n.clamp(0.0, f64::from(u16::MAX)) as u16)
}

fn is_text(ty: Type) -> bool {
    matches!(ty, Type::String { .. } | Type::Null)
}

fn is_number(ty: Type) -> bool {
    matches!(ty, Type::Number(_) | Type::Null)
}

/// Whether values of the two types can stand for each other (the branches
/// of `IIF`, the arguments of `COALESCE`).
fn same_kind(a: Type, b: Type) -> bool {
    use std::mem::discriminant;
    a == Type::Null || b == Type::Null || discriminant(&a) == discriminant(&b)
}

/// The type of `func`'s result for arguments `args`.
pub(crate) fn check(func: Func, args: &[Node]) -> Result<Type, ErrorKind> {
    use Func::*;
    let ty = |i: usize| args[i].ty;
    let mismatch = Err(ErrorKind::TypeMismatch);
    let number = |format: Format| Ok(Type::Number(format));
    let string = |width: u16| Ok(Type::String { width, ci: false });
    let integer = number(computed_format(0));
    let text_width = |i: usize| match ty(i) {
        Type::String { width, .. } => width,
        _ => 0,
    };
    // Memo fields go only to the functions that take NULL checks.
    if !matches!(func, Empty | IsNull | Contains) && args.iter().any(|a| a.ty == Type::Memo) {
        return mismatch;
    }
    match func {
        Upper | Lower | Trim | LTrim | AllTrim | Reverse if is_text(ty(0)) => string(text_width(0)),
        Len | Length | Soundex | Value | Char2Hex | Base64Encode | Base64Decode
            if !is_text(ty(0)) =>
        {
            mismatch
        }
        Len | Length => integer,
        Soundex => string(4),
        Value => number(computed_format(0)),
        Char2Hex => Ok(Type::Binary {
            width: text_width(0).div_ceil(2),
        }),
        // The original's keys of it are one longer (`fixtures/write/nchar_expr_more`).
        Base64Encode => string(text_width(0).div_ceil(3) * 4 + 1),
        Base64Decode => Ok(Type::Binary {
            width: text_width(0) / 4 * 3,
        }),
        Hex2Char => match ty(0) {
            Type::Binary { width } => string(width * 2),
            Type::Null => string(0),
            _ => mismatch,
        },
        Substr => {
            if !is_text(ty(0)) || !args[1..].iter().all(|a| is_number(a.ty)) {
                return mismatch;
            }
            string(
                args.get(2)
                    .and_then(constant_width)
                    .unwrap_or(text_width(0)),
            )
        }
        Left | Right | Repeat => {
            if !is_text(ty(0)) || !is_number(ty(1)) {
                return mismatch;
            }
            let count = constant_width(&args[1]);
            match func {
                Repeat => string(text_width(0).saturating_mul(count.unwrap_or(1))),
                _ => string(count.unwrap_or(text_width(0))),
            }
        }
        At | Rat | Difference if is_text(ty(0)) && is_text(ty(1)) => integer,
        Chr | Space | I2Bin | L2Bin if !is_number(ty(0)) => mismatch,
        Chr => string(1),
        Space => string(constant_width(&args[0]).unwrap_or(0)),
        I2Bin => string(2),
        L2Bin => string(4),
        PadL | PadR | PadC => {
            if !is_text(ty(0)) || !is_number(ty(1)) || args.len() == 3 && !is_text(ty(2)) {
                return mismatch;
            }
            string(constant_width(&args[1]).unwrap_or(text_width(0)))
        }
        Str | StrZero => {
            let Type::Number(format) = ty(0) else {
                return if ty(0) == Type::Null {
                    string(10)
                } else {
                    mismatch
                };
            };
            if !args[1..].iter().all(|a| is_number(a.ty)) {
                return mismatch;
            }
            let length = args.get(1).and_then(constant).filter(|&n| n >= 1.0);
            string(length.map_or(format.length, |n| n.min(f64::from(u16::MAX)) as u16))
        }
        Descend => match ty(0) {
            Type::Date => integer,
            Type::Number(_) | Type::Logical | Type::String { .. } | Type::Null => Ok(ty(0)),
            _ => mismatch,
        },
        Abs => match ty(0) {
            Type::Number(format) => number(Format {
                fixed: format.fixed,
                ..computed_format(format.decimals)
            }),
            Type::Null => Ok(Type::Null),
            _ => mismatch,
        },
        Ceiling | Floor | Sign if is_number(ty(0)) => integer,
        Round if is_number(ty(0)) && is_number(ty(1)) => match ty(0) {
            Type::Number(format) => number(computed_format(format.decimals)),
            _ => Ok(Type::Null),
        },
        Max | Min => match (ty(0), ty(1)) {
            (Type::Number(a), Type::Number(b)) => {
                number(computed_format(a.decimals.max(b.decimals)))
            }
            (Type::Date, Type::Date) => Ok(Type::Date),
            _ => mismatch,
        },
        CToD | SToD if is_text(ty(0)) => Ok(Type::Date),
        CToT if is_text(ty(0)) => Ok(Type::Time),
        SToTs | CToTs if is_text(ty(0)) => Ok(Type::TimeStamp),
        DToC | DToS if matches!(ty(0), Type::Date | Type::Null) => {
            string(if func == DToS { 8 } else { 10 })
        }
        Year | Month | Day | DayOfWeek | DayOfYear | Quarter | Week | IsoWeek
            if matches!(ty(0), Type::Date | Type::TimeStamp | Type::Null) =>
        {
            integer
        }
        DayName | MonthName if matches!(ty(0), Type::Date | Type::TimeStamp | Type::Null) => {
            string(9)
        }
        Hour | Minute | Second | FracSecond
            if matches!(ty(0), Type::Time | Type::TimeStamp | Type::Null) =>
        {
            integer
        }
        TsToD if matches!(ty(0), Type::TimeStamp | Type::Null) => Ok(Type::Date),
        CreateTimestamp if args.iter().all(|a| is_number(a.ty)) => Ok(Type::TimeStamp),
        Today => Ok(Type::Date),
        Now | NowUtc => Ok(Type::TimeStamp),
        Time => string(8),
        Iif => {
            if !matches!(ty(0), Type::Logical | Type::Null) || !same_kind(ty(1), ty(2)) {
                return mismatch;
            }
            Ok(if ty(2) == Type::Null { ty(1) } else { ty(2) })
        }
        Coalesce => {
            if !args.iter().all(|a| same_kind(a.ty, ty(0))) {
                return mismatch;
            }
            Ok(args
                .iter()
                .map(|a| a.ty)
                .find(|&t| t != Type::Null)
                .unwrap_or(Type::Null))
        }
        InList => {
            // The value looked for has to have a type; a NULL among the
            // others is fine.
            if ty(0) == Type::Null {
                return mismatch;
            }
            if !args
                .iter()
                .all(|a| a.ty == Type::Null || same_kind(a.ty, ty(0)))
            {
                return mismatch;
            }
            Ok(Type::Logical)
        }
        Empty | IsNull | Deleted | Contains => Ok(Type::Logical),
        RecNo => integer,
        NewId => Ok(Type::Binary { width: 16 }),
        Transform if is_text(ty(1)) => match &args[1].kind {
            Kind::Const(Val::Str(picture)) => string(
                picture_options(picture.as_slice())
                    .map_or(picture.as_slice(), |(_, picture)| picture)
                    .len() as u16,
            ),
            _ => string(text_width(1)),
        },
        Collate => Err(ErrorKind::Collation),
        TToC | Sys => Err(ErrorKind::UnsupportedFunction),
        // `NEWIDSTRING(format)`: a GUID as text, the format a string naming the
        // layout. Its width is a fixed 38 (the widest form, with braces), as the
        // niladic SQL form also reports. A non-text format is a mismatch (3409).
        NewIdString if is_text(ty(0)) => string(38),
        NewIdString => mismatch,
        _ => mismatch,
    }
}

/// `UPPER` (`upper`) or `LOWER` of `bytes`: under a static collation each byte
/// goes through its case-fold table (see [`crate::CaseFold`]); otherwise ASCII
/// letters are folded and the rest left alone.
fn fold_case(bytes: &[u8], env: &Env, upper: bool) -> Vec<u8> {
    match &env.case_fold {
        Some(fold) => {
            let table = if upper { &fold.upper } else { &fold.lower };
            bytes.iter().map(|&b| table[usize::from(b)]).collect()
        }
        None if upper => bytes.to_ascii_uppercase(),
        None => bytes.to_ascii_lowercase(),
    }
}

/// `SUBSTR(s, start[, count])` of text in any unit (bytes, or the UTF-16 units
/// of Unicode text).
pub(crate) fn substr<T: Copy>(s: &[T], start: i64, count: Option<i64>) -> Vec<T> {
    // The start is taken as a 16-bit number: 2147483647 is -1.
    let start = start as i16 as i64;
    let start = match start {
        ..0 => (s.len() as i64 + start).max(0),
        0 => 0,
        _ => start - 1,
    } as usize;
    let count = count.map_or(usize::MAX, |count| count.max(0) as usize);
    let start = start.min(s.len());
    s[start..start.saturating_add(count).min(s.len())].to_vec()
}

/// `LEFT(s, count)`, in any unit.
pub(crate) fn left<T: Copy>(s: &[T], count: i64) -> Vec<T> {
    s[..(count.max(0) as usize).min(s.len())].to_vec()
}

/// `RIGHT(s, count)`, in any unit.
pub(crate) fn right<T: Copy>(s: &[T], count: i64) -> Vec<T> {
    s[s.len() - (count.max(0) as usize).min(s.len())..].to_vec()
}

/// `PADL`, `PADR` or `PADC` (`func`) of `s` to `length` with `fill`, in any
/// unit: text longer than that is cut.
pub(crate) fn pad<T: Copy>(func: Func, s: &[T], length: i64, fill: T) -> Vec<T> {
    let length = length.max(0) as usize;
    if s.len() >= length {
        return s[..length].to_vec();
    }
    let missing = length - s.len();
    let before = match func {
        Func::PadL => missing,
        Func::PadR => 0,
        _ => missing / 2,
    };
    let mut padded = vec![fill; before];
    padded.extend_from_slice(s);
    padded.resize(length, fill);
    padded
}

/// The value of `func` for `args`, none of them NULL; `ty` is the result's
/// type, `types` the arguments'.
pub(crate) fn apply(
    func: Func,
    args: &[Val],
    types: &[Type],
    ty: Type,
    env: &Env,
) -> Result<Val, Runtime> {
    use Func::*;
    let text = |i: usize| match &args[i] {
        Val::Str(bytes) => bytes.as_slice(),
        _ => &[],
    };
    let num = |i: usize| match args[i] {
        Val::Number(n) => n,
        _ => 0.0,
    };
    let int = |i: usize| num(i).trunc() as i64;
    let str = |bytes: Vec<u8>| Ok(Val::Str(bytes));
    let number = |n: f64| Ok(Val::Number(n));
    let date = |i: usize| match args[i] {
        Val::Date(julian) | Val::TimeStamp(julian, _) => julian,
        _ => 0,
    };
    let ms = |i: usize| match args[i] {
        Val::Time(ms) | Val::TimeStamp(_, ms) => ms,
        _ => 0,
    };
    let width = |ty: Type| ty.width();
    match func {
        Upper => str(fold_case(text(0), env, true)),
        Lower => str(fold_case(text(0), env, false)),
        Len => number(text(0).len() as f64),
        Length => number(trim_end(text(0)).len() as f64),
        Trim => str(trim_end(text(0)).to_vec()),
        LTrim => str(trim_start(text(0)).to_vec()),
        AllTrim => str(trim_start(trim_end(text(0))).to_vec()),
        Substr => str(substr(text(0), int(1), (args.len() > 2).then(|| int(2)))),
        Left => str(left(text(0), int(1))),
        Right => str(right(text(0), int(1))),
        At | Rat => {
            let (needle, hay) = (text(0), text(1));
            if needle.is_empty() || needle.len() > hay.len() {
                return number(0.0);
            }
            let mut found = hay
                .windows(needle.len())
                .enumerate()
                .filter(|(_, w)| *w == needle);
            let at = if func == At {
                found.next()
            } else {
                found.next_back()
            };
            number(at.map_or(0.0, |(i, _)| (i + 1) as f64))
        }
        Chr => str(vec![int(0) as u8]),
        Space => str(vec![b' '; int(0).max(0) as usize]),
        Repeat => str(text(0).repeat(int(1).max(0) as usize)),
        Reverse => str(text(0).iter().rev().copied().collect()),
        PadL | PadR | PadC => {
            let fill = if args.len() > 2 {
                text(2).first().copied().unwrap_or(b' ')
            } else {
                b' '
            };
            str(pad(func, text(0), int(1), fill))
        }
        Str | StrZero => {
            let Type::Number(format) = types[0] else {
                unreachable!("checked")
            };
            let (length, decimals) = match args.len() {
                1 => (format.length, format.decimals),
                _ if num(1) < 1.0 => (format.length, format.decimals),
                2 => (width(ty), 0),
                _ => (width(ty), num(2).clamp(0.0, 255.0) as u16),
            };
            let write = if func == Str {
                number::str
            } else {
                number::str_zero
            };
            str(write(num(0), length, decimals))
        }
        Value => number(number::val(&latin(text(0)))),
        Soundex => str(soundex(text(0))),
        // The characters the two codes have in common.
        Difference => {
            let (a, mut b) = (soundex(text(0)), soundex(text(1)));
            let common = a.iter().filter(|c| {
                b.iter()
                    .position(|d| d == *c)
                    .map(|i| b.remove(i))
                    .is_some()
            });
            number(common.count() as f64)
        }
        Char2Hex => {
            let mut nibbles = Vec::new();
            for &c in text(0) {
                match (c as char).to_digit(16) {
                    Some(n) => nibbles.push(n as u8),
                    None if c == b' ' => break,
                    None => return Err(Runtime::InvalidHex),
                }
            }
            if nibbles.len() % 2 == 1 {
                nibbles.push(0);
            }
            Ok(Val::Binary(
                nibbles.chunks(2).map(|p| p[0] << 4 | p[1]).collect(),
            ))
        }
        Hex2Char => match &args[0] {
            Val::Binary(bytes) => str(bytes
                .iter()
                .flat_map(|b| format!("{b:02X}").into_bytes())
                .collect()),
            _ => str(Vec::new()),
        },
        Base64Encode => str(base64_encode(text(0))),
        Base64Decode => Ok(Val::Binary(base64_decode(text(0)))),
        I2Bin => str((int(0) as i16).to_le_bytes().to_vec()),
        L2Bin => str((int(0) as i32).to_le_bytes().to_vec()),
        Descend => Ok(match &args[0] {
            Val::Number(n) => Val::Number(-n),
            Val::Date(julian) => Val::Number(f64::from(DESCEND_DATE - julian)),
            Val::Logical(b) => Val::Logical(!b),
            Val::Str(bytes) => Val::Str(bytes.iter().map(|b| b.wrapping_neg()).collect()),
            other => other.clone(),
        }),
        Abs => number(num(0).abs()),
        Ceiling => number(num(0).ceil()),
        Floor => number(num(0).floor()),
        // From 15 significant digits, so that 1.255 (1.25499999...) is 1.26;
        // a number too large to have decimals in them is left as it is.
        Round if num(0).abs() >= 1e15 => number(num(0)),
        Round => match int(1) {
            places @ 0.. => {
                let places = places.min(300) as u16;
                number(number::rounded(num(0), places, 15).parse().unwrap_or(0.0))
            }
            places => {
                let scale = 10f64.powi(places.max(-300) as i32);
                number(
                    number::rounded(num(0) * scale, 0, 15)
                        .parse::<f64>()
                        .unwrap_or(0.0)
                        / scale,
                )
            }
        },
        Sign => number(if num(0) > 0.0 {
            1.0
        } else if num(0) < 0.0 {
            -1.0
        } else {
            0.0
        }),
        Max | Min => {
            let greater = match (&args[0], &args[1]) {
                (Val::Number(a), Val::Number(b)) => a >= b,
                (Val::Date(a), Val::Date(b)) => a >= b,
                _ => true,
            };
            Ok(args[usize::from(greater == (func == Min))].clone())
        }
        CToD => Ok(Val::Date(
            parse_date_with(&latin(text(0)), &env.date_format, env.epoch)
                .ok()
                .flatten()
                .map_or(0, |(j, _)| j),
        )),
        SToD => Ok(Val::Date(stod(text(0)).unwrap_or(0))),
        DToC => str(if date(0) == 0 {
            empty_date(&env.date_format)
        } else {
            format_date(date(0), &env.date_format).into_bytes()
        }),
        DToS => str(tflip_storage::value::date_text(date(0)).to_vec()),
        Year => number(f64::from(julian_to_ymd(date(0)).year)),
        Month => number(f64::from(julian_to_ymd(date(0)).month)),
        Day => number(f64::from(julian_to_ymd(date(0)).day)),
        DayOfWeek => number(f64::from(weekday(date(0)) + 1)),
        DayOfYear => number(f64::from(day_of_year(date(0)))),
        DayName => str(padded_name(DAYS[weekday(date(0)) as usize])),
        MonthName => str(padded_name(
            MONTHS[julian_to_ymd(date(0)).month as usize - 1],
        )),
        Quarter => number(f64::from((julian_to_ymd(date(0)).month - 1) / 3 + 1)),
        Week => {
            let julian = date(0);
            let first = julian - day_of_year(julian) as i32 + 1;
            number(f64::from(
                (day_of_year(julian) - 1 + weekday(first)) / 7 + 1,
            ))
        }
        IsoWeek => number(f64::from(iso_week(date(0)))),
        Hour => number(f64::from(ms(0) / 3_600_000)),
        Minute => number(f64::from(ms(0) / 60_000 % 60)),
        Second => number(f64::from(ms(0) / 1000 % 60)),
        FracSecond => number(f64::from(ms(0) % 1000)),
        TsToD => Ok(Val::Date(date(0))),
        SToTs => Ok(stots(text(0))),
        CToT => Ok(Val::Time(parse_time(&latin(text(0))))),
        // Read as a TimeStamp value's text is, but the time is not checked
        // and is kept with no date (`fixtures/fields/tstexts.tsv`).
        CToTs => {
            let text = latin(text(0));
            let (end, at) = timestamp_split(&text);
            let date = parse_date_with(&text[..end], &env.date_format, env.epoch);
            let julian = date.ok().flatten().map_or(0, |(julian, _)| julian);
            Ok(Val::TimeStamp(julian, timestamp_time(&text[at..])))
        }
        CreateTimestamp => {
            let ymd = Ymd {
                year: int(0) as i32,
                month: int(1) as u32,
                day: int(2) as u32,
            };
            let julian = ymd_to_julian(ymd).unwrap_or(0);
            let ms = ((int(3) * 60 + int(4)) * 60 + int(5)) * 1000 + int(6);
            Ok(Val::TimeStamp(julian, ms as u32))
        }
        Today => Ok(Val::Date(env.now.0)),
        Now => Ok(Val::TimeStamp(env.now.0, env.now.1)),
        NowUtc => Ok(Val::TimeStamp(env.utc.0, env.utc.1)),
        Time => {
            let s = env.now.1 / 1000;
            str(format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60).into_bytes())
        }
        InList => Ok(Val::Logical(args[1..].iter().zip(&types[1..]).any(
            |(arg, &t)| {
                let eq = crate::eval::equal(&args[0], arg, types[0], t, env);
                eq == Some(true)
            },
        ))),
        NewId => Ok(Val::Binary(new_id().to_vec())),
        NewIdString => match guid_string(&new_id(), text(0)) {
            Some(s) => str(s),
            None => Err(Runtime::GuidFormat),
        },
        Contains => Ok(Val::Logical(true)),
        Transform => str(transform(&args[0], text(1))),
        Collate | TToC | Sys => unreachable!("refused when checked"),
        Iif | Empty | IsNull | Coalesce | Deleted | RecNo => {
            unreachable!("evaluated with their arguments")
        }
    }
}

/// The picture of `TRANSFORM` without its `@` options, and whether they ask
/// to left-justify (`B`); `None` for options other than `B` and `R`, which
/// tflip does not do.
fn picture_options(picture: &[u8]) -> Option<(bool, &[u8])> {
    let Some(rest) = picture.strip_prefix(b"@") else {
        return Some((false, picture));
    };
    let space = rest.iter().position(|&c| c == b' ')?;
    let options = &rest[..space];
    options
        .iter()
        .all(|&c| matches!(c, b'B' | b'R'))
        .then(|| (options.contains(&b'B'), &rest[space + 1..]))
}

/// `TRANSFORM(value, picture)` for a number and a picture of `9`s, a `.` for
/// decimals and the literals `%-/:`, right-aligned in the picture's width, or
/// asterisks if it does not fit. A picture with a `.` puts the literals
/// around a number of the digits' width; one without formats the number to
/// the picture's whole width and pushes the literals in, so that the last
/// digits fall off (`999%` of 100 is ` 10%`), as the original engine does
/// (`tools/ace/expr/iif.c`). The option `B` takes the leading blanks off.
/// Other values and pictures are returned as they are.
fn transform(value: &Val, picture: &[u8]) -> Vec<u8> {
    let literal = |c: u8| matches!(c, b'%' | b'-' | b'/' | b':');
    let Some((left, picture)) = picture_options(picture) else {
        return transform_plain(value, picture);
    };
    let Val::Number(n) = value else {
        return transform_plain(value, picture);
    };
    if !picture
        .iter()
        .all(|&c| matches!(c, b'9' | b'.') || literal(c))
    {
        return transform_plain(value, picture);
    }
    let digits: Vec<u8> = picture.iter().copied().filter(|&c| !literal(c)).collect();
    let mut text = match digits.iter().position(|&c| c == b'.') {
        Some(at) => {
            let mut text = number::str(*n, digits.len() as u16, (digits.len() - at - 1) as u16);
            // The picture's point stays among the asterisks.
            if text.first() == Some(&b'*') {
                text[at] = b'.';
            }
            text
        }
        None => number::str(*n, picture.len() as u16, 0),
    };
    for (at, &c) in picture.iter().enumerate().filter(|&(_, &c)| literal(c)) {
        text.insert(at, c);
    }
    text.truncate(picture.len());
    if left {
        text.drain(..text.iter().take_while(|&&c| c == b' ').count());
    }
    text
}

/// [`transform`] of what it does not know: a number as an integer, text as
/// it is.
fn transform_plain(value: &Val, picture: &[u8]) -> Vec<u8> {
    let digits = picture.iter().all(|&c| c == b'9' || c == b'.');
    match value {
        Val::Number(n) if digits => {
            let decimals = picture
                .iter()
                .position(|&c| c == b'.')
                .map_or(0, |at| picture.len() - at - 1);
            number::str(*n, picture.len() as u16, decimals as u16)
        }
        Val::Str(bytes) => bytes.clone(),
        Val::Number(n) => number::fixed(*n, 0).into_bytes(),
        _ => Vec::new(),
    }
}

/// `DESCEND` of a date: this day less the date.
const DESCEND_DATE: i32 = 5231808;

const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A day or month name, padded to the longest (9).
fn padded_name(name: &str) -> Vec<u8> {
    format!("{name:9}").into_bytes()
}

/// Text read as Latin-1 (each byte one character), for the date and number
/// readers, which only look at ASCII.
fn latin(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

fn trim_end(s: &[u8]) -> &[u8] {
    &s[..s.iter().rposition(|&c| c != b' ').map_or(0, |i| i + 1)]
}

fn trim_start(s: &[u8]) -> &[u8] {
    &s[s.iter().position(|&c| c != b' ').unwrap_or(s.len())..]
}

/// The day of the week, 0 for Sunday.
pub fn weekday(julian: i32) -> u32 {
    (julian + 1).rem_euclid(7) as u32
}

/// The day of the year, 1 for January 1.
pub fn day_of_year(julian: i32) -> u32 {
    let year = julian_to_ymd(julian).year;
    let first = ymd_to_julian(Ymd {
        year,
        month: 1,
        day: 1,
    })
    .unwrap_or(julian);
    (julian - first + 1) as u32
}

/// The ISO 8601 week number.
pub fn iso_week(julian: i32) -> u32 {
    // The week's Thursday decides its year.
    let monday_based = (weekday(julian) + 6) % 7;
    let thursday = julian - monday_based as i32 + 3;
    (day_of_year(thursday) - 1) / 7 + 1
}

/// `STOTS`: the date in the text up to its first space (after its first
/// character), and after it the
/// first four numbers, whatever separates them, as hours, minutes, seconds
/// and milliseconds (`.7` is 7 milliseconds). None is checked against its
/// range: each reads as a 32-bit number cut to its low 16 bits, and the sum
/// wraps. Text with no space has no time.
fn stots(text: &[u8]) -> Val {
    let (date, time) = match text.iter().skip(1).position(|&b| b == b' ') {
        Some(i) => (&text[..=i], &text[i + 2..]),
        None => (text, &[][..]),
    };
    let mut fields = [0u32; 4];
    let mut rest = time;
    for field in &mut fields {
        let skipped = rest.iter().take_while(|b| !b.is_ascii_digit()).count();
        rest = &rest[skipped..];
        let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        let number = rest[..digits].iter().fold(0u64, |n, &b| {
            (n * 10 + u64::from(b - b'0')).min(i32::MAX as u64)
        });
        *field = number as u32 & 0xFFFF;
        rest = &rest[digits..];
    }
    let [h, m, s, ms] = fields;
    let ms = h
        .wrapping_mul(3_600_000)
        .wrapping_add(m * 60_000)
        .wrapping_add(s * 1000)
        .wrapping_add(ms);
    Val::TimeStamp(stod(date).unwrap_or(0), ms)
}

/// A date written as `CCYYMMDD`; `None` if it is not one.
fn stod(text: &[u8]) -> Option<i32> {
    if text.len() < 8 || !text[..8].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let field = |range: std::ops::Range<usize>| latin(&text[range]).parse::<u32>().ok();
    let ymd = Ymd {
        year: field(0..4)? as i32,
        month: field(4..6)?,
        day: field(6..8)?,
    };
    ymd_to_julian(ymd)
}

/// The empty date in `format`: its separators, with spaces for the fields.
fn empty_date(format: &str) -> Vec<u8> {
    format
        .bytes()
        .map(|b| {
            if matches!(b.to_ascii_uppercase(), b'C' | b'Y' | b'M' | b'D') {
                b' '
            } else {
                b
            }
        })
        .collect()
}

/// The American Soundex code of the text's letters: the first letter, then
/// the digits of the following consonants, adjacent ones with the same digit
/// (also across `H` and `W`) once; `0000` without letters.
pub fn soundex(text: &[u8]) -> Vec<u8> {
    let code = |c: u8| match c.to_ascii_uppercase() {
        b'B' | b'F' | b'P' | b'V' => b'1',
        b'C' | b'G' | b'J' | b'K' | b'Q' | b'S' | b'X' | b'Z' => b'2',
        b'D' | b'T' => b'3',
        b'L' => b'4',
        b'M' | b'N' => b'5',
        b'R' => b'6',
        _ => 0,
    };
    let mut letters = text.iter().copied().filter(u8::is_ascii_alphabetic);
    let Some(first) = letters.next() else {
        return b"0000".to_vec();
    };
    let mut result = vec![first.to_ascii_uppercase()];
    let mut last = code(first);
    for c in letters {
        let digit = code(c);
        if digit != 0 && digit != last {
            result.push(digit);
            if result.len() == 4 {
                break;
            }
        }
        if !matches!(c.to_ascii_uppercase(), b'H' | b'W') {
            last = digit;
        }
    }
    result.resize(4, b'0');
    result
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The Base64 text of `bytes`, padded with `=`.
pub fn base64_encode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                BASE64[(n >> (18 - 6 * i) & 63) as usize]
            } else {
                b'='
            });
        }
    }
    out
}

/// The bytes of Base64 text; characters outside the alphabet are skipped.
pub fn base64_decode(text: &[u8]) -> Vec<u8> {
    let sextets: Vec<u32> = text
        .iter()
        .filter_map(|c| BASE64.iter().position(|b| b == c).map(|i| i as u32))
        .collect();
    let mut out = Vec::new();
    for chunk in sextets.chunks(4) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &s)| n | s << (18 - 6 * i));
        for i in 0..chunk.len().saturating_sub(1) {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    out
}

/// A GUID's 36-character delimited text, `01234567-89ab-cdef-0123-456789abcdef`,
/// lower case (the first three groups little-endian in the stored bytes, as the
/// original engine writes them).
fn guid_delimited(g: &[u8; 16]) -> String {
    let d1 = u32::from_le_bytes([g[0], g[1], g[2], g[3]]);
    let d2 = u16::from_le_bytes([g[4], g[5]]);
    let d3 = u16::from_le_bytes([g[6], g[7]]);
    let rest: String = g[8..].iter().map(|b| format!("{b:02x}")).collect();
    format!("{d1:08x}-{d2:04x}-{d3:04x}-{}-{}", &rest[..4], &rest[4..])
}

/// `NEWIDSTRING(format)`: the GUID `g` in the string `format` (case-insensitive),
/// as the original engine writes it; `None` for a format it does not have. Used
/// by the expression engine (4407 on `None`) and, via `tflip-sql`, by the SQL
/// scalar (2159, and `MIME` mapped to `M` first — the SQL repertoire). The
/// formats (probed on the VM against ADS 12.0):
///   D / DELIMITED    8-4-4-4-12 hex with hyphens, lower case (36)
///   B / BRACKETED    the delimited form in square brackets (38)
///   P / PARENTHESIS  the delimited form in parentheses (38)
///   N / NUMBERS      the 32 hex digits, no separators
///   M                MIME Base64 (`+/`, `=` padding), 24 bytes
///   F / FILE         URL- and file-safe Base64 (`-_`, no padding), 22 bytes
/// The documented `MIME` long name is *not* accepted by the engine (only `M`),
/// nor is `REGISTRY`; both give 4407 (see `docs/ADS_BUGS.md`).
pub fn guid_string(g: &[u8; 16], format: &[u8]) -> Option<Vec<u8>> {
    let delimited = guid_delimited(g);
    let text = match format.to_ascii_uppercase().as_slice() {
        b"D" | b"DELIMITED" => delimited,
        b"B" | b"BRACKETED" => format!("[{delimited}]"),
        b"P" | b"PARENTHESIS" => format!("({delimited})"),
        b"N" | b"NUMBERS" => delimited.replace('-', ""),
        b"M" => return Some(base64_encode(g)),
        b"F" | b"FILE" => {
            let mut b = base64_encode(g);
            b.retain(|&c| c != b'=');
            for c in &mut b {
                match *c {
                    b'+' => *c = b'-',
                    b'/' => *c = b'_',
                    _ => {}
                }
            }
            return Some(b);
        }
        _ => return None,
    };
    Some(text.into_bytes())
}

/// A new random GUID, as the original makes them: its text shows the version
/// 4 as the third digit of the third group (the high nibble of byte 6) and
/// the variant in the fourth (`fixtures/sql/guid.json`'s probe). Shared with
/// SQL's `NEWID` and `NEWIDSTRING`.
pub fn new_id() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        // Without the system's randomness, the clock and a counter still keep
        // the GUIDs of this process apart.
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        bytes[..8].copy_from_slice(&nanos.to_le_bytes());
        bytes[8..].copy_from_slice(&count.to_le_bytes());
    }
    bytes[6] = bytes[6] & 0x0F | 0x40;
    bytes[8] = bytes[8] & 0x3F | 0x80;
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_ids_are_random_with_the_originals_version_and_variant() {
        let (a, b) = (new_id(), new_id());
        assert_ne!(a, b);
        for id in [a, b] {
            let text = String::from_utf8(guid_string(&id, b"D").unwrap()).unwrap();
            let groups: Vec<&str> = text.split('-').collect();
            assert_eq!(groups[2].as_bytes()[2], b'4', "{text}");
            assert!(b"89ab".contains(&groups[3].as_bytes()[0]), "{text}");
        }
        // The counter of the fallback would leave these zero.
        assert!(a[10..].iter().chain(&b[10..]).any(|&byte| byte != 0));
    }

    #[test]
    fn soundex_codes() {
        assert_eq!(soundex(b"Robert"), b"R163");
        assert_eq!(soundex(b"Rupert"), b"R163");
        assert_eq!(soundex(b"Tymczak"), b"T522");
        assert_eq!(soundex(b"Pfister"), b"P236");
        assert_eq!(soundex(b""), b"0000");
        assert_eq!(soundex(b"Zz    "), b"Z000");
    }

    #[test]
    fn base64() {
        assert_eq!(base64_encode(b"abc"), b"YWJj");
        assert_eq!(base64_encode(b"ab"), b"YWI=");
        assert_eq!(base64_decode(b"YWI="), b"ab");
    }

    #[test]
    fn newidstring_formats() {
        // The reference example GUID: the first three groups are little-endian
        // in the bytes, the last eight bytes in order.
        let g = [
            0x67, 0x45, 0x23, 0x01, 0xab, 0x89, 0xef, 0xcd, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef,
        ];
        let s = |fmt: &[u8]| String::from_utf8(guid_string(&g, fmt).unwrap()).unwrap();
        let d = "01234567-89ab-cdef-0123-456789abcdef";
        assert_eq!(s(b"D"), d);
        assert_eq!(s(b"DELIMITED"), d);
        assert_eq!(s(b"delimited"), d); // case-insensitive
        assert_eq!(s(b"B"), format!("[{d}]"));
        assert_eq!(s(b"BRACKETED"), format!("[{d}]"));
        assert_eq!(s(b"P"), format!("({d})"));
        assert_eq!(s(b"PARENTHESIS"), format!("({d})"));
        assert_eq!(s(b"N"), "0123456789abcdef0123456789abcdef");
        assert_eq!(s(b"NUMBERS"), "0123456789abcdef0123456789abcdef");
        // M: MIME Base64, 24 bytes with `=` padding, round-tripping the GUID.
        let m = guid_string(&g, b"M").unwrap();
        assert_eq!(m.len(), 24);
        assert_eq!(base64_decode(&m), g);
        // F: URL-safe Base64, 22 bytes, no padding, `-_` for `+/`.
        let f = guid_string(&g, b"F").unwrap();
        assert_eq!(f.len(), 22);
        assert!(!f.contains(&b'=') && !f.contains(&b'+') && !f.contains(&b'/'));
        // The documented long name MIME, REGISTRY and anything else are refused.
        for bad in [&b"MIME"[..], b"REGISTRY", b"X", b"", b"NUMBER", b"FIL"] {
            assert_eq!(guid_string(&g, bad), None, "{:?}", bad);
        }
    }

    #[test]
    fn weeks() {
        let julian = |year, month, day| ymd_to_julian(Ymd { year, month, day }).unwrap();
        assert_eq!(iso_week(julian(2003, 12, 29)), 1);
        assert_eq!(iso_week(julian(2010, 1, 1)), 53);
        assert_eq!(iso_week(julian(9999, 12, 31)), 52);
        assert_eq!(weekday(julian(2003, 1, 5)), 0);
    }
}
