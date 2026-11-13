//! Statement text to tokens, each with the position of its first character.

use crate::error::{Pos, SqlError};

/// A token.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    /// A name or keyword, as written.
    Ident(String),
    /// A name in `"…"` or `[…]`, which is never a keyword.
    Quoted(String),
    /// A string literal, its quotes removed and `''` undone.
    Str(String),
    /// A number literal, as written.
    Number(String),
    /// `?`, or `:name`.
    Param(Option<String>),
    /// An operator or punctuation: `( ) , . * + - / % = <> < > <= >= ; { }
    /// | & ^ ~ !`.
    Punct(&'static str),
}

impl Tok {
    /// Whether the token is the keyword `word` (in any case).
    pub(crate) fn is(&self, word: &str) -> bool {
        matches!(self, Tok::Ident(name) if name.eq_ignore_ascii_case(word))
    }

    /// The token as `Unexpected token` names it: a text has no name there
    /// (`fixtures/sql/gram_syntax.json`).
    pub(crate) fn unexpected_text(&self) -> String {
        match self {
            Tok::Str(_) => String::new(),
            other => other.text(),
        }
    }

    /// The token as the original engine quotes it in messages.
    pub(crate) fn text(&self) -> String {
        match self {
            Tok::Ident(s) | Tok::Number(s) => s.clone(),
            Tok::Quoted(s) => format!("\"{s}\""),
            Tok::Str(s) => format!("'{s}'"),
            // A named parameter too (`GRANT … TO DB:Admin` is `Unexpected token: ?`).
            Tok::Param(_) => "?".to_owned(),
            // The original reads `!=` as `!` and then `=`.
            Tok::Punct("!=") => "!".to_owned(),
            Tok::Punct(p) => (*p).to_owned(),
        }
    }
}

/// A token and where it starts.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Token {
    pub tok: Tok,
    pub pos: Pos,
    /// Just past the token's last character.
    pub end: Pos,
}

/// Whether `c` can be part of a name: letters (any alphabet), digits, `_`.
pub(crate) fn name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

const PUNCTS: [&str; 26] = [
    "<<", ">>", "<>", "<=", ">=", "!=", "(", ")", ",", ".", "*", "+", "-", "/", "%", "=", "<", ">",
    ";", "{", "}", "|", "&", "^", "~", "!",
];

/// The tokens of `text`.
pub(crate) fn lex(text: &str) -> Result<Vec<Token>, SqlError> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    let starts = |i: usize, s: &str| {
        s.chars()
            .enumerate()
            .all(|(k, c)| chars.get(i + k) == Some(&c))
    };
    while i < chars.len() {
        let c = chars[i];
        let pos = i;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if starts(i, "--(*")
            && let Some((inner, close)) = vendor_escape(&chars, i)
        {
            // ODBC's vendor escape is the braces' escape by another name.
            tokens.push(Token {
                tok: Tok::Punct("{"),
                pos,
                end: pos + 1,
            });
            for mut token in lex(&chars[inner..close].iter().collect::<String>())? {
                token.pos += inner;
                token.end += inner;
                tokens.push(token);
            }
            tokens.push(Token {
                tok: Tok::Punct("}"),
                pos: close,
                end: close + 4,
            });
            i = close + 4;
            continue;
        }
        if starts(i, "--") || starts(i, "//") {
            while i < chars.len() && chars[i] != '\n' && chars[i] != '\r' {
                i += 1;
            }
            continue;
        }
        if starts(i, "/*") {
            i += 2;
            while i < chars.len() && !starts(i, "*/") {
                i += 1;
            }
            if i >= chars.len() {
                return Err(SqlError::unexpected_end());
            }
            i += 2;
            continue;
        }
        let tok = if c == '\'' || ((c == 'N' || c == 'n') && chars.get(i + 1) == Some(&'\'')) {
            if c != '\'' {
                i += 1;
            }
            i += 1;
            let mut s = String::new();
            loop {
                match chars.get(i) {
                    None => return Err(SqlError::unexpected_end()),
                    Some('\'') if chars.get(i + 1) == Some(&'\'') => {
                        s.push('\'');
                        i += 2;
                    }
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some(&c) => {
                        s.push(c);
                        i += 1;
                    }
                }
            }
            Tok::Str(s)
        } else if c == '"' || c == '[' {
            let close = if c == '"' { '"' } else { ']' };
            let end = chars[i + 1..]
                .iter()
                .position(|&d| d == close)
                .ok_or_else(SqlError::unexpected_end)?;
            let name: String = chars[i + 1..i + 1 + end].iter().collect();
            i += end + 2;
            Tok::Quoted(name)
        } else if c.is_ascii_digit()
            || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = i;
            let mut text = String::new();
            while chars.get(i).is_some_and(char::is_ascii_digit) {
                text.push(chars[i]);
                i += 1;
            }
            if chars.get(i) == Some(&'.') {
                i += 1;
                let fraction = i;
                while chars.get(i).is_some_and(char::is_ascii_digit) {
                    i += 1;
                }
                // `5.` is the integer 5: the point is part of the number, not
                // of its value.
                if i > fraction || text.is_empty() {
                    text.push('.');
                    text.extend(&chars[fraction..i]);
                }
                // A number with more points is read whole, for the parser to
                // call malformed.
                while chars.get(i) == Some(&'.')
                    && chars.get(i + 1).is_some_and(char::is_ascii_digit)
                {
                    text.push('.');
                    i += 1;
                    while chars.get(i).is_some_and(char::is_ascii_digit) {
                        text.push(chars[i]);
                        i += 1;
                    }
                }
            }
            if matches!(chars.get(i), Some('e' | 'E')) {
                let mut j = i + 1;
                if matches!(chars.get(j), Some('+' | '-')) {
                    j += 1;
                }
                if chars.get(j).is_some_and(char::is_ascii_digit) {
                    text.extend(&chars[i..j]);
                    i = j;
                    while chars.get(i).is_some_and(char::is_ascii_digit) {
                        text.push(chars[i]);
                        i += 1;
                    }
                } else if chars.get(i + 1).is_none_or(|&c| !name_char(c)) {
                    // A bare exponent mark is a zero exponent.
                    text.push_str("e0");
                    i += 1;
                }
            }
            if i - start > 1024 {
                return Err(SqlError::lexical_element_too_large(
                    &chars[start..i].iter().collect::<String>(),
                ));
            }
            Tok::Number(text)
        } else if c == '?' {
            i += 1;
            Tok::Param(None)
        } else if c == ':' && chars.get(i + 1) == Some(&':') {
            // A statement-scope name, `::stmt.TrigRecNo`: one identifier, dots
            // and all, so the trigger host can resolve it whole.
            let start = i;
            i += 2;
            while chars.get(i).is_some_and(|&c| name_char(c) || c == '.') {
                i += 1;
            }
            Tok::Ident(chars[start..i].iter().collect())
        } else if c == ':' && chars.get(i + 1).is_some_and(|&c| name_char(c)) {
            let start = i + 1;
            i = start;
            while chars.get(i).is_some_and(|&c| name_char(c)) {
                i += 1;
            }
            Tok::Param(Some(chars[start..i].iter().collect()))
        } else if c == '.'
            && matches!(chars.get(i + 1), Some('T' | 't' | 'F' | 'f'))
            && chars.get(i + 2) == Some(&'.')
        {
            // `.T.` and `.F.`, the logical literals of the xBase languages.
            i += 3;
            Tok::Ident(chars[pos..i].iter().collect())
        } else if name_char(c) || c == '@' || c == '#' {
            let start = i;
            i += 1;
            // A quote or `&` goes on a name, a form feed too, but a quote
            // after the word of a typed literal (`X'4A'`) opens its text.
            while let Some(&c) = chars.get(i) {
                let literal_prefix = || {
                    ["x", "n", "date", "time", "timestamp", "datetime", "binary"]
                        .iter()
                        .any(|w| {
                            chars[start..i]
                                .iter()
                                .collect::<String>()
                                .eq_ignore_ascii_case(w)
                        })
                };
                if !(name_char(c)
                    || matches!(c, '@' | '#' | '$' | '&' | '\x0c')
                    || (c == '\'' && !literal_prefix()))
                {
                    break;
                }
                i += 1;
            }
            if i - start > 1024 {
                return Err(SqlError::lexical_element_too_large(
                    &chars[start..i].iter().collect::<String>(),
                ));
            }
            Tok::Ident(chars[start..i].iter().collect())
        } else if let Some(p) = PUNCTS.iter().find(|p| starts(i, p)) {
            i += p.chars().count();
            Tok::Punct(p)
        } else {
            // A character the language has no use for: a token of its own,
            // which the parser refuses where it stands.
            i += 1;
            Tok::Ident(c.to_string())
        };
        tokens.push(Token { tok, pos, end: i });
    }
    Ok(tokens)
}

/// Where the text of the vendor escape `--(*vendor(…),product(…) text *)--`
/// that starts at `at` begins and where its `*)--` is.
fn vendor_escape(chars: &[char], at: usize) -> Option<(usize, usize)> {
    let text: String = chars[at..].iter().collect();
    let close = text.find("*)--")?;
    let inner = &text[4..close];
    let rest = inner.trim_start();
    if !rest.to_ascii_lowercase().starts_with("vendor(") {
        return None;
    }
    let after_vendor = rest.find(')')? + 1;
    let rest = rest[after_vendor..].trim_start().strip_prefix(',')?;
    let rest = rest.trim_start();
    if !rest.to_ascii_lowercase().starts_with("product(") {
        return None;
    }
    let after_product = rest.find(')')? + 1;
    let consumed = inner.chars().count() - rest[after_product..].chars().count();
    let close = text[..close].chars().count();
    Some((at + 4 + consumed, at + close))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(text: &str) -> Vec<Tok> {
        lex(text).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn tokens() {
        assert_eq!(
            toks("SELECT a1, 'it''s' FROM [t x] -- c\n"),
            vec![
                Tok::Ident("SELECT".into()),
                Tok::Ident("a1".into()),
                Tok::Punct(","),
                Tok::Str("it's".into()),
                Tok::Ident("FROM".into()),
                Tok::Quoted("t x".into()),
            ]
        );
        assert_eq!(
            toks("1.5e3 .5 7. 2e"),
            vec![
                Tok::Number("1.5e3".into()),
                Tok::Number(".5".into()),
                Tok::Number("7".into()),
                Tok::Number("2e0".into()),
            ]
        );
        assert_eq!(
            toks("a<>b<=c/*x*/N'y' ? :p1"),
            vec![
                Tok::Ident("a".into()),
                Tok::Punct("<>"),
                Tok::Ident("b".into()),
                Tok::Punct("<="),
                Tok::Ident("c".into()),
                Tok::Str("y".into()),
                Tok::Param(None),
                Tok::Param(Some("p1".into())),
            ]
        );
    }

    #[test]
    fn loose_names_and_logical_literals() {
        assert_eq!(
            toks("a#b$c@d abc'def a&b a\x0cb .T. x'41'"),
            vec![
                Tok::Ident("a#b$c@d".into()),
                Tok::Ident("abc'def".into()),
                Tok::Ident("a&b".into()),
                Tok::Ident("a\x0cb".into()),
                Tok::Ident(".T.".into()),
                Tok::Ident("x".into()),
                Tok::Str("41".into()),
            ]
        );
    }

    #[test]
    fn vendor_escape_is_braces() {
        assert_eq!(
            toks("--(*vendor(Microsoft),product(ODBC) fn UCASE('a') *)--"),
            vec![
                Tok::Punct("{"),
                Tok::Ident("fn".into()),
                Tok::Ident("UCASE".into()),
                Tok::Punct("("),
                Tok::Str("a".into()),
                Tok::Punct(")"),
                Tok::Punct("}"),
            ]
        );
    }

    #[test]
    fn positions_count_characters() {
        let tokens = lex("'ä' x").unwrap();
        assert_eq!(tokens[1].pos, 4);
    }

    #[test]
    fn unterminated() {
        assert_eq!(lex("'x").unwrap_err().native, 2113);
        assert_eq!(lex("/* x").unwrap_err().native, 2113);
    }
}
