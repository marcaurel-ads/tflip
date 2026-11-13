//! How far an AOF's expression is optimized: which of its parts the table's
//! index orders find, and the part they do not, as the original engine
//! decides and prints them (`AdsGetAOFOptLevel`; the help's "AOF
//! Optimization", checked against `fixtures/write/aofs`).
//!
//! A part is found by an order when it compares the order's key expression
//! (on the left; or the first field of a key of several, or a leading part
//! of a key of text parts joined by `+`) with a value that does not depend on
//! the record, with `=`, `==`, `<>`, `<`, `<=`, `>`, `>=` or `$` (not by an
//! order of a dynamic collation); when it is the key itself (a logical one),
//! `EMPTY()` or `ISNULL()` of it (alone or compared with a logical value), or
//! `YEAR()` of a date key (or of a `DTOS()` key's date); or when it is a
//! constant. `.AND.` and `.OR.` combine
//! the parts as the help's table says, and `.NOT.` of a part found is found
//! in a CDX table; in an ADT table, it is found partly if the part is a term
//! (but `.NOT.` of `EMPTY()` or `ISNULL()` alone fully), else not at all
//! (`fixtures/write/aofs_more`).
//!
//! The original engine parses `.AND.` and `.OR.` from the right and keeps
//! their operands swapped: an expression not optimized at all is printed
//! that way (`a .OR. b` is `(b.OR.a)`). Of a partly optimized one it prints
//! what is left to evaluate: the parts not found of an `.AND.` (its plain
//! terms first, in order, then what is left of the others, last first), and
//! an `.OR.` with one part found partly as its other parts and what is left
//! of that one; parts it copies print in their order.
//!
//! A full-text term (`CONTAINS(field,"condition")=.T.`), which the expression
//! engine cannot compile, is a part the caller describes ([`FtsTerm`]): how far
//! the field's full-text order answers it, and the records it finds. One the
//! order only narrows is found partly, and left to evaluate unless the order
//! found nothing for it; an `.OR.` with such a term as a side is printed whole,
//! as the original engine keeps it (`fixtures/sql/fts_plans.json`).

use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::check::{Kind, Node};
use crate::func::Func;
use crate::lex::{self, Op, Token};
use crate::{ErrorKind, Expr, Schema};

/// How far an expression is optimized: `ADS_OPTIMIZED_FULL`,
/// `ADS_OPTIMIZED_PART` or `ADS_OPTIMIZED_NONE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Level {
    Full,
    Partial,
    None,
}

impl Level {
    /// The ACE constant.
    pub fn ace(self) -> u16 {
        match self {
            Level::Full => 1,
            Level::Partial => 2,
            Level::None => 3,
        }
    }
}

/// What the orders tell of the records an expression holds on.
#[derive(Debug, Clone, PartialEq)]
pub enum Bound {
    /// Nothing: any record may hold.
    All,
    /// Exactly the records `expr` holds on, which order `order` finds (none
    /// for a constant); `compare` is how, where the term compares the key
    /// with a value.
    Exact {
        expr: Expr,
        order: Option<usize>,
        compare: Option<Compare>,
    },
    And(Box<Bound>, Box<Bound>),
    Or(Box<Bound>, Box<Bound>),
    /// The records an exact bound leaves out.
    Not(Box<Bound>),
    /// The records a full-text order finds for a full-text term: exactly those
    /// it holds for, or (for a term the order only narrows) a superset.
    Fts(Vec<u32>),
}

impl Bound {
    fn and(a: Bound, b: Bound) -> Bound {
        match (a, b) {
            (Bound::All, b) => b,
            (a, Bound::All) => a,
            (a, b) => Bound::And(Box::new(a), Box::new(b)),
        }
    }

    fn or(a: Bound, b: Bound) -> Bound {
        match (a, b) {
            (Bound::All, _) | (_, Bound::All) => Bound::All,
            (a, b) => Bound::Or(Box::new(a), Box::new(b)),
        }
    }
}

/// A term that compares an order's key with a value: `key op value`.
#[derive(Debug, Clone, PartialEq)]
pub struct Compare {
    pub op: Comparison,
    /// The value, which does not depend on the record.
    pub value: Expr,
    /// For a term `YEAR()` of the key's date (`YEAR(born) = 1990`), what
    /// the key is; then the value is a year, not a key's value.
    pub year: Option<Box<Year>>,
    /// For a term on the leading part of a key of text parts joined by `+`
    /// (`a` of a FoxPro key `A+C`), the part's width; `None` where the term
    /// compares the whole key (or the first field of an ADI key of several
    /// fields).
    pub lead: Option<usize>,
    /// Where the term is in the expression's text, in characters.
    pub term: Option<Range<usize>>,
    /// Where the term's left operand is in the expression's text, in
    /// characters (for a comparison).
    pub left: Option<Range<usize>>,
}

/// A term that compares `YEAR()` of a date with a year, which an order on
/// the date, or on `DTOS()` of it, finds.
#[derive(Debug, Clone, PartialEq)]
pub struct Year {
    /// Whether the order's key is `DTOS()` of the date, not the date.
    pub dtos: bool,
    /// The date's text, as the expression gives it.
    pub date: String,
    /// Where the term is in the expression's text, in characters.
    pub term: Range<usize>,
}

/// How a term compares a key with a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    /// `=`, which compares text as far as the value goes unless the exact
    /// setting is on.
    Eq,
    /// `==`
    Exactly,
    /// `<>`, `#`, `!=`
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// `$`: the key's text is in the value's.
    Within,
    /// `EMPTY()` of the key (`.T.`, the value), or such a term compared with
    /// a logical value: whether the key is an empty value's (NULL in an ADT
    /// table, blank in a DBF table).
    Empty,
    /// `ISNULL()` of the key, as [`Comparison::Empty`]: whether the key is
    /// NULL's.
    Null,
}

/// An expression's optimization.
#[derive(Debug, Clone, PartialEq)]
pub struct Optimized {
    pub level: Level,
    /// The part not optimized, as the original engine prints it; empty for
    /// a full optimization.
    pub residual: String,
    /// What the orders find.
    pub bound: Bound,
    /// Its terms as a plan details them (see [`Detail`]).
    pub details: Vec<Detail>,
    /// The parts of its top `.AND.`, which the details are of.
    pub parts: Vec<Part>,
}

/// A line of the "AOF Optimization Details" of a live SQL cursor's plan: a
/// term, or two terms of an `.AND.` that bound one order's keys from both
/// sides (`id>=1 AND id<=3`), as the original engine lists them: an
/// `.OR.`'s last first, an `.AND.`'s terms last first and then its other
/// parts in order.
#[derive(Debug, Clone, PartialEq)]
pub struct Detail {
    pub text: String,
    /// The order that finds its records; `None` for none.
    pub order: Option<usize>,
    /// Its terms: each one's expression and how it compares the key.
    pub terms: Vec<(Expr, Option<Compare>)>,
    /// Whether it is an `ISNULL()` with a `.NOT.` before it, whose records
    /// the original engine counts (of other terms, it counts their own).
    pub negated: bool,
    /// The full-text term it is, by its place in [`Orders::fts`].
    pub fts: Option<usize>,
    /// Whether its records are looked up: not under a part the orders cannot
    /// optimize at all.
    pub looked_up: bool,
    /// The part of the expression's top `.AND.` it is of, by its place in
    /// [`Optimized::parts`].
    pub part: usize,
}

/// A part of an expression's top `.AND.` (the whole expression if it is no
/// `.AND.`): how its records follow from those its detail lines find.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// A term: its line's.
    Term,
    /// An `.OR.` of terms: those of any of its lines.
    Or,
    /// A `.NOT.` of a term: the records its line leaves out.
    Not,
    /// Anything else: not known from its lines.
    Other,
}

/// The orders an expression may be optimized with: the key expressions of
/// those that serve (see the module's documentation for which).
pub struct Orders<'a> {
    pub keys: &'a [Expr],
    /// Whether each key's order weighs its text by a dynamic collation; such
    /// an order does not find `$` (`fixtures/write/aofs_rest`).
    pub collated: &'a [bool],
    /// Whether `.NOT.` of a part found is found too (CDX), or only partly
    /// (ADI).
    pub negation_found: bool,
    /// The full-text terms of the expression.
    pub fts: &'a [FtsTerm],
}

/// A full-text term of an expression: its text, as the expression has it
/// (`CONTAINS(body,"medical")=.T.`), how far the field's full-text order
/// answers it (`None` for a field with no order, or a condition the order
/// cannot answer), and the records the order finds for it.
#[derive(Debug, Clone, PartialEq)]
pub struct FtsTerm {
    pub text: String,
    pub level: Level,
    pub records: Vec<u32>,
}

/// Optimizes the filter `text` for the table `schema` describes. Fails as
/// [`Expr::compile`] does.
pub fn optimize(text: &str, schema: &dyn Schema, orders: &Orders) -> Result<Optimized, ErrorKind> {
    // The full-text terms are logical, but the expression engine cannot
    // compile them.
    let mut plain = text.to_owned();
    for term in orders.fts {
        plain = plain.replace(&term.text, ".T.");
    }
    Expr::compile(&plain, schema)?;
    let (tokens, spans) = lex::lex_spans(text, Some(schema));
    let chars: Vec<char> = text.chars().collect();
    let tree = Logic {
        tokens: &tokens,
        at: 0,
    }
    .or();
    let context = Context {
        schema,
        orders,
        tokens: &tokens,
        spans: &spans,
        chars: &chars,
    };
    let (level, bound) = context.level(&tree)?;
    // `.NOT.` of a term on NULL keys of an ADT table alone is found fully;
    // with other parts, partly (`!ISNULL(a) .AND. !ISNULL(b)`).
    let level = match &tree {
        Tree::Not(a) if level == Level::Partial && context.is_null_like(a)? => Level::Full,
        _ => level,
    };
    let residual = match level {
        Level::Full => String::new(),
        Level::None => context.swapped(&tree),
        Level::Partial => context.residual(&tree)?.unwrap_or_default(),
    };
    let details = context.details(&tree)?;
    let items = chain(&tree, |t| match t {
        Tree::And(a, b) => Some((a, b)),
        _ => None,
    });
    let parts = items
        .iter()
        .map(|item| match item {
            Tree::Term(_) => Part::Term,
            Tree::Not(a) if matches!(**a, Tree::Term(_)) => Part::Not,
            Tree::Or(..) => {
                let or = chain(item, |t| match t {
                    Tree::Or(a, b) => Some((a, b)),
                    _ => None,
                });
                match or.iter().all(|t| matches!(t, Tree::Term(_))) {
                    true => Part::Or,
                    false => Part::Other,
                }
            }
            _ => Part::Other,
        })
        .collect();
    Ok(Optimized {
        level,
        residual,
        bound,
        details,
        parts,
    })
}

/// The operands of a chain of one operator (`split` says what a node joins, if
/// it is one), first to last.
fn chain(tree: &Tree, split: impl Fn(&Tree) -> Option<(&Box<Tree>, &Box<Tree>)>) -> Vec<&Tree> {
    let mut items = Vec::new();
    let mut rest = tree;
    while let Some((a, b)) = split(rest) {
        items.push(&**a);
        rest = b;
    }
    items.push(rest);
    items
}

/// An expression's logical structure: its terms, joined by `.AND.`, `.OR.`
/// and `.NOT.` (from the right, as the original engine parses them).
#[derive(Debug)]
enum Tree {
    /// A term: a range of the tokens.
    Term(Range<usize>),
    And(Box<Tree>, Box<Tree>),
    Or(Box<Tree>, Box<Tree>),
    Not(Box<Tree>),
}

/// Splits tokens into a [`Tree`]; the expression is known to compile.
struct Logic<'t> {
    tokens: &'t [Token],
    at: usize,
}

impl Logic<'_> {
    fn or(&mut self) -> Tree {
        let left = self.and();
        if self.tokens.get(self.at) == Some(&Token::Or) {
            self.at += 1;
            return Tree::Or(Box::new(left), Box::new(self.or()));
        }
        left
    }

    fn and(&mut self) -> Tree {
        let left = self.unary();
        if self.tokens.get(self.at) == Some(&Token::And) {
            self.at += 1;
            return Tree::And(Box::new(left), Box::new(self.and()));
        }
        left
    }

    /// A `.NOT.`, a parenthesized expression or a term. `.NOT.` binds more
    /// tightly than comparisons, so one followed by a comparison is part of
    /// a term, as is a parenthesized expression an operator follows.
    fn unary(&mut self) -> Tree {
        let start = self.at;
        let end = self.term_end(start);
        let operator = (start..end)
            .any(|i| matches!(self.tokens[i], Token::Op(_)) && self.depth(start, i) == 0);
        match self.tokens.get(start) {
            Some(Token::Not) if !operator => {
                self.at += 1;
                return Tree::Not(Box::new(self.unary()));
            }
            Some(Token::Open) if self.closing(start) == Some(end - 1) => {
                self.at += 1;
                let inner = self.or();
                self.at += 1;
                return inner;
            }
            _ => {}
        }
        self.at = end;
        Tree::Term(start..end)
    }

    /// Where the operand starting at `start` ends: at the next `.AND.`,
    /// `.OR.` or unmatched `)` outside parentheses, or the end.
    fn term_end(&self, start: usize) -> usize {
        let mut depth = 0usize;
        for (i, token) in self.tokens.iter().enumerate().skip(start) {
            match token {
                Token::Open => depth += 1,
                Token::Close if depth == 0 => return i,
                Token::Close => depth -= 1,
                Token::And | Token::Or if depth == 0 => return i,
                _ => {}
            }
        }
        self.tokens.len()
    }

    /// The parentheses open at token `i`, counting from `start`.
    fn depth(&self, start: usize, i: usize) -> usize {
        let mut depth = 0usize;
        for token in &self.tokens[start..i] {
            match token {
                Token::Open => depth += 1,
                Token::Close => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        depth
    }

    /// The `)` that closes the `(` at `open`.
    fn closing(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (i, token) in self.tokens.iter().enumerate().skip(open) {
            match token {
                Token::Open => depth += 1,
                Token::Close => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }
}

struct Context<'a> {
    schema: &'a dyn Schema,
    orders: &'a Orders<'a>,
    tokens: &'a [Token],
    spans: &'a [Range<usize>],
    chars: &'a [char],
}

impl Context<'_> {
    fn level(&self, tree: &Tree) -> Result<(Level, Bound), ErrorKind> {
        Ok(match tree {
            Tree::Term(range) if let Some(i) = self.fts(range) => {
                let term = &self.orders.fts[i];
                match term.level {
                    Level::None => (Level::None, Bound::All),
                    level => (level, Bound::Fts(term.records.clone())),
                }
            }
            Tree::Term(range) => {
                let expr = Expr::compile(&self.source(range), self.schema)?;
                match self.found(&expr.node) {
                    Some(order) => {
                        let key = order.map(|order| &self.orders.keys[order].node);
                        let term = self.spans[range.start].start..self.spans[range.end - 1].end;
                        let compare = key.and_then(|key| {
                            compare(&expr.node, key).or_else(|| self.year(range, &expr.node, key))
                        });
                        let left = self.left_operand(range);
                        let compare = compare.map(|c| Compare {
                            term: Some(term),
                            left,
                            ..c
                        });
                        (
                            Level::Full,
                            Bound::Exact {
                                expr,
                                order,
                                compare,
                            },
                        )
                    }
                    None => (Level::None, Bound::All),
                }
            }
            Tree::And(a, b) => {
                let ((la, ba), (lb, bb)) = (self.level(a)?, self.level(b)?);
                let level = match (la, lb) {
                    (Level::Full, Level::Full) => Level::Full,
                    (Level::None, Level::None) => Level::None,
                    _ => Level::Partial,
                };
                (level, Bound::and(ba, bb))
            }
            Tree::Or(a, b) => {
                let ((la, ba), (lb, bb)) = (self.level(a)?, self.level(b)?);
                let level = match (la, lb) {
                    (Level::Full, Level::Full) => Level::Full,
                    (Level::None, _) | (_, Level::None) => Level::None,
                    _ => Level::Partial,
                };
                (level, Bound::or(ba, bb))
            }
            Tree::Not(a) => match self.level(a)? {
                (Level::Full, bound) if self.orders.negation_found => {
                    (Level::Full, Bound::Not(Box::new(bound)))
                }
                // In an ADT table, only of a term, as its keys leave out
                // NULL; but the records not NULL are found (and alone, it is
                // found fully: see [`optimize`]).
                (Level::Full, bound) if self.is_null_like(a)? => {
                    (Level::Partial, Bound::Not(Box::new(bound)))
                }
                (Level::Full, _) if matches!(**a, Tree::Term(_)) => (Level::Partial, Bound::All),
                _ => (Level::None, Bound::All),
            },
        })
    }

    /// The full-text term the tokens `range` are, by its place in
    /// [`Orders::fts`].
    fn fts(&self, range: &Range<usize>) -> Option<usize> {
        let source = self.source(range);
        self.orders.fts.iter().position(|term| term.text == source)
    }

    /// How far `tree` is optimized once its full-text terms are looked up: a
    /// term the order only narrows, but finds no record for, leaves nothing to
    /// evaluate.
    fn resolved(&self, tree: &Tree) -> Result<Level, ErrorKind> {
        Ok(match tree {
            Tree::Term(range) if let Some(i) = self.fts(range) => {
                let term = &self.orders.fts[i];
                match term.level {
                    Level::Partial if term.records.is_empty() => Level::Full,
                    level => level,
                }
            }
            Tree::And(a, b) => match (self.resolved(a)?, self.resolved(b)?) {
                (Level::Full, Level::Full) => Level::Full,
                (Level::None, Level::None) => Level::None,
                _ => Level::Partial,
            },
            Tree::Or(a, b) => match (self.resolved(a)?, self.resolved(b)?) {
                (Level::Full, Level::Full) => Level::Full,
                (Level::None, _) | (_, Level::None) => Level::None,
                _ => Level::Partial,
            },
            tree => self.level(tree)?.0,
        })
    }

    /// Whether `tree` is a full-text term the order only narrows (and finds
    /// records for).
    fn narrowed_term(&self, tree: &Tree) -> Result<bool, ErrorKind> {
        Ok(
            matches!(tree, Tree::Term(range) if self.fts(range).is_some())
                && self.resolved(tree)? == Level::Partial,
        )
    }

    /// Whether `tree` is a term `ISNULL()` of a key.
    fn is_null_term(&self, tree: &Tree) -> Result<bool, ErrorKind> {
        let Tree::Term(range) = tree else {
            return Ok(false);
        };
        if self.fts(range).is_some() {
            return Ok(false);
        }
        let expr = Expr::compile(&self.source(range), self.schema)?;
        Ok(matches!(expr.node.kind, Kind::Call(Func::IsNull, _)))
    }

    /// Whether `tree` is a term that holds on the records whose key is NULL
    /// in an ADT table: `ISNULL()` or `EMPTY()` of a key (which is NULL
    /// there).
    fn is_null_like(&self, tree: &Tree) -> Result<bool, ErrorKind> {
        let Tree::Term(range) = tree else {
            return Ok(false);
        };
        if self.fts(range).is_some() {
            return Ok(false);
        }
        let expr = Expr::compile(&self.source(range), self.schema)?;
        Ok(matches!(
            expr.node.kind,
            Kind::Call(Func::IsNull | Func::Empty, _)
        ))
    }

    /// The detail lines of `tree` (see [`Detail`]).
    fn details(&self, tree: &Tree) -> Result<Vec<Detail>, ErrorKind> {
        let mut details = self.parts(tree)?;
        // The orders look nothing up for a part they cannot optimize.
        if !matches!(tree, Tree::Term(_)) && self.level(tree)?.0 == Level::None {
            details
                .iter_mut()
                .for_each(|detail| detail.looked_up = false);
        }
        Ok(details)
    }

    /// The detail lines of `tree`'s parts (see [`Context::details`]).
    fn parts(&self, tree: &Tree) -> Result<Vec<Detail>, ErrorKind> {
        Ok(match tree {
            Tree::Term(range) => vec![self.detail(range)?],
            // Of an `.OR.`, as of an `.AND.`, the parts that are not terms
            // come first, in order; then the terms, last first.
            Tree::Or(..) => {
                let items = chain(tree, |t| match t {
                    Tree::Or(a, b) => Some((a, b)),
                    _ => None,
                });
                let mut out = Vec::new();
                for item in &items {
                    if !matches!(item, Tree::Term(_)) {
                        out.extend(self.details(item)?);
                    }
                }
                for item in items.iter().rev() {
                    if let Tree::Term(range) = item {
                        out.push(self.detail(range)?);
                    }
                }
                out
            }
            Tree::Not(a) => {
                let mut out = self.details(a)?;
                if self.is_null_term(a)?
                    && let [detail] = out.as_mut_slice()
                {
                    detail.negated = true;
                }
                out
            }
            Tree::And(..) => {
                let items = chain(tree, |t| match t {
                    Tree::And(a, b) => Some((a, b)),
                    _ => None,
                });
                // The parts that are not terms come first, in order; then
                // the terms, last first. Each line says which part it is of.
                let mut out = Vec::new();
                for (part, item) in items.iter().enumerate() {
                    if !matches!(item, Tree::Term(_)) {
                        let mut details = self.details(item)?;
                        details.iter_mut().for_each(|detail| detail.part = part);
                        out.extend(details);
                    }
                }
                let mut terms: Vec<Detail> = Vec::new();
                for (part, item) in items.iter().enumerate().rev() {
                    let Tree::Term(range) = item else { continue };
                    let detail = Detail {
                        part,
                        ..self.detail(range)?
                    };
                    // Two bounds of one order's keys are one line, in order.
                    let side = |d: &Detail| match d.terms.as_slice() {
                        [(_, Some(c))] => match c.op {
                            Comparison::Gt | Comparison::Ge => Some(false),
                            Comparison::Lt | Comparison::Le => Some(true),
                            _ => None,
                        },
                        _ => None,
                    };
                    let pair = terms.iter_mut().find(|t| {
                        t.order.is_some()
                            && t.order == detail.order
                            && side(t).is_some()
                            && side(&detail).is_some()
                            && side(t) != side(&detail)
                    });
                    // The pair's line has its lower bound first.
                    match pair {
                        Some(t) if side(&detail) == Some(false) => {
                            t.text = format!("{} AND {}", detail.text, t.text);
                            t.terms.insert(0, detail.terms[0].clone());
                        }
                        Some(t) => {
                            t.text = format!("{} AND {}", t.text, detail.text);
                            t.terms.push(detail.terms[0].clone());
                        }
                        None => terms.push(detail),
                    }
                }
                out.extend(terms);
                out
            }
        })
    }

    fn detail(&self, range: &Range<usize>) -> Result<Detail, ErrorKind> {
        if let Some(i) = self.fts(range) {
            let text = self.term(range);
            let terms = Vec::new();
            let fts = Some(i);
            let negated = false;
            return Ok(Detail {
                text,
                order: None,
                terms,
                negated,
                fts,
                looked_up: true,
                part: 0,
            });
        }
        let expr = Expr::compile(&self.source(range), self.schema)?;
        let order = self.found(&expr.node).flatten();
        let compare = order.and_then(|order| compare(&expr.node, &self.orders.keys[order].node));
        let terms = vec![(expr, compare)];
        let text = self.term(range);
        Ok(Detail {
            text,
            order,
            terms,
            negated: false,
            fts: None,
            looked_up: true,
            part: 0,
        })
    }

    /// The order that finds the records the term `node` holds on: `Some`
    /// of it, or `Some(None)` for a constant; `None` if none does.
    fn found(&self, node: &Node) -> Option<Option<usize>> {
        if !varies(node) {
            return Some(None);
        }
        let key = |node: &Node, within: bool| {
            self.orders.keys.iter().enumerate().position(|(i, key)| {
                leads(&key.node, node) && !(within && self.orders.collated.get(i) == Some(&true))
            })
        };
        match &node.kind {
            // `EMPTY()` or `ISNULL()` of a key compared with a logical value.
            Kind::Binary(Op::Eq | Op::Exactly | Op::Ne, left, right)
                if matches!(right.kind, Kind::Const(crate::Val::Logical(_)))
                    && matches!(left.kind, Kind::Call(Func::Empty | Func::IsNull, _)) =>
            {
                self.found(left)
            }
            Kind::Binary(
                op @ (Op::Eq | Op::Exactly | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::In),
                left,
                right,
            ) if !varies(right) => {
                if let Some(order) = key(left, *op == Op::In) {
                    return Some(Some(order));
                }
                // YEAR() of a date that is a key, compared by `=`; or that a
                // DTOS() key holds, compared by any comparison.
                let Kind::Call(Func::Year, args) = &left.kind else {
                    return None;
                };
                let date = args.first()?;
                let dtos = |key: &Node| matches!(&first_part(key).kind, Kind::Call(Func::DToS, a) if a.first() == Some(date));
                let by_date = match op {
                    Op::Eq => key(date, false),
                    _ => None,
                };
                let order = match by_date {
                    Some(order) => order,
                    None if *op == Op::In => return None,
                    None => self.orders.keys.iter().position(|key| dtos(&key.node))?,
                };
                Some(Some(order))
            }
            Kind::Call(Func::Empty | Func::IsNull, args) => key(args.first()?, false).map(Some),
            _ if node.ty == crate::Type::Logical => key(node, false).map(Some),
            _ => None,
        }
    }

    /// How the term in `range`, `node`, compares `YEAR()` of the date the
    /// key `key` is (or holds by `DTOS()`) with a year; `None` if it does
    /// not.
    fn year(&self, range: &Range<usize>, node: &Node, key: &Node) -> Option<Compare> {
        let Kind::Binary(op, left, right) = &node.kind else {
            return None;
        };
        let Kind::Call(Func::Year, args) = &left.kind else {
            return None;
        };
        let date = args.first()?;
        // The date's tokens: those inside `YEAR(`'s parentheses.
        if self.tokens.get(range.start + 1) != Some(&Token::Open) {
            return None;
        }
        let close = Logic {
            tokens: self.tokens,
            at: 0,
        }
        .closing(range.start + 1)?;
        if close <= range.start + 2 || close >= range.end {
            return None;
        }
        let year = Year {
            dtos: !leads(key, date),
            date: self.source(&(range.start + 2..close)),
            term: self.spans[range.start].start..self.spans[range.end - 1].end,
        };
        Some(Compare {
            op: comparison(op)?,
            value: Expr {
                node: (**right).clone(),
            },
            year: Some(Box::new(year)),
            lead: None,
            term: None,
            left: None,
        })
    }

    /// Where the left operand of the comparison the tokens `range` are is in
    /// the expression's text, in characters; `None` if they are none.
    fn left_operand(&self, range: &Range<usize>) -> Option<Range<usize>> {
        let mut depth = 0usize;
        for i in range.clone() {
            match &self.tokens[i] {
                Token::Open => depth += 1,
                Token::Close => depth = depth.saturating_sub(1),
                Token::Op(
                    Op::Eq | Op::Exactly | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::In,
                ) if depth == 0 && i > range.start => {
                    return Some(self.spans[range.start].start..self.spans[i - 1].end);
                }
                _ => {}
            }
        }
        None
    }

    /// The text of the tokens in `range`.
    fn source(&self, range: &Range<usize>) -> String {
        let (first, last) = (&self.spans[range.start], &self.spans[range.end - 1]);
        self.chars[first.start..last.end].iter().collect()
    }

    /// A term as the original engine prints it: without spaces, fields by
    /// the names the table gives them, functions, aliases and logical
    /// values in capitals, and the rest as written.
    fn term(&self, range: &Range<usize>) -> String {
        let mut out = String::new();
        for i in range.clone() {
            let written: String = self.chars[self.spans[i].clone()].iter().collect();
            let next = self.tokens.get(i + 1);
            match &self.tokens[i] {
                Token::Ident(name) if matches!(next, Some(Token::Open | Token::Alias)) => {
                    out.push_str(&name.to_ascii_uppercase())
                }
                Token::Ident(name) => match self.schema.field(name) {
                    Some(field) => match self.schema.field_name(field.index) {
                        Some(declared) => out.push_str(&declared),
                        None => out.push_str(name),
                    },
                    None => out.push_str(&name.to_ascii_uppercase()),
                },
                Token::Alias => out.push_str("->"),
                Token::Logical(_) | Token::Null | Token::Not | Token::And | Token::Or => {
                    out.push_str(&written.to_ascii_uppercase())
                }
                _ => out.push_str(&written),
            }
        }
        out
    }

    /// `tree` as the original engine keeps it: `.AND.` and `.OR.` swapped.
    fn swapped(&self, tree: &Tree) -> String {
        match tree {
            Tree::Term(range) => self.term(range),
            Tree::And(a, b) => format!("{}.AND.{}", self.swapped(b), self.swapped(a)),
            Tree::Or(a, b) => format!("({}.OR.{})", self.swapped(b), self.swapped(a)),
            Tree::Not(a) => format!("!({})", self.swapped(a)),
        }
    }

    /// `tree` as the original engine copies it: in order.
    fn plain(&self, tree: &Tree) -> String {
        match tree {
            Tree::Term(range) => self.term(range),
            Tree::And(a, b) => format!("{}.AND.{}", self.plain(a), self.plain(b)),
            Tree::Or(a, b) => format!("({}.OR.{})", self.plain(a), self.plain(b)),
            Tree::Not(a) => format!("!({})", self.plain(a)),
        }
    }

    /// What is left to evaluate of `tree` when the orders find part of it:
    /// `None` for nothing.
    fn residual(&self, tree: &Tree) -> Result<Option<String>, ErrorKind> {
        let level = self.resolved(tree)?;
        Ok(match (level, tree) {
            (Level::Full, _) => None,
            (Level::None, tree) => Some(self.plain(tree)),
            (_, Tree::And(a, b)) => {
                let mut parts = Vec::new();
                for item in [a, b] {
                    if matches!(**item, Tree::Term(_)) {
                        parts.extend(self.residual(item)?);
                    }
                }
                for item in [b, a] {
                    if !matches!(**item, Tree::Term(_)) {
                        parts.extend(self.residual(item)?);
                    }
                }
                (!parts.is_empty()).then(|| parts.join(".AND."))
            }
            (_, Tree::Or(a, b)) => {
                let (la, lb) = (self.resolved(a)?, self.resolved(b)?);
                // A side the full-text order only narrows keeps it whole too.
                if la == Level::Partial && lb == Level::Partial
                    || self.narrowed_term(a)?
                    || self.narrowed_term(b)?
                {
                    return Ok(Some(self.swapped(tree)));
                }
                // A side found partly that leaves nothing to evaluate (a
                // `.NOT.` in an ADT table) leaves nothing of the `.OR.`.
                let mut nothing = true;
                for (item, level) in [(a, la), (b, lb)] {
                    if level != Level::Full {
                        nothing &= self.residual(item)?.is_none();
                    }
                }
                if nothing {
                    return Ok(None);
                }
                let kept = |item: &Tree, level| -> Result<Option<String>, ErrorKind> {
                    match level {
                        Level::Full => Ok(Some(self.plain(item))),
                        _ => self.residual(item),
                    }
                };
                let mut parts = Vec::new();
                for (item, level) in [(a, la), (b, lb)] {
                    if matches!(**item, Tree::Term(_)) {
                        parts.extend(kept(item, level)?);
                    }
                }
                for (item, level) in [(b, lb), (a, la)] {
                    if !matches!(**item, Tree::Term(_)) {
                        parts.extend(kept(item, level)?);
                    }
                }
                Some(format!("({})", parts.join(".OR.")))
            }
            // `.NOT.` of a part found in an ADT table: the original engine
            // leaves nothing to evaluate.
            (_, Tree::Not(_)) => None,
            // A full-text term the order only narrows is evaluated.
            (_, Tree::Term(range)) if self.fts(range).is_some() => Some(self.term(range)),
            (_, Tree::Term(_)) => None,
        })
    }
}

/// How the term `node` compares the key `key` (or its first segment, or a
/// leading part of it) with a value: a comparison, a logical key alone (`=
/// .T.`), or `EMPTY()` or `ISNULL()` of the key (alone, `.T.`, or compared
/// with a logical value); `None` if it does otherwise (`YEAR()` of the
/// key).
fn compare(node: &Node, key: &Node) -> Option<Compare> {
    let truth = |b| Node {
        ty: crate::Type::Logical,
        kind: Kind::Const(crate::Val::Logical(b)),
    };
    fn empty_of<'n>(node: &'n Node, key: &Node) -> Option<(Comparison, &'n Node)> {
        let (op, args) = match &node.kind {
            Kind::Call(Func::Empty, args) => (Comparison::Empty, args),
            Kind::Call(Func::IsNull, args) => (Comparison::Null, args),
            _ => return None,
        };
        args.first().filter(|a| leads(key, a)).map(|arg| (op, arg))
    }
    let (op, value, left) = match &node.kind {
        Kind::Binary(op @ (Op::Eq | Op::Exactly | Op::Ne), left, right)
            if let Some((empty, arg)) = empty_of(left, key)
                && let Kind::Const(crate::Val::Logical(b)) = right.kind =>
        {
            // `= .T.` holds on the empty keys, `= .F.` on the others.
            (empty, truth(b != (*op == Op::Ne)), arg)
        }
        Kind::Binary(op, left, right) if leads(key, left) => {
            (comparison(op)?, (**right).clone(), &**left)
        }
        Kind::Field(_) if node.ty == crate::Type::Logical && node == key => {
            (Comparison::Eq, truth(true), node)
        }
        _ if let Some((empty, arg)) = empty_of(node, key) => (empty, truth(true), arg),
        _ => return None,
    };
    let lead = (left != key && matches!(key.kind, Kind::Binary(Op::Add, ..)))
        .then(|| usize::from(left.ty.width()));
    Some(Compare {
        op,
        value: Expr { node: value },
        year: None,
        lead,
        term: None,
        left: None,
    })
}

/// The comparison a term's operator makes.
fn comparison(op: &Op) -> Option<Comparison> {
    Some(match op {
        Op::Eq => Comparison::Eq,
        Op::Exactly => Comparison::Exactly,
        Op::Ne => Comparison::Ne,
        Op::Lt => Comparison::Lt,
        Op::Le => Comparison::Le,
        Op::Gt => Comparison::Gt,
        Op::Ge => Comparison::Ge,
        Op::In => Comparison::Within,
        _ => return None,
    })
}

/// Whether `node`'s value depends on the record.
fn varies(node: &Node) -> bool {
    match &node.kind {
        Kind::Const(_) => false,
        Kind::Field(_) => true,
        Kind::Call(Func::RecNo | Func::Deleted, _) => true,
        Kind::Call(_, args) => args.iter().any(varies),
        Kind::Binary(_, a, b) | Kind::And(a, b) | Kind::Or(a, b) | Kind::Join(a, b) => {
            varies(a) || varies(b)
        }
        Kind::Not(a) => varies(a),
    }
}

/// Whether the key `key` is `node`, or a key of segments (`a;b`) whose
/// first is, or a key of text parts joined by `+` (a FoxPro key of several
/// fields, `A+C`) whose leading parts are (`a`, `a+c`).
fn leads(key: &Node, node: &Node) -> bool {
    match &key.kind {
        _ if key == node => true,
        Kind::Join(first, _) => leads(first, node),
        Kind::Binary(Op::Add, first, _) if matches!(key.ty, crate::Type::String { .. }) => {
            leads(first, node)
        }
        _ => false,
    }
}

/// The first part of the key `key`: of its segments (`a;b`), or of its text
/// parts joined by `+`.
fn first_part(key: &Node) -> &Node {
    match &key.kind {
        Kind::Join(first, _) => first_part(first),
        Kind::Binary(Op::Add, first, _) if matches!(key.ty, crate::Type::String { .. }) => {
            first_part(first)
        }
        _ => key,
    }
}

#[cfg(test)]
mod tests {
    use tflip_storage::field::FieldType;

    use super::*;
    use crate::Field;

    /// The probes' table (`fixtures/write/aofs`): its fields and the keys of
    /// the orders that serve (`k2`'s has a condition).
    struct People;

    const FIELDS: [(&str, FieldType, u16, u16); 9] = [
        ("id", FieldType::Integer, 4, 0),
        ("name", FieldType::Character, 10, 0),
        ("ciname", FieldType::CiCharacter, 10, 0),
        ("amount", FieldType::Money, 8, 4),
        ("born", FieldType::Date, 4, 0),
        ("active", FieldType::Logical, 1, 0),
        ("code", FieldType::Integer, 4, 0),
        ("k2", FieldType::Integer, 4, 0),
        ("u", FieldType::Integer, 4, 0),
    ];

    impl Schema for People {
        fn alias(&self) -> &str {
            "p"
        }

        fn field(&self, name: &str) -> Option<Field> {
            let index = FIELDS.iter().position(|f| f.0.eq_ignore_ascii_case(name))?;
            let (_, field_type, length, decimals) = FIELDS[index];
            Some(Field {
                index,
                field_type,
                length,
                decimals,
            })
        }

        fn field_name(&self, index: usize) -> Option<String> {
            Some(FIELDS[index].0.to_owned())
        }
    }

    /// What the original engine reported for each filter on the table (with
    /// one record found or more by the orders where some are left to
    /// evaluate: with none, it reports nothing left).
    #[test]
    fn levels_and_residuals_are_the_original_engines() {
        let keys: Vec<Expr> = [
            "id",
            "name",
            "ciname",
            "born",
            "active",
            "amount",
            "name;id",
            "UPPER(name)",
            "u",
        ]
        .iter()
        .map(|key| Expr::compile(key, &People).unwrap())
        .collect();
        let orders = Orders {
            keys: &keys,
            collated: &[],
            negation_found: false,
            fts: &[],
        };
        let cases: &[(&str, u16, &str)] = &[
            ("id = 3", 1, ""),
            ("id > 3", 1, ""),
            ("id >= 3", 1, ""),
            ("id < 3", 1, ""),
            ("id <> 3", 1, ""),
            ("id # 3", 1, ""),
            ("id != 3", 1, ""),
            ("3 = id", 3, "3=id"),
            ("3 < id", 3, "3<id"),
            ("id == 3", 1, ""),
            ("id = 3 .AND. code = 1", 2, "code=1"),
            ("code = 1 .AND. id = 3", 2, "code=1"),
            ("id = 3 .OR. code = 1", 3, "(code=1.OR.id=3)"),
            ("id = 3 .OR. name = 'Carl'", 1, ""),
            (".NOT. (id = 3 .AND. code = 1)", 3, "!(code=1.AND.id=3)"),
            ("!(id=3)", 2, ""),
            ("!(code=1)", 3, "!(code=1)"),
            ("name = 'Bob'", 1, ""),
            ("name = 'B'", 1, ""),
            ("name == 'Bob'", 1, ""),
            ("UPPER(name) = 'BOB'", 1, ""),
            ("upper(name)='BOB'", 1, ""),
            ("UPPER( name ) = 'BOB'", 1, ""),
            ("ciname = 'bob'", 1, ""),
            ("name $ 'Bob'", 1, ""),
            ("'B' $ name", 3, "'B'$name"),
            ("EMPTY(name)", 1, ""),
            ("EMPTY(id)", 1, ""),
            ("YEAR(born) = 1990", 1, ""),
            ("born = CTOD('01/05/1990')", 1, ""),
            ("born > STOD('19900101')", 1, ""),
            ("active", 1, ""),
            ("!active", 2, ""),
            (".NOT. active", 2, ""),
            ("active = .T.", 1, ""),
            ("active = .F.", 1, ""),
            (".T.", 1, ""),
            (".F.", 1, ""),
            ("code = 1", 3, "code=1"),
            ("id = code", 3, "id=code"),
            ("DELETED()", 3, "DELETED()"),
            ("!DELETED()", 3, "!(DELETED())"),
            ("id > 2 .AND. id < 5", 1, ""),
            ("id >= 2 .AND. id <= 5 .AND. code = 1", 2, "code=1"),
            ("(id = 1 .OR. id = 2) .AND. code = 1", 2, "code=1"),
            ("id = 1 .OR. (id = 2 .AND. code = 1)", 2, "(id=1.OR.code=1)"),
            ("amount > 25", 1, ""),
            ("k2 > 4", 3, "k2>4"),
            ("k2 = 5", 3, "k2=5"),
            ("u = 3", 1, ""),
            ("name = 'Bob' .AND. id = 7", 1, ""),
            ("id = 3 .and. code = 1 .and. name = 'Carl'", 2, "code=1"),
            (
                "code = 1 .and. code = 2 .or. id = 3",
                3,
                "(id=3.OR.code=2.AND.code=1)",
            ),
            ("ISNULL(id)", 1, ""),
            ("id = 3 + 0", 1, ""),
            ("id + 0 = 3", 3, "id+0=3"),
            ("name = UPPER('bob')", 1, ""),
            (
                "id = 1 .OR. code = 1 .OR. id = 2",
                3,
                "((id=2.OR.code=1).OR.id=1)",
            ),
            (
                "id = 1 .AND. (code = 1 .OR. code = 2)",
                2,
                "(code=1.OR.code=2)",
            ),
            ("RECNO() = 3", 3, "RECNO()=3"),
            ("id = 3.5", 1, ""),
            ("id = 3.0", 1, ""),
            ("LEFT(name, 1) = 'B'", 3, "LEFT(name,1)='B'"),
            ("CODE = 1", 3, "code=1"),
            ("Code=1", 3, "code=1"),
            ("  code   =   1  ", 3, "code=1"),
            ("code = 1 .OR. code = 2", 3, "(code=2.OR.code=1)"),
            (
                "code = 1 .OR. code = 2 .OR. code = 3",
                3,
                "((code=3.OR.code=2).OR.code=1)",
            ),
            (
                "(code = 1 .OR. code = 2) .OR. id = 3",
                3,
                "(id=3.OR.(code=2.OR.code=1))",
            ),
            (
                "id = 3 .OR. (code = 1 .OR. code = 2)",
                3,
                "((code=2.OR.code=1).OR.id=3)",
            ),
            ("code = 1 .AND. code = 2", 3, "code=2.AND.code=1"),
            (
                "code = 1 .AND. code = 2 .AND. code = 3",
                3,
                "code=3.AND.code=2.AND.code=1",
            ),
            (
                "id = 1 .AND. code = 1 .AND. code = 2",
                2,
                "code=1.AND.code=2",
            ),
            (
                "code = 1 .AND. id = 1 .AND. code = 2",
                2,
                "code=1.AND.code=2",
            ),
            (
                "code = 1 .AND. code = 2 .AND. id = 1",
                2,
                "code=1.AND.code=2",
            ),
            (
                "id = 1 .OR. code = 1 .AND. code = 2",
                3,
                "(code=2.AND.code=1.OR.id=1)",
            ),
            ("id = 1 .OR. (code = 2 .AND. id = 2)", 2, "(id=1.OR.code=2)"),
            ("(id = 2 .AND. code = 1) .OR. id = 1", 2, "(id=1.OR.code=1)"),
            (
                "(id = 2 .AND. code = 1) .OR. (id = 3 .AND. code = 2)",
                2,
                "(code=2.AND.id=3.OR.code=1.AND.id=2)",
            ),
            ("id = 1 .OR. code = 1", 3, "(code=1.OR.id=1)"),
            ("code = 1 .OR. id = 1", 3, "(id=1.OR.code=1)"),
            ("!(code = 1 .OR. code = 2)", 3, "!((code=2.OR.code=1))"),
            ("!(code = 1) .AND. id = 1", 2, "!(code=1)"),
            ("!(id = 1 .OR. code = 1)", 3, "!((code=1.OR.id=1))"),
            ("!(id = 1 .AND. id = 2)", 3, "!(id=2.AND.id=1)"),
            ("!(id = 1) .AND. code = 1", 2, "code=1"),
            ("!(id = 1 .OR. id = 2)", 3, "!((id=2.OR.id=1))"),
            ("name = \"Bob\"", 1, ""),
            ("code = 1.50", 3, "code=1.50"),
            ("code = -1", 3, "code=-1"),
            ("code = +1", 3, "code=+1"),
            ("code <> 1", 3, "code<>1"),
            ("code # 1", 3, "code#1"),
            ("code != 1", 3, "code!=1"),
            ("code >= 1", 3, "code>=1"),
            ("code == 1", 3, "code==1"),
            ("name $ 'abc'", 1, ""),
            ("code = 1 .and. .t.", 2, "code=1"),
            (".T. .AND. code = 1", 2, "code=1"),
            ("id = 1 .AND. .T.", 1, ""),
            ("id = 1 .OR. .F.", 1, ""),
            ("code = 1 .OR. .T.", 3, "(.T..OR.code=1)"),
            ("UPPER(name) = 'BOB' .AND. code = 1", 2, "code=1"),
            ("SUBSTR(name, 1, 2) = 'Bo'", 3, "SUBSTR(name,1,2)='Bo'"),
            ("code + 1 = 2", 3, "code+1=2"),
            ("code * 2 = 2", 3, "code*2=2"),
            ("code = id + 1", 3, "code=id+1"),
            ("TRIM(name) = 'Bob'", 3, "TRIM(name)='Bob'"),
            ("name = 'Bob' + ''", 1, ""),
            ("IIF(code = 1, .T., .F.)", 3, "IIF(code=1,.T.,.F.)"),
            ("code = 1 .AND. (id = 1 .OR. id = 2)", 2, "code=1"),
            (
                "code = 1 .OR. (id = 1 .AND. id = 2)",
                3,
                "(id=2.AND.id=1.OR.code=1)",
            ),
            ("(code = 1)", 3, "code=1"),
            ("((code = 1))", 3, "code=1"),
            ("(id = 1)", 1, ""),
            ("!(!(code = 1))", 3, "!(!(code=1))"),
            ("EMPTY(code)", 3, "EMPTY(code)"),
            ("ISNULL(code)", 3, "ISNULL(code)"),
            ("active .AND. code = 1", 2, "code=1"),
            ("!active .AND. code = 1", 2, "code=1"),
            ("left(name, 1) = 'B'", 3, "LEFT(name,1)='B'"),
            ("upper(name) = 'BOB' .AND. code = 1", 2, "code=1"),
            ("deleted()", 3, "DELETED()"),
            ("TRIM(name) = \"Bob\"", 3, "TRIM(name)=\"Bob\""),
            ("TRIM(name) = [Bob]", 3, "TRIM(name)=[Bob]"),
            ("TRIM(name) = 'B o'", 3, "TRIM(name)='B o'"),
            (
                "id = 1 .AND. (code = 1 .OR. code = 2 .OR. code = 3)",
                2,
                "(code=1.OR.(code=2.OR.code=3))",
            ),
            (
                "id = 1 .AND. !(code = 1 .OR. code = 2)",
                2,
                "!((code=1.OR.code=2))",
            ),
            ("id = 1 .AND. ISNULL(code)", 2, "ISNULL(code)"),
            ("code = 1 .OR. .t.", 3, "(.T..OR.code=1)"),
            ("1 = 1", 1, ""),
            ("1 = 1 .AND. code = 1", 2, "code=1"),
            ("1 = 2 .OR. code = 1", 3, "(code=1.OR.1=2)"),
            (
                "id = 1 .OR. id = 2 .OR. (id = 3 .AND. code = 1)",
                2,
                "(id=1.OR.(id=2.OR.code=1))",
            ),
            (
                "(id = 1 .OR. (id = 2 .AND. code = 1)) .AND. code = 2",
                2,
                "code=2.AND.(id=1.OR.code=1)",
            ),
            ("p->id = 1", 1, ""),
            ("p->code = 1", 3, "P->code=1"),
            ("id = 1 .AND. code = 1", 2, "code=1"),
            ("id = 1 .AND. code = 99", 2, "code=99"),
            ("EMPTY(name) .AND. code = 1", 2, "code=1"),
            ("YEAR(born) = 1990 .AND. code = 1", 2, "code=1"),
            ("1 = code", 3, "1=code"),
            ("id = 1 .AND. (code = 1)", 2, "code=1"),
            (".NOT.(code = 1)", 3, "!(code=1)"),
            (
                "id = 1 .AND. code = 1 .OR. id = 2 .AND. code = 2",
                2,
                "(code=2.AND.id=2.OR.code=1.AND.id=1)",
            ),
            (
                "code = 1 .AND. id = 1 .OR. code = 2",
                3,
                "(code=2.OR.id=1.AND.code=1)",
            ),
            ("id = 1 .OR. code = 1 .AND. id = 2", 2, "(id=1.OR.code=1)"),
            (
                "code = 1 .AND. (id = 1 .OR. id = 2) .AND. code = 2",
                2,
                "code=1.AND.code=2",
            ),
            (
                "id = 1 .AND. code = 1 .AND. code = 2 .AND. code = 3",
                2,
                "code=1.AND.code=2.AND.code=3",
            ),
            (
                "id = 1 .AND. (code = 1 .AND. code = 2)",
                2,
                "code=1.AND.code=2",
            ),
            (
                "code = 1 .AND. id < 3 .AND. code = 2 .AND. id > 0",
                2,
                "code=1.AND.code=2",
            ),
            (
                "!(code = 1) .AND. !(code = 2) .AND. id = 1",
                2,
                "!(code=2).AND.!(code=1)",
            ),
            ("id = 1 .AND. (id = 2 .OR. code = 1)", 2, "(id=2.OR.code=1)"),
            ("IIF(.T., id, 0) = 1", 3, "IIF(.T.,id,0)=1"),
            ("id = 1 .and. code + id = 2", 2, "code+id=2"),
            ("name = 'Bob' .AND. ISNULL(code)", 2, "ISNULL(code)"),
            ("ciname = 'BOB' .AND. code = 1", 2, "code=1"),
            ("active .AND. code = 1 .AND. id = 1", 2, "code=1"),
            ("active = .T. .AND. code = 1", 2, "code=1"),
            ("id = 1 .AND. code = 1.0", 2, "code=1.0"),
            ("id = 1 .AND. code = -1.5", 2, "code=-1.5"),
            (
                "id = 1 .AND. born = STOD('19900105') .AND. code = 1",
                2,
                "code=1",
            ),
            ("id = 1 .AND. born > CTOD('01/01/1990')", 1, ""),
            ("id = 1 .AND. TRIM(name) = CHR(66)", 2, "TRIM(name)=CHR(66)"),
        ];
        let mut wrong = Vec::new();
        for &(text, level, residual) in cases {
            let got = optimize(text, &People, &orders).unwrap();
            if (got.level.ace(), got.residual.as_str()) != (level, residual) {
                wrong.push(format!(
                    "{text}: {:?} {:?}, not {level} {residual:?}",
                    got.level, got.residual
                ));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
