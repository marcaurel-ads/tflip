//! Scalar functions: their argument checks, result types and values.
//!
//! Result types follow the original engine: text functions give VarChar
//! (`UPPER` and `LOWER` keep their argument's type), with the length the
//! constant arguments fix; the date parts are SMALLINT but `YEAR`, which is
//! INTEGER; math functions over doubles give DOUBLE, and `ABS`, `ROUND`,
//! `TRUNCATE`, `CEILING` and `FLOOR` keep their argument's type.

use tflip_storage::cp1252;
use tflip_storage::field::FieldType;
use tflip_storage::value::{Ymd, julian_to_ymd, ymd_to_julian};

use crate::compile::{
    Kind, Node, PARAM_TEXT, clamp, iso_date, iso_time, iso_timestamp, mixed_collations, time_text,
    timestamp_text,
};
use crate::error::{Pos, SqlError};
use crate::eval::{Env, text_date};
use crate::types::{self, Ty};
use crate::value::{V, pow10};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Func {
    Upper,
    Lower,
    Left,
    Right,
    Substring,
    /// `OSUBSTRING(text, start[, length])`: `SUBSTRING` counting from 0.
    OSubstring,
    /// `OINDEXOF(text, find)`: the 0-based place of `find`, -1 if absent.
    OIndexOf,
    Ltrim,
    Rtrim,
    AllTrim,
    /// `LENGTH`: without trailing spaces.
    Length,
    /// `LEN`: with them.
    Len,
    /// `CHAR_LENGTH(x [USING unit])`: with them, in characters, octets or
    /// code points (`fixtures/sql/gram_charlen.json`).
    CharLength,
    OctetLength,
    BitLength,
    Locate,
    At,
    Rat,
    Repeat,
    Replace,
    Reverse,
    Space,
    Insert,
    Ascii,
    /// `ASCII` of OEM table text: the code of the character in ANSI.
    AsciiOem,
    Char,
    Soundex,
    Difference,
    Str,
    StrZero,
    Val,
    Char2Hex,
    Hex2Char,
    Concat,
    StartsWith,
    EndsWith,
    SubstringOf,
    Base64Encode,
    Base64Decode,
    Abs,
    Sign,
    Mod,
    Ceiling,
    Floor,
    Round,
    Truncate,
    Sqrt,
    Power,
    Exp,
    Log,
    Log10,
    Pi,
    /// `RAND([seed])`: the connection's next random number.
    Rand,
    Degrees,
    Radians,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Cot,
    Year,
    Month,
    Day,
    DayOfWeek,
    DayOfYear,
    Quarter,
    Week,
    IsoWeek,
    Hour,
    Minute,
    Second,
    FracSecond,
    DayName,
    MonthName,
    Dtoc,
    Dtos,
    Stod,
    Ctod,
    Ctot,
    Ctots,
    Stots,
    CreateTimestamp,
    TimestampAdd,
    TimestampDiff,
    Tstod,
    CurDate,
    CurTime,
    /// `TIME()`: the time as `HH:MM:SS` text.
    TimeText,
    Now,
    NowUtc,
    /// `DATABASE()`: the connection's database path.
    Database,
    /// `NEWID()`: a new GUID.
    NewId,
    /// `NEWIDSTRING()`: a new GUID as text.
    NewIdString,
    /// `TTOC(timestamp, format)`.
    Ttoc,
    /// `APPLICATIONID()`: the connection's application ID.
    ApplicationId,
    /// `LASTAUTOINC(CONNECTION)` and `LASTAUTOINC(STATEMENT)`: the last
    /// AutoInc value an `INSERT` generated on the connection or the
    /// statement.
    LastAutoIncConnection,
    LastAutoIncStatement,
    /// `LASTROWID()`: the `ROWID` of the last record the statement inserted.
    LastRowId,
    /// `LASTROWVERSION()`: the ROWVERSION of the statement's last write.
    LastRowVersion,
    /// `NEXTROWVERSION(table, count)`: reads a dictionary's row version.
    NextRowVersion,
    /// `ROWNUM()`: the number of the row in the result.
    RowNum,
    /// `EMPTY(column)` of an ADT table: whether the field is NULL.
    Empty,
    /// `EMPTY(column)` of a DBF table: whether the field holds its empty
    /// value.
    EmptyDbf,
    Iif,
    Coalesce,
    IfNull,
}

fn lookup(name: &str) -> Option<Func> {
    use Func::*;
    Some(match name {
        "UPPER" | "UCASE" => Upper,
        "LOWER" | "LCASE" => Lower,
        "LEFT" => Left,
        "RIGHT" => Right,
        "SUBSTRING" => Substring,
        "OSUBSTRING" => OSubstring,
        "OINDEXOF" => OIndexOf,
        "LTRIM" => Ltrim,
        "RTRIM" => Rtrim,
        "ALLTRIM" => AllTrim,
        "LENGTH" => Length,
        "LEN" => Len,
        "CHAR_LENGTH" | "CHARACTER_LENGTH" => CharLength,
        "OCTET_LENGTH" => OctetLength,
        "BIT_LENGTH" => BitLength,
        "LOCATE" => Locate,
        "AT" => At,
        "RAT" => Rat,
        "REPEAT" => Repeat,
        "REPLACE" => Replace,
        "REVERSE" => Reverse,
        "SPACE" => Space,
        "INSERT" => Insert,
        "ASCII" => Ascii,
        "CHAR" => Char,
        "SOUNDEX" => Soundex,
        "DIFFERENCE" => Difference,
        "STR" => Str,
        "STRZERO" => StrZero,
        "VAL" => Val,
        "CHAR2HEX" => Char2Hex,
        "HEX2CHAR" => Hex2Char,
        "CONCAT" => Concat,
        "STARTSWITH" => StartsWith,
        "ENDSWITH" => EndsWith,
        "SUBSTRINGOF" => SubstringOf,
        "BASE64ENCODE" => Base64Encode,
        "BASE64DECODE" => Base64Decode,
        "ABS" => Abs,
        "SIGN" => Sign,
        "MOD" => Mod,
        "CEILING" => Ceiling,
        "FLOOR" => Floor,
        "ROUND" => Round,
        "TRUNCATE" => Truncate,
        "SQRT" => Sqrt,
        "POWER" => Power,
        "EXP" => Exp,
        "LOG" => Log,
        "LOG10" => Log10,
        "PI" => Pi,
        "RAND" => Rand,
        "DEGREES" => Degrees,
        "RADIANS" => Radians,
        "SIN" => Sin,
        "COS" => Cos,
        "TAN" => Tan,
        "ASIN" => Asin,
        "ACOS" => Acos,
        "ATAN" => Atan,
        "ATAN2" => Atan2,
        "COT" => Cot,
        "YEAR" => Year,
        "MONTH" => Month,
        "DAY" | "DAYOFMONTH" => Day,
        "DAYOFWEEK" => DayOfWeek,
        "DAYOFYEAR" => DayOfYear,
        "QUARTER" => Quarter,
        "WEEK" => Week,
        "ISOWEEK" => IsoWeek,
        "HOUR" => Hour,
        "MINUTE" => Minute,
        "SECOND" => Second,
        "FRAC_SECOND" => FracSecond,
        "DAYNAME" => DayName,
        "MONTHNAME" => MonthName,
        "DTOC" => Dtoc,
        "DTOS" => Dtos,
        "STOD" => Stod,
        "CTOD" => Ctod,
        "CTOT" => Ctot,
        "CTOTS" => Ctots,
        "CREATETIMESTAMP" => CreateTimestamp,
        "TIMESTAMPADD" => TimestampAdd,
        "TIMESTAMPDIFF" => TimestampDiff,
        "TSTOD" => Tstod,
        "CURDATE" | "CURRENT_DATE" | "DATE" => CurDate,
        "CURTIME" | "CURRENT_TIME" => CurTime,
        "TIME" => TimeText,
        "NOW" | "CURRENT_TIMESTAMP" => Now,
        "CURRENT_TIMESTAMP_UTC" => NowUtc,
        "DATABASE" => Database,
        "NEWID" => NewId,
        "NEWIDSTRING" => NewIdString,
        "TTOC" => Ttoc,
        "APPLICATIONID" => ApplicationId,
        "LASTROWID" => LastRowId,
        "LASTROWVERSION" => LastRowVersion,
        "NEXTROWVERSION" => NextRowVersion,
        "ROWNUM" => RowNum,
        "EMPTY" => Empty,
        "STOTS" => Stots,
        "IIF" => Iif,
        "COALESCE" => Coalesce,
        "IFNULL" | "ISNULL" => IfNull,
        _ => return None,
    })
}

/// Arguments functions take as bare words (`SQL_TSI_DAY`), which are not
/// column names.
pub(crate) fn takes_word(name: &str, word: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "TIMESTAMPADD" | "TIMESTAMPDIFF"
    ) && word.to_ascii_uppercase().starts_with("SQL_TSI_")
}

/// Checks a call of the function `name` with `args`, giving parameters
/// among them a type; returns the function and its result type.
pub(crate) fn check(name: &str, args: &mut [Node], pos: Pos) -> Result<(Func, Ty), SqlError> {
    use Func::*;
    let upper = name.to_ascii_uppercase();
    let func = lookup(&upper).ok_or_else(|| SqlError::no_function(name, pos))?;
    let bad = || SqlError::bad_argument(&upper, pos);
    // Parameters take the type the function wants.
    let n = args.len();
    let text = |i: usize, args: &[Node]| args.get(i).is_some_and(|a| a.ty.is_text());
    let number = |i: usize, args: &[Node]| args.get(i).is_some_and(|a| a.ty.is_numeric());
    // The original reads text as the date or time the function wants
    // (`YEAR('2024-05-06')`); `apply` converts it.
    let date = |i: usize, args: &[Node]| {
        args.get(i)
            .is_some_and(|a| matches!(a.ty, Ty::Date | Ty::TimeStamp) || a.ty.is_text())
    };
    let time = |i: usize, args: &[Node]| {
        args.get(i)
            .is_some_and(|a| matches!(a.ty, Ty::Time | Ty::TimeStamp) || a.ty.is_text())
    };
    let untyped = |a: &Node| a.ty == Ty::Unknown && matches!(a.kind, Kind::Param(_));
    // COALESCE and IFNULL type no parameter, nor do the timestamp
    // functions the dates they take (`fixtures/sql/params.json`).
    if func == Coalesce
        && let Some(i) = args.iter().position(untyped)
    {
        return Err(SqlError::parameter_not_allowed(&upper, i + 1, pos));
    }
    if func == IfNull && args.iter().any(untyped) {
        return Err(SqlError::null_type());
    }
    // `IIF` types no parameter among its results either.
    if func == Iif && args.iter().skip(1).any(untyped) {
        return Err(SqlError::iif_parameter(pos));
    }
    for (i, arg) in args.iter_mut().enumerate() {
        if untyped(arg) {
            arg.ty = param_type(func, i).ok_or_else(SqlError::null_type)?;
        }
    }
    let length_of = |i: usize, args: &[Node]| args.get(i).map_or(0, |a: &Node| a.ty.length());
    let constant = |i: usize, args: &[Node]| args.get(i).and_then(|a| a.constant()?.as_i128());
    let fixed_length =
        |i: usize, args: &[Node]| constant(i, args).map(|c| clamp(c.max(0) as usize));
    let arity = |range: std::ops::RangeInclusive<usize>| {
        if range.contains(&n) {
            Ok(())
        } else {
            Err(bad())
        }
    };
    let ty = match func {
        Upper | Lower => {
            arity(1..=1)?;
            if !text(0, args) {
                return Err(bad());
            }
            args[0].ty
        }
        Left | Right => {
            arity(2..=2)?;
            if !text(0, args) || !number(1, args) {
                return Err(bad());
            }
            Ty::varchar(fixed_length(1, args).unwrap_or(length_of(0, args)).max(1))
        }
        Substring => {
            arity(3..=3)?;
            if !text(0, args) || !number(1, args) || !number(2, args) {
                return Err(bad());
            }
            Ty::varchar(fixed_length(2, args).unwrap_or(length_of(0, args)).max(1))
        }
        OSubstring => {
            arity(2..=3)?;
            if !text(0, args) || !number(1, args) || (n == 3 && !number(2, args)) {
                return Err(bad());
            }
            let rest = constant(1, args)
                .map(|start| length_of(0, args).saturating_sub(start.clamp(0, 65535) as u16));
            let length = if n == 3 { fixed_length(2, args) } else { rest };
            Ty::varchar(length.unwrap_or(length_of(0, args)).max(1))
        }
        OIndexOf => {
            arity(2..=2)?;
            if !text(0, args) || !text(1, args) {
                return Err(bad());
            }
            Ty::INTEGER
        }
        Ltrim | Rtrim | AllTrim | Reverse => {
            arity(1..=1)?;
            if !text(0, args) {
                return Err(bad());
            }
            Ty::varchar(length_of(0, args))
        }
        Length | Len | OctetLength | BitLength => {
            arity(1..=1)?;
            if !text(0, args) {
                return Err(bad());
            }
            Ty::INTEGER
        }
        CharLength => {
            arity(1..=2)?;
            if matches!(
                args[0].ty,
                Ty::Bin {
                    field: FieldType::Binary | FieldType::Image,
                    ..
                }
            ) {
                return Err(SqlError::bad_argument_why(
                    &upper,
                    " Argument cannot be LONGVARBINARY.",
                    pos,
                    "",
                ));
            }
            if !text(0, args) && args[0].ty != Ty::Unknown {
                return Err(bad());
            }
            Ty::INTEGER
        }
        Locate | At | Rat => {
            arity(2..=if func == Locate { 3 } else { 2 })?;
            if !text(0, args) || !text(1, args) || (n == 3 && !number(2, args)) {
                return Err(bad());
            }
            Ty::INTEGER
        }
        Repeat => {
            arity(2..=2)?;
            if !text(0, args) || !number(1, args) {
                return Err(bad());
            }
            let times = fixed_length(1, args).unwrap_or(1);
            Ty::varchar(length_of(0, args).saturating_mul(times).max(1))
        }
        Replace => {
            arity(3..=3)?;
            if !(0..3).all(|i| text(i, args)) {
                return Err(bad());
            }
            let (from, to) = (length_of(1, args).max(1), length_of(2, args));
            let length = length_of(0, args);
            Ty::varchar(if to > from {
                length / from * to + length % from
            } else {
                length
            })
        }
        Space => {
            arity(1..=1)?;
            if !number(0, args) {
                return Err(bad());
            }
            Ty::varchar(fixed_length(0, args).unwrap_or(1).max(1))
        }
        Insert => {
            arity(4..=4)?;
            if !text(0, args) || !number(1, args) || !number(2, args) || !text(3, args) {
                return Err(bad());
            }
            let removed = fixed_length(2, args).unwrap_or(0);
            Ty::varchar(
                (length_of(0, args).saturating_sub(removed))
                    .saturating_add(length_of(3, args))
                    .max(1),
            )
        }
        Ascii | AsciiOem => {
            arity(1..=1)?;
            if !text(0, args) {
                return Err(bad());
            }
            Ty::SHORT
        }
        Char => {
            arity(1..=1)?;
            if !number(0, args) {
                return Err(bad());
            }
            Ty::Str {
                field: FieldType::Character,
                length: 1,
            }
        }
        Soundex => {
            arity(1..=1)?;
            if !text(0, args) {
                return Err(bad());
            }
            Ty::varchar(4)
        }
        Difference => {
            arity(2..=2)?;
            if !text(0, args) || !text(1, args) {
                return Err(bad());
            }
            Ty::INTEGER
        }
        Str | StrZero => {
            arity(3..=3)?;
            if !(0..3).all(|i| number(i, args)) {
                return Err(bad());
            }
            Ty::varchar(fixed_length(1, args).unwrap_or(10).max(1))
        }
        Val => {
            arity(1..=1)?;
            if !text(0, args) {
                return Err(bad());
            }
            Ty::Double
        }
        Char2Hex => {
            arity(1..=1)?;
            if !text(0, args) {
                return Err(bad());
            }
            Ty::Bin {
                field: FieldType::VarBinary,
                length: length_of(0, args).div_ceil(2).max(1),
            }
        }
        Hex2Char => {
            arity(1..=1)?;
            if !matches!(args[0].ty, Ty::Bin { .. }) {
                return Err(bad());
            }
            Ty::varchar(length_of(0, args).saturating_mul(2))
        }
        Concat => {
            arity(2..=2)?;
            if !text(0, args) || !text(1, args) {
                return Err(bad());
            }
            if mixed_collations(&args[0], &args[1]) {
                return Err(SqlError::ambiguous_character());
            }
            Ty::varchar(length_of(0, args).saturating_add(length_of(1, args)))
        }
        StartsWith | EndsWith | SubstringOf => {
            arity(2..=2)?;
            if !text(0, args) || !text(1, args) {
                return Err(bad());
            }
            Ty::Bool
        }
        Base64Encode => {
            arity(1..=1)?;
            Ty::varchar(length_of(0, args).div_ceil(3).saturating_mul(4).max(1))
        }
        Base64Decode => {
            arity(1..=1)?;
            if !text(0, args) {
                return Err(bad());
            }
            Ty::Bin {
                field: FieldType::VarBinary,
                length: (length_of(0, args) / 4 * 3).max(1),
            }
        }
        Abs | Ceiling | Floor => {
            arity(1..=1)?;
            if !number(0, args) {
                return Err(bad());
            }
            args[0].ty
        }
        Round | Truncate => {
            // `TRUNCATE` has no one-argument form, as `ROUND` has.
            arity(if func == Truncate { 2..=2 } else { 1..=2 })?;
            if !number(0, args) || (n == 2 && !number(1, args)) {
                return Err(bad());
            }
            args[0].ty
        }
        Sign => {
            arity(1..=1)?;
            if !number(0, args) {
                return Err(bad());
            }
            Ty::SHORT
        }
        Mod => {
            arity(2..=2)?;
            match (args[0].ty, args[1].ty) {
                (Ty::Int { .. }, Ty::Int { .. }) => args[0].ty,
                _ => return Err(bad()),
            }
        }
        Sqrt | Exp | Log | Log10 | Degrees | Radians | Sin | Cos | Tan | Asin | Acos | Atan
        | Cot => {
            arity(1..=1)?;
            if !number(0, args) {
                return Err(bad());
            }
            Ty::Double
        }
        Power | Atan2 => {
            arity(2..=2)?;
            if !number(0, args) || !number(1, args) {
                return Err(bad());
            }
            Ty::Double
        }
        Pi => {
            arity(0..=0)?;
            Ty::Double
        }
        // A seed is a number (an integer, exact or not, money), or NULL.
        Rand => {
            arity(0..=1)?;
            if n == 1 && !number(0, args) && args[0].ty != Ty::Unknown {
                return Err(bad());
            }
            Ty::Double
        }
        Year | Month | Day | DayOfWeek | DayOfYear | Quarter | Week | IsoWeek | DayName
        | MonthName | Dtos | Tstod => {
            arity(1..=1)?;
            if !date(0, args) {
                return Err(bad());
            }
            match func {
                Year => Ty::INTEGER,
                DayName | MonthName => Ty::varchar(10),
                Dtos => Ty::varchar(8),
                Tstod => Ty::Date,
                _ => Ty::SHORT,
            }
        }
        Dtoc => {
            arity(1..=2)?;
            if !date(0, args) || (n == 2 && !matches!(args[1].ty, Ty::Int { .. } | Ty::Unknown)) {
                return Err(bad());
            }
            match constant(1, args) {
                Some(1) => Ty::varchar(8),
                _ => Ty::varchar(11),
            }
        }
        Hour | Minute | Second | FracSecond => {
            arity(1..=1)?;
            if !time(0, args) {
                return Err(bad());
            }
            Ty::SHORT
        }
        Stod | Ctod => {
            arity(1..=1)?;
            if !text(0, args) {
                return Err(bad());
            }
            Ty::Date
        }
        Ctot => {
            arity(1..=1)?;
            if !text(0, args) {
                return Err(bad());
            }
            Ty::Time
        }
        Ctots | Stots => {
            arity(1..=1)?;
            if !text(0, args) {
                return Err(bad());
            }
            Ty::TimeStamp
        }
        CreateTimestamp => {
            arity(7..=7)?;
            if !(0..7).all(|i| number(i, args)) {
                return Err(bad());
            }
            Ty::TimeStamp
        }
        TimestampAdd => {
            arity(3..=3)?;
            if interval(&args[0]).is_none() || !number(1, args) || !date(2, args) {
                return Err(bad());
            }
            Ty::TimeStamp
        }
        TimestampDiff => {
            arity(3..=3)?;
            if interval(&args[0]).is_none() || !date(1, args) || !date(2, args) {
                return Err(bad());
            }
            Ty::INTEGER
        }
        CurDate => {
            arity(0..=0)?;
            Ty::Date
        }
        CurTime => {
            arity(0..=1)?;
            Ty::Time
        }
        TimeText => {
            arity(0..=0)?;
            Ty::varchar(8)
        }
        Now | NowUtc => {
            // Only `CURRENT_TIMESTAMP` takes a precision (checked by the
            // parser), `NOW` takes none.
            arity(0..=usize::from(upper != "NOW"))?;
            Ty::TimeStamp
        }
        // `DATABASE()` is VARCHAR(255); `NEWIDSTRING()` is VARCHAR(38) (the
        // GUID text, braces included in the width); `NEWID()` is a GUID.
        Database => {
            arity(0..=0)?;
            Ty::varchar(255)
        }
        NewIdString => {
            arity(0..=0)?;
            Ty::varchar(38)
        }
        NewId => {
            arity(0..=0)?;
            Ty::Guid
        }
        // The format is an integer (a NULL too); a constant one is 1, 2 or 3,
        // and gives the length of the text (`fixtures/sql/rowfuncs.json`).
        Ttoc => {
            arity(2..=2)?;
            if !matches!(args[0].ty, Ty::TimeStamp | Ty::Unknown)
                || !matches!(args[1].ty, Ty::Int { .. } | Ty::Unknown)
            {
                return Err(bad());
            }
            match constant(1, args) {
                Some(1) => Ty::varchar(14),
                Some(2) => Ty::varchar(6),
                Some(3) => Ty::varchar(19),
                Some(_) => return Err(SqlError::bad_format(&upper, pos)),
                None if args[0].ty == Ty::Unknown => Ty::varchar(14),
                None => Ty::varchar(19),
            }
        }
        ApplicationId => {
            arity(0..=0)?;
            Ty::Str {
                field: FieldType::Memo,
                length: 0,
            }
        }
        LastAutoIncConnection | LastAutoIncStatement => Ty::INTEGER,
        LastRowId => {
            arity(0..=0)?;
            Ty::varchar(19)
        }
        NextRowVersion => {
            arity(2..=2)?;
            Ty::LONG
        }
        LastRowVersion => {
            arity(0..=0)?;
            Ty::LONG
        }
        RowNum => {
            arity(0..=0)?;
            Ty::INTEGER
        }
        Empty | EmptyDbf => {
            arity(1..=1)?;
            if args[0].ty == Ty::Unknown {
                return Err(SqlError::null_type());
            }
            Ty::Bool
        }
        Iif => {
            arity(3..=3)?;
            if args[0].ty != Ty::Bool {
                return Err(bad());
            }
            let ty = types::merge(args[1].ty, args[2].ty).ok_or_else(|| {
                match args[1].ty.is_temporal() && args[2].ty.is_temporal() {
                    true => SqlError::branch_types("IIF", None, pos),
                    false => bad(),
                }
            })?;
            if mixed_collations(&args[1], &args[2]) {
                return Err(SqlError::ambiguous_character());
            }
            if ty == Ty::Unknown {
                return Err(bad());
            }
            ty
        }
        Coalesce | IfNull => {
            if n < 2 || (func == IfNull && n != 2) {
                return Err(bad());
            }
            // `COALESCE` takes no bytes but a RAW's
            // (`fixtures/sql/gram_cast.json`).
            if func == Coalesce
                && let Some(i) = args.iter().position(|a| {
                    matches!(
                        a.ty,
                        Ty::Bin {
                            field: FieldType::VarBinary | FieldType::Binary | FieldType::Image,
                            ..
                        }
                    )
                })
            {
                return Err(SqlError::blob_not_allowed(&upper, i + 1, pos));
            }
            if func == IfNull && args[0].ty == Ty::Unknown {
                return Err(SqlError::null_type());
            }
            let mut ty = Ty::Unknown;
            for (i, arg) in args.iter().enumerate() {
                if args[..i].iter().any(|before| mixed_collations(before, arg)) {
                    return Err(SqlError::ambiguous_character());
                }
                ty = types::merge(ty, arg.ty).ok_or_else(|| {
                    match ty.is_temporal() && arg.ty.is_temporal() {
                        true => SqlError::branch_types("COALESCE", Some(i + 1), pos),
                        false => bad(),
                    }
                })?;
            }
            // `COALESCE` of an integer and a NUMERIC is as wide as the
            // wider of them, where the others add the integer part.
            if func == Coalesce
                && let Ty::Num { scale, .. } = ty
                && args.iter().any(|a| matches!(a.ty, Ty::Int { .. }))
            {
                let precision = args
                    .iter()
                    .filter_map(|a| a.ty.exact())
                    .map(|(p, _)| p)
                    .max();
                ty = Ty::Num {
                    precision: precision.unwrap_or(0),
                    scale,
                };
            }
            if ty == Ty::Unknown {
                return Err(
                    match args.iter().any(|a| matches!(a.kind, Kind::Param(_))) {
                        true => SqlError::null_type(),
                        false => SqlError::coalesce_null(pos),
                    },
                );
            }
            ty
        }
    };
    // Text made from case-insensitive text is case-insensitive itself: a
    // CICharacter of the same width, padded to it
    // (`fixtures/sql/groupconcat.json`).
    let ty = match ty {
        Ty::Str {
            field: FieldType::Character | FieldType::VarChar,
            length,
        } if !matches!(func, Iif | Coalesce | IfNull)
            && args.iter().any(|a| a.ty.case_insensitive()) =>
        {
            Ty::Str {
                field: FieldType::CiCharacter,
                length,
            }
        }
        ty => ty,
    };
    Ok((func, ty))
}

/// The type a parameter takes as argument `i` of `func`; `None` if the
/// function types none.
fn param_type(func: Func, i: usize) -> Option<Ty> {
    use Func::*;
    Some(match (func, i) {
        (TimestampAdd, 2) | (TimestampDiff, 1 | 2) => return None,
        (TimestampAdd, 1) => Ty::INTEGER,
        (Iif, 0) => Ty::Bool,
        (Left | Right | Repeat | Space | Char | Round | Truncate | Mod, 1)
        | (Substring | OSubstring | Insert, 1 | 2)
        | (Locate | At | Rat, 2)
        | (Space | Char, 0)
        | (CreateTimestamp, _) => Ty::INTEGER,
        (
            Abs | Ceiling | Floor | Round | Truncate | Sign | Sqrt | Exp | Log | Log10 | Degrees
            | Radians | Sin | Cos | Tan | Asin | Acos | Atan | Cot | Power | Atan2 | Str | StrZero
            | Rand,
            _,
        ) => Ty::Double,
        _ => Ty::varchar(PARAM_TEXT),
    })
}

/// The interval a `TIMESTAMPADD` or `TIMESTAMPDIFF` argument names.
fn interval(node: &Node) -> Option<&'static str> {
    let V::Str(word) = node.constant()? else {
        return None;
    };
    let upper = word.to_ascii_uppercase();
    let name = upper.strip_prefix("SQL_TSI_")?;
    [
        "FRAC_SECOND",
        "SECOND",
        "MINUTE",
        "HOUR",
        "DAY",
        "WEEK",
        "MONTH",
        "QUARTER",
        "YEAR",
    ]
    .into_iter()
    .find(|n| *n == name)
}

/// The value of `func` on `args`, which are evaluated; `ty` is the result
/// type. (`IIF`, `COALESCE` and `IFNULL` are evaluated lazily by the
/// caller.)
pub(crate) fn apply(
    func: Func,
    nodes: &[Node],
    args: &[V],
    ty: Ty,
    env: &Env,
    pos: Pos,
) -> Result<V, SqlError> {
    use Func::*;
    // A NULL argument gives NULL, but for the functions without arguments.
    if args.iter().any(V::is_null) && !matches!(func, Concat) {
        return Ok(V::Null);
    }
    // Text given where a date or time is wanted is read as one.
    let converted: Vec<V>;
    let args = if args.iter().any(|a| matches!(a, V::Str(_))) {
        let as_date = |s: &str| text_date(s, env).map(V::Date);
        let as_stamp = |s: &str| {
            timestamp_text(s, &env.date_format, env.epoch())
                .map(|(d, t)| V::TimeStamp(d, t))
                .or_else(|| as_date(s))
        };
        let read = |i: usize, parse: &dyn Fn(&str) -> Option<V>| match &args[i] {
            V::Str(s) => parse(s)
                .ok_or_else(|| SqlError::bad_argument(&format!("{func:?}").to_uppercase(), pos)),
            other => Ok(other.clone()),
        };
        let mut all = args.to_vec();
        match func {
            Year | Month | Day | DayOfWeek | DayOfYear | Quarter | Week | IsoWeek | DayName
            | MonthName | Dtos | Tstod => all[0] = read(0, &as_date)?,
            Hour | Minute | Second | FracSecond => {
                all[0] = read(0, &|s| time_text(s).map(V::Time).or_else(|| as_stamp(s)))?
            }
            TimestampDiff => {
                all[1] = read(1, &as_stamp)?;
                all[2] = read(2, &as_stamp)?;
            }
            TimestampAdd => all[2] = read(2, &as_stamp)?,
            _ => {}
        }
        converted = all;
        &converted[..]
    } else {
        args
    };
    let text = |i: usize| match &args[i] {
        V::Str(s) => s.clone(),
        V::Bin(b) => cp1252::decode(b),
        other => other.display(),
    };
    let int = |i: usize| args[i].as_i128().unwrap_or(0);
    let float = |i: usize| args[i].as_f64().unwrap_or(0.0);
    let chars = |s: &str| s.chars().collect::<Vec<char>>();
    let day = |i: usize| match args[i] {
        V::Date(d) | V::TimeStamp(d, _) => d,
        _ => 0,
    };
    let ms = |i: usize| match args[i] {
        V::Time(t) | V::TimeStamp(_, t) => t,
        _ => 0,
    };
    let double = |d: f64| Ok(V::Double(d));
    // An argument the function refuses, as a value: 2159 with what is wrong.
    let refuse = |why: &str| {
        let name = format!("{func:?}").to_uppercase();
        SqlError::bad_argument_why(&name, why, pos, "")
    };
    Ok(match func {
        Upper => fold_case(&args[0], env, true),
        Lower => fold_case(&args[0], env, false),
        Left | Right if int(1) <= 0 => {
            return Err(refuse(
                ". The second argument must be an positive integer withint range.",
            ));
        }
        Left => {
            let s = chars(&text(0));
            let n = int(1).clamp(0, s.len() as i128) as usize;
            V::Str(s[..n].iter().collect())
        }
        Right => {
            let s = chars(&text(0));
            let n = int(1).clamp(0, s.len() as i128) as usize;
            V::Str(s[s.len() - n..].iter().collect())
        }
        Substring if int(1) <= 0 => {
            return Err(refuse(
                ". The second parameter cannot be negative or out of range.",
            ));
        }
        Substring if int(2) <= 0 => {
            return Err(refuse(
                ". The third argument must be greater than 0 and within range.",
            ));
        }
        Substring => {
            let s = chars(&text(0));
            let start = (int(1).max(1) - 1) as usize;
            let length = int(2).max(0) as usize;
            V::Str(s.iter().skip(start).take(length).collect())
        }
        OSubstring if int(1) < 0 => {
            return Err(refuse(
                ". The second parameter cannot be negative or out of range.",
            ));
        }
        OSubstring => {
            let s = chars(&text(0));
            let length = if args.len() == 3 {
                int(2).max(0) as usize
            } else {
                usize::MAX
            };
            V::Str(s.iter().skip(int(1) as usize).take(length).collect())
        }
        OIndexOf => {
            let (haystack, needle) = (chars(&text(0)), chars(&text(1)));
            V::Int(find(&needle, &haystack, 0, 1).map_or(-1, |i| i as i64))
        }
        Ltrim => V::Str(text(0).trim_start_matches(' ').to_owned()),
        Rtrim => V::Str(text(0).trim_end_matches(' ').to_owned()),
        AllTrim => V::Str(text(0).trim_matches(' ').to_owned()),
        Length => V::Int(text(0).trim_end_matches(' ').chars().count() as i64),
        Len => V::Int(text(0).chars().count() as i64),
        CharLength => {
            let unit = match nodes.get(1).and_then(Node::constant) {
                Some(V::Str(unit)) => unit.to_ascii_uppercase(),
                _ => "CHARACTERS".to_owned(),
            };
            let s = text(0);
            let units = s.encode_utf16().count();
            let national = nodes[0].ty.is_national();
            V::Int(match (unit.as_str(), national) {
                ("OCTETS", true) => 2 * units,
                ("OCTETS", false) => cp1252::encode(&s).len(),
                // Unicode text counts its UTF-16 code units, which for a
                // character of two of them is one code point.
                ("CHARACTERS", true) => units,
                _ => s.chars().count(),
            } as i64)
        }
        OctetLength | BitLength => {
            let s = text(0);
            let octets = match nodes[0].ty.is_national() {
                true => 2 * s.encode_utf16().count(),
                false => cp1252::encode(&s).len(),
            };
            V::Int(match func {
                BitLength => 8 * octets,
                _ => octets,
            } as i64)
        }
        Locate | At => {
            let (needle, haystack) = (chars(&text(0)), chars(&text(1)));
            let from = if args.len() == 3 && func == Locate {
                (int(2).max(1) - 1) as usize
            } else {
                0
            };
            let occurrence = if args.len() == 3 && func == At {
                int(2).max(1) as usize
            } else {
                1
            };
            V::Int(find(&needle, &haystack, from, occurrence).map_or(0, |i| i as i64 + 1))
        }
        Rat => {
            let (needle, haystack) = (chars(&text(0)), chars(&text(1)));
            let found = (0..=haystack.len().saturating_sub(needle.len()))
                .rev()
                .find(|&i| !needle.is_empty() && haystack[i..].starts_with(&needle));
            V::Int(found.map_or(0, |i| i as i64 + 1))
        }
        Repeat if int(1) <= 0 => {
            return Err(refuse(
                ". The second argument must be greater than 0 and within range.",
            ));
        }
        Repeat => {
            let repeated = text(0).repeat(int(1).min(65_535) as usize);
            if repeated.chars().count() > 1024 {
                return Err(refuse(" Resulting string exceeds maximum allowed length."));
            }
            V::Str(repeated)
        }
        Replace => {
            let from = text(1);
            if from.is_empty() {
                return Err(refuse(". The second argument cannot be an empty string."));
            }
            let replaced = text(0).replace(&from, &text(2));
            // Text grown past 1024 characters is too long, though text that
            // was longer to begin with is not.
            if replaced.chars().count() > 1024 && replaced.chars().count() > text(0).chars().count()
            {
                return Err(refuse(" The length of the result is too long."));
            }
            V::Str(replaced)
        }
        Reverse => V::Str(text(0).chars().rev().collect()),
        Space if int(0) < 0 => {
            return Err(refuse(
                ". Value of the argument cannot be less than 0 or out of range.",
            ));
        }
        Space if int(0) > 1024 => {
            return Err(refuse(
                ". The length must be between 0 and 1024 characters.",
            ));
        }
        Space => V::Str(" ".repeat(int(0) as usize)),
        Insert => {
            let s = chars(&text(0));
            let start = ((int(1).max(1) - 1) as usize).min(s.len());
            let end = (start + int(2).max(0) as usize).min(s.len());
            let mut out: String = s[..start].iter().collect();
            out.push_str(&text(3));
            out.extend(&s[end..]);
            V::Str(out)
        }
        AsciiOem => V::Int(
            cp1252::encode(&crate::compile::oem_to_ansi_text(&text(0)))
                .first()
                .map_or(0, |&b| i64::from(b)),
        ),
        Ascii => V::Int(
            cp1252::encode(&text(0))
                .first()
                .map_or(0, |&b| i64::from(b)),
        ),
        // The code wraps modulo 256.
        Char => V::Str(cp1252::decode(&[int(0).rem_euclid(256) as u8])),
        Soundex => V::Str(cp1252::decode(&tflip_expr::soundex(&cp1252::encode(
            &text(0),
        )))),
        Difference => {
            let a = tflip_expr::soundex(&cp1252::encode(&text(0)));
            let b = tflip_expr::soundex(&cp1252::encode(&text(1)));
            V::Int(a.iter().zip(&b).filter(|(x, y)| x == y).count() as i64)
        }
        Str if !(1..=32).contains(&int(1)) => {
            return Err(refuse(". The length must be in the range from 1 to 32."));
        }
        Str if !(0..=int(1)).contains(&int(2)) => {
            return Err(refuse(". The decimals must range from 0 to length."));
        }
        Str => {
            let (length, decimals) = (int(1).clamp(0, 255) as u16, int(2).clamp(0, 255) as u16);
            V::Str(cp1252::decode(&tflip_expr::number::str(
                float(0),
                length,
                decimals,
            )))
        }
        StrZero => {
            let (length, decimals) = (int(1).clamp(0, 255) as u16, int(2).clamp(0, 255) as u16);
            V::Str(cp1252::decode(&tflip_expr::number::str_zero(
                float(0),
                length,
                decimals,
            )))
        }
        Val => V::Double(tflip_expr::number::val(&text(0))),
        Char2Hex => {
            let digits: Vec<u8> = cp1252::encode(&text(0));
            let mut bytes = Vec::new();
            for pair in digits.chunks(2) {
                let hex = std::str::from_utf8(pair)
                    .ok()
                    .and_then(|p| u8::from_str_radix(p, 16).ok());
                bytes.push(hex.ok_or_else(|| SqlError::bad_argument("CHAR2HEX", nodes[0].pos))?);
            }
            V::Bin(bytes)
        }
        Hex2Char => V::Str(
            bytes_of(&args[0])
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect(),
        ),
        Concat => {
            if args.iter().any(V::is_null) {
                return Ok(V::Null);
            }
            V::Str(text(0) + &text(1))
        }
        StartsWith => V::Bool(text(0).starts_with(&text(1))),
        EndsWith => V::Bool(text(0).ends_with(&text(1))),
        SubstringOf => V::Bool(text(1).contains(&text(0))),
        Base64Encode => V::Str(cp1252::decode(&tflip_expr::base64_encode(&bytes_of(
            &args[0],
        )))),
        Base64Decode => V::Bin(tflip_expr::base64_decode(&cp1252::encode(&text(0)))),
        Abs => match &args[0] {
            V::Int(n) => V::Int(n.abs()),
            V::Num(n, s) => V::Num(n.abs(), *s),
            V::Money(m) => V::Money(m.abs()),
            v => V::Double(v.as_f64().unwrap_or(0.0).abs()),
        },
        Sign => V::Int(match args[0].as_f64().unwrap_or(0.0) {
            x if x > 0.0 => 1,
            x if x < 0.0 => -1,
            _ => 0,
        }),
        Mod => {
            let divisor = int(1);
            if divisor == 0 {
                return Err(SqlError::division_by_zero());
            }
            V::Int((int(0) % divisor) as i64)
        }
        Ceiling | Floor => {
            let ceiling = func == Ceiling;
            match &args[0] {
                V::Int(n) => V::Int(*n),
                V::Num(n, s) => V::Num(
                    round_exact(*n, *s, 0, if ceiling { Mode::Up } else { Mode::Down }),
                    *s,
                ),
                V::Money(m) => V::Money(round_exact(
                    i128::from(*m),
                    4,
                    0,
                    if ceiling { Mode::Up } else { Mode::Down },
                ) as i64),
                v => {
                    let d = v.as_f64().unwrap_or(0.0);
                    V::Double(if ceiling { d.ceil() } else { d.floor() })
                }
            }
        }
        Round | Truncate => {
            let places = if args.len() == 2 { int(1) } else { 0 };
            let mode = if func == Round {
                Mode::HalfAway
            } else {
                Mode::Toward
            };
            match &args[0] {
                V::Int(n) if places >= 0 => V::Int(*n),
                V::Int(n) => V::Int(round_exact(i128::from(*n), 0, places, mode) as i64),
                V::Num(n, s) => V::Num(round_exact(*n, *s, places, mode), *s),
                V::Money(m) => V::Money(round_exact(i128::from(*m), 4, places, mode) as i64),
                v => {
                    let d = v.as_f64().unwrap_or(0.0);
                    let factor = 10f64.powi(places.clamp(-308, 308) as i32);
                    let scaled = d * factor;
                    let r = if mode == Mode::HalfAway {
                        scaled.round()
                    } else {
                        scaled.trunc()
                    };
                    V::Double(r / factor)
                }
            }
        }
        Sqrt => return double(float(0).sqrt()),
        Power => return double(float(0).powf(float(1))),
        Exp => return double(float(0).exp()),
        Log => return double(float(0).ln()),
        Log10 => return double(float(0).log10()),
        Pi => return double(std::f64::consts::PI),
        // A seed restarts the generator (a NULL one, above, leaves it alone).
        Rand => {
            let mut rand = env.rand.lock().unwrap_or_else(|e| e.into_inner());
            if !args.is_empty() {
                rand.seed(crate::random::seed_of(float(0)));
            }
            return double(rand.draw());
        }
        Degrees => return double(float(0).to_degrees()),
        Radians => return double(float(0).to_radians()),
        Sin => return double(float(0).sin()),
        Cos => return double(float(0).cos()),
        Tan => return double(float(0).tan()),
        Asin => return double(float(0).asin()),
        Acos => return double(float(0).acos()),
        Atan => return double(float(0).atan()),
        Atan2 => return double(float(0).atan2(float(1))),
        Cot => return double(1.0 / float(0).tan()),
        Year => V::Int(i64::from(julian_to_ymd(day(0)).year)),
        Month => V::Int(i64::from(julian_to_ymd(day(0)).month)),
        Day => V::Int(i64::from(julian_to_ymd(day(0)).day)),
        DayOfWeek => V::Int(i64::from(tflip_expr::weekday(day(0)) + 1)),
        DayOfYear => V::Int(i64::from(tflip_expr::day_of_year(day(0)))),
        Quarter => V::Int(i64::from((julian_to_ymd(day(0)).month - 1) / 3 + 1)),
        Week => {
            let d = day(0);
            let first = d - tflip_expr::day_of_year(d) as i32 + 1;
            let offset = tflip_expr::weekday(first);
            V::Int(i64::from((tflip_expr::day_of_year(d) + offset - 1) / 7 + 1))
        }
        IsoWeek => V::Int(i64::from(tflip_expr::iso_week(day(0)))),
        Hour => V::Int(i64::from(ms(0) / 3_600_000)),
        Minute => V::Int(i64::from(ms(0) / 60_000 % 60)),
        Second => V::Int(i64::from(ms(0) / 1000 % 60)),
        FracSecond => V::Int(i64::from(ms(0) % 1000)),
        DayName => V::Str(DAYS[tflip_expr::weekday(day(0)) as usize].to_owned()),
        MonthName => V::Str(MONTHS[julian_to_ymd(day(0)).month as usize - 1].to_owned()),
        Dtoc if args.len() == 2 && !(0..=1).contains(&int(1)) => {
            return Err(refuse(". The valid format values are 0 or 1."));
        }
        // `DTOC(d, 1)`: as `DTOS`.
        Dtoc if args.len() == 2 && int(1) == 1 => {
            let ymd = julian_to_ymd(day(0));
            V::Str(format!("{:04}{:02}{:02}", ymd.year, ymd.month, ymd.day))
        }
        Dtoc => V::Str(tflip_expr::text::format_date(day(0), &env.date_format)),
        Dtos => {
            let ymd = julian_to_ymd(day(0));
            V::Str(format!("{:04}{:02}{:02}", ymd.year, ymd.month, ymd.day))
        }
        Stod => {
            let s = text(0);
            let field = |r: std::ops::Range<usize>| s.get(r).and_then(|t| t.parse::<u32>().ok());
            match (field(0..4), field(4..6), field(6..8)) {
                (Some(year), Some(month), Some(day)) => ymd_to_julian(Ymd {
                    year: year as i32,
                    month,
                    day,
                })
                .map_or(V::Null, V::Date),
                _ => V::Null,
            }
        }
        Ctod => match tflip_expr::text::parse_date_with(&text(0), &env.date_format, env.epoch()) {
            Ok(Some((julian, _))) => V::Date(julian),
            _ => iso_date(&text(0)).map_or(V::Null, V::Date),
        },
        Ctot => iso_time(&text(0)).map_or(V::Null, V::Time),
        Ctots => iso_timestamp(&text(0)).map_or(V::Null, |(d, t)| V::TimeStamp(d, t)),
        // `CCYYMMDD HH:MM:SS.fff`.
        Stots => {
            let s = text(0);
            let field = |r: std::ops::Range<usize>| s.get(r).and_then(|t| t.parse::<u32>().ok());
            let date = match (field(0..4), field(4..6), field(6..8)) {
                (Some(year), Some(month), Some(day)) => ymd_to_julian(Ymd {
                    year: year as i32,
                    month,
                    day,
                }),
                _ => None,
            };
            let time = s
                .get(8..)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map_or(Some(0), iso_time);
            match (date, time) {
                (Some(d), Some(t)) => V::TimeStamp(d, t),
                _ => V::Null,
            }
        }
        CreateTimestamp => {
            let date = ymd_to_julian(Ymd {
                year: int(0) as i32,
                month: int(1) as u32,
                day: int(2) as u32,
            });
            let (h, m, s, f) = (int(3), int(4), int(5), int(6));
            match date {
                // A day the calendar has not is a conversion that failed.
                None => return Err(SqlError::bad_literal()),
                Some(d) if (0..24).contains(&h) && (0..60).contains(&m) && (0..60).contains(&s) => {
                    V::TimeStamp(d, (((h * 60 + m) * 60 + s) * 1000 + f.clamp(0, 999)) as u32)
                }
                _ => return Err(SqlError::bad_argument("CREATETIMESTAMP", nodes[0].pos)),
            }
        }
        TimestampAdd => {
            let unit = interval(&nodes[0]).unwrap_or("DAY");
            let (d, t) = timestamp_of(&args[2]);
            add_interval(d, t, unit, int(1))
        }
        TimestampDiff => {
            let unit = interval(&nodes[0]).unwrap_or("DAY");
            let (d1, t1) = timestamp_of(&args[1]);
            let (d2, t2) = timestamp_of(&args[2]);
            // The result is an INTEGER.
            let n = i32::try_from(diff_interval((d1, t1), (d2, t2), unit))
                .map_err(|_| SqlError::out_of_range())?;
            V::Int(i64::from(n))
        }
        Tstod => V::Date(day(0)),
        CurDate => V::Date(env.now.0),
        CurTime => V::Time(env.now.1),
        TimeText => {
            let t = env.now.1 / 1000;
            V::Str(format!("{:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60))
        }
        // `CURRENT_TIMESTAMP(n)` keeps `n` digits of the second
        // (`tools/ace/sql/literals.txt`).
        Now => {
            let ms = match args.first() {
                Some(_) => {
                    let cut = 10u32.pow(3 - int(0).clamp(0, 3) as u32);
                    env.now.1 - env.now.1 % cut
                }
                None => env.now.1,
            };
            V::TimeStamp(env.now.0, ms)
        }
        NowUtc => V::TimeStamp(env.utc_now.0, env.utc_now.1),
        Database => V::Str(env.database.clone()),
        NewId => V::Guid(tflip_expr::new_id()),
        // The format (a letter) was validated at compile time, so it formats.
        NewIdString => {
            let bytes = tflip_expr::guid_string(&tflip_expr::new_id(), text(0).as_bytes())
                .expect("NEWIDSTRING format checked at compile time");
            V::Str(String::from_utf8(bytes).expect("GUID text is ASCII"))
        }
        Ttoc => {
            let (d, t) = timestamp_of(&args[0]);
            let ymd = julian_to_ymd(d);
            let (h, m, s) = (t / 3_600_000, t / 60_000 % 60, t / 1000 % 60);
            match int(1) {
                1 => V::Str(format!(
                    "{:04}{:02}{:02}{h:02}{m:02}{s:02}",
                    ymd.year, ymd.month, ymd.day
                )),
                2 => V::Str(format!("{h:02}{m:02}{s:02}")),
                3 => V::Str(format!(
                    "{:04}-{:02}-{:02}T{h:02}:{m:02}:{s:02}",
                    ymd.year, ymd.month, ymd.day
                )),
                _ => V::Null,
            }
        }
        ApplicationId => V::Str(env.application_id.clone()),
        LastAutoIncConnection => {
            lock(&env.connection_autoinc).map_or(V::Null, |n| V::Int(i64::from(n)))
        }
        LastAutoIncStatement => lock(&env.statement_last)
            .autoinc
            .map_or(V::Null, |n| V::Int(i64::from(n))),
        // Before an insert the text is all NUL characters.
        LastRowId => V::Str(
            lock(&env.statement_last)
                .rowid
                .clone()
                .unwrap_or_else(|| "\0".repeat(18)),
        ),
        NextRowVersion => return Err(SqlError::no_dictionary()),
        LastRowVersion => lock(&env.statement_last).rowversion.map_or(V::Null, V::Int),
        RowNum | Empty | EmptyDbf | Iif | Coalesce | IfNull => unreachable!("evaluated by eval"),
    })
    .map(|v| match (v, ty) {
        // Integer results of exact-number types (`ROUND` keeps NUMERIC).
        (V::Int(n), Ty::Num { scale, .. }) => V::Num(i128::from(n) * pow10(scale), scale),
        (v, _) => v,
    })
}

fn lock<T>(cell: &std::sync::Mutex<T>) -> T
where
    T: Clone,
{
    cell.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

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

/// `f` applied to text, keeping a value's kind.
/// `UPPER` (`upper`) or `LOWER` of a text value. Under a static collation each
/// Windows-1252 byte goes through the collation's case table (its display
/// character; see [`crate::CaseFold`]); otherwise ASCII letters are folded and
/// the rest left alone.
fn fold_case(v: &V, env: &Env, upper: bool) -> V {
    let V::Str(s) = v else { return v.clone() };
    match &env.case_fold {
        Some(fold) => {
            let table = if upper { &fold.upper } else { &fold.lower };
            let folded: Vec<u8> = cp1252::encode(s)
                .iter()
                .map(|&b| table[usize::from(b)])
                .collect();
            V::Str(cp1252::decode(&folded))
        }
        None if upper => V::Str(s.to_ascii_uppercase()),
        None => V::Str(s.to_ascii_lowercase()),
    }
}

fn bytes_of(v: &V) -> Vec<u8> {
    match v {
        V::Bin(b) => b.clone(),
        V::Str(s) => cp1252::encode(s),
        other => cp1252::encode(&other.display()),
    }
}

/// The index of the `occurrence`th `needle` in `haystack` at or after
/// `from`.
fn find(needle: &[char], haystack: &[char], from: usize, occurrence: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    (from..haystack.len())
        .filter(|&i| haystack[i..].starts_with(needle))
        .nth(occurrence - 1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Up,
    Down,
    HalfAway,
    Toward,
}

/// `n` × 10^-scale rounded to `places` decimals, still × 10^-scale.
fn round_exact(n: i128, scale: u16, places: i128, mode: Mode) -> i128 {
    let drop = i128::from(scale) - places;
    if drop <= 0 {
        return n;
    }
    let unit = pow10(drop.min(38) as u16);
    let (q, r) = (n / unit, n % unit);
    let adjusted = match mode {
        Mode::Toward => q,
        Mode::Down if r < 0 => q - 1,
        Mode::Up if r > 0 => q + 1,
        Mode::Up | Mode::Down => q,
        Mode::HalfAway if r.abs() * 2 >= unit => q + r.signum(),
        Mode::HalfAway => q,
    };
    adjusted * unit
}

fn timestamp_of(v: &V) -> (i32, u32) {
    match v {
        V::TimeStamp(d, t) => (*d, *t),
        V::Date(d) => (*d, 0),
        _ => (0, 0),
    }
}

const DAY_MS: i64 = 86_400_000;

fn add_interval(day: i32, ms: u32, unit: &str, n: i128) -> V {
    let n = n as i64;
    let total = |delta_ms: i64| {
        let t = i64::from(day) * DAY_MS + i64::from(ms) + delta_ms;
        V::TimeStamp(t.div_euclid(DAY_MS) as i32, t.rem_euclid(DAY_MS) as u32)
    };
    match unit {
        "FRAC_SECOND" => total(n),
        "SECOND" => total(n * 1000),
        "MINUTE" => total(n * 60_000),
        "HOUR" => total(n * 3_600_000),
        "DAY" => total(n * DAY_MS),
        "WEEK" => total(n * 7 * DAY_MS),
        months_unit => {
            let months = n * match months_unit {
                "QUARTER" => 3,
                "YEAR" => 12,
                _ => 1,
            };
            let ymd = julian_to_ymd(day);
            let index = i64::from(ymd.year) * 12 + i64::from(ymd.month - 1) + months;
            let (year, month) = (index.div_euclid(12) as i32, index.rem_euclid(12) as u32 + 1);
            // Years carry a day the new year has not (29 February) into the
            // next month; months stop at their last day.
            if months_unit == "YEAR"
                && let Some(first) = ymd_to_julian(Ymd {
                    year,
                    month,
                    day: 1,
                })
            {
                return V::TimeStamp(first + ymd.day as i32 - 1, ms);
            }
            let mut d = ymd.day;
            loop {
                if let Some(julian) = ymd_to_julian(Ymd {
                    year,
                    month,
                    day: d,
                }) {
                    return V::TimeStamp(julian, ms);
                }
                d -= 1;
            }
        }
    }
}

fn diff_interval(a: (i32, u32), b: (i32, u32), unit: &str) -> i128 {
    let at = |(d, t): (i32, u32)| i128::from(d) * i128::from(DAY_MS) + i128::from(t);
    let delta = at(b) - at(a);
    match unit {
        "FRAC_SECOND" => delta,
        "SECOND" => delta / 1000,
        "MINUTE" => delta / 60_000,
        "HOUR" => delta / 3_600_000,
        "DAY" => delta / i128::from(DAY_MS),
        "WEEK" => delta / i128::from(7 * DAY_MS),
        months => {
            let (ya, yb) = (julian_to_ymd(a.0), julian_to_ymd(b.0));
            let m = (i128::from(yb.year) - i128::from(ya.year)) * 12 + i128::from(yb.month)
                - i128::from(ya.month);
            match months {
                "QUARTER" => m / 3,
                "YEAR" => i128::from(yb.year - ya.year),
                _ => m,
            }
        }
    }
}
