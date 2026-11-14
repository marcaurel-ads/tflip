//! Tokens to a syntax tree.
//!
//! From the loosest binding to the tightest: `;`, `OR`, `AND`, the
//! comparisons (one level, left to right: `1 < 2 = .T.` is `(1 < 2) = .T.`),
//! `+` and `-`, `*`, `/` and `%`, `^` and `**` (left to right too: `2^3^2` is
//! 64), then `NOT` (so `NOT 1 = 2` is `(NOT 1) = 2`, a type mismatch, as in
//! the original engine).

use crate::func::Func;
use crate::lex::{Op, Token};
use crate::{ErrorKind, Schema};

/// A syntax tree, names not resolved yet.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ast {
    Number {
        value: f64,
        digits: u16,
        decimals: u16,
    },
    Str(Vec<u8>),
    Logical(bool),
    Null,
    /// A name, with the alias that qualifies it.
    Name {
        alias: Option<String>,
        name: String,
    },
    Call {
        name: String,
        args: Vec<Ast>,
    },
    Binary {
        op: Op,
        left: Box<Ast>,
        right: Box<Ast>,
    },
    And(Box<Ast>, Box<Ast>),
    Or(Box<Ast>, Box<Ast>),
    Not(Box<Ast>),
    Join(Box<Ast>, Box<Ast>),
}

/// Parses `tokens` into one expression. With the table's `schema`, names
/// and functions are resolved where they are read: as in the original
/// engine, the first error from the left is the one reported, and
/// unbalanced parentheses only when the tokens run out.
pub(crate) fn parse(tokens: &[Token], schema: Option<&dyn Schema>) -> Result<Ast, ErrorKind> {
    let opened = tokens.iter().filter(|t| **t == Token::Open).count();
    let closed = tokens.iter().filter(|t| **t == Token::Close).count();
    let mut parser = Parser {
        tokens,
        at: 0,
        schema,
        unbalanced: opened > closed,
    };
    let tree = parser.join()?;
    match parser.peek() {
        None => Ok(tree),
        Some(Token::Close | Token::Comma) => Err(ErrorKind::Incomplete),
        Some(&Token::Bad(error)) => Err(error),
        Some(_) => Err(ErrorKind::UnsupportedOperator),
    }
}

struct Parser<'t> {
    tokens: &'t [Token],
    at: usize,
    schema: Option<&'t dyn Schema>,
    /// Whether there are more opening parentheses than closing ones.
    unbalanced: bool,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<&Token> {
        let token = self.tokens.get(self.at);
        self.at += 1;
        token
    }

    fn eat(&mut self, token: &Token) -> bool {
        let found = self.peek() == Some(token);
        if found {
            self.at += 1;
        }
        found
    }

    fn join(&mut self) -> Result<Ast, ErrorKind> {
        let mut left = self.or()?;
        while self.eat(&Token::Join) {
            left = Ast::Join(Box::new(left), Box::new(self.or()?));
        }
        Ok(left)
    }

    fn or(&mut self) -> Result<Ast, ErrorKind> {
        let mut left = self.and()?;
        while self.eat(&Token::Or) {
            left = Ast::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Ast, ErrorKind> {
        let mut left = self.binary(0)?;
        while self.eat(&Token::And) {
            left = Ast::And(Box::new(left), Box::new(self.binary(0)?));
        }
        Ok(left)
    }

    /// The binary operators from `level` on: comparisons, then `+ -`, then
    /// `* / %`, then powers.
    fn binary(&mut self, level: usize) -> Result<Ast, ErrorKind> {
        const LEVELS: [&[Op]; 4] = [
            &[
                Op::Eq,
                Op::Exactly,
                Op::Ne,
                Op::Lt,
                Op::Gt,
                Op::Le,
                Op::Ge,
                Op::In,
            ],
            &[Op::Add, Op::Sub],
            &[Op::Mul, Op::Div, Op::Mod],
            &[Op::Pow],
        ];
        let Some(ops) = LEVELS.get(level) else {
            return self.unary();
        };
        let mut left = self.binary(level + 1)?;
        while let Some(&Token::Op(op)) = self.peek()
            && ops.contains(&op)
        {
            self.at += 1;
            let right = self.binary(level + 1)?;
            left = Ast::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Ast, ErrorKind> {
        if self.eat(&Token::Not) {
            return Ok(Ast::Not(Box::new(self.unary()?)));
        }
        self.operand()
    }

    fn operand(&mut self) -> Result<Ast, ErrorKind> {
        let first = self.at == 0;
        let unbalanced = self.unbalanced;
        let ast = match self.next() {
            Some(&Token::Number {
                value,
                digits,
                decimals,
            }) => Ast::Number {
                value,
                digits,
                decimals,
            },
            Some(Token::Str(bytes)) => Ast::Str(bytes.clone()),
            Some(&Token::Logical(value)) => Ast::Logical(value),
            Some(Token::Null) => Ast::Null,
            Some(Token::Open) => {
                let inner = self.join()?;
                if !self.eat(&Token::Close) {
                    return Err(self.missing());
                }
                inner
            }
            Some(Token::Ident(name)) => {
                let name = name.clone();
                if self.eat(&Token::Open) {
                    if self.schema.is_some() && Func::named(&name).is_none() {
                        return Err(ErrorKind::UnsupportedFunction);
                    }
                    let args = self.arguments()?;
                    Ast::Call { name, args }
                } else if self.eat(&Token::Alias) {
                    match self.next() {
                        Some(Token::Ident(field)) => {
                            let field = field.clone();
                            self.resolve(Some(&name), &field)?;
                            Ast::Name {
                                alias: Some(name),
                                name: field,
                            }
                        }
                        _ => return Err(ErrorKind::UnknownIdentifier),
                    }
                } else {
                    self.resolve(None, &name)?;
                    Ast::Name { alias: None, name }
                }
            }
            Some(&Token::Bad(error)) => return Err(error),
            // Nothing where an operand belongs: the end, or an operator at
            // the start, is taken as a name that is missing; an operator
            // after another as an operator out of place.
            None if unbalanced => return Err(ErrorKind::Parentheses),
            None | Some(Token::Close | Token::Comma) => return Err(ErrorKind::UnknownIdentifier),
            Some(_) if first => return Err(ErrorKind::UnknownIdentifier),
            Some(_) => return Err(ErrorKind::UnsupportedOperator),
        };
        Ok(ast)
    }

    /// Checks that the field `name`, qualified by `alias`, exists.
    fn resolve(&self, alias: Option<&str>, name: &str) -> Result<(), ErrorKind> {
        let Some(schema) = self.schema else {
            return Ok(());
        };
        if alias.is_some_and(|alias| !crate::names_table(alias, schema)) {
            return Err(ErrorKind::UnknownAlias);
        }
        schema
            .field(name)
            .map(|_| ())
            .ok_or(ErrorKind::UnknownIdentifier)
    }

    /// A function's arguments, after its `(`.
    fn arguments(&mut self) -> Result<Vec<Ast>, ErrorKind> {
        let mut args = Vec::new();
        if self.eat(&Token::Close) {
            return Ok(args);
        }
        loop {
            args.push(self.join()?);
            if self.eat(&Token::Close) {
                return Ok(args);
            }
            if !self.eat(&Token::Comma) {
                return Err(self.missing());
            }
        }
    }

    /// The error for a `)` that is not where it should be.
    fn missing(&self) -> ErrorKind {
        match self.peek() {
            None => ErrorKind::Parentheses,
            Some(&Token::Bad(error)) => error,
            Some(_) => ErrorKind::UnsupportedOperator,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex::lex;

    fn tree(text: &str) -> Result<Ast, ErrorKind> {
        parse(&lex(text, None), None)
    }

    fn number(value: f64) -> Box<Ast> {
        let digits = if value == 0.0 { 0 } else { 1 };
        Box::new(Ast::Number {
            value,
            digits,
            decimals: 0,
        })
    }

    #[test]
    fn precedence() {
        let pow = Ast::Binary {
            op: Op::Pow,
            left: number(2.0),
            right: number(3.0),
        };
        let pow = Ast::Binary {
            op: Op::Pow,
            left: Box::new(pow),
            right: number(2.0),
        };
        assert_eq!(tree("2^3^2").unwrap(), pow);
        let not = Ast::Not(Box::new(Ast::Logical(true)));
        let eq = Ast::Binary {
            op: Op::Eq,
            left: Box::new(not),
            right: number(1.0),
        };
        assert_eq!(tree(".NOT. .T. = 1").unwrap(), eq);
    }

    #[test]
    fn errors_as_the_original_engine_reports_them() {
        assert_eq!(tree("1 +"), Err(ErrorKind::UnknownIdentifier));
        assert_eq!(tree("= 1"), Err(ErrorKind::UnknownIdentifier));
        assert_eq!(tree(","), Err(ErrorKind::UnknownIdentifier));
        assert_eq!(tree("()"), Err(ErrorKind::UnknownIdentifier));
        assert_eq!(tree("1 + * 2"), Err(ErrorKind::UnsupportedOperator));
        assert_eq!(tree("1 2"), Err(ErrorKind::UnsupportedOperator));
        assert_eq!(tree("-(1)"), Err(ErrorKind::UnsupportedOperator));
        assert_eq!(tree("1)"), Err(ErrorKind::Incomplete));
        assert_eq!(tree("UPPER(k))"), Err(ErrorKind::Incomplete));
        assert_eq!(tree("UPPER("), Err(ErrorKind::Parentheses));
    }
}
