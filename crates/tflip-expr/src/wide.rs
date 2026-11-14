//! Unicode text in expressions: what an order's key expression gets when it
//! reads an NChar or NVarChar field.
//!
//! The rest of the engine takes text as Windows-1252 bytes, which would lose
//! what a Unicode field holds. The original engine's key expressions keep it:
//! `UPPER`, `LOWER`, the trims, `SUBSTR`, `LEFT`, `RIGHT`, the pads, `+`,
//! `IIF` and `COALESCE` of Unicode text give Unicode text, and the key of such
//! a value is an ICU sort key (see [`tflip_storage::icu`]). 8-bit text in one
//! of them is widened (`fixtures/write/nchar_expr`). What this module does not
//! know it refuses ([`Unicode::Unsupported`]), and the engine does not build
//! such keys.

use tflip_storage::cp1252;
use tflip_storage::field::FieldType;
use tflip_storage::icu;
use tflip_storage::value::Value;

use crate::check::{Kind, Node};
use crate::eval::{self, Val};
use crate::func::{self, Func};
use crate::lex::Op;
use crate::{Env, Failure, Row, Type};

/// How an expression takes Unicode text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unicode {
    /// It reads no Unicode field.
    No,
    /// Its value is Unicode text of up to `width` characters (`width` is the
    /// original's, which differs from the type's where a constant count
    /// asks for more than there is, and is below 0 for `LEFT(nc, -1)`).
    /// `descending` is `DESCEND` of it: the complement of the sort key
    /// (`fixtures/write/nchar_expr_more`).
    Text { width: i32, descending: bool },
    /// Its value is 8-bit text of `width` bytes made of Unicode text
    /// (`TRANSFORM`, `SOUNDEX`, `BASE64ENCODE`), where each character the
    /// Windows-1252 code page lacks is a 0x1A.
    Narrow { width: i32 },
    /// Its value is a number counted from Unicode text (`LEN`, `AT`, `VAL`).
    Number,
    /// It does something with Unicode text this module does not know.
    Unsupported,
}

/// A value of an expression that takes Unicode text.
#[derive(Debug, Clone, PartialEq)]
pub enum UVal {
    Null,
    Text(String),
    /// Any other value, a number or the like.
    Plain(Val),
}

#[derive(Clone, Copy, PartialEq)]
enum Class {
    /// Nothing in it is Unicode.
    Plain,
    /// Unicode text of that many characters.
    Wide(i32),
    /// A number, computed from Unicode text (`LEN(nc)`, and what is made of
    /// it).
    Derived,
    /// 8-bit text of that many bytes, computed from Unicode text.
    Narrow(i32),
    /// `DESCEND` of Unicode text of that many characters, which nothing can
    /// be made of (the original refuses `DESCEND(nc) + nv`).
    Descended(i32),
}

fn constant(node: &Node) -> Option<f64> {
    match node.kind {
        Kind::Const(Val::Number(n)) => Some(n),
        _ => None,
    }
}

fn plain_width(node: &Node) -> i32 {
    i32::from(node.ty.width())
}

/// The class of `node`, `None` for one this module does not know.
fn classify(node: &Node) -> Option<Class> {
    use Class::*;
    let text = |c: Class, node: &Node| match c {
        Plain => Some(plain_width(node)),
        Wide(w) | Narrow(w) => Some(w),
        Derived | Descended(_) => None,
    };
    match &node.kind {
        Kind::Const(_) => Some(Plain),
        Kind::Field(field) => Some(match field.field_type {
            FieldType::NChar | FieldType::NVarChar => Wide(i32::from(field.length)),
            _ => Plain,
        }),
        Kind::Binary(op, left, right) => {
            let (l, r) = (classify(left)?, classify(right)?);
            if (l, r) == (Plain, Plain) {
                return Some(Plain);
            }
            let arithmetic = matches!(
                op,
                Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod | Op::Pow
            );
            match node.ty {
                // `nc + NULL` is typed as NULL, and is.
                Type::String { .. } | Type::Null if *op == Op::Add => {
                    let width = text(l, left)? + text(r, right)?;
                    match (l, r) {
                        (Wide(_), _) | (_, Wide(_)) => Some(Wide(width)),
                        _ => Some(Narrow(width)),
                    }
                }
                Type::Number(_) if arithmetic && !matches!((l, r), (Wide(_), _) | (_, Wide(_))) => {
                    Some(Derived)
                }
                _ => None,
            }
        }
        Kind::And(left, right) | Kind::Join(left, right) => {
            (classify(left)? == Plain && classify(right)? == Plain).then_some(Plain)
        }
        Kind::Or(left, right) => {
            (classify(left)? == Plain && classify(right)? == Plain).then_some(Plain)
        }
        Kind::Not(operand) => (classify(operand)? == Plain).then_some(Plain),
        Kind::Call(func, args) => {
            let classes = args.iter().map(classify).collect::<Option<Vec<_>>>()?;
            if classes.iter().all(|&c| c == Plain) {
                return Some(Plain);
            }
            // What is made of values that are not Unicode text, though some
            // of them come from it.
            let takes_null = matches!(
                func,
                Func::IsNull
                    | Func::Empty
                    | Func::Iif
                    | Func::Coalesce
                    | Func::InList
                    | Func::RecNo
                    | Func::Deleted
                    | Func::Contains
            );
            if classes.iter().any(|c| matches!(c, Descended(_))) {
                return None;
            }
            if !takes_null && !classes.iter().any(|c| matches!(c, Wide(_))) {
                return Some(match node.ty {
                    Type::String { .. } => Narrow(plain_width(node)),
                    _ => Derived,
                });
            }
            let first = match classes[0] {
                Wide(w) => w,
                _ => 0,
            };
            let rest_plain = |from: usize| classes[from..].iter().all(|c| !matches!(c, Wide(_)));
            // A constant count: no more than the text has (or fewer than 0).
            let counted = |arg: Option<&Node>| match arg.and_then(constant) {
                Some(n) => (n.trunc().clamp(-1e6, 1e6) as i32).min(first),
                None => first,
            };
            use Func::*;
            match func {
                Upper | Lower | Trim | LTrim | AllTrim | Reverse
                    if matches!(classes[0], Wide(_)) =>
                {
                    Some(Wide(first))
                }
                Substr if matches!(classes[0], Wide(_)) && rest_plain(1) => {
                    Some(Wide(counted(args.get(2))))
                }
                Left | Right if matches!(classes[0], Wide(_)) && rest_plain(1) => {
                    Some(Wide(counted(args.get(1))))
                }
                PadL | PadR | PadC
                    if matches!(classes[0], Wide(_))
                        && !matches!(classes[1], Wide(_))
                        && classes.get(2).is_none_or(|&c| text(c, &args[2]).is_some()) =>
                {
                    match constant(&args[1]).map(f64::trunc) {
                        Some(n) if n > 0.0 => Some(Wide(n.min(1e6) as i32)),
                        Some(n) if n < 0.0 => None,
                        _ => Some(Wide(first)),
                    }
                }
                Iif if classes[0] == Plain && matches!(classes[1], Wide(_)) => Some(Wide(
                    text(classes[1], &args[1])?.max(text(classes[2], &args[2])?),
                )),
                // An 8-bit value among them is widened, whichever place it
                // has (`fixtures/write/nchar_expr_more`).
                Coalesce if classes.iter().any(|c| matches!(c, Wide(_))) => {
                    let mut width = 0;
                    for (class, arg) in classes.iter().zip(args) {
                        width = width.max(text(*class, arg)?);
                    }
                    Some(Wide(width))
                }
                Len | Length if matches!(classes[0], Wide(_)) => Some(Derived),
                Repeat if matches!(classes[0], Wide(_)) && rest_plain(1) => {
                    // A count only known on the record needs the key buffer
                    // the original overflows (4004) on writing.
                    let count = constant(&args[1])?.trunc().clamp(0.0, 1e6) as i32;
                    Some(Wide(first.saturating_mul(count).min(i32::from(u16::MAX))))
                }
                Descend if matches!(classes[0], Wide(_)) => Some(Descended(first)),
                Soundex if matches!(classes[0], Wide(_)) => Some(Narrow(4)),
                // The text's UTF-16 bytes in base64, and one more.
                Base64Encode if matches!(classes[0], Wide(_)) => {
                    Some(Narrow((first * 2 + 2) / 3 * 4 + 1))
                }
                Value if matches!(classes[0], Wide(_)) => Some(Derived),
                _ => None,
            }
        }
    }
}

/// How `node` takes Unicode text.
pub(crate) fn unicode(node: &Node) -> Unicode {
    match classify(node) {
        Some(Class::Plain) => Unicode::No,
        Some(Class::Wide(width)) => Unicode::Text {
            width,
            descending: false,
        },
        Some(Class::Descended(width)) => Unicode::Text {
            width,
            descending: true,
        },
        Some(Class::Narrow(width)) => Unicode::Narrow { width },
        Some(Class::Derived) if matches!(node.ty, Type::Number(_)) => Unicode::Number,
        Some(Class::Derived) | None => Unicode::Unsupported,
    }
}

/// `node`'s value on `row`; `None` for an expression [`unicode`] refuses.
pub(crate) fn eval<R: Row + ?Sized>(
    node: &Node,
    row: &mut R,
    env: &Env,
) -> Result<Option<UVal>, Failure<R::Error>> {
    let Some(class) = classify(node) else {
        return Ok(None);
    };
    if class == Class::Plain {
        return Ok(Some(match eval::eval(node, row, env)? {
            Val::Null => UVal::Null,
            value => UVal::Plain(value),
        }));
    }
    let sub = |node: &Node, row: &mut R| eval(node, row, env);
    Ok(Some(match &node.kind {
        Kind::Field(field) => match row.value(field.index).map_err(Failure::Row)? {
            Value::Text(text) => UVal::Text(text),
            _ => UVal::Null,
        },
        Kind::Binary(op, left, right) => {
            let (Some(l), Some(r)) = (sub(left, row)?, sub(right, row)?) else {
                return Ok(None);
            };
            match node.ty {
                Type::String { .. } | Type::Null if matches!(class, Class::Wide(_)) => {
                    match (text_of(l), text_of(r)) {
                        (Some(mut l), Some(r)) => {
                            l.push_str(&r);
                            UVal::Text(l)
                        }
                        _ => UVal::Null,
                    }
                }
                Type::String { .. } | Type::Null => match (l, r) {
                    (UVal::Plain(Val::Str(mut l)), UVal::Plain(Val::Str(r))) => {
                        l.extend(r);
                        UVal::Plain(Val::Str(l))
                    }
                    _ => UVal::Null,
                },
                _ => {
                    match eval::binary(*op, val(l), val(r), left.ty, right.ty, (false, false), env)?
                    {
                        Val::Null => UVal::Null,
                        value => UVal::Plain(value),
                    }
                }
            }
        }
        Kind::Call(func, args) => {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                let Some(value) = sub(arg, row)? else {
                    return Ok(None);
                };
                values.push(value);
            }
            let wide = args
                .iter()
                .any(|arg| matches!(classify(arg), Some(Class::Wide(_))));
            if wide && matches!(func, Func::Soundex | Func::Base64Encode | Func::Value) {
                let wide_first = matches!(args.first().and_then(classify), Some(Class::Wide(_)));
                match narrow::<R>(*func, values, wide_first, node, args, env)? {
                    Some(value) => value,
                    None => return Ok(None),
                }
            } else if wide {
                match call(*func, values) {
                    Some(value) => value,
                    None => return Ok(None),
                }
            } else if values.contains(&UVal::Null) {
                UVal::Null
            } else {
                let types: Vec<Type> = args.iter().map(|arg| arg.ty).collect();
                let values: Vec<Val> = values.into_iter().map(val).collect();
                match func::apply(*func, &values, &types, node.ty, env)? {
                    Val::Null => UVal::Null,
                    value => UVal::Plain(value),
                }
            }
        }
        _ => return Ok(None),
    }))
}

/// `value` as text: widened if it is 8-bit; `None` for NULL.
fn text_of(value: UVal) -> Option<String> {
    match value {
        UVal::Text(text) => Some(text),
        UVal::Plain(Val::Str(bytes)) => Some(cp1252::decode(&bytes)),
        _ => None,
    }
}

fn val(value: UVal) -> Val {
    match value {
        UVal::Plain(value) => value,
        _ => Val::Null,
    }
}

fn number(value: &UVal) -> i64 {
    match value {
        UVal::Plain(Val::Number(n)) => n.trunc() as i64,
        _ => 0,
    }
}

/// `text` as the original engine makes it 8-bit for the functions that take
/// no Unicode: Windows-1252, a 0x1A for each character that has no place in it
/// (a supplementary one too, which is one character).
fn lossy(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| match cp1252::encode(c.encode_utf8(&mut [0; 4])) {
            encoded if encoded == b"?" && c != '?' => 0x1A,
            encoded => encoded[0],
        })
        .collect()
}

/// What `func`, which takes 8-bit text, makes of values some of which are
/// Unicode; `None` for one it does not know.
fn narrow<R: Row + ?Sized>(
    func: Func,
    values: Vec<UVal>,
    wide_first: bool,
    node: &Node,
    args: &[Node],
    env: &Env,
) -> Result<Option<UVal>, Failure<R::Error>> {
    if values.contains(&UVal::Null) {
        return Ok(Some(UVal::Null));
    }
    // It encodes what the field holds, UTF-16 as it is.
    if func == Func::Base64Encode && wide_first {
        let Some(text) = values.into_iter().next().and_then(text_of) else {
            return Ok(None);
        };
        let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        return Ok(Some(UVal::Plain(Val::Str(func::base64_encode(&bytes)))));
    }
    let values: Vec<Val> = values
        .into_iter()
        .map(|value| match value {
            UVal::Text(text) => Val::Str(lossy(&text)),
            UVal::Plain(value) => value,
            UVal::Null => Val::Null,
        })
        .collect();
    let types: Vec<Type> = args.iter().map(|arg| arg.ty).collect();
    Ok(Some(
        match func::apply(func, &values, &types, node.ty, env)? {
            Val::Null => UVal::Null,
            value => UVal::Plain(value),
        },
    ))
}

/// The units of UTF-16 text `text` takes: what `LEN` counts, though the
/// other functions count characters (`fixtures/write/nchar_expr`).
fn units(text: &str) -> f64 {
    text.encode_utf16().count() as f64
}

fn call(func: Func, mut args: Vec<UVal>) -> Option<UVal> {
    use Func::*;
    // Every argument of these is evaluated, but not all of them need a value.
    match func {
        Iif => {
            let otherwise = args.pop()?;
            let then = args.pop()?;
            let chosen = match args.pop()? {
                UVal::Plain(Val::Logical(true)) => then,
                UVal::Plain(Val::Logical(false)) => otherwise,
                _ => return Some(UVal::Null),
            };
            return Some(text_of(chosen).map_or(UVal::Null, UVal::Text));
        }
        Coalesce => {
            let first = args.into_iter().find_map(text_of);
            return Some(first.map_or(UVal::Null, UVal::Text));
        }
        _ => {}
    }
    if args.contains(&UVal::Null) {
        return Some(UVal::Null);
    }
    let text = text_of(args[0].clone())?;
    let chars: Vec<char> = text.chars().collect();
    let string = |chars: Vec<char>| UVal::Text(chars.into_iter().collect());
    Some(match func {
        Upper => UVal::Text(icu::upper(&text)),
        Lower => UVal::Text(icu::lower(&text)),
        Trim => UVal::Text(text.trim_end_matches(' ').to_owned()),
        LTrim => UVal::Text(text.trim_start_matches(' ').to_owned()),
        AllTrim => UVal::Text(text.trim_matches(' ').to_owned()),
        Reverse => string(chars.into_iter().rev().collect()),
        Substr => string(func::substr(
            &chars,
            number(&args[1]),
            args.get(2).map(number),
        )),
        Left => string(func::left(&chars, number(&args[1]))),
        Right => string(func::right(&chars, number(&args[1]))),
        PadL | PadR | PadC => {
            // The fill is its first two UTF-16 units, as many times as the
            // text is short of characters (`fixtures/write/nchar_expr`). A
            // pair cut in two weighs nothing, as U+0001 does not.
            let mut fill: Vec<char> = match args.get(2) {
                Some(fill) => char::decode_utf16(text_of(fill.clone())?.encode_utf16().take(2))
                    .map(|c| c.unwrap_or('\u{1}'))
                    .collect(),
                None => Vec::new(),
            };
            if fill.is_empty() {
                fill.push(' ');
            }
            let length = number(&args[1]).max(0) as usize;
            if chars.len() >= length {
                string(chars[..length].to_vec())
            } else {
                let missing = length - chars.len();
                let before = match func {
                    PadL => missing,
                    PadR => 0,
                    _ => missing / 2,
                };
                let mut padded = fill.repeat(before);
                padded.extend_from_slice(&chars);
                padded.extend(fill.repeat(missing - before));
                string(padded)
            }
        }
        Descend => UVal::Text(text),
        Repeat => string(chars.repeat(number(&args[1]).max(0) as usize)),
        Len => UVal::Plain(Val::Number(units(&text))),
        Length => UVal::Plain(Val::Number(units(text.trim_end_matches(' ')))),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use tflip_storage::field::FieldType;

    use super::*;
    use crate::{Expr, Field, Schema};

    struct Table;

    const FIELDS: [(&str, FieldType, u16); 3] = [
        ("nc", FieldType::NChar, 8),
        ("nv", FieldType::NVarChar, 10),
        ("c", FieldType::Character, 4),
    ];

    impl Schema for Table {
        fn alias(&self) -> &str {
            "t"
        }

        fn field(&self, name: &str) -> Option<Field> {
            let index = FIELDS.iter().position(|f| f.0.eq_ignore_ascii_case(name))?;
            Some(Field {
                index,
                field_type: FIELDS[index].1,
                length: FIELDS[index].2,
                decimals: 0,
            })
        }
    }

    struct Record(Vec<Value>);

    impl Row for Record {
        type Error = ();

        fn value(&mut self, index: usize) -> Result<Value, ()> {
            Ok(self.0[index].clone())
        }

        fn record_number(&self) -> u32 {
            1
        }

        fn deleted(&self) -> bool {
            false
        }
    }

    fn env() -> Env {
        Env {
            exact: false,
            date_format: "MM/DD/CCYY".into(),
            epoch: crate::text::DEFAULT_EPOCH,
            now: (0, 0),
            utc: (0, 0),
            case_fold: None,
            weights: None,
            dynamic: None,
            blank_empty: false,
            contains: None,
        }
    }

    /// `expression` on a record of `nc`, `nv` and `c`: how it takes Unicode
    /// text, and its value if it does.
    fn run(expression: &str, nc: &str, nv: &str, c: &str) -> (Unicode, Option<UVal>) {
        let expr = Expr::compile(expression, &Table).unwrap();
        let mut row = Record(vec![
            Value::Text(nc.into()),
            Value::Text(nv.into()),
            Value::Text(c.into()),
        ]);
        (expr.unicode(), expr.eval_unicode(&mut row, &env()).unwrap())
    }

    fn text(expression: &str, nc: &str, nv: &str, c: &str) -> String {
        match run(expression, nc, nv, c).1 {
            Some(UVal::Text(text)) => text,
            other => panic!("{expression}: {other:?}"),
        }
    }

    #[test]
    fn widths_follow_the_engine() {
        // `fixtures/write/nchar_expr`: what the original's key lengths say.
        let width = |e: &str| run(e, "", "", "").0;
        assert_eq!(
            width("UPPER(nc)"),
            Unicode::Text {
                width: 8,
                descending: false
            }
        );
        assert_eq!(
            width("nc+nv"),
            Unicode::Text {
                width: 18,
                descending: false
            }
        );
        assert_eq!(
            width("nc+c+nv"),
            Unicode::Text {
                width: 22,
                descending: false
            }
        );
        assert_eq!(
            width("LEFT(nc,20)"),
            Unicode::Text {
                width: 8,
                descending: false
            }
        );
        assert_eq!(
            width("LEFT(nc,-1)"),
            Unicode::Text {
                width: -1,
                descending: false
            }
        );
        assert_eq!(
            width("PADL(nc,12,nv)"),
            Unicode::Text {
                width: 12,
                descending: false
            }
        );
        assert_eq!(
            width("PADL(nc,0)"),
            Unicode::Text {
                width: 8,
                descending: false
            }
        );
        assert_eq!(
            width("IIF(.T.,nc,nv)"),
            Unicode::Text {
                width: 10,
                descending: false
            }
        );
        assert_eq!(width("LEN(nc)+1"), Unicode::Number);
        assert_eq!(width("UPPER(c)"), Unicode::No);
        assert_eq!(width("TRANSFORM(nc,'@!')"), Unicode::Unsupported);
        // `fixtures/write/nchar_expr_more`.
        assert_eq!(
            width("REPEAT(nc,2)"),
            Unicode::Text {
                width: 16,
                descending: false
            }
        );
        assert_eq!(
            width("DESCEND(nv)"),
            Unicode::Text {
                width: 10,
                descending: true
            }
        );
        assert_eq!(width("DESCEND(nc)+nv"), Unicode::Unsupported);
        assert_eq!(width("REPEAT(nc,LEN(nc))"), Unicode::Unsupported);
        assert_eq!(width("BASE64ENCODE(nc)"), Unicode::Narrow { width: 25 });
        assert_eq!(width("SOUNDEX(nv)+STR(1,3)"), Unicode::Narrow { width: 7 });
        assert_eq!(width("VAL(nc)"), Unicode::Number);
        assert_eq!(
            width("COALESCE(c,nv)"),
            Unicode::Text {
                width: 10,
                descending: false
            }
        );
        assert_eq!(width("IIF(.T.,'xyz',nc)"), Unicode::Unsupported);
    }

    #[test]
    fn values_follow_the_engine() {
        // Case a character at a time; blanks of the field kept inside a sum.
        assert_eq!(
            text("UPPER(nc)", "a\u{df}\u{e9}  ", "", ""),
            "A\u{df}\u{c9}  "
        );
        assert_eq!(text("nc+c", "ab    ", "", "\u{e9}"), "ab    \u{e9}");
        // Characters, not UTF-16 units, for the text functions; units for `LEN`.
        assert_eq!(text("LEFT(nc,2)", "a\u{1f642}b", "", ""), "a\u{1f642}");
        assert_eq!(text("SUBSTR(nc,2,1)", "a\u{1f642}b", "", ""), "\u{1f642}");
        assert_eq!(
            run("LEN(nc)", "a\u{1f642}", "", ""),
            (Unicode::Number, Some(UVal::Plain(Val::Number(3.0))))
        );
        // The fill is two UTF-16 units: a pair cut in two weighs nothing.
        assert_eq!(text("PADL(nc,5,nv)", "ab", "xyz", ""), "xyxyxyab");
        assert_eq!(text("PADR(nc,3,nv)", "ab", "\u{1f642}", ""), "ab\u{1f642}");
        assert_eq!(text("PADR(nc,3,nv)", "ab", "x\u{1f642}", ""), "abx\u{1}");
    }
}
