//! What kind of key an index expression builds, as the original engine
//! reports it in an order's flags (`AdsGetIndexFlags`, bits 0x4000 and
//! 0x8000, which `ace.h` does not name).
//!
//! Learnt from the original engine, by creating orders on many expressions:
//! the key of a field, of a list of fields, or of a transformation of one
//! field that keeps its order (`STR`, `DTOS`, arithmetic with numbers,
//! `name+name`) is plain; `DESCEND` of one is descending; anything else
//! (`UPPER`, `SUBSTR`, `YEAR`, two fields together, constants, `RECNO()`)
//! is computed.

use crate::lex::{Op, lex};
use crate::parse::{Ast, parse};

/// The kind of key an index expression builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// A field or fields, or a transformation that keeps their order.
    Plain,
    /// `DESCEND` of a plain key.
    Descend,
    /// Anything else, or an expression that does not parse.
    Computed,
}

/// The kind of key `expression` builds.
pub fn key_kind(expression: &str) -> KeyKind {
    let Ok(tree) = parse(&lex(expression, None), None) else {
        return KeyKind::Computed;
    };
    match &tree {
        Ast::Call { name, args } if name.eq_ignore_ascii_case("DESCEND") && args.len() == 1 => {
            if plain(&args[0]) {
                KeyKind::Descend
            } else {
                KeyKind::Computed
            }
        }
        tree if plain(tree) => KeyKind::Plain,
        _ => KeyKind::Computed,
    }
}

/// Whether `tree` is a field list whose every part keeps a field's order.
fn plain(tree: &Ast) -> bool {
    match tree {
        Ast::Join(a, b) => plain(a) && plain(b),
        tree => single_field(tree).is_some(),
    }
}

/// The one field `tree` transforms keeping its order, if it is such a
/// transformation.
fn single_field(tree: &Ast) -> Option<String> {
    let constant = |tree: &Ast| matches!(tree, Ast::Number { .. });
    match tree {
        Ast::Name { name, .. } => Some(name.to_ascii_uppercase()),
        Ast::Call { name, args } => {
            let name = name.to_ascii_uppercase();
            match (name.as_str(), args.as_slice()) {
                ("DTOS", [field]) => single_field(field),
                ("STR", [field, rest @ ..]) if rest.len() <= 2 && rest.iter().all(constant) => {
                    single_field(field)
                }
                _ => None,
            }
        }
        Ast::Binary {
            op: Op::Add | Op::Sub | Op::Mul | Op::Div,
            left,
            right,
        } => match (single_field(left), single_field(right)) {
            (Some(a), Some(b)) if a == b => Some(a),
            (Some(a), None) if constant(right) => Some(a),
            (None, Some(b)) if constant(left) => Some(b),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_as_the_original_engine_reports_them() {
        let plain = [
            "k_char",
            "k_integer",
            "k_char;k_integer",
            "k_char+k_char",
            "STR(k_integer,11)",
            "STR(k_numeric,10,2)",
            "DTOS(k_date)",
            "k_integer*2",
            "k_integer+1",
            "F1",
            "F11;F1;F16",
        ];
        for e in plain {
            assert_eq!(key_kind(e), KeyKind::Plain, "{e}");
        }
        assert_eq!(key_kind("DESCEND(k_char)"), KeyKind::Descend);
        let computed = [
            "UPPER(k_char)",
            "LOWER(k_char)",
            "k_char+k_varchar",
            "DTOC(k_date)",
            "SUBSTR(k_char,1,2)",
            "LEFT(k_char,2)",
            "TRIM(k_char)",
            "YEAR(k_date)",
            "IIF(k_logical,'a','b')",
            "UPPER(k_char)+DTOS(k_date)",
            "DTOS(k_date)+k_char",
            "STR(k_integer,11)+k_char",
            "UPPER(k_char);k_integer",
            "RECNO()",
            "DELETED()",
            "CTOD('01/01/2000')",
        ];
        for e in computed {
            assert_eq!(key_kind(e), KeyKind::Computed, "{e}");
        }
    }
}
