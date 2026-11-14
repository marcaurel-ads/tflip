//! Expression text to tokens.

use tflip_storage::cp1252;

use crate::{ErrorKind, Schema};

/// A token, without its position (errors have no position to report).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token {
    /// A number and its display format's decimals (the digits written after
    /// the point) and integer digits.
    Number {
        value: f64,
        digits: u16,
        decimals: u16,
    },
    /// A string literal, as Windows-1252 bytes.
    Str(Vec<u8>),
    Logical(bool),
    Null,
    Ident(String),
    /// `->`, or `.` between two names.
    Alias,
    And,
    Or,
    Not,
    Op(Op),
    Open,
    Close,
    Comma,
    /// `;`, which joins keys.
    Join,
    /// Text that is no token, and the error it is; the last token. The
    /// original engine reports it only when it gets there.
    Bad(ErrorKind),
}

/// A binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    /// `=`
    Eq,
    /// `==`
    Exactly,
    /// `!=`, `<>`, `#`
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    /// `$`: is a substring of.
    In,
}

impl Token {
    /// Whether the token ends an operand, after which `+` and `-` are
    /// operators rather than signs.
    fn ends_operand(&self) -> bool {
        matches!(
            self,
            Token::Number { .. }
                | Token::Str(_)
                | Token::Logical(_)
                | Token::Null
                | Token::Ident(_)
                | Token::Close
        )
    }
}

/// The tokens of `text`, up to the first that is [`Token::Bad`]. With the
/// table's `schema`, words joined by single spaces are one name where the
/// table has a field of that name (`Query Number`), as procedures' results
/// have.
pub(crate) fn lex(text: &str, schema: Option<&dyn Schema>) -> Vec<Token> {
    lex_spans(text, schema).0
}

/// [`lex`], with where each token is in `text`: a range of its characters
/// (not bytes).
pub(crate) fn lex_spans(
    text: &str,
    schema: Option<&dyn Schema>,
) -> (Vec<Token>, Vec<std::ops::Range<usize>>) {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens: Vec<Token> = Vec::new();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() && !matches!(tokens.last(), Some(Token::Bad(_))) {
        let start = i;
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let operand_expected = !tokens.last().is_some_and(Token::ends_operand);
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let token = match c {
            '\'' | '"' | '[' => {
                let close = if c == '[' { ']' } else { c };
                let Some(end) = chars[i + 1..].iter().position(|&d| d == close) else {
                    tokens.push(Token::Bad(ErrorKind::MissingDelimiter));
                    spans.push(start..chars.len());
                    break;
                };
                let string: String = chars[i + 1..i + 1 + end].iter().collect();
                i += end + 2;
                Token::Str(cp1252::encode(&string))
            }
            '0'..='9' => number(&chars, &mut i, 1.0),
            '.' if next.is_some_and(|d| d.is_ascii_digit()) => number(&chars, &mut i, 1.0),
            '+' | '-' if operand_expected => {
                i += 1;
                number(&chars, &mut i, if c == '-' { -1.0 } else { 1.0 })
            }
            '.' => match dot_word(&chars[i..]) {
                Some((token, length)) => {
                    i += length;
                    token
                }
                None if matches!(tokens.last(), Some(Token::Ident(_)))
                    && next.is_some_and(is_name_start) =>
                {
                    i += 1;
                    Token::Alias
                }
                None => Token::Bad(ErrorKind::UnsupportedOperator),
            },
            c if is_name_start(c) => {
                let mut length = chars[i..].iter().take_while(|&&c| is_name_char(c)).count();
                if let Some(schema) = schema {
                    length = spaced_name(&chars[i..], length, schema);
                }
                let name: String = chars[i..i + length].iter().collect();
                i += length;
                let after = chars[i..].iter().find(|c| !c.is_whitespace()).copied();
                let call = after == Some('(');
                let upper = name.to_ascii_uppercase();
                if !call
                    && let Some(schema) = schema
                    && names_field(&upper, operand_expected, after, schema)
                    && schema.field(&name).is_some()
                {
                    Token::Ident(name)
                } else {
                    keyword(&upper, call, name)
                }
            }
            _ => {
                let pair: String = chars[i..chars.len().min(i + 2)].iter().collect();
                let (token, length) = match pair.as_str() {
                    // A qualifier is `name->field`, the arrow right after
                    // the name, as in the original engine.
                    "->" if i > 0 && chars[i - 1].is_whitespace() => {
                        (Token::Bad(ErrorKind::UnsupportedOperator), 2)
                    }
                    "->" => (Token::Alias, 2),
                    "**" => (Token::Op(Op::Pow), 2),
                    "==" => (Token::Op(Op::Exactly), 2),
                    "!=" | "<>" => (Token::Op(Op::Ne), 2),
                    "<=" => (Token::Op(Op::Le), 2),
                    ">=" => (Token::Op(Op::Ge), 2),
                    _ => (
                        match c {
                            '+' => Token::Op(Op::Add),
                            '-' => Token::Op(Op::Sub),
                            '*' => Token::Op(Op::Mul),
                            '/' => Token::Op(Op::Div),
                            '%' => Token::Op(Op::Mod),
                            '^' => Token::Op(Op::Pow),
                            '=' => Token::Op(Op::Eq),
                            '#' => Token::Op(Op::Ne),
                            '<' => Token::Op(Op::Lt),
                            '>' => Token::Op(Op::Gt),
                            '$' => Token::Op(Op::In),
                            '!' => Token::Not,
                            '(' => Token::Open,
                            ')' => Token::Close,
                            ',' => Token::Comma,
                            ';' => Token::Join,
                            _ => Token::Bad(ErrorKind::UnknownIdentifier),
                        },
                        1,
                    ),
                };
                i += length;
                token
            }
        };
        tokens.push(token);
        spans.push(start..i);
    }
    (tokens, spans)
}

/// The length of the name at the start of `chars`, whose first word is
/// `word` characters long: the longest run of words joined by single
/// spaces that names a field of `schema`, else the first word.
fn spaced_name(chars: &[char], word: usize, schema: &dyn Schema) -> usize {
    let mut longest = word;
    let mut end = word;
    while chars.get(end) == Some(&' ') && chars.get(end + 1).is_some_and(|&c| is_name_char(c)) {
        end += 1 + chars[end + 1..]
            .iter()
            .take_while(|&&c| is_name_char(c))
            .count();
        let name: String = chars[..end].iter().collect();
        if schema.field(&name).is_some() {
            longest = end;
        }
    }
    longest
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The number at `chars[*i]`: digits with an optional point and fraction,
/// none of them required (a sign alone is 0). The exponent of C is not part
/// of a number: `1e3` is `1` followed by the name `e3`.
fn number(chars: &[char], i: &mut usize, sign: f64) -> Token {
    let digits = |from: usize| {
        chars[from..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count()
    };
    let whole = digits(*i);
    let mut text: String = chars[*i..*i + whole].iter().collect();
    *i += whole;
    let mut decimals = 0;
    if chars.get(*i) == Some(&'.') {
        decimals = digits(*i + 1);
        text.push('.');
        text.extend(&chars[*i + 1..*i + 1 + decimals]);
        *i += 1 + decimals;
    }
    // A sign alone is 0, not -0.
    let value: f64 = if whole + decimals == 0 {
        0.0
    } else {
        sign * text.parse().unwrap_or(0.0)
    };
    let significant = text
        .split('.')
        .next()
        .unwrap_or("")
        .trim_start_matches('0')
        .len();
    Token::Number {
        value,
        digits: significant as u16,
        decimals: decimals as u16,
    }
}

/// The token of the word `upper` (in capitals) where no field takes it over.
fn keyword(upper: &str, call: bool, name: String) -> Token {
    match upper {
        "AND" => Token::And,
        "OR" => Token::Or,
        "NOT" => Token::Not,
        _ if call => Token::Ident(name),
        "TRUE" => Token::Logical(true),
        "FALSE" => Token::Logical(false),
        "NULL" => Token::Null,
        _ => Token::Ident(name),
    }
}

/// Whether the word `upper` (in capitals), followed by the character
/// `after`, names the table's field of that name, if it has one, rather than
/// being a keyword: `TRUE`, `FALSE` and `NULL` always do; `AND`, `OR` and
/// `NOT` as [`Schema::fields_hide_operators`] says.
fn names_field(
    upper: &str,
    operand_expected: bool,
    after: Option<char>,
    schema: &dyn Schema,
) -> bool {
    match upper {
        "TRUE" | "FALSE" | "NULL" => true,
        "AND" | "OR" => schema.fields_hide_operators() || operand_expected,
        "NOT" => {
            schema.fields_hide_operators()
                || (operand_expected && matches!(after, None | Some(',' | ')')))
        }
        _ => false,
    }
}

/// The dotted word at the start of `chars` (`.T.`, `.AND.`), and its length.
fn dot_word(chars: &[char]) -> Option<(Token, usize)> {
    let end = chars[1..].iter().position(|&c| c == '.')? + 1;
    let word: String = chars[1..end].iter().collect();
    let token = match word.to_ascii_uppercase().as_str() {
        "T" | "Y" => Token::Logical(true),
        "F" | "N" => Token::Logical(false),
        "AND" => Token::And,
        "OR" => Token::Or,
        "NOT" => Token::Not,
        _ => return None,
    };
    Some((token, end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex_(text: &str) -> Result<Vec<Token>, ErrorKind> {
        let tokens = lex(text, None);
        match tokens.last() {
            Some(&Token::Bad(error)) => Err(error),
            _ => Ok(tokens),
        }
    }

    fn num(value: f64, decimals: u16) -> Token {
        let digits = (value.abs().trunc() as u64)
            .to_string()
            .trim_start_matches('0')
            .len() as u16;
        Token::Number {
            value,
            digits,
            decimals,
        }
    }

    #[test]
    fn signs_start_numbers_where_an_operand_is_expected() {
        assert_eq!(lex_("-2").unwrap(), [num(-2.0, 0)]);
        assert_eq!(lex_("-").unwrap(), [num(0.0, 0)]);
        assert_eq!(
            lex_("1--2").unwrap(),
            [num(1.0, 0), Token::Op(Op::Sub), num(-2.0, 0)]
        );
        assert_eq!(
            lex_("--1").unwrap(),
            [num(0.0, 0), Token::Op(Op::Sub), num(1.0, 0)]
        );
        assert_eq!(lex_("-.5").unwrap(), [num(-0.5, 1)]);
        assert_eq!(lex_("1.").unwrap(), [num(1.0, 0)]);
    }

    #[test]
    fn dotted_words_and_aliases() {
        assert_eq!(
            lex_(".t..AND..N.").unwrap(),
            [Token::Logical(true), Token::And, Token::Logical(false)]
        );
        assert_eq!(
            lex_("keys.k_char").unwrap(),
            [
                Token::Ident("keys".into()),
                Token::Alias,
                Token::Ident("k_char".into())
            ]
        );
        assert_eq!(lex_(". x"), Err(ErrorKind::UnsupportedOperator));
    }

    #[test]
    fn strings_and_their_delimiters() {
        assert_eq!(lex_("[a'b]").unwrap(), [Token::Str(b"a'b".to_vec())]);
        assert_eq!(
            lex_("'\u{e4}\u{20ac}'").unwrap(),
            [Token::Str(vec![0xE4, 0x80])]
        );
        assert_eq!(lex_("'abc"), Err(ErrorKind::MissingDelimiter));
        assert_eq!(lex_("{1}"), Err(ErrorKind::UnknownIdentifier));
    }
}
